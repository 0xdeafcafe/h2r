//! [`Backend`] on `gfx`: surfaces in a `SurfaceStore`, draws through
//! `gfx::Renderer`, textures through `gfx::upload`.
//!
//! What renders:
//! * pre-transformed (`XYZRHW | DIFFUSE`) fixed-function draws whose whole
//!   state the built-in shader reproduces (see
//!   [`crate::nv2a::builtin_equivalent`]: no texture,
//!   fog or specular, stages passing diffuse through) through gfx's built-in
//!   shader (alpha test included), with the depth format's z full scale;
//! * everything else through the installed [`ShaderProvider`]:
//!   [`Nv2aShaders`] by default (translated NV2A vertex programs and
//!   register combiners, plus the fixed-function texture stages turned into
//!   combiners). A draw the provider cannot translate is skipped with the
//!   reason.
//!
//! Every draw is clipped with a scissor to [`ResolvedDraw::clip`] (the
//! viewport narrowed by the target and the guest scissor); when the guest
//! scissors leave several pieces ([`ResolvedDraw::clip_rects`]) the draw is
//! recorded once per piece in the same pass, each with its own scissor
//! (the pieces are disjoint, so blending sees every pixel once). The wgpu
//! viewport stays the whole target: vertex positions are already window
//! coordinates (pre-transformed vertices, and NV2A programs apply the
//! viewport themselves), so a `gfx::DrawViewport` would transform them
//! twice. Indexed draws pass their indices through `gfx::Draw::indices`.
//!
//! Partial and channel-masked clears ([`Backend::clear_rects`]) go to
//! `gfx::Renderer::clear_rects`. Render-to-texture levels are
//! `SurfaceStore::create_texture_target` surfaces, filled from guest memory
//! with `SurfaceStore::write_color` and read back with `gfx::readback` and
//! `gfx::encode`; depth levels are `SurfaceStore` depth surfaces, filled
//! with `SurfaceStore::write_depth` and read back with
//! `SurfaceStore::read_depth_as` (which also serves textures over the depth
//! buffer, [`Backend::read_depth`]). A texture over the back buffer is
//! sampled from a `SurfaceStore::snapshot` copy
//! ([`Backend::snapshot_surface`]).
//! `set_pretransformed_z_max` is renderer-wide, so it is set
//! before every built-in draw.
//!
//! Draws are batched: consecutive draws (and a clear before them) to the
//! same colour and depth surfaces are recorded into one pending render
//! pass, their vertex, index and uniform bytes packed into the renderer's
//! and the shader provider's upload rings. Anything else that touches
//! surfaces (another target, partial clears, surface reads, writes, copies
//! and snapshots, destruction, present) first submits the pending pass, so
//! the GPU sees the same operations in the same order as one pass per draw
//! would give it. The pending pass is encoded as one render pass, except
//! that a draw that follows a draw writing only some colour channels starts
//! a render pass of its own, and under the exact rule so does every draw
//! whose program can `discard` (see [`GfxBackend::set_merge_passes`] and
//! [`GfxBackend::set_merge_discards`]); with merging off every draw gets
//! its own render pass.
//!
//! Visibility tests: a draw issued while `SET_ZPASS_PIXEL_COUNT_ENABLE` is
//! set has its samples counted into a [`gfx::occlusion::PixelCounter`]
//! (a count pass before the draw; see that module for what is and is not
//! counted). `report_pixel_count` copies the total without waiting and
//! `read_pixel_count` waits for the GPU. A counted draw that is skipped (no
//! translation) counts nothing. Clearing and reporting the counter submit
//! the pending pass first, so they stay in order with the counted draws.

use std::collections::HashMap;

use gfx::occlusion::{PixelCounter, PixelReport};
use gfx::renderer::{SHADER_XYZRHW_DIFFUSE, d3dz_max};
use gfx::surface::{SurfaceDesc, SurfaceId, SurfaceKind, SurfaceStore};
use gfx::texture::DecodedTexture;
use gfx::texture::TextureKind;
use gfx::{
    Clear, D3DFormat, Draw, GfxContext, MaskedClear, RecordedDraw, Renderer, ScissorRect,
    VertexStream,
};

use crate::backend::{Backend, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw};
use crate::nv2a::{Nv2aShaders, builtin_equivalent};
use crate::shader::{ShaderProvider, VertexProcessing};

/// Called on every `Swap` with the surface shown and the swap flags.
pub type PresentHook =
    Box<dyn FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>) + Send>;

/// A texture uploaded for the guest, with the view the shaders sample it
/// through (`None` when its format cannot be bound as a filterable float
/// texture: the program then reads a stand-in).
#[derive(Debug)]
pub struct HostTexture {
    /// The texture.
    pub texture: wgpu::Texture,
    view: Option<wgpu::TextureView>,
}

/// The render pass being recorded: its target, the clear it starts with and
/// its draws.
struct PendingPass {
    color: SurfaceId,
    depth: Option<SurfaceId>,
    clear: Clear,
    draws: Vec<RecordedDraw>,
    /// Passes (render and compute) its draws will be encoded as, about.
    encoded: usize,
    /// Each draw's number in its frame (backend `draw` calls since the last
    /// present, from 0), for the merge self-check.
    tags: Vec<u64>,
}

/// Passes one submission may hold before the pending pass is submitted and
/// a new one started for the same target. Metal loses the device when one
/// command buffer holds about 2048 render passes (one per draw, unless
/// passes are merged); the split changes nothing drawn, since each draw's
/// render pass loads what the one before it stored.
const MAX_ENCODED_PASSES: usize = 1024;

/// Rendering through `gfx`.
pub struct GfxBackend {
    /// The GPU context.
    pub ctx: GfxContext,
    /// All surfaces.
    pub store: SurfaceStore,
    /// The draw path.
    pub renderer: Renderer,
    shaders: Option<Box<dyn ShaderProvider>>,
    presented: Option<SurfaceId>,
    present_hook: Option<PresentHook>,
    pending: Option<PendingPass>,
    stats: BatchStats,
    merge_passes: bool,
    /// [`set_merge_discards`](Self::set_merge_discards).
    merge_discards: bool,
    /// Made at the first counted draw.
    counter: Option<PixelCounter>,
    /// The depth-bits pass ([`Backend::depth_level_texture`]), made on first use.
    depth_bits: Option<gfx::depth_bits::DepthBits>,
    reports: HashMap<u64, PixelReport>,
    next_ticket: u64,
    /// Presents so far plus one.
    frame: u64,
    /// Backend `draw` calls in this frame so far.
    frame_draw: u64,
    /// The frame whose passes the merge self-check compares.
    merge_check_frame: Option<u64>,
}

/// How draws were grouped into passes and submissions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchStats {
    /// Draws recorded (each scissor piece counts).
    pub draws: u64,
    /// Pending passes submitted: consecutive draws to one target, and a
    /// whole-target clear before them.
    pub passes: u64,
    /// Render passes those were encoded as (one per draw unless passes are
    /// merged, plus the count passes of visibility tests).
    pub render_passes: u64,
    /// Queue submissions.
    pub submits: u64,
}

impl GfxBackend {
    /// A backend on `ctx`, translating shaders with [`Nv2aShaders`].
    pub fn new(ctx: GfxContext) -> Self {
        let store = SurfaceStore::new(ctx.caps);
        let renderer = Renderer::new(&ctx);
        Self {
            ctx,
            store,
            renderer,
            shaders: Some(Box::new(Nv2aShaders::new())),
            presented: None,
            present_hook: None,
            pending: None,
            stats: BatchStats::default(),
            merge_passes: true,
            merge_discards: merge_discards_from_env(
                std::env::var("H2_GFX_MERGE_PASSES").ok().as_deref(),
            ),
            counter: None,
            reports: HashMap::new(),
            depth_bits: None,
            next_ticket: 0,
            frame: 1,
            frame_draw: 0,
            merge_check_frame: None,
        }
    }

    /// Pass and submission counts so far.
    pub fn batch_stats(&self) -> BatchStats {
        self.stats
    }

    /// Encode the draws of a pending pass into one render pass (the
    /// default) or one render pass per draw (both in one submission).
    /// Merged passes cost much less (no Metal render encoder per draw, no
    /// attachment store and load between draws). A draw that follows a
    /// draw writing only some colour channels (unless it writes the same
    /// ones, and cannot `discard`) still starts a render pass of its own,
    /// because on Apple GPUs its edge pixels otherwise depend on earlier
    /// draws in the pass; under the exact rule
    /// ([`set_merge_discards`](Self::set_merge_discards) off) so does every
    /// draw whose program can `discard` (`PreparedShader::discards`). With
    /// the exact rule the main menu matches one pass per draw byte for
    /// byte; one pass per draw is the exact reference (docs/graphics.md,
    /// "Merged passes").
    pub fn set_merge_passes(&mut self, merge: bool) {
        self.merge_passes = merge;
    }

    /// The fast merge rule (the default): a draw whose program can
    /// `discard` starts a render pass of its own only after a draw that
    /// writes some colour channels and not others ([`merge_split`]),
    /// instead of always (the exact rule, `false`). In a level that is most
    /// of the render passes (every alpha-tested draw had one under the
    /// exact rule), and each costs about 40-75 us of CPU in wgpu and Metal.
    /// Not proven exact: at The Heretic's frame 4216 the merge self-check
    /// finds 61 edge pixels in two passes off by up to 4 levels, after
    /// additive draws (docs/graphics.md, "Merged passes").
    /// `H2_GFX_MERGE_PASSES=exact` turns it off when the backend is made.
    pub fn set_merge_discards(&mut self, on: bool) {
        self.merge_discards = on;
    }

    /// Compare merged and per-draw render passes for every pending pass of
    /// `frame` (presents so far plus one) and print what differs to
    /// stderr: the pixels, the first draw that shows it and a smallest set
    /// of earlier draws it needs (draw numbers count backend `draw` calls
    /// in the frame, as `H2_BOOT_CAPTURE` does). Slow: a diagnostic.
    pub fn set_merge_check(&mut self, frame: Option<u64>) {
        self.merge_check_frame = frame;
    }

    /// Replaces the shader translator for textured and programmable draws.
    pub fn with_shaders(mut self, shaders: Box<dyn ShaderProvider>) -> Self {
        self.shaders = Some(shaders);
        self
    }

    /// Called on every `Swap` with the surface, flags and borrowed RGB gamma
    /// ramp. `app::FrameSink` applies the ramp during its handoff; custom
    /// hooks must apply it themselves. `gfx::surface::present_surface` is
    /// a raw presentation helper and does not apply the ramp.
    ///
    /// ```no_run
    /// fn install(backend: d3d::GfxBackend) -> d3d::GfxBackend {
    ///     backend.on_present(|_ctx, _store, _surface, _flags, gamma| {
    ///         let _options = gfx::blit::BlitOptions { gamma, ..Default::default() };
    ///         // Pass these options to the blit that publishes the frame.
    ///     })
    /// }
    /// ```
    pub fn on_present(
        mut self,
        hook: impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>)
        + Send
        + 'static,
    ) -> Self {
        self.present_hook = Some(Box::new(hook));
        self
    }

    /// Translates shaders with [`Nv2aShaders`] recording every pipeline to
    /// the cache file at `path` and building the ones already in it ahead of
    /// time, on worker threads (see [`crate::pipeline_store`]). Rendering is
    /// the same with or without it: a draw whose pipeline is still being
    /// built waits for it when its pass is encoded.
    pub fn with_pipeline_cache(mut self, path: std::path::PathBuf) -> Self {
        let mut shaders = Nv2aShaders::with_pipeline_cache(path);
        shaders.maintain(&self.ctx, &mut self.renderer, &[]);
        self.shaders = Some(Box::new(shaders));
        self
    }

    /// Removes the shader translator: only draws the built-in shader handles
    /// render; the rest are skipped.
    pub fn without_shaders(mut self) -> Self {
        self.shaders = None;
        self
    }

    /// The surface shown by the last `Swap`.
    pub fn last_presented(&self) -> Option<SurfaceId> {
        self.presented
    }

    /// `texture` with the view the shaders sample a `kind` texture through,
    /// if its format can be bound as a filterable float texture.
    fn host_texture(&self, texture: wgpu::Texture, kind: TextureKind) -> HostTexture {
        let view =
            gfx::program::filterable(texture.format(), self.ctx.device.features()).then(|| {
                let dimension = match kind {
                    TextureKind::D2 => wgpu::TextureViewDimension::D2,
                    TextureKind::Cube => wgpu::TextureViewDimension::Cube,
                    TextureKind::Volume => wgpu::TextureViewDimension::D3,
                };
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(dimension),
                    ..Default::default()
                })
            });
        HostTexture { texture, view }
    }

    /// Reads a colour surface back (after submitting the pending pass), in
    /// host pixel order.
    pub fn read_color(
        &mut self,
        id: SurfaceId,
    ) -> Result<(Vec<u8>, gfx::encode::HostPixelOrder), String> {
        self.flush()?;
        self.store
            .read_color(&self.ctx, id)
            .map_err(|e| format!("{e:?}"))
    }

    /// Submits the pending pass, if any.
    pub fn flush(&mut self) -> Result<(), String> {
        let Some(mut p) = self.pending.take() else {
            return Ok(());
        };
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        if self.merge_check_frame == Some(self.frame)
            && p.draws.len() > 1
            && !p.draws.iter().any(RecordedDraw::counts_samples)
        {
            self.merge_check(&mut p)?;
        }
        let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "flush pass")
            .arg("draws", p.draws.len() as u64);
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        if let Some(d) = p.depth {
            self.store
                .prepare_target(&self.ctx, &mut enc, p.color, d)
                .map_err(|e| e.to_string())?;
        }
        {
            let target = self
                .store
                .target(p.color, p.depth)
                .map_err(|e| e.to_string())?;
            let ctx = &self.ctx;
            let counter = p
                .draws
                .iter()
                .any(RecordedDraw::counts_samples)
                .then(|| self.counter.get_or_insert_with(|| PixelCounter::new(ctx)));
            let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "render encode");
            self.stats.render_passes +=
                Renderer::encode_pass(ctx, &mut enc, &target, p.clear, &p.draws, counter)
                    .map_err(|e| e.to_string())? as u64;
        }
        self.store
            .finish_target(&mut enc, p.color)
            .map_err(|e| e.to_string())?;
        self.submit(enc);
        self.stats.passes += 1;
        Ok(())
    }

    /// The merge self-check ([`set_merge_check`](Self::set_merge_check)):
    /// renders `p` merged and with one render pass per draw from the same
    /// starting attachments, reports the pixels that differ, and narrows a
    /// difference down to the first draw that shows it and a smallest set of
    /// earlier draws it needs. Leaves the attachments and `p` as they were.
    fn merge_check(&mut self, p: &mut PendingPass) -> Result<(), String> {
        let ctx = &self.ctx;
        self.renderer.flush_uploads(&ctx.queue);
        if let Some(s) = self.shaders.as_mut() {
            s.flush_uploads(ctx);
        }
        let surface = |id| {
            self.store
                .get(id)
                .map(|s| s.texture().clone())
                .map_err(|e| format!("{e:?}"))
        };
        let mut attachments = vec![surface(p.color)?];
        if let Some(d) = p.depth {
            attachments.push(surface(d)?);
        }
        if attachments
            .iter()
            .any(|t| t.size() != attachments[0].size() || t.sample_count() > 1)
        {
            eprintln!("merge-check: padded or multisampled target, skipped");
            return Ok(());
        }
        let backups: Vec<wgpu::Texture> = attachments
            .iter()
            .map(|t| {
                ctx.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("merge check backup"),
                    size: t.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: t.format(),
                    usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            })
            .collect();
        let copy_all = |from_backup: bool| {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            for (t, b) in attachments.iter().zip(&backups) {
                let (src, dst) = if from_backup { (b, t) } else { (t, b) };
                enc.copy_texture_to_texture(src.as_image_copy(), dst.as_image_copy(), src.size());
            }
            ctx.queue.submit([enc.finish()]);
        };
        copy_all(false);
        let merged_flags: Vec<bool> = p.draws.iter().map(RecordedDraw::starts_pass).collect();
        let store = &self.store;
        // Renders the draws `sel` (indices into `draws`, in order) from the
        // backed-up attachments, merged (each draw's own flag) or one render
        // pass per draw, and reads the colour back.
        let render = |draws: &mut Vec<RecordedDraw>,
                      sel: &[usize],
                      merged: bool|
         -> Result<Vec<u8>, String> {
            let mut slots: Vec<Option<RecordedDraw>> = draws.drain(..).map(Some).collect();
            let mut picked: Vec<RecordedDraw> = sel
                .iter()
                .map(|&i| {
                    let mut d = slots[i].take().expect("indices are distinct");
                    d.set_start_pass(!merged || merged_flags[i]);
                    d
                })
                .collect();
            copy_all(true);
            let mut result = store.target(p.color, p.depth).map_err(|e| e.to_string());
            if let Ok(target) = &result {
                // Bounded submissions: Metal loses the device at about 2048
                // render passes in one command buffer.
                let mut clear = p.clear;
                let mut encoded = Ok(());
                for chunk in picked.chunks(MAX_ENCODED_PASSES / 2) {
                    let mut enc = ctx.device.create_command_encoder(&Default::default());
                    encoded = encoded.and(
                        Renderer::encode_pass(ctx, &mut enc, target, clear, chunk, None)
                            .map(|_| ())
                            .map_err(|e| e.to_string()),
                    );
                    clear = Clear::NONE;
                    ctx.queue.submit([enc.finish()]);
                }
                if let Err(e) = encoded {
                    result = Err(e);
                }
            }
            for (&i, d) in sel.iter().zip(picked.drain(..)) {
                slots[i] = Some(d);
            }
            draws.extend(
                slots
                    .into_iter()
                    .map(|d| d.expect("every draw is put back")),
            );
            result?;
            Ok(store
                .read_color(ctx, p.color)
                .map_err(|e| format!("{e:?}"))?
                .0)
        };
        let differs = |draws: &mut Vec<RecordedDraw>, sel: &[usize]| -> Result<bool, String> {
            Ok(render(draws, sel, true)? != render(draws, sel, false)?)
        };
        let all: Vec<usize> = (0..p.draws.len()).collect();
        let tags = p.tags.clone();
        let width = attachments[0].width() as usize;
        let draws = &mut p.draws;
        let merged = render(draws, &all, true)?;
        let split = render(draws, &all, false)?;
        let differing: Vec<usize> = (0..merged.len() / 4)
            .filter(|&i| merged[i * 4..i * 4 + 4] != split[i * 4..i * 4 + 4])
            .collect();
        let max = merged
            .iter()
            .zip(&split)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        eprintln!(
            "merge-check frame {}: pass of {} draws (draws {}..={} of the frame): {} pixels differ, by up to {max}",
            self.frame,
            all.len(),
            tags[0],
            tags[all.len() - 1],
            differing.len()
        );
        for &i in differing.iter().take(8) {
            let at = |px: &[u8]| -> [u8; 4] { px[i * 4..i * 4 + 4].try_into().expect("4 bytes") };
            eprintln!(
                "  ({}, {}): merged {:?}, per draw {:?}",
                i % width,
                i / width,
                at(&merged),
                at(&split)
            );
        }
        if !differing.is_empty() {
            // The shortest prefix that differs ends at the first draw that
            // shows the difference.
            let (mut lo, mut hi) = (1, all.len());
            while lo < hi {
                let mid = (lo + hi) / 2;
                if differs(draws, &all[..mid])? {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            let last = lo - 1;
            // Drop earlier draws, in halving chunks, while the difference
            // stays.
            let mut set: Vec<usize> = (0..last).collect();
            let mut chunk = set.len().div_ceil(2).max(1);
            loop {
                let mut i = 0;
                while i < set.len() {
                    let mut trial = set.clone();
                    trial.drain(i..(i + chunk).min(set.len()));
                    trial.push(last);
                    if differs(draws, &trial)? {
                        trial.pop();
                        set = trial;
                    } else {
                        i += chunk;
                    }
                }
                if chunk == 1 {
                    break;
                }
                chunk = chunk.div_ceil(2);
            }
            set.push(last);
            eprintln!(
                "  first differs at draw {} of the frame; smallest set that still differs: {:?}",
                tags[last],
                set.iter().map(|&i| tags[i]).collect::<Vec<_>>()
            );
            for &i in &set {
                eprintln!("    draw {}: {}", tags[i], draws[i].describe());
            }
        }
        copy_all(true);
        for (d, on) in draws.iter_mut().zip(merged_flags) {
            d.set_start_pass(on);
        }
        Ok(())
    }

    /// Writes every staged upload, submits `enc` and lets the rings reuse
    /// their space.
    fn submit(&mut self, enc: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        self.renderer.flush_uploads(&self.ctx.queue);
        if let Some(s) = self.shaders.as_mut() {
            s.flush_uploads(&self.ctx);
        }
        let index = {
            let _p = timeline::phase::enter(timeline::phase::Phase::Submit);
            timeline::phase::count(timeline::phase::Counter::Submits, 1);
            let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "queue submit");
            self.ctx.queue.submit([enc.finish()])
        };
        self.renderer.submitted();
        if let Some(s) = self.shaders.as_mut() {
            s.submitted();
        }
        self.stats.submits += 1;
        index
    }

    /// The pending pass for `color` and `depth`, submitting one for another
    /// target first.
    fn pending_for(
        &mut self,
        color: SurfaceId,
        depth: Option<SurfaceId>,
    ) -> Result<&mut PendingPass, String> {
        self.switch_to(color, depth)?;
        Ok(self.pending.get_or_insert_with(|| PendingPass {
            color,
            depth,
            clear: Clear::NONE,
            draws: Vec::new(),
            encoded: 1,
            tags: Vec::new(),
        }))
    }

    /// Submits the pending pass if it is for another target. Must run before
    /// anything for a draw on `color` and `depth` is staged (the shader
    /// provider's uniforms included): the submit lets every ring reuse what
    /// was staged before it.
    fn switch_to(&mut self, color: SurfaceId, depth: Option<SurfaceId>) -> Result<(), String> {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.color != color || p.depth != depth)
        {
            self.flush()?;
        }
        Ok(())
    }

    /// Records `draw` into the pending pass for `color` and `depth` (after
    /// [`switch_to`](Self::switch_to)).
    /// `first` is the first scissor piece of a backend draw: unless passes
    /// are merged, it starts a render pass of its own.
    fn record(
        &mut self,
        color: SurfaceId,
        depth: Option<SurfaceId>,
        draw: &Draw<'_>,
        first: bool,
        discards: bool,
    ) -> Result<(), String> {
        debug_assert!(
            self.pending
                .as_ref()
                .is_none_or(|p| p.color == color && p.depth == depth)
        );
        let mut recorded = {
            let target = self.store.target(color, depth).map_err(|e| e.to_string())?;
            self.renderer
                .record(&self.ctx, &target, draw)
                .map_err(|e| e.to_string())?
        };
        // With merged passes, a draw that follows a draw writing only some
        // colour channels (unless it writes the same ones) starts a render
        // pass of its own: on Apple GPUs its edge pixels otherwise depend
        // on earlier draws in the pass (docs/graphics.md, "Merged passes").
        // The fast rule (the default) splits a discarding draw only after a
        // partly masked one; the exact rule splits every discarding draw.
        let prev_mask = self
            .pending
            .as_ref()
            .and_then(|p| p.draws.last())
            .map(RecordedDraw::color_writes);
        let mask = recorded.color_writes();
        let split = if self.merge_discards {
            merge_split(prev_mask, mask, discards)
        } else {
            discards || merge_split(prev_mask, mask, false)
        };
        let own_pass = first && (!self.merge_passes || split);
        // A counted draw adds its count pass and the sum, and the draws
        // after it resume in a new render pass.
        let encoded = if recorded.counts_samples() {
            3
        } else {
            usize::from(own_pass)
        };
        if own_pass {
            recorded.start_pass();
        }
        let tag = self.frame_draw.wrapping_sub(1);
        let pending = self.pending_for(color, depth)?;
        pending.draws.push(recorded);
        pending.encoded += encoded;
        pending.tags.push(tag);
        self.stats.draws += 1;
        Ok(())
    }
}

impl Backend for GfxBackend {
    type Surface = SurfaceId;
    type Texture = HostTexture;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<SurfaceId, String> {
        let kind = if depth {
            SurfaceKind::DepthStencil
        } else {
            SurfaceKind::Color
        };
        self.store
            .create(&self.ctx, SurfaceDesc::new(format, width, height), kind)
            .map_err(|e| e.to_string())
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), String> {
        self.flush()?;
        if self.store.remove(surface) {
            Ok(())
        } else {
            Err(format!("surface {surface:?} is not in the store"))
        }
    }

    fn upload_texture(&mut self, t: &DecodedTexture) -> Result<HostTexture, String> {
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        timeline::phase::count(timeline::phase::Counter::Uploads, 1);
        let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "texture upload");
        let texture =
            gfx::upload::upload_texture(&self.ctx.device, &self.ctx.queue, t, "guest texture");
        Ok(self.host_texture(texture, t.desc.kind))
    }

    fn bc_supported(&self) -> bool {
        self.ctx.caps.texture_compression_bc
    }

    fn judge(&self) -> Option<Box<dyn crate::backend::DrawJudge>> {
        let shaders = match &self.shaders {
            Some(s) => Some(s.judge(self.ctx.caps)?),
            None => None,
        };
        Some(Box::new(GfxJudge {
            features: self.ctx.device.features(),
            shaders,
        }))
    }

    fn clear(
        &mut self,
        color: SurfaceId,
        depth: Option<SurfaceId>,
        clear: Clear,
    ) -> Result<(), String> {
        // A clear starts a pass: it becomes the pass's load operations.
        if self.pending.as_ref().is_some_and(|p| !p.draws.is_empty()) {
            self.flush()?;
        }
        let p = self.pending_for(color, depth)?;
        p.clear = Clear {
            color: clear.color.or(p.clear.color),
            depth: clear.depth.or(p.clear.depth),
            stencil: clear.stencil.or(p.clear.stencil),
        };
        Ok(())
    }

    fn draw(
        &mut self,
        color: SurfaceId,
        depth: Option<SurfaceId>,
        d: &ResolvedDraw,
        textures: [Option<&HostTexture>; 4],
    ) -> Result<DrawOutcome, String> {
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        timeline::phase::count(timeline::phase::Counter::Draws, 1);
        let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "draw");
        // A full pass is submitted here, before the draw stages anything,
        // like a target switch.
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.encoded >= MAX_ENCODED_PASSES)
        {
            self.flush()?;
        }
        self.frame_draw += 1;
        self.switch_to(color, depth)?;
        let streams: Vec<VertexStream<'_>> = d
            .streams
            .iter()
            .map(|s| VertexStream {
                data: &s.data,
                stride: s.stride,
            })
            .collect();
        // Window-coordinate geometry: the clip rectangle only scissors.
        let pieces: Vec<Option<ScissorRect>> = d
            .clip_rects
            .iter()
            .map(|c| {
                Some(ScissorRect {
                    x: c.x,
                    y: c.y,
                    width: c.width,
                    height: c.height,
                })
            })
            .collect();
        let scissor = Some(match d.clip {
            Some(c) => ScissorRect {
                x: c.x,
                y: c.y,
                width: c.width,
                height: c.height,
            },
            None => {
                let v = d.viewport;
                ScissorRect::from_edges(
                    v.x as i64,
                    v.y as i64,
                    v.x as i64 + v.width as i64,
                    v.y as i64 + v.height as i64,
                )
            }
        });
        let indices = d.indices.as_deref();
        let textured = textures.iter().any(Option::is_some);
        let builtin = match route(d, textured, self.shaders.is_some()) {
            Ok(b) => b,
            Err(why) => return Ok(DrawOutcome::Skipped(why)),
        };
        if builtin {
            self.renderer
                .set_pretransformed_z_max(d3dz_max(d.target.depth_format));
            let draw = Draw {
                scissor,
                indices,
                count_samples: d.hw.zpass_count_enable,
                ..Draw::new(
                    SHADER_XYZRHW_DIFFUSE,
                    &d.layout,
                    &streams,
                    d.vertex_count,
                    d.primitive,
                    d.state.render,
                )
            };
            for (i, piece) in split(draw, &pieces).iter().enumerate() {
                self.record(color, depth, piece, i == 0, false)?;
            }
            return Ok(DrawOutcome::Drawn);
        }
        let shaders = self.shaders.as_mut().expect("checked above");
        // Views as the shaders sample them; a texture that cannot be bound
        // as a filterable float texture is left out (the program reads a
        // stand-in).
        let view_refs: [Option<&wgpu::TextureView>; 4] = std::array::from_fn(|i| {
            d.texture_info[i]?;
            textures[i]?.view.as_ref()
        });
        let prepare = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "shader prepare");
        let prepared = match shaders.prepare(&self.ctx, &mut self.renderer, d, view_refs) {
            Ok(p) => p,
            Err(e) => return Ok(DrawOutcome::Skipped(e)),
        };
        drop(prepare);
        let mut render = d.state.render;
        if prepared.writes_depth {
            // The program adds the offset to the depth it writes.
            render.solid_offset_enable = false;
            render.wireframe_offset_enable = false;
            render.point_offset_enable = false;
        }
        let draw = Draw {
            bind_groups: &prepared.bind_groups,
            scissor,
            indices,
            count_samples: d.hw.zpass_count_enable,
            ..Draw::new(
                prepared.id,
                &d.layout,
                &streams,
                d.vertex_count,
                d.primitive,
                render,
            )
        };
        for (i, piece) in split(draw, &pieces).iter().enumerate() {
            self.record(color, depth, piece, i == 0, prepared.discards)?;
        }
        Ok(DrawOutcome::Drawn)
    }

    fn clear_rects(
        &mut self,
        color: SurfaceId,
        depth: Option<SurfaceId>,
        c: &RectClear,
    ) -> Result<DrawOutcome, String> {
        let (rgba, mask) = c.color.unzip();
        let mut channels = wgpu::ColorWrites::empty();
        if let Some(m) = mask {
            for (on, w) in [
                (m.red, wgpu::ColorWrites::RED),
                (m.green, wgpu::ColorWrites::GREEN),
                (m.blue, wgpu::ColorWrites::BLUE),
                (m.alpha, wgpu::ColorWrites::ALPHA),
            ] {
                if on {
                    channels |= w;
                }
            }
        }
        let clear = MaskedClear {
            color: rgba,
            channels,
            depth: c.depth,
            stencil: c.stencil,
        };
        let rects: Vec<ScissorRect> = c
            .rects
            .iter()
            .map(|r| ScissorRect {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            })
            .collect();
        self.flush()?;
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        if let Some(d) = depth {
            self.store
                .prepare_target(&self.ctx, &mut enc, color, d)
                .map_err(|e| e.to_string())?;
        }
        {
            let target = self.store.target(color, depth).map_err(|e| e.to_string())?;
            self.renderer
                .clear_rects(&self.ctx, &mut enc, &target, clear, &rects);
        }
        self.store
            .finish_target(&mut enc, color)
            .map_err(|e| e.to_string())?;
        self.submit(enc);
        Ok(DrawOutcome::Drawn)
    }

    fn write_surface(
        &mut self,
        surface: SurfaceId,
        desc: &gfx::TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.flush()?;
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "surface write");
        let kind = self.store.get(surface).map_err(|e| e.to_string())?.kind();
        match kind {
            SurfaceKind::Color => self.store.write_color(&self.ctx, surface, desc, bytes),
            SurfaceKind::DepthStencil => self.store.write_depth(&self.ctx, surface, desc, bytes),
        }
        .map_err(|e| e.to_string())?;
        Ok(true)
    }

    fn read_depth(
        &mut self,
        surface: SurfaceId,
        format: D3DFormat,
        _width: u32,
        _height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        use gfx::surface::{SurfaceError, SurfaceLayout};
        // The bits include everything drawn so far.
        self.flush()?;
        let layout = match layout {
            GuestLayoutKind::Swizzled => SurfaceLayout::Swizzled,
            GuestLayoutKind::Linear { pitch } => SurfaceLayout::Linear { pitch },
        };
        match self
            .store
            .read_depth_as(&self.ctx, surface, format, &layout)
        {
            Ok(b) => Ok(Some(b)),
            // Depth24Plus (no DEPTH32FLOAT_STENCIL8) or multisampled.
            Err(SurfaceError::NotReadable(..)) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn snapshot_surface(
        &mut self,
        surface: SurfaceId,
        opaque: bool,
    ) -> Result<Option<HostTexture>, String> {
        // The copy holds everything drawn so far.
        self.flush()?;
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        let _t = timeline::span(
            timeline::Cat::Gfx,
            timeline::Track::Host,
            "surface snapshot",
        );
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        let t = self
            .store
            .snapshot(&self.ctx, &mut enc, surface)
            .map_err(|e| e.to_string())?;
        if opaque {
            // Alpha to one: an alpha-only masked clear of the copy.
            let view = t.create_view(&Default::default());
            let target = gfx::RenderTarget {
                color: &view,
                color_format: t.format(),
                depth: None,
                depth_format: None,
                size: (t.width(), t.height()),
                samples: 1,
            };
            let clear = MaskedClear {
                color: Some([0.0, 0.0, 0.0, 1.0]),
                channels: wgpu::ColorWrites::ALPHA,
                depth: None,
                stencil: None,
            };
            let all = ScissorRect {
                x: 0,
                y: 0,
                width: t.width(),
                height: t.height(),
            };
            self.renderer
                .clear_rects(&self.ctx, &mut enc, &target, clear, &[all]);
        }
        self.submit(enc);
        Ok(Some(self.host_texture(t, TextureKind::D2)))
    }

    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<SurfaceId, String> {
        self.store
            .create_texture_target(&self.ctx, SurfaceDesc::new(format, width, height), 1)
            .map_err(|e| e.to_string())
    }

    fn can_sample_surfaces(&self) -> bool {
        true
    }

    fn can_sample_depth_surfaces(&self) -> bool {
        // D24S8 surfaces are DEPTH32FLOAT_STENCIL8 only with the feature
        // (Depth24Plus cannot be read at all, here or by `read_depth`).
        self.ctx.caps.depth32_stencil8
    }

    fn depth_level_texture(&mut self, surface: SurfaceId) -> Result<Option<HostTexture>, String> {
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        // After the draws into it.
        self.flush()?;
        let s = self.store.get(surface).map_err(|e| e.to_string())?;
        let texture = s.texture().clone();
        let d = *s.desc();
        if texture.format() != wgpu::TextureFormat::Depth32FloatStencil8 || d.samples > 1 {
            return Ok(None);
        }
        let ctx = &self.ctx;
        let bits = self.depth_bits.get_or_insert_with(|| gfx::depth_bits::DepthBits::new(ctx));
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let out = bits.convert(ctx, &mut enc, &texture, (d.width, d.height));
        self.submit(enc);
        Ok(Some(self.host_texture(out, TextureKind::D2)))
    }

    fn surface_texture(&mut self, surface: SurfaceId) -> Result<Option<HostTexture>, String> {
        // No flush: the draw that binds the view submits a pending pass
        // into `surface` before it records anything (another target).
        let s = self.store.get(surface).map_err(|e| e.to_string())?;
        let texture = s.texture();
        if !texture
            .usage()
            .contains(wgpu::TextureUsages::TEXTURE_BINDING)
        {
            return Ok(None);
        }
        // The surface's own view, the same every time, so the texture
        // groups that hold it stay cached.
        let view = gfx::program::filterable(texture.format(), self.ctx.device.features())
            .then(|| s.view().clone());
        Ok(Some(HostTexture {
            texture: texture.clone(),
            view,
        }))
    }

    fn read_surface(
        &mut self,
        surface: SurfaceId,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        use gfx::encode::HostPixelOrder;
        use gfx::surface::SurfaceLayout;
        self.flush()?;
        // A GPU round trip: submit, wait, map.
        let _p = timeline::phase::enter(timeline::phase::Phase::Wait);
        timeline::phase::count(timeline::phase::Counter::Readbacks, 1);
        let _t = timeline::span(
            timeline::Cat::Gfx,
            timeline::Track::Host,
            "surface readback",
        );
        let s = self.store.get(surface).map_err(|e| e.to_string())?;
        let order = match s.wgpu_format() {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
                HostPixelOrder::Bgra8
            }
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {
                HostPixelOrder::Rgba8
            }
            f => return Err(format!("cannot read back a {f:?} surface")),
        };
        let pixels = gfx::readback::read_texture(
            &self.ctx,
            s.texture(),
            wgpu::TextureAspect::All,
            4,
            (width, height),
        );
        let layout = match layout {
            GuestLayoutKind::Swizzled => SurfaceLayout::Swizzled,
            GuestLayoutKind::Linear { pitch } => SurfaceLayout::Linear { pitch },
        };
        gfx::encode::encode_color(format, order, &pixels, width, height, &layout)
            .map_err(|e| e.to_string())
    }

    fn copy_surface(&mut self, src: SurfaceId, dst: SurfaceId) -> Result<bool, String> {
        self.flush()?;
        let s = *self.store.get(src).map_err(|e| e.to_string())?.desc();
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        self.store
            .copy_rect(
                &mut enc,
                src,
                dst,
                gfx::surface::CopyRect {
                    x: 0,
                    y: 0,
                    width: s.width,
                    height: s.height,
                },
                0,
                0,
            )
            .map_err(|e| e.to_string())?;
        self.submit(enc);
        Ok(true)
    }

    fn present(&mut self, surface: SurfaceId, flags: u32) -> Result<(), String> {
        self.present_with_gamma(surface, flags, None)
    }

    fn present_with_gamma(
        &mut self,
        surface: SurfaceId,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "present");
        self.flush()?;
        self.frame += 1;
        self.frame_draw = 0;
        let new_pipelines = self.renderer.take_new_keys();
        if let Some(s) = self.shaders.as_mut() {
            s.end_frame();
            s.maintain(&self.ctx, &mut self.renderer, &new_pipelines);
        }
        self.presented = Some(surface);
        if let Some(hook) = self.present_hook.as_mut() {
            hook(&self.ctx, &self.store, surface, flags, gamma);
        }
        if flags & 4 != 0 {
            // D3DSWAP_FINISH: wait for the GPU.
            let _p = timeline::phase::enter(timeline::phase::Phase::Wait);
            let _w = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "GPU wait");
            let _ = self.ctx.device.poll(wgpu::PollType::wait_indefinitely());
        }
        // Reports from this frame and before are read later (Halo reads a
        // visibility test two frames after it ends): start their reads now,
        // so the read finds the count mapped instead of waiting.
        for r in self.reports.values() {
            r.try_read(&self.ctx)?;
        }
        Ok(())
    }

    fn clear_pixel_count(&mut self) -> Result<(), String> {
        // After the counted draws before it.
        self.flush()?;
        let ctx = &self.ctx;
        let counter = self.counter.get_or_insert_with(|| PixelCounter::new(ctx));
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        counter.clear(&mut enc);
        self.submit(enc);
        Ok(())
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        // The counted draws before it are in the total.
        self.flush()?;
        let ctx = &self.ctx;
        let counter = self.counter.get_or_insert_with(|| PixelCounter::new(ctx));
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let report = counter.report(ctx, &mut enc);
        // The read waits for this submission only.
        report.submitted(self.submit(enc));
        self.next_ticket += 1;
        self.reports.insert(self.next_ticket, report);
        Ok(Some(self.next_ticket))
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        // Waits for the GPU when the report is not yet back.
        let _p = timeline::phase::enter(timeline::phase::Phase::Wait);
        timeline::phase::count(timeline::phase::Counter::Readbacks, 1);
        let _t = timeline::span(
            timeline::Cat::Gfx,
            timeline::Track::Host,
            "pixel count readback",
        );
        let report = self
            .reports
            .remove(&ticket)
            .ok_or_else(|| format!("pixel-count ticket {ticket} is not live"))?;
        report.read(&self.ctx)
    }

    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        let Some(report) = self.reports.get(&ticket) else {
            return Err(format!("pixel-count ticket {ticket} is not live"));
        };
        let count = report.try_read(&self.ctx)?;
        if count.is_some() {
            self.reports.remove(&ticket);
        }
        Ok(count)
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        self.reports.remove(&ticket);
    }
}

/// The fast merge rule ([`GfxBackend::set_merge_discards`]) for a value
/// of `H2_GFX_MERGE_PASSES`: on unless it is `exact`. (`0`, one render
/// pass per draw, is applied by the embedder through
/// [`GfxBackend::set_merge_passes`] and needs no rule.)
fn merge_discards_from_env(value: Option<&str>) -> bool {
    value != Some("exact")
}

/// Whether a merged draw writing `mask` starts a render pass of its own
/// after a draw in the same pending pass that wrote `prev` (`None` for the
/// first draw): after a partly masked draw, unless it writes the same
/// channels and (`discards`) its program cannot `discard`.
///
/// With `discards` false this is the exact mode's second rule. In the fast
/// mode ([`GfxBackend::set_merge_discards`]) it is the whole rule: because
/// a draw with another mask always splits after a partly masked one, a
/// render pass holds a partly masked draw only while its last draw is one,
/// so checking the previous draw covers every earlier draw in the pass.
fn merge_split(prev: Option<wgpu::ColorWrites>, mask: wgpu::ColorWrites, discards: bool) -> bool {
    prev.is_some_and(|prev| prev != wgpu::ColorWrites::ALL && (prev != mask || discards))
}

/// Which shader a draw takes: `Ok(true)` gfx's built-in one, `Ok(false)`
/// the provider's, `Err` the reason it is skipped. With a provider the
/// built-in shader only takes draws it renders exactly; without one it
/// stands in for every untextured fixed-function draw it can read (stage
/// states, fog and specular ignored).
fn route(d: &ResolvedDraw, textured: bool, provider: bool) -> Result<bool, String> {
    let fixed = matches!(d.shader.vertex, VertexProcessing::FixedFunction { .. });
    let readable = fixed && Renderer::builtin_supports(&d.layout);
    let builtin_ok = readable
        && if provider {
            builtin_equivalent(d, textured)
        } else {
            !textured && d.shader.pixel_shader.is_none()
        };
    if builtin_ok {
        return Ok(true);
    }
    if provider {
        return Ok(false);
    }
    let what = if fixed {
        "fixed-function draw with textures, a pixel shader, or vertices the built-in shader cannot read"
    } else {
        "vertex program"
    };
    Err(format!("{what}: no shader provider"))
}

/// [`GfxBackend`]'s draw outcomes without a GPU ([`Backend::judge`]).
struct GfxJudge {
    features: wgpu::Features,
    shaders: Option<Box<dyn crate::shader::ShaderJudge>>,
}

impl crate::backend::DrawJudge for GfxJudge {
    fn upload_view(&self, texture: &DecodedTexture) -> bool {
        gfx::program::filterable(texture.host.to_wgpu(), self.features)
    }
    fn surface_view(&self, format: D3DFormat) -> bool {
        gfx::surface::color_surface_format(format)
            .is_some_and(|f| gfx::program::filterable(f, self.features))
    }
    fn draw(&mut self, d: &ResolvedDraw, views: [Option<bool>; 4]) -> DrawOutcome {
        let textured = views.iter().any(Option::is_some);
        match route(d, textured, self.shaders.is_some()) {
            Err(why) => DrawOutcome::Skipped(why),
            Ok(true) => DrawOutcome::Drawn,
            Ok(false) => {
                // As `draw` passes views: a stage with texture info and a
                // texture that has a view.
                let present = std::array::from_fn(|i| {
                    d.texture_info[i].is_some() && views[i] == Some(true)
                });
                let shaders = self.shaders.as_mut().expect("routed to the provider");
                match shaders.check(d, present) {
                    Ok(()) => DrawOutcome::Drawn,
                    Err(e) => DrawOutcome::Skipped(e),
                }
            }
        }
    }
}

/// `draw` once per scissor piece, or alone when there are none.
fn split<'a>(draw: Draw<'a>, pieces: &[Option<ScissorRect>]) -> Vec<Draw<'a>> {
    if pieces.is_empty() {
        return vec![draw];
    }
    pieces
        .iter()
        .map(|&scissor| Draw {
            scissor,
            ..draw.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{merge_discards_from_env, merge_split};
    use wgpu::ColorWrites as W;

    #[test]
    fn the_fast_merge_rule_is_the_default_and_exact_turns_it_off() {
        assert!(merge_discards_from_env(None));
        for v in ["fast", "1", "0", ""] {
            assert!(merge_discards_from_env(Some(v)), "{v:?}");
        }
        assert!(!merge_discards_from_env(Some("exact")));
    }

    #[test]
    fn merged_draws_split_only_after_a_partly_masked_draw() {
        // The first draw of a pending pass, and any draw after a fully
        // written one, stay in the pass, discard or not.
        for discards in [false, true] {
            assert!(!merge_split(None, W::COLOR, discards));
            for mask in [W::ALL, W::COLOR, W::ALPHA] {
                assert!(!merge_split(Some(W::ALL), mask, discards));
            }
        }
        // After a partly masked draw: the same mask stays unless it can
        // discard; any other mask splits.
        assert!(!merge_split(Some(W::COLOR), W::COLOR, false));
        assert!(merge_split(Some(W::COLOR), W::COLOR, true));
        assert!(merge_split(Some(W::COLOR), W::ALL, false));
        assert!(merge_split(Some(W::COLOR), W::ALPHA, false));
        assert!(merge_split(Some(W::empty()), W::ALL, true));
    }
}
