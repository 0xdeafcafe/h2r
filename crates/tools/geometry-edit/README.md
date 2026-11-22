# geometry-edit

The Modeling tool's targets and save, with no UI: a structure BSP's clusters and instanced geometry definitions and a render model's sections, opened as `mesh-edit` meshes, edited with undo and an op log, and written back in place.

## Where it sits

`tools` domain. Depends on `map`, `tags` (the section decoder and `tags::geometry_write`), `mesh-edit` and `editor-core` (the undo stack). The editor (`crates/apps/studio`) shows it and hands its patches to the map's save.

## Layout

- `src/target.rs`: `Target` (kind, tag, index) and the listing of a map's targets.
- `src/import.rs`: a decoded section as a `mesh_edit::Source`.
- `src/write.rs`: one section's in-place rewrite (moved vertices, frames, part bounds) through `tags::geometry_write`.
- `src/session.rs`: `GeometrySession`: open targets, undo, the op log, write classes and the patches a save writes.

## Tests

`cargo nextest run -p geometry-edit`. Invented sections and the invented textured map of `scenario_edit::fixture`.

## Docs

- [editor/modeling.md](../../../docs/editor/modeling.md)
- [editor/modeling-targets.md](../../../docs/editor/modeling-targets.md)
