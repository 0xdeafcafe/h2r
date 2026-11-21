//! The op log: replaying it reproduces the mesh, it survives JSON, and a
//! seeded random op sequence keeps every invariant, with each failed op
//! changing nothing.

mod common;

use mesh_edit::fixtures::{self, Caps};
use mesh_edit::{Affine, DeleteWhat, EdgeId, MergeAt, Mesh, MeshOp, SelectHow, SelectMode, VertId};

fn sample_log() -> Vec<MeshOp> {
    vec![
        MeshOp::Mode { mode: SelectMode::Face },
        MeshOp::Select { ids: vec![2, 3], how: SelectHow::Set },
        MeshOp::ExtrudeFaces { offset: [0.0, 0.0, 1.0] },
        MeshOp::Transform { matrix: Affine::scale([0.5, 0.5, 1.0]).rows, pivot: None },
        MeshOp::SelectMore,
        MeshOp::Flip,
        MeshOp::SelectAll,
        MeshOp::RecalcNormals,
        MeshOp::LoopCut { edge: EdgeId(0), cuts: 1, factor: 0.5 },
        MeshOp::Mode { mode: SelectMode::Vert },
        MeshOp::Select { ids: vec![0, 1], how: SelectHow::Set },
        MeshOp::Merge { at: MergeAt::Center },
    ]
}

#[test]
fn replaying_the_log_reproduces_the_mesh() {
    let (mut live, _) = Mesh::from_triangles(fixtures::cube_source(1.0)).unwrap();
    let start = live.clone();
    let mut log = Vec::new();
    for op in sample_log() {
        // The session logs only ops that applied.
        if live.apply(&op).is_ok() {
            live.assert_valid();
            log.push(op);
        }
    }
    assert!(log.len() >= 8, "the sample should mostly apply: {log:?}");
    let mut replay = start.clone();
    replay.apply_all(&log).unwrap();
    assert_eq!(replay, live);
    assert_eq!(replay.export(), live.export());

    // Through JSON, as `doc.diff` prints it and agents send it.
    let json = serde_json::to_string(&log).unwrap();
    let back: Vec<MeshOp> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, log);
    let mut replay = start;
    replay.apply_all(&back).unwrap();
    assert_eq!(replay, live);
}

#[test]
fn ops_read_from_hand_written_json() {
    let ops: Vec<MeshOp> = serde_json::from_str(
        r#"[
            {"op": "mode", "mode": "face"},
            {"op": "select", "ids": [1]},
            {"op": "extrude_faces", "offset": [0.0, 0.0, 1.0]},
            {"op": "select_none"},
            {"op": "loop_cut", "edge": 0},
            {"op": "delete", "what": "only_faces"},
            {"op": "merge", "at": "center"},
            {"op": "merge", "at": {"cursor": [1.0, 2.0, 3.0]}}
        ]"#,
    )
    .unwrap();
    assert_eq!(ops[1], MeshOp::Select { ids: vec![1], how: SelectHow::Set });
    assert_eq!(ops[4], MeshOp::LoopCut { edge: EdgeId(0), cuts: 1, factor: 0.5 });
    assert_eq!(ops[5], MeshOp::Delete { what: DeleteWhat::OnlyFaces });
    assert_eq!(ops[7], MeshOp::Merge { at: MergeAt::Cursor([1.0, 2.0, 3.0]) });
    let mut m = fixtures::cube(1.0);
    m.apply_all(&ops[..5]).unwrap();
    m.assert_valid();
    // Edge 0 (verts 0-2) rings round the extruded box through six quads.
    assert_eq!(m.face_count(), 10 + 6);
}

/// A small deterministic generator (a 64-bit LCG), so failures replay.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { self.next() as usize % n }
    }

    fn unit(&mut self) -> f32 {
        self.next() as f32 / (1u64 << 31) as f32
    }

    fn offset(&mut self) -> [f32; 3] {
        [self.unit() - 0.5, self.unit() - 0.5, self.unit() - 0.5]
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        if items.is_empty() { None } else { Some(items[self.below(items.len())]) }
    }
}

fn random_op(rng: &mut Rng, m: &Mesh) -> MeshOp {
    let verts: Vec<VertId> = m.vert_ids().collect();
    let edges: Vec<EdgeId> = m.edge_ids().collect();
    let faces: Vec<u32> = m.face_ids().map(|f| f.0).collect();
    let some_ids = |rng: &mut Rng, ids: &[u32]| -> Vec<u32> {
        (0..1 + rng.below(4)).filter_map(|_| rng.pick(ids)).collect()
    };
    let vert_ids: Vec<u32> = verts.iter().map(|v| v.0).collect();
    let edge_ids: Vec<u32> = edges.iter().map(|e| e.0).collect();
    match rng.below(26) {
        0 => MeshOp::Mode { mode: [SelectMode::Vert, SelectMode::Edge, SelectMode::Face][rng.below(3)] },
        1 | 2 => {
            let ids = match m.selection().mode() {
                SelectMode::Vert => some_ids(rng, &vert_ids),
                SelectMode::Edge => some_ids(rng, &edge_ids),
                SelectMode::Face => some_ids(rng, &faces),
            };
            let how = [SelectHow::Set, SelectHow::Add, SelectHow::Remove, SelectHow::Toggle][rng.below(4)];
            MeshOp::Select { ids, how }
        }
        3 => MeshOp::SelectMore,
        4 => MeshOp::SelectLess,
        5 => MeshOp::SelectLinked,
        6 => MeshOp::SelectInvert,
        7 => MeshOp::SelectLoop { edge: rng.pick(&edges).unwrap_or(EdgeId(0)), ring: rng.below(2) == 0 },
        8 => MeshOp::Transform {
            matrix: Affine::rotate([0.0, 0.0, 1.0], rng.unit()).rows,
            pivot: if rng.below(2) == 0 { None } else { Some(rng.offset()) },
        },
        9 => MeshOp::Transform { matrix: Affine::translate(rng.offset()).rows, pivot: None },
        10 | 11 => MeshOp::ExtrudeFaces { offset: rng.offset() },
        12 => MeshOp::ExtrudeEdges { offset: rng.offset() },
        13 => MeshOp::Duplicate { offset: rng.offset() },
        14 => MeshOp::Delete {
            what: [DeleteWhat::Faces, DeleteWhat::Edges, DeleteWhat::Verts, DeleteWhat::OnlyFaces][rng.below(4)],
        },
        15 => MeshOp::Dissolve,
        16 => MeshOp::Merge {
            at: [MergeAt::Center, MergeAt::First, MergeAt::Last, MergeAt::Cursor(rng.offset())][rng.below(4)],
        },
        17 => MeshOp::Weld { distance: rng.unit() * 0.5 },
        18 | 19 => MeshOp::LoopCut {
            edge: rng.pick(&edges).unwrap_or(EdgeId(0)),
            cuts: 1 + rng.below(2) as u32,
            factor: rng.unit(),
        },
        20 => MeshOp::Subdivide { cuts: 1 + rng.below(2) as u32 },
        21 => MeshOp::Fill {
            verts: if rng.below(2) == 0 {
                None
            } else {
                Some((0..3 + rng.below(2)).filter_map(|_| rng.pick(&verts)).collect())
            },
        },
        22 => MeshOp::Bridge {
            a: rng.pick(&edges).unwrap_or(EdgeId(0)),
            b: rng.pick(&edges).unwrap_or(EdgeId(0)),
        },
        23 => MeshOp::Flip,
        24 => MeshOp::RecalcNormals,
        _ => {
            if rng.below(2) == 0 {
                MeshOp::Triangulate
            } else {
                MeshOp::SetMaterial { material: rng.below(4) as u16 }
            }
        }
    }
}

fn random_run(seed: u64, start: Mesh, steps: usize) {
    let mut rng = Rng(seed);
    let mut m = start.clone();
    let mut log = Vec::new();
    for step in 0..steps {
        if m.face_count() > 600 {
            break;
        }
        let op = random_op(&mut rng, &m);
        let before = m.clone();
        let marks = m.id_marks();
        match m.apply(&op) {
            Ok(result) => {
                if let Err(errors) = m.validate() {
                    panic!("seed {seed} step {step}: {op:?} broke the mesh: {errors:?}");
                }
                assert_eq!((result.verts, result.edges, result.faces), m.counts());
                assert!(result.new_verts.iter().all(|v| v.0 >= marks.0));
                assert!(result.new_edges.iter().all(|e| e.0 >= marks.1));
                assert!(result.new_faces.iter().all(|f| f.0 >= marks.2));
                log.push(op);
            }
            Err(_) => assert_eq!(m, before, "seed {seed} step {step}: a failed {op:?} changed the mesh"),
        }
    }
    let mut replay = start;
    replay.apply_all(&log).unwrap_or_else(|(i, e)| panic!("seed {seed}: replay failed at {i} ({:?}): {e}", log[i]));
    assert_eq!(replay, m, "seed {seed}: replay differs");
}

#[test]
fn random_ops_on_a_capped_cylinder() {
    for seed in 1..=6 {
        random_run(seed, fixtures::cylinder(8, 2, 1.0, 2.0, Caps::Ngon), 150);
    }
}

#[test]
fn random_ops_on_open_and_non_manifold_meshes() {
    for seed in 10..=13 {
        random_run(seed, fixtures::fin(), 120);
        random_run(seed + 100, fixtures::open_box(1.0), 120);
        random_run(seed + 200, fixtures::grid(3, 3, 1.0), 120);
    }
}

#[test]
fn random_ops_on_an_imported_section() {
    for seed in 20..=23 {
        let (m, _) = Mesh::from_triangles(fixtures::cube_source(1.0)).unwrap();
        random_run(seed, m, 120);
    }
}
