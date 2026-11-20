# d3d

The guest-facing Direct3D 8 (Xbox, XDK 5849 LTCG) for Halo 2, at the API
level. It does not emulate the NV2A. D3D entry points the game calls become
`Device` methods that read guest structures and render through a `Backend`
(`GfxBackend` on `gfx`). Halo is an LTCG build, so the game also writes
render-state arrays and push-buffer words itself; the device reads those at
draw time.

## Where it sits

Domain `platform`; it depends on `gfx`, `shaders`, `hot-render`, `timeline`,
`snapshot` and `runtime`. `boot`, `game`, `h2host`, `uwp-host`, `perf` and
`native-render` use it.

## Layout

- `src/device/`, `src/device.rs`, `src/state.rs`: the `Device` and its state.
- `src/pushbuf.rs`, `src/nv2a.rs`, `src/nv2a_draw.rs`, `src/regs.rs`:
  push-buffer decode and the register model.
- `src/backend.rs`, `src/gfx_backend/`: the `Backend` trait and its
  wgpu implementation.
- `src/shader.rs`, `src/pipeline_store.rs`: program translation and the
  pipeline cache.
- `src/scaled.rs`, `src/scene_stage.rs`, `src/threaded.rs`: render
  scale, FrameScene staging and the render thread.
- `src/runtime_glue.rs`: installs the overrides into the runtime.

## Features

- `gpu` (default): the wgpu back end. Without it the guest device and the
  software recorder build with no GPU or window system.

## Tests

`cargo nextest run -p d3d`. The test files build as one binary, `tests/it.rs`
([docs/dev/build-speed.md](../../../docs/dev/build-speed.md)). Tests that
check retail addresses (`xbe_tables`, `fog_xbe`) need `H2_XBE` set to your
`default.xbe`; the benchmarks are `#[ignore]`d. `H2_SHADER_CORPUS` and
`SHADERS_NO_GPU` select the shader corpus and skip GPU checks.

## Docs

- [docs/graphics/d3d.md](../../../docs/graphics/d3d.md)
- [docs/graphics/graphics.md](../../../docs/graphics/graphics.md)
- [docs/graphics/render-thread.md](../../../docs/graphics/render-thread.md)
- [docs/graphics/framescene.md](../../../docs/graphics/framescene.md)
