//! `anim.*`: the native animation checks' counters (docs/runtime/agent-api.md,
//! "anim"; the graph walk is docs/engine/skeleton-walk.md). Wire types only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Method names. Game methods: `session.methods` lists them where the game
/// registered them (`agent::anim::install`).
pub mod method {
    /// The graph walk's counters since start (no params → [`super::GraphWalk`]).
    pub const GRAPH_WALK: &str = "anim.graph_walk";

    /// Manager and graph segment counters since start (no params → [`super::Manager`]).
    pub const MANAGER: &str = "anim.manager";

    pub const ALL: &[&str] = &[GRAPH_WALK, MANAGER];
}

/// `anim.graph_walk`: the animation graph's node walk `1dd1c0`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphWalk {
    /// The guest entry, 8 hex digits.
    pub addr: String,
    /// The body: calls answered in Rust without the guest.
    pub body: Body,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub native: u64,
    /// Nodes it posed.
    pub nodes: u64,
    /// Calls the plan declined, left to the guest, by reason (`float`,
    /// `fault`, `count`, `queue`, `alias`, `parent`).
    pub declined: BTreeMap<String, u64>,
}

/// `anim.manager`: segment outcomes keyed by the function entry (8 hex digits).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manager {
    pub entries: BTreeMap<String, ManagerEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ManagerEntry {
    pub matched: u64,
    pub differed: u64,
    /// The Rust arm declined before the guest ran.
    pub declined: u64,
    /// The guest exited before reaching the predicted boundary.
    pub interrupted: u64,
    /// Segments the Rust body answered without the guest.
    pub answered: u64,
    pub first: Option<SegmentMismatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentMismatch {
    /// The entry or return selector at which this segment started (8 hex digits).
    pub pc: String,
    /// The differing transfer, register or memory byte.
    pub what: String,
    pub plan: String,
    pub guest: String,
}
