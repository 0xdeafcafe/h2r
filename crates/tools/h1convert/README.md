# h1convert

Halo 1 multiplayer maps converted to Halo 2 tags: collision, structure BSP and lightmap, shaders and textures, sky, sandbox items and placements.

## Where it sits

`tools` domain. Depends on `h1map`, `h1-assets`, `h1-contract`, `scenario-edit`, `shaders`, `content`, `map`, `tags` and `mopp`. Used by `h1objects`, `h1campaign`, `studio` and `cli`.

## Layout

- `src/collision.rs`, `src/query.rs`, `src/surface_query.rs`: the collision BSP and the queries that check a conversion against its source.
- `src/structure.rs`, `src/clusters.rs`, `src/flood.rs`: the structure BSP, clusters and portals.
- `src/shad.rs`, `src/materials.rs`, `src/textures.rs`, `src/sky.rs`: render data.
- `src/sandbox.rs`, `src/placements.rs`, `src/apply.rs`: items, spawns and writing into a base map.
- `src/convert.rs`, `src/convert/`: the top-level conversion.

## Tests

`cargo nextest run -p h1convert`. Most of `tests/` is `#[ignore]`d and reads the discs (`H1_ISO`, `H2_DISC`, sometimes `H2_XBE`); run them with `-- --ignored`.

## Docs

- [halo1/multiplayer.md](../../../docs/halo1/multiplayer.md)
- [halo1/map-format.md](../../../docs/halo1/map-format.md)
- [halo1/campaign.md](../../../docs/halo1/campaign.md)
- [formats/collision.md](../../../docs/formats/collision.md)
- [formats/map-builder.md](../../../docs/formats/map-builder.md)
