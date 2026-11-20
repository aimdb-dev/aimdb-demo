//! A node: one sender of `power_reading`, flashed with any of five
//! firmwares. The visitor's phone, the on-page node and every simulated
//! gateway are nodes; each sends under its own place.
//!
//! | Firmware | On the wire | The hub's verdict |
//! |---|---|---|
//! | `v1` | `power_reading@1`, watts | admitted, migrated to @2 |
//! | `v2` | `power_reading@2`, milliwatts | admitted as current |
//! | `v3` | `power_reading@3`, adds `string_id` | rejected, `too_new` |
//! | `unbumped` | declared @1, milliwatt shape | rejected, `shape_mismatch` |
//! | `none` | plain JSON, no envelope | rejected, `malformed` |

use crate::{buf, topic, Built, Bus, Plug, Runtime, CODEC};
use aimdb_core::buffer::BufferCfg;
use aimdb_core::{AimDb, AimDbBuilder};
use aimdb_data_contracts::{
    EnvelopeCarrier, EnvelopeLinkExt, Key, LinkCodec, LinkCodecBuilderExt, MigrationChain,
    SchemaType,
};
use std::sync::Arc;

const NODE_RECORD: &str = "node.power";
/// The node's own link; its port plug puts the place in front.
const NODE_LINK: &str = "live://power";
/// A node reports on channel 1.
pub const CHANNEL: u8 = 1;

/// What a node can be flashed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Firmware {
    V1,
    V2,
    V3,
    /// Milliwatts, still declared as version 1: the forgotten bump.
    Unbumped,
    /// Plain JSON, no contract and no envelope.
    NoContract,
}

impl Firmware {
    pub const ALL: [Firmware; 5] = [
        Self::V1,
        Self::V2,
        Self::V3,
        Self::Unbumped,
        Self::NoContract,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "v1" => Self::V1,
            "v2" => Self::V2,
            "v3" => Self::V3,
            "unbumped" => Self::Unbumped,
            "none" => Self::NoContract,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::V1 => "v1",
            Self::V2 => "v2",
            Self::V3 => "v3",
            Self::Unbumped => "unbumped",
            Self::NoContract => "none",
        }
    }
}

/// The JSON a node without a contract sends: the @2 fields, no label.
pub fn plain_json(sample_s: u64, seq: u32, power_mw: u32) -> String {
    format!(r#"{{"channel":{CHANNEL},"sample_s":{sample_s},"seq":{seq},"power_mw":{power_mw}}}"#)
}

/// What a firmware sends when nothing checks it: its own struct as plain
/// JSON, field names and all, with no contract, version or shape. The
/// forgotten bump looks exactly like @2: without a version there is nothing
/// to forget.
pub fn untyped_json(fw: Firmware, sample_s: u64, seq: u32, power_w: u16) -> String {
    let mw = u32::from(power_w) * 1000;
    match fw {
        Firmware::V1 => format!(
            r#"{{"channel":{CHANNEL},"sample_s":{sample_s},"seq":{seq},"power_w":{power_w}}}"#
        ),
        Firmware::V2 | Firmware::Unbumped | Firmware::NoContract => plain_json(sample_s, seq, mw),
        Firmware::V3 => format!(
            r#"{{"channel":{CHANNEL},"sample_s":{sample_s},"seq":{seq},"power_mw":{mw},"string_id":1}}"#
        ),
    }
}

fn key<T: SchemaType>() -> Key<T> {
    Key::new(NODE_RECORD).with_link(NODE_LINK)
}

fn outbound<R: Runtime, T>(b: &mut AimDbBuilder)
where
    T: MigrationChain + Clone + core::fmt::Debug + Send + Sync + 'static,
    aimdb_data_contracts::link_codecs::Postcard<64>: LinkCodec<T>,
{
    b.configure(key::<T>(), |r| {
        buf::<R, _, _>(r, BufferCfg::SpmcRing { capacity: 8 })
            .link_to_key()
            .with_link_codec(CODEC)
            .with_envelope(EnvelopeCarrier::Inline)
            .finish();
    });
}

/// A node's gateway. Flashing builds a new one.
pub struct NodeGateway {
    pub firmware: Firmware,
    bus: Bus,
    /// `<place>/power`, for frames sent without AimDB.
    topic: String,
    db: Option<AimDb>,
}

impl NodeGateway {
    /// Builds the gateway at `place`: a node `1`..`8`, the on-page node, or
    /// a simulated gateway. Its frames go out as `<place>/power`.
    /// `NoContract` builds no AimDB instance: it publishes raw JSON the way
    /// an untyped device would.
    pub async fn build<R: Runtime>(
        rt: Arc<R>,
        firmware: Firmware,
        bus: &Bus,
        place: &str,
    ) -> (NodeGateway, Option<Built>) {
        let gw = |db| NodeGateway {
            firmware,
            bus: bus.clone(),
            topic: topic(place),
            db,
        };
        if firmware == Firmware::NoContract {
            return (gw(None), None);
        }
        let plug: Plug = bus.port(place.to_string());
        let handle = plug.handle();
        let mut b = AimDbBuilder::new().runtime(rt).with_connector(plug);
        match firmware {
            Firmware::V1 => outbound::<R, contracts_r1::PowerReading>(&mut b),
            Firmware::V2 => outbound::<R, contracts_r2::PowerReadingV2>(&mut b),
            Firmware::V3 => outbound::<R, exhibit_v3::PowerReadingV3>(&mut b),
            Firmware::Unbumped => outbound::<R, exhibit_unbumped::PowerReading>(&mut b),
            Firmware::NoContract => unreachable!(),
        }
        let (db, runner) = b.build().await.expect("node graph");
        (
            gw(Some(db.clone())),
            Some(Built {
                db,
                runner,
                plugs: vec![handle],
            }),
        )
    }

    /// Sends one reading as plain JSON on `<place>/raw`: what this firmware
    /// sends when the hub has contracts switched off.
    pub fn send_untyped(&self, sample_s: u64, seq: u32, power_w: u16) {
        let topic = crate::raw_topic(self.topic.trim_end_matches("/power"));
        let json = untyped_json(self.firmware, sample_s, seq, power_w);
        self.bus.inject(&topic, json.as_bytes());
    }

    /// Sends one reading, measured in whole watts. The firmware decides the
    /// wire form.
    pub fn send(&self, sample_s: u64, seq: u32, power_w: u16) {
        let mw = u32::from(power_w) * 1000;
        let Some(db) = &self.db else {
            self.bus
                .inject(&self.topic, plain_json(sample_s, seq, mw).as_bytes());
            return;
        };
        fn put<T>(db: &AimDb, v: T)
        where
            T: SchemaType + Clone + core::fmt::Debug + Send + Sync + 'static,
        {
            db.producer(key::<T>()).expect("node record").produce(v);
        }
        let channel = CHANNEL;
        match self.firmware {
            Firmware::V1 => put(
                db,
                contracts_r1::PowerReading {
                    channel,
                    sample_s,
                    seq,
                    power_w,
                },
            ),
            Firmware::V2 => put(
                db,
                contracts_r2::PowerReadingV2 {
                    channel,
                    sample_s,
                    seq,
                    power_mw: mw,
                },
            ),
            Firmware::V3 => put(
                db,
                exhibit_v3::PowerReadingV3 {
                    channel,
                    sample_s,
                    seq,
                    power_mw: mw,
                    string_id: 1,
                },
            ),
            Firmware::Unbumped => put(
                db,
                exhibit_unbumped::PowerReading {
                    channel,
                    sample_s,
                    seq,
                    power_mw: mw,
                },
            ),
            Firmware::NoContract => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The untyped JSON is each firmware's own struct, field for field.
    #[test]
    fn untyped_json_is_the_firmware_struct() {
        let (s, q, w) = (1_781_613_000, 30, 326);
        let mw = u32::from(w) * 1000;
        let json =
            |fw| serde_json::from_str::<serde_json::Value>(&untyped_json(fw, s, q, w)).unwrap();
        let channel = CHANNEL;
        assert_eq!(
            json(Firmware::V1),
            serde_json::to_value(contracts_r1::PowerReading {
                channel,
                sample_s: s,
                seq: q,
                power_w: w
            })
            .unwrap()
        );
        assert_eq!(
            json(Firmware::V2),
            serde_json::to_value(contracts_r2::PowerReadingV2 {
                channel,
                sample_s: s,
                seq: q,
                power_mw: mw
            })
            .unwrap()
        );
        assert_eq!(
            json(Firmware::V3),
            serde_json::to_value(exhibit_v3::PowerReadingV3 {
                channel,
                sample_s: s,
                seq: q,
                power_mw: mw,
                string_id: 1
            })
            .unwrap()
        );
        assert_eq!(
            json(Firmware::Unbumped),
            serde_json::to_value(exhibit_unbumped::PowerReading {
                channel,
                sample_s: s,
                seq: q,
                power_mw: mw
            })
            .unwrap()
        );
    }
}
