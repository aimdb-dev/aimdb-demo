//! The demo's claim about the baseline, as evidence (DESIGN.md §6, §8):
//! the same bytes, three ordinary receivers, three numbers. With the
//! contract, every receiver reads the same number.
//!
//! The receivers are `live_graphs::baseline`, the same code the hub runs
//! when contracts are switched off. It is about this baseline, not every
//! pipeline.

use aimdb_data_contracts::LinkCodec;
use contracts_r2::PowerReadingV2;
use live_graphs::baseline::read;
use live_graphs::node::{untyped_json, Firmware};
use live_graphs::CODEC;

/// 326 W, sent by the newer firmware in milliwatts.
const SAMPLE_S: u64 = 1_781_613_000;

#[test]
fn three_consumers_read_three_numbers_from_the_same_message() {
    let r = read(untyped_json(Firmware::V2, SAMPLE_S, 30, 326).as_bytes());
    assert_eq!(r.dashboard, None, "NULL, drawn as 0 W");
    assert_eq!(r.told, Some(326.0));
    assert_eq!(r.lenient, Some(326_000.0));
}

/// Fair to the baseline: where the untyped pipeline happens to work, it
/// works. The older firmware's watts read the same everywhere.
#[test]
fn the_untyped_pipeline_works_while_nothing_changes() {
    let r = read(untyped_json(Firmware::V1, SAMPLE_S, 29, 383).as_bytes());
    assert_eq!(
        (r.dashboard, r.told, r.lenient),
        (Some(383.0), Some(383.0), Some(383.0))
    );
}

/// With the contract, the meaning travels with the value: every receiver
/// decodes the same bytes as `power_reading@2` and reads the same watts.
/// They agree by construction: every reader decodes with the same typed
/// contract.
#[test]
fn with_the_contract_every_receiver_reads_the_same_number() {
    let sent = PowerReadingV2 {
        channel: 1,
        sample_s: SAMPLE_S,
        seq: 30,
        power_mw: 326_000,
    };
    let bytes = LinkCodec::<PowerReadingV2>::encode(&CODEC, &sent).unwrap();
    let readings: Vec<f64> = (0..3)
        .map(|_| {
            let v: PowerReadingV2 = LinkCodec::decode(&CODEC, &bytes).unwrap();
            f64::from(v.power_mw) / 1000.0
        })
        .collect();
    assert_eq!(readings, [326.0; 3]);
}
