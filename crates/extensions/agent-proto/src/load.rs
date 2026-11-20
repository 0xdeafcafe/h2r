//! `load.timeline`: level loads as the agent saw them
//! (docs/runtime/agent-api.md, phase 5).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::At;

/// `load.timeline`: the last loads, newest last.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadParams {
    /// At most this many (default 8, at most [`LoadParams::KEEP`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<usize>,
}

impl LoadParams {
    pub const DEFAULT: usize = 8;
    /// Loads kept.
    pub const KEEP: usize = 32;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Loads {
    pub at: At,
    pub loads: Vec<LoadRecord>,
    /// A stall going on now (no tick for 500 ms), so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<LoadRecord>,
    /// Frame stats were off: segments have times but no counts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_frame_stats: bool,
}

/// One load: the game stopped ticking (or changed map or BSP), then ticked
/// again and settled.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadRecord {
    /// `boot` (start to first tick), `map` (the map changed), `bsp` (the
    /// BSP changed), or `stall` (no tick for 500 ms, same map and BSP).
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_map: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_bsp: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bsp: Option<u32>,
    /// The present count when the stall ended (the first tick after it).
    pub present: u64,
    /// From the last tick before to the first after; then the settle.
    pub stall_ms: f64,
    pub settle_ms: f64,
    /// It reached [`LoadRecord::SETTLE_TICKS`] ticks without another load.
    pub settled: bool,
    /// `stall`, then `settle` (the first ticks after it), each with what
    /// the frames in it counted.
    pub segments: Vec<LoadSegment>,
}

impl LoadRecord {
    /// Ticks after the stall that count as settling in.
    pub const SETTLE_TICKS: u64 = 30;
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadSegment {
    pub name: String,
    pub ms: f64,
    /// Frames presented in it.
    pub frames: u64,
    /// Each frame-stats phase it spent time in, ms (`cpu`, `gpu-wait`, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub phases_ms: BTreeMap<String, f64>,
    /// Each counter it counted (`read_bytes`, `uploads`, `shaders`,
    /// `pipelines`, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<String, u64>,
}
