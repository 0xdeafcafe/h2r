# audio

The audio foundation: a DirectSound-level model, the Xbox ADPCM decoder
and a software mixer, with host output on top. Nothing here emulates the
MCPX APU; DirectSound is replaced at the API level. `dsound` drives it
from guest calls.

## Where it sits

Domain `platform`; it depends on `snapshot`. `dsound` and `bink-hle` build
on it, and `boot`, `game`, `app`, `h2host`, `h1-assets` and `cli` use it.

## Layout

- `src/adpcm.rs`, `src/format.rs`: the ADPCM decoder, wave formats and
  `WaveFormat`.
- `src/dsound.rs`, `src/mixer.rs`, `src/voice.rs`: buffers, streams and
  the `Mixer`.
- `src/fx.rs`, `src/reverb.rs`, `src/hrtf.rs`, `src/resample.rs`,
  `src/route.rs`: effects, 3D, resampling and mix routing.
- `src/sink.rs`, `src/output.rs`, `src/devices.rs`: the `AudioSink` trait
  and the host device output.
- `src/snap.rs`: snapshot sections.

## Features

- `device` (default): host output through cpal. Without it the core never
  touches a device, so a port can supply its own sink.

## Tests

`cargo nextest run -p audio`. One test lists this machine's audio devices
and is `#[ignore]`d.

## Docs

- [docs/audio/audio.md](../../../docs/audio/audio.md)
- [docs/audio/dsound.md](../../../docs/audio/dsound.md)
