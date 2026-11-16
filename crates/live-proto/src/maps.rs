//! Custom and modded maps over Live (docs/maps.md §5): which maps a hopper
//! allows.
//!
//! A map is named exactly by its [`MapId`] (a base plus layers); sessions
//! and games carry it beside the map's name.
use serde::{Deserialize, Serialize};

pub use content::MapId;

/// `HopperPinsSet`: the maps a matchmaking hopper (playlist) allows beside
/// retail ones, server-signed on [`hopper_stream`].
pub const HOPPER_PINS_SET: &str = "HopperPinsSet";

/// The stream a hopper's pins are kept on.
pub fn hopper_stream(hopper_id: u32) -> String {
    format!("hopper-{hopper_id}")
}

/// A hopper entry's pinned maps. The game's hopper file
/// (`multiplayer_hopper.dat`) has no room for a `MapId`, so the pin is kept
/// by the server beside it, keyed by the `hopper_id` sessions advertise.
/// A ranked (non-custom) session may only be on a retail map or on one
/// pinned here for its hopper.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HopperPin {
    pub hopper_id: u32,
    /// Empty: retail maps only.
    #[serde(default)]
    pub map_ids: Vec<MapId>,
}

/// At most this many maps are pinned on one hopper.
pub const MAX_PINS: usize = 64;

impl HopperPin {
    pub fn validate(&self) -> Result<(), String> {
        if self.map_ids.len() > MAX_PINS {
            return Err(format!("at most {MAX_PINS} maps are pinned on a hopper"));
        }
        let mut seen = std::collections::BTreeSet::new();
        if !self.map_ids.iter().all(|m| seen.insert(*m)) {
            return Err("a map is pinned twice".into());
        }
        Ok(())
    }

    pub fn allows(&self, id: &MapId) -> bool {
        self.map_ids.contains(id)
    }
}

/// Whether a session with these `flags` (`custom`, `hopper`, `ranked`,
/// `coop`) is held to the ranked rule: retail or pinned maps only.
pub fn ranked_flags(flags: &str) -> bool {
    flags != "custom" && flags != "coop"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pins_parse_validate_and_allow() {
        let a = MapId([1; 32]);
        let p: HopperPin =
            serde_json::from_value(json!({"hopper_id": 7, "map_ids": [a.to_string()]})).unwrap();
        assert!(p.validate().is_ok());
        assert!(p.allows(&a));
        assert!(!p.allows(&MapId([2; 32])));
        let empty: HopperPin = serde_json::from_value(json!({"hopper_id": 7})).unwrap();
        assert!(empty.map_ids.is_empty());
        let twice = HopperPin {
            hopper_id: 1,
            map_ids: vec![a, a],
        };
        assert!(twice.validate().is_err());
        let many = HopperPin {
            hopper_id: 1,
            map_ids: (0..=MAX_PINS as u8).map(|i| MapId([i; 32])).collect(),
        };
        assert!(many.validate().is_err());
        assert!(serde_json::from_value::<HopperPin>(json!({"hopper_id": 1, "x": 1})).is_err());
        assert_eq!(hopper_stream(3), "hopper-3");
    }

    #[test]
    fn ranked_is_everything_but_custom_and_coop() {
        assert!(!ranked_flags("custom"));
        assert!(!ranked_flags("coop"));
        assert!(ranked_flags("hopper"));
        assert!(ranked_flags("ranked"));
    }
}
