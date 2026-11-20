#![no_std]
//! Revision 1 of the demo's one contract, `power_reading`. Immutable once
//! merged: a change is a new revision.
//!
//! The name carries no industry: one power reading from one channel of a
//! node.

use aimdb_data_contracts::SchemaType;
use serde::{Deserialize, Serialize};

/// power_reading@1: one power sample, in whole watts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PowerReading {
    pub channel: u8,
    /// Unix seconds.
    pub sample_s: u64,
    pub seq: u32,
    /// Whole watts. Zero is a valid reading; a missing sample is no value.
    pub power_w: u16,
}
impl SchemaType for PowerReading {
    const NAME: &'static str = "power_reading";
    const VERSION: u32 = 1;
}

aimdb_data_contracts::catalog! { contracts: [PowerReading], helpers: [] }
aimdb_data_contracts::migration_chain! { type Current = PowerReading; steps {} }
