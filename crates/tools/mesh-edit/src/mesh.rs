//! The radial-edge mesh: arenas, queries, and the few primitives every
//! operation is made of.
//!
//! Four arenas of slots (`Option<T>`, `None` once deleted). A vertex names
//! one incident edge; its edges form the **disk cycle**, linked through
//! each edge's per-end `Disk`. A face names its first loop; its loops (its
//! corners) form a cycle through `next`/`prev`. An edge names one loop; the
//! loops of every face using the edge form its **radial ring** through
//! `radial_next`/`radial_prev`. A ring may hold one loop (a boundary edge),
//! two (manifold) or more (three faces on one edge, as Halo 2's BSP
//! clusters have), and faces on one edge need not agree on direction.
//!
//! Operations are written with four primitives: `add_face` and
//! `rebuild_face` (which make the edges they need), `kill_face`, and
//! `kill_edge`/`kill_vert`. The links are maintained by those alone.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::attrs::{Corner, Skin};
use crate::error::MeshError;
use crate::ids::{EdgeId, FaceId, LoopId, VertId};
use crate::io::Imported;
use crate::math::{self, Vec3};
use crate::select::Selection;

/// Edge flags.
pub mod edge_flags {
    pub const SEAM: u8 = 1;
    pub const SHARP: u8 = 2;
}

/// Face flags.
pub mod face_flags {
    /// The face moved or is new: its lightmap texels no longer fit it.
    pub const LIGHTMAP_STALE: u8 = 1;
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Vert {
    pub(crate) co: Vec3,
    pub(crate) e: Option<EdgeId>,
    pub(crate) skin: Option<Skin>,
    pub(crate) flags: u8,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Disk {
    pub(crate) prev: EdgeId,
    pub(crate) next: EdgeId,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Edge {
    pub(crate) v: [VertId; 2],
    pub(crate) disk: [Disk; 2],
    pub(crate) l: Option<LoopId>,
    pub(crate) flags: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Loop {
    pub(crate) v: VertId,
    pub(crate) e: EdgeId,
    pub(crate) f: FaceId,
    pub(crate) next: LoopId,
    pub(crate) prev: LoopId,
    pub(crate) radial_next: LoopId,
    pub(crate) radial_prev: LoopId,
    pub(crate) attrs: Corner,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Face {
    pub(crate) l: LoopId,
    pub(crate) len: u32,
    pub(crate) material: u16,
    pub(crate) group: u16,
    pub(crate) flags: u8,
    /// The source triangle this face was imported from.
    pub(crate) src: Option<u32>,
}

/// A face as plain data: corners in winding order (counter-clockwise seen
/// from the front), their attributes, and the face's fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Poly {
    pub verts: Vec<VertId>,
    pub corners: Vec<Corner>,
    pub material: u16,
    pub group: u16,
    pub flags: u8,
    pub src: Option<u32>,
}

impl Poly {
    /// A polygon with default corners, material and group 0.
    pub fn new(verts: Vec<VertId>) -> Poly {
        let corners = vec![Corner::default(); verts.len()];
        Poly { verts, corners, material: 0, group: 0, flags: 0, src: None }
    }

    pub fn len(&self) -> usize {
        self.verts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.verts.is_empty()
    }

    /// Wound the other way; each corner keeps its attributes, normals turn
    /// round.
    pub fn reversed(&self) -> Poly {
        let mut out = self.clone();
        out.verts.reverse();
        out.corners.reverse();
        for c in &mut out.corners {
            c.normal = math::scale(c.normal, -1.0);
            c.dirty |= crate::attrs::dirty::NORMAL | crate::attrs::dirty::FRAME;
        }
        out
    }
}

/// An editable mesh and its selection. See the crate docs.
///
/// Query methods panic on a dead id; `is_vert` and friends check first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub(crate) verts: Vec<Option<Vert>>,
    pub(crate) edges: Vec<Option<Edge>>,
    pub(crate) loops: Vec<Option<Loop>>,
    pub(crate) faces: Vec<Option<Face>>,
    pub(crate) select: Selection,
    /// The import this mesh exports against (`io.rs`).
    pub(crate) imported: Option<Arc<Imported>>,
}

fn live<T>(slots: &[Option<T>]) -> impl Iterator<Item = (usize, &T)> + '_ {
    slots.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|t| (i, t)))
}

impl Mesh {
    pub fn new() -> Mesh {
        Mesh::default()
    }

    // ---- counts, iteration, liveness ----

    pub fn vert_count(&self) -> usize {
        self.verts.iter().flatten().count()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.iter().flatten().count()
    }

    pub fn face_count(&self) -> usize {
        self.faces.iter().flatten().count()
    }

    /// `(verts, edges, faces)` alive.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.vert_count(), self.edge_count(), self.face_count())
    }

    pub fn vert_ids(&self) -> impl Iterator<Item = VertId> + '_ {
        live(&self.verts).map(|(i, _)| VertId(i as u32))
    }

    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        live(&self.edges).map(|(i, _)| EdgeId(i as u32))
    }

    pub fn face_ids(&self) -> impl Iterator<Item = FaceId> + '_ {
        live(&self.faces).map(|(i, _)| FaceId(i as u32))
    }

    pub(crate) fn loop_ids(&self) -> impl Iterator<Item = LoopId> + '_ {
        live(&self.loops).map(|(i, _)| LoopId(i as u32))
    }

    pub fn is_vert(&self, v: VertId) -> bool {
        self.verts.get(v.index()).is_some_and(Option::is_some)
    }

    pub fn is_edge(&self, e: EdgeId) -> bool {
        self.edges.get(e.index()).is_some_and(Option::is_some)
    }

    pub fn is_face(&self, f: FaceId) -> bool {
        self.faces.get(f.index()).is_some_and(Option::is_some)
    }

    /// Slots ever allocated, live or dead: the next ids to be handed out.
    pub fn id_marks(&self) -> (u32, u32, u32) {
        (self.verts.len() as u32, self.edges.len() as u32, self.faces.len() as u32)
    }

    // ---- internal accessors ----

    pub(crate) fn v(&self, v: VertId) -> &Vert {
        self.verts[v.index()].as_ref().unwrap_or_else(|| panic!("dead vertex {v:?}"))
    }

    pub(crate) fn v_mut(&mut self, v: VertId) -> &mut Vert {
        self.verts[v.index()].as_mut().unwrap_or_else(|| panic!("dead vertex {v:?}"))
    }

    pub(crate) fn e(&self, e: EdgeId) -> &Edge {
        self.edges[e.index()].as_ref().unwrap_or_else(|| panic!("dead edge {e:?}"))
    }

    pub(crate) fn e_mut(&mut self, e: EdgeId) -> &mut Edge {
        self.edges[e.index()].as_mut().unwrap_or_else(|| panic!("dead edge {e:?}"))
    }

    pub(crate) fn l(&self, l: LoopId) -> &Loop {
        self.loops[l.index()].as_ref().unwrap_or_else(|| panic!("dead loop {l:?}"))
    }

    pub(crate) fn l_mut(&mut self, l: LoopId) -> &mut Loop {
        self.loops[l.index()].as_mut().unwrap_or_else(|| panic!("dead loop {l:?}"))
    }

    pub(crate) fn f(&self, f: FaceId) -> &Face {
        self.faces[f.index()].as_ref().unwrap_or_else(|| panic!("dead face {f:?}"))
    }

    pub(crate) fn f_mut(&mut self, f: FaceId) -> &mut Face {
        self.faces[f.index()].as_mut().unwrap_or_else(|| panic!("dead face {f:?}"))
    }

    // ---- vertices ----

    pub fn position(&self, v: VertId) -> Vec3 {
        self.v(v).co
    }

    pub fn skin(&self, v: VertId) -> Option<Skin> {
        self.v(v).skin
    }

    /// The edges at `v`, in disk order.
    pub fn vert_edges(&self, v: VertId) -> Vec<EdgeId> {
        let mut out = Vec::new();
        let Some(first) = self.v(v).e else { return out };
        let mut e = first;
        loop {
            out.push(e);
            e = self.disk_next(e, v);
            if e == first || out.len() > self.edges.len() {
                break;
            }
        }
        out
    }

    /// The corners at `v`.
    pub fn vert_loops(&self, v: VertId) -> Vec<LoopId> {
        let mut out = Vec::new();
        for e in self.vert_edges(v) {
            for l in self.edge_loops(e) {
                if self.l(l).v == v {
                    out.push(l);
                }
            }
        }
        out
    }

    /// The faces with a corner at `v`, each once.
    pub fn vert_faces(&self, v: VertId) -> Vec<FaceId> {
        let mut seen = BTreeSet::new();
        self.vert_loops(v).into_iter().map(|l| self.l(l).f).filter(|&f| seen.insert(f)).collect()
    }

    /// The vertices joined to `v` by an edge.
    pub fn vert_neighbors(&self, v: VertId) -> Vec<VertId> {
        self.vert_edges(v).into_iter().map(|e| self.other_end(e, v)).collect()
    }

    // ---- edges ----

    pub fn edge_verts(&self, e: EdgeId) -> [VertId; 2] {
        self.e(e).v
    }

    pub fn other_end(&self, e: EdgeId, v: VertId) -> VertId {
        let [a, b] = self.e(e).v;
        if a == v { b } else { a }
    }

    pub fn edge_flags(&self, e: EdgeId) -> u8 {
        self.e(e).flags
    }

    /// The loops in `e`'s radial ring: one per face side using the edge.
    pub fn edge_loops(&self, e: EdgeId) -> Vec<LoopId> {
        let mut out = Vec::new();
        let Some(first) = self.e(e).l else { return out };
        let mut l = first;
        loop {
            out.push(l);
            l = self.l(l).radial_next;
            if l == first || out.len() > self.loops.len() {
                break;
            }
        }
        out
    }

    /// The faces using `e` (a face appears once per side it uses).
    pub fn edge_faces(&self, e: EdgeId) -> Vec<FaceId> {
        self.edge_loops(e).into_iter().map(|l| self.l(l).f).collect()
    }

    /// How many face sides use `e`: 0 wire, 1 boundary, 2 manifold, more
    /// non-manifold.
    pub fn edge_face_count(&self, e: EdgeId) -> usize {
        self.edge_loops(e).len()
    }

    /// The edge joining `a` and `b`.
    pub fn find_edge(&self, a: VertId, b: VertId) -> Option<EdgeId> {
        if !self.is_vert(a) || !self.is_vert(b) {
            return None;
        }
        self.vert_edges(a).into_iter().find(|&e| self.other_end(e, a) == b)
    }

    pub(crate) fn end(&self, e: EdgeId, v: VertId) -> usize {
        if self.e(e).v[0] == v { 0 } else { 1 }
    }

    pub(crate) fn disk_next(&self, e: EdgeId, v: VertId) -> EdgeId {
        self.e(e).disk[self.end(e, v)].next
    }

    // ---- loops ----

    pub fn loop_vert(&self, l: LoopId) -> VertId {
        self.l(l).v
    }

    pub fn loop_edge(&self, l: LoopId) -> EdgeId {
        self.l(l).e
    }

    pub fn loop_face(&self, l: LoopId) -> FaceId {
        self.l(l).f
    }

    pub fn loop_next(&self, l: LoopId) -> LoopId {
        self.l(l).next
    }

    pub fn loop_prev(&self, l: LoopId) -> LoopId {
        self.l(l).prev
    }

    pub fn corner(&self, l: LoopId) -> Corner {
        self.l(l).attrs
    }

    // ---- faces ----

    /// The face's loops in winding order, from its first.
    pub fn face_loops(&self, f: FaceId) -> Vec<LoopId> {
        let first = self.f(f).l;
        let mut out = vec![first];
        let mut l = self.l(first).next;
        while l != first && out.len() <= self.loops.len() {
            out.push(l);
            l = self.l(l).next;
        }
        out
    }

    pub fn face_len(&self, f: FaceId) -> usize {
        self.f(f).len as usize
    }

    pub fn face_verts(&self, f: FaceId) -> Vec<VertId> {
        self.face_loops(f).into_iter().map(|l| self.l(l).v).collect()
    }

    pub fn face_edges(&self, f: FaceId) -> Vec<EdgeId> {
        self.face_loops(f).into_iter().map(|l| self.l(l).e).collect()
    }

    pub fn face_corners(&self, f: FaceId) -> Vec<Corner> {
        self.face_loops(f).into_iter().map(|l| self.l(l).attrs).collect()
    }

    pub fn face_material(&self, f: FaceId) -> u16 {
        self.f(f).material
    }

    pub fn face_group(&self, f: FaceId) -> u16 {
        self.f(f).group
    }

    pub fn face_flags(&self, f: FaceId) -> u8 {
        self.f(f).flags
    }

    /// The source triangle the face was imported from, while it has one.
    pub fn face_src(&self, f: FaceId) -> Option<u32> {
        self.f(f).src
    }

    pub fn face_poly(&self, f: FaceId) -> Poly {
        let face = self.f(f);
        let loops = self.face_loops(f);
        Poly {
            verts: loops.iter().map(|&l| self.l(l).v).collect(),
            corners: loops.iter().map(|&l| self.l(l).attrs).collect(),
            material: face.material,
            group: face.group,
            flags: face.flags,
            src: face.src,
        }
    }

    pub fn face_positions(&self, f: FaceId) -> Vec<Vec3> {
        self.face_verts(f).into_iter().map(|v| self.position(v)).collect()
    }

    /// The unit normal of the winding (Newell's method).
    pub fn face_normal(&self, f: FaceId) -> Vec3 {
        math::normalize(math::newell(&self.face_positions(f)))
    }

    pub fn face_center(&self, f: FaceId) -> Vec3 {
        math::centroid(&self.face_positions(f))
    }

    /// The faces sharing an edge with `f`.
    pub fn face_neighbors(&self, f: FaceId) -> Vec<FaceId> {
        let mut seen = BTreeSet::from([f]);
        let mut out = Vec::new();
        for e in self.face_edges(f) {
            for g in self.edge_faces(e) {
                if seen.insert(g) {
                    out.push(g);
                }
            }
        }
        out
    }

    /// The face whose vertex cycle is `verts`, in either direction.
    pub fn find_face(&self, verts: &[VertId]) -> Option<FaceId> {
        if verts.len() < 3 {
            return None;
        }
        let e = self.find_edge(verts[0], verts[1])?;
        self.edge_faces(e).into_iter().find(|&f| {
            self.face_len(f) == verts.len() && same_cycle(&self.face_verts(f), verts)
        })
    }

    // ---- primitives ----

    pub fn add_vert(&mut self, co: Vec3, skin: Option<Skin>) -> VertId {
        let id = VertId(self.verts.len() as u32);
        self.verts.push(Some(Vert { co, e: None, skin, flags: 0 }));
        id
    }

    /// A copy of `v` (position, skin, flags), with no edges.
    pub(crate) fn copy_vert(&mut self, v: VertId) -> VertId {
        let (co, skin, flags) = { let x = self.v(v); (x.co, x.skin, x.flags) };
        let id = self.add_vert(co, skin);
        self.v_mut(id).flags = flags;
        id
    }

    /// The edge joining `a` and `b`, made if missing.
    pub fn add_edge(&mut self, a: VertId, b: VertId) -> Result<EdgeId, MeshError> {
        if !self.is_vert(a) || !self.is_vert(b) {
            return Err(MeshError::Dead);
        }
        if a == b {
            return Err(MeshError::Degenerate);
        }
        if let Some(e) = self.find_edge(a, b) {
            return Ok(e);
        }
        let e = EdgeId(self.edges.len() as u32);
        let unlinked = Disk { prev: e, next: e };
        self.edges.push(Some(Edge { v: [a, b], disk: [unlinked; 2], l: None, flags: 0 }));
        self.disk_link(e, a);
        self.disk_link(e, b);
        Ok(e)
    }

    fn disk_link(&mut self, e: EdgeId, v: VertId) {
        let end = self.end(e, v);
        match self.v(v).e {
            None => {
                self.e_mut(e).disk[end] = Disk { prev: e, next: e };
                self.v_mut(v).e = Some(e);
            }
            Some(first) => {
                let first_end = self.end(first, v);
                let last = self.e(first).disk[first_end].prev;
                let last_end = self.end(last, v);
                self.e_mut(e).disk[end] = Disk { prev: last, next: first };
                self.e_mut(last).disk[last_end].next = e;
                self.e_mut(first).disk[first_end].prev = e;
            }
        }
    }

    fn disk_unlink(&mut self, e: EdgeId, v: VertId) {
        let end = self.end(e, v);
        let Disk { prev, next } = self.e(e).disk[end];
        if next == e {
            self.v_mut(v).e = None;
            return;
        }
        let prev_end = self.end(prev, v);
        let next_end = self.end(next, v);
        self.e_mut(prev).disk[prev_end].next = next;
        self.e_mut(next).disk[next_end].prev = prev;
        if self.v(v).e == Some(e) {
            self.v_mut(v).e = Some(next);
        }
    }

    fn radial_link(&mut self, l: LoopId, e: EdgeId) {
        match self.e(e).l {
            None => {
                let lp = self.l_mut(l);
                lp.radial_next = l;
                lp.radial_prev = l;
                self.e_mut(e).l = Some(l);
            }
            Some(first) => {
                let last = self.l(first).radial_prev;
                let lp = self.l_mut(l);
                lp.radial_prev = last;
                lp.radial_next = first;
                self.l_mut(last).radial_next = l;
                self.l_mut(first).radial_prev = l;
            }
        }
    }

    fn radial_unlink(&mut self, l: LoopId) {
        let (e, prev, next) = { let x = self.l(l); (x.e, x.radial_prev, x.radial_next) };
        if next == l {
            self.e_mut(e).l = None;
            return;
        }
        self.l_mut(prev).radial_next = next;
        self.l_mut(next).radial_prev = prev;
        if self.e(e).l == Some(l) {
            self.e_mut(e).l = Some(next);
        }
    }

    /// Checks a polygon can become a face: live, distinct corners, at
    /// least three, matching attribute count, and no face but `except`
    /// with the same cycle.
    fn check_poly(&self, poly: &Poly, except: Option<FaceId>) -> Result<(), MeshError> {
        if poly.corners.len() != poly.verts.len() {
            return Err(MeshError::BadInput("corner count differs from vertex count".into()));
        }
        if poly.verts.len() < 3 {
            return Err(MeshError::Degenerate);
        }
        let mut seen = BTreeSet::new();
        for &v in &poly.verts {
            if !self.is_vert(v) {
                return Err(MeshError::Dead);
            }
            if !seen.insert(v) {
                return Err(MeshError::Degenerate);
            }
        }
        if let Some(found) = self.find_face(&poly.verts) {
            if Some(found) != except {
                return Err(MeshError::DuplicateFace);
            }
        }
        Ok(())
    }

    /// Adds a face, making the edges it needs.
    pub fn add_face(&mut self, poly: &Poly) -> Result<FaceId, MeshError> {
        self.check_poly(poly, None)?;
        let f = FaceId(self.faces.len() as u32);
        self.faces.push(Some(Face {
            l: LoopId(0),
            len: 0,
            material: poly.material,
            group: poly.group,
            flags: poly.flags,
            src: poly.src,
        }));
        self.link_loops(f, poly)?;
        Ok(f)
    }

    /// Replaces face `f`'s corners with `poly`'s, keeping its id. Edges
    /// the face no longer uses stay (possibly as wire edges). Refuses, with
    /// the face untouched, as `add_face` would.
    pub(crate) fn rebuild_face(&mut self, f: FaceId, poly: &Poly) -> Result<(), MeshError> {
        self.check_poly(poly, Some(f))?;
        for l in self.face_loops(f) {
            self.radial_unlink(l);
            self.loops[l.index()] = None;
        }
        {
            let face = self.f_mut(f);
            face.material = poly.material;
            face.group = poly.group;
            face.flags = poly.flags;
            face.src = poly.src;
        }
        self.link_loops(f, poly)
    }

    fn link_loops(&mut self, f: FaceId, poly: &Poly) -> Result<(), MeshError> {
        let n = poly.len();
        let mut edges = Vec::with_capacity(n);
        for i in 0..n {
            edges.push(self.add_edge(poly.verts[i], poly.verts[(i + 1) % n])?);
        }
        let base = self.loops.len() as u32;
        for i in 0..n {
            self.loops.push(Some(Loop {
                v: poly.verts[i],
                e: edges[i],
                f,
                next: LoopId(base + ((i + 1) % n) as u32),
                prev: LoopId(base + ((i + n - 1) % n) as u32),
                radial_next: LoopId(base + i as u32),
                radial_prev: LoopId(base + i as u32),
                attrs: poly.corners[i],
            }));
        }
        for (i, &e) in edges.iter().enumerate() {
            self.radial_link(LoopId(base + i as u32), e);
        }
        let face = self.f_mut(f);
        face.l = LoopId(base);
        face.len = n as u32;
        Ok(())
    }

    /// Removes a face. Its edges and vertices stay.
    pub(crate) fn kill_face(&mut self, f: FaceId) {
        for l in self.face_loops(f) {
            self.radial_unlink(l);
            self.loops[l.index()] = None;
        }
        self.faces[f.index()] = None;
        self.select.faces.remove(f.index());
    }

    /// Removes an edge and the faces on it. Its vertices stay.
    pub(crate) fn kill_edge(&mut self, e: EdgeId) {
        let faces: BTreeSet<FaceId> = self.edge_faces(e).into_iter().collect();
        for f in faces {
            self.kill_face(f);
        }
        let [a, b] = self.e(e).v;
        self.disk_unlink(e, a);
        self.disk_unlink(e, b);
        self.edges[e.index()] = None;
        self.select.edges.remove(e.index());
    }

    /// Removes a vertex with its edges and faces.
    pub(crate) fn kill_vert(&mut self, v: VertId) {
        for e in self.vert_edges(v) {
            self.kill_edge(e);
        }
        self.verts[v.index()] = None;
        self.select.verts.remove(v.index());
    }

    /// Removes `v` if no edge uses it.
    pub(crate) fn kill_vert_if_loose(&mut self, v: VertId) {
        if self.is_vert(v) && self.v(v).e.is_none() {
            self.kill_vert(v);
        }
    }

    /// Removes `e` if no face uses it.
    pub(crate) fn kill_edge_if_wire(&mut self, e: EdgeId) {
        if self.is_edge(e) && self.e(e).l.is_none() {
            self.kill_edge(e);
        }
    }

    pub(crate) fn set_position(&mut self, v: VertId, co: Vec3) {
        self.v_mut(v).co = co;
    }
}

/// Whether `a` and `b` are the same cycle, in either direction.
pub(crate) fn same_cycle(a: &[VertId], b: &[VertId]) -> bool {
    let n = a.len();
    if n != b.len() || n == 0 {
        return false;
    }
    let Some(start) = a.iter().position(|&v| v == b[0]) else { return false };
    let forward = (0..n).all(|k| a[(start + k) % n] == b[k]);
    let backward = (0..n).all(|k| a[(start + n - k) % n] == b[k]);
    forward || backward
}
