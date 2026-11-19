//! The debug menu over the game (docs/runtime/debug-menu.md): the lines
//! `debug_menu::layout` makes, in the watermark's font and drawing path
//! (CPU rasterised into one texture when the text changes, one alpha
//! blended quad), on a dark plate in the top left of the game image.
//! Each line has a tone: the selected row on a lighter bar, greyed rows
//! dim, errors red.

use debug_menu::{Line, Tone};

use crate::overlay::Sprite;
use crate::scale::Viewport;
use crate::watermark::{CELL, PITCH, glyph, scale_for};

/// Room around the text, in font pixels.
const PAD: u32 = 4;
/// Distance from the image's top and left edges, in font pixels.
const MARGIN: u32 = 6;
/// The plate: near black, mostly opaque.
const PLATE: [u8; 4] = [10, 12, 16, 210];
/// The selected row's bar.
const BAR: [u8; 4] = [52, 84, 120, 235];

/// A tone's text colour.
pub fn colour(t: Tone) -> [u8; 3] {
    match t {
        Tone::Title => [255, 214, 120],
        Tone::Normal => [228, 232, 238],
        Tone::Selected => [255, 255, 255],
        Tone::Greyed => [118, 124, 134],
        Tone::Hint => [150, 170, 190],
        Tone::Error => [255, 110, 100],
    }
}

/// The plate for `lines` at `scale`: (width, height) in pixels.
pub fn size(lines: &[Line], scale: u32) -> (u32, u32) {
    let cols = lines
        .iter()
        .map(|l| l.text.chars().count())
        .max()
        .unwrap_or(0) as u32;
    let rows = lines.len() as u32;
    if cols == 0 || rows == 0 {
        return (0, 0);
    }
    let s = scale.max(1);
    ((cols * CELL + 2 * PAD) * s, (rows * PITCH + 2 * PAD) * s)
}

/// One step bigger than the watermark (it is read, not glanced at), and
/// smaller until the plate fits the image.
pub fn scale_in(lines: &[Line], area: Viewport) -> u32 {
    let mut s = scale_for(area.height) + 1;
    while s > 1 {
        let (w, h) = size(lines, s);
        if w + 2 * MARGIN * s <= area.width && h + 2 * MARGIN * s <= area.height {
            break;
        }
        s -= 1;
    }
    s
}

/// Where the plate goes in `area`, and at what scale.
pub fn place(lines: &[Line], area: Viewport) -> (u32, (u32, u32)) {
    let s = scale_in(lines, area);
    (s, (area.x + MARGIN * s, area.y + MARGIN * s))
}

/// `lines` as straight-alpha RGBA at `scale`: (width, height, pixels).
pub fn rasterize(lines: &[Line], scale: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = size(lines, scale);
    if w == 0 {
        return (0, 0, Vec::new());
    }
    let s = scale.max(1);
    let mut px: Vec<u8> = PLATE
        .iter()
        .copied()
        .cycle()
        .take((w * h * 4) as usize)
        .collect();
    for (r, line) in lines.iter().enumerate() {
        let top = (PAD + r as u32 * PITCH) * s;
        if line.tone == Tone::Selected {
            for y in top.saturating_sub(s)..(top + (CELL + 1) * s).min(h) {
                for x in 0..w {
                    let o = ((y * w + x) * 4) as usize;
                    px[o..o + 4].copy_from_slice(&BAR);
                }
            }
        }
        let rgb = colour(line.tone);
        for (i, c) in line.text.chars().enumerate() {
            for (gy, bits) in glyph(c).iter().enumerate() {
                for gx in 0..CELL {
                    if bits & (1 << gx) == 0 {
                        continue;
                    }
                    let x0 = (PAD + i as u32 * CELL + gx) * s;
                    let y0 = top + gy as u32 * s;
                    for dy in 0..s {
                        for dx in 0..s {
                            let o = (((y0 + dy) * w + x0 + dx) * 4) as usize;
                            px[o..o + 3].copy_from_slice(&rgb);
                            px[o + 3] = 255;
                        }
                    }
                }
            }
        }
    }
    (w, h, px)
}

#[derive(Clone, Copy)]
pub(crate) enum PlatePosition {
    TopLeft,
    TopRight,
    LeftCenter,
    BottomCenter,
}

/// Draws the menu over an already composited target.
pub struct MenuRenderer {
    sprite: Sprite,
    /// The lines and scale last rasterised.
    drawn: Option<(Vec<Line>, u32)>,
}

impl MenuRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            sprite: Sprite::new(device, "app debug menu"),
            drawn: None,
        }
    }

    /// Records the pass drawing `lines` in the top left of `area` (the
    /// game image) in `dst`. Re-rasterises only when they changed.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        lines: &[Line],
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
        area: Viewport,
    ) {
        self.draw_at(device, queue, encoder, lines, dst, dst_format, dst_size, area, PlatePosition::TopLeft);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_at(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        lines: &[Line],
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
        area: Viewport,
        position: PlatePosition,
    ) {
        if lines.is_empty() {
            return;
        }
        let area = if area.is_empty() {
            Viewport {
                x: 0,
                y: 0,
                width: dst_size.0,
                height: dst_size.1,
            }
        } else {
            area
        };
        let (scale, (left, top)) = place(lines, area);
        let (w, h) = size(lines, scale);
        let margin = MARGIN * scale;
        let at = match position {
            PlatePosition::TopLeft => (left, top),
            PlatePosition::TopRight => (area.x + area.width.saturating_sub(w + margin), top),
            PlatePosition::LeftCenter => (left, area.y + area.height.saturating_sub(h) / 2),
            PlatePosition::BottomCenter => (
                area.x + area.width.saturating_sub(w) / 2,
                area.y + area.height.saturating_sub(h + margin),
            ),
        };
        if self
            .drawn
            .as_ref()
            .is_none_or(|(l, s)| l != lines || *s != scale)
        {
            let (w, h, px) = rasterize(lines, scale);
            self.sprite.upload(device, queue, (w, h), &px);
            self.drawn = Some((lines.to_vec(), scale));
        }
        self.sprite
            .draw(device, encoder, dst, dst_format, dst_size, at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(text: &str, tone: Tone) -> Line {
        Line {
            text: text.into(),
            tone,
        }
    }

    fn sample() -> Vec<Line> {
        vec![
            l("DEBUG MENU", Tone::Title),
            l("> fog            < on >", Tone::Selected),
            l("  overrides          off", Tone::Greyed),
        ]
    }

    #[test]
    fn the_plate_fits_the_text_and_the_image() {
        let lines = sample();
        assert_eq!(size(&lines, 1), (24 * 8 + 8, 3 * 10 + 8));
        assert_eq!(size(&lines, 2), (2 * (24 * 8 + 8), 2 * 38));
        let area = |w, h| Viewport {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        // One step above the watermark's scale.
        assert_eq!(scale_in(&lines, area(1280, 960)), 2);
        assert_eq!(scale_in(&lines, area(2560, 1920)), 3);
        // A full menu in a small window comes down until it fits.
        let wide = vec![l(&"x".repeat(48), Tone::Normal); 22];
        let s = scale_in(&wide, area(640, 480));
        assert_eq!(s, 1);
        let (w, h) = size(&wide, s);
        assert!(w <= 640 && h <= 480);
        // Top left of the game image, letterbox included.
        let boxed = Viewport {
            x: 160,
            y: 0,
            width: 960,
            height: 720,
        };
        assert_eq!(place(&lines, boxed), (2, (172, 12)));
        assert_eq!(place(&lines, area(1280, 960)), (2, (12, 12)));
    }

    #[test]
    fn tones_colour_the_text_and_the_selected_row_has_a_bar() {
        let lines = sample();
        let (w, h, px) = rasterize(&lines, 1);
        assert_eq!(px.len(), (w * h * 4) as usize);
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..][..4];
        // The plate between letters.
        assert_eq!(at(1, 1), &PLATE);
        // The selected row's bar, beyond its text.
        assert_eq!(at(w - 2, PAD + PITCH + 4), &BAR);
        // Some pixel of each line is lit in its tone's colour.
        for (r, line) in lines.iter().enumerate() {
            let top = PAD + r as u32 * PITCH;
            let lit = (top..top + CELL)
                .any(|y| (0..w).any(|x| at(x, y)[..3] == colour(line.tone) && at(x, y)[3] == 255));
            assert!(lit, "line {r} ({:?}) not drawn", line.tone);
        }
        assert_ne!(colour(Tone::Greyed), colour(Tone::Normal));
        assert_eq!(rasterize(&[], 2), (0, 0, Vec::new()));
    }

    #[test]
    fn draws_over_the_frame() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let (w, h) = (640u32, 480u32);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut menu = MenuRenderer::new(&ctx.device);
        let area = Viewport {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        menu.draw(
            &ctx.device,
            &ctx.queue,
            &mut enc,
            &sample(),
            &view,
            format,
            (w, h),
            area,
        );
        ctx.queue.submit([enc.finish()]);
        let px = gfx::readback::read_texture(&ctx, &tex, wgpu::TextureAspect::All, 4, (w, h));
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..][..4];
        // Outside the plate the frame is untouched; inside, the dark plate.
        assert_eq!(at(w - 2, h - 2), &[255, 255, 255, 255]);
        assert_eq!(at(2, 2), &[255, 255, 255, 255], "the margin");
        let (_, (x, y)) = place(&sample(), area);
        let p = at(x + 1, y + 1);
        assert!(p[0] < 80 && p[1] < 80 && p[2] < 80, "{p:?}");
    }
}
