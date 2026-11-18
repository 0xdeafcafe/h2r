//! `render.*`: one frame's passes, draws and targets as data, and the
//! render pipeline's counters (docs/runtime/agent-api.md, phase 5).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `render.frame`: record the next whole frame (`arm`), or read the last
/// one recorded.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameParams {
    /// Record the next whole frame: from the next present to the one after.
    /// Replaces an arming not yet done, and forgets the last frame.
    #[serde(default)]
    pub arm: bool,
    /// List every draw, not only each pass's totals.
    #[serde(default)]
    pub draws: bool,
    /// List at most this many draws (default 2000, at most 20000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_draws: Option<usize>,
    /// Passes (by index in the frame) whose colour target is read back as
    /// a PNG when the pass ends: at most [`FrameParams::MAX_SNAPS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snap: Vec<usize>,
}

impl FrameParams {
    pub const DEFAULT_DRAWS: usize = 2000;
    pub const MAX_DRAWS: usize = 20_000;
    pub const MAX_SNAPS: usize = 4;
}

/// `render.frame`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameReply {
    /// A frame is still to be recorded.
    pub armed: bool,
    /// The arming's number; the frame it records carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// Presents seen when it was armed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed_at_present: Option<u64>,
    /// The last frame recorded, until the next arming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<FrameRecord>,
}

/// One guest frame as the D3D layer handed it to the backend: everything
/// between two presents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameRecord {
    /// The arming that asked for it.
    pub seq: u64,
    /// The present that began it and the one that ended it.
    pub from_present: u64,
    pub present: u64,
    /// Wall time between the two presents, ms.
    pub ms: f64,
    pub totals: FrameTotals,
    /// Runs of work on one colour target (and depth), in order. The same
    /// target can come back as a later pass.
    pub passes: Vec<PassRecord>,
    /// Each draw, when asked for (`draws`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draws: Vec<DrawRecord>,
    /// More draws than `max_draws`: the list stops there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Calls into each native render path (`impl.render.*`) during the
    /// frame; a path not listed made none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub paths: BTreeMap<String, u64>,
    /// Surface copies and readbacks, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ops: Vec<OpRecord>,
    /// Pass images asked for (`snap`): where each went, or why not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snaps: Vec<SnapRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameTotals {
    pub passes: usize,
    pub draws: u64,
    /// Draws by outcome (`drawn`, or why not: `skipped …`, `error`).
    pub outcomes: BTreeMap<String, u64>,
    pub vertices: u64,
    pub clears: u64,
    pub copies: u64,
    pub reads: u64,
    /// Distinct colour targets drawn to or cleared.
    pub targets: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PassRecord {
    pub index: usize,
    /// The colour surface (the backend's handle, as text) and its size and
    /// format as the guest describes it (`640x480 X8R8G8B8`), once a draw
    /// says.
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub clears: u64,
    pub draws: u64,
    pub vertices: u64,
    /// The pass's first draw, by index in the frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_draw: Option<u64>,
    pub outcomes: BTreeMap<String, u64>,
    pub primitives: BTreeMap<String, u64>,
    /// The D3D entry that issued each draw.
    pub entries: BTreeMap<String, u64>,
    /// Texture formats sampled, per draw.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub textures: BTreeMap<String, u64>,
    /// Draws with a pixel program (the rest are fixed function).
    pub programs: u64,
    /// Draws with blending on.
    pub blended: u64,
    /// Draws that write depth.
    pub depth_writes: u64,
}

/// What the backend wrapper tells the recorder about one draw.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawRecord {
    pub index: u64,
    pub pass: usize,
    pub entry: String,
    pub primitive: String,
    pub vertices: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub indices: u32,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub textures: Vec<String>,
    pub program: bool,
    pub blend: bool,
    pub depth_write: bool,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// A surface copy or readback in the frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpRecord {
    /// `copy`, `read` or `read_depth`.
    pub op: String,
    pub surface: String,
    /// A copy's destination; a read's format and size.
    pub detail: String,
    /// Draws in the frame before it.
    pub after_draws: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapRecord {
    pub pass: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `render.stats`: the render pipeline's counters since the game started,
/// each `null` where that piece isn't running.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderStats {
    /// In-between frames (`impl.render.interpolate`, `.reproject`):
    /// `d3d::interp::InterpStats`, every field, including the hold reasons
    /// (`unsure_*`, `mismatch_*`, `dropped`).
    pub interp: Value,
    /// The pipelined render (`H2_PIPELINE_RENDER`): frames pipelined and
    /// completed, why frames ran serially, fallbacks by call, joins, ms.
    pub pipeline: Value,
    /// Calls into each native render path (`impl.render.*` placed as an
    /// implementation) since the start; 0 for a path that is off.
    pub paths: BTreeMap<String, u64>,
    /// The frame-stats window's per-frame means of the render counters.
    pub counters: Value,
}
