# shaders

Two independent translators from Xbox (NV2A) shader programs to WGSL:
`vsh` turns vertex microcode into a `@vertex` entry point, and `psh` turns
a `D3DPIXELSHADERDEF` (texture-stage modes plus register combiners) into a
`@fragment` one. Each has a disassembler and a CPU interpreter used as a
reference in tests.

## Where it sits

Domain `formats`; no workspace dependencies. `d3d` and `hot-render` use it
at run time, `native-render` and `h1convert` use it for their own
shaders, and `boot`, `cli` and `game` reach it for tooling.

## Layout

- `src/vsh/`: `Program`, `disasm`, `interp`, `wgsl`.
- `src/psh/`: `PixelShaderDef`, `bits`, `disasm`, `interp`, `fog`,
  `shadow`, `wgsl`.
- `tests/`: golden WGSL, a corpus test (`corpus.rs`), and GPU render
  checks (`gpu.rs`, `render.rs`).

## Tests

`cargo nextest run -p shaders`. `H2_SHADER_CORPUS` names a corpus
directory for the corpus tests, `SHADERS_NO_GPU` skips the GPU checks, and
`SHADOW_GOLDEN_BLESS` rewrites the shadow goldens.

## Docs

- [docs/graphics/shaders.md](../../../docs/graphics/shaders.md)
- [docs/graphics/graphics.md](../../../docs/graphics/graphics.md)
