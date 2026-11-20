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
    /// Vertex shader constant registers (D3D numbering, -96..=95) to
    /// record on each listed draw: at most [`FrameParams::MAX_CONSTANTS`],
    /// and only with `draws`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constants: Vec<i32>,
}

impl FrameParams {
    pub const DEFAULT_DRAWS: usize = 2000;
    pub const MAX_DRAWS: usize = 20_000;
    pub const MAX_SNAPS: usize = 4;
    pub const MAX_CONSTANTS: usize = 32;
    /// The constant registers, D3D numbering (hardware register = this + 96).
    pub const CONSTANT_REGISTERS: std::ops::RangeInclusive<i32> = -96..=95;
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
    /// The vertex side: `fvf 0x…` (fixed function) or `prog @start
    /// n=slots #hash` (a vertex program; the hash is of its words, so one
    /// program reads the same in every draw and frame).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vs: String,
    /// How the draw tests and stores depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<DepthRecord>,
    /// The constant registers asked for (`FrameParams::constants`), by
    /// D3D register number, as the draw saw them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub constants: BTreeMap<i32, [f32; 4]>,
}

/// A draw's depth state: the render states, the viewport's depth range
/// and the NV2A registers that map the vertex's z or w into the stored
/// depth.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DepthRecord {
    /// `D3DRS_ZENABLE`: `off`, `z` or `w` (`D3DZB_USEW`).
    pub mode: String,
    /// Whether the draw is W-buffered as drawn: `SET_CONTROL0`'s
    /// z-perspective bit once pushed, else `mode` is `w`. The stored depth
    /// is then the vertex's w, clipped (or clamped) to `clip`, over `full`.
    pub w: bool,
    /// `D3DRS_ZFUNC` (`Less`, `LessEqual`, `Always`, ...).
    pub func: String,
    /// `D3DRS_ZWRITEENABLE`.
    pub write: bool,
    /// The D3D viewport's `[MinZ, MaxZ]`.
    pub viewport: [f32; 2],
    /// `SET_VIEWPORT_SCALE` z and `SET_VIEWPORT_OFFSET` z.
    pub scale: [f32; 2],
    /// `[SET_CLIP_MIN, SET_CLIP_MAX]` as pushed (depth units).
    pub clip: [f32; 2],
    /// The depth target's full scale (16777215 for D24S8), the stored
    /// depth's 1.0; 0 with no depth target.
    pub full: f32,
    /// `D3DRS_DEPTHCLIPCONTROL` clamps out-of-range depth instead of
    /// culling it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clamp: bool,
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
    /// The pipelined render (`impl.render.pipelined_frame`): frame counts,
    /// serial and fallback reasons, joins, snapshot coverage and times in ms.
    pub pipeline: Value,
    /// The frame-stats window's per-frame means of the render counters.
    pub counters: Value,
}

/// `render.compare`: two PNG files compared pixel by pixel
/// (docs/graphics/framescene.md, section 9.2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareParams {
    pub a: String,
    pub b: String,
    /// Where to write the difference image (PNG), if wanted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

/// `render.compare`'s answer (and `render.parity`'s, with its tick).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompareReply {
    /// `[width, height]`, the same for both.
    pub size: [u32; 2],
    /// Pixels with any channel off by more than
    /// [`CompareReply::NOISE`].
    pub differing: u64,
    /// The largest difference of any one channel.
    pub max_channel: u8,
    /// The mean absolute difference over every channel of every pixel.
    pub mean: f64,
    /// The first differing pixel in row order, `[x, y]`; `null` when none.
    pub first: Option<[u32; 2]>,
    /// Within the parity bar: `max_channel <= 8` and `differing` at most
    /// 0.01% of the pixels.
    pub within_bar: bool,
    /// The difference image, when one was asked for and written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

impl CompareReply {
    /// A channel off by this much or less does not make a pixel differ.
    pub const NOISE: u8 = 2;
    /// The bar's largest channel difference.
    pub const BAR_MAX_CHANNEL: u8 = 8;
    /// The bar's share of differing pixels: one in this many (0.01%).
    pub const BAR_DIFFERING_ONE_IN: u64 = 10_000;

    /// Whether `max_channel` and `differing` over `pixels` meet the bar.
    pub fn meets_bar(max_channel: u8, differing: u64, pixels: u64) -> bool {
        max_channel <= Self::BAR_MAX_CHANNEL
            && u128::from(differing) * u128::from(Self::BAR_DIFFERING_ONE_IN)
                <= u128::from(pixels)
    }
}

/// `render.capture`: the frame of a sim tick as a PNG.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameCaptureParams {
    /// The game tick (as `at.tick` counts) whose frame is wanted; default
    /// the next one. At most [`FrameCaptureParams::MAX_AHEAD`] ticks ahead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    /// A pass of that frame (as `render.frame` numbers them) read back
    /// instead of the presented image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<usize>,
}

impl FrameCaptureParams {
    /// How far ahead a tick may be asked for: the answer must come within
    /// the agent's deferred-answer limit (300 ticks).
    pub const MAX_AHEAD: u64 = 240;
}

/// `render.capture`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CaptureReply {
    /// The PNG.
    pub path: String,
    /// The last tick that ended before the frame's present: the tick asked
    /// for, or a later one when the game ran two ticks in one frame.
    pub tick: u64,
    /// The present that showed it (as `frame-N.png` and `render.frame`
    /// number presents).
    pub present: u64,
    /// The pass read back, when one was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<usize>,
    /// The sealed scene's summary hash at that present (`render.scene`);
    /// `null` with no scene source.
    pub scene_hash: Option<String>,
}

/// `render.scene`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneParams {
    /// Also write the whole scene as JSON under the cache directory.
    #[serde(default)]
    pub dump: bool,
}

/// `render.scene`'s answer: the last sealed `FrameScene`
/// (docs/graphics/framescene.md, section 4).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneReply {
    /// False until a scene has been sealed.
    pub sealed: bool,
    pub map_epoch: u32,
    pub frame: u64,
    pub tick: u32,
    /// The scene's summary hash, hex.
    pub hash: String,
    /// Why this scene is a cut (`map`, `bsp`, `revert`, `load`, `views`); empty when none.
    pub cut: Vec<String>,
    pub views: Vec<SceneViewRecord>,
    /// scene_dropped_*, scene_unresident, scene_readback, scene_held_draws, scenes_sealed, ...
    pub counters: BTreeMap<String, u64>,
    /// The entry census (`frame-scene.census`; framescene.md, section 8.2):
    /// per guest entry slices 4 and 5 replace (`layer.scene.3`,
    /// `layer.light.12`, `layer.scene.16`, `light_passes`,
    /// `object_shadow`, `screen_effects`, `colour_pass`, `screen_effect`,
    /// ...), what the guest still does under it since the start. `None`
    /// when the census is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<SceneEntries>,
    /// The last frame's carried part (`frame-scene.sink`; framescene.md,
    /// slice 6): the guest's D3D work by slot, the side targets a game
    /// view names and the readbacks. `None` when the sink is off or has
    /// recorded no frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carried: Option<SceneCarried>,
    /// The JSON file written when `dump` was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dump: Option<String>,
}

/// `render.scene`'s entry census.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneEntries {
    /// By entry name.
    pub by_entry: BTreeMap<String, SceneEntryRecord>,
    /// Entries whose exit never ran (dropped unended) and exits with no
    /// entry open.
    pub unbalanced: u64,
    /// Whether `d3d_draws` counts: a D3D draw counter is registered.
    pub draws_counted: bool,
}

/// One guest entry's totals in the entry census.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneEntryRecord {
    pub calls: u64,
    /// Guest draws handed to the D3D backend under the entry: what its
    /// replacement would have to draw (8.2 rule 1 needs zero).
    pub d3d_draws: u64,
    /// Steps of the shared random seeds `+0` and `+4` under the entry.
    pub seed_steps: u64,
    pub effects_seed_steps: u64,
    /// Calls after which a seed was not a run of steps from its value at
    /// the entry.
    pub seed_unmatched: u64,
    /// Calls of `animation_resource_get` and of the particle systems'
    /// per-view update under the entry.
    pub animation_gets: u64,
    pub particle_updates: u64,
}

/// `render.scene`'s carried part: one frame's guest D3D work as the
/// device's scene sink filed it (framescene.md, slice 6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneCarried {
    /// Ops by slot (`opaque`, `after_opaque`, `after_lights`,
    /// `transparents`, `after_post`, `overlay`, `outside`); slots with
    /// none are left out.
    pub slots: BTreeMap<String, SceneOpCounts>,
    /// Surfaces other than the scene's colour and depth that a game view's
    /// ops name (section 8.3's census).
    pub side_targets: Vec<SceneSideTarget>,
    /// Readbacks in a game view, and outside one, by kind (`surface`,
    /// `depth`, `mirror`, `pixel_count`); kinds with none are left out.
    pub readbacks: BTreeMap<String, u32>,
    pub readbacks_outside: BTreeMap<String, u32>,
}

/// Carried ops of each kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneOpCounts {
    pub draws: u32,
    /// Attachment changes (and the first ops of each slot and view).
    pub binds: u32,
    pub clears: u32,
    /// Whole-surface copies.
    pub copies: u32,
    /// Guest texels written into a target.
    pub uploads: u32,
    /// Targets sampled: framebuffer snapshots, render-to-texture levels in
    /// place, depth bits.
    pub samples: u32,
}

/// A side target named in a game view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSideTarget {
    /// Its number in the frame (`TargetRef::Aux`).
    pub aux: u32,
    pub width: u32,
    pub height: u32,
    /// The guest's `D3DFORMAT` code.
    pub format: u32,
    pub depth: bool,
    /// The ops that named it, by slot.
    pub ops: BTreeMap<String, u32>,
}

/// One view of a sealed scene.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneViewRecord {
    /// `game:0:0` (kind:player:view).
    pub key: String,
    pub cut: bool,
    pub clusters: u32,
    pub instances: u32,
    pub parts: u32,
    pub rows: u32,
    pub lights: u32,
    /// Lights that cast their shadow: the view's shadow maps.
    pub shadows: u32,
    /// Lights by each layer their passes draw (`"10"` to `"14"`); layers
    /// no light draws are left out.
    #[serde(default)]
    pub light_layers: BTreeMap<String, u32>,
    /// Items of each typed layer draw, by layer (`"1"`, `"5"`, ...);
    /// a layer drawn twice sums.
    #[serde(default)]
    pub layer_items: BTreeMap<String, u32>,
    /// Items of the typed layer draws the guest still draws (refused by
    /// extraction).
    #[serde(default)]
    pub layer_remainder: u32,
    /// Instances the guest still draws (refused by extraction).
    pub remainder: u32,
    /// Parts by how they draw beyond their material (`stencil_mark`,
    /// `strip`); flags no part has are left out.
    #[serde(default)]
    pub part_flags: BTreeMap<String, u32>,
    /// The view's carried ops by slot name (`frame-scene.sink`); empty
    /// without it.
    #[serde(default)]
    pub carried: BTreeMap<String, u32>,
    /// Carried draws by `CameraUse`; empty until the sink classifies them.
    #[serde(default)]
    pub camera_use: BTreeMap<String, u32>,
    /// Model decorator draws (one per placement) of the view's layer 3,
    /// their instances, and the draws the guest still draws (refused by
    /// extraction; docs/graphics/native-decorators.md).
    #[serde(default)]
    pub decorators: u32,
    #[serde(default)]
    pub decorator_instances: u32,
    #[serde(default)]
    pub decorator_remainder: u32,
    /// The view's fog, layer 16, when its draw ran (framescene.md,
    /// slice 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog: Option<SceneLateRecord>,
    /// The view's water edge blend, layer 6, when its draw ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water: Option<SceneLateRecord>,
    /// The view's post passes, when `render_screen_effects` ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<ScenePostRecord>,
}

/// A view's late layer (the fog's 16 or the water's 6) in a sealed scene.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneLateRecord {
    /// The layer's own gate held: it draws at all.
    pub draws: bool,
    /// Its items in the current list: what the guest would draw.
    pub items: u32,
    /// Materials the native stage draws with, their fog words resolved.
    pub materials: u32,
    /// Parts the native stage draws.
    pub parts: u32,
    /// The native stage took the layer (the guest skips its parts).
    pub drawn: bool,
    /// Items the guest still draws on the D3D path.
    pub kept: u32,
    /// Device fog words (of 7) that read at the layer.
    pub device_words: u32,
}

/// A view's post passes in a sealed scene.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePostRecord {
    /// `render_screen_effects`' two stack bytes.
    pub colour: bool,
    pub late: bool,
    pub view_kind: u32,
    /// The scenario's screen-effect entries, and whether the loop's gates
    /// read from memory pass.
    pub screen_effects: u32,
    pub screen_effects_gated: bool,
    /// The colour pass ran, with its arguments in the scene.
    pub colour_pass: bool,
    /// Screen effects drawn under the call.
    pub effects: u32,
}

/// `render.parity`: arm the in-process two-path comparison for one frame
/// (a diagnostic, deleted with the D3D backend).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParityParams {
    pub arm: bool,
}

/// `render.parity`'s answer: `render.compare`'s, plus the tick compared.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParityReply {
    /// The last tick that ended before the frame step that drew both paths.
    pub tick: Option<u64>,
    #[serde(flatten)]
    pub compare: CompareReply,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draw_without_depth_or_constants_prints_neither() {
        let d = DrawRecord::default();
        let v = serde_json::to_value(&d).unwrap();
        assert!(v.get("depth").is_none());
        assert!(v.get("constants").is_none());
        assert!(v.get("vs").is_none());
    }

    #[test]
    fn depth_and_constants_round_trip_with_register_keys() {
        let d = DrawRecord {
            vs: "prog @0 n=12 #0011aabb".into(),
            depth: Some(DepthRecord {
                mode: "w".into(),
                w: true,
                func: "LessEqual".into(),
                write: true,
                viewport: [0.0, 1.0],
                scale: [16777215.0, 0.0],
                clip: [0.0, 16777215.0],
                full: 16777215.0,
                clamp: false,
            }),
            constants: BTreeMap::from([
                (-38, [1.0, 0.0, 0.0, 0.0]),
                (58, [320.0, -240.0, 1.0, 1.0]),
            ]),
            ..DrawRecord::default()
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(
            v["constants"]["-38"],
            serde_json::json!([1.0, 0.0, 0.0, 0.0])
        );
        assert_eq!(v["depth"]["func"], "LessEqual");
        assert!(v["depth"].get("clamp").is_none(), "clamp only when on");
        let back: DrawRecord = serde_json::from_value(v).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn constants_arm_as_a_register_list() {
        let p: FrameParams = serde_json::from_value(serde_json::json!({
            "arm": true,
            "draws": true,
            "constants": [-96, 95],
        }))
        .unwrap();
        assert_eq!(p.constants, vec![-96, 95]);
    }

    #[test]
    fn the_bar_is_max_channel_8_and_one_pixel_in_ten_thousand() {
        assert!(CompareReply::meets_bar(8, 1, 10_000));
        assert!(!CompareReply::meets_bar(9, 0, 10_000));
        assert!(!CompareReply::meets_bar(8, 2, 10_000));
        assert!(CompareReply::meets_bar(0, 0, 0));
        assert!(!CompareReply::meets_bar(0, 1, 9_999));
    }

    #[test]
    fn a_comparison_prints_first_as_null_and_parity_adds_the_tick() {
        let c = CompareReply {
            size: [4, 2],
            within_bar: true,
            ..Default::default()
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["first"], Value::Null);
        assert!(v.get("diff").is_none());
        let p = ParityReply {
            tick: Some(90),
            compare: c.clone(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["tick"], 90);
        assert_eq!(v["size"], serde_json::json!([4, 2]));
        assert_eq!(serde_json::from_value::<ParityReply>(v).unwrap(), p);
    }

    #[test]
    fn a_views_late_passes_print_only_when_they_ran() {
        let v = serde_json::to_value(SceneViewRecord::default()).unwrap();
        for absent in ["fog", "water", "post"] {
            assert!(v.get(absent).is_none(), "{absent}");
        }
        let r = SceneViewRecord {
            fog: Some(SceneLateRecord { draws: true, items: 9, materials: 2, parts: 7, drawn: true, kept: 2, device_words: 7 }),
            post: Some(ScenePostRecord { colour: true, colour_pass: true, effects: 1, ..Default::default() }),
            ..Default::default()
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["fog"]["kept"], 2);
        assert_eq!(v["post"]["effects"], 1);
        assert!(v.get("water").is_none());
        assert_eq!(serde_json::from_value::<SceneViewRecord>(v).unwrap(), r);
    }

    #[test]
    fn a_scenes_carried_part_prints_only_when_the_sink_recorded() {
        let v = serde_json::to_value(SceneReply::default()).unwrap();
        assert!(v.get("carried").is_none());
        let r = SceneReply {
            carried: Some(SceneCarried {
                slots: BTreeMap::from([("opaque".to_owned(), SceneOpCounts { draws: 3, binds: 1, ..Default::default() })]),
                side_targets: vec![SceneSideTarget {
                    aux: 0,
                    width: 256,
                    height: 256,
                    format: 6,
                    depth: false,
                    ops: BTreeMap::from([("after_lights".to_owned(), 2)]),
                }],
                readbacks: BTreeMap::from([("pixel_count".to_owned(), 4)]),
                readbacks_outside: BTreeMap::new(),
            }),
            ..Default::default()
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["carried"]["slots"]["opaque"]["draws"], 3);
        assert_eq!(v["carried"]["side_targets"][0]["ops"]["after_lights"], 2);
        assert_eq!(v["carried"]["readbacks"]["pixel_count"], 4);
        assert_eq!(serde_json::from_value::<SceneReply>(v).unwrap(), r);
    }

    #[test]
    fn params_take_their_defaults_and_refuse_strangers() {
        let s: SceneParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!s.dump);
        let c: FrameCaptureParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(c, FrameCaptureParams::default());
        assert!(serde_json::from_value::<FrameCaptureParams>(serde_json::json!({"tik": 3})).is_err());
        assert!(serde_json::from_value::<CompareParams>(serde_json::json!({"a": "x"})).is_err());
        let v = serde_json::to_value(CaptureReply::default()).unwrap();
        assert_eq!(v["scene_hash"], Value::Null, "null, not missing");
        assert!(v.get("pass").is_none());
    }
}
