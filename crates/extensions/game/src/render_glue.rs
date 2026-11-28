//! The render's install (docs/graphics/native-renderer.md, "How a frame
//! flows"). Every run overrides the guest's D3D entries
//! (`render_bridge::guest_entries`) and its frame wait, with vblanks on the
//! 60 Hz logical clock (`render_bridge::frame_wait`). With a window, a
//! `native_render::Renderer` on the host's device draws each sealed frame
//! on the frame owner's thread and presents it on the window's
//! `gfx::Presenter`, and the native stages' glue is installed.
//! Hosts, `h2 sim` and headless runs build no renderer.

#[cfg(feature = "gpu")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "gpu")]
mod output_size;
#[cfg(feature = "gpu")]
pub use output_size::output_size;

/// What [`install_headless`] and [`install_game`] leave the boot.
pub struct Installed {
    /// The vblank callback on the 60 Hz logical clock, for
    /// `Kernel::add_periodic` (taken once).
    pub vblank: Option<kernel::periodic::Handler>,
    /// The UI fit the native UI items read.
    #[cfg(feature = "gpu")]
    pub ui_layout: Arc<dyn crate::render_aspect::UiLayoutSink>,
    /// The frame owner's thread; it ends when the mailbox's publisher drops.
    #[cfg(feature = "gpu")]
    _owner: Option<std::thread::JoinHandle<()>>,
    renderer: bool,
}

/// The guest entries and the frame wait: every run has them.
fn install_guest(rt: &runtime::Runtime) -> Option<kernel::periodic::Handler> {
    rt.edit_dispatch(|d| {
        render_bridge::guest_entries::register(d);
        render_bridge::frame_wait::register(d);
    });
    crate::presents::install();
    Some(Box::new(render_bridge::frame_wait::clocked_vblanks()))
}

/// No renderer: a host, `h2 sim` or a headless run.
pub fn install_headless(rt: &runtime::Runtime) -> anyhow::Result<Installed> {
    Ok(Installed {
        vblank: install_guest(rt),
        #[cfg(feature = "gpu")]
        ui_layout: Arc::new(UiLayout),
        #[cfg(feature = "gpu")]
        _owner: None,
        renderer: false,
    })
}

/// A renderer on `ctx` at `output`'s size, drawing on the frame owner's
/// thread at `fps` and presenting on `presenter`. Each scene the frame step
/// seals becomes a `native_render::Frame` and goes to the owner through the
/// mailbox (`locked`: every frame in order, for captures and films).
#[cfg(feature = "gpu")]
pub fn install_game(
    rt: &runtime::Runtime,
    ctx: gfx::GfxContext,
    presenter: gfx::Presenter,
    output: crate::run_opts::NativeSize,
    fps: FrameRate,
    locked: bool,
    profile: film::RenderHooks,
) -> anyhow::Result<Installed> {
    anyhow::ensure!(
        crate::run_plan::compiled_render_hooks().supports(profile),
        "unavailable render hook profile {profile:?}"
    );
    let vblank = install_guest(rt);
    let archive = gfx::pipeline_store::default_path().map(|path| path.with_added_extension("native-v1"));
    native_render::pipelines::start(archive);
    let mode = if locked { frame_scene::mailbox::Mode::Locked } else { frame_scene::mailbox::Mode::Paced };
    let (publisher, frames) = frame_scene::mailbox::channel(mode);
    anyhow::ensure!(FRAMES.set(publisher).is_ok(), "the renderer is installed once per process");
    render_bridge::scene_step::set_seal_observer(Some(render_bridge::scene_step::SealObserver { sealed, reset }));
    let renderer = native_render::Renderer::new(ctx, output());
    let display = native_render::frame_graph::display_rate::Display::new(fps);
    let owner = native_render::FrameOwner::new(renderer, presenter, frames, display);
    let captures = owner.captures();
    let _ = CAPTURES.set(Mutex::new(captures));
    let thread = owner.spawn();
    if profile == film::RenderHooks::Gpu {
        crate::native_world_glue::install();
        crate::native_objects_glue::install();
        crate::native_decals_glue::install();
        crate::native_decal_decorators_glue::install();
        crate::native_weather_glue::install();
        crate::native_particles_glue::install();
        crate::native_first_person_glue::install();
        crate::native_world_layers_glue::install(rt);
        crate::native_lights_glue::walk::install(rt);
        crate::native_targets_glue::install();
        crate::native_flare_query_glue::install();
        crate::native_flare_draw_glue::install();
        crate::native_fog_quad_glue::install();
        crate::native_fog_fans_glue::install();
    }
    Ok(Installed {
        vblank,
        ui_layout: Arc::new(UiLayout),
        _owner: Some(thread),
        renderer: true,
    })
}

/// Where sealed frames go: the frame owner's mailbox.
#[cfg(feature = "gpu")]
static FRAMES: std::sync::OnceLock<frame_scene::mailbox::Publisher<native_render::Frame>> = std::sync::OnceLock::new();

/// Where captures go: the frame owner's thread.
#[cfg(feature = "gpu")]
static CAPTURES: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<native_render::frame_owner::Capture>>> =
    std::sync::OnceLock::new();

/// The newest frame drawn at `size` on the frame owner's thread (screenshots,
/// film thumbnails, `render.capture`). `None` with no renderer or before
/// the first frame.
#[cfg(feature = "gpu")]
pub fn capture(size: (u32, u32)) -> Option<native_render::gpu::Image> {
    let (reply, answer) = std::sync::mpsc::channel();
    CAPTURES.get()?.lock().unwrap_or_else(|e| e.into_inner()).send(native_render::frame_owner::Capture { size, reply }).ok()?;
    answer.recv().ok().flatten()
}

/// The frame step sealed `scene`: its frame, with the stages its glue
/// pushed, goes to the owner.
#[cfg(feature = "gpu")]
fn sealed(_: u64, scene: Arc<frame_scene::FrameScene>) {
    let present = gfx::present_camera::Present::Guest { image: gfx::present_camera::guest_image(), reference: false };
    let frame = native_render::frame_builder().seal(scene, present);
    if let Some(frames) = FRAMES.get() {
        let _ = frames.publish(frame);
    }
}

/// The scene timeline was cut: the frame being built is dropped.
#[cfg(feature = "gpu")]
fn reset() {
    native_render::frame_builder().discard();
}

/// The run report's render line.
pub fn describe(i: &Installed) -> String {
    format!(
        "render: {} guest D3D entries; {}; {} swaps",
        render_bridge::guest_entries::ENTRIES.len(),
        if i.renderer { "native renderer" } else { "no renderer" },
        render_bridge::frame_wait::swaps(),
    )
}

/// The UI fit and projection factors the retail UI callbacks set
/// (`crate::render_aspect`), for the native UI items to read.
#[cfg(feature = "gpu")]
pub struct UiLayout;

/// [`UiLayout`]'s values.
#[cfg(feature = "gpu")]
#[derive(Clone, Copy, Debug, Default)]
pub struct UiState {
    pub fit: Option<gfx::external::UiFit>,
    pub projection_factor: f32,
    pub camera_projection_factor: f32,
}

#[cfg(feature = "gpu")]
static UI: Mutex<UiState> = Mutex::new(UiState { fit: None, projection_factor: 1.0, camera_projection_factor: 1.0 });

/// The UI layout the retail callbacks last set.
#[cfg(feature = "gpu")]
pub fn ui_state() -> UiState {
    *UI.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(feature = "gpu")]
impl crate::render_aspect::UiLayoutSink for UiLayout {
    fn set_projection_factor(&self, factor: f32) {
        UI.lock().unwrap_or_else(|e| e.into_inner()).projection_factor = factor;
    }
    fn set_camera_projection_factor(&self, factor: f32) {
        UI.lock().unwrap_or_else(|e| e.into_inner()).camera_projection_factor = factor;
    }
    fn set_fit(&self, _: &mut runtime::Guest, fit: Option<gfx::external::UiFit>) {
        UI.lock().unwrap_or_else(|e| e.into_inner()).fit = fit;
    }
}

/// Network, film and lockstep runs read guest pixels back: the image the
/// guest reads must be the retail one.
#[cfg(feature = "gpu")]
pub fn protected_readbacks(network: bool, film: bool, lockstep: bool) -> bool {
    network || film || lockstep
}

#[cfg(feature = "gpu")]
/// The frame-rate setting as a display rate, read at every present.
pub type FrameRate = Box<dyn Fn() -> frame_scene::pace::Rate + Send>;

#[cfg(feature = "gpu")]
/// The Frame Rate row (`graphics.fps`, `H2_FPS`, `--fps`) for the frame
/// owner's display-rate loop (docs/graphics/framescene.md, section 7):
/// above 30 the owner draws display frames between scenes; 30 draws each
/// scene once. The setting is read live.
pub fn frame_rate(settings: Option<&Arc<settings::Store>>) -> FrameRate {
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
/// Another handle to `ctx`'s device and queue.
pub fn share_context(ctx: &gfx::GfxContext) -> gfx::GfxContext {
    gfx::GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

/// RGBA pixels to a PNG file.
pub fn write_png(path: &std::path::Path, px: &[u8], w: u32, h: u32) -> anyhow::Result<()> {
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    encode_png(f, px, w, h)
}

/// RGBA pixels as a PNG.
pub(crate) fn encode_png(mut output: impl std::io::Write, px: &[u8], w: u32, h: u32) -> anyhow::Result<()> {
    let mut enc = png::Encoder::new(&mut output, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(px)?;
    writer.finish()?;
    output.flush()?;
    Ok(())
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;

    #[test]
    fn fps_reads_uncapped_and_falls_back_to_30() {
        assert_eq!(fps_from(Some("uncapped")), 0);
        assert_eq!(fps_from(Some(" 60 ")), 60);
        assert_eq!(fps_from(Some("fast")), 30);
        assert_eq!(fps_from(None), 30);
    }
}
