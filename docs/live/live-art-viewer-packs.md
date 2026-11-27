# Live art: the 3D viewer, map packs and mods

Part of [Map geometry and pre-drawn views](live-art.md). Covers: In the 3D viewer; Map pack; Halo 1 maps; Mods.

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
pictures sixteen at a time (`textures3d.ts` `deferTextures`,
`streamTextures`; `PICTURES_AT_ONCE` in `scene.ts`) and swaps each in
when it arrives: the images are decoded off the main thread
(`createImageBitmap`, no flip, no colour conversion). Player, weapon and
vehicle models wait for `map.glb` (`ModelLibrary.hold`), so they don't
share its bandwidth. Placed objects of one model are instanced (below). Every
art URL the site builds carries the manifest's `source` stamp (`?v=<first
16 characters>`, `web/src/map/geometry.ts`); the picture and sky URLs
keep it. The art server (`crates/live/live-server/src/art.rs`) answers a
stamped request with `Cache-Control: private, max-age=31536000,
immutable` and anything else with `no-cache`, and sends a file's `.br`
copy to a browser that takes Brotli, else its `.gz` copy to one that takes
gzip ([Packing](live-art-pipeline.md#packing)). The glTF loader (`web/src/viewer/gltf.ts`)
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

**Performance (measured 2026-11-19).** `bun run bench:viewer` (in `web/`,
after `bun run build`; `web/bench/viewer.bench.ts`) serves the built site
locally with the API and art from live.forbes.red, opens a game's page
with `?view=3d&perf` in headless Chrome and reports the load (requests,
bytes on the wire, ms to the first frame with the level and to fully
textured; median of cold runs, then one warm) and, with the replay
playing, draw calls, triangles and CPU ms per frame in `update()` and in
`draw()` (three.js's submission, not GPU time), with AO on and off.
`--profile` adds the functions the page spent its time in; `--census`
the visible meshes; `--timeline` every request. `?perf` puts the scene on
`window.h2Viewer`; the scene marks `h2-viewer:first-map-frame` and
`h2-viewer:textured` with User Timing. On a 10-core Mac (cold, cache
off; load times swing with the network, by a second or two):

| | before | after |
|---|---|---|
| Old Mombasa (AI, 5 zones): first frame / textured | 5037 / 17017 ms | 2385 / 6398 ms |
| Old Mombasa, AO on: draw calls, draw ms | 3762, 19.6 (p95 20.9) | 1931, 9.8 (p95 10.5) |
| Old Mombasa: update ms | 1.3 (p95 18.3) | 1.1 (p95 1.3) |
| Cyclotron (4 players): first frame / textured | 1654 / 5011 ms | 1413 / 2838 ms |
| Cyclotron, AO on: draw calls, draw ms | 931, 5.1 (p95 5.5) | 497, 3.2 (p95 3.4) |

What changed: the ambient occlusion reads the depth the scene was drawn
with (the composer's target has a depth texture, `aopass.ts`) instead of
drawing the whole scene again for its own depth and normals, which also
had three look each mesh's program up again (skinned, instanced and plain
meshes shared that one material); the world matrices are updated once a
frame, not once per pass and again for the labels; floor probes for
bipeds' feet (`ground.ts`) go to a grid of the level's triangles
(`floorgrid.ts`) rather than raycasting every triangle of the level (the
update's 12 to 18 ms spikes on a campaign level); pictures sixteen at a
time (each is a round trip of 100 to 300 ms to the art host, so six at a
time was latency-bound: a campaign level's 197 took 12 s), and models
after the level. What is left, by the profile: the shadow pass redraws
every caster each frame though the level never moves (about half the
remaining draw calls; a cached static shadow map would halve them), and
three's per-object program checks (`getProgram`) between skinned and
plain meshes in that pass. A campaign level with its AI is ~1900 draw
calls, of which the AI's skinned meshes are about 450 (223 meshes, drawn
and shadowed).

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
playlist builder's catalogue lists each under the English name and
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
  `skipped`. Halo 1 maps are not in the catalogue (`catalog.json`): their map
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

### Mods

The Mods catalogue serves each release's cover and gallery from Live. On the
uploader's machine, `h2 live mods capture PACK_ID|MAP_ID` starts a private
headless game for each member. The CLI gets a Live capture permit and uses
the game agent to load the installed package, place the camera at player
starts and capture three rendered views. The game binds the exact package,
tick, camera and PNG digest to the returned frame. For a pack, the CLI also
selects three member views for the release gallery. Private captures enter
the review queue and appear in the gallery after approval. The site labels
retail map art when it uses it as a fallback.
[Capture options](../runtime/switches.md#tools) · [Gallery API](live-api-content.md#verified-screenshot-uploads)

Installed mods, the map packages under `<data>/maps/`
([maps.md](../game/maps.md)), get art of their own, extracted from the
patched map the game would load (`live_mods`), so the 3D view of a game
on a mod draws the mod's level, objects, scripted spawns and pictures,
not the retail map's.

- **Where.** The same data root as the map pack and Halo 1 maps:
  `--installed DATA` (default: the one `h2 play` uses), and the default
  data root for `h2 play --live-local`. Every valid package with patch
  layers is a mod; converted Halo 1 maps (packages with a variant per
  sandbox) are left to the Halo 1 section above.
- **Resolving.** Each package goes through `content::Library` as the
  game's package backend resolves it: its layers over the disc's map of
  its `[base] map` (or the map pack map `h2 import-mappack` installed),
  the base checked against the package's SHA-256, or its own whole map for
  a derived map. An offset-patched (PPF, IPS) result is written once into
  `<out>.mods/` beside the output while the extraction runs and removed
  after; a materialised (BPS) result is read from `<data>/maps/cache/`. A
  package that does not resolve (no base, a base that differs, a damaged
  patch) is left out with a `mods:` notice.
- **Keys.** A mod's stem is `mod-<first 16 hex digits of its MapId>`,
  never its base map's name, so it never stands in for the retail map and
  two mods of one map are two entries. Its manifest entry carries `name`
  (the package's), `map_id` (all 64 hex digits, as games report it),
  `base` (the map it is played as) and `sha256` (of the patched map). The
  art's cache key covers every installed mod's `MapId`, which hashes the
  base's SHA-256 and every layer's, so a new, changed or removed mod
  extracts again, and one that fails to resolve does not make every start
  extract again.
- **Content.** Everything is read from the patched map: the BSPs, the
  scenario's placements and netgame equipment (however many the mod has,
  wherever it moved them, whatever tags it names), machines, scripted
  spawns, the sky and weather, and the pictures (bitmap tags and their
  pixel data, the mod's own tags and retail-named tags it changed alike).
  Pictures are cached per level by tag ID while it is extracted and
  written under `map/<stem>/`, so no picture is shared by name with the
  base map's. The shared maps (`shared`, `single_player_shared`) are the
  disc's, as in the game.
- **In the web.** A game's view looks its art up by `map_id` first
  (`map/views.ts` `mapFilesFor`) and draws the mod's entry when there is
  one, else the base map's (when the package says its geometry is the
  base's) or nothing.
- **Shaders.** A solid shader whose base map cannot be found or decoded
  (a mod's own template among them) is drawn in its surface class's
  colour, and the level lists `map/<stem>/shaders` under `skipped`: how
  many and which templates.
