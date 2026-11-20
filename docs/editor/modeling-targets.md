# Modeling targets: the Halo 2 round trip

Status: Design. Nothing here is built.
Code: `crates/tools/geometry-edit` (targets, session, save), `crates/formats/tags/src/geometry_write.rs` (the section encoder), `crates/formats/collision-build` (the BSP compiler), `map::MapEditor` (`patch_raw`, `append_raw`). Tests: each crate's own, on invented data.
Switch: none in the environment. The session switches **Allow growing full blocks** and **Allow unverified geometry writes** (both off by default) gate the write classes below.

How each thing the Modeling tool of [modeling.md](modeling.md) edits is
read into a mesh and written back: structure BSP render geometry, its
collision, portals and clusters, lightmaps, render models, collision and
physics models. For each: the read path, the edit model, the write path,
what is recomputed, the limits, what is risky and what is deferred. The
rule that must hold: an unedited target writes back byte for byte, and
an edit changes only the bytes it has to.

Evidence labels are those of [docs-style.md](../meta/docs-style.md);
**V** and **E** facts are cited from the format pages, which hold their
evidence. The slices that build this are in modeling.md, "Slices".

## The session and the save

`geometry_edit::Target` names what a mesh came from; `GeometrySession`
holds the open targets, their source bytes, the meshes and the checks,
with no UI, as `scenario_edit::Session` does for placements. Its edits
join the map's save: `Session::set_geometry_changes` hands
`scenario_edit::Changes` three more lists, so the dirty mark, **Save map
as...** and packages count them as they count tag patches.

| List | Written by | Rule |
|---|---|---|
| `tag_patches`: `(TagId, offset, bytes)` | `MapEditor::patch_tag` | fixed length, in the tag's own space, so it reaches `sbsp` and `ltmp` in a BSP region |
| `raw_patches`: `(file offset, bytes)` | `MapEditor::patch_raw` (new) | fixed length, inside one local raw block, never the header, index or tables |
| `raw_new`: bytes, alignment, and the `(TagId, offset)` words that take its file offset | `MapEditor::append_raw` (new) | placed where the tag index starts, as `map::build` places raw data; the index and metadata follow it, header `+0x10` and `+8` grow; the metadata still ends the file, so `append_metadata` still works |

Three write classes follow from them. A target's edit is in the lowest
class that holds it, and the status bar and `geometry.check` name it.

| Class | When | Writes | Gate |
|---|---|---|---|
| **In place** | no element count changes: verts moved, coordinates or normals changed | `raw_patches` over the old block, `tag_patches` for bounds | none |
| **Grown raw** | vertex, index or part counts change | a new raw block in `raw_new`; `tag_patches` for the block info, resource entries and counts | Allow unverified geometry writes, until a game run has loaded one |
| **Grown blocks** | a tag block gets more elements (collision, a new material) | `raw_new` as needed, and the block appended to the ordinary metadata with its `(count, address)` pointed there (`Changes::tag_grown`) | Allow growing full blocks, and for a block of an `sbsp` also Allow unverified geometry writes |

A package of such a map is a BPS patch when the length changed, as for
grown scenario blocks, and its `[result] geometry` hash
(`scenario_edit::package::geometry_hash`) differs from its base's.

`crates/formats/tags/src/geometry_write.rs` (new) is the one section
encoder: `encode_section(layout: &SectionLayout, data: &SectionData)`
returns the framed raw block (`blkh`, struct, `rsrc` resources, `blkf`)
and its resource entries, for `STRUCTURE_SECTION` and `MODEL_SECTION`. It
keeps every resource it does not interpret from the source block. Its
test is the decoder: `decode_section(encode_section(x)) == x`, and on a
source block `encode(decode(block))` is the block.

## Structure BSP render geometry

- **Read.** `tags::structure::StructureRender::read`, then
  `GeometryHeader::load` per cluster and per instanced geometry
  definition. One mesh per section; a part is a face group; the part's
  material is the face material. Instances are objects placing a
  definition's mesh (`InstancedGeometryInstance::transform`).
- **Edit.** Positions are `f32` (stream `01`), texture coordinates `f32`
  (`18`), the frame three packed 11:11:10 vectors (`1b`), the lightmap
  coordinate two normalised `i16` (`1f`), the incident direction packed
  (`30`). An instance moves, turns and scales as an object, with no mesh
  edit.
- **Write, in place.** Changed vertices' stream bytes; each touched
  part's sphere (`+0x10`, `+0x2c`) and box (`+0x30`); the cluster's
  bounds (`+0x54`). An instance: the `0x58` placement's scale, basis and
  translation, kept rigid (`InstancedGeometryInstance::is_rigid`).
- **Write, grown.** The whole section through `encode_section`. Parts
  keep their source order and kinds, a new face stays in its group, and
  each part has one subpart. Recomputed: the section info counts (`00`
  to `26`, filled as `h1convert::structure::Structure::write` fills
  them), the block info sizes and resource entries, the visibility MOPP
  and its `2n` table (`h1convert::structure::visibility_mopp`, checked by
  `check_visibility`), part spheres and boxes, cluster bounds. The
  cluster's predicted resources stay, since no material is added.
- **Limits.** 65,535 vertices and 65,535 indices a section (`u16`
  counts; `h1convert::structure::SECTION_MAX`); a raw block under 16 MiB
  (`geometry::MAX_BLOCK_SIZE`); a material index below the count at
  `sbsp+0xa4`; the map under `map::writer::cache_size_limit`; raw file
  offsets under 2^30.
- **Risky.** The visibility MOPP and table: a wrong table made the engine
  write past a view's mask and fault
  ([multiplayer.md](../halo1/multiplayer.md), "The converted structure
  BSP"). `unknown_0c`, the opaque vertex count, sizes lightmap bucket
  ranges ([structure-bsp.md](../formats/structure-bsp.md)): a section
  that a lightmap bucket reference reaches keeps its vertex count and
  order, so it is refused for the grown class.
- **Deferred.** New materials (the block at `sbsp+0xa4` grows and the
  cluster's predicted resources change), new clusters, new instanced
  definitions, the second visibility code (`0x1000` entries), in-tag
  sections (`GeometryHeader::embedded_section`, 44 on the disc).

## Structure collision

The collision BSP is the block at `sbsp+0x14`
(`Bsp::decode_structure`): planes, a 3D tree, leaves, 2D references, 2D
trees, surfaces, a winged-edge table and vertices. A render edit does
not touch it. **Editing render geometry leaves the collision as it was**
and the session marks the target collision-stale; the viewport's
collision overlay shows the difference. Changing the collision is three
steps of rising risk.

1. **The compiler** (`crates/formats/collision-build`, beside `mopp` and
   `pathfind`). `build(polygons: &[Polygon]) -> Result<Bsp>` from convex,
   planar polygons with a material and flags each:
   - weld vertices; build the winged-edge table (each edge names its two
     surfaces, forward and reverse edges, so `Bsp::surface_ring` closes);
     split polygons over eight vertices (**V**: retail rings have at most
     eight, and Havok's surface shape copies a ring with no count check);
   - gather unit planes, one per distinct plane, a surface naming its
     plane with bit 15 for the flipped side;
   - build the 3D tree by recursive partition on a surface's plane
     (fewest splits, then best balance, ties by lowest plane index),
     splitting straddling polygons; a cell with no polygon left is a
     leaf, solid on the back of its last plane;
   - give each open leaf one 2D reference per plane it borders that has
     surfaces, its flip bit saying the leaf is on the plane's back, and
     a 2D tree over those surfaces in the plane's projection
     (`surface_query::PROJECTION`, `dominant_axis`);
   - set leaf flag bit 0 where a reference reaches a two-sided surface.

   It refuses what the packed fields cannot hold (32,767 surfaces behind
   a 2D child, 65,534 edges or vertices, 2^23 nodes or leaves, as
   `h1convert::scenery_collision`'s limits). Acceptance is the engine's
   own walks as `h1convert::surface_query` models them, which move to
   this crate: `Bsp::validate` passes; every surface's centroid walks
   its reference's 2D tree back to it; every reference is on its leaf's
   path on the side its flip bit says; and for a set of random segments
   the first hit of `vector` equals a brute-force ray against the
   polygons.
2. **Adding solids** to a level. A closed mesh compiles to its own small
   `Bsp` and unites with the level's through
   `scenery_collision::merge`, which appends and so keeps every existing
   plane, surface and leaf index. Each added leaf takes the cluster of
   the leaf it stands in (`Merged::stand_ins`), in the per-leaf block at
   `sbsp+0x2c`. Then the structure's MOPP is rebuilt
   (`mopp::build_structure` plus retail's other keys, as the test
   `replaced_retail_levels` does) and written over the old program when
   it fits, else as a grown block. A level with pathfinding (`sbsp+0xc4`)
   keeps its sectors; the new solid has none. This is the grown-blocks
   class: all eight collision blocks and the leaf block move to the
   ordinary metadata.
3. **Rebuilding** a level's collision from its edited surfaces (moving or
   deleting a wall). Every table that indexes the collision must follow:
   the per-leaf block (cluster per leaf, by locating each new leaf's
   centre in the old tree), portals' plane indices (their planes are
   appended first, in order), the MOPP, pathfinding
   (`pathfind::generate`, which has no instanced or object sectors),
   surface materials and breakable-surface bytes (carried as face
   attributes). Named fields cover under 15% of `sbsp`
   ([editor.md](editor.md), "The field registry"), so other holders of a
   surface, leaf or plane index may exist (**S**). This step is built
   for converted Halo 1 maps first, where `h1convert` writes every field
   ([modeling.md](modeling.md), "Converted Halo 1 maps"), and for retail maps only when a disc survey shows no other
   holder.

## Portals, clusters and visibility

The engine draws a structure by flooding portals from the camera's
cluster and draws a cluster's section only when the flood reaches it
(**E**, multiplayer.md "Clusters and portals"). So a face is seen from
where its *section's* cluster is seen, wherever the face is.

- A face stays in its section. `geometry.check` reports
  `OUT_OF_CLUSTER` for a vertex whose point locates
  (`surface_query::locate`, then the leaf's cluster) in another cluster,
  and the viewport tints it: it is drawn, but culled with the wrong
  room. Moving faces between sections (a cut and a paste, both sections
  written) is the fix.
- Portals, the cluster graph, the visibility rows (`sbsp+0x54`,
  `+0x58`), sky indices and sound palettes are shown (an overlay of
  portal polygons and cluster colours) and not edited. A wall moved past
  a portal leaves a hole in the flood's seal; the check reports a portal
  whose polygon no longer touches collision surfaces on all its edges.
- Portal and cluster editing, and new clusters, are out of scope. They
  need the cluster split, the portal limits (512, and 256 flood nodes:
  `h1convert::clusters::MAX_PORTALS`, `h1convert::flood`) and the sound
  and sky tables rebuilt together.

## Lightmaps

A cluster's lightmap is a page of the `ltmp` group's bitmap sampled at
each vertex's `1f` coordinate; instances and some clusters are lit per
vertex from bucket streams (`2e`, `2f`), reached by first-vertex offsets
([lighting.md](../formats/lighting.md)). Three policies, in the order
they are built (**Design**):

1. **Keep, marked stale.** Moved vertices keep their lightmap
   coordinates, so they keep their baked light. New faces copy the
   coordinates of the corners they came from. Every such face is
   lightmap-stale: the viewport can tint them, `geometry.check` counts
   them, and the save's log line says how many. Nothing is invalidated,
   because a level with its lightmap dropped is drawn by another shader
   path and looks worse than a stale one.
2. **Checksum.** A grown or collision write gives the structure a new
   checksum in each place that holds it (`sbsp+0x08`, each cluster's
   `+0x94`, the lightmap group's `+0x04`, the pathfinding element's), so
   the engine still pairs BSP and lightmap, and offline caches keyed by
   it (the split pages of
   [dynamic-lighting.md](../graphics/dynamic-lighting.md)) miss instead
   of serving the old level's light.
3. **Rebake.** `geometry_edit::Relighter` is a trait the session calls
   with the stale faces' world triangles and their page:
   `relight(&StaleRegion) -> Result<PagePatch>`. An implementation
   assigns new faces free texels (a chart per connected stale patch,
   packed into the page's unused area, else a new page through the
   Textures tool's grown path), lights them, and returns pixels and
   coordinates; the session writes them as a bitmap edit and `1f`
   changes. The first implementation is direct sun plus a flat ambient
   fitted to the page's neighbours; the transport of
   [live-gi.md](../graphics/live-gi.md) replaces it when that work
   lands. A level rebaked this way is a relight, not Bungie's bake
   (dynamic-lighting.md, "Approaches").

A vertex lit from a bucket keeps its place: its section is in-place
only (above). Rewriting bucket streams is deferred.

## Render models (`mode`)

- **Read.** `tags::model::RenderModelGeometry::read`; each section
  through `MODEL_SECTION`. The Modeling tool lists a model's regions,
  permutations and the six LOD sections each names, and opens one
  section as a mesh, drawn on the model's other sections in the bind
  pose.
- **Edit.** Positions are `s16` in the model's one compression box
  (types `02`, `04`, `06`, `08`, `0a`), texture coordinates `s16` in its
  UV box (`19`) or `f32` (`18`), the frame packed (`1b`). A vert carries
  its nodes and weights; copies and cuts copy or interpolate them, and
  a result with more influences than the section's vertex type holds
  keeps the largest, renormalised to 255. Markers (the groups at
  `+0x58`) are points with a rotation, moved with the gizmo in their
  node's space.
- **Write, in place.** Requantised positions and coordinates. A vertex
  outside the compression box grows the box (`ModeCompression`, patched)
  and then every section of the model is requantised, each still its own
  size. A marker is a fixed patch of its `0x24` element.
- **Write, grown.** The section through `encode_section`, keeping the
  node map and the uninterpreted blocks (`10`, `44`, `54`, `5c`) from
  the source; the section record's counts and block info; parts as
  triangle lists (flag `0x8`) with one subpart each, as
  `h1_assets::models::mode` writes them. A part that was a strip becomes
  a list, except in a sky's model, which is refused (the sky draw draws
  every part as a strip, [sky.md](../halo1/sky.md)).
- **LODs.** An edit changes one section. The others are stale, and the
  tool offers **Use for lower LODs**: a fixed patch pointing the
  permutation's lower LOD indices at the edited section.
- **Limits.** 65,535 vertices a section; node indices are bytes into the
  section's node map; the raw pointer's file. A section whose block is
  in `shared.map` or another cache is copied into the open map on the
  first grown write and its pointer made local; an in-place write to it
  is refused (the editor never writes the shared caches).
- **Deferred.** Weight painting, node edits (animations depend on the
  rest pose), new sections, regions and permutations, the second
  texture coordinate set, PRT data.

## Collision and physics models

- **`coll`.** One collision BSP per region permutation
  (`tags::collision::Collision`), the same eight blocks as the level's.
  A BSP opens as a polygon mesh (`Mesh::from_polygons` over
  `Bsp::surface_ring`); the save compiles it with `collision-build` and
  writes the blocks as grown blocks (a `coll` is one contiguous payload,
  so a block that still fits is patched where it is). A permutation with
  BSP physics slots gets each slot's box and MOPP code again
  (`h1objects::coll::bsp_box`, `bsp_mopp`). The object's small trees
  make this the compiler's first user. **From render** makes a
  collision mesh from a render section's faces, welded and joined into
  convex polygons.
- **`phmo`.** Spheres, pills and boxes are edited as primitives, in
  place: radius, half extents, end points, the transform
  (`tags::collision::PhysicsModel`). A polyhedron's vertices move while
  it stays convex; its planes and box are recomputed, its counts kept.
  Mass, centre of mass and inertia are kept and the check says they are
  stale. New shapes, rigid bodies and constraints are out of scope.

## Evidence

- **Code**: no encoder for sections exists in `crates/formats/tags`; the
  two writers are `h1convert::structure::raw_block` and
  `h1_assets::models::mode`'s `build_section`.
- **Code**: `MapEditor::queue` takes file offsets and refuses only the
  header, the index and the string and name tables, so a raw patch is
  the same queue behind a new entry point.
- **Code**: `map::build` writes local raw data after the pinned prefix
  and before the index (`TagSet::from_map`, `add_raw`); `append_raw`
  places it the same way.
- **V** ([map-builder.md](../formats/map-builder.md)): retail `sbsp` and
  `ltmp` are not relocated by the schema; a pinned map's BSP space
  cannot grow.
- **V** ([collision.md](../formats/collision.md), "What the engine does
  with a BSP"): the centroid, reference-side and leaf-flag properties
  hold on every retail structure BSP, so they are the compiler's
  acceptance tests.
- **S**: the engine follows an `sbsp` block pointer into ordinary
  metadata. A map with one loading and colliding verifies it.
- **S**: nothing but the fields listed under "Structure collision" holds
  a collision surface, leaf or plane index. A disc survey of `sbsp`
  words that are valid indices on every map, then the engine's readers,
  verifies it.
- **S**: `unknown_0c` is the opaque vertex count and nothing else reads
  vertex order. The bucket range check of structure-bsp.md is its only
  evidence.

## Not verified

- That the game loads a map with raw data appended before its index by
  `MapEditor::append_raw` (the builder's maps place raw data the same
  way and converted maps load).
- That a grown cluster section draws: its visibility code, table and
  section info as the engine's volume pass reads them.
- That a grown `mode` section draws and skins in game, at every LOD.
- That a compiled collision BSP plays as retail's does: bullets, a
  biped's ground checks, vehicles and Havok contacts on its surfaces.
- That an `sbsp` block in ordinary metadata is accepted, and how much
  tag memory a map really has past the empirical bound.
- That a new structure checksum leaves the lightmap applied and nothing
  else compares it.
- Editing a map-pack map, whose metadata is already past the multiplayer
  bound.

## Related

- [modeling.md](modeling.md): the mesh kernel, the tools, the API and
  the slices.
- [scenario-editor.md](scenario-editor.md) §3 and §5: the save these
  changes join.
- [structure-bsp.md](../formats/structure-bsp.md),
  [map-format.md](../formats/map-format.md) ("Render-model geometry",
  "Growing the metadata"), [collision.md](../formats/collision.md),
  [mopp.md](../formats/mopp.md),
  [pathfinding.md](../formats/pathfinding.md),
  [lighting.md](../formats/lighting.md),
  [map-builder.md](../formats/map-builder.md): the formats.
- [multiplayer.md](../halo1/multiplayer.md),
  [models.md](../halo1/models.md): what the Halo 1 converter writes,
  field by field.
- [dynamic-lighting.md](../graphics/dynamic-lighting.md),
  [live-gi.md](../graphics/live-gi.md): the lighting work the rebake
  hook meets.
