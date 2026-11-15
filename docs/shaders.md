# Shaders

`crates/shaders` turns Halo 2's Xbox shader programs into WGSL. We don't
emulate the NV2A. D3D8 is replaced at the API level, but Halo 2's shaders are
written for NV2A features (vertex microcode, register combiners, texture-stage
"shader modes"), so they still have to be translated.

- `shaders::vsh`: NV2A vertex program microcode, giving a decoder,
  disassembler, reference interpreter and WGSL `@vertex` translator.
- `shaders::psh`: `D3DPIXELSHADERDEF` (texture modes plus register
  combiners), giving a decoder, listing, reference interpreter and WGSL
  `@fragment` translator.
- `shaders::link` joins the two into one module with `vs_main` and `fs_main`.
  `shaders::validate` runs naga over any WGSL.

## What Halo 2 stores

Everything here was checked against the retail disc (cache build
`02.09.27.09809`, all 30 maps). `h2 extract-shaders` (`crates/cli/src/extract_shaders.rs`) reproduces it,
reading the maps with `crates/map`. It
reads your own disc image at run time and writes the corpus, a manifest
and a provenance record to a directory you choose (keep it in `$TMPDIR`; no
game data belongs in the repo). Every count in this section is a line of its
`provenance.txt`. Across the maps there are 353 distinct vertex programs and
149 distinct pixel shader definitions. All of them decode, translate and pass
naga validation, and none of them produce a translation warning.

### `vrtx` (vertex_shader)

The layout follows the Assembly plugin `Halo2Xbox/vrtx.xml`, confirmed on the
data:

```text
root (0xC):  enum16 platform (1 = Xbox in all 2,261 tags) | int16 | block geometry classifications
classification (0x1C): u32 | block<u16> | dataref compiled shader | dataref code
```

- **compiled shader** is the `XGAssembleShader` output: the bytes `'x' 0x20`,
  a u16 instruction count, then 16 bytes per instruction. Every retail
  program has kind `0x20` (a normal shader). None are read/write (`'xw'`) or
  state (`'xs'`) programs.
- **code** is empty in every classification (all 11,837). Retail maps carry no D3D
  `vs.1.1` token streams or source, so the crate only takes microcode.
  Nothing on the disc needs a D3D token parser.
- The `u16` block is a short list of small integers, such as `0,1,2,3,4,5,6`.
  It is **not** the set of `v` registers the program reads: of the 10,712
  classifications with a non-empty block, it matches in 1,985 and differs in
  8,727. It is probably vertex-declaration related, and its meaning is still
  unconfirmed.
- 765 classifications are empty; the other 11,072 all hold `'x' 0x20`
  programs. Some data pointers land in neighbouring tags because the cache
  deduplicates identical blocks, so the extractor resolves pointers against
  the whole metadata region rather than one tag.

### `spas` (shader_pass): the pixel shader lives here

There are **no `pixl` tags in the Xbox maps.** The combiner program sits
inside each shader pass:

```text
spas root + 0x1C: block postprocess definitions (0x58 each)
  + 0x00: block implementations (0x132 each)
            +0x00..0x06  three int16 block-index fields
            +0x06..0xF6  D3DPIXELSHADERDEF, verbatim, 2-byte aligned
            +0xF6..      fragment/permutation/combiner indices, vertex shader tag ref (+0xFC), ...
```

Assembly marks `+0x8..0xF4` as unknown. To locate the struct we looked for
fields that only make sense at one offset. Taken from `+6`, `PSCombinerCount`
reads as the classic `0x00011101` (and `0x000111nn` across the corpus).
`PSTextureModes` gives meaningful stage chains, for example lightmap passes
decode as 2D, 2D, 2D, cube. RGB input/output pairs then form real programs,
such as `desaturate` being `dot(t0, c0)`. At any 4-byte-aligned offset the
same bytes look halfword-swapped.

The extractor checks this mechanically on all 4,593 implementations (2,688
`spas` tags). At every even offset from 0 to 0x42 it parses a PSDEF and
scores it against invariants of the encoding (combiner count at most 8 with
only the documented flag bits, texture modes at most 0x12, dot mappings at
most 7, input-texture selectors inside their fields, compare modes in 16
bits, active stage outputs inside 20 bits, not all zero), then translates the
passing ones. It refuses to write a corpus unless `+6`, and only `+6`, passes
for every implementation:

| offset | pass invariants | translate cleanly | ...with combiner stages |
|---|---|---|---|
| **0x6** | **4,593** | **4,593** | **3,358** (all that have any) |
| 0xE | 4,467 | 4,467 | 60 |
| 0x12 | 4,274 | 4,274 | 0 |
| 0xC | 4,003 | 4,003 | 757 |
| 0xA | 1,095 | 1,095 | 941 |
| others | at most 2,905 | | at most 484 |

The invariants alone are weak: a misaligned window often reads a combiner
count of 0, which leaves nothing to contradict. The last column is the
telling one. Only at `+6` do the 3,358 implementations with general
combiners decode as combiner programs. The other 1,235 have a count of 0
(texture stages only). Other observations:

- 140 of 149 definitions use `MUX_MSB | UNIQUE_C0 | UNIQUE_C1`. The other 9
  have no general combiners at all.
- `PSC0Mapping`, `PSC1Mapping` and `PSFinalCombinerConstants` are always 0.
  Halo 2 doesn't route constants through D3D's mapping, and never sets
  `PS_GLOBALFLAGS_TEXMODE_ADJUST`. 38 definitions carry embedded `c0`
  values.
- Texture modes in use: none, project2d, project3d, cubemap, passthru,
  bumpenvmap, dotproduct, dot_st, dot_rflct_diff and dot_rflct_spec. Every
  dot-product stage uses `MINUS1_TO_1_D3D`.

To reproduce the corpus checks:

```sh
cargo run --release -p cli -- extract-shaders --out $TMPDIR/h2-shaders     # disc: $H2_DISC or disc/*.iso, or --disc
cargo run -p shaders --example corpus -- $TMPDIR/h2-shaders                 # stats + validation
cargo run -p shaders --example corpus -- $TMPDIR/h2-shaders --dump <file>   # disassembly + WGSL
H2_SHADER_CORPUS=$TMPDIR/h2-shaders cargo test -p shaders --test corpus     # also GPU vs interpreter
H2_SHADER_CORPUS=$TMPDIR/h2-shaders cargo test -p d3d --test shader_corpus -- --ignored   # every shader drawn through GfxBackend
```

Each blob is the bytes of a `vrtx` classification's compiled shader
(`*.xvu`), or the 0xF0 bytes at `+6` of a `spas` implementation
(`*.psdef`), named by the first 16 hex digits of its SHA-256.
`manifest.tsv` lists every file with its full hash, how many times it occurs
and where it was first seen. `provenance.txt` records the image size,
`default.xbe`'s SHA-256, each map's path, size, build and the SHA-256 of
the bytes read from it (header, tag index and metadata), every count above,
the offset table, and the manifest's own hash. Our run gave `default.xbe`
`03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d` and
manifest `961edf58b37cd0e7204f88dadc56120433d5a7ddf052b9e48a0bfc69336a4d91`.
The corpus test checks the directory against the manifest and fails if it
is empty, missing files or holds extras.

## Vertex microcode

Each instruction drives two units at once. The **MAC** is a 4-wide
multiply/accumulate unit, and the **ILU** is the inverse/log unit, which is
scalar except for `mov` and `lit`. Both read the same three operands A, B and
C. The ILU only reads C. Word 0 is unused.

| field | word | low bit | width |
|---|---|---|---|
| ILU op | 1 | 25 | 3 |
| MAC op | 1 | 21 | 4 |
| constant index | 1 | 13 | 8 |
| input (`v`) index | 1 | 9 | 4 |
| A: neg, swizzle x/y/z/w | 1 | 8; 6/4/2/0 | 1; 2 each |
| A: temp, mux | 2 | 28, 26 | 4, 2 |
| B: neg, swizzle | 2 | 25; 23/21/19/17 | |
| B: temp, mux | 2 | 13, 11 | 4, 2 |
| C: neg, swizzle | 2 | 10; 8/6/4/2 | |
| C: temp | w2 bits 0-1 high, w3 bits 30-31 low | | 4 |
| C: mux | 3 | 28 | 2 |
| MAC mask, temp, ILU mask, output mask | 3 | 24, 20, 16, 12 | 4 each |
| output is register (1) or constant (0) | 3 | 11 | 1 |
| output address | 3 | 3 | 8 |
| output from ILU (1) or MAC (0) | 3 | 2 | 1 |
| constant is `c[a0.x + i]` | 3 | 1 | 1 |
| final | 3 | 0 | 1 |

- Mux 1 is `r`, 2 is `v` and 3 is `c`. Masks put x in the top bit. Swizzle
  values 0 to 3 mean x, y, z, w.
- MAC ops are `nop mov mul add mad dp3 dph dp4 dst min max slt sge arl`
  (0 to 13). ILU ops are `nop mov rcp rcc rsq exp log lit` (0 to 7).
- Output addresses are 0 for `oPos`, 3 and 4 for `oD0` and `oD1`, 5 for
  `oFog`, 6 for `oPts`, 7 and 8 for `oB0` and `oB1`, and 9 to 12 for
  `oT0` to `oT3`.
- Constants use hardware numbering 0 to 191. D3D's `c-96` to `c95` map to
  `c[i + 96]`, so D3D's `c-38` is `c[58]`. The disassembler prints hardware
  numbers.

The semantics are defined by `vsh::interp`, and the generated WGSL uses the
same formulas:

- **Reads happen before writes.** Each instruction evaluates its operands and
  then writes, so a paired MAC and ILU, or an op that writes a register it
  reads, both see the old state.
- **Pairing.** When both units are active, the ILU's temporary write always
  goes to `r1`, and a MAC temporary write to `r1` is dropped.
- `add` is **A + C**. `mad` is A·B + C. Scalar ILU ops (`rcp rcc rsq exp
  log`) read the first selected component of C.
- **Multiplying by zero gives zero**, even for inf or NaN, in `mul`, `mad`,
  dots and `dst`.
- `rcc` clamps 1/x to ±[2^-64, 2^64], keeping the sign: +0 gives +2^64 and
  −0 gives −2^64. The saturation is decided on |x| before dividing. On Metal
  (which compiles with fast math) clamping `1/0 = inf` afterwards returned
  −5.42e-20 for RCC(+0). `rsq` is 1/√|x|.
- `exp` returns (2^⌊x⌋, x−⌊x⌋, 2^x, 1), with y = 0 for ±inf
  (NV_vertex_program): EXP(+inf) = (inf, 0, inf, 1) and EXP(−inf) = (0, 0,
  0, 1).
- `log` of |x| returns (exponent, mantissa, log2|x|, 1). The exponent and
  mantissa come from the IEEE fields, not from ⌊log2⌋, which rounds up just
  below a power of two: bits `0x037fffff` give −121 and 1.99999988, while
  log2 rounds to −120. Zero, and denormals (flushed, as on the NV2A), give
  (−inf, 1, −inf, 1). ±inf gives (inf, 1, inf, 1).
- The generated WGSL recognises infinities and signs by their bits, for the
  same fast-math reason. `lit` clamps the power to ±(128 − 1/256).
- `arl` sets `a0.x = floor(x + 0.001)`. The bias absorbs host attribute
  conversion landing just under an integer, and xemu uses the same one.
  `a0.x` is held to ±1024, and NaN gives 0. Any value beyond that range
  already saturates the clamp, so the result doesn't change, but `a0 + i`
  can't overflow. Relative constant reads clamp the index to 0..191.
- **Decoding** rejects instructions that read an absolute constant above
  `c[191]` (`Error::BadConst`), a temporary above `r12` or an operand with
  no register file. Only operands the active units use are checked or read:
  an unused B field naming `r15` is ignored.
- **`r12` is `oPos`**, read back mid-program. This is how the standard
  epilogue works (`mul oPos.xyz, r12, c-38 : rcc r1.x, r12.w` then
  `mad oPos.xyz, r12, r1.x, c-37`). Halo 2 uses its own constants for this
  (`c[10]` and `c[11]`, for example) instead of `c-38`/`c-37`.
- **`oFog` takes the result's x** whenever any component is masked. Halo 2
  writes `oFog.xw` and `oFog.w` from replicated scalars.
- Outputs start at (0, 0, 0, 1). Colours are clamped to [0, 1] when they
  leave the vertex stage.
- Writes to constants (from `'xw'` programs) go to a per-vertex copy. They
  don't persist across vertices the way they do on the hardware. Halo 2
  doesn't use this.

### Position fixup

NV2A vertex programs output **window coordinates**: x and y in pixels of the
render target, z in [0, depth full scale], and w for perspective. The Xbox
D3D runtime puts the viewport scale and offset into constants, and the
program applies them itself (the epilogue above). Before writing
`@builtin(position)`, the generated code maps window coordinates back to
wgpu clip space:

```text
x, y  ← trunc(x·16)/16, trunc(y·16)/16      (rasterizer keeps 4 fractional bits, truncates)
w     ← clamp_away(w)                       (as rcc: never 0 or inf)
ndc   = (2x/W − 1,  1 − 2y/H,  z/Zmax)      (wgpu y is up; NV2A window y is down)
clip  = (ndc·w, w)                          (the GPU's divide by w restores the window position)
```

`W` and `H` are the guest render-target size in the units the game's
viewport constants use, so they include any supersampling scale the game
folded in. `Zmax` is 65535 for D16, 16777215 for D24, or 1 for float depth.
They come from `VertexUniforms::{surface, z_max}`. Window coordinates map
one-to-one onto wgpu framebuffer coordinates, pixel centres included, so any
half-pixel offsets the game puts in its offset constant are preserved.
Rendering at a higher host resolution only changes the wgpu viewport.
Because window-space y is preserved, triangle winding in the framebuffer
matches the guest. Front-face and cull state can be taken from the guest
unchanged. The `render` test checks that guest screen row 0 is the top row.

## Pixel side: `D3DPIXELSHADERDEF`

The struct is 60 dwords in SDK order. `PSAlphaInputs[8]`,
`PSFinalCombinerInputsABCD`/`EFG`, `PSConstant0[8]`, `PSConstant1[8]`,
`PSAlphaOutputs[8]`, `PSRGBInputs[8]`, `PSCompareMode`,
`PSFinalCombinerConstant0`/`1`, `PSRGBOutputs[8]`, `PSCombinerCount`,
`PSTextureModes`, `PSDotMapping`, `PSInputTexture`, `PSC0Mapping`,
`PSC1Mapping`, `PSFinalCombinerConstants`. The order mirrors the NV2A
register file. `psh::bits` has the `PS_*` encodings.

- **Inputs.** A combiner reads one byte per input, with A in the top byte.
  The low 4 bits pick the register: zero, c0, c1, fog, v0, v1, t0 to t3, r0,
  r1, and, in the final combiner only, `v1+r0` and `E·F`. Bit 4 selects
  alpha. In the RGB portion that's `.aaa`. In the alpha portion, clear means
  blue and set means alpha. Bits 5 to 7 are the mapping: unsigned identity
  `max(0,x)`, invert `1−clamp(x)`, expand `2max(0,x)−1`, expand-negate,
  half-bias `max(0,x)−½`, half-bias-negate, signed `x` and signed-negate
  `−x`.
- **Outputs.** CD is bits 0 to 3, AB bits 4 to 7 and the sum bits 8 to 11.
  The flags are: bit 12 CD dot, bit 13 AB dot, bit 14 mux instead of sum,
  bits 15 to 17 output mapping (identity, −½, ×2, (x−½)×2, ×4, ÷2), bit 18
  CD blue-to-alpha and bit 19 AB blue-to-alpha. The mapping applies to each
  product and to the sum of the unmapped products. Every result is clamped
  to [−1, 1].
- **Order** follows xemu. All RGB and alpha inputs of a stage are read
  before any write. The RGB writes go first, AB then CD then sum, each
  followed by its blue-to-alpha copy, and the alpha writes come after.
- **Mux** picks CD when `r0.a >= 0.5` under `MUX_MSB`, and otherwise when
  the low bit of `u32(r0.a·255)` is set.
- **Constants.** Stage *n* reads `c0[n]` with `UNIQUE_C0` and `c0[0]`
  without it, and the same for c1. The final combiner reads index 8.
  `r0.a` starts as t0's alpha, or 1 when stage 0 has no texture. All other
  registers start at zero, and textures in mode NONE read (0, 0, 0, 1).
- **Final combiner.** When either inputs dword is non-zero, rgb =
  A·B + (1−A)·C + D and alpha = G, both clamped to [0, 1]. Without a final
  combiner the output is `clamp(r0)`.
- **`v1+r0` (V1R0_SUM)** unsigned-maps each operand before adding: each is
  clamped to [0, 1], and inverted (1 − clamp) when its complement flag is
  set. The clamp-sum flag then clamps the sum. So r0 = −0.5 and v1 = 0.75
  give 0.75, not 0.25. This is the `NV_register_combiners` definition of
  the final combiner's spare0-plus-secondary-colour input. The
  `SpecularR0SumFlags` hardware test in abaire's nxdk_pgraph_tests
  exercises the same behaviour (see Sources): negative r0 adds nothing,
  inversion applies to the clamped operand, and the sum is only clamped with
  the flag (0.5·(1 + 1) reads 1.0 unclamped and 0.5 clamped). Both were read
  for the behaviour only, and nothing was transcribed.
- **Fog** reads (fog colour rgb, factor). The factor is the interpolated
  `oFog.x`, or a linear, exp or exp2 table mode from `XpsUniforms.fog`.
  Applying fog is up to the shader's final combiner, as on the Xbox.

### Texture modes

Stages run in order 0 to 3, and dependent modes read an earlier stage's
result: stage 1 reads t0, and stages 2 and 3 read `PSInputTexture`. A
selector that doesn't name an earlier stage (stage 2: t0 to t1, stage 3: t0
to t2) reads zero, with a warning. Only bump, dependent and dot modes read
the selector, and only dot modes read the dot mapping, so unused fields can
hold anything.

| mode | status |
|---|---|
| none, project2d (÷q), project3d (÷q), cubemap, passthru (clamped coords) | supported |
| clipplane (kill per coordinate against `PSCompareMode`) | supported |
| bumpenvmap, bumpenvmap_lum (uv + M·(du,dv); ×clamp(lum·scale + offset)) | supported; see below |
| dotproduct, dot_st, dot_str_3d, dot_str_cube | supported |
| dot_rflct_diff, dot_rflct_spec (eye = q of t1 to t3) | supported |
| dot_rflct_spec_const | supported; eye from `XpsUniforms.eye` (the source register is unconfirmed) |
| dpndnt_ar, dpndnt_gb | supported |
| **dot_zw** | **unsupported.** The dot product is computed but depth is not replaced (no `frag_depth` write) |
| **brdf** | **unsupported.** Reads zero (xemu doesn't implement it either) |
| dot mappings zero_to_one, minus1_to_1_d3d/_gl/(nv) | supported, for unsigned 8-bit data |
| **dot mappings hilo_*** | **unsupported.** Treated as zero_to_one with a warning; HILO formats need the renderer's cooperation |

Halo 2 uses none of the unsupported entries. A stage that holds a mode it
can't (dot_st in stage 0, for example) is skipped with a warning.
`PixelShader::warnings` lists everything that was approximated.

Texture dimensions follow the mode. `PixelOptions::texture_dims` overrides
project2d, project3d and cubemap the way D3D's `TEXMODE_ADJUST` would, but
Halo 2 never sets that flag. For bump mapping the generated code reads du/dv
as `.rg` and luminance as `.b` of the source stage, already signed, so the
renderer should decode V8U8/L6V5U5 to match. 2D coordinates are multiplied by
`XpsUniforms.tex_scale[n].xy`, which covers linear (texel-addressed)
textures.

The other per-pipeline state the combiners need comes in through
`PixelOptions`: `D3DTSS_ALPHAKILL` per stage (kill when the fetched alpha is
0), the alpha test (wgpu has none, so it's emulated with `discard` against
`alpha_ref` in 0 to 255), two-sided lighting (back faces read `oB0`/`oB1`),
and flat shading.

## Generated code and the resource contract

| group | binding | contents |
|---|---|---|
| 0 | 0 | `XvsUniforms`: `c[192]`, `defaults[16]`, `viewport` (vertex) |
| 0 | 1 | `XpsUniforms`: `c0[9]`, `c1[9]`, fog, bump-env, `tex_scale`, eye, alpha ref (fragment) |
| 1 | 2n | stage n texture (`texture_2d`, `texture_3d` or `texture_cube`, per `PixelShader::textures`) |
| 1 | 2n+1 | stage n sampler |

`vsh::VertexUniforms::to_bytes` and `psh::PixelUniforms::to_bytes` produce
the buffer contents. Vertex register `vN` is `@location(N)`.
`VertexOptions::provided_inputs` names the registers the vertex layout
supplies. Any other register the program reads comes from
`XvsUniforms.defaults[N]` (the `SetVertexData4f` value). The varyings are
locations 0 to 3 for oD0, oD1, oB0 and oB1, 4 to 7 for oT0 to oT3, and 8 for
the fog factor.

Each instruction becomes a commented block with its disassembly and raw
words. The program sits in plain functions, `xvs_program(v…) -> XvsRegs`
and `xps_program(XpsInputs) -> XpsResult`, which the entry points wrap. The
tests call those functions from compute shaders. Texture sampling happens
before any `discard`, so it stays in uniform control flow.

### Plugging into `gfx`

An adapter implementing gfx's `ShaderProgram` needs very little:

1. Translate once per (vertex program, pixel definition, `VertexOptions`,
   `PixelOptions`) and use a hash of all four as the `ShaderId`, since the
   options change the code. `shaders::link` gives the module source, with
   entries `vs_main` and `fs_main`.
2. Build the pipeline layout from the table above. Group 0 has a vertex
   uniform at binding 0 (`VertexUniforms::SIZE`) and a fragment uniform at
   binding 1. Group 1 has a texture and sampler for each stage where
   `PixelShader::textures[n]` is `Some`, with the view dimension taken from
   it.
3. For each draw, fill `VertexUniforms` from the guest's vertex constants
   (hardware order), render-target size and depth format. Fill
   `PixelUniforms` from the combiner constants, fog, bump-env stage state and
   alpha ref. Pass both groups in `Draw::bind_groups`.
4. The vertex layout must declare a `Float32xN` (or normalised) attribute at
   location `N` for each supplied register `vN`, and set `provided_inputs` to
   match. wgpu fills missing components with (0, 0, 0, 1), which is the Xbox
   default. D3DCOLOR attributes need BGRA unpacking.

## Testing

- `tests/vsh.rs` and `tests/psh.rs` cover instruction words assembled by
  hand from the field table (independent of the encoder), encoder round
  trips, error paths and golden disassembly. They check the interpreters
  against hand-worked values: the transform plus viewport fixup, paired-unit
  rules, `r12`, relative addressing, op formulas, combiner mappings, mux,
  blue-to-alpha, final combiner, fog modes, clip planes, alpha test, and the
  address maths of every texture mode through an echoing sampler. Every
  generated shader, including all texture-mode chains and option
  combinations, goes through naga validation and the naga MSL backend.
- `tests/vsh.rs` and `tests/gpu.rs` share a table of RCC, EXP and LOG
  boundary cases (±0, saturation both ways, infinities, just below a power
  of two). The expected values are written from the spec, not taken from the
  interpreter, and both the interpreter and the GPU must reproduce them.
  `tests/common::close` only accepts a non-finite value when it matches
  exactly.
- `tests/gpu.rs` runs randomly generated vertex programs (120 programs × 32
  vertices) on the GPU through a compute harness and compares every finite
  output with the interpreter at a relative tolerance, however large (over
  1,000 exceed 1e4). Non-finite expected values are tallied separately; see
  the gaps.
- It also runs random combiner programs (200 × 32 fragments) with real
  textures. Each one is RGBA8 with nearest filtering and repeat addressing
  (2D 4×4, 3D 2×2×2, cube 2×2 per face), with a CPU twin as the
  interpreter's sampler. The generator covers every supported texture mode,
  including bump-env, dependent, dot_st/str and reflection chains, as well
  as random dot mappings, input selectors, bump matrices, eye vectors and
  alpha kill. The harness samples at LOD 0 (`PixelOptions::explicit_lod`),
  since compute shaders have no derivatives. Lookups within rounding of a
  texel or cube-face boundary are skipped and counted. Every other fragment
  matches, and each mode is compared on at least 32 fragments.
- `tests/render.rs` draws through a linked module with a real texture and
  checks the pixels.
- The GPU tests pass trivially when no adapter is present. Set
  `SHADERS_NO_GPU` to force that.
- `tests/corpus.rs` is optional. With `H2_SHADER_CORPUS` set it checks the
  directory against its manifest, validates the disc's shaders and runs
  every vertex program on the GPU against the interpreter. On the retail
  corpus that is 248,512 comparisons with no magnitude cut-off, all
  matching.

## Gaps

- **Point sprites.** `oPts` is computed, but WGSL has no point size. Halo 2
  writes it in 2 programs, which need quad expansion in the renderer.
- **dot_zw depth replacement, brdf, HILO dot mappings.** See the table
  above. None of them appear in Halo 2.
- **Persistent constant writes** from read/write programs and **state
  programs** are not translated. Neither is used by Halo 2.
- **Edge-case float behaviour.** wgpu's Metal backend compiles with fast
  math, so arithmetic on an infinity isn't IEEE on the GPU. An `add` fed by
  LOG(0) returned 0 where the interpreter gives −inf. In the random vertex
  test 352 of 2,472 non-finite expected values differ this way, all in two
  programs. The ILU helpers produce their infinities from bit patterns and
  match exactly. Inf and NaN in `rcp`/`rsq`, and NaN through `min`/`max`,
  follow the host GPU. The NV2A's exact rounding isn't modelled. No Halo 2
  program produces a non-finite value on the corpus test's inputs.
- **Colour key, convolution filters, shadow-map compare (PROJECT2D/3D on
  depth textures) and texture border adjustment** are sampler and texture
  state outside the PSDEF. They're left to the renderer and not implemented.
- **Bump-env channel and sign conventions** rely on the renderer decoding
  V8U8/L6V5U5 as signed `.rg` (and `.b` for luminance).
- **What the `vrtx` u16 block and the `spas` index fields mean**, and how the
  `spas` constant blocks feed `c0`/`c1`, belongs to the tag and renderer
  work. The translator only takes ready constant values.

## Sources

The bit layouts and semantics were checked against these sources, read for
facts only. No code was copied, and xemu and Cxbx-Reloaded are GPL.

- xemu, commit `4bdad199102b22ed0cfa998e9af6b35d76238b3e`:
  `hw/xbox/nv2a/pgraph/glsl/vsh-prog.c` (microcode field table, paired-unit
  rules, op formulas, ARL bias), `glsl/vsh.c` (output defaults,
  `clampAwayZeroInf`, 1/16 screen-coordinate truncation, clip-space
  conversion), `glsl/psh.c` and `psh_regs.h` (combiner encodings,
  evaluation order, texture modes, dot mappings), and `vsh_regs.h` (opcode
  and mux enums). <https://github.com/xemu-project/xemu>
- Cxbx-Reloaded, commit `585c49a50af1255ab155099e06f24505f9c5a800`:
  `src/core/hle/D3D8/XbD3D8Types.h` (`X_D3DPIXELSHADERDEF` field order,
  `X_VSH_SHADER_HEADER` and the `0x2078`/`0x7778`/`0x7378` versions) and
  `Direct3D9/CxbxVertexShaderTemplate.hlsl` (the screen-space epilogue and
  its reversal). <https://github.com/Cxbx-Reloaded/Cxbx-Reloaded>
- Assembly (XboxChaos), commit `fd14ef3c46135e9422966c6a64cd926fb0bd23fb`:
  `src/Assembly/Plugins/Halo2Xbox/vrtx.xml` and `spas.xml` (tag layouts;
  the PSDEF offset inside the implementation block is our own finding,
  above). <https://github.com/XboxChaos/Assembly>
- NVIDIA's `NV_register_combiners` and `NV_vertex_program1_1` OpenGL
  extension specs, for the general combiner and vertex-program model the
  NV2A is built on. The final combiner's V1R0 operand mapping and the
  EXP/LOG infinity cases come from these.
- abaire's nxdk_pgraph_tests, commit
  `23714459529bdd6bc8587f358625485ba3eabfef`: `src/tests/combiner_tests.cpp`,
  `TestSpecularR0SumFlags` (lines 376 to 475; the cases are around line
  425), the hardware test of the final combiner's `SRC_SPEC_R0_SUM`
  (V1R0_SUM) flags and its negative-r0 cases.
  <https://github.com/abaire/nxdk_pgraph_tests>
