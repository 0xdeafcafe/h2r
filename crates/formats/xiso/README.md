# xiso

A reader for the Xbox DVD filesystem (XDVDFS). It handles XISO images
(game partition only) and full Redump-style images, where the game
partition starts at a fixed offset after the video partition. It also
reads the disc's map packs.

## Where it sits

Domain `formats`; no workspace dependencies. Everything that reads the
owner's disc uses it: `vfs`, `recomp`, `boot`, `cli`, `tags` and `map`
tests, the converters and the editors.

## Layout

- `src/lib.rs`: `Xiso<R>`: `open`, `entries`, `find`, `read`, `copy_to`,
  and `subset_matches` for path filters.
- `src/mappack.rs`: reading map packs.

## Tests

`cargo nextest run -p xiso`. A test in `src/mappack.rs` is `#[ignore]`d and
needs a disc image.

## Docs

- [docs/game/map-pack.md](../../../docs/game/map-pack.md)
- [docs/runtime/storage.md](../../../docs/runtime/storage.md)
