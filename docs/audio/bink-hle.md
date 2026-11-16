# Bink HLE

`crates/bink-hle` provides opt-in replacements for the Bink 1.x player that
Halo 2 links statically (sections `BINK*`, `BINKDATA`). It has no Bink decoder;
see [movies.md](movies.md) for the optional install-time MOV transcode.
The boot runner registers it in skip mode (`RunOpts::bink`, off with
`h2 run --no-bink`); the movie frame source below is not installed by any
host yet.
Campaign scenes rendered by the engine are separate from disc Bink files.

All addresses are virtual addresses in the retail XBE (XDK 5849). The
identification was done by hand from `h2 disasm` plus a throwaway
call-site scanner: the Bink code has no signature database (`h2 symbols`
covers XDK libraries only, see [xdk-libraries.md](../lifter/xdk-libraries.md)), so each
function is named from its call shape and the Bink 1.x public API.

## Two execution options

**(a) Let the lifted library run.** Bink is linked into the XBE, not an
imported DLL: the lifter already sees the code at `BinkOpen` (`0x003e0af0`),
`BinkDoFrame` (`0x003e2870`), and `BinkCopyToBuffer` (`0x003e2830`). Leaving
those addresses alone executes the retail decoder as translated soft code.
It still needs the library's file I/O, allocation callbacks, CPU operations,
DirectSound opener (`0x003e3c90`) and D3D copy path to behave correctly. It
avoids install-time conversion, but decoder correctness and performance
become part of the full recompiler bring-up. We do not extract that library
or copy any of its code into this public repository.

**(b) Override the public API at function entries.** `BinkOpen` resolves a
manifest entry; `BinkDoFrame` obtains a host BGRA frame;
`BinkCopyToBuffer` copies the retained picture into the guest's locked
texture. `BinkWait`, `BinkNextFrame`, and `BinkClose` preserve pacing,
position, looping, and lifetime. The engine wrapper remains compiled code.
The table below gives all ten candidate override addresses. A future host
must opt into this mode after D3D and movie audio integration are ready.
Missing or corrupt transcodes make the source decline the open; the
existing HLE's NULL return uses the wrapper's failed-open handling.

### Read-only address verification

Rechecked on the local retail image using the repository's `h2 disasm`
command (2026-11-15), for example:

```sh
h2 disasm /path/to/own/default.xbe 0x156090 300
h2 disasm /path/to/own/default.xbe 0x1568d0 60
h2 disasm /path/to/own/default.xbe 0x3e0af0 100
```

Call sites `0x156256` / `0x156278` target sound setup / open;
`0x1562de` and `0x156316` target mix bins; `0x15632f` targets mix-bin
volumes; `0x155eb9` targets memory setup; and `0x156573` / `0x1566d3`
target close. Wait is called at `0x155faa` and `0x155fc9`. The decode wrapper calls DoFrame at `0x1568da`, NextFrame at
`0x1568e6`, then CopyToBuffer at `0x156949`. Only addresses and behavioral
observations are recorded here, not instruction bytes. Function names are
inferences from calling convention, call shape and public API behavior,
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
| `0x1568d0` | `BinkDoFrame`, `BinkNextFrame`, lock texture, `BinkCopyToBuffer`, unlock |
| `0x156960` | draw the texture as a quad |
| `0x155f80` | service: finished check (current frame == frames), polls skip input via `0x156ab0` |
| `0x156ab0` | true if any of ten controller buttons (ids 0..7, 0xc, 0xd) is down |
| `0x1565a0` / `0x156560` / `0x155f10` / `0x1566c0` | stop variants: `BinkClose`, release texture, clear flags |
| `0x155f60` | `IsPlaying`: `ST_BINK != 0 && ST_ACTIVE` |
| `0x1564e0` | remaining time in ms (`frames * den * 1000 / rate`) |
| `0x223aa8` | path builder: `d:\bink\%s_%d.bik` (`0x4575d0`) with name and `60` |

Start flags (`0x4e9190`): bit 0 loop (never finishes), bit 1 skippable by
button, bit 2 and bit 5 extra mode bits (bit 5 restores state on stop), bit 4
unscaled draw, bit 8 large preload buffer.

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

## Behaviour

`register(&mut dispatch, &hle)` picks a mode by whether a `MovieSource` is
configured.

**Skip (no source, the default).** One override, on `0x156090`. It reads and
logs the path and flags, writes `ST_BINK = 0`, `ST_MOVIE_OPEN = 0`,
`ST_FINISHED = 1`, and returns with `ret 8`. `IsPlaying` is then false and
every caller carries on at once. It does not touch D3D, Bink or audio, so it
needs nothing else of the boot. If the wrapper's state page is not mapped it
leaves it alone.

**Playback (a source is present).** Ten overrides, one per row in the table
above. The wrapper runs as compiled. `BinkOpen` asks the source for the
movie's stream and returns a fake HBINK; if it declines (movie not
transcoded, unreadable, nonsense dimensions, no free handle) it returns 0,
which the wrapper already handles as a failed open, so a missing file is a
skip in this mode too.

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
  `BinkNextFrame` advances `FrameNum` and moves the due time one frame
  on; past the last frame it wraps to frame 1, as the original does
  (`0x3e2fb9..0x3e2fc8` calling the goto at `0x3e0820`), which is what loops
  rely on. Non-looping callers stop at `FrameNum == Frames` first. `BinkCopyToBuffer` copies the *decoded* buffer, since
  the engine calls it after `NextFrame`. Surface 3 (32-bit) and 5 write
  BGRA; 4 and 6 swap red and blue; other types write nothing and return 1.
  Rows and columns are clipped to the destination's pitch and height.
- `BinkWait` returns 1 until the next frame is due, from an injectable
  `Clock` (so tests do not sleep). A burst after a long stall is avoided by
  resyncing when more than a frame behind.
- `BinkSetSoundSystem`, `BinkSetMixBins`, `BinkSetMixBinVolumes` and
  `BinkSetMemory` are accepted and do nothing.

`BinkHle::requests()` lists every movie asked for and whether it was played.

## Future host integration (not installed by any host yet)

Constructing the source opens no guest dispatch entries:

```rust
use std::sync::Arc;
let source = bink_hle::TranscodedSource::new(
    movies_dir,
    Arc::new(bink_hle::AvFoundationDecoder),
);
// source.open("intro_60") returns a MovieStream, without a BinkHle or runtime.
```

`MovieSource` resolves names, `MovieDecoder` opens a container, and
`MovieStream` provides metadata and zero-based BGRA frames. The macOS
`AvFoundationDecoder` uses `AVAssetReaderTrackOutput`, with system H.264/HEVC
decoders and `kCVPixelFormatType_32BGRA`. It counts compressed samples to
avoid duration-rounding errors, gets the rational timebase from track/sample
metadata, reads sequentially, and recreates the reader for backward requests
(including loops). It rejects unsupported dimensions and unknown timing.
Only one picture is copied at a time, accounting for native row padding.
Failed and out-of-range requests leave the destination buffer untouched.
`try_open` exposes diagnostics; the trait's `open` returns `None` on failure.
The Objective-C bridge uses ARC and per-call autorelease pools; its retained
reader has one Rust owner and is released on drop. Other targets can inject
their own `MovieDecoder`. `SyntheticDecoder` remains available without codecs.
See Apple's [track output documentation](https://developer.apple.com/documentation/avfoundation/avassetreadertrackoutput)
and [sample output contract](https://developer.apple.com/documentation/avfoundation/avassetreaderoutput/copynextsamplebuffer%28%29).

The registration API below is what `boot` calls today, in skip mode
(`crates/boot/src/lib.rs`, `if opts.bink`). Passing a `TranscodedSource`
(the commented line) is the playback integration no host does yet:

```rust
use std::sync::Arc;
let hle = bink_hle::BinkHle::new(None); // skip mode
// or: BinkHle::new(Some(Arc::new(bink_hle::TranscodedSource::new(movies_dir, decoder))))
runtime.edit_dispatch(|d| {
    let (mode, installed) = bink_hle::register(d, &hle);
    // keep `installed` to call bink_hle::unregister(d, installed) later
});
```

- Register after the generated handlers; overrides replace the generated
  original at that entry address, and nothing else is changed.
- Entry addresses must be function starts in the lifted output. The ten Bink API
  entries and `0x156090` were checked against the analyzer's function-start
  list (written before `h2 analyze` panicked at `analyze/src/lib.rs:971`).
- Skip mode needs no mapped memory beyond the image. Playback also needs the
  D3D texture and lock path (`0x23e340`, `0x3fdbc0`) to work, and room for one extra page.

## Tests

`cargo test -p bink-hle` includes synthetic-state tests, a loader-backed test,
and macOS AVFoundation tests using media generated at test time by ffmpeg.
The latter are ignored when ffmpeg is absent at build time; they need access
to macOS's media decoder service. No runtime overrides are registered in the
AVFoundation tests. The existing guest-state tests cover skip state and stack cleanup, skip with
unmapped state and a null path, fall-through to NULL when the source declines
or the manifest is missing, a full open/decode/copy/next/close cycle including
NextFrame-before-Copy ordering and pacing, loop wrap (`[0,1,2,0]`), clipping
and red/blue swap, handle-table limits and reuse, setup calls popping their
arguments, restoring a displaced override, arena never adopting existing
memory, and independently written address pins. `tests/loader_arena.rs`
builds a tiny XBE, loads it with the runtime's loader, and checks the arena
stays out of the loader's stack with sentinels in place.

`tests/xbe_pins.rs` is `#[ignore]`d. It checks the constants against the real
retail image (ten Bink entries are called from the wrapper, `BIKi` inside
BinkOpen, the four `MovieStart` call sites, the `ST_*` state words in
`IsPlaying` and the service function):
`H2_XBE=/path/to/default.xbe cargo test -p bink-hle --test xbe_pins -- --ignored`.

## What real playback still needs

1. Host installation wiring. Use `movies::install::install_iso` and configure
   a `TranscodedSource` with `AvFoundationDecoder` only after installation.
   No boot, CLI or runtime wiring is included here.
2. Audio. The transcoded audio track is AAC in the MOV. The decoder should expose it
   and the `audio` crate should play it, started at `BinkOpen` and stopped at
   `BinkClose`, with the video clock slaved to it (today the video runs on the
   wall clock). Volume and mixbin calls are ignored.
3. The boot's D3D HLE must support the wrapper's texture: create a
   `bitm`-style 640x480 32-bit texture (`0x23e340`), lock it
   (`0x3fdbc0`) and draw the quad (`0x156960`). If the boot replaces D3D
   wholesale, the better seam may be to override `0x1568d0` and `0x156960`
   and hand frames straight to the gfx layer.
4. Verification against a real boot: none of this has run in the full lifted
   game yet. The skip path's effect on the menu is by reading the callers, not
   by observation.
5. Credits (`0x2ab8c0`) show `0x1564e0`'s remaining time on screen. In playback
   mode it works from the HBINK; in skip mode the stub leaves it to the
   widget's own handling of "no movie".
