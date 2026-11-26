# film

Seekable Halo 2 engine films, presentation records and the local film library. It is the data side of theater: recording, indexing and reading films.

## Where it sits

`live`. Uses `live-proto`. Used by `boot`, `cli`, `game` and `live-hle`.

## Layout

- `src/tape.rs`, `src/range.rs`: the seekable chunk format and ranges.
- `src/machine.rs`: the machine section of a film.
- `src/file.rs`, `src/columns.rs`, `src/start.rs`: the film file, its tick
  chunks and its game start.
- `src/presentation.rs`: presentation records.
- `src/library.rs`: the local film library.
- `src/upload.rs`, `src/image.rs`: upload and images.
- `src/blam/`: the engine's own film layout.

## Tests

`cargo nextest run -p film`.

## Docs

- [theater.md](../../../docs/game/theater.md)
- [live-api.md](../../../docs/live/live-api.md)
