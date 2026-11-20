//! The work attributable to native E3 lighting on one completed draw.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum E3PassName {
    #[serde(rename = "e3.tiles")]
    Tiles,
    #[serde(rename = "e3.sun")]
    Sun,
    #[serde(rename = "e3.marks")]
    Marks,
    #[serde(rename = "e3.lights")]
    Lights,
}

/// A logical lighting pass. Empty passes do not allocate a timestamp
/// query or occupy a physical pass number for a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct E3Pass {
    pub name: E3PassName,
    pub draws: u32,
    /// Measured GPU interval in nanoseconds, null when unmeasured.
    pub gpu_ns: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct E3Report {
    pub passes: [E3Pass; 4],
    pub lights_lit: u32,
    pub lights_shadowed: u32,
    pub lights_over_cap: u32,
    pub sun_tiles: u32,
    pub sun_tiles_redrawn: u32,
    pub atlas_halved: u32,
}
