# Object update: entries, conventions and liveness

Status: Partly built. Slices 1, 3, 4, 5, 6a, 6b, 6c, 7a, 7b, 8, 9, 10 and 11 have bodies in the body table (table below); the other slices of [sim-rewrite-objects.md](sim-rewrite-objects.md) add the rest.
Code: `crates/rewrites/sim-objects` (kernels and typed views, no `runtime` dependency: `src/wake.rs` the wake leaves, `src/events.rs` the tick's events, `src/damage.rs` the damage and kill event reads, `src/bounds.rs` the bounds kernel, `src/functions/curve.rs` the tag-function curve evaluator, `src/phase.rs` the walks' working set), `crates/rewrites/engine/src/sim_objects` (the guest boundary: `BODIES`, `drive`, `supported`, retail frame helpers; `walks.rs`, `object.rs`, `wake.rs` the slice 1 bodies; `functions/` the slice 3 bodies, `functions/parity.rs` the curve evaluator's capture arm; `lights/` the slice 5 bodies; `damage.rs` the slice 8 body and the event recorders; `item.rs`, `scenery.rs`, `device.rs`, `machine.rs`, `bloc.rs`, `creature.rs`, `projectile.rs`, `projectile_step.rs` the slice 11 bodies over `sse.rs` (scalar SSE on the guest registers) and `alu.rs` (shift and 16-bit flag idioms); `weapon/` the slice 9 body, one file per state group; `vehicle.rs` the slice 10 bodies; `biped/` the slice 7a bodies, `biped/post/` the slice 7b bodies; `post.rs`, `pose.rs`, `bounds.rs`, `transform.rs` the slice 4 bodies over `xform.rs` (the node-matrix rows); `unit.rs` the slice 6a bodies, with `sim_physics::unit_aim::step` for `c7840`; `unit_state/` the slice 6b bodies; `unit_rest.rs` and `unit_anim.rs` the slice 6c bodies; `native_walk.rs` the walk table and its counters), `crates/rewrites/sim-bridge/src/sim_objects.rs` (installer). Tests: `crates/rewrites/engine/src/sim_objects/native_walk/tests.rs` (the walk table, its float-free bodies and its counters), `crates/rewrites/sim-objects/src/phase.rs` (the working set), `crates/rewrites/engine/src/sim_objects/tests.rs` (the dispatcher, guarding, finalize and a leaf on a synthetic pool), `crates/rewrites/engine/src/sim_objects/functions/tests.rs` (the slice 3 helpers and the import dispatcher on synthetic memory), `crates/rewrites/engine/src/sim_objects/functions/curve_tests.rs` (the curve kernel against the `function_eval` oracle, and the arm's counters), `crates/rewrites/engine/src/sim_objects/damage/tests.rs` (the damage update's frame and the recorders), `crates/rewrites/engine/src/sim_objects/unit/tests.rs` (the slice 6a frames, callouts and stores), `crates/rewrites/engine/src/sim_objects/biped/tests.rs` (the slice 7a leaves, and the biped update's frame, callouts and stores), `crates/rewrites/engine/src/sim_objects/biped/post/tests.rs` (the slice 7b leaves, the crouch step, and the post-update's early returns, physics input path and contact counter), `crates/rewrites/engine/src/sim_objects/unit_state/tests.rs` (the slice 6b frames, callouts, stores and the divide error), `crates/rewrites/engine/src/sim_objects/lights/tests.rs` (the slice 5 bodies' frames, sites and stores on a synthetic light array), `crates/rewrites/sim-bridge/src/sim_objects.rs` (the installer, the guard, the event observers, and resumes at a body's sites on the unit shell), `crates/rewrites/engine/tests/sim_objects_differential.rs` (fused transcripts and mixed frames against the lifted original for every slice 1, slice 3, slice 4, slice 5, slice 6a, slice 6b, slice 6c, slice 7a, slice 7b, slice 8, slice 9, slice 10 and slice 11 root, comparing the integer registers, flags, memory, MXCSR, XMM registers and x87 state at every callout and return (`FLOAT_OPT_OUT` lists the roots that do not, each with its reason), feature `differential`), `crates/rewrites/sim-objects/src/wake/tests.rs` (the wake kernels), `crates/rewrites/sim-objects/src/damage/tests.rs` (the event reads), `crates/rewrites/sim-objects/src/bounds/tests.rs` (the bounds kernel), `crates/rewrites/engine/src/sim_objects/bounds/tests.rs` (`bdef0` against that kernel), and the views in `crates/rewrites/sim-objects` and `crates/rewrites/sim-state`.
Native only, no switch.

The contract page for phase 3 of the simulation rewrite: the object walks,
the per-object pre- and post-update, the type dispatchers, function values,
change colours, the post-update shell and lights. The design, its decisions
and its slices are in [sim-rewrite-objects.md](sim-rewrite-objects.md); this
page records what each entry and callout takes and returns, what the
static reading resolved, and which registers each call site relies on. The
rule every body keeps is the callout contract in that page's section 5: at
every call out and every return, `esp`, the frame slots read later and all
eight integer registers equal retail's.


## Bodies by slice

| Slice | Bodies | Left as callouts |
|---|---|---|
| 1 | the three walks, the per-object pre- and post-update, the four type dispatchers, the wake leaves, the early-mover wake | |
| 3 | function values, the export evaluator, the import dispatcher, every type level's import handler, change colours | |
| 4 | the post-update shell (component transform, node matrices, bounds, attachment refresh, node-matrix notify), the transform setters and getters | |
| 5 | the light walk, the light record update and its phase step, the light-list link, unlink and expire | |
| 8 | the damage update `d5de0`, with the tick's `Damage` and `Kill` events | |
| 11 | the item, scenery, device, machine, crate, creature and projectile updates, the projectile step `f8eb0` | |
| 6a | the unit update's shell `c6b30`, the movement `c60c0`, the direction clamp `cba50`, the idle test `c85c0`, the aim update `c7840` | |
| 6b | control `c49b0`, state `c58f0`, powered seats `c5eb0`, control refresh `ce0c0` | |
| 6c | timers `c6740`, dialogue update `114c60`, team `c6990` with seat iterator `d0620` and `d0690`, active camouflage `c6810`, integrated light `c50a0`, the unit animation state `111650` | |
| 9 | the weapon update `fdfb0` | |
| 10 | the vehicle update `efde0` with its gate `f1320` and contact reset `f06c0` | the type physics and the rest of its tree |
| 7a | the biped update `dd360` with the carrier `baf40`, timer update `e2d80`, `1469f0`, action request `e68c0`, animation kind `dd300`, landing test `e4050`, fall update `e45e0`, animation state update `e4c50`, look update `de190` | the movement and animation steps, the seat updates |
| 7b | the biped post-physics update `dd990` with crouch step `dd7d0`, crouch height `dd8e0` and its forced test `e0dc0`, ground probe `e30c0` and its surface test `ddf60`, jump update `e4680`, physics input `dd0d0`, landing update `e4330`, contact update `e2fa0` | the AI hooks, the damage packet, the physics step |

The tag-function curve evaluator `13b390` is answered by the idiomatic kernel `sim-objects/src/functions/curve.rs`. What it does not carry (curve kinds 9 and 10, a float environment or operand off its exact path) the register-level port `engine::function_eval` answers; a fault or an exception the port does not model panics (`objects curve:`). Counted in `engine::sim_objects::functions::parity`.

## The walks in Rust

The walk table installs with every body (`engine::sim_objects::native_walk::TABLE`, installer `sim_bridge::sim_objects::install_walk`). No switch runs the guest's walks: a call outside the float environment (`bd090` or a type callback) is counted as `float_env` and the Rust still answers. The differential tests (`walk_table_roots`, `walk_table_types`) call the lifted walks directly as the reference.

| In Rust | The guest's |
|---|---|
| walks (`native_walk::POINTS`): the walks `b6f10`, `b7060` and finalize `b7150`, the per-object updates `bc470` and `bc820`, the node matrices `bd090`, the four type dispatchers, the wake leaves, the early-mover wake `109580`, the eight leaves `bd090` calls (below) | damage `d5de0`, change colours `be8e0`, the rest of the post-update shell (`bc5e0`, `bdef0`, `bef30`, `be1d0`), functions, lights; finalize's `fd910`, `b83b0` and `b8460` |
| type callbacks (`native_walk::TYPES`): the unit update `c6b30` (slot `+40` of `unit`, which bipeds and vehicles inherit) with every body of slices 6a, 6b and 6c, the aim update `c7840` among them; the biped update `dd360` (slot `+40`) and post-physics update `dd990` (slot `+44`) with every body of slices 7a and 7b; the weapon update `fdfb0` (slot `+40`, slice 9); the item level `10b4d0` (slot `+40` of `item`, run first by weapons, equipment and garbage, which have no other slot); the vehicle update `efde0` with `f1320` and `f06c0` (slice 10); the projectile update `f8de0` with its step `f8eb0`, the scenery update `10a2f0`, the device update `106790` (slot `+40` of `devi`, run first by machines, controls and light fixtures; controls and light fixtures have no other slot), the machine update `1152e0`, the crate update `11bce0` and the creature update `118700` (slot `+40`) and post-physics update `118b80` (slot `+44`) (slice 11); the vehicle and device pose slots `f0f50` and `107000` (slot `+6c`, "Slot pose callees") | the unit tree's own guest callees ([object-update-bodies-units.md](object-update-bodies-units.md)), the creature's AI calls, the machine's slot `+48` query `115a40` (no dispatcher calls it), the unit pose `112070`, the after-root slots (`112dc0`, `10a390`), the pose slots' own callees `f0760` and `1c6f60` |

Units, bipeds and weapons first because a multiplayer game is mostly
them: each player is a biped (a unit) with two weapons, and the dropped
and spawned weapons are on the map besides. Then what else a multiplayer
map holds: equipment, vehicles, projectiles and scenery, and finalize,
which runs both updates of every new object (a spawned weapon, a thrown
grenade). In the Waterworks attribution
([sim-10x.md](../perf/notes/sim-10x.md), "the Lockout budget model")
units are 6.66% of the thread, bipeds 2.74%, vehicles 2.85% and weapons
1.46%. A biped's update is its unit level `c6b30` then `dd360`, and a
vehicle's its unit level then `efde0`, all Rust.

Then the device family, which multiplayer maps place as doors, lifts and
switches and the campaign everywhere. A machine's update is its device
level `106790` then `1152e0`; a control's and a light fixture's is the
device level alone. Crates and creatures close the type table: every
slot `+40` and `+44` callback is then Rust.

- **The aim hand-off:** the animation subsystem implements `c7840`, so the installer takes `native_walk::WITHOUT_AIM` (the table without it, built from `TABLE`; `sim_bridge::sim_objects::walk_table`), as `sim_bridge::sim_objects::table` does for the full table. The movement `c60c0`'s call to `c7840` is then a callout, made in place, and its hook answers.

```
entry or site of a table body at pc
  float-free body (the walk bodies but bd090)  -> Rust, fused into each next table body
  bd090 or a type callback, float env holds   -> Rust, fused the same way
  bd090 or a type callback, float env fails   -> refused: float_env; the guest runs that call
                                                 (entry) or the rest of it (site), and the
                                                 caller's Rust frame resumes at its site
```

- **Answers:** the Rust runs every segment it is given. Nothing is predicted or compared; the disc differential harness is the proof, in `crates/rewrites/engine/tests/sim_objects_differential.rs`, at the default float environment and with flush to zero and denormals as zero: `walk_table_roots` (the five walk roots over the walk table, finalize over 24 cases with its deletion events counted) and `walk_table_types` (the roots `c6b30` over the slice 6a, 6b and 6c dressings, `c60c0` with and without `c7840`, `fdfb0`, `dd360`, `dd990`, `10b4d0`, `efde0`, `f8de0`, `f8eb0`, `10a2f0`, `cba50` and `111650` over their dressings, `106790` and `1152e0` over the slice 11 dressing and the device dressing (`dress_device`: power and position flags, groups, rates, delay, machine kind), `11bce0`, `118700` and `118b80` over the slice 11 dressing, and the two dispatchers over a biped, a vehicle, every type, and every type with its machines, controls and light fixtures device-dressed, with every body outside the table a callout); `walk_table_getters` and `node_matrix_getters` run each `bd090` leaf as a root over the dressed pool, against the lifted original's registers, flags and memory.
- **Float-free bodies:** the walks (finalize too), the updates, the dispatchers, the wake leaves and the `bd090` leaves make no float instruction, so `Table::float_free` marks them and `Table::supports_at` lets them run whatever MXCSR and the x87 hold. `bd090` and the type callbacks check `engine::sim_objects::supported`.
- **The `bd090` leaves** (`engine::sim_objects::getters`): `bd090` calls up to 15 guest functions per object. Eight are small leaves that call nothing and make no float instruction, so each is a table body and `bd090` and the guest's own callers reach it without a guest call. Each returns the registers, flags and memory retail does (`ebx` and `edi` that `bfed0` pushes and pops return unchanged); `bfed0` honours the direction flag as its `rep stosd` does.

| Entry | Point | In | Out |
|---|---|---|---|
| `b8bd0` | `OBJECT_NODE_MATRIX` | `eax` object, stack 0 node (i16); `ret 4` | `eax` the node's matrix in the render model's node block |
| `bf5a0` | `OBJECT_HAS_ANIMATION` | `eax` object | `al` 1 when body word `+12a` is not `ffff` |
| `bfc90` | `OBJECT_ATTACHED` | `ecx` object | `al` 1 when the object is not none and body `+b3` is not 0 |
| `bff10` | `NODE_MASK_EMPTY` | `ecx` mask | `al` 1 when all eight words are 0 |
| `bfe80` | `NODE_MASK_INVERT` | `eax` mask | words inverted, the last masked to its low byte |
| `bfed0` | `NODE_MASK_TRIM` | `eax` bit count, `esi` mask | words past the count cleared, the partial word masked |
| `bfe20` | `NODE_MASK_SET` | `eax` word, `cl` bit, `dl` set | the bit set or cleared |
| `b6780` | `ANIMATION_VALID` | `eax` record | `eax` 1 when dword `+68`, dword `+0` and word `+6` are all not none |

  Seven callees stay guest calls: `e1670` (calls `1ec640`, float), `1d3920` (component nodes, a large frame and a long body), `16d940` (the orientation seed, an `xmm` loop), `bdb60` (the animation, calls `13e530` and more), `1c6440` (calls `1daea0`), `a98b0` and `a9940` (the motion position and basis, each a large body). None is a leaf of the small kind.
- **Refusals:** `float_env` is the only one. It is per call: one object's `bd090`, one unit's `c6b30` or one weapon's `fdfb0` is the guest's, the walk, the update and the dispatcher around it stay Rust. A callee that changes the float environment and returns to a type callback's site refuses the rest of that call.
- **Shared points:** a type callback another feature hooks too is guarded (`Table::guard`, through `HookRef::when_shared`), so the dispatcher's call goes through the chain and that feature's hooks run around the body. The Halo 1 features' `before`s on the dialogue update `114c60` and the integrated light `c50a0` are such hooks, and so are the opt-in creature probe's (`game::actor_net::creature`) on `118700` and `118b80`, which skip a replica's update. The Assassins skull gate at `c683f`, inside `c6810`, is read by the body itself (`engine::skull_gate`).
- **Replaced:** the hooks at `b6f10`, `b7060`, `b7150`, `bc470` and `bc820` never reach their guest bodies, so they carry `replaces_guest` (`native_walk::REPLACES_GUEST`). Each resumes at its sites, so `Plan::replaced` keeps their bodies lifted ([lift-pruning.md](../lifter/lift-pruning.md#the-replaced-list)). The dispatchers and wake leaves are not marked, so their bodies stay lifted.
- **Events:** finalize appends an `ObjectDeleted` before each `b83b0` ("Slice 8 body"), so the walk table's installer empties the buffer at `GAME_TICK`'s entry (one extra chain beside the table's, `sim_objects::walk_chains`). The `Damage`, `Kill` and `ActorKilled` observers are always recorded.
- **Callouts:** a call from a table body to the guest is made in place (`sim_bridge::sim_objects::drive_in_place`, `ExecCtx::call_inline`) and the Rust carries on at the site; the runtime leaves it to the runner, by a named fallback, when the runner would have work ([sim-10x-callouts.md](../perf/sim-10x-callouts.md)).
- **Counters:** `native_walk::stats`, per body: segments the Rust ran, calls refused by reason, callouts back in place (`inline`) and callouts left to the runner by fallback (`left`). `sim.arms` reports one arm per body, `objects.walk.<entry>` (`answered` the segments, `declined` the refusals).

## Evidence labels

This page uses the sim-rewrite scheme: **C** confirmed (read from the
retail image with `h2 disasm`, at the address given, or encoded in a
differential-tested port), **L** likely, **O** open. Addresses are guest
virtual addresses; bare numbers in tables are hex. Names are ours.

## Code paths

- `engine::sim_objects::BODIES` lists every body: its point, its entry,
  its retail return sites. Lanes append one row block per slice.
- `engine::sim_objects::drive` is the one local dispatcher: a call or
  return between two enabled bodies stays in Rust; anything else returns a
  `TransferAbi` to the runtime.
- `sim_bridge::sim_objects` registers each body's entry and sites. A call outside
  the float environment is counted and the Rust still answers. A per-function port at a point a body
  owns (`sim_physics::PORTS`, `sim_native::ENTRIES`) stands down while the
  switch is on (`sim_bridge::sim_objects::owns`). The ports the slice 1
  bodies replaced (the two walks and the two per-object updates),
  change colours, the unit update's shell `c6b30` and the light record
  update `c0c80` that slice 5's body takes over, are in
  `sim_physics::ORACLES`: never installed, kept as test oracles. The
  unit aim body shares `sim_physics::unit_aim::step` with the animation
  subsystem. With both switches enabled, the object dispatcher hands
  calls to `c7840` to the animation hook. The individual aim port is
  available when both subsystems are off. The
  function value, import and base import ports
  (`sim_native::object`, `sim_native::object_base`) are not installed
  either; `tests/sim_native_differential.rs` keeps them as oracles. The
  node-matrix port `sim_physics::node_matrices` is the `bd090` body's
  code (`pose.rs` runs it), so the port
  `sim.port.pose_matrix` installs only where no body owns its point.
- A body whose point another feature hooks too is guarded
  (`Table::guard`, which each body's implementation registers with
  `HookRef::when_shared` and the plan installer calls through
  `hooks::install::guard_shared_bodies`): `drive` hands calls into it to the
  runtime, so the point's chain runs its hooks around the body. The Halo
  1 health model's `before` and `after` on `OBJECT_DAMAGE_UPDATE` make
  that body guarded whenever both are on; so do the Halo 1 search
  dialogue's `before` on `UNIT_DIALOGUE_UPDATE`, and the Halo 1
  flashlight's and scope's `before`s on `UNIT_LIGHT_UPDATE`.
- `sim_bridge::sim_objects` also installs the event observers: the buffer is
  emptied at `GAME_TICK`'s entry. `DAMAGE_REPORT` appends `Damage` and
  appends `ActorKilled` when the report depletes an actor unit's body.
  `PLAYER_KILLED` appends `Kill` ("Slice 8 body").

## Entries

"ABI" is the convention at entry and exit: argument registers, stack
arguments (`stack n` is `[esp + 4 + 4n]` at entry), the result, and the
bytes `ret` pops. All are **C**, read at the entry address and its `ret`
instructions, and cross-checked by the liveness scan below, unless marked.
A register a function leaves unwritten is still part of the contract
(rule 2): the "Reaches `ret` with unwritten" column of the liveness table
lists them.

### Slice 1: walks and dispatchers

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `b6f10` | `OBJECTS_PRE_PHYSICS` | no arguments; `ret` | segment |
| `b7060` | `OBJECTS_POST_PHYSICS` | no arguments; `ret` | segment |
| `b7150` | `OBJECTS_FINALIZE` | no arguments; restores its saved registers, then `jmp bf380` | segment |
| `bc470` | `OBJECT_PRE_UPDATE` | `(object: stack 0) -> al`; `ret 4` | segment |
| `bc820` | `OBJECT_POST_UPDATE` | `(object: stack 0)`; `ret 4` | segment |
| `108c60` | `OBJECT_TYPE_UPDATE` | `(object: stack 0) -> al`, the OR of every slot result; `ret 4`. Slot `+40`, called `(object)` stdcall | segment (indirect callout) |
| `108cd0` | `OBJECT_TYPE_POST_UPDATE` | `(object: edi)`; `ret`. Slot `+44`, called `(object)`. `eax` at return is the address of the last ancestry entry, not a result | segment (indirect callout) |
| `109050` | `OBJECT_TYPE_POSE` | `(object: ebx, stack 0, stack 1, stack 2)`; `ret 0c`. Slot `+6c`, called `(object, s0, s1, s2)` | segment (indirect callout) |
| `1090d0` | `OBJECT_TYPE_AFTER_ROOT` | `(object: ebx, matrix: stack 0)`; `ret 4`. Slot `+70`, called `(object, matrix)` | segment (indirect callout) |
| `b7360` | `OBJECT_WAKE` | `(object: eax)`; `ret` | leaf |
| `baf80` | `OBJECT_ROOT` | `(object: ecx) -> eax` root ancestor (none in, none out); `ret`; clobbers `ecx`, `edx` | leaf |
| `bba20` | `OBJECT_WAKE_FOR_POST` | `(object: eax)`; `ret` | leaf |
| `bc7b0` | `OBJECT_WAKE_CHILDREN` | `(object: stack 0)`; `ret 4` | leaf |
| `109580` | `OBJECT_EARLY_MOVER_WAKE` | `(mover: stack 0)`; `ret 4`. A `1004`-byte frame made through the stack probe `320560` (`eax` = size) | segment until phase 2's `bb050` is callable from Rust |

How the slice 1 bodies keep the contract is in "Slice 1 bodies" below.

### Slice 3: functions and colours

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `bab40` | `OBJECT_FUNCTION_VALUE` | `(object: stack 0, name: stack 1, value: stack 2) -> al`; `ret 0c` | segment; the Rust evaluator `sim-objects/src/functions/evaluator.rs` answers at the entry (`functions/arm.rs`): the answer stores the value, sets `al`, `ecx`'s top byte (the frame's result byte) and `xmm0`'s low lane where retail leaves the value there, pops the frame, flags undefined; `eax` above `al`, `edx`, the other `xmm` state and the x87 status are untouched |
| `ba8c0` | `OBJECT_FUNCTION_EVALUATE` | `(record: eax, object: stack 0, value out: stack 1) -> al`; `ret 8`; stores the `f32` at `[stack 1]` on every path except a scripted override | segment, then leaf |
| `108d90` | `OBJECT_FUNCTION_IMPORT` | `(object: stack 0, name: stack 1, value: ebx, active: edi) -> al`; `ret 8`. Stores 0 to `[ebx]` and `[edi]` first, then calls slot `+4c` per ancestry level as `(object, name, value, active)` until one returns non-zero. Clobbers `xmm0` | segment (indirect callout) |
| `bcc20` | `BASE_OBJECT_IMPORT` | `(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al`; `ret 10` | segment, then leaf |
| `c6b90`, `de3b0`, `f6d10`, `ff5f0`, `fbe70`, `106d70`, `11bd60` | `UNIT_`, `BIPED_`, `VEHICLE_`, `WEAPON_`, `PROJECTILE_`, `DEVICE_`, `MACHINE_FUNCTION_IMPORT` | the slot `+4c` shape: `(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al`; `ret 10`; no register arguments | segment or leaf |
| `d1210` | `UNIT_IMPORT_FRACTION` | `(unit: eax) -> xmm0`; `ret`. Called by `c6b90` and `f6d10` | leaf |
| `d1410` | `UNIT_IMPORT_ANIMATION` | `(unit: eax) -> xmm0`; `ret`. Calls `1c6e30` | segment |
| `be8e0` | `OBJECT_CHANGE_COLORS` | `(object: stack 0) -> al`; `ret 4` | segment, then leaf |
| `13bb90` | none yet | `(function: eax, stack 0, stack 1)`, input in `xmm0` (clamped), result in `xmm0`; `ret 8`. Calls the evaluator `13b390` | callout |
| `13b390` | `FUNCTION_EVALUATE` | `(reference: edi, input: stack 0, range: stack 1) -> xmm0`; `ret 8`. The curve evaluator under every function value, light phase and screen effect ([engine-map.md](engine-map.md), `function_evaluate`) | the kernel `functions/curve.rs` answers (`xmm0`, the popped frame; `eax`, `ecx`, `edx`, `edi` untouched, flags undefined; `functions/parity.rs` counts); curve kinds 9 and 10 and off-path floats go to the register-level port `function_eval` |

### Slice 4: post-update shell

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `bc5e0` | `OBJECT_COMPONENT_TRANSFORM` | `(object: stack 0)`; `ret 4` | segment |
| `bd090` | `OBJECT_NODE_MATRICES` | `(object: stack 0)`; `ret 4`. Calls `1090d0` after the root node matrix (`bd507`) | segment; shared with phase 1b |
| `bdef0` | `OBJECT_BOUNDS` | `(object: stack 0) -> al`; `ret 4` | segment |
| `bef30` | `OBJECT_ATTACHMENTS` | `(object: stack 0, lights: stack 1, lists: stack 2, recurse: stack 3, keep: stack 4)`; `ret 14`. `bc820` passes `(object, 1, 1, 0, 0)` | segment |
| `be1d0` | `OBJECT_NODES_NOTIFY` | `(object: eax)`; `ret`. Calls `b8c00`, then `109140` (below) | segment |
| `b7430` | `OBJECT_SET_TRANSFORM` | `(object, position, forward, up, connect, refresh, notify, value, keep: stack 0..8) -> al`; `ret 24` | segment |
| `b7740` | `OBJECT_SET_VELOCITY` | `(object: eax, linear: ecx, angular: edi, quiet: stack 0)`; `ret 4` | segment |
| `b9dd0` | `OBJECT_WORLD_POSITION` | `(object: eax, out: esi) -> eax` (`out`); `ret` | leaf |
| `b9fc0` | `OBJECT_WORLD_BASIS` | `(object: eax, forward: esi, up: edi)`; `ret` | leaf |
| `ba1d0` | `OBJECT_ROOT_VELOCITY` | `(object: eax, linear: ecx, angular: stack 0)`; `ret 4` | segment |
| `b8c00` | none yet | `(object: eax, count out: stack 0) -> eax` the node matrices (`body + [body+116]`); stores the node count (`[body+114]` ÷ `34`) at `[stack 0]`; `ret 4` | callout |
| `109140` | none yet | `(object: ebx, count: stack 0, matrices: stack 1)`; `ret 8`. Slot `+74`, called `(object, count, matrices)` | callout (indirect) |

How the slice 4 bodies keep the contract is in "Slice 4 bodies" below.

### Slice 5: lights

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `c1400` | `LIGHTS_UPDATE` | no arguments; `ret` | segment |
| `c0c80` | `LIGHT_RECORD_UPDATE` | `(light: stack 0)`; `ret 4` | segment |
| `c0b00` | `LIGHT_RECORD_PHASE` | `(colour: eax, block: ecx, light: stack 0, input: stack 1, brightness: stack 2, out: stack 3)`; `ret 10`. `c0c80` passes the light element's `+d4` colour triple in `eax` and the definition's `+c0` block in `ecx` | segment |
| `c2d00` | `LIGHT_LINK` | `(light: stack 0)`; `ret 4` | segment |
| `c3220` | `LIGHT_UNLINK` | `(light: eax)`, light array `[4e030c]`, stride `110`; `ret` | segment |
| `c3260` | `LIGHT_EXPIRE` | `(light: stack 0, notify: stack 1)`; `ret 8` | segment |

### Slice 8: damage

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `d5de0` | `OBJECT_DAMAGE_UPDATE` | `(object: stack 0) -> al`, whether anything changed; `ret 4` | segment |

How the body keeps the contract is in "Slice 8 body" below.

### Slice 11: small types

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `10b4d0` | `ITEM_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `item`; weapons, equipment and garbage inherit it | segment |
| `10a2f0` | `SCENERY_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `scen` | segment |
| `106790` | `DEVICE_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `devi`; machines, controls and light fixtures inherit it | segment |
| `1152e0` | `MACHINE_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `mach` | segment |
| `115a40` | `MACHINE_SLOT_48` | `(object: stack 0, bits: stack 1, out: stack 2) -> al`; `ret 0c`. Slot `+48` of `mach`; no dispatcher in slice 1 calls it | leaf |
| `f8de0` | `PROJECTILE_UPDATE` | `(object: stack 0) -> al`, always 1; `ret 4`. Slot `+40` of `proj` | segment |
| `11bce0` | `CRATE_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `bloc` | segment |
| `118700` | `CREATURE_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `crea` | segment |
| `118b80` | `CREATURE_POST_UPDATE` | `(object: stack 0)`; `ret 4`. Slot `+44` of `crea` | segment |
| `f8eb0` | `PROJECTILE_STEP` | `(object: stack 0, velocity: stack 1)`; `ret 8`. The projectile step; called by `f8de0` | segment |

### Slice 6a: unit shell and movement

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `c6b30` | `UNIT_UPDATE` | `(unit: stack 0) -> al`; `ret 4`. Slot `+40` of `unit`; bipeds and vehicles inherit it | segment |
| `c60c0` | `UNIT_MOVEMENT` | `(unit: stack 0) -> al`, whether the aim or look moved; `ret 4` | segment |
| `cba50` | `UNIT_CLAMP_DIRECTION` | `(unit: eax, direction: stack 0, look: stack 1) -> al`, whether it clamped; `ret 8` | segment |
| `c85c0` | `UNIT_IDLE_TEST` | `(unit: eax) -> al`; `ret`. Called from `c58f0` (slice 6b) | segment |
| `c7840` | `UNIT_AIM_UPDATE` | `eax`, `esi`, `edi` and five stack arguments; `ret 14` | segment |

### Slice 9: weapon

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `fdfb0` | `WEAPON_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `weap`; the item update runs before it | segment |

How the body keeps the contract is in "Slice 9 body" below.

Later slices add one block each here, in the same shape.

### Slice 10: vehicle

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `efde0` | `VEHICLE_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `vehi`. A `1984`-byte frame made through the stack probe `320560` (`eax` = size) | segment |
| `f1320` | `VEHICLE_UPDATE_GATE` | `(object: eax) -> al`; `ret` | leaf |
| `f06c0` | `VEHICLE_CONTACTS_RESET` | `(buffer: eax)`; `ret` | leaf |

How the bodies keep the contract is in "Slice 10 body" below.

### Slice 6b: unit state

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `c49b0` | `UNIT_UPDATE_CONTROL` | `(unit: stack 0) -> al`; `ret 4`. `c6b30`'s first call | segment |
| `c58f0` | `UNIT_UPDATE_STATE` | `(unit: stack 0) -> al`; `ret 4`. `c6b30`'s second call | segment |
| `c5eb0` | `UNIT_POWERED_SEATS` | `(unit: eax) -> al`; `ret`. `c6b30`'s fourth call. `ecx` returns with the result in its top byte | segment |
| `ce0c0` | `UNIT_CONTROL_REFRESH` | `(unit: edi)`; `ret`. Called from `c5eb0`; the Halo 1 drive model hooks it, so the body is guarded when that feature is on | segment |

How the bodies keep the contract is in "Slice 6b bodies" below.

### Slice 6c: the rest of the unit

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `c6740` | `UNIT_TIMERS` | `(unit: eax) -> al`, whether a timer was running; `ret` | segment |
| `114c60` | `UNIT_DIALOGUE_UPDATE` | `(unit: eax)`; `ret` | segment |
| `c6990` | `UNIT_TEAM_UPDATE` | `(unit: eax)`; `ret` | segment |
| `d0620` | `UNIT_SEATS_BEGIN` | `(unit: eax, iterator: ebx)`; `ret`. `ecx` returns 1 when it climbed | leaf |
| `d0690` | `UNIT_SEATS_NEXT` | `(iterator: edx) -> al`; `ret`. `ecx`'s top byte returns `al` | leaf |
| `c6810` | `UNIT_CAMO_UPDATE` | `(unit: eax)`; `ret` | segment |
| `c50a0` | `UNIT_LIGHT_UPDATE` | `(unit: eax)`; `ret` | segment |
| `111650` | `UNIT_ANIMATION_STATE` | `(unit: stack 0, record: stack 1) -> al`; `ret 8`. Called by the vehicle update among others, not by `c6b30` | segment |

How the bodies keep the contract is in "Slice 6c bodies" below.

### Slice 7a: biped

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `dd360` | `BIPED_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `bipd` | segment |
| `baf40` | `OBJECT_CARRIER` | `(object: eax) -> eax`; `ret` | leaf |
| `e2d80` | `BIPED_TIMER_UPDATE` | `(object: stack 0) -> al`; `ret 4` | segment |
| `1469f0` | `GAME_SECONDS_TO_TICKS` | `(seconds: stack 0) -> eax`, `ecx` the same; `ret 4` | leaf |
| `e68c0` | `BIPED_ACTION` | `(action: ecx, object: stack 0)`; `ret 4` | segment |
| `dd300` | `BIPED_ANIMATION_KIND` | `(name: ecx) -> al`; `ret` | leaf |
| `e4050` | `BIPED_AIRBORNE_TEST` | `(object: eax) -> eax`, 1 or 0; `ret` | leaf |
| `e45e0` | `BIPED_FALL_UPDATE` | `(object: esi)`; `ret` | segment |
| `e4c50` | `BIPED_ANIMATION_STATE_UPDATE` | `(object: edi)`; `ret` | segment |
| `de190` | `BIPED_LOOK_UPDATE` | `(object: eax)`; `ret` | segment |

How the bodies keep the contract is in "Slice 7a bodies" below.

### Slice 7b: biped post-physics

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `dd990` | `BIPED_POST_UPDATE` | `(object: stack 0)`; `ret 4`. Slot `+44` of `bipd` | segment |
| `dd7d0` | `BIPED_CROUCH_STEP` | `(object: eax) -> xmm0`; `ret` | segment |
| `dd8e0` | `BIPED_CROUCH_HEIGHT` | `(object: edi, out: stack 0) -> al`; `ret 4` | segment |
| `e0dc0` | `BIPED_CROUCH_FORCED` | `(object: eax) -> al`; `ret` | leaf |
| `e30c0` | `BIPED_GROUND_PROBE` | `(object: ebx)`; `ret` | segment |
| `ddf60` | `BIPED_GROUND_FLAG` | `(object: eax) -> al`; `ret` | leaf |
| `e4680` | `BIPED_JUMP_UPDATE` | `(object: edi) -> al`; `ret` | segment |
| `dd0d0` | `BIPED_PHYSICS_INPUT` | `(object: stack 0, record: stack 1, flag: stack 2)`; `ret c` | segment |
| `e4330` | `BIPED_LANDING_UPDATE` | `(object: eax)`, a speed in `xmm2`; `ret` | leaf |
| `e2fa0` | `BIPED_CONTACT_UPDATE` | `(object: eax, contact: ebx, stack 0 unread)`; `ret 4` | segment |

How the bodies keep the contract is in "Slice 7b bodies" below.

### Slot pose callees

Slot `+6c`, called by the pose dispatcher `109050` as `(object, s0, s1, s2)`. Bodies in `sim_objects::slot_pose`.

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `f0f50` | `VEHICLE_POSE` | `(object, s0, s1, s2: stack)`; `ret 10`. Slot `+6c` of `vehi`: frames `{s1, s2, s0}` and calls `f0760(object, f06e0, &frame)` (`ret c`), which stays guest | segment |
| `107000` | `DEVICE_POSE` | `(object, s0, s1, s2: stack)`; `ret 10`. Slot `+6c` of `devi`; machines, controls and light fixtures inherit it. With a model (definition `+38`), its graph (model `+4`) and an animation block (`+12a`), stores the graph's definition over `object` and calls `1c6f60(1.0, s0, s1, s2)` (`ebx` the channel, `ret 10`) for each live channel at body `+18c` and `+1ac` | segment |

### Callouts that stay guest in this phase

| Callout | Point | ABI |
|---|---|---|
| `b7b40` object_new | none yet | `(stack 0)`; `ret 4` |
| `108a90` new dispatch | none yet | `(object: ebx, stack 0, stack 1) -> al`, the AND of the slot results, stopping at the first zero; `ret 8`. Slot `+2c`, called `(object, s0, s1)`. Called from `b7b40` at `b8166` |
| `b83b0`, `b8460` delete | none yet | `(object: stack 0, stack 1)`; `ret 8`. `b83b0` calls `OBJECT_DELETE` `b8540`, then eight function pointers from `468664`, each `(object)` |
| `b8540` | `OBJECT_DELETE` | `(object: stack 0)`; `ret 4` |
| `fd910` | none yet | no arguments; `ret` |
| `bf380` | none yet | no arguments; `ret` |
| `109660` | none yet | `(index: ecx, mover: eax)`; `ret` |
| `a9570` | none yet | `(object: eax)`; `ret` |
| `1cd4e0` | none yet | `(animation block: esi) -> al`; `ret` |
| `1ced00` | none yet | `(component record: edi)`; `ret` |
| `1d3920`, `1d44f0` | none yet | three stack arguments; `ret 0c` |
| `bb050` | `OBJECT_SPATIAL_VISIT` | seven stack arguments; `ret 1c` |
| `b87b0` | none yet | `(object: stack 0)`; `ret 4` |
| `b8600` | none yet | `(object: stack 0, stack 1)`; `ret 8` |
| `baeb0` | none yet | `(iterator: stack 0) -> eax` the next body or 0; `ret 4` |
| `fd980` | none yet | `(weapon: stack 0)`; `ret 4` |
| `320560` | none yet | stack probe: `(size: eax)` |

## Bodies

| Page | Holds |
|---|---|
| [object-update-bodies-core.md](object-update-bodies-core.md) | Slice 0 resolutions and the bodies of slices 1, 3 and 4: the walks and dispatchers, function values and change colours, and the post-update shell. |
| [object-update-bodies-types.md](object-update-bodies-types.md) | The bodies of slices 8, 11, 5 and 6b: the damage update, the item, scenery, device, machine, crate, creature and projectile updates, and the lights. |
| [object-update-bodies-units.md](object-update-bodies-units.md) | The bodies of slices 6a, 9, 10, 6c, 7a and 7b: the unit update, the weapon, the vehicle and the biped. |
| [object-update-liveness.md](object-update-liveness.md) | The register liveness scan over every entry and call site of the object update bodies. |

## Resolved in slice 0

In [object-update-bodies-core.md](object-update-bodies-core.md).

- **Who reads it, and when.** Early-mover record readers: in [object-update-bodies-core.md](object-update-bodies-core.md).
- **Node matrices `bd090`.** In [object-update-bodies-core.md](object-update-bodies-core.md).
- **Phase `c0b00`.** In [object-update-bodies-types.md](object-update-bodies-types.md).

## Slice 1 bodies

In [object-update-bodies-core.md](object-update-bodies-core.md).

## Slice 3 bodies

In [object-update-bodies-core.md](object-update-bodies-core.md).

## Slice 4 bodies

In [object-update-bodies-core.md](object-update-bodies-core.md).

## Slice 8 body

In [object-update-bodies-types.md](object-update-bodies-types.md).

## Slice 11 bodies

In [object-update-bodies-types.md](object-update-bodies-types.md).

## Slice 5 bodies

In [object-update-bodies-types.md](object-update-bodies-types.md).

## Slice 6a bodies

In [object-update-bodies-units.md](object-update-bodies-units.md).

## Slice 6b bodies

In [object-update-bodies-types.md](object-update-bodies-types.md).

## Slice 9 body

In [object-update-bodies-units.md](object-update-bodies-units.md).

## Slice 10 body

In [object-update-bodies-units.md](object-update-bodies-units.md).

## Slice 6c bodies

In [object-update-bodies-units.md](object-update-bodies-units.md).

## Slice 7a bodies

In [object-update-bodies-units.md](object-update-bodies-units.md).

## Slice 7b bodies

In [object-update-bodies-units.md](object-update-bodies-units.md).

## Register liveness

In [object-update-liveness.md](object-update-liveness.md).

## Not verified

- That the `Damage` and `Kill` events equal the Live telemetry's hits and
  kills over a recorded match. Both read the same two entries; no match
  has been compared.
- Nodes posed per tick on Waterworks and in co-op campaign (slice 12,
  step 0: a count at `bd090` through the agent API).
- That no other DEFER setter exists outside the `or` forms scanned (a flags
  byte copied whole from another header, or a computed store, would not
  match). A differential run of the walks with DEFER watched would show it.
- The purpose of the definition flags bit 6 and of body `+d8` bit 0 (L),
  and whether the replicated transform `a9570` applies ever runs outside
  distributed modes.
- Whether the type callbacks behind slots `+70`, `+74`, `+78` and `+7c`
  draw from the RNG (not followed here; slices 4 and 6 read them).
- Liveness through the indirect jumps listed above, and of `xmm0` and x87
  state at every site.
- Whether charging Rust bodies to the logical clock changes thread
  interleaving in a film (film runs compared with `impl.sim.arena-hash`).

## Related

- [sim-rewrite-objects.md](sim-rewrite-objects.md): the design, decisions
  and slices this page serves.
- [sim-rewrite.md](sim-rewrite.md): the plan.
- [collision-queries.md](collision-queries.md): the phase 2 contract page
  (`bb050`, `b8600`, `b87b0` move there).
- [anim-sampling.md](anim-sampling.md): phase 1, which shares `bd090`.
- [live-gamestate.md](../live/live-gamestate.md): unit and weapon offsets.
- [bots.md](../game/bots.md): the network mode byte.
- [README.md](README.md): the engine contract format and fault behaviour.
