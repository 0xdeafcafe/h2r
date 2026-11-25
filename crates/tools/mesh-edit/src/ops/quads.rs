//! `TrisToQuads`: joins pairs of coplanar triangles into quads.

use std::collections::BTreeSet;

use crate::error::MeshError;
use crate::ids::{EdgeId, FaceId};
use crate::math::{self, Vec3};
use crate::mesh::{Mesh, Poly};

/// Two coordinates closer than this are one coordinate.
const SEAM_EPSILON: f32 = 1e-5;

/// A pair of triangles that may join across `edge`.
struct Pair {
    edge: EdgeId,
    keep: FaceId,
    drop: FaceId,
    /// `1 - cos` of the angle between the two normals: smaller is flatter.
    bend: f32,
    quad: Poly,
}

impl Mesh {
    /// `TrisToQuads`: each pair of selected triangles that share an edge
    /// with exactly two faces and a material and group, whose normals
    /// differ by at most `angle_deg` degrees, whose texture and lightmap
    /// coordinates agree on both ends of the shared edge, and whose union
    /// is a convex quad, joins into one quad. The flattest pair joins
    /// first, ties go to the lowest edge id, and a triangle joins at most
    /// once. The first triangle (the lower face id) keeps its id and its
    /// corners; the shared edge goes. Selects the quads and the triangles
    /// left. F -pairs, E -pairs.
    pub(crate) fn tris_to_quads(&mut self, angle_deg: f32) -> Result<(), MeshError> {
        if !angle_deg.is_finite() || !(0.0..=180.0).contains(&angle_deg) {
            return Err(MeshError::BadInput("angle: degrees from 0 to 180".into()));
        }
        let scope: BTreeSet<FaceId> = self.selected_faces().into_iter().collect();
        if scope.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let floor = angle_deg.to_radians().cos();
        let mut pairs = Vec::new();
        let mut seen = BTreeSet::new();
        for &f in &scope {
            if self.face_len(f) != 3 {
                continue;
            }
            for e in self.face_edges(f) {
                if !seen.insert(e) {
                    continue;
                }
                if let Some(pair) = self.join_candidate(e, &scope, floor) {
                    pairs.push(pair);
                }
            }
        }
        pairs.sort_by(|a, b| a.bend.total_cmp(&b.bend).then(a.edge.cmp(&b.edge)));
        let mut used = BTreeSet::new();
        let mut joined = Vec::new();
        for pair in pairs {
            if used.contains(&pair.keep) || used.contains(&pair.drop) {
                continue;
            }
            used.insert(pair.keep);
            used.insert(pair.drop);
            joined.push(pair);
        }
        if joined.is_empty() {
            return Err(MeshError::BadInput("no pair of triangles can join".into()));
        }
        let mut result: BTreeSet<FaceId> = scope;
        for pair in joined {
            self.rebuild_face(pair.keep, &pair.quad)?;
            self.kill_face(pair.drop);
            self.kill_edge_if_wire(pair.edge);
            result.remove(&pair.drop);
        }
        let faces: Vec<FaceId> = result.into_iter().collect();
        self.select_result(&[], &[], &faces);
        Ok(())
    }

    /// The pair of triangles on `e` if they may join (see `tris_to_quads`).
    fn join_candidate(&self, e: EdgeId, scope: &BTreeSet<FaceId>, floor: f32) -> Option<Pair> {
        let loops = self.edge_loops(e);
        let [l1, l2] = loops.as_slice() else { return None };
        let (f1, f2) = (self.loop_face(*l1), self.loop_face(*l2));
        let (keep, drop, lk, ld) = if f1 < f2 { (f1, f2, *l1, *l2) } else { (f2, f1, *l2, *l1) };
        if keep == drop || !scope.contains(&keep) || !scope.contains(&drop) {
            return None;
        }
        if self.face_len(keep) != 3 || self.face_len(drop) != 3 {
            return None;
        }
        if self.face_material(keep) != self.face_material(drop) || self.face_group(keep) != self.face_group(drop) {
            return None;
        }
        // The shared edge runs x0 -> x1 in `keep` and x1 -> x0 in `drop`.
        let (x0, x1) = (self.loop_vert(lk), self.loop_vert(self.loop_next(lk)));
        if self.loop_vert(ld) != x1 {
            return None;
        }
        let (a, b) = (self.face_poly(keep), self.face_poly(drop));
        let ia = a.verts.iter().position(|&v| v == x0)?;
        let ib = b.verts.iter().position(|&v| v == x1)?;
        // Rotated so the shared edge comes first: a = [x0, x1, x2],
        // b = [x1, x0, y].
        let at = |p: &Poly, i: usize, k: usize| (p.verts[(i + k) % 3], p.corners[(i + k) % 3]);
        let (_, a0) = at(&a, ia, 0);
        let (_, a1) = at(&a, ia, 1);
        let (x2, a2) = at(&a, ia, 2);
        let (_, b0) = at(&b, ib, 0);
        let (_, b1) = at(&b, ib, 1);
        let (y, by) = at(&b, ib, 2);
        // Continuous across the edge: a's corner at x0 is b's corner at x0.
        let same = |p: [f32; 2], q: [f32; 2]| (0..2).all(|i| (p[i] - q[i]).abs() <= SEAM_EPSILON);
        if !same(a0.uv, b1.uv) || !same(a0.lightmap, b1.lightmap) {
            return None;
        }
        if !same(a1.uv, b0.uv) || !same(a1.lightmap, b0.lightmap) {
            return None;
        }
        let (na, nb) = (self.face_normal(keep), self.face_normal(drop));
        if na == [0.0; 3] || nb == [0.0; 3] {
            return None;
        }
        let cosine = math::dot(na, nb);
        if cosine < floor {
            return None;
        }
        // The quad runs x1, x2, x0, y: a's edges but the shared one, then
        // b's.
        let verts = vec![x1, x2, x0, y];
        let points: Vec<Vec3> = verts.iter().map(|&v| self.position(v)).collect();
        if !is_strictly_convex(&points) {
            return None;
        }
        let quad = Poly {
            verts,
            corners: vec![a1, a2, a0, by],
            material: a.material,
            group: a.group,
            flags: a.flags | b.flags,
            src: None,
        };
        Some(Pair { edge: e, keep, drop, bend: 1.0 - cosine, quad })
    }
}

/// Whether the polygon turns the same way, by a real angle, at each corner.
fn is_strictly_convex(points: &[Vec3]) -> bool {
    let n = points.len();
    let normal = math::newell(points);
    if normal == [0.0; 3] {
        return false;
    }
    (0..n).all(|i| {
        let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
        math::dot(math::cross(math::sub(b, a), math::sub(c, b)), normal) > 1e-9
    })
}
