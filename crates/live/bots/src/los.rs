//! Line of sight against the map's collision (docs/game/bots.md, section
//! 3.1).
//!
//! [`Walls`] holds every collision surface of the structure BSP as
//! triangles, bucketed on an XY grid. [`Walls::clear`] walks the cells a
//! segment crosses and tests their triangles. Built offline with the nav
//! mesh and kept in the same cache file. A map's visibility grid
//! (`crate::vis`), attached once loaded, answers segments between cells
//! that never see each other without the walk.

use crate::vis::Vis;
use crate::visits::Visits;
use rustc_hash::FxHashMap;
use std::sync::{Arc, OnceLock};

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
#[derive(Clone, Debug, Default)]
pub struct Walls {
    pub tris: Vec<[V3; 3]>,
    grid: Grid,
    /// The map's visibility grid, built from these triangles
    /// (`crate::vis`): [`Walls::clear`] skips the walk for a pair of
    /// cells that never see each other. Set once, in place, so a grid
    /// built while a match runs goes into the shared map data without
    /// replacing it.
    vis: OnceLock<Arc<Vis>>,
}

/// Walls are their triangles: the bucket grid is made from them, and the
/// visibility grid only ever skips a walk whose answer it knows.
impl PartialEq for Walls {
    fn eq(&self, other: &Self) -> bool {
        self.tris == other.tris
    }
}

fn cell(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

#[derive(Clone, Debug)]
enum Grid {
    Sparse(FxHashMap<(i32, i32), Vec<u32>>),
    Dense {
        origin: (i32, i32),
        width: usize,
        height: usize,
        buckets: Vec<Vec<u32>>,
    },
}

impl Default for Grid {
    fn default() -> Self {
        Self::Sparse(FxHashMap::default())
    }
}

impl Grid {
    fn new(sparse: FxHashMap<(i32, i32), Vec<u32>>) -> Self {
        if sparse.is_empty() {
            return Self::Sparse(sparse);
        }
        let (mut lx, mut ly, mut hx, mut hy) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &(x, y) in sparse.keys() {
            lx = lx.min(x);
            ly = ly.min(y);
            hx = hx.max(x);
            hy = hy.max(y);
        }
        let width = (i64::from(hx) - i64::from(lx) + 1) as u64;
        let height = (i64::from(hy) - i64::from(ly) + 1) as u64;
        // Bound the dense metadata and keep widely separated islands sparse.
        let Some(len) = width.checked_mul(height).filter(|&n| {
            n <= 65_536 && n <= 2 * sparse.len() as u64
        }) else {
            return Self::Sparse(sparse);
        };
        let (width, height) = (width as usize, height as usize);
        let mut buckets = vec![Vec::new(); len as usize];
        for ((x, y), triangles) in sparse {
            let index = (y - ly) as usize * width + (x - lx) as usize;
            buckets[index] = triangles;
        }
        Self::Dense {
            origin: (lx, ly),
            width,
            height,
            buckets,
        }
    }

    #[inline]
    fn at(&self, x: i32, y: i32) -> &[u32] {
        match self {
            Self::Sparse(grid) => grid.get(&(x, y)).map_or(&[], Vec::as_slice),
            Self::Dense { origin, width, height, buckets } => {
                let dx = x.wrapping_sub(origin.0) as u32 as usize;
                let dy = y.wrapping_sub(origin.1) as u32 as usize;
                if dx < *width && dy < *height {
                    &buckets[dy * *width + dx]
                } else {
                    &[]
                }
            }
        }
    }
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
        Walls {
            tris,
            grid: Grid::new(grid),
            vis: OnceLock::new(),
        }
    }

    /// Attaches a visibility grid; `false` when one already is. It must
    /// have been built from these triangles (`crate::vis::load` and
    /// `crate::autonav::offer_vis` check).
    pub(crate) fn set_vis(&self, vis: Arc<Vis>) -> bool {
        self.vis.set(vis).is_ok()
    }

    /// The attached visibility grid.
    pub fn vis(&self) -> Option<&Arc<Vis>> {
        self.vis.get()
    }

    /// Fans each polygon into triangles.
    pub fn from_polygons<'a>(polys: impl IntoIterator<Item = &'a [V3]>) -> Self {
        let mut tris = Vec::new();
        for p in polys {
            Self::fan_polygon(p.iter().copied(), &mut tris);
        }
        Walls::new(tris)
    }

    pub(crate) fn fan_polygon(
        vertices: impl IntoIterator<Item = V3>,
        tris: &mut Vec<[V3; 3]>,
    ) {
        let mut vertices = vertices.into_iter();
        let (Some(first), Some(mut previous)) = (vertices.next(), vertices.next()) else {
            return;
        };
        for next in vertices {
            tris.push([first, previous, next]);
            previous = next;
        }
    }

    /// Whether nothing solid is between `a` and `b`. With a visibility
    /// grid attached, a segment between two cells that never see each
    /// other is blocked without the walk: the same answer, proved when the
    /// grid was built (`crate::vis`).
    pub fn clear(&self, a: V3, b: V3) -> bool {
        if let Some(vis) = self.vis.get()
            && vis.never(a, b)
        {
            return false;
        }
        let d = sub(b, a);
        self.walk(a, b, d, |t| !hits(&self.tris[t as usize], a, d))
    }

    /// Every triangle the open segment `a`-`b` crosses, as
    /// [`Walls::clear`] would test them (in the order it would), into
    /// `out`. Used by the scalar visibility reference.
    #[cfg(test)]
    pub(crate) fn crossed(&self, a: V3, b: V3, out: &mut Vec<u32>) {
        let d = sub(b, a);
        self.walk(a, b, d, |t| {
            if hits(&self.tris[t as usize], a, d) {
                out.push(t);
            }
            true
        });
    }

    /// Calls `f` with each triangle bucketed in the cells the segment
    /// `a`-`b` crosses, until it returns `false`; whether it never did.
    fn walk(&self, a: V3, b: V3, d: V3, mut f: impl FnMut(u32) -> bool) -> bool {
        // The cells the segment's XY projection crosses (Amanatides-Woo).
        let (mut cx, mut cy) = (cell(a[0]), cell(a[1]));
        let (ex, ey) = (cell(b[0]), cell(b[1]));
        // Each triangle occurs once per bucket. Chassis headroom probes
        // are vertical, so one bucket suffices with no visited bookkeeping.
        if (cx, cy) == (ex, ey) {
            return self.grid.at(cx, cy).iter().all(|&t| f(t));
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
                for &t in self.grid.at(cx, cy) {
                    if seen.insert(t) && !f(t) {
                        return false;
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

    /// Whether the closed segment touches collision. For a swept capsule,
    /// which needs the same endpoint handling as [`Self::hit`].
    pub(crate) fn any_hit(&self, a: V3, b: V3) -> bool {
        let d = sub(b, a);
        !self.walk_bounds(a, b, |t| contact(&self.tris[t as usize], a, d).is_none())
    }

    /// The first solid thing on the way from `a` to `b`: how far along
    /// (0 to 1) and the surface's unit normal, facing back toward `a`. For
    /// a grenade's flight (`crate::grenades`), whose steps are short.
    pub fn hit(&self, a: V3, b: V3) -> Option<(f32, V3)> {
        let d = sub(b, a);
        let mut best: Option<(f32, V3)> = None;
        self.walk_bounds(a, b, |t| {
            let tri = &self.tris[t as usize];
            if let Some(at) = contact(tri, a, d)
                && best.is_none_or(|(previous, _)| at < previous)
            {
                let n = cross(sub(tri[1], tri[0]), sub(tri[2], tri[0]));
                let l = dot(n, n).sqrt().max(1e-9);
                let mut n = [n[0] / l, n[1] / l, n[2] / l];
                if dot(n, d) > 0.0 {
                    n = [-n[0], -n[1], -n[2]];
                }
                best = Some((at, n));
            }
            true
        });
        best
    }

    /// Tests the buckets of the segment's XY bounding box, retaining their
    /// order for hits that tie. A triangle can occur only once in a bucket.
    fn walk_bounds(&self, a: V3, b: V3, mut f: impl FnMut(u32) -> bool) -> bool {
        let (x0, x1) = (cell(a[0].min(b[0])), cell(a[0].max(b[0])));
        let (y0, y1) = (cell(a[1].min(b[1])), cell(a[1].max(b[1])));
        if x0 == x1 && y0 == y1 {
            return self.grid.at(x0, y0).iter().all(|&t| f(t));
        }
        Visits::query(self.tris.len(), |seen| {
            for x in x0..=x1 {
                for y in y0..=y1 {
                    for &t in self.grid.at(x, y) {
                        if seen.insert(t) && !f(t) {
                            return false;
                        }
                    }
                }
            }
            true
        })
    }
}

/// [`Walls::hit`] counts a crossing this far (of the step) past either
/// end: the rounding of a crossing exactly at an end.
const ENDS: f32 = 1e-4;

// A flight or sweep touches a surface even if it lands exactly on the
// plane or starts there moving in. Both callers keep the same open/closed
// distinction for sight rays and contacts.
fn contact(t: &[V3; 3], o: V3, d: V3) -> Option<f32> {
    crossing(t, o, d)
        .filter(|at| (-ENDS..=1.0 + ENDS).contains(at))
        .map(|at| at.clamp(0.0, 1.0))
}

/// Whether the segment `o + t d`, `t` in `(0, 1)`, crosses triangle `t`
/// (Moller-Trumbore, both faces).
fn hits(t: &[V3; 3], o: V3, d: V3) -> bool {
    crossing(t, o, d).is_some_and(open)
}

/// Where along the line `o + t d` it crosses triangle `t`, if it does:
/// `t` unbounded; the callers keep the part of the segment they mean.
fn crossing(t: &[V3; 3], o: V3, d: V3) -> Option<f32> {
    RayTriangle::new(t).crossing(o, d)
}

struct RayTriangle {
    v: V3,
    e1: V3,
    e2: V3,
}

impl RayTriangle {
    #[inline]
    fn new(t: &[V3; 3]) -> Self {
        Self {
            v: t[0],
            e1: sub(t[1], t[0]),
            e2: sub(t[2], t[0]),
        }
    }

    #[inline]
    fn crossing(&self, o: V3, d: V3) -> Option<f32> {
        let p = cross(d, self.e2);
        let det = dot(self.e1, p);
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let s = sub(o, self.v);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = cross(s, self.e1);
        let v = dot(d, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        Some(dot(self.e2, q) * inv)
    }

    #[inline]
    fn origin(&self, o: V3) -> OriginTerms {
        let s = sub(o, self.v);
        let q = cross(s, self.e1);
        OriginTerms { s, q, numerator: dot(self.e2, q) }
    }

    #[inline]
    fn open_hit(d: V3, p: V3, inv: f32, origin: OriginTerms) -> bool {
        // For a fixed origin, check the unchanged t expression first.
        // Most triangles bucketed here are beyond an endpoint of the ray.
        if !open(origin.numerator * inv) {
            return false;
        }
        let u = dot(origin.s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return false;
        }
        let v = dot(d, origin.q) * inv;
        !(v < 0.0 || u + v > 1.0)
    }
}

#[inline]
fn open(at: f32) -> bool {
    at > ENDS && at < 1.0 - ENDS
}

#[derive(Clone, Copy, Default)]
struct OriginTerms {
    s: V3,
    q: V3,
    numerator: f32,
}

/// Origin terms shared by the rays of one visibility row. Only triangles
/// visited by at least one ray are initialised. Each worker reuses its storage.
pub(crate) struct RayOrigin {
    at: V3,
    generation: u32,
    terms: Vec<(u32, OriginTerms)>,
}

impl RayOrigin {
    pub(crate) fn new(triangles: usize) -> Self {
        Self {
            at: [0.0; 3],
            generation: 0,
            terms: vec![(0, OriginTerms::default()); triangles],
        }
    }

    pub(crate) fn reset(&mut self, at: V3) {
        self.at = at;
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.terms.fill((0, OriginTerms::default()));
            self.generation = 1;
        }
    }

    #[inline]
    fn terms(&mut self, index: usize, triangle: &RayTriangle) -> OriginTerms {
        let (generation, terms) = &mut self.terms[index];
        if *generation != self.generation {
            *generation = self.generation;
            *terms = triangle.origin(self.at);
        }
        *terms
    }
}

/// Cached triangle edges used only during visibility generation. Shares the
/// same bucket traversal with sight rays and preserves candidate order.
pub(crate) struct BuildRays<'a> {
    walls: &'a Walls,
    triangles: Vec<RayTriangle>,
}

impl<'a> BuildRays<'a> {
    pub(crate) fn new(walls: &'a Walls) -> Self {
        Self {
            walls,
            triangles: walls.tris.iter().map(RayTriangle::new).collect(),
        }
    }

    /// The legacy DDA candidate sequence. The walk and its duplicate
    /// suppression use only XY, so all end heights at one XY share it.
    pub(crate) fn candidates<'b>(&'b self, a: V3, b: V3, out: &'b mut Vec<u32>) -> &'b [u32] {
        let (x, y) = (cell(a[0]), cell(a[1]));
        if (x, y) == (cell(b[0]), cell(b[1])) {
            return self.walls.grid.at(x, y);
        }
        out.clear();
        self.walls.walk(a, b, sub(b, a), |t| {
            out.push(t);
            true
        });
        out
    }

    /// Calls `f` for the open-segment hits, until it returns false.
    pub(crate) fn crossed(&self, a: V3, b: V3, mut f: impl FnMut(u32) -> bool) -> bool {
        let d = sub(b, a);
        self.walls.walk(a, b, d, |t| {
            !self.triangles[t as usize].crossing(a, d).is_some_and(open) || f(t)
        })
    }

    pub(crate) fn crossed_from(
        &self,
        origin: &mut RayOrigin,
        b: V3,
        candidates: Option<&[u32]>,
        mut f: impl FnMut(u32) -> bool,
    ) -> bool {
        let a = origin.at;
        let d = sub(b, a);
        let test = |t: u32| {
            let tri = &self.triangles[t as usize];
            let p = cross(d, tri.e2);
            let det = dot(tri.e1, p);
            !(det.abs() >= 1e-9
                && RayTriangle::open_hit(d, p, 1.0 / det, origin.terms(t as usize, tri)))
                || f(t)
        };
        match candidates {
            Some(ids) => ids.iter().copied().all(test),
            None => self.walls.walk(a, b, d, test),
        }
    }
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
    fn fixed_origin_and_shared_xy_keep_crossed_triangle_order() {
        let floor = [[-8., -8., 0.], [8., -8., 0.], [8., 8., 0.], [-8., 8., 0.]];
        let wall = [[-6., -6., 0.], [6., 6., 0.], [6., 6., 3.], [-6., -6., 3.]];
        let walls = Walls::from_polygons([floor.as_slice(), wall.as_slice()]);
        let rays = BuildRays::new(&walls);
        let mut origin = RayOrigin::new(walls.tris.len());
        let mut candidates = Vec::new();
        let mut expected = Vec::new();
        let mut hits = Vec::new();
        for a in [[-7., -4., 2.], [6., 4., 3.], [0., 0., 0.]] {
            origin.reset(a);
            for [x, y] in [[7., -4.], [-7., 4.], [a[0], a[1]], [2., 2.]] {
                let ids = rays.candidates(a, [x, y, 0.], &mut candidates);
                for z in [-2., 0., 1., 3., 5.] {
                    let b = [x, y, z];
                    expected.clear();
                    walls.crossed(a, b, &mut expected);
                    for cached in [None, Some(ids)] {
                        hits.clear();
                        assert!(rays.crossed_from(&mut origin, b, cached, |t| {
                            hits.push(t);
                            true
                        }));
                        assert_eq!(hits, expected, "{a:?} to {b:?}");
                    }
                    assert_eq!(walls.any_hit(a, b), walls.hit(a, b).is_some());
                }
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
