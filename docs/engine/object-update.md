# Object update: entries, conventions and liveness

Status: Partly built. Behind the switch, bodies cover the object walks, the per-object pre- and post-update, the type dispatchers, function values and change colours, the post-update shell, lights, damage, the small types, and the unit, weapon, vehicle and biped updates (slices 1 to 11). The tag-function curve evaluator `13b390` runs as an arm: the kernel `sim-objects/src/functions/curve.rs` predicts each call and the guest's answer is compared, until 4096 matches take it over. Type physics, the AI hooks, the damage packet and the rest of the vehicle and biped trees stay guest callouts. Slices are in [sim-rewrite-objects.md](sim-rewrite-objects.md).
Code: `crates/rewrites/sim-objects` (kernels and typed views, no `runtime` dependency), `crates/rewrites/engine/src/sim_objects` (the guest boundary: `BODIES`, `drive`, `supported`; one file or directory per slice), `crates/rewrites/sim-bridge/src/sim_objects.rs` (installer). Tests: `crates/rewrites/engine/src/sim_objects/**/tests.rs`, `crates/rewrites/sim-objects/src/*/tests.rs`, and `crates/rewrites/engine/tests/sim_objects_differential.rs` (feature `differential`: fused transcripts and mixed frames against the lifted original for every slice root, comparing registers, flags, memory, MXCSR, XMM and x87 at every callout and return; `FLOAT_OPT_OUT` lists the exceptions). Per-file lists: see the [detail note](notes/2026-11-26-object-update-detail.md), "Status and code line".
Switch: `impl.sim.native.objects` (default off), listed in [switches.md](../runtime/switches.md).

The contract page for phase 3 of the simulation rewrite: what each entry and
callout takes and returns, what the static reading resolved, and which
registers each call site relies on. The design is in
[sim-rewrite-objects.md](sim-rewrite-objects.md). The rule every body keeps is
the callout contract in that page's section 5: at every call out and every
return, `esp`, the frame slots read later and all eight integer registers
equal retail's.

## Evidence labels

**C** confirmed (read from the retail image with `h2 disasm`, or encoded in a
differential-tested port), **L** likely, **O** open. Addresses are guest
virtual addresses; bare numbers in tables are hex. Names are ours.

## Code paths

- `engine::sim_objects::BODIES` lists every body: its point, entry and retail
  return sites.
- `engine::sim_objects::drive` is the one local dispatcher: a call or return
  between two enabled bodies stays in Rust; anything else returns a
  `TransferAbi` to the runtime.
- `sim_bridge::sim_objects` registers each body under
  `impl.sim.native.objects`. Where the float environment check fails, the
  original runs from the same frame. Per-function ports at a point a body
  owns (`sim_physics::PORTS`, `sim_native::ENTRIES`) stand down while the
  switch is on (`sim_bridge::sim_objects::owns`); the replaced ports live in
  `sim_physics::ORACLES` as test oracles. `c7840` shares
  `sim_physics::unit_aim::step` with the animation subsystem, which wins when
  both switches are on. Detail: see the [detail note](notes/2026-11-26-object-update-detail.md), "Code paths".
- A body whose point another feature hooks is guarded (`Table::guard`,
  registered with `HookRef::when_shared`, installed through
  `hooks::install::guard_shared_bodies`): `drive` hands calls into it to the
  runtime so the point's hook chain runs around it. Halo 1 health
  (`OBJECT_DAMAGE_UPDATE`), search dialogue (`UNIT_DIALOGUE_UPDATE`) and
  flashlight and scope (`UNIT_LIGHT_UPDATE`) guard their bodies.
- `sim_bridge::sim_objects` installs the event observers: the buffer empties at
  `GAME_TICK`'s entry; `DAMAGE_REPORT` appends `Damage` (and `ActorKilled`);
  `PLAYER_KILLED` appends `Kill` ("Slice 8 body").

## Entries

"ABI" is the convention at entry and exit: argument registers, stack
arguments (`stack n` is `[esp + 4 + 4n]` at entry), the result and the bytes
`ret` pops. All are **C** unless marked. A register a function leaves
unwritten is part of the contract (rule 2). The full ABI column of every table
below: see the [detail note](notes/2026-11-26-object-update-detail.md), "Entries".

### Slice 1: walks and dispatchers

| Entry | Point | Kind |
|---|---|---|
| `b6f10`, `b7060`, `b7150` | `OBJECTS_PRE_PHYSICS`, `OBJECTS_POST_PHYSICS`, `OBJECTS_FINALIZE` (`jmp bf380`) | segment |
| `bc470`, `bc820` | `OBJECT_PRE_UPDATE`, `OBJECT_POST_UPDATE` | segment |
| `108c60`, `108cd0`, `109050`, `1090d0` | `OBJECT_TYPE_UPDATE`, `_POST_UPDATE`, `_POSE`, `_AFTER_ROOT` (slots `+40`, `+44`, `+6c`, `+70`) | segment, indirect callout |
| `b7360`, `baf80`, `bba20`, `bc7b0` | `OBJECT_WAKE`, `_ROOT`, `_WAKE_FOR_POST`, `_WAKE_CHILDREN` | leaf |
| `109580` | `OBJECT_EARLY_MOVER_WAKE` (`1004`-byte frame via probe `320560`) | segment until `bb050` is callable from Rust |

How the slice 1 bodies keep the contract is in "Slice 1 bodies" below.

### Slice 3: functions and colours

| Entry | Point | Kind |
|---|---|---|
| `bab40`, `ba8c0` | `OBJECT_FUNCTION_VALUE`, `OBJECT_FUNCTION_EVALUATE` | segment, then leaf |
| `108d90` | `OBJECT_FUNCTION_IMPORT` (slot `+4c` per ancestry level) | segment, indirect callout |
| `bcc20` | `BASE_OBJECT_IMPORT` | segment, then leaf |
| `c6b90`, `de3b0`, `f6d10`, `ff5f0`, `fbe70`, `106d70`, `11bd60` | unit, biped, vehicle, weapon, projectile, device, machine `_FUNCTION_IMPORT` | segment or leaf |
| `d1210`, `d1410` | `UNIT_IMPORT_FRACTION` (leaf), `UNIT_IMPORT_ANIMATION` (segment) | |
| `be8e0` | `OBJECT_CHANGE_COLORS` | segment, then leaf |
| `13bb90` | tag-function lookup; calls `13b390` | callout |
| `13b390` | `FUNCTION_EVALUATE`, the curve evaluator ([engine-map.md](engine-map.md), `function_evaluate`) | arm (`functions/parity.rs`): kernel predicts, guest compared until 4096 matches, then the kernel answers and every 64th answerable call is compared; curve kinds 9 and 10 decline |

### Slice 4: post-update shell

| Entry | Point | Kind |
|---|---|---|
| `bc5e0`, `bd090`, `bdef0`, `bef30`, `be1d0` | `OBJECT_COMPONENT_TRANSFORM`, `_NODE_MATRICES` (shared with phase 1b), `_BOUNDS`, `_ATTACHMENTS`, `_NODES_NOTIFY` | segment |
| `b7430`, `b7740`, `ba1d0` | `OBJECT_SET_TRANSFORM`, `_SET_VELOCITY`, `_ROOT_VELOCITY` | segment |
| `b9dd0`, `b9fc0` | `OBJECT_WORLD_POSITION`, `_WORLD_BASIS` | leaf |
| `b8c00`, `109140` | node matrices and count; slot `+74` dispatcher | callout |

How the slice 4 bodies keep the contract is in "Slice 4 bodies" below.

### Slice 5: lights

| Entry | Point | Kind |
|---|---|---|
| `c1400`, `c0c80`, `c0b00`, `c2d00`, `c3220`, `c3260` | `LIGHTS_UPDATE`, `LIGHT_RECORD_UPDATE`, `_RECORD_PHASE`, `LIGHT_LINK`, `_UNLINK`, `_EXPIRE` | segment |

### Slice 8: damage

| Entry | Point | Kind |
|---|---|---|
| `d5de0` | `OBJECT_DAMAGE_UPDATE`, `(object: stack 0) -> al`, whether anything changed | segment |

How the body keeps the contract is in "Slice 8 body" below.

### Slice 11: small types

| Entry | Point | Kind |
|---|---|---|
| `10b4d0`, `10a2f0`, `106790`, `1152e0` | `ITEM_`, `SCENERY_`, `DEVICE_`, `MACHINE_UPDATE` (slot `+40`; children inherit) | segment |
| `115a40` | `MACHINE_SLOT_48` | leaf |
| `f8de0`, `f8eb0` | `PROJECTILE_UPDATE`, `PROJECTILE_STEP` | segment |
| `11bce0`, `118700`, `118b80` | `CRATE_UPDATE`, `CREATURE_UPDATE`, `CREATURE_POST_UPDATE` | segment |

### Slice 6a: unit shell and movement

| Entry | Point | Kind |
|---|---|---|
| `c6b30`, `c60c0`, `cba50`, `c85c0`, `c7840` | `UNIT_UPDATE`, `_MOVEMENT`, `_CLAMP_DIRECTION`, `_IDLE_TEST`, `_AIM_UPDATE` | segment |

### Slice 9: weapon

| Entry | Point | Kind |
|---|---|---|
| `fdfb0` | `WEAPON_UPDATE` (slot `+40` of `weap`; the item update runs first) | segment |

How the body keeps the contract is in "Slice 9 body" below.

Later slices add one block each here, in the same shape.

### Slice 10: vehicle

| Entry | Point | Kind |
|---|---|---|
| `efde0` | `VEHICLE_UPDATE` (`1984`-byte frame via probe `320560`) | segment |
| `f1320`, `f06c0` | `VEHICLE_UPDATE_GATE`, `VEHICLE_CONTACTS_RESET` | leaf |

How the bodies keep the contract is in "Slice 10 body" below.

### Slice 6b: unit state

| Entry | Point | Kind |
|---|---|---|
| `c49b0`, `c58f0`, `c5eb0` | `UNIT_UPDATE_CONTROL`, `_UPDATE_STATE`, `UNIT_POWERED_SEATS` | segment |
| `ce0c0` | `UNIT_CONTROL_REFRESH` (guarded when the Halo 1 drive model is on) | segment |

How the bodies keep the contract is in "Slice 6b bodies" below.

### Slice 6c: the rest of the unit

| Entry | Point | Kind |
|---|---|---|
| `c6740`, `114c60`, `c6990`, `c6810`, `c50a0`, `111650` | `UNIT_TIMERS`, `_DIALOGUE_UPDATE`, `_TEAM_UPDATE`, `_CAMO_UPDATE`, `_LIGHT_UPDATE`, `_ANIMATION_STATE` | segment |
| `d0620`, `d0690` | `UNIT_SEATS_BEGIN`, `UNIT_SEATS_NEXT` | leaf |

How the bodies keep the contract is in "Slice 6c bodies" below.

### Slice 7a: biped

| Entry | Point | Kind |
|---|---|---|
| `dd360`, `e2d80`, `e68c0`, `e45e0`, `e4c50`, `de190` | `BIPED_UPDATE`, `_TIMER_UPDATE`, `_ACTION`, `_FALL_UPDATE`, `_ANIMATION_STATE_UPDATE`, `_LOOK_UPDATE` | segment |
| `baf40`, `1469f0`, `dd300`, `e4050` | `OBJECT_CARRIER`, `GAME_SECONDS_TO_TICKS`, `BIPED_ANIMATION_KIND`, `_AIRBORNE_TEST` | leaf |

How the bodies keep the contract is in "Slice 7a bodies" below.

### Slice 7b: biped post-physics

| Entry | Point | Kind |
|---|---|---|
| `dd990`, `dd7d0`, `dd8e0`, `e30c0`, `e4680`, `dd0d0`, `e2fa0` | `BIPED_POST_UPDATE`, `_CROUCH_STEP`, `_CROUCH_HEIGHT`, `_GROUND_PROBE`, `_JUMP_UPDATE`, `_PHYSICS_INPUT`, `_CONTACT_UPDATE` | segment |
| `e0dc0`, `ddf60`, `e4330` | `BIPED_CROUCH_FORCED`, `_GROUND_FLAG`, `_LANDING_UPDATE` | leaf |

How the bodies keep the contract is in "Slice 7b bodies" below.

### Callouts that stay guest in this phase

`b7b40` object_new, `108a90` new dispatch (slot `+2c`), `b83b0` and `b8460`
delete, `b8540` `OBJECT_DELETE`, `fd910`, `bf380`, `109660`, `a9570`,
`1cd4e0`, `1ced00`, `1d3920`, `1d44f0`, `bb050` `OBJECT_SPATIAL_VISIT`,
`b87b0`, `b8600`, `baeb0`, `fd980` and the stack probe `320560`. Each stays
guest until its owning phase; ABIs: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Entries".

## Resolved in slice 0

Each item of section 13 "Not resolved" of the design page. The evidence, with
addresses, is in the [detail note](notes/2026-11-26-object-update-detail.md),
"Resolved in slice 0".

**DEFER (`20`) setter.** C. Only the attach function `b93b0` sets it on the
child; `bc470` skips the object for the rest of the tick and finalize clears it.

**NEW (`08`) at creation.** C. `object_new` `b7b40` sets it at `b7c0e`; `109580`
sets it on woken weapons, equipment, garbage and projectiles.

**The list at `[5107f0]+1d00`.** C: early movers, appended by `109390` and
removed by `109400`; `109360` and `109380` reset it.

**What `109660` writes.** C. A per-mover record at `[5107f0] + index × e8`:
previous and current transform blocks, velocities and a delta transform.

**Who reads it, and when.** C for the call chains, L for the purpose. Riders
read the record in the same tick from the post-physics walk and in the next tick
from the pre-physics walk (readers `109a00`, `109e00`, `109fd0`).

**`bc470` step 3.** The byte `[[4e6948]+c]` is the network mode (1 local to 5
distributed server; [bots.md](../game/bots.md)); `a9570` applies a pending
replicated transform.

**`fd910`.** C. The deferred weapon fire: iterates awake weapons and calls
`fd980`.

**`bf380` (finalize tail).** C. Object memory collection through the table at
`440578`.

**`be1d0` → `b8c00`, `109140`.** C. Only `unit` sets slot `+74`.

**Register conventions.** C, in "Entries" above. `bc7b0` takes its object on
the stack; `108cd0` takes it in `edi`; `109050`, `1090d0`, `108a90` and
`109140` take it in `ebx`.

**The slot `1090d0` dispatches.** C. Slot `+70` of `unit` and `scen`: the
"after root" hook, called by `bd090` at `bd507`.

**RNG in function values.** C, for direct calls. No function in the closure
references `[4e7408]`, so function values and change colours are a pure kernel
of their inputs.

**Unit offsets.** C. Actor `+12c`, team `+138`, weapon index `+212`, weapons
`+218`; named in `crates/hooking/guest-layout/src/unit.rs` and
[live-gamestate.md](../live/live-gamestate.md), "AI bipeds, teams and vehicles".

**Not answerable statically.** The node count per tick (slice 12's step 0) and
the logical clock: a Rust port is charged nothing for its own guest accesses, so
the only preemption points inside a walk are the true callouts, which is
deterministic. Whether to charge bodies is a decision, not a reading.

## Slice 1 bodies

The walks and the typed object iterator share the bounded bitmap scan in
`sim_objects::walk::next_live`. Each is **C**, compared with the lifted original
by `tests/sim_objects_differential.rs`.

**Walks** (`engine::sim_objects::walks`). The pre- and post-physics bodies are
the per-function ports, returning each transfer to `drive`. Finalize `b7150`
runs pass A (clear DEFER and NEW, call `bc470`, then `bc820` when POST is set)
and pass B (`b83b0`, `b8460` for each DELETE object), then jumps to `bf380`.

**Events.** Before finalize's `b83b0` call the body appends `ObjectDeleted` to
the tick's buffer (`sim_objects::events`, read through `sim_state::events`).

**Per-object updates** (`object.rs`). `bc470` and `bc820` are the ports.

**Type dispatchers** (`object.rs`). Each reads the type from body `+aa`, the
descriptor from `[468630 + 4 × type]` and walks the ancestry at descriptor
`+84`, calling the level's slot when non-null. A type without a slot gets its
parent's: item update for weapons, equipment and garbage; device update for
machines, controls and light fixtures; unit update before biped and vehicle.
Register use per dispatcher: see the [detail note](notes/2026-11-26-object-update-detail.md), "Slice 1 bodies".

**Wake leaves** (`sim_objects::wake`, bodies in `engine::sim_objects::wake`).
Plain functions over guest memory; the header address is `(handle & ffff) × c`.
`bc7b0` keeps an explicit stack instead of recursing and reports a fault past
`2^16` levels. The leaves leave the stack below their entry `esp` as retail
does (`sim_objects::wake::Stack`). Exit registers and flags per leaf: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 1 bodies".

**Early-mover wake** (`wake.rs`). `109580` makes a `1004`-byte frame through
`320560`, visits `bb050(0, 3f, location, centre, radius, buffer, 400)`, and per
result sets body `+c0` bit 2, stores the mover at `+b8` and calls `b7360`; for
types 2 to 5 it also calls `bba20` and sets NEW.

## Slice 3 bodies

Each is **C**: read at its address with `h2 disasm`, and compared with
the lifted original by `tests/sim_objects_differential.rs`. All are
segments in `engine::sim_objects::functions`. They leave the arithmetic
flags undefined at every call and return and keep `xmm0` to `xmm4` and the
x87 state as retail leaves them. Function names compared by `cmp` or `sub`
are read from those instructions' immediates in the loaded image.

`sim_objects::functions::ExportCursor` scans consecutive misses in one Rust
loop and resumes from the guest frame, so duplicate exports stay in
definition order even if a callback replaces the table.

| Body | Address | File |
|---|---|---|
| Function value | `bab40` | `functions/value.rs` |
| Export evaluator | `ba8c0` | `functions/` |
| Import dispatcher | `108d90` | `functions/` |
| Base import handler | `bcc20` | `base.rs` |
| Unit | `c6b90`, `d1210`, `d1410` | `unit.rs` |
| Biped | `de3b0` | `biped.rs` |
| Vehicle | `f6d10` | `vehicle.rs` |
| Weapon | `ff5f0` | `weapon.rs` |
| Projectile | `fbe70` | `projectile.rs` |
| Device | `106d70` | `device.rs` |
| Machine and crate | `11bd60` | `machine.rs` |
| Change colours | `be8e0` | `colors.rs` |

**Function value** `bab40`. Stores 0 through the value pointer first, runs
the scripted override `10aac0`, the built-in names, the import `108d90`,
every matching export through `ba8c0`, then the parent's value when body
flags bit 26 is set. The result byte is the top byte of the saved `ecx`
slot.

**Export evaluator** `ba8c0`. Computes one export: override, input import,
function `13bb90` (or game time for a time input), scale, threshold, gate,
clamped to `[0, 1]` or 0 when inactive.

**Import dispatcher** `108d90`. Calls slot `+4c` of each ancestry level as
`(object, name, ebx, edi)` until `al` is non-zero.

**Import handlers.** The slot `+4c` shape, `ret 10`. An unknown name
returns `al` 0 and stores nothing; an answered one stores the `f32` value
and the byte "value > 0". Each body's register use and the names it answers
are in the detail note.

**Change colours** `be8e0` (`colors.rs`): the per-function port, as a body.
Frame and registers as `sim_physics::colors`; `bab40` for each permutation
and scale function, `131c20` for each permutation.

**Callees.** Every guest callout and its `ret` size is read from the
loaded image; the list is in the detail note.

**Not yet a leaf.** The evaluator becomes a leaf kernel (section 5 of the
design) only when `13bb90`, `10aac0`, `131c20` and the type handlers'
callees are Rust, so every slice 3 body is a segment.

Frames, handler tables and callee lists: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 3 bodies".

## Slice 4 bodies

Each is **C**: read at its address with `h2 disasm`, and compared with the
lifted original by `tests/sim_objects_differential.rs` (each entry as a
root over a dressed pool, and `bc820` over the same pool). Each body
file's header records its frame and the order of its calls.

**Shared rules.** The bodies keep all eight integer registers, the `xmm`
registers and MXCSR as retail leaves them (scalar SSE through
`sim_objects::sse`). The arithmetic flags are reproduced at every return,
because `b7430`, `b7740`, `b9dd0`, `b9fc0` and `ba1d0` are called from
all over the image.

**Node-matrix rows** (`xform.rs`). `bdef0`, `b9dd0` and `b9fc0` inline one
node-matrix row sequence; `sim_objects::bounds` is the same arithmetic as
a pure kernel (exact or declining, through `sim_core::det::Fp`) for a
speculated pass.

**Bounds `bdef0`** (`bounds.rs`). Stores the first and second spheres, then
the registered sphere only when it stores (not in the partition, or moved).

**Node matrices `bd090`** (`pose.rs`). The port
`sim_physics::node_matrices` run as a body (design section 6). The
skeleton walk from node 1 is phase 1b's: `sim_anim::build_matrices`
through `engine::sim_anim::skeleton` (`impl.sim.native.anim.skeleton`),
with the port's per-node loop as the fallback when it declines
([skeleton-walk.md](skeleton-walk.md)).

**Attachment refresh `bef30`**, **node-matrix notify `be1d0`** and
**component transform `bc5e0`** are in `post.rs`; **setters and getters**
(`b7430`, `b7740`, `b9dd0`, `b9fc0`, `ba1d0`) are in `transform.rs`.

Per-body callees, frames and flag rules: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 4 bodies".

### Not verified for slice 4

Captures and run parity on Waterworks and a campaign level with
`impl.sim.native.objects` on and off are not done; the harness covers
synthetic pools, and compares `xmm` at callouts for every root except
`bd090` (`tests/sim_physics_differential.rs`). Callee `ret` sizes are read
from their first `ret`.

## Slice 8 body

**C**: read at `d5de0` with `h2 disasm`, and compared with the lifted
original by `tests/sim_objects_differential.rs` (root
`OBJECT_DAMAGE_UPDATE`, and inside `bc470`'s tree).

**Frame** (`engine::sim_objects::damage`). `sub esp, 70` and three pushes
(`edi` only once `d5b60` returns non-zero). The body is the retail code
block by block, keyed by retail address, so the lifted original can take
over at any of its 26 return sites and the reverse.

**Floating point.** The body keeps every XMM register as retail leaves it
at each call; the section decay rounds with `fld` / `fistp` under the
guest's x87 control word.

**Callees** are all callouts (`d5b60`, `d9f70`, `da110`, `dbc80`, `d6bc0`,
`1e4a10`, `d66d0`, `d6800`, `dbfb0`, `1588b0`, `14de90`, `13a6e8`,
`a7a30`, `1e9720`, `ba7f0`, `176780`, `138880`, `d8b50`).

**Events.** The function applies no damage itself: damage reaches an
object through the damage report `d9640` and a kill is credited at
`15ce70` (`PLAYER_KILLED`). `sim_bridge::sim_objects` records events with
observers at those two entries, the same places the Live telemetry's hooks
read:

- `Damage` (`sim_objects::damage::damage`, at `DAMAGE_REPORT`): the report
  (`guest_layout::DamageReport`), the victim's handle, type, controlling
  player and position, and the game tick.
- `Kill` (`sim_objects::damage::kill`, at `PLAYER_KILLED`): attacker and
  victim players, the damage code, each player's unit and position, and
  the tick.

The buffer (`sim_objects::events`, read through `sim_state::events` and the
agent API's `sim.events`) belongs to the game thread and is emptied at
`GAME_TICK`'s entry. Events are recorded only while
`impl.sim.native.objects` is on.

**The Halo 1 health model** ([health.md](../halo1/health.md)) hooks this
point with a `before` and an `after`. With both on, the installer guards
the body (see "Code paths"): `bc470` reaches it through the runtime, which
runs the `before`, the body through its shadow, then the `after`.

Frame slots and the callee `ret` sizes: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 8 body".

## Slice 11 bodies

Each is **C** for its control flow and accesses: read at its address with
`h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (the roots below, and the type
dispatchers and walks). Field names are ours and **L**. Each body file's
header records its frame and call order.

**Shared rules.** The bodies keep the `xmm` registers, MXCSR and x87 state
as retail leaves them, as well as the eight integer registers: float
arithmetic goes through `sim_objects::sse`, x87 sequences through
`x87::abi`. A body stores into its caller's argument slots where retail
does (`f8de0`, `118b80`, `115a40`). Flags are reproduced at every transfer
from the last instruction that sets them.

| Body | Address | What it does |
|---|---|---|
| Item | `10b4d0` | Early-mover follow, fall and damping, wake for post, garbage timer (`OBJECT_DELETE` `b8540` at 0 outside network mode 4), then `10b850(object)` |
| Scenery | `10a2f0` | Advances the animation block (`1cb710`); wakes for the post walk when it has more than one frame |
| Device, machine | `106790`, `1152e0`, `115a40` | Power and position toward targets; machine update for cycling machines and doors; `115a40` is a leaf |
| Crate | `11bce0` | `1d24a0(component record, 1.0)` with body `+c0` bit 12 and a physics component |
| Creature | `118700`, `118b80` | AI calls stay callouts in retail order; `118700` keeps `fld`/`fistp` for the timer at body `+204` |
| Projectile | `f8de0` | Displacement from `109a00`, the step `f8eb0`, then adds the displacement to body `+88` |
| Projectile step | `f8eb0` | `projectile_step.rs`: 1247 bytes, 39 calls, retail's basic blocks keyed by address |

The projectile step runs, in retail order: attachment, timers, then
sub-steps while the tick's time lasts (at most ten hits: speed, homing,
drag, gravity, collision, flyby, orientation, new transform), then
detonation `fc330` or deletion `b8540` by the state word `+130`.

Per-body walkthroughs and callee `ret` sizes: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 11 bodies".

### Not verified for slice 11

Campaign captures with `impl.sim.native.objects` off are not done. The
harness covers synthetic pools only, its stubs answer in a float register
only for `30bf0`, `1e9700` (`xmm0`) and `24590` (`st(0)`), and the stubs
for `f87f0` and `fc030` fill little of their output, so homing and hit
arithmetic run on what the stack held.

## Slice 5 bodies

`sim_objects::light::blend` interpolates the two radius endpoints (light
record `+ec`) and the two intensity endpoints (`+c4`) in the same order.
It returns both the sum and the weighted second endpoint, which remains
visible in the caller's XMM registers. The scalar SSE model handles inputs
outside the kernel's proved range and tracks exceptions when the guest
precision flag is clear.

Each is **C** for its control flow and accesses: read at its address with
`h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (roots `c1400`, `c0c80`, `c0b00`,
`c2d00`, `c3220` and `c3260`). Field names are ours and **L**. The module
header of `sim_objects/lights/mod.rs` names the element fields.

**Shared rules.** Integer registers, `esp` and the frame slots match retail
at every transfer. The arithmetic flags are undefined at every call and
return (no flag reader at any slice 5 entry or site). Scalar SSE goes
through `sim_objects::sse`; the colour scaling in `c0b00` runs on the
guest's x87 state through `x87::abi`. The light lists stay guest: `1cac60`
adds, `1cae40` removes, `16ba40` frees, `16bc00` iterates.

**Walk `c1400`.** Per live light, in index order: `c0c80(handle)`; a linked
light whose owner or marker parent has POST set is removed and relinked
(`c2d00`); a timed light past its lifetime expires (`c3260(handle, 1)`).
The array header, high-water mark and bitmap are re-read after each light.

**Record `c0c80`.** The per-function port of `LIGHT_RECORD_UPDATE` (the
oracle `oracle.light_record` in `sim_physics::ORACLES`) with its transfers
returned to `drive`. It links a light that turns on only while the object
globals' first byte `[[4de2f4]]` is set, and unlinks one that turns off.

**Phase `c0b00`.** The phase is game time times a rate, or with a
per-light offset `((handle × 1387) & 7fffffff) mod (ticks per second ×
3c)`; a zero tick rate is the guest's divide error at `c0b3b`. Then up to
three function blocks: brightness, colour and two `13bb90` values.

**Link `c2d00`, unlink `c3220`, expire `c3260`.** `c2d00` computes the world
position from the owner's node matrices or marker, finds the cluster and
adds the light, then sets flags bit 3. `c3220` removes an active light and
clears bit 3. `c3260` removes a linked light, updates the marker parent's
body flags `+4` bit 6 when `notify` is set, and frees the light.

**Callee `ret` sizes.** Read from each callee's first `ret`; listed in the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 5 bodies".

### Not verified for slice 5

Captures from campaign and multiplayer at the n-th call of `c0c80` and
`c2d00` (with `impl.sim.native.objects` off) are not done, and the harness
answers `16bc00` from a synthetic bitmap, so the iterator is not compared.

## Slice 6a bodies

On the look path of `cba50`, `sim_objects::unit::look_up` completes
`frame_up × aim` and evaluates `aim × left` in packed binary32 lanes. The
adapter reconstructs the six XMM outputs at the normalizer call. The scalar
SSE path handles exceptional operands, out-of-range intermediates and float
environments outside the packed path's proof.

Each is **C** for its control flow and accesses: read at its address with
`h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (the four roots below, and the type
dispatchers and walks, which reach `c6b30` through the unit types' `+40`
slot). Field names are ours and **L**. `unit.rs`'s header records each
body's frame.

**Transfers.** The eight integer registers, the `xmm` registers, MXCSR and
the x87 state are retail's at every call and return. Flags are left
undefined at every transfer of `c60c0`, `cba50` and `c85c0`
(`scripts/sim-liveness.py`); `c6b30` and `c7840` reproduce them. x87
sequences run through `x87::abi` on the guest's x87 state.

**Shell `c6b30`.** Calls `c49b0`, `c58f0`, `c60c0`, `c5eb0`, `c6740`,
`114c60`, `c6990`, `c6810` and `c50a0`; `al` is the OR of the first five
answers. Slice 6b and 6c bodies in it are fused. The port it comes from
(`sim_physics::objects::unit`) is an oracle.

**Movement `c60c0`.** Returns 0 when definition `+bc` bit 10 or body `+10a`
bit 2 is set; otherwise moves the aim (`+168`) and look (`+18c`) toward the
desired values, through `cba50` when both rates are zero, else `c7840` (in
the unit's frame) or `11f0d0` (world). It wakes the unit for the post walk
(`bba20`) when any component moved by more than `[45dbdc]`.

**Clamp `cba50`.** Returns 0 when the limits record's bit for the direction
(5, or 6 for the look) is clear; otherwise clamps yaw and pitch to the
record's ranges and writes the direction back when either clamped.

**Idle test `c85c0`.** 0 for a parent, a record-word bit 26 or 27, any of
seven `e6fe0(unit, n)` tests, or a held weapon answering `100880`; else 1.
The result is also stored in the top byte of the caller's `ecx`.

**Aim `c7840`.** `sim_physics::unit_aim::step` computes joint-space
direction and velocity with scalar SSE and x87 and returns a call or return
to its driver. The object update can fuse the step when it owns aim; with
the animation subsystem selected, it emits the call and
`sim_anim::unit_aim` plans and commits the segment. Both drivers use the
guest frame across all nine return sites.

### Not verified for slice 6a

Captures from Waterworks and a campaign level at the n-th call of `c6b30`,
`c60c0`, `cba50`, `c85c0` and `c7840` are not done (harness: synthetic
pools; the `_ftol2` stub pops without storing, so the `+243` and `+1f8`
bytes are checked against stub answers only). That the x87 crate's
transcendentals do not depend on the rounding mode is read from its
source, not tested.

## Slice 6b bodies

Each is **C** for its control flow and accesses: read at its address with
`h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (the four roots below over the slice 6a
dressing, and the unit update root). Field names are ours and **L**. The
body files (`engine::sim_objects::unit_state`: `control.rs`, `state.rs`,
`seats.rs`) record each frame and call order in their headers.

**Transfers.** The eight integer registers, the `xmm` registers, MXCSR and
the x87 state are retail's at every call and return, and every store
retail makes is made, stack stores included. Flags are left undefined at
every transfer (`scripts/sim-liveness.py`). x87 sequences in `c49b0` and
`ce0c0` run through `x87::abi`.

**Constants.** String IDs and the `0.5` pushed at `c59e1` and `c59fa` are
read from the instructions that carry them in the loaded image.

**Control `c49b0`.** Sets the desired aim and look (`+15c`, `+180`) from
`d0080`'s facing or the parent's seat node, then the timed bits, the two
linked objects at `+248` and `+24c`, and the `+334`/`+338` pair. The
`idiv` by the tick count is the guest's divide error when it is 0 (fault
at `c4c19`).

**State `c58f0`.** Timers, unit states through `e68c0`, held weapons' time
stamps and states, `110ab0` and `cbec0`, the idle test `c85c0` (fused),
`100430` per held weapon, then `d1540`.

**Seats `c5eb0` and refresh `ce0c0`.** `c5eb0` calls `d0c10` and `ce0c0`,
then moves each powered seat toward 1 or 0 into body `+25c + 4n`; its
result is the top byte of the `ecx` it pushed, which the Halo 1 drive
model's `before` hook on `ce0c0` sets through the return address `c5f04`.
`ce0c0` clears body `+134` bits 25 and 26, asks `d1080` for the control
frame and stores the position and basis changes. The drive model's hook
makes `UNIT_CONTROL_REFRESH` a guarded body: `c5eb0` reaches it through
the runtime, which runs the hook, then the body.

**Callees** are callouts except `c85c0`, `ce0c0`, `e68c0` and `e4050`
(bodies, fused); `ret` sizes are in the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 6b bodies".

### Not verified for slice 6b

Captures from Waterworks and a campaign level with vehicles and turrets are
not done (harness: synthetic pools, stubbed callees). Units with linked
objects of another type, and `10f5f0`'s other names, are read from code
only. The drive model's hook with the body guarded is read from the
installer (`hooks::install::guard_shared_bodies`), not run.

## Slice 9 body

**C** for its control flow and accesses: read at `fdfb0` with `h2 disasm`
(`fdfb0..ff576`, 77 calls, five jump tables), and compared with the lifted
original by `tests/sim_objects_differential.rs` (root `WEAPON_UPDATE`, and
inside the type dispatcher and the walks), which compare the float state
at every callout. Field names are ours and **L**.

**One body, three files** (`engine::sim_objects::weapon`). A body is one
row in `BODIES`; the dispatcher in `weapon/mod.rs` picks the file by retail
address:

| File | Range | Covers |
|---|---|---|
| `state.rs` | `fdfb0..fe5ff` | frame, owner, animation block, timers, charge, heat, effect |
| `triggers.rs` | `fe5ff..fefee` | the trigger loop, states 0 to 7 |
| `barrels.rs` | `fefee..ff576` | the barrel loop, heat, spin, the return |

Each file's header records its fields; `weapon/mod.rs` records the frame
and the registers kept across calls. The jump tables are read as
retail-address arrays.

**Frame.** `sub esp, 38` and four pushes. The barrel loop indexes the body
through `[S+34]` even after a callee moved it, as retail does.

**Floating point.** Scalar SSE through `sim_objects::sse`; two x87
sequences (the heat value for `_ftol2` `322b0c`, and the spin clamps) run
on the guest's x87 state through `x87::abi`.

**Callees** are all callouts; none is a body yet. Register arguments and
`ret` sizes are in the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 9 body".

### Not verified for slice 9

Captures from a Waterworks match and a campaign level at the n-th call of
`WEAPON_UPDATE` are not done. The harness does not compare `st(0)` at
callouts, so a wrong heat value shows only in what `322b0c` answers into
body `+175`. The liveness tables do not cover this body's sites; the body
keeps all eight registers exact.

## Slice 10 body

**C** for the control flow and accesses: read at `efde0`, `f1320` and
`f06c0` with `h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (root `VEHICLE_UPDATE`, and the type
update dispatcher over a vehicle), which compare the float state at every
callout. Field names are ours and **L**. The body file's header
(`engine::sim_objects::vehicle`) records the frame, registers and call
order.

**Frame.** The stack probe is a callout, as in `109580`; the body resumes
at `efdea` with `esp` 1984 bytes down, then pushes `ebx`, `ebp`, `esi`,
`edi`. All 39 return sites are at that `esp`.

**Flags and floats.** The body reproduces the arithmetic flags at every
transfer (rule 3) through `sse.rs` and `alu.rs`; its scalar SSE work and
three x87 sequences (around `372d48`, `f02f5`, `f04e0`..`f04f2`) run on
the guest's state.

**The type physics.** Definition `+1f0` (`i16`) selects, through the
seven-entry table at `f069c`, `f1cd0` (`VEHICLE_TREAD_UPDATE`), `f1f80`
(`VEHICLE_WHEEL_UPDATE`), nothing, `f2360`, `f3010`, `f4800` or `2056e0`
(`VEHICLE_CONTACT_SOLVE`). These stay callouts, so the Halo 1 drive
model's hooks on them run as before.

**Callees** are callouts except the leaves, `bba20`, `e6830` and `111650`
(slice 6c, fused); `ret` sizes are in the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 10 body".

**Not ported.** The rest of the tree stays guest: the type physics, the
contact gather `205be0`, `ef4f0`, `ef870`, `efb60`, `f6010`, `f6670`,
`f6bd0` and `eec90`. Each is a later lane's body.

### Not verified for slice 10

Captures from a match with vehicles, one per vehicle type the table
dispatches, are not done (harness: synthetic pools, stubbed callees). The
harness does not compare x87 state at callouts, so the body's x87
sequences are checked only through what they store.

## Slice 6c bodies

Each body is **C** for control flow and accesses: read at its address with
`h2 disasm` and compared with the lifted original by
`tests/sim_objects_differential.rs` (a root per body over the unit dressing,
the shell `c6b30` fusing its five slice 6c children, and the vehicle roots,
which reach `111650`). Field names are ours and **L**. The headers of
`unit_rest.rs` and `unit_anim.rs` record each body's frame, kept registers
and call order. Per-body detail: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 6c bodies".

**Transfers.** The integer registers, `xmm`, MXCSR and x87 state are
retail's at every call and return, and every retail store is made. Flags are
left undefined: no entry or call site reads them
(`python3 -I scripts/sim-liveness.py 0xc6740 0x114c60 0xc6990 0xd0620
0xd0690 0xc6810 0xc50a0 0x111650`). Scalar SSE runs through `sse.rs`.

| Body | Address | What it does |
|---|---|---|
| Timers | `c6740` | counts down the unit's timers, the deferred injury notice (`1c9c80`) and `cffc0` |
| Dialogue | `114c60` | runs the dialogue block's timers and plays a line; Halo 1 hooks it (`before`), so it is guarded under Halo 1 |
| Team | `c6990` | the unit's team from the player, the actor, or the seated units of a vehicle |
| Seats | `d0620`, `d0690` | leaf walks of the parent chain and the children; return their found byte in `ecx` |
| Camouflage | `c6810` | Assassins skull marks AI units; `engine::skull_gate` answers it outside the campaign ([skulls.md](../game/skulls.md#the-campaign-gate)) |
| Light | `c50a0` | player's flashlight and camouflage toggles, campaign only; Halo 1 hooks it (`before`), so it is guarded under Halo 1 |
| Animation state | `111650` | picks animation names A and B and drives the seat, channel and weapon updates |

**Callees.** Every callee is a callout, with the bytes `ret` pops read from
its own `ret`; the list is in the note.

### Not verified for slice 6c

- No captures from Waterworks or a campaign level compared with
  `impl.sim.native.objects` off; the harness uses synthetic pools with
  stubbed callees.
- `111650`'s field names come from data flow, not from its callees.
- Which arm of `111650`'s ladder a stub answer reaches depends on the names
  read from the image; the test does not assert it.

## Slice 7a bodies

Bipeds and vehicles share the unit action update at `e6830`. The signed
offset at unit `+346` locates the action record, whose bitmap starts at
`+4`. The body visits set bits in ascending order, calls descriptor slot `+4`
with the object and action index, and clears the action when the result is
zero. It rereads the bitmap so it sees changes the callback made to later
actions; AL returns whether any callback answered nonzero.

The bodies are **C** for control flow and accesses, read with `h2 disasm`
and compared with the lifted original by
`tests/sim_objects_differential.rs` (a root per body over the slice 11
dressing plus a biped's fields, parents reached directly or through a link
flagged bit 26, and the type update dispatcher over a biped). Field names
are ours and **L**. The module header (`engine::sim_objects::biped`) records
`dd360`'s frame, kept registers and call order. Detail: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 7a bodies".

**Frame of `dd360`.** `ebx` is pushed only once the biped has a physics
component (body `+b4`), so the early return pops three registers and later
sites sit four bytes lower. The body reads the seated-path slots where retail
does and nowhere else.

**Flags and floats.** Every body reproduces the arithmetic flags at every
transfer (rule 3) through `sse.rs` and `alu.rs`. Three `fld`/`fistp` pairs
(`dd476`, `e409a`, `146a0a`) run on the guest's x87 state.

**Stores that look dead.** `1469f0`, `e4c50` and `de190` make stores that
look dead but change what `ecx` or the stack hold on return; the bodies make
every stack store retail makes.

**Callees** that stay callouts, with the bytes `ret` pops, and the fused
bodies (`bba20`, `ba1d0`, `cba50`, `111650`, `e6830`) are listed in the note.

**A split block.** Function discovery takes `de336`, the look update's third
clamp step, for a function start (`h2 analyze --explain de336`). The body
runs it as its own block; the harness's lifted side steps on through it
(`SPLITS`).

**Not ported.** The movement and animation steps, the seat updates `e3380`
and `e3700`, and the remaining leaves (named in the note) are later lanes'
bodies; the segments keep every frame slot and register they read.

### Not verified for slice 7a

- No captures from a match or a campaign level of `BIPED_UPDATE` (on foot,
  airborne, seated, riding a biped) compared with
  `impl.sim.native.objects` off.
- `1469f0` and the leaves have callers outside the biped update; the bodies
  run there when the switch is on, and the harness enters them only as roots
  and from `dd360`.
- Callee `ret` sizes come from each callee's last `ret`.

## Slice 7b bodies

The bodies are **C** for control flow and accesses, read with `h2 disasm`
and compared with the lifted original by
`tests/sim_objects_differential.rs` (a root per body over the slice 7a
dressing plus the post-update's fields and contact records, and the type
post-update dispatcher over a biped). Field and point names are ours and
**L**. The module header (`engine::sim_objects::biped::post`) records
`dd990`'s frame, kept registers and call order. Detail: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Slice 7b bodies".

**Frame of `dd990`.** `ebx` is pushed only once the biped has a physics
component (body `+b4`); the early return pops three registers (`ddf46`) and
the rest four. The output `1e5a60` writes overlays the root velocity slots.

**The contact switch.** The output's index is checked against the
component's count `+8c` with `jle`, so an index equal to the count reads the
record past the last. The record's kind (bits 29 to 31 of `+4`) selects a
surface index, an object through `[51e9cc]`, a structure surface through
`[4e0348]`, or the record's own object; each stores the five contact words
`+354..364` in retail's order.

**Flags and floats.** Every body reproduces the arithmetic flags at every
transfer (rule 3) through `sse.rs` and `alu.rs`. Five `fld`/`fistp` pairs
(`e441a`, `e46e2`, `e2ffa`, `e30a5`, `e3314`) run on the guest's x87 state.

**Read from the image.** The `.rdata` constants, the down vector `[4687bc]`,
the probe's immediates and `dd0d0`'s jump table `dd2e4` are read from the
image. The table has no bounds check: a state of 0 or above 6 returns an
unowned fault, and the harness keeps the state in 1 to 6.

**Stores that look dead.** `e4330`, `dd990` and `dd0d0` make stack stores
nothing reads; the bodies make every stack store retail makes.

**Callees** that stay callouts and the fused bodies are listed in the note.
None reaches `bd090`, so no root is in `FLOAT_OPT_OUT`, and no function
start lies inside the ten bodies, so none needs `SPLITS`.

**Not ported.** The AI hooks, the damage packet, the jump `e4770`, the
physics step `1e68a0` and the queries below the contact switch and the probe
are later lanes' bodies.

### Not verified for slice 7b

- No captures from a match or a campaign level of `BIPED_POST_UPDATE`
  compared with `impl.sim.native.objects` off.
- The leaves and segments may have callers outside the post-update; the
  bodies run there when the switch is on.
- The `fistp`s assume the guest's control word masks the precision
  exception, as in slice 7a.
- Callee `ret` sizes come from each callee's last `ret`.

## Register liveness

`scripts/sim-liveness.py` answers two questions for every entry above and
every direct callee of their bodies. **Entry:** which registers and flags a
function reads before writing, saves and restores, or leaves unwritten at a
`ret`. **Site:** for every call in the slice 1 to 5 bodies, which registers
and flags the caller reads after the return, which a replacing body must hand
back.

Rerun it from the worktree (it runs `h2 disasm` in the main checkout, where
`disc/default.xbe` is):

```sh
python3 -I scripts/sim-liveness.py                 # these tables, markdown
python3 -I scripts/sim-liveness.py --text          # with "reaches call X with R unwritten"
python3 -I scripts/sim-liveness.py 0xbc470 0xbdef0 # chosen entries and their sites
python3 -I scripts/sim-liveness.py --h2 ~/.cargo/bin/h2 --repo <checkout>
```

The walk follows both arms of every branch, stops at `ret`, `int3`, an
indirect `jmp`, a jump out of the function, when everything is written, or
after 400 instructions, and does not enter calls (it over-reports reads after
a call, never under-reports). Limits: indirect jumps end the path; only
integer registers and EFLAGS are tracked (not `xmm` or x87); rows marked
"truncated" (`b7b40`, `1d96d0`, `16a8e0`) are a lower bound; `esp` is left
out. The full list of limitations: see the
[detail note](notes/2026-11-26-object-update-detail.md), "Register liveness".

### Entries and callees

One row per entry or callee: registers read before writing, flags read (DF
comes from `rep movsd` and `rep stosd`), registers saved and restored, and
registers unwritten at `ret`. The table is generated by the script above:
see the [detail note](notes/2026-11-26-object-update-detail.md), "Entries
and callees".

### Call sites

Every call in the update bodies, with the registers read after return and
the registers that reach the caller's `ret` unwritten, is listed in the
`Body::sites` tables. Columns: caller, call address, retail return site,
callee (or indirect), registers read after return, flags, unwritten at `ret`.

Table: see the [detail note](notes/2026-11-26-object-update-detail.md),
"Call sites".

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
  interleaving in a film (on/off run parity with `impl.sim.arena-hash`).

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
