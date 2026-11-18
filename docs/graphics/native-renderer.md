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

The crate `native-render` (`crates/native-render`), no game wiring yet:

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

### Next, in order

1. **`d3d` external pass.** `Device::request_external_pass(pass)`,
   pending like `request_depth_grid` and issued at the next push-buffer
   drain with the bound colour surface, depth surface and viewport;
   `Backend::external_pass(colour, depth, viewport, pass) -> bool`
   (default `false`: the device then leaves the world to the D3D path for
   that frame by not clearing the bits, see 2). `GfxBackend` closes its
   open pass, hands the encoder and the two surfaces' views to the pass;
   `ThreadedBackend` posts it; the scaled backend gives the view surfaces
   and a scaled viewport; the interpolating backend (E1) forwards it
   and, until it can replay a pass with a warped camera, marks the frame
   not replayable.
2. **The feature `native-world`** (`impl.render.native_world`, off by
   default, exact key): a `before` and an `after` on `0x00044550`. Before,
   with layer 3 and a world loaded for the active BSP (`0x004e0348`): read
   the camera, ask the device for the pass (the GPU world prepared with
   that camera on the render thread), and only when the ask is accepted,
   `skip::clear_structure`; after, `skip::restore`. Two hooked calls per
   layer draw, about 30 a frame, nothing per draw.
3. **Boot glue.** On a BSP change, load the world from the disc on a
   worker (`load::load_world` over the map and its shared resource maps,
   the base maps and lightmap pages decoded with `assets`), then build the
   `GpuWorld` on the render thread. Until it is ready the hook does
   nothing and the D3D path draws the world.
4. **Templates.** The base map per material from the shader's template
   and bitmaps; the 03a templates moved to their translated programs
   (above), each behind the per-material fallback (a material left out of
   the world keeps its D3D draws: the skip tests the item's shader datum
   against the world's set).

## Open

- What reads the stencil marks of modes 1 and 2.
- E1 reprojection: a replayed frame must redraw the native pass with its
  warped camera; the interpolating backend records draws, not passes.
- Split screen: one native pass per view, with the view's camera (the
  camera record is per view; the hook runs per view already).
- Which 03a templates have a layer-1 texaccum pass that layer 3 samples
  in screen space; those need the texaccum target bound to the native
  pass, or stage 1b.
