# Halo 1 multiplayer: the converted structure BSP

The structure BSP built for a converted map, split from [multiplayer-sandbox.md](multiplayer-sandbox.md).

| Section |
|---|
| [The converted structure BSP (**Built** as `h1convert::structure`)](#the-converted-structure-bsp-built-as-h1convertstructure) |

Part of [multiplayer-sandbox.md](multiplayer-sandbox.md).

## The converted structure BSP (**Built** as `h1convert::structure`)

`Structure::write` writes the `sbsp` and `ltmp` below as `map::build`
block trees into the base's own `sbsp`/`ltmp` slots (same datums, so
nothing that names them changes), each cluster's render geometry as a raw
`blkh` block, and adds a built BSP section; `Prepared::build` then points
the base scenario's BSP descriptor (file offset, size, address) at it.
The base's old BSP region stays in the file, unused.

- **Render geometry.** Each Halo 1 lightmap material is a mesh: its
  vertices (position, packed normal/binormal/tangent, texture and lightmap
  coordinates, which both games store alike) and its triangles from the
  render surfaces. Triangles are turned to Halo 2's winding, decided once
  per level by majority against the packed normals: Halo 1's beavercreek
  needed flipping. Each mesh is split by the Halo 1 cluster its surfaces
  are in, and each Halo 1 cluster becomes one section, its cluster's
  geometry (see **Clusters and portals**; at most 65535 vertices and
  indices, the section's `u16` counts, which no Halo 1 cluster comes near:
  the largest has 28,398 vertices). Streams written:
  `01` position, `18` texture coordinate, `1b` frame, `1f` lightmap
  coordinate, `30` incident light direction (Halo 1's lightmap vertex
  direction, the same 11:11:10 packing; the normal where a vertex has
  none). One subpart per part.
- **Every render field retail fills** (docs/formats/structure-bsp.md,
  "Geometry header", "Section struct"). Parts: flags `0xb`, as on all 2,450
  retail multiplayer parts, and the kind retail gives the template of
  the material's shader (`structure::part_kind`): 4 for every template
  under `shader_templates\transparent\` and for `water_pool`, 3 for
  `decal_simple` and `overlay`, 2 for every other (alpha-tested ones
  included: 155 of 157 retail `tex_bump_alpha_test_single_pass` parts
  are kind 2); the bounding sphere (`+0x10` centre of the box, `+0x2c`
  farthest vertex from it, at least 1 mm) and the box (`+0x30`). Cluster
  section info: shadow-casting triangles and parts are kind 2's, opaque
  parts kind 2 and 3's, as on every retail cluster with a kind 3 or 4
  part (961); rigid counts as the shadow-casting; compression flags 4 (the
  `i16` lightmap coordinates), the subpart count, lighting flags 3
  (lightmap coordinates and incident radiance; 0 for an unlit cluster).
  Cluster tail: `+0x6d` `0xff`, the structure checksum at `+0x94`, and
  the predicted resources (`+0x84`): the cluster's own geometry (type 3),
  its lightmap page (type 0, image = page) and every bitmap its
  materials' shaders name (type 0, image 0), each only when the bitmap
  has that image (the engine indexes the images unchecked); the engine
  loads these for the camera's cluster and those it sees (E
  `0x0016e5e0`). Lightmap bucket entries (types 6 to 8) are never
  written: the converted lightmap has no buckets and the engine indexes
  them unchecked. `+0x6c` stays 0: the engine reads it as the cluster's
  first sky (E `0x00130f60`) and falls back to `+0x6e` only for -1, with
  a check that can pick the BSP's own sky instead. Each section has a
  visibility MOPP with one leaf per subpart (`mopp::build` over each
  part's box) and a reorder table of `2n` entries, the leaf order and its
  inverse, which is how the engine's volume pass (E `0x00044e90`) reads
  a wholly seen subtree's keys (docs/formats/structure-bsp.md). Without
  them the pass takes the object branch and faults (pc `0x4539c`). A leaf
  per triangle with a table of `n` is wrong too: the pass reads past the
  table and sets bits past the view's mask area, into the area's word
  count, and clearing that many words runs over the view and what follows
  it (faults at `0x134925` in the clear and at `0x147b75` on a render pass
  object it has zeroed).
  `tests/render_fields.rs` surveys which 16-bit words of every drawn
  retail multiplayer cluster, part and section are nonzero, and which
  words of every retail bitmap image are none, and fails if a converted
  map differs, or if a converted section's code and table are not laid
  out as the pass reads them. `h1campaign/tests/disc.rs`
  (`every_campaign_level_s_clusters_are_drawn_as_the_engine_reads_them`)
  checks the same layout, the mask count and every predicted resource on
  all ten converted campaign levels, and their ambient sound: each
  structure's palettes name the scenario's, every cluster's indices
  and the scenario's cluster data agree, and every collision leaf plays
  its Halo 1 cluster's pair (all 700,932 leaves).
- **Bitmap images say "not cached".** Every converted image (Halo 1's
  textures, its lightmap) has `0xffffffff` at `+0x20..+0x33`, as every
  retail image does: LOD pointers 1 and 2, and the texture cache's
  handle per LOD (`+0x28`, `+0x2c`, `+0x30`). The cache reads a handle
  other than -1 as "resident in that slot" and never loads the pixels
  (E `0x0012c9fb`), so an image with 0 there draws whatever cache slot
  0 holds: the level's one flat colour. `h1bitmaps` writes -1
  (`IMAGE_NONE_WORDS`).
- **Clusters and portals** (`h1convert::clusters`). Halo 2 draws a
  structure by flooding portals from the camera's cluster (E
  `0x00173130`, `0x00172840`): from a cluster it walks the portal list at
  `+0x8c` (u16), takes each portal (`sbsp +0x5c`, `0x24` each) whose
  plane faces the right way, which is open and whose screen rectangle
  overlaps the current one, and draws the cluster on its far side. Halo 1's
  clusters, portals and leaves are kept (with no portals and every
  collision leaf in cluster 0, only cluster 0 would draw). Halo 1's portal (`sbsp +0x154`, `0x40` each) has
  Halo 2's fields in Halo 2's order: `+0x00` the cluster behind the
  plane, `+0x02` the one in front, `+0x04` the collision plane, `+0x08`
  centre, `+0x14` radius, and its polygon (`+0x34` in Halo 1, `+0x1c` in
  Halo 2, 12 bytes a vertex). Both games put the cluster at `+0x00` behind
  the plane (V, by which side of the plane each cluster's centre is: 85 of
  Coagulation's 95 and 100 of Zanzibar's 125 portals, the rest ambiguous,
  none the other way round; on Halo 1's disc 31 portals come out the other
  way against thousands this way, Guess: clusters whose centre lies
  outside them). Flags (`+0x18`) are 0: bit 3 starts a portal closed (E
  `0x00210e43`). The converted collision BSP keeps Halo 1's planes in
  order, so the plane index carries over. A cluster's portals are its
  Halo 1 list (`+0x5c`, i16). Every surface goes to each Halo 1 cluster
  whose subclusters (`+0x34`, `0x24` each, a surface index block at
  `+0x18`) list it, and a handful (37 at most, b40) are in none, which
  Halo 1 never draws either. A surface that crosses a portal is listed by
  every cluster it is in (760 of Blood Gulch's 5503): Halo 1 draws it
  when any of them is seen, Halo 2 draws a cluster's section only when
  the flood reaches that cluster, so the surface goes to every cluster it
  is in (to its first cluster only, it would vanish when seen from another
  of its clusters without the first). `h1convert::flood` models the
  flood (one node per chain of portals, at most 256, E `0x001729d9`; the
  camera against the portal's collision plane within 1/64, E
  `0x00173890`; the portal's screen polygon clipped to the node's, E
  `0x00173380`) and `tests/portal_flood.rs`
  (`bloodgulch_surfaces_in_two_clusters`) proves it from every spawn
  turning through 360°: of the surfaces seen (centre in the frustum, the
  sight line clear of solid), drawn with the first cluster 189 went
  missing in 157 of 2556 views, drawn with each none. The same test
  finds the portals sound: all 70 name the cluster behind their plane
  first (the collision BSP's leaf 5 cm either side of each centre), the
  flood never comes near its 256-node limit (94 at most) and reaches
  every cluster a seen surface is in. Each collision leaf names its Halo 1 cluster (leaf
  `+0x08`). `tests/h1_clusters.rs`: every portal lies in its plane and is
  on both its clusters' lists, and every cluster with geometry is reached
  through portals from every other, on every level (Chiron TL-34 has no
  portals at all: its rooms are sealed, and Halo 1 draws them one at a
  time too). The v8 visibility data (`+0x54/+0x58`) carries the source
  BSP's directional cluster bit rows in the original cluster order.
- **Materials** (`h1convert::textures`). Each render material names its
  rebuilt source shader. Collision palette entries keep the source shader
  and translate its physical material into the globals index. Both
  palettes preserve source order. Missing shaders stop conversion.

- **Halo 1's own textures** (`h1convert::textures`). The structure's
  shaders are rebuilt as `shad` records from the definition, with the
  appropriate Halo 2 `stem` and Halo 1's converted bitmaps. An `senv`
  supplies base `+0x88`, primary detail `+0xb8`, secondary detail
  `+0xcc`, micro detail `+0xfc` and bump `+0x128`. The base is sampled
  at 1× and each other map uses the scale before its reference. On
  `tex_bump_detail_blend_detail`, the colour pass uses the base alpha
  to blend the primary and secondary details, then applies the micro
  detail. The installer adds the rebuilt shader and a material head
  that points to it (`textures::plan`, `Retexture::install`).
- **Terrain detail blend and shader image list (Built).** Halo 1's
  `senv +0xb0` and `+0xf4` select the operations for the detail
  blend and micro detail independently (E `0x0007785d`.. `0x000778dd`).
  For surface colour `c` and detail colour `d`, the XBE emits
  `2*c*d`, `c*d` or `c+2*d-1` for values 0, 1 and 2. Where the
  opaque blend-detail layout is available, the installer emits a
  colour pass for non-default operations and connects it with the
  existing light, shadow and fog passes. The pass bypasses an operation
  when it has no detail bitmap. On the owner's disc both fields are
  zero except for the shaft cover in `b30`, which uses value 1 in both
  and has neither detail map. The rebuild also fills the `shad +0x2c`
  image list with the new compiled bitmap datums, numbering repeats
  from zero (V, the retail instance layout in
  [shader-instances.md](../formats/shader-instances.md)).
  `terrain_detail_modes_are_read_and_installed` checks the non-default
  program against both discs, and `every_halo1_shader_and_sky_shader_rebuilds`
  checks `+0x2c` against the rebuilt bindings.
- **The sky is Halo 1's** (`h1convert::sky`). Cluster `+0x6e` is the
  cluster's sky index into the scenario's skies (`scnr+0x8`, 8-byte
  references; E `0x0014b38d`..`0x0014b3c4`, where -1 also falls back to
  the first sky when the scenario has one). The engine draws one sky,
  the camera's cluster's (E `0x0002b865` calls `0x0014b360`, which reads
  that cluster's `+0x6e`, and passes its index to the sky draw
  `0x0003ede0` through `0x004b9eec`); Halo 1 names a sky per cluster
  (`+0x00`, i16, -1 none). Each converted cluster keeps Halo 1's, and a
  cluster with none (an interior) takes the nearest cluster's through
  portals, so a window still shows the sky outside
  (`clusters::cluster_skies`). A cluster no sky reaches names none
  (`0xff`): `0x0014b360` turns -1 into sky 0 only when the scenario's
  `+0x12` has bit 1 and it has a sky (`0x0014b399`..`0x0014b3a6`), and
  converted scenarios leave the bit clear, so the engine draws no sky
  there, as Halo 1 doesn't. A cluster no sky reaches must name none: a10's
  cryo bay (`a10a`, 33 clusters, none with a sky or a portal to one) drew
  a sky as a flat grey slab over two thirds of the view when it named sky
  0 (Run: the camera's cluster's `+0x6e` set to `0xff`, the bay drew whole
  and the sky's draw left the frame). Each
  sky the Halo 1 scenario names is converted by `h1sky` (model, lights,
  fog colour, and a model scale that keeps it behind the level;
  docs/halo1/sky.md) and added as a new tag, and every
  element of the base's skies block names one (element `k` Halo 1's sky
  `k`, or Halo 1's last), so none names the base's. Rat Race and Chiron
  TL34 are indoors and name no sky: the base's stays, unseen. `disc_maps.rs`
  checks every converted map's skies. The base's decorator block
  (`convert::BASE_BLOCKS`: `+0x378`, 2 `DECR` on Coagulation, D) is
  emptied: the converted clusters name none. The background sound and
  sound environment palettes (`scnr+0x250`, `+0x258`) name Halo 1's
  (below, "Ambient sound"). The base per map (`convert::MAPS`):
  Coagulation for the maps with vehicles (below), Beaver Creek otherwise.
  Waterworks (snow, and a wraith) would suit Sidewinder and Ice Fields,
  but Sidewinder's placements grow Waterworks' metadata past the engine's
  tag memory, so they take Coagulation too.
- **Lightmap** (Halo 1's own). One group, the structure checksum (FNV-1a
  of the positions), the bitmap reference `+0x18` naming the converted
  lightmap bitmap, and per cluster a render info `{page, -1}` (`{-1, -1}`
  for unlit clusters). A Halo 2 cluster samples one page, its render
  info's; a Halo 1 cluster's surfaces sample several (22 of Blood Gulch's
  30 clusters use more than one), and a page serves several clusters. So
  each lit cluster gets a page of its own (`clusters::lightmap_pages`,
  `h1bitmaps::atlas`): its triangles fall into charts (triangles sharing
  vertices), each chart takes the texels bilinear filtering reads under
  its lightmap coordinates on its Halo 1 page, charts of one page that
  overlap or touch are merged so no texel is taken twice, and the pieces,
  each with a one-texel border repeating its edge, are packed by shelves
  into the smallest power-of-two page. The lightmap coordinates move with
  them. Surfaces Halo 1 doesn't lightmap (lights, glass, water) sample a
  white piece. The pages keep Halo 1's format (R5G6B5) and swizzling,
  at half Halo 1's light (`h1bitmaps::atlas::to_halo2_light`): Halo 2's
  lighting pass draws `2 × page × albedo`
  (`tests/lightmap_light.rs`, the pass run on the CPU for every retail
  `tex_bump` and `tex_bump_detail_blend_detail` on Coagulation), and a
  Halo 1 page is the albedo's multiplier itself. Blood Gulch and
  Coagulation, one valley with one ground albedo, show it: half of
  Blood Gulch's texels are at full light in their brightest channel,
  99 % of Coagulation's at most 0.85. Taken as they are, the ×2 would draw
  Blood Gulch's ground saturated yellow and its cliffs bright orange.
  `tests/h1_clusters.rs` samples every lit vertex on both bitmaps
  (bilinear, clamped) on every level: all agree with half Halo 1's
  light within half a 5-bit step. The
  pages hold 1.6 to 3.9 times Halo 1's texels (power-of-two padding and
  the borders): Blood Gulch 220,160 for 126,016, c20's largest structure
  7.7 million for 2.9 million. The bitmap takes the
  slot of the base's own lightmap bitmap, which nothing else names
  ([Freed slots](multiplayer-vehicles.md#vehicles)), through `h1models::Converted::install_at`.
  The lightmap coordinates (stream `1f`, in every cluster section as on
  retail maps) are page-relative, in [0, 1]. Retail lightmaps also carry
  per-vertex lighting buckets (`+0x40`, streams `30` and `2f`); clusters
  read none (`+0x60` empty, as on Coagulation).

  **Colour pages with no palette (E).** Retail pages are P8 with a
  palette per cluster; Halo 1's are R5G6B5. Nothing in the engine needs
  P8: the structure renderer stores the cluster's page whatever its
  palette, and binds a palette at stage 3 only when one is named (E
  `0x0004d3ff`..`0x0004d420`: `cmp edx, -1` skips the bind, the page goes
  to `[0x485a60]` either way), and the texture sampler returns a colour
  for both. The CPU lightmap query that lights objects (E `0x000d2f90`)
  samples the page through the general pixel reader (`0x001362b0`) and
  looks the value up in a palette only when the image's format is P8
  (`cmp word [esi+0Ch], 12h` at `0x000d309a`); otherwise the sample is the
  colour. So the pages stay as Halo 1 stored them: no quantizing.

  **Every converted image is a cache resource (`0x200`).** Without the
  bit a converted level draws without light. A capture of a10
  (frame 2806) showed why: the structure's lighting pass (pixel shader:
  `r0 = t3 · (1 + v1.a)`, then `× t0 × 2`, `t0` the screen copy of the
  albedo pass) had nothing bound at stage 3, and no R5G6B5 texture was
  created in the whole run. The binding itself was right (group, checksum
  and page all pass `0x00018bfe`); the texture cache returned null. It
  loads an image only when its flags have `0x200` (E `0x0012cd26`,
  `0x0012ce81`: `test byte [image+0Fh], 2`); otherwise it hands back the
  image's runtime texture (`+0x50`), never set for these. Every retail
  image has the bit (all 1,686 of Coagulation's and 2,231 of Delta
  Approach's: `0x203` DXT, `0x1209` swizzled, `0x290` linear) and no Halo
  1 image does. `h1bitmaps` sets it on every converted image
  (`IMAGE_CACHED`), which covers the lightmap pages, Halo 1's textures on
  structure and objects, skies and the map card. Both disc tests check
  every image in every converted map (`check_images`).

**Proof (tests).** `structure_tests.rs` writes an invented two-mesh level
and a 30,000-triangle one into an invented map and reads them back with
Halo 2's own decoders: `tags::structure` (clusters, parts, materials,
every vertex, triangles, the lightmap group), `tags::collision` (the
collision BSP equals the converted one), and the root's leaves,
visibility, bounds, Havok box, MOPP block and the one-element blocks. The
ignored `beavercreek_converts_onto_its_halo_2_base_and_reads_back` does
the whole conversion on the real maps: Halo 1's beavercreek onto the
disc's Beaver Creek gives one cluster, 4242 vertices, 3317 triangles, 18
materials, 13 collision materials, 70 spawns, 88 flags, 28 items; the map
reads back with a valid checksum, its scenario's descriptor leads to the
new structure, every cluster loads, and the trigger volumes are parked.
Those two tests use a stand-in MOPP block (a header and `00`, which emits
nothing); nothing written for play does.

Not covered by the engine reading and left empty: root blocks at `+0x4c`,
`+0x94`, `+0x148` and `+0x1d0`, which every retail multiplayer BSP fills
(see the disc survey under "Open items"). No direct read of them from
the active BSP pointer was found; if the game faults on load, these are
the first suspects.
