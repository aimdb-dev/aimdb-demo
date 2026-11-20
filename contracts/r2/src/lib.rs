#![no_std]
//! Revision 2 of `power_reading`. Keeps the shape of revision 1, byte for
//! byte, and adds power_reading@2 in milliwatts with a migration from @1.
//! The step widens before multiplying, so every @1 value fits.
//!
//! Immutable once merged.

use aimdb_data_contracts::{migration_chain, MigrationError, MigrationStep, SchemaType};
use serde::{Deserialize, Serialize};

/// The current version.
pub type PowerReading = PowerReadingV2;

/// power_reading@1, unchanged from revision 1: whole watts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PowerReadingV1 {
    pub channel: u8,
    /// Unix seconds.
    pub sample_s: u64,
    pub seq: u32,
    /// Whole watts. Zero is a valid reading; a missing sample is no value.
    pub power_w: u16,
}
impl SchemaType for PowerReadingV1 {
    const NAME: &'static str = "power_reading";
    const VERSION: u32 = 1;
}

/// power_reading@2: milliwatts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PowerReadingV2 {
    pub channel: u8,
    /// Unix seconds.
    pub sample_s: u64,
    pub seq: u32,
    pub power_mw: u32,
}
impl SchemaType for PowerReadingV2 {
    const NAME: &'static str = "power_reading";
    const VERSION: u32 = 2;
}

pub struct PowerReadingV1ToV2;
impl MigrationStep for PowerReadingV1ToV2 {
    type Older = PowerReadingV1;
    type Newer = PowerReadingV2;
    const FROM_VERSION: u32 = 1;
    const TO_VERSION: u32 = 2;
    /// Widen, then multiply: u16::MAX W is 65 535 000 mW, inside u32.
    fn up(v: PowerReadingV1) -> Result<PowerReadingV2, MigrationError> {
        Ok(PowerReadingV2 {
            channel: v.channel,
            sample_s: v.sample_s,
            seq: v.seq,
            power_mw: u32::from(v.power_w) * 1000,
        })
    }
    /// Rounds to whole watts; refuses a value @1 cannot carry.
    fn down(v: PowerReadingV2) -> Result<PowerReadingV1, MigrationError> {
        let w = (u64::from(v.power_mw) + 500) / 1000;
        let power_w = u16::try_from(w).map_err(|_| {
            MigrationError::ConversionFailed("power out of range for power_reading@1")
        })?;
        Ok(PowerReadingV1 {
            channel: v.channel,
            sample_s: v.sample_s,
            seq: v.seq,
            power_w,
        })
    }
}

aimdb_data_contracts::catalog! { contracts: [PowerReadingV1, PowerReadingV2], helpers: [] }
migration_chain! {
    type Current = PowerReadingV2;
    steps { PowerReadingV1ToV2: PowerReadingV1 => PowerReadingV2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A revision never changes a shape an earlier revision locked.
    #[test]
    fn keeps_every_shape_of_r1() {
        let r1 = include_str!("../../r1/aimdb.lock");
        let r2 = include_str!("../aimdb.lock");
        for entry in r1.split("[[contract]]").skip(1) {
            assert!(
                r2.contains(entry.trim_end()),
                "r2 dropped or changed:{entry}"
            );
        }
    }

    fn v1(w: u16) -> PowerReadingV1 {
        PowerReadingV1 {
            channel: 1,
            sample_s: 0,
            seq: 0,
            power_w: w,
        }
    }

    fn v2(mw: u32) -> PowerReadingV2 {
        PowerReadingV2 {
            channel: 1,
            sample_s: 0,
            seq: 0,
            power_mw: mw,
        }
    }

    #[test]
    fn watts_widen_before_multiplying() {
        let up = |w| PowerReadingV1ToV2::up(v1(w)).unwrap().power_mw;
        assert_eq!(up(383), 383_000);
        assert_eq!(up(0), 0);
        assert_eq!(up(u16::MAX), 65_535_000);
    }

    #[test]
    fn milliwatts_round_to_whole_watts_or_refuse() {
        let down = |mw| PowerReadingV1ToV2::down(v2(mw)).map(|v| v.power_w).ok();
        assert_eq!(down(326_000), Some(326));
        assert_eq!(down(326_499), Some(326));
        assert_eq!(down(326_500), Some(327));
        assert!(down(65_535_500).is_none());
    }
}
