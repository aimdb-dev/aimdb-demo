//! Running instances in a browser page, shared by the computer page's
//! bundle and the node page's bundle.

use crate::node::{Firmware, NodeGateway};
use crate::{Built, Bus, PlugHandle};
use aimdb_core::AimDb;
use aimdb_wasm_adapter::WasmAdapter;
use futures_util::future::{abortable, AbortHandle};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn rt() -> Arc<WasmAdapter> {
    Arc::new(WasmAdapter)
}

/// A running instance that can be stopped.
pub struct Running {
    pub db: AimDb,
    plugs: Vec<PlugHandle>,
    /// Cancels the instance: dropping `runner.run()` drops every task it
    /// drives (see `AimDbRunner`). Replace with the runner's own stop handle
    /// once aimdb-dev/aimdb#495 lands.
    abort: AbortHandle,
}

impl Running {
    /// Unplugs the instance and ends its tasks. Without the abort, every
    /// flash would leave the old runner's tasks pending until the page
    /// closes (DESIGN.md §9).
    pub fn stop(&self) {
        self.plugs.iter().for_each(PlugHandle::unplug);
        self.abort.abort();
    }
}

/// Runs `b` on the page's executor, abortably.
pub fn spawn(b: Built) -> Running {
    let Built { db, runner, plugs } = b;
    let (run, abort) = abortable(runner.run());
    wasm_bindgen_futures::spawn_local(async move {
        let _ = run.await;
    });
    Running { db, plugs, abort }
}

/// How long a new instance gets to start its tasks before it sends.
const START_MS: i32 = 20;

/// Whether the hub has contracts switched on. The computer page owns it;
/// a phone's copy follows the hub's `mode` frames.
pub type Contracts = Arc<AtomicBool>;

pub fn contracts_on() -> Contracts {
    Arc::new(AtomicBool::new(true))
}

/// A node that can be flashed: one gateway at a time on its place.
pub struct FlashableNode {
    bus: Bus,
    place: String,
    gw: RefCell<Option<(NodeGateway, Option<Running>)>>,
    seq: Cell<u32>,
    contracts: Contracts,
    /// The last reading sent, in watts, for [`resend`](Self::resend).
    last: Cell<Option<u16>>,
}

impl FlashableNode {
    /// A node at `place` on `bus`, flashed with `v2`. While `contracts` is
    /// off it sends what its firmware sends untyped.
    pub async fn new(bus: Bus, place: String, contracts: Contracts) -> Self {
        let n = FlashableNode {
            bus,
            place,
            gw: RefCell::default(),
            seq: Cell::new(0),
            contracts,
            last: Cell::new(None),
        };
        n.flash(Firmware::V2).await;
        n
    }

    /// Unplugs and stops the old instance, then builds the new firmware.
    /// Resolves once the new instance is ready to send: a value produced
    /// before its outbound task has started would never leave.
    pub async fn flash(&self, fw: Firmware) {
        if let Some((_, Some(old))) = self.gw.borrow_mut().take() {
            old.stop();
        }
        let (gw, built) = NodeGateway::build(rt(), fw, &self.bus, &self.place).await;
        let started = built.is_some();
        *self.gw.borrow_mut() = Some((gw, built.map(spawn)));
        if started {
            page_connector::sleep_ms(START_MS).await;
        }
    }

    pub fn firmware(&self) -> &'static str {
        self.gw
            .borrow()
            .as_ref()
            .map_or("", |(g, _)| g.firmware.name())
    }

    pub fn place(&self) -> &str {
        &self.place
    }

    /// Sends one reading in watts; returns its sequence number. With
    /// contracts on, the firmware's contract frame on `<place>/power`;
    /// with contracts off, its plain JSON on `<place>/raw`.
    pub fn send(&self, power_w: u16) -> u32 {
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        self.last.set(Some(power_w));
        let sample_s = (js_sys::Date::now() / 1000.0) as u64;
        if let Some((gw, _)) = self.gw.borrow().as_ref() {
            if self.contracts() {
                gw.send(sample_s, seq, power_w);
            } else {
                gw.send_untyped(sample_s, seq, power_w);
            }
        }
        seq
    }

    /// Sends the last reading again, in the current mode: what a node does
    /// when the hub flips contracts. `None` before the first send.
    pub fn resend(&self) -> Option<u32> {
        self.last.get().map(|w| self.send(w))
    }

    pub fn contracts(&self) -> bool {
        self.contracts.load(Ordering::Relaxed)
    }

    /// The last reading sent, in watts.
    pub fn last(&self) -> Option<u16> {
        self.last.get()
    }

    /// Unplugs and stops the current instance, for good.
    pub fn stop(&self) {
        if let Some((_, Some(old))) = self.gw.borrow_mut().take() {
            old.stop();
        }
    }
}
