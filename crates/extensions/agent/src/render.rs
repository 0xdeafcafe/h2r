//! `render.frame`, `render.frame_layout` and `render.stats` (docs/runtime/agent-api.md, phase 5).
//!
//! [`FrameProbe`] records one whole guest frame when armed: the backend
//! wrapper in front of the D3D backend (boot's `d3d_diag`) tells it each
//! clear, draw, copy, readback and present, as plain text and numbers, so
//! this crate needs no `d3d` types. Unarmed, each call site costs one
//! relaxed load ([`FrameProbe::armed`]).
//!
//! A frame is everything between two presents: arming waits for the next
//! present, records until the one after, then disarms. Work on one colour
//! target (and depth) in a row is a pass; going back to a target makes a
//! new pass.
//!
//! The FrameScene methods (docs/graphics/framescene.md, sections 9 and 10):
//!
//! - `render.compare`: two PNG files compared ([`crate::compare`]).
//! - `render.capture`: the frame of a sim tick as a PNG. [`FrameCapture`]
//!   ([`CAPTURE`]) is told each tick that ends and each guest present
//!   (one relaxed load each when nothing is asked); the present after the
//!   tick asked for is the frame, which the host's frame writer (boot's
//!   `H2_BOOT_GFX` present hook) writes, or which [`FrameProbe`] reads a
//!   pass of.
//! - `render.scene`: the host's [`SceneSource`] ([`set_scene_source`]).
//! - `render.parity`: compares one presented reference frame to a private
//!   rendering of its sealed scene, through the host's [`ParitySource`].
//!
//! `render.picture.*`: the presented frames, live, in a shared-memory ring
//! or a JPEG stream ([`picture`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent_proto::{
    CaptureCamera, CaptureReply, CaptureSource, CompareParams, DepthRecord, DrawLayout, DrawRecord,
    FrameCaptureParams, FrameParams, FrameRecord, FrameReply, OpRecord, ParityParams,
    PassRecord, RenderStats, RpcError, SceneParams, SceneReply, SnapRecord, code, method,
};
use live_proto::maps::{MapId, Sha256};
use serde_json::{Map, Value};

use crate::exec::{Access, Agent, DEFER_TICKS_MAX};

pub mod clip;
pub mod picture;
pub mod private_graph;

/// One draw, as the backend wrapper describes it.
#[derive(Debug, Clone, Default)]
pub struct DrawInfo {
    pub color: String,
    pub depth: Option<String>,
    /// `640x480 X8R8G8B8`.
    pub target: String,
    pub entry: String,
    pub primitive: String,
    pub vertices: u32,
    pub indices: u32,
    /// `drawn`, or why not.
    pub outcome: String,
    /// Each bound texture's format.
    pub textures: Vec<String>,
    pub program: bool,
    pub blend: bool,
    pub depth_write: bool,
    /// The vertex side, as [`DrawRecord::vs`].
    pub vs: String,
    /// How the draw tests and stores depth, as [`DrawRecord::depth`]
    /// (`depth` here is the depth surface).
    pub depth_state: Option<DepthRecord>,
    /// The draw's constant registers, hardware order (D3D register + 96),
    /// shared with the backend: the probe copies out the ones asked for.
    pub constants: Option<Arc<[[f32; 4]]>>,
    /// Geometry supplied by the host for a layout frame.
    pub layout: Option<DrawLayout>,
}

/// What the wrapper should do after telling the probe about a draw or a
/// clear: read back the pass that just ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapAsk {
    /// A retry of the same arming after its original display disappeared.
    attempt: u64,
    /// The arming's number, for the file name.
    pub seq: u64,
    pub pass: usize,
    /// Read the display raster for a layout capture.
    pub display: bool,
}

/// The recorder. One per process ([`PROBE`]); tests make their own.
pub struct FrameProbe {
    armed: AtomicBool,
    state: Mutex<ProbeState>,
}

struct ProbeState {
    attempt: u64,
    next_seq: u64,
    stage: Stage,
    last: Option<FrameRecord>,
    presents: u64,
}

enum Stage {
    Idle,
    Waiting(Arming),
    Recording(Box<Recording>),
    Graph(Arming),
    /// The original draw report, pending the worker's choice for this device Swap.
    Routing {
        swap: u64,
        arming: Arming,
        original: Box<FrameRecord>,
        original_files: Vec<private_capture::Written>,
    },
}

#[derive(Clone)]
struct Arming {
    attempt: u64,
    seq: u64,
    at_present: u64,
    draws: bool,
    layout: bool,
    max_draws: usize,
    snap: BTreeSet<usize>,
    constants: Vec<i32>,
}

struct Recording {
    arming: Arming,
    // Original snapshots become public with their report. A rearm or
    // a native presentation discards any files still held by this route.
    files: Vec<private_capture::Written>,
    began: Instant,
    rec: FrameRecord,
    targets: BTreeSet<String>,
    /// The current pass's colour and depth.
    key: Option<(String, Option<String>)>,
}

/// The game's probe.
pub static PROBE: FrameProbe = FrameProbe::new();

impl Default for FrameProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameProbe {
    pub const fn new() -> FrameProbe {
        FrameProbe {
            armed: AtomicBool::new(false),
            state: Mutex::new(ProbeState {
                attempt: 0,
                next_seq: 1,
                stage: Stage::Idle,
                last: None,
                presents: 0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ProbeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a frame is wanted: the one check on the draw path.
    #[inline]
    pub fn armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Arms for the next whole frame; forgets the last one.
    pub fn arm(&self, p: &FrameParams) -> Result<FrameReply, String> {
        self.arm_frame(p, false)
    }

    /// Arms for display layout, including draws and display snapshots.
    pub fn arm_layout(&self, p: &FrameParams) -> Result<FrameReply, String> {
        self.arm_frame(p, true)
    }

    fn arm_frame(&self, p: &FrameParams, layout: bool) -> Result<FrameReply, String> {
        let draws = p.draws || layout;
        let max_draws = p.max_draws.unwrap_or(FrameParams::DEFAULT_DRAWS);
        if max_draws > FrameParams::MAX_DRAWS {
            return Err(format!("max_draws is at most {}", FrameParams::MAX_DRAWS));
        }
        if p.snap.len() > FrameParams::MAX_SNAPS {
            return Err(format!("snap at most {} passes", FrameParams::MAX_SNAPS));
        }
        if p.constants.len() > FrameParams::MAX_CONSTANTS {
            return Err(format!(
                "constants: at most {} registers",
                FrameParams::MAX_CONSTANTS
            ));
        }
        if let Some(r) = p
            .constants
            .iter()
            .find(|r| !FrameParams::CONSTANT_REGISTERS.contains(r))
        {
            return Err(format!("constants: c{r} is not a register (-96..=95)"));
        }
        if !p.constants.is_empty() && !draws {
            return Err("constants go with draws".into());
        }
        let mut s = self.lock();
        let seq = s.next_seq;
        s.next_seq += 1;
        let at_present = s.presents;
        s.attempt = 0;
        let old = std::mem::replace(&mut s.stage, Stage::Waiting(Arming {
            attempt: 0,
            seq,
            at_present,
            draws,
            layout,
            max_draws,
            snap: p.snap.iter().copied().collect(),
            constants: p.constants.clone(),
        }));
        s.last = None;
        self.armed.store(true, Ordering::Relaxed);
        let answer = reply(&s);
        drop(s);
        drop(old);
        Ok(answer)
    }

    /// A superseded pass capture owns only its own display arming. An
    /// independent render.frame request may already have replaced it.
    fn cancel_capture_pass(&self, job: Option<&CaptureJob>) {
        let Some(seq) = job.and_then(|j| j.probe_seq) else { return };
        let mut s = self.lock();
        if s.stage.arming().is_some_and(|a| a.seq == seq) {
            let old = std::mem::replace(&mut s.stage, Stage::Idle);
            self.armed.store(false, Ordering::Relaxed);
            drop(s);
            drop(old);
        }
    }

    /// The last frame, and whether one is still to come.
    pub fn reply(&self) -> FrameReply {
        reply(&self.lock())
    }

    fn recording<R>(&self, f: impl FnOnce(&mut Recording) -> R) -> Option<R> {
        if !self.armed() {
            return None;
        }
        match &mut self.lock().stage {
            Stage::Recording(r) => Some(f(r)),
            _ => None,
        }
    }

    /// Whether the frame currently being recorded requests display layout.
    pub fn layout_requested(&self) -> bool {
        self.recording(|r| r.arming.layout).unwrap_or(false)
    }

    /// Work is about to go to `color` (and `depth`): when that ends a pass
    /// that wants a snapshot, read the pass's target back before the work
    /// goes on. [`clear`](Self::clear) and [`draw`](Self::draw) do this too;
    /// a wrapper that snapshots calls it first.
    pub fn enter(&self, color: &str, depth: Option<&str>) -> Option<SnapAsk> {
        self.recording(|r| r.enter(color, depth)).flatten()
    }

    /// A clear, whole-target or by rectangles.
    pub fn clear(&self, color: &str, depth: Option<&str>) -> Option<SnapAsk> {
        self.recording(|r| {
            let ask = r.enter(color, depth);
            let p = r.pass();
            p.clears += 1;
            r.rec.totals.clears += 1;
            ask
        })
        .flatten()
    }

    pub fn draw(&self, d: DrawInfo) -> Option<SnapAsk> {
        self.recording(|r| {
            let ask = r.enter(&d.color, d.depth.as_deref());
            r.draw(d);
            ask
        })
        .flatten()
    }

    /// A surface copy or readback (`op`: `copy`, `read`, `read_depth`).
    pub fn op(&self, op: &str, surface: &str, detail: String) {
        self.recording(|r| {
            match op {
                "copy" => r.rec.totals.copies += 1,
                _ => r.rec.totals.reads += 1,
            }
            let after_draws = r.rec.totals.draws;
            r.rec.ops.push(OpRecord {
                op: op.into(),
                surface: surface.into(),
                detail,
                after_draws,
            });
        });
    }

    /// A guest present, numbered `n` (the run's count after it). Returns
    /// the last pass when it wants a snapshot: read it back before the
    /// present goes on.
    pub fn present(&self, n: u64) -> Option<SnapAsk> {
        self.present_routed(n, None)
    }

    /// On an owned device the renderer selects the displayed graph or original
    /// commands on its own thread. Keep the original report until that decision.
    /// `swap` is device-local, while `n` numbers the run's presents on the wire.
    pub fn present_routed(&self, n: u64, swap: Option<u64>) -> Option<SnapAsk> {
        let mut s = self.lock();
        s.presents = n;
        if !self.armed() {
            return None;
        }
        match std::mem::replace(&mut s.stage, Stage::Idle) {
            Stage::Idle => None,
            pending @ (Stage::Graph(_) | Stage::Routing { .. }) => {
                s.stage = pending;
                None
            }
            Stage::Waiting(arming) => {
                s.stage = Stage::Recording(Box::new(Recording {
                    rec: FrameRecord {
                        seq: arming.seq,
                        from_present: n,
                        ..Default::default()
                    },
                    arming,
                    files: Vec::new(),
                    began: Instant::now(),
                    targets: BTreeSet::new(),
                    key: None,
                }));
                None
            }
            Stage::Recording(mut r) => {
                let ask = r
                    .key
                    .as_ref()
                    .and_then(|_| r.wants_snap(r.rec.passes.len() - 1));
                r.rec.present = n;
                r.rec.ms = (r.began.elapsed().as_secs_f64() * 1e5).round() / 100.0;
                r.rec.totals.passes = r.rec.passes.len();
                r.rec.totals.targets = r.targets.len();
                if let Some(swap) = swap {
                    s.stage = Stage::Routing {
                        swap, arming: r.arming, original: Box::new(r.rec), original_files: r.files,
                    };
                } else {
                    for file in r.files { file.retain(); }
                    s.last = Some(r.rec);
                    if ask.is_none() {
                        self.armed.store(false, Ordering::Relaxed);
                    }
                }
                ask
            }
        }
    }

    /// Where a snapshot the probe asked for went, or why it failed. The
    /// frame may be done already (a snapshot asked for at its present).
    pub fn snapped(&self, pass: usize, result: Result<String, String>) {
        self.note_snap(None, pass, result, None);
    }

    /// A callback can arrive after a second arming. Only the request which
    /// paid for this readback may consume it.
    pub fn snapped_for(&self, ask: SnapAsk, result: Result<String, String>) {
        self.note_snap(Some(ask), ask.pass, result, None);
    }

    /// Write the original pass for this arming. A stale request skips the
    /// read, and a replacement during readback or PNG encoding discards the
    /// file. The display worker may still choose a different graph.
    pub fn write_snapshot(&self, ask: SnapAsk, read: impl FnOnce() -> Result<crate::compare::Rgba, String>) {
        self.write_snapshot_in(ask, &agent_proto::cache_dir().join("frames"), read);
    }

    fn write_snapshot_in(&self, ask: SnapAsk, dir: &Path, read: impl FnOnce() -> Result<crate::compare::Rgba, String>) {
        let wanted = {
            let s = self.lock();
            s.attempt == ask.attempt && match &s.stage {
                Stage::Recording(r) => r.arming.seq == ask.seq,
                Stage::Routing { original, .. } => original.seq == ask.seq,
                Stage::Idle => s.last.as_ref().is_some_and(|f| f.seq == ask.seq && f.display_frame.is_none()),
                _ => false,
            }
        };
        if !wanted { return; }
        let written = read().and_then(|image| {
            let retry = retry_suffix(ask.attempt);
            let name = format!("frame-{}-{}{retry}-pass-{}.png", std::process::id(), ask.seq, ask.pass);
            let file = private_capture::Written::new(dir.join(name));
            crate::compare::write_png(file.path(), &image).map_err(|e| e.to_string())?;
            Ok(file)
        });
        match written {
            Ok(file) => self.note_snap(Some(ask), ask.pass, Ok(file.path().display().to_string()), Some(file)),
            Err(error) => self.snapped_for(ask, Err(error)),
        }
    }

    fn note_snap(&self, ask: Option<SnapAsk>, pass: usize, result: Result<String, String>, file: Option<private_capture::Written>) {
        let mut s = self.lock();
        let attempt = s.attempt;
        let belongs = |actual| ask.is_none_or(|wanted| wanted.seq == actual && wanted.attempt == attempt);
        let snap = match result {
            Ok(file) => SnapRecord {
                pass,
                file: Some(file),
                error: None,
            },
            Err(e) => SnapRecord {
                pass,
                file: None,
                error: Some(e),
            },
        };
        match &mut s.stage {
            Stage::Recording(r) if belongs(r.arming.seq) => {
                r.rec.snaps.push(snap);
                r.files.extend(file);
            }
            Stage::Routing { original, original_files, .. } if belongs(original.seq) => {
                original.snaps.push(snap);
                original_files.extend(file);
            }
            Stage::Idle => {
                if let Some(last) = &mut s.last && belongs(last.seq) && last.display_frame.is_none() {
                    last.snaps.push(snap);
                    if let Some(file) = file { file.retain(); }
                    self.armed.store(false, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }
}

impl Recording {
    fn draw(&mut self, d: DrawInfo) {
        self.targets.insert(d.color.clone());
        let index = self.rec.totals.draws;
        let pass_index = self.rec.passes.len() - 1;
        let p = self.pass();
        p.target.get_or_insert_with(|| d.target.clone());
        p.draws += 1;
        p.vertices += u64::from(d.vertices);
        p.first_draw.get_or_insert(index);
        *p.outcomes.entry(d.outcome.clone()).or_default() += 1;
        *p.primitives.entry(d.primitive.clone()).or_default() += 1;
        *p.entries.entry(d.entry.clone()).or_default() += 1;
        for t in &d.textures {
            *p.textures.entry(t.clone()).or_default() += 1;
        }
        p.programs += u64::from(d.program);
        p.blended += u64::from(d.blend);
        p.depth_writes += u64::from(d.depth_write);
        let t = &mut self.rec.totals;
        t.draws += 1;
        t.vertices += u64::from(d.vertices);
        *t.outcomes.entry(d.outcome.clone()).or_default() += 1;
        if self.arming.draws {
            if self.rec.draws.len() < self.arming.max_draws {
                let constants = picked_constants(&self.arming.constants, d.constants.as_deref());
                self.rec.draws.push(DrawRecord {
                    index,
                    pass: pass_index,
                    entry: d.entry,
                    primitive: d.primitive,
                    vertices: d.vertices,
                    indices: d.indices,
                    outcome: d.outcome,
                    textures: d.textures,
                    program: d.program,
                    blend: d.blend,
                    depth_write: d.depth_write,
                    vs: d.vs,
                    depth: d.depth_state,
                    constants,
                    layout: if self.arming.layout { d.layout } else { None },
                });
            } else {
                self.rec.truncated = true;
            }
        }
    }

    /// Starts a new pass unless this work is on the current pass's
    /// targets; answers the pass that ended when it wants a snapshot.
    fn enter(&mut self, color: &str, depth: Option<&str>) -> Option<SnapAsk> {
        let key = (color.to_owned(), depth.map(str::to_owned));
        if self.key.as_ref() == Some(&key) {
            return None;
        }
        let ended = self
            .key
            .as_ref()
            .and_then(|_| self.wants_snap(self.rec.passes.len() - 1));
        self.targets.insert(color.to_owned());
        let index = self.rec.passes.len();
        self.rec.passes.push(PassRecord {
            index,
            color: key.0.clone(),
            depth: key.1.clone(),
            ..Default::default()
        });
        self.key = Some(key);
        ended
    }

    fn pass(&mut self) -> &mut PassRecord {
        self.rec.passes.last_mut().expect("entered")
    }

    fn wants_snap(&self, pass: usize) -> Option<SnapAsk> {
        let seq = self.arming.seq;
        self.arming
            .snap
            .contains(&pass)
            .then_some(SnapAsk {
                seq,
                pass,
                attempt: self.arming.attempt,
                display: self.arming.layout,
            })
    }
}

fn retry_suffix(attempt: u64) -> String {
    if attempt == 0 { String::new() } else { format!("-retry-{attempt}") }
}

impl ProbeState {
    fn retry_original(&mut self) -> Option<Stage> {
        let mut next = self.stage.arming()?.clone();
        self.attempt = self.attempt.wrapping_add(1);
        next.attempt = self.attempt;
        Some(std::mem::replace(&mut self.stage, Stage::Waiting(next)))
    }
}

impl Stage {
    fn arming(&self) -> Option<&Arming> {
        match self {
            Self::Idle => None,
            Self::Waiting(a) | Self::Graph(a) | Self::Routing { arming: a, .. } => Some(a),
            Self::Recording(r) => Some(&r.arming),
        }
    }
}

fn reply(s: &ProbeState) -> FrameReply {
    let arming = s.stage.arming();
    FrameReply {
        armed: arming.is_some(),
        seq: arming.map(|a| a.seq),
        armed_at_present: arming.map(|a| a.at_present),
        frame: s.last.clone(),
    }
}

/// What `render.stats` reads, each `None` where that piece isn't running.
pub struct RenderSources {
    pub display_rate: Box<dyn Fn() -> Option<agent_proto::DisplayRateStats> + Send>,
    pub ui: Box<dyn Fn() -> Option<agent_proto::UiStats> + Send>,
    pub counters: Box<dyn Fn() -> Option<Value> + Send>,
}

/// Adds the frame recorders (over `probe`) and `render.stats` (over `sources`).
pub fn add_methods(agent: &Agent, probe: &'static FrameProbe, sources: RenderSources) {
    for (name, layout) in [
        (method::RENDER_FRAME, false),
        (method::RENDER_FRAME_LAYOUT, true),
    ] {
        agent.add_method(name, Access::Read, move |v| {
            let p: FrameParams = if v.is_null() {
                FrameParams::default()
            } else {
                serde_json::from_value(v)
                    .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
            };
            let probe = if p.source == Some(agent_proto::frame_graph::Source::Private) {
                if !private_capture_available() {
                    return Err(RpcError::new(code::UNAVAILABLE, "no private-target scene renderer registered"));
                }
                &private_graph::PROBE
            } else {
                probe
            };
            let r = if p.arm {
                probe
                    .arm_frame(&p, layout)
                    .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?
            } else {
                if p.draws || p.max_draws.is_some() || !p.snap.is_empty() || !p.constants.is_empty() {
                    return Err(RpcError::new(
                        code::INVALID_PARAMS,
                        "draws, max_draws, snap and constants go with arm",
                    ));
                }
                probe.reply()
            };
            serde_json::to_value(r).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
        });
    }
    agent.add_method(method::RENDER_STATS, Access::Read, move |v| {
        if !(v.is_null() || v.as_object().is_some_and(Map::is_empty)) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "render.stats takes no params",
            ));
        }
        let s = RenderStats {
            display_frame: private_graph::display_stats(),
            private_frame: private_graph::last_stats(),
            display_rate: (sources.display_rate)(),
            ui: (sources.ui)(),
            counters: (sources.counters)().unwrap_or(Value::Null),
        };
        serde_json::to_value(s).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
    });
}

// ---- render.scene and render.parity: filled by the host ----

/// Fills `render.scene` (boot's scene glue registers it).
pub type SceneSource = fn(dump: bool) -> Result<agent_proto::SceneReply, String>;

mod parity;
pub use parity::{ParityRequest, ParitySource};

/// The host's scene and parity sources, and the last tick that ended.
pub struct Hosted {
    scene: Mutex<Option<SceneSource>>,
    parity: Mutex<Option<ParitySource>>,
    /// The last tick that ended; `u64::MAX` before the first.
    last_tick: AtomicU64,
}

/// The game's.
pub static HOSTED: Hosted = Hosted::new();

impl Default for Hosted {
    fn default() -> Self {
        Self::new()
    }
}

impl Hosted {
    pub const fn new() -> Hosted {
        Hosted {
            scene: Mutex::new(None),
            parity: Mutex::new(None),
            last_tick: AtomicU64::new(u64::MAX),
        }
    }

    pub fn set_scene_source(&self, f: SceneSource) {
        *self.scene.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    pub fn set_parity_source(&self, f: Option<ParitySource>) {
        *self.parity.lock().unwrap_or_else(|e| e.into_inner()) = f;
    }

    fn scene_source(&self) -> Option<SceneSource> {
        *self.scene.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn parity_source(&self) -> Option<ParitySource> {
        *self.parity.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The last tick that ended, as the host's tick observer said
    /// ([`FrameCapture::tick_ended`]).
    pub fn last_tick(&self) -> Option<u64> {
        let t = self.last_tick.load(Ordering::Relaxed);
        (t != u64::MAX).then_some(t)
    }

    fn note_tick(&self, tick: u64) {
        self.last_tick.store(tick, Ordering::Relaxed);
    }

    /// `render.scene`: the source's answer, or why there is none.
    pub fn scene(&self, dump: bool) -> Result<SceneReply, RpcError> {
        let f = self.scene_source().ok_or_else(|| {
            RpcError::new(
                code::UNAVAILABLE,
                "no scene has been built: render.scene reads the native renderer's FrameScene \
                 (docs/graphics/framescene.md, slice 1), and nothing registered one",
            )
        })?;
        f(dump).map_err(|e| RpcError::new(code::UNAVAILABLE, e))
    }

    /// The sealed scene's summary hash, when there is a source and a sealed
    /// scene.
    fn scene_hash(&self) -> Option<String> {
        let f = self.scene_source()?;
        f(false).ok().filter(|s| s.sealed).map(|s| s.hash)
    }
}

/// Registers `render.scene`'s source (boot's scene glue).
pub fn set_scene_source(f: SceneSource) {
    HOSTED.set_scene_source(f);
}

/// Registers the reference and private rendering pair for `render.parity`.
pub fn set_parity_source(source: Option<ParitySource>) {
    HOSTED.set_parity_source(source);
}

// ---- render.capture ----

mod private_capture;
pub use private_capture::{PrivateCaptureScene, PrivateCaptureSource, PrivateCaptureWrite, private_capture_available, set_private_capture_source};

fn capture_interrupted() -> RpcError {
    RpcError::new(code::INTERRUPTED, "a later render.capture or a renderer reset replaced this one")
}

/// `render.capture`'s state: the frame of a tick, by tick. One per process
/// ([`CAPTURE`]); tests make their own.
///
/// The host calls [`tick_ended`](Self::tick_ended) after each game tick and
/// [`present`](Self::present) at each guest present (both on the game
/// thread; one relaxed load each when nothing is asked). The first present
/// after the tick asked for has ended is the tick's frame (on the D3D path
/// the frame of a tick is the present that follows it). Its image comes
/// from the host's frame writer ([`wanted`](Self::wanted),
/// [`written`](Self::written), on whatever thread presents), or, with
/// `pass`, from [`FrameProbe`]'s snapshot of that pass: the probe is armed
/// at the end of the tick before, so the frame it records is the one from
/// the present before the tick to the present after it.
pub struct FrameCapture {
    /// A capture waits for its present: the tick and present calls look.
    pending: AtomicBool,
    source_armed: AtomicBool,
    /// The present whose image the frame writer should write; 0 for none.
    want: AtomicU64,
    probe: &'static FrameProbe,
    hosted: &'static Hosted,
    state: Mutex<CaptureState>,
    map_source: Mutex<Option<Arc<CaptureMapSource>>>,
}

type CaptureMapSource = dyn Fn(&str) -> Option<(MapId, String)> + Send + Sync;

struct CaptureState {
    /// The host can write presented frames.
    writer: bool,
    next_seq: u64,
    job: Option<CaptureJob>,
    private: private_capture::PrivateState,
}

struct CaptureJob {
    seq: u64,
    target: u64,
    pass: Option<usize>,
    /// The probe's arming, with `pass`.
    probe_seq: Option<u64>,
    /// The tick and present it became.
    bound: Option<(u64, u64)>,
    scene_hash: Option<String>,
    path: PathBuf,
    source: Option<CaptureSourceJob>,
    /// The frame writer's result, or why it failed before.
    outcome: Option<Result<Option<Sha256>, String>>,
}

struct CaptureSourceJob {
    game_id: String,
    drawn: Option<(String, CaptureCamera)>,
    frame: Option<(String, MapId, CaptureCamera)>,
}

/// The frame writer's destination and whether the exact PNG needs a digest.
#[derive(Debug, PartialEq, Eq)]
pub struct CaptureWrite {
    pub path: PathBuf,
    pub digest: bool,
}

/// The game's.
pub static CAPTURE: FrameCapture = FrameCapture::new(&PROBE, &HOSTED);

impl FrameCapture {
    pub const fn new(probe: &'static FrameProbe, hosted: &'static Hosted) -> FrameCapture {
        FrameCapture {
            pending: AtomicBool::new(false),
            source_armed: AtomicBool::new(false),
            want: AtomicU64::new(0),
            probe,
            hosted,
            map_source: Mutex::new(None),
            state: Mutex::new(CaptureState {
                writer: false,
                next_seq: 1,
                job: None,
                private: private_capture::PrivateState::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CaptureState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The host writes presented frames (boot's `H2_BOOT_GFX` present hook
    /// for the display): whole-frame captures are possible.
    pub fn set_writer(&self) {
        self.lock().writer = true;
    }

    /// The host resolves a drawn map's cache alias to its installed package and base name.
    pub fn set_map_source(
        &self,
        source: impl Fn(&str) -> Option<(MapId, String)> + Send + Sync + 'static,
    ) {
        *self.map_source.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(source));
    }

    /// The renderer applied this spawn view to the frame in progress.
    /// Resolves the map and the pose only for an armed provenance capture.
    pub fn note_drawn_camera(&self, read: impl FnOnce() -> Option<(String, CaptureCamera)>) {
        if self.source_armed.load(Ordering::Relaxed)
            && let Some(source) = self.lock().job.as_mut().and_then(|j| j.source.as_mut())
        {
            source.drawn = read();
        }
    }

    /// The old map's draw can no longer be used for a provenance capture.
    pub fn clear_drawn_camera(&self) {
        self.note_drawn_camera(|| None);
    }

    /// Arms for `p.tick` (default: the tick after the last one ended); the
    /// image goes under `dir`. Replaces a capture still waiting. Answers
    /// the arming's number for [`finish`](Self::finish).
    pub fn arm(&self, p: &FrameCaptureParams, dir: &Path) -> Result<u64, RpcError> {
        if let Some(id) = &p.game_id
            && (p.pass.is_some() || !live_proto::game::valid_id(id))
        {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "game_id must be a valid Live id and can only accompany a whole-frame capture",
            ));
        }
        if let Some(lighting) = p.lighting {
            if p.pass.is_some() {
                return Err(RpcError::new(code::INVALID_PARAMS, "lighting selects a whole scene and cannot be combined with pass"));
            }
            if p.game_id.is_some() {
                return Err(RpcError::new(code::INVALID_PARAMS, "game_id captures the displayed frame and cannot be combined with lighting"));
            }
            return self.arm_private(lighting, p.tick, dir);
        }
        let Some(last) = self.hosted.last_tick() else {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "no game tick has ended yet (or the agent-frames feature is off)",
            ));
        };
        let target = p.tick.unwrap_or(last + 1);
        if target <= last {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("tick {target} has ended (the last was {last}); ask for a later one"),
            ));
        }
        if target - last > FrameCaptureParams::MAX_AHEAD {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!(
                    "tick {target} is more than {} ticks ahead of {last}",
                    FrameCaptureParams::MAX_AHEAD
                ),
            ));
        }
        let mut s = self.lock();
        if p.pass.is_none() && !s.writer {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "no frame writer: a whole-frame capture needs a run that renders on the GPU \
                 (H2_BOOT_GFX, with H2_BOOT_GFX_EVERY=never for no other screenshots); \
                 `pass` reads one pass back without it",
            ));
        }
        let seq = s.next_seq;
        s.next_seq += 1;
        let mut job = CaptureJob {
            seq,
            target,
            pass: p.pass,
            probe_seq: None,
            bound: None,
            scene_hash: None,
            path: dir.join(format!("capture-{}-{seq}-tick-{target}.png", std::process::id())),
            source: p.game_id.clone().map(|game_id| CaptureSourceJob {
                game_id,
                drawn: None,
                frame: None,
            }),
            outcome: None,
        };
        if let Some(pass) = p.pass
            && target == last + 1
        {
            job.probe_seq = Some(self.arm_probe(pass)?);
        }
        self.probe.cancel_capture_pass(s.job.as_ref());
        s.private.active = None;
        s.job = Some(job);
        self.want.store(0, Ordering::Relaxed);
        self.source_armed.store(p.game_id.is_some(), Ordering::Relaxed);
        self.pending.store(true, Ordering::Relaxed);
        Ok(seq)
    }

    fn arm_probe(&self, pass: usize) -> Result<u64, RpcError> {
        let r = self
            .probe
            .arm(&FrameParams {
                arm: true,
                snap: vec![pass],
                ..Default::default()
            })
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
        r.seq
            .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "the frame recorder didn't arm"))
    }

    /// After each game tick, on the game thread: the tick that ended (the
    /// game's own count, as `at.tick`).
    pub fn tick_ended(&self, tick: u64) {
        self.hosted.note_tick(tick);
        if !self.pending.load(Ordering::Relaxed) {
            return;
        }
        let mut s = self.lock();
        let Some(j) = &mut s.job else {
            return;
        };
        if let Some(pass) = j.pass
            && j.probe_seq.is_none()
            && j.bound.is_none()
            && j.outcome.is_none()
            && tick + 1 >= j.target
        {
            match self.arm_probe(pass) {
                Ok(seq) => j.probe_seq = Some(seq),
                Err(e) => j.outcome = Some(Err(e.message)),
            }
        }
    }

    /// A guest present, numbered `n` (as `render.frame` and `frame-N.png`
    /// number them), on the game thread before the backend gets it.
    pub fn present(&self, n: u64) {
        if !self.pending.load(Ordering::Relaxed) {
            return;
        }
        let last = self.hosted.last_tick();
        let (seq, drawn) = {
            let mut s = self.lock();
            let Some(j) = &mut s.job else {
                return;
            };
            if j.bound.is_some() || j.outcome.is_some() {
                return;
            }
            // A view belongs only to this present. Dropping a draw from an
            // early present prevents a later, camera-less frame using it.
            let drawn = j.source.as_mut().and_then(|source| source.drawn.take());
            let Some(tick) = last.filter(|&t| t >= j.target) else {
                return;
            };
            if j.source.is_some() && drawn.is_none() {
                return;
            }
            j.bound = Some((tick, n));
            self.pending.store(false, Ordering::Relaxed);
            self.source_armed.store(false, Ordering::Relaxed);
            (j.seq, drawn)
        };
        // The sources are the host's. Call them without the capture lock.
        let frame = drawn.and_then(|(map, camera)| {
            let source = self.map_source.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
            let (id, base) = source(&map)?;
            Some((id, base, camera))
        });
        let hash = self.hosted.scene_hash();
        if let Some(j) = &mut self.lock().job
            && j.seq == seq
        {
            j.scene_hash = hash;
            if let Some(source) = &mut j.source {
                let Some((map_id, map, camera)) = frame else {
                    j.outcome = Some(Err("the drawn map is not an installed package".into()));
                    return;
                };
                source.frame = Some((map, map_id, camera));
            }
            if j.pass.is_none() {
                self.want.store(n, Ordering::Relaxed);
            }
        }
    }

    /// The frame writer, at present `present`: where to write its image,
    /// when it is wanted (one relaxed load when not). Call
    /// [`written`](Self::written) after.
    pub fn wanted(&self, present: u64) -> Option<CaptureWrite> {
        if present == 0 || self.want.load(Ordering::Relaxed) != present {
            return None;
        }
        let s = self.lock();
        s.job
            .as_ref()
            .filter(|j| j.bound.map(|b| b.1) == Some(present) && j.outcome.is_none())
            .map(|j| CaptureWrite {
                path: j.path.clone(),
                digest: j.source.is_some(),
            })
    }

    /// The frame writer wrote present `present`'s image where
    /// [`wanted`](Self::wanted) said, with a digest if requested, or failed.
    pub fn written(&self, present: u64, result: Result<Option<Sha256>, String>) {
        let _ = self
            .want
            .compare_exchange(present, 0, Ordering::Relaxed, Ordering::Relaxed);
        let mut s = self.lock();
        if let Some(j) = &mut s.job
            && j.bound.map(|b| b.1) == Some(present)
            && j.outcome.is_none()
        {
            j.outcome = Some(result);
        }
    }

    /// The answer for arming `seq`; `None` while it waits.
    pub fn finish(&self, seq: u64) -> Option<Result<CaptureReply, RpcError>> {
        let mut s = self.lock();
        if let Some((active, job)) = &s.private.active
            && *active == seq
        {
            let job = Arc::clone(job);
            drop(s);
            return self.finish_private(seq, job);
        }
        let out = {
            let Some(j) = s.job.as_ref().filter(|j| j.seq == seq) else {
                return Some(Err(capture_interrupted()));
            };
            match j.pass {
                None => self.frame_outcome(j)?,
                Some(pass) => self.pass_outcome(j, pass)?,
            }
        };
        s.job = None;
        self.pending.store(false, Ordering::Relaxed);
        self.source_armed.store(false, Ordering::Relaxed);
        self.want.store(0, Ordering::Relaxed);
        Some(out)
    }

    fn frame_outcome(&self, j: &CaptureJob) -> Option<Result<CaptureReply, RpcError>> {
        match j.outcome.as_ref()? {
            Err(e) => Some(Err(RpcError::new(
                code::UNAVAILABLE,
                format!("writing the frame: {e}"),
            ))),
            Ok(digest) => {
                let (tick, present) = j.bound?;
                let source = if let Some(source) = &j.source {
                    let (Some((map, map_id, camera)), Some(digest)) = (&source.frame, digest) else {
                        return Some(Err(RpcError::new(
                            code::UNAVAILABLE,
                            "the capture writer did not supply complete provenance",
                        )));
                    };
                    Some(CaptureSource {
                        game_id: source.game_id.clone(),
                        map_id: map_id.to_string(),
                        map: map.clone(),
                        camera: *camera,
                        digest: digest.to_string(),
                    })
                } else {
                    None
                };
                Some(Ok(CaptureReply {
                    path: j.path.display().to_string(),
                    tick,
                    present,
                    pass: None,
                    lighting: None,
                    scene_hash: j.scene_hash.clone(),
                    source,
                }))
            }
        }
    }

    fn pass_outcome(&self, j: &CaptureJob, pass: usize) -> Option<Result<CaptureReply, RpcError>> {
        let fail = |m: String| Some(Err(RpcError::new(code::UNAVAILABLE, m)));
        if let Some(Err(e)) = &j.outcome {
            return fail(format!("arming the frame recorder: {e}"));
        }
        let (tick, present) = j.bound?;
        let Some(probe_seq) = j.probe_seq else {
            return fail("the frame recorder wasn't armed before the frame began".into());
        };
        // The probe stays armed after the frame until the last pass's
        // snapshot is in (its reply already says not armed): read that first.
        let snapping = self.probe.armed();
        let r = self.probe.reply();
        let Some(f) = r.frame.as_ref().filter(|f| f.seq == probe_seq) else {
            if r.armed && r.seq == Some(probe_seq) {
                return None;
            }
            return Some(Err(RpcError::new(
                code::INTERRUPTED,
                "render.frame was armed again before the frame was recorded",
            )));
        };
        if f.present != present {
            return fail(format!(
                "the frame recorded ended at present {}, not {present} (more than one tick in a frame?)",
                f.present
            ));
        }
        match f.snaps.iter().find(|s| s.pass == pass) {
            Some(snap) => match (&snap.file, &snap.error) {
                (Some(file), _) => Some(Ok(CaptureReply {
                    path: file.clone(),
                    tick,
                    present,
                    pass: Some(pass),
                    lighting: None,
                    scene_hash: j.scene_hash.clone(),
                    source: None,
                })),
                (None, e) => fail(format!(
                    "reading pass {pass} back: {}",
                    e.as_deref().unwrap_or("no file")
                )),
            },
            None if snapping => None,
            None => fail(format!(
                "the frame had {} passes; no pass {pass}",
                f.passes.len()
            )),
        }
    }
}

/// Adds `render.compare`, `render.capture` (over `capture`), and
/// `render.scene` and `render.parity` (over `hosted`).
pub fn add_frame_methods(agent: &Agent, capture: &'static FrameCapture, hosted: &'static Hosted) {
    let invalid = |e: serde_json::Error| RpcError::new(code::INVALID_PARAMS, e.to_string());
    let reply = |v: Result<Value, serde_json::Error>| {
        v.map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
    };
    agent.add_method(method::RENDER_COMPARE, Access::Read, move |v| {
        let p: CompareParams = serde_json::from_value(v).map_err(invalid)?;
        let r = crate::compare::compare_files(&p).map_err(|e| e.rpc())?;
        reply(serde_json::to_value(r))
    });
    agent.add_game_method(method::RENDER_CAPTURE, Access::Read, move |v, ctx| {
        let p: FrameCaptureParams = if v.is_null() {
            FrameCaptureParams::default()
        } else {
            serde_json::from_value(v).map_err(invalid)?
        };
        let seq = capture.arm(&p, &agent_proto::cache_dir().join("frames"))?;
        let encode = move |result: Result<CaptureReply, RpcError>| result.and_then(|r| reply(serde_json::to_value(r)));
        if p.lighting.is_some() {
            let lease = private_capture::Lease::new(capture, seq);
            ctx.defer_host(move || lease.finish().map(encode));
        } else {
            ctx.defer(DEFER_TICKS_MAX, Box::new(move |_| capture.finish(seq).map(encode)));
        }
        Ok(Value::Null)
    });
    agent.add_method(method::RENDER_SCENE, Access::Read, move |v| {
        let p: SceneParams = if v.is_null() {
            SceneParams::default()
        } else {
            serde_json::from_value(v).map_err(invalid)?
        };
        reply(serde_json::to_value(hosted.scene(p.dump)?))
    });
    agent.add_game_method(method::RENDER_PARITY, Access::Act, move |v, ctx| {
        let p: ParityParams = serde_json::from_value(v).map_err(invalid)?;
        if !p.arm {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "render.parity {\"arm\": true} arms the next frame step",
            ));
        }
        let source = hosted.parity_source().ok_or_else(|| RpcError::new(
            code::UNAVAILABLE,
            "render.parity needs both the D3D display and the sealed scene's private renderer",
        ))?;
        parity::arm(ctx, source);
        Ok(Value::Null)
    });
}

/// The registers `wanted` (D3D numbering) out of a draw's constants
/// (hardware order): those the block has.
fn picked_constants(wanted: &[i32], block: Option<&[[f32; 4]]>) -> BTreeMap<i32, [f32; 4]> {
    let Some(block) = block else {
        return BTreeMap::new();
    };
    wanted
        .iter()
        .filter_map(|&r| {
            let i = usize::try_from(r + 96).ok()?;
            block.get(i).map(|v| (r, *v))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(color: &str, outcome: &str, verts: u32) -> DrawInfo {
        DrawInfo {
            color: color.into(),
            depth: Some("z".into()),
            target: "640x480 X8R8G8B8".into(),
            entry: "DrawIndexedVertices".into(),
            primitive: "TriangleList".into(),
            vertices: verts,
            indices: verts * 2,
            outcome: outcome.into(),
            textures: vec!["Dxt1".into()],
            program: true,
            blend: false,
            depth_write: true,
            ..DrawInfo::default()
        }
    }

    #[test]
    fn original_snapshots_survive_only_if_their_frame_is_published() {
        let p = FrameProbe::new();
        let params = FrameParams { arm: true, snap: vec![0], ..Default::default() };
        let dir = std::env::temp_dir().join(format!("h2-probe-original-{}", std::process::id()));
        let arm = |start| {
            let seq = p.arm(&params).unwrap().seq.unwrap();
            p.present(start);
            p.clear("colour", None);
            (SnapAsk { seq, pass: 0, attempt: 0, display: false }, dir.join(format!("frame-{}-{seq}-pass-0.png", std::process::id())))
        };
        let image = || Ok(crate::compare::Rgba::filled(2, 1, [40, 12, 9, 255]));

        // The first snapshot arrives while the original frame is open.
        let (old, old_path) = arm(1);
        p.write_snapshot_in(old, &dir, image);
        assert!(old_path.is_file());
        p.present_routed(2, Some(2));
        let (current, path) = arm(3);
        assert!(!old_path.exists(), "the abandoned route released its PNG");
        p.write_snapshot_in(old, &dir, || panic!("a stale pass requested pixels"));

        // The final pass's callback can arrive after the original present.
        assert_eq!(p.present(4), Some(current));
        p.write_snapshot_in(current, &dir, image);
        assert_eq!(p.reply().frame.unwrap().snaps[0].file.as_deref(), path.to_str());
        p.arm(&params).unwrap();
        assert!(path.is_file(), "a published reply owns its file");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn unarmed_records_nothing() {
        let p = FrameProbe::new();
        assert!(!p.armed());
        assert_eq!(p.draw(draw("a", "drawn", 3)), None);
        assert_eq!(p.present(1), None);
        let r = p.reply();
        assert!(!r.armed && r.frame.is_none());
    }

    #[test]
    fn a_whole_frame_between_two_presents_in_passes() {
        let p = FrameProbe::new();
        p.present(10);
        let armed = p
            .arm(&FrameParams {
                arm: true,
                draws: true,
                max_draws: Some(3),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            (armed.armed, armed.seq, armed.armed_at_present),
            (true, Some(1), Some(10))
        );
        // The rest of the frame being drawn when it was armed: not kept.
        p.draw(draw("shadow", "drawn", 99));
        p.present(11);
        p.clear("shadow", Some("z"));
        p.draw(draw("shadow", "drawn", 30));
        p.draw(draw("main", "drawn", 6));
        p.op("copy", "main", "-> mirror".into());
        p.draw(draw("main", "skipped empty", 3));
        p.draw(draw("shadow", "drawn", 3));
        p.present(12);
        assert!(!p.armed(), "one frame, then disarmed");
        // After it: not recorded.
        p.draw(draw("main", "drawn", 1));
        let f = p.reply().frame.unwrap();
        assert_eq!((f.seq, f.from_present, f.present), (1, 11, 12));
        assert_eq!(f.totals.draws, 4);
        assert_eq!(f.totals.vertices, 42);
        assert_eq!(
            (f.totals.clears, f.totals.copies, f.totals.targets),
            (1, 1, 2)
        );
        assert_eq!(f.totals.outcomes["skipped empty"], 1);
        let colors: Vec<&str> = f.passes.iter().map(|p| p.color.as_str()).collect();
        assert_eq!(
            colors,
            ["shadow", "main", "shadow"],
            "back to a target: a new pass"
        );
        assert_eq!(f.passes[0].clears, 1);
        assert_eq!(f.passes[0].target.as_deref(), Some("640x480 X8R8G8B8"));
        assert_eq!(f.passes[1].draws, 2);
        assert_eq!(f.passes[1].first_draw, Some(1));
        assert_eq!(f.passes[1].textures["Dxt1"], 2);
        assert_eq!(f.ops[0].after_draws, 2);
        assert_eq!(f.draws.len(), 3);
        assert!(f.truncated, "four draws, three listed");
        assert_eq!(f.draws[2].pass, 1);
    }

    #[test]
    fn snapshots_are_asked_for_when_their_pass_ends() {
        let p = FrameProbe::new();
        p.arm(&FrameParams {
            arm: true,
            snap: vec![0, 1],
            ..Default::default()
        })
        .unwrap();
        p.present(1);
        assert_eq!(p.draw(draw("a", "drawn", 3)), None);
        assert_eq!(p.draw(draw("a", "drawn", 3)), None);
        assert_eq!(
            p.draw(draw("b", "drawn", 3)),
            Some(SnapAsk {
                seq: 1,
                pass: 0,
                attempt: 0,
                display: false,
            })
        );
        p.snapped(0, Ok("/tmp/p0.png".into()));
        // The last pass ends at the present: the frame is done, the probe
        // stays armed until the snapshot is in.
        assert_eq!(
            p.present(2),
            Some(SnapAsk {
                seq: 1,
                pass: 1,
                attempt: 0,
                display: false,
            })
        );
        assert!(p.armed());
        p.snapped(1, Err("readback failed".into()));
        assert!(!p.armed());
        let f = p.reply().frame.unwrap();
        assert_eq!(f.snaps.len(), 2);
        assert_eq!(f.snaps[0].file.as_deref(), Some("/tmp/p0.png"));
        assert_eq!(f.snaps[1].error.as_deref(), Some("readback failed"));
        // Nothing more is recorded while the snapshot was pending.
        assert_eq!(p.reply().frame.unwrap().totals.draws, 3);
    }

    #[test]
    fn layout_records_one_frame_and_requests_display_snapshots() {
        let p = FrameProbe::new();
        let params = FrameParams {
            arm: true,
            snap: vec![0, 1],
            constants: vec![0],
            ..Default::default()
        };
        assert!(p.arm(&params).is_err(), "the regular recorder needs draws");
        p.arm_layout(&params).unwrap();
        assert!(!p.layout_requested(), "the current frame is still drawing");
        p.present(1);
        assert!(p.layout_requested());
        let layout = DrawLayout {
            guest_size: [640, 480],
            display_size: Some([1920, 1080]),
            viewport: [0, 0, 640, 480],
            projection_factor: Some(0.75),
            ..Default::default()
        };
        p.draw(DrawInfo {
            layout: Some(layout.clone()),
            ..draw("scene", "drawn", 6)
        });
        assert_eq!(
            p.enter("output", None),
            Some(SnapAsk {
                seq: 1,
                pass: 0,
                attempt: 0,
                display: true,
            })
        );
        p.snapped(0, Ok("scene.png".into()));
        assert_eq!(
            p.present(2),
            Some(SnapAsk {
                seq: 1,
                pass: 1,
                attempt: 0,
                display: true,
            })
        );
        assert!(!p.layout_requested());
        p.snapped(1, Ok("output.png".into()));
        let frame = p.reply().frame.unwrap();
        assert_eq!(frame.draws[0].layout.as_ref(), Some(&layout));
        assert_eq!(frame.snaps.len(), 2);

        p.arm(&FrameParams {
            arm: true,
            draws: true,
            ..Default::default()
        })
        .unwrap();
        p.present(3);
        assert!(!p.layout_requested());
        p.draw(DrawInfo {
            layout: Some(layout),
            ..draw("output", "drawn", 4)
        });
        p.present(4);
        assert!(p.reply().frame.unwrap().draws[0].layout.is_none());
    }

    #[test]
    fn arming_checks_its_limits_and_forgets_the_last_frame() {
        let p = FrameProbe::new();
        let bad = FrameParams {
            arm: true,
            max_draws: Some(FrameParams::MAX_DRAWS + 1),
            ..Default::default()
        };
        assert!(p.arm(&bad).is_err());
        let bad = FrameParams {
            arm: true,
            snap: vec![0, 1, 2, 3, 4],
            ..Default::default()
        };
        assert!(p.arm(&bad).is_err());
        let ok = FrameParams {
            arm: true,
            ..Default::default()
        };
        p.arm(&ok).unwrap();
        p.present(1);
        p.present(2);
        assert!(p.reply().frame.is_some());
        let r = p.arm(&ok).unwrap();
        assert_eq!((r.seq, r.frame), (Some(2), None));
    }

    #[test]
    fn draws_carry_depth_state_and_the_constants_asked_for() {
        let p = FrameProbe::new();
        p.arm(&FrameParams {
            arm: true,
            draws: true,
            constants: vec![-96, -38, 95],
            ..Default::default()
        })
        .unwrap();
        p.present(1);
        let mut block = vec![[0.0f32; 4]; 192];
        block[0] = [1.0, 2.0, 3.0, 4.0];
        block[58] = [0.0, 0.0, 16384.0, 1.0];
        block[191] = [9.0; 4];
        let depth = DepthRecord {
            mode: "w".into(),
            w: true,
            func: "LessEqual".into(),
            write: false,
            viewport: [0.0, 1.0],
            scale: [16777215.0, 0.0],
            clip: [0.0, 16777215.0],
            full: 16777215.0,
            clamp: true,
        };
        p.draw(DrawInfo {
            vs: "prog @0 n=4 #00000001".into(),
            depth_state: Some(depth.clone()),
            constants: Some(Arc::from(block)),
            ..draw("main", "drawn", 3)
        });
        // A draw with no constant block (fixed function before any
        // register was set): no constants, no error.
        p.draw(draw("main", "drawn", 3));
        p.present(2);
        let f = p.reply().frame.unwrap();
        let d = &f.draws[0];
        assert_eq!(d.vs, "prog @0 n=4 #00000001");
        assert_eq!(d.depth.as_ref(), Some(&depth));
        assert_eq!(d.constants.len(), 3);
        assert_eq!(d.constants[&-96], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(d.constants[&-38], [0.0, 0.0, 16384.0, 1.0]);
        assert_eq!(d.constants[&95], [9.0; 4]);
        assert!(f.draws[1].constants.is_empty());
        assert!(f.draws[1].depth.is_none());
    }

    #[test]
    fn constants_are_checked_when_arming() {
        let p = FrameProbe::new();
        let arm = |constants: Vec<i32>, draws: bool| {
            p.arm(&FrameParams {
                arm: true,
                draws,
                constants,
                ..Default::default()
            })
        };
        assert!(
            arm(vec![96], true).is_err(),
            "c96 is past the last register"
        );
        assert!(arm(vec![-97], true).is_err());
        assert!(arm((0..33).collect(), true).is_err(), "at most 32");
        assert!(arm(vec![0], false).is_err(), "only on listed draws");
        assert!(arm(vec![-96, 95], true).is_ok());
    }
}

#[cfg(test)]
mod frame_scene_tests {
    use super::*;
    use serde_json::json;
    use std::sync::mpsc::{Receiver, sync_channel};
    use std::time::Duration;

    fn leak<T>(t: T) -> &'static T {
        Box::leak(Box::new(t))
    }

    fn rigs() -> (&'static FrameProbe, &'static Hosted, &'static FrameCapture) {
        let probe = leak(FrameProbe::new());
        let hosted = leak(Hosted::new());
        (probe, hosted, leak(FrameCapture::new(probe, hosted)))
    }

    fn whole(tick: u64) -> FrameCaptureParams {
        FrameCaptureParams {
            tick: Some(tick),
            ..Default::default()
        }
    }

    const DIR: &str = "/nowhere/frames";

    #[test]
    fn a_capture_is_the_present_after_its_tick() {
        let (_, _, c) = rigs();
        let dir = Path::new(DIR);
        assert_eq!(
            c.arm(&whole(12), dir).unwrap_err().code,
            code::UNAVAILABLE,
            "no tick has ended"
        );
        c.tick_ended(10);
        assert_eq!(
            c.arm(&whole(12), dir).unwrap_err().code,
            code::UNAVAILABLE,
            "no frame writer"
        );
        c.set_writer();
        assert_eq!(c.arm(&whole(10), dir).unwrap_err().code, code::INVALID_PARAMS);
        assert_eq!(
            c.arm(&whole(11 + FrameCaptureParams::MAX_AHEAD), dir)
                .unwrap_err()
                .code,
            code::INVALID_PARAMS,
            "too far ahead"
        );
        let seq = c.arm(&whole(12), dir).unwrap();
        // The frame of tick 10, then of 11: not it.
        c.present(40);
        assert_eq!(c.wanted(40), None);
        c.tick_ended(11);
        c.present(41);
        assert_eq!(c.wanted(41), None);
        assert_eq!(c.finish(seq), None);
        c.tick_ended(12);
        c.present(42);
        assert_eq!(c.wanted(41), None, "only the bound present");
        let write = c.wanted(42).unwrap();
        assert!(!write.digest);
        let path = write.path;
        assert_eq!(path, dir.join(format!("capture-{}-{seq}-tick-12.png", std::process::id())));
        assert_eq!(c.finish(seq), None, "not written yet");
        c.written(42, Ok(None));
        assert_eq!(c.wanted(42), None);
        let r = c.finish(seq).unwrap().unwrap();
        assert_eq!(r.path, path.display().to_string());
        assert_eq!((r.tick, r.present, r.pass), (12, 42, None));
        assert_eq!(r.scene_hash, None, "no scene source");
        // Nothing waits now.
        c.tick_ended(13);
        c.present(43);
        assert_eq!(c.wanted(43), None);
    }

    fn spawn_camera() -> CaptureCamera {
        CaptureCamera {
            position: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn live_capture_binds_the_drawn_map_camera_and_png_to_one_present() {
        let (_, _, c) = rigs();
        let map = MapId([0x1f; 32]);
        c.set_map_source(move |name| (name == "package-alias").then(|| (map, "lockout".into())));
        c.set_writer();
        c.tick_ended(10);
        let params = FrameCaptureParams {
            tick: Some(12),
            game_id: Some("private-game-id".into()),
            ..Default::default()
        };
        let seq = c.arm(&params, Path::new(DIR)).unwrap();
        let camera = spawn_camera();
        c.note_drawn_camera(|| Some(("earlier-map".into(), camera)));
        c.present(40); // Tick 10's draw must not carry forward.
        c.tick_ended(12);
        c.present(41);
        assert_eq!(c.wanted(41), None);
        assert_eq!(c.finish(seq), None);
        c.note_drawn_camera(|| Some(("package-alias".into(), camera)));
        c.tick_ended(13);
        c.present(42);
        let write = c.wanted(42).unwrap();
        assert!(write.digest);
        assert_eq!(c.finish(seq), None);
        let digest = Sha256::of(b"synthetic PNG");
        c.written(42, Ok(Some(digest)));
        let capture = c.finish(seq).unwrap().unwrap();
        assert_eq!((capture.tick, capture.present), (13, 42));
        assert_eq!(capture.path, write.path.display().to_string());
        let source = capture.source.unwrap();
        assert_eq!(source.game_id, "private-game-id");
        assert_eq!(source.map_id, map.to_string());
        assert_eq!(source.map, "lockout");
        assert_eq!(source.camera, camera);
        assert_eq!(source.digest, digest.to_string());
    }

    #[test]
    fn live_capture_requires_a_whole_frame_installed_map_and_writer_digest() {
        let (_, _, c) = rigs();
        c.set_writer();
        c.tick_ended(10);
        let mut params = FrameCaptureParams {
            game_id: Some("private-game-id".into()),
            pass: Some(0),
            ..Default::default()
        };
        let dir = Path::new(DIR);
        assert_eq!(c.arm(&params, dir).unwrap_err().code, code::INVALID_PARAMS);
        params.pass = None;
        let seq = c.arm(&params, dir).unwrap();
        c.note_drawn_camera(|| Some(("package-alias".into(), spawn_camera())));
        c.tick_ended(11);
        c.present(40);
        assert_eq!(c.wanted(40), None);
        assert_eq!(c.finish(seq).unwrap().unwrap_err().code, code::UNAVAILABLE);

        c.set_map_source(|_| Some((MapId([0x1f; 32]), "lockout".into())));
        let seq = c.arm(&params, dir).unwrap();
        c.note_drawn_camera(|| Some(("package-alias".into(), spawn_camera())));
        c.clear_drawn_camera();
        c.tick_ended(12);
        c.present(41);
        assert_eq!(c.wanted(41), None);
        c.note_drawn_camera(|| Some(("package-alias".into(), spawn_camera())));
        c.present(42);
        assert!(c.wanted(42).unwrap().digest);
        c.written(42, Ok(None));
        assert_eq!(c.finish(seq).unwrap().unwrap_err().code, code::UNAVAILABLE);
    }

    fn sealed(dump: bool) -> Result<SceneReply, String> {
        Ok(SceneReply {
            sealed: true,
            map_epoch: 1,
            frame: 7,
            tick: 3,
            hash: "00ff00ff".into(),
            views: vec![agent_proto::SceneViewRecord {
                key: "game:0:0".into(),
                instances: 12,
                ..Default::default()
            }],
            counters: BTreeMap::from([("scenes_sealed".to_owned(), 4)]),
            dump: dump.then(|| "/nowhere/scene.json".to_owned()),
            ..Default::default()
        })
    }

    #[test]
    fn two_ticks_in_one_frame_report_the_later_and_a_new_arming_replaces_the_old() {
        let (_, hosted, c) = rigs();
        hosted.set_scene_source(sealed);
        c.set_writer();
        c.tick_ended(10);
        let first = c.arm(&whole(12), Path::new(DIR)).unwrap();
        let seq = c.arm(&whole(12), Path::new(DIR)).unwrap();
        assert_eq!(c.finish(first).unwrap().unwrap_err().code, code::INTERRUPTED);
        c.tick_ended(11);
        c.tick_ended(12);
        c.tick_ended(13);
        c.present(50);
        assert!(c.wanted(50).is_some());
        c.written(50, Err("readback failed".into()));
        let e = c.finish(seq).unwrap().unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(e.message.contains("readback failed"), "{}", e.message);
        // Default tick: the next one; the scene's hash comes along.
        let seq = c.arm(&FrameCaptureParams::default(), Path::new(DIR)).unwrap();
        c.tick_ended(14);
        c.present(51);
        c.written(51, Ok(None));
        let r = c.finish(seq).unwrap().unwrap();
        assert_eq!((r.tick, r.present), (14, 51));
        assert_eq!(r.scene_hash.as_deref(), Some("00ff00ff"));
    }

    fn draw(color: &str) -> DrawInfo {
        DrawInfo {
            color: color.into(),
            target: "640x480 X8R8G8B8".into(),
            outcome: "drawn".into(),
            vertices: 3,
            ..DrawInfo::default()
        }
    }

    #[test]
    fn a_pass_capture_reads_the_pass_of_that_ticks_frame_through_the_frame_recorder() {
        let (probe, _, c) = rigs();
        c.tick_ended(20);
        let p = FrameCaptureParams {
            tick: Some(22),
            pass: Some(1),
            ..Default::default()
        };
        let seq = c.arm(&p, Path::new(DIR)).unwrap();
        assert!(!probe.armed(), "armed at the end of the tick before");
        // As boot's draw wrapper calls them: the probe, then the capture.
        probe.present(60);
        c.present(60);
        c.tick_ended(21);
        assert!(probe.armed());
        probe.present(61);
        c.present(61);
        c.tick_ended(22);
        assert_eq!(probe.draw(draw("a")), None);
        assert_eq!(probe.draw(draw("b")), None, "pass 0 was not asked for");
        let ask = probe.present(62).unwrap();
        c.present(62);
        assert_eq!(ask.pass, 1);
        assert_eq!(c.finish(seq), None, "the snapshot is still to come");
        probe.snapped(1, Ok("/nowhere/frame-1-pass-1.png".into()));
        let r = c.finish(seq).unwrap().unwrap();
        assert_eq!(r.path, "/nowhere/frame-1-pass-1.png");
        assert_eq!((r.tick, r.present, r.pass), (22, 62, Some(1)));
        assert_eq!(c.wanted(62), None, "no frame writer involved");

        // A pass the frame doesn't have.
        let seq = c.arm(
            &FrameCaptureParams {
                pass: Some(5),
                ..Default::default()
            },
            Path::new(DIR),
        )
        .unwrap();
        probe.present(63);
        c.present(63);
        c.tick_ended(23);
        probe.draw(draw("a"));
        probe.present(64);
        c.present(64);
        let e = c.finish(seq).unwrap().unwrap_err();
        assert!(e.message.contains("no pass 5"), "{}", e.message);
    }

    struct Ticking(u64);

    impl crate::Reader for Ticking {
        fn status(&mut self) -> crate::GameStatus {
            crate::GameStatus {
                tick: Some(self.0),
                ..Default::default()
            }
        }
        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            Ok(Vec::new())
        }
    }

    struct Rig {
        agent: Agent,
        conn: crate::ConnId,
        rx: Receiver<String>,
        hosted: &'static Hosted,
        capture: &'static FrameCapture,
    }

    impl Rig {
        fn new() -> Rig {
            let (_, hosted, capture) = rigs();
            let agent = Agent::new();
            add_frame_methods(&agent, capture, hosted);
            let (tx, rx) = sync_channel(16);
            let conn = agent.connect(tx);
            Rig {
                agent,
                conn,
                rx,
                hosted,
                capture,
            }
        }

        fn send(&self, m: &str, p: Value) {
            self.agent.submit(
                self.conn,
                &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
            );
        }

        fn tick(&self, tick: u64) -> Option<Value> {
            self.agent.on_tick(&mut Ticking(tick));
            self.rx
                .recv_timeout(Duration::from_millis(200))
                .ok()
                .map(|l| serde_json::from_str(&l).unwrap())
        }

        fn call(&self, m: &str, p: Value, tick: u64) -> Value {
            self.send(m, p);
            self.tick(tick).expect("an answer")
        }
    }

    fn parity(request: ParityRequest) {
        request.finish(Ok(agent_proto::ParityReply {
            tick: Some(6),
            compare: agent_proto::CompareReply {
                size: [4, 4],
                max_channel: 6,
                within_bar: true,
                ..Default::default()
            },
        }));
    }

    #[test]
    fn scene_and_parity_say_why_without_a_source_and_pass_one_through() {
        let r = Rig::new();
        let e = r.call(method::RENDER_SCENE, Value::Null, 1);
        assert_eq!(e["error"]["code"], code::UNAVAILABLE, "{e}");
        assert!(
            e["error"]["message"]
                .as_str()
                .unwrap()
                .contains("no scene has been built")
        );
        let e = r.call(method::RENDER_PARITY, json!({"arm": true}), 2);
        assert_eq!(e["error"]["code"], code::UNAVAILABLE, "{e}");
        assert!(e["error"]["message"].as_str().unwrap().contains("private renderer"));
        let e = r.call(method::RENDER_PARITY, json!({"arm": false}), 3);
        assert_eq!(e["error"]["code"], code::INVALID_PARAMS, "{e}");

        r.hosted.set_scene_source(sealed);
        let s = r.call(method::RENDER_SCENE, Value::Null, 4);
        assert_eq!(s["result"]["sealed"], true, "{s}");
        assert_eq!(s["result"]["hash"], "00ff00ff");
        assert_eq!(s["result"]["views"][0]["key"], "game:0:0");
        assert!(s["result"].get("dump").is_none());
        let s = r.call(method::RENDER_SCENE, json!({"dump": true}), 5);
        assert_eq!(s["result"]["dump"], "/nowhere/scene.json");

        r.hosted.set_parity_source(Some(parity));
        r.capture.tick_ended(6);
        r.send(method::RENDER_PARITY, json!({"arm": true}));
        assert_eq!(r.tick(6), None, "answered after the frame step");
        r.capture.tick_ended(7);
        let p = r.tick(7).expect("the comparison");
        assert_eq!(p["result"]["tick"], 6, "{p}");
        assert_eq!(p["result"]["max_channel"], 6);
        assert_eq!(p["result"]["within_bar"], true);
    }

    #[test]
    fn a_lighting_capture_needs_a_private_renderer_even_with_a_present_writer() {
        let r = Rig::new();
        r.capture.set_writer();
        r.capture.tick_ended(7);
        for (tick, params, code) in [
            (7, json!({"lighting": "e3"}), code::UNAVAILABLE),
            (8, json!({"pass": 1, "lighting": "retail"}), code::INVALID_PARAMS),
            (9, json!({"lighting": "sharp"}), code::INVALID_PARAMS),
        ] {
            let out = r.call(method::RENDER_CAPTURE, params, tick);
            assert_eq!(out["error"]["code"], code, "{out}");
        }
        r.capture.present(30);
        assert!(r.capture.wanted(30).is_none());

        // The renderer can answer while the guest is held on this
        // boundary. A private draw should not need a simulation step.
        r.capture.set_private_source(Some(|_| Err(RpcError::new(code::UNAVAILABLE, "tick expired"))));
        let out = r.call(method::RENDER_CAPTURE, json!({"lighting": "retail", "tick": 3}), 10);
        assert_eq!(out["error"]["message"], "tick expired", "{out}");
    }

    #[test]
    fn capture_compare_and_the_method_list_through_the_agent() {
        let r = Rig::new();
        let m = r.call(method::METHODS, Value::Null, 1);
        for name in [
            method::RENDER_COMPARE,
            method::RENDER_CAPTURE,
            method::RENDER_SCENE,
            method::RENDER_PARITY,
        ] {
            assert!(m["result"].as_array().unwrap().contains(&json!(name)), "{name}");
        }
        let e = r.call(
            method::RENDER_COMPARE,
            json!({"a": "/nowhere/a.png", "b": "/nowhere/b.png"}),
            2,
        );
        assert_eq!(e["error"]["code"], code::INVALID_PARAMS, "{e}");
        assert_eq!(e["error"]["data"]["error"], "read");

        r.capture.set_writer();
        r.capture.tick_ended(7);
        r.send(method::RENDER_CAPTURE, json!({}));
        assert_eq!(r.tick(7), None, "waits for its frame");
        r.capture.tick_ended(8);
        r.capture.present(30);
        assert!(r.capture.wanted(30).is_some());
        r.capture.written(30, Ok(None));
        let c = r.tick(8).expect("the capture");
        assert_eq!(c["result"]["tick"], 8, "{c}");
        assert_eq!(c["result"]["present"], 30);
        assert_eq!(c["result"]["scene_hash"], Value::Null);
        assert!(c["result"]["at"].is_object(), "stamped");
    }
}
