# scenario-edit

A Sapien-style scenario placement editor for Halo 2 cache files: typed `scnr` placement blocks, undoable edits written as patched `.map` files, a UI-free session and an offscreen wgpu viewport.

## Where it sits

`tools` domain. Depends on `map`, `tags`, `assets`, `content` and `xiso`. Used by `h1convert`, `h1campaign`, `hud-edit`, `game`, `studio` and `cli`.

## Layout

- `src/layout.rs`: the `scnr` placement blocks, strides and fields.
- `src/scenario.rs`, `src/edit.rs`: typed get/set and the minimal patches.
- `src/session/`, `src/session.rs`: the UI-free editor session.
- `src/save.rs`: writing the patched `.map` through `map::writer::MapEditor`.
- `src/script.rs`, `src/recipe/`: JSON edit scripts and test-map recipes.
- `src/render.rs`, `src/scene.rs`, `src/geometry.rs`: the offscreen viewport.

## Tests

`cargo nextest run -p scenario-edit`. Six tests are `#[ignore]`d and need the disc (`H2_DISC`).

## Docs

- [editor/scenario-editor.md](../../../docs/editor/scenario-editor.md)
- [editor/editor.md](../../../docs/editor/editor.md)
- [game/test-maps.md](../../../docs/game/test-maps.md)
