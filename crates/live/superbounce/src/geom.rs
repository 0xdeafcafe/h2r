//! Small vector, plane and convex-polygon helpers for the scan.
use std::collections::HashMap;

use crate::{Poly, Vec3};

pub(crate) type V2 = [f32; 2];

pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn len(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
pub(crate) fn unit(a: Vec3) -> Option<Vec3> {
    let l = len(a);
    (l > 1e-6 && l.is_finite()).then(|| [a[0] / l, a[1] / l, a[2] / l])
}
pub(crate) fn dot2(a: V2, b: V2) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
pub(crate) fn sub2(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}
pub(crate) fn cross2(a: V2, b: V2) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}
pub(crate) fn len2(a: V2) -> f32 {
    dot2(a, a).sqrt()
}

/// A polygon's bounds and centroid, computed once.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
    pub centroid: Vec3,
}

impl Bounds {
    pub fn of(verts: &[Vec3]) -> Self {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut sum = [0.0f32; 3];
        for v in verts {
            for k in 0..3 {
                min[k] = min[k].min(v[k]);
                max[k] = max[k].max(v[k]);
                sum[k] += v[k];
            }
        }
        let n = verts.len().max(1) as f32;
        Self {
            min,
            max,
            centroid: sum.map(|s| s / n),
        }
    }
}

/// The height of `poly`'s plane over `(x, y)`, or `None` for a vertical
/// plane.
pub(crate) fn plane_z(poly: &Poly, xy: V2) -> Option<f32> {
    let n = poly.normal;
    let p = *poly.verts.first()?;
    (n[2].abs() > 1e-4).then(|| p[2] - (n[0] * (xy[0] - p[0]) + n[1] * (xy[1] - p[1])) / n[2])
}

/// The polygon projected onto the ground plane, counter-clockwise.
pub(crate) fn xy_ccw(verts: &[Vec3]) -> Vec<V2> {
    let mut out: Vec<V2> = verts.iter().map(|v| [v[0], v[1]]).collect();
    if area2(&out) < 0.0 {
        out.reverse();
    }
    out
}

/// Signed area (counter-clockwise positive).
pub(crate) fn area2(poly: &[V2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| cross2(poly[i], poly[(i + 1) % n]))
        .sum::<f32>()
        * 0.5
}

/// Inside a counter-clockwise convex polygon (edges included, `eps` slack).
pub(crate) fn inside2(poly: &[V2], p: V2, eps: f32) -> bool {
    let n = poly.len();
    n >= 3
        && (0..n).all(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let e = sub2(b, a);
            let l = len2(e);
            l < 1e-9 || cross2(e, sub2(p, a)) / l >= -eps
        })
}

/// A uniform grid of item indices over the ground plane.
pub(crate) struct Grid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

/// An item spanning more cells than this a side is not bucketed (no map
/// polygon is that big at [`Grid`]'s cell size; it guards hostile data).
const MAX_SPAN: i32 = 1024;

impl Grid {
    pub fn new(cell: f32) -> Self {
        Self {
            cell,
            cells: HashMap::new(),
        }
    }

    fn range(&self, min: V2, max: V2) -> Option<[i32; 4]> {
        let c = |v: f32| (v / self.cell).floor();
        let [x0, y0, x1, y1] = [c(min[0]), c(min[1]), c(max[0]), c(max[1])];
        if ![x0, y0, x1, y1]
            .iter()
            .all(|v| v.is_finite() && v.abs() < 1e6)
        {
            return None;
        }
        let r = [x0 as i32, y0 as i32, x1 as i32, y1 as i32];
        (r[2] - r[0] <= MAX_SPAN && r[3] - r[1] <= MAX_SPAN).then_some(r)
    }

    pub fn insert(&mut self, item: u32, min: V2, max: V2) {
        let Some([x0, y0, x1, y1]) = self.range(min, max) else {
            return;
        };
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.cells.entry((x, y)).or_default().push(item);
            }
        }
    }

    /// Every item whose cells meet the box, each once, ascending.
    pub fn query(&self, min: V2, max: V2, out: &mut Vec<u32>) {
        out.clear();
        let Some([x0, y0, x1, y1]) = self.range(min, max) else {
            return;
        };
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(items) = self.cells.get(&(x, y)) {
                    out.extend_from_slice(items);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

pub(crate) fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(crate) fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// The unit normal of a polygon's own winding (Newell's method), so the
/// inside test below doesn't depend on which way the ring runs.
pub(crate) fn winding_normal(verts: &[Vec3]) -> Option<Vec3> {
    let mut n = [0.0f32; 3];
    for (i, a) in verts.iter().enumerate() {
        let b = verts[(i + 1) % verts.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    unit(n)
}

/// `p` (on the polygon's plane) inside a convex polygon wound
/// counter-clockwise about `n`.
fn inside3(verts: &[Vec3], n: Vec3, p: Vec3) -> bool {
    (0..verts.len()).all(|i| {
        let (a, b) = (verts[i], verts[(i + 1) % verts.len()]);
        dot(cross(sub(b, a), sub(p, a)), n) >= -1e-6
    })
}

/// The closest points of segments `p1 q1` and `p2 q2` (Ericson, Real-Time
/// Collision Detection, 5.1.9).
pub(crate) fn closest_seg_seg(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    const EPS: f32 = 1e-12;
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let (s, t) = if a <= EPS && e <= EPS {
        (0.0, 0.0)
    } else if a <= EPS {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = dot(d1, r);
        if e <= EPS {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let s = if denom > EPS {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let t = (b * s + f) / e;
            if t < 0.0 {
                ((-c / a).clamp(0.0, 1.0), 0.0)
            } else if t > 1.0 {
                (((b - c) / a).clamp(0.0, 1.0), 1.0)
            } else {
                (s, t)
            }
        }
    };
    (add(p1, scale(d1, s)), add(p2, scale(d2, t)))
}

/// The distance from segment `p0 p1` to a convex polygon, and the
/// polygon's point nearest it.
pub(crate) fn seg_poly(p0: Vec3, p1: Vec3, verts: &[Vec3]) -> (f32, Vec3) {
    let mut best = (f32::INFINITY, verts[0]);
    if let Some(n) = winding_normal(verts) {
        let o = verts[0];
        let (d0, d1) = (dot(n, sub(p0, o)), dot(n, sub(p1, o)));
        if d0 * d1 <= 0.0 && d0 != d1 {
            let x = add(p0, scale(sub(p1, p0), d0 / (d0 - d1)));
            if inside3(verts, n, x) {
                return (0.0, x);
            }
        }
        for (p, d) in [(p0, d0), (p1, d1)] {
            let q = sub(p, scale(n, d));
            if d.abs() < best.0 && inside3(verts, n, q) {
                best = (d.abs(), q);
            }
        }
    }
    for i in 0..verts.len() {
        let (a, b) = (verts[i], verts[(i + 1) % verts.len()]);
        let (c1, c2) = closest_seg_seg(p0, p1, a, b);
        let d = len(sub(c1, c2));
        if d < best.0 {
            best = (d, c2);
        }
    }
    best
}

/// The distance from point `p` to the infinite line through `a` and `b`.
pub(crate) fn point_line(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let d = sub(b, a);
    let l = len(d);
    if l < 1e-9 {
        return len(sub(p, a));
    }
    len(cross(d, sub(p, a))) / l
}

/// The horizontal distance from `p` to the segment `edge`.
pub(crate) fn segment_xy([a, b]: [Vec3; 2], p: V2) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a[0] + dx * t - p[0]).hypot(a[1] + dy * t - p[1])
}
