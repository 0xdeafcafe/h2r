//! Delete and dissolve.

use std::collections::BTreeSet;

use crate::error::MeshError;
use crate::ids::{EdgeId, FaceId, VertId};
use crate::mesh::{Mesh, Poly};
use crate::op::DeleteWhat;
use crate::select::SelectMode;

impl Mesh {
    pub(crate) fn delete(&mut self, what: DeleteWhat) -> Result<(), MeshError> {
        match what {
            DeleteWhat::Verts => {
                let verts = self.selected_verts();
                if verts.is_empty() {
                    return Err(MeshError::NothingSelected);
                }
                for v in verts {
                    self.kill_vert(v);
                }
            }
            DeleteWhat::Edges => {
                let edges = self.selected_edges();
                if edges.is_empty() {
                    return Err(MeshError::NothingSelected);
                }
                for e in edges {
                    if self.is_edge(e) {
                        self.kill_edge(e);
                    }
                }
            }
            DeleteWhat::Faces | DeleteWhat::OnlyFaces => {
                let faces = self.selected_faces();
                if faces.is_empty() {
                    return Err(MeshError::NothingSelected);
                }
                let mut edges = BTreeSet::new();
                let mut verts = BTreeSet::new();
                for &f in &faces {
                    edges.extend(self.face_edges(f));
                    verts.extend(self.face_verts(f));
                }
                for f in faces {
                    self.kill_face(f);
                }
                if what == DeleteWhat::Faces {
                    for e in edges {
                        self.kill_edge_if_wire(e);
                    }
                    for v in verts {
                        self.kill_vert_if_loose(v);
                    }
                }
            }
        }
        self.select.clear();
        Ok(())
    }

    /// `Dissolve`, by mode. Vert mode: each selected vert with exactly two
    /// edges leaves the faces round it and its edges join (V -1, E -1).
    /// Edge mode: each selected edge with exactly two different faces that
    /// run it in opposite directions joins them into one face when the
    /// result is a simple polygon (E -1, F -1). Face mode: the edges
    /// between two selected faces, as in edge mode. Elements that do not
    /// qualify are left alone. Selects the joined faces.
    pub(crate) fn dissolve(&mut self) -> Result<(), MeshError> {
        match self.select.mode {
            SelectMode::Vert => {
                let verts = self.selected_verts();
                if verts.is_empty() {
                    return Err(MeshError::NothingSelected);
                }
                let mut joined = Vec::new();
                for v in verts {
                    if self.is_vert(v) && self.vert_edges(v).len() == 2 {
                        if let Some(e) = self.join_edges_at(v)? {
                            joined.push(e);
                        }
                    }
                }
                // Joining at a vert kills its two edges, so with two
                // two-edge verts in a row the later join kills the edge
                // the earlier one made: only live edges select.
                let joined: Vec<EdgeId> = joined.into_iter().filter(|&e| self.is_edge(e)).collect();
                self.select_result(&[], &joined, &[]);
            }
            SelectMode::Edge | SelectMode::Face => {
                let edges: Vec<EdgeId> = if self.select.mode == SelectMode::Edge {
                    self.selected_edges()
                } else {
                    let faces: BTreeSet<FaceId> = self.selected_faces().into_iter().collect();
                    self.selected_edges()
                        .into_iter()
                        .filter(|&e| {
                            let around = self.edge_faces(e);
                            around.len() == 2 && around.iter().all(|f| faces.contains(f))
                        })
                        .collect()
                };
                if edges.is_empty() {
                    return Err(MeshError::NothingSelected);
                }
                let mut merged = BTreeSet::new();
                for e in edges {
                    if self.is_edge(e) {
                        if let Some(f) = self.join_faces_at(e)? {
                            merged.insert(f);
                        }
                    }
                }
                let merged: Vec<FaceId> = merged.into_iter().filter(|&f| self.is_face(f)).collect();
                self.select_result(&[], &[], &merged);
            }
        }
        Ok(())
    }

    /// Joins the two faces on `e` into the first. `None` when the edge does
    /// not have two different faces running it in opposite directions, or
    /// the joined polygon would repeat a corner.
    pub(crate) fn join_faces_at(&mut self, e: EdgeId) -> Result<Option<FaceId>, MeshError> {
        let loops = self.edge_loops(e);
        let [lf, lg] = loops.as_slice() else { return Ok(None) };
        let (lf, lg) = (*lf, *lg);
        let (f, g) = (self.loop_face(lf), self.loop_face(lg));
        if f == g || self.loop_vert(lf) == self.loop_vert(lg) {
            return Ok(None);
        }
        // f runs a -> b at lf, g runs b -> a at lg. The joined cycle is f
        // from b round to just before a, then g from a round to just
        // before b.
        let mut poly = Poly { verts: Vec::new(), corners: Vec::new(), ..self.face_poly(f) };
        poly.src = None;
        for start in [lf, lg] {
            let mut l = self.loop_next(start);
            while l != start {
                poly.verts.push(self.loop_vert(l));
                poly.corners.push(self.corner(l));
                l = self.loop_next(l);
            }
        }
        let distinct: BTreeSet<VertId> = poly.verts.iter().copied().collect();
        if distinct.len() != poly.verts.len() {
            return Ok(None);
        }
        self.kill_face(g);
        self.rebuild_face(f, &poly)?;
        self.kill_edge_if_wire(e);
        Ok(Some(f))
    }

    /// Removes a two-edge vert `v` from its faces and joins its edges.
    /// Returns the joining edge.
    pub(crate) fn join_edges_at(&mut self, v: VertId) -> Result<Option<EdgeId>, MeshError> {
        let edges = self.vert_edges(v);
        let [e1, e2] = edges.as_slice() else { return Ok(None) };
        let (u, w) = (self.other_end(*e1, v), self.other_end(*e2, v));
        for f in self.vert_faces(v) {
            let mut poly = self.face_poly(f);
            let keep: Vec<usize> = (0..poly.len()).filter(|&i| poly.verts[i] != v).collect();
            if keep.len() < 3 {
                // A triangle loses its corner: it collapses onto u-w.
                self.kill_face(f);
                continue;
            }
            poly.verts = keep.iter().map(|&i| poly.verts[i]).collect();
            poly.corners = keep.iter().map(|&i| poly.corners[i]).collect();
            poly.src = None;
            self.rebuild_face(f, &poly)?;
        }
        self.kill_vert(v);
        Ok(Some(self.add_edge(u, w)?))
    }
}
