//! `superbounce.view` and `superbounce.view_test`: the superbounce bounce
//! view on the flashlight and the motion tracker (docs/runtime/agent-api.md,
//! "superbounce"; docs/game/superbounce.md, "The bounce view").
//! Wire types only. Game methods: `session.methods` lists them where the
//! game registered them (`game::superbounce_view::install`).

use serde::{Deserialize, Serialize};

/// The view's state now ([`View`]; no params).
pub const VIEW: &str = "superbounce.view";
/// Drives the view with no pad ([`ViewTestParams`], [`ViewTest`]).
pub const VIEW_TEST: &str = "superbounce.view_test";

/// Method names.
pub mod method {
    pub const ALL: &[&str] = &[super::VIEW, super::VIEW_TEST];
}

/// One spot on the tracker or the map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Blip {
    /// `bounce` (a landing) or `crouch`.
    pub kind: String,
    pub proven: bool,
    /// Right and ahead of the player over the range (the tracker's or the
    /// map's): inside the unit circle, or on it for a rim chevron.
    pub x: f64,
    pub y: f64,
    /// `same`, `above` or `below`: the spot's floor from the player's.
    pub level: String,
    /// Out of range: a chevron on the rim.
    pub rim: bool,
    /// The navpoint is on it.
    pub nav: bool,
}

/// The motion tracker's blips.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tracker {
    /// The variant shows the motion tracker.
    pub enabled: bool,
    /// The tracker's range, world units.
    pub range: f64,
    /// In range nearest first, then the nearest rim chevrons.
    pub blips: Vec<Blip>,
}

/// The map the tracker grows into while held.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Map {
    /// Floor polygons drawn.
    pub floors: usize,
    /// Outline edges drawn.
    pub edges: usize,
    /// Crouch-spot-to-landing lines drawn.
    pub pairs: usize,
    /// In range, then every proven spot out of it on the rim.
    pub blips: Vec<Blip>,
}

/// `superbounce.view`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// The superbounce mode is on. Everything else is empty while off.
    pub active: bool,
    /// `none`, `pulse` (a tap's pulse is showing) or `hold`.
    pub mode: String,
    /// The flashlight action at the last tick.
    pub button: bool,
    /// `superbounce.view_test {"mode":"hold"}` is holding.
    pub test_hold: bool,
    /// Seconds since the pulse left, while it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulse_age: Option<f64>,
    /// How far the pulse's ring is from the player, world units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulse_front: Option<f64>,
    /// The tracker's growth into the map, 0 to 1.
    pub grow: f64,
    /// The spots the view knows (the failed ones left out).
    pub spots: usize,
    /// Spots marked in the world now.
    pub marks: usize,
    /// Of those, the ones behind a wall (dashed).
    pub hidden: usize,
    /// Spots with a line-of-sight answer.
    pub tested: usize,
    pub tracker: Tracker,
    /// While held or shrinking back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<Map>,
}

/// `superbounce.view_test`'s params.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewTestParams {
    /// `pulse` (a tap), `hold` (held until `none`) or `none` (let go, and
    /// the pulse stopped).
    pub mode: String,
}

/// `superbounce.view_test`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewTest {
    pub mode: String,
    pub test_hold: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_test_takes_a_mode_and_nothing_else() {
        let p: ViewTestParams =
            serde_json::from_value(serde_json::json!({"mode": "hold"})).unwrap();
        assert_eq!(p.mode, "hold");
        assert!(
            serde_json::from_value::<ViewTestParams>(serde_json::json!({"mode": "hold", "x": 1}))
                .is_err()
        );
        assert!(serde_json::from_value::<ViewTestParams>(serde_json::json!({})).is_err());
    }

    #[test]
    fn a_quiet_view_leaves_out_the_pulse_and_the_map() {
        let v = View {
            active: true,
            mode: "none".into(),
            ..Default::default()
        };
        let j = serde_json::to_value(&v).unwrap();
        assert!(j.get("pulse_age").is_none() && j.get("map").is_none());
        assert_eq!(j["tracker"]["blips"], serde_json::json!([]));
        assert_eq!(serde_json::from_value::<View>(j).unwrap(), v);
        assert_eq!(method::ALL, [VIEW, VIEW_TEST]);
    }
}
