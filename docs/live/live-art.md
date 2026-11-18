# Map geometry and pre-drawn views

`h2 live assets extract` (and `h2 play --live-local`, once per disc) writes each
multiplayer and campaign level's geometry and five pre-drawn pictures of it next to
the rest of the game art ([live.md](live.md#game-art)). The stats site's
tactical map plots players on the pictures; the 3D game viewer loads the
model. Everything comes from the owner's disc at run time and is served
under `/art/game/` like the other art, to this machine only unless the
server runs with `--share-game-art`. Nothing derived from the
disc is in the repository: the tests build invented tag data.

Code: `crates/cli/src/live_geometry.rs` (extraction, mesh, glTF, files),
`crates/cli/src/live_looks.rs` (surface pictures),
`crates/cli/src/live_basemap.rs` (which bitmap a shader draws as its base),
`crates/cli/src/live_mappack.rs` (map pack maps),
`crates/cli/src/live_scenario.rs` (placed objects),
`crates/cli/src/live_layers.rs` (decals and effects), `live_sky.rs` (the sky),
`live_weather.rs` (weather volumes), `live_models.rs` (player, weapon and
vehicle models), `live_weapons.rs` (weapon ballistics), `live_features.rs`
(teleporters and lifts), `live_catalog.rs` (the playlist builder's catalog) and
`crates/cli/src/live_render.rs` (the software renderer). Tests:
`crates/cli/src/live_geometry_tests.rs` and the modules' own.

## Files

Per map, keyed by the map stem the art manifest already uses
(`map/<stem>/preview`):

```
map/<stem>/map.glb          glTF 2.0 binary: the level's render geometry (meshopt)
map/<stem>/map.glb.br       the same, Brotli (and .gz, gzip): see Packing
map/<stem>/tex/<id>.webp    the surface pictures map.glb names
map/<stem>/views.json       the pre-drawn views and how to plot on them
map/<stem>/views/<id>.webp  top, iso-ne, iso-nw, iso-sw, iso-se
map/<stem>/sky.glb          the level's sky model (when it has one)
```

Every glb and JSON file has `.br` and `.gz` copies beside it
([Packing](#packing)).

`manifest.json` gains a `maps` object (absent when nothing was extracted):

```json
"maps": { "<stem>": { "geometry": "map/<stem>/map.glb",
                      "views": "map/<stem>/views.json",
                      "kind": "multiplayer", "bsps": 1 } }
```

`kind` is `"multiplayer"` or `"campaign"` (the map's cache type); `bsps` is
how many structure BSPs `map.glb` holds. Both are absent in manifests from
older builds. A converted Halo 1 map's entry also has `"game": "halo1"` and
`"name"`, its display name ([Halo 1 maps](#halo-1-maps)); Halo 2's own maps
have neither. Paths are relative to the manifest. A map whose geometry could
not be read is listed under `skipped` as `map/<stem>/geometry` and has no
entry; a single BSP of a level that could not be read is listed as
`map/<stem>/bsp_<index>` and the level keeps its other BSPs.
`GEOMETRY_VERSION` is 1 (`views.json` is unchanged; the web viewer checks
it); the art's `EXTRACTOR_VERSION` (part of the cache
key) is 22, so a disc extracted by an older build is extracted again
(10 added the layers, the sky and `extras.name`; 12 the surface
pictures, texture coordinates and the `scenario` node; 13 the base map
from the shader's passes, pictures as files beside `map.glb`, the gzip
copy and the map pack maps; 15 vehicles in their own colours, the
reflection tints and shaders whose compiled data another shader shares;
16 vehicles' turrets and hull decals that double what is under them;
17 the weather volumes and the sky light's glow; 18 turrets as parts that aim;
19 the title and save images, `title/image` and `title/save-image`;
20 solid shaders whose template name only contains "overlay" keep their
textures; 21 packing (Brotli and gzip copies, meshopt where it pays), WebP
pictures and views, surface pictures at the disc's full size; 22 the
bipeds' strafing, backwards and crouched movement clips, the vehicle seat
poses `any:seat_<mode>`, the models' `seats` table and `asset.extras.lift`).

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
  one triangle primitive per surface class and look present: `POSITION`,
  `NORMAL` (both float VEC3), u16 indices and, when textured,
  `TEXCOORD_0` (float VEC2, as the game stores them: they tile). A
  primitive with more than 65,536 vertices is split into several so every
  index fits in 16 bits. No `COLOR_0`.
- Class materials are named by class (one material per class, shared by
  every mesh): `floor`, `wall`, `metal`, `glass`, `water`, `foliage`,
  `default`. Metallic-roughness PBR with base colour factors only; `glass`
  and `water` are `alphaMode: BLEND`. Single-sided: the level's triangles
  face into the playable space, so back faces should be culled.
- **Textures.** A surface whose shader has a base map that decodes gets
  its shader's *look*: a material named after the shader (its last path
  component), `extras.look: true`, `baseColorTexture` (sRGB, the shared
  sampler: linear, mipmapped, repeating), metallic 0, roughness 0.85;
  cut-out shaders (`alpha_test` templates) are `alphaMode: MASK` at 0.5 and
  double-sided. The primitive then has `TEXCOORD_0` and `extras`
  `{"class": "<class>", "flat": <class material index>}`, so a reader can
  draw it untextured with the class material instead (the viewer's
  Textures switch). A class material is written even when only `flat`
  names it. Glass and water keep their class material and are not
  textured. When the shader tiles its base map (pipeline step 10), the
  `baseColorTexture` has `KHR_texture_transform` with that `scale`
  (listed in `extensionsUsed`, not required: a reader without it draws
  the picture once per stored repeat). Each picture is stored once per
  bitmap however many shaders use it, as its own file beside `map.glb`:
  the image has a `uri` (`tex/<bitmap tag id, 8 hex digits>.webp`, `m`
  after the id for a cut-out's alpha-kept copy), a `mimeType` and
  `extras` `width` and `height`, and no buffer view, so the geometry loads
  without them and each picture caches on its own (`tex/` is rewritten
  with `map.glb`). WebP (lossy, quality 90; a cut-out's alpha kept
  exactly, [Packing](#packing)) at the bitmap's full stored size (the
  discs' largest are 1024 pixels a side; the cap is 2048), every picture of
  a level halved until they total at most 32 MiB (no retail level needs
  it). Machines and placed objects get looks the
  same way. Detail maps, bump maps, lightmaps and the shaders' colour
  parameters are not used.
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
- **Scenario objects.** When the scenario places any other objects, a
  root node `scenario` (no mesh) follows `machines`, with a child per
  placement named `<kind>_<placement index>`: a mesh shared by every
  placement of the same object (named after the first) and a `matrix`,
  as machines have them. `extras`: `kind` (the block: `scen` scenery,
  `bloc` crates, `vehi` vehicles, `weap` weapons, `eqip` equipment, `ctrl`
  controls, `lifi` light fixtures, `ssce` sound scenery; or `netgame` for
  a multiplayer spawn), `index` (the placement's index in its kind's
  block), `tag` (the object tag's file name), `pos`, `forward`, `up` and
  `scale` (game coordinates, as for machines), `bsp` (the zone the
  placement says it belongs to, when it names one) and, for `netgame`,
  `item` (the drawn object's group: `weap`, `vehi`, `eqip`). The views do
  not draw these objects, and they are not in `bounds` or the frame.
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
  `end` in game units from the eye), `glow` (the first sky light's
  `colour`, a power the extractor calls `density`, and the light's
  `heading` and `pitch` in radians, game frame; it is the light's
  radiosity record, not a fog, so the viewer draws no haze from it) and `scale` (the model's scale in the
  game). `sky.glb` is the sky's render model at rest in model space (same
  axes), textured, every region's first permutation at the highest detail
  under 30,000 triangles; its `asset.extras.blend` says per material, in
  order, `opaque`, `alpha` or `additive` (from the template's words:
  `add`; `alpha`, `cloud(s)`).
- **Weather.** `asset.extras.weather`, when a BSP has any: per weather
  system (per BSP palette entry with a `weat` tag and clusters), `name`
  (the palette entry's), `kind` (`rain`, `snow`, else `motes`, from words
  in the entry's and the tag's names), `amount` (0.5 `light`/`misty`, 2
  `heavy`/`monsoon`, else 1), `boxes` (the bounds of the clusters it is
  set on, glTF frame: min x, y, z, max x, y, z) and `exclude` (its BSP's
  weather polyhedra, the places it is kept out of: each a list of planes
  `[nx, ny, nz, d]`, glTF frame, inside when `n·p <= d` for all).
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
- **Size.** Retail multiplayer maps come out at 11k to 31k triangles of
  level and 1.2 to 4.1 MB (0.3 to 1.1 MB before textures and objects):
  0.4 to 1.8 MB of it pictures (45 to 150 looks), 40 to 176 objects. Campaign
  levels (1 to 8 BSPs) come out at 38k to 412k triangles of level and 2.1
  to 24 MB (1.2 to 16.6 MB before): 0.4 to 3.4 MB pictures, texture
  coordinates a fifth of the rest, 17 to 854 objects of 2 to 95 models. The
  largest, 04b_floodlab (356k triangles and 566k vertices of level, 108
  machines), is about the most a browser should be asked to fetch. Since
  `EXTRACTOR_VERSION` 13 the pictures are not in that count: Outskirts'
  `map.glb` is 17.5 MB and its `tex/` 2.9 MB; gzipped, `map.glb` is 6.3 MB
  (Zanzibar 2.1 MB, 0.99 gzipped). The level is not simplified. Before
  `EXTRACTOR_VERSION` 6 indices were u32.

### In the 3D viewer

**Water, weather and haze.** The `water` class material ripples
(`water3d.ts`: a few procedural travelling waves bend its normal, no
pictures), is glassy, and at grazing angles reflects the sky's colour
(its clear colour, else its fog's) and turns opaque (Schlick
Fresnel). The weather (`weather3d.ts`) is GPU points in a box around what
the camera looks at, fixed in the world and wrapping as it moves: rain
falls as streaks, snow and motes drift; each point is drawn only inside
one of its system's boxes and outside its kept-out volumes (tested in the
vertex shader against float textures). The toolbar's Weather switch
shows when a level has some. Both keep the view redrawing while shown,
and hold still when motion is reduced. A level's fog is its sky's
atmospheric fog (else its secondary fog) as the tag gives it: linear from
`start` to `end` past the camera, never thicker than its maximum density.
A level with no fog of its own (Zanzibar, Colossus, Foundation, the
converted Halo 1 maps) is drawn nearly clear, as the game draws it: only
the viewer's faint blue-grey haze, so far scenery still reads as far. The
sky light's `glow` once gave such levels a haze in its colour, as thick
as its "density"; that record is the light's colour and power (its
radiosity parameters), and the haze drowned Halo 1's maps in cyan.

**Loading.** The viewer draws the level as soon as `map.glb` has loaded,
untextured (each look shows its class material), then fetches the
pictures six at a time (`textures3d.ts` `deferTextures`,
`streamTextures`) and swaps each in when it arrives: the images are
decoded off the main thread (`createImageBitmap`, no flip, no colour
conversion). Placed objects of one model are instanced (below). Every
art URL the site builds carries the manifest's `source` stamp (`?v=<first
16 characters>`, `web/src/map/geometry.ts`); the picture and sky URLs
keep it. The art server (`crates/live-server/src/art.rs`) answers a
stamped request with `Cache-Control: private, max-age=31536000,
immutable` and anything else with `no-cache`, and sends a file's `.br`
copy to a browser that takes Brotli, else its `.gz` copy to one that takes
gzip ([Packing](#packing)). The glTF loader (`web/src/viewer/gltf.ts`)
has meshoptimizer's decoder; the pictures get the GPU's most anisotropic
filtering (up to 16) and mipmaps. A new extraction
has a new stamp, so nothing stale is kept. Measured on a 10-core Mac,
Chrome, cache off, median of five (time to the first drawn frame /
fully textured, ms):

| | before 13 | 13, uncompressed |
|---|---|---|
| Outskirts, local | 489 / 504 | 307 / 663 |
| Zanzibar, local | 373 / 379 | 211 / 380 |
| Outskirts, 100 Mbit/s | 2443 / 2461 | 1823 / 2702 |
| Zanzibar, 100 Mbit/s | 690 / 695 | 498 / 862 |
| Outskirts, 25 Mbit/s | 6953 / 6968 | 6052 / 7162 |
| Zanzibar, 25 Mbit/s | 1421 / 1426 | 919 / 1468 |

The geometry is most of the wait; gzip takes Outskirts' to about a third
on the wire.

`web/src/viewer/textures3d.ts` keeps both materials of every textured
mesh (the look and the class material its primitive's `flat` names), so
the toolbar's **Textures** button swaps them at once; the choice is kept
in `localStorage` under `h2.viewer.textures` (`on` unless `off`).

`web/src/viewer/scenario3d.ts` draws the `scenario` node: placements of the
same model, kind and zone become one `InstancedMesh`; the placement nodes
stay in the tree, hidden. The Show menu has **Objects** and a switch per
kind present; an object is hidden with its zone (`extras.bsp`). For frames
that move objects (`Frame.objects`: `{k, i, p, f, u, gone?}`, kind and
index as in `extras`, game coordinates), `ViewerScene.scenarioObjects` is:

```ts
// The placement's node, taken out of its batch and shown (unless its kind,
// the layer or its zone is hidden), or undefined when the level has no
// such placement. The caller then owns its position, quaternion and
// visibility; its scale is the placement's.
scenarioObjects.find(kind: string, index: number): THREE.Object3D | undefined
// The same from frame entries: moves each (game axes, as machines), hides
// the `gone`, shows them again when they come back. Returns whether
// anything changed (the scene redraws itself).
scenarioObjects.apply(list: readonly ObjectPose[]): boolean
// Every handed-out object back where the scenario placed it, in its batch
// (after a seek, or when the frames stop sending objects).
scenarioObjects.restAll(): void
```

### Map pack

The nine Multiplayer Map Pack maps ([map-pack.md](../game/map-pack.md)) get the
same art as the disc's multiplayer maps (preview, `map.glb`, `sky.glb`,
views) under their own stems (`containment`, `deltatap`, ...), and the
playlist builder's catalog lists each under the English name and
description its map list record gives (Containment, Sanctuary, ...),
with its map id. They are found in two places (`live_mappack`):

- `h2 live assets extract --mappack <iso>`: the map pack disc's
  `/demos/dlc/<package>/*.map`, named by the package's `patch_v3.lvl`
  record for the map (turned into a version 2 record:
  map-pack.md section 1).
- Where `h2 import-mappack` installed them under a runtime data root:
  `<data>/disk/Partition1/TDATA/4D530064/$c/<offer>/`, each package's
  map and its version 2 `patch.lvl` (name at `+0x114`, 32 UTF-16 units;
  description at `+0x354`, 128; map id at `+0x10c`). `h2 live assets
  extract` reads the default data root unless `--installed <data>` names
  another; `h2 play --live-local` reads the default one.

Each map opens with the retail disc's shared maps, like a disc map. A map
whose stem the disc (or an earlier source) already has is not added
again. The maps found are part of the art's cache key, so installing the
map pack later extracts again. Nothing of them is in the repository.

### Halo 1 maps

The Halo 1 maps `h2 import halo1` (or `h2 play`'s automatic import)
converted into Halo 2 maps under the runtime data root
([campaign.md](../halo1/campaign.md#10-importing-built-clih1_import),
[multiplayer.md](../halo1/multiplayer.md)) get the same art as the
disc's maps: `map.glb`, `sky.glb`, the views and a preview, extracted by
the same code (`live_halo1`). They are real Halo 2 cache files, opened
with the Halo 2 disc's shared maps.

- **Where.** `h2 live assets extract` reads them from the data root its
  `--installed DATA` names (default: the one `h2 play` uses);
  `h2 play --live-local` reads the default data root. Campaign levels are
  `<data>/campaigns/halo1/<level>.map`, named by that folder's
  `campaign.toml` and pictured by its `cards/<level>.png`. Multiplayer maps
  are the `<data>/maps/<dir>/manifest.toml` packages with a variant whose
  `origin.game` is `halo1`: the Halo 1 sandbox's map (`h1/`) when there is
  one, else the first (the level is the same in both), named and pictured
  by the package (`name`, `image`).
- **`--halo1 ISO`** runs the import into that data root first (only what
  its stamp says is missing or stale) and then extracts. Without it the
  extractor uses whatever is converted already; it never reads the Halo 1
  disc itself.
- **Names.** Each map is keyed by the name the game reports it by, the
  cache header's short name (`+0x198`): `h1_<map>` for a multiplayer map,
  the level's own name for a campaign level. So a live game or recording on
  one finds its geometry as a Halo 2 game does. A stem the Halo 2 disc (or
  the map pack) already has is never taken by a Halo 1 map. The game's
  staged copy of a converted multiplayer map
  (`<data>/disk/.../$c/<offer>/`, beside `h2recomp-map.toml`, which
  `live_mappack` finds like a map pack map) gives way to the Halo 1 entry.
- **Manifest.** The entry carries `"game": "halo1"` and `"name"` (the
  package's or campaign list's). `map/<stem>/preview` is the import's PNG,
  copied as it is; a map without one lists `map/<stem>/preview` under
  `skipped`. Halo 1 maps are not in the catalog (`catalog.json`): their map
  list records are the import's, not the disc's.
- **Cache key.** Each converted map's path, size and header, and
  `<data>/campaigns/halo1/import.stamp` (the Halo 1 disc's hash and the
  converter version), join the art's cache key, so a new conversion
  extracts again and an unchanged one does not.
- **Frame.** Halo 1 levels are low-poly: a canyon floor is a few dozen
  large triangles, one or two to a frame grid cell, which [the
  frame](#pipeline) (step 3, at least 3 triangles a dense cell) would leave
  out. For a Halo 1 map any triangle makes a cell dense
  (`Level::with_sparse_frame`); Halo 2's maps keep the rule as it was.
- **Shared blocks.** The converter places identical tag blocks once, so a
  converted render model's blocks may lie in another tag's payload.
  `tags::model::RenderModelGeometry::read_in` then decodes the model
  against the whole metadata from the tag's address (as
  `collision::Bsp::read` does for collision); the extractor passes the
  metadata it has read already.
- **What differs.** The campaign levels' scripts are the converter's; when
  they do not decompile, no BSP is flagged cinematic and the level lists
  `map/<stem>/cinematic` under `skipped` (all its BSPs are drawn). Nothing
  of the maps is in the repository; the tests build invented packages.

### `views.json`

```json
{ "geometry_version": 1, "map": "<stem>",
  "bounds": { "min": [x, y, z], "max": [x, y, z] },
  "frame":  { "min": [x, y, z], "max": [x, y, z] },
  "views": [
    { "id": "top", "kind": "ortho", "width": 2048, "height": 2048,
      "world_to_pixel": [[a, b, c, d], [e, f, g, h]],
      "file": "views/top.webp", "up": "north", "cutaway": true,
      "elevation_deg": 90, "scale": 21.4, "forward": [0, 0, -1] },
    { "id": "iso-ne", "kind": "ortho", "width": 2048, "height": 1536,
      "world_to_pixel": [[...], [...]], "file": "views/iso-ne.webp",
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

RGBA WebP (lossy, quality 85, alpha kept exactly) with a transparent background, so the site can put them on its own
backdrop. Colours are our own calm palette (light warm grey floors, slate
walls, muted teal water and green foliage) lit by a fixed sun high in the
north-west (hillshade convention: shadows fall to the lower right of the top
view), the same for every view.

## Packing

The last step of an extraction (`crates/live-server/src/art_pack.rs`,
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
| JSON (manifest, views, catalog) | 61 | 0.3 MiB | 0.3 MiB | 0.3 MiB | 0.05 MiB |
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
   no longer draws it. A sky
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
   on them too (1/4096).
11. **Scenario objects.** The placement blocks of the object type table
   (docs/formats/objects.md) other than machines: scenery `+0x50`, vehicles
   `+0x70`, equipment `+0x80`, weapons `+0x90`, controls `+0xb8`, light
   fixtures `+0xc8`, sound scenery `+0xd8` and crates `+0x328` (strides
   0x5c, 0x54, 0x38, 0x54, 0x44, 0x54, 0x50, 0x4c; palette 8 bytes on,
   0x28-byte entries), read like machine placements, plus the zone at
   `+0x2c` (i16, kept when it is one of the scenario's BSPs). A placement
   whose flags (`+4`) have bit 0 set is created by a script, not at the
   start, and is left out: on the disc these are the cutscenes' props
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
  the variant the placement names. Objects a script creates later are not
  drawn, nor bipeds (AI come from the frames). A multiplayer spawn shows
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
- A level with several skies (one per zone) gets only the first with a
  model. Additive sky layers (haze, glows) are left out by the viewer:
  without the shader's constants they wash the sky out.

# Stats banners

The stats site's banners are collages of in-game views of the owner's own
maps: a tall one over the home page (`banner/home`, 902 × 180 CSS pixels)
and a slim strip over the other stats pages (`banner/strip`, 902 × 96),
both drawn at twice that. Like the rest of the art they are made on the
owner's machine from their extraction and are never in the repository;
only the rules for drawing them are.

```sh
cd web && bun install && bun run banner [--art DIR] [--chrome PATH] [--shots DIR]
```

`--art` is the extracted art (default `game-art/` in the server directory,
or `$H2_LIVE_DATA/game-art`). The banners are written into it as
`banner/home.jpg` and `banner/strip.jpg` and added to its `manifest.json`
as `banner/home` and `banner/strip` (earlier `banner/*` keys are replaced,
nothing else is touched), and the manifest's `.br` and `.gz` copies are
written again from the new bytes (Brotli quality 11 with a 16 MiB window,
gzip level 9, as the extractor writes them), so the native server serves
them and `h2 live assets publish` uploads them with the rest. An extraction starts
the directory afresh: run it again after `h2 live assets extract`. It
takes about ten seconds. `--shots DIR` also writes each shot alone.

**How.** `web/banner/run.ts` starts Vite on the page `web/banner/index.html`
with the art served at `/art/game/` as the server does, runs a headless
Chrome (`--chrome`, else `$CHROME`, else the usual install) and drives it
over the DevTools protocol. The page (`web/banner/page.ts`) loads each map
in the 3D viewer's own scene (`viewer/scene.ts` with `bare` set: players
without rings, aim rays or labels; teleporter markers off; ambient
occlusion on), waits for its pictures and sky, puts the camera where the
shot says (`aim`) and reads the frame back. Players in a shot are drawn
with the server's models, each on the floor below them (`floorBelow`).
The level's own vehicles are drawn as the viewer draws a game's (from
`models/vehicle/*.glb`, so a Warthog has its turret) where it has a model
for each; the level's turretless hulls are hidden then.

**The plan** (`web/banner/plan.ts`) names no coordinates from the disc. A
shot is either relative to the level's playable bounds (a point as
fractions of them, an azimuth, an elevation and a distance as a share of
their half-diagonal) or stands at one of the level's placed objects, as
the viewer keys them (`netgame:66`: a multiplayer spawn), a few units
behind and above it, looking at another or across the level, with an
optional cast (species, weapon, team colour, where they stand relative to
the camera and which way they face). A composition lists shots left to
right: the width is split evenly, the seams lean (`slant`) and blend over
`feather` of a panel's width, with weights that sum to one across a seam.
Each shot is drawn at its panel's size, so it is framed for it. The whole
is then graded so different maps read as one: a soft-light navy tint, a
faint rule along each seam, dark sides (darker on the right, under the
wordmark), a fade at the bottom into the page's navy (the home page's
columns ride up over it) and fine scan lines. A composition with a shot
that cannot be drawn (its map or spot missing) is not written, so the
page keeps its fallback rather than show a gap.

**Choosing shots.** `bun run banner --sheet DIR [--maps a,b] [--size WxH]`
draws a contact sheet per map of candidate shots, labelled with their ids:
from each of up to 16 of the level's multiplayer spawns (vehicles first,
then spread out), looking across the level. `--shot-file FILE` draws a JSON
list of shots instead, for trying casts and angles. `--verbose` passes the
page's console through.

**The site.** `Strip` (`web/src/stats/Layout.tsx`) shows the banner when
the art has its key (`bannerKey`), whole and ungraded by CSS. Without it the
strip is the art pack's picture over an original backdrop (slanted light
seams like the collage's, a glow from the top right, scan lines), so a
server without game art, or a browser it won't share the art with, still
gets a finished header. The home strip is 180 px tall either way, so the
page doesn't jump when the art arrives.

**Limits.** Shots name spawn indexes and map stems of the retail disc; a
different disc or map pack may need new ones (`check` catches only a
malformed plan; a missing spot skips the composition). Skies are drawn as
the viewer draws them (additive layers left out), so they read flatter
than in the game; the grade's tint carries the top of the picture.

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
            "vehicles": { "warthog": { "glb": "models/vehicle/warthog.glb" } },
            "seats": { "warthog": [ { "label": "warthog_d", "node": "seat_driver" },
                                    { "label": "warthog_p", "node": "seat_passenger" },
                                    { "label": "warthog_g", "node": "seat_gunner" } ] } }
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

`seats` (extractor 22) lists, by vehicle stem, the vehicle's seats in the
order of its tag's seats block (`vehi +0x1c8`, stride `0xb0`;
crates/tags/src/defs/unit.txt), the block the unit's seat index
(`unit +0x1fc`, telemetry's `seat`) indexes: element `i` is seat `i`.
`label` is the seat's label (string ID at `+4`), the rider's animation
mode, so `any:seat_<label>` is the biped clip that seats the rider (the
example's order is illustrative; the tag's order is what is written). `node` is the vehicle glb's `seat_` node for the seat's marker
(string ID at `+8`, named as in pipeline step 8), or `null` when the
marker is not a seat marker or the model has no such node (the rider is
then placed by the reader's own rule). A vehicle without seats is not
listed, and `seats` is absent when no vehicle has any. The vehicle's own
entry under `vehicles` is unchanged.

Art from an older extractor (the manifest's `extractor` below the running
`h2`'s) is stale: before version 2 its bipeds had no clips, so the viewer's
players couldn't animate. `h2 live serve` prints a warning naming the
command to extract again, `h2 live assets check [--out DIR]` exits 1 for
missing or stale art (deploy/live/setup.sh extracts again then), `h2 play
--live-local` extracts again by itself (the extractor version is part of the
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
  u32 indices. Biped primitives also have `COLOR_0` (float
  `VEC4`) when any of the model's materials has a change-colour map: red is
  the primary change-colour weight, green the secondary (0..1), sampled
  bilinearly from the material's change-colour map at the vertex. A
  material without a change-colour map has zero weights; weapons have no
  `COLOR_0`. These are weights, not colours: a reader must not multiply
  the base colour by them (the viewer turns vertex colours off).
- **Turrets.** A vehicle's turrets and guns are objects of their own that
  its model variant attaches (hlmt variants `+0x50`, 0x38 bytes: name
  string ID at `+0`, objects block at `+0x1c`, 0x10 bytes each: parent
  marker and child marker string IDs, then the child object reference).
  The variant is `multiplayer` when the model has one (the Scorpion's
  `cannon_mp`, the Wraith's `mortar_mp`, which has no side guns), else the
  object's default (`obje +0x30`), else the first (the Warthog's
  `default`: the chaingun; `gauss` and `troop` are other variants). Each
  child is built like a vehicle at rest, in its own colours, its child
  marker (or its origin, when it names none) on the parent's marker in the
  vehicle's pose, and its own attached objects in turn (two levels). An
  object attached to the vehicle itself is drawn as parts that aim (live
  art 18): a `turret_<n>` node at its attachment frame, extras
  `{"turret": "<tag stem>"}`, with a mesh of what turns with its yaw
  (everything but its guns, and what it carries in turn), and under it a
  `turret_<n>_pitch_<k>` node per gun, at that gun node's rest origin with
  the turret's axes, with a mesh of what hangs from it. The guns are the
  turret's nodes named `gun`, `cannon`, `gun_arm` or `turret_gun` with no
  such node above them (the Warthog's chaingun and gauss `gun`, the
  Scorpion's `cannon` and coaxial `gun`, the Wraith mortar's `gun_arm`,
  the Spectre's `turret_gun`); a triangle goes with its first vertex's
  heaviest node. A reader turns the yaw node about its own up (glTF +y)
  and each gun about its own side (glTF +z, raising it); at rest they
  are as the game models them. Parts are not skinned. Its materials join
  the vehicle's, and its seat nodes join the vehicle's (`seat_gunner` on
  the Warthog's chaingun) unless the vehicle has that name.
  `asset.extras.attached` lists the attached objects' tag names. On the disc the Ghost and Banshee attach nothing:
  their guns are in their hull models.
- **Vehicle colours.** Halo 2 paints a vehicle with its object's own
  change colours, not its driver's team (a red player's Banshee is as
  violet as anyone's). The `obje` change colours (`+0xac`, 0x10 bytes
  each: a block of initial permutations, then one of functions; a
  permutation is 0x20 bytes: weight, lower and upper bound RGB, variant
  string ID) give, per colour, the permutation of the object's default
  variant (`obje +0x30`), else the first, at the middle of its bounds. On
  the disc only the Banshee (violet and dark violet; the heretic variant's
  gold) and the Scorpion (grey, on no marked surface) have any. They are
  painted into the base maps as the game tints (per channel: one where
  the change-colour map is empty, the colour where it is full; red
  primary, green secondary), so a vehicle has no change-colour map and no
  `COLOR_0`.
- Materials (`material_look`): `baseColorTexture` (an embedded PNG, at most
  512 pixels a side) when the diffuse map decoded, else a grey
  `baseColorFactor`; metallic 0, roughness 0.8 unless:
  - the shader is a bump-env or shiny one (`tex_bump_env*`,
    `tex_bump_shiny`): the base map's alpha is Halo 2's specular mask, and
    becomes a `metallicRoughnessTexture` (green roughness 0.85 to 0.3,
    blue metalness up to 0.6 for env, 0.35 for shiny, red the mask
    itself); the base map is then stored opaque. The shader's reflection
    tint is `extras.h2.env` `{perpendicular, parallel}` (RGB 0..1): its
    last two colour parameters with any colour (compiled postprocess
    `+0x0c`, packed ARGB; those after carry only alpha). Facing the eye
    the reflection takes the first, at a grazing angle the second: the
    Covenant's sheen (Ghost orange to purple, Spectre pale yellow to blue,
    Wraith light to deep blue, Banshee blue to dark violet). The viewer
    adds the environment, clamped to 1 as the game's cube maps are low
    range, times the mask and the tint (`ENV_FRAGMENT`, at 0.5);
  - it has a self-illumination map (a bitmap named `*illum*`), or is a lit
    display (`illum`, `*meter*`, `*plasma*` templates): `emissiveTexture`
    (that map, or the base map) with `emissiveFactor` 1;
  - it is transparent (`shader_templates\transparent\...`): `*_add_*`
    templates are glows, `alphaMode` `BLEND`, black base, the base map as
    the emission and `extras.h2.blend` `"add"` (the viewer adds them);
    the others are glass, `BLEND` with a see-through blue-grey factor and
    no texture; `overlay` templates (decals) blend by their alpha;
  - it is an `overlay` decal on an opaque model whose pass blends by
    colour (render states `ALPHABLENDENABLE`, `SRCBLEND`, `DESTBLEND`;
    Xbox `D3DBLEND`): `DESTCOLOR`, `SRCCOLOR` doubles what is under it
    (`extras.h2.blend` `"double"`), `DESTCOLOR`, `ZERO` or `ZERO`,
    `SRCCOLOR` multiplies it (`"multiply"`). The material is
    `KHR_materials_unlit`, opaque, its picture as stored (mid-grey leaves
    the hull alone). Every vehicle overlay on the disc doubles (the
    Scorpion's `icon`, `number_decal`, `marine`, `no_step`, its cannon's
    `scorpion_decal`). The viewer blends it so, unlit, not tone mapped,
    without writing depth and pulled towards the camera (polygon offset);
  - it is any other `overlay` decal on an opaque model whose picture has
    no alpha and reads, as decal layers do
    (pipeline step 7), as doubling or multiplying: it is turned into one
    blended by alpha that looks about the same (`overlay_as_alpha`; by
    luminance l, double: white at l − 0.5 when l ≥ 0.5, else black at
    1 − 2l; multiply: black at 1 − l), not a grey slab;
  - it is alpha-tested (`*alpha_test*`): `alphaMode` `MASK` at 0.5, its
    alpha kept.
  The change-colour map (at most 256 pixels a side) is also stored, its
  texture index in `extras.h2.change_colour`, for readers that tint per
  pixel (the viewer does); `extras.h2.template` names the shader template.
  Each distinct picture is stored once. Bump maps (8-bit palettized
  normals; the palette is the engine's, not the tag's) and detail maps are
  not used.
- `asset.extras`: `generator`, `models_version` (2) and, for bipeds,
  `height` (game units, feet to the top of the head) and `lift` (game
  units, extractor 22): how far the model was raised to put its lowest
  vertex on the floor (pipeline step 7), positive when raised. The game
  draws a biped at its object origin, not lowest vertex on the floor, so
  a reader that lowers the model by `lift` puts the model's own origin at
  the object's position.
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
  `airborne`, `land`, `land_hard`, `grenade`, `melee`, `reload`, `fire`,
  `switch`, then the movement states (extractor 22) `run_back`,
  `run_left`, `run_right`, `walk_back`, `walk_left`, `walk_right`,
  `crouch_run`, `crouch_back`, `crouch_left`, `crouch_right`, each only
  where the graph has it; and `any:die`, `any:dead` for every class
  (before extractor 13, `die` per class was the dead pose). After those,
  one seat pose per vehicle seat mode of the graph, `any:seat_<mode>`
  with `<mode>` the engine's animation mode as the seat names it
  (`any:seat_warthog_d`, `any:seat_warthog_g`, `any:seat_ghost_d`,
  `any:seat_scorpion_p_lf`, ...; extractor 22): the seated rider, one
  frame. Every joint has a `rotation`
  channel (normalised i16 `VEC4`, the core glTF option) and a
  `translation` channel (float `VEC3`), `LINEAR`, one key per frame at
  30 fps; a channel that never changes in the clip has a single key at
  time 0, so a seat pose has one key at time 0 on every channel. The clip
  keeps the floor of the default pose (no root motion: the
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
whole step takes about 0.6 s on 10 cores. (Counted at extractor 21,
before the movement clips and seat poses; the disc test prints the
current counts.)

## Pipeline

1. **Bipeds.** On the multiplayer maps, the first `bipd` tag whose file name
   is `masterchief_mp` (else `masterchief`) is the spartan, `elite_mp` (else
   `elite`) the elite. Its object model (`obje +0x34`, docs/formats/objects.md) gives
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
   `walk_front` (walk), `move_front` (run), `airborne` (the whole leap:
   the game has no take-off clip), `land_soft` (land), `land_hard`
   (land_hard) in mode `combat`, and `idle` in mode `crouch` (crouch).
   The movement states are actions of the same kind, after a class's
   other states: `move_back`, `move_left`, `move_right` (`run_back`,
   `run_left`, `run_right`), `walk_back`, `walk_left`, `walk_right` (the
   same names) in mode `combat`, and `move_front`, `move_back`,
   `move_left`, `move_right` in mode `crouch` (`crouch_run`,
   `crouch_back`, `crouch_left`, `crouch_right`).
   Seat poses: a mode is a vehicle seat when it is not `combat` or
   `crouch` and a `_`-separated word after its first is `d`, `g` or `p`
   (driver, gunner, passenger: `warthog_d`, `spectre_g`, `scorpion_p_lf`,
   `c_turret_ap_d`, `pelican_p_l01`); generic, matched at run time. Each
   seat mode in graph order gives `any:seat_<mode>` from its `idle`
   action: class `rifle` (a passenger holding a weapon), else `any`, else
   the first class the mode has, type `any` before the others. Only its
   first frame is sampled. A seat mode without an `idle` (or whose idle
   is an aim grid) has no pose.
   Dying and lying dead are the same for every class, so they are written
   once as `any:die` and `any:dead`, first in the glb: `die` is the death
   animation of a soft kill from the front (`s_kill:front:gut`, its first
   variant; else `h_kill:front:gut`, else the dead pose), `dead` the
   action `landing_dead` of class `any` (the body on the ground; the viewer
   holds the death clip's last frame, and uses this pose for a body it
   never saw alive). These go
   through the action table, so `unarmed` plays what the graph
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
   biped glb over 3.25 MiB is made to fit (`next_size_drop`): first the
   movement clips of every class but `rifle` go, a class at a time, last
   of the class list first (`ball`, `flag`, `unarmed`, `sword`,
   `support`, `missile`, `pistol`); then whole classes, last first
   (`ball`, `flag`, `unarmed`, ...; the rifle's movement goes only with
   the rifle class); only then the shared `any` clips (dying and the seat
   poses, which are one frame each and cost little). The elite keeps its
   sword clips; the report's animation note names what was dropped
   (`ball moves`, `flag`, ...).
10. **Weapon class.** The string ID at `weap +0x288` (D: every multiplayer
    weapon names a class of the player graphs there: `rifle` for the
    battle rifle, carbine, shotgun, SMG and the snipers, `pistol` for the
    magnum, needler and plasma pistol, `missile` for the rocket launcher and
    fuel rod, `support` for the brute shot, `sword`, `flag`, `ball`; vehicle
    guns say `fixed`).
11. **Textures.** The shader's bitmap parameters (`shad +0x20` postprocess,
   `+4` block, datum first; docs/formats/shader-instances.md). The compiler
   shares identical compiled data: a shader's postprocess may lie in
   another shader's payload (the Scorpion's right and spare treads point
   into its left tread's; the Wraith's boost glow and damage shaders do
   the same), so the blocks are read from the map's metadata wherever
   they are, not only the tag's own bytes. A bitmap's role comes
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
that the elite keeps its sword clips, that the spartan has `rifle:run_left`,
`rifle:run_back`, `rifle:crouch_run` and `any:seat_warthog_d` and the
Warthog's `seats` a `warthog_d` (it prints every vehicle's seats, the
spartan's seat poses and every biped's `lift`), and the battle rifle's, plasma rifle's
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
  whole class (the battle rifle's for `rifle`, and so on), and `any:die`
  is one death animation for every way of dying (multiplayer deaths are
  ragdolls in the game). Strafing, backwards and crouched movement are
  separate clips (extractor 22), not blended by speed or direction; a
  glb over the size cap may lose them for its less held classes. No
  turning clips, no aim. Characters without a rifle stance
  use their own idle (grunts the pistol one, hunters and flood forms the
  unarmed one, prophets and sentinels a combat idle); Lord Hood has no idle
  and stands in his rest pose.
- Vehicles are posed once: no wheel suspension. Turrets aim by their
  parts (above), at rest until a frame says where; the gun nodes are
  chosen by name, so a turret whose gun is named otherwise only turns in
  yaw, and pitch is about the gun node's rest origin, which may not be
  the game's exact hinge. A Warthog is always the chaingun one (a gauss
  Warthog is the same tag in another variant). A
  vehicle with no seat markers has no `seat_` nodes, and a seat node is the
  marker's frame; the rider's seated pose is the biped's
  `any:seat_<label>` clip, one still frame (no steering, aiming or
  entering), and the seat's mode is matched by its name's `_d`, `_g`,
  `_p` words.
- `COLOR_0` change-colour weights are per vertex, so their edges are as
  coarse as the mesh; the per-pixel map in `extras.h2.change_colour` is
  exact. Only the primary and secondary colours are tinted.
- No bump or detail maps; reflections are the reader's environment, not
  the shader's cube map, tinted by the shader's last two colours; other
  colour parameters are not read, so the chief's visor is the diffuse
  map's dark colour, not gold. The Ghost's red glyph is in its base map
  (no decal): it is drawn as the disc paints it. The Warthog's olive
  hubs are its tyre map's colour; its rubber and dirt detail maps would
  vary it but are not drawn.
- A vehicle is drawn in its default variant: the Banshee's indexed base
  map shows its first image (the heretic Banshee's is the second).
- Elite helmets and other permutations: only `base` (or the first).
