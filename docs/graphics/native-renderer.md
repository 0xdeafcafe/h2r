# Native GPU renderer

Status: design, and stage 1 in part (below, "Stage 1 as built"). The goal
is every frame drawn from GPU-resident data, with the game's recompiled
render front-end left doing only what the GPU path cannot do yet. Evidence
labels as in [feature-engine.md](../engine/feature-engine.md): **Code**
(read in the disassembly, `h2 disasm`), **Test**, **S** (supposed). No
game data: addresses and offsets are research points, everything else is
read from the user's disc at run time.

## Ordered replacement handoff

World, objects and held post packets use the device reservation below. Geometry
hooks for particles, decals, HUD, gel producers and world layers can register
`Device::add_draw_admission(Arc<dyn DrawAdmission>)`. The device calls admissions
in registration order at the resolved draw, for immediate, inline, indexed and
array geometry. Scope ownership to the current producer and match the complete
state. Returning `None` leaves the draw available to the next admission. A
failed preparation of a claimed draw falls through to the original D3D draw.

```rust
trait DrawAdmission: Send + Sync {
    fn admit(&self, snapshot: &DrawSnapshot<'_>)
        -> Option<(ExternalPlan, Box<dyn ExternalPass>)>;
}

// At a hook outside draw submission, drain the preceding guest commands.
let reservation = device.prepare_external_pass(mem, plan, pass)?;
if let Some(reservation) = reservation {
    // Apply reversible mask edits here. Drop the reservation to cancel.
    reservation.commit();
}
```

`DrawSnapshot::draw` borrows the exact `ResolvedDraw` that D3D can still issue.
It includes vertex declaration, streams, indices, shader inputs (microcode,
constants, defaults, pixel definition and combiner state), raw and translated
render and stage state, texture headers and formats, viewport and scissors.
The snapshot adds bound colour/depth headers and palette headers and entries.
The device replaces only the draw packet. The guest's shader setup and
stencil tail still run, including when adjacent draws have different owners.

`ExternalPlan { stage, view, ranges, slots }` promises a positive number of
geometry or effect ranges on **each** participating copy. `slots` is an indexed
`Vec<Option<ExternalSlot>>`; each populated slot names a guest colour surface
header and whether it will be written. `ExternalPass::writes()` declares writes
to the bound colour and depth/stencil. In `record_ranges(ExternalTarget) -> usize`,
`target.copy` is `Reference` or `Display`, `target.stages` holds the four resident
stage views for a resolved draw, and `target.slots[i]` supplies the live surface's
attachment, sample view, format and texel size for this copy. Record only in
the supplied encoder. A queue submission or write to a live texture would
escape cancellation. A declared sampled header or intermediate surface that
cannot be resolved on one required copy refuses the entire replacement.

Recording takes place during prepare. Partial recordings are discarded.
Commit splices only previously sealed command buffers into the D3D submission
order. Commit has no remaining stage callbacks or target lookups.

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
  draw's world bounds on the CPU (a few thousand AABBs: microseconds).
  With material programs only the four sides and the eye's plane
  (`Frustum::sides`): the programs clip as the D3D path does, and the
  render camera's `near`/`far` are not that range (03a's distant
  buildings in the intro cinematic, and a far girder in play, lay past
  the camera's `far` and were left out natively while their items were
  skipped: holes showing what was behind). The cull alone did not close
  them (the 02000 frame was byte-identical before and after it): the
  programs also clipped w to `[0, z_max]` with out-of-range depth culled,
  where the D3D path takes the device's pushed clip range
  (`SET_CLIP_MIN`/`MAX`, `nv2a::w_clip_range`) and
  `D3DRS_DEPTHCLIPCONTROL`'s clamp, which the game sets per view
  (`0x00015720`). The device now gives the pass both at issue
  (`ExternalPass::depth_clip`, `Device::depth_clip`) and the world's
  programs draw with them (`ViewInputs::w_clip`), as the objects' lit
  parts now do (`ObjectsPass::depth_clip`; 03a's near is 256 depth
  units, which the objects clipped at 0). The Stage 4 cluster light pass also takes the device clip. Transparent
  passes still take the whole range (S: the same gap for anything far in
  a cinematic, or nearer than the view's near). That did not close the
  02000 slivers either (the 2026-11-19 A/B: 1490 px, max 87, unchanged).
  To tell the remaining causes apart, `H2_NATIVE_WORLD_TRACE=1` prints a
  line per pass (the first 8, then every 250th, or every `n`th with
  `H2_NATIVE_WORLD_TRACE_EVERY=n`: the clip, whether draws
  are W-buffered, the render camera and its view-projection beside the
  constants `c[0..3]`, the cull's count), and `H2_NATIVE_WORLD_NO_CULL=1`
  draws every part without the CPU cull. With the trace,
  `H2_NATIVE_WORLD_PROBE=x,y` adds, on each traced pass, the parts whose
  triangles cover that guest pixel through the render camera, nearest
  first: owner (cluster or placement), part, material, view depth and the
  distance to the triangle's nearest edge (under a pixel: an edge tie).
  `H2_NATIVE_WORLD_OWNERS=clusters` (or `placements`) draws only that
  kind natively and leaves the other's items to the D3D path. At 03000
  the probe put the 43 px girder line on placement 282's edge (0.23 px
  outside its triangle, over cluster 31), and the 02000 remainder and
  01000's 3 px are edge pixels too (the van's outline, the curb): the
  owners switch says whether the rigid path or the clusters own them.
  It split them (the 2026-11-19 runs): the placements own the girder
  line, 01000's specks and 220 of 02000's px; the clusters the curb's 80.
  Both also left about 4% of 03000's pixels one level darker than the
  D3D path, in noise over every lit surface (13 400 px each, 26 800 with
  both): the lighting vector (`v5`). The stream format table
  (`0x0043f408`, type `0x30`) declares it `NORMPACKED3`, so the GPU reads
  `s / 1023` (`s / 511` for z); the native world had it from the CPU's
  midpoint decode (`(2 s + 1) / (2^bits - 1)`, [lighting.md](../formats/lighting.md)),
  up to half a step higher in each component. It now takes the GPU's
  decode (`load::program_lighting_vector`), which matches the D3D path
  but did not remove the speckle (mean 0.204 to 0.203). The extended
  compare found where the two still differed: the D3D path binds the
  frame and lighting vector (`v2..v5`) as their `NORMPACKED3` words and
  its translation decodes them in the shader (`xvs_normpacked3`), where
  the native programs read floats decoded on the CPU, so the two modules
  were different code. A world vertex now carries the four words too
  (`WorldVertex::packed`, `world::pack_normpacked3`, which gives back the
  stream's word for any value decoded from one) and the material
  programs fetch and decode them as the D3D path does
  (`program::INPUTS`, `WORLD_FETCH`): a world program's module is then
  the D3D path's for the same draw, text for text
  (`a_world_program_s_module_is_the_d3d_path_s`), position math included.
  Stage 4 cluster lights now use the same packed fetch and program vertex buffer.
  That left the speckle as it was (w13), so it is not the code. What
  remains is the texels: the D3D path uploads a DXT bitmap whose base
  size is whole blocks as BC on a host that takes BC
  (`gfx::texture::decode_texture`, `bc_supported`), and the GPU decodes
  it, while the native stage bitmaps were decoded on the CPU
  (`gfx::dxt`), which truncates the interpolated colours (`(2a + b) / 3`
  rounded down): one level darker on some texels, never brighter. The
  native stage bitmaps now keep their BC blocks too
  (`assets::decode_image_blocks`, `Image::with_blocks`) and upload them
  where the device takes BC, on the D3D path's rule (no signed channel:
  `bias_signed` drops them). The compare prints each stage's native
  upload beside the D3D texture (`program::describe_image`: format,
  size, levels, a hash of level 0). That removed the speckle (w15:
  03000's mean 0.203 to 0.009; clusters alone match exactly but for
  02000's curb). The compare's first draws are the world's first passes,
  while the game may still be streaming bitmaps at low detail: 03000's
  dump saw the D3D path bind 2x8 and 4x8 DXT stand-ins where the native
  stage has the 64x256 bitmap (S: low-detail stand-ins, since the
  later frames match native to a mean of 0.009), so `H2_NATIVE_WORLD_DUMP` takes `@passes` to arm
  later. It now lists only the pixel shader and option fields that
  differ, the D3D draw's stipple pattern when it masks anything, and the
  pipeline's primitive (topology, winding, cull, fill) and depth write,
  compare and bias as `gfx::translate` derives them from the D3D draw's
  render state, against the native pipeline's (`program::pipeline_state`).
  On 03a's 03000 that showed the girder's edge (43 px, placement 282):
  the D3D draw culls `Front` (`CULL_CW`, front face `CW`) and the native
  pipeline nothing. Material 107's layer-3 implementation does not set
  `CULLMODE`, so it inherits the device's, and the native pass culled
  nothing there: an open, single-sided girder seen from behind drew its
  back faces. Such a pass is now marked (`look::WorldPass::inherits_cull`)
  and takes the device's `CULLMODE` and `FRONTFACE` at the layer-3 call
  (`Device::cull_state`, read from the guest's render-state array as the
  stencil is; `program::InheritedCull`). That made 01000, 03000 and
  04000 match exactly (w19). The clusters take it too: 02000's 81
  cluster pixels on a curb (box [120,327]-[235,374]) went with it and
  nothing else changed (w21), so that is the default;
  `H2_NATIVE_WORLD_INHERIT_CULL=placements` gives it to the placements
  only and `=none` to neither. The other fields
  the compare listed change nothing: `explicit_final` is the device's
  final-combiner latch, which only matters when both final-combiner
  input words are zero (`PixelShaderDef::has_final_combiner`), and a
  world pass's are not (the module is the same either way, a test); the
  D3D draw's `stipple` option with a pattern of all ones masks nothing
  (the compare prints the pattern only when it is not all ones);
  `fog_color` and `bump_mat`/`bump_lum` are read only by a combiner that
  reads fog and by the bump-environment texture modes, which
  `world_parity` refuses. Without a material the dump arms every
  material the native pass draws the placement with, and each D3D draw
  is compared with the one whose pixel shader it is
  (`uniform_compare::pick`, fewest differing definition and option
  fields): the placement's materials can share a vertex program, so 03000's
  second kind of draw (another pixel shader, an alpha test) was compared
  with material 107's. A D3D draw that pairs with no material exactly is
  one the native pass may not draw at all.
  For the placements' edges,
  `H2_NATIVE_WORLD_DUMP=282[:107]` (with `H2_NATIVE_WORLD_OWNERS=clusters`,
  so that the D3D path draws the placement) prints, once per loaded
  world, the uniforms the native rigid draw of that placement and
  material would take, and arms `d3d::uniform_compare`: the D3D path's
  next draws with the same rows `c[50..52]` and the same program print
  every constant the program reads, every input it reads (streamed by
  one and not the other, or with other defaults) and every vertex or
  pixel uniform that differs, then the input fetch and interpolation,
  the pixel shader definition and options when they differ, and each
  sampled stage's D3D texture and sampler. On 03000 (placement 282,
  material 107) the layer-3 draw matched in every constant and input
  the program reads; its pixel side differed only in `fog_color` and
  `bump_lum`, which no native pass reads (`world_parity` refuses a
  combiner that reads fog and the luminance bump modes). The draws that
  also differed in `c0`, `c1` and `alpha_ref` share the rows and the
  vertex program but are another layer's. A view whose draws are not
  W-buffered (`SET_CONTROL0` without z-perspective) is left to the D3D
  path (counted in the log): the programs write and clip w. The trace
  found it: every cinematic pass culled 0 of 1449 draws (and drawing
  them all changed no pixel), because the world was the wrong BSP. A
  level's BSPs load at one address (03a's intro and its first play space
  both at `0x800e6010`), and the glue loaded the first scenario BSP at
  the active BSP's address. The play space's parts matched some of the
  cinematic BSP's items by cluster, part and shader, so those items were
  skipped and nothing drew them: the slivers. The world is now keyed and
  loaded by the game's active BSP index (game globals `+2`, `i16`;
  `native_world_glue::active_bsp_index`), which must load at that
  address. A placement's placed copy is
  unscaled, as the game draws it (`load`), so its bounds are where it
  draws. The
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
(`0x0002cbf0` fills it); its `+4` is an object's render model tag datum
(not its handle; see "Stage 2" for how a record names its object), `-1`
for the BSP:
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

The structure draw also switches the stencil around an item in layers 1
and 3 (`0x0004d0b0`; modes by `0x00015370`'s table at `0x00015658`):

- before the item (`0x0004d640`): either flag byte `0x00485a75`,
  `0x00485a76` set, mode 0 (stencil off); else, the render record's `+0`
  bit 14, mode 2 (`ALWAYS`, reference `0xff`, `REPLACE` on pass, masks
  `0xff`). Bit 14 is the item's visibility flag `0x200` moved up
  (`0x001332f0`), the same for clusters and placements;
- after it (`0x0004d720`), on either condition, mode 1 (the same with
  reference 0).

An item with neither draws with the stencil as the layer found it.

The feature reads how layer 3's items switch it before asking the host
(`skip::stencil_plan`: the flags, the marked BSP sections by cluster or
placement, whether any item switches; unreadable, the D3D path draws the
view, `native_world::stencil_views`). The native pass draws its parts
before the layer's items and clears their items, so their switches do
not run; the glue (`native_world_glue::pass_stencil`) makes that exact:

- no item switches: every part draws with the stencil the device holds
  when asked (`Device::stencil_state`, translated as the D3D path's
  pipeline translates it, `gpu::PassStencil`);
- some item switches: an item leaves mode 1, so the layer must start in
  mode 1 (`PassStencil::is_structure_mode`) for the items left to D3D to
  draw as they would; else the D3D path draws the view (counted in the
  log). Then with a flag every part draws with the stencil off (mode 0),
  and otherwise a marked section's parts draw with reference `0xff`
  (mode 2, its own run in the batch, `PassStencil::reference_of`), the
  rest with mode 1.

Each part writes what its own item would where it is drawn (`ALWAYS`,
`REPLACE` on pass): which part is last at a pixel is the depth test's,
as on the D3D path (S: coplanar ties may differ). The native pass sets
no device state, so the layer's D3D items and everything after it see
the state the layer started in: mode 1 whenever an item switches, which
is what the game's last switching item leaves (`0x0004d720`).
`native_world_glue`'s test checks the per-mode writes and that state
against `engine::render::stencil::for_mode` over every stored value.
Nothing after layer 3 tests the stencil from engine code: every caller
of `0x00015370` passes mode 0, 1 or 2 (the light passes 0), and only
`0x00015370` and the quad `0x0003bf20` set `STENCILFUNC`
(e3-lighting.md, "Stencil: every use, named"). The transparents and
objects still draw without the stencil (S).

**Only the world's layer 3.** The layer draw also runs for the
first-person render (`0x0003fdf0`, after the transparents): layers 1,
3, 8, 20 and 7 of a list holding the player's first-person models only,
under its own projection, with `0x00485a76` set by its caller (so the
stencil plan says "off") and no depth clear under that flag
(`0x0004004f`..`0x00040066`). About a third of all layer-3 draws are
these. The feature asks for the native pass only when the layer's list
holds a BSP item (`skip::has_structure_items`); otherwise the whole
world was drawn again over the finished scene with the first-person
constants, nearer than the scene's depth: decals, the dynamic lights'
`ZFUNC` equal passes and the transparents (03a's fire and smoke) were
painted over with bare world while the walls looked right. That, not
depth or stencil, is what the 2026-11-19 A/Bs at 03000/04000 showed;
refusing stencil-switching views hid it only because the first-person
view's flag counted as a switch.

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

That dependency is not only layers 7 and 8: on 03a every world template
has, after layer 3, shadow application (11), the lights (12, 13) and
planar fog (16), all `ZFUNC` equal (`0x202`) without depth writes
(`h2 shader`). The 2026-11-19 A/B (the walls right, the palette fixed)
lost decals and signs, a doorway's light and the fire's light on the
rocks; most of that was the world drawn again in the first-person
render's layer 3 ("Only the world's layer 3", above), not depth (S: the
speckle on the rocks may still be). What the native pass must match,
per part:

- **Clusters**: classification 0 of the pass's vertex program, `v0`
  the section's position. The world keeps a cluster's position bits as
  decoded (`WorldBuilder::add_section`, no identity arithmetic: `0 + -0`
  is `+0`, `x + 0·inf` is NaN), and the program, view constants
  (`c[0..18]` at the issue) and W depth (`floor(w)/16777215` for D24,
  `xps_w_depth`) are the D3D path's.
- **Instanced placements**: the structure draw takes the rigid
  classification for them (render record `+0` bits 3 and 12, set by the
  placement caller `0x001336d3` of `0x0002cbf0`; the cluster caller
  `0x00133504` sets neither), whose program decompresses `v0` with
  `c[170..171]` and places it with `c[50..52]`; `0x0004d830` uploads
  those rows from the matrix at render record `+0x1c`, which for a
  placement is the placement's tag element (`[sbsp+0x144] + 0x58·i`):
  forward, left and up by column and the position, copied bit for bit,
  the scale at `+0` not applied. The native pass does the same: the
  load reads each placement's rows from its tag element
  (`world::model_rows`, `World::rows`), the material's pass is compiled
  with classification 1 as well (`look::rigid_program`,
  `program::compile_rigid`, which requires that it read `c[50..52]`;
  `program::rigid_fit`), and each placement's parts draw with it from
  the definition's vertices as their streams hold them
  (`World::program_vertices`), the placement's uniforms at its slot's
  dynamic offset in one buffer per material (`program::rigid_vertex`,
  the frame's view then the rows). Within a batch the draws sort by
  owner, so the clusters draw as one run and each placement as another.
  `tests/model_rows_differential.rs` runs the lifted `0x0004d830` on
  seeded placements and checks the floats it hands the constant upload
  `0x003f7330` (at `c[50]`, twelve) equal the port's, bit for bit. A
  material without a rigid program (or whose placements lack a stream
  it reads) keeps its placements on the D3D path, by reason in the
  load's log; the lights take only materials with none left there.

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
  off by default (the A/B on screen showed the stage 1 look, base map
  times lightmap, far from the D3D path), `H2_FEATURES=impl.render.native_world=on`
  turns it on, exact key). `LAYER_DRAW` is an opaque point, so the hook is one
  observer before it: it first sets back the bits the last layer 3
  cleared (on items that still hold the mask it left), then, with layer 3
  current, reads the camera and the active BSP (`0x004e0348`) and asks the
  host; when the host accepts it clears layer 3's bit on the BSP items
  whose part the world draws (`skip::clear_parts`, below). It sits inside `render-timing`'s
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
  runs on the render thread; off by default, and on with pipelining placed; the
  switch off places nothing; the skip and its restore at the next layer
  draw on synthetic memory, a rebuilt list left alone, and a part left to
  D3D keeping its bit though its shader is drawn);
  `crates/boot/src/render_thread.rs` (the default plan's `LAYER_DRAW`
  is render-thread safe).
- **Boot glue** (`crates/boot/src/native_world_glue.rs`). The host: on a
  new (map name, active BSP address) a worker opens the loaded map
  through the VFS with its shared maps, picks the scenario BSP whose tag
  data sits at that address, builds the world, decodes each lightmap page
  (with its mip chain), and translates each material's layer-3 pass
  ("Order of work" 4). A material is native only when its pass translates
  and every part of it can be drawn with it; every other material is left
  out of the world and keeps its D3D draws, never stage 1's look. The skip
  goes by part: the host returns each drawn part, by cluster or placement
  and part index, with its material's shader, and `skip::clear_parts`
  clears an item only when its part is one of them with that shader. An
  item names its part exactly as the build `0x0004cbf0` reads it
  (`0x0004cec1`..`0x0004cf21`): render record `+0` bit 12 clear, the
  cluster at `+0xc` bits 9..17 (`[sbsp+0xa0] + 0xb0·cluster`, its section
  at `+0x50`); set, the placement at bits 18..28 (`[sbsp+0x144] +
  0x58·placement`, its definition at `+0x34`, `[sbsp+0x13c] +
  0xc8·definition`, section at `+0x50`); then the item argument's low
  byte indexes the section's parts (`[section+4] + 0x48·part`, the order
  `tags::geometry` decodes and `native_render::load` keeps, undrawn parts
  included). So a material that shares its shader with one left to D3D
  is native too (until 2026-11-19 the skip went by shader, and 70 of
  03a's materials, 690 parts, stayed on D3D for that alone). The lights'
  split still clears by shader (`feature::native_lights`), so the lights
  take only the materials whose shader is native whole. Each layer-3 view then asks the device for
  a `WorldPass` with the view's camera; the first pass uploads the world
  on the render thread. Lines per load:
  `native-world: N of M materials draw natively with their own programs
  (P of Q parts); K stay on the D3D path for R reasons`, then one line per
  reason, most parts first, `native-world: D3D keeps K materials (P
  parts): <why>; e.g. <a shader that has it>`, then `native-world: <map>
  BSP at <addr>: N draws in B batches, P parts native, in S s`; at the
  upload `native-world: N materials draw with their own programs`; every
  1,800 layer-3 views: `native-world: D of V layer-3 views drawn natively,
  I world items skipped`. Said once if it ever happens (it should not):
  `native-world: a pass came without the device's view constants; it
  draws nothing`, `native-world: N batch draws left out: their material
  program could not be bound`.
  With `impl.render.native_world_programs=off` (the A/B against stage 1
  only) every material whose base map decodes is drawn as base map times
  lightmap, as stage 1 first did.

### The live A/B

The same 03a frame with and without the switch, E1 off, frame stats on.
What passes: the `native-world:` load line names `03a_oldmombasa`; the
views line shows every layer-3 view drawn natively; the frame shows the
world (no holes where the BSP was, objects and transparents in front of
and behind it as before); `cpu` and the `0x0002bed0` site (layer 3) drop
by most of the world's share of the 8.26 ms, and `encode` and `submit`
drop with the D3D draws that are gone, in proportion to the parts the
coverage line says are native.
A frame where the device's `external_passes_dropped` grows has a world
hole: it means the pass was asked for and could not be drawn.

The stage 1 look (base map times lightmap, on by default until
2026-11-19) was far from the D3D path on screen, lighting most of all:
no detail maps, no bump, none of the material's constants. The world is
now drawn only with each material's own translated pass, and a material
whose pass does not translate stays on the D3D path, so with any mix the
frame should match the D3D path's. Where it still differs, on native
surfaces only (the D3D ones are the game's own):

- **speckle or holes on world surfaces**, or objects showing through:
  depth. Layer 1 (D3D) wrote the world's depth and the native layer 3
  tests `LessEqual` against it, so the native W depth must be the D3D
  path's bit for bit; one step deeper fails the test;
- **a whole material too dark, too bright or flat**: its lightmap (the
  P8 page through its palette, bound at stage 3), its bump map (`P8Bump`
  through the device palette at stage 2; black walls with bright pillars
  was the bump sampled as its index) or a constant;
  `impl.render.native_world_programs=off` shows that material in the
  stage 1 look for comparison;
- **shimmer or blur at a distance**: sampling (the bump stage is 4x
  anisotropic; mip chains);
- **cut-out foliage or grates filled in or missing**: the pass's alpha
  test (03a's alpha-tested world passes pass below 255);
- **stencil-driven effects** after layer 3: the native pass writes the
  device's stencil as the layer starts and mode 2's mark on marked
  sections, and leaves views to D3D where an item switches the stencil
  and the layer does not start in mode 1 (no engine code after layer 3
  tests the stencil; a tag's or engine render-state list could, S);
- **decals, lights, fire and smoke gone, walls right**: the world drawn
  over the finished scene by a native pass in a layer 3 that is not the
  world's (the first-person render's; `skip::has_structure_items`).

### Next: matching the look

Step 4 below draws every native material with its own translated pass
(above, "Match the look"), and a material whose pass does not translate
stays on the D3D path: there is no hand-written look in between, and the
stage 1 look (the base map by a name rule, times the lightmap) is kept
only behind `impl.render.native_world_programs=off` for the A/B.

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
| `v5` | the lightmap bucket's signed lighting vector (stream `30`, [lighting.md](../formats/lighting.md)) | bucket stream's words, decoded in the shader as the D3D path does (`NORMPACKED3`, not the CPU's midpoint decode) |
| `v6` | lightmap coordinate | bucket stream `1f` |

and these constants (hardware numbering, D3D register + 96):

| Constants | What | Set by | For the native pass |
|---|---|---|---|
| `c[0..3]` | view-projection rows (`dph` with `v0`) | per view, in one block upload | the device's values at the pass (below) |
| `c[5].w`, `c[6]` | the scale that packs a unit vector into a colour (S: `0.5`, `n·s + s`); a world-space vector (dotted with the tangent frame into `oD0`) | per view | the device's values |
| `c[10]`, `c[11]` | the viewport scale and offset (`oPos` fix-up) | per view | the device's values |
| `c[12..17]` | six registers in one block | `0x00025c70` | the device's values |
| `c[18..20]` | stage 0..2 texture-coordinate scale (xy) and offset (zw) | the instance (`0x00018e80`) | `tags::material`'s instance constants |
| `c[50..52]` | model transform rows | `0x0004d830` from the render record's matrix, skipped for clusters (`[record+8] == -1`) | identity for clusters; for a placement (rigid program) its rows per draw, `world::model_rows` |
| `c[170..172]` | position and texcoord decompression (scale, offset) | `0x0001cdd0`; `0x0004d7c0` passes no compression for the BSP (record `+4` is `-1`), so the default upload (`0x0001cf29`, scale 1, offset 0) | `c[170]` all 1, `c[171]` 0, `c[172]` `(1, 1, 0, 0)`: BSP vertices are uncompressed |

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
   bind group per material. Built as `native-render::program`, and the
   native world's only look: `impl.render.native_world_programs` is on
   with the world (exact key; `=off` brings back stage 1's look for the
   A/B against it, nothing else):
   - `look::layer3_pass` takes the pass's implementation as above and
     its own render states (`pass_state`, as transparents do): depth test
     and write, colour writes, alpha test and cull. The 03a world passes
     set `COLORWRITEENABLE` (all four channels), `ALPHABLENDENABLE` 0 and
     `ZWRITEENABLE` 1; some add an alpha test (`ALPHATESTENABLE`,
     `ALPHAREF`, `ALPHAFUNC`), and those that sample the texaccum (the
     `_combined` templates) set `ZFUNC` equal without depth writes. A
     state the pass does not set is inherited from the previous draw on
     the D3D path (S, not traced): `ZFUNC` is taken as `LessEqual`, and
     an unset `CULLMODE` takes the device's at the layer-3 call
     (`InheritedCull`, above; nothing where it does not read). It refuses: engine-driven render or stage states, a
     blending pass, fog enabled, stencil, depth bias.
   - `program::compile_world` translates the pass (inputs `v0..v6` from
     the world vertex: position, uv, normal, binormal, tangent, lighting
     vector, lightmap uv) and fills the instance constants by their
     written components, `c[172] = (1, 1, 0, 0)` and identity model rows
     `c[50..52]` (the vertices are already placed). It refuses:
     per-draw engine inputs, a program reading other inputs (but those
     the pass sets itself with `SET_VERTEX_DATA4F_M`, `0x00018e3b`: the
     resolved `vertex_defaults`, read from `xvs.defaults[n]`; 03a's
     transparents read `v10` and `v11`, two colours, mask `0x0c00`), a
     translation warning, a non-2D fetch, a sampled stage with nothing
     bound, signed channels on the lightmap; and (`world_parity`, the
     constants check) anything the D3D path takes from device state the
     native pass does not have:
     - a vertex constant outside the view's `c[0..18]`, the instance's,
       `c[50..52]` and `c[172]` (the structure draw uploads `c[21]`,
       `c[24]`, `c[31]`, `c[174]` and others per draw), or one addressed
       relative to `a0.x`. 03a's world programs (`lightmap_perpixel_*`,
       classification 0) read `c[0..3]`, `c[5..11]` (view), `c[18..20]`
       (instance) and `c[172]` only;
     - fog: a combiner that reads the fog register, or a definition
       without a final combiner (D3D builds one from the fog and specular
       states). The pass does not set `FOGENABLE`, so the device's fog is
       whatever the previous draw left; 03a's world passes program their
       own final combiner and read no fog (the world's fog is a later
       layer), so none is refused for it;
     - texture modes that read device state: bump-environment matrices,
       the eye of the reflection modes, BRDF;
     - border addressing on a sampled stage.
   - `program::world_fit`: every part of the material supplies every
     vertex register the program reads (`WorldDraw::inputs`: a section
     without lighting vectors or a tangent frame would read zero), and,
     where the program samples the lightmap, has a page that decoded. A
     stage bitmap must decode and be swizzled (a linear texture is
     addressed in texels on the D3D path, `tex_scale`), and so must a
     lightmap page. One part that cannot be drawn keeps the whole
     material on the D3D path (the skip could go per part; the material
     is the unit the coverage is counted in).
   - Palettized stage bitmaps. `tex_bump`'s bump stage (stage 2) is
     `P8Bump`, and `P8` is palettized too: the tag has no palette, and
     `assets` decodes both as their index in every channel. The D3D path
     samples them through the device palette bound at their stage, so the
     native pass expands them through the same one: the load gets the
     palettes bound at stages 0..3 when it starts (`Device::stage_palette`,
     read-only), and refuses a palettized bitmap at stage 3 or at a stage
     with no full 256-entry palette. One decoder does this for every pass
     loaded with the world (`native_world_glue::StageDecoder`: layer 3
     and the transparents' layer-15 passes, which share its images; the
     objects can take it too): an image is expanded once, and another
     stage shares it only when it holds the same palette; a refusal for
     the stage's palette is not the image's, so another stage may still
     take it. Each ask checks that the palettes the load used are still
     bound, and otherwise leaves the world to the D3D path (said once). Research (Code, `h2 disasm`): `SetPalette`
     (`0x3f84d0`) is called with the default palette (`[0x484DBC]`) on
     every stage at boot and by `0x00015b70`, which also clears the
     per-stage cache at `0x484DC0`; `0x00015c90` binds a lightmap's
     palette and its one caller is the structure draw (`0x0004d416`), at
     stage 3; `0x00044628` puts stage 0 back to the default. So for the
     world's layer 3, stages 0..2 hold the default palette. Before this
     (the first 03a A/B with programs on, 2026-11-19) the bump map was
     sampled as its index, a grey: the bump term `bx2(t2)·L` was wrong,
     `r0` clamped to black on most walls and floors and came out too
     bright on others. The load names the native materials that sample a
     palettized bitmap (`sample a palettized stage bitmap`), the first to
     compare in an A/B.
   - Samplers are made as the D3D path makes them
     (`gfx::translate::SamplerKey::from_stage`: anisotropy, `MAXMIPLEVEL`
     as the smallest LOD, address 5 clamping to the edge), and
     `D3DTSS_MIPMAPLODBIAS` goes into the pixel uniforms' `lod_bias` as
     `d3d::nv2a` puts it. Lightmap pages keep their mip chains.
   - Per frame the device's `c[0..18]` (the external pass's
     `view_constants`), the guest and texel sizes (surface and the
     render-scale shift) and the W-buffer's depth scale (`w_clip`,
     `depth`, `depth_range`). A program batch draws over the whole
     target: the game's viewport is in `c[10..11]`. A pass without the
     view constants draws nothing.
   - Colour: the D3D path and the native pass both sample `Rgba8Unorm`
     (or compressed unorm) textures and write the game's own target, with
     no sRGB view on either side, so there is no gamma step to match.
   - Left (S): the inherited `ZFUNC`; the decoded chain of a bitmap whose top level is
     not resident starts lower (a smaller image, the same coordinates).

   Of 03a's scenario shaders with a layer 3, about a third sample the
   texaccum (engine texture 3, the `_combined` templates) and stay on the
   D3D path; most of the rest are `tex_bump` and its variants, one pass
   with the per-pixel lightmap implementation, and translate.
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
| `0x0002cbf0` | Fills render record `0x004ba138 + 0x20·i` (at most `0x352` per frame, counter `0x004c0b78`): `+4` its second stack argument, the render model (mode) tag datum (`-1` for the BSP), `+8` a word from the caller's per-section state, `+0x10` its fifth, the object's lighting (below), `+0`, `+0xc` and `+0x1a..+0x1b` bit-packed. Its caller for objects is `0x00133f70`, which walks a per-object render state built under `0x00133ab0` (`+0` the object handle, count `+0x166`, per-section words from `+0x48` with stride 4: the lighting pointer at `+0x48 + 4i`, the mode datum at `+0x5c + 4i`, both filled by `0x0003d4f0` and `0x0004b5d0`). **The record does not hold the object handle** (the first live run's walk met 209,131 object records and resolved none by reading `+4` as a handle). Which bits are the section (LOD and permutation) is not read yet. |
| record `+0x10` → cache record → owner | How an object's render record names its object (`objects::record_owner`): `+0x10` points at `+0x44` of the object's record in the render-state cache (`0x0004b5d0`: `lea ebx, [record+0x44]`), or is 0; the cache record's `+4` is the owner's handle; the owner's body `+0xcc` holds the cache record's handle, which `0x0003ddd0` checks (`[data + 0x100·(body[+0xcc] & 0xffff) + 4] == handle`). The resolve requires all of it (pointer at a record's `+0x44`, index below the cache's high water `+0x38`, owner live by `0x000badc0`'s rules, `+0xcc` naming the record back); a record that fails stays on D3D. Code; Test on synthetic memory, Live pending. |
| `0x000badc0` | The object lookup: slot `handle & 0xffff` below `[H+0x38]`, element `[H+0x24]·slot + [H+0x44]` (12 bytes; `0x0003ddd0` uses a fixed 12), live when its `u16` salt is non-zero and equals `handle >> 16`, body at `+8`. `objects::body` follows it. |
| `0x0002bed5` | The return address of the base layers' layer-3 call (`call 0x00044550` at `0x0002bed0` in `render_scene_passes`, view kinds 1 and 2). The object skip acts only there, so first-person or other-kind layer draws are never touched. |
| `0x00040136` | The return address of the other layer-3 caller (about a third of layer-3 calls in the first live run): `call 0x00044550` at `0x00040131` in the first-person draw `0x0003fdf0`, which runs after the transparents for its own record list (`0x004c8740`, `[0x0050943c]` records) under the first-person camera and projection, then `0x000445d0`. It stays on D3D: the observer would draw it with the scene camera, and first-person models are a later stage. Counted as `first person`, not as a wrong caller. |
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

The game picks a LOD per object by pixel size, and each render record
names the section it draws (the key's bits 9..17). The parts drawn
through the game's programs are drawn by that section: an object's parts
come from the sections its records name, nothing else. So the load takes
every detail level's section of the permutations picked
(`model_load::pick_lod_sections`, highest first per region), and a
distant object draws the lower level the game picked. Before, only the
highest level loaded, and a distant object's records named a section the
pass had no parts for: its items were skipped and nothing drawn (a thin
object on 03a's rubble, gone at frames 3000 and 4000).

An object whose records still name a section none of its native parts
come from (another permutation than the one loaded, or a section whose
parts did not load) is left to D3D at the hook, before its items' skip is
kept (`NativeSet::sections`, `NativeSet::missing_section`), logged once
per definition and section. The base-map stand-in for transparent parts
still draws the highest level's.

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

Base maps go through the world's stage decoder (`StageDecoder`, one per
map's model worker): a palettized one (P8, P8 bump) is expanded through
the device palette bound, when the map's first object was asked for, at
the stage its material binds it (the first layer-3 implementation that
names the bitmap; stage 0 otherwise), as the D3D path samples it, not
drawn as its index in grey. The objects draw only while the palettes the
expansions used stay bound (checked at each layer-3 view); a stage
without a full palette refuses the material.

### Object lighting

The first lit run (03a, objects native, 4,677 of 7,202 layer-3 calls
drawn) drew the wall vent flat, bright grey-white where the D3D path
shades it dim: the fixed term above is not the game's lighting, so a
material that reads the object's lighting must not be drawn with it.
How the game lights an object's layer-3 pass (Code, `h2 disasm`;
`native_render::object_lighting` has the table):

- **The constants.** The object geometry draw `0x0004d0b0` calls
  `0x0004d9c0`, which hands the render record's lighting pointer
  (`+0x10`, cache record `+0x44`) to `0x0001bd50`: that makes it the
  current lighting `[0x00485a58]` and marks the engine inputs that read
  it dirty. The shader draw then evaluates the implementation's dirty
  inputs (`0x0001ae70`: scalars at `0x000336f0`, vectors at
  `0x00033980`), and `0x00019be0` writes each of the implementation's
  engine vertex constants (implementation `+0x124`, records of the pass's
  `+0x38`: register bits 0..3, bank bit 4, mode bits 5..9, input byte 3)
  into bank 1, uploaded as hardware `c[18 + i]` by `0x00018e80` (bank 0
  is the vertex attribute defaults, `0x00018d70`). So the lit vertex
  programs read their lighting from `c[18 + i]`, the registers their
  implementation names, computed from vector inputs 3, 4, 14..17, 21..24,
  35, 40 and 42..45 and scalars 23 and 37 (the table in
  `object_lighting`).
- **The implementation.** The build `0x0004cbf0` picks which of the
  pass's implementations an object draws from its lighting: 0 when the
  byte `+0x76` is not `0xff`, 1 when it is, 2 for a shader flag. On 03a
  the object passes' programs are the `lightmap_dynamic*` and `sh_dynamic`
  vertex shaders (`h2 tags 03a_oldmombasa vrtx`).
- **The transform.** `0x0004d830` uploads a single-node object's matrix
  (record `+0x1c`) as three registers at hardware `c[50]`.
- **The port.** `object_lighting::vector_input` and `scalar_input` give
  every lighting input. Vector 40 (all three components) and scalar 37
  are `object_lighting::root_value` (`0x00033670`): the object whose
  handle is the lighting's `+0` (`0x000badc0`, mask -1: index below the
  array's count, salt non-zero and equal), walked up its parents
  (`+0x14`) while its byte `+0xaa` is set, the elements read straight
  from `[0x004e0300] + 0x44` at a stride of 12; the root's `+0x2b0`
  clamped to 0..1, or 0 when a handle does not resolve. Both read
  `[[0x00485a58]]` unchecked, so without a lighting they are not given.
  The value is taken with the lighting's words at hook time.
  `tests/object_lighting_differential.rs` checks every input bit for bit
  (NaN for NaN) against the lifted `0x00033980` and `0x000336f0` on
  seeded states, 40 and 37 over a seeded object array of parents,
  salts and empty elements. The hook copies each drawn object's `0x78` lighting
  bytes and the inputs' defaults into its `ObjectFrame`
  (`objects::object_lighting`: body `+0xcc`, the record still its own),
  so the pass has the frame's values whatever the cache does next.
- **The refusal.** The native models pass does not apply them: its
  program is ours, not the game's `lightmap_dynamic`/`sh_dynamic`. So no
  opaque material is drawn by it (below): one none of whose layer-3
  implementations reads only what the object path gives is refused at
  load and keeps its D3D draws (`object_program::program_refusal`, which
  names what each implementation reads that the path cannot give, or says
  it has no layer-3 pass).

### Object programs

Every opaque object material, whether or not its layer-3 pass reads the
object's lighting, is drawn with the game's own vertex program and pixel
shader (`native_render::object_program`, `object_program_gpu`), or left
to D3D, each piece with synthetic tests. The models pass's base-map
stand-in (base map, fixed ambient and directional light, posed from the
object's own node matrices) is no longer used for opaque parts: on 03a's
intro (frame 02000) the scarab's materials that read no lighting were
drawn by it, flat and dark where the game lights them, and posed apart
from its parts drawn through their programs, which use the rows its
records upload and matched D3D pixel for pixel. The native set accepts a
shader per definition (`NativeSet::shaders` holds `(definition, shader)`):
a material one model draws through its programs may be refused for
another whose sections take other programs, and that one's items keep
their D3D draws.

A material with no layer-3 pass ("it has no layer-3 pass" in the load
log) is not drawn at layer 3 at all: its items sit in other layers, which
the objects stage does not touch, so they keep their D3D draws while the
definition's layer-3 materials go native. On 03a masterchief
(0xf40a1293) and 0xe703058e each have one, beside a material whose every
implementation samples engine texture 3.

- **The texture accumulation target** (engine texture 3, Code). The
  binding `0x00019530` takes for 3 (`0x000195b2`) binds render target 1
  through `0x00014f60` (its kind, the byte at `0x00015154 + 1`, takes
  `0x00015044`: the texture `0x0001dcc0(1)` returns, the descriptor
  `0x004b4b58 + 0x98` and its texture at `+4`, valid while `+0x88` is set
  and `+0x95` clear) and writes the target's size (`+0x80`, `+0x84`) as
  the stage's size, which engine vertex constants of modes 5..20 read.
  Layer 1's `_combined` passes draw into it; layer 3 samples it in screen
  space, a linear texture addressed in texels.
  - `object_look_stages` takes the stage out of the look
    (`EngineBound::texaccum`); it compiles as a 2D stage without a bias.
  - The pass asks the device for the level (`ExternalPass::sampled`, the
    header `TEXACCUM_HEADER`); after the drain the device finds the
    render-to-texture level over it as a draw binding it would
    (`Device::rtt_level`, in place, nothing written back) and the backend
    gives the pass that copy's view (`ExternalTarget::sampled`, the scaled
    copy's under the render scale). A view whose objects need it is left
    to D3D when levels are not sampled in place
    (`Device::samples_levels_in_place`), and an object whose parts need
    it is left to D3D at the hook, before its items' skip is kept, when
    the target is not bound (`object_nodes::texaccum_bound`, the check
    `0x0001dcc0` makes; `NativeSet::texaccum`). If the device still finds
    no level after the drain, those parts go undrawn for that view: logged
    once and counted.
  - Per draw the stage's coordinate scale is one over the level's guest
    size (`texaccum_scale`), and the engine vertex constants that read
    the stage's size (`ResolvedImplementation::dimension_bindings`, which
    the resolver left 0 by 0 for an engine texture) get it
    (`texaccum_dimensions`).
- **The probe.** `H2_NATIVE_OBJECTS_PROBE=x,y` (guest pixels) prints, on
  the first eight passes and every 250th after, each object of the pass
  whose bounding sphere covers that pixel: its definition and handle,
  the (section, implementation) its records draw, the sections its native
  parts come from, whether it samples the texture accumulation target,
  and the level's size. A section in the first list but not the second
  is drawn by nothing. Then, per lit part of each such object, how the
  pass draws it (`LitObjects::describe_parts`): its section and shader,
  the implementation its records pick and that program's depth, blend,
  alpha test, cull and colour writes, and for a part sampling the texture
  accumulation target its stages, the level's guest size and the size
  constants it gets; or why it is not drawn.
  `H2_NATIVE_OBJECTS_EXCEPT=0xe92207ad,...` leaves those definitions to
  D3D (the load refuses them), to tell which one a difference is.
- **The comparison.** `H2_NATIVE_OBJECTS_COMPARE=definition:part[:after]`
  (e.g. `0xe92207ad:2:30`) is the objects' `H2_NATIVE_WORLD_DUMP`: the
  definition's objects keep their D3D draws (`NativeSet::shadowed`: the
  host gets their frames, their items are not skipped), and on the pass
  after `after` holding one, its first object's part is armed in
  `d3d::uniform_compare` with what the pass would draw it with
  (`LitObjects::part_uniforms`: program words, vertex and pixel uniforms
  with the object's rows, decompression, lighting and texture
  accumulation constants, translation options, pixel shader and options,
  samplers, primitive, depth). The D3D draws with the same rows
  `c[50..52]` and the same program are compared field by field; those
  with another program (the object's other parts, and its earlier
  layers, which wrote the depth a later `Equal` pass tests) are named
  with their cull, winding, depth state and `c[0..4]`.
- **The inherited cull.** A lit part whose layer-3 implementation does
  not set `CULLMODE` (`object_program::sets_cull`; 03a's
  `concrete_rebar` part of `concrete_chunk_j`) draws on the D3D path
  with the device's cull and winding at the layer-3 call, as a world
  pass does. Its program is marked (`MaterialProgram::inherits_cull`);
  the host reads `Device::cull_state` when the feature asks for the set,
  and the pass draws those parts with it
  (`LitObjects::set_inherited_cull`, through `program::pipeline_state`;
  the cull is part of the pipeline key). Before, they culled nothing.
  `H2_NATIVE_OBJECTS_INHERIT_CULL=0` turns it off.

- **The vertex block.** What the game's upload would hold for one drawn
  object: `c[0..18]` the view's; `c[18 + i]` the implementation's
  constants with every engine binding that reads the lighting put in from
  the port (`object_vertex_uniforms`: the vector inputs through
  `ResolvedImplementation::vector_bindings`, the scalar ones through the
  new `scalar_bindings`, both keeping only what later bindings leave);
  `c[50..]` the node rows; `c[170..173]` the decompression.
  `inputs_supported` refuses an implementation that reads any other
  per-draw input (time, the camera, object functions) or puts a lighting
  input into a combiner constant (that would need a per-object pixel
  block).
- **The node rows** (`node_rows`, Code): a record with a matrix at
  `+0x1c` uploads it transposed, without its scale, as three registers at
  hardware `c[50]` (`0x0004d830`). Otherwise `0x0004d8e0` reads the
  record's `+8` node block handle (-1: nothing is uploaded, so the object
  is refused): the block at `0x00487b1c + (handle & 0x0fffffff)` holds per
  part (`+0xc & 0xf`) a first matrix and a count at `+4 + 4·part`, and the
  matrices at `+0x44 + 0x30·n`, twelve floats in register order. A model
  record (`+0xc` bits 29..31 = 1) whose section names a node
  (`0x0004cb80`: the `mode`'s sections, stride `0x5c`, the i16 at
  `+0x2c`) uploads one matrix (the part's first when it has one, else
  that node's); anything else uploads the part's whole palette
  (`0x000152a0`), `c[50 + 3·k]` for matrix `k`, which the skinned program
  indexes with `a0.x`. A palette that would run past `c[170]` is refused.
- **The decompression** (`decompression_rows`, `0x0001cdd0` from the
  section setup `0x0004d7c0`): `c[170]` half the position bounds' extent
  (w 1), `c[171]` their centre (w 0), `c[172]` the texture coordinate's
  scale and offset; the identity where the section's flag is clear.
- **The program** (`object_look`, `compile_object`): the implementation's
  look with its engine constants allowed, translated as the world's are;
  every constant read directly must be one of the above, and a read
  relative to `a0.x` must start in the node rows.

- **The vertex streams** (`object_streams`, Code). The draw sets its
  streams through `0x0001caa0`: the section's `vrtx` classification
  (`0x0001cb20`: the geometry classification at section `+0x18`, 0..2 as
  they are, 3 the influences byte `+0x14` plus 2 when the tag has that
  many, else 0), its program, and the streams (`0x0001c8c0`): the
  section's vertex buffer descriptors in order, each stream's format the
  record of its type in the table at `0x0043f408` (`0x15` bytes a type:
  the type, then up to ten (semantic, format) pairs; semantic `0xfe` skips
  bytes, any other negative one ends it). The classification lists per
  register `v<i>` the semantic it reads; the declaration puts each
  attribute in the register reading its semantic. The format codes are
  the Xbox vertex types in order (float 1..4, packed bytes 1..4, shorts
  1..4, normalised shorts 1..4, normalised packed 11:11:10, `D3DCOLOR`).
  The table is read from guest memory when the map is first seen, never
  copied. A section's streams are gathered once per declaration, each
  register's guest bytes in a 16-byte slot (`object_streams::gather`),
  and the vertex unit converts them with the formats and shader fetch the
  D3D path's default (GPU) fetch uses (`object_program_gpu::vertex_fetch`,
  `input_fetch`; `NORMSHORT3` as `snorm16x4` read `(x, y, z, 1)`, and so
  on). They were expanded to floats on the CPU before, and a CPU division
  can round another way than the GPU's normalised conversion: a part
  whose layer-3 pass tests depth `Equal` against the depth its D3D pass
  wrote with the same program (03a's `concrete_rebar`, part 2 of
  `concrete_chunk_j`, at frames 3000 and 4000) then failed wherever w
  differed in the last bit, as the world's packed normals did.
- **The implementation** (`record_implementation`, Code `0x0004cbf0`).
  The build of one record's draw switches on its third argument, the
  draw (the jump table at `0x0004d054`, indexed by the bytes at
  `0x0004d068` from draw 3). At draw 3, layer 3, it takes the record's
  `+0` bits 29..31 (`0x0004cc70`, the table at `0x0004d07c`: 0..4 as they
  are, 5..7 give 3), overriding what `0x00015d70` chose. The lighting's
  `+0x76` and the template's layer-15 bit (byte `+3` of its level's
  layer mask) pick the implementation only at draw 18
  (`0x0004cc9f`..`0x0004cd46`), not at layer 3. The hook copies each
  section's implementation with its rows. Per material each implementation
  of its layer-3 pass that compiles for every section it is in, with its
  stage bitmaps decoded (linear textures refused, as the world programs
  do), is drawn; one that does not is left out with its reason, and a
  material with none stays on D3D whole. An object whose records pick an
  implementation some lit part of its definition lacks stays on D3D.
- **The normalization cube.** A stage sampling engine texture 1 is bound
  to the vector normalization cube the lights load
  (`native_lights_glue::light_textures`, the globals' rasterizer data),
  sampled as a cube without a bias, as the lights sample it.
- **The lightmap stage** (`object_program::OBJECT_LIGHTMAP`, Code). An
  object's draw binds no lightmap page. The draw (`0x0004d0b0`, draw 3)
  sets the current page `[0x00485a60]` to -1 for every record with a
  model datum at `+4` (`0x0004d2c4`..`0x0004d2de`, `0x0004d362`, then
  `0x0004d420`); only records without one (the BSP's instanced geometry,
  `0x0004d367`) get a cluster's page and palette through `0x00015c90`.
  With the page at -1 the stage setup (`0x00018c43`..`0x00018c4c`, then
  `0x00018d1f`) binds the globals' rasterizer data element's bitmap at
  `+0x64` (`[[0x00485a80] + 0x64]`) and no palette. The coordinates the
  draw adds for it come from the BSP: for a record whose implementation
  is not 2 or 3 and whose `+0xc` bits 18..28 are not `0x7ff`, the BSP's
  `+0x64` element (stride 12) at that index holds per section a u16 that
  `0x0004dc60` turns into the vertex offset of a stream of type `0x2e` in
  the BSP geometry block its `+2` names (`+0x44`, stride `0x38`); the
  streams step (`0x0001c8c0`, `0x0001ca25`) adds it with its own type's
  format. The host loads that bitmap as stage 3 decodes it and takes it
  only when it is uniform (every texel of every level the same): then it
  samples the same wherever the coordinates fall, and the lit build feeds
  the stream as zeros instead of reading the BSP. A bitmap that is not
  uniform keeps implementation 0 on D3D, with the reason; reading the
  BSP's stream per object section is what that would need.
- **Combiner constants** (`object_pixel_uniforms`). A lighting vector
  bound to a pixel combiner constant is packed per object as `0x00019ca0`
  packs an engine value (`pack_engine_colour`: alpha 0, `(x + 1) / 2`
  first where the binding halves it, each channel rounded to 8 bits) into
  the byte lanes the binding keeps, the rest as compiled. A lighting
  scalar goes through its binding's transform first
  (`tags::material::engine_scalar`, the one `0x00019ca0` applies: the
  complement, or a cosine or sine wave over one period clamped to 0..1),
  which the binding now keeps.
- **Which implementations a map draws.** The observer counts live layer-3
  object items by their record's implementation and logs the counts once
  per map, after 600 views.
- **The node rows at the hook** (`objects::attach_node_rows`). The
  observer copies, for each record a drawn object's items name, the rows
  that record's draw would upload, one set per mode section (two records
  of a section that would upload different rows refuse the object). Under
  the pipelined render this reads the render's snapshot, as the rest of
  the observer does (below): the node blocks at `0x00487b1c..` are guest
  RAM, so they are frame `k`'s. An object of a definition with lit parts
  is drawn only with its lighting (and the root value inputs 40 and 37
  read) and every record's rows; else all of it stays on D3D.
- **The draw** (`LitObjects::encode`). After the models pass, one render
  pass over the same targets: per drawn (object, lit part) its vertex
  block (`object_vertex_uniforms` over the view's `c[0..18]`) and pixel
  block in one buffer at dynamic offsets, as the lights do; the viewport
  is the whole target, as the programs' own fix-up expects.

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
- **Diagnostics** (added after the first live run drew nothing and
  printed no `native-objects:` line at all). Every layer-3 call of the
  observer ends in one counted `Stop`: first person (`[esp]` is
  `0x00040136`, left to D3D), wrong caller (`[esp]` is neither that nor
  `0x0002bed5`), no host, no camera, no map, no list, no object items (no
  item's record names a live object), none accepted (models not loaded,
  refused, or shaders not native), none read (node count), draw refused,
  drawn. The first of each prints `native-objects: first layer-3 call
  ending in "<stop>": <detail>`; each distinct wrong caller prints its
  `[esp]` (up to four); every 1,800 layer-3 calls one line counts them
  all, with the definitions wanted and the summed walk (items, in layer,
  geometry builds, object records, live, accepted, failed writes, and up
  to four other build functions met in layer 3). A run that draws nothing
  says which return it took.
- **The first diagnosed run** (03a, world D3D) printed `items 4 in layer
  4 geometry 4 with handles 0 live 0`, then over 7,202 layer-3 calls
  209,131 records "with handles" and none live, and 2,521 calls from
  `[esp]` `0x00040136`. Both had one cause each: the walk read a render
  record's `+4` as the object handle, but it is the render model datum,
  so no lookup could succeed; the object is named through the lighting
  pointer at `+0x10` instead (research table, `record +0x10`). The other
  caller is the first-person draw, now counted on its own and left to
  D3D. The transparents' entries (stage 3) read their objects the same
  way.

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

### Other builds in layer 3

The walk's `other builds [0x0004de20 x15580]` are decorators (Likely):
`0x0004de20` is the build the layer draw's handler table (`0x00041640`)
gives the BSP's decorator placements (`[[0x004e0348] + 0x238]`, entries
of `0x18` bytes whose first byte indexes the scenario's decorator palette
at `+0x37c`, 8 bytes each with the set's datum at `+4`). They have no
render record, are not objects, and neither skip touches them (both take
only build `0x0004cbf0`): they stay on D3D, a later world stage.

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
  object's render model datum (the object itself through `+0x10`, as in
  stage 2), or `-1` for the BSP with `+0xc` bits 9..17 the cluster
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

The observer at `RENDER_LIGHT_PASSES` captures the light and lets its body
run. The guest projects its bounding sphere, sets the scissor, produces
any shader gel, selects the layer, runs the build callbacks and sorts
their output. The native decision is at the first BSP geometry handler
called from the sorted dispatch loop (`0x0004d0b0`, return
`0x0003bc90`). The owning light-layer call site and list must match.
The ported layer loop supplies a named continuation for the same call.
The original layer setup, teardown and light tail run on both paths.

`light_list::sorted` reads the dispatcher's stack array, skips its
`[[entry+0x10]+0x100] == -1` exclusions and retains its exact order,
duplicates, handler records and implementation bytes. A light with an
earlier executed handler, shadow or stencil work, layer 14, or two active
light layers stays on D3D. Each layer builds a different sorted array;
the second array cannot be confirmed at the first dispatch.

The host reserves every range on the bound reference and each required
display copy. It then advances the guest dispatch cursor with one checked
stack write and a register update, and commits the reservation. A refused
copy or failed write drops all recordings. Adjacent whole lights with the
same gel and attachments can share one load/store pass. Guest draws,
clears, a view change, light fallback and shadow or gel dependencies end a
run. The GPU keeps each light's scissor, draw and blend order. Threaded
preparation still waits synchronously for each reservation.

### What a native light draws

- **Geometry.** The gathered BSP owner, part and full material datum must
  match a resident cluster draw. The handler's selected implementation
  chooses the program. Models and rigid placements retain their D3D
  handlers. A split may consume a contiguous native prefix. All remaining
  entries run in guest order, even when one shares a shader with the prefix.
- **Shading.** Each draw takes its view constants and W range at issue,
  the light's 36 frame words in `c[31..39]`, material colours and
  inherited device culling. The guest runs the sphere projection and
  rounds its rectangle before this hook. The host reads that integer
  device scissor, intersects the viewport and scales the half-open
  rectangle independently for each copy. Empty rectangles cover no pixels.

- **Shadows.** Layer 10's casters into a depth texture of our own from
  the light's projection (`0x00048ee0`, skipped with the call, so ported
  as a pure function), then the shadow term sampled in the light's own
  pass (`GREATER`, one tap as the D3D path's), instead of layer 11's
  stencil and alpha marks. Lights whose shadow cannot be drawn keep their
  D3D passes.

### Order of work

1. **Admission and runs** (built, parity pending). Enable
   `impl.render.native_lights` to reserve sorted cluster dispatches.
   Positive admitted and per-copy recorded ranges establish coverage.
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
   **Implemented for cluster geometry. The headless 03a A/B is pending.**
   `boot::native_lights_glue` checks the loaded map and BSP, resolves
   the sorted layer and reserves both physical copies before advancing the
   guest dispatch cursor. Placements, objects and shadow maps keep their
   D3D draws. A shader-driven gel requires a fresh target-13 binding from
   the original constants call. The consumer pins that read-only slot in
   its reservation, samples the matching physical copy and normalizes
   linear texture coordinates by the guest gel width and height. A
   missing invocation, slot or copy keeps the whole light on D3D.
   See [native gels](native-gels.md) for the producer and comparison prefix.
   - **Engine textures** (`0x00019530`, a jump table at `0x00019b68`
     indexed by id - 1; Code): ids 1, 9 and 14 are bitmaps from the
     globals' rasterizer data (`[0x00485a80]`, set by `0x00022590` from
     the `matg` block at `+0x108`: it tests the count `[eax+0x108]` and
     keeps the address `[eax+0x10c]`, the first element, or zero), the
     datums at `+0xc`, `+4` and `+0x14` (each the datum word of a tag
     reference); S from how the light passes sample them, 1 is the
     vector normalization cube map and 9 the distance attenuation volume.
     V on 03a: the block's count is 1; the bitmaps are a swizzled
     `Xrgb8888` cube and a swizzled `Ay8` volume, both with mips.
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
     draws that shader into render target 13 (shader layer 1, pass -1,
     selector 0, target 13, draw 1 at `0x00016610`, return `0x0003a5b0`,
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
     reads the light textures' bitmaps from the `matg +0x108` block
     (`light_bitmaps`, `matg_light_bitmaps` over the ordinary metadata,
     `light_textures`; `layered` takes a cube's six level-0 faces and a
     volume's one level 0 of face 0, as `assets::decode_image` gives
     them) and says which lights are drawable (constants, no shadow).
     The first wiring read the block one word late (the address as the
     count, the next field as the address), so neither texture loaded
     and every pass sampling them stayed on D3D.
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
     textures, the gels of the map's `ligh` tags, and the cluster parts
     keyed by owner and part. At the hook,
     `native_render::light_list::sorted` checks the built dispatches,
     including handler, material variant and layer. The host resolves
     each requested layer against the full shader datum and the material's
     inputs. Missing streams, textures, stale stage palettes, unsupplied
     constants or unsupported device state refuse the light before submission.
     The GPU uses the cluster program vertex buffer with packed fetch and
     uploads eligible DXT stages as BC on capable devices. The encoder
     binds each prepared draw in order, without rereading the list or
     resolving its materials.
     - Colours: the resolve records where each mode-4 engine constant put
       its vector (`ResolvedImplementation::vector_bindings`: a vertex
       register's components, or a combiner constant's lanes packed as
       `0x00019ca0` does, `pack_engine_colour`, minus what later bindings
       overwrite). The cache key leaves the colours out and the host
       resolves with them zero; `colour_uniforms` puts each light's
       colours A and B (vectors 5 and 6) into its uniforms, so an animated
       colour resolves nothing.
     - Uniforms: each light/copy has immutable vertex and pixel uniforms
       at aligned dynamic offsets (`light_gpu::uniforms_layout`,
       `run_layout`).
     - Diagnostics (stderr, `native-lights:`). At load: each light
       texture loads with its size and layers, or says why not (no datum,
       the tag does not read, does not decode, or the wrong shape with
       the faces and level-0 sizes it has); the surfaces left to D3D split
       into opaque D3D draws, opaque unread shaders, transparent lit and
       transparent unread. At run time each refusal reason prints on its
       first occurrence.
       The host reports admissions, splits, lights sampling shader gels, reference and scaled passes and
       actual synchronous worker round trips. Every 1800 host decisions it
       also prints counts by reason.
     - Engine textures' texels (Code, V on 03a's `tile_metal_dark`): the
       light passes' samplers for the normalization cube and the
       attenuation volume carry no `D3DTSS_COLORSIGN` (the implementations'
       stage states; an engine-set stage state refuses the pass in
       `light_look`), and the combiners expand the cube with `bx2`, so
       both are sampled unsigned. The D3D path decodes them with
       `gfx::decode_texture` and the stage's sign bits (none), so the
       native upload is the decode as it is, with no bias; a pass whose
       engine stage reads signed channels is refused by
       `program::translate_pass` (not stored biased) and the light keeps
       its D3D passes. The samplers filter between mips (mip filter
       linear) and the D3D path uploads the whole chain, so
       `LayeredImage` carries it too (`with_mips`, `level_size`;
       `layered` takes each level's faces in order). Test:
       `the_native_light_textures_hold_the_texels_the_d3d_path_uploads`
       (synthetic X8R8G8B8 cube and AL8 volume through both decodes,
       every face, level and texel).
     - **Split lights** (`impl.render.native_lights_split`, off by default).
       The host validates the entire sorted array, reserves the exact
       supported prefix and skips those same sorted slots. Exclusions
       between two consumed entries are harmless. Matching materials in
       the D3D suffix retain their original order. The gathered item masks
       stay intact. The synthetic split test drives the guest entry loop
       through a duplicate, an exclusion and a failed counter write.

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
8. **First 03a A/B** (coordinator, pending). Use the same image,
   initial data and deterministic input in both arms. Set
   `H2_LEVEL=03a_oldmombasa H2_OPT=2 H2_FPS=30 H2_RENDER_SCALE=1
   H2_FRAME_STATS=1 H2_BOOT_STOP_AT_PRESENT=4000 H2_BOOT_GFX_EVERY=1000`.
   Keep render timing and split lights off. Use a separate
   `H2_BOOT_GFX` directory for each arm so matching present numbers survive.

   | Arm | `H2_FEATURES` | `H2_NATIVE_LIGHTS_COMPARE` |
   | --- | --- | --- |
   | D3D lights | `impl.render.native_world=on,impl.render.native_objects=on,impl.render.native_lights=off` | `0` |
   | Native diagnostic | `impl.render.native_world=on,impl.render.native_objects=on,impl.render.native_lights=on` | `1` |
   | Native pixel comparison | same as native diagnostic | `0` |

   For the combined target-13 test add `impl.render.native_gels=on`
   to both arms. Set `H2_NATIVE_GELS_COMPARE=16` for the producer
   diagnostic and `0` for pixel comparisons. Repeat at scale 2 to
   exercise display-copy reservations. A scene with zero shader-gel
   admissions provides no evidence for that binding.

   On stderr the first eligible diagnostic light stays on D3D. Two
   external-pass markers bracket its draws on the backend. Look for
   `native-lights compare: paired`, the material, layer and
   implementation, then the indented `uniform-compare` fields (D3D
   first). Pairing uses the vertex program and closest pixel definition.
   Zero matching draws means the comparison found no program to compare.
   Separately, `D3D fallback light` compares the original pass flags
   and each frame lane against the port, even when every light is refused.
   Check `admitted`, reference/scaled passes, worker round trips and
   `native_lights_v*_reference_ranges` in frame stats. Investigate
   dropped reservations and nonzero invariant counts. Compare decoded pixels at equal presents
   against the native arm with the diagnostic off. Identical captures
   from a run with zero native submissions establish no light coverage.
   Record the light site time separately after establishing parity.

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

### Hook point and order

The calls run; their draws are what the native renderer takes over.
For the colour pass (the only part native so far), one context-only
`before` on `COLOUR_PASS` (`0x000214f0`) arms the device to drop the
pass's own quads and to draw the native glow in place of the last
(order of work, item 2). So the glow lands where the game's composite
did: after layer 17, before the screen effects, `0x0002df00`, the fade
and the HUD, once per view, in the game's order; and the game's code
leaves every state, register and guest word as it always does.

The hook has nothing after its body, so the pipelined render runs it on
its own thread, and render-timing does not matter (the call runs).

### What the native post draws

- **The colour pass**, `0x000214f0` (Code). It is two passes, a glow:
  1. The view's target (`[0x004858b8]`) bound to all four stages
     (`0x00014f60`; their sampler states clamp and linear, stored into
     `D3D__TextureState` with the dirty bits), and a four-tap combiner
     (PSDEF at `0x00484f68`, constant colour `pack(c, second x3)` at
     `0x00484f9c`) drawn by `0x00034a90` into target 7, then blurred by
     `0x000211a0` (EAX 7; not traced), which returns the texture.
  2. That texture at stage 0, alpha blending on (`0x000222d0`: states
     59 = 1, 62 = 1 or `0x307` by the sixth argument, 63 = the seventh
     argument clear, 74 = `0x8006`), a one-stage combiner with
     `pack(x, third x3)` at `0x00484f90`, drawn by `0x00034a90` into the
     view's target; then the target is set back (`0x00014bc0`).

  Between them, `0x000211a0` blurs target 7 (Code):
  - `0x00020f30` first writes the blur's combiner (eight stages with the
    fifth argument above 0, else three; `0x00131ed0` packs its colour,
    and the local `y` sets a `<< 24` word) and selects it (the device's
    `+0x924`, `+0x928`, `+0x92c` = the block, `0x003fa3d0`);
  - then up to `ceil(first argument)` steps (about 3.99: four), each
    binding the last result to stages 0..3 (sampler words as above) and
    drawing four taps into the other buffer (7 and 8 in turn), with
    spread `fraction * scale`, the scale growing by
    `1 - g + g * [0x0045dc0c]` per step (`g` the fourth argument, 0.25):
    a Kawase-style blur. It returns the last buffer drawn. At infinity
    (or past 2^24, where `i + 1` is `i`) the loop never ends; the port
    stops after `engine::glow::MAX_BLUR_STEPS` (64) with
    `Error::Unbounded`, so the glue leaves such a call to D3D.

  The quad draw `0x00034a90` (eleven arguments, `ret 0x2c`): the target
  (`0x00014bc0` unless -1), `0x000142f0` with the second argument unless
  -1, the colour write mask (third, `0x1010101`), Z off (or mode 2 with
  the first byte), Z bias 0, Z write off, cull none, stencil off, with
  the byte two more simple states (S: alpha test; `0x203`/`0x204` by the sign of the sixth
  argument); then per tap (the ninth argument: 4 or 1) an offset from the
  word tables at `0x00485af4`/`0x00485b04` over the source's size
  (`0x0001dd60`), or the screen rectangle `0x004b9dd0..`, and the
  spread (seventh); the quad through `0x0001c590` and `0x00034420`.

  Its numbers are built: `engine::colour_pass` (the arguments and
  parameters), `engine::glow` (`pass_a`, `pass_b`, `blur_combiner`: the
  three 60-word blocks; `blur`: each step's source, target and spread,
  and the result). The native pass is these three stages over a copy of
  the view's colour target, through the stage 3 combiner compiler with
  these blocks.
- **The screen effects**, each entry `0x0002d140` would draw: its shader
  compiled as stage 3 compiles layer 15, drawn with the rectangle and
  the entry's function values. Blur needs the scene as a texture, which
  the colour pass's copy already gives.
- **`0x0002df00`**, once it is traced; until then a call with `late`
  set keeps its D3D passes.

### Stage 5 status

With `impl.render.native_post` off (the default) everything here draws
on D3D, and the host only observes. With it on, a call is armed only
once nothing in it is missing (`native_post_glue::Missing`, counted and
logged once per reason). It is then probed before any of its draws are
dropped, and it falls back on its own: the glow alone, and each screen
effect per egor.

| Effect kind | With the switch on | Why, or what refuses it |
|---|---|---|
| Colour pass glow (`0x000214f0`) | Armed after its probe | Refused when its plan fails (a blur past `MAX_BLUR_STEPS`, a target it cannot read). A refused draw or a swallow mismatch keeps every later glow on D3D. |
| Screen effect, passes into the bound target, nothing convolved (`desaturate`'s shape) | Armed after its probe | `TemplatePass` when a pass's program did not load, or a stage binds an engine texture that is not supplied (`screen_gpu::supplied`). |
| Pass into target 1, read by a later pass | Armed | `PassTarget` when the pass into target 1 is the call's last, or has no size. `TemplatePass` when target 1 is read before the call drew it (`call_ordered`). |
| Kept convolution (flag bit 0 or 1 with its value 0) | Armed | Nothing to refuse: it draws nothing, and its result is the pass's target. |
| Blur convolution (`0x000211a0`) | Armed (blur egor; waiting on the mainmenu A/B) | `Blur` and `StandIns` without the quads' frame. At the probe: a convolving pass that blends or masks colour (`ConvolutionStates`), a combiner that does not compile, the colour not sampleable. |
| Wide convolution (`0x00021360`) | Armed (built; not run live yet) | `Wide` and `StandIns` without the frame, or when a grid cannot be made (`grid_of`: the source bound at no size, the grid's arithmetic failing). At the probe: as the blur, plus a grid strip under a triangle (`NoStrips`). |
| A pass `0x00016610` does not draw (no vertex shader for its implementation) | Refused (`TemplateSkipped`) | `0x0002d140` skips it whole; the plan would wait for strips that never come. |
| Convolution that copies its result (`0x000351a0`) | Refused (`TemplatePass`) | Not drawable natively. `0x0002d140` passes -1, so it never happens. |
| A screen effect's plan that fails (too many corners, unreadable values, a convolution past its budget) | D3D | Logged once ("a screen effect keeps its D3D draws"). |
| `0x0002df00` (`late`), `0x0002dd30` | D3D | Not traced yet (step 5 below). |
| Layer 17, the fade `0x00040890`, the UI and HUD | D3D | Not started (steps 6 to 8). |

### Order of work

1. **The hook and its order** (built: `feature::native_post`,
   `impl.render.native_post`, off by default; point `COLOUR_PASS`,
   `0x000214f0`). The first design skipped `render_screen_effects`
   whole and redid what it leaves (its prologue first); it is replaced
   by the swallow below, under which the game's code runs and leaves
   every state itself, so nothing is redone. `native_render::post`'s
   `PostFrame` (the call's gates) stays for the screen effects.
2. **The swallow** (built: `d3d::Device::swallow`). At the colour
   pass's entry a context-only `before` (nothing after its body, so the
   pipelined render runs it on its own thread; fine with render-timing,
   since the call runs) hands the host the call's seven stack words and
   the caller's MXCSR and x87 control word. The host plans the glow
   (`boot::native_post_glue::plan`: `colour_pass::parameters` on those
   words, then `glow::pass_a`, `blur_combiner`, `blur` and `pass_b`) and
   arms the device with one entry per quad (`targets`): the view's
   target (slot 0, the one bound at the entry), target 7 (1) or 8 (2).
   Arming drains what is queued first, so only quads the pass pushes
   count. Each must be the next planned: an inline `Begin(7)`/`End`
   (a four-vertex fan, `0x00034420`), no other draw since the last, into
   its slot's target (a slot's first quad fixes its target; slots
   differ). Each is dropped; the native glow is issued in place of the
   last, into the targets bound then (the composite's: the view). Any
   other draw ends the swallow (`Stats::swallow_mismatches`): it and
   every draw after it are drawn by the D3D path and the glow is not;
   the host logs it and keeps every later call on the D3D path. The
   quads already dropped stay dropped (S: one frame without its glow or
   with a composite over stale 7 and 8; the plan matches the game's
   control flow bit for bit, so this takes a port that is wrong). A call
   the host cannot plan (a copy after the blur, a target it does not
   know) is not armed.

   Nothing is dropped unless the glow will be drawn. The glow and the
   A/B's capture are colour-only external passes
   (`ExternalPass::colour_only`): the composite binds no depth, so the
   device issues them with none and the GPU backend gives them a scratch
   depth (`Backend::external_pass` with the colour as its own depth).
   The device refuses to arm a drop, dropping nothing, when the pass
   could not be issued into the targets bound at the entry (no external
   passes in the backend, no colour target, an unreadable header) or is
   not colour-only (`Stats::swallow_refusals`). Every pass not issued is
   counted by reason (`Stats::external_refusals`, `ExternalRefusal`)
   and each reason logged once (`d3d: an external pass was dropped:
   ...`). The host then observes its first call with a probe that checks
   on the GPU's thread that the glow would draw into the target it gets
   (`native_render::glow::check`: the target can be sampled, is the
   plan's size, every block compiles); only then are quads dropped. A
   probe or glow refused keeps every later call on the D3D path (logged).
   The first A/B on 03a ran headless (no `H2_BOOT_GFX`, so no GPU
   backend): every pass was dropped (`Unsupported`) after its quads
   were, and no capture could be made; both are now refused up front and
   logged. A backend that takes the pass on the render thread and then
   refuses it there (a multisampled or non-drawable target) is not seen
   in time: the probe's pass is never encoded, so nothing is dropped.

   Tests: d3d `swallow` (exactly the plan's quads dropped and nothing
   after, the pass issued colour-only after the last; a missing or extra
   quad, another draw or another shape ends it; an unfinished swallow;
   through the render thread; a backend that takes no passes or a pass
   that needs depth drops nothing; each refusal counted), boot
   `native_post_glue::tests` (nothing dropped before the probe; every
   pass colour-only), feature `native_post`.
3. **The colour pass.** Its numbers are built: `engine::colour_pass`
   (`arguments`: the caller's half with `0x000137a0`'s curves;
   `parameters`: the callee's override record and clamps;
   `pack_colour`: `0x00131e50`; `Parameters::colours`: the two combiner
   colours), bit-equal to the lifted original
   (`engine/tests/colour_pass_differential.rs`), and the glow's:
   `engine::glow` (`engine/tests/glow_differential.rs`: the three
   combiner blocks, and the blur's steps with the device calls stubbed).
   The host (`boot::native_post_glue`) uses them through the swallow
   (item 2).
   1. **The quad, traced** (Code). `0x0001c590` loads the vertex shader
      the rasterizer globals name for screen quads
      (`[[0x00485a80] + 0x5c] + 0x64`, entry 0; `LoadVertexShaderProgram`
      when Halo's cache at `0x0051f0f0` holds another), which passes
      every input through: `oPos = v0`, `oT0..oT3 = v1..v4`,
      `oD0 = v5`, `oD1 = v6`. So positions are the target's pixels.
      `0x00034420` (EAX 0: rectangle, callback `0x00034100`, 0, the tap
      record) runs the draw-state reset `0x00016b10` again, sets the
      four stages' dirty bits and zeroes their `D3DTSS_ALPHAKILL` words,
      commits the textures (`0x0001cf50`: `SetTexture` for each stage
      whose `0x0051f3d8` word differs from `0x0051f3c8`'s, which it
      updates), the vertex input (`0x0001c710`) and the deferred state
      (`0x00400fa0`), then emits one four-vertex fan (`Begin(7)`) with corners
      `(0, 0)`, `(0, 1)`, `(1, 1)`, `(1, 0)`, attributes 15 down to 0 from
      the callback as inline `SET_VERTEX_DATA4F`. The callback: attribute
      0 is the rectangle's lerp (`0x000350e0`, z and w from the near and
      far at `0x00485ad4`); attribute `k + 1`, for `k` below the tap
      count, the same lerp moved by the spread times a sign
      (`[0x0045dbbc]` with `k & 1` in x and `k & 2` in y,
      `[0x0045dbc4]` otherwise) and scaled by the stage's bound size
      over the target's. With the quad draw's tenth argument clear (all
      three passes) the rectangle is Halo's window (`0x004b9dd0`: top,
      left, bottom, right shorts), and the eleventh clear means no
      window offset, so a fragment's base coordinate is its own pixel
      and each tap reads the source in texels. The down-sample's four
      taps at spread 0.5 land half way between texel centres: a 4 by 4
      box. `0x0001dd60` reads Halo's target table (`0x004b4b58`, stride
      `0x98`: width `+0x80`, height `+0x84`, surface `+0x88`, a hiding
      byte `+0x95`; targets 0, 3 and `0x18` default to 640 by 480).
      Targets 7 and 8 are made by `0x0001df20` at 160 by 120,
      `D3DFMT_LIN_A8R8G8B8` (`0x0001d6f0`'s linear format `0x12`), one
      level. `0x00014f60` binds a target to a stage: its size into
      `0x00485af4`/`0x00485b04` and its texture into `0x0051f3d8`
      (through the jump table at `0x00015114` by the target's kind).
   2. **The native glow** (built): `native_render::glow`.
      `read_frame` reads the window, the tap signs and the sizes;
      `Plan::quad` gives each quad its corners and per-stage offsets and
      scales; `VERTEX` is the screen shader for them; `program` compiles
      a block through `shaders::psh` with its stages linear (`tex_scale`
      one over the guest size) and fog off; `GpuGlow::encode` draws a
      plan's quads in order into stand-ins for targets 7 and 8
      (`Rgba8Unorm`, the guest size times the copy's scale) and the
      view's colour, sampling it directly (gfx's colour surfaces are
      `TEXTURE_BINDING`), the composite with the blend of
      `0x000222d0`'s states (`Blend`: source `ONE` or
      `INVDESTCOLOR` by the sixth argument, destination `ONE` unless
      the seventh's byte is set, alpha alike). The down-sample and
      blur draw with blending off (item 3), and S: the D3D path's pixel centres
      match a quad whose corners are the window's (the frame A/B shows
      it). Tests: `glow::tests` (the taps of the down-sample and the
      composite, the blend, a block's module, the frame's words).
   3. **The leftovers**: none to redo. The game's code runs; only its
      quads are dropped (item 2). `0x00034a90`'s `0x000142f0(10)` sets
      `NV097_SET_BLEND_ENABLE` (`0x00040304`) to 0 for the down-sample
      and the blur (Code), so they draw unblended, as `GpuGlow` does.
   4. The switch on draws the glow natively; after the frame A/B below
      it can become the default.

   **The frame A/B** for the glow: the same frame (a fixed save, view
   and time, `H2_FEATURES` differing only in `impl.render.native_post`)
   captured with the D3D path and the native one. `H2_POST_AB=<dir>`
   (`native_post_glue::Ab`) captures the view's colour where the glow
   lands (after the composite; before the screen effects, the fade and
   the HUD) from each call `H2_POST_AB_AT` names (default 600; a list
   `601,1500,2700`, or a start and a stride `600+500` for ten windows
   spread over the run, intro and gameplay), `H2_POST_AB_COUNT` calls
   each (default 1), into
   `<dir>/<native|d3d>-<call>-<w>x<h>.png`, one per copy of the target
   under the render scale. With the switch off the hook is placed for it
   and observes the same quads, dropping none, so the D3D side is
   captured at the same draw. It needs the GPU backend: a headless run
   writes nothing (logged); set `H2_BOOT_GFX=<dir>
   H2_BOOT_GFX_EVERY=never`. With the switch on the first call is the
   probe's (a D3D-side capture if it falls in the window), so start the
   window after it. Compare:
   - the two captures per pixel, within a small tolerance for filtering
     (max and mean error and the count over it);
   - `swallow_mismatches` 0 and `swallowed_draws` the quads planned on
     both sides, `external_passes_dropped` steady;
   - the frame's next draws (the screen effects, the fade, the HUD) as
     d3d records them: equal (the game's code left the same states);
   - the ms of `render_screen_effects`.
4. **The screen effects** (`0x0002d140`; 03a's blur first), with their
   function values (`0x00134fc0`) read at the call. Traced (Code,
   `h2 disasm`); slice (a) built (below).
   - **The loop** (`0x00013a0d..0x00013b72`): the window `0x004b9dd0`
     as four floats twice (the corners), then per entry of the scenario
     block (`+0x3d0` count, `+0x3d4` address, `0x24` bytes): skipped when
     its tag datum `+0x14` is -1 or both function indices `+0x20`/`+0x22`
     are -1; otherwise each index not -1 is evaluated by `0x00134fc0`
     (ECX the index; the scenario's function block `[0x004e6740]`,
     `0x20` bytes each, through the tag function evaluator `0x0013b390`)
     into two floats, and with either non-zero
     `0x0002d140(values, 2, 2, corners)` runs with EAX the datum (`ret
     0x10`).
   - **`0x0002d140`** copies the four value words to `0x004c19b0..bc`
     and returns at once for a datum of -1, an empty window, or a target
     override `[0x004858b8]` set. The datum is an `egor` tag; its shader
     reference (`+0x44`, a template through `0x00137bd0` when its group is
     `MTRP` or `3trp`) gives the implementation count. Then per entry of
     its pass block (`+0x88` count, `+0x8c` address, `0xac` bytes):
     - the pass's implementation index (`+0x08`, -1 the loop index) and
       one of four bytes `+0x0c..0x0f` chosen by which of the two values
       are zero (-1: skipped);
     - its target by `+0x60` (the jump table at `0x0002db68`: the view
       at full size, or a half or quarter of the window, `0x0001ddd0`);
     - `0x00016610(shader, 15, index, byte, ...)`, the shader's layer-15
       pass (a template pass, as the world's and the transparents'), then
       the quad pushed by hand: `0x417fc` (`SET_BEGIN_END`, 6) and per
       vertex up to four `SET_VERTEX_DATA4F` texture coordinates (the
       kinds at `+0x50`, jump table `0x0002db78`: from the corners, the
       window over its size, or scaled by `[0x0045dbbc]`/`[0x0045dc20]`)
       and the position (`0x101a00`, z and w `[0x0045dd40]`), S: a strip of
       `2 x 2` vertices for the call's `(2, 2)`; `0x003fb480` ends it;
     - then per entry of its convolution block (`+0xa4` count, `+0xa8`
       address, `0x5c` bytes): a size from the target's (`0x0001dd60`)
       times `+0x58`, at least 64 and its area capped at `0x12c00` by a
       square root (`0x00322b0c` rounds), targets `0x19`/`0x1a` (first)
       or `0x1b`, and with flag bit 2 `0x00021360` (ported:
       `engine::glow::wide`) else the blur `0x000211a0` (ported:
       `engine::glow::blur`), its result in
       `[0x00485a78 + 4 i]`.
     - At the end the targets 1, `0x19`, `0x19` and `0x1b` are set back,
       `[0x004670bc]` set, the draw state reset (`0x00016b10`) and
       `0x00485a78`/`7c` cleared.
   - **What the native side needs.** The draws can be swallowed as the
     glow's are (the hand-pushed strip is an inline `Begin(6)` of four
     vertices; the blurs are `0x00034a90` quads). Drawing them natively
     needs, in order: (a) the plan: which entries draw (the function
     values, `0x00134fc0` over `0x0013b390`, a port with its own
     differential test), and per pass the implementation, target and
     texture coordinates (pure, from the tag); (b) the layer-15 template
     pass through `program::translate_pass` (the world's and lights'
     path) with the engine inputs the screen-effect pass reads; (c)
     `0x00021360` traced and ported beside `blur`; (d) targets `0x19` to
     `0x1b` as stand-ins like 7 and 8. Each is a slice; (a) first.
   - **Built: which entries draw** (`engine::screen_effects`):
     `function_value` (`0x00134fc0` and `0x00134fe0`: the state's input
     plus its `+0x1c`; without a parent (`+0x16` -1 or past the count)
     the function through `0x0013bb90`, its output range unless a flag in
     `0xf0`; with one (itself without a parent, else 0) the parent's
     value times its own raw value, then its range), the evaluator being
     `engine::function_eval`; `full_screen_single_view`
     (`0x00022710`: the viewport `0x00485648` is 0, 0, 480, 640 and at
     most one view `[0x004ba04c]`); `gated`, `corners` and `drawn` (the
     loop: each entry's datum and four values). Tests: unit tests, and
     `engine/tests/screen_effects_differential.rs` (`0x00134fc0` over
     random functions of every type with parents, and the loop from
     `0x000139ba` with `0x0002d140` stubbed, against the lifted
     original, bit for bit).
   - **Built: the evaluator** (`engine::function_eval`, `0x0013b390`
     instruction for instruction): SSE in the original's operand order,
     so a NaN keeps the sign and payload the original's does (the blend
     is `second * r + (1 - r) * first`, type 9's `(1 - r) * first +
     second * r`); shape `0x0017ca10` and wave `0x0017c900` over the
     image's tables with the CRT `fmod(v, 1.0)` (`0x00373b6a`: `fprem`
     for finite non-zero, `fldz` for a zero, a quieted NaN, the
     indefinite for an infinity) and an x87 `fistp`; type 9 and spline
     2 through the CRT `pow` (`0x00372d48`) on the x87 model: `fyl2x`
     and `0x00327f84`'s `frndint`/`f2xm1`/`fscale`, negative bases by
     `0x00372f31`'s integer and odd test, zero bases, NaNs (`faddp`),
     infinities (`_powhlp` `0x003738a8`), the range check `0x0032806d`
     (scaled by 2^1536 or 2^-1536) and `_handle_exc` (`0x003246b5`):
     overflow to the rounding mode's infinity or largest value,
     underflow to a denormal by truncation or zero, the others kept.
     Spline 2's exponent is `rsqrtss` of one segment's squared length
     times the halving-trick root of the other's, as the original
     computes it. Control words with an x87 exception unmasked are
     refused. Tests: unit tests, and the differential above with
     `_getptd` (`0x00326079`) stubbed so the CRT's error paths run
     lifted.
   - **Built: what each draws** (`engine::screen_effect::plan`, one call
     of `0x0002d140` as an `Effect`): the shader and layer 15's
     implementation count (the template datum the shader's `+0x24`
     points at, its definition's `+0x5c` element: the word at
     `[+4] & 0x1ff`, then the word that many plus 15 into `[+0xc]`, its
     high 7 bits; `Err(Data)` for `MTRP`/`3trp`, which go through a
     virtual call). Per pass that draws: its index, implementation and
     selector (`0x00016610`'s third and fourth arguments; skipped as the
     original skips them), its target kind and target 1's size (window
     width and height, halved or quartered toward zero), the strips (per
     row of corners, per column the corner and the one below; four
     texture coordinates by kind, 0 the corner's uv, 1 the window
     fraction, 2 pixels from the window's top left, 3 the corner's x and
     y, 4 the vector at `[[0x004687c0]]`, z 0 and w 1 except kind 4; plus
     the offsets `+0x5c` when `+0x58` is above 0, u and v times
     `[0x004b9de4]` when the set's flag bit 0 is set; the position raw,
     from the window's top left, or that times `[0x0045dbbc]` or
     `[0x0045dc20]`) and `Pass::pushed`, the exact words; and its
     convolutions (kept, the blur with `glow::blur`'s draws, or
     `0x00021360`'s registers and arguments), each with its size (the
     area cap: x87 `fild`, `fdivr`, `fsqrt`, `_ftol2` toward zero, both
     64 when either side is under 64). At the end the original sets
     target `0x19` twice, the second time to `0x1a`'s saved size (as it
     does; a native draw need not copy it). Tests: unit tests, and
     `effect_plans_match_the_lifted_original` (`0x0002d140` lifted, its
     callees stubbed and recorded: the template passes, every size set,
     the convolution calls and the words pushed must be the plan's).
   - **Built: `0x00021360`** (`engine::glow::wide`, slice (c)), the blur's
     sibling (Code): EAX the source, ECX the draw count, XMM0 a scale,
     then stack words passed to each draw, the two buffers, the last
     target, a byte that first sets the combiner `0x00020f30` (XMM0 0,
     XMM3 its eighth word), `ret 0x20`. `s = scale * [0x0045e4e0]`; per
     draw (count above 0) the source is bound to stages 0..3 (`0x00014f60`,
     the same sampler states as the blur) and `0x00034d90` draws into the
     destination with EDI pointing at `s`, `s * [0x0045e1f4]`,
     `s * [0x0045e4dc]` and `0 - s`, the buffers ping-pong as the blur's,
     and `s` grows by `[0x0045dc0c]` (the blur's spread term). The result
     and the copy into the last target are the blur's. `MAX_WIDE_TAPS`
     (64) bounds it. `0x00034d90` is `0x00034a90`'s sibling: the
     target's size (`0x0001dd60`, -1 the bound one `[0x004858b8]`), the
     target's rectangle or the window by a byte argument, the per-stage
     size ratios (`0x00485af4`/`0x00485b04` over the target's) and EDI's
     four floats where `0x00034a90` takes a spread; how the four move
     each tap is for the native draw to trace. Tests: unit test
     `the_wide_convolution_ping_pongs_with_a_growing_scale`, and
     `glow_differential::wide_matches_the_lifted_original` (stubbed as
     the blur's: draws, offsets, the quad's stack words, combiner, copy
     and result); the plan now follows a convolution after a wide one.
   - **Built: `0x00034d90`'s grid** (`engine::screen_grid`): the grid
     each step of `0x00021360` draws, 30 strips of 21 x 2 vertices
     (`0x00034770` with the attribute function `0x00034100`). Each tap is
     the rectangle at (u, v) pulled toward its centre by its EDI float,
     weighted, when the radii (words 5 and 6) differ, by where the
     vertex's distance from the centre (x87 `fsqrt`) falls between them,
     times the stage's bound-over-target ratio; the position through
     `0x000350e0`. A radial (zoom) blur. `strips()` and `pushed()` give
     the exact words. Tests: unit tests and
     `screen_grid_differential::the_grid_matches_the_lifted_original`.
   - **Wired: the screen-effect hook** (`feature::native_post`, switch on
     only): a context-only `before` on `0x0002d140`
     (`points::SCREEN_EFFECT`) hands boot's host EAX and the four stack
     words; `native_post_glue::plan_effect` plans the call and
     `missing` lists what does not draw natively yet (the layer-15
     template pass, passes into target 1, blur and wide convolutions,
     the stand-ins for targets `0x19` to `0x1b`). Every effect is
     refused, counted and logged once per reason, and nothing is armed:
     the D3D path draws them all. Each piece then gets a slice and is
     struck from `missing`; only an effect with nothing missing will be
     armed, behind the same swallow and probe as the glow.
   - **Built: the template pass's look** (`native_render::screen_look`):
     `screen_look` takes the egor's shader resolved and an
     layer pass and implementation and gives that pass of layer 15 (at
     the first level of detail; its vertex shader, stages as the shader's bitmaps
     or engine textures the draw supplies, stage and render states,
     pixel shader, constants), or why it stays on D3D (no layer, no such
     pass or implementation, animated, engine inputs beyond the
     instance's, engine-driven states, unset or default textures, a
     lightmap, states the native pass does not fit). `compile_screen`
     translates it through `program::translate_pass` over the strips'
     inputs, `v0` the position and `v1`..`v4` the four texture
     coordinates (`0x00101a00`, `0x00101a10 + 16 s`). Still missing
     before `TemplatePass` leaves `missing`: reading each egor's shader
     at load (its material, vertex program and bitmaps), the engine
     textures layer 15 binds, and the GPU pass drawing a plan's strips.
   - **Built: the load** (`boot::native_post_load`, the switch on): every `egor` tag's shader (the definition's `+0x44`)
     resolved, each implementation of each of its layer-15 passes
     looked at and translated with its vertex shader's classification-0
     program (below), and the stages' bitmaps decoded; refusals counted by reason, one line on
     stderr. The post glue runs it itself, on a thread of its own at the
     first screen-effect call on a map (`native_post_glue::EffectsSlot`),
     with or without the native world: it used to ride the world's load,
     so with `native_world` off no egor loaded and every call was refused
     as `TemplatePass`.
   - **Traced: the engine textures** (`0x00019530`, the jump table at
     `0x00019b68` by id - 1; `native_render::screen_look::engine_texture`):
     1, 9, 14 and 15 the rasterizer data's bitmaps (`+0xc`, `+4`, `+0x14`);
     render targets through `0x00014f60`: 2 and 3 target 1, 6 target 3, 8
     target 10, 13 target 9, 17 `0x13`, 19 `0x15`, 20 `0x16`, 21 13, 22
     `0x11`, 23 `0x17`; 5 the view's bound target `[0x004858b8]`; 24 and
     25 the target the word `[0x00485a18 + 4 id]` names (`0x19`..`0x1b`
     sized through `0x0001dd60`: the screen effects' own targets); 4
     clears the stage; 10 and 12 the light's; 16, 18, 26 and 27 chosen at
     run time; 7 and 11 null entries. A template pass binding the light's,
     a run-time one or an invalid id is refused
     (`ScreenRefusal::EngineTexture`); the rest the draw supplies.
   - **Traced: which pass, implementation and program**
     (`0x00016610(shader, layer, pass, implementation, target, flag)`,
     Code): the layer's passes are the template's word at its layer
     index (low 9 bits the first, high 7 the count, which is
     `screen_effect::Effect::implementations`); `pass` -1 draws them all,
     else that one (`screen_effect::Pass::implementation` names a layer
     pass), each with implementation `implementation`
     (`Pass::selector`; `0x132` bytes each, the vertex shader at `+0x100`,
     -1 not drawn). After `0x0001bbf0`, `0x0001c590` loads that vertex
     shader with classification 0 (`0x0001c2e0`, `0x1c` bytes per
     classification), so the load translates only the first program
     (`screen_look::SCREEN_CLASSIFICATION`), per (layer pass,
     implementation): `effects\screen_effects\blur` has two layer-15
     passes (`copy_fb_to_texaccum`, `copy_texaccum_to_fb`), so a layer
     with several passes is no longer refused. What the real egors
     (`blur`, `desaturate`) showed the look must take: their passes set
     `ZENABLE` 0 and no ZWRITEENABLE (`pass_state` takes the depth
     buffer off as enough; a screen pass testing or writing depth is
     refused, `ScreenRefusal::Depth`), and `screen_xform_4` moves `v5`,
     `v6` and `v7.w` into the colours and fog, registers the strips do
     not push: `compile_screen` gives them the uniforms' defaults only
     for a pixel side that reads no colour or fog
     (`screen_look::reads_vertex_colours`, which also counts a missing
     final combiner as reading them). Its WGSL writes no depth
     (`PassParts::w_buffer` off). The engine constants are the call's
     (below).
   - **Built: the GPU pass, unarmed** (`native_render::screen_gpu`,
     `native_post_glue`'s `ScreenPass` and `ScreenProbe`): each strip a
     triangle strip of `StripVertex` (attribute 0 the position, z and w
     `[0x0045dd40]`; 1..4 the texture coordinates), the translated
     program with the target's guest size as its surface and the
     device's depth scale as its `z_max`, each render-target stage
     fetched in its texels (`tex_scale`, as a linear texture), no depth,
     no culling (S: the D3D path's cull mode is inherited; the A/B checks
     the facing), the pass's blend and colour writes. Of the engine
     textures only the view's (5) is supplied, as a copy of the bound
     colour taken before each pass that samples it (S: 5 and `kind` 0
     both resolve to target 0 while `[0x004858b8]` is clear). The host
     strikes `TemplatePass` from `missing` for a pass whose program
     loaded and whose stages are all bound (`pass_drawable`), and arms a
     call with nothing missing as the glow is armed: one swallow of its
     strips (slot 0, `SET_BEGIN_END` 6, twice the columns), a probe
     first, then the pass in place of the strips; a mismatch after a
     screen swallow keeps every later call on D3D.
   - **Traced: the end of the call draws nothing**
     (`0x0002dae8..0x0002db64`): `0x0001ddd0` writes the sizes of
     targets 1, `0x19` and `0x1b` back into the target table (`+0x80`,
     `+0x84`, the surface words `+0x14`, `+0x28`) from what
     `0x0002d26e..0x0002d344` saved on entry (0 for a target without a
     surface; `0x19` is written twice, the second time with `0x1a`'s
     saved size, and `0x1a` never), `0x00016b10` applies the last pass's
     restore states and clears the draw-state cache at `0x00485b48`,
     `[0x004670bc]` is set and `0x00485a78`/`7c` cleared. The game runs
     it on both paths, so the stand-ins for `0x19`..`0x1b` are missing
     only for a call with a drawn (blur or wide) convolution, which
     renders into them; a pass sampling them (engine textures 24 and 25)
     is not drawable on its own. A call of passes into the bound target
     with every convolution kept, the shape of
     `effects\screen_effects\desaturate` (one pass, kind 0, none
     convolved, stage 0 the view), now misses nothing and is armed with
     the switch on: a probe on its first call, its strips dropped for the
     native pass after. The blur (`kind` 1 and a convolution in its first
     pass) stays on D3D.
   - **Built: passes into target 1** (`screen_gpu::Dest`): a pass of
     `kind` 1..3 draws into a stand-in for target 1, its guest size the
     plan's (the window's, a half, a quarter) and its texels that times
     the copy's render scale, cleared first (S: the strips cover it); a
     later pass of the call samples it as engine textures 2 and 3, in its
     texels. Target 1 read before a pass of the call drew it (what the
     D3D path left there), or by the pass drawing it, is refused
     (`screen_gpu::check_order`, `native_post_glue::call_ordered`). The
     swallow sends target 1's strips to slot 1. `PassTarget` stays
     missing only for a pass into target 1 that is the call's last (the
     native pass is issued into the target bound at the last strip) or
     has no size. Nothing more is armed by it: the only egor with such a
     pass, the blur, still misses its convolution and the stand-ins, and
     its second pass reads engine pixel constants and engine texture 24.
     (S: what the D3D side later reads of target 1 is not this call's,
     since its strips are dropped; target 1 is scratch to the screen
     effects, as 7 and 8 are to the glow.)
   - **Built: the call's engine constants**
     (`screen_look::ScreenConstants`): `0x0002d140` copies its four
     values to `0x004c19b0`, and the scalar evaluator `0x000336f0` (its
     table at `0x000338d4`) returns `[0x004c19b0]` for scalar 41 and
     `[0x004c19b4]` for 42 (`screen_look::SCREEN_SCALARS`), so a pass
     reading those is no longer refused. The blur's
     `copy_texaccum_to_fb` reads 41 into `c0.a` (pixel mode 3), the mix
     of its two stages, and sets `c18`..`c21` from its stages' sizes
     (vertex modes 14 and 15 of stages 1 and 2: `[w, 0, 0, 0]`,
     `[0, h, 0, 0]`), which carry its coordinates into texels. Per call
     the pass gets each combiner constant reading a value packed as
     `0x00019ca0` packs it (the binding's transform, then
     `pack_engine_colour`, into its lanes) and each vertex constant
     reading a value or an engine stage's size evaluated as
     `0x00019be0` evaluates it (`tags::material::EngineVertexConstant`,
     from the `spas`, keeping what no later binding of the register
     writes), the size the guest size of the target the stage binds
     (`screen_gpu::stage_sizes`: the view's, target 1's as the call drew
     it). The rest stay as resolved at load: the instance's scalar 21,
     the pixel side's sizes (`0x00019ca0` takes 1 by 1), a bitmap stage's
     size. A vertex register the resolver took from the engine that no
     external of the implementation names is refused
     (`ScreenRefusal::EngineVertexConstants`).
   - **Built: the blur's convolution** (`screen_gpu::ScreenConvolution`,
     `native_post_glue::screen_convolution`): `0x0002d140` calls
     `0x000211a0` with its seventh word 1 and the eighth to tenth 0, so
     the combiner is `0x00020f30`'s at strength 0 and `y` 0
     (`engine::glow::blur_combiner`), and each quad goes through
     `0x00034a90` with its tenth argument 0: the window's rectangle in
     the stand-in's pixels, four taps, each stage's coordinates scaled by
     its source's size over the stand-in's (`0x00034bf7`..`0x00034c27`),
     the quad `glow::quad` gives the glow's, drawn by the glow's
     `glow::QuadDrawer` (the glow now draws through it too). The quads go
     into stand-ins for `0x19`..`0x1b` (`screen_gpu::STAND_INS`, at the
     plan's guest size times the copy's scale, `STAND_IN_FORMAT`; S: an
     8-bit target as 7 and 8), each cleared when the call first draws
     into it; their source is the pass's target (the bound colour, or
     target 1's stand-in) or the stand-in the last quad drew. Engine
     textures 24 and 25 bind the target the pass's convolution `n` left in
     `[0x00485a78 + 4 n]` (`screen_gpu::Bound::Result`), refused before
     one set it in the call, since `0x0002d140` clears them on its way out
     (`screen_gpu::Drawn`, `check_order`). A pass that blends or masks
     colour does not convolve natively (its quads draw under its states).
     The swallow takes each draw's own shape (`d3d::Swallow::shapes`: the
     strips of `SET_BEGIN_END` 6, the quads `Begin(7)` of four), the quads
     into slots 3..5 for `0x19`..`0x1b`. Each egor is probed on its own
     before its draws are dropped, and falls back on its own: a refused
     pass, a failed probe or a swallow mismatch (charged to the egor that
     armed last) keeps that egor's later calls on D3D and no other's. With these, the blur misses nothing and
     is armed with the switch on (off by default).
   - **Built: the wide convolution** (`0x00021360`; `screen_gpu::ConvolutionGrid`,
     `native_post_glue::wide_grid`, `grid_of`). `0x0002d140` passes it
     the words `+0x50`, `+0x54`, the stand-ins, -1, 1, 0, 0, so its
     combiner is the blur's (`0x00020f30` with XMM0 0 and XMM3 its eighth
     word, 0), its stages are bound as the blur's (clamp, linear), and
     each draw is `0x00034d90` (Code, `0x00021455..0x00021485`) with the
     same screen vertex shader (`0x0001c590`) and states as the quad's:
     the window's rectangle (word 7's low byte clear), depth `1.0`, the
     two words its radii, EDI the draw's offsets, each stage's ratio the
     size `0x00014f60` bound the source at (`0x0001dd60` of it: the
     pass's target as the call sized it, `screen_effect::target_size`,
     or a stand-in at the pair's size) over the stand-in's. The glue
     makes each draw's 30 strips of 42 vertices with
     `engine::screen_grid::strips` (the original's arithmetic, under the
     call's MXCSR and x87 control word), the taps in texels of that bound
     size; `glow::QuadDrawer::draw_grid` draws them through
     `glow::GRID_VERTEX` (attribute 0 the position in the stand-in's
     pixels, 1..4 the taps; z and w ignored as the quad's are: no depth
     test and w the same at every vertex) and the blur's combiner with
     its stages scaled by the bound size. The swallow takes each strip
     as `SET_BEGIN_END` 6 of 42 (`0x00034770`) into the stand-in's slot.
     A grid that cannot be made keeps the call on D3D
     (`Missing::Wide`), as does a missing frame; a grid with a strip
     under a triangle is refused (`ScreenRefused::NoStrips`). With these
     a wide convolution misses nothing and is armed with the switch on
     (off by default).
   - **Fixed: the first mainmenu A/B armed nothing** (1500 presents,
     pixel-identical): the effects loaded, but the first probe's swallow
     ended at its first strip (`swallowed_draws` was the glow's six per
     frame exactly), and the effect fell back. The likely cause: slot 0
     is the target bound when the swallow is armed, and `0x00016610`
     binds each pass's own target (`0x00014bc0` of 0 or 1, Code,
     `0x0001675c..0x00016765`). `0x0002d140` has two callers,
     `render_screen_effects` (`0x00013b52`, 2 x 2 corners) and the UI
     (`0x0022bbbb`, 3 x 3), and the UI's call need not have target 0
     bound. A screen call's swallow now plans no slot 0
     (`native_post_glue::SLOT_BOUND` 1, target 1 slot 2, the stand-ins
     3..5), and `d3d::Swallow` pins slot 0 only when a draw plans it.
     The device says how a swallow ended (`Device::swallow_mismatch`:
     the draw, its planned shape and slot, what was pushed, after
     another draw or into another target, or armed over), and the glue
     logs it with the fall-back. Arming over an unfinished swallow is
     charged to that one, not the new one. A proof holds for the egor
     and the plan it probed (`Proofs`): a call of another shape is
     probed again before any of its strips are dropped. A convolution
     drawn after the last pass is refused (`PassTarget`): the native pass
     is issued into the target bound at the last draw. `H2_POST_SHAPES`
     writes each call's plan (egor, probe or drop, slot:primitive x
     count), the first 64 calls.
   - **Then: the strips were never pushed.** With those, the next A/B
     said `draw 0 of 1: planned TriangleStrip x4 into slot 1, pushed
     QuadList x0, an inline array, after another draw` for the one egor
     the menu calls (`0xe4380218`, `render_screen_effects`' 2 x 2): the
     first draws after arming were the UI's, so `0x0002d140` drew
     nothing. The plan assumed `0x00016610` draws (the differential stubs
     it so), but `0x0002d140` skips a pass whole when it does not
     (`0x0002d4be`). Its gate is now ported,
     `screen_effect::template_draws` (Code, `0x00016610..0x000166ed`:
     the template at the shader's `+4`, its `+0x5c` count, layer 15's bit
     and word, the pass the implementation names, and the selector's
     implementation's `+0x100` not -1), and a call with a pass it skips
     keeps its D3D draws (`Missing::TemplateSkipped`). The menu's other
     egor is never called there; the blur needs another scene.
   - **The next A/B was the same**: no `TemplateSkipped`, the same
     mismatch. So the static reading is set aside for a runtime account,
     `H2_POST_TRACE=N` (`N,after`; empty, 4): for the first `N` calls of
     `0x0002d140` the device keeps, from the hook to the next hook of
     either kind (`d3d::Traced`, the first 512 events): every D3D entry
     the runtime calls and its guest return address (so a call from
     inside `0x0002d140..0x0002db67` is its own); `Arming` before the
     drain at arming, so a draw pushed before the hook shows ahead of
     `Armed`; every draw with its entry, primitive, vertex count, inline
     array words, the push-buffer address of its `End`, the colour target,
     `Stats::draws` before it, and a pushed draw's fate (`drawn`,
     `dropped`, `observed`, `mismatch`). The host adds the call, the
     hook's plan, whether it armed or what it refused, and each
     `0x00016610` (`points::TEMPLATE_PASS`, a context-only `before`) with
     its words, caller and what `template_draws` says of it. `,after`
     notes the returns of `0x00016610` (AL) and `0x0002d140`, after-hooks
     that keep those chains off the pipelined render thread. Each line
     is `native-post: H2_POST_TRACE #<call> ...`, the trace written when
     it ends.
   - **Fixed: the strip comes through the `D3DDevice_End` entry.** The
     trace said `0x0002d140` draws (`0x00016610` from `0x0002d4bc`, the
     port agreeing; the commit from `0x0002d51b`), but its strip is
     `D3DDevice_End` called from `0x0002d8b2` (`Begin`,
     `SetVertexData4f`, `End` as entries), not a pushed `Begin`/`End`.
     The swallow watched only pushed draws, so the strip drew and the
     UI's next pushed quad was taken for it. Both paths now go through
     `Device::swallow_immediate`. A probe that ends early dropped
     nothing, so the egor is only left unproved and its next call probes
     again; only a dropping swallow that ends early keeps the egor on D3D
     (`screen_mismatch`). A call made while the map's effects are still
     loading is no longer counted as a `TemplatePass` refusal.
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
