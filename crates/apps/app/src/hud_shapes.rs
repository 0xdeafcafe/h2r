//! A small CPU rasteriser for host overlays drawn in code: filled
//! polygons, strokes, rings and arcs with one-pixel anti-aliased edges,
//! soft glows, and the `font8x8` letters at any scale. It draws into a
//! premultiplied float canvas and hands back straight-alpha RGBA for
//! `overlay::Sprite`. The superbounce HUD ([`crate::superbounce_hud`]) is
//! its user.
//!
//! Every shape is a signed distance (negative inside) turned into
//! coverage by `clamp(0.5 - d, 0, 1)`, evaluated only inside the shape's
//! bounding box, so a shape costs its box's pixels times its edges.

/// A point in canvas pixels (x right, y down). Pixel `(i, j)` is the
/// square from `(i, j)` to `(i + 1, j + 1)`; its centre is sampled.
pub type Pt = (f32, f32);

/// Straight-alpha colour, each 0..1.
pub type Rgba = [f32; 4];

/// `c` with its alpha times `a`.
pub fn fade(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * a]
}

/// A canvas of premultiplied RGBA floats, transparent to start.
#[derive(Clone)]
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    px: Vec<[f32; 4]>,
}

/// Coverage of a pixel whose centre is `d` pixels from an edge (negative
/// inside).
fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Distance from `p` to the segment `a`-`b`.
pub fn segment_distance(p: Pt, a: Pt, b: Pt) -> f32 {
    let (ex, ey) = (b.0 - a.0, b.1 - a.1);
    let len2 = ex * ex + ey * ey;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * ex + (p.1 - a.1) * ey) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.0 - a.0 - t * ex).hypot(p.1 - a.1 - t * ey)
}

/// Signed distance from `p` to the polygon `poly` (any simple polygon,
/// even-odd inside): negative inside.
pub fn polygon_distance(p: Pt, poly: &[Pt]) -> f32 {
    let mut d = f32::INFINITY;
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        d = d.min(segment_distance(p, a, b));
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    if inside { -d } else { d }
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            px: vec![[0.0; 4]; (w as usize) * (h as usize)],
        }
    }

    /// The pixel at `(x, y)`, premultiplied.
    pub fn get(&self, x: u32, y: u32) -> [f32; 4] {
        self.px[(y * self.w + x) as usize]
    }

    /// `c` over the pixel at `(x, y)` with `cover` (0..1).
    pub fn blend(&mut self, x: u32, y: u32, c: Rgba, cover: f32) {
        let a = c[3] * cover;
        if a <= 0.0 {
            return;
        }
        let d = &mut self.px[(y * self.w + x) as usize];
        let keep = 1.0 - a;
        d[0] = c[0] * a + d[0] * keep;
        d[1] = c[1] * a + d[1] * keep;
        d[2] = c[2] * a + d[2] * keep;
        d[3] = a + d[3] * keep;
    }

    /// Runs `f(centre)` for every pixel in the box `lo`..`hi`, clipped to
    /// the canvas, and blends `c` at the coverage it returns.
    fn shade(&mut self, lo: Pt, hi: Pt, c: Rgba, f: impl Fn(Pt) -> f32) {
        let x0 = lo.0.floor().max(0.0) as u32;
        let y0 = lo.1.floor().max(0.0) as u32;
        let x1 = (hi.0.ceil().max(0.0) as u32).min(self.w);
        let y1 = (hi.1.ceil().max(0.0) as u32).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let k = f((x as f32 + 0.5, y as f32 + 0.5));
                if k > 0.0 {
                    self.blend(x, y, c, k.min(1.0));
                }
            }
        }
    }

    /// Fills `poly` with anti-aliased edges.
    pub fn fill_polygon(&mut self, poly: &[Pt], c: Rgba) {
        if poly.len() < 3 {
            return;
        }
        let (lo, hi) = bounds(poly, 1.0);
        self.shade(lo, hi, c, |p| coverage(polygon_distance(p, poly)));
    }

    /// A soft glow `radius` pixels wide around (and under) `poly`: full
    /// `c` at the edge, falling off to nothing. Draw it before the fill.
    pub fn glow_polygon(&mut self, poly: &[Pt], radius: f32, c: Rgba) {
        if poly.len() < 3 || radius <= 0.0 {
            return;
        }
        let (lo, hi) = bounds(poly, radius + 1.0);
        self.shade(lo, hi, c, |p| {
            let d = polygon_distance(p, poly).max(0.0);
            let t = (1.0 - d / radius).max(0.0);
            t * t
        });
    }

    /// A line through `points`, `width` pixels wide with round ends;
    /// `closed` joins the last point to the first.
    pub fn stroke(&mut self, points: &[Pt], width: f32, closed: bool, c: Rgba) {
        if points.is_empty() {
            return;
        }
        let half = width / 2.0;
        let (lo, hi) = bounds(points, half + 1.0);
        let mut segs: Vec<(Pt, Pt)> = points.windows(2).map(|w| (w[0], w[1])).collect();
        if segs.is_empty() {
            segs.push((points[0], points[0]));
        }
        if closed && points.len() > 2 {
            segs.push((points[points.len() - 1], points[0]));
        }
        self.shade(lo, hi, c, |p| {
            let d = segs
                .iter()
                .map(|&(a, b)| segment_distance(p, a, b))
                .fold(f32::INFINITY, f32::min);
            coverage(d - half)
        });
    }

    /// A soft glow `radius` pixels wide around (and under) the disc of
    /// radius `r`, as [`Canvas::glow_polygon`].
    pub fn glow_disc(&mut self, centre: Pt, r: f32, radius: f32, c: Rgba) {
        if radius <= 0.0 {
            return;
        }
        let reach = r + radius + 1.0;
        self.shade(
            (centre.0 - reach, centre.1 - reach),
            (centre.0 + reach, centre.1 + reach),
            c,
            |p| {
                let d = ((p.0 - centre.0).hypot(p.1 - centre.1) - r).max(0.0);
                let t = (1.0 - d / radius).max(0.0);
                t * t
            },
        );
    }

    /// A filled disc.
    pub fn disc(&mut self, centre: Pt, r: f32, c: Rgba) {
        self.ring(centre, r, 0.0, c);
    }

    /// The band between radii `inner` and `outer`.
    pub fn ring(&mut self, centre: Pt, outer: f32, inner: f32, c: Rgba) {
        let (lo, hi) = (
            (centre.0 - outer - 1.0, centre.1 - outer - 1.0),
            (centre.0 + outer + 1.0, centre.1 + outer + 1.0),
        );
        self.shade(lo, hi, c, |p| {
            let r = (p.0 - centre.0).hypot(p.1 - centre.1);
            let d = (r - outer).max(inner - r);
            coverage(if inner > 0.0 { d } else { r - outer })
        });
    }

    /// An arc of the ring between `inner` and `outer`, from angle `a0` to
    /// `a1` (radians, clockwise on screen from 12 o'clock), with square
    /// ends.
    pub fn arc(&mut self, centre: Pt, outer: f32, inner: f32, a0: f32, a1: f32, c: Rgba) {
        use std::f32::consts::TAU;
        let span = (a1 - a0).clamp(0.0, TAU);
        if span <= 0.0 {
            return;
        }
        let mid_r = (outer + inner) / 2.0;
        let (lo, hi) = (
            (centre.0 - outer - 1.0, centre.1 - outer - 1.0),
            (centre.0 + outer + 1.0, centre.1 + outer + 1.0),
        );
        self.shade(lo, hi, c, |p| {
            let (dx, dy) = (p.0 - centre.0, p.1 - centre.1);
            let r = dx.hypot(dy);
            let radial = (r - outer).max(inner - r);
            // Angle clockwise from up, relative to a0, in 0..TAU.
            let a = (dx.atan2(-dy) - a0).rem_euclid(TAU);
            let along = if a <= span {
                // Inside the span: distance to the nearer end, as an arc
                // length, negative.
                -(a.min(span - a) * mid_r)
            } else {
                ((a - span).min(TAU - a)) * mid_r
            };
            coverage(radial.max(along))
        });
    }

    /// Fills the axis-aligned box `x0..x1`, `y0..y1` (fractional edges
    /// get partial coverage, exactly).
    pub fn fill_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgba) {
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        self.shade((x0, y0), (x1, y1), c, |p| {
            let cx = (x1.min(p.0 + 0.5) - x0.max(p.0 - 0.5)).clamp(0.0, 1.0);
            let cy = (y1.min(p.1 + 0.5) - y0.max(p.1 - 0.5)).clamp(0.0, 1.0);
            cx * cy
        });
    }

    /// `text` in the `font8x8` letters with its top-left at `at`, each
    /// font pixel `px` canvas pixels (any size; edges that fall between
    /// pixels are blended).
    pub fn text(&mut self, at: Pt, px: f32, text: &str, c: Rgba) {
        for (i, ch) in text.chars().enumerate() {
            let gx0 = at.0 + i as f32 * 8.0 * px;
            for (gy, bits) in crate::watermark::glyph(ch).iter().enumerate() {
                let mut gx = 0u32;
                while gx < 8 {
                    if bits & (1 << gx) == 0 {
                        gx += 1;
                        continue;
                    }
                    // One rectangle for each run of lit pixels in the row.
                    let start = gx;
                    while gx < 8 && bits & (1 << gx) != 0 {
                        gx += 1;
                    }
                    let y = at.1 + gy as f32 * px;
                    self.fill_rect(gx0 + start as f32 * px, y, gx0 + gx as f32 * px, y + px, c);
                }
            }
        }
    }

    /// The canvas as straight-alpha RGBA bytes.
    pub fn into_rgba8(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in self.px {
            let a = p[3].clamp(0.0, 1.0);
            if a <= 0.0 {
                out.extend_from_slice(&[0; 4]);
                continue;
            }
            for v in &p[..3] {
                out.push(((v / a).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            out.push((a * 255.0).round() as u8);
        }
        out
    }
}

/// The width of `text` drawn at `px` canvas pixels a font pixel.
pub fn text_width(text: &str, px: f32) -> f32 {
    text.chars().count() as f32 * 8.0 * px
}

fn bounds(points: &[Pt], pad: f32) -> (Pt, Pt) {
    let mut lo = (f32::INFINITY, f32::INFINITY);
    let mut hi = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in points {
        lo = (lo.0.min(p.0), lo.1.min(p.1));
        hi = (hi.0.max(p.0), hi.1.max(p.1));
    }
    ((lo.0 - pad, lo.1 - pad), (hi.0 + pad, hi.1 + pad))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba = [1.0, 1.0, 1.0, 1.0];

    fn alpha(c: &Canvas, x: u32, y: u32) -> f32 {
        c.get(x, y)[3]
    }

    #[test]
    fn a_filled_triangle_covers_its_inside_and_nothing_past_its_edges() {
        let mut c = Canvas::new(16, 16);
        c.fill_polygon(&[(2.0, 2.0), (14.0, 2.0), (2.0, 14.0)], WHITE);
        // Well inside: solid. Well outside (past the hypotenuse): clear.
        assert_eq!(alpha(&c, 4, 4), 1.0);
        assert_eq!(alpha(&c, 3, 10), 1.0);
        assert_eq!(alpha(&c, 13, 13), 0.0);
        assert_eq!(alpha(&c, 0, 0), 0.0);
        // A pixel whose centre sits on the hypotenuse (x + y = 16) is about
        // half covered.
        let edge = alpha(&c, 7, 8);
        assert!((edge - 0.5).abs() < 0.05, "{edge}");
        // Straight edges along the pixel grid are exact: pixel 2's centre is
        // half a pixel in.
        assert_eq!(alpha(&c, 5, 2), 1.0);
        assert_eq!(alpha(&c, 5, 1), 0.0);
    }

    #[test]
    fn an_anti_aliased_edge_ramps_across_one_pixel() {
        let mut c = Canvas::new(8, 4);
        // A box whose right edge is at x = 4.25: pixel 3 (centre 3.5) is
        // in, pixel 4 (centre 4.5) a quarter covered, pixel 5 clear.
        c.fill_polygon(&[(0.0, 0.0), (4.25, 0.0), (4.25, 4.0), (0.0, 4.0)], WHITE);
        assert_eq!(alpha(&c, 3, 2), 1.0);
        assert!((alpha(&c, 4, 2) - 0.25).abs() < 1e-5, "{}", alpha(&c, 4, 2));
        assert_eq!(alpha(&c, 5, 2), 0.0);
        // The same with the exact box fill.
        let mut r = Canvas::new(8, 4);
        r.fill_rect(0.0, 0.0, 4.25, 4.0, WHITE);
        assert!((alpha(&r, 4, 2) - 0.25).abs() < 1e-5);
    }

    #[test]
    fn colours_blend_over_and_come_back_straight() {
        let mut c = Canvas::new(1, 1);
        c.blend(0, 0, [0.0, 0.0, 1.0, 0.5], 1.0);
        c.blend(0, 0, [1.0, 0.0, 0.0, 0.5], 1.0);
        let p = c.get(0, 0);
        assert!((p[3] - 0.75).abs() < 1e-6);
        let out = c.into_rgba8();
        // Red over blue at half each: 2/3 red, 1/3 blue, alpha 0.75.
        assert_eq!(out, vec![170, 0, 85, 191]);
    }

    #[test]
    fn rings_arcs_and_strokes_land_where_they_should() {
        let mut c = Canvas::new(32, 32);
        c.ring((16.0, 16.0), 10.0, 6.0, WHITE);
        assert_eq!(alpha(&c, 16, 16), 0.0, "the hole");
        assert_eq!(alpha(&c, 16, 8), 1.0, "the band, 8 above the centre");
        assert_eq!(alpha(&c, 16, 2), 0.0, "outside");
        // A quarter arc from 12 to 3 o'clock: lit at 1:30, dark at 7:30.
        let mut a = Canvas::new(32, 32);
        a.arc(
            (16.0, 16.0),
            10.0,
            6.0,
            0.0,
            std::f32::consts::FRAC_PI_2,
            WHITE,
        );
        assert_eq!(alpha(&a, 21, 10), 1.0);
        assert_eq!(alpha(&a, 10, 21), 0.0);
        // A stroke two pixels wide along y = 5.
        let mut s = Canvas::new(16, 10);
        s.stroke(&[(2.0, 5.0), (14.0, 5.0)], 2.0, false, WHITE);
        assert_eq!(alpha(&s, 8, 4), 1.0);
        assert_eq!(alpha(&s, 8, 5), 1.0);
        assert_eq!(alpha(&s, 8, 7), 0.0);
    }

    #[test]
    fn a_glow_fades_out_from_the_edge() {
        let mut c = Canvas::new(40, 10);
        c.glow_polygon(
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            8.0,
            WHITE,
        );
        let near = alpha(&c, 11, 5);
        let far = alpha(&c, 16, 5);
        assert!(near > far && far > 0.0, "{near} {far}");
        assert_eq!(alpha(&c, 25, 5), 0.0);
    }

    #[test]
    fn text_scales_to_any_size() {
        // 'I' in font8x8 has a lit column; at 2x the letter is 16 tall.
        let mut c = Canvas::new(16, 16);
        c.text((0.0, 0.0), 2.0, "I", WHITE);
        let lit = (0..16)
            .filter(|&y| (0..16).any(|x| alpha(&c, x, y) > 0.0))
            .count();
        assert!((10..=16).contains(&lit), "{lit} rows");
        assert_eq!(text_width("abc", 1.5), 36.0);
    }
}
