# Map geometry and pre-drawn views

`h2 live assets extract` (and `h2 play --live`, once per disc) writes each
multiplayer and campaign level's geometry and five pre-drawn pictures of it next to
the rest of the game art ([live.md](live.md#game-art)). The stats site's
tactical map plots players on the pictures; the 3D game viewer loads the
model. Everything comes from the owner's disc at run time and is served
loopback-only under `/art/game/` like the other art. Nothing derived from the
disc is in the repository: the tests build invented tag data.

Code: `crates/cli/src/live_geometry.rs` (extraction, mesh, glTF, files) and
`crates/cli/src/live_render.rs` (the software renderer). Tests:
`crates/cli/src/live_geometry_tests.rs`.

## Files

Per map, keyed by the map stem the art manifest already uses
(`map/<stem>/preview`):

```
map/<stem>/map.glb          glTF 2.0 binary: the level's render geometry
map/<stem>/views.json       the pre-drawn views and how to plot on them
map/<stem>/views/<id>.png   top, iso-ne, iso-nw, iso-sw, iso-se
map/<stem>/sky.glb          the level's sky model (when it has one)
```

`manifest.json` gains a `maps` object (absent when nothing was extracted):

```json
"maps": { "<stem>": { "geometry": "map/<stem>/map.glb",
                      "views": "map/<stem>/views.json",
                      "kind": "multiplayer", "bsps": 1 } }
```

`kind` is `"multiplayer"` or `"campaign"` (the map's cache type); `bsps` is
how many structure BSPs `map.glb` holds. Both are absent in manifests from
older builds. Paths are relative to the manifest. A map whose geometry could
not be read is listed under `skipped` as `map/<stem>/geometry` and has no
entry; a single BSP of a level that could not be read is listed as
`map/<stem>/bsp_<index>` and the level keeps its other BSPs.
`GEOMETRY_VERSION` is 1 (`views.json` is unchanged; the web viewer checks
it); the art's `EXTRACTOR_VERSION` (part of the cache
key) is 10, so a disc extracted by an older build is extracted again
(10 added the layers, the sky and `extras.name`).

### `map.glb`

- glTF is Y-up and right-handed. Game units are kept. A game position
  `(x, y, z)` (z up, as telemetry frames give it) is the glTF point
  `(x, z, -y)`; normals are converted the same way. There are no node
  transforms.
- A scenario with one structure BSP (every multiplayer map) has one node
  and one mesh, both named `structure`. A scenario with several (campaign
  levels) has a node and mesh per BSP, named `bsp_<index>` by the BSP's
  index in the scenario, with `extras.bsp` holding that index; all are root
  nodes of the scene, in index order, already in world space. A BSP only
  cinematics load also has `extras.cinematic: true` (pipeline step 2), so
  a viewer can hide it; the index is still the game's BSP index, as
  telemetry frames give it. `extras.name` is the BSP tag's file name, for
  a zone picker to show.
- **Backdrop.** A BSP's huge triangles (sea, sea floor and distant terrain
  planes; step 5) are not in its mesh but in a child node and mesh
  `backdrop_<index>` (`backdrop` on a single-BSP level) with
  `extras.backdrop: true`, same space and classes. A BSP without any
  has no such child. Every retail multiplayer map has none. Each mesh has
  one triangle primitive per surface class present: `POSITION`, `NORMAL`
  (both float VEC3) and u16 indices. A class with more than 65,536 vertices
  in one mesh is split into several primitives so every index fits in 16
  bits. No `COLOR_0` and no texture coordinates: the colour
  is the material's.
- Materials are named by class (one material per class, shared by every
  mesh): `floor`, `wall`, `metal`, `glass`, `water`, `foliage`, `default`. Metallic-roughness PBR with base colour factors only;
  `glass` and `water` are `alphaMode: BLEND`. Single-sided: the level's
  triangles face into the playable space, so back faces should be culled.
- **Machines.** When the scenario places machines (`mach` objects: doors,
  lifts, the turning wheel and the like), a root node `machines` (no mesh)
  follows the BSP nodes with a child per placement, named
  `<tag stem>_<placement index>`. A child's mesh is the machine's render
  model in model space (same axes conversion), shared by every placement
  of that machine and named after the first, and the child has a
  `matrix`: the placement's rotation, uniform scale and position in glTF
  axes. These are the only nodes with a transform. The child's `extras`
  repeat the placement in game coordinates as telemetry frames give a
  moving machine: `pos`, unit `forward` and `up`, and `scale`. The node
  names are stable: a frame's machine names the node it moves. Machines are all class
  `metal`, unless their shader names them glass, water or foliage. The
  model is drawn at rest (no animation): each region's first permutation
  at the highest detail level up to 3 of 5 it has.
- **Layers.** Decals and effect surfaces (pipeline steps 4 and 7) are in a
  BSP's child nodes `decals_<index>` (`extras.decals: true`) and
  `effects_<index>` (`extras.effects: true`), `decals` and `effects` on a
  single-BSP level; a BSP without any has no such child. These meshes have
  `TEXCOORD_0` and u32 indices, one primitive per material, and their
  materials (one per shader) carry their picture, embedded as a PNG (at
  most 256 pixels a side), and `extras.layer` (`decal` or `effect`) and
  `extras.blend`: `alpha` (over, by the picture's alpha), `multiply`
  (darkens by the picture), `double` (twice the picture times what is
  behind: mid-grey leaves it), `test` (cut out where alpha is under a
  half; `alphaMode: MASK`) or `additive` (added; no depth). Multiplying,
  doubling and additive materials are `KHR_materials_unlit`; effects are
  double-sided.
- **Sky.** `asset.extras.sky`, when the scenario has a sky: `name` (the
  sky tag's file name), `glb` (`"sky.glb"`, beside `map.glb`, when its
  model could be built), `clear` (the clear colour, RGB 0..1, when not
  black), `fog` (`colour`, `density` 0..1 reached at `end`, `start` and
  `end` in game units from the eye) and `scale` (the model's scale in the
  game). `sky.glb` is the sky's render model at rest in model space (same
  axes), textured, every region's first permutation at the highest detail
  under 30,000 triangles; its `asset.extras.blend` says per material, in
  order, `opaque`, `alpha` or `additive` (from the template's words:
  `add`; `alpha`, `cloud(s)`).
- `asset.generator` is `h2 live art`. `asset.extras` holds `generator`,
  `geometry_version`, `bounds` (`min`/`max`, game coordinates) and `frame`
  (the part of the level the views show, below). `bounds` covers the
  gameplay BSPs without their backdrops: what the views draw.
- **Features.** When a level has any, `asset.extras.features` lists its
  teleporters, their links and its lifts (players contract section 13;
  game coordinates):

  ```json
  "features": {
    "teleporters": [ { "id": "t0", "pos": [x, y, z], "facing": yaw,
                       "role": "two_way", "channel": 0 } ],
    "links": [ { "from": "t0", "to": "t1" } ],
    "lifts": [ { "id": "l0", "pos": [x, y, z], "dir": [x, y, z],
                 "length": 6.0, "radius": 3.0,
                 "source": "mach <stem> <index>: bounding sphere" } ]
  }
  ```

  Teleporters come from the scenario's netgame flags (`scnr+0x118`,
  0x20-byte entries: position, facing in radians at `+0xc`, type at
  `+0x10`, team at `+0x12`, identifier at `+0x14`, all i16 after the
  floats). Type 7 is a teleporter's entry and 8 its exit; flags within
  0.5 units are one pad, `two_way` when it has both. A pad's `channel` is
  its entry's identifier, else its exit's; `facing` is its exit's (the way
  an arriving player faces), else its entry's. A link goes from each pad
  with an entry to each other pad with an exit of the same identifier.
  Lifts (multiplayer maps only) are the placed scenery, machines, controls
  and crates (`scnr+0x50`, `+0xa8`, `+0xb8`, `+0x328`, each palette 8
  bytes on) whose tag's own name has the word `lift`, `grav` or `jumppad`.
  A lift stands on the middle of its render model's base, along the
  placement's up axis, as tall as the model and as wide as its wider side,
  when the model is at least 1 unit tall; else the `obje` bounding sphere
  (radius `+0x4`, offset `+0x8`) when that is; else a nominal 4 × 1
  column. `source` names the block, tag stem, placement index and which.
- Retail multiplayer maps come out at 11k to 31k triangles of level and
  0.3 to 1.1 MB (0 to 6 machines). Campaign levels (1 to 8 BSPs) come out
  at 38k to 412k triangles of level, 6 to 206 machines, and 1.2 to 15.9 MB;
  the level is not simplified. The largest (356k triangles and 538k
  vertices of level, 108 machines) is about the most a browser should be
  asked to fetch. Before `EXTRACTOR_VERSION` 6 indices were u32.

### `views.json`

```json
{ "geometry_version": 1, "map": "<stem>",
  "bounds": { "min": [x, y, z], "max": [x, y, z] },
  "frame":  { "min": [x, y, z], "max": [x, y, z] },
  "views": [
    { "id": "top", "kind": "ortho", "width": 2048, "height": 2048,
      "world_to_pixel": [[a, b, c, d], [e, f, g, h]],
      "file": "views/top.png", "up": "north", "cutaway": true,
      "elevation_deg": 90, "scale": 21.4, "forward": [0, 0, -1] },
    { "id": "iso-ne", "kind": "ortho", "width": 2048, "height": 1536,
      "world_to_pixel": [[...], [...]], "file": "views/iso-ne.png",
      "depth_sorted": true, "azimuth_deg": 45, "elevation_deg": 30,
      "scale": 18.2, "forward": [-0.61, -0.61, -0.5] }
  ] }
```

- `world_to_pixel` is exact (every view is orthographic):
  `px = M[0]·(x, y, z, 1)`, `py = M[1]·(x, y, z, 1)`, game coordinates,
  pixel origin at the top-left corner of the image (pixel `i` covers
  `[i, i + 1)`, so its centre is `i + 0.5`).
- `top` looks straight down; north (+y) is up and east (+x) is right.
  `cutaway: true`: roofs and ceilings over walkable space are cut away, so
  the picture shows the floors players stand on (pipeline step 8). The
  matrix is still exact; the cut only decides which surface each pixel
  shows.
- `iso-ne`, `iso-nw`, `iso-sw`, `iso-se` look from that compass direction
  (`azimuth_deg` from +x towards +y: 45, 135, 225, 315) at 30° elevation.
  `depth_sorted: true` means the picture has real occlusion: a plotted point
  may be behind a wall in it.
- Added fields: `file` (relative to `views.json`), `scale` (pixels per world
  unit), `forward` (unit view direction, game coordinates; a point's depth is
  `p·forward`, smaller is nearer), `elevation_deg`, `azimuth_deg`, and the
  top-level `frame`.

### Pictures

RGBA PNG with a transparent background, so the site can put them on its own
backdrop. Colours are our own calm palette (light warm grey floors, slate
walls, muted teal water and green foliage) lit by a fixed sun high in the
north-west (hillshade convention: shadows fall to the lower right of the top
view), the same for every view.

## Pipeline

1. **Maps.** The multiplayer and campaign maps, found by cache type, each
   opened with its shared maps (`live_art::Disc`).
2. **BSPs.** The scenario's structure BSP block (`scnr+0x210`, stride
   `0x44`, BSP tag at `+0x14`; docs/live-gamestate.md). Every BSP is used
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
   first), from the object type table (docs/objects.md). A placement record
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
   decoder ([structure-bsp.md](structure-bsp.md)): every cluster as stored and
   every instanced-geometry placement transformed (`translation + scale ×
   basis`). Only parts the engine draws (kinds other than 0 and 5). Vertex
   normals come from the tangent-frame stream, rotated for placements. A
   section that fails to decode is counted and skipped.
4. **Classes.** The BSP material block (`sbsp+0xa4`) is read with the
   render-model material layout: 0x20-byte elements, shader tag ID at `+0xc`
   ([map-format.md](map-format.md), `ModeMaterial`). It holds on every retail
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
   colour at +0xa0. A sky that cannot be read is listed under `skipped`
   as `map/<stem>/sky`.
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
9. **Write.** `map.glb` as soon as a map is built, then every (map, view)
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
- No textures or lightmaps, and no object geometry but machines
  (scenery, crates and vehicles are not drawn). Machines are at rest: a
  door is drawn as its model's default pose has it, and the wheel does not
  turn.
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
- A level with several skies (one per zone) gets only the first with a
  model. Additive sky layers (haze, glows) are left out by the viewer:
  without the shader's constants they wash the sky out.

# Player and weapon models

The same extraction writes a posed model per player species and campaign
character, a model per multiplayer weapon and a model per vehicle, for the
3D viewer's players and AI (the players contract, sections 2 and 5). Like the maps, everything comes from the owner's disc at run
time; the tests build invented models.

Code: `crates/cli/src/live_models.rs`. Tests:
`crates/cli/src/live_models_tests.rs`.

## Files

```
models/biped/spartan.glb
models/biped/elite.glb
models/biped/<species>.glb    one per campaign character (grunt, jackal, ...)
models/weapon/<stem>.glb      one per multiplayer weapon
models/vehicle/<stem>.glb     one per vehicle tag
```

`manifest.json` gains a `models` object (absent when nothing was
extracted):

```json
"models": { "version": 2,
            "bipeds":  { "spartan": { "glb": "models/biped/spartan.glb",
                                      "clips": ["rifle:idle", "rifle:run", ...] },
                         "elite":   { "glb": "models/biped/elite.glb", "clips": [...] } },
            "weapons": { "battle_rifle": { "glb": "models/weapon/battle_rifle.glb",
                                           "class": "rifle",
                                           "ballistics": { "kind": "bullet", "speed": 400,
                                                           "final_speed": 400, "rate": 6.5 } } },
            "vehicles": { "warthog": { "glb": "models/vehicle/warthog.glb" } } }
```

`vehicles` is absent when there are none. `clips` (a biped's animations,
in the glb's order) is absent when it has none; `class` (the weapon class a
weapon is held with, picking the clip set) is absent when the tag names
none. `ballistics` (what the weapon fires: `kind` one of `bullet`,
`plasma`, `needle`, `rocket`, `grenade`, `beam`, `melee`; `speed` and
`final_speed` in world units a second; `rate` sustained rounds a second;
each absent when unknown) comes from the weapon's barrel and its projectile
tag (`crates/cli/src/live_weapons.rs` has the offsets and how they were
established) and drives the viewer's tracers, bolts and rockets. Version 1
had none of these and unskinned bipeds; a version 1 reader that skips the
version check still draws the version 2 files (below).

Art from an older extractor (the manifest's `extractor` below the running
`h2`'s) is stale: before version 2 its bipeds had no clips, so the viewer's
players couldn't animate. `h2 live serve` prints a warning naming the
command to extract again, `h2 live assets check [--out DIR]` exits 1 for
missing or stale art (deploy/live/setup.sh extracts again then), `h2 play
--live` extracts again by itself (the extractor version is part of the
cache key), and the viewer says so when the models it is given can't
animate. The server sends the art with `Cache-Control: no-cache` and the
viewer asks for each model with the manifest's `source` as a query, so a
browser never keeps showing models from before an extraction.

A model that could not be built is listed under `skipped` as
`models/biped/<species>`, `models/weapon/<stem>` or
`models/vehicle/<stem>` and has no entry. Only keys matching
`[a-z0-9_]{1,32}` are written.

Keys (what telemetry sends):

- **Weapons and vehicles:** the tag's file name, lower case
  (`battle_rifle`, `energy_blade`, `brute_shot`, `flag`, ...; `warthog`,
  `ghost`, `banshee`, `scorpion`, `wraith`, `spectre`, `phantom`,
  `pelican`, and the turrets as their own tags: `chaingun`, `gauss`,
  `c_turret_ap`, ...).
- **Player bipeds:** `spartan` and `elite`.
- **Campaign characters:** the name of the biped tag's directory, lower
  case: `objects\characters\grunt\grunt` is `grunt`,
  `...\marine\marine_female\marine_female` is `marine_female`,
  `...\prophet\prophet_minor` is `prophet`. Where several biped tags share
  a directory, the one named like the directory is drawn, else the one on
  the most campaign maps, else the shortest name. The retail directories are
  not always the common names: drones are `bugger`, combat forms are
  `floodcombat_human` and `floodcombat_elite`, Tartarus is `tartarus`. The
  campaign `elite` directory is the player key `elite` and keeps the
  multiplayer model; the campaign chief is `masterchief`.

### glb

- Axes and units as `map.glb`: game `(x, y, z)` is glTF `(x, z, -y)`, game
  units kept.
- Nodes: `root`, whose children are the mesh node (`body` for a biped,
  `weapon` for a weapon) and empty nodes with `translation` and `rotation`
  only (on a biped, the skeleton's root joints too). Bipeds have `weapon`
  (the right-hand marker: a weapon glb placed under it sits in the hand)
  and `head` (the head marker), each the child of the joint it rides on
  (`joint_r_hand`, `joint_head`, ...; the root's child when it has none); weapons have
  `muzzle` (the `primary_trigger` marker, or the front of the model at mid
  height when it has none); vehicles (mesh node `vehicle`) have a
  `seat_<name>` node per seat marker (`seat_driver`, `seat_gunner`,
  `seat_passenger`, `seat_passenger_lf`, ...) where the model has them.
  Creatures without the usual markers still get both biped nodes: `weapon`
  at a node named with `hand` (right first), else the root; `head` at the
  top centre of the mesh.
- One primitive per material: `POSITION`, `NORMAL`, `TEXCOORD_0` (float),
  u32 indices. Biped and vehicle primitives also have `COLOR_0` (float
  `VEC4`) when any of the model's materials has a change-colour map: red is
  the primary change-colour weight, green the secondary (0..1), sampled
  bilinearly from the material's change-colour map at the vertex. A
  material without a change-colour map has zero weights; weapons have no
  `COLOR_0`.
- Materials (`material_look`): `baseColorTexture` (an embedded PNG, at most
  512 pixels a side) when the diffuse map decoded, else a grey
  `baseColorFactor`; metallic 0, roughness 0.8 unless:
  - the shader is a bump-env or shiny one (`tex_bump_env*`,
    `tex_bump_shiny`): the base map's alpha is Halo 2's specular mask, and
    becomes a `metallicRoughnessTexture` (green roughness 0.85 to 0.3,
    blue metalness up to 0.6 for env, 0.35 for shiny); the base map is
    then stored opaque;
  - it has a self-illumination map (a bitmap named `*illum*`), or is a lit
    display (`illum`, `*meter*`, `*plasma*` templates): `emissiveTexture`
    (that map, or the base map) with `emissiveFactor` 1;
  - it is transparent (`shader_templates\transparent\...`): `*_add_*`
    templates are glows, `alphaMode` `BLEND`, black base, the base map as
    the emission and `extras.h2.blend` `"add"` (the viewer adds them);
    the others are glass, `BLEND` with a see-through blue-grey factor and
    no texture; `overlay` templates (decals) blend by their alpha;
  - it is alpha-tested (`*alpha_test*`): `alphaMode` `MASK` at 0.5, its
    alpha kept.
  The change-colour map (at most 256 pixels a side) is also stored, its
  texture index in `extras.h2.change_colour`, for readers that tint per
  pixel (the viewer does); `extras.h2.template` names the shader template.
  Each distinct picture is stored once. Bump maps (8-bit palettized
  normals; the palette is the engine's, not the tag's) and detail maps are
  not used.
- `asset.extras`: `generator`, `models_version` (2) and, for bipeds,
  `height` (game units, feet to the top of the head).
- A biped stands with its feet on the origin plane (lowest vertex at
  glTF y = 0) and faces glTF +x (game +x, yaw 0). It holds a rifle in its
  idle stance (or its own idle, below), and that posed shape is what the
  vertices store, so a reader that ignores the skin draws it as it was in
  version 1.
- Bipeds are skinned (weapons and vehicles are not). One node per
  render-model node, named `joint_<node>` (`joint_pelvis`, `joint_r_hand`,
  ...), in the model's tree under `root`, their `translation` and
  `rotation` the default (idle) pose. One skin over all of them; its
  inverse bind matrices are the inverse of each joint's default frame, so
  the default pose leaves every vertex where it is stored. Primitives add
  `JOINTS_0` (u16 `VEC4`) and `WEIGHTS_0` (float `VEC4`, summing to 1; at
  most four nodes per vertex, as the game's).
- Biped animations, one per clip, named `<weapon_class>:<state>`: classes
  `rifle`, `pistol`, `missile`, `support`, `sword`, `unarmed`, `flag`,
  `ball` (what multiplayer weapons are held with; `dual`, vehicle seats and
  AI-only classes are left out), states `idle`, `walk`, `run`, `crouch`,
  `airborne`, `land`, `grenade`, `melee`, `reload`, `fire`, `switch`,
  `die`, each only where the graph has it. Every joint has a `rotation`
  channel (normalised i16 `VEC4`, the core glTF option) and a
  `translation` channel (float `VEC3`), `LINEAR`, one key per frame at
  30 fps; a channel that never changes in the clip has a single key at
  time 0. The clip keeps the floor of the default pose (no root motion: the
  game moves the biped, not the animation).
- A vehicle is in its neutral pose (below), origin at the vehicle's object
  origin, facing glTF +x. `asset.extras.pose` names the frame used
  (`combat:any:any:closing, last frame`, or `rest pose`).

On the retail disc: spartan 3,042 triangles, 80 clips and 2.6 MB (0.8 MB
of it mesh and textures), elite 2,744 triangles, 58 clips and 2.8 MB
(`ball`, `flag` and `unarmed` dropped for size; the dervish and heretics
likewise), 22 weapons of 430 to 1,400 triangles and 60 to
380 KB; 34 campaign characters of 330 to 4,800 triangles, 0 to 56 clips
and 0.2 to 2.8 MB; 38 vehicles of 26 to 4,750 triangles and 26 KB to 1.7 MB. The
whole step takes about 0.6 s on 10 cores.

## Pipeline

1. **Bipeds.** On the multiplayer maps, the first `bipd` tag whose file name
   is `masterchief_mp` (else `masterchief`) is the spartan, `elite_mp` (else
   `elite`) the elite. Its object model (`obje +0x34`, docs/objects.md) gives
   the render model (`hlmt +0x00`) and the animation graph (`hlmt +0x10`).
   Campaign characters: every `bipd` tag on the campaign maps, one per
   species key (above), built in parallel.
2. **Vehicles.** Every `vehi` tag of the shared, multiplayer and campaign
   maps, first one per stem. Posed like a biped from its animation graph:
   the last frame of `closing` (a vehicle's rest pose has its canopy or
   hatch open: a banshee's whole cab is tilted up), else the first frame of
   `opening`, else an `idle` (turrets, the enforcer); else the rest pose
   (the warthog, ghost, phantom and most turrets have no actions). Per region the permutation named `base` (else
   the first), the highest detail level under 40,000 triangles. Vehicle tags
   with fewer than 8 drawn triangles are placeholders and are skipped
   (two on the retail disc).
3. **Weapons.** The `weap` tags under `objects\weapons\` of the shared map
   and the multiplayer maps, except fixed emplacements (`\fixed\`), first
   one per stem. Vehicle guns live elsewhere and are not included.
4. **Geometry.** `tags::model::RenderModelGeometry`. Per region, the
   permutation named `base` (else the first); for bipeds the highest detail
   level whose drawn triangles fit 15,000 (the retail bipeds fit at their
   highest), for weapons the highest. Section-local node indices go through
   the section's node map. Parts of materials whose shader template name
   says `emblem` (the emblem decal) are left out.
5. **Skeleton.** Each render-model node's rest transform (translation at
   `+0xc`, quaternion at `+0x18`, relative to its parent) composed up the
   tree. The disc test checks the result against the inverse rest frames the
   tag stores (`+0x2c`: three axes and a position): they agree to 4e-7 on
   the spartan, which fixes the quaternion convention.
6. **Pose.** In the biped's graph: mode `combat`, class `rifle` (else
   `pistol`, `unarmed`), type `any`, action `idle`; else any `idle` action
   (the combat mode first); else any action with `idle` in its name; else
   the rest pose. An inherited action is read from its graph. Frame 0 is sampled (`tags::anim_sample`). Graph
   nodes are matched to model nodes by name; a channel the frame does not
   supply keeps the rest value. Sections whose vertices carry no node
   indices are rigid pieces of their section's first mapped node (a
   banshee's cab, a jackal's shield, a helmet), not of the root.
7. **Bake.** Linear blend skinning: each vertex is moved by the weighted sum
   of `posed_world × inverse(rest_world)` of its nodes (weights
   renormalised), normals likewise. Then the model is lifted so its lowest
   vertex is at height 0. The same weights become `JOINTS_0`/`WEIGHTS_0`,
   and the posed frames (lifted) the joints' defaults and bind matrices.
8. **Markers.** Render-model header `+0x58` is a block of marker groups
   (0xc bytes: string-ID name, block of markers); a marker is 0x24 bytes,
   node index at `+2`, translation `+4`, quaternion `+0x10`, relative to its
   node. On the retail models every name resolves and every node is in
   range. `weapon` is the posed `right_hand` marker (`right_hand_elite` on
   the elite; else a node named `r_hand`), `head` the `head` marker. A
   vehicle marker is a seat when its first word is `driver`, `gunner`,
   `passenger`, `seat`, `pilot` or `rider` and no word says it is something
   at the seat (`enter`, `camera`, `target`, `hand`, `foot`, `damage`, ...);
   the node is `seat_` plus the marker's words joined by `_`, first marker
   of a name. Phantoms, Pelicans and some turrets have no seat markers.
9. **Clips.** In the biped's graph and the graphs it inherits from (its
   own first). The locomotion states are actions of type `any`: `idle`,
   `walk_front` (walk), `move_front` (run), `airborne`, `land_soft`
   (land) in mode `combat`, `idle` in mode `crouch` (crouch), and
   `landing_dead` of class `any` (die, the same clip for every class).
   These go through the action table, so `unarmed` plays what the graph
   maps it to (the spartan's sword animations). The weapon states are
   per weapon type in the game (their table is not decoded), so they are
   found by animation name, `combat:<class>[:<type>]:<action>[:varN]`:
   `throw_grenade` (grenade), `melee_strike_1` or `melee` (melee),
   `reload_1` (reload), `fire_1` (fire), `ready` (switch). The class's own
   animation comes first, then one of the types `br`, `hp`, `rl`, `bs`
   (battle rifle, magnum, rocket launcher, brute shot), then the first type
   by name. Aim grids (codec 8) are skipped. Every frame is sampled. A
   channel a frame lacks (an upper-body replacement's legs) keeps the
   default pose's value. An overlay (`fire_1`) adds its change since its
   first frame to the default pose: rotation `default × first⁻¹ × frame`,
   translation `default + frame − first`. Scale channels are dropped. A
   biped glb over 3.25 MiB loses whole classes, `ball`, then `flag`, then
   `unarmed`, until it fits (the elite keeps its sword clips).
10. **Weapon class.** The string ID at `weap +0x288` (D: every multiplayer
    weapon names a class of the player graphs there: `rifle` for the
    battle rifle, carbine, shotgun, SMG and the snipers, `pistol` for the
    magnum, needler and plasma pistol, `missile` for the rocket launcher and
    fuel rod, `support` for the brute shot, `sword`, `flag`, `ball`; vehicle
    guns say `fixed`).
11. **Textures.** The shader's bitmap parameters (`shad +0x20` postprocess,
   `+4` block, datum first; docs/shader-instances.md). A bitmap's role comes
   from generic words in its tag name read at run time: engine defaults
   (`default_bitmaps`, `rasterizer`) are never used; a name ending in
   `_cc` or saying `change_color` (or `color_change`) is the change-colour
   map; one saying `illum` the self-illumination map; the first name
   that is not a bump, detail, reflection, cube, mask or engine map is the
   diffuse map. Decoded with `assets::decode_image` (DXT and the other
   formats the art already decodes; the largest stored level). The look
   follows from the template's name (above).

## Checking against your disc

```sh
cargo test --release -p cli --bin h2 models_from_the_disc -- --ignored --nocapture
```

writes every model, `report.txt` (sources, pose, triangle counts, sizes),
`models.json`, `seats.txt` (each vehicle's seat nodes),
`check-<species>.png`, `check-vehicle-<stem>.png`, `animation.txt` (each
biped's size and clips, each weapon's class, and the checks below) and
`strip-spartan.png` (six frames each of `rifle:run`, `rifle:grenade` and
`rifle:melee`, skinned from the glb as a viewer would) to
`$RUSH_KEEP/players-models` (a directory outside the repository). Each
picture shows the model from the front, the right side and three-quarter
with its diffuse texture, then the change-colour weights (red primary, blue
secondary over grey). Bipeds hold the battle rifle at their `weapon` node;
vehicles have the spartan standing at `seat_driver`. It also checks the
spartan's file: the `gltf` crate reads its skin and clips, playing
`rifle:idle` frame 0 moves no vertex (the default pose is that frame), and
every `rifle:run` key at frames 0, 7, 13 and the last, back in game axes,
is the rotation `tags::anim_sample` gives the node (to 6e-8). It checks
true scale (the spartan 0.6 to 0.8 units tall, the elite taller, the
Warthog 1.5 to 3 long), the looks (change-colour maps on the spartan, elite
and banshee, the elite's glow, the Warthog's glass, the spartan's shine),
that the elite keeps its sword clips, and the battle rifle's, plasma rifle's
and rocket launcher's ballistics. `shaders_from_the_disc` (same flags)
writes `shaders.txt`: every player biped's and vehicle's materials with
their template, bitmaps (name, format, size, role) and instance colours and
vectors; `live_weapons::tests::weapons_from_the_disc` writes `weapons.txt`.
Do not commit anything they write.

## Limits

- The default pose is the rifle idle, whatever the player holds; a viewer
  that does not play the class's clips shows a pistol or sword in a rifle
  grip.
- Clips are whole-body and one at a time: the game layers aim, overlays
  and upper-body replacements over locomotion, a viewer playing these
  clips does not. `fire` is the overlay applied over the default pose;
  `reload`, `melee` and `fire` are one representative weapon type's for the
  whole class (the battle rifle's for `rifle`, and so on), and `die` is
  the graph's `landing_dead` (multiplayer deaths are ragdolls in the
  game). No turning, strafing or backwards clips, no aim. Characters without a rifle stance
  use their own idle (grunts the pistol one, hunters and flood forms the
  unarmed one, prophets and sentinels a combat idle); Lord Hood has no idle
  and stands in his rest pose.
- Vehicles are posed once: no turret aim, no wheel suspension. A vehicle
  turret is its own tag and model (`cannon` on the scorpion, `mortar` on the
  wraith, `chaingun`/`gauss` on the warthog), not part of the vehicle. A
  vehicle with no seat markers has no `seat_` nodes, and a seat node is the
  marker's frame, not a seated pose.
- `COLOR_0` change-colour weights are per vertex, so their edges are as
  coarse as the mesh; the per-pixel map in `extras.h2.change_colour` is
  exact. Only the primary and secondary colours are tinted.
- No bump or detail maps; reflections are the reader's environment, not
  the shader's cube map, and the shader's colour parameters (specular and
  environment tints) are not read, so the chief's visor is the diffuse
  map's dark colour, not gold.
- Elite helmets and other permutations: only `base` (or the first).
