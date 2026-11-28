//! The host side of the device: where surfaces live and draws go.
//!
//! The device resolves every guest-visible thing (state arrays, resource
//! headers, vertex bytes, shader inputs) into a [`ResolvedDraw`] and hands it
//! to a [`Backend`]. `GfxBackend` renders
//! through `gfx`; [`RecordingBackend`] records calls for CPU-only tests.

use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::vertex::{PrimitiveType, VertexLayout};
use gfx::{Clear, D3DFormat};

use gfx::texture::TextureKind;

pub use gfx::external::{
    CachedBytes, CachedIndices, IndexData, PixelRect, StreamBytes, StreamData, TextureInfo, UiFit,
};
#[cfg(feature = "gpu")]
pub use gfx::external::{ExternalAttachments, ExternalColor, SampledLevel};

use crate::shader::ShaderInputs;
use crate::state::{DrawState, StageExtras, StateWords};

/// The render target a draw goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Colour format (as the guest header describes it).
    pub format: D3DFormat,
    /// Depth/stencil format, if a depth buffer is bound.
    pub depth_format: Option<D3DFormat>,
}

/// Host allocation sizes for a draw's attachments and texture stages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderExtents {
    pub color: Option<(u32, u32)>,
    pub depth: Option<(u32, u32)>,
    pub textures: [Option<(u32, u32)>; 4],
}

/// A display surface's extent and its tightly packed BGRA8 pixels.
pub type DisplaySurfacePixels = ((u32, u32), Vec<u8>);

/// Geometry in the vertex window selected by one complete part.
pub struct IndexedStreams {
    pub layout: VertexLayout,
    pub vertex_count: u32,
    pub streams: Vec<StreamBytes>,
    pub indices: IndexData,
}

/// Remaining dispatches on the caller's stack. An indexed scene batch
/// consumes them only with a synchronous, complete GPU reservation.
#[derive(Clone, Copy)]
pub struct IndexedTail {
    pub indexed_esp: u32,
    pub counter: u32,
    pub remaining: u32,
}

impl IndexedTail {
    pub(crate) fn consume(self, mem: &mut dyn crate::GuestMemory, esp: Option<u32>) -> bool {
        esp == Some(self.indexed_esp) && self.remaining > 0
            && mem.read_u32(self.counter).ok() == Some(self.remaining)
            && (self.remaining == 1 || mem.try_write(self.counter, &1u32.to_le_bytes()))
    }
}

/// A host render pass drawn into the guest's own colour target and depth,
/// in order with the draws (docs/graphics/native-renderer.md): the native
/// world. The device decides where (the bound targets and viewport); the
/// backend gives the pass its encoder and the targets' views.
pub trait ExternalPass: Send {
    /// The producer, even for untyped passes without an `ExternalPlan`.
    fn diagnostic_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Produce counted packets for a replacement of a z-pass query draw.
    /// Every active copy prepares the count pipeline before any can commit.
    fn counts_samples(&self) -> bool { false }

    /// Supplemental geometry seen only through the display lens. The game
    /// has no draw for it. Only display attachments may receive a recording.
    fn display_only(&self) -> bool { false }

    /// The scene may retain this pass after presentation and record it again
    /// on other attachments of the same device. Its selections, view and
    /// material inputs must stay valid for the lifetime of the pass. It
    /// must produce packets or encoded commands using only the attachments
    /// and uploads supplied to that recording, with no guest feedback.
    fn replays_scene(&self) -> bool { false }

    /// This pass for the draws it stands in for, then `next`'s: a kept
    /// scene joins adjacent draws of one state into a single pass
    /// (`crate::scene_sink`). `next` follows this pass's last packet
    /// directly, on the same attachments. `None` (the default): the two
    /// cannot be one pass and each records on its own. Needs
    /// [`Self::as_any`] on both.
    fn run_with(&self, _next: &dyn ExternalPass) -> Option<Box<dyn ExternalPass>> { None }

    /// The pass as its concrete type, for [`Self::run_with`] to read `next`.
    fn as_any(&self) -> Option<&dyn std::any::Any> { None }

    /// This queued native producer pass, then `next`'s, as one new pass
    /// that later adjacent draws can join ([`Self::extend_run`]): a
    /// backend that queues draws ([`Backend::queue_external_draw`]) admits
    /// a run of particles as one queued pass this way, at admission time,
    /// not only in a replay. `next` follows this pass's last packet
    /// directly, on the same attachments, textures and request. The two
    /// passes are left as they are. `None` (the default): each is queued
    /// on its own.
    fn queued_run(&self, _next: &dyn ExternalPass) -> Option<Box<dyn ExternalPass>> { None }

    /// Appends `next`'s draws after this run's ([`Self::queued_run`]'s
    /// pass). `false` (the default): `next` cannot join; this run is left
    /// as it was.
    fn extend_run(&mut self, _next: &dyn ExternalPass) -> bool { false }

    /// Selected world ranges that bind a private page whose two lamp states differ.
    fn relit_lightmap_ranges(&self) -> usize { 0 }

    /// The one lighting mode whose frames draw this pass, `None` for every
    /// mode (docs/graphics/e3-lighting.md, "Lights: which, and how many").
    /// Such a pass is a scene stage that only the frame graph records,
    /// and only in that mode (`native_render::frame_graph`): the render
    /// thread's live recording, on the reference copy, takes it and draws
    /// nothing ([`Backend::run_scene_stage`]).
    fn lighting_only(&self) -> Option<crate::Lighting> { None }

    /// The light, by handle, whose cone shadow this pass's layers read
    /// from the destination alpha, drawn by the renderer
    /// (docs/graphics/e3-lighting-cones.md, "The shadowed cones"): the
    /// frame graph draws the cone's tile and its receivers' term just
    /// before the pass. `None` for a pass that reads no such term.
    fn cone_shadow(&self) -> Option<u32> { None }

    fn indexed_tail(&self) -> Option<IndexedTail> { None }

    /// All these ranges share the textures resolved at the first draw.
    /// Refuse if a later original draw could observe an earlier write.
    fn requires_independent_samples(&self) -> bool { false }

    #[cfg(feature = "gpu")]
    /// Records the pass once per active target copy on the GPU thread.
    /// Single-copy scaling records the display. Exact scaling records both
    /// the 1x reference and the display with the same frame inputs.
    fn encode(&mut self, target: ExternalTarget<'_>) { self.record_ranges(target); }

    /// The guest is deciding whether to skip the work this pass replaces.
    /// Report the encoder's decision to that caller even on a threaded backend.
    fn confirm_issue(&self) -> bool {
        false
    }

    /// Validate stage-specific resources on this copy before recording.
    #[cfg(feature = "gpu")]
    fn prepare_copy(&mut self, _target: &ExternalTarget<'_>) -> Result<(), &'static str> {
        Ok(())
    }

    /// Record the complete replacement in an isolated encoder. Return the number
    /// of guest ranges actually recorded. Legacy, uncounted passes fail closed.
    #[cfg(feature = "gpu")]
    fn record_ranges(&mut self, _target: ExternalTarget<'_>) -> usize {
        0
    }

    /// Prepare draws with cached pipelines and bindings for an ordered
    /// attachment run. They can be appended without a private encoder.
    #[cfg(feature = "gpu")]
    fn record_draws(
        &mut self,
        _target: &ExternalAttachments,
        _request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> { None }

    /// Commands for the frame's open encoder, issued at commit after the
    /// pending pass: for a stage that draws into targets of its own or
    /// copies between them (post, water's view copy), so it needs no
    /// command buffer of its own. Tried after [`ExternalPass::record_draws`];
    /// `None` falls back to [`ExternalPass::record_ranges`].
    #[cfg(feature = "gpu")]
    fn record_encoded(
        &mut self,
        _target: &ExternalAttachments,
        _request: ExternalPassRequest,
    ) -> Option<Result<ExternalEncoded, &'static str>> { None }

    /// The pass validates cached GPU resources when creating them and
    /// checks attachment compatibility, usage and draw bounds explicitly.
    fn uses_validated_resources(&self) -> bool { false }

    /// A run whose immutable attachments can be pinned once. Opt in only
    /// when prepare/record can run off the render worker on the pinned copies.
    fn batch_key(&self) -> Option<u64> { None }

    /// Fully recorded bundles for an ordered run on the bound attachments.
    /// The per-copy reservation checks their range count before commit.
    /// Bundle uniforms use the pinned upload arena, leaving the encoder untouched.
    #[cfg(feature = "gpu")]
    fn record_bundles(&mut self, _target: &mut ExternalTarget<'_>) -> Option<ExternalBundles> { None }

    /// A resolved draw admission can retain its original, immutable D3D packet for
    /// ordered fallback on the worker. Guest-mask reservations leave this empty.
    /// For a native producer (`Device::try_native_*_draw`) the packet stands in
    /// for the guest's own emission, which is skipped once the draw is queued, so
    /// it must be the whole draw the guest would issue.
    fn resolved_fallback(&self) -> Option<std::sync::Arc<ResolvedDraw>> { None }

    /// The packets the pass stands in for, in the order the guest issues
    /// them, for a scene that retains the frame ([`RetainedAdmission`]):
    /// the replay keeps one draw for each, all under the one admission.
    /// By default the [`Self::resolved_fallback`] packet alone. A pass that
    /// replaces several guest draws (a UI batch with sibling ranges) has no
    /// single fallback but names each of them here; empty when none is
    /// exact.
    fn resolved_packets(&self) -> Vec<std::sync::Arc<ResolvedDraw>> {
        self.resolved_fallback().into_iter().collect()
    }

    /// The same pass drawing `draw` in place of its own (the draw
    /// [`Self::resolved_fallback`] gives, with other constants): a display
    /// frame between ticks moves a typed draw onto a blended camera this
    /// way (`crate::scene_sink::Retarget`). `None` (the default): the pass
    /// draws only what it was built with, and that frame draws it as the
    /// tick did.
    fn with_draw(&self, _draw: std::sync::Arc<ResolvedDraw>) -> Option<Box<dyn ExternalPass>> { None }

    /// An immutable packet for render diagnostics, independent of whether
    /// this pass can replay that packet as a fallback.
    fn diagnostic_draw(&self) -> Option<std::sync::Arc<ResolvedDraw>> {
        self.resolved_fallback()
    }

    /// Number of suppressed guest items represented by this reservation. Most
    /// geometry promises one range per item; multi-range effects override it.
    fn items(&self, ranges: usize) -> usize { ranges }

    /// Potential guest-visible writes. Read-only probes use the default.
    fn writes(&self) -> ExternalWrites {
        ExternalWrites::default()
    }


    /// The device's vertex constants `c[0..VIEW_CONSTANTS]` (hardware
    /// numbering) when the pass is issued, after the drain: the view's
    /// own (view-projection, viewport fix-up), for a pass that runs the
    /// game's vertex programs. Called once, on the device's thread, before
    /// the backend gets the pass.
    fn view_constants(&mut self, _constants: &[[f32; 4]; VIEW_CONSTANTS]) {}

    /// The W-buffer's clip as a W-buffered draw issued now would take it
    /// ([`DepthClip`]), for a pass that runs the game's vertex programs.
    /// Called once, on the device's thread, with
    /// [`ExternalPass::view_constants`].
    fn depth_clip(&mut self, _clip: DepthClip) {}

    /// Whether the pass draws colour only (a full-screen post pass): the
    /// device then issues it with no depth bound, and the backend gives it
    /// a scratch depth (`ExternalTarget::depth`) it must not rely on.
    fn colour_only(&self) -> bool {
        false
    }

    /// Guest texture headers the pass samples, read when it is issued: the
    /// render-to-texture levels the D3D path drew into them (the game's
    /// texture accumulation target), given to `ExternalPass::encode` as
    /// `ExternalTarget::sampled` in this order. Empty by default.
    fn sampled(&self) -> Vec<u32> {
        Vec::new()
    }
}

/// The W-buffer's clip of a W-buffered vertex-program draw, as the D3D path
/// draws it (`nv2a::w_clip_range`, `D3DRS_DEPTHCLIPCONTROL`): the vertex's
/// w held to `range` (depth units, `[SET_CLIP_MIN, SET_CLIP_MAX]`, or the
/// whole range when that is empty), and with `clamp` out-of-range depth
/// clamped to it rather than culled. The game sets both per view
/// (`0x00015720`); a cinematic's range reaches past its camera's far.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthClip {
    pub range: [f32; 2],
    pub clamp: bool,
    /// Whether draws are W-buffered (`SET_CONTROL0` z-perspective,
    /// `nv2a::CONTROL0_Z_PERSPECTIVE`); `None` before any `SET_CONTROL0`.
    /// Not W-buffered, the D3D path stores z, not the w the native
    /// programs write, and clips z, not w.
    pub w_buffered: Option<bool>,
}

pub use gfx::external::VIEW_CONSTANTS;

#[cfg(feature = "gpu")]
/// What an [`ExternalPass`] draws into.
pub struct ExternalTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub color: &'a wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    pub depth: &'a wgpu::TextureView,
    pub depth_format: wgpu::TextureFormat,
    /// The viewport `(x, y, width, height)` in the targets' texels.
    pub viewport: [f32; 4],
    /// The depth format's full scale as the guest's W-buffer uses it
    /// (device `+0x948`).
    pub z_scale: f32,
    pub display_projection_factor: f32,
    /// The colour target's size in guest pixels (what the game's viewport
    /// constants are in) and in this copy's texels (larger under the
    /// render scale).
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
    /// Per texture header of [`ExternalPass::sampled`], in order, a view of
    /// the render-to-texture level that holds its texels on the host (this
    /// copy's: larger under the render scale), or `None` when there is
    /// none to sample in place.
    pub sampled: &'a [Option<SampledLevel>],
    /// Indexed live colour surfaces in the same order as the plan's slots.
    pub slots: &'a [Option<ExternalColor>],
    pub uploads: &'a gfx::ring::SubmissionUploads,
    /// Lightmap arm permitted for this physical target. An isolated
    /// display copy or a private capture may select reconstructed pages.
    pub lighting: crate::Lighting,
    pub copy: TargetCopy,
    pub stages: [Option<&'a wgpu::TextureView>; 4],
    /// An initially empty list for [`ExternalPass::record_ranges`] on an
    /// isolated encoder. Callbacks run just before submission and are
    /// discarded with a refused or cancelled recording.
    pub on_submit: Option<&'a mut Vec<Box<dyn FnOnce() + Send>>>,
}

#[cfg(feature = "gpu")]
impl ExternalTarget<'_> {
    pub fn projection_factor(&self) -> f32 {
        if self.copy == TargetCopy::Display { self.display_projection_factor } else { 1.0 }
    }

    pub fn view_constants(&self, mut c: [[f32; 4]; VIEW_CONSTANTS]) -> [[f32; 4]; VIEW_CONSTANTS] {
        crate::device::widen_projection_rows(&mut c[..4], self.projection_factor());
        c
    }
}


#[cfg(feature = "gpu")]
impl ExternalTarget<'_> {
    pub fn encoder_uploads(&mut self) -> gfx::ring::EncoderUploads<'_> {
        gfx::ring::EncoderUploads { device: self.device, queue: self.queue, arena: self.uploads, encoder: self.encoder }
    }
}

/// Bound colour and depth/stencil writes. RTT mirrors and aliases follow these writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExternalWrites {
    pub color: bool,
    pub depth_stencil: bool,
}

/// Defined in `gfx` (docs/graphics/framescene.md, section 4.1).
pub use gfx::external::TargetCopy;

/// A guest colour surface required for intermediate draws within one replacement.
#[derive(Clone, Copy, Debug)]
pub struct ExternalSlot {
    pub header: u32,
    pub written: bool,
}

#[derive(Clone, Debug)]
pub struct ExternalPlan {
    pub stage: timeline::native::Stage,
    pub view: u32,
    pub ranges: usize,
    pub slots: Vec<Option<ExternalSlot>>,
}

impl ExternalPlan {
    pub(crate) fn complete(&self, copy: TargetCopy, actual: usize) -> bool {
        if self.ranges != 0 && self.ranges == actual { return true; }
        self.stage.invariant(self.view, copy == TargetCopy::Display);
        static LOGGED: std::sync::Mutex<Vec<(timeline::native::Stage, u32, TargetCopy)>> = std::sync::Mutex::new(Vec::new());
        let mut seen = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
        let key = (self.stage, self.view.min(4), copy);
        if !seen.contains(&key) {
            seen.push(key);
            eprintln!("native {:?} view {} {copy:?}: recorded {actual} of {} ranges", self.stage, self.view, self.ranges);
        }
        false
    }
}

/// Resolved device handles. Every sampled level and populated slot is required.
#[derive(Clone)]
pub struct ExternalSurfaces<S> {
    pub color: S,
    pub depth: S,
    pub sampled: Vec<S>,
    pub slots: Vec<Option<S>>,
    pub slot_sizes: Vec<Option<(u32, u32)>>,
}

impl<S> ExternalSurfaces<S> {
    pub fn try_map<T>(&self, mut f: impl FnMut(&S) -> Option<T>) -> Option<ExternalSurfaces<T>> {
        Some(ExternalSurfaces {
            slot_sizes: self.slot_sizes.clone(),
            color: f(&self.color)?,
            depth: f(&self.depth)?,
            sampled: self.sampled.iter().map(&mut f).collect::<Option<_>>()?,
            slots: self.slots.iter().map(|s| match s {
                Some(s) => Some(Some(f(s)?)),
                None => Some(None),
            }).collect::<Option<_>>()?,
        })
    }
}

/// All pinned targets, or the physical copy whose attachments could not be resolved.
#[cfg(feature = "gpu")]
pub enum ExternalCopies<T = Vec<ExternalAttachments>> {
    Ready(T),
    Missing(TargetCopy),
}

/// Sealed commands for a single physical target. Only complete recordings can be committed.
pub struct ExternalRecording {
    pub stage: timeline::native::Stage,
    pub view: u32,
    pub items: usize,
    /// The active scaled policy requires just the display recording.
    pub(crate) single_display: bool,
    pub copy: TargetCopy,
    pub ranges: usize,
    #[cfg(feature = "gpu")]
    pub commands: ExternalCommands,
    #[cfg(feature = "gpu")]
    pub uploads: gfx::ring::SubmissionUploads,
    pub on_submit: Vec<Box<dyn FnOnce() + Send>>,
}

#[cfg(feature = "gpu")]
pub struct ExternalBundles {
    pub run: u64,
    pub scissor: [u32; 4],
    pub groups: Vec<(wgpu::Color, wgpu::RenderBundle)>,
    pub ranges: usize,
}

#[cfg(feature = "gpu")]
impl ExternalBundles {
    /// Use identical attachments and dynamic state for validation and final submission.
    pub fn encode_run(encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView,
        depth: &wgpu::TextureView, stencil: bool, packets: &[Self]) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native draw run"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: stencil.then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
            }),
            ..Default::default()
        });
        for draws in packets {
            let [x, y, width, height] = draws.scissor;
            pass.set_scissor_rect(x, y, width, height);
            for (blend, bundle) in &draws.groups {
                pass.set_blend_constant(*blend);
                pass.execute_bundles(std::iter::once(bundle));
            }
        }
    }
}

/// Defined in `gfx` (docs/graphics/framescene.md, section 4.1).
#[cfg(feature = "gpu")]
pub use gfx::external::{ExternalDraws, ExternalEncoded};

#[cfg(feature = "gpu")]
pub enum ExternalCommands {
    Standalone(wgpu::CommandBuffer),
    /// Recorded into the open encoder at commit, after the pending pass.
    Encoded(ExternalEncoded),
    Draws {
        targets: (gfx::surface::SurfaceId, gfx::surface::SurfaceId),
        draws: ExternalDraws,
    },
    Bundles {
        targets: (gfx::surface::SurfaceId, gfx::surface::SurfaceId),
        color: wgpu::TextureView,
        depth: wgpu::TextureView,
        stencil: bool,
        draws: ExternalBundles,
    },
}

pub type SharedReplacement = std::sync::Arc<std::sync::Mutex<Box<dyn ExternalPass>>>;

/// A resolved external draw whose guest-visible writes agree with its D3D fallback.
/// The worker chooses one complete recording or replays the original in this queue slot.
pub struct QueuedExternalDraw<S> {
    pub surfaces: ExternalSurfaces<S>,
    pub request: ExternalPassRequest,
    pub plan: ExternalPlan,
    pub pass: SharedReplacement,
    pub fallback: std::sync::Arc<ResolvedDraw>,
    /// The original draw's depth. A colour-only native pass has scratch depth.
    pub fallback_depth: Option<S>,
    /// None on native commit, otherwise the result of the fallback draw.
    pub finished: Box<dyn FnOnce(Option<&Result<DrawOutcome, String>>) + Send>,
}

/// The typed admission a guest draw was offered as and did not take: the
/// exact packets the pass would replace, and the pass, with the attachments it
/// resolved. A backend that retains the frame keeps it beside the draw
/// ([`Backend::draw_admitted`]) so replay can prepare the pass on its own
/// attachments; any other backend drops it.
pub struct RetainedAdmission<S> {
    /// The guest's draws the pass replaces, in issue order
    /// ([`ExternalPass::resolved_packets`]): one for a guest draw's own
    /// admission, a native producer's head and siblings for a batch.
    pub packets: std::sync::Arc<[std::sync::Arc<ResolvedDraw>]>,
    pub surfaces: ExternalSurfaces<S>,
    pub request: ExternalPassRequest,
    pub plan: ExternalPlan,
    pub pass: SharedReplacement,
}

/// A typed stage of the frame step's scene (docs/graphics/framescene.md,
/// slice 2): one command in the render thread's queue, in order with the
/// draws around it. The game thread has already decided, from the scene
/// and the guest, every item the stage draws and told the guest to skip
/// them; the render thread prepares and records the stage when it reaches
/// the command ([`Backend::run_scene_stage`]). What it cannot record is a
/// named error ([`crate::scene_stage`]), not a fallback.
pub struct SceneStage<S> {
    /// The attachments by slot: colour, depth, sampled levels and slots,
    /// as the device resolved them when the stage was queued.
    pub surfaces: ExternalSurfaces<S>,
    pub request: ExternalPassRequest,
    /// The stage, the guest view and the ranges the stage promised.
    pub plan: ExternalPlan,
    /// The stage's typed inputs and its recording.
    pub pass: SharedReplacement,
}

/// Logs once per stage, site and cause (stderr) that a typed external pass
/// was declined before it could record, so a stage that silently stays on
/// its D3D draw names where it stopped. The cause counters of each stage
/// (`timeline::native`) stay the totals.
pub(crate) fn note_declined(stage: timeline::native::Stage, site: &'static str, line: u32, why: &str) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<(String, &'static str, u32, String)>>> = Mutex::new(None);
    let key = (format!("{stage:?}"), site, line, why.to_owned());
    let fresh = SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().insert(key);
    if fresh {
        eprintln!("d3d: {stage:?} external pass declined at {site}:{line}: {why} (logged once)");
    }
}

/// Validate and seal every copy, either on the worker or over pinned handles
/// on the device thread. Refusal discards all copies and their uploads.
#[cfg(feature = "gpu")]
pub(crate) fn prepare_external_recordings<T: AsRef<[ExternalAttachments]>>(
    request: ExternalPassRequest, plan: &ExternalPlan, copy: TargetCopy, pass: SharedReplacement,
    slots_valid: bool, pin: impl FnOnce() -> Result<ExternalCopies<T>, String>,
) -> Result<Option<Vec<ExternalRecording>>, String> {
    use timeline::native::{Metric, Timer};
    let item_count = pass.lock().map_err(|_| "native pass poisoned")?.items(plan.ranges) as u64;
    plan.stage.count(Metric::Examined, item_count);
    let refuse = |reason: Metric, line: u32| {
        note_declined(plan.stage, "backend.rs", line, &format!("{reason:?}"));
        plan.stage.count(reason, item_count);
        Ok(None)
    };
    let preparing = Timer::start(plan.stage, timeline::native::Metric::PrepareNs);
    if plan.ranges == 0 { return refuse(Metric::RefusedRanges, line!()); }
    let (x, y, w, h) = request.rect;
    let (sw, sh) = request.surface_size;
    if w == 0 || h == 0 || x.checked_add(w).is_none_or(|r| r > sw)
        || y.checked_add(h).is_none_or(|b| b > sh)
    {
        return refuse(Metric::RefusedResources, line!());
    }
    if !slots_valid {
        return refuse(Metric::RefusedResources, line!());
    }
    let targets = match pin() {
        Ok(ExternalCopies::Ready(targets)) => targets,
        Ok(ExternalCopies::Missing(copy)) => {
            plan.stage.copy_refused(plan.view, copy == TargetCopy::Display);
            return refuse(Metric::RefusedResources, line!());
        }
        Err(e) => { plan.stage.count(Metric::RefusedResources, item_count); return Err(e) }
    };
    let targets = targets.as_ref();
    if !matches!(targets, [one] if one.copy == copy || (one.copy == TargetCopy::Display && one.single_display))
        && !matches!(targets, [r, d] if r.copy == TargetCopy::Reference && d.copy == TargetCopy::Display)
    {
        return refuse(Metric::RefusedResources, line!());
    }
    enum Prepared {
        Draws(ExternalDraws),
        Encoded(ExternalEncoded),
        Commands(wgpu::CommandEncoder),
    }
    // Native wgpu resolves validation scopes synchronously on the CPU.
    // If a backend cannot report validation immediately, fail closed.
    fn validated(future: impl std::future::Future<Output = Option<wgpu::Error>>) -> bool {
        let mut future = std::pin::pin!(future);
        matches!(
            std::future::Future::poll(future.as_mut(), &mut std::task::Context::from_waker(std::task::Waker::noop())),
            std::task::Poll::Ready(None)
        )
    }

    let mut pass = pass.lock().map_err(|_| "native pass poisoned")?;
    let check_recording = !pass.uses_validated_resources();
    let counted = pass.counts_samples();
    let mut failure = None;
    let mut detail = String::new();
    let mut prepared = [None, None];
    for (slot, target) in prepared.iter_mut().zip(targets) {
        if pass.display_only() && target.copy == TargetCopy::Reference {
            continue;
        }
        let mut cause = String::new();
        let result = if let Some(draws) = pass.record_draws(target, request) {
            draws.map(Prepared::Draws).map_err(|e| { cause = format!("record_draws: {e}"); Metric::RefusedInputs })
        } else if counted {
            // Only ordered draw packets can contribute to the backend's report.
            cause = "a counted pass recorded no draw packets".into();
            Err(Metric::RefusedInputs)
        } else if let Some(encoded) = pass.record_encoded(target, request) {
            encoded.map_err(|e| { cause = format!("record_encoded: {e}"); Metric::RefusedInputs }).and_then(|encoded| {
                if check_recording {
                    let scope = target.device.push_error_scope(wgpu::ErrorFilter::Validation);
                    let mut scratch = target.device.create_command_encoder(&Default::default());
                    (encoded.commands)(&mut scratch);
                    drop(scratch.finish());
                    if !validated(scope.pop()) { return Err(Metric::RefusedValidation); }
                }
                Ok(Prepared::Encoded(encoded))
            })
        } else {
            let scope = check_recording.then(|| target.device.push_error_scope(wgpu::ErrorFilter::Validation));
            let mut encoder = target.device.create_command_encoder(&Default::default());
            let ready = pass.prepare_copy(&attachments_target(target, request, &mut encoder, None)).is_ok();
            if !scope.is_none_or(|scope| validated(scope.pop())) { cause = "prepare_copy validation".into(); Err(Metric::RefusedValidation) }
            else if !ready { cause = "prepare_copy refused".into(); Err(Metric::RefusedInputs) }
            else { Ok(Prepared::Commands(encoder)) }
        };
        match result {
            Ok(copy) => *slot = Some((target, copy)),
            Err(why) => {
                plan.stage.copy_refused(plan.view, target.copy == TargetCopy::Display);
                if failure.is_none() {
                    detail = format!("{:?} copy: {cause}", target.copy);
                }
                failure.get_or_insert(why);
            }
        }
    }
    if let Some(why) = failure {
        note_declined(plan.stage, "backend.rs", line!(), &detail);
        return refuse(why, line!());
    }
    drop(preparing);

    let _encode = Timer::start(plan.stage, Metric::EncodeNs);
    let mut recordings = Vec::with_capacity(targets.len());
    for (target, prepared) in prepared.into_iter().flatten() {
        let mut on_submit = Vec::new();
        let (ranges, commands, valid) = match prepared {
            Prepared::Draws(draws) => {
                let ranges = draws.ranges;
                // A packet can join adjacent ranges (the world's triangle lists).
                let valid = !draws.packets.is_empty()
                    && draws.packets.iter().all(|packet| packet.counts_samples() == counted);
                (ranges, ExternalCommands::Draws { targets: target.targets, draws }, valid)
            }
            Prepared::Encoded(encoded) => (encoded.ranges, ExternalCommands::Encoded(encoded), true),
            Prepared::Commands(mut encoder) => {
                let scope = check_recording.then(|| target.device.push_error_scope(wgpu::ErrorFilter::Validation));
                let bundles = pass.record_bundles(&mut attachments_target(target, request, &mut encoder, Some(&mut on_submit)));
                let (ranges, commands) = match bundles {
                    Some(draws) => {
                        // Bundle attachment compatibility is checked at execution.
                        if check_recording {
                            ExternalBundles::encode_run(&mut encoder, &target.color, &target.depth,
                                target.depth_format.has_stencil_aspect(), std::slice::from_ref(&draws));
                            drop(encoder.finish());
                        }
                        (draws.ranges, ExternalCommands::Bundles {
                            targets: target.targets, color: target.color.clone(), depth: target.depth.clone(),
                            stencil: target.depth_format.has_stencil_aspect(), draws,
                        })
                    }
                    None => {
                        let ranges = pass.record_ranges(attachments_target(target, request, &mut encoder, Some(&mut on_submit)));
                        (ranges, ExternalCommands::Standalone(encoder.finish()))
                    }
                };
                (ranges, commands, scope.is_none_or(|scope| validated(scope.pop())))
            }
        };
        plan.stage.attempted(plan.view, target.copy == TargetCopy::Display, ranges);
        if !valid || !plan.complete(target.copy, ranges) {
            plan.stage.copy_refused(plan.view, target.copy == TargetCopy::Display);
            failure.get_or_insert(if valid { Metric::RefusedRanges } else { Metric::RefusedValidation });
            continue;
        }
        recordings.push(ExternalRecording {
            stage: plan.stage,
            view: plan.view,
            items: item_count as usize,
            copy: target.copy,
            single_display: target.single_display,
            ranges,
            commands,
            uploads: target.uploads.clone(),
            on_submit,
        });
    }
    if let Some(why) = failure { return refuse(why, line!()); }
    Ok(Some(recordings))
}

/// Available only at an ordered draw boundary, after D3D resolved the draw.
pub struct DrawSnapshot<'a> {
    pub draw: &'a ResolvedDraw,
    pub vertex_shader: u32,
    pub pixel_shader: u32,
    /// Raw guest bindings, including bound stages the draw does not sample.
    pub bound_textures: [u32; 4],
    pub sampled_stages: [bool; 4],
    pub palettes: [u32; 4],
    pub color_header: u32,
    pub depth_header: u32,
}

pub trait DrawAdmission: Send + Sync {
    /// Attribute selection time and draws declined by this ownership filter.
    /// A dispatcher spanning several stages can account for its own filters.
    fn stage(&self) -> Option<timeline::native::Stage> { None }

    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)>;

    /// A query replacement must preserve the running sample counter.
    /// Ordinary owners are never consulted for counted packets.
    fn admit_counted(&self, _: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        None
    }
}

pub(crate) fn admit_draw(
    admission: &dyn DrawAdmission, snapshot: &DrawSnapshot<'_>,
) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
    use timeline::native::{Metric, Timer};
    if snapshot.draw.hw.zpass_count_enable {
        return admission.admit_counted(snapshot).filter(|(_, pass)| pass.counts_samples());
    }
    let stage = admission.stage();
    let _timer = stage.map(|s| Timer::start(s, Metric::PrepareNs));
    let candidate = admission.admit(snapshot).filter(|(_, pass)| !pass.counts_samples());
    if candidate.is_none() && let Some(s) = stage {
        s.count(Metric::Examined, 1);
        s.count(Metric::RefusedOwnership, 1);
    }
    candidate
}

pub use gfx::external::ExternalPassRequest;

/// The viewport of `request` in a `texels`-sized target: `[x, y, w, h]`.
pub fn external_viewport(request: &ExternalPassRequest, texels: (u32, u32)) -> [f32; 4] {
    gfx::external::external_viewport(request.rect, request.surface_size, texels)
}

/// What `request` draws into on `attachments`.
#[cfg(feature = "gpu")]
fn attachments_target<'a>(
    attachments: &'a ExternalAttachments,
    request: ExternalPassRequest,
    encoder: &'a mut wgpu::CommandEncoder,
    on_submit: Option<&'a mut Vec<Box<dyn FnOnce() + Send>>>,
) -> ExternalTarget<'a> {
    let this = attachments;
    ExternalTarget {
        device: &this.device, queue: &this.queue, encoder, color: &this.color, color_format: this.color_format,
        depth: &this.depth, depth_format: this.depth_format, viewport: external_viewport(&request, this.size),
        display_projection_factor: request.display_projection_factor,
        z_scale: request.z_scale, guest_size: request.surface_size, size: this.size, sampled: &this.sampled,
        uploads: &this.uploads, slots: &this.slots, stages: this.stages.each_ref().map(Option::as_ref), copy: this.copy,
        lighting: this.lighting,
        on_submit,
    }
}
/// `D3DVIEWPORT8`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Near depth.
    pub min_z: f32,
    /// Far depth.
    pub max_z: f32,
}

/// Colour channels a clear writes (`D3DCLEAR_TARGET_R/G/B/A`, Xbox bits
/// `0x10`/`0x20`/`0x40`/`0x80`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChannelMask {
    /// Red (`0x10`).
    pub red: bool,
    /// Green (`0x20`).
    pub green: bool,
    /// Blue (`0x40`).
    pub blue: bool,
    /// Alpha (`0x80`).
    pub alpha: bool,
}

impl ChannelMask {
    /// From the `D3DCLEAR` flags.
    pub fn from_flags(flags: u32) -> Self {
        Self {
            red: flags & 0x10 != 0,
            green: flags & 0x20 != 0,
            blue: flags & 0x40 != 0,
            alpha: flags & 0x80 != 0,
        }
    }

    /// Any channel at all.
    pub fn any(&self) -> bool {
        self.red || self.green || self.blue || self.alpha
    }

    /// All four.
    pub fn all(&self) -> bool {
        self.red && self.green && self.blue && self.alpha
    }
}

/// A clear that is not one whole-attachment clear: some rectangles, or
/// only some colour channels. The values are the same as [`Clear`]'s.
#[derive(Clone, Debug, PartialEq)]
pub struct RectClear {
    /// Rectangles, already clipped to the viewport and the target, none
    /// empty.
    pub rects: Vec<PixelRect>,
    /// Colour and the channels to write (`None`: colour untouched).
    pub color: Option<([f32; 4], ChannelMask)>,
    /// Depth (0..1).
    pub depth: Option<f32>,
    /// Stencil.
    pub stencil: Option<u8>,
}

/// How a colour surface's bytes are laid out in guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestLayoutKind {
    /// Morton order (a swizzled texture level).
    Swizzled,
    /// Rows `pitch` bytes apart.
    Linear {
        /// Row pitch in bytes.
        pitch: u32,
    },
}

/// A fully resolved draw: nothing in it refers to guest memory.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedDraw {
    /// Entry point that issued it (for events).
    pub entry: &'static str,
    /// Primitive type.
    pub primitive: PrimitiveType,
    /// Vertices in `streams`.
    pub vertex_count: u32,
    /// Vertex format.
    pub layout: VertexLayout,
    /// One per stream of `layout`.
    pub streams: Vec<StreamBytes>,
    /// Indexed draws: indices into `streams`, each below `vertex_count`, in
    /// primitive order. `None` draws the vertices in order.
    pub indices: Option<IndexData>,
    /// Translated render and stage state.
    pub state: DrawState,
    /// The raw state words the translation came from: everything the guest
    /// set, including what `DrawState` does not model (combiner registers,
    /// deferred states, ...). Shared with the device's pushed snapshot,
    /// so a draw does not copy the arrays.
    pub raw: std::sync::Arc<StateWords>,
    /// Stage state outside `TextureStageState` (alpha kill, colour sign,
    /// bump environment, colour key).
    pub extras: [StageExtras; 4],
    /// The render target.
    pub target: TargetInfo,
    /// Bound textures (dimensions and format per stage).
    pub texture_info: [Option<TextureInfo>; 4],
    /// Shader inputs.
    pub shader: ShaderInputs,
    /// Current viewport.
    pub viewport: Viewport,
    /// Where the draw may write: the viewport intersected with the
    /// (inclusive) scissor rectangles and the target. On the Xbox,
    /// `SetViewport` resets the scissors to the viewport rectangle
    /// (`SetScissors(0, 0, 0)` at `0x3f82e8`), so pre-transformed geometry is
    /// clipped to it. `None`: the scissors keep nothing. With several
    /// inclusive scissor rectangles, or exclusive ones, this is the bounding
    /// box of the area and [`clip_rects`](Self::clip_rects) holds its pieces.
    pub clip: Option<PixelRect>,
    /// The writable area as disjoint rectangles inside `clip`, when it is
    /// not one rectangle: the union of several inclusive scissor
    /// rectangles, or what exclusive ones leave of the viewport. Empty when
    /// `clip` alone describes it. A backend draws once per piece (disjoint,
    /// so blending touches every pixel once).
    pub clip_rects: Vec<PixelRect>,
    /// NV2A register state when the draw was issued (viewport offset and
    /// scale, depth clip range, `SET_CONTROL0`; see [`crate::hw`]). The
    /// viewport transform of programmable and pass-through shaders lives
    /// here (and in constants 58/59), not in `viewport`.
    pub hw: crate::hw::HwState,
    /// Header addresses of the bound textures (identity for the backend's
    /// own bookkeeping; the resolved host textures are passed separately).
    pub textures: [Option<u32>; 4],
    /// Layout of this draw when issued by a screen or HUD callback.
    pub ui_fit: Option<UiFit>,
    /// Original GPU constants when this perspective draw has a different
    /// display lens. The exact reference and its readbacks use these.
    pub retail_constants: Option<(crate::shader::Constants, f32)>,
    /// Stages (bit `n` for stage `n`) whose texture is the game's glyph
    /// cache ([`crate::glyph`]): text a view backend may magnify with the
    /// glyph filter. Zero when smoothing is off.
    pub glyph_cache: u8,
}

impl ResolvedDraw {
    /// The exact copy sees retail placement and lens. Identity draws need
    /// neither a new packet nor another copy of the constant block.
    pub fn for_reference(&self) -> std::borrow::Cow<'_, Self> {
        if self.ui_fit.is_none() && self.retail_constants.is_none() {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut retail = self.clone();
        if let Some((constants, _)) = retail.retail_constants.take() {
            retail.shader.constants = constants;
        }
        retail.ui_fit = None;
        std::borrow::Cow::Owned(retail)
    }

    pub fn for_copy(&self, copy: TargetCopy) -> std::borrow::Cow<'_, Self> {
        if copy == TargetCopy::Reference { self.for_reference() } else { std::borrow::Cow::Borrowed(self) }
    }

    /// Bump offsets added to a projected scene lookup are screen displacements.
    /// The reference copy has already restored the original projection.
    #[cfg(feature = "gpu")]
    pub(crate) fn bump_matrix(&self, stage: usize) -> [f32; 4] {
        let extra = self.extras[stage];
        let mut matrix = extra.bump_env_mat;
        if extra.samples_frame && let Some((_, factor)) = &self.retail_constants {
            let (x, y) = (canvas_fit::horizontal_term(*factor), canvas_fit::vertical_term(*factor));
            matrix[0] *= x;
            matrix[2] *= x;
            matrix[1] *= y;
            matrix[3] *= y;
        }
        matrix
    }

    /// Translate a scissor in the same guest-pixel space as its draw.
    pub fn display_rect(&self, rect: PixelRect) -> PixelRect {
        if let Some(fit) = self.ui_fit { return fit.map_rect(rect); }
        let Some((_, factor)) = &self.retail_constants else { return rect };
        let v = self.viewport;
        let view = (v.x, v.y, v.width, v.height);
        if *factor > 1.0 { rect.projected_y(view, *factor) } else { rect.projected_x(view, *factor) }
    }

    /// Texture stages whose program reads the bound image.
    pub fn sampled_texture_stages(&self) -> [bool; 4] {
        crate::device::sampled_stages(&self.shader, &self.raw, &self.state)
    }

    /// Whether submission can change the attached guest depth or stencil.
    pub fn writes_depth_stencil(&self) -> bool {
        crate::device::writes_depth_or_stencil(&self.state.render, self.target.depth_format)
    }
}

/// Predicts a backend's [`Backend::draw`] outcomes without running it, so
/// a backend that renders on another thread (`crate::threaded`) answers
/// each draw exactly when it is issued: skipped where the backend would
/// skip it. Made by [`Backend::judge`] before the backend moves.
pub trait DrawJudge: Send {
    /// Whether a texture uploaded from `texture` has a view the shaders
    /// sample.
    fn upload_view(&self, texture: &DecodedTexture) -> bool;
    /// The same for a texture over a colour surface of `format` (a
    /// snapshot, or the surface sampled in place).
    fn surface_view(&self, format: D3DFormat) -> bool;
    /// What [`Backend::draw`] answers for `draw`: `views[n]` is `None`
    /// with no texture on stage `n`, else whether it has a view.
    fn draw(&mut self, draw: &ResolvedDraw, views: [Option<bool>; 4]) -> DrawOutcome;
}

/// Result of [`Backend::draw`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrawOutcome {
    /// Rendered.
    Drawn,
    /// Not rendered; the device records `Event::DrawSkipped` with the reason.
    Skipped(String),
    /// The backend would render this wrongly (state it cannot express):
    /// the device stops with `D3dError::Unimplemented`.
    Unsupported(String),
}

/// Create immutable, device-local assets for future scene draws. This
/// command has no target handles, guest bindings, frame encoder or reads.
#[cfg(feature = "gpu")]
pub type AssetUpload = Box<dyn FnOnce(&wgpu::Device, &wgpu::Queue) + Send>;

/// The resolution setting and guest surface size at the start of a Swap.
/// A scaled backend applies setting changes after it shows the frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct RasterMeasurement {
    pub configured: Option<u32>,
    pub guest: Option<(u32, u32)>,
}

impl RasterMeasurement {
    pub fn finish(self, shown: Option<(u32, u32)>) -> crate::scene_sink::window::PresentScale {
        let physical = self.guest.zip(shown).and_then(|((gw, gh), (w, h))| {
            let scale = w.checked_div(gw)?;
            (gh != 0 && scale != 0 && gw.checked_mul(scale) == Some(w)
                && gh.checked_mul(scale) == Some(h)).then_some(scale)
        });
        crate::scene_sink::window::PresentScale { configured: self.configured, physical }
    }
}

#[cfg(test)]
mod raster_tests {
    use super::RasterMeasurement;

    #[test]
    fn a_physical_scale_needs_a_uniform_integral_extent_in_both_dimensions() {
        let measure = RasterMeasurement { configured: Some(4), guest: Some((8, 4)) };
        for shown in [None, Some((0, 0)), Some((28, 14)), Some((32, 4)), Some((4, 2))] {
            assert_eq!(measure.finish(shown).physical, None);
        }
        assert_eq!(measure.finish(Some((8, 4))).physical, Some(1));
        assert_eq!(measure.finish(Some((32, 16))).physical, Some(4));
        assert_eq!(RasterMeasurement::default().finish(Some((32, 16))).physical, None);
    }
}

/// The ordinary display probe reads tightly packed guest-format BGRA pixels.
pub(crate) fn read_frame_probe<B: Backend + ?Sized>(
    backend: &mut B, surface: B::Surface, width: u32, height: u32,
) -> Result<Vec<u8>, String> {
    let pitch = width.checked_mul(4).ok_or("frame probe row pitch overflow")?;
    pitch.checked_mul(height).ok_or("frame probe image size overflow")?;
    backend.read_surface(surface, D3DFormat::LinA8R8G8B8, width, height, GuestLayoutKind::Linear { pitch })
}

/// A target the device asks for by role ([`Backend::create_target`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetDesc {
    pub role: frame_types::TargetRole,
    pub format: D3DFormat,
    pub width: u32,
    pub height: u32,
    pub depth: bool,
    /// A texture's level bound as a target (sampled in place).
    pub texture: bool,
}

/// Host rendering for the device.
/// `Send` (with its handles) so a device can sit behind the runtime's
/// `Send + Sync` override handlers.
pub trait Backend: Send {
    /// Run an asset upload on the same device that records scene stages.
    /// A backend without a GPU discards the request.
    #[cfg(feature = "gpu")]
    fn upload_scene_assets(&mut self, _upload: AssetUpload) {}

    /// Handle of a colour or depth/stencil surface.
    type Surface: Copy + Eq + std::fmt::Debug + Send;
    /// A host texture made from guest texture memory.
    type Texture: Send;

    /// Pins the GPU images bound by the original texture. Immutable samples
    /// can be shared. A live view uses these handles only to verify that its
    /// original source allocation still matches the current frame target.
    #[cfg(feature = "gpu")]
    fn frozen_images(&self, _texture: &Self::Texture) -> Option<crate::scene_sink::FrozenImages> {
        None
    }

    /// The threaded backend resolves its proxy in queue order without a
    /// guest wait. Synchronous encoders can share the submitted images now.
    #[cfg(feature = "gpu")]
    fn retain_scene_sample(&self, texture: &Self::Texture) -> Option<crate::scene_sink::FrozenSample> {
        self.frozen_images(texture).map(crate::scene_sink::FrozenSample::ready)
    }

    /// Creates a surface (`depth` selects depth/stencil).
    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String>;
    /// Destroys a surface made by [`create_surface`](Self::create_surface).
    /// The handle must not be used again.
    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String>;
    /// Uploads a decoded guest texture.
    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String>;
    /// An immutable upload shared with a scene. The render queue can
    /// identify its exact texels by allocation across frames.
    fn upload_shared(&mut self, texture: std::sync::Arc<DecodedTexture>) -> Result<Self::Texture, String> {
        self.upload_texture(&texture)
    }
    /// Whether the backend can supply descriptor-only textures. Such handles
    /// never back real drawing, and no texel or palette data is needed.
    fn uses_texture_placeholders(&self) -> bool {
        false
    }
    fn texture_placeholder(&mut self, _desc: &TextureDesc) -> Result<Self::Texture, String> {
        Err("the backend has no descriptor-only texture handles".into())
    }
    /// Whether BC formats pass through (decides the decode path).
    fn bc_supported(&self) -> bool;
    /// Clears the whole of the given attachments.
    fn clear(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: Clear,
    ) -> Result<(), String>;
    /// Draws into `color` (+ `depth`).
    fn draw(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: &ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String>;
    /// [`draw`](Self::draw) of a draw the caller is done with, so a backend
    /// that keeps it past the call (the render thread) takes it instead of
    /// copying it. Also returns stream buffers for the caller to fill next
    /// time: the draw's own, or an earlier draw's the backend is done with.
    fn draw_owned(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        let r = self.draw(color, depth, &draw, textures);
        (r, draw.streams)
    }
    /// Submit a packet whose immutable contents also belong to a sealed
    /// frame. Queued renderers may share it with the replay until Swap.
    fn draw_shared(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: std::sync::Arc<ResolvedDraw>,
        textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        let result = self.draw(color, depth, &draw, textures);
        let streams = std::sync::Arc::into_inner(draw).map(|d| d.streams).unwrap_or_default();
        (result, streams)
    }
    /// Marks this thread's frame-stat boundary. Asynchronous backends
    /// forward queued statistics without waiting for the render thread.
    fn frame_stats(&mut self) {}
    /// Observe the next original Swap on this device. A wrapping display may
    /// provide the guest extent itself and arm its view with no surface.
    fn measure_present_raster(&mut self, _surface: Option<Self::Surface>) -> RasterMeasurement {
        RasterMeasurement::default()
    }
    /// Physical texel extent actually handed to the display for that Swap.
    /// A failed or unshown Swap and a backend without an observer return None.
    fn take_presented_extent(&mut self) -> Option<(u32, u32)> { None }

    /// True only when the last ordinary Swap exhausted all display attempts.
    /// Backends without display feedback leave the outcome unknown.
    fn last_present_unshown(&self) -> bool { false }

    /// Shows a host-only frame between the guest's presents (a display
    /// frame between scenes): the present hook sees it,
    /// but it is not a guest frame, so frame counters, shader upkeep and
    /// the `D3DSWAP_FINISH` wait are left alone where a backend has them.
    /// The default presents with no flags.
    fn present_replay(
        &mut self,
        surface: Self::Surface,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.present_with_gamma(surface, 0, gamma)
    }
    /// The size in host texels of what `surface` shows when presented (a
    /// view backend's scaled copy, say). Host-only: a private draw pass
    /// sizes its targets by it. `None` (the default): not known.
    fn surface_extent(&self, surface: Self::Surface) -> Option<(u32, u32)> {
        let _ = surface;
        None
    }
    /// Host allocation sizes of the selected display copy, or the
    /// reference copy when that draw has no display allocation. A backend
    /// may need to wait for resource creation on a worker. This reports
    /// backing allocations, so a temporary attachment used by a particular
    /// draw can have a different size.
    fn render_extents(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        textures: [Option<&Self::Texture>; 4],
    ) -> RenderExtents {
        let _ = textures;
        RenderExtents {
            color: self.surface_extent(color),
            depth: depth.and_then(|surface| self.surface_extent(surface)),
            textures: [None; 4],
        }
    }

    /// Reads a selected display allocation at its host size for diagnostics.
    /// Unsupported backends return `Ok(None)`.
    fn read_display_surface(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<DisplaySurfacePixels>, String> {
        let _ = surface;
        Ok(None)
    }

    /// Waits until a render thread has run everything queued so far
    /// (`ThreadedBackend::finish`). Host-only: a machine snapshot drains
    /// the render thread before it saves (docs/runtime/snapshots.md). The
    /// default has no thread to wait for.
    fn drain(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Discards host mark samples when a snapshot restores the device.
    fn reset_present_metadata(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Shows `surface` (`Swap`). `flags` are the guest's `D3DSWAP_*` flags.
    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String>;
    /// Presents with the current DAC ramp. Backends without a display may ignore it.
    fn present_with_gamma(
        &mut self,
        surface: Self::Surface,
        flags: u32,
        _gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.present(surface, flags)
    }
    /// Clears rectangles of the attachments, or only some colour channels.
    /// `Unsupported` stops the device (the default: no backend support).
    fn clear_rects(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        let _ = (color, depth);
        Ok(DrawOutcome::Unsupported(format!(
            "partial clear ({} rectangle(s), channels {:?}): the backend has no clear_rects",
            clear.rects.len(),
            clear.color.map(|c| c.1)
        )))
    }
    /// Diagnostic pixels at this pass boundary. A worker that adopts the
    /// sealed scene discards this callback; its graph provides the snapshot.
    /// A read failure belongs to the probe and leaves guest rendering live.
    fn queue_frame_probe_read(
        &mut self, surface: Self::Surface, width: u32, height: u32,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        done(read_frame_probe(self, surface, width, height));
    }

    /// Reads a colour surface back as guest bytes: `format` texels of a
    /// `width x height` level laid out as `layout` (render-to-texture
    /// write-back). The default cannot.
    fn read_surface(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        let _ = (surface, width, height, layout);
        Err(format!(
            "the backend cannot read surfaces back ({format:?})"
        ))
    }
    /// Reads a depth/stencil surface back as guest bytes: `format` (`D16`
    /// or `D24S8`, as `z << 8 | stencil`) texels of a `width x height`
    /// image laid out as `layout`. For a render-to-texture depth level's
    /// write-back, and for textures that alias the depth buffer's memory.
    /// `Ok(None)`: the backend cannot (the default); a write-back then
    /// stops and an aliasing texture skips its draw.
    fn read_depth(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        let _ = (surface, format, width, height, layout);
        Ok(None)
    }
    /// Copies the whole of colour surface `src` into `dst` (same format and
    /// size). `Ok(false)`: the backend cannot (the default).
    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        let _ = (src, dst);
        Ok(false)
    }
    /// Fills a colour or depth/stencil surface from guest texels: `desc` is
    /// the level as the guest lays it out (format, size, and the pitch of a
    /// linear level), so a render-to-texture target starts with the level's
    /// current texels (`D16`/`D24S8` for a depth level).
    /// `Ok(false)`: the backend cannot, and the surface keeps whatever it
    /// was created with (the default).
    fn write_surface(
        &mut self,
        surface: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        let _ = (surface, desc, bytes);
        Ok(false)
    }
    /// A texture holding colour surface `surface` as it is now (a copy:
    /// later drawing into the surface does not reach it). For a texture
    /// header whose memory is a surface that has no guest copy, such as the
    /// back buffer read through a texture that aliases it. With `opaque`
    /// the copy's alpha is one (an `X8` texture over the surface).
    /// `Ok(None)`: the backend cannot (the default), and the draw is
    /// skipped.
    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        let _ = (surface, opaque);
        Ok(None)
    }
    /// Refills `into`, a texture an earlier [`snapshot_surface`] of a
    /// surface of the same size and format made, with colour surface
    /// `surface` as it is now (alpha one with `opaque`): a GPU copy into
    /// the existing texture, recorded after everything drawn so far and
    /// before any later draw, so a draw recorded before this call still
    /// reads the old contents and one after it the new. Nothing is
    /// allocated. The device keeps a small pool of these copies and
    /// refills them (docs/graphics/d3d.md, "Frame buffers as textures").
    /// `Ok(false)`: the backend cannot (the default, or `into` does not
    /// match the surface), and the caller makes a new copy instead.
    ///
    /// [`snapshot_surface`]: Self::snapshot_surface
    fn snapshot_surface_into(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
        into: &mut Self::Texture,
    ) -> Result<bool, String> {
        let _ = (surface, opaque, into);
        Ok(false)
    }
    /// `CLEAR_REPORT_VALUE` (zpass): the sample count restarts at zero.
    fn clear_pixel_count(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Pin the actual resources for every live copy. No pass has run yet.
    #[cfg(feature = "gpu")]
    fn external_attachments(
        &mut self, _surfaces: ExternalSurfaces<Self::Surface>, _textures: [Option<&Self::Texture>; 4],
        _copy: TargetCopy,
    ) -> Result<ExternalCopies, String> {
        Ok(ExternalCopies::Missing(_copy))
    }

    /// Prepare all target copies before recording any of them. Record into
    /// disposable command buffers and check validation before releasing the guest draw.
    #[cfg(feature = "gpu")]
    fn prepare_external(
        &mut self, surfaces: ExternalSurfaces<Self::Surface>, request: ExternalPassRequest,
        textures: [Option<&Self::Texture>; 4], plan: &ExternalPlan, copy: TargetCopy, pass: SharedReplacement,
    ) -> Result<Option<Vec<ExternalRecording>>, String> {
        let slots_valid = surfaces.slots.len() == surfaces.slot_sizes.len()
            && surfaces.slots.iter().zip(&surfaces.slot_sizes).all(|(s, size)| match (s, size) {
                (None, None) => true,
                (Some(_), Some((w, h))) => *w != 0 && *h != 0,
                _ => false,
            });
        prepare_external_recordings(request, plan, copy, pass, slots_valid, || {
            self.external_attachments(surfaces, textures, copy)
        })
    }

    #[cfg(not(feature = "gpu"))]
    fn prepare_external(
        &mut self, _surfaces: ExternalSurfaces<Self::Surface>, _request: ExternalPassRequest,
        _textures: [Option<&Self::Texture>; 4], _plan: &ExternalPlan, _copy: TargetCopy, _pass: SharedReplacement,
    ) -> Result<Option<Vec<ExternalRecording>>, String> {
        Ok(None)
    }

    /// A light can replace the guest's pass at this queue boundary. Also
    /// checked for an empty light, which has no stage to record.
    fn can_commit_carried(&self) -> bool { true }

    /// Seal every physical copy of a typed stage without issuing commands.
    /// The caller holds these recordings until the *whole* light and its
    /// device tail have been admitted. A scene sink also keeps the typed
    /// inputs on commit, for fresh recording on the replay's attachments.
    fn prepare_carried_stage(
        &mut self, stage: &SceneStage<Self::Surface>,
    ) -> Result<Option<Vec<ExternalRecording>>, String> {
        self.prepare_external(stage.surfaces.clone(), stage.request, [None; 4], &stage.plan,
            TargetCopy::Reference, std::sync::Arc::clone(&stage.pass))
    }

    /// Issue the sealed stage at this position in the command stream.
    /// A frame owner consumes either these recordings or the matching
    /// replay stage on its own attachments.
    fn commit_carried_stage(
        &mut self, _stage: SceneStage<Self::Surface>, copies: Vec<ExternalRecording>,
    ) {
        for copy in copies { self.commit_external(copy); }
    }

    /// The guest's one draw for a packet that was offered to a typed pass
    /// and not taken. The draw, its outcome and its stream buffers are those
    /// of [`draw_owned`](Self::draw_owned), which is all the default does.
    /// A backend that retains the frame may keep `admission` beside it.
    fn draw_admitted(
        &mut self, color: Self::Surface, depth: Option<Self::Surface>, draw: ResolvedDraw,
        textures: [Option<&Self::Texture>; 4], _admission: RetainedAdmission<Self::Surface>,
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        self.draw_owned(color, depth, draw, textures)
    }

    /// A native producer is about to commit `admission`'s pass with no guest
    /// draw to carry it (the guest's call is skipped, [`Self::draw_admitted`]
    /// is never reached). A backend that retains the frame answers whether it
    /// will carry that commit and the packets the pass replaces, so the
    /// retained frame holds the pass as it holds an admitted guest draw: one
    /// draw per packet, in order, under the one admission. On
    /// true, [`Self::prepare_external`] and the commits that follow are not
    /// refused as unretainable until [`Self::end_native_carry`]. Every
    /// `begin_native_carry` is closed by one `end_native_carry`.
    fn begin_native_carry(
        &mut self, _color: Self::Surface, _depth: Option<Self::Surface>, _admission: &RetainedAdmission<Self::Surface>,
    ) -> bool { false }

    /// The pass prepared under a true [`Self::begin_native_carry`] is about to
    /// commit: retain its packets, each with the textures the pass was
    /// resolved against (a batch's siblings share its head's), as admitted
    /// draws.
    fn record_native_carry(
        &mut self, _color: Self::Surface, _depth: Option<Self::Surface>, _textures: [Option<&Self::Texture>; 4],
    ) {}

    /// Closes [`Self::begin_native_carry`], committed or not.
    fn end_native_carry(&mut self) {}

    /// Try to enqueue an independently resolved draw with its exact fallback.
    /// True guarantees the same Drawn outcome and attachment writes for either route.
    fn queue_external_draw(
        &mut self, _candidate: QueuedExternalDraw<Self::Surface>,
        _textures: [Option<&Self::Texture>; 4],
    ) -> bool { false }

    /// Can simple, ordered draws reuse pinned attachments until the next
    /// non-batch command? Backends with live admission policy can opt out.
    fn reusable_external_attachments(&self) -> bool { false }

    /// Whether [`run_scene_stage`](Self::run_scene_stage) records a stage
    /// asked now (docs/graphics/framescene.md, slice 2). `false` by
    /// default.
    fn runs_scene_stages(&self) -> bool { false }

    /// Prepares and records `stage` on this backend's thread, every copy,
    /// and commits it in order after the work so far. `Ok(false)`: it
    /// could not be recorded (the caller counts it dropped).
    fn run_scene_stage(&mut self, stage: SceneStage<Self::Surface>) -> Result<bool, String> {
        #[cfg(feature = "gpu")]
        {
            let SceneStage { surfaces, request, plan, pass } = stage;
            // The reference draws retail: a pass of one mode is the frame
            // graph's to record ([`ExternalPass::lighting_only`]).
            if pass.lock().map_err(|_| "native pass poisoned")?.lighting_only().is_some() {
                return Ok(true);
            }
            let display_only = pass.lock().map_err(|_| "native pass poisoned")?.display_only();
            let Some(recordings) = self.prepare_external(surfaces, request, [None; 4], &plan, TargetCopy::Reference, pass)? else {
                return Ok(false);
            };
            // A reference-only backend has nothing to submit for a display
            // supplement. The scene sink can still retain it for display replay.
            let complete = if display_only {
                matches!(recordings.as_slice(), [] | [_])
                    && recordings.iter().all(|r| r.copy == TargetCopy::Display && plan.complete(r.copy, r.ranges))
            } else {
                crate::device::complete_recordings(&plan, &recordings)
            };
            if !complete {
                return Ok(false);
            }
            for recording in recordings {
                self.commit_external(recording);
            }
            Ok(true)
        }
        #[cfg(not(feature = "gpu"))]
        {
            let _ = stage;
            Ok(false)
        }
    }

    /// The backend can accept this particular pass into a scene reservation.
    /// Checked while the guest still owns all the draws. A sink that keeps
    /// a replay requires the pass to support repeated recording.
    fn accepts_scene_pass(&self, _pass: &dyn ExternalPass) -> bool { true }

    /// Whether the last [`prepare_external`](Self::prepare_external) was
    /// refused because a scene recorder would not keep the pass, cleared by
    /// the call. Only a replaying sink says yes.
    fn take_scene_refusal(&mut self) -> bool { false }

    /// Whether [`queue_scene_stage`](Self::queue_scene_stage) takes a
    /// stage now: a render thread whose backend
    /// [runs scene stages](Self::runs_scene_stages). Asked before the
    /// guest is told to skip anything. `false` by default.
    fn queues_scene_stages(&self) -> bool { false }

    /// Queues `stage` for the render thread, in order after the work so
    /// far; it is prepared and recorded there. `false`: not taken (only
    /// when [`queues_scene_stages`](Self::queues_scene_stages) said no).
    fn queue_scene_stage(&mut self, stage: SceneStage<Self::Surface>) -> bool {
        let _ = stage;
        false
    }

    /// Splice an already validated recording into submission order. A backend
    /// implementing prepare must also implement commit. Commit cannot refuse.
    fn commit_external(&mut self, _recording: ExternalRecording) {
        unreachable!("backend did not prepare an external recording")
    }

    /// Device and queue shared by every copy this backend draws. A map
    /// loader can prepare immutable resources without touching a live pass.
    #[cfg(feature = "gpu")]
    fn upload_device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        None
    }

    /// Whether [`external_pass`](Self::external_pass) can draw (asked
    /// before the guest skips its own work for it). `false` by default.
    fn supports_external_pass(&self) -> bool {
        false
    }
    /// Runs `pass` into colour surface `color` and depth surface `depth`,
    /// in order after the work recorded so far. `depth` the same surface
    /// as `color`: a colour-only pass ([`ExternalPass::colour_only`]),
    /// given a scratch depth of the colour's size. `sampled`: per header
    /// of [`ExternalPass::sampled`], the surface it samples, which the pass
    /// gets a view of (`ExternalTarget::sampled`). `Ok(false)`: the
    /// backend cannot (the default) and `pass` is dropped.
    fn external_pass(
        &mut self,
        color: Self::Surface,
        depth: Self::Surface,
        sampled: Vec<Option<Self::Surface>>,
        request: ExternalPassRequest,
        pass: Box<dyn ExternalPass>,
    ) -> Result<bool, String> {
        let _ = (color, depth, sampled, request, pass);
        Ok(false)
    }
    /// `GET_REPORT`: a ticket for the samples counted since the last
    /// [`clear_pixel_count`](Self::clear_pixel_count), by every draw whose
    /// [`ResolvedDraw::hw`] had `zpass_count_enable` set (samples that
    /// passed the depth and stencil tests, as the NV2A counts them). It need
    /// not wait for the GPU; [`read_pixel_count`](Self::read_pixel_count)
    /// does. `Ok(None)`: the backend cannot count (the default), and the
    /// device stops.
    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        Ok(None)
    }
    /// The count behind a ticket from
    /// [`report_pixel_count`](Self::report_pixel_count), waiting for the
    /// GPU if it has to. The ticket is spent.
    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        Err(format!("the backend cannot count pixels (ticket {ticket})"))
    }
    /// The count behind a ticket if the GPU has already written it,
    /// without waiting; `Some` spends the ticket and is the value
    /// [`read_pixel_count`](Self::read_pixel_count) would give. `Ok(None)`:
    /// not back yet, or the backend cannot tell (the default); the ticket
    /// stays live.
    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        let _ = ticket;
        Ok(None)
    }
    /// Forgets a ticket that will never be read.
    fn drop_pixel_count(&mut self, ticket: u64) {
        let _ = ticket;
    }
    /// Whether the report behind `ticket` is still only recorded, not yet
    /// on its way to the GPU: [`try_pixel_count`](Self::try_pixel_count)
    /// would submit it. A poller that should not force that (the render
    /// thread landing counts after each batch) skips it. Default false.
    fn pixel_count_unsubmitted(&self, ticket: u64) -> bool {
        let _ = ticket;
        false
    }
    /// A colour surface that is also a texture, for a render-to-texture
    /// level. The default is [`create_surface`](Self::create_surface).
    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<Self::Surface, String> {
        self.create_surface(format, width, height, false)
    }
    /// A render target or depth surface of the game's, made by its role
    /// (docs/graphics/retire-d3d-display.md, "The graph's targets"): the
    /// threaded backend's worker asks the frame graph's store for it, so
    /// one role keeps one surface across frames. `texture` is a level of a
    /// texture bound as a target. A backend with no store makes the
    /// surface itself, as [`create_surface`](Self::create_surface) and
    /// [`create_texture_target`](Self::create_texture_target) do.
    fn create_target(&mut self, target: TargetDesc) -> Result<Self::Surface, String> {
        let TargetDesc { format, width, height, depth, texture, .. } = target;
        if texture { self.create_texture_target(format, width, height) } else { self.create_surface(format, width, height, depth) }
    }
    /// Gives back a surface made by [`create_target`](Self::create_target).
    /// The handle must not be used again; the store may keep the surface
    /// for the role's next target of the same shape.
    fn release_target(&mut self, role: frame_types::TargetRole, surface: Self::Surface) -> Result<(), String> {
        let _ = role;
        self.destroy_surface(surface)
    }
    /// Whether [`surface_texture`](Self::surface_texture) works. A device
    /// on a backend that can sample its render-to-texture surfaces keeps
    /// their pixels on the host and writes them back to guest memory only
    /// when the guest is about to read them (`docs/graphics/d3d.md`).
    fn can_sample_surfaces(&self) -> bool {
        false
    }
    /// A texture that samples [`create_texture_target`](Self::create_texture_target)
    /// surface `surface` itself, not a copy: draws recorded later see what
    /// was drawn into it before them. Sampled exactly as a texture uploaded
    /// from the surface's read-back texels would be (formats whose texels
    /// read back unchanged only). `Ok(None)`: the backend cannot (the
    /// default).
    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        let _ = surface;
        Ok(None)
    }
    /// Whether [`depth_level_texture`](Self::depth_level_texture) answers
    /// for `D24S8` depth surfaces. With it, lazy render-to-texture keeps
    /// depth levels on the host too (`docs/graphics/d3d.md`).
    fn can_sample_depth_surfaces(&self) -> bool {
        false
    }
    /// A texture with the texels a depth texture over depth/stencil
    /// surface `surface` decodes to once the surface is written back as
    /// `D24S8` ([`read_depth`](Self::read_depth)): its raw bits
    /// ([`gfx::convert::depth_channels`]), one 2D level of the surface's
    /// size, as of the work queued so far, without reading back.
    /// `Ok(None)`: the backend cannot (the default).
    fn depth_level_texture(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<Self::Texture>, String> {
        let _ = surface;
        Ok(None)
    }
    /// A [`DrawJudge`] that answers this backend's draw outcomes on the
    /// device's side. `None` (the default): they cannot be predicted.
    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        None
    }
    /// Whether [`depth_alias_texture`](Self::depth_alias_texture) can also reinterpret a colour or depth RTT
    /// at any byte offset. A zero source pitch selects Morton order.
    fn handles_surface_aliases(&self) -> bool {
        false
    }

    /// Whether [`depth_alias_texture`](Self::depth_alias_texture) makes
    /// depth-alias textures (default false).
    fn defers_depth_aliases(&self) -> bool {
        false
    }
    /// A texture over the depth buffer's memory, made on the backend's side:
    /// the guest words [`read_depth`](Self::read_depth) gives for `depth`
    /// (`format`, `width x height`, rows `pitch` bytes) as the device's
    /// depth generation `generation` left them, from byte `offset` on,
    /// decoded as `desc` with `options` and uploaded. The bytes never reach
    /// guest memory, so a backend that renders on another thread can make
    /// it there without the guest waiting (`threaded`). `Ok(None)`: the
    /// device reads the depth back and decodes it itself (the default).
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
        let _ = (
            depth, format, width, height, pitch, generation, offset, desc, options,
        );
        Ok(None)
    }
    /// Whether the backend keeps the alias mirror: a buffer on its side
    /// standing for a range of guest memory, which render-to-texture levels
    /// over the same memory move their pixels through without a read-back
    /// (`mirror_*`, docs/graphics/d3d.md "Overlapping levels on the GPU"). Default
    /// false; the device then writes overlapping levels back to guest
    /// memory.
    fn can_mirror(&self) -> bool {
        false
    }
    /// Makes the mirror `len` bytes (a multiple of 4). Its contents are
    /// undefined until written.
    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        Err(format!("the backend has no alias mirror ({len} bytes)"))
    }
    /// Stores guest bytes into the mirror at byte `offset` (both multiples
    /// of 4), after the work queued so far.
    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        let _ = bytes;
        Err(format!(
            "the backend has no alias mirror (write at {offset:#x})"
        ))
    }
    /// Encodes a render-to-texture surface into the mirror from byte
    /// `offset` as `format` texels laid out as `layout`, exactly the bytes
    /// [`read_surface`](Self::read_surface) (a colour surface) or
    /// [`read_depth`](Self::read_depth) (a `D24S8` one) would give for the
    /// texels; pitch padding is left as it was.
    fn mirror_store(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        let _ = (surface, width, height, layout, offset);
        Err(format!("the backend has no alias mirror ({format:?})"))
    }
    /// Fills colour surface `surface` from the mirror's bytes at `offset`
    /// as [`write_surface`](Self::write_surface) of those bytes would; or a
    /// depth/stencil surface from `D24S8` words, when the backend
    /// [samples depth surfaces](Self::can_sample_depth_surfaces).
    fn mirror_load(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        let _ = (surface, width, height, layout, offset);
        Err(format!("the backend has no alias mirror ({format:?})"))
    }
    /// Reads `len` mirror bytes from `offset` (multiples of 4) after the
    /// work queued so far: a GPU round trip.
    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        Err(format!(
            "the backend has no alias mirror (read {len} at {offset:#x})"
        ))
    }
}

/// What [`RecordingBackend`] saw.
#[derive(Clone, Debug, PartialEq)]
pub enum Recorded {
    /// `create_surface`.
    Surface {
        /// Handle.
        id: u32,
        /// Format.
        format: D3DFormat,
        /// Size.
        size: (u32, u32),
        /// Depth surface?
        depth: bool,
    },
    /// `upload_texture`.
    Upload {
        /// Handle.
        id: u32,
        /// Size.
        size: (u32, u32),
    },
    /// `clear`.
    Clear {
        /// Target.
        color: u32,
        /// What.
        clear: Clear,
    },
    /// `draw`.
    Draw {
        /// Target.
        color: u32,
        /// The draw.
        draw: Box<ResolvedDraw>,
        /// Texture handles per stage.
        textures: [Option<u32>; 4],
    },
    /// `destroy_surface`.
    DestroySurface {
        /// Handle.
        id: u32,
    },
    /// `clear_rects`.
    ClearRects {
        /// Target.
        color: u32,
        /// What.
        clear: RectClear,
    },
    /// `read_surface`.
    ReadSurface {
        /// Surface.
        surface: u32,
        /// Bytes returned.
        len: usize,
    },
    /// `read_depth`.
    ReadDepth {
        /// Surface.
        surface: u32,
        /// Format asked for.
        format: D3DFormat,
        /// Bytes returned.
        len: usize,
    },
    /// `copy_surface`.
    CopySurface {
        /// Source.
        src: u32,
        /// Destination.
        dst: u32,
    },
    /// `surface_texture`.
    SurfaceTexture {
        /// Surface sampled.
        surface: u32,
        /// The texture handle returned.
        id: u32,
    },
    /// `depth_level_texture`.
    DepthLevelTexture {
        /// Depth surface converted.
        surface: u32,
        /// The texture handle returned.
        id: u32,
    },
    /// `snapshot_surface`.
    Snapshot {
        /// Surface copied.
        surface: u32,
        /// Alpha forced to one.
        opaque: bool,
        /// The texture handle returned.
        id: u32,
    },
    /// `snapshot_surface_into`: an earlier copy refilled.
    SnapshotInto {
        /// Surface copied.
        surface: u32,
        /// Alpha forced to one.
        opaque: bool,
        /// The texture refilled (the handle an earlier `Snapshot` returned).
        id: u32,
    },
    /// `write_surface`.
    WriteSurface {
        /// Surface.
        surface: u32,
        /// The bytes written.
        bytes: Vec<u8>,
    },
    /// `present`.
    Present {
        /// Surface shown.
        surface: u32,
        /// Flags.
        flags: u32,
    },
    /// `clear_pixel_count`.
    ClearPixelCount,
    /// `report_pixel_count`.
    ReportPixelCount {
        /// The ticket handed out.
        ticket: u64,
        /// The count it stands for.
        value: u64,
    },
    /// `read_pixel_count`.
    ReadPixelCount {
        /// The ticket.
        ticket: u64,
    },
    /// `run_scene_stage`.
    SceneStage {
        /// The stage.
        stage: timeline::native::Stage,
        /// Its colour and depth surfaces.
        color: u32,
        depth: u32,
        /// The guest items it stands for ([`ExternalPass::items`]).
        items: usize,
        /// The thread that ran it.
        thread: Option<String>,
    },
}

/// Records every call; surfaces and textures are numbers.
#[derive(Default)]
pub struct RecordingBackend {
    /// Calls in order.
    pub calls: Vec<Recorded>,
    next: u32,
    /// The byte `read_surface` returns.
    pub readback_fill: u8,
    /// The DAC ramp passed at the most recent present.
    pub present_gamma: Option<Box<[u8; 768]>>,
    /// What `read_depth` returns (repeated or cut to the image's length);
    /// `None` returns `readback_fill` bytes.
    pub depth_readback: Option<Vec<u8>>,
    /// Samples counted since the last `clear_pixel_count`: each counted
    /// draw adds the area of its clip rectangle (every pixel it may touch
    /// passes; nothing is rasterised here).
    pub pixel_count: u64,
    /// Reports not yet read, by ticket.
    pub pixel_reports: std::collections::HashMap<u64, u64>,
    /// `try_pixel_count` answers every report as not back yet (a GPU
    /// still busy with them); `read_pixel_count` still reads them.
    pub pixel_counts_pending: bool,
    next_ticket: u64,
    /// `copy_surface` answers `Ok(false)` (a backend that cannot copy).
    pub no_copies: bool,
    /// Whether it claims to sample surfaces directly
    /// ([`Backend::can_sample_surfaces`]; default false).
    pub sample_surfaces: bool,
    /// Whether it answers [`Backend::depth_level_texture`] (lazy depth
    /// levels); off by default.
    pub sample_depth: bool,
    /// What [`Backend::surface_extent`] answers, by surface (none listed:
    /// not known).
    pub extents: std::collections::HashMap<u32, (u32, u32)>,
    /// Whether it takes external passes ([`Backend::external_pass`]; off
    /// by default). Taken, a pass is recorded in
    /// [`external`](Self::external), not encoded (there is no GPU).
    pub external_passes: bool,
    /// Synthetic host error during confirmed attachment lookup.
    pub external_prepare_error: Option<String>,
    /// The external passes taken: `(colour, depth)` surfaces, in order.
    pub external: Vec<(u32, u32)>,
    /// Per pass taken, the surfaces it samples ([`ExternalPass::sampled`]).
    pub external_sampled: Vec<Vec<Option<u32>>>,
    /// Per pass taken, how many [`calls`](Self::calls) were recorded
    /// before it: its place in the stream among the D3D path's work.
    pub external_at: Vec<usize>,
    /// Whether it queues independently resolved draws
    /// ([`Backend::queue_external_draw`]) as a threaded backend does; off by
    /// default. Queued, a draw commits at once and is recorded in
    /// [`queued`](Self::queued).
    pub queue_external_draws: bool,
    /// The draws queued: each one's stage and range count, in order.
    pub queued: Vec<(timeline::native::Stage, usize)>,
}

impl RecordingBackend {
    /// An empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Surfaces created and not destroyed.
    pub fn live_surfaces(&self) -> usize {
        let created = self
            .calls
            .iter()
            .filter(|c| matches!(c, Recorded::Surface { .. }))
            .count();
        let destroyed = self
            .calls
            .iter()
            .filter(|c| matches!(c, Recorded::DestroySurface { .. }))
            .count();
        created - destroyed
    }

    /// Number of texture uploads so far.
    pub fn uploads(&self) -> usize {
        self.calls
            .iter()
            .filter(|c| matches!(c, Recorded::Upload { .. }))
            .count()
    }

    /// The recorded draws.
    pub fn draws(&self) -> Vec<&ResolvedDraw> {
        self.calls
            .iter()
            .filter_map(|c| {
                if let Recorded::Draw { draw, .. } = c {
                    Some(&**draw)
                } else {
                    None
                }
            })
            .collect()
    }
}

/// [`RecordingBackend`] draws everything.
struct DrawsEverything;

impl DrawJudge for DrawsEverything {
    fn upload_view(&self, _: &DecodedTexture) -> bool {
        true
    }
    fn surface_view(&self, _: D3DFormat) -> bool {
        true
    }
    fn draw(&mut self, _: &ResolvedDraw, _: [Option<bool>; 4]) -> DrawOutcome {
        DrawOutcome::Drawn
    }
}

impl RecordingBackend {
    /// A handle for a headless draw. It has no sample storage.
    pub fn texture_placeholder_id(&mut self) -> u32 {
        self.next += 1;
        self.next
    }
}

impl Backend for RecordingBackend {
    fn queue_external_draw(
        &mut self, candidate: QueuedExternalDraw<u32>, _textures: [Option<&u32>; 4],
    ) -> bool {
        if !self.queue_external_draws {
            return false;
        }
        self.queued.push((candidate.plan.stage, candidate.plan.ranges));
        (candidate.finished)(None);
        true
    }

    type Surface = u32;
    type Texture = u32;

    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        Some(Box::new(DrawsEverything))
    }

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<u32, String> {
        self.next += 1;
        self.calls.push(Recorded::Surface {
            id: self.next,
            format,
            size: (width, height),
            depth,
        });
        Ok(self.next)
    }
    fn destroy_surface(&mut self, id: u32) -> Result<(), String> {
        let live = self
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::Surface { id: i, .. } if *i == id));
        let gone = self
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::DestroySurface { id: i } if *i == id));
        if !live || gone {
            return Err(format!("surface {id} is not live"));
        }
        self.calls.push(Recorded::DestroySurface { id });
        Ok(())
    }
    fn upload_texture(&mut self, t: &DecodedTexture) -> Result<u32, String> {
        self.next += 1;
        self.calls.push(Recorded::Upload {
            id: self.next,
            size: (t.desc.width, t.desc.height),
        });
        Ok(self.next)
    }
    fn bc_supported(&self) -> bool {
        false
    }
    fn clear(&mut self, color: u32, _depth: Option<u32>, clear: Clear) -> Result<(), String> {
        self.calls.push(Recorded::Clear { color, clear });
        Ok(())
    }
    fn draw(
        &mut self,
        color: u32,
        _depth: Option<u32>,
        draw: &ResolvedDraw,
        textures: [Option<&u32>; 4],
    ) -> Result<DrawOutcome, String> {
        if draw.hw.zpass_count_enable
            && let Some(c) = draw.clip
        {
            // The writable area: the clip, or its pieces when it has them.
            let area = |r: &PixelRect| r.width as u64 * r.height as u64;
            self.pixel_count += if draw.clip_rects.is_empty() {
                area(&c)
            } else {
                draw.clip_rects.iter().map(area).sum()
            };
        }
        self.calls.push(Recorded::Draw {
            color,
            draw: Box::new(draw.clone()),
            textures: textures.map(|t| t.copied()),
        });
        Ok(DrawOutcome::Drawn)
    }
    fn present(&mut self, surface: u32, flags: u32) -> Result<(), String> {
        self.calls.push(Recorded::Present { surface, flags });
        Ok(())
    }
    fn surface_extent(&self, surface: u32) -> Option<(u32, u32)> {
        self.extents.get(&surface).copied()
    }
    fn present_with_gamma(
        &mut self,
        surface: u32,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.present_gamma = gamma.map(|ramp| Box::new(*ramp));
        self.present(surface, flags)
    }
    fn clear_rects(
        &mut self,
        color: u32,
        _depth: Option<u32>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        self.calls.push(Recorded::ClearRects {
            color,
            clear: clear.clone(),
        });
        Ok(DrawOutcome::Drawn)
    }
    fn copy_surface(&mut self, src: u32, dst: u32) -> Result<bool, String> {
        if self.no_copies {
            return Ok(false);
        }
        self.calls.push(Recorded::CopySurface { src, dst });
        Ok(true)
    }
    fn can_sample_surfaces(&self) -> bool {
        self.sample_surfaces
    }
    fn surface_texture(&mut self, surface: u32) -> Result<Option<u32>, String> {
        if !self.sample_surfaces {
            return Ok(None);
        }
        self.next += 1;
        self.calls.push(Recorded::SurfaceTexture {
            surface,
            id: self.next,
        });
        Ok(Some(self.next))
    }
    fn can_sample_depth_surfaces(&self) -> bool {
        self.sample_depth
    }
    fn depth_level_texture(&mut self, surface: u32) -> Result<Option<u32>, String> {
        if !self.sample_depth {
            return Ok(None);
        }
        self.next += 1;
        self.calls.push(Recorded::DepthLevelTexture {
            surface,
            id: self.next,
        });
        Ok(Some(self.next))
    }
    fn snapshot_surface(&mut self, surface: u32, opaque: bool) -> Result<Option<u32>, String> {
        self.next += 1;
        self.calls.push(Recorded::Snapshot {
            surface,
            opaque,
            id: self.next,
        });
        Ok(Some(self.next))
    }
    fn snapshot_surface_into(&mut self, surface: u32, opaque: bool, into: &mut u32) -> Result<bool, String> {
        self.calls.push(Recorded::SnapshotInto {
            surface,
            opaque,
            id: *into,
        });
        Ok(true)
    }
    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.pixel_count = 0;
        self.calls.push(Recorded::ClearPixelCount);
        Ok(())
    }
    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        self.pixel_reports.insert(ticket, self.pixel_count);
        self.calls.push(Recorded::ReportPixelCount {
            ticket,
            value: self.pixel_count,
        });
        Ok(Some(ticket))
    }
    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.calls.push(Recorded::ReadPixelCount { ticket });
        self.pixel_reports
            .remove(&ticket)
            .ok_or_else(|| format!("pixel-count ticket {ticket} is not live"))
    }
    /// Back unless [`pixel_counts_pending`](Self::pixel_counts_pending); a
    /// count handed out is recorded as a read (the ticket is spent).
    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        if !self.pixel_reports.contains_key(&ticket) {
            return Err(format!("pixel-count ticket {ticket} is not live"));
        }
        if self.pixel_counts_pending {
            return Ok(None);
        }
        self.read_pixel_count(ticket).map(Some)
    }
    fn drop_pixel_count(&mut self, ticket: u64) {
        self.pixel_reports.remove(&ticket);
    }
    /// Records the bytes.
    fn write_surface(
        &mut self,
        surface: u32,
        _desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.calls.push(Recorded::WriteSurface {
            surface,
            bytes: bytes.to_vec(),
        });
        Ok(true)
    }
    /// Returns `depth_readback` (cycled to the image's length) or
    /// `readback_fill` bytes.
    fn read_depth(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        let bpp = format
            .bytes_per_texel()
            .ok_or_else(|| format!("{format:?} has no texel size"))?;
        let len = match layout {
            GuestLayoutKind::Swizzled => (width * height * bpp) as usize,
            GuestLayoutKind::Linear { pitch } => (pitch * height) as usize,
        };
        self.calls.push(Recorded::ReadDepth {
            surface,
            format,
            len,
        });
        Ok(Some(match &self.depth_readback {
            Some(b) if !b.is_empty() => b.iter().copied().cycle().take(len).collect(),
            _ => vec![self.readback_fill; len],
        }))
    }
    /// Returns `readback_fill` repeated to the level's size.
    fn read_surface(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        let bpp = format
            .bytes_per_texel()
            .ok_or_else(|| format!("{format:?} has no texel size"))?;
        let len = match layout {
            GuestLayoutKind::Swizzled => (width * height * bpp) as usize,
            GuestLayoutKind::Linear { pitch } => (pitch * height) as usize,
        };
        self.calls.push(Recorded::ReadSurface { surface, len });
        Ok(vec![self.readback_fill; len])
    }

    #[cfg(feature = "gpu")]
    fn external_attachments(
        &mut self, _: ExternalSurfaces<Self::Surface>, _: [Option<&Self::Texture>; 4], copy: TargetCopy,
    ) -> Result<ExternalCopies, String> {
        match &self.external_prepare_error {
            Some(error) => Err(error.clone()),
            None => Ok(ExternalCopies::Missing(copy)),
        }
    }

    fn supports_external_pass(&self) -> bool {
        self.external_passes
    }

    fn runs_scene_stages(&self) -> bool {
        self.external_passes
    }

    /// Records the stage, the items its pass stands for and the thread it
    /// ran on; nothing is encoded (there is no GPU).
    fn run_scene_stage(&mut self, stage: SceneStage<u32>) -> Result<bool, String> {
        if !self.external_passes {
            return Ok(false);
        }
        let items = stage.pass.lock().map_err(|_| "native pass poisoned")?.items(stage.plan.ranges);
        self.calls.push(Recorded::SceneStage {
            stage: stage.plan.stage,
            color: stage.surfaces.color,
            depth: stage.surfaces.depth,
            items,
            thread: std::thread::current().name().map(str::to_owned),
        });
        Ok(true)
    }

    fn external_pass(
        &mut self,
        color: u32,
        depth: u32,
        sampled: Vec<Option<u32>>,
        _request: ExternalPassRequest,
        _pass: Box<dyn ExternalPass>,
    ) -> Result<bool, String> {
        if self.external_passes {
            self.external.push((color, depth));
            self.external_sampled.push(sampled);
            self.external_at.push(self.calls.len());
        }
        Ok(self.external_passes)
    }
}

#[cfg(test)]
mod indexed_tail_tests {
    use super::IndexedTail;
    use crate::{GuestError, GuestMemory, SparseMemory};

    #[test]
    fn indexed_tail_consumes_only_the_matching_live_loop() {
        let mut mem = SparseMemory::new();
        mem.map(0x6000, 4096);
        mem.write_u32(0x6040, 3).unwrap();
        let tail = IndexedTail { indexed_esp: 0x6000, counter: 0x6040, remaining: 3 };
        assert!(!tail.consume(&mut mem, None));
        assert!(!tail.consume(&mut mem, Some(0x6004)));
        assert!(!IndexedTail { remaining: 2, ..tail }.consume(&mut mem, Some(0x6000)));
        assert_eq!(mem.read_u32(0x6040).unwrap(), 3);
        assert!(tail.consume(&mut mem, Some(0x6000)));
        assert_eq!(mem.read_u32(0x6040).unwrap(), 1);
        assert!(!tail.consume(&mut mem, Some(0x6000)));
    }

    #[test]
    fn indexed_tail_refuses_an_unwritable_stack_before_commit() {
        struct ReadOnly;
        impl GuestMemory for ReadOnly {
            fn read(&self, _: u32, out: &mut [u8]) -> Result<(), GuestError> {
                out.copy_from_slice(&5u32.to_le_bytes());
                Ok(())
            }
            fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), GuestError> {
                Err(GuestError { addr, len: data.len() as u32 })
            }
        }
        let tail = IndexedTail { indexed_esp: 40, counter: 80, remaining: 5 };
        assert!(!tail.consume(&mut ReadOnly, Some(40)));
    }
}
