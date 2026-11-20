//! Import from a section's triangles and stable export
//! (docs/editor/modeling.md, "Import and export").

mod common;

use common::*;
use mesh_edit::fixtures;
use mesh_edit::{Affine, DeleteWhat, Mesh, MeshError, MeshOp, SelectMode, dirty};

#[test]
fn import_welds_shared_positions() {
    let (m, report) = Mesh::from_triangles(fixtures::cube_source(1.0)).unwrap();
    m.assert_valid();
    assert_eq!(report.welded, 16);
    assert_eq!(report.passthrough, 0);
    assert_eq!(m.counts(), (8, 18, 12));
    for f in m.face_ids().collect::<Vec<_>>() {
        assert_eq!(m.face_src(f), Some(f.0));
        for c in m.face_corners(f) {
            assert!(c.src.is_some());
            assert_eq!(c.dirty, 0);
        }
    }
}

#[test]
fn an_unedited_import_exports_its_source() {
    let source = fixtures::cube_source(1.0);
    let (m, _) = Mesh::from_triangles(source.clone()).unwrap();
    let out = m.export();
    assert_eq!(out.positions, source.positions);
    assert_eq!(out.triangles, source.triangles);
    assert_eq!(out.groups, source.groups);
    assert_eq!(out.materials, source.materials);
    assert_eq!(out.corners.len(), source.corners.len());
    for (i, (got, want)) in out.corners.iter().zip(&source.corners).enumerate() {
        assert_eq!(got.src, Some(i as u32));
        assert_eq!(got.dirty, 0, "vertex {i}");
        assert_eq!((got.uv, got.lightmap, got.normal), (want.uv, want.lightmap, want.normal));
        assert_eq!((got.tangent, got.binormal, got.light_dir), (want.tangent, want.binormal, want.light_dir));
    }
    assert!(out.vert_ids.iter().all(Option::is_some));
}

#[test]
fn moving_verts_keeps_the_source_order() {
    let source = fixtures::cube_source(1.0);
    let (mut m, _) = Mesh::from_triangles(source.clone()).unwrap();
    let top: Vec<u32> = m.vert_ids().filter(|&x| m.position(x)[2] > 0.0).map(|x| x.0).collect();
    assert_eq!(top.len(), 4);
    select(&mut m, SelectMode::Vert, &top);
    let up = Affine::translate([0.0, 0.0, 1.0]);
    ok(&mut m, MeshOp::Transform { matrix: up.rows, pivot: None });

    let out = m.export();
    assert_eq!(out.positions.len(), source.positions.len(), "no vertex split");
    assert_eq!(out.triangles, source.triangles);
    for (i, p) in source.positions.iter().enumerate() {
        let moved = p[2] > 0.0;
        let want = if moved { p[2] + 1.0 } else { p[2] };
        assert!(close(out.positions[i][2], want), "vertex {i}");
        assert_eq!(out.corners[i].dirty & dirty::POSITION != 0, moved, "vertex {i}");
    }
}

#[test]
fn deleting_a_face_keeps_the_others_in_place() {
    let source = fixtures::cube_source(1.0);
    let (mut m, _) = Mesh::from_triangles(source.clone()).unwrap();
    // Source triangles 2 and 3 are the top.
    select(&mut m, SelectMode::Face, &[2, 3]);
    ok(&mut m, MeshOp::Delete { what: DeleteWhat::OnlyFaces });
    let out = m.export();
    let mut want = source.triangles.clone();
    want.drain(2..4);
    assert_eq!(out.triangles, want);
    assert_eq!(out.positions.len(), source.positions.len());
}

#[test]
fn new_faces_follow_their_group() {
    let source = fixtures::cube_source(1.0);
    let (mut m, _) = Mesh::from_triangles(source.clone()).unwrap();
    select(&mut m, SelectMode::Face, &[2, 3]);
    ok(&mut m, MeshOp::ExtrudeFaces { offset: [0.0, 0.0, 1.0] });
    let out = m.export();
    // The source triangles keep their indices; everything indexes a real
    // vertex; group runs stay contiguous.
    assert_eq!(out.triangles[..4], source.triangles[..4], "group 0 leads with its source triangles");
    for tri in &source.triangles {
        assert!(out.triangles.contains(tri));
    }
    let n = out.positions.len() as u32;
    assert!(out.triangles.iter().flatten().all(|&i| i < n));
    assert_eq!(out.triangles.len(), out.groups.len());
    assert_eq!(out.triangles.len(), out.materials.len());
    let mut seen = Vec::new();
    for &g in &out.groups {
        if seen.last() != Some(&g) {
            assert!(!seen.contains(&g), "group {g} split in two runs");
            seen.push(g);
        }
    }
    // The 12 source triangles (the top's two still match theirs, on the
    // moved copies) and 8 for the four side quads.
    assert_eq!(out.triangles.len(), 20);
}

#[test]
fn degenerate_and_doubled_triangles_pass_through() {
    let mut source = fixtures::cube_source(1.0);
    let first = source.triangles[0];
    source.triangles.push([0, 0, 1]);
    source.triangles.push(first);
    source.groups.extend([5, 5]);
    source.materials.extend([7, 7]);
    let (m, report) = Mesh::from_triangles(source.clone()).unwrap();
    m.assert_valid();
    assert_eq!(report.passthrough, 2);
    assert_eq!(m.face_count(), 12);
    let out = m.export();
    assert_eq!(out.triangles, source.triangles);
    assert_eq!(out.groups, source.groups);
    assert_eq!(out.materials, source.materials);
}

#[test]
fn bad_sources_are_refused() {
    let mut source = fixtures::cube_source(1.0);
    source.triangles.push([0, 1, 99]);
    source.groups.push(0);
    source.materials.push(0);
    assert!(matches!(Mesh::from_triangles(source), Err(MeshError::BadInput(_))));

    let mut source = fixtures::cube_source(1.0);
    source.corners.pop();
    assert!(matches!(Mesh::from_triangles(source), Err(MeshError::BadInput(_))));

    assert!(matches!(
        Mesh::from_polygons(&[[0.0; 3]], &[vec![0, 1, 2]]),
        Err(MeshError::BadInput(_))
    ));
}

#[test]
fn polygons_import_without_a_source() {
    let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [2.0, 0.5, 0.0]];
    let m = Mesh::from_polygons(&p, &[vec![0, 1, 2, 3], vec![1, 4, 2]]).unwrap();
    m.assert_valid();
    assert_eq!(m.counts(), (5, 6, 2));
    assert!(m.source().is_none());
    let out = m.export();
    assert_eq!(out.triangles.len(), 3);
    assert_eq!(out.positions.len(), 5);
}
