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
| none | 0 of 1,476,706 | 0 | 417.4 MB |
| `dead-flags` | 371,560 (25.2%) | 0 | 373.1 MB |
| `lazy-flags` | 0 | 1,058,370 (71.7%) | 538.3 MB |
| all | 336,041 (22.8%) | 1,058,370 | 505.2 MB |

Clang at `-O1` on every 25th shard (40 files, CPU seconds, on a loaded
machine): none 46.4 s and 8.49 MB of objects; all 43.4 s and 8.86 MB
(+4.4%: the exit-path settles and each handler's `h2lz_` switch). The C is
larger but cheaper to compile, because the flag code that is left is
mostly on cold paths.

Hot loops (`opt_bench`, `--release`, ns per iteration, thread CPU time,
best of 61):

| loop | none | `dead-flags` | `lazy-flags` | all |
|---|---:|---:|---:|---:|
| alu | 2.95 | 3.03 | 2.08 (1.42x) | 2.07 (1.42x) |
| sum | 11.28 | 11.39 | 10.94 (1.03x) | 11.02 (1.02x) |
| copy | 17.58 | 17.74 | 17.02 (1.03x) | 16.99 (1.04x) |
| strcmp | 13.43 | 12.60 | 12.19 (1.10x) | 12.16 (1.10x) |

The memory loops are dominated by the calls to the memory accessors, which
no IR pass removes.

`dead-flags` does not make the hot loops faster at `-O1`: clang already
deletes flag updates no exit can see. What it removes is the C clang would
have parsed and optimised away, so its gain is size and compile time.
Flags observed only by a possible fault or a poll were the bulk of what it
left; `lazy-flags` moves those to the exit path.
