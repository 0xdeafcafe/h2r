# h2recomp

A static recompilation of Halo 2 (original Xbox) to native code, with the
runtime written in Rust. The retail executable is analysed, lifted to IR,
emitted as C11 and built by clang into a dylib. `crates/runtime` runs that
dylib against high-level replacements for the Xbox kernel, xapi, Direct3D
and DirectSound, rendering through wgpu on Metal.

You need your own legally obtained copy of the disc. No game data, extracted
assets or screenshots live in this repo.

Public repo: <https://github.com/0xdeafcafe/h2r>.

## Status

What works today:

- **Analysis and lifting.** The retail XBE is analysed (about 18,700
  functions and 504 jump tables), lifted to IR and emitted as C11, which clang
  builds into a dylib.
- **Boot to the main menu.** The recompiled game boots through the HLE
  kernel, xapi, D3D (replaced at the API level, push buffer included, on
  wgpu/Metal) and DirectSound with a WMA decoder, to the real main menu
  with its 3D scene and blue tint.
- **Into the campaign.** Scripted input clicks through the menus into the
  first level, The Heretic (`00a_introduction`). It renders past 8,000
  frames, with shadow maps: one run there took 50,207 shadow-map samples
  and skipped none.
- **Windowed player.** `h2 play` runs with keyboard, gamepad and sound.
- **Tooling.** Deterministic logical-time scheduling, checkpoints, a sampling
  profiler and a benchmark tool.

Speed is the big gap. On current master the main menu runs at about 5-6 fps
on an unloaded M-series Mac. The target is 30 fps. Three speedups are on by
default for `h2 build`, `h2 run` and `h2 play`: the inline RAM path
(`H2_FASTMEM`, [docs/perf/performance.md](docs/perf/performance.md)), which cuts the
menu's CPU time per frame by about 35%; native binary64 x87
(`H2_X87_NATIVE`, [docs/lifter/x87-native.md](docs/lifter/x87-native.md)); and
register-local access accounting (`H2_ACCESS_COUNT`). Set any of them to `0`
to build without it.

### What is on master

Master has the analyser, lifter, emitter (`h2 emit`), runtime, kernel,
xapi, D3D and DirectSound crates, the tag and map readers, the boot runner
(`h2 build`, `h2 run`), the windowed player (`h2 play`, below),
logical-time scheduling, checkpoints, and the profiler and benchmarks
(`h2 profile`, `h2 bench`).

In progress for 30 fps: the rest of fast guest memory access, merged
render passes by default, and native x87.

## Proof

Screenshots are kept in issue threads, not in the repo:

- [Boot to the main menu](https://github.com/0xdeafcafe/h2r/issues/1)
- [In-game levels](https://github.com/0xdeafcafe/h2r/issues/2)

## Work left

In priority order:

1. Fidelity gaps, such as point sprites and the open items in
   [docs/runtime/BOOT.md](docs/runtime/BOOT.md) and [docs/graphics/shaders.md](docs/graphics/shaders.md)
   ("Gaps").
2. 30 fps. The levers, measured in
   [docs/perf/perf-budget.md](docs/perf/perf-budget.md): faster guest memory access
   (the first step is in, on by default), merged render passes, native x87
   (on by default) and
   push-buffer decode cost. Draw batching, upload rings, IR optimisation and
   sampling render-to-texture levels in place have landed. See
   [docs/perf/perf.md](docs/perf/perf.md) for the tools.
3. More campaign levels.
4. Opt-in enhancements, off by default and later exposed in an expanded
   in-game settings menu ([docs/game/settings-menu.md](docs/game/settings-menu.md)):
   - 60 and 120 fps, decoupling rendering from the game's 30 Hz tick.
   - 4K and other high-resolution rendering
     ([docs/graphics/high-resolution.md](docs/graphics/high-resolution.md)).
   - Recovering the E3 2003 stencil-shadow lighting engine
     ([docs/graphics/e3-lighting.md](docs/graphics/e3-lighting.md)).
5. Later: a map editing kit, and TV targets (see
   [docs/runtime/app.md](docs/runtime/app.md)).

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
