//! The live demo's node, flashed with each firmware, sending to a hub that
//! knows power_reading @1 and @2. Every verdict below is AimDB's own
//! admission result (DESIGN.md §2, §8).
#![cfg(target_arch = "wasm32")]
mod common;
use common::*;

use live_graphs::hub::{self, Label};
use live_graphs::node::{Firmware, NodeGateway};
use live_graphs::wasm::{rt, spawn, Running};
use live_graphs::{rejected, upgraded, Bus, SCHEME};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const PLACE: &str = "1";

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Counts {
    accepted: u64,
    upgraded: u64,
    too_new: u64,
    shape_mismatch: u64,
    malformed: u64,
    rejected: u64,
}

fn counts(hub: &Running) -> Counts {
    hub::stats(&hub.db, PLACE)
        .map(|s| Counts {
            accepted: s.accepted,
            upgraded: upgraded(&s),
            too_new: s.too_new,
            shape_mismatch: s.shape_mismatch,
            malformed: s.malformed,
            rejected: rejected(&s),
        })
        .unwrap_or_default()
}

/// Flashes the node, sends one reading of 326 W, and returns the hub's
/// counter change, what it admitted and the frame's label.
async fn send(hub: &Running, bus: &Bus, fw: Firmware, seq: u32) -> (Counts, Option<u32>, Label) {
    let (gw, built) = NodeGateway::build(rt(), fw, bus, PLACE).await;
    let running = built.map(spawn);
    sleep(20).await;
    let before = counts(hub);
    bus.clear_frames();
    gw.send(1_781_613_000 + u64::from(seq), seq, 326);
    until("the hub's verdict", 2000, || counts(hub) != before).await;
    let after = counts(hub);
    if after.accepted > before.accepted {
        // The counter moves on admission; the value lands in the buffer next.
        until("the admitted value", 2000, || {
            hub::admitted(&hub.db, PLACE).is_some_and(|v| v.seq == seq)
        })
        .await;
    }
    let label = hub::label(&bus.frames().last().expect("node frame").payload);
    if let Some(r) = running {
        r.stop();
    }
    let delta = Counts {
        accepted: after.accepted - before.accepted,
        upgraded: after.upgraded - before.upgraded,
        too_new: after.too_new - before.too_new,
        shape_mismatch: after.shape_mismatch - before.shape_mismatch,
        malformed: after.malformed - before.malformed,
        rejected: after.rejected - before.rejected,
    };
    let admitted = hub::admitted(&hub.db, PLACE)
        .filter(|v| v.seq == seq)
        .map(|v| v.power_mw);
    (delta, admitted, label)
}

#[wasm_bindgen_test]
async fn every_firmware_gets_its_own_verdict() {
    let bus = Bus::new(SCHEME);
    let hub = spawn(hub::build(rt(), bus.hub(), &[PLACE]).await);
    sleep(20).await;

    // @2: the hub's current version.
    let (d, admitted, label) = send(&hub, &bus, Firmware::V2, 1).await;
    assert_eq!((d.accepted, d.upgraded, d.rejected), (1, 0, 0), "{d:?}");
    assert_eq!(admitted, Some(326_000));
    assert_eq!(label.contract.as_deref(), Some("power_reading"));
    assert_eq!(label.version, Some(2));
    assert_eq!(label.shape, hub::SHAPE_V2);
    assert_eq!(label.field, Some(("power_mw", 326_000)));

    // @1: admitted and migrated, 326 W becomes 326 000 mW.
    let (d, admitted, label) = send(&hub, &bus, Firmware::V1, 2).await;
    assert_eq!((d.accepted, d.upgraded, d.rejected), (1, 1, 0), "{d:?}");
    assert_eq!(admitted, Some(326_000));
    assert_eq!(label.version, Some(1));
    assert_eq!(label.shape, hub::SHAPE_V1);
    assert_eq!(label.field, Some(("power_w", 326)));

    // @3: unknown to the hub.
    let (d, admitted, label) = send(&hub, &bus, Firmware::V3, 3).await;
    assert_eq!((d.accepted, d.too_new, d.rejected), (0, 1, 1), "{d:?}");
    assert_eq!(admitted, None);
    assert_eq!(label.version, Some(3));
    assert_eq!(label.shape, hub::SHAPE_V3);
    assert_eq!(label.field, Some(("power_mw", 326_000)));

    // Milliwatts declared as @1: the shape gives it away.
    let (d, admitted, label) = send(&hub, &bus, Firmware::Unbumped, 4).await;
    assert_eq!(
        (d.accepted, d.shape_mismatch, d.rejected),
        (0, 1, 1),
        "{d:?}"
    );
    assert_eq!(admitted, None);
    assert_eq!(label.version, Some(1));
    assert_eq!(label.shape, hub::SHAPE_V2);

    // No contract: plain JSON, no envelope.
    let (d, admitted, label) = send(&hub, &bus, Firmware::NoContract, 5).await;
    assert_eq!((d.accepted, d.malformed, d.rejected), (0, 1, 1), "{d:?}");
    assert_eq!(admitted, None);
    assert_eq!(label.contract, None);
}
