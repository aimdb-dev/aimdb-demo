//! Connectors for AimDB instances running in one browser page.
//!
//! - [`Bus`]: an in-page broker for one link scheme. Instances attach with a
//!   [`Plug`]: a hub plug sees every frame, a port plug prefixes what its
//!   node publishes (`power` becomes `3/power`) and receives only its
//!   port's frames, with the prefix removed.
//! - `ws_bus` (wasm32): the same bus stretched over the relay's `/bus/<code>`
//!   route, so a node on a phone reaches the hub on a computer.
//!
//! The bus never parses a payload. Checking is the receiving hub's job.

use aimdb_core::{
    AimDb, ConnectorBuilder, DbResult, ExactGrammar, InboundDispatch, OutboundRoutes,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

#[cfg(target_arch = "wasm32")]
mod wasm_util;
#[cfg(target_arch = "wasm32")]
pub use wasm_util::sleep_ms;
#[cfg(target_arch = "wasm32")]
pub mod ws_bus;

type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// One frame that crossed a bus, as the page shows it.
#[derive(Clone, Debug)]
pub struct Frame {
    pub topic: String,
    pub payload: Vec<u8>,
    /// No attached instance had a route for the topic.
    pub unrouted: bool,
}

type Observer = Box<dyn Fn(&Frame) + Send>;
type Uplink = Box<dyn Fn(&str, &[u8]) + Send>;

/// An in-page broker for one link scheme. Cheap to clone.
#[derive(Clone)]
pub struct Bus(Arc<Mutex<BusInner>>);

struct BusInner {
    scheme: &'static str,
    peers: Vec<Peer>,
    next_id: usize,
    frames: Vec<Frame>,
    observers: Vec<Observer>,
    /// Frames that leave the page (the `/bus` client), after the local peers.
    uplink: Option<Uplink>,
}

struct Peer {
    id: usize,
    /// `Some("port1")` for a port plug.
    prefix: Option<String>,
    dispatch: Arc<InboundDispatch>,
}

impl Bus {
    pub fn new(scheme: &'static str) -> Self {
        Self(Arc::new(Mutex::new(BusInner {
            scheme,
            peers: Vec::new(),
            next_id: 0,
            frames: Vec::new(),
            observers: Vec::new(),
            uplink: None,
        })))
    }

    /// A plug that sees every frame on the bus, unchanged.
    pub fn hub(&self) -> Plug {
        Plug {
            bus: self.clone(),
            prefix: None,
            slot: Arc::default(),
            gate: Gate::open(),
        }
    }

    /// A plug for one port: what the node publishes gets the port's
    /// prefix, and it receives only frames under that prefix.
    pub fn port(&self, prefix: impl Into<String>) -> Plug {
        Plug {
            bus: self.clone(),
            prefix: Some(prefix.into()),
            slot: Arc::default(),
            gate: Gate::open(),
        }
    }

    /// Publishes a raw frame as a device outside any AimDB instance would.
    pub fn inject(&self, topic: &str, payload: &[u8]) {
        self.publish(usize::MAX, topic, payload, true);
    }

    /// Delivers a frame that arrived from outside the page; it is not sent
    /// back out.
    pub fn deliver_from_uplink(&self, topic: &str, payload: &[u8]) {
        self.publish(usize::MAX, topic, payload, false);
    }

    pub fn frames(&self) -> Vec<Frame> {
        self.0.lock().unwrap().frames.clone()
    }

    pub fn clear_frames(&self) {
        self.0.lock().unwrap().frames.clear();
    }

    /// Called for every frame, after delivery. The page reads each frame's verdict here.
    pub fn observe(&self, f: impl Fn(&Frame) + Send + 'static) {
        self.0.lock().unwrap().observers.push(Box::new(f));
    }

    pub fn set_uplink(&self, f: impl Fn(&str, &[u8]) + Send + 'static) {
        self.0.lock().unwrap().uplink = Some(Box::new(f));
    }

    pub fn clear_uplink(&self) {
        self.0.lock().unwrap().uplink = None;
    }

    fn attach(&self, prefix: Option<String>, dispatch: Arc<InboundDispatch>) -> usize {
        let mut inner = self.0.lock().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.peers.push(Peer {
            id,
            prefix,
            dispatch,
        });
        id
    }

    fn detach(&self, id: usize) {
        self.0.lock().unwrap().peers.retain(|p| p.id != id);
    }

    fn is_attached(&self, id: usize) -> bool {
        self.0.lock().unwrap().peers.iter().any(|p| p.id == id)
    }

    fn publish(&self, from: usize, topic: &str, payload: &[u8], uplink: bool) {
        // Deliver outside the lock: a dispatch may run code that publishes.
        let targets: Vec<(Arc<InboundDispatch>, String)> = {
            let inner = self.0.lock().unwrap();
            inner
                .peers
                .iter()
                .filter(|p| p.id != from)
                .filter_map(|p| match &p.prefix {
                    None => Some((p.dispatch.clone(), topic.to_string())),
                    Some(prefix) => topic
                        .strip_prefix(prefix.as_str())
                        .and_then(|rest| rest.strip_prefix('/'))
                        .map(|rest| (p.dispatch.clone(), rest.to_string())),
                })
                .collect()
        };
        let mut routed = false;
        for (dispatch, topic) in &targets {
            routed |= dispatch
                .subscriptions()
                .iter()
                .any(|s| s.as_ref() == topic.as_str());
            dispatch.dispatch(topic, payload);
        }
        let mut inner = self.0.lock().unwrap();
        if uplink {
            if let Some(up) = &inner.uplink {
                up(topic, payload);
                // Whether anyone beyond the page has a route is not known here.
                routed = true;
            }
        }
        let frame = Frame {
            topic: topic.into(),
            payload: payload.to_vec(),
            unrouted: !routed,
        };
        for o in &inner.observers {
            o(&frame);
        }
        inner.frames.push(frame);
    }
}

/// Attaches one AimDB instance to a [`Bus`]. Unplugging detaches it: it
/// receives nothing more, and what it publishes is dropped.
pub struct Plug {
    bus: Bus,
    prefix: Option<String>,
    slot: Arc<Mutex<Option<usize>>>,
    gate: Gate,
}

/// Unplugs an instance from its bus. Held by the page after `build`.
#[derive(Clone)]
pub struct PlugHandle {
    bus: Bus,
    slot: Arc<Mutex<Option<usize>>>,
    gate: Gate,
}

impl PlugHandle {
    pub fn unplug(&self) {
        if let Some(id) = self.slot.lock().unwrap().take() {
            self.bus.detach(id);
        }
    }

    /// Stops reading the instance's outbound routes, as a dead link would.
    /// Values stay in the outbound readers' buffers until [`resume`].
    ///
    /// [`resume`]: PlugHandle::resume
    pub fn pause(&self) {
        self.gate.set_open(false);
    }

    pub fn resume(&self) {
        self.gate.set_open(true);
    }
}

impl Plug {
    pub fn handle(&self) -> PlugHandle {
        PlugHandle {
            bus: self.bus.clone(),
            slot: self.slot.clone(),
            gate: self.gate.clone(),
        }
    }
}

impl ConnectorBuilder for Plug {
    fn build<'a>(
        &'a self,
        db: &'a AimDb,
    ) -> Pin<Box<dyn Future<Output = DbResult<Vec<BoxFuture>>> + Send + 'a>> {
        Box::pin(async move {
            let scheme = self.bus.0.lock().unwrap().scheme;
            let inbound = Arc::new(InboundDispatch::new(db, scheme, &ExactGrammar)?);
            let mut outbound = OutboundRoutes::new(db, scheme)?;
            let id = self.bus.attach(self.prefix.clone(), inbound);
            *self.slot.lock().unwrap() = Some(id);
            let bus = self.bus.clone();
            let prefix = self.prefix.clone();
            let gate = self.gate.clone();
            let pump: BoxFuture = Box::pin(async move {
                loop {
                    gate.wait_open().await;
                    let Some(msg) = outbound.next().await else {
                        break;
                    };
                    // A value taken while the link went down waits with it.
                    gate.wait_open().await;
                    if !bus.is_attached(id) {
                        continue;
                    }
                    match &prefix {
                        Some(p) => {
                            let topic = format!("{p}/{}", msg.topic);
                            bus.publish(id, &topic, msg.payload.as_slice(), true)
                        }
                        None => bus.publish(id, msg.topic, msg.payload.as_slice(), true),
                    }
                }
            });
            Ok(vec![pump])
        })
    }

    fn scheme(&self) -> &str {
        // Only read before build; the scheme never changes.
        let scheme: &'static str = self.bus.0.lock().unwrap().scheme;
        scheme
    }

    fn owns_scheme(&self) -> bool {
        true
    }
}

/// Open or closed; a closed gate parks the plug's pump.
#[derive(Clone)]
struct Gate(Arc<Mutex<(bool, Option<core::task::Waker>)>>);

impl Gate {
    fn open() -> Self {
        Self(Arc::new(Mutex::new((true, None))))
    }

    fn set_open(&self, open: bool) {
        let waker = {
            let mut g = self.0.lock().unwrap();
            g.0 = open;
            if open {
                g.1.take()
            } else {
                None
            }
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    fn wait_open(&self) -> impl Future<Output = ()> + Send + '_ {
        core::future::poll_fn(move |cx| {
            let mut g = self.0.lock().unwrap();
            if g.0 {
                core::task::Poll::Ready(())
            } else {
                g.1 = Some(cx.waker().clone());
                core::task::Poll::Pending
            }
        })
    }
}
