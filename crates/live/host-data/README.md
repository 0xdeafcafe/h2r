# host-data

The dedicated host's game data: build the owner's bundle from their disc, and fetch, verify and cache it on a host. The bundle is private to its owner and never enters the repository.

## Where it sits

`live`. Uses `map` and `xiso`. Used by `cli`.

## Layout

- `src/bundle.rs`: building the bundle.
- `src/tar.rs`: the archive writer.
- `src/lib.rs`: `Manifest`, `provision` and `provision_with` (fetch, verify by SHA-256, cache).
- `src/cli.rs`: `bundle_cmd`, the bundle command.

## Tests

`cargo nextest run -p host-data`. Tests build a synthetic DVD directory with a `default.xbe` and a few maps with real headers.

## Docs

- [dedicated-host.md](../../../docs/live/dedicated-host.md)
- [dedicated-host-build.md](../../../docs/live/dedicated-host-build.md)
