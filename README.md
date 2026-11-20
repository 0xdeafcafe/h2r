# h2recomp

A static recompilation of Halo 2 (original Xbox) to native code, with the
runtime written in Rust. The retail executable is analysed, lifted to IR,
emitted as C11 and built by clang into a dylib. `crates/runtime/runtime` runs that
dylib against high-level replacements for the Xbox kernel, xapi, Direct3D
and DirectSound, rendering through wgpu on Metal.

You need your own legally obtained copy of the disc. No game data or extracted
assets live in this repo. The only screenshots are the docs' own, rendered by
scripts and stored in Git LFS (see [CONTRIBUTING.md](CONTRIBUTING.md)).

Public repo: <https://github.com/0xdeafcafe/h2r>.

h2recomp was created under Microsoft's "[Game Content Usage Rules](https://www.xbox.com/en-US/developers/rules)" using assets from Halo 2 and Halo: Combat Evolved, and it is not endorsed by or affiliated with Microsoft. Halo © Microsoft Corporation. This is a free, non-commercial fan project.

## Where it is

The recompiled game boots to the main menu and runs campaign levels and
multiplayer maps, starting any of them directly, on macOS, Apple TV and
iPhone/iPad. Around it are an Xbox Live replacement with a web app, a map and
variant editor, tools to convert Halo 1's maps into Halo 2's engine, and an
agent API for driving and checking the running game.

Rendering is the part that moved most. A native GPU renderer now draws the
frame from data kept on the GPU, and it is on by default. On an M3 at 4K with
the render scale at 4x, the owner sees a steady 30 fps in the campaign,
dipping when six or more enemies are on screen. The target is a locked 30 at
4x, with 60 and above as the goal. The numbers are below, with what is
measured kept apart from what is only a target or an estimate.

<p align="center">
  <img src="docs/images/readme/oldmombasa-4x.jpg" alt="Old Mombasa at 4x render scale" width="100%">
</p>
<p align="center">
  <img src="docs/images/readme/deltaapproach-4x.jpg" alt="Delta Approach at 4x render scale" width="49%">
  <img src="docs/images/readme/ascension-4x.jpg" alt="Ascension at 4x render scale" width="49%">
</p>

Old Mombasa, Delta Approach and Ascension at 4x, rendered headless from the
author's own disc by [scripts/readme-screenshots.sh](scripts/readme-screenshots.sh).
No game data is in the repo. More shots are in the issue threads:
[boot to the main menu](https://github.com/0xdeafcafe/h2r/issues/1),
[in-game levels](https://github.com/0xdeafcafe/h2r/issues/2).

## Features

**Built**: works in the game or the tools. **Design**: written up, not built. Speed claims are in the next section.

| Area | Feature | State | Docs |
| --- | --- | --- | --- |
| Recompiler | XBE analysis (about 18,700 functions), x86 to IR to C11, clang build with source and object caches | Built | [LIFTER](docs/lifter/LIFTER.md), [cache](docs/lifter/BUILD-CACHE.md) |
| | Native x87 and SSE fast paths, inline guest RAM, direct calls (all on by default) | Built | [x87](docs/lifter/x87-native.md), [memory](docs/perf/guest-memory.md) |
| Runtime | Xbox kernel, XAPI, saves and storage replaced in Rust, on a deterministic logical clock with checkpoints | Built | [BOOT](docs/runtime/BOOT.md), [storage](docs/runtime/storage.md) |
| | Direct3D 8 on wgpu (push buffer, NV2A shaders to WGSL), render thread, merged passes | Built | [d3d](docs/graphics/d3d.md), [graphics](docs/graphics/graphics.md) |
| | DirectSound mixer, native WMA, Bink replacement (cutscenes skipped; optional MP4 conversion) | Built | [audio](docs/audio/audio.md), [movies](docs/audio/movies.md) |
| Player | `h2 play`: window, sound, keyboard, mouse, pads; any level or map from the command line; `--follow` rebuilds as master moves | Built | [app](docs/runtime/app.md), [levels](docs/runtime/level-boot.md) |
| Agent API | `h2 serve` runs the game headless with a JSON-RPC socket; `h2 agent` queries state, drives input, waits on conditions, saves and restores state, replays scenarios | Built | [agent API](docs/runtime/agent-api.md) |
| Sim and hosts | Headless simulation and dedicated hosts: Halo 2 matches without a window, light hosts for friends, bots on a host | Built | [sim](docs/runtime/sim.md), [dedicated host](docs/live/dedicated-host.md) |
| Platforms | macOS (Metal) | Built | [building](docs/runtime/building.md) |
| | Apple TV and iPhone/iPad apps, with touch controls on iOS | Built | [tvOS](docs/runtime/tvos.md), [iOS](docs/runtime/ios.md) |
| | Windows: the runtime and game module cross-build from a Mac; not run yet, no host app | Built | [windows](docs/runtime/windows.md) |
| | Xbox in Developer Mode (UWP app): runs on a Series X and presents frames while the title boots; pads and levels are not verified | Partly built | [xbox](docs/runtime/xbox.md) |
| Graphics | Native GPU renderer, on by default: world, objects, lights, shadows, transparents, decals, particles, fog, water, post, HUD and UI. Scene lists are built in Rust, the guest's opaque handlers are skipped, geometry is cached across frames, and anything it can't take falls back to D3D (`impl.render.native=off` turns it all off) | Built | [native renderer](docs/graphics/native-renderer.md), [switches](docs/graphics/notes/not-adopted.md) |
| | Render scale 1x to 6x (Resolution row), one copy rendered at the scaled size | Built | [high-res](docs/graphics/high-resolution.md) |
| | Pipelined frames: tick N+1 overlapping frame N (off by default, `impl.render.pipelined_frame`) | Built, off | [pipelined](docs/graphics/pipelined-render.md) |
| | E3 2003-style lighting: the Lighting row (sharper shadow maps) | Built | [E3 lighting](docs/graphics/e3-lighting.md) |
| | 60 and 120 fps over the 30 Hz tick: the setting, determinism harness, interpolated replay and reprojection are built and tested on synthetic draws; not yet run in the game | Partly built | [high-framerate](docs/game/high-framerate.md) |
| Game | Expanded settings screens: Video, Controls, FOV | Built | [settings](docs/game/settings-menu.md) |
| | Campaign skulls per profile, a Skulls page, our own skulls | Built | [skulls](docs/game/skulls.md) |
| | 13 community game variants (Zombies, SWAT and more) as built-ins | Built | [variants](docs/game/community-gametypes.md) |
| | Aim assist strength setting | Built | [aim assist](docs/game/aim-assist.md) |
| | Multiplayer bots (Clankers) join hosted matches as phantom peers, and controller ports stay for people; their nav meshes build themselves in the background, again when a map or the generator changes (`H2_BOTS_NAV_AUTO=0` turns that off) | Built | [bots](docs/game/bots.md) |
| | Multiplayer Map Pack installed from your own disc | Built | [map pack](docs/game/map-pack.md) |
| | New, edited and patched maps as packages | Built | [maps](docs/game/maps.md) |
| | More than one campaign: picker and per-campaign progress | Built | [campaigns](docs/halo1/campaign-picker.md) |
| | Halo 1 health model and meter | Built | [health](docs/halo1/health.md) |
| | Theater: `.h2film` films recorded by dedicated hosts, played back with the game's renderer and director cameras (first slice; host verification pending) | Partly built | [theater](docs/game/theater.md) |
| | Forge | Design | [forge](docs/game/forge.md) |
| Halo 1 | Halo 1 campaign maps convert offline into Halo 2 maps and run in the Halo 2 engine, with Halo 1 assets and a Halo 2 setting | Built | [campaign](docs/halo1/campaign.md) |
| | All 13 Halo 1 multiplayer maps convert to Halo 2 maps (collision, BSP, MOPP, objects, shaders) | Built | [h1 multiplayer](docs/halo1/multiplayer.md) |
| | Halo 1 maps, bitmaps, models and animations read and converted to Halo 2 tags | Built | [h1 format](docs/halo1/map-format.md), [models](docs/halo1/models.md) |
| Input | Remappable bindings, input timelines, typing into the game's keyboard | Built | [input](docs/input/input.md) |
| | Button icons that follow the pad (Xbox, PlayStation, keyboard); extra button layouts | Built | [icons](docs/input/button-icons.md), [layouts](docs/input/controller-layouts.md) |
| | DualSense: speaker and mute button | Built | [PlayStation](docs/audio/playstation-audio.md) |
| | Steam Controller | Built | [Steam](docs/input/steam-controller.md) |
| Live | Service: accounts, friends, teams, invites, stats, title files, cloud saves, custom maps, a relay | Built | [live](docs/live/live.md), [API](docs/live/live-api.md) |
| | In the game: sign-in, matchmaking, custom games browser | Built | [matchmaking](docs/live/live-matchmaking.md), [custom games](docs/live/live-custom-games.md) |
| | Co-op campaign over Live | Built | [co-op](docs/live/live-coop.md) |
| | Web app: dashboard, stats, live top-down map, replays, admin | Built | [web](web/README.md), [art](docs/live/live-art.md) |
| | Hosting for friends: Docker, Cloudflare Tunnel, SQLite and R2 | Built | [hosting](docs/live/live-hosting.md) |
| Editor | `h2-editor`: 3D viewport, placements, game variants, playlists, Halo 1 maps, with its own socket API | Built | [guide](docs/editor/editor-guide.md), [API](docs/editor/agent-api.md) |
| Tools | Tag decoders (`h2 bitmap`, `sound`, `ui`, `hs`, ...), map builder, profiler, benchmarks | Built | [formats](docs/README.md), [perf](docs/perf/profiling.md) |

## Speed: measured, target, in flight

Three different things, kept apart. Only the first two tables are numbers
anyone has taken.

### Measured

| What | Before | After | Where |
| --- | --- | --- | --- |
| Native world prep | 5.9 ms/frame | 0.19 ms/frame | Headless Metal, Old Mombasa gameplay, 4x, 2026-11-20 |
| World upload | 3.1 MB/frame | 38 KB/frame | same |
| 1x reference copy | 295 draws | 0 draws | same |
| Guest opaque handlers skipped | 0 | 124 handlers (291 draws) per frame | same |
| Dedicated host, Waterworks, 4 bots | 14.96 ms/tick | 14.12 ms/tick | Five-minute run, same seed, 2026-11-20 |
| Dedicated host, Lockout | 9.07 ms/tick | 8.49 ms/tick | same |
| Campaign frame rate | | steady 30 fps, dips with 6+ enemies | Owner's M3, 4K, 4x |
| Headless gameplay, 600 ticks, native on vs off | 34.3 fps (off) | 32.4 fps (on) | Old Mombasa, 4x, M3, quiet machine, 2026-11-20 |
| Flat guest RAM, Waterworks 2v2 | 10.11 ms/tick (off) | 21.67 ms/tick (on), state diverges at tick 6 | 3600 ticks, same seed, 2026-11-20 |

Two of those rows are regressions. The native renderer (on by default) is still about 6% slower than the D3D path at 4x (its
submit is 1.4 ms/frame dearer, with object and light fallbacks still drawing
through the guest), and flat guest RAM (off by default) doubles the tick
and breaks determinism. Both are being fixed. The host numbers are one run each, and same-build repeatability on Waterworks
is open ([sim-budget.md](docs/perf/sim-budget.md)). The A/B record of the native renderer,
including the rounds that were slower than D3D, is in
[native-ab.md](docs/graphics/notes/native-ab.md).

<p align="center">
  <img src="docs/images/readme/measured-costs.svg" alt="Bar chart of the measured costs before and after" width="100%">
</p>

### Targets (not reached)

| Target | Today |
| --- | --- |
| Locked 30 fps at 4x on an M3, with 60 and above as the goal | 32-34 fps headless, dips with 6+ enemies |
| Sim about 2x faster per tick: Waterworks at 7.48 ms/tick | 14.12 ms/tick |
| One render path: sim, then native render, nothing drawn twice | Native on by default; 0 reference-copy draws in the measured scene, other scenes unchecked |

### In flight (unmeasured)

Built or being built. The figures are estimates, not timings.

| Work | Estimate |
| --- | --- |
| 32 native sim functions replacing guest code | 0.5-0.6 ms/tick for each batch |
| SIMD transform and collision | not estimated |
| Pipelined frames (tick N+1 overlapping frame N) | not estimated |

<p align="center">
  <img src="docs/images/readme/render-pipeline.svg" alt="Render pipeline before and after: guest front-end, D3D translate, two copies; then native scene lists and one native render" width="100%">
</p>

## Roadmap

No dates. Items move when they are measured, not when they are written down.

<p align="center">
  <img src="docs/images/readme/roadmap.svg" alt="Roadmap: now, next, later" width="100%">
</p>

**Now**

- A locked 30 fps at 4x on an M3: the rest of the gameplay frame and the
  residual render callers
  ([native-renderer.md](docs/graphics/native-renderer.md),
  [gameplay-budget.md](docs/perf/notes/gameplay-budget.md)).
- Measure the in-flight sim work (flat guest RAM, the native sim batches) on
  the Waterworks and Lockout cases ([sim-budget.md](docs/perf/sim-budget.md)).
- Fidelity gaps, such as point sprites and the open items in
  [BOOT.md](docs/runtime/BOOT.md) and [shaders.md](docs/graphics/shaders.md).

**Next**

- Pipelined frames on by default, once hash parity holds
  ([pipelined-render.md](docs/graphics/pipelined-render.md)).
- SIMD transform and collision; rewrites of the hot guest families in
  [sim-budget.md](docs/perf/sim-budget.md), toward 7.48 ms/tick on Waterworks.
- One render path: every scene native, the D3D fallback for leftovers only.

**Later**

- 60 and 120 fps over the 30 Hz tick, and the rest of E3 lighting.
- Theater and Forge.
- A Windows and Xbox host app.

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

See [docs/perf/profiling.md](docs/perf/profiling.md).

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
