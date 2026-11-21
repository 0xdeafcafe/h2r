# Shaders

Status: Built. Reference for `crates/formats/shaders`.
Code: `crates/formats/shaders/src/`. Tests: `crates/formats/shaders/tests/`.
Switch: none.

`crates/formats/shaders` turns Halo 2's Xbox shader programs into WGSL. We don't
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
`02.09.27.09809`, all 30 maps). `h2 extract-shaders` (`crates/apps/cli/src/extract_shaders.rs`) reproduces it,
reading the maps with `crates/formats/map`. It
reads your own disc image at run time and writes the corpus, a manifest
and a provenance record to a directory you choose (keep it in `$TMPDIR`; no
game data belongs in the repo). Every count in this section is a line of its
`provenance.txt`. Across the maps there are 353 distinct vertex programs and
149 distinct pixel shader definitions. All of them decode, translate and pass
naga validation, and none of them produce a translation warning.

### `vrtx` (vertex_shader)

The layout, as checked on the data:

```text
root (0xC):  u16 (1 in all 2,261 tags) | int16 | block program entries
entry (0x1C): u32 | block<u16> | dataref compiled shader | dataref code
```

- **compiled shader** is the `XGAssembleShader` output: the bytes `'x' 0x20`,
  a u16 instruction count, then 16 bytes per instruction. Every retail
  program has kind `0x20` (a normal shader). None are read/write (`'xw'`) or
  state (`'xs'`) programs.
- **code** is empty in every entry (all 11,837). Retail maps carry no D3D
  `vs.1.1` token streams or source, so the crate only takes microcode.
  Nothing on the disc needs a D3D token parser.
- The `u16` block is a short list of small integers, such as `0,1,2,3,4,5,6`.
  It is **not** the set of `v` registers the program reads: of the 10,712
  entries with a non-empty block, it matches in 1,985 and differs in
  8,727. It is probably vertex-declaration related, and its meaning is still
  unconfirmed.
- 765 entries are empty; the other 11,072 all hold `'x' 0x20`
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
            +0xF6..      index fields (meaning unverified), vertex shader tag ref (+0xFC), ...
```

To locate the struct we looked for fields that only make sense at one
offset. Taken from `+6`, `PSCombinerCount`
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

Each blob is the bytes of a `vrtx` entry's compiled shader
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
x, y  ← trunc(x·16)/16, trunc(y·16)/16      (rasterizer keeps 4 fractional bits, truncates;
                                             not at a render scale: high-resolution.md)
w     ← clamp_away(w)                       (as rcc: never 0 or inf)
ndc   = (2x/W − 1,  1 − 2y/H,  z/Zmax)      (wgpu y is up; NV2A window y is down)
        z/Zmax held to 1 when z <= Zmax,    (the depth clip range is inclusive)
        and to at least 1 + 2^-23 when z > Zmax
clip  = (ndc·w, w)                          (the GPU's divide by w restores the window position)
```

The NV2A clips depth against `[CLIP_MIN, CLIP_MAX]` inclusively (xemu
discards only when `z < clipRange.z || clipRange.w < z`, in
`glsl/psh.c`), and D3D sets the clip max to the depth full scale. So z
exactly at `Zmax` is inside. Halo's full-screen passes rely on this: the
main menu's blue tint draws its quad at z = w = 0xFFFFFF, the D24 maximum.
On Metal `z / Zmax` divides through a reciprocal and 0xFFFFFF / 0xFFFFFF
comes out just above 1, so the quad was clipped and the tint was missing.
The fixup therefore clamps to 1 whenever `z <= Zmax`. Since `1·w` is exact,
clip z then equals w. The division can also round the next float above
`Zmax` down to exactly 1 (D16: 65535.0039 / 65535), so z past `Zmax` is
raised to at least the float after 1, and `(1 + 2^-23)·w` is always above
w. The test is a comparison of the bit patterns (`Zmax` is positive, so
integer order is float order there). With a float comparison, Metal's fast
math folded the select into a plain `min(z/Zmax, 1)` and drew the D16 quad
at the next float. Anything past `Zmax`, and anything below 0, still
clips. `tests/render.rs` (`quad_at_exactly_z_max_is_not_clipped`) draws
quads at z = 0, exactly `Zmax`, the next float above it, a few ulps past
it, 1.5·`Zmax` and −1, for
D24, D16 and float depth, with w = 1 and w = z. Without the clamp the D24
case fails on Metal. Without W-buffering the clip range is taken as `[0, Zmax]`: the
`CLIP_MIN`/`CLIP_MAX` the game pushes (`d3d::hw::HwState`) are applied
only to w when W-buffered (below), and the NV2A's clamp mode isn't applied either (`ZCLAMP_EN`), where out-of-range z is
clamped instead of culled.

### W-buffering

With `SET_CONTROL0` bit 16 (z-perspective; D3D sets it
for `D3DRS_ZENABLE = D3DZB_USEW`, and Halo enables that at boot with
`ZEnable(2)` at `0x1267a`) the NV2A ignores the program's z: it depth-tests
and stores the perspective-correct interpolated w, plus the polygon
offset, and clips w to `[CLIP_MIN, CLIP_MAX]` (xemu's z-perspective path
in `glsl/psh.c`). `d3d::nv2a` turns this on for vertex programs
(`nv2a::w_buffered`) with `VertexUniforms::w_clip` and
`PixelOptions::w_buffer`. The vertex stage then outputs clip z =
a·(w − lo) with a = hi / (hi − lo): it is 0 at w = lo and equals w at
w = hi, and it is linear in the clip coordinates, so the rasterizer's
`0 <= z <= w` clip keeps exactly `lo <= w <= hi` (`interp::fixup_w_buffer`).
With `CLIP_MIN` at 0 that cannot cull the far end (a = 1), so such
draws are clamped there instead. The fragment stage writes
`frag_depth = q(clamp(1 / position.w + units + factor · max(|dpdx|, |dpdy|),
lo, hi)) / Zmax`, with the polygon offset in `XpsUniforms.depth` and the
range in `XpsUniforms.depth_range`, and the pipeline's own depth bias is
turned off for such draws (`PreparedShader::writes_depth`). For D16 and D24,
q is `floor`: the NV2A stores the whole part of w, as xemu's D24 and D16
paths do. Halo draws a surface in several passes with different programs,
the later ones testing EQUAL; passes whose w differs in the last bit or two
stored different floats, the likely source of the dense stripes on
Ascension's tower and the Banshee, while the whole part almost always agrees
(`w_buffered_passes_with_ulp_different_w_store_equal_depth`: 88% of the
pixels failed without it, under 1% with it). The position is also
`@invariant`, so identical position code gives identical bits in every
program.

`D3DRS_DEPTHCLIPCONTROL` (`NV097_SET_ZMIN_MAX_CONTROL`) with
`D3DDCC_DEPTHCLAMP` (bits 7:4 = 1) clamps out-of-range depth instead of
culling it (`ZCLAMP_EN` in xemu's `pgraph.c` and `glsl/psh.c`). Such a draw
(`VertexUniforms::w_clamp`) outputs clip z = w / 2, so nothing is culled by
the range, and the fragment clamp keeps the stored w inside it. Ascension's black
sky is the likely case: sky geometry past the far clip, culled
(`w_buffered_depth_clamp_keeps_what_lies_past_the_clip_range`).

Such a draw's pipeline also leaves depth unclipped
(`PrimitiveState::unclipped_depth` from `RenderState::depth_clamp`, where
the device has `DEPTH_CLIP_CONTROL`; Metal's depth clip mode `clamp`).
This is emulation correctness: on the NV2A a clamped draw keeps what lies
outside the depth range, so the host must not cull it at the near or far
plane. The other planes still clip: the clip volume is bounded by
`-w <= x, y <= w` (WebGPU spec, 23.2.4 "Primitive Clipping"). It does not
remove a10's grey slab, which stays open (docs/halo1/sky.md, "Edge-on
triangles"). A Z-buffered draw
that clamps gets the NV2A's meaning from the same switch: its depth, z
over `Zmax` as the guest computed it, is clamped to the viewport's depth
range (the WebGPU spec clamps every fragment's depth to it) instead of
culled. Without the feature, draws are clipped as before and the renderer
says so once.

A draw that neither tests nor writes depth gets no depth write. Writing
the depth from the fragment stage turns off the GPU's early depth test for
the draw, which costs frame time; rasterizing the depth instead needs a
mapping of w that is affine in screen space (a function of 1/w), and none
keeps the ordering across the range at float precision as the stored w
does, so this stays a known cost. Halo's first-person weapon and hands use
another projection than the level, so their z did not order against the
level's while their w does; drawn with z they were hidden behind nearby
walls. `crates/platform/d3d/tests/shaders_render.rs`
(`w_buffered_programs_depth_test_w_not_z`) checks the order and that z past
`Zmax` no longer clips. Not covered: pre-transformed (`XYZRHW`) draws keep
z, and an occlusion count pass (which does not run the fragment program)
still tests the rasterized z.

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
  combiner the output is `clamp(r0)`. That is the translator's own
  default; the hardware always runs one. When a definition has none,
  Xbox D3D's `SetPixelShader` (`0x3fa3d0`) skips
  `SET_COMBINER_SPECULAR_FOG_CW0/1` and marks the fog/specular state dirty.
  The lazy commit (`0x00400000`) then programs the final combiner it also
  uses for the fixed-function path: with `FOGENABLE`, `A = fog.a`, `B = r0`
  (`v1+r0` with `SPECULARENABLE`), `C = fog.rgb`; without it, `D = r0`
  (or `v1+r0`); always `G = r0.a` and the clamp-sum flag (`0x130C0300`,
  `+0x20000` for specular, `0x0C`/`0x0E`, and `0x1C80`).
  `psh::d3d_default_final_combiner` returns those words, and
  `PixelShaderDef::with_default_final_combiner` fills them in. `d3d::nv2a`
  applies it to every guest definition. All 140 retail definitions with
  general combiners carry their own final combiner. The other 9 are
  variant shells whose combiners the engine builds at run time, and those
  always get one too (ABCD `0xc`, EFG `0x2000`; see shader-instances.md).
  So this only changes draws whose pixel shader the game builds without
  one.
  Whether a definition has one is latched when it is bound
  (`0x3fa452`, the OR of its two final dwords into device `+0x788`), and
  pushing zero to both final-combiner methods later does not clear the
  latch. The hardware then runs an all-zero final combiner, RGB 0 and
  alpha 0. The lazy commit reads the stored latch, not the
  definition, so editing the bound definition's buffer without rebinding
  changes nothing either. The device reads `+0x788` at each draw into
  `ShaderInputs::final_combiner_latch`, and `d3d::nv2a::guest_def` sets
  `PixelShaderDef::explicit_final` from it rather than from the merged
  words or the buffer; `d3d::nv2a` counts such draws in
  `Nv2aStats::zero_final_draws`.
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
- **Fog** reads (fog colour rgb, factor) in any combiner. The factor comes
  from the NV2A fog unit, below.

### Fog

The vertex stage writes a fog *coordinate*, `oFog.x`, and the fog unit
turns it into the factor the combiners read as `fog.a` (`psh::fog`). The
NV2A state is `SET_FOG_ENABLE`, `SET_FOG_MODE`, the bias and scale of
`SET_FOG_PARAMS`, and `SET_FOG_GEN_MODE`. Halo 2's D3D pushes them in its
fog commit (`0x00400000`, from the deferred render states at `0x407608`),
and `d3d::nv2a::d3d_fog` derives the same values from the render state:

| `FOGTABLEMODE` | mode | bias | scale | gen mode |
|---|---|---|---|---|
| `NONE` (0) | `LINEAR` (`0x2601`) | 1 | 1 | `SPEC_ALPHA` |
| `LINEAR` (3) | `LINEAR` | `1 + end/(end − start)` | `−1/(end − start)`, or −8192 when start == end | `PLANAR`, or `RADIAL` with `RANGEFOGENABLE` |
| `EXP` (1) | `EXP` (`0x800`) | 1.5 | `density · −1/(16 ln 2)` | as for `LINEAR` |
| anything else, `EXP2` (2) included | `EXP2` (`0x801`) | 1.5 | `density · −1/√(32 ln 2)` | as for `LINEAR` |

With `FOGENABLE` off only `SET_FOG_ENABLE` 0 is pushed. The fog unit then
computes, per fragment from the interpolated coordinate `d`:

```text
LINEAR   bias + scale·d − 1
EXP      bias + 2^(16·scale·d) − 1.5
EXP2     bias + 2^(−32·(scale·d)²) − 1.5
*_ABS    the same with |d|     (0x802 EXP_ABS, 0x803 EXP2_ABS, 0x804 LINEAR_ABS)
off      1
```

and clamps to [0, 1]. These are the formulas xemu's `glsl/vsh.c`
evaluates. With D3D's parameters they give `(end − d)/(end − start)`,
`e^(−density·d)` and `e^(−(density·d)²)`, and with no table the factor is
`oFog.x` itself. The model computes the exp scales in closed form. D3D multiplies by its
own constants (`0x45E378`, `0x45E374`), which differ from the closed forms
by about 4e-6 relative; the ignored `crates/platform/d3d/tests/fog_xbe.rs` reads
them from `$H2_XBE` and checks that tolerance. The uniform
carries `[mode, bias, scale]`, computed with f64 intermediates and f32
stores, as the x87 code rounds. The fog register's rgb is
`D3DRS_FOGCOLOR`. It stays a `D3DCOLOR` (ARGB) in the render-state array,
and `SetRenderState_FogColor` (`0x3f5370`) swaps red and blue only in the
`SET_FOG_COLOR` method it pushes. The register's alpha is always the
factor, never the colour's alpha.

**Where the coordinate comes from.**

- With a vertex program bound, the coordinate is whatever the program
  leaves in `oFog.x` (64 of Halo's 353 programs write it). The generation
  mode only picks the
  coordinate of the fixed-function transform (specular alpha, radial eye
  distance, planar eye depth, absolute planar, fog X), and xemu likewise
  takes `oFog.x` as the distance whenever a program is bound. So
  `RANGEFOGENABLE` changes nothing for any draw this project renders.
  Halo never draws untransformed fixed-function vertices (`d3d::nv2a`
  skips them), and none of the fog writes in its vertex programs depends
  on an `rsq`, which a radial distance needs (see the census below).
- Pre-transformed (`XYZRHW`) vertices run one of D3D's three pass-through
  programs (`0x3f6f40`, `GuestLayout::passthrough_programs`), which are
  vertex programs too. Without a fog table, `mov oFog, v4.w` writes the
  specular alpha (D3D's vertex fog). With a table, `mov oFog, v0.z` writes
  the window z when the projection is affine (device flag 2), and
  `rcp oFog, v0.w` writes w otherwise. `d3d::nv2a::PassthroughFog`
  reproduces the choice (the device records the flag in
  `ShaderInputs::affine_projection`), and the synthesized pre-transformed
  program writes the same coordinate: `rcp` of the vertex's own rhw, so
  `rhw = 0` gives infinity as in the guest, while the position still
  treats `rhw = 0` as 1. An infinite coordinate, of either sign, gives a
  fixed factor per mode whatever the bias and scale, as xemu's
  `glsl/vsh.c` assigns: 1 for `LINEAR`, `LINEAR_ABS` and `EXP`, 0 for
  `EXP_ABS`, `EXP2` and `EXP2_ABS` (`psh::fog::special_factor`). A NaN
  coordinate or formula result gives the same. These are not the limits
  of the formulas: with D3D's negative scale, linear fog tends to 0, not
  1, so with start −10 and end 10 an infinite coordinate reads 1 where
  the coordinate 0 would read 0.5. The WGSL recognises infinity and NaN
  by their bits, because the GPU's fast math does not keep IEEE
  semantics. xemu evaluates the factor per vertex and clamps it to the
  finite float range before interpolating; here the coordinate itself is
  interpolated. Whether
  the rasteriser interpolates an infinite varying as infinity across a
  triangle (rather than NaN where a vertex's weight is 0) is not
  established. `crates/platform/d3d/tests/fog_xbe.rs` (ignored, `$H2_XBE`) runs the
  image's three programs through the interpreter, `rhw = 0` included, and
  checks that their `oFog` matches, along with the commit's constants
  (against the closed forms) and method headers.

**Not settled by hardware evidence here.** The factor is evaluated per
fragment. xemu evaluates it per vertex and marks that as a guess; the two
agree for linear fog and differ inside a triangle for the exp modes. The
`ABS` modes are taken to fold the coordinate (the eye-plane-absolute
meaning), where xemu folds the factor. Xbox D3D never selects them. With
fog off the factor is 1, as in xemu.

### What the retail definitions use

The extracted corpus (149 definitions) gives this tally. Counts are per
input or output slot that names a register, RGB portion first, then alpha.

- **Input mappings:** unsigned identity 1,032 / 503, invert 99 / 47,
  expand 71 / 51, expand-negate 5 / 3, half-bias 3 / 0, half-bias-negate
  12 / 19, signed identity 10 / 35, signed negate 2 / 8.
- **Output mappings:** identity 428 / 458, ×2 55 / 10, ×4 1 / 16, ÷2
  2 / 2. No definition uses the −½ bias.
- **Flags:** AB dot 64, CD dot 37, mux 8 (RGB) and 49 (alpha), and no
  blue-to-alpha.
- **Final combiner:** inputs only use the identity and invert mappings.
  `E·F` is read 36 times (9 as A, 24 as B, 3 as D). `v1+r0` and the
  complement flags are never read. `G` is mostly `r0.a` (47) or `t0.a`
  (24), with 10 blue reads.
- **Fog:** 5 definitions read the fog register, only in the final
  combiner: `1 − fog.a` as F (2) and as A (2), `fog.a` as B (1) and
  `fog.rgb` as C (2). Where `1 − fog.a` is A and `fog.rgb` is C, the
  definition treats the factor as the amount of fog, the reverse of D3D's
  convention.
  64 of the 353 vertex programs write `oFog`, one write each. By the
  shape of the writing instruction: 54 are a dot with a constant
  (`dph oFog.xw, rN, c[35]`, or `c[26]` in 6), 9 multiply a `.w`
  (`mad oFog.w, rN.w, c.x, c.y`) and 1 moves a vertex attribute
  (`mov oFog.xw, v7.w`). The shapes say nothing about how `rN` was
  computed; a backward slice over the temporaries does, and no fog write
  depends on an `rsq`, so none is a radial distance.
  `tests/corpus.rs` (`corpus_fog_census`) prints this tally and fails on
  a write of another shape or one whose slice has an `rsq`
  (`fog_slice_finds_a_radial_distance` checks the slice on synthetic
  programs).
- No definition reads a texture register of a `NONE` stage before writing
  it. Only one reads `r0.rgb` before writing it (zero on both sides), and
  only one reads the initial `r0.a`, with stage 0 textured.

`h2 shader --all all` over the resolved materials adds the stage and blend
states. `COLORSIGN` is set 30 times, always to `0xE0000000` on stage 0.
`COLORKEYOP` is never set. `BLENDOP` is always `ADD`. `SRCBLEND` is
`CONSTANTCOLOR` in 50,046 implementations and `DESTALPHA` in 41,525.
`DESTBLEND` uses `CONSTANTCOLOR`/`INVCONSTANTCOLOR` 10,359 times, and
`BLENDCOLOR` is mostly `0x00FFFFFF`. That last pair writes RGB and leaves
alpha to the constant's zero.

### Signed texture channels (`COLORSIGN`)

`D3DTSS_COLORSIGN` sets the NV2A texture unit's per-channel sign bits
(`D3DTSIGN_ASIGNED` bit 28, `RSIGNED` 29, `GSIGNED` 30, `BSIGNED` 31; the
same bits as `SET_TEXTURE_FILTER`). A signed channel is the texel byte read
as two's complement `s`, and the texture unit delivers `2s/255`, clamped at
−1: 0x7F gives 254/255 (not 1), 0x40 gives 128/255, 0x00 gives 0, 0xFF gives
−2/255. The positive side is taken from Xbox hardware captures
(nxdk_pgraph_tests golden results, `Texture_signed_component_tests`: bytes
64, 100 and 127 render as 128, 200 and 254), not from the modern
`max(s/127, −1)` SNORM rule. The negative side, and 0x80 in particular,
follows the same formula but has not been checked against hardware.

Hardware converts each texel before filtering, so a bilinear blend of 0x7F
and 0x80 is about 0. To get that from the host's UNORM filtering, the
texture is uploaded with every signed channel's byte XORed with 0x80
(stored as `s + 128`, `gfx::texture::DecodeOptions::signed`, set from
`COLORSIGN` by the device; signed DXT takes the CPU decode path for this).
Filtering is then linear in `s`, and the pixel shader turns the filtered
value `v` back into `max(2v − 256/255, −1)` right after the fetch, before
bump-env, dot products, alpha kill or the combiners see it. The `COLORSIGN`
bits are part of the device's texture cache key, so one texture used signed
and unsigned is two host textures. `PixelOptions::color_sign` holds
`COLORSIGN >> 28` per stage (`bits::color_sign`); the interpreter's
`Sampler` returns host (biased) values too and applies the same transfer.
Frame-buffer snapshots bound as textures are not stored biased, so the
device clears `COLORSIGN` for those stages. Colour textures over depth
memory use signed-channel bias and separate host copies for each sign mask,
as ordinary textures do. Depth-format shadow maps always retain their raw
bits and ignore `COLORSIGN` in the upload and sampling state. Stand-in
textures (unbound stages, dimension mismatches) are biased by the stage's
sign mask too (`ProgramBindings::dummy`, keyed by it), so they still read black. The
host's border presets cannot express a biased or arbitrary border, so
the shader supplies those colours (and all colours on hosts without border
addressing) (`PixelOptions::border`): the sampler clamps to the
edge on the `BORDER` axes and a coordinate outside `[0, 1]` there reads
`D3DTSS_BORDERCOLOR` as the texture unit would, signed channels as `2s/255`
of the border byte (an unverified guess at the hardware; 0x00 reads 0
either way). The half texel at the edge where hardware blends texels with
the border reads the edge texel instead. The transfer applies to the
channel the texel format expands to (an implicit alpha of 1 reads −2/255),
which is unverified for anything but 8-bit channels. Retail Halo 2 sets
`COLORSIGN` in 30 resolved implementations, all on stage 0 with RGB signed
(`0xE0000000`).

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
| dot_rflct_spec_const | supported; eye from `XpsUniforms.eye`, which the d3d provider fills from `NV097_SET_EYE_VECTOR` (`0x181C`, PGRAPH `EYEVEC0..2`) |
| dpndnt_ar, dpndnt_gb | supported |
| **dot_zw** | **unsupported.** The dot product is computed but depth is not replaced (no `frag_depth` write). Used 0 times |
| **brdf** | **unsupported.** Reads zero (xemu doesn't implement it either). Used 0 times |
| dot mappings zero_to_one, minus1_to_1_d3d/_gl/(nv) | supported, for unsigned 8-bit data |
| dot mappings hilo_1, hilo_hemisphere_d3d/_gl/(nv) | supported; the texel is read as a 32-bit word, see below |

Halo 2's tags use none of the unsupported entries. A stage that holds a mode it
can't (dot_st in stage 0, for example) is skipped with a warning.
`PixelShader::warnings` lists everything that was approximated. The d3d
provider prints them to stderr (`nv2a: pixel shader <id>: ...`) once per
translated program, and `Nv2aShaders::warnings` returns them all.

The "used" counts cover the 149 tag definitions and the 2,931 distinct
pixel shaders of the 307,944 resolved material implementations
(`h2 shader --all all`), which all translate without a warning. Neither
BRDF nor dot_zw appears, so both stay unimplemented. dot_rflct_spec_const
doesn't appear either, and no push of `SET_EYE_VECTOR` has been seen
(the device would otherwise stop on it as an unknown method). The method
is recorded anyway (`d3d::hw::HwState::eye_vector`), so the eye is set if
something pushes it.

#### HILO dot mappings

A HILO mapping reads its source texel as two 16-bit halves of one 32-bit
word instead of three 8-bit channels. The translator takes the word as an
`A8R8G8B8` texture samples it in gfx (a = bits 31..24, r = 23..16,
g = 15..8, b = 7..0, the split xemu's `dotmap_hilo_1` uses too), so
hi = a·256 + r and lo = g·256 + b, each channel rounded from `x·255`.

| mapping | vector |
|---|---|
| hilo_1 | (hi/65535, lo/65535, 1) |
| hilo_hemisphere_d3d | (h, l, √max(0, 1 − h² − l²)) with h = (hi − 32768)/32767, likewise l |
| hilo_hemisphere_gl | the same with two's-complement halves, h = (2s + 1)/65535 |
| hilo_hemisphere | the same with two's-complement halves, h = s/32767 |

`hilo_1` is NV_texture_shader's `HILO`-to-`(hi, lo, 1)` rule. The
hemisphere variants follow the spec's hemisphere completion, with the three
signed conventions carried over from the 8-bit mappings. xemu leaves them as
`FIXME`, and none of them has been checked against hardware.

The menu's depth-fog pass uses this. It binds the depth buffer as a colour
texture and dots it, with `hilo_1`, against texture coordinates that put the
halves back together: a D24S8 word is `depth << 8 | stencil`, so hi is the
top 16 bits of depth and lo is the low depth byte above the stencil byte.
For this to work the renderer has to present the depth texel's 32-bit word
in that `A8R8G8B8` layout, which is what d3d-r8 provides. Two caveats
apply. Filtering 8-bit halves is not 16-bit filtering (a carry between r
and a is lost), so only point-sampled HILO stages are exact. A format that
forces alpha (`X8R8G8B8`) loses the top byte. No retail tag uses a HILO
mapping: all 10 dot-product stages in the 149 definitions, and every dot
stage across the 307,944 resolved material implementations, use
`minus1_to_1_d3d`. `tests/psh.rs` (`hilo_dot_mappings_split_the_texel_word`)
works through a D24S8 word and each hemisphere convention by hand.
`tests/gpu.rs` draws HILO mappings in the random combiner test and requires
at least 32 compared dot stages per mapping.

Texture dimensions follow the mode. `PixelOptions::texture_dims` overrides
project2d, project3d and cubemap the way D3D's `TEXMODE_ADJUST` would, but
Halo 2 never sets that flag. For bump mapping the generated code reads du/dv
as `.rg` and luminance as `.b` of the source stage as the texture unit
delivered them. They are signed only through `COLORSIGN` (see "Signed
texture formats" below). 2D coordinates are multiplied by
`XpsUniforms.tex_scale[n].xy`, which covers linear (texel-addressed)
textures.

#### Signed texture formats

`D3DFMT_V8U8`, `Q8W8V8U8` and `L6V5U5` aren't formats of their own on the
Xbox. They are the same NV2A format codes as `G8B8` (0x28, linear 0x17),
`A8B8G8R8` (0x3A) and `R6G5B5` (0x27), and `gfx::format::D3DFormat` gives
each pair one variant. Nothing in the texture header says that a texel is
signed, so neither the device nor gfx can decode one of these formats as
signed from the format alone. The NV2A takes signedness from the texture
unit's per-channel sign bits, `D3DTSS_COLORSIGN`. The dot mappings
(`minus1_to_1_*`) convert inside the dot product instead.

Halo 2 sets `COLORSIGN` exactly where it needs signed texels. An
`h2 shader --all all` tally of the 307,944 resolved material
implementations finds 30 implementations with bump-env stages, each of
them reading stage 0 from stages 1, 2 and 3, and stage 0 is the only stage
with `COLORSIGN`, always `0xE0000000` (RGB signed), in exactly those 30. No
dot-product source stage is signed, and they all use `minus1_to_1_d3d`. So
signed V8U8 data reaches bump-env through the `COLORSIGN` path (see
"Signed texture channels"). That path stores the signed byte `s` as
`s + 128` (`gfx::texture::DecodeOptions::signed`) so host filtering stays
linear, and converts the filtered value to `2s/255` at the fetch, before
bump-env reads `.rg`. No format-based signed decode is needed, and one
would be wrong: a `G8B8` texture without `COLORSIGN` reads unsigned on
the NV2A too.

### Shadow maps

On the NV2A a texture in a depth format (`D24S8`, `D16`, `F24S8`, `F16`,
swizzled or linear) is always a shadow map. PROJECT2D and PROJECT3D on it
compare the stored depth with a reference and give 1 or 0 in every
channel. `PixelOptions::shadow[n]` marks such a stage and carries the
format, the function, the filter and the addressing (`psh::shadow`). The
behaviour follows xemu's `psh_append_shadowmap`:

- The comparison is `stored OP reference`. `OP` is `SHADOWCTL`'s
  function, which D3D sets from `D3DRS_SHADOWFUNC`
  (`SetRenderState_ShadowFunc` pushes `Value - 0x200` to
  `NV097_SET_SHADOW_DEPTH_FUNC`), so the `D3DCMP_*` value applies as
  written. `GREATER` (`0x204`, the first campaign level's) passes where the
  stored depth is greater than the reference.
- PROJECT2D compares with 0. PROJECT3D compares with `r/q` clamped to
  `[0, max]`: 16777215 for D24, 65535 for D16, 511.9375 for F16 and
  `(2 - 2^-16) * 2^127` for F24 (the `D3DZ_MAX` values window z uses;
  xemu uses 16777215 for every 24-bit format).
- NEVER gives 0 and ALWAYS gives 1, without sampling.
- The renderer uploads depth textures as raw bits in RGBA8 (depth bytes
  high to low in r, g, b, stencil in a; D16 and F16 in r and g), without
  any `COLORSIGN` bias. The stored value is rebuilt from `round(c * 255)`
  exactly: an integer for D24 and D16; for F24 the 24 bits are an IEEE
  single's top bits without
  the sign (8-bit exponent, bias 127, 16-bit mantissa); F16 has a 4-bit
  exponent (bias 7) and a 12-bit mantissa with no denormals: every word
  but 0 has the implicit leading one, exponent 0 included. Both readings
  are xemu's (`convert_f16_to_float`, `convert_f24_to_float` in
  `pgraph/util.h`).
- Filtering is per texel, then on the results. GeForce3-class hardware,
  which the NV2A is, compares each of the four neighbouring texels and
  bilinearly weights the 1/0 results when the filter is linear (2x2
  percentage-closer filtering; NVIDIA's "Hardware Shadow Mapping"
  whitepaper). xemu filters the depth values and compares once; we don't.
  The translation fetches texels with `textureLoad`, so the host never
  filters raw bits. It uses one texel (`floor(uv * size)`) for point
  filtering, or four around `uv * size - 0.5` weighted by the fraction
  for linear. Wrap, mirror and clamp addressing apply to the texel
  indices.
- `s/q` and `t/q` are multiplied by `tex_scale`, as for colour textures,
  so linear (texel-addressed) shadow maps work.
- **Hard shadows** (`PixelOptions::hard_shadow`, the Lighting row on the
  D3D path, [e3-lighting-setting.md](e3-lighting-setting.md)): a stage with a non-zero mask
  compares once at the nearest texel, whatever its filter, at the mean of
  the coordinates of the stages in its mask (the stages that sample the
  same map), so taps spread over stages collapse to one. Only the
  render-scale view sets it; all zero is the retail code byte for byte
  (`tests/shadow.rs`, golden files in `tests/golden/`).

abaire's nxdk_pgraph_tests check the comparison on hardware
(`texture_shadow_comparator_tests.cpp`, with results in
nxdk_pgraph_tests_golden_results). Its function names are the operands
swapped: its `GREATER` is register value 1, `LESS` here, and on hardware
that passes where the stored depth is below the reference (raw-value test
`3R24_0-FFFFFF_7FFFFF_GT`: the 0, 1 and `ref - 1` boxes pass, `ref` and
above don't). This matches xemu and the order used here. PROJECT2D with
the same function passes nowhere (`2R24_..._GT`), matching "compare with
0".

The other per-pipeline state the combiners need comes in through
`PixelOptions`: `D3DTSS_ALPHAKILL` per stage (kill when the fetched alpha is
0), `D3DTSS_COLORSIGN` per stage, the alpha test (wgpu has none, so it's emulated with `discard` against
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
the buffer contents. The final `XpsUniforms` vector, `depth_range`, packs
the W clamp in `.xy` and the host pixels per guest pixel for polygon
stipple in `.zw`. Vertex register `vN` is `@location(N)`.
`VertexOptions::provided_inputs` names the registers the vertex layout
supplies. Any other register the program reads comes from
`XvsUniforms.defaults[N]` (the `SetVertexData4f` value). The varyings are
locations 0 to 3 for oD0, oD1, oB0 and oB1, 4 to 7 for oT0 to oT3, and 8 for
the fog coordinate (`oFog.x`; the fragment stage turns it into the factor).

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
- `tests/gpu.rs` (`fog_unit_modes_on_gpu`) also runs every fog mode, off
  and vertex fog over coordinates on both sides of their ranges and
  compares the fog register with the interpreter, which `tests/psh.rs`
  checks against D3D's closed forms.
- `tests/shadow.rs` covers shadow maps with synthetic raw-bit depth
  textures. It checks every comparison function with D24, D16, F24 and F16,
  in PROJECT2D and PROJECT3D, against stored values below, equal to and
  above the reference. It also checks clamping of `r/q`, exact decoding
  (neighbouring 24-bit values that any depth filtering would blur), 2x2
  PCF weights, and wrap, mirror and clamp addressing. NEVER and ALWAYS
  must not sample. The expected values come from the ordering alone, and
  both the interpreter and the GPU must match them exactly (461 fragments
  on the GPU). The hard (one-tap) option has its own tests there: golden
  WGSL for retail and for E3 (`tests/golden/shadow_retail.wgsl`,
  `shadow_e3.wgsl`; retail's text must not change), one tap on the
  interpreter and the GPU, and stages of one map comparing at their mean
  coordinate.
- `tests/render.rs` draws through a linked module with a real texture and
  checks the pixels.
- The GPU tests pass trivially when no adapter is present. Set
  `SHADERS_NO_GPU` to force that.
- `crates/platform/d3d/tests/nv2a_conformance.rs` is the end-to-end conformance
  suite. It renders small synthetic combiner programs, fixed-function
  stages and pipeline states through `GfxBackend` and `Nv2aShaders` into an
  `A8R8G8B8` target and checks known pixel values. It covers per-stage and
  global `c0`/`c1` and the final constants (ARGB order), all eight input
  mappings on positive and negative values, output scale/bias with the
  register clamp, mux, AB/CD dots and blue-to-alpha, the final combiner's
  lerp, `v1+r0` flags, `E·F` and `G`, every fog table mode (an unnamed
  one falls through to exp2) with D3D's default final combiner and fog
  colour order, fog clamping on both sides, `RANGEFOGENABLE` having no
  effect on a program, a definition's own final combiner reading the
  factor, texture formats (`A8R8G8B8`,
  `X8R8G8B8`, `A8B8G8R8`, 16-bit, `L8`, `A8L8`, DXT1, P8), `COLORSIGN`
  (the hardware ramp exactly, and filtering across 0x7F/0x80),
  `BLENDCOLOR` with constant blend factors, `TFACTOR` and `D3DCOLOR` vertex
  diffuse, and `FRONTFACE`/`CULLMODE` with two-sided lighting.
- `tests/corpus.rs` is optional. With `H2_SHADER_CORPUS` set it checks the
  directory against its manifest, validates the disc's shaders and runs
  every vertex program on the GPU against the interpreter. On the retail
  corpus that is 248,512 comparisons with no magnitude cut-off, all
  matching.

## Gaps

- **Point sprites.** `oPts` is computed, but WGSL has no point size. Halo 2
  writes it in 2 programs, which need quad expansion in the renderer.
  [xemu](https://github.com/xemu-project/xemu/blob/master/hw/xbox/nv2a/pgraph/glsl/vsh.c)
  forwards `oPts.x` to the host point-size output.
- **dot_zw depth replacement and brdf.** See the table above. Neither
  appears in Halo 2's tags.
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
- **Colour key, convolution filters and texture border adjustment** are
  sampler and texture state outside the PSDEF. They're left to the renderer
  and not implemented. No retail material sets `COLORKEYOP` (all 809,233
  resolved stage states are 0).
- **Shadow maps** (see "Shadow maps" above) read mip level 0 only, treat
  `BORDER` addressing as clamp, and choose PCF when either the min or the
  mag filter is linear, since there is no LOD to choose between them: with
  mixed filters (one point, one linear) a stage always gets PCF, whether
  the map is minified or magnified. Keeping both filters and choosing by
  derivatives (with a fallback for compute) is follow-up work. The
  float formats' decoding follows xemu's conversions and is not checked
  on hardware. F24 words with a zero exponent are IEEE denormals, which
  the GPU may flush to zero. The float formats' PROJECT3D clamp is their
  largest value (gfx's `D3DZ_MAX`), where xemu clamps any 24-bit format
  to 16777215.
  The weights are exact f32; the NV2A's filter-weight precision isn't
  modelled. Only PROJECT2D and PROJECT3D compare.
- **Signed blend equations.** `BLENDOP = ADDSIGNED/REVSUBTRACTSIGNED` have
  no wgpu equivalent and fall back to the unsigned ones. Retail materials
  only use `ADD`.
- **Blue-to-alpha against an alpha-portion write.** When a stage both
  copies AB/CD blue to alpha and writes the same register from its alpha
  portion, the alpha portion wins here (xemu's order). This is unverified on
  hardware, and no retail definition does it.
- **Bump-env channel and sign conventions** rely on `COLORSIGN` on the
  source stage (see "Signed texture formats"). Bump-env from a source
  without it reads du/dv unsigned, in [0, 1]. Whether the hardware does
  the same hasn't been checked.
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
- xemu, same commit: `glsl/psh.c` `psh_append_shadowmap` (shadow-map
  comparison, PROJECT2D against 0, PROJECT3D against clamped `r/q`,
  NEVER/ALWAYS), `nv2a_regs.h` (`NV097_SET_SHADOW_DEPTH_FUNC` values),
  `pgraph/util.h` `convert_f16_to_float`/`convert_f24_to_float` (float
  depth words).
- abaire's nxdk_pgraph_tests, `src/tests/texture_shadow_comparator_tests.cpp`
  (main branch, read 2026-11-15), and the hardware results in
  <https://github.com/abaire/nxdk_pgraph_tests_golden_results>
  (`results/Texture_shadow_comparator/3R24_0-FFFFFF_7FFFFF_GT.png`,
  `..._LT.png`, `2R24_0-FFFFFF_7FFFFF_GT.png`); nxdk's `pbkit/nv_regs.h`
  for its `NV097_SET_SHADOW_COMPARE_FUNC_*` names.
- Cass Everitt, Ashu Rege and Cem Cebenoyan, "Hardware Shadow Mapping"
  (NVIDIA whitepaper): GeForce3 compares the four neighbouring texels and
  bilinearly filters the results under linear filtering.
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
