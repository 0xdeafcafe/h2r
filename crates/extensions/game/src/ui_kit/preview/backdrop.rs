//! The blur the game draws under a dialog, after the overlay colour's
//! fill (docs/game/ui-builder.md "A dialog is an overlay on its opener"):
//! the window stack's render calls `0x235756` with the screen's
//! transition fraction, which runs the Kawase blur `0x211a0` on the
//! screen.
//!
//! - **Steps.** `3.75 * fraction` (`[0x0045de14]`): at rest four draws,
//!   the last weighted by the fractional step (fractions 1, 1, 1, 0.75).
//! - **Spread.** Each draw's spread is its fraction times the scale; the
//!   scale starts at 1.0 and grows by 1.25 a step (`engine::glow::blur`).
//! - **Taps.** Each draw makes every pixel the mean of four bilinear taps,
//!   tap `k` moved by the spread times `[0x0045dbbc]` (0.5) for `k & 1`
//!   in x and `k & 2` in y, else `[0x0045dbc4]` (-0.5): the corners of a
//!   square around the pixel (`native_render::glow::quad`). Its sampler
//!   clamps at the edges.
//!
//! The four taps are a square, and a bilinear tap is a horizontal lerp
//! then a vertical one, so a draw is a two-tap mean across then a
//! two-tap mean down: the same sums, in half the reads.

use super::Canvas;

/// `[0x0045de14]`: the blur's steps per unit of transition fraction.
pub const STEPS: f32 = 3.75;
/// The first draw's spread scale.
pub const FIRST_SCALE: f32 = 1.0;
/// Each draw's scale over the last's.
pub const GROWTH: f32 = 1.25;
/// A tap's offset over the spread (`[0x0045dbbc]`; `[0x0045dbc4]` is its
/// negative).
pub const TAP: f32 = 0.5;
/// The transition fraction of a screen at rest.
pub const AT_REST: f32 = 1.0;

/// Each draw's spread for a screen at transition `fraction`.
pub fn spreads(fraction: f32) -> Vec<f32> {
    let steps = STEPS * fraction.clamp(0.0, 1.0);
    let (mut i, mut scale, mut out) = (0.0f32, FIRST_SCALE, Vec::new());
    while i < steps {
        out.push((steps - i).min(1.0) * scale);
        scale *= GROWTH;
        i += 1.0;
    }
    out
}

/// `c` blurred as the game blurs the screen at transition `fraction`.
pub fn blur(c: &mut Canvas, fraction: f32) {
    let out = blurred(&c.rgba, c.width as usize, c.height as usize, fraction);
    for (d, v) in c.rgba.iter_mut().zip(&out) {
        *d = v.round().clamp(0.0, 255.0) as u8;
    }
}

/// RGBA `rgba`, `w` by `h`, blurred at transition `fraction`, unrounded.
pub fn blurred(rgba: &[u8], w: usize, h: usize, fraction: f32) -> Vec<f32> {
    let mut a: Vec<f32> = rgba.iter().map(|v| f32::from(*v)).collect();
    let mut b = vec![0.0f32; a.len()];
    for spread in spreads(fraction) {
        pass(&a, &mut b, w, h, spread);
        std::mem::swap(&mut a, &mut b);
    }
    a
}

/// One bilinear tap along a line: the two texels and the weight of the
/// second.
#[derive(Clone, Copy)]
struct Tap {
    lo: usize,
    hi: usize,
    t: f32,
}

/// The tap at texel coordinate `u` (texel centres at whole numbers) on a
/// line of `n`, clamped to its ends.
fn tap(n: usize, u: f32) -> Tap {
    let f = u.floor();
    let last = n as isize - 1;
    let i = f as isize;
    Tap {
        lo: i.clamp(0, last) as usize,
        hi: (i + 1).clamp(0, last) as usize,
        t: u - f,
    }
}

/// Each position's two taps, `d` either side.
fn line_taps(n: usize, d: f32) -> Vec<[Tap; 2]> {
    (0..n)
        .map(|i| [tap(n, i as f32 - d), tap(n, i as f32 + d)])
        .collect()
}

/// One draw of spread `spread` from `src` into `dst` (RGBA, `w` by `h`).
pub fn pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, spread: f32) {
    let d = TAP * spread;
    let mut across = vec![0.0f32; src.len()];
    let xs = line_taps(w, d);
    for y in 0..h {
        let row = y * w;
        for (x, taps) in xs.iter().enumerate() {
            for ch in 0..4 {
                let at = |i: usize| src[(row + i) * 4 + ch];
                let v = |t: Tap| at(t.lo) + (at(t.hi) - at(t.lo)) * t.t;
                across[(row + x) * 4 + ch] = 0.5 * (v(taps[0]) + v(taps[1]));
            }
        }
    }
    let ys = line_taps(h, d);
    for (y, taps) in ys.iter().enumerate() {
        for x in 0..w {
            for ch in 0..4 {
                let at = |i: usize| across[(i * w + x) * 4 + ch];
                let v = |t: Tap| at(t.lo) + (at(t.hi) - at(t.lo)) * t.t;
                dst[(y * w + x) * 4 + ch] = 0.5 * (v(taps[0]) + v(taps[1]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_rest_the_blur_draws_four_times_the_last_at_three_quarters() {
        assert_eq!(spreads(AT_REST), [1.0, 1.25, 1.5625, 0.75 * 1.953125]);
        assert!(spreads(0.0).is_empty());
        assert_eq!(spreads(0.5), [1.0, 0.875 * 1.25]);
    }

    /// One draw of spread 1 is the 1 2 1 kernel each way: four taps at
    /// half a texel, each the mean of a two by two block.
    #[test]
    fn a_draw_of_spread_one_is_the_one_two_one_kernel() {
        let (w, h) = (5, 5);
        let mut src = vec![0.0f32; w * h * 4];
        src[(2 * w + 2) * 4] = 16.0;
        let mut dst = vec![0.0f32; src.len()];
        pass(&src, &mut dst, w, h, 1.0);
        let r = |x: usize, y: usize| dst[(y * w + x) * 4];
        assert_eq!(r(2, 2), 4.0);
        assert_eq!((r(1, 2), r(3, 2), r(2, 1), r(2, 3)), (2.0, 2.0, 2.0, 2.0));
        assert_eq!((r(1, 1), r(3, 3)), (1.0, 1.0));
        assert_eq!(r(0, 2), 0.0);
    }

    /// A white pixel on black spreads to its neighbours and keeps its
    /// light: exactly before rounding, and within half a level for each
    /// pixel it reaches after.
    #[test]
    fn a_white_pixel_spreads_and_keeps_its_light() {
        let (w, h) = (64u32, 48u32);
        let mut c = Canvas::new(w, h, [0, 0, 0, 255]);
        let i = ((24 * w + 32) * 4) as usize;
        c.rgba[i..i + 3].copy_from_slice(&[255, 255, 255]);
        let exact = blurred(&c.rgba, w as usize, h as usize, AT_REST);
        let reached = exact.chunks(4).filter(|p| p[0] > 0.0).count();
        let total: f32 = exact.chunks(4).map(|p| p[0]).sum();
        assert!((total - 255.0).abs() < 0.01, "{total}");
        blur(&mut c, AT_REST);
        assert!(c.rgba[i] < 255, "the pixel spread");
        assert!(c.rgba[i + 4] > 0 && c.rgba[i + (w as usize) * 4] > 0, "to its neighbours");
        for ch in 0..3 {
            let sum: f32 = c.rgba.chunks(4).map(|p| f32::from(p[ch])).sum();
            assert!(
                (sum - 255.0).abs() <= reached as f32 / 2.0,
                "channel {ch}: {sum} over {reached} pixels"
            );
        }
        assert!(c.rgba.chunks(4).all(|p| p[3] == 255), "opaque stays opaque");
    }

    /// A dialog drawn over a screen of ours (its `underlay`) draws the
    /// backdrop over that screen, as one over its opener does: the
    /// brightest pixel of `layout.list`'s rows is darker under
    /// `layout.dialog.over-screen` than in `layout.list` alone.
    #[test]
    fn a_dialog_over_a_screen_of_ours_draws_the_backdrop_over_it() {
        use crate::ui_kit::preview::{From, render_view_where};
        let from = From::Synthetic;
        let spec = crate::ui_kit::screens::sample("layout.dialog.over-screen").unwrap();
        let v = from.view(&spec, "sample").unwrap();
        let parent = v.underlay.as_deref().expect("drawn over layout.list");
        let src = from.source();
        let alone = render_view_where(parent, src, &|_| true, false);
        // The dialog's own widgets left out: the screen under it and the
        // backdrop.
        let under = render_view_where(&v, src, &|_| false, false);
        let lum = |c: &Canvas, (x, y): (i32, i32)| {
            let i = ((y as u32 * c.width + x as u32) * 4) as usize;
            c.rgba[i..i + 3].iter().map(|v| u32::from(*v)).sum::<u32>()
        };
        let at = parent
            .widgets
            .iter()
            .filter(|w| w.id.starts_with("row."))
            .flat_map(|w| {
                let [x, y, wd, h] = w.pixels();
                (x.max(0)..(x + wd).min(640))
                    .flat_map(move |px| (y.max(0)..(y + h).min(480)).map(move |py| (px, py)))
            })
            .max_by_key(|&p| lum(&alone, p))
            .expect("layout.list has rows");
        assert!(lum(&alone, at) > 100, "a lit pixel of the rows: {}", lum(&alone, at));
        assert!(
            lum(&under, at) < lum(&alone, at),
            "{at:?}: under the dialog {}, alone {}",
            lum(&under, at),
            lum(&alone, at)
        );
    }

    #[test]
    fn a_flat_canvas_is_unchanged() {
        let mut c = Canvas::new(640, 480, [0, 20, 44, 217]);
        let before = c.rgba.clone();
        blur(&mut c, AT_REST);
        assert!(c.rgba == before);
    }
}
