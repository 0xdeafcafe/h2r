//! FrameScene pass graph diagnostics shared by the renderer and agent clients.

mod e3;
pub use e3::{E3Pass, E3PassName, E3Report};

use serde::{Deserialize, Serialize};

use crate::CaptureLighting;

/// Which recorder to arm or read with `render.frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Display,
    Private,
}

/// A graph pass can span slots when their attachments stay bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassSlot {
    pub view: u8,
    pub slot: String,
}

/// A completed graph recording. Counts describe the graph's attachment runs
/// and commands, independent of backend pass merging or GPU timestamps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameGraphStats {
    pub map_epoch: u64,
    pub frame: u64,
    pub tick: u32,
    pub present: u64,
    pub scene_hash: String,
    pub lighting: CaptureLighting,
    /// Work submitted by the four E3 lighting stages on this graph draw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e3: Option<E3Report>,
    pub passes: usize,
    pub draws: u64,
    pub clears: u64,
    pub copies: u64,
    /// GfxBackend deltas for the draw, including the cost of requested snapshots.
    pub render_passes: u64,
    pub backend_draws: u64,
    pub submits: u64,
}

/// Backend counters for the most recent native presentation. Available
/// without arming the recorder or walking its per-draw state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayStats {
    pub map_epoch: u64,
    pub frame: u64,
    pub tick: u32,
    pub present: u64,
    pub lighting: CaptureLighting,
    /// Work submitted by the four E3 lighting stages on this graph draw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e3: Option<E3Report>,
    pub render_passes: u64,
    pub backend_draws: u64,
    pub submits: u64,
}
