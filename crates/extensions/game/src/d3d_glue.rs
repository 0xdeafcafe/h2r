//! Installs crates/platform/d3d through its own registration (`d3d::register`,
//! docs/graphics/d3d.md "Integration") on a headless RecordingBackend that keeps no
//! history ([`Headless`]), so the boot
//! tracer gets past device creation without a window (or on a headless
//! GfxBackend with `H2_BOOT_GFX`). Uploads are `UploadPolicy::Tracked`,
//! fed by the write barrier below.

use std::sync::{Arc, Mutex};

use d3d::{DeviceConfig, GuestRange, UploadPolicy};
#[cfg(feature = "desktop-runtime")]
use d3d::RecordingBackend;

#[cfg(feature = "gpu")]
mod clip_png;

pub const PUSH_BASE: u32 = 0xa0c0_0000;
pub const PUSH_LEN: u32 = 0x8_0000;
pub const SCRATCH_BASE: u32 = 0xa0c8_0000;

pub struct Installed {
    report: Box<dyn Fn() -> String + Send + Sync>,
    pub implemented: usize,
    pub stopping: usize,
    pub native: usize,
    /// The vblank callback on the 60 Hz logical clock, for
    /// `Kernel::add_periodic` (taken once).
    pub vblank: Option<kernel::periodic::Handler>,
    /// Present only when this backend draws an image.
    #[cfg(feature = "gpu")]
    pub view: Option<DrawingView>,
    /// Keeps the sealed-scene render worker alive for this installation.
    #[cfg(feature = "gpu")]
    _private_scenes: Option<crate::frame_owner::Service>,
    /// The device, for machine snapshots (`crate::snapshot_glue`).
    pub snapshot: Arc<dyn DeviceSnapshot>,
    #[cfg(feature = "gpu")]
    pub ui_layout: Arc<dyn crate::render_aspect::UiLayoutSink>,
}

/// The installed GPU view. A plain backend draws at guest resolution. A
/// scaled backend publishes its effective scale when it presents.
#[cfg(feature = "gpu")]
pub struct DrawingView {
    pub scale_stats: Option<Arc<Mutex<d3d::scaled::ScaleStats>>>,
}

#[cfg(feature = "gpu")]
impl Installed {
    pub fn with_gpu_view(mut self, scale_stats: Option<Arc<Mutex<d3d::scaled::ScaleStats>>>) -> Self {
        self.view = Some(DrawingView { scale_stats });
        self
    }
}

/// The device as a machine snapshot sees it, whatever backend it was made
/// with (docs/runtime/snapshots.md, phase 4: `Device::snapshot_blockers`,
/// `save_snapshot`, `load_snapshot`).
pub trait DeviceSnapshot: Send + Sync {
    #[cfg(feature = "gpu")]
    fn set_mark_image_source(&self, _source: fn() -> Option<d3d::present_camera::Image>) {}
    #[cfg(feature = "gpu")]
    fn mark_projection_factor(&self) -> f32 { 1.0 }
    /// What keeps the device from being saved now.
    fn blockers(&self) -> Vec<String>;
    /// Pending device operations which prevent an isolated film restore.
    fn film_load_blockers(&self) -> Vec<String> {
        self.blockers()
    }
    /// The `d3d` section. Writes guest memory (render targets written
    /// back), so it runs before guest RAM is saved.
    fn save(&self, rt: &runtime::Runtime, out: &mut snapshot::SnapshotWriter)
    -> Result<(), String>;
    /// A film keyframe includes the parameters needed to create a cold device.
    fn save_film(&self, rt: &runtime::Runtime, out: &mut snapshot::SnapshotWriter) -> Result<(), String> {
        self.save(rt, out)
    }
    /// After guest RAM is loaded.
    fn load(&self, snapshot: &snapshot::Snapshot<'_>) -> Result<(), String>;
    /// Recreate host resources from a cold film keyframe after guest RAM loads.
    fn load_film(&self, _rt: &runtime::Runtime, snapshot: &snapshot::Snapshot<'_>) -> Result<(), String> {
        self.load(snapshot)
    }
}

impl<B: d3d::backend::Backend + Send + 'static> DeviceSnapshot for Mutex<d3d::Device<B>> {
    #[cfg(feature = "gpu")]
    fn set_mark_image_source(&self, source: fn() -> Option<d3d::present_camera::Image>) {
        self.lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_mark_image_source(source);
    }

    #[cfg(feature = "gpu")]
    fn mark_projection_factor(&self) -> f32 {
        self.lock()
            .unwrap_or_else(|e| e.into_inner())
            .projection_factor()
    }

    fn blockers(&self) -> Vec<String> {
        self.lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot_blockers()
    }
    fn film_load_blockers(&self) -> Vec<String> {
        self.lock().unwrap_or_else(|e| e.into_inner()).film_load_blockers()
    }
    fn save(
        &self,
        rt: &runtime::Runtime,
        out: &mut snapshot::SnapshotWriter,
    ) -> Result<(), String> {
        let mut device = self.lock().unwrap_or_else(|e| e.into_inner());
        let mut guest = rt.memory();
        let mut mem = d3d::runtime_glue::GuestAdapter::new(&mut guest);
        device
            .save_snapshot(&mut mem, out)
            .map_err(|e| e.to_string())
    }
    fn save_film(
        &self,
        rt: &runtime::Runtime,
        out: &mut snapshot::SnapshotWriter,
    ) -> Result<(), String> {
        let mut device = self.lock().unwrap_or_else(|e| e.into_inner());
        let mut guest = rt.memory();
        let mut mem = d3d::runtime_glue::GuestAdapter::new(&mut guest);
        device.save_film_snapshot(&mut mem, out).map_err(|e| e.to_string())
    }
    fn load(&self, snapshot: &snapshot::Snapshot<'_>) -> Result<(), String> {
        let mut device = self.lock().unwrap_or_else(|e| e.into_inner());
        device.load_snapshot(snapshot).map_err(|e| e.to_string())
    }
    fn load_film(&self, rt: &runtime::Runtime, snapshot: &snapshot::Snapshot<'_>) -> Result<(), String> {
        let mut device = self.lock().unwrap_or_else(|e| e.into_inner());
        let mut guest = rt.memory();
        device.load_film_snapshot(snapshot, &d3d::runtime_glue::GuestAdapter::new(&mut guest))
            .map_err(|e| e.to_string())
    }
}

/// `d3d::register` on a headless `RecordingBackend`, or, with
/// `H2_BOOT_GFX=<dir>`, on a headless `GfxBackend` with the nv2a shader
/// translator; then every `Swap` that shows a new image (at most one per
/// `H2_BOOT_GFX_EVERY` swaps, default 30) is written to `<dir>` as
/// `frame-NNNNN.png`. With `H2_RENDER_SCALE=N` (2..6) the frames come
/// from a view backend at N times the guest's size (`d3d::ScaledBackend`,
/// docs/graphics/high-resolution.md), `frame-NNNNN.png` is the view's image and
/// exact two-copy mode also writes `frame-NNNNN-1x.png`. The run report gives the
/// view's counts. `H2_LIGHTING=e3` (read by `d3d::scaled::lighting`, this
/// capture path only) gives the view E3 lighting's scaled,
/// one-tap shadow maps (docs/graphics/e3-lighting-setting.md), with a view at 1x
/// too when the scale is 1. `H2_BOOT_GFX_LIGHTING_AB=1` draws a retail and
/// an E3 view of the same run (`d3d::LightingAb`, at least 2x), compares
/// them on every shadow-map frame and writes differing pairs as
/// `frame-NNNNN-retail.png` and `frame-NNNNN-e3.png`. `H2_BOOT_GFX_SHADOWS=N` also writes the first
/// N frames that sample a shadow map (`d3d_diag::samples_shadow_map`).
/// Halo's frame wait goes through `d3d::register_frame_wait` either way.
pub fn install(rt: &runtime::Runtime) -> anyhow::Result<Installed> {
    #[cfg(feature = "gpu")]
    {
        install_headless(rt, None)
    }
    #[cfg(all(not(feature = "gpu"), feature = "desktop-runtime"))]
    {
        install_headless_cpu_for_profile(rt, crate::run_plan::compiled_render_hooks())
    }
    #[cfg(not(feature = "desktop-runtime"))]
    {
        install_light(rt)
    }
}

#[cfg(all(not(feature = "gpu"), feature = "desktop-runtime"))]
pub fn install_headless_cpu_for_profile(
    rt: &runtime::Runtime,
    profile: film::RenderHooks,
) -> anyhow::Result<Installed> {
    install_for_profile(rt, Headless::default(), None, profile)
}

/// No display backend. Leftover lobby and loading draws carry texture
/// descriptors and handles through the ABI without copying the pixels.
pub fn install_light(rt: &runtime::Runtime) -> anyhow::Result<Installed> {
    install_light_for_profile(rt, crate::run_plan::compiled_render_hooks())
}

pub fn install_light_for_profile(
    rt: &runtime::Runtime,
    profile: film::RenderHooks,
) -> anyhow::Result<Installed> {
    let backend = d3d::descriptor::DescriptorBackend::default();
    #[cfg(not(feature = "desktop-runtime"))]
    let backend = backend.with_present(crate::d3d_diag::note_descriptor_present);
    install_for_profile(rt, backend, None, profile)
}

#[cfg(feature = "gpu")]
/// [`install`], with the frame rate `fps` ([`frame_rate`]) for the frame
/// owner's display-rate loop on the headless GPU backend
/// (docs/graphics/framescene.md, section 7). A paced owner draws display
/// frames between scenes on the wall clock and shows each through the
/// backend's replay hook, which counts no guest swap. With
/// `H2_BOOT_GFX_BETWEEN=N` the first N of them are written as
/// `frame-NNNNN-between-KK.png` (KK-th after guest swap NNNNN), and the
/// guest frames shown meanwhile as `frame-NNNNN.png` ([`BetweenShots`]).
/// Without `H2_BOOT_GFX` (nothing is drawn), under the lighting A/B and
/// for a locked owner (every scene drawn once), `fps` is ignored.
pub fn install_headless(
    rt: &runtime::Runtime,
    fps: Option<FrameRate>,
) -> anyhow::Result<Installed> {
    install_headless_for_profile(rt, fps, crate::run_plan::compiled_render_hooks(), None, false)
}

#[cfg(feature = "gpu")]
/// Boot selects the presenting backend after network and film registration.
pub fn window_targets(
    _store: &settings::Store,
    native_target: crate::run_opts::NativeTarget,
    display_size: impl Fn((u32, u32)) -> (u32, u32) + Send + Sync + 'static,
    reference: impl FnOnce() -> d3d::GfxBackend,
    display: impl FnOnce() -> d3d::GfxBackend,
) -> (d3d::GfxBackend, crate::run_opts::DisplayView) {
    let (main, backend) = (reference(), Some(display()));
    (main, crate::run_opts::DisplayView { backend, native_target, viewer_output: None, viewer_lod: None, display_size: Arc::new(display_size) })
}

/// The private BSP review's lamp arm (docs/graphics/relighting-review.md):
/// `H2_LIGHTING=relight-on` or `relight-off` at launch. No setting holds
/// it and nothing changes it during a run.
#[cfg(feature = "gpu")]
pub fn review_lighting() -> Option<d3d::Lighting> {
    std::env::var("H2_LIGHTING").ok().as_deref().and_then(d3d::Lighting::relight_from_name)
}

/// A locked retail target has no live raster edits. It can present directly
/// when the run permits single-copy rendering.
#[cfg(feature = "gpu")]
pub fn locked_retail_target(store: Option<&settings::Store>) -> bool {
    let Some(store) = store else { return false };
    use crate::setting_defs::{GUEST_FRAME, RESOLUTION};
    let fixed = settings::Resolution::Fixed(GUEST_FRAME.0, GUEST_FRAME.1);
    let locked = |key| store.state(key).is_some_and(|state| state.locked);
    store.handle::<settings::Resolution>(RESOLUTION.key).get() == fixed
        && locked(RESOLUTION.key)
        && review_lighting().is_none()
}

#[cfg(feature = "gpu")]
mod output_size;
#[cfg(feature = "gpu")]
pub use output_size::output_size;

#[cfg(feature = "gpu")]
pub fn protected_readbacks(network: bool, film: bool, lockstep: bool) -> bool {
    // XNet sockets and film taps are installed before entry. A match may
    // start later on that same device; earlier retail pixels must already exist.
    network || film || lockstep
}

#[cfg(feature = "gpu")]
pub fn single_copy(store: Option<&settings::Store>, protected: bool) -> anyhow::Result<bool> {
    if protected {
        return Ok(false);
    }
    // Relit pixels must never feed guest readbacks, including in offline play.
    if review_lighting().is_some() {
        return Ok(false);
    }
    let def = &crate::setting_defs::SINGLE_COPY;
    let value = match store {
        Some(store) => store.get(def.key).unwrap_or(def.default),
        None => match std::env::var("H2_SINGLE_COPY") {
            Ok(raw) => def.parse_env_text(&raw).map_err(anyhow::Error::msg)?,
            Err(_) => def.default,
        },
    };
    Ok(value == settings::Value::Bool(true))
}

/// A private-target renderer for sealed game frames. Its device and queue
/// are shared with the displayed backend. It has its own targets and never
/// invokes the window or headless screenshot hook.
#[cfg(feature = "gpu")]
#[derive(Clone, Copy)]
pub struct PrivateScenes {
    mode: frame_scene::mailbox::Mode,
}

#[cfg(feature = "gpu")]
impl PrivateScenes {
    pub fn requested(switches: &hooks::Switches, profile: film::RenderHooks, locked: bool) -> Option<Self> {
        // The original census has to measure all guest-visible readbacks.
        // A paced display owns its frames by default: its display-rate
        // loop is what 60 and 120 fps draw. A locked run (headless,
        // unpaced, stop-at-present) asks for it with `frame-scene.sink`.
        let record = profile == film::RenderHooks::Gpu
            && switches.feature(render_bridge::carried_slots::ID, !locked)
            && !switches.feature(render_bridge::entry_census::ID, false)
            && hooks::switches::native_render_on(switches)
            && switches.feature(render_bridge::scene_step::ID, true);
        use frame_scene::mailbox::Mode;
        record.then_some(Self { mode: if locked { Mode::Locked } else { Mode::Paced } })
    }

    /// The display-rate loop for this owner at `fps`
    /// (docs/graphics/framescene.md, section 7). A locked owner draws every
    /// scene once and has none.
    fn display(self, fps: Option<FrameRate>) -> Option<native_render::frame_graph::display_rate::Display> {
        let fps = fps?;
        (self.mode == frame_scene::mailbox::Mode::Paced)
            .then(|| native_render::frame_graph::display_rate::Display::new(fps))
    }

    /// Taps the ordinary display backend for an armed parity comparison.
    /// Captures draw independently on the worker and never call this tap.
    fn install(
        self,
        display: &mut d3d::GfxBackend,
    ) -> anyhow::Result<crate::frame_owner::Service> {
        let worker = crate::frame_owner::Service::new(
            d3d::GfxBackend::new(share_context(&display.ctx)),
            self.mode,
        )?;
        display.observe_guest_present(worker.reference_observer());
        display.observe_unshown_guest_present(worker.unshown_observer());
        Ok(worker)
    }
}

#[cfg(feature = "gpu")]
pub fn install_headless_for_profile(
    rt: &runtime::Runtime,
    fps: Option<FrameRate>,
    profile: film::RenderHooks,
    settings: Option<&settings::Store>,
    protected_readbacks: bool,
) -> anyhow::Result<Installed> {
    let switches = hooks::Switches::from_env().map_err(anyhow::Error::msg)?;
    let private = PrivateScenes::requested(&switches, profile, true);
    install_headless_with_scenes(rt, fps, profile, settings, protected_readbacks, private)
}

#[cfg(feature = "gpu")]
pub fn install_headless_with_scenes(
    rt: &runtime::Runtime,
    fps: Option<FrameRate>,
    profile: film::RenderHooks,
    settings: Option<&settings::Store>,
    protected_readbacks: bool,
    private: Option<PrivateScenes>,
) -> anyhow::Result<Installed> {
    match std::env::var_os("H2_BOOT_GFX") {
        None => install_for_profile(rt, Headless::default(), None, profile),
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir)?;
            // `never`: render on the GPU but write no screenshots (benchmarks).
            let every: u64 = match std::env::var("H2_BOOT_GFX_EVERY").ok().as_deref() {
                Some("never") => u64::MAX,
                v => v
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| n > 0)
                    .unwrap_or(30),
            };
            let native = Arc::new(|| settings::NativeTarget::windowed(crate::setting_defs::GUEST_FRAME));
            let (width, height) = output_size(settings, native)?();
            let scale = d3d::scaled::RenderScale::for_output(width, height).map_err(anyhow::Error::msg)?;
            let lighting = d3d::scaled::lighting().map_err(anyhow::Error::msg)?;
            let ctx = gfx::GfxContext::headless()
                .map_err(|e| anyhow::anyhow!("gfx headless context: {e}"))?;
            let clip_size = std::env::var_os("H2_BOOT_GFX_SIZE")
                .map(|raw| {
                    clip_png::parse_size(
                        raw.to_str().unwrap_or_default(),
                        ctx.device.limits().max_texture_dimension_2d,
                    ).map_err(anyhow::Error::msg)
                })
                .transpose()?;
            let clip = Arc::new(Mutex::new(clip_png::ClipPng::new(clip_size)));
            // `H2_BOOT_GFX_SHADOWS=N`: also the first N frames that sample
            // a shadow map (`d3d_diag::samples_shadow_map`, the same frames
            // in every run of the same input) and show something, whatever
            // `every` says. A cinematic draws its lights' shadow passes
            // under a fade to black too; those frames are not written.
            let shadow_shots: u64 = std::env::var("H2_BOOT_GFX_SHADOWS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            // `H2_BOOT_GFX_BETWEEN=N`: the first N display frames between
            // scenes (with a frame rate above 30), and the guest frames
            // among them.
            let between = BetweenShots::new(
                std::env::var("H2_BOOT_GFX_BETWEEN")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .filter(|_| fps.is_some())
                    .unwrap_or(0),
            );
            // `shadow_limit`: how many shadow-map frames it may add.
            // `between`: the in-between frames' shared state, for the
            // backend that shows them.
            let shots = |suffix: &'static str,
                         shadow_limit: u64,
                         between: Option<Arc<BetweenShots>>| {
                let dir = dir.clone();
                let clip = Arc::clone(&clip);
                let mut swaps = 0u64;
                let mut shadows = 0u64;
                // The display's frames (`frame-N.png`) are what the agent's
                // `render.capture` writes: swaps count guest presents as
                // `d3d_diag` numbers them.
                let display = suffix.is_empty();
                if display {
                    agent::render::CAPTURE.set_writer();
                }
                move |ctx: &gfx::GfxContext,
                      store: &gfx::surface::SurfaceStore,
                      id,
                      _flags,
                      _gamma: Option<&[u8; 768]>| {
                    swaps += 1;
                    if display {
                        clip.lock().unwrap().guest(swaps, ctx, store, id);
                    }
                    if display && let Some(write) = agent::render::CAPTURE.wanted(swaps) {
                        let r = save_agent_capture(ctx, store, id, &write)
                            .map_err(|e| e.to_string());
                        agent::render::CAPTURE.written(swaps, r);
                    }
                    // An in-between frame's neighbours: this frame after
                    // one now; before one later, from a GPU copy.
                    let around = match between.as_ref().map(|b| (b, b.guest_swap(swaps))) {
                        Some((b, shot)) => {
                            if shot.keep {
                                b.keep(ctx, store, id);
                            }
                            shot.write
                        }
                        None => false,
                    };
                    let regular =
                        (swaps == 1 && every != u64::MAX) || swaps.is_multiple_of(every) || around;
                    let shadow = !regular
                        && shadows < shadow_limit
                        && crate::d3d_diag::samples_shadow_map(swaps);
                    if !regular && !shadow {
                        return;
                    }
                    let path = dir.join(format!("frame-{swaps:05}{suffix}.png"));
                    match save_png(ctx, store, id, &path, shadow) {
                        Ok(written) => shadows += u64::from(shadow && written),
                        Err(e) => eprintln!("boot: gfx screenshot {swaps}: {e}"),
                    }
                }
            };
            let mut between_hook = between.hook(dir.clone(), "");
            let replay_clip = Arc::clone(&clip);
            let replay = move |ctx: &gfx::GfxContext,
                               store: &gfx::surface::SurfaceStore,
                               id,
                               flags,
                               gamma: Option<&[u8; 768]>| {
                replay_clip.lock().unwrap().replay(ctx, store, id);
                between_hook(ctx, store, id, flags, gamma);
            };
            if std::env::var_os("H2_BOOT_GFX_LIGHTING_AB").is_some_and(|v| v != "0") {
                // One run, both lighting modes: the 1x reference answers the
                // guest; a retail view and an E3 view draw the same calls at
                // `scale` (`d3d::LightingAb`). `frame-N-retail.png` and
                // `frame-N-e3.png` are a pair from the same frame.
                let (width, height) = if (width, height) == crate::setting_defs::GUEST_FRAME {
                    (1280, 960)
                } else {
                    (width, height)
                };
                let scale = d3d::scaled::RenderScale::for_output(width, height).map_err(anyhow::Error::msg)?;
                let reference = configure_gfx(
                    d3d::GfxBackend::new(share_context(&ctx)).on_present(shots("-1x", 0, None)),
                );
                // Every shadow-map frame is compared between the views; the
                // first `H2_BOOT_GFX_SHADOWS` (default 40) that differ are
                // written as pairs. `H2_BOOT_GFX_EVERY` writes both views'
                // frames as usual.
                let ab = Arc::new(Mutex::new(AbCompare::default()));
                let pairs = if shadow_shots == 0 { 40 } else { shadow_shots };
                let mut retail_shots = shots("-retail", 0, None);
                let held = Arc::clone(&ab);
                let mut retail_swaps = 0u64;
                let retail_hook =
                    move |ctx: &gfx::GfxContext,
                          store: &gfx::surface::SurfaceStore,
                          id,
                          flags,
                          gamma: Option<&[u8; 768]>| {
                        retail_swaps += 1;
                        if crate::d3d_diag::samples_shadow_map(retail_swaps) {
                            match read_rgba(ctx, store, id) {
                                Ok((px, _, _)) => {
                                    held.lock().unwrap().pending = Some((retail_swaps, px))
                                }
                                Err(e) => eprintln!("boot: lighting A/B {retail_swaps}: {e}"),
                            }
                        }
                        retail_shots(ctx, store, id, flags, gamma);
                    };
                let mut e3_shots = shots("-e3", 0, None);
                let held = Arc::clone(&ab);
                let (dir_ab, mut e3_swaps, mut written) = (dir.clone(), 0u64, 0u64);
                let e3_hook = move |ctx: &gfx::GfxContext,
                                    store: &gfx::surface::SurfaceStore,
                                    id,
                                    flags,
                                    gamma: Option<&[u8; 768]>| {
                    e3_swaps += 1;
                    if crate::d3d_diag::samples_shadow_map(e3_swaps) {
                        let result = read_rgba(ctx, store, id).and_then(|(px, w, h)| {
                            let retail = held.lock().unwrap().compare(e3_swaps, &px);
                            if let Some(retail) = retail.filter(|_| written < pairs) {
                                written += 1;
                                write_png(
                                    &dir_ab.join(format!("frame-{e3_swaps:05}-retail.png")),
                                    &retail,
                                    w,
                                    h,
                                )?;
                                write_png(
                                    &dir_ab.join(format!("frame-{e3_swaps:05}-e3.png")),
                                    &px,
                                    w,
                                    h,
                                )?;
                            }
                            Ok(())
                        });
                        if let Err(e) = result {
                            eprintln!("boot: lighting A/B {e3_swaps}: {e}");
                        }
                    }
                    e3_shots(ctx, store, id, flags, gamma);
                };
                let retail = configure_gfx(
                    d3d::GfxBackend::new(share_context(&ctx)).on_present(retail_hook),
                );
                let e3 = configure_gfx(d3d::GfxBackend::new(ctx).on_present(e3_hook));
                if fps.is_some() {
                    eprintln!(
                        "d3d: display frames between scenes are off under the lighting A/B (H2_BOOT_GFX_LIGHTING_AB)"
                    );
                }
                let mut b =
                    d3d::scaled::lighting_ab_at_resolution(reference, retail, e3, scale).map_err(anyhow::Error::msg)?;
                eprintln!(
                    "d3d: lighting A/B at {width}x{height} (H2_BOOT_GFX_LIGHTING_AB): a retail and an E3 view of one run; the guest sees a 1x reference"
                );
                let scale_stats = b.shared_stats();
                let (retail_report, e3_report) =
                    (scale_report(b.reference_mut()), scale_report(&mut b));
                let report: ExtraReport = Box::new(move || {
                    format!(
                        "retail view: {}\ne3 view: {}\n{}",
                        retail_report(),
                        e3_report(),
                        ab.lock().unwrap().report()
                    )
                });
                let installed = if render_thread_requested() {
                    install_for_profile(rt, d3d::ThreadedBackend::new(b)?, Some(report), profile)
                } else {
                    install_for_profile(rt, b, Some(report), profile)
                }?;
                return Ok(installed.with_gpu_view(Some(scale_stats)));
            }
            agent::render::clip::CLIPS.set_writer(dir.canonicalize()?, 1);
            if (width, height) != crate::setting_defs::GUEST_FRAME || lighting != d3d::Lighting::Retail {
                // The display presents scaled frames. Exact reference captures
                // use their own hook and swap count on the shared device.
                let reference = configure_gfx(
                    d3d::GfxBackend::new(share_context(&ctx))
                        .on_present(shots("-1x", shadow_shots, None))
                        .on_present_replay(|_, _, _, _, _| {}),
                );
                let view = configure_gfx(
                    d3d::GfxBackend::new(ctx)
                        .on_present(shots("", shadow_shots, Some(Arc::clone(&between))))
                        .on_present_replay(replay),
                );
                let single = single_copy(settings, protected_readbacks)?;
                let mut b =
                    d3d::ScaledBackend::with_resolution(reference, view, scale, single).map_err(anyhow::Error::msg)?;
                b.set_lighting(lighting);
                eprintln!(
                    "d3d: rendering at {width}x{height}, {:?} lighting; single copy {single}",
                    b.lighting()
                );
                let thread = RenderThread::requested(false);
                return install_scaled_view(rt, b, fps, thread, profile, private);
            }
            let mut backend = configure_gfx(
                d3d::GfxBackend::new(ctx)
                    .on_present(shots("", shadow_shots, Some(Arc::clone(&between))))
                    .on_present_replay(replay),
            );
            if !protected_readbacks {
                crate::native_shadows_glue::configure_single(&mut backend);
            }
            // Backend work (shader preparation, encoding, uploads,
            // submission, present) on a render thread of its own
            // (docs/graphics/render-thread.md), unless H2_RENDER_THREAD=0.
            let thread = RenderThread::requested(false);
            install_game_view(rt, backend, fps, thread, profile, private)
        }
    }
}

#[cfg(feature = "gpu")]
/// `H2_BOOT_GFX_BETWEEN`'s captures: the in-between frames a headless
/// run's replay hook is handed, numbered after the guest swap before them,
/// until `limit` are written, and each one's neighbours: the guest frame
/// shown before it and the one shown after it (as shown: a blended present
/// is a made frame). So at most `limit` in-between frames and `2 * limit`
/// guest frames are written, however rarely in-between frames come.
///
/// The guest frame before is only known to be wanted once an in-between
/// frame follows it, so while captures are left each guest frame is copied
/// on the GPU ([`gfx::surface::SurfaceStore::keep_color`], no read back) and
/// that copy is read back only then. (The first version wrote every guest
/// frame from the first in-between frame on until `limit` in-between
/// frames were written: a run that made one and then held, or whose
/// readbacks pushed every frame past the gap, wrote thousands.)
pub struct BetweenShots {
    limit: u64,
    state: Mutex<BetweenState>,
    /// The last guest frame, copied while captures are left.
    kept: Mutex<Option<gfx::surface::KeptImage>>,
}

#[cfg(feature = "gpu")]
#[derive(Default)]
struct BetweenState {
    /// The last guest swap's number (the present hook's count).
    swap: u64,
    /// In-between frames written so far, and since the last guest swap.
    written: u64,
    since: u64,
    /// The last guest swap written (directly or from its copy).
    shown: Option<u64>,
}

#[cfg(feature = "gpu")]
/// What the present hook does with guest swap `n` ([`BetweenShots::guest_swap`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestShot {
    /// Write it now: an in-between frame came before it.
    pub write: bool,
    /// Copy it on the GPU: an in-between frame may come after it.
    pub keep: bool,
}

#[cfg(feature = "gpu")]
/// What the replay hook writes for an in-between frame ([`BetweenShots::next_between`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BetweenShot {
    /// The in-between frame's file name.
    pub name: String,
    /// The guest frame before it, from its copy, when not written yet.
    pub before: Option<String>,
}

#[cfg(feature = "gpu")]
impl BetweenShots {
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            state: Mutex::default(),
            kept: Mutex::new(None),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, BetweenState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The present hook's guest swap `n`: written when the frame before it
    /// was followed by an in-between frame (its neighbour after), kept on
    /// the GPU while in-between frames are left to write.
    pub fn guest_swap(&self, n: u64) -> GuestShot {
        let mut s = self.state();
        let write = s.since > 0 && s.shown != Some(n);
        s.swap = n;
        s.since = 0;
        if write {
            s.shown = Some(n);
        }
        GuestShot {
            write,
            keep: !write && s.written < self.limit,
        }
    }

    /// The next in-between frame's file name (`suffix` after the swap
    /// number) and the guest frame before it when that is not written yet,
    /// or `None` when `limit` are written.
    pub fn next_between(&self, suffix: &str) -> Option<BetweenShot> {
        let mut s = self.state();
        if s.written >= self.limit {
            return None;
        }
        s.written += 1;
        s.since += 1;
        let swap = s.swap;
        let before =
            (swap > 0 && s.shown != Some(swap)).then(|| format!("frame-{swap:05}{suffix}.png"));
        s.shown = Some(swap);
        Some(BetweenShot {
            name: format!("frame-{swap:05}{suffix}-between-{:02}.png", s.since),
            before,
        })
    }

    /// Copies guest frame `id` on the GPU, for [`Self::next_between`]'s
    /// `before`.
    fn keep(
        &self,
        ctx: &gfx::GfxContext,
        store: &gfx::surface::SurfaceStore,
        id: gfx::surface::SurfaceId,
    ) {
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = store.keep_color(ctx, id, &mut kept) {
            *kept = None;
            eprintln!("boot: gfx in-between neighbour copy: {e:?}");
        }
    }

    /// Writes the kept guest frame as `path`.
    fn write_kept(&self, ctx: &gfx::GfxContext, path: &std::path::Path) -> anyhow::Result<()> {
        let kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        let k = kept
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no copy of the guest frame"))?;
        let (mut px, order, (w, h)) = k.read(ctx);
        opaque_rgba(&mut px, order);
        write_png(path, &px, w, h)
    }

    /// The replay hook: writes the next in-between frame, and the guest
    /// frame before it, while any are left; otherwise does nothing (no
    /// read back).
    fn hook(
        self: &Arc<Self>,
        dir: std::path::PathBuf,
        suffix: &'static str,
    ) -> impl FnMut(
        &gfx::GfxContext,
        &gfx::surface::SurfaceStore,
        gfx::surface::SurfaceId,
        u32,
        Option<&[u8; 768]>,
    ) + Send
    + 'static {
        let me = Arc::clone(self);
        move |ctx, store, id, _flags, _gamma| {
            let Some(shot) = me.next_between(suffix) else {
                return;
            };
            if let Some(before) = &shot.before
                && let Err(e) = me.write_kept(ctx, &dir.join(before))
            {
                eprintln!("boot: gfx in-between neighbour {before}: {e}");
            }
            if let Err(e) = save_png(ctx, store, id, &dir.join(&shot.name), false) {
                eprintln!("boot: gfx in-between screenshot {}: {e}", shot.name);
            }
        }
    }
}

#[cfg(feature = "gpu")]
/// `h2 run`'s settings for a headless `GfxBackend`.
fn configure_gfx(mut backend: d3d::GfxBackend) -> d3d::GfxBackend {
    // Merged render passes under the loose rule are the default (a few
    // edge pixels can differ from one render pass per draw).
    // H2_GFX_MERGE_CHECK=N: compare merged and per-draw passes in
    // frame N and print what differs.
    backend.set_merge_check(
        std::env::var("H2_GFX_MERGE_CHECK")
            .ok()
            .and_then(|v| v.parse().ok()),
    );
    // The persistent pipeline cache (H2_PIPELINE_CACHE, default in
    // the user's cache directory): prewarm last run's pipelines.
    match d3d::pipeline_store::default_path() {
        Some(path) => backend.with_pipeline_cache(path),
        None => backend,
    }
}

/// An offscreen retail GPU reference for local film verification. Its raster
/// stays at the guest dimensions and it never presents to a window.
#[cfg(feature = "gpu")]
pub fn offscreen_film_reference() -> anyhow::Result<d3d::GfxBackend> {
    let ctx = gfx::GfxContext::headless().map_err(|e| anyhow::anyhow!("offscreen film GPU: {e}"))?;
    Ok(configure_gfx(d3d::GfxBackend::new(ctx)))
}

#[cfg(feature = "gpu")]
/// Another handle to `ctx`'s device and queue, for a second backend on it.
pub fn share_context(ctx: &gfx::GfxContext) -> gfx::GfxContext {
    gfx::GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

#[cfg(feature = "gpu")]
/// The run report's line for a scaled backend: what the view did
/// differently, as of the last present (`d3d::scaled::ScaleStats`).
pub fn scale_report<R, V>(b: &mut d3d::ScaledBackend<R, V>) -> ExtraReport
where
    R: d3d::backend::Backend,
    V: d3d::scaled::ScaleView<R>,
{
    let stats = b.shared_stats();
    Box::new(move || {
        let s = *stats.lock().unwrap_or_else(|e| e.into_inner());
        format!("render resolution {}x{}: {s}", s.output.0, s.output.1)
    })
}

/// An extra line for [`describe`].
pub type ExtraReport = Box<dyn Fn() -> String + Send + Sync>;

/// The GPU backend's work runs on a render thread (`h2 run` with
/// `H2_BOOT_GFX`, and `h2 play`) unless `H2_RENDER_THREAD=0`. On by
/// default (docs/graphics/render-thread.md, "Measurements").
pub fn render_thread_requested() -> bool {
    cfg!(feature = "gpu")
        && render_thread_from(std::env::var("H2_RENDER_THREAD").ok().as_deref())
}

/// What frame stats say when the run has no GPU backend: `h2 run` without
/// `H2_BOOT_GFX` (or a window) draws into [`Headless`], which renders
/// nothing, so the counters the GPU backend keeps (draws, submits,
/// pipelines, shaders, uploads, passes, queue writes, the `pass_*`
/// columns) stay 0 in every CSV row. The guest's own draw calls are in the
/// exit `d3d:` stats line either way. `None` when frame stats are off or a
/// GPU backend draws.
pub fn frame_stats_gpu_note(frame_stats: bool, gpu: bool) -> Option<&'static str> {
    (frame_stats && !gpu).then_some(
        "frame stats: no GPU backend in this run (headless: no H2_BOOT_GFX), so draws, submits, pipelines, shaders, uploads, passes, queue_writes and pass_* stay 0; H2_BOOT_GFX=<dir> H2_BOOT_GFX_EVERY=never renders on the GPU without screenshots",
    )
}

#[cfg(feature = "gpu")]
/// The frame-rate setting as a display rate, read at every present.
pub type FrameRate = Box<dyn Fn() -> frame_scene::pace::Rate + Send>;

#[cfg(feature = "gpu")]
/// The Frame Rate row (`graphics.fps`, `H2_FPS`, `--fps`) for the frame
/// owner's display-rate loop (docs/graphics/framescene.md, section 7): above
/// 30 a paced owner draws display frames between scenes; 30 draws each
/// scene once. The setting is read live.
pub fn frame_rate(settings: Option<&std::sync::Arc<settings::Store>>) -> FrameRate {
    use frame_scene::pace::Rate;
    match settings {
        Some(store) => {
            let fps = store.handle::<u32>(crate::setting_defs::FPS.key);
            Box::new(move || Rate::from_setting(fps.get()))
        }
        None => {
            let fps = fps_from(std::env::var("H2_FPS").ok().as_deref());
            Box::new(move || Rate::from_setting(fps))
        }
    }
}

#[cfg(feature = "gpu")]
/// `H2_FPS` as the setting stores it: 0 for uncapped, 30 when unset or
/// unreadable.
pub fn fps_from(value: Option<&str>) -> u32 {
    match value.map(str::trim) {
        Some("uncapped" | "unlimited" | "max") => 0,
        Some(v) => v.parse().unwrap_or(30),
        None => 30,
    }
}

#[cfg(feature = "gpu")]
pub fn install_threaded_for_profile<B>(
    rt: &runtime::Runtime,
    backend: B,
    extra: Option<ExtraReport>,
    profile: film::RenderHooks,
) -> anyhow::Result<Installed>
where
    B: d3d::backend::Backend + Send + 'static,
    B::Texture: Sync,
{
    install_gpu_for_profile(rt, backend, extra, Some(RenderThread::default()), profile, None)
}

/// How a game view's backend runs on its render thread.
#[cfg(feature = "gpu")]
#[derive(Clone, Copy, Default)]
pub struct RenderThread {
    /// Each present completes before the guest's next frame starts.
    pub synchronous_presents: bool,
}

#[cfg(feature = "gpu")]
impl RenderThread {
    /// `None` when `H2_RENDER_THREAD=0` keeps the backend on the guest thread.
    pub fn requested(synchronous_presents: bool) -> Option<Self> {
        render_thread_requested().then_some(Self { synchronous_presents })
    }
}

/// The sealed-frame renderer and its transaction on the display device.
#[cfg(feature = "gpu")]
struct SceneOutput<B: d3d::Backend> {
    service: crate::frame_owner::Service,
    owner: d3d::threaded::owner::FrameOwner<B>,
    /// The owner's display-rate loop on the render thread; `None` for a
    /// locked owner or without a frame rate.
    display_rate: Option<Box<dyn d3d::threaded::owner::DisplayRate<B>>>,
}

/// A private-target worker uses a second surface store on the same GPU.
/// Eligible displayed frames use the display's existing store on its thread.
#[cfg(feature = "gpu")]
pub fn install_game_view(
    rt: &runtime::Runtime,
    mut backend: d3d::GfxBackend,
    fps: Option<FrameRate>,
    thread: Option<RenderThread>,
    profile: film::RenderHooks,
    private: Option<PrivateScenes>,
) -> anyhow::Result<Installed> {
    let scenes = if let Some(private) = private {
        let service = private.install(&mut backend)?;
        // Retail, or the review launch's lamp arm: fixed for the run.
        let wanted = review_lighting().unwrap_or_default();
        let lighting: Arc<dyn Fn() -> d3d::Lighting + Send + Sync> = Arc::new(move || wanted);
        let display = private.display(fps);
        let owner = crate::frame_cutover::owner(service.display_device(), Arc::clone(&lighting), display.clone());
        let display_rate = display.map(|d| d.on_display(lighting));
        Some(SceneOutput { service, owner, display_rate })
    } else {
        None
    };
    let installed = install_gpu_for_profile(rt, backend, None, thread, profile, scenes)?;
    Ok(installed.with_gpu_view(None))
}

#[cfg(feature = "gpu")]
pub fn install_scaled_view(
    rt: &runtime::Runtime,
    mut backend: d3d::ScaledBackend<d3d::GfxBackend, d3d::GfxBackend>,
    fps: Option<FrameRate>,
    thread: Option<RenderThread>,
    profile: film::RenderHooks,
    private: Option<PrivateScenes>,
) -> anyhow::Result<Installed> {
    let stats = backend.shared_stats();
    let scenes = if let Some(private) = private {
        let service = private.install(backend.view_mut())?;
        let display = private.display(fps);
        let owner = crate::frame_cutover::scaled_owner(service.display_device(), display.clone());
        let display_rate = display.map(|d| d.on_scaled());
        Some(SceneOutput { service, owner, display_rate })
    } else {
        None
    };
    let report = scale_report(&mut backend);
    let installed = install_gpu_for_profile(rt, backend, Some(report), thread, profile, scenes)?;
    Ok(installed.with_gpu_view(Some(stats)))
}

#[cfg(feature = "gpu")]
enum SceneWorker {
    Private(crate::frame_owner::Service),
    Display(crate::frame_owner::Service),
}

#[cfg(feature = "gpu")]
fn install_gpu_for_profile<B>(
    rt: &runtime::Runtime,
    backend: B,
    extra: Option<ExtraReport>,
    thread: Option<RenderThread>,
    profile: film::RenderHooks,
    scenes: Option<SceneOutput<B>>,
) -> anyhow::Result<Installed>
where
    B: d3d::backend::Backend + Send + 'static,
    B::Texture: Sync,
{
    let threaded = thread.is_some();
    let RenderThread { synchronous_presents } = thread.unwrap_or_default();
    let switches = hooks::Switches::from_env().map_err(anyhow::Error::msg)?;
    let measure = threaded
        && profile == film::RenderHooks::Gpu
        && switches.feature(render_bridge::carried_slots::ID, true);
    let window = measure.then(|| {
        let w = d3d::scene_sink::window::PresentWindow::new(scenes.is_some());
        Arc::new(w.with_seal_probe(|| {
            render_bridge::scene_step::last().is_some_and(|scene| {
                scene.id.map_epoch == render_bridge::native_map_work::generation()
            })
        }))
    });
    let worker = if let Some(scenes) = scenes {
        if threaded {
            let mut display = d3d::ThreadedBackend::new(backend)?
                .with_synchronous_presents(synchronous_presents)
                .with_frame_owner(scenes.owner)
                // The game's targets are the frame graph's, by role
                // (docs/graphics/retire-d3d-display.md, "The graph's targets").
                .with_target_store(Box::new(native_render::frame_graph::targets::Store::<B>::default()))
                .with_present_window(window.clone());
            if let Some(rate) = scenes.display_rate {
                display = display.with_display_rate(rate);
            }
            return install_device(rt, display, extra, profile, Some(SceneWorker::Display(scenes.service)), window);
        }
        Some(SceneWorker::Private(scenes.service))
    } else {
        None
    };
    if !threaded {
        return install_device(rt, backend, extra, profile, worker, None);
    }
    let display = d3d::ThreadedBackend::new(backend)?
        .with_synchronous_presents(synchronous_presents)
        .with_present_window(window.clone());
    install_device(rt, display, extra, profile, worker, window)
}

/// [`render_thread_requested`] for a given value of the variable: only
/// `0` turns it off.
pub fn render_thread_from(value: Option<&str>) -> bool {
    value != Some("0")
}

#[cfg(feature = "gpu")]
fn save_agent_capture(
    ctx: &gfx::GfxContext,
    store: &gfx::surface::SurfaceStore,
    id: gfx::surface::SurfaceId,
    write: &agent::render::CaptureWrite,
) -> anyhow::Result<Option<content::Sha256>> {
    if let Some(dir) = write.path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if !write.digest {
        save_png(ctx, store, id, &write.path, false)?;
        return Ok(None);
    }

    let (pixels, width, height) = read_rgba(ctx, store, id)?;
    let mut png = Vec::new();
    encode_png(&mut png, &pixels, width, height)?;
    let digest = content::Sha256::of(&png);
    std::fs::write(&write.path, png)?;
    Ok(Some(digest))
}

#[cfg(feature = "gpu")]
fn save_png(
    ctx: &gfx::GfxContext,
    store: &gfx::surface::SurfaceStore,
    id: gfx::surface::SurfaceId,
    path: &std::path::Path,
    skip_black: bool,
) -> anyhow::Result<bool> {
    let (px, w, h) = read_rgba(ctx, store, id)?;
    if skip_black && all_black(&px) {
        return Ok(false);
    }
    write_png(path, &px, w, h)?;
    Ok(true)
}

#[cfg(feature = "gpu")]
/// Surface `id`'s pixels as RGBA with opaque alpha, and its size.
pub(crate) fn read_rgba(
    ctx: &gfx::GfxContext,
    store: &gfx::surface::SurfaceStore,
    id: gfx::surface::SurfaceId,
) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let (mut px, order) = store
        .read_color(ctx, id)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let d = store.get(id).map_err(|e| anyhow::anyhow!("{e:?}"))?.desc();
    opaque_rgba(&mut px, order);
    Ok((px, d.width, d.height))
}

#[cfg(feature = "gpu")]
/// Host pixels in `order` as RGBA with opaque alpha, in place.
fn opaque_rgba(px: &mut [u8], order: gfx::surface::HostPixelOrder) {
    if matches!(order, gfx::surface::HostPixelOrder::Bgra8) {
        for p in px.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
    }
    for p in px.as_chunks_mut::<4>().0 {
        p[3] = 0xff; // X8R8G8B8 back buffer: alpha is undefined
    }
}

pub fn write_png(path: &std::path::Path, px: &[u8], w: u32, h: u32) -> anyhow::Result<()> {
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    encode_png(f, px, w, h)
}

pub(crate) fn encode_png(
    mut output: impl std::io::Write,
    px: &[u8],
    w: u32,
    h: u32,
) -> anyhow::Result<()> {
    let mut enc = png::Encoder::new(&mut output, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(px)?;
    writer.finish()?;
    output.flush()?;
    Ok(())
}

#[cfg(feature = "gpu")]
/// The lighting A/B's comparison (`H2_BOOT_GFX_LIGHTING_AB`): the retail
/// view presents a frame before the E3 view does (it is the E3 view's
/// reference), so its image of each shadow-map frame is held here until
/// the E3 view's arrives, and the two are compared.
#[derive(Default)]
struct AbCompare {
    /// The retail view's image of a frame, waiting for the E3 view's.
    pending: Option<(u64, Vec<u8>)>,
    /// Shadow-map frames compared, those whose images differ, and those
    /// that differ only by rounding (no channel by more than `AB_NOISE`).
    compared: u64,
    differ: u64,
    noise: u64,
    /// The first differing frames, for the report.
    first: Vec<u64>,
}

#[cfg(feature = "gpu")]
impl AbCompare {
    /// The E3 view's image `e3` of `frame`: compared with the retail
    /// view's, if it was held. `Some(retail image)` when they differ.
    fn compare(&mut self, frame: u64, e3: &[u8]) -> Option<Vec<u8>> {
        let (f, retail) = self.pending.take()?;
        if f != frame || retail.len() != e3.len() {
            return None;
        }
        self.compared += 1;
        if retail == e3 {
            return None;
        }
        if !differs_beyond_noise(&retail, e3) {
            self.noise += 1;
            return None;
        }
        self.differ += 1;
        if self.first.len() < 20 {
            self.first.push(frame);
        }
        Some(retail)
    }

    fn report(&self) -> String {
        let first: Vec<String> = self.first.iter().map(u64::to_string).collect();
        format!(
            "lighting A/B: {} of {} shadow-map frames differ between the views \
             ({} more by rounding only){}",
            self.differ,
            self.compared,
            self.noise,
            if first.is_empty() {
                String::new()
            } else {
                format!(" (first: {})", first.join(", "))
            }
        )
    }
}

#[cfg(feature = "gpu")]
/// The largest per-channel change the A/B treats as rounding: the two
/// views take different shader variants, which can round a channel by one.
const AB_NOISE: u8 = 2;

#[cfg(feature = "gpu")]
/// Whether RGBA images `a` and `b` differ in some colour channel by more
/// than `AB_NOISE` (alpha aside).
fn differs_beyond_noise(a: &[u8], b: &[u8]) -> bool {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .any(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > AB_NOISE))
}

#[cfg(feature = "gpu")]
/// Whether RGBA pixels `px` are all black (alpha aside).
fn all_black(px: &[u8]) -> bool {
    px.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 0, 0])
}

/// The headless default: a `RecordingBackend` that forgets every call once
/// it returns, except the live surface records its `destroy_surface`
/// checks against. Without this the recorder would hold
/// a copy of every draw (streams included) for the whole run; the report's
/// draw counts come from `DiagBackend`.
#[cfg(feature = "desktop-runtime")]
#[derive(Default)]
pub struct Headless(RecordingBackend, bool);

#[cfg(feature = "desktop-runtime")]
impl Headless {
    fn forget<T>(&mut self, r: T) -> T {
        use d3d::backend::Recorded;
        if self.0.calls.last().is_some_and(|c| {
            !matches!(
                c,
                Recorded::Surface { .. } | Recorded::DestroySurface { .. }
            )
        }) {
            self.0.calls.pop();
        }
        r
    }

    /// Calls still held (surface lifetimes only).
    pub fn held(&self) -> usize {
        self.0.calls.len()
    }
}

#[cfg(feature = "desktop-runtime")]
impl d3d::backend::Backend for Headless {
    type Surface = u32;
    type Texture = u32;

    #[cfg(feature = "gpu")]
    fn upload_scene_assets(&mut self, upload: d3d::backend::AssetUpload) {
        self.0.upload_scene_assets(upload);
    }

    #[cfg(feature = "gpu")]
    fn frozen_images(&self, texture: &u32) -> Option<d3d::scene_sink::FrozenImages> {
        self.0.frozen_images(texture)
    }

    #[cfg(feature = "gpu")]
    fn retain_scene_sample(&self, texture: &u32) -> Option<d3d::scene_sink::FrozenSample> {
        self.0.retain_scene_sample(texture)
    }

    fn create_surface(
        &mut self,
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<u32, String> {
        self.0.create_surface(format, width, height, depth)
    }
    fn destroy_surface(&mut self, surface: u32) -> Result<(), String> {
        self.0.destroy_surface(surface)?;
        // The recorder just checked the handle. Drop both records so maps
        // can cycle without accumulating a history of dead surfaces.
        self.0.calls.pop();
        self.0.calls.retain(|c| {
            !matches!(c, d3d::backend::Recorded::Surface { id, .. } if *id == surface)
        });
        Ok(())
    }
    fn create_target(&mut self, target: d3d::TargetDesc) -> Result<u32, String> {
        self.0.create_target(target)
    }
    fn release_target(&mut self, _role: frame_scene::TargetRole, surface: u32) -> Result<(), String> {
        self.destroy_surface(surface)
    }
    fn upload_texture(&mut self, texture: &gfx::texture::DecodedTexture) -> Result<u32, String> {
        let r = self.0.upload_texture(texture);
        self.forget(r)
    }
    fn upload_shared(&mut self, texture: Arc<gfx::texture::DecodedTexture>) -> Result<u32, String> {
        let r = self.0.upload_shared(texture);
        self.forget(r)
    }
    fn uses_texture_placeholders(&self) -> bool {
        self.1
    }
    fn texture_placeholder(&mut self, _: &gfx::texture::TextureDesc) -> Result<u32, String> {
        Ok(self.0.texture_placeholder_id())
    }
    fn bc_supported(&self) -> bool {
        self.0.bc_supported()
    }
    fn clear(&mut self, color: u32, depth: Option<u32>, clear: gfx::Clear) -> Result<(), String> {
        let r = self.0.clear(color, depth, clear);
        self.forget(r)
    }
    fn draw(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: &d3d::backend::ResolvedDraw,
        textures: [Option<&u32>; 4],
    ) -> Result<d3d::backend::DrawOutcome, String> {
        let r = self.0.draw(color, depth, draw, textures);
        self.forget(r)
    }
    fn draw_owned(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: d3d::backend::ResolvedDraw,
        textures: [Option<&u32>; 4],
    ) -> (
        Result<d3d::backend::DrawOutcome, String>,
        Vec<d3d::backend::StreamBytes>,
    ) {
        let r = self.0.draw_owned(color, depth, draw, textures);
        self.forget(r)
    }
    fn begin_native_carry(
        &mut self,
        color: u32,
        depth: Option<u32>,
        admission: &d3d::backend::RetainedAdmission<u32>,
    ) -> bool {
        self.0.begin_native_carry(color, depth, admission)
    }
    fn record_native_carry(&mut self, color: u32, depth: Option<u32>, textures: [Option<&u32>; 4]) {
        self.0.record_native_carry(color, depth, textures)
    }
    fn end_native_carry(&mut self) {
        self.0.end_native_carry()
    }
    fn draw_admitted(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: d3d::backend::ResolvedDraw,
        textures: [Option<&u32>; 4],
        admission: d3d::backend::RetainedAdmission<u32>,
    ) -> (
        Result<d3d::backend::DrawOutcome, String>,
        Vec<d3d::backend::StreamBytes>,
    ) {
        let r = self.0.draw_admitted(color, depth, draw, textures, admission);
        self.forget(r)
    }
    fn draw_shared(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: Arc<d3d::backend::ResolvedDraw>,
        textures: [Option<&u32>; 4],
    ) -> (
        Result<d3d::backend::DrawOutcome, String>,
        Vec<d3d::backend::StreamBytes>,
    ) {
        let r = self.0.draw_shared(color, depth, draw, textures);
        self.forget(r)
    }
    fn frame_stats(&mut self) {
        self.0.frame_stats()
    }
    fn present(&mut self, surface: u32, flags: u32) -> Result<(), String> {
        let r = self.0.present(surface, flags);
        self.forget(r)
    }
    fn present_replay(&mut self, surface: u32, gamma: Option<&[u8; 768]>) -> Result<(), String> {
        let r = self.0.present_replay(surface, gamma);
        self.forget(r)
    }
    fn reset_present_metadata(&mut self) -> Result<(), String> {
        self.0.reset_present_metadata()
    }

    fn drain(&mut self) -> Result<(), String> {
        self.0.drain()
    }
    // The native renderer's pass (docs/graphics/native-renderer.md):
    // nothing to draw into headless.
    #[cfg(feature = "gpu")]
    fn external_attachments(
        &mut self, surfaces: d3d::ExternalSurfaces<Self::Surface>, textures: [Option<&Self::Texture>; 4],
        copy: d3d::TargetCopy,
    ) -> Result<d3d::ExternalCopies, String> {
        self.0.external_attachments(surfaces, textures, copy)
    }

    fn prepare_external(
        &mut self, surfaces: d3d::ExternalSurfaces<Self::Surface>, request: d3d::ExternalPassRequest,
        textures: [Option<&Self::Texture>; 4], plan: &d3d::ExternalPlan, copy: d3d::TargetCopy, pass: d3d::SharedReplacement,
    ) -> Result<Option<Vec<d3d::ExternalRecording>>, String> {
        self.0.prepare_external(surfaces, request, textures, plan, copy, pass)
    }

    fn can_commit_carried(&self) -> bool {
        self.0.can_commit_carried()
    }

    fn prepare_carried_stage(
        &mut self, stage: &d3d::SceneStage<Self::Surface>,
    ) -> Result<Option<Vec<d3d::ExternalRecording>>, String> {
        self.0.prepare_carried_stage(stage)
    }

    fn commit_carried_stage(
        &mut self, stage: d3d::SceneStage<Self::Surface>, copies: Vec<d3d::ExternalRecording>,
    ) {
        self.0.commit_carried_stage(stage, copies);
    }

    fn queue_external_draw(
        &mut self, candidate: d3d::backend::QueuedExternalDraw<Self::Surface>,
        textures: [Option<&Self::Texture>; 4],
    ) -> bool {
        self.0.queue_external_draw(candidate, textures)
    }

    fn reusable_external_attachments(&self) -> bool {
        self.0.reusable_external_attachments()
    }

    fn runs_scene_stages(&self) -> bool {
        self.0.runs_scene_stages()
    }

    fn run_scene_stage(&mut self, stage: d3d::SceneStage<Self::Surface>) -> Result<bool, String> {
        self.0.run_scene_stage(stage)
    }

    fn accepts_scene_pass(&self, pass: &dyn d3d::ExternalPass) -> bool {
        self.0.accepts_scene_pass(pass)
    }

    fn take_scene_refusal(&mut self) -> bool {
        self.0.take_scene_refusal()
    }

    fn queues_scene_stages(&self) -> bool {
        self.0.queues_scene_stages()
    }

    fn queue_scene_stage(&mut self, stage: d3d::SceneStage<Self::Surface>) -> bool {
        self.0.queue_scene_stage(stage)
    }

    fn commit_external(&mut self, recording: d3d::ExternalRecording) {
        self.0.commit_external(recording);
    }

    #[cfg(feature = "gpu")]
    fn upload_device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        self.0.upload_device()
    }

    fn supports_external_pass(&self) -> bool {
        self.0.supports_external_pass()
    }
    fn external_pass(
        &mut self,
        color: u32,
        depth: u32,
        sampled: Vec<Option<u32>>,
        request: d3d::ExternalPassRequest,
        pass: Box<dyn d3d::ExternalPass>,
    ) -> Result<bool, String> {
        self.0.external_pass(color, depth, sampled, request, pass)
    }
    fn queue_frame_probe_read(
        &mut self, surface: Self::Surface, width: u32, height: u32,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        self.0.queue_frame_probe_read(surface, width, height, done)
    }

    fn measure_present_raster(&mut self, surface: Option<Self::Surface>) -> d3d::backend::RasterMeasurement {
        self.0.measure_present_raster(surface)
    }
    fn last_present_unshown(&self) -> bool {
        self.0.last_present_unshown()
    }

    fn take_presented_extent(&mut self) -> Option<(u32, u32)> {
        self.0.take_presented_extent()
    }
    fn surface_extent(&self, surface: u32) -> Option<(u32, u32)> {
        self.0.surface_extent(surface)
    }
    fn render_extents(
        &mut self,
        color: u32,
        depth: Option<u32>,
        textures: [Option<&u32>; 4],
    ) -> d3d::backend::RenderExtents {
        self.0.render_extents(color, depth, textures)
    }
    fn read_display_surface(
        &mut self,
        surface: u32,
    ) -> Result<Option<d3d::backend::DisplaySurfacePixels>, String> {
        self.0.read_display_surface(surface)
    }
    fn present_with_gamma(
        &mut self,
        surface: u32,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        let r = self.0.present_with_gamma(surface, flags, gamma);
        self.forget(r)
    }
    fn clear_rects(
        &mut self,
        color: u32,
        depth: Option<u32>,
        clear: &d3d::backend::RectClear,
    ) -> Result<d3d::backend::DrawOutcome, String> {
        let r = self.0.clear_rects(color, depth, clear);
        self.forget(r)
    }
    fn read_surface(
        &mut self,
        surface: u32,
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
        layout: d3d::backend::GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        let r = self.0.read_surface(surface, format, width, height, layout);
        self.forget(r)
    }
    fn read_depth(
        &mut self,
        surface: u32,
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
        layout: d3d::backend::GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        let r = self.0.read_depth(surface, format, width, height, layout);
        self.forget(r)
    }
    fn copy_surface(&mut self, src: u32, dst: u32) -> Result<bool, String> {
        let r = self.0.copy_surface(src, dst);
        self.forget(r)
    }
    fn write_surface(
        &mut self,
        surface: u32,
        desc: &gfx::texture::TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        let r = self.0.write_surface(surface, desc, bytes);
        self.forget(r)
    }
    fn snapshot_surface(&mut self, surface: u32, opaque: bool) -> Result<Option<u32>, String> {
        let r = self.0.snapshot_surface(surface, opaque);
        self.forget(r)
    }
    fn snapshot_surface_into(&mut self, surface: u32, opaque: bool, into: &mut u32) -> Result<bool, String> {
        let r = self.0.snapshot_surface_into(surface, opaque, into);
        self.forget(r)
    }
    fn clear_pixel_count(&mut self) -> Result<(), String> {
        let r = self.0.clear_pixel_count();
        self.forget(r)
    }
    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        let r = self.0.report_pixel_count();
        self.forget(r)
    }
    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        let r = self.0.read_pixel_count(ticket);
        self.forget(r)
    }
    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        let r = self.0.try_pixel_count(ticket);
        self.forget(r)
    }
    fn drop_pixel_count(&mut self, ticket: u64) {
        self.0.drop_pixel_count(ticket)
    }
    fn pixel_count_unsubmitted(&self, ticket: u64) -> bool {
        self.0.pixel_count_unsubmitted(ticket)
    }
    fn can_sample_depth_surfaces(&self) -> bool {
        self.0.can_sample_depth_surfaces()
    }
    fn depth_level_texture(&mut self, surface: u32) -> Result<Option<u32>, String> {
        // Never called: the recorder does not sample depth surfaces.
        self.0.depth_level_texture(surface)
    }
    fn create_texture_target(
        &mut self,
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<u32, String> {
        self.0.create_texture_target(format, width, height)
    }
    fn can_sample_surfaces(&self) -> bool {
        self.0.can_sample_surfaces()
    }
    fn surface_texture(&mut self, surface: u32) -> Result<Option<u32>, String> {
        // Never called: the recorder does not sample surfaces.
        self.0.surface_texture(surface)
    }
    fn can_mirror(&self) -> bool {
        self.0.can_mirror()
    }
    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        self.0.mirror_reserve(len)
    }
    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        self.0.mirror_write(offset, bytes)
    }
    fn mirror_store(
        &mut self,
        surface: u32,
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
        layout: d3d::backend::GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.0
            .mirror_store(surface, format, width, height, layout, offset)
    }
    fn mirror_load(
        &mut self,
        surface: u32,
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
        layout: d3d::backend::GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.0
            .mirror_load(surface, format, width, height, layout, offset)
    }
    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.0.mirror_read(offset, len)
    }
    fn handles_surface_aliases(&self) -> bool {
        self.0.handles_surface_aliases()
    }
    fn defers_depth_aliases(&self) -> bool {
        self.0.defers_depth_aliases()
    }
    fn judge(&self) -> Option<Box<dyn d3d::backend::DrawJudge>> {
        self.0.judge()
    }
    fn depth_alias_texture(
        &mut self,
        depth: u32,
        format: gfx::D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: &gfx::texture::TextureDesc,
        options: gfx::texture::DecodeOptions,
    ) -> Result<Option<u32>, String> {
        self.0.depth_alias_texture(
            depth, format, width, height, pitch, generation, offset, desc, options,
        )
    }
}

/// [`install`] on the caller's backend (a host window's `GfxBackend`, with
/// its present hook already attached).
pub fn install_with<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    backend: B,
) -> anyhow::Result<Installed> {
    install_with_report(rt, backend, None)
}

/// [`install_with`], with `extra` added to the run report ([`describe`]).
pub fn install_with_report<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    backend: B,
    extra: Option<ExtraReport>,
) -> anyhow::Result<Installed> {
    install_for_profile(rt, backend, extra, crate::run_plan::compiled_render_hooks())
}

pub fn install_for_profile<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    backend: B,
    extra: Option<ExtraReport>,
    profile: film::RenderHooks,
) -> anyhow::Result<Installed> {
    install_device(
        rt, backend, extra, profile,
        #[cfg(feature = "gpu")]
        None,
        #[cfg(feature = "gpu")]
        None,
    )
}

fn install_device<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    backend: B,
    extra: Option<ExtraReport>,
    profile: film::RenderHooks,
    #[cfg(feature = "gpu")] worker: Option<SceneWorker>,
    #[cfg(feature = "gpu")] window: Option<Arc<d3d::scene_sink::window::PresentWindow>>,
) -> anyhow::Result<Installed> {
    anyhow::ensure!(
        crate::run_plan::compiled_render_hooks().supports(profile),
        "unavailable D3D hook profile {profile:?}"
    );
    #[cfg(feature = "desktop-runtime")]
    let (backend, draw_stats) = crate::d3d_diag::DiagBackend::new(backend);
    #[cfg(feature = "gpu")]
    let backend = backend.with_owner_probe(matches!(&worker, Some(SceneWorker::Display(_))));
    // The frame's carried part by slot (frame-scene.sink; docs/graphics/
    // framescene.md, slice 6): records only while the frame step declares
    // a slot, and passes every call on. Each object shadow's ops are
    // marked with its scene index for E3's sun tiles; the marks draw
    // nothing (docs/graphics/e3-lighting.md, L5).
    #[cfg(feature = "desktop-runtime")]
    let backend = d3d::SceneSink::new(backend, render_bridge::carried_slots::slot)
        .with_object_shadows(render_bridge::scene_step::object_shadow_at);
    #[cfg(feature = "gpu")]
    let backend = {
        crate::scene_glue::set_present_window(window.as_ref());
        backend.with_present_window(window.clone(), render_bridge::carried_slots::take_present_game_view)
    };
    #[cfg(feature = "gpu")]
    let (backend, worker) = if let Some(worker) = worker {
        let (backend, service) = match worker {
            SceneWorker::Private(service) => (backend.with_replay(service.recorder()), service),
            SceneWorker::Display(service) => (backend.with_replay(crate::frame_cutover::record(&service)), service),
        };
        let device = service.display_device();
        let backend = backend
            .with_guest_reference_request(move || device.reference_armed())
            .on_present_error(service.unshown_observer())
            .with_census(false);
        (backend, Some(service))
    } else {
        (backend, None)
    };
    // Tracked uploads trust the write barrier below; H2_BOOT_UPLOAD_ALWAYS
    // goes back to re-reading every resource on every use.
    let upload = if std::env::var_os("H2_BOOT_UPLOAD_ALWAYS").is_some() {
        UploadPolicy::Always
    } else {
        UploadPolicy::Tracked
    };
    // With the native renderer every render target and depth surface is
    // made by its role (docs/graphics/retire-d3d-display.md, "The graph's
    // targets").
    let native = hooks::switches::native_render_on(&hooks::Switches::from_env().unwrap_or_default());
    let cfg = DeviceConfig {
        upload,
        nv2a_window: Some(NV2A_BASE),
        #[cfg(feature = "desktop-runtime")]
        roles: native.then_some(render_bridge::native_targets::role as fn(u32, &dyn Fn(u32) -> Option<u32>) -> Option<frame_scene::TargetRole>),
        #[cfg(not(feature = "desktop-runtime"))]
        roles: { let _ = native; None },
        ..DeviceConfig::halo2(
            GuestRange {
                addr: PUSH_BASE,
                len: PUSH_LEN,
            },
            GuestRange {
                addr: SCRATCH_BASE,
                len: 0x1000,
            },
        )
    };
    d3d::runtime_glue::map_device_memory(&mut rt.memory(), &cfg)?;
    let device = d3d::runtime_glue::shared(cfg, backend);
    #[cfg(feature = "desktop-runtime")]
    {
        let drain = Arc::new(SinkDrain {
            device: Arc::clone(&device),
            #[cfg(feature = "gpu")]
            window,
        });
        // render.residual's producer boundaries drain the same device
        // (render_bridge::residual_census::Drain).
        render_bridge::residual_census::set_drain(drain.clone());
        // The frame step's publish (docs/graphics/retire-d3d-display.md,
        // "The final display") reads the sink's open frame and presents
        // through the same device.
        render_bridge::render_frame::publish::set(drain.clone());
        render_bridge::carried_slots::set_drain(drain);
    }
    let watch = subscribe_write_barrier(rt)?;
    #[cfg(feature = "gpu")]
    let shadow_admission = if profile == film::RenderHooks::Gpu {
        if hooks::switches::native_render_on(&hooks::Switches::from_env().unwrap_or_default()) {
            let archive = d3d::pipeline_store::default_path().map(|path| path.with_added_extension("native-v1"));
            native_render::pipelines::start(archive);
        }
        let shadows = crate::native_world_glue::install(&device, watch);
        crate::native_objects_glue::install(&device);
        crate::native_decals_glue::install(&device);
        crate::native_decal_decorators_glue::install(&device);
        crate::native_weather_glue::install(&device);
        crate::native_particles_glue::install(&device);
        crate::native_first_person_glue::install(&device);
        shadows
    } else {
        None
    };
    let mut out = None;
    rt.edit_dispatch(|d| {
        out = Some(d3d::register(d, Arc::clone(&device)));
        // Yield to the kernel until the 60 Hz logical vblank source delivers
        // enough callbacks for this frame (docs/graphics/d3d.md, "Vertical blank").
        d3d::register_frame_wait(
            d,
            Arc::clone(&device),
            d3d::FrameWait::HALO2_CLOCKED,
            Arc::new(d3d::runtime_glue::default_stop),
        );
    });
    let r = out.unwrap();
    let entries: Vec<u32> = r
        .implemented
        .iter()
        .chain(&r.stopping)
        .map(|e| e.0)
        .collect();
    install_write_barrier(rt, &device, &entries, watch);
    #[cfg(feature = "gpu")]
    if profile == film::RenderHooks::Gpu {
        crate::native_world_layers_glue::install(rt, &device);
        crate::native_lights_glue::walk::install(rt, &device, shadow_admission);
        crate::native_residual_glue::install(&device);
        crate::native_targets_glue::install(&device);
        crate::native_flare_query_glue::install(&device);
        crate::native_flare_draw_glue::install(&device);
        crate::native_fog_quad_glue::install(&device);
        crate::native_fog_fans_glue::install(&device);
    }
    let vblank: Option<kernel::periodic::Handler> =
        Some(Box::new(d3d::vblank::clocked_vblanks(Arc::clone(&device))));
    let snapshot: Arc<dyn DeviceSnapshot> = device.clone();
    #[cfg(feature = "gpu")]
    let ui_layout = device.clone();
    let dev = device;
    let report = Box::new(move || {
        let mut d = dev.lock().unwrap();
        // Device events by kind and reason, most frequent first.
        let mut ev: std::collections::BTreeMap<String, u64> = Default::default();
        let (events, dropped) = d.take_events_counted();
        for e in events {
            let k = match e {
                d3d::Event::DrawSkipped { entry, reason } => {
                    format!("draw skipped in {entry}: {reason}")
                }
                d3d::Event::BadState(b) => format!("bad state {b:?}"),
                other => format!("{other:?}"),
            };
            let k: String = k.chars().take(200).collect();
            *ev.entry(k).or_default() += 1;
        }
        let mut ev: Vec<_> = ev.into_iter().collect();
        ev.sort_by_key(|e| std::cmp::Reverse(e.1));
        let ev: String = ev
            .iter()
            .take(40)
            .map(|(k, n)| format!("\n  event {n:>8}  {k}"))
            .chain((dropped > 0).then(|| format!("\n  event {dropped:>8}  more, past the device's kept limit")))
            .collect();
        let extra = extra
            .as_ref()
            .map(|f| format!("\n{}", f()))
            .unwrap_or_default();
        #[cfg(feature = "desktop-runtime")]
        let draws = draw_stats.lock().unwrap().report();
        #[cfg(not(feature = "desktop-runtime"))]
        let draws = "";
        format!("stats {:?}; last stop {:?}{ev}\n{draws}{extra}", d.stats(), d.last_stop())
    });
    Ok(Installed {
        #[cfg(feature = "gpu")]
        ui_layout,
        #[cfg(feature = "gpu")]
        view: None,
        #[cfg(feature = "gpu")]
        _private_scenes: worker,
        vblank,
        snapshot,
        report,
        implemented: r.implemented.len(),
        stopping: r.stopping.len(),
        native: r.native.len(),
    })
}

/// The scene sink's drain (`render_bridge::carried_slots::Drain`): the
/// device decodes what game code wrote into the push buffer before the
/// frame step moves the slot.
#[cfg(feature = "desktop-runtime")]
struct SinkDrain<B: d3d::backend::Backend> {
    device: d3d::abi::SharedDevice<B>,
    #[cfg(feature = "gpu")]
    window: Option<Arc<d3d::scene_sink::window::PresentWindow>>,
}

#[cfg(feature = "desktop-runtime")]
impl<B: d3d::backend::Backend + Send + 'static> render_bridge::carried_slots::Drain for SinkDrain<B> {
    #[cfg(feature = "gpu")]
    fn game_view(&self) {
        if let Some(window) = &self.window {
            window.game_view(crate::native_objects_glue::palette_ready());
        }
    }

    fn drain(&self, mem: &mut dyn engine::memory::GuestMemory) {
        // A latched fault answers every later entry; nothing to add here.
        if let Ok(mut d) = self.device.lock() {
            let _ = d.sync_push_buffer(&mut SinkMem(mem));
        }
    }
}

/// The engine's guest memory as the device reads it.
#[cfg(feature = "desktop-runtime")]
struct SinkMem<'a>(&'a mut dyn engine::memory::GuestMemory);

#[cfg(feature = "desktop-runtime")]
impl d3d::GuestMemory for SinkMem<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.0.read(addr, out).map_err(|f| d3d::GuestError { addr: f.addr, len: f.len })
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        self.0.write(addr, data).map_err(|f| d3d::GuestError { addr: f.addr, len: f.len })
    }
}

/// The publish's host: the sink's open frame, after the device has run
/// what game code wrote, and the device's present of a frame drawn from
/// the scene (`d3d::Device::publish`).
#[cfg(feature = "desktop-runtime")]
impl<B: d3d::backend::Backend + Send + 'static> render_bridge::render_frame::publish::Publish for SinkDrain<B> {
    fn open(&self, mem: &mut dyn engine::memory::GuestMemory) -> render_bridge::render_frame::publish::Open {
        use d3d::scene_sink::OpenFrame;
        use render_bridge::render_frame::publish::Open;
        render_bridge::carried_slots::Drain::drain(self, mem);
        match d3d::scene_sink::open_frame() {
            OpenFrame::Scene => Open::Scene,
            OpenFrame::Carried(slot) => Open::Carried(slot.name()),
            OpenFrame::Untracked => Open::Untracked,
        }
    }

    fn present(
        &self,
        mem: &mut dyn engine::memory::GuestMemory,
        gamma: Option<&[u8; render_bridge::render_frame::publish::RAMP_BYTES]>,
    ) -> Result<(), String> {
        let mut d = self.device.lock().map_err(|_| "the device lock is poisoned".to_owned())?;
        d.publish(&mut SinkMem(mem), gamma).map(|_| ()).map_err(|e| e.to_string())
    }

    /// `0x00013d80`'s image drawn from the overlay over the back buffer,
    /// the frame's last pass before the present. Without the GPU overlay
    /// nothing places it, and the guest's present draws it.
    fn blit(
        &self,
        mem: &mut dyn engine::memory::GuestMemory,
        blit: &render_bridge::render_frame::blit::Blit,
    ) -> Result<(), &'static str> {
        #[cfg(feature = "gpu")]
        return crate::hud_overlay::place_blit(&self.device, mem, blit);
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (mem, blit);
            Err("host")
        }
    }
}

#[cfg(feature = "desktop-runtime")]
impl<B: d3d::backend::Backend + Send + 'static> render_bridge::residual_census::Drain for SinkDrain<B> {
    /// What the drain decoded, for the producer that was open while game
    /// code wrote it.
    fn drain(&self, mem: &mut dyn engine::memory::GuestMemory) -> render_bridge::residual_census::Tally {
        let before = self.device.lock().map(|d| d.residual_tally()).unwrap_or_default();
        render_bridge::carried_slots::Drain::drain(self, mem);
        let after = self.device.lock().map(|d| d.residual_tally()).unwrap_or_default();
        residual_tally(after.since(before))
    }
}

/// [`d3d::residual::Tally`] as `render_bridge::residual_census` has it.
#[cfg(feature = "desktop-runtime")]
pub(crate) fn residual_tally(t: d3d::residual::Tally) -> render_bridge::residual_census::Tally {
    render_bridge::residual_census::Tally {
        draws: t.draws,
        claimed: t.claimed,
        residual: t.residual,
        push_words: t.push_words,
        state_words: t.state_words,
        copies: t.copies,
        uploads: t.uploads,
        native_guest_binds: t.native_guest_binds,
        guest_bind_refused_destroy: t.guest_bind_refused_destroy,
        native_rect_clears: t.native_rect_clears,
        clear_refused_rect_count: t.clear_refused_rect_count,
    }
}

/// One line for the run report.
pub fn describe(i: &Installed) -> String {
    format!(
        "d3d: {} implemented, {} stopping, {} native overrides; {}",
        i.implemented,
        i.stopping,
        i.native,
        (i.report)()
    )
}

/// The NV2A register window crates/platform/d3d maps and stores at device+0x934
/// (`DeviceConfig::nv2a_window`): the retail 0xfd000000 cannot be used, as
/// the runtime keeps the NV2A aperture free of RAM mappings.
pub const NV2A_BASE: u32 = 0xa040_0000;

/// Shadow page for the displaced crates/platform/d3d handlers the boot wraps.
const D3D_SHADOW: u32 = 0xa0ff_2000;
const CREATE_DEVICE: u32 = 0x003f_5240;

/// Halo creates the device, releases it (the caller zeroes the device
/// object) and creates it again, so the back and depth surfaces are backed
/// after every Direct3D_CreateDevice ([`back_surfaces`]): crates/platform/d3d's
/// handler moves to a shadow address and a continuation runs once it
/// returns.
pub fn install_back_surfaces(rt: &runtime::Runtime) -> anyhow::Result<()> {
    rt.memory()
        .map_zeroed(D3D_SHADOW, 0x1000, runtime::guest::Permissions::RX)?;
    let device = d3d::GuestLayout::HALO2.device;
    let shadow = D3D_SHADOW;
    rt.edit_dispatch(|d| {
        let prev = d.register_override(
            CREATE_DEVICE,
            "boot:CreateDevice+back surfaces",
            move |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let ret_pc = mem.u32(cpu.esp)?;
                cpu.esp = cpu.esp.wrapping_add(4);
                ctx.call_guest(cpu, &mut mem, shadow, move |cpu, ctx| {
                    if cpu.eax == 0 {
                        back_surfaces(&mut ctx.runtime().memory(), device);
                    }
                    Ok(runtime::TransferAbi::ret(ret_pc))
                })
            },
        );
        if let Some(prev) = prev {
            d.restore_override(shadow, prev);
        }
    });
    Ok(())
}

/// Xbox D3D addresses the same physical page three ways: the contiguous
/// window `0x80000000 | phys`, the GPU's `phys`, and the CPU write-combined
/// window `0xf0000000 | phys` (Halo's CPU blits to surfaces use the last,
/// e.g. `or [esp+0Ch], 0xF0000000` at 0x13e4b after 0x3fdbc0). The runtime
/// maps none of 0xf0000000. Boot stopgap: alias `0xf0000000 | (va &
/// 0x0fffffff)` onto the physical pages behind `va` for the ranges the boot
/// itself maps (the fixed arena, the back and depth buffers). Blocker:
/// runtime/kernel (the write-combined window over all contiguous memory).
pub const WC_BASE: u32 = 0xf000_0000;

/// The write-combined window of every physical reservation (the loader was
/// given `WC_BASE | start` as an extra alias; see `run`).
pub fn map_reserved_write_combined(rt: &runtime::Runtime) -> anyhow::Result<()> {
    let mut mem = rt.memory();
    let rs: Vec<_> = mem
        .physical_reservations()
        .iter()
        .map(|r| (r.id, r.physical_start, r.len))
        .collect();
    for (id, start, len) in rs {
        mem.map_reserved(id, WC_BASE | start, len, runtime::guest::Permissions::RW)?;
    }
    Ok(())
}

/// crates/platform/d3d's CreateDevice writes the back-buffer and depth surface
/// headers with `Data = 0` (no guest backing; the host surface is the real
/// one), so Halo's CPU writes into the back buffer (0x13d80, a full-screen
/// blit right after the first map load) land at 0xf0000000. Boot stopgap:
/// give each header that still has `Data = 0` its own zeroed pages, at a
/// physical address P mapped at both `0x80000000 | P` and `0xf0000000 |
/// P`, and store P as Data. Owner: crates/platform/d3d (guest-backed surfaces).
pub fn back_surfaces(mem: &mut runtime::Guest, device: u32) {
    use d3d::layout::device_field as field;
    for f in [field::BACK_BUFFER, field::DEPTH_STENCIL] {
        let Ok(header) = mem.u32(device + f) else {
            continue;
        };
        if header == 0 || mem.u32(header + 4).is_ok_and(|d| d != 0) {
            continue;
        }
        let Ok(size) = mem.u32(header + 16) else {
            continue;
        };
        let (w, h, pitch) = (
            (size & 0xfff) + 1,
            ((size >> 12) & 0xfff) + 1,
            ((size >> 24) + 1) * 64,
        );
        let len = (pitch * h).max(w * 4).next_multiple_of(0x1000);
        let Some(phys) = mem.find_free_physical(len, 0..runtime::guest::RAM_SIZE as u32, 0x1_0000) else {
            crate::log::note(&format!(
                "back surface {header:#x}: no free physical memory"
            ));
            continue;
        };
        if mem
            .map(
                0x8000_0000 | phys,
                len,
                phys,
                runtime::guest::Permissions::RW,
            )
            .is_err()
        {
            crate::log::note(&format!("back surface {header:#x}: no room at {phys:#x}"));
            continue;
        }
        let _ = mem.map(
            0xf000_0000 | phys,
            len,
            phys,
            runtime::guest::Permissions::RW,
        );
        let _ = mem.write_bytes(0x8000_0000 | phys, &vec![0; len as usize], 1);
        let _ = mem.set_u32(header + 4, phys);
    }
}

/// The write barrier's write-watch owner: it drains its subscription by id
/// through the Guest the entry runs against.
pub const WATCH_OWNER: &str = "boot:d3d";

/// The D3D write barrier (docs/graphics/d3d.md "Integration", step 3) on the
/// runtime's write watch: one subscriber watches all of RAM in the folded
/// view, and every d3d entry is wrapped ([`barrier_handler`]) so that
///
/// - on entry, the pages written since the last drain (lifted code, the
///   interpreter, kernel and HLE copies) go to
///   `Device::note_written_pages`, before the device reads any resource;
/// - nothing is drained on return. An entry may call guest code before it
///   returns (a push-buffer callback, `D3D_DestroyResource`, the vblank
///   callback), and runner safe points, interrupts and other contexts can
///   store in between, so dropping the pages then would lose those stores.
///   The device's own stores are therefore reported back to it at the next
///   entry, which costs a re-check of a page it already has (its
///   render-target writebacks it invalidates itself).
///
/// The device's tracker keeps only the pages it watches, so watching all of
/// RAM here costs a bit per store and nothing more.
///
/// The drain is `Guest::take_dirty`, which folds the window's cards first
/// (`Guest::fold_cards`): with a flat-RAM image, generated stores mark
/// cards rather than the paged model's dirty bitmap, and every page stored
/// to since the last drain (and the page after it, since a store may
/// cross) reaches the device at the next entry
/// (docs/runtime/contract-thin-transfers-flat-ram.md, slice F4).
fn install_write_barrier<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    device: &Arc<Mutex<d3d::Device<B>>>,
    entries: &[u32],
    id: runtime::WatchId,
) {
    rt.edit_dispatch(|d| {
        for &addr in entries {
            let Some(prev) = d.override_handler(addr) else {
                continue;
            };
            let dev = Arc::clone(device);
            let note = move |dirty: &[runtime::DirtyRange]| {
                let mut dv = dev.lock().unwrap();
                for r in dirty {
                    dv.note_written_pages(r.addr, r.len);
                }
            };
            d.register_override(
                addr,
                "boot:d3d write barrier",
                barrier_handler(prev, id, note),
            );
        }
    });
}

/// Subscribe before installing native render hosts. Opaque section prepare,
/// sorted emit and D3D guest entries must consume the same stream of stores.
fn subscribe_write_barrier(rt: &runtime::Runtime) -> anyhow::Result<runtime::WatchId> {
    let mut mem = rt.memory();
    let id = mem.subscribe_writes(WATCH_OWNER);
    mem.watch_writes(id, runtime::watch::CONTIGUOUS_BASE, runtime::guest::RAM_SIZE as u32)?;
    Ok(id)
}

/// One entry's write barrier: drains watch `id` into `note` (when anything
/// is dirty), then runs the entry's own handler `prev` in place, as the
/// dispatcher would have.
///
/// Until 2026-11 the barrier moved `prev` to a shadow address and reached it
/// as a guest call whose Rust continuation returned to the caller: two more
/// runner transfers, a boxed continuation and a token on every D3D call
/// (thousands a frame). Running `prev` in place gives the guest the same
/// registers, memory and return; the only difference is the dead stack word
/// under the caller's `ESP` afterwards, which keeps the caller's return
/// address, as on the Xbox, instead of a continuation token.
pub fn barrier_handler(
    prev: runtime::exec::OriginalHandler,
    id: runtime::WatchId,
    note: impl Fn(&[runtime::DirtyRange]) + Send + Sync + 'static,
) -> impl Fn(
    &mut runtime::CpuAbi,
    &mut runtime::ExecCtx,
    u32,
) -> Result<runtime::TransferAbi, runtime::GuestFault>
+ Send
+ Sync
+ 'static {
    thread_local! {
        // The barrier drains on every D3D entry; reusing one buffer per
        // thread keeps that free of allocation.
        static DIRTY: std::cell::Cell<Vec<runtime::DirtyRange>> = const { std::cell::Cell::new(Vec::new()) };
    }
    move |cpu, ctx, selector| {
        let mut dirty = DIRTY.take();
        // The guard goes before `prev` runs: it takes memory itself. An
        // unknown id leaves `dirty` empty.
        let _ = ctx.runtime().memory().take_dirty_into(id, &mut dirty);
        if !dirty.is_empty() {
            note(&dirty);
        }
        DIRTY.set(dirty);
        Ok(prev.invoke(cpu, ctx, selector))
    }
}

#[cfg(all(test, feature = "desktop-runtime"))]
mod tests {
    use super::*;
    use d3d::backend::Backend;

    #[cfg(feature = "gpu")]
    #[test]
    fn sealed_frame_worker_requires_the_sink_and_a_serial_game_view() {
        use frame_scene::mailbox::Mode;
        let policy = |flags, profile, locked| {
            let switches = hooks::Switches::parse(flags).unwrap();
            PrivateScenes::requested(&switches, profile, locked).map(|p| p.mode)
        };
        let gpu = film::RenderHooks::Gpu;
        assert_eq!(policy("frame-scene.sink", gpu, true), Some(Mode::Locked));
        assert_eq!(policy("frame-scene.sink", gpu, false), Some(Mode::Paced));
        // A paced display owns its frames by default; a locked run asks.
        assert_eq!(policy("", gpu, false), Some(Mode::Paced));
        assert_eq!(policy("frame-scene.sink=off", gpu, false), None);
        for spec in [
            "", "frame-scene.sink,frame-scene.census", "frame-scene.sink,impl.render.native=off",
            "frame-scene.sink,frame-scene=off",
        ] {
            assert_eq!(policy(spec, gpu, true), None, "{spec}");
        }
        assert_eq!(policy("frame-scene.sink", film::RenderHooks::Slim, false), None);
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn readback_policy_covers_network_and_film_before_gameplay_starts() {
        let store = settings::Store::defaults(crate::setting_defs::ALL);
        // XNet is installed at boot for both LAN-only configurations (System
        // Link) and signed-in ones. Joining a match later uses that device.
        for (network, film, lockstep, single, label) in [
            (true, false, false, false, "lobby before a System Link, Live, or co-op join"),
            (false, true, false, false, "armed recorder or Theater replay"),
            (false, false, true, false, "lockstep or input-tape playback"),
            (true, true, false, false, "film of a network match"),
            (false, false, false, true, "offline play"),
        ] {
            let protected = protected_readbacks(network, film, lockstep);
            assert_eq!(single_copy(Some(&store), protected).unwrap(), single, "{label}");
        }
        let exact = settings::Flag::parse_set("graphics.single_copy=false").unwrap();
        let (store, errors) = settings::Store::load(crate::setting_defs::ALL, None, &|_| None, &[exact]);
        assert!(errors.is_empty());
        assert!(!single_copy(Some(&store), false).unwrap());
    }

    #[test]
    fn the_feature_arena_starts_after_the_device_pages() {
        const {
            assert!(PUSH_BASE + PUSH_LEN <= SCRATCH_BASE);
            assert!(SCRATCH_BASE + 0x1000 <= hooks::arena::ARENA_START);
            assert!(hooks::arena::ARENA_END <= crate::community_variants::DATA_BASE);
        }
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn slim_film_keeps_the_dedicated_indexed_draw_dispatch_on_a_gpu_player() {
        let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
        install_light_for_profile(&rt, film::RenderHooks::Slim).unwrap();
        rt.edit_dispatch(|dispatch| {
            assert_eq!(
                dispatch.override_name(native_render::world_layers::DRAW_INDEXED_VERTICES),
                Some("boot:d3d write barrier"),
            );
        });
    }

    #[test]
    fn light_textures_use_handles_without_retaining_payloads() {
        let mut light = Headless(RecordingBackend::default(), true);
        let desc = gfx::texture::TextureDesc::new(
            gfx::D3DFormat::LinA8R8G8B8, gfx::texture::TextureKind::D2, 64, 32, 1, 1,
        );
        assert!(light.uses_texture_placeholders());
        let a = light.texture_placeholder(&desc).unwrap();
        let b = light.texture_placeholder(&desc).unwrap();
        assert_ne!(a, b);
        assert_eq!(light.held(), 0);
    }

    /// A headless run's frame stats name the counters it can't fill and
    /// the switch that fills them; a GPU run, or no frame stats, says
    /// nothing.
    #[test]
    fn frame_stats_without_a_gpu_say_the_gpu_counters_stay_zero() {
        let note = frame_stats_gpu_note(true, false).unwrap();
        assert!(
            note.contains("draws") && note.contains("passes") && note.contains("stay 0"),
            "{note}"
        );
        assert!(
            note.contains("H2_BOOT_GFX=<dir> H2_BOOT_GFX_EVERY=never"),
            "{note}"
        );
        assert_eq!(frame_stats_gpu_note(true, true), None);
        assert_eq!(frame_stats_gpu_note(false, false), None);
    }

    #[cfg(feature = "gpu")]
    /// The A/B compares the E3 view's image with the retail view's of the
    /// same frame only, counts what it compared, and hands back the
    /// retail image when they differ by more than rounding.
    #[test]
    fn the_lighting_ab_compares_each_frames_two_views() {
        let mut ab = AbCompare::default();
        let px = |r: u8, a: u8| vec![r, 10, 10, a];
        assert_eq!(ab.compare(5, &px(1, 9)), None, "nothing held");
        ab.pending = Some((5, px(1, 9)));
        assert_eq!(ab.compare(5, &px(1, 9)), None, "the same");
        ab.pending = Some((6, px(1, 9)));
        assert_eq!(ab.compare(7, &px(9, 9)), None, "another frame's");
        ab.pending = Some((8, px(1, 9)));
        assert_eq!(ab.compare(8, &px(9, 9)), Some(px(1, 9)));
        ab.pending = Some((9, px(1, 9)));
        assert_eq!(ab.compare(9, &px(3, 9)), None, "rounding only");
        ab.pending = Some((10, px(1, 9)));
        assert_eq!(ab.compare(10, &px(1, 200)), None, "alpha only");
        assert_eq!(ab.pending, None);
        assert_eq!(
            ab.report(),
            "lighting A/B: 1 of 4 shadow-map frames differ between the views \
             (2 more by rounding only) (first: 8)"
        );
    }

    #[cfg(feature = "gpu")]
    /// A frame under a fade to black is all black whatever its alpha; one
    /// lit pixel is not.
    #[test]
    fn a_faded_frame_is_all_black() {
        let mut px = [0u8, 0, 0, 0xff].repeat(16);
        assert!(all_black(&px));
        px[3] = 0x40;
        assert!(all_black(&px));
        px[4 * 9 + 1] = 1;
        assert!(!all_black(&px));
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn in_between_captures_stop_at_their_limit() {
        let (keep, write, skip) = (
            GuestShot {
                write: false,
                keep: true,
            },
            GuestShot {
                write: true,
                keep: false,
            },
            GuestShot {
                write: false,
                keep: false,
            },
        );
        let shot = |name: &str, before: Option<&str>| {
            Some(BetweenShot {
                name: name.into(),
                before: before.map(Into::into),
            })
        };
        let b = BetweenShots::new(3);
        assert_eq!(b.guest_swap(10), keep, "no in-between yet: only a GPU copy");
        assert_eq!(
            b.next_between(""),
            shot("frame-00010-between-01.png", Some("frame-00010.png")),
            "its neighbour before, from the copy"
        );
        assert_eq!(
            b.next_between(""),
            shot("frame-00010-between-02.png", None),
            "written once"
        );
        assert_eq!(b.guest_swap(11), write, "its neighbour after");
        assert_eq!(
            b.next_between(""),
            shot("frame-00011-between-01.png", None),
            "already written"
        );
        assert_eq!(b.guest_swap(12), write, "the last one's neighbour after");
        assert_eq!(b.next_between(""), None, "the limit reached");
        assert_eq!(b.guest_swap(13), skip);
        assert_eq!(b.next_between(""), None);
        let off = BetweenShots::new(0);
        assert_eq!(off.guest_swap(1), skip);
        assert_eq!(off.next_between(""), None);
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn in_between_captures_write_only_their_neighbours() {
        // The live bug: one in-between frame, then thousands of guest
        // frames with none (held, or every frame a gap once the readbacks
        // slowed the run), each written while the limit was not reached.
        let b = BetweenShots::new(12);
        let mut written = Vec::new();
        let mut copies = 0;
        for n in 1..=3000u64 {
            let g = b.guest_swap(n);
            copies += u64::from(g.keep);
            if g.write {
                written.push(format!("frame-{n:05}.png"));
            }
            // In-between frames after swaps 465 and 2000 only.
            if n == 465 || n == 2000 {
                let s = b.next_between("").unwrap();
                written.extend(s.before);
                written.push(s.name);
            }
        }
        assert_eq!(
            written,
            [
                "frame-00465.png",
                "frame-00465-between-01.png",
                "frame-00466.png",
                "frame-02000.png",
                "frame-02000-between-01.png",
                "frame-02001.png",
            ]
        );
        // Copies on the GPU (no read back) while captures are left.
        assert_eq!(copies, 3000 - 2);
        // With every slot used, nothing more: at most N in-between frames
        // and 2N guest frames.
        let b = BetweenShots::new(2);
        let mut files = 0;
        for n in 1..=100u64 {
            files += usize::from(b.guest_swap(n).write);
            if let Some(s) = b.next_between("") {
                files += 1 + usize::from(s.before.is_some());
            }
        }
        assert_eq!(
            files,
            2 + 3,
            "frames 1 and 2 between, guest frames 1, 2 and 3"
        );
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn the_frame_rate_from_h2_fps() {
        assert_eq!(fps_from(None), 30);
        assert_eq!(fps_from(Some("60")), 60);
        assert_eq!(fps_from(Some("uncapped")), 0);
        assert_eq!(fps_from(Some("nonsense")), 30);
    }

    #[test]
    fn the_render_thread_is_on_unless_zero() {
        assert!(render_thread_from(None), "on when unset");
        assert!(render_thread_from(Some("1")));
        assert!(!render_thread_from(Some("0")));
        assert!(render_thread_from(Some("true")));
        assert!(render_thread_from(Some("")));
        // Hosts default to the synchronous backend.
        assert!(!crate::Host::default().render_thread);
    }

    #[test]
    fn headless_keeps_only_surface_lifetimes() {
        let mut b = Headless::default();
        let s = b
            .create_surface(gfx::D3DFormat::LinA8R8G8B8, 64, 64, false)
            .unwrap();
        let desc = gfx::texture::TextureDesc {
            format: gfx::D3DFormat::LinA8R8G8B8,
            kind: gfx::texture::TextureKind::D2,
            width: 64,
            height: 64,
            depth: 1,
            levels: 1,
            pitch: 256,
        };
        let bytes = vec![0u8; 64 * 64 * 4];
        for _ in 0..1000 {
            b.write_surface(s, &desc, &bytes).unwrap();
            b.clear(
                s,
                None,
                gfx::Clear {
                    color: Some([0.0; 4]),
                    depth: None,
                    stencil: None,
                },
            )
            .unwrap();
            b.present(s, 0).unwrap();
        }
        assert_eq!(b.held(), 1, "only the surface creation is held");
        b.destroy_surface(s).unwrap();
        assert!(b.destroy_surface(s).is_err(), "destroy is still checked");
        assert_eq!(b.held(), 0);
    }
}
