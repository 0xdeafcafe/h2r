//! The engine's vector test with surface lookup, modelled over both games'
//! collision BSPs, so a converted structure BSP can be checked against its
//! Halo 1 source surface for surface (docs/formats/collision.md, "What the
//! engine does with a BSP").
//!
//! [`crate::query`] answers "solid or open"; this answers "which surface":
//! the walk of Halo 2's `0x001de6d0` (split the segment at each node plane,
//! near side first; a hit is an open leaf followed by solid, or a leaf with
//! two-sided surfaces followed by another), the reference matcher
//! `0x001dea00` (the hit plane among the open leaf's 2D references, the
//! point projected by the plane's dominant axis), the 2D point walk
//! `0x00296520` and the polygon test that confirms the surface. Each game
//! reads its own encoding through [`Surfaces`].
//!
//! Halo 1's engine is not disassembled here; that its data follows the same
//! rules is what the disc test (`tests/collision_engine.rs`) shows.

/// A 3D child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kid {
    Node(usize),
    Leaf(usize),
    Solid,
}

/// A 2D child: a node, a surface, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kid2 {
    Node(usize),
    Surface(usize),
    None,
}

/// A collision BSP as each game stores it.
pub trait Surfaces {
    fn node_count(&self) -> usize;
    /// Plane index, back child, front child.
    fn node(&self, i: usize) -> Option<(usize, Kid, Kid)>;
    fn plane(&self, i: usize) -> Option<[f32; 4]>;
    /// Flags, first reference, reference count.
    fn leaf(&self, i: usize) -> Option<(u32, usize, usize)>;
    /// Plane index, flip, 2D root.
    fn reference(&self, i: usize) -> Option<(usize, bool, Kid2)>;
    /// Line `a, b, d`, back child (`a*u + b*v - d < 0`), front child.
    fn node_2d(&self, i: usize) -> Option<([f32; 3], Kid2, Kid2)>;
    /// Plane index, flip, flags.
    fn surface(&self, i: usize) -> Option<(usize, bool, u8)>;
    fn surface_count(&self) -> usize;
    /// The surface's polygon.
    fn ring(&self, i: usize) -> Vec<[f32; 3]>;
}

impl Surfaces for h1map::bsp::CollisionBsp {
    fn node_count(&self) -> usize {
        self.nodes.len()
    }
    fn node(&self, i: usize) -> Option<(usize, Kid, Kid)> {
        use h1map::bsp::Child;
        let n = self.nodes.get(i)?;
        let kid = |c| match c {
            Child::Node(n) => Kid::Node(n as usize),
            Child::Leaf(l) => Kid::Leaf(l as usize),
            Child::Solid => Kid::Solid,
        };
        Some((n.plane as usize, kid(n.back), kid(n.front)))
    }
    fn plane(&self, i: usize) -> Option<[f32; 4]> {
        self.planes.get(i).copied()
    }
    fn leaf(&self, i: usize) -> Option<(u32, usize, usize)> {
        let l = self.leaves.get(i)?;
        Some((l.flags.into(), l.first_ref as usize, l.ref_count.into()))
    }
    fn reference(&self, i: usize) -> Option<(usize, bool, Kid2)> {
        let r = self.refs.get(i)?;
        Some((
            (r.plane & 0x7fff_ffff) as usize,
            r.plane & 0x8000_0000 != 0,
            kid2_h1(r.node),
        ))
    }
    fn node_2d(&self, i: usize) -> Option<([f32; 3], Kid2, Kid2)> {
        let n = self.nodes_2d.get(i)?;
        Some((n.plane, kid2_h1(n.back), kid2_h1(n.front)))
    }
    fn surface(&self, i: usize) -> Option<(usize, bool, u8)> {
        let s = self.surfaces.get(i)?;
        Some((
            (s.plane & 0x7fff_ffff) as usize,
            s.plane & 0x8000_0000 != 0,
            s.flags,
        ))
    }
    fn surface_count(&self) -> usize {
        self.surfaces.len()
    }
    fn ring(&self, i: usize) -> Vec<[f32; 3]> {
        self.surface_ring(i)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| self.vertices.get(v).map(|v| v.point))
            .collect()
    }
}

fn kid2_h1(v: u32) -> Kid2 {
    if v == u32::MAX {
        Kid2::None
    } else if v & 0x8000_0000 != 0 {
        Kid2::Surface((v & 0x7fff_ffff) as usize)
    } else {
        Kid2::Node(v as usize)
    }
}

impl Surfaces for tags::collision::Bsp {
    fn node_count(&self) -> usize {
        self.nodes.len()
    }
    fn node(&self, i: usize) -> Option<(usize, Kid, Kid)> {
        use tags::collision::Child;
        let n = self.nodes.get(i)?;
        let kid = |c| match c {
            Child::Node(n) => Kid::Node(n as usize),
            Child::Leaf(l) => Kid::Leaf(l as usize),
            Child::None => Kid::Solid,
        };
        Some((n.plane.into(), kid(n.back), kid(n.front)))
    }
    fn plane(&self, i: usize) -> Option<[f32; 4]> {
        self.planes.get(i).map(|p| p.0)
    }
    fn leaf(&self, i: usize) -> Option<(u32, usize, usize)> {
        let l = self.leaves.get(i)?;
        Some((l.flags.into(), l.first_ref.into(), l.ref_count.into()))
    }
    fn reference(&self, i: usize) -> Option<(usize, bool, Kid2)> {
        let r = self.refs.get(i)?;
        let node = if r.node == u16::MAX {
            Kid2::None
        } else if r.node & 0x8000 != 0 {
            Kid2::Surface((r.node & 0x7fff).into())
        } else {
            Kid2::Node(r.node.into())
        };
        Some(((r.plane & 0x7fff).into(), r.plane & 0x8000 != 0, node))
    }
    fn node_2d(&self, i: usize) -> Option<([f32; 3], Kid2, Kid2)> {
        use tags::collision::Child2d;
        let n = self.nodes_2d.get(i)?;
        // 0xffff (a surface flag on index 0x7fff) is the engine's none.
        let kid = |c| match c {
            Child2d::Surface(0x7fff) => Kid2::None,
            Child2d::Node(n) => Kid2::Node(n.into()),
            Child2d::Surface(s) => Kid2::Surface(s.into()),
        };
        Some((n.plane, kid(n.back), kid(n.front)))
    }
    fn surface(&self, i: usize) -> Option<(usize, bool, u8)> {
        let s = self.surfaces.get(i)?;
        Some(((s.plane & 0x7fff).into(), s.plane & 0x8000 != 0, s.flags))
    }
    fn surface_count(&self) -> usize {
        self.surfaces.len()
    }
    fn ring(&self, i: usize) -> Vec<[f32; 3]> {
        self.surface_ring(i)
            .into_iter()
            .filter_map(|v| self.vertices.get(v).map(|v| v.point))
            .collect()
    }
}

/// The projection table at `0x00440b94` (E): for `(axis, sign)` the two
/// world axes a point on a plane becomes in 2D. `axis` is the plane's
/// dominant normal axis (z when `|z| >= |y|` and `|z| >= |x|`, else y when
/// `|y| >= |x|`, else x: `0x001dea5c`..`0x001deab0`), `sign` is
/// `(n[axis] > 0) != flip`.
pub const PROJECTION: [[[usize; 2]; 2]; 3] = [[[2, 1], [1, 2]], [[0, 2], [2, 0]], [[1, 0], [0, 1]]];

/// The dominant axis of a normal, with the engine's tie rule.
pub fn dominant_axis(n: [f32; 3]) -> usize {
    let [x, y, z] = n.map(f32::abs);
    if z >= y && z >= x {
        2
    } else if y >= x {
        1
    } else {
        0
    }
}

/// `p` in the 2D space of `plane` with `flip`.
pub fn project(plane: [f32; 4], flip: bool, p: [f32; 3]) -> [f32; 2] {
    let n = [plane[0], plane[1], plane[2]];
    let axis = dominant_axis(n);
    let sign = (n[axis] > 0.0) != flip;
    let [u, v] = PROJECTION[axis][usize::from(sign)];
    [p[u], p[v]]
}

/// The 2D point walk (`0x00296520`): front child when `a*u + b*v - d >= 0`.
/// `None` for a missing child or a broken tree.
pub fn walk_2d(b: &impl Surfaces, root: Kid2, uv: [f32; 2]) -> Option<usize> {
    let mut k = root;
    for _ in 0..=b.surface_count() + 4096 {
        match k {
            Kid2::Surface(s) => return Some(s),
            Kid2::None => return None,
            Kid2::Node(n) => {
                let (l, back, front) = b.node_2d(n)?;
                k = if l[0] * uv[0] + l[1] * uv[1] - l[2] >= 0.0 {
                    front
                } else {
                    back
                };
            }
        }
    }
    None
}

/// How far `p` lies outside surface `s`'s polygon, in world units along its
/// plane (0 inside). The polygon is taken in the plane's projection, either
/// winding.
pub fn outside(b: &impl Surfaces, s: usize, p: [f32; 3]) -> f32 {
    let Some((plane, flip, _)) = b.surface(s) else {
        return f32::INFINITY;
    };
    let Some(pl) = b.plane(plane) else {
        return f32::INFINITY;
    };
    let ring: Vec<[f32; 2]> = b
        .ring(s)
        .into_iter()
        .map(|v| project(pl, flip, v))
        .collect();
    if ring.len() < 3 {
        return f32::INFINITY;
    }
    let q = project(pl, flip, p);
    let area: f32 = (0..ring.len())
        .map(|i| {
            let (a, c) = (ring[i], ring[(i + 1) % ring.len()]);
            a[0] * c[1] - c[0] * a[1]
        })
        .sum();
    let w = area.signum();
    let mut worst = 0.0f32;
    for i in 0..ring.len() {
        let (a, c) = (ring[i], ring[(i + 1) % ring.len()]);
        let e = [c[0] - a[0], c[1] - a[1]];
        let len = (e[0] * e[0] + e[1] * e[1]).sqrt();
        if len == 0.0 {
            continue;
        }
        // Signed distance to the left of the edge, for a positive winding.
        let d = w * (e[0] * (q[1] - a[1]) - e[1] * (q[0] - a[0])) / len;
        worst = worst.max(-d);
    }
    worst
}

/// Distance allowed outside a surface's polygon for the polygon test.
pub const ON_POLYGON: f32 = 1e-3;

/// The reference matcher (`0x001dea00`): the surface on `plane` among
/// `leaf`'s references whose 2D tree takes `p` to a surface whose polygon
/// holds it. `None` when none does: the engine then reports no hit there.
pub fn surface_at(b: &impl Surfaces, leaf: usize, plane: usize, p: [f32; 3]) -> Option<usize> {
    let (_, first, count) = b.leaf(leaf)?;
    let pl = b.plane(plane)?;
    (first..first + count).find_map(|r| {
        let (rp, flip, root) = b.reference(r)?;
        if rp != plane {
            return None;
        }
        let s = walk_2d(b, root, project(pl, flip, p))?;
        (outside(b, s, p) <= ON_POLYGON).then_some(s)
    })
}

/// A vector test's first hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Fraction along the segment.
    pub t: f32,
    /// The plane crossed.
    pub plane: usize,
    /// The surface the matcher found there.
    pub surface: usize,
    /// The open leaf the surface was looked up in.
    pub leaf: usize,
}

/// What a vector test met.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// Nothing: the segment stays in open space or only enters solid where
    /// no surface is found ([`Outcome::Missed`] counts those).
    Clear,
    Hit(Hit),
}

/// A vector test: its outcome and the solid entries it passed through
/// because no surface was found there (the engine's fall-through).
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub outcome: Outcome,
    /// Each entry into solid with no surface: (t, plane, leaf).
    pub missed: Vec<(f32, usize, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    Open(usize),
    TwoSided(usize),
    Solid,
}

struct Walk<'a, B> {
    b: &'a B,
    o: [f32; 3],
    v: [f32; 3],
    prev: State,
    plane: usize,
    missed: Vec<(f32, usize, usize)>,
    budget: usize,
}

impl<B: Surfaces> Walk<'_, B> {
    fn at(&self, t: f32) -> [f32; 3] {
        std::array::from_fn(|i| self.o[i] + self.v[i] * t)
    }

    /// The engine's recursion: `true` with a hit in `best`.
    fn go(&mut self, k: Kid, t0: f32, t1: f32, best: &mut Option<Hit>) -> bool {
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        let n = match k {
            Kid::Node(n) => n,
            leaf => return self.leaf(leaf, t0, best),
        };
        let Some((pi, back, front)) = self.b.node(n) else {
            return false;
        };
        let Some(p) = self.b.plane(pi) else {
            return false;
        };
        let base = p[0] * self.o[0] + p[1] * self.o[1] + p[2] * self.o[2] - p[3];
        let dv = p[0] * self.v[0] + p[1] * self.v[1] + p[2] * self.v[2];
        let (d0, d1) = (base + dv * t0, base + dv * t1);
        let (s0, s1) = (d0 >= 0.0, d1 >= 0.0);
        if s0 == s1 {
            return self.go(if s0 { front } else { back }, t0, t1, best);
        }
        let tm = -base / dv;
        let (near, far) = if dv > 0.0 {
            (back, front)
        } else {
            (front, back)
        };
        if self.go(near, t0, tm, best) {
            return true;
        }
        self.plane = pi;
        self.go(far, tm, t1, best)
    }

    fn leaf(&mut self, k: Kid, t: f32, best: &mut Option<Hit>) -> bool {
        let now = match k {
            Kid::Solid => State::Solid,
            Kid::Leaf(l) => match self.b.leaf(l) {
                Some((f, ..)) if f & 1 != 0 => State::TwoSided(l),
                _ => State::Open(l),
            },
            Kid::Node(_) => unreachable!(),
        };
        let prev = std::mem::replace(&mut self.prev, now);
        // Front-facing hits (query flag bit 0) and two-sided ones (bit 2
        // clear), as a biped's or a projectile's test asks.
        let lookup = match (prev, now) {
            (State::Open(l) | State::TwoSided(l), State::Solid) => Some(l),
            (State::TwoSided(l), State::TwoSided(_)) => Some(l),
            _ => None,
        };
        let Some(l) = lookup else {
            return false;
        };
        match surface_at(self.b, l, self.plane, self.at(t)) {
            Some(s) => {
                *best = Some(Hit {
                    t,
                    plane: self.plane,
                    surface: s,
                    leaf: l,
                });
                true
            }
            None => {
                if now == State::Solid {
                    self.missed.push((t, self.plane, l));
                }
                false
            }
        }
    }
}

/// The vector test from `a` to `b`.
pub fn vector(bsp: &impl Surfaces, a: [f32; 3], b: [f32; 3]) -> Trace {
    let mut w = Walk {
        b: bsp,
        o: a,
        v: std::array::from_fn(|i| b[i] - a[i]),
        prev: State::Start,
        plane: usize::MAX,
        missed: Vec::new(),
        budget: 64 * (bsp.node_count() + 1),
    };
    let root = if bsp.node_count() == 0 {
        Kid::Solid
    } else {
        Kid::Node(0)
    };
    let mut best = None;
    w.go(root, 0.0, 1.0, &mut best);
    Trace {
        outcome: best.map_or(Outcome::Clear, Outcome::Hit),
        missed: w.missed,
    }
}

/// Where `p` is: `Some(leaf)` open, `None` solid.
pub fn locate(b: &impl Surfaces, p: [f32; 3]) -> Option<usize> {
    let mut k = if b.node_count() == 0 {
        Kid::Solid
    } else {
        Kid::Node(0)
    };
    for _ in 0..=b.node_count() {
        match k {
            Kid::Solid => return None,
            Kid::Leaf(l) => return Some(l),
            Kid::Node(n) => {
                let (pi, back, front) = b.node(n)?;
                let p4 = b.plane(pi)?;
                let s = p4[0] * p[0] + p4[1] * p[1] + p4[2] * p[2] - p4[3];
                k = if s >= 0.0 { front } else { back };
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use h1map::bsp::{
        Child as C, CollisionBsp, Edge, Leaf, Node2d, Node3d, Ref2d, Surface, Vertex,
    };

    /// A floor: the half-space `z < 0` solid, one square surface on `z = 0`
    /// from -1 to 1 behind a 2D line `x = -1` whose front is the square and
    /// whose back is the square too, or nothing.
    fn floor(line_back_none: bool) -> CollisionBsp {
        let v = |x: f32, y: f32| Vertex {
            point: [x, y, 0.0],
            first_edge: 0,
        };
        let e = |a: u32, b: u32, f: u32, r: u32| Edge {
            start_vertex: a,
            end_vertex: b,
            forward_edge: f,
            reverse_edge: r,
            left_surface: 0,
            right_surface: u32::MAX,
        };
        CollisionBsp {
            nodes: vec![Node3d {
                plane: 0,
                back: C::Solid,
                front: C::Leaf(0),
            }],
            planes: vec![[0.0, 0.0, 1.0, 0.0]],
            leaves: vec![Leaf {
                flags: 0,
                ref_count: 1,
                first_ref: 0,
            }],
            refs: vec![Ref2d { plane: 0, node: 0 }],
            nodes_2d: vec![Node2d {
                plane: [1.0, 0.0, -1.0],
                back: if line_back_none {
                    u32::MAX
                } else {
                    0x8000_0000
                },
                front: 0x8000_0000,
            }],
            surfaces: vec![Surface {
                plane: 0,
                first_edge: 0,
                flags: 0,
                breakable_surface: -1,
                material: 0,
            }],
            edges: vec![e(0, 1, 1, 0), e(1, 2, 2, 0), e(2, 3, 3, 0), e(3, 0, 0, 0)],
            vertices: vec![v(-1.0, -1.0), v(1.0, -1.0), v(1.0, 1.0), v(-1.0, 1.0)],
        }
    }

    #[test]
    fn projection_table_is_right_handed_on_the_positive_side() {
        // A floor (+z) projects to (x, y); a ceiling, flipped, to (y, x).
        assert_eq!(
            project([0.0, 0.0, 1.0, 0.0], false, [1.0, 2.0, 3.0]),
            [1.0, 2.0]
        );
        assert_eq!(
            project([0.0, 0.0, 1.0, 0.0], true, [1.0, 2.0, 3.0]),
            [2.0, 1.0]
        );
        assert_eq!(
            project([0.0, -1.0, 0.0, 0.0], false, [1.0, 2.0, 3.0]),
            [1.0, 3.0]
        );
        // Ties go to z, then y.
        assert_eq!(dominant_axis([0.6, 0.6, 0.6]), 2);
        assert_eq!(dominant_axis([0.7, 0.7, 0.1]), 1);
    }

    #[test]
    fn a_ray_down_hits_the_floor_in_both_encodings() {
        let h1 = floor(false);
        let h2 = crate::collision::convert(&h1).unwrap();
        let t1 = vector(&h1, [0.2, 0.3, 1.0], [0.2, 0.3, -1.0]);
        let t2 = vector(&h2, [0.2, 0.3, 1.0], [0.2, 0.3, -1.0]);
        let Outcome::Hit(h) = t1.outcome else {
            panic!("{t1:?}")
        };
        assert_eq!((h.t, h.surface, h.plane, h.leaf), (0.5, 0, 0, 0));
        assert_eq!(t1, t2);
        // Off the polygon: solid entered, no surface.
        let t = vector(&h1, [3.0, 0.0, 1.0], [3.0, 0.0, -1.0]);
        assert_eq!(t.outcome, Outcome::Clear);
        assert_eq!(t.missed.len(), 1);
    }

    #[test]
    fn a_missing_2d_child_is_a_fall_through() {
        // The line is x + 1 >= 0: in front, the floor; behind, nothing.
        let h1 = floor(true);
        let t = vector(&h1, [0.5, 0.0, 1.0], [0.5, 0.0, -1.0]);
        assert!(matches!(t.outcome, Outcome::Hit(_)), "{t:?}");
        let t = vector(&h1, [-1.5, 0.0, 1.0], [-1.5, 0.0, -1.0]);
        assert_eq!((t.outcome, t.missed.len()), (Outcome::Clear, 1));
    }
}
