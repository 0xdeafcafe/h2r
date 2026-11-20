# debug-menu

The game's in-game debug menu: a tree of rows over the agent API's registries, its navigation, touchpad and pad input, and the lines the host overlay draws. The retail game has no such menu.

## Where it sits

`extensions`. Uses `input`. Used by `app`, `agent`, `boot`, `cli` and `game`.

## Layout

- `src/model.rs`: rows, sections and the navigation state machine.
- `src/sources.rs`: the sections (Render, Shader overrides, Tag edits, Cheats, Perf, Features), each over agent methods.
- `src/gate.rs`: the touchpad and Options chord, gestures, and hiding pads from the game while the menu is open.
- `src/layout.rs`: the lines the overlay draws.

## Tests

`cargo nextest run -p debug-menu`.

## Docs

- [debug-menu.md](../../../docs/runtime/debug-menu.md)
- [agent-api.md](../../../docs/runtime/agent-api.md)
