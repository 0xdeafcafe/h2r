# Build iteration measurements

Measured on 2026-11-15, Apple M1 Max (10 CPUs, 64 GiB), Apple clang 21.0.0,
using the release `h2` binary, `-O1 -gline-tables-only`, ten clang jobs,
and the user's read-only retail XBE. Base: `a41dae4`. Other lanes were
building and running guests concurrently; these are single-run wall times
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
serialization/version contract with the analyzer and IR lanes.

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
introducing a serialization contract into the other lanes.

## Output equivalence

All six full-image `--run-entry` runs produced identical transfers, GPRs,
EFLAGS/defined masks and event lists. Before/after manifests match for both
the original and changed images. Every one of the 1004 generated C/support
files was also compared byte for byte for the changed image and matched.
The warm after run hits the emission receipt, skips the three front-end
stages, and compiles zero objects.

## Validation

`cargo test -p cli -p emit --release -j 2`, with `H2_XBE` set to the user's
image, passed 117 tests with two existing tests ignored, including all
three new `emit_options` integration tests. The ignored tests are the retail
ffmpeg movie conversion and the back-edge benchmark. There were no runtime
skips. This includes the retail
entry comparison against the IR interpreter (CPU state and guest memory),
generated-C differential tests, cache invalidation tests, and incremental
object/header tests.

`cargo clippy -p cli -p emit --all-targets --release -j 2 -- -D warnings`
passed. Tests and clippy were repeated after enabling hardware SHA dispatch.
Existing Rust files
were not rustfmt-clean before this change, so only the two new Rust files
were formatted, per lane rules.


## Review corrections

The preceding full-image timings describe the original implementation, before
Astra's review fixes. They do not include the new per-file dependency-discovery
cost. The original unguarded receipt transaction is superseded by an output
lock; repository output/cache paths now require `--allow-repo-output`; and
object keys include every transitive header discovered by clang. Regression
tests impose the reviewed A/B receipt schedule, exercise path aliases and
relative/default repository locations, and compare cached versus clean
execution after changing an included constant. Full-image timings have not
been repeated for these correctness changes.


After rebasing onto master `4330dfb`, review-fix validation passed 131 tests
with the same two explicit ignores and no runtime skips:
`RUSTC_WRAPPER= cargo test -p cli -p emit --release -j 2 -- --test-threads=2`,
with `H2_XBE` set to the user's read-only image. This includes retail entry
CPU/memory equivalence and the scheduled receipt race, repository location,
transitive-header, new-shadow, and cross-source-directory reuse regressions.
`cargo clippy -p cli -p emit --all-targets --release -j 2 -- -D warnings`
was also clean with `RUSTC_WRAPPER=`.


The second review also adds full output/cache-tree symlink rejection and
`O_NOFOLLOW` on the transaction lock. The regression uses the review's valid
synthetic XBE and a temporary Git repository, checking that source-directory
and header-leaf aliases neither emit files into the repository nor overwrite
a sentinel. ABI, pin, receipt, library, lock, external-cache and dangling
aliases are covered too. A clean synthetic emission is the positive control.

Object keys additionally fingerprint the effective `clang -###` driver
configuration and reported configuration file contents. Real-clang probes
cover `CCC_OVERRIDE_OPTIONS`, explicit `--config`, target-specific default
configs, comment-only edits, and nested response-file option changes. Cached
objects execute the new constant and match clean builds. Source-argument
normalization leaves configuration paths and macro values unchanged. Prior
full-image timings exclude this additional driver discovery and tree-inspection
cost; no new full-image benchmark has been claimed.


Second-review fixes validated after rebasing onto master `367d4ca`:
136 tests passed, two explicitly ignored, no runtime skips, including retail
entry CPU/memory equivalence. Commands were `cargo test -p cli -p emit
--release -j 2 -- --test-threads=2` with `H2_XBE` set to the user's image and
`cargo clippy -p cli -p emit --all-targets --release -j 2 -- -D warnings`.
Both used `RUSTC_WRAPPER=` and a private target directory. The target and
all scratch fixtures were removed after validation.


The final hard-link correction replaces mutable output through fresh sibling
temporaries and rename, including C/headers, ABI installation, manifests,
receipts, bridge source/object files, static archives and host dylibs.
Astra's hard-linked repository-sentinel probe is a CLI regression test for
all generated C/support/manifest destinations. Native-output tests cover
ABI, bridge, archive and dylib links, execute the linked synthetic functions,
and verify the dylib's final install name. Cached objects and pins still
share an inode and an unchanged warm build reuses the same object.
No full-image performance timings have been repeated for these publication
changes.

Hard-link fixes validated against reviewed parent `0413935`: 139 tests passed,
two explicitly ignored, no runtime skips, including retail entry CPU/memory
equivalence. Commands were `cargo test -p cli -p emit --release -j 2 --
--test-threads=2` with `H2_XBE` set to the user's image, and
`cargo clippy -p cli -p emit --all-targets --release -j 2 -- -D warnings`.
Both used `RUSTC_WRAPPER=` and a private target directory.
