# Movies

Halo 2 ships three full-motion movies on the disc, all Bink 1 (`BIKi`),
640x480 at 29.97 fps with 44.1 kHz stereo Bink audio:

| Disc path | Size | Used for |
|---|---|---|
| `/bink/intro_60.bik` | 13 MB | opening movie |
| `/bink/attract_60.bik` | 123 MB | attract mode |
| `/bink/credits_60.bik` | 353 MB | credits |

`h2 ls` shows them. The movies are optional: if they have not been
converted, the game skips them.

## Decision

The movies are converted once, at install time, with the user's own ffmpeg.

- **Bink is proprietary.** The format is owned by RAD Game Tools and there is
  no licence or spec we can use to build a decoder from.
- **RAD Video Tools only converts on Windows.** The official converter is not
  an option on macOS or in a build pipeline for tvOS.
- **ffmpeg can decode Bink 1**, but ffmpeg is LGPL/GPL code. We run it as a
  separate program the user already has. The repository contains no Bink
  decoder, does not link ffmpeg and does not vendor any of its source, so
  ffmpeg's licence does not attach to our code.
- **Nothing derived from the disc is committed.** The converted files live in
  a directory the user chooses and are never checked in.

### Licensing caveats

- ffmpeg's licence depends on the user's own build: a default build is LGPL,
  but one with `libx264` (which we prefer) is GPL. That applies only to the
  ffmpeg the user installed, not to anything we ship.
- The terms change if we ever bundle ffmpeg in an installer or app, or
  redistribute converted output. Bundling brings ffmpeg's licence with it,
  and H.264 and AAC carry patent licensing considerations for redistribution.
  Converting for your own use from your own disc does not.

Consequence: a machine without a suitable ffmpeg still runs the game, just
without movies.

## Output format

H.264 video and AAC audio in an MP4 container.

- AVFoundation (macOS, tvOS, iOS) plays it natively with hardware decode, so
  the player needs no codec of its own on those targets.
- It is a plain, widely supported format. Other targets can decode it with
  their platform decoder or any H.264/AAC library.
- 640x480 is tiny; H.264 High profile is far more than enough and is the safest
  choice across Apple TV generations. HEVC would shave some bytes but gains
  little at this resolution and plays on fewer devices.

Settings (`libx264`, or `h264_videotoolbox` if the ffmpeg has no x264):

- video: `-c:v libx264 -preset slow -crf 18 -profile:v high -pix_fmt yuv420p`.
  CRF 18 is visually close to transparent for the source, which already
  has Bink compression artifacts. The frame rate and size are left as they are.
- audio: `-c:a aac -b:a 192k`. The source is lossy, so the bitrate is on the
  generous side to avoid a visible second-generation loss.
- `-movflags +faststart` so the index is at the front of the file.

The exact settings used are recorded in the manifest.

## Using the command

```sh
h2 movies <iso> --out <dir> [--ffmpeg PATH] [--force]
```

- `<iso>` may be left out; the usual `$H2_DISC` / `disc/` lookup applies.
- ffmpeg is found on `PATH`, or with `--ffmpeg`. The command checks `-version`,
  that `-decoders` lists the Bink video and audio decoders, and that an H.264
  and an AAC encoder are present.
- Each `.bik` is streamed from the ISO straight into ffmpeg's stdin; the
  source is never copied to disk. ffmpeg writes the MP4 to a scratch file in
  the system temp directory (`faststart` needs a seekable output), which is then
  copied into `<dir>` and deleted. Expect roughly the size of the largest
  output as scratch space.
- Output goes through the same symlink-safe writer as the other exporters. It
  refuses to replace an existing file unless `--force` is given. A movie whose
  output exists fails (and the exit code is non-zero) without running ffmpeg.
- Ctrl-C (SIGINT) and SIGTERM stop the run cleanly: streaming halts, ffmpeg is
  stopped, the scratch directory (created mode 0700) is removed, and the exit
  code is 130. Movies already finished stay in place and in the manifest.
- Writes are atomic: each file and `movies.json` are written under a temporary
  name and moved into place only when complete, so a failure or interrupt
  leaves no partial file, and under `--force` the old file survives until the
  new one is ready. An existing `movies.json` that cannot be parsed makes the
  run refuse unless `--force` is given.
- `--only NAME` limits the run to movies whose file name contains NAME (for
  example `--only intro`).
- Progress is printed per movie as the source is read, then a summary with
  each movie's duration and size.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | every movie converted |
| 1 | at least one movie failed, or the disc or output could not be used |
| 2 | bad arguments |
| 3 | ffmpeg not found; says the movies are optional |
| 4 | ffmpeg found but unusable (no Bink decoder, no H.264 or AAC encoder) |
| 130 | interrupted |

An install script can treat 3 and 4 as "continue without movies".

## Manifest and lookup

`<dir>/movies.json` records, for each movie: source name, source size and
SHA-256 (taken from the bytes as they were streamed), output file name and
size, duration, the ffmpeg version line and the encode settings. It is updated
after every successful movie, so a partial run is still usable.

The `movies` crate reads it:

```rust
if let Some(path) = movies::locate(dir, "intro_60") {
    // play it
} // else: not installed, skip the movie
```

`locate` returns nothing when the manifest, entry or output is missing, the
output's size differs from what was recorded, or the recorded `output_name` is
not a bare file name (no separators, `..` or absolute path).
`movies::locate_fresh(dir, name, size, sha256)` additionally requires the
entry to match the disc file. For finer control, load the `Manifest`, `find` the entry and use
`MovieEntry::freshness(size, sha256)`; the `ffmpeg_version` and `settings` fields are there for
the same purpose.
