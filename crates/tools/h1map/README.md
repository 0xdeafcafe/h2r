# h1map

A reader for Halo 1 Xbox cache files (cache version 5): the header, the tag index, the tag data and the structure BSPs, plus the multiplayer placements and BSP collision.

## Where it sits

`tools` domain. Depends on `xiso` and `xbe`. Used by `h1-assets`, `h1convert`, `h1objects`, `h1campaign`, `hud-edit`, `editor-core`, `studio` and `cli`.

## Layout

- `src/reader.rs`, `src/format.rs`: `H1Map::from_reader` and the file layout.
- `src/scenario.rs`: scenario tag and multiplayer placements.
- `src/bsp.rs`: structure BSP regions and collision.
- `src/bitmap.rs`: bitmap tag access.
- `src/disc.rs`: finding Halo 1 maps on a disc image.
- `src/testing.rs`: synthetic maps for tests.

## Tests

`cargo nextest run -p h1map`. Disc tests are `#[ignore]`d and read `H1_ISO` (or `H1_MAP_DIR` for loose maps); run them with `-- --ignored`.

## Docs

- [halo1/map-format.md](../../../docs/halo1/map-format.md)
- [halo1/multiplayer.md](../../../docs/halo1/multiplayer.md)
- [halo1/campaign.md](../../../docs/halo1/campaign.md)
