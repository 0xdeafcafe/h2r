//! Rendering above the guest's resolution without the guest seeing it
//! (docs/graphics/high-resolution.md, phases 1 and 2).
//!
//! At scales above 1, single-copy mode draws into the display backend.
//! It also holds shared handles to guest-sized targets, so all GPU draws
//! and samples are ordered in one stream. CPU reads resolve on demand.
//! The following describes the exact two-copy comparison mode.
//!
//! [`ScaledBackend`] holds two backends that get the same calls. The
//! *reference* renders at the guest's size and answers everything the guest
//! can observe: surface and depth read-backs, pixel counts, draw outcomes,
//! the judge and every capability question. The *view* renders the frame
//! buffers and the depth buffer at an integer multiple of their size and is
//! what is shown. Nothing the view does reaches the device, so the guest's
//! values are the 1x run's by construction.
//!
//! What the view keeps at scale: every surface made at the frame size (the
//! size of the first colour surface, the back buffer), with
//! [`Backend::create_surface`] or as a render-to-texture level
//! ([`Backend::create_texture_target`], an intermediate like Halo's
//! texaccum): the frame buffers, the depth buffer, copies of them and
//! back-buffer-sized targets. Their snapshots are copies of the scaled
//! image. Every other surface (smaller render-to-texture levels, other
//! sizes) lives only in the reference, and the view samples the
//! reference's texture of it (the two share a device), so offscreen passes
//! run once. Textures are uploaded once and shared the same way. A texture
//! over the whole depth buffer is made from the view's scaled depth when the
//! view can make it on the GPU; other depth textures are the reference's.
//!
//! The view draws everything the reference draws into a scaled target. What
//! it cannot draw the same way it repairs from the reference, per draw:
//!
//! - a draw into a guest-size target with the scaled depth buffer bound
//!   that writes depth or stencil is drawn again into the view's depth,
//!   through a scaled colour surface of the depth's size with colour writes
//!   off ([`ScaleStats::proxy_depth_draws`]);
//! - guest bytes written into a scaled surface, colour or depth, are copied
//!   from the reference's, each guest pixel repeated;
//! - a draw the view cannot make, or makes with another outcome (a texture
//!   it cannot sample, a depth surface that is not scaled), is replaced by
//!   the reference's target, each guest pixel repeated
//!   ([`ScaleStats::color_resyncs`], [`ScaleStats::depth_resyncs`]). That
//!   target is then at 1x until it is drawn again, but it has no holes.
//!
//! Each kind of repair is logged once. Stipple is still indexed by host
//! pixel.
//!
//! A view failure never reaches the guest: it is counted and that frame is
//! shown from the reference's image.
//!
//! **Changing the scale during a run.** [`ScaledBackend::follow_scale`]
//! gives it a source of the wanted scale (the Video screen's Resolution
//! setting), read at every present. A change is made at that frame
//! boundary, on whichever thread presents (the render thread under
//! [`crate::ThreadedBackend`]): every scaled surface is made again at the
//! new scale and filled from the reference's, and the device's handles are
//! mapped onto the new ones, so the guest and the device see nothing. At
//! scale 1 the view sleeps: it draws nothing and each frame is the
//! reference's ([`ScaleStats::reference_frames`]), so a run can start at
//! 1x and go up without a second backend's work until it does.
//!
//! **The Lighting row** ([`Lighting`], [`ScaledBackend::follow_lighting`],
//! docs/graphics/e3-lighting-setting.md): harder retail shadows, in the view only.
//! Every `D24S8` depth level not of the frame's size (a shadow map) gets a
//! view copy at [`shadow_scale`] times its size, made at its first use,
//! with a colour surface of that size to draw it through. Draws that write
//! the level into a guest-size target are drawn again into the copy,
//! colour writes off ([`ScaleStats::shadow_draws`]), and a draw that
//! samples the level reads the copy ([`ScaleStats::shadow_textures`]); the
//! view's shaders compare it with one tap (`ShaderProvider::set_hard_shadows`).
//! What the view cannot repeat (guest bytes, a copy, a fill from the alias
//! mirror, a draw it cannot make) leaves the copy stale, and a stale copy
//! is filled from the reference at its next use (each guest pixel
//! repeated; [`ScaleStats::shadow_refills`]) unless a clear makes it fresh
//! first. The reference keeps every level at 1x and answers every read of
//! it, so the guest sees retail's bytes. With E3 lighting the view is
//! awake at render scale 1 too (frame buffers at 1x, shadow maps at 2x).

mod single;

use std::sync::{Arc, Mutex};

use gfx::state::{ColorWriteMask, DepthBufferMode, StencilOp};
use gfx::texture::{DecodeOptions, DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::backend::{Backend, DrawJudge, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw};
use crate::gfx_backend::{GfxBackend, HostTexture};

pub use crate::{Lighting, MAX_SCALE};

/// `H2_RENDER_SCALE`: unset, empty or `1` is 1 (no view backend at all);
/// `2`..=`6` render the frame at that multiple.
pub fn render_scale_from(value: Option<&str>) -> Result<u32, String> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(1);
    };
    match v.parse::<u32>() {
        Ok(n) if (1..=MAX_SCALE).contains(&n) => Ok(n),
        _ => Err(format!(
            "H2_RENDER_SCALE={v:?}: expected a whole number from 1 to {MAX_SCALE}"
        )),
    }
}

/// [`render_scale_from`] of the environment.
pub fn render_scale() -> Result<u32, String> {
    render_scale_from(std::env::var("H2_RENDER_SCALE").ok().as_deref())
}

/// `H2_LIGHTING`: unset, empty or `retail` is [`Lighting::Retail`]; `e3`
/// is [`Lighting::E3`].
pub fn lighting_from(value: Option<&str>) -> Result<Lighting, String> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(Lighting::Retail),
        Some(v) if v.eq_ignore_ascii_case("retail") => Ok(Lighting::Retail),
        Some(v) if v.eq_ignore_ascii_case("e3") => Ok(Lighting::E3),
        Some(v) => Err(format!("H2_LIGHTING={v:?}: expected retail or e3")),
    }
}

/// [`lighting_from`] of the environment.
pub fn lighting() -> Result<Lighting, String> {
    lighting_from(std::env::var("H2_LIGHTING").ok().as_deref())
}

/// The factor the view's shadow maps are made at, for render scale
/// `scale`: 1 (none made) for retail lighting. For E3 lighting the render
/// scale, so a shadow texel covers no more host pixels than it covers
/// guest pixels at 1x and a one-tap edge steps no coarser than retail's
/// filtered one; and at least 2, so it is sharper than retail at 1x too.
/// Tying it to the render scale keeps one setting and one rescale point;
/// 512x512 at 6x is 3072x3072, well inside a GPU's texture limit.
pub fn shadow_scale(scale: u32, lighting: Lighting) -> u32 {
    match lighting {
        Lighting::Retail => 1,
        Lighting::E3 => scale.clamp(2, MAX_SCALE),
    }
}

/// The view backend for `scale`, made only above 1: at 1 the run is
/// today's single backend, with no second one built.
pub fn view_for<V>(scale: u32, make: impl FnOnce() -> V) -> Option<V> {
    (scale > 1).then(make)
}

/// What a view backend needs beyond [`Backend`]: surfaces at a multiple of
/// their guest size, and access to its reference `R`'s resources (the two
/// are on one device).
pub trait ScaleView<R: Backend>: Backend {
    /// A surface `scale` times `width x height`. Draws and clears into it
    /// keep taking guest-pixel rectangles; the backend scales them.
    fn create_scaled_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
        scale: u32,
    ) -> Result<Self::Surface, String>;
    /// A render-to-texture colour target `scale` times `width x height`
    /// ([`Backend::create_texture_target`] at scale).
    fn create_scaled_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        scale: u32,
    ) -> Result<Self::Surface, String>;
    /// Single-copy operations (the nested lighting A/B uses exact copies).
    fn import_guest(&mut self, _reference: &R, _s: R::Surface) -> Result<Self::Surface, String> {
        Err("cannot import guest surface into display backend".into())
    }
    fn resize(&mut self, _src: Self::Surface, _dst: Self::Surface) -> Result<(), String> {
        Err("cannot resize display surface".into())
    }
    /// Copy a source image to the top-left of a target with another scale.
    fn copy_scaled(&mut self, _src: Self::Surface, _dst: Self::Surface, _extent: (u32, u32)) -> Result<(), String> {
        Err("cannot copy between render scales".into())
    }
    fn export_texture(&self, _texture: &Self::Texture) -> Result<R::Texture, String> {
        Err("cannot share display texture with guest backend".into())
    }
    fn enable_surface_aliases(&mut self, _reference: &mut R) {}
    fn move_counter(&mut self, _reference: &mut R, _to_display: bool) {}
    fn share_mirror(&mut self, _reference: &R) {}
    /// Counted draws stop counting (the reference answers every count).
    fn stop_counting(&mut self);
    /// This backend and `reference` sample each other's results: both
    /// submit side work as they record it
    /// ([`GfxBackend::set_defer_side_passes`]), so neither's submissions
    /// can run ahead of what the other recorded. The default does nothing.
    fn share_results(&mut self, _reference: &mut R) {}
    /// E3 lighting's one-tap shadow lookup for the draws prepared from now
    /// on (`crate::ShaderProvider::set_hard_shadows`). The default does
    /// nothing.
    fn set_hard_shadows(&mut self, _on: bool) {}
    /// This backend's handle for `reference`'s texture. `None`: it cannot
    /// sample it.
    fn share_texture(&mut self, reference: &R, texture: &R::Texture) -> Option<Self::Texture>;
    /// Submits work recorded so far, before the reference changes a
    /// surface this backend may be sampling.
    fn submit_recorded(&mut self) -> Result<(), String>;
    /// Fills scaled surface `dst` from `reference`'s guest-size surface
    /// `src` of the same kind (colour or depth/stencil), each guest pixel
    /// repeated. `Ok(false)`: it cannot (the depth cannot be read back).
    fn fill_from(
        &mut self,
        reference: &mut R,
        src: R::Surface,
        dst: Self::Surface,
    ) -> Result<bool, String>;
    /// Ends the frame showing `reference`'s `surface` instead of one of
    /// its own (the fallback after a view failure).
    fn present_reference(
        &mut self,
        reference: &R,
        surface: R::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String>;
}

/// A [`ScaledBackend`] surface: the reference's, and the view's when the
/// surface is scaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaledSurface<R, V> {
    /// The reference's surface.
    pub reference: R,
    /// The original view handle. Resolve it through [`ScaledBackend::view_surface`]
    /// before use, since a scale change replaces the view's allocation.
    /// `None` for a surface only the reference keeps.
    pub view: Option<V>,
    /// The view backend's handle on the reference's actual 1x allocation.
    guest: Option<V>,
    /// Guest size.
    pub size: (u32, u32),
}

impl<R, V: Copy> crate::ExternalSurfaces<ScaledSurface<R, V>> {
    /// None means the reference is the only required copy. A required
    /// display copy must resolve every attachment and sampled slot.
    fn display_copy(&self, awake: bool, frame: Option<(u32, u32)>,
        scaled_shadow: impl Fn(&R) -> bool, live: impl Fn(Option<V>) -> Option<V>,
    ) -> Result<Option<crate::ExternalSurfaces<V>>, crate::TargetCopy> {
        let needs_view = |s: &ScaledSurface<R, V>| {
            s.view.is_some() || frame == Some(s.size) || scaled_shadow(&s.reference)
        };
        if awake && (needs_view(&self.color) || needs_view(&self.depth)
            || self.slots.iter().flatten().any(needs_view)) {
            return self.try_map(|s| live(s.view)).map(Some).ok_or(crate::TargetCopy::Display);
        }
        Ok(None)
    }
}

/// A [`ScaledBackend`] texture.
#[derive(Debug)]
pub struct ScaledTexture<R, V> {
    /// A texture usable by the guest-size encoder. In single-copy mode
    /// this can be a shared handle to a scaled GPU result.
    pub reference: R,
    /// What the view samples: its own (a snapshot of a scaled surface) or
    /// the reference's, shared. `None`: the view cannot sample it.
    pub view: Option<V>,
}

/// Cumulative copy, transfer and readback counts for the scale report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScaleStats {
    /// Successful D3D draws and committed native ranges, by encoder.
    pub reference_draws: u64,
    pub display_draws: u64,
    /// On-demand transfers to guest dimensions.
    pub downsamples: u64,
    /// Completed CPU surface, mirror and pixel-count reads.
    pub cpu_readbacks: u64,
    /// View calls that failed.
    pub view_errors: u64,
    /// View draws and clears whose outcome differed from the reference's
    /// (each repaired from the reference: `color_resyncs`).
    pub mismatches: u64,
    /// Draws, clears and guest writes that changed the reference's depth
    /// buffer and could not be repeated in the view's (its depth cannot be
    /// read back to copy).
    pub depth_missed: u64,
    /// Frames shown from the reference after a view failure.
    pub fallback_frames: u64,
    /// Frames shown from the view.
    pub view_frames: u64,
    /// Scaled colour targets replaced by the reference's (a draw the view
    /// could not make the same way).
    pub color_resyncs: u64,
    /// Scaled depth surfaces replaced by the reference's (guest writes, or
    /// a depth-writing draw the view could not repeat).
    pub depth_resyncs: u64,
    /// Depth-writing draws into a guest-size target repeated into the
    /// view's scaled depth with colour writes off.
    pub proxy_depth_draws: u64,
    /// Textures over the depth buffer made from the view's scaled depth.
    pub scaled_depth_aliases: u64,
    /// Frames shown from the reference because the scale is 1 (the view
    /// sleeps): not failures.
    pub reference_frames: u64,
    /// Scale changes made during the run.
    pub rescales: u64,
    /// The scale in effect.
    pub scale: u32,
    /// The factor shadow maps are made at in the view ([`shadow_scale`]; 1:
    /// at the guest's size, only in the reference).
    pub shadow_scale: u32,
    /// Draws into a shadow map repeated into the view's scaled copy.
    pub shadow_draws: u64,
    /// Textures of a shadow map made from the view's scaled copy.
    pub shadow_textures: u64,
    /// Scaled shadow-map copies filled from the reference's (each guest
    /// texel repeated): changes the view could not repeat.
    pub shadow_refills: u64,
}

impl std::fmt::Display for ScaleStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} view frames, {} fallback frames, {} view errors, {} mismatches, {} depth missed, \
             {} colour resyncs, {} depth resyncs, {} depth-only proxy draws, {} scaled depth aliases, \
             {} 1x frames, {} scale changes, shadow maps at {}x: {} draws, {} textures, {} refills, \
             {} reference draws, {} display draws, {} downsamples, {} CPU reads",
            self.view_frames,
            self.fallback_frames,
            self.view_errors,
            self.mismatches,
            self.depth_missed,
            self.color_resyncs,
            self.depth_resyncs,
            self.proxy_depth_draws,
            self.scaled_depth_aliases,
            self.reference_frames,
            self.rescales,
            self.shadow_scale,
            self.shadow_draws,
            self.shadow_textures,
            self.shadow_refills,
            self.reference_draws,
            self.display_draws,
            self.downsamples,
            self.cpu_readbacks,
        )
    }
}

/// Repairs logged once each ([`ScaledBackend::note_once`]).
#[derive(Clone, Copy)]
enum Repair {
    Texture = 1,
    DepthNotScaled = 2,
    Outcome = 4,
    DepthGuestSize = 8,
    DepthWrite = 16,
    DepthUnreadable = 32,
    ShadowDraw = 64,
}

/// A reference backend at the guest's size and a view backend at `scale`
/// (module docs).
pub struct ScaledBackend<R: Backend, V: ScaleView<R>> {
    reference: R,
    view: V,
    single_copy: bool,
    scale: u32,
    /// The frame size: the first colour surface's (the back buffer).
    frame: Option<(u32, u32)>,
    /// Every scaled surface, in the order made: the colour ones serve
    /// clears and depth-only draws of the scaled depth buffer bound with a
    /// guest-size colour target, and all are made again on a scale change.
    scaled: Vec<Scaled<R::Surface, V::Surface>>,
    /// A scale change has mapped a handle the device holds onto another.
    remapped: bool,
    /// Where the wanted scale comes from ([`ScaledBackend::follow_scale`]).
    follow: Option<Box<dyn Fn() -> u32 + Send>>,
    /// The view has recorded work not yet submitted.
    view_pending: bool,
    /// A view call failed since the last present.
    frame_failed: bool,
    stats: ScaleStats,
    /// [`Repair`]s already logged.
    logged: u32,
    /// A copy of `stats` updated at every present, for a reader on another
    /// thread ([`ScaledBackend::shared_stats`]).
    shared: Option<Arc<Mutex<ScaleStats>>>,
    /// The lighting in effect ([`ScaledBackend::set_lighting`]).
    lighting: Lighting,
    /// Where the wanted lighting comes from ([`ScaledBackend::follow_lighting`]).
    follow_lighting: Option<Box<dyn Fn() -> Lighting + Send>>,
    /// Every depth level that may be a shadow map, by its reference
    /// surface (module docs).
    shadows: Vec<ShadowLevel<R::Surface, V::Surface>>,
}

/// A `D24S8` depth surface not of the frame's size: a shadow map, which
/// E3 lighting keeps a scaled copy of in the view.
struct ShadowLevel<R, V> {
    guest: Option<V>,
    reference: R,
    size: (u32, u32),
    format: D3DFormat,
    view: Option<ShadowView<V>>,
}

/// The view's copy of a [`ShadowLevel`].
struct ShadowView<V> {
    depth: V,
    /// A colour surface of the copy's size: the target its draws and
    /// clears go through, colour writes off.
    proxy: V,
    proxy_format: D3DFormat,
    scale: u32,
    /// The reference's level changed in a way the copy did not follow:
    /// its depth, its stencil.
    stale_depth: bool,
    stale_stencil: bool,
}

/// A scaled surface of the view: the handle the device holds (`original`),
/// the one in use now, and what it takes to make it again.
struct Scaled<R, V> {
    guest: Option<V>,
    original: V,
    current: V,
    reference: R,
    size: (u32, u32),
    format: D3DFormat,
    depth: bool,
    /// Made by [`Backend::create_texture_target`].
    target: bool,
}

/// Whether `draw` can change the depth or stencil buffer it is bound with.
fn writes_depth_or_stencil(draw: &ResolvedDraw) -> bool {
    let r = &draw.state.render;
    let z = r.depth_mode != DepthBufferMode::Disabled && r.depth_write;
    let s = &r.stencil;
    let stencil = s.enable
        && s.write_mask != 0
        && [s.fail, s.depth_fail, s.pass]
            .iter()
            .any(|op| *op != StencilOp::Keep);
    z || stencil
}

impl<R: Backend, V: ScaleView<R>> ScaledBackend<R, V> {
    /// Render once at scales above one. Both backends must share a device.
    pub fn new(reference: R, view: V, scale: u32) -> Result<Self, String> {
        Self::with_single_copy(reference, view, scale, true)
    }

    /// Select the exact 1x reference or single-copy policy at construction.
    pub fn with_single_copy(reference: R, mut view: V, scale: u32, single_copy: bool) -> Result<Self, String> {
        if !(1..=MAX_SCALE).contains(&scale) {
            return Err(format!("render scale {scale} is not 1 to {MAX_SCALE}"));
        }
        view.stop_counting();
        let mut reference = reference;
        view.share_results(&mut reference);
        if single_copy {
            view.enable_surface_aliases(&mut reference);
            if scale > 1 { view.move_counter(&mut reference, true); }
        }
        Ok(Self {
            single_copy,
            reference,
            view,
            scale,
            frame: None,
            scaled: Vec::new(),
            remapped: false,
            follow: None,
            view_pending: false,
            frame_failed: false,
            stats: ScaleStats {
                scale,
                shadow_scale: 1,
                ..ScaleStats::default()
            },
            logged: 0,
            shared: None,
            lighting: Lighting::Retail,
            follow_lighting: None,
            shadows: Vec::new(),
        })
    }

    fn finish_frame(&mut self) {
        // The frame boundary: a wanted scale and lighting are made here,
        // before the next frame's first call.
        let scale = self.follow.as_ref().map_or(self.scale, |f| f());
        let lighting = self.follow_lighting.as_ref().map_or(self.lighting, |f| f());
        self.retarget(scale, lighting);
        if let Some(s) = &self.shared
            && let Ok(mut s) = s.lock()
        {
            *s = self.stats;
        }
    }

    /// The scale in effect.
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// Reads the wanted scale from `wanted` at every present and changes to
    /// it there (module docs). Values outside 1 to [`MAX_SCALE`] are
    /// clamped.
    pub fn follow_scale(&mut self, wanted: impl Fn() -> u32 + Send + 'static) {
        self.follow = Some(Box::new(wanted));
    }

    /// The lighting in effect.
    pub fn lighting(&self) -> Lighting {
        self.lighting
    }

    /// Changes the lighting now (module docs). Call it between frames, as
    /// [`ScaledBackend::follow_lighting`] does at each present; at the
    /// start of a run, before the first call, is one such point.
    pub fn set_lighting(&mut self, lighting: Lighting) {
        self.retarget(self.scale, lighting);
    }

    /// Reads the wanted lighting from `wanted` at every present and changes
    /// to it there, as [`ScaledBackend::follow_scale`] does the scale.
    pub fn follow_lighting(&mut self, wanted: impl Fn() -> Lighting + Send + 'static) {
        self.follow_lighting = Some(Box::new(wanted));
    }

    /// The view's scaled copy of each shadow map that has one, with the
    /// map's guest size.
    pub fn shadow_copies(&self) -> Vec<((u32, u32), V::Surface)> {
        self.shadows
            .iter()
            .filter_map(|s| s.view.as_ref().map(|v| (s.size, v.depth)))
            .collect()
    }

    /// The factor the view's shadow maps are at now ([`shadow_scale`]).
    fn shadow_factor(&self) -> u32 {
        shadow_scale(self.scale, self.lighting)
    }

    /// Whether the view draws: above render scale 1, or with scaled
    /// shadow maps (E3 lighting, whose view runs at 1x too).
    fn awake(&self) -> bool {
        self.scale > 1 || self.shadow_factor() > 1
    }

    /// Takes the wanted scale and lighting at a frame boundary.
    fn retarget(&mut self, scale: u32, lighting: Lighting) {
        let scale = scale.clamp(1, MAX_SCALE);
        let was_awake = self.awake();
        // A scaled shadow can be the only up-to-date image in single-copy
        // mode. Preserve it before retiring or handing it to the exact path.
        if self.single() && (scale == 1 || self.shadow_factor() != shadow_scale(scale, lighting)) {
            for shadow in &self.shadows {
                if let Some(v) = &shadow.view && !v.stale_depth && !v.stale_stencil
                    && let Err(e) = self.view.resize(v.depth, shadow.guest.expect("imported shadow")) {
                    eprintln!("d3d: keeping current scale and lighting: {e}");
                    return;
                }
            }
        }
        if lighting != self.lighting {
            eprintln!("d3d: lighting {:?} -> {lighting:?}", self.lighting);
            self.lighting = lighting;
            self.view.set_hard_shadows(lighting == Lighting::E3);
        }
        let old = self.scale;
        if scale != old {
            self.rescale(scale);
        }
        // Woken by the lighting alone: the view's surfaces hold what they
        // held when it went to sleep.
        if self.scale == old && self.awake() && !was_awake {
            if let Err(e) = self.view.submit_recorded() {
                self.view_failed("submit", e);
            }
            self.view_pending = false;
            self.fill_scaled();
        }
        self.stats.shadow_scale = self.shadow_factor();
        // Copies at another factor go; the next use makes them again.
        let keep = self.stats.shadow_scale;
        for i in 0..self.shadows.len() {
            if self.shadows[i]
                .view
                .as_ref()
                .is_some_and(|v| v.scale != keep)
            {
                self.drop_shadow_view(i);
            }
        }
    }

    /// Fills every scaled surface from the reference's, each guest pixel
    /// repeated.
    fn fill_scaled(&mut self) {
        for i in 0..self.scaled.len() {
            let (r, v) = (self.scaled[i].reference, self.scaled[i].current);
            match self.view.fill_from(&mut self.reference, r, v) {
                Ok(true) => {}
                Ok(false) => self.stats.depth_missed += 1,
                Err(e) => self.view_failed("fill", e),
            }
        }
    }

    /// The shadow level whose reference surface is `r`.
    fn shadow_at(&self, r: &R::Surface) -> Option<usize> {
        self.shadows.iter().position(|s| s.reference == *r)
    }

    fn drop_shadow_view(&mut self, i: usize) {
        if let Some(v) = self.shadows[i].view.take() {
            for s in [v.depth, v.proxy] {
                if let Err(e) = self.view.destroy_surface(s) {
                    self.view_failed("destroy", e);
                }
            }
        }
    }

    /// Shadow level `i`'s view copy at the current factor, made (stale)
    /// if it has none. `None` without scaled shadow maps, or when it
    /// cannot be made.
    fn shadow_view(&mut self, i: usize) -> Option<&mut ShadowView<V::Surface>> {
        let f = self.shadow_factor();
        if f == 1 {
            return None;
        }
        if self.shadows[i].view.as_ref().is_none_or(|v| v.scale != f) {
            self.drop_shadow_view(i);
            let (format, (w, h)) = (self.shadows[i].format, self.shadows[i].size);
            let proxy_format = D3DFormat::LinA8R8G8B8;
            let depth = match self.view.create_scaled_surface(format, w, h, true, f) {
                Ok(d) => d,
                Err(e) => {
                    self.view_failed("shadow map", e);
                    return None;
                }
            };
            let proxy = match self
                .view
                .create_scaled_surface(proxy_format, w, h, false, f)
            {
                Ok(p) => p,
                Err(e) => {
                    let _ = self.view.destroy_surface(depth);
                    self.view_failed("shadow map", e);
                    return None;
                }
            };
            self.shadows[i].view = Some(ShadowView {
                depth,
                proxy,
                proxy_format,
                scale: f,
                stale_depth: true,
                stale_stencil: true,
            });
        }
        self.shadows[i].view.as_mut()
    }

    /// The reference changed shadow level `r` in a way the view did not
    /// repeat: its copy is filled again at its next use.
    fn shadow_stale(&mut self, r: &R::Surface) {
        if let Some(i) = self.shadow_at(r)
            && let Some(v) = self.shadows[i].view.as_mut()
        {
            v.stale_depth = true;
            v.stale_stencil = true;
        }
    }

    /// Fills shadow level `i`'s copy from the reference's level as the
    /// reference's work so far left it. `false`: it could not.
    fn refill_shadow(&mut self, i: usize) -> bool {
        let Some(v) = self.shadows[i].view.as_ref().map(|v| v.depth) else {
            return false;
        };
        self.view_pending = false;
        match self
            .view
            .fill_from(&mut self.reference, self.shadows[i].reference, v)
        {
            Ok(true) => {
                self.stats.shadow_refills += 1;
                if let Some(v) = self.shadows[i].view.as_mut() {
                    v.stale_depth = false;
                    v.stale_stencil = false;
                }
                true
            }
            Ok(false) => false,
            Err(e) => {
                self.view_failed("shadow map fill", e);
                false
            }
        }
    }

    /// A drawn reference `draw` into shadow level `i` (depth) and `color`:
    /// repeated into the view's copy, colour writes off, through the
    /// copy's own colour surface. A draw the copy cannot follow leaves it
    /// stale; one into a stale copy fills it from the reference instead,
    /// which holds this draw already.
    fn shadow_draw(
        &mut self,
        i: usize,
        color_scaled: bool,
        draw: &ResolvedDraw,
        textures: [Option<&V::Texture>; 4],
        sampled: bool,
    ) {
        if !writes_depth_or_stencil(draw) {
            return;
        }
        let size = self.shadows[i].size;
        let Some(v) = self.shadow_view(i) else {
            return;
        };
        if color_scaled || !sampled {
            v.stale_depth = true;
            v.stale_stencil = true;
            return;
        }
        let stencil = draw.state.render.stencil.enable;
        if v.stale_depth || (stencil && v.stale_stencil) {
            self.refill_shadow(i);
            return;
        }
        let (depth, proxy, format) = (v.depth, v.proxy, v.proxy_format);
        let mut d = draw.clone();
        d.state.render.color_write = ColorWriteMask {
            red: false,
            green: false,
            blue: false,
            alpha: false,
        };
        // The copy's colour surface is the level's size: window
        // coordinates map onto it as they do onto the level.
        (d.target.width, d.target.height) = size;
        d.target.format = format;
        let r = self.view.draw(proxy, Some(depth), &d, textures);
        self.view_pending = true;
        match r {
            Ok(DrawOutcome::Drawn) => { self.stats.shadow_draws += 1; self.stats.display_draws += 1; }
            Ok(o) => {
                self.note_once(Repair::ShadowDraw, || {
                    format!("the view's shadow-map draw was {o:?}")
                });
                self.refill_shadow(i);
            }
            Err(e) => {
                self.view_failed("shadow-map draw", e);
                let r = self.shadows[i].reference;
                self.shadow_stale(&r);
            }
        }
    }

    /// A reference clear of shadow level `i`, whole (`rects` `None`) or of
    /// rectangles, repeated into the view's copy through its colour
    /// surface. A rectangle clear of a stale aspect fills it from the
    /// reference instead; a whole clear makes what it clears fresh.
    fn shadow_clear(&mut self, i: usize, clear: Clear, rects: Option<&RectClear>) {
        if clear.depth.is_none() && clear.stencil.is_none() {
            return;
        }
        let Some(v) = self.shadow_view(i) else {
            return;
        };
        let partial = rects.is_some();
        let stale = (clear.depth.is_some() && v.stale_depth)
            || (clear.stencil.is_some() && v.stale_stencil);
        if partial && stale {
            self.refill_shadow(i);
            return;
        }
        let (depth, proxy) = (v.depth, v.proxy);
        if !partial {
            v.stale_depth &= clear.depth.is_none();
            v.stale_stencil &= clear.stencil.is_none();
        }
        let r = match rects {
            None => self.view.clear(
                proxy,
                Some(depth),
                Clear {
                    color: None,
                    ..clear
                },
            ),
            Some(rc) => self
                .view
                .clear_rects(
                    proxy,
                    Some(depth),
                    &RectClear {
                        color: None,
                        ..rc.clone()
                    },
                )
                .map(|_| ()),
        };
        self.view_pending = true;
        if let Err(e) = r {
            self.view_failed("shadow-map clear", e);
            let r = self.shadows[i].reference;
            self.shadow_stale(&r);
        }
    }

    /// What the view samples for shadow level `r`'s texture: a texture of
    /// its scaled copy, when it has a fresh one.
    fn shadow_texture(&mut self, r: &R::Surface) -> Option<V::Texture> {
        let i = self.shadow_at(r)?;
        let f = self.shadow_factor();
        let v = self.shadows[i].view.as_ref()?;
        if v.scale != f || f == 1 || v.stale_depth {
            return None;
        }
        let depth = v.depth;
        match self.view.depth_level_texture(depth) {
            Ok(Some(t)) => {
                self.stats.shadow_textures += 1;
                Some(t)
            }
            Ok(None) => None,
            Err(e) => {
                self.view_failed("shadow-map texture", e);
                None
            }
        }
    }

    /// The view surface standing for `s` now: `None` while the view sleeps
    /// (scale 1) or for a surface only the reference keeps.
    pub fn view_surface(&self, s: &ScaledSurface<R::Surface, V::Surface>) -> Option<V::Surface> {
        self.live(s.view)
    }

    fn live(&self, v: Option<V::Surface>) -> Option<V::Surface> {
        if !self.awake() {
            return None;
        }
        let v = v?;
        if !self.remapped {
            return Some(v);
        }
        Some(
            self.scaled
                .iter()
                .find(|s| s.original == v)
                .map_or(v, |s| s.current),
        )
    }

    /// Makes every scaled surface again at `scale` and fills it from the
    /// reference's. Nothing changes if one cannot be made.
    fn rescale(&mut self, scale: u32) {
        let scale = scale.clamp(1, MAX_SCALE);
        if scale == self.scale {
            return;
        }
        let old = self.scale;
        if let Err(e) = self.view.submit_recorded() {
            self.view_failed("submit", e);
        }
        self.view_pending = false;
        let mut made = Vec::with_capacity(self.scaled.len());
        for s in &self.scaled {
            let (w, h) = s.size;
            let r = if s.target {
                self.view
                    .create_scaled_texture_target(s.format, w, h, scale)
            } else {
                self.view
                    .create_scaled_surface(s.format, w, h, s.depth, scale)
            };
            match r {
                Ok(v) => made.push(v),
                Err(e) => {
                    for v in made {
                        let _ = self.view.destroy_surface(v);
                    }
                    eprintln!("d3d: render scale stays {old}: a {w}x{h} surface at {scale}x: {e}");
                    return;
                }
            }
        }
        // Preserve the authoritative image before changing the public handles.
        // The source may contain GPU work from earlier frames.
        if self.single_copy {
            for (s, &new) in self.scaled.iter().zip(&made) {
                let guest = s.guest.expect("imported 1x surface");
                let from = if old > 1 { s.current } else { guest };
                let to = if scale > 1 { new } else { guest };
                if let Err(e) = self.view.resize(from, to) {
                    for v in made.iter().copied() { let _ = self.view.destroy_surface(v); }
                    eprintln!("d3d: render scale stays {old}: {e}");
                    return;
                }
                if scale == 1 && self.lighting == Lighting::E3 {
                    if let Err(e) = self.view.resize(guest, new) {
                        for v in made.iter().copied() { let _ = self.view.destroy_surface(v); }
                        eprintln!("d3d: render scale stays {old}: {e}");
                        return;
                    }
                }
            }
            if (old == 1) != (scale == 1) {
                self.view.move_counter(&mut self.reference, scale > 1);
            }
        }
        self.scale = scale;
        self.stats.scale = scale;
        self.stats.rescales += 1;
        self.remapped = true;
        for (i, v) in made.into_iter().enumerate() {
            let old = std::mem::replace(&mut self.scaled[i].current, v);
            if let Err(e) = self.view.destroy_surface(old) {
                self.view_failed("destroy", e);
            }
        }
        // Waking or changing: the new surfaces start as the reference's
        // image, each guest pixel repeated, until they are drawn again.
        if self.awake() && !self.single_copy {
            self.fill_scaled();
        }
        eprintln!("d3d: render scale {old}x -> {scale}x");
    }

    /// What the view did differently so far.
    pub fn stats(&self) -> ScaleStats {
        self.stats
    }

    /// A handle to [`ScaledBackend::stats`] as of the last present, for a
    /// run report read on another thread (the backend may live on a render
    /// thread).
    pub fn shared_stats(&mut self) -> Arc<Mutex<ScaleStats>> {
        Arc::clone(
            self.shared
                .get_or_insert_with(|| Arc::new(Mutex::new(self.stats))),
        )
    }

    /// The reference.
    pub fn reference(&self) -> &R {
        &self.reference
    }

    /// The reference, mutably.
    pub fn reference_mut(&mut self) -> &mut R {
        &mut self.reference
    }

    /// The view.
    pub fn view(&self) -> &V {
        &self.view
    }

    /// The view, mutably.
    pub fn view_mut(&mut self) -> &mut V {
        &mut self.view
    }

    fn view_failed(&mut self, what: &str, e: String) {
        self.stats.view_errors += 1;
        self.frame_failed = true;
        if self.stats.view_errors <= 10 {
            eprintln!(
                "d3d: render scale {}: view {what}: {e} (the frame is shown at 1x)",
                self.scale
            );
        }
    }

    /// Logs the first repair of kind `r`.
    fn note_once(&mut self, r: Repair, what: impl FnOnce() -> String) {
        if self.logged & r as u32 == 0 {
            self.logged |= r as u32;
            eprintln!(
                "d3d: render scale {}: {} (repaired from the 1x reference; logged once)",
                self.scale,
                what()
            );
        }
    }

    /// Compares the view's outcome with the reference's. `true`: the view
    /// did something else, and its target needs the reference's pixels.
    fn compare(
        &mut self,
        what: &str,
        reference: &DrawOutcome,
        view: Result<DrawOutcome, String>,
    ) -> bool {
        match view {
            Ok(v) if v == *reference => false,
            Ok(v) => {
                self.stats.mismatches += 1;
                self.note_once(Repair::Outcome, || {
                    format!("view {what} {v:?}, reference {reference:?}")
                });
                true
            }
            Err(e) => {
                self.view_failed(what, e);
                false
            }
        }
    }

    /// Before the reference changes guest-size surface `s`: the view's
    /// recorded draws that sample it must reach the GPU first.
    fn before_shared_write(&mut self, s: &ScaledSurface<R::Surface, V::Surface>) {
        if self.live(s.view).is_none() && self.view_pending {
            self.view_pending = false;
            if let Err(e) = self.view.submit_recorded() {
                self.view_failed("submit", e);
            }
        }
    }

    /// A scaled colour surface of guest size `size`, and its format, to
    /// clear or draw the scaled depth through.
    fn proxy_color(&self, size: (u32, u32)) -> Option<(V::Surface, D3DFormat)> {
        self.scaled
            .iter()
            .find(|s| !s.depth && s.size == size)
            .map(|s| (s.current, s.format))
    }

    fn share(&mut self, t: &R::Texture) -> Option<V::Texture> {
        self.view.share_texture(&self.reference, t)
    }

    /// Refreshes the view's copy of scaled surface `s` from the
    /// reference's. `false`: it could not (counted).
    fn refill(&mut self, s: &ScaledSurface<R::Surface, V::Surface>) -> bool {
        let Some(v) = self.live(s.view) else {
            return true;
        };
        // The view's recorded draws into `v` come first; the fill replaces
        // them.
        self.view_pending = false;
        match self.view.fill_from(&mut self.reference, s.reference, v) {
            Ok(true) => true,
            Ok(false) => {
                self.stats.depth_missed += 1;
                self.note_once(Repair::DepthUnreadable, || {
                    "the reference's depth cannot be read back, so the view's misses a change"
                        .into()
                });
                false
            }
            Err(e) => {
                self.view_failed("fill", e);
                false
            }
        }
    }

    /// The view's `color` (and `depth`, when `draw` may write it) are
    /// replaced by the reference's after a draw the view could not repeat.
    fn resync(
        &mut self,
        color: &Surf<R, V>,
        depth: Option<&Surf<R, V>>,
        draw: Option<&ResolvedDraw>,
    ) {
        if self.live(color.view).is_some() && self.refill(color) {
            self.stats.color_resyncs += 1;
        }
        if let Some(d) = depth
            && self.live(d.view).is_some()
            && draw.is_none_or(writes_depth_or_stencil)
            && self.refill(d)
        {
            self.stats.depth_resyncs += 1;
        }
    }

    /// A draw into guest-size target `color` with the scaled depth `depth`
    /// bound wrote depth or stencil in the reference: the view repeats it
    /// into its depth through a scaled colour surface of the depth's size,
    /// colour writes off. Without one (or a texture it cannot sample) the
    /// view's depth is replaced by the reference's.
    fn depth_only_draw(
        &mut self,
        depth: &Surf<R, V>,
        draw: &ResolvedDraw,
        textures: [Option<&V::Texture>; 4],
        sampled: bool,
    ) {
        let Some(vd) = self.live(depth.view) else {
            return;
        };
        let proxy = self.proxy_color(depth.size).filter(|_| sampled);
        if let Some((proxy, format)) = proxy {
            let mut d = draw.clone();
            d.state.render.color_write = ColorWriteMask {
                red: false,
                green: false,
                blue: false,
                alpha: false,
            };
            // The proxy is the depth's size: window coordinates map onto
            // it as they do onto the depth buffer.
            (d.target.width, d.target.height) = depth.size;
            d.target.format = format;
            let r = self.view.draw(proxy, Some(vd), &d, textures);
            self.view_pending = true;
            match r {
                Ok(DrawOutcome::Drawn) => {
                    self.stats.proxy_depth_draws += 1;
                    self.stats.display_draws += 1;
                    return;
                }
                Ok(o) => self.note_once(Repair::DepthWrite, || {
                    format!("the view's depth-only draw was {o:?}")
                }),
                Err(e) => return self.view_failed("depth-only draw", e),
            }
        } else {
            self.note_once(Repair::DepthGuestSize, || {
                format!(
                    "a draw into a {}x{} target writes the scaled depth buffer and the view cannot repeat it",
                    draw.target.width, draw.target.height
                )
            });
        }
        if self.refill(depth) {
            self.stats.depth_resyncs += 1;
        }
    }
}

/// One external pass drawn into both copies of the targets.
#[derive(Clone)]
struct SharedPass(std::sync::Arc<std::sync::Mutex<Box<dyn crate::backend::ExternalPass>>>, crate::TargetCopy);

impl crate::backend::ExternalPass for SharedPass {
    fn confirm_issue(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).confirm_issue()
    }

    fn encode(&mut self, mut target: crate::backend::ExternalTarget<'_>) {
        target.copy = self.1;
        self.0.lock().unwrap_or_else(|e| e.into_inner()).encode(target);
    }
}

const DISPLAY_TICKET: u64 = 1 << 63;

type Surf<R, V> = ScaledSurface<<R as Backend>::Surface, <V as Backend>::Surface>;
type Tex<R, V> = ScaledTexture<<R as Backend>::Texture, <V as Backend>::Texture>;

impl<R: Backend, V: ScaleView<R>> Backend for ScaledBackend<R, V> {
    type Surface = Surf<R, V>;
    type Texture = Tex<R, V>;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String> {
        let reference = self
            .reference
            .create_surface(format, width, height, depth)?;
        if !depth && self.frame.is_none() {
            self.frame = Some((width, height));
        }
        let view = if self.frame == Some((width, height)) {
            match self
                .view
                .create_scaled_surface(format, width, height, depth, self.scale)
            {
                Ok(v) => Some(v),
                Err(e) => {
                    self.view_failed("surface", e);
                    None
                }
            }
        } else {
            None
        };
        let guest = if self.single_copy { Some(self.view.import_guest(&self.reference, reference)?) } else { None };
        if let Some(v) = view {
            self.scaled.push(Scaled {
                guest,
                original: v,
                current: v,
                reference,
                size: (width, height),
                format,
                depth,
                target: false,
            });
        } else if depth
            && matches!(format, D3DFormat::D24S8 | D3DFormat::LinD24S8)
            && self.frame.is_some()
        {
            // A depth level of another size: a shadow map (module docs).
            self.shadows.push(ShadowLevel {
                guest,
                reference,
                size: (width, height),
                format,
                view: None,
            });
        }
        Ok(ScaledSurface {
            guest,
            reference,
            view,
            size: (width, height),
        })
    }

    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        self.before_shared_write(&surface);
        if let Some(guest) = surface.guest {
            self.view.destroy_surface(guest)?;
        }
        self.reference.destroy_surface(surface.reference)?;
        if let Some(i) = self.shadow_at(&surface.reference) {
            self.drop_shadow_view(i);
            self.shadows.remove(i);
        }
        if let Some(v) = surface.view {
            let at = self.scaled.iter().position(|s| s.original == v);
            let v = at.map_or(v, |i| self.scaled.remove(i).current);
            if let Err(e) = self.view.destroy_surface(v) {
                self.view_failed("destroy", e);
            }
        }
        Ok(())
    }

    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        let reference = self.reference.upload_texture(texture)?;
        let view = self.share(&reference);
        Ok(ScaledTexture { reference, view })
    }

    fn bc_supported(&self) -> bool {
        self.reference.bc_supported()
    }

    fn clear(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: Clear,
    ) -> Result<(), String> {
        if self.single() {
            return self.clear_single(color, depth, clear, None).map(|_| ());
        }
        self.before_shared_write(&color);
        let (rc, rd) = (color.reference, depth.map(|d| d.reference));
        self.reference.clear(rc, rd, clear)?;
        if let Some(i) = rd.and_then(|d| self.shadow_at(&d)) {
            self.shadow_clear(i, clear, None);
        }
        let vd = self.live(depth.and_then(|d| d.view));
        let r = match (self.live(color.view), depth) {
            (Some(vc), None) => self.view.clear(vc, None, clear),
            (Some(vc), Some(_)) if vd.is_some() => self.view.clear(vc, vd, clear),
            // The depth surface is not scaled: the view's colour only.
            (Some(vc), Some(_)) => self.view.clear(
                vc,
                None,
                Clear {
                    depth: None,
                    stencil: None,
                    ..clear
                },
            ),
            // A guest-size colour target with the scaled depth buffer:
            // clear the view's depth through a scaled colour of its size.
            (None, Some(d))
                if vd.is_some() && (clear.depth.is_some() || clear.stencil.is_some()) =>
            {
                match self.proxy_color(d.size) {
                    Some((proxy, _)) => self.view.clear(
                        proxy,
                        vd,
                        Clear {
                            color: None,
                            ..clear
                        },
                    ),
                    None => {
                        if self.refill(&d) {
                            self.stats.depth_resyncs += 1;
                        }
                        return Ok(());
                    }
                }
            }
            _ => return Ok(()),
        };
        self.view_pending = true;
        if let Err(e) = r {
            self.view_failed("clear", e);
        }
        Ok(())
    }

    fn draw(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: &ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String> {
        if self.single() {
            return self.draw_single(color, depth, draw, textures);
        }
        self.before_shared_write(&color);
        let out = self.reference.draw(
            color.reference,
            depth.map(|d| d.reference),
            draw,
            textures.map(|t| t.map(|t| &t.reference)),
        )?;
        let drawn = out == DrawOutcome::Drawn;
        self.stats.reference_draws += u64::from(drawn);
        let vd = self.live(depth.and_then(|d| d.view));
        let vt = textures.map(|t| t.and_then(|t| t.view.as_ref()));
        let sampled = textures
            .iter()
            .zip(&vt)
            .all(|(t, v)| t.is_none() || v.is_some());
        if drawn && let Some(i) = depth.and_then(|d| self.shadow_at(&d.reference)) {
            let color_scaled = self.live(color.view).is_some();
            self.shadow_draw(i, color_scaled, draw, vt, sampled);
        }
        match (self.live(color.view), depth) {
            (Some(vc), d) if d.is_none() || vd.is_some() => {
                if !sampled {
                    // A texture the view cannot sample: it would read a
                    // stand-in. The reference's pixels instead.
                    if drawn {
                        self.note_once(Repair::Texture, || {
                            "a draw samples a texture the view cannot".into()
                        });
                        self.resync(&color, depth.as_ref(), Some(draw));
                    }
                    return Ok(out);
                }
                let r = self.view.draw(vc, vd, draw, vt);
                self.stats.display_draws += u64::from(matches!(r, Ok(DrawOutcome::Drawn)));
                self.view_pending = true;
                if self.compare("draw", &out, r) {
                    self.resync(&color, depth.as_ref(), Some(draw));
                }
            }
            // Colour scaled, depth not: attachments of different sizes.
            (Some(_), Some(d)) => {
                if drawn {
                    self.note_once(Repair::DepthNotScaled, || {
                        format!(
                            "a draw into the scaled {}x{} target with a {}x{} depth surface",
                            color.size.0, color.size.1, d.size.0, d.size.1
                        )
                    });
                    self.resync(&color, None, None);
                }
            }
            (None, Some(d)) if vd.is_some() && drawn && writes_depth_or_stencil(draw) => {
                self.depth_only_draw(&d, draw, vt, sampled);
            }
            _ => {}
        }
        Ok(out)
    }

    fn frame_stats(&mut self) {
        self.reference.frame_stats();
        self.view.frame_stats();
    }

    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String> {
        self.present_with_gamma(surface, flags, None)
    }

    fn present_with_gamma(
        &mut self,
        surface: Self::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        if self.single() {
            self.view.present_with_gamma(self.image(surface), flags, gamma)?;
            self.stats.view_frames += 1;
            self.frame_failed = false;
            self.finish_frame();
            return Ok(());
        }
        // The reference ends its frame too (its swap ring, shader upkeep);
        // it shows nothing, unless it has a present hook.
        self.reference
            .present_with_gamma(surface.reference, flags, gamma)?;
        self.view_pending = false;
        let shown = match self.live(surface.view) {
            Some(v) if !self.frame_failed => match self.view.present_with_gamma(v, flags, gamma) {
                Ok(()) => true,
                Err(e) => {
                    self.view_failed("present", e);
                    false
                }
            },
            _ => false,
        };
        if shown {
            self.stats.view_frames += 1;
        } else {
            if !self.awake() {
                self.stats.reference_frames += 1;
            } else {
                self.stats.fallback_frames += 1;
            }
            if let Err(e) =
                self.view
                    .present_reference(&self.reference, surface.reference, flags, gamma)
            {
                self.stats.view_errors += 1;
                if self.stats.view_errors <= 10 {
                    eprintln!(
                        "d3d: render scale {}: showing the 1x frame: {e}",
                        self.scale
                    );
                }
            }
        }
        self.frame_failed = false;
        self.finish_frame();
        Ok(())
    }

    fn warp_surface(
        &mut self,
        src: Self::Surface,
        dst: Self::Surface,
        h: [[f32; 3]; 3],
    ) -> Result<bool, String> {
        if self.single() {
            return self.view.warp_surface(self.image(src), self.image(dst), h);
        }
        // Host-only: the view shows it when it has both surfaces; the
        // reference's copy is what shows otherwise.
        let warped = self.reference.warp_surface(src.reference, dst.reference, h)?;
        match (self.live(src.view), self.live(dst.view)) {
            (Some(s), Some(d)) if !self.frame_failed => self.view.warp_surface(s, d, h),
            _ => Ok(warped),
        }
    }

    fn surface_extent(&self, surface: Self::Surface) -> Option<(u32, u32)> {
        if self.single() { return self.view.surface_extent(self.image(surface)); }
        // What a present shows: the view's copy while it draws, else the
        // reference's.
        match self.live(surface.view) {
            Some(v) if !self.frame_failed => self.view.surface_extent(v),
            _ => self.reference.surface_extent(surface.reference),
        }
    }

    fn gpu_fence(&mut self) -> Option<Box<dyn Fn() -> bool + Send>> {
        if self.single() { return self.view.gpu_fence(); }
        // Both are on one queue (`ScaleView`): the later fence covers the
        // other's work too once both have submitted.
        let reference = self.reference.gpu_fence();
        if !self.awake() {
            return reference;
        }
        let view = self.view.gpu_fence();
        match (reference, view) {
            (Some(r), Some(v)) => Some(Box::new(move || r() && v())),
            (r, v) => r.or(v),
        }
    }

    fn present_replay(
        &mut self,
        surface: Self::Surface,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        if self.single() { return self.view.present_replay(self.image(surface), gamma); }
        // A host-only frame (crate::interp): shown by the view when it has
        // the surface; not a frame boundary (no scale or lighting change).
        match self.live(surface.view) {
            Some(v) if !self.frame_failed => self.view.present_replay(v, gamma),
            _ => self
                .view
                .present_reference(&self.reference, surface.reference, 0, gamma),
        }
    }

    fn clear_rects(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        if self.single() {
            let aspects = Clear { color: clear.color.map(|(rgba, _)| rgba), depth: clear.depth, stencil: clear.stencil };
            return self.clear_single(color, depth, aspects, Some(clear));
        }
        self.before_shared_write(&color);
        let out = self
            .reference
            .clear_rects(color.reference, depth.map(|d| d.reference), clear)?;
        if out != DrawOutcome::Drawn {
            return Ok(out);
        }
        if let Some(i) = depth.and_then(|d| self.shadow_at(&d.reference)) {
            let whole = Clear {
                color: None,
                depth: clear.depth,
                stencil: clear.stencil,
            };
            self.shadow_clear(i, whole, Some(clear));
        }
        let vd = self.live(depth.and_then(|d| d.view));
        let touches_depth = clear.depth.is_some() || clear.stencil.is_some();
        let r = match (self.live(color.view), depth) {
            (Some(vc), d) if d.is_none() || vd.is_some() => self.view.clear_rects(vc, vd, clear),
            (Some(vc), Some(_)) => {
                let colour_only = RectClear {
                    depth: None,
                    stencil: None,
                    ..clear.clone()
                };
                self.view.clear_rects(vc, None, &colour_only)
            }
            (None, Some(d)) if vd.is_some() && touches_depth => match self.proxy_color(d.size) {
                Some((proxy, _)) => {
                    let depth_only = RectClear {
                        color: None,
                        ..clear.clone()
                    };
                    self.view.clear_rects(proxy, vd, &depth_only)
                }
                None => {
                    if self.refill(&d) {
                        self.stats.depth_resyncs += 1;
                    }
                    return Ok(out);
                }
            },
            _ => return Ok(out),
        };
        self.view_pending = true;
        if self.compare("clear", &out, r) {
            let depth = depth.filter(|_| touches_depth);
            self.resync(&color, depth.as_ref(), None);
        }
        Ok(out)
    }

    fn read_surface(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        if self.single() {
            let guest = self.guest_image(surface)?;
            let bytes = self.view.read_surface(guest, format, width, height, layout)?;
            self.stats.cpu_readbacks += 1;
            return Ok(bytes);
        }
        let bytes = self.reference.read_surface(surface.reference, format, width, height, layout)?;
        self.stats.cpu_readbacks += 1;
        Ok(bytes)
    }

    fn read_depth(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        if self.single() {
            let guest = self.guest_image(surface)?;
            let bytes = self.view.read_depth(guest, format, width, height, layout)?;
            self.stats.cpu_readbacks += u64::from(bytes.is_some());
            return Ok(bytes);
        }
        let bytes = self.reference.read_depth(surface.reference, format, width, height, layout)?;
        self.stats.cpu_readbacks += u64::from(bytes.is_some());
        Ok(bytes)
    }

    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        if self.single() {
            let (s, d) = (self.image(src), self.image(dst));
            if (s == self.guest(src)) == (d == self.guest(dst)) {
                return self.view.copy_surface(s, d);
            }
            if src.size.0 > dst.size.0 || src.size.1 > dst.size.1 {
                return Err("source rectangle exceeds destination surface".into());
            }
            let scale = if d == self.guest(dst) { 1 } else { self.scale };
            self.view.copy_scaled(s, d, (src.size.0 * scale, src.size.1 * scale))?;
            return Ok(true);
        }
        self.before_shared_write(&dst);
        let copied = self.reference.copy_surface(src.reference, dst.reference)?;
        if copied {
            self.shadow_stale(&dst.reference);
        }
        if copied && let Some(vd) = self.live(dst.view) {
            match self.live(src.view) {
                Some(vs) => match self.view.copy_surface(vs, vd) {
                    Ok(true) => {}
                    Ok(false) => {
                        self.refill(&dst);
                    }
                    Err(e) => self.view_failed("copy", e),
                },
                None => {
                    self.refill(&dst);
                }
            }
        }
        Ok(copied)
    }

    fn write_surface(
        &mut self,
        surface: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        if self.single() {
            let guest = self.guest(surface);
            let written = self.view.write_surface(guest, desc, bytes)?;
            let image = self.image(surface);
            if written && image != guest { self.view.resize(guest, image)?; }
            return Ok(written);
        }
        self.before_shared_write(&surface);
        let written = self
            .reference
            .write_surface(surface.reference, desc, bytes)?;
        if written {
            self.refill(&surface);
            self.shadow_stale(&surface.reference);
        }
        Ok(written)
    }

    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        if self.single() {
            let texture = self.view.snapshot_surface(self.image(surface), opaque)?;
            return self.sampled(texture);
        }
        let Some(reference) = self.reference.snapshot_surface(surface.reference, opaque)? else {
            return Ok(None);
        };
        let view = match self.live(surface.view) {
            // The copy of the scaled image.
            Some(v) => match self.view.snapshot_surface(v, opaque) {
                Ok(t) => t,
                Err(e) => {
                    self.view_failed("snapshot", e);
                    None
                }
            },
            None => self.share(&reference),
        };
        Ok(Some(ScaledTexture { reference, view }))
    }

    fn clear_pixel_count(&mut self) -> Result<(), String> {
        if self.single() { return self.view.clear_pixel_count(); }
        self.reference.clear_pixel_count()
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        // Preserve ticket ownership across run-time resolution changes.
        if self.single() { return self.view.report_pixel_count().map(|t| t.map(|n| n | DISPLAY_TICKET)); }
        self.reference.report_pixel_count()
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        if ticket & DISPLAY_TICKET != 0 {
            let result = self.view.read_pixel_count(ticket & !DISPLAY_TICKET)?;
            self.stats.cpu_readbacks += 1;
            return Ok(result);
        }
        let result = self.reference.read_pixel_count(ticket)?;
        self.stats.cpu_readbacks += 1;
        Ok(result)
    }

    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        if ticket & DISPLAY_TICKET != 0 {
            let result = self.view.try_pixel_count(ticket & !DISPLAY_TICKET)?;
            self.stats.cpu_readbacks += u64::from(result.is_some());
            return Ok(result);
        }
        let result = self.reference.try_pixel_count(ticket)?;
        self.stats.cpu_readbacks += u64::from(result.is_some());
        Ok(result)
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        if ticket & DISPLAY_TICKET != 0 { return self.view.drop_pixel_count(ticket & !DISPLAY_TICKET); }
        self.reference.drop_pixel_count(ticket)
    }

    fn pixel_count_unsubmitted(&self, ticket: u64) -> bool {
        if ticket & DISPLAY_TICKET != 0 { return self.view.pixel_count_unsubmitted(ticket & !DISPLAY_TICKET); }
        self.reference.pixel_count_unsubmitted(ticket)
    }

    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<Self::Surface, String> {
        let reference = self
            .reference
            .create_texture_target(format, width, height)?;
        // A level the size of the back buffer (an intermediate drawn with
        // the depth buffer, like Halo's texaccum) scales with it; smaller
        // ones (bloom, reflections, shadows) stay at 1x in the reference.
        let view = if self.frame == Some((width, height)) {
            match self
                .view
                .create_scaled_texture_target(format, width, height, self.scale)
            {
                Ok(v) => Some(v),
                Err(e) => {
                    self.view_failed("render target", e);
                    None
                }
            }
        } else {
            None
        };
        let guest = if self.single_copy { Some(self.view.import_guest(&self.reference, reference)?) } else { None };
        if let Some(v) = view {
            self.scaled.push(Scaled {
                guest,
                original: v,
                current: v,
                reference,
                size: (width, height),
                format,
                depth: false,
                target: true,
            });
        }
        Ok(ScaledSurface {
            guest,
            reference,
            view,
            size: (width, height),
        })
    }

    fn can_sample_surfaces(&self) -> bool {
        self.reference.can_sample_surfaces()
    }

    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        if self.single() {
            let texture = self.view.surface_texture(self.image(surface))?;
            return self.sampled(texture);
        }
        let Some(reference) = self.reference.surface_texture(surface.reference)? else {
            return Ok(None);
        };
        let view = match self.live(surface.view) {
            Some(v) => match self.view.surface_texture(v) {
                Ok(t) => t,
                Err(e) => {
                    self.view_failed("surface texture", e);
                    None
                }
            },
            None => self.share(&reference),
        };
        Ok(Some(ScaledTexture { reference, view }))
    }

    fn can_sample_depth_surfaces(&self) -> bool {
        self.reference.can_sample_depth_surfaces()
    }

    fn reusable_external_attachments(&self) -> bool {
        self.reference.reusable_external_attachments() && self.view.reusable_external_attachments()
    }

    /// Both copies record through this backend's own attachments and
    /// commit (the trait's [`Backend::run_scene_stage`]).
    fn runs_scene_stages(&self) -> bool {
        self.reference.runs_scene_stages() && self.view.runs_scene_stages()
    }

    fn external_attachments(
        &mut self, surfaces: crate::ExternalSurfaces<Self::Surface>, textures: [Option<&Self::Texture>; 4],
        copy: crate::TargetCopy,
    ) -> Result<crate::ExternalCopies, String> {
        if self.single() {
            let live = surfaces.try_map(|s| Some(self.image(*s))).unwrap();
            let vt = textures.map(|t| t.and_then(|t| t.view.as_ref()));
            if textures.iter().zip(vt).any(|(t, v)| t.is_some() && v.is_none()) {
                return Ok(crate::ExternalCopies::Missing(crate::TargetCopy::Display));
            }
            let mut result = self.view.external_attachments(live, vt, crate::TargetCopy::Display)?;
            if let crate::ExternalCopies::Ready(copies) = &mut result
                && let [one] = copies.as_mut_slice()
            {
                one.single_display = true;
            }
            return Ok(result);
        }
        let reference = surfaces.try_map(|s| Some(s.reference)).expect("every surface has a reference");
        let rt = textures.map(|t| t.map(|t| &t.reference));
        self.before_shared_write(&surfaces.color);
        for slot in surfaces.slots.iter().flatten() { self.before_shared_write(slot); }
        let mut commands = match self.reference.external_attachments(reference, rt, copy)? {
            crate::ExternalCopies::Ready(copies) => copies,
            missing => return Ok(missing),
        };
        let view = match surfaces.display_copy(self.awake(), self.frame,
            |r| self.shadow_factor() > 1 && self.shadow_at(r).is_some(), |v| self.live(v)) {
            Ok(Some(view)) => view,
            Ok(None) => return Ok(crate::ExternalCopies::Ready(commands)),
            Err(copy) => return Ok(crate::ExternalCopies::Missing(copy)),
        };
        let vt = textures.map(|t| t.and_then(|t| t.view.as_ref()));
        if (0..4).any(|i| textures[i].is_some() && vt[i].is_none()) { return Ok(crate::ExternalCopies::Missing(crate::TargetCopy::Display)); }
        match self.view.external_attachments(view, vt, crate::TargetCopy::Display)? {
            crate::ExternalCopies::Ready(display) => commands.extend(display),
            missing => return Ok(missing),
        }
        Ok(crate::ExternalCopies::Ready(commands))
    }

    fn commit_external(&mut self, recording: crate::ExternalRecording) {
        if recording.copy == crate::TargetCopy::Display {
            self.stats.display_draws += recording.ranges as u64;
        } else {
            self.stats.reference_draws += recording.ranges as u64;
        }
        match recording.copy {
            crate::TargetCopy::Reference => self.reference.commit_external(recording),
            crate::TargetCopy::Display => {
                self.view.commit_external(recording);
                self.view_pending = true;
            }
        }
    }

    fn upload_device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        let (device, queue) = self.reference.upload_device()?;
        let (view_device, view_queue) = self.view.upload_device()?;
        (device == view_device && queue == view_queue).then_some((device, queue))
    }

    fn supports_external_pass(&self) -> bool {
        self.reference.supports_external_pass()
    }

    fn external_pass(
        &mut self,
        color: Self::Surface,
        depth: Self::Surface,
        sampled: Vec<Option<Self::Surface>>,
        request: crate::backend::ExternalPassRequest,
        pass: Box<dyn crate::backend::ExternalPass>,
    ) -> Result<bool, String> {
        if self.single() {
            let levels = sampled.into_iter().map(|s| s.map(|s| self.image(s))).collect();
            let pass = SharedPass(Arc::new(Mutex::new(pass)), crate::TargetCopy::Display);
            let drawn = self.view.external_pass(self.image(color), self.image(depth), levels, request, Box::new(pass))?;
            self.stats.display_draws += u64::from(drawn);
            return Ok(drawn);
        }
        // Both copies get the pass: the reference (what the guest reads
        // back and what repairs come from) and the view (what is shown),
        // each sampling its own copy of the sampled levels.
        let mut shared = SharedPass(std::sync::Arc::new(std::sync::Mutex::new(pass)), crate::TargetCopy::Reference);
        let reference = sampled.iter().map(|s| s.as_ref().map(|s| s.reference)).collect();
        let view = sampled.iter().map(|s| s.as_ref().and_then(|s| self.live(s.view))).collect();
        let drawn = self
            .reference
            .external_pass(color.reference, depth.reference, reference, request, Box::new(shared.clone()))?;
        self.stats.reference_draws += u64::from(drawn);
        // A confirmed reference refusal returns the entire light to D3D,
        // which will draw both copies.
        if !drawn && crate::backend::ExternalPass::confirm_issue(&shared) {
            return Ok(false);
        }
        shared.1 = crate::TargetCopy::Display;
        if let (Some(c), Some(d)) = (self.view_surface(&color), self.view_surface(&depth))
        {
            if self.view.external_pass(c, d, view, request, Box::new(shared))? {
                self.stats.display_draws += 1;
            } else {
                self.view_failed("external pass", "the view backend refused it".to_string());
            }
        }
        Ok(drawn)
    }

    fn depth_level_texture(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<Self::Texture>, String> {
        if self.single() {
            let texture = self.view.depth_level_texture(self.image(surface))?;
            return self.sampled(texture);
        }
        // Depth levels (shadow maps) are compared texel by texel: the view
        // samples the reference's 1x bits, or with E3 lighting its own
        // scaled copy's (the comparison indexes the texels it has).
        let Some(reference) = self.reference.depth_level_texture(surface.reference)? else {
            return Ok(None);
        };
        let view = match self.shadow_texture(&surface.reference) {
            Some(t) => Some(t),
            None => self.share(&reference),
        };
        Ok(Some(ScaledTexture { reference, view }))
    }

    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        self.reference.judge()
    }

    fn handles_surface_aliases(&self) -> bool {
        self.single_copy
    }

    fn defers_depth_aliases(&self) -> bool {
        self.single() || self.reference.defers_depth_aliases()
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
        options: DecodeOptions,
    ) -> Result<Option<Self::Texture>, String> {
        if self.single() {
            let src = self.image(depth);
            let alias = self.view.depth_alias_texture(src, format, width, height, pitch, generation,
                offset, desc, options)?;
            if alias.is_some() && src != self.guest(depth) { self.stats.scaled_depth_aliases += 1; }
            return self.sampled(alias);
        }
        let Some(reference) = self.reference.depth_alias_texture(
            depth.reference,
            format,
            width,
            height,
            pitch,
            generation,
            offset,
            desc,
            options,
        )?
        else {
            return Ok(None);
        };
        // The whole buffer as one linear texture of its size (Halo's
        // 0x509368 and 0x50936C, the fog and distortion passes): made from
        // the view's scaled depth, on the GPU. The draw keeps the guest
        // size for texel addressing, so the scaled texture lines up.
        // Anything else is the reference's 1x texture.
        let s = self.scale;
        let whole = offset == 0
            && format == D3DFormat::LinD24S8
            && (desc.width, desc.height) == (width, height)
            && desc.pitch == pitch
            && desc.levels == 1
            && desc.kind == gfx::texture::TextureKind::D2
            && desc.format.layout() == gfx::format::Layout::Linear;
        let mut view = None;
        if let Some(vd) = self
            .live(depth.view)
            .filter(|_| whole && self.view.defers_depth_aliases())
        {
            let scaled = TextureDesc {
                width: desc.width * s,
                height: desc.height * s,
                pitch: desc.pitch * s,
                ..desc.clone()
            };
            match self.view.depth_alias_texture(
                vd,
                format,
                width * s,
                height * s,
                pitch * s,
                generation,
                0,
                &scaled,
                options,
            ) {
                Ok(Some(t)) => {
                    self.stats.scaled_depth_aliases += 1;
                    view = Some(t);
                }
                Ok(None) => {}
                Err(e) => self.view_failed("depth alias", e),
            }
        }
        let view = match view {
            Some(v) => Some(v),
            None => self.share(&reference),
        };
        Ok(Some(ScaledTexture { reference, view }))
    }

    fn can_mirror(&self) -> bool {
        self.reference.can_mirror()
    }

    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        if self.single() { self.view.submit_recorded()?; }
        self.reference.mirror_reserve(len)?;
        if self.single_copy { self.view.share_mirror(&self.reference); }
        Ok(())
    }

    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        if self.single() { return self.view.mirror_write(offset, bytes); }
        self.reference.mirror_write(offset, bytes)
    }

    fn mirror_store(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        if self.single() {
            let source = self.image(surface);
            self.view.mirror_store(source, format, width, height, layout, offset)?;
            if source != self.guest(surface) { self.stats.downsamples += 1; }
            return Ok(());
        }
        self.reference
            .mirror_store(surface.reference, format, width, height, layout, offset)
    }

    fn mirror_load(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        if self.single() {
            let guest = self.guest(surface);
            self.view.mirror_load(guest, format, width, height, layout, offset)?;
            let image = self.image(surface);
            if image != guest { self.view.resize(guest, image)?; }
            return Ok(());
        }
        self.before_shared_write(&surface);
        self.reference
            .mirror_load(surface.reference, format, width, height, layout, offset)?;
        self.refill(&surface);
        self.shadow_stale(&surface.reference);
        Ok(())
    }

    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        if self.single() {
            let bytes = self.view.mirror_read(offset, len)?;
            self.stats.cpu_readbacks += 1;
            return Ok(bytes);
        }
        let result = self.reference.mirror_read(offset, len)?;
        self.stats.cpu_readbacks += 1;
        Ok(result)
    }
}

/// `bytes`, `width x height` texels of `bpp` bytes in tight rows, with each
/// texel repeated into a `scale x scale` block.
fn repeat_texels(bytes: &[u8], width: u32, height: u32, bpp: u32, scale: u32) -> Vec<u8> {
    let (w, h, bpp, s) = (
        width as usize,
        height as usize,
        bpp as usize,
        scale as usize,
    );
    let mut out = Vec::with_capacity(bytes.len() * s * s);
    for y in 0..h {
        let row = &bytes[y * w * bpp..(y + 1) * w * bpp];
        let start = out.len();
        for texel in row.chunks_exact(bpp) {
            for _ in 0..s {
                out.extend_from_slice(texel);
            }
        }
        let end = out.len();
        for _ in 1..s {
            out.extend_from_within(start..end);
        }
    }
    out
}

impl ScaleView<GfxBackend> for GfxBackend {
    fn create_scaled_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
        scale: u32,
    ) -> Result<gfx::surface::SurfaceId, String> {
        let (w, h) = (width.checked_mul(scale), height.checked_mul(scale));
        let (Some(w), Some(h)) = (w, h) else {
            return Err(format!("{width}x{height} at {scale}x overflows"));
        };
        let s = self.create_surface(format, w, h, depth)?;
        if scale > 1 {
            self.scales.insert(s, scale);
        }
        Ok(s)
    }

    fn create_scaled_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        scale: u32,
    ) -> Result<gfx::surface::SurfaceId, String> {
        let (w, h) = (width.checked_mul(scale), height.checked_mul(scale));
        let (Some(w), Some(h)) = (w, h) else {
            return Err(format!("{width}x{height} at {scale}x overflows"));
        };
        let s = self.create_texture_target(format, w, h)?;
        if scale > 1 {
            self.scales.insert(s, scale);
        }
        Ok(s)
    }

    fn import_guest(&mut self, reference: &GfxBackend, s: gfx::surface::SurfaceId) -> Result<gfx::surface::SurfaceId, String> {
        if self.ctx.device != reference.ctx.device {
            return Err("render-scale backends must share a GPU device".into());
        }
        let surface = reference.store.get(s).map_err(|e| e.to_string())?;
        Ok(self.store.import(surface))
    }

    fn resize(&mut self, src: gfx::surface::SurfaceId, dst: gfx::surface::SurfaceId) -> Result<(), String> {
        self.resize_image(src, dst, None)
    }

    fn copy_scaled(&mut self, src: gfx::surface::SurfaceId, dst: gfx::surface::SurfaceId, extent: (u32, u32)) -> Result<(), String> {
        self.resize_image(src, dst, Some(extent))
    }

    fn export_texture(&self, texture: &HostTexture) -> Result<HostTexture, String> {
        Ok(texture.shared())
    }

    fn enable_surface_aliases(&mut self, reference: &mut GfxBackend) {
        self.single_copy_aliases = true;
        reference.single_copy_aliases = true;
    }

    fn move_counter(&mut self, reference: &mut GfxBackend, to_display: bool) {
        std::mem::swap(&mut self.counter, &mut reference.counter);
        self.count_samples = to_display;
    }

    fn share_mirror(&mut self, reference: &GfxBackend) {
        self.mirror = reference.mirror.clone();
    }

    fn stop_counting(&mut self) {
        self.count_samples = false;
    }

    fn share_results(&mut self, reference: &mut GfxBackend) {
        self.set_defer_side_passes(false);
        reference.set_defer_side_passes(false);
    }

    fn set_hard_shadows(&mut self, on: bool) {
        GfxBackend::set_hard_shadows(self, on);
    }

    fn share_texture(
        &mut self,
        reference: &GfxBackend,
        texture: &HostTexture,
    ) -> Option<HostTexture> {
        (reference.ctx.device == self.ctx.device).then(|| texture.shared())
    }

    fn submit_recorded(&mut self) -> Result<(), String> {
        self.flush()
    }

    fn fill_from(
        &mut self,
        reference: &mut GfxBackend,
        src: gfx::surface::SurfaceId,
        dst: gfx::surface::SurfaceId,
    ) -> Result<bool, String> {
        use gfx::surface::{SurfaceError, SurfaceKind, SurfaceLayout};
        reference.flush()?;
        self.flush()?;
        let s = reference.store.get(src).map_err(|e| e.to_string())?;
        let d = self.store.get(dst).map_err(|e| e.to_string())?;
        match (s.kind(), d.kind()) {
            (SurfaceKind::Color, SurfaceKind::Color) => {}
            (SurfaceKind::DepthStencil, SurfaceKind::DepthStencil) => {
                // Read the reference's words back, repeat each, and draw
                // them into the scaled depth (a guest write's path).
                let (sd, dd) = (*s.desc(), *d.desc());
                let scale = dd.width / sd.width.max(1);
                if scale == 0 || (sd.width * scale, sd.height * scale) != (dd.width, dd.height) {
                    return Err(format!(
                        "depth fill from {}x{} into {}x{}",
                        sd.width, sd.height, dd.width, dd.height
                    ));
                }
                let format = match sd.format {
                    D3DFormat::D24S8 | D3DFormat::LinD24S8 => D3DFormat::LinD24S8,
                    D3DFormat::D16 | D3DFormat::LinD16 => D3DFormat::LinD16,
                    // Float depth: no guest words to carry it.
                    _ => return Ok(false),
                };
                let bpp = format.bytes_per_texel().unwrap_or(4);
                let layout = SurfaceLayout::Linear {
                    pitch: sd.width * bpp,
                };
                let bytes =
                    match reference
                        .store
                        .read_depth_as(&reference.ctx, src, format, &layout)
                    {
                        Ok(b) => b,
                        Err(SurfaceError::NotReadable(..)) => return Ok(false),
                        Err(e) => return Err(e.to_string()),
                    };
                reference.readbacks += 1;
                let words = repeat_texels(&bytes, sd.width, sd.height, bpp, scale);
                let desc = TextureDesc::new(
                    format,
                    gfx::texture::TextureKind::D2,
                    dd.width,
                    dd.height,
                    1,
                    1,
                );
                self.store
                    .write_depth(&self.ctx, dst, &desc, &words)
                    .map_err(|e| e.to_string())?;
                return Ok(true);
            }
            (a, b) => return Err(format!("fill of a {b:?} surface from a {a:?} one")),
        }
        let (src_view, dst_view, format) = (s.view().clone(), d.view().clone(), d.wgpu_format());
        let ctx = &self.ctx;
        let blitter = self
            .blitter
            .get_or_insert_with(|| gfx::blit::Blitter::new(ctx));
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        // Nearest: each guest pixel becomes a scale x scale block.
        blitter.blit(
            ctx,
            &mut enc,
            &src_view,
            &dst_view,
            format,
            gfx::blit::BlitOptions::default(),
        );
        ctx.queue.submit([enc.finish()]);
        Ok(true)
    }

    fn present_reference(
        &mut self,
        reference: &GfxBackend,
        surface: gfx::surface::SurfaceId,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.present_from(Some(reference), surface, flags, gamma)
    }
}

/// The retail-or-E3 A/B capture ([`LightingAb`]): a second view over a
/// [`ScaledBackend`] used as its reference. The inner backend answers the
/// guest from its 1x reference and draws its own view (retail); this view
/// draws the same calls at the same scale with the other lighting. It
/// takes the inner reference's textures and pixels, as a view over a
/// plain backend does, so the two views differ only where lighting does.
impl ScaleView<ScaledBackend<GfxBackend, GfxBackend>> for GfxBackend {
    fn create_scaled_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
        scale: u32,
    ) -> Result<gfx::surface::SurfaceId, String> {
        <Self as ScaleView<GfxBackend>>::create_scaled_surface(self, format, width, height, depth, scale)
    }

    fn create_scaled_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        scale: u32,
    ) -> Result<gfx::surface::SurfaceId, String> {
        <Self as ScaleView<GfxBackend>>::create_scaled_texture_target(self, format, width, height, scale)
    }

    fn stop_counting(&mut self) {
        self.count_samples = false;
    }

    fn share_results(&mut self, _reference: &mut ScaledBackend<GfxBackend, GfxBackend>) {
        // The inner backend turned its own two off when it was made.
        self.set_defer_side_passes(false);
    }

    fn set_hard_shadows(&mut self, on: bool) {
        GfxBackend::set_hard_shadows(self, on);
    }

    fn share_texture(
        &mut self,
        reference: &ScaledBackend<GfxBackend, GfxBackend>,
        texture: &Tex<GfxBackend, GfxBackend>,
    ) -> Option<HostTexture> {
        <Self as ScaleView<GfxBackend>>::share_texture(self, reference.reference(), &texture.reference)
    }

    fn submit_recorded(&mut self) -> Result<(), String> {
        self.flush()
    }

    fn fill_from(
        &mut self,
        reference: &mut ScaledBackend<GfxBackend, GfxBackend>,
        src: Surf<GfxBackend, GfxBackend>,
        dst: gfx::surface::SurfaceId,
    ) -> Result<bool, String> {
        <Self as ScaleView<GfxBackend>>::fill_from(self, reference.reference_mut(), src.reference, dst)
    }

    fn present_reference(
        &mut self,
        reference: &ScaledBackend<GfxBackend, GfxBackend>,
        surface: Surf<GfxBackend, GfxBackend>,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.present_from(Some(reference.reference()), surface.reference, flags, gamma)
    }
}

/// A same-run A/B of the lighting modes (`H2_BOOT_GFX_LIGHTING_AB`): the
/// game and its 1x reference once, drawn at the render scale by two views
/// from the same calls, the inner one retail and the outer one E3.
pub type LightingAb = ScaledBackend<ScaledBackend<GfxBackend, GfxBackend>, GfxBackend>;

/// [`LightingAb`] over `reference`, a retail view and an E3 view at
/// `scale` (2 to [`MAX_SCALE`]).
pub fn lighting_ab(
    reference: GfxBackend,
    retail: GfxBackend,
    e3: GfxBackend,
    scale: u32,
) -> Result<LightingAb, String> {
    if scale < 2 {
        return Err(format!("the lighting A/B draws at a render scale of 2 or more, not {scale}"));
    }
    let inner = ScaledBackend::with_single_copy(reference, retail, scale, false)?;
    let mut outer = ScaledBackend::with_single_copy(inner, e3, scale, false)?;
    outer.set_lighting(Lighting::E3);
    Ok(outer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transactional_stages_require_live_frame_and_scaled_shadow_copies() {
        use crate::{ExternalSurfaces, TargetCopy};

        let frame = (80, 48);
        let local = ScaledSurface { guest: None, reference: 1, view: None, size: (20, 12) };
        let missing_frame = ScaledSurface { guest: None, reference: 2, view: None, size: frame };
        let shadow = ScaledSurface { guest: None, reference: 3, view: None, size: (16, 16) };
        let select = |surfaces: &ExternalSurfaces<ScaledSurface<u32, u32>>, awake, shadows| {
            surfaces.display_copy(awake, Some(frame), |r| shadows && *r == shadow.reference, |v| v)
        };
        let mut pass = ExternalSurfaces {
            color: local, depth: local, sampled: vec![], slots: vec![], slot_sizes: vec![],
        };
        assert!(matches!(select(&pass, true, true), Ok(None)));
        pass.color = missing_frame;
        assert!(matches!(select(&pass, false, false), Ok(None)));
        assert!(matches!(select(&pass, true, false), Err(TargetCopy::Display)));
        pass.color = local;
        pass.depth = shadow;
        assert!(matches!(select(&pass, true, false), Ok(None)));
        assert!(matches!(select(&pass, true, true), Err(TargetCopy::Display)));
        pass.depth = local;
        pass.slots = vec![Some(missing_frame)];
        pass.slot_sizes = vec![Some(frame)];
        assert!(matches!(select(&pass, true, false), Err(TargetCopy::Display)));
        pass.color.view = Some(10);
        pass.depth.view = Some(11);
        assert!(matches!(select(&pass, true, false), Err(TargetCopy::Display)));
        pass.slots[0].as_mut().unwrap().view = Some(12);
        let display = select(&pass, true, false).unwrap().unwrap();
        assert_eq!((display.color, display.depth, display.slots), (10, 11, vec![Some(12)]));
    }

    #[test]
    fn the_flag_takes_whole_scales_from_one_to_six() {
        assert_eq!(render_scale_from(None), Ok(1));
        assert_eq!(render_scale_from(Some("")), Ok(1));
        assert_eq!(render_scale_from(Some("1")), Ok(1));
        assert_eq!(render_scale_from(Some(" 4 ")), Ok(4));
        assert_eq!(render_scale_from(Some("6")), Ok(6));
        for bad in ["0", "7", "1.5", "2x", "-1"] {
            assert!(render_scale_from(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn lighting_is_retail_unless_e3_is_named() {
        assert_eq!(lighting_from(None), Ok(Lighting::Retail));
        assert_eq!(lighting_from(Some("")), Ok(Lighting::Retail));
        assert_eq!(lighting_from(Some("retail")), Ok(Lighting::Retail));
        assert_eq!(lighting_from(Some(" E3 ")), Ok(Lighting::E3));
        assert!(lighting_from(Some("stencil")).is_err());
        assert_eq!(Lighting::default(), Lighting::Retail);
    }

    #[test]
    fn e3_shadow_maps_follow_the_render_scale_and_are_at_least_doubled() {
        for scale in 1..=MAX_SCALE {
            assert_eq!(shadow_scale(scale, Lighting::Retail), 1);
        }
        let e3: Vec<u32> = (1..=MAX_SCALE)
            .map(|s| shadow_scale(s, Lighting::E3))
            .collect();
        assert_eq!(e3, [2, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn scale_one_builds_no_view() {
        assert!(view_for(1, || -> u32 { panic!("a view at 1x") }).is_none());
        assert_eq!(view_for(2, || 7u32), Some(7));
    }

    #[test]
    fn texels_repeat_into_blocks() {
        // 2x1 texels of 2 bytes at 3x: 6x3 texels.
        let out = repeat_texels(&[1, 2, 3, 4], 2, 1, 2, 3);
        let row = [1, 2, 1, 2, 1, 2, 3, 4, 3, 4, 3, 4];
        assert_eq!(out, [row, row, row].concat());
        assert_eq!(repeat_texels(&[9], 1, 1, 1, 1), [9]);
    }
}
