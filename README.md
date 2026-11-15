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
  kernel, xapi, D3D (NV2A push-buffer emulation onto wgpu/Metal) and
  DirectSound with a WMA decoder, to the real main menu with its 3D scene
  and blue tint.
- **Into the campaign.** Scripted input clicks through the menus into the
  first level, The Heretic (`00a_introduction`). It renders past 8,000
  frames. Shadow maps are still skipped.
- **Windowed player.** `h2 play` runs with keyboard, gamepad and sound.
- **Tooling.** Deterministic logical-time scheduling, checkpoints, a sampling
  profiler and a benchmark tool.

Speed is the big gap: the main menu runs at about 1.9 fps today. The target
is 30 fps.

### What is on master

Master has the analyser, lifter, emitter (`h2 emit`), runtime, kernel,
xapi, D3D and DirectSound crates, the tag and map readers, the boot runner
(`h2 build`, `h2 run`), the windowed player (`h2 play`, below),
logical-time scheduling, checkpoints, and the profiler and benchmarks
(`h2 profile`, `h2 bench`).

In review, landing soon: depth-texture support in the D3D layer (`d3d-r8`
branch).

## Proof

Screenshots are kept in issue threads, not in the repo:

- [Boot to the main menu](https://github.com/0xdeafcafe/h2r/issues/1)
- [In-game levels](https://github.com/0xdeafcafe/h2r/issues/2)

## Work left

In priority order:

1. Fidelity gaps: shadow maps and the like.
2. 30 fps. The planned levers are runtime fast paths, GPU batching (a branch
   already roughly halves CPU time per frame), IR optimisation, native
   overrides of hot functions, and D3D decode cost. See
   [docs/perf.md](docs/perf.md).
3. More campaign levels.
4. Later: a map editing kit, and TV targets (see
   [docs/app.md](docs/app.md)).

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
[docs/BUILD-CACHE.md](docs/BUILD-CACHE.md). `cargo h2` with no arguments
prints the full command list, including the tag decoders (`tag`, `bitmap`,
`sound`, `strings`, `ui`, `hs` and more).

Profiling and benchmarks need the `perf` feature (the alias doesn't pass it):

```sh
cargo run --release -p cli --features perf -- bench list
cargo run --release -p cli --features perf -- bench --only hot-loop
cargo run --release -p cli --features perf -- profile hot-loop/memory --seconds 5 --out $TMPDIR/prof
```

See [docs/perf.md](docs/perf.md).

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
([docs/input.md](docs/input.md)).

To keep playing while master moves, add `--follow`:

```sh
cargo h2 play --follow disc/halo2.iso
```

It fetches `origin/master` every 45 s and builds each new commit in the
background under `~/Library/Caches/h2recomp/follow`; your checkout is never
touched. A banner in the window shows progress. Press F5 on "Update ready"
to restart into the new build, or F6 to hide the banner. See
[docs/app.md](docs/app.md) for every flag.

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

- [ARCHITECTURE.md](docs/ARCHITECTURE.md): goals, the two-stage plan, crate table, milestones
- [BUILD-CACHE.md](docs/BUILD-CACHE.md): generated-source and object caches, iteration builds
- [animation.md](docs/animation.md): animation graphs (jmad) and frame decompression (`crates/tags`)
- [audio.md](docs/audio.md): DirectSound replacement (`crates/audio`)
- [bitmaps.md](docs/bitmaps.md): bitmap tags to textures and PNG (`crates/assets`)
- [bink-hle.md](docs/bink-hle.md): high-level Bink player replacement (`crates/bink-hle`)
- [collision.md](docs/collision.md): collision and physics model tags (`crates/tags`)
- [dsound.md](docs/dsound.md): guest-facing DirectSound shim (`crates/dsound`)
- [effects.md](docs/effects.md): effects-family tags (effe, prt3, pmov and more) (`crates/tags`)
- [engine-map.md](docs/engine-map.md): survey of the retail engine and where to start
- [fonts.md](docs/fonts.md): packed bitmap fonts and text rasteriser (`crates/font`)
- [graphics.md](docs/graphics.md): D3D8 on wgpu (`crates/gfx`)
- [haloscript.md](docs/haloscript.md): HaloScript syntax, builtins and the VM (`crates/hs`)
- [input.md](docs/input.md): XInput replacement (`crates/input`)
- [kernel-imports.md](docs/kernel-imports.md): kernel imports work list
- [LIFTER.md](docs/LIFTER.md): lifter design proposal
- [localization.md](docs/localization.md): string IDs and localized text (`crates/tags`)
- [map-format.md](docs/map-format.md): cache file (v8) format and the `map` reader
- [movies.md](docs/movies.md): optional Bink movie conversion with your own ffmpeg (`h2 movies`)
- [oracle.md](docs/oracle.md): independent x86 instruction oracle
- [perf.md](docs/perf.md): sampling profiler, timeline traces and benchmarks (`h2 profile`, `h2 bench`, `crates/perf`, `crates/timeline`)
- [shaders.md](docs/shaders.md): NV2A vertex microcode and register combiners to WGSL (`crates/shaders`)
- [structure-bsp.md](docs/structure-bsp.md): render models and structure BSP geometry, and the viewer (`crates/tags`, `crates/viewer`)
- [sound.md](docs/sound.md): sound tags (`snd!`, `ugh!`)
- [storage.md](docs/storage.md): Xbox storage boundary (`crates/vfs`)
- [ui.md](docs/ui.md): user-interface tags (wgit, wgtz, wigl, skin) (`crates/tags`)
- [xapi.md](docs/xapi.md): guest XAPI service replacements (`crates/xapi`)
- [xdk-libraries.md](docs/xdk-libraries.md): XDK library identification
