# Architecture

Goal: Halo 2 (original Xbox, title `4D530064`) running natively, ending up as
well-structured, mostly safe Rust. The game disc is supplied by the user at
build time and nothing derived from it is committed.

## Targets

- **macOS on arm64 and x86-64, rendering through Metal.** This is the first target.
- **Apple TV and iPhone/iPad.** tvOS and iOS share the Mac's arm64 and Metal
  path; the UIKit hosts are in `platform/tvos` and `platform/ios`
  ([tvos.md](runtime/tvos.md), [ios.md](runtime/ios.md)).
- **Windows, then Xbox in Developer Mode** (x86-64 and DX12). The runtime and
  the generated module cross-compile for Windows from a Mac; there is no
  Windows host app yet ([windows.md](runtime/windows.md)). Personal,
  sideloaded use only.
- **Rendering goes through wgpu,** which covers Metal, Vulkan and DX12 with
  one renderer.

The recompiled code is portable C or LLVM IR, so arm64 and x86-64 builds
take the same path. Nothing relies on running the original x86 directly.

## What "done" means

Stage 1 below is scaffolding. The real product is stage 2: an engine in
readable Rust that you can extend and build editors on, using the
recompiled code as the reference it's tested against. So rewrite order
favours the parts that editing and modding touch:

1. Tag and cache-file (`.map`) loading, as a typed Rust data model. Editors
   build on this.
2. The renderer, above the D3D layer: start with D3D8 replaced on wgpu to get
   pixels, then rewrite Halo's own rendering layer natively.
3. Objects, game state and the HaloScript runtime.
4. Physics and AI, last. Their floating-point behaviour needs the most care.

Platform code (window, input, audio, file access) lives in its own layer,
so a tvOS, iOS or Xbox port only swaps that layer (docs/runtime/tvos.md,
docs/runtime/ios.md; Windows in docs/runtime/windows.md, the Xbox app in
docs/runtime/xbox.md).

## The path to good Rust

Going straight from x86 to idiomatic Rust is not something a tool can do. The
game is about 3.4 MB of machine code, and a mechanical translation will always
be ugly. So the project runs in two stages that overlap:

1. **Mechanical recompile (correct, ugly).** A lifter turns every function in
   `.text` into generated code (LLVM IR or C, compiled and linked as a library).
   Guest memory is one flat 64 MB region, registers live in a `Cpu` struct, and
   every function has the signature `fn(&mut Cpu, &mut Guest)`. This stage only
   has to be right, not readable. It gets regenerated from the disc and is
   never edited by hand.

2. **Function-by-function rewrite (readable, typed).** Every guest function
   address goes through a dispatch table. A hand-written Rust function can
   register itself as an override for an address, and from then on calls go to
   it instead of the generated version. This is the same approach decomp
   projects use: the game runs at every step, and each replacement is small and
   can be backed out.

What makes stage 2 safe:

- **Differential testing.** Calls to a function can be recorded (input
  registers, the guest memory it reads, the memory it writes, its return
  value) while playing the recompiled build. Each override is replayed
  against those recordings and has to match before it ships. For floating
  point the bar is matching bits, or a tolerance documented per function.
- **Switch at runtime.** Overrides are enabled per function from config, so a
  regression can be bisected down to a single function.
- **Work up from the leaves.** Start with leaf functions such as math, string
  handling and the tag system. A caller is rewritten only after its callees
  are, so typed Rust grows upward through the call graph.
- **Typed memory in stages.** Overrides start out reading guest memory through
  `Guest` accessors. Once a structure is understood (the Halo 2 tag and map
  formats are well documented), it becomes a `#[repr(C)]` view type, and
  later on Rust-owned data with the guest copy kept in sync at the boundary.
  The `unsafe` code ends up confined to `runtime::guest`.

Statically linked SDK libraries (D3D, DSOUND, XONLINE, XNET, BINK, ...) are
not recompiled. Their functions are found by signature and replaced at the
dispatch table with native Rust implementations: D3D on wgpu, DSound on
cpal, Bink by a high-level player ([bink-hle.md](audio/bink-hle.md)), and Xbox
Live by client HLE over our own service ([live.md](live/live.md)).

## Crates

Every crate below is on master. "Done" means it does what its row says;
"in progress" means it is there but incomplete.

| crate     | purpose | status |
|-----------|---------|--------|
| `xiso`    | Reads the XDVDFS game partition (plain XISO or full Redump image) | done |
| `xbe`     | Parses the XBE: header, certificate, sections, libraries, kernel thunks | done |
| `analyze` | Disassembly (iced-x86), function discovery, coverage report, per-function CFG export | done |
| `symbols` | Identifies statically linked XDK library functions by OOVPA signature ([xdk-libraries.md](lifter/xdk-libraries.md)) | done |
| `map`     | Reader for retail cache v8 `.map` files, with no game data ([map-format.md](formats/map-format.md)) | done |
| `tags`    | Reviewed tag definitions, fixed-record codecs and owned domain types, including sound ([sound.md](formats/sound.md)), localized text ([localization.md](formats/localization.md)) and the effects family ([effects.md](formats/effects.md)) | done |
| `gfx`     | Direct3D 8 replacement on wgpu, plus the texture decoder ([graphics.md](graphics/graphics.md)) | done, foundation only |
| `assets`  | Glue from decoded tags to `gfx`; bitmap tags to RGBA/PNG ([bitmaps.md](formats/bitmaps.md)) | done |
| `font`    | Halo 2 packed bitmap font parser and CPU text rasteriser ([fonts.md](formats/fonts.md)) | done |
| `movies`  | Manifest, hashing and naming for optional Bink-to-MP4 conversion with the user's ffmpeg ([movies.md](audio/movies.md)) | done |
| `hs`      | HaloScript scenario tables, builtin catalog, typed expression API, and a native VM ([haloscript.md](formats/haloscript.md), [engine/hs_vm.md](engine/hs_vm.md)) | decoding done; the VM is not wired into the game yet |
| `shaders` | Xbox vertex microcode and register-combiner translation to WGSL ([shaders.md](graphics/shaders.md)) | done |
| `viewer`  | Render-model and structure BSP viewer and exporters, drawing with `gfx` ([structure-bsp.md](formats/structure-bsp.md)) | done |
| `audio`   | DirectSound replacement: voices, mixer, effects, output ([audio.md](audio/audio.md)) | done |
| `dsound`  | Guest-facing DirectSound replacement over `audio` ([dsound.md](audio/dsound.md)) | done |
| `xapi`    | Guest XAPI service replacements over `input` ([xapi.md](runtime/xapi.md)) | done |
| `asf`     | ASF demuxer for the retail sound permutations ([wma.md](audio/wma.md)) | done |
| `wma`     | Native WMAv2 decoder ([wma.md](audio/wma.md)) | done |
| `bink-hle` | High-level replacement for the statically linked Bink player ([bink-hle.md](audio/bink-hle.md)) | done, on by default: plays the movies `h2 movies` installed (sound through the game's mixer), skips them otherwise (`--no-movies`; `h2 run --no-bink` turns it off) |
| `input`   | XInput (XAPI) controller replacement foundation ([input.md](input/input.md)) | done, foundation only |
| `app`     | Host shell: window, Metal surface, frame presentation, live input, audio device, guest-thread lifecycle ([app.md](runtime/app.md)) | done; `h2 play` runs the game in it |
| `vfs`     | Xbox object-manager storage, independent of guest memory ([storage.md](runtime/storage.md)) | done |
| `ir`      | Backend-independent semantic IR for lifted code, with a reference interpreter | done for integer, x87, SSE1 and MMX |
| `lift`    | Lowers decoded x86 to the IR, per instruction and per CFG function ([LIFTER.md](lifter/LIFTER.md)) | done for integer, x87, SSE1 and MMX |
| `x87`     | Exact software x87 (ext80) helpers that lifted x87 code calls ([FLOAT-ABI.md](lifter/FLOAT-ABI.md)) | done |
| `simd`    | Bit-exact SSE1 reference semantics, in Rust and C ([simd.md](lifter/simd.md)) | done |
| `emit`    | IR to C11 against the runtime ABI (`h2abi.h`), sharded; clang build glue (output under `target/`, never committed) | done |
| `oracle`  | Independent synthetic x86-32 oracle for checking the lifter ([oracle.md](lifter/oracle.md)) | done |
| `sse-oracle` | SSE1/MMX results from x86-64 hardware under Rosetta 2 ([sse-oracle.md](lifter/sse-oracle.md)) | done |
| `runtime` | Guest memory, CPU state, dispatch/override table and the runner that drives generated code ([runtime README](../crates/runtime/README.md)) | done |
| `kernel`  | Xbox kernel HLE: threads, scheduling on a logical clock, sync, memory, files ([kernel-imports.md](runtime/kernel-imports.md), [kernel README](../crates/kernel/README.md)) | in progress: covers the boot to the menu and the first level |
| `d3d`     | Guest-facing Direct3D 8 over `gfx`, including push-buffer execution ([d3d.md](graphics/d3d.md)) | in progress: the menu and the first level render |
| `boot`    | Builds the whole XBE into a dylib and runs it with every HLE crate registered ([BOOT.md](runtime/BOOT.md)) | done |
| `engine`  | Hand-written Rust overrides over guest memory: checked tag lookup, datum arrays, localized-string lookup, with per-function adapters ([engine/README.md](engine/README.md)) | in progress; the game installs the leaves through the `engine-leaves` feature (locale group on by default) |
| `timeline` | Dependency-free timeline events (frames, slices, waits, interrupts, drains, submits) to Chrome/Perfetto JSON; one atomic load per hook when off ([perf.md](perf/perf.md)) | done |
| `perf`    | Guest-aware sampling profiler, timeline sessions, benchmark harness behind `h2 profile` and `h2 bench` ([perf.md](perf/perf.md)) | done |
| `cli`     | `h2` binary: `build`, `run`, `play`, `info`, `ls`, `extract`, `analyze`, `symbols`, `lift`, `emit`, the tag decoders and more (`cargo h2` lists them) | done |
| `opt`     | IR-to-IR optimisation passes between lifting and emission | done |
| `feature` | The feature registry: named points, hook chains, switches, the frame and game bridges ([feature-engine.md](engine/feature-engine.md)) | in progress |
| `settings` | Host settings as a schema: definitions, one store, one writer for `settings.toml` ([feature-engine.md](engine/feature-engine.md), "Settings") | done |
| `lockstep` | Determinism harness: per-tick guest-RAM hashes, run traces and their comparison ([high-framerate.md](game/high-framerate.md)) | done |
| `content` | Map packages: manifest, `MapId`, PPF/IPS/BPS patches and the resolver ([maps.md](game/maps.md)) | foundation built |
| `studio`  | `h2-editor`, the native editor suite (Dear ImGui + wgpu): scenario, game variant and playlist tools ([editor.md](editor/editor.md)) | in progress |
| `editor-core` | The editor's UI-free core: map sources, the undo stack, where variants come from ([editor.md](editor/editor.md)) | in progress |
| `hopper-edit` | The playlist (`multiplayer_hopper.dat`) editor's logic and session ([editor.md](editor/editor.md)) | in progress |
| `scenario-edit` | Sapien-style scenario placement editor writing patched `.map` files ([scenario-editor.md](editor/scenario-editor.md)) | in progress |
| `variants` | Game variants: the 0x130-byte record, schema, saved files, editor state ([variants.md](editor/variants.md)) | in progress |
| `live-proto` | Wire types, signing and verification shared by the Live clients and server ([live-api.md](live/live-api.md)) | in progress |
| `live-server` | The Xbox Live replacement service: signed events on Postgres behind axum ([live.md](live/live.md)) | in progress |
| `live-hle` | Xbox Live public-entry overrides in the game ([live-boundary.md](live/live-boundary.md)) | in progress, opt-in |
| `live-telemetry` | Guest game-state observation and live ingest ([live-telemetry.md](live/live-telemetry.md)) | in progress |
| `coop`    | Co-op over Live, phase 0: peer transport, pad schedule, per-tick hash exchange ([live-coop.md](live/live-coop.md)) | in progress |
| `apple-host` | The tvOS and iOS apps' Rust half: a static library the UIKit hosts in `platform/tvos` and `platform/ios` call to boot the game ([tvos.md](runtime/tvos.md), [ios.md](runtime/ios.md)) | in progress |
| `uwp-host` | The Xbox (Dev Mode) UWP app, `h2uwp.exe`, and its packaging helpers (`platform/xbox`, [xbox.md](runtime/xbox.md)) | in progress: boots on a Series X (DX12, audio); out of memory in App mode |
| `cloud-saves` | Cloud saves on the Live service: merge an account's saves at sign-in, push changes as the game writes them ([live-cloud-saves.md](live/live-cloud-saves.md)) | in progress |
| `live-menubar` | The optional menu bar app for a Live server run by launchd (`h2 live menubar`; [live-hosting.md](live/live-hosting.md)) | done |
| `h1map`   | Halo 1 Xbox cache (v5) reader ([h1-map-format.md](formats/h1-map-format.md)) | done |
| `h1bitmaps` | Halo 1 bitmaps decoded to RGBA8 and converted to Halo 2 `bitm` tags ([h1-bitmaps.md](formats/h1-bitmaps.md)) | done |
| `h1models` | Halo 1 render models and animation graphs as Halo 2 tags ([h1-models.md](formats/h1-models.md)) | in progress |
| `h1convert` | Halo 1 multiplayer maps converted to Halo 2 tags: collision, shaders, objects ([h1-multiplayer.md](game/h1-multiplayer.md)) | in progress: all 13 disc maps convert |
| `mopp`    | Havok MOPP code for structure BSPs: the engine's interpreter as a query model, and an offline compiler ([mopp.md](formats/mopp.md)) | done |
| `pathfind` | AI pathfinding data: decode retail, generate from collision geometry, compare, emit `map::build` blocks ([pathfinding.md](formats/pathfinding.md)) | in progress |
| `fastmem-proto` | Isolated Mach exception write-watch prototype ([fastmem-proto.md](perf/fastmem-proto.md)) | prototype, not used by the runtime |

### Lifter checks

The lowering is cross-checked against the `oracle` crate's Unicorn-backed
fixtures (synthetic encodings, no game data): `cargo test -p lift --features
oracle-fixtures --test oracle` with `H2_ORACLE_FIXTURES` naming the `.jsonl`
files. Registers, EIP, XMM, x87, MXCSR, memory and the stop must match, and
so must which status flags are defined. The harness itself has
self-contained tests (no fixture files): state import, every comparison,
defined-flag handling and skip counting.

Two documented accommodations apply. After an MMX transition, Unicorn
(QEMU) leaves bits 64-79 of a written MMX register unchanged and reports a
content-classified tag word, where the SDM sets those bits to ones and the
transition marks every tag valid. `rcp*`/`rsqrt*` lanes are held to the
architectural bound (relative error at most 1.5 * 2^-12; a denormal input
acts as a signed zero and a tiny result flushes to zero) rather than to
exact bits, because QEMU computes exact quotients where the Pentium III
uses tables. Unicorn also never sets MXCSR's exception flags, so after SSE
arithmetic only MXCSR's control bits are compared. `crates/simd` checks
the flags against hardware with its own oracle corpus.

The corpus was 37,207 fixtures when last counted: the original 21,152, plus
16,055 covering every SSE and MMX form the image uses. It gave 36,293
matches, 0 differences and 914 skips. These counts predate x87 lowering
(e5c4251) and have not been redone since: the x87 skips below should now be
matches, and the fixture files come from `oracle-fixtures`
(docs/lifter/oracle.md), so the totals may differ too. The test prints the skip
classes (`--nocapture`):

| skipped | why |
|--------:|-----|
| 330 | 16-bit address-size string ops (`67` prefix, CX/SI/DI): not lowered |
| 259 | BCD and `salc` (`aaa`, `aad`, `aam`, `aas`, `daa`, `das`, `salc`): not lowered |
| 185 | refused by the oracle (`popfd`, `pause`) |
| 66  | x87 arithmetic (`fld1`, `faddp`): skipped before x87 was lowered (e5c4251); expected to be matches now |
| 37  | `cmpxchg8b`: not lowered |
| 37  | 16-bit addressing (`lea` with `67`): not lowered |

The emitter's differential test (`crates/emit/tests/differential.rs`) runs
synthetic functions through the interpreter and through generated C from
random states. The atomics in generated C go through the runtime's
operation-wide accessors (`crates/emit/RUNTIME-CONTRACT.md`).

## Milestones

1. Parse the disc and XBE. *(done)*
2. Discover functions until coverage of `.text` is close to 100%. *(enough
   for the boot; a few holed jump tables still go through the fallback
   interpreter, see [BOOT.md](runtime/BOOT.md))*
3. Lift the code and run from the entry point to the first kernel call. *(done)*
4. Replace the kernel imports and reach D3D device creation. *(done)*
5. Draw the first frame (`mainmenu.map`) through the D3D replacement. *(done)*
6. Reach and render the first campaign level. *(done: The Heretic renders
   past 8,000 frames)*
7. 30 fps ([perf-budget.md](perf/perf-budget.md)).
