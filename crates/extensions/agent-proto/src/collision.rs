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
    /// One per monitored entry in contract order.
    pub entries: Vec<EntryStats>,
    /// Decode attempts since start and the number that failed.
    pub decodes: u64,
    pub decode_failures: u64,
    /// Times the world was dropped (map load, structure switch, snapshot
    /// load).
    pub flushes: u64,
    /// Decoded BSPs held, and charged map storage including index and BSP owners.
    pub bsps: u64,
    pub bytes: u64,
    /// Decoded MOPP programs held (entry F).
    #[serde(default)]
    pub mopps: u64,
    /// Actual capacities of BSP arrays, the packed cluster table and the
    /// MOPP trees.
    #[serde(default)]
    pub array_bytes: u64,
    #[serde(default)]
    pub budget_bytes: u64,
    /// Times the active map switched to scalar ring reads.
    #[serde(default)]
    pub ring_fallbacks: u64,
    /// Location queries using the live cluster word for lack of space.
    #[serde(default)]
    pub cluster_budget_reads: u64,
    /// The x87 precision control seen at the vector-test entries.
    pub precision: Precision,
    /// Entry I's parity arm: the native ray walk against the guest's.
    #[serde(default)]
    pub ray: RayParity,
    #[serde(default)]
    pub vector: VectorParity,
    #[serde(default)]
    pub context: VectorParity,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorParity {
    pub matched: u64,
    pub mismatched: u64,
    pub unverified: BTreeMap<String, u64>,
    pub first_mismatch: Option<String>,
}

/// Queries of the general MOPP ray (entry I) the native walk was compared
/// on. The guest's result is the answer in every case.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RayParity {
    /// Every interpreter call (and every key, where seen) matched.
    pub matched: u64,
    pub mismatched: u64,
    /// Queries not compared to the end, by reason (the bail names of
    /// [`EntryStats::bails`]).
    pub unverified: BTreeMap<String, u64>,
    /// Queries whose fraction moved at a terminal the arm could not see.
    pub fraction_moved: u64,
    /// The first mismatch: the observation and both sides.
    pub first_mismatch: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntryStats {
    /// Entry name. Vector and instance trace rows count only float environments.
    pub name: String,
    /// The guest entry, 8 hex digits.
    pub addr: String,
    /// Calls the native kernel answered.
    pub native: u64,
    /// Calls left to the lifted code, by reason (`env`, `float`, `fault`,
    /// `data`, `depth`, `alias`, `trap`, `budget`, `grow`).
    pub bails: BTreeMap<String, u64>,
    /// Complete guest control words at entry, as zero-padded lowercase
    /// hexadecimal keys. Absent keys have count zero.
    #[serde(default)]
    pub mxcsr: BTreeMap<String, u64>,
    #[serde(default)]
    pub x87_control: BTreeMap<String, u64>,
}

/// Calls seen with each x87 precision control.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Precision {
    pub single: u64,
    pub reserved: u64,
    pub double: u64,
    pub extended: u64,
}
