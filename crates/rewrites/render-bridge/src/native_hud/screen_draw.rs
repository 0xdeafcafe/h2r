//! The UI's two guest screen draws made in Rust: the rectangle fill
//! `0x0013c050` (the dim under a dialog, `0x00147cdb`; the meters) and a
//! widget's debug outline `0x00235d69`. Each is an implementation of the
//! guest function, so its begin, state commit, vertex push and submit
//! (`0x0001eb00`, `0x0001ed70`, `0x0001ee60`, `0x00052040`) never run: the
//! draw is listed in the scene's overlay in the order the game makes it.
//!
//! The rectangle (`ECX` the bounds, `i16` top, left, bottom, right; `EAX`
//! the ARGB colour) is one quad of the rasterizer bitmap's second image
//! ([`bitmaps::rect_bitmap`]). The outline (`EAX` the widget's rectangle,
//! `ESI` its view's, `XMM0` the depth, one stack word pointing at four
//! floats, alpha first) projects the four corners about the view's centre
//! (`0x002360c3`, [`bitmaps::project`]), packs the colour as `0x00131e50`
//! does ([`bitmaps::tint`]) and strokes the closed strip of five points,
//! one pixel wide, over the corners' pixels.

use std::sync::Arc;

use guest_layout::memory::GuestMemory;
use frame_scene::{ClipRect, Overlay, OverlayBlend};
use hooks::Builder;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use super::ui_overlay::Ro;
use super::ui_overlay::bitmaps::{self, Lens, at};

/// The five points of the outline, in the order `0x00235d69` stores
/// them: the top left, bottom left, bottom right, top right, the top left
/// again. `rect` is `[top, left, bottom, right]`.
fn strip(project: impl Fn(f32, f32) -> [i16; 2], [top, left, bottom, right]: [i16; 4]) -> [[i16; 2]; 5] {
    let [top, left, bottom, right] = [top, left, bottom, right].map(f32::from);
    let a = project(left, top);
    [a, project(left, bottom), project(right, bottom), project(right, top), a]
}

/// The strip's four segments as pixel rectangles `[x0, y0, x1, y1]`, one
/// pixel across, or the first segment that is not axis-aligned.
fn strokes(points: &[[i16; 2]; 5]) -> Result<Vec<[f32; 4]>, [[i16; 2]; 2]> {
    points
        .windows(2)
        .map(|w| {
            let ([x0, y0], [x1, y1]) = (w[0], w[1]);
            let (x0, y0, x1, y1) = (i32::from(x0), i32::from(y0), i32::from(x1), i32::from(y1));
            if x0 != x1 && y0 != y1 {
                return Err([w[0], w[1]]);
            }
            let [lx, hx, ly, hy] = [x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1)];
            Ok([lx as f32, ly as f32, (hx + 1) as f32, (hy + 1) as f32])
        })
        .collect()
}

/// `points` as the overlay's fills in `color` (RGBA).
fn outline(points: &[[i16; 2]; 5], color: [f32; 4]) -> Overlay {
    let size = scene_extract::ui::CANVAS;
    let mut overlay = Overlay::new(size);
    let rects = strokes(points).unwrap_or_else(|s| unimplemented!("ui outline: a slanted segment {s:?}"));
    for rect in rects {
        overlay
            .push_fill(ClipRect::full(size), rect, color, OverlayBlend::ALPHA)
            .unwrap_or_else(|e| unimplemented!("ui outline: the overlay refused it: {e:?}"));
    }
    overlay
}

/// Lists `part` in the scene's overlay.
fn append(part: &Overlay, what: &str) {
    if crate::scene_step::with_overlay(|o| o.append(part).is_ok()) != Some(true) {
        unimplemented!("{what}: no scene overlay to list it in");
    }
}

/// `0x0013c050`: the rectangle fill.
fn rect_call(cpu: &mut CpuAbi, ctx: &mut ExecCtx, _addr: u32) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mem = rt.memory();
    let esp = cpu.esp;
    let ret = mem.u32(esp)?;
    let read = Ro(&*mem);
    let [top, left, bottom, right] =
        bitmaps::view(&read, cpu.ecx).unwrap_or_else(|| unimplemented!("ui rectangle: the bounds did not read")).map(f32::from);
    match bitmaps::rect_bitmap(&read, [left, top, right, bottom], cpu.eax) {
        Ok(Some(quad)) => {
            let mut part = Overlay::new(scene_extract::ui::CANVAS);
            scene_extract::ui::push_bitmap(&mut part, &quad)
                .unwrap_or_else(|e| unimplemented!("ui rectangle: the overlay refused it: {e:?}"));
            append(&part, "ui rectangle");
        }
        Ok(None) => {}
        Err(why) => unimplemented!("ui rectangle: {why:?}"),
    }
    cpu.esp = esp.wrapping_add(4);
    Ok(TransferAbi::ret(ret))
}

/// `0x00235d69`: a widget's outline.
fn outline_call(cpu: &mut CpuAbi, ctx: &mut ExecCtx, _addr: u32) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mem = rt.memory();
    let esp = cpu.esp;
    let ret = mem.u32(esp)?;
    let colour = mem.u32(esp.wrapping_add(4))?;
    let read = Ro(&*mem);
    let rect = bitmaps::view(&read, cpu.eax).unwrap_or_else(|| unimplemented!("ui outline: the rectangle did not read"));
    let view = bitmaps::view(&read, cpu.esi).unwrap_or_else(|| unimplemented!("ui outline: the view did not read"));
    let lens = Lens::read(&read).unwrap_or_else(|| unimplemented!("ui outline: the lens did not read"));
    let depth = f32::from_le_bytes([cpu.xmm[0][0], cpu.xmm[0][1], cpu.xmm[0][2], cpu.xmm[0][3]]);
    let word = |at: u32| mem.u32(at).map(f32::from_bits).unwrap_or_else(|_| unimplemented!("ui outline: the colour did not read"));
    let channel = word(at::CHANNEL);
    let argb = bitmaps::tint(word(colour), [word(colour + 4), word(colour + 8), word(colour + 12)], channel);
    let points = strip(
        |x, y| {
            let [px, py, _] = bitmaps::project(&lens, [x, y, depth], view);
            [bitmaps::truncated(px), bitmaps::truncated(py)]
        },
        rect,
    );
    append(&outline(&points, bitmaps::rgba(argb)), "ui outline");
    cpu.esp = esp.wrapping_add(8);
    Ok(TransferAbi::ret(ret))
}

pub(super) fn place(b: &mut Builder<'_>) {
    b.raw().implement(&guest_layout::points::UI_OVERLAY_RECT_DRAW, "native_ui_rect_fill", rect_call).context_only().optional();
    b.raw().implement(&guest_layout::points::UI_WIDGET_OUTLINE, "native_ui_outline", outline_call).context_only().optional();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strip runs top left, bottom left, bottom right, top right and
    /// back, projected corner by corner.
    #[test]
    fn the_strip_goes_round_the_rectangle_and_closes() {
        let points = strip(|x, y| [x as i16, y as i16], [10, 20, 50, 80]);
        assert_eq!(points, [[20, 10], [20, 50], [80, 50], [80, 10], [20, 10]]);
    }

    /// Each segment is a pixel-wide rectangle covering both end pixels.
    #[test]
    fn each_segment_is_one_pixel_across() {
        let points = [[20, 10], [20, 50], [80, 50], [80, 10], [20, 10]];
        assert_eq!(
            strokes(&points).unwrap(),
            [[20.0, 10.0, 21.0, 51.0], [20.0, 50.0, 81.0, 51.0], [80.0, 10.0, 81.0, 51.0], [20.0, 10.0, 81.0, 11.0]],
        );
        assert_eq!(strokes(&[[0, 0], [5, 5], [5, 5], [5, 5], [5, 5]]), Err([[0, 0], [5, 5]]));
    }
}
