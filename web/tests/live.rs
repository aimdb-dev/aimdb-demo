//! The computer page's hub with several senders at once: the on-page node
//! and a simulated fleet, each on its own route (DESIGN.md §4, §5);
//! and flashing, which must end the old instance (§9).
#![cfg(target_arch = "wasm32")]
mod common;
use common::*;

use live_graphs::node::{Firmware, NodeGateway};
use live_graphs::wasm::rt;
use live_graphs::{Bus, SCHEME};
use live_web::LiveHub;
use serde::Deserialize;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[derive(Deserialize, Debug)]
struct View {
    frames_total: usize,
    contracts: bool,
    raw: Raw,
    graph: Vec<GraphNode>,
    counts: Counts,
    nodes: Vec<String>,
    admitted: Option<Admitted>,
    frames: Vec<Frame>,
}

#[derive(Deserialize, Debug)]
struct GraphNode {
    place: String,
    kind: String,
    firmware: String,
    verdict: Option<String>,
    disagree: Option<bool>,
    accepted: u64,
    upgraded: u64,
    rejected: u64,
}

#[derive(Deserialize, Debug, PartialEq)]
struct Raw {
    read: u64,
    disagree: u64,
}

#[derive(Deserialize, Debug, PartialEq)]
struct Readings {
    dashboard: Option<f64>,
    told: Option<f64>,
    lenient: Option<f64>,
}

#[derive(Deserialize, Debug)]
struct Counts {
    accepted: u64,
    upgraded: u64,
    too_new: u64,
    shape_mismatch: u64,
    malformed: u64,
}

#[derive(Deserialize, Debug)]
struct Admitted {
    place: String,
    power_mw: u32,
}

#[derive(Deserialize, Debug)]
struct Frame {
    place: String,
    kind: String,
    verdict: Option<String>,
    readings: Option<Readings>,
}

fn view(hub: &LiveHub) -> View {
    serde_wasm_bindgen::from_value(hub.view().unwrap()).unwrap()
}

/// Six senders, five firmwares, all in the same tick and in rounds: every
/// frame gets its own route's verdict, never a neighbour's.
#[wasm_bindgen_test]
async fn every_frame_gets_its_own_verdict_while_the_fleet_sends() {
    let hub = LiveHub::start(String::new()).await;
    let senders = [
        ("here", "v1", "upgraded"),
        ("roof-east", "v2", "accepted"),
        ("line-3", "v3", "too_new"),
        ("pump-12", "unbumped", "shape_mismatch"),
        ("dock-b", "none", "malformed"),
        ("lab", "v1", "upgraded"),
    ];
    // The firmware the graph shows, as the hub reads it from each frame.
    fn firmware(fw: &str) -> &str {
        match fw {
            "v1" => "@1",
            "v2" => "@2",
            "v3" => "@3",
            other => other,
        }
    }
    let mut nodes = Vec::new();
    for (place, fw, _) in senders {
        let n = hub.node(place.into()).await.unwrap();
        n.flash(fw.into()).await.unwrap();
        nodes.push(n);
    }
    sleep(30).await;

    const ROUNDS: usize = 5;
    for round in 0..ROUNDS {
        // No await between sends: the frames are interleaved as tightly as
        // the page can make them.
        for (i, n) in nodes.iter().enumerate() {
            n.send(300 + (round * 10 + i) as u16);
        }
        sleep(5).await;
    }
    let total = ROUNDS * senders.len();
    until("every frame seen", 2000, || {
        view(&hub).frames_total == total
    })
    .await;

    let v = view(&hub);
    assert_eq!(v.frames.len(), total);
    for f in &v.frames {
        let (_, _, want) = senders.iter().find(|s| s.0 == f.place).unwrap();
        assert_eq!(f.verdict.as_deref(), Some(*want), "{f:?}");
        assert_eq!(f.kind, if f.place == "here" { "here" } else { "sim" });
    }
    let r = ROUNDS as u64;
    assert_eq!(v.counts.accepted, 3 * r, "two v1 senders and one v2: {v:?}");
    assert_eq!(v.counts.upgraded, 2 * r);
    assert_eq!(v.counts.too_new, r);
    assert_eq!(v.counts.shape_mismatch, r);
    assert_eq!(v.counts.malformed, r);
    assert!(v.nodes.is_empty(), "no visitor has joined: {:?}", v.nodes);
    // The graph: one node per sender, with its firmware, last verdict and
    // its own route's counters (DESIGN.md §7).
    assert_eq!(v.graph.len(), senders.len(), "{:?}", v.graph);
    for (place, fw, want) in senders {
        let g = v.graph.iter().find(|g| g.place == place).unwrap();
        assert_eq!(g.firmware, firmware(fw), "{g:?}");
        assert_eq!(g.verdict.as_deref(), Some(want), "{g:?}");
        assert_eq!(g.kind, if place == "here" { "here" } else { "sim" });
        let ok = matches!(want, "accepted" | "upgraded");
        assert_eq!(g.accepted, if ok { r } else { 0 }, "{g:?}");
        assert_eq!(g.upgraded, if want == "upgraded" { r } else { 0 }, "{g:?}");
        assert_eq!(g.rejected, if ok { 0 } else { r }, "{g:?}");
    }
    // The stage shows the on-page node, not the fleet.
    until("the on-page node's value", 2000, || {
        view(&hub).admitted.is_some_and(|a| a.place == "here")
    })
    .await;
    let a = view(&hub).admitted.unwrap();
    assert_eq!(a.power_mw % 1000, 0, "watts migrated to milliwatts: {a:?}");
}

/// Visitors' places are not local nodes: a page cannot send as a visitor.
#[wasm_bindgen_test]
async fn a_page_node_cannot_take_a_visitors_place() {
    let hub = LiveHub::start(String::new()).await;
    assert!(hub.node("1".into()).await.is_err());
    assert!(hub.node("nowhere".into()).await.is_err());
    assert!(hub.node("here".into()).await.is_ok());
}

/// A stopped node sends nothing: a gateway that left the fleet is gone.
#[wasm_bindgen_test]
async fn a_stopped_node_sends_nothing() {
    let hub = LiveHub::start(String::new()).await;
    let n = hub.node("carport".into()).await.unwrap();
    sleep(20).await;
    n.send(100);
    until("first frame", 2000, || view(&hub).frames_total == 1).await;
    n.stop();
    n.send(100);
    sleep(100).await;
    assert_eq!(view(&hub).frames_total, 1);
}

/// A send right after a flash arrives: `flash` resolves once the new
/// instance is ready. The fleet flashes and sends in one tick.
#[wasm_bindgen_test]
async fn a_send_right_after_a_flash_arrives() {
    let hub = LiveHub::start(String::new()).await;
    let n = hub.node("lab".into()).await.unwrap();
    for (i, fw) in ["v1", "v3", "v2", "unbumped", "none", "v1"]
        .iter()
        .enumerate()
    {
        n.flash(fw.to_string()).await.unwrap();
        n.send(200);
        until("the frame after a flash", 2000, || {
            view(&hub).frames_total == i + 1
        })
        .await;
    }
    let v = view(&hub);
    let verdicts: Vec<_> = v
        .frames
        .iter()
        .rev()
        .map(|f| f.verdict.clone().unwrap())
        .collect();
    assert_eq!(
        verdicts,
        [
            "upgraded",
            "too_new",
            "accepted",
            "shape_mismatch",
            "malformed",
            "upgraded"
        ]
    );
}

/// What a flash leaves behind, with and without aborting the old runner.
#[wasm_bindgen_test]
async fn flashing_ends_the_old_runner_only_when_aborted() {
    use std::cell::Cell;
    use std::rc::Rc;
    /// Set when the spawned runner task is dropped: completed or aborted.
    struct Done(Rc<Cell<bool>>);
    impl Drop for Done {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    /// (database freed, runner ended)
    async fn flash_once(abort: bool) -> (bool, bool) {
        let bus = Bus::new(SCHEME);
        let (gw, built) = NodeGateway::build(rt(), Firmware::V2, &bus, "1").await;
        let built = built.unwrap();
        let weak = std::sync::Arc::downgrade(built.db.inner());
        let ended = Rc::new(Cell::new(false));
        let guard = Done(ended.clone());
        let (run, handle) = futures_util::future::abortable(built.runner.run());
        wasm_bindgen_futures::spawn_local(async move {
            let _guard = guard;
            let _ = run.await;
        });
        sleep(20).await;
        gw.send(1, 1, 326);
        sleep(20).await;
        built.plugs.iter().for_each(|p| p.unplug());
        drop(built.db);
        drop(gw);
        if abort {
            handle.abort();
        }
        sleep(100).await;
        (weak.upgrade().is_none(), ended.get())
    }
    // The runner's tasks hold no strong reference to the database: it is
    // freed either way. Unplugging alone leaves the runner's task set
    // (buffers, connector pump) pending for the life of the page.
    assert_eq!(flash_once(false).await, (true, false), "unplug only");
    assert_eq!(flash_once(true).await, (true, true), "aborted");
}

/// The contracts switch (DESIGN.md §6): with contracts off, the node
/// sends its firmware's plain JSON, nothing is checked or rejected, and the
/// three baseline receivers read it. AimDB's counters do not move.
#[wasm_bindgen_test]
async fn with_contracts_off_every_value_passes_and_the_receivers_guess() {
    let hub = LiveHub::start(String::new()).await;
    let n = hub.node("here".into()).await.unwrap();
    n.send(326);
    until("checked frame", 2000, || view(&hub).frames_total == 1).await;
    assert_eq!(view(&hub).frames[0].verdict.as_deref(), Some("accepted"));

    hub.set_contracts(false);
    assert!(!view(&hub).contracts);
    assert_eq!(n.resend(), Some(2), "the last reading, again");
    until("raw frame", 2000, || view(&hub).frames_total == 2).await;
    let v = view(&hub);
    let f = &v.frames[0];
    assert_eq!(f.verdict, None, "nothing is checked: {f:?}");
    assert_eq!(
        f.readings,
        Some(Readings {
            dashboard: None,
            told: Some(326.0),
            lenient: Some(326_000.0)
        })
    );
    assert_eq!(
        v.raw,
        Raw {
            read: 1,
            disagree: 1
        }
    );
    let g = &v.graph[0];
    assert_eq!(
        (g.firmware.as_str(), g.verdict.as_deref(), g.disagree),
        ("JSON", None, Some(true)),
        "without a contract the hub cannot tell the firmware: {g:?}"
    );
    assert_eq!(
        (v.counts.accepted, v.counts.malformed),
        (1, 0),
        "AimDB's counters stay"
    );

    // The old firmware's watts read the same everywhere.
    n.flash("v1".into()).await.unwrap();
    n.send(383);
    until("v1 raw frame", 2000, || view(&hub).frames_total == 3).await;
    let v = view(&hub);
    assert_eq!(
        v.frames[0].readings,
        Some(Readings {
            dashboard: Some(383.0),
            told: Some(383.0),
            lenient: Some(383.0)
        })
    );
    assert_eq!(
        v.raw,
        Raw {
            read: 2,
            disagree: 1
        }
    );

    // Even @3 and the forgotten bump pass: there is nothing to check.
    for fw in ["v3", "unbumped", "none"] {
        n.flash(fw.into()).await.unwrap();
        n.send(200);
    }
    until("three more", 2000, || view(&hub).frames_total == 6).await;
    let v = view(&hub);
    assert!(v
        .frames
        .iter()
        .take(3)
        .all(|f| f.verdict.is_none() && f.readings.is_some()));
    assert_eq!(
        v.raw,
        Raw {
            read: 5,
            disagree: 4
        }
    );

    // Back on: checked again, and the counters for "off" start over next time.
    hub.set_contracts(true);
    n.flash("v1".into()).await.unwrap();
    n.resend();
    until("checked again", 2000, || view(&hub).frames_total == 7).await;
    assert_eq!(view(&hub).frames[0].verdict.as_deref(), Some("upgraded"));
}
