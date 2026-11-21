# mesh-edit

The modeling kernel of `h2-editor`: an editable radial-edge mesh in index arenas (an edge may have three or more faces, shells may be open, faces are n-gons), its selection, the operations as serialisable `MeshOp` values, and import from and stable export to an indexed triangle list.

## Where it sits

`tools` domain. Depends on no workspace crate, only `serde`: it knows nothing of Halo formats. The editor's geometry tools turn sections and collision surfaces into a `Source` and write an `Export` back. Undo is the editor's `editor_core::UndoStack` of `Arc<Mesh>` snapshots; the session keeps the op log.

## Layout

- `src/mesh.rs`: `Mesh`, its arenas, queries, and the primitives (`add_face`, `rebuild_face`, `kill_*`) that alone touch the disk and radial links.
- `src/ids.rs`, `src/attrs.rs`: ids, `BitSet`, per-corner `Corner` attributes and per-vert `Skin`.
- `src/select.rs`: selection modes and flushing, grow and shrink, edge loops and rings.
- `src/op.rs`: `MeshOp`, `OpResult` and `Mesh::apply` (run on a copy, validate, swap).
- `src/ops/`: extrude and duplicate, delete and dissolve, merge and weld, loop cut and subdivide, fill, bridge, flip, normals, triangulate, transform.
- `src/validate.rs`: the invariants, checked after every op.
- `src/io.rs`: `Mesh::from_triangles`, `Mesh::from_polygons` and `Mesh::export`.
- `src/triangulate.rs`: ear clipping for export and `Triangulate`.
- `src/fixtures.rs`: invented test meshes (cube, open box, grid, cylinder, a three-face fin, a cube as a section).

## Tests

`cargo xtask test -p mesh-edit`. All synthetic: per-op counts on the fixtures, loops and rings, import and export, op-log replay through JSON, and seeded random op sequences validated after every op.

## Docs

- [editor/modeling.md](../../../docs/editor/modeling.md)
