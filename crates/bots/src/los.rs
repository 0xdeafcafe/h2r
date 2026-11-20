//! Line of sight against the map's collision (docs/game/bots.md, section
//! 3.1).
//!
//! [`Walls`] holds every collision surface of the structure BSP as
//! triangles, bucketed on an XY grid. [`Walls::clear`] walks the cells a
//! segment crosses and tests their triangles. Built offline with the nav
//! mesh and kept in the same cache file.

use crate::visits::Visits;
use rustc_hash::FxHashMap;

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
    grid: FxHashMap<(i32, i32), Vec<u32>>,
}

fn cell(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

impl Walls {
    pub fn new(tris: Vec<[V3; 3]>) -> Self {
        let mut grid: FxHashMap<(i32, i32), Vec<u32>> = FxHashMap::default();
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
        // The cells the segment's XY projection crosses (Amanatides-Woo).
        let (mut cx, mut cy) = (cell(a[0]), cell(a[1]));
        let (ex, ey) = (cell(b[0]), cell(b[1]));
        // Each triangle occurs once per bucket. Chassis headroom probes
        // are vertical, so one bucket suffices with no visited bookkeeping.
        if (cx, cy) == (ex, ey) {
            return self
                .grid
                .get(&(cx, cy))
                .is_none_or(|ts| ts.iter().all(|&t| !hits(&self.tris[t as usize], a, d)));
        }
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
        Visits::query(self.tris.len(), |seen| {
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
        })
    }

    /// The first solid thing on the way from `a` to `b`: how far along
    /// (0 to 1) and the surface's unit normal, facing back toward `a`. For
    /// a grenade's flight (`crate::grenades`), whose steps are short: it
    /// tests every triangle in the cells the segment's bounding box covers.
    pub fn hit(&self, a: V3, b: V3) -> Option<(f32, V3)> {
        let d = sub(b, a);
        let mut best: Option<(f32, V3)> = None;
        Visits::query(self.tris.len(), |seen| {
            for x in cell(a[0].min(b[0]))..=cell(a[0].max(b[0])) {
                for y in cell(a[1].min(b[1]))..=cell(a[1].max(b[1])) {
                    let Some(ts) = self.grid.get(&(x, y)) else {
                        continue;
                    };
                    for &t in ts {
                        if !seen.insert(t) {
                            continue;
                        }
                        let tri = &self.tris[t as usize];
                        // Closed at both ends: a step that ends exactly on a
                        // surface, or starts on it moving in, touches it. With
                        // the open range of `clear`, a flight whose steps land
                        // on the plane (6 a second in 1/60 s steps reaches
                        // x = 3 exactly) passed through it.
                        if let Some(at) = crossing(tri, a, d)
                            .filter(|at| (-ENDS..=1.0 + ENDS).contains(at))
                            .map(|at| at.clamp(0.0, 1.0))
                            && best.is_none_or(|(b, _)| at < b)
                        {
                            let n = cross(sub(tri[1], tri[0]), sub(tri[2], tri[0]));
                            let l = dot(n, n).sqrt().max(1e-9);
                            let mut n = [n[0] / l, n[1] / l, n[2] / l];
                            if dot(n, d) > 0.0 {
                                n = [-n[0], -n[1], -n[2]];
                            }
                            best = Some((at, n));
                        }
                    }
                }
            }
            best
        })
    }
}

/// [`Walls::hit`] counts a crossing this far (of the step) past either
/// end: the rounding of a crossing exactly at an end.
const ENDS: f32 = 1e-4;

/// Whether the segment `o + t d`, `t` in `(0, 1)`, crosses triangle `t`
/// (Moller-Trumbore, both faces).
fn hits(t: &[V3; 3], o: V3, d: V3) -> bool {
    crossing(t, o, d).is_some_and(|at| at > 1e-4 && at < 1.0 - 1e-4)
}

/// Where along the line `o + t d` it crosses triangle `t`, if it does:
/// `t` unbounded; the callers keep the part of the segment they mean.
fn crossing(t: &[V3; 3], o: V3, d: V3) -> Option<f32> {
    let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, t[0]);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(dot(e2, q) * inv)
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

    #[test]
    fn single_and_multi_cell_rays_match_all_triangles() {
        let floor = [[-8., -8., 0.], [8., -8., 0.], [8., 8., 0.], [-8., 8., 0.]];
        let wall = [[-6., -6., 0.], [6., 6., 0.], [6., 6., 3.], [-6., -6., 3.]];
        let walls = Walls::from_polygons([floor.as_slice(), wall.as_slice()]);
        for (a, b) in [
            ([1., 1., -1.], [1., 1., 1.]),    // vertical
            ([0.2, 0.8, 1.], [0.8, 0.2, 1.]), // diagonal within one cell
            ([-7., 2., 1.], [7., -2., 1.]),   // wall spans several buckets
            ([-7., 2., 4.], [7., -2., 4.]),   // above the wall
            ([0., 0., 1.], [0., 0., 0.]),     // open endpoint on the floor
            ([7., -7., 1.], [7., -7., 1.]),   // point
        ] {
            for (a, b) in [(a, b), (b, a)] {
                let expected = walls.tris.iter().all(|t| !hits(t, a, sub(b, a)));
                assert_eq!(walls.clear(a, b), expected);
            }
        }
    }

    #[test]
    fn a_hit_says_where_and_which_way_the_wall_faces() {
        let w = wall();
        let (t, n) = w.hit([4.0, 0.0, 1.0], [6.0, 0.0, 1.0]).unwrap();
        assert!((t - 0.5).abs() < 1e-4, "{t}");
        assert!((n[0] + 1.0).abs() < 1e-4, "faces back: {n:?}");
        let (_, n) = w.hit([6.0, 0.0, 1.0], [4.0, 0.0, 1.0]).unwrap();
        assert!((n[0] - 1.0).abs() < 1e-4, "{n:?}");
        assert_eq!(w.hit([0.0, 0.0, 1.0], [4.0, 0.0, 1.0]), None);
        assert_eq!(w.hit([0.0, 0.0, 3.0], [10.0, 0.0, 3.0]), None);
        // A step ending exactly on the wall touches it, and so does the
        // next one starting there: a flight cannot slip through between
        // them. Line of sight keeps the open range.
        let end = w.hit([4.5, 0.0, 1.0], [5.0, 0.0, 1.0]).expect("ends on it");
        assert!((end.0 - 1.0).abs() < 1e-4, "{end:?}");
        let start = w
            .hit([5.0, 0.0, 1.0], [5.5, 0.0, 1.0])
            .expect("starts on it");
        assert!(start.0.abs() < 1e-4, "{start:?}");
        assert!(w.clear([4.5, 0.0, 1.0], [5.0, 0.0, 1.0]));
    }
}
