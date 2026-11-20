# Native renderer stage 4: lights and shadows

Status: Partly built. Unshadowed dynamic lights over cluster geometry, the gathered light and shadow geometry walk and the shadow replacements are built; the visible-light gather, object-model lights, the first-person lights and parity with D3D are not.
Code: `crates/feature/src/native_lights.rs`, `native_lights/walk.rs`, `crates/boot/src/native_lights_glue.rs`, `native_lights_glue/walk.rs`, `crates/native-render/src/lights.rs`, `light_look.rs`, `light_gpu.rs`, `light_list.rs`. Tests: `crates/feature/tests/native_lights.rs`, `native_lights_split.rs`, `crates/engine/tests/light_passes.rs`, `light_constants.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

Part of [native-renderer.md](native-renderer.md). The dynamic lights are not layers of the scene list: after layer 7, `render_scene_passes` calls `render_dynamic_lights` (`0x00031c80`, `points::RENDER_DYNAMIC_LIGHTS`), which gathers each visible light's geometry into a list of its own and draws layers 10 to 14 over it, one light at a time. The game has two more shadow paths beside it: object shadows before layer 7, and the first-person view's lights after the scene. The pages for the other pieces are [native-shadows.md](native-shadows.md) (layers 10, 11 and 14) and [native-gels.md](native-gels.md) (target 13).

## Stage 4: lights and shadows

The light path in the order the game draws it ([e3-lighting.md](e3-lighting.md), "The light render path", has the light records and the definition flags).

### The path (Code, `h2 disasm`)

- **Visible lights.** `0x00031c80` builds the visible-light list
  (`0x00031910`, from the candidates at `[0x00547f98]`; entries of `0x14`
  bytes with light datum `+4`, root owner `+8`, weight `+0xc` and
  screen priority `+0x10`). The original `0x0004a7c0`/`0x0004ac30`
  helpers merge and rank that list before the pass walks it. For each
  light above the weight threshold (`[0x0045dc58]`) it re-poses an attached light
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

### Gathered light and shadow geometry

At `RENDER_LIGHT_PASSES` the game has chosen the light, receivers and
casters. It still computes the projection and scissor and produces the
gel. Immediately before `LAYER_BUILD` runs, Rust reads the active
interval of the appropriate gathered list. Layers 11–14 use receivers
and layer 10 uses casters. The caller identifies the dynamic light, an
additional caster, or an object shadow. Each candidate must have the
standard geometry build/draw pair. Rust resolves the live material
implementation, render section, part and vertex program classification.
It preserves the scratch bytes the callbacks leave alone, rejects the
same unusable programs, and sorts the surviving 24-byte entries with
the original tie order. A failed read or unknown callback leaves the
guest builder at its entry with its original output buffer.

A dynamic light can skip handlers when only one reachable layer
has active geometry. Empty intervals still run their target setup and clears. An object shadow can do so separately for generation
and application. All sorted entries must share one record, shader,
selector, implementation, vertex class, topology and per-part state.
The first `LIGHT_GEOMETRY_DRAW` runs to its complete-part indexed
emission. That supplies the live streams, pose, constants, texture
bindings (including the gel), scissor and depth/stencil state. Each
remaining part supplies its own index span and vertex window. The GPU
records each range in sorted order, including duplicates. Shadow draws
also go through the existing gate for their site, projection, caster
generation and sampled map.

The device verifies the indexed call and loop counter and records every
required target copy before shortening the loop to one. The guest then
finishes the first handler and exits the loop normally. A missing
resource, different binding, target feedback, incomplete copy or failed
cursor write leaves the original draw and every remaining handler in
place. Multi-layer dynamic lights and additional-caster chains can use
the Rust builder and retain their original draw dispatch. Clears, map
freezes, object shadow filters, gel updates and target restores run at
their original points. The live-binding requirement still costs one
guest handler even for a successful group.

Frame stats count completed dynamic lights in `lights_walked_native`,
`lights_walked_fallback` and `lights_walk_empty`. Each fallback adds
one `lights_walk_fallback_*` reason (graph, memory, callback, material,
geometry or GPU). `light_walk_layer_builds` counts replaced builds.
`light_walk_handlers_skipped` counts the remaining handlers actually
bypassed. `shadow_walk_layers` counts committed shadow layers, including
object shadows outside a dynamic light.

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

- **Shadows.** Layer 10's casters draw into a depth texture of our own from the light's projection (`0x00048ee0`, ported as a pure function), as [native-shadows.md](native-shadows.md) describes. Applying the shadow term inside the light's own pass (`GREATER`, one tap as the D3D path's), instead of layer 11's stencil and alpha marks, is not built; lights whose shadow cannot be drawn keep their D3D passes.

### The light's constants

`engine::light_constants::light_globals` returns the whole `0x0004a1e0` block (with `0x000497a0`'s frame) reading guest memory and writing none, and `LightGlobals::write` stores it. `engine::shadow_constants::shadow_globals` does the same for `0x00048ee0`. The arithmetic is the original's instruction for instruction, SSE through the `simd` crate under the caller's MXCSR and the special frame's x87 block through `x87::abi` under its control word, so the words are bit-equal (NaNs, rounding modes and precisions included); the image's constants are read at their addresses. `0x001650a0` (the sphere's screen bounds) is the caller's to answer, and `0x0003a550` (the scissor) the caller's to run after. `engine/tests/light_constants_differential.rs` runs the original, lifted, on the IR interpreter from seeded states and compares all memory; `engine/tests/light_constants.rs` checks invented lights. `engine/tests/shadow_constants_differential.rs` and `shadow_constants.rs` do the same for the shadow block. What the trace found:
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

### Unshadowed lights over the native world

The host takes lights without the shadow bit, with the light's list copied at the hook, and draws the world's lit parts with their materials' layer-12 and 13 passes. This is implemented for cluster geometry. `boot::native_lights_glue` checks the loaded map and BSP, resolves the sorted layer and reserves both physical copies before advancing the guest dispatch cursor. Placements, objects and shadow maps keep their D3D draws. A shader-driven gel requires a fresh target-13 binding from the original constants call. The consumer pins that read-only slot in its reservation, samples the matching physical copy and normalizes linear texture coordinates by the guest gel width and height. A missing invocation, slot or copy keeps the whole light on D3D. See [native-gels.md](native-gels.md) for the producer.

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
  `c[40]`..`c[48]` the same way. The original constants call runs
  before admission. The host checks its 36 frame words against
  `LightGlobals::frame_constants` before taking any dispatches. The
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
    actual synchronous worker round trips. Frame and agent captures include
    `native_lights_worker_round_trips`, `native_lights_reference_passes`
    and `native_lights_display_passes`. Every 1800 host decisions it
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

## Rules and limits

- Only cluster geometry is drawn natively. Models and rigid placements keep their D3D handlers. A split may consume a contiguous native prefix; the remaining entries run in guest order.
- A light that samples a shadow map (engine textures 13 and 8), an animated input, an engine-driven state or an unsupplied constant keeps its D3D passes (`light_look`).
- The visible-light gather at `0x00031c80`, the object-model lights, the object-shadow call `0x00032350`'s lights and the first-person lights (`0x000404b0`) run in the guest. The first-person records at `0x004c6b00` would need writing before the gather can be skipped.
- Diagnostics print as `native-lights:` on stderr: at load, each light texture with its size or why it did not load, and the surfaces left to D3D; at run time, each refusal reason on its first occurrence, and counts by reason every 1800 host decisions.

## Not verified

- Pixel parity at equal presents, with render scale 1 and 2, with the lights stage on and off (`impl.render.native=off`). Captures from a run with zero native submissions establish no light coverage, and a scene with zero shader-gel admissions says nothing about the target-13 binding.
- That reservations are not dropped and the invariant counters stay zero, in `native_lights_v*_reference_ranges` and the other `lights` stage counters of `H2_FRAME_STATS`.
- The time of the light site after parity is established.

## Related

[native-renderer.md](native-renderer.md), [native-shadows.md](native-shadows.md), [native-gels.md](native-gels.md), [e3-lighting.md](e3-lighting.md), [layer-draw.md](layer-draw.md).
