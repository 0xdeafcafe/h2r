# Native GPU renderer

Status: design, and stage 1 in part (below, "Stage 1 as built"). The goal
is every frame drawn from GPU-resident data, with the game's recompiled
render front-end left doing only what the GPU path cannot do yet. Evidence
labels as in [feature-engine.md](../engine/feature-engine.md): **Code**
(read in the disassembly, `h2 disasm`), **Test**, **S** (supposed). No
game data: addresses and offsets are research points, everything else is
read from the user's disc at run time.

## Why a new shape

Today a frame on Old Mombasa costs about 25 ms of game-thread time in the
render front-end ([render-loop.md](render-loop.md), "Where the scene
passes' time goes"): the opaque layer 3 alone is 8.26 ms, nearly all of it
per-draw guest work in the shader draw `0x00016b90` (parameter evaluation,
render-state lists, texture binding) before a D3D call is made. Those
calls become push-buffer words that `d3d` decodes and translates back into
wgpu draws on the render thread (decode, then about 6 to 8 ms of encode
and 4 ms of submit).

Stage 7a ported the orchestrators (`render_views`, `render_scene_passes`,
the layer loop, the sorts) and saved nothing: it moved code, not work.
Every Rust port that still calls the pass bodies pays a runner round trip
per call ([layer-draw.md](layer-draw.md), "Hooked calls leave the direct
path"). The native renderer does the opposite: it takes the *data* the
game draws from (tags, at map load) and the *camera* (per frame), draws
it with its own GPU code, and has the game skip the subtree that would
have drawn it. A skipped subtree costs nothing on the game thread, in
`d3d` decode, or in encode.

## Stage plan

| Stage | Draws natively | What the game skips (CPU removed) | Shared with the D3D path |
|---|---|---|---|
| 1. Static world | Structure BSP clusters and instanced geometry, layer 3 (lightmap) | Layer 3's structure entries: their builds, shader draws (`0x0001bbf0`/`0x00016b90`), geometry submission (`0x00041040`'s callbacks `0x0004cbf0`/`0x0004d0b0`) and the push-buffer words; most of the 8.26 ms layer-3 site, the matching `d3d` decode, encode and submit | Colour target and depth/stencil, viewport, the camera |
| 1b. World, all layers | The world's layers 1 (base depth / texaccum), 5, 7 (self-illumination) and 8 (environment map) | The rest of the world's entries in the base layers | Texaccum target (when kept), depth |
| 2. Objects and skinning | Render models: rigid and skinned sections, permutations, LODs; node matrices from the game per frame | `0x00133790`'s render records for drawn objects, their layer entries in every base layer | Node matrices (read only), object list (read only) |
| 3. Transparents and particles | Sorted transparents (`0x0001e370`, 2.58 ms), particle systems, decals, contrails | The transparent list and its per-entry handlers; particle update stays in the simulation | Depth (read), sorted order computed on the GPU or on our side |
| 4. Lights and shadows | Dynamic lights (layers 10 to 14, `0x00031c80`, 1.88 ms of bookkeeping), shadow maps; E3 lighting becomes a pass ([e3-lighting.md](e3-lighting.md)) | The per-light passes and their gathering | Light list (read) |
| 5. Post and HUD | Fog, bloom, active camouflage, screen effects, HUD and text, then the present | `render_screen_effects` (`0x00013870`: the colour pass, screen effects, `0x0002df00`), `0x0002dd30`, layer 17, the fade `0x00040890`, the UI and HUD `0x00147af1` (see "Stage 5: post and HUD") | Nothing left: the D3D path is only a fallback |

Each stage is a feature switch under `impl.render.native_*`, off by
default until its A/B passes, and each falls back per material (or per
draw) to the D3D path for anything it cannot draw yet. The visibility
work (`0x00040e90`, 0.91 ms) and the object setup are removed in stages 2
and 3, once nothing on the D3D path reads their records.

## Stage 1: the static world

### Load once

At map load (and on a BSP switch: the active BSP is `0x004e0348`, the
lightmap `0x004e0344`, [structure-bsp.md](../formats/structure-bsp.md)):

- **Geometry.** Every cluster section and every instanced-geometry
  placement (`tags::structure::StructureRender`), decoded by
  `tags::geometry` exactly as the viewer and editor already do. One shared
  vertex buffer (position, base-map UV, lightmap UV, normal) and one
  index buffer. Instances are placed at load (rigid transform + uniform
  scale) so every draw is in world space; the static world never moves.
- **Draws.** One draw per drawn part (`Part::is_drawn`, not kind 4
  transparent): `(cluster or instance, material, lightmap page and
  palette, index range, world bounds)`. Draws are sorted by batch
  (material, lightmap page) at load, so a batch is a contiguous slot range
  of the indirect buffer.
- **Lightmaps.** The `ltmp` group whose checksum matches the BSP's; each
  cluster's and instance's render info picks a P8 page and palette
  ([lighting.md](../formats/lighting.md)). A page is expanded through its
  palette *before* filtering (the palette is an index lookup, not a
  multiplier), into an RGBA8 texture array, one layer per (page, palette)
  pair used.
- **Materials.** The part's material indexes `sbsp+0xa4` (stride `0x20`,
  shader at `+0xc`). Each shader's template and bitmaps decide the
  native program (below). A material the native path does not support is
  marked *fallback* and keeps its D3D draws.
- **Pipelines** are made once per supported template.

### Per frame

- **Camera.** The render camera record `0x004b9da0` (0x74 bytes, filled by
  `0x0002f800` per view): position `+0`, forward `+0xc`, up `+0x18`,
  vertical FOV `+0x28`, viewport `+0x30` (i16 top, left, bottom, right),
  near `+0x40`, far `+0x44` ([occlusion-culling.md](occlusion-culling.md),
  "Camera and view"). From it, a view matrix and a projection with the
  game's own aspect rule (`0x0002fb70`). Read once per view, at the layer-3
  hook (below), so it is the camera the game's own draws would use.
- **Cull.** Frustum planes from the view-projection, tested against each
  draw's world bounds on the CPU (a few thousand AABBs: microseconds). The
  result is written as `instance_count` 0 or 1 into the indirect buffer
  (one 20-byte slot per draw), which is the only per-frame upload besides
  one uniform block. A compute shader can take this over unchanged (same
  buffer, same slots); the cluster visibility from the game's portal flood
  (`0x00132220`'s visible-cluster list) can be ANDed in later, but the
  frustum alone is conservative: it draws a superset, and depth sorts out
  the rest.
- **Draw.** Per batch: bind the batch's textures, then
  `multi_draw_indexed_indirect` over its slot range. One render pass into
  the game's own colour target and depth, with the game's viewport.

### Skip only the replaced work (Code)

The scene list `0x004c1a48` (layer lists at `0x004c1a48 + 0x18·n`) holds
items of `0x18` bytes at `[list+0x14]`, appended by `0x0003b9d0`:

| Item | Meaning |
|---|---|
| `+0x00` | build function (called by `0x0003bab0` per layer) |
| `+0x04` | draw function (called by `0x0003bc30` per entry) |
| `+0x08` | shader datum |
| `+0x0c` | argument: render record index `<< 8`, low byte a part index |
| `+0x10` | layer mask (bit `n`: drawn in layer `n`); the list's `+8` is the OR of all |
| `+0x14`, `+0x15` | bytes passed to both functions |

Structure and model geometry are queued by `0x00041040` through
`0x00040e30` with build `0x0004cbf0` and draw `0x0004d0b0` (pushed at
`0x000412fa`/`0x0004132b`; the transparent path instead records
`0x00041020`). The render record is `0x004ba138 + 0x20·index`
(`0x0002cbf0` fills it); its `+4` is the object handle, `-1` for the BSP:
the cluster records (`0x00133390`, call at `0x00133504`) and the
instanced-geometry records (`0x00133520`, call at `0x001336d3`) both pass
`-1`, and the build function itself tests `[record+4] != -1`
(`0x0004cc2c`).

So a structure item in layer 3 is: build `0x0004cbf0`, `[0x004ba138 +
0x20·(item[+0xc] >> 8) + 4] == -1`, mask bit 3 set. The skip is one hook
per layer draw, not one per item:

- **before** the layer draw `0x00044550` with the current layer
  `[0x00467130] == 3`: walk the current list (`[0x00467138]`; `+4`
  count, `+0xc` first, `+0x14` items), clear bit 3 of every structure
  item's mask, remember them, and ask `d3d` for the native pass. The
  list's OR word `+8` is left alone: the layer still walks its items and
  finds the world's masks clear;
- **before the next layer draw** (5 and up follow 3 on the scene list,
  in the same view): set the bits back, on each item that still holds
  the mask the skip left. Not after the layer itself: something after the
  body makes the chain call it through a Rust continuation, which the
  pipelined render thread cannot run (docs/graphics/pipelined-render.md,
  "Render-safe hooks").

The layer draw then builds no entry for the world: no build call, no
shader draw, no geometry submission, no push-buffer words. Objects,
transparents, lights and the HUD keep their entries. Guest memory after
the layer is as without the switch except for what the skipped draws
would have written (the shader draw's state cache, D3D's shadows), which
is render state only; the simulation never reads it, and co-op lockstep
hashes nothing of it. The native renderer itself never writes guest
memory.

The structure draw also sets stencil mode 1 in layers 1 and 3
(`0x0004d720` -> `0x00015370`), and mode 2 for sections with flag `0x4000`
([e3-lighting.md](e3-lighting.md)). What reads those marks is not traced;
the native pass writes the same stencil value as a first approximation
(S), and the A/B shows whether anything depends on it.

### Where the native pass goes, and depth

The ask is recorded in order with the draws, like the occlusion depth
grid (`d3d::Device::request_depth_grid`): the device issues it at the next
drain of the push buffer, which is the first D3D entry inside the layer
draw, so it lands after layer 1's draws and before layer 3's object
draws. At that moment the device knows the bound colour surface, depth
surface and viewport; the backend gets `(colour, depth, viewport, pass)`
and runs the pass's encoder on the render thread with the wgpu views of
those surfaces (`Backend::external_pass`). With the render scale on it
runs in the scaled view; with E1 reprojection the interpolating backend
must record it with the frame so a replay redraws it with the warped
camera (S: not built; see "Open").

Depth must agree with what the D3D path writes, because later passes
depth-test against it (layers 5, 7, 8 on the same surfaces, objects,
transparents). The game W-buffers: the stored depth is `floor(w · zs /
far) / zs`, with `zs` the D24 full scale and `far` the camera's far plane
([occlusion-culling.md](occlusion-culling.md), "The W scale"). The
translated pixel programs write exactly that as `frag_depth`
(`shaders::psh::wgsl`, `xps_w_depth`), and so does the native fragment
shader, from the same perspective-correct `1 / position.w`. A world draw
whose layer-1 pass already wrote depth is tested `LessEqual` against it;
one without writes it. Equal-depth passes the D3D path still draws over
the world (layers 7 and 8 in stage 1) only line up when the native clip
position equals the translated vertex program's bit for bit; stage 1b
removes that dependency by drawing those layers natively too.

### Match the look

The world's layer-3 passes are lightmap passes: the material's base
colour (base map and detail maps, by template) times the lightmap
irradiance, with fog applied later by layer 16. Two ways to match them:

1. **Translate the material's own programs.** The pass's vertex program
   (`vrtx`) and pixel shader definition (`spas`) are what `shaders`
   already translates for the D3D path; the native renderer can compile
   them once per material with the vertex constants it computes itself
   (the camera, the material's parameters at their rest values). Exact by
   construction, as far as the constants are.
2. **WGSL per template.** One program per template the world uses,
   written from the template's passes. Readable and cheaper, but every
   template is a new matching job.

Stage 1 starts with (2) for the plain lightmapped template (base map ×
lightmap, the shape `scenario-edit`'s viewport already draws) to bring up
loading, culling, the hook and the A/B; then moves the 03a templates to
(1), template by template, behind the per-material fallback. A material
is native only once its A/B is clean.

### How correctness is checked

- **Unit tests** (no disc): world building from invented sections (draw
  ranges, batches, instance placement, bounds), the camera read from an
  invented guest record, frustum culling against known boxes, the scene
  list walk and restore on invented guest memory.
- **GPU tests** (headless wgpu, skip without an adapter): a native draw
  of an invented world into an offscreen target; its W depth equals the
  formula.
- **Frame A/B** (the game): the same 03a frame with and without
  `impl.render.native_world`, compared by pixel diff, through the headless
  capture (`H2_BOOT_GFX`) or the agent API's `capture`. Report the share of
  differing pixels, the largest difference, and an image of the
  differences. Frame stats `cpu`, `d3d`, `encode`, `submit` and the
  `render-timing` site `0x0002bed0` (layer 3) give the CPU saved.

## Stage 1 as built

The crate `native-render` (`crates/native-render`):

- `world`: one vertex and index buffer for the whole BSP, one draw per
  opaque part in world space (instances placed at load), sorted into
  batches of one material and one lightmap page; transparent parts and
  bad triangles counted and left out (Test).
- `load`: the scenario's (BSP, lightmap) pairs, the BSP's material
  shaders, every cluster and placed instance through `tags::structure`,
  each with its lightmap page and palette from the group whose checksum
  matches; P8 pages expanded through their palette (Test for the
  expansion and the render-info rules; the map read needs the disc).
- `camera`: the render camera record read from guest memory (rejects
  NaN, empty viewports, `far <= near`), the view, the projection by the
  game's aspect rule, and the W-buffer depth formula (Test).
- `cull`: frustum planes from the view-projection, the conservative box
  test, one indirect slot per draw whose `instance_count` a frame flips
  (Test).
- `skip`: the current list's walk, the structure-item test, clear and
  restore (Test, on invented guest memory).
- `gpu`: buffers and bind groups made once, one pipeline per (colour,
  depth) format pair, `prepare` (cull, uniform, slots) and `encode` (one
  pass that loads and keeps the game's target and depth, one
  `multi_draw_indexed_indirect` per batch) (GPU test:
  `tests/gpu.rs` draws an invented wall at view depth 10 and a culled one
  behind the camera; the colour is base × page and the depth is
  `w_depth(10, far, D24)` within one step).

### Wired into the game

- **`d3d` external pass** (`crates/d3d`). `Device::request_external_pass`
  holds a boxed `ExternalPass` and issues it at the next push-buffer drain
  (the next D3D entry), with the bound colour target, the bound depth and
  the current viewport, like `request_depth_grid`; `can_external_pass`
  asks the backend first. `GfxBackend::external_pass` encodes the pending
  pass, then hands the pass the open encoder, single-mip views of the two
  surfaces, their formats, the viewport in texels and the device's depth
  full scale (`+0x948`). `ThreadedBackend` posts it to the render thread;
  `ScaledBackend` runs it into both copies (the 1x reference and the
  scaled view: `encode` is called once per copy); `InterpBackend` (E1)
  forwards it and holds the frame (`unpinned`), since a replay redraws
  draws, not passes. `DiagBackend` and boot's headless backend forward it.
  Counters: `external_passes`, `external_passes_dropped`. Test:
  `crates/d3d/tests/external_pass.rs` (direct, render thread, E1, and a
  backend that cannot).
- **The feature `native-world`** (`crates/feature/src/native_world.rs`,
  on by default, `H2_FEATURES=impl.render.native_world=off` turns it off,
  exact key). `LAYER_DRAW` is an opaque point, so the hook is one
  observer before it: it first sets back the bits the last layer 3
  cleared (on items that still hold the mask it left), then, with layer 3
  current, reads the camera and the active BSP (`0x004e0348`) and asks the
  host; when the host accepts it clears layer 3's bit on the BSP items
  whose shader the world draws. It sits inside `render-timing`'s
  observers (`after_feature`). One observer call per layer draw, nothing
  per draw.
  It runs on the pipelined render thread (context-only, nothing after the
  body), so pipelining stays available with the world on. What it
  touches there: the guest memory its context sees (the render's view:
  the item masks, the camera, the active BSP, the map name); the host's
  load state (a mutex only `ask` takes, and only the render reaches it;
  the loader is a host thread that opens the map through the VFS's
  mutex and never touches guest memory); the D3D device's pending
  external pass, which the render owns while it is in flight (the sim's
  D3D calls join first) and which is issued at the render's next D3D
  entry, in render order, with that entry's memory.
  Tests: `crates/feature/tests/native_world.rs` (the plan with
  `render-timing`, deep, and the layer-draw port all on; alone, the chain
  runs on the render thread; on by default with pipelining placed; the
  switch off places nothing; the skip and its restore at the next layer
  draw on synthetic memory, and a rebuilt list left alone);
  `crates/boot/src/render_thread.rs` (the default plan's `LAYER_DRAW`
  is render-thread safe).
- **Boot glue** (`crates/boot/src/native_world_glue.rs`). The host: on a
  new (map name, active BSP address) a worker opens the loaded map
  through the VFS with its shared maps, picks the scenario BSP whose tag
  data sits at that address, builds the world, decodes each material's
  base map and each lightmap page, and drops the draws of materials whose
  base map does not decode (they stay on the D3D path). Each layer-3 view
  then asks the device for a `WorldPass` with the view's camera; the first
  pass uploads the world on the render thread. A line per load:
  `native-world: <map> BSP at <addr>: N draws in B batches, M materials
  native, in S s`; every 1,800 layer-3 views: `native-world: D of V
  layer-3 views drawn natively, I world items skipped`.

### The live A/B

The same 03a frame with and without the switch, E1 off, frame stats on.
What passes: the `native-world:` load line names `03a_oldmombasa`; the
views line shows every layer-3 view drawn natively; the frame shows the
world (no holes where the BSP was, objects and transparents in front of
and behind it as before); `cpu` and the `0x0002bed0` site (layer 3) drop
by most of the world's share of the 8.26 ms, and `encode` and `submit`
drop with the D3D draws that are gone. The pixel diff is expected to be
large on world surfaces until step 4 (base map times lightmap only, no
detail maps or bump, no fog-free exposure match; base maps are mipmapped
since step 4) and small elsewhere.
A frame where the device's `external_passes_dropped` grows has a world
hole: it means the pass was asked for and could not be drawn.

### Next: matching the look

The base map per material comes from a name rule today; step 4 moves the
03a templates to their own translated programs (above, "Match the look"),
each behind the per-material fallback, with mipmaps and the template's
detail maps and scales. No hand-written per-template WGSL in between: it
would be replaced by the translated programs.

#### What a world pass reads (Code, `h2 disasm`, `h2 shader`, `vsh` disassembly)

A world material's layer-3 pass, as `tags::material` resolves it: on 03a
the opaque world templates are `tex_bump` and its variants (`_env`,
`_detail_blend`, `_three_detail_blend`, `_two_detail`, ...). A `tex_bump`
layer 3 is one pass (`lightmap_tex_detail`) with three implementations:
per-pixel lightmap (stage 3 the lightmap), per-vertex, and dynamic. The
engine picks the implementation per draw; the BSP's lightmapped sections
take the per-pixel one (S, from the lightmap bind at `0x0004d40f`).

The pass's vertex program comes from its `vrtx` tag's geometry
classification. Classification 0 is world space (S: its program reads
`v0` as a world position and applies no model transform). For the
per-pixel `tex_detail` program it reads:

| Input | Meaning | Our source |
|---|---|---|
| `v0` | world position | section positions |
| `v1` | texture coordinate | section texcoords |
| `v2`, `v3`, `v4` | the tangent frame (normal, binormal, tangent) | section tangent-frame stream |
| `v5` | the lightmap bucket's signed lighting vector (stream `30`, [lighting.md](../formats/lighting.md)) | bucket stream, to decode |
| `v6` | lightmap coordinate | bucket stream `1f` |

and these constants (hardware numbering, D3D register + 96):

| Constants | What | Set by | For the native pass |
|---|---|---|---|
| `c[0..3]` | view-projection rows (`dph` with `v0`) | per view, in one block upload | the device's values at the pass (below) |
| `c[5].w`, `c[6]` | the scale that packs a unit vector into a colour (S: `0.5`, `n·s + s`); a world-space vector (dotted with the tangent frame into `oD0`) | per view | the device's values |
| `c[10]`, `c[11]` | the viewport scale and offset (`oPos` fix-up) | per view | the device's values |
| `c[12..17]` | six registers in one block | `0x00025c70` | the device's values |
| `c[18..20]` | stage 0..2 texture-coordinate scale (xy) and offset (zw) | the instance (`0x00018e80`) | `tags::material`'s instance constants |
| `c[50..52]` | model transform rows | `0x0004d830` from the render record's matrix, skipped for clusters (`[record+8] == -1`) | identity for clusters; the instance's matrix for instanced geometry |
| `c[170..172]` | position and texcoord decompression (scale, offset) | `0x0001cdd0`; `0x0004d7c0` passes no compression for the BSP, so the default upload (`0x0001cf29`, scale 1, offset 0) | `(1, 1, 0, 0)`: BSP vertices are uncompressed |

Where `v5` and `v6` come from depends on the render record
(`0x004ba138 + 0x20·i`). `0x0002cbf0` packs its `+0xc` as bits 0..3 an
argument, 4..8 an index into the `0x24`-byte table at `0x004c152c` (`0x1f` none), 9..17 the cluster index, 18..28 a placement
index; record `+0` bit 12 says which (the structure draw's choice at
`0x0004d18b`). The cluster records (`0x00133390`) pass `-1` for the
placement, so bits 18..28 are `0x7ff`. The structure draw's layer-3 part
(`0x0004d285`..`0x0004d420`) then does:

- **A cluster** (bits 18..28 `0x7ff`, `[record+4] == -1`): no bucket. The
  section's own streams carry `1f` and `30` (structure-bsp.md: `30` is
  present exactly when the section's lighting flag bit 1 is set), and the
  cluster render info (group `+0x2c`) at the cluster index picks the
  lightmap page and palette.
- **An instanced placement** (`[record+4] == -1`, bits 18..28 the
  placement): `instance_bucket_refs[placement]` (group `+0x54`), its first
  offset times 4 bytes written into the bucket's first descriptor when
  bucket flag bit 0 is set (`0x0004d3a1`), then `0x0004dc60` binds the
  bucket's streams; page and palette from `instance_render_info[placement]`
  (group `+0x4c`). The definition's section is shared by every placement,
  so its lightmap coordinates and lighting vectors are the placement's,
  in the bucket.
- **An object** (`[record+4] != -1`, bits 18..28 not `0x7ff`):
  `cluster_bucket_refs[bits 18..28]` (group `+0x64`; despite the name, the
  engine reaches it only for object records here), the offset at entry
  bits 9..17. Not the world's concern.

Stage 1 took every section's own `lightmap_texcoords`. That is right for
clusters and wrong for placements: a definition has no `1f` of its own,
so every instanced placement lost its lightmap (the builder leaves a
section without coordinates unlit). Fixed: placements read their run of
the bucket (`load::instance_run`, `load::bucket_slice`); the lighting
vectors (`30`) the same way (S: the bucket's `30` stream is indexed like
its `1f`).

The per-view constants (`c[0..17]`) are the same for every draw of a
view, and nothing in layer 3 writes them per draw (the per-draw uploads
found are `c[18..]`, `c[21]`, `c[22]`, `c[24]`, `c[25]`, `c[31]`,
`c[50..52]`, `c[170..172]`, `c[174]`). So the native pass reads them
from the device's constant state when the pass is issued (after the
drain, so the view's own upload is in): exact by construction, and no
camera maths of ours to keep in step with the game's.

The pixel side is the implementation's final pixel shader definition
(variants built, instance colours patched), translated by `shaders::psh`.
For `lightmap_tex_detail` it combines base (stage 0) and detail (stage 1,
`x2`), the bump map (stage 2) dotted with the tangent-space lighting
vector (`v1`) and view vector (`v0`), and the lightmap (stage 3). Its
`c0` constants of the specular stages (4 and 5) are marked engine-driven
(`h2 shader` prints "engine constants", mask `0x30`), traced (Code,
`h2 disasm`, the 03a pass's bindings read off the disc):

- The per-pixel implementation has one spas `+0x38` binding through
  `0x00019ca0`: mode 3 (the alpha channel), no transform, engine scalar
  21. Its input masks (spas `+0x104..+0x114`, the inputs `0x0001ae70`
  evaluates before the draw, bit `n` input `n`) are scalar 21 and integer
  5 (the fragment selector), nothing else.
- Scalar 21 is `0x000337e0`, which returns `0x485a6c`; integers 4 and 5
  are `0x485a64/0x485a68`. `0x0001bbf0` copies all three from the `shad`
  (`+0x44`, `+0x3e`, `+0x40`) before each draw. So the "engine" constant
  is the instance's own specular scalar, the same on every draw of the
  shader, and `tags::material` already resolves it into the pixel shader
  definition (`EngineInputs::for_shader`). The stage-5 `c0.rgb` is an
  instance colour.
- `ResolvedImplementation::per_draw_inputs` says when an implementation
  reads anything else (another scalar, any vector, another integer, by
  mask, binding or fragment selector); only those stay on the fallback.
  Of the pass's other implementations the per-vertex one has the same
  masks; the dynamic one reads engine vectors (lighting), but the world
  pass uses the per-pixel one.

#### Order of work

1. `d3d`: the external pass gets the device's vertex constants at issue
   (a read-only snapshot, nothing else changes for other backends).
2. `native-render::load`: per material, the resolved layer-3 pass and
   implementation 0: the classification-0 program, the final pixel shader
   definition, stage textures (instance bitmaps with their image, the
   lightmap), sampler states and instance constants. Materials without
   one (no layer 3, more than one pass, an engine texture source other
   than the lightmap, per-draw engine inputs) stay on the D3D path.
3. World vertices gain the tangent frame and the bucket's lighting vector;
   textures keep their mip chains (done: base maps upload their decoded
   chain, `gpu::Image::from_levels`).
4. `native-render::gpu`: one pipeline per (program, definition) through
   `shaders::link`, the uniforms filled as the D3D path fills them, one
   bind group per material. Built as `native-render::program` behind
   `impl.render.native_world_programs` (off by default; exact key, and
   only with the native world on):
   - `program::compile` translates the pass (inputs `v0..v6` from the
     world vertex: position, uv, normal, binormal, tangent, lighting
     vector, lightmap uv) and fills the instance constants by their
     written components, `c[172] = (1, 1, 0, 0)` and identity model rows
     `c[50..52]` (the vertices are already placed). It refuses, and the
     material keeps stage 1's look: per-draw engine inputs, a program
     reading other inputs (but those the pass sets itself with
     `SET_VERTEX_DATA4F_M`, `0x00018e3b`: the resolved
     `vertex_defaults`, read from `xvs.defaults[n]`; 03a's transparents
     read `v10` and `v11`, two colours, mask `0x0c00`), a translation
     warning, a non-2D fetch, a
     sampled stage with nothing bound, signed channels on the lightmap.
     Signed channels of a stage bitmap are stored biased (`s + 128`) as
     the D3D path stores them.
   - Per frame the device's `c[0..18]` (the external pass's
     `view_constants`), the guest and texel sizes (surface and the
     render-scale shift) and the W-buffer's depth scale (`w_clip`,
     `depth`, `depth_range`). A program batch draws over the whole
     target: the game's viewport is in `c[10..11]`.
   - Gaps (S): fog stays at the pixel shader's default (no fog colour
     from the device yet), stage borders clamp to the edge, `D3DTSS`
     LOD bias and anisotropy are not applied.
   The load line says how many materials translate and why the rest do
   not (`native-world: N materials translate; with stage 1: [...]`).
5. The A/B per template, and `native-world` keeps a material native only
   once its template's A/B is clean.

## Stage 2: objects and skinning (design)

Bipeds, vehicles, weapons, equipment, scenery, machines and crates: every
object whose render model the game queues in the scene list. Geometry is
resident per render-model tag, loaded once per map; each frame the CPU
hands over only the drawn objects' node matrices, read from the game's own
computed transforms in guest memory. Skinning runs in the vertex shader.

### Research points (Code unless marked; `h2 disasm`)

| Point | What |
|---|---|
| `0x004e0300` (`points::OBJECTS`) | The object datum array header `H`. Header of handle `h`: `[H+0x44] + 0xc·(h & 0xffff)`; salt `u16 +0` must equal `h >> 16`; body `[header+8]` ([live-gamestate.md](../live/live-gamestate.md)). The pool compacts, so a body address is read every frame, never kept. |
| body `+0x00` | The object definition tag datum (`bipd`, `vehi`, `weap`, `scen`, ...). |
| body `+0x30`, `+0x3c` | World bounding sphere: centre (3 floats), radius ([occlusion-culling.md](occlusion-culling.md)). |
| body `+0x114` (u16), `+0x116` (i16) | **The node-matrix block**: its size in bytes and its offset from the body. `object_new` `0x000b7b40` reserves it at `0x000b800b..0x000b801e` with `0x000bc380(field 0x114, 0x34 · nodes, align 0)`, the node count from hlmt `+0x78` (at least one). `0x000bc380` writes a block reference as `+0` size and `+2` offset (`0x000bc427`, `0x000bc430`). The other blocks it places: `+0x10c` and `+0x110` (32 bytes per node, when animated), `+0x118` (10 bytes per region), `+0x11c` (8 per attachment), `+0x120`, `+0x124` (`0x18` per change colour), `+0x128`. |
| node matrix (`0x34` bytes) | The game's `real_matrix4x3`: `+0` scale, `+4` forward, `+0x10` left, `+0x1c` up, `+0x28` position. A local point `p` goes to `scale · (p.x·forward + p.y·left + p.z·up) + position`: the bounding-sphere update `0x000bdf90..0x000be068` applies the root node exactly so, reading the block through `[body+0x116]`. The matrices are world space (the sphere it produces is the world sphere at `+0x30`). |
| mode node `+0x28` (`0x34` bytes) | The node's inverse rest frame in the same layout (scale, then the inverse's three columns, then its position; `crates/h1models/src/tests.rs`, `node_inverse_frames_are_stored_as_columns`, and the retail check `retail_h2_mode_layouts`). A retail hlmt node is its mode node's first `0x5c` bytes (V, `crates/tags/src/defs/hlmt.txt`), so object node `i` is mode node `i`. |
| skinning | Vertex world position `Σ wₖ · N[nₖ] · I[nₖ] · p`, with `N` the object's node matrix, `I` the mode node's inverse rest frame, `nₖ` the section's node map applied to the vertex's node index (S: that the game composes the same `N·I` for its node constants; "rest frame times inverse is the identity" is Test in h1models, the draw side is checked by the A/B). Sections without vertex nodes (classification 1) are bound to the first node of their node map, else node 0 (S). |
| `0x0002cbf0` | Fills render record `0x004ba138 + 0x20·i` (at most `0x352` per frame, counter `0x004c0b78`): `+4` the object handle (`-1` for the BSP), `+8` a word from the caller's per-section state, `+0`, `+0xc` and `+0x1a..+0x1b` bit-packed. Its caller for objects is `0x00133f70`, which walks a per-object render state of about `0x1e4` bytes built under `0x00133ab0` (entries from `+0x48`, stride `0x18`, count `+0x166`; handle `[entry+0x44]`). Which bits are the section (LOD and permutation) is not read yet. |
| `0x0002bed5` | The return address of the base layers' layer-3 call (`call 0x00044550` at `0x0002bed0` in `render_scene_passes`, view kinds 1 and 2). The object skip acts only there, so first-person or other-kind layer draws are never touched. |
| `0x0004baf0`, `0x0004b5d0` | LOD choice by pixel size inside `0x00133790` ([occlusion-culling.md](occlusion-culling.md)). |
| `0x0016d764` | Region permutation choice (Likely): the variant's region map, its regions at `+0x18`, then `0x0016dad0` ([objects.md](../formats/objects.md)). |
| `0x00165f16` (`points::FIRST_PERSON_RENDER`) | One local player's first-person weapon and arms, after the transparents, with their own projection (camera `0x0023c110`, FOV `0x00187130`). |
| `0x0003ddd0`, `[0x00509434]`, body `+0xcc` | The per-object render-state (lighting) cache: records of `0x100` bytes, lit per frame by `0x0003dfe0`, lighting at `+0x44..` and `+0xc0..` (layout not read; [pipelined-render.md](pipelined-render.md)). |

### Skip point

The same layer-draw observers as the world (`LAYER_DRAW`, an opaque
point), under the same plan rules: **before** the layer draw, when the
current layer is 3 and the caller is the base layers' call (`[esp] ==
0x0002bed5`), walk the current list; an item is an object item when its
build is `0x0004cbf0` and its render record's `+4` is a live object
handle. It is skipped when the host draws that object's definition and
the item's shader (`+8`) is one of the host's native materials: layer 3's
bit cleared, the item remembered, the handle added to the view's drawn
set (once). The bits go back **before the next layer draw** (5 and up
follow 3 in the same view), on items that still hold the mask the skip
left, as the world's do. The host gets one
external pass per view carrying, per drawn handle, the definition and the
node matrices copied out of the body (`0x34 · count` bytes) and the
bounding sphere.

Only layer 3 in the first cut, as in stage 1: the objects' layer 1, 5, 7,
8 and 20 entries and their dynamic-light layers (10 to 13) keep drawing
through D3D, so self-illumination, environment maps, specular, change
colour and dynamic lights still come from the game's own passes on top of
the native base. Those passes depth-test `LessEqual` against the depth the
native pass wrote; skinned on the GPU in float, the native positions match
the translated vertex programs to within an ulp or two, not bit for bit,
so an equal-depth pass can lose pixels on curved surfaces (the A/B shows
how much). The skipped layers are one mask
(`native_render::objects::REPLACED_LAYERS`); adding layers later is a mask
change plus the shading those layers did.

What the native pass draws is exactly the set of objects whose layer-3
items it skipped. The game's own visibility, object list, attachment
recursion and "not drawn" decisions (`0x00133790`, occlusion) have already
run, so a held weapon, a vehicle's driver or a culled crate need no logic
of ours. A conservative sphere-frustum test runs anyway (cheap, and a
guard against a stale sphere).

### Transform source

Node matrices are read once per view at the hook, from the body
(`[body+0x116]`, `[body+0x114] / 0x34` nodes), after the game has finished
the frame's animation and attachment updates (the records exist, so
`0x00133ab0`'s node pass ran). They are uploaded raw (13 floats a node)
into one storage buffer per pass, with one small instance record per
object (node base, inverse base). The inverse rest frames are resident
per model. No matrix product is done on the CPU. An object whose node
count disagrees with its model's stays on the D3D path.

### LODs

The game picks a LOD per object by pixel size; the native path draws the
highest LOD present in each permutation (`Permutation::best_section`).
Distant objects then cost more triangles than the game would draw: GPU
work, never a missing surface. Matching the game's LOD needs the record
bits above (the section in the record), a later step; the record would
then also pin the permutation.

### Permutations and regions

The variant is the object definition's default (`0x0016d180`'s rule,
`tags::object::Context::resolve_model`); its regions map to the render
model's regions by name, and each region's permutation to the render
model's by name. A model is drawn natively only when every region the
variant lists has **exactly one** permutation. Models with a random or
state-driven choice (marine heads, damage states, a variant changed at run
time through `+0xb1`) stay on the D3D path until the record's section is
read. Regions the variant does not list draw their first permutation, as
the viewer does (S: some may be hidden in game).

### First person versus third person

The first-person weapon and arms are drawn by the player chain after the
transparents (`0x0002afa0`, `FIRST_PERSON_RENDER`) with their own
projection, not by the base layers' layer-3 call, which the caller check
pins. They stay on the D3D path in stage 2. Natively they need the
first-person camera (`0x0023c110`) and their nodes (the first-person model
references nodes past its own list; `tags::model` leaves those indices
unchecked), and they draw after the transparents, so they come with or
after stage 3. The local player's third-person body draws natively like
any biped when the game queues it.

### Shading

Like the world's first cut: the material's base map (the same name rule,
`load::base_map`) times a lighting term. The game's per-object lighting
lives in the render-state cache (`0x0003ddd0`, body `+0xcc`, lighting at
record `+0x44..`), whose layout is not read; until it is, the term is a
fixed ambient plus one directional light (`ambient + diffuse · max(n·l,
0)`), so objects read as lit solids, not as their final look. Reading the
cache's lighting (ambient, the dominant light's direction and colour, the
probe's spherical harmonics, [lighting.md](../formats/lighting.md)) is the
first look step. Materials without a decodable base map keep their D3D
draws (the skip leaves their items alone) and are not drawn natively.
Transparent parts (kind 4) are not in layer 3 and are left out.

### Pipelined render

The observer reads the object array and bodies during the render. With
`H2_PIPELINE_RENDER` the render runs one frame behind the simulation on
its own thread, while the next tick moves the same bodies and may compact
the pool. That is already safe: on the render thread the observer's
memory is the render's copy-on-write snapshot of the frame
(docs/runtime/second-executor.md; `Obs::guest` is the executor's own
runtime), so the handles, bodies and node matrices it reads are frame
`k`'s, the same frame the rest of the render draws, and the tick's writes
to live RAM never reach it. The node matrices are copied out of that
snapshot at the hook into the pass (`ObjectFrame`), and nothing reads
guest memory after it, so the snapshot copy the design asked for is the
pipeline's own snapshot plus that copy. The observer is the one hook
(context-only, nothing after the body, the restore at the next layer
draw), so the pipelined render runs it on its own thread like the
world's. The host's writes are its map state (a mutex only the observer
takes, with the model loader on host threads) and the device's pending
external pass, issued at the render's next D3D entry, in render order.

### Per frame on the CPU

Per view: one walk of layer 3's list, one object lookup per drawn object,
a copy of its nodes (a biped has about 50: 2.6 KB) and one sphere test.
The render thread writes two storage buffers and issues one indexed draw
per (object, opaque part), grouped by model and material. Nothing per
vertex and no matrix work on the CPU.

## Stage 2 as built

Not compiled or run by the lane that wrote it (the coordinator builds and
tests); evidence labels below are what the tests will show once run.

- **`native-render`.** `model` (the game's node matrix, `Model::build`:
  one vertex and index buffer per model in rest space, vertex node indices
  through the section's node map, one draw per opaque part sorted by
  material, the inverse rest frames; `Model::skin`, the shader's
  arithmetic on the CPU), `model_load` (definition -> `hlmt` -> `mode`,
  the section pick of "Permutations and regions", the inverse frames from
  node `+0x28`), `objects` (the handle lookup, the node block at body
  `+0x114`/`+0x116`, the sphere; `clear_objects`, `keep_only`, `restore`
  on the base layers' layer 3), `instances` (per view: the node array,
  one instance record per object, a draw per (object, part) sorted by
  model and material, the sphere-frustum cull, the node-count guard),
  `model_gpu` (`GpuModels`: models added one at a time, per view one
  uniform and two storage writes, one pass with an indexed draw per
  (object, part), `instance_index` the object) and `models.wgsl`
  (skinning `Σ w · N · I · p`, base map times ambient plus one
  directional term, the W depth). Tests: the unit tests in each module on
  invented models and synthetic guest memory; `tests/models_gpu.rs` draws
  an invented two-node model whose square hangs on a turned node, checks
  colour (lit by the skinned normal), W depth, one object culled and one
  with the wrong node count left out.
- **The feature `native-objects`** (`crates/feature/src/native_objects.rs`,
  `H2_FEATURES=impl.render.native_objects=on`, off by default, exact key).
  One observer before `LAYER_DRAW`, inside `render-timing`'s and the
  world's (`after_feature` on both), context-only. First it sets back the
  bits the last base-layer layer 3 cleared (items still holding the mask
  it left). Then, only for layer 3 called from `0x0002bed5`: asks the host
  for its native set (definitions with their node counts, shaders),
  clears the accepted items, asks the host to load any definition it has
  not seen, copies the cleared objects' nodes and spheres, gives back the
  bits of objects it could not read or whose node count disagrees, and
  asks for the pass (all bits back if the device cannot). Tests:
  `crates/feature/tests/native_objects.rs` (the plan with `render-timing`,
  deep, the layer-draw port and the world; with the world alone the chain
  runs on the render thread; the switch off; the observer on
  `FlatMemory`, with the restore at the next layer draw and a rebuilt
  list left alone). Boot plans it (`registration.rs`, in `ALL_ON` of the
  registration tests); `render_thread`'s test checks the layer draw is
  render-thread safe with the world and objects on (see "Pipelined
  render").
- **Boot glue** (`crates/boot/src/native_objects_glue.rs`). Per map, a
  worker with the map, its shared maps and the object tag context open;
  definitions arrive one at a time from the feature and are loaded once;
  a model is queued for upload before its definition is published, so a
  pass that sees it finds it. Materials whose base map does not decode
  leave the model and keep their D3D draws. The render thread uploads the
  queue at the next pass. Log lines: `native-objects: <map>: definition
  <datum>: D draws, N nodes, in T ms (L models native, R left to D3D)`, or
  `... left to the D3D path (<why>)`; every 1,800 views
  `native-objects: D of V layer-3 views drawn natively, O objects, I
  object items skipped`.

### The live A/B

Old Mombasa, E1 off, frame stats and `render-timing` on, the same spot
with `impl.render.native_objects` off and on (with
`impl.render.native_world` on in both, so the difference is the
objects'). What passes:

- the load lines name the map's models, most as native; the refused ones
  give a reason (the run-time permutations: marines, some scenery);
- the views line shows every base-layer layer-3 view drawn natively, and
  objects and items per view in the tens;
- the frame shows every object in place and posed: bipeds animate (a
  T-pose or exploded limbs mean the `N · I` reading is wrong), held
  weapons sit in hands, vehicles and their turrets turn; flat-lit bases
  with the game's own self-illumination, specular and dynamic lights on
  top (those layers still draw through D3D);
- the first-person weapon and arms are unchanged (still D3D);
- the `0x0002bed0` site (layer 3) and `cpu` drop by the objects' share of
  layer 3's shader draws (below), `encode` and `submit` with them;
  `external_passes_dropped` stays 0 (a growth there is an object hole).

What this stage should remove: layer 3 is 8.26 ms on the census run,
about two thirds of it per-draw guest work in `0x00016b90` and the rest
D3D; objects are roughly a third to a half of layer 3's 1,882 D3D calls
on a populated 03a view (S: the split between world and object draws is
not measured; `render-census` can give it). So with the world already
native, expect about 2 to 4 ms off `0x0002bed0` and `cpu`, plus their
`encode`/`submit` share on the render thread. Layers 1, 5, 7, 8 and the
dynamic-light passes keep their object draws in this cut.

## Stage 3: transparents (design)

The sorted transparents are not layers of the scene list: after the
opaque layers, layer 15 and the late passes, `render_scene_passes` calls
`SCENE_TRANSPARENTS` (`0x0001e370`, 2.58 ms on the census run), which
sorts a list of its own and calls one handler per entry. So the skip is
not on `LAYER_DRAW`; it is on the geometry entries' handler. Stage 1b
(the world's layers 1, 5, 7, 8) is not a prerequisite: the transparents
depth-test against what the opaque layers left (the world's native pass
writes the game's W depth already), and their materials are the
template's layer-15 passes, which do not read the other layers' targets.

### The list (Code, `h2 disasm`)

- `[0x004b6288]`: entries of `0x70` bytes, the live ones from index
  `[0x004b6284]` to `[0x004b6280]` (at most `0x400`), allocated by
  `0x0001e2d0` (and `0x0001e4e0`/`0x0001e5e0` for other kinds).
- `0x0001e370` fills the `u16` index array `[0x004b628c]`, sorts it
  with `TRANSPARENT_SORT` (`0x0013df80`, comparator `0x0001e700`: by
  `+0` first, then through the plane at `+0x10` when `+0xc` is set; the
  rest is not read),
  then for each index in order calls `[entry+0x20]` with `entry+0x24`
  (stdcall), skipping a null handler.
- A geometry entry (`0x00041040`'s transparent branch, `0x00041381`):
  `+0` a class from `0x00040e10`, `+4` the view depth, `+0xc` a byte
  (plane present), `+0x20` the handler `0x00041020`, `+0x24` the shader
  datum, `+0x28` the argument the opaque items carry at `+0xc` (render
  record index `<< 8`, low byte the part), `+0x64` the position.
- The handler `0x00041020` (`points::TRANSPARENT_GEOMETRY`, `ret 4`) runs
  `0x00040f60`: the shader's template layer-15 pass range (`[template
  layers + 0x1e]`, `count << 9 | first`), and per pass the opaque path's
  build `0x0004cbf0` and, when it returns non-zero, the draw `0x0004d0b0`
  (with layer `0xf`). So an entry is drawn exactly like an opaque item,
  but with layer 15's passes, one entry at a time in sorted order.
- The render record at `+0x28 >> 8` says what the geometry is: `+4` an
  object handle, or `-1` for the BSP with `+0xc` bits 9..17 the cluster
  and 18..28 the instanced-geometry placement (`0x7ff` for a cluster;
  "What a world pass reads").

Other handlers in the list (effects, particles, contrails, decals) are
not read yet; they keep their D3D draws in this stage.

### Skip point and order

One `before` on `TRANSPARENT_GEOMETRY`, context-only: for an entry the
host draws (`native_render::transparent::read_entry`, then
`Host::accepts`), it answers `ret 4`, so the entry builds and draws
nothing and no guest memory changes (no restore). The hook meets the
entries in the game's sorted order, so no sort of ours is needed.

Ordering against the D3D path's transparents uses the device's external
pass as it already works: a pass asked for is issued at the device's
next entry, after the draws queued before it. Consecutive native entries
join one `transparent::Run`; the first asks the device for a pass that
owns the run, and the pass seals the run when the device issues it
(`ExternalPass::view_constants`, the issue moment, on the device's
thread). A D3D entry's draw is a device entry, so the next native entry
finds its run sealed and opens a new one. Each run therefore lands
between the D3D draws it sorts between, and its entries draw in their
sorted order inside the pass. An observer before `SCENE_TRANSPARENTS`
forgets the open run, so a run never spans two views, or outlives a
pass the device dropped.

Both hooks are context-only with nothing after a body, so the pipelined
render runs them on its own thread; the entries are read from the
render's snapshot of the frame.

### Order of work

1. **The skip and the runs** (built: `feature::native_transparents`,
   `native_render::transparent`, `impl.render.native_transparents`, off by
   default). Without a host nothing is skipped.
2. **The layer-15 look** (built: `look::transparent_passes`,
   `pass_state`). Every pass of the template's layer 15 at the first
   level of detail, in order, each with `look::implementation_look`'s
   rules (the ones layer 3 uses) and the pipeline state from the
   implementation's render states (`ResolvedImplementation::render_states`,
   decoded by `gfx` as the D3D path decodes them: blend enable, factors,
   op and constant, colour writes, depth test and write, cull, alpha
   test). All or nothing per material. It stays on the D3D path when a
   pass has more than one implementation (S: which one the engine binds
   for a transparent entry is not traced), reads engine render states
   (`engine_render_states`), turns on fog, stencil or a depth bias, or
   leaves blend enable, its factors or depth write to whatever the
   previous draw left (S: the states it does not set are taken at the
   D3D defaults).
3. **The world's transparent parts.** The draw table is built
   (`world::TransparentDraw`, `World::transparent(owner, part)`): the
   kind-4 parts in the world's vertex and index buffers, keyed by
   (cluster or placement, the part's index in its section; the loader now
   keeps every part in place, a part not drawn as an empty one), their
   lightmap pages in the lightmap array. The GPU side is built too
   (`transparent_gpu::TransparentSet`, `GpuWorld::encode_transparents`):
   each material's passes compiled with `program::compile_pass` (the
   world programs' translation plus the pass's alpha test) and uploaded
   with the same per-program code as the world's (`program::Gpu`), one
   pipeline per pass with its `PassState` (blend, write mask, depth write
   and compare, cull) and its blend constant set per draw. A run's pass
   loads the target and depth and draws, per entry in order, the part
   with each of its passes, over the whole target (the programs carry
   the viewport). Boot's host is built too (`boot::native_transparents_glue`, over the
   native world's loaded BSP, so the native world must be on): at load,
   each transparent material's passes compiled all or nothing and their
   stage images decoded, then the parts that can draw (a compiled
   material, and its lightmap page when a pass samples it), with one line
   on stderr: how many materials and parts, and why the rest stay on the
   D3D path. It accepts a cluster or placement entry whose part is in
   that table with the entry's shader; a run's pass takes the run's
   entries when the device issues it and draws them with
   `GpuWorld::encode_transparents`.
4. **Objects' transparent parts** (with `impl.render.native_objects` on
   as well). The hook copies the object's node matrices and sphere into
   the run (`transparent::RunEntry`, `objects::read_object`, as stage 2
   does at layer 3), from the render's snapshot, so the pass draws the
   frame's pose. An object's entry stands for all of its parts with that
   shader (S: which section and part an object's render record names is
   not read), so a later entry of the same object and shader in the view
   is skipped without drawing again (the parts draw at the first one's
   place in the order). On the GPU (`model_gpu`) a model keeps its
   transparent parts in its index buffer, in their own draw table
   (`Model::transparents`); the caller sets each transparent material's
   base map and pass state (`GpuModels::set_transparent`; S: an
   alpha-tested pass is refused, the models' shader has no alpha test
   yet), and `GpuModels::encode_transparents` draws a run's object items
   in order with the models' shading and the base map's alpha
   (`fs_transparent`), each pass's blend and depth state, and the run's
   node matrices in buffers of its own, so runs and the opaque objects'
   pass in one submit do not overwrite each other's. In boot, the objects'
   worker takes a definition's transparent materials with one layer-15
   pass, no alpha test and a base map that decodes, and publishes the
   definition's node count and each shader's materials; the transparents'
   host accepts an object entry when its definition is published with
   the copy's node count and the entry's shader, and a run's pass draws
   its entries in order as segments: consecutive world entries through
   the world's pass, consecutive objects through the models'.
5. **The live A/B**, as stage 1's: the same frame with the switch on and
   off, `external_passes_dropped` steady, and the `0x0002c19a` site's ms.

## Stage 4: lights and shadows (design)

The dynamic lights are not layers of the scene list either: after layer
7, `render_scene_passes` calls `render_dynamic_lights` (`0x00031c80`,
`points::RENDER_DYNAMIC_LIGHTS`), which gathers each visible light's
geometry into a list of its own and draws layers 10 to 14 over it, one
light at a time. The game has two more shadow paths beside it: object
shadows before layer 7, and the first-person view's lights after the
scene. What follows is the light path in the order the game draws it
(e3-lighting.md, "The light render path", has the light records and the
definition flags).

### The path (Code, `h2 disasm`)

- **Visible lights.** `0x00031c80` builds the visible-light list
  (`0x00031910`, from the candidates at `[0x00547f98]`; entries of `0x14`
  bytes, `+0` the light datum, `+8` a weight; sorted and trimmed by
  `0x0004a7c0`/`0x0004ac30`), then walks it. For each light above the
  weight threshold (`[0x0045dc58]`) it re-poses an attached light
  (`0x00031dac`), records it for the first-person pass (`0x004c6b00`,
  stride `0x408`, at most eight, count `[0x00509440]`), decides the
  shadow (`0x00031e02`), gathers the light's geometry into the light's
  list `0x004c1a60` (`0x000c1d90`, `0x001320f0`, `0x00031660`, and the
  extra call `0x00031c20` for definition bit 22), runs the objects'
  render records for it (`0x00133390`, `0x00133520`, `0x00133720`), and
  calls `render_light_passes` once.
- **One light's passes.** `0x00031170` (`points::RENDER_LIGHT_PASSES`; its
  only caller is `0x00031f26`): EAX the `ligh` datum, ECX a context from
  `0x000c17f0` (its first word sets per-light flag bit 0), stack `(light
  datum, &position, shadow byte, weight)`, `ret 0x10`. It sets the light's
  constants (`0x0004a1e0`, which takes 1024.0) and the flags
  `[0x004ba014]` and `[0x004ba018]`, then: with the shadow bit, the shadow
  projection (`0x00048ee0`, `0x24` dwords at `0x004c9798`) and layer 10
  (shadow generate into target 9, cleared to depth 1.0); layer 11 (shadow
  apply, `ShadowFunc GREATER`) when shadowed, bit 1 or definition bit 5;
  `0x00030ed0` when shadowed, the byte at `[0x00510c50]+5` set and
  `[0x004ba134]` positive (not traced); layer 14 (opaque,
  depth write, role unknown); layer 12 (diffuse) and, with bits 0 and 1,
  layer 13 (specular), each on `0x004c1a60` through the layer draw
  (`0x00044550`). It ends by turning stencil off, clearing the scissor,
  setting the view's target back (`0x00014bc0`) and zeroing both flag
  words.
- **Object shadows.** `0x000335e0(mode)` (from `0x0002c021`, before layer
  7) picks up to 32 casters (`0x00033400`, entries of `0x14` bytes) and
  calls `0x00032350` (stdcall, two words of the entry, `ret 8`) for each:
  its constants and projection (`0x0004a1e0`, `0x00048ee0`), layer 10
  into target 10 (S: 128x128,
  flags `0x14`: bit 4 picks that target) and layer 11 over what the
  shadow falls on.
- **First-person lights.** `0x000404b0` (from `0x0002c2b9`, after the
  first-person draw `0x0003fdf0`) draws the recorded lights again over the
  first-person view, with its own constants (`0x0004a1e0`) and layers on
  `0x004c1a60`.

### Skip point and order

One `before` on `RENDER_LIGHT_PASSES`, context-only: for a light the
host draws (`native_render::lights::Light`, the record copied with
`feature::native_lights::read_light` and its checks), it answers
`ret 0x10`, so none of the light's layers is built or drawn. It does the
passes' tail first, so a skipped light leaves what a drawn one does,
whatever the state it found (after layer 7, stencil may still be on):

- the device side, through the host (`Host::tail`, which calls
  `d3d::Device::light_passes_tail` at that point of the render, so in
  render order): what is queued drains as before any entry, then
  `SetRenderState_StencilEnable(FALSE)` and `SetScissors(0, FALSE, NULL)`
  through the code their entries run, and the game's depth-clip restore
  `0x00015720` (the words at `0x00485adc`/`0x00485ae0` into the
  programmable clip range, its game-owned bit, the viewport fragment
  `0x3f6190` at the put pointer). A push-buffer callback queued first, or
  no push-buffer room, refuses the tail with nothing set and the light
  keeps its D3D passes (d3d's `light_tail` test: the same device, guest
  globals, push buffer and next draw as the game's tail);
- then the guest words the tail zeroes (`lights::TAIL_ZEROES`:
  `0x004858bc`, `0x004b843c` and the per-light flags `0x004ba014`,
  `0x004ba018`), in the memory the hook's context sees (the render's
  snapshot on the render thread), as the world's mask restores are.

The tail's last call, the view's target back (`0x00014bc0` with
`[0x004858b8]`), is not repeated: a skipped light never moved the target.
Every way into the call should leave the view's target bound (the layer
begins draw every layer but 10 into `[0x004858b8]`, and layer 10's users,
this tail and the object shadows', set it back), and a guard makes it
so: the observer before `RENDER_DYNAMIC_LIGHTS` records the device's
bound target (`Host::target`, `d3d::Device::bound_target`: colour and
depth surfaces and viewport), and a light whose call finds another bound
keeps its D3D passes (`native_lights::target_refusals` counts them; a
steady count in the A/B says the reasoning was wrong). The gather before
the call still runs (its CPU, 1.88 ms on the census run, is the next
target, below).

Order uses the external pass as stage 3 does: consecutive native lights
join one `lights::Run`, whose pass the device issues at its next entry,
the next D3D light's first draw or what follows the lights; the pass
seals the run then. An observer before `RENDER_DYNAMIC_LIGHTS` forgets the
open run. The light's frame is read at the call, after the re-pose, from
the render's snapshot, so a run draws this frame's lights. Lights add, so
their order matters only through the shadow target each one reuses; the
runs keep the game's order anyway.

Both hooks are context-only with nothing after a body, so the pipelined
render runs them on its own thread. With `render-timing` on the feature
places nothing: its timer on the light passes needs their end, which a
skipped light never reaches.

### What a native light draws

- **What it lights.** The light's list `0x004c1a60` is gathered before
  the call, so the hook can copy its entries (render records, as the
  layer entries name them) into the run, with the objects' node matrices
  as stage 3 does: BSP clusters and placements through the native world's
  draw tables, objects through the models'. Until then the host can light
  the world's parts whose bounds meet the light's sphere (S: more than the
  game lights, never less).
- **Shading.** Each lit material's layer-12 and layer-13 passes compiled
  as stage 3 compiles layer 15 (`look`, `program::compile_pass`, the
  passes' render states: additive, no depth write, `LessEqual` against
  the game's W depth), drawn with the light's constants. Those constants
  are `0x0004a1e0`'s and its callees' (position, frustum or sphere
  falloff, colour, the gel's projection), not traced yet (e3-lighting.md,
  "What is still unknown"); tracing them into a pure function of the
  copied light and its `ligh` definition is the first research step. A
  material with layer-14 passes keeps the light on the D3D path until
  layer 14 is understood.
- **Shadows.** Layer 10's casters into a depth texture of our own from
  the light's projection (`0x00048ee0`, skipped with the call, so ported
  as a pure function), then the shadow term sampled in the light's own
  pass (`GREATER`, one tap as the D3D path's), instead of layer 11's
  stencil and alpha marks. Lights whose shadow cannot be drawn keep their
  D3D passes.

### Order of work

1. **The skip and the runs** (built: `feature::native_lights`,
   `native_render::lights`, `impl.render.native_lights`, off by default;
   points `RENDER_DYNAMIC_LIGHTS` and `RENDER_LIGHT_PASSES`). The host
   (slice 3) takes no light yet, so every light keeps its D3D passes.
2. **The light's constants**: trace `0x0004a1e0` and `0x00048ee0` and
   port them as pure functions of `Light` and the `ligh` definition, with
   tests against invented lights. `0x0004a1e0` and `0x000497a0` are done
   (Code): `engine::light_constants::light_globals` returns the whole
   block below, reading guest memory and writing none, and
   `LightGlobals::write` stores it. The arithmetic is the original's
   instruction for instruction, SSE through the `simd` crate under the
   caller's MXCSR and the special frame's x87 block through `x87::abi`
   under its control word, so the words are bit-equal (NaNs, rounding
   modes and precisions included); the image's constants are read at their
   addresses. `0x001650a0` (the sphere's screen bounds) is the caller's to
   answer, and `0x0003a550` (the scissor) the caller's to run after.
   `engine/tests/light_constants_differential.rs` runs the original,
   lifted, on the IR interpreter from seeded states (records, contexts of
   each type, the special frame, every colour scale mode, projected and
   whole-viewport rectangles, float edge values, three MXCSR and x87
   modes) and compares all memory; `engine/tests/light_constants.rs` checks
   invented lights. What the trace found:
   - `0x0004a1e0` (`ret 0x10`, about 250 instructions): EAX the light
     record's `+0x84` (so `[EAX+0x50]` is the record's colour A at
     `+0xd4`, `+0x5c` colour B, `+0x68`..`+0x84` the record's words
     `+0xec`..`+0x108`), EDX the light datum (-1 for the first-person
     pass's own), ECX the context (its word into `[0x004858c8]`), stack
     `(ligh definition address, 1024.0, shadow byte, weight)`. It writes
     the light's globals: `[0x004858bc]` 1 (the tail's zero),
     `[0x004858c4]` the definition, `[0x004858cc]` the driver object,
     colour A and B at `0x00485944` and `0x00485950` scaled by the
     weight-scaled record words, `0x00485a30`..`0x00485a40` (falloff and
     cutoff words; `0x00485a40` is the one `0x00031170` compares for the
     shadow), the bounding sphere at `0x00485934`..`0x00485940`, the
     screen rectangle at `0x00485964`..`0x00485978` (the sphere projected
     with the view at `[0x00547f88]` through `0x001650a0`), the shadow
     byte at `0x00485a44`, and calls `0x000497a0` (about 560
     instructions, `ret 0x20`: the light's frustum and gel projection
     into `0x0048597c`, `0x00485a0c`, `0x00485a1c`, `0x00485a2c`) and
     `0x0003a550` (the scissor from that rectangle, through D3D).
   - `0x000497a0` builds a 31-word frame block: the origin (the
     context's `+0x2c`, or, for a definition with flags `0x20` and
     `0x40000`, the globals at `0x004b9da0` moved along the light's axes
     by an x87 sway of sines and cosines of its first two words), the
     projection rows (by context type: 0 the context's and the light's
     up vector, 2 cross products of the axes over the projection's
     extents, 1 none), forward times -1 over the depth range, either a
     constant vector or the projective tail (definition word `+0x8e` is
     1), the record's offsets `+0xf0` and `+0xf4` along forward, and both
     colours times the scale, halved over 1 or quartered over 2 (mode at
     `0x00485a2c`) and clamped to `[0, 1]`. Words a path does not set
     keep their old values, so the port reads them.
   - `0x00048ee0` (about 400 instructions): the shadow projection, `0x24`
     dwords at `0x004c9798`. Done (Code): `engine::shadow_constants::
     shadow_globals`, the same way. ECX the context, EBX the light's frame
     block, stack the definition; `ret 4` through a tail jump to
     `0x003f7330`, which uploads the 36-word block as vertex shader
     constants (ECX `0x28` the first register, stack `0x24` the dwords:
     `c[40]`..`c[48]`). It also sets the shadow mode
     byte `0x00485a47` (2 when scene flag bit 10 is clear, 1 for a light
     with falloff word 4 above zero and definition word `+0x14` zero),
     clears the four desired textures at `0x0051f3d8` and binds them
     (`0x0001cf50`), and sizes the shadow map: 128 or 512 square by pass
     flags 4 and 5, the extent from the context's field of view and the
     falloff, each side rounded up to 64 (`0x00048e70`) and cut to the
     square; the full map is target 9, sized through `0x0001ddd0`. The two
     device calls are the caller's; `0x00048e70` and `0x0001ddd0` are
     ported. Tests: `engine/tests/shadow_constants_differential.rs` (the
     lifted original, all memory) and `engine/tests/shadow_constants.rs`.
   These globals are what the layer-12/13 templates' engine inputs read,
   so the port's output is that block, compared word for word with the
   game's in a test that runs on recorded inputs.
3. **Unshadowed lights over the native world**: the host takes lights
   without the shadow bit, the light's list copied at the hook, and draws
   the world's lit parts with their materials' layer-12 and 13 passes.
   Started (Code): `boot::native_lights_glue` is installed on the device
   with the world's host. Its `tail` runs `Device::light_passes_tail` over
   the hook's memory (an adapter to the device's memory trait; the hook
   holds the guest's memory, then the host locks the device, the D3D
   entries' order) and its `target` is `Device::bound_target`. Its
   `accepts` says no to every light until the passes are drawn, so the
   feature records the view's target and every light keeps its D3D
   passes. What the drawing needs, traced so far:
   - **Engine textures** (`0x00019530`, a jump table at `0x00019b68`
     indexed by id - 1; Code): ids 1, 9 and 14 are bitmaps from the
     globals' rasterizer data (`[0x00485a80]`, set by `0x00022590` from
     `matg +0x10c`, the block's first element), the datums at `+0xc`,
     `+4` and `+0x14`; S from how the light passes sample them, 1 is the
     vector normalization cube map and 9 the distance attenuation volume.
     Id 10 is the light's own texture: with `[0x004858c8]` zero or no
     definition, `0x000196d3` (not traced); definition flag `0x80000`
     redirects to id 18; a definition `+0xe0` other than -1, with
     `[0x00485a77]` clear, is render target 13; else the definition's
     bitmap at `+0x7c` (the gel), through `0x000144f0`. Id 13 is render
     target 9 (the shadow map, sized by `0x00048ee0`) and id 8 target 10
     (the object shadows'), through `0x00014f60`.
   - **Implementations** (Code; `engine::light_passes`). A world
     material's layer 12 has one pass with several implementations; on
     03a's `tile_metal_dark` (the `light_response` pass) they are 0
     `lightaccum_frustum_tex`, 1 `lightaccum_sphere_diffuse_tex` (stage 3
     id 9, sampled as a volume), 2 `lightaccum_frustum_no_specular_tex`,
     3 `lightaccum_frustum_no_specular_gel_tex` (stage 3 id 10) and 4
     `lightaccum_frustum_tex_first_person_change_color`; its layer 13 has
     one, `stencil_light_specular_tangentspace_sphere`. The engine picks
     one per entry:
     - `render_light_passes` draws layers 11, 14, 12 and 13 on the light's
       list, each only when the list's OR mask (`[0x004c1a68]`) has the
       layer's bit, and 13 only with flag bits 0 and 1. Before them it
       sets the flags (`0x000311c8`..`0x0003128e`, ported as
       `pass_flags`): bit 0 when the context's first word is 0; bit 1
       when a colour B component is above the image's constant at
       `0x0045dc7c` (`ja`), after colour B is zeroed when neither the
       pointer `[0x00510c50]` with its byte `+5` nor the definition's word
       `+4` keeps it; with the shadow byte (the caller's, kept only under
       scene flag bit 10) and bit 0 clear, bits 2 and 5, and bit 3 when
       falloff word 4 is above zero and definition word `+0x14` is not 2;
       bit 4 always cleared. The byte `[0x004ba018]` becomes 1.
     - The layer draw's handler table for structure and model geometry
       (`0x00041640`) gives layers 12 and 13 the draw `0x0004ebd0` for one
       item type (S: its slots read as layers 11 to 22), and the world's items use build `0x0004cbf0` and draw
       `0x0004d0b0`: the build stores the implementation in the entry's
       byte `+4` (-1: no entry) and the draw hands it to `0x0001bbf0` as
       its fourth argument, whose `0x00016b90` binds implementation `n`
       (stride `0x132`) of pass `a3` of layer `a2` in variant `a1` (the
       item's byte `+0x15`).
     - The build's own cases are layers 3, 16, 18 and 19; 12 and 13 take
       `0x00015d70`'s answer: -1 when the template has no pass in the
       layer (the count in the layer word's bits 9 and up), else
       `0x00030da0` for 12 (`diffuse`: 1 with flag bit 0; 4 with the
       definition's first byte bit 5; 0 with flag bit 1 and the shader's
       integer input 4 (`shad +0x3e`) non-zero; 2 with flag bit 2; else
       3) and `0x00030e00` for 13 (`specular`: 0 with flag bits 0 and 1
       and the integer input, else no entry). So a frustum light without a
       shadow draws the gel implementation 3, a shadowed one 2 (its gel is
       folded into layer 11's marks, S), a context-zero light the sphere
       implementation 1 and, with a bright colour B, the specular layer.
     - Tests: `engine/tests/light_passes_differential.rs` (the flag code
       run from `0x000311c8` to either successor of `0x00031293`, all
       memory; both choosers' answers and stack) and
       `engine/tests/light_passes.rs`.
   - **Engine constants.** The light's vertex constants are uploaded by
     `0x0003a550` (the last call of `0x0004a1e0`), not by the pass: the
     frame block and the five words after it, 36 dwords from `0x0048597c`
     to `c[31]`..`c[39]` (`0x003f7330` takes ECX the first register in
     hardware numbering and the dword count on the stack: it writes the
     push-buffer methods `0x1ea4` and `0x0b80`). The shadow block goes to
     `c[40]`..`c[48]` the same way. A skipped light uploads nothing, so the
     host fills those registers from `LightGlobals::frame_constants`. The
     same function then, for a definition with a `shad` at `+0xdc`/`+0xe0`,
     draws that shader into render target 13 (`0x00016610` with layer 13,
     `[0x00485a77]` set meanwhile): the light's animated gel, which
     texture id 10 then samples. The engine inputs the light passes can
     bind (`0x0001b2c0`): vector 5 (`0x00033a0b`) and 6 (`0x00033a25`)
     are the frame's words 24..26 and 28..30, colours A and B scaled and
     clamped (`LightGlobals::colour_a_input`, `colour_b_input`); integers
     0 and 1 are the colour scale mode `[0x00485a2c]`, 2 and 3 the
     definition's words `+0x8a` and `+0x80`; scalar 16 reads falloff word
     0 (`0x000336fb`). Which of them each lightaccum implementation binds
     is in its `spas` external constants (`ResolvedImplementation`, mode 4
     a vector). Scalar 16 is falloff word 0 when below 1, else 1; integer
     7 is the definition's bit 1 clear; integer 6 (`0x0003409f`) also
     tests bit 13 of an argument not traced, so it is not supplied.
   - **The look and the program** (Code; `native_render::light_look`).
     `tags::material` now reports every input an implementation reads
     (`ResolvedImplementation::input_reads`: its masks, its engine
     constant bindings' sources and its fragment selectors).
     `engine_inputs` fills the light's inputs over the instance's
     (`LightInputs`, from `LightGlobals`); the host resolves the material
     with them (`ShaderLibrary::material`) and `light_look` takes the
     chosen implementation of layer 12 or 13, refusing passes that read
     inputs nobody supplies, animated ones, engine-driven states, and
     engine textures but 1 (`Normalization`, bound as a cube), 9
     (`Attenuation`, a volume) and 10 (`Gel`, 2D): the shadow maps (13,
     8) keep a shadowed light on the D3D path. `compile_light` translates
     it through the same code as the world's and transparents' passes
     (`program::translate_pass`), and `light_vertex` puts the light's
     frame constants into `c[31]`..`c[39]`.
   - **Host pieces** (Code). The hook computes each light's constants as
     `0x00031170` would (`feature::native_lights::light_constants`: the
     definition from the tag table, the shadow word under scene flag bit
     10, `light_globals` without the screen bounds, `pass_flags`) into
     `Light::constants`; `LightInputs::from_constants` and
     `LookCache` (keyed by shader, layer, implementation and the inputs'
     bits) resolve a material once per distinct light;
     `native_render::light_gpu` uploads the cube and volume
     (`LayeredImage`, `layered_texture`) and draws a compiled pass with
     one uniform group per light (`LightGpu`); `boot::native_lights_glue`
     reads the light textures' bitmaps from `matg +0x10c`
     (`light_bitmaps`, `light_textures`) and says which lights are
     drawable (constants, no shadow).
   - **Host wiring** (Code; `boot::native_lights_glue`). Texture 10, the
     gel (`0x00019530`'s id-10 case, `engine::light_passes::gel`): with
     the context's first word non-zero (`[0x004858c8]`, copied by
     `0x0004a1e0`), the definition's flag `0x80000` clear and no gel
     shader at `+0xe0`, image 0 of the bitmap at `+0x7c`; the hook keeps
     it in `LightConstants::gel`, and a pass sampling any other texture 10
     keeps the light on D3D. At world load, with the switch on and before
     the world drops the materials left to D3D (`load`): each material's
     shader and instance inputs, the vertex programs and stage images of
     every implementation of its layer-12 and 13 passes, the light
     textures, the gels of the map's `ligh` tags, the opaque parts of
     native materials with lit layers, and the bounds of every other
     surface that has (or may have) lit layers: materials left to D3D
     and transparents. Per light (`LightsWorld::plan`): the parts whose
     bounds meet the light's sphere (`Aabb::meets_sphere`, an
     over-approximation: the attenuation map fades the rest), none if
     the sphere meets one of those other surfaces; per material the
     implementation `0x00030da0`/`0x00030e00` would take, resolved with the
     light's inputs, `light_look` and `compile_light`, cached by inputs
     (`LookCache`). `accepts` is `drawable` and a plan; `open` asks for one
     external pass per run, which plans the run's lights at its issue and
     draws them with `GpuWorld::encode_lights` (`light_gpu::LightSet`: the
     engine textures and gels uploaded once, one `LightGpu` per
     `light_gpu_key`, so lights whose inputs differ only in constants share
     a pipeline with their own uniforms). With the switch off the hooks are
     not placed, so `light_constants` never runs, and nothing loads.
     - Objects: the hook leaves a light to D3D when its sphere meets a
       live object's (`objects::object_meets_sphere`, the object array to
       its high-water mark; unreadable or non-finite answers yes), counted
       in `native_lights::object_refusals`, so no object loses a light
       until stage 4's objects item draws them.
     - Colours: the resolve records where each mode-4 engine constant put
       its vector (`ResolvedImplementation::vector_bindings`: a vertex
       register's components, or a combiner constant's lanes packed as
       `0x00019ca0` does, `pack_engine_colour`, minus what later bindings
       overwrite). The cache key leaves the colours out and the host
       resolves with them zero; `colour_uniforms` puts each light's
       colours A and B (vectors 5 and 6) into its uniforms, so an animated
       colour resolves nothing.
     - Uniforms: one buffer per run, each draw's vertex and pixel uniforms
       at aligned dynamic offsets (`light_gpu::uniforms_layout`,
       `run_layout`), one group for the run.

4. **Objects**, through the models as stage 3's objects.
5. **Shadows**: layer 10 into our depth texture, the term in the light's
   pass.
6. **Object shadows** (`0x00032350`, the same skip and run shape) and
   the **first-person lights** (`0x000404b0`, with stage 2's first
   person).
7. **The gather**: once every light is native, skip `0x00031c80`'s gather
   too and build the visible lights from the light array ourselves; the
   first-person records at `0x004c6b00` then need writing, so this is not
   a pure skip.
8. **The live A/B**, as the other stages': the same frame with the switch
   on and off, `external_passes_dropped` steady, and the light site's ms.

## Stage 5: post and HUD (design)

What the game draws after a view's scene comes in three groups: the
last passes of `render_scene_passes`, the rest of `render_view_scene`
(a fade, then the HUD and UI), and the end of the frame after every
view. None of them is a layer of the scene list except layer 17. This
section follows 03a's path in the order the game draws it. The census
run's figure for `render_screen_effects` is 0.76 ms and 164 D3D calls a
frame.

### The path (Code, `h2 disasm`)

- **The frame.** `render_frame_and_present` (`0x00223240`) renders the
  views (`0x0002b5d0` → `render_game_view` `0x0002b790` →
  `render_view_scene` `0x0002ba10`), then ends drawing (`0x00013b90`,
  `0x00013cd0`) and presents (`0x00013f10`).
- **One view.** `render_view_scene` runs the visibility and records
  (`0x0002c3b0`, `0x0003fd70`, `0x0002b990`), then `0x00013680`, which
  calls `render_scene_passes` (`0x0002bcd0`, at `0x000136ac`). After it,
  in order: `0x00015370`, `0x00015680` (the view kind `[0x00485898]`),
  `0x001917eb` when `[0x004b9ed8]` is not -1, then `0x00040890`, then
  `0x00147af1` (EAX -1, two words pushed), then the view's end
  `0x00013c20`.
- **The scene passes' tail** (from `0x0002c19a`): the sorted
  transparents `0x0001e370`, `0x00025dc0` (view kind not 2), the player
  chain with `[0x00485a76]` set and `0x0002afa0`, the first person
  (`0x0003fdf0(0)`, its lights `0x000404b0`, `0x0003fdf0(1)`),
  `0x00040870` when BL is set (the transparents again, then
  `[0x004b6284]` copied to `[0x004b6280]`), `[0x00485a76]` cleared, then
  the late passes:
  - `0x0002dd30` when the caller's byte `[esp+0x28]` is set: its own
    target (`[0x00485602]`), render states (`0x003f5cf0`), a stipple
    (`0x003f8cc0`) and quads (`0x0001cf50`, `0x0001c590`, `0x0001c710`,
    `0x00020bb0`);
  - layer 17 when the byte `[esp+0x2c]` is set (S: bloom; 250 passes on
    the disc, from the scene list, gated by `0x004ba024`);
  - `0x00020770`: the occlusion query results (`[0x0051f408]`,
    `[0x0051f409]`, the frame count mod 3, `0x003f8630`);
  - `render_screen_effects` (`0x00013870`,
    `points::RENDER_SCREEN_EFFECTS`), stdcall `(late, colour)`, `ret 8`.
    `colour` is the byte that gates layer 17, and `late` the one that
    gates `0x0002dd30`;
  - the exit `0x00013630`: with `[0x004850c8]` set (a view drawn into a
    target of its own), the target back to 0 (`0x00014bc0`) and the
    depth clip (`0x00015720`).
- **`render_screen_effects`.** Its prologue sets `[0x004670bc]` to 1,
  resets the shader draw state (`0x00016b10` on `0x00485b48`), stores the
  word `[0x00485600]` at `[[0x00485890]+0x1c]`, and runs `0x000245c0`
  with EAX `0x0051ecf0`. Then:
  1. With `colour`, the colour pass `0x000214f0`. Its inputs are
     computed by `0x000137a0` and blended with `[0x004b9fd0]..`, with
     constants `0x407f7cee` and 0.25, and its last argument is "view
     kind is 6". It binds the view's target `[0x004858b8]` as a texture
     (`0x00014f60`), writes a combiner into the PSDEF at `0x00484f68`, and
     draws through `0x0001ccf0`.
  2. The scenario's screen effects. This needs a scenario
     (`[0x004e0350]`), a view kind outside 5..7, both `[0x00485607]` and
     `[0x004850c8]` clear, and `0x00022710` true. It reads a rectangle
     from the words at `0x004b9dd0..`, then walks the scenario block at
     `+0x3d0` (count) and `+0x3d4` (address), entries of `0x24` bytes:
     `+0x14` is a tag datum, and `+0x20`/`+0x22` are function indices
     whose values (`0x00134fc0`) decide whether `0x0002d140` draws the
     entry (a shader through `0x00016610`). S: 03a's map holds one
     `egor`, `effects\screen_effects\blur`.
  3. With `late`, `0x0002df00`. It is shaped like `0x0002dd30`: a
     target, render states, the viewport (`0x003f6840`, `0x003f6f40`,
     `0x003f6390`, `0x003f6190`) and quads.
- **After the scene.** `0x00040890` is a full-screen fade: when the mode
  `[0x00485870]` is not 0, it draws the colour at `0x00485874..`
  (packed by `0x00131e50`), with a blend picked through the jump table
  at `0x00040dc0`. Then `0x00147af1` composes the UI and HUD. The HUD
  render (`0x0013954b`, gated by `HUD_SHOWN` `0x0013939b`) is S: under
  it; its caller chain is not traced. So the HUD is always drawn after
  `render_screen_effects`, and the post passes need no HUD split: the
  colour pass and the screen effects see the scene only, and the fade
  and the HUD go over them.

### Skip point and order

One `before` on `RENDER_SCREEN_EFFECTS`, context-only:
`feature::native_post`, with what the call was asked to draw read by
`native_render::post::PostFrame` (the two stack bytes, the view kind,
the scenario's screen-effect count and the loop's gates other than
`0x00022710`). When the host draws the call, the hook answers `ret 8` in
place of it. The host asks the device for an external pass, as stages 3
and 4 do, and the device issues it at its next entry. That is the first
device call after the scene passes return: the exit's target change for
a view drawn into its own target, otherwise the fade or the UI and HUD.
So the native post lands after layer 17 and before the fade and the
HUD, once per view, in the game's order.

The hook has nothing after its body, so the pipelined render runs it on
its own thread. With `render-timing` on the feature places nothing: the
timer it puts on the scene's callees, `0x00013870` among them, needs the
call's end, which a skipped call never reaches.

A skipped call must still leave what its prologue leaves. A host
accepts a call only once it does that (item 2 below). Until then boot
installs no host, the hook only counts (`native_post::counts`: calls,
colour pass, late pass, screen effects possible), and every call keeps
its D3D passes.

### What the native post draws

- **The colour pass**, `0x000214f0`'s combiner over a copy of the view's
  colour target: a full-screen triangle sampling the scene the native
  passes and the D3D passes drew. Its inputs (`0x000137a0` and the
  blend) are ported as a pure function of guest memory, as the light
  constants are.
- **The screen effects**, each entry `0x0002d140` would draw: its shader
  compiled as stage 3 compiles layer 15, drawn with the rectangle and
  the entry's function values. Blur needs the scene as a texture, which
  the colour pass's copy already gives.
- **`0x0002df00`**, once it is traced; until then a call with `late`
  set keeps its D3D passes.

### Order of work

1. **The skip and its order** (built: `feature::native_post`,
   `native_render::post`, `impl.render.native_post`, off by default;
   point `RENDER_SCREEN_EFFECTS`). Boot has no host, so nothing is
   skipped, and the periodic line says what 03a's calls ask for.
2. **The prologue of a skipped call.** `[0x004670bc]`, the shader draw
   state `0x00016b10` resets on `0x00485b48` (the device side, as the
   lights' tail does), `[[0x00485890]+0x1c]`, and what `0x000245c0`
   leaves. Each is a guest word or a device call at that point, with a
   test that a skipped call leaves what a drawn one does.
3. **The colour pass**: trace `0x000137a0` and `0x000214f0`, port its
   inputs, compile its combiner, and draw it from a copy of the colour
   target. The host accepts calls with `colour` set and `late` clear, and
   with no screen effect that would draw.
4. **The screen effects** (`0x0002d140`; 03a's blur first), with their
   function values (`0x00134fc0`) read at the call.
5. **`0x0002df00`, and `0x0002dd30`** before layer 17, which has the
   same shape and gets its own skip point.
6. **Layer 17** (S: bloom), as stage 1b draws the world's other layers.
7. **The fade** `0x00040890`, a skip point of its own: one full-screen
   quad with the colour and blend it reads.
8. **The UI and HUD** (`0x00147af1`; `HUD_SHOWN`, the HUD render
   `0x0013954b`): trace the caller chain first, then text and bitmaps
   through the native renderer. Once that is done, `0x00013b90`,
   `0x00013cd0` and the present (`0x00013f10`) are the last D3D work in
   the frame.
9. **The live A/B**, as the other stages': the same frame with the
   switch on and off, `external_passes_dropped` steady, and the time of
   `render_screen_effects`.

## Open

- Seams: the drawn world is welded (no T-junctions, shared edges
  bit-identical; `viewer`'s `bsp_cracks`), so no load-time T-junction
  repair is needed. Draw every material's parts from one vertex buffer
  with one position transform, and don't snap to a 1/16 grid at scale, so
  shared edges match bit for bit (high-resolution.md, "Subpixel snapping
  and cracks"). Re-run `bsp_cracks` on the instanced geometry (not covered
  yet) and on the campaign BSPs before relying on it.
- What reads the stencil marks of modes 1 and 2.
- E1 reprojection: a replayed frame must redraw the native pass with its
  warped camera; the interpolating backend records draws, not passes.
- Split screen: one native pass per view, with the view's camera (the
  camera record is per view; the hook runs per view already).
- Which 03a templates have a layer-1 texaccum pass that layer 3 samples
  in screen space; those need the texaccum target bound to the native
  pass, or stage 1b.
