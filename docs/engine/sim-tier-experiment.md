# Sim tier experiment

The experiment behind step 2 of [sim-review.md](sim-review.md) section 6:
compile five hot guest functions with the sim tier of section 4.2 and
measure them against the reference tier (the emitted image) and the hand
ports, with bit-identical effects. The gate is **at least 5x over the
reference tier with identical memory and CPU state** on every case the
tier completes.

| Root | Point | Hand port |
|---|---|---|
| `0x00142a60` | `OBJECT_TRANSFORM_COMPOSE` | `engine::sim_native::compose` |
| `0x001421f0` | `OBJECT_POSE_MATRIX` | `engine::sim_native::pose_matrix` |
| `0x002ff8b0` | `SIM_MOPP_BOX` | `engine::sim_native::mopp_query::box_query` |
| `0x001de6d0` | `COLLISION_BSP_SEGMENT` | `engine::sim_native::bsp::segment` |
| `0x0031c940` | `HAVOK_SOLVER` | `engine::sim_physics` solver port |

## The tier

`crates/emit/src/sim.rs` (`emit::sim`) builds on the same lifted IR as
`emit::write_module`; nothing in the lifter, IR or reference emitter
changes. The tier is chosen per root by the test that builds it: there is
no switch or environment variable.

- **Closure.** `Closure::build` lifts the roots and everything they reach
  by direct calls and tail transfers. Indirect calls are listed, not
  followed.
- **Signatures.** `Tier::build` recovers for each function the registers,
  flags, XMM registers and x87 state it may read before writing (backward
  liveness, callees included), what it may write, its x87 TOP change and
  its `ret n`, as a fixpoint over the closure.
- **Refusals.** A function with an instruction form the tier has no exact
  lowering for (MMX, `rdtsc`, `ldmxcsr`, x87 transcendentals, `fldcw`,
  80-bit loads and stores, lazy flags), or with an x87 instruction at a
  stack depth that is not static, stays on the reference tier. A call to
  it from tier code is a rare event.
- **Values.** Registers, flags and XMM registers are C locals (SSA after
  clang); a function spills only what it writes, and only at exits and
  before calls that read or write it. The x87 stack is eight `double`
  locals indexed by the static TOP, so every index is a constant.
- **Calls.** A direct call is a C call. The guest return address is still
  pushed and popped, so stack memory is identical; the caller checks the
  popped target is its own continuation. Indirect calls go through a
  generated switch from guest address to tier function.
- **Memory.** `base + address`, unchecked, with `base` a parameter.
- **Floats.** SSE add, subtract, multiply, divide and square root are host
  binary32, compiled without contraction; `minps`/`maxps` are the x86
  select; `rcpss`/`rsqrtss` call the exact table model in `h2simd.h`. x87
  values are binary64 while they stay in the subset
  `docs/lifter/x87-native.md` proves exact at PC=53: zero, or normal with
  exponent within 2^±400. PE and C1 are rebuilt exactly from the rounding
  error (`crates/emit/c/h2tier.h`).
- **Boundary.** Each root exports
  `uint32_t h2tier_<entry>(CpuAbi *, uint8_t *base, uint32_t *rare_pc)`.
  It requires MXCSR `0x1f80` (masked, round to nearest, no FTZ), the host
  in the same mode, and an x87 control word with PC=53, round to nearest
  and every exception masked; then it moves only the root's signature
  between `CpuAbi` and the tier's state.

### Rare events

The tier checks each operation instead of reading sticky flags at the
end of a tick (section 4.2's scheme). An operation outside the contract
returns a code at once, every caller passes it up, and the boundary
returns it; the caller restores memory and runs the reference tier. The
tier never continues past one, so it never computes with a value it
cannot vouch for.

| Code | Name | Cause |
|---:|---|---|
| 1 | `H2T_RARE_ENTRY` | FP environment at the boundary outside the contract |
| 2 | `H2T_RARE_FP` | an SSE result is NaN |
| 3 | `H2T_RARE_X87` | an x87 operand or result outside the exact subset (NaN, infinity, denormal, division by zero, an empty or full register) |
| 4 | `H2T_RARE_CALL` | a call to a function the tier refused, or an indirect call to an address outside the closure |
| 5 | `H2T_RARE_FAULT` | a guest fault (`int`, divide error) |
| 6 | `H2T_RARE_STOP` | an IR stop (an unknown switch target, an unmodelled instruction) |
| 7 | `H2T_RARE_RETURN` | a callee returned somewhere other than its continuation |

MXCSR's sticky exception flags are not kept: the comparison excludes
MXCSR. Nothing in the five roots reads them.

## Method

`crates/engine/tests/sim_tier_differential.rs` (feature `differential`)
needs the user's XBE. It compiles the closure of the five roots twice
into one dylib, once by `emit::write_module_with` (the reference tier,
registered with the production runtime's `Dispatch` and run by `Runner`)
and once by `emit::sim::write`, plus a synthetic `ret` at `0x007e0000`.

For each root, 48 seeded cases: random registers and XMM lanes, the x87
control word `0x027f`, MXCSR `0x1f80`, and a fixture adapted from
`sim_native_differential.rs` and `sim_physics_differential.rs` with
seeded floats in 2^±12. Each case runs:

1. **Interpreter** (`ir::interp`) from the root to the harness exit over
   the 8 MiB image. A case it cannot finish is skipped and counted.
2. **Sim tier** on a copy of the same 8 MiB. A rare event is counted. A
   completed case must match the interpreter in every byte and every CPU
   field but MXCSR, or the test fails.
3. **Reference tier** on the production runtime; a disagreement with the
   interpreter is printed and the case is not timed.
4. **Hand port**, where it finishes the call without a transfer (calls
   the port hands back to the guest are not followed).

Timing (`tier_bench`, `--ignored`, release): per case, the minimum over 5
trials of 200 calls, each call preceded by restoring the bytes the call
dirties, minus the same loop with the restore alone. The reference
runner's cost around the empty function is printed once; the table shows
the reference raw and net of it. Each row is the mean over timed cases.

`crates/emit/tests/sim_tier.rs` needs no disc: iced-assembled integer,
`rep movsd`, SSE, x87 and call fixtures, 400 random states each, against
the interpreter.

## Running it

```sh
# No disc: the tier against the interpreter on synthetic code.
cargo test -p emit --test sim_tier
cargo test -p emit --lib sim::

# The five roots against the interpreter (correctness gate).
H2_XBE=/path/to/default.xbe cargo test -p engine --features differential --test sim_tier_differential

# The bench: markdown rows on stdout.
H2_XBE=/path/to/default.xbe cargo test -p engine --release --features differential \
  --test sim_tier_differential -- --ignored --nocapture tier_bench

# The write census (sim-tier-writes.md).
H2_XBE=/path/to/default.xbe cargo test -p emit --release --test sim_tier_writes -- --ignored --nocapture
```

Numbers from a run go in a dated note under `docs/engine/notes/`, with
the commit, the machine and the command; the table below links to it.

## Results

| function | cases | tier ok | rare | timed | tier ns | reference ns | reference - empty ns | port ns | reference / tier | port / tier |
|---|---|---|---|---|---|---|---|---|---|---|
| | | | | | | | | | | |

## Limits

- Inputs are synthetic. The fixtures exercise the paths the existing
  differential tests chose; a film-driven run on the tick is the next
  proof (section 4.2, "Proof").
- The census of writes outside the arena is static: indirect calls are
  not followed, and a store through a pointer of unknown origin is listed
  as unknown. See [sim-tier-writes.md](sim-tier-writes.md).
- Host faults are not caught. The test runs the tier only on cases the
  interpreter has finished inside the 8 MiB, so every access is in range.
