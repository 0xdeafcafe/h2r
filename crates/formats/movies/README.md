# movies

The record of the intro, attract and credits movies after transcoding.
The disc's movies are Bink 1 and we carry no decoder: `h2 movies` runs the
user's own ffmpeg to convert them. This crate describes what that wrote
(`movies.json` beside the files), installs it resumably into app data, and
lets a player find the outputs and notice when they are stale. It decodes
nothing.

## Where it sits

Domain `formats`; it depends on `xiso`. `bink-hle` plays what it finds, and
`boot`, `game` and `cli` install and look up movies.

## Layout

- `src/install.rs`: the resumable installer.
- `src/status.rs`: lookup and staleness (`disc_movies`, `DiscMovie`).
- `src/sha256.rs`: hashing for the manifest.

## Tests

`cargo nextest run -p movies`. The tests use a stub ffmpeg and temporary directories. `H2_FFMPEG` names the
ffmpeg binary the installer runs.

## Docs

- [docs/audio/movies.md](../../../docs/audio/movies.md)
