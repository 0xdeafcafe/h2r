# The first-person filter step (`11eed0`)

Status: Partly built. The filter step runs as one Rust call wherever the
first-person filter port calls it; the port `166d75` around it stays a
register-level port. Its retirement and the unit aim update remain in phase 1c.
Code: `crates/rewrites/engine/src/sim_anim/filter.rs` (`step`, `run`).
Tests: `crates/rewrites/engine/src/sim_anim/filter/tests.rs`,
`crates/rewrites/engine/tests/sim_physics_differential.rs`
(`first_person_filter_kernel`).
Switch: `impl.sim.native.anim` (the port's own
`impl.sim.native_first_person_filters` gates the vehicle), listed in
[switches.md](../runtime/switches.md).

Phase 1c of the [simulation rewrite](sim-rewrite.md). A first-person
seat's weapon view eases each of four axes toward its target once a
tick. `11eed0` is one axis's step: it moves a position and its velocity
toward a target under a speed bound and an acceleration bound, wraps
across a range when asked, and clamps into it. The register-level port
(`engine::sim_physics::first_person::filters`) stays the vehicle; at each
of its calls to `11eed0`, `engine::sim_anim::filter::run` does the call's
work in Rust and the port carries on at the return site with no guest
transfer. What the call leaves — memory, registers, flags, MXCSR and the
x87 state — is the guest's, bit for bit.

## How it works

1. **The call.** `run(c, g, site)` is entered where the port would make
   the call: the five stack arguments pushed at `esp` — the target, the
   tick length, the acceleration bound, `lo` and `hi` — `esi` the
   velocity, `edi` the position, the speed bound in `xmm5`'s low lane
   and the wrap flag in `dl`. The port's four filter calls (`166d75`,
   returning to `166edb`, `166f1e`, `166fcf` and `16700c`) and the
   weapon-slot step's call (returning to `1673ff`) go through it.
2. **Gather** (`plan`). The body's stack window, from the call's `esp`
   less 16: the three locals, the return address (the site) and the five
   arguments; the velocity and the position; the registers, MXCSR and the
   x87 state; `0.5` and `2.0` from the image (`[0x45dbbc]`,
   `[0x45dc0c]`).
3. **Kernel** (`step`), on the gathered state with no guest memory, in
   the guest's instruction order on the `simd` and `x87` crates:
   - the distance to the target, taken the short way round `[lo, hi]`
     when the wrap flag is set;
   - the tick's reach, the smaller of `tick² · accel` and
     `tick · speed`;
   - **snap**, when `|distance − velocity · tick|` is within the reach:
     the position becomes the target clamped into `[lo, hi]`, the
     velocity zero, and `al` 1;
   - otherwise **move**: the speed that stops at the target,
     `sqrt(2 · accel · |distance|)` (x87) capped at the speed bound and
     signed toward the target; the acceleration that reaches it in one
     tick, stored over the tick argument and, when its size passes the
     bound, replaced by the bound with its sign (`eax` the sign, from
     `lahf` / `test ah, 44h`); the new velocity, and the new position
     `(½ · a · tick + v') · tick + p`, wrapped and clamped into
     `[lo, hi]`; `al` 0.
4. **Scatter** (`scatter`), in the guest's store order: the return
   address, the stack slots the body writes (the velocity, squared speed
   and reach locals, the target and tick arguments), then the velocity
   and the position. Then the registers: `xmm0`–`xmm4`, `xmm6`, `xmm7`
   and MXCSR as the kernel leaves them; `al` and `cl` the snap answer
   (`eax` above `al` the sign when it was taken); the x87 registers,
   status and tags, with the last instruction the path's closing
   `fstp st(0)` (`11ef8f` or `11f01c`, FOP read from the image) and the
   last memory operand its `fld` (`[esp+4]` or `[esp+18h]`); the flags of
   `add esp, 0Ch`; and `esp` past the arguments, as `ret 14h` leaves it.

## Interfaces

- Entered only from the first-person filter port and its weapon-slot
  step, at the calls listed above; nothing else calls `run`. The sim
  objects' device body calls `11eed0` as a guest callout and does not
  use the kernel.
- `step(&In) -> Result<Out, Decline>` is the call as a pure function of
  gathered state, for tests and later callers.
- `engine::sim_anim::filter::stats()` counts calls, snaps and declines
  by reason.

## Rules and limits

The call declines, writing nothing (the port makes the guest call),
when:

- a guest read faults (`Fault`);
- an instruction would deliver an unmasked SSE or x87 exception
  (`Float`) — the port's `supported` check keeps both masked, so this
  does not happen under the port;
- the velocity or the position overlaps the call's stack window
  (`Alias`): the body reads them before its stack stores and writes them
  after, which the gather does not model when they share bytes.

The arithmetic is the guest's own operation sequence on the emulators,
so there is no fast-path range and no precision or rounding decline.

## Evidence

- `filter/tests.rs` (engine, no image): hand-worked calls down every
  path — snap and the clamped snap, the acceleration bound with either
  sign, the speed bound, the wrap before the reach test and the wrap of
  the new position — with the stack, register, flag and x87 shape each
  leaves; every decline leaving RAM and the CPU untouched; the switch;
  `step` against `run`.
- `sim_physics_differential.rs` (`first_person_filter_kernel`,
  `H2_XBE`): the filter port with the kernel on, against the lifted
  `166d75` with `11eed0` run inside the segment — all memory and
  registers at every boundary — over twelve cases of the first-person
  fixture across x87 precisions and MXCSR rounding modes.
  `first_person_filter_update` keeps the port itself equal to the
  lifted guest with the kernel off.

## Not verified

- Run parity: the sim-budget A/B with `-impl.sim.native.anim` against
  on, identical `state_hashes_v1` and an empty `h2 sim diff`, with the
  first-person ports on and a mapped local seat; the decline rate on
  real seats (`stats()`).

## Related

- [sim-rewrite.md](sim-rewrite.md), section 5, phase 1c.
- [palette.md](palette.md): the first-person pose port's palette step.
- [sim-budget.md](../perf/sim-budget.md): `166d75` and `16760c` on the
  tick.
