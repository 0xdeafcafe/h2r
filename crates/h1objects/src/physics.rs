//! Halo 1 collision as Havok convex pieces, for a Halo 2 physics model.
//!
//! A collision BSP's 3D tree splits space by planes down to leaves (open
//! space) and missing children (solid: inside the object, [`crate::query`]).
//! Every root-to-solid path is a set of half-spaces whose intersection is
//! convex, and the solid cells together are exactly the object's solid
//! space. [`solid_cells`] walks the tree, clips each path's half-spaces to
//! the BSP's vertex box (the tree's outermost solid cells are open on the
//! sides no plane closes) and returns each non-empty cell as a [`Hull`]:
//! its corners, its face planes (`n . p + w <= 0` inside, the phmo's form)
//! and its volume, centroid and inertia. These become the physics model's
//! polyhedra ([`crate::rebuild::physics_model`]).
//!
//! The arithmetic is `f64`; corners are the intersections of three of the
//! cell's planes that lie inside all of them.

use tags::collision::{Bsp, Child};

/// Slack for a corner lying inside a plane and for corner identity
/// (world units).
const EPS: f64 = 1e-4;
/// Cells thinner than this volume (cubic world units) are dropped: slivers
/// of a split plane, not material.
const MIN_VOLUME: f64 = 1e-6;
/// The most planes a path may collect before the walk gives up on it (the
/// tree is bounded by its node count anyway; this keeps a malformed one
/// from running long).
const MAX_DEPTH: usize = 96;

/// One convex solid piece.
#[derive(Clone, Debug, PartialEq)]
pub struct Hull {
    pub vertices: Vec<[f32; 3]>,
    /// `[nx, ny, nz, w]`, unit normal pointing out, `n . p + w <= 0` inside.
    pub planes: Vec<[f32; 4]>,
    pub volume: f32,
    pub centroid: [f32; 3],
    /// Inertia about the centroid for a density of 1 (rows).
    pub inertia: [[f32; 3]; 3],
}

impl Hull {
    /// Bounding box: (centre, half extents).
    pub fn aabb(&self) -> ([f32; 3], [f32; 3]) {
        aabb(&self.vertices)
    }

    /// The part of the piece at or above `z`, or `None` when nothing of it
    /// is (or only a sliver).
    pub fn above(&self, z: f32) -> Option<Hull> {
        let mut planes: Vec<(V, f64)> = self
            .planes
            .iter()
            .map(|p| {
                (
                    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])],
                    f64::from(p[3]),
                )
            })
            .collect();
        // -z + floor <= 0.
        planes.push(([0.0, 0.0, -1.0], f64::from(z)));
        hull(&planes)
    }

    /// Moved by `m` (`p' = R p + t`).
    pub fn transformed(&self, m: &Transform) -> Hull {
        let v = self.vertices.iter().map(|p| m.point(*p)).collect();
        let planes = self
            .planes
            .iter()
            .map(|pl| {
                let n = m.vector([pl[0], pl[1], pl[2]]);
                // n'.p' + w' = n.p + w with p' = R p + t
                let w = pl[3] - (n[0] * m.t[0] + n[1] * m.t[1] + n[2] * m.t[2]);
                [n[0], n[1], n[2], w]
            })
            .collect();
        // I' = R I R^T for the rotation part.
        let r = m.r;
        let i = self.inertia;
        let ri: [[f32; 3]; 3] = std::array::from_fn(|a| {
            std::array::from_fn(|b| (0..3).map(|k| r[a][k] * i[k][b]).sum())
        });
        let inertia = std::array::from_fn(|a| {
            std::array::from_fn(|b| (0..3).map(|k| ri[a][k] * r[b][k]).sum())
        });
        Hull {
            vertices: v,
            planes,
            volume: self.volume,
            centroid: m.point(self.centroid),
            inertia,
        }
    }
}

/// A rigid transform `p' = r p + t` (`r` row-major, acting on column
/// vectors, as [`h1models::mode::world_transforms`] gives a node's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub r: [[f32; 3]; 3],
    pub t: [f32; 3],
}

impl Transform {
    pub const IDENTITY: Self = Self {
        r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };

    pub fn vector(&self, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|a| self.r[a][0] * v[0] + self.r[a][1] * v[1] + self.r[a][2] * v[2])
    }

    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let v = self.vector(p);
        std::array::from_fn(|a| v[a] + self.t[a])
    }

    /// `self` after `inner`: `p -> self(inner(p))`.
    pub fn then(&self, inner: &Transform) -> Transform {
        let r = std::array::from_fn(|a| {
            std::array::from_fn(|b| (0..3).map(|k| self.r[a][k] * inner.r[k][b]).sum())
        });
        Transform {
            r,
            t: self.point(inner.t),
        }
    }
}

/// (centre, half extents) of points; zeros for none.
pub fn aabb(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    if points.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    (
        std::array::from_fn(|a| (lo[a] + hi[a]) / 2.0),
        std::array::from_fn(|a| (hi[a] - lo[a]) / 2.0),
    )
}

type V = [f64; 3];

fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The solid cells of `bsp`, in tree order (back before front).
pub fn solid_cells(bsp: &Bsp) -> Vec<Hull> {
    if bsp.nodes.is_empty() || bsp.vertices.is_empty() {
        return Vec::new();
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &bsp.vertices {
        for a in 0..3 {
            lo[a] = lo[a].min(f64::from(v.point[a]));
            hi[a] = hi[a].max(f64::from(v.point[a]));
        }
    }
    // The box's six faces, n . p + w <= 0 inside.
    let mut bounds = Vec::new();
    for a in 0..3 {
        let mut n = [0.0; 3];
        n[a] = 1.0;
        bounds.push((n, -hi[a]));
        n[a] = -1.0;
        bounds.push((n, lo[a]));
    }
    let mut out = Vec::new();
    let mut path = Vec::new();
    walk(bsp, Child::Node(0), &mut path, &bounds, &mut out);
    out
}

fn walk(bsp: &Bsp, c: Child, path: &mut Vec<(V, f64)>, bounds: &[(V, f64)], out: &mut Vec<Hull>) {
    match c {
        Child::Leaf(_) => {}
        Child::None => {
            let mut planes: Vec<(V, f64)> = bounds.to_vec();
            planes.extend(path.iter().copied());
            if let Some(h) = hull(&planes) {
                out.push(h);
            }
        }
        Child::Node(i) => {
            if path.len() >= MAX_DEPTH {
                return;
            }
            let Some(node) = bsp.nodes.get(i as usize) else {
                return;
            };
            let Some(p) = bsp.planes.get(usize::from(node.plane)) else {
                return;
            };
            let n = [f64::from(p.0[0]), f64::from(p.0[1]), f64::from(p.0[2])];
            let d = f64::from(p.0[3]);
            let len = dot(n, n).sqrt();
            if len < 1e-9 {
                return;
            }
            let (n, d) = ([n[0] / len, n[1] / len, n[2] / len], d / len);
            // Back: n.p - d < 0, i.e. n.p - d <= 0.
            path.push((n, -d));
            walk(bsp, node.back, path, bounds, out);
            path.pop();
            // Front: n.p - d >= 0, i.e. -n.p + d <= 0.
            path.push(([-n[0], -n[1], -n[2]], d));
            walk(bsp, node.front, path, bounds, out);
            path.pop();
        }
    }
}

/// The convex cell `n . p + w <= 0` for every plane, or `None` when it is
/// empty or a sliver.
pub(crate) fn hull(planes: &[(V, f64)]) -> Option<Hull> {
    // Drop repeated planes.
    let mut ps: Vec<(V, f64)> = Vec::new();
    for &(n, w) in planes {
        if !ps
            .iter()
            .any(|&(m, x)| dot(n, m) > 1.0 - 1e-9 && (w - x).abs() < EPS)
        {
            ps.push((n, w));
        }
    }
    let inside = |p: V| ps.iter().all(|&(n, w)| dot(n, p) + w <= EPS);
    let mut verts: Vec<V> = Vec::new();
    for a in 0..ps.len() {
        for b in a + 1..ps.len() {
            let ab = cross(ps[a].0, ps[b].0);
            if dot(ab, ab) < 1e-12 {
                continue;
            }
            for c in b + 1..ps.len() {
                let det = dot(ps[c].0, ab);
                if det.abs() < 1e-9 {
                    continue;
                }
                // p = (-wa (nb x nc) - wb (nc x na) - wc (na x nb)) / det
                let (na, wa) = ps[a];
                let (nb, wb) = ps[b];
                let (nc, wc) = ps[c];
                let bc = cross(nb, nc);
                let ca = cross(nc, na);
                let p: V = std::array::from_fn(|k| (-wa * bc[k] - wb * ca[k] - wc * ab[k]) / det);
                if inside(p)
                    && !verts
                        .iter()
                        .any(|q| dot(sub(*q, p), sub(*q, p)) < EPS * EPS)
                {
                    verts.push(p);
                }
            }
        }
    }
    if verts.len() < 4 {
        return None;
    }
    let mid: V =
        std::array::from_fn(|k| verts.iter().map(|v| v[k]).sum::<f64>() / verts.len() as f64);
    // Faces: planes holding three or more corners, each once.
    let mut faces: Vec<(V, f64, Vec<usize>)> = Vec::new();
    for &(n, w) in &ps {
        let on: Vec<usize> = (0..verts.len())
            .filter(|&i| (dot(n, verts[i]) + w).abs() <= EPS)
            .collect();
        if on.len() < 3 || faces.iter().any(|f| f.2 == on) {
            continue;
        }
        faces.push((n, w, on));
    }
    // Volume, centroid and covariance by tetrahedra from `mid` to each
    // face's fan.
    let mut volume = 0.0;
    let mut first = [0.0; 3];
    let mut cov = [[0.0f64; 3]; 3];
    for (n, _, on) in &mut faces {
        let c: V =
            std::array::from_fn(|k| on.iter().map(|&i| verts[i][k]).sum::<f64>() / on.len() as f64);
        let ref_axis = if n[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let u = cross(*n, ref_axis);
        let ul = dot(u, u).sqrt();
        let u = [u[0] / ul, u[1] / ul, u[2] / ul];
        let v = cross(*n, u);
        on.sort_by(|&a, &b| {
            let ang = |i: usize| {
                let d = sub(verts[i], c);
                dot(d, v).atan2(dot(d, u))
            };
            ang(a).total_cmp(&ang(b))
        });
        for k in 1..on.len() - 1 {
            let (a, b, cc) = (
                sub(verts[on[0]], mid),
                sub(verts[on[k]], mid),
                sub(verts[on[k + 1]], mid),
            );
            let det = dot(a, cross(b, cc)).abs();
            let vol = det / 6.0;
            volume += vol;
            for x in 0..3 {
                first[x] += vol * (a[x] + b[x] + cc[x]) / 4.0;
            }
            let s: V = std::array::from_fn(|x| a[x] + b[x] + cc[x]);
            for x in 0..3 {
                for y in 0..3 {
                    cov[x][y] +=
                        det / 120.0 * (a[x] * a[y] + b[x] * b[y] + cc[x] * cc[y] + s[x] * s[y]);
                }
            }
        }
    }
    if volume < MIN_VOLUME {
        return None;
    }
    let g: V = std::array::from_fn(|k| first[k] / volume);
    // Move the covariance from `mid` to the centroid, then I = tr(C) 1 - C.
    let cg: [[f64; 3]; 3] =
        std::array::from_fn(|x| std::array::from_fn(|y| cov[x][y] - volume * g[x] * g[y]));
    let tr = cg[0][0] + cg[1][1] + cg[2][2];
    let inertia = std::array::from_fn(|x| {
        std::array::from_fn(|y| ((if x == y { tr } else { 0.0 }) - cg[x][y]) as f32)
    });
    let f = |p: V| [p[0] as f32, p[1] as f32, p[2] as f32];
    Some(Hull {
        vertices: verts.iter().map(|v| f(*v)).collect(),
        planes: faces
            .iter()
            .map(|(n, w, _)| [n[0] as f32, n[1] as f32, n[2] as f32, *w as f32])
            .collect(),
        volume: volume as f32,
        centroid: f(std::array::from_fn(|k| mid[k] + g[k])),
        inertia,
    })
}

/// The combined mass properties of hulls at `density`: (mass, centre of
/// mass, inertia about it).
pub fn mass_properties(hulls: &[Hull], density: f32) -> (f32, [f32; 3], [[f32; 3]; 3]) {
    let mass: f32 = hulls.iter().map(|h| h.volume * density).sum();
    if mass <= 0.0 {
        return (0.0, [0.0; 3], [[0.0; 3]; 3]);
    }
    let com: [f32; 3] = std::array::from_fn(|k| {
        hulls
            .iter()
            .map(|h| h.volume * density * h.centroid[k])
            .sum::<f32>()
            / mass
    });
    let mut i = [[0.0f32; 3]; 3];
    for h in hulls {
        let m = h.volume * density;
        let d = [
            h.centroid[0] - com[0],
            h.centroid[1] - com[1],
            h.centroid[2] - com[2],
        ];
        let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        for x in 0..3 {
            for y in 0..3 {
                // Parallel axis: I + m (|d|^2 1 - d d^T).
                i[x][y] +=
                    h.inertia[x][y] * density + m * ((if x == y { dd } else { 0.0 }) - d[x] * d[y]);
            }
        }
    }
    (mass, com, i)
}

/// One convex piece holding every corner of `hulls`: the intersection of
/// the pieces' own face planes that hold all the corners and the 26
/// planes of the corners' axis, edge and corner directions, each moved
/// out to the farthest corner. It is not the exact hull (a face of the
/// hull that no piece has is replaced by the nearest of those
/// directions), but it is convex, holds all the solid space, and is
/// tight where the pieces' own faces are. For a body with more pieces
/// than [`MAX_PIECES`].
pub fn enclosing(hulls: &[Hull]) -> Option<Hull> {
    let pts: Vec<V> = hulls
        .iter()
        .flat_map(|h| h.vertices.iter())
        .map(|p| [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])])
        .collect();
    if pts.is_empty() {
        return None;
    }
    let support = |n: V| {
        pts.iter()
            .map(|p| dot(n, *p))
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let mut planes: Vec<(V, f64)> = Vec::new();
    for x in -1i32..=1 {
        for y in -1i32..=1 {
            for z in -1i32..=1 {
                if (x, y, z) == (0, 0, 0) {
                    continue;
                }
                let n = [f64::from(x), f64::from(y), f64::from(z)];
                let l = dot(n, n).sqrt();
                let n = [n[0] / l, n[1] / l, n[2] / l];
                planes.push((n, -support(n)));
            }
        }
    }
    for h in hulls {
        for pl in &h.planes {
            let n = [f64::from(pl[0]), f64::from(pl[1]), f64::from(pl[2])];
            let w = f64::from(pl[3]);
            if pts.iter().all(|p| dot(n, *p) + w <= EPS) {
                planes.push((n, w));
            }
        }
    }
    hull(&planes)
}

/// The most convex pieces one rigid body gets; a body with more is one
/// [`enclosing`] piece per node instead.
pub const MAX_PIECES: usize = 64;
