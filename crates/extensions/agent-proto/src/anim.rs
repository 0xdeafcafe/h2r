//! `anim.*`: the native animation checks' counters (docs/runtime/agent-api.md,
//! "anim"; the graph walk is docs/engine/skeleton-walk.md). Wire types only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Method names. Game methods: `session.methods` lists them where the game
/// registered them (`agent::anim::install`).
pub mod method {
    /// The graph walk's counters since start (no params → [`super::GraphWalk`]).
    pub const GRAPH_WALK: &str = "anim.graph_walk";

    pub const ALL: &[&str] = &[GRAPH_WALK];
}

/// `anim.graph_walk`: the animation graph's node walk `1dd1c0`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphWalk {
    /// The switch the arm runs under.
    pub switch: String,
    /// The guest entry, 8 hex digits.
    pub addr: String,
    /// The arm: the guest answers, the Rust plan is compared with it.
    pub arm: Arm,
    /// The body: calls answered in Rust without the guest.
    pub body: Body,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    /// Calls planned at entry and compared at the return.
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    /// Plans dropped without a return to compare them with.
    pub dropped: u64,
    /// Calls the plan declined, by reason (`float`, `fault`, `count`,
    /// `queue`, `alias`, `parent`); the guest ran unchecked.
    pub declined: BTreeMap<String, u64>,
    /// The first mismatch since start.
    #[serde(default)]
    pub first: Option<Mismatch>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mismatch {
    /// The register or byte, by name: `eax`, `xmm3`, `matrix 2 +0x10`,
    /// `stack entry-0x3f8 (queue entry 1)`.
    pub what: String,
    /// The plan's value and the guest's, in hex.
    pub plan: String,
    pub guest: String,
    /// The general registers at the call's entry, 8 hex digits each.
    pub entry: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub native: u64,
    /// Nodes it posed.
    pub nodes: u64,
    /// Calls left to the guest, by reason as in [`Arm::declined`].
    pub declined: BTreeMap<String, u64>,
}
