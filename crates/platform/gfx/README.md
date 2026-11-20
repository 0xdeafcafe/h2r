# gfx

A Direct3D 8 (Xbox flavour) replacement on wgpu: texture decoding, the
state model, and the presenter. It has three layers: Xbox data handling
that needs no GPU (formats, swizzle, DXT, texture layout), the wgpu
renderer and pipelines, and the presenter.

## Where it sits

Domain `platform`; it depends only on `timeline`. `d3d` builds on it, and
`assets`, `viewer`, `native-render`, `perf`, `h1-assets`, `boot`, `app`
and `game` use it.

## Layout

- `src/format.rs`, `src/swizzle.rs`, `src/dxt.rs`, `src/convert.rs`,
  `src/encode.rs`: the CPU-only data handling.
- `src/texture.rs`, `src/vertex.rs`, `src/state.rs`, `src/xdk.rs`: guest
  texture, vertex and state types.
- `src/renderer.rs`, `src/pipeline.rs`, `src/program.rs`, `src/compile.rs`:
  the wgpu renderer and the pipeline compile.
- `src/surface.rs`, `src/surface_alias.rs`, `src/depth_fill.rs`,
  `src/occlusion.rs`: render targets, aliasing, depth and visibility.
- `src/presenter.rs`, `src/resize.rs`, `src/readback.rs`: presenting,
  scaling and readback.

## Features

- `gpu` (default): wgpu and the renderer. Without it only the CPU data
  handling builds.

## Tests

`cargo nextest run -p gfx`. `examples/window.rs` opens a window. The timing tests (`tests/staging_cost.rs`,
`tests/pass_cost.rs`) are `#[ignore]`d.

## Docs

- [docs/graphics/graphics.md](../../../docs/graphics/graphics.md)
- [docs/graphics/shaders.md](../../../docs/graphics/shaders.md)
- [docs/graphics/high-resolution.md](../../../docs/graphics/high-resolution.md)
