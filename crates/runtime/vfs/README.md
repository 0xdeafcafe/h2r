# vfs

Xbox object-manager storage, independent of guest memory and kernel ABI
packing: paths, handles and the backends behind them. A backend serves the
disc image or directory, host-written files (with write-behind), map
packages, per-map overrides and the campaign and film maps.

## Where it sits

Domain `runtime`; it depends on `content` and `xiso`. `kernel` and `xapi`
sit on it, and `boot`, `game`, `studio`, `h2host`, `uwp-host`,
`cloud-saves` and `cli` configure it.

## Layout

- `src/backend.rs`, `src/types.rs`, `src/path.rs`, `src/state.rs`: the
  `Backend` trait, handles and path rules.
- `src/disc.rs`, `src/dir_disc.rs`: disc image and directory backends.
- `src/host.rs`, `src/write_behind.rs`, `src/save_meta.rs`: host files,
  deferred writes and save metadata.
- `src/packages.rs`, `src/content_maps.rs`, `src/map_override.rs`,
  `src/campaign_maps.rs`, `src/film_map.rs`, `src/variants.rs`: layered
  maps and packages.
- `src/rewrite.rs`, `src/sim_store.rs`: path rewrites and the sim store.

## Tests

`cargo nextest run -p vfs`. Tests that read a disc are `#[ignore]`d and
need `H2_DISC` (your disc image). `VFS_TEST_NO_HOME` keeps tests out of
your home directory.

## Docs

- [docs/runtime/storage.md](../../../docs/runtime/storage.md)
- [docs/runtime/cache-slots.md](../../../docs/runtime/cache-slots.md)
- [docs/game/maps.md](../../../docs/game/maps.md)
