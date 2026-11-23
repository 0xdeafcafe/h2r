# host-data

The dedicated host's game data: build the owner's bundle from their disc, and fetch, verify and cache it on a host. The bundle is private to its owner and never enters the repository.

## Where it sits

`live`. Uses `bots`, `content`, `map`, `vfs`, `xiso` and, with the `firefight` feature, `firefight-kit`. Used by `cli`.

## Layout

- `src/bundle.rs`: building the bundle.
- `src/bundle/stores.rs`: verifying private shared resource stores against the disc.
- `src/bundle/kits.rs`: each multiplayer map's Firefight kit, built from the disc (`firefight` feature).
- `src/bundle/nav.rs`, `src/bundle/image.rs`: the bots' nav data and the game module.
- `src/tar.rs`: the archive writer.
- `src/lib.rs`: `Manifest`, `provision` and `provision_with` (fetch, verify by SHA-256, cache), and placing a bundle's bot data and Firefight kits in a host's data root (`install_bot_data`, `install_firefight_kit`).
- `src/cli.rs`: `bundle_cmd`, the bundle command.

## Features

- `firefight` (default): `h2 host bundle` builds the Firefight kits. The headless host's build leaves it off; it only places bundled kits.

## Tests

`cargo nextest run -p host-data`. Tests build a synthetic DVD directory with a `default.xbe` and a few maps with real headers.

## Docs

- [dedicated-host.md](../../../docs/live/dedicated-host.md)
- [dedicated-host-build.md](../../../docs/live/dedicated-host-build.md)
