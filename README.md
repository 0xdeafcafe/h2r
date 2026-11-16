# h2recomp

A static recompilation of Halo 2 (original Xbox) to native code, with the
runtime written in Rust. The retail executable is analysed, lifted to IR,
emitted as C11 and built by clang into a dylib. `crates/runtime` runs that
dylib against high-level replacements for the Xbox kernel, xapi, Direct3D
and DirectSound, rendering through wgpu on Metal.

You need your own legally obtained copy of the disc. No game data or extracted
assets live in this repo. The only screenshots are the docs' own, rendered by
scripts and stored in Git LFS (see CONTRIBUTING.md).

Public repo: <https://github.com/0xdeafcafe/h2r>.

## Where it is

The recompiled game boots to the main menu and runs campaign levels and
multiplayer maps, starting any of them directly, on macOS, Apple TV and
iPhone/iPad. Around it are an Xbox Live replacement with a web app, a map and variant editor, and
tools to convert Halo 1's maps into Halo 2's engine.

Speed is the big gap: about 11-16 fps in the main menu and about 15 in The
Heretic on an M-series Mac, against a target of 30
([perf-budget.md](docs/perf/perf-budget.md),
[frame-budget.md](docs/perf/frame-budget.md)). Much of the newer work is
built and proven by tests but not yet confirmed in the game; the table says
which.

## Features

**Built**: works in the game or the tools. **Tests**: built and proven by
tests, not yet confirmed in the game. **Design**: written up, not built.

| Area | Feature | State | Docs |
| --- | --- | --- | --- |
| Recompiler | XBE analysis (about 18,700 functions), x86 to IR to C11, clang build with source and object caches | Built | [LIFTER](docs/lifter/LIFTER.md), [cache](docs/lifter/BUILD-CACHE.md) |
| | Native x87 and SSE fast paths, inline guest RAM, direct calls (all on by default) | Built | [x87](docs/lifter/x87-native.md), [memory](docs/perf/performance.md) |
| Runtime | Xbox kernel, XAPI, saves and storage replaced in Rust, on a deterministic logical clock with checkpoints | Built | [BOOT](docs/runtime/BOOT.md), [storage](docs/runtime/storage.md) |
| | Direct3D 8 on wgpu (push buffer, NV2A shaders to WGSL), render thread, merged passes | Built | [d3d](docs/graphics/d3d.md), [graphics](docs/graphics/graphics.md) |
| | DirectSound mixer, native WMA, Bink replacement (cutscenes skipped; optional MP4 conversion) | Built | [audio](docs/audio/audio.md), [movies](docs/audio/movies.md) |
| Player | `h2 play`: window, sound, keyboard, mouse, pads; any level or map from the command line; `--follow` rebuilds as master moves | Built | [app](docs/runtime/app.md), [levels](docs/runtime/level-boot.md) |
| Platforms | macOS (Metal) | Built | [building](docs/runtime/building.md) |
| | Apple TV and iPhone/iPad apps, with touch controls on iOS | Built | [tvOS](docs/runtime/tvos.md), [iOS](docs/runtime/ios.md) |
| | Windows: the runtime and game module cross-build from a Mac; not run yet, no host app | Built | [windows](docs/runtime/windows.md) |
| | Xbox in Developer Mode (UWP app) | Design | [windows](docs/runtime/windows.md) |
| Graphics | Render scale up to 4K and beyond (Resolution row) | Built | [high-res](docs/graphics/high-resolution.md) |
| | E3 2003-style lighting, phase 1: sharper shadow maps (Lighting row) | Built | [E3 lighting](docs/graphics/e3-lighting.md) |
| | 60 and 120 fps over the 30 Hz tick (determinism harness built) | Design | [high-framerate](docs/game/high-framerate.md) |
| Game | Expanded settings screens: Video, Controls, FOV | Built | [settings](docs/game/settings-menu.md) |
| | Campaign skulls per profile, a Skulls page, our own skulls | Built | [skulls](docs/game/skulls.md) |
| | Twelve community game variants (Zombies, SWAT and more) as built-ins | Built | [variants](docs/game/community-gametypes.md) |
| | Aim assist strength setting | Tests | [aim assist](docs/game/aim-assist.md) |
| | Multiplayer Map Pack installed from your own disc | Built | [map pack](docs/game/map-pack.md) |
| | New, edited and patched maps as packages | Tests | [maps](docs/game/maps.md) |
| | More than one campaign: picker and per-campaign progress | Tests | [campaigns](docs/game/multiple-campaigns.md) |
| | Halo 1 health model and meter | Tests | [health](docs/game/health.md) |
| | Forge, saved films and theater | Design | [forge](docs/game/forge.md), [theater](docs/game/theater.md) |
| Halo 1 | All 13 Halo 1 multiplayer maps convert to Halo 2 maps (collision, BSP, MOPP, objects, shaders) | Tests | [h1 multiplayer](docs/game/h1-multiplayer.md) |
| | Halo 1 maps, bitmaps, models and animations read and converted to Halo 2 tags | Built | [h1 format](docs/formats/h1-map-format.md), [models](docs/formats/h1-models.md) |
| Input | Remappable bindings, input timelines, typing into the game's keyboard | Built | [input](docs/input/input.md) |
| | Button icons that follow the pad (Xbox, PlayStation, keyboard); extra button layouts | Built | [icons](docs/input/button-icons.md), [layouts](docs/input/controller-layouts.md) |
| | DualSense: speaker and mute button | Built | [PlayStation](docs/audio/playstation-audio.md) |
| | Steam Controller | Tests | [Steam](docs/input/steam-controller.md) |
| Live | Service: accounts, friends, teams, invites, stats, title files, cloud saves, custom maps, a relay | Built | [live](docs/live/live.md), [API](docs/live/live-api.md) |
| | In the game: sign-in, matchmaking, custom games browser | Built | [matchmaking](docs/live/live-matchmaking.md), [custom games](docs/live/live-custom-games.md) |
| | Co-op campaign over Live | Tests | [co-op](docs/live/live-coop.md) |
| | Web app: dashboard, stats, live top-down map, replays, admin | Built | [web](web/README.md), [art](docs/live/live-art.md) |
| | Hosting for friends: Docker, Cloudflare Tunnel, SQLite and R2, a menu bar app | Built | [hosting](docs/live/live-hosting.md) |
| Editor | `h2-editor`: 3D viewport, placements, game variants, playlists, Halo 1 maps | Built | [guide](docs/editor/editor-guide.md) |
| Tools | Tag decoders (`h2 bitmap`, `sound`, `ui`, `hs`, ...), map builder, profiler, benchmarks | Built | [formats](docs/README.md), [perf](docs/perf/perf.md) |

## Proof

Screenshots are kept in issue threads, not in the repo:

- [Boot to the main menu](https://github.com/0xdeafcafe/h2r/issues/1)
- [In-game levels](https://github.com/0xdeafcafe/h2r/issues/2)

## Work left

In priority order:

1. 30 fps: the rest of fast guest memory access and the gameplay frame
   ([architecture-review.md](docs/perf/architecture-review.md)).
2. Fidelity gaps, such as point sprites and the open items in
   [BOOT.md](docs/runtime/BOOT.md) and [shaders.md](docs/graphics/shaders.md).
3. Confirm the **Tests** rows above in the game.
4. 60 and 120 fps, and the rest of E3 lighting.
5. Later: Forge, theater, and a Windows and Xbox host app.

## Requirements

- macOS with Metal. The profiler is macOS-only, and the runtime and player
  target Metal.
- Rust 1.98 or newer and `cargo`. With rustup, `rust-toolchain.toml` picks the right version for you; with Homebrew, `brew upgrade rust`.
- clang on your `PATH`, for building the generated C.
- Your own disc image (see below).

## Your disc

Put your own disc image in `disc/` (it's git-ignored). On macOS, `cp -c`
clones the image without using any extra space:

```sh
mkdir -p disc
cp -c ~/Downloads/your.iso disc/halo2.iso
```

You can also pass a path to any command, or set `H2_DISC`. Both XISO (game
partition only) and full Redump images work. Without an argument `h2` uses
`$H2_DISC`, then the first `.iso` in `disc/`.

## Building

The root `justfile` builds every target from your disc
([casey/just](https://github.com/casey/just), `brew install just`):

```sh
just                 # list the recipes
just doctor          # what each target needs and what is missing
just mac-play        # build and play on this Mac
just tvos            # the Apple TV app's inputs; then just tvos-install
just ios maps=all    # the iPhone/iPad app with every map and the map pack
just windows-dll     # the game module cross-built for Windows
just check           # fmt, clippy and tests, as before a push (scripts/check.sh)
```

It uses `disc/halo2.iso` and, when present, `disc/mappack.iso` (or `iso=`,
`mappack=`, `H2_ISO`, `H2_MAPPACK`). Every recipe and what it needs is in
[docs/runtime/building.md](docs/runtime/building.md).

## Commands

`.cargo/config.toml` defines an alias, so `cargo h2 <command>` is
`cargo run --release -q -p cli -- <command>`. These all exist on master:

```sh
cargo h2 info                    # title, sections, libraries
cargo h2 ls                      # files on the disc
cargo h2 maps                    # map headers and tag counts
cargo h2 tags <map> [group]      # tags in a map
cargo h2 extract                 # copy the disc's files to disc/extracted/
cargo h2 analyze                 # function discovery and coverage
cargo h2 lift --stats            # lowering coverage
cargo h2 emit --out $TMPDIR/h2gen   # generate C and build the host library
cargo h2 h1 info disc/halo1.iso  # Halo 1 map headers (your own Halo 1 disc)
```

`emit` is slow and writes a lot of output (about 1 GB), so point `--out`
at scratch space. Unchanged output is cached; see
[docs/lifter/BUILD-CACHE.md](docs/lifter/BUILD-CACHE.md). `cargo h2` with no arguments
prints the full command list, including the tag decoders (`tag`, `bitmap`,
`sound`, `strings`, `ui`, `hs` and more).

Profiling and benchmarks need the `perf` feature (the alias doesn't pass it):

```sh
cargo run --release -p cli --features perf -- bench list
cargo run --release -p cli --features perf -- bench --only hot-loop
cargo run --release -p cli --features perf -- profile hot-loop/memory --seconds 5 --out $TMPDIR/prof
```

See [docs/perf/perf.md](docs/perf/perf.md).

## Play

```sh
cargo h2 play disc/halo2.iso
```

This opens a window and runs the game with sound, keyboard, mouse and
controllers. The first run generates the game library into `$TMPDIR/h2boot`
(slow; later runs reuse it). Close the window, press Cmd+Q or press Ctrl-C in
the terminal to quit.

| Key | Pad |
| --- | --- |
| Enter or Space | A |
| Backspace or V | B |
| E / Q | X / Y |
| Escape / Tab | Start / Back |
| Arrows | D-pad |
| WASD, mouse (click to capture) | Left stick, look |

Cmd+F toggles fullscreen and Cmd+P pauses. `--bindings FILE` remaps keys
([docs/input/input.md](docs/input/input.md)).

To keep playing while master moves, add `--follow`:

```sh
cargo h2 play --follow disc/halo2.iso
```

It fetches `origin/master` every 45 s and builds each new commit in the
background under `~/Library/Caches/h2recomp/follow`; your checkout is never
touched. A banner in the window shows progress. Press F5 on "Update ready"
to restart into the new build, or F6 to hide the banner. See
[docs/runtime/app.md](docs/runtime/app.md) for every flag.

## Editor

```sh
cargo run --release -p studio -- --disc disc/halo2.iso
```

`h2-editor` opens a map from your disc, shows it in 3D and edits its
placements, your game variants and the matchmaking playlists file. Start with
[docs/editor/editor-guide.md](docs/editor/editor-guide.md).

## Ghidra

Ghidra is used as a second opinion and for browsing. The pipeline never
depends on it at build time.

```sh
scripts/setup-ghidra.sh     # Ghidra 12.1.4 + XboxDev XBE loader + ghidra-cli, into .tools/
scripts/ghidra-export.sh    # import + analyse once, write disc/ghidra-functions.tsv
cargo h2 analyze --against disc/ghidra-functions.tsv
scripts/ghidra decompile 0x2d0aee
```

Ghidra is pinned to 12.1.4 because the XBE loader only loads on the version
it was built for.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how this is meant to end
up as structured Rust.

## Docs

[docs/README.md](docs/README.md) is the index: every doc, grouped by area
(lifter, runtime, input, graphics, audio, perf, formats, engine, game, Live,
editor), with the entry point for each marked. Start with
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the overall design.
