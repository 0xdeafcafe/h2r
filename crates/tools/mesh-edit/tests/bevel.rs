//! `TrisToQuads`, `Inset` and `Bevel`: counts, positions, attributes, and
//! refusals, on invented meshes.

mod common;

use common::*;
use mesh_edit::fixtures::{self, corner};
use mesh_edit::math;
use mesh_edit::{FaceId, Mesh, MeshError, MeshOp, Poly, SelectMode};

const UP: [f32; 3] = [0.0, 0.0, 1.0];

fn bad(what: &str) -> MeshError {
    MeshError::BadInput(what.into())
}

/// A grid cell's quad, cut into two triangles.
fn two_triangles() -> Mesh {
    let mut m = fixtures::grid(1, 1, 1.0);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::Triangulate);
    assert_eq!(m.counts(), (4, 5, 2));
    m
}

/// Two triangles on the diagonal 0-2 of the square (0,0) (1,0) (1,1) and
/// a fourth corner at `top`, with a material each and plain coordinates.
fn bent(top: [f32; 3], materials: [u16; 2], uv_of_second: [f32; 2]) -> Mesh {
    let mut m = Mesh::new();
    let v: Vec<_> = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], top]
        .iter()
        .map(|&p| m.add_vert(p, None))
        .collect();
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
    let mut a = Poly::new(vec![v[0], v[1], v[2]]);
    a.corners = uvs.iter().map(|uv| corner(uv[0], uv[1], [0.0; 3])).collect();
    a.material = materials[0];
    m.add_face(&a).unwrap();
    // The second triangle runs 0, 2, top; its corner at 2 may disagree.
    let mut b = Poly::new(vec![v[0], v[2], v[3]]);
    b.corners = vec![
        corner(0.0, 0.0, [0.0; 3]),
        corner(uv_of_second[0], uv_of_second[1], [0.0; 3]),
        corner(0.0, 1.0, [0.0; 3]),
    ];
    b.material = materials[1];
    m.add_face(&b).unwrap();
    m.assert_valid();
    m
}

fn all_faces(m: &mut Mesh) {
    ok(m, MeshOp::Mode { mode: SelectMode::Face });
    ok(m, MeshOp::SelectAll);
}

// ---- TrisToQuads ----

#[test]
fn join_then_triangulate_restores_a_pair() {
    let mut m = two_triangles();
    let r = ok(&mut m, MeshOp::TrisToQuads { angle: 1.0 });
    // F -1, E -1.
    assert_eq!(m.counts(), (4, 4, 1));
    assert!(r.new_faces.is_empty(), "the first triangle keeps its id");
    assert_eq!(m.face_len(FaceId(0)), 4);
    assert!(!m.is_face(FaceId(1)));
    assert_eq!(r.selected, 1);
    // Every corner keeps the coordinates it had.
    let poly = m.face_poly(FaceId(0));
    for (v, c) in poly.verts.iter().zip(&poly.corners) {
        let p = m.position(*v);
        assert!(close(c.uv[0], p[0]) && close(c.uv[1], p[1]), "{p:?} has {:?}", c.uv);
    }
    all_faces(&mut m);
    ok(&mut m, MeshOp::Triangulate);
    assert_eq!(m.counts(), (4, 5, 2));
}

#[test]
fn a_cube_triangulated_joins_back_face_by_face() {
    let mut m = fixtures::cube(1.0);
    all_faces(&mut m);
    ok(&mut m, MeshOp::Triangulate);
    assert_eq!(m.counts(), (8, 18, 12));
    // 90 degrees between faces is far past one degree: only the pairs on
    // a face join.
    ok(&mut m, MeshOp::TrisToQuads { angle: 1.0 });
    assert_eq!(m.counts(), (8, 12, 6));
    assert_eq!(euler(&m), 2);
    for f in m.face_ids().collect::<Vec<_>>() {
        assert_eq!(m.face_len(f), 4);
    }
}

#[test]
fn a_bent_pair_joins_only_within_the_angle() {
    // The normals differ by about 54.7 degrees.
    let mut m = bent([0.0, 1.0, 1.0], [0, 0], [1.0, 1.0]);
    all_faces(&mut m);
    refused(&mut m, MeshOp::TrisToQuads { angle: 30.0 }, bad("no pair of triangles can join"));
    ok(&mut m, MeshOp::TrisToQuads { angle: 60.0 });
    assert_eq!(m.counts(), (4, 4, 1));
}

#[test]
fn different_materials_and_seams_do_not_join() {
    let mut m = bent([0.0, 1.0, 0.0], [0, 1], [1.0, 1.0]);
    all_faces(&mut m);
    refused(&mut m, MeshOp::TrisToQuads { angle: 1.0 }, bad("no pair of triangles can join"));

    // The same pair but with a texture seam along the diagonal.
    let mut m = bent([0.0, 1.0, 0.0], [0, 0], [0.5, 0.5]);
    all_faces(&mut m);
    refused(&mut m, MeshOp::TrisToQuads { angle: 1.0 }, bad("no pair of triangles can join"));

    // And with matching coordinates it joins.
    let mut m = bent([0.0, 1.0, 0.0], [0, 0], [1.0, 1.0]);
    all_faces(&mut m);
    ok(&mut m, MeshOp::TrisToQuads { angle: 1.0 });
    assert_eq!(m.counts(), (4, 4, 1));
}

#[test]
fn a_pair_that_would_be_concave_does_not_join() {
    // The fourth corner is past the first corner's far side, so the quad
    // is reflex there.
    let mut m = bent([-1.0, -0.1, 0.0], [0, 0], [1.0, 1.0]);
    all_faces(&mut m);
    refused(&mut m, MeshOp::TrisToQuads { angle: 1.0 }, bad("no pair of triangles can join"));
}

#[test]
fn tris_to_quads_needs_a_selection() {
    let mut m = two_triangles();
    ok(&mut m, MeshOp::SelectNone);
    refused(&mut m, MeshOp::TrisToQuads { angle: 1.0 }, MeshError::NothingSelected);
    all_faces(&mut m);
    refused(&mut m, MeshOp::TrisToQuads { angle: -1.0 }, bad("angle: degrees from 0 to 180"));
}

// ---- Inset ----

#[test]
fn inset_a_quad_leaves_a_ring() {
    let mut m = fixtures::grid(1, 1, 1.0);
    all_faces(&mut m);
    let r = ok(&mut m, MeshOp::Inset { thickness: 0.1, depth: 0.0, individual: false });
    // |B| = |Vb| = 4: V +4, E +8, F +4.
    assert_eq!(m.counts(), (8, 12, 5));
    assert_eq!(euler(&m), 1);
    assert_eq!(r.new_faces.len(), 4);
    // The face keeps its id and now spans 0.1 to 0.9, at the same height.
    for v in m.face_verts(FaceId(0)) {
        let p = m.position(v);
        assert!(close(p[0], 0.1) || close(p[0], 0.9), "{p:?}");
        assert!(close(p[1], 0.1) || close(p[1], 0.9), "{p:?}");
        assert!(close(p[2], 0.0));
    }
    // The ring faces up, like the face it came from, and keeps its
    // material.
    for &f in &r.new_faces {
        assert!(close3(m.face_normal(f), UP), "{:?}", m.face_normal(f));
        assert_eq!(m.face_material(f), m.face_material(FaceId(0)));
    }
    assert_eq!(m.selection().faces().collect::<Vec<_>>(), vec![FaceId(0)]);
}

#[test]
fn inset_with_depth_moves_the_face_along_its_normal() {
    let mut m = fixtures::grid(1, 1, 1.0);
    all_faces(&mut m);
    let r = ok(&mut m, MeshOp::Inset { thickness: 0.25, depth: 0.5, individual: false });
    for v in m.face_verts(FaceId(0)) {
        assert!(close(m.position(v)[2], 0.5));
    }
    assert!(close3(m.face_normal(FaceId(0)), UP));
    // The ring slopes up to it: its normals tilt inward and up.
    for &f in &r.new_faces {
        assert!(m.face_normal(f)[2] > 0.1);
    }
}

#[test]
fn inset_a_region_keeps_its_interior_vert() {
    let mut m = fixtures::grid(2, 2, 1.0);
    all_faces(&mut m);
    ok(&mut m, MeshOp::Inset { thickness: 0.1, depth: 0.0, individual: false });
    // As ExtrudeFaces on the same region: V +8, E +16, F +8.
    assert_eq!(m.counts(), (17, 28, 12));
    assert_eq!(euler(&m), 1);
    assert!(close3(m.position(v(4)), [1.0, 1.0, 0.0]));
}

#[test]
fn inset_individually_gives_each_face_its_own_ring() {
    let mut m = fixtures::grid(2, 1, 1.0);
    all_faces(&mut m);
    ok(&mut m, MeshOp::Inset { thickness: 0.1, depth: 0.0, individual: true });
    assert_eq!(m.counts(), (14, 23, 10));
    assert_eq!(euler(&m), 1);

    let mut m = fixtures::grid(2, 1, 1.0);
    all_faces(&mut m);
    ok(&mut m, MeshOp::Inset { thickness: 0.1, depth: 0.0, individual: false });
    // One region: the edge between the cells goes with its copies.
    assert_eq!(m.counts(), (12, 19, 8));

    let mut m = fixtures::cube(1.0);
    all_faces(&mut m);
    ok(&mut m, MeshOp::Inset { thickness: 0.1, depth: 0.0, individual: true });
    assert_eq!(m.counts(), (32, 60, 30));
    assert_eq!(euler(&m), 2);
}

#[test]
fn inset_refuses_what_has_no_border() {
    let mut m = fixtures::cube(1.0);
    all_faces(&mut m);
    refused(
        &mut m,
        MeshOp::Inset { thickness: 0.1, depth: 0.0, individual: false },
        bad("the selected faces have no border to inset"),
    );
    ok(&mut m, MeshOp::SelectNone);
    refused(&mut m, MeshOp::Inset { thickness: 0.1, depth: 0.0, individual: false }, MeshError::NothingSelected);
    all_faces(&mut m);
    refused(
        &mut m,
        MeshOp::Inset { thickness: -1.0, depth: 0.0, individual: true },
        bad("thickness: not negative; depth: finite"),
    );
}

// ---- Bevel ----

#[test]
fn bevel_a_cube_edge_makes_one_strip() {
    let mut m = fixtures::cube(2.0);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    let r = ok(&mut m, MeshOp::Bevel { width: 0.25 });
    // Both ends have three edges: V +2, E +3, F +1.
    assert_eq!(m.counts(), (10, 15, 7));
    assert_eq!(euler(&m), 2);
    assert_eq!(r.new_faces.len(), 1);
    assert!(!m.is_vert(v(0)) && !m.is_vert(v(1)), "the ends were replaced");

    let strip = r.new_faces[0];
    assert_eq!(m.face_len(strip), 4);
    // Each strip vert is 0.25 from one old end, along an edge of the cube.
    let (p0, p1) = ([-1.0, -1.0, -1.0], [1.0, -1.0, -1.0]);
    for x in m.face_verts(strip) {
        let p = m.position(x);
        let d = math::length(math::sub(p, p0)).min(math::length(math::sub(p, p1)));
        assert!(close(d, 0.25), "{p:?} is {d} from the edge's ends");
    }
    // It faces out, across the corner the edge cut off.
    let n = m.face_normal(strip);
    assert!(close(math::dot(n, math::normalize([0.0, -1.0, -1.0])), 1.0), "{n:?}");
    assert_eq!(m.selection().faces().collect::<Vec<_>>(), vec![strip]);
    assert_eq!(m.selection().mode(), SelectMode::Edge);
}

#[test]
fn bevel_an_edge_between_quads_keeps_the_far_faces_whole() {
    let mut m = fixtures::cube(2.0);
    let before: Vec<usize> = (0..6).map(|i| m.face_len(FaceId(i))).collect();
    assert_eq!(before, vec![4; 6]);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    ok(&mut m, MeshOp::Bevel { width: 0.25 });
    // The two faces on the edge lost two corners and gained two; the
    // faces at the ends gained a corner each; the others are untouched.
    let mut lens: Vec<usize> = m.face_ids().map(|f| m.face_len(f)).collect();
    lens.sort_unstable();
    assert_eq!(lens, vec![4, 4, 4, 4, 4, 5, 5]);
}

#[test]
fn bevel_an_interior_grid_edge_ends_on_the_boundary() {
    let mut m = fixtures::grid(2, 1, 1.0);
    let e = edge(&m, 1, 4);
    select_edges(&mut m, &[e]);
    let r = ok(&mut m, MeshOp::Bevel { width: 0.2 });
    assert_eq!(m.counts(), (8, 10, 3));
    assert_eq!(euler(&m), 1);
    let strip = r.new_faces[0];
    assert_eq!(m.face_len(strip), 4);
    // The strip is as wide as the cut: 0.4 across the ends' spread.
    let xs: Vec<f32> = m.face_verts(strip).iter().map(|&x| m.position(x)[0]).collect();
    let (lo, hi) = (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max));
    assert!(close(lo, 0.8) && close(hi, 1.2), "{lo} {hi}");
    // The strip is coplanar with the grid: it faces up.
    assert!(close3(m.face_normal(strip), UP));
}

#[test]
fn bevel_keeps_a_vertex_that_other_faces_still_use() {
    // The centre of a 2 by 2 grid has four edges; bevelling the edge from
    // it to the east keeps it, and the strip passes through it.
    let mut m = fixtures::grid(2, 2, 1.0);
    let e = edge(&m, 4, 5);
    select_edges(&mut m, &[e]);
    let r = ok(&mut m, MeshOp::Bevel { width: 0.2 });
    assert_eq!(m.counts(), (12, 16, 5));
    assert_eq!(euler(&m), 1);
    assert!(m.is_vert(v(4)));
    assert_eq!(m.face_len(r.new_faces[0]), 5);
}

#[test]
fn bevel_interpolates_coordinates_on_the_cut() {
    let mut m = fixtures::grid(2, 1, 1.0);
    let e = edge(&m, 1, 4);
    select_edges(&mut m, &[e]);
    let r = ok(&mut m, MeshOp::Bevel { width: 0.25 });
    // The grid's u runs 0 to 1 across 2 cells, so u = x / 2; the strip's
    // corners follow the faces they join.
    let poly = m.face_poly(r.new_faces[0]);
    for (x, c) in poly.verts.iter().zip(&poly.corners) {
        let p = m.position(*x);
        assert!(close(c.uv[0], p[0] / 2.0), "{p:?} has u {}", c.uv[0]);
        assert!(close(c.uv[1], p[1]), "{p:?} has v {}", c.uv[1]);
    }
}

#[test]
fn bevel_refuses_what_it_cannot_cut() {
    // Two edges at one vertex.
    let mut m = fixtures::cube(2.0);
    let (e0, e1) = (edge(&m, 0, 1), edge(&m, 0, 2));
    select_edges(&mut m, &[e0, e1]);
    refused(&mut m, MeshOp::Bevel { width: 0.25 }, bad("bevelled edges share a vertex"));

    // A boundary edge has one face.
    let mut m = fixtures::grid(1, 1, 1.0);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    refused(&mut m, MeshOp::Bevel { width: 0.1 }, MeshError::NotBoundary);

    // A width of nothing, and no selection.
    let mut m = fixtures::cube(2.0);
    refused(&mut m, MeshOp::Bevel { width: 0.25 }, MeshError::NothingSelected);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    refused(&mut m, MeshOp::Bevel { width: 0.0 }, bad("width: more than zero"));
}

#[test]
fn two_far_apart_edges_bevel_together() {
    let mut m = fixtures::cube(2.0);
    let (a, b) = (edge(&m, 0, 1), edge(&m, 6, 7));
    select_edges(&mut m, &[a, b]);
    ok(&mut m, MeshOp::Bevel { width: 0.25 });
    assert_eq!(m.counts(), (12, 18, 8));
    assert_eq!(euler(&m), 2);
}

#[test]
fn the_new_ops_read_from_json() {
    let ops: Vec<MeshOp> = serde_json::from_str(
        r#"[
            {"op": "tris_to_quads"},
            {"op": "inset", "thickness": 0.1},
            {"op": "inset", "thickness": 0.1, "depth": -0.2, "individual": true},
            {"op": "bevel", "width": 0.05}
        ]"#,
    )
    .unwrap();
    assert_eq!(ops[0], MeshOp::TrisToQuads { angle: 1.0 });
    assert_eq!(ops[1], MeshOp::Inset { thickness: 0.1, depth: 0.0, individual: false });
    assert_eq!(ops[2], MeshOp::Inset { thickness: 0.1, depth: -0.2, individual: true });
    assert_eq!(ops[3], MeshOp::Bevel { width: 0.05 });
}
