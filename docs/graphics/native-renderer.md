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
| 5. Post and HUD | Fog, bloom, active camouflage, screen effects, HUD and text, then the present | The late passes, `0x00013870`, the HUD draw | Nothing left: the D3D path is only a fallback |

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
- **after** it: set the bits back.

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
  exact key). While it is on, pipelining is off (`pipeline_render` prints
  `pipeline: off: native-world's layer-draw hooks run on the game
  thread`): the observers are not render-thread safe (an after
  continuation, guest masks, the device's pass), so on the render thread
  every pipelined frame would fall back at its first layer draw. Making
  them render-thread safe (no after part, an audit of the host's writes)
  is a later step.
  `LAYER_DRAW` is an opaque point, so both hooks are observers: before,
  with layer 3 current, it reads the camera and the active BSP
  (`0x004e0348`) and asks the host; when the host accepts it clears layer
  3's bit on the BSP items whose shader the world draws; after, it sets
  them back. Both sit inside `render-timing`'s observers
  (`after_feature`). Two observer calls per layer draw, nothing per draw.
  Tests: `crates/feature/tests/native_world.rs` (the plan with
  `render-timing`, deep, and the layer-draw port all on; on by default
  with pipelining off; the switch off places nothing and keeps
  pipelining; the skip and restore on synthetic memory).
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
     reading other inputs, a translation warning, a non-2D fetch, a
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

## Open

- What reads the stencil marks of modes 1 and 2.
- E1 reprojection: a replayed frame must redraw the native pass with its
  warped camera; the interpolating backend records draws, not passes.
- Split screen: one native pass per view, with the view's camera (the
  camera record is per view; the hook runs per view already).
- Which 03a templates have a layer-1 texaccum pass that layer 3 samples
  in screen space; those need the texaccum target bound to the native
  pass, or stage 1b.
