# boot

The composition root. It runs a built module on the runtime with the kernel HLE attached, plans every feature that `game` holds onto the hooks, and runs the guest's entry thread until something stops it. Every other crate is wired to the game here.

## Where it sits

`apps`. Depends on nearly every domain: `recomp`, `runtime`, `kernel`, `hooks`, `engine`, `game`, `dsound`, `live-hle` and the rest of the stack. Used by `cli`, `h2host` and `studio`.

## Layout

- `src/lib.rs`: `run`, `run_hosted`, `run_with_live`, `run_hosted_with_live`, `run_hosted_with_publisher`, `run_sim` and `run_theater`, one per way to start the game. `game` is re-exported whole.
- `src/registration.rs`, `src/registration/`: the feature plan that attaches every feature to the hooks.
- `src/test_maps_recipes.rs`: `game`'s `Recipes` over the scenario editor and the Firefight kit, for `maps.test.*` (`test-maps` feature).
- `tests/`: headless runs of the guest (interpreter, hosted, live, sign-in, audio, render points).
- `examples/`: small drivers.

## Features

- `default` = `gpu`, `movie-playback`, `local-io`, `test-maps`.
- `desktop-runtime`: the desktop runtime pieces of `recomp`, `render-bridge` and `game`.
- `gpu`: the wgpu back ends, shader overrides and the native renderer.
- `movie-playback`: Bink movie decode and playback.
- `local-io`: host audio and voice.
- `perf`: the in-process sampling profiler.
- `test-maps`: `maps.test.*` on the agent API; `src/test_maps_recipes.rs` builds placement rigs with the scenario editor and the AI probe with the Firefight kit.

## Tests

`cargo nextest run -p boot`. These tests read the retail XBE and are `#[ignore]`d without it: `audio_idle`, `content_sign`, `crt_copy`, `health_xbe`, `render_points_xbe` and `hosted` take `H2_XBE` (some also `H2_DISC`); run them with `--run-ignored all`.

## Docs

- [BOOT.md](../../../docs/runtime/BOOT.md)
- [feature-engine.md](../../../docs/engine/feature-engine.md)
- [crate-structure.md](../../../docs/dev/crate-structure.md)
- [build-speed.md](../../../docs/dev/build-speed.md)
