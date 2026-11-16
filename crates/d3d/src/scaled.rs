//! Rendering above the guest's resolution without the guest seeing it
//! (docs/high-resolution.md, phase 1).
//!
//! [`ScaledBackend`] holds two backends that get the same calls. The
//! *reference* renders at the guest's size and answers everything the guest
//! can observe: surface and depth read-backs, pixel counts, draw outcomes,
//! the judge and every capability question. The *view* renders the frame
//! buffers and the depth buffer at an integer multiple of their size and is
//! what is shown. Nothing the view does reaches the device, so the guest's
//! values are the 1x run's by construction.
//!
//! What the view keeps at scale: every surface made with
//! [`Backend::create_surface`] at the frame size (the size of the first
//! colour surface, the back buffer): the frame buffers, the depth buffer and
//! copies of them. Their snapshots are copies of the scaled image. Every
//! other surface (render-to-texture levels, other sizes) lives only in the
//! reference, and the view samples the reference's texture of it (the two
//! share a device), so offscreen passes run once. Textures are uploaded once
//! and shared the same way, and so are depth textures (depth levels and
//! textures over the depth buffer): phase 1 gives the view the reference's
//! 1x depth texture.
//!
//! Known gaps, all view-only (the guest cannot tell): a draw into a
//! guest-size target with the scaled depth buffer bound updates only the
//! reference's depth; guest bytes written into a scaled depth surface do not
//! reach the view; stipple is indexed by host pixel. They are counted in
//! [`ScaleStats`].
//!
//! A view failure never reaches the guest: it is counted and that frame is
//! shown from the reference's image.

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
    /// Counted draws stop counting (the reference answers every count).
    fn stop_counting(&mut self);
    /// This backend's handle for `reference`'s texture. `None`: it cannot
    /// sample it.
    fn share_texture(&mut self, reference: &R, texture: &R::Texture) -> Option<Self::Texture>;
    /// Submits work recorded so far, before the reference changes a
    /// surface this backend may be sampling.
    fn submit_recorded(&mut self) -> Result<(), String>;
    /// Fills scaled colour surface `dst` from `reference`'s guest-size
    /// surface `src`, each guest pixel repeated. `Ok(false)`: it cannot (a
    /// depth surface).
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
    /// View draws and clears whose outcome differed from the reference's.
    pub mismatches: u64,
    /// Draws, clears and guest writes that changed the reference's depth
    /// buffer but not the view's (a guest-size colour target with the
    /// scaled depth buffer bound, or a guest write into it).
    pub depth_missed: u64,
    /// Frames shown from the reference after a view failure.
    pub fallback_frames: u64,
    /// Frames shown from the view.
    pub view_frames: u64,
}

/// A reference backend at the guest's size and a view backend at `scale`
/// (module docs).
pub struct ScaledBackend<R: Backend, V: ScaleView<R>> {
    reference: R,
    view: V,
    scale: u32,
    /// The frame size: the first colour surface's (the back buffer).
    frame: Option<(u32, u32)>,
    /// Scaled colour surfaces, for clears of the scaled depth buffer bound
    /// with a guest-size colour target.
    scaled_colors: Vec<(V::Surface, (u32, u32))>,
    /// The view has recorded work not yet submitted.
    view_pending: bool,
    /// A view call failed since the last present.
    frame_failed: bool,
    stats: ScaleStats,
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

    fn compare(&mut self, what: &str, reference: &DrawOutcome, view: Result<DrawOutcome, String>) {
        match view {
            Ok(v) if v == *reference => {}
            Ok(v) => {
                self.stats.mismatches += 1;
                if self.stats.mismatches <= 10 {
                    eprintln!(
                        "d3d: render scale {}: view {what} {v:?}, reference {reference:?}",
                        self.scale
                    );
                }
            }
            Err(e) => self.view_failed(what, e),
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

    /// A scaled colour surface the size of depth surface `size`, to clear
    /// the scaled depth through.
    fn proxy_color(&self, size: (u32, u32)) -> Option<V::Surface> {
        self.scaled_colors
            .iter()
            .find(|(_, s)| *s == size)
            .map(|(c, _)| *c)
    }

    fn share(&mut self, t: &R::Texture) -> Option<V::Texture> {
        self.view.share_texture(&self.reference, t)
    }

    /// Refreshes the view's copy of scaled surface `s` from the reference's
    /// after a guest write.
    fn refill(&mut self, s: &ScaledSurface<R::Surface, V::Surface>) {
        let Some(v) = s.view else { return };
        match self.view.fill_from(&mut self.reference, s.reference, v) {
            Ok(true) => {}
            Ok(false) => self.stats.depth_missed += 1,
            Err(e) => self.view_failed("fill", e),
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
            self.scaled_colors.push((v, (width, height)));
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
            self.scaled_colors.retain(|(c, _)| *c != v);
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
                    Some(proxy) => self.view.clear(
                        proxy,
                        vd,
                        Clear {
                            color: None,
                            ..clear
                        },
                    ),
                    None => {
                        self.stats.depth_missed += 1;
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
        let vd = depth.and_then(|d| d.view);
        match (color.view, depth) {
            (Some(vc), d) if d.is_none() || vd.is_some() => {
                let vt = textures.map(|t| t.and_then(|t| t.view.as_ref()));
                if textures
                    .iter()
                    .zip(&vt)
                    .any(|(t, v)| t.is_some() && v.is_none())
                {
                    // A texture the view cannot sample: it would read a
                    // stand-in. Leave the view's pixels as they are.
                    self.stats.mismatches += 1;
                    return Ok(out);
                }
                let r = self.view.draw(vc, vd, draw, vt);
                self.view_pending = true;
                self.compare("draw", &out, r);
            }
            // Colour scaled, depth not: attachments of different sizes.
            (Some(_), _) => self.stats.mismatches += 1,
            (None, _) if vd.is_some() && out == DrawOutcome::Drawn => self.stats.depth_missed += 1,
            (None, _) => {}
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
                Some(proxy) => {
                    let depth_only = RectClear {
                        color: None,
                        ..clear.clone()
                    };
                    self.view.clear_rects(proxy, vd, &depth_only)
                }
                None => {
                    self.stats.depth_missed += 1;
                    return Ok(out);
                }
            },
            _ => return Ok(out),
        };
        self.view_pending = true;
        self.compare("clear", &out, r);
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
                    Ok(false) => self.refill(&dst),
                    Err(e) => self.view_failed("copy", e),
                },
                None => self.refill(&dst),
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
        // Phase 1: render-to-texture levels stay at the guest's size, in
        // the reference only.
        let reference = self
            .reference
            .create_texture_target(format, width, height)?;
        Ok(ScaledSurface {
            reference,
            view: None,
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
        // Phase 1: the view samples the reference's 1x depth bits.
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
        // Phase 1: built from the reference's 1x depth buffer.
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
        let view = self.share(&reference);
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
        use gfx::surface::SurfaceKind;
        reference.flush()?;
        self.flush()?;
        let s = reference.store.get(src).map_err(|e| e.to_string())?;
        let d = self.store.get(dst).map_err(|e| e.to_string())?;
        if s.kind() != SurfaceKind::Color || d.kind() != SurfaceKind::Color {
            return Ok(false);
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
}
