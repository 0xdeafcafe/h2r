# Modeling: mesh, BSP and texture editing in h2-editor

Status: Design. Nothing here is built.
Code: `crates/tools/mesh-edit` (the mesh kernel), `crates/tools/geometry-edit` (targets, session, save), `crates/tools/bitmap-edit` (the canvas), `crates/formats/collision-build` (the BSP compiler), a Modeling and a Textures tool in `crates/apps/studio`. Tests: each crate's own, on invented data.
Switch: none in the environment. Two session switches in the Edit menu: **Allow growing full blocks** (the existing one) and **Allow unverified geometry writes** (off by default).

The editor gains two tools. **Modeling** edits the triangles the game
draws and collides with: a structure BSP's clusters and instanced
geometry, its collision, render models and collision models. **Textures**
paints and replaces `bitm` images. Both edit the open map as the Scenario
and Tags tools do: in memory with undo, saved with the map as a `.map` or
a package ([scenario-editor.md](scenario-editor.md) §5), every command
also a method of the agent API. The rule that must hold: **an unedited
target writes back byte for byte, and an edit changes only the bytes it
has to**, so everything the decoders do not interpret survives.

Evidence labels are those of [docs-style.md](../meta/docs-style.md):
**Code** read from this repository, **Design** our choice, **S** supposed.
**V** and **E** facts are cited from the format pages, which hold their
evidence.

## What exists to build on

| Piece | Where | State |
|---|---|---|
| Section decoder (parts, subparts, indices, vertex streams) for `sbsp`, `ltmp` and `mode` | `crates/formats/tags/src/geometry.rs` (`decode_section`, `Section::vertices`), [structure-bsp.md](../formats/structure-bsp.md) | read only (**Code**) |
| Section writers | `h1convert::structure::raw_block` (structure, `0x44` struct), `crates/tools/h1-assets/src/models/mode.rs` (`build_section`, `0x70` struct) | each writes its own layout from Halo 1 data (**Code**) |
| Collision BSP codec and validator | `tags::collision::Bsp` (`decode_blocks`, `decode_structure`, `encode_blocks`, `validate`), [collision.md](../formats/collision.md) | reads and writes the eight blocks (**Code**) |
| Collision BSP from polygons | nowhere | `h1convert` re-encodes Halo 1's compiled tree and unites trees (`h1convert::scenery_collision::{transformed, merge}`); nothing compiles a tree from faces (**Code**) |
| Engine's collision walks, modelled | `h1convert::surface_query` (`vector`, `locate`, `surface_at`) | the oracle a compiled tree is tested against (**Code**) |
| Havok MOPP, pathfinding | `mopp::build_structure`, `mopp::build`, `pathfind::generate` | regenerate from a `Bsp` (**Code**) |
| Clusters, portals, visibility MOPP per section | `h1convert::clusters`, `h1convert::structure::visibility_mopp`, `h1convert::flood` | written from Halo 1's data; the flood model checks portals (**Code**) |
| In-place map patches, metadata growth | `map::MapEditor` (`patch_tag`, `patch_metadata`, `append_metadata`), [map-format.md](../formats/map-format.md) "Growing the metadata" | no raw-data patch or append (**Code**) |
| Whole-map builder with pinned retail tags | `map::build::TagSet` ([map-builder.md](../formats/map-builder.md)) | adds raw data and built tags; retail `sbsp` and `ltmp` do not relocate ([tag-repo.md](../formats/tag-repo.md)) (**Code**) |
| Bitmap decode | `assets::bitmap` (`texture_desc`, `decode_bytes`, `decode_image`), `gfx::swizzle` (both directions), [bitmaps.md](../formats/bitmaps.md) | no encoder but `h1_assets::bitmaps::mask::encode_dxt5` and `tags::rgba_bitmap` (linear ARGB) (**Code**) |
| Viewport, picking, gizmo, snapshot undo | `scenario_edit::render::Viewport`, `scenario_edit::scene`, `crates/apps/studio/src/viewport.rs`, `editor_core::UndoStack` | reused as they are (**Code**) |

## The mesh kernel (`crates/tools/mesh-edit`)

A crate with no dependencies in the workspace: no `tags`, no `map`, no
wgpu. It sits in `tools` with the other UI-free editor cores
([crate-structure.md](../dev/crate-structure.md)); it is not a format, so
not in `formats`. It holds one type, `Mesh`, its operations, and
`MeshOp`, the operations as values.

### Representation

A radial-edge mesh (Blender's BMesh shape) in index arenas, not a plain
half-edge mesh (**Design**). Halo 2's geometry is not manifold: clusters
are open shells with T-junctions, three faces meet on one edge where a
wall meets a floor, and collision surfaces are convex polygons of up to
eight vertices. A half-edge mesh needs every edge to have at most two
faces and would have to split such edges on import, which loses the very
adjacency the tools walk. In a radial-edge mesh a loop is a half-edge and
an edge's radial ring has one loop (boundary), two (manifold) or more.

| Element | Fields |
|---|---|
| `Vert` | position `[f32; 3]`, one incident edge, skin (up to four `(node, weight)` bytes), flags |
| `Edge` | two verts, a disk link per end, one loop of its radial ring, flags (seam, sharp) |
| `Loop` (a face corner) | vert, edge, face, next and previous in the face, next and previous in the radial ring, corner attributes |
| `Face` | first loop, corner count, material `u16`, group `u16` (the part it came from), flags |

Corner attributes are what a vertex buffer stores apart from position:
texture coordinate, lightmap coordinate, normal, binormal, tangent,
incident light direction, and `src`, the index of the source vertex with
a dirty bit per attribute. A UV seam or a hard edge is two corners of one
`Vert` with different attributes, which is how the source stores it.
Corners of a vert whose source normals are bit-equal form a **smooth
fan**; a fan survives every operation that keeps its faces, and its
normal is recomputed (angle-weighted) only when one of its faces moves.

Ids are `u32`, allocated by appending and never reused within a session;
a deleted element is a dead slot until export. So applying the same
`MeshOp`s to the same mesh gives the same ids.

### Invariants (`Mesh::validate`)

1. A live edge joins two different live verts; at most one edge joins a
   pair of verts.
2. A face has at least three corners; its loop cycle closes; `next.prev`
   is the loop itself; every loop names its face; consecutive corners'
   verts are the loop's edge's ends; no vert appears twice in a face.
3. An edge's radial ring holds exactly the loops that name the edge. A
   vert's disk cycle holds exactly its live edges.
4. No two faces have the same vert cycle in either direction.
5. Positions, coordinates and normals are finite; a normal is unit or
   zero; skin weights sum to 255 or the vert has none.
6. The selection names live elements only.

Every operation leaves a valid mesh or returns an error and changes
nothing (it works on a copy-on-write clone and swaps at the end).

### Operations

Selection is three bit sets (verts, edges, faces) and a mode. Changing
mode converts as Blender does: an edge is selected when both ends are, a
face when all its edges are. Counts below are the change in live
elements; `S` is the selected face set, `B` its boundary edges (a
selected face on one side only), `Vb` the verts on `B`.

| Op (`MeshOp`) | Semantics | Counts |
|---|---|---|
| `Select` | set, add, remove or toggle ids; `all`, `none`, `invert`, `more`, `less`, `linked` (flood over edges), `by_material`, `by_group` | none |
| `SelectLoop` | from an edge, at each end continue through a vert with exactly four edges, all with two faces, to the edge sharing no face with the current one; on a boundary edge, follow the boundary. Stops at any other vert or on return | none |
| `SelectRing` | from an edge, cross each quad to its opposite edge, both ways, until a face that is not a quad, an edge without exactly two faces, or the start | none |
| `Transform` | a 4x3 matrix about a pivot on the selection's verts; the touched fans' normals and frames are marked dirty | none |
| `ExtrudeFaces` | each vert of `Vb` is copied; `S` is rewired to the copies; one quad joins each edge of `B` to its copy; interior verts of `S` stay. The copies and interior verts then move by `offset`. A side quad takes material, group and both coordinates from the corner of `S` beside it, and its faces are marked lightmap-stale | V +\|Vb\|, E +\|B\|+\|Vb\|, F +\|B\| |
| `ExtrudeEdges` | each selected edge's verts are copied and a quad joins the edge to its copy (boundary or wire edges only) | V +\|Ve\|, E +\|Es\|+\|Ve\|, F +\|Es\| |
| `Duplicate` | selected faces, edges and verts are copied, unattached, and the copy becomes the selection | V +\|Vs\|, E +\|Es\|, F +\|S\| |
| `Delete` | `faces`: the faces, then edges left with no face and verts left with no edge. `edges`: the edges and their faces. `verts`: the verts, their edges and faces. `only_faces` keeps the wire | as stated |
| `Dissolve` | an edge with exactly two different faces joins them into one face when the result is a simple polygon; a vert with two edges joins them | edge: E -1, F -1; vert: V -1, E -1 |
| `Merge` | selected verts become one, at the centre, the first, the last, or the cursor; faces left with under three verts, doubled edges and doubled faces go. Corner attributes of the kept corner win | V -(n-1) |
| `Weld` | verts of the selection within `distance` are clustered (grid hash, lowest id first) and each cluster merged at its lowest id | as `Merge` per cluster |
| `LoopCut` | the ring of `SelectRing` from an edge; each ring edge gets `cuts` new verts at `factor`, each quad becomes `cuts + 1` quads. Position, coordinates and skin interpolate; weights are renormalised to 255 | ring of k edges, m quads: V +cuts·k, E +cuts·(k+m), F +cuts·m |
| `Fill` | one face from an ordered vert cycle or a closed chain of selected edges; attributes from the neighbouring corners | F +1, E +missing edges |
| `Flip` | reverses the selected faces' loop order and negates their corner normals | none |
| `RecalcNormals` | per connected component, orient faces alike across two-face edges; a closed component faces out (positive volume), an open one agrees with the majority of its source normals. Then every fan is recomputed | none |
| `TrisToQuads` | pairs of triangles sharing a two-face edge join when coplanar within `angle`, of one material and group, and continuous in both coordinates across the edge; best pair first, ties by lowest edge id | F -pairs, E -pairs |
| `Triangulate` | a convex face as a fan from its first corner, any other by ear clipping from its first corner | F +(n-3) per face |
| `SetMaterial`, `UvTransform`, `UvProject` | face material; a 3x2 matrix on selected corners' coordinates; planar or box projection | none |
| `Bevel`, `Inset` | one-segment edge bevel; face inset | later slice |

`ExtrudeFaces` on one face of a closed cube: V 8 to 12, E 12 to 20, F 6
to 10, and V - E + F stays 2. That, and the count column for every row,
is a test.

Halo's meshes are triangles, and loops and rings need quads, so
`TrisToQuads` is part of the loop slice, not an extra.

### Import and stable export

`Mesh::from_triangles(positions, corners, triangles, groups)` welds
source vertices that have bit-equal positions (and equal skin) into one
`Vert` and keeps each source vertex's attributes on its corners.
`Mesh::from_polygons` takes convex polygons (collision surfaces).

`Mesh::export()` gives vertices, indices and groups for a writer:

- A face that is a source triangle and has no dirty corner comes out as
  that triangle, at its place in its group. A corner with clean
  attributes comes out as its source vertex, with the source's packed
  bytes.
- So a mesh with no topology change exports the source's vertex and index
  order, and only edited vertices differ: the output has the source's
  size. With no edit at all it is the source.
- New and changed faces follow, per group, triangulated; a new vertex is
  made per distinct (vert, attributes) pair, and tangent frames are
  computed from the texture coordinates for dirty corners only.

### Undo and the op log

The document is `Vec<Arc<Mesh>>` (one per open object) plus the
selection, in an `editor_core::UndoStack` (**Design**): a step is a
snapshot, as in every other tool, and only the mesh an op touched is
cloned. A gizmo drag is one step (`record_coalesced` with the gesture's
key, sealed when the drag ends), as the scenario gizmo's is. Beside the
stack the session keeps the **op log**, the `MeshOp`s applied since the
target was opened. Replaying the log on the source gives the current
mesh; `geometry-edit` tests that after every op in its fixtures. The log
is what `doc.diff` prints, what an agent sends, and what a converted
Halo 1 map stores (below).

## Targets and the save

[modeling-targets.md](modeling-targets.md) has the round trip for each
thing a mesh comes from: what is read, what an edit may change, what is
written and recomputed, the limits and the risks. In short:

| Target | Opens as | Simplest write | Hard part |
|---|---|---|---|
| Structure cluster, instanced geometry definition | triangle mesh, parts as face groups | moved vertices and bounds, in place | a changed vertex count needs a new raw block, a new visibility MOPP and table; bucket-lit sections keep their vertex order |
| Instanced geometry placement, `mode` marker | an object with a transform | a fixed patch | none |
| Structure collision | polygon mesh | none: it is rebuilt | a BSP compiler, and every table that indexes surfaces, leaves and planes |
| Portals, clusters, visibility | overlay, read-only | not written | a face is culled with its section's cluster, so checks report faces outside it |
| Lightmap | stale marks on faces | kept as it is, with a warning | new texels and a rebake behind `geometry_edit::Relighter` |
| `mode` section | triangle mesh with skin | requantised vertices in place | the model's one compression box; other LODs go stale |
| `coll` BSP | polygon mesh | recompiled, blocks grown | the compiler; BSP physics slots |
| `phmo` shape | primitive, or hull points | fixed patches | mass and inertia stay as they were |

`geometry_edit::GeometrySession` holds the open targets with no UI and
hands its changes to the map's save, so the dirty mark, **Save map
as...** and packages count them. An edit falls into one of three write
classes, named in the status bar and by `geometry.check`: **in place**
(no count changes; fixed patches only, always allowed), **grown raw** (a
new raw block; behind Allow unverified geometry writes until a game run
has loaded one) and **grown blocks** (a tag block with more elements;
behind Allow growing full blocks as well).

## Texture editing (`crates/tools/bitmap-edit`)

- **Decode.** `assets::bitmap::decode_image` gives every level, face and
  slice of an image as RGBA8. The canvas keeps the source's stored bytes
  beside it. `P8` lightmap pages decode through their group's palette
  (`tags::lighting::palette_rgba`); `P8Bump`, `V8U8` and `G8B8` open
  read-only as data (their channels are hypotheses in
  `assets::FORMAT_TABLE`).
- **Canvas.** One RGBA8 layer per face or slice at mip 0, in tiles of
  256, each an `Arc`, so an undo snapshot shares every tile a stroke did
  not touch. Tools: brush and eraser (size, hardness, opacity, spacing;
  round stamp, linear-light blend), line, fill (flood by tolerance, or
  the whole selection), rectangle selection, eyedropper, clone, levels
  (black, white, gamma per channel), hue, saturation and value, invert,
  channel copy (alpha is often a mask, so channels paint apart), flip
  and rotate by quarter turns. **Import** replaces an image, a face or a
  selection from a PNG, resampled (box down, bilinear up) when sizes
  differ; **Export** writes a PNG outside any Git checkout, as every
  render does ([agent-api.md](agent-api.md), "Documents").
- **Encode** (`crates/platform/assets/src/encode.rs`, new). Linear and
  swizzled packers for every format whose row is `Pixels` or
  `LayoutOnly` (`gfx::swizzle::swizzle`, rows padded to 64 bytes for
  linear images); DXT1, DXT3 and DXT5 by a deterministic block
  compressor in Rust (principal-axis endpoints refined over the four
  nearest pairs; DXT1 with one-bit alpha when the source image used it);
  mips by a box filter in linear light, alpha by coverage for cut-out
  textures; chains laid out as the decoder reads them (DXT chains stop
  at one block, cube faces aligned to 128, an LOD rounded up to 128).
  A `P8` page is quantised back to its palette, which does not change.
  Tests: decode(encode(x)) equals x for the exact formats, is within a
  stated error for DXT, and encode(decode(stored)) is the stored bytes
  for every exact format; a tile no stroke touched keeps its stored
  blocks, so painting one corner of a DXT image changes only the blocks
  under the brush and their mips.
- **Write.** Same size and format: `raw_patches` over the image's three
  LOD chains (each is a whole chain from mip 0, 1 or 2). A new size or
  format: new raw data, and fixed patches of the `0x74` image record
  (size, format, mip count, LOD pointers and sizes; the cache handles
  stay `0xffffffff`, [multiplayer.md](../halo1/multiplayer.md) "Bitmap
  images say not cached"). An image whose pixels are in a shared cache,
  or whose raw range another image also names, takes the new-raw path.
  Sizes stay powers of two for swizzled and mipped images, depth fits a
  byte, DXT volumes are refused as the decoder refuses them.
- **Preview.** The viewport's texture for the bitmap
  (`scenario_edit::scene`) is updated from the canvas after each stamp
  (`Queue::write_texture` on the touched rectangle, mips at the stroke's
  end), at full size for the bitmap being painted, so a stroke shows on
  every surface and model using it. **Paint in the viewport** casts the
  cursor's ray (`Session::surface_hit`, or the edit mesh), takes the
  hit's texture coordinate and stamps there; it paints through the same
  canvas.
- **Deferred.** Layers, text, sprite sheet and sequence editing, a UV
  editor, the low-detail copy (header `+0x158`), which stays as the
  source's.

## The tools in the window

- **Modeling** (`ToolKind::Modeling`). The outliner lists the map's
  targets: the structure BSPs with their clusters and instanced
  geometry, then models by object (object, `hlmt`, `mode`, `coll`,
  `phmo`, through `tags::object`), each with its write class and stale
  marks. The viewport is the scenario's, with the level drawn.
  **Object mode** selects targets and moves instances and markers with
  the existing gizmo. **Edit mode** (Tab) opens the selected targets'
  meshes; the scene draws them from the edit mesh's own export, so the
  viewport shows what a save writes.
- **Selection.** Click, Cmd/Ctrl+click and Shift+click as the outliner's
  rules; box (drag), circle (C) and lasso (Ctrl+drag); through-mesh
  selection in wireframe only. The viewport turns a pick into ids and
  dispatches `Action::Mesh(MeshOp::Select)`; the kernel never sees a
  ray.
- **Gizmo.** `dear-imguizmo`, as the scenario's: one drag is
  `MeshOp::Transform`s coalesced into one step. Pivot: selection centre,
  active element, or 3D cursor; local axes are the active face's.
  Grid and angle snapping are the toolbar's; vertex snapping (hold
  Ctrl) snaps to the nearest vert under the cursor.
- **Overlays.** Wire, verts and face dots by mode; selection; normals;
  boundary and three-face edges; part and cluster colours; the collision
  mesh; portals; lightmap-stale and out-of-cluster tints; texel density.
  A budget line in the status bar: vertices of 65,535, the block's
  bytes against its in-place size, the write class, the map's headroom
  under its file and tag-memory bounds.
- **Textures** (`ToolKind::Textures`). The outliner lists bitmaps (by
  shader use of the selected object when one is selected), the centre is
  the 2D canvas with zoom, pan, channel toggles, mip and face pickers,
  the inspector the tool's settings and the image's format and budget.
- **Keys.** A table in `crates/apps/studio/src/ui/keymap.rs`, two
  presets. *Blender* (default in Edit mode): Tab mode; 1, 2, 3 vert,
  edge, face; G, R, S move, rotate, scale, then X, Y, Z to constrain; E
  extrude; Shift+D duplicate; X or Delete the delete menu; M merge;
  Ctrl+R loop cut; Alt+click (or double-click) loop, Ctrl+Alt+click
  ring; A all, Alt+A none, Ctrl+I invert, L linked under the cursor,
  Ctrl+L linked, Ctrl+Plus and Ctrl+Minus more and less; F fill; Ctrl+B
  bevel, I inset; Alt+N the normals menu; Ctrl+T triangulate, Alt+J
  tris to quads; H hide, Alt+H show; Z shading; numpad period or Home
  frame. *Editor* keeps W, E, R and X as the scenario tool has them and
  puts every op on the toolbar and the context menu. The camera is
  unchanged (right button to fly; Alt+left drag orbits, and a click
  without a drag is the loop select). Textures: B brush, E eraser, G
  fill, I eyedropper, `[` and `]` size, X swap colours.

## Interfaces

Every edit is an `Action` (`Action::Mesh(MeshOp)`,
`Action::Geometry(..)`, `Action::Bitmap(BitmapOp)`), dispatched by the
window and performed by the API, so both make the same document
([agent-api.md](agent-api.md), "The command layer"). Two providers
(`api/geometry.rs`, `api/bitmaps.rs`) register in `api::seams`. Two
documents join `doc.*`: `geometry` and `bitmaps`, each with its own undo
stack, saved and diffed with the map as `tags` is. The planned `model.*`
namespace (player models and looks) is separate and unchanged.

| Method | Does |
|---|---|
| `geometry.list {kind?}` | targets with kind (`cluster`, `instance_def`, `instance`, `structure_collision`, `mode_section`, `marker`, `coll_bsp`, `phmo_shape`), counts, write class, stale marks |
| `geometry.open {target}`, `geometry.close {target}` | enter or leave Edit mode on a target; `open` returns the mesh's element counts |
| `geometry.read {target, verts?, faces?, range?}` | positions, faces as vert ids, materials, groups; bounded by `range` |
| `geometry.check {target?}` | the invariants, limits, write class, `OUT_OF_CLUSTER`, portal seal, stale counts |
| `geometry.move {target, position?, ypr?, scale?}` | an instance or a marker, one undo step |
| `geometry.collision_from_render {target}` | the collision mesh from a render section |
| `geometry.relight {target?}` | run the `Relighter` on stale faces |
| `mesh.mode {mode}` | `vert`, `edge` or `face` |
| `mesh.select {ids? \| all \| none \| invert \| more \| less \| linked \| material \| group \| box, op?}` | `box` is a world box; `op` is `set`, `add`, `remove`, `toggle` |
| `mesh.select_loop {edge, ring?}` | loop, or ring with `ring: true` |
| `mesh.transform {translate?, rotate?, scale?, pivot?, matrix?, merge?}` | `merge` joins a drag into one step |
| `mesh.extrude {offset?, distance?, edges?}` | faces, or edges with `edges: true`; returns the new ids |
| `mesh.duplicate {offset?}`, `mesh.delete {what}`, `mesh.dissolve` | as the table above |
| `mesh.merge {at}`, `mesh.weld {distance}` | |
| `mesh.loop_cut {edge, cuts?, factor?}` | returns the new verts |
| `mesh.fill`, `mesh.flip`, `mesh.recalc_normals`, `mesh.triangulate`, `mesh.tris_to_quads {angle?}` | |
| `mesh.set_material {material}`, `mesh.uv {matrix \| project}` | |
| `mesh.ops {target}` | the op log |
| `mesh.apply {ops}` | a list of `MeshOp`s as one undo step: how a recorded log is replayed |
| `bitmap.list {name?, used_by?}`, `bitmap.read {bitmap, image?}` | images with size, format, mips, where the pixels are, write class |
| `bitmap.export {bitmap, image?, mip?, out}`, `bitmap.import {bitmap, image?, path, rect?}` | PNG out and in |
| `bitmap.paint {bitmap, image?, points, color, size, hardness?, opacity?, erase?}` | one stroke through pixel points, one undo step |
| `bitmap.fill {bitmap, image?, at \| rect, color, tolerance?}` | |
| `bitmap.adjust {bitmap, image?, levels? \| hsv? \| invert?, rect?, channels?}` | |
| `bitmap.pixels {bitmap, image?, rect}` | RGBA of a small rectangle, for tests |

Each op answers with the element counts after it, the ids it made and
the selection's size, so a script can check its own work. Refusals use
`REFUSED` with the reason and, in `data`, the limit and the value (a
section at 65,535 vertices; a grown write with its switch off; a sky's
strips). `view.snapshot` draws the edit mesh and the painted texture as
the window does.

## Rules and limits

- The source is read-only; a save never writes over it or into a Git
  checkout. No game data enters the repository: every fixture is
  invented.
- An unedited target contributes no change. An in-place edit changes
  only bytes of the vertices, bounds and pixels it touched, and the
  header checksum.
- A save that an op's write class does not allow is refused whole,
  before anything is written, naming the target and the switch.
- The map's bounds are the writer's: `map::writer::cache_size_limit`
  for the file, `max_metadata_size` for grown blocks (an empirical
  bound: the largest retail map of the type). A map's metadata sits
  straight after its largest BSP region, so an `sbsp` block of a map
  with one BSP cannot grow inside the BSP space; a grown `sbsp` block lives in the
  ordinary metadata (**S** that the engine accepts the pointer).
- The shared caches (`shared.map`, `single_player_shared.map`,
  `mainmenu.map`) are never written. What they hold is copied into the
  open map when edited.
- Geometry and textures are not sent to a paired game: Play and pair's
  `edit.set` carries placements only. A geometry or bitmap edit reaches
  the game through a saved package and a map load.
- Multiplayer peers must load the same bytes
  ([scenario-editor.md](scenario-editor.md) §5).

Out of scope: sculpting, modifiers, boolean operations, subdivision
surfaces, animation and rigging (`jmad`, nodes, weight painting), new
tags (a new model, bitmap or shader; tags are made by the converters and
`map::build`), shader editing beyond the Tags tool's fields, decorators,
water and fog planes, portal and cluster authoring, a lightmap bake that
matches retail, import and export of mesh files (OBJ or glTF; a later
page), and editing more than one map at once.

## Converted Halo 1 maps

A converted map is built from scratch by `h1convert`
(`Prepared::build_fresh`, [multiplayer.md](../halo1/multiplayer.md)):
its `sbsp` is a built tree whose every field the converter writes, its
BSP space is sized to its own regions, and a new conversion replaces it.
So such a map takes edits at the source, not as patches (**Design**):

- The package keeps a **geometry overlay** beside its manifest: per
  Halo 1 source mesh (a lightmap material's surfaces, a model's
  geometry, a collision BSP), the hash of the source's bytes and the op
  log made in the editor.
- `h1convert` applies the overlay where it already has plain meshes:
  after `structure::meshes` and before `clusters::pack_clusters` for the
  level, in `h1_assets::models::mode::convert_model` for a model, and
  before `Bsp::encode_blocks` for collision (recompiled by
  `collision-build`, with clusters, portals, the MOPP and pathfinding
  then written from the edited data as for any conversion). An overlay
  whose source hash differs is a named error, never applied or dropped
  silently.
- In the editor a converted map opens like any map; saving a mesh edit
  writes the overlay and runs the conversion again
  (`crates/apps/studio/src/app/convert.rs`), so no write class or growth
  limit applies and the structure's collision can be rebuilt
  (modeling-targets.md, "Structure collision", step 3) with every dependent table regenerated.
- Lightmaps follow the same three policies; a Halo 1 page is R5G6B5,
  one per cluster (`h1convert::clusters::lightmap_pages`), so the
  relighter's new charts go into the cluster's own page.
- Texture edits are ordinary: the converted bitmap is local raw data and
  the edit is kept as a PNG in the overlay, applied after
  `h1bitmaps::convert_bitmap`.

## Slices

In build order. Sizes: S under 800 lines with tests, M under 2,500, L
more. Every acceptance test is on invented data; a disc test is
`#[ignore]`d and extra.

| # | Slice | Size | Needs | Acceptance |
|---|---|---|---|---|
| 1 | **Mesh kernel**: arenas, `validate`, selection, `Transform`, `ExtrudeFaces`, `ExtrudeEdges`, `Duplicate`, `Delete`, `Merge`, `Weld`, `Fill`, `Flip`, `RecalcNormals`, `Triangulate`, `MeshOp`, import and stable export | L | nothing | every op's count row on a cube, a plane grid, an open shell and a three-face edge; `validate` after every op of a seeded random op sequence; an unedited import exports the source; replaying the op log reproduces the mesh; an op that fails changes nothing |
| 2 | **Preview, no write**: `geometry-edit` targets for a `mode` section and a cluster; `tags::geometry_write::encode_section`; the Modeling tool (object and edit mode, overlays, gizmo, keymap); `geometry.*` and `mesh.*`; `doc` `geometry` with undo | L | 1 | `decode(encode(x)) == x` and `encode(decode(block)) == block` on invented sections of both layouts; every method's example runs (`every_method_in_methods_has_a_test`); the window's path and the API's make the same mesh; undo and redo through `doc.*`; a save with geometry edits is refused with a named reason; headless snapshot differs after an extrude |
| 3 | **Write, in place**: `MapEditor::patch_raw`; moved vertices for clusters, instance definitions and `mode` sections; compression box growth; part and cluster bounds; instance placements and markers; stale marks | M | 2 | the saved map differs from the source only in the moved vertices' bytes, the bounds and the checksum; it reads back to the edited positions within quantisation; a box growth rewrites every section at its own size; a count change is refused; package round trip (PPF3) |
| 4 | **Loop tools**: `SelectLoop`, `SelectRing`, `LoopCut`, `TrisToQuads`, `Dissolve`; then `Bevel` and `Inset` | M | 1 | counts on a quad cylinder (closed ring) and a strip (open ring); loops stop at a pole and at a three-face edge; interpolated coordinates and weights (sum 255); join then triangulate restores an untouched pair |
| 5 | **Textures, paint and export**: `bitmap-edit` canvas, tools, PNG in and out, the Textures tool, live preview, `bitmap.*` without write | M | nothing | stamp and fill pixel tests; an undo snapshot shares untouched tiles; import resample; `bitmap.pixels` after `bitmap.paint`; export refused in a Git checkout |
| 6 | **Textures, write**: `assets::encode` (linear, swizzled, DXT1/3/5, mips, cube faces), same-size in place; then new size or format and shared-cache copies through `MapEditor::append_raw` | M | 5; 3 for `patch_raw` | the encode tests above; a painted corner changes only its blocks and their mips in the saved map; a resized image reads back through `assets::bitmap::decode_image` with every level |
| 7 | **Write, grown raw**: `MapEditor::append_raw`; topology changes for clusters and `mode` sections; section counts, visibility MOPP and table, frames; LOD repointing; behind Allow unverified geometry writes | L | 3 | the saved map validates and reads back to the exported mesh; `check_visibility` passes on every written cluster; the old block is unreferenced; a bucket-lit section and a sky strip are refused; the metadata still ends the file and a scenario block can still grow; package round trip (BPS) with a changed geometry hash |
| 8 | **Collision compiler and `coll`**: `collision-build`; `coll` BSPs as meshes; BSP physics slots; **From render** | L | 1 | `Bsp::validate`; centroid and reference-side walks; random segments against brute force on a box, a concave room, a ramp and a two-sided sheet; limits refused; a saved `coll` reads back and its slot code emits each surface once |
| 9 | **`phmo` primitives** in place | S | 2 | a radius, a box and a hull vertex change only their bytes; a hull made concave is refused |
| 10 | **Converted maps**: the overlay, its application in `h1convert` and `h1-assets`, the editor's save through conversion | M | 2, 8 | an overlay with a stale hash is an error; a converted fixture with an extrude overlay has the extruded faces in its cluster and passes the converter's own checks (`check_visibility`, portal and cluster tests) |
| 11 | **Level collision, adding solids**: compile, `merge`, the leaf block, MOPP, checksum; grown blocks in ordinary metadata | L | 7, 8 | merged level validates; every old segment query gives its old answer away from the solid, and hits the solid inside it; added leaves have their stand-in's cluster; the MOPP returns every touching surface on sampled boxes |
| 12 | **Lightmaps**: stale marks are in slice 3; here the `Relighter` trait, chart packing and the sun-plus-ambient implementation | M | 6, 7 | new charts do not overlap used texels (a coverage mask from the page's `1f` triangles); a lit quad facing the sun is brighter than one facing away; the page's other texels are unchanged |
| 13 | **Level collision, rebuild**, converted maps first | L | 10, 11 | the converter's collision and flood tests on a fixture with a wall moved and a wall deleted; for retail, the disc survey of index holders first |

Slices 4 and 5 need nothing from 2 and 3 and can run beside them.

## Evidence

- **Code**: `editor_core::UndoStack` is a snapshot stack with coalescing
  keys; every tool's undo is one. The mesh document follows it.
- **Code**: the only DXT encoder in the tree is
  `h1_assets::bitmaps::mask::encode_dxt5`, for one mask; `gfx::swizzle`
  has both directions.
- **V** ([bitmaps.md](../formats/bitmaps.md)): each of an image's three
  LOD pointers is a whole chain; DXT chains stop at one block; LOD sizes
  round up to 128 bytes; linear rows pad to 64.
- **Design**: radial-edge mesh, snapshot undo with an op log, stable
  export, the write classes, converted maps edited at the source.
- The evidence for each target's layout and write is in
  [modeling-targets.md](modeling-targets.md), "Evidence".

## Not verified

- Everything under "Not verified" in
  [modeling-targets.md](modeling-targets.md): that the game loads and
  plays each class of geometry write.
- How a re-encoded DXT texture looks beside its neighbours in game, and
  the texture cache's behaviour with a resized image.
- That the Blender preset's keys do not collide with Dear ImGui's own
  on every platform's keyboard.

## Related

- [modeling-targets.md](modeling-targets.md): the round trip for each
  target.
- [editor.md](editor.md), [scenario-editor.md](scenario-editor.md),
  [agent-api.md](agent-api.md): the app, the save and the API this
  extends.
- [structure-bsp.md](../formats/structure-bsp.md),
  [map-format.md](../formats/map-format.md) ("Render-model geometry"),
  [collision.md](../formats/collision.md), [mopp.md](../formats/mopp.md),
  [pathfinding.md](../formats/pathfinding.md),
  [lighting.md](../formats/lighting.md),
  [bitmaps.md](../formats/bitmaps.md),
  [map-builder.md](../formats/map-builder.md): the formats.
- [multiplayer.md](../halo1/multiplayer.md),
  [models.md](../halo1/models.md), [bitmaps.md](../halo1/bitmaps.md):
  what the Halo 1 converter writes, field by field.
- [dynamic-lighting.md](../graphics/dynamic-lighting.md),
  [live-gi.md](../graphics/live-gi.md): the lighting work the rebake
  hook meets.
