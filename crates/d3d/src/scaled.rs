//! Rendering above the guest's resolution without the guest seeing it
//! (docs/high-resolution.md, phases 1 and 2).
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

use std::sync::{Arc, Mutex};

use gfx::state::{ColorWriteMask, DepthBufferMode, StencilOp};
use gfx::texture::{DecodeOptions, DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::backend::{Backend, DrawJudge, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw};
use crate::gfx_backend::{GfxBackend, HostTexture};

/// The largest render scale: 6x of 640x480 is 3840x2880.
pub const MAX_SCALE: u32 = 6;

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
    /// Counted draws stop counting (the reference answers every count).
    fn stop_counting(&mut self);
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
    /// The view's scaled surface; `None` for a surface only the reference
    /// keeps (or one the view failed to make).
    pub view: Option<V>,
    /// Guest size.
    pub size: (u32, u32),
}

/// A [`ScaledBackend`] texture.
#[derive(Debug)]
pub struct ScaledTexture<R, V> {
    /// The reference's texture.
    pub reference: R,
    /// What the view samples: its own (a snapshot of a scaled surface) or
    /// the reference's, shared. `None`: the view cannot sample it.
    pub view: Option<V>,
}

/// What the view did differently from the reference. None of it reaches
/// the guest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScaleStats {
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
}

impl std::fmt::Display for ScaleStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} view frames, {} fallback frames, {} view errors, {} mismatches, {} depth missed, \
             {} colour resyncs, {} depth resyncs, {} depth-only proxy draws, {} scaled depth aliases",
            self.view_frames,
            self.fallback_frames,
            self.view_errors,
            self.mismatches,
            self.depth_missed,
            self.color_resyncs,
            self.depth_resyncs,
            self.proxy_depth_draws,
            self.scaled_depth_aliases
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
}

/// A reference backend at the guest's size and a view backend at `scale`
/// (module docs).
pub struct ScaledBackend<R: Backend, V: ScaleView<R>> {
    reference: R,
    view: V,
    scale: u32,
    /// The frame size: the first colour surface's (the back buffer).
    frame: Option<(u32, u32)>,
    /// Scaled colour surfaces and their guest size and format, for clears
    /// and depth-only draws of the scaled depth buffer bound with a
    /// guest-size colour target.
    scaled_colors: Vec<(V::Surface, (u32, u32), D3DFormat)>,
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
    /// `reference` answers the guest; `view` renders at `scale` (1 to
    /// [`MAX_SCALE`]) and presents. They must share a device.
    pub fn new(reference: R, mut view: V, scale: u32) -> Result<Self, String> {
        if !(1..=MAX_SCALE).contains(&scale) {
            return Err(format!("render scale {scale} is not 1 to {MAX_SCALE}"));
        }
        view.stop_counting();
        Ok(Self {
            reference,
            view,
            scale,
            frame: None,
            scaled_colors: Vec::new(),
            view_pending: false,
            frame_failed: false,
            stats: ScaleStats::default(),
            logged: 0,
            shared: None,
        })
    }

    /// The scale.
    pub fn scale(&self) -> u32 {
        self.scale
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
        if s.view.is_none() && self.view_pending {
            self.view_pending = false;
            if let Err(e) = self.view.submit_recorded() {
                self.view_failed("submit", e);
            }
        }
    }

    /// A scaled colour surface of guest size `size`, and its format, to
    /// clear or draw the scaled depth through.
    fn proxy_color(&self, size: (u32, u32)) -> Option<(V::Surface, D3DFormat)> {
        self.scaled_colors
            .iter()
            .find(|(_, s, _)| *s == size)
            .map(|(c, _, f)| (*c, *f))
    }

    fn share(&mut self, t: &R::Texture) -> Option<V::Texture> {
        self.view.share_texture(&self.reference, t)
    }

    /// Refreshes the view's copy of scaled surface `s` from the
    /// reference's. `false`: it could not (counted).
    fn refill(&mut self, s: &ScaledSurface<R::Surface, V::Surface>) -> bool {
        let Some(v) = s.view else { return true };
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
        if color.view.is_some() && self.refill(color) {
            self.stats.color_resyncs += 1;
        }
        if let Some(d) = depth
            && d.view.is_some()
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
        let Some(vd) = depth.view else { return };
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
        if let (Some(v), false) = (view, depth) {
            self.scaled_colors.push((v, (width, height), format));
        }
        Ok(ScaledSurface {
            reference,
            view,
            size: (width, height),
        })
    }

    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        self.before_shared_write(&surface);
        self.reference.destroy_surface(surface.reference)?;
        if let Some(v) = surface.view {
            self.scaled_colors.retain(|(c, _, _)| *c != v);
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
        self.before_shared_write(&color);
        self.reference
            .clear(color.reference, depth.map(|d| d.reference), clear)?;
        let vd = depth.and_then(|d| d.view);
        let r = match (color.view, depth) {
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
        self.before_shared_write(&color);
        let out = self.reference.draw(
            color.reference,
            depth.map(|d| d.reference),
            draw,
            textures.map(|t| t.map(|t| &t.reference)),
        )?;
        let drawn = out == DrawOutcome::Drawn;
        let vd = depth.and_then(|d| d.view);
        let vt = textures.map(|t| t.and_then(|t| t.view.as_ref()));
        let sampled = textures
            .iter()
            .zip(&vt)
            .all(|(t, v)| t.is_none() || v.is_some());
        match (color.view, depth) {
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
        // The reference ends its frame too (its swap ring, shader upkeep);
        // it shows nothing.
        self.reference
            .present_with_gamma(surface.reference, flags, gamma)?;
        self.view_pending = false;
        let shown = match surface.view {
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
            self.stats.fallback_frames += 1;
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
        if let Some(s) = &self.shared
            && let Ok(mut s) = s.lock()
        {
            *s = self.stats;
        }
        Ok(())
    }

    fn clear_rects(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        self.before_shared_write(&color);
        let out = self
            .reference
            .clear_rects(color.reference, depth.map(|d| d.reference), clear)?;
        if out != DrawOutcome::Drawn {
            return Ok(out);
        }
        let vd = depth.and_then(|d| d.view);
        let touches_depth = clear.depth.is_some() || clear.stencil.is_some();
        let r = match (color.view, depth) {
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
        self.reference
            .read_surface(surface.reference, format, width, height, layout)
    }

    fn read_depth(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.reference
            .read_depth(surface.reference, format, width, height, layout)
    }

    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        self.before_shared_write(&dst);
        let copied = self.reference.copy_surface(src.reference, dst.reference)?;
        if copied && let Some(vd) = dst.view {
            match src.view {
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
        self.before_shared_write(&surface);
        let written = self
            .reference
            .write_surface(surface.reference, desc, bytes)?;
        if written {
            self.refill(&surface);
        }
        Ok(written)
    }

    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        let Some(reference) = self.reference.snapshot_surface(surface.reference, opaque)? else {
            return Ok(None);
        };
        let view = match surface.view {
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
        self.reference.clear_pixel_count()
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.reference.report_pixel_count()
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.reference.read_pixel_count(ticket)
    }

    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        self.reference.try_pixel_count(ticket)
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        self.reference.drop_pixel_count(ticket)
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
        if let Some(v) = view {
            self.scaled_colors.push((v, (width, height), format));
        }
        Ok(ScaledSurface {
            reference,
            view,
            size: (width, height),
        })
    }

    fn can_sample_surfaces(&self) -> bool {
        self.reference.can_sample_surfaces()
    }

    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        let Some(reference) = self.reference.surface_texture(surface.reference)? else {
            return Ok(None);
        };
        let view = match surface.view {
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

    fn depth_level_texture(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<Self::Texture>, String> {
        // Depth levels (shadow maps) are compared texel by texel: the view
        // samples the reference's 1x bits.
        let Some(reference) = self.reference.depth_level_texture(surface.reference)? else {
            return Ok(None);
        };
        let view = self.share(&reference);
        Ok(Some(ScaledTexture { reference, view }))
    }

    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        self.reference.judge()
    }

    fn defers_depth_aliases(&self) -> bool {
        self.reference.defers_depth_aliases()
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
        if let Some(vd) = depth
            .view
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
        self.reference.mirror_reserve(len)
    }

    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
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
        self.before_shared_write(&surface);
        self.reference
            .mirror_load(surface.reference, format, width, height, layout, offset)?;
        self.refill(&surface);
        Ok(())
    }

    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.reference.mirror_read(offset, len)
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

    fn stop_counting(&mut self) {
        self.count_samples = false;
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

#[cfg(test)]
mod tests {
    use super::*;

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
