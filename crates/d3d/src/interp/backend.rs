//! [`InterpBackend`]: records each frame's draws into the presented
//! surface and replays the newest one with interpolated matrices into
//! surfaces of its own, at the display rate, on the render thread.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use super::epilogue::{Epilogue, Position, position};
use super::holds::{DrawAt, Facts, FrameFacts, HoldLog, Offence, Why};
use super::model::{FrameModel, Kind, Substituted, Tuning, match_draws, substitute};
use super::pacing::{Rate, Schedule, alpha, blendable, defer};
use super::reproject::{self, Screen};
use super::screen_space::{self, Fit, PassThrough};
use crate::backend::{
    Backend, DrawJudge, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw, StreamBytes,
};
use crate::shader::{Constants, Microcode, VertexProcessing};

/// Counters, shared with whoever wants to print them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterpStats {
    /// Guest frames presented while the replay was on.
    pub frames: u64,
    /// Of those, frames shown blended (a replay at the present).
    pub blended_frames: u64,
    /// Frames shown as the guest drew them: no previous frame to blend
    /// from, a gap, a view change, or nothing to move.
    pub held_frames: u64,
    /// In-between frames replayed between presents.
    pub replays: u64,
    /// Draws replayed with substituted constants, over all replays.
    pub substituted_draws: u64,
    /// Views or blocks snapped instead of blended (cuts, teleports).
    pub snapped: u64,
    /// Idle calls that put a replay off because a pixel count was open.
    pub deferred: u64,
    /// Design D: frames made by replaying the whole frame (at presents and
    /// in between), their render-thread time in total and the longest, ns
    /// (the draws and the present).
    pub replay_frames: u64,
    pub replay_ns: u64,
    pub replay_max_ns: u64,
    /// E1: world images drawn (once per guest frame), their time, ns.
    pub captures: u64,
    pub capture_ns: u64,
    pub capture_max_ns: u64,
    /// E1: frames made by warping (at presents and in between: the warp,
    /// the overlay draws and the present), their time, ns.
    pub warps: u64,
    pub warp_ns: u64,
    pub warp_max_ns: u64,
    /// E1: pass-through world draws (window pixels: sprites, full-target
    /// layers) drawn into world images, scaled about the centre with the
    /// world (`screen_shrunk`) or kept over the whole image with their
    /// other inputs carried outward (`screen_extended`)
    /// (`interp::screen_space`).
    pub screen_shrunk: u64,
    pub screen_extended: u64,
    /// Frames held because a made frame would not have matched the
    /// guest's: the sum of the four below.
    pub size_mismatch: u64,
    /// Of those: the made frame's host size differs from the guest's.
    pub mismatch_extent: u64,
    /// E1: a world draw's screen mapping (its program's viewport
    /// transform) is not one the replay can read, differs from the
    /// camera's, does not cover the target, or is scissored; or a partial
    /// clear in the world.
    pub mismatch_mapping: u64,
    /// E1: a world draw in screen space (pre-transformed, fixed function,
    /// or without a centred perspective: an orthographic quad, a
    /// perspective into pixels): it is not part of the turning world.
    pub mismatch_screen: u64,
    /// E1: a draw that uses the camera after the overlay starts: it would
    /// go on top of the warped world unwarped.
    pub mismatch_overlay: u64,
    /// Frames held because the classification was not sure of a gameplay
    /// view: the sum of the two below.
    pub unsure: u64,
    /// Of those: a camera that is not a centred perspective (menus, screen
    /// space).
    pub unsure_camera: u64,
    /// Of those: draws that read something the replay could not keep (a
    /// render target it could not copy, a surface copied in).
    pub unsure_unpinned: u64,
    /// Frames held because there was nothing to blend: the sum of the four
    /// below.
    pub unpaired: u64,
    /// Of those: no previous frame close enough to blend from (the first
    /// frame, or one after a gap: a load, a pause, a hitch).
    pub unpaired_gap: u64,
    /// Of those: no camera (no perspective block that two draws compute
    /// their position from: a 2D menu, a loading screen).
    pub unpaired_camera: u64,
    /// Of those: the views are not the previous frame's (a split-screen
    /// change); for E1, not one view with a world before the overlay.
    pub unpaired_views: u64,
    /// Of those: a pixel count was open at the present.
    pub unpaired_counting: u64,
    /// In-between frames dropped (and guest frames shown as drawn) because
    /// the GPU had not finished the last made frame.
    pub dropped: u64,
}

impl InterpStats {
    fn time_replay(&mut self, ns: u64) {
        self.replay_frames += 1;
        self.replay_ns += ns;
        self.replay_max_ns = self.replay_max_ns.max(ns);
    }

    fn time_capture(&mut self, ns: u64) {
        self.captures += 1;
        self.capture_ns += ns;
        self.capture_max_ns = self.capture_max_ns.max(ns);
    }

    fn time_warp(&mut self, ns: u64) {
        self.warps += 1;
        self.warp_ns += ns;
        self.warp_max_ns = self.warp_max_ns.max(ns);
    }

    fn held(&mut self, why: Hold) {
        match why {
            Hold::Unsure(u) => {
                self.unsure += 1;
                match u {
                    Unsure::Camera => self.unsure_camera += 1,
                    Unsure::Unpinned => self.unsure_unpinned += 1,
                }
            }
            Hold::Unpaired(u) => {
                self.unpaired += 1;
                match u {
                    Unpaired::Gap => self.unpaired_gap += 1,
                    Unpaired::Camera => self.unpaired_camera += 1,
                    Unpaired::Views => self.unpaired_views += 1,
                    Unpaired::Counting => self.unpaired_counting += 1,
                }
            }
            Hold::SizeMismatch(m) => {
                self.size_mismatch += 1;
                match m {
                    Mismatch::Extent => self.mismatch_extent += 1,
                    Mismatch::Mapping => self.mismatch_mapping += 1,
                    Mismatch::Screen => self.mismatch_screen += 1,
                    Mismatch::Overlay => self.mismatch_overlay += 1,
                }
            }
        }
    }
}

fn avg_ms(total: u64, n: u64) -> f64 {
    if n == 0 {
        0.0
    } else {
        total as f64 / n as f64 / 1e6
    }
}

impl std::fmt::Display for InterpStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "frames {} blended {} held {} replays {} substituted draws {} snapped {} deferred {} size_mismatch {} (extent {} mapping {} screen {} overlay {}) unsure {} (camera {} unpinned {}) unpaired {} (gap {} camera {} views {} counting {}) dropped {}",
            self.frames,
            self.blended_frames,
            self.held_frames,
            self.replays,
            self.substituted_draws,
            self.snapped,
            self.deferred,
            self.size_mismatch,
            self.mismatch_extent,
            self.mismatch_mapping,
            self.mismatch_screen,
            self.mismatch_overlay,
            self.unsure,
            self.unsure_camera,
            self.unsure_unpinned,
            self.unpaired,
            self.unpaired_gap,
            self.unpaired_camera,
            self.unpaired_views,
            self.unpaired_counting,
            self.dropped
        )?;
        if self.replay_frames > 0 {
            write!(
                f,
                "; replay mode: {} frames, {:.2} ms avg, {:.2} ms max",
                self.replay_frames,
                avg_ms(self.replay_ns, self.replay_frames),
                self.replay_max_ns as f64 / 1e6
            )?;
        }
        if self.captures > 0 || self.warps > 0 {
            write!(
                f,
                "; reproject mode: {} world images {:.2} ms avg {:.2} ms max, {} warped frames {:.2} ms avg {:.2} ms max",
                self.captures,
                avg_ms(self.capture_ns, self.captures),
                self.capture_max_ns as f64 / 1e6,
                self.warps,
                avg_ms(self.warp_ns, self.warps),
                self.warp_max_ns as f64 / 1e6
            )?;
            if self.screen_shrunk > 0 || self.screen_extended > 0 {
                write!(
                    f,
                    ", screen-space draws {} shrunk {} extended",
                    self.screen_shrunk, self.screen_extended
                )?;
            }
        }
        Ok(())
    }
}

/// How in-between frames are made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Design D: the frame's draws again, with camera and object matrices
    /// blended from the frame before.
    #[default]
    Replay,
    /// E1: frame N's world image (drawn once, with overscan) turned by the
    /// camera's rotation in one full-screen pass, the first-person and HUD
    /// draws replayed on top. With `ahead`, the rotation carries on past N
    /// instead of blending from N-1.
    Reproject { ahead: bool },
}

enum Op<B: Backend> {
    Clear {
        color: B::Surface,
        depth: Option<B::Surface>,
        clear: Clear,
    },
    ClearRects {
        color: B::Surface,
        depth: Option<B::Surface>,
        clear: RectClear,
    },
    Draw {
        color: B::Surface,
        depth: Option<B::Surface>,
        draw: Box<ResolvedDraw>,
        textures: [Option<Arc<B::Texture>>; 4],
        /// Where it was in the guest's frame (for the hold log).
        at: DrawAt,
    },
    Copy {
        src: B::Surface,
        dst: B::Surface,
    },
}

impl<B: Backend> Op<B> {
    fn targets(&self, s: B::Surface) -> bool {
        match self {
            Op::Clear { color, .. } | Op::ClearRects { color, .. } | Op::Draw { color, .. } => {
                *color == s
            }
            Op::Copy { dst, .. } => *dst == s,
        }
    }
}

struct Frame<B: Backend> {
    id: u64,
    surface: B::Surface,
    /// Arrival (ns), stamped on the game's thread.
    at: u64,
    ops: Vec<Op<B>>,
    model: FrameModel,
    /// For each draw of `model`, the draw of the previous frame it
    /// continues.
    matches: Vec<Option<usize>>,
    /// E1: the first op of the overlay (first-person and HUD draws).
    world_end: usize,
    /// E1: where the world draws' coordinates land on the target.
    screen: Option<Screen>,
    /// E1: each draw's viewport transform (the world image scales it).
    epilogues: Vec<Option<Epilogue>>,
    /// E1: this frame's world image has been drawn.
    captured: bool,
    /// E1: per draw, the streams a pass-through world draw is drawn with
    /// into the world image, and how they were made (`screen_space`).
    streams: Vec<Option<(Vec<StreamBytes>, Fit)>>,
    /// Why the frame may not be made again, if it may not.
    hold: Option<Hold>,
}

/// Why a frame is shown only as the guest drew it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    /// Not sure it is a gameplay view ([`InterpStats::unsure`]).
    Unsure(Unsure),
    /// It would not land where the guest's frame does
    /// ([`InterpStats::size_mismatch`]).
    SizeMismatch(Mismatch),
    /// There is nothing to blend ([`InterpStats::unpaired`]).
    Unpaired(Unpaired),
}

/// Why a frame is [`Hold::Unpaired`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unpaired {
    Gap,
    Camera,
    Views,
    Counting,
}

/// Why a frame is [`Hold::Unsure`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unsure {
    Camera,
    Unpinned,
}

/// Why a made frame would not land where the guest's does
/// ([`InterpStats::mismatch_extent`] and the rest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mismatch {
    Extent,
    Mapping,
    Screen,
    Overlay,
}

impl Why {
    fn mismatch(self) -> Mismatch {
        match self.reason() {
            "screen" => Mismatch::Screen,
            "overlay" => Mismatch::Overlay,
            _ => Mismatch::Mapping,
        }
    }
}

/// E1: the screen mapping the frame's world image is warped through, or
/// why the warp could not be shown to put the world where the guest drew
/// it: every world draw or clear the guard refuses, in order (the first is
/// the frame's hold reason). The world image is the ops before `world_end` again with every
/// program's viewport scale times the overscan; that moves a draw with
/// the rest only when its program's viewport transform is known, is the
/// camera's and covers the whole target, unscissored. Screen-space draws
/// (pre-transformed, fixed function, no centred perspective: an
/// orthographic quad, a perspective into pixels) would shrink with the
/// overscan and then turn with the world, partial clears are in pixels, and a
/// draw after the overlay starts that uses the camera would go on top of
/// the turned world unturned.
fn world_screen<B: Backend>(
    ops: &[Op<B>],
    world_end: usize,
    model: &FrameModel,
    epilogues: &[Option<Epilogue>],
    pass_throughs: &[Option<PassThrough>],
) -> Result<(Screen, Vec<Option<(Vec<StreamBytes>, Fit)>>), Vec<Offence>> {
    let draws: Vec<(usize, &ResolvedDraw)> = ops
        .iter()
        .enumerate()
        .filter_map(|(i, o)| match o {
            Op::Draw { draw, .. } => Some((i, &**draw)),
            _ => None,
        })
        .collect();
    let one = |op: usize, draw: Option<usize>, why: Why| vec![Offence { op, draw, why }];
    // The screen mapping is the first draw's the camera places.
    let first = (0..draws.len())
        .find(|&k| {
            model.draws[k]
                .position_block()
                .is_some_and(|b| b.kind == Kind::Camera)
        })
        .ok_or_else(|| one(0, None, Why::NoCameraDraw))?;
    let e = epilogues[first]
        .as_ref()
        .ok_or_else(|| one(draws[first].0, Some(first), Why::NoEpilogue))?;
    let screen = Screen::of(draws[first].1, e);
    if !screen.covers_target() {
        return Err(one(draws[first].0, Some(first), Why::CameraNotCovering));
    }
    let (w, h) = (screen.size[0] as u32, screen.size[1] as u32);
    let mut out = Vec::new();
    let mut streams = vec![None; draws.len()];
    for (i, o) in ops[..world_end].iter().enumerate() {
        if let Op::ClearRects { clear, .. } = o
            && clear.rects.iter().any(|r| !r.is_whole(w, h))
        {
            out.push(Offence {
                op: i,
                draw: None,
                why: Why::PartialClear,
            });
        }
    }
    for (k, &(i, d)) in draws.iter().enumerate() {
        let why = draw_offence(
            d,
            &model.draws[k],
            epilogues[k].as_ref(),
            &screen,
            i >= world_end,
        );
        // A world draw in window pixels whose program moves its position
        // and nothing else: drawn into the world image moved with the
        // world (`screen_space`), when its positions can be rewritten and
        // it is not scissored.
        if why == Some(Why::PassThrough)
            && let Some(pt) = pass_throughs[k]
            && d.clip.is_none_or(|c| c.is_whole(w, h))
            && d.clip_rects.is_empty()
            && let Some(sub) =
                screen_space::world_image_streams(d, pt, reproject::overscan_scale(), &screen)
        {
            streams[k] = Some(sub);
            continue;
        }
        if let Some(why) = why {
            out.push(Offence {
                op: i,
                draw: Some(k),
                why,
            });
        }
    }
    if out.is_empty() {
        Ok((screen, streams))
    } else {
        Err(out)
    }
}

/// Why world draw `d` (or, `overlay`, a draw after the overlay starts)
/// stops the warp from showing the frame, if it does.
fn draw_offence(
    d: &ResolvedDraw,
    m: &super::model::DrawModel,
    e: Option<&Epilogue>,
    screen: &Screen,
    overlay: bool,
) -> Option<Why> {
    let (w, h) = (screen.size[0] as u32, screen.size[1] as u32);
    if overlay {
        return m.placed_by_camera().then_some(Why::CameraAfterOverlay);
    }
    // Only the block the program puts its vertices through says where
    // they go: the rest of its constants may hold anything.
    let block = m.position_block().map(|b| b.kind);
    if d.layout.pretransformed {
        return Some(Why::Pretransformed);
    }
    if matches!(d.shader.vertex, VertexProcessing::FixedFunction { .. }) {
        return Some(Why::FixedFunction);
    }
    match (m.position, block) {
        (_, Some(Kind::Camera | Kind::World(_))) => {}
        (_, Some(Kind::Local(p))) if p.centred() => {}
        (_, Some(Kind::Local(_))) => return Some(Why::OffCentre),
        (Position::Fixed, _) => return Some(Why::PassThrough),
        (Position::Unknown, _) => return Some(Why::PositionUnknown),
        (Position::Rows(_), _) => return Some(Why::NotPerspective),
    }
    let Some(e) = e else {
        return Some(Why::NoEpilogue);
    };
    if !screen.same(&Screen::of(d, e)) {
        return Some(Why::OtherMapping);
    }
    if d.clip.is_some_and(|c| !c.is_whole(w, h)) || !d.clip_rects.is_empty() {
        return Some(Why::Scissored);
    }
    None
}

/// Runs `ops` on `inner`, each surface through `to`, draw `k` (counting
/// from `first_draw`) with `subs(k)` for constants when it gives some.
fn run_ops<B: Backend>(
    inner: &mut B,
    ops: &mut [Op<B>],
    first_draw: usize,
    to: &dyn Fn(B::Surface) -> B::Surface,
    subs: &dyn Fn(usize) -> Option<Constants>,
    streams: &dyn Fn(usize) -> Option<Vec<StreamBytes>>,
) -> Result<(), String> {
    let mut k = first_draw;
    for o in ops.iter_mut() {
        match o {
            Op::Clear {
                color,
                depth,
                clear,
            } => inner.clear(to(*color), depth.map(to), *clear)?,
            Op::ClearRects {
                color,
                depth,
                clear,
            } => {
                inner.clear_rects(to(*color), depth.map(to), clear)?;
            }
            Op::Copy { src, dst } => {
                inner.copy_surface(to(*src), to(*dst))?;
            }
            Op::Draw {
                color,
                depth,
                draw,
                textures,
                ..
            } => {
                let tex: [Option<&B::Texture>; 4] = std::array::from_fn(|i| textures[i].as_deref());
                let own_constants =
                    subs(k).map(|c| std::mem::replace(&mut draw.shader.constants, c));
                let own_streams = streams(k).map(|s| std::mem::replace(&mut draw.streams, s));
                let r = inner.draw(to(*color), depth.map(to), draw, tex);
                if let Some(c) = own_constants {
                    draw.shader.constants = c;
                }
                if let Some(s) = own_streams {
                    draw.streams = s;
                }
                r?;
                k += 1;
            }
        }
    }
    Ok(())
}

fn draws_in<B: Backend>(ops: &[Op<B>]) -> usize {
    ops.iter().filter(|o| matches!(o, Op::Draw { .. })).count()
}

/// The depth surfaces `ops` use, in order.
fn depths_in<B: Backend>(ops: &[Op<B>]) -> Vec<B::Surface> {
    let mut v = Vec::new();
    for o in ops {
        if let Op::Clear { depth: Some(d), .. }
        | Op::ClearRects { depth: Some(d), .. }
        | Op::Draw { depth: Some(d), .. } = o
            && !v.contains(d)
        {
            v.push(*d);
        }
    }
    v
}

fn constants_of<B: Backend>(ops: &[Op<B>]) -> Vec<Constants> {
    ops.iter()
        .filter_map(|o| match o {
            Op::Draw { draw, .. } => Some(draw.shader.constants.clone()),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct SurfaceInfo {
    format: D3DFormat,
    width: u32,
    height: u32,
    depth: bool,
}

/// A [`Backend`] that passes everything to `B` and, with the frame-rate
/// setting above 30, shows in-between frames: each guest frame's draws into
/// the presented surface are kept, and the newest frame is drawn again into
/// a surface of its own with camera and object matrices blended from the
/// frame before (docs/game/high-framerate.md, "Phase 2").
///
/// Nothing the guest can observe goes through the replay: it draws only
/// into its own surfaces, never reads back, never runs while a pixel count
/// is open, and the guest's own calls reach `B` exactly as without it.
/// With the setting at 30 it records nothing and forwards every call.
pub struct InterpBackend<B: Backend> {
    inner: B,
    rate: Box<dyn Fn() -> u32 + Send>,
    clock: Box<dyn Fn() -> u64 + Send>,
    base: std::time::Instant,
    tuning: Tuning,
    surfaces: Vec<(B::Surface, SurfaceInfo)>,
    /// Surfaces presented so far (the swap chain).
    shown: Vec<B::Surface>,
    /// The frame being recorded.
    ops: Vec<Op<B>>,
    /// The arrival of the present about to run ([`Backend::stamp_present`]).
    stamp: Option<u64>,
    /// When the counters were last printed (every 5 s while running, so a
    /// run stopped by a signal still leaves them in the log).
    reported: std::time::Instant,
    prev: Option<Frame<B>>,
    cur: Option<Frame<B>>,
    /// Each real surface's private copy.
    private: Vec<(B::Surface, B::Surface)>,
    /// E1: each real surface's world-image copy.
    world_private: Vec<(B::Surface, B::Surface)>,
    mode: Mode,
    /// A pixel count is open (cleared, not yet reported): no replay may
    /// add to it.
    counting: bool,
    schedule: Schedule,
    /// The frame and alpha last shown.
    last: Option<(u64, f32)>,
    gamma: Option<Box<[u8; 768]>>,
    next_id: u64,
    failed: bool,
    stats: Arc<Mutex<InterpStats>>,
    /// Textures that are views of render targets (their contents change
    /// when the target is drawn again), with the surface.
    live: Vec<(Weak<B::Texture>, B::Surface)>,
    /// Copies of render targets taken for recorded draws that sampled them,
    /// valid until the surface is written.
    snaps: Vec<(B::Surface, Arc<B::Texture>)>,
    /// The frame being recorded read something the replay cannot keep: a
    /// render target it could not copy, or a surface copied in.
    unpinned: bool,
    /// Each vertex program, as read from its microcode once: its viewport
    /// transform ([`Epilogue::of`]; `None`: not one E1 can follow) and the
    /// block its clip position comes from ([`position`]).
    programs: HashMap<Microcode, (Option<Epilogue>, Position, Option<PassThrough>)>,
    /// Each vertex program's id (`pipeline_store::program_id`), for the
    /// hold log.
    program_ids: HashMap<Microcode, u64>,
    /// Whether the GPU has finished the last made frame.
    fence: Option<Box<dyn Fn() -> bool + Send>>,
    /// The guest frame so far, numbered as `render.frame` numbers it: its
    /// draws over every target, the pass (`None` before its first work)
    /// and that pass's colour and depth surfaces.
    frame_draws: u32,
    pass: Option<(u32, B::Surface, Option<B::Surface>)>,
    /// E1: which draws held frames ([`HoldLog`]).
    holds: Arc<Mutex<HoldLog>>,
}

impl<B: Backend> InterpBackend<B> {
    /// Wraps `inner`; `rate` reads the frame-rate setting (30 off, 0
    /// uncapped), on the render thread, at every present and idle call.
    pub fn new(inner: B, rate: impl Fn() -> u32 + Send + 'static) -> Self {
        let base = std::time::Instant::now();
        Self {
            inner,
            rate: Box::new(rate),
            clock: Box::new(move || base.elapsed().as_nanos() as u64),
            base,
            tuning: Tuning::default(),
            surfaces: Vec::new(),
            shown: Vec::new(),
            ops: Vec::new(),
            stamp: None,
            reported: std::time::Instant::now(),
            prev: None,
            cur: None,
            private: Vec::new(),
            world_private: Vec::new(),
            mode: Mode::Replay,
            counting: false,
            schedule: Schedule::default(),
            last: None,
            gamma: None,
            next_id: 1,
            failed: false,
            stats: Arc::default(),
            live: Vec::new(),
            snaps: Vec::new(),
            unpinned: false,
            programs: HashMap::new(),
            program_ids: HashMap::new(),
            fence: None,
            frame_draws: 0,
            pass: None,
            holds: Arc::default(),
        }
    }

    /// Uses `clock` (nanoseconds) instead of wall time: a fake display
    /// clock for tests. Stamps given to [`Backend::stamp_present`] must be
    /// on the same clock.
    pub fn with_clock(mut self, clock: impl Fn() -> u64 + Send + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Nanoseconds on this backend's clock for `at` (for stamps made with
    /// the wall clock).
    pub fn wall_ns(base: std::time::Instant, at: std::time::Instant) -> u64 {
        at.saturating_duration_since(base).as_nanos() as u64
    }

    /// Makes in-between frames the `mode` way ([`Mode::Replay`] unless
    /// set).
    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    /// The counters.
    pub fn stats(&self) -> Arc<Mutex<InterpStats>> {
        Arc::clone(&self.stats)
    }

    /// E1: the draws that held frames, kind by kind (`render.stats`'s
    /// `interp.holds`).
    pub fn holds(&self) -> Arc<Mutex<HoldLog>> {
        Arc::clone(&self.holds)
    }

    /// Work on `color` and `depth` in the guest's frame: a new pass when
    /// they are not the current pass's.
    fn enter(&mut self, color: B::Surface, depth: Option<B::Surface>) -> u32 {
        let n = match self.pass {
            Some((n, c, d)) if c == color && d == depth => n,
            Some((n, ..)) => n + 1,
            None => 0,
        };
        self.pass = Some((n, color, depth));
        n
    }

    /// A draw in the guest's frame: where it is ([`DrawAt`]).
    fn draw_at(&mut self, color: B::Surface, depth: Option<B::Surface>) -> DrawAt {
        let pass = self.enter(color, depth);
        let draw = self.frame_draws;
        self.frame_draws += 1;
        DrawAt { draw, pass }
    }

    /// `d`'s vertex program's id, if it has one.
    fn program_id(&mut self, d: &ResolvedDraw) -> Option<u64> {
        match &d.shader.vertex {
            VertexProcessing::Program { microcode, .. } => Some(
                *self
                    .program_ids
                    .entry(microcode.clone())
                    .or_insert_with(|| crate::pipeline_store::program_id(microcode)),
            ),
            VertexProcessing::FixedFunction { .. } => None,
        }
    }

    /// Notes a frame the screen guard held in the hold log: every
    /// offence, with its draw.
    fn note_held(
        &mut self,
        frame: u64,
        ops: &[Op<B>],
        model: &FrameModel,
        world_end: usize,
        offences: &[Offence],
    ) {
        let draws: Vec<(&ResolvedDraw, DrawAt)> = ops
            .iter()
            .filter_map(|o| match o {
                Op::Draw { draw, at, .. } => Some((&**draw, *at)),
                _ => None,
            })
            .collect();
        let world_draws = draws_in(&ops[..world_end]);
        let ids: Vec<Option<u64>> = offences
            .iter()
            .map(|o| o.draw.and_then(|k| self.program_id(draws[k].0)))
            .collect();
        let facts: Vec<(Why, Option<Facts<'_>>)> = offences
            .iter()
            .zip(ids)
            .map(|(o, program)| {
                (
                    o.why,
                    o.draw.map(|k| Facts {
                        draw: draws[k].0,
                        at: draws[k].1,
                        recorded: k,
                        model: &model.draws[k],
                        program,
                    }),
                )
            })
            .collect();
        let frame = FrameFacts {
            frame,
            recorded_draws: draws.len(),
            world_draws,
            overlay_draw: draws.get(world_draws).map(|d| d.1.draw),
        };
        self.holds.lock().unwrap().record(&frame, &facts);
    }

    /// The wrapped backend.
    pub fn inner(&self) -> &B {
        &self.inner
    }

    /// The wrapped backend.
    pub fn inner_mut(&mut self) -> &mut B {
        &mut self.inner
    }

    fn rate(&self) -> Rate {
        if self.failed {
            Rate::Off
        } else {
            Rate::from_setting((self.rate)())
        }
    }

    fn recording(&self, color: B::Surface) -> bool {
        self.shown.contains(&color) && self.rate() != Rate::Off
    }

    fn info(&self, s: B::Surface) -> Option<SurfaceInfo> {
        self.surfaces.iter().find(|(x, _)| *x == s).map(|(_, i)| *i)
    }

    /// Forgets the frames (their surfaces or textures are going away, or
    /// the replay is off).
    fn reset(&mut self) {
        self.prev = None;
        self.cur = None;
        self.ops.clear();
        self.unpinned = false;
        self.last = None;
    }

    /// `s` was written: copies taken of it are stale.
    fn wrote(&mut self, s: B::Surface) {
        if !self.snaps.is_empty() {
            self.snaps.retain(|(x, _)| *x != s);
        }
    }

    /// The texture a recorded draw keeps for `t`: `t` itself, or, when `t`
    /// is a view of a render target, a copy of the target as the draw
    /// sampled it (a replay later must not see what the guest drew into it
    /// since: a smaller pass, the next frame). Marks the frame unpinned
    /// when no copy can be made.
    fn pin(&mut self, t: &Arc<B::Texture>) -> Arc<B::Texture> {
        let Some(&(_, surface)) = self
            .live
            .iter()
            .find(|(w, _)| std::ptr::eq(w.as_ptr(), Arc::as_ptr(t)))
        else {
            return Arc::clone(t);
        };
        if let Some((_, c)) = self.snaps.iter().find(|(s, _)| *s == surface) {
            return Arc::clone(c);
        }
        match self.inner.snapshot_surface(surface, false) {
            Ok(Some(c)) => {
                let c = Arc::new(c);
                self.snaps.push((surface, Arc::clone(&c)));
                c
            }
            _ => {
                self.unpinned = true;
                Arc::clone(t)
            }
        }
    }

    /// The textures a recorded draw keeps ([`Self::pin`]).
    fn pinned(&mut self, textures: [Option<&Arc<B::Texture>>; 4]) -> [Option<Arc<B::Texture>>; 4] {
        let mut out: [Option<Arc<B::Texture>>; 4] = Default::default();
        for (o, t) in out.iter_mut().zip(textures) {
            *o = t.map(|t| self.pin(t));
        }
        out
    }

    /// Notes a texture that views render target `s`.
    fn note_live(&mut self, t: &Arc<B::Texture>, s: B::Surface) {
        self.live.retain(|(w, _)| w.strong_count() > 0);
        self.live.push((Arc::downgrade(t), s));
    }

    /// `d`'s vertex program's viewport transform, if E1 can follow it, and
    /// what its clip position comes from.
    fn program(&mut self, d: &ResolvedDraw) -> (Option<Epilogue>, Position, Option<PassThrough>) {
        match &d.shader.vertex {
            VertexProcessing::Program { microcode, .. } => {
                *self.programs.entry(microcode.clone()).or_insert_with(|| {
                    (
                        Epilogue::of(microcode),
                        position(microcode),
                        screen_space::pass_through(microcode),
                    )
                })
            }
            VertexProcessing::FixedFunction { .. } => (None, Position::Unknown, None),
        }
    }

    /// Whether the GPU is still on the last made frame.
    fn behind(&self) -> bool {
        self.fence.as_ref().is_some_and(|done| !done())
    }

    /// Whether made frame `made` shows at the size the guest's `real` does.
    fn same_extent(&self, made: B::Surface, real: B::Surface) -> bool {
        match (
            self.inner.surface_extent(made),
            self.inner.surface_extent(real),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }

    fn private_of(&mut self, s: B::Surface) -> Result<B::Surface, String> {
        self.copy_of(s, false)
    }

    /// `s`'s private copy, or with `world` its world-image copy (E1).
    fn copy_of(&mut self, s: B::Surface, world: bool) -> Result<B::Surface, String> {
        let list = if world {
            &self.world_private
        } else {
            &self.private
        };
        if let Some((_, p)) = list.iter().find(|(r, _)| *r == s) {
            return Ok(*p);
        }
        let i = self.info(s).ok_or_else(|| {
            format!("interpolation: surface {s:?} was not made through this backend")
        })?;
        let p = self
            .inner
            .create_surface(i.format, i.width, i.height, i.depth)?;
        if world {
            self.world_private.push((s, p));
        } else {
            self.private.push((s, p));
        }
        Ok(p)
    }

    /// The in-between frame at `alpha`, made the mode's way: returns the
    /// surface to show, what was substituted, and when the shown frame's
    /// own work began (after E1's world image, which is timed apart).
    fn make(
        &mut self,
        alpha: f32,
    ) -> Result<(B::Surface, Substituted, std::time::Instant), String> {
        match self.mode {
            Mode::Replay => {
                let t = std::time::Instant::now();
                let (s, st) = self.replay(alpha)?;
                Ok((s, st, t))
            }
            Mode::Reproject { .. } => {
                if !self.cur.as_ref().is_some_and(|c| c.captured) {
                    let t = std::time::Instant::now();
                    self.capture()?;
                    let ns = t.elapsed().as_nanos() as u64;
                    self.stats.lock().unwrap().time_capture(ns);
                }
                let t = std::time::Instant::now();
                Ok((self.warp(alpha)?, Substituted::default(), t))
            }
        }
    }

    /// Records the render-thread time of a frame made from `t` under the
    /// mode (its draws, and for in-between frames their present too).
    fn timed(&self, t: std::time::Instant) {
        let ns = t.elapsed().as_nanos() as u64;
        let mut st = self.stats.lock().unwrap();
        match self.mode {
            Mode::Replay => st.time_replay(ns),
            Mode::Reproject { .. } => st.time_warp(ns),
        }
    }

    /// E1: draws frame N's world (the ops before the overlay) into its
    /// world-image copies with overscan, once per guest frame.
    fn capture(&mut self) -> Result<(), String> {
        let cur = self.cur.as_ref().ok_or("interpolation: no frame")?;
        let end = cur.world_end;
        let mut targets = vec![cur.surface];
        targets.extend(depths_in(&cur.ops[..end]));
        let world_draws = draws_in(&cur.ops[..end]);
        let constants = constants_of(&cur.ops);
        let subs = reproject::overscan(
            &cur.epilogues,
            world_draws,
            reproject::overscan_scale(),
            &|i| constants[i].clone(),
        );
        drop(constants);
        let mut map = Vec::with_capacity(targets.len());
        for t in targets {
            map.push((t, self.copy_of(t, true)?));
        }
        let to = |s: B::Surface| map.iter().find(|(r, _)| *r == s).map_or(s, |(_, p)| *p);
        let cur = self.cur.as_mut().expect("checked above");
        let streams = std::mem::take(&mut cur.streams);
        let r = run_ops(
            &mut self.inner,
            &mut cur.ops[..end],
            0,
            &to,
            &|k| subs.get(k).cloned().flatten(),
            &|k| streams.get(k).and_then(|s| s.as_ref()).map(|s| s.0.clone()),
        );
        let (shrunk, extended) = streams.iter().flatten().fold((0, 0), |n, s| match s.1 {
            Fit::Shrink => (n.0 + 1, n.1),
            Fit::Extend => (n.0, n.1 + 1),
        });
        cur.streams = streams;
        r?;
        let mut st = self.stats.lock().unwrap();
        st.screen_shrunk += shrunk;
        st.screen_extended += extended;
        drop(st);
        cur.captured = true;
        Ok(())
    }

    /// E1: the world image turned to the display camera's rotation at
    /// `alpha`, the overlay drawn on top, into the private copy of the
    /// presented surface; returns that copy.
    fn warp(&mut self, alpha: f32) -> Result<B::Surface, String> {
        let ahead = matches!(self.mode, Mode::Reproject { ahead: true });
        let (Some(prev), Some(cur)) = (self.prev.as_ref(), self.cur.as_ref()) else {
            return Err("interpolation: no frames to warp".into());
        };
        let (Some(pc), Some(cc), Some(screen)) = (
            prev.model.cameras.first(),
            cur.model.cameras.first(),
            cur.screen,
        ) else {
            return Err("interpolation: no camera to warp".into());
        };
        let rotation =
            reproject::display_rotation(&pc.camera, &cc.camera, alpha, ahead, &self.tuning);
        let h = reproject::homography(&cc.camera, rotation, reproject::overscan_scale(), &screen)
            .ok_or("interpolation: the camera's projection does not invert")?;
        let (surface, end) = (cur.surface, cur.world_end);
        let first_overlay_draw = draws_in(&cur.ops[..end]);
        let depths = depths_in(&cur.ops[end..]);
        let world = self.copy_of(surface, true)?;
        let shown = self.private_of(surface)?;
        if !self.inner.warp_surface(world, shown, h)? {
            return Err("interpolation: the backend cannot warp surfaces".into());
        }
        let mut map = vec![(surface, shown)];
        for d in depths {
            let p = self.private_of(d)?;
            // The overlay draws over far depth: the world under it is a
            // picture now.
            self.inner.clear(
                shown,
                Some(p),
                Clear {
                    color: None,
                    depth: Some(1.0),
                    stencil: Some(0),
                },
            )?;
            map.push((d, p));
        }
        let to = |s: B::Surface| map.iter().find(|(r, _)| *r == s).map_or(s, |(_, p)| *p);
        let cur = self.cur.as_mut().expect("checked above");
        run_ops(
            &mut self.inner,
            &mut cur.ops[end..],
            first_overlay_draw,
            &to,
            &|_| None,
            &|_| None,
        )?;
        Ok(shown)
    }

    /// Draws the newest frame at `alpha` into the private copy of its
    /// surface and returns that copy.
    fn replay(&mut self, alpha: f32) -> Result<(B::Surface, Substituted), String> {
        let (Some(prev), Some(cur)) = (self.prev.as_ref(), self.cur.as_ref()) else {
            return Err("interpolation: no frames to replay".into());
        };
        let draws: Vec<&ResolvedDraw> = cur
            .ops
            .iter()
            .filter_map(|o| match o {
                Op::Draw { draw, .. } => Some(&**draw),
                _ => None,
            })
            .collect();
        let (subs, st) = substitute(
            &prev.model,
            &cur.model,
            &cur.matches,
            alpha,
            &self.tuning,
            &|i| draws[i].shader.constants.clone(),
        );
        drop(draws);
        // Private surfaces for every target the frame touches.
        let mut targets: Vec<B::Surface> = vec![cur.surface];
        for o in &cur.ops {
            match o {
                Op::Clear { depth: Some(d), .. }
                | Op::ClearRects { depth: Some(d), .. }
                | Op::Draw { depth: Some(d), .. } => {
                    if !targets.contains(d) {
                        targets.push(*d);
                    }
                }
                _ => {}
            }
        }
        let mut map = Vec::with_capacity(targets.len());
        for t in targets {
            map.push((t, self.private_of(t)?));
        }
        let to = |s: B::Surface| map.iter().find(|(r, _)| *r == s).map_or(s, |(_, p)| *p);
        let cur = self.cur.as_mut().expect("checked above");
        let surface = to(cur.surface);
        run_ops(
            &mut self.inner,
            &mut cur.ops,
            0,
            &to,
            &|k| subs.get(k).cloned().flatten(),
            &|_| None,
        )?;
        Ok((surface, st))
    }

    /// The guest's present: closes the frame being recorded and shows it,
    /// blended from the one before when it can.
    fn end_frame(
        &mut self,
        surface: B::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        let rate = self.rate();
        let now = (self.clock)();
        let at = self.stamp.take().unwrap_or(now);
        // A new guest frame: its draws and passes count from zero.
        self.frame_draws = 0;
        self.pass = None;
        let Some(period) = rate.period_ns() else {
            if self.cur.is_some() || !self.ops.is_empty() {
                self.reset();
            }
            if !self.shown.contains(&surface) {
                self.shown.push(surface);
            }
            return self.inner.present_with_gamma(surface, flags, gamma);
        };
        self.gamma = gamma.map(|g| Box::new(*g));
        let ops: Vec<Op<B>> = std::mem::take(&mut self.ops)
            .into_iter()
            .filter(|o| o.targets(surface))
            .collect();
        // A surface copied in is read when the frame is made again, by then
        // with whatever the guest drew into it since.
        let unpinned =
            std::mem::take(&mut self.unpinned) || ops.iter().any(|o| matches!(o, Op::Copy { .. }));
        if !self.shown.contains(&surface) {
            self.shown.push(surface);
        }
        let draws: Vec<&ResolvedDraw> = ops
            .iter()
            .filter_map(|o| match o {
                Op::Draw { draw, .. } => Some(&**draw),
                _ => None,
            })
            .collect();
        let programs: Vec<(Option<Epilogue>, Position, Option<PassThrough>)> =
            draws.iter().map(|d| self.program(d)).collect();
        let positions: Vec<Position> = programs.iter().map(|p| p.1).collect();
        let model = FrameModel::build(&draws, &positions);
        drop(draws);
        // The pair to blend: only consecutive frames close together, with
        // the same views (a split-screen change, a load or a pause starts
        // over).
        let paired = match &self.cur {
            Some(c) if !blendable(c.at, at) => Err(Unpaired::Gap),
            None => Err(Unpaired::Gap),
            Some(_) if model.cameras.is_empty() => Err(Unpaired::Camera),
            Some(c)
                if c.model.cameras.len() != model.cameras.len()
                    || c.model
                        .cameras
                        .iter()
                        .zip(&model.cameras)
                        .any(|(a, b)| a.view != b.view) =>
            {
                Err(Unpaired::Views)
            }
            Some(_) => Ok(()),
        };
        let keep = self.cur.take().filter(|_| paired.is_ok());
        let matches = keep
            .as_ref()
            .map(|p| match_draws(&p.model, &model))
            .unwrap_or_default();
        // E1: where the overlay starts (as an op index), each draw's
        // viewport transform, and the screen mapping the warp goes through.
        let overlay = reproject::overlay_start(&model);
        let reproject = self.mode != Mode::Replay;
        let epilogues: Vec<Option<Epilogue>> = if reproject {
            programs.iter().map(|p| p.0).collect()
        } else {
            Vec::new()
        };
        let mut world_end = ops.len();
        let mut k = 0;
        for (i, o) in ops.iter().enumerate() {
            if let Op::Draw { .. } = o {
                if k == overlay && world_end == ops.len() {
                    world_end = i;
                }
                k += 1;
            }
        }
        let pass_throughs: Vec<Option<PassThrough>> = programs.iter().map(|p| p.2).collect();
        let mut screen = None;
        let mut streams = Vec::new();
        let hold = if unpinned {
            Some(Hold::Unsure(Unsure::Unpinned))
        } else if !model.cameras.iter().all(|c| c.camera.centred()) {
            Some(Hold::Unsure(Unsure::Camera))
        } else if reproject {
            match world_screen(&ops, world_end, &model, &epilogues, &pass_throughs) {
                Ok((s, subs)) => {
                    screen = Some(s);
                    streams = subs;
                    None
                }
                Err(offences) => {
                    // A frame with no camera is held as unpaired, with
                    // nothing to warp: not logged.
                    if !model.cameras.is_empty() {
                        let frame = self.stats.lock().unwrap().frames + 1;
                        self.note_held(frame, &ops, &model, world_end, &offences);
                    }
                    Some(Hold::SizeMismatch(offences[0].why.mismatch()))
                }
            }
        } else {
            None
        };
        let id = self.next_id;
        self.next_id += 1;
        self.prev = keep;
        self.cur = Some(Frame {
            id,
            surface,
            at,
            ops,
            model,
            matches,
            world_end,
            screen,
            epilogues,
            captured: false,
            streams,
            hold,
        });
        let mut st = self.stats.lock().unwrap();
        st.frames += 1;
        drop(st);
        let mode = self.mode;
        let c = self.cur.as_ref().expect("set above");
        let unpaired = match paired {
            Err(u) => Some(u),
            Ok(()) if self.counting => Some(Unpaired::Counting),
            Ok(()) if !c.model.has_matrices() => Some(Unpaired::Camera),
            // E1 is for a single gameplay view with a world.
            Ok(()) if mode != Mode::Replay && (c.model.cameras.len() != 1 || c.world_end == 0) => {
                Some(Unpaired::Views)
            }
            Ok(()) => None,
        };
        let mut blend = unpaired.is_none();
        if let Some(u) = unpaired {
            self.stats.lock().unwrap().held(Hold::Unpaired(u));
        }
        if blend {
            let mut st = self.stats.lock().unwrap();
            match hold {
                Some(why) => st.held(why),
                None if self.behind() => st.dropped += 1,
                None => {}
            }
            blend = hold.is_none() && !self.behind();
        }
        if blend {
            let a = alpha(now, self.prev.as_ref().unwrap().at, at);
            match self.make(a) {
                Ok((p, _, _)) if !self.same_extent(p, surface) => {
                    let mut st = self.stats.lock().unwrap();
                    st.held(Hold::SizeMismatch(Mismatch::Extent));
                }
                Ok((p, s, t)) => {
                    // The present is the guest's own (its frame boundary
                    // and any wait): not the replay's cost.
                    self.timed(t);
                    let mut st = self.stats.lock().unwrap();
                    st.blended_frames += 1;
                    st.substituted_draws += s.draws as u64;
                    st.snapped += s.snapped as u64;
                    drop(st);
                    self.last = Some((id, a));
                    self.schedule.shown_at(now, period);
                    self.inner.present_with_gamma(p, flags, gamma)?;
                    self.fence = self.inner.gpu_fence();
                    return Ok(());
                }
                Err(e) => self.fail(e),
            }
        }
        self.stats.lock().unwrap().held_frames += 1;
        self.last = Some((id, 1.0));
        self.inner.present_with_gamma(surface, flags, gamma)
    }

    /// Turns the replay off for the rest of the run: a replay error is a
    /// host bug, and the guest's frames go on without it.
    fn fail(&mut self, e: String) {
        eprintln!("d3d: interpolation off for the rest of the run: {e}");
        self.failed = true;
        self.reset();
    }

    /// The idle step at `now` (ns): an in-between frame if one is due.
    /// Returns when it wants to be called again (ns), if at all.
    pub fn idle_at(&mut self, now: u64) -> Option<u64> {
        let period = self.rate().period_ns()?;
        let (prev_at, cur_id, cur_at) = match (&self.prev, &self.cur) {
            (Some(p), Some(c)) if c.hold.is_none() => (p.at, c.id, c.at),
            _ => return None,
        };
        if self.last.is_some_and(|(id, a)| id == cur_id && a >= 1.0) {
            return None;
        }
        if self.counting {
            self.stats.lock().unwrap().deferred += 1;
            return Some(now + 1_000_000);
        }
        if !self.schedule.is_due(now) {
            return self.schedule.deadline();
        }
        if let Some(wait) = defer(now, prev_at, cur_at, period) {
            return Some(wait);
        }
        self.schedule.due(now, period);
        if self.behind() {
            // The GPU is still on the last made frame: skip this slot.
            self.stats.lock().unwrap().dropped += 1;
            return self.schedule.deadline();
        }
        let a = alpha(now, prev_at, cur_at);
        let real = self.cur.as_ref().map(|c| c.surface);
        match self.make(a) {
            Ok((p, _, _)) if real.is_some_and(|r| !self.same_extent(p, r)) => {
                let mut st = self.stats.lock().unwrap();
                st.held(Hold::SizeMismatch(Mismatch::Extent));
                drop(st);
                self.last = Some((cur_id, 1.0));
                None
            }
            Ok((p, s, t)) => {
                let gamma = self.gamma.take();
                let r = self.inner.present_replay(p, gamma.as_deref());
                self.gamma = gamma;
                self.timed(t);
                if let Err(e) = r {
                    self.fail(e);
                    return None;
                }
                self.fence = self.inner.gpu_fence();
                let mut st = self.stats.lock().unwrap();
                st.replays += 1;
                st.substituted_draws += s.draws as u64;
                drop(st);
                self.last = Some((cur_id, a));
                if a >= 1.0 {
                    None
                } else {
                    self.schedule.deadline()
                }
            }
            Err(e) => {
                self.fail(e);
                None
            }
        }
    }
}

impl<B: Backend> InterpBackend<B> {
    /// Prints the counters so far once per `every` (a run ended by a signal
    /// never reaches [`Drop`]).
    fn report_every(&mut self, every: std::time::Duration) {
        if self.reported.elapsed() < every {
            return;
        }
        self.reported = std::time::Instant::now();
        let s = *self.stats.lock().unwrap();
        if s.frames > 0 {
            eprintln!("d3d: interpolation so far: {s}");
        }
    }
}

impl<B: Backend> Drop for InterpBackend<B> {
    fn drop(&mut self) {
        let s = *self.stats.lock().unwrap();
        if s.frames > 0 {
            eprintln!("d3d: interpolation: {s}");
        }
    }
}

impl<B: Backend> Backend for InterpBackend<B>
where
    B::Texture: Sync,
{
    type Surface = B::Surface;
    type Texture = Arc<B::Texture>;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String> {
        let s = self.inner.create_surface(format, width, height, depth)?;
        self.surfaces.push((
            s,
            SurfaceInfo {
                format,
                width,
                height,
                depth,
            },
        ));
        Ok(s)
    }

    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        let in_frames = [&self.prev, &self.cur].iter().any(|f| {
            f.as_ref().is_some_and(|f| {
                f.surface == surface
                    || f.ops.iter().any(|o| match o {
                        Op::Clear { depth, .. }
                        | Op::ClearRects { depth, .. }
                        | Op::Draw { depth, .. } => *depth == Some(surface),
                        Op::Copy { src, .. } => *src == surface,
                    })
            })
        });
        if in_frames {
            self.reset();
        }
        self.ops.retain(|o| match o {
            Op::Clear { depth, .. } | Op::ClearRects { depth, .. } | Op::Draw { depth, .. } => {
                *depth != Some(surface)
            }
            Op::Copy { src, .. } => *src != surface,
        });
        self.shown.retain(|s| *s != surface);
        self.surfaces.retain(|(s, _)| *s != surface);
        self.wrote(surface);
        self.live.retain(|(_, s)| *s != surface);
        if let Some(i) = self.private.iter().position(|(r, _)| *r == surface) {
            let (_, p) = self.private.remove(i);
            self.inner.destroy_surface(p)?;
        }
        if let Some(i) = self.world_private.iter().position(|(r, _)| *r == surface) {
            let (_, p) = self.world_private.remove(i);
            self.inner.destroy_surface(p)?;
        }
        self.inner.destroy_surface(surface)
    }

    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        self.inner.upload_texture(texture).map(Arc::new)
    }

    fn bc_supported(&self) -> bool {
        self.inner.bc_supported()
    }

    fn clear(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: Clear,
    ) -> Result<(), String> {
        self.enter(color, depth);
        self.wrote(color);
        if let Some(d) = depth {
            self.wrote(d);
        }
        if self.recording(color) {
            self.ops.push(Op::Clear {
                color,
                depth,
                clear,
            });
        }
        self.inner.clear(color, depth, clear)
    }

    fn draw(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: &ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String> {
        let tex: [Option<&B::Texture>; 4] = std::array::from_fn(|i| textures[i].map(|t| &**t));
        let r = self.inner.draw(color, depth, draw, tex)?;
        let at = self.draw_at(color, depth);
        self.wrote(color);
        if let Some(d) = depth {
            self.wrote(d);
        }
        if r == DrawOutcome::Drawn && self.recording(color) {
            let textures = self.pinned(textures);
            self.ops.push(Op::Draw {
                color,
                depth,
                draw: Box::new(draw.clone()),
                textures,
                at,
            });
        }
        Ok(r)
    }

    fn draw_owned(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        let at = self.draw_at(color, depth);
        self.wrote(color);
        if let Some(d) = depth {
            self.wrote(d);
        }
        if !self.recording(color) {
            let tex: [Option<&B::Texture>; 4] = std::array::from_fn(|i| textures[i].map(|t| &**t));
            return self.inner.draw_owned(color, depth, draw, tex);
        }
        let tex: [Option<&B::Texture>; 4] = std::array::from_fn(|i| textures[i].map(|t| &**t));
        let r = self.inner.draw(color, depth, &draw, tex);
        if r == Ok(DrawOutcome::Drawn) {
            let textures = self.pinned(textures);
            self.ops.push(Op::Draw {
                color,
                depth,
                draw: Box::new(draw),
                textures,
                at,
            });
        }
        (r, Vec::new())
    }

    fn frame_stats(&mut self) {
        self.inner.frame_stats()
    }

    fn stamp_present(&mut self, at: std::time::Instant) {
        self.stamp = Some(Self::wall_ns(self.base, at));
    }

    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String> {
        self.report_every(std::time::Duration::from_secs(5));
        self.end_frame(surface, flags, None)
    }

    fn present_with_gamma(
        &mut self,
        surface: Self::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.report_every(std::time::Duration::from_secs(5));
        self.end_frame(surface, flags, gamma)
    }

    fn present_replay(
        &mut self,
        surface: Self::Surface,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.inner.present_replay(surface, gamma)
    }

    fn warp_surface(
        &mut self,
        src: Self::Surface,
        dst: Self::Surface,
        h: [[f32; 3]; 3],
    ) -> Result<bool, String> {
        self.inner.warp_surface(src, dst, h)
    }

    fn idle(&mut self) -> Option<std::time::Instant> {
        let now = (self.clock)();
        self.idle_at(now)
            .map(|ns| self.base + std::time::Duration::from_nanos(ns))
    }

    fn clear_rects(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        let r = self.inner.clear_rects(color, depth, clear)?;
        self.enter(color, depth);
        self.wrote(color);
        if let Some(d) = depth {
            self.wrote(d);
        }
        if r == DrawOutcome::Drawn && self.recording(color) {
            self.ops.push(Op::ClearRects {
                color,
                depth,
                clear: clear.clone(),
            });
        }
        Ok(r)
    }

    fn read_surface(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        self.inner
            .read_surface(surface, format, width, height, layout)
    }

    fn read_depth(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.inner
            .read_depth(surface, format, width, height, layout)
    }

    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        let r = self.inner.copy_surface(src, dst)?;
        self.wrote(dst);
        if r && self.recording(dst) {
            self.ops.push(Op::Copy { src, dst });
        }
        Ok(r)
    }

    fn write_surface(
        &mut self,
        surface: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.wrote(surface);
        self.inner.write_surface(surface, desc, bytes)
    }

    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        Ok(self.inner.snapshot_surface(surface, opaque)?.map(Arc::new))
    }

    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.counting = true;
        self.inner.clear_pixel_count()
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.counting = false;
        self.inner.report_pixel_count()
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.inner.read_pixel_count(ticket)
    }

    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        self.inner.try_pixel_count(ticket)
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        self.inner.drop_pixel_count(ticket)
    }

    fn pixel_count_unsubmitted(&self, ticket: u64) -> bool {
        self.inner.pixel_count_unsubmitted(ticket)
    }

    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<Self::Surface, String> {
        let s = self.inner.create_texture_target(format, width, height)?;
        self.surfaces.push((
            s,
            SurfaceInfo {
                format,
                width,
                height,
                depth: false,
            },
        ));
        Ok(s)
    }

    fn can_sample_surfaces(&self) -> bool {
        self.inner.can_sample_surfaces()
    }

    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        let t = self.inner.surface_texture(surface)?.map(Arc::new);
        if let Some(t) = &t {
            self.note_live(t, surface);
        }
        Ok(t)
    }

    fn can_sample_depth_surfaces(&self) -> bool {
        self.inner.can_sample_depth_surfaces()
    }

    fn request_depth_grid(
        &mut self,
        surface: Self::Surface,
        request: crate::backend::DepthGridRequest,
        sink: crate::backend::DepthGridSink,
    ) -> Result<bool, String> {
        // Only the guest's own frames ask (the device, between its draws);
        // a replay is host-only and never produces a grid.
        self.inner.request_depth_grid(surface, request, sink)
    }

    fn external_attachments(
        &mut self, surfaces: crate::ExternalSurfaces<Self::Surface>, textures: [Option<&Self::Texture>; 4],
        copy: crate::TargetCopy,
    ) -> Result<crate::ExternalCopies, String> {
        if self.rate() != Rate::Off { return Ok(crate::ExternalCopies::Missing(copy)); }
        self.inner.external_attachments(surfaces, textures.map(|t| t.map(Arc::as_ref)), copy)
    }

    fn prepare_external(
        &mut self, surfaces: crate::ExternalSurfaces<Self::Surface>, request: crate::ExternalPassRequest,
        textures: [Option<&Self::Texture>; 4], plan: &crate::ExternalPlan, copy: crate::TargetCopy, pass: crate::SharedReplacement,
    ) -> Result<Option<Vec<crate::ExternalRecording>>, String> {
        if self.rate() != Rate::Off {
            // Replay needs the original geometry to rebuild an intermediate view.
            let items = pass.lock().map_err(|_| "native pass poisoned")?.items(plan.ranges) as u64;
            plan.stage.count(timeline::native::Metric::Examined, items);
            plan.stage.count(timeline::native::Metric::RefusedReplay, items);
            return Ok(None);
        }
        self.inner.prepare_external(surfaces, request, textures.map(|t| t.map(Arc::as_ref)), plan, copy, pass)
    }

    fn commit_external(&mut self, recording: crate::ExternalRecording) {
        self.unpinned = true;
        self.inner.commit_external(recording);
    }

    fn supports_external_pass(&self) -> bool {
        self.inner.supports_external_pass()
    }

    fn external_pass(
        &mut self,
        color: Self::Surface,
        depth: Self::Surface,
        sampled: Vec<Option<Self::Surface>>,
        request: crate::backend::ExternalPassRequest,
        pass: Box<dyn crate::backend::ExternalPass>,
    ) -> Result<bool, String> {
        // A replay redraws recorded draws, not passes: until it can redraw
        // this one with its warped camera, the frame is held, not blended.
        self.unpinned = true;
        self.inner.external_pass(color, depth, sampled, request, pass)
    }

    fn depth_level_texture(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<Self::Texture>, String> {
        // The level's texels as of now, made by the backend (a copy, not a
        // view of the surface: `Backend::depth_level_texture`), so a draw
        // that samples it replays as drawn.
        Ok(self.inner.depth_level_texture(surface)?.map(Arc::new))
    }

    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        self.inner.judge()
    }

    fn defers_depth_aliases(&self) -> bool {
        self.inner.defers_depth_aliases()
    }

    fn depth_alias_texture(
        &mut self,
        depth: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: &TextureDesc,
        options: gfx::texture::DecodeOptions,
    ) -> Result<Option<Self::Texture>, String> {
        Ok(self
            .inner
            .depth_alias_texture(
                depth, format, width, height, pitch, generation, offset, desc, options,
            )?
            .map(Arc::new))
    }

    fn can_mirror(&self) -> bool {
        self.inner.can_mirror()
    }

    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        self.inner.mirror_reserve(len)
    }

    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        self.inner.mirror_write(offset, bytes)
    }

    fn mirror_store(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.inner
            .mirror_store(surface, format, width, height, layout, offset)
    }

    fn mirror_load(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.wrote(surface);
        self.inner
            .mirror_load(surface, format, width, height, layout, offset)
    }

    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.inner.mirror_read(offset, len)
    }
}
