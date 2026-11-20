//! The live demo's AimDB graphs (DESIGN.md §2, §3).
//!
//! - [`node`]: a node, the visitor's phone or a simulated gateway, that can
//!   be flashed with any `power_reading` version, or none, and sends one
//!   reading at a time.
//! - [`hub`] (feature `hub`): the receiver that knows `power_reading` @1
//!   and @2, one route per place, and what the page reads from it.
//! - [`wasm`] (feature `wasm`): running and stopping instances in a page,
//!   and a node that can be flashed, shared by both bundles.
//!
//! Contracts, envelopes, admission and migration are real AimDB. The runtime
//! is a type parameter; the page uses the WASM adapter.

extern crate alloc;

#[cfg(feature = "hub")]
pub mod baseline;
#[cfg(feature = "hub")]
pub mod hub;
pub mod node;
#[cfg(feature = "wasm")]
pub mod wasm;

use aimdb_core::builder::AimDbRunner;
use aimdb_core::{AimDb, InboundRouteStats};
use aimdb_data_contracts::link_codecs::Postcard;

pub use page_connector::{Bus, Plug, PlugHandle};

/// Every link in the demo carries Postcard.
pub const CODEC: Postcard<64> = Postcard::<64>;
/// The link scheme: `live://<place>/power`.
pub const SCHEME: &str = "live";

/// One built AimDB instance. The caller runs `runner` on its executor.
pub struct Built {
    pub db: AimDb,
    pub runner: AimDbRunner,
    pub plugs: Vec<PlugHandle>,
}

/// A runtime the graphs can be built on. Buffers are the one
/// adapter-specific registration step.
pub trait Runtime: aimdb_core::RuntimeOps + 'static {
    fn buffer<T, S>(
        r: &mut aimdb_core::RecordRegistrar<'_, T, S>,
        cfg: aimdb_core::buffer::BufferCfg,
    ) where
        T: Send + Sync + Clone + core::fmt::Debug + 'static,
        S: aimdb_core::SchemaState;
}

#[cfg(feature = "wasm")]
impl Runtime for aimdb_wasm_adapter::WasmAdapter {
    fn buffer<T, S>(
        r: &mut aimdb_core::RecordRegistrar<'_, T, S>,
        cfg: aimdb_core::buffer::BufferCfg,
    ) where
        T: Send + Sync + Clone + core::fmt::Debug + 'static,
        S: aimdb_core::SchemaState,
    {
        use aimdb_wasm_adapter::WasmRecordRegistrarExt;
        r.buffer(cfg);
    }
}

pub fn buf<'r, 'a, R, T, S>(
    r: &'r mut aimdb_core::RecordRegistrar<'a, T, S>,
    cfg: aimdb_core::buffer::BufferCfg,
) -> &'r mut aimdb_core::RecordRegistrar<'a, T, S>
where
    R: Runtime,
    T: Send + Sync + Clone + core::fmt::Debug + 'static,
    S: aimdb_core::SchemaState,
{
    R::buffer(r, cfg);
    r
}

/// Record names are built per place at run time; AimDB keys are `'static`.
/// A page builds a bounded number of routes, so this leaks a bounded amount.
pub fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// A node's frames arrive as `<place>/power`; the relay only forwards a
/// frame from place `n` whose topic starts with `n/`.
pub fn topic(place: &str) -> String {
    format!("{place}/power")
}

/// A node's frames while the hub has contracts switched off: plain JSON,
/// on a topic no AimDB route listens to, so admission counters stay clean.
pub fn raw_topic(place: &str) -> String {
    format!("{place}/raw")
}

/// The hub's answer to one node: `<place>/feedback`.
pub fn feedback_topic(place: &str) -> String {
    format!("{place}/feedback")
}

/// How a reading crossed the bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carried {
    /// `<place>/power`: admitted, or rejected, by AimDB.
    Checked,
    /// `<place>/raw`: contracts off, read by the baseline receivers.
    Raw,
}

/// The place a reading came from and how it was carried, by its topic.
pub fn reading_of(topic: &str) -> Option<(&str, Carried)> {
    let (place, kind) = topic.split_once('/')?;
    let kind = match kind {
        "power" => Carried::Checked,
        "raw" => Carried::Raw,
        _ => return None,
    };
    (!place.is_empty()).then_some((place, kind))
}

/// The place a checked reading came from, by its topic.
pub fn place_of(topic: &str) -> Option<&str> {
    reading_of(topic).and_then(|(p, c)| (c == Carried::Checked).then_some(p))
}

pub fn rejected(s: &InboundRouteStats) -> u64 {
    s.malformed
        + s.unsupported_format
        + s.contract_mismatch
        + s.too_new
        + s.too_old
        + s.shape_mismatch
        + s.codec_mismatch
        + s.decode_error
}

pub fn upgraded(s: &InboundRouteStats) -> u64 {
    s.upgraded.iter().map(|u| u.count).sum()
}

/// The hub's verdict on one message, from its route's counters before and
/// after the message was dispatched. Admission records its outcome
/// synchronously inside `InboundDispatch::dispatch`, so a snapshot taken
/// right after delivery tells exactly what happened to that one frame
/// (DESIGN.md §3, §9). `None` when no counter moved.
pub fn verdict(before: &InboundRouteStats, after: &InboundRouteStats) -> Option<&'static str> {
    if after.accepted > before.accepted {
        return Some(if upgraded(after) > upgraded(before) {
            "upgraded"
        } else {
            "accepted"
        });
    }
    [
        ("too_new", before.too_new, after.too_new),
        ("too_old", before.too_old, after.too_old),
        (
            "shape_mismatch",
            before.shape_mismatch,
            after.shape_mismatch,
        ),
        ("malformed", before.malformed, after.malformed),
        ("decode_error", before.decode_error, after.decode_error),
        (
            "contract_mismatch",
            before.contract_mismatch,
            after.contract_mismatch,
        ),
        (
            "unsupported_format",
            before.unsupported_format,
            after.unsupported_format,
        ),
        (
            "codec_mismatch",
            before.codec_mismatch,
            after.codec_mismatch,
        ),
    ]
    .iter()
    .find(|(_, a, b)| b > a)
    .map(|(k, _, _)| *k)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_topic_names_one_place() {
        assert_eq!(super::topic("3"), "3/power");
        assert_eq!(super::place_of("3/power"), Some("3"));
        assert_eq!(super::place_of("roof-east/power"), Some("roof-east"));
        assert_eq!(super::place_of("a/b/power"), None);
        assert_eq!(super::place_of("/power"), None);
        assert_eq!(super::place_of("3/status"), None);
        assert_eq!(super::place_of("3/raw"), None, "raw is not checked");
        assert_eq!(super::reading_of("3/raw"), Some(("3", super::Carried::Raw)));
        assert_eq!(super::reading_of("3/feedback"), None);
    }
}
