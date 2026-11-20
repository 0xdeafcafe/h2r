# xapi

Guest XAPI service replacements for Halo 2: the Xbox service calls that
cannot run through the generated kernel path. `register` installs them into
a runtime `Dispatch`. They cover controllers and rumble, the cache
partitions and storage, saved games, launch data and guest memory access.

## Where it sits

Domain `platform`; it depends on `input`, `runtime`, `snapshot` and `vfs`.
`boot`, `game`, `app`, `agent`, `lockstep` and `coop` use it.

## Layout

- `src/register.rs`: the install into `Dispatch`.
- `src/controllers.rs`: input, rumble and device-change signals.
- `src/saves.rs`, `src/storage.rs`, `src/vfs_saves.rs`,
  `src/vfs_storage.rs`: saved games and storage over `vfs`.
- `src/cache.rs`, `src/config.rs`: cache partitions and configuration.
- `src/launch.rs`, `src/memory.rs`, `src/snapshot.rs`, `src/tap.rs`.

## Tests

`cargo nextest run -p xapi`. Tests in `tests/xbe_pins.rs`,
`tests/save_pins.rs`, `tests/controllers.rs`, `tests/input_semantics.rs`
and `tests/checkpoints.rs` are `#[ignore]`d and need `H2_XBE` set to your
`default.xbe`; `tests/checkpoints.rs` also needs clang.

## Docs

- [docs/runtime/xapi.md](../../../docs/runtime/xapi.md)
- [docs/runtime/storage.md](../../../docs/runtime/storage.md)
- [docs/runtime/checkpoints.md](../../../docs/runtime/checkpoints.md)
