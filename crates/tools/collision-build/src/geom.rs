//! `f64` vector helpers and convex polygon clipping.

pub(crate) type V3 = [f64; 3];

/// Distance within which a point is on a plane (world units).
pub(crate) const PLANE_EPS: f64 = 1e-4;
/// Smallest fragment area kept, and smallest overlap that makes a leaf
/// border a surface (square world units).
pub(crate) const AREA_EPS: f64 = 1e-8;

pub(crate) fn wide(p: [f32; 3]) -> V3 {
    p.map(f64::from)
}

pub(crate) fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// Newell's normal: twice the polygon's vector area, pointing along the
/// right-hand winding.
pub(crate) fn newell(points: &[V3]) -> V3 {
    let mut n = [0.0; 3];
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

pub(crate) fn area(points: &[V3]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    len(newell(points)) / 2.0
}

pub(crate) fn centroid(points: &[V3]) -> V3 {
    let k = points.len().max(1) as f64;
    std::array::from_fn(|a| points.iter().map(|p| p[a]).sum::<f64>() / k)
}

/// The parts of a convex polygon on the front (`n . p - d >= -eps`) and
/// the back (`<= eps`) of a plane. Points on the plane go to both.
pub(crate) fn split(poly: &[V3], n: V3, d: f64) -> (Vec<V3>, Vec<V3>) {
    let (mut front, mut back) = (Vec::new(), Vec::new());
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (da, db) = (dot(n, a) - d, dot(n, b) - d);
        if da >= -PLANE_EPS {
            front.push(a);
        }
        if da <= PLANE_EPS {
            back.push(a);
        }
        if (da > PLANE_EPS && db < -PLANE_EPS) || (da < -PLANE_EPS && db > PLANE_EPS) {
            let t = da / (da - db);
            let p = std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
            front.push(p);
            back.push(p);
        }
    }
    let tidy = |mut v: Vec<V3>| {
        v.dedup_by(|a, b| len(sub(*a, *b)) <= 1e-9);
        if v.len() > 1 && len(sub(v[0], v[v.len() - 1])) <= 1e-9 {
            v.pop();
        }
        if v.len() < 3 { Vec::new() } else { v }
    };
    (tidy(front), tidy(back))
}

/// The part of a convex polygon on one side of a plane.
pub(crate) fn clip(poly: &[V3], n: V3, d: f64, front: bool) -> Vec<V3> {
    let (f, b) = split(poly, n, d);
    if front { f } else { b }
}

/// Signed distances of `points` to `n . p - d`: (least, greatest).
pub(crate) fn extent(points: &[V3], n: V3, d: f64) -> (f64, f64) {
    points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
        let v = dot(n, *p) - d;
        (lo.min(v), hi.max(v))
    })
}
