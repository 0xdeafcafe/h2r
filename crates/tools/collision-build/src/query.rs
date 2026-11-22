//! The engine's walks over a Halo 2 collision BSP: the vector test with
//! its surface lookup, point location and the 2D point walk
//! (docs/formats/collision.md, "What the engine does with a BSP"). The
//! compiler is accepted against these.
//!
//! The same model is `h1convert::surface_query`, which also reads Halo 1's
//! encoding; this one reads `tags::collision::Bsp` only.
//!
//! - Vector test `0x001de6d0`: recursive over the 3D tree, near side
//!   first; a hit is an open leaf followed by solid, or a leaf with flag
//!   bit 0 followed by another, the surface looked up in the leaf left.
//! - Reference matcher `0x001dea00`: every reference of that leaf on the
//!   crossed plane (flip bit not compared), the point projected by the
//!   reference's plane and flip and walked down its 2D tree, accepted when
//!   it lies on the surface's polygon.
//! - 2D walk `0x00296520`: front child when `a*u + b*v - d >= 0`.

use tags::collision::{Bsp, Child, Child2d};

/// The projection table at `0x00440b94` (E): for `(axis, sign)` the two
/// world axes a point on a plane becomes in 2D.
pub const PROJECTION: [[[usize; 2]; 2]; 3] = [[[2, 1], [1, 2]], [[0, 2], [2, 0]], [[1, 0], [0, 1]]];

/// Distance allowed outside a surface's polygon for the polygon test.
pub const ON_POLYGON: f32 = 1e-3;

/// The dominant axis of a normal, with the engine's tie rule (z, then y).
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

/// The two world axes of `plane`'s projection with `flip`.
pub fn axes(plane: [f32; 4], flip: bool) -> [usize; 2] {
    let n = [plane[0], plane[1], plane[2]];
    let axis = dominant_axis(n);
    let sign = (n[axis] > 0.0) != flip;
    PROJECTION[axis][usize::from(sign)]
}

/// `p` in the 2D space of `plane` with `flip`.
pub fn project(plane: [f32; 4], flip: bool, p: [f32; 3]) -> [f32; 2] {
    let [u, v] = axes(plane, flip);
    [p[u], p[v]]
}

fn kid2(c: Child2d) -> Option<Child2d> {
    // 0xffff (a surface flag on index 0x7fff) is the engine's none.
    (c != Child2d::Surface(0x7fff)).then_some(c)
}

/// The 2D point walk from `root`. `None` for a missing child or a broken
/// tree.
pub fn walk_2d(b: &Bsp, root: Child2d, uv: [f32; 2]) -> Option<usize> {
    let mut k = kid2(root)?;
    for _ in 0..=b.nodes_2d.len() {
        match k {
            Child2d::Surface(s) => return Some(usize::from(s)),
            Child2d::Node(n) => {
                let node = b.nodes_2d.get(usize::from(n))?;
                let l = node.plane;
                let next = if l[0] * uv[0] + l[1] * uv[1] - l[2] >= 0.0 {
                    node.front
                } else {
                    node.back
                };
                k = kid2(next)?;
            }
        }
    }
    None
}

/// A surface's polygon.
pub fn ring(b: &Bsp, s: usize) -> Vec<[f32; 3]> {
    b.surface_ring(s)
        .into_iter()
        .filter_map(|v| b.vertices.get(v).map(|v| v.point))
        .collect()
}

/// A surface's plane (unflipped) and flip bit.
pub fn surface_plane(b: &Bsp, s: usize) -> Option<([f32; 4], bool)> {
    let surf = b.surfaces.get(s)?;
    let p = b.planes.get(usize::from(surf.plane & 0x7fff))?;
    Some((p.0, surf.plane & 0x8000 != 0))
}

/// How far `p` lies outside surface `s`'s polygon, along its plane (0
/// inside), either winding.
pub fn outside(b: &Bsp, s: usize, p: [f32; 3]) -> f32 {
    let Some((pl, flip)) = surface_plane(b, s) else {
        return f32::INFINITY;
    };
    let ring: Vec<[f32; 2]> = ring(b, s).into_iter().map(|v| project(pl, flip, v)).collect();
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
        let d = w * (e[0] * (q[1] - a[1]) - e[1] * (q[0] - a[0])) / len;
        worst = worst.max(-d);
    }
    worst
}

/// The reference matcher: the surface on `plane` among `leaf`'s
/// references whose 2D tree takes `p` to a surface whose polygon holds it.
pub fn surface_at(b: &Bsp, leaf: usize, plane: usize, p: [f32; 3]) -> Option<usize> {
    let l = b.leaves.get(leaf)?;
    let pl = b.planes.get(plane)?.0;
    let first = usize::from(l.first_ref);
    (first..first + usize::from(l.ref_count)).find_map(|r| {
        let r = b.refs.get(r)?;
        if usize::from(r.plane & 0x7fff) != plane || r.node == u16::MAX {
            return None;
        }
        let s = walk_2d(b, r.target(), project(pl, r.plane & 0x8000 != 0, p))?;
        (outside(b, s, p) <= ON_POLYGON).then_some(s)
    })
}

/// A vector test's first hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Fraction along the segment.
    pub t: f32,
    pub plane: usize,
    pub surface: usize,
    /// The open leaf the surface was looked up in.
    pub leaf: usize,
}

/// A vector test: its first hit, and each entry into solid where no
/// surface was found (t, plane, leaf): the engine's fall-through.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub hit: Option<Hit>,
    pub missed: Vec<(f32, usize, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    Open(usize),
    TwoSided(usize),
    Solid,
}

struct Walk<'a> {
    b: &'a Bsp,
    o: [f32; 3],
    v: [f32; 3],
    prev: State,
    plane: usize,
    missed: Vec<(f32, usize, usize)>,
    budget: usize,
}

impl Walk<'_> {
    fn at(&self, t: f32) -> [f32; 3] {
        std::array::from_fn(|i| self.o[i] + self.v[i] * t)
    }

    fn go(&mut self, k: Child, t0: f32, t1: f32, best: &mut Option<Hit>) -> bool {
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        let n = match k {
            Child::Node(n) => n as usize,
            leaf => return self.leaf(leaf, t0, best),
        };
        let Some(node) = self.b.nodes.get(n) else {
            return false;
        };
        let pi = usize::from(node.plane);
        let Some(p) = self.b.planes.get(pi).map(|p| p.0) else {
            return false;
        };
        let base = p[0] * self.o[0] + p[1] * self.o[1] + p[2] * self.o[2] - p[3];
        let dv = p[0] * self.v[0] + p[1] * self.v[1] + p[2] * self.v[2];
        let (d0, d1) = (base + dv * t0, base + dv * t1);
        let (s0, s1) = (d0 >= 0.0, d1 >= 0.0);
        if s0 == s1 {
            return self.go(if s0 { node.front } else { node.back }, t0, t1, best);
        }
        let tm = -base / dv;
        let (near, far) = if dv > 0.0 {
            (node.back, node.front)
        } else {
            (node.front, node.back)
        };
        if self.go(near, t0, tm, best) {
            return true;
        }
        self.plane = pi;
        self.go(far, tm, t1, best)
    }

    fn leaf(&mut self, k: Child, t: f32, best: &mut Option<Hit>) -> bool {
        let now = match k {
            Child::None => State::Solid,
            Child::Leaf(l) => match self.b.leaves.get(l as usize) {
                Some(leaf) if leaf.flags & 1 != 0 => State::TwoSided(l as usize),
                _ => State::Open(l as usize),
            },
            Child::Node(_) => unreachable!(),
        };
        let prev = std::mem::replace(&mut self.prev, now);
        // Front-facing hits and two-sided ones, as a projectile's test asks.
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
pub fn vector(bsp: &Bsp, a: [f32; 3], b: [f32; 3]) -> Trace {
    let mut w = Walk {
        b: bsp,
        o: a,
        v: std::array::from_fn(|i| b[i] - a[i]),
        prev: State::Start,
        plane: usize::MAX,
        missed: Vec::new(),
        budget: 64 * (bsp.nodes.len() + 1),
    };
    let root = if bsp.nodes.is_empty() {
        Child::None
    } else {
        Child::Node(0)
    };
    let mut best = None;
    w.go(root, 0.0, 1.0, &mut best);
    Trace {
        hit: best,
        missed: w.missed,
    }
}

/// Where `p` is: `Some(leaf)` open, `None` solid.
pub fn locate(b: &Bsp, p: [f32; 3]) -> Option<usize> {
    let mut k = if b.nodes.is_empty() {
        Child::None
    } else {
        Child::Node(0)
    };
    for _ in 0..=b.nodes.len() {
        match k {
            Child::None => return None,
            Child::Leaf(l) => return Some(l as usize),
            Child::Node(n) => {
                let node = b.nodes.get(n as usize)?;
                let p4 = b.planes.get(usize::from(node.plane))?.0;
                let s = p4[0] * p[0] + p4[1] * p[1] + p4[2] * p[2] - p4[3];
                k = if s >= 0.0 { node.front } else { node.back };
            }
        }
    }
    None
}
