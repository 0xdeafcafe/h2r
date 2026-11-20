# Iteration builds and caches

Status: Built.
Code: `crates/recompiler/emit/src/compile.rs` (object cache, `obj_dir_with`, `prune_obj_cache`), `crates/apps/boot/src/tier.rs`, `h2 emit` and `h2 build` in `crates/apps/cli/src/main.rs`. Tests: `crates/recompiler/emit/src/compile.rs` (unit tests), `crates/apps/boot/src/tier.rs`.
Switch: `H2_OBJ_CACHE`, `H2_OBJ_CACHE_MAX`, `H2_OPT`, `H2_DEBUG`, `H2_CPU`, `H2_PGO*`, `H2_HOT`, `H2_OPT_COLD`, listed in [switches.md](../runtime/switches.md).

`cargo h2 emit /path/to/default.xbe --run-entry` builds only the host
architecture. Use `--arch all` for both arm64 and x86_64, or select one
with `--arch arm64` / `--arch x86_64`. Running the entry requires a host
build. `--jobs N` selects clang parallelism; the default uses every available
core. All outputs are derived from the user's disc and must stay untracked.
The default output is `$TMPDIR/h2gen` (`std::env::temp_dir()`); use `--out`
to choose another temporary or user cache directory. The CLI resolves symlinks
and refuses output or `H2_OBJ_CACHE` paths inside any git work tree, including
relative paths and defaults selected through `TMPDIR`. `--allow-repo-output`
is the explicit override; derived game output must still never be committed.

## What a warm run reuses

An emission receipt in `<out>/emit-cache-v1` covers the XBE bytes, package
version and the entire `h2` executable, hashed with SHA-256. A matching
receipt skips analysis, lifting, planning and C emission. Before accepting
it, the CLI hashes every generated C file, support header and manifest and
checks the complete file list. The SHA-256 dependency enables its runtime
hardware dispatch, including ARM SHA instructions, to keep validation cheap.
Editing, deleting or adding a source causes
a miss even when sizes and timestamps are unchanged. The receipt is
invalidated before regeneration and published atomically after success.

The CLI holds an exclusive `flock` on `<out>/.h2-emit.lock` from before
receipt validation through emission, ABI installation, object compilation,
pruning, linking and entry execution. Concurrent processes using the same
output (including symlink aliases) serialize that entire transaction. The
lock is released on return, failure or process death. The lock file stays
in place so later writers lock the same inode, and is opened with
`O_NOFOLLOW`. After acquiring the lock, the CLI inspects every existing
entry under the output and object-cache roots and rejects symlinks,
including dangling links and directory aliases. This covers source/header,
ABI, pin, receipt, library and cached-object destinations before any game
output is written. `--allow-repo-output` permits a repository location;
it does not permit descendant symlinks. Hard-linked cached objects remain
supported. User-supplied root aliases are resolved before taking the lock.

Rebuilding `h2` invalidates the receipt, including analyzer, IR, lifter or
emitter edits without a crate version bump. This deliberately does not
persist IR across generator builds, so a changed pass can never silently
use old results. `--no-emit-cache` forces regeneration, as does
setting `H2_TRACE` for analysis diagnostics. The installed `h2abi.h` is
refreshed separately on every build: changing the runtime ABI invalidates
objects without repeating analysis and lifting.

The object cache already works per emitted C file. Its keys cover source
contents, all support/ABI headers, target, flags and compiler/SDK identity.
Before object lookup, clang runs dependency preprocessing (`-M`, including
system headers) with the actual target, flags and include environment for
each C file. Every discovered dependency's contents enter that object's
key. Shared headers are read and hashed once per build. Dependency discovery
runs again on warm builds, so new include-path shadows and conditional
includes cannot reuse a stale dependency graph. This also covers nested
headers and unchanged `CPATH`/`C_INCLUDE_PATH` directories whose files change.
Output-local header names are relative in the key, preserving reuse across
output directories.

Clang's expanded driver command (`-###`) is also fingerprinted for each
translation unit with its actual flags and target. The key includes the
contents of explicit and default configuration files reported by clang;
expanded response-file options participate too. This detects a changed
`--config` file even when `CCC_OVERRIDE_OPTIONS` and include dependencies
are unchanged. Configuration comments also change the content key. Only the
source-file operand is normalized for reuse across output locations;
configuration options and macro values retain their full contents.
The compiler's working directory is pinned too (`-fdebug-compilation-dir=.`
and `-fcoverage-compilation-dir=.`). Clang otherwise puts its cwd in the `-###`
command, so a build started from another checkout or shell directory keyed
every object differently and reused none of a shared `H2_OBJ_CACHE`.
Debug info names sources relative to their directory
(`-fdebug-prefix-map=<abs src>=.`, so `./shard_NNNNN.c`). Before, an
object a shared cache handed to another output still named the absolute
source path of the output that first compiled it, and once that output
was deleted the profiler lost every instruction (`perf::symbolize` reads
the shard sources for its markers). The flag is left out of the key on
purpose, since every tree maps to the same `.`; a constant in the key
retires objects built under the old naming.
A change of key naming retires the objects built under the old one, so the
cache needs no manual clear.

### Sharing probes between sources

Running both probes for every translation unit would cost about 0.1 s each,
about 2000 clang processes on every build even when every object is cached.
Generated sources look alike to both probes, so one run serves many sources;
the keys are the same either way.

- **Driver (`-###`).** The driver sees a source only as its path. One dry
  run serves every source in a directory with the same extension: the
  source's path is replaced by a placeholder in the trace, and each source's
  own name goes into its key. If the name appears anywhere else in the
  trace (a `-D` value, an `-include`), that directory falls back to one
  probe per source.
- **Dependencies (`-M`).** Two sources in one directory include the same
  files when they have the same *preprocessor skeleton*: every line that
  holds or could hold a directive (`#`, `%:`, `??=`), with its line number,
  since `__LINE__` is visible to `#if`. Lines joined to such a line count
  too: one ending in a backslash (or `??/`), the line after it, and every
  line that starts or ends inside a block comment. One scan serves every
  source with that skeleton. The scanned source is dropped from the shared
  list, and each source hashes its own text.
- **Never shared.** A source whose text has `_Pragma`, `__COUNTER__`,
  `__FILE__`, `__FILE_NAME__` or `__BASE_FILE__` is scanned on its own.
  If any header of the build uses `__BASE_FILE__`, `push_macro`/`pop_macro`
  or `include_alias`, every source is scanned on its own.

Generated shards share one skeleton and one directory, so a full image
needs about two probes instead of about 2000. Tests in `compile.rs` check
that sharing still rebuilds exactly the right objects: a header only one
source includes, a conditional include, a moved directive, and a header
that turns sharing off.
A change confined to one shard recompiles that shard. Sources use stable
address buckets and oversized functions get their own file; the compiler
schedules large sources first. Unchanged files keep their objects even
after regenerating the whole source tree.

`H2_OBJ_CACHE` shares objects between output directories and worktrees;
otherwise they live in `<out>/obj`. `H2_OBJ_CACHE_MAX` accepts binary sizes
such as `2G`, `512M` or a byte count, and prunes least recently used objects.
Set both when sharing a persistent cache:

```sh
H2_OBJ_CACHE="$HOME/Library/Caches/h2recomp-cobj" H2_OBJ_CACHE_MAX=2G cargo h2 emit /path/to/default.xbe --out "$TMPDIR/h2gen" --run-entry
```

Each build hard-links (or copies) its current objects under `<out>/pin` so
another worktree pruning the shared cache cannot break linking. These pins
keep disk space alive until replaced by the next build, or `<out>` is
removed. The size cap spares the current build even if it exceeds the cap.
Delete completed output directories when disk space is tight.

## Mutable files and hard links

Changed generated C, support/ABI headers, manifests, receipts and bridge
sources are written to an exclusively created sibling temporary, then
renamed over the destination. This publishes a new inode rather than
truncating an existing one, so an output hard-linked into a repository
cannot change that repository file. Unchanged generated files keep their
existing bytes and timestamps.

Compiler objects, bridge objects, static archives and host dylibs are also
produced at fresh reserved temporary paths before publication. Failed writes
or tools leave the old destination intact and remove unfinished temporaries.
A successful compiler action must produce nonempty output before it can be
pinned or cached. Host dylibs retain the final output's install name.

Content-keyed cached objects remain immutable after publication, with the
existing hard links for pins and shared-cache reuse. Those links are not
rejected or rewritten in place. Copy fallback across filesystems also uses
a fresh temporary at its destination.

## Profiles and timing

The default remains `H2_OPT=1 H2_DEBUG=lines`: `-O1 -gline-tables-only`,
with strict floating-point behavior. `H2_OPT` also accepts `0`, `g`, `s`,
`2`, `3`; `H2_DEBUG` accepts `none`, `lines`, `full`. Both are cache-keyed.
`H2_OPT=0 H2_DEBUG=none` is available for compile-speed experiments, but
can make guest execution slower; it is not the default.

### Optimisation tiers (opt-in)

`H2_OPT_COLD=<level>` with `H2_HOT=<file>` compiles every source that
holds none of the file's guest addresses at `<level>` (`0`, `1`, `g`, `s`
or `2`) and the rest at `H2_OPT`. Off unless both are set; setting only
one is an error. A source is hot when it holds the instruction marker
(`/* 0x0001234 */`) of any listed address. The file can be a profiler's
`profile.folded` or `profile.txt` as written (`guest 0x…` and `@0x…`
frames), or any list of `0x…` addresses or `h2fn_…` names.

Each tier has its own flags in its objects' keys and its own dependency
and driver probes, so moving a source between tiers recompiles exactly
that source, and turning tiers off reuses every object built hot. The boot
stamp records the cold level and a hash of the hot list, so `h2 run` and
`h2 play` rebuild when either changes; with tiers off the stamp is the
same as before. `h2 build` prints how many sources were cold. It only
changes how fast cold code runs, never what it does; measure fps before
relying on it (notes/build-perf.md has the compile-time side).

### Compiler switches (opt-in): `-O3`, the CPU, PGO

All off by default; each adds flags to `compile::cflags`, so each is in
every object's key and in the boot stamp. Set the same values on `h2 run`
and `h2 play` as on `h2 build`, or they rebuild (`crates/recompiler/emit/src/compile/pgo.rs`).

- `H2_OPT=3`: `-O3`.
- `H2_CPU=<name>`: `-mcpu=<name>` (`apple-m1`, `apple-m3`, ...) for arm64
  targets (macOS, tvOS, iOS); an x86_64 target (Intel in `--arch all`,
  Windows) compiles without it. Clang rejects an unknown name.
- `H2_PGO=gen`: `-fprofile-instr-generate`. The module link passes the
  same flag, so the clang driver links its own profile runtime
  (`libclang_rt.profile_osx.a` from `clang -print-resource-dir`) into
  `libh2game.dylib`, with a small shim (`pgo::SHIM`). The raw profile goes
  to `H2_PGO_OUT` (read when the module loads; default
  `$TMPDIR/h2.profraw`; llvm's `%p`/`%m` patterns work): written at the end
  of every `boot::run` (`H2_BOOT_STOP_AT_PRESENT`, the slice limit, the
  window closing), printing `pgo: raw profile written to …`, and again by
  the runtime at exit. Not for Windows yet.
- `H2_PGO=use:<file.profdata>`: `-fprofile-instr-use=<file>` (made
  absolute), `-Wno-profile-instr-out-of-date`,
  `-Wno-profile-instr-unprofiled`, and `-DH2_PGO_PROFILE=<content hash>`,
  which no source reads: it keys the objects and the stamp on the
  profile's contents, so a new profile at the same path recompiles. The
  file must stay in place for `h2 run`/`play` to match the stamp.

Merge with the `llvm-profdata` of the clang that compiled
(`xcrun llvm-profdata` beside `/usr/bin/clang`: the versions printed by
`clang --version` and `xcrun llvm-profdata --version` should agree; a raw
profile newer than the tool is rejected). Static functions are named by
source file name in the profile, not path, so the training and the use
build may use different `--out` directories. The loop on the real image
(the training level is a choice; any headless run works):

```
OUT=$TMPDIR/h2pgo; T=$TMPDIR/h2pgo-run
cargo build --release -p cli
# 1. Instrumented build, then the training run with the same switches.
H2_OPT=2 H2_PGO=gen target/release/h2 build disc/halo2.iso --out $OUT/gen
H2_OPT=2 H2_PGO=gen H2_PGO_OUT=$T/h2-%p.profraw \
H2_LEVEL=03a_oldmombasa H2_BOOT_STOP_AT_PRESENT=4000 \
target/release/h2 run disc/halo2.iso --out $OUT/gen --data $T/data --fresh-data
# 2. Merge (one or more runs).
xcrun llvm-profdata merge -o $T/h2.profdata $T/h2-*.profraw
# 3. The optimised build, and the same switches to run or play it.
H2_OPT=2 H2_PGO=use:$T/h2.profdata target/release/h2 build disc/halo2.iso --out $OUT/use
H2_OPT=2 H2_PGO=use:$T/h2.profdata target/release/h2 play disc/halo2.iso --out $OUT/use
```

`crates/recompiler/emit/tests/pgo.rs` runs the same loop on a synthetic handler:
an instrumented build writes a profile, `llvm-profdata merge` and `show`
read it, a `use` build compiles, links and computes the same result, and
`H2_OPT=3` and `H2_CPU` build; each keys its objects apart.

The CLI prints emission receipt validation, analysis, lift/plan, C emission,
object dependency/driver discovery, hash/pin work, clang wall time, archive validation/creation,
host bridge compilation plus linking, and total wall time. A warm run still
validates source and toolchain contents; `--run-entry` also rebuilds the
small host bridge, links, loads and executes the image entry.

For cold measurements use a new `--out` with `H2_OBJ_CACHE` unset. Repeat
the identical command for warm measurements. For an incremental measurement,
change one function in a private XBE copy and use the same output; compare
`src/manifest.tsv` to verify how many shards actually changed. Include
`--run-entry` in every case. Measure per-file clang and link times with a
`clang` wrapper on `PATH` that forwards to the real compiler, records wall
time and exit status, and handles version/resource probes unchanged. Use
the same wrapper for before/after runs: changing its contents changes the
object cache identity. Measured results are in [notes/build-perf.md](notes/build-perf.md).

## The `h2 build` and `h2 play` path

`h2 build`, and `h2 run`/`play` when they rebuild, don't use the emission
receipt above. They write `<out>/src`, compile it with the same object
cache, link `libh2game.dylib` and stamp it (docs/runtime/BOOT.md).
`H2_EMIT_TARGET` (`host` or unset, `macos`, `tvos`, `ios`, `windows`) picks
what they compile for; the target is in the objects' flags and the stamp
([windows.md](../runtime/windows.md), [tvos.md](../runtime/tvos.md),
[ios.md](../runtime/ios.md)). What each kind of change costs:

- **Nothing changed.** `play` and `build --check` read the image and
  compare the stamp, and do nothing else.
- **Runtime only.** The stamp covers the runtime only through
  `runtime/include` (`h2abi.h`), so a runtime change keeps the dylib.
- **Generator change** (xbe, analyze, ir, lift, opt, emit, x87, or boot's
  bridge in `crates/apps/boot/c`): the whole pipeline runs again, but most of
  it is cheap.
  - *Analysis* is cached in the boot cache (`H2_BOOT_CACHE`, default
    `$TMPDIR/h2boot-cache`; `off` disables it), keyed by the image's hash
    and size and by `ANALYSIS_SOURCE_HASH`: the xbe and analyze sources,
    `Cargo.lock` and the rustc version. A change outside xbe and analyze
    reuses it. `H2_TRACE` bypasses it, so diagnostics always come from a
    fresh analysis. A damaged, truncated or other-version file is
    recomputed (`Analysis::decode` refuses it).
  - *Lifting, IR passes, C generation and file writes* run on every core
    (`emit::par`). Results are kept in owner order, so the C is
    byte-identical to a serial run.
  - *Keying* runs about two clang probes, not two per source (above).
  - Only shards whose C changed are rewritten and compiled.
- `build` prints every phase: analyze (and whether it was cached), lift,
  plan, emit, keying, compiling, archive, link and the total.

`h2 play --follow` builds each new commit's game into `gen/<short>` of its
cache, with the object cache at `<cache>/objcache`. A new `gen/<short>`
starts as an APFS clone (`cp -cR`) of the last game's `src/`, so the emit
rewrites only the files that commit changed, and the unchanged ones share
their blocks with the last game. If the clone fails, the emit writes every
file as before.

## Rust dependency builds

Rust compilation is separate from these generated-code caches. A machine
may set a caching rustc wrapper (kache, sccache) as Cargo's
`build.rustc-wrapper`. Cargo reads configuration from ancestor directories,
so a config below the repository root may apply only to nested worktrees.
Avoid committing machine-specific cache paths or requiring a wrapper for
other developers.

Leave a configured wrapper on. Commands in these docs don't clear
`RUSTC_WRAPPER`: a wrapper that hardlinks its outputs (kache) is what keeps
many worktree targets cheap, and clearing it makes every target a full
private copy (13 GB and more each). If a wrapper serves a stale artifact
(errors that contradict the source), `touch` the edited file rather than
turning the wrapper off. Use one wrapper per machine: running a second one
(sccache next to kache) keeps a second full store.

Give each worktree its own `CARGO_TARGET_DIR`; generated C object sharing
does not require sharing Cargo target directories.
