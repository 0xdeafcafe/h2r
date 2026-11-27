# Live art: packing, pipeline and limits

## Packing

The last step of an extraction (`crates/live/live-server/src/art_pack.rs`,
`pack`) readies the art for the web, so both homes send the same bytes:
the art server (`/art/game/`, `--share-game-art`) and the published copy
the Worker redirects to ([live-worker.md](live-worker.md)).

- **Pictures are WebP.** Surface pictures (`tex/<id>.webp`) are lossy
  quality 90 at the disc's full stored size (they were JPEG quality 80,
  PNG for cut-outs, at most 512 pixels a side); pre-drawn views
  (`views/<id>.webp`) lossy quality 85 (they were barely compressed PNG,
  1.3 MB each). A cut-out keeps its alpha exactly (a lossless alpha
  plane) and the colour under clear pixels, so it cuts and filters as it
  did. Every browser the app supports decodes WebP with
  `createImageBitmap`, off the main thread. The stats site's small
  pictures (emblems, ranks, medals, previews) stay PNG: emblems are
  composited from their exact masks.
- **Brotli and gzip copies.** Every glb and JSON file gets `<file>.br`
  (quality 11, 16 MiB window) and `<file>.gz` (level 9). The art server
  sends the `.br` copy to a browser that takes Brotli, else the `.gz` one;
  the publish uploads the `.br` copy with `Content-Encoding: br`. Brotli is
  decoded by the browser's network stack, before any script sees it.
  Pictures get no copies (they are compressed already). A copy older than
  its file (by modification time) is stale and ignored, by the server's
  redirects and by the publish, which then uploads the plain file: whoever
  rewrites a file after extraction (`bun run banner` rewriting
  `manifest.json`) writes its copies again, and a copy left behind cannot
  go out in its place.
- **Meshopt, where it pays.** A glb's vertex and index data is compressed
  with `EXT_meshopt_compression` (meshoptimizer's codecs, lossless, no
  quantisation or filters; the index codec may rotate a triangle's corners,
  keeping its winding) only when that makes its Brotli copy at least 5 %
  smaller. On unquantised floats meshopt's byte deltas defeat Brotli's
  matching: every level's `map.glb` came out bigger on the wire (all 59
  levels together 120 MiB against 74 MiB as Brotli alone), so levels stay
  plain; 10 model glbs keep it. A glb that uses it lists it in
  `extensionsRequired`; `web/src/viewer/gltf.ts` gives every glTF loader
  meshoptimizer's WebAssembly decoder (shipped with three.js). Quantising
  positions, normals and texture coordinates (`KHR_mesh_quantization`) is
  what would make meshopt pay for levels; not done yet.
- **Content-addressed by extraction.** Published files live under
  `art/<stamp>/`, the stamp a hash of every file's path and size, which
  form of it is uploaded (plain, Brotli or gzip) and that form's size, and
  the manifest: a new extraction, or any change to what would be uploaded
  (a copy written again or gone stale), is a new set of addresses, so
  every object is
  `Cache-Control: public, max-age=31536000, immutable`, the manifest too.
  The Worker's redirect to it is cacheable for five minutes (a
  `?v=<stamp>` address for a year). The web app loads the manifest
  through the redirect and every other file from the directory it was
  redirected to (`web/src/lib/gameart.ts` `artBase`): no redirect per file.
- **Coverage.** `art_pack::coverage` follows the manifest as the viewer
  and the stats site do: `images`; per map its `map.glb`, `views.json` and
  each view's picture, the pictures each glb names and its sky
  (`asset.extras.sky.glb`, with the sky's own pictures); every biped,
  weapon and vehicle glb; `catalog.json`. `h2 live assets publish`
  refuses art whose manifest names a file the directory lacks, and prints
  what it holds.

Measured on the retail disc, the map pack disc and the Halo 1 disc (59
levels: 36 Halo 2 and map pack, 23 converted Halo 1; 91 models), extractor
18 against 21, bytes as stored and as sent (the compressed copy when there
is one):

| | files | before, stored | before, sent | after, stored | after, sent |
|---|---|---|---|---|---|
| level geometry (`map.glb`) | 59 | 343.7 MiB | 116.6 MiB (gzip) | 343.7 MiB | 73.6 MiB (Brotli) |
| surface pictures (`tex/`) | 4,319 | 71.3 MiB (≤ 512 px JPEG/PNG) | 71.3 MiB | 64.6 MiB (full size WebP) | 64.6 MiB |
| pre-drawn views | 295 | 370.8 MiB (PNG) | 370.8 MiB | 19.8 MiB (WebP) | 19.8 MiB |
| models (bipeds, vehicles, weapons) | 91 | 89.0 MiB | 89.0 MiB | 87.9 MiB | 51.9 MiB |
| skies (`sky.glb`) | 54 | 2.8 MiB | 2.8 MiB | 2.8 MiB | 1.8 MiB |
| JSON (manifest, views, catalogue) | 61 | 0.3 MiB | 0.3 MiB | 0.3 MiB | 0.05 MiB |
| stats pictures (PNG) | 299 | 4.2 MiB | 4.2 MiB | 4.2 MiB | 4.2 MiB |
| **total** | 5,178 | **882 MiB** | **655 MiB** | **523 MiB** | **216 MiB** |

The biggest level, `a10` (Halo 1's Pillar of Autumn), sends 4.5 MiB (was
15.6); the largest now, `03a_oldmombasa`, 6.8 MiB.

Loading a level in headless Chrome (the development Mac, cache off, the
harness loads `map.glb` with the viewer's loader and streams its pictures
with `textures3d.ts`; median of three, ms to geometry / fully textured,
and what came over the wire):

| | before | after |
|---|---|---|
| `a10`, local | 156 / 211, 6.9 MiB | 204 / 271, 4.7 MiB |
| `a10`, 50 Mbit/s | 911 / 1233 | 602 / 944 |
| `03b_newmombasa` (189 pictures), local | 134 / 271, 8.7 MiB | 149 / 642, 6.9 MiB |
| `03b_newmombasa`, 50 Mbit/s | 834 / 1797 | 661 / 1757 |

On a real connection the smaller bytes win. Locally, decoding the full-size
pictures (four times the pixels of the old 512-pixel cap) costs time after
the level is already drawn; GPU-ready textures (KTX2/Basis) are the next
step for that and for video memory.

## Pipeline

1. **Maps.** The multiplayer and campaign maps, found by cache type, each
   opened with its shared maps (`live_art::Disc`).
2. **BSPs.** The scenario's structure BSP block (`scnr+0x210`, stride
   `0x44`, BSP tag at `+0x14`; docs/live/live-gamestate.md). Every BSP is used
   and becomes its own mesh (steps 3 to 5 run per BSP); the views draw them
   all together, except cinematic ones. A BSP that fails to read is skipped
   on its own. **Cinematic BSPs**: with more than one BSP, the gameplay
   ones are BSP 0 (where a level starts), both ends of every BSP switch
   trigger volume (`scnr+0x130`, 0xe-byte entries: trigger volume, source
   and destination BSP as i16s), and every BSP the scenario's scripts
   switch to by number (`(switch_bsp <n>)` in the scripts decompiled with
   the disc's own `default.xbe`, as `h2 scripts` does). The rest are only
   loaded by cinematics, which switch by BSP name: they are flagged
   `cinematic`, kept in `map.glb` and left out of the views, the frame and
   `bounds` (unless every BSP would be). If the scripts cannot be read no
   BSP is flagged and the level lists `map/<stem>/cinematic` under
   `skipped`.
   **Machines** come next: the scenario's machine placements (`scnr+0xa8`,
   stride `0x48`) and palette (`scnr+0xb0`, stride `0x28`, tag reference
   first), from the object type table (docs/formats/objects.md). A placement record
   starts with the palette index (`+0`), position (`+8`), yaw, pitch and
   roll in radians (`+0x14`) and scale (`+0x20`, 0 meaning 1): the public
   scenario object layout, checked on the disc by the retail machines
   landing on their levels (the Zanzibar wheel stands in its gate). The
   rotation is `Rz(yaw) · Ry(−pitch) · Rx(roll)`: forward is `+x`, pitch
   tips it up, roll turns left (`+y`) up. The machine's `obje` model
   (`hlmt`) gives the `render_model`, decoded by `tags::model`, vertices in
   model space. Each machine is read once per level. A machine without a
   model is not drawn; one whose model fails to read or has nothing drawn
   is listed under `skipped` as `map/<stem>/machine_<index>` (on the retail
   disc: a few volumes and trays with only undrawn parts, and nine campaign
   machines whose `hlmt` is not in the level). Machines are drawn in every
   view and cast shadows, but the top view's cutaway (step 8) neither cuts
   them nor counts their surfaces: a closed model hanging over a floor
   would otherwise read as a roof and vanish. They are not part of the
   frame, `bounds` or the triangle counts.
3. **Triangles.** `tags::structure::StructureRender` and the shared section
   decoder ([structure-bsp.md](../formats/structure-bsp.md)): every cluster as stored and
   every instanced-geometry placement transformed (`translation + scale ×
   basis`). Only parts the engine draws (kinds other than 0 and 5). Vertex
   normals come from the tangent-frame stream, rotated for placements. A
   section that fails to decode is counted and skipped.
4. **Classes.** The BSP material block (`sbsp+0xa4`) is read with the
   render-model material layout: 0x20-byte elements, shader tag ID at `+0xc`
   ([map-format.md](../formats/map-format.md), `ModeMaterial`). It holds on every retail
   multiplayer BSP (all 1,101 materials resolve to `shad` tags); if any
   element does not resolve on some disc, every material there is unknown.
   The shader's template comes first (`shad +0`, a tag reference: the
   template's tag name, by generic words in its last path component and
   its folder): sky templates are dropped; decal or overlay layers (named
   so first: `overlay`, `decal_simple`; an opaque `..._detail_overlay` is
   a solid surface blending a detail map) are decals; additive ones
   (`add`: glows, light shafts and light volumes, which have no solid
   surface) and cloud templates (spray, falling water) are effects. Layers
   are not level: they go to the layer nodes (step 7); a template in a `water` folder or named
   for water or waves is water; another `transparent` template is glass
   unless the shader's name says otherwise. Then the last path component
   of the shader's own tag name is split into words and matched against
   generic English words only (nothing from the game is named in the
   code): `sky`, `cloud(s)`, `fog`, `mist`, `haze` mark surfaces that are
   not level and are dropped; `glass`; `water`, `waterfall`, `pool` ...;
   plant words; `metal…`, `steel…`, `rust…`, `grate…` .... Machines'
   shaders go through the same rules. A triangle
   facing up (normal z ≥ 0.6) is `floor` unless the name says water, foliage
   or glass; other named classes come next; then steep faces are `wall` and
   overhangs and ceilings `default`.
5. **Mesh.** Backdrop goes to the BSP's backdrop mesh: every triangle of
   at least 4,000 square units that is not water (the biggest level
   triangle on a retail multiplayer map is about 1,500; campaign sea-floor
   and terrain planes run from about 5,000 to 100,000), and, grown from
   each, every triangle reached through shared corners that has the same
   surface and lies in that triangle's plane (normal within about 6°,
   corners within 0.5 units), so a plane goes whole instead of leaving its
   smaller triangles behind as shards. Sea and lake surfaces stay in the
   level. Drop non-finite, degenerate (area < 5e-7)
   and repeated triangles; replace a vertex normal that is missing or points against its
   triangle with the face normal; weld vertices per class on a 1/1024-unit
   position grid and 1/64 normal grid. Back faces are not removed from the
   model: whether a face looks into solid rock cannot be told from render
   geometry alone.
6. **Frame.** Levels sit inside large backdrops (distant terrain, cliffs, a
   ground plane far below a floating level), so the views are fitted to and
   clipped by a frame. Triangle centroids are counted on a 64 × 64 grid over
   the level's x/y extent; cells with at least a tenth of the
   90th-percentile count are dense; every 8-connected group of dense cells
   holding at least 15% of the largest group's triangles is kept, grown by a
   cell. z covers the kept centroids, except a sparse tail (under 5%)
   separated by a gap over 10% of the range. The frame is padded by 3%.
7. **Layers.** A layer's picture is its shader's diffuse map (for a glow,
   the first map that is not an engine default, bump, normal, cube or
   reflection map), decoded as the player models' textures are and
   shrunk to 256 pixels. A decal or `alpha` effect without a picture is
   dropped (it would be a solid slab); a glow without one is kept as a
   faint grey. A decal's blend then comes from its picture: one with
   alpha blends by it (unless cut out); an opaque one centred on
   mid-grey (no more than a quarter of its pixels over 160) doubles,
   else multiplies. Triangles are welded per material (1/1024-unit
   position, 1/64 normal, 1/4096 texture coordinate).
8. **Sky.** The scenario's skies block (`scnr+0x8`, 8-byte tag references,
   ID at +4); the first sky with a model, else the first. Of the `sky `
   tag: the render model reference at +0 (ID at +4), the model scale at
   +0x14, the atmospheric fog block at +0x48 (0x18-byte elements: colour,
   maximum density, start, opaque distance), the secondary fog block at
   +0x50 (the same; used when there is no atmospheric fog) and the clear
   colour at +0xa0. The lights block at +0x78 (0x34 bytes each: heading
   and pitch at +0xc, a fog block at +0x2c whose first element has
   colour at +4 and maximum density at +0x10) gives the `glow`. That record is the
   light's parameters (Halo 1's sky lights: flags, colour at +8, power at
   +0x14), so the names are the extractor's, not the tag's; the viewer
   does not draw it. A sky
   that cannot be read is listed under `skipped` as `map/<stem>/sky`.
   **Weather** (`live_weather.rs`): per BSP, the weather palette at
   `sbsp+0x84` (0x88 bytes: a 32-byte name, the `weat` reference with its
   ID at +0x24), the weather polyhedra at `sbsp+0x8c` (0x18 bytes: a
   bounding sphere, then a block of 0x10-byte planes) and the clusters at
   `sbsp+0x9c` (0xb0 bytes: bounds at +0x54, the weather palette index,
   a short, at +0x76). A BSP whose weather cannot be read adds none.
   Each view's camera is then fitted to what it draws, not to the frame's
   box: the corners of every drawn triangle cut to the frame (plus 1%),
   projected, with a 3% margin. A level that fills only part of its frame
   box (a tall, narrow structure like a sky platform) still fills the
   picture.
7. **Pictures.** `live_render`: each view is rasterised at 2 × 2 samples per
   pixel into depth and packed normal/class buffers, culling back faces and
   anything outside the frame (plus 1%). Shading per sample: Lambert from the
   sun with a 2048² shadow map (sun-facing casters inside the frame, 3 × 3
   taps); hemispheric ambient (cool sky, warm ground); screen-space ambient
   occlusion from 8 depth taps at a 1.2-unit radius; a height tint (lower is
   darker); thin darker lines at silhouettes, depth steps and creases. A
   pixel whose samples lie on one flat surface is shaded once. Glass is not
   drawn. Samples are averaged in linear light; alpha is coverage.
8. **Cutaway (top view).** Render geometry faces into the playable space,
   so going down a vertical ray an up-facing surface starts solid and a
   down-facing one (a ceiling) ends it. On a grid of 2 × 2 output pixels the
   nearest 8 surfaces of both facings are kept per cell. A cell is cut where
   a ceiling has an up-facing surface somewhere above it (a roof, or an
   upper storey) and a floor at least 0.6 units below it (headroom for a
   crouching player): everything nearer than that ceiling is not drawn
   there, nor its shadow. Single-sided ceilings with nothing above them are
   already invisible from above and need no cut. Machines take no part. Cut regions are then
   opened (eroded and dilated by a 0.8-unit square), so regions narrower
   than about 1.6 units, such as bridges and walkways over open ground,
   keep their top. Walls have no ceiling inside them, so they keep their
   tops and read as rims around opened rooms. The iso views and `map.glb`
   are not cut.
10. **Looks.** For each BSP material whose surface is solid level (not
   glass or water), and each machine's and object's model material: the
   shader's template (`shad +0`) must not be in a `transparent` folder or
   be an `overlay` (named so first: `overlay`, `overlay_...`; an opaque
   `tex_bump_detail_overlay`, Coagulation's rocks, is a solid surface and
   has a look); its base map is what the shader draws on texture
   stage 0 (`live_basemap`): the resolved material's (`tags::material`,
   docs/shader-instances.md) first level of detail, pass by pass, the
   instance bitmap on stage 0, skipping engine defaults (`rasterizer`,
   `default_bitmaps`) and bump maps (`P8Bump`); its tiling is that pass's
   vertex constant register 18 x and y (stage k's scale is register
   18 + k: a `tex_bump` pass scales base, detail and bump; a terrain
   blend's accumulation pass scales its colour overlay and up to four
   detail maps). When the shader does not resolve or draws no such
   bitmap, the first of its bitmaps (`shad +0x20` postprocess) whose name
   says diffuse (`live_models::bitmap_role`; `_bmp` names bump maps) at
   scale 1. Names alone pick a bump map on Outskirts' beach
   (`zanzibar_sand_bmp` comes first), which drew as black and white
   noise; of 7,113 solid shaders on the disc 306 pick another bitmap or
   scale by the passes, most a scale (a carpet 6 × 6, rubber mats 16 ×
   16). Decoded once per bitmap (the
   largest stored level, DXT and swizzle as `assets::decode_image` does),
   box-filtered down to 2048 pixels (none is larger); solid looks are made opaque (their
   alpha is a specular or blend mask); cut-out looks (`alpha_test`) have
   their alpha stretched so its strongest value is opaque, then doubled,
   so cutting at a half keeps what is at least a quarter: the leaves of
   the campaign's trees and bushes keep only 0.5 % to 3 % of their pixels
   at a half. Triangles keep their stored texture coordinates and weld
   on them too (1/4096). The plasma pass uses layer 15 instead. Ascension,
   Midship and Ivory Tower have kind-4 parts on their placed lift models.
   Lockout's beam is in its BSP. Each resolves the same additive pass
   with two noise maps on stages 1 and 2 and a mask on stage 3. The
   extractor reads the time-driven UV properties by destination
   component and preserves the individual periods and directions.
11. **Scenario objects.** The placement blocks of the object type table
   (docs/formats/objects.md) other than machines: scenery `+0x50`, vehicles
   `+0x70`, equipment `+0x80`, weapons `+0x90`, controls `+0xb8`, light
   fixtures `+0xc8`, sound scenery `+0xd8` and crates `+0x328` (strides
   0x5c, 0x54, 0x38, 0x54, 0x44, 0x54, 0x50, 0x4c; palette 8 bytes on,
   0x28-byte entries), read like machine placements, plus the zone at
   `+0x2c` (i16, kept when it is one of the scenario's BSPs). A placement
   whose flags (`+4`) have bit 0 set is created by a script, not at the
   start, and is left out unless the scripts read below create it: on the disc these are the cutscenes' props
   (capital ships, a ring world, drop pods stacked under a level), the
   weapons a cutscene hands out and vehicles brought in later. Then the
   netgame equipment (`scnr+0x120`, 0x90-byte entries; docs/game/forge.md):
   position `+0x40`, yaw, pitch and roll `+0x4c`, the item at `+0x58`; an
   item or vehicle collection (`itmc`, `vehc`) is drawn as its heaviest
   entry. Every object's model is read once per level (`obje` model,
   `hlmt` render model, as machines). Objects with no render model or no
   drawn triangles (anchors, invisible helpers), and objects whose own
   tag name has the word `invisible`, are left out quietly; others that
   fail are listed under `skipped` as `map/<stem>/object_<kind>_<index>`.
   On the retail disc: Lockout 50 objects (30 spawns), Zanzibar 165,
   campaign levels 17 to 854.

   **Scripted spawns** (`live_scripted`). The scripts, decompiled once
   per map with the disc's executable (the same source the cinematic test
   reads), are read as text for `object_create`, `object_create_anew` and
   their `_containing` forms (scenario object names, `scnr+0x48`, stride
   `0x24`: name `+0`, object type `+0x20`, placement `+0x22`), and
   `effect_new` (at a cutscene flag, `scnr+0x1e0`, stride `0x38`: name
   `+4`, position `+0x24`) and `effect_new_on_object_marker` (at the named
   object's placement). Static scripts are followed through the scripts
   that call them (four levels deep). A `continuous` script's period is
   the sum of its literal `(sleep N)` ticks at 30 a second (at least one
   tick); `sleep_until`, a computed sleep or `(sleep -1)` leave it unknown.
   A placement any such script creates is drawn even when its flags say
   it is not made at the start, and carries the script. Names the
   scenario lacks are listed as `map/<stem>/scripts`; a level whose
   scripts do not decompile lists the reason there.
12. **Write.** `map.glb` as soon as a map is built, then every (map, view)
   picture in parallel, then `views.json`; the manifest is written last by
   the art extractor.

Maps load on all CPUs, then all pictures (five per level) are drawn on all
CPUs. On a 10-core machine the whole geometry step takes about 7 s for the
12 retail multiplayer maps and 15 campaign levels (0.02 to 0.36 s to load
each; most of the rest is drawing, 1 to 3 s of CPU per level).

## Checking against your disc

```sh
cargo test --release -p cli geometry_from_the_disc -- --ignored --nocapture
```

writes the outputs, a timing report, the shader names behind the classes,
a contact sheet of every view and, under `cutaway/`, each top view with and
without the cutaway side by side, and under `machines/` the top and
`iso-ne` views of two multiplayer maps with large machines, `bsps.txt`
(each level's cinematic BSPs and backdrop triangles per BSP) and, when an
earlier extraction's art directory is at `$H2_GEOMETRY_BEFORE` (else
`BEFORE_DIR` in the test), `compare/<stem>.png` and `compare.png`: that
extraction's top and `iso-ne` views beside these. All go to a directory
outside the repository (the path is `$H2_GEOMETRY_CHECK`, else `CHECK_DIR`
in the test). `shader_templates_on_the_disc` (same flags) writes every BSP
material's template, shader, triangle count and surface by name alone and
with the template there, and the materials whose surface the template
changed; `features_from_the_disc` writes `features.txt`, every level's
teleporters, links and lifts. Do not commit anything they write.

## Limits

- The top view's cutaway shows one floor per spot: where storeys stack, the
  highest floor that has a ceiling over it wins, so a walkable roof over a
  room shows the room, and a player on that roof is plotted over the room.
  Roofs of spaces under 0.6 units high, or rooms narrower than 1.6 units,
  are not cut.
- The iso views are not cut away; they cull walls facing away from the
  camera, which opens rooms facing it.
- Classes from names are a heuristic; unnamed surfaces fall back to facing.
- The frame is a heuristic too. It keeps every retail map's playable area on
  the disc this was checked against, but a map with a dense, detailed
  backdrop could be framed wider than its play space.
- Lift volumes are in the objects' physics models (phantoms), which are
  not read: a lift's size comes from its render model or bounding sphere,
  or is nominal. Campaign lifts (lift platforms, gravity fields, lift
  effects) are not listed.
- Textures are each shader's base map alone: no lightmaps, detail or bump
  maps, and none of the shader's tints, so surfaces read flatter and
  sometimes paler than in the game, and terrain that blends several maps
  shows one. Cut-outs are kept where alpha is at least a quarter of the
  picture's strongest (step 10).
- Machines and objects are at rest: a door is drawn as its model's default
  pose has it, the wheel does not turn, and a vehicle sits in its model's
  rest pose. Each object is drawn as its regions' first permutations, not
  the variant the placement names. Objects a script creates are drawn
  where the scenario places them, once, whatever the script does next
  (the period is in `extras.script`); a script that creates an object by
  a computed name, from a folder, or under a condition is read only as far
  as its literal calls go. Bipeds are not drawn (AI come from the frames).
  A multiplayer spawn shows
  the heaviest entry of its collection: the game rolls it, and gametype
  settings can swap it.
- Campaign levels sit in large backdrop planes (sea or ground around the
  playable strip). The biggest planes go to the backdrop nodes, but
  smaller backdrop terrain stays in the level and the frame keeps some of
  it in the views.
- The cinematic test only knows switch volumes and numeric script
  switches: a gameplay BSP reached only some other way (a script switching
  by name in play) would be flagged cinematic.
- Template words decide what is not level: an additive or overlay
  template used for a solid surface would become a layer.
- Layers are drawn without the game's shader constants, animation or
  lightmaps: a glow is its picture added, a waterfall does not flow.
  Decals on dynamic objects, and particle systems (campaign fires, smoke,
  sparks) are not in the BSP and not extracted.
- Scripted effects are marks, not effects: no particles or projectiles
  are simulated, and an effect played on an object marker is placed at the
  object's origin. Projectiles a weapon or a device fires on its own (no
  script) are not read.
- Player and weapon models (`model/...`) are the disc's: a mod that
  changes a weapon's or a biped's model gets its level and placed objects
  from the patched map, but players are drawn with the retail models.
- A level with several skies (one per zone) gets only the first with a
  model. Additive sky layers (haze, glows) are left out by the viewer:
  without the shader's constants they wash the sky out.
