//! Glyph coverage resampled to the display's density
//! (docs/graphics/high-resolution.md, "Text"). The game's fonts are
//! bitmap glyphs rasterised once at their face size with 4-bit alpha;
//! drawn at a window several times the guest's 480p they magnify soft
//! (bilinear) or stair-stepped (point). A glyph page filled at the
//! display's scale draws each texel near 1:1 instead, and the upscale is
//! computed once per glyph from the coverage the font already carries —
//! no glyph data is stored or shipped.
//!
//! The upscale reads the coverage as alpha, which across an edge rises
//! about linearly with the distance to it, crossing one half on the edge
//! (the premise of the translated glyph filter, `shaders::psh::GLYPH_WGSL`,
//! which runs per fragment; this runs once per glyph on the CPU):
//!
//! - `a`, a Catmull-Rom surface through the 4x4 texels around the sample:
//!   it passes through every texel, so a one-texel stem keeps its alpha,
//!   a straight edge between flat texels stays on the boundary, and it
//!   smooths the 4-bit steps of the coverage instead of following them;
//! - `m`, the same spline through each texel's brightest neighbour (3x3):
//!   the full alpha of the stroke the sample is near, 1 for solid text,
//!   lower for a stroke that never covered a whole texel;
//! - the edge is put back where `a` crosses `m / 2`, with a ramp one
//!   output texel wide (`k` = the scale), drawn at alpha `m`. Faint
//!   strokes keep their weight instead of vanishing under a fixed
//!   threshold.
//!
//! Unlike the texture the shader reads, a glyph's own edge is extended
//! (clamped) rather than read as the empty border around a cache slot: a
//! stroke that runs to the glyph's edge stays solid to it.

/// One glyph's coverage (`width * height` bytes, 0 to 255, row-major) at
/// `scale` times its size: `width * scale` by `height * scale` bytes.
/// `scale` 0 or 1 (or an empty glyph) returns the input unchanged: the
/// reconstruction only pays where the glyph is magnified.
pub fn upsample(alpha: &[u8], width: u16, height: u16, scale: u32) -> Vec<u8> {
    let (w, h) = (u32::from(width) as usize, u32::from(height) as usize);
    if scale <= 1 || w == 0 || h == 0 || alpha.len() != w * h {
        return alpha.to_vec();
    }
    let s = scale as f32;
    let (w1, h1) = (w as isize, h as isize);
    let at = |x: isize, y: isize| -> f32 {
        let x = x.clamp(0, w1 - 1) as usize;
        let y = y.clamp(0, h1 - 1) as usize;
        alpha[y * w + x] as f32 / 255.0
    };
    let mut out = vec![0u8; w * scale as usize * h * scale as usize];
    for oy in 0..h * scale as usize {
        // The output texel's centre in source texel space; the spline
        // runs through the source texels' centres (offset by half a texel).
        let py = (oy as f32 + 0.5) / s - 0.5;
        let base_y = py.floor();
        let fy = py - base_y;
        let wy = catmull_rom(fy);
        for ox in 0..w * scale as usize {
            let px = (ox as f32 + 0.5) / s - 0.5;
            let base_x = px.floor();
            let fx = px - base_x;
            let wx = catmull_rom(fx);
            let (bx, by) = (base_x as isize, base_y as isize);
            let mut t = [0.0f32; 36];
            for y in 0..6 {
                for x in 0..6 {
                    t[y * 6 + x] = at(bx + x as isize - 2, by + y as isize - 2);
                }
            }
            let mut a = 0.0;
            let mut m = 0.0;
            for y in 0..4 {
                for x in 0..4 {
                    let weight = wx[x] * wy[y];
                    let i = (y + 1) * 6 + x + 1;
                    a += weight * t[i];
                    let top = t[i - 7].max(t[i - 6]).max(t[i - 5]);
                    let mid = t[i - 1].max(t[i]).max(t[i + 1]);
                    let bottom = t[i + 5].max(t[i + 6]).max(t[i + 7]);
                    m += weight * top.max(mid).max(bottom);
                }
            }
            let m = m.clamp(0.0, 1.0);
            let n = (a / m.max(1e-4)).clamp(0.0, 1.0);
            let alpha_out = m * (((n - 0.5) * s + 0.5).clamp(0.0, 1.0));
            out[oy * (w * scale as usize) + ox] = (alpha_out * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// The Catmull-Rom spline's four weights at `t` in 0..1: a cubic through
/// the texel centres, so a stroke one texel wide keeps its full alpha at
/// its centre (a B-spline would smooth it to two thirds) and a straight
/// edge between two flat texels still crosses half at the boundary.
fn catmull_rom(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        (-t3 + 2.0 * t2 - t) / 2.0,
        (3.0 * t3 - 5.0 * t2 + 2.0) / 2.0,
        (-3.0 * t3 + 4.0 * t2 + t) / 2.0,
        (t3 - t2) / 2.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(w: u16, h: u16, scale: u32, rows: &[&[u8]]) -> Vec<u8> {
        let alpha: Vec<u8> = rows.concat();
        upsample(&alpha, w, h, scale)
    }

    #[test]
    fn scale_zero_and_one_return_the_input() {
        let alpha = [0, 128, 255, 64];
        assert_eq!(upsample(&alpha, 2, 2, 0), alpha);
        assert_eq!(upsample(&alpha, 2, 2, 1), alpha);
        assert!(upsample(&[], 0, 5, 4).is_empty());
        assert_eq!(upsample(&[1, 2, 3], 2, 2, 2), [1, 2, 3], "the wrong length changes nothing");
    }

    /// A straight edge keeps its position: the crossing of half the
    /// stroke's alpha stays within half an output texel of the source
    /// boundary, at every scale, and the ramp is about one output texel
    /// wide.
    #[test]
    fn an_edge_stays_on_its_texel_boundary() {
        for scale in [2, 3, 4, 8] {
            let all = up(4, 1, scale, &[&[0, 0, 255, 255]]);
            assert_eq!(all.len(), 4 * scale as usize * scale as usize);
            // Every output row of a one-row source is the same; read the first.
            let out = &all[..4 * scale as usize];
            // The true edge sits between source texels 1 and 2: output
            // texel 2*scale, half a step in.
            let boundary = 2.0 * scale as f32;
            let cross = |lo: usize, hi: usize| -> f32 {
                let (a, b) = (out[lo] as f32, out[hi] as f32);
                lo as f32 + (127.5 - a) / (b - a)
            };
            let mut first = 0;
            while first + 1 < out.len() && out[first + 1] < 128 {
                first += 1;
            }
            let mid = cross(first, first + 1);
            assert!(
                (mid - boundary).abs() <= 0.5,
                "scale {scale}: edge at {mid}, boundary {boundary}"
            );
            let mut high_at = first;
            while high_at + 1 < out.len() && out[high_at] < 230 {
                high_at += 1;
            }
            let width = cross(high_at - 1, high_at) - mid;
            assert!((0.0..=1.75).contains(&width), "scale {scale}: edge width {width}");
        }
    }

    /// A stroke that never covered a whole texel keeps its weight: the
    /// upsampled maximum stays the stroke's own alpha, not half of it.
    #[test]
    fn a_thin_stroke_keeps_its_weight() {
        for scale in [2, 4, 8] {
            let out = up(3, 1, scale, &[&[0, 128, 0]]);
            let peak = *out.iter().max().unwrap();
            assert!(
                (120..=136).contains(&peak),
                "scale {scale}: peak {peak}, want about 128"
            );
        }
    }

    /// Coverage is conserved: the upsampled glyph draws the same area
    /// (within the ramp's cost) as the bitmap it came from.
    #[test]
    fn coverage_is_conserved() {
        // A ring on 8x8, the shape the glyph filter is measured with.
        let mut alpha = vec![0u8; 64];
        for y in 0..8usize {
            for x in 0..8usize {
                let d = ((x as f32 - 3.5).hypot(y as f32 - 3.5) - 2.5).abs();
                alpha[y * 8 + x] = ((3.0 - d).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
        let before: f64 = alpha.iter().map(|v| f64::from(*v)).sum();
        for scale in [2, 4] {
            let out = upsample(&alpha, 8, 8, scale);
            assert_eq!(out.len(), 64 * scale as usize * scale as usize);
            let after = out.iter().map(|v| f64::from(*v)).sum::<f64>()
                / f64::from(scale * scale);
            assert!(
                (after - before).abs() < before * 0.25,
                "scale {scale}: {before} became {after}"
            );
        }
    }

    /// Two parallel strokes a texel apart stay two strokes: the valley
    /// between them does not fill in (each stroke's own edge ramp aside).
    #[test]
    fn neighbouring_strokes_do_not_merge() {
        let out = up(5, 1, 4, &[&[255, 0, 0, 0, 255]]);
        // The strokes sit on source texels 0 and 4: the valley's output
        // texels 6 to 14 are past both ramps.
        let valley = &out[6..14];
        assert!(valley.iter().all(|v| *v <= 8), "valley {valley:?} filled in");
    }

    /// The upsampled edge runs the same on both axes: a horizontal edge
    /// and a vertical one cross at the same place (to a rounding step).
    #[test]
    fn rows_and_columns_agree() {
        let scale = 4;
        let tall = up(1, 4, scale, &[&[0, 0, 255, 255]]);
        let wide = up(4, 1, scale, &[&[0, 0, 255, 255]]);
        let side = scale as usize;
        assert_eq!((tall.len(), wide.len()), (4 * side * side, 4 * side * side));
        for y in 0..4 * side {
            let (a, b) = (tall[y * side], wide[y]);
            assert!(a.abs_diff(b) <= 1, "row {y}: {a} against {b}");
            assert!(tall[y * side..(y + 1) * side].iter().all(|v| *v == tall[y * side]), "a column's rows are equal");
        }
    }
}
