# render-bridge

The guest side of the native renderer: the stage hooks, admissions and the frame step that builds the scene. It is the `rewrites` crate that uses `hooking` for rendering.

## Where it sits

`rewrites` domain. Depends on `hooks`, `runtime`, `engine`, `tags` and, with `native-render`, `native-render`, `frame-scene` and `scene-extract`. Used by `game`, `agent`, `h1-runtime` and `boot`.

## Layout

- `src/lib.rs`: the install entry point.
- `src/native_*.rs`, `src/native_*/`: one admission and hook set per stage (world, objects, lights, shadows, particles, HUD).
- `src/scene_step.rs`: the frame step's scene.
- `src/guest_entries.rs`: the guest D3D entries, each a named panic.
- `src/frame_wait.rs`: the vblank source and the engine frame wait.
- `src/producers.rs`: the guest render producers the cuts key by.
- `src/render_timing.rs`: diagnostics.

## Features

- `native-render` (default): builds the renderer crates and the hooks that use them.

## Tests

`cargo nextest run -p render-bridge`. Some tests read the retail executable from `H2_XBE`.

## Docs

- [graphics/framescene.md](../../../docs/graphics/framescene.md)
- [graphics/native-renderer.md](../../../docs/graphics/native-renderer.md)
- [engine/feature-engine.md](../../../docs/engine/feature-engine.md)
