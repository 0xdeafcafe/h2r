//! Installs crates/d3d through its own registration (`d3d::register`,
//! docs/graphics/d3d.md "Integration") on a headless RecordingBackend that keeps no
//! history ([`Headless`]), so the boot
//! tracer gets past device creation without a window (or on a headless
//! GfxBackend with `H2_BOOT_GFX`). Uploads are `UploadPolicy::Tracked`,
//! fed by the write barrier below.

use std::sync::{Arc, Mutex};

use d3d::{DeviceConfig, GuestRange, RecordingBackend, UploadPolicy};

pub const PUSH_BASE: u32 = 0xa0e0_0000;
pub const PUSH_LEN: u32 = 0x8_0000;
pub const SCRATCH_BASE: u32 = 0xa0e8_0000;

pub struct Installed {
    report: Box<dyn Fn() -> String + Send + Sync>,
    pub implemented: usize,
    pub stopping: usize,
    pub native: usize,
    /// The entries the write barrier wraps, in shadow-slot order.
    barrier: Vec<u32>,
    /// The vblank callback on the 60 Hz logical clock, for
    /// `Kernel::add_periodic` (taken once).
    pub vblank: Option<kernel::periodic::Handler>,
}

impl Installed {
    /// Where the write barrier moved `entry`'s real handler.
    pub fn barrier_shadow(&self, entry: u32) -> Option<u32> {
        self.barrier.iter().position(|&e| e == entry).map(|i| BARRIER_SHADOW + 4 * i as u32)
    }
}

/// `d3d::register` on a headless `RecordingBackend`, or, with
/// `H2_BOOT_GFX=<dir>`, on a headless `GfxBackend` with the nv2a shader
/// translator; then every `Swap` that shows a new image (at most one per
/// `H2_BOOT_GFX_EVERY` swaps, default 30) is written to `<dir>` as
/// `frame-NNNNN.png`. With `H2_RENDER_SCALE=N` (2..6) the frames come
/// from a view backend at N times the guest's size (`d3d::ScaledBackend`,
/// docs/graphics/high-resolution.md), `frame-NNNNN.png` is the view's image and
/// `frame-NNNNN-1x.png` the 1x reference's, and the run report gives the
/// view's counts. `H2_LIGHTING=e3` gives the view E3 lighting's scaled,
/// one-tap shadow maps (docs/graphics/e3-lighting.md), with a view at 1x
/// too when the scale is 1. `H2_BOOT_GFX_LIGHTING_AB=1` draws a retail and
/// an E3 view of the same run (`d3d::LightingAb`, at least 2x), compares
/// them on every shadow-map frame and writes differing pairs as
/// `frame-NNNNN-retail.png` and `frame-NNNNN-e3.png`. `H2_BOOT_GFX_SHADOWS=N` also writes the first
/// N frames that sample a shadow map (`d3d_diag::samples_shadow_map`).
/// Halo's frame wait goes through `d3d::register_frame_wait` either way.
pub fn install(rt: &runtime::Runtime) -> anyhow::Result<Installed> {
    match std::env::var_os("H2_BOOT_GFX") {
        None => install_with(rt, Headless::default()),
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir)?;
            // `never`: render on the GPU but write no screenshots (benchmarks).
            let every: u64 = match std::env::var("H2_BOOT_GFX_EVERY").ok().as_deref() {
                Some("never") => u64::MAX,
                v => v.and_then(|v| v.parse().ok()).filter(|&n| n > 0).unwrap_or(30),
            };
            let scale = d3d::render_scale().map_err(anyhow::Error::msg)?;
            let lighting = d3d::scaled::lighting().map_err(anyhow::Error::msg)?;
            let ctx = gfx::GfxContext::headless().map_err(|e| anyhow::anyhow!("gfx headless context: {e}"))?;
            // `H2_BOOT_GFX_SHADOWS=N`: also the first N frames that sample
            // a shadow map (`d3d_diag::samples_shadow_map`, the same frames
            // in every run of the same input) and show something, whatever
            // `every` says. A cinematic draws its lights' shadow passes
            // under a fade to black too; those frames are not written.
            let shadow_shots: u64 = std::env::var("H2_BOOT_GFX_SHADOWS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            // `shadow_limit`: how many shadow-map frames it may add.
            let shots = |suffix: &'static str, shadow_limit: u64| {
                let dir = dir.clone();
                let mut swaps = 0u64;
                let mut shadows = 0u64;
                move |ctx: &gfx::GfxContext, store: &gfx::surface::SurfaceStore, id, _flags, _gamma: Option<&[u8; 768]>| {
                    swaps += 1;
                    let regular = (swaps == 1 && every != u64::MAX) || swaps.is_multiple_of(every);
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
            if std::env::var_os("H2_BOOT_GFX_LIGHTING_AB").is_some_and(|v| v != "0") {
                // One run, both lighting modes: the 1x reference answers the
                // guest; a retail view and an E3 view draw the same calls at
                // `scale` (`d3d::LightingAb`). `frame-N-retail.png` and
                // `frame-N-e3.png` are a pair from the same frame.
                let scale = scale.max(2);
                let reference = configure_gfx(d3d::GfxBackend::new(share_context(&ctx)).on_present(shots("-1x", 0)));
                // Every shadow-map frame is compared between the views; the
                // first `H2_BOOT_GFX_SHADOWS` (default 40) that differ are
                // written as pairs. `H2_BOOT_GFX_EVERY` writes both views'
                // frames as usual.
                let ab = Arc::new(Mutex::new(AbCompare::default()));
                let pairs = if shadow_shots == 0 { 40 } else { shadow_shots };
                let mut retail_shots = shots("-retail", 0);
                let held = Arc::clone(&ab);
                let mut retail_swaps = 0u64;
                let retail_hook = move |ctx: &gfx::GfxContext, store: &gfx::surface::SurfaceStore, id, flags, gamma: Option<&[u8; 768]>| {
                    retail_swaps += 1;
                    if crate::d3d_diag::samples_shadow_map(retail_swaps) {
                        match read_rgba(ctx, store, id) {
                            Ok((px, _, _)) => held.lock().unwrap().pending = Some((retail_swaps, px)),
                            Err(e) => eprintln!("boot: lighting A/B {retail_swaps}: {e}"),
                        }
                    }
                    retail_shots(ctx, store, id, flags, gamma);
                };
                let mut e3_shots = shots("-e3", 0);
                let held = Arc::clone(&ab);
                let (dir_ab, mut e3_swaps, mut written) = (dir.clone(), 0u64, 0u64);
                let e3_hook = move |ctx: &gfx::GfxContext, store: &gfx::surface::SurfaceStore, id, flags, gamma: Option<&[u8; 768]>| {
                    e3_swaps += 1;
                    if crate::d3d_diag::samples_shadow_map(e3_swaps) {
                        let result = read_rgba(ctx, store, id).and_then(|(px, w, h)| {
                            let retail = held.lock().unwrap().compare(e3_swaps, &px);
                            if let Some(retail) = retail.filter(|_| written < pairs) {
                                written += 1;
                                write_png(&dir_ab.join(format!("frame-{e3_swaps:05}-retail.png")), &retail, w, h)?;
                                write_png(&dir_ab.join(format!("frame-{e3_swaps:05}-e3.png")), &px, w, h)?;
                            }
                            Ok(())
                        });
                        if let Err(e) = result {
                            eprintln!("boot: lighting A/B {e3_swaps}: {e}");
                        }
                    }
                    e3_shots(ctx, store, id, flags, gamma);
                };
                let retail = configure_gfx(d3d::GfxBackend::new(share_context(&ctx)).on_present(retail_hook));
                let e3 = configure_gfx(d3d::GfxBackend::new(ctx).on_present(e3_hook));
                let mut b = d3d::lighting_ab(reference, retail, e3, scale).map_err(anyhow::Error::msg)?;
                eprintln!("d3d: lighting A/B at {scale}x (H2_BOOT_GFX_LIGHTING_AB): a retail and an E3 view of one run; the guest sees a 1x reference");
                let (retail_report, e3_report) = (scale_report(b.reference_mut()), scale_report(&mut b));
                let report: ExtraReport = Box::new(move || {
                    format!("retail view: {}\ne3 view: {}\n{}", retail_report(), e3_report(), ab.lock().unwrap().report())
                });
                return if render_thread_requested() {
                    install_with_report(rt, d3d::ThreadedBackend::new(b)?, Some(report))
                } else {
                    install_with_report(rt, b, Some(report))
                };
            }
            if scale > 1 || lighting != d3d::Lighting::Retail {
                // The reference answers the guest at 1x; the view, on the
                // same device, renders at `scale` and presents.
                let reference = configure_gfx(d3d::GfxBackend::new(share_context(&ctx)).on_present(shots("-1x", shadow_shots)));
                let view = configure_gfx(d3d::GfxBackend::new(ctx).on_present(shots("", shadow_shots)));
                let mut b = d3d::ScaledBackend::new(reference, view, scale).map_err(anyhow::Error::msg)?;
                b.set_lighting(lighting);
                eprintln!("d3d: rendering at {scale}x (H2_RENDER_SCALE), {lighting:?} lighting (H2_LIGHTING); the guest sees a 1x reference");
                let report = scale_report(&mut b);
                return if render_thread_requested() {
                    install_with_report(rt, d3d::ThreadedBackend::new(b)?, Some(report))
                } else {
                    install_with_report(rt, b, Some(report))
                };
            }
            let backend = configure_gfx(d3d::GfxBackend::new(ctx).on_present(shots("", shadow_shots)));
            // Backend work (shader preparation, encoding, uploads,
            // submission, present) on a render thread of its own
            // (docs/graphics/render-thread.md), unless H2_RENDER_THREAD=0.
            if render_thread_requested() {
                return install_with(rt, d3d::ThreadedBackend::new(backend)?);
            }
            install_with(rt, backend)
        }
    }
}

/// `h2 run`'s settings for a headless `GfxBackend`.
fn configure_gfx(mut backend: d3d::GfxBackend) -> d3d::GfxBackend {
    // Merged render passes are the default, with the fast rule
    // (alpha-tested draws merged too; a few edge pixels can differ
    // from one render pass per draw). H2_GFX_MERGE_PASSES=exact
    // gives the exact rule (read by GfxBackend::new) and =0 one
    // render pass per draw (about 3x slower), as escape hatches.
    if std::env::var("H2_GFX_MERGE_PASSES").is_ok_and(|v| v == "0") {
        backend.set_merge_passes(false);
    }
    // H2_GFX_MERGE_CHECK=N: compare merged and per-draw passes in
    // frame N and print what differs.
    backend.set_merge_check(std::env::var("H2_GFX_MERGE_CHECK").ok().and_then(|v| v.parse().ok()));
    // The persistent pipeline cache (H2_PIPELINE_CACHE, default in
    // the user's cache directory): prewarm last run's pipelines.
    match d3d::pipeline_store::default_path() {
        Some(path) => backend.with_pipeline_cache(path),
        None => backend,
    }
}

/// Another handle to `ctx`'s device and queue, for a second backend on it.
fn share_context(ctx: &gfx::GfxContext) -> gfx::GfxContext {
    gfx::GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

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
        format!("render scale {}: {s}", s.scale)
    })
}

/// An extra line for [`describe`].
pub type ExtraReport = Box<dyn Fn() -> String + Send + Sync>;

/// The GPU backend's work runs on a render thread (`h2 run` with
/// `H2_BOOT_GFX`, and `h2 play`) unless `H2_RENDER_THREAD=0`. On by
/// default (docs/graphics/render-thread.md, "Measurements").
pub fn render_thread_requested() -> bool {
    render_thread_from(std::env::var("H2_RENDER_THREAD").ok().as_deref())
}

/// The frame-rate setting's reader for the interpolated replay
/// (`d3d::InterpBackend`, docs/game/high-framerate.md "Phase 2"), when it
/// is asked for: `impl.render.interpolate` in `H2_FEATURES` (that exact
/// switch; `impl.render=on` does not include it). It needs the render
/// thread, and is off under `H2_PIPELINE_RENDER` (the lockstep harness
/// cannot certify that combination). The rate follows the Frame Rate row
/// (`graphics.fps`, `H2_FPS`, `--fps`): 30 replays nothing.
pub fn interpolation(
    settings: Option<&std::sync::Arc<settings::Store>>,
    render_thread: bool,
) -> Option<Box<dyn Fn() -> u32 + Send>> {
    let switches = feature::Switches::from_env().ok()?;
    interpolation_from(
        switches.feature("impl.render.interpolate", false),
        render_thread,
        std::env::var("H2_PIPELINE_RENDER").ok().as_deref(),
    )
    .then(|| -> Box<dyn Fn() -> u32 + Send> {
        eprintln!(
            "d3d: interpolated replay on (impl.render.interpolate): in-between frames at up to {} fps while the frame rate is above 30",
            d3d::interp::pacing::MAX_REPLAY_HZ
        );
        match settings {
            Some(store) => {
                let fps = store.handle::<u32>(crate::setting_defs::FPS.key);
                Box::new(move || fps.get())
            }
            None => {
                let fps = fps_from(std::env::var("H2_FPS").ok().as_deref());
                Box::new(move || fps)
            }
        }
    })
}

/// [`interpolation`]'s decision: the switch, the render thread, and
/// `H2_PIPELINE_RENDER` unset or `0`.
pub fn interpolation_from(switch: bool, render_thread: bool, pipeline_render: Option<&str>) -> bool {
    if !switch {
        return false;
    }
    if !render_thread {
        eprintln!("d3d: impl.render.interpolate needs the render thread (H2_RENDER_THREAD=0 is set): off");
        return false;
    }
    if pipeline_render.is_some_and(|v| v != "0") {
        eprintln!("d3d: impl.render.interpolate is off under H2_PIPELINE_RENDER");
        return false;
    }
    true
}

/// `H2_FPS` as the setting stores it: 0 for uncapped, 30 when unset or
/// unreadable.
pub fn fps_from(value: Option<&str>) -> u32 {
    match value.map(str::trim) {
        Some("uncapped" | "unlimited" | "max") => 0,
        Some(v) => v.parse().unwrap_or(30),
        None => 30,
    }
}

/// `backend` on a render thread, behind the interpolated replay when
/// `interp` (from [`interpolation`]) is given.
pub fn install_threaded<B>(
    rt: &runtime::Runtime,
    backend: B,
    extra: Option<ExtraReport>,
    interp: Option<Box<dyn Fn() -> u32 + Send>>,
) -> anyhow::Result<Installed>
where
    B: d3d::backend::Backend + Send + 'static,
    B::Texture: Sync,
{
    match interp {
        Some(rate) => install_with_report(rt, d3d::ThreadedBackend::new(d3d::InterpBackend::new(backend, rate))?, extra),
        None => install_with_report(rt, d3d::ThreadedBackend::new(backend)?, extra),
    }
}

/// [`render_thread_requested`] for a given value of the variable: only
/// `0` turns it off.
pub fn render_thread_from(value: Option<&str>) -> bool {
    value != Some("0")
}

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

/// Surface `id`'s pixels as RGBA with opaque alpha, and its size.
fn read_rgba(
    ctx: &gfx::GfxContext,
    store: &gfx::surface::SurfaceStore,
    id: gfx::surface::SurfaceId,
) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let (mut px, order) = store.read_color(ctx, id).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let d = store.get(id).map_err(|e| anyhow::anyhow!("{e:?}"))?.desc();
    if matches!(order, gfx::surface::HostPixelOrder::Bgra8) {
        for p in px.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
    }
    for p in px.as_chunks_mut::<4>().0 {
        p[3] = 0xff; // X8R8G8B8 back buffer: alpha is undefined
    }
    Ok((px, d.width, d.height))
}

fn write_png(path: &std::path::Path, px: &[u8], w: u32, h: u32) -> anyhow::Result<()> {
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(f, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(px)?;
    Ok(())
}

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
            if first.is_empty() { String::new() } else { format!(" (first: {})", first.join(", ")) }
        )
    }
}

/// The largest per-channel change the A/B treats as rounding: the two
/// views take different shader variants, which can round a channel by one.
const AB_NOISE: u8 = 2;

/// Whether RGBA images `a` and `b` differ in some colour channel by more
/// than `AB_NOISE` (alpha aside).
fn differs_beyond_noise(a: &[u8], b: &[u8]) -> bool {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .any(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > AB_NOISE))
}

/// Whether RGBA pixels `px` are all black (alpha aside).
fn all_black(px: &[u8]) -> bool {
    px.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 0, 0])
}

/// The headless default: a `RecordingBackend` that forgets every call once
/// it returns, except the surface creations and destructions its
/// `destroy_surface` checks against. Without this the recorder would hold
/// a copy of every draw (streams included) for the whole run; the report's
/// draw counts come from `DiagBackend`.
#[derive(Default)]
pub struct Headless(RecordingBackend);

impl Headless {
    fn forget<T>(&mut self, r: T) -> T {
        use d3d::backend::Recorded;
        if self.0.calls.last().is_some_and(|c| !matches!(c, Recorded::Surface { .. } | Recorded::DestroySurface { .. })) {
            self.0.calls.pop();
        }
        r
    }

    /// Calls still held (surface lifetimes only).
    pub fn held(&self) -> usize {
        self.0.calls.len()
    }
}

impl d3d::backend::Backend for Headless {
    type Surface = u32;
    type Texture = u32;

    fn create_surface(&mut self, format: gfx::D3DFormat, width: u32, height: u32, depth: bool) -> Result<u32, String> {
        self.0.create_surface(format, width, height, depth)
    }
    fn destroy_surface(&mut self, surface: u32) -> Result<(), String> {
        self.0.destroy_surface(surface)
    }
    fn upload_texture(&mut self, texture: &gfx::texture::DecodedTexture) -> Result<u32, String> {
        let r = self.0.upload_texture(texture);
        self.forget(r)
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
    ) -> (Result<d3d::backend::DrawOutcome, String>, Vec<d3d::backend::StreamBytes>) {
        let r = self.0.draw_owned(color, depth, draw, textures);
        self.forget(r)
    }
    fn frame_stats(&mut self) {
        self.0.frame_stats()
    }
    fn present(&mut self, surface: u32, flags: u32) -> Result<(), String> {
        let r = self.0.present(surface, flags);
        self.forget(r)
    }
    fn present_with_gamma(&mut self, surface: u32, flags: u32, gamma: Option<&[u8; 768]>) -> Result<(), String> {
        let r = self.0.present_with_gamma(surface, flags, gamma);
        self.forget(r)
    }
    fn clear_rects(&mut self, color: u32, depth: Option<u32>, clear: &d3d::backend::RectClear) -> Result<d3d::backend::DrawOutcome, String> {
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
    fn write_surface(&mut self, surface: u32, desc: &gfx::texture::TextureDesc, bytes: &[u8]) -> Result<bool, String> {
        let r = self.0.write_surface(surface, desc, bytes);
        self.forget(r)
    }
    fn snapshot_surface(&mut self, surface: u32, opaque: bool) -> Result<Option<u32>, String> {
        let r = self.0.snapshot_surface(surface, opaque);
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
    fn create_texture_target(&mut self, format: gfx::D3DFormat, width: u32, height: u32) -> Result<u32, String> {
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
    fn mirror_store(&mut self, surface: u32, format: gfx::D3DFormat, width: u32, height: u32, layout: d3d::backend::GuestLayoutKind, offset: u32) -> Result<(), String> {
        self.0.mirror_store(surface, format, width, height, layout, offset)
    }
    fn mirror_load(&mut self, surface: u32, format: gfx::D3DFormat, width: u32, height: u32, layout: d3d::backend::GuestLayoutKind, offset: u32) -> Result<(), String> {
        self.0.mirror_load(surface, format, width, height, layout, offset)
    }
    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.0.mirror_read(offset, len)
    }
    fn defers_depth_aliases(&self) -> bool {
        self.0.defers_depth_aliases()
    }
    fn judge(&self) -> Option<Box<dyn d3d::backend::DrawJudge>> {
        self.0.judge()
    }
    #[allow(clippy::too_many_arguments)]
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
        self.0.depth_alias_texture(depth, format, width, height, pitch, generation, offset, desc, options)
    }
}

/// [`install`] on the caller's backend (a host window's `GfxBackend`, with
/// its present hook already attached).
pub fn install_with<B: d3d::backend::Backend + Send + 'static>(rt: &runtime::Runtime, backend: B) -> anyhow::Result<Installed> {
    install_with_report(rt, backend, None)
}

/// [`install_with`], with `extra` added to the run report ([`describe`]).
pub fn install_with_report<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    backend: B,
    extra: Option<ExtraReport>,
) -> anyhow::Result<Installed> {
    let (backend, draw_stats) = crate::d3d_diag::DiagBackend::new(backend);
    // Tracked uploads trust the write barrier below; H2_BOOT_UPLOAD_ALWAYS
    // goes back to re-reading every resource on every use.
    let upload = if std::env::var_os("H2_BOOT_UPLOAD_ALWAYS").is_some() {
        UploadPolicy::Always
    } else {
        UploadPolicy::Tracked
    };
    let cfg = DeviceConfig {
        upload,
        nv2a_window: Some(NV2A_BASE),
        ..DeviceConfig::halo2(
            GuestRange { addr: PUSH_BASE, len: PUSH_LEN },
            GuestRange { addr: SCRATCH_BASE, len: 0x1000 },
        )
    };
    d3d::runtime_glue::map_device_memory(&mut rt.memory(), &cfg)?;
    let device = d3d::runtime_glue::shared(cfg, backend);
    install_occlusion_capture(&device);
    let mut out = None;
    rt.edit_dispatch(|d| {
        out = Some(d3d::register(d, Arc::clone(&device)));
        // Halo's frame wait 0x12b2e0 ("Vertical blank" in docs/graphics/d3d.md):
        // calls the vblank callback at device+0x1db8 until the counter
        // reaches the target, unpaced. Generated code would otherwise spin
        // on the counter natively and never return to the runner.
        d3d::register_frame_wait(
            d,
            Arc::clone(&device),
            d3d::FrameWait::HALO2_CLOCKED,
            Arc::new(d3d::runtime_glue::default_stop),
        );
    });
    let r = out.unwrap();
    let entries: Vec<u32> = r.implemented.iter().chain(&r.stopping).map(|e| e.0).collect();
    install_write_barrier(rt, &device, &entries)?;
    let vblank: Option<kernel::periodic::Handler> = Some(Box::new(d3d::vblank::clocked_vblanks(Arc::clone(&device))));
    let dev = device;
    Ok(Installed {
        vblank,
        report: Box::new(move || {
            let mut d = dev.lock().unwrap();
            // Device events by kind and reason, most frequent first.
            let mut ev: std::collections::BTreeMap<String, u64> = Default::default();
            for e in d.take_events() {
                let k = match e {
                    d3d::Event::DrawSkipped { entry, reason } => format!("draw skipped in {entry}: {reason}"),
                    d3d::Event::BadState(b) => format!("bad state {b:?}"),
                    other => format!("{other:?}"),
                };
                let k: String = k.chars().take(200).collect();
                *ev.entry(k).or_default() += 1;
            }
            let mut ev: Vec<_> = ev.into_iter().collect();
            ev.sort_by_key(|e| std::cmp::Reverse(e.1));
            let ev: String = ev.iter().take(40).map(|(k, n)| format!("\n  event {n:>8}  {k}")).collect();
            let extra = extra.as_ref().map(|f| format!("\n{}", f())).unwrap_or_default();
            format!("stats {:?}; last stop {:?}{ev}\n{}{extra}", d.stats(), d.last_stop(), draw_stats.lock().unwrap().report())
        }),
        implemented: r.implemented.len(),
        stopping: r.stopping.len(),
        native: r.native.len(),
        barrier: entries,
    })
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



/// The NV2A register window crates/d3d maps and stores at device+0x934
/// (`DeviceConfig::nv2a_window`): the retail 0xfd000000 cannot be used, as
/// the runtime keeps the NV2A aperture free of RAM mappings.
pub const NV2A_BASE: u32 = 0xa040_0000;

/// Shadow page for the displaced crates/d3d handlers the boot wraps.
const D3D_SHADOW: u32 = 0xa0ff_2000;
const CREATE_DEVICE: u32 = 0x003f_5240;

/// Halo creates the device, releases it (the caller zeroes the device
/// object) and creates it again, so the back and depth surfaces are backed
/// after every Direct3D_CreateDevice ([`back_surfaces`]): crates/d3d's
/// handler moves to a shadow address and a continuation runs once it
/// returns.
pub fn install_back_surfaces(rt: &runtime::Runtime) -> anyhow::Result<()> {
    rt.memory().map_zeroed(D3D_SHADOW, 0x1000, runtime::guest::Permissions::RX)?;
    let device = d3d::GuestLayout::HALO2.device;
    let shadow = D3D_SHADOW;
    rt.edit_dispatch(|d| {
        let prev = d.register_override(CREATE_DEVICE, "boot:CreateDevice+back surfaces", move |cpu, ctx, _| {
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
        });
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
    let rs: Vec<_> = mem.physical_reservations().iter().map(|r| (r.id, r.physical_start, r.len)).collect();
    for (id, start, len) in rs {
        mem.map_reserved(id, WC_BASE | start, len, runtime::guest::Permissions::RW)?;
    }
    Ok(())
}

pub fn map_write_combined(mem: &mut runtime::Guest, va: u32, len: u32) -> anyhow::Result<()> {
    let m = mem
        .mappings()
        .iter()
        .find(|m| m.virtual_start <= va && va as u64 + len as u64 <= m.virtual_start as u64 + m.len as u64)
        .map(|m| m.physical_start + (va - m.virtual_start))
        .ok_or_else(|| anyhow::anyhow!("{va:#x}: not one mapping"))?;
    let phys = m;
    mem.map(0xf000_0000 | (va & 0x0fff_ffff), len, phys, runtime::guest::Permissions::RW)?;
    Ok(())
}

/// crates/d3d's CreateDevice writes the back-buffer and depth surface
/// headers with `Data = 0` (no guest backing; the host surface is the real
/// one), so Halo's CPU writes into the back buffer (0x13d80, a full-screen
/// blit right after the first map load) land at 0xf0000000. Boot stopgap:
/// give each header that still has `Data = 0` its own zeroed pages, at a
/// physical address P mapped at both `0x80000000 | P` and `0xf0000000 |
/// P`, and store P as Data. Owner: crates/d3d (guest-backed surfaces).
pub fn back_surfaces(mem: &mut runtime::Guest, device: u32) {
    use d3d::layout::device_field as field;
    for f in [field::BACK_BUFFER, field::DEPTH_STENCIL] {
        let Ok(header) = mem.u32(device + f) else { continue };
        if header == 0 || mem.u32(header + 4).is_ok_and(|d| d != 0) {
            continue;
        }
        let Ok(size) = mem.u32(header + 16) else { continue };
        let (w, h, pitch) = ((size & 0xfff) + 1, ((size >> 12) & 0xfff) + 1, ((size >> 24) + 1) * 64);
        let len = (pitch * h).max(w * 4).next_multiple_of(0x1000);
        let Some(phys) = mem.find_free_physical(len, 0..0x0400_0000, 0x1_0000) else {
            crate::log::note(&format!("back surface {header:#x}: no free physical memory"));
            continue;
        };
        if mem.map(0x8000_0000 | phys, len, phys, runtime::guest::Permissions::RW).is_err() {
            crate::log::note(&format!("back surface {header:#x}: no room at {phys:#x}"));
            continue;
        }
        let _ = mem.map(0xf000_0000 | phys, len, phys, runtime::guest::Permissions::RW);
        let _ = mem.write_bytes(0x8000_0000 | phys, &vec![0; len as usize], 1);
        let _ = mem.set_u32(header + 4, phys);
    }
}


/// Shadow page for the d3d entries the write barrier wraps (4 bytes each).
const BARRIER_SHADOW: u32 = 0xa0ff_4000;

/// The write barrier's write-watch owner: it drains its subscription by id
/// through whichever Guest the entry runs against, so a pipelined render
/// thread takes it over at each snapshot (`pipeline-render`).
pub const WATCH_OWNER: &str = "boot:d3d";

/// `pipeline-render` (docs/graphics/pipelined-render.md, "The prototype")
/// with this D3D layer: the game's D3D entries (the sim's calls there join
/// the render first), and every D3D override the render thread may run
/// (the entries, the barrier's and CreateDevice's shadows, the port I/O
/// stubs). Off unless `H2_PIPELINE_RENDER=1`.
pub fn pipeline_render(i: Option<&Installed>) -> feature::pipeline_render::PipelineRender {
    let entries: Vec<u32> = match i {
        Some(i) => i.barrier.clone(),
        None => Vec::new(),
    };
    let allowed: std::collections::BTreeSet<u32> =
        entries.iter().copied().chain(d3d::abi::PORT_IO.iter().map(|p| p.0)).collect();
    let shadows = [BARRIER_SHADOW, D3D_SHADOW];
    feature::pipeline_render::PipelineRender::new().with_d3d(
        entries,
        move |addr, _| allowed.contains(&addr) || shadows.iter().any(|&s| (s..s + 0x1000).contains(&addr)),
        &[WATCH_OWNER],
    )
}

/// The D3D write barrier (docs/graphics/d3d.md "Integration", step 3) on the
/// runtime's write watch: one subscriber watches all of RAM in the folded
/// view, and every d3d entry is wrapped so that
///
/// - on entry, the pages written since the last drain (lifted code, the
///   interpreter, kernel and HLE copies) go to
///   `Device::note_written_pages`, before the device reads any resource;
/// - nothing is drained on return. The entry is a guest call: runner safe
///   points, interrupts and other contexts can store between the entry
///   drain and the return, and dropping the pages then would lose those
///   stores. The device's own stores are therefore reported back to it at
///   the next entry, which costs a re-check of a page it already has (its
///   render-target writebacks it invalidates itself).
///
/// The device's tracker keeps only the pages it watches, so watching all of
/// RAM here costs a bit per store and nothing more.
fn install_write_barrier<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    device: &Arc<Mutex<d3d::Device<B>>>,
    entries: &[u32],
) -> anyhow::Result<()> {
    use runtime::watch::CONTIGUOUS_BASE;
    let id = {
        let mut mem = rt.memory();
        mem.map_zeroed(BARRIER_SHADOW, 0x1000, runtime::guest::Permissions::RX)?;
        let id = mem.subscribe_writes(WATCH_OWNER);
        mem.watch_writes(id, CONTIGUOUS_BASE, 0x0400_0000).map_err(|e| anyhow::anyhow!("{e}"))?;
        id
    };
    anyhow::ensure!(entries.len() <= 0x400, "too many d3d entries for the shadow page");
    rt.edit_dispatch(|d| {
        for (i, &addr) in entries.iter().enumerate() {
            let shadow = BARRIER_SHADOW + 4 * i as u32;
            let dev = Arc::clone(device);
            let Some(prev) = d.register_override(addr, "boot:d3d write barrier", move |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let dirty = mem.take_dirty(id).unwrap_or_default();
                if !dirty.is_empty() {
                    let mut dv = dev.lock().unwrap();
                    for r in dirty {
                        dv.note_written_pages(r.addr, r.len);
                    }
                }
                let ret_pc = mem.u32(cpu.esp)?;
                cpu.esp = cpu.esp.wrapping_add(4);
                ctx.call_guest(cpu, &mut mem, shadow, move |_, _| Ok(runtime::TransferAbi::ret(ret_pc)))
            }) else {
                continue;
            };
            d.restore_override(shadow, prev);
        }
    });
    Ok(())
}

/// The occlusion feature's depth capture (docs/graphics/occlusion-culling.md):
/// its observer asks the device for a grid of the main depth, recorded in
/// order with the draws; the finished grid goes back to the feature with
/// the W-buffer's mapping to view depth. Inert unless
/// `impl.render.occlusion` is on (nothing asks).
fn install_occlusion_capture<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let dev = Arc::clone(device);
    feature::occlusion::set_capture(Box::new(move |tag, width, height| {
        let Ok(mut d) = dev.lock() else { return };
        d.request_depth_grid(
            tag,
            width,
            height,
            Arc::new(|r: d3d::DepthGridResult, s: d3d::DepthScale| {
                feature::occlusion::deliver(r.tag, r.width, r.height, &r.depth, w_buffer_mapping(s));
            }),
        );
    }));
}

/// The W-buffer's raw depth to view depth: stored `floor(w · inv_w_far ·
/// z_scale) / z_scale` (`z_scale` the format's full scale, `+0x948`).
pub fn w_buffer_mapping(s: d3d::DepthScale) -> engine::render_loop::occlusion::DepthMapping {
    engine::render_loop::occlusion::DepthMapping::WBuffer {
        zmax: s.z_scale,
        scale: s.inv_w_far * s.z_scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d3d::backend::Backend;

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

    #[test]
    fn interpolation_needs_its_switch_and_the_render_thread() {
        assert!(!interpolation_from(false, true, None), "off by default");
        assert!(interpolation_from(true, true, None));
        assert!(interpolation_from(true, true, Some("0")));
        assert!(!interpolation_from(true, false, None), "no render thread");
        assert!(!interpolation_from(true, true, Some("1")), "pipelined render");
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
        let s = b.create_surface(gfx::D3DFormat::LinA8R8G8B8, 64, 64, false).unwrap();
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
            b.clear(s, None, gfx::Clear { color: Some([0.0; 4]), depth: None, stencil: None }).unwrap();
            b.present(s, 0).unwrap();
        }
        assert_eq!(b.held(), 1, "only the surface creation is held");
        b.destroy_surface(s).unwrap();
        assert!(b.destroy_surface(s).is_err(), "destroy is still checked");
        assert_eq!(b.held(), 2);
    }
}
