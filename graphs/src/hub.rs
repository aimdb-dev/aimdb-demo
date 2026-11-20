//! The hub: one route per place, each a record of `power_reading@2` that
//! admits versions 1 and 2, with its own admission counters. The routes are
//! separate records outside any application logic, so the page reads only
//! the counters and the latest admitted value (DESIGN.md §3).

use crate::{buf, leak, Built, Plug, Runtime, CODEC, SCHEME};
use aimdb_core::buffer::BufferCfg;
use aimdb_core::envelope::{name_id, Envelope};
use aimdb_core::{AimDb, AimDbBuilder, InboundRouteStats};
use aimdb_data_contracts::{
    ContractShape, EnvelopeCarrier, EnvelopeLinkExt, Key, LinkCodec, LinkCodecBuilderExt,
    SchemaType, VersionedInboundExt,
};
use contracts_r2::{PowerReadingV1, PowerReadingV2};
use exhibit_v3::PowerReadingV3;
use std::sync::Arc;

/// The hub's record for one place: `live.<place>.power`.
pub fn record(place: &str) -> String {
    format!("{SCHEME}.{place}.power")
}

/// The hub's route for one place: record `live.<place>.power`, link
/// `live://<place>/power`.
pub fn route<R: Runtime>(b: &mut AimDbBuilder, place: &str) {
    let k = Key::<PowerReadingV2>::interned(&record(place))
        .with_link(leak(format!("{SCHEME}://{place}/power")));
    b.configure(k, |r| {
        // The page reads the latest admitted value.
        buf::<R, _, _>(r, BufferCfg::SingleLatest)
            .link_from_key()
            .with_link_codec(CODEC)
            .with_envelope(EnvelopeCarrier::Inline)
            .accepts_from::<1>()
            .finish();
    });
}

/// The hub with one route per place: the visitors' nodes, the on-page node
/// and the simulated fleet. It knows `power_reading` @1 and @2, nothing
/// else. Records are named at run time, so a page builds this once.
pub async fn build<R: Runtime>(rt: Arc<R>, plug: Plug, places: &[&str]) -> Built {
    let handle = plug.handle();
    let mut b = AimDbBuilder::new().runtime(rt).with_connector(plug);
    for place in places {
        route::<R>(&mut b, place);
    }
    let (db, runner) = b.build().await.expect("hub graph");
    Built {
        db,
        runner,
        plugs: vec![handle],
    }
}

/// A route's admission counters, as the page reads them.
pub fn stats(hub: &AimDb, place: &str) -> Option<InboundRouteStats> {
    let rec = record(place);
    let records = hub.list_records();
    let r = records.iter().find(|r| r.record_key == rec)?;
    r.inbound_routes.first().map(|m| m.stats.clone())
}

/// Every route's counters, by place, from one `list_records` call.
pub fn stats_by_place(hub: &AimDb) -> std::collections::HashMap<String, InboundRouteStats> {
    let prefix = format!("{SCHEME}.");
    hub.list_records()
        .into_iter()
        .filter_map(|r| {
            let place = r
                .record_key
                .strip_prefix(&prefix)?
                .strip_suffix(".power")?
                .to_string();
            let stats = r.inbound_routes.first()?.stats.clone();
            Some((place, stats))
        })
        .collect()
}

/// The last value the hub admitted from `place`.
pub fn admitted(hub: &AimDb, place: &str) -> Option<PowerReadingV2> {
    hub.latest(Key::<PowerReadingV2>::interned(&record(place)))
        .ok()
        .flatten()
        .map(|v| v.value)
}

/// Shape prefixes, read from the `aimdb.lock` files: revision 2's @1 and
/// @2, and the @3 exhibit's. The unbumped exhibit declares @1 with @2's
/// shape.
pub const SHAPE_V1: Option<u64> = <PowerReadingV1 as ContractShape>::SHAPE;
pub const SHAPE_V2: Option<u64> = <PowerReadingV2 as ContractShape>::SHAPE;
pub const SHAPE_V3: Option<u64> = <PowerReadingV3 as ContractShape>::SHAPE;

/// What a frame says about itself, before any check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    /// `None` when the bytes carry no envelope.
    pub contract: Option<String>,
    pub version: Option<u32>,
    pub shape: Option<u64>,
    /// The power field and its number, decoded by the shape the frame
    /// declares.
    pub field: Option<(&'static str, u32)>,
    /// The reading's sequence number, from the decoded value or, without
    /// an envelope, from the JSON.
    pub seq: Option<u32>,
}

/// The power field, its number and the sequence number, decoded by the
/// declared shape.
fn field(shape: Option<u64>, payload: &[u8]) -> Option<(&'static str, u32, u32)> {
    let shape = shape?;
    if Some(shape) == SHAPE_V1 {
        LinkCodec::<PowerReadingV1>::decode(&CODEC, payload)
            .ok()
            .map(|v| ("power_w", u32::from(v.power_w), v.seq))
    } else if Some(shape) == SHAPE_V2 {
        LinkCodec::<PowerReadingV2>::decode(&CODEC, payload)
            .ok()
            .map(|v| ("power_mw", v.power_mw, v.seq))
    } else if Some(shape) == SHAPE_V3 {
        LinkCodec::<PowerReadingV3>::decode(&CODEC, payload)
            .ok()
            .map(|v| ("power_mw", v.power_mw, v.seq))
    } else {
        None
    }
}

/// The firmware a frame says it came from, as the graph labels a node:
/// `@1`, `@2`, `@3`, `unbumped` (declared @1 with another shape) or `none`
/// (no envelope). Read from the frame, so it is what the hub can know.
pub fn firmware_of(label: &Label) -> &'static str {
    match (label.version, label.shape) {
        (None, _) => "none",
        (Some(1), s) if s == SHAPE_V1 => "@1",
        (Some(1), _) => "unbumped",
        (Some(2), _) => "@2",
        (Some(3), _) => "@3",
        _ => "other",
    }
}

/// Reads a frame's label for the page. This is display only: the verdict is
/// the hub's admission result, never this.
pub fn label(frame: &[u8]) -> Label {
    match Envelope::parse(frame) {
        Ok(e) => {
            let c = e.contract();
            let known = <PowerReadingV2 as SchemaType>::NAME;
            let contract = if name_id(known) == c.name_id() {
                known.to_string()
            } else {
                format!("{:#010x}", c.name_id())
            };
            let decoded = field(c.shape(), e.payload());
            Label {
                contract: Some(contract),
                version: Some(c.version()),
                shape: c.shape(),
                field: decoded.map(|(name, n, _)| (name, n)),
                seq: decoded.map(|(_, _, seq)| seq),
            }
        }
        Err(_) => Label {
            contract: None,
            version: None,
            shape: None,
            field: None,
            seq: crate::baseline::seq(frame),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locked(lock: &str, version: u32) -> u64 {
        let entry = lock
            .split("[[contract]]")
            .find(|e| {
                e.contains("name = \"power_reading\"")
                    && e.contains(&format!("version = {version}\n"))
            })
            .expect("lock entry");
        let hex = entry.split("sha256:").nth(1).unwrap();
        u64::from_str_radix(&hex[..16], 16).unwrap()
    }

    /// Shapes on any page come from the lock files (DESIGN.md §1).
    #[test]
    fn shapes_come_from_the_locks() {
        let r2 = include_str!("../../contracts/r2/aimdb.lock");
        let v3 = include_str!("../../contracts/exhibits/v3/aimdb.lock");
        let unbumped = include_str!("../../contracts/exhibits/unbumped/aimdb.lock");
        assert_eq!(SHAPE_V1, Some(locked(r2, 1)));
        assert_eq!(SHAPE_V2, Some(locked(r2, 2)));
        assert_eq!(SHAPE_V3, Some(locked(v3, 3)));
        // The forgotten bump: declared @1, shaped like @2.
        assert_eq!(locked(unbumped, 1), locked(r2, 2));
        assert_ne!(SHAPE_V1, SHAPE_V2);
    }
}
