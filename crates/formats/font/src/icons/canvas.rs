//! A small anti-aliased rasteriser for icon glyphs.
//!
//! Shapes are signed distance functions in pixel units (negative inside).
//! A pixel's coverage is `0.5 - distance` at its centre, clamped to 0..1,
//! which gives a one-pixel anti-aliased edge: enough for 16 to 24 pixel
//! icons, and deterministic. Colours are straight (non-premultiplied) RGBA.
use crate::encode::quantize_alpha;

pub type Rgba = [f32; 4];
pub const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

/// White at the given opacity: the paint of the monochrome icons.
pub fn white(alpha: f32) -> Rgba {
    [1.0, 1.0, 1.0, alpha]
}

#[derive(Debug, Clone, PartialEq)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pixels: Vec<Rgba>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![[0.0; 4]; width * height],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Rgba {
        self.pixels[y * self.width + x]
    }

    fn blend(&mut self, x: usize, y: usize, src: Rgba) {
        let sa = src[3].clamp(0.0, 1.0);
        if sa <= 0.0 {
            return;
        }
        let d = &mut self.pixels[y * self.width + x];
        let da = d[3] * (1.0 - sa);
        let a = sa + da;
        for k in 0..3 {
            d[k] = (src[k] * sa + d[k] * da) / a;
        }
        d[3] = a;
    }

    /// Draw `shape` filled with `paint` (which gets the pixel centre), over
    /// what is there.
    pub fn fill(&mut self, shape: impl Fn(f32, f32) -> f32, paint: impl Fn(f32, f32) -> Rgba) {
        for y in 0..self.height {
            for x in 0..self.width {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let cover = (0.5 - shape(px, py)).clamp(0.0, 1.0);
                if cover > 0.0 {
                    let mut c = paint(px, py);
                    c[3] *= cover;
                    self.blend(x, y, c);
                }
            }
        }
    }

    /// Fill with one colour.
    pub fn fill_flat(&mut self, shape: impl Fn(f32, f32) -> f32, color: Rgba) {
        self.fill(shape, |_, _| color);
    }

    /// Set one whole pixel (for bitmap lettering). Out of range is ignored.
    pub fn plot(&mut self, x: i32, y: i32, color: Rgba) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.blend(x as usize, y as usize, color);
        }
    }

    /// ARGB4444 with each pixel's own colour: 16 alpha levels, 4 bits a channel.
    pub fn to_argb4444(&self) -> Vec<u16> {
        self.pixels
            .iter()
            .map(|p| {
                let q = |v: f32| (v.clamp(0.0, 1.0) * 15.0).round() as u16;
                let a = q(p[3]);
                if a == 0 {
                    0x0fff
                } else {
                    a << 12 | q(p[0]) << 8 | q(p[1]) << 4 | q(p[2])
                }
            })
            .collect()
    }

    /// ARGB4444 for a monochrome icon: white, with alpha snapped to the eight
    /// levels the font's run bytes carry (the retail monochrome icons use
    /// exactly those), so the glyph compresses without literal words.
    pub fn to_white_argb4444(&self) -> Vec<u16> {
        self.pixels
            .iter()
            .map(|p| quantize_alpha((p[3].clamp(0.0, 1.0) * 15.0).round() as u16) << 12 | 0xfff)
            .collect()
    }
}

/// Circle of radius `r` about (`cx`, `cy`).
pub fn circle(cx: f32, cy: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| (x - cx).hypot(y - cy) - r
}

/// Axis-aligned box from (`x0`, `y0`) to (`x1`, `y1`) with corner radius `r`.
pub fn round_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hw, hh) = ((x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r);
    move |x, y| {
        let (qx, qy) = ((x - cx).abs() - hw, (y - cy).abs() - hh);
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
    }
}

/// A line from `a` to `b`, `thickness` wide, with round ends.
pub fn segment(a: (f32, f32), b: (f32, f32), thickness: f32) -> impl Fn(f32, f32) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    move |x, y| {
        let t = (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0);
        (x - a.0 - t * dx).hypot(y - a.1 - t * dy) - thickness / 2.0
    }
}

/// A convex polygon, corners in either winding.
pub fn polygon<const N: usize>(points: [(f32, f32); N]) -> impl Fn(f32, f32) -> f32 {
    let area: f32 = (0..N)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % N]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    let sign = if area < 0.0 { -1.0 } else { 1.0 };
    move |x, y| {
        let mut d = f32::MIN;
        for i in 0..N {
            let (a, b) = (points[i], points[(i + 1) % N]);
            let (ex, ey) = (b.0 - a.0, b.1 - a.1);
            let len = ex.hypot(ey).max(1e-6);
            // Distance to the edge's line, positive on the outside.
            d = d.max(sign * ((x - a.0) * ey - (y - a.1) * ex) / len);
        }
        d
    }
}

/// The outline of `shape`, `thickness` wide, centred on its edge.
pub fn outline(shape: impl Fn(f32, f32) -> f32, thickness: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| shape(x, y).abs() - thickness / 2.0
}

/// The outline of `shape`, `thickness` wide, lying just inside its edge.
pub fn inner_outline(shape: impl Fn(f32, f32) -> f32, thickness: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| (shape(x, y) + thickness / 2.0).abs() - thickness / 2.0
}

/// Points in both shapes.
pub fn both(a: impl Fn(f32, f32) -> f32, b: impl Fn(f32, f32) -> f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| a(x, y).max(b(x, y))
}
