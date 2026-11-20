//! Extrude faces and edges, and duplicate.

use std::collections::{BTreeMap, BTreeSet};

use crate::attrs::Corner;
use crate::error::MeshError;
use crate::ids::{EdgeId, FaceId, VertId};
use crate::math::Vec3;
use crate::mesh::{Mesh, Poly, face_flags};
use crate::ops::Affine;

impl Mesh {
    /// The copy of `v` in `map`, made on first use.
    pub(crate) fn copy_of(&mut self, map: &mut BTreeMap<VertId, VertId>, v: VertId) -> VertId {
        if let Some(&c) = map.get(&v) {
            return c;
        }
        let c = self.copy_vert(v);
        map.insert(v, c);
        c
    }

    /// Selects these elements and their parts, keeping the mode.
    pub(crate) fn select_result(&mut self, verts: &[VertId], edges: &[EdgeId], faces: &[FaceId]) {
        let mut all_verts = verts.to_vec();
        let mut all_edges = edges.to_vec();
        for &e in edges {
            all_verts.extend(self.edge_verts(e));
        }
        for &f in faces {
            all_verts.extend(self.face_verts(f));
            all_edges.extend(self.face_edges(f));
        }
        let mode = self.select.mode;
        self.select_only(mode, &all_verts, &all_edges, faces);
    }

    /// `ExtrudeFaces`. `S` is the selected faces and `B` their border: the
    /// edges with one selected face, or a selected face and an unselected
    /// one. Each vert on `B` is copied, `S` is rewired to the copies (its
    /// faces keep their ids), and a quad joins each edge of `B` to its
    /// copy. Interior verts of `S` stay `S`'s. Copies and interior verts
    /// then move by `offset`. Counts: V +|Vb|, E +|B| + |Vb|, F +|B|.
    pub(crate) fn extrude_faces(&mut self, offset: Vec3) -> Result<(), MeshError> {
        let s: BTreeSet<FaceId> = self.selected_faces().into_iter().collect();
        if s.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let mut s_edges = BTreeSet::new();
        for &f in &s {
            s_edges.extend(self.face_edges(f));
        }
        let mut border = BTreeSet::new();
        for &e in &s_edges {
            let faces = self.edge_faces(e);
            let inside = faces.iter().filter(|f| s.contains(f)).count();
            if inside == 1 || inside < faces.len() {
                border.insert(e);
            }
        }
        let on_border: BTreeSet<VertId> = border.iter().flat_map(|&e| self.edge_verts(e)).collect();
        let s_verts: BTreeSet<VertId> = s.iter().flat_map(|&f| self.face_verts(f)).collect();
        let interior: Vec<VertId> = s_verts.difference(&on_border).copied().collect();

        // Each side quad follows the first selected corner on its edge: the
        // selected face ran a -> b, the side keeps that direction on the
        // old edge and runs b' -> a' against the rewired face.
        let mut sides = Vec::new();
        for &e in &border {
            let l = self
                .edge_loops(e)
                .into_iter()
                .find(|&l| s.contains(&self.loop_face(l)))
                .expect("a border edge has a selected face");
            let lp = self.l(l);
            let next = self.l(lp.next);
            let face = self.f(lp.f);
            sides.push((lp.v, next.v, lp.attrs.fresh(), next.attrs.fresh(), face.material, face.group));
        }

        let mut copies = BTreeMap::new();
        for &v in &on_border {
            self.copy_of(&mut copies, v);
        }
        for &f in &s {
            let mut poly = self.face_poly(f);
            poly.verts = poly.verts.iter().map(|v| *copies.get(v).unwrap_or(v)).collect();
            poly.flags |= face_flags::LIGHTMAP_STALE;
            self.rebuild_face(f, &poly)?;
        }
        let mut new_faces = Vec::new();
        for (a, b, ca, cb, material, group) in sides {
            let poly = Poly {
                verts: vec![a, b, copies[&b], copies[&a]],
                corners: vec![ca, cb, cb, ca],
                material,
                group,
                flags: face_flags::LIGHTMAP_STALE,
                src: None,
            };
            new_faces.push(self.add_face(&poly)?);
        }
        // Interior edges that ran to a border vert now run to its copy.
        for &e in &s_edges {
            self.kill_edge_if_wire(e);
        }
        if offset != [0.0; 3] {
            let moved: BTreeSet<VertId> = copies.values().copied().chain(interior).collect();
            self.move_verts(&moved, &Affine::translate(offset));
        }
        for f in new_faces {
            self.flat_normals(f);
        }
        let faces: Vec<FaceId> = s.into_iter().collect();
        self.select_result(&[], &[], &faces);
        Ok(())
    }

    /// `ExtrudeEdges`: each selected edge (boundary or wire only) is copied
    /// with its ends, and a quad joins it to its copy, taking material,
    /// group and corners from the face beside it. Copies move by
    /// `offset`. Counts: V +|Ve|, E +|Es| + |Ve|, F +|Es|.
    pub(crate) fn extrude_edges(&mut self, offset: Vec3) -> Result<(), MeshError> {
        let edges = self.selected_edges();
        if edges.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let mut plans = Vec::new();
        for &e in &edges {
            let loops = self.edge_loops(e);
            let plan = match loops.as_slice() {
                [] => {
                    let [a, b] = self.edge_verts(e);
                    let c = Corner::default().fresh();
                    (a, b, c, c, 0, 0)
                }
                [l] => {
                    // The face runs x -> y; the quad runs y -> x beside it.
                    let lp = self.l(*l);
                    let next = self.l(lp.next);
                    let face = self.f(lp.f);
                    (next.v, lp.v, next.attrs.fresh(), lp.attrs.fresh(), face.material, face.group)
                }
                _ => return Err(MeshError::NotBoundary),
            };
            plans.push(plan);
        }
        let mut copies = BTreeMap::new();
        for &(a, b, ..) in &plans {
            self.copy_of(&mut copies, a);
            self.copy_of(&mut copies, b);
        }
        let mut new_faces = Vec::new();
        let mut top = Vec::new();
        for (a, b, ca, cb, material, group) in plans {
            let (a2, b2) = (copies[&a], copies[&b]);
            let poly = Poly {
                verts: vec![a, b, b2, a2],
                corners: vec![ca, cb, cb, ca],
                material,
                group,
                flags: face_flags::LIGHTMAP_STALE,
                src: None,
            };
            new_faces.push(self.add_face(&poly)?);
            top.push(self.find_edge(a2, b2).expect("the quad made its top edge"));
        }
        let moved: BTreeSet<VertId> = copies.values().copied().collect();
        if offset != [0.0; 3] {
            self.move_verts(&moved, &Affine::translate(offset));
        }
        for f in new_faces {
            self.flat_normals(f);
        }
        let verts: Vec<VertId> = moved.into_iter().collect();
        self.select_result(&verts, &top, &[]);
        Ok(())
    }

    /// `Duplicate`: the selected verts, edges and faces copied, attached to
    /// nothing, moved by `offset`, and selected. Copied corners are new (no
    /// source vertex). Counts: V +|Vs|, E +|Es|, F +|S|.
    pub(crate) fn duplicate(&mut self, offset: Vec3) -> Result<(), MeshError> {
        let verts = self.selected_verts();
        let edges = self.selected_edges();
        let faces = self.selected_faces();
        if verts.is_empty() && edges.is_empty() && faces.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let mut copies = BTreeMap::new();
        for &v in &verts {
            self.copy_of(&mut copies, v);
        }
        let mut new_edges = Vec::new();
        for &e in &edges {
            let [a, b] = self.edge_verts(e);
            let (a2, b2) = (self.copy_of(&mut copies, a), self.copy_of(&mut copies, b));
            new_edges.push(self.add_edge(a2, b2)?);
        }
        let mut new_faces = Vec::new();
        for &f in &faces {
            let mut poly = self.face_poly(f);
            let mut mapped = Vec::with_capacity(poly.len());
            for &v in &poly.verts {
                mapped.push(self.copy_of(&mut copies, v));
            }
            poly.verts = mapped;
            poly.corners = poly.corners.iter().map(Corner::fresh).collect();
            poly.src = None;
            new_faces.push(self.add_face(&poly)?);
        }
        let moved: BTreeSet<VertId> = copies.values().copied().collect();
        if offset != [0.0; 3] {
            self.move_verts(&moved, &Affine::translate(offset));
        }
        let new_verts: Vec<VertId> = moved.into_iter().collect();
        self.select_result(&new_verts, &new_edges, &new_faces);
        Ok(())
    }
}
