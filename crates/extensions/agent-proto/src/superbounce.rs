//! `superbounce.*`: the superbounce game type's state (docs/runtime/agent-api.md,
//! "superbounce"; docs/game/superbounce.md, "The superbounce game type"
//! and "Spots in the geometry").
//! Wire types only. A game method: `session.methods` lists it where the game
//! registered it (`game::agent_glue`).

use serde::{Deserialize, Serialize};

/// Method names.
pub mod method {
    /// The local player's primed state and the spot scan
    /// ([`super::State`]; no params).
    pub const STATE: &str = "superbounce.state";

    /// Turns the mode on or off in the running game, over the game type
    /// and `H2_SUPERBOUNCE` ([`super::SetParams`], [`super::Set`]).
    pub const SET: &str = "superbounce.set";

    /// The spots the bots know for a map ([`super::SPOTS`]).
    pub const SPOTS: &str = super::SPOTS;

    pub const ALL: &[&str] = &[STATE, SET, SPOTS, super::ASSIST, super::ASSIST_TEST];
}

// ---- superbounce.state ----

/// The scan of the loaded map's spots.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scan {
    /// `idle` (nothing asked for yet), `scanning`, `ready` or `failed`.
    pub status: String,
    /// The map the spots are for, once a scan has started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Why a scan failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Bounce spots found.
    pub bounce: usize,
    /// Forced-crouch spots found.
    pub crouch: usize,
}

/// The bounce spot nearest the player.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Nearest {
    /// World units, to the spot's edge.
    pub distance: f64,
    /// Estimated launch speed straight up, world units a second.
    pub launch: f64,
    /// How far to fall onto it, world units (0 for a standing jump).
    pub fall: f64,
    pub edge: [[f64; 3]; 2],
}

/// `superbounce.state`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Whether the mode is on: the variant's flag, the built-in
    /// Superbounce variant, or `H2_SUPERBOUNCE=1`.
    pub active: bool,
    /// The level pair's word is -3.
    pub primed: bool,
    /// The level pair's word, eight hex digits, as `havok.agents` prints it;
    /// absent when the player has no level pair (between lives, asleep).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<String>,
    /// The same word as a signed number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_signed: Option<i32>,
    /// The local player's position, world units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearest: Option<Nearest>,
    pub scan: Scan,
    /// The local player's biped is off the ground (biped flags bit 0, as
    /// `world.object`'s `physics.airborne` reads it); absent without a
    /// biped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airborne: Option<bool>,
    /// The local player's vertical velocity, world units a second (up is
    /// positive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vz: Option<f64>,
    /// The way the local player's unit aims, radians from +X toward +Y:
    /// the first-person camera's yaw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f64>,
    /// The charge meter; absent while hidden (not primed, and not crouched
    /// by a crouch spot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge: Option<Charge>,
    /// The recipe's step on the HUD's prompt line, while `under` or `held`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verb: Option<Verb>,
    /// The navpoint's target: the nearest crouch spot while not primed,
    /// the nearest strip while primed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_target: Option<NavTarget>,
    /// The landing the assist predicts, while airborne and falling toward
    /// a strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<Predicted>,
    /// The last landing within 0.5 of a strip, filed at touchdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_landing: Option<Landing>,
    /// Whether the standing capsule fits at the player's origin (the
    /// mode's capsule test against the scanned collision); absent before
    /// the scan is ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_fits: Option<bool>,
}

/// How far through priming the player is, or that the charge just went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargePhase {
    /// Crouched within 1.5 of a crouch spot, the standing capsule fits.
    Near,
    /// Crouched, crouch held, the standing capsule does not fit.
    Under,
    /// Crouch let go and the crouch request still set: the forced crouch.
    Held,
    /// The word is -3 and the player is not crouching.
    Primed,
    /// The charge just went: shown for a second.
    Lost,
}

/// The charge meter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Charge {
    pub phase: ChargePhase,
    /// 0..1: 0 near, 1/3 under, 2/3 held, 1 less `still_ticks` over 38
    /// primed, 0 lost.
    pub fill: f64,
    /// Ticks the primed body has been still (speed under 0.03 units/s); 0
    /// while not primed.
    pub still_ticks: u32,
}

/// The prompt line's verb for a crouch step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verb {
    /// "Let go of (crouch)".
    LetGoOfCrouch,
    /// "Walk out to stand up".
    WalkOutToStand,
}

/// What the navpoint points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavKind {
    Crouch,
    Strip,
}

/// The navpoint's target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavTarget {
    /// The spot's stand point, world units.
    pub at: [f64; 3],
    pub kind: NavKind,
    /// From the player's origin, world units.
    pub distance: f64,
}

/// Whether a predicted landing is on the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Course {
    /// The coasting landing is inside the slot.
    OnSlot,
    /// It is not, and the fall assist can still bring it in.
    Fixable,
    /// It is not, and the assist cannot.
    Miss,
}

/// The landing the assist predicts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predicted {
    /// Where the player lands coasting, at the strip's floor height.
    pub at: [f64; 3],
    pub verdict: Course,
    /// The vertical speed at touchdown, units/s (positive).
    pub speed: f64,
    /// The touchdown speed the strip needs, units/s.
    pub need: f64,
}

/// What a landing by a strip came to, by its first failed test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandingResult {
    /// The vertical speed came out over 8 units/s.
    Bounce,
    /// The word was not -3 on the tick before.
    NotPrimed,
    /// The origin was outside the slot.
    Missed,
    /// The speed in was under the strip's need.
    TooSlow,
    /// Primed, in the slot, fast enough, no bounce.
    NoBounce,
}

/// A landing filed at touchdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Landing {
    pub result: LandingResult,
    /// The game tick it was filed on.
    pub tick: u32,
    /// The origin on the tick before touchdown, world units.
    pub at: [f64; 3],
}

// ---- superbounce.set ----

/// `superbounce.set`'s params.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetParams {
    /// `true` forces the mode on, `false` forces it off, `null` (or absent)
    /// follows the game again: the variant, its flag and `H2_SUPERBOUNCE`.
    #[serde(default)]
    pub on: Option<bool>,
}

/// `superbounce.set`'s result. The mode follows within a second (the
/// on/off check runs every 30 ticks); `superbounce.state` says when.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Set {
    /// The override now: `on`, `off` or `game`.
    pub mode: String,
}

// ---- superbounce.spots ----

/// `superbounce.spots`: the spots the bots know for a map. A read.
pub const SPOTS: &str = "superbounce.spots";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotsParams {
    /// The map's short name (`ascension`); default the loaded map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Only `bounce` or `crouch` spots; default both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One spot. Coordinates are world units, Z up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotRecord {
    /// `bounce`: a slot a primed landing launches out of. `crouch`: a
    /// place a standing player is forced into a crouch, which primes.
    pub kind: String,
    /// `measured` (a game run bounced from it) or `scan` (found in the
    /// collision geometry).
    pub source: String,
    /// The line to draw.
    pub edge: [[f32; 3]; 2],
    /// Where a player stands to use it: a unit's origin, standing.
    pub stand: [f32; 3],
    /// Bounce: how far to fall onto the slot (0 for a standing jump).
    pub fall: f32,
    /// Bounce: the launch speed straight up (world units a second).
    pub launch: f32,
    /// A world direction (unit, X and Y) to drift along when landing, or
    /// zero.
    pub drift: [f32; 2],
    /// 0..1: how sure the scan is; 1 for a measured spot.
    pub score: f32,
    /// Bounce: the slot's width, the gap between its two joints. Crouch:
    /// the widest the forced band measures.
    #[serde(default)]
    pub width: f32,
    /// Bounce: the downward speed (world units a second) the landing must
    /// touch down at; `fall` is worked from it. Crouch: 0.
    #[serde(default)]
    pub need: f32,
    /// Crouch: `proven` (a measured priming place) or `likely`. Bounce:
    /// `likely`.
    #[serde(default = "likely")]
    pub status: String,
    /// Crouch: the unit direction (X and Y) from `stand` to the standing
    /// room beside it. Bounce: zero.
    #[serde(default)]
    pub out: [f32; 2],
    /// Crouch: up to 8 edges of the faces the standing capsule hits at
    /// `stand`, at their own height. Bounce: empty.
    #[serde(default)]
    pub lip: Vec<[[f32; 3]; 2]>,
}

fn likely() -> String {
    "likely".into()
}

/// The result of `superbounce.spots`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spots {
    /// The map the spots are for.
    pub map: String,
    /// The map has a scan (the nav worker has made or read one); the
    /// measured spots are listed either way.
    pub scanned: bool,
    /// Measured spots first, then scanned, best first.
    pub spots: Vec<SpotRecord>,
}

// ---- superbounce.assist, superbounce.assist_test (the fall assist) ----
//
// The hold-X assist (crates/extensions/game/src/superbounce_assist.rs;
// docs/game/superbounce.md, "Hold X: the fall assist").

/// `superbounce.assist`: whether the fall assist is steering the player,
/// and where to. A read; no params.
pub const ASSIST: &str = "superbounce.assist";

/// `superbounce.assist_test`: holds the assist's button as if pressed, for
/// an agent with no pad ([`AssistTestParams`], [`AssistTest`]). An act.
pub const ASSIST_TEST: &str = "superbounce.assist_test";

/// `superbounce.assist`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Assist {
    /// The superbounce mode is on (the assist only works with it on).
    pub active: bool,
    /// `superbounce.assist_test` is holding the button.
    pub test_hold: bool,
    /// The assist steered the player at its last pad read.
    pub engaged: bool,
    /// The strip's stand point, world units; absent when not engaged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<[f64; 3]>,
    /// The point being aimed at: the stand point a little before along the
    /// drift (X and Y).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<[f64; 2]>,
    /// Where the player lands if the stick is let go (X and Y).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<[f64; 2]>,
    /// Seconds until touchdown at the strip's height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fall_time: Option<f64>,
    /// The left stick sent to the guest, `[x (right), y (forward)]`, -1..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stick: Option<[f64; 2]>,
}

/// `superbounce.assist_test`'s params.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistTestParams {
    /// `true` holds the button, `false` lets go.
    pub hold: bool,
}

/// `superbounce.assist_test`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistTest {
    /// The hold now.
    pub hold: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_omits_what_is_absent() {
        let s = State {
            active: true,
            primed: true,
            word: Some("fffffffd".into()),
            word_signed: Some(-3),
            position: Some([1.0, 2.0, 3.0]),
            nearest: None,
            scan: Scan {
                status: "ready".into(),
                map: Some("ascension".into()),
                error: None,
                bounce: 4,
                crouch: 2,
            },
            airborne: Some(true),
            vz: Some(-4.5),
            yaw: None,
            charge: Some(Charge {
                phase: ChargePhase::Held,
                fill: 0.5,
                still_ticks: 0,
            }),
            verb: Some(Verb::WalkOutToStand),
            nav_target: Some(NavTarget {
                at: [1.0, 2.0, 3.0],
                kind: NavKind::Crouch,
                distance: 2.5,
            }),
            landing: Some(Predicted {
                at: [1.0, 2.0, -3.0],
                verdict: Course::OnSlot,
                speed: 5.5,
                need: 4.0,
            }),
            last_landing: Some(Landing {
                result: LandingResult::NotPrimed,
                tick: 6753,
                at: [1.0, 2.0, -3.0],
            }),
            standing_fits: Some(false),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["charge"]["phase"], "held");
        assert_eq!(v["verb"], "walk_out_to_stand");
        assert_eq!(v["nav_target"]["kind"], "crouch");
        assert_eq!(v["landing"]["verdict"], "on_slot");
        assert_eq!(v["last_landing"]["result"], "not_primed");
        assert_eq!(v["standing_fits"], false);
        let bare = serde_json::to_value(State::default()).unwrap();
        for absent in [
            "charge",
            "verb",
            "nav_target",
            "landing",
            "last_landing",
            "standing_fits",
        ] {
            assert!(bare.get(absent).is_none(), "{absent}");
        }
        for (r, name) in [
            (LandingResult::Bounce, "bounce"),
            (LandingResult::Missed, "missed"),
            (LandingResult::TooSlow, "too_slow"),
            (LandingResult::NoBounce, "no_bounce"),
        ] {
            assert_eq!(serde_json::to_value(r).unwrap(), name);
        }
        assert_eq!(
            serde_json::to_value(Verb::LetGoOfCrouch).unwrap(),
            "let_go_of_crouch"
        );
        assert_eq!(serde_json::to_value(Course::Fixable).unwrap(), "fixable");
        assert_eq!(v["word"], "fffffffd");
        assert!(v.get("nearest").is_none() && v["scan"].get("error").is_none());
        assert_eq!(v["airborne"], true);
        assert_eq!(v["vz"], -4.5);
        assert!(v.get("yaw").is_none());
        assert_eq!(serde_json::from_value::<State>(v).unwrap(), s);
        let a = Assist {
            engaged: true,
            stick: Some([0.5, -1.0]),
            ..Assist::default()
        };
        let v = serde_json::to_value(&a).unwrap();
        assert!(v.get("target").is_none() && v["stick"][1] == -1.0);
        assert_eq!(serde_json::from_value::<Assist>(v).unwrap(), a);
        assert!(
            serde_json::from_value::<AssistTestParams>(serde_json::json!({"hold": true, "x": 1}))
                .is_err()
        );
        assert_eq!(
            method::ALL,
            [
                method::STATE,
                method::SET,
                method::SPOTS,
                ASSIST,
                ASSIST_TEST
            ]
        );
    }
}
