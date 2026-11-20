//! The computer page's bundle: the hub, the on-page node and the simulated
//! fleet's nodes (DESIGN.md §2 to §5).
//!
//! ```js
//! import init, { LiveHub } from "./pkg/live_web.js";
//! await init();
//!
//! // A hub that knows power_reading @1 and @2, one route per place: nodes
//! // "1".."8", the on-page node "here", and the simulated fleet.
//! const hub = await LiveHub.start("wss://aimdb.dev/bus/<code>?as=hq");
//! setInterval(() => render(hub.view()), 200);  // counts, admitted, frames
//! const here = await hub.node("here");         // "No phone? Send from here"
//! const sim  = await hub.node("roof-east");    // one simulated gateway
//! await here.flash("v3");                      // v1, v2, v3, unbumped, none
//! here.send(326);                              // watts
//! ```
//!
//! Every verdict is the hub's own: admission updates the route's counters
//! inside `dispatch`, so the bus observer, which runs right after delivery,
//! reads each frame's outcome from its route's counters. Nothing is
//! inferred from the frame itself.
//!
//! The page can switch contracts off (DESIGN.md §6): `hub.setContracts(false)`.
//! The hub tells every node; nodes then send their firmware's plain JSON on
//! `<place>/raw`, which no AimDB route reads, and the hub reads it with the
//! three baseline receivers instead. Either way, each visitor's node is
//! told what the hub made of its reading, on `<place>/feedback`.
#![cfg(target_arch = "wasm32")]

use aimdb_core::InboundRouteStats;
use live_graphs::baseline::{self, Readings};
use live_graphs::hub;
use live_graphs::node::Firmware;
use live_graphs::wasm::{contracts_on, rt, spawn, Contracts, FlashableNode, Running};
use live_graphs::{feedback_topic, reading_of, rejected, upgraded, verdict, Bus, Carried, SCHEME};
use page_connector::ws_bus::BusLink;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;

/// Frames the page keeps for its log.
const KEEP: usize = 60;
/// How often the hub sends what it has for the nodes.
const SEND_MS: i32 = 50;
/// How often the hub repeats its mode, for nodes that joined since.
const MODE_EVERY: u32 = 40;
/// Places for visitors' nodes, one QR code each.
pub const NODES: u8 = 8;
/// The on-page node.
pub const HERE: &str = "here";
/// The simulated fleet's names; the page runs 3 to 12 of them at a time.
pub const FLEET: [&str; 12] = [
    "roof-east",
    "roof-west",
    "line-1",
    "line-3",
    "pump-12",
    "inverter-2",
    "carport",
    "boiler-room",
    "chiller-4",
    "dock-b",
    "press-7",
    "lab",
];

fn places() -> Vec<String> {
    let mut p: Vec<String> = (1..=NODES).map(|n| n.to_string()).collect();
    p.push(HERE.into());
    p.extend(FLEET.iter().map(|s| s.to_string()));
    p
}

/// `node` for a visitor's place, `here` for the on-page node, `sim` for the
/// simulated fleet.
fn kind(place: &str) -> &'static str {
    if place == HERE {
        "here"
    } else if place.parse::<u8>().is_ok() {
        "node"
    } else {
        "sim"
    }
}

struct Seen {
    at_ms: f64,
    place: String,
    payload: Vec<u8>,
    /// The hub's verdict, read from the route's counters right after
    /// delivery; `None` when the topic had no route, or for a raw frame.
    verdict: Option<&'static str>,
    /// What the baseline receivers read: raw frames only.
    readings: Option<Readings>,
}

/// The hub's answer to one node, on `<place>/feedback`.
#[derive(Serialize)]
struct Feedback {
    contracts: bool,
    /// Which reading this answers, so a node that sends every second
    /// holds each answer against the right value.
    seq: Option<u32>,
    /// Contracts on: the hub's verdict, and the value it admitted, in watts.
    verdict: Option<&'static str>,
    admitted_w: Option<f64>,
    /// Contracts off: what the three receivers read, in watts.
    readings: Option<Readings>,
}

/// What the observer keeps between frames.
#[derive(Default)]
struct Log {
    seen: Vec<Seen>,
    /// Frames seen since the page started.
    total: usize,
    /// Each route's counters after the last frame on it.
    last: HashMap<String, InboundRouteStats>,
    /// Visitors' places that have sent, checked or raw.
    joined: std::collections::BTreeSet<u8>,
    /// Every sender's last reading, for the graph (DESIGN.md §7).
    senders: HashMap<String, Last>,
    /// Raw messages since contracts went off, and how many of them the
    /// receivers read differently.
    raw: RawCounts,
    /// Frames for the relay: feedback and the mode.
    outbox: Vec<(String, Vec<u8>)>,
}

/// A sender's last reading, as the graph shows it.
#[derive(Clone, Copy)]
struct Last {
    at_ms: f64,
    /// From the frame: `@1`, `@2`, `@3`, `unbumped`, `none`; `JSON` raw.
    firmware: &'static str,
    verdict: Option<&'static str>,
    /// With contracts off: whether the three receivers read it differently.
    disagree: Option<bool>,
}

/// One node of the graph.
#[derive(Serialize)]
struct GraphNode {
    place: String,
    /// `node`, `here` or `sim`.
    kind: &'static str,
    firmware: &'static str,
    /// The hub's last verdict, or `null` for a raw reading.
    verdict: Option<&'static str>,
    /// Raw readings only: the three receivers disagreed.
    disagree: Option<bool>,
    /// When the hub last heard from it, milliseconds since the epoch.
    last_ms: f64,
    /// Its own route's counters (DESIGN.md §4).
    accepted: u64,
    upgraded: u64,
    rejected: u64,
}

#[derive(Serialize, Default, Clone, Copy)]
struct RawCounts {
    read: u64,
    disagree: u64,
}

#[derive(Serialize, Default)]
struct Counts {
    accepted: u64,
    upgraded: u64,
    too_new: u64,
    shape_mismatch: u64,
    malformed: u64,
    decode_error: u64,
    other: u64,
}

impl Counts {
    fn add(&mut self, s: &InboundRouteStats) {
        self.accepted += s.accepted;
        self.upgraded += upgraded(s);
        self.too_new += s.too_new;
        self.shape_mismatch += s.shape_mismatch;
        self.malformed += s.malformed;
        self.decode_error += s.decode_error;
        self.other += s.unsupported_format + s.contract_mismatch + s.too_old + s.codec_mismatch;
    }
}

#[derive(Serialize)]
struct Admitted {
    place: String,
    seq: u32,
    sample_s: u64,
    power_mw: u32,
}

#[derive(Serialize)]
struct FrameJs {
    /// Milliseconds since the epoch, when the hub saw the frame.
    at_ms: f64,
    /// `1`..`8`, `here`, or a simulated gateway's name.
    place: String,
    /// `node`, `here` or `sim`.
    kind: &'static str,
    /// The hub's verdict: `accepted`, `upgraded`, `too_new`, … or `null`
    /// for a topic without a route.
    verdict: Option<&'static str>,
    bytes: usize,
    /// From the envelope; `null` when there is none.
    contract: Option<String>,
    version: Option<u32>,
    /// The shape hash prefix, 16 hex digits.
    shape: Option<String>,
    /// The bytes as text, when they are text (a frame without a contract).
    text: Option<String>,
    /// The power field the declared shape names, and its number.
    field: Option<String>,
    number: Option<u32>,
    /// Carried raw, with contracts off: what the three receivers read.
    readings: Option<Readings>,
}

#[derive(Serialize)]
struct LiveView {
    /// `Local`, `Connecting`, `Open`, `Taken` or `Closed`.
    link: String,
    /// Frames seen so far; the page compares it to tell new frames apart.
    frames_total: usize,
    /// Every route's counters, summed.
    counts: Counts,
    /// Visitors' places (`1`..`8`) that have sent at least one frame.
    nodes: Vec<String>,
    /// The latest value admitted from the newest non-simulated frame's
    /// place.
    admitted: Option<Admitted>,
    /// The latest value admitted from each visitor's place and the on-page
    /// node, by place: the stage shows whichever node changed last.
    admitted_at: HashMap<String, Admitted>,
    /// Every node that has sent, for the graph (DESIGN.md §7).
    graph: Vec<GraphNode>,
    /// Whether contracts are on.
    contracts: bool,
    /// Since contracts went off: messages read, and read differently.
    raw: RawCounts,
    /// Newest first.
    frames: Vec<FrameJs>,
}

#[wasm_bindgen]
pub struct LiveHub {
    bus: Bus,
    hub: Running,
    link: Option<BusLink>,
    log: Arc<Mutex<Log>>,
    contracts: Contracts,
}

#[wasm_bindgen]
impl LiveHub {
    /// `bus_url`: the relay's `/bus/<code>?as=hq`, or empty for a hub with
    /// only local nodes. The hub only listens: its own nodes' frames stay
    /// in the page, so the fleet never reaches a visitor's phone.
    pub async fn start(bus_url: String) -> LiveHub {
        let bus = Bus::new(SCHEME);
        let places = places();
        let refs: Vec<&str> = places.iter().map(String::as_str).collect();
        let hub = spawn(hub::build(rt(), bus.hub(), &refs).await);
        let log: Arc<Mutex<Log>> = Arc::default();
        let contracts = contracts_on();
        let (l, db, on) = (log.clone(), hub.db.clone(), contracts.clone());
        bus.observe(move |f| {
            let Some((place, carried)) = reading_of(&f.topic) else {
                return;
            };
            let mut l = l.lock().unwrap();
            let label = hub::label(&f.payload);
            let (verdict, readings) = match carried {
                Carried::Checked => {
                    let v = hub::stats(&db, place).and_then(|after| {
                        let before = l.last.insert(place.to_string(), after.clone());
                        verdict(&before.unwrap_or_default(), &after)
                    });
                    (v, None)
                }
                Carried::Raw => {
                    let r = baseline::read(&f.payload);
                    l.raw.read += 1;
                    l.raw.disagree += u64::from(r.disagree());
                    (None, Some(r))
                }
            };
            if kind(place) == "node" {
                if let Ok(n) = place.parse() {
                    l.joined.insert(n);
                }
                let fb = Feedback {
                    contracts: on.load(Ordering::Relaxed),
                    seq: label.seq,
                    verdict,
                    admitted_w: matches!(verdict, Some("accepted" | "upgraded"))
                        .then(|| watts(&f.payload))
                        .flatten(),
                    readings,
                };
                if let Ok(json) = serde_json::to_vec(&fb) {
                    l.outbox.push((feedback_topic(place), json));
                }
            }
            let at_ms = js_sys::Date::now();
            l.senders.insert(
                place.to_string(),
                Last {
                    at_ms,
                    firmware: match carried {
                        Carried::Checked => hub::firmware_of(&label),
                        Carried::Raw => "JSON",
                    },
                    verdict,
                    disagree: readings.map(|r| r.disagree()),
                },
            );
            l.total += 1;
            l.seen.push(Seen {
                at_ms,
                place: place.to_string(),
                payload: f.payload.clone(),
                verdict,
                readings,
            });
            // Bounded: the page runs for the length of a meeting.
            if l.seen.len() > 4 * KEEP {
                l.seen.drain(..2 * KEEP);
            }
        });
        // The hub listens: its own nodes' frames stay in the page, so the
        // fleet never reaches a visitor's phone. It sends only feedback to
        // one node and its mode to all.
        let link = (!bus_url.is_empty()).then(|| BusLink::listen(&bus, &bus_url));
        let (l, out, on) = (log.clone(), link.clone(), contracts.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let mut tick = 0u32;
            loop {
                let mut frames = std::mem::take(&mut l.lock().unwrap().outbox);
                if tick.is_multiple_of(MODE_EVERY) {
                    frames.push((line_protocol::MODE.into(), mode(on.load(Ordering::Relaxed))));
                }
                if let Some(link) = &out {
                    for (topic, payload) in &frames {
                        link.send(topic, payload);
                    }
                }
                tick = tick.wrapping_add(1);
                page_connector::sleep_ms(SEND_MS).await;
            }
        });
        LiveHub {
            bus,
            hub,
            link,
            log,
            contracts,
        }
    }

    /// Switches contracts on or off for the whole demo: this page's nodes
    /// at once, the visitors' nodes when the mode reaches them.
    #[wasm_bindgen(js_name = setContracts)]
    pub fn set_contracts(&self, on: bool) {
        if self.contracts.swap(on, Ordering::Relaxed) == on {
            return;
        }
        let mut l = self.log.lock().unwrap();
        if !on {
            l.raw = RawCounts::default();
        }
        l.outbox.push((line_protocol::MODE.into(), mode(on)));
    }

    pub fn contracts(&self) -> bool {
        self.contracts.load(Ordering::Relaxed)
    }

    /// A node in this page: `here` for the on-page node, or a simulated
    /// gateway from [`FLEET`]. Its frames never leave the page.
    pub async fn node(&self, place: String) -> Result<LocalNode, JsError> {
        if kind(&place) == "node" || !(place == HERE || FLEET.contains(&place.as_str())) {
            return Err(JsError::new("a local node is `here` or a fleet name"));
        }
        Ok(LocalNode(
            FlashableNode::new(self.bus.clone(), place, self.contracts.clone()).await,
        ))
    }

    /// The simulated fleet's names.
    pub fn fleet() -> Vec<String> {
        FLEET.iter().map(|s| s.to_string()).collect()
    }

    /// Places for visitors' nodes.
    #[wasm_bindgen(js_name = nodePlaces)]
    pub fn node_places() -> u8 {
        NODES
    }

    pub fn view(&self) -> Result<JsValue, JsError> {
        // The bus's own frame list is for tests; keep it from growing.
        self.bus.clear_frames();
        let stats = hub::stats_by_place(&self.hub.db);
        let mut counts = Counts::default();
        stats.values().for_each(|s| counts.add(s));
        let l = self.log.lock().unwrap();
        // Every visitor's place that has sent, also those rotated out of the
        // log.
        let nodes: Vec<String> = l.joined.iter().map(u8::to_string).collect();
        let admitted = l
            .seen
            .iter()
            .rev()
            .find(|f| kind(&f.place) != "sim")
            .and_then(|f| {
                hub::admitted(&self.hub.db, &f.place).map(|v| Admitted {
                    place: f.place.clone(),
                    seq: v.seq,
                    sample_s: v.sample_s,
                    power_mw: v.power_mw,
                })
            });
        // The latest admitted value of every node that has sent.
        let admitted_at = l
            .senders
            .keys()
            .filter_map(|place| {
                let v = hub::admitted(&self.hub.db, place)?;
                Some((
                    place.clone(),
                    Admitted {
                        place: place.clone(),
                        seq: v.seq,
                        sample_s: v.sample_s,
                        power_mw: v.power_mw,
                    },
                ))
            })
            .collect();
        let graph = l
            .senders
            .iter()
            .map(|(place, last)| {
                let s = stats.get(place).cloned().unwrap_or_default();
                GraphNode {
                    place: place.clone(),
                    kind: kind(place),
                    firmware: last.firmware,
                    verdict: last.verdict,
                    disagree: last.disagree,
                    last_ms: last.at_ms,
                    accepted: s.accepted,
                    upgraded: upgraded(&s),
                    rejected: rejected(&s),
                }
            })
            .collect();
        let frames = l
            .seen
            .iter()
            .rev()
            .take(KEEP)
            .map(|f| {
                let lb = hub::label(&f.payload);
                let text = lb
                    .contract
                    .is_none()
                    .then(|| String::from_utf8(f.payload.clone()).ok())
                    .flatten();
                FrameJs {
                    at_ms: f.at_ms,
                    place: f.place.clone(),
                    kind: kind(&f.place),
                    verdict: f.verdict,
                    bytes: f.payload.len(),
                    contract: lb.contract,
                    version: lb.version,
                    shape: lb.shape.map(|s| format!("{s:016x}")),
                    text,
                    field: lb.field.map(|f| f.0.to_string()),
                    number: lb.field.map(|f| f.1),
                    readings: f.readings,
                }
            })
            .collect();
        Ok(serde_wasm_bindgen::to_value(&LiveView {
            link: self
                .link
                .as_ref()
                .map_or("Local".into(), |l| format!("{:?}", l.state())),
            frames_total: l.total,
            counts,
            nodes,
            admitted,
            admitted_at,
            graph,
            contracts: self.contracts(),
            raw: l.raw,
            frames,
        })?)
    }
}

/// A node in the computer page: the on-page node or a simulated gateway.
#[wasm_bindgen]
pub struct LocalNode(FlashableNode);

#[wasm_bindgen]
impl LocalNode {
    /// Flashes new firmware: `v1`, `v2`, `v3`, `unbumped` or `none`. The old
    /// instance is stopped, not left running.
    pub async fn flash(&self, firmware: String) -> Result<(), JsError> {
        let fw = Firmware::parse(&firmware)
            .ok_or_else(|| JsError::new("firmware is v1, v2, v3, unbumped or none"))?;
        self.0.flash(fw).await;
        Ok(())
    }

    pub fn firmware(&self) -> String {
        self.0.firmware().into()
    }

    pub fn place(&self) -> String {
        self.0.place().into()
    }

    /// Sends one reading in watts; returns its sequence number.
    pub fn send(&self, power_w: u16) -> u32 {
        self.0.send(power_w)
    }

    /// Sends the last reading again, in the current mode. `undefined`
    /// before the first send.
    pub fn resend(&self) -> Option<u32> {
        self.0.resend()
    }

    /// Stops the node for good: a simulated gateway that leaves the fleet.
    pub fn stop(&self) {
        self.0.stop();
    }
}

fn mode(on: bool) -> Vec<u8> {
    if on {
        b"on".to_vec()
    } else {
        b"off".to_vec()
    }
}

/// The power a checked frame carries, in watts, decoded by the shape it
/// declares. Used only for a frame the hub admitted, so it is the admitted
/// value: @1 watts migrate to @2 as `power_w × 1000`.
fn watts(payload: &[u8]) -> Option<f64> {
    hub::label(payload).field.map(|(name, n)| match name {
        "power_w" => f64::from(n),
        _ => f64::from(n) / 1000.0,
    })
}
