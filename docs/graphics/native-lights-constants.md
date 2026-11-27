# Native lights: constants, engine inputs and the native look

Status: Partly built. The constants, flag and chooser ports are built and tested against the lifted functions; the native look draws cluster-part lights. Pixel parity is not verified.
Code: `crates/rewrites/engine/src/light_constants.rs`, `shadow_constants.rs`, `light_passes.rs`, `crates/rewrites/native-render/src/light_look.rs`, `light_list.rs`, `light_gpu.rs`, `crates/extensions/game/src/native_lights_glue.rs`, `native_lights_glue/materials.rs`. Tests: named per section.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

Part of [native-lights.md](native-lights.md), which has the light path, the frame under the cut and the scene groups. This page holds what the guest's setup functions store for a light and how a light pass is translated.

| Section | What it states |
|---|---|
| The light's constants | `0x0004a1e0`, `0x000497a0`, `0x00048ee0`: the blocks the passes' engine inputs read, and their ports |
| Lights over the native world | the per-layer path for cluster geometry, engine textures, implementations, engine constants, the look and its host pieces |

## The light's constants

| Piece | Rust | Guest function | Test |
|---|---|---|---|
| Light block, frame, screen bounds | `engine::light_constants::projected_light_globals` | `0x0004a1e0`, `0x000497a0`, `0x001650a0` | `engine/tests/light_constants_differential.rs`, `screen_bounds_match_the_lifted_original` in `engine/tests/light_gather_differential.rs` |
| Shadow projection | `engine::shadow_constants::shadow_globals` | `0x00048ee0` | `engine/tests/shadow_constants_differential.rs`, `shadow_constants.rs` |
| Pass flags and implementation choice | `engine::light_passes` (`pass_flags`) | `0x000311c8`..`0x0003128e`, `0x00030da0`, `0x00030e00` | `engine/tests/light_passes_differential.rs`, `light_passes.rs` |
| Publication | `light_gather::typed::plan`, `Globals::write` | | `engine/tests/light_constants.rs` |

`light_constants::projected_light_globals` computes the light block for
`0x0004a1e0`, including the frame from `0x000497a0` and the sphere's
screen bounds from `0x001650a0`. SSE runs under the guest's MXCSR and
the special frame's x87 arithmetic uses its control word. The functions
read their constants from the image.

An offscreen sphere takes the guest's whole-viewport fallback. A
projection fault refuses the typed pass before it writes memory.

`light_gather::typed::plan` reads the block, the site's flags, any
shadow projection and the five shader-input masks marked dirty by
`0x0001be50`. The shadow calculation consumes this light's frame rows,
falloff and pass flags before any of them reach guest RAM. A shadow
with a live cached texture keeps the original setup, whose
`0x0001cf50` unbinds the texture.

After whole-pass admission, `Globals::write` publishes the light block,
dirty masks, flags and projection in site order. Object shadows apply
their weight to the first falloff word after projection. The differential
tests compare seeded guest memory against the lifted setup functions;
`light_constants.rs` covers all three callers' flag and weight rules,
complete setup memory and refusal before publication.

- `0x0004a1e0` (`ret 0x10`, about 250 instructions): EAX the light
  record's `+0x84` (so `[EAX+0x50]` is the record's colour A at
  `+0xd4`, `+0x5c` colour B, `+0x68`..`+0x84` the record's words
  `+0xec`..`+0x108`), EDX the light datum, ECX the context (its word into `[0x004858c8]`), stack
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
  `0x0003a550`, which updates shader input masks, uploads the light
  frame and renders the gel for a valid shader reference. The device
  scissor is inherited through this call.
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
  dwords at `0x004c9798`. Code: `engine::shadow_constants::
  shadow_globals`. ECX the context, EBX the light's frame
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
  ported.
These globals are what the layer-12/13 templates' engine inputs read,
so the port's output is that block, compared word for word with the
game's in a test that runs on recorded inputs.

## Lights over the native world

The per-layer path draws the world's lit parts with their materials' layer-12 and 13 passes, shadowed light or not, with the light's list copied at the hook.

- The hook's candidate is the light's from its passes' entry to the next light. Its layers are taken one at a time, each when its own layer draw runs (`native_lights::enter_layer`); a layer the gathered walk batches leaves the candidate without a submission.
- No stencil links layers 12 and 13 to layers 10, 11 and 14. What they read of those is the shadow map and layer 11's destination alpha, which the queue order keeps. Each admitted packet must match the live render state (`PreparedLight::matches`). The light's gel checkpoint serves both layers.
- This is built for cluster geometry. `boot::native_lights_glue` checks the loaded map and BSP, resolves the sorted layer and reserves both physical copies before advancing the guest dispatch cursor. Placements, objects and shadow maps keep their D3D draws.
- A shader-driven gel requires a fresh target-13 binding from the original constants call. The consumer pins that read-only slot in its reservation, samples the matching physical copy and normalises linear texture coordinates by the guest gel width and height. A missing invocation, slot or copy keeps the whole light on D3D. [native-gels.md](native-gels.md) has the producer.

- **Engine textures** (`0x00019530`, a jump table at `0x00019b68`
  indexed by id - 1; Code): ids 1, 9 and 14 are bitmaps from the
  globals' rasterizer data (`[0x00485a80]`, set by `0x00022590` from
  the `matg` block at `+0x108`: it tests the count `[eax+0x108]` and
  keeps the address `[eax+0x10c]`, the first element, or zero), the
  datums at `+0xc`, `+4` and `+0x14` (each the datum word of a tag
  reference); S from how the light passes sample them, 1 is the
  vector normalisation cube map and 9 the distance attenuation volume.
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
  - Tests: `engine/tests/light_passes_differential.rs` runs the flag
    code from `0x000311c8` to either successor of `0x00031293` over all
    memory, with both choosers' answers and stack; `light_passes.rs`.
- **Engine constants.** The light's vertex constants are uploaded by
  `0x0003a550`, the last call of `0x0004a1e0`. It sends the frame
  block and the five words after it, 36 dwords from `0x0048597c`
  to `c[31]`..`c[39]` (`0x003f7330` takes ECX the first register in
  hardware numbering and the dword count on the stack: it writes the
  push-buffer methods `0x1ea4` and `0x0b80`). The shadow block goes to
  `c[40]`..`c[48]` the same way. On the per-layer path the original
  constants call runs before admission. The host checks its 36 frame words against
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
  `tags::material` reports every input an implementation reads
  (`ResolvedImplementation::input_reads`: its masks, its engine
  constant bindings' sources and its fragment selectors).
  `engine_inputs` fills the light's inputs over the instance's
  (`LightInputs`, from `LightGlobals`); the host resolves the material
  with them (`ShaderLibrary::material`) and `light_look` takes the
  chosen implementation of layer 12 or 13, refusing passes that read
  inputs nobody supplies, animated ones, engine-driven states from a
  source other than the instance's scalar 21
  (`engine_states::apply_static`, which puts the rest over the pass's
  own), and
  engine textures but 1 (`Normalization`, bound as a cube), 9
  (`Attenuation`, a volume) and 10 (`Gel`, 2D): a pass sampling the
  shadow maps (13, 8) keeps its D3D draws. `compile_light` translates
  it through the same code as the world's and transparents' passes
  (`program::translate_pass`), and `light_vertex` puts the light's
  frame constants into `c[31]`..`c[39]`.
- **Host pieces** (Code). The hook computes each light's constants as
  `0x00031170` would (`feature::native_lights::light_constants`: the
  definition from the tag table, the shadow word under scene flag bit
  10, `light_globals` without the screen bounds, `pass_flags`) into
  `Light::constants`. The BSP's material worker
  (`crates/extensions/game/src/native_lights_glue/materials.rs`) resolves
  each requested shader, LOD, layer, pass, implementation, vertex class and set of light inputs,
  translates its program and uploads its stage textures. The map worker
  uploads the normalisation cube, attenuation volume and static gels.
  The guest checks the resident pipeline for the live target formats,
  cull and vertex layout before admission. D3D handles the light until
  the first swap after its pipeline completes. `boot::native_lights_glue`
  reads the light textures' bitmaps from the `matg +0x108` block
  (`light_bitmaps`, `matg_light_bitmaps` over the ordinary metadata,
  `light_textures`; `layered` takes a cube's six level-0 faces and a
  volume's one level 0 of face 0, as `assets::decode_image` gives
  them) and says which lights are drawable (constants, no shadow).
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
  `native_render::light_list::SortedCall` checks the built dispatches,
  including its list interval and layer. The host decodes the
  standard handlers' entries with the scene group's reader and resolves
  them against the loaded parts and materials. A missing stream,
  texture, stale stage palette or unsupplied constant leaves the
  handler in place. The GPU uses the shared BSP program vertices or
  the model section buffer for the selected shader and class. It
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
    the faces and level-0 sizes it has). The BSP summary reports resident
    lit parts and parts whose materials could not be read. The host
    counts requests to prepare a light or layer. A light can make more
    than one request and a declined resident plan can still draw through
    the native indexed path. The summary aggregates these decisions by
    reason, with up to 32 distinct shader refusal details.
    Frame and agent captures include
    `native_lights_worker_round_trips`, `native_lights_reference_passes`
    and `native_lights_display_passes`. Every 1800 host decisions it
    also prints counts by reason.
  - Engine textures' texels (Code, V on 03a's `tile_metal_dark`): the
    light passes' samplers for the normalisation cube and the
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
