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
//! portals, giving waypoints. [`Follower`] walks them: the move direction,
//! when to jump, and when it is stuck. The mesh is saved in a small format
//! of ours ([`NavMesh::to_bytes`]); it is derived from the disc, so it is
//! cached under the data root, never committed.

use std::collections::{BinaryHeap, HashMap};

use tags::pathfinding::PathfindingData;

pub type V3 = [f32; 3];

/// How a link is crossed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Walk = 0,
    Drop = 1,
    Jump = 2,
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
/// `a`-`b` on the `from` side.
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
    }
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
}

const CELL: f32 = 4.0;

/// The nav mesh of one structure BSP.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavMesh {
    pub polys: Vec<Poly>,
    pub edges: Vec<Edge>,
    /// Each polygon's outgoing edges (indices into `edges`).
    out: Vec<Vec<u32>>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

fn cell(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

impl NavMesh {
    /// The mesh over `polys` with `edges`; builds the lookups.
    pub fn new(polys: Vec<Poly>, edges: Vec<Edge>) -> Self {
        let mut out = vec![Vec::new(); polys.len()];
        for (i, e) in edges.iter().enumerate() {
            if let Some(o) = out.get_mut(e.from as usize) {
                o.push(i as u32);
            }
        }
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
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
        NavMesh {
            polys,
            edges,
            out,
            grid,
        }
    }

    /// The walkable polygons from `pathfind`'s data (one per walkable
    /// collision surface), with walk links where two share an edge, and
    /// the step, drop and jump links of `b`.
    pub fn from_pathfinding(pf: &PathfindingData, b: &Build) -> Self {
        let none = u16::MAX;
        let mut polys = Vec::with_capacity(pf.sectors.len());
        for (k, s) in pf.sectors.iter().enumerate() {
            let mut verts = Vec::new();
            let mut l = s.first_link as usize;
            for _ in 0..pf.links.len() {
                let Some(link) = pf.links.get(l) else { break };
                let left = usize::from(link.left_sector) == k;
                let v = if left { link.vertex1 } else { link.vertex2 };
                verts.push(pf.vertices.get(usize::from(v)).copied().unwrap_or_default());
                l = usize::from(if left {
                    link.forward_link
                } else {
                    link.reverse_link
                });
                if l == s.first_link as usize {
                    break;
                }
            }
            polys.push(Poly::new(verts));
        }
        let mut edges = Vec::new();
        let mut boundary = Vec::new();
        for link in &pf.links {
            let (a, b2) = (
                pf.vertices[usize::from(link.vertex1)],
                pf.vertices[usize::from(link.vertex2)],
            );
            match (link.left_sector, link.right_sector) {
                (l, r) if l != none && r != none => {
                    edges.push(Edge {
                        from: l.into(),
                        to: r.into(),
                        a,
                        b: b2,
                        kind: Kind::Walk,
                    });
                    edges.push(Edge {
                        from: r.into(),
                        to: l.into(),
                        a,
                        b: b2,
                        kind: Kind::Walk,
                    });
                }
                (l, _) if l != none => boundary.push((u32::from(l), a, b2)),
                (_, r) if r != none => boundary.push((u32::from(r), a, b2)),
                _ => {}
            }
        }
        let base = NavMesh::new(polys, Vec::new());
        for (from, a, e) in boundary {
            if let Some(edge) = base.cross_link(from, a, e, b) {
                edges.push(edge);
            }
        }
        NavMesh::new(base.polys, edges)
    }

    /// What lies past the boundary edge `a`-`e` of polygon `from`.
    fn cross_link(&self, from: u32, a: V3, e: V3, b: &Build) -> Option<Edge> {
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
        // Just past the edge: a seam, a step down or a drop.
        let near = add(m, scale(out, b.probe));
        if let Some((to, z)) = self.below(near, m[2] + b.max_jump_up, from) {
            let dz = z - m[2];
            if dz.abs() <= b.step {
                return Some(edge(to, Kind::Walk));
            }
            if dz < 0.0 && -dz <= b.max_drop {
                return Some(edge(to, Kind::Drop));
            }
            if dz > 0.0 {
                return Some(edge(to, Kind::Jump));
            }
        }
        // Across a gap: jump.
        let far = add(m, scale(out, b.max_gap));
        if let Some((to, z)) = self.below(far, m[2] + b.max_jump_up, from) {
            let dz = z - m[2];
            if dz <= b.max_jump_up && -dz <= b.max_drop {
                return Some(edge(to, Kind::Jump));
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
        #[derive(PartialEq)]
        struct Open(f32, u32);
        impl Eq for Open {}
        impl PartialOrd for Open {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Open {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0)
            }
        }
        let n = self.polys.len();
        if start as usize >= n || goal as usize >= n {
            return None;
        }
        let goal_at = self.polys[goal as usize].center;
        let mut best = vec![f32::INFINITY; n];
        let mut came: Vec<Option<u32>> = vec![None; n];
        let mut heap = BinaryHeap::new();
        best[start as usize] = 0.0;
        heap.push(Open(
            dist(self.polys[start as usize].center, goal_at),
            start,
        ));
        while let Some(Open(_, i)) = heap.pop() {
            if i == goal {
                let mut path = Vec::new();
                let mut at = goal;
                while at != start {
                    let e = came[at as usize]?;
                    path.push(e);
                    at = self.edges[e as usize].from;
                }
                path.reverse();
                return Some(path);
            }
            let here = self.polys[i as usize].center;
            for &ei in self.out.get(i as usize).into_iter().flatten() {
                let e = &self.edges[ei as usize];
                let there = self.polys[e.to as usize].center;
                let m = mid(e.a, e.b);
                let g = best[i as usize] + dist(here, m) + dist(m, there) + penalty(e.kind);
                if g < best[e.to as usize] {
                    best[e.to as usize] = g;
                    came[e.to as usize] = Some(ei);
                    heap.push(Open(g + dist(there, goal_at), e.to));
                }
            }
        }
        None
    }

    /// Waypoints from `from` to `to`: the funnel over the corridor's
    /// portals. Each waypoint says how it is reached.
    pub fn path(&self, from: V3, to: V3) -> Option<Vec<Waypoint>> {
        let start = self.locate(from, 1.0)?;
        let goal = self.locate(to, 1.0)?;
        let corridor = self.corridor(start, goal)?;
        Some(funnel(self, from, to, &corridor))
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
        if e.kind == Kind::Walk {
            let c = mesh.polys[e.from as usize].center;
            // Left and right as seen from inside the polygon, looking out.
            let (l, r) = if cross_xy(c, e.a, e.b) > 0.0 {
                (e.b, e.a)
            } else {
                (e.a, e.b)
            };
            run.push((l, r));
            continue;
        }
        // Funnel up to the special edge, aimed at its nearest point.
        let at = closest_on(e.a, e.b, start);
        for (k, c) in pull(start, at, &run).into_iter().enumerate() {
            out.push(Waypoint {
                at: c,
                kind: if k == 0 { pending } else { Kind::Walk },
            });
        }
        pending = e.kind;
        start = at;
        run.clear();
    }
    let corners = pull(start, to, &run);
    for (k, c) in corners.into_iter().enumerate() {
        out.push(Waypoint {
            at: c,
            kind: if k == 0 { pending } else { Kind::Walk },
        });
    }
    out.dedup_by(|b, a| dist_xy(a.at, b.at) < 1e-4 && b.kind == Kind::Walk);
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
}

/// Within this (XY) a waypoint counts as reached.
pub const REACHED: f32 = 0.3;
/// Frames without getting closer before the follower jumps, then gives up.
pub const STUCK_FRAMES: u64 = 30;

impl Follower {
    pub fn new(path: Vec<Waypoint>, frame: u64) -> Self {
        Follower {
            path,
            next: 0,
            best: f32::INFINITY,
            best_frame: frame,
            jumped: false,
        }
    }

    pub fn done(&self) -> bool {
        self.next >= self.path.len()
    }

    /// The goal, if any.
    pub fn goal(&self) -> Option<V3> {
        self.path.last().map(|w| w.at)
    }

    pub fn steer(&mut self, at: V3, frame: u64) -> Steer {
        while let Some(w) = self.path.get(self.next) {
            if dist_xy(w.at, at) > REACHED {
                break;
            }
            self.next += 1;
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
        s.dir = [
            (w.at[0] - at[0]) / d.max(1e-6),
            (w.at[1] - at[1]) / d.max(1e-6),
        ];
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
