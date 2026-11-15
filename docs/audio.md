# Audio

`crates/audio` replaces the Xbox SDK's DirectSound at the API level. The
MCPX APU behind it is not emulated: there are no voice slots, no DSP
programs, no scatter-gather DMA. A game call like `SetVolume` becomes a field
write on a Rust struct, and a software mixer renders the result.

## Layout

| Module | Role |
|---|---|
| `adpcm` | Xbox ADPCM decoder, pure functions. |
| `format` | `WaveFormat`/`Codec`; decodes PCM8, PCM16, Xbox ADPCM to interleaved `f32`. |
| `resample` | Linear resampling (whole clip, streaming). |
| `dsound` | `SoundBuffer` (Lock/Unlock, Play/Stop, loop region, position), `Stream` (packets), `MixIn` (an FX-send bus), `Listener`/`Source3d`/`Spatial`. |
| `route` | Xbox mix bins folded to stereo (`dest`, `channel_of`); the table is below. |
| `mixer` | `Mixer::render(&mut [f32])`: stereo f32 at the host rate. No device knowledge. |
| `sink` | Device-independent output plumbing: lock-free SPSC `ring`, `AudioSink` trait, headless `NullSink`, `Pump`. |
| `output` | The only platform code: cpal streams (`device` feature, on by default). `DeviceSink` is the ring-fed one; the older `Output` (mixer behind a `try_lock`) remains for the tone example. |

`cargo run --release --example tone -p audio` plays a PCM tone and a
synthesised ADPCM clip. Port to tvOS or another host by disabling `device`
and calling `Mixer::render` from the platform callback (cpal also supports
iOS/tvOS CoreAudio, untested here).

## Host output (what `boot` calls)

The game thread owns the `Mixer`. Mixed audio crosses to the device through
a lock-free single-producer/single-consumer ring of interleaved stereo `f32`
(`sink::ring`, power-of-two atomics, no locks or allocation on the device
side). The device callback only drains the ring.

```rust
use audio::{AudioSink, Mixer, Pump, output::DeviceSink};

let mut sink = DeviceSink::open_default()?;          // CoreAudio via cpal
let mut mixer = Mixer::new(sink.sample_rate());
let mut pump = Pump::new(audio::sink::DEFAULT_TARGET_LATENCY_MS); // 40
// ... create/control voices on `mixer` from the DirectSound shim ...
loop {                                               // once per game frame
    pump.pump(&mut mixer, &mut sink);                // renders up to the target
}
// sink.stats() -> SinkStats { underruns, underrun_frames, played_frames }
// sink.latency() -> LatencyReport { ring_ms, device_ms, total_ms, .. }
```

- **Underruns.** If the ring runs dry the callback emits silence for the
  shortfall (never garbage, never blocks) and counts one underrun plus the
  missing frames. Before the first write nothing is counted, so a slow start
  is not an underrun. Counters are cumulative across device reopens.
- **Latency target.** `Pump` keeps `target_latency_ms` of audio queued
  (default 40 ms) and renders only the shortfall each call, so queued audio
  is the latency the game adds. Call `pump` at least once per target period.
  The callback period must fit inside the target (see measurements).
- **Sample rate and device change.** The sink opens the system default output
  at its native rate and the mixer renders at that rate (voices resample
  per render). A stream error or a change of the default device id (polled
  every 500 ms in `maintain`, which `pump` calls) rebuilds the stream; if it
  has a new rate `Pump` calls `Mixer::set_sample_rate` before rendering, and
  the old ring (a few ms) is dropped. With no device, reopening is retried
  once a second and `pump` keeps going into a ring nobody drains, so the game
  does not stall. `DeviceSink::reopen_count()` shows how often this happened.
  The reopen path is logic-tested through `NullSink::set_rate`; a real hot
  switch of output hardware has not been exercised.
- **Headless.** `NullSink::new(rate, capacity)` has no device: tests call
  `advance(frames)` to play the ring like a callback would, with optional
  capture of everything played. CI and the tests use it.
- **Latency measurement.** `cargo run --release --example latency -p audio --
  [target_ms] [seconds]` pumps a tone at 60 Hz and prints mean/peak ring
  fill, the driver-reported callback-to-DAC delay (cpal output timestamps),
  their sum, and underruns. Measured numbers are in the table below.

Measured on an Apple Silicon Mac with the built-in speakers (44.1 kHz, 4 s,
60 Hz pump, ~512-frame/11.6 ms callbacks, driver delay 14.8 ms):

| target | ring mean | total (ring + driver) | underruns |
|---|---|---|---|
| 40 ms | 36.0 ms | 50.8 ms | 1 (284 frames, at start) |
| 20 ms | 16.2 ms | 31.0 ms | 111 |
| 10 ms | 6.9 ms | 21.6 ms | 345 |

So 40 ms is the lowest comfortable target with a 60 Hz feeder: a target
below the callback period plus one game frame (~28 ms here) starves. To go
lower, feed from a thread that wakes per device callback. The single underrun at 40 ms is
at startup (probably a callback landing between the first and second pump;
not confirmed).

Units match the SDK so a guest shim stays a thin mapping: volume and pan in
millibels (100 mB = 1 dB, amplitude `10^(mb/2000)`, `-10000` is silent),
frequency in Hz, buffer positions in bytes at the API and frames internally.

## Formats

Xbox DirectSound takes `WAVE_FORMAT_PCM` (8-bit unsigned, 16-bit signed LE)
and `WAVE_FORMAT_XBOX_ADPCM` (0x0069), mono or stereo. All decode to f32 on
`Unlock`, and the mixer resamples per voice by linear interpolation from
`frequency / host_rate`.

### Xbox ADPCM

IMA ADPCM in MS block layout with a fixed block size. Per channel, 36 bytes:
`i16 predictor`, `u8 step index`, `u8 reserved`, then 32 data bytes (64
nibbles, low nibble first). Stereo blocks are 72 bytes: both channel headers
first, then eight groups of 4 bytes for ch0 followed by 4 bytes for ch1.
Step and index tables are the standard IMA ones.

Sources: [xboxdevwiki, Xbox ADPCM](https://xboxdevwiki.net/Xbox_ADPCM) and
the [Multimedia Wiki, Microsoft IMA ADPCM](https://wiki.multimedia.cx/index.php/Microsoft_IMA_ADPCM).
No code was copied from GPL decoders. (The xboxdevwiki page was unreachable
while writing this; see the block-size note below for how that was settled.)

**Samples per block: 64.** A generic IMA reading of the layout gives 65
(header predictor plus 64 nibbles), and xboxdevwiki describes that expansion.
Xbox ADPCM yields 64 per channel per block: the header predictor plus 63
nibbles, final high nibble discarded. An independent review confirmed this
against vgmstream `ima_decoder.c` and xemu `hw/xbox/mcpx/apu/vp/vp.c`
(pinned at 4bdad199). `SamplesPerBlock::Ima65` remains only as a non-Xbox
comparison mode. Confirmed against real Halo 2 data and the engine's own
format builder; see [sound.md](sound.md).

### Tests

The ADPCM tests contain a tiny IMA encoder producing Xbox blocks and
round-trip synthetic mono and stereo signals. They also include a block
decoded by hand (predictor 1000, nibbles 7 then F, expected 1011 then 981)
and a layout test that proves the 4-byte stereo interleave. No game data is
involved.

## DirectSound model notes

- `SoundBuffer::lock` returns a guard with up to two regions (wrap-around,
  as DirectSound); dropping it is `Unlock` and re-decodes. Offsets and sizes
  must be block-aligned. A non-looping buffer that reaches its end stops and
  rewinds to 0. A Pcm8 buffer starts at 0x80 so it is silent.
- `Stream` is XMO-like: `submit` queues a packet, the mixer consumes in
  order, `take_completed` yields `(PacketId, Success | Flushed)`. Starvation
  plays silence. `flush` completes everything as `Flushed`.
- Not modelled: `SetPlayRegion`, `SetBufferData` hand-off semantics, the
  notification (`SetNotificationPositions`) events, `GetStatus` flags beyond
  playing, `SetHeadroom`, `SetI3DL2Source`, pitch envelopes, `SetFilter`,
  LFO, and `DSSTREAMCAPS` details.

### Mix bins and mix-ins

A voice with `VoiceParams::mix_bins = Some(pairs)` is routed by its
`(bin, volume mB)` pairs instead of the plain stereo path (`None`, kept for
voices nobody routed). `route` folds the 32 MCPX bins to stereo:

| bins | goes to |
|---|---|
| 0, 4 / 1, 5 | left / right |
| 2 (centre) | both at -3 dB |
| 3 (LFE), 10 (I3DL2 send), 31+ | dropped |
| 6, 8 / 7, 9 (crosstalk front/back) | left / right; a 3D voice is panned here, front pairs weighted by `Spatial::front`, back by `1 - front` |
| 11..=30 (FX sends) | every playing `MixIn` whose input bin it is; else dropped |

A `MixIn` (`Mixer::add_mix_in`) is a bus: it sums what is sent to its input
bin and then, like a voice, applies its own `VoiceParams` (volume, mix bins,
3D source). That is how Halo 2 positions sound: its sound core makes a 3D
mix-in buffer per emitter, streams send into its bin, and `SetPosition` moves
the mix-in (`docs/dsound.md`, "Positioned sound"). `render` mixes buses in
dependency order; a mix-in sending to one already mixed this period (a cycle)
is dropped, in id order, rather than delayed.

Channels: a mono voice puts its one channel on every pair; a stereo voice
puts left on even pairs and right on odd ones (`{0, 1}`, `{0, 1, 4, 5}`).

### Positional model (`Listener::spatial`)

Documented and deliberately simple; it is not the MCPX HRTF.

- **Pan**: `p` is the source direction's dot with the listener's right axis
  (`front × top`), in -1..1. Equal-power, narrowed by `PAN_WIDTH = 0.8` so a
  hard-side source keeps a little of the far channel:
  `t = (0.8·p + 1)·π/4`, `left = min(1, √2·cos t)`, `right = min(1, √2·sin t)`
  (both 1 at the centre).
- **Front/back**: `front = (1 + z)/2`, z the direction's dot with the
  listener's front; it only splits the crosstalk front/back pairs, which fold
  to the same side.
- **Distance**: DirectSound3D inverse distance in world units,
  `min / (min + rolloff·(d − min))` for `d > min`, `d` capped at the max
  distance (beyond it the level holds), `rolloff = listener × source` rolloff
  factors. Halo 2 sets min and max to `FLT_MAX` and rolloff 0, and attenuates
  through `SetVolume` itself, so its mix-ins come out at unity.
- **Cone**: unity inside half the inner angle, the outside volume (mB)
  beyond half the outer angle, linear in mB between; 360° (the default)
  disables it.
- **Head-relative** sources take their position as the direction itself.
- Not modelled: doppler (factors stored), distance factor (stored),
  occlusion/obstruction, HRTF filtering, reverb.

## Unsupported: the MCPX DSP

The retail APU had a Motorola 56300 "GP" DSP running Microsoft's default
effect image and the SDK's effect images. These are skipped:

- **SFX/EP processing**: I3DL2 reverb, reverb sends, and effects on mix bins.
  Halo 2 uses reverb, so environments will sound dry. This is the largest
  audible gap.
- **3D/HRTF**: the DSP implemented positional audio with HRTF and XTalk.
  We pan, attenuate and apply cones with the simple model above; HRTF
  filtering, doppler and occlusion are missing.
- **Dolby Digital / DTS encoding** for the S/PDIF output and 5.1 mixing:
  output is stereo.
- **WMA**: not a DSP feature, but noted here: the game's own codec is
  replaced at its six entries by `crates/wma` (`dsound::xwma`), with the
  game's XMO wrapper still running around it; see [wma.md](wma.md),
  "Runtime boundary".
- Voice limits and the hardware's fixed-point rounding. Our mix is f32.

## How others handled it

Both projects replace the API layer, with the APU emulated or bypassed:

- **Cxbx-Reloaded** implements `DirectSound` in its HLE layer
  (`src/core/hle/DSOUND`): it creates a PC DirectSound buffer per guest
  buffer, and decodes Xbox ADPCM in software (the PC driver cannot).
  Effects/3D are largely stubbed. <https://github.com/Cxbx-Reloaded/Cxbx-Reloaded>
- **xemu** emulates the MCPX APU instead (`hw/xbox/mcpx/apu`): voices, the
  scatter-gather memory, hardware ADPCM, and a mixbuf, with the DSP emulated
  (`dsp/`) so SDK effects run for real; this is accurate and heavy.
  <https://github.com/xemu-project/xemu>

For this project the API-level path is Cxbx's, so the stubs above are
expected; xemu is the reference for what the effects should sound like and
is the trace source for validating the mixer later.

## Next steps

1. (Done: 64 confirmed, docs/sound.md.)
2. A guest-facing shim crate mapping `IDirectSound*` vtable calls onto this
   model (owned by the runtime).
3. Notifications and play regions, then reverb (a simple FDN as an I3DL2
   stand-in).
4. Replace linear resampling with a windowed-sinc or polyphase path for big
   downsamples.
5. (Done: the ring-fed `DeviceSink` removes the callback lock, and
   `dsound::Hle::spawn_output` is the one consumer that renders into it; see
   `docs/dsound.md`, "Consumers and the output thread".)
