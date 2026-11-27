# Shaders: shadow maps

Status: Built. Reference for `psh::shadow` in `crates/formats/shaders`; part of [shaders.md](shaders.md).
Code: `crates/formats/shaders/src/psh/shadow.rs`, `PixelOptions::shadow`, `hard_shadow`. Tests: `crates/formats/shaders/tests/shadow.rs` (golden files in `tests/golden/`).
Switch: none.

| Input | Source | Effect |
|---|---|---|
| texture in `D24S8`, `D16`, `F24S8` or `F16` (swizzled or linear) | the stage's texture format | the stage is a shadow map |
| `PROJECT2D` | texture mode | compare with 0 |
| `PROJECT3D` | texture mode | compare with `r/q` clamped to `[0, max]` |
| function | `D3DRS_SHADOWFUNC` via `SHADOWCTL` | `stored OP reference` |
| filter, addressing | stage state | point (one texel) or 2x2 PCF; wrap, mirror, clamp on texel indices |

On the NV2A a texture in a depth format is always a shadow map: PROJECT2D and PROJECT3D on it compare the stored depth with a reference and give 1 or 0 in every channel. `PixelOptions::shadow[n]` marks such a stage and carries the format, the function, the filter and the addressing. The behaviour follows xemu's `psh_append_shadowmap`.

## Comparison

- **Function.** `OP` is `SHADOWCTL`'s function, which D3D sets from `D3DRS_SHADOWFUNC` (`SetRenderState_ShadowFunc` pushes `Value - 0x200` to `NV097_SET_SHADOW_DEPTH_FUNC`), so the `D3DCMP_*` value applies as written. `GREATER` (`0x204`, the first campaign level's) passes where the stored depth is greater than the reference.
- **Reference.** PROJECT2D compares with 0. PROJECT3D compares with `r/q` clamped to `[0, max]`: 16777215 for D24, 65535 for D16, 511.9375 for F16 and `(2 - 2^-16) * 2^127` for F24 (the `D3DZ_MAX` values window z uses; xemu uses 16777215 for every 24-bit format).
- **NEVER and ALWAYS** give 0 and 1 without sampling.
- **Hardware check.** abaire's `texture_shadow_comparator_tests.cpp` checks the comparison on hardware. Its function names are the operands swapped: its `GREATER` is register value 1, `LESS` here, and passes where the stored depth is below the reference (`3R24_0-FFFFFF_7FFFFF_GT`: the 0, 1 and `ref - 1` boxes pass, `ref` and above do not). PROJECT2D with the same function passes nowhere (`2R24_..._GT`). Both match xemu and the order used here.

## Depth bits

The renderer uploads depth textures as raw bits in RGBA8 (depth bytes high to low in r, g, b, stencil in a; D16 and F16 in r and g), without any `COLORSIGN` bias. The stored value is rebuilt from `round(c * 255)` exactly.

| Format | Reading |
|---|---|
| D24, D16 | an integer |
| F24 | the 24 bits are an IEEE single's top bits without the sign (8-bit exponent, bias 127, 16-bit mantissa) |
| F16 | 4-bit exponent (bias 7), 12-bit mantissa, no denormals: every word but 0 has the implicit leading one, exponent 0 included |

Both float readings are xemu's (`convert_f16_to_float`, `convert_f24_to_float` in `pgraph/util.h`).

## Filtering

GeForce3-class hardware, which the NV2A is, compares each of the four neighbouring texels and bilinearly weights the 1/0 results when the filter is linear (2x2 percentage-closer filtering; NVIDIA's "Hardware Shadow Mapping" whitepaper). xemu filters the depth values and compares once; this code does not.

The translation fetches texels with `textureLoad`, so the host never filters raw bits. It uses one texel (`floor(uv * size)`) for point filtering, or four around `uv * size - 0.5` weighted by the fraction for linear. Wrap, mirror and clamp addressing apply to the texel indices. `s/q` and `t/q` multiply by `tex_scale`, as for colour textures, so linear (texel-addressed) shadow maps work.

## Hard shadows

`PixelOptions::hard_shadow` (the Lighting row on the D3D path, [e3-lighting-setting.md](e3-lighting-setting.md)) makes a stage with a non-zero mask compare once at the nearest texel, whatever its filter. The coordinate is the mean of the coordinates of the stages in its mask (the stages that sample the same map), so taps spread over stages collapse to one. Only the render-scale view sets it. All zero is the retail code byte for byte (`tests/shadow.rs`, golden WGSL `shadow_retail.wgsl` must not change).

## Limits

- Mip level 0 only, and `BORDER` addressing is treated as clamp.
- PCF is chosen when either the min or the mag filter is linear, since there is no LOD to choose between them. Keeping both filters and choosing by derivatives (with a compute fallback) is open work.
- Weights are exact f32; the NV2A's filter-weight precision is not modelled.
- Only PROJECT2D and PROJECT3D compare.
- F24 words with a zero exponent are IEEE denormals, which the GPU may flush to zero.
- The float formats' PROJECT3D clamp is their largest value (gfx's `D3DZ_MAX`), where xemu clamps any 24-bit format to 16777215.

## Not verified

- The float formats' decoding follows xemu's conversions and is not checked on hardware.

## Related

- [shaders-pixel.md](shaders-pixel.md): the combiner side; [shaders-testing.md](shaders-testing.md): the shadow tests.
- [d3d.md](d3d.md): depth textures and aliases on the device side.
