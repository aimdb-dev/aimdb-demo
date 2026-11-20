//! The `/bus/<code>` frame format and join rules.
//!
//! One binary WebSocket message is one frame:
//!
//! ```text
//! u8 topic length | topic, UTF-8 | payload
//! ```
//!
//! The topic is the link address after the scheme (`3/power` for
//! `live://3/power`). The payload is opaque: the relay never parses it, the
//! receiving hub checks it.
//!
//! Join: `/bus/<code>?as=hq` for the hub, `/bus/<code>?as=<n>` for a node,
//! place 1 to 8, taken from the QR code. One member per place; a second one
//! is refused with close code [`CLOSE_TAKEN`]. Frames from a place go to
//! `hq`. Frames from `hq` go to the one place their topic starts with
//! (`3/feedback` to place 3 only), or to every place when the topic names
//! none ([`MODE`]); see [`hq_target`].
//!
//! Topics a node sends: `<n>/power` (a reading with its contract, or raw
//! bytes from firmware without one) and `<n>/raw` (plain JSON while the
//! hub has contracts switched off). Topics the hub sends: [`MODE`] and
//! `<n>/feedback`.
//!
//! The relay forwards a node's frame without saying who sent it, so the
//! topic is the sender's identity: a frame from place `n` must have a topic
//! under `n/` ([`may_send`]), or the relay drops it. The relay reads the
//! topic, never the payload.
#![no_std]
extern crate alloc;
use alloc::vec::Vec;

pub const MAX_TOPIC: usize = 255;
/// Close code when a place is already taken: "This place is taken. Scan the
/// code on the screen again."
pub const CLOSE_TAKEN: u16 = 4009;
/// The hub's role.
pub const HQ: &str = "hq";
/// Places for visitors' nodes.
pub const PLACES: u8 = 8;

/// The hub's broadcast of whether contracts are on: payload `on` or `off`.
pub const MODE: &str = "mode";

/// Where the relay forwards a frame from `hq`: `Some(place)` when the topic
/// starts with a valid place (`3/feedback`), `None` for every place.
pub fn hq_target(topic: &str) -> Option<&str> {
    let (first, _) = topic.split_once('/')?;
    (first != HQ && valid_role(first)).then_some(first)
}

/// Whether `role` may join: the hub, or a place `1` to [`PLACES`].
pub fn valid_role(role: &str) -> bool {
    role == HQ
        || (role.len() <= 2
            && !role.starts_with('0')
            && role.parse::<u8>().is_ok_and(|n| (1..=PLACES).contains(&n)))
}

/// Whether `role` may send a frame on `topic`. `hq` sends anywhere; a place
/// only under its own name, so no node can speak for another.
pub fn may_send(role: &str, topic: &str) -> bool {
    role == HQ
        || topic
            .strip_prefix(role)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn encode(topic: &str, payload: &[u8]) -> Option<Vec<u8>> {
    let t = topic.as_bytes();
    if t.len() > MAX_TOPIC {
        return None;
    }
    let mut out = Vec::with_capacity(1 + t.len() + payload.len());
    out.push(t.len() as u8);
    out.extend_from_slice(t);
    out.extend_from_slice(payload);
    Some(out)
}

pub fn decode(frame: &[u8]) -> Option<(&str, &[u8])> {
    let (&len, rest) = frame.split_first()?;
    let len = len as usize;
    if rest.len() < len {
        return None;
    }
    let topic = core::str::from_utf8(&rest[..len]).ok()?;
    Some((topic, &rest[len..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let f = encode("3/power", &[1, 2, 3]).unwrap();
        assert_eq!(decode(&f), Some(("3/power", &[1u8, 2, 3][..])));
        assert_eq!(decode(&[5, b'a']), None);
    }

    #[test]
    fn a_place_sends_only_under_its_own_name() {
        assert!(may_send("3", "3/power"));
        assert!(!may_send("3", "1/power"), "speaking for place 1");
        assert!(
            !may_send("3", "30/power"),
            "a longer place with the same start"
        );
        assert!(!may_send("3", "3"), "no topic after the place");
        assert!(
            !may_send("3", "roof-east/power"),
            "speaking for the simulated fleet"
        );
        assert!(may_send("hq", "anything/at/all"));
    }

    #[test]
    fn the_hub_reaches_one_place_or_all() {
        assert_eq!(hq_target("3/feedback"), Some("3"));
        assert_eq!(hq_target("8/feedback"), Some("8"));
        assert_eq!(hq_target(MODE), None, "the mode goes to every place");
        assert_eq!(hq_target("9/feedback"), None);
        assert_eq!(hq_target("roof-east/power"), None);
    }

    #[test]
    fn roles_are_the_hub_or_places_one_to_eight() {
        for ok in ["hq", "1", "8"] {
            assert!(valid_role(ok), "{ok}");
        }
        for bad in ["", "0", "9", "01", "here", "roof-east", "1/x", "HQ"] {
            assert!(!valid_role(bad), "{bad}");
        }
    }
}
