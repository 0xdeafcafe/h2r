# Shaders: the pixel side

Status: Built. Reference for `shaders::psh` in `crates/formats/shaders`; part of [shaders.md](shaders.md).
Code: `crates/formats/shaders/src/psh/` (`bits`, `interp`, `wgsl`, `shadow`). Tests: `crates/formats/shaders/tests/psh.rs`, `gpu.rs`, `shadow.rs`.
Switch: none.

`D3DPIXELSHADERDEF` is the NV2A's texture-stage and register-combiner program. `shaders::psh` decodes it, lists it, interprets it and translates it to a WGSL `@fragment`. Counts from the retail corpus are in [notes/shaders-corpus.md](notes/shaders-corpus.md).

| Stage | What it does | Where |
|---|---|---|
| Texture modes (stages 0 to 3) | fetch, project, bump, dot-product, reflect | "Texture modes" |
| General combiners (up to 8) | RGB and alpha: AB, CD, sum or mux | "Combiners" |
| Final combiner | `A·B + (1−A)·C + D`, alpha `G` | "Final combiner" |
| Fog unit | the factor read as `fog.a` | [shaders-fog.md](shaders-fog.md) |
| Shadow stages | depth-format textures compare, not sample | [shaders-shadow-maps.md](shaders-shadow-maps.md) |

## Combiners

The struct is 60 dwords in SDK order, mirroring the NV2A register file: `PSAlphaInputs[8]`, `PSFinalCombinerInputsABCD`/`EFG`, `PSConstant0[8]`, `PSConstant1[8]`, `PSAlphaOutputs[8]`, `PSRGBInputs[8]`, `PSCompareMode`, `PSFinalCombinerConstant0`/`1`, `PSRGBOutputs[8]`, `PSCombinerCount`, `PSTextureModes`, `PSDotMapping`, `PSInputTexture`, `PSC0Mapping`, `PSC1Mapping`, `PSFinalCombinerConstants`. `psh::bits` has the `PS_*` encodings.

| Part | Encoding |
|---|---|
| Input | one byte per input, A in the top byte. Bits 0 to 3: zero, c0, c1, fog, v0, v1, t0 to t3, r0, r1, and (final combiner only) `v1+r0`, `E·F`. Bit 4: alpha (RGB portion `.aaa`; alpha portion clear = blue, set = alpha). Bits 5 to 7: mapping |
| Input mapping | unsigned identity `max(0,x)`, invert `1−clamp(x)`, expand `2max(0,x)−1`, expand-negate, half-bias `max(0,x)−½`, half-bias-negate, signed `x`, signed-negate `−x` |
| Output | CD bits 0 to 3, AB bits 4 to 7, sum bits 8 to 11 |
| Output flags | 12 CD dot, 13 AB dot, 14 mux instead of sum, 15 to 17 mapping (identity, −½, ×2, (x−½)×2, ×4, ÷2), 18 CD blue-to-alpha, 19 AB blue-to-alpha |

- **Mapping and clamp.** The output mapping applies to each product and to the sum of the unmapped products. Every result is clamped to [−1, 1].
- **Order** follows xemu. A stage reads all RGB and alpha inputs before any write. RGB writes go first (AB, CD, sum, each followed by its blue-to-alpha copy), then the alpha writes.
- **Mux** picks CD when `r0.a >= 0.5` under `MUX_MSB`, otherwise when the low bit of `u32(r0.a·255)` is set.
- **Constants.** Stage *n* reads `c0[n]` with `UNIQUE_C0` and `c0[0]` without it, and the same for c1. The final combiner reads index 8. `r0.a` starts as t0's alpha, or 1 when stage 0 has no texture. All other registers start at zero, and a texture in mode NONE reads (0, 0, 0, 1).
- **Fog** reads (fog colour rgb, factor) in any combiner.
- **Per-stage state** that arrives through `PixelOptions`: `D3DTSS_ALPHAKILL` (kill when the fetched alpha is 0), `D3DTSS_COLORSIGN`, the alpha test (emulated with `discard` against `alpha_ref` in 0 to 255, since wgpu has none), two-sided lighting (back faces read `oB0`/`oB1`) and flat shading.

### Final combiner

When either inputs dword is non-zero, rgb = A·B + (1−A)·C + D and alpha = G, both clamped to [0, 1]. Without one the translator outputs `clamp(r0)`; the hardware always runs one.

- **The default.** A definition with no final combiner makes Xbox D3D's `SetPixelShader` (`0x3fa3d0`) skip `SET_COMBINER_SPECULAR_FOG_CW0/1` and mark the fog/specular state dirty. The lazy commit (`0x00400000`) then programs the one the fixed-function path uses. With `FOGENABLE`: `A = fog.a`, `B = r0` (`v1+r0` with `SPECULARENABLE`), `C = fog.rgb`. Without: `D = r0` (or `v1+r0`). Always `G = r0.a` and the clamp-sum flag (words `0x130C0300`, `+0x20000` for specular, `0x0C`/`0x0E`, `0x1C80`).
- **In code.** `psh::d3d_default_final_combiner` returns those words, `PixelShaderDef::with_default_final_combiner` fills them in, and `d3d::nv2a` applies it to every guest definition. All 140 retail definitions with general combiners carry their own. The other 9 are variant shells whose combiners the engine builds at run time (always given one: ABCD `0xc`, EFG `0x2000`; [shader-instances.md](../formats/shader-instances.md)), so the default only changes draws whose shader the game builds without one.
- **The latch.** Whether a definition has a final combiner is latched at bind (`0x3fa452`, the OR of its two final dwords into device `+0x788`). Pushing zero to both final-combiner methods later does not clear it, and editing the bound buffer without rebinding changes nothing, because the lazy commit reads the latch. The hardware then runs an all-zero final combiner (RGB 0, alpha 0).
- **Reading the latch.** The device reads `+0x788` at each draw into `ShaderInputs::final_combiner_latch`. `d3d::nv2a::guest_def` sets `PixelShaderDef::explicit_final` from it, and `Nv2aStats::zero_final_draws` counts such draws.
- **`v1+r0` (V1R0_SUM)** unsigned-maps each operand before adding: each is clamped to [0, 1] and inverted (1 − clamp) when its complement flag is set, then the clamp-sum flag clamps the sum. So r0 = −0.5 and v1 = 0.75 give 0.75, not 0.25. This is `NV_register_combiners`' spare0-plus-secondary-colour input, and abaire's `SpecularR0SumFlags` hardware test shows the same: negative r0 adds nothing, inversion applies to the clamped operand, the sum clamps only with the flag (0.5·(1 + 1) reads 1.0 unclamped, 0.5 clamped). Both were read for behaviour only ([shaders-testing.md](shaders-testing.md), "Sources").

## Texture modes

Stages run 0 to 3, and dependent modes read an earlier stage's result: stage 1 reads t0, stages 2 and 3 read `PSInputTexture`. A selector that names no earlier stage (stage 2: t0 to t1, stage 3: t0 to t2) reads zero, with a warning. Only bump, dependent and dot modes read the selector, and only dot modes read the dot mapping, so unused fields can hold anything.

| Mode | Status |
|---|---|
| none, project2d (÷q), project3d (÷q), cubemap, passthru (clamped coords) | supported |
| clipplane (kill per coordinate against `PSCompareMode`) | supported |
| bumpenvmap, bumpenvmap_lum (uv + M·(du,dv); ×clamp(lum·scale + offset)) | supported |
| dotproduct, dot_st, dot_str_3d, dot_str_cube | supported |
| dot_rflct_diff, dot_rflct_spec (eye = q of t1 to t3) | supported |
| dot_rflct_spec_const | supported; eye from `XpsUniforms.eye`, which the d3d provider fills from `NV097_SET_EYE_VECTOR` (`0x181C`, PGRAPH `EYEVEC0..2`), also recorded in `d3d::hw::HwState::eye_vector` |
| dpndnt_ar, dpndnt_gb | supported |
| **dot_zw** | **unsupported.** The dot product is computed but depth is not replaced (no `frag_depth` write) |
| **brdf** | **unsupported.** Reads zero (xemu does not implement it either) |
| dot mappings zero_to_one, minus1_to_1_d3d/_gl/(nv) | supported, for unsigned 8-bit data |
| dot mappings hilo_1, hilo_hemisphere_d3d/_gl/(nv) | supported; the texel is read as a 32-bit word ("HILO dot mappings") |

Halo 2's tags use none of the unsupported entries. A stage that holds a mode it cannot run (dot_st in stage 0, say) is skipped with a warning. `PixelShader::warnings` lists everything approximated, the d3d provider prints it once per translated program (`nv2a: pixel shader <id>: ...`), and `Nv2aShaders::warnings` returns all of it.

- **Dimensions.** `PixelOptions::texture_dims` overrides project2d, project3d and cubemap the way D3D's `TEXMODE_ADJUST` would. Halo 2 never sets that flag.
- **Bump-env.** The generated code reads du/dv as `.rg` and luminance as `.b` of the source stage as the texture unit delivered them. They are signed only through `COLORSIGN` ("Signed texture formats").
- **Texel addressing.** 2D coordinates multiply by `XpsUniforms.tex_scale[n].xy`, which covers linear (texel-addressed) textures.

### HILO dot mappings

A HILO mapping reads its source texel as two 16-bit halves of one 32-bit word. The translator takes the word as an `A8R8G8B8` texture samples it in gfx (a = bits 31..24, r = 23..16, g = 15..8, b = 7..0, the split xemu's `dotmap_hilo_1` uses), so hi = a·256 + r and lo = g·256 + b, each channel rounded from `x·255`.

| Mapping | Vector |
|---|---|
| hilo_1 | (hi/65535, lo/65535, 1) |
| hilo_hemisphere_d3d | (h, l, √max(0, 1 − h² − l²)) with h = (hi − 32768)/32767, likewise l |
| hilo_hemisphere_gl | the same with two's-complement halves, h = (2s + 1)/65535 |
| hilo_hemisphere | the same with two's-complement halves, h = s/32767 |

`hilo_1` is NV_texture_shader's `HILO`-to-`(hi, lo, 1)` rule. The hemisphere variants follow the spec's hemisphere completion with the three signed conventions of the 8-bit mappings. xemu leaves them as `FIXME`, and none has been checked on hardware.

The menu's depth-fog pass is the one user. It binds the depth buffer as a colour texture and dots it, with `hilo_1`, against coordinates that rebuild the halves: a D24S8 word is `depth << 8 | stencil`, so hi is the top 16 bits of depth and lo the low depth byte above the stencil byte. The renderer must present the depth texel's 32-bit word in that `A8R8G8B8` layout (d3d-r8 does).

- **Filtering.** Filtering 8-bit halves is not 16-bit filtering (a carry between r and a is lost), so only point-sampled HILO stages are exact. A format that forces alpha (`X8R8G8B8`) loses the top byte.
- **Tests.** `hilo_dot_mappings_split_the_texel_word` (`tests/psh.rs`) works a D24S8 word and each hemisphere convention by hand. `tests/gpu.rs` draws HILO mappings in the random combiner test and needs at least 32 compared dot stages per mapping.

### Signed texture formats

`D3DFMT_V8U8`, `Q8W8V8U8` and `L6V5U5` are the same NV2A format codes as `G8B8` (0x28, linear 0x17), `A8B8G8R8` (0x3A) and `R6G5B5` (0x27), and `gfx::format::D3DFormat` gives each pair one variant. Nothing in the texture header marks a texel signed, so neither the device nor gfx can decode one of these as signed from the format. The NV2A takes signedness from the texture unit's per-channel sign bits (`D3DTSS_COLORSIGN`). The dot mappings (`minus1_to_1_*`) convert inside the dot product instead.

Halo 2 sets `COLORSIGN` exactly where it needs signed texels: stage 0 of the bump-env materials, RGB signed ([notes/shaders-corpus.md](notes/shaders-corpus.md)). Signed V8U8 data reaches bump-env through the `COLORSIGN` path below, which stores the signed byte `s` as `s + 128` (`gfx::texture::DecodeOptions::signed`) so host filtering stays linear, and converts the filtered value to `2s/255` at the fetch, before bump-env reads `.rg`. A format-based signed decode would be wrong: a `G8B8` texture without `COLORSIGN` reads unsigned on the NV2A too.

### Signed texture channels (`COLORSIGN`)

`D3DTSS_COLORSIGN` sets the texture unit's per-channel sign bits (`D3DTSIGN_ASIGNED` bit 28, `RSIGNED` 29, `GSIGNED` 30, `BSIGNED` 31, the same bits as `SET_TEXTURE_FILTER`). A signed channel is the texel byte read as two's complement `s`, and the unit delivers `2s/255`, clamped at −1:

| Byte | Delivered |
|---|---|
| 0x7F | 254/255 (not 1) |
| 0x40 | 128/255 |
| 0x00 | 0 |
| 0xFF | −2/255 |

The positive side is read from Xbox hardware captures (nxdk_pgraph_tests golden results, `Texture_signed_component_tests`: bytes 64, 100 and 127 render as 128, 200 and 254), not the modern `max(s/127, −1)` SNORM rule. The negative side, 0x80 in particular, follows the same formula and is not checked against hardware.

**Filtering.** Hardware converts each texel before filtering, so a bilinear blend of 0x7F and 0x80 is about 0. To get that from host UNORM filtering, the texture is uploaded with every signed channel's byte XORed with 0x80 (stored as `s + 128`, `gfx::texture::DecodeOptions::signed`, set from `COLORSIGN` by the device; signed DXT takes the CPU decode path). Filtering is then linear in `s`, and the pixel shader turns the filtered value `v` back into `max(2v − 256/255, −1)` right after the fetch, before bump-env, dot products, alpha kill or the combiners see it.

- **Cache key.** The `COLORSIGN` bits are part of the device's texture cache key, so one texture used signed and unsigned is two host textures.
- **Options.** `PixelOptions::color_sign` holds `COLORSIGN >> 28` per stage (`bits::color_sign`). The interpreter's `Sampler` returns host (biased) values and applies the same transfer.
- **Exceptions.** Frame-buffer snapshots bound as textures are not stored biased, so the device clears `COLORSIGN` for those stages. Depth-format shadow maps keep their raw bits and ignore `COLORSIGN`. Colour textures over depth memory use the bias and separate host copies per sign mask, like ordinary textures.
- **Stand-ins.** Textures for unbound stages and dimension mismatches are biased by the stage's sign mask too (`ProgramBindings::dummy`, keyed by it), so they still read black.
- **Borders.** The host's border presets cannot express a biased or arbitrary border, so the shader supplies those colours, and all colours on hosts without border addressing (`PixelOptions::border`). The sampler clamps to the edge on the `BORDER` axes, and a coordinate outside `[0, 1]` there reads `D3DTSS_BORDERCOLOR` as the unit would, signed channels as `2s/255` of the border byte (a guess; 0x00 reads 0 either way). The half texel at the edge where hardware blends with the border reads the edge texel instead.
- **Implicit channels.** The transfer applies to the channel the texel format expands to (an implicit alpha of 1 reads −2/255). This is unverified for anything but 8-bit channels.

## Not verified

- The negative side of the signed transfer, signed borders, and signed transfer on non-8-bit channels.
- HILO hemisphere conventions.
- The final combiner's blue-to-alpha against an alpha-portion write: the alpha portion wins here (xemu's order), and no retail definition does it.
- Bump-env from a source without `COLORSIGN` reads du/dv unsigned in [0, 1]; the hardware's behaviour is not checked.

## Related

- [shaders.md](shaders.md): the index, vertex microcode, resource contract and gaps.
- [shaders-fog.md](shaders-fog.md), [shaders-shadow-maps.md](shaders-shadow-maps.md), [shaders-testing.md](shaders-testing.md).
- [notes/shaders-corpus.md](notes/shaders-corpus.md): retail usage tallies.
