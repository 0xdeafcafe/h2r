# debug-menu

The game's in-game debug menu: a tree of rows over the agent API's registries, its navigation, touchpad and pad input, and the lines the host overlay draws. The retail game has no such menu.

## Where it sits

`extensions`. Uses `input`. Used by `app`, `agent`, `boot`, `cli` and `game`.

## Layout

- `src/model.rs`: rows, sections and the navigation state machine.
- `src/sources/`: the sections, one file each (`player.rs`, `world.rs`, `game.rs`, `time.rs`, `view.rs`, `perf.rs`, `settings.rs`, `dev.rs`) over agent methods, the status `header.rs`, the shared cheat rows (`cheats.rs`) and a fake game for their tests (`fake.rs`).
- `src/gate.rs`: the touchpad and Options chord, gestures, and hiding pads from the game while the menu is open.
- `src/layout.rs`: the lines the overlay draws.

## Tests

`cargo nextest run -p debug-menu`.

## Docs

- [debug-menu.md](../../../docs/runtime/debug-menu.md)
- [agent-api.md](../../../docs/runtime/agent-api.md)
