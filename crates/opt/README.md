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

After any pass, `dce` removes pure statements whose results became unused.

## Testing

- `tests/flags.rs`: what each pass removes and keeps, and an interpreter
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

| passes | flag writes removed | C size |
|---|---:|---:|
| none | 0 of 1,476,706 | 417.4 MB |
| `dead-flags` | 371,560 (25.2%) | 373.1 MB |

`dead-flags` does not make the hot loops faster at `-O1`: clang already
deletes flag updates no exit can see. What it removes is the C clang would
have parsed and optimised away, so its gain is size and compile time.
Flags observed only by a possible fault or a poll are the bulk of what is
left (another 29% of all writes); computing those only on the exit path is
the next step.
