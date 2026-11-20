#![no_std]
//! Node firmware newer than the hub knows: power_reading@3 adds a field.
//! A hub built from revision 2 rejects it as `too_new`; the sample is
//! missing with a reason, never a wrong value.
use aimdb_data_contracts::SchemaType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PowerReadingV3 {
    pub channel: u8,
    pub sample_s: u64,
    pub seq: u32,
    pub power_mw: u32,
    /// Which input string the sensor sits on. Unknown to the hub.
    pub string_id: u8,
}
impl SchemaType for PowerReadingV3 {
    const NAME: &'static str = "power_reading";
    const VERSION: u32 = 3;
}

aimdb_data_contracts::catalog! { contracts: [PowerReadingV3], helpers: [] }
aimdb_data_contracts::migration_chain! { type Current = PowerReadingV3; steps {} }
