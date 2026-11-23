# Modeling: mesh, BSP and texture editing in h2-editor

Status: Partly built. The mesh kernel (slice 1), the in-place
geometry path (slices 2 and 3, for clusters, instanced definitions and
`mode` sections), grown raw writes (slice 7, with
`MapEditor::append_raw` and **Use for lower LODs** in the session), the
collision compiler with `coll` BSPs as meshes (slice 8, the crate only),
adding solids to a structure's level collision (slice 11, the crates and
the map's save only) and the Textures tool (slices 5 and 6: painting,
exporting, and writing the edited images with the map's save) are
implemented; the Modeling window is a section of the map inspector with
buttons, not yet the gizmo, overlays and keymap below, and has no control
or API method for **Use for lower LODs** yet. A `coll` BSP and a
structure's collision are not yet `geometry-edit` targets, so the window,
`geometry.*` and the session do not reach them; no tool makes a
`geometry_edit::level_collision` edit yet. `Modeling::level_collision`
holds them (`App::set_level_collision`), and Play sends what it holds to
a paired game (realtime-edits.md, "The editor"). Rebuilding a level's collision
and markers are designs.
Code: `crates/tools/mesh-edit`, `crates/tools/geometry-edit`,
`crates/tools/collision-build`,
`crates/tools/collision-build/src/{merge,level}.rs`,
`crates/tools/geometry-edit/src/level_collision.rs`,
`crates/formats/tags/src/geometry_write.rs`,
`crates/formats/map/src/writer.rs` (`append_raw`, `append_metadata`),
`crates/tools/scenario-edit/src/save.rs`,
`crates/platform/assets/src/{encode,write}.rs`,
`crates/apps/studio/src/app/{modeling,textures}.rs`,
`crates/apps/studio/src/api/bitmaps.rs`,
`crates/apps/studio/src/api/geometry.rs`,
`crates/apps/studio/src/ui/modeling.rs`.
Tests: `crates/tools/mesh-edit/tests`,
`crates/tools/geometry-edit/src/{session,write,grow,level_collision}_tests.rs`,
`crates/tools/collision-build/src/{build,coll,merge}_tests.rs`,
`crates/formats/tags/src/geometry_write_tests.rs`, the `append*raw*`
tests in `crates/formats/map/src/tests.rs`,
`a_geometry_raw_append_*`, `a_geometry_grown_block_*` and `bitmap_changes_*` in
`crates/tools/scenario-edit/src/tests.rs`,
`crates/platform/assets/src/encode_tests.rs`, the write tests in
`crates/apps/studio/src/api/tests/bitmaps_tests.rs`, the `geometry_*` and `doc_undo_on_the_map_*` tests in
`crates/apps/studio/src/api/tests.rs`.
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
| Collision BSP from polygons | `collision_build::build` (below, "The collision compiler") | compiles a tree from convex polygons; `h1convert` also re-encodes Halo 1's compiled tree and unites trees (`h1convert::scenery_collision::{transformed, merge}`) (**Code**) |
| Engine's collision walks, modelled | `collision_build::query` (`vector`, `locate`, `surface_at`), and the same model over both games' encodings in `h1convert::surface_query` | the oracle a compiled tree is tested against (**Code**) |
| Havok MOPP, pathfinding | `mopp::build_structure`, `mopp::build`, `pathfind::generate` | regenerate from a `Bsp` (**Code**) |
| Clusters, portals, visibility MOPP per section | `h1convert::clusters`, `h1convert::structure::visibility_mopp`, `h1convert::flood` | written from Halo 1's data; the flood model checks portals (**Code**) |
| In-place map patches, metadata growth, raw data | `map::MapEditor` (`patch_tag`, `patch_metadata`, `append_metadata`, `patch_raw`, `append_raw`), [map-format.md](../formats/map-format.md) "Growing the metadata" and "Inserting raw data" | `append_raw` places new raw data where the tag index starts; the index and metadata follow (**Code**) |
| Whole-map builder with pinned retail tags | `map::build::TagSet` ([map-builder.md](../formats/map-builder.md)) | adds raw data and built tags; retail `sbsp` and `ltmp` do not relocate ([tag-repo.md](../formats/tag-repo.md)) (**Code**) |
| Bitmap decode and encode | `assets::bitmap` (`texture_desc`, `decode_bytes`, `decode_image`), `assets::encode` (`encode_lod`, `encode_tiles`), `assets::write` (`plan_image`, `reshaped`), `gfx::swizzle` (both directions), [bitmaps.md](../formats/bitmaps.md) | the editor's encoder; `h1_assets::bitmaps::mask::encode_dxt5` and `tags::rgba_bitmap` (linear ARGB) are the converters' (**Code**) |
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

#### Import and export

`Mesh::from_triangles(Source)` in `crates/tools/mesh-edit/src/io.rs` welds
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

## The collision compiler (`crates/tools/collision-build`)

`collision_build::build(&[Polygon]) -> Result<Compiled>` (**Code**). A
`Polygon` is convex and planar, wound counter-clockwise seen from its
open side, with a material, surface flags (bit 0 two-sided) and a
breakable-surface byte. `Compiled` holds the `tags::collision::Bsp` and,
per surface, the input polygon it came from. The steps are
modeling-targets.md's ("Structure collision", step 1):

- **Prepare** (`prep.rs`): weld within 0.001 world units; drop a polygon
  left with under three corners or no area; triangulate one that is not
  planar within 0.001 or not convex; split a ring over eight into fans
  of at most eight. `Report` names each polygon so treated.
- **Planes**: one per distinct plane (normals within 1e-6 of cosine,
  distances within 0.001); a surface facing the plane's back sets bit 15.
- **Edges** (`edges.rs`): the surface that runs an edge from start to
  end is its left surface and names its next edge as the forward edge;
  one running it the other way is the right surface and names its next
  edge as the reverse edge. So `Bsp::surface_ring` gives every ring in
  its input winding. An edge with no right surface names itself as its
  reverse edge.
- **3D tree** (`tree.rs`): partition on a fragment's plane, fewest
  splits, then best balance, then lowest plane index (48 candidate
  planes per node, sampled evenly past that). Fragments on the plane stay
  at the node. An empty side is solid when a one-sided fragment there
  faces away from it and none faces it, else an open leaf.
- **References and 2D trees** (`tree.rs`, `tree2d.rs`): an open leaf
  takes one reference per plane of its path where a fragment kept at that
  node, seen from its side, still has area clipped to the leaf; the flip
  bit is set on the back. The 2D tree over those surfaces splits on their
  edge lines; a surface reaching both sides goes to both, unsplit.
  Overlapping coplanar surfaces have no separating line and are refused.
  Leaf flag bit 0 is set where a reference reaches a two-sided surface.

`Limits::HALO2` refuses what the packed fields cannot hold: 32,767
surfaces, 65,535 edges and vertices, 32,768 planes and 2D nodes, 65,535
references, 2^23 3D nodes, 2^23 - 1 leaves (leaf `0x7fffff` would encode
as the "none" child), 255 references a leaf. `verify::check` is the
acceptance and the check an edited BSP gets: `Bsp::validate`, rings of
at most eight, every centroid walking its reference's 2D tree back to
its surface, every reference on its leaf's path on its flip bit's side,
and the leaf flag rule.

**`coll`** (`coll.rs`). `mesh_from_bsp` opens a BSP as a mesh: a vert per
BSP vertex, a face per surface wound about its open side, the material
as the face material, and the surface flags and breakable byte as the
face group (`group_of`: flags low, breakable high), so kernel ops carry
them. `polygons_from_mesh` turns it back. `rebuild` compiles polygons
into a permutation's BSP, keeps its leading word, refuses a material
past the tag's list, and gives a BSP physics slot its box
(`bsp_box`) and a MOPP code object keyed by plain surface index
(`bsp_mopp`), keeping the slot's `+0x30` datum. `encode_collision`
writes a whole `coll` payload, every block 16-aligned after the root,
with retail's slot words (`0x81`, `0x80`, `0x81`). `stage_blocks` gives
one BSP's eight blocks by name (`BLOCK_NAMES`, the fields of
`tags::collision::Bsp`) for `edit.collision.stage`
([realtime-edits.md](realtime-edits.md), "Collision").

**Adding solids** (`merge.rs`, `level.rs`; slice 11).
`merge::merge(level, piece)` appends a compiled solid to a level's BSP
by modeling-targets.md's method ("Structure collision", step 2), the
method of `h1convert::scenery_collision::merge` on this crate's types
and `Limits`: the piece's planes, 2D nodes, surfaces, edges, vertices and
references go after the level's, one copy of its 3D tree serves every
open level leaf its solid reaches, each through six nodes testing its box
grown by `BOX_MARGIN` (0.05), and the copy's open leaves carry the
references of the level leaves they stand in whose planes their cells
touch. Every existing index holds; only the parent of a gated leaf
changes a child. The piece's surfaces keep material and flags but get no
breakable surface (bit 3 cleared, byte `0xff`). `Merged` names, per added
leaf, its stand-in (the first level leaf it stands in) and every host.
A copy that serves several level leaves carries all their references, so
`verify::check`'s path rule names a reference off one of its paths; the
engine's matcher only spends a lookup on it, and the rule holds for a
solid inside one level leaf.

`level::Level` is a structure's level collision as plain data: the BSP,
the per-leaf block (`sbsp+0x2c`, `LEAF_SIZE` 8 bytes a leaf, the cluster
an `i16` at `+0`), the MOPP block (`sbsp+0x1e0`) and the collision
material count (`sbsp+0x0c`). `Level::add_solid(polygons, options)`
compiles the solid, refuses a material past the list, merges it, refuses
a solid that reaches no open space or a merged BSP that fails
`Bsp::validate`, copies each added leaf's stand-in element whole (so the
leaf is in its stand-in's cluster), and rebuilds the MOPP over every
structure surface plus every other key of the old program as the box its
old path guarantees (`level::rebuild_mopp`, as `replaced_retail_levels`
in `crates/formats/mopp/tests/disc.rs` does; an old program that does not
parse is refused, not dropped). `Added::stage` gives the eight blocks by
name, the per-leaf block and the MOPP for `edit.collision.stage`
(`level::LevelStage`). The structure checksum (`sbsp+0x08`) is kept: it
is not a hash of the collision.

`geometry_edit::level_collision` reads a `Level` from a map (`read`,
each block where its pair points, in the structure's region or the
ordinary metadata) and writes an added solid back
(`LevelEdit::writes`): a block whose count holds is patched in place, one
whose count grows is a `GrownBlock`, appended to the ordinary metadata
16-aligned with its pair pointed there, and the MOPP goes over the old
program when it fits (count set to the new length, the old tail zeroed),
else it grows too. Adding a solid grows all eight blocks that gain
elements and the per-leaf block. The save takes grown blocks as
`scenario_edit::GeometryChanges::grown` (`MapEditor::append_metadata`,
then `patch_metadata` or `patch_bsp` for the pair), and the writer
repairs the header checksum. `LevelEdit::stage` sends the same bytes.

**From render** (`from_render.rs`): weld, drop degenerate and repeated
triangles (one drawn both ways becomes two-sided), then join neighbours
across shared edges, of one material and flags, within 0.5 degrees and
0.001 of each other's plane, while the result is convex with at most
eight vertices.

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
(no count changes; fixed patches only; behind Allow unverified geometry
writes until a game run has loaded one), **grown raw** (a
new raw block; behind Allow unverified geometry writes until a game run
has loaded one) and **grown blocks** (a tag block with more elements;
behind Allow growing full blocks as well).

## Texture editing (`bitmap-edit`)

Status: built (`crates/tools/bitmap-edit`, `crates/platform/assets/src/{encode,write}.rs`,
`app/textures.rs`, `ui/textures.rs`, `api/bitmaps.rs`). Edits live in the
editor until a save, which encodes what changed and writes it with the map.
The live preview is the canvas; the 3D viewport does not show them yet. P8
pages open read-only.

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
  **Generate...** makes a new picture for the image from a prompt and
  references the user picks, fitted to its size and format, and
  imports it as one undo step
  ([editor-guide.md](editor-guide.md#generating-an-image)).
- **Encode** (`assets::encode`). Texels invert `gfx::convert` for
  `A8`, `Y8`, `AY8`, `A8Y8`, `R5G6B5`, `A1R5G5B5`, `A4R4G4B4`,
  `X8R8G8B8` and `A8R8G8B8`, each channel quantised with rounding, so
  decode, encode, decode is the identity; `Y8` and `AY8` take the
  Rec. 601 luma, `A8` the alpha. Swizzled levels go through
  `gfx::swizzle`, linear rows are padded to the descriptor's pitch,
  DXT1, DXT3 and DXT5 are range fitted blocks (the inset bounding box of
  the block's colours along the diagonal their covariance picks, 5:6:5
  endpoints, nearest palette entry; DXT1 uses the three colour mode and
  a transparent index for texels under half alpha; DXT3 alpha is 4 bits,
  DXT5 alpha the interpolated set). Mips are a box filter from mip 0 in
  the stored encoding, so a region encoded alone equals the same region
  of a whole chain. Chains are laid out as the decoder reads them (DXT
  chains stop at one block, cube faces `face_stride` apart, a LOD rounded
  up to 128). The formats whose channels are hypotheses (`P8`, `P8Bump`,
  `V8U8`, `G8B8`) and the float formats are not written.
  `encode_lod(image, lod, planes)` writes a whole LOD chain.
  `encode_tiles(image, planes, dirty)` writes only the units (a texel, or
  a 4x4 block) that dirty rectangles of mip 0 touch, in every LOD the
  image stores and every level of each (the rectangle maps to the texels
  that read it at each mip), and returns byte `Span`s (LOD, offset,
  bytes), adjacent units joined. A block is re-encoded whole from its
  pixels, so an untouched block keeps its stored bytes and painting one
  corner of a DXT image changes only the blocks under it and their mips.
  The live push of docs/editor/realtime-edits.md sends these spans
  (`App::bitmap_tile_spans`, `bitmap.spans`).
- **Write** (`assets::write::plan_image`; the Textures tool's
  `App::push_bitmap_changes`, which every map save runs first, hands the
  plan to the scenario session as `Session::set_bitmap_changes`, which
  saves with the map and counts toward its dirty mark). Same size and
  format, pixels in this map: `RawWrite`s, `MapEditor::patch_raw` over the
  spans, in each LOD chain the image stores (each is a whole chain from
  mip 0, 1 or 2). Pixels in a shared cache (`shared`, `mainmenu`,
  `single_player_shared`), which the editor never writes: the LOD's
  stored bytes are read, the spans patched in the copy, the copy appended
  with `MapEditor::append_raw` and the image's LOD pointer word pointed at
  it. A new size or format (`bitmap.reshape`, an image-level request, not
  an undo step): every LOD the image stores is encoded whole from the
  layers resampled to the new size and appended, and the `0x74` image
  record's width, height, format, flags and mip count are patched, with
  each LOD's pointer and size (a LOD the new size cannot hold gets no
  pointer, `0xffffffff`, and size 0; the cache handles stay
  `0xffffffff`, [multiplayer.md](../halo1/multiplayer.md) "Bitmap images
  say not cached"). The mip count stays a full chain when the image had
  mips and is 0 otherwise; DXT needs power of two sides; a side that is
  not a power of two is linear with no mips; a cube stays square; DXT
  volumes are refused as the decoder refuses them. The old blocks stay as
  dead data. The changes are recomputed from the source map at every save
  (the session never rebases onto a saved file), so a second save carries
  the first edit too, and undoing every edit saves the source again. A
  plan that fails (a locked image, a format not written, a LOD shorter
  than the edit) is the session's `bitmap_refusal`: the save fails by
  name before it writes. A map that cannot take a new raw block
  (`raw_append_refusal`) refuses a shared-cache copy or a reshape.
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
  source's (and so goes stale when the picture changes), a window control
  for a new size or format (only `bitmap.reshape` asks for one), and a
  LOD 1 or 2 where the map stored no LOD 0.

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
- **Keys.** A table in the editor keymap, two
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

What is built (Code): every Modeling command is
`Action::Model(ModelAction)` (`app/modeling.rs`): open, close, apply a
list of `MeshOp`s as one undo step (or one merged step per `merge` key),
seal, undo and redo. The provider `api/geometry.rs` answers
`geometry.list {model?}`, `geometry.open`, `geometry.close`,
`geometry.read`, `geometry.check`, `mesh.mode`, `mesh.select {ids, op |
all | none | invert}`, `mesh.transform {translate | matrix, pivot?,
merge?}`, `mesh.ops` and `mesh.apply`, each with a target id
`kind:tag:index` (`cluster:e1000004:0`) or the target in edit mode.
There is no `geometry` document: while a target is in edit mode the
map's undo (`doc.undo {"doc":"map"}`, Cmd+Z) is the geometry session's,
and the edits save with the map, counted in its dirty mark. After every
command the session's writes go to the save
(`scenario_edit::GeometryChanges`): tag patches, raw patches and, for a
grown section, a `RawAppend` (the new block, and the block info word that
takes its raw pointer), which the save inserts with
`MapEditor::append_raw`. With **Allow unverified geometry writes** off, or
an edit the format cannot hold (a bucket-lit structure section or a sky
model's strips grown, a part or material the section does not have, a map
whose metadata does not end the file), the save is refused with the reason
(`geometry.check`'s `save_refusal`) and writes nothing.
`geometry_edit::RawAppend` keeps a grown section's bytes, alignment,
pointer word and dependent writes together, and `Patches::raw` the
changed spans of an in-place edit, so the live path sends the same bytes
([realtime-edits.md](realtime-edits.md), "Geometry"). The
viewport redraws an open cluster, or each placement of an open
definition, in the native world from `GeometrySession::previews`; a
`mode` section has no live preview yet.

The design the rest of this section describes: every edit is an `Action` (`Action::Mesh(MeshOp)`,
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
| `bitmap.list {name?}`, `bitmap.read {bitmap, image?}` | images with size, format, mips, where the pixels are, write class (`in_place`, `appended`, `none`) and any reshape |
| `bitmap.reshape {bitmap, image?, width?, height?, format?}` | asks for a new size or format; a save writes the image whole |
| `bitmap.spans {bitmap, image?, face?, slice?, rect?}` | the stored bytes the edits change, by LOD and offset; nothing is written |
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
- **Code**: `assets::encode` is the editor's encoder; the converters'
  are `h1_assets::bitmaps::mask::encode_dxt5` (one mask) and
  `tags::rgba_bitmap` (linear ARGB). `gfx::swizzle` has both directions.
- **Design**: which LODs a new size stores (the ones the image had that
  the new chain still holds) and how the mips of a reshaped image are
  filtered (box, in the stored encoding) are choices, not measured
  against the engine's own tools.
- **V** ([bitmaps.md](../formats/bitmaps.md)): each of an image's three
  LOD pointers is a whole chain; DXT chains stop at one block; LOD sizes
  round up to 128 bytes; linear rows pad to 64.
- **Design**: radial-edge mesh, snapshot undo with an op log, stable
  export, the write classes, converted maps edited at the source.
- **Design**: the collision compiler's splitter choice, its solid rule
  for an empty side and its 2D trees over unsplit surfaces; they are
  accepted by the engine's walks as `collision_build::query` models them
  (E, [collision.md](../formats/collision.md), "What the engine does with
  a BSP") on invented shapes.
- **S**: surface flag bit 0 is "two-sided". It is Halo 1's bit, which
  `h1convert::collision` copies unchanged, and retail sets leaf flag bit
  0 exactly where a reference reaches a surface so flagged
  (collision.md); no engine read of the surface bit is cited.
- The evidence for each target's layout and write is in
  [modeling-targets.md](modeling-targets.md), "Evidence".

## Not verified

- Everything under "Not verified" in
  [modeling-targets.md](modeling-targets.md): that the game loads and
  plays each class of geometry write.
- How a re-encoded DXT texture looks beside its neighbours in game, and
  the texture cache's behaviour with a resized image.
- That a compiled collision BSP plays as retail's does (modeling-targets.md,
  "Not verified"), and that Havok's surface shape, which copies a ring as
  one polygon, takes rings in the compiler's winding (counter-clockwise
  about the open side, as the converted Halo 1 rings run).
- That a level with an added solid loads and plays: its collision blocks,
  per-leaf block and MOPP pointed from a BSP region into the ordinary
  metadata (modeling-targets.md, "Not verified": an `sbsp` block in
  ordinary metadata), and Havok's structure shape built over the
  rebuilt MOPP. No game run has loaded one.
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
