//! `render.*`: frame passes, draws, target layouts, and the
//! render pipeline's counters (docs/runtime/agent-api.md, phase 5).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `render.markers`: the game type markers the last present drew
/// (docs/runtime/agent-api.md, "render.markers").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkersParams {
    /// Only the markers of this kind (`spawn`, `hill`, `flag_stand`, ...);
    /// the counts still cover every marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Only the markers drawn in this form, `edit` or `play`
    /// (docs/game/gametype-marker-forms.md); the counts still cover every
    /// marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
}

/// One row of `render.markers`' `markers`: a marker the last present drew.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerRow {
    /// What it stands for: `<block>:<index>`, `added:<op>`,
    /// `problem:<label>`, `nav:<what>` or `zone:<hex>`.
    pub thing: String,
    /// `game_markers::Kind::name`, or `wall`.
    pub kind: String,
    /// 0 to 7, or none.
    pub team: Option<u8>,
    /// Where its label sits, in world units.
    pub anchor: [f32; 3],
    /// Its line count, the dimmed edit-only parts' included.
    pub lines: usize,
    /// A marker of another game type, drawn at a quarter.
    pub faded: bool,
    /// The form it drew: `edit` or `play`.
    pub form: String,
    /// The icon on its plate (`game_markers::Icon::name`); none for a wall
    /// and a volume.
    pub icon: Option<String>,
    /// How much its plate was scaled with the eye's distance, 1 out to 8
    /// world units; none when no plate drew.
    pub plate_scale: Option<f32>,
}

/// `render.spawn_view`: select a player starting location, or release the camera.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpawnViewParams {
    pub index: Option<u32>,
    pub yaw_offset: Option<f32>,
    pub reset: bool,
}

/// `render.view`: place the private world camera at an explicit eye, or
/// release it. Angles are radians; `fov` is the horizontal lens in degrees.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewParams {
    pub eye: Option<[f32; 3]>,
    /// Heading about +z, from +x towards +y.
    pub yaw: Option<f32>,
    /// Up is positive, within [`ViewParams::PITCH_LIMIT`].
    pub pitch: Option<f32>,
    /// Omit to keep the drawn view's own lens.
    pub fov: Option<f32>,
    pub reset: bool,
}

/// A checked [`ViewParams`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewRequest {
    Reset,
    /// `yaw` is wrapped into `0..TAU`.
    Pose {
        eye: [f32; 3],
        yaw: f32,
        pitch: f32,
        fov: Option<f32>,
    },
}

impl ViewParams {
    /// The private camera's pitch limit (the spectator camera's).
    pub const PITCH_LIMIT: f32 = 1.55;
    /// The horizontal lens range, degrees.
    pub const FOV_DEGREES: std::ops::RangeInclusive<f32> = 1.0..=150.0;

    /// `{reset: true}` alone, or an eye, yaw and pitch with an optional
    /// lens, all finite and in range.
    pub fn check(&self) -> Result<ViewRequest, String> {
        let posed =
            self.eye.is_some() || self.yaw.is_some() || self.pitch.is_some() || self.fov.is_some();
        if self.reset {
            return if posed {
                Err("reset takes no other params".into())
            } else {
                Ok(ViewRequest::Reset)
            };
        }
        let (Some(eye), Some(yaw), Some(pitch)) = (self.eye, self.yaw, self.pitch) else {
            return Err(
                "supply eye: [x, y, z], yaw and pitch (fov optional), or reset: true".into(),
            );
        };
        if !eye.iter().all(|v| v.is_finite()) {
            return Err("eye must be three finite numbers".into());
        }
        if !yaw.is_finite() {
            return Err("yaw must be finite".into());
        }
        if !pitch.is_finite() || pitch.abs() > Self::PITCH_LIMIT {
            return Err(format!(
                "pitch must be within -{0}..={0} radians",
                Self::PITCH_LIMIT
            ));
        }
        if let Some(fov) = self.fov
            && !(fov.is_finite() && Self::FOV_DEGREES.contains(&fov))
        {
            return Err(format!(
                "fov must be {}..={} horizontal degrees",
                Self::FOV_DEGREES.start(),
                Self::FOV_DEGREES.end()
            ));
        }
        Ok(ViewRequest::Pose {
            eye,
            yaw: yaw.rem_euclid(std::f32::consts::TAU),
            pitch,
            fov: self.fov,
        })
    }
}

/// `render.view.current`: read the pose of the view the game draws for a
/// local player.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CurrentViewParams {
    /// The local player's seat, `0..4`; omit for seat 0.
    pub seat: Option<u8>,
}

impl CurrentViewParams {
    /// The local seats a view record can belong to.
    pub const SEATS: u8 = 4;

    /// The seat asked for, within [`CurrentViewParams::SEATS`].
    pub fn check(&self) -> Result<u8, String> {
        let seat = self.seat.unwrap_or(0);
        if seat < Self::SEATS {
            Ok(seat)
        } else {
            Err(format!("seat must be 0..{}", Self::SEATS))
        }
    }
}

/// `render.view.current`'s reply: a pose in the conventions of
/// [`ViewParams`], so `render.view` takes its `eye`, `yaw`, `pitch` and
/// `fov` as they are.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurrentView {
    pub eye: [f32; 3],
    /// Heading about +z, from +x towards +y, in `0..TAU`.
    pub yaw: f32,
    /// Up is positive, within [`ViewParams::PITCH_LIMIT`].
    pub pitch: f32,
    /// The horizontal lens, degrees.
    pub fov: f32,
    /// The pose is the private `render.view` or `render.spawn_view` camera.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
}

impl CurrentView {
    /// The pose of a camera at `eye` looking along `forward` (any length),
    /// with a horizontal lens of `fov` degrees. `None` for a non-finite
    /// input or a zero-length `forward`.
    pub fn looking(eye: [f32; 3], forward: [f32; 3], fov: f32) -> Option<Self> {
        let [x, y, z] = forward;
        let length = (x * x + y * y + z * z).sqrt();
        if !eye.into_iter().chain(forward).chain([fov, length]).all(f32::is_finite)
            || length < 1e-4
        {
            return None;
        }
        let tau = std::f32::consts::TAU;
        let mut yaw = y.atan2(x).rem_euclid(tau);
        if yaw >= tau {
            // A tiny negative angle rounds up to TAU: the same heading as 0.
            yaw = 0.0;
        }
        Some(Self {
            eye,
            yaw,
            pitch: (z / length)
                .clamp(-1.0, 1.0)
                .asin()
                .clamp(-ViewParams::PITCH_LIMIT, ViewParams::PITCH_LIMIT),
            fov,
            private: false,
        })
    }

    /// The `render.view` params that draw this pose.
    pub fn view_params(&self) -> ViewParams {
        ViewParams {
            eye: Some(self.eye),
            yaw: Some(self.yaw),
            pitch: Some(self.pitch),
            fov: Some(self.fov),
            reset: false,
        }
    }
}

/// `render.frame` and `render.frame_layout`: record the next whole frame
/// (`arm`), or read the last one recorded.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameParams {
    /// Omit for the displayed frame. A private graph runs on an explicit lighting capture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::frame_graph::Source>,
    /// Record the next whole frame: from the next present to the one after.
    /// Replaces an arming not yet done, and forgets the last frame.
    #[serde(default)]
    pub arm: bool,
    /// List each draw. `render.frame_layout` always lists draws.
    #[serde(default)]
    pub draws: bool,
    /// List at most this many draws (default 2000, at most 20000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_draws: Option<usize>,
    /// Passes (by index in the frame) whose colour target is read back as
    /// a PNG when the pass ends: at most [`FrameParams::MAX_SNAPS`].
    /// `render.frame_layout` requests the display copy.
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
    /// Counters from an actual native graph presented to the display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_frame: Option<crate::frame_graph::FrameGraphStats>,
    /// Identifies the sealed scene and the draw mode for a private-target recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_frame: Option<crate::frame_graph::FrameGraphStats>,
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
    /// Named typed stages, in execution order within the attachment run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<String>,
    /// Carried slots in execution order on this pair of private graph attachments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<crate::frame_graph::PassSlot>,
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
    /// Display geometry requested with `render.frame_layout`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<DrawLayout>,
}

/// The guest and display geometry of a draw.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawLayout {
    pub guest_size: [u32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_size: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_depth_size: Option<[u32; 2]>,
    /// Guest viewport and writable rectangles as x, y, width, height.
    pub viewport: [u32; 4],
    pub clip: Vec<[u32; 4]>,
    /// Writable display rectangles, in display raster pixels when known.
    pub display_clip: Vec<[u32; 4]>,
    /// Horizontal clip correction applied to the draw, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_factor: Option<f32>,
    /// Display-space UI scale and translation in guest window units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_fit: Option<[f32; 4]>,
    pub vertex_position: String,
    pub sampled: Vec<LayoutTexture>,
    /// The native stage that submits the draw, when observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_stage: Option<String>,
    /// Minimum and maximum guest-window X/Y, when the bounds are available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_xy: Option<[[f32; 2]; 2]>,
}

/// One sampled stage and the image it reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutTexture {
    pub stage: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub header: u32,
    pub guest_size: [u32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_size: Option<[u32; 2]>,
    /// U and V address modes.
    pub address: [String; 2],
    /// Minimum and maximum normalized U/V, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uv: Option<[[f32; 2]; 2]>,
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

/// `render.stats`' `ui`: the layout the menus and the HUD are drawn with
/// (docs/graphics/high-resolution.md, "The game's 640x480 assumptions"),
/// as the game's UI scopes last set it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiStats {
    /// The window the UI is drawn at, `[width, height]`; the guest's own
    /// 640x480 canvas before the first UI scope.
    pub output: [u32; 2],
    /// The menu's fit in guest window units, `[sx, sy, ox, oy]`:
    /// guest `p` draws at `p * [sx, sy] + [ox, oy]`. The safe area's
    /// `min(w/640, h/480)` centred box; identity `[1, 1, 0, 0]` at 4:3.
    pub scale: [f32; 2],
    /// The fit's translation, guest window units.
    pub offset: [f32; 2],
    /// How the HUD's corner groups anchor: `pinned` to the window's edges
    /// at their physical scale, or `centered` on the safe area's edges.
    pub anchor: String,
    /// The scale the glyph pages rasterise the text atlas at, in whole
    /// multiples of the glyph's own size (0 before the first text run).
    pub atlas_scale: u32,
    /// The side of a glyph page in texels.
    pub atlas_page: u32,
}

/// `render.stats`: the render pipeline's counters since the game started,
/// each `null` where that piece isn't running.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderStats {
    /// Counters from an actual native graph presented to the display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_frame: Option<crate::frame_graph::DisplayStats>,
    /// The latest completed private-target draw while its renderer is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_frame: Option<crate::frame_graph::FrameGraphStats>,
    /// The frame owner's display-rate loop: display frames between sealed
    /// scenes (docs/graphics/framescene.md, section 7). Absent without a
    /// paced frame owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_rate: Option<DisplayRateStats>,
    /// The layout the menus and the HUD are drawn with. Absent before the
    /// game's UI has drawn a scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiStats>,
    /// The frame-stats window's per-frame means of the render counters.
    pub counters: Value,
}

/// `render.stats`' `display_rate`: counters since the loop was installed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct DisplayRateStats {
    /// The display rate the frame-rate setting asks for; 0 at 30 fps.
    pub rate_hz: u32,
    /// The rate the loop holds after measuring its own frames' cost.
    pub shown_hz: u32,
    /// Whether that cost holds the loop below `rate_hz`.
    pub capped: bool,
    /// Owned presents the loop saw.
    pub swaps: u64,
    /// Display frames drawn with a blended camera, at a Swap or between.
    pub blended: u64,
    /// Of `blended`, the ones shown at a Swap in place of its scene.
    pub swap_blends: u64,
    /// Display frames between Swaps that drew the newest scene as it is
    /// (a cut, a gap, or waiting on the next scene).
    pub between_newest: u64,
    /// Display frames that failed; the window kept the frame before.
    pub failed: u64,
    /// Times the loop dropped its two scenes (a Swap without the graph).
    pub resets: u64,
    /// Mean cost of a display frame on the render thread, microseconds.
    pub avg_cost_us: u64,
    /// Carried draws moved onto a blended camera.
    pub moved_draws: u64,
    /// Typed stages moved onto a blended camera.
    pub moved_stages: u64,
    /// Typed stages in a game view drawn as their tick drew them.
    pub held_stages: u64,
    /// Views on display frames whose world marks were moved onto the
    /// camera the frame drew the view with.
    pub marks_moved: u64,
    /// Views on display frames whose world marks kept their image's
    /// camera (no drawn view at their viewport).
    pub marks_held: u64,
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

/// Lighting for a private draw of one sealed scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureLighting {
    Retail,
    E3,
    #[serde(rename = "relight-on")]
    RelightOn,
    #[serde(rename = "relight-off")]
    RelightOff,
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
    /// Draw the selected sealed scene with this mode into private targets.
    /// The tick defaults to the latest sealed scene. Excludes `pass`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<CaptureLighting>,
    /// Include the rendered package, spawn camera and PNG digest for this
    /// Live capture game. Only whole frames from a spawn view qualify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
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
    /// The mode the private renderer drew, when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<CaptureLighting>,
    /// The sealed scene's summary hash at that present (`render.scene`);
    /// `null` with no scene source.
    pub scene_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CaptureSource>,
}

/// Render provenance for the captured frame. The capture reply supplies
/// the corresponding game tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureSource {
    pub game_id: String,
    pub map_id: String,
    pub map: String,
    pub camera: CaptureCamera,
    pub digest: String,
}

/// World-space eye position and orientation used to render this frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CaptureCamera {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
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
    /// Counters from an actual native graph presented to the display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_frame: Option<crate::frame_graph::DisplayStats>,
    /// The last private-target draw if it used exactly this sealed scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_frame: Option<crate::frame_graph::FrameGraphStats>,
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
    /// Side target uses and readbacks across all frames the device's sink recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carried_census: Option<SceneCarriedCensus>,
    /// Frozen census corpus at original Swap 3000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carried_checkpoint: Option<SceneCensusCheckpoint>,
    /// Activation of the requested level on a fresh level boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carried_activation: Option<SceneCensusActivation>,
    /// The first view whose Rust visibility differed from the guest's, and
    /// how (`scene_visibility_*` count them; docs/graphics/native-visibility.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_first_mismatch: Option<String>,
    /// The first light whose Rust visibility differed from its guest lists
    /// (`scene_light_visibility_*` count them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_visibility_first_mismatch: Option<String>,
    /// First differing slot or replay record of a first-person light view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_person_visibility_first_mismatch: Option<String>,
    /// The first view whose Rust-built records (camera, projection, start
    /// cluster, mode) differed from the guest's, and in which field
    /// (`scene_view_record_*` count them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_record_first_mismatch: Option<String>,
    /// The JSON file written when `dump` was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dump: Option<String>,
}

/// Totals over sink frames, including targets that only appeared during loading.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneCarriedCensus {
    pub frames: u64,
    /// Frames containing a guest D3D op inside a game view.
    pub game_frames: u64,
    pub targets: Vec<SceneTargetUse>,
    pub reads: Vec<SceneReadUse>,
}

/// Only a consecutive prefix from boot can supply the inventory and the
/// measured window. Each field appears when its own side reaches the Swap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneCensusCheckpoint {
    pub present: u64,
    pub sealed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory: Option<SceneCarriedCensus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counters: Option<BTreeMap<String, u64>>,
}

/// The level-boot mark, used as the baseline for post-load deltas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneCensusActivation {
    pub map: String,
    pub present: u64,
    /// The sink frame reached at activation.
    pub frame: u64,
    pub counters: BTreeMap<String, u64>,
}

/// Occurrences of a census row. Sink frames are numbered from one.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneUses {
    pub count: u64,
    pub first_frame: u64,
    pub last_frame: u64,
}

/// A side surface class, where it was used, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneTargetUse {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub depth: bool,
    pub slot: String,
    pub operation: String,
    #[serde(flatten)]
    pub uses: SceneUses,
}

/// Guest reads inside or outside a game view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneReadUse {
    pub slot: String,
    pub kind: String,
    #[serde(flatten)]
    pub uses: SceneUses,
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
    /// Upstream cluster counts for this game's camera. The scene dump
    /// has the corresponding IDs for each view ordinal in cluster_gathers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_clusters: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure_clusters: Option<u32>,
    /// The game view's object list length, from the same gather.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_objects: Option<u32>,
    /// Who computed the view's visible clusters and objects: `rust`, or
    /// `guest` when the Rust visibility declined for the view
    /// (docs/graphics/native-visibility.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_source: Option<String>,
    pub instances: u32,
    pub parts: u32,
    pub rows: u32,
    /// Guest-selected lights, in the view's draw list.
    pub lights: u32,
    /// All extracted light candidates, including the guest-selected lights.
    #[serde(default)]
    pub lights_candidates: u32,
    /// Candidates with both sets of lighting constants and any projection.
    #[serde(default)]
    pub lights_with_constants: u32,
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
    fn markers_params_take_a_kind_and_refuse_strays() {
        let none: MarkersParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(none.kind, None);
        let one: MarkersParams = serde_json::from_value(serde_json::json!({"kind": "hill"})).unwrap();
        assert_eq!(one.kind.as_deref(), Some("hill"));
        let play: MarkersParams = serde_json::from_value(serde_json::json!({"form": "play"})).unwrap();
        assert_eq!((play.kind, play.form.as_deref()), (None, Some("play")));
        for bad in [
            serde_json::json!({"team": 1}),
            serde_json::json!({"kind": 3}),
            serde_json::json!({"form": true}),
        ] {
            assert!(serde_json::from_value::<MarkersParams>(bad).is_err());
        }
    }

    #[test]
    fn a_marker_row_carries_its_form_icon_and_plate_scale() {
        let row = MarkerRow {
            thing: "netgame_flags:0".into(),
            kind: "hill".into(),
            team: None,
            anchor: [1.0, 2.0, 3.0],
            lines: 40,
            faded: false,
            form: "edit".into(),
            icon: Some("crown".into()),
            plate_scale: Some(2.0),
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["form"], "edit");
        assert_eq!(json["icon"], "crown");
        assert_eq!(json["plate_scale"], 2.0);
        assert_eq!(json["team"], serde_json::Value::Null);
        assert_eq!(serde_json::from_value::<MarkerRow>(json).unwrap(), row);
    }

    #[test]
    fn a_draw_omits_layout_depth_and_constants_when_absent() {
        let d = DrawRecord::default();
        let v = serde_json::to_value(&d).unwrap();
        assert!(v.get("depth").is_none());
        assert!(v.get("constants").is_none());
        assert!(v.get("vs").is_none());
        assert!(v.get("layout").is_none());
        assert_eq!(serde_json::from_value::<DrawRecord>(v).unwrap(), d);
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
    fn draw_layout_round_trips_extents_and_a_sampled_stage() {
        let d = DrawRecord {
            layout: Some(DrawLayout {
                guest_size: [640, 480],
                display_size: Some([2560, 1920]),
                clip: vec![[0, 0, 640, 480]],
                display_clip: vec![[0, 0, 2560, 1920]],
                window_xy: Some([[0.0, 0.0], [640.0, 480.0]]),
                sampled: vec![LayoutTexture {
                    stage: 1,
                    id: Some(5),
                    source: Some("render target".into()),
                    guest_size: [160, 120],
                    display_size: Some([160, 120]),
                    address: ["Clamp".into(), "Clamp".into()],
                    uv: Some([[-0.0625, 0.0], [1.0625, 1.0]]),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let v = serde_json::to_value(&d).unwrap();
        let layout = &v["layout"];
        assert_eq!(layout["display_size"], serde_json::json!([2560, 1920]));
        assert_eq!(layout["sampled"][0]["stage"], 1);
        assert!(layout.get("projection_factor").is_none());
        assert_eq!(serde_json::from_value::<DrawRecord>(v).unwrap(), d);
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
    fn the_census_preserves_counts_and_frame_bounds_on_the_wire() {
        let uses = SceneUses { count: 3, first_frame: 2, last_frame: 7 };
        let reply = SceneReply {
            carried_checkpoint: Some(SceneCensusCheckpoint {
                present: 3000, sealed: true, inventory: None,
                counters: Some(BTreeMap::from([("scene_window_games".into(), 5)])),
            }),
            carried_activation: Some(SceneCensusActivation {
                map: "invented".into(), present: 2, frame: 2,
                counters: BTreeMap::from([("scene_readback".into(), 1)]),
            }),
            carried_census: Some(SceneCarriedCensus {
                frames: 10,
                game_frames: 8,
                targets: vec![SceneTargetUse {
                    width: 9, height: 7, format: 6, depth: false,
                    slot: "after_lights".into(), operation: "copy_from".into(), uses,
                }],
                reads: vec![SceneReadUse { slot: "outside".into(), kind: "mirror".into(), uses }],
            }),
            ..Default::default()
        };
        let v = serde_json::to_value(&reply).unwrap();
        let saved = &v["carried_checkpoint"];
        assert_eq!(saved["present"], 3000);
        assert_eq!(saved["counters"]["scene_window_games"], 5);
        assert!(saved.get("inventory").is_none());
        let census = &v["carried_census"];
        assert_eq!(census["targets"][0]["first_frame"], 2);
        assert_eq!(census["reads"][0]["last_frame"], 7);
        assert_eq!(census["reads"][0]["count"], 3);
        assert_eq!(serde_json::from_value::<SceneReply>(v).unwrap(), reply);
        let mut old = serde_json::to_value(SceneReply::default()).unwrap();
        assert!(old.get("carried_checkpoint").is_none());
        assert!(old.as_object_mut().unwrap().remove("carried_census").is_none());
        assert!(serde_json::from_value::<SceneReply>(old).unwrap().carried_census.is_none());
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
        for (name, mode) in [("relight-on", CaptureLighting::RelightOn), ("relight-off", CaptureLighting::RelightOff)] {
            let p: FrameCaptureParams = serde_json::from_value(serde_json::json!({"lighting": name})).unwrap();
            assert_eq!(p.lighting, Some(mode));
            assert_eq!(serde_json::to_value(p).unwrap(), serde_json::json!({"lighting": name}));
        }
        assert!(serde_json::from_value::<FrameCaptureParams>(serde_json::json!({"tik": 3})).is_err());
        assert!(serde_json::from_value::<CompareParams>(serde_json::json!({"a": "x"})).is_err());
        let v = serde_json::to_value(CaptureReply::default()).unwrap();
        assert_eq!(v["scene_hash"], Value::Null, "null, not missing");
        assert!(v.get("pass").is_none());
    }

    #[test]
    fn view_params_are_checked() {
        let parse = |v: Value| serde_json::from_value::<ViewParams>(v).unwrap().check();
        assert_eq!(
            parse(serde_json::json!({"reset": true})),
            Ok(ViewRequest::Reset)
        );
        assert_eq!(
            parse(
                serde_json::json!({"eye": [1.0, 2.0, 3.0], "yaw": -1.0, "pitch": 0.5, "fov": 70.0})
            ),
            Ok(ViewRequest::Pose {
                eye: [1.0, 2.0, 3.0],
                yaw: (-1.0f32).rem_euclid(std::f32::consts::TAU),
                pitch: 0.5,
                fov: Some(70.0),
            })
        );
        assert!(matches!(
            parse(serde_json::json!({"eye": [0, 0, 0], "yaw": 0, "pitch": 0})),
            Ok(ViewRequest::Pose { fov: None, .. })
        ));
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"reset": true, "yaw": 0}),
            serde_json::json!({"eye": [0, 0, 0], "yaw": 0}),
            serde_json::json!({"eye": [0, 0, 0], "pitch": 0}),
            serde_json::json!({"yaw": 0, "pitch": 0}),
            serde_json::json!({"eye": [0, 0, 0], "yaw": 0, "pitch": 1.6}),
            serde_json::json!({"eye": [0, 0, 0], "yaw": 0, "pitch": -1.6}),
            serde_json::json!({"eye": [0, 0, 0], "yaw": 0, "pitch": 0, "fov": 0.5}),
            serde_json::json!({"eye": [0, 0, 0], "yaw": 0, "pitch": 0, "fov": 151}),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
        let nan = ViewParams {
            eye: Some([f32::NAN, 0.0, 0.0]),
            yaw: Some(0.0),
            pitch: Some(0.0),
            ..Default::default()
        };
        assert!(nan.check().is_err());
        let inf = ViewParams {
            eye: Some([0.0; 3]),
            yaw: Some(f32::INFINITY),
            pitch: Some(0.0),
            ..Default::default()
        };
        assert!(inf.check().is_err());
        for unknown in [
            serde_json::json!({"eye": [0, 0], "yaw": 0, "pitch": 0}),
            serde_json::json!({"index": 0}),
        ] {
            assert!(serde_json::from_value::<ViewParams>(unknown).is_err());
        }
    }

    #[test]
    fn a_current_view_derives_yaw_and_pitch_from_its_forward() {
        use std::f32::consts::{FRAC_PI_2, PI, TAU};
        let at = |forward: [f32; 3]| CurrentView::looking([1.0, 2.0, 3.0], forward, 70.0).unwrap();
        assert_eq!((at([1.0, 0.0, 0.0]).yaw, at([1.0, 0.0, 0.0]).pitch), (0.0, 0.0));
        assert!((at([0.0, 1.0, 0.0]).yaw - FRAC_PI_2).abs() < 1e-6);
        assert!((at([-1.0, 0.0, 0.0]).yaw - PI).abs() < 1e-6);
        // Wrapped into 0..TAU, not -PI..PI.
        assert!((at([0.0, -1.0, 0.0]).yaw - 1.5 * PI).abs() < 1e-5);
        let down = at([2.0, 0.0, -2.0]);
        assert!((down.pitch + std::f32::consts::FRAC_PI_4).abs() < 1e-5, "length is ignored");
        // Straight up clamps to the limit instead of failing the check.
        assert_eq!(at([0.0, 0.0, 1.0]).pitch, ViewParams::PITCH_LIMIT);
        assert_eq!(at([0.0, 0.0, -1.0]).pitch, -ViewParams::PITCH_LIMIT);
        // A tiny negative heading must not round up to TAU.
        let tiny = at([1.0, -1e-9, 0.0]);
        assert!((0.0..TAU).contains(&tiny.yaw), "{}", tiny.yaw);
        assert_eq!(tiny.eye, [1.0, 2.0, 3.0]);
        assert!(!tiny.private);
        for bad in [[0.0; 3], [f32::NAN, 0.0, 0.0], [f32::INFINITY, 0.0, 0.0]] {
            assert!(CurrentView::looking([0.0; 3], bad, 70.0).is_none());
        }
        assert!(CurrentView::looking([f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], 70.0).is_none());
        assert!(CurrentView::looking([0.0; 3], [1.0, 0.0, 0.0], f32::NAN).is_none());
    }

    #[test]
    fn a_current_view_replays_through_render_view() {
        let view = CurrentView::looking([12.5, -3.0, 2.2], [0.3, -0.8, 0.2], 74.0).unwrap();
        let wire = serde_json::to_value(view).unwrap();
        assert!(wire.get("private").is_none(), "{wire}");
        // The reply's pose fields, as a client passes them on.
        let params: ViewParams = serde_json::from_value(serde_json::json!({
            "eye": wire["eye"], "yaw": wire["yaw"], "pitch": wire["pitch"], "fov": wire["fov"],
        }))
        .unwrap();
        assert_eq!(params, view.view_params());
        assert_eq!(
            params.check(),
            Ok(ViewRequest::Pose {
                eye: view.eye,
                yaw: view.yaw,
                pitch: view.pitch,
                fov: Some(74.0),
            })
        );
        let private = serde_json::to_value(CurrentView { private: true, ..view }).unwrap();
        assert_eq!(private["private"], true);
        assert!(serde_json::from_value::<CurrentView>(private).unwrap().private);
    }

    #[test]
    fn current_view_params_check_the_seat() {
        let parse = |v: Value| serde_json::from_value::<CurrentViewParams>(v);
        assert_eq!(parse(serde_json::json!({})).unwrap().check(), Ok(0));
        assert_eq!(parse(serde_json::json!({"seat": 3})).unwrap().check(), Ok(3));
        assert!(parse(serde_json::json!({"seat": 4})).unwrap().check().is_err());
        assert!(parse(serde_json::json!({"seat": -1})).is_err());
        assert!(parse(serde_json::json!({"player": 1})).is_err());
    }
}
