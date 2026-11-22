# map

The reader and writer for Halo 2 retail Xbox (cache v8) `.map` files. It
opens a file, a seekable stream or a bounded region of a disc, loads only
indexes and strings eagerly, and reads tag payloads on demand. It carries
no game data and no unsafe code.

## Where it sits

Domain `formats`; no workspace dependencies. It is the base of most of the
workspace: `tags`, `content`, `hs`, `pathfind`, `mopp`, `asf`, `assets`,
the converters, the editors, `game`, `agent` and `boot` use it.

## Layout

- `src/reader.rs`: `Map::open`, `Map::from_reader`, `Map::from_region`.
- `src/format.rs`: header, index and tag layout types.
- `src/writer.rs`, `src/build.rs`: write a map, and build one from blocks.
- `src/checksum.rs`: the map checksum.
- `src/recording.rs`: a recording reader that logs which ranges were read.
- `src/scenario_layout.rs`: the placement schema shared by `scenario-edit` and `forge-doc`.

## Tests

`cargo nextest run -p map`.

## Docs

- [docs/formats/map-format.md](../../../docs/formats/map-format.md):
  provenance, addressing rules and limits.
- [docs/formats/map-builder.md](../../../docs/formats/map-builder.md)
