# Native shadow layers from the scene

Status: Built. Layers 10, 11 and 14 draw from their whole scene groups with no guest geometry handler. An object mask drawn alone goes into a private chain, with its filter over the stage's own pair; the guest's begin, end and filter call still run, the disc census is not built, and parity with D3D is not verified.
Code: `crates/rewrites/native-render/src/shadow_look.rs`, `shadow_scene.rs`, `shadow_filter_gpu.rs`, `shadow_chain.rs`, `light_models.rs`, `light_gpu.rs`, `crates/rewrites/scene-extract/src/light_groups.rs`, `crates/rewrites/render-bridge/src/native_shadows.rs`, `native_lights/walk.rs`, `crates/extensions/game/src/native_lights_glue.rs`, `native_shadows_glue.rs`, `native_shadow_chain.rs`. Tests: `native_render::shadow_look`, `shadow_scene`, `shadow_filter_gpu`, `shadow_chain`, `boot::native_shadow_chain`, `light_gpu`, `scene_extract::light_groups`, `render_bridge::native_shadows`, `crates/rewrites/native-render/tests/shadow_gpu.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

The shadow stage draws layers 10, 11 and 14 from their gathered groups
in the frame scene (`frame_scene::LightGroup`) with no guest geometry
handler run, as the light stage draws layers 12 and 13
([native-lights.md](native-lights.md), "Gathered groups in the scene").
`native_render::shadow_look` compiles the passes from tags,
`shadow_scene` plans the group, and `native_lights_glue::shadow_alone`
reserves the packets on the guest target before the walk empties the
entry loop. Receivers take their frozen depth or colour-mask map from
the boundary's `ReceiverTerm`. The guest's begin, clear and end still
run. The object filter goes through `native_render::shadow_filter_gpu`
inside a swallow at `0x000222a0`, so its guest quads are dropped after
validation. The one rule: at every point a guest draw or the freeze
reads target 9 or 10, that target holds what the guest's own casters and
filter would have left there. A private chain keeps it by having no such
reader, or by copying its mask back first.

Evidence labels as in [docs-style.md](../meta/docs-style.md): **E**
(`h2 disasm`), **Code**, **S**, **Design**.

## How it works

### What the guest does around the three layers

Three callers draw the layers, each through the layer begin `0x00044370`
(EDI the layer), the layer draw `0x00044550` and the layer end
`0x000445d0` (**E**).

| Caller | Order | Filter |
|---|---|---|
| The light passes `0x00031170` | projection `0x00048ee0`; begin, draw, end of 10; target, `0x000496f0`, stencil mode, draw, end of 11; `0x00030ed0`; 14; 12; 13 | none |
| The additional casters `0x00030ed0` | begin, draw, end of 10 and the same inline 11, on a suffix of the lists | none |
| An object shadow `0x00032350` | constants `0x0004a1e0`; projection; both lists' layer bits checked; begin, draw, end of 10; `0x000222a0`; target, `0x000496f0`, stencil mode, draw, end of 11; tail `0x0004a780` | `0x000222a0` |

- **The target** (**E**, `0x00044408`..`0x000444ad`). For layer 10 the
  begin binds target 10 when pass flag bit 4 is set and bit 5 clear, and
  clears its colour; otherwise target 9, and clears depth and stencil.
  [native-shadows.md](native-shadows.md) has the clear values.
- **The raster state** (**E**, `0x000495e0`, reached from the begin's
  jump table for layer 10). Four device writes: the depth clip control,
  one of two words by pass flag bit 4; a render state chosen by the byte
  at `0x0047fe84` (**S**: which state; a trace of the setter
  `0x003f5a60` names it); solid polygon offset on; and the offset's
  constant and slope from two floats of the image. Each is also stored
  in the game's state words at `0x004b8344`, `0x004b8464`,
  `0x004b8340`, `0x004b8334` and `0x004b8330`. The end's `0x000496a0`
  clears them.
- **The compare** (**E**, `0x000496f0`). Before layer 11's draw: the
  shadow function `GREATER` (state word `0x004b846c`), the depth clip
  control, and one state from `0x005093b4`. The end's `0x00049740`
  sets the function back.
- **Stencil** (**E**). `0x00015370` takes a mode in BX and stores it at
  `0x00467010`; mode 0 is one write that turns the stencil test off.
  The begin passes mode 0 for every layer but 1 and 3, and the callers
  pass 0 before layers 11 and 14. The geometry handler `0x0004d0b0`
  calls `0x00015370` twice, both under a test that the layer is 1 or 3.
  So nothing the layer machinery runs gives a shadow layer's draw a
  stencil test. **S**: no template pass of layers 10, 11 or 14 sets a
  stencil state through its own state block (slice 2's tag census
  answers it).
- **The filter** (**E**). `0x000222a0` is one call of the blur
  `0x000211a0`: EAX the mask target 10 as the source, target 11 as the
  first destination, the "other" word unset (so the source), and target
  10 as where the result is copied when the last quad wrote elsewhere
  (`0x000351a0`). Its combiner byte is set with strength and `y` zero,
  the combiner `engine::glow::blur_combiner` makes. Its eighth stack
  word is set, and `0x000211a0` passes that word to the quad draw
  `0x00034a90` as its tenth argument (`0x000212af`..`0x000212e7`).
  `0x00032350` calls `0x000222a0` after the end of layer 10 whether or
  not the begin drew.
- **Who reads the targets.** The filter's quads read 10 and 11. Layer
  11's receivers read target 9's depth through engine texture 13 or the
  filtered target 10 through engine texture 8. The shadow stage's
  freeze (`boot::native_shadows_draw::Freeze`) copies the bound target
  at the start of layer 11. Nothing else reads them (**S**: a
  `render.frame` of the gate corpus with `draws` shows every draw that
  binds either header).

### The compiler

`native_render::shadow_look` compiles layers 10, 11 and 14 by
implementation, vertex shader and vertex classification, the way
`native_render::light_look::compile_light` does for layers 12 and 13: the
template pass's vertex microcode over the stream's declaration, its
pixel side through `shaders::psh`, and its state block.

| Layer | Geometry | Implementations | Inputs beyond the material |
|---|---|---|---|
| 10 | structure | 2 | the projection block in `c[40..48]` |
| 10 | models | 0 (the colour mask), 1 (depth) | the same, node rows at `c[50..]`, the section's decompression rows |
| 11 | both | 0 to 6 | the projection block; engine texture 13 as a D24 shadow stage, 10 the gel, 8 the filtered mask |
| 14 | both | 0 | none of the shadow's |

- **The projection** comes from `LightGroup::projection`, read at the
  layer's build after `0x00048ee0` wrote it. The compiler never reads
  the block from the guest.
- **Casters.** The geometry chooser `0x00030cd0` picks the
  implementation ([native-shadows.md](native-shadows.md), "Caster
  enumeration and projection"); the entry carries it
  (`LightBinding::implementation`). The pixel side is the pass's own,
  so a material's alpha kill cuts the silhouette as it does on D3D. A
  program that writes W depth draws with the pipeline's offset off
  (**Code**, `boot::native_shadows_draw::Range::prepare`).
- **Receivers.** Each stage that binds engine texture 13 becomes
  `shaders::psh::shadow::Shadow` with `DepthFormat::D24` and the
  function `GREATER`, from `SampledMap::shadow_stage` with the stage's
  own filter and addressing. Engine texture 10 binds the light's gel
  ([native-gels.md](native-gels.md)) and 8 the frozen colour mask. The
  blend, alpha test, depth test and channel writes are the pass's
  state block; [native-shadows.md](native-shadows.md), "Application
  and layer 14", lists what each implementation leaves. Implementation
  5 is not in that list (**S**: slice 2's tag census shows whether any
  material selects it; unselected, it compiles from its template like
  the rest).
- **Layer 14** compiles as a receiver without shadow inputs, into the
  colour surface of target 1 with the view's shared depth surface. Its
  geometry carries no shadow projection, so `shadow_look` keeps a shader
  that reads `c[40..48]` on the guest path.
- **Geometry.** Structure entries draw from the world's parts
  (`native_lights_glue`'s `Held`). Model entries draw from the per-map
  model parts the light passes build (`native_render::light_models`);
  the map worker adds the vertex shaders that layers 10, 11 and 14 of
  each part's material name, so each has a vertex buffer for its
  classification.
- **Refusals** are named (`light_look::LightRefusal`,
  `shadow_scene::Refusal`): a program that does not translate, any engine
  texture in a caster, a state the pipeline does not model, or a
  classification with no buffer.

### The plan and the state

`native_render::shadow_scene::plan` reuses
`native_render::light_scene::plan_entries`: the sorted order, duplicates
kept, adjacent list ranges of one stream, binding, class and pose joined.
A group with a remainder is dropped (`remainder`); its handlers do not run.

The scene carries what the layer's begin decided, so the plan needs no
device. `frame_scene::LightGroup` holds `pass: Option<ShadowPass>` for
layers 10, 11 and 14:

- the bound target (`0x004858b4`) and, for layer 10, its kind (depth or
  colour mask, by the pass flags) and size;
- the pass flags (`0x004ba014`);
- the raster words of `0x000495e0` as the game stored them: the clip
  control, the offset's enable, constant and slope, and the fourth
  state;
- the stencil mode (`0x00467010`) and the shadow function
  (`0x004b846c`).

`scene_extract::light_groups` reads them at the layer's build, which
runs after the begin. `frame_scene::summary_hash` covers them. The
values are read from the running image; none is in the repository.

Per draw, the pipeline state is the pass's state block applied over the
`ShadowPass`, the order the guest applies them in. Stencil is off unless
a pass turns it on; a pass that does is drawn with its stencil state
when `gfx::state` models every word of it, and refused by name
otherwise.

### The guest targets: three options

| | A. Casters into the guest's targets | B. Private targets, filter native | C. Hybrid |
|---|---|---|---|
| What it is | The planned casters are one external pass into the bound target (`Device::external_pass_now`), after the begin's clear. The guest's filter and the freeze read that target as they do today. | Casters go into a map the stage owns (`native_render::shadow_gpu::Producer`). The filter is drawn natively over a private pair. Receivers sample the result. Targets 9, 10 and 11 are not written. | A for every layer that draws alone. The filter drawn natively into the guest's targets with the guest's quads swallowed. B for a site once its guest entry is replaced. |
| Correct when | always: the target holds the same casters whether the next reader is the guest's filter, a D3D receiver or a native one | only when the filter and every receiver of the chain are native too | always |
| How it fails | A refusal before the issue leaves the handlers running: nothing is lost. A pass that fails after the entry loop was emptied leaves the map without casters: that generation's shadow is missing. | A receiver left on D3D reads the guest target, which holds the previous chain's map: a wrong shadow on the wrong surface. Layer 11's group does not exist when layer 10 must choose, so the choice needs a prediction. | as A until the entry is replaced; after that there is no D3D reader |
| Guest work saved | every geometry handler of the layer and the resolve of its draws | the same, and the begin, clear, end and the filter's draws | A's, then B's |
| GPU cost | today's: the same draws into the same target, the same freeze | one colour copy less per object shadow; maps can share a pass | A's, then B's |
| New code | the compiler, the plan, the recorder | the same, a chain decision, the filter, private targets | all of it, in an order where each step is checked against the guest's own result |

**B alone is not safe before the entry is replaced.** An object shadow's
receivers are gathered and sorted at layer 11's build, after the casters
and the filter have drawn. Choosing private targets at layer 10 means
walking the receivers' list early and trusting it unchanged (**S**:
`0x00032350` gathers both lists before its first layer, so an early walk
would see the same items). When the prediction is wrong the picture is
wrong and nothing can fall back. Option A needs no prediction: each layer
decides alone, as each light layer is its own reservation today.

**B is where the path ends.** [framescene.md](framescene.md) replaces
`RENDER_LIGHT_PASSES` and `OBJECT_SHADOW` whole (section 8.2), and then
the begin does not run, no guest target is bound or cleared, and a
receiver the stage cannot draw is a named drop, not a D3D draw. From
there the guest targets have no reader and private targets are the only
choice.

**The design is C** (**Design**). A is not a stopgap: it is the mixed
path for as long as any layer of a chain can stay on D3D, and its
compiler, plan and recorder are B's unchanged; only the attachment
differs. The filter is drawn natively into the guest's targets first,
where it is compared with the guest's own quads on the same input,
before any chain depends on it.

### Option A in the frame

At the layer's build, with the group in the scene and whole,
`render_bridge::native_shadows::draw_alone` asks the host to draw it:

1. Drain to the call boundary (`Device::begin_external_boundary`); a
   pending push callback refuses.
2. Check the device against the group's `ShadowPass`: the bound target,
   its size, the clip control, the offset and the stencil state. A
   difference drops the layer (`state`). The size is the pass's own, so
   a scaled view draws its layers like an unscaled one.
3. Plan the entries, resolve every program, buffer and texture, and
   build every pipeline. A missing one refuses.
4. For layer 11, take the receiver term's frozen map
   (`ReceiverTerm::sampled`); `None` refuses.
5. Reserve one sealed packet run on every required target copy
   (`Device::prepare_external_bound`). `None` refuses.
6. Hand the guest's entry loop no entries.

A refusal at 1 to 5 issues nothing and drops the layer under its cause
(`scene_dropped_shadow_layers_<cause>`); the guest's handlers do not run. The begin and end still run, so the clear, the
raster state and its restore are the guest's. The freeze stays at the
start of layer 11, and a caster generation is counted as today, so
`observe_receivers` consumers see no change.

A pass that fails while recording on the render side, after step 6, is
counted in `scene_dropped_shadow_layers` with one log line for its
cause. Step 3 makes that a device loss or a bug.

### The filter, native

`engine::glow::blur` gives the quads of `0x000211a0` from the step
count, scale and growth words in the filter wrapper's three pushes.
The wrapper passes zero strength and zero `y` to
`engine::glow::blur_combiner`. `scene_extract::light_groups::shadow_filter` puts
the quads, final copy, signs and target size in the current
`frame_scene::ObjectShadow` (`filter`). The observer at the blur
(`0x000211a0`) accepts the filter wrapper's return site `0x000222c7`.
The wrapper pushes this address in guest code on either executor.
Its ten stack arguments put the wrapper's own return slot at the
blur's entry `ESP + 44`. The device uses this slot to track the
swallow through the wrapper's return. The native pass
uses a full-target rectangle, as `0x00034a90`'s tenth argument selects,
and each source's own size for its taps.

While the entry is the guest's, the guest's call runs and its quads are
swallowed (`Device::swallow`, as the post stage swallows the glow's).
`native_render::shadow_filter_gpu` draws the same combiner into the
guest's targets 10 and 11 and performs the final copy back to 10.
`ShadowFilterPass::validate` checks every original quad's colour state
and source headers before the guest draws are dropped. The device state
the call leaves is the guest's own, which the receiver after it may
depend on.

### A private chain

An object mask whose layer-10 group draws alone takes a private chain
(`boot::native_shadow_chain`). Its casters, its filter and its receivers
use a `native_render::shadow_chain::Pair` the stage owns: the mask, the
filter's other target and the casters' depth, at target 10's format and
size.

1. **Offer.** At the layer's build, `native_shadows_glue`'s `draw_alone`
   offers a `Chain` when the group is an object's colour mask and the
   layer-10 call is the boundary's own. `native_lights_glue::shadow_alone`
   claims it as it reserves the pass and wraps the planned casters in
   `PrivateCasters`. A group that refuses, or a reservation that fails,
   leaves the offer unclaimed and the layer as option A.
2. **Casters.** `PrivateCasters` records the planned packets with the
   bound target's formats, makes the pair, and encodes them into the
   mask with the begin's clear (white, zero alpha; depth 1, stencil 0).
   The guest's begin still clears target 10; no caster writes it.
3. **Filter.** The swallow at `0x000222a0` is the same as slice 5, but
   `ShadowFilterPass` draws its quads over the pair and makes the final
   copy into the private mask. It then publishes the mask into the
   layer-10 call's output.
4. **Receivers.** At layer 11 the boundary requests no freeze; the
   receiver term resolves to the published mask, both for a group drawn
   alone and for a resolved receiver the gate admits.

**Spill.** A filter the stage does not take (refused before or by the
host, or a layer-10 end that does not match its call) runs the guest's
quads, which read target 10. Before they do, `Spill` copies the private
mask into target 10 and the chain becomes the guest's again: the guest's
filter writes targets 10 and 11, and the freeze runs at layer 11.

A pass that fails while recording after its reservation (the pair cannot
be made, the filter finds no pair, a spill does not fit target 10) draws
no shadow for that generation. It is counted in
`scene_dropped_shadow_layers` with one log line for its cause.

Still to come, with the guest calls they remove: the caster body skips
the layer-10 begin, clear and end and the filter call once their writes
are mirrored (framescene.md, 5.2), so a committed chain binds no guest
target at all. Light chains (depth maps on target 9) keep option A: their
freeze shares the map with additional casters. Object shadows of one
view may share a pass; each map keeps its own texture or a gutter the
filter's taps cannot cross (**Design**, decided when measured).

### Slices

Each slice lands under `impl.render.native`. The checks are the
coordinator's; lanes write code and tests.

| Slice | Status | Draws change |
|---|---|---|
| 1 | Built: `ShadowPass` in the scene and its extraction | no |
| 2 | Partly built: `shadow_look` and the model parts' shadow buffers; the tag census is not built | no |
| 3 | Built: layer 10 alone at all three sites, with parity unverified | casters from the scene |
| 4 | Built: layers 11 and 14 alone, with parity unverified | receivers from the scene |
| 5 | Built: the object filter native, swallowed, with parity unverified | the filter's quads |
| 6 | Partly built: object-mask chains private, with spill; the guest's begin, end and filter call not yet skipped | targets |

**Slice 1.**
- Tests (synthetic memory): `scene_extract::light_groups` reads each
  word of `ShadowPass` at a layer-10, 11 and 14 build, and gives `None`
  for 12 and 13; `frame_scene::summary` shows each word changes the hash.
- `render.scene` counters: `scene_shadow_groups`,
  `scene_shadow_groups_whole`, each also per layer
  (`scene_shadow_groups_10`).
- Census: unchanged from the commit before. `sim.hashes`: identical
  over the gate corpus (extraction only reads).

**Slice 2.**
- Tests: an invented caster pass compiles valid WGSL with the projection
  in `c[40..48]`, a receiver pass compiles its D24 compare stage, and a
  material that overwrites those registers is refused
  (`native_render::shadow_look`).
- Disc census: not built. It still needs to cover every map, shader,
  layer, implementation and classification, count each refusal, and
  answer the two **S** items on implementations and stencil. Its proposed
  `scene_shadow_program*` counters are not present.

**Slice 3.**
- Tests: `native_render::light_gpu` renders a tiny caster scene and checks
  its pixels and projection uniforms; `native_render::shadow_scene`
  checks pass-state refusals; `render_bridge::native_shadows` checks that
  a whole group commits at any size and a refusal is dropped under its cause. The planned
  differential of a planned group against resolved draws, including depth,
  colour mask, stencil and bias, is not run here.
- `render.scene` counters: `scene_shadow_layers_alone_10`,
  `scene_dropped_shadow_layers_<cause>` (`remainder`, `stream`, `pose`,
  `program`, `state`, `target`, `callback`, `device`),
  `scene_dropped_shadow_layers` (zero).
- Census: `layer.light.10` and `layer.object_shadow.10` count no
  backend draw on the gate corpus where the counter above equals
  `scene_shadow_groups_10`. `object_shadow` still counts the filter's
  quads and layer 11's.
- `render.compare`: frames of the gate corpus against
  `impl.render.native=off` within the bar. For the map itself,
  `render.capture {pass}` of the caster pass on both launches,
  compared; a difference in the mask is a fail at any size.
- `light_walk_handlers_skipped` rises by the casters' handlers.

**Slice 4.**
- Tests: `native_render::light_gpu` samples a frozen map and checks every
  pixel, and the bridge commits receiver and opacity groups. The mixed
  differential against resolved draws for implementations 0 to 4 and 6
  and layer 14 is not run here.
- `render.scene` counters: `scene_shadow_layers_alone_11` and `_14`.
- Census: `layer.light.11`, `layer.light.14` and
  `layer.object_shadow.11` count no backend draw; `light_passes` counts
  none where layers 12 and 13 are alone too; `object_shadow` counts
  exactly the filter's quads.
- `render.compare` as slice 3, and at the E3 lighting setting, where
  the scaled copies draw their layers like the rest.

**Slice 5.**
- Tests: `native_render::shadow_filter_gpu` copies a tiny mask through
  target 11 and back to target 10 and checks every pixel. The lifted
  quad differential and resolved-quad comparison are not run here.
- `render.scene` counters: `scene_shadow_filters_native`,
  `scene_dropped_shadow_filter_<cause>`.
- Census: `object_shadow` counts no backend draw.
- `render.capture {pass}` of the filtered mask, compared with the D3D
  launch's.

**Slice 6.**
- Tests: `native_render::shadow_chain` clears a pair as the begin does,
  filters an invented mask through it and back, and spills only into a
  matching texture; `boot::native_shadow_chain` checks the offer is
  claimed once and the filter publishes into the receivers' output;
  `render_bridge::native_shadows` checks the chain and drop counters and
  that a filter refused before the host lets it spill.
- `render.scene` counters: `scene_shadow_chains_private` (object chains
  whose casters went private) and `scene_shadow_chains_spilled` (of
  those, chains handed back to target 10).
- Census: `object_shadow` counts no backend draw, as slice 5;
  `scene_shadow_chains_spilled` is zero over the gate corpus.
- `render.capture {pass}` of the receivers against the D3D launch's.
- Still to come, the entry's own rules:
- The four rules of framescene.md 8.2 for `light_passes` and
  `object_shadow`, with the census of this page's counters at zero
  refusals over the census corpus.
- Tests: the entry's differential against the lifted original for the
  tick-visible writes (framescene.md 5.2).
- `render.parity` within the bar on 20 armed frames.
- `scene_dropped_shadow_layers` is zero over the gate corpus.

## Interfaces

- **In.** `FrameScene::groups_of`, `shadow_groups_of` and `entries_of`
  with `LightGroup::pass` and `projection`; the world's parts and
  `native_render::light_models`; the receiver term of
  `boot::native_shadows_glue`.
- **Out.** Sealed external packet runs on the D3D device
  (`prepare_external_bound`) and the filter's swallowed quads; for an
  object mask, a private `Pair` and its `SampledMap` per generation.
- **Guest points.** `LAYER_BUILD` (where a group is drawn alone),
  `LIGHT_GEOMETRY_DRAW` (`0x0004d0b0`, not run for a layer drawn
  alone), `RENDER_LIGHT_PASSES`, `OBJECT_SHADOW`,
  `SCREEN_CONVOLUTION_BLUR` (the filter's call, return `0x000222c7`)
  and `OBJECT_SHADOW_FILTER` (the wrapper returning to `0x00032966`).
- **What the guest still writes**: the target binds and clears, the
  raster state and its restore, the filter's device state. With the
  entry's calls skipped: the tick-visible writes of framescene.md 5.2
  only.
- **Agent API.** No new method. The counters above are rows of
  `render.scene`'s `counters`, documented in
  [agent-api.md](../runtime/agent-api.md) by the slice that adds them.
  `render.capture {pass}` and `render.compare` are used as they are.

## Rules and limits

- A layer draws alone only when its whole group does. There is no
  split between scene draws and handlers in one layer.
- A layer never draws alone into a target the device does not have
  bound, and never with a resolved fallback: every refusal is before
  the issue and is a counted drop.
- A private chain has no guest reader of target 10 or 11. A guest draw
  that would read target 10 (the guest's filter) runs only after a
  spill has copied the private mask there.
- Scaled and E3 display copies and interpolated replay draw their
  shadow layers from the scene, in slot 3 before layer 7. Nothing keeps
  the original renderer.
- A query-counting draw, a material animation and a template callback
  are remainders of the group by cause; their layers keep the handlers.
- Model casters need `native_render::light_models`. Without it only
  structure entries draw alone and a group with a model entry is
  refused (`stream`).
- No value read from the image is written into the repository: the
  offset's floats, the clip words, the filter's arguments and the clear
  values are read from the running game.

## Evidence

- **E** `0x00044370`: the layer-10 target choice and clears, the jump
  to `0x000495e0` and `0x000496f0`, the stencil mode at its end.
- **E** `0x000495e0`, `0x000496a0`, `0x000496f0`, `0x00049740`: the
  writes listed under "What the guest does".
- **E** `0x00015370`: mode 0 turns the test off; the mode is stored at
  `0x00467010`. `0x0004d0b0` calls it at two places, each after a
  compare of the layer with 1 or 3.
- **E** `0x00032350` (`0x00032949`..`0x000329de`): the object chain's
  order, and that `0x000222a0` is called outside the begin's branch.
- **E** `0x00031170` and `0x00030ed0` do not call `0x000222a0`.
- **E** `0x000222a0`: a single call of `0x000211a0`; `0x000211a0`
  hands its eighth stack word to `0x00034a90`.
- **Code** `Device::external_pass_now` draws into the bound colour and
  depth and takes the levels a pass samples from their guest headers
  (`crates/platform/d3d/src/device.rs`).
- **Code** `boot::native_lights_glue`'s `Glue::alone` is the pattern
  for steps 1 to 6 (`crates/extensions/game/src/native_lights_glue.rs`).

## Not verified

- Whether a dynamic light ever sets pass flag bit 4 without bit 5, and
  so draws a colour mask.
- Which render state `0x000495e0` sets from the byte at `0x0047fe84`,
  and whether `gfx::state` models it.
- Whether `render.capture {pass}` shows target 9's depth; if not, the
  depth map is checked only through the frame and the GPU differential.
- That no template pass of the three layers sets a stencil state, and
  which receiver implementations the disc's materials select.
- The filter's exact pixels against its lifted original and against D3D.
- That nothing but the filter, layer 11 and the freeze reads targets 9,
  10 and 11.
- That the backend gives a colour-only pass's `record_encoded` a scratch
  depth of the format its packets were recorded with, and that a swallow
  the device aborts after accepting a private filter is rare: its
  replayed quads read a target 10 holding only the clear, and that
  generation draws no shadow.
- Pixel parity and timing of every slice.

## Related

[native-shadows.md](native-shadows.md), [native-lights.md](native-lights.md), [framescene.md](framescene.md), [native-gels.md](native-gels.md), [native-post-hud.md](native-post-hud.md), [shaders.md](shaders.md), [agent-api.md](../runtime/agent-api.md).
