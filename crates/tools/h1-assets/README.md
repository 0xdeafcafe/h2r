# h1-assets

Halo 1 assets as Halo 2 tags: bitmaps, models, sounds, skies and text, read from Xbox cache files. The converters build their output from it.

## Where it sits

`tools` domain. Depends on `h1map`, `assets`, `gfx`, `tags`, `map`, `audio` and `h1-contract`. Used by `h1convert`, `h1objects`, `h1campaign`, `hud-edit`, `editor-core` and `cli`.

## Layout

- `src/bitmaps/`: bitmap decoding to Halo 2 bitmap tags.
- `src/models/`: Halo 1 models as Halo 2 render models.
- `src/sounds/`: sound banks and looping.
- `src/sky/`, `src/text/`: skies and string tables.

## Tests

`cargo nextest run -p h1-assets`. The `*_disc` and `*_real_disc` tests are `#[ignore]`d and read the Halo 1 disc from `H1_ISO` (some also `H2_DISC`); run them with `-- --ignored`.

## Docs

- [halo1/README.md](../../../docs/halo1/README.md)
- [halo1/bitmaps.md](../../../docs/halo1/bitmaps.md)
- [halo1/models.md](../../../docs/halo1/models.md)
- [halo1/sounds.md](../../../docs/halo1/sounds.md)
- [halo1/sky.md](../../../docs/halo1/sky.md)
- [halo1/text.md](../../../docs/halo1/text.md)
