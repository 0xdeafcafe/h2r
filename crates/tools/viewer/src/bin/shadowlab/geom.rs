//! Vector and matrix helpers, welded triangle meshes and edge topology.
//!
//! Render geometry is stored per part with vertices duplicated at material
//! and normal seams, so its index buffers say nothing about which triangles
//! share an edge. [`TriMesh::weld`] merges vertices by position first; the
//! topology ([`Topology`]) is then what a shadow volume sees.
use std::collections::HashMap;

pub type V3 = [f32; 3];
/// Column-major 4x4: `m[col][row]`, as WGSL reads a `mat4x4<f32>`.
pub type M4 = [[f32; 4]; 4];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}
pub fn normalize(a: V3) -> V3 {
    scale(a, 1.0 / length(a).max(1e-20))
}

pub const IDENTITY: M4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

pub fn mul(a: M4, b: M4) -> M4 {
    let mut o = [[0.0; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            o[c][r] = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    o
}
pub fn apply(m: &M4, v: [f32; 4]) -> [f32; 4] {
    let mut o = [0.0; 4];
    for (r, out) in o.iter_mut().enumerate() {
        *out = (0..4).map(|c| m[c][r] * v[c]).sum();
    }
    o
}
#[cfg(test)]
/// `m * (p, 1)` divided by w: normalised device coordinates.
pub fn project(m: &M4, p: V3) -> V3 {
    let c = apply(m, [p[0], p[1], p[2], 1.0]);
    [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
}

/// Right-handed view matrix (camera looks down -z).
pub fn look_at(eye: V3, at: V3, up: V3) -> M4 {
    let f = normalize(sub(at, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}
/// An up vector that is not parallel to `forward`.
pub fn any_up(forward: V3) -> V3 {
    if forward[2].abs() > 0.99 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Right-handed perspective, depth 0..1.
pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let f = 1.0 / (fovy * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, near * far / (near - far), 0.0],
    ]
}

/// How far below 1.0 a point at infinity lands, so the far plane never clips
/// a shadow volume's extruded (w = 0) vertices.
pub const INFINITE_EPSILON: f32 = 1.0 / 4096.0;

/// Right-handed perspective with the far plane at infinity, depth 0..1.
/// A direction `(d, 0)` in front of the camera lands at depth
/// `1 - INFINITE_EPSILON`; a point on the near plane at 0.
pub fn perspective_infinite(fovy: f32, aspect: f32, near: f32) -> M4 {
    let f = 1.0 / (fovy * 0.5).tan();
    let k = 1.0 - INFINITE_EPSILON;
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, -k, -1.0],
        [0.0, 0.0, -near * k, 0.0],
    ]
}

/// Right-handed orthographic projection of the view-space box
/// `x in l..r, y in b..t, -z in n..f` onto x, y in -1..1 and depth 0..1.
pub fn ortho(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> M4 {
    [
        [2.0 / (r - l), 0.0, 0.0, 0.0],
        [0.0, 2.0 / (t - b), 0.0, 0.0],
        [0.0, 0.0, -1.0 / (f - n), 0.0],
        [-(r + l) / (r - l), -(t + b) / (t - b), -n / (f - n), 1.0],
    ]
}

/// Axis-aligned bounds of points (`None` when empty).
pub fn bounds(points: impl IntoIterator<Item = V3>) -> Option<(V3, V3)> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    let mut any = false;
    for p in points {
        any = true;
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    any.then_some((lo, hi))
}
pub fn box_corners((lo, hi): (V3, V3)) -> [V3; 8] {
    std::array::from_fn(|i| {
        [
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ]
    })
}

/// An indexed triangle mesh whose vertices are unique by position.
#[derive(Clone, Debug, Default)]
pub struct TriMesh {
    pub positions: Vec<V3>,
    pub tris: Vec<[u32; 3]>,
    /// Triangles dropped by welding because two corners merged.
    pub degenerate: usize,
}

/// Positions closer than this (per axis, in world units) are one vertex.
pub const WELD_EPSILON: f32 = 1e-4;

impl TriMesh {
    /// Weld a triangle soup (three positions per triangle).
    pub fn from_soup(soup: &[[V3; 3]], eps: f32) -> Self {
        let mut keys: HashMap<[i64; 3], u32> = HashMap::new();
        let mut m = TriMesh::default();
        for t in soup {
            let idx = t.map(|p| {
                let key = p.map(|c| (c / eps).round() as i64);
                *keys.entry(key).or_insert_with(|| {
                    m.positions.push(p);
                    (m.positions.len() - 1) as u32
                })
            });
            if idx[0] == idx[1] || idx[1] == idx[2] || idx[0] == idx[2] {
                m.degenerate += 1;
            } else {
                m.tris.push(idx);
            }
        }
        m
    }
    pub fn corners(&self, t: usize) -> [V3; 3] {
        self.tris[t].map(|i| self.positions[i as usize])
    }
    /// Unnormalised face normal (counter-clockwise front faces).
    pub fn face_normal(&self, t: usize) -> V3 {
        let [a, b, c] = self.corners(t);
        cross(sub(b, a), sub(c, a))
    }
}

/// One undirected edge and the triangles that use it.
#[derive(Clone, Debug)]
pub struct Edge {
    /// Lower and higher vertex index.
    pub a: u32,
    pub b: u32,
    /// Range into [`Topology::uses`].
    pub first: u32,
    pub count: u32,
}

/// A triangle's use of an edge: `forward` when it walks `a -> b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeUse {
    pub face: u32,
    pub forward: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopoStats {
    pub triangles: usize,
    pub vertices: usize,
    pub edges: usize,
    /// Used by one triangle: the mesh has a hole or a border here.
    pub open_edges: usize,
    /// Used by three or more triangles.
    pub nonmanifold_edges: usize,
    /// Used by exactly two triangles that walk it the same way (a winding flip).
    pub flipped_edges: usize,
    /// Triangles dropped while welding.
    pub degenerate: usize,
    /// Edge-connected pieces, and how many of them are closed (every edge
    /// used by exactly two oppositely wound triangles).
    pub components: usize,
    pub closed_components: usize,
    /// Triangles in closed components.
    pub closed_triangles: usize,
}

impl TopoStats {
    pub fn closed(&self) -> bool {
        self.triangles > 0 && self.components == self.closed_components
    }
    pub fn add(&mut self, o: &TopoStats) {
        self.triangles += o.triangles;
        self.vertices += o.vertices;
        self.edges += o.edges;
        self.open_edges += o.open_edges;
        self.nonmanifold_edges += o.nonmanifold_edges;
        self.flipped_edges += o.flipped_edges;
        self.degenerate += o.degenerate;
        self.components += o.components;
        self.closed_components += o.closed_components;
        self.closed_triangles += o.closed_triangles;
    }
}

pub struct Topology {
    pub edges: Vec<Edge>,
    pub uses: Vec<EdgeUse>,
    pub stats: TopoStats,
}

impl Topology {
    pub fn build(m: &TriMesh) -> Self {
        let mut directed: Vec<(u32, u32, EdgeUse)> = Vec::with_capacity(m.tris.len() * 3);
        for (f, t) in m.tris.iter().enumerate() {
            for k in 0..3 {
                let (p, q) = (t[k], t[(k + 1) % 3]);
                directed.push((
                    p.min(q),
                    p.max(q),
                    EdgeUse {
                        face: f as u32,
                        forward: p < q,
                    },
                ));
            }
        }
        directed.sort_unstable_by_key(|&(a, b, u)| (a, b, u.face));
        let mut edges = Vec::new();
        let mut uses = Vec::with_capacity(directed.len());
        let mut i = 0;
        while i < directed.len() {
            let (a, b, _) = directed[i];
            let first = uses.len() as u32;
            while i < directed.len() && directed[i].0 == a && directed[i].1 == b {
                uses.push(directed[i].2);
                i += 1;
            }
            edges.push(Edge {
                a,
                b,
                first,
                count: uses.len() as u32 - first,
            });
        }

        // Components by union-find over faces sharing an edge.
        let mut parent: Vec<u32> = (0..m.tris.len() as u32).collect();
        fn find(p: &mut [u32], mut x: u32) -> u32 {
            while p[x as usize] != x {
                p[x as usize] = p[p[x as usize] as usize];
                x = p[x as usize];
            }
            x
        }
        for e in &edges {
            let us = &uses[e.first as usize..(e.first + e.count) as usize];
            for w in us.windows(2) {
                let (x, y) = (find(&mut parent, w[0].face), find(&mut parent, w[1].face));
                parent[x as usize] = y;
            }
        }
        let mut stats = TopoStats {
            triangles: m.tris.len(),
            vertices: m.positions.len(),
            edges: edges.len(),
            degenerate: m.degenerate,
            ..Default::default()
        };
        let mut open_root: HashMap<u32, bool> = HashMap::new();
        for e in &edges {
            let us = &uses[e.first as usize..(e.first + e.count) as usize];
            let bad = match e.count {
                1 => {
                    stats.open_edges += 1;
                    true
                }
                2 if us[0].forward == us[1].forward => {
                    stats.flipped_edges += 1;
                    true
                }
                2 => false,
                _ => {
                    stats.nonmanifold_edges += 1;
                    true
                }
            };
            let root = find(&mut parent, us[0].face);
            *open_root.entry(root).or_default() |= bad;
        }
        let mut size: HashMap<u32, usize> = HashMap::new();
        for f in 0..m.tris.len() as u32 {
            *size.entry(find(&mut parent, f)).or_default() += 1;
        }
        stats.components = size.len();
        for (root, n) in &size {
            if !open_root.get(root).copied().unwrap_or(true) {
                stats.closed_components += 1;
                stats.closed_triangles += n;
            }
        }
        Self { edges, uses, stats }
    }
    pub fn uses_of(&self, e: &Edge) -> &[EdgeUse] {
        &self.uses[e.first as usize..(e.first + e.count) as usize]
    }
}

pub mod shapes {
    //! Invented geometry for tests and `--synthetic`: a cube, an open box, a plane with a hole.
    use super::*;

    /// Axis-aligned box from `lo` to `hi`, outward-facing, as a soup; the
    /// faces listed in `skip` (0..6: -x, +x, -y, +y, -z, +z) are left out.
    pub fn boxed(lo: V3, hi: V3, skip: &[usize]) -> Vec<[V3; 3]> {
        let c = box_corners((lo, hi));
        // Corners: bit 0 x, bit 1 y, bit 2 z. Quads wound CCW seen from outside.
        let faces: [[usize; 4]; 6] = [
            [0, 4, 6, 2], // -x
            [1, 3, 7, 5], // +x
            [0, 1, 5, 4], // -y
            [2, 6, 7, 3], // +y
            [0, 2, 3, 1], // -z
            [4, 5, 7, 6], // +z
        ];
        let mut out = Vec::new();
        for (i, q) in faces.iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            out.push([c[q[0]], c[q[1]], c[q[2]]]);
            out.push([c[q[0]], c[q[2]], c[q[3]]]);
        }
        out
    }
    pub fn cube(lo: V3, hi: V3) -> Vec<[V3; 3]> {
        boxed(lo, hi, &[])
    }
    /// A box with no top (+z), like a crate with its lid off.
    pub fn open_box(lo: V3, hi: V3) -> Vec<[V3; 3]> {
        boxed(lo, hi, &[5])
    }
    /// A square plane at height `z`, facing +z, from `-half` to `half`, on an
    /// `n` x `n` grid, with the cells in `hole` (inclusive cell range) left out.
    pub fn plane(half: f32, z: f32, n: usize, hole: Option<(usize, usize)>) -> Vec<[V3; 3]> {
        let step = 2.0 * half / n as f32;
        let p = |i: usize, j: usize| [-half + i as f32 * step, -half + j as f32 * step, z];
        let mut out = Vec::new();
        for i in 0..n {
            for j in 0..n {
                if let Some((a, b)) = hole
                    && (a..=b).contains(&i)
                    && (a..=b).contains(&j)
                {
                    continue;
                }
                out.push([p(i, j), p(i + 1, j), p(i + 1, j + 1)]);
                out.push([p(i, j), p(i + 1, j + 1), p(i, j + 1)]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::shapes::*;
    use super::*;

    fn topo(soup: &[[V3; 3]]) -> TopoStats {
        Topology::build(&TriMesh::from_soup(soup, WELD_EPSILON)).stats
    }

    #[test]
    fn cube_is_closed() {
        let s = topo(&cube([0.0; 3], [1.0; 3]));
        assert_eq!(
            (s.triangles, s.vertices, s.edges, s.open_edges),
            (12, 8, 18, 0)
        );
        assert_eq!((s.components, s.closed_components), (1, 1));
        assert!(s.closed());
        // Outward faces: every normal points away from the centre.
        let m = TriMesh::from_soup(&cube([0.0; 3], [1.0; 3]), WELD_EPSILON);
        for t in 0..m.tris.len() {
            let c = scale(
                add(add(m.corners(t)[0], m.corners(t)[1]), m.corners(t)[2]),
                1.0 / 3.0,
            );
            assert!(dot(m.face_normal(t), sub(c, [0.5; 3])) > 0.0);
        }
    }

    #[test]
    fn open_box_has_its_rim_open() {
        let s = topo(&open_box([0.0; 3], [1.0; 3]));
        assert_eq!((s.triangles, s.open_edges), (10, 4));
        assert_eq!((s.components, s.closed_components), (1, 0));
        assert!(!s.closed());
    }

    #[test]
    fn plane_with_hole_counts_outer_and_inner_borders() {
        // 4x4 grid: 16 border segments outside; a 2x2 hole adds 8 inside.
        let s = topo(&plane(2.0, 0.0, 4, Some((1, 2))));
        assert_eq!(s.triangles, (16 - 4) * 2);
        assert_eq!(s.open_edges, 16 + 8);
        assert_eq!(topo(&plane(2.0, 0.0, 4, None)).open_edges, 16);
    }

    #[test]
    fn duplicated_seam_vertices_are_welded() {
        // The same cube, but each triangle with its own copies of the corners
        // (as render geometry stores it at material seams), slightly jittered.
        let soup: Vec<[V3; 3]> = cube([0.0; 3], [1.0; 3])
            .into_iter()
            .enumerate()
            .map(|(i, t)| t.map(|p| add(p, [(i % 3) as f32 * 1e-6; 3])))
            .collect();
        assert!(topo(&soup).closed());
    }

    #[test]
    fn nonmanifold_and_flipped_edges_are_counted() {
        // Two cubes sharing an edge exactly: that edge has four uses.
        let mut soup = cube([0.0; 3], [1.0; 3]);
        soup.extend(cube([1.0, 1.0, 0.0], [2.0, 2.0, 1.0]));
        let s = topo(&soup);
        assert_eq!(s.nonmanifold_edges, 1);
        assert_eq!(s.components, 1);
        assert_eq!(s.closed_components, 0);
        // One triangle wound the wrong way flips its three edges.
        let mut soup = cube([0.0; 3], [1.0; 3]);
        soup[0].swap(1, 2);
        let s = topo(&soup);
        assert_eq!((s.flipped_edges, s.open_edges), (3, 0));
        assert!(!s.closed());
    }

    #[test]
    fn degenerate_triangles_are_dropped_and_counted() {
        let s = topo(&[[[0.0; 3], [0.0, 0.0, 1e-6], [1.0, 0.0, 0.0]]]);
        assert_eq!((s.triangles, s.degenerate), (0, 1));
    }
}
