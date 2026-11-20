# snapshot

Machine snapshots: the versioned file container, the compressed guest RAM
page codec, the runtime's own sections and the on-disk store. It is the
foundation `state.save` and `state.load` build on. It knows nothing of the
kernel, devices or the game: each subsystem adds its own tagged section
with `SnapshotWriter::section` and reads it back with `Snapshot::require`.

## Where it sits

Domain `runtime`; it depends on `runtime`. `kernel`, `d3d`, `dsound`,
`audio`, `wma`, `xapi`, `boot`, `game` and `agent` write sections.

## Layout

- `src/container.rs`: the file format, writer and reader.
- `src/pages.rs`, `src/codec.rs`: RAM page diffing and compression
  (`Base`, `PageStats`).
- `src/guest.rs`: the runtime's own sections.
- `src/store.rs`: `Store`, the on-disk store.
- `src/features.rs`: the feature set a snapshot records.

## Tests

`cargo nextest run -p snapshot`. `tests/machine.rs` round-trips a machine.

## Docs

- [docs/runtime/snapshots.md](../../../docs/runtime/snapshots.md)
