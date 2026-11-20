//! Draw diagnostics for the boot tracer: a [`Backend`] wrapper that keeps a
//! histogram of what reaches the backend and, with `H2_BOOT_DRAWLOG=<file>`,
//! writes one line per draw for the frames in `H2_BOOT_DRAWLOG_FRAMES`
//! (comma-separated present counts, default `240`).
//!
//! Frame N is the draws after the (N-1)th present, so it matches
//! `frame-N.png` from `H2_BOOT_GFX`.
//!
//! `H2_BOOT_CAPTURE=<dir>` records what a backend needs to redraw frame
//! `H2_BOOT_CAPTURE_FRAME` (default 480) offline:
//!
//! - `textures.txt` and `tex-ID-L.bin`: every texture uploaded in the run
//!   (descriptor, host format, and each decoded level's bytes), by ID;
//! - `surfaces.txt`: every surface created and its handle;
//! - `writes.txt` and `write-N.bin`: guest texels written into surfaces;
//! - `frame.txt`: the frame's backend calls in order. A draw is the full
//!   `ResolvedDraw` (`{:#?}`) with its vertex streams and indices moved to
//!   `draw-K-sI.bin` and `draw-K-idx.bin` (u32 LE), and its textures as
//!   the IDs above.
//!
//! - `snap-K.bin`: with `H2_BOOT_CAPTURE_SNAP=K[-L][,...]`, the colour
//!   target read back (guest bytes, `read_surface`) after draw K;
//! - `H2_BOOT_CAPTURE_SKIP=K[-L][,...]` drops those draws, to see what
//!   each contributes;
//! - `H2_BOOT_CAPTURE_LITE=1` writes no texture or guest-write bytes.
//!
//! The format is Rust `Debug` text: d3d and gfx types have no serde.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::{Arc, Mutex};

use d3d::backend::{
    Backend, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw, StreamBytes, TargetInfo,
    TextureInfo,
};
use gfx::texture::{DecodedTexture, TextureDesc, TextureKind};
use gfx::{Clear, D3DFormat};

/// Presents so far, for timelines keyed on frames (`H2_BOOT_INPUT`).
static PRESENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Presents seen by any [`DiagBackend`] so far.
pub fn presents() -> u64 {
    PRESENTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Counts a present sim mode skipped with the world render
/// (`sim_bridge::sim_frame`), so what counts presents (`H2_BOOT_STOP_AT_PRESENT`,
/// the input script, the agent's stamps) moves on as in a rendered run. No
/// frame time is kept for it.
pub fn note_skipped_present() {
    PRESENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A descriptor-only lobby swap still advances host frame timing.
#[cfg(not(feature = "desktop-runtime"))]
pub fn note_descriptor_present() {
    let present = PRESENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    crate::host_perf::on_present(present);
}

/// Wall-clock time of every present, for the frame-rate report.
static KEEP_PRESENT_HISTORY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static PRESENT_TIMES: Mutex<Vec<std::time::Instant>> = Mutex::new(Vec::new());

pub fn keep_present_history(keep: bool) {
    KEEP_PRESENT_HISTORY.store(keep, std::sync::atomic::Ordering::Relaxed);
    if !keep {
        *PRESENT_TIMES.lock().unwrap() = Vec::new();
    }
}

/// Wall-clock frame rate: presents per second over the run since `start`,
/// and over a window with its mean, median and worst frame time: from
/// present `from` (1-based) when given, else the last `tail` presents.
/// `None` before two presents.
pub fn frame_rate(start: std::time::Instant, tail: usize, from: Option<usize>) -> Option<String> {
    let times = PRESENT_TIMES.lock().unwrap();
    let n = times.len();
    if n < 2 {
        return None;
    }
    let total = times[n - 1].duration_since(start).as_secs_f64();
    let from = match from {
        Some(f) => f.saturating_sub(1).min(n - 2),
        None => n.saturating_sub(tail + 1),
    };
    let mut gaps: Vec<f64> = times[from..]
        .windows(2)
        .map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1e3)
        .collect();
    let span: f64 = gaps.iter().sum();
    gaps.sort_by(f64::total_cmp);
    Some(format!(
        "frames: {n} presents in {total:.1}s ({:.2} fps); presents {}-{n}: {:.2} fps, frame ms mean {:.0} median {:.0} max {:.0}",
        n as f64 / total,
        from + 1,
        gaps.len() as f64 * 1e3 / span,
        span / gaps.len() as f64,
        gaps[gaps.len() / 2],
        gaps[gaps.len() - 1],
    ))
}

/// What the wrapper saw over the run.
///
/// Counted from each draw outcome. The keys are plain values and the names
/// are only formatted for [`DrawStats::report`].
#[derive(Default)]
pub struct DrawStats {
    pub presents: u64,
    /// (outcome, target) -> draws; the outcome is `None` for drawn.
    by_outcome: rustc_hash::FxHashMap<(Option<String>, Target), u64>,
    /// Texture (format, kind) bound to drawn draws -> draws.
    textures: rustc_hash::FxHashMap<(D3DFormat, TextureKind), u64>,
    /// Frames (numbered as `frame-N.png`) with a drawn draw that samples
    /// a depth texture: Halo's shadow apply (layer 11) reading a shadow
    /// map. Worker fallbacks can land after the guest counts a newer frame.
    shadow_frames: BTreeSet<u64>,
}

/// [`DrawStats::shadow_frames`] of the run, for the screenshot hook
/// (`H2_BOOT_GFX_SHADOWS`), which may run on the render thread.
static SHADOW_FRAMES: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

/// Whether frame `n` (as `frame-N.png` numbers it) sampled a shadow map.
/// Worker fallback draws complete before the present on the same queue.
pub fn samples_shadow_map(n: u64) -> bool {
    SHADOW_FRAMES.lock().unwrap().contains(&n)
}

/// `frames` (ascending) as ranges, `a-b` or `a`, the first `max` of them.
fn ranges(frames: &[u64], max: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < frames.len() {
        let mut j = i;
        while j + 1 < frames.len() && frames[j + 1] == frames[j] + 1 {
            j += 1;
        }
        if out.len() == max {
            out.push("...".into());
            break;
        }
        out.push(if i == j {
            format!("{}", frames[i])
        } else {
            format!("{}-{}", frames[i], frames[j])
        });
        i = j + 1;
    }
    out.join(", ")
}

/// A draw target's width, height and format.
type Target = (u32, u32, D3DFormat);

fn target_name(t: &TargetInfo) -> String {
    format!("{}x{} {:?}", t.width, t.height, t.format)
}

impl DrawStats {
    /// (outcome, target WxH format) -> draws, by name.
    pub fn by_outcome(&self) -> BTreeMap<String, u64> {
        let mut m = BTreeMap::new();
        for ((out, (w, h, f)), v) in &self.by_outcome {
            let out = out.as_deref().unwrap_or("drawn");
            *m.entry(format!("{out} -> {w}x{h} {f:?}")).or_default() += v;
        }
        m
    }

    /// Texture formats bound to drawn draws -> draws, by name.
    pub fn textures(&self) -> BTreeMap<String, u64> {
        let mut m = BTreeMap::new();
        for ((f, k), v) in &self.textures {
            *m.entry(format!("{f:?} {k:?}")).or_default() += v;
        }
        m
    }

    /// Adds one draw. `Some(frame)`: the first draw of this frame to
    /// sample a depth texture (a shadow map).
    fn add(
        &mut self,
        r: &Result<DrawOutcome, String>,
        t: &TargetInfo,
        info: &[Option<TextureInfo>; 4],
    ) -> Option<u64> {
        self.add_at(self.presents + 1, r, t, info)
    }

    fn add_at(
        &mut self,
        frame: u64,
        r: &Result<DrawOutcome, String>,
        t: &TargetInfo,
        info: &[Option<TextureInfo>; 4],
    ) -> Option<u64> {
        let drawn = matches!(r, Ok(DrawOutcome::Drawn));
        let out = (!drawn).then(|| outcome_name(r));
        *self
            .by_outcome
            .entry((out, (t.width, t.height, t.format)))
            .or_default() += 1;
        if !drawn {
            return None;
        }
        for ti in info.iter().flatten() {
            *self.textures.entry((ti.format, ti.kind)).or_default() += 1;
        }
        let shadow = info.iter().flatten().any(|t| t.format.is_depth());
        (shadow && self.shadow_frames.insert(frame)).then_some(frame)
    }

    pub fn report(&self) -> String {
        let mut s = format!("draw diag: {} presents\n", self.presents);
        for (k, v) in &self.by_outcome() {
            s += &format!("  draws {v:>8}  {k}\n");
        }
        for (k, v) in &self.textures() {
            s += &format!("  texture {v:>8}  {k}\n");
        }
        if !self.shadow_frames.is_empty() {
            s += &format!(
                "  shadow-map frames {:>3}  {}\n",
                self.shadow_frames.len(),
                ranges(&self.shadow_frames.iter().copied().collect::<Vec<_>>(), 40)
            );
        }
        s
    }
}

pub struct DiagBackend<B: Backend> {
    inner: B,
    /// The agent's `render.frame` recorder (`agent::render::PROBE`; a
    /// test's own in tests).
    probe: &'static agent::render::FrameProbe,
    owner_probe: bool,
    /// The current pass's colour target and size, while a frame is being
    /// recorded, so a pass asked for can be read back when it ends.
    probe_target: Option<(B::Surface, u32, u32)>,
    stats: Arc<Mutex<DrawStats>>,
    log: Option<std::io::BufWriter<std::fs::File>>,
    frames: BTreeSet<u64>,
    in_frame: u64,
    capture: Option<Capture>,
    next_texture: u64,
    /// `H2_SWEEP_DIAG` (see [`crate::sweep_diag`]).
    sweep: Option<crate::sweep_diag::SweepDiag>,
    /// With `H2_SWEEP_DIAG`, until its frame is logged: where each host
    /// texture came from, by ID.
    origins: BTreeMap<u64, String>,
}

/// A host texture with the ID the capture names it by.
pub struct Tracked<T> {
    pub id: u64,
    pub inner: T,
}

struct Capture {
    dir: std::path::PathBuf,
    frame: u64,
    draws: u64,
    writes: u64,
    snaps: BTreeSet<u64>,
    skips: BTreeSet<u64>,
    /// `H2_BOOT_CAPTURE_LITE`: no texture or guest-write bytes (frame.txt,
    /// streams and snapshots only), for quick looks at one frame.
    lite: bool,
    /// `H2_BOOT_CAPTURE_TEX=K[-L],...`: texture upload ids whose bytes are
    /// kept even with `lite` (ids as in textures.txt and frame.txt).
    keep_tex: BTreeSet<u64>,
}

/// `K[-L][,...]` as a set of draw indices.
fn draw_set(var: &str) -> BTreeSet<u64> {
    let spec = std::env::var(var).unwrap_or_default();
    let mut set = BTreeSet::new();
    for item in spec.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let (a, b) = item.split_once('-').unwrap_or((item, item));
        if let (Ok(a), Ok(b)) = (a.trim().parse::<u64>(), b.trim().parse::<u64>()) {
            set.extend(a..=b);
        }
    }
    set
}

impl Capture {
    fn append(&self, file: &str, s: &str) {
        let path = self.dir.join(file);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{s}");
        }
    }
    fn blob(&self, file: &str, bytes: &[u8]) {
        if self.lite && (file.starts_with("tex-") || file.starts_with("write-")) {
            return;
        }
        let _ = std::fs::write(self.dir.join(file), bytes);
    }
}

impl<B: Backend> DiagBackend<B> {
    pub fn new(inner: B) -> (Self, Arc<Mutex<DrawStats>>) {
        let stats = Arc::new(Mutex::new(DrawStats::default()));
        let log =
            std::env::var_os("H2_BOOT_DRAWLOG").and_then(|p| match std::fs::File::create(&p) {
                Ok(f) => Some(std::io::BufWriter::new(f)),
                Err(e) => {
                    eprintln!("boot: H2_BOOT_DRAWLOG {}: {e}", p.to_string_lossy());
                    None
                }
            });
        let frames = std::env::var("H2_BOOT_DRAWLOG_FRAMES")
            .unwrap_or_else(|_| "240".into())
            .split(',')
            .filter_map(|f| f.trim().parse().ok())
            .collect();
        let capture = std::env::var_os("H2_BOOT_CAPTURE").and_then(|d| {
            let dir = std::path::PathBuf::from(d);
            std::fs::create_dir_all(&dir)
                .map_err(|e| eprintln!("boot: H2_BOOT_CAPTURE {}: {e}", dir.display()))
                .ok()?;
            let frame = std::env::var("H2_BOOT_CAPTURE_FRAME")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(480);
            Some(Capture {
                dir,
                frame,
                draws: 0,
                writes: 0,
                snaps: draw_set("H2_BOOT_CAPTURE_SNAP"),
                skips: draw_set("H2_BOOT_CAPTURE_SKIP"),
                lite: std::env::var_os("H2_BOOT_CAPTURE_LITE").is_some(),
                keep_tex: draw_set("H2_BOOT_CAPTURE_TEX"),
            })
        });
        let sweep = crate::sweep_diag::SweepDiag::from_env();
        (
            DiagBackend {
                inner,
                probe: &agent::render::PROBE,
                owner_probe: false,
                probe_target: None,
                stats: Arc::clone(&stats),
                log,
                frames,
                in_frame: 1,
                capture,
                next_texture: 0,
                sweep,
                origins: BTreeMap::new(),
            },
            stats,
        )
    }

    /// The renderer reports a final routing decision for every Swap of this device.
    pub fn with_owner_probe(mut self, enabled: bool) -> Self {
        self.owner_probe = enabled;
        self
    }

    /// The same, recording frames for `render.frame` into `probe`.
    pub fn with_probe(mut self, probe: &'static agent::render::FrameProbe) -> Self {
        self.probe = probe;
        self
    }

    /// The capture, while frame `H2_BOOT_CAPTURE_FRAME` is being drawn.
    fn capturing(&mut self) -> Option<&mut Capture> {
        let f = self.in_frame;
        self.capture.as_mut().filter(|c| c.frame == f)
    }

    /// `H2_SWEEP_DIAG`: reads `color` back and logs it, in the frame being
    /// logged.
    fn sweep_samples(&mut self, color: B::Surface, width: u32, height: u32, what: &str)
    where
        B: Backend,
    {
        if !self
            .sweep
            .as_ref()
            .is_some_and(|s| s.dumping(self.in_frame))
        {
            return;
        }
        let layout = GuestLayoutKind::Linear { pitch: width * 4 };
        let r = self
            .inner
            .read_surface(color, D3DFormat::LinA8R8G8B8, width, height, layout);
        if let Some(s) = &mut self.sweep {
            match r {
                Ok(bytes) => s.samples(what, &bytes, width, height),
                Err(e) => s.line(&format!("  {what}: read back failed: {e}")),
            }
        }
    }

    /// `H2_SWEEP_DIAG`: notes where host texture `id` came from.
    fn origin(&mut self, id: u64, what: impl FnOnce() -> String) {
        if self.sweep.as_ref().is_some_and(|s| s.active()) {
            self.origins
                .insert(id, format!("{} (frame {})", what(), self.in_frame));
        }
    }

    fn logging(&self) -> bool {
        self.log.is_some() && self.frames.contains(&self.in_frame)
    }

    fn line(&mut self, s: String) {
        if self.logging()
            && let Some(l) = &mut self.log
        {
            let _ = writeln!(l, "{} {s}", self.in_frame);
        }
    }

    /// `render.frame`: work is about to go to `color`; reads back the pass
    /// it ends when that pass was asked for.
    fn probe_enter(
        &mut self,
        color: B::Surface,
        depth: Option<B::Surface>,
        size: Option<(u32, u32)>,
    ) {
        let probe = self.probe;
        if !probe.armed() {
            self.probe_target = None;
            return;
        }
        let depth_name = depth.map(|d| format!("{d:?}"));
        if let Some(ask) = probe.enter(&format!("{color:?}"), depth_name.as_deref()) {
            self.probe_snap(ask);
        }
        match size {
            Some((w, h)) => self.probe_target = Some((color, w, h)),
            None if self.probe_target.is_some_and(|t| t.0 != color) => self.probe_target = None,
            None => {}
        }
    }

    /// Reads the current pass's colour target back into a PNG under the
    /// cache directory's `frames/`, and tells the probe.
    fn probe_snap(&mut self, ask: agent::render::SnapAsk) {
        let result = match self.probe_target {
            None => Err("the pass's size is unknown (no draw in it)".to_owned()),
            Some((surface, w, h)) => {
                let layout = GuestLayoutKind::Linear { pitch: w * 4 };
                self.inner
                    .read_surface(surface, D3DFormat::LinA8R8G8B8, w, h, layout)
                    .and_then(|bgra| {
                        let rgba: Vec<u8> = bgra
                            .chunks_exact(4)
                            .flat_map(|p| [p[2], p[1], p[0], 0xff])
                            .collect();
                        let dir = agent_proto::cache_dir().join("frames");
                        let path = dir.join(format!("frame-{}-pass-{}.png", ask.seq, ask.pass));
                        std::fs::create_dir_all(&dir)
                            .map_err(|e| e.to_string())
                            .and_then(|()| {
                                crate::d3d_glue::write_png(&path, &rgba, w, h)
                                    .map_err(|e| e.to_string())
                            })
                            .map(|()| path.display().to_string())
                    })
            }
        };
        self.probe.snapped_for(ask, result);
    }
}

/// A draw for `render.frame`.
pub(crate) fn probe_draw<S: std::fmt::Debug>(
    color: S,
    depth: Option<S>,
    draw: &ResolvedDraw,
    r: &Result<DrawOutcome, String>,
) -> agent::render::DrawInfo {
    let t = &draw.target;
    let st = &draw.state.render;
    agent::render::DrawInfo {
        color: format!("{color:?}"),
        depth: depth.map(|d| format!("{d:?}")),
        target: target_name(t),
        entry: draw.entry.to_owned(),
        primitive: format!("{:?}", draw.primitive),
        vertices: draw.vertex_count,
        indices: draw.indices.as_ref().map_or(0, |i| i.len() as u32),
        outcome: outcome_name(r),
        textures: draw
            .texture_info
            .iter()
            .flatten()
            .map(|t| format!("{:?}", t.format))
            .collect(),
        program: draw.shader.pixel_shader.is_some(),
        blend: st.blend_enable,
        depth_write: st.depth_write,
        vs: vertex_name(&draw.shader.vertex),
        depth_state: Some(depth_record(draw)),
        constants: Some(draw.shader.constants.clone() as Arc<[[f32; 4]]>),
    }
}

/// The vertex side of a draw for `render.frame`: `fvf 0x…`, or `prog
/// @start n=slots #hash` (the low 32 bits of the words' FNV-1a hash).
fn vertex_name(v: &d3d::shader::VertexProcessing) -> String {
    match v {
        d3d::shader::VertexProcessing::FixedFunction { fvf } => format!("fvf {fvf:#x}"),
        d3d::shader::VertexProcessing::Program { start, microcode } => format!(
            "prog @{start} n={} #{:08x}",
            microcode.len() / 4,
            microcode.words_hash() as u32
        ),
    }
}

/// A draw's depth state for `render.frame`.
fn depth_record(draw: &ResolvedDraw) -> agent_proto::DepthRecord {
    use gfx::state::DepthBufferMode;
    let st = &draw.state.render;
    let hw = &draw.hw;
    agent_proto::DepthRecord {
        mode: match st.depth_mode {
            DepthBufferMode::Disabled => "off",
            DepthBufferMode::Z => "z",
            DepthBufferMode::W => "w",
        }
        .into(),
        w: d3d::hw::w_buffered(draw),
        func: format!("{:?}", st.depth_func),
        write: st.depth_write,
        viewport: [draw.viewport.min_z, draw.viewport.max_z],
        scale: [hw.viewport_scale[2], hw.viewport_offset[2]],
        clip: [hw.clip_min, hw.clip_max],
        full: if draw.target.depth_format.is_some() {
            gfx::format::d3dz_max(draw.target.depth_format)
        } else {
            0.0
        },
        clamp: d3d::hw::draw_clamps_depth(draw),
    }
}

fn outcome_name(r: &Result<DrawOutcome, String>) -> String {
    match r {
        Ok(DrawOutcome::Drawn) => "drawn".into(),
        Ok(DrawOutcome::Skipped(why)) => format!("skipped: {why}"),
        Ok(DrawOutcome::Unsupported(why)) => format!("unsupported: {why}"),
        Err(e) => format!("error: {e}"),
    }
}

impl<B: Backend> DiagBackend<B> {
    fn inspects_draw(&mut self) -> bool {
        self.capturing().is_some() || self.logging() || self.sweep.is_some() || self.probe.armed()
    }

    fn diagnosed_draw(
        &mut self,
        color: B::Surface,
        depth: Option<B::Surface>,
        draw: &ResolvedDraw,
        shared: Option<&Arc<ResolvedDraw>>,
        textures: [Option<&Tracked<B::Texture>>; 4],
    ) -> Result<DrawOutcome, String> {
        let ids: Vec<Option<u64>> = textures.iter().map(|t| t.map(|t| t.id)).collect();
        let mut snap = None;
        if let Some(c) = self.capturing() {
            let k = c.draws;
            if c.skips.contains(&k) {
                c.draws += 1;
                c.append(
                    "frame.txt",
                    &format!("draw {k} skipped (H2_BOOT_CAPTURE_SKIP)"),
                );
                return Ok(DrawOutcome::Drawn);
            }
            if c.snaps.contains(&k) {
                snap = Some((k, c.dir.join(format!("snap-{k}.bin"))));
            }
            c.draws += 1;
            let mut d = draw.clone();
            for (i, st) in d.streams.iter_mut().enumerate() {
                c.blob(&format!("draw-{k}-s{i}.bin"), &std::mem::take(&mut st.data));
            }
            if let Some(idx) = d.indices.take() {
                let bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
                c.blob(&format!("draw-{k}-idx.bin"), &bytes);
            }
            c.append(
                "frame.txt",
                &format!("draw {k} color={color:?} depth={depth:?} textures={ids:?}\n{d:#?}"),
            );
        }
        let sweep_logging = self
            .sweep
            .as_ref()
            .is_some_and(|s| s.dumping(self.in_frame));
        if sweep_logging && self.sweep.as_ref().is_some_and(|s| s.logged_draws() == 0) {
            self.sweep_samples(
                color,
                draw.target.width,
                draw.target.height,
                "target before the frame's first draw",
            );
        }
        let probing = self.probe.armed();
        if probing {
            self.probe_enter(color, depth, Some((draw.target.width, draw.target.height)));
        }
        let inner: [Option<&B::Texture>; 4] =
            std::array::from_fn(|i| textures[i].map(|t| &t.inner));
        let r = match shared {
            Some(packet) => self.inner.draw_shared(color, depth, Arc::clone(packet), inner).0,
            None => self.inner.draw(color, depth, draw, inner),
        };
        if probing {
            self.probe.draw(probe_draw(color, depth, draw, &r));
        }
        if let Some(s) = &mut self.sweep {
            if s.dumping(self.in_frame) {
                s.draw(draw, &format!("{color:?}"), &outcome_name(&r));
                let src: Vec<String> = ids
                    .iter()
                    .enumerate()
                    .filter_map(|(i, id)| {
                        id.map(|id| {
                            let o = self
                                .origins
                                .get(&id)
                                .map_or("made before the log", String::as_str);
                            format!("t{i}=tex {id}: {o}")
                        })
                    })
                    .collect();
                s.line(&format!("  src [{}]", src.join("; ")));
            }
            s.saw_draw(self.in_frame, draw);
        }
        if sweep_logging {
            self.sweep_samples(color, draw.target.width, draw.target.height, "target after");
        }
        if let Some((k, path)) = snap {
            let t = &draw.target;
            let layout = GuestLayoutKind::Linear { pitch: t.width * 4 };
            match self
                .inner
                .read_surface(color, D3DFormat::LinA8R8G8B8, t.width, t.height, layout)
            {
                Ok(bytes) => {
                    let _ = std::fs::write(&path, bytes);
                }
                Err(e) => eprintln!("boot: snap after draw {k}: {e}"),
            }
        }
        let (out, target) = self.count(&r, &draw.target, &draw.texture_info);
        if self.logging() {
            let tex: Vec<String> = draw
                .texture_info
                .iter()
                .enumerate()
                .filter_map(|(i, t)| {
                    t.map(|t| {
                        format!(
                            "t{i}={:?} {}x{} @{:#x}",
                            t.format, t.width, t.height, t.header
                        )
                    })
                })
                .collect();
            let st = &draw.state;
            self.line(format!(
                "draw {} {:?} n={} idx={} -> {color:?} ({target}) clip={:?} ps={} vs={:?} tex=[{}] blend={}:{:?}/{:?} atest={}:{:?}/{} z={:?}/{} cw={:?} cull={:?} {}",
                draw.entry,
                draw.primitive,
                draw.vertex_count,
                draw.indices.as_ref().map_or(0, |i| i.len()),
                draw.clip,
                if draw.shader.pixel_shader.is_some() { "program" } else { "fixed" },
                std::mem::discriminant(&draw.shader.vertex),
                tex.join(" "),
                st.render.blend_enable,
                st.render.src_blend,
                st.render.dst_blend,
                st.render.alpha_test_enable,
                st.render.alpha_func,
                st.render.alpha_ref,
                st.render.depth_func,
                st.render.depth_write,
                st.render.color_write,
                st.render.cull_mode,
                out
            ));
        }
        if self.logging() && std::env::var_os("H2_BOOT_DRAWLOG_VERTS").is_some() {
            self.line(format!("  layout {:?}", draw.layout));
            for (i, st) in draw.streams.iter().enumerate() {
                let stride = st.stride.max(4) as usize;
                for v in st.data.chunks(stride).take(8) {
                    let words: Vec<String> = v
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|w| {
                            let f = f32::from_le_bytes(*w);
                            if f.is_finite() && (f == 0.0 || (1e-4..1e5).contains(&f.abs())) {
                                format!("{f}")
                            } else {
                                format!("#{:08x}", u32::from_le_bytes(*w))
                            }
                        })
                        .collect();
                    self.line(format!("  s{i} {}", words.join(" ")));
                }
            }
        }
        r
    }

    /// Adds a draw's outcome to the stats; returns the outcome's and the
    /// target's names.
    fn count(
        &self,
        r: &Result<DrawOutcome, String>,
        t: &TargetInfo,
        info: &[Option<TextureInfo>; 4],
    ) -> (String, String) {
        self.tally(r, t, info);
        (outcome_name(r), target_name(t))
    }

    /// [`Self::count`] without the names: the path every draw takes.
    fn tally(
        &self,
        r: &Result<DrawOutcome, String>,
        t: &TargetInfo,
        info: &[Option<TextureInfo>; 4],
    ) {
        let frame = self.stats.lock().unwrap().add(r, t, info);
        if let Some(n) = frame {
            SHADOW_FRAMES.lock().unwrap().insert(n);
        }
    }
}

impl<B: Backend> Backend for DiagBackend<B> {
    type Surface = B::Surface;
    type Texture = Tracked<B::Texture>;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String> {
        let r = self.inner.create_surface(format, width, height, depth);
        self.line(format!(
            "create_surface {format:?} {width}x{height} depth={depth} -> {r:?}"
        ));
        if let Some(c) = &self.capture {
            c.append(
                "surfaces.txt",
                &format!(
                    "frame {} create_surface {format:?} {width}x{height} depth={depth} -> {r:?}",
                    self.in_frame
                ),
            );
        }
        r
    }
    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        self.inner.destroy_surface(surface)
    }
    // Every provided method is forwarded: a default here would silently
    // replace the inner backend's (a frame-buffer snapshot would skip draws).
    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.snapshot_surface(surface, opaque)?;
        self.line(format!(
            "snapshot_surface {surface:?} opaque={opaque} -> {}",
            if r.is_some() { "ok" } else { "unsupported" }
        ));
        Ok(r.map(|inner| {
            let id = self.next_texture;
            self.next_texture += 1;
            if let Some(c) = self.capturing() {
                c.append(
                    "frame.txt",
                    &format!("snapshot_surface {surface:?} opaque={opaque} -> tex {id}"),
                );
            }
            self.origin(id, || {
                format!("copy of surface {surface:?} opaque={opaque}")
            });
            Tracked { id, inner }
        }))
    }
    fn judge(&self) -> Option<Box<dyn d3d::backend::DrawJudge>> {
        self.inner.judge()
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
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
        layout: d3d::backend::GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.inner
            .mirror_store(surface, format, width, height, layout, offset)
    }
    fn mirror_load(
        &mut self,
        surface: Self::Surface,
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
        layout: d3d::backend::GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.inner
            .mirror_load(surface, format, width, height, layout, offset)
    }
    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.inner.mirror_read(offset, len)
    }
    fn handles_surface_aliases(&self) -> bool { self.inner.handles_surface_aliases() }

    fn defers_depth_aliases(&self) -> bool {
        self.inner.defers_depth_aliases()
    }
    #[allow(clippy::too_many_arguments)]
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
        let r = self.inner.depth_alias_texture(
            depth, format, width, height, pitch, generation, offset, desc, options,
        )?;
        Ok(r.map(|inner| {
            let id = self.next_texture;
            self.next_texture += 1;
            self.origin(id, || {
                format!("depth bits of surface {depth:?} ({format:?} {width}x{height}) at +{offset:#x} generation {generation}")
            });
            Tracked { id, inner }
        }))
    }
    fn can_sample_surfaces(&self) -> bool {
        self.inner.can_sample_surfaces()
    }
    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.surface_texture(surface)?;
        Ok(r.map(|inner| {
            let id = self.next_texture;
            self.next_texture += 1;
            if let Some(c) = self.capturing() {
                c.append(
                    "frame.txt",
                    &format!("surface_texture {surface:?} -> tex {id}"),
                );
            }
            self.origin(id, || {
                format!("render-to-texture surface {surface:?} in place")
            });
            Tracked { id, inner }
        }))
    }
    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.inner.clear_pixel_count()
    }
    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.inner.report_pixel_count()
    }
    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.inner.read_pixel_count(ticket)
    }
    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        self.inner.try_pixel_count(ticket)
    }
    fn pixel_count_unsubmitted(&self, ticket: u64) -> bool {
        self.inner.pixel_count_unsubmitted(ticket)
    }
    fn can_sample_depth_surfaces(&self) -> bool {
        self.inner.can_sample_depth_surfaces()
    }
    #[cfg(feature = "gpu")]
    fn external_attachments(
        &mut self, surfaces: d3d::ExternalSurfaces<Self::Surface>, textures: [Option<&Self::Texture>; 4],
        copy: d3d::TargetCopy,
    ) -> Result<d3d::ExternalCopies, String> {
        self.inner.external_attachments(surfaces, textures.map(|t| t.map(|t| &t.inner)), copy)
    }

    fn prepare_external(
        &mut self, surfaces: d3d::ExternalSurfaces<Self::Surface>, request: d3d::ExternalPassRequest,
        textures: [Option<&Self::Texture>; 4], plan: &d3d::ExternalPlan, copy: d3d::TargetCopy, pass: d3d::SharedReplacement,
    ) -> Result<Option<Vec<d3d::ExternalRecording>>, String> {
        self.inner.prepare_external(surfaces, request, textures.map(|t| t.map(|t| &t.inner)), plan, copy, pass)
    }

    fn reusable_external_attachments(&self) -> bool {
        self.inner.reusable_external_attachments()
    }

    // Scene stages (docs/graphics/framescene.md, slice 2): typed native
    // stages, not guest draws; the probes and captures see the frame they
    // join, as with the other native passes.
    fn runs_scene_stages(&self) -> bool {
        self.inner.runs_scene_stages()
    }

    fn run_scene_stage(&mut self, stage: d3d::SceneStage<Self::Surface>) -> Result<bool, String> {
        self.inner.run_scene_stage(stage)
    }

    fn accepts_scene_pass(&self, pass: &dyn d3d::ExternalPass) -> bool {
        self.inner.accepts_scene_pass(pass)
    }

    fn queues_scene_stages(&self) -> bool {
        self.inner.queues_scene_stages()
    }

    fn queue_scene_stage(&mut self, stage: d3d::SceneStage<Self::Surface>) -> bool {
        self.inner.queue_scene_stage(stage)
    }

    fn queue_external_draw(
        &mut self, mut candidate: d3d::backend::QueuedExternalDraw<Self::Surface>,
        textures: [Option<&Self::Texture>; 4],
    ) -> bool {
        let draw = &candidate.fallback;
        if self.capturing().is_some() || self.logging() || self.sweep.is_some() || self.probe.armed() {
            return false;
        }
        let stats = Arc::clone(&self.stats);
        let frame = self.in_frame;
        let (target, info) = (draw.target, draw.texture_info);
        let finished = candidate.finished;
        candidate.finished = Box::new(move |fallback| {
            if let Some(r) = fallback {
                let shadow = stats.lock().unwrap().add_at(frame, r, &target, &info);
                if let Some(frame) = shadow {
                    SHADOW_FRAMES.lock().unwrap().insert(frame);
                }
            }
            finished(fallback);
        });
        self.inner.queue_external_draw(candidate, textures.map(|t| t.map(|t| &t.inner)))
    }

    fn commit_external(&mut self, recording: d3d::ExternalRecording) {
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
        request: d3d::ExternalPassRequest,
        pass: Box<dyn d3d::ExternalPass>,
    ) -> Result<bool, String> {
        self.inner.external_pass(color, depth, sampled, request, pass)
    }
    fn warp_surface(
        &mut self,
        src: Self::Surface,
        dst: Self::Surface,
        h: [[f32; 3]; 3],
    ) -> Result<bool, String> {
        self.inner.warp_surface(src, dst, h)
    }
    fn surface_extent(&self, surface: Self::Surface) -> Option<(u32, u32)> {
        self.inner.surface_extent(surface)
    }
    fn gpu_fence(&mut self) -> Option<Box<dyn Fn() -> bool + Send>> {
        self.inner.gpu_fence()
    }

    fn depth_level_texture(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.depth_level_texture(surface)?;
        Ok(r.map(|inner| {
            let id = self.next_texture;
            self.next_texture += 1;
            self.origin(id, || format!("depth level surface {surface:?}"));
            Tracked { id, inner }
        }))
    }
    fn drop_pixel_count(&mut self, ticket: u64) {
        self.inner.drop_pixel_count(ticket)
    }
    fn uses_texture_placeholders(&self) -> bool {
        self.inner.uses_texture_placeholders()
    }
    fn texture_placeholder(
        &mut self,
        desc: &gfx::texture::TextureDesc,
    ) -> Result<Self::Texture, String> {
        let inner = self.inner.texture_placeholder(desc)?;
        let id = self.next_texture;
        self.next_texture += 1;
        Ok(Tracked { id, inner })
    }
    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        let r = self.inner.upload_texture(texture);
        if let Some(s) = &mut self.sweep {
            s.upload(texture);
        }
        if let Err(e) = &r {
            self.line(format!("upload_texture failed: {e}"));
        }
        let id = self.next_texture;
        self.next_texture += 1;
        self.origin(id, || {
            let d = &texture.desc;
            format!(
                "upload of {:?} {}x{} (guest bytes or a decoded readback)",
                d.format, d.width, d.height
            )
        });
        if let Some(c) = &self.capture {
            let levels: Vec<String> = texture
                .levels
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let name = format!("tex-{id}-{i}.bin");
                    if c.keep_tex.contains(&id) {
                        let _ = std::fs::write(c.dir.join(name), &l.data);
                    } else {
                        c.blob(&name, &l.data);
                    }
                    format!(
                        "face {} level {} {}x{}x{} {} bytes",
                        l.face,
                        l.level,
                        l.width,
                        l.height,
                        l.depth,
                        l.data.len()
                    )
                })
                .collect();
            c.append(
                "textures.txt",
                &format!(
                    "tex {id} frame {} host {:?} desc {:?} ok={} levels [{}]",
                    self.in_frame,
                    texture.host,
                    texture.desc,
                    r.is_ok(),
                    levels.join("; ")
                ),
            );
        }
        r.map(|inner| Tracked { id, inner })
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
        self.line(format!("clear {color:?} depth={depth:?} {clear:?}"));
        if let Some(s) = &mut self.sweep
            && s.dumping(self.in_frame)
        {
            s.line(&format!("clear {color:?} depth={depth:?} {clear:?}"));
        }
        if let Some(c) = self.capturing() {
            c.append(
                "frame.txt",
                &format!("clear color={color:?} depth={depth:?} {clear:?}"),
            );
        }
        if self.probe.armed() {
            self.probe_enter(color, depth, None);
            self.probe.clear(
                &format!("{color:?}"),
                depth.map(|d| format!("{d:?}")).as_deref(),
            );
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
        self.diagnosed_draw(color, depth, draw, None, textures)
    }
    fn draw_owned(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        // Capturing, logging and the sweep diagnostic read the draw after
        // the inner backend has drawn it.
        if self.inspects_draw() {
            let r = self.draw(color, depth, &draw, textures);
            return (r, draw.streams);
        }
        let (target, info) = (draw.target, draw.texture_info);
        let inner: [Option<&B::Texture>; 4] =
            std::array::from_fn(|i| textures[i].map(|t| &t.inner));
        let (r, spare) = self.inner.draw_owned(color, depth, draw, inner);
        self.tally(&r, &target, &info);
        (r, spare)
    }
    fn draw_shared(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: Arc<ResolvedDraw>,
        textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        if self.inspects_draw() {
            let result = self.diagnosed_draw(color, depth, &draw, Some(&draw), textures);
            return (result, Vec::new());
        }
        let (target, info) = (draw.target, draw.texture_info);
        let (result, spare) = self.inner.draw_shared(color, depth, draw, textures.map(|t| t.map(|t| &t.inner)));
        self.tally(&result, &target, &info);
        (result, spare)
    }
    fn frame_stats(&mut self) {
        self.inner.frame_stats()
    }
    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String> {
        self.present_with_gamma(surface, flags, None)
    }
    // The interpolated replay's host-only calls (d3d::interp): passed
    // through, not counted as guest presents.
    fn stamp_present(&mut self, at: std::time::Instant) {
        self.inner.stamp_present(at)
    }
    fn present_replay(
        &mut self,
        surface: Self::Surface,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.inner.present_replay(surface, gamma)
    }
    fn idle(&mut self) -> Option<std::time::Instant> {
        self.inner.idle()
    }
    fn drain(&mut self) -> Result<(), String> {
        self.inner.drain()
    }
    fn present_with_gamma(
        &mut self,
        surface: Self::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.line(format!("present {surface:?} flags={flags:#x}"));
        if let Some(s) = &mut self.sweep {
            if s.dumping(self.in_frame) {
                s.line(&format!("present {surface:?} flags={flags:#x}"));
            }
            s.presented(self.in_frame);
            if !s.active() {
                self.origins = BTreeMap::new();
            }
        }
        if let Some(c) = self.capturing() {
            c.append(
                "frame.txt",
                &format!("present {surface:?} flags={flags:#x}"),
            );
        }
        self.stats.lock().unwrap().presents += 1;
        let n = PRESENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if KEEP_PRESENT_HISTORY.load(std::sync::atomic::Ordering::Relaxed) {
            let mut times = PRESENT_TIMES.lock().unwrap();
            let now = std::time::Instant::now();
            // A heartbeat for long runs: where the run is, and how fast the last 100 went.
            if n.is_multiple_of(100) && times.len() >= 100 {
                let secs = now.duration_since(times[times.len() - 100]).as_secs_f64();
                eprintln!("present {n}: last 100 at {:.2} fps", 100.0 / secs);
            }
            times.push(now);
        }
        // H2_HOST_PERF: where the frame time went (host_perf.rs), on the
        // game thread before the backend presents.
        crate::host_perf::on_present(n);
        // render.frame: the frame's last pass is read back before the
        // present when it was asked for.
        if let Some(ask) = self.probe.present_routed(n, self.owner_probe.then_some(self.in_frame)) {
            self.probe_snap(ask);
        }
        // render.capture: the present after the tick asked for is its frame.
        agent::render::CAPTURE.present(n);
        if !self.probe.armed() {
            self.probe_target = None;
        }
        self.in_frame += 1;
        if let Some(l) = &mut self.log {
            let _ = l.flush();
        }
        let presented = self.inner.present_with_gamma(surface, flags, gamma);
        if presented.is_err() && self.owner_probe {
            agent::render::private_graph::discard_display(self.in_frame - 1);
        }
        presented
    }
    fn clear_rects(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        if let Some(c) = self.capturing() {
            c.append(
                "frame.txt",
                &format!("clear_rects color={color:?} depth={depth:?} {clear:#?}"),
            );
        }
        if self.probe.armed() {
            self.probe_enter(color, depth, None);
            self.probe.clear(
                &format!("{color:?}"),
                depth.map(|d| format!("{d:?}")).as_deref(),
            );
        }
        let r = self.inner.clear_rects(color, depth, clear);
        self.line(format!(
            "clear_rects {color:?} {} rects -> {}",
            clear.rects.len(),
            outcome_name(&r)
        ));
        r
    }
    fn read_surface(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        self.line(format!(
            "read_surface {surface:?} {format:?} {width}x{height}"
        ));
        self.probe.op(
            "read",
            &format!("{surface:?}"),
            format!("{format:?} {width}x{height}"),
        );
        if let Some(c) = self.capturing() {
            c.append(
                "frame.txt",
                &format!("read_surface {surface:?} {format:?} {width}x{height}"),
            );
        }
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
        self.line(format!(
            "read_depth {surface:?} {format:?} {width}x{height}"
        ));
        self.probe.op(
            "read_depth",
            &format!("{surface:?}"),
            format!("{format:?} {width}x{height}"),
        );
        if let Some(c) = self.capturing() {
            c.append(
                "frame.txt",
                &format!("read_depth {surface:?} {format:?} {width}x{height}"),
            );
        }
        self.inner
            .read_depth(surface, format, width, height, layout)
    }
    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        self.line(format!("copy_surface {src:?} -> {dst:?}"));
        self.probe
            .op("copy", &format!("{src:?}"), format!("to {dst:?}"));
        if let Some(s) = &mut self.sweep
            && s.dumping(self.in_frame)
        {
            s.line(&format!("copy_surface {src:?} -> {dst:?}"));
        }
        if let Some(c) = self.capturing() {
            c.append("frame.txt", &format!("copy_surface {src:?} -> {dst:?}"));
        }
        self.inner.copy_surface(src, dst)
    }
    fn write_surface(
        &mut self,
        surface: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.line(format!("write_surface {surface:?}"));
        if let Some(c) = &mut self.capture {
            let n = c.writes;
            c.writes += 1;
            c.blob(&format!("write-{n}.bin"), bytes);
            c.append(
                "writes.txt",
                &format!("write {n} frame {} {surface:?} {desc:?}", self.in_frame),
            );
        }
        self.inner.write_surface(surface, desc, bytes)
    }
    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<Self::Surface, String> {
        let r = self.inner.create_texture_target(format, width, height);
        self.line(format!(
            "create_texture_target {format:?} {width}x{height} -> {r:?}"
        ));
        if let Some(c) = &self.capture {
            c.append(
                "surfaces.txt",
                &format!(
                    "frame {} create_texture_target {format:?} {width}x{height} -> {r:?}",
                    self.in_frame
                ),
            );
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report names outcomes, targets and textures as the per-draw
    /// strings did: drawn draws by target, others by their full outcome.
    #[test]
    fn report_names_what_it_counted() {
        let t = |w, h, format| TargetInfo {
            width: w,
            height: h,
            format,
            depth_format: None,
        };
        let tex = |format, kind| TextureInfo {
            header: 0x1000,
            palette: 0,
            format,
            kind,
            width: 4,
            height: 4,
            depth: 1,
            levels: 1,
        };
        let a = t(640, 480, D3DFormat::LinA8R8G8B8);
        let b = t(320, 240, D3DFormat::A8R8G8B8);
        let info = [
            Some(tex(D3DFormat::Dxt1, TextureKind::D2)),
            None,
            Some(tex(D3DFormat::Dxt1, TextureKind::D2)),
            None,
        ];
        let mut s = DrawStats::default();
        s.add(&Ok(DrawOutcome::Drawn), &a, &info);
        s.add(&Ok(DrawOutcome::Drawn), &a, &[None; 4]);
        s.add(
            &Ok(DrawOutcome::Drawn),
            &b,
            &[
                Some(tex(D3DFormat::A8R8G8B8, TextureKind::Cube)),
                None,
                None,
                None,
            ],
        );
        s.add(&Ok(DrawOutcome::Skipped("no view".into())), &a, &info);
        s.add(&Err("lost".into()), &b, &info);
        s.presents = 2;
        assert_eq!(
            s.report(),
            "draw diag: 2 presents\n\
             \x20 draws        1  drawn -> 320x240 A8R8G8B8\n\
             \x20 draws        2  drawn -> 640x480 LinA8R8G8B8\n\
             \x20 draws        1  error: lost -> 320x240 A8R8G8B8\n\
             \x20 draws        1  skipped: no view -> 640x480 LinA8R8G8B8\n\
             \x20 texture        1  A8R8G8B8 Cube\n\
             \x20 texture        2  Dxt1 D2\n"
        );
    }

    /// Frames whose drawn draws sample a depth texture (a shadow map) are
    /// listed once each, numbered as `frame-N.png` is; a skipped draw or a
    /// colour texture does not count.
    #[test]
    fn report_lists_the_frames_that_sample_a_shadow_map() {
        let t = TargetInfo {
            width: 640,
            height: 480,
            format: D3DFormat::LinA8R8G8B8,
            depth_format: None,
        };
        let tex = |format| TextureInfo {
            header: 0x1000,
            palette: 0,
            format,
            kind: TextureKind::D2,
            width: 512,
            height: 512,
            depth: 1,
            levels: 1,
        };
        let map = [
            Some(tex(D3DFormat::LinD24S8)),
            Some(tex(D3DFormat::L8)),
            None,
            None,
        ];
        let colour = [Some(tex(D3DFormat::LinA8R8G8B8)), None, None, None];
        let mut s = DrawStats::default();
        let mut seen = Vec::new();
        for frame in 1..=9u64 {
            s.presents = frame - 1;
            seen.push(s.add(&Ok(DrawOutcome::Drawn), &t, &colour));
            if [3, 4, 5, 8].contains(&frame) {
                seen.push(s.add(&Ok(DrawOutcome::Drawn), &t, &map));
                seen.push(s.add(&Ok(DrawOutcome::Drawn), &t, &map));
            }
            if frame == 6 {
                seen.push(s.add(&Ok(DrawOutcome::Skipped("no view".into())), &t, &map));
            }
        }
        let seen: Vec<u64> = seen.into_iter().flatten().collect();
        assert_eq!(seen, [3, 4, 5, 8]);
        assert!(
            s.report().ends_with("  shadow-map frames   4  3-5, 8\n"),
            "{}",
            s.report()
        );
        // A delayed worker fallback keeps the frame it was queued in.
        assert_eq!(s.add_at(4, &Ok(DrawOutcome::Drawn), &t, &map), None);
        assert_eq!(s.add_at(2, &Ok(DrawOutcome::Drawn), &t, &map), Some(2));
        assert!(s.report().ends_with("  shadow-map frames   5  2-5, 8\n"));
        assert_eq!(ranges(&[1, 2, 4, 6, 7], 2), "1-2, 4, ...");
    }

    /// Held by every test that presents through a [`DiagBackend`]: the
    /// probe numbers presents by the process-wide count ([`presents`], the
    /// run's frame clock, one game per process), so another test presenting
    /// in between moves a frame's numbers. Without it
    /// `the_agent_probe_records_one_frame_in_passes` saw `present` 4 for
    /// `from_present` 1 when `the_probe_records_the_constants_asked_for`
    /// presented on another thread.
    static PRESENTING: Mutex<()> = Mutex::new(());

    /// `render.frame` through the wrapper: clears and copies of one frame,
    /// in passes, between two presents; the presents themselves still
    /// reach the backend.
    #[test]
    fn the_agent_probe_records_one_frame_in_passes() {
        let _one = PRESENTING.lock().unwrap_or_else(|e| e.into_inner());
        static PROBE: agent::render::FrameProbe = agent::render::FrameProbe::new();
        let (d, _) = DiagBackend::new(d3d::RecordingBackend::new());
        let mut d = d.with_probe(&PROBE);
        let clear = Clear {
            color: Some([0.0; 4]),
            depth: None,
            stencil: None,
        };
        d.clear(1, None, clear).unwrap();
        PROBE
            .arm(&agent_proto::FrameParams {
                arm: true,
                ..Default::default()
            })
            .unwrap();
        d.clear(1, None, clear).unwrap();
        d.present(1, 0).unwrap();
        d.clear(2, Some(9), clear).unwrap();
        d.copy_surface(2, 3).unwrap();
        d.clear(1, None, clear).unwrap();
        d.present(1, 0).unwrap();
        d.clear(1, None, clear).unwrap();
        let f = PROBE.reply().frame.expect("recorded");
        assert_eq!(f.present, f.from_present + 1);
        assert_eq!(
            (
                f.totals.clears,
                f.totals.copies,
                f.totals.passes,
                f.totals.targets
            ),
            (2, 1, 2, 2)
        );
        assert_eq!(f.passes[0].color, "2");
        assert_eq!(f.passes[0].depth.as_deref(), Some("9"));
        assert_eq!(f.ops[0].detail, "to 3");
        assert!(!PROBE.armed());
    }

    /// A W-buffered draw of a vertex program to a D24S8 target, depth
    /// clamped, LessEqual without a depth write.
    fn depth_draw() -> ResolvedDraw {
        use gfx::xdk::RenderStateTable;
        let table = RenderStateTable::default();
        let mut words = d3d::state::StateWords::defaults(true);
        let clip_control = table
            .canonical_to_guest(d3d::hw::RS_DEPTHCLIPCONTROL)
            .expect("DEPTHCLIPCONTROL has a guest slot");
        words.render[clip_control as usize] = 0x10;
        let mut state = words.translate(&table);
        state.render.depth_mode = gfx::state::DepthBufferMode::W;
        state.render.depth_write = false;
        let mut constants = [[0.0; 4]; d3d::shader::CONSTANTS];
        constants[96 - 38] = [0.0, 0.0, 16384.0, 1.0];
        constants[96 + 58] = [320.0, -240.0, 16777215.0, 0.0];
        let mut hw = d3d::hw::HwState {
            control0: Some(0x0010_0001 | d3d::hw::CONTROL0_Z_PERSPECTIVE),
            clip_min: 0.0,
            clip_max: 16777215.0,
            ..Default::default()
        };
        hw.viewport_scale[2] = 16777215.0;
        hw.viewport_offset[2] = 0.5;
        ResolvedDraw {
            entry: "DrawIndexedVertices",
            primitive: gfx::vertex::PrimitiveType::TriangleList,
            vertex_count: 3,
            layout: gfx::vertex::VertexLayout::default(),
            streams: Vec::new(),
            indices: None,
            state,
            extras: words.stage_extras(),
            raw: Arc::new(words),
            target: TargetInfo {
                width: 640,
                height: 480,
                format: D3DFormat::LinA8R8G8B8,
                depth_format: Some(D3DFormat::LinD24S8),
            },
            texture_info: [None; 4],
            shader: d3d::shader::ShaderInputs {
                vertex: d3d::shader::VertexProcessing::Program {
                    start: 7,
                    microcode: vec![0u32; 12].into(),
                },
                provided_inputs: 1,
                vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
                constants: Arc::new(constants),
                pixel_shader: None,
                combiner_state: Arc::new([0; 57]),
                final_combiner_latch: false,
                affine_projection: false,
            },
            viewport: d3d::backend::Viewport {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
                min_z: 0.0,
                max_z: 1.0,
            },
            clip: None,
            clip_rects: Vec::new(),
            hw,
            textures: [None; 4],
            glyph_cache: 0,
        }
    }

    /// `render.frame`'s draw carries the depth state the backend draws
    /// with, the vertex side, and the constants the probe can pick from.
    #[test]
    fn a_probed_draw_says_how_it_tests_and_stores_depth() {
        let draw = depth_draw();
        let info = probe_draw(1u32, Some(2u32), &draw, &Ok(DrawOutcome::Drawn));
        let depth = info.depth_state.expect("depth state");
        assert_eq!(
            depth,
            agent_proto::DepthRecord {
                mode: "w".into(),
                w: true,
                func: "LessEqual".into(),
                write: false,
                viewport: [0.0, 1.0],
                scale: [16777215.0, 0.5],
                clip: [0.0, 16777215.0],
                full: 16777215.0,
                clamp: true,
            }
        );
        let hash = d3d::shader::Microcode::hash_words(&[0; 12]) as u32;
        assert_eq!(info.vs, format!("prog @7 n=3 #{hash:08x}"));
        let block = info.constants.expect("constants");
        assert_eq!(block.len(), d3d::shader::CONSTANTS);
        assert_eq!(block[96 - 38], [0.0, 0.0, 16384.0, 1.0]);

        // SET_CONTROL0 decides W-buffering once pushed, whatever ZENABLE
        // says; no depth target, no full scale.
        let mut draw = depth_draw();
        draw.hw.control0 = Some(0x0010_0001);
        draw.target.depth_format = None;
        draw.shader.vertex = d3d::shader::VertexProcessing::FixedFunction { fvf: 0x152 };
        let info = probe_draw(1u32, None, &draw, &Ok(DrawOutcome::Drawn));
        let depth = info.depth_state.expect("depth state");
        assert_eq!(
            (depth.mode.as_str(), depth.w, depth.full),
            ("w", false, 0.0)
        );
        assert_eq!(info.vs, "fvf 0x152");
    }

    /// The constants asked for reach the frame by D3D register number.
    #[test]
    fn the_probe_records_the_constants_asked_for() {
        let _one = PRESENTING.lock().unwrap_or_else(|e| e.into_inner());
        static PROBE: agent::render::FrameProbe = agent::render::FrameProbe::new();
        let (d, _) = DiagBackend::new(d3d::RecordingBackend::new());
        let mut d = d.with_probe(&PROBE);
        PROBE
            .arm(&agent_proto::FrameParams {
                arm: true,
                draws: true,
                constants: vec![-38, 58],
                ..Default::default()
            })
            .unwrap();
        d.present(1, 0).unwrap();
        let _ = d.draw(1, Some(2), &depth_draw(), [None; 4]);
        d.present(1, 0).unwrap();
        let f = PROBE.reply().frame.expect("recorded");
        let draw = &f.draws[0];
        assert_eq!(draw.constants[&-38], [0.0, 0.0, 16384.0, 1.0]);
        assert_eq!(draw.constants[&58], [320.0, -240.0, 16777215.0, 0.0]);
        assert_eq!(draw.depth.as_ref().map(|d| d.w), Some(true));
    }

    #[test]
    fn the_headless_gpu_stack_pins_plain_lights_once() {
        struct PlainLight {
            index: u32,
            seen: Arc<Mutex<Vec<u32>>>,
        }
        impl d3d::ExternalPass for PlainLight {
            fn batch_key(&self) -> Option<u64> { Some(37) }

            fn prepare_copy(&mut self, target: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
                assert_eq!(target.copy, d3d::TargetCopy::Reference);
                self.seen.lock().unwrap().push(self.index);
                Ok(())
            }

            fn record_bundles(&mut self, _: &mut d3d::ExternalTarget<'_>) -> Option<d3d::backend::ExternalBundles> {
                Some(d3d::backend::ExternalBundles {
                    run: self.batch_key()?,
                    scissor: [0, 0, 8, 8],
                    groups: Vec::new(),
                    ranges: 1,
                })
            }
        }

        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let gpu = d3d::GfxBackend::new(ctx);
        let (mut backend, _) = DiagBackend::new(d3d::ThreadedBackend::new(gpu).unwrap());
        let surfaces = d3d::ExternalSurfaces {
            color: backend.create_surface(D3DFormat::LinA8R8G8B8, 8, 8, false).unwrap(),
            depth: backend.create_surface(D3DFormat::LinD16, 8, 8, true).unwrap(),
            sampled: vec![], slots: vec![], slot_sizes: vec![],
        };
        let request = d3d::ExternalPassRequest { rect: (0, 0, 8, 8), surface_size: (8, 8), z_scale: 65_535. };
        let plan = d3d::ExternalPlan { stage: timeline::native::Stage::Lights, view: 0, ranges: 1, slots: vec![] };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let reads = backend.inner.waits().reads;
        for index in 0..4 {
            let pass: d3d::SharedReplacement = Arc::new(Mutex::new(Box::new(PlainLight { index, seen: seen.clone() })));
            let recordings = backend.prepare_external(surfaces.clone(), request, [None; 4], &plan,
                d3d::TargetCopy::Reference, pass).unwrap().expect("complete reservation");
            for recording in recordings { backend.commit_external(recording); }
        }
        assert_eq!(backend.inner.waits().reads - reads, 1);
        assert_eq!(*seen.lock().unwrap(), [0, 1, 2, 3]);
        backend.drain().unwrap();
    }

    /// Every `Backend` method must be forwarded by `DiagBackend`; a missed
    /// provided method silently falls back to the trait's default.
    #[test]
    fn forwards_every_backend_method() {
        fn methods(src: &str, start: &str) -> std::collections::BTreeSet<String> {
            let body = &src[src.find(start).expect(start)..];
            let body = &body[..body.find("\n}\n").unwrap()];
            body.lines()
                .filter_map(|l| {
                    l.trim()
                        .strip_prefix("fn ")
                        .map(|r| r.split(['(', '<']).next().unwrap().to_string())
                })
                .collect()
        }
        let tr = methods(
            include_str!("../../../platform/d3d/src/backend.rs"),
            "pub trait Backend",
        );
        let ours = methods(
            include_str!("d3d_diag.rs"),
            "impl<B: Backend> Backend for DiagBackend<B>",
        );
        let missing: Vec<_> = tr.difference(&ours).collect();
        assert!(
            missing.is_empty(),
            "DiagBackend does not forward {missing:?}"
        );
        let headless = methods(
            include_str!("d3d_glue.rs"),
            "impl d3d::backend::Backend for Headless",
        );
        let missing: Vec<_> = tr.difference(&headless).collect();
        assert!(missing.is_empty(), "Headless does not forward {missing:?}");
    }
}
