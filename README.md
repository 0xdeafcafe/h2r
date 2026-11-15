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
(`H2_FASTMEM`, [docs/performance.md](docs/performance.md)), which cuts the
menu's CPU time per frame by about 35%; native binary64 x87
(`H2_X87_NATIVE`, [docs/x87-native.md](docs/x87-native.md)); and
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
   [docs/BOOT.md](docs/BOOT.md) and [docs/shaders.md](docs/shaders.md)
   ("Gaps").
2. 30 fps. The levers, measured in
   [docs/perf-budget.md](docs/perf-budget.md): faster guest memory access
   (the first step is in, on by default), merged render passes, native x87
   (on by default) and
   push-buffer decode cost. Draw batching, upload rings, IR optimisation and
   sampling render-to-texture levels in place have landed. See
   [docs/perf.md](docs/perf.md) for the tools.
3. More campaign levels.
4. Opt-in enhancements, off by default and later exposed in an expanded
   in-game settings menu ([docs/settings-menu.md](docs/settings-menu.md)):
   - 60 and 120 fps, decoupling rendering from the game's 30 Hz tick.
   - 4K and other high-resolution rendering
     ([docs/high-resolution.md](docs/high-resolution.md)).
   - Recovering the E3 2003 stencil-shadow lighting engine
     ([docs/e3-lighting.md](docs/e3-lighting.md)).
5. Later: a map editing kit, and TV targets (see
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

Design and pipeline:

- [ARCHITECTURE.md](docs/ARCHITECTURE.md): goals, the two-stage plan, crate table, milestones
- [LIFTER.md](docs/LIFTER.md): the lifter's original design proposal
- [FLOAT.md](docs/FLOAT.md): floating point in the lifter: what the game uses and the correctness-first plan
- [FLOAT-ABI.md](docs/FLOAT-ABI.md): the x87 IR ops and `h2x87_v1_*` runtime helpers (`crates/x87`)
- [x87-native.md](docs/x87-native.md): the native binary64 x87 path, on by default (`H2_X87_NATIVE=0` disables it): range proof, status bits, fallbacks
- [render-thread.md](docs/render-thread.md): the render thread (on by default, `H2_RENDER_THREAD=0` turns it off): which thread renders, what the guest can observe, measured gain
- [simd.md](docs/simd.md): bit-exact SSE1 reference semantics (`crates/simd`)
- [oracle.md](docs/oracle.md): independent x86 instruction oracle on Unicorn (`crates/oracle`)
- [sse-oracle.md](docs/sse-oracle.md): SSE/MMX results measured on Rosetta 2 (`crates/sse-oracle`)
- [golden-trace.md](docs/golden-trace.md): golden boot traces from Unicorn (design; the code is not on master)
- [dispatch-coverage.md](docs/dispatch-coverage.md): indirect-dispatch coverage of the campaign's main tables
- [xdk-libraries.md](docs/xdk-libraries.md): XDK library identification (`h2 symbols`)
- [BUILD-CACHE.md](docs/BUILD-CACHE.md): generated-source and object caches, iteration builds
- [build-perf.md](docs/build-perf.md): build-time measurements behind those caches

Running the game:

- [BOOT.md](docs/BOOT.md): `h2 build` and `h2 run`, the boot runner's flags and environment, current state and blockers
- [app.md](docs/app.md): the host shell, `h2 play` and `h2 play --follow` (`crates/app`)
- [input.md](docs/input.md): XInput replacement, bindings and input timelines (`crates/input`); [input-default.toml](docs/input-default.toml) is the default binding file and [input-menu-example.toml](docs/input-menu-example.toml) an example timeline
- [settings-menu.md](docs/settings-menu.md): design for an expanded in-game settings menu for the opt-in enhancements (not built)
- [title-updates.md](docs/title-updates.md): supporting title-update XBEs (1.1 and later): what assumes the 1.0 image, and the address-map design (not implemented)
- [map-pack.md](docs/map-pack.md): the Multiplayer Map Pack disc: its content packages, the title update it carries, and `h2 import-mappack` (the maps need the update, which doesn't run yet)
- [kernel-imports.md](docs/kernel-imports.md): work list of every kernel import Halo 2 uses
- [storage.md](docs/storage.md): Xbox storage boundary (`crates/vfs`)
- [xapi.md](docs/xapi.md): guest XAPI service replacements (`crates/xapi`)
- [checkpoints.md](docs/checkpoints.md): checkpoint save and revert acceptance test
- [cache-slots.md](docs/cache-slots.md): campaign cache-slot integration test
- [level-preflight.md](docs/level-preflight.md): static audit of what the first campaign levels need
- [perf.md](docs/perf.md): sampling profiler, timeline traces and benchmarks (`h2 profile`, `h2 bench`, `crates/perf`, `crates/timeline`)
- [fastmem-proto.md](docs/fastmem-proto.md): prototype write watch on Mach exception ports, and 4 KiB guest pages on 16 KiB host pages (`crates/fastmem-proto`)
- [fastmem-4k.md](docs/fastmem-4k.md): measured 4 KiB guest translation and write-bit variants; slower than phase-1 inline access, so not adopted
- [perf-budget.md](docs/perf-budget.md): where a main-menu frame's time goes, and the plan to 30 fps
- [frame-budget.md](docs/frame-budget.md): an in-level frame budget, the worst frames and their causes, and what a worst frame under 33 ms needs
- [high-framerate.md](docs/high-framerate.md): design for opt-in 60 and 120 fps that keeps the 30 Hz tick exact (super bounces included): host-side interpolated replay, prior art, determinism test (not built)
- [performance.md](docs/performance.md): guest memory accessors, the inline RAM path (`H2_FASTMEM`, on by default) and runtime counters

Graphics:

- [d3d.md](docs/d3d.md): the guest-facing Direct3D 8 replacement: entries, device model, push buffer, render targets, vblank (`crates/d3d`)
- [graphics.md](docs/graphics.md): D3D8 state and textures on wgpu (`crates/gfx`)
- [shaders.md](docs/shaders.md): NV2A vertex microcode, register combiners, fog and shadow maps to WGSL (`crates/shaders`)
- [high-resolution.md](docs/high-resolution.md): design for opt-in 4K and other internal resolutions: which surfaces scale, what the guest may observe, tests, prior art (not implemented)
- [e3-lighting.md](docs/e3-lighting.md): design for opt-in E3 2003-style stencil shadows and per-pixel lighting: the evidence, what retail kept, phases and risks (not implemented)

Audio and movies:

- [audio.md](docs/audio.md): DirectSound replacement: voices, mixer, effects, output (`crates/audio`)
- [dsound.md](docs/dsound.md): guest-facing DirectSound shim (`crates/dsound`)
- [wma.md](docs/wma.md): ASF demuxing and native WMAv2 decoding (`crates/asf`, `crates/wma`)
- [bink-hle.md](docs/bink-hle.md): high-level Bink player replacement (`crates/bink-hle`)
- [movies.md](docs/movies.md): optional Bink movie conversion with your own ffmpeg (`h2 movies`)

Data formats and tags:

- [map-format.md](docs/map-format.md): cache file (v8) format and the `map` reader
- [animation.md](docs/animation.md): animation graphs (jmad) and frame decompression (`crates/tags`)
- [bitmaps.md](docs/bitmaps.md): bitmap tags to textures and PNG (`crates/assets`)
- [collision.md](docs/collision.md): collision and physics model tags (`crates/tags`)
- [effects.md](docs/effects.md): effects-family tags (effe, prt3, pmov and more) (`crates/tags`)
- [fonts.md](docs/fonts.md): packed bitmap fonts and text rasteriser (`crates/font`)
- [haloscript.md](docs/haloscript.md): HaloScript syntax, builtins and decoding (`crates/hs`)
- [lighting.md](docs/lighting.md): level lighting, lightmaps and atmosphere (`crates/tags`)
- [localization.md](docs/localization.md): string IDs and localized text (`crates/tags`)
- [objects.md](docs/objects.md): object tags (obje family, hlmt, itmc, vehc) (`crates/tags`)
- [shader-instances.md](docs/shader-instances.md): shader instance, template and pass tags (shad, stem, spas) (`crates/tags`)
- [sound.md](docs/sound.md): sound tags (`snd!`, `ugh!`)
- [structure-bsp.md](docs/structure-bsp.md): render models and structure BSP geometry, and the viewer (`crates/tags`, `crates/viewer`)
- [ui.md](docs/ui.md): user-interface tags (wgit, wgtz, wigl, skin) (`crates/tags`)
- [keyboard-charset.md](docs/keyboard-charset.md): the on-screen keyboard's characters, what players use, and a design for extending them (not built)

Engine:

- [engine-map.md](docs/engine-map.md): survey of the retail engine and where to start
- [engine/README.md](docs/engine/README.md): contracts for the engine functions rewritten in Rust (`crates/engine`), and the HaloScript VM ([engine/hs_vm.md](docs/engine/hs_vm.md))
