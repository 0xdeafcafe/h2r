# hud-kit

HUD elements of ours drawn by Halo 2's own HUD: a feature describes its HUD as kit elements (texts, meters, icons, pointers, the message, prompt, training and navpoint channels, and marks lying on the level's geometry), the kit compiles them to the game's `nhdt` widget records, and an offline preview draws them over the retail player HUD with the checks a layout must pass.

## Where it sits

`tools` domain. Depends on `hud-kit-core` (the model, the HUDs, the template resolver and the record compiler, shared with the game's runtime), `map`, `tags`, `assets`, `xiso`, `xbe`, `hud-edit`, `scenario-edit`, `h1map` and `h1-assets`. Used by `cli` (`h2 hud`).

## Layout

- `src/rects.rs`: the retail rectangles an element is placed beside, and `above`.
- `src/hudg.rs`: the `hudg` fields the kit reads: flash records, message fields, training references, the waypoint arrows and marker.
- `src/templates.rs`: `hud_kit_core`'s resolver over the HUD editor's model and renderer, with the `hudg`'s arrows.
- `src/compile.rs`: `hud_kit_core`'s compiled records as a `hud_edit` HUD, for the preview.
- `src/source.rs`, `src/synthetic.rs`: the disc and the invented map the preview draws from, and the retail state words the sheet draws at.
- `src/preview.rs`: the 640 by 480 preview, its PNG and JSON, and the sheet; world marks are drawn from each state's sample scene.
- `src/check.rs`: the eleven checks.

## Tests

`cargo nextest run -p hud-kit`. The tests in `tests/disc.rs` are `#[ignore]`d and need the disc (`H2_DISC`).

## Docs

- [game/hud-kit.md](../../../docs/game/hud-kit.md)
- [editor/hud-editor.md](../../../docs/editor/hud-editor.md)
