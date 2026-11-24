# Native renderer stage 4b: water, fog and camouflage

Status: Partly built. Planar fog (layer 16), the water edge blend (layer 6) and active camouflage's layers 18 and 19 and the hologram's layer 21 are drawn natively, and parity with D3D is not verified.
Code: `crates/rewrites/render-bridge/src/native_fog.rs`, `native_water.rs`, `native_layers.rs`, `crates/extensions/game/src/native_fog_glue.rs`, `native_water_glue.rs`, `native_layers_glue.rs`, `native_objects_glue.rs`, `crates/rewrites/native-render/src/fog.rs`, `fog_gpu.rs`, `water_gpu.rs`, `object_program_gpu.rs`, `crates/rewrites/scene-extract/src/layout/world_layers.rs` (`sorted_site`). Tests: `crates/rewrites/render-bridge/tests/native_fog.rs`, the unit tests of `render_bridge::native_layers`, `native_render::object_program` and `object_program_gpu`, `scene_extract::layout::world_layers`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

Part of [native-renderer.md](native-renderer.md).

## Stage 4b: water, fog and camouflage

These are the scene-list layers that are not the world's, the objects' or the lights': water (6), planar fog (16), active camouflage (18, 19) and the hologram (21), plus the scene-copy and reflection inputs water may read. Everything here draws through the same layer draw (`0x00044550`) over the same scene list (`0x004c1a48`) as layer 3, so the skip is the world's shape: clear the layer's bit on the items the native pass draws, restore at the next layer draw, and ask for one external pass per view.

### The census (Code, from the lifted `render_scene_passes` and layer draw ports, `engine::render_loop::scene` and `layer`; pass counts are V on the owner's disc, e3-lighting.md "Pass taxonomy")

The late passes of `render_scene_passes` (`0x0002bcd0`), in order
(view kinds 1 and 2's mode 2 skips them; the fog byte `0x00485a76`
gates the player chain after):

| Calls | What | Evidence |
|---|---|---|
| `0x00025d50`, then the layer draw of 16 | **Planar fog.** `0x25d50` (no stack cleanup) runs between the state call at `0x0002c0bc`'s return and the layer-16 begin at `0x0002c0d6`; the layer draw is at `0x0002c0e0` (returns `0x0002c0e9`). Its content is not read (S: the fog render states and any per-view constants; whatever it pushes is in the device's state before the layer begins, so a native pass asked at the layer draw reads it after the drain). | Code: the block machine `0x0002c0cc`..`0x0002c0ee` |
| `0x0003ba90(esi, ecx=6)`, the layer draw of 6, `0x0003c3c0(al, 1)` | **The water edge blend.** Unless bit 4 of `[0x004b9fe0]` is set, when `0x0003c3c0(0, 0)` runs instead and layer 6 does not draw. `0x0003ba90` (`ret 4`) returns AL, stored and passed back to `0x0003c3c0` (`ret 8`). `0x0003c3c0` contains the alpha-only clear `0x3c515` with EAX `0x10` (d3d.md, "Channels"): the teardown clears the bound target's alpha, so the water edge blend's mask is the alpha channel the draws before it left. The teardown can also issue an indexed quad draw at `0x0003c598` and continues with resource updates. | Code: `0x0002c0ee`..`0x0002c137`; d3d.md |

- **Layer 16's own gate** (Code, the layer-draw port `0x00044582`): the
  layer draw draws layer 16 only when `[0x0048574c] == 3` or either fog
  byte `0x00485a75`/`0x00485a76` is set (the same test as layers in
  `DRAWN_LAYERS` `0x00bffdee`, which holds bits 16, 18, 19 and 21). A
  native fog pass must apply the same gate or it fogs views the game
  left clear.
- **By caller and material.** The layer-16 items are the scene list's
  structure and object items with bit 16 set — every shader with a
  layer-16 pass (16,668 resolved `(lod, layer)` passes on the disc's 30
  maps, against layer 3's 16,622: the fog pass is a per-material pass
  like the lightmap one, drawn where the same geometry drew in layer 3).
  Layer 6 has two passes on the whole disc: the water templates'
  (`shader_templates\...\water_pool`, `water_pool_static`; a water
  surface is a kind-4 transparent part, structure-bsp.md "Observations
  from the disc", so the surfaces themselves are stage 3's, and layer 6
  is only the edge blend over them). Layers 18 and 19 (111 and 30
  passes) are the active-camouflage materials', layer 21 (6) the
  hologram's; their items are object render records, stage 2's.
- **Scene-copy and reflection inputs.** A template pass binds its stages from the instance bitmaps or the engine textures (`ResolvedImplementation::textures`); the engine-texture table (`0x00019530`, the jump table at `0x00019b68`) names every render target a pass can sample: 5 the view's bound target (a scene copy), 2 and 3 target 1 (the texaccum), 6 target 3, 13 target 9 (the shadow map), 8 target 10. Engine textures 14 and 15 bind the rasterizer data's bitmap at `+0x14` (`0x000197dd`, `0x000197fb`: `[[0x00485a80]+0x14]` through `0x000143c0`), 14 its image 0 and 15 its image 1 (14 clears `ecx`, the image argument; 15 leaves the 1 loaded at `0x00019538`); the fog load resolves it from the globals tag (`fog_gpu::Selection::rasterizer`, `look::rasterizer_image`) and samples it as an ordinary bitmap stage. A pass sampling a target the native side has no copy of stays on the D3D path. The fog pass draws before layer 6 and touches no target but the bound one. The water edge blend reads the alpha of the bound target (the teardown clears it after), so its native pass runs inside the wrapper's window, before `0x0003c3c0`'s clear; the pass is issued at the next D3D entry, and the teardown's clear is an entry.

### Planar fog

`feature::native_fog` and
`boot::native_fog_glue`, `native_render::fog`/`fog_gpu`. The most
self-contained of the four: the same geometry the native world already
loaded, drawn with each material's own translated layer-16 pass.
At guest admission, `GpuWorld::prewarm_fog` and `prewarm_water` request
the pipelines for the bound color and depth formats. D3D keeps the layer
while its programs compile. Once the formats are ready, later views use
the cached pipelines with their own uniforms and sampled textures.

- **The look** (`fog_gpu::fog_passes`): every pass of the material's
  layer 16 at the first level of detail, in order, each through
  `look::implementation_look` (a fog pass is exactly the "combiner that
  reads the fog register" that layer 3's `world_parity` refuses: its
  colour is `SET_FOG_COLOR` and its factor the fog unit's, both device
  state at the layer). It refuses what layer 3 refuses — per-draw
  engine inputs, engine textures other than the lightmap, animated
  passes, engine-driven states from a source other than the instance's
  scalar 21 or a driven fog colour with the fog on
  (`engine_states::apply_static`), a definition without
  its own final combiner (D3D builds one from the fog and specular
  states), bump-env/BRDF modes, border addressing — but
  not the fog register: the pass supplies it. The pipeline state must
  be the implementation's own (`pass_state`'s rule but its `Fog`
  refusal, the fog words taken out of the decode): a pass that leaves
  blend or depth to the previous draw stays on the D3D path. `CULLMODE`
  is the exception, below.
- **The implementation** (`fog::layer_implementation`, Code `0x0004cbf0`).
  Layer 16's passes carry four implementations. The geometry build has
  its own case for layer 16 (jump table `0x0004d054`, byte table
  `0x0004d068`, entry `0x0004cdc8`), not `0x00015d70`'s answer, and
  stores its choice in the entry's byte `+4`: from the render record's
  flag word `+0`, bit 7 clear gives no entry (the layer draws nothing of
  the item); bit 7 set and bit 8 clear gives 2 with the planar gate and
  0 without it; both set give 1 without the gate and no entry with it.
  The gate (`fog::planar_gate`) is `[0x004ba004]` non-zero with both fog
  bytes (`0x00485a75`, `0x00485a76`) clear. The skip (`fog::clear_fog`)
  computes the same from the record and carries it on `FogItem`; a
  structure or object record with no entry keeps its mask and contributes no draw.
  The load compiles implementation 2 (`native_fog_glue::LOADED_IMPLEMENTATION`,
  the gate's) for every pass that has more than one
  (`fog_gpu::layer_passes_selected`); a pass with one draws it whatever
  the item binds. `FogView::draws` accepts an item of a material with a
  choice only when its record binds 2, so a view with a fog byte set
  (the gate shut: implementations 0 and 1) stays on D3D. Which bits of
  the record `+0` mean what (7 and 8) is not traced beyond this use.
- **The cull** (`fog_gpu::Selection::layer_cull`). A pass that leaves
  `CULLMODE` unset draws with the cull the layer starts with: the shader
  draw begin (`0x0001bbf0`) runs the layer setup on the first draw after
  a layer draw (byte `0x004670bc`), which calls `0x003f53d0` and
  `0x003f5450` with `0x900` (`SET_CULL_FACE_ENABLE` `0x40308`,
  `SET_FRONT_FACE` `0x403a0`): `CULLMODE` and `FRONTFACE` both `CW`
  (`fog_gpu::layer_cull`; `RenderState`'s own default is `CCW`). Nothing
  restores the cull between draws, so a later unset pass inherits what an
  earlier pass set. The load therefore admits an unset pass only when no
  implementation of any pass of that layer, in any world material, sets
  `CULLMODE` or `FRONTFACE` to something else
  (`fog_gpu::layer_sets_other_cull`; `restore_states` are not credited
  with putting it back). A whole-layer native pass starts from the layer setup. The world
  layers (`layers_load`: 1, 5, 7, 8 and 20) apply the same rule.
- **The fog state** (`fog_gpu::FogWords`): the effective fog unit of a
  draw is the implementation's own render states overlaid on the device's
  (`d3d::nv2a::d3d_fog` over `D3DRS_FOGENABLE`, `FOGCOLOR`,
  `FOGTABLEMODE`, `FOGSTART`, `FOGEND`, `FOGDENSITY`, `RANGEFOGENABLE`,
  read through `Device::render_state` as `stencil_state` reads the
  stencil). The glue resolves it at the layer-16 call and fills the
  programs' `PixelUniforms::fog` and `fog_color`; unreadable, the view
  stays on the D3D path. The vertex side needs nothing: the pass's own
  program writes `oFog` as the D3D path's does (the translated module is
  the D3D path's, text for text), and the constants check is the
  world's (a constant outside the view's `c[0..18]`, the instance's,
  `c[50..52]` and `c[172]` keeps the material on D3D; uploads by `0x25d50` outside that set show as these refusals).
- **The skip** (`feature::native_fog`): one context-only observer before
  `LAYER_DRAW`, after the world's. It restores the last skip, then when
  the current layer is 16, the caller is the late passes' call
  (`[esp] == 0x0002c0e9`), the layer-draw's own gate holds
  (`[0x0048574c] == 3` or a fog byte set) and the list holds a BSP item
  in the layer, asks the host; the host plans the skip over a preview,
  writes the view's fog into the frame step's scene and builds the pass
  from the scene's copy (framescene.md, slice 5), reserves it (a scene
  stage the render thread prepares and records when it takes them,
  else through the ordered handoff, recording on every copy before
  anything is surrendered), checks that the same map and BSP load is
  current, applies the mask edits and commits.
  The fog and water mask walk only surrenders a list when the stage can
  draw every entry. A mixed list keeps all its masks for the guest sort.
  The native range hook then offers each supported geometry range, including selected subparts, at its
  original draw position, after the handler commits its state. A refused
  range continues through its guest draw. The hook restores a surrendered
  list's bits at the next layer draw. A placement's part draws with its pass's
  rigid classification (the `vrtx` tag's classification 1, the model
  rows `c[50..52]` at its slot's dynamic offset, as its layer 3 does);
  a material whose rigid program does not compile, or whose
  placement's rows did not read, keeps those parts on the D3D path (by
  count in the load's log). Objects' layer-16 items are never cleared:
  their fog is stage 2's.
- **The draw** (`fog_gpu::FogSet`, `GpuWorld::encode_fog`): the world's
  parts selected by the mask preview, in list order, each with its
  layer-16 passes blended over the loaded depth with the pass's own
  `PassState` (the 03a fog passes are `ZFUNC` equal without depth
  writes — the depth the world's native pass already wrote bit for
  bit).
- **Diagnostics.** The load line names the materials that stayed on
  D3D and why, and every 1,800 layer-16 views a line gives the skip
  counts.

### Water and the rest

`feature::native_water`,
`native_render::water_gpu` and `boot::native_water_glue`. Two parts:

- **The water edge blend, drawn natively (layer 6).** The skip, the
  scene and the reservation are the fog's: one context-only observer before the layer draw, ordered
  after the world's and the fog's. It runs for layer 6 when the caller
  is the late passes' call
  (`[esp] == 0x0002c120`), gated by the water branch's own gate (bit 4
  of `0x004b9fe0` clear). The reservation drains the setup wrapper's
  pushes before the native draw. The teardown `0x0003c3c0` keeps its
  conditional alpha clear at `0x0003c515` and its state commit. At
  `0x0001c8a5`, the indexed helper offers the composite's quad list to
  the water stage. The keyed view owns the quad corners, and the stage verifies
  them against the current vertex window before expanding them into triangles.
  Admission resumes at the helper's epilogue only after
  the native draw commits. Refusals take the original indexed call.
  Layer 6 draws the kind-4 transparent parts of clusters and placements. A placement uses
  its own model rows at the rigid program's dynamic uniform offset.
  Each pass must compile the rigid classification and the part must supply
  every vertex input it reads. Missing rows, streams or rigid programs
  leave the placement on the guest path. The look is the material's own layer-6 pass
  (`fog_gpu::supplied_layer_passes` — the acceptance is the fog
  layer's: the states the implementation sets itself, a final combiner
  of its own, no device-state texture modes).
- **The scene-copy and reflection inputs, explicit.** A layer-6 pass
  binds engine textures, and each is supplied per view
  (`water_gpu::Supply`): the view's own target (id 5) as a copy of the
  bound colour made inside each Reference and Display recording.
  Each copy caches its own scratch texture by size and format. Render
  targets use the reservation's `sampled` levels. The guest texture
  header is embedded at `+4` in Halo's descriptor table (`0x004b4b58`,
  stride `0x98`). The resolver at `0x0001dcc0` requires a nonzero
  surface at `+0x88` and a clear byte at `+0x95`. The reservation
  refuses if a required copy has an unresolved sample, a sample that
  aliases an attachment, an unfilterable sample, or an unsuitable
  source for the scene copy. Any other
  engine id — run-time chosen, the light's, a rasterizer bitmap, a
  cleared stage — refuses the material at the load
  (`FogRefusal::Unsupplied`).
- **The caller** (Code, `h2 disasm`). Layers 18, 19 and 21 are drawn by
  the sorted transparent list's object-layer handlers, not by the scene
  passes (`0x0002bcd0` draws 1, 3, 5, 8, 20, 7, 2, 16, 6 and 15) or the
  first-person render (`0x0003fdf0`). The list calls an entry's handler
  at `+0x20` with `entry+0x24` (native-decals-research.md, "The
  sorted-list handlers are not decals"):
  - `0x00041cc0`, added by `0x000420a0` for the render records whose
    `+0x1c` is 1, the active camouflage. It starts a list of its own
    (`0x000352e0`), adds the records' items through `0x00044c90` (its
    layer mask `0x0011818e`, layers 1, 2, 3, 7, 8, 15, 16 and 20, and the
    camouflage mask: `0x40000`, layer 18, or `0xc0000`, 18 and 19, when
    the modulate state below holds), replays layers 1, 3, 8, 20, 7, 2
    and 16, then 18 (the draw at `0x00042010`, returning `0x00042015`),
    then 19 when `[0x004b9f5c] != -1`, `[0x004b9f88] == 0` and the larger
    of `[0x004b9f78]` and `[0x004b9f7c]` exceeds `[0x0045dbd8]` (the
    draw at `0x0004205a`, returning `0x0004205f`), then 15.
  - `0x000423c0`, added by `0x00042760` for the records whose `+0x1c`
    is 3, the hologram: one `0x00044c90` call over every layer, then
    layers 1, 3, 8, 7, 2, 16, 15 and 21 (the draw at `0x0004271c`,
    returning `0x00042721`).

  Every layer goes through the layer begin `0x00044370`, the layer draw
  `0x00044550` and the end `0x000445d0` with no wrapper of its own. The
  begin's descriptor (`0x0043fa1c`, `0x24` bytes a layer) gives 18 and
  21 the bound target (`+0`: -1) and 19 render target 3; its per-layer
  jump table covers layers 3 to 17 only, so 18, 19 and 21 have no setup
  but the target and the state call `0x00015680` (descriptor `+0x18`: 1
  for 18, 0 for 19 and 21). The end's table also stops at 17.
- **The shader a record draws** (Code). The record producer
  `0x0002cbf0` takes a shader argument from the object render
  (`0x001341d0`); when it is not -1 it stores it in the table at
  `0x004c1228`, its index byte at `0x004c0ed6 + record` (-1 for none; at
  most `0xc0` a frame, counted at `0x004c1528`). `0x00044c90` hands it
  to the item build `0x00041040`, which draws the item with that shader
  in place of its part's material (`[esp+0x3c]` there): for the
  camouflage handler when the record's `+0` bit 15 is set, for the
  hologram's always (its last argument 1). The record's fade byte `+0x1a`
  below `0xff` sets the polygon stipple in the geometry draw's prologue
  (`0x0004d640`, `SetStipple` `0x003f8cc0`).
- **Layers 18 and 19 (active camouflage) and 21 (hologram), built.**
  `frame-scene.layers` takes them (`render_bridge::native_layers`,
  `LayerCaller::Sorted`; `sorted_site` names the handler from the layer
  draw's return): the
  items are object records only and draw from the scene's resident
  models, with the plan, skip and restore of the other object layers
  (framescene.md, slice 4). The bit goes back at the next layer draw
  only on an item that still holds the mask, shader and argument it
  left: 18, 19 or 21 can be the handler's last taken layer, and the next
  entry's handler rebuilds the same items. The begin runs before the
  layer draw, so the stage's pass draws on what it bound: the view's
  target for 18 and 21, render target 3 for 19. A hologram item always
  draws the shader its record names, a camouflage item when the
  record's `+0` bit 15 is set; either way the item's shader is the one
  its passes are looked up by.
  - **Override shaders** (`object_program_gpu::override_sections`,
    `native_objects_glue::override_materials`). The map's shaders with
    passes in layer 18, 19 or 21 (`OVERRIDE_LAYERS`) are read once per map;
    each model compiles their passes over every drawn part, after its
    own (`LitModelInput::append`), keyed by part and shader, so an item
    draws the passes of the shader its record names. One whose passes
    do not compile keeps its D3D draws; the first reason per shader and
    layer is logged.
  - **The view's own target** (engine texture 5,
    `object_program::VIEW_TEXTURE`). A stage of layer 18, 19 or 21 that
    samples it binds a copy of the bound colour (`LitObjects::view_copy`,
    one scratch texture per target copy): the pass is recorded as
    commands (`record_encoded`), the copy first, then a render pass of
    its own on the bound attachments with the draws
    (`gfx::renderer::encode_in_pass`). The coordinates are texels of the
    target's guest size, as a linear render target is read
    (`texaccum_dimensions`, `texaccum_scale`). In any other layer a stage
    sampling the view refuses at load.
## Rules and limits

- The fog pass applies the layer draw's own gate (`[0x0048574c] == 3` or a fog byte set); otherwise it would fog views the game left clear.
- Objects retain their layer-16 mask. In a mixed list, range admission uses the shader, geometry and texture bindings established by the sorted guest handler. A range that cannot sample its bound textures safely stays on the guest path.
- Any engine texture id that is run-time chosen, the light's, a rasterizer bitmap other than 14 and 15 (which resolve) or a cleared stage refuses the material at load (`FogRefusal::Unsupplied`). The load line names the materials left on D3D and why, and every 1,800 layer-16 views a line gives the skip counts.
- Layers 18 and 19 are taken only from the camouflage handler's calls (returns `0x00042015` and `0x0004205f`), 21 only from the hologram handler's (`0x00042721`); the handlers' replays of the scene's layers stay on D3D. An item extraction refuses (a fade byte below `0xff`, which sets the stipple, and the rest of `LayerObjectRefusal`), whose model is not resident, or whose shader's passes in the layer did not compile keeps its D3D draw, and so does every item after it.

## Not verified

- The meaning of the render record's `+0` bits 7 and 8 and of `[0x004ba004]` (only their use in layer 16's build is read), the fog implementations 0 and 1 (views with a fog byte set stay on D3D), and that no draw between a layer's setup and the native passes changes the cull (read from the layer draw's order, not a captured trace).
- Pixel parity of fog and the water edge blend against D3D, including the alpha channel the teardown clears.
- The content of `0x00025d50` (what the fog render states and per-view constants it pushes).
- Pixel parity of layers 18, 19 and 21 against D3D (19 drawn into render target 3): the view copy is made once before the layer, so a later item reads the target as it was before the earlier items drew, where the D3D path samples the target it draws into; and that the camouflage and hologram shaders' engine inputs are ones the object path supplies (their refusals are logged at load).
- What `0x00015680` sets from the layer descriptor's `+0x18` (read from the device at the call, not from the descriptor).

## Related

[native-renderer.md](native-renderer.md), [native-world.md](native-world.md), [native-transparents.md](native-transparents.md), [layer-draw.md](layer-draw.md), [d3d.md](d3d.md).
