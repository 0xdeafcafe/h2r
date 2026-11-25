//! `Bevel`: each selected edge replaced by a strip of one face.

use std::collections::BTreeSet;

use crate::attrs::Corner;
use crate::error::MeshError;
use crate::ids::{EdgeId, FaceId, VertId};
use crate::math;
use crate::mesh::{Mesh, Poly, face_flags};

/// One bevelled edge `a -> b`, as its face `f1` runs it (`f2` runs it
/// back), and the four edges its ends cut: for each end, the edge of `f1`
/// and the edge of `f2` that meet the bevelled edge there, with the end
/// they are cut from.
struct Plan {
    edge: EdgeId,
    a: VertId,
    b: VertId,
    f1: FaceId,
    f2: FaceId,
    /// `a` along `f1`, `a` along `f2`, `b` along `f1`, `b` along `f2`.
    cuts: [(EdgeId, VertId); 4],
}

fn corner_at(poly: &Poly, v: VertId) -> Corner {
    poly.corners[poly.verts.iter().position(|&x| x == v).expect("the face has the vert")]
}

fn without(poly: &Poly, drop: [VertId; 2]) -> Poly {
    let mut out = Poly { verts: Vec::new(), corners: Vec::new(), src: None, ..poly.clone() };
    for (&v, &c) in poly.verts.iter().zip(&poly.corners) {
        if !drop.contains(&v) {
            out.verts.push(v);
            out.corners.push(c);
        }
    }
    out.flags |= face_flags::LIGHTMAP_STALE;
    out
}

impl Mesh {
    /// `Bevel`: each selected edge, which must have two faces that run it
    /// in opposite directions, is replaced by a strip face. At each end
    /// the two faces' other edges are cut at `width` from the end (at most
    /// half of the edge), and the strip joins the four cuts; a face round
    /// the end between the two cuts has its corner there replaced by them,
    /// and an end that no face is left on is removed. The bevelled edges
    /// must not share an end, and no edge may be cut by two of them. The
    /// strip takes material and group from the first face, coordinates
    /// from the faces it joins, and a flat normal. Selects the strips. For
    /// an edge between two faces whose ends each have three edges: V +2,
    /// E +3, F +1.
    pub(crate) fn bevel(&mut self, width: f32) -> Result<(), MeshError> {
        if !width.is_finite() || width <= 0.0 {
            return Err(MeshError::BadInput("width: more than zero".into()));
        }
        let edges = self.selected_edges();
        if edges.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let plans = self.bevel_plans(&edges)?;

        // Every cut first, on the unchanged topology; then each strip.
        let mut cut_verts = Vec::with_capacity(plans.len());
        for plan in &plans {
            let mut made = [VertId(0); 4];
            for (slot, &(edge, from)) in plan.cuts.iter().enumerate() {
                let [p, q] = self.edge_verts(edge);
                let len = math::length(math::sub(self.position(p), self.position(q)));
                let t = if len > 0.0 { (width / len).min(0.5) } else { 0.5 };
                made[slot] = self.split_edge(edge, from, &[t])?[0];
            }
            cut_verts.push(made);
        }
        let mut strips = Vec::new();
        for (plan, &[a1, a2, b1, b2]) in plans.iter().zip(&cut_verts) {
            let (p1, p2) = (self.face_poly(plan.f1), self.face_poly(plan.f2));
            let (a, b) = (plan.a, plan.b);
            let corners = [
                corner_at(&p1, b1),
                corner_at(&p1, a1),
                corner_at(&p1, a),
                corner_at(&p2, a2),
                corner_at(&p2, b2),
                corner_at(&p2, b),
            ];
            self.rebuild_face(plan.f1, &without(&p1, [a, b]))?;
            self.rebuild_face(plan.f2, &without(&p2, [a, b]))?;
            let keep_a = self.close_end(a, a1, a2)?;
            let keep_b = self.close_end(b, b1, b2)?;
            let mut verts = vec![b1, a1];
            let mut strip_corners = vec![corners[0], corners[1]];
            if keep_a {
                verts.push(a);
                strip_corners.push(corners[2]);
            }
            verts.push(a2);
            strip_corners.push(corners[3]);
            verts.push(b2);
            strip_corners.push(corners[4]);
            if keep_b {
                verts.push(b);
                strip_corners.push(corners[5]);
            }
            let strip = Poly {
                verts,
                corners: strip_corners.iter().map(Corner::fresh).collect(),
                material: p1.material,
                group: p1.group,
                flags: face_flags::LIGHTMAP_STALE,
                src: None,
            };
            let f = self.add_face(&strip)?;
            self.flat_normals(f);
            strips.push(f);

            self.kill_edge_if_wire(plan.edge);
            for (v, v1, v2) in [(a, a1, a2), (b, b1, b2)] {
                for w in [v1, v2] {
                    if let Some(x) = self.find_edge(v, w) {
                        self.kill_edge_if_wire(x);
                    }
                }
                self.kill_vert_if_loose(v);
            }
        }
        self.select_result(&[], &[], &strips);
        Ok(())
    }

    /// Checks every selected edge and reads off its cuts, before anything
    /// changes.
    fn bevel_plans(&self, edges: &[EdgeId]) -> Result<Vec<Plan>, MeshError> {
        let mut ends = BTreeSet::new();
        let mut cut_edges = BTreeSet::new();
        let mut plans = Vec::new();
        for &e in edges {
            let loops = self.edge_loops(e);
            let [l1, l2] = loops.as_slice() else { return Err(MeshError::NotBoundary) };
            let (f1, f2) = (self.loop_face(*l1), self.loop_face(*l2));
            let (a, b) = (self.loop_vert(*l1), self.loop_vert(self.loop_next(*l1)));
            if f1 == f2 || self.loop_vert(*l2) != b {
                return Err(MeshError::BadInput("a bevelled edge needs two faces that run it in opposite directions".into()));
            }
            for v in [a, b] {
                if !ends.insert(v) {
                    return Err(MeshError::BadInput("bevelled edges share a vertex".into()));
                }
                if self.vert_edges(v).len() < 3 {
                    return Err(MeshError::BadInput("a bevelled edge ends at a vertex of two edges".into()));
                }
            }
            let (p1, p2) = (self.face_poly(f1), self.face_poly(f2));
            let (n1, n2) = (p1.len(), p2.len());
            let at = |p: &Poly, v: VertId| p.verts.iter().position(|&x| x == v).ok_or(MeshError::Dead);
            // `f1` runs ... x, a, b, y ...; `f2` runs ... z, b, a, w ...
            let x = p1.verts[(at(&p1, a)? + n1 - 1) % n1];
            let y = p1.verts[(at(&p1, b)? + 1) % n1];
            let z = p2.verts[(at(&p2, b)? + n2 - 1) % n2];
            let w = p2.verts[(at(&p2, a)? + 1) % n2];
            let find = |u: VertId, v: VertId| self.find_edge(u, v).ok_or(MeshError::Dead);
            let cuts = [(find(a, x)?, a), (find(a, w)?, a), (find(b, y)?, b), (find(b, z)?, b)];
            for (edge, _) in cuts {
                if !cut_edges.insert(edge) {
                    return Err(MeshError::BadInput("two bevelled edges would cut the same edge".into()));
                }
            }
            plans.push(Plan { edge: e, a, b, f1, f2, cuts });
        }
        Ok(plans)
    }

    /// Settles the end `v` of a bevelled edge once the two faces on the
    /// edge have let go of it, `v1` and `v2` being the cuts beside it.
    /// A face that runs `v1, v, v2` drops `v`, and `v` goes (`false`); if
    /// other faces remain on `v` it stays, joined to both cuts, and the
    /// strip passes through it (`true`); else it goes (`false`).
    fn close_end(&mut self, v: VertId, v1: VertId, v2: VertId) -> Result<bool, MeshError> {
        let others = self.vert_faces(v);
        for &g in &others {
            let poly = self.face_poly(g);
            let n = poly.len();
            let Some(i) = poly.verts.iter().position(|&x| x == v) else { continue };
            let (p, q) = (poly.verts[(i + n - 1) % n], poly.verts[(i + 1) % n]);
            if (p == v1 && q == v2) || (p == v2 && q == v1) {
                self.rebuild_face(g, &without(&poly, [v, v]))?;
                return Ok(false);
            }
        }
        Ok(!others.is_empty())
    }
}
