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

pub(crate) fn centroid2(poly: &[V2]) -> V2 {
    let n = poly.len().max(1) as f32;
    let s = poly
        .iter()
        .fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
    [s[0] / n, s[1] / n]
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

/// Distance from `p` to a counter-clockwise convex polygon, 0 inside.
pub(crate) fn dist2_to_poly(poly: &[V2], p: V2) -> f32 {
    if inside2(poly, p, 0.0) {
        return 0.0;
    }
    let n = poly.len();
    (0..n)
        .map(|i| seg_dist2(poly[i], poly[(i + 1) % n], p))
        .fold(f32::INFINITY, f32::min)
}

pub(crate) fn seg_dist2(a: V2, b: V2, p: V2) -> f32 {
    let ab = sub2(b, a);
    let l2 = dot2(ab, ab);
    let t = if l2 > 0.0 {
        (dot2(sub2(p, a), ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    len2(sub2(p, [a[0] + ab[0] * t, a[1] + ab[1] * t]))
}

/// A polygon vertex and the tag of the edge that starts at it.
pub(crate) type Tagged<T> = Vec<(V2, T)>;

/// Sutherland–Hodgman: keep the part of `poly` where `f(p) >= 0` (`f`
/// linear). Edges along the cut get `tag`; kept edges keep theirs.
pub(crate) fn clip<T: Copy>(poly: &Tagged<T>, f: impl Fn(V2) -> f32, tag: T) -> Tagged<T> {
    let n = poly.len();
    let mut out = Vec::with_capacity(n + 2);
    for k in 0..n {
        let (p, edge) = poly[k];
        let q = poly[(k + 1) % n].0;
        let (fp, fq) = (f(p), f(q));
        let cut = || {
            let t = fp / (fp - fq);
            [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]
        };
        match (fp >= 0.0, fq >= 0.0) {
            (true, true) => out.push((p, edge)),
            (true, false) => {
                out.push((p, edge));
                out.push((cut(), tag));
            }
            (false, true) => out.push((cut(), edge)),
            (false, false) => {}
        }
    }
    out
}

/// The half-plane left of the directed line `a -> b` (inside of a
/// counter-clockwise polygon's edge), moved `inset` further in.
pub(crate) fn left_of(a: V2, b: V2, inset: f32) -> impl Fn(V2) -> f32 {
    let e = sub2(b, a);
    let l = len2(e).max(1e-9);
    move |p| cross2(e, sub2(p, a)) / l - inset
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
