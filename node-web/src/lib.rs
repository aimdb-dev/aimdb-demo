//! The node page's bundle: one node that can be flashed, and the `/bus`
//! client. The computer page's hub and fleet are not in it (DESIGN.md §2).
//!
//! ```js
//! import init, { Node } from "./pkg-node/node_web.js";
//! await init();
//! const node = await Node.join("wss://aimdb.dev/bus/<code>", "3");
//! await node.flash("v1");   // v1, v2, v3, unbumped, none
//! node.send(326);           // watts; the firmware and the hub's mode decide the wire form
//! node.linkState();         // Connecting, Open, Taken, Closed
//! node.contracts();         // whether the hub has contracts on (DESIGN.md §6)
//! node.feedback();          // the hub's answer to the last reading, JSON, or undefined
//! node.resend();            // the last reading again, in the current mode
//! ```
#![cfg(target_arch = "wasm32")]

use live_graphs::node::Firmware;
use live_graphs::wasm::{contracts_on, Contracts, FlashableNode};
use live_graphs::{feedback_topic, Bus, SCHEME};
use page_connector::ws_bus::BusLink;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Node {
    node: FlashableNode,
    bus: Bus,
    link: BusLink,
    /// The hub's latest answer to this node, as it arrived.
    feedback: Arc<Mutex<Option<String>>>,
}

#[wasm_bindgen]
impl Node {
    /// `bus_url`: the relay's `/bus/<code>`; `place`: `1` to `8`, from the
    /// QR code.
    pub async fn join(bus_url: String, place: String) -> Result<Node, JsError> {
        if place == line_protocol::HQ || !line_protocol::valid_role(&place) {
            return Err(JsError::new("a node's place is 1 to 8"));
        }
        let bus = Bus::new(SCHEME);
        let contracts: Contracts = contracts_on();
        let feedback: Arc<Mutex<Option<String>>> = Arc::default();
        // The hub's frames: its mode, to every node, and its answer to this
        // one. The relay delivers only those two.
        let (c, fb, mine) = (contracts.clone(), feedback.clone(), feedback_topic(&place));
        bus.observe(move |f| {
            if f.topic == line_protocol::MODE {
                c.store(f.payload != b"off", Ordering::Relaxed);
            } else if f.topic == mine {
                *fb.lock().unwrap() = String::from_utf8(f.payload.clone()).ok();
            }
        });
        let link = BusLink::connect(&bus, &format!("{bus_url}?as={place}"));
        Ok(Node {
            node: FlashableNode::new(bus.clone(), place, contracts).await,
            bus,
            link,
            feedback,
        })
    }

    /// Flashes new firmware: `v1`, `v2`, `v3`, `unbumped` or `none`. The old
    /// instance is stopped, not left running.
    pub async fn flash(&self, firmware: String) -> Result<(), JsError> {
        let fw = Firmware::parse(&firmware)
            .ok_or_else(|| JsError::new("firmware is v1, v2, v3, unbumped or none"))?;
        self.node.flash(fw).await;
        Ok(())
    }

    pub fn firmware(&self) -> String {
        self.node.firmware().into()
    }

    pub fn place(&self) -> String {
        self.node.place().into()
    }

    /// Sends one reading in watts; returns its sequence number.
    pub fn send(&self, power_w: u16) -> u32 {
        self.node.send(power_w)
    }

    /// Sends the last reading again, in the current mode.
    pub fn resend(&self) -> Option<u32> {
        self.node.resend()
    }

    /// Whether the hub has contracts switched on.
    pub fn contracts(&self) -> bool {
        self.node.contracts()
    }

    /// The hub's latest answer to this node, as JSON: `{seq, contracts,
    /// sent_w, verdict, admitted_w}` with contracts on, `{seq, contracts,
    /// sent_w, readings}` with contracts off.
    pub fn feedback(&self) -> Option<String> {
        // The bus keeps every frame for tests; a phone polls this, so keep
        // it from growing.
        self.bus.clear_frames();
        self.feedback.lock().unwrap().clone()
    }

    #[wasm_bindgen(js_name = linkState)]
    pub fn link_state(&self) -> String {
        format!("{:?}", self.link.state())
    }
}
