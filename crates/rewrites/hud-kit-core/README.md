# hud-kit-core

The HUD kit's shared core: the element model (`Hud`, its elements and their states), the templates a compiled element takes from the loaded map and their resolution, the compiler to `nhdt` widget record bytes, and the HUDs of ours (superbounce). The offline preview (`hud-kit`) and the game's runtime (`game::hud_kit`) both use it, so the bytes the preview checks are the bytes the game is given.

## Where it sits

`rewrites` domain, guest-free. Depends on `map`, `tags` and `assets`. Used by `hud-kit`, `hud-edit` (its `Mode`) and `game`.

## Layout

- `src/model.rs`: the `Hud`, its elements and their states.
- `src/templates.rs`: what a compiled element takes from the map, the grenade row included, resolved through a `MapView`.
- `src/map_file.rs`: a `MapView` over an open map file and its resources (the runtime's).
- `src/records.rs`: elements to widget record bytes.
- `src/world.rs`: world marks, elements that lie on the level's geometry: their shapes, poses and line segments.
- `src/data.rs`: a HUD read from the YAML of a package's `hud` item (`content/<package>/huds/`); `BUILTIN` embeds the first-party ones.
- `src/views/`: the HUDs' states, one module each (superbounce, with its tutorial's step machine; its readout, with the charge segments and the approach guide), the Forge readout, Firefight, and `pill_block`, a HUD of the kit's tests alone. A module's `hud()` is its YAML's.
- `src/expected.rs`, `src/data_tests.rs`: the Rust each HUD's elements were built with, and the test that holds the YAML to it.

## Tests

`cargo nextest run -p hud-kit-core`. No disc needed; `hud-kit`'s disc test compares `MapFile`'s templates with the preview's.

## Docs

- [game/hud-kit.md](../../../docs/game/hud-kit.md)
