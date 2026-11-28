//! Placed solid scenery as level collision.
//!
//! The engine never gives scenery a Havok body (`0x001c35f0` returns at
//! `0x001c361c` unless the object type is in mask `0x1883`: biped,
//! vehicle, machine, crate, creature), so a converted Halo 1 rock drawn as
//! a scenery object is walked through. Retail Halo 2 builds its rocks into
//! the structure. So does the conversion: each placed scenery object with
//! a collision model has that collision moved to its placement
//! ([`transformed`]) and merged into the structure's collision BSP
//! ([`merge`]). The rock still draws as its scenery object; its surfaces
//! join the level's, so the structure's MOPP code (built over every
//! structure surface, `mopp::build_structure`) puts them in front of
//! Havok, and the BSP's own tests (bullets, projectiles, a biped's ground
//! checks) find them through the 3D tree.
//!
//! **Moving a BSP.** Points and planes move rigidly. A 2D tree is written
//! in its reference plane's projection (drop the dominant normal axis;
//! [`crate::surface_query::project`]), which a rotation changes, so every
//! 2D line is carried over exactly: a point of the moved plane in the new
//! projection lifts to 3D, moves back to object space and projects the
//! old way, an affine map, so the old line's value is an affine function
//! of the new coordinates. Its sign, and so every walk, is unchanged; the
//! line is renormalised for the engine's distance tests.
//!
//! **Merging.** A BSP's solid space is the union of its solid leaves'
//! cells, so two BSPs are united by putting the piece's tree in place of
//! each of the level's open leaves the piece's solid reaches: inside that
//! leaf's cell, the piece decides; everywhere else the level does. One
//! copy of the piece's tree serves every such leaf, each reaching it
//! through six nodes testing the piece's bounding box whose outside keeps
//! that level leaf, so a piece whose tree calls unbounded space solid (an
//! open mesh) cannot reach past its box, and the merge grows the level by
//! one tree and six nodes a leaf. An open leaf of the copy stands in part
//! of the level, so it carries, with its own 2D references, those of each
//! level leaf it stands in whose planes its cell there touches: a ray
//! leaving the piece's open space into the level's solid looks its surface
//! up in the leaf it leaves (`0x001dea00`, docs/formats/collision.md,
//! "What the engine does with a BSP"), which tries every reference on the
//! crossed plane and keeps a surface only when the point is on its
//! polygon. Cells are clipped convex polyhedra in `f64`.

use std::collections::HashMap;

use tags::collision::{Bsp, Bsp2dRef, Child, Child2d, Leaf, Node2d, Node3d, Plane};

use crate::surface_query::{PROJECTION, dominant_axis};
use crate::{Error, Result};

/// A rigid transform `p' = r p + t`, `r` row-major acting on column
/// vectors (as `h1_assets::models::mode::world_transforms` gives a node's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub r: [[f32; 3]; 3],
    pub t: [f32; 3],
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

fn wide(v: [f32; 3]) -> V {
    v.map(f64::from)
}

fn mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

impl Pose {
    pub const IDENTITY: Self = Self {
        r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };

    /// A scenario placement's: position, and yaw, pitch and roll in
    /// radians as `Rz(yaw) · Ry(−pitch) · Rx(roll)` (forward `+x`, pitch
    /// tips it up, roll turns `+y` up), the rotation the placement record
    /// (`+0x14`) is drawn with (docs/live/live-art.md, checked on the disc
    /// by retail machines landing on their levels). A converted Halo 1
    /// scenery placement keeps Halo 1's three angles there.
    pub fn placement(position: [f32; 3], [yaw, pitch, roll]: [f32; 3]) -> Self {
        let (sy, cy) = f64::from(yaw).sin_cos();
        let (sp, cp) = f64::from(pitch).sin_cos();
        let (sr, cr) = f64::from(roll).sin_cos();
        let rz = [[cy, -sy, 0.0], [sy, cy, 0.0], [0.0, 0.0, 1.0]];
        let ry = [[cp, 0.0, -sp], [0.0, 1.0, 0.0], [sp, 0.0, cp]];
        let rx = [[1.0, 0.0, 0.0], [0.0, cr, -sr], [0.0, sr, cr]];
        let r = mul(mul(rz, ry), rx);
        Self {
            r: r.map(|row| row.map(|v| v as f32)),
            t: position,
        }
    }

    fn r64(&self) -> [[f64; 3]; 3] {
        self.r.map(wide)
    }

    /// `self` after `inner`: `p ↦ self(inner(p))`.
    pub fn then(&self, inner: &Pose) -> Pose {
        let r = mul(self.r64(), inner.r64());
        let t = self.point(inner.t);
        Pose {
            r: r.map(|row| row.map(|v| v as f32)),
            t: t.map(|v| v as f32),
        }
    }

    fn vector(&self, v: [f32; 3]) -> V {
        let r = self.r64();
        let v = wide(v);
        std::array::from_fn(|a| dot(r[a], v))
    }

    /// `p` moved.
    pub fn point(&self, p: [f32; 3]) -> V {
        let v = self.vector(p);
        std::array::from_fn(|a| v[a] + f64::from(self.t[a]))
    }

    /// World point `p` back in object space (`r` is a rotation).
    fn inverse_point(&self, p: V) -> V {
        let r = self.r64();
        let d: V = std::array::from_fn(|a| p[a] - f64::from(self.t[a]));
        std::array::from_fn(|a| (0..3).map(|k| r[k][a] * d[k]).sum())
    }
}

/// The two axes of `plane`'s 2D projection with `flip`, and the dropped
/// one (the engine's table, [`PROJECTION`]).
fn axes(n: V, flip: bool) -> (usize, usize, usize) {
    let n32 = n.map(|v| v as f32);
    let k = dominant_axis(n32);
    let sign = (n32[k] > 0.0) != flip;
    let [u, v] = PROJECTION[k][usize::from(sign)];
    (u, v, k)
}

/// A 2D line carried from `old` (object space) to `new` (world): the map
/// from the new projection to the old is affine, so the old line's value
/// is too.
struct Carry {
    pose: Pose,
    old: (usize, usize),
    new: (usize, usize, usize),
    normal: V,
    d: f64,
}

impl Carry {
    fn line(&self, l: [f32; 3]) -> Result<[f32; 3]> {
        let (nu, nv, nk) = self.new;
        let value = |u: f64, v: f64| {
            let mut x = [0.0; 3];
            x[nu] = u;
            x[nv] = v;
            x[nk] = (self.d - self.normal[nu] * u - self.normal[nv] * v) / self.normal[nk];
            let o = self.pose.inverse_point(x);
            f64::from(l[0]) * o[self.old.0] + f64::from(l[1]) * o[self.old.1] - f64::from(l[2])
        };
        let c0 = value(0.0, 0.0);
        let (a, b) = (value(1.0, 0.0) - c0, value(0.0, 1.0) - c0);
        let len = (a * a + b * b).sqrt();
        if !(len.is_finite() && len > 1e-9) {
            return Err(Error::Invalid("a 2D line collapses when its BSP is moved"));
        }
        Ok([(a / len) as f32, (b / len) as f32, (-c0 / len) as f32])
    }
}

/// `bsp` moved by `pose`: vertices and planes moved, every reference's 2D
/// tree carried into its plane's new projection (a tree reached from two
/// planes or flips is copied for each).
pub fn transformed(bsp: &Bsp, pose: &Pose) -> Result<Bsp> {
    let mut out = bsp.clone();
    for v in &mut out.vertices {
        v.point = pose.point(v.point).map(|c| c as f32);
    }
    for p in &mut out.planes {
        let [x, y, z, d] = p.0;
        // Renormalised: a model's default pose is rotations of stored
        // quaternions, unit only to their precision.
        let n = pose.vector([x, y, z]);
        let len = dot(n, n).sqrt();
        let n = if len > 0.0 { n.map(|c| c / len) } else { n };
        let d = f64::from(d) + dot(n, wide(pose.t));
        *p = Plane([n[0] as f32, n[1] as f32, n[2] as f32, d as f32]);
    }
    out.nodes_2d.clear();
    let mut done: HashMap<(u16, u16), u16> = HashMap::new();
    for r in &mut out.refs {
        let Child2d::Node(root) = r.target() else {
            continue;
        };
        let plane = usize::from(r.plane & 0x7fff);
        let flip = r.plane & 0x8000 != 0;
        let (Some(old), Some(new)) = (bsp.planes.get(plane), out.planes.get(plane)) else {
            return Err(Error::Invalid("a 2D reference names no plane"));
        };
        let key = (root, r.plane);
        let at = match done.get(&key) {
            Some(&at) => at,
            None => {
                let (ou, ov, _) = axes(wide([old.0[0], old.0[1], old.0[2]]), flip);
                let n = wide([new.0[0], new.0[1], new.0[2]]);
                let carry = Carry {
                    pose: *pose,
                    old: (ou, ov),
                    new: axes(n, flip),
                    normal: n,
                    d: f64::from(new.0[3]),
                };
                let at = carry_2d(bsp, root, &carry, &mut out.nodes_2d, 0)?;
                done.insert(key, at);
                at
            }
        };
        r.node = Child2d::Node(at).raw();
    }
    Ok(out)
}

/// Copies the 2D subtree at `node` of `src` into `out`, each line carried.
fn carry_2d(
    src: &Bsp,
    node: u16,
    carry: &Carry,
    out: &mut Vec<Node2d>,
    depth: usize,
) -> Result<u16> {
    if depth > src.nodes_2d.len() {
        return Err(Error::Invalid("a 2D tree has a cycle"));
    }
    let n = *src
        .nodes_2d
        .get(usize::from(node))
        .ok_or(Error::Invalid("a 2D child names no node"))?;
    let at = out.len();
    if at > MAX_FLAGGED {
        return Err(too_big("2D node", at));
    }
    out.push(Node2d {
        plane: carry.line(n.plane)?,
        back: n.back,
        front: n.front,
    });
    let mut kid = |c: Child2d| -> Result<Child2d> {
        Ok(match c {
            Child2d::Node(k) => Child2d::Node(carry_2d(src, k, carry, out, depth + 1)?),
            s => s,
        })
    };
    let back = kid(n.back)?;
    let front = kid(n.front)?;
    out[at].back = back;
    out[at].front = front;
    Ok(at as u16)
}

/// Largest index a flagged `u16` holds (bit 15 is the flag).
const MAX_FLAGGED: usize = 0x7fff;
/// Largest index a plain `u16` holds (`0xffff` is none).
const MAX_INDEX: usize = 0xfffe;
/// Largest 3D node or leaf index (24-bit children, bit 23 the leaf flag).
const MAX_CHILD: usize = 0x7f_ffff;
/// The surface "none" (`0xffff` as a 2D child).
const NO_SURFACE: u16 = 0x7fff;
/// Surface flag bit 3: tested against the breakable-surface mask.
const BREAKABLE: u8 = 0x8;
/// How far a piece's gate box stands off its vertices (world units).
pub const BOX_MARGIN: f32 = 0.05;
/// Slack for a cell touching a plane (world units). Generous: a reference
/// carried into a cell it doesn't border costs a lookup, one missing
/// lets a ray through the level.
const TOUCH: f64 = 1e-2;
/// Slack for clipping cells.
const EPS: f64 = 1e-7;

fn too_big(what: &'static str, value: usize) -> Error {
    Error::TooBig {
        what,
        value: value as u32,
        max: match what {
            "2D node" | "plane" | "surface" => MAX_FLAGGED as u32,
            "3D node" | "leaf" => MAX_CHILD as u32,
            _ => MAX_INDEX as u32,
        },
    }
}

/// A convex cell: its faces, each a convex polygon.
#[derive(Clone, Debug)]
struct Cell {
    faces: Vec<Vec<V>>,
}

impl Cell {
    fn cube(lo: V, hi: V) -> Self {
        let c = |i: usize| -> V {
            [
                if i & 1 != 0 { hi[0] } else { lo[0] },
                if i & 2 != 0 { hi[1] } else { lo[1] },
                if i & 4 != 0 { hi[2] } else { lo[2] },
            ]
        };
        let f = |is: [usize; 4]| is.iter().map(|&i| c(i)).collect::<Vec<_>>();
        Self {
            faces: vec![
                f([0, 2, 6, 4]),
                f([1, 5, 7, 3]),
                f([0, 4, 5, 1]),
                f([2, 3, 7, 6]),
                f([0, 1, 3, 2]),
                f([4, 6, 7, 5]),
            ],
        }
    }

    fn is_empty(&self) -> bool {
        self.faces.len() < 4
    }

    fn vertices(&self) -> impl Iterator<Item = &V> {
        self.faces.iter().flatten()
    }

    /// The part on `n . p - d`'s front (`front`) or back.
    fn clip(&self, n: V, d: f64, front: bool) -> Self {
        let s = if front { 1.0 } else { -1.0 };
        let dist = |p: V| s * (dot(n, p) - d);
        let mut faces = Vec::new();
        let mut cut: Vec<V> = Vec::new();
        for f in &self.faces {
            let mut kept = Vec::new();
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                let (da, db) = (dist(a), dist(b));
                if da >= -EPS {
                    kept.push(a);
                    if da <= EPS {
                        cut.push(a);
                    }
                }
                if (da > EPS && db < -EPS) || (da < -EPS && db > EPS) {
                    let t = da / (da - db);
                    let p = std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
                    kept.push(p);
                    cut.push(p);
                }
            }
            if kept.len() >= 3 {
                faces.push(kept);
            }
        }
        if faces.is_empty() {
            return Self { faces };
        }
        // The cap: the cut points in order round their centre.
        if cut.len() >= 3 {
            let c: V =
                std::array::from_fn(|k| cut.iter().map(|p| p[k]).sum::<f64>() / cut.len() as f64);
            let any = if n[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            let u = cross(n, any);
            let w = cross(n, u);
            let angle = |p: &V| {
                let q = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
                dot(q, w).atan2(dot(q, u))
            };
            cut.sort_by(|a, b| angle(a).total_cmp(&angle(b)));
            cut.dedup_by(|a, b| (0..3).all(|k| (a[k] - b[k]).abs() <= 1e-9));
            if cut.len() >= 3 {
                faces.push(cut);
            }
        }
        Self { faces }
    }

    /// Whether the cell reaches the plane `n . p - d = 0`.
    fn touches(&self, n: V, d: f64) -> bool {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in self.vertices() {
            let v = dot(n, *p) - d;
            lo = lo.min(v);
            hi = hi.max(v);
        }
        lo <= TOUCH && hi >= -TOUCH
    }
}

/// One of the level's open leaves the piece's box reaches: the node and
/// side holding it, and the planes on its path that cut the box, with the
/// side taken.
struct Slot {
    node: usize,
    front: bool,
    leaf: usize,
    path: Vec<(V, f64, bool)>,
}

fn plane_of(b: &Bsp, i: usize) -> Result<(V, f64)> {
    let p = b
        .planes
        .get(i)
        .ok_or(Error::Invalid("a node names no plane"))?;
    Ok((wide([p.0[0], p.0[1], p.0[2]]), f64::from(p.0[3])))
}

fn slots(level: &Bsp, lo: V, hi: V) -> Result<Vec<Slot>> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    if !level.nodes.is_empty() {
        walk(level, 0, lo, hi, &mut path, &mut out, 0)?;
    }
    Ok(out)
}

fn walk(
    level: &Bsp,
    node: usize,
    lo: V,
    hi: V,
    path: &mut Vec<(V, f64, bool)>,
    out: &mut Vec<Slot>,
    depth: usize,
) -> Result<()> {
    if depth > level.nodes.len() {
        return Err(Error::Invalid("the level's 3D tree has a cycle"));
    }
    let n = level
        .nodes
        .get(node)
        .ok_or(Error::Invalid("a 3D child names no node"))?;
    let (normal, d) = plane_of(level, usize::from(n.plane))?;
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..8 {
        let c = [
            if i & 1 != 0 { hi[0] } else { lo[0] },
            if i & 2 != 0 { hi[1] } else { lo[1] },
            if i & 4 != 0 { hi[2] } else { lo[2] },
        ];
        let v = dot(normal, c) - d;
        min = min.min(v);
        max = max.max(v);
    }
    for (front, child) in [(false, n.back), (true, n.front)] {
        let reaches = if front { max >= 0.0 } else { min < 0.0 };
        if !reaches {
            continue;
        }
        let cut = min < 0.0 && max >= 0.0;
        if cut {
            path.push((normal, d, front));
        }
        match child {
            Child::None => {}
            Child::Leaf(l) => out.push(Slot {
                node,
                front,
                leaf: l as usize,
                path: path.clone(),
            }),
            Child::Node(k) => walk(level, k as usize, lo, hi, path, out, depth + 1)?,
        }
        if cut {
            path.pop();
        }
    }
    Ok(())
}

/// What [`merge`] added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Merged {
    /// For each leaf added, in order, the level leaf whose space it stands
    /// in (its cluster is that leaf's).
    pub stand_ins: Vec<usize>,
    /// Level leaves the piece was put in.
    pub slots: usize,
}

/// Index offsets of the piece's blocks in the merged BSP.
#[derive(Clone, Copy)]
struct Base {
    planes: usize,
    nodes_2d: usize,
    surfaces: usize,
    edges: usize,
    vertices: usize,
    refs: usize,
}

fn shift_flagged(v: u16, by: usize) -> u16 {
    (v & 0x8000) | ((usize::from(v & 0x7fff) + by) as u16)
}

fn shift_2d(c: Child2d, b: Base) -> Child2d {
    match c {
        Child2d::Node(n) => Child2d::Node((usize::from(n) + b.nodes_2d) as u16),
        Child2d::Surface(NO_SURFACE) => Child2d::Surface(NO_SURFACE),
        Child2d::Surface(s) => Child2d::Surface((usize::from(s) + b.surfaces) as u16),
    }
}

fn shift(v: u16, by: usize) -> u16 {
    if v == u16::MAX {
        v
    } else {
        (usize::from(v) + by) as u16
    }
}

/// What the one copy of a piece's tree must hold for every level leaf it
/// stands in: per piece leaf, the level references its cells touch and
/// the level leaf it first stands in.
struct Gather<'a> {
    level: &'a Bsp,
    piece: &'a Bsp,
    carried: Vec<Vec<Bsp2dRef>>,
    host_flags: Vec<u8>,
    stand_in: Vec<Option<usize>>,
}

impl Gather<'_> {
    /// Walks the piece's subtree `c` over `cell`, inside level leaf
    /// `host`; `true` when it reaches the piece's solid there.
    fn walk(&mut self, c: Child, cell: &Cell, host: usize, depth: usize) -> Result<bool> {
        if depth > self.piece.nodes.len() {
            return Err(Error::Invalid("a piece's 3D tree has a cycle"));
        }
        match c {
            Child::None => Ok(true),
            Child::Leaf(l) => {
                self.leaf(l as usize, cell, host)?;
                Ok(false)
            }
            Child::Node(n) => {
                let src = *self
                    .piece
                    .nodes
                    .get(n as usize)
                    .ok_or(Error::Invalid("a piece's 3D child names no node"))?;
                let (normal, d) = plane_of(self.piece, usize::from(src.plane))?;
                let mut solid = false;
                for (front, child) in [(false, src.back), (true, src.front)] {
                    let part = cell.clip(normal, d, front);
                    if !part.is_empty() {
                        solid |= self.walk(child, &part, host, depth + 1)?;
                    }
                }
                Ok(solid)
            }
        }
    }

    /// Piece leaf `l` reached over `cell` inside level leaf `host`: it
    /// carries each of the host's references whose plane `cell` touches.
    fn leaf(&mut self, l: usize, cell: &Cell, host: usize) -> Result<()> {
        if l >= self.piece.leaves.len() {
            return Err(Error::Invalid("a piece's leaf is missing"));
        }
        let h = *self
            .level
            .leaves
            .get(host)
            .ok_or(Error::Invalid("a level leaf is missing"))?;
        self.stand_in[l].get_or_insert(host);
        if h.ref_count == 0 {
            return Ok(());
        }
        let first = usize::from(h.first_ref);
        let refs = self
            .level
            .refs
            .get(first..first + usize::from(h.ref_count))
            .ok_or(Error::Invalid(
                "a level leaf's references run past the block",
            ))?;
        for r in refs {
            let (n, d) = plane_of(self.level, usize::from(r.plane & 0x7fff))?;
            if cell.touches(n, d) && !self.carried[l].contains(r) {
                self.carried[l].push(*r);
                self.host_flags[l] |= h.flags & 1;
            }
        }
        Ok(())
    }
}

/// Merges `piece` (in the level's space) into `level`: its planes, 2D
/// nodes, surfaces (material `material`, no breakable surface), edges,
/// vertices and references appended, one copy of its 3D tree, and in place
/// of each open leaf of the level that the piece's solid reaches, six
/// nodes testing the piece's box whose outside keeps that leaf and whose
/// inside is the copy. The copy's open leaves carry the references of
/// every such level leaf whose planes their cells touch (the matcher tries
/// each reference on the crossed plane and keeps a surface only when the
/// point is on its polygon, so one a leaf doesn't border costs a lookup,
/// no false hit). Nothing changes when the piece's solid reaches no open
/// leaf, or when it fails (an index past what Halo 2's packed fields
/// hold).
pub fn merge(level: &mut Bsp, piece: &Bsp, material: u16) -> Result<Merged> {
    merge_materials(level, piece, |_| material)
}

/// Preserve the surface indices after the caller resolves the source material palette.
pub(crate) fn merge_materials(
    level: &mut Bsp,
    piece: &Bsp,
    material: impl Fn(u16) -> u16,
) -> Result<Merged> {
    if piece.nodes.is_empty() || piece.surfaces.is_empty() || piece.vertices.is_empty() {
        return Ok(Merged::default());
    }
    crate::collision::check_bounds(piece)?;
    let base = Base {
        planes: level.planes.len(),
        nodes_2d: level.nodes_2d.len(),
        surfaces: level.surfaces.len(),
        edges: level.edges.len(),
        vertices: level.vertices.len(),
        refs: level.refs.len(),
    };
    let gate = base.planes + piece.planes.len();
    for (what, n, max) in [
        ("plane", gate + 6, MAX_FLAGGED + 1),
        (
            "2D node",
            base.nodes_2d + piece.nodes_2d.len(),
            MAX_FLAGGED + 1,
        ),
        // 0x7fff is the 2D tree's "no surface".
        ("surface", base.surfaces + piece.surfaces.len(), MAX_FLAGGED),
        ("edge", base.edges + piece.edges.len(), MAX_INDEX + 1),
        (
            "vertex",
            base.vertices + piece.vertices.len(),
            MAX_INDEX + 1,
        ),
        ("2D reference", base.refs + piece.refs.len(), MAX_INDEX + 1),
    ] {
        if n > max {
            return Err(too_big(what, n));
        }
    }
    let mut out = level.clone();
    out.planes.extend_from_slice(&piece.planes);
    out.nodes_2d.extend(piece.nodes_2d.iter().map(|n| Node2d {
        plane: n.plane,
        back: shift_2d(n.back, base),
        front: shift_2d(n.front, base),
    }));
    out.surfaces.extend(piece.surfaces.iter().map(|s| {
        let mut s = *s;
        s.plane = shift_flagged(s.plane, base.planes);
        s.first_edge = shift(s.first_edge, base.edges);
        s.flags &= !BREAKABLE;
        s.breakable_surface = 0xff;
        s.material = material(s.material);
        s
    }));
    out.edges.extend(piece.edges.iter().map(|e| {
        let mut e = *e;
        e.start_vertex = shift(e.start_vertex, base.vertices);
        e.end_vertex = shift(e.end_vertex, base.vertices);
        e.forward_edge = shift(e.forward_edge, base.edges);
        e.reverse_edge = shift(e.reverse_edge, base.edges);
        e.left_surface = shift(e.left_surface, base.surfaces);
        e.right_surface = shift(e.right_surface, base.surfaces);
        e
    }));
    out.vertices.extend(piece.vertices.iter().map(|v| {
        let mut v = *v;
        v.edge = v.edge.wrapping_add(base.edges as u32);
        v
    }));
    out.refs.extend(piece.refs.iter().map(|r| Bsp2dRef {
        plane: shift_flagged(r.plane, base.planes),
        node: if r.node == u16::MAX {
            r.node
        } else {
            shift_2d(r.target(), base).raw()
        },
    }));

    // The gate box: inside is every plane's back.
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for v in &piece.vertices {
        for a in 0..3 {
            lo[a] = lo[a].min(f64::from(v.point[a]));
            hi[a] = hi[a].max(f64::from(v.point[a]));
        }
    }
    let m = f64::from(BOX_MARGIN);
    let (lo, hi): (V, V) = (lo.map(|v| v - m), hi.map(|v| v + m));
    for a in 0..3 {
        let mut n = [0.0f32; 3];
        n[a] = 1.0;
        out.planes.push(Plane([n[0], n[1], n[2], hi[a] as f32]));
        n[a] = -1.0;
        out.planes.push(Plane([n[0], n[1], n[2], -lo[a] as f32]));
    }

    // The level leaves the piece's solid reaches, and what its copy's
    // leaves carry from them.
    let mut gather = Gather {
        level: &*level,
        piece,
        carried: vec![Vec::new(); piece.leaves.len()],
        host_flags: vec![0; piece.leaves.len()],
        stand_in: vec![None; piece.leaves.len()],
    };
    let mut kept = Vec::new();
    for slot in slots(&*level, lo, hi)? {
        let mut cell = Cell::cube(lo, hi);
        for (n, d, front) in &slot.path {
            cell = cell.clip(*n, *d, *front);
        }
        // A leaf of an earlier piece's shared copy is reached once per
        // gate in front of it: gathered on every path, gated once.
        if !cell.is_empty()
            && gather.walk(Child::Node(0), &cell, slot.leaf, 0)?
            && !kept
                .iter()
                .any(|k: &Slot| (k.node, k.front) == (slot.node, slot.front))
        {
            kept.push(slot);
        }
    }
    let Some(first_host) = kept.first().map(|s| s.leaf) else {
        return Ok(Merged::default());
    };

    // The copy: the piece's nodes and leaves, once.
    let (n0, l0) = (out.nodes.len(), out.leaves.len());
    let nodes_end = n0 + piece.nodes.len() + 6 * kept.len();
    if nodes_end > MAX_CHILD + 1 {
        return Err(too_big("3D node", nodes_end));
    }
    if l0 + piece.leaves.len() > MAX_CHILD + 1 {
        return Err(too_big("leaf", l0 + piece.leaves.len()));
    }
    let kid = |c: Child| -> Result<Child> {
        Ok(match c {
            Child::None => Child::None,
            Child::Node(k) if (k as usize) < piece.nodes.len() => Child::Node(k + n0 as u32),
            Child::Leaf(j) if (j as usize) < piece.leaves.len() => Child::Leaf(j + l0 as u32),
            _ => return Err(Error::Invalid("a piece's 3D child is out of range")),
        })
    };
    for n in &piece.nodes {
        out.nodes.push(Node3d {
            plane: (usize::from(n.plane) + base.planes) as u16,
            back: kid(n.back)?,
            front: kid(n.front)?,
        });
    }
    let mut merged = Merged::default();
    for (j, src) in piece.leaves.iter().enumerate() {
        let carried = &gather.carried[j];
        let leaf = if carried.is_empty() {
            Leaf {
                flags: src.flags,
                ref_count: src.ref_count,
                first_ref: if src.ref_count == 0 {
                    u16::MAX
                } else {
                    shift(src.first_ref, base.refs)
                },
            }
        } else {
            let first = usize::from(src.first_ref);
            let own = if src.ref_count == 0 {
                &[][..]
            } else {
                piece
                    .refs
                    .get(first..first + usize::from(src.ref_count))
                    .ok_or(Error::Invalid(
                        "a piece leaf's references run past the block",
                    ))?
            };
            let count = carried.len() + own.len();
            let count = u8::try_from(count).map_err(|_| Error::TooBig {
                what: "leaf 2D reference count",
                value: count as u32,
                max: 0xff,
            })?;
            let at = out.refs.len();
            if at + usize::from(count) > MAX_INDEX + 1 {
                return Err(too_big("2D reference", at + usize::from(count)));
            }
            out.refs.extend_from_slice(carried);
            out.refs.extend(own.iter().map(|r| Bsp2dRef {
                plane: shift_flagged(r.plane, base.planes),
                node: if r.node == u16::MAX {
                    r.node
                } else {
                    shift_2d(r.target(), base).raw()
                },
            }));
            Leaf {
                flags: src.flags | gather.host_flags[j],
                ref_count: count,
                first_ref: at as u16,
            }
        };
        out.leaves.push(leaf);
        merged
            .stand_ins
            .push(gather.stand_in[j].unwrap_or(first_host));
    }

    // Each kept level leaf behind the gate box.
    for slot in &kept {
        let first = out.nodes.len();
        for k in 0..6 {
            out.nodes.push(Node3d {
                plane: (gate + k) as u16,
                back: Child::Node((first + k + 1) as u32),
                front: Child::Leaf(slot.leaf as u32),
            });
        }
        out.nodes[first + 5].back = Child::Node(n0 as u32);
        let parent = &mut out.nodes[slot.node];
        let gated = Child::Node(first as u32);
        if slot.front {
            parent.front = gated;
        } else {
            parent.back = gated;
        }
    }
    merged.slots = kept.len();
    *level = out;
    Ok(merged)
}

#[cfg(test)]
#[path = "scenery_collision_tests.rs"]
mod tests;
