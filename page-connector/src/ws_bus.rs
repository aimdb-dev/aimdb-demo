//! Stretches an in-page [`Bus`] over the relay's `/bus/<code>` route.
//!
//! Frames published on the local bus go out; frames from the relay are
//! delivered to the local bus. While the socket is down, outgoing frames are
//! dropped: the bus does not store and forward. The bridge redials every
//! second until [`BusLink::close`].
use crate::wasm_util::sleep_ms;
use crate::Bus;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Open,
    /// The relay refused the role: another device holds it.
    Taken,
    Closed,
}

struct Inner {
    ws: Option<web_sys::WebSocket>,
    state: LinkState,
    closed_by_page: bool,
    on_state: Option<Box<dyn Fn(LinkState)>>,
    _handlers: Vec<Closure<dyn FnMut(JsValue)>>,
}

/// A running bridge. Dropping it does not stop it; call [`close`].
///
/// [`close`]: BusLink::close
#[derive(Clone)]
pub struct BusLink(Rc<RefCell<Inner>>);

impl BusLink {
    /// `url` is the full route, e.g. `wss://aimdb.dev/bus/<code>?as=3`.
    pub fn connect(bus: &Bus, url: &str) -> Self {
        Self::open(bus, url, true)
    }

    /// Receives only: frames published on the local bus stay in the page.
    /// The live demo's hub, whose simulated fleet and on-page node must not
    /// reach the visitors' phones.
    pub fn listen(bus: &Bus, url: &str) -> Self {
        Self::open(bus, url, false)
    }

    fn open(bus: &Bus, url: &str, uplink: bool) -> Self {
        let link = BusLink(Rc::new(RefCell::new(Inner {
            ws: None,
            state: LinkState::Connecting,
            closed_by_page: false,
            on_state: None,
            _handlers: Vec::new(),
        })));
        // The uplink runs inside `Bus::publish`, on the page's only thread.
        let out = SendCell(link.clone());
        if uplink {
            bus.set_uplink(move |topic, payload| {
                let inner = out.get().0.borrow();
                if let (Some(ws), LinkState::Open) = (&inner.ws, inner.state) {
                    if let Some(frame) = line_protocol::encode(topic, payload) {
                        let _ = ws.send_with_u8_array(&frame);
                    }
                }
            });
        }
        let (bus, url, l) = (bus.clone(), url.to_string(), link.clone());
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                if l.0.borrow().closed_by_page {
                    break;
                }
                l.dial(&bus, &url);
                // Wait until this socket is gone.
                while matches!(l.state(), LinkState::Connecting | LinkState::Open) {
                    sleep_ms(200).await;
                }
                if l.state() == LinkState::Taken || l.0.borrow().closed_by_page {
                    break;
                }
                sleep_ms(1000).await;
            }
            bus.clear_uplink();
        });
        link
    }

    /// Sends one frame to the relay now, outside the local bus: how a
    /// listening hub answers one node. Dropped while the socket is down.
    pub fn send(&self, topic: &str, payload: &[u8]) {
        let inner = self.0.borrow();
        if let (Some(ws), LinkState::Open) = (&inner.ws, inner.state) {
            if let Some(frame) = line_protocol::encode(topic, payload) {
                let _ = ws.send_with_u8_array(&frame);
            }
        }
    }

    pub fn state(&self) -> LinkState {
        self.0.borrow().state
    }

    pub fn on_state(&self, f: impl Fn(LinkState) + 'static) {
        self.0.borrow_mut().on_state = Some(Box::new(f));
    }

    /// Drops the socket, as a locked phone would; the bridge redials.
    pub fn drop_socket(&self) {
        if let Some(ws) = self.0.borrow().ws.as_ref() {
            let _ = ws.close();
        }
    }

    pub fn close(&self) {
        self.0.borrow_mut().closed_by_page = true;
        self.drop_socket();
    }

    fn set_state(&self, s: LinkState) {
        let inner = &mut *self.0.borrow_mut();
        // A refusal is final, whatever close event follows.
        if inner.state == LinkState::Taken && s == LinkState::Closed {
            return;
        }
        inner.state = s;
        if let Some(f) = &inner.on_state {
            f(s);
        }
    }

    fn dial(&self, bus: &Bus, url: &str) {
        self.set_state(LinkState::Connecting);
        let Ok(ws) = web_sys::WebSocket::new(url) else {
            self.set_state(LinkState::Closed);
            return;
        };
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let me = self.clone();
        let on_open = Closure::<dyn FnMut(JsValue)>::new(move |_| me.set_state(LinkState::Open));
        let bus_in = bus.clone();
        let on_message = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let e: web_sys::MessageEvent = e.unchecked_into();
            let bytes = js_sys::Uint8Array::new(&e.data()).to_vec();
            if let Some((topic, payload)) = line_protocol::decode(&bytes) {
                bus_in.deliver_from_uplink(topic, payload);
            }
        });
        let me = self.clone();
        let on_close = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let code = e.dyn_ref::<web_sys::CloseEvent>().map(|c| c.code());
            me.set_state(if code == Some(line_protocol::CLOSE_TAKEN) {
                LinkState::Taken
            } else {
                LinkState::Closed
            });
        });
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        let mut inner = self.0.borrow_mut();
        if let Some(old) = inner.ws.take() {
            old.set_onopen(None);
            old.set_onmessage(None);
            old.set_onclose(None);
        }
        inner.ws = Some(ws);
        inner._handlers = vec![on_open, on_message, on_close];
    }
}

struct SendCell<T>(T);
// SAFETY: wasm32 without atomics has one thread.
unsafe impl<T> Send for SendCell<T> {}

impl<T> SendCell<T> {
    fn get(&self) -> &T {
        &self.0
    }
}
