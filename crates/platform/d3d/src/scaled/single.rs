//! Both sizes live in the display encoder's store. The guest-sized handle
//! aliases the reference store's allocation, including for unscaled levels.
use super::*;

impl<R: Backend, V: ScaleView<R>> ScaledBackend<R, V> {
    pub(super) fn single(&self) -> bool {
        self.single_copy && self.scale != RenderScale::ONE
    }

    pub(super) fn guest(&self, s: Surf<R, V>) -> V::Surface {
        s.guest.expect("single-copy surfaces have a guest-size handle")
    }

    pub(super) fn image(&self, s: Surf<R, V>) -> V::Surface {
        self.live(s.view).or_else(|| {
            if self.lighting == Lighting::Retail { return None; }
            self.shadow_at(&s.reference).and_then(|i| {
                self.shadows[i].view.as_ref().filter(|v| !v.stale_depth && !v.stale_stencil).map(|v| v.depth)
            })
        }).unwrap_or_else(|| self.guest(s))
    }

    pub(super) fn guest_image(&mut self, s: Surf<R, V>) -> Result<V::Surface, String> {
        let src = self.image(s);
        let dst = self.guest(s);
        if src != dst {
            self.view.resize(src, dst)?;
            self.stats.downsamples += 1;
        }
        Ok(dst)
    }

    pub(super) fn sampled(&self, view: Option<V::Texture>) -> Result<Option<Tex<R, V>>, String> {
        view.map(|t| Ok(ScaledTexture {
            reference: self.view.export_texture(&t)?,
            view: Some(t),
        })).transpose()
    }

    // Make an E3 shadow's scaled attachment current before its first draw
    // or clear. The unscaled level remains the source until this succeeds.
    fn shadow_attachment(&mut self, depth: Surf<R, V>) -> Result<Option<(V::Surface, V::Surface)>, String> {
        if self.lighting == Lighting::Retail { return Ok(None); }
        let Some(i) = self.shadow_at(&depth.reference) else { return Ok(None) };
        let guest = self.guest(depth);
        let Some(v) = self.shadow_view(i) else { return Ok(None) };
        let (target, proxy, stale) = (v.depth, v.proxy, v.stale_depth || v.stale_stencil);
        if stale {
            self.view.resize(guest, target)?;
            let v = self.shadows[i].view.as_mut().unwrap();
            v.stale_depth = false;
            v.stale_stencil = false;
        }
        Ok(Some((target, proxy)))
    }

    pub(super) fn draw_single(&mut self, color: Surf<R, V>, depth: Option<Surf<R, V>>,
        draw: &ResolvedDraw, textures: [Option<&Tex<R, V>>; 4]) -> Result<DrawOutcome, String> {
        let vt = textures.map(|t| t.and_then(|t| t.view.as_ref()));
        if textures.iter().zip(vt).any(|(t, v)| t.is_some() && v.is_none()) {
            return Err("display texture was not shared".into());
        }
        let mut c = self.image(color);
        let mut z = depth.map(|d| self.image(d));
        let mut proxy_color = None;
        if let Some(d) = depth && let Some((target, proxy)) = self.shadow_attachment(d)? {
            z = Some(target);
            if c == self.guest(color) && draw.state.render.color_write.xbox_value() == 0 {
                proxy_color = Some((proxy, D3DFormat::LinA8R8G8B8));
            }
        }
        let scaled_color = c != self.guest(color);
        let scaled_depth = depth.is_some_and(|d| z != Some(self.guest(d)));
        // Mixed-size attachments must use the same raster grid. Run a
        // guest-size intermediate once and transfer only its scaled partner.
        if proxy_color.is_none() && !scaled_color && scaled_depth
            && draw.state.render.color_write.xbox_value() == 0
        {
            proxy_color = self.proxy_color(depth.expect("scaled depth").size);
        }
        let mixed = proxy_color.is_none() && depth.is_some() && scaled_color != scaled_depth;
        if mixed {
            c = self.guest_image(color)?;
            let d = depth.expect("mixed draw has depth");
            z = Some(if draw.state.render.depth_mode != DepthBufferMode::Disabled || draw.state.render.stencil.enable {
                self.guest_image(d)?
            } else {
                self.guest(d)
            });
        }
        let out = if let Some((proxy, format)) = proxy_color {
            let mut d = draw.clone();
            (d.target.width, d.target.height) = depth.unwrap().size;
            d.target.format = format;
            self.view.draw(proxy, z, &d, vt)?
        } else {
            self.view.draw(c, z, draw, vt)?
        };
        if out == DrawOutcome::Drawn {
            self.stats.display_draws += 1;
            if proxy_color.is_some() { self.stats.proxy_depth_draws += 1; }
            if mixed && scaled_color && draw.state.render.color_write.xbox_value() != 0 {
                self.view.resize(c, self.image(color))?;
            }
            if mixed && scaled_depth && writes_depth_or_stencil(draw) {
                let d = depth.unwrap();
                self.view.resize(self.guest(d), self.image(d))?;
            }
        }
        Ok(out)
    }

    // Clears of a guest-size colour and a scaled depth need no readback.
    // They can clear the aspects separately through compatible attachments.
    pub(super) fn clear_single(&mut self, color: Surf<R, V>, depth: Option<Surf<R, V>>,
        clear: Clear, rects: Option<&RectClear>) -> Result<DrawOutcome, String> {
        let c = self.image(color);
        let mut z = depth.map(|d| self.image(d));
        let shadow = depth.map(|d| self.shadow_attachment(d)).transpose()?.flatten();
        if let Some((target, _)) = shadow { z = Some(target); }
        let mixed = depth.is_some_and(|d| (c != self.guest(color)) != (z != Some(self.guest(d))));
        let apply = |view: &mut V, c, z, clear: Clear| {
            if let Some(rc) = rects {
                view.clear_rects(c, z, &RectClear { color: rc.color.filter(|_| clear.color.is_some()), depth: clear.depth, stencil: clear.stencil, ..rc.clone() })
            } else {
                view.clear(c, z, clear).map(|()| DrawOutcome::Drawn)
            }
        };
        if !mixed { return apply(&mut self.view, c, z, clear); }
        if clear.color.is_some() {
            let out = apply(&mut self.view, c, None, Clear { color: clear.color, depth: None, stencil: None })?;
            if out != DrawOutcome::Drawn { return Ok(out); }
        }
        if clear.depth.is_none() && clear.stencil.is_none() { return Ok(DrawOutcome::Drawn); }
        let d = depth.unwrap();
        let proxy = if z == Some(self.guest(d)) {
            self.guest(color)
        } else if let Some((_, proxy)) = shadow {
            proxy
        } else {
            self.proxy_color(d.size).ok_or("missing scaled colour for depth clear")?.0
        };
        apply(&mut self.view, proxy, z, Clear { color: None, ..clear })
    }
}
