//! `player.goto`, `player.goto_status`, `player.goto_cancel`: drive the
//! local player somewhere with the bots' route brain (docs/runtime/agent-api.md,
//! "`player.goto`: drive the player somewhere"; the autopilot is
//! `bots::goto`, the glue `game::player_goto`). Wire types only.

use serde::{Deserialize, Serialize};

/// `player.goto`: start driving `player.0` to a point or a spot. An act.
pub const GOTO: &str = "player.goto";
/// `player.goto_status`: where the drive stands; may wait for its end
/// ([`GotoStatusParams::wait_ticks`]). A read.
pub const GOTO_STATUS: &str = "player.goto_status";
/// `player.goto_cancel`: stop driving and let go of the pad. An act; no
/// params.
pub const GOTO_CANCEL: &str = "player.goto_cancel";

/// The methods above.
pub const ALL: &[&str] = &[GOTO, GOTO_STATUS, GOTO_CANCEL];

/// A spot of the superbounce mode's scan, by its index among the spots of
/// its kind, best first: the indices `superbounce.verify` uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotRef {
    pub spot: usize,
    /// `bounce` (default) or `crouch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Where to go: a point (`[x, y, z]`, world units) or a spot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GotoTo {
    Point([f32; 3]),
    Spot(SpotRef),
}

/// `player.goto`'s params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GotoParams {
    pub to: GotoTo,
    /// How near a point counts as there, world units (default 0.3, 0.05 to
    /// 10). A spot is reached at its own approach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<f32>,
    /// Turn to this yaw (radians from +X toward +Y) on arriving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<f32>,
    /// Give up after this many ticks (default 3600, two minutes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ticks: Option<u32>,
    /// `stop` (default), `crouch_recipe` (prime at the crouch spot, or at
    /// the one the planner picks for a bounce spot) or `bounce` (a bounce
    /// spot's whole route, onto the strip and back down).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then: Option<String>,
}

/// `player.goto`'s result: the drive has started (or waits for the map's
/// nav mesh); read `player.goto_status`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GotoStarted {
    pub job: u32,
    /// `point`, `crouch` or `bounce`.
    pub kind: String,
    /// Where it ends: the point, a crouch spot's start, a bounce spot's
    /// approach (its stand point when that is not known yet).
    pub goal: [f32; 3],
    pub then: String,
    pub map: String,
    /// `ready`, or `loading` while the nav mesh is read or built.
    pub nav: String,
}

/// `player.goto_status`'s params.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GotoStatusParams {
    /// Answer once the drive is over, or after this many ticks (at most
    /// 290), whichever is first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ticks: Option<u64>,
}

/// Where the drive stands.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GotoStatus {
    /// `none`, `loading` (waiting for the nav mesh), `running`, `facing`
    /// (turning at the end), `done`, `failed` or `cancelled`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub then: String,
    /// The plan's steps (`walk`, `walk_crouched`, `prime`, `bounce`,
    /// `drop`, `fly`, `nav`), the one it is on, and that step's phase.
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub step: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phase: String,
    /// The level pair word is at -3 (read, or assumed after a prime).
    #[serde(default)]
    pub primed: bool,
    /// The player's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    /// World units left along the walk under way, else straight to the
    /// goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f32>,
    /// The walk under way: the player, then the waypoints ahead (at most
    /// 32).
    #[serde(default)]
    pub path: Vec<[f32; 3]>,
    /// Plans made again, and why the last was: `strayed`, `stuck`,
    /// `path_lost`, `stalled`, `pair_lost`.
    #[serde(default)]
    pub replans: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replanned: Option<String>,
    /// Ticks since it started.
    #[serde(default)]
    pub ticks: u32,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_target_is_a_point_or_a_spot() {
        let p: GotoParams = serde_json::from_value(json!({"to": [1.0, 2.0, 3.0]})).unwrap();
        assert_eq!(p.to, GotoTo::Point([1.0, 2.0, 3.0]));
        assert!(p.within.is_none() && p.then.is_none());
        let p: GotoParams = serde_json::from_value(json!({
            "to": {"spot": 2, "kind": "crouch"}, "then": "crouch_recipe", "face": 1.5,
            "within": 0.1, "timeout_ticks": 600
        }))
        .unwrap();
        assert_eq!(
            p.to,
            GotoTo::Spot(SpotRef {
                spot: 2,
                kind: Some("crouch".into())
            })
        );
        assert_eq!((p.face, p.timeout_ticks), (Some(1.5), Some(600)));
        assert!(serde_json::from_value::<GotoParams>(json!({"to": [1, 2, 3], "x": 1})).is_err());
        assert!(serde_json::from_value::<GotoParams>(json!({"to": {"spot": 1, "x": 1}})).is_err());
        assert!(serde_json::from_value::<GotoParams>(json!({})).is_err());
        assert_eq!(ALL, [GOTO, GOTO_STATUS, GOTO_CANCEL]);
    }

    #[test]
    fn an_empty_status_is_small() {
        let v = serde_json::to_value(GotoStatus {
            state: "none".into(),
            ..GotoStatus::default()
        })
        .unwrap();
        assert_eq!(v["state"], "none");
        assert!(v.get("goal").is_none() && v.get("error").is_none());
        assert_eq!(
            serde_json::from_value::<GotoStatus>(v).unwrap().state,
            "none"
        );
    }
}
