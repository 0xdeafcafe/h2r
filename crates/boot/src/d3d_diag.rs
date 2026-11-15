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

use d3d::backend::{Backend, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw};
use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

/// Presents so far, for timelines keyed on frames (`H2_BOOT_INPUT`).
static PRESENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Presents seen by any [`DiagBackend`] so far.
pub fn presents() -> u64 {
    PRESENTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Wall-clock time of every present, for the frame-rate report.
static PRESENT_TIMES: Mutex<Vec<std::time::Instant>> = Mutex::new(Vec::new());

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
    let mut gaps: Vec<f64> = times[from..].windows(2).map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1e3).collect();
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
#[derive(Default)]
pub struct DrawStats {
    pub presents: u64,
    /// (outcome, target WxH format) -> draws.
    pub by_outcome: BTreeMap<String, u64>,
    /// Texture formats bound to drawn draws -> draws.
    pub textures: BTreeMap<String, u64>,
}

impl DrawStats {
    pub fn report(&self) -> String {
        let mut s = format!("draw diag: {} presents\n", self.presents);
        for (k, v) in &self.by_outcome {
            s += &format!("  draws {v:>8}  {k}\n");
        }
        for (k, v) in &self.textures {
            s += &format!("  texture {v:>8}  {k}\n");
        }
        s
    }
}

pub struct DiagBackend<B> {
    inner: B,
    stats: Arc<Mutex<DrawStats>>,
    log: Option<std::io::BufWriter<std::fs::File>>,
    frames: BTreeSet<u64>,
    in_frame: u64,
    capture: Option<Capture>,
    next_texture: u64,
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
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
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

impl<B> DiagBackend<B> {
    pub fn new(inner: B) -> (Self, Arc<Mutex<DrawStats>>) {
        let stats = Arc::new(Mutex::new(DrawStats::default()));
        let log = std::env::var_os("H2_BOOT_DRAWLOG").and_then(|p| match std::fs::File::create(&p) {
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
            std::fs::create_dir_all(&dir).map_err(|e| eprintln!("boot: H2_BOOT_CAPTURE {}: {e}", dir.display())).ok()?;
            let frame = std::env::var("H2_BOOT_CAPTURE_FRAME").ok().and_then(|v| v.parse().ok()).unwrap_or(480);
            Some(Capture { dir, frame, draws: 0, writes: 0, snaps: draw_set("H2_BOOT_CAPTURE_SNAP"), skips: draw_set("H2_BOOT_CAPTURE_SKIP"), lite: std::env::var_os("H2_BOOT_CAPTURE_LITE").is_some(), keep_tex: draw_set("H2_BOOT_CAPTURE_TEX") })
        });
        (DiagBackend { inner, stats: Arc::clone(&stats), log, frames, in_frame: 1, capture, next_texture: 0 }, stats)
    }

    /// The capture, while frame `H2_BOOT_CAPTURE_FRAME` is being drawn.
    fn capturing(&mut self) -> Option<&mut Capture> {
        let f = self.in_frame;
        self.capture.as_mut().filter(|c| c.frame == f)
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
}

fn outcome_name(r: &Result<DrawOutcome, String>) -> String {
    match r {
        Ok(DrawOutcome::Drawn) => "drawn".into(),
        Ok(DrawOutcome::Skipped(why)) => format!("skipped: {why}"),
        Ok(DrawOutcome::Unsupported(why)) => format!("unsupported: {why}"),
        Err(e) => format!("error: {e}"),
    }
}

impl<B: Backend> Backend for DiagBackend<B> {
    type Surface = B::Surface;
    type Texture = Tracked<B::Texture>;

    fn create_surface(&mut self, format: D3DFormat, width: u32, height: u32, depth: bool) -> Result<Self::Surface, String> {
        let r = self.inner.create_surface(format, width, height, depth);
        self.line(format!("create_surface {format:?} {width}x{height} depth={depth} -> {r:?}"));
        if let Some(c) = &self.capture {
            c.append("surfaces.txt", &format!("frame {} create_surface {format:?} {width}x{height} depth={depth} -> {r:?}", self.in_frame));
        }
        r
    }
    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        self.inner.destroy_surface(surface)
    }
    // Every provided method is forwarded: a default here would silently
    // replace the inner backend's (a frame-buffer snapshot would skip draws).
    fn snapshot_surface(&mut self, surface: Self::Surface, opaque: bool) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.snapshot_surface(surface, opaque)?;
        self.line(format!("snapshot_surface {surface:?} opaque={opaque} -> {}", if r.is_some() { "ok" } else { "unsupported" }));
        Ok(r.map(|inner| {
            let id = self.next_texture;
            self.next_texture += 1;
            if let Some(c) = self.capturing() {
                c.append("frame.txt", &format!("snapshot_surface {surface:?} opaque={opaque} -> tex {id}"));
            }
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
    fn drop_pixel_count(&mut self, ticket: u64) {
        self.inner.drop_pixel_count(ticket)
    }
    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        let r = self.inner.upload_texture(texture);
        if let Err(e) = &r {
            self.line(format!("upload_texture failed: {e}"));
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
                    format!("face {} level {} {}x{}x{} {} bytes", l.face, l.level, l.width, l.height, l.depth, l.data.len())
                })
                .collect();
            c.append(
                "textures.txt",
                &format!("tex {id} frame {} host {:?} desc {:?} ok={} levels [{}]", self.in_frame, texture.host, texture.desc, r.is_ok(), levels.join("; ")),
            );
        }
        r.map(|inner| Tracked { id, inner })
    }
    fn bc_supported(&self) -> bool {
        self.inner.bc_supported()
    }
    fn clear(&mut self, color: Self::Surface, depth: Option<Self::Surface>, clear: Clear) -> Result<(), String> {
        self.line(format!("clear {color:?} depth={depth:?} {clear:?}"));
        if let Some(c) = self.capturing() {
            c.append("frame.txt", &format!("clear color={color:?} depth={depth:?} {clear:?}"));
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
        let ids: Vec<Option<u64>> = textures.iter().map(|t| t.map(|t| t.id)).collect();
        let mut snap = None;
        if let Some(c) = self.capturing() {
            let k = c.draws;
            if c.skips.contains(&k) {
                c.draws += 1;
                c.append("frame.txt", &format!("draw {k} skipped (H2_BOOT_CAPTURE_SKIP)"));
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
            c.append("frame.txt", &format!("draw {k} color={color:?} depth={depth:?} textures={ids:?}\n{d:#?}"));
        }
        let inner: [Option<&B::Texture>; 4] = std::array::from_fn(|i| textures[i].map(|t| &t.inner));
        let r = self.inner.draw(color, depth, draw, inner);
        if let Some((k, path)) = snap {
            let t = &draw.target;
            let layout = GuestLayoutKind::Linear { pitch: t.width * 4 };
            match self.inner.read_surface(color, D3DFormat::LinA8R8G8B8, t.width, t.height, layout) {
                Ok(bytes) => {
                    let _ = std::fs::write(&path, bytes);
                }
                Err(e) => eprintln!("boot: snap after draw {k}: {e}"),
            }
        }
        let out = outcome_name(&r);
        let t = &draw.target;
        let target = format!("{}x{} {:?}", t.width, t.height, t.format);
        {
            let mut s = self.stats.lock().unwrap();
            let kind = out.split(':').next().unwrap_or("").to_string();
            let key = if kind == "drawn" { format!("drawn -> {target}") } else { format!("{out} -> {target}") };
            *s.by_outcome.entry(key).or_default() += 1;
            if kind == "drawn" {
                for ti in draw.texture_info.iter().flatten() {
                    *s.textures.entry(format!("{:?} {:?}", ti.format, ti.kind)).or_default() += 1;
                }
            }
        }
        if self.logging() {
            let tex: Vec<String> = draw
                .texture_info
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.map(|t| format!("t{i}={:?} {}x{} @{:#x}", t.format, t.width, t.height, t.header)))
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
                            if f.is_finite() && (f == 0.0 || (1e-4..1e5).contains(&f.abs())) { format!("{f}") } else { format!("#{:08x}", u32::from_le_bytes(*w)) }
                        })
                        .collect();
                    self.line(format!("  s{i} {}", words.join(" ")));
                }
            }
        }
        r
    }
    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String> {
        self.line(format!("present {surface:?} flags={flags:#x}"));
        if let Some(c) = self.capturing() {
            c.append("frame.txt", &format!("present {surface:?} flags={flags:#x}"));
        }
        self.stats.lock().unwrap().presents += 1;
        let n = PRESENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        {
            let mut times = PRESENT_TIMES.lock().unwrap();
            let now = std::time::Instant::now();
            // A heartbeat for long runs: where the run is, and how fast the last 100 went.
            if n.is_multiple_of(100) && times.len() >= 100 {
                let secs = now.duration_since(times[times.len() - 100]).as_secs_f64();
                eprintln!("present {n}: last 100 at {:.2} fps", 100.0 / secs);
            }
            times.push(now);
        }
        self.in_frame += 1;
        if let Some(l) = &mut self.log {
            let _ = l.flush();
        }
        self.inner.present(surface, flags)
    }
    fn clear_rects(&mut self, color: Self::Surface, depth: Option<Self::Surface>, clear: &RectClear) -> Result<DrawOutcome, String> {
        if let Some(c) = self.capturing() {
            c.append("frame.txt", &format!("clear_rects color={color:?} depth={depth:?} {clear:#?}"));
        }
        let r = self.inner.clear_rects(color, depth, clear);
        self.line(format!("clear_rects {color:?} {} rects -> {}", clear.rects.len(), outcome_name(&r)));
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
        self.line(format!("read_surface {surface:?} {format:?} {width}x{height}"));
        if let Some(c) = self.capturing() {
            c.append("frame.txt", &format!("read_surface {surface:?} {format:?} {width}x{height}"));
        }
        self.inner.read_surface(surface, format, width, height, layout)
    }
    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        self.line(format!("copy_surface {src:?} -> {dst:?}"));
        if let Some(c) = self.capturing() {
            c.append("frame.txt", &format!("copy_surface {src:?} -> {dst:?}"));
        }
        self.inner.copy_surface(src, dst)
    }
    fn write_surface(&mut self, surface: Self::Surface, desc: &TextureDesc, bytes: &[u8]) -> Result<bool, String> {
        self.line(format!("write_surface {surface:?}"));
        if let Some(c) = &mut self.capture {
            let n = c.writes;
            c.writes += 1;
            c.blob(&format!("write-{n}.bin"), bytes);
            c.append("writes.txt", &format!("write {n} frame {} {surface:?} {desc:?}", self.in_frame));
        }
        self.inner.write_surface(surface, desc, bytes)
    }
    fn create_texture_target(&mut self, format: D3DFormat, width: u32, height: u32) -> Result<Self::Surface, String> {
        let r = self.inner.create_texture_target(format, width, height);
        self.line(format!("create_texture_target {format:?} {width}x{height} -> {r:?}"));
        if let Some(c) = &self.capture {
            c.append("surfaces.txt", &format!("frame {} create_texture_target {format:?} {width}x{height} -> {r:?}", self.in_frame));
        }
        r
    }
}

#[cfg(test)]
mod tests {
    /// Every `Backend` method must be forwarded by `DiagBackend`; a missed
    /// provided method silently falls back to the trait's default.
    #[test]
    fn forwards_every_backend_method() {
        fn methods(src: &str, start: &str) -> std::collections::BTreeSet<String> {
            let body = &src[src.find(start).expect(start)..];
            let body = &body[..body.find("\n}\n").unwrap()];
            body.lines()
                .filter_map(|l| l.trim().strip_prefix("fn ").map(|r| r.split(['(', '<']).next().unwrap().to_string()))
                .collect()
        }
        let tr = methods(include_str!("../../d3d/src/backend.rs"), "pub trait Backend");
        let ours = methods(include_str!("d3d_diag.rs"), "impl<B: Backend> Backend for DiagBackend<B>");
        let missing: Vec<_> = tr.difference(&ours).collect();
        assert!(missing.is_empty(), "DiagBackend does not forward {missing:?}");
        let headless = methods(include_str!("d3d_glue.rs"), "impl d3d::backend::Backend for Headless");
        let missing: Vec<_> = tr.difference(&headless).collect();
        assert!(missing.is_empty(), "Headless does not forward {missing:?}");
    }
}
