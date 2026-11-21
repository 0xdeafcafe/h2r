# game

Our additions to the game: menus and settings screens, skulls, variants, the campaign picker, the glue between the game and the platform and rewrite crates, and the gameplay hooks. It is the largest crate and the place a new game feature goes.

## Where it sits

`extensions`. Uses most domains below it: `hooks`, `engine`, `runtime`, `kernel`, `d3d`, `dsound`, `sim-bridge`, `render-bridge`, `coop`, `bots`, `live-hle`, `agent` and others. Used by `boot`, which re-exports it.

## Layout

- `src/lib.rs`: the feature list and `RunOpts`.
- Screens: `src/*_screen.rs`, `src/skulls*.rs`, `src/campaign_*`, `src/mods_screen/`, `src/pause_menu/`, `src/theater_ui/`.
- Glue to a subsystem: `src/*_glue.rs` (`d3d`, `dsound`, `xapi`, `hot`, `live`, `lockstep`, `sim`, `snapshot`, `voice` and the `native_*` renderer layers).
- Halo 1 in the game: `src/h1_*`.
- Co-op, bots and hosting: `src/coop_*`, `src/bot_glue/`, `src/host_lobby/`.
- `src/test_maps/`: maps built from recipes for `maps.test.*`.

## Features

- `default` = `gpu`, `movie-playback`, `local-io`, `test-maps`.
- `desktop-runtime`: the desktop pieces of `recomp` and `render-bridge`.
- `gpu`: wgpu back ends and the native renderer glue.
- `movie-playback`: Bink movie playback.
- `local-io`: host audio and voice.
- `perf`: the sampling profiler.
- `test-maps`: `maps.test.*`; the recipes come from the caller (`test_maps::Recipes`, which `boot` implements over the scenario editor and the Firefight kit).

## Tests

`cargo nextest run -p game`. Tests marked `#[ignore]` (campaign cards, the level boot, variant pages) read the disc through `H2_DISC`.

## Docs

- [settings-menu.md](../../../docs/game/settings-menu.md)
- [feature-engine.md](../../../docs/engine/feature-engine.md)
- [crate-structure.md](../../../docs/dev/crate-structure.md)
- [test-maps.md](../../../docs/game/test-maps.md)
