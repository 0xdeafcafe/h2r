//! `bots.bounce_to`, `bots.bounce_status`: send a brain-driven bot
//! somewhere by way of a superbounce (docs/runtime/agent-api.md, "Bounce
//! routes"; the plan is `bots::bounce_route`). Wire types only.

use serde::{Deserialize, Serialize};

/// `bots.bounce_to`: ask a bot for a bounce route to a point. An act.
pub const BOUNCE_TO: &str = "bots.bounce_to";
/// `bots.bounce_status`: where a bot's bounce route stands. A read.
pub const BOUNCE_STATUS: &str = "bots.bounce_status";

/// The methods above.
pub const ALL: &[&str] = &[BOUNCE_TO, BOUNCE_STATUS];

/// Which bot: its `handle` from `bots.stats`, its gamertag, or its
/// `player.<n>` id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BotRef {
    Handle(u32),
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BounceToParams {
    pub bot: BotRef,
    /// Where to end up, world units.
    pub to: [f32; 3],
    /// `help` (default): a bounce only where it gets the bot there
    /// when walking cannot, or much sooner; else the plain walk. `fun`:
    /// the highest launch within reach, then on to `to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BounceStatusParams {
    pub bot: BotRef,
}

/// The result of `bots.bounce_to`: the route is planned at the bot's next
/// sample, so read `bots.bounce_status` for the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BounceAccepted {
    pub bot: String,
    pub handle: u32,
    pub mode: String,
    pub to: [f32; 3],
}

/// The result of `bots.bounce_status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BounceStatus {
    pub bot: String,
    pub handle: u32,
    /// `none`, `queued`, `running`, `done` or `failed`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// `helps`, `fun`, or `plain` (no bounce helped: the plain walk).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The plan's steps: `walk_crouched`, `prime`, `walk`, `bounce`,
    /// `drop`, `fly`, `nav`.
    pub steps: Vec<String>,
    /// The step it is on (an index into `steps`; for a failed route, the
    /// step it failed on).
    pub step: usize,
    /// The step's phase while running: a prime's `start`, `in`, `release`,
    /// `out` (or `crouch`, `stand` in place); a drop's `line_up`,
    /// `run_off`, `air`; a bounce's `settle`, `air`; a walk's `walking`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phase: String,
    /// The step's try, from 1 (0 for a step that does not retry).
    #[serde(default)]
    pub tries: u32,
    /// The last touchdown during a bounce or a drop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<BounceLanding>,
    /// What the route did, oldest first (at most 64 lines).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<BounceLog>,
    /// The level pair word is at -3: primed and not yet bounced.
    pub primed: bool,
    /// Whether `primed` was read from the game or assumed after the
    /// crouch and the stand-up.
    pub primed_read: bool,
    /// Why a failed route failed: `timeout`, `no_map`, `no_path`,
    /// `goal_off_mesh`, `path_lost`, `prime_failed`, `pair_lost`,
    /// `no_bounce`, `no_drop` (walked off the launch floor and did not
    /// fall), `stalled`, `state` (an internal fault), `enemy` (a
    /// show-off), `no_route` (a show-off or a practice with no route),
    /// `practice_off` (the practice stopped), `died` or `map_changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The bot's bounce practice while the superbounce mode is on: it
    /// fights no one and practises bounces instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub practice: Option<BouncePractice>,
}

/// A bot's bounce practice so far.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BouncePractice {
    /// `idle` (between tries), `route` (on one) or `waiting` (no site
    /// within reach).
    pub state: String,
    /// The site being tried: its stand point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<[f32; 3]>,
    pub attempts: u32,
    pub bounces: u32,
    /// By site, in the order first tried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spots: Vec<PracticeSpot>,
}

/// One site's practice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PracticeSpot {
    pub stand: [f32; 3],
    pub attempts: u32,
    pub bounces: u32,
    /// Why the other tries did not bounce, by count: a route's error, or
    /// the landing's result for one that landed without launching.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub why: std::collections::BTreeMap<String, u32>,
}

/// A touchdown by the route's strip, as the bot saw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BounceLanding {
    pub tick: u32,
    /// The origin on the tick before touchdown.
    pub at: [f32; 3],
    /// Down, world units a second.
    pub speed_in: f32,
    /// Up, the best of the touchdown tick and the two after.
    pub vz_out: f32,
    /// Sideways from the site's target, world units.
    pub miss: f32,
    /// `bounce`, `not_primed`, `missed`, `too_slow` or `no_bounce`, as the
    /// game's landing classifier names them.
    pub result: String,
}

/// One line of a route's log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BounceLog {
    pub tick: u32,
    /// The step's name.
    pub step: String,
    /// `start`, `done`, `retry` (another drop), `reprime` (the word was
    /// lost on the way), a landing's result, or why the step failed.
    pub what: String,
    pub at: [f32; 3],
}
