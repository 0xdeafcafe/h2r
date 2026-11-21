//! `superbounce.*`: the superbounce game type's state (docs/runtime/agent-api.md,
//! "superbounce"; docs/game/superbounce.md, "The superbounce game type"
//! and "Spots in the geometry").
//! Wire types only. A game method: `session.methods` lists it where the game
//! registered it (`game::agent_glue`).

use std::collections::BTreeMap;

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

    /// Starts a batch of trials of the loaded map's spots in the game
    /// ([`super::VerifyParams`], [`super::VerifyStarted`]).
    pub const VERIFY: &str = super::VERIFY;

    /// The batch's progress and results ([`super::VerifyStatusParams`],
    /// [`super::VerifyReport`]).
    pub const VERIFY_STATUS: &str = super::VERIFY_STATUS;

    /// Stops the batch and lets go of the seat ([`super::VerifyReport`]).
    pub const VERIFY_CANCEL: &str = super::VERIFY_CANCEL;

    /// Files a bounce at a chain count for the first local player, as a
    /// real one would show it ([`super::MedalTestParams`],
    /// [`super::MedalTest`]).
    pub const MEDAL_TEST: &str = super::MEDAL_TEST;

    pub const ALL: &[&str] = &[
        STATE,
        SET,
        SPOTS,
        MEDAL_TEST,
        super::ASSIST,
        super::ASSIST_TEST,
        VERIFY,
        VERIFY_STATUS,
        VERIFY_CANCEL,
    ];
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
    /// The next thing to do on the HUD's prompt line: a crouch step
    /// while `under` or `held`, the warning while primed and still, the
    /// new charge after a loss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verb: Option<Verb>,
    /// Why the charge went: while `lost`, and while the verb is
    /// `charge_again`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_reason: Option<LostReason>,
    /// The navpoint's target: the nearest crouch spot while not primed;
    /// while primed the chosen strip (the nearest one the player's floor
    /// is high enough for, else the nearest; falling, the predicted
    /// landing's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_target: Option<NavTarget>,
    /// Where to jump from for the chosen strip, while primed: the floor
    /// mark's place (docs/game/superbounce.md, "Floor marks"); absent
    /// when none is found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takeoff: Option<[f64; 3]>,
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
    /// The medals this game, every player's (docs/game/superbounce.md,
    /// "Medals").
    #[serde(default)]
    pub medals: Medals,
}

/// `superbounce.state`'s `medals`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Medals {
    /// A row for each player that has earned a medal this game, by slot.
    pub players: Vec<MedalPlayer>,
    /// The last 8 awards, oldest first.
    pub recent: Vec<MedalAward>,
}

/// One player's medals this game.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MedalPlayer {
    /// The player's slot.
    pub player: u8,
    /// The count of its last chain.
    pub chain: u32,
    /// Whether its next bounce adds to that chain.
    pub live: bool,
    /// Each medal id's total this game (`superbounce`, `double-bounce`,
    /// `triple-bounce`, `quad-bounce`, `chain-bounce`, `bouncetacular`,
    /// `bounced`).
    pub counts: BTreeMap<String, u32>,
}

/// One award: a bounce, or a Bounced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MedalAward {
    /// The player's slot.
    pub player: u8,
    /// The game tick.
    pub tick: u32,
    /// The chain's count: the bounce's, or for a Bounced the chain it was
    /// in (0 if none).
    pub chain: u32,
    /// The ids the award earned, lowest first.
    pub medals: Vec<String>,
}

// ---- superbounce.medal_test ----

/// `superbounce.medal_test`: files a bounce at a chain count, or a
/// Bounced, for the first local player, as the chain's rule gives it: the
/// HUD line and `superbounce.state`'s `medals` as a real one gives them,
/// nothing sent or recorded. An act.
pub const MEDAL_TEST: &str = "superbounce.medal_test";

/// `superbounce.medal_test`'s params: `chain` (1 and up) or `bounced`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MedalTestParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<u32>,
    #[serde(default)]
    pub bounced: bool,
}

/// `superbounce.medal_test`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MedalTest {
    /// The first local player's slot.
    pub player: u8,
    pub chain: u32,
    /// The ids filed, lowest first.
    pub medals: Vec<String>,
    /// The HUD's line: the highest medal's glyph and words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
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

/// The prompt line's verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verb {
    /// "Let go of (crouch)".
    LetGoOfCrouch,
    /// "Walk out to stand up".
    WalkOutToStand,
    /// "Keep moving or lose the charge": primed and still for 19 ticks or
    /// more.
    KeepMoving,
    /// "Find a crouch spot to charge again": for 150 ticks after a loss,
    /// while no crouch step has begun.
    ChargeAgain,
}

/// Why the charge went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LostReason {
    /// The word left -3 after 30 still ticks or more: the body slept.
    StoodStill,
    /// A crouch while primed.
    Crouched,
    /// The word left -3 for another reason.
    Unknown,
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
    /// A strip's fall (world units), the drop the navpoint's number
    /// gives in metres; 0 for a crouch spot.
    #[serde(default)]
    pub fall: f64,
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
    /// How far the landing is outside the slot, world units (0 inside).
    #[serde(default)]
    pub miss: f64,
    /// In the slot and at least `need`: the lock-on is red.
    #[serde(default)]
    pub locked: bool,
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
    /// `proven` (a measured place, or one the map's verified file proved),
    /// `failed` (the verified file tried it and it never worked) or
    /// `likely` (found in the geometry, untried).
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
    /// Bounce: `slot` (two joints) or `crease` (one concave crease, a
    /// candidate for the verifier). Crouch: `slot`.
    #[serde(default = "slot")]
    pub shape: String,
}

fn slot() -> String {
    "slot".into()
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
    /// The player has turned the assist on: the `superbounce.fall_assist`
    /// setting (`settings.set` changes it). Off, it never steers.
    #[serde(default)]
    pub enabled: bool,
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

// ---- superbounce.verify (trials of the spots in the game) ----
//
// The verifier (crates/extensions/game/src/superbounce_verify.rs;
// docs/game/superbounce.md, "Verifying spots").

/// `superbounce.verify`: starts a batch of trials of the loaded map's spots,
/// run in the game tick by tick on one seat. An act.
pub const VERIFY: &str = "superbounce.verify";
/// `superbounce.verify_status`: the batch's progress and, once done, its
/// results. A read; may wait for the batch ([`VerifyStatusParams::wait_ticks`]).
pub const VERIFY_STATUS: &str = "superbounce.verify_status";
/// `superbounce.verify_cancel`: stops the batch. An act; no params.
pub const VERIFY_CANCEL: &str = "superbounce.verify_cancel";

/// An explicit spot to try, instead of the scan's by index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyPoint {
    /// Crouch: where the forced crouch lets go (the scan's `stand`).
    /// Drop: the strip's stand point. A unit's origin.
    pub stand: [f32; 3],
    /// Crouch: the way out, X and Y (normalised); default +X.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<[f32; 2]>,
    /// Drop: the strip's midline, floor height; default a 0.3-unit line
    /// across `drift` through `stand`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<[[f32; 3]; 2]>,
    /// Drop: across the strip from the crease; default +Y.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<[f32; 2]>,
    /// Drop: the slot's width (default 0.2) and the touchdown speed it
    /// needs (default 4.3), for the landing classifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need: Option<f32>,
}

/// `superbounce.verify`'s params. Every field but `kind` has a default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyParams {
    /// `crouch`: does a forced crouch here turn the word to -3? `drop`: does
    /// a primed fall onto this strip launch? `route`: does a bot's bounce
    /// route (`bots.bounce_to`) work end to end?
    pub kind: String,
    /// The scan's spots of that kind by index (the order
    /// `superbounce.verify {dry_run}` lists, best first); default every
    /// one, up to `max_spots`. Not with `points`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spots: Option<Vec<usize>>,
    /// Explicit spots instead of the scan's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<VerifyPoint>>,
    /// With no `spots` or `points`: this many of the scan's, best first
    /// (default 12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spots: Option<usize>,
    /// The seat: `player.<n>` (default `player.0`, the first local
    /// player, driven through its pad), or a brain-driven bot by gamertag
    /// or handle (`bots.stats`), driven by holding its pad. `route` needs a
    /// bot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// Crouch: starting offsets from the spot's start (X and Y, world
    /// units). Default `[[0,0]]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offsets: Option<Vec<[f32; 2]>>,
    /// Crouch: approach turns, degrees from straight in along the way out.
    /// Default `[0, 25, -25]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<Vec<f32>>,
    /// Crouch: how far outside the lip each trial starts, along its
    /// approach, world units. Default `[0.3, 0.6]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distances: Option<Vec<f32>>,
    /// Drop: offsets across the slot from its midline, toward the drift
    /// (world units). Default `[-0.05, 0, 0.05, 0.1, 0.15]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub across: Option<Vec<f32>>,
    /// Drop: offsets along the slot from its middle (world units). Default
    /// `[0]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub along: Option<Vec<f32>>,
    /// Drop: heights over the strip to fall from, at rest (world units).
    /// Default `[1.5, 2.5, 3.5]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heights: Option<Vec<f32>>,
    /// Drop: the word before the landing. `set` (default): written -3 every
    /// tick of the fall while there is a pair; `none`: written 0 (the
    /// control); `keep`: left as the game has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prime: Option<String>,
    /// Drop: the stick along the strip's drift while falling, 0..1
    /// (default 0: straight down).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_stick: Option<f32>,
    /// Ticks: settle after a teleport (default 20), crouch held under the
    /// lip (default 15), walk out (default 30), fall (most, default 120),
    /// a route (most, default 3600).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settle_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fall_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_ticks: Option<u32>,
    /// Route: where to send the bot (default 6 units straight up from the
    /// nearest strip's stand, so any launch counts), and `help` or `fun`
    /// (default `fun`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Route: repeat the route this many times (default 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// Keep the per-tick trace of this trial (by its index).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<usize>,
    /// Write the outcome to the map's verified file (default true; not for
    /// `points` or `route`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save: Option<bool>,
    /// Plan the trials and list the spots, run nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// One spot of a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifySpot {
    /// The scan's index of that kind; absent for an explicit point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub kind: String,
    pub stand: [f32; 3],
    /// The scan's status before the batch: `likely`, `proven`, `failed`.
    pub status: String,
    pub score: f32,
}

/// One planned trial.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyPlanned {
    /// Index into the batch's `spots`.
    pub spot: usize,
    /// Crouch: the start (where it is teleported). Drop: the drop's origin.
    pub start: [f32; 3],
    /// Crouch: the facing and walk-in direction, degrees about +Z from +X.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
    /// Crouch: the start's distance outside the lip along the approach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f32>,
    /// The offset tried (crouch: X and Y from the spot's start; drop:
    /// across and along the slot).
    pub offset: [f32; 2],
    /// Drop: the height over the strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
}

/// `superbounce.verify`'s result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyStarted {
    /// The batch's number; `superbounce.verify_status` reports on the
    /// newest.
    pub job: u32,
    pub kind: String,
    pub seat: String,
    pub map: String,
    pub spots: Vec<VerifySpot>,
    pub trials: Vec<VerifyPlanned>,
    /// A guess at the batch's length in game ticks (30 a second).
    pub estimated_ticks: u64,
    /// Nothing runs: `dry_run` was set.
    pub dry_run: bool,
}

/// `superbounce.verify_status`'s params.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyStatusParams {
    /// Wait for the batch to end, up to this many ticks (at most 300), and
    /// answer then; default answer at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ticks: Option<u64>,
    /// Leave out the per-trial list (the per-spot summary and lines stay).
    #[serde(default)]
    pub brief: bool,
}

/// One tick of a traced trial.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VerifyTick {
    /// Ticks since the trial's first move (after its settle).
    pub t: u32,
    /// The phase: `in`, `release`, `out` (crouch); `fall`, `after`
    /// (drop); a route's step.
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vz: Option<f32>,
    pub airborne: bool,
    /// The crouch request (unit flags bit 23).
    pub request: bool,
    /// The crouch button as the trial pressed it.
    pub crouch_button: bool,
    /// The crouch amount, 0 standing to 1 crouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crouch: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_fits: Option<bool>,
    /// The level pair word, signed; absent with no pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<i32>,
}

/// One trial's outcome.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VerifyTrial {
    pub index: usize,
    /// Index into the batch's `spots`.
    pub spot: usize,
    pub start: [f32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f32>,
    pub offset: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    /// Crouch: primed. Drop: launched. Route: the route ended `done` with
    /// a bounce.
    pub ok: bool,
    /// Crouch: the crouch was down (amount 0.9 or more) when the walk in
    /// started: crouched before walking in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crouched_first: Option<bool>,
    /// Crouch: the standing capsule did not fit at some tick under the lip
    /// (the mode's capsule test; absent without it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<bool>,
    /// Crouch: the request stayed set with the button up (forced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced: Option<bool>,
    /// Crouch: ticks from letting go to the word reading -3, and where.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primed_tick: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primed_at: Option<[f32; 3]>,
    /// Crouch: how close (world units) the body got to the stand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closest: Option<f32>,
    /// Drop: ticks from the release to the contact tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_tick: Option<u32>,
    /// Drop: the speed in (down, units/s) and the best vertical speed out
    /// (up) over the contact tick and the three after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_in: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vz_out: Option<f32>,
    /// Drop: the origin on the tick before contact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed_at: Option<[f32; 3]>,
    /// Drop: the word on the tick before contact, signed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_before: Option<i32>,
    /// Drop and route: the landing classifier's verdict (`bounce`,
    /// `not_primed`, `missed`, `too_slow`, `no_bounce`), or `no_touchdown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<String>,
    /// Route: the step it ended on and why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Route: each step and phase change, `<tick> <step>/<phase> <what>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    /// What went wrong outside the physics (a teleport refused, no pair to
    /// write, the seat gone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The best trial of a spot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VerifyBest {
    pub trial: usize,
    pub offset: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    /// Drop: its launch (units/s up).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<f32>,
}

/// One spot's outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifySpotResult {
    /// Index into the batch's `spots`.
    pub spot: usize,
    pub trials: u32,
    pub ok: u32,
    /// `proven` (one worked), `failed` (all were tried and none did),
    /// `untried` (the batch ended first).
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<VerifyBest>,
    /// Why the failures failed, by count (`not_forced`, `forced_no_word`,
    /// `not_primed`, `missed`, `too_slow`, `no_bounce`, ...).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub why: std::collections::BTreeMap<String, u32>,
}

/// `superbounce.verify_status`'s result (and `superbounce.verify_cancel`'s).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VerifyReport {
    /// `none` (no batch yet), `running`, `done`, `failed` or `cancelled`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub seat: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub map: String,
    /// Trials finished and planned.
    pub done: usize,
    pub total: usize,
    /// The trial running and its phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spots: Vec<VerifySpot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<VerifySpotResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trials: Vec<VerifyTrial>,
    /// The `trace` trial's ticks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<VerifyTick>,
    /// One line a spot, e.g. `crouch #3 (-10.91, 0.72, -1.16): primed 4/9,
    /// best offset (0.06, -0.03) yaw -121`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// The verified file written, once the batch is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
            lost_reason: Some(LostReason::StoodStill),
            nav_target: Some(NavTarget {
                at: [1.0, 2.0, 3.0],
                kind: NavKind::Crouch,
                distance: 2.5,
                fall: 0.0,
            }),
            takeoff: None,
            landing: Some(Predicted {
                at: [1.0, 2.0, -3.0],
                verdict: Course::OnSlot,
                speed: 5.5,
                need: 4.0,
                miss: 0.0,
                locked: true,
            }),
            last_landing: Some(Landing {
                result: LandingResult::NotPrimed,
                tick: 6753,
                at: [1.0, 2.0, -3.0],
            }),
            standing_fits: Some(false),
            medals: Medals {
                players: vec![MedalPlayer {
                    player: 5,
                    chain: 2,
                    live: true,
                    counts: [
                        ("superbounce".to_owned(), 2),
                        ("double-bounce".to_owned(), 1),
                    ]
                    .into_iter()
                    .collect(),
                }],
                recent: vec![MedalAward {
                    player: 5,
                    tick: 6800,
                    chain: 2,
                    medals: vec!["superbounce".into(), "double-bounce".into()],
                }],
            },
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["medals"]["players"][0]["counts"]["double-bounce"], 1);
        assert_eq!(v["medals"]["players"][0]["live"], true);
        assert_eq!(v["medals"]["recent"][0]["medals"][1], "double-bounce");
        // An older reply without medals reads as none.
        let mut old = v.clone();
        old.as_object_mut().unwrap().remove("medals");
        let back: State = serde_json::from_value(old).unwrap();
        assert_eq!(back.medals, Medals::default());
        assert_eq!(v["charge"]["phase"], "held");
        assert_eq!(v["verb"], "walk_out_to_stand");
        assert_eq!(v["lost_reason"], "stood_still");
        assert_eq!(v["nav_target"]["kind"], "crouch");
        assert_eq!(v["landing"]["verdict"], "on_slot");
        assert_eq!(v["landing"]["locked"], true);
        assert_eq!(v["nav_target"]["fall"], 0.0);
        assert_eq!(v["last_landing"]["result"], "not_primed");
        assert_eq!(v["standing_fits"], false);
        let bare = serde_json::to_value(State::default()).unwrap();
        for absent in [
            "charge",
            "verb",
            "lost_reason",
            "nav_target",
            "takeoff",
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
        assert_eq!(
            serde_json::to_value(Verb::KeepMoving).unwrap(),
            "keep_moving"
        );
        assert_eq!(
            serde_json::to_value(Verb::ChargeAgain).unwrap(),
            "charge_again"
        );
        assert_eq!(
            serde_json::to_value(LostReason::Crouched).unwrap(),
            "crouched"
        );
        assert_eq!(serde_json::to_value(Course::Fixable).unwrap(), "fixable");
        assert_eq!(v["word"], "fffffffd");
        assert!(v.get("nearest").is_none() && v["scan"].get("error").is_none());
        assert_eq!(v["airborne"], true);
        assert_eq!(v["vz"], -4.5);
        assert!(v.get("yaw").is_none());
        assert_eq!(serde_json::from_value::<State>(v).unwrap(), s);
        let a = Assist {
            enabled: true,
            engaged: true,
            stick: Some([0.5, -1.0]),
            ..Assist::default()
        };
        let v = serde_json::to_value(&a).unwrap();
        assert!(v.get("target").is_none() && v["stick"][1] == -1.0);
        assert_eq!(serde_json::from_value::<Assist>(v).unwrap(), a);
        // A reply without the field reads as off.
        let old = serde_json::json!({"active": true, "test_hold": false, "engaged": false});
        assert!(!serde_json::from_value::<Assist>(old).unwrap().enabled);
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
                ASSIST_TEST,
                VERIFY,
                VERIFY_STATUS,
                VERIFY_CANCEL
            ]
        );
    }

    #[test]
    fn verify_params_default_and_refuse_unknown_fields() {
        let p: VerifyParams =
            serde_json::from_value(serde_json::json!({"kind": "crouch"})).unwrap();
        assert_eq!(p.kind, "crouch");
        assert!(p.spots.is_none() && !p.dry_run);
        let p: VerifyParams = serde_json::from_value(serde_json::json!({
            "kind": "drop", "spots": [0, 2], "heights": [2.43], "prime": "none", "trace": 1
        }))
        .unwrap();
        assert_eq!(
            (p.spots, p.heights, p.trace),
            (Some(vec![0, 2]), Some(vec![2.43]), Some(1))
        );
        assert!(
            serde_json::from_value::<VerifyParams>(serde_json::json!({"kind": "drop", "x": 1}))
                .is_err()
        );
        let r = VerifyReport {
            state: "none".into(),
            ..VerifyReport::default()
        };
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("trials").is_none() && v.get("job").is_none());
        assert_eq!(serde_json::from_value::<VerifyReport>(v).unwrap(), r);
    }

    #[test]
    fn medal_test_takes_a_chain_or_bounced() {
        let p: MedalTestParams = serde_json::from_value(serde_json::json!({"chain": 5})).unwrap();
        assert_eq!((p.chain, p.bounced), (Some(5), false));
        let p: MedalTestParams =
            serde_json::from_value(serde_json::json!({"bounced": true})).unwrap();
        assert_eq!((p.chain, p.bounced), (None, true));
        assert!(
            serde_json::from_value::<MedalTestParams>(serde_json::json!({"chains": 5})).is_err()
        );
        assert!(method::ALL.contains(&MEDAL_TEST));
        let r = MedalTest {
            player: 0,
            chain: 2,
            medals: vec!["superbounce".into(), "double-bounce".into()],
            line: Some("\u{e2a1} Double bounce!".into()),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["medals"][1], "double-bounce");
    }
}
