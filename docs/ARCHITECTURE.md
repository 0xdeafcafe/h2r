# Architecture

Goal: Halo 2 (original Xbox, title `4D530064`) running natively, ending up as
well-structured, mostly safe Rust. The game disc is supplied by the user at
build time and nothing derived from it is committed.

## Targets

- **macOS on arm64 and x86-64, rendering through Metal.** This is the first target.
- **The TV.** Two candidates: Apple TV (tvOS: arm64 and Metal, so close to the
  Mac build) or an Xbox in Developer Mode (x86-64 and DX12). Personal,
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
so a tvOS or Xbox port only swaps that layer.

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
cpal, Bink on a native decoder, and Xbox Live stubbed out.

## Crates

Status "done" means the crate exists on master and does what its row says;
"in progress" means the work is landing elsewhere and is not on master yet.

| crate     | purpose | status |
|-----------|---------|--------|
| `xiso`    | Reads the XDVDFS game partition (plain XISO or full Redump image) | done |
| `xbe`     | Parses the XBE: header, certificate, sections, libraries, kernel thunks | done |
| `analyze` | Disassembly (iced-x86), function discovery, coverage report, per-function CFG export | done |
| `symbols` | Identifies statically linked XDK library functions by OOVPA signature ([xdk-libraries.md](xdk-libraries.md)) | done |
| `map`     | Reader for retail cache v8 `.map` files, with no game data ([map-format.md](map-format.md)) | done |
| `tags`    | Reviewed tag definitions, fixed-record codecs and owned domain types, including sound ([sound.md](sound.md)), localized text ([localization.md](localization.md)) and the effects family ([effects.md](effects.md)) | done |
| `gfx`     | Direct3D 8 replacement on wgpu, plus the texture decoder ([graphics.md](graphics.md)) | done, foundation only |
| `assets`  | Glue from decoded tags to `gfx`; bitmap tags to RGBA/PNG ([bitmaps.md](bitmaps.md)) | done |
| `font`    | Halo 2 packed bitmap font parser and CPU text rasteriser ([fonts.md](fonts.md)) | done |
| `movies`  | Manifest, hashing and naming for optional Bink-to-MP4 conversion with the user's ffmpeg ([movies.md](movies.md)) | done |
| `hs`      | HaloScript scenario tables, builtin catalog and typed expression API; no VM yet ([haloscript.md](haloscript.md)) | done, decoding only |
| `shaders` | Xbox vertex microcode and register-combiner translation to WGSL ([shaders.md](shaders.md)) | done |
| `viewer`  | Render-model and structure BSP viewer and exporters, drawing with `gfx` ([structure-bsp.md](structure-bsp.md)) | done |
| `audio`   | DirectSound replacement foundation ([audio.md](audio.md)) | done, foundation only |
| `dsound`  | Guest-facing DirectSound replacement over `audio` ([dsound.md](dsound.md)) | done |
| `xapi`    | Guest XAPI service replacements over `input` ([xapi.md](xapi.md)) | done |
| `input`   | XInput (XAPI) controller replacement foundation ([input.md](input.md)) | done, foundation only |
| `app`     | Host shell: window, Metal surface, frame presentation, live input, audio device, guest-thread lifecycle ([app.md](app.md)) | done, not wired into `h2 run` yet |
| `vfs`     | Xbox object-manager storage, independent of guest memory ([storage.md](storage.md)) | done |
| `ir`      | Backend-independent semantic IR for lifted code, with a reference interpreter | done for integer, SSE1 and MMX; x87 not yet |
| `lift`    | Lowers decoded x86 to the IR, per instruction and per CFG function ([LIFTER.md](LIFTER.md)) | done for integer, SSE1 and MMX; x87 not yet |
| `emit`    | IR to C11 against the runtime ABI (`h2abi.h`), sharded; clang build glue (output under `target/`, never committed) | done |
| `oracle`  | Independent synthetic x86-32 oracle for checking the lifter ([oracle.md](oracle.md)) | done |
| `runtime` | Guest memory, CPU state, dispatch/override table; nothing runs yet | in progress |
| `engine`  | Hand-written Rust overrides over guest memory: checked tag lookup, datum arrays, localized-string lookup, with per-function adapters ([engine/README.md](engine/README.md)) | in progress; not yet registered with `runtime` |
| `cli`     | `h2` binary: `info`, `ls`, `extract`, `analyze`, `symbols`, `lift`, `emit`, `bitmap`, `sound`, `strings`, `font`, `movies`, `hs`, `effect` and more | done |

Planned crates, not on master yet and in progress elsewhere:

| crate / area | purpose |
|--------------|---------|
| `kernel`     | Rust replacement for the kernel imports ([kernel-imports.md](kernel-imports.md)) |
| `d3d`        | D3D8 API surface over `gfx` |
| `hs` runtime | HaloScript VM over the `hs` decoder |

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

The current corpus is 37,207 fixtures: the original 21,152, plus 16,055
covering every SSE and MMX form the image uses. It gives 36,293 matches, 0
differences and 914 skips. The test prints the skip classes:

| skipped | why |
|--------:|-----|
| 330 | 16-bit address-size string ops (`67` prefix, CX/SI/DI): not lowered |
| 259 | BCD and `salc` (`aaa`, `aad`, `aam`, `aas`, `daa`, `das`, `salc`): not lowered |
| 185 | refused by the oracle (`popfd`, `pause`) |
| 66  | x87 arithmetic (`fld1`, `faddp`): not lowered yet |
| 37  | `cmpxchg8b`: not lowered |
| 37  | 16-bit addressing (`lea` with `67`): not lowered |

The emitter's differential test (`crates/emit/tests/differential.rs`) runs
synthetic functions through the interpreter and through generated C from
random states. The atomics in generated C go through the runtime's
operation-wide accessors (`crates/emit/RUNTIME-CONTRACT.md`).

## Milestones

1. Parse the disc and XBE. *(done)*
2. Discover functions until coverage of `.text` is close to 100%.
3. Lift the code and run from the entry point to the first kernel call.
4. Replace the kernel imports and reach D3D device creation.
5. Draw the first frame (`mainmenu.map`) through the D3D replacement.
