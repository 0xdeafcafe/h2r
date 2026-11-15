//! Draw diagnostics for the boot tracer: a [`Backend`] wrapper that keeps a
//! histogram of what reaches the backend and, with `H2_BOOT_DRAWLOG=<file>`,
//! writes one line per draw for the frames in `H2_BOOT_DRAWLOG_FRAMES`
//! (comma-separated present counts, default `240`).
//!
//! Frame N is the draws after the (N-1)th present, so it matches
//! `frame-N.png` from `H2_BOOT_GFX`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::{Arc, Mutex};

use d3d::backend::{Backend, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw};
use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

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
        (DiagBackend { inner, stats: Arc::clone(&stats), log, frames, in_frame: 1 }, stats)
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
    type Texture = B::Texture;

    fn create_surface(&mut self, format: D3DFormat, width: u32, height: u32, depth: bool) -> Result<Self::Surface, String> {
        let r = self.inner.create_surface(format, width, height, depth);
        self.line(format!("create_surface {format:?} {width}x{height} depth={depth} -> {r:?}"));
        r
    }
    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        self.inner.destroy_surface(surface)
    }
    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        let r = self.inner.upload_texture(texture);
        if let Err(e) = &r {
            self.line(format!("upload_texture failed: {e}"));
        }
        r
    }
    fn bc_supported(&self) -> bool {
        self.inner.bc_supported()
    }
    fn clear(&mut self, color: Self::Surface, depth: Option<Self::Surface>, clear: Clear) -> Result<(), String> {
        self.line(format!("clear {color:?} depth={depth:?} {clear:?}"));
        self.inner.clear(color, depth, clear)
    }
    fn draw(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: &ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String> {
        let r = self.inner.draw(color, depth, draw, textures);
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
        self.stats.lock().unwrap().presents += 1;
        self.in_frame += 1;
        if let Some(l) = &mut self.log {
            let _ = l.flush();
        }
        self.inner.present(surface, flags)
    }
    fn clear_rects(&mut self, color: Self::Surface, depth: Option<Self::Surface>, clear: &RectClear) -> Result<DrawOutcome, String> {
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
        self.inner.read_surface(surface, format, width, height, layout)
    }
    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        self.line(format!("copy_surface {src:?} -> {dst:?}"));
        self.inner.copy_surface(src, dst)
    }
    fn write_surface(&mut self, surface: Self::Surface, desc: &TextureDesc, bytes: &[u8]) -> Result<bool, String> {
        self.line(format!("write_surface {surface:?}"));
        self.inner.write_surface(surface, desc, bytes)
    }
    fn create_texture_target(&mut self, format: D3DFormat, width: u32, height: u32) -> Result<Self::Surface, String> {
        let r = self.inner.create_texture_target(format, width, height);
        self.line(format!("create_texture_target {format:?} {width}x{height} -> {r:?}"));
        r
    }
}
