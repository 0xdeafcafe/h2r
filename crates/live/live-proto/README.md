# live-proto

Wire types, signing and verification shared by the Xbox Live replacement's clients (the game-side HLE, the CLI, the web app's model) and its server. It has no runtime and no I/O, so the recompiler's runtime can link it without tokio.

## Where it sits

`live`. Uses `content`. Used by `live-server`, `live-worker`, `live-hle`, `live-telemetry`, `cloud-saves`, `film`, `bots`, `boot`, `cli`, `agent`, `game`, `h2host`, `studio` and `hopper-edit`.

## Layout

- `src/sign.rs`, `src/jcs.rs`: signing and canonical JSON.
- `src/lib.rs`: the module list; one module per wire area (`session.rs`, `matchmaking.rs`, `maps.rs`, `skulls.rs`, `storage.rs`, `film.rs`, `hosts.rs`, `bans.rs` and more).
- `src/pack.rs`: the packed telemetry frame format (keyframes and deltas, bit-packed); `src/pack_floor.rs` and `src/pack_speed.rs` measure it.
- `src/archive.rs`: the replay segment archive (Brotli, transposed by field).
- `src/motd.rs`, `src/title_file.rs`: title files the game downloads.

## Features

- `live-floor-bench`: measurement hooks in the packed-frame writer, compiled out otherwise.

## Tests

`cargo nextest run -p live-proto`. `tests/live_floor_speed.rs`, `tests/live_floor_profile.rs` and `tests/packed_v8.rs` are `#[ignore]`d release-mode measurements. Golden files regenerate with `H2_UPDATE_GOLDEN`.

## Docs

- [live-api.md](../../../docs/live/live-api.md)
- [live-security.md](../../../docs/live/live-security.md)
- [live-custom-games.md](../../../docs/live/live-custom-games.md)
