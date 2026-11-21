# hud-kit

HUD elements of ours drawn by Halo 2's own HUD: a feature describes its HUD as kit elements (texts, meters, icons, pointers, and the message, prompt, training and navpoint channels), the kit compiles them to the game's `nhdt` widget records, and an offline preview draws them over the retail player HUD with the checks a layout must pass.

## Where it sits

`tools` domain. Depends on `map`, `tags`, `assets`, `xiso`, `xbe`, `hud-edit`, `scenario-edit`, `h1map` and `h1-assets`. Used by `cli` (`h2 hud`).

## Layout

- `src/model.rs`: the `Hud`, its elements and their states.
- `src/rects.rs`: the retail rectangles an element is placed beside.
- `src/hudg.rs`: the `hudg` fields the kit reads: flash records, message fields, training references, the waypoint arrows and marker.
- `src/templates.rs`: what a compiled element takes from the loaded map, the grenade row included.
- `src/compile.rs`: elements to widget record bytes.
- `src/source.rs`, `src/synthetic.rs`: the disc and the invented map the preview draws from.
- `src/preview.rs`: the 640 by 480 preview, its PNG and JSON, and the sheet.
- `src/check.rs`: the ten checks.
- `src/huds/`: the HUDs the kit knows (superbounce).

## Tests

`cargo nextest run -p hud-kit`. The tests in `tests/disc.rs` are `#[ignore]`d and need the disc (`H2_DISC`).

## Docs

- [game/hud-kit.md](../../../docs/game/hud-kit.md)
- [editor/hud-editor.md](../../../docs/editor/hud-editor.md)
