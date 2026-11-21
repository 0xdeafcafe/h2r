# content

Map packages: a map is a base plus layers. The crate holds the package
manifest, `MapId`, the PPF, IPS and BPS patch readers, the list files for
maps and campaigns, and the resolver that turns a package into the bytes
of `maps\<name>.map`.

## Where it sits

Domain `formats`; it depends on `map`. `vfs` serves resolved maps to the
guest, `variants` and the editors read packages, and `boot`, `cli`,
`studio`, the converters and the Live crates use it.

## Layout

- `src/manifest.rs`, `src/id.rs`, `src/canonical.rs`: the manifest and the
  identity of a map (SHA-256 of its canonical JSON).
- `src/patch/`, `src/patch_lvl.rs`: patch readers; offset patches become
  overlays laid over the base as it streams.
- `src/forge.rs`: the format 2 document identity and load checks.
- `src/store.rs`, `src/resolve.rs`: the packages under `<data>/maps/` and
  the resolver.
- `src/map_pack.rs`, `src/map_list.rs`, `src/campaign_list.rs`: map packs
  and the list files.

## Tests

`cargo nextest run -p content`. One test in `src/resolve.rs` is `#[ignore]`d
and needs a disc.

## Docs

- [docs/game/maps.md](../../../docs/game/maps.md)
- [docs/game/map-pack.md](../../../docs/game/map-pack.md)
