//! Small planar helpers shared by the generator and the comparison.

pub type V3 = [f32; 3];
pub type V2 = [f32; 2];

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Signed area of a polygon projected onto the XY plane (positive when
/// counter-clockwise seen from +Z).
pub fn signed_area_xy(p: &[V3]) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (u, v) = (p[i], p[(i + 1) % p.len()]);
        a += u[0] * v[1] - v[0] * u[1];
    }
    a * 0.5
}

/// Area normal (not normalised; length = 2 × area) of a 3D polygon (Newell).
pub fn newell(p: &[V3]) -> V3 {
    let mut n = [0.0f32; 3];
    for i in 0..p.len() {
        let (u, v) = (p[i], p[(i + 1) % p.len()]);
        n[0] += (u[1] - v[1]) * (u[2] + v[2]);
        n[1] += (u[2] - v[2]) * (u[0] + v[0]);
        n[2] += (u[0] - v[0]) * (u[1] + v[1]);
    }
    n
}

/// Whether (x, y) is inside the XY projection of a polygon (even-odd rule;
/// points on an edge may go either way).
pub fn contains_xy(p: &[V3], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = p.len().wrapping_sub(1);
    for i in 0..p.len() {
        let (a, b) = (p[i], p[j]);
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Whether a polygon's XY projection is convex with consistent turning
/// (collinear vertices allowed). `ccw`: the expected turning direction.
pub fn is_convex_xy(p: &[V3], ccw: bool) -> bool {
    let n = p.len();
    if n < 3 {
        return false;
    }
    let scale = p
        .iter()
        .flat_map(|v| [v[0].abs(), v[1].abs()])
        .fold(1.0f32, f32::max);
    for i in 0..n {
        let (a, b, c) = (p[i], p[(i + 1) % n], p[(i + 2) % n]);
        let z = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        let tol = 1e-6 * scale * scale;
        if (ccw && z < -tol) || (!ccw && z > tol) {
            return false;
        }
    }
    true
}

/// Mean of the vertices.
pub fn centroid(p: &[V3]) -> V3 {
    let mut c = [0.0; 3];
    for v in p {
        for k in 0..3 {
            c[k] += v[k];
        }
    }
    let n = p.len().max(1) as f32;
    [c[0] / n, c[1] / n, c[2] / n]
}
