# voice

The communicator under the title's own voice engine. On an Xbox a headset
is two USB media objects (XMOs) per port: a microphone object that fills
640-byte 16 kHz PCM packets and a headphone object that plays them. The
engine's codec and voice detection are title code and run recompiled; this
crate supplies the two objects over host capture and playback.

## Where it sits

Domain `platform`; it depends on `runtime`. `boot`, `game` and `cli` use
it.

## Layout

- `src/xmo.rs`: the media objects.
- `src/hle.rs`: the guest-facing hooks that create them.
- `src/host.rs`, `src/io.rs`: host capture and playback.
- `src/convert.rs`, `src/resample.rs`: sample conversion.

## Features

- `device`: host microphone and speaker through cpal.

## Tests

`cargo nextest run -p voice`.

## Docs

- [docs/live/live-voice.md](../../../docs/live/live-voice.md)
