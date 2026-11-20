//! Primitives the compiler indexes, and an exact polygon/box overlap test
//! for checking query results against brute force.

use crate::vm::Aabb;

/// A convex planar polygon and the key a query should emit for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub key: u32,
    pub vertices: Vec<[f32; 3]>,
}

/// Key type of a structure collision surface: `key >> 29 == 1`, surface
/// index in the low 16 bits (E `0x001ef3e0`).
pub const STRUCTURE_KEY_TYPE: u32 = 1;

/// Key for structure collision surface `index`.
pub fn structure_key(index: usize) -> u32 {
    (STRUCTURE_KEY_TYPE << 29) | index as u32
}

impl Surface {
    pub fn bounds(&self) -> Aabb {
        let mut b = Aabb {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        };
        for v in &self.vertices {
            for (a, &x) in v.iter().enumerate() {
                b.min[a] = b.min[a].min(x);
                b.max[a] = b.max[a].max(x);
            }
        }
        b
    }
}

/// Every surface of a structure collision BSP, keyed as the engine keys
/// structure surfaces.
pub fn structure_surfaces(bsp: &tags::collision::Bsp) -> Vec<Surface> {
    (0..bsp.surfaces.len())
        .map(|i| Surface {
            key: structure_key(i),
            vertices: bsp
                .surface_ring(i)
                .into_iter()
                .map(|v| bsp.vertices[v].point)
                .collect(),
        })
        .collect()
}

type V = [f64; 3];
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Separating-axis test of one triangle against a box (box axes, triangle
/// normal, and the nine edge cross products).
fn triangle_overlaps(t: [V; 3], c: V, h: V) -> bool {
    let p = [sub(t[0], c), sub(t[1], c), sub(t[2], c)];
    let separated = |axis: V| {
        if dot(axis, axis) < 1e-24 {
            return false;
        }
        let d = [dot(p[0], axis), dot(p[1], axis), dot(p[2], axis)];
        let r = h[0] * axis[0].abs() + h[1] * axis[1].abs() + h[2] * axis[2].abs();
        d.iter().cloned().fold(f64::INFINITY, f64::min) > r
            || d.iter().cloned().fold(f64::NEG_INFINITY, f64::max) < -r
    };
    let units = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    if units.iter().any(|&u| separated(u)) {
        return false;
    }
    let e = [sub(p[1], p[0]), sub(p[2], p[1]), sub(p[0], p[2])];
    if separated(cross(e[0], e[1])) {
        return false;
    }
    for u in units {
        for ei in e {
            if separated(cross(u, ei)) {
                return false;
            }
        }
    }
    true
}

/// Does the polygon (fan-triangulated) touch the closed box?
pub fn polygon_overlaps(vertices: &[[f32; 3]], b: &Aabb) -> bool {
    let c = [0, 1, 2].map(|a| (f64::from(b.min[a]) + f64::from(b.max[a])) * 0.5);
    let h = [0, 1, 2].map(|a| (f64::from(b.max[a]) - f64::from(b.min[a])) * 0.5);
    let v: Vec<V> = vertices.iter().map(|p| p.map(f64::from)).collect();
    match v.len() {
        0 => false,
        1 => (0..3).all(|a| (v[0][a] - c[a]).abs() <= h[a]),
        2 => triangle_overlaps([v[0], v[1], v[1]], c, h),
        _ => (1..v.len() - 1).any(|i| triangle_overlaps([v[0], v[i], v[i + 1]], c, h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_box_overlap() {
        let b = Aabb {
            min: [0.0; 3],
            max: [1.0; 3],
        };
        // Crosses the box.
        assert!(polygon_overlaps(
            &[[-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], [0.5, 2.0, 0.5]],
            &b
        ));
        // Bounds overlap but the plane misses the corner region.
        assert!(!polygon_overlaps(
            &[[1.2, 0.0, 0.0], [0.0, 1.2, 0.0], [0.0, 0.0, 1.2]].map(|p| p.map(|x| x + 0.9)),
            &b
        ));
        // Diagonal triangle whose box overlaps but which passes beside it.
        assert!(!polygon_overlaps(
            &[[2.0, -0.5, 0.5], [-0.5, 2.0, 0.5], [2.0, 2.0, 0.5]].map(|p| [
                p[0] + 0.6,
                p[1] + 0.6,
                p[2]
            ]),
            &b
        ));
        // Square polygon inside.
        assert!(polygon_overlaps(
            &[
                [0.2, 0.2, 0.5],
                [0.8, 0.2, 0.5],
                [0.8, 0.8, 0.5],
                [0.2, 0.8, 0.5]
            ],
            &b
        ));
    }
}
