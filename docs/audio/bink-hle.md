# Bink HLE

Status: Built. Code: `crates/platform/bink-hle/src`, `crates/extensions/game/src/movies_glue.rs`. Tests: `crates/platform/bink-hle` (`xbe_pins` is ignored).
Switch: `--movies`, `--no-movies`, `--no-bink` (command-line flags).

`crates/platform/bink-hle` replaces the Bink 1.x player that Halo 2 links statically
(sections `BINK*`, `BINKDATA`). It has no Bink decoder; see
[movies.md](movies.md) for the optional install-time MOV transcode.
Every host (`h2 run`, `h2 serve`, `h2 play`, the apps) makes the same choice
at startup (`crates/extensions/game/src/movies_glue.rs`). The decoder plays
current transcodes, including attract clips prepared from the disc on a
windowed menu boot. Playback is optional. Headless runs and level boots
default to skip mode. The log records the selection:

```text
movies: playback (3 transcoded: attract_60, credits_60, intro_60)
movies: playback (preparing attract_60 from disc)
movies: skip (none installed in ~/Library/Application Support/h2recomp/movies; `h2 movies <disc.iso>` installs them)
movies: skip (stale: intro_60 was transcoded from another disc (re-run `h2 movies`))
movies: skip (off: --no-movies)
movies: skip (off: headless run; --movies DIR plays them)
movies: skip (off: level boot 01a_tutorial; --movies DIR plays them)
```

`--movies DIR` points `h2 run`/`h2 play` at another install and plays from
it even headless or on a level boot, `--no-movies` forces skip mode, and `--no-bink` (run only) leaves the original player in
place. Campaign scenes rendered by the engine are separate from disc Bink
files.

All addresses are virtual addresses in the retail XBE (XDK 5849). The
identification was done by hand from `h2 disasm` plus a throwaway
call-site scanner: the Bink code has no signature database (`h2 symbols`
covers XDK libraries only, see [xdk-libraries.md](../lifter/xdk-libraries.md)), so each
function is named from its call shape and the Bink 1.x public API.

## Execution

Bink is linked into the XBE, not an imported DLL. The lifted library is not
run: the HLE overrides the public API at function entries. `BinkOpen`
(`0x003e0af0`) resolves a manifest entry; `BinkDoFrame` (`0x003e2870`)
obtains a host BGRA frame; `BinkCopyToBuffer` (`0x003e2830`) copies the
retained picture into the guest's locked texture. `BinkWait`,
`BinkNextFrame` and `BinkClose` preserve pacing, position, looping and
lifetime. The engine wrapper remains compiled code. The table below gives
all ten override addresses. Missing or corrupt transcodes make the source
decline the open; the HLE's NULL return uses the wrapper's failed-open
handling. No Bink code is extracted or copied into this repository.

### Read-only address verification

Rechecked on the local retail image using the repository's `h2 disasm`
command, for example:

```sh
h2 disasm /path/to/own/default.xbe 0x156090 300
h2 disasm /path/to/own/default.xbe 0x1568d0 60
h2 disasm /path/to/own/default.xbe 0x3e0af0 100
```

Call sites `0x156256` / `0x156278` target sound setup / open;
`0x1562de` and `0x156316` target mix bins; `0x15632f` targets mix-bin
volumes; `0x155eb9` targets memory setup; and `0x156573` / `0x1566d3`
target close. Wait is called at `0x155faa` and `0x155fc9`. The decode wrapper calls DoFrame at `0x1568da`, NextFrame at
`0x1568e6`, then CopyToBuffer at `0x156949`. Only addresses and behavioural
observations are recorded here, not instruction bytes. Function names are
inferences from calling convention, call shape and public API behaviour,
not symbols recovered from the stripped executable.

## What the game calls

Only ten functions in the Bink sections are called from outside them, and
every caller is inside the movie wrapper at `0x155e00..0x156b00`. All are
`__stdcall` (callee pops).

| Address | Bink function | Args | Evidence |
|---|---|---|---|
| `0x003e0450` | `BinkSetSoundSystem` | `(open_fn, ds)` ret 8 | called with `0x3e3c90` (Bink's DirectSound opener) and `[kernel_audio + 0x2ab0]` |
| `0x003e0af0` | `BinkOpen` | `(path, flags)` ret 8 | contains the `BIKi` signature compare (`0x3e0be9`); result stored as the handle |
| `0x003e2330` | `BinkWait` | `(bnk)` ret 4 | result negated into the "frame ready" byte |
| `0x003e2630` | `BinkSetMixBins` | `(bnk, track, bins, n)` ret 16 | 4 args, sits next to the volume variant |
| `0x003e2680` | `BinkSetMixBinVolumes` | `(bnk, track, bins, vols, n)` ret 20 | 5 args, run right after the above |
| `0x003e2820` | `BinkSetMemory` | `(alloc, free)` ret 8 | called with the wrapper's alloc/free callbacks `0x156710` / `0x156810` |
| `0x003e2830` | `BinkCopyToBuffer` | `(bnk, dst, pitch, h, x, y, flags)` ret 28 | passed a locked D3D texture, `flags = 3 \| 0x80000000` (32-bit surface, copy all) |
| `0x003e2870` | `BinkDoFrame` | `(bnk)` ret 4 | |
| `0x003e2e50` | `BinkNextFrame` | `(bnk)` ret 4 | |
| `0x003e2ff0` | `BinkClose` | `(bnk)` ret 4 | |

`BinkGoto`, `BinkPause`, `BinkSetVolume`, `BinkGetError` and friends are
linked in but never called; there is nothing to hook.

HBINK fields the wrapper reads: `+0` width, `+4` height, `+8` frames,
`+0xc` current frame (1-based, equals frames on the last frame), `+0x14` and
`+0x18` the frame rate numerator and denominator.

## The engine's movie wrapper

One global state block of `0xe0` bytes at `0x4e9188`:

| Address | Meaning |
|---|---|
| `0x4e9188` u8 | wrapper initialised (set by `0x155ea0`) |
| `0x4e9189` u8 | a movie was started |
| `0x4e918a` u8 | `BinkWait` says a frame is due |
| `0x4e918b` u8 | input skip enabled |
| `0x4e918c` u8 | finished (last frame shown) |
| `0x4e9190` u32 | start flags (below) |
| `0x4e9194` u32 | HBINK, 0 when no movie |
| `0x4e9198` / `0x4e919a` i16 | width / height |
| `0x4e91a0` u32 | D3D texture the frames are copied into |

Functions:

| Address | Role |
|---|---|
| `0x155ea0` / `0x155ed0` | init (calls `BinkSetMemory`) / dispose; referenced from a table at `0x440f64` / `0x440f68` |
| `0x156090` | start movie `(path, flags)` ret 8: allocates a buffer, opens Bink, sets mixbins, creates the texture |
| `0x156040` | per-frame tick: wait, decode and blit, draw, service |
| `0x1568d0` | `BinkDoFrame`, `BinkNextFrame`, lock texture, `BinkCopyToBuffer` |
| `0x156960` | draw the texture as one screen quad (record `0x4e91c4`, rectangle `0x485a8a`) |
| `0x155f80` | service: finished check (current frame == frames), polls skip input via `0x156ab0` |
| `0x156ab0` | true if any of ten controller buttons (ids 0..7, 0xc, 0xd) is down |
| `0x1565a0` / `0x156560` / `0x155f10` / `0x1566c0` | stop variants: `BinkClose`, release texture, clear flags |
| `0x155f60` | `IsPlaying`: `ST_BINK != 0 && ST_ACTIVE` |
| `0x1564e0` | remaining time in ms (`frames * den * 1000 / rate`) |
| `0x223aa8` | path builder: `d:\bink\%s_%d.bik` (`0x4575d0`) with name and `60` |

Start flags (`0x4e9190`): bit 0 loop (never finishes), bit 1 skippable by
button, bit 2 and bit 5 extra mode bits (bit 5 restores state on stop), bit 4
draws the movie stretched over the screen rectangle (clear: centred at its
own size, `0x00156982`), bit 8 large preload buffer.

Name strings: `attract` `0x4575e4`, `credits` `0x4575ec`, `intro` `0x4575f4`.

Callers of the start function:

| Call site | Movie | Flags | Where |
|---|---|---|---|
| `0x12bff5` | intro | `0x1c6` | `0x12bf90`, boot intro; returns whether a movie is playing; called from `0x18e937` |
| `0x163949` | intro | `0x3c6` | `0x163890`, level/menu entry (the "intro pending" byte `0x547f28`) |
| `0x223a63` | attract | `0xe` | `0x223a21`, from idle timer `0x223976` (called from `0x147aad`) |
| `0x2ab8e7` | credits | `0x40c` | `0x2ab8c0`, credits widget (table `0x44eca8`; remaining time at `0x44ecb8`) |

End-of-movie and skip are entirely wrapper logic: `0x155f80` sets
`ST_FINISHED` when `FrameNum == Frames`, or `0x156ab0` reports a button on a
skippable movie, and the next tick runs the stop path.

### Attract timer

The idle timer at `0x223976` compares the UI clock with the newer of the
last input and the attract reset. On the title and main-menu screens, the
timer closes the music gate a map-configured lead time before the movie
deadline. The shared UI tag supplies that lead at `wigl +0x1c0` via
`0x235ca9`. The menu-music controller at `0x235b9c` applies the gate subject
to its six-second change guard. When frame limiting is active, the same
screen check (`0x14a224`) makes frame timing at `0x12b0e0` use at least two
vblanks per guest frame. The start path at `0x223a21` selects
`d:\bink\attract_60.bik` and passes the skippable movie flags to the wrapper.
Its input skip and end-of-movie paths close Bink and reset idle time at
`0x1565a0`. The next eligible UI update opens the music gate and the
controller restarts the intro.

The `attract` feature (`crates/extensions/game/src/attract.rs`) observes
the gate before the timer and checks the decoder when the timer first
closes it. `BinkHle::prepare` validates the selected transcode, decodes its
first frame and keeps that stream for `BinkOpen`. The guest starts the
movie and opens its audio voice at the usual deadline. If the clip is
unavailable, the guard keeps the gate open before the music controller
sees it. At the deadline the guard logs once and resets the idle clock.
This gives a transcode still being prepared another full idle cycle.
The guard runs in movie playback mode outside film sessions. In skip mode,
the retail timer can fade the music, call the wrapper's skip stub, then reset
the idle clock and restart the music. Film recording and playback use the
retail timer and the film's movie selection.
[`ui.attract`](../runtime/agent-api-host.md#uiattract) reports the timer, asset
and the wrapper's last attempt. [`audio.loops`](../runtime/agent-api-input.md#audioloops)
shows the accompanying music transitions.

## Behaviour

`register(&mut dispatch, &hle)` picks a mode by whether a `MovieSource` is
configured.

**Skip (no source).** One override, on `0x156090`. It reads and
logs the path and flags, writes `ST_BINK = 0`, `ST_MOVIE_OPEN = 0`,
`ST_FINISHED = 1`, and returns with `ret 8`. `IsPlaying` is then false and
every caller carries on at once. It does not touch D3D, Bink or audio, so it
needs nothing else of the boot. If the wrapper's state page is not mapped it
leaves it alone.

**Playback (a source is present).** Ten overrides, one per row in the table
above. The wrapper runs as compiled, except its decode and blit `0x1568d0` and
its draw `0x156960`, which are replaced too: the picture goes to the
overlay without touching the guest's texture, and the screen-quad batch
the draw would begin never runs
([retire-d3d-display.md](../graphics/retire-d3d-display.md#movies)). `BinkOpen` asks the source for the
movie's stream and returns a fake HBINK; if it declines (movie not
transcoded or not current, unreadable, nonsense dimensions, no free handle)
it returns 0, which the wrapper already handles as a failed open, so a
missing file is a skip in this mode too.

- HBINK structs live in a one-page arena (8 handles of `0x40` bytes). On the
  first open the crate scans `bink_hle::HLE_RESERVED`
  (`[0xa0000000, 0xa1000000)`, the interval reserved for HLE-owned storage and
  never handed to the guest) for the first completely unmapped page, maps it RW
  itself and uses only that page. The search never leaves that range, so it
  cannot take a page the guest has reserved but not yet committed, and it never
  adopts memory that is already mapped. If the range is exhausted `BinkOpen`
  returns NULL (a clean skip). `with_arena_hint` only moves the start inside
  the range; `BinkHle::arena()` reports the page. Other HLE crates should take
  their pages from the same range.
- `BinkDoFrame` decodes frame `FrameNum - 1` into a host BGRA buffer.
  `BinkNextFrame` advances `FrameNum`; past the last frame it wraps to frame
  1, as the original does (`0x3e2fb9..0x3e2fc8` calling the goto at
  `0x3e0820`), which is what loops rely on (the sound restarts with it).
  Non-looping callers stop at `FrameNum == Frames` first: the service
  (`0x15601c`) compares after the decode and blit, so the last picture of a
  movie is never blitted, as with the original player.
- `BinkCopyToBuffer` copies the *decoded* buffer, since the engine calls it
  after `NextFrame`. Surface 3 (32-bit) and 5 write BGRA; 4 and 6 swap red
  and blue; other types write nothing and return 1. Rows and columns are
  clipped to the destination's pitch and height.
- **Sound.** If the stream has a sound track (`MovieStream::audio_info`) and
  the host set an output (`BinkHle::set_audio_out`), `BinkOpen` opens a
  voice and the player keeps 500 ms of PCM queued on it, in 10 ms packets,
  topping it up on every DoFrame and NextFrame (not on Wait, see Pacing). `MixerAudio` makes the
  voice a stream in the `audio` crate's `Mixer`, the one the DirectSound HLE
  renders (`dsound_glue::Audio::mixer`), so movie sound goes out through the
  game's own output and device, at the track's rate (resampled by the
  mixer). `BinkClose` drops the voice, which removes it from the mixer.
  `BinkSetSoundSystem`, `BinkSetMixBins`, `BinkSetMixBinVolumes` and
  `BinkSetMemory` are accepted and do nothing (the game's mix-bin volumes
  for the movie are ignored).
- **Pacing.** `BinkWait` returns 1 until frame `FrameNum` is due:
  `(FrameNum - 1) * 1000 * den / num` ms of movie time. Movie time is the
  wall clock (`SystemClock`, injectable as `Clock`) from `BinkOpen` or the
  last loop; the sound starts at the same moment and plays at its own rate.
  `BinkNextFrame` moves to the frame now due when the clock has run more
  than a frame ahead (dropping frames to keep time, as Bink does), never
  past the last frame. `BinkWait` only reads the clock: no feeding and no
  mixer lock.

  Why not the sound's position: the first playback build used it, and the
  intro ran at 0.3 to 0.5 fps with about 1.4 s of game CPU per present
  (`mean ms cpu 1408.8`, d3d 0.1; 240 to 400 fps over the same presents
  with `--no-movies`). The wrapper's service spins on `BinkWait`
  (`0x155fc2..0x155fd0`) until it returns 0, and the mixer renders on the
  kernel's logical clock, which moves by executed guest work (1 ns per
  access). One spin iteration charges about 4 ns, so the sound took about
  8 million `BinkWait` calls to cover one 33 ms frame. On the wall clock a
  spin ends within one frame of real time
  (`a_spin_on_bink_wait_ends_on_the_wall_clock_with_the_sound_frozen`). In
  a paced run (`h2 play`) logical time keeps to the wall, so picture and
  sound stay together.
- **Cost.** Measured in release by the tests below on the development Mac: `BinkWait`
  about 1.3 us a call through the test runner; `CopyToBuffer` 33 us at
  640x480 and 75 us at 1280x720 (one `write_bytes` for the whole picture
  when the rows line up, else one per row through a reused row buffer;
  per-pixel-row writes were 54 and 182 us); synthetic decode plus copy and
  NextFrame 0.2 ms and 0.6 ms; AVFoundation 1.1 to 1.6 ms a frame at
  640x480 in order. `DoFrame` never decodes the picture already in the
  buffer, and the AVFoundation stream keeps its last picture: asking for it
  again would restart the reader from frame 0 (201 ms at frame 90, and
  growing); it costs 28 us. Copy and decode are not the cause of a slow
  intro.

`BinkHle::requests()` lists every movie asked for and whether it was played.

### What the wrapper does around the API

Read from the retail image, addresses only:

- `MovieStart` (`0x156090`) calls `GlobalMemoryStatus` (`0x331336`, which
  calls `MmQueryStatistics`, kernel ordinal 181) and stores the free memory
  in KiB at `0x47ff80`; it is informational. The kernel implements 181 from
  its own page accounting.
- It allocates the read buffer through the game's own cache allocator
  (`0x12d2f0`, 4 MiB, or 20 MiB with start flag bit 8), protects it
  write-combined (`0x2d15da`, `MmSetAddressProtect` with `0x404`), calls
  `BinkSetSoundSystem` only when the DirectSound object at
  `[0x51ebe4] + 0x2ab0` is set, then `BinkOpen(path, 0x8002000 or 0)`.
- The texture comes from `0x23e340(width, height, 10, ...)`: Halo 2 bitmap
  format 10 (X8R8G8B8) with the linear flag in its `bitm` header (`0x10`),
  so `CopyToBuffer`'s rows are plain linear BGRA at the lock's pitch.
- `0x1568d0` calls `D3D_BlockOnResource` (`0x3fad50`, overridden by
  crates/platform/d3d), the native lock helper `0x3fdbc0` (texture address and
  pitch), then `BinkCopyToBuffer(h, bits, pitch, height, 0, 0, 3 | 0x80000000)`.
  `CopyToBuffer` writes through `Guest::write_bytes`, so the runtime's write
  watch marks the texture's physical pages and the D3D write barrier
  (`d3d_glue::install_write_barrier`) re-uploads them at the next D3D entry,
  as for any guest store; nothing D3D-specific is needed in this crate.

## Host integration

`boot::movies_glue::install` wires the decoder and returns the attract
guard for the feature registry. `RunOpts::movies` is a
`movies_glue::Movies`: `Auto` (the default: the app data directory,
`default_dir()`), `Dir(path)` (`--movies DIR`) or `Off` (`--no-movies`).
`movies_glue::why_off` gives the reason a run skips regardless:

- A movie is current when the manifest has an entry for its disc path whose
  source size matches the disc file and whose output passes `movies::locate`
  (regular file, recorded size, output hash or install stamp). Source
  hashes are not recomputed at boot (that is `h2 movies`' and the
  installer's job); the size is the cheap tell for another disc. Movies that
  are not current are left out of the source (`TranscodedSource::only`), so
  only they skip.
- A normal windowed `Auto` boot looks up the attract movie through the same
  VFS that serves the guest. If the disc contains it and the transcode is
  missing or stale, a background worker prepares that one file using the
  existing local ffmpeg installer. Film sessions use the recorded asset
  choices. Explicit movie directories are read as supplied.
- A lockstep run (`H2_LOCKSTEP`, `H2_INPUT_RECORD`, `H2_INPUT_REPLAY`) is
  forced to skip (`movies: skip (off: lockstep)`): movie pacing follows the
  wall clock, which a schedule comparison must not see.
- With `Auto`, a direct-to-level boot (`h2 play --level`, `H2_LEVEL`) and a
  headless run (no window: `h2 run`, `h2 serve`, benchmarks) skip, with the
  reason in the log line: nothing there wants the intro, and a benchmark
  should not spend its first presents on it. `--movies DIR` plays them
  there; lockstep still skips.
- Platforms without a decoder (`bink_hle::platform_decoder()`: AVFoundation
  on macOS only) skip with `no movie decoder on this platform`.
- Register after the generated handlers; overrides replace the generated
  original at that entry address, and nothing else is changed.
- Entry addresses must be function starts in the lifted output. The ten Bink API
  entries and `0x156090` were checked against the analyzer's function-start
  list.

### Decoders

`MovieSource` resolves names, `MovieDecoder` opens a container, and
`MovieStream` provides metadata, zero-based BGRA frames and the optional
sound track. The macOS `AvFoundationDecoder` uses
`AVAssetReaderTrackOutput`, with system H.264/HEVC decoders and
`kCVPixelFormatType_32BGRA`. It counts compressed samples to avoid
duration-rounding errors, gets the rational timebase from track/sample
metadata, reads sequentially, and recreates the reader for backward requests
(including loops). It rejects unsupported dimensions and unknown timing.
Only one picture is copied at a time, accounting for native row padding.
Failed and out-of-range requests leave the destination buffer untouched.
The sound track is read on a second reader (so video rewinds leave it
alone) as interleaved 16-bit stereo at the track's own rate; Core Audio
decodes the AAC. A movie whose track cannot be read plays silent.
`try_open` exposes diagnostics; the trait's `open` returns `None` on failure.
The Objective-C bridge uses ARC and per-call autorelease pools; its retained
readers have one Rust owner and are released on drop. Other targets can
inject their own `MovieDecoder`. `SyntheticDecoder` (optionally with a
synthetic sound track as long as the video) needs no codecs.
See Apple's [track output documentation](https://developer.apple.com/documentation/avfoundation/avassetreadertrackoutput)
and [sample output contract](https://developer.apple.com/documentation/avfoundation/avassetreaderoutput/copynextsamplebuffer%28%29).

## Tests

`cargo test -p bink-hle` includes synthetic-state tests, a loader-backed test,
and macOS AVFoundation tests using media generated at test time by ffmpeg.
The latter are ignored when ffmpeg is absent at build time; they need access
to macOS's media decoder service. No runtime overrides are registered in the
AVFoundation tests.

The guest-state tests (`tests/synthetic.rs`) cover skip state and stack
cleanup, skip with unmapped state and a null path, fall-through to NULL when
the source declines or the manifest is missing, a full
open/decode/copy/next/close cycle including NextFrame-before-Copy ordering
and pacing, loop wrap (`[0,1,2,0]`), clipping and red/blue swap,
handle-table limits and reuse, setup calls popping their arguments,
restoring a displaced override, arena never adopting existing memory, and
independently written address pins. On top of those, with the synthetic
decoder and a Rust model of the wrapper's calls (start blit, Wait, decode
and blit, the service's `FrameNum == Frames` and skip button, close):

- `mode_is_picked_from_what_is_installed`: off, none installed, current,
  no decoder, stale, and partly stale (only the current movie opens);
- `frames_reach_the_locked_texture_and_mark_its_pages_written`: rows land
  at the lock's pitch in a contiguous-window texture, padding untouched, and
  the runtime's write watch reports the page D3D re-uploads;
- `the_wrapper_sees_the_end_of_the_movie_and_closes_cleanly`: every picture
  once, in order, the end seen through `FrameNum == Frames`, the voice
  removed from the mixer, and the next movie starting clean;
- `a_skip_mid_movie_closes_cleanly`: the voice is gone and the mixer silent
  at once, late calls on the old handle do nothing, and the handle is reused
  from frame 1 and the start of the sound;
- `sound_plays_at_the_movie_rate_through_the_mixer`: 1 s of 48 kHz mixer
  output consumes 44100 (less one 10 ms packet) frames of the 44.1 kHz
  track without starving, the output is the track, the video is at the same
  second, and NextFrame catches up;
- `a_spin_on_bink_wait_ends_on_the_wall_clock_with_the_sound_frozen`: with
  the mixer never rendering, every spin on `BinkWait` ends within one frame
  of wall time and every picture is shown;
- `bink_wait_call_cost` and `decode_and_blit_cost_per_frame` (640x480 and
  1280x720 into a contiguous-window texture with the write watch on) print
  the per-call and per-frame costs; the latter asserts one dirty range of
  the picture's size and a copy under 20 ms.

`tests/avfoundation.rs` also reads a generated movie's AAC track
(`generated_sound_track_reads_as_stereo_pcm_and_rewinds`): 44.1 kHz stereo,
the right length, a tone, unaffected by video reads, and rewindable;
`real_decoder_cost_per_frame_at_640x480` times in-order decoding (under
15 ms a frame) and a repeat of the last picture (a copy, not a rewind).
`movies::status` has its own tests for none, current, partial and stale
installs and extracted-disc listing, `boot::movies_glue` for off and none
and for level boots and headless runs skipping unless a directory is named, and
`kernel` for `MmQueryStatistics`.

`tests/loader_arena.rs` builds a tiny XBE, loads it with the runtime's
loader, and checks the arena stays out of the loader's stack with sentinels
in place.

`tests/xbe_pins.rs` is `#[ignore]`d. It checks the constants against the real
retail image (ten Bink entries are called from the wrapper, `BIKi` inside
BinkOpen, the four `MovieStart` call sites, the `ST_*` state words in
`IsPlaying` and the service function):
`H2_XBE=/path/to/default.xbe cargo test -p bink-hle --test xbe_pins -- --ignored`.

## Not verified

Everything above is tested against synthetic guest state. A run of the full
lifted game is needed to show:

1. The wrapper's allocation (`0x12d2f0`, 4 or 20 MiB from the game's cache)
   succeeding at the intro, main menu idle and credits.
2. Sound level and latency: the movie voice plays at full volume with no
   mix bins (the game's `BinkSetMixBinVolumes` is ignored), and picture and
   sound both start at `BinkOpen` and run on their own clocks (wall and
   mixer). They stay together only while logical time keeps to the wall;
   a run that falls behind drops frames to keep the picture on time while
   the sound lags.
3. Credits (`0x2ab8c0`) show `0x1564e0`'s remaining time on screen; in
   playback mode it works from the HBINK.
