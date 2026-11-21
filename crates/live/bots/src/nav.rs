//! The nav mesh (docs/game/bots.md, section 3.2 and stage 2).
//!
//! Built offline per map from the structure BSP's collision: the
//! `pathfind` generator makes one sector per walkable collision surface and
//! links the ones that share an edge ([`NavMesh::from_pathfinding`]). On top
//! of that this adds the links collision edges do not give:
//!
//! - **step**: a boundary edge whose far side is another walkable polygon
//!   at nearly the same height (a seam between surfaces that do not share
//!   an edge);
//! - **drop**: the far side is lower, within [`Build::max_drop`];
//! - **jump**: the far side is higher, within [`Build::max_jump_up`], or
//!   across a gap of up to [`Build::max_gap`].
//!
//! [`NavMesh::path`] is A\* over the polygons, then a funnel pass over the
//! portals, giving waypoints. A portal's end at a wall corner (a vertex on
//! the mesh's boundary) is pulled in by [`CLEARANCE`], so paths turn a
//! body's width off a corner instead of grazing it. A start or goal off the
//! mesh (on scenery, in the air mid-jump) snaps to the nearest polygon
//! ([`NavMesh::nearest`]). [`Follower`] walks the waypoints: the move
//! direction (bending toward the next corner as it nears one), when to
//! jump, and when it is stuck or has fallen below its path. The mesh is
//! saved in a small format of ours ([`NavMesh::to_bytes`]); it is derived
//! from the disc, so it is cached under the data root, never committed.

use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::Cell;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use tags::pathfinding::PathfindingData;

use crate::los::Walls;
use crate::vis::Threads;

mod search;
mod traversal;
mod vehicle;
use search::Search;
pub(crate) use vehicle::VehiclePaths;

pub type V3 = [f32; 3];

/// How a link is crossed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Walk = 0,
    Drop = 1,
    Jump = 2,
    CrouchJump = 3,
    Teleporter = 4,
    Lift = 5,
    Elevator = 6,
}

impl Kind {
    /// Links whose endpoints are a takeoff/trigger and a separate landing.
    pub fn traversal(self) -> bool {
        matches!(
            self,
            Self::CrouchJump | Self::Teleporter | Self::Lift | Self::Elevator
        )
    }
}

/// One polygon: a walkable surface.
#[derive(Clone, Debug, PartialEq)]
pub struct Poly {
    /// The outline, counter-clockwise seen from above.
    pub verts: Vec<V3>,
    pub center: V3,
    /// Plane: `normal . p = d`, normal up-facing.
    pub normal: V3,
    pub d: f32,
}

/// A directed link from one polygon to another, crossed at the segment
/// `a`-`b` on the `from` side. For traversal links these are the entry
/// and landing, respectively.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub from: u32,
    pub to: u32,
    pub a: V3,
    pub b: V3,
    pub kind: Kind,
}

/// Build settings, in world units (one is about 3 m).
#[derive(Clone, Copy, Debug)]
pub struct Build {
    /// Height differences up to this are walked.
    pub step: f32,
    /// The deepest drop taken.
    pub max_drop: f32,
    /// The highest ledge jumped onto.
    pub max_jump_up: f32,
    /// The widest gap jumped across.
    pub max_gap: f32,
    /// How far past an edge the far side is probed for a step or a drop.
    pub probe: f32,
}

impl Default for Build {
    fn default() -> Self {
        Build {
            step: 0.12,
            max_drop: 1.6,
            max_jump_up: 0.35,
            max_gap: 0.8,
            probe: 0.25,
        }
    }
}

/// Extra cost of a link of each kind, in world units.
fn penalty(k: Kind) -> f32 {
    match k {
        Kind::Walk => 0.0,
        Kind::Drop => 0.5,
        Kind::Jump => 2.0,
        Kind::CrouchJump => 2.5,
        Kind::Teleporter | Kind::Lift | Kind::Elevator => 0.0,
    }
}

// masterchief_mp capsule heights: docs/game/superbounce.md.
pub(crate) const STANDING: f32 = 0.725;
const CROUCHED: f32 = 0.5;
const CROUCH_GAIN: f32 = STANDING - CROUCHED;

pub(crate) fn standing_clear(walls: &Walls, floor: V3) -> bool {
    walls.clear(
        add(floor, [0.0, 0.0, 0.025]),
        add(floor, [0.0, 0.0, STANDING]),
    )
}

const JUMP_STEPS: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
enum JumpSweep {
    Clear,
    PostureBlocked,
    BothBlocked,
}

struct JumpArc {
    centers: [V3; JUMP_STEPS + 1],
    shoulder: V3,
}

impl JumpArc {
    fn new(a: V3, b: V3) -> Self {
        let side = sub(b, a);
        let line = [side[1], -side[0], 0.0];
        let shoulder = scale(line, 0.16 / len(line).max(1e-6));
        let centers = std::array::from_fn(|i| {
            let t = i as f32 / JUMP_STEPS as f32;
            let p = add(a, scale(side, t));
            add(p, [0.0, 0.0, 1.44 * t * (1.0 - t)])
        });
        Self { centers, shoulder }
    }

    fn sweep(&self, walls: &Walls, crouch: bool, common_clear: &mut usize) -> JumpSweep {
        // Sweep the same samples for the centre and both shoulders.
        // Crouching changes only the headroom and crown rays after step 1.
        let height = |step| {
            if crouch && step >= 2 { CROUCHED } else { STANDING }
        };
        let shoulders = [scale(self.shoulder, -1.0), [0.0; 3], self.shoulder];
        for (lateral, side) in shoulders.into_iter().enumerate() {
            let mut lo = add(side, self.centers[0]);
            for i in 1..=JUMP_STEPS {
                let hi = add(side, self.centers[i]);
                let low = [0.0, 0.0, 0.025];
                let crown = [0.0, 0.0, height(i)];
                let blocked = if i == 1 {
                    JumpSweep::BothBlocked
                } else {
                    JumpSweep::PostureBlocked
                };
                if !walls.clear(add(hi, low), add(hi, crown)) {
                    return blocked;
                }
                let sample = lateral * JUMP_STEPS + i;
                if sample > *common_clear {
                    // Adjacent samples share their endpoints, so use closed
                    // rays there too, just as for the crown sweep below.
                    for offset in [low, [0.0, 0.0, 0.28]] {
                        if walls.any_hit(add(lo, offset), add(hi, offset)) {
                            return JumpSweep::BothBlocked;
                        }
                    }
                    *common_clear = sample;
                }
                if walls.any_hit(add(lo, [0.0, 0.0, height(i - 1)]), add(hi, crown)) {
                    return blocked;
                }
                lo = hi;
            }
        }
        JumpSweep::Clear
    }

    fn kind(&self, walls: &Walls, try_standing: bool) -> Option<Kind> {
        let mut common_clear = 0;
        if try_standing {
            match self.sweep(walls, false, &mut common_clear) {
                JumpSweep::Clear => return Some(Kind::Jump),
                JumpSweep::BothBlocked => return None,
                JumpSweep::PostureBlocked => {}
            }
        }
        (self.sweep(walls, true, &mut common_clear) == JumpSweep::Clear).then_some(Kind::CrouchJump)
    }
}

#[cfg(test)]
fn jump_clear(walls: &Walls, a: V3, b: V3, crouch: bool) -> bool {
    let mut common_clear = 0;
    JumpArc::new(a, b).sweep(walls, crouch, &mut common_clear) == JumpSweep::Clear
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
fn dist(a: V3, b: V3) -> f32 {
    len(sub(a, b))
}
fn dist_xy(a: V3, b: V3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}
fn mid(a: V3, b: V3) -> V3 {
    scale(add(a, b), 0.5)
}
/// Twice the signed XY area of the triangle `a b c` (positive: c is left of a->b).
fn cross_xy(a: V3, b: V3, c: V3) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

impl Poly {
    pub fn new(mut verts: Vec<V3>) -> Self {
        let mut n = [0.0f32; 3];
        for i in 0..verts.len() {
            let (u, v) = (verts[i], verts[(i + 1) % verts.len()]);
            n[0] += (u[1] - v[1]) * (u[2] + v[2]);
            n[1] += (u[2] - v[2]) * (u[0] + v[0]);
            n[2] += (u[0] - v[0]) * (u[1] + v[1]);
        }
        if n[2] < 0.0 {
            verts.reverse();
            n = scale(n, -1.0);
        }
        let l = len(n).max(1e-12);
        let normal = scale(n, 1.0 / l);
        let center = scale(
            verts.iter().fold([0.0; 3], |s, v| add(s, *v)),
            1.0 / verts.len().max(1) as f32,
        );
        Poly {
            d: dot(normal, center),
            verts,
            center,
            normal,
        }
    }

    /// Whether `p` is inside the outline seen from above.
    pub fn contains_xy(&self, p: V3) -> bool {
        let n = self.verts.len();
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (self.verts[i], self.verts[j]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let x = (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0];
                if p[0] < x {
                    inside = !inside;
                }
            }
            j = i;
        }
        inside
    }

    /// The surface's height under `p`.
    pub fn z_at(&self, p: V3) -> f32 {
        let n = self.normal;
        if n[2].abs() < 1e-6 {
            return self.center[2];
        }
        (self.d - n[0] * p[0] - n[1] * p[1]) / n[2]
    }

    /// Nearest point on the surface, nudged away from a boundary edge.
    pub(crate) fn interior_near(&self, target: V3, inset: f32) -> V3 {
        let mut p = target;
        if !self.contains_xy(target) {
            p = self
                .verts
                .iter()
                .zip(self.verts.iter().cycle().skip(1))
                .map(|(&a, &b)| closest_xy(a, b, target))
                .min_by(|a, b| dist_xy(*a, target).total_cmp(&dist_xy(*b, target)))
                .unwrap_or(self.center);
        }
        let d = dist_xy(self.center, p);
        p = add(
            p,
            scale(sub(self.center, p), (inset / d.max(1e-6)).min(1.0)),
        );
        p[2] = self.z_at(p);
        p
    }
}

const CELL: f32 = 4.0;

/// Paths keep this far off a wall corner (world units; a biped is about
/// 0.2 across its waist, so this leaves a margin).
pub const CLEARANCE: f32 = 0.3;
/// A point off the mesh snaps to a polygon within this (XY)...
pub const SNAP: f32 = 3.0;
/// ...whose floor is at most this far over the point...
pub const SNAP_UP: f32 = 0.6;
/// ...or this far under it (someone seen mid-jump, a bot on a crate).
pub const SNAP_DOWN: f32 = 3.0;

/// A vertex to the millimetre, for matching shared corners.
type Key = (i32, i32, i32);
fn key(v: V3) -> Key {
    let q = |f: f32| (f * 1000.0).round() as i32;
    (q(v[0]), q(v[1]), q(v[2]))
}
fn pair(a: Key, b: Key) -> (Key, Key) {
    if a <= b { (a, b) } else { (b, a) }
}

/// The nav mesh of one structure BSP.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavMesh {
    pub polys: Vec<Poly>,
    pub edges: Vec<Edge>,
    /// Each polygon's outgoing edges (indices into `edges`).
    out: Vec<Vec<u32>>,
    grid: FxHashMap<(i32, i32), Vec<u32>>,
    /// The vertices on the mesh's boundary: an end of a polygon side that
    /// is not walked across (a wall, a ledge). Portal ends here keep
    /// [`CLEARANCE`].
    corners: FxHashSet<Key>,
    shortcuts: bool,
}

fn cell(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

/// Computes short ranges on the build's workers and merges results in
/// input order. Chunk scheduling keeps a dense patch of expensive collision
/// probes from holding up the rest of the build on one thread.
pub(crate) fn ordered_build_chunks<T: Send>(
    len: usize,
    chunk_len: usize,
    threads: Threads,
    build: impl Fn(Range<usize>) -> T + Sync,
) -> Vec<T> {
    let count = len.div_ceil(chunk_len);
    let workers = count.min(threads.count.max(1));
    let range = |i: usize| i * chunk_len..((i + 1) * chunk_len).min(len);
    if workers <= 1 {
        return (0..count).map(|i| build(range(i))).collect();
    }
    let next = AtomicUsize::new(0);
    let work = || {
        if threads.background {
            background::lower_current();
        }
        let mut results = Vec::new();
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= count {
                break;
            }
            results.push((i, build(range(i))));
        }
        results
    };
    let mut results: Vec<_> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..workers).map(|_| scope.spawn(work)).collect();
        jobs.into_iter()
            .flat_map(|job| job.join().expect("nav build worker panicked"))
            .collect()
    });
    results.sort_unstable_by_key(|&(i, _)| i);
    results.into_iter().map(|(_, result)| result).collect()
}

fn is_shortcut(kind: Kind) -> bool {
    matches!(kind, Kind::Teleporter | Kind::Lift | Kind::Elevator)
}

impl NavMesh {
    /// The mesh over `polys` with `edges`; builds the lookups.
    pub fn new(polys: Vec<Poly>, edges: Vec<Edge>) -> Self {
        let mut mesh = Self::with_polys(polys);
        mesh.set_edges(edges);
        mesh
    }

    fn with_polys(polys: Vec<Poly>) -> Self {
        let mut grid: FxHashMap<(i32, i32), Vec<u32>> = FxHashMap::default();
        for (i, p) in polys.iter().enumerate() {
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for v in &p.verts {
                for k in 0..2 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            for x in cell(lo[0])..=cell(hi[0]) {
                for y in cell(lo[1])..=cell(hi[1]) {
                    grid.entry((x, y)).or_default().push(i as u32);
                }
            }
        }
        Self {
            polys,
            grid,
            ..Self::default()
        }
    }

    fn set_edges(&mut self, edges: Vec<Edge>) {
        let mut out = vec![Vec::new(); self.polys.len()];
        for (i, e) in edges.iter().enumerate() {
            if let Some(o) = out.get_mut(e.from as usize) {
                o.push(i as u32);
            }
        }
        let linked: FxHashSet<(Key, Key)> = edges
            .iter()
            .filter(|e| e.kind == Kind::Walk)
            .map(|e| pair(key(e.a), key(e.b)))
            .collect();
        let mut corners = FxHashSet::default();
        for p in &self.polys {
            let n = p.verts.len();
            for i in 0..n {
                let (u, v) = (key(p.verts[i]), key(p.verts[(i + 1) % n]));
                if !linked.contains(&pair(u, v)) {
                    corners.insert(u);
                    corners.insert(v);
                }
            }
        }
        self.shortcuts = edges.iter().any(|e| is_shortcut(e.kind));
        self.edges = edges;
        self.out = out;
        self.corners = corners;
    }

    /// Adds scenario shortcuts without rebuilding the polygon lookup or
    /// boundary corners, which depend on the walk links.
    pub(crate) fn append_shortcuts(&mut self, edges: Vec<Edge>) {
        for edge in edges {
            debug_assert!(is_shortcut(edge.kind));
            if let Some(out) = self.out.get_mut(edge.from as usize) {
                out.push(self.edges.len() as u32);
            }
            self.edges.push(edge);
            self.shortcuts = true;
        }
    }

    /// Whether `v` is a vertex on the mesh's boundary (a wall corner).
    pub fn is_corner(&self, v: V3) -> bool {
        self.corners.contains(&key(v))
    }

    /// Where a path may cross edge `e`: its segment, cut to the part the
    /// polygon beyond lies along (a step or seam link spans the whole side
    /// of the near polygon, which can be much wider than the far one: a
    /// doorway off a long wall), with each end at a wall corner, or cut,
    /// pulled in by [`CLEARANCE`]. A portal narrower than that shrinks to
    /// a point between its ends (the middle when both are corners).
    pub fn portal(&self, e: &Edge) -> (V3, V3) {
        if e.kind.traversal() {
            return (e.a, e.a);
        }
        let (a, b, cut_a, cut_b) = self.clip(e);
        let ab = sub(b, a);
        let l = (ab[0] * ab[0] + ab[1] * ab[1]).sqrt();
        if l < 1e-6 {
            return (a, b);
        }
        let ia = if cut_a || self.is_corner(a) {
            CLEARANCE
        } else {
            0.0
        };
        let ib = if cut_b || self.is_corner(b) {
            CLEARANCE
        } else {
            0.0
        };
        if ia + ib >= l {
            let m = add(a, scale(ab, ia / (ia + ib)));
            return (m, m);
        }
        (add(a, scale(ab, ia / l)), sub(b, scale(ab, ib / l)))
    }

    /// `e`'s segment cut to the span of the far polygon's vertices that lie
    /// on its line (within 0.3, XY), and whether each end was cut. Uncut
    /// when the far polygon has no side along the edge (a drop or a jump
    /// to a floor that is not beside it).
    fn clip(&self, e: &Edge) -> (V3, V3, bool, bool) {
        let (a, b) = (e.a, e.b);
        let ab = sub(b, a);
        let l2 = ab[0] * ab[0] + ab[1] * ab[1];
        let Some(far) = self.polys.get(e.to as usize) else {
            return (a, b, false, false);
        };
        if l2 < 1e-12 {
            return (a, b, false, false);
        }
        let l = l2.sqrt();
        let n = [-ab[1] / l, ab[0] / l];
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for v in &far.verts {
            let av = sub(*v, a);
            if (av[0] * n[0] + av[1] * n[1]).abs() > 0.3 {
                continue;
            }
            let t = (av[0] * ab[0] + av[1] * ab[1]) / l2;
            lo = lo.min(t);
            hi = hi.max(t);
        }
        let (t0, t1) = (lo.max(0.0), hi.min(1.0));
        if t0 >= t1 {
            return (a, b, false, false);
        }
        let eps = 1e-4;
        (
            add(a, scale(ab, t0)),
            add(a, scale(ab, t1)),
            t0 > eps,
            t1 < 1.0 - eps,
        )
    }

    /// The polygon nearest `p` (XY) within `reach` whose floor there is at
    /// most `up` over `p` and `down` under it, and the point on it to walk
    /// to: `p` itself over the polygon, else its nearest point pulled
    /// [`CLEARANCE`] inside.
    pub fn nearest(&self, p: V3, reach: f32, up: f32, down: f32) -> Option<(u32, V3)> {
        crate::visits::Visits::query(self.polys.len(), |seen| {
            let mut best: Option<(f32, u32, V3)> = None;
            for x in cell(p[0] - reach)..=cell(p[0] + reach) {
                for y in cell(p[1] - reach)..=cell(p[1] + reach) {
                    for &i in self.grid.get(&(x, y)).into_iter().flatten() {
                        if !seen.insert(i) {
                            continue;
                        }
                        let poly = &self.polys[i as usize];
                        let q = if poly.contains_xy(p) {
                            p
                        } else {
                            let n = poly.verts.len();
                            let edge = (0..n)
                                .map(|k| closest_xy(poly.verts[k], poly.verts[(k + 1) % n], p))
                                .min_by(|a, b| dist_xy(*a, p).total_cmp(&dist_xy(*b, p)));
                            let Some(e) = edge else { continue };
                            let inward = sub(poly.center, e);
                            let il = (inward[0] * inward[0] + inward[1] * inward[1]).sqrt();
                            if il < 1e-6 {
                                e
                            } else {
                                add(e, scale(inward, CLEARANCE.min(il) / il))
                            }
                        };
                        let dxy = dist_xy(q, p);
                        if dxy > reach {
                            continue;
                        }
                        let z = poly.z_at(q);
                        let dz = z - p[2];
                        if dz > up || -dz > down {
                            continue;
                        }
                        let score = dxy + 0.5 * dz.abs();
                        if best.is_none_or(|(s, _, _)| score < s) {
                            best = Some((score, i, [q[0], q[1], z]));
                        }
                    }
                }
            }
            best.map(|(_, i, q)| (i, q))
        })
    }

    /// The polygon a biped at `p` is on, or (off the mesh) the nearest one
    /// within [`SNAP`].
    pub fn poly_near(&self, p: V3) -> Option<u32> {
        self.locate(p, 1.0)
            .or_else(|| self.nearest(p, SNAP, SNAP_UP, SNAP_DOWN).map(|(i, _)| i))
    }

    /// The walkable polygons from `pathfind`'s data (one per walkable
    /// collision surface), with walk links where two share an edge, and
    /// the step, drop and jump links of `b`.
    pub fn from_pathfinding(pf: &PathfindingData, b: &Build) -> Self {
        Self::from_collision(
            pf,
            b,
            &Walls::default(),
            Threads { count: 1, background: false },
        )
    }

    pub(crate) fn from_collision(
        pf: &PathfindingData,
        b: &Build,
        walls: &Walls,
        threads: Threads,
    ) -> Self {
        const CHUNK: usize = 128;
        let (polys, standing): (Vec<_>, Vec<_>) =
            ordered_build_chunks(pf.sectors.len(), CHUNK, threads, |range| {
                range
                    .map(|k| {
                        let s = &pf.sectors[k];
                        let mut verts = Vec::new();
                        let mut l = s.first_link as usize;
                        for _ in 0..pf.links.len() {
                            let Some(link) = pf.links.get(l) else { break };
                            let left = usize::from(link.left_sector) == k;
                            let v = if left { link.vertex1 } else { link.vertex2 };
                            verts.push(pf.vertices.get(usize::from(v)).copied().unwrap_or_default());
                            l = usize::from(if left { link.forward_link } else { link.reverse_link });
                            if l == s.first_link as usize {
                                break;
                            }
                        }
                        let poly = Poly::new(verts);
                        let clear = standing_clear(walls, poly.center);
                        (poly, clear)
                    })
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .unzip();
        let mut mesh = Self::with_polys(polys);
        let none = u16::MAX;
        let chunks = ordered_build_chunks(pf.links.len(), CHUNK, threads, |range| {
            let mut shared = Vec::new();
            let mut boundary = Vec::new();
            for link in &pf.links[range] {
                let (a, end) = (
                    pf.vertices[usize::from(link.vertex1)],
                    pf.vertices[usize::from(link.vertex2)],
                );
                let from = match (link.left_sector, link.right_sector) {
                    (l, r) if l != none && r != none => {
                        if standing[usize::from(l)]
                            && standing[usize::from(r)]
                            && standing_clear(walls, mid(a, end))
                        {
                            for (from, to) in [(l, r), (r, l)] {
                                shared.push(Edge {
                                    from: from.into(),
                                    to: to.into(),
                                    a,
                                    b: end,
                                    kind: Kind::Walk,
                                });
                            }
                        }
                        continue;
                    }
                    (l, _) if l != none => u32::from(l),
                    (_, r) if r != none => u32::from(r),
                    _ => continue,
                };
                if standing[from as usize]
                    && let Some(edge) = mesh.cross_link(from, a, end, b, walls)
                    && standing[edge.to as usize]
                {
                    boundary.push(edge);
                }
            }
            (shared, boundary)
        });
        // Shared links precede boundary links in the file. The order within
        // each group is the order of the original collision links.
        let mut edges: Vec<_> = chunks.iter().flat_map(|(walk, _)| walk).copied().collect();
        edges.extend(chunks.into_iter().flat_map(|(_, boundary)| boundary));
        mesh.set_edges(edges);
        mesh
    }

    /// What lies past the boundary edge `a`-`e` of polygon `from`.
    fn cross_link(&self, from: u32, a: V3, e: V3, b: &Build, walls: &Walls) -> Option<Edge> {
        let poly = &self.polys[from as usize];
        let m = mid(a, e);
        // Outward: perpendicular to the edge, away from the polygon.
        let along = sub(e, a);
        let mut out = [along[1], -along[0], 0.0];
        let l = len(out);
        if l < 1e-6 {
            return None;
        }
        out = scale(out, 1.0 / l);
        if dot(out, sub(m, poly.center)) < 0.0 {
            out = scale(out, -1.0);
        }
        let edge = |to: u32, kind| Edge {
            from,
            to,
            a,
            b: e,
            kind,
        };
        let takeoff = add(m, scale(out, -0.28));
        for reach in [b.probe, b.max_gap] {
            let p = add(m, scale(out, reach));
            let Some((to, z)) = self.below(p, m[2] + b.max_jump_up + CROUCH_GAIN, from) else {
                continue;
            };
            let dz = z - m[2];
            let landing = [p[0], p[1], z];
            if reach == b.probe && dz.abs() <= b.step {
                // At a step seam the higher tread is part of the floor.
                let seam = [m[0], m[1], m[2].max(z)];
                if standing_clear(walls, seam) && standing_clear(walls, landing) {
                    return Some(edge(to, Kind::Walk));
                }
                continue;
            }
            if reach == b.probe && dz < -b.step && -dz <= b.max_drop {
                if standing_clear(walls, m) && standing_clear(walls, landing) {
                    return Some(edge(to, Kind::Drop));
                }
                continue;
            }
            if -dz > b.max_drop {
                continue;
            }
            match JumpArc::new(takeoff, landing).kind(walls, dz <= b.max_jump_up) {
                Some(Kind::CrouchJump) => {
                    return Some(Edge {
                        a: takeoff,
                        b: landing,
                        ..edge(to, Kind::CrouchJump)
                    });
                }
                Some(kind) => return Some(edge(to, kind)),
                None => {}
            }
        }
        None
    }

    /// The highest polygon under `p` whose surface there is at most `top`
    /// high, other than `skip`, with that height.
    fn below(&self, p: V3, top: f32, skip: u32) -> Option<(u32, f32)> {
        self.grid
            .get(&(cell(p[0]), cell(p[1])))?
            .iter()
            .filter(|&&i| i != skip && self.polys[i as usize].contains_xy(p))
            .map(|&i| (i, self.polys[i as usize].z_at(p)))
            .filter(|&(_, z)| z <= top + 1e-4)
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The polygon a biped standing at `p` (its origin, up to `above` over
    /// the floor) is on.
    pub fn locate(&self, p: V3, above: f32) -> Option<u32> {
        self.below(p, p[2] + 0.1, u32::MAX)
            .filter(|&(_, z)| p[2] - z <= above)
            .map(|(i, _)| i)
    }

    /// The outgoing edges of polygon `i`.
    pub fn out(&self, i: u32) -> impl Iterator<Item = &Edge> {
        self.out
            .get(i as usize)
            .into_iter()
            .flatten()
            .map(|&e| &self.edges[e as usize])
    }

    /// A\*: the polygons and edges from `start`'s polygon to `goal`'s.
    pub fn corridor(&self, start: u32, goal: u32) -> Option<Vec<u32>> {
        self.corridor_avoiding(start, goal, &|_| false)
    }

    /// [`NavMesh::corridor`] never taking an edge `skip` rules out (one a
    /// bot got stuck on, docs/game/bots.md section 3.2).
    pub fn corridor_avoiding(
        &self,
        start: u32,
        goal: u32,
        skip: &dyn Fn(&Edge) -> bool,
    ) -> Option<Vec<u32>> {
        // A skip predicate can itself ask for a corridor. Taking the workspace
        // leaves an empty one available to such a nested search.
        thread_local! { static SCRATCH: Cell<Search> = Cell::default(); }
        SCRATCH.with(|slot| {
            let mut search = slot.take();
            let path = search.corridor(self, start, goal, |_, e| skip(e));
            slot.set(search);
            path
        })
    }

    fn vehicle_portal(&self, e: &Edge, half_width: f32, height: f32, walls: &Walls) -> Option<V3> {
        let (a, b, cut_a, cut_b) = self.clip(e);
        let width = dist_xy(a, b);
        let clearance_a = if cut_a || self.is_corner(a) {
            half_width
        } else {
            0.0
        };
        let clearance_b = if cut_b || self.is_corner(b) {
            half_width
        } else {
            0.0
        };
        if width <= clearance_a + clearance_b {
            return None;
        }
        let span = sub(b, a);
        let at = add(
            a,
            scale(span, (clearance_a + width - clearance_b) / (2.0 * width)),
        );
        let cross = scale(span, half_width / width);
        for q in [sub(at, cross), at, add(at, cross)] {
            if !walls.clear(add(q, [0.0, 0.0, 0.15]), add(q, [0.0, 0.0, height])) {
                return None;
            }
        }
        Some(at)
    }

    /// Waypoints from `from` to `to`: the funnel over the corridor's
    /// portals. Each waypoint says how it is reached.
    pub fn path(&self, from: V3, to: V3) -> Option<Vec<Waypoint>> {
        self.path_avoiding(from, to, &|_| false)
    }

    /// [`NavMesh::path`] over a corridor that never takes an edge `skip`
    /// rules out. A start off the mesh walks to the nearest polygon first
    /// (jumping when its floor is higher); a goal off it ends at the
    /// nearest point on the mesh ([`NavMesh::nearest`], within [`SNAP`]).
    pub fn path_avoiding(
        &self,
        from: V3,
        to: V3,
        skip: &dyn Fn(&Edge) -> bool,
    ) -> Option<Vec<Waypoint>> {
        let (start, lead) = match self.locate(from, 1.0) {
            Some(i) => (i, None),
            None => {
                let (i, q) = self.nearest(from, SNAP, SNAP_UP, SNAP_DOWN)?;
                (i, Some(q))
            }
        };
        let (goal, to) = match self.locate(to, 1.0) {
            Some(i) => (i, to),
            None => self.nearest(to, SNAP, SNAP_UP, SNAP_DOWN)?,
        };
        let corridor = self.corridor_avoiding(start, goal, skip)?;
        let Some(q) = lead else {
            return Some(funnel(self, from, to, &corridor));
        };
        let kind = if q[2] - from[2] > Build::default().step {
            Kind::Jump
        } else {
            Kind::Walk
        };
        let mut path = vec![Waypoint { at: q, kind }];
        path.extend(funnel(self, q, to, &corridor));
        path.dedup_by(|b, a| {
            dist_xy(a.at, b.at) < 1e-4 && b.kind == Kind::Walk && !a.kind.traversal()
        });
        Some(path)
    }
}

/// A point to walk to, and how the step that reaches it is taken.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Waypoint {
    pub at: V3,
    pub kind: Kind,
}

/// The simple stupid funnel (Mononen's) over the portals, in XY. A drop or
/// a jump is a corner of its own: the path goes to its edge's nearest
/// point, then on.
fn funnel(mesh: &NavMesh, from: V3, to: V3, corridor: &[u32]) -> Vec<Waypoint> {
    let mut out = Vec::new();
    let mut start = from;
    let mut pending = Kind::Walk;
    let mut run: Vec<(V3, V3)> = Vec::new();
    for &ei in corridor {
        let e = &mesh.edges[ei as usize];
        // Off the wall corners by a body's width.
        let (ea, eb) = mesh.portal(e);
        if e.kind == Kind::Walk {
            let c = mesh.polys[e.from as usize].center;
            // Left and right as seen from inside the polygon, looking out.
            let (l, r) = if cross_xy(c, e.a, e.b) > 0.0 {
                (eb, ea)
            } else {
                (ea, eb)
            };
            run.push((l, r));
            continue;
        }
        // Funnel up to the special edge, aimed at its nearest point.
        let at = closest_on(ea, eb, start);
        let mut approach = pull(start, at, &run);
        if e.kind.traversal() && (pending == Kind::Walk || approach.len() > 1) {
            approach.pop();
        }
        for (k, c) in approach.into_iter().enumerate() {
            out.push(Waypoint {
                at: c,
                kind: if k == 0 { pending } else { Kind::Walk },
            });
        }
        if e.kind.traversal() {
            // The entry is followed by its exact landing. The follower
            // consumes both before looking at another walking waypoint.
            out.push(Waypoint { at, kind: e.kind });
            out.push(Waypoint {
                at: e.b,
                kind: Kind::Walk,
            });
            pending = Kind::Walk;
            start = e.b;
        } else {
            pending = e.kind;
            start = at;
        }
        run.clear();
    }
    let corners = pull(start, to, &run);
    for (k, c) in corners.into_iter().enumerate() {
        out.push(Waypoint {
            at: c,
            kind: if k == 0 { pending } else { Kind::Walk },
        });
    }
    out.dedup_by(|b, a| dist_xy(a.at, b.at) < 1e-4 && b.kind == Kind::Walk && !a.kind.traversal());
    out
}

/// The corners of the shortest XY path from `from` to `to` through the
/// portals `(left, right)`, ending with `to`.
fn pull(from: V3, to: V3, portals: &[(V3, V3)]) -> Vec<V3> {
    // Mononen's triarea2 is the negative of `cross_xy`.
    let tri = |a, b, c| -cross_xy(a, b, c);
    let mut ps: Vec<(V3, V3)> = Vec::with_capacity(portals.len() + 2);
    ps.push((from, from));
    ps.extend_from_slice(portals);
    ps.push((to, to));
    let mut out = Vec::new();
    let (mut apex, mut left, mut right) = (from, from, from);
    let (mut left_i, mut right_i) = (0usize, 0usize);
    let mut i = 1;
    while i < ps.len() {
        let (pl, pr) = ps[i];
        if tri(apex, right, pr) <= 0.0 {
            if dist_xy(apex, right) < 1e-6 || tri(apex, left, pr) > 0.0 {
                right = pr;
                right_i = i;
            } else {
                out.push(left);
                apex = left;
                let apex_i = left_i;
                (left, right) = (apex, apex);
                (left_i, right_i) = (apex_i, apex_i);
                i = apex_i + 1;
                continue;
            }
        }
        if tri(apex, left, pl) >= 0.0 {
            if dist_xy(apex, left) < 1e-6 || tri(apex, right, pl) < 0.0 {
                left = pl;
                left_i = i;
            } else {
                out.push(right);
                apex = right;
                let apex_i = right_i;
                (left, right) = (apex, apex);
                (left_i, right_i) = (apex_i, apex_i);
                i = apex_i + 1;
                continue;
            }
        }
        i += 1;
    }
    out.push(to);
    out
}

/// The point of segment `a`-`b` nearest `p`, in XY (Z interpolated), ends
/// included.
fn closest_xy(a: V3, b: V3, p: V3) -> V3 {
    let ab = sub(b, a);
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    if l2 < 1e-12 {
        return a;
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0);
    add(a, scale(ab, t))
}

/// The point of segment `a`-`b` nearest `p`, in XY (Z interpolated).
fn closest_on(a: V3, b: V3, p: V3) -> V3 {
    let ab = sub(b, a);
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    if l2 < 1e-12 {
        return a;
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.05, 0.95);
    add(a, scale(ab, t))
}

// ---- following a path ----

/// What the follower wants this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Steer {
    /// The direction to move, in world XY, unit length or zero.
    pub dir: [f32; 2],
    pub jump: bool,
    pub crouch: bool,
    /// The entry and landing are under control of the traversal.
    pub traversing: bool,
    /// The path is done or lost; plan again.
    pub replan: bool,
}

/// Walks a list of waypoints.
#[derive(Clone, Debug, Default)]
pub struct Follower {
    pub path: Vec<Waypoint>,
    next: usize,
    /// The best distance to the next waypoint so far, and the frame it was
    /// reached: no progress for [`STUCK_FRAMES`] is stuck.
    best: f32,
    best_frame: u64,
    jumped: bool,
    transit: Option<traversal::Transit>,
    /// The goal it was asked for, when the path ends elsewhere (a goal off
    /// the mesh ends at the nearest point on it).
    target: Option<V3>,
}

/// Within this (XY) a waypoint counts as reached...
pub const REACHED: f32 = 0.3;
/// ...unless its floor is this much over the bot: then it has fallen
/// below its path (off a ledge, knocked off a bridge) and plans again.
/// The allowance grows with the XY distance (a ramp climbs).
pub const FELL: f32 = 1.2;
/// Within this of a corner it starts bending toward the next waypoint.
pub const LOOKAHEAD: f32 = 0.8;
/// Frames without getting closer before the follower jumps, then gives up.
pub const STUCK_FRAMES: u64 = 30;

pub(crate) fn approach_pace(distance: f32) -> f32 {
    0.44 + 0.50 * (distance / 1.25).min(1.0)
}

impl Follower {
    pub fn new(path: Vec<Waypoint>, frame: u64) -> Self {
        Follower {
            path,
            next: 0,
            best: f32::INFINITY,
            best_frame: frame,
            jumped: false,
            transit: None,
            target: None,
        }
    }

    /// The same, remembering `goal` as the goal asked for.
    pub fn toward(mut self, goal: V3) -> Self {
        self.target = Some(goal);
        self
    }

    pub fn done(&self) -> bool {
        self.next >= self.path.len()
    }

    /// The goal asked for, else the path's end, if any.
    pub fn goal(&self) -> Option<V3> {
        self.target.or_else(|| self.path.last().map(|w| w.at))
    }

    /// The waypoint it is walking to, if any.
    pub fn waypoint(&self) -> Option<V3> {
        self.path.get(self.next).map(|w| w.at)
    }

    /// A turn to look into before the feet commit to it.
    pub(crate) fn corner(&self, at: V3) -> Option<V3> {
        let (w, next) = (self.path.get(self.next)?, self.path.get(self.next + 1)?);
        if w.kind != Kind::Walk || next.kind != Kind::Walk || dist_xy(w.at, at) > 1.8 {
            return None;
        }
        let a = [w.at[0] - at[0], w.at[1] - at[1]];
        let b = [next.at[0] - w.at[0], next.at[1] - w.at[1]];
        let dot = a[0] * b[0] + a[1] * b[1];
        (dot < 0.91 * a[0].hypot(a[1]) * b[0].hypot(b[1])).then_some(next.at)
    }

    pub(crate) fn committed(&self) -> bool {
        self.transit.is_some_and(|t| t.armed())
    }

    pub(crate) fn crouch_jumping(&self) -> bool {
        self.transit.is_some()
            && self
                .path
                .get(self.next)
                .is_some_and(|w| w.kind == Kind::CrouchJump)
    }

    pub(crate) fn pace(&self, at: V3) -> f32 {
        let Some(w) = self.path.get(self.next) else {
            return 0.0;
        };
        let d = dist_xy(w.at, at);
        if w.kind != Kind::Walk
            || self
                .path
                .get(self.next + 1)
                .is_some_and(|n| n.kind != Kind::Walk)
        {
            1.0
        } else if self.next + 1 == self.path.len() {
            approach_pace(d)
        } else if d < 1.2 && self.corner(at).is_some() {
            0.70 + 0.24 * (d / 1.2)
        } else {
            0.94
        }
    }

    pub fn steer(&mut self, at: V3, frame: u64) -> Steer {
        self.steer_with_clearance(at, frame, |_| true)
    }

    pub(crate) fn steer_with_clearance(
        &mut self,
        at: V3,
        frame: u64,
        clear: impl FnOnce([f32; 2]) -> bool,
    ) -> Steer {
        while let Some(w) = self.path.get(self.next).copied() {
            if w.kind.traversal() {
                let Some(exit) = self.path.get(self.next + 1).map(|w| w.at) else {
                    return Steer {
                        replan: true,
                        ..Steer::default()
                    };
                };
                if let Some(steer) = traversal::follow(w, exit, at, frame, &mut self.transit) {
                    return steer;
                }
                self.transit = None;
                self.next += 2;
            } else {
                let d = dist_xy(w.at, at);
                if w.at[2] - at[2] > FELL + d {
                    // Below it with no way up from here.
                    return Steer {
                        replan: true,
                        ..Steer::default()
                    };
                }
                if d > REACHED {
                    break;
                }
                self.next += 1;
            }
            self.best = f32::INFINITY;
            self.best_frame = frame;
            self.jumped = false;
        }
        let Some(w) = self.path.get(self.next).copied() else {
            return Steer {
                replan: true,
                ..Steer::default()
            };
        };
        let d = dist_xy(w.at, at);
        if d < self.best - 0.02 {
            self.best = d;
            self.best_frame = frame;
        }
        let mut s = Steer::default();
        let stalled = frame.saturating_sub(self.best_frame);
        if stalled >= STUCK_FRAMES {
            if self.jumped {
                return Steer {
                    replan: true,
                    ..Steer::default()
                };
            }
            self.jumped = true;
            self.best_frame = frame;
            s.jump = true;
        }
        if w.kind == Kind::Jump && d < 1.0 {
            s.jump = true;
        }
        let mut dir = [
            (w.at[0] - at[0]) / d.max(1e-6),
            (w.at[1] - at[1]) / d.max(1e-6),
        ];
        // Nearing a corner of a walk with more walk after it: bend toward
        // the next leg, so the turn is a curve, not a stop and a pivot.
        // Never into a jump or a drop: those start at their edge.
        if let Some(n) = self.path.get(self.next + 1)
            && w.kind == Kind::Walk
            && n.kind == Kind::Walk
            && d < LOOKAHEAD
        {
            let dn = dist_xy(n.at, at).max(1e-6);
            let to_n = [(n.at[0] - at[0]) / dn, (n.at[1] - at[1]) / dn];
            let t = 0.5 * (1.0 - d / LOOKAHEAD);
            let b = [
                dir[0] * (1.0 - t) + to_n[0] * t,
                dir[1] * (1.0 - t) + to_n[1] * t,
            ];
            let bl = (b[0] * b[0] + b[1] * b[1]).sqrt();
            if bl > 1e-3 {
                let bend = [b[0] / bl, b[1] / bl];
                if clear(bend) {
                    dir = bend;
                }
            }
        }
        s.dir = dir;
        s
    }
}

// ---- the file ----

/// The cache file's first bytes and version.
pub const MAGIC: &[u8; 4] = b"H2NV";
pub const VERSION: u32 = 1;

impl NavMesh {
    /// Our format: magic, version, then the polygons (vertex count and
    /// vertices) and the edges, all little-endian.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(MAGIC);
        o.extend_from_slice(&VERSION.to_le_bytes());
        o.extend_from_slice(&(self.polys.len() as u32).to_le_bytes());
        let v3 =
            |o: &mut Vec<u8>, v: V3| v.iter().for_each(|f| o.extend_from_slice(&f.to_le_bytes()));
        for p in &self.polys {
            o.extend_from_slice(&(p.verts.len() as u32).to_le_bytes());
            p.verts.iter().for_each(|v| v3(&mut o, *v));
        }
        o.extend_from_slice(&(self.edges.len() as u32).to_le_bytes());
        for e in &self.edges {
            o.extend_from_slice(&e.from.to_le_bytes());
            o.extend_from_slice(&e.to.to_le_bytes());
            v3(&mut o, e.a);
            v3(&mut o, e.b);
            o.push(e.kind as u8);
        }
        o
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let mut r = Reader { b, at: 0 };
        if r.take(4)? != MAGIC || r.u32()? != VERSION {
            return None;
        }
        let n = r.u32()? as usize;
        let mut polys = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            let k = r.u32()? as usize;
            let mut verts = Vec::with_capacity(k.min(1 << 10));
            for _ in 0..k {
                verts.push(r.v3()?);
            }
            if verts.len() < 3 {
                return None;
            }
            polys.push(Poly::new(verts));
        }
        let m = r.u32()? as usize;
        let mut edges = Vec::with_capacity(m.min(1 << 20));
        for _ in 0..m {
            let (from, to) = (r.u32()?, r.u32()?);
            let (a, b) = (r.v3()?, r.v3()?);
            let kind = match r.take(1)?[0] {
                0 => Kind::Walk,
                1 => Kind::Drop,
                2 => Kind::Jump,
                3 => Kind::CrouchJump,
                4 => Kind::Teleporter,
                5 => Kind::Lift,
                6 => Kind::Elevator,
                _ => return None,
            };
            if from as usize >= n || to as usize >= n {
                return None;
            }
            edges.push(Edge {
                from,
                to,
                a,
                b,
                kind,
            });
        }
        (r.at == b.len()).then(|| NavMesh::new(polys, edges))
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn v3(&mut self) -> Option<V3> {
        let mut v = [0.0; 3];
        for f in &mut v {
            *f = f32::from_bits(self.u32()?);
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests;
