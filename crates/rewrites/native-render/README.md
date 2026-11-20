# native-render

The native GPU renderer: the static world and the objects, lights, shadows and effects that follow, drawn from a `FrameScene` with wgpu instead of through the lifted Direct3D calls.

## Where it sits

`rewrites` domain, guest-free. Depends on `frame-scene`, `scene-extract` and, with `gpu`, `map`, `tags`, `shaders`, `gfx`, `d3d` and `wgpu`. Used by `render-bridge`, `game` and `boot`.

## Layout

- `src/lib.rs`: the stages and their re-exports.
- `src/world.rs`, `src/load.rs`, `src/cull.rs`, `src/camera.rs`: the static world, loaded once and culled.
- `src/model*.rs`, `src/object_*.rs`: object models and their programs.
- `src/*_gpu.rs`, `src/*.wgsl`: the wgpu passes and shaders (fog, decals, particles, shadows, water, UI).
- `src/light*.rs`, `src/look.rs`: lighting.

## Features

- `gpu` (default): the map loader and the wgpu passes. Without it only the pure parts (world, camera, cull, skip) build.
- `differential`: differential tests of the object lighting against the lifted original.

## Tests

`cargo nextest run -p native-render`. The GPU tests (`tests/*_gpu.rs`) need a wgpu adapter. With `differential`, `tests/model_rows_differential.rs` and `tests/object_lighting_differential.rs` read the retail executable from `H2_XBE` and take `H2_DIFF_SEEDS`.

## Docs

- [graphics/native-renderer.md](../../../docs/graphics/native-renderer.md)
- [graphics/framescene.md](../../../docs/graphics/framescene.md)
- [graphics/native-lights.md](../../../docs/graphics/native-lights.md)
- [graphics/native-decorators.md](../../../docs/graphics/native-decorators.md)
