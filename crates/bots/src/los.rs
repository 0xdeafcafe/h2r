//! Line of sight against the map's collision (docs/game/bots.md, section
//! 3.1).
//!
//! [`Walls`] holds every collision surface of the structure BSP as
//! triangles, bucketed on an XY grid. [`Walls::clear`] walks the cells a
//! segment crosses and tests their triangles. Built offline with the nav
//! mesh and kept in the same cache file.

use std::collections::HashMap;

pub type V3 = [f32; 3];

/// Grid cell size, world units.
const CELL: f32 = 2.0;

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The collision as triangles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Walls {
    pub tris: Vec<[V3; 3]>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

fn cell(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

impl Walls {
    pub fn new(tris: Vec<[V3; 3]>) -> Self {
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, t) in tris.iter().enumerate() {
            let lo = |k: usize| t.iter().map(|v| v[k]).fold(f32::MAX, f32::min);
            let hi = |k: usize| t.iter().map(|v| v[k]).fold(f32::MIN, f32::max);
            for x in cell(lo(0))..=cell(hi(0)) {
                for y in cell(lo(1))..=cell(hi(1)) {
                    grid.entry((x, y)).or_default().push(i as u32);
                }
            }
        }
        Walls { tris, grid }
    }

    /// Fans each polygon into triangles.
    pub fn from_polygons<'a>(polys: impl IntoIterator<Item = &'a [V3]>) -> Self {
        let mut tris = Vec::new();
        for p in polys {
            for k in 1..p.len().saturating_sub(1) {
                tris.push([p[0], p[k], p[k + 1]]);
            }
        }
        Walls::new(tris)
    }

    /// Whether nothing solid is between `a` and `b`.
    pub fn clear(&self, a: V3, b: V3) -> bool {
        let d = sub(b, a);
        let mut seen = std::collections::HashSet::new();
        // The cells the segment's XY projection crosses (Amanatides-Woo).
        let (mut cx, mut cy) = (cell(a[0]), cell(a[1]));
        let (ex, ey) = (cell(b[0]), cell(b[1]));
        let step = |v: f32| if v > 0.0 { 1 } else { -1 };
        let (sx, sy) = (step(d[0]), step(d[1]));
        let next = |c: i32, s: i32, o: f32, dv: f32| -> f32 {
            if dv.abs() < 1e-9 {
                return f32::INFINITY;
            }
            let edge = (c + i32::from(s > 0)) as f32 * CELL;
            (edge - o) / dv
        };
        let mut tx = next(cx, sx, a[0], d[0]);
        let mut ty = next(cy, sy, a[1], d[1]);
        let dtx = if d[0].abs() < 1e-9 {
            f32::INFINITY
        } else {
            CELL / d[0].abs()
        };
        let dty = if d[1].abs() < 1e-9 {
            f32::INFINITY
        } else {
            CELL / d[1].abs()
        };
        for _ in 0..100_000 {
            if let Some(ts) = self.grid.get(&(cx, cy)) {
                for &t in ts {
                    if seen.insert(t) && hits(&self.tris[t as usize], a, d) {
                        return false;
                    }
                }
            }
            if (cx, cy) == (ex, ey) {
                return true;
            }
            if tx < ty {
                if tx > 1.0 {
                    return true;
                }
                cx += sx;
                tx += dtx;
            } else {
                if ty > 1.0 {
                    return true;
                }
                cy += sy;
                ty += dty;
            }
        }
        true
    }
}

/// Whether the segment `o + t d`, `t` in `(0, 1)`, crosses triangle `t`
/// (Moller-Trumbore, both faces).
fn hits(t: &[V3; 3], o: V3, d: V3) -> bool {
    let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-9 {
        return false;
    }
    let inv = 1.0 / det;
    let s = sub(o, t[0]);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let at = dot(e2, q) * inv;
    at > 1e-4 && at < 1.0 - 1e-4
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall in the plane x = 5, from y -1 to 1 and z 0 to 2.
    fn wall() -> Walls {
        let q = [
            [5.0, -1.0, 0.0],
            [5.0, 1.0, 0.0],
            [5.0, 1.0, 2.0],
            [5.0, -1.0, 2.0],
        ];
        Walls::from_polygons([&q[..]])
    }

    #[test]
    fn a_wall_blocks_the_line_through_it_only() {
        let w = wall();
        assert_eq!(w.tris.len(), 2);
        assert!(!w.clear([0.0, 0.0, 1.0], [10.0, 0.0, 1.0]));
        assert!(!w.clear([10.0, 0.5, 1.5], [0.0, -0.5, 0.5]), "either way");
        assert!(w.clear([0.0, 0.0, 3.0], [10.0, 0.0, 3.0]), "over it");
        assert!(w.clear([0.0, 2.0, 1.0], [10.0, 2.0, 1.0]), "beside it");
        assert!(w.clear([0.0, 0.0, 1.0], [4.9, 0.0, 1.0]), "short of it");
        assert!(w.clear([0.0, 0.0, 1.0], [0.0, 0.0, 1.0]), "a point");
    }

    #[test]
    fn long_and_negative_rays_cross_many_cells() {
        let w = wall();
        assert!(!w.clear([-40.0, -0.2, 1.0], [40.0, 0.3, 1.0]));
        assert!(!w.clear([40.0, 0.0, 1.0], [-40.0, 0.0, 1.0]));
        assert!(w.clear([-40.0, 30.0, 1.0], [40.0, 31.0, 1.0]));
        // Straight down through the wall's top edge plane, beside it.
        assert!(w.clear([6.0, 0.0, 5.0], [6.0, 0.0, -5.0]));
    }
}
