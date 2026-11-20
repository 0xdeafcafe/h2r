//! FrameScene pass graph diagnostics shared by the renderer and agent clients.

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

/// A completed private draw. Counts describe the graph's attachment runs
/// and commands, independent of backend pass merging or GPU timestamps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateFrameStats {
    pub map_epoch: u64,
    pub frame: u64,
    pub tick: u32,
    pub present: u64,
    pub scene_hash: String,
    pub lighting: CaptureLighting,
    pub passes: usize,
    pub draws: u64,
    pub clears: u64,
    pub copies: u64,
    /// GfxBackend deltas for the draw, including the cost of requested snapshots.
    pub render_passes: u64,
    pub backend_draws: u64,
    pub submits: u64,
}
