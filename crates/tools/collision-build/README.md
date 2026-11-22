# collision-build

The collision compiler: convex, planar polygons in, a Halo 2 collision BSP out (planes, the 3D tree, leaves, 2D references and trees, surfaces, the winged-edge table and vertices, as `tags::collision::Bsp`). It also opens a `coll` BSP as a `mesh-edit` mesh, compiles it back into its permutation with the BSP physics slot rebuilt, writes a whole `coll` payload, stages the eight blocks by name for the game's `edit.collision.stage`, and makes collision polygons from render triangles (**From render**).

## Where it sits

`tools` domain. Depends on `tags` (the BSP and `coll` model), `mopp` (the slots' MOPP code) and `mesh-edit` (the editable mesh). The editor's Modeling tool and the converters are its users.

## Layout

- `src/lib.rs`: `Polygon`, `Options`, `Limits`, `build` and `build_with`.
- `src/prep.rs`: weld, drop the degenerate, triangulate what is not convex and planar, split rings over eight, gather planes.
- `src/edges.rs`: the winged-edge table.
- `src/tree.rs`, `src/tree2d.rs`: the 3D tree, open leaves, references and 2D trees.
- `src/query.rs`: the engine's walks (vector test, point location, 2D walk) the compiler is accepted against.
- `src/verify.rs`: `check`, the properties every retail BSP has.
- `src/coll.rs`: `coll` BSPs as meshes, `rebuild` with its slot, `encode_collision`, `stage_blocks`.
- `src/from_render.rs`: collision polygons from render triangles.

## Tests

`cargo xtask test -p collision-build`. All synthetic: random segments through the engine's vector test against a brute-force ray on a box, a concave room, a ramp and a two-sided sheet; the centroid, reference-side and leaf-flag checks; limits refused; `coll` mesh round trip; a saved `coll` read back with its slot's code keying each surface once; From render.

## Docs

- [editor/modeling.md](../../../docs/editor/modeling.md)
- [editor/modeling-targets.md](../../../docs/editor/modeling-targets.md)
