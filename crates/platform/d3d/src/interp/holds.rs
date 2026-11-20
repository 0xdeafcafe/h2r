//! Why E1 held a frame, draw by draw: the world draws (and clears) its
//! guard found it could not warp, kept as a bounded log the agent's
//! `render.stats` shows as `interp.holds` (docs/game/high-framerate.md,
//! "Held frames, draw by draw").
//!
//! [`InterpStats`](super::InterpStats) counts frames under one reason;
//! this says which draws: the first offending draw of the last held frame
//! in full, and every kind of offending draw seen (reason, detail, vertex
//! program and primitive) with how many frames it held first, how many it
//! was in at all, and one example. The examples carry what the guard read
//! (where the program's clip position comes from, the block there) and
//! where the draw's first vertices land on the target after its program,
//! so a full-screen pass and a sprite tell apart; vertex programs met are
//! listed disassembled.

use std::collections::BTreeMap;

use serde::Serialize;

use super::camera::Persp;
use super::epilogue::Position;
use super::model::{DrawModel, Kind};
use crate::backend::ResolvedDraw;
use crate::shader::VertexProcessing;

/// Kinds kept ([`HoldLog::kinds`]); more are counted in
/// [`HoldLog::other_kinds`].
pub const MAX_KINDS: usize = 48;
/// Vertex programs kept disassembled ([`HoldLog::programs`]).
pub const MAX_PROGRAMS: usize = 24;
/// Vertices run through a draw's program for [`HeldDraw::window`].
pub const WINDOW_VERTICES: usize = 64;

/// Why the warp cannot show a world draw (or clear) where the guest drew
/// it. Each is one of the three `mismatch_*` reasons ([`Why::reason`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Why {
    // `screen`: not part of the turning world.
    /// Pre-transformed (`XYZRHW`) vertices: already in pixels.
    Pretransformed,
    /// Fixed-function vertex processing.
    FixedFunction,
    /// A program that passes a vertex through to `oPos` (`mov oPos, v0`):
    /// already in screen or clip space ([`Position::Fixed`]).
    PassThrough,
    /// A program whose clip position is not one constant block's rows
    /// ([`Position::Unknown`]).
    PositionUnknown,
    /// The block the clip position comes from is not a perspective (an
    /// orthographic quad, or no matrix at all).
    NotPerspective,
    /// A perspective of its own that is not centred (into pixels, sheared,
    /// off-axis).
    OffCentre,
    // `mapping`: where it lands on the target is not the camera's.
    /// No draw the camera places: nothing to read the mapping from.
    NoCameraDraw,
    /// A program whose viewport transform E1 cannot read.
    NoEpilogue,
    /// The camera's draws do not cover the target.
    CameraNotCovering,
    /// A clear of part of the target.
    PartialClear,
    /// A viewport transform other than the camera's draws'.
    OtherMapping,
    /// Scissored to part of the target.
    Scissored,
    // `overlay`.
    /// A draw the camera places after the overlay starts.
    CameraAfterOverlay,
}

impl Why {
    /// The `mismatch_*` counter it falls under: `screen`, `mapping` or
    /// `overlay`.
    pub fn reason(self) -> &'static str {
        use Why::*;
        match self {
            Pretransformed | FixedFunction | PassThrough | PositionUnknown | NotPerspective
            | OffCentre => "screen",
            NoCameraDraw | NoEpilogue | CameraNotCovering | PartialClear | OtherMapping
            | Scissored => "mapping",
            CameraAfterOverlay => "overlay",
        }
    }

    pub fn name(self) -> &'static str {
        use Why::*;
        match self {
            Pretransformed => "pretransformed",
            FixedFunction => "fixed_function",
            PassThrough => "pass_through",
            PositionUnknown => "position_unknown",
            NotPerspective => "not_perspective",
            OffCentre => "off_centre",
            NoCameraDraw => "no_camera_draw",
            NoEpilogue => "no_epilogue",
            CameraNotCovering => "camera_not_covering",
            PartialClear => "partial_clear",
            OtherMapping => "other_mapping",
            Scissored => "scissored",
            CameraAfterOverlay => "camera_after_overlay",
        }
    }
}

/// Where a recorded draw was in the guest's frame, numbered as
/// `render.frame` numbers them: draws over every target since the last
/// present, and passes (runs of work on one colour and depth surface; a
/// return to an earlier target is a new pass).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawAt {
    pub draw: u32,
    pub pass: u32,
}

/// One world draw or clear the guard refused, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offence {
    /// The op in the frame's recording.
    pub op: usize,
    /// The draw among the recorded ones, when it is a draw.
    pub draw: Option<usize>,
    pub why: Why,
}

/// What the screen-space guard knew of one offending draw.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HeldDraw {
    /// The guest frame (`InterpStats::frames` when it was presented).
    pub frame: u64,
    /// `screen`, `mapping` or `overlay`, and the detail ([`Why`]).
    pub reason: &'static str,
    pub why: &'static str,
    /// The draw in the guest frame and its pass, as `render.frame` numbers
    /// them (`None` for a clear, or a frame-wide reason).
    pub draw: Option<u32>,
    pub pass: Option<u32>,
    /// The draw among the presented surface's (the recorded ones).
    pub recorded: Option<usize>,
    /// The frame's recorded draws, those before the overlay, and the first
    /// overlay draw in the guest frame.
    pub recorded_draws: usize,
    pub world_draws: usize,
    pub overlay_draw: Option<u32>,
    /// The vertex program's id (`pipeline_store::program_id` of its
    /// microcode: what `pipelines-v2.jsonl` records it under), if it has
    /// one.
    pub vertex_program: Option<String>,
    pub entry: &'static str,
    pub primitive: String,
    pub vertices: u32,
    /// The bound textures' formats and sizes.
    pub textures: Vec<String>,
    pub blend: bool,
    pub depth_write: bool,
    pub depth: bool,
    pub pretransformed: bool,
    /// Where the program's clip position comes from: `rows c[N]` (one
    /// block's rows, hardware numbering), `pass-through`, `unknown`.
    pub position: String,
    /// What the block there is: `camera`, `world`, `local centred`,
    /// `local off-centre`, or `none` (not a perspective).
    pub block: String,
    /// The block's four rows as uploaded, when the position comes from one.
    pub rows: Option<[[f32; 4]; 4]>,
    /// The projection split from it (rows of `K`), when it is a perspective.
    pub projection: Option<[[f64; 4]; 4]>,
    /// The bounding box `[x0, y0, x1, y1]` of where the first
    /// [`WINDOW_VERTICES`] vertices land after the draw's program (window
    /// pixels; for pre-transformed vertices, their own), and the target's
    /// size, so a full-target pass shows as one.
    pub window: Option<[f32; 4]>,
    pub target: [u32; 2],
}

/// One kind of offending draw: the same reason, detail, vertex program and
/// primitive.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HoldKind {
    pub reason: &'static str,
    pub why: &'static str,
    pub vertex_program: Option<String>,
    pub primitive: String,
    /// Held frames whose first offending draw was this kind.
    pub frames_first: u64,
    /// Held frames with a draw of this kind among the offending ones.
    pub frames: u64,
    /// Offending draws of this kind, over all held frames.
    pub draws: u64,
    /// The first one seen.
    pub example: HeldDraw,
}

/// The log ([`super::InterpBackend::holds`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HoldLog {
    /// Frames the screen guard held (the sum of `mismatch_mapping`,
    /// `mismatch_screen` and `mismatch_overlay` as classified; a frame
    /// also unpaired is counted here and not there).
    pub frames: u64,
    /// The last such frame's first offending draw.
    pub last: Option<HeldDraw>,
    /// Every kind seen, most frames first.
    pub kinds: Vec<HoldKind>,
    /// Kinds past [`MAX_KINDS`], counted only.
    pub other_kinds: u64,
    /// The vertex programs of the kinds, disassembled, by id.
    pub programs: BTreeMap<String, String>,
}

/// One offending draw's facts, for [`HoldLog::record`].
pub(crate) struct Facts<'a> {
    pub draw: &'a ResolvedDraw,
    pub at: DrawAt,
    pub recorded: usize,
    pub model: &'a DrawModel,
    pub program: Option<u64>,
}

/// The frame-wide facts.
pub(crate) struct FrameFacts {
    pub frame: u64,
    pub recorded_draws: usize,
    pub world_draws: usize,
    pub overlay_draw: Option<u32>,
}

impl HoldLog {
    /// Records one held frame's offences (in the guard's order: the first
    /// is the one it held for), each with its draw's facts when it is a
    /// draw.
    pub(crate) fn record(&mut self, f: &FrameFacts, offences: &[(Why, Option<Facts<'_>>)]) {
        let Some(first) = offences.first() else {
            return;
        };
        self.frames += 1;
        self.last = Some(describe(f, first.0, first.1.as_ref()));
        let mut seen: Vec<usize> = Vec::new();
        for (n, (why, facts)) in offences.iter().enumerate() {
            let program = facts.as_ref().and_then(|x| x.program).map(hex);
            let primitive = facts
                .as_ref()
                .map(|x| format!("{:?}", x.draw.primitive))
                .unwrap_or_default();
            let i = match self.kinds.iter().position(|k| {
                k.why == why.name() && k.vertex_program == program && k.primitive == primitive
            }) {
                Some(i) => i,
                None if self.kinds.len() < MAX_KINDS => {
                    let example = describe(f, *why, facts.as_ref());
                    if let (Some(id), Some(x)) = (&program, facts.as_ref())
                        && self.programs.len() < MAX_PROGRAMS
                        && let VertexProcessing::Program { microcode, .. } = &x.draw.shader.vertex
                        && let Ok(p) = shaders::vsh::Program::from_words(
                            shaders::vsh::ProgramKind::Normal,
                            microcode,
                        )
                    {
                        self.programs
                            .entry(id.clone())
                            .or_insert_with(|| shaders::vsh::disassemble(&p));
                    }
                    self.kinds.push(HoldKind {
                        reason: why.reason(),
                        why: why.name(),
                        vertex_program: program,
                        primitive,
                        frames_first: 0,
                        frames: 0,
                        draws: 0,
                        example,
                    });
                    self.kinds.len() - 1
                }
                None => {
                    self.other_kinds += 1;
                    continue;
                }
            };
            let k = &mut self.kinds[i];
            k.draws += 1;
            if n == 0 {
                k.frames_first += 1;
            }
            if !seen.contains(&i) {
                seen.push(i);
                k.frames += 1;
            }
        }
        // Most frames first, stable.
        self.kinds.sort_by(|a, b| b.frames.cmp(&a.frames));
    }
}

fn hex(id: u64) -> String {
    format!("{id:016x}")
}

fn describe(f: &FrameFacts, why: Why, facts: Option<&Facts<'_>>) -> HeldDraw {
    let mut h = HeldDraw {
        frame: f.frame,
        reason: why.reason(),
        why: why.name(),
        recorded_draws: f.recorded_draws,
        world_draws: f.world_draws,
        overlay_draw: f.overlay_draw,
        ..HeldDraw::default()
    };
    let Some(x) = facts else {
        return h;
    };
    let d = x.draw;
    let r = &d.state.render;
    h.draw = Some(x.at.draw);
    h.pass = Some(x.at.pass);
    h.recorded = Some(x.recorded);
    h.vertex_program = x.program.map(hex);
    h.entry = d.entry;
    h.primitive = format!("{:?}", d.primitive);
    h.vertices = d.vertex_count;
    h.textures = d
        .texture_info
        .iter()
        .flatten()
        .map(|t| format!("{:?} {}x{}", t.format, t.width, t.height))
        .collect();
    h.blend = r.blend_enable;
    h.depth_write = r.depth_write;
    h.depth = d.target.depth_format.is_some();
    h.pretransformed = d.layout.pretransformed;
    h.position = match x.model.position {
        Position::Rows(b) => format!("rows c[{b}]"),
        Position::Fixed => "pass-through".into(),
        Position::Unknown => "unknown".into(),
    };
    h.block = match x.model.position_block().map(|b| b.kind) {
        Some(Kind::Camera) => "camera".into(),
        Some(Kind::World(_)) => "world".into(),
        Some(Kind::Local(p)) if p.centred() => "local centred".into(),
        Some(Kind::Local(_)) => "local off-centre".into(),
        Some(Kind::Node(_) | Kind::Eye) | None => "none".into(),
    };
    if let Position::Rows(b) = x.model.position {
        let b = b as usize;
        let c = &d.shader.constants;
        if b + 4 <= c.len() {
            let rows = [c[b], c[b + 1], c[b + 2], c[b + 3]];
            h.rows = Some(rows);
            let m = rows.map(|row| row.map(f64::from));
            h.projection = Persp::split(&m).map(|p| p.projection);
        }
    }
    h.window = window(d);
    h.target = [d.target.width, d.target.height];
    h
}

/// Where the first [`WINDOW_VERTICES`] vertices of `d` land after its
/// program (or as given, pre-transformed): their bounding box in window
/// pixels.
pub fn window(d: &ResolvedDraw) -> Option<[f32; 4]> {
    let program = match &d.shader.vertex {
        VertexProcessing::Program { microcode, .. } => Some(
            shaders::vsh::Program::from_words(shaders::vsh::ProgramKind::Normal, microcode).ok()?,
        ),
        VertexProcessing::FixedFunction { .. } if d.layout.pretransformed => None,
        VertexProcessing::FixedFunction { .. } => return None,
    };
    let n = (d.vertex_count as usize).min(WINDOW_VERTICES);
    let mut b = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for v in 0..n {
        let mut inputs = d.shader.vertex_defaults;
        for (layout, stream) in d.layout.streams.iter().zip(&d.streams) {
            // The guest's stride wins over the declaration's.
            let stride = if stream.stride > 0 {
                stream.stride
            } else {
                layout.stride
            } as usize;
            let bytes: &[u8] = &stream.data;
            for a in &layout.attributes {
                let at = v * stride + a.offset as usize;
                let size = a.data_type.byte_size() as usize;
                let Some(src) = bytes.get(at..at + size) else {
                    continue;
                };
                if let Some(slot) = inputs.get_mut(a.register.0 as usize) {
                    *slot = crate::device::attribute_value(a.data_type, src);
                }
            }
        }
        let pos = match &program {
            Some(p) => {
                let mut c = *d.shader.constants;
                shaders::vsh::interp::run(p, &inputs, &mut c).get(shaders::vsh::OutReg::Pos)
            }
            None => inputs[0],
        };
        if !(pos[0].is_finite() && pos[1].is_finite()) {
            continue;
        }
        b = [
            b[0].min(pos[0]),
            b[1].min(pos[1]),
            b[2].max(pos[0]),
            b[3].max(pos[1]),
        ];
    }
    b[0].is_finite().then_some(b)
}
