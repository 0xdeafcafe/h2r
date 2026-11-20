# guest-layout

Typed guest layouts: every Halo 2 structure offset and global address, written once, with the guest-memory boundary, typed views and the named points. Code reads `unit.player(m)` instead of adding an offset.

## Where it sits

`hooking`. Uses `runtime` (behind a feature). Used by `engine`, `agent`, `live-telemetry`, `sim-objects` and `sim-state`.

## Layout

- `src/macros.rs`: the `layout!` and `record!` macros.
- `src/memory.rs`, `src/mem.rs`, `src/testing.rs`: the `Mem` boundary and a test memory.
- `src/datum.rs`, `src/datum_array.rs`, `src/handle.rs`: datum arrays and handles.
- `src/object.rs`, `src/unit.rs`, `src/player.rs`, `src/game.rs`, `src/tag.rs`, `src/tags.rs`: the structure layouts.
- `src/global.rs`, `src/points.rs`: the global address table and named guest functions.

## Features

- `runtime-glue`: pulls in `runtime` so the memory boundary is implemented over its guest; `engine` turns it on.

## Tests

`cargo nextest run -p guest-layout`. `tests/raw_offsets.rs` and the unit tests use synthetic memory and need no disc.

## Docs

- [guest-layout.md](../../../docs/engine/guest-layout.md)
- [feature-engine.md](../../../docs/engine/feature-engine.md)
