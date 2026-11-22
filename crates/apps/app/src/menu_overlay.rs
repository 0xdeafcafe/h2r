//! The debug menu over the game (docs/runtime/debug-menu.md "Layout"):
//! the lines `debug_menu::layout` makes, in the watermark's font and
//! drawing path (CPU rasterised into one texture when the text changes,
//! one alpha blended quad), on a dark panel in the top left of the game
//! image. The first line sits on a title bar; rules are thin drawn lines
//! and headings carry one to the right edge; the selected row is on a bar
//! exactly the panel's width; greyed rows are dim, errors red, results
//! green. The same plate, narrower and with an accent strip in its tone,
//! shows a lone result for a few seconds after the menu closes, placed
//! clear of the window's other plates.

use debug_menu::{Line, Tone};

use crate::NoticeKind;
use crate::overlay::Sprite;
use crate::scale::Viewport;
use crate::watermark::{CELL, PITCH, glyph, scale_for};

/// Room around the text at the sides and bottom (and the top, without a
/// title bar), in font pixels.
const PAD: u32 = 6;
/// The title bar's height; its text is centred in it.
const TITLE_H: u32 = 16;
/// Between the title bar and the first line under it.
const GAP: u32 = 4;
/// The height a rule takes; its line is [`RULE_AT`] down, one pixel
/// thick, so the space above and below it is the same.
const RULE_H: u32 = 7;
const RULE_AT: u32 = 2;
/// A lone result's accent strip, down its left edge.
const STRIP: u32 = 2;
/// Distance from the image's top and left edges (and from a plate it
/// keeps clear of), in font pixels.
const MARGIN: u32 = 6;
/// The panel: near black, mostly opaque.
const PLATE: [u8; 4] = [10, 12, 16, 215];
/// The title bar.
const TITLE_BAR: [u8; 4] = [30, 46, 66, 240];
/// The selected row's bar.
const BAR: [u8; 4] = [52, 84, 120, 235];
/// Rules and a heading's line.
const RULE: [u8; 4] = [70, 86, 106, 235];

/// A tone's text colour (a rule's line colour for [`Tone::Rule`]).
pub fn colour(t: Tone) -> [u8; 3] {
    match t {
        Tone::Title => [236, 240, 244],
        Tone::Normal => [214, 220, 228],
        Tone::Selected => [255, 255, 255],
        Tone::Greyed => [118, 124, 134],
        Tone::Hint => [150, 166, 184],
        Tone::Error => [255, 110, 100],
        Tone::Done => [150, 226, 160],
        Tone::Heading => [126, 172, 220],
        Tone::Rule => [RULE[0], RULE[1], RULE[2]],
    }
}

/// How a window notice reads inside the open menu
/// (`debug_menu::layout::lines_with`).
pub fn notice_tone(kind: NoticeKind) -> Tone {
    match kind {
        NoticeKind::Info => Tone::Hint,
        NoticeKind::Ready => Tone::Done,
        NoticeKind::Error | NoticeKind::Crashed => Tone::Error,
    }
}

/// Whether `lines` are the open menu (a title bar first), not a lone
/// result.
fn is_menu(lines: &[Line]) -> bool {
    lines.first().is_some_and(|l| l.tone == Tone::Title)
}

/// Each line's top and the plate's height, in font pixels.
fn tops(lines: &[Line]) -> (Vec<u32>, u32) {
    let mut out = Vec::with_capacity(lines.len());
    let mut y = PAD;
    for (i, l) in lines.iter().enumerate() {
        if i == 0 && l.tone == Tone::Title {
            out.push((TITLE_H - CELL) / 2);
            y = TITLE_H + GAP;
            continue;
        }
        out.push(y);
        y += if l.tone == Tone::Rule { RULE_H } else { PITCH };
    }
    let h = match lines.last() {
        None => 0,
        Some(_) if lines.len() == 1 && is_menu(lines) => TITLE_H,
        Some(l) if l.tone == Tone::Rule => y - RULE_H + RULE_AT + 1 + PAD,
        Some(_) => y - PITCH + CELL + PAD,
    };
    (out, h)
}

/// The plate's width in characters: the menu's is always the layout's
/// full width, so it doesn't change size from page to page.
fn cols(lines: &[Line]) -> u32 {
    let text = lines
        .iter()
        .map(|l| l.text.chars().count())
        .max()
        .unwrap_or(0);
    let cols = if is_menu(lines) {
        text.max(debug_menu::layout::COLS)
    } else {
        text
    };
    cols as u32
}

/// The plate for `lines` at `scale`: (width, height) in pixels.
pub fn size(lines: &[Line], scale: u32) -> (u32, u32) {
    let cols = cols(lines);
    if cols == 0 || lines.is_empty() {
        return (0, 0);
    }
    let s = scale.max(1);
    let (_, h) = tops(lines);
    ((cols * CELL + 2 * PAD) * s, h * s)
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

/// Whether two rectangles share a pixel.
pub fn intersects(a: Viewport, b: Viewport) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && a.x < b.x + b.width
        && b.x < a.x + a.width
        && a.y < b.y + b.height
        && b.y < a.y + a.height
}

/// Where the plate goes in `area`, and at what scale: the top left, moved
/// down below any of `avoid` (the window's banner and editor plate, in
/// the same pixels) it would cover.
pub fn place(lines: &[Line], area: Viewport, avoid: &[Viewport]) -> (u32, (u32, u32)) {
    let s = scale_in(lines, area);
    let (width, height) = size(lines, s);
    let x = area.x + MARGIN * s;
    let mut y = area.y + MARGIN * s;
    // Each step moves strictly down, past a rect it covered.
    while let Some(below) = avoid
        .iter()
        .filter(|a| intersects(Viewport { x, y, width, height }, **a))
        .map(|a| a.y + a.height)
        .max()
    {
        y = below + MARGIN * s;
    }
    (s, (x, y))
}

/// Fills `x` by `y` (pixels) of the `w` by `h` image with `rgba`.
fn fill(
    px: &mut [u8],
    (w, h): (u32, u32),
    x: std::ops::Range<u32>,
    y: std::ops::Range<u32>,
    rgba: [u8; 4],
) {
    for yy in y.start..y.end.min(h) {
        for xx in x.start..x.end.min(w) {
            let o = ((yy * w + xx) * 4) as usize;
            px[o..o + 4].copy_from_slice(&rgba);
        }
    }
}

/// `lines` as straight-alpha RGBA at `scale`: (width, height, pixels).
pub fn rasterize(lines: &[Line], scale: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = size(lines, scale);
    if w == 0 {
        return (0, 0, Vec::new());
    }
    let s = scale.max(1);
    let wh = (w, h);
    let mut px: Vec<u8> = PLATE
        .iter()
        .copied()
        .cycle()
        .take((w * h * 4) as usize)
        .collect();
    if is_menu(lines) {
        fill(&mut px, wh, 0..w, 0..TITLE_H * s, TITLE_BAR);
    } else if let Some(first) = lines.first() {
        let c = colour(first.tone);
        fill(&mut px, wh, 0..STRIP * s, 0..h, [c[0], c[1], c[2], 255]);
    }
    let (tops, _) = tops(lines);
    for (line, &top) in lines.iter().zip(&tops) {
        let top = top * s;
        match line.tone {
            Tone::Selected => {
                fill(&mut px, wh, 0..w, top.saturating_sub(s)..top + (CELL + 1) * s, BAR);
            }
            Tone::Rule => {
                let y = top + RULE_AT * s;
                fill(&mut px, wh, 0..w, y..y + s, RULE);
            }
            Tone::Heading => {
                let n = line.text.chars().count() as u32;
                let y = top + (CELL / 2) * s;
                fill(&mut px, wh, (PAD + (n + 1) * CELL) * s..w, y..y + s, RULE);
            }
            _ => {}
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
    /// game image) in `dst`, clear of the plates in `avoid` ([`place`]).
    /// Re-rasterises only when they changed.
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
        avoid: &[Viewport],
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
        let (scale, at) = place(lines, area, avoid);
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
    use debug_menu::layout::COLS;

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

    /// A menu with every kind of line.
    fn full() -> Vec<Line> {
        vec![
            l("DEBUG MENU > Player", Tone::Title),
            l("mainmenu \u{b7} tick 376 \u{b7} 11 fps \u{b7} offline", Tone::Hint),
            l("", Tone::Rule),
            l("  Favourites", Tone::Heading),
            l("> God mode                    < off >", Tone::Selected),
            l("  Give weapon  >", Tone::Normal),
            l("", Tone::Rule),
            l("makes you take no damage", Tone::Hint),
            l("", Tone::Rule),
            l(debug_menu::layout::CONTROLS, Tone::Hint),
        ]
    }

    fn area(w: u32, h: u32) -> Viewport {
        Viewport {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn the_plate_fits_the_text_and_the_image() {
        let lines = sample();
        // The menu is always the layout's full width; its height is the
        // title bar, the gap, two rows and the bottom padding.
        let width = COLS as u32 * CELL + 2 * PAD;
        let height = TITLE_H + GAP + PITCH + CELL + PAD;
        assert_eq!(size(&lines, 1), (width, height));
        assert_eq!(size(&lines, 2), (2 * width, 2 * height));
        // One step above the watermark's scale.
        assert_eq!(scale_in(&lines, area(1280, 960)), 2);
        assert_eq!(scale_in(&lines, area(2560, 1920)), 3);
        // A full menu in a small window comes down until it fits.
        let wide = vec![l(&"x".repeat(COLS), Tone::Normal); 22];
        let s = scale_in(&wide, area(640, 480));
        assert_eq!(s, 1);
        let (w, h) = size(&wide, s);
        assert!(w <= 640 && h <= 480);
        // Top left of the game image, letterbox included.
        let boxed = Viewport {
            x: 160,
            y: 0,
            width: 1120,
            height: 840,
        };
        assert_eq!(place(&lines, boxed, &[]), (2, (172, 12)));
        assert_eq!(place(&lines, area(1280, 960), &[]), (2, (12, 12)));
        // A lone result fits its text, with no title bar.
        let toast = [l("Reload: done", Tone::Done)];
        assert_eq!(size(&toast, 1), (12 * CELL + 2 * PAD, CELL + 2 * PAD));
    }

    #[test]
    fn padding_and_the_space_around_rules_are_even() {
        let (tops_, h) = tops(&full());
        let last = *tops_.last().unwrap();
        assert_eq!(h - (last + CELL), PAD, "bottom padding");
        // The title's text is centred in its bar.
        assert_eq!(tops_[0] * 2 + CELL, TITLE_H);
        assert_eq!(tops_[1], TITLE_H + GAP);
        // A rule's line has as much room above it as below it.
        let r = 2;
        let above = tops_[r] + RULE_AT - (tops_[r - 1] + CELL);
        let below = tops_[r + 1] - (tops_[r] + RULE_AT + 1);
        assert_eq!(above, below);
    }

    #[test]
    fn rules_and_the_selected_bar_span_exactly_the_panel() {
        let lines = full();
        let s = 2;
        let (w, h, px) = rasterize(&lines, s);
        let at = |x: u32, y: u32| -> [u8; 4] {
            px[((y * w + x) * 4) as usize..][..4].try_into().unwrap()
        };
        let (tops_, _) = tops(&lines);
        // The title bar across the top, edge to edge.
        assert_eq!(at(0, 0), TITLE_BAR);
        assert_eq!(at(w - 1, TITLE_H * s - 1), TITLE_BAR);
        assert_eq!(at(w - 1, TITLE_H * s), PLATE);
        // Every rule is a thin line from the left edge to the right one,
        // with the plate right above and below it.
        for (i, line) in lines.iter().enumerate() {
            if line.tone != Tone::Rule {
                continue;
            }
            let y = (tops_[i] + RULE_AT) * s;
            assert!((0..w).all(|x| at(x, y) == RULE && at(x, y + s - 1) == RULE), "rule {i}");
            assert_eq!(at(w / 2, y - 1), PLATE, "rule {i}: thin");
            assert_eq!(at(w / 2, y + s), PLATE, "rule {i}: thin");
        }
        // The selected bar: from x 0 to the panel's last pixel, no further.
        let sel = lines.iter().position(|l| l.tone == Tone::Selected).unwrap();
        let y = (tops_[sel] + CELL) * s; // under the glyphs, inside the bar
        assert!((0..w).all(|x| at(x, y) == BAR), "the bar is the panel's width");
        assert_eq!(at(w - 1, y), BAR);
        assert_eq!(px.len(), (w * h * 4) as usize, "and the panel is all there is");
        // A heading's line runs from after its text to the edge.
        let hd = lines.iter().position(|l| l.tone == Tone::Heading).unwrap();
        let y = (tops_[hd] + CELL / 2) * s;
        assert_eq!(at(w - 1, y), RULE);
        assert_ne!(at(PAD * s, y), RULE);
    }

    #[test]
    fn tones_colour_the_text() {
        let lines = sample();
        let (w, _, px) = rasterize(&lines, 1);
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..][..4];
        let (tops_, _) = tops(&lines);
        // Some pixel of each line is lit in its tone's colour.
        for (r, line) in lines.iter().enumerate() {
            let top = tops_[r];
            let lit = (top..top + CELL)
                .any(|y| (0..w).any(|x| at(x, y)[..3] == colour(line.tone) && at(x, y)[3] == 255));
            assert!(lit, "line {r} ({:?}) not drawn", line.tone);
        }
        assert_ne!(colour(Tone::Greyed), colour(Tone::Normal));
        assert_ne!(colour(Tone::Done), colour(Tone::Error));
        assert_eq!(rasterize(&[], 2), (0, 0, Vec::new()));
        // A lone result has its tone's strip down the left edge.
        let toast = [l("nope", Tone::Error)];
        let (w, h, px) = rasterize(&toast, 1);
        let c = colour(Tone::Error);
        assert_eq!(&px[((h - 1) * w * 4) as usize..][..4], &[c[0], c[1], c[2], 255]);
        assert_eq!(&px[(STRIP * 4) as usize..][..4], &PLATE);
    }

    #[test]
    fn window_notices_take_the_menus_tones() {
        assert_eq!(notice_tone(NoticeKind::Info), Tone::Hint);
        assert_eq!(notice_tone(NoticeKind::Ready), Tone::Done);
        assert_eq!(notice_tone(NoticeKind::Error), Tone::Error);
        assert_eq!(notice_tone(NoticeKind::Crashed), Tone::Error);
    }

    #[test]
    fn a_lone_result_keeps_clear_of_the_banner_and_the_editor_plate() {
        let toast = vec![l(&"x".repeat(COLS), Tone::Done)];
        let screen = area(1280, 960);
        let (s, (x, y)) = place(&toast, screen, &[]);
        let (w, h) = size(&toast, s);
        let rect = |x, y| Viewport { x, y, width: w, height: h };
        // The banner in the top left, the editor plate in the top right
        // under it: the result goes below both.
        let banner = Viewport { x: 12, y: 12, width: 700, height: 28 };
        let editor = Viewport { x: 900, y: 52, width: 368, height: 28 };
        assert!(intersects(rect(x, y), banner));
        let (_, (x2, y2)) = place(&toast, screen, &[banner, editor]);
        assert_eq!(x2, x);
        assert!(!intersects(rect(x2, y2), banner));
        assert!(!intersects(rect(x2, y2), editor));
        assert_eq!(y2, editor.y + editor.height + MARGIN * s);
        // Nothing in the way: it stays put.
        let far = Viewport { x: 1200, y: 900, width: 40, height: 40 };
        assert_eq!(place(&toast, screen, &[far]).1, (x, y));
        assert!(!intersects(banner, Viewport { x: 12, y: 40, width: 10, height: 10 }));
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
        menu.draw(
            &ctx.device,
            &ctx.queue,
            &mut enc,
            &sample(),
            &view,
            format,
            (w, h),
            area(w, h),
            &[],
        );
        ctx.queue.submit([enc.finish()]);
        let px = gfx::readback::read_texture(&ctx, &tex, wgpu::TextureAspect::All, 4, (w, h));
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..][..4];
        // Outside the plate the frame is untouched; inside, the dark plate.
        assert_eq!(at(w - 2, h - 2), &[255, 255, 255, 255]);
        assert_eq!(at(2, 2), &[255, 255, 255, 255], "the margin");
        let (s, (x, y)) = place(&sample(), area(w, h), &[]);
        let (_, ph) = size(&sample(), s);
        let p = at(x + 1, y + ph - 2);
        assert!(p[0] < 80 && p[1] < 80 && p[2] < 80, "{p:?}");
    }
}
