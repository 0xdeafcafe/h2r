# bink-hle

Runtime overrides for the statically linked Bink 1.x player in Halo 2.
Skip mode replaces the engine's movie-start wrapper with a stub that starts
nothing. With the `playback` feature, a movie source selects playback
instead: the host decodes the user's transcoded movie and the override
feeds frames and audio into the player's state.

## Where it sits

Domain `platform`; it depends on `audio`, `movies` and `runtime`. `boot`
and `game` register it.

## Layout

- `src/addr.rs`: the engine addresses it hooks.
- `src/select.rs`: picks skip or playback and the platform decoder.
- `src/playback.rs`: `BinkHle`, the playback override.
- `src/source.rs`, `src/audio_out.rs`: movie sources and audio output
  through the mixer.
- `src/avfoundation.rs`: the macOS decoder.

## Features

- `playback` (default): movie playback; pulls in `movies` and `audio`.

## Tests

`cargo nextest run -p bink-hle`. `tests/xbe_pins.rs` and one test in
`tests/synthetic.rs` are `#[ignore]`d: they check the hooked addresses
against the retail binary and need `H2_XBE` set to your own `default.xbe`
(`cargo test -p bink-hle -- --ignored`).

## Docs

- [docs/audio/bink-hle.md](../../../docs/audio/bink-hle.md)
- [docs/audio/movies.md](../../../docs/audio/movies.md)
