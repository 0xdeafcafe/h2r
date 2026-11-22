//! A solid added to a level's collision BSP by appending
//! (docs/editor/modeling-targets.md, "Structure collision", step 2):
//! every plane, surface, edge, vertex, 2D node, 2D reference and leaf the
//! level has keeps its index, so everything that holds one (pathfinding's
//! surface references, objects' leaves, Havok's cached keys, the per-leaf
//! cluster block) stays valid. This is the `append` mode of
//! `edit.collision.commit` (docs/editor/realtime-edits.md, "Collision").
//!
//! The method is `h1convert::scenery_collision::merge`'s, on this crate's
//! types and limits, without a dependency on the converter:
//!
//! - A BSP's solid space is the union of its solid leaves' cells, so the
//!   piece's tree goes in place of each open leaf of the level that the
//!   piece's solid reaches: inside that leaf's cell the piece decides,
//!   everywhere else the level does.
//! - One copy of the piece's tree serves every such leaf. Each reaches it
//!   through six nodes testing the piece's bounding box (grown by
//!   [`BOX_MARGIN`]) whose outside keeps the level leaf, so a piece whose
//!   tree calls unbounded space solid cannot reach past its box.
//! - An open leaf of the copy stands in part of the level, so it carries,
//!   besides its own references, those of each level leaf it stands in
//!   whose planes its cell there touches: a ray leaving the piece's open
//!   space into the level's solid looks its surface up in the leaf it
//!   leaves (`query::surface_at`), which tries every reference on the
//!   crossed plane and keeps a surface only when the point is on its
//!   polygon. Leaf flag bit 0 is set when the leaf's references then
//!   reach a two-sided surface, the compiler's rule.
//! - The piece's surfaces keep their material (an index into the level's
//!   collision materials) and flags but get no breakable surface: bit 3
//!   is cleared and the byte is [`NO_BREAKABLE`], since the structure's
//!   breakable-surface list has no entry for them.
//!
//! Cells are clipped convex polyhedra in `f64`.
//!
//! A copy that serves several level leaves carries every one's references,
//! so a reference can sit off one of the copy leaf's paths. The engine's
//! matcher only costs a lookup for it; [`crate::verify::check`]'s path
//! rule names it, and holds for a solid inside one level leaf.

use tags::collision::{Bsp, Bsp2dRef, Child, Child2d, Leaf, Node2d, Node3d, Plane};

use crate::{Error, Limits, NO_BREAKABLE, Result, TWO_SIDED, verify};

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
            let c: V = std::array::from_fn(|k| cut.iter().map(|p| p[k]).sum::<f64>() / cut.len() as f64);
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
        .ok_or_else(|| Error::Invalid("a node names no plane".into()))?;
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
        return Err(Error::Invalid("the level's 3D tree has a cycle".into()));
    }
    let n = level
        .nodes
        .get(node)
        .ok_or_else(|| Error::Invalid("a level 3D child names no node".into()))?;
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
    /// For each leaf added, in order (leaf `first_leaf + i`), the level
    /// leaf whose space it stands in first: an added leaf takes that
    /// leaf's cluster.
    pub stand_ins: Vec<usize>,
    /// For each leaf added, every level leaf it stands in, the first being
    /// its stand-in.
    pub hosts: Vec<Vec<usize>>,
    /// Index of the first added leaf (the level's leaf count before).
    pub first_leaf: usize,
    /// Index of the first added surface (the level's surface count before).
    pub first_surface: usize,
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

fn shift_ref(r: &Bsp2dRef, b: Base) -> Bsp2dRef {
    Bsp2dRef {
        plane: shift_flagged(r.plane, b.planes),
        node: if r.node == u16::MAX {
            r.node
        } else {
            shift_2d(r.target(), b).raw()
        },
    }
}

/// What the one copy of a piece's tree must hold for every level leaf it
/// stands in: per piece leaf, the level references its cells touch and
/// the level leaves it stands in, in the order first reached.
struct Gather<'a> {
    level: &'a Bsp,
    piece: &'a Bsp,
    carried: Vec<Vec<Bsp2dRef>>,
    hosts: Vec<Vec<usize>>,
}

impl Gather<'_> {
    /// Walks the piece's subtree `c` over `cell`, inside level leaf
    /// `host`; `true` when it reaches the piece's solid there.
    fn walk(&mut self, c: Child, cell: &Cell, host: usize, depth: usize) -> Result<bool> {
        if depth > self.piece.nodes.len() {
            return Err(Error::Invalid("the solid's 3D tree has a cycle".into()));
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
                    .ok_or_else(|| Error::Invalid("the solid's 3D child names no node".into()))?;
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
            return Err(Error::Invalid("a leaf of the solid is missing".into()));
        }
        let h = *self
            .level
            .leaves
            .get(host)
            .ok_or_else(|| Error::Invalid("a level leaf is missing".into()))?;
        if !self.hosts[l].contains(&host) {
            self.hosts[l].push(host);
        }
        if h.ref_count == 0 {
            return Ok(());
        }
        let first = usize::from(h.first_ref);
        let refs = self
            .level
            .refs
            .get(first..first + usize::from(h.ref_count))
            .ok_or_else(|| Error::Invalid("a level leaf's references run past the block".into()))?;
        for r in refs {
            let (n, d) = plane_of(self.level, usize::from(r.plane & 0x7fff))?;
            if cell.touches(n, d) && !self.carried[l].contains(r) {
                self.carried[l].push(*r);
            }
        }
        Ok(())
    }
}

/// Merges `piece` (in the level's space, its materials the level's) into
/// `level`, as the module docs describe. Nothing changes when the piece's
/// solid reaches no open leaf (`slots` is 0), or when it fails (an index
/// past what [`Limits::HALO2`] holds, or a malformed tree).
pub fn merge(level: &mut Bsp, piece: &Bsp) -> Result<Merged> {
    merge_with(level, piece, &Limits::HALO2)
}

/// [`merge`] with other limits.
pub fn merge_with(level: &mut Bsp, piece: &Bsp, limits: &Limits) -> Result<Merged> {
    if piece.nodes.is_empty() || piece.surfaces.is_empty() || piece.vertices.is_empty() {
        return Ok(Merged::default());
    }
    let base = Base {
        planes: level.planes.len(),
        nodes_2d: level.nodes_2d.len(),
        surfaces: level.surfaces.len(),
        edges: level.edges.len(),
        vertices: level.vertices.len(),
        refs: level.refs.len(),
    };
    let gate = base.planes + piece.planes.len();
    limits.check("plane", gate + 6)?;
    limits.check("2D node", base.nodes_2d + piece.nodes_2d.len())?;
    limits.check("surface", base.surfaces + piece.surfaces.len())?;
    limits.check("edge", base.edges + piece.edges.len())?;
    limits.check("vertex", base.vertices + piece.vertices.len())?;
    limits.check("2D reference", base.refs + piece.refs.len())?;

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
        s.breakable_surface = NO_BREAKABLE;
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
    out.refs.extend(piece.refs.iter().map(|r| shift_ref(r, base)));

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
        hosts: vec![Vec::new(); piece.leaves.len()],
    };
    let mut kept: Vec<Slot> = Vec::new();
    for slot in slots(level, lo, hi)? {
        let mut cell = Cell::cube(lo, hi);
        for (n, d, front) in &slot.path {
            cell = cell.clip(*n, *d, *front);
        }
        // A leaf behind an earlier merge's shared copy is reached once per
        // gate in front of it: gathered on every path, gated once.
        if !cell.is_empty()
            && gather.walk(Child::Node(0), &cell, slot.leaf, 0)?
            && !kept.iter().any(|k| (k.node, k.front) == (slot.node, slot.front))
        {
            kept.push(slot);
        }
    }
    let Some(first_host) = kept.first().map(|s| s.leaf) else {
        return Ok(Merged::default());
    };

    // The copy: the piece's nodes and leaves, once.
    let (n0, l0) = (out.nodes.len(), out.leaves.len());
    limits.check("3D node", n0 + piece.nodes.len() + 6 * kept.len())?;
    limits.check("leaf", l0 + piece.leaves.len())?;
    let kid = |c: Child| -> Result<Child> {
        Ok(match c {
            Child::None => Child::None,
            Child::Node(k) if (k as usize) < piece.nodes.len() => Child::Node(k + n0 as u32),
            Child::Leaf(j) if (j as usize) < piece.leaves.len() => Child::Leaf(j + l0 as u32),
            _ => return Err(Error::Invalid("a 3D child of the solid is out of range".into())),
        })
    };
    for n in &piece.nodes {
        out.nodes.push(Node3d {
            plane: (usize::from(n.plane) + base.planes) as u16,
            back: kid(n.back)?,
            front: kid(n.front)?,
        });
    }
    let mut merged = Merged {
        first_leaf: l0,
        first_surface: base.surfaces,
        ..Merged::default()
    };
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
                    .ok_or_else(|| Error::Invalid("a leaf of the solid has references past the block".into()))?
            };
            let count = carried.len() + own.len();
            limits.check("leaf 2D reference", count)?;
            let at = out.refs.len();
            limits.check("2D reference", at + count)?;
            out.refs.extend_from_slice(carried);
            out.refs.extend(own.iter().map(|r| shift_ref(r, base)));
            // The compiler's rule: bit 0 where a reference reaches a
            // two-sided surface.
            let two_sided = out.refs[at..at + count].iter().any(|r| {
                verify::reached(&out, r.target())
                    .into_iter()
                    .any(|s| out.surfaces.get(s).is_some_and(|s| s.flags & TWO_SIDED != 0))
            });
            Leaf {
                flags: (src.flags & !1) | u8::from(two_sided),
                ref_count: count as u8,
                first_ref: at as u16,
            }
        };
        out.leaves.push(leaf);
        let hosts = std::mem::take(&mut gather.hosts[j]);
        merged.stand_ins.push(hosts.first().copied().unwrap_or(first_host));
        merged.hosts.push(if hosts.is_empty() { vec![first_host] } else { hosts });
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
