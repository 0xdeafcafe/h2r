# Iteration builds and caches

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
persist IR across generator builds, so pass changes in other lanes cannot
silently use old results. `--no-emit-cache` forces regeneration, as does
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
These keys supersede earlier header-only keys, so existing objects rebuild
once without requiring a manual cache clear.
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

## Profiles and timing

The default remains `H2_OPT=1 H2_DEBUG=lines`: `-O1 -gline-tables-only`,
with strict floating-point behavior. `H2_OPT` also accepts `0`, `g`, `s`,
`2`; `H2_DEBUG` accepts `none`, `lines`, `full`. Both are cache-keyed.
`H2_OPT=0 H2_DEBUG=none` is available for compile-speed experiments, but
can make guest execution slower; it is not the default.

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
object cache identity. Results from this lane are in [build-perf.md](build-perf.md).

## Rust dependency builds

Rust compilation is separate from these generated-code caches. A machine
may use `sccache` as Cargo's `build.rustc-wrapper` to share registry artifacts
across worktrees. Cargo reads configuration from ancestor directories, so
a config below the repository root may apply only to nested worktrees.
Avoid committing machine-specific cache paths or requiring sccache for
other developers. `RUSTC_WRAPPER=` disables a configured wrapper for one
command. Use a per-lane `CARGO_TARGET_DIR` under `$TMPDIR`; generated C object
sharing does not require sharing Cargo target directories.
