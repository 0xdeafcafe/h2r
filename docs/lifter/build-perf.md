# Build iteration measurements

Measured on 2026-11-15, Apple M1 Max (10 CPUs, 64 GiB), Apple clang 21.0.0,
using the release `h2` binary, `-O1 -gline-tables-only`, ten clang jobs,
and the user's read-only retail XBE. Base: `a41dae4`. Other builds and
guests were running concurrently; these are single-run wall times
under shared load, not isolated-machine medians. Rust compilation is excluded.

Each primary case includes `--run-entry`; separate build-only and SHA probes
are labelled below. Cold means a fresh output directory
and no shared object cache; warm repeats the command unchanged. The changed
case modifies one immediate in a discovered leaf function at `0x73790` in a
private XBE copy, keeping instruction lengths unchanged. Only
`shard_00073.c` changed in both runs. The benchmark checks the generated manifests
and entry result as well as cache hit counts.

The original CLI builds both architectures; the changed CLI defaults to the
host (arm64). This is an intentional change in build work, not faster clang
flags. `--arch all` retains the original coverage. Generated code, compiler
flags, sharding and scheduling are unchanged.

## Results

| Case | Before, both architectures | After, host only |
|---|---:|---:|
| Cold | 1400.842 s | 658.255 s |
| Warm | 66.534 s | 3.572 s |
| One function changed | 75.809 s | 63.983 s |

Cold builds compile 2002 versus 1001 objects. Warm builds compile zero in
both cases. The changed-function case compiles two objects before (one per
architecture) and one after, reusing all other objects. The cold improvement
is predominantly the omitted x86_64 build, not faster optimization passes.

The original supplied eight-minute cold / eighteen-second warm figures were
not reproduced under the concurrent load. In the measured warm run, analysis
cost 39.4 s, lift/plan 4.5 s, and C emission 16.9 s; none of these stages
changed the output. Cold analysis was only 10.0 s, illustrating the scheduling
noise. Cold clang wall time was 620.5 s arm64 and 743.4 s x86_64.

| Cold per-file clang wall | Median | p95 | Maximum |
|---|---:|---:|---:|
| arm64 before, 1001 files | 5.421 s | 11.531 s | 37.389 s |
| x86_64 before, 1001 files | 6.717 s | 12.915 s | 39.609 s |
| arm64 after, 1001 files | 5.098 s | 11.279 s | 55.526 s |

The changed shard compiled in 6.3 s arm64 and 6.8 s x86_64. The cold host
bridge took 0.185 s and the dylib link 0.933 s, measured separately by wrappers.
These per-file values include time waiting for CPU; they are not CPU usage.

## Changes

- Reuse validated C output when the XBE, package version and executable
  match. Missing, modified or extra source files invalidate the receipt.
- Enable runtime hardware SHA dispatch. A full-image receipt validation
  dropped from 8.649 s with software SHA to 0.968 s with hardware SHA under
  load; the complete source/manifest digest lists are identical.
- Build the host architecture by default. Keep both targets with `--arch all`.
- Expose `--jobs` and separate cache, clang, archive, bridge/link and total
  times. Keep output under `$TMPDIR` by default.

Per-file content caching, stable address sharding, isolated oversized owners,
large-file-first scheduling and configurable C profiles already existed on
the base branch. They were retained. There is no measured reason here to
sacrifice optimization or alter the generated C. A generator rebuild still
reruns the front end; caching IR across such rebuilds would require a separate
serialization/version contract with the analyzer and IR.

## Remaining warm work

The warm run reports the following internal CLI timings:

| Stage | Time |
|---|---:|
| Validate image/executable/source receipt | 0.719 s |
| Toolchain checks, source hash and object pins | 1.037 s |
| Validate static archive | 0.032 s |
| Host bridge compile plus dylib link | 0.569 s |
| Load/run entry and other CLI work | 1.204 s |
| Total inside CLI | 3.561 s |

The wrapper measures the warm bridge at 0.086 s and link at 0.356 s; the
combined CLI figure also includes process/wrapper startup. A subsequent warm
build without `--run-entry` took 1.718 s wall (1.691 s inside the CLI), with
no analysis, lifting, C emission or clang compilation. The corresponding
source/object/archive checks were 0.629 / 1.033 / 0.019 s. This is the remaining
measured build-validation cost under this load; entry verification adds
roughly another two seconds. None of the repeated analyze/lift/emit work is
necessary for a validated unchanged image and generator. These results do
not establish an absolute lower bound or a precise fraction of the supplied
18-second figure, which was measured under different load.

A changed image still requires analysis and generation: the after incremental
run spent 31.129 s in analysis, 6.577 s in lift/plan, 16.606 s in emission,
1.368 s recording the receipt, and 4.063 s compiling its single changed
shard. Its receipt lookup, object lookup, archive and host bridge/link took another
0.007 / 0.976 / 0.447 / 0.849 s respectively. Persisting analysis/IR across
generator rebuilds is the next separate design question; this change avoids
introducing a serialization contract between the crates.

## Output equivalence

All six full-image `--run-entry` runs produced identical transfers, GPRs,
EFLAGS/defined masks and event lists. Before/after manifests match for both
the original and changed images. Every one of the 1004 generated C/support
files was also compared byte for byte for the changed image and matched.
The warm after run hits the emission receipt, skips the three front-end
stages, and compiles zero objects.

## Validation

`cargo test -p cli -p emit --release` with `H2_XBE` set to the user's image
passed, with two tests ignored by design (the retail ffmpeg movie
conversion and the back-edge benchmark). This includes the retail entry
comparison against the IR interpreter (CPU state and guest memory),
generated-C differential tests, cache invalidation tests, and incremental
object/header tests. `cargo clippy -p cli -p emit --all-targets --release --
-D warnings` was clean.

## Later correctness changes

The timings above describe the first implementation. Review then added, in
order:

- an exclusive output lock in place of the unguarded receipt transaction;
- refusal of output and cache paths inside a git work tree unless
  `--allow-repo-output` is given;
- object keys over every transitive header clang reports (`-M`), and over
  the expanded `clang -###` driver command and its configuration files;
- rejection of symlinks anywhere under the output and cache roots, and
  `O_NOFOLLOW` on the lock;
- publication of every mutable output through a fresh sibling temporary
  and a rename, so hard-linked outputs never change in place;
- a pinned compiler working directory (2c44a25), so objects are reused
  across output directories and working directories.

Each has regression tests; [BUILD-CACHE.md](BUILD-CACHE.md) describes the
result. Full-image timings were not repeated after these changes, so they
do not include the added dependency and driver discovery or the tree
inspection.

## The `h2 build` path after a generator change

Measured on 2026-11-15, same machine, release `h2` from the `emit-speed`
branch against master `2affe31`, one `h2 build` at a time under the shared
emit gate, with other lanes' builds running (load average 25-120). Each
case used a private object cache and boot cache. Single runs, so read them
as rough.

| Case | Before (2affe31) | After |
|---|---:|---:|
| Cold: new `--out`, empty object and analysis caches | 386.3 s | 242.3 s |
| No-op: `build --check` on a current dylib | not measured | 0.02 s |
| Rebuild into the same `--out`, analyzer unchanged | 25.7 s to the link (failed, disk full) | 3.5 s |
| Same, analyzer changed (analysis cache miss) | the row above | 12.2 s |
| `--follow`: new `gen/<short>`, shared object cache | not measured | 4.4 s |
| `--follow` with the last game's `src/` cloned first | | 3.8 s, plus a 0.17 s clone |

| Phase, rebuild into the same `--out` | Before | After |
|---|---:|---:|
| analyze | 8.5 s | 0.0 s (cached) |
| lift | 0.8 s | 0.2 s |
| plan (IR passes) | not printed | 0.6 s |
| emit (1006 files, 505 MB of C) | 4.9 s | 1.0 s |
| keying (probes, hashing, pins) | not printed, about 2000 clang probes | 0.7 s, two probes |
| compile | 0 objects | 0 objects |
| link | failed | 0.5 s |

- The cold case is dominated by clang (350.2 s before, 223.9 s after). That
  difference is load, not this change. Before, about 14 s of the cold wall
  time was in no printed phase (plan and keying). After, every phase is
  printed and they add up to the total.
- These rebuilds change no C. A real generator change also compiles each
  shard whose C changed, about 5 s each under this load.
- `--follow` today writes all 1006 files into the new directory. Cloning
  the last game's `src/` first rewrote none of them. The emit took 1.1 s
  instead of 1.4 s, and the disk used by the new directory fell from about
  877 MB to about 393 MB. That's free space before and after, so other
  lanes add noise, but the difference is about the 505 MB of C. The rest
  is the archive and the dylib.
- The C is byte-identical. The parallel emit's `src/manifest.tsv` matches
  the one master wrote for all 1006 files. The run into the cloned
  directory produced the same manifest again.

## Where clang's time goes (2026-11-16)

Measured on the same M1 Max under heavy shared load (load average about
220 on 10 cores), so only CPU (user) time and clang's own `-ftime-trace`
are quoted, never wall time. The C is a full image's `src/` from master
(1007 files, 552 MB). The sample is ten shards picked evenly by size rank
(5.5 MB, every hundredth file by size), compiled one at a time with the
build's own flags and `-target arm64-apple-macos11`.

| Profile, ten-shard sample | clang user time | vs `-O2` |
|---|---:|---:|
| `-O2 -gline-tables-only` (`H2_OPT=2`, what the author's builds set; the built-in default is `-O1`) | 56.8 s | 1.00 |
| `-O2`, no debug info | 53.5 s | 0.94 |
| `-O1 -gline-tables-only` (the default) | 47.5 s | 0.84 |
| `-O0`, no debug info | 7.4 s | 0.13 |
| `-fsyntax-only` (parse and type-check only) | 2.6 s | 0.05 |

`-ftime-trace` over the same ten at `-O2` (125 s of traced time): the
front end (parse, semantic analysis, IR generation) is 11%; the rest is
the optimiser and the backend. In the median shard, the greedy register
allocator alone is 26% and the register coalescer 12%;
`CorrelatedValuePropagation` is the costliest optimiser pass (9%).

**The cost is in the biggest functions, and it grows faster than their
size.** Fitting per-function optimiser plus backend time against source
lines over the sample's 62 functions above 200 lines gives time ∝
lines^1.4, and the two largest are steeper still (a 5615-line handler takes
29 s alone, a 2121-line one 4 s). In two of the ten, one handler is 70%
and 86% of its shard's time. Across the image, 663 of 18647 handlers are 2000 lines
or more, and by that fit they are about half of all clang time; the
largest 1000 handlers are 55-75% of it. Splitting files differently does
not touch this: the time is inside single functions.

**What the C is.** Classifying every line of the 552 MB:

| What | Share of bytes |
|---|---:|
| Temporaries (`uint32_t tN = …;`) | 28% |
| Lazy flags computed in place where a branch reads one flag (one line each) | 23% |
| Lazy flags settled in place (`{ uint32_t m_ = lzm & …`) | 11% |
| `h2lz_` per-handler flag switches | 6% |
| Instruction markers (`/* 0x… */ {`) and closing braces | 7% |
| `H2_LD` / `H2_ST` | 6% |
| Lazy-flag bookkeeping (`lz0 = …; lzk = …; lzm = …;`) | 3% |
| Register write-backs, flag writes, branches, labels, transfers | 8% |
| Everything else (x87 and SSE calls, prototypes, dispatch tables) | 8% |

Most of the in-place lazy flag code was dead: reading ZF computed every
flag of the formula (carry, overflow, parity, auxiliary) and kept one.
Clang deleted the rest, after parsing and lowering it.

**Header changes.** Keying objects on clang's IR instead of on source plus
headers was tried and rejected. The IR records the line of every function
the shard calls from a header (even external declarations, for call-site
debug info) and the source offsets of inline assembly, so any change that
moves a line in `h2abi.h` or `h2gen.h` changes every shard's IR. Over the
ten-shard sample: adding one prototype to `h2abi.h`, or a comment at the
top of `h2gen.h`, left 0 of 10 IRs unchanged; reverting the real
`h2gen.h` change d3c4e011 left 0 of 10; reverting the `h2x87_native.h`
changes 8eec6008 and cc557007 left 4 of 10. Header changes in practice
change real code in every shard, so an IR key would save the compile of
under half the shards for the rarest kind of change, at the price of a
front-end run per shard on every miss. (Compiling in two steps, front end
to bitcode and bitcode to object, gives a byte-identical object, so it
would have been safe; it just does not pay.)

## Changes from this measurement

- **Lazy flags read and settled in place declare only what the flags
  they need depend on** (`emit::lazy::live_temps`). Generated behaviour is
  unchanged: the dropped temps are pure and unused, and the emit tests
  (differential against the IR interpreter, accounting, exits, native
  x87 and SSE) pass. Applying the same pruning to the ten sample shards
  makes their C 20% smaller (5.49 to 4.39 MB), so a full image should
  come to about 440 MB: less to write, hash, keep and clone per output.
  It does not make clang faster: 46.1 s before and 46.2 s after over the
  ten. Clang removed the dead code cheaply. The machine code is not
  byte-identical, though: `__text` of one shard went from 211152 to
  209008 bytes, because the optimiser's heuristics saw different input.
  So this changes the generated code's speed by an unmeasured amount.
  Measure fps before keeping it, or drop the commit.
- **Optimisation tiers, opt-in** (`H2_OPT_COLD` and `H2_HOT`,
  BUILD-CACHE.md). With a profile's hot list, sources holding no hot
  instruction compile at a cheaper level. By the table above a cold source
  costs 13% of its `-O2` time at `-O0` and 84% at `-O1`. If, say, nine
  tenths of clang's time is in cold sources, a cold `-O0` build takes
  about a fifth of today's clang time; at `-O1`, about 86%. It changes
  only the speed of cold code, so it is off until fps is measured with it.
