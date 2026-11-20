//! `collision.*`: the native collision world's counters
//! (docs/runtime/agent-api.md, "collision"; the entries are
//! docs/engine/collision-queries.md). Wire types only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Method names. Game methods: `session.methods` lists them where the game
/// registered them (`agent::collision::install`).
pub mod method {
    /// The world's counters since start (no params → [`super::Stats`]).
    pub const STATS: &str = "collision.stats";

    pub const ALL: &[&str] = &[STATS];
}

/// `collision.stats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// The switch the entries are installed under.
    pub switch: String,
    /// One per taken-over entry, in contract order (A, B, C, D, ...).
    pub entries: Vec<EntryStats>,
    /// BSPs decoded since start, and those left to the guest.
    pub decodes: u64,
    pub decode_failures: u64,
    /// Times the world was dropped (map load, structure switch, snapshot
    /// load).
    pub flushes: u64,
    /// Decoded BSPs held now, and their bytes.
    pub bsps: u64,
    pub bytes: u64,
    /// The x87 precision control seen at the vector-test entries.
    pub precision: Precision,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntryStats {
    /// `point_to_leaf`, `location`, `vector_test`, `vector_context`.
    pub name: String,
    /// The guest entry, 8 hex digits.
    pub addr: String,
    /// Calls the native kernel answered.
    pub native: u64,
    /// Calls left to the lifted code, by reason (`env`, `float`, `fault`,
    /// `data`, `depth`, `alias`).
    pub bails: BTreeMap<String, u64>,
}

/// Calls seen with each x87 precision control.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Precision {
    pub single: u64,
    pub reserved: u64,
    pub double: u64,
    pub extended: u64,
}
