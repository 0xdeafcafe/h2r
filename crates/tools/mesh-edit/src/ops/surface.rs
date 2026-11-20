//! Fill, bridge, flip, recalculate normals, triangulate.

use std::collections::{BTreeMap, BTreeSet};

use crate::attrs::Corner;
use crate::error::MeshError;
use crate::ids::{EdgeId, FaceId, VertId};
use crate::math::{self, Vec3};
use crate::mesh::{Mesh, Poly, face_flags};
use crate::triangulate::triangulate;

impl Mesh {
    /// The corner a new face at `v` should copy: the corner at `v` of a
    /// face on one of `edges` (fresh: no source, all dirty), and that
    /// face's material and group.
    fn corner_near(&self, v: VertId, edges: &[Option<EdgeId>]) -> Option<(Corner, u16, u16)> {
        for &e in edges.iter().flatten() {
            for l in self.edge_loops(e) {
                let at = if self.loop_vert(l) == v { l } else { self.loop_next(l) };
                if self.loop_vert(at) == v {
                    let f = self.loop_face(at);
                    return Some((self.corner(at).fresh(), self.face_material(f), self.face_group(f)));
                }
            }
        }
        None
    }

    /// A face on `cycle`, wound so that edges already used by one face are
    /// run against that face's direction, with corners from the faces
    /// beside it and its flat normal.
    fn add_fill_face(&mut self, cycle: Vec<VertId>) -> Result<FaceId, MeshError> {
        let n = cycle.len();
        if n < 3 {
            return Err(MeshError::Degenerate);
        }
        if cycle.iter().any(|&v| !self.is_vert(v)) {
            return Err(MeshError::Dead);
        }
        let mut score = 0i32;
        for i in 0..n {
            let (a, b) = (cycle[i], cycle[(i + 1) % n]);
            if let Some(e) = self.find_edge(a, b) {
                for l in self.edge_loops(e) {
                    score += if self.loop_vert(l) == a { -1 } else { 1 };
                }
            }
        }
        let mut cycle = cycle;
        if score < 0 {
            cycle.reverse();
        }
        let mut corners = Vec::with_capacity(n);
        let (mut material, mut group) = (None, 0);
        for i in 0..n {
            let v = cycle[i];
            let edges = [self.find_edge(cycle[(i + n - 1) % n], v), self.find_edge(v, cycle[(i + 1) % n])];
            match self.corner_near(v, &edges) {
                Some((c, m, g)) => {
                    corners.push(c);
                    if material.is_none() {
                        material = Some(m);
                        group = g;
                    }
                }
                None => corners.push(Corner::default().fresh()),
            }
        }
        let poly = Poly {
            verts: cycle,
            corners,
            material: material.unwrap_or(0),
            group,
            flags: face_flags::LIGHTMAP_STALE,
            src: None,
        };
        let f = self.add_face(&poly)?;
        self.flat_normals(f);
        Ok(f)
    }

    /// `Fill`: one face from an ordered vert cycle, or from the closed
    /// chain of selected edges. F +1, E + the missing edges.
    pub(crate) fn fill(&mut self, verts: Option<&[VertId]>) -> Result<(), MeshError> {
        let cycle = match verts {
            Some(verts) => verts.to_vec(),
            None => self.selected_edge_cycle()?,
        };
        let f = self.add_fill_face(cycle)?;
        self.select_result(&[], &[], &[f]);
        Ok(())
    }

    /// The selected edges as one closed chain of verts.
    fn selected_edge_cycle(&self) -> Result<Vec<VertId>, MeshError> {
        let edges = self.selected_edges();
        if edges.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let not_a_cycle = || MeshError::BadInput("the selected edges are not one closed chain".into());
        let mut at: BTreeMap<VertId, Vec<EdgeId>> = BTreeMap::new();
        for &e in &edges {
            for v in self.edge_verts(e) {
                at.entry(v).or_default().push(e);
            }
        }
        if at.values().any(|list| list.len() != 2) {
            return Err(not_a_cycle());
        }
        let [start, mut v] = self.edge_verts(edges[0]);
        let mut cur = edges[0];
        let mut cycle = vec![start];
        while v != start {
            cycle.push(v);
            let list = &at[&v];
            cur = if list[0] == cur { list[1] } else { list[0] };
            v = self.other_end(cur, v);
            if cycle.len() > edges.len() {
                return Err(not_a_cycle());
            }
        }
        if cycle.len() != edges.len() {
            return Err(not_a_cycle());
        }
        Ok(cycle)
    }

    /// The closed boundary through boundary edge `e`, as verts, wound so
    /// the faces beside it run each edge the other way.
    fn boundary_cycle(&self, e: EdgeId) -> Result<Vec<VertId>, MeshError> {
        if !self.is_edge(e) {
            return Err(MeshError::Dead);
        }
        if self.edge_face_count(e) != 1 {
            return Err(MeshError::NotBoundary);
        }
        let [start, mut v] = self.edge_verts(e);
        let mut cur = e;
        let mut cycle = vec![start];
        while v != start {
            cycle.push(v);
            cur = self.boundary_step(cur, v).ok_or(MeshError::LoopMismatch)?;
            v = self.other_end(cur, v);
            if cycle.len() > self.edge_count() {
                return Err(MeshError::LoopMismatch);
            }
        }
        let l = self.edge_loops(e)[0];
        if self.loop_vert(l) == start {
            cycle[1..].reverse();
        }
        Ok(cycle)
    }

    /// `Bridge`: a band of quads joining the boundary loops through `a`
    /// and `b`, which must be distinct, disjoint and of equal length. The
    /// loops are paired at the rotation that puts matching verts closest.
    pub(crate) fn bridge(&mut self, a: EdgeId, b: EdgeId) -> Result<(), MeshError> {
        let la = self.boundary_cycle(a)?;
        let lb = self.boundary_cycle(b)?;
        let n = la.len();
        let set_a: BTreeSet<VertId> = la.iter().copied().collect();
        if lb.len() != n || n < 3 || lb.iter().any(|v| set_a.contains(v)) {
            return Err(MeshError::LoopMismatch);
        }
        // Both loops run against their faces, so facing each other they
        // run in opposite senses: la[i] pairs with lb[s - i].
        let pa: Vec<Vec3> = la.iter().map(|&v| self.position(v)).collect();
        let pb: Vec<Vec3> = lb.iter().map(|&v| self.position(v)).collect();
        let cost = |s: usize| -> f32 { (0..n).map(|i| math::dist2(pa[i], pb[(s + n - i) % n])).sum() };
        let s = (0..n).min_by(|&x, &y| cost(x).total_cmp(&cost(y))).unwrap_or(0);
        let near = |mesh: &Mesh, ring: &[VertId], i: usize| {
            let m = ring.len();
            let edges = [mesh.find_edge(ring[(i + m - 1) % m], ring[i]), mesh.find_edge(ring[i], ring[(i + 1) % m])];
            mesh.corner_near(ring[i], &edges)
        };
        let fallback = (Corner::default().fresh(), 0, 0);
        let ca: Vec<_> = (0..n).map(|i| near(self, &la, i).unwrap_or(fallback)).collect();
        let cb: Vec<_> = (0..n).map(|i| near(self, &lb, i).unwrap_or(fallback)).collect();
        let mut band = Vec::with_capacity(n);
        for i in 0..n {
            let i1 = (i + 1) % n;
            let j0 = (s + n - i) % n;
            let j1 = (s + 2 * n - i - 1) % n;
            let poly = Poly {
                verts: vec![la[i], la[i1], lb[j1], lb[j0]],
                corners: vec![ca[i].0, ca[i1].0, cb[j1].0, cb[j0].0],
                material: ca[i].1,
                group: ca[i].2,
                flags: face_flags::LIGHTMAP_STALE,
                src: None,
            };
            let f = self.add_face(&poly)?;
            self.flat_normals(f);
            band.push(f);
        }
        self.select_result(&[], &[], &band);
        Ok(())
    }

    /// `Flip`: reverses the selected faces; corner normals turn round. A
    /// radial-edge mesh holds faces of either winding on one edge, so any
    /// set of faces can flip.
    pub(crate) fn flip(&mut self) -> Result<(), MeshError> {
        let faces = self.selected_faces();
        if faces.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        for f in faces {
            let poly = self.face_poly(f).reversed();
            self.rebuild_face(f, &poly)?;
        }
        Ok(())
    }

    /// `RecalcNormals`: per connected component of the selected faces
    /// (joined across edges with exactly two faces), orients the faces
    /// alike, then outward: a closed component to positive volume, an open
    /// one to face away from its centre. Then recomputes every
    /// smooth fan of the selected faces.
    pub(crate) fn recalc_normals(&mut self) -> Result<(), MeshError> {
        let scope: BTreeSet<FaceId> = self.selected_faces().into_iter().collect();
        if scope.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let mut seen = BTreeSet::new();
        for &seed in &scope {
            if !seen.insert(seed) {
                continue;
            }
            let mut component = vec![seed];
            let mut stack = vec![seed];
            while let Some(f) = stack.pop() {
                for l in self.face_loops(f) {
                    let ring = self.edge_loops(self.loop_edge(l));
                    if ring.len() != 2 {
                        continue;
                    }
                    let other = if ring[0] == l { ring[1] } else { ring[0] };
                    let g = self.loop_face(other);
                    if !scope.contains(&g) || !seen.insert(g) {
                        continue;
                    }
                    if self.loop_vert(other) == self.loop_vert(l) {
                        // g runs the shared edge the same way as f: turn it.
                        let poly = self.face_poly(g).reversed();
                        self.rebuild_face(g, &poly)?;
                    }
                    component.push(g);
                    stack.push(g);
                }
            }
            if self.faces_point_inward(&component) {
                for &f in &component {
                    let poly = self.face_poly(f).reversed();
                    self.rebuild_face(f, &poly)?;
                }
            }
        }
        self.refresh_fans(&scope, None);
        Ok(())
    }

    /// Whether a consistently wound component faces inward: by signed
    /// volume when it is closed, else by its area-weighted normals against
    /// its centre.
    fn faces_point_inward(&self, component: &[FaceId]) -> bool {
        let set: BTreeSet<FaceId> = component.iter().copied().collect();
        let closed = component.iter().all(|&f| {
            self.face_edges(f).into_iter().all(|e| {
                let faces = self.edge_faces(e);
                faces.len() == 2 && faces.iter().all(|g| set.contains(g))
            })
        });
        if closed {
            let mut volume = 0.0f32;
            for &f in component {
                let p = self.face_positions(f);
                for i in 1..p.len() - 1 {
                    volume += math::dot(p[0], math::cross(p[i], p[i + 1]));
                }
            }
            return volume < 0.0;
        }
        // Open: corner normals turn with their faces, so they cannot say
        // which way is out. Take the area-weighted normals against the
        // component's centre instead: a bowl or a box without a lid faces
        // away from its middle.
        let mut points = Vec::new();
        for &f in component {
            points.extend(self.face_positions(f));
        }
        let middle = math::centroid(&points);
        let mut outward = 0.0f32;
        for &f in component {
            let p = self.face_positions(f);
            let area = math::newell(&p);
            outward += math::dot(area, math::sub(self.face_center(f), middle));
        }
        outward < 0.0
    }

    /// `Triangulate`: each selected face of more than three corners
    /// becomes triangles, a convex one as a fan from its first corner, any
    /// other by ear clipping. The first triangle keeps the face's id.
    /// F +(n-3) per face.
    pub(crate) fn triangulate_faces(&mut self) -> Result<(), MeshError> {
        let faces = self.selected_faces();
        if faces.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let mut result = Vec::new();
        for f in faces {
            let poly = self.face_poly(f);
            let n = poly.len();
            result.push(f);
            if n == 3 {
                continue;
            }
            let points = self.face_positions(f);
            let tris: Vec<[usize; 3]> = if is_convex(&points) {
                (1..n - 1).map(|i| [0, i, i + 1]).collect()
            } else {
                triangulate(&points)
            };
            for (k, tri) in tris.iter().enumerate() {
                let part = Poly {
                    verts: tri.iter().map(|&i| poly.verts[i]).collect(),
                    corners: tri.iter().map(|&i| poly.corners[i]).collect(),
                    src: None,
                    ..poly.clone()
                };
                if k == 0 {
                    self.rebuild_face(f, &part)?;
                } else {
                    result.push(self.add_face(&part)?);
                }
            }
        }
        self.select_result(&[], &[], &result);
        Ok(())
    }
}

/// Whether the polygon turns the same way at every corner.
fn is_convex(points: &[Vec3]) -> bool {
    let n = points.len();
    let normal = math::newell(points);
    (0..n).all(|i| {
        let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
        math::dot(math::cross(math::sub(b, a), math::sub(c, b)), normal) >= -1e-9
    })
}
