//! The selection, its mode, and the walks that build selections: edge
//! loops and rings, linked, more and less.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ids::{BitSet, EdgeId, FaceId, VertId};
use crate::mesh::Mesh;

/// Which kind of element clicks and `MeshOp::Select` act on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectMode {
    #[default]
    Vert,
    Edge,
    Face,
}

/// How `MeshOp::Select` combines its ids with the selection.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectHow {
    #[default]
    Set,
    Add,
    Remove,
    Toggle,
}

/// Three bit sets and a mode. After every operation the selection is
/// flushed from the mode's kind, as Blender does: in vertex mode an edge
/// is selected when both its ends are and a face when all its corners are;
/// in edge mode the ends of selected edges are selected and a face when
/// all its edges are; in face mode the edges and corners of selected faces
/// are selected.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub(crate) mode: SelectMode,
    pub(crate) verts: BitSet,
    pub(crate) edges: BitSet,
    pub(crate) faces: BitSet,
}

impl Selection {
    pub fn mode(&self) -> SelectMode {
        self.mode
    }

    pub fn verts(&self) -> impl Iterator<Item = VertId> + '_ {
        self.verts.iter().map(|i| VertId(i as u32))
    }

    pub fn edges(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.edges.iter().map(|i| EdgeId(i as u32))
    }

    pub fn faces(&self) -> impl Iterator<Item = FaceId> + '_ {
        self.faces.iter().map(|i| FaceId(i as u32))
    }

    pub fn has_vert(&self, v: VertId) -> bool {
        self.verts.contains(v.index())
    }

    pub fn has_edge(&self, e: EdgeId) -> bool {
        self.edges.contains(e.index())
    }

    pub fn has_face(&self, f: FaceId) -> bool {
        self.faces.contains(f.index())
    }

    /// Selected elements of the mode's kind.
    pub fn len(&self) -> usize {
        match self.mode {
            SelectMode::Vert => self.verts.len(),
            SelectMode::Edge => self.edges.len(),
            SelectMode::Face => self.faces.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.verts.is_empty() && self.edges.is_empty() && self.faces.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.verts.clear();
        self.edges.clear();
        self.faces.clear();
    }

    /// The mode's set.
    pub(crate) fn base_mut(&mut self) -> &mut BitSet {
        match self.mode {
            SelectMode::Vert => &mut self.verts,
            SelectMode::Edge => &mut self.edges,
            SelectMode::Face => &mut self.faces,
        }
    }
}

impl Mesh {
    pub fn selection(&self) -> &Selection {
        &self.select
    }

    pub(crate) fn selected_verts(&self) -> Vec<VertId> {
        self.select.verts().filter(|&v| self.is_vert(v)).collect()
    }

    pub(crate) fn selected_edges(&self) -> Vec<EdgeId> {
        self.select.edges().filter(|&e| self.is_edge(e)).collect()
    }

    pub(crate) fn selected_faces(&self) -> Vec<FaceId> {
        self.select.faces().filter(|&f| self.is_face(f)).collect()
    }

    /// Makes the selection consistent with its mode (see `Selection`) and
    /// drops dead ids.
    pub(crate) fn flush_selection(&mut self) {
        let mode = self.select.mode;
        let mut verts = BitSet::default();
        let mut edges = BitSet::default();
        let mut faces = BitSet::default();
        match mode {
            SelectMode::Vert => {
                for v in self.selected_verts() {
                    verts.insert(v.index());
                }
                for e in self.edge_ids() {
                    let [a, b] = self.edge_verts(e);
                    if verts.contains(a.index()) && verts.contains(b.index()) {
                        edges.insert(e.index());
                    }
                }
                for f in self.face_ids() {
                    if self.face_verts(f).iter().all(|v| verts.contains(v.index())) {
                        faces.insert(f.index());
                    }
                }
            }
            SelectMode::Edge => {
                for e in self.selected_edges() {
                    edges.insert(e.index());
                    for v in self.edge_verts(e) {
                        verts.insert(v.index());
                    }
                }
                for f in self.face_ids() {
                    if self.face_edges(f).iter().all(|e| edges.contains(e.index())) {
                        faces.insert(f.index());
                    }
                }
            }
            SelectMode::Face => {
                for f in self.selected_faces() {
                    faces.insert(f.index());
                    for l in self.face_loops(f) {
                        verts.insert(self.loop_vert(l).index());
                        edges.insert(self.loop_edge(l).index());
                    }
                }
            }
        }
        self.select = Selection { mode, verts, edges, faces };
    }

    /// Changes mode, converting the selection: to a coarser kind, an
    /// element is selected when all its parts are.
    pub(crate) fn set_mode(&mut self, mode: SelectMode) {
        // The flushed sets already hold every kind; re-flushing from the
        // new mode's set converts as Blender does.
        self.flush_selection();
        self.select.mode = mode;
        self.flush_selection();
    }

    /// Replaces the selection with these elements, in `mode`.
    pub(crate) fn select_only(
        &mut self,
        mode: SelectMode,
        verts: &[VertId],
        edges: &[EdgeId],
        faces: &[FaceId],
    ) {
        self.select.clear();
        self.select.mode = mode;
        for v in verts {
            self.select.verts.insert(v.index());
        }
        for e in edges {
            self.select.edges.insert(e.index());
        }
        for f in faces {
            self.select.faces.insert(f.index());
        }
        self.flush_selection();
    }

    /// Applies ids of the mode's kind.
    pub(crate) fn select_ids(&mut self, ids: &[u32], how: SelectHow) {
        if how == SelectHow::Set {
            self.select.clear();
        }
        let base = self.select.base_mut();
        for &id in ids {
            let i = id as usize;
            match how {
                SelectHow::Set | SelectHow::Add => base.insert(i),
                SelectHow::Remove => base.remove(i),
                SelectHow::Toggle => {
                    let on = !base.contains(i);
                    base.set(i, on);
                }
            }
        }
        if how == SelectHow::Remove || how == SelectHow::Toggle {
            self.unflush_removed();
        }
        self.flush_selection();
    }

    /// Before flushing after a removal, drops the coarser kinds so the
    /// flush rebuilds them from the mode's set instead of keeping them.
    fn unflush_removed(&mut self) {
        match self.select.mode {
            SelectMode::Vert => {}
            SelectMode::Edge => self.select.verts.clear(),
            SelectMode::Face => {
                self.select.verts.clear();
                self.select.edges.clear();
            }
        }
    }

    pub(crate) fn select_all(&mut self) {
        let mode = self.select.mode;
        let verts: Vec<VertId> = self.vert_ids().collect();
        let edges: Vec<EdgeId> = self.edge_ids().collect();
        let faces: Vec<FaceId> = self.face_ids().collect();
        self.select_only(mode, &verts, &edges, &faces);
    }

    pub(crate) fn select_invert(&mut self) {
        let mode = self.select.mode;
        let (mut verts, mut edges, mut faces) = (Vec::new(), Vec::new(), Vec::new());
        match mode {
            SelectMode::Vert => verts = self.vert_ids().filter(|&v| !self.select.has_vert(v)).collect(),
            SelectMode::Edge => edges = self.edge_ids().filter(|&e| !self.select.has_edge(e)).collect(),
            SelectMode::Face => faces = self.face_ids().filter(|&f| !self.select.has_face(f)).collect(),
        }
        self.select_only(mode, &verts, &edges, &faces);
    }

    /// Grows (`more`) or shrinks the selection by one step of adjacency:
    /// across edges for vertices, across vertices for edges, across edges
    /// for faces.
    pub(crate) fn select_grow(&mut self, more: bool) {
        let mode = self.select.mode;
        let (mut verts, mut edges, mut faces) = (Vec::new(), Vec::new(), Vec::new());
        match mode {
            SelectMode::Vert => {
                for v in self.vert_ids() {
                    let on = self.select.has_vert(v);
                    let near: Vec<bool> =
                        self.vert_neighbors(v).into_iter().map(|n| self.select.has_vert(n)).collect();
                    let keep = if more { on || near.iter().any(|&x| x) } else { on && near.iter().all(|&x| x) };
                    if keep {
                        verts.push(v);
                    }
                }
            }
            SelectMode::Edge => {
                for e in self.edge_ids() {
                    let on = self.select.has_edge(e);
                    let near: Vec<bool> = self
                        .edge_verts(e)
                        .iter()
                        .flat_map(|&v| self.vert_edges(v))
                        .filter(|&x| x != e)
                        .map(|x| self.select.has_edge(x))
                        .collect();
                    let keep = if more { on || near.iter().any(|&x| x) } else { on && near.iter().all(|&x| x) };
                    if keep {
                        edges.push(e);
                    }
                }
            }
            SelectMode::Face => {
                for f in self.face_ids() {
                    let on = self.select.has_face(f);
                    let near: Vec<bool> =
                        self.face_neighbors(f).into_iter().map(|g| self.select.has_face(g)).collect();
                    let keep = if more { on || near.iter().any(|&x| x) } else { on && near.iter().all(|&x| x) };
                    if keep {
                        faces.push(f);
                    }
                }
            }
        }
        self.select_only(mode, &verts, &edges, &faces);
    }

    /// Selects everything connected (over edges) to the selection.
    pub(crate) fn select_linked(&mut self) {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<VertId> = self.selected_verts();
        while let Some(v) = stack.pop() {
            if !seen.insert(v) {
                continue;
            }
            stack.extend(self.vert_neighbors(v).into_iter().filter(|n| !seen.contains(n)));
        }
        let verts: Vec<VertId> = seen.into_iter().collect();
        let mode = self.select.mode;
        self.select_only(SelectMode::Vert, &verts, &[], &[]);
        self.set_mode(mode);
    }

    /// Selects the faces matching `pred`, adding to the selection.
    pub(crate) fn select_faces_where(&mut self, pred: impl Fn(&Mesh, FaceId) -> bool) {
        let mode = self.select.mode;
        let faces: Vec<FaceId> = self.face_ids().filter(|&f| pred(self, f)).collect();
        let mut verts = self.selected_verts();
        let mut edges = self.selected_edges();
        let mut all_faces = self.selected_faces();
        for &f in &faces {
            verts.extend(self.face_verts(f));
            edges.extend(self.face_edges(f));
        }
        all_faces.extend(faces);
        self.select_only(mode, &verts, &edges, &all_faces);
    }

    // ---- loops and rings ----

    /// The edge loop through `e`, in order. From each end it goes on
    /// through a vertex with exactly four edges, all with two faces, to
    /// the edge that shares no face with the current one. A boundary
    /// edge's loop follows the boundary while each vertex has exactly one
    /// other boundary edge. A loop stops at any other vertex (a pole, a
    /// three-face edge) and closes when it comes back.
    pub fn edge_loop(&self, e: EdgeId) -> Vec<EdgeId> {
        if !self.is_edge(e) {
            return Vec::new();
        }
        let boundary = self.edge_face_count(e) == 1;
        if self.edge_face_count(e) == 0 {
            return vec![e];
        }
        let [a, b] = self.edge_verts(e);
        let (forward, closed) = self.loop_walk(e, b, boundary);
        let mut out = Vec::new();
        if !closed {
            let (backward, _) = self.loop_walk(e, a, boundary);
            out.extend(backward.into_iter().rev());
        }
        out.push(e);
        out.extend(forward);
        out
    }

    fn loop_walk(&self, start: EdgeId, through: VertId, boundary: bool) -> (Vec<EdgeId>, bool) {
        let mut out = Vec::new();
        let mut seen = BTreeSet::from([start]);
        let (mut cur, mut v) = (start, through);
        loop {
            let next = if boundary { self.boundary_step(cur, v) } else { self.loop_step(cur, v) };
            let Some(next) = next else { return (out, false) };
            if next == start {
                return (out, true);
            }
            if !seen.insert(next) {
                return (out, false);
            }
            out.push(next);
            v = self.other_end(next, v);
            cur = next;
        }
    }

    fn loop_step(&self, cur: EdgeId, v: VertId) -> Option<EdgeId> {
        let edges = self.vert_edges(v);
        if edges.len() != 4 || edges.iter().any(|&e| self.edge_face_count(e) != 2) {
            return None;
        }
        let faces: BTreeSet<FaceId> = self.edge_faces(cur).into_iter().collect();
        let mut candidates = edges
            .into_iter()
            .filter(|&e| e != cur && self.edge_faces(e).iter().all(|f| !faces.contains(f)));
        let next = candidates.next()?;
        if candidates.next().is_some() { None } else { Some(next) }
    }

    pub(crate) fn boundary_step(&self, cur: EdgeId, v: VertId) -> Option<EdgeId> {
        let mut others =
            self.vert_edges(v).into_iter().filter(|&e| e != cur && self.edge_face_count(e) == 1);
        let next = others.next()?;
        if others.next().is_some() { None } else { Some(next) }
    }

    /// The edge ring through `e`: cross each quad to its opposite edge,
    /// both ways, until a face that is not a quad, an edge without exactly
    /// two faces, or the start.
    pub fn edge_ring(&self, e: EdgeId) -> Vec<EdgeId> {
        match self.ring(e) {
            Some(ring) => ring.edges.into_iter().map(|(e, _)| e).collect(),
            None => Vec::new(),
        }
    }

    /// The ring through `e`, with each edge's "from" end (all on the same
    /// side of the ring) and the quad between consecutive edges.
    pub(crate) fn ring(&self, e: EdgeId) -> Option<Ring> {
        if !self.is_edge(e) {
            return None;
        }
        let from = self.edge_verts(e)[0];
        let faces = self.edge_faces(e);
        let mut forward = Ring { edges: vec![(e, from)], quads: Vec::new(), closed: false };
        if faces.len() > 2 {
            return Some(forward);
        }
        let mut seen = BTreeSet::from([e]);
        if let Some(&f) = faces.first() {
            if self.ring_walk(e, from, f, &mut seen, &mut forward) {
                forward.closed = true;
                return Some(forward);
            }
        }
        let mut backward = Ring { edges: vec![(e, from)], quads: Vec::new(), closed: false };
        if let Some(&g) = faces.get(1) {
            self.ring_walk(e, from, g, &mut seen, &mut backward);
        }
        // backward holds e first, then edges going away from it.
        let mut edges: Vec<(EdgeId, VertId)> = backward.edges.into_iter().skip(1).rev().collect();
        edges.extend(forward.edges);
        let mut quads: Vec<FaceId> = backward.quads.into_iter().rev().collect();
        quads.extend(forward.quads);
        Some(Ring { edges, quads, closed: false })
    }

    /// Walks the ring from `(e, from)` into face `f`. Appends to `ring`;
    /// returns whether it came back to `e`.
    fn ring_walk(
        &self,
        e: EdgeId,
        from: VertId,
        f: FaceId,
        seen: &mut BTreeSet<EdgeId>,
        ring: &mut Ring,
    ) -> bool {
        let (mut cur, mut cur_from, mut face) = (e, from, f);
        loop {
            if self.face_len(face) != 4 {
                return false;
            }
            let (next, next_from) = self.across_quad(face, cur, cur_from);
            if next == e {
                ring.quads.push(face);
                return true;
            }
            if !seen.insert(next) {
                return false;
            }
            ring.quads.push(face);
            ring.edges.push((next, next_from));
            let faces = self.edge_faces(next);
            if faces.len() != 2 {
                return false;
            }
            let Some(&other) = faces.iter().find(|&&g| g != face) else { return false };
            cur = next;
            cur_from = next_from;
            face = other;
        }
    }

    /// In quad `f`, the edge opposite `e`, and its end on the same side as
    /// `from`.
    pub(crate) fn across_quad(&self, f: FaceId, e: EdgeId, from: VertId) -> (EdgeId, VertId) {
        let verts = self.face_verts(f);
        let to = self.other_end(e, from);
        let i = verts.iter().position(|&v| v == from).expect("edge on the quad");
        let j = verts.iter().position(|&v| v == to).expect("edge on the quad");
        // The neighbour of `from` that is not `to`, and of `to` that is
        // not `from`.
        let side = |k: usize, not: VertId| {
            let next = verts[(k + 1) % 4];
            if next == not { verts[(k + 3) % 4] } else { next }
        };
        let (y, w) = (side(i, to), side(j, from));
        let opposite = self.find_edge(y, w).expect("quad sides are edges");
        (opposite, y)
    }
}

/// An edge ring: edges with their same-side end, the quads between, and
/// whether it closes (then the last quad leads back to the first edge).
#[derive(Clone, Debug, Default)]
pub(crate) struct Ring {
    pub(crate) edges: Vec<(EdgeId, VertId)>,
    pub(crate) quads: Vec<FaceId>,
    pub(crate) closed: bool,
}
