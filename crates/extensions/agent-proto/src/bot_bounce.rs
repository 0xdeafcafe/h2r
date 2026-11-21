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
    /// `fly`, `nav`.
    pub steps: Vec<String>,
    /// The step it is on (an index into `steps`).
    pub step: usize,
    /// The level pair word is at -3: primed and not yet bounced.
    pub primed: bool,
    /// Whether `primed` was read from the game or assumed after the
    /// crouch and the stand-up.
    pub primed_read: bool,
    /// Why a failed route failed: `timeout`, `no_map`, `no_path`,
    /// `path_lost`, `prime_failed`, `pair_lost`, `no_bounce`, `enemy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
