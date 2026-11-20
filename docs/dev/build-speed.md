# Rust build and test speed

How long `cargo check` and `cargo test` take on this workspace, what was
changed to shorten them, how to measure it, and what is left. The game
image's clang build is a separate pipeline with its own notes:
[BUILD-CACHE.md](../lifter/BUILD-CACHE.md),
[build-perf.md](../lifter/build-perf.md) and the unmerged `build-speed-2`
branch (optimisation tiers for the generated C).

Status: the changes below are committed but **not measured**. They were
written without running a build. The numbers are read from an existing
target directory and from the compiler cache's own report; every
expected gain is reasoning from those, to be confirmed with the plan
further down. The two scripts were run as far as they go without
building (see the end of "The gate").

## The shape of the problem

- 86 crates, about 387 files in `crates/*/tests/*.rs`. Cargo links each
  file into its own executable.
- `boot` is 155,000 lines in 157 top-level modules and depends on 59
  workspace crates. `cli` is a 77,000 line binary crate on top of it. An
  edit almost anywhere recompiles both.
- A machine-wide rustc cache (kache) sits in front of rustc and turns
  cargo's incremental compilation off, so a changed crate is compiled
  whole.
- Many checkouts are built one after another into one target directory.

## What the target directory shows

Read from a 70 GB shared target directory on 2026-11-20 (52 GB in
`debug/deps`, 16 GB in `incremental.kache-auto`):

| Observation | Number |
|---|---:|
| Test executables kept in `deps` | 757 files, 20.6 GiB |
| `.dSYM` bundles in `deps` | 347, 10 GiB; the newest minutes old |
| `d3d` test executables, one build | 50, 1.5 GiB (30 MiB each) |
| `live-server` test executables, one build | 40, 2.5 GiB (63 MiB each) |
| `boot` test executables, one build | 16, 0.6 GiB (largest 215 MiB) |
| Feature sets `boot` was compiled with in 36 hours | 5 |
| Profile settings `boot` was compiled with | 4 |
| Third-party crates compiled with more than one feature set | 79 |

Three things follow.

**Links and executables dominate the test loop.** Each test executable
statically links its whole dependency tree. A one-line change to `d3d`
relinks 50 executables of 30 MiB.

**The same crate is compiled several times with the same code.** `boot`
appears as `["default", "gpu", "local-io", "movie-playback"]` (from
`cargo test -p boot`) and as `["gpu", "local-io", "movie-playback"]` (from
`cargo test -p cli`, which turns the three on one by one). `input`,
`agent`, `bink-hle`, `feature` and `dsound` show the same pair. Cargo
resolves features over the packages named on the command line, so every
separate `cargo test -p X` can build its own copy of everything `X`
shares with the others. `syn` has 7 copies and `tokio` 4.

**Something still produces `.dSYM` bundles.** Cargo's macOS default is
unpacked debug info, which never runs `dsymutil`, yet the `h2` binary got
a 602 MB bundle on its last link. See "Check first".

The directory holds one path hash for `boot`: cargo does not put the
checkout's path in a workspace crate's artifact name. Two checkouts
building into one target directory therefore overwrite each other's
files, and cargo decides freshness by comparing file times, which is how
a second checkout can be served the first one's build.

## What changed

### Dev profile (`Cargo.toml`)

- `[profile.dev.package."*"] debug = false`. Dependencies had line
  tables; now they carry no debug info. About 670 crates' rlibs shrink
  and every link reads less. A panic inside a dependency still prints its
  file and line (that comes from the panic location, not debug info); a
  backtrace still names the dependency's functions but not their lines.
  Workspace crates keep `line-tables-only`.
- `split-debuginfo = "unpacked"` is now written out. It is cargo's
  default on macOS, so by itself it changes nothing; it is there so an
  override cannot bring `packed` and `dsymutil` back unnoticed.
- `opt-level = 3` for `miniz_oxide`, `zlib-rs`, `flate2`, `fdeflate`,
  `png`, `zune-jpeg`, `ruzstd`, `crc32fast`, `adler2`, `simd-adler32`
  and `sha2`: small codec and checksum crates that tests push whole
  images and maps through. They compile in seconds either way and are
  the classic 10 to 50 times slowdown at `opt-level = 0`.

All three change every crate's fingerprint once, so the first build
after this lands is a full rebuild.

### Build scripts

`crates/formats/movies/build.rs` and `crates/platform/bink-hle/build.rs` asked cargo to
rerun them whenever `PATH` changed, to learn whether `ffmpeg` is
installed (only the tests use the answer). Cargo rebuilds a crate whose
build script reran, and then everything above it: `movies` and
`bink-hle` are under `boot` and `cli`. Two shells with different `PATH`s
sharing a target directory therefore rebuilt `boot` and `cli` on each
switch. The scripts now find `ffmpeg` on `PATH` themselves and follow
that file. After installing ffmpeg, touch the build script.

The other build scripts were read and left alone:

- `crates/apps/boot/build.rs` names every file it hashes. It also hashes
  `Cargo.lock`, so any dependency change in any crate recompiles `boot`.
- `crates/apps/app/build.rs` bakes the commit and build time and reruns when
  `HEAD` moves. That is correct, and costs a rebuild of `app` and `cli`
  on every commit or checkout (see follow-up 5).
- `crates/formats/tags/build.rs` follows `schema.rs` and itself.

### One test binary per crate (`tests/it.rs`)

For `h1objects` (27 files), `h1convert` (16), `engine` (16) and `d3d`
(37 of 50), the integration test files now build into one executable
named `it`. 96 executables become 4.

- No test file moved. `tests/it.rs` names each file as a module with
  `#[path = "name.rs"]`. A file loaded that way resolves its own
  `mod common;` beside itself, exactly as it did as a crate root, so the
  files needed no edits (one exception: `d3d/tests/draw_frame/mod.rs`
  said `crate::common` and now says `super::common`).
- The crate sets `autotests = false`, so cargo no longer builds every
  file under `tests/` by itself. To keep a new file from silently never
  running, `it.rs` ends with a shared guard test
  (`scripts/it_guard.rs`): it fails, with the line to add, when a file
  under `tests/` is neither a module of `it.rs` nor a `[[test]]` in
  `Cargo.toml`.
- A test's name gains its file's name: `cargo test -p d3d --test interp`
  is now `cargo test -p d3d --test it interp::`. The filter is a
  substring, so a name that ends another name needs a skip:
  `--test it -- disc:: --skip _disc::`. The commands in the docs and
  file headers were rewritten.
- Feature-gated files keep their own `#![cfg(feature = "...")]` line,
  which works on a module.

Thirteen `d3d` files keep a process and a `[[test]]` of their own: the
benches and corpus runs (`draw_bench`, `draw_churn`, `render_churn`,
`pipeline_cost`, `shader_corpus`), and the files that time threads,
count process-wide work or watch the filesystem (`threaded`,
`threaded_frame_stats`, `vblank_clock`, `visibility_async`,
`alias_mirror`, `gpu_depth_alias`, `hot_overrides`, `pipeline_cache`).

**What can differ.** Under `cargo test`, the merged files now share one
process, where each had its own. The merged crates were chosen and
checked for that: `h1objects` and `h1convert` are pure converters with
no process-wide state and every test `#[ignore]`d behind a disc; the
`engine` and `d3d` test files hold no locks or statics shared between
files, and their temporary paths are distinct. What a read of the code
cannot rule out is a test that passes only when it has the machine to
itself. If one turns up, give that file a `[[test]]`; nothing else
changes. Under nextest there is no difference: every test already gets
its own process.

**Why the other large crates were not merged.** `feature`, `boot`,
`kernel`, `runtime`, `emit` and `xapi` have process-wide state that
tests in different files set: `feature`'s installed hosts and draw
counters, `boot::fallback::set_recovery`, `kernel/tests/files.rs`'s
global allocator, `emit/tests/pgo.rs` and `boot/tests/headless_interp.rs`
setting environment variables, the runtime's signal handler and its 16
protection regions per process. Merging those needs a test run to
settle which files can share a process (follow-up 2). `live-server`
(40 files, the largest saving) belongs to another session.

### nextest (`.config/nextest.toml`)

`cargo nextest run` runs each test in its own process and spreads the
tests of all binaries over the cores at once; `cargo test` runs one
binary at a time, so a binary with two tests uses two cores. The config
reports every failure instead of stopping at the first, keeps tests
that read discs to four at a time, and runs timed tests one at a time.
Doctests still need `cargo test --doc`.

nextest is not a pure win here. A file whose tests share an expensive
fixture through a `OnceLock` (the C library `emit/tests/native_x87.rs`
compiles, the oracles in `engine`'s `sim_*_differential`) builds it once
per test instead of once per file. Measure per crate before switching
the sweep.

## Many worktrees, many agents

The sections above shorten one build. These five are about the flow this
repo is actually built in: many agents, each in a worktree of its own,
and one coordinator that builds and tests their branches. What the cache
reports (kache, 24 hours to 2026-11-20):

| Observation | Number |
|---|---:|
| Compiles asked for | 85,809 |
| Served from the cache | 40.9% |
| Time spent compiling on misses | about 57 hours |
| Compiles passed straight to rustc as "edit rebuilds", not cached | 6,888 |
| Entries in the cache | 271 (4 GiB of a 15 GiB limit) |
| Cost of one hit | 1.2 s on average |
| One `boot` compile | 51 s for the rlib (451 MiB), 9 s for a check |

### 1. Cache hits across worktrees

**The checkout's path is not what misses.** Cargo's artifact names do
not carry it: the shared target directory has one path hash for `boot`
across every checkout built into it. The cache does serve one worktree
from another (35,060 hits that day; an earlier measurement had a second
worktree's test build of `hs`, `tags` and `map` at 3 s against 15 s).
So `--remap-path-prefix`, `CARGO_ENCODED_RUSTFLAGS` per worktree and a
stable path prefix are not needed, and the first two would hurt: any
change to the rustc flags changes every crate's fingerprint, and
remapping breaks the paths in panics and line tables.

What does miss:

- **The cache is nearly empty.** 271 entries. The disk is 96% full, the
  cache backs off to keep 10 GiB free, and it is cleared by hand when
  space runs out. Most lookups miss because the entry is gone. Disk
  headroom is the largest single lever on the hit rate, which is why
  the test-binary and target-directory changes here matter beyond their
  own time.
- **"Edit rebuilds" are not cached.** When a crate changes twice in one
  build tree, the cache decides it is being edited, lets rustc build it
  incrementally and stores nothing. That is right for a person editing
  one crate. In a target directory that many branches are built into in
  turn, every switch looks like an edit: 6,888 uncached compiles in a
  day, plus a 16 GB incremental directory.
- **Feature sets and profile settings.** Each distinct set is a
  distinct key, correctly. See "What the target directory shows".
- **`app` and `cli`.** `crates/apps/app/build.rs` bakes the commit hash and
  the wall-clock build time, so no two builds of `app`, or of `cli`
  above it, are ever the same input (follow-up 5).
- **`PATH`**, fixed above.

A hit is not free either: 1.2 s each, so a new target directory filled
entirely from the cache still costs about 750 crates times that, spread
over the build jobs. A crate that cargo itself finds fresh costs
nothing. That is the argument for target directories that persist
(next section) over fresh ones filled from the cache.

To confirm the path claim directly, build one small crate in two
checkouts of the same commit with two empty target directories, and
read `kache explain <crate>`: the second must be a hit. The commands
are in the plan below.

**Through the `target` links.** On the build machine a post-checkout
hook (`.git/hooks/post-checkout`, calling `target-on-builds.sh`) makes
each new worktree's `target` a link to
`/Volumes/Builds/targets/h2recomp/<worktree name>`. That changes
nothing for the key: cargo still hands rustc `<worktree>/target/...`,
which differed per worktree before the links too, and the cache
already had to see past it. A fixed prefix is still not needed, and
could not be had from links anyway. On the source side cargo works
from the real path of the current directory, so a linked source path
does not survive. On the target side the only way to give every tree
the same target path is to give them the same target, which is the
shared directory the next section argues against.

The links do change one thing. The cache restores by cloning, and an
APFS clone cannot cross volumes. The cache's store is still under
`~/Library/Caches`, a different volume from `/Volumes/Builds`, so until
it moves, a restore into a linked target has to be a real copy: slower
than the 1.2 s above and a second copy on disk. The cache reports this
as "restored without copying, N copied" in `kache stats`; after the
first build into a linked target that N should still be 0. If it is
not, moving the store to the builds volume is worth more than anything
else in this section.

The cache's own settings were not changed. Its binary names switches for
the adaptive incremental mode and for caching linked executables; what
they do should be read from its documentation and measured before any
is set.

### 2. Target directories

The layout on the build machine, as of 2026-11-20:

- Every worktree's `target` is a link to
  `/Volumes/Builds/targets/h2recomp/<worktree name>`, made by the
  post-checkout hook when the worktree is created. The same hook then
  removes the targets of worktrees that no longer exist, when nothing
  has files open in them.
- The coordinator's sweeps do not use those: they export
  `CARGO_TARGET_DIR` to one shared directory (`coord-target`, 70 GB,
  still on the system volume), which every branch is built into in
  turn.
- `/Volumes/Builds` is a volume in the same APFS container as the
  system volume. It keeps Spotlight and file-event logging away from
  build output, but it shares the container's free space: about 28 GB
  for both. Moving a directory there frees nothing.

Three ways to give many worktrees somewhere to build:

| | Each worktree's own target (the hook's default) | One shared target, a lock (`coord-target`) | A pool of leased slots |
|---|---|---|---|
| A stale artifact from another tree | cannot happen | **happens**: same file names, freshness by file time | cannot happen |
| First build of a new branch | every crate restored from the cache (1.2 s each, more if copied) or compiled, every test binary linked | whatever has a newer file time, plus what the last tree left wrong | only what differs from the slot's last commit |
| Builds at once | as many as there are worktrees | one | one per slot |
| Disk | one target per worktree that ever built; the hook removes it with the worktree | one directory that never shrinks | bounded: slots times one target |
| Edit rebuilds uncached | no | yes, constantly | yes, but the incremental state is reused, because successive commits in a slot are near each other |

The shared target is the worst of the three: it is the only one that
can hand a test another tree's code, which is where the bogus
missing-symbol errors and one false benchmark result came from. The
"never build two trees at once" rule does not fix it; building them one
after the other is enough to go wrong.

Each worktree's own target is safe, and now the default for anything
that does not set `CARGO_TARGET_DIR`. It suits agents that build their
own branch. It does not suit a coordinator that builds 20 branches a
day: each is a cold start, and each leaves a full set of test
executables until its worktree is removed.

**Recommendation: a pool of two slots for the coordinator, on top of
the hook.** A slot is a lock and a worktree that exists only to be
built (`gate-N`, moved with `git checkout --detach`). Its target is its
own `target`: the pool worktree is created with `git worktree add`, so
the hook fires and links it to `/Volumes/Builds/targets/h2recomp/gate-N`
like any other. Git rewrites only the files that differ between two
commits, so cargo's file times are right and it rebuilds exactly what
changed. Two slots match the existing rule of two compiles at once; the
lease is that rule. `scripts/gate.py --pool DIR` implements it, and
`--exec` runs any other command in a leased slot. The gate does not
pass `CARGO_TARGET_DIR` on, since that would route a slot back into a
shared directory.

Two things about the hook to work with:

- It names a target by the worktree directory's last component. Two
  worktrees with the same name in different places would share a
  target. The pool's `gate-1` and `gate-2` must not be reused as
  worktree names.
- It prunes only when a worktree is created, and only targets whose
  worktree is gone. A pool slot's worktree never goes, so its target
  is never pruned by the hook; see the policy below.

Disk, with 28 GB free in the container: one target for the whole test
sweep is roughly 12 to 20 GB (the 52 GB of `deps` in `coord-target`
holds at least four profile variants of everything). Two slots do not
fit beside `coord-target`. They replace it: once nothing uses it,
deleting it returns 70 GB, and the pool costs less than half of that.
Lane worktrees whose agents do not compile keep empty targets and cost
nothing.

**Prune policy.** Cleaning disk is a job for the machine's cleanup
tooling and the hook, not for this repository, so the gate only refuses
to start a step under a free-space floor (6 GiB by default) and prints
the target's free space. What the cleanup should do:

1. Lane targets: leave them to the hook. A lane target over a gigabyte
   means a lane compiled, which the rules say it should not.
2. Pool slots, one at a time: take the slot's lock (`gate-N.lock` in
   the pool directory, `flock`, non-blocking). If it is held, leave the
   slot alone. Holding it makes the rest safe: nothing can be linking
   against a file while it is deleted.
3. In the slot's target, delete test executables (files in
   `debug/deps` whose name ends in a 16-digit hash with no extension)
   and `.dSYM` bundles older than a day. Cargo relinks any it needs.
4. Delete `debug/incremental*` entries older than three days.
5. If the target is still over 25 GB, delete its `debug/` whole. The
   next build is cold for cargo but mostly cache hits.
6. Delete gate ledger files (section 4) older than 14 days.

Never prune by file age without the lock: an rlib nothing has rebuilt
keeps its old time while a running build still links against it. The
hook's own check, that nothing has a file open, covers a removed
worktree; it does not cover a slot between two cargo commands of one
gate run. The lock does.

### 3. Test only what changed

`scripts/affected.py` prints the crates a branch can have changed:

    scripts/affected.py --base master --why
    scripts/affected.py --format args       # -p a -p b ..., for cargo

It takes the files that differ from the merge base (and uncommitted
ones), maps each to its crate, and adds every crate that builds against
one of those. The dependency graph comes from
`cargo metadata --offline --no-deps`, which resolves nothing and
compiles nothing. The rules:

- A change under a crate's `tests/`, `benches/` or `examples/` affects
  that crate only.
- A crate that only dev-depends on a changed crate is added, but its
  own dependents are not.
- A file outside the crates counts for the crates whose sources name
  it: `include_str!("../../../docs/graphics/d3d.md")` in a `d3d` test,
  `#[path]` into another crate's `tests/common`, the runtime's ABI
  header. A change to `docs/graphics/d3d.md` tests `d3d`.
- `Cargo.lock`, the root `Cargo.toml` and `.cargo/config.toml` affect
  everything.
- A changed file that belongs to no crate and is named by none is
  listed on stderr. It is not tested, and the script says so.

Examples on today's tree: a change in `crates/platform/d3d/src` gives 15 crates;
in `crates/platform/d3d/tests`, 1; in `crates/formats/tags/src`, 45; in
`crates/apps/cli/src`, 1.

What it cannot see: a test that opens a file outside its crate at run
time through a path the scan does not recognise. The scan covers
`include!` forms, `#[path]`, and string paths joined to
`CARGO_MANIFEST_DIR` on one line.

### 4. The test ledger

There is a ledger already, `tested`, outside the repository. It keys a
pass on the whole checkout's tree, the cargo arguments and some
environment. Three problems: any commit anywhere, a doc fix included,
is a new key for every crate; it calls a lock script at a temporary
path that no longer exists, so it cannot run; and none of the sweep
scripts call it. They run cargo directly.

`scripts/gate.py` keeps its own, keyed per step on what the step can
depend on (`scripts/affected.py --keys` prints the crate part):

- the crate's own git tree;
- the trees of every crate its tests build against, without their
  `tests/`, `benches/` and `examples/`;
- the outside files those crates name;
- `Cargo.lock`, the root `Cargo.toml`, the cargo config;
- the cargo arguments, `rustc -vV`, the machine, and the environment
  that changes builds or tests (`RUSTFLAGS`, `CARGO_PROFILE_*`,
  `H2_*`, `H1_*`);
- the retail XBE's contents, when the step is given one.

Two checkouts with the same key build and test the same thing, so a
pass in one is a pass in the other: a lane rebased onto a master that
moved only in crates it does not depend on keeps its passes. Only
passes are recorded. A crate with uncommitted changes anywhere in its
inputs has no key and always runs. Two gates that reach the same key
at once run it once; the second waits and reads the result.

The key is deliberately coarse in one place: any `Cargo.lock` change
invalidates every crate. Narrowing that to each crate's own resolved
dependencies needs a full `cargo metadata`, and is a refinement.

Because a pass must mean the same thing wherever it is reused, the gate
tests each crate with its own command, `cargo test -p X`. One command
for many crates (`cargo test -p a -p b ...`) compiles each shared crate
once, because features are then resolved once, but its result depends
on which other crates were named, so it cannot be recorded per crate.
Use the single command for a full cold sweep with no ledger; use the
gate for branches.

### 5. The gate

`scripts/gate.py` is the one command a branch passes before it merges:

    scripts/gate.py --pool /Volumes/Builds/h2recomp-gate
    scripts/gate.py --dry-run          # the plan, no cargo

1. `cargo check --locked -p cli`, then again with
   `--no-default-features --features host`.
2. `cargo test --locked -p X` for each affected crate, dependencies
   first, with `H2_XBE` set when the disc is there.
3. The disc differentials when the branch touches what they cover:
   `engine` and `native-render` with `--features differential` (for a
   change in `engine`, `emit` or `runtime`), and `runtime` and `emit`
   on flat RAM. The table is `DISC_STEPS` at the top of the script.

Every step ends as `PASS`, `LEDGER`, `NOT NEEDED`, `NOT RUN` or `FAIL`,
and the summary lists them. The exit status is 0 only when nothing is
`FAIL`. There is no way for a step to be skipped quietly:

- A step that cannot run is `FAIL`: no disc for a disc step, free
  space under the floor, cargo missing.
- The time limit is the script's own (`--timeout-min`, per step). It
  does not use `timeout`, which macOS lacks; a step over the limit is
  killed with its children and is `FAIL`.
- A step's exit status is read directly, never through a pipe.
- `NOT RUN` only comes from a flag on the command line (`--exclude`,
  `--without-disc REASON`), and the final line then reads `PASSED,
  WITH N STEPS LEFT OUT BY FLAGS`.
- For test steps the summary shows how many tests passed and how many
  were ignored, so a step that ran nothing is visible.

Each step's output goes to a log file; a failure prints its failing
lines and the log's path. `--fail-fast` stops at the first failure and
marks the rest `FAIL, not reached`.

State of testing: `affected.py` was run against this repository. The
gate's plan, ledger, failure, timeout, lease and exit-status paths were
exercised with a stand-in `cargo` that builds nothing, including a
lease on the builds volume, where the hook linked the slot's target as
described. It has not yet run a real build.

## Measurement plan

Run on a quiet machine, in one checkout moved between the two commits
with `git checkout --detach`, into a scratch target directory so old
artifacts do not blur the sizes. "Before" is
`origin/master` at `100f13cef`; "after" is `build-1120`. Each timing is
taken twice: cold (the scratch target deleted; kache as it is) and warm
(the same command again after touching one file). Record `du -sh` of
the target after each step. The exact commands are in
`build-speed-commands.md` in the coordinator's keep directory, and
repeated here in short.

### Check first

1. **Is `dsymutil` still running?** After any test build on
   `build-1120`: `find $T/debug/deps -name '*.dSYM' -newer $STAMP | wc -l`.
   If it is not zero, something overrides the profile (an environment
   variable, or the rustc wrapper rewriting the flag). Find it before
   measuring anything else: on a 400 MB binary `dsymutil` costs more
   than the link.
2. **Does `PATH` still rebuild `boot`?**
   `cargo check -p cli` then `PATH="$PATH:/nonexistent" cargo check -p cli`.
   Before: `movies`, `bink-hle`, `boot` and `cli` compile again. After:
   nothing does.

### Then

| # | What | Command (before and after) | Compare |
|---|---|---|---|
| 3 | Test builds of the merged crates, cold | `cargo test --locked --no-run -p d3d -p engine -p h1objects -p h1convert` | wall time, executables in `deps`, `du -sh` |
| 4 | Relink after an edit | `touch crates/platform/d3d/src/lib.rs`, then `cargo test --no-run -p d3d` | wall time: 50 links against 14 |
| 5 | Same tests, same results | `cargo test -p d3d -p engine` | the `test result` lines sum to the same passed and ignored counts, plus one guard test per crate |
| 6 | Flakes from sharing a process | `cargo test -p d3d --test it` ten times | every run passes |
| 7 | Dependencies without debug info | `cargo build -p cli`, cold, then `touch crates/apps/cli/src/main.rs` and again | size of `deps/*.rlib`, link time of `h2` |
| 8 | The sweep as one command | a loop of `cargo test --no-run -p X` against one `cargo test --no-run -p X -p Y ...` | wall time, and copies per crate: `ls deps \| sed -E 's/-[0-9a-f]{16}.*//' \| sort \| uniq -c \| sort -rn` |
| 9 | nextest against `cargo test` | `cargo nextest run -p ...` against `cargo test -p ...`, both after `--no-run` | wall time of the run alone |
| 10 | Where a build's time goes | `cargo build -p cli --timings` | the `boot` and `cli` bars and the link, in `target/cargo-timings` |
| 11 | The cache ignores the checkout's path | `cargo test --no-run -p xbe` in two checkouts of one commit, each with an empty target directory | `kache explain xbe` after the second: a hit |
| 12 | A pool slot against a fresh target | gate branch A, then branch B, in one slot; then branch B in its own worktree with an empty target | wall time of B's gate both ways, `du -sh` of both targets, and "copied" in `kache stats` |
| 13 | The ledger | the gate twice on one commit; then on a branch rebased over a master that moved in a crate it does not depend on | second run: every step `LEDGER`; after the rebase: only the steps for crates that changed |
| 14 | The gate fails loudly | the gate with `--disc /nonexistent` on a branch touching `engine`; with `--timeout-min 0.1`; with `--min-free-gib 10000` | exit status 1 each time, the step `FAIL` with its reason |

## Expected gains

These are estimates from the counts above.

- **Merged test binaries.** A change to `d3d` or anything under it links
  14 executables instead of 50; a test build of `d3d` keeps about
  0.5 GiB of executables instead of 1.5 GiB per build. `h1objects` and
  `h1convert` go from 43 links to 2 on every disc run. Link time is
  roughly proportional to executable size, so expect the link part of a
  `d3d` test build to fall by about two thirds.
- **`PATH`.** When two shells differ, this removed a full `boot` and
  `cli` recompile per switch, the most expensive thing in the
  workspace. When they do not differ, nothing.
- **Dependencies without debug info.** Smaller `deps` (line tables are
  typically a tenth to a fifth of an unoptimised rlib) and shorter
  links. Modest.
- **One command for a full sweep.** Removes the duplicate compiles of
  `boot`, `feature`, `input`, `agent`, `dsound` and `bink-hle`, and of
  the 79 third-party crates with several feature sets. Likely the
  largest saving in a cold sweep of everything, and it costs nothing to
  try.
- **Pool slots.** Turn "a new worktree" from a pass over every crate
  (1.2 s each even when the cache has it) into a rebuild of what the
  branch changed, and end the stale-artifact failures. Disk goes from
  an unbounded shared directory to two bounded ones.
- **Affected crates.** A branch that changes `d3d` tests the 15 crates
  that can see the change, instead of a fixed list of 16 that may not
  include them; one that changes only `d3d`'s tests, 1. A change to
  `tags` still tests 45: the graph is wide at the bottom.
- **The ledger.** A branch re-gated after a rebase, or two lanes merged
  into an integration branch, rerun only the steps whose inputs
  changed. With the old whole-tree key that was never.

## Follow-ups, ranked

1. **Merge `live-server`'s 40 test files** (2.5 GiB and 40 links of
   63 MiB per build, more than any other crate). Its owner can use the
   same `tests/it.rs` and `scripts/it_guard.rs`. Files that bind fixed
   ports or set process-wide state keep a `[[test]]`.
2. **Merge `boot`, `kernel`, `runtime`, `emit`, `xapi` and `feature`**
   (105 files). Needs test runs: merge everything, run the `it` binary
   repeatedly, and move each file that fails in company to a
   `[[test]]`. `boot` first: its executables are the largest.
3. **Split `boot`.** With incremental compilation off, any edit under
   `boot` compiles 155,000 lines, then `cli`. Its modules fall into
   groups that could be crates of their own, cached and compiled in
   parallel: the Halo 1 glue (`h1_ai_control`, `h1_conversation`,
   `h1_hud_text`, `h1_command_atoms`, about 16,000 lines), theater
   (`theater`, `theater_ui`, 11,000), the native renderer glue
   (`native_*_glue`, `d3d_glue`, `layout_glue`, 12,000), and the menu
   screens (`list_class`, `mods_screen`, `map_select`, `skulls*`,
   `campaigns`, `cheats`, 18,000). Start by mapping which modules use
   `crate::` items of which others; the split follows the leaves.
4. **Make `cli` a library with a thin `main.rs`,** and move the Live
   art pipeline (`live_models`, `live_geometry`, `live_art` and their
   tests, 15,000 lines) into its own crate. Today every test build
   compiles the 77,000 line binary crate twice.
5. **Take the commit hash out of `app`.** A small crate that only
   `cli`'s `main.rs` reads would stop every commit and checkout from
   recompiling `app` and `cli`. Worth it only after 4.
6. **Stop hashing `Cargo.lock` into `boot`'s build script** for the
   hashes that do not need it, so adding a dependency elsewhere does
   not recompile `boot`.
7. **Optimise more in dev,** if test run time (not build time) is
   still the cost after the above. Candidates, each to be measured with
   `--config 'profile.dev.package.NAME.opt-level=2'`: `naga`,
   `wgpu-core` and `wgpu-hal` (shader and pipeline work in the `d3d`
   and `gfx` tests), `cranelift-codegen` and `regalloc2` (the fast
   tier), and the data crates the disc tests spend their time in
   (`h1map`, `h1bitmaps`, `h1models`, `map`, `tags`, `mopp`). That last
   group would let the disc tests run in the dev profile instead of
   needing a second, release, set of artifacts. Also
   `[profile.dev.build-override] opt-level = 3` for the proc macros
   (841 serde derives, sqlx's macros).

## Considered and not done

- **A different linker.** Apple's linker has been the fast one since
  Xcode 15; `ld64.lld` is not reliably faster and is one more thing
  between the build and `libloading`, frameworks and code signing. With
  fewer, larger test binaries the link matters less. If item 10 shows
  links dominating, try
  `--config 'target.aarch64-apple-darwin.rustflags=["-Clink-arg=-fuse-ld=/opt/homebrew/bin/ld64.lld"]'`
  in a scratch target (changing `rustflags` rebuilds everything).
- **`incremental`.** kache owns this setting and overrides the profile.
  What its 16 GB `incremental.kache-auto` directory buys is a question
  for `kache explain boot` after an edit, not for `Cargo.toml`.
- **`codegen-units`.** Left at cargo's default. More units would not
  help a 10 core machine that is already saturated by parallel crates.
- **`resolver = "3"`.** It changes how versions are chosen for a
  minimum Rust version, not how features unify.
- **A workspace-hack crate (cargo-hakari).** It would force one feature
  set for third-party crates, which also pushes the desktop features
  into the host build, the one feature set that must stay separate.
  Running a full sweep as one command gets the same unification with no
  crate to maintain. `resolver.feature-unification = "workspace"` in
  cargo's config would do the same; check whether this cargo has it as
  a stable setting before relying on it, and apply it with `--config`
  to the sweep only.
- **Separate target directories per feature set.** Cargo already keeps
  the host and desktop builds side by side under different hashes; a
  second directory would only duplicate the third-party crates.
- **Duplicate dependency versions.** `Cargo.lock` has 60 names with two
  or more versions (four `windows-sys`, three each of `getrandom`,
  `hashbrown`, `rand`, `rand_core`). Nearly all come from third-party
  crates' own requirements and are small; the ones under our control
  (`thiserror` 1 and 2, `tungstenite` 0.26 and 0.29, `sha2` 0.10 and
  0.11) are worth aligning when those crates are next touched, not as
  a project.
- **The Cranelift codegen backend and `-Zshare-generics`.** Both need a
  nightly compiler. This machine builds with a stable rustc from
  Homebrew.
