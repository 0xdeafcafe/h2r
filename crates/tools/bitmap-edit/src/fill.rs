//! Fill: a rectangle in one colour, or a flood by tolerance.

use crate::canvas::Canvas;
use crate::color::{Channels, replace};
use crate::error::Error;
use crate::rect::Rect;

/// Sets every pixel of `r` (clipped to the canvas) to `color` on the
/// enabled channels. Returns the clipped rectangle.
pub fn fill_rect(canvas: &mut Canvas, r: Rect, color: [u8; 4], channels: Channels) -> Rect {
    let r = r.intersect(canvas.bounds());
    if r.is_empty() {
        return r;
    }
    let (_, mut data) = canvas.read_rect(r);
    for px in data.chunks_exact_mut(4) {
        let old = [px[0], px[1], px[2], px[3]];
        px.copy_from_slice(&replace(old, color, channels));
    }
    // The rectangle is inside the canvas and the length is right.
    let _ = canvas.write_rect(r, &data);
    r
}

/// Fills the region of pixels connected (four ways) to (`x`, `y`) whose
/// every channel is within `tolerance` (0 to 1 of the channel's range) of
/// the seed's colour, limited to `within` when given. Returns the
/// rectangle covering the region.
///
/// The seed must lie inside `within` and the canvas.
pub fn flood(
    canvas: &mut Canvas,
    x: u32,
    y: u32,
    color: [u8; 4],
    tolerance: f32,
    within: Option<Rect>,
    channels: Channels,
) -> Result<Rect, Error> {
    if !tolerance.is_finite() || !(0.0..=1.0).contains(&tolerance) {
        return Err(Error::Params(format!("tolerance {tolerance} is not 0 to 1")));
    }
    let area = within.unwrap_or_else(|| canvas.bounds()).intersect(canvas.bounds());
    if !area.contains(x, y) {
        return Err(Error::Rect(format!(
            "the point {x},{y} is outside the area filled"
        )));
    }
    let seed = canvas.pixel(x, y);
    let limit = (tolerance * 255.0).round() as i32;
    let near = |p: [u8; 4]| (0..4).all(|c| (i32::from(p[c]) - i32::from(seed[c])).abs() <= limit);
    let w = area.w as usize;
    let mut seen = vec![false; w * area.h as usize];
    let at = |px: u32, py: u32| (py - area.y) as usize * w + (px - area.x) as usize;
    let mut stack = vec![(x, y)];
    seen[at(x, y)] = true;
    let mut dirty = Rect::default();
    while let Some((px, py)) = stack.pop() {
        let old = canvas.pixel(px, py);
        canvas.set_pixel(px, py, replace(old, color, channels));
        dirty = dirty.union(Rect::new(px, py, 1, 1));
        let mut visit = |nx: u32, ny: u32, canvas: &Canvas| {
            if area.contains(nx, ny) && !seen[at(nx, ny)] {
                seen[at(nx, ny)] = true;
                if near(canvas.pixel(nx, ny)) {
                    stack.push((nx, ny));
                }
            }
        };
        if px > 0 {
            visit(px - 1, py, canvas);
        }
        visit(px + 1, py, canvas);
        if py > 0 {
            visit(px, py - 1, canvas);
        }
        visit(px, py + 1, canvas);
    }
    Ok(dirty)
}
