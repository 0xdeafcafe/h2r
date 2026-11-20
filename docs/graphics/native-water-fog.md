# Native renderer stage 4b: water, fog and camouflage

Status: Partly built. Planar fog (layer 16) and the water edge blend (layer 6) are drawn natively; camouflage (layers 18 and 19) and the hologram (layer 21) are not, and parity with D3D is not verified.
Code: `crates/rewrites/render-bridge/src/native_fog.rs`, `native_water.rs`, `crates/apps/boot/src/native_fog_glue.rs`, `native_water_glue.rs`, `crates/rewrites/native-render/src/fog.rs`, `fog_gpu.rs`, `water_gpu.rs`. Tests: `crates/rewrites/render-bridge/tests/native_fog.rs`.
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
| `0x0003ba90(esi, ecx=6)`, the layer draw of 6, `0x0003c3c0(al, 1)` | **The water edge blend.** Unless bit 4 of `[0x004b9fe0]` is set, when `0x0003c3c0(0, 0)` runs instead and layer 6 does not draw. `0x0003ba90` (`ret 4`) returns AL, stored and passed back to `0x0003c3c0` (`ret 8`). `0x0003c3c0` contains the alpha-only clear `0x3c515` with EAX `0x10` (d3d.md, "Channels"): the teardown clears the bound target's alpha, so the water edge blend's mask is the alpha channel the draws before it left. Both wrappers' content is otherwise not read. | Code: `0x0002c0ee`..`0x0002c137`; d3d.md |

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
- **Scene-copy and reflection inputs.** A template pass binds its stages from the instance bitmaps or the engine textures (`ResolvedImplementation::textures`); the engine-texture table (`0x00019530`, the jump table at `0x00019b68`) names every render target a pass can sample: 5 the view's bound target (a scene copy), 2 and 3 target 1 (the texaccum), 6 target 3, 13 target 9 (the shadow map), 8 target 10. A pass sampling a target the native side has no copy of stays on the D3D path. The fog pass draws before layer 6 and touches no target but the bound one. The water edge blend reads the alpha of the bound target (the teardown clears it after), so its native pass runs inside the wrapper's window, before `0x0003c3c0`'s clear; the pass is issued at the next D3D entry, and the teardown's clear is an entry.

### Planar fog

`feature::native_fog` and
`boot::native_fog_glue`, `native_render::fog`/`fog_gpu`. The most
self-contained of the four: the same geometry the native world already
loaded, drawn with each material's own translated layer-16 pass.

- **The look** (`fog_gpu::fog_passes`): every pass of the material's
  layer 16 at the first level of detail, in order, each through
  `look::implementation_look` (a fog pass is exactly the "combiner that
  reads the fog register" that layer 3's `world_parity` refuses: its
  colour is `SET_FOG_COLOR` and its factor the fog unit's, both device
  state at the layer). It refuses what layer 3 refuses — per-draw
  engine inputs, engine textures other than the lightmap, animated
  passes, engine-driven render or stage states, a definition without
  its own final combiner (D3D builds one from the fog and specular
  states), bump-env/BRDF modes, border addressing — but
  not the fog register: the pass supplies it. The pipeline state must
  be the implementation's own (`pass_state`'s rule but its `Fog`
  refusal, the fog words taken out of the decode): a pass that leaves
  blend, depth or `CULLMODE` to the previous draw stays on the D3D path
  (a pass inheriting the device's cull at the call, as layer 3 does, is not admitted).
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
  The shared fog and water mask walk requires the accepted items to
  form a prefix of the layer's list so the guest remainder draws in
  order after the native pass. A later accepted item leaves the
  whole layer on D3D. The hook remembers what was cleared for the restore
  at the next layer draw. A placement's part draws with its pass's
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
  of `0x004b9fe0` clear). The wrappers are never skipped — `0x0003ba90`
  before the layer and `0x0003c3c0` with its alpha-only clear
  `0x3c515` after it stay guest — so the native replacement lands
  inside their window (the reservation drains `0x0003ba90`'s pushes
  first, and the teardown's clear is a later guest entry) and the
  alpha channel's lifetime is the game's own. The geometry is the
  water surfaces themselves, the kind-4 transparent cluster parts.
  Placements keep their D3D draws until the water path has their rigid
  programs. The look is the material's own layer-6 pass
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
- **Layers 18/19 (camouflage) and 21 (hologram)**, not drawn. What
  the repo establishes (Code, the lifted ports; the disc census,
  e3-lighting.md "Pass taxonomy"; `engine::render::Role`): the two
  camo layers are named ActiveCamo and ActiveCamoModulate (111 and 30
  passes on the disc's 30 maps) and the hologram's 6; all three draw
  through the same scene-list layer draw (`DRAWN_LAYERS` holds their
  bits) — but **no lifted port names their caller**: the scene passes
  (`0x0002bcd0`) begin and draw layers 1, 3, 5, 8, 20, 7, 2, 16, 6
  and 15 only, the first-person render (`0x0003fdf0`) 1, 3, 8, 20 and
  7, and the light passes 10 to 14. The caller is therefore some
  function no port covers — most plausibly in the object-render chain
  the camo wraps (`0x00133790`'s; the camo fades with the parent's
  `+0x2b0`, e3-lighting's table), an S. Finding the caller needs a new trace. Drawing them is
  refused outright: nothing about their wrapper (whether, like water,
  a setup and a teardown bracket the layer, or whether the draw is
  per-object) or their passes' state is established, so there is
  nothing to admit against the handoff's contract.

## Rules and limits

- The fog pass applies the layer draw's own gate (`[0x0048574c] == 3` or a fog byte set); otherwise it would fog views the game left clear.
- Water: the shared fog and water mask walk requires the accepted items to form a prefix of the layer's list; a later accepted item leaves the whole layer on D3D. Placements keep their D3D draws until the water path has their rigid programs. Objects' layer-16 items are never cleared: their fog belongs to the objects stage.
- Any engine texture id that is run-time chosen, the light's, a rasterizer bitmap or a cleared stage refuses the material at load (`FogRefusal::Unsupplied`). The load line names the materials left on D3D and why, and every 1,800 layer-16 views a line gives the skip counts.
- Layers 18, 19 and 21 are refused outright: no lifted port names their caller, so their wrappers and state are not established.

## Not verified

- Pixel parity of fog and the water edge blend against D3D, including the alpha channel the teardown clears.
- The content of `0x00025d50` (what the fog render states and per-view constants it pushes).
- The caller, wrapper and pass state of layers 18, 19 and 21.

## Related

[native-renderer.md](native-renderer.md), [native-world.md](native-world.md), [native-transparents.md](native-transparents.md), [layer-draw.md](layer-draw.md), [d3d.md](d3d.md).
