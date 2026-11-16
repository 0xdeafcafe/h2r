# Map geometry and pre-drawn views

`h2 live assets extract` (and `h2 play --live`, once per disc) writes each
multiplayer map's level geometry and five pre-drawn pictures of it next to
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
```

`manifest.json` gains a `maps` object (absent when nothing was extracted):

```json
"maps": { "<stem>": { "geometry": "map/<stem>/map.glb",
                      "views": "map/<stem>/views.json" } }
```

Paths are relative to the manifest. A map whose geometry could not be read
is listed under `skipped` as `map/<stem>/geometry` and has no entry.
`GEOMETRY_VERSION` is 1; the art's `EXTRACTOR_VERSION` (part of the cache
key) is 5, so a disc extracted by an older build is extracted again.

### `map.glb`

- glTF is Y-up and right-handed. Game units are kept. A game position
  `(x, y, z)` (z up, as telemetry frames give it) is the glTF point
  `(x, z, -y)`; normals are converted the same way. There are no node
  transforms.
- One node and one mesh, both named `structure`, with one triangle
  primitive per surface class present: `POSITION`, `NORMAL` (both float
  VEC3) and u32 indices. No `COLOR_0` and no texture coordinates: the colour
  is the material's.
- Materials are named by class: `floor`, `wall`, `metal`, `glass`, `water`,
  `foliage`, `default`. Metallic-roughness PBR with base colour factors only;
  `glass` and `water` are `alphaMode: BLEND`. Single-sided: the level's
  triangles face into the playable space, so back faces should be culled.
- `asset.generator` is `h2 live art`. `asset.extras` holds `generator`,
  `geometry_version`, `bounds` (`min`/`max`, game coordinates) and `frame`
  (the part of the level the views show, below).
- Retail multiplayer maps come out at 11k to 31k triangles and 0.4 to
  1.3 MB.

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

1. **Maps.** The multiplayer maps, found by cache type, each opened with its
   shared maps (`live_art::Disc`).
2. **BSPs.** The scenario's structure BSP block (`scnr+0x210`, stride
   `0x44`, BSP tag at `+0x14`; docs/live-gamestate.md). Every BSP is used.
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
   The last path component of the shader's tag name is split into words and
   matched against generic English words only (nothing from the game is
   named in the code): `sky`, `cloud(s)`, `fog`, `mist`, `haze` mark surfaces
   that are not level and are dropped; `glass`; `water`, `waterfall`, `pool`
   ...; plant words; `metal…`, `steel…`, `rust…`, `grate…` .... A triangle
   facing up (normal z ≥ 0.6) is `floor` unless the name says water, foliage
   or glass; other named classes come next; then steep faces are `wall` and
   overhangs and ceilings `default`.
5. **Mesh.** Drop non-finite, degenerate (area < 5e-7) and repeated
   triangles; replace a vertex normal that is missing or points against its
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
   already invisible from above and need no cut. Cut regions are then
   opened (eroded and dilated by a 0.8-unit square), so regions narrower
   than about 1.6 units, such as bridges and walkways over open ground,
   keep their top. Walls have no ceiling inside them, so they keep their
   tops and read as rims around opened rooms. The iso views and `map.glb`
   are not cut.
9. **Write.** `map.glb` as soon as a map is built, then every (map, view)
   picture in parallel, then `views.json`; the manifest is written last by
   the art extractor.

Maps load on all CPUs, then all 60 pictures are drawn on all CPUs. On a
10-core machine the whole geometry step takes 2 to 3 s for the 12 retail
multiplayer maps (about 0.1 s to load each map; most of the rest is
drawing).

## Checking against your disc

```sh
cargo test --release -p cli geometry_from_the_disc -- --ignored --nocapture
```

writes the outputs, a timing report, the shader names behind the classes,
a contact sheet of every view and, under `cutaway/`, each top view with and
without the cutaway side by side to a directory outside the repository (the
path is `CHECK_DIR` in the test). Do not commit anything it writes.

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
- No textures, lightmaps or object geometry (scenery, crates, vehicles):
  only the BSP.

# Player and weapon models

The same extraction writes a posed player model per species and a model per
multiplayer weapon, for the 3D viewer's players (the players contract,
section 2). Like the maps, everything comes from the owner's disc at run
time; the tests build invented models.

Code: `crates/cli/src/live_models.rs`. Tests:
`crates/cli/src/live_models_tests.rs`.

## Files

```
models/biped/spartan.glb
models/biped/elite.glb
models/weapon/<stem>.glb      one per multiplayer weapon
```

`manifest.json` gains a `models` object (absent when nothing was
extracted):

```json
"models": { "version": 1,
            "bipeds":  { "spartan": { "glb": "models/biped/spartan.glb" },
                         "elite":   { "glb": "models/biped/elite.glb" } },
            "weapons": { "battle_rifle": { "glb": "models/weapon/battle_rifle.glb" } } }
```

A model that could not be built is listed under `skipped` as
`models/biped/<species>` or `models/weapon/<stem>` and has no entry.
Weapon stems are the weapon tag's file name, lower case (`battle_rifle`,
`energy_blade`, `brute_shot`, `flag`, `ball`, `assault_bomb`, ...), and only
stems matching `[a-z0-9_]{1,32}` are written.

### glb

- Axes and units as `map.glb`: game `(x, y, z)` is glTF `(x, z, -y)`, game
  units kept.
- Nodes: `root`, whose children are the mesh node (`body` for a biped,
  `weapon` for a weapon) and empty nodes with `translation` and `rotation`
  only. Bipeds have `weapon` (the right-hand marker: a weapon glb placed
  under it sits in the hand) and `head` (the head marker); weapons have
  `muzzle` (the `primary_trigger` marker, or the front of the model at mid
  height when it has none).
- One primitive per material: `POSITION`, `NORMAL`, `TEXCOORD_0` (float),
  u32 indices. Biped primitives also have `COLOR_0` (float `VEC4`): red is
  the primary change-colour weight, green the secondary (0..1), sampled
  bilinearly from the material's change-colour map at the vertex. A
  material without a change-colour map has zero weights; weapons have no
  `COLOR_0`.
- Materials: `baseColorTexture` (an embedded PNG, at most 512 pixels a side,
  shared by materials with the same map) when the diffuse map decoded, else
  a grey `baseColorFactor`. Metallic 0, roughness 0.8.
- `asset.extras`: `generator`, `models_version` (1) and, for bipeds,
  `height` (game units, feet to the top of the head).
- A biped stands with its feet on the origin plane (lowest vertex at
  glTF y = 0) and faces glTF +x (game +x, yaw 0). It holds a rifle in its
  idle stance: the posed shape is baked into the vertices; nothing is skinned
  or animated.

On the retail disc: spartan 3,042 triangles and 0.8 MB, elite 2,744
triangles and 0.8 MB, 22 weapons of 430 to 1,400 triangles and 60 to
380 KB. The whole step takes about 0.1 s.

## Pipeline

1. **Bipeds.** On the multiplayer maps, the first `bipd` tag whose file name
   is `masterchief_mp` (else `masterchief`) is the spartan, `elite_mp` (else
   `elite`) the elite. Its object model (`obje +0x34`, docs/objects.md) gives
   the render model (`hlmt +0x00`) and the animation graph (`hlmt +0x10`).
2. **Weapons.** The `weap` tags under `objects\weapons\` of the shared map
   and the multiplayer maps, except fixed emplacements (`\fixed\`), first
   one per stem. Vehicle guns live elsewhere and are not included.
3. **Geometry.** `tags::model::RenderModelGeometry`. Per region, the
   permutation named `base` (else the first); for bipeds the highest detail
   level whose drawn triangles fit 15,000 (the retail bipeds fit at their
   highest), for weapons the highest. Section-local node indices go through
   the section's node map. Parts of materials whose shader template name
   says `emblem` (the emblem decal) are left out.
4. **Skeleton.** Each render-model node's rest transform (translation at
   `+0xc`, quaternion at `+0x18`, relative to its parent) composed up the
   tree. The disc test checks the result against the inverse rest frames the
   tag stores (`+0x2c`: three axes and a position): they agree to 4e-7 on
   the spartan, which fixes the quaternion convention.
5. **Pose.** In the biped's graph: mode `combat`, class `rifle` (else
   `pistol`, `unarmed`), type `any`, action `idle`; an inherited action is
   read from its graph. Frame 0 is sampled (`tags::anim_sample`). Graph
   nodes are matched to model nodes by name; a channel the frame does not
   supply keeps the rest value.
6. **Bake.** Linear blend skinning: each vertex is moved by the weighted sum
   of `posed_world × inverse(rest_world)` of its nodes (weights
   renormalised), normals likewise. Then the model is lifted so its lowest
   vertex is at height 0.
7. **Markers.** Render-model header `+0x58` is a block of marker groups
   (0xc bytes: string-ID name, block of markers); a marker is 0x24 bytes,
   node index at `+2`, translation `+4`, quaternion `+0x10`, relative to its
   node. On the retail models every name resolves and every node is in
   range. `weapon` is the posed `right_hand` marker (`right_hand_elite` on
   the elite; else a node named `r_hand`), `head` the `head` marker.
8. **Textures.** The shader's bitmap parameters (`shad +0x20` postprocess,
   `+4` block, datum first; docs/shader-instances.md). A bitmap's role comes
   from generic words in its tag name read at run time: a name ending in
   `_cc` or saying `change_color` is the change-colour map; the first name
   that is not a bump, detail, reflection, cube, mask or engine map is the
   diffuse map. Decoded with `assets::decode_image` (DXT and the other
   formats the art already decodes).

## Checking against your disc

```sh
cargo test --release -p cli --bin h2 models_from_the_disc -- --ignored --nocapture
```

writes every model, `report.txt` (sources, pose, triangle counts, sizes),
`models.json` and `check-<species>.png` to `$RUSH_KEEP/players-models` (a
directory outside the repository). Each picture shows the biped from the
front, the right side and three-quarter with its diffuse texture, then the
change-colour weights (red primary, blue secondary over grey), holding the
battle rifle at its `weapon` node. Do not commit anything it writes.

## Limits

- One pose for everyone: the rifle idle, whatever the player holds. A
  pistol or sword sits in a rifle grip.
- Change-colour weights are per vertex, so their edges are as coarse as the
  mesh. The diffuse texture already carries the fine detail.
- No bump, specular, reflection or self-illumination: base colour only. The
  visor is the diffuse map's colour.
- Elite helmets and other permutations: only `base` (or the first).
