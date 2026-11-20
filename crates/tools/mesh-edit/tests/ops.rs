//! Each operation's counts (docs/editor/modeling.md, "Operations") on a
//! cube, a plane grid, an open shell and a three-face edge, with the mesh
//! valid after every op.

mod common;

use common::*;
use mesh_edit::fixtures::{self, Caps};
use mesh_edit::{
    Affine, DeleteWhat, FaceId, MergeAt, Mesh, MeshError, MeshOp, Poly, SelectMode, Skin, VertId,
};

const UP: [f32; 3] = [0.0, 0.0, 1.0];

#[test]
fn fixtures_are_valid_with_their_counts() {
    let cases: [(Mesh, (usize, usize, usize)); 8] = [
        (fixtures::cube(1.0), (8, 12, 6)),
        (fixtures::open_box(1.0), (8, 12, 5)),
        (fixtures::grid(2, 2, 1.0), (9, 12, 4)),
        (fixtures::grid(3, 1, 1.0), (8, 10, 3)),
        (fixtures::cylinder(8, 1, 1.0, 2.0, Caps::Open), (16, 24, 8)),
        (fixtures::cylinder(8, 1, 1.0, 2.0, Caps::Fan), (18, 40, 24)),
        (fixtures::cylinder(8, 1, 1.0, 2.0, Caps::Ngon), (16, 24, 10)),
        (fixtures::fin(), (8, 10, 3)),
    ];
    for (m, counts) in cases {
        m.assert_valid();
        assert_eq!(m.counts(), counts);
    }
    let fin = fixtures::fin();
    assert_eq!(fin.edge_face_count(edge(&fin, 0, 1)), 3);
}

#[test]
fn extrude_one_face_of_a_cube() {
    let mut m = fixtures::cube(1.0);
    select(&mut m, SelectMode::Face, &[1]);
    let r = ok(&mut m, MeshOp::ExtrudeFaces { offset: UP });
    assert_eq!(m.counts(), (12, 20, 10));
    assert_eq!(euler(&m), 2);
    assert_eq!((r.new_verts.len(), r.new_edges.len(), r.new_faces.len()), (4, 8, 4));
    assert_eq!(r.selected, 1);
    // The top face keeps its id and moved up; the sides face outward.
    assert!(m.selection().has_face(FaceId(1)));
    for v in m.face_verts(FaceId(1)) {
        assert!(close(m.position(v)[2], 1.5));
    }
    assert!(close3(m.face_normal(FaceId(1)), UP));
    for &f in &r.new_faces {
        let c = m.face_center(f);
        let n = m.face_normal(f);
        assert!(n[0] * c[0] + n[1] * c[1] > 0.0, "side {f:?} faces in");
        assert_eq!(m.face_material(f), 1, "a side takes the extruded face's material");
    }
}

#[test]
fn extrude_a_grid_region_keeps_its_interior_vert() {
    let mut m = fixtures::grid(2, 2, 1.0);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::ExtrudeFaces { offset: UP });
    // |Vb| = 8, |B| = 8: V +8, E +16, F +8.
    assert_eq!(m.counts(), (17, 28, 12));
    assert!(m.is_vert(v(4)), "the interior vert stays the region's");
    assert!(close(m.position(v(4))[2], 1.0));
}

#[test]
fn extrude_on_an_open_shell_and_a_three_face_edge() {
    let mut m = fixtures::open_box(1.0);
    select(&mut m, SelectMode::Face, &[0]);
    ok(&mut m, MeshOp::ExtrudeFaces { offset: [0.0, 0.0, -1.0] });
    assert_eq!(m.counts(), (12, 20, 9));

    let mut m = fixtures::fin();
    select(&mut m, SelectMode::Face, &[2]);
    ok(&mut m, MeshOp::ExtrudeFaces { offset: [1.0, 0.0, 0.0] });
    assert_eq!(m.counts(), (12, 18, 7));
    // The floor halves and a side quad now share the old edge.
    assert_eq!(m.edge_face_count(edge(&m, 0, 1)), 3);
}

#[test]
fn extrude_with_nothing_selected_changes_nothing() {
    let mut m = fixtures::cube(1.0);
    refused(&mut m, MeshOp::ExtrudeFaces { offset: UP }, MeshError::NothingSelected);
}

#[test]
fn extrude_edges() {
    let mut m = fixtures::grid(1, 1, 1.0);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    let r = ok(&mut m, MeshOp::ExtrudeEdges { offset: [0.0, -1.0, 0.0] });
    assert_eq!(m.counts(), (6, 7, 2));
    let quad = r.new_faces[0];
    assert!(close3(m.face_normal(quad), UP), "the quad continues the grid's winding");
    // Its corners copy the grid's coordinates along the edge.
    let poly = m.face_poly(quad);
    let at0 = poly.verts.iter().position(|&x| x == v(0)).unwrap();
    assert_eq!(poly.corners[at0].uv, [0.0, 0.0]);
    assert_eq!(poly.corners[at0].src, None);

    // A wire edge.
    let mut m = Mesh::new();
    let a = m.add_vert([0.0; 3], None);
    let b = m.add_vert([1.0, 0.0, 0.0], None);
    let e = m.add_edge(a, b).unwrap();
    select_edges(&mut m, &[e]);
    ok(&mut m, MeshOp::ExtrudeEdges { offset: [0.0, 1.0, 0.0] });
    assert_eq!(m.counts(), (4, 4, 1));

    // An edge with two faces has no room.
    let mut m = fixtures::cube(1.0);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    refused(&mut m, MeshOp::ExtrudeEdges { offset: UP }, MeshError::NotBoundary);
}

#[test]
fn duplicate_makes_a_disconnected_copy() {
    let mut m = fixtures::cube(1.0);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    ok(&mut m, MeshOp::SelectAll);
    let r = ok(&mut m, MeshOp::Duplicate { offset: [3.0, 0.0, 0.0] });
    assert_eq!(m.counts(), (16, 24, 12));
    assert_eq!(r.selected, 6);
    for &f in &r.new_faces {
        assert!(m.selection().has_face(f));
        for c in m.face_corners(f) {
            assert_eq!(c.src, None);
        }
    }
    for &x in &r.new_verts {
        assert!(m.position(x)[0] > 2.0);
        assert!(m.vert_neighbors(x).iter().all(|n| r.new_verts.contains(n)));
    }
}

#[test]
fn delete_each_kind() {
    let mut m = fixtures::cube(1.0);
    select(&mut m, SelectMode::Face, &[1]);
    ok(&mut m, MeshOp::Delete { what: DeleteWhat::Faces });
    assert_eq!(m.counts(), (8, 12, 5));

    let mut m = fixtures::grid(1, 1, 1.0);
    select(&mut m, SelectMode::Face, &[0]);
    ok(&mut m, MeshOp::Delete { what: DeleteWhat::Faces });
    assert_eq!(m.counts(), (0, 0, 0));

    let mut m = fixtures::grid(1, 1, 1.0);
    select(&mut m, SelectMode::Face, &[0]);
    ok(&mut m, MeshOp::Delete { what: DeleteWhat::OnlyFaces });
    assert_eq!(m.counts(), (4, 4, 0));

    let mut m = fixtures::cube(1.0);
    select(&mut m, SelectMode::Vert, &[0]);
    ok(&mut m, MeshOp::Delete { what: DeleteWhat::Verts });
    assert_eq!(m.counts(), (7, 9, 3));

    let mut m = fixtures::cube(1.0);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    ok(&mut m, MeshOp::Delete { what: DeleteWhat::Edges });
    assert_eq!(m.counts(), (8, 11, 4));

    // The wall of the fin: its own edges and top verts go, the shared
    // edge stays with the floor.
    let mut m = fixtures::fin();
    select(&mut m, SelectMode::Face, &[2]);
    ok(&mut m, MeshOp::Delete { what: DeleteWhat::Faces });
    assert_eq!(m.counts(), (6, 7, 2));
    assert_eq!(m.edge_face_count(edge(&m, 0, 1)), 2);
}

#[test]
fn dissolve_edges_verts_and_faces() {
    // grid(2, 1): verts (i, j) are j * 3 + i.
    let mut m = fixtures::grid(2, 1, 1.0);
    let e = edge(&m, 1, 4);
    select_edges(&mut m, &[e]);
    let r = ok(&mut m, MeshOp::Dissolve);
    assert_eq!(m.counts(), (6, 6, 1));
    assert_eq!(m.face_len(FaceId(0)), 6);
    assert_eq!(r.new_faces.len(), 0, "the joined face keeps the first face's id");

    // Its two-edge verts now dissolve: V -1, E -1 each.
    select(&mut m, SelectMode::Vert, &[1]);
    ok(&mut m, MeshOp::Dissolve);
    assert_eq!(m.counts(), (5, 5, 1));

    // A vert with three edges is left alone.
    let mut m = fixtures::grid(2, 1, 1.0);
    select(&mut m, SelectMode::Vert, &[1]);
    ok(&mut m, MeshOp::Dissolve);
    assert_eq!(m.counts(), (6, 7, 2));

    // Face mode: the edge between the two selected faces.
    let mut m = fixtures::grid(2, 1, 1.0);
    select(&mut m, SelectMode::Face, &[0, 1]);
    ok(&mut m, MeshOp::Dissolve);
    assert_eq!(m.counts(), (6, 6, 1));

    // The fin's shared edge has three faces: nothing joins.
    let mut m = fixtures::fin();
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    ok(&mut m, MeshOp::Dissolve);
    assert_eq!(m.counts(), (8, 10, 3));
}

#[test]
fn merge_at_centre_first_last_and_cursor() {
    for (at, survivor, pos) in [
        (MergeAt::Center, 0, [0.5, 0.0, 0.0]),
        (MergeAt::First, 0, [0.0, 0.0, 0.0]),
        (MergeAt::Last, 1, [1.0, 0.0, 0.0]),
        (MergeAt::Cursor([5.0, 5.0, 5.0]), 0, [5.0, 5.0, 5.0]),
    ] {
        let mut m = fixtures::grid(1, 1, 1.0);
        select(&mut m, SelectMode::Vert, &[0, 1]);
        ok(&mut m, MeshOp::Merge { at });
        assert_eq!(m.counts(), (3, 3, 1), "{at:?}");
        assert!(m.is_vert(v(survivor)));
        assert!(!m.is_vert(v(1 - survivor)));
        assert!(close3(m.position(v(survivor)), pos), "{at:?}");
        assert_eq!(m.face_len(FaceId(0)), 3, "the quad keeps its id as a triangle");
    }
}

#[test]
fn merge_blends_skins_to_255() {
    let mut m = Mesh::new();
    let a = m.add_vert([0.0, 0.0, 0.0], Some(Skin::rigid(1)));
    let b = m.add_vert([1.0, 0.0, 0.0], Some(Skin::rigid(2)));
    let c = m.add_vert([1.0, 1.0, 0.0], Some(Skin::rigid(1)));
    let d = m.add_vert([0.0, 1.0, 0.0], Some(Skin::rigid(1)));
    m.add_face(&Poly::new(vec![a, b, c, d])).unwrap();
    select(&mut m, SelectMode::Vert, &[a.0, b.0]);
    ok(&mut m, MeshOp::Merge { at: MergeAt::Center });
    let skin = m.skin(a).unwrap();
    assert_eq!(skin.total(), 255);
    let nodes: Vec<u8> = (0..4).filter(|&k| skin.weights[k] > 0).map(|k| skin.nodes[k]).collect();
    assert_eq!(nodes.len(), 2);
    assert!(nodes.contains(&1) && nodes.contains(&2));
}

/// Two quads side by side whose seam verts are doubled, with a UV seam:
/// the left quad has u = 1 on the seam, the right u = 0.
fn seamed_quads() -> Mesh {
    let mut m = Mesh::new();
    let p = [
        [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0], [1.0, 1.0, 0.0],
    ];
    let ids: Vec<VertId> = p.iter().map(|&x| m.add_vert(x, None)).collect();
    for (q, material) in [([0, 1, 2, 3], 0u16), ([4, 5, 6, 7], 1)] {
        let mut poly = Poly::new(q.iter().map(|&i| ids[i]).collect());
        poly.corners = q
            .iter()
            .map(|&i| fixtures::corner(p[i][0] - f32::from(material), p[i][1], UP))
            .collect();
        poly.material = material;
        m.add_face(&poly).unwrap();
    }
    m
}

#[test]
fn weld_by_distance_keeps_corner_attributes() {
    let mut m = seamed_quads();
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::Weld { distance: 1e-4 });
    assert_eq!(m.counts(), (6, 7, 2));
    // The seam verts are 1 and 2 (lowest ids); each face keeps its UVs.
    for (f, u) in [(FaceId(0), 1.0), (FaceId(1), 0.0)] {
        let poly = m.face_poly(f);
        for seam in [v(1), v(2)] {
            let k = poly.verts.iter().position(|&x| x == seam).expect("face uses the seam");
            assert_eq!(poly.corners[k].uv[0], u);
        }
    }
    // Nothing within half a unit on a unit grid.
    let mut m = fixtures::grid(1, 1, 1.0);
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::Weld { distance: 0.5 });
    assert_eq!(m.counts(), (4, 4, 1));
}

#[test]
fn weld_drops_a_doubled_face() {
    let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let positions: Vec<[f32; 3]> = p.iter().chain(p.iter()).copied().collect();
    let mut m = Mesh::from_polygons(&positions, &[vec![0, 1, 2], vec![3, 4, 5]]).unwrap();
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::Weld { distance: 1e-4 });
    assert_eq!(m.counts(), (3, 3, 1));
}

#[test]
fn fill_a_hole_and_loose_verts() {
    let mut m = fixtures::open_box(1.0);
    let rim: Vec<_> = [(4, 5), (5, 7), (7, 6), (6, 4)].iter().map(|&(a, b)| edge(&m, a, b)).collect();
    select_edges(&mut m, &rim);
    let r = ok(&mut m, MeshOp::Fill { verts: None });
    assert_eq!(m.counts(), (8, 12, 6));
    assert!(close3(m.face_normal(r.new_faces[0]), UP), "the lid faces out");

    let mut m = Mesh::new();
    let ids: Vec<VertId> =
        [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].iter().map(|&p| m.add_vert(p, None)).collect();
    ok(&mut m, MeshOp::Fill { verts: Some(ids) });
    assert_eq!(m.counts(), (3, 3, 1));

    let mut m = fixtures::cube(1.0);
    let e = edge(&m, 0, 1);
    select_edges(&mut m, &[e]);
    let before = m.clone();
    assert!(matches!(m.apply(&MeshOp::Fill { verts: None }), Err(MeshError::BadInput(_))));
    assert_eq!(m, before);
}

#[test]
fn flip_then_recalc_normals_restores_outward() {
    let mut m = fixtures::cube(1.0);
    select(&mut m, SelectMode::Face, &[3]);
    let before = m.face_normal(FaceId(3));
    ok(&mut m, MeshOp::Flip);
    assert!(close3(m.face_normal(FaceId(3)), before.map(|x| -x)));
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::RecalcNormals);
    for f in m.face_ids().collect::<Vec<_>>() {
        let (n, c) = (m.face_normal(f), m.face_center(f));
        assert!(n[0] * c[0] + n[1] * c[1] + n[2] * c[2] > 0.0, "{f:?} faces in");
        for corner in m.face_corners(f) {
            assert!(close3(corner.normal, n), "a flat cube's fans are single corners");
        }
    }

    // An open shell follows its corner normals.
    let mut m = fixtures::open_box(1.0);
    select(&mut m, SelectMode::Face, &[0]);
    ok(&mut m, MeshOp::Flip);
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::RecalcNormals);
    assert!(close3(m.face_normal(FaceId(0)), [0.0, 0.0, -1.0]));
}

#[test]
fn triangulate_counts() {
    let mut m = fixtures::cube(1.0);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::Triangulate);
    assert_eq!(m.counts(), (8, 18, 12));

    let mut m = fixtures::cylinder(8, 1, 1.0, 1.0, Caps::Ngon);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    ok(&mut m, MeshOp::SelectMaterial { material: 1 });
    ok(&mut m, MeshOp::SelectMaterial { material: 2 });
    ok(&mut m, MeshOp::Triangulate);
    assert_eq!(m.counts(), (16, 34, 20));

    // A concave L: ear clipping keeps every triangle facing up.
    let l = [
        [0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0],
        [1.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 2.0, 0.0],
    ];
    let mut m = Mesh::from_polygons(&l, &[vec![0, 1, 2, 3, 4, 5]]).unwrap();
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::Triangulate);
    assert_eq!(m.face_count(), 4);
    for f in m.face_ids().collect::<Vec<_>>() {
        assert!(close3(m.face_normal(f), UP));
    }
}

#[test]
fn transform_moves_the_selection_about_its_centre() {
    let mut m = fixtures::cube(2.0);
    select(&mut m, SelectMode::Face, &[1]);
    let scale = Affine::scale([2.0, 2.0, 1.0]);
    ok(&mut m, MeshOp::Transform { matrix: scale.rows, pivot: None });
    // The top's centre is (0, 0, 1); its corners go from +-1 to +-2.
    for x in m.face_verts(FaceId(1)) {
        let p = m.position(x);
        assert!(close(p[0].abs(), 2.0) && close(p[1].abs(), 2.0) && close(p[2], 1.0));
    }
    // The bottom did not move.
    for x in m.face_verts(FaceId(0)) {
        assert!(close(m.position(x)[0].abs(), 1.0));
    }
    let mut empty = fixtures::cube(1.0);
    refused(
        &mut empty,
        MeshOp::Transform { matrix: Affine::IDENTITY.rows, pivot: None },
        MeshError::NothingSelected,
    );
}

#[test]
fn set_material() {
    let mut m = fixtures::cube(1.0);
    select(&mut m, SelectMode::Face, &[2, 3]);
    ok(&mut m, MeshOp::SetMaterial { material: 9 });
    assert_eq!(m.face_material(FaceId(2)), 9);
    assert_eq!(m.face_material(FaceId(3)), 9);
    assert_eq!(m.face_material(FaceId(4)), 4);
}
