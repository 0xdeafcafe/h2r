# Shader instances, templates and passes

Status: Built. Code: `crates/tags/src/shader.rs`. Tests: `crates/tags/src/shader_tests.rs`. Switch: none.

`crates/tags` decodes the three tag groups that make up a Halo 2 material:

- **`shad`** (shader instance) holds the parameters: bitmaps, colours, vectors
  and animated properties.
- **`stem`** (shader template) says which passes draw, per LOD and layer, and
  how the instance parameters bind to each pass's inputs.
- **`spas`** (shader pass) holds the implementations: texture bindings, render
  and sampler state, the pixel shader definition and the vertex shader
  reference.

`tags::shader` has the records and `tags::material` turns a `shad` into the
ordered passes, ready for the D3D/WGSL layer. The pixel shader definition and
the vertex microcode are translated by `crates/shaders` (see
[shaders.md](../graphics/shaders.md)).

```sh
h2 shader <map> '<shad tag name>' [--disc disc.iso]   # print one material
h2 shader --all <map|all> [--disc disc.iso]           # validate; exit 1 on failure
```

Neither command writes anything. No tag bytes, names or dumps from the disc
are stored in the repo, and the tests build invented tags in memory.

## Method and provenance

Every interpreted field comes from the **retail engine's draw-time setup**,
with the address of the instruction that reads it. The disc was used to fix
root sizes (the smallest in-tag child pointer of every tag) and to check
counts and strides. No outside layout was transcribed. The `spas` root
block at `+0x1c` is unverified, and the `D3DPIXELSHADERDEF` at implementation
`+6` was established in shaders.md.

Evidence labels: **E** means read by engine code at the address given. **D**
means a structural property checked on all 30 disc maps. **U** means the
field is not read by any decoded engine path; it is kept raw, and where it
is a block descriptor it is preserved but never dereferenced.

### The draw path

`0x0001bbf0` begins a shader draw: it checks that the tag is a `shad`
(`0x0001bcc9`), copies four root fields into engine inputs, then calls
`0x00016b90(state 0x485b48, shad, lod, layer, pass, implementation, size)`.
That function walks the compiled postprocess records:

```text
shad +0x20 -> postprocess P (0x7c)         P+0 = stem datum
stem +0x58 -> postprocess T (0x28)
  T.lods[lod]                 (stride 10)  range -> T.layers
  T.layers[start + layer]     (u16)        range -> T.passes
  T.passes[start + pass]      (stride 10)  spas ref, range -> T.implementations
  T.implementations[start+k]  (stride 6)   ranges -> T.bindings
spas +0x1c -> postprocess S (0x58)
  S.implementations[k]        (0x132)
```

Child ranges are packed into a `u16`: the first element in the low 9 bits and
the count in the high 7 (`and 0x1ff` / `shr 9` everywhere). `0x00016610`
walks every pass of a layer this way, skipping implementations that have no
vertex shader. Callers pass `lod` 0 or a computed level, and a layer index.
Examples are `0x0003c577` (layer 3) and `0x00016713`. `size` becomes the
mip-LOD term at state `+0x28/+0x2c`.

The per-draw order inside `0x00016b90` is the order the material API applies
them in:

1. `0x00016f60`: bitmap parameters go into 64 texture slots (state `+0x320`,
   0x40 bytes each).
2. `0x00017000`: instance vectors go into vertex attribute defaults and
   vertex constants.
3. When the implementation changes, the previous implementation's restore
   states run (`0x00018900` on `+0x11c`), then this one's render states
   (`+2`) and sampler states (`0x000189a0` on `+4`).
4. `0x0001ae70`: the engine inputs this implementation needs are computed.
5. The pixel shader definition is copied from implementation `+6`
   (`0x00016e81`, 0x3c dwords), or built from fragments by `0x0001ba00`.
6. `0x00017170`: instance colours are written into combiner constants.
7. `0x000174d0` then `0x00018a90`: animated slot fields, then the
   texture-stage bindings. For a slot binding, `0x00018afb` first copies
   the whole 0x40-byte slot to the stage. Only then, when the binding asks
   (`flags & 2`), does `0x00018b31` compare the slot's **raw** frame with
   the image count (skipping when frame >= count, before any wrapping), and
   `0x00018b90`/`0x00018ba2` write the image size back to the **slot**. The
   stage therefore carries the slot's earlier size, which is what engine
   dimension modes read (`0x0001b435`/`0x0001b43e`); a later binding of the
   same slot sees the refreshed one. The frame is wrapped by the image count
   only at bind time (`0x0001b0a0`).
8. `0x00019be0`, `0x00019ca0`, `0x0001a170`, `0x0001ab50`: engine-driven
   vertex constants, pixel constants, render states and sampler states.
9. `0x00017420`: animated pixel constants, vertex constants, render states
   and sampler states.
10. `0x00018d70` uploads bank 0 as `NV097_SET_VERTEX_DATA4F_M` (method
    `0x1a00 + 16i`). `0x00018e80` uploads bank 1 through
    `D3DDevice_SetVertexShaderConstant1Fast` at **hardware** `c[18 + i]`
    (method `0x1ea4` takes the index unbiased). `0x0001b0a0` binds the
    textures, `0x0001cf50` flushes them to `D3DDevice_SetTexture`, and
    `0x0001b050` calls `D3DDevice_SetPixelShader` with the patched copy.

## `shad` (root 0x54)

| Offset | Field | Evidence |
|---|---|---|
| `00` | `stem` reference | E `0x0001663a` (datum at +4); D resolves, and equals `P+0` |
| `0c` | block, always count 1 | U |
| `20` | compiled postprocess P, stride 0x7c | E `0x00016bc7`; D exactly 1 element |
| `2c` | block, up to 98 elements | U |
| `34` | reference (group bytes `slit`) | U |
| `3e`, `40` | u16 engine integer inputs 4 and 5 | E `0x0001bce1`, `0x0001bceb` → `0x485a64/68`, returned by `0x0003408f/97` |
| `44` | f32: engine scalar 21, and multiplier for vector inputs 23 and 35 | E `0x0001bcf5`, `0x000337e0`, `0x00033b11` |
| `48` | f32: source of vector input 24 | E `0x0001bd02`, `0x00033c63` |
| rest | raw | U |

Postprocess P (0x7c):

| Offset | Stride | Field | Evidence |
|---|---|---|---|
| `00` | | template datum | E `0x00016bde` path; D equals root reference |
| `04` | `0c` | bitmaps: `bitm` datum, i32 image (frame), f32 LOD offset | E `0x00016f60` (`+0`, `+4` → slot frame, `+8` → slot `+0x14`); `0x0001b0a0` wraps the frame by the bitmap's image count and adds the LOD offset to the draw's LOD term |
| `0c` | 4 | colours, packed ARGB | E `0x0001717b` |
| `14` | `10` | vectors, 4 × f32 | E `0x00017039` |
| `1c` | 6 | LODs: u32 layer mask, range → `24` | E `0x000174d9`..`0x000174f3` |
| `24` | 2 | layers: range → `2c` | E `0x00017507` |
| `2c` | 2 | passes: range → `34` | E `0x00017519` |
| `34` | `0a` | per implementation: five animation ranges (bitmaps, render states, sampler states, pixel constants, vertex constants) → `54` | E `0x00017532`, consumers `0x00017550`, `0x00017b60`, `0x00018560`, `0x000176a0`, `0x00017960` |
| `3c` | `14` | animated properties: u32 input (0 = time), u32 input A, u32 input B (f32 period when time-driven), data ref function | E `0x000175d9`, `0x0001b230` |
| `44` | 4 | property refs: i16 property, i16 component | E `0x000175d0`, `0x00017635` |
| `4c` | 2 | property ranges → `44` | E `0x0001759d` |
| `54` | 4 | animation bindings: target, b1, b2, property-range index | E `0x00017568` |
| `5c`, `64`, `6c` | | blocks with constant counts 5, 2 and 6, pointing at shared pool bytes | U, not dereferenced |
| `74` | | 8 bytes | U |

The `shad` LOD/layer/pass/implementation tree mirrors the template's. **D:**
the shapes match in every disc tag. The engine bounds-checks each level
(`0x000174d0`), so a missing level just means "not animated".

## `stem` (root 0x60)

| Offset | Field | Evidence |
|---|---|---|
| `40`, `4c` | `shad` references (usually null) | U; D resolve or are null |
| `58` | compiled postprocess T, stride 0x28 | E `0x00016bf4`; D exactly 1 element |
| rest | raw | U |

Postprocess T (0x28):

| Offset | Stride | Field | Evidence |
|---|---|---|---|
| `00` | `0a` | LODs: range → `08` (one entry per layer index), u32 layer mask (`+2`), u32 (`+6`) | E `0x00016bfd`, mask `0x0001666d`; mask also feeds integer input 6 (`0x000340a4`); `+6` U |
| `08` | 2 | layers: range → `10` | E `0x00016c0d` |
| `10` | `0a` | passes: `spas` reference, range → `18` | E `0x00016c2c`, `0x00016d66`; D the range count equals the pass's implementation count |
| `18` | 6 | implementation rows: bitmap, colour and vector ranges → `20` | E `0x00016d71`; passed to `0x00016f60` (`+0`), `0x00017170` (`+2`) and `0x00017000` (`+4`) |
| `20` | 4 | parameter bindings | E (below) |

Parameter bindings, by the range that selects them:

- **Bitmap** (`0x00016f60`): byte 0 is the texture slot (0-63), byte 3 is the
  instance bitmap. A null bitmap selects the rasterizer default
  (`[0x485a80] + 0x64`).
- **Vector** (`0x00017000`): bits 0-3 are the register and bit 4 the bank
  (0 = `vN` default, 1 = `c[18 + n]`). Bits 5-9 are the component mode.
  It goes through the byte map at `0x00017154` to copy routines at
  `0x00017130`, which give modes 0-20 = x, y, z, w, xyz, x, y, z, w, xy,
  zw, xy, zw, xyzw, xyw, xyw, xyz, xyz, then xyzw three times. Byte 3 is
  the instance vector.
- **Colour** (`0x00017170`): bits 6-9 are the register, where 8 selects the
  final-combiner constant. Bit 10 selects `c1`. Byte 2 bits 0-2 pick a lane
  mask from `0x467068` (R, G, B, A, RGB, R, G, B), and byte 3 is the
  instance colour. The value is merged into the copied pixel shader
  definition's `PSConstant0/1[n]` or `PSFinalCombinerConstant0/1`. For
  variant pixel shaders, bits 0-2 name a fragment and bits 3-5 a required
  selector value (skipped on mismatch), and the register is offset by the
  fragment's first combiner stage.

## `spas` (root 0x24)

| Offset | Field | Evidence |
|---|---|---|
| `1c` | compiled postprocess S, stride 0x58 | E `0x00016c3c`; D exactly 1 element |

Postprocess S (0x58):

| Offset | Stride | Field | Evidence |
|---|---|---|---|
| `00` | `132` | implementations | E `0x00016c26` |
| `08` | 4 | texture bindings: slot (`ff` = engine source), engine source (0 none, 11 lightmap, else `0x00019530` id), stage, flags (bit 1 = record image size) | E `0x00018a90` |
| `10` | 5 | render states: u8 Xbox `D3DRS`, u32 value | E `0x00018900` |
| `18` | `18` | sampler states, see below | E `0x000189a0` |
| `20` | 4 | combiner fragments: i16 selector (engine integer input, -1 fixed), u16 first variant | E `0x0001ba91`, `0x0001ba98` |
| `28` | 6 | variants: u16 (U), u8 flags (bit 0: last stage is the final combiner), u8 (U), range → `30` | E `0x0001bac1`, `0x0001baf0` |
| `30` | `20` | combiner stages: RGB in, RGB out, alpha in, alpha out, C0, C1 (+2 unread dwords) | E `0x0001bafd`..`0x0001bb23` |
| `38` | 4 | engine-driven bindings | E `0x00019be0`, `0x00019ca0`, `0x0001a170`, `0x0001ab50` |
| `40`, `48`, `50` | | always empty, and enforced | D |

Implementation (0x132):

| Offset | Field | Evidence |
|---|---|---|
| `000` | range: texture bindings | E `0x00018a97` |
| `002` | range: render states on entry | E `0x00016e2e` |
| `004` | range: sampler states (range position = stage) | E `0x00016e3e` |
| `006` | `D3DPIXELSHADERDEF` (0xf0) | E `0x00016e73` (copy) |
| `0f6` | range: combiner fragments; nonzero means a variant shader | E `0x00016e6a`, `0x0001ba3b`. D: exactly the 1,235 implementations whose embedded definition has zero general combiners |
| `0fc` | `vrtx` reference (fully null in 56 implementations) | E `0x00016718` |
| `104` | u64 vector engine-input mask | E `0x00016e16` (to state `+0x4c/+0x50`); `0x0001ae8f` walks it for the vector evaluator `0x00033980` (`0x0001af7d`) |
| `10c` | u64 scalar engine-input mask | E `0x00016e04` (to state `+0x44/+0x48`); `0x0001ae7b` walks it for the scalar evaluator `0x000336f0` (`0x0001aee6`) |
| `114` | u32 integer engine-input mask | E `0x00016e28` (to state `+0x54`) |
| `11c` | range: render states applied when the next implementation replaces this one | E `0x00016dee` |
| `11e` | range: engine render states | E `0x00016ef2` |
| `120` | range: engine sampler states | E `0x00016f01` |
| `122` | range: engine pixel constants | E `0x00016ee1` |
| `124` | range: engine vertex constants | E `0x00016ecf` |
| `0f8`, `118`, `126..132` | raw | U |

Sampler-state record (`0x000189a0`, stored to the deferred texture-state
array `0x404dd0 + 0x80·stage` in XDK order):

- byte 0 high nibble: MINFILTER
- byte 1: MAGFILTER in the low nibble, MIPFILTER in the high
- byte 2: ALPHAKILL low, COLORKEYOP high
- byte 3: MAXANISOTROPY low, MAXMIPLEVEL high
- bytes 4-6: ADDRESSU/V/W
- `+8`: BORDERCOLOR (`D3DDevice_SetTextureState_BorderColor`)
- `+0xc`: COLORKEYCOLOR
- `+0x10`: COLORSIGN
- `+0x14`: MIPMAPLODBIAS

Byte 0's low nibble and byte 7 are never read.

### Variant pixel shaders (`0x0001ba00`)

For each fragment k, the variant index is the fragment's first variant plus
`ints[selector]` (or plus nothing when the selector is -1). Each variant
stage fills the next general combiner slot: `PSRGBInputs`, `PSRGBOutputs`,
`PSAlphaInputs`, `PSAlphaOutputs`, `PSConstant0` and `PSConstant1`. When the
flag is set, the variant's last stage instead becomes the final combiner (ABCD,
EFG, both final constants). Without a final stage the engine writes ABCD `0xc`
and EFG `0x2000`. `PSCombinerCount` becomes `max(n, 1) | 0x11100`. The
texture modes, dot mapping and input texture come from the implementation's
own definition.

### Engine inputs

`0x0001b2c0` fills a register from an engine input:

- **Modes 0-3** write one component from scalar input `[source]`. The pixel
  path applies a transform: 1 = `1 − x`; 2-5 = ½(cos 2πx + 1), ½ − ½cos 2πx,
  ½(sin 2πx + 1) and ½ − ½sin 2πx, each clamped to 0..1.
- **Mode 4** writes xyz from vector input `[source]`.
- **Modes 5-20** write the width and height of texture stage `n − 1` in
  fixed patterns. `n` comes from bits 13-15 for vertex constants; with
  `n = 0` the width and height read as 1.

Pixel results get an optional `(x + 1) / 2` (byte 1 bit 3), are clamped and
packed as R = x, G = y, B = z, A = w, then go through the colour path.

Integer inputs: 4 and 5 come from the `shad` root, 0-3 from engine globals,
and 6-7 are flags. Scalar 21 comes from the `shad` root. Vector inputs 23,
24 and 35 use the object-function callback at `0x485a58`.

## Element budgets and aliasing

The cache deduplicates identical child data. **D:** of 134,919 nonempty
postprocess blocks, 60,293 point outside their own tag, and 16 share an
address with another block of the same tag. So decoding reads the whole
ordinary metadata region (`shader::Metadata`), never a single tag's
payload. Budgets:

- **1,024 elements per block** (`MAX_BLOCK_ELEMENTS`). Every block is
  reached through a 9+7-bit range (at most 638 reachable elements), a u8
  index or a small draw argument. A larger count is rejected, not truncated.
- **65,536 elements and 1 MiB of copied bytes per tag** in total
  (`MAX_TAG_ELEMENTS`, `MAX_TAG_BYTES`). One budget is shared by every
  block of the tag at every depth: all root postprocess elements, their
  children, and every animation function blob copy (aliased blobs are
  charged each time). On the disc no tag uses more than 562 elements or 5,493
  bytes.
- **4 KiB per animation function blob.**
- Blocks must lie entirely inside the metadata region and be 2-byte aligned.

Observed maxima across the disc:

- `shad` P: bitmaps 11, colours 16, vectors 29, LODs 4, layers 100,
  passes 28, implementations 89, properties 41, bindings 210.
- `stem` T: bindings 340.
- `spas` S: implementations 7, texture bindings 20, render states 44,
  fragments 31, variants 63, stages 78, engine bindings 28.

## Resolved material API

Valid ranges can select the same pass and implementation data many times,
so decoder budgets do not bound a resolved material. `material()` decodes
and validates each reached tag once, checks the summed pass and
implementation counts against `MAX_MATERIAL_PASSES` and
`MAX_MATERIAL_IMPLEMENTATIONS` (4,096 each) before expanding anything, and
charges every binding, state, combiner and animation record it reads to
`MAX_MATERIAL_WORK` (262,144). The disc's largest material has 28 passes,
89 implementations and 4,215 records.

`ShaderLibrary::read(&mut map)` loads the metadata and tag table, and
`library.material(shad_id, None)` returns a `Material`. That is a list of
`ResolvedLayer { lod, layer, passes }`. Each `ResolvedPass` lists every
implementation of its `spas`, because the caller picks one per draw. Each
`ResolvedImplementation` holds:

- the vertex shader (`vrtx`)
- per-stage textures (`TextureSource`, with the image wrapped by the bitmap's
  image count), the size the binding refreshes into its slot, and the size
  the stage carries (see below)
- sampler states per stage
- render states on entry, and restore states for when the next
  implementation replaces it
- the final 0xf0-byte pixel shader definition, with variant combiners built
  and constants patched, plus masks of the constants written
- vertex constants `c[18..34]` and vertex defaults `v[0..16]`, each tagged
  `Instance` or `Engine`
- the animated targets, which are listed but not evaluated
- the raw engine render-state and sampler-state bindings

`EngineInputs::for_shader` is the default input set. It is zero except for
the three inputs the root supplies. Pass your own inputs to re-resolve
variants and engine constants.

## Disc results

`h2 shader --all all` on the retail disc (all 30 maps, cache build
`02.09.27.09809`) finds 0 failures:

- `shad`: 11,495 tags, 2,331 unique names. `stem`: 2,581 tags, 165 unique
  names. `spas`: 2,688 tags, 147 unique names, 4,593 implementations
  (1,235 variants, 56 without a vertex shader).
- All 11,495 materials resolve into 86,497 (lod, layer) pass lists, 86,562
  passes and 307,944 implementations.
- The 2,931 distinct resolved pixel shader definitions translate through
  `crates/shaders` with no warnings and pass naga.
- Every reference resolves to the right group or is null. Every range fits
  its block. Every render state used has a name.
- 3,428 instances have animated parameters.
- Texture sources include instance bitmaps, the lightmap, 17 distinct engine
  sources and 28 null bitmap parameters (the default texture).

## Open items

- **Animated values are not evaluated.** Targets and property references are
  decoded. Evaluating them needs the mapping-function evaluator
  (`0x0013b390`, `0x0013bc00`) and the object-function callback.
- **Engine texture sources** (`0x00019530`: 1, 3, 5, 8-10, 13-20, 24, 26 and
  27 occur) are reported by id. Which render target or global texture each
  id means is not mapped.
- **Engine render-state and sampler-state bindings** (`+0x11e`, `+0x120`)
  are only fully decoded for byte 1 = 1, which reads a scalar input. They
  are returned raw.
- **Engine input semantics** (which scalar or vector is time, camera, light
  colour and so on) are not named, apart from the root-fed ones.
- **Persistent state.** The engine keeps texture slots, constant banks and the
  pixel shader buffer across draws. For a full rebind,
  `ShaderLibrary::selected_implementation` accepts an incoming
  `ParameterState` and preserves unwritten fields. The map-wide material
  inspector evaluates alternatives independently. It starts each implementation
  from zero and the implementation's own definition. A variant shader's
  untouched fields (compare mode, the `C0/C1Mapping` words) therefore come
  from the implementation's embedded definition, where the engine would
  keep the previous draw's buffer. Texture slot sizes start at zero for
  each implementation. Engine dimension modes read 0×0 when the stage's
  size is unknown (engine sources) or still zero.
- **Unread fields** (marked U) are preserved but have no meaning assigned.
  These are `shad +0x0c/+0x2c/+0x34`, `P +0x5c..+0x7c`, `stem +0x40/+0x4c`,
  LOD `+6`, the variant `u16`/`u8`, and implementation
  `+0xf8/+0x118/+0x126`.
- Deferred render-state names (92-135) follow the XDK order only. Names
  0-91 and 136-165 are checked against the engine's method and setter
  tables.
