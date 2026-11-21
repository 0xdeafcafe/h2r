//! Cutting: split an edge, connect two corners of a face, loop cut,
//! subdivide.

use crate::attrs::blend_skins;
use crate::error::MeshError;
use crate::ids::{EdgeId, VertId};
use crate::math;
use crate::mesh::{Mesh, Poly, face_flags};

impl Mesh {
    /// Splits `e` at the factors `ts` (ascending, 0 at `from`, 1 at the
    /// other end). New verts interpolate position and skin; every face on
    /// the edge, whatever the count, gets them with its own corners
    /// interpolated (so seams stay seams) and keeps its id. Returns the new
    /// verts in order from `from`.
    pub(crate) fn split_edge(
        &mut self,
        e: EdgeId,
        from: VertId,
        ts: &[f32],
    ) -> Result<Vec<VertId>, MeshError> {
        if !self.is_edge(e) {
            return Err(MeshError::Dead);
        }
        let [v0, v1] = self.edge_verts(e);
        let (a, b) = if from == v0 {
            (v0, v1)
        } else if from == v1 {
            (v1, v0)
        } else {
            return Err(MeshError::BadInput("vertex is not on the edge".into()));
        };
        let (pa, pb) = (self.position(a), self.position(b));
        let (sa, sb) = (self.skin(a), self.skin(b));
        let mut faces = self.edge_faces(e);
        faces.dedup();
        let mids: Vec<VertId> = ts
            .iter()
            .map(|&t| self.add_vert(math::lerp(pa, pb, t), blend_skins(&[(sa, 1.0 - t), (sb, t)])))
            .collect();
        for f in faces {
            let poly = self.face_poly(f);
            let n = poly.len();
            let mut grown = Poly { verts: Vec::new(), corners: Vec::new(), src: None, ..poly.clone() };
            grown.flags |= face_flags::LIGHTMAP_STALE;
            for i in 0..n {
                let j = (i + 1) % n;
                let (x, y) = (poly.verts[i], poly.verts[j]);
                let (cx, cy) = (poly.corners[i], poly.corners[j]);
                grown.verts.push(x);
                grown.corners.push(cx);
                if x == a && y == b {
                    for (k, &m) in mids.iter().enumerate() {
                        grown.verts.push(m);
                        grown.corners.push(cx.lerp(&cy, ts[k]));
                    }
                } else if x == b && y == a {
                    // Walking b -> a the cuts come in descending t, and the
                    // corner at a is cy.
                    for (k, &m) in mids.iter().enumerate().rev() {
                        grown.verts.push(m);
                        grown.corners.push(cy.lerp(&cx, ts[k]));
                    }
                }
            }
            self.rebuild_face(f, &grown)?;
        }
        let was_selected = self.select.has_edge(e);
        self.kill_edge(e);
        let mut chain = vec![a];
        chain.extend(&mids);
        chain.push(b);
        for pair in chain.windows(2) {
            let piece = self.add_edge(pair[0], pair[1])?;
            if was_selected {
                self.select.edges.insert(piece.index());
            }
        }
        Ok(mids)
    }

    /// Joins two non-adjacent corners `a` and `b` of one face with an edge,
    /// splitting the face: the part from `a` round to `b` keeps the face's
    /// id. Returns the new edge.
    pub(crate) fn connect(&mut self, a: VertId, b: VertId) -> Result<EdgeId, MeshError> {
        for f in self.vert_faces(a) {
            let poly = self.face_poly(f);
            let n = poly.len();
            let Some(ia) = poly.verts.iter().position(|&v| v == a) else { continue };
            let Some(ib) = poly.verts.iter().position(|&v| v == b) else { continue };
            if (ia + 1) % n == ib || (ib + 1) % n == ia {
                continue;
            }
            let arc = |from: usize, to: usize| {
                let mut part = Poly { verts: Vec::new(), corners: Vec::new(), src: None, ..poly.clone() };
                let mut i = from;
                loop {
                    part.verts.push(poly.verts[i]);
                    part.corners.push(poly.corners[i]);
                    if i == to {
                        break;
                    }
                    i = (i + 1) % n;
                }
                part
            };
            let (first, second) = (arc(ia, ib), arc(ib, ia));
            self.rebuild_face(f, &first)?;
            self.add_face(&second)?;
            return self.find_edge(a, b).ok_or(MeshError::NoSharedFace);
        }
        Err(MeshError::NoSharedFace)
    }

    /// `LoopCut`: the ring through `edge` (`edge_ring`); each ring edge
    /// gets `cuts` new verts (at `factor` when `cuts` is 1, else evenly
    /// spaced), measured from the same side of the ring all the way round,
    /// and each quad of the ring becomes `cuts + 1` quads. Positions,
    /// corner attributes and skins interpolate. Faces at an open ring's
    /// ends that are not quads gain the new verts in their side. Selects
    /// the new loops. Counts, for k ring edges and m quads: V +cuts·k,
    /// E +cuts·(k+m), F +cuts·m.
    pub(crate) fn loop_cut(&mut self, edge: EdgeId, cuts: u32, factor: f32) -> Result<(), MeshError> {
        let ring = self.ring(edge).ok_or(MeshError::Dead)?;
        if ring.quads.is_empty() {
            return Err(MeshError::NotQuadRing);
        }
        let cuts = cuts.max(1) as usize;
        let ts: Vec<f32> = if cuts == 1 {
            vec![factor.clamp(0.0, 1.0)]
        } else {
            (1..=cuts).map(|k| k as f32 / (cuts + 1) as f32).collect()
        };
        let mut mids = Vec::with_capacity(ring.edges.len());
        for &(e, from) in &ring.edges {
            mids.push(self.split_edge(e, from, &ts)?);
        }
        let k = mids.len();
        let quads = if ring.closed { k } else { k - 1 };
        let mut new_edges = Vec::new();
        for i in 0..quads {
            for c in 0..cuts {
                new_edges.push(self.connect(mids[i][c], mids[(i + 1) % k][c])?);
            }
        }
        let new_verts: Vec<VertId> = mids.into_iter().flatten().collect();
        self.select.mode = crate::select::SelectMode::Edge;
        self.select_result(&new_verts, &new_edges, &[]);
        Ok(())
    }

    /// `Subdivide`: each selected edge cut into `cuts + 1` equal parts; the
    /// faces on it gain the corners. Selects the pieces.
    pub(crate) fn subdivide(&mut self, cuts: u32) -> Result<(), MeshError> {
        let edges = self.selected_edges();
        if edges.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let cuts = cuts.max(1) as usize;
        let ts: Vec<f32> = (1..=cuts).map(|k| k as f32 / (cuts + 1) as f32).collect();
        let mut pieces = Vec::new();
        let mut new_verts = Vec::new();
        for e in edges {
            if !self.is_edge(e) {
                continue;
            }
            let [a, b] = self.edge_verts(e);
            let mids = self.split_edge(e, a, &ts)?;
            let mut chain = vec![a];
            chain.extend(&mids);
            chain.push(b);
            for pair in chain.windows(2) {
                pieces.extend(self.find_edge(pair[0], pair[1]));
            }
            new_verts.extend(mids);
        }
        self.select_result(&new_verts, &pieces, &[]);
        Ok(())
    }
}
