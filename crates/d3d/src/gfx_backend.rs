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
//! and the shader provider's upload rings. A draw to another target encodes
//! the pending pass and submits it with everything encoded before it, as
//! does anything else that touches surfaces (partial clears, surface reads,
//! writes, copies and snapshots, destruction, present), so the GPU sees the
//! same operations in the same order as one pass per draw would give it.
//! (Batching the passes of several targets into one submission was slower
//! in game, 30.2 against 33.5 fps on Old Mombasa at 4x, and was removed;
//! docs/graphics/render-switches.md.) GPU-only
//! side work (the alias mirror's stores and loads, depth-alias and
//! depth-level textures, pixel-counter clears) goes into the open encoder
//! after the pending pass and waits for the next submission
//! ([`GfxBackend::set_defer_side_passes`]). The pending pass is encoded as one render pass, except
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

impl HostTexture {
    /// Another handle to the same texture and view (for a backend on the
    /// same device, `crate::scaled`).
    pub fn shared(&self) -> HostTexture {
        HostTexture {
            texture: self.texture.clone(),
            view: self.view.clone(),
        }
    }
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
    /// [`Self::on_present_replay`]'s hook; `None`: in-between frames go
    /// through `present_hook`.
    replay_hook: Option<PresentHook>,
    pending: Option<PendingPass>,
    stats: BatchStats,
    merge_passes: bool,
    /// [`set_merge_discards`](Self::set_merge_discards).
    merge_discards: bool,
    /// [`set_merge_masks`](Self::set_merge_masks).
    merge_masks: bool,
    /// Made at the first counted draw.
    counter: Option<PixelCounter>,
    /// The depth-bits pass ([`Backend::depth_level_texture`]), made on first use.
    depth_bits: Option<gfx::depth_bits::DepthBits>,
    /// The occlusion grid pass ([`Backend::request_depth_grid`]), made on
    /// first use.
    depth_grid: Option<gfx::depth_grid::DepthGrid>,
    /// Grids on their way back: mapped after the submission that holds
    /// them, handed to their sink at a later present.
    depth_grids: Vec<(gfx::depth_grid::PendingGrid, u64, crate::backend::DepthGridSink)>,
    /// Guest-word passes ([`Backend::depth_alias_texture`]), made on first use.
    guest_words: Option<gfx::guest_words::GuestWords>,
    /// [`set_gpu_depth_aliases`](Self::set_gpu_depth_aliases).
    gpu_depth_aliases: bool,
    /// The depth buffer's words for aliases the GPU pass cannot make: the
    /// surface, the device's depth generation, and the bytes.
    depth_words: Option<(SurfaceId, u64, Vec<u8>)>,
    /// Surface and depth read-backs so far ([`readbacks`](Self::readbacks)).
    pub(crate) readbacks: u64,
    /// The alias mirror ([`Backend::mirror_reserve`]).
    mirror: Option<wgpu::Buffer>,
    reports: HashMap<u64, PixelReport>,
    /// Reports recorded into the open encoder, told their submission when
    /// it is made ([`set_defer_side_passes`](Self::set_defer_side_passes)).
    unsubmitted_reports: Vec<PixelReport>,
    next_ticket: u64,
    /// Presents so far plus one.
    frame: u64,
    /// Backend `draw` calls in this frame so far.
    frame_draw: u64,
    /// The frame whose passes the merge self-check compares.
    merge_check_frame: Option<u64>,
    /// Host pixels per guest pixel of surfaces made at a multiple of their
    /// guest size (a view backend, `crate::scaled`); absent is 1.
    pub(crate) scales: HashMap<SurfaceId, u32>,
    /// Whether counted draws count (off on a view backend: the reference
    /// answers every count).
    pub(crate) count_samples: bool,
    /// Fills of scaled surfaces from guest-size ones, made on first use.
    pub(crate) blitter: Option<gfx::blit::Blitter>,
    /// The interpolated reprojection's warp pass (made on first use).
    warper: Option<gfx::blit::Warper>,
    /// The latest queue submission.
    last_submit: Option<wgpu::SubmissionIndex>,
    /// The latest submission when the previous frame was presented: what a
    /// pipelined `D3DSWAP_FINISH` waits for.
    prev_frame_end: Option<wgpu::SubmissionIndex>,
    /// [`set_swap_finish`](Self::set_swap_finish).
    swap_finish: SwapFinish,
    /// Passes encoded but not submitted yet (a target switch encodes the
    /// pending pass here instead of submitting it), and the render passes
    /// they hold.
    open: Option<(wgpu::CommandEncoder, usize)>,
    /// [`set_defer_side_passes`](Self::set_defer_side_passes).
    defer_side_passes: bool,
    /// The scratch depth a colour-only external pass gets, by size.
    scratch_depth: Option<((u32, u32), wgpu::TextureView)>,
}

/// What `Swap` with `D3DSWAP_FINISH` waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapFinish {
    /// The previous frame's work (the default): the GPU runs this frame
    /// while the render thread encodes the next one, and at most about one
    /// frame is queued ahead of the GPU. Nothing the guest reads depends
    /// on the wait: every read back (surfaces, the alias mirror,
    /// visibility counts) waits for its own copy.
    Pipelined,
    /// Everything submitted, this frame included (`H2_GFX_SWAP_FINISH=full`).
    Full,
}

impl SwapFinish {
    /// `H2_GFX_SWAP_FINISH`: `full` waits for everything; anything else is
    /// [`SwapFinish::Pipelined`].
    pub fn from_env() -> Self {
        match std::env::var("H2_GFX_SWAP_FINISH").as_deref() {
            Ok("full") => SwapFinish::Full,
            _ => SwapFinish::Pipelined,
        }
    }
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
    /// Blocking waits for the GPU at `Swap` (`D3DSWAP_FINISH`).
    pub swap_waits: u64,
}

impl GfxBackend {
    /// Counters of the GPU buffers kept for vertex cache snapshots
    /// ([`crate::vertex_cache`]).
    pub fn persistent_vertex_stats(&self) -> gfx::persistent::PersistentStats {
        self.renderer.persistent_stats()
    }

    /// E3 lighting's one-tap shadow lookup for the draws prepared from now
    /// on (`ShaderProvider::set_hard_shadows`). Only a render-scale view
    /// sets it (`crate::scaled`); the backend the guest reads stays retail.
    pub fn set_hard_shadows(&mut self, on: bool) {
        if let Some(s) = self.shaders.as_mut() {
            s.set_hard_shadows(on);
        }
    }

    /// Where vertex formats are converted for program draws (default
    /// [`gfx::repack::FetchMode::from_env`]: the vertex shader, unless
    /// `H2_VERTEX_FETCH=cpu`). For tests and benches comparing the paths.
    pub fn set_vertex_fetch(&mut self, mode: gfx::repack::FetchMode) {
        self.renderer.set_vertex_fetch(mode);
    }

    /// A backend on `ctx`, translating shaders with [`Nv2aShaders`].
    pub fn new(ctx: GfxContext) -> Self {
        let store = SurfaceStore::new(ctx.caps);
        let mut renderer = Renderer::new(&ctx);
        // `H2_GFX_STAGED_UPLOADS=0`: a queue write per persistent buffer.
        renderer.set_staged_uploads(std::env::var_os("H2_GFX_STAGED_UPLOADS").is_none_or(|v| v != "0"));
        // `H2_GFX_VERTEX_SLABS=0`: a buffer per persistent stream.
        renderer.set_vertex_slabs(std::env::var_os("H2_GFX_VERTEX_SLABS").is_none_or(|v| v != "0"));
        Self {
            ctx,
            store,
            renderer,
            shaders: Some(Box::new(Nv2aShaders::new())),
            presented: None,
            present_hook: None,
            replay_hook: None,
            pending: None,
            stats: BatchStats::default(),
            merge_passes: true,
            merge_discards: merge_discards_from_env(
                std::env::var("H2_GFX_MERGE_PASSES").ok().as_deref(),
            ),
            merge_masks: merge_masks_from_env(std::env::var("H2_GFX_MERGE_PASSES").ok().as_deref()),
            counter: None,
            reports: HashMap::new(),
            unsubmitted_reports: Vec::new(),
            depth_bits: None,
            depth_grid: None,
            depth_grids: Vec::new(),
            guest_words: None,
            gpu_depth_aliases: std::env::var_os("H2_D3D_GPU_DEPTH_ALIAS").is_none_or(|v| v != "0"),
            depth_words: None,
            readbacks: 0,
            mirror: None,
            next_ticket: 0,
            frame: 1,
            frame_draw: 0,
            merge_check_frame: None,
            scales: HashMap::new(),
            count_samples: true,
            blitter: None,
            warper: None,
            last_submit: None,
            prev_frame_end: None,
            swap_finish: SwapFinish::from_env(),
            open: None,
            defer_side_passes: std::env::var_os("H2_GFX_SIDE_SUBMIT").is_none_or(|v| v != "1"),
            scratch_depth: None,
        }
    }

    /// Record the GPU-only side work (the alias mirror's stores and loads,
    /// depth-alias and depth-level textures, pixel-counter clears) into the
    /// encoder kept open across target switches, after the pending pass,
    /// instead of submitting the pending pass and then the work in a
    /// submission of its own (on by default; `H2_GFX_SIDE_SUBMIT=1` turns
    /// it off). The GPU sees the same commands in the same order; they reach
    /// it at the next submission, which anything that reads their results
    /// on the CPU (a read-back, a queued write over the mirror) or replaces
    /// what they write makes first ([`flush`](Self::flush)). Each one saved
    /// two submissions per call (about 0.1 ms each in a level). Off when
    /// another backend on the device samples this one's results
    /// (`crate::scaled`): that backend's submissions would not carry them.
    pub fn set_defer_side_passes(&mut self, on: bool) {
        self.defer_side_passes = on;
    }

    /// What `Swap` with `D3DSWAP_FINISH` waits for (default
    /// [`SwapFinish::from_env`]).
    pub fn set_swap_finish(&mut self, mode: SwapFinish) {
        self.swap_finish = mode;
    }

    /// Textures over the depth buffer's memory made on the GPU (on by
    /// default; `H2_D3D_GPU_DEPTH_ALIAS=0` turns it off): the device hands
    /// them to [`Backend::depth_alias_texture`], which decodes the depth
    /// buffer's `D24S8` words as the texture's format in a compute pass
    /// ([`gfx::guest_words::GuestWords::depth_alias`]) instead of reading
    /// the buffer back, decoding on the CPU and uploading. Formats the pass
    /// does not cover still read back here, once per depth generation. Off:
    /// the device reads back and decodes itself, as before.
    pub fn set_gpu_depth_aliases(&mut self, on: bool) {
        self.gpu_depth_aliases = on;
    }

    /// Surface and depth read-backs so far (`read_surface`, `read_depth`):
    /// each one waits for the GPU.
    pub fn readbacks(&self) -> u64 {
        self.readbacks
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
    /// byte; one pass per draw is the exact reference (docs/graphics/graphics.md,
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
    /// additive draws (docs/graphics/graphics.md, "Merged passes").
    /// `H2_GFX_MERGE_PASSES=exact` turns it off when the backend is made.
    pub fn set_merge_discards(&mut self, on: bool) {
        self.merge_discards = on;
    }

    /// The loose merge rule (the default; `H2_GFX_MERGE_PASSES=fast` or
    /// `exact` turns it off): no draw starts a render pass of its own for
    /// the merge rules, not after a partly masked draw either. Counted
    /// draws and target switches still split. Not exact: on Old Mombasa
    /// it took a 1000-draw frame from about 193 render passes to 41, and
    /// the merge self-check ([`set_merge_check`](Self::set_merge_check))
    /// found 12 of a 970-draw frame's pixels off, by up to 3 levels
    /// (docs/graphics/graphics.md, "Merged passes").
    pub fn set_merge_masks(&mut self, on: bool) {
        self.merge_masks = on;
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

    /// Called instead of the [`Self::on_present`] hook for host-only frames
    /// shown between the guest's presents ([`Backend::present_replay`], the
    /// interpolated replay's and the reprojection's in-between frames,
    /// [`crate::interp`]), with flags 0. Without it they go through the
    /// present hook, as a window wants; a hook that counts the guest's
    /// swaps (the headless run's screenshots) sets this to keep them apart.
    pub fn on_present_replay(
        mut self,
        hook: impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>)
        + Send
        + 'static,
    ) -> Self {
        self.replay_hook = Some(Box::new(hook));
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

    /// Hands every mapped occlusion grid to its sink (no wait: a grid whose
    /// map has not completed waits for a later present), oldest first: the
    /// feature drops the asks older than a delivered one, so a newer grid
    /// handed over first would lose the older.
    fn deliver_depth_grids(&mut self) {
        let mut i = 0;
        while i < self.depth_grids.len() {
            let (g, tag, _) = &self.depth_grids[i];
            if g.failed() {
                self.depth_grids.remove(i);
                continue;
            }
            let tag = *tag;
            match g.try_take() {
                Some(depth) => {
                    let (g, _, sink) = self.depth_grids.remove(i);
                    sink(crate::backend::DepthGridResult {
                        tag,
                        width: g.width,
                        height: g.height,
                        depth,
                    });
                }
                None => i += 1,
            }
        }
    }

    /// Submits the pending pass, if any, and every pass encoded before it.
    pub fn flush(&mut self) -> Result<(), String> {
        let encoded = self.encode_pending();
        self.submit_open();
        encoded
    }

    /// The encoder kept open (a new one if there is none) and its render
    /// passes, after encoding the pending pass into it: for GPU-only work
    /// that may wait for the next submission. Put it back in `self.open`.
    fn take_open(&mut self) -> Result<(wgpu::CommandEncoder, usize), String> {
        self.encode_pending()?;
        Ok(self
            .open
            .take()
            .unwrap_or_else(|| (self.ctx.device.create_command_encoder(&Default::default()), 0)))
    }

    /// Submits the passes encoded so far, if any.
    fn submit_open(&mut self) {
        if let Some((enc, _)) = self.open.take() {
            self.submit(enc);
        }
    }

    /// The encoder for GPU-only side work that must follow everything
    /// recorded so far: the open one (after the pending pass) when side
    /// passes are deferred ([`set_defer_side_passes`](Self::set_defer_side_passes)),
    /// else a new one after submitting everything. Hand it back with
    /// [`side_done`](Self::side_done), also when recording failed.
    fn side_encoder(&mut self) -> Result<(wgpu::CommandEncoder, usize), String> {
        if self.defer_side_passes {
            self.take_open()
        } else {
            self.flush()?;
            Ok((self.ctx.device.create_command_encoder(&Default::default()), 0))
        }
    }

    /// Puts the [`side_encoder`](Self::side_encoder) back to be submitted
    /// with the next submission, or submits it now.
    fn side_done(&mut self, enc: wgpu::CommandEncoder, passes: usize) {
        if self.defer_side_passes {
            // One pass or two; counted so a long run of them is bounded too.
            self.open = Some((enc, passes + 2));
            if passes + 2 >= MAX_ENCODED_PASSES {
                self.submit_open();
            }
        } else {
            self.submit(enc);
        }
    }

    /// Encodes the pending pass, if any, after the passes encoded before it
    /// (without submitting them).
    fn encode_pending(&mut self) -> Result<(), String> {
        let Some(mut p) = self.pending.take() else {
            return Ok(());
        };
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        if self.merge_check_frame == Some(self.frame)
            && p.draws.len() > 1
            && !p.draws.iter().any(RecordedDraw::counts_samples)
        {
            // It copies the attachments with submissions of its own: the
            // passes before must be on the GPU first.
            self.submit_open();
            self.merge_check(&mut p)?;
        }
        let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "flush pass")
            .arg("draws", p.draws.len() as u64);
        let (mut enc, mut passes) = self
            .open
            .take()
            .unwrap_or_else(|| (self.ctx.device.create_command_encoder(&Default::default()), 0));
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
            let n = Renderer::encode_pass(ctx, &mut enc, &target, p.clear, &p.draws, counter)
                .map_err(|e| e.to_string())?;
            self.stats.render_passes += n as u64;
            passes += n;
        }
        self.store
            .finish_target(&mut enc, p.color)
            .map_err(|e| e.to_string())?;
        self.open = Some((enc, passes));
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
        if let Some(copies) = self.renderer.upload_commands(&ctx.device) {
            ctx.queue.submit([copies]);
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
    /// Passes encoded but not submitted yet go first, in the same
    /// submission.
    fn submit(&mut self, enc: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        let open = self.open.take().map(|(e, _)| e.finish());
        self.renderer.flush_uploads(&self.ctx.queue);
        if let Some(s) = self.shaders.as_mut() {
            s.flush_uploads(&self.ctx);
        }
        // Persistent buffers' copies go ahead of everything that draws.
        let copies = self.renderer.upload_commands(&self.ctx.device);
        let index = {
            let _p = timeline::phase::enter(timeline::phase::Phase::Submit);
            timeline::phase::count(timeline::phase::Counter::Submits, 1);
            let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "queue submit");
            self.ctx
                .queue
                .submit(copies.into_iter().chain(open).chain([enc.finish()]))
        };
        for r in self.unsubmitted_reports.drain(..) {
            r.submitted(index.clone());
        }
        self.last_submit = Some(index.clone());
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

    /// Encodes and submits the pending pass if it is for another target,
    /// with everything encoded before it ([`flush`](Self::flush)). Must run
    /// before anything for a draw on `color` and `depth` is staged (the
    /// shader provider's uniforms included): a submit lets every ring reuse
    /// what was staged before it.
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

    /// `Swap`'s work: ends the frame and shows `surface` through the present
    /// hook, from this backend's store or, with `from`, from another
    /// backend's on the same device.
    pub(crate) fn present_from(
        &mut self,
        from: Option<&GfxBackend>,
        surface: SurfaceId,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "present");
        self.flush()?;
        // Occlusion grids recorded before this present are submitted now.
        for (g, _, _) in &mut self.depth_grids {
            g.map();
        }
        self.frame += 1;
        self.frame_draw = 0;
        let new_pipelines = self.renderer.take_new_keys();
        if let Some(s) = self.shaders.as_mut() {
            s.end_frame();
            s.maintain(&self.ctx, &mut self.renderer, &new_pipelines);
        }
        if from.is_none() {
            self.presented = Some(surface);
        }
        if let Some(hook) = self.present_hook.as_mut() {
            match from {
                // Another backend's surface (a view showing its reference's
                // image, `crate::scaled`), on the same device.
                Some(other) => hook(&other.ctx, &other.store, surface, flags, gamma),
                None => hook(&self.ctx, &self.store, surface, flags, gamma),
            }
        }
        let frame_end = self.last_submit.clone();
        if flags & 4 != 0 {
            // D3DSWAP_FINISH: wait for the GPU (for the previous frame's
            // work, unless `SwapFinish::Full`).
            let wait = match self.swap_finish {
                SwapFinish::Full => Some(wgpu::PollType::wait_indefinitely()),
                SwapFinish::Pipelined => {
                    self.prev_frame_end
                        .take()
                        .map(|index| wgpu::PollType::Wait {
                            submission_index: Some(index),
                            timeout: None,
                        })
                }
            };
            if let Some(wait) = wait {
                let _p = timeline::phase::enter(timeline::phase::Phase::Wait);
                let _w = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "GPU wait");
                self.stats.swap_waits += 1;
                let _ = self.ctx.device.poll(wait);
            }
        }
        self.prev_frame_end = frame_end;
        self.deliver_depth_grids();
        // Reports from this frame and before are read later (Halo reads a
        // visibility test two frames after it ends): start their reads now,
        // so the read finds the count mapped instead of waiting.
        for r in self.reports.values().filter(|r| r.is_submitted()) {
            r.try_read(&self.ctx)?;
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
        // A depth surface larger than the colour one draws through the
        // colour's depth-sized stand-in; it must exist to record against
        // (the copy into it is made again when the pass is encoded, so
        // this encoder is dropped).
        let size = |s| {
            self.store
                .get(s)
                .map(|s| (s.desc().width, s.desc().height))
                .map_err(|e| e.to_string())
        };
        if let Some(d) = depth
            && size(d)? != size(color)?
        {
            let mut enc = self.ctx.device.create_command_encoder(&Default::default());
            self.store
                .prepare_target(&self.ctx, &mut enc, color, d)
                .map_err(|e| e.to_string())?;
        }
        let mut recorded = {
            let target = self.store.target(color, depth).map_err(|e| e.to_string())?;
            self.renderer
                .record(&self.ctx, &target, draw)
                .map_err(|e| e.to_string())?
        };
        // With merged passes, a draw that follows a draw writing only some
        // colour channels (unless it writes the same ones) starts a render
        // pass of its own: on Apple GPUs its edge pixels otherwise depend
        // on earlier draws in the pass (docs/graphics/graphics.md, "Merged passes").
        // The fast rule (the default) splits a discarding draw only after a
        // partly masked one; the exact rule splits every discarding draw.
        let prev_mask = self
            .pending
            .as_ref()
            .and_then(|p| p.draws.last())
            .map(RecordedDraw::color_writes);
        let mask = recorded.color_writes();
        let split = if self.merge_masks {
            false
        } else if self.merge_discards {
            merge_split(prev_mask, mask, discards)
        } else {
            discards || merge_split(prev_mask, mask, false)
        };
        let own_pass = first && (!self.merge_passes || split);
        // Why a merged pending pass splits (frame stats `pass_*`).
        if own_pass && self.merge_passes && !recorded.counts_samples() {
            use timeline::phase::{Counter, count};
            match prev_mask {
                Some(prev) if prev != wgpu::ColorWrites::ALL && prev != mask => {
                    count(if prev.is_empty() { Counter::PassNoColor } else { Counter::PassMask }, 1)
                }
                Some(_) => count(Counter::PassDiscard, 1),
                None => {}
            }
        }
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

/// `r` in the pixels of a surface `scale` times its guest size (exact for
/// an integer scale: pieces that tile the guest target tile the host one).
fn scale_rect(r: ScissorRect, scale: u32) -> ScissorRect {
    if scale == 1 {
        return r;
    }
    ScissorRect {
        x: r.x.saturating_mul(scale),
        y: r.y.saturating_mul(scale),
        width: r.width.saturating_mul(scale),
        height: r.height.saturating_mul(scale),
    }
}

/// A level at mirror byte `offset` as the guest-word passes address it.
fn word_layout(layout: GuestLayoutKind, offset: u32) -> Result<gfx::guest_words::WordLayout, String> {
    if !offset.is_multiple_of(4) {
        return Err(format!("mirror offset {offset:#x} is not word-aligned"));
    }
    let pitch_words = match layout {
        GuestLayoutKind::Swizzled => 0,
        GuestLayoutKind::Linear { pitch } if pitch.is_multiple_of(4) && pitch > 0 => pitch / 4,
        GuestLayoutKind::Linear { pitch } => return Err(format!("mirror pitch {pitch} is not word-aligned")),
    };
    Ok(gfx::guest_words::WordLayout { base: offset / 4, pitch_words })
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
        self.scales.remove(&surface);
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
        // like a target switch; so is one that would take the passes not
        // yet submitted past the limit.
        let open = self.open.as_ref().map_or(0, |&(_, n)| n);
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.encoded + open >= MAX_ENCODED_PASSES)
        {
            self.flush()?;
        }
        // A surface made at a multiple of its guest size (a view backend)
        // takes guest-pixel rectangles: scissors scale, geometry does not.
        let scale = self.scales.get(&color).copied().unwrap_or(1);
        if depth.is_some_and(|z| self.scales.get(&z).copied().unwrap_or(1) != scale) {
            return Ok(DrawOutcome::Skipped(
                "colour and depth surfaces at different scales".into(),
            ));
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
        // Vertex cache snapshots keep their GPU buffers across frames.
        let stream_ids: Vec<Option<u64>> = d.streams.iter().map(|s| s.data.cache_id()).collect();
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
            .map(|p| p.map(|r| scale_rect(r, scale)))
            .collect();
        let scissor = Some(scale_rect(
            match d.clip {
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
            },
            scale,
        ));
        // Pre-transformed positions stay in the guest's window.
        let window = (scale > 1).then_some((d.target.width, d.target.height));
        let count_samples = d.hw.zpass_count_enable && self.count_samples;
        let indices = d.indices.as_deref();
        let index_id = d.indices.as_ref().and_then(|i| i.cache_id());
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
                count_samples,
                window,
                stream_ids: &stream_ids,
                index_id,
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
        shaders.set_render_scale(scale);
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
            count_samples,
            vertex_fetch: self.renderer.vertex_fetch(),
            stream_ids: &stream_ids,
            index_id,
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
        let scale = self.scales.get(&color).copied().unwrap_or(1);
        let rects: Vec<ScissorRect> = c
            .rects
            .iter()
            .map(|r| {
                scale_rect(
                    ScissorRect {
                        x: r.x,
                        y: r.y,
                        width: r.width,
                        height: r.height,
                    },
                    scale,
                )
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
        self.readbacks += 1;
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
        let s = self.store.get(surface).map_err(|e| e.to_string())?;
        let texture = s.texture().clone();
        let d = *s.desc();
        if texture.format() != wgpu::TextureFormat::Depth32FloatStencil8 || d.samples > 1 {
            return Ok(None);
        }
        // After the draws into it, in the same submission: the pass writes
        // only buffers and a texture of its own, so nothing else needs it
        // on the GPU sooner (unless side passes are deferred, submitted now).
        let (mut enc, passes) = self.take_open()?;
        let ctx = &self.ctx;
        let bits = self.depth_bits.get_or_insert_with(|| gfx::depth_bits::DepthBits::new(ctx));
        let out = bits.convert(ctx, &mut enc, &texture, (d.width, d.height));
        self.open = Some((enc, passes));
        if !self.defer_side_passes {
            self.submit_open();
        }
        Ok(Some(self.host_texture(out, TextureKind::D2)))
    }

    fn request_depth_grid(
        &mut self,
        surface: SurfaceId,
        request: crate::backend::DepthGridRequest,
        sink: crate::backend::DepthGridSink,
    ) -> Result<bool, String> {
        let s = self.store.get(surface).map_err(|e| e.to_string())?;
        let texture = s.texture().clone();
        let samples = s.desc().samples;
        let readable = matches!(
            texture.format(),
            wgpu::TextureFormat::Depth32FloatStencil8 | wgpu::TextureFormat::Depth32Float
        ) && texture.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING);
        if !readable || samples > 1 || request.width == 0 || request.height == 0 {
            return Ok(false);
        }
        // Too many in flight (nobody presents): drop the request.
        if self.depth_grids.len() >= 8 {
            return Ok(false);
        }
        let Some(rect) = grid_rect(request, (texture.width(), texture.height())) else {
            return Ok(false);
        };
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        // After the draws into it, in the same submission as them.
        let (mut enc, passes) = self.take_open()?;
        let ctx = &self.ctx;
        let grid = self.depth_grid.get_or_insert_with(|| gfx::depth_grid::DepthGrid::new(ctx));
        let pending = grid.reduce(ctx, &mut enc, &texture, rect, request.width, request.height);
        self.open = Some((enc, passes));
        if !self.defer_side_passes {
            self.submit_open();
        }
        self.depth_grids.push((pending, request.tag, sink));
        Ok(true)
    }

    fn supports_external_pass(&self) -> bool {
        true
    }

    fn external_pass(
        &mut self,
        color: SurfaceId,
        depth: SurfaceId,
        sampled: Vec<Option<SurfaceId>>,
        request: crate::backend::ExternalPassRequest,
        mut pass: Box<dyn crate::backend::ExternalPass>,
    ) -> Result<bool, String> {
        // The sampled levels' own views (as `surface_texture` gives them).
        let sampled: Vec<Option<crate::backend::SampledLevel>> = sampled
            .into_iter()
            .map(|s| {
                let s = self.store.get(s?).ok()?;
                let t = s.texture();
                let sampleable = t.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
                    && gfx::program::filterable(t.format(), self.ctx.device.features());
                sampleable.then(|| crate::backend::SampledLevel { view: s.view().clone(), size: (t.width(), t.height()) })
            })
            .collect();
        let attachment = |s: &gfx::surface::Surface| {
            let single = s.desc().samples <= 1;
            let drawable = s.texture().usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT);
            (single && drawable).then(|| {
                (
                    s.texture().create_view(&wgpu::TextureViewDescriptor {
                        base_mip_level: 0,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                    s.wgpu_format(),
                    (s.texture().width(), s.texture().height()),
                )
            })
        };
        let c = self.store.get(color).map_err(|e| e.to_string())?;
        let Some((cv, cf, size)) = attachment(c) else { return Ok(false) };
        let (dv, df) = if depth == color {
            // Colour only: a scratch depth of the colour's size.
            const SCRATCH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
            if self.scratch_depth.as_ref().is_none_or(|(s, _)| *s != size) {
                let t = self.ctx.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("external pass scratch depth"),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: SCRATCH,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                self.scratch_depth = Some((size, t.create_view(&wgpu::TextureViewDescriptor::default())));
            }
            let Some((_, v)) = &self.scratch_depth else { return Ok(false) };
            (v.clone(), SCRATCH)
        } else {
            let d = self.store.get(depth).map_err(|e| e.to_string())?;
            let Some((dv, df, dsize)) = attachment(d) else { return Ok(false) };
            if !df.is_depth_stencil_format() || dsize.0 < size.0 || dsize.1 < size.1 {
                return Ok(false);
            }
            (dv, df)
        };
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        // After the draws recorded so far, in the same submission.
        let (mut enc, passes) = self.take_open()?;
        pass.encode(crate::backend::ExternalTarget {
            device: &self.ctx.device,
            queue: &self.ctx.queue,
            encoder: &mut enc,
            color: &cv,
            color_format: cf,
            depth: &dv,
            depth_format: df,
            viewport: crate::backend::external_viewport(&request, size),
            z_scale: request.z_scale,
            guest_size: request.surface_size,
            size,
            sampled: &sampled,
        });
        self.open = Some((enc, passes + 1));
        Ok(true)
    }

    fn defers_depth_aliases(&self) -> bool {
        // Depth24Plus cannot be read at all (nor by `read_depth`).
        self.gpu_depth_aliases && self.ctx.caps.depth32_stencil8
    }

    #[allow(clippy::too_many_arguments)]
    fn depth_alias_texture(
        &mut self,
        depth: SurfaceId,
        format: D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: &gfx::TextureDesc,
        options: gfx::texture::DecodeOptions,
    ) -> Result<Option<HostTexture>, String> {
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        let s = self.store.get(depth).map_err(|e| e.to_string())?;
        let texture = s.texture().clone();
        let d = *s.desc();
        let word_layout = match desc.format.layout() {
            gfx::format::Layout::Swizzled => Some(0),
            gfx::format::Layout::Linear if desc.pitch.is_multiple_of(4) => Some(desc.pitch / 4),
            _ => None,
        };
        // The pass reads `D24S8` words in rows of the surface's width.
        let gpu = format == D3DFormat::LinD24S8
            && pitch == width * 4
            && (d.width, d.height) == (width, height)
            && d.samples == 1
            && texture.format() == wgpu::TextureFormat::Depth32FloatStencil8
            && offset.is_multiple_of(4)
            && desc.kind == TextureKind::D2
            && desc.levels == 1
            && gfx::guest_words::WordFormat::of(desc.format).is_some();
        if let (true, Some(pitch_words)) = (gpu, word_layout) {
            // After the draws so far: the words as they left them.
            let (mut enc, passes) = self.side_encoder()?;
            let ctx = &self.ctx;
            let words = self.guest_words.get_or_insert_with(|| gfx::guest_words::GuestWords::new(ctx));
            let layout = gfx::guest_words::WordLayout { base: (offset / 4) as u32, pitch_words };
            let flip = gfx::guest_words::sign_flip(options.signed);
            let made = words.depth_alias(ctx, &mut enc, &texture, desc.format, (desc.width, desc.height), layout, flip);
            // A failed pass records nothing; the passes before it stay.
            self.side_done(enc, passes);
            // Out of bounds or an odd swizzle: the CPU path says why.
            if let Ok(out) = made {
                return Ok(Some(self.host_texture(out, TextureKind::D2)));
            }
        }
        // The CPU path, as the device runs it: one read-back per depth
        // generation, decoded per texture.
        if self
            .depth_words
            .as_ref()
            .is_none_or(|(s, g, _)| (*s, *g) != (depth, generation))
        {
            let layout = GuestLayoutKind::Linear { pitch };
            let Some(bytes) = self.read_depth(depth, format, width, height, layout)? else {
                return Err(format!("depth-alias texture: the {format:?} depth buffer cannot be read back"));
            };
            self.depth_words = Some((depth, generation, bytes));
        }
        let bytes = &self.depth_words.as_ref().expect("read above").2;
        let decoded = bytes
            .get(offset..)
            .ok_or_else(|| format!("read_depth returned {} bytes, offset {offset}", bytes.len()))
            .and_then(|b| gfx::texture::decode_texture(desc, b, None, options).map_err(|e| e.to_string()))
            .map_err(|e| format!("depth-alias texture: {e}"))?;
        self.upload_texture(&decoded).map(Some)
    }

    fn can_mirror(&self) -> bool {
        true
    }

    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        self.flush()?;
        self.mirror = Some(gfx::churn::create_buffer(&self.ctx.device, &wgpu::BufferDescriptor {
            label: Some("d3d alias mirror"),
            size: (len.max(4) as u64).next_multiple_of(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        Ok(())
    }

    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        // After everything recorded before it (`write_buffer` runs ahead
        // of the next submission only).
        self.flush()?;
        let m = self.mirror.as_ref().ok_or("mirror_write before mirror_reserve")?;
        if !offset.is_multiple_of(4) || !bytes.len().is_multiple_of(4) || offset as u64 + bytes.len() as u64 > m.size() {
            return Err(format!("mirror_write of {} bytes at {offset:#x} into {}", bytes.len(), m.size()));
        }
        if !bytes.is_empty() {
            gfx::churn::queue_write();
            self.ctx.queue.write_buffer(m, offset as u64, bytes);
        }
        Ok(())
    }

    fn mirror_store(
        &mut self,
        surface: SurfaceId,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        let m = self.mirror.clone().ok_or("mirror_store before mirror_reserve")?;
        let s = self.store.get(surface).map_err(|e| e.to_string())?;
        let (texture, kind) = (s.texture().clone(), s.kind());
        let layout = word_layout(layout, offset)?;
        let (mut enc, passes) = self.side_encoder()?;
        let ctx = &self.ctx;
        let words = self.guest_words.get_or_insert_with(|| gfx::guest_words::GuestWords::new(ctx));
        let stored = match kind {
            SurfaceKind::Color => words.store_color(ctx, &mut enc, &texture, format, (width, height), &m, layout),
            SurfaceKind::DepthStencil => words.store_depth(ctx, &mut enc, &texture, (width, height), &m, layout),
        };
        self.side_done(enc, passes);
        stored.map_err(|e| format!("mirror_store: {e}"))
    }

    fn mirror_load(
        &mut self,
        surface: SurfaceId,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        let _p = timeline::phase::enter(timeline::phase::Phase::Encode);
        let m = self.mirror.clone().ok_or("mirror_load before mirror_reserve")?;
        let s = self.store.get(surface).map_err(|e| e.to_string())?;
        let (texture, kind) = (s.texture().clone(), s.kind());
        let layout = word_layout(layout, offset)?;
        let (mut enc, passes) = self.side_encoder()?;
        let ctx = &self.ctx;
        let words = self.guest_words.get_or_insert_with(|| gfx::guest_words::GuestWords::new(ctx));
        let loaded = match kind {
            SurfaceKind::Color => words.load_color(ctx, &mut enc, &texture, format, (width, height), &m, layout),
            SurfaceKind::DepthStencil => words.load_depth(ctx, &mut enc, &texture, (width, height), &m, layout),
        };
        self.side_done(enc, passes);
        loaded.map_err(|e| format!("mirror_load: {e}"))
    }

    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.flush()?;
        self.readbacks += 1;
        let _p = timeline::phase::enter(timeline::phase::Phase::Wait);
        timeline::phase::count(timeline::phase::Counter::Readbacks, 1);
        let m = self.mirror.as_ref().ok_or("mirror_read before mirror_reserve")?;
        if !offset.is_multiple_of(4) || !len.is_multiple_of(4) || offset as u64 + len as u64 > m.size() {
            return Err(format!("mirror_read of {len} bytes at {offset:#x} from {}", m.size()));
        }
        if len == 0 {
            return Ok(Vec::new());
        }
        let staging = gfx::churn::create_buffer(&self.ctx.device, &wgpu::BufferDescriptor {
            label: Some("d3d alias mirror read"),
            size: len as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(m, offset as u64, &staging, 0, len as u64);
        self.submit(enc);
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map mirror read"));
        self.ctx
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        let out = slice.get_mapped_range().map_err(|e| format!("{e:?}"))?.to_vec();
        Ok(out)
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
        self.readbacks += 1;
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
        self.present_from(None, surface, flags, gamma)
    }

    fn warp_surface(
        &mut self,
        src: SurfaceId,
        dst: SurfaceId,
        h: [[f32; 3]; 3],
    ) -> Result<bool, String> {
        self.flush()?;
        let s = self.store.get(src).map_err(|e| e.to_string())?;
        let d = self.store.get(dst).map_err(|e| e.to_string())?;
        if d.desc().samples > 1 {
            // A multisampled target would load its stale samples over
            // the warp at the next pass.
            return Ok(false);
        }
        let (src_view, dst_view, format) = (s.view().clone(), d.view().clone(), d.wgpu_format());
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        self.store
            .resolve_msaa(&mut enc, src)
            .map_err(|e| e.to_string())?;
        let ctx = &self.ctx;
        self.warper
            .get_or_insert_with(|| gfx::blit::Warper::new(ctx))
            .warp(ctx, &mut enc, &src_view, &dst_view, format, h);
        self.submit(enc);
        Ok(true)
    }

    fn surface_extent(&self, surface: SurfaceId) -> Option<(u32, u32)> {
        let d = *self.store.get(surface).ok()?.desc();
        Some((d.width, d.height))
    }

    fn gpu_fence(&mut self) -> Option<Box<dyn Fn() -> bool + Send>> {
        if self.flush().is_err() {
            return None;
        }
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let set = std::sync::Arc::clone(&done);
        self.ctx.queue.on_submitted_work_done(move || {
            set.store(true, std::sync::atomic::Ordering::Release)
        });
        let device = self.ctx.device.clone();
        Some(Box::new(move || {
            if !done.load(std::sync::atomic::Ordering::Acquire) {
                // Runs finished callbacks; never waits.
                let _ = device.poll(wgpu::PollType::Poll);
            }
            done.load(std::sync::atomic::Ordering::Acquire)
        }))
    }

    fn present_replay(
        &mut self,
        surface: SurfaceId,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        // A host-only frame (crate::interp): submitted and shown through
        // the hook; the frame count, shader upkeep, `presented` and the
        // swap wait belong to the guest's presents.
        self.flush()?;
        if let Some(hook) = self.replay_hook.as_mut().or(self.present_hook.as_mut()) {
            hook(&self.ctx, &self.store, surface, 0, gamma);
        }
        Ok(())
    }

    fn clear_pixel_count(&mut self) -> Result<(), String> {
        // After the counted draws before it, in the same submission (the
        // report that reads the counter submits; unless side passes are
        // deferred, submitted now).
        let (mut enc, passes) = self.take_open()?;
        let ctx = &self.ctx;
        let counter = self.counter.get_or_insert_with(|| PixelCounter::new(ctx));
        counter.clear(&mut enc);
        self.open = Some((enc, passes));
        if !self.defer_side_passes {
            self.submit_open();
        }
        Ok(())
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        let report = if self.defer_side_passes {
            // After the counted draws, in the open encoder: it reaches the
            // GPU with the next submission (a target switch, a read or the
            // present), which tells it its index. Until then it reads as not
            // back, and a waiting read submits it first.
            let (mut enc, passes) = self.take_open()?;
            let ctx = &self.ctx;
            let counter = self.counter.get_or_insert_with(|| PixelCounter::new(ctx));
            let report = counter.report(ctx, &mut enc);
            self.open = Some((enc, passes));
            self.unsubmitted_reports.push(report.clone());
            report
        } else {
            // The counted draws before it are in the total.
            self.flush()?;
            let ctx = &self.ctx;
            let counter = self.counter.get_or_insert_with(|| PixelCounter::new(ctx));
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            let report = counter.report(ctx, &mut enc);
            // The read waits for this submission only.
            report.submitted(self.submit(enc));
            report
        };
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
        if !report.is_submitted() {
            self.flush()?;
        }
        report.read(&self.ctx)
    }

    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        let Some(report) = self.reports.get(&ticket) else {
            return Err(format!("pixel-count ticket {ticket} is not live"));
        };
        if !report.is_submitted() {
            // Still in the open encoder: on its way once submitted.
            self.flush()?;
        }
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

    fn pixel_count_unsubmitted(&self, ticket: u64) -> bool {
        self.reports.get(&ticket).is_some_and(|r| !r.is_submitted())
    }
}

/// The fast merge rule ([`GfxBackend::set_merge_discards`]) for a value
/// of `H2_GFX_MERGE_PASSES`: on unless it is `exact`. (`0`, one render
/// pass per draw, is applied by the embedder through
/// [`GfxBackend::set_merge_passes`] and needs no rule.)
fn merge_discards_from_env(value: Option<&str>) -> bool {
    value != Some("exact")
}

/// The loose merge rule ([`GfxBackend::set_merge_masks`]) for a value of
/// `H2_GFX_MERGE_PASSES`: on unless it is `fast` or `exact`.
fn merge_masks_from_env(value: Option<&str>) -> bool {
    !matches!(value, Some("fast" | "exact"))
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

/// A grid request's rectangle in the texels of a `texels`-sized texture
/// (scaled from the guest size), clamped to it; `None` when empty.
pub(crate) fn grid_rect(
    request: crate::backend::DepthGridRequest,
    texels: (u32, u32),
) -> Option<gfx::depth_grid::Rect> {
    let (gw, gh) = request.surface_size;
    if gw == 0 || gh == 0 {
        return None;
    }
    let (x, y, w, h) = request.rect;
    let sx = |v: u32| (v as u64 * texels.0 as u64 / gw as u64).min(texels.0 as u64) as u32;
    let sy = |v: u32| (v as u64 * texels.1 as u64 / gh as u64).min(texels.1 as u64) as u32;
    let (x0, y0) = (sx(x), sy(y));
    let (x1, y1) = (sx(x.saturating_add(w)), sy(y.saturating_add(h)));
    (x1 > x0 && y1 > y0).then_some(gfx::depth_grid::Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

#[cfg(test)]
mod tests {
    use super::{grid_rect, merge_discards_from_env, merge_masks_from_env, merge_split};
    use wgpu::ColorWrites as W;

    #[test]
    fn a_grid_rect_scales_from_guest_pixels_to_texels() {
        let req = |rect| crate::backend::DepthGridRequest {
            tag: 0,
            rect,
            surface_size: (640, 480),
            width: 64,
            height: 32,
        };
        let r = |x, y, width, height| gfx::depth_grid::Rect { x, y, width, height };
        assert_eq!(grid_rect(req((0, 0, 640, 480)), (640, 480)), Some(r(0, 0, 640, 480)));
        // A 2x view: the same viewport, twice the texels.
        assert_eq!(grid_rect(req((0, 0, 640, 480)), (1280, 960)), Some(r(0, 0, 1280, 960)));
        // A split-screen half.
        assert_eq!(grid_rect(req((0, 240, 640, 240)), (1280, 960)), Some(r(0, 480, 1280, 480)));
        // Clamped to the texture; empty is nothing.
        assert_eq!(grid_rect(req((600, 0, 100, 480)), (640, 480)), Some(r(600, 0, 40, 480)));
        assert_eq!(grid_rect(req((0, 0, 0, 480)), (640, 480)), None);
        let mut z = req((0, 0, 640, 480));
        z.surface_size = (0, 0);
        assert_eq!(grid_rect(z, (640, 480)), None);
    }

    #[test]
    fn the_fast_merge_rule_is_the_default_and_exact_turns_it_off() {
        assert!(merge_discards_from_env(None));
        for v in ["fast", "1", "0", ""] {
            assert!(merge_discards_from_env(Some(v)), "{v:?}");
        }
        assert!(!merge_discards_from_env(Some("exact")));
    }

    #[test]
    fn the_loose_merge_rule_is_the_default_and_fast_or_exact_turn_it_off() {
        assert!(merge_masks_from_env(None));
        for v in ["loose", "1", ""] {
            assert!(merge_masks_from_env(Some(v)), "{v:?}");
        }
        assert!(!merge_masks_from_env(Some("fast")));
        assert!(!merge_masks_from_env(Some("exact")));
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
