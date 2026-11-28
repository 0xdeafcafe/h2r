# Shadow execution: ports checked on the states the game reaches

Status: **built, not yet run on the disc.** Item 6 of the
[architecture review](../reviews/2026-11-20-architecture.md) (section 3.4,
"Shadow execution in place of record-and-replay").

The differential tests in `crates/rewrites/engine/tests/*_differential.rs` check
each native port against the lifted original from seeded, synthetic
states. Shadow execution checks the same ports while the game runs: at a
port's call, the port and the lifted original each run on their own copy
of guest memory from the state the game is in, and their results are
compared. A mismatch is recorded with the port, the function, the tick,
the entry registers and the diff. The agent API turns it on, so there is
no switch or variable for it.

## The `lifted` crate

`crates/runtime/lifted` is the one "run the lifted original" fixture. Before it,
each differential test rebuilt its own: image loading and pinning, a lazy
lifter cache, the register conversion between `runtime::CpuAbi` and
`ir::interp::Cpu`, and a stepping loop.

| Module | What |
|---|---|
| `image` | the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by SHA-256, parsed, analysed and loaded once per test binary; skipped without it. `Rng` (SplitMix64) for seeded fixtures |
| `code` | `Code`: instructions by address. `Original` lifts the image's functions on demand (the nearest function at or below an address that holds it, `prime` to give a function's own edges precedence). `Assembled` lowers raw x86 for synthetic tests. `step`, `segment` (to the next call or return, as a segmented port hands back) and `run_until` |
| `cpu` | `to_interp`, `from_interp` (exact both ways) and `diff` (named register differences) |
| `shadow` | the check and the process's registry ([below](#the-check)) |

Migrated: `sim_native_differential.rs`, `sim_physics_differential.rs` and
the engine tests' shared `common` helpers. The render differentials in
`crates/rewrites/engine/tests` and the two in `crates/rewrites/native-render/tests` still
carry their own copies; each moves when next touched.

## The check

`lifted::shadow::check`, for one call of a port at `pc` with entry state
`entry`:

1. **The port on a copy.** A copy-on-write view of the live Guest
   (`Guest::cow_snapshot`, then `cow_refresh` on later checks, which keeps
   the view's page table while the live layout holds; see
   [second-executor.md](../runtime/second-executor.md), section 2). The
   view's write watch gives the pages the port stored to; the bytes it
   changed are those that differ from the live Guest, which nothing has
   written since.
2. **The original on the interpreter.** From `entry`, over an overlay:
   reads see the live Guest under the original's own writes, writes stay
   in the overlay keyed by folded physical address, so the two sides are
   compared in one address space whatever the mapping.
3. **Where to stop.** The port's transfer says it: its target with the
   port's ESP. The original steps into calls on the way. Each pass through
   that point is compared: the transfer kind (a call with the same
   continuation, a return), every register (EFLAGS on the bits both sides
   define), and the bytes each side changed. The first pass that matches
   in full is a match. A port that yields at a loop head after several
   iterations therefore matches the pass it yielded at.
4. **Mismatch.** The original returns out of the frame it was entered in
   with no matching pass: the first pass's differences, or `returned` when
   it never passed the point.
5. **Skipped**, counted by reason and never a mismatch: the port faulted
   on its copy, the original reached code the interpreter has no
   instruction for (a kernel import token, an address no function holds),
   an interpreter exit (a fault, an unsupported instruction), or the step
   limit.
6. **The real call.** For an authoritative port, `Shadow::run` runs the
   port on the live Guest. If its transfer or registers differ from the
   copy's, the call is a `nondeterministic` mismatch. A guest-authoritative
   arm calls `Shadow::compare_only` and dispatches the generated original
   on the live Guest. In that arm the Rust port runs only on sampled copies.

Sim ports split at their callees (`sim_physics`) are entered at the
function and at each registered resume; each segment is checked from the
state it starts in.

Cost: one relaxed atomic load per port call while nothing is on. A check
copies RAM copy-on-write (a VM remap on macOS, a 64 MB copy elsewhere),
runs the port on the copy and the original on the interpreter before the
live call. Use `every` to sample hot ports.

## Registration and the agent API

`feature::sim_native`, `feature::sim_physics` and `feature::sim_seats`
register every port with the registry at plan build
(`feature::shadow::register`), installed or not, under its name and its
group (`sim.native`, `sim.physics`, `sim.seats`). Calls pass through one
of two arms, and the two never mix on one port:

- **`shadow::run`** (the per-function ports): the body is authoritative;
  a sampled call is checked before it runs. A guest-authoritative body
  that only compares uses `feature::shadow::compare_only`.
- **`Shadow::check_before_original`** (the seats, phase 7 of the
  simulation rewrite): while the port's checks are on, the *original*
  answers on the live state, sampled or not and after a mismatch too, and
  the sampled call runs the body on a copy for the comparison. A body
  that declines is recorded as a skipped check. Turning the checks off
  (`shadow.disable`) makes the bodies authoritative. That is the seats'
  rollout: parity first, authority after.

Only installed ports are checked, and `shadow.enable` turns its checks on. The seats sit behind the
one `impl.sim.native.seats` switch and are named per entry
(`sim.seats.player_control_update`, ...). Boot's run gives the registry
the image once the module is loaded
(`recomp`'s [`fallback.rs`](../../crates/runtime/recomp/src/fallback.rs)
`shadow_original`): the build's cached analysis, lifted on demand, built
at the first `shadow.enable`.

The object subsystem registers six points in the `sim.native.objects` group
under their guest point names. Its installer in
`crates/rewrites/sim-bridge/src/sim_objects.rs` uses the same subsystem
switch for all six. A sample covers one entry or resume through its next
transfer, including the call's return-address push or the return's pop.
Checks also see calls and resumes fused inside the object dispatcher.
Callees run separately, so each live event or callback is delivered once.

| Object walk | Function lookup | Light | Unit |
|---|---|---|---|
| `OBJECTS_PRE_PHYSICS`, `OBJECTS_POST_PHYSICS` | `OBJECT_FUNCTION_VALUE` | `LIGHT_RECORD_UPDATE` | `UNIT_CLAMP_DIRECTION`, `UNIT_ACTIONS_UPDATE` |

When checks are off, the dispatcher reads one atomic at the start of
each fused chain. During a run, `shadow.status` reports the samples,
skips and mismatches for each named point. A point with zero checked
samples still needs a gameplay case that reaches it.

Methods ([agent-api.md](../runtime/agent-api-checks.md#shadow-checks)):
`shadow.enable`, `shadow.disable`, `shadow.status`.

```sh
h2 serve ...
h2 agent shadow.enable '{"ports":["sim.port.unit_pose","sim.physics"],"every":4}'
h2 agent shadow.status
```

## The disc gate

`scripts/shadow-gate.sh [MAP] [TICKS] [PORT|GROUP|all ...]` runs
`h2 sim run` on a two-phantom match on MAP with the named ports' `impl.`
switches added to `H2_FEATURES` (the seats' one switch once, for any
`sim.seats` name), waits for gameplay, turns their checks on, waits
TICKS ticks and reads `shadow.status`. With the seats checked the match
answers exactly as retail - the guest arm - so the run also proves the
phantom seats' path with the bodies installed. It fails when a named
port is not installed, when any call mismatched, or when nothing was
checked; the verdict, with the full status, is in its output directory.
It never builds: build the image first (same `H2_OPT`), and the map needs
its nav mesh.

```sh
H2_OPT=2 scripts/shadow-gate.sh lockout 900          # standalone ports
H2_OPT=2 scripts/shadow-gate.sh lockout 1800 sim.physics
H2_OPT=2 scripts/shadow-gate.sh lockout 1800 sim.native.objects
SHADOW_EVERY=8 H2_OPT=2 scripts/shadow-gate.sh midship 3000 all
```

The gate's `all` profile selects the standalone ports. The
`sim.native.objects` profile runs separately because the object
dispatcher owns entries also used by standalone ports. The registry
tests in `sim-bridge` check that the script lists every port.

## Limits

- A port that is off is not checked: the game runs the generated original
  and there is no hook to run the port beside it. The seats are the
  exception in authority, not in this: off, nothing of theirs installs.
- The original runs on the IR interpreter, not the generated code: a
  difference between those is the lifter's (its own differential suites),
  not the port's.
- Bytes are compared by physical address; a mismatch lists up to two
  virtual addresses for each run of bytes.
- The fixture is the entry registers. Saving the bytes the original read
  as a replayable fixture is the next step.
