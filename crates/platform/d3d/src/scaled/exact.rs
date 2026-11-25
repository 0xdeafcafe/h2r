//! Mixed attachment grids in the exact display copy. The colour draw uses
//! its target's texel spacing. Depth/stencil tests read the display depth
//! at those positions, including when the reference has a different lens.
use super::*;

pub(super) struct Scratch<S> {
    size: (u32, u32),
    format: D3DFormat,
    grid: RenderScale,
    image: S,
}

impl<R: Backend, V: ScaleView<R>> ScaledBackend<R, V> {
    pub(super) fn exact_scratch(
        &mut self,
        size: (u32, u32),
        format: D3DFormat,
        grid: RenderScale,
    ) -> Result<V::Surface, String> {
        let depth = format.is_depth();
        let slot = if depth { &mut self.mixed_depth } else { &mut self.scratch_color };
        if let Some(s) = slot
            && (s.size, s.format, s.grid) == (size, format, grid)
        {
            return Ok(s.image);
        }
        let image = self.view.create_scaled_surface(format, size.0, size.1, depth, grid)?;
        if let Some(old) = slot.replace(Scratch { size, format, grid, image }) {
            self.view.destroy_surface(old.image)?;
        }
        Ok(image)
    }

    pub(super) fn mixed_draw(
        &mut self,
        color: V::Surface,
        color_grid: RenderScale,
        depth: Surf<R, V>,
        depth_image: V::Surface,
        draw: &ResolvedDraw,
        textures: [Option<&V::Texture>; 4],
    ) -> Result<DrawOutcome, String> {
        if draw.state.render.depth_mode == DepthBufferMode::Disabled && !draw.state.render.stencil.enable {
            return self.view.draw(color, None, draw, textures);
        }
        let depth_grid = self.surface_grid(depth.size);
        let format = draw.target.depth_format.ok_or("mixed draw has no depth format")?;
        let scratch = self.exact_scratch(depth.size, format, color_grid)?;
        self.view.resize(depth_image, scratch)?;
        let out = self.view.draw(color, Some(scratch), draw, textures)?;
        if out == DrawOutcome::Drawn && writes_depth_or_stencil(draw) {
            if depth_grid == RenderScale::ONE {
                self.view.resize(scratch, depth_image)?;
            } else {
                // A small colour draw can also write a frame-sized depth.
                // Repeat its coverage on that grid so an edge in the depth
                // test keeps its display samples for the next world draw.
                self.depth_only_draw(&depth, draw, textures, true);
            }
        }
        Ok(out)
    }

    pub(super) fn clear_exact(
        &mut self,
        color: Surf<R, V>,
        depth: Option<Surf<R, V>>,
        clear: Clear,
        rects: Option<&RectClear>,
    ) -> Result<DrawOutcome, String> {
        let vc = self.live(color.view);
        let vd = depth.and_then(|d| self.live(d.view));
        let apply = |view: &mut V, c, z, aspects: Clear| {
            if let Some(rc) = rects {
                if aspects == clear { return view.clear_rects(c, z, rc); }
                view.clear_rects(c, z, &RectClear {
                    color: rc.color.filter(|_| aspects.color.is_some()),
                    depth: aspects.depth,
                    stencil: aspects.stencil,
                    ..rc.clone()
                })
            } else {
                view.clear(c, z, aspects).map(|()| DrawOutcome::Drawn)
            }
        };
        self.view_pending |= vc.is_some() || vd.is_some();
        if let Some(c) = vc {
            if depth.is_none_or(|d| self.surface_grid(d.size) == self.surface_grid(color.size)) {
                return apply(&mut self.view, c, vd, clear);
            }
            if clear.color.is_some() {
                let out = apply(&mut self.view, c, None, Clear { color: clear.color, ..Clear::NONE })?;
                if out != DrawOutcome::Drawn { return Ok(out); }
            }
        }
        if let (Some(d), Some(z)) = (depth, vd)
            && (clear.depth.is_some() || clear.stencil.is_some())
        {
            let proxy = match self.proxy_color(d.size) {
                Some((c, _)) => c,
                None => self.exact_scratch(d.size, D3DFormat::LinA8R8G8B8, self.surface_grid(d.size))?,
            };
            return apply(&mut self.view, proxy, Some(z), Clear { color: None, ..clear });
        }
        Ok(DrawOutcome::Drawn)
    }
}
