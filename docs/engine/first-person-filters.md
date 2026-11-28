# The first-person filter step (`11eed0`)

Status: Implemented. Run parity and timing remain open.
Code: `crates/rewrites/engine/src/sim_anim/segments.rs`,
`sim_anim/filter.rs`, `sim_physics/first_person.rs` and `sim_physics/weapon_slots.rs`.
Tests: `crates/rewrites/engine/src/sim_anim/filter/tests.rs` and
`crates/rewrites/engine/tests/sim_physics_differential.rs`.
Native only, no switch.

`engine::sim_anim::first_person_filters` runs the seat update from
`166d75` through its final return. It gathers the seat, steps its four
view axes, then visits each of the two weapon slots. The slot index,
animation channels, motion and idle timers stay in guest RAM. Calls to
the animation manager, random-duration helper and first-person pose
return to the original frame. The bridge installs the body when the
image exports its entry and all 40 return selectors.

The body computes each segment on a private CPU with an ordered store
log. Loads see earlier stores through either guest mapping of the same
physical bytes. A successful segment commits its stores and CPU before
the next guest call or return. A fault or unsupported floating-point
environment leaves the segment's entry state available to its generated
selector. The individual filter port remains available with the
animation switch off.

## How it works

1. **The call.** `run(c, g, site)` is entered where the port would make
   the call: the five stack arguments pushed at `esp` - the target, the
   tick length, the acceleration bound, `lo` and `hi` - `esi` the
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
   address and each stack store, including values overwritten later in
   the same call, followed by the velocity and position. The snap path
   preserves the tick argument. Then the registers: `xmm0`–`xmm4`, `xmm6`, `xmm7`
   and MXCSR as the kernel leaves them; `al` and `cl` the snap answer
   (`eax` above `al` the sign when it was taken); the x87 registers,
   status and tags, with the last instruction the path's closing
   `fstp st(0)` (`11ef8f` or `11f01c`, FOP read from the image) and the
   last memory operand its `fld` (`[esp+4]` or `[esp+18h]`); the flags of
   `add esp, 0Ch`; and `esp` past the arguments, as `ret 14h` leaves it.

## Interfaces

- The seat update calls the kernel at four axis sites and one slot
  damping site. Its caller continues within the segment after a
  successful inline call. A declined kernel panics
  `anim filter: <reason>`; the guest call to `11eed0` runs only when the
  kernel's switch is off.
- `step(&In) -> Result<Out, Decline>` is the call as a pure function of
  gathered state, for tests and later callers.
- `engine::sim_anim::filter::stats()` counts calls and snaps. The seat
  update publishes counts when the segment commits.

## Rules and limits

The call declines, writing nothing, and the caller panics
`anim filter: <reason>` (no guest call runs), when:

- a guest read faults (`Fault`);
- an instruction would deliver an unmasked SSE or x87 exception
  (`Float`) - the port's `supported` check keeps both masked, so this
  does not happen under the port;
- a position or velocity overlaps the stack, a stack write aliases a
  gathered constant, or a write overlaps the function's instructions
  (`Alias`). These checks compare physical byte ranges.

The arithmetic is the guest's own operation sequence on the emulators,
so there is no fast-path range and no precision or rounding decline.

## Evidence

- `filter/tests.rs` (engine, no image): hand-worked calls down every
  path - snap and the clamped snap, the acceleration bound with either
  sign, the speed bound, the wrap before the reach test and the wrap of
  the new position - with the stack, register, flag and x87 shape each
  leaves; every decline leaving RAM and the CPU untouched; the switch;
  `step` against `run`.
- `first_person_filter_body` compares the body with lifted `166d75`
  over eight seeded seat fixtures. It requires all 40 return sites and
  compares each transfer, the full CPU and RAM. Cases include two live
  weapon slots, motion transitions and the idle timer's RNG path.
- `first_person_filter_kernel` runs twelve fixtures with `11eed0`
  inline on both sides. `first_person_filter_update` exercises the
  individual port with kernel fusion disabled.

## Not verified

- Run parity: the sim-budget match against the lifted build, identical
  `state_hashes_v1` and an empty `h2 sim diff`, with
  a mapped local seat. The decline rate on real seats is not measured.

## Related

- [sim-rewrite.md](sim-rewrite.md), section 5, phase 1c.
- [palette.md](palette.md): the first-person pose port's palette step.
- [sim-budget.md](../perf/sim-budget.md): `166d75` and `16760c` on the
  tick.
