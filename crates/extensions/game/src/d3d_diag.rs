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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::sync::{Arc, Mutex};

use d3d::backend::{
    Backend, DisplaySurfacePixels, DrawOutcome, GuestLayoutKind, RectClear, RenderExtents,
    ResolvedDraw, StreamBytes, TargetInfo, TextureInfo,
};
use gfx::texture::{DecodedTexture, TextureDesc, TextureKind};
use gfx::{Clear, D3DFormat};

mod layout;

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

/// Keep the total and a bounded window of timestamps for the frame-rate report.
static KEEP_PRESENT_HISTORY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static PRESENT_TIMES: Mutex<PresentHistory> = Mutex::new(PresentHistory {
    recorded: 0,
    times: VecDeque::new(),
});

const PRESENT_HISTORY_LIMIT: usize = 65_536;
const SHADOW_HISTORY_LIMIT: usize = 4_096;
const OUTCOME_LIMIT: usize = 1_024;

#[derive(Default)]
struct PresentHistory {
    recorded: usize,
    times: VecDeque<std::time::Instant>,
}

impl PresentHistory {
    fn push(&mut self, now: std::time::Instant) {
        if self.times.len() == PRESENT_HISTORY_LIMIT {
            self.times.pop_front();
        }
        self.times.push_back(now);
        self.recorded += 1;
    }

    fn hundred_frame_rate(&self, now: std::time::Instant) -> Option<f64> {
        let first = self.times.get(self.times.len().checked_sub(100)?)?;
        Some(100.0 / now.duration_since(*first).as_secs_f64())
    }
}

pub fn keep_present_history(keep: bool) {
    KEEP_PRESENT_HISTORY.store(keep, std::sync::atomic::Ordering::Relaxed);
    if !keep {
        *PRESENT_TIMES.lock().unwrap() = PresentHistory::default();
    }
}

/// Wall-clock frame rate: presents per second over the run since `start`,
/// and over a window with its mean, median and worst frame time: from
/// present `from` (1-based) when given, else the last `tail` intervals,
/// limited to the available history.
/// `None` before two presents.
pub fn frame_rate(start: std::time::Instant, tail: usize, from: Option<usize>) -> Option<String> {
    let history = PRESENT_TIMES.lock().unwrap();
    let n = history.recorded;
    let times = &history.times;
    let len = times.len();
    if len < 2 {
        return None;
    }
    let total = times[len - 1].duration_since(start).as_secs_f64();
    let first = n - len + 1;
    let from = match from {
        Some(f) => f.saturating_sub(first).min(len - 2),
        None => len.saturating_sub(tail.max(1).saturating_add(1)),
    };
    let mut gaps: Vec<f64> = (from + 1..len)
        .map(|i| times[i].duration_since(times[i - 1]).as_secs_f64() * 1e3)
        .collect();
    let span: f64 = gaps.iter().sum();
    gaps.sort_by(f64::total_cmp);
    Some(format!(
        "frames: {n} presents in {total:.1}s ({:.2} fps); presents {}-{n}: {:.2} fps, frame ms mean {:.0} median {:.0} max {:.0}",
        n as f64 / total,
        first + from,
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
    other_outcomes: u64,
    /// Texture (format, kind) bound to drawn draws -> draws.
    textures: rustc_hash::FxHashMap<(D3DFormat, TextureKind), u64>,
    /// Recent frames (numbered as `frame-N.png`) with a drawn draw that samples
    /// a depth texture: Halo's shadow apply (layer 11) reading a shadow
    /// map. Worker fallbacks can land after the guest counts a newer frame.
    shadow_frames: BTreeSet<u64>,
}

/// Recent shadow frames, for the screenshot hook
/// (`H2_BOOT_GFX_SHADOWS`), which may run on the render thread.
static SHADOW_FRAMES: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

fn remember_shadow(frames: &mut BTreeSet<u64>, frame: u64) -> bool {
    if frames.len() == SHADOW_HISTORY_LIMIT && frames.first().is_some_and(|&first| frame <= first) {
        return false;
    }
    if !frames.insert(frame) {
        return false;
    }
    if frames.len() > SHADOW_HISTORY_LIMIT {
        frames.pop_first();
    }
    true
}

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
        if self.other_outcomes != 0 {
            m.insert("other draw outcomes".into(), self.other_outcomes);
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
        let key = (out, (t.width, t.height, t.format));
        if self.by_outcome.len() < OUTCOME_LIMIT {
            *self.by_outcome.entry(key).or_default() += 1;
        } else if let Some(count) = self.by_outcome.get_mut(&key) {
            *count += 1;
        } else {
            self.other_outcomes += 1;
        }
        if !drawn {
            return None;
        }
        for ti in info.iter().flatten() {
            *self.textures.entry((ti.format, ti.kind)).or_default() += 1;
        }
        let shadow = info.iter().flatten().any(|t| t.format.is_depth());
        (shadow && remember_shadow(&mut self.shadow_frames, frame)).then_some(frame)
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
}

/// A host texture with the ID the capture names it by.
pub struct Tracked<T, S> {
    pub id: u64,
    pub inner: T,
    source: TextureSource<S>,
}

enum TextureSource<S> {
    Upload,
    Placeholder,
    Surface(&'static str, S),
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

    fn logging(&self) -> bool {
        self.log.is_some() && self.frames.contains(&self.in_frame)
    }

    fn line(&mut self, s: std::fmt::Arguments<'_>) {
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
        let probe = self.probe;
        let Some((surface, width, height)) = self.probe_target else {
            probe.snapped_for(ask, Err("the pass's size is unknown (no draw in it)".into()));
            return;
        };
        if ask.display {
            // A layout capture reads the displayed raster at its own size.
            probe.write_snapshot(ask, || {
                let ((width, height), mut pixels) = self.inner.read_display_surface(surface)?
                    .ok_or_else(|| "the backend has no display readback for this surface".to_owned())?;
                if pixels.len() as u64 != u64::from(width) * u64::from(height) * 4 {
                    return Err(format!("display readback has {} bytes for {width}x{height}", pixels.len()));
                }
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                    pixel[3] = 255;
                }
                Ok(agent::compare::Rgba { width, height, pixels })
            });
            return;
        }
        let Some(pitch) = width.checked_mul(4) else {
            probe.snapped_for(ask, Err("frame probe row pitch overflow".into()));
            return;
        };
        let Some(size) = pitch.checked_mul(height) else {
            probe.snapped_for(ask, Err("frame probe image size overflow".into()));
            return;
        };
        let rgba = move |mut pixels: Vec<u8>| {
            if pixels.len() != size as usize {
                return Err(format!("frame probe read {} bytes, expected {size}", pixels.len()));
            }
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
            Ok(agent::compare::Rgba { width, height, pixels })
        };
        if self.owner_probe {
            // Only the renderer that draws the pass should read it back.
            // The worker executes this at the pass boundary on fallback.
            self.inner.queue_frame_probe_read(surface, width, height, Box::new(move |result| {
                probe.write_snapshot(ask, || result.and_then(rgba));
            }));
        } else {
            probe.write_snapshot(ask, || {
                self.inner.read_surface(surface, D3DFormat::LinA8R8G8B8, width, height,
                    GuestLayoutKind::Linear { pitch }).and_then(rgba)
            });
        }
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
        vs: vertex_name(&draw.shader.vertex, draw.retail_constants.as_ref().map(|(_, f)| *f)),
        depth_state: Some(depth_record(draw)),
        constants: Some(draw.shader.constants.clone() as Arc<[[f32; 4]]>),
        layout: None,
    }
}

/// The vertex side of a draw for `render.frame`: `fvf 0x…`, or `prog
/// @start n=slots #hash clip=<source>` (the low 32 bits of the words'
/// FNV-1a hash; `clip` is where the clip position comes from, see
/// `Microcode::clip_text`), then ` lens=x<factor>` when the display lens
/// widened the draw (`ResolvedDraw::retail_constants`). A program draw on
/// a wide display with `clip=unknown` or `clip=fixed` and no `lens` is
/// drawn at the retail 4:3 width.
fn vertex_name(v: &d3d::shader::VertexProcessing, lens: Option<f32>) -> String {
    match v {
        d3d::shader::VertexProcessing::FixedFunction { fvf } => format!("fvf {fvf:#x}"),
        d3d::shader::VertexProcessing::Program { start, microcode } => {
            let mut name = format!(
                "prog @{start} n={} #{:08x}",
                microcode.len() / 4,
                microcode.words_hash() as u32
            );
            let clip = microcode.clip_text();
            if !clip.is_empty() {
                name.push_str(&format!(" clip={clip}"));
            }
            if let Some(factor) = lens {
                name.push_str(&format!(" lens=x{factor:.3}"));
            }
            name
        }
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
    fn observe_upload(
        &mut self, texture: &DecodedTexture, r: Result<B::Texture, String>,
    ) -> Result<Tracked<B::Texture, B::Surface>, String> {
        if let Err(e) = &r {
            self.line(format_args!("upload_texture failed: {e}"));
        }
        let id = self.next_texture;
        self.next_texture += 1;
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
        r.map(|inner| Tracked { id, inner, source: TextureSource::Upload })
    }

    fn inspects_draw(&mut self) -> bool {
        self.capturing().is_some() || self.logging() || self.probe.armed()
    }

    fn diagnosed_draw(
        &mut self,
        color: B::Surface,
        depth: Option<B::Surface>,
        draw: &ResolvedDraw,
        issue: impl FnOnce(&mut B, [Option<&B::Texture>; 4]) -> Result<DrawOutcome, String>,
        textures: [Option<&Tracked<B::Texture, B::Surface>>; 4],
    ) -> Result<DrawOutcome, String> {
        let mut snap = None;
        if let Some(c) = self.capturing() {
            let ids = textures.map(|t| t.map(|t| t.id));
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
        let probing = self.probe.armed();
        if probing {
            self.probe_enter(color, depth, Some((draw.target.width, draw.target.height)));
        }
        let inner: [Option<&B::Texture>; 4] =
            std::array::from_fn(|i| textures[i].map(|t| &t.inner));
        let r = issue(&mut self.inner, inner);
        if probing {
            let mut info = probe_draw(color, depth, draw, &r);
            if self.probe.layout_requested() {
                let extents = self.inner.render_extents(color, depth, inner);
                info.layout = Some(layout::describe(draw, textures, extents, None));
            }
            self.probe.draw(info);
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
        self.tally(&r, &draw.target, &draw.texture_info);
        if self.logging() {
            let (out, target) = (outcome_name(&r), target_name(&draw.target));
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
            self.line(format_args!(
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
            self.line(format_args!("  layout {:?}", draw.layout));
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
                    self.line(format_args!("  s{i} {}", words.join(" ")));
                }
            }
        }
        r
    }

    /// Adds a draw's outcome to the stats.
    fn tally(
        &self,
        r: &Result<DrawOutcome, String>,
        t: &TargetInfo,
        info: &[Option<TextureInfo>; 4],
    ) {
        let frame = self.stats.lock().unwrap().add(r, t, info);
        if let Some(n) = frame {
            remember_shadow(&mut SHADOW_FRAMES.lock().unwrap(), n);
        }
    }
}

impl<B: Backend> Backend for DiagBackend<B> {
    type Surface = B::Surface;
    type Texture = Tracked<B::Texture, B::Surface>;

    #[cfg(feature = "gpu")]
    fn upload_scene_assets(&mut self, upload: d3d::backend::AssetUpload) {
        self.inner.upload_scene_assets(upload);
    }

    #[cfg(feature = "gpu")]
    fn frozen_images(&self, texture: &Self::Texture) -> Option<d3d::scene_sink::FrozenImages> {
        self.inner.frozen_images(&texture.inner)
    }

    #[cfg(feature = "gpu")]
    fn retain_scene_sample(&self, texture: &Self::Texture) -> Option<d3d::scene_sink::FrozenSample> {
        self.inner.retain_scene_sample(&texture.inner)
    }

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String> {
        let r = self.inner.create_surface(format, width, height, depth);
        self.line(format_args!(
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
    fn create_target(&mut self, target: d3d::TargetDesc) -> Result<Self::Surface, String> {
        let r = self.inner.create_target(target);
        self.line(format_args!("create_target {target:?} -> {r:?}"));
        r
    }
    fn release_target(&mut self, role: frame_scene::TargetRole, surface: Self::Surface) -> Result<(), String> {
        self.line(format_args!("release_target {role:?} {surface:?}"));
        self.inner.release_target(role, surface)
    }
    // Every provided method is forwarded: a default here would silently
    // replace the inner backend's (a frame-buffer snapshot would skip draws).
    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.snapshot_surface(surface, opaque)?;
        self.line(format_args!(
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
            Tracked { id, inner, source: TextureSource::Surface("snapshot of", surface) }
        }))
    }
    fn snapshot_surface_into(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
        into: &mut Self::Texture,
    ) -> Result<bool, String> {
        let r = self.inner.snapshot_surface_into(surface, opaque, &mut into.inner)?;
        self.line(format_args!(
            "snapshot_surface_into {surface:?} opaque={opaque} tex {} -> {}",
            into.id,
            if r { "ok" } else { "unsupported" }
        ));
        if r {
            into.source = TextureSource::Surface("snapshot of", surface);
            if let Some(c) = self.capturing() {
                c.append(
                    "frame.txt",
                    &format!("snapshot_surface_into {surface:?} opaque={opaque} -> tex {}", into.id),
                );
            }
        }
        Ok(r)
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
            Tracked { id, inner, source: TextureSource::Surface("depth bits of", depth) }
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
            Tracked { id, inner, source: TextureSource::Surface("render-to-texture", surface) }
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
        let draw = if self.probe.layout_requested() {
            pass.lock().unwrap_or_else(|e| e.into_inner()).diagnostic_draw()
        } else {
            None
        };
        let (color, depth) = (surfaces.color, surfaces.depth);
        let inner = textures.map(|t| t.map(|t| &t.inner));
        let result = self.inner.prepare_external(surfaces, request, inner, plan, copy, pass);
        if matches!(&result, Ok(Some(recordings)) if !recordings.is_empty())
            && let Some(draw) = draw
        {
            let depth = draw.target.depth_format.map(|_| depth);
            self.probe_enter(color, depth, Some((draw.target.width, draw.target.height)));
            let extents = self.inner.render_extents(color, depth, inner);
            let mut info = probe_draw(color, depth, &draw, &Ok(DrawOutcome::Drawn));
            info.layout = Some(layout::describe(
                &draw, textures, extents, Some(format!("{:?}", plan.stage)),
            ));
            self.probe.draw(info);
        }
        result
    }

    fn can_commit_carried(&self) -> bool {
        self.inner.can_commit_carried()
    }

    fn prepare_carried_stage(
        &mut self, stage: &d3d::SceneStage<Self::Surface>,
    ) -> Result<Option<Vec<d3d::ExternalRecording>>, String> {
        self.inner.prepare_carried_stage(stage)
    }

    fn commit_carried_stage(
        &mut self, stage: d3d::SceneStage<Self::Surface>, copies: Vec<d3d::ExternalRecording>,
    ) {
        self.inner.commit_carried_stage(stage, copies);
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

    fn take_scene_refusal(&mut self) -> bool {
        self.inner.take_scene_refusal()
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
        if self.capturing().is_some() || self.logging() || self.probe.armed() {
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
                    remember_shadow(&mut SHADOW_FRAMES.lock().unwrap(), frame);
                }
            }
            finished(fallback);
        });
        self.inner.queue_external_draw(candidate, textures.map(|t| t.map(|t| &t.inner)))
    }

    fn commit_external(&mut self, recording: d3d::ExternalRecording) {
        self.inner.commit_external(recording);
    }

    #[cfg(feature = "gpu")]
    fn upload_device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        self.inner.upload_device()
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
    fn queue_frame_probe_read(
        &mut self, surface: Self::Surface, width: u32, height: u32,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        self.inner.queue_frame_probe_read(surface, width, height, done)
    }

    fn measure_present_raster(&mut self, surface: Option<Self::Surface>) -> d3d::backend::RasterMeasurement {
        self.inner.measure_present_raster(surface)
    }
    fn last_present_unshown(&self) -> bool {
        self.inner.last_present_unshown()
    }

    fn take_presented_extent(&mut self) -> Option<(u32, u32)> {
        self.inner.take_presented_extent()
    }
    fn surface_extent(&self, surface: Self::Surface) -> Option<(u32, u32)> {
        self.inner.surface_extent(surface)
    }

    fn render_extents(
        &mut self, color: Self::Surface, depth: Option<Self::Surface>,
        textures: [Option<&Self::Texture>; 4],
    ) -> RenderExtents {
        self.inner.render_extents(color, depth, textures.map(|t| t.map(|t| &t.inner)))
    }

    fn read_display_surface(
        &mut self, surface: Self::Surface,
    ) -> Result<Option<DisplaySurfacePixels>, String> {
        self.inner.read_display_surface(surface)
    }

    fn depth_level_texture(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.depth_level_texture(surface)?;
        Ok(r.map(|inner| {
            let id = self.next_texture;
            self.next_texture += 1;
            Tracked { id, inner, source: TextureSource::Surface("depth level", surface) }
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
        Ok(Tracked { id, inner, source: TextureSource::Placeholder })
    }
    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        let result = self.inner.upload_texture(texture);
        self.observe_upload(texture, result)
    }
    fn upload_shared(&mut self, texture: Arc<DecodedTexture>) -> Result<Self::Texture, String> {
        let result = self.inner.upload_shared(Arc::clone(&texture));
        self.observe_upload(&texture, result)
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
        self.line(format_args!("clear {color:?} depth={depth:?} {clear:?}"));
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
        self.diagnosed_draw(color, depth, draw, |backend, inputs| {
            backend.draw(color, depth, draw, inputs)
        }, textures)
    }
    fn draw_owned(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        // Capturing and logging read the draw after
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
    fn draw_admitted(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
        admission: d3d::backend::RetainedAdmission<Self::Surface>,
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        if self.inspects_draw() {
            let result = self.diagnosed_draw(color, depth, &draw, |backend, inputs| {
                backend.draw_admitted(color, depth, draw.clone(), inputs, admission).0
            }, textures);
            return (result, draw.streams);
        }
        let (target, info) = (draw.target, draw.texture_info);
        let inputs = textures.map(|t| t.map(|t| &t.inner));
        let (result, spare) = self.inner.draw_admitted(color, depth, draw, inputs, admission);
        self.tally(&result, &target, &info);
        (result, spare)
    }
    fn begin_native_carry(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        admission: &d3d::backend::RetainedAdmission<Self::Surface>,
    ) -> bool {
        self.inner.begin_native_carry(color, depth, admission)
    }
    fn record_native_carry(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        textures: [Option<&Self::Texture>; 4],
    ) {
        self.inner.record_native_carry(color, depth, textures.map(|t| t.map(|t| &t.inner)))
    }
    fn end_native_carry(&mut self) {
        self.inner.end_native_carry()
    }
    fn draw_shared(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: Arc<ResolvedDraw>,
        textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        if self.inspects_draw() {
            let result = self.diagnosed_draw(color, depth, &draw, |backend, inputs| {
                backend.draw_shared(color, depth, Arc::clone(&draw), inputs).0
            }, textures);
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
    fn present_replay(
        &mut self,
        surface: Self::Surface,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.inner.present_replay(surface, gamma)
    }
    fn reset_present_metadata(&mut self) -> Result<(), String> {
        self.inner.reset_present_metadata()
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
        self.line(format_args!("present {surface:?} flags={flags:#x}"));
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
            if n.is_multiple_of(100) && let Some(fps) = times.hundred_frame_rate(now) {
                eprintln!("present {n}: last 100 at {fps:.2} fps");
            }
            times.push(now);
        }
        #[cfg(feature = "gpu")]
        if n.is_multiple_of(100) {
            crate::native_particles_admission::report_refusals();
            crate::native_gels_glue::report_refusals();
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
        #[cfg(feature = "gpu")]
        agent::render::clip::CLIPS.present(n);
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
        self.line(format_args!(
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
        self.line(format_args!(
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
        self.line(format_args!(
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
        self.line(format_args!("copy_surface {src:?} -> {dst:?}"));
        self.probe
            .op("copy", &format!("{src:?}"), format!("to {dst:?}"));
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
        self.line(format_args!("write_surface {surface:?}"));
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
        self.line(format_args!(
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

    #[test]
    fn draw_diagnostics_bound_long_run_history_and_outcome_detail() {
        let start = std::time::Instant::now();
        let mut presents = PresentHistory::default();
        let mut shadows = BTreeSet::new();
        for n in 1..=PRESENT_HISTORY_LIMIT + 50 {
            presents.push(start + std::time::Duration::from_millis(n as u64));
            assert!(remember_shadow(&mut shadows, n as u64));
        }
        assert_eq!(presents.recorded, PRESENT_HISTORY_LIMIT + 50);
        assert_eq!(presents.times.len(), PRESENT_HISTORY_LIMIT);
        assert_eq!(presents.times.front().unwrap().duration_since(start).as_millis(), 51);
        let next = *presents.times.back().unwrap() + std::time::Duration::from_millis(1);
        assert_eq!(presents.hundred_frame_rate(next), Some(1000.0));
        assert_eq!(shadows.len(), SHADOW_HISTORY_LIMIT);
        assert!(!remember_shadow(&mut shadows, 1));
        let newest = *shadows.last().unwrap();
        assert!(shadows.contains(&newest));

        let mut stats = DrawStats::default();
        let target = TargetInfo { width: 32, height: 32, format: D3DFormat::L8, depth_format: None };
        for n in 0..OUTCOME_LIMIT + 50 {
            stats.add(&Err(format!("error {n}")), &target, &[None; 4]);
        }
        stats.add(&Err("error 0".into()), &target, &[None; 4]);
        assert_eq!(stats.by_outcome.len(), OUTCOME_LIMIT);
        assert_eq!(stats.by_outcome()["other draw outcomes"], 50);
        assert_eq!(stats.by_outcome()["error: error 0 -> 32x32 L8"], 2);
    }

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

    /// The outer sink records the same draw packets the worker sees. A pass
    /// snapshot between them, or at Swap, must leave that pairing intact.
    #[cfg(feature = "gpu")]
    #[test]
    fn pass_snapshots_follow_the_display_worker_without_changing_its_choice() {
        use d3d::scene_sink::ReplayFrame;
        use d3d::threaded::owner::{Candidate, FrameOwner, Outputs, Submission};
        use d3d::{RecordingBackend, SceneSink, ThreadedBackend};
        use frame_scene::{Slot, SlotAt, TargetRef};
        use std::sync::atomic::{AtomicU64, Ordering};

        let _one = PRESENTING.lock().unwrap_or_else(|e| e.into_inner());
        let _serial = crate::frame_parity::RPC_RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let probe = &agent::render::PROBE;
        for native in [true, false] {
            let slot = Arc::new(Mutex::new(None::<Arc<ReplayFrame>>));
            let sealed = Arc::clone(&slot);
            let adopted = Arc::new(AtomicU64::new(0));
            let owner_swap = Arc::clone(&adopted);
            let mut recording = RecordingBackend::new();
            recording.readback_fill = 0x6d;
            struct ProbeEvents;
            impl d3d::threaded::owner::OwnerEvents for ProbeEvents {
                fn declined(&self, _: u64, n: u64) { agent::render::private_graph::display_declined(n); }
                fn failed(&self, _: u64, _: u64) { panic!("synthetic Swap failed"); }
                fn stopped(&self) { agent::render::private_graph::display_stopped(); }
            }
            let worker = ThreadedBackend::new(recording).unwrap().with_frame_owner(FrameOwner {
                select: Box::new(move |n, _| {
                    let frame = slot.lock().unwrap().take()?;
                    let selected = Arc::clone(&owner_swap);
                    Some(Submission::Native(Candidate {
                        frame,
                        before_original: None,
                        eligible: Box::new(move |_, _| native),
                        draw: Box::new(move |gpu: &mut RecordingBackend, targets| {
                            gpu.present(targets[&TargetRef::SceneColour], 0)?;
                            selected.store(n, Ordering::Relaxed);
                            Ok(Outputs::default())
                        }),
                    }))
                }),
                events: Arc::new(ProbeEvents),
            });
            let (diag, _) = DiagBackend::new(worker);
            let mut sink = SceneSink::new(diag.with_owner_probe(true), || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
                .with_census(false)
                .with_replay(move |frame| *sealed.lock().unwrap() = Some(Arc::new(frame)));
            let back = sink.create_surface(D3DFormat::LinA8R8G8B8, 2, 1, false).unwrap();
            let side = sink.create_surface(D3DFormat::LinA8R8G8B8, 1, 1, false).unwrap();

            probe.arm(&agent_proto::FrameParams { arm: true, snap: vec![0, 1], ..Default::default() }).unwrap();
            sink.present(back, 0).unwrap();
            let mut draw = depth_draw();
            draw.target.width = 2;
            draw.target.height = 1;
            draw.target.depth_format = None;
            sink.draw(back, None, &draw, [None; 4]).unwrap();
            draw.target.width = 1;
            sink.draw(side, None, &draw, [None; 4]).unwrap();
            sink.present(back, 0).unwrap();

            let trace = sink.inner_mut().inner.with_inner(|gpu| {
                gpu.calls.iter().filter_map(|call| match call {
                    d3d::Recorded::Draw { .. } => Some("draw"),
                    d3d::Recorded::ReadSurface { .. } => Some("read"),
                    d3d::Recorded::Present { .. } => Some("swap"),
                    _ => None,
                }).collect::<Vec<_>>()
            }).unwrap();
            if native {
                assert_eq!(adopted.load(Ordering::Relaxed), 2);
                assert_eq!(trace, ["swap", "swap"]);
            } else {
                assert_eq!(adopted.load(Ordering::Relaxed), 0);
                assert_eq!(trace, ["swap", "draw", "read", "draw", "read", "swap"]);
                let report = probe.reply().frame.unwrap();
                assert_eq!(report.snaps.len(), 2);
                for (snap, width) in report.snaps.into_iter().zip([2, 1]) {
                    let file = std::path::PathBuf::from(snap.file.expect("original pass PNG"));
                    let image = agent::compare::read_png(&file).unwrap();
                    std::fs::remove_file(file).unwrap();
                    assert_eq!(image, agent::compare::Rgba::filled(width, 1, [0x6d, 0x6d, 0x6d, 255]));
                }
            }
        }
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
            retail_constants: None,
            ui_fit: None,
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
        assert!(info.vs.starts_with(&format!("prog @7 n=3 #{hash:08x}")), "{}", info.vs);
        assert!(!info.vs.contains("lens="), "no display lens on this draw: {}", info.vs);
        let mut wide = depth_draw();
        wide.retail_constants = Some((wide.shader.constants.clone(), 0.75));
        let info = probe_draw(1u32, Some(2u32), &wide, &Ok(DrawOutcome::Drawn));
        assert!(info.vs.ends_with(" lens=x0.750"), "a widened draw says so: {}", info.vs);
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
    fn frame_layout_keeps_guest_clip_and_reports_the_display_clip() {
        let _one = PRESENTING.lock().unwrap_or_else(|e| e.into_inner());
        static PROBE: agent::render::FrameProbe = agent::render::FrameProbe::new();
        let mut inner = d3d::RecordingBackend::new();
        inner.extents.insert(1, (1920, 1080));
        let (backend, _) = DiagBackend::new(inner);
        let mut backend = backend.with_probe(&PROBE);

        let mut draw = depth_draw();
        draw.clip = Some(d3d::backend::PixelRect { x: 0, y: 0, width: 640, height: 480 });
        draw.ui_fit = d3d::backend::UiFit::about((1920, 1080), [320.0, 240.0]);

        PROBE.arm_layout(&agent_proto::FrameParams { arm: true, ..Default::default() }).unwrap();
        backend.present(1, 0).unwrap();
        backend.draw(1, None, &draw, [None; 4]).unwrap();
        backend.present(1, 0).unwrap();

        let frame = PROBE.reply().frame.unwrap();
        let layout = frame.draws[0].layout.as_ref().unwrap();
        assert_eq!(layout.guest_size, [640, 480]);
        assert_eq!(layout.display_size, Some([1920, 1080]));
        assert_eq!(layout.clip, [[0, 0, 640, 480]]);
        // A clip of the whole canvas spans the display (docs/graphics/high-resolution.md, "Scissor, clears and viewport").
        assert_eq!(layout.display_clip, [[0, 0, 1920, 1080]]);
        assert_eq!(layout.ui_fit, Some([0.75, 1.0, 80.0, 0.0]));
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
        let request = d3d::ExternalPassRequest { rect: (0, 0, 8, 8), surface_size: (8, 8), display_projection_factor: 1.0, z_scale: 65_535. };
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

    /// Exercise the same two decorators and display-worker window as a game
    /// installation. Asset uploads and immutable textures must reach the GPU
    /// even when this present uses the original renderer.
    #[cfg(feature = "gpu")]
    #[test]
    fn scene_assets_samples_and_original_presents_cross_the_diagnostic_stack() {
        let _one = PRESENTING.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let window = Arc::new(d3d::scene_sink::window::PresentWindow::new(false));
        let worker = d3d::ThreadedBackend::new(d3d::GfxBackend::new(ctx)).unwrap()
            .with_present_window(Some(Arc::clone(&window)));
        let (diag, _) = DiagBackend::new(worker);
        let mut backend = d3d::SceneSink::new(diag, || None)
            .with_census(false)
            .with_present_window(Some(Arc::clone(&window)), || true);
        let (mark, uploaded) = std::sync::mpsc::channel();
        backend.upload_scene_assets(Box::new(move |_, _| { mark.send(()).unwrap(); }));

        let colour = backend.create_surface(D3DFormat::LinA8R8G8B8, 4, 4, false).unwrap();
        backend.clear(colour, None, Clear { color: Some([0.5; 4]), ..Clear::NONE }).unwrap();
        let texture = backend.snapshot_surface(colour, false).unwrap().unwrap();
        window.activate();
        window.game_view(Some(true));
        backend.present(colour, 0).unwrap();
        // A threaded backend retains the prior Swap's exact proxy. Falling
        // through to the synchronous GPU-image default would return None.
        let held = backend.retain_scene_sample(&texture).expect("immutable scene texture");
        drop(texture);
        backend.drain().unwrap();
        assert!(uploaded.try_recv().is_ok());
        let counts = window.counters();
        assert_eq!(counts["scene_window_games"], 1);
        assert_eq!(counts["scene_window_ordinary"], 1);
        assert_eq!(counts["scene_window_owned"], 0);
        assert_eq!(counts["scene_window_gaps"], 0);
        drop(held);
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
