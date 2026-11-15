# opt

IR-to-IR optimisation passes between `crates/lift` and `crates/emit`.
`emit::plan` runs them on every owner once its shape is known (start
selectors and polled loop headers), so everything downstream, the C and its
tests, sees optimised IR. The interpreter can run optimised IR too, which is
how the passes are tested without a C compiler.

## The rule every pass keeps

State is observable only where a handler can stop: transfers, calls,
returns, faults (any load, store, `FaultIf` or intrinsic may fault), stops,
atomics (the runtime reads the spilled state) and preemption polls at loop
headers. At each of those, the registers, EFLAGS, the defined-flags mask and
memory must be exactly what the unoptimised IR gives. Between them a pass may
change anything. A handler can also be entered at any start selector, with
its state from `CpuAbi`.

## Passes

Each pass has a name and can be turned off on its own: `H2_PASSES` (read by
`emit::plan`, the tests and the CLI) or `h2 emit --passes`, a comma-separated
spec applied left to right: `all`, `none`, `name`, `-name`. The default is
`all`.

| name | what it does |
|---|---|
| `dead-flags` | Removes flag writes that are rewritten on every path before anything above can observe them, then the formulas that only fed them (`flags::dead_flags`, `dce`). |
| `lazy-flags` | Groups an instruction's flag writes that nothing reads before they are rewritten, apart from a `ReadFlag` in the same block, into one `LazyFlags`: a shared formula over values the instruction computes anyway (`lazy::lazy_flags`). The C keeps the inputs in slots and computes the flags only at an exit or a read, so faults and polls see exact flags without the hot path paying for them. |

`lazy-flags` runs first: it keeps an instruction's flags together, and
`dead-flags` then removes a `LazyFlags` only if all its flags are dead.
Splitting a group would force the C to settle the survivors early.

After any pass, `dce` removes pure statements whose results became unused.

## Testing

- `tests/flags.rs` and `tests/lazy.rs`: what each pass changes and keeps, and an interpreter
  equivalence check: every fixture, from hundreds of random states with
  random preemption at loop headers, ends the same way with identical CPU
  state (flags and defined mask included) and memory, optimised or not.
- `crates/emit/tests/differential.rs` compares the interpreter on the
  *unoptimised* IR with the generated C of the optimised IR.
- `crates/lift/tests/oracle.rs` with `H2_ORACLE_PASSES=all` runs the Unicorn
  corpus through the passes.
- `crates/emit/tests/opt_bench.rs` (ignored) times synthetic hot loops per
  pass selection, in thread CPU time: `H2_BENCH_PASSES="none;all;-dead-flags"`.

## Measurements

Retail image, `h2 emit --no-build`:

| passes | flag writes removed | made lazy | C size |
|---|---:|---:|---:|
| none | 0 of 1,476,772 | 0 | 417.4 MB |
| `dead-flags` | 371,608 (25.2%) | 0 | 373.1 MB |
| `lazy-flags` | 0 | 1,045,207 (70.8%) | 538.9 MB |
| all | 336,097 (22.8%) | 1,045,207 | 504.5 MB |

Full `h2 build` (clang `-O1`, arm64, every object compiled, on a shared
machine): none 214.2 MB of objects, a 128.5 MB dylib, 1229 s of clang CPU;
all 223.5 MB (+4.3%: the exit-path settles and each handler's `h2lz_`
switch), 138.8 MB, 1336 s (+9%, within the machine's noise).

Hot loops, ns per iteration (`opt_bench`, `--release`, thread CPU time,
best of 61):

| loop | none | `dead-flags` | `lazy-flags` | all |
|---|---:|---:|---:|---:|
| alu | 2.78 | 2.78 | 1.97 (1.41x) | 1.96 (1.41x) |
| sum | 11.58 | 11.57 | 11.30 (1.02x) | 11.26 (1.03x) |
| copy | 17.91 | 17.95 | 17.48 (1.02x) | 17.42 (1.03x) |
| strcmp | 13.90 | 13.35 | 13.37 (1.04x) | 12.82 (1.08x) |

`h2 bench --only hot-loop` (crates/perf, the real runtime): `alu` 2.97 to
1.04 ns per trip (2.85x); `memory` and `call` unchanged (their minimums,
111-116 and 175-180 ns, are the accessor calls and the runner's round trip).

The main menu idle (docs/BOOT.md, presents 440-560, the same 25,271 slices
either way), second runs: none 1.86 fps, 501 ms median frame, 139 s user
CPU; all 2.44 fps, 376 ms, 129 s. First runs, under heavier load: none
0.96 fps (153 s), all 1.95 fps (138 s). Wall-clock frame rate moves with
the machine's load more than with the passes; user CPU is 7-10% lower with
them. The menu's frame time is mostly not in flag arithmetic.

The memory loops are dominated by the calls to the memory accessors, which
no IR pass removes.

`dead-flags` does not make the hot loops faster at `-O1`: clang already
deletes flag updates no exit can see. What it removes is the C clang would
have parsed and optimised away, so its gain is size. Flags observed only by
a possible fault or a poll were the bulk of what it left; `lazy-flags`
moves those to the exit path.

Two things decide whether `lazy-flags` pays: where paths meet with
different formulas pending (a loop header reached from before the loop and
from its back edge), the C settles in place with a `switch` on `lzk` over
the few formulas that can be pending rather than calling `h2lz_`; and an
instruction whose only lazy candidates are CF/OF (a rotate, `bt`) stays
eager, because deferring a bit the instruction computes anyway only forces
the next lazy instruction to settle the flags it leaves pending. Without
those two, the perf crate's `alu` loop was 1.6x *slower*.
