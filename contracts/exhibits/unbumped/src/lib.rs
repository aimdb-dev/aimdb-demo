#![no_std]
//! Firmware that changes `power_reading@1` to milliwatts and forgets the
//! version bump. Without a contract this reaches the field, and every
//! consumer that reads `power_w` silently loses the value.
//!
//! A firmware build checks its contracts against the revision it ships
//! with, so the change fails in CI before it ships (test below). The correct
//! change is `power_reading@2` with a migration, as in revision 2. If a
//! build skips CI anyway, the hub still refuses it at runtime: the declared
//! shape does not match @1 (`shape_mismatch`).
use aimdb_data_contracts::SchemaType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PowerReading {
    pub channel: u8,
    pub sample_s: u64,
    pub seq: u32,
    pub power_mw: u32,
}
impl SchemaType for PowerReading {
    const NAME: &'static str = "power_reading";
    const VERSION: u32 = 1;
}

aimdb_data_contracts::catalog! { contracts: [PowerReading], helpers: [] }
aimdb_data_contracts::migration_chain! { type Current = PowerReading; steps {} }

#[cfg(test)]
mod refused {
    extern crate std;
    #[test]
    #[should_panic(expected = "power_reading@1: the shape changed without a VERSION bump")]
    fn by_the_r1_lock() {
        let r1 = std::concat!(std::env!("CARGO_MANIFEST_DIR"), "/../../r1");
        aimdb_data_contracts::lock::check(
            r1,
            &[aimdb_data_contracts::lock::trace::<super::PowerReading>(&[])],
        );
    }
}
