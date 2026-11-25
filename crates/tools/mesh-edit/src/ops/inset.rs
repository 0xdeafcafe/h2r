//! `Inset`: the selected faces shrunk inside a ring of new faces.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::MeshError;
use crate::ids::{FaceId, VertId};
use crate::math::{self, Vec3};
use crate::mesh::{Mesh, Poly, face_flags};
use crate::ops::Affine;

/// How far an inset corner may run along its bisector, as a multiple of
/// the thickness, at a very sharp corner.
const MAX_MITRE: f32 = 4.0;

/// A border edge: the selected face ran `a -> b`.
struct Side {
    a: VertId,
    b: VertId,
    corner_a: crate::attrs::Corner,
    corner_b: crate::attrs::Corner,
    material: u16,
    group: u16,
}

impl Mesh {
    /// `Inset`: each region of selected faces (`individual`: each selected
    /// face alone) is rewired to a copy of its border verts, and a quad
    /// joins each border edge to its copy, as `ExtrudeFaces` does. The
    /// copies then move into the face by `thickness`, measured across the
    /// plane from each border edge (so a corner runs along its bisector,
    /// at most four times as far), and the region's verts move along the
    /// region's normal by `depth`. A side quad takes material, group and
    /// both coordinates from the corner of the region beside it. Selects
    /// the inset faces. Counts, for a region with border `B` and border
    /// verts `Vb`: V +|Vb|, E +|B| + |Vb|, F +|B|.
    pub(crate) fn inset(&mut self, thickness: f32, depth: f32, individual: bool) -> Result<(), MeshError> {
        if !thickness.is_finite() || thickness < 0.0 || !depth.is_finite() {
            return Err(MeshError::BadInput("thickness: not negative; depth: finite".into()));
        }
        let selected = self.selected_faces();
        if selected.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let groups: Vec<Vec<FaceId>> =
            if individual { selected.iter().map(|&f| vec![f]).collect() } else { vec![selected.clone()] };
        let whole: BTreeSet<FaceId> = selected.iter().copied().collect();
        let mut sides_made = Vec::new();
        for group in &groups {
            self.inset_group(group, &whole, thickness, depth, &mut sides_made)?;
        }
        for f in sides_made {
            self.flat_normals(f);
        }
        self.select_result(&[], &[], &selected);
        Ok(())
    }

    fn inset_group(
        &mut self,
        group: &[FaceId],
        whole: &BTreeSet<FaceId>,
        thickness: f32,
        depth: f32,
        sides_made: &mut Vec<FaceId>,
    ) -> Result<(), MeshError> {
        let set: BTreeSet<FaceId> = group.iter().copied().collect();
        let mut group_edges = BTreeSet::new();
        for &f in &set {
            group_edges.extend(self.face_edges(f));
        }
        // A border edge has one face of the group, or a face of the group
        // and a face that is not. Among individually inset faces, an edge
        // two selected faces share is a border of each.
        let mut border = Vec::new();
        for &e in &group_edges {
            let faces = self.edge_faces(e);
            let inside = faces.iter().filter(|f| set.contains(f)).count();
            let outside_selected = faces.iter().any(|f| whole.contains(f) && !set.contains(f));
            if inside == 1 || inside < faces.len() || outside_selected {
                border.push(e);
            }
        }
        if border.is_empty() {
            return Err(MeshError::BadInput("the selected faces have no border to inset".into()));
        }
        let mut sides = Vec::new();
        let mut inward: BTreeMap<VertId, Vec<Vec3>> = BTreeMap::new();
        for &e in &border {
            let l = self
                .edge_loops(e)
                .into_iter()
                .find(|&l| set.contains(&self.loop_face(l)))
                .expect("a border edge has a face of the group");
            let lp = self.l(l);
            let next = self.l(lp.next);
            let face = self.f(lp.f);
            // The side's normals start empty so that no smooth fan joins
            // them to the inset face's; `flat_normals` sets them last.
            let (mut corner_a, mut corner_b) = (lp.attrs.fresh(), next.attrs.fresh());
            corner_a.normal = [0.0; 3];
            corner_b.normal = [0.0; 3];
            let side = Side {
                a: lp.v,
                b: next.v,
                corner_a,
                corner_b,
                material: face.material,
                group: face.group,
            };
            // The face is to the left of a -> b, seen from its front.
            let n = self.face_normal(lp.f);
            let along = math::sub(self.position(side.b), self.position(side.a));
            let into = math::normalize(math::cross(n, along));
            if into != [0.0; 3] {
                inward.entry(side.a).or_default().push(into);
                inward.entry(side.b).or_default().push(into);
            }
            sides.push(side);
        }
        let on_border: BTreeSet<VertId> = border.iter().flat_map(|&e| self.edge_verts(e)).collect();

        // Everything is measured before anything moves.
        let mut shift: BTreeMap<VertId, Vec3> = BTreeMap::new();
        let mut normal_at: BTreeMap<VertId, Vec3> = BTreeMap::new();
        for &f in &set {
            let n = self.face_normal(f);
            for v in self.face_verts(f) {
                let sum = normal_at.entry(v).or_insert([0.0; 3]);
                *sum = math::add(*sum, n);
            }
        }
        for (&v, sum) in &normal_at {
            let up = math::normalize(*sum);
            let mut d = math::scale(up, depth);
            if let Some(ins) = inward.get(&v) {
                let total = ins.iter().fold([0.0f32; 3], |acc, &n| math::add(acc, n));
                let bisector = math::normalize(total);
                if bisector != [0.0; 3] {
                    let reach = math::dot(bisector, ins[0]).max(1.0 / MAX_MITRE);
                    d = math::add(d, math::scale(bisector, thickness / reach));
                }
            }
            shift.insert(v, d);
        }

        let mut copies = BTreeMap::new();
        for &v in &on_border {
            self.copy_of(&mut copies, v);
        }
        for &f in &set {
            let mut poly = self.face_poly(f);
            poly.verts = poly.verts.iter().map(|v| *copies.get(v).unwrap_or(v)).collect();
            poly.flags |= face_flags::LIGHTMAP_STALE;
            self.rebuild_face(f, &poly)?;
        }
        for side in sides {
            let poly = Poly {
                verts: vec![side.a, side.b, copies[&side.b], copies[&side.a]],
                corners: vec![side.corner_a, side.corner_b, side.corner_b, side.corner_a],
                material: side.material,
                group: side.group,
                flags: face_flags::LIGHTMAP_STALE,
                src: None,
            };
            sides_made.push(self.add_face(&poly)?);
        }
        for &e in &group_edges {
            self.kill_edge_if_wire(e);
        }
        for (v, d) in shift {
            if d == [0.0; 3] {
                continue;
            }
            let target = *copies.get(&v).unwrap_or(&v);
            self.move_verts(&BTreeSet::from([target]), &Affine::translate(d));
        }
        Ok(())
    }
}
