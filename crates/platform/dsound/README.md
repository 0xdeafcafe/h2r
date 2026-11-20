# dsound

The guest-facing DirectSound for Halo 2. The DirectSound the game links
statically is replaced at the API level: guest calls land on `DirectSound`,
which reads and writes guest memory through the `Host` trait and drives
`audio::Mixer`. Only `register` knows about the runtime crate; the rest is
runtime-neutral.

## Where it sits

Domain `platform`; it depends on `audio`, `wma`, `snapshot` and `runtime`.
`boot`, `game` and `cli` use it.

## Layout

- `src/state.rs`, `src/state/`: the DirectSound object, buffers and streams.
- `src/host.rs`: the `Host`, `GuestMemory` and `EventSink` traits.
- `src/register.rs`, `src/register/`, `src/overrides.rs`: the install into
  the runtime and the override table.
- `src/mixbins.rs`, `src/padroute.rs`, `src/voice_rings.rs`: mixbin
  routing, pad speaker routing and voice rings.
- `src/xwma.rs`: the xWMA stream feed to `wma`.

## Features

- `playback` (default): host audio. A dedicated host builds without it and
  only advances DirectSound's guest-visible clock.

## Tests

`cargo nextest run -p dsound`. `tests/xbe_pins.rs`, `tests/register.rs`
and `tests/xwma.rs` have `#[ignore]`d tests that need `H2_XBE` (your
`default.xbe`) and, for one, `H2_ISO` (your disc image).

## Docs

- [docs/audio/dsound.md](../../../docs/audio/dsound.md)
- [docs/audio/audio.md](../../../docs/audio/audio.md)
