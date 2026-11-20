//! Loop cut, subdivide and bridge: counts and interpolated attributes.

mod common;

use common::*;
use mesh_edit::fixtures::{self, Caps};
use mesh_edit::{DeleteWhat, MeshError, MeshOp, SelectMode};

#[test]
fn loop_cut_round_a_tube_interpolates_corners() {
    let mut m = fixtures::cylinder(8, 1, 1.0, 2.0, Caps::Open);
    let e = edge(&m, 0, 8);
    let r = ok(&mut m, MeshOp::LoopCut { edge: e, cuts: 1, factor: 0.25 });
    assert_eq!(m.counts(), (24, 40, 16));
    assert_eq!(r.new_verts.len(), 8);
    assert_eq!(m.selection().mode(), SelectMode::Edge);

    // Every new vert sits at the same height, a quarter of the way along
    // its ring edge from one end.
    let z = m.position(r.new_verts[0])[2];
    assert!(close(z, 0.5) || close(z, 1.5), "z = {z}");
    for &x in &r.new_verts {
        let p = m.position(x);
        assert!(close(p[2], z));
        assert!(close(p[0] * p[0] + p[1] * p[1], 1.0), "stays on the radius");
    }
    // Corners at the new verts: v = z / height, lightmap half that, and
    // the radial normal.
    for f in m.face_ids().collect::<Vec<_>>() {
        let poly = m.face_poly(f);
        for (k, &x) in poly.verts.iter().enumerate() {
            if !r.new_verts.contains(&x) {
                continue;
            }
            let c = poly.corners[k];
            let p = m.position(x);
            assert!(close(c.uv[1], z / 2.0), "uv {:?}", c.uv);
            assert!(close(c.lightmap[1], z / 4.0), "lightmap {:?}", c.lightmap);
            assert!(close3(c.normal, [p[0], p[1], 0.0]), "normal {:?} at {p:?}", c.normal);
            assert_eq!(c.src, None);
        }
    }
}

#[test]
fn loop_cut_counts_on_a_grid_and_at_a_pole() {
    // grid(3, 1): the ring across verts 0-4, 1-5, 2-6, 3-7 crosses 3 quads.
    let mut m = fixtures::grid(3, 1, 1.0);
    let e = edge(&m, 0, 4);
    ok(&mut m, MeshOp::LoopCut { edge: e, cuts: 2, factor: 0.5 });
    assert_eq!(m.counts(), (16, 24, 9));

    // A ring edge between a side quad and a fan triangle: the ring is the
    // one quad, its two ring edges split, the triangles beside take the
    // new verts.
    let mut m = fixtures::cylinder(8, 1, 1.0, 1.0, Caps::Fan);
    let before = m.counts();
    let e = edge(&m, 0, 1);
    ok(&mut m, MeshOp::LoopCut { edge: e, cuts: 1, factor: 0.5 });
    assert_eq!(m.counts(), (before.0 + 2, before.1 + 3, before.2 + 1));
}

#[test]
fn loop_cut_on_a_dead_edge_changes_nothing() {
    let mut m = fixtures::grid(1, 1, 1.0);
    refused(&mut m, MeshOp::LoopCut { edge: mesh_edit::EdgeId(99), cuts: 1, factor: 0.5 }, MeshError::Dead);
}

#[test]
fn subdivide_edges() {
    let mut m = fixtures::grid(1, 1, 1.0);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    ok(&mut m, MeshOp::Subdivide { cuts: 1 });
    assert_eq!(m.counts(), (5, 5, 1));
    assert_eq!(m.face_len(mesh_edit::FaceId(0)), 5);

    let mut m = fixtures::cube(1.0);
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::Subdivide { cuts: 3 });
    assert_eq!(m.counts(), (8 + 12 * 3, 12 * 4, 6));
    for f in m.face_ids().collect::<Vec<_>>() {
        assert_eq!(m.face_len(f), 16);
    }
}

#[test]
fn bridge_two_boundary_loops() {
    // cylinder(4, 3): rows of faces 0-3, 4-7, 8-11. Removing the middle
    // row leaves two tubes facing each other across a gap.
    let mut m = fixtures::cylinder(4, 3, 1.0, 3.0, Caps::Open);
    select(&mut m, SelectMode::Face, &[4, 5, 6, 7]);
    ok(&mut m, MeshOp::Delete { what: DeleteWhat::Faces });
    assert_eq!(m.counts(), (16, 24, 8));
    let (a, b) = (edge(&m, 4, 5), edge(&m, 8, 9));
    let r = ok(&mut m, MeshOp::Bridge { a, b });
    assert_eq!(m.counts(), (16, 28, 12));
    assert_eq!(r.new_faces.len(), 4);
    for &f in &r.new_faces {
        let (n, c) = (m.face_normal(f), m.face_center(f));
        assert!(n[0] * c[0] + n[1] * c[1] > 0.0, "bridge face {f:?} faces in");
        assert!(close(c[2], 1.5));
    }
    // An edge with two faces is not on a boundary.
    let (side, rim) = (edge(&m, 0, 4), edge(&m, 0, 1));
    refused(&mut m, MeshOp::Bridge { a: side, b: rim }, MeshError::NotBoundary);
}
