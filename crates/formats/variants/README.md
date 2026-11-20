# variants

Halo 2 game variants for the editor suite: the 0x130-byte variant record,
the settings schema read from the disc, saved-variant files and the save
store, a portable sharing format, and the editor state (`VariantEditor`:
UI-free state and commands that `h2-editor` draws).

## Where it sits

Domain `formats`; it depends on `content`, `map`, `tags` and `xbe`.
`editor-core`, `hopper-edit`, `studio`, `boot`, `game` and `h1objects` use
it.

## Layout

- `src/record.rs`: `GameVariant`, `GameType`, `SettingId`.
- `src/schema.rs`: the settings schema.
- `src/save.rs`, `src/portable.rs`: saved-variant files and the shared format.
- `src/panel.rs`: `VariantEditor`.
- `src/ext.rs`, `src/extra.rs`, `src/sha1.rs`: extensions, extra fields, hashing.

## Tests

`cargo nextest run -p variants`. `tests/disc.rs` and tests in
`src/record.rs` and `src/extra.rs` are `#[ignore]`d: they need `H2_ISO` or
`H2_XBE` for your own disc, and `H2_SAVED_VARIANT` for a variant the game
saved (`cargo test -p variants -- --ignored`).

## Docs

- [docs/editor/variants.md](../../../docs/editor/variants.md)
- [docs/game/game-options.md](../../../docs/game/game-options.md)
