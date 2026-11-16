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
key) is 4, so a disc extracted by an older build is extracted again.

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
      "file": "views/top.png", "up": "north",
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
7. **Pictures.** `live_render`: each view is rasterised at 2 × 2 samples per
   pixel into depth and packed normal/class buffers, culling back faces and
   anything outside the frame (plus 1%). Shading per sample: Lambert from the
   sun with a 2048² shadow map (sun-facing casters inside the frame, 3 × 3
   taps); hemispheric ambient (cool sky, warm ground); screen-space ambient
   occlusion from 8 depth taps at a 1.2-unit radius; a height tint (lower is
   darker); thin darker lines at silhouettes, depth steps and creases. A
   pixel whose samples lie on one flat surface is shaded once. Glass is not
   drawn. Samples are averaged in linear light; alpha is coverage.
8. **Write.** `map.glb` as soon as a map is built, then every (map, view)
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

writes the outputs, a timing report, the shader names behind the classes
and a contact sheet of every view to a directory outside the repository (the
path is `CHECK_DIR` in the test). Do not commit anything it writes.

## Limits

- The top view shows roofs: indoor maps read as their rooftops from above.
  The iso views cull walls facing away from the camera, which opens rooms
  facing it.
- Classes from names are a heuristic; unnamed surfaces fall back to facing.
- The frame is a heuristic too. It keeps every retail map's playable area on
  the disc this was checked against, but a map with a dense, detailed
  backdrop could be framed wider than its play space.
- No textures, lightmaps or object geometry (scenery, crates, vehicles):
  only the BSP.
