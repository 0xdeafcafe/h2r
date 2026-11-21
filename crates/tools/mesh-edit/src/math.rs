//! `[f32; 3]` vector helpers. The crate has no math dependency.

/// A point or direction.
pub type Vec3 = [f32; 3];
/// A texture coordinate.
pub type Vec2 = [f32; 2];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

pub fn dist2(a: Vec3, b: Vec3) -> f32 {
    let d = sub(a, b);
    dot(d, d)
}

/// `a` scaled to unit length, or zero if `a` is (nearly) zero.
pub fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l > 1e-20 { scale(a, 1.0 / l) } else { [0.0; 3] }
}

pub fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

pub fn lerp2(a: Vec2, b: Vec2, t: f32) -> Vec2 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// The polygon's normal by Newell's method, not normalized: its length is
/// twice the area. Works for concave and slightly non-planar polygons.
pub fn newell(points: &[Vec3]) -> Vec3 {
    let mut n = [0.0f32; 3];
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

pub fn centroid(points: &[Vec3]) -> Vec3 {
    if points.is_empty() {
        return [0.0; 3];
    }
    let mut c = [0.0f32; 3];
    for p in points {
        c = add(c, *p);
    }
    scale(c, 1.0 / points.len() as f32)
}

/// The angle between two directions, in radians; zero if either is zero.
pub fn angle(a: Vec3, b: Vec3) -> f32 {
    let (a, b) = (normalize(a), normalize(b));
    if a == [0.0; 3] || b == [0.0; 3] {
        return 0.0;
    }
    dot(a, b).clamp(-1.0, 1.0).acos()
}
