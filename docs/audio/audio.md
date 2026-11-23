# Audio

Status: Built. Code: `crates/platform/audio/src` (`mixer.rs`, `dsound.rs`, `output.rs`). Tests: `crates/platform/audio/tests`.
Switch: none for the mixer; `medal-announcer` for [our medals' announcer lines](#our-medals-announcer-lines).

`crates/platform/audio` replaces the Xbox SDK's DirectSound at the API level. The
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
| `meter` | Peak, RMS and BS.1770-4 loudness of PCM; see "Metering the mixer". |
| `hrtf` | Binaural filtering for positioned mix-ins (on by default). |
| `sink` | Device-independent output plumbing: lock-free SPSC `ring`, `AudioSink` trait, headless `NullSink`, `Pump`. |
| `devices` | Picks an output device by name (the DualSense speaker); no device access. |
| `fx`, `reverb` | Per-voice effects and the mixer's I3DL2 reverb; see "Voice effects". |
| `output` | The only platform code: cpal streams (`device` feature, on by default). `DeviceSink` is the ring-fed one; `Output` (mixer behind a `try_lock`) is used by the tone example. |

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
format builder; see [sound.md](../formats/sound.md).

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
  playing, the pitch/filter envelope generator, envelope release, and
  `DSSTREAMCAPS` details. Filters, LFOs, the amplitude envelope, headroom,
  I3DL2 sources and the reverb are modelled: see "Voice effects" below.

### Mix bins and mix-ins

A voice with `VoiceParams::mix_bins = Some(pairs)` is routed by its
`(bin, volume mB)` pairs instead of the plain stereo path (`None`, kept for
voices nobody routed). `route` folds the 32 MCPX bins to stereo:

| bins | goes to |
|---|---|
| 0, 4 / 1, 5 | left / right |
| 2 (centre) | both at -3 dB |
| 3 (LFE), 31+ | dropped |
| 10 (I3DL2 send) | the mixer's reverb once an environment is set (`Mixer::set_reverb`); else dropped |
| 6, 8 / 7, 9 (crosstalk front/back) | left / right; a 3D voice is panned here, front pairs weighted by `Spatial::front`, back by `1 - front` |
| 11..=30 (FX sends) | every playing `MixIn` whose input bin it is; else dropped |
| 31 (`route::SUBMIX`) | dropped: the guest's "my output buffer" bin, which the shim replaces with the submix's host bin |
| `0x100` and up (host) | the one submix (`Mixer::add_submix`) that owns the bin |

A `MixIn` (`Mixer::add_mix_in`) is a bus: it sums what is sent to its input
bin and then, like a voice, applies its own `VoiceParams` (volume, mix bins,
3D source). That is how Halo 2 positions sound: its sound core makes 3D
submix buffers, each channel's stream is routed into one, and `SetPosition`
moves the submix (`docs/audio/dsound.md`, "Positioned sound"). An FX-send
mix-in is shared: every voice sending to its bin reaches it. A submix
(`Mixer::add_submix`, for a `DSBCAPS_MIXIN` buffer) gets a host bin of its
own, so only the voices given that bin reach it. On the Xbox every submix
had the same bin 31 in its voices' lists, meaning each voice's own output
buffer. `render` mixes buses in
dependency order; a mix-in sending to one already mixed this period (a cycle)
is dropped, in id order, rather than delayed.

Channels: a mono voice puts its one channel on every pair; a stereo voice
puts left on even pairs and right on odd ones (`{0, 1}`, `{0, 1, 4, 5}`).

### Positional model (`Listener::spatial`)

The mixer applies the DirectSound 3D model per voice, against the one
listener, in every render (`crate::dsound`, `Listener::spatial`; the terms
one by one in `Listener::attenuation`). A voice with no 3D source (or in
`Mode3d::Disabled`) is 2D: none of the terms below touch it, wherever the
listener is. The model is a pure function of the source, the listener and
the voice's own clock, so a render is deterministic. The default stereo mix
uses a simple DirectSound panner.

- **Pan**: `p` is the source direction's dot with the listener's right axis
  (`top × front`), in -1..1. Equal-power, narrowed by `PAN_WIDTH = 0.8` so a
  hard-side source keeps a little of the far channel:
  `t = (0.8·p + 1)·π/4`, `left = min(1, √2·cos t)`, `right = min(1, √2·sin t)`
  (both 1 at the centre).
- **Front/back**: `front = (1 + z)/2`, z the direction's dot with the
  listener's front; it only splits the crosstalk front/back pairs, which fold
  to the same side.
- **Distance**: DirectSound3D inverse distance in world units,
  `min / (min + rolloff·(d − min))` for `d > min`, `d` capped at the max
  distance (beyond it the level holds), `rolloff = listener × source` rolloff
  factors. Halo 2 sets min and max to `FLT_MAX` and rolloff 0 on its
  submixes and the listener, and attenuates through voice and mix-bin
  volumes, so its submixes come out at unity
  ([dsound.md](dsound.md#distance-attenuation)).
- **Cone**: unity inside half the inner angle, the outside volume (mB)
  beyond half the outer angle, linear in mB between (the DS3D model). Only
  a 360° inner angle (the default) or a zero orientation disables it; a
  360° outer angle with a narrower inner one fades all the way round to the
  outside volume directly behind.
- **Head-relative** sources take their position as the direction itself.
- **Doppler**: `Spatial::pitch = (c − k·vl)/(c − k·vs)` with `c` = 343 m/s,
  `vl`/`vs` the listener's and source's velocities along the source-to-
  listener direction times the distance factor, and `k` the product of the
  listener's and source's Doppler factors (each clamped to 0..=10; NaN is 0).
  The ratio is limited to 1/8..8. It scales the playback rate of a buffer,
  and the (bounded) read position of a stream, that has its own 3D source (a mix-in has no samples to
  resample, so a positioned mix-in does not shift its inputs). Speeds are
  capped at `0.9 c / k`.

`crates/platform/audio/tests/distance.rs` checks the model on synthetic
voices: the gain at, inside, twice and past the min and max distances for
rolloff 0, 0.5, 1 and 2, the listener's rolloff, Halo's unity parameters,
the cone inside, between and outside, the Doppler sign for a moving source
and a moving listener, the pan, the distance applied in a render with and
without HRTF, a submix hearing only its own voices, and a 2D voice
rendering the same samples whatever the listener does.

### Headphones (3D)

Binaural rendering for headphones is on by default. Launch with
`H2_FEATURES=audio.hrtf=off` to turn it off; the switch is read at launch.
Halo 2 never enabled the MCPX HRTF. With the switch off, the original
panner produces bit-identical output.

HRTF and the optional distance reverb are layers on top of the positional
model: they never replace its distance, cone or Doppler terms.

Each playing, positioned mix-in sums its stream sends to mono, then filters
its dry stereo output using the source direction in the listener's frame.
Halo sets the listener at the origin facing +x and supplies each mix-in's
position relative to its own listener. The binaural stage replaces the
crosstalk pan and preserves the mix-bin levels, including the stream
volumes Halo uses for distance attenuation. The mix-in's distance and cone
gain, volume, headroom and direct-path effects still apply. UI, music and
player sounds that play without a positioned mix-in use their normal
routing. Reverb sends and sends to other mix-ins keep their original
signals.

The spherical head uses a Woodworth interaural delay and a one-pole,
one-zero head-shadow filter for each ear. [Brown & Duda, 1998][brown-duda].
The filter is normalised against its straight-ahead response, so a source
directly in front produces equal channels at the original centre gain.
An elevation-dependent comb adds a dip that shifts with elevation and a
gentle high-frequency shelf darkens rear sources. Each mix-in has one mono
delay line with fractional reads at the mixer rate. Parameters ramp over
10 ms when the direction changes. The model is computed in code without
HRIR files or game data.

[brown-duda]: https://escholarship.org/uc/item/0cr9d23p

### Distance reverb

Extra depth for positioned voices is on by default;
`H2_FEATURES=audio.distance_reverb=off` turns it off. It uses the length of the listener-relative position in metres,
before the DS3D min/max clamp. For distance `d`, the mixer adds
`6 × d/(10+d)` dB to an existing room send and subtracts
`12 × d/(10+d)` dB from the direct HF level. At ten metres this gives
+3 dB wet and −6 dB at the HF reference. These are host tuning choices.
Unpositioned streams keep their game mix. When
a stream feeds a positioned mix-in, the cue applies there. Both switches can
be turned off with `H2_FEATURES=audio.hrtf=off,audio.distance_reverb=off`.
The direct HF filter runs before binaural rendering. Bin 10 feeds the room
reverb through its own filter.

### Voice effects

`crate::fx` (per-voice) and `crate::reverb` (one per mixer). Every effect is
a function of the voice's own *clock*, the number of frames it has produced
since it (re)started, so output does not depend on render block sizes and a
starved stream does not advance its envelope. Effects run after the voice is
rendered at unity gain and before routing; none changes how many frames a
voice consumes, so packet completion is untouched (the pitch LFO and Doppler
change playback rate by design).

- **Filter** (`SetFilter`, `FilterSpec::from_dsfilter`): up to two cascaded
  RBJ biquads (low, high, band, notch, peaking) at the host rate. Hardware
  coefficient words decode as `4096·log2(2 sin(π f / 48000))` (cutoff),
  `32768 / Q` (damping), `8192·(10^(dB/20) − 1)` (parametric gain).
- **LFO** (`SetLFO`): two per voice, triangle, delay in 1/1500 s, rate
  `delta·1500/65536` Hz, depths for pitch, cutoff and amplitude.
- **Envelope** (`SetEG`): amplitude delay, attack, hold, decay, sustain in
  512-sample (1/93.75 s) units; release is not entered because stops and
  flushes are immediate.
- `Mixer::reserve_block(frames)` reserves the reverb and direct-path working
  memory so an idle render with a reverb allocates nothing (`tests/alloc.rs`).
- **Headroom** (`SetHeadroom`): attenuates the voice by its millibels.
- **I3DL2 source** (`SetI3DL2Source`): direct-path level, obstruction and
  occlusion use each effect's LF ratio to split its loss between broadband
  attenuation and HF filtering. Bin 10 has its own filter state. It hears
  room, roomHF and occlusion. Obstruction and directHF affect the direct
  output. Each one-pole response reaches the requested loss at the
  environment's HF reference (5 kHz by default). Both paths advance only
  over frames actually produced. The send retains its own bin volume.
  For a positioned source the model applies the inverse distance law above
  to the room path using the sum of source and environment room rolloff.
  Halo's positioned mix-ins stay inside their minimum distance.
  Streams keep 514 frames of completed-packet history for pitch-modulated
  reads that reach behind the front packet.
- **Reverb** (`Reverb`, `I3dl2Listener`): early-reflection taps, a
  four-line feedback delay network with per-line decay for the requested
  RT60, HF decay ratio via an in-loop one-pole, diffusion allpasses, room
  HF low-pass. See the module docs for the exact structure.

`docs/audio/dsound.md`, "Voice effects", has the descriptor layouts and which
parts are assumptions.

## The announcer queue

Read for this page (E, `h2 disasm 0x19e770 70`, `0x23ef80 40`,
`0x23f000 70`, `0x23f0a0 60`). Names: `points::ANNOUNCER_QUEUE_UPDATE`
and `points::announcer_queue` (`crates/hooking/guest-layout/src/points.rs`).

Halo 2 speaks one announcer line at a time, each after the last has
finished. A game event's sound ("Double kill", [hud-kit.md](../game/hud-kit.md#game-events-and-medals-0x0019de80))
goes from `0x0019e770` to `0x0023ef80` with a delay in ticks (the
response's `+0x3c` seconds). `0x0023ef80` checks a table of the last four
events first (`0x0023f180`, not read in detail), then queues the sound
tag through `0x0023f140`.

- **The queue.** Five 12-byte lines from `0x00502350`: the sound tag
  (`+0`), the sound playing (`+4`), a playing byte (`+8`) and an i16 delay
  in ticks (`+0xa`). The count, the head included, is the i32 at
  `0x005023ec`. `0x0023f140` drops a line when five are queued.
  `0x0023f0a0` empties it (a count of 1, a head with no sound).
- **The update** (`0x0023f000`, no arguments, `ret`). While the head's
  delay is 0 or more it counts down; at -1 it plays the head's sound
  (`0x00189710`) and sets the playing byte. With the delay below zero and
  the playing byte clear, the head is dropped and the next line becomes
  the head. Its caller is not traced here (S: once a tick).
- **The end.** `0x0023f120`, given a sound that ended, clears the playing
  byte when that sound is the head's.

So the queue is idle when it is empty, or holds only a head whose delay
has run out and whose sound has ended. Whether the game lowers other
sounds under an announcer line is not read.

## Metering the mixer

Code: `crates/platform/audio/src/meter.rs` (the maths),
`crates/extensions/game/src/mixer_meter.rs` (the tap and `audio.meter`),
the tap's wiring in boot's `lib.rs`. Tests: `meter.rs`'s own and
`mixer_meter/tests.rs`.

`audio.meter` ([agent-api.md](../runtime/agent-api.md#audiometer))
reads the level of the mixer's output bus: the interleaved stereo `f32`
DirectSound's output thread renders in logical time, handed to
`dsound::Output::tap_samples` before the device queue. The same tap
carries a recording film's sound. A run with no audio device renders
into a `NullSink` and meters the same samples a speaker would get. A
clock-only run (a light host or sim) renders no samples, so the meter is
not connected and the method answers `UNAVAILABLE`.

- **Capture.** Nothing is kept until the first request. Then the last
  10 seconds of output (`KEEP_SECONDS`) stay in a ring, counted in frames
  from that request.
- **Levels.** `audio::meter::measure_f32` gives the sample peak and RMS
  in dBFS, integrated loudness after ITU-R BS.1770-4 (K-weighting, 400 ms
  blocks stepped 100 ms, gated at -70 LUFS and 10 LU under the gated
  mean) and the loudest block. Each channel weighs 1. Studio's
  `sound.measure` meters 16-bit clips with the same code
  (`audio::meter::measure`), so a clip's own level and its level after
  the mixer compare on one scale.
- **Host clips.** `play` starts one of our embedded clips as a plain 2D
  buffer in the mixer, as the game does: `pogo/compress`, `pogo/spring`,
  `pogo/settle` ([pogo-stick.md](../game/pogo-stick.md)) and
  `announcer/<medal id>`. No game sound tag carries these, so only this
  bus shows their level. The reply meters from the call to the clip's end
  plus 20 ms (`TAIL_MS`), with whatever else the game plays in that span,
  and the buffer leaves the mixer when it answers. A clip that is still
  a Git LFS pointer answers `UNAVAILABLE`.

## Our medals' announcer lines

Code: `crates/extensions/game/src/medal_announcer.rs`. Tests:
`medal_announcer_tests.rs`. Switch: the `medal-announcer` feature (on;
`H2_FEATURES=-medal-announcer` keeps the medals silent).

Our medals (the superbounce ones, [superbounce.md](../game/superbounce.md#medals))
have announcer lines of their own, AI-generated placeholders:
`art/announcer/<medal id>.wav`, 16-bit PCM, matched offline to the retail announcer's loudness
(`art/announcer/README.md`). When the HUD shows one of our medal lines
(`superbounce_hud`, [hud-kit.md](../game/hud-kit.md#the-medals)), its
clip is queued; one line per HUD line, so a chain of three bounces says
Superbounce, Double bounce and Triple bounce in that order, and a Monster
says Monster bounce. Bounced has no clip and is silent.

- **Taking turns with the game.** Our clips have no sound tag, so they
  cannot be lines in the game's queue. A clip of ours starts only when the
  game's queue is idle (`game_idle`, read each tick). While it plays, a
  `before` hook on `ANNOUNCER_QUEUE_UPDATE` skips the update, so a retail
  line queued meanwhile waits, its delay held, and plays once ours ends.
  Retail lines already waiting go before our next one. A line that holds
  the game's queue for 150 ticks (`MAX_HOLD_TICKS`, five seconds, over
  twice the longest clip) lets it go, so a stalled mixer cannot silence
  the game's announcer.
- **Our queue.** One line at a time, in the HUD's order. At most four wait
  behind the one playing (five in all, the game's length); past that the
  oldest waiting line is dropped, so the newest medal is spoken. A new
  game drops the waiting lines. In split screen each local player's line
  is queued, as the game queues a multi-kill for each local player who
  earned it.
- **Playback.** A clip plays as a 2D buffer in DirectSound's mixer, on no
  mix bins (both speakers), under the mixer's master volume. Halo 2 has
  no announcer or dialogue volume setting; the gain the game's sound
  classes give its own dialogue is not applied, and our lines neither
  duck nor are ducked. Boot connects the mixer whenever audio renders
  samples, into a device or a `NullSink`; clock-only runs (a light host
  or sim) drop the lines.
- **Embedded.** The clips are compiled in with `include_bytes!`, so the
  installed `h2` needs no repository next to it. They are in Git LFS: a
  checkout without them (the lab's `GIT_LFS_SKIP_SMUDGE`) still builds,
  and each clip that is an LFS pointer is named once at run time
  (`announcer: art/announcer/<id>.wav: a Git LFS pointer ...`) and stays
  silent. `git lfs pull` and a rebuild bring them in.

The Incel medal has a clip too (`incel.wav`), not compiled in: no line in
the game shows that medal ([teabag.md](../game/teabag.md)), so nothing
speaks it.

## Unsupported: the MCPX DSP

The retail APU had a Motorola 56300 "GP" DSP running Microsoft's default
effect image and the SDK's effect images. These are skipped:

- **SFX/EP processing**: the DSP effect images themselves. The I3DL2 reverb
  is replaced by a clean-room approximation (`crate::reverb`); the
  crosstalk-cancellation effect and generic effects on mix bins are not
  modelled.
- **MCPX 3D DSP**: the DSP implemented positional audio with HRTF and XTalk.
  The mixer uses the panner or the optional headphone model described above.
- **Dolby Digital / DTS encoding** for the S/PDIF output and 5.1 mixing:
  output is stereo.
- **WMA**: not a DSP feature, but noted here: the game's own codec is
  replaced at its six entries by `crates/platform/wma` (`dsound::xwma`), with the
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

## Not implemented

- Notifications and play regions: Halo 2 has no call sites for them (see
  [dsound.md](dsound.md)).
- Resampling is linear. A windowed-sinc or polyphase path would be better for
  big downsamples.

The guest mapping is in `crates/platform/dsound`; `dsound::Hle::spawn_output` is the
one consumer that renders into the ring-fed `DeviceSink` ([dsound.md](dsound.md),
"Consumers and the output thread").
