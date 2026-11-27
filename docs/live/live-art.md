# Map geometry and pre-drawn views

`h2 live assets extract` (and `h2 play --live-local`, once per disc) writes each
multiplayer and campaign level's geometry and five pre-drawn pictures of it next to
the rest of the game art ([live.md](live-pieces.md#game-art)). The stats site's
tactical map plots players on the pictures; the 3D game viewer loads the
model. Everything comes from the owner's disc at run time and is served
under `/art/game/` like the other art, to this machine only unless the
server runs with `--share-game-art`. Nothing derived from the
disc is in the repository: the tests build invented tag data.

Code: `crates/apps/cli/src/live_geometry.rs` (extraction, mesh, glTF, files),
`crates/apps/cli/src/live_looks.rs` (surface pictures),
`crates/apps/cli/src/live_basemap.rs` (which bitmap a shader draws as its base),
`crates/apps/cli/src/live_mappack.rs` (map pack maps),
`crates/apps/cli/src/live_scenario.rs` (placed objects),
`crates/apps/cli/src/live_layers.rs` (decals and effects), `live_sky.rs` (the sky),
`live_weather.rs` (weather volumes), `live_models.rs` (player, weapon and
vehicle models), `live_weapons.rs` (weapon ballistics), `live_features.rs`
(teleporters and lifts), `live_catalog.rs` (the playlist builder's catalogue) and
`crates/apps/cli/src/live_render.rs` (the software renderer). Tests:
`crates/apps/cli/src/live_geometry_tests.rs` and the modules' own.

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
([Packing](live-art-pipeline.md#packing)).

`manifest.json` gains a `maps` object (absent when nothing was extracted):

```json
"maps": { "<stem>": { "geometry": "map/<stem>/map.glb",
                      "views": "map/<stem>/views.json",
                      "kind": "multiplayer", "bsps": 1 } }
```

`kind` is `"multiplayer"` or `"campaign"` (the map's cache type); `bsps` is
how many structure BSPs `map.glb` holds. Both are absent in manifests from
older builds. A converted Halo 1 map's entry also has `"game": "halo1"` and
`"name"`, its display name ([Halo 1 maps](live-art-viewer-packs.md#halo-1-maps)); Halo 2's own maps
have neither. Paths are relative to the manifest. A map whose geometry could
not be read is listed under `skipped` as `map/<stem>/geometry` and has no
entry; a single BSP of a level that could not be read is listed as
`map/<stem>/bsp_<index>` and the level keeps its other BSPs.
`GEOMETRY_VERSION` is 1 (`views.json` is unchanged; the web viewer checks
it); the art's `EXTRACTOR_VERSION` (part of the cache
key) is 25, so a disc extracted by an older build is extracted again
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
poses `any:seat_<mode>`, the models' `seats` table and `asset.extras.lift`;
23 installed mods' art and scripted spawns; 24 the sky's see-through
layers keep their pictures; 25 exports the animated plasma pass on lift geometry).

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
  exactly, [Packing](live-art-pipeline.md#packing)) at the bitmap's full stored size (the
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
  axes. The child's `extras`
  repeat the placement in game coordinates as telemetry frames give a
  moving machine: `pos`, unit `forward` and `up`, and `scale`. The node
  names are stable so a frame's world pose can move the matching node.
  Machines are class `metal` unless their shader names them glass, water or
  foliage. The mesh uses each region's first permutation at the highest
  detail level it has through 3 of 5.

  For a machine with a `position` overlay, the extractor writes a glTF
  skin and a clip named `device_position_<placement index>`, with time
  from 0 to 1. The node's `extras.device` carries the scenario machine
  `index` and a `loop` flag. Its geometry, inverse bind matrices and
  animation samples are shared across placements of the same model.
  The viewer scrubs the clip to the travel in telemetry and applies any
  world pose to the root. In the absence of travel samples, the rig
  stays in its bind pose. Older map art can still use root poses. The
  viewer interpolates cyclic travel across the wrap by the shortest
  distance, assuming the wheel advances less than half a cycle between
  samples [guess]. The retail clips checked on Zanzibar, Ascension and
  the campaign maps sampled here have rotation and translation channels
  with no animated scale.

  The extractor follows the `hlmt` animation reference and the graph's
  `position` overlay (see [Animation graphs](../formats/animation.md)).
  Looping devices have machine definition type 2 (`mach+0x11c`, checked
  on the Zanzibar wheel and Ascension antenna). The engine branches on that
  value at `0x001153ee` and wraps `+0x140` at 1 in `0x0011547d–0x00115494`.

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
  `item` (the drawn object's group: `weap`, `vehi`, `eqip`), and `script`
  for a placement a script creates (below). The views do not draw these
  objects, and they are not in `bounds` or the frame.
- **Scripted spawns** (`live_scripted`). A placement the level does not
  make at the start (flag bit 0) is drawn when one of the scenario's
  scripts creates it, and any placement a script creates carries
  `extras.script`: `name` (the script), `looping` (a `continuous` script)
  and `period` (seconds between runs, when its sleeps say). Effects a
  script plays are `asset.extras.spawners`, a list of `script`, `effect`
  (the effect tag as the script names it), `at` (the cutscene flag or
  object it is played at), `pos` (game coordinates), `looping` and
  `period`. The 3D view marks each spawner with a diamond that bursts at
  each play (`viewer/spawners.ts`), shown with the effects. Mods use these
  for vehicles that arrive later, respawning turrets and effects (a
  projectile salvo) on a loop; retail multiplayer maps have almost none.
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
- **Plasma.** A `transparent/plasma_alpha` surface keeps its geometry and
  texture coordinates under its BSP or placed model. Its unlit material
  names the coverage mask in `baseColorTexture`, the two noise textures
  by index in `extras.noise`, and the pass data in `extras.plasma`.
  `uv` holds the scale, offset and optional per-axis scroll
  (`from`, `to`, `period` in seconds) of noise A, noise B and the mask,
  in that order. `colors` holds the three RGB constants used to tint and
  brighten the pass. The viewer adds the masked result with depth testing
  and no depth writes. The separate 2D tactical views still use their
  simplified glass class for those triangles.
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
  `add`; `alpha`, `cloud(s)`). Nearly every sky layer is a
  `transparent\sky_*` (or `two_alpha_clouds`) template: the alpha ones
  carry their picture as `baseColorTexture`, blended by its alpha
  (`live_models::Surfaces::Sky`), not the glass tint an object's
  see-through surfaces get; one whose bitmaps' names none read as a base
  map takes its first own picture (`live_models::sky_picture`). Before
  extractor 24 every alpha layer was that glass tint (`GLASS`, alpha
  0.35), so the viewer drew each sky as a faint dark veil over its fog
  colour, which read as no sky at all.
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

Moved to [Live art: the 3D viewer, map packs and mods](live-art-viewer-packs.md#in-the-3d-viewer).

**Loading.** How the viewer fetches and draws `map.glb`: [live-art-viewer-packs.md](live-art-viewer-packs.md#in-the-3d-viewer).

### Map pack

Moved to [Live art: the 3D viewer, map packs and mods](live-art-viewer-packs.md#map-pack).

### Halo 1 maps

Moved to [Live art: the 3D viewer, map packs and mods](live-art-viewer-packs.md#halo-1-maps).

### Mods

Moved to [Live art: the 3D viewer, map packs and mods](live-art-viewer-packs.md#mods).

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

Moved to [Live art: packing, pipeline and limits](live-art-pipeline.md#packing).

## Pipeline

Moved to [Live art: packing, pipeline and limits](live-art-pipeline.md#pipeline).

**Looks.** How the surface pictures are chosen per BSP material: [live-art-pipeline.md](live-art-pipeline.md#pipeline).

## Checking against your disc

Moved to [Live art: packing, pipeline and limits](live-art-pipeline.md#checking-against-your-disc).

## Limits

Moved to [Live art: packing, pipeline and limits](live-art-pipeline.md#limits).

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

**Source art.** `web/art-source/medals/` holds our modes' medals (the
superbounce family and Incel) drawn natively at 512 × 512 and halved to 256 (`<id>-512.png`,
`<id>-256.png`, transparent, 8 bits a channel, no 4-bit quantisation):
source art for banners and other pictures, not for a medal view, where
they would sit wrong beside the 24-pixel medals and Halo 2's glyphs. They
are outside `web/public/`, so not served, and no page names them.
`python3 scripts/live-medals.py --out DIR --hires` draws Incel and the
bounce medals into `DIR/hires/`, from the same drawing as the site's medal
files (`web/public/art/ART.md`, Batches 07 and 08). Halo 2's own medals
have no source art.

**Limits.** Shots name spawn indexes and map stems of the retail disc; a
different disc or map pack may need new ones (`check` catches only a
malformed plan; a missing spot skips the composition). Skies are drawn as
the viewer draws them (additive layers left out), so they read flatter
than in the game; the grade's tint carries the top of the picture.

# Player and weapon models

Moved to [Live art: player and weapon models](live-art-models.md#player-and-weapon-models).

**Turrets.** How vehicle turrets and guns are posed: [live-art-models.md](live-art-models.md#glb).

## Files

### glb

## Pipeline

## Checking against your disc

## Limits

