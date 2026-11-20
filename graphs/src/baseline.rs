//! The baseline: the three ordinary receivers of explainer scene 5, as
//! code. With contracts switched off, the hub reads every message with all
//! three and shows what each one got (DESIGN.md §6). The same code backs
//! the claim in `tests/without_contracts.rs`.
//!
//! Each reads the bytes through an assumption someone wrote into it:
//!
//! - **Dashboard**, written for the old firmware: reads `power_w`. A
//!   missing field is NULL, which is not an error; it is drawn as 0 W.
//! - **Analytics**, told about the change by e-mail: `power_w`, else
//!   `power_mw / 1000`.
//! - **Data lake import**, lenient: the first field whose name starts with
//!   `power`, taken as watts.

use serde::Serialize;
use serde_json::Value;

/// What the three receivers read from one message, in watts. `None` is
/// NULL: the receiver stored nothing, and nothing failed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Readings {
    pub dashboard: Option<f64>,
    pub told: Option<f64>,
    pub lenient: Option<f64>,
}

impl Readings {
    /// Whether the receivers disagree. NULL counts as its own reading.
    pub fn disagree(&self) -> bool {
        self.dashboard != self.told || self.told != self.lenient
    }
}

/// Reads one message with the three receivers. Bytes that are not a JSON
/// object read as NULL everywhere.
pub fn read(bytes: &[u8]) -> Readings {
    let Ok(msg) = serde_json::from_slice::<Value>(bytes) else {
        return Readings::default();
    };
    Readings {
        dashboard: dashboard(&msg),
        told: told(&msg),
        lenient: lenient(&msg),
    }
}

/// The `seq` field of a JSON message: which reading it was. Not one of the
/// receivers; the hub echoes it so a node can match an answer to its send.
pub fn seq(bytes: &[u8]) -> Option<u32> {
    let msg = serde_json::from_slice::<Value>(bytes).ok()?;
    msg.get("seq")?.as_u64()?.try_into().ok()
}

fn dashboard(msg: &Value) -> Option<f64> {
    msg.get("power_w").and_then(Value::as_f64)
}

fn told(msg: &Value) -> Option<f64> {
    dashboard(msg).or_else(|| msg.get("power_mw")?.as_f64().map(|mw| mw / 1000.0))
}

fn lenient(msg: &Value) -> Option<f64> {
    msg.as_object()?
        .iter()
        .find(|(k, _)| k.starts_with("power"))
        .and_then(|(_, v)| v.as_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{untyped_json, Firmware};

    fn sent(fw: Firmware) -> Readings {
        read(untyped_json(fw, 1_781_613_000, 30, 326).as_bytes())
    }

    #[test]
    fn the_old_firmware_reads_the_same_everywhere() {
        let r = sent(Firmware::V1);
        assert_eq!(
            (r.dashboard, r.told, r.lenient),
            (Some(326.0), Some(326.0), Some(326.0))
        );
        assert!(!r.disagree());
    }

    #[test]
    fn the_new_firmware_reads_as_three_numbers() {
        for fw in [
            Firmware::V2,
            Firmware::V3,
            Firmware::Unbumped,
            Firmware::NoContract,
        ] {
            let r = sent(fw);
            assert_eq!(r.dashboard, None, "{fw:?}: NULL, drawn as 0 W");
            assert_eq!(r.told, Some(326.0), "{fw:?}");
            assert_eq!(r.lenient, Some(326_000.0), "{fw:?}");
            assert!(r.disagree());
        }
    }

    #[test]
    fn bytes_that_are_not_json_read_as_null() {
        assert_eq!(read(&[0x01, 0x02]), Readings::default());
    }
}
