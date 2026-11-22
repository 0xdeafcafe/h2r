//! Helpers shared by the integration tests.
#![allow(dead_code)]

use mesh_edit::{EdgeId, Mesh, MeshError, MeshOp, OpResult, SelectHow, SelectMode, VertId};

pub fn v(i: u32) -> VertId {
    VertId(i)
}

/// The edge joining fixture verts `a` and `b`.
pub fn edge(m: &Mesh, a: u32, b: u32) -> EdgeId {
    m.find_edge(VertId(a), VertId(b)).unwrap_or_else(|| panic!("no edge {a}-{b}"))
}

/// Applies `op`, which must succeed and leave a valid mesh.
pub fn ok(m: &mut Mesh, op: MeshOp) -> OpResult {
    let result = m.apply(&op).unwrap_or_else(|e| panic!("{op:?} failed: {e}"));
    m.assert_valid();
    result
}

/// Applies `op`, which must fail with `want` and change nothing.
pub fn refused(m: &mut Mesh, op: MeshOp, want: MeshError) {
    let before = m.clone();
    let got = m.apply(&op).expect_err("op should fail");
    assert_eq!(got, want, "{op:?}");
    assert_eq!(*m, before, "a refused op changed the mesh");
}

/// Sets the mode and selects exactly these ids of its kind.
pub fn select(m: &mut Mesh, mode: SelectMode, ids: &[u32]) {
    ok(m, MeshOp::Mode { mode });
    ok(m, MeshOp::Select { ids: ids.to_vec(), how: SelectHow::Set });
}

pub fn select_edges(m: &mut Mesh, edges: &[EdgeId]) {
    let ids: Vec<u32> = edges.iter().map(|e| e.0).collect();
    select(m, SelectMode::Edge, &ids);
}

pub fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

pub fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| close(a[i], b[i]))
}

/// V - E + F.
pub fn euler(m: &Mesh) -> i64 {
    let (v, e, f) = m.counts();
    v as i64 - e as i64 + f as i64
}
