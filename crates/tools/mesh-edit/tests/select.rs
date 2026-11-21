//! Selection modes, flushing, growing, and loop and ring walks.

mod common;

use common::*;
use mesh_edit::fixtures::{self, Caps};
use mesh_edit::{FaceId, MeshOp, SelectHow, SelectMode};

#[test]
fn modes_convert_the_selection() {
    let mut m = fixtures::cube(1.0);
    // The bottom four verts select the bottom face and its four edges.
    select(&mut m, SelectMode::Vert, &[0, 1, 2, 3]);
    assert_eq!(m.selection().len(), 4);
    assert_eq!(m.selection().edges().count(), 4);
    assert_eq!(m.selection().faces().collect::<Vec<_>>(), vec![FaceId(0)]);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    assert_eq!(m.selection().len(), 1);

    // Three verts make no face.
    select(&mut m, SelectMode::Vert, &[0, 1, 2]);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    assert!(m.selection().is_empty());

    // A face selects its verts and edges.
    select(&mut m, SelectMode::Face, &[1]);
    assert_eq!(m.selection().verts().count(), 4);
    assert_eq!(m.selection().edges().count(), 4);
}

#[test]
fn add_remove_toggle_and_invert() {
    let mut m = fixtures::cube(1.0);
    select(&mut m, SelectMode::Face, &[0]);
    ok(&mut m, MeshOp::Select { ids: vec![1], how: SelectHow::Add });
    assert_eq!(m.selection().len(), 2);
    ok(&mut m, MeshOp::Select { ids: vec![0, 2], how: SelectHow::Toggle });
    let faces: Vec<_> = m.selection().faces().collect();
    assert_eq!(faces, vec![FaceId(1), FaceId(2)]);
    ok(&mut m, MeshOp::Select { ids: vec![1], how: SelectHow::Remove });
    assert_eq!(m.selection().faces().collect::<Vec<_>>(), vec![FaceId(2)]);
    // Removing a face leaves none of its verts behind in face mode.
    assert_eq!(m.selection().verts().count(), 4);
    ok(&mut m, MeshOp::SelectInvert);
    assert_eq!(m.selection().len(), 5);
    ok(&mut m, MeshOp::SelectNone);
    assert!(m.selection().is_empty());
}

#[test]
fn more_less_and_linked() {
    let mut m = fixtures::cube(1.0);
    select(&mut m, SelectMode::Face, &[0]);
    ok(&mut m, MeshOp::SelectMore);
    assert_eq!(m.selection().len(), 5, "the bottom and its four sides");
    ok(&mut m, MeshOp::SelectLess);
    assert_eq!(m.selection().faces().collect::<Vec<_>>(), vec![FaceId(0)]);

    // Two cubes: linked stays on one.
    let mut m = fixtures::cube(1.0);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    ok(&mut m, MeshOp::SelectAll);
    ok(&mut m, MeshOp::Duplicate { offset: [3.0, 0.0, 0.0] });
    select(&mut m, SelectMode::Face, &[0]);
    ok(&mut m, MeshOp::SelectLinked);
    assert_eq!(m.selection().len(), 6);
    assert!(m.selection().faces().all(|f| f.0 < 6));
}

#[test]
fn material_and_group_add_to_the_selection() {
    let mut m = fixtures::cube(1.0);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Face });
    ok(&mut m, MeshOp::SelectMaterial { material: 2 });
    ok(&mut m, MeshOp::SelectMaterial { material: 5 });
    assert_eq!(m.selection().faces().collect::<Vec<_>>(), vec![FaceId(2), FaceId(5)]);
    ok(&mut m, MeshOp::SelectGroup { group: 0 });
    assert_eq!(m.selection().len(), 6);
}

#[test]
fn edge_loops_stop_at_poles_boundaries_and_three_face_edges() {
    // grid(3, 3): vert (i, j) is j * 4 + i.
    let m = fixtures::grid(3, 3, 1.0);
    assert_eq!(m.edge_loop(edge(&m, 4, 5)).len(), 3, "an interior row ends at the boundary");
    assert_eq!(m.edge_loop(edge(&m, 0, 1)).len(), 12, "a boundary loop goes all the way round");

    let m = fixtures::cylinder(8, 1, 1.0, 1.0, Caps::Fan);
    assert_eq!(m.edge_loop(edge(&m, 0, 8)).len(), 3, "up the side, stopping at both poles");
    assert_eq!(m.edge_loop(edge(&m, 8, 9)).len(), 8, "a ring closes");

    let m = fixtures::fin();
    assert_eq!(m.edge_loop(edge(&m, 0, 1)), vec![edge(&m, 0, 1)]);
}

#[test]
fn edge_rings_cross_quads_only() {
    let m = fixtures::cylinder(8, 1, 1.0, 1.0, Caps::Fan);
    assert_eq!(m.edge_ring(edge(&m, 0, 8)).len(), 8, "round the side, closed");
    assert_eq!(m.edge_ring(edge(&m, 0, 1)).len(), 2, "the fan triangles end it");

    let m = fixtures::grid(3, 3, 1.0);
    assert_eq!(m.edge_ring(edge(&m, 0, 1)).len(), 4);

    let m = fixtures::fin();
    assert_eq!(m.edge_ring(edge(&m, 2, 3)).len(), 2, "it reaches the three-face edge and stops");
}

#[test]
fn select_loop_op() {
    let mut m = fixtures::grid(3, 3, 1.0);
    let e = edge(&m, 4, 5);
    ok(&mut m, MeshOp::SelectLoop { edge: e, ring: false });
    assert_eq!(m.selection().mode(), SelectMode::Edge);
    assert_eq!(m.selection().len(), 3);
    ok(&mut m, MeshOp::SelectLoop { edge: e, ring: true });
    assert_eq!(m.selection().len(), 4);
}
