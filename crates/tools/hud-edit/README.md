# hud-edit

The HUD editor's UI-free logic: a map's `nhdt` HUDs and their widgets, the engine's placement rules, a preview canvas, layout checks and edits with undo, saved as a patched map.

## Where it sits

`tools` domain. Depends on `map`, `tags`, `assets`, `font`, `scenario-edit`, `h1map` and `h1-assets`. Used by `studio`.

## Layout

- `src/layout.rs`: the widget bytes and the field table.
- `src/place.rs`, `src/state.rs`: the engine's placement rules and preview state.
- `src/model.rs`: a map's HUDs and their widgets.
- `src/render.rs`: the 640 by 480 preview.
- `src/checks.rs`: overlap, off-screen and text-spill checks.
- `src/session.rs`: edits, undo and redo, the patched-map save.

## Features

- `fixture`: fixture helpers that use `h1map` and `h1-assets`.

## Tests

`cargo nextest run -p hud-edit`. One test is `#[ignore]`d and needs the disc (`H2_DISC`).

## Docs

- [editor/hud-editor.md](../../../docs/editor/hud-editor.md)
- [editor/editor.md](../../../docs/editor/editor.md)
