//! `probe.*`: tick-boundary behaviour measurements (docs/runtime/agent-api.md,
//! "probe"; docs/halo1/parity.md).
//! Wire types only. Game methods: `session.methods` lists them where the
//! game registered them (`game::agent_glue`).

use serde::{Deserialize, Serialize};

/// Method names.
pub mod method {
    /// Capture a held weapon's barrel fires over a scripted pad
    /// ([`super::Weapon`]).
    pub const WEAPON: &str = "probe.weapon";

    /// Ask the retail animation selector for a biped's graph
    /// ([`super::Animation`]).
    pub const ANIMATION: &str = "probe.animation";

    /// Launch a projectile at a live target and capture its impact and
    /// damage ([`super::Hit`]).
    pub const HIT: &str = "probe.hit";

    /// Who has had control and who has acted since the round began
    /// (no params).
    pub const ROUND: &str = "probe.round";

    pub const ALL: &[&str] = &[WEAPON, ANIMATION, HIT, ROUND];
}

/// `probe.weapon`'s params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weapon {
    /// The player's index.
    pub player: u32,
    /// The held weapon's object datum, eight hex digits.
    pub object: String,
    /// Ticks to capture, 1 to 290.
    pub ticks: u64,
    /// The pad script, in whole ticks, no longer than `ticks`.
    pub steps: Vec<crate::PadStep>,
}

/// `probe.hit`'s params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hit {
    /// The projectile tag, a datum or name.
    pub projectile: String,
    /// The target biped or vehicle's object datum, eight hex digits.
    pub target: String,
    /// A node of the target the two points are relative to; world space
    /// when absent.
    #[serde(default)]
    pub node: Option<u16>,
    /// Sets the target's shield, 0 to 1, before the launch.
    #[serde(default)]
    pub shield: Option<f32>,
    pub from: [f32; 3],
    pub to: [f32; 3],
    /// Ticks to wait for the impact, 1 to 290.
    pub ticks: u64,
}

/// `probe.animation`'s params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Animation {
    /// The biped's object datum, eight hex digits.
    pub unit: String,
    /// The mode, class, type and action string names.
    pub path: [String; 4],
    /// Use the overlay lookup rather than the base one.
    #[serde(default)]
    pub overlay: bool,
}
