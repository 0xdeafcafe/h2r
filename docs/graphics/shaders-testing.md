# Shaders: tests and sources

Status: Built. Tests of `crates/formats/shaders` and the sources its semantics were checked against; part of [shaders.md](shaders.md).
Code: `crates/formats/shaders/tests/`, `crates/platform/d3d/tests/nv2a_conformance.rs`, `crates/platform/d3d/tests/shader_corpus.rs`.
Switch: none. Test environment: `SHADERS_NO_GPU` (forces the GPU tests to pass trivially) and `H2_SHADER_CORPUS` (corpus directory, listed in [switches.md](../runtime/switches.md)).

| Test file | What it proves | Needs |
|---|---|---|
| `tests/vsh.rs`, `tests/psh.rs` | hand-assembled words, encoder round trips, golden disassembly, interpreter formulas, naga validation and MSL | nothing |
| `tests/gpu.rs` | interpreter against GPU: random vertex programs, random combiner programs, fog modes, boundary cases | GPU adapter |
| `tests/shadow.rs` | shadow-map comparison, decoding, PCF, addressing, hard option | GPU adapter for the GPU half |
| `tests/render.rs` | a linked module draws real pixels, screen row 0 is the top row, z at `Zmax` is not clipped | GPU adapter |
| `tests/corpus.rs` | the extracted retail corpus: manifest, validation, GPU against interpreter | `H2_SHADER_CORPUS` |
| `d3d/tests/nv2a_conformance.rs` | synthetic programs through `GfxBackend` and `Nv2aShaders` into an `A8R8G8B8` target | GPU adapter |
| `d3d/tests/shader_corpus.rs` (ignored) | every corpus shader drawn through `GfxBackend` | `H2_SHADER_CORPUS` |

## Unit and interpreter tests

- `tests/vsh.rs` and `tests/psh.rs` assemble instruction words by hand from the field table (independent of the encoder) and check the interpreters against hand-worked values: the transform plus viewport fixup, paired-unit rules, `r12`, relative addressing, op formulas, combiner mappings, mux, blue-to-alpha, final combiner, fog modes, clip planes, alpha test, and the address maths of every texture mode through an echoing sampler.
- Every generated shader, including all texture-mode chains and option combinations, goes through naga validation and the naga MSL backend.
- `tests/vsh.rs` and `tests/gpu.rs` share a table of RCC, EXP and LOG boundary cases (±0, saturation both ways, infinities, just below a power of two). The expected values come from the spec, not the interpreter, and both must reproduce them. `tests/common::close` accepts a non-finite value only on an exact match.

## GPU tests

- **Random vertex programs.** `tests/gpu.rs` runs 120 programs × 32 vertices through a compute harness and compares every finite output with the interpreter at a relative tolerance, however large. Non-finite expected values are tallied separately ([shaders.md](shaders.md), "Gaps").
- **Random combiner programs.** 200 × 32 fragments with real RGBA8 textures, nearest filtering and repeat addressing (2D 4×4, 3D 2×2×2, cube 2×2 per face), with a CPU twin as the interpreter's sampler. The generator covers every supported texture mode (bump-env, dependent, dot_st/str, reflection chains) plus random dot mappings, input selectors, bump matrices, eye vectors and alpha kill. The harness samples at LOD 0 (`PixelOptions::explicit_lod`), since compute shaders have no derivatives. Lookups within rounding of a texel or cube-face boundary are skipped and counted; every other fragment matches, and each mode is compared on at least 32 fragments.
- **Fog.** `fog_unit_modes_on_gpu` runs every fog mode, off, and vertex fog over coordinates on both sides of their ranges, and compares the fog register with the interpreter, which `tests/psh.rs` checks against D3D's closed forms.
- **No adapter.** The GPU tests pass trivially when none is present. `SHADERS_NO_GPU` forces that.

## Shadow tests

`tests/shadow.rs` uses synthetic raw-bit depth textures. It checks every comparison function with D24, D16, F24 and F16, in PROJECT2D and PROJECT3D, against stored values below, equal to and above the reference. It also checks `r/q` clamping, exact decoding (neighbouring 24-bit values that any depth filtering would blur), 2x2 PCF weights, and wrap, mirror and clamp addressing. NEVER and ALWAYS must not sample.

The expected values come from the ordering alone, and the interpreter and the GPU must both match exactly (461 fragments on the GPU). The hard option has golden WGSL for retail and for E3 (`tests/golden/shadow_retail.wgsl`, `shadow_e3.wgsl`; retail's text must not change), one tap on both, and stages of one map comparing at their mean coordinate.

## Conformance through the backend

`crates/platform/d3d/tests/nv2a_conformance.rs` renders small synthetic combiner programs, fixed-function stages and pipeline states through `GfxBackend` and `Nv2aShaders` and checks known pixel values. It covers:

- per-stage and global `c0`/`c1` and the final constants (ARGB order); all eight input mappings on positive and negative values; output scale/bias with the register clamp; mux, AB/CD dots and blue-to-alpha; the final combiner's lerp, `v1+r0` flags, `E·F` and `G`.
- every fog table mode (an unnamed one falls through to exp2) with D3D's default final combiner and fog colour order, fog clamping on both sides, `RANGEFOGENABLE` having no effect on a program, and a definition's own final combiner reading the factor.
- texture formats (`A8R8G8B8`, `X8R8G8B8`, `A8B8G8R8`, 16-bit, `L8`, `A8L8`, DXT1, P8), `COLORSIGN` (the hardware ramp exactly, and filtering across 0x7F/0x80), `BLENDCOLOR` with constant blend factors, `TFACTOR` and `D3DCOLOR` vertex diffuse, and `FRONTFACE`/`CULLMODE` with two-sided lighting.

## Corpus tests

`tests/corpus.rs` is optional. With `H2_SHADER_CORPUS` set it checks the directory against its manifest, validates the disc's shaders and runs every vertex program on the GPU against the interpreter, with no magnitude cut-off. How to extract the corpus: [notes/shaders-corpus.md](notes/shaders-corpus.md). The directory belongs in `$TMPDIR`; no game data goes in the repo.

## Sources

The bit layouts and semantics were checked against these, read for facts only. No code was copied (xemu and Cxbx-Reloaded are GPL).

| Source | Used for |
|---|---|
| xemu, commit `4bdad199102b22ed0cfa998e9af6b35d76238b3e` (<https://github.com/xemu-project/xemu>) | `glsl/vsh-prog.c` (microcode field table, paired-unit rules, op formulas, ARL bias), `glsl/vsh.c` (output defaults, `clampAwayZeroInf`, 1/16 screen-coordinate truncation, clip-space conversion), `glsl/psh.c` and `psh_regs.h` (combiner encodings, evaluation order, texture modes, dot mappings), `vsh_regs.h` (opcode and mux enums), `psh_append_shadowmap`, `nv2a_regs.h` (`NV097_SET_SHADOW_DEPTH_FUNC`), `pgraph/util.h` (`convert_f16_to_float`, `convert_f24_to_float`) |
| Cxbx-Reloaded, commit `585c49a50af1255ab155099e06f24505f9c5a800` (<https://github.com/Cxbx-Reloaded/Cxbx-Reloaded>) | `XbD3D8Types.h` (`X_D3DPIXELSHADERDEF` field order, `X_VSH_SHADER_HEADER` and the `0x2078`/`0x7778`/`0x7378` versions), `CxbxVertexShaderTemplate.hlsl` (the screen-space epilogue and its reversal) |
| abaire's nxdk_pgraph_tests (<https://github.com/abaire/nxdk_pgraph_tests>), main branch read 2026-11-15, and the hardware results in <https://github.com/abaire/nxdk_pgraph_tests_golden_results> | `texture_shadow_comparator_tests.cpp` (`results/Texture_shadow_comparator/3R24_0-FFFFFF_7FFFFF_GT.png`, `..._LT.png`, `2R24_0-FFFFFF_7FFFFF_GT.png`); nxdk's `pbkit/nv_regs.h` for `NV097_SET_SHADOW_COMPARE_FUNC_*`; `Texture_signed_component_tests` |
| abaire's nxdk_pgraph_tests, commit `23714459529bdd6bc8587f358625485ba3eabfef` | `src/tests/combiner_tests.cpp` `TestSpecularR0SumFlags` (lines 376 to 475; the cases are around line 425): the final combiner's `SRC_SPEC_R0_SUM` (V1R0_SUM) flags and negative-r0 cases |
| Everitt, Rege and Cebenoyan, "Hardware Shadow Mapping" (NVIDIA whitepaper) | GeForce3 compares the four neighbouring texels and bilinearly filters the results under linear filtering |
| NVIDIA `NV_register_combiners` and `NV_vertex_program1_1` OpenGL extension specs | the general combiner and vertex-program model; the final combiner's V1R0 operand mapping and the EXP/LOG infinity cases |

## Related

- [shaders.md](shaders.md), [shaders-pixel.md](shaders-pixel.md), [shaders-shadow-maps.md](shaders-shadow-maps.md).
