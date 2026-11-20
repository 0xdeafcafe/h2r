# Nightly Rust for dev builds: Cranelift and parallel front end (2026-11-20)

Measured on master `a1502e02d`, on the 10-core M3 used for development,
with other work running (smoke numbers, not lab numbers). kache was off
for every run (`--config "build.rustc-wrapper=''"`) and each configuration
built into its own empty target directory.

| Configuration | Clean `cargo build -p cli` | Edit in `boot`, rebuild `cli` | `cargo check -p cli` after | Target size |
|---|---:|---:|---:|---:|
| A. stable (Homebrew rustc) | 136 s | 8 s | 49 s | 8.5 GB |
| B. nightly, LLVM | 122 s | 8 s | 36 s | 6.6 GB |
| C. nightly, `-Zthreads=8` | 110 s | 7 s | 36 s | 6.6 GB |
| D. nightly, `-Zcodegen-backend=cranelift` | 98 s | 7 s | 39 s | 6.0 GB |
| E. nightly, Cranelift and `-Zthreads=8` | 90 s | 7 s | 36 s | 6.0 GB |

Nightly was `nightly-aarch64-apple-darwin` (rustc 1.102.0-nightly,
2026-11-19) with `rustc-codegen-cranelift-preview`, installed under a
private `RUSTUP_HOME`.

Correctness under E: the tests of `d3d`, `engine`, `hooks`, `runtime`,
`kernel`, `x87` and `simd` gave 1,417 passed and 1 failed:
`runtime --test write_watch muting_survives_a_panic` panics at
`crates/runtime/runtime/tests/write_watch.rs:162`, where stable passes.

## Verdict

Not adopted. The loop that matters, an edit followed by a rebuild, goes
from 8 s to 7 s. The clean-build gain (34% for E) is mostly what kache
already serves across worktrees. Most of the `check` gain comes from the
nightly compiler itself (B), not from either flag. Against that, the
project would depend on a moving nightly, and Cranelift breaks a test
that relies on unwinding through a panic.

## Reproduce

`nightly-measure.sh` in the coordinator's keep directory ran the five
configurations: per configuration, a clean `cargo build -p cli`, one
appended comment in `crates/apps/boot/src/lib.rs` and a rebuild, then
`cargo check -p cli`, each timed with `date +%s`.
