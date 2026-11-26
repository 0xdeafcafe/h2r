# Object update: entries, conventions and liveness: detail moved off the page

Evidence and detail for [object-update.md](../object-update.md).

## 2026-11-26: detail moved off the design page

Read at commit `c7ed1e40c`. These blocks are the page's text at that commit, verbatim, moved here when the page was cut down to its design. Reproduce with `git show c7ed1e40c:docs/engine/object-update.md`. They describe the code as it was at that commit; the page states the current design.

### Status line: slice-by-slice history and curve arm detail

Status: Partly built. Slice 1's, slice 3's, slice 4's, slice 5's, slice 6a's, slice 6b's, slice 6c's, slice 7a's, slice 8's, slice 9's, slice 10's and slice 11's bodies are in the body table behind the switch: the three walks, the per-object pre- and post-update, the four type dispatchers, the wake leaves and the early-mover wake; function values, the export evaluator, the import dispatcher, every type level's import handler and change colours; the light walk, the light record update and its phase step, and the light-list link, unlink and expire; the damage update `d5de0`, with the tick's `Damage` and `Kill` events; the item, scenery, device, machine, crate, creature and projectile updates, and the projectile step `f8eb0`; the post-update shell (component transform, node matrices, bounds, attachment refresh, node-matrix notify) and the transform setters and getters; the unit update's shell `c6b30`, the movement `c60c0`, the direction clamp `cba50`, the idle test `c85c0` and the aim update `c7840`; the unit update's control `c49b0`, state `c58f0`, powered seats `c5eb0` and control refresh `ce0c0`; the unit update's timers `c6740`, dialogue update `114c60`, team `c6990` with its seat iterator `d0620` and `d0690`, active camouflage `c6810` and integrated light `c50a0`, and the unit animation state `111650`; the weapon update `fdfb0`; the vehicle update `efde0` with its gate `f1320` and contact reset `f06c0` (the type physics and the rest of its tree stay callouts); the biped update `dd360` with the carrier `baf40`, the timer update `e2d80`, `1469f0`, the action request `e68c0`, the animation kind `dd300`, the landing test `e4050`, the fall update `e45e0`, the animation state update `e4c50` and the look update `de190` (the movement and animation steps and the seat updates stay callouts); the biped post-physics update `dd990` with the crouch step `dd7d0`, the crouch height `dd8e0` and its forced test `e0dc0`, the ground probe `e30c0` and its surface test `ddf60`, the jump update `e4680`, the physics input `dd0d0`, the landing update `e4330` and the contact update `e2fa0` (the AI hooks, the damage packet and the physics step stay callouts). The tag-function curve evaluator `13b390` runs as an arm: the idiomatic kernel `sim-objects/src/functions/curve.rs` predicts every call and the guest's answer is compared with it, until 4096 calls have matched with no mismatch; from then on the kernel answers, and one answerable call in 64 still goes to the guest and is compared, so a later mismatch returns every call to the guest. A map load (`CACHE_TAGS_LOAD`) clears the proof and any mismatch, so each map earns the take-over again. Counted in `engine::sim_objects::functions::parity`. The other slices of [sim-rewrite-objects.md](../sim-rewrite-objects.md) add the rest.

### Code line: per-file lists and test descriptions

Code: `crates/rewrites/sim-objects` (kernels and typed views, no `runtime` dependency: `src/wake.rs` the wake leaves, `src/events.rs` the tick's events, `src/damage.rs` the damage and kill event reads, `src/bounds.rs` the bounds kernel, `src/functions/curve.rs` the tag-function curve evaluator), `crates/rewrites/engine/src/sim_objects` (the guest boundary: `BODIES`, `drive`, `supported`, retail frame helpers; `walks.rs`, `object.rs`, `wake.rs` the slice 1 bodies; `functions/` the slice 3 bodies, `functions/parity.rs` the curve evaluator's capture arm; `lights/` the slice 5 bodies; `damage.rs` the slice 8 body and the event recorders; `item.rs`, `scenery.rs`, `device.rs`, `machine.rs`, `bloc.rs`, `creature.rs`, `projectile.rs`, `projectile_step.rs` the slice 11 bodies over `sse.rs` (scalar SSE on the guest registers) and `alu.rs` (shift and 16-bit flag idioms); `weapon/` the slice 9 body, one file per state group; `vehicle.rs` the slice 10 bodies; `biped/` the slice 7a bodies, `biped/post/` the slice 7b bodies; `post.rs`, `pose.rs`, `bounds.rs`, `transform.rs` the slice 4 bodies over `xform.rs` (the node-matrix rows); `unit.rs` the slice 6a bodies, with `sim_physics::unit_aim::step` for `c7840`; `unit_state/` the slice 6b bodies; `unit_rest.rs` and `unit_anim.rs` the slice 6c bodies), `crates/rewrites/sim-bridge/src/sim_objects.rs` (installer). Tests: `crates/rewrites/engine/src/sim_objects/tests.rs` (the dispatcher, guarding, finalize and a leaf on a synthetic pool), `crates/rewrites/engine/src/sim_objects/functions/tests.rs` (the slice 3 helpers and the import dispatcher on synthetic memory), `crates/rewrites/engine/src/sim_objects/functions/curve_tests.rs` (the curve kernel against the `function_eval` oracle, and the arm's counters), `crates/rewrites/engine/src/sim_objects/damage/tests.rs` (the damage update's frame and the recorders), `crates/rewrites/engine/src/sim_objects/unit/tests.rs` (the slice 6a frames, callouts and stores), `crates/rewrites/engine/src/sim_objects/biped/tests.rs` (the slice 7a leaves, and the biped update's frame, callouts and stores), `crates/rewrites/engine/src/sim_objects/biped/post/tests.rs` (the slice 7b leaves, the crouch step, and the post-update's early returns, physics input path and contact counter), `crates/rewrites/engine/src/sim_objects/unit_state/tests.rs` (the slice 6b frames, callouts, stores and the divide error), `crates/rewrites/engine/src/sim_objects/lights/tests.rs` (the slice 5 bodies' frames, sites and stores on a synthetic light array), `crates/rewrites/sim-bridge/src/sim_objects.rs` (the installer, the guard, the event observers, and resumes at a body's sites on the unit shell), `crates/rewrites/engine/tests/sim_objects_differential.rs` (fused transcripts and mixed frames against the lifted original for every slice 1, slice 3, slice 4, slice 5, slice 6a, slice 6b, slice 6c, slice 7a, slice 7b, slice 8, slice 9, slice 10 and slice 11 root, comparing the integer registers, flags, memory, MXCSR, XMM registers and x87 state at every callout and return (`FLOAT_OPT_OUT` lists the roots that do not, each with its reason), feature `differential`), `crates/rewrites/sim-objects/src/wake/tests.rs` (the wake kernels), `crates/rewrites/sim-objects/src/damage/tests.rs` (the event reads), `crates/rewrites/sim-objects/src/bounds/tests.rs` (the bounds kernel), `crates/rewrites/engine/src/sim_objects/bounds/tests.rs` (`bdef0` against that kernel), and the views in `crates/rewrites/sim-objects` and `crates/rewrites/sim-state`.

### Code paths: ports kept as oracles, aim sharing, import ports

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
  code (`pose.rs` runs it), so it stays installed under
  `impl.sim.native_pose_matrix` for when this switch is off.

### Slice 1: walks and dispatchers, ABI table

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

### Slice 3: functions and colours, ABI table

**Slice 3: functions and colours**

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `bab40` | `OBJECT_FUNCTION_VALUE` | `(object: stack 0, name: stack 1, value: stack 2) -> al`; `ret 0c` | segment, then leaf |
| `ba8c0` | `OBJECT_FUNCTION_EVALUATE` | `(record: eax, object: stack 0, value out: stack 1) -> al`; `ret 8`; stores the `f32` at `[stack 1]` on every path except a scripted override | segment, then leaf |
| `108d90` | `OBJECT_FUNCTION_IMPORT` | `(object: stack 0, name: stack 1, value: ebx, active: edi) -> al`; `ret 8`. Stores 0 to `[ebx]` and `[edi]` first, then calls slot `+4c` per ancestry level as `(object, name, value, active)` until one returns non-zero. Clobbers `xmm0` | segment (indirect callout) |
| `bcc20` | `BASE_OBJECT_IMPORT` | `(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al`; `ret 10` | segment, then leaf |
| `c6b90`, `de3b0`, `f6d10`, `ff5f0`, `fbe70`, `106d70`, `11bd60` | `UNIT_`, `BIPED_`, `VEHICLE_`, `WEAPON_`, `PROJECTILE_`, `DEVICE_`, `MACHINE_FUNCTION_IMPORT` | the slot `+4c` shape: `(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al`; `ret 10`; no register arguments | segment or leaf |
| `d1210` | `UNIT_IMPORT_FRACTION` | `(unit: eax) -> xmm0`; `ret`. Called by `c6b90` and `f6d10` | leaf |
| `d1410` | `UNIT_IMPORT_ANIMATION` | `(unit: eax) -> xmm0`; `ret`. Calls `1c6e30` | segment |
| `be8e0` | `OBJECT_CHANGE_COLORS` | `(object: stack 0) -> al`; `ret 4` | segment, then leaf |
| `13bb90` | none yet | `(function: eax, stack 0, stack 1)`, input in `xmm0` (clamped), result in `xmm0`; `ret 8`. Calls the evaluator `13b390` | callout |
| `13b390` | `FUNCTION_EVALUATE` | `(reference: edi, input: stack 0, range: stack 1) -> xmm0`; `ret 8`. The curve evaluator under every function value, light phase and screen effect ([engine-map.md](../engine-map.md), `function_evaluate`) | arm (`functions/parity.rs`): the kernel `functions/curve.rs` predicts every call; the guest answers and is compared until 4096 matches with no mismatch, then the kernel answers (`xmm0`, the popped frame; `eax`, `ecx`, `edx`, `edi` untouched, flags undefined) and every 64th answerable call is compared; curve kinds 9 and 10 decline |

### Slice 4: post-update shell, ABI table

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

### Slice 5: lights, ABI table

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `c1400` | `LIGHTS_UPDATE` | no arguments; `ret` | segment |
| `c0c80` | `LIGHT_RECORD_UPDATE` | `(light: stack 0)`; `ret 4` | segment |
| `c0b00` | `LIGHT_RECORD_PHASE` | `(colour: eax, block: ecx, light: stack 0, input: stack 1, brightness: stack 2, out: stack 3)`; `ret 10`. `c0c80` passes the light element's `+d4` colour triple in `eax` and the definition's `+c0` block in `ecx` | segment |
| `c2d00` | `LIGHT_LINK` | `(light: stack 0)`; `ret 4` | segment |
| `c3220` | `LIGHT_UNLINK` | `(light: eax)`, light array `[4e030c]`, stride `110`; `ret` | segment |
| `c3260` | `LIGHT_EXPIRE` | `(light: stack 0, notify: stack 1)`; `ret 8` | segment |

### Slice 11: small types, ABI table

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

### Slice 6a: unit shell and movement, ABI table

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `c6b30` | `UNIT_UPDATE` | `(unit: stack 0) -> al`; `ret 4`. Slot `+40` of `unit`; bipeds and vehicles inherit it | segment |
| `c60c0` | `UNIT_MOVEMENT` | `(unit: stack 0) -> al`, whether the aim or look moved; `ret 4` | segment |
| `cba50` | `UNIT_CLAMP_DIRECTION` | `(unit: eax, direction: stack 0, look: stack 1) -> al`, whether it clamped; `ret 8` | segment |
| `c85c0` | `UNIT_IDLE_TEST` | `(unit: eax) -> al`; `ret`. Called from `c58f0` (slice 6b) | segment |
| `c7840` | `UNIT_AIM_UPDATE` | `eax`, `esi`, `edi` and five stack arguments; `ret 14` | segment |

### Slice 9: weapon, ABI row

| `fdfb0` | `WEAPON_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `weap`; the item update runs before it | segment |

### Slice 10: vehicle, ABI rows

| `efde0` | `VEHICLE_UPDATE` | `(object: stack 0) -> al`; `ret 4`. Slot `+40` of `vehi`. A `1984`-byte frame made through the stack probe `320560` (`eax` = size) | segment |
| `f1320` | `VEHICLE_UPDATE_GATE` | `(object: eax) -> al`; `ret` | leaf |
| `f06c0` | `VEHICLE_CONTACTS_RESET` | `(buffer: eax)`; `ret` | leaf |

### Slice 6b: unit state, ABI rows

| `c49b0` | `UNIT_UPDATE_CONTROL` | `(unit: stack 0) -> al`; `ret 4`. `c6b30`'s first call | segment |
| `c58f0` | `UNIT_UPDATE_STATE` | `(unit: stack 0) -> al`; `ret 4`. `c6b30`'s second call | segment |
| `c5eb0` | `UNIT_POWERED_SEATS` | `(unit: eax) -> al`; `ret`. `c6b30`'s fourth call. `ecx` returns with the result in its top byte | segment |
| `ce0c0` | `UNIT_CONTROL_REFRESH` | `(unit: edi)`; `ret`. Called from `c5eb0`; the Halo 1 drive model hooks it, so the body is guarded when that feature is on | segment |

### Slice 6c: the rest of the unit, ABI rows

| `c6740` | `UNIT_TIMERS` | `(unit: eax) -> al`, whether a timer was running; `ret` | segment |
| `114c60` | `UNIT_DIALOGUE_UPDATE` | `(unit: eax)`; `ret` | segment |
| `c6990` | `UNIT_TEAM_UPDATE` | `(unit: eax)`; `ret` | segment |
| `d0620` | `UNIT_SEATS_BEGIN` | `(unit: eax, iterator: ebx)`; `ret`. `ecx` returns 1 when it climbed | leaf |
| `d0690` | `UNIT_SEATS_NEXT` | `(iterator: edx) -> al`; `ret`. `ecx`'s top byte returns `al` | leaf |
| `c6810` | `UNIT_CAMO_UPDATE` | `(unit: eax)`; `ret` | segment |
| `c50a0` | `UNIT_LIGHT_UPDATE` | `(unit: eax)`; `ret` | segment |
| `111650` | `UNIT_ANIMATION_STATE` | `(unit: stack 0, record: stack 1) -> al`; `ret 8`. Called by the vehicle update among others, not by `c6b30` | segment |

### Slice 7a: biped, ABI rows

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

### Slice 7b: biped post-physics, ABI rows

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

### Callouts that stay guest in this phase, ABI table

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

### Resolved in slice 0: DEFER setter

**DEFER (`20`) setter.** C. The only store found is in the attach function
`b93b0` `(parent: stack 0, child: stack 1, node: stack 2)`, `ret 0c`: it
links the child under the parent (child body `+14`, `+18`, parent `+10`,
child `+0c`), sets CHILD (`b9532`), and after the transform and attachment
refresh sets DEFER on the child's header (`b97df`, stored at `b97e4`). So an
object attached during a tick is skipped by `bc470` (returns 1) for the
rest of that tick's walks, and finalize clears DEFER. Method: every
`or byte ptr [r+2],20h` and `or r8,20h` followed by a flags store in
`.text`; the other hits address other arrays (`b465e` on `[4cf78c]`,
`6c067` and the `6c...` run, `21fd33` on a `+3` byte).

### Resolved in slice 0: NEW at creation

**NEW (`08`) at creation.** C. `object_new` `b7b40` sets NEW on the new
header (`b7c07`, stored at `b7c0e`) right after taking the slot, before it
stores the type at `+3`. `109580` sets it on woken weapon, equipment,
garbage and projectile objects (`109638`).

### Resolved in slice 0: the early-mover list

**The list at `[5107f0]+1d00`.** C: early movers. Appended by `109390`
`(object: edx)` from `object_new` (`b8332`) when the definition's flags
byte `+2` has bit 6 set and the count is below 32; it sets body `+c0`
bit 3. Removed by `109400` `(object: stack 0)`, `ret 4`, from
`OBJECT_DELETE` (`b85d9`): swap with the last entry, clear `+c0` bit 3,
then every live object woken by this mover (body `+b8` equal, `+c0` bit 1
or 2) is passed to `b9b90` and woken again (`b7360`); bipeds go through
`e4c10` first. `109360` zeroes the whole block (`762` dwords) and sets the
byte `+1d84`; `109380` clears that byte. Bit 6 of definition flags is the
"early mover" flag of the community tag layouts (L for the name only).

### Resolved in slice 0: what 109660 writes

**What `109660` writes.** C. A record per list index at
`[5107f0] + index × e8` (the 32 records fill `+0..+1d00`, the handle list
follows). From the mover's physics component (body `+b4`) it picks the
first rigid body whose shape is flagged, then: copies the current block to
the previous block (`+7c..+af` to `+48..+7b`, and three vectors `+0c`,
`+24`, `+3c` to `+00`, `+18`, `+30`) when `+e5` is set, and sets `+e4`;
writes the current rigid-body transform at `+7c` (`1d08e0`), two velocity
vectors at `+24`, `+3c` and a vector at `+0c`; sets `+e5`; and when a
previous transform exists, stores the delta transform previous⁻¹ × current
at `+b0` (`141590`, `142a60`) and re-orthonormalises its basis (`30bf0`).
Without a component or a flagged body it clears `+e4` and `+e5`.

### Resolved in slice 0: who reads it, and when

**Who reads it, and when.** C for the call chains, L for the purpose
(riders move with what woke them). The readers find the record through
body `+b8` (the waking mover):

| Reader | Called from | Walk | Record it sees |
|---|---|---|---|
| `109a00` `(object: stack 0, ..)`, `ret 0c` | projectile update `f8de0` (`f8e52`); item update `10b4d0` → `10b850` | pre-physics | the previous tick's |
| `109e00` (root ancestor's mover), `ret 0c` | biped update `dd360` → `e3700` | pre-physics | the previous tick's |
| `109e00` | biped post-physics `dd990` → `e4680` → `e4770` | post-physics slot pass | this tick's (the early-mover pass ran first) |
| `109e00` | barrel fire `104150` (from `fd910` → `fd980`), players `14e200`, AI `1e0fe0`, `1ed340`, `1a68a0`, `e7aa6` | post-physics step 3 for weapons; others outside the walks | this tick's |
| `109fd0` `(mover: stack 0, out: edi)`, `ret 4`: copies record `+24` | AI `1eb6f0`, `1f2250` | AI, before the walks | the previous tick's |

So riders read `109660`'s record in the same tick from the post-physics
walk (biped post-update, deferred weapon fire) and in the next tick from
the pre-physics walk.

### Resolved in slice 0: bc470 step 3

**`bc470` step 3.** The byte `[[4e6948]+c]` is the game's network mode,
written at game start (`0x6ec80`; [bots.md](../../game/bots.md) has the
derivation): 1 local, 2 synchronous client, 3 synchronous server, 4
distributed client, 5 distributed server. C (writers `67fd6..68054`,
`6af09`, `6afd4`; readers across the object, unit and damage code compare
it with 4). `a9570` `(object: eax)` applies a pending replicated transform:
when body `+d8` bit 0 is set it gathers state (`a9820`), applies it
(`a9a70`, `a9b40`, `a99f0`) and clears the bit; a parented object goes
through `a9640` instead. The bit is set by `a96d0` and `a9770`, which take
the body's position (`+64`) and an orientation vector from the
distributed-simulation entity update `aab40`; body `+d4` is the entity
index (reset by `a7ab0`). C for the stores, L for "replicated transform".

### Resolved in slice 0: fd910

**`fd910`.** C. No arguments, `ret`. Builds an object iterator on its
stack (type mask `4`, weapons; header mask `02`, AWAKE; slot cursor; the
handle out) and steps it with `baeb0`. For each weapon whose body `+16c`
bit 0 is set it calls `fd980(weapon)`, which fires every barrel
(`104150(weapon, barrel)`) whose state byte `+1a8 + 34 × i` has bit 1
set, clears that bit, and finally clears `+16c` bit 0. It is the deferred
weapon fire. `baeb0` advances the cursor at iterator `+6`, matches the
header flags against the mask byte at `+4` and the type against the mask
at `+0`, stores the handle at `+8` and returns the body.

### Resolved in slice 0: bf380 finalize tail

**`bf380` (finalize tail).** C. Object memory collection. It allocates a
`2a04`-byte frame through `320560`. The mode comes from the object
globals `[4de2f4]` bytes `+1`/`+2` when a request is pending, otherwise
from pool pressure: the free space of the pool at `[4de2ec]` and the free
object slots (`800` − `[4e0300]+3c`), gated by the word `[4de2f4]+4`.
`bf1c0(mode: eax, out: esi)` decides whether to run. It then walks the
table at `440578` (`10`-byte entries: a type mask at `-8`, a flag at `-4`,
an optional prepare function at `+0`, a collector at `+4`; the walk ends
at a null collector). The two indirect calls are `call eax` with the entry
`+0` value `(mode, buffer, 2800)` once per run of entries, and
`call [edi+4]` `(mode, buffer, 2800, out, done, 200)`. It clears the
request bytes `+1`, `+2` on exit.

### Resolved in slice 0: be1d0 to b8c00, 109140

**`be1d0` → `b8c00`, `109140`.** C. When the object's definition has a
model (definition `+38`) whose two references at `+4` and `+14` are set,
`be1d0` gets the node matrices and count from `b8c00` and calls the
slot `+74` dispatcher `109140(object, count, matrices)`. Only `unit`
(`4679b0`) sets slot `+74` (`1130f0`, unit animation).

### Resolved in slice 0: register conventions

**Register conventions.** C, in "Entries" above. Corrections to the design
page's section 5 table: `bc7b0` takes its object on the stack (`ret 4`),
not in a register; `108cd0` takes it in `edi`; `109050`, `1090d0`,
`108a90` and `109140` take it in `ebx`.

### Resolved in slice 0: the slot 1090d0 dispatches

**The slot `1090d0` dispatches.** C. Descriptor slot `+70`, set by `unit`
(`112dc0`) and `scen` (`10a390`). `bd090` calls it once per object at
`bd507`, after the root node matrix is composed (`bdc40`, `1421f0`,
`142a60`) and before the loop over the other nodes, with the root matrix as
its argument: it is the "after root" hook. The full slot map read from the
thirteen descriptors also has `+2c` (new) for unit, biped, vehicle, item,
weapon, garbage, projectile, scenery, device, machine, sound scenery, crate
and creature; `+74` for unit; `+78` for biped (`de4c0`) and vehicle
(`ee9b0`); `+7c` for unit (`114710`).

### Resolved in slice 0: RNG in function values

**RNG in function values.** C, for direct calls. No function reachable
from `ba8c0`, `bab40`, `13bb90`, `bcc20`, the seven import handlers or
`d1210`/`d1410` through direct calls (the whole closure, at most 36
functions) references the RNG global `[4e7408]`. The RNG is inlined, so a
draw is a reference to that global. The only indirect calls in the
closure are `108d90`'s slot calls (whose targets are the import handlers
checked here) and the import thunks reached from `bcc20` (`33198b`,
`3317d9`, `3315cf`). `13bb90` is a tag-function lookup with no state. So
function values and change colours are a pure kernel of their inputs;
section 7's wording holds.

### Resolved in slice 0: unit offsets

**Unit offsets.** C. Actor handle `+12c` (`unit_new` `c4410` stores none;
`c6810` tests it against none at `c682c`), team `+138` `i16` (`c684a`),
current weapon index `+212` `i8` and weapon handles `+218` `[u32; 4]`
(`c8755`, `c86ad`). They are named in `crates/hooking/guest-layout/src/unit.rs`
(`ACTOR`, `TEAM`, `WEAPON_INDEX`, `WEAPONS`) and in
[live-gamestate.md](../../live/live-gamestate.md), "AI bipeds, teams and
vehicles". The actor array is `[4f55f0]` (that page, L here).

### Resolved in slice 0: not answerable statically

**Not answerable statically.** Nodes posed per tick needs a count at
`bd090` over a Waterworks match and a co-op campaign level: slice 12's
step 0. The logical clock: today a Rust port is charged nothing for its own
guest accesses. The slice is counted in guest memory accessor calls
(`h2abi_v1_*`) made by generated code (`crates/runtime/runtime/src/preempt.rs`,
`spend_access`); a port reads and writes through the memory guard
(`ExecCtx::memory`, `runtime::Guest`), which does not spend; only a few hooks
charge explicitly with `ExecCtx::charge_accesses`
(`crates/runtime/recomp/src/fallback.rs`).
So a long Rust segment runs without expiring its slice, and the only
preemption points inside a walk are the true callouts. That is
deterministic. Whether to charge bodies is a decision, not a reading; a run
with `impl.sim.native.objects` on and off over the same film shows whether
thread interleaving changes.

### Slice 1 bodies: bitmap scan detail

The three walks and the typed object iterator use the same bounded bitmap scan
in `sim_objects::walk::next_live`. Each visit reads through the first occupied
slot and stops. The next visit reads the high-water mark and the bitmap again,
so a callback can activate or delete a slot in the same word.

### Slice 1 bodies: walks register detail

**Walks** (`engine::sim_objects::walks`). The pre- and post-physics
bodies are the per-function ports, with each transfer returned to
`drive` instead of the runtime. Finalize `b7150` keeps the objects array in `ebp`, read
once per pass. Its scan is one slot at a time: `test eax, eax; jl`, the
high-water mark into `esi`, `cmp eax, esi; jge`, the bitmap into `edx`,
then `ecx` the word index and `edi` the bit of each slot tried, until a
live slot or the mark. The body reads each bitmap word once and leaves
`eax`, `ecx`, `edi` and the flags as the last slot tried left them. Pass
A keeps the slot in `ebx`, the handle in `edi` and the header in `esi`;
it stores the flags with DEFER cleared (`dl`, so `edx` holds the bitmap
pointer with that byte), and for a NEW, ACTIVE, AWAKE object without
DELETE stores them again with NEW cleared, then calls `bc470` and, if
POST is set after it, `bc820`. Pass B keeps the slot in `edi` and the
handle in `esi` and calls `b83b0(handle, 1)` then `b8460(handle, 1)` for
each DELETE object. The phase byte is cleared after `pop edi`, `pop esi`
and `pop ebp`, before `pop ebx` and the jump to `bf380`.

### Slice 1 bodies: events detail

**Events.** Before finalize's `b83b0` call the body appends
`ObjectDeleted { id, kind, definition }` (the handle, header `+3`, body
`+0`) to the tick's buffer (`sim_objects::events`, read through
`sim_state::events`). The buffer belongs to the thread that runs the
walks, and it is emptied at `GAME_TICK`'s entry ("Slice 8 body"). Those two reads are not retail's,
so a fault there leaves the field none instead of faulting the guest.

### Slice 1 bodies: type dispatchers table

**Type dispatchers** (`object.rs`). Each reads the type from body `+aa`
(`movsx`), the descriptor from `[468630 + 4 × type]`, and walks the
ancestry at descriptor `+84` (descriptor pointers, null-terminated). Per
level it loads `eax` from the level's slot and calls it when non-null;
between levels it keeps the address of the level entry in `eax`, the
entry in `ecx` and (except `108cd0`) the level number sign-extended from
16 bits in `edx`.

| Body | Keeps | At the slot call |
|---|---|---|
| `108c60` | object `ebp`, descriptor `edi`, level `esi`, result `bl` | `push ebp`; after it `or bl, al` |
| `108cd0` | address of the level entry `esi` (pushed only when the list is not empty) | `push edi` |
| `109050` | `s2` `ebp`, descriptor `esi`, level `edi` | `edx` = `s1`, `ecx` = `s0` from the frame, then `push ebp, edx, ecx, ebx` |
| `1090d0` | matrix `ebp`, descriptor `edi`, level `esi` | `push ebp, ebx` |

The slots resolve per type through the ancestry, so a type without a slot
of its own gets its parent's: weapons, equipment and garbage get the item
update `10b4d0`, machines, controls and light fixtures the device update
`106790` (and the device pose slot `107000`), bipeds and vehicles the
unit update `c6b30` before their own. Sound scenery has no update slot at
any level.

### Slice 1 bodies: wake leaves registers and stack

**Wake leaves** (`sim_objects::wake`, bodies in `engine::sim_objects::wake`).
Plain functions over guest memory; the body puts their exit in `eax`,
`ecx`, `edx` and the flags. They save or never write `ebx`, `ebp`, `esi`
and `edi`. The header address is `(handle & ffff) × c` (the stride is
hard-coded, not the array's element size).

| Leaf | `eax` | `ecx` | `edx` | Flags |
|---|---|---|---|---|
| `b7360` | the object when it was already AWAKE; else none | the header storage, re-read after each store | `(object & ffff) × 3` | `test` of AWAKE (all clear) when it was AWAKE; else `cmp` with none (ZF, PF) |
| `baf80` | the root ancestor; none for none | none | the header storage; the caller's for none | `cmp` with none |
| `bc7b0` | for a leaf, the object or `b7360`'s `eax`; else the last child's body | none | for a leaf, `1 << type` or `b7360`'s `edx`; else the last child's `edx` | `cmp` with none |
| `bba20` | `bc7b0`'s | `bc7b0`'s | `bc7b0`'s | `bc7b0`'s |

`bc7b0` reads each child's next sibling after the child's subtree,
through the child's header again. The body keeps an explicit stack of
open frames instead of recursing, and reports a fault past `2^16` levels
(a cycle in the child links, where retail recurses until the guest stack
runs out).

The leaves leave the stack below their entry `esp` as retail does
(`sim_objects::wake::Stack`): `b7360` pushes `ebx` and `esi`, `bba20`
pushes `ebx`, `esi` and `edi` and calls `baf80`, `b7360` and `bc7b0`
(returning to `bba49`, `bba63`, `bba6d`), and frame `d` of `bc7b0` is
entered `c × d` below the first and pushes `esi`, then for `b7360` its
return `bc7e6`, then for each child the handle and the return `bc80c`.
Those slots are dead when the leaf returns, but the caller's next deeper
frame overlays them, and a local it never writes holds what the leaf left
there.

### Slice 1 bodies: early-mover wake frame

**Early-mover wake** (`wake.rs`). `109580` sets `eax` = `1004` and calls
the stack probe `320560`, which leaves the caller's `ecx` at the top of
the new frame and `esp` `1004` lower. After its pushes (`ebx`, `ebp`,
then `esi`, `edi` once the count is positive) the frame holds the count
at `[esp+10]`, the result buffer at `[esp+14]` and the mover at
`[esp+1018]`. `ebx` keeps the objects array from before the visit across
every call. The visit is `bb050(0, 3f, location, centre, radius bits,
buffer, 400)`; its `ax` is sign-extended. Per result: `or` body `+c0`
with 4, then store the mover at `+b8`, then `b7360`; for types 2 to 5,
`bba20` and `or` the header with NEW.

### Slice 3 bodies: ExportCursor detail

The `sim_objects::functions::ExportCursor` scans consecutive misses in
one Rust loop. A match returns to the evaluator with the record, index
and byte offset in the guest frame. On resume the body advances that
cursor and reads the current count, table address and requested name.
This keeps duplicate exports in definition order even if a callback
replaces the table.

### Slice 3 bodies: Function value bab40

**Function value** `bab40` (`functions/value.rs`). Saves `ecx`, `ebx`,
`ebp`, `esi`, `edi`; stores 0 through the value pointer first. The
result byte is the top byte of the saved `ecx` slot, so `ecx` at return
carries it. Order: the scripted override `10aac0` when body `+c0` bit 9
is set (`ebx` the object, `(name, value, &[stack 2])`); the built-in
names (none or "one" store 1.0, "zero" stores 0); for a valid handle the
import `108d90` (`ebx` the value pointer, `edi` the result byte); then
every export of the definition (`+64` count, `+68` records of `20`
bytes, the name at `+8`) through `ba8c0`, each match storing its result
byte, all matches evaluated; then, when body flags bit 26 is set and
the parent (`+14`) is valid, the parent's value (`bab40`, tail-like:
its registers are returned). `esi` holds the header offset until the
exports, then the export index; `ebx` the record offset; `edi` the
definition. Before the exports the value pointer's argument slot is
overwritten with the body.

### Slice 3 bodies: Export evaluator ba8c0

**Export evaluator** `ba8c0`. A `10`-byte frame below the saved `ebx`,
`ebp`, `esi` (`edi` pushed after the override): the value at `E-0c`,
the time input at `E-8`, the hash at `E-4`, "input is time" (the byte
the record's `+18` points at is 3) at `E-0d`, "not a range" (record
flags bit 1 clear) at `E-0e`; the low byte of the object's argument
slot is the active flag. Steps: the override `10aac0`; the input import
`108d90(object, record +4)`; the function `13bb90` (record `+14`, input
inverted when flags bit 0) or, for a time input, game time
(`[510c54]`: ticks × seconds per tick) plus, with flags bit 3, a hash of
the handle through the x87, then `13bb90` scaled by the value; a scale
`bab40(object, record +1c)`; a threshold (record `+10`, when positive);
flags bit 2 forces active; a gate import `108d90(object, record +c)`;
the result clamped to `[0, 1]`, or 0 when inactive.

### Slice 3 bodies: Import dispatcher 108d90

**Import dispatcher** `108d90`. Zeroes `[ebx]` and `[edi]`, keeps the
object in `ebp` and the ancestry entry in `esi` (saved), and calls slot
`+4c` of each level as `(object, name, ebx, edi)` until `al` is
non-zero; at the list's end with no answer it zeroes both again. `eax`
starts as the body with `al` cleared; `ecx` is the slot, `edx` the name.

### Slice 3 bodies: Import handlers table

**Import handlers.** The slot `+4c` shape, `ret 10`. An unknown name
returns `al` 0 and stores nothing; an answered one stores the `f32`
value and the byte "value > 0" (1 where noted).

| Body | Keeps | Answers |
|---|---|---|
| `bcc20` base (`base.rs`) | `esi` the body and `bl` across every call; the value at `E-4` (the saved `ecx` slot), the result byte at `E+4` | constants, body `+ec`, `+f0`, `+f4`, `+f8`, `+f0 − 1`, the `+10a` bit 2 flag, the model variant fraction, a hash of the handle, a facing angle (`b8bd0`, x87 `fpatan`, `bff60`), the world's `[4de2f4]+70..7c`, and callouts `bca70`, `bcbb0`, `bcad0`, `bcb10`, `bcb60`, `c18e0`, `1588b0`; the component names go to `bc970`, and its answer in `bl` makes active 1. Most values clamp to `[xmm2, 1]`; after `bca70`, `1588b0` and `bc970` retail reads `xmm2` as the callee left it |
| `c6b90` unit (`unit.rs`) | `ebx` saved, `bl` 1 across `d1210`, `d1410` | body fields and flags of the unit, `d1210`, `d1410` and `1 − d1410`; two names ask the held weapon (`+212` index into `+218`) through `ff5f0` with a weapon name in `esi` (saved), returning its `al` |
| `d1210` (`unit.rs`) | a leaf, no stack | 0 unless body `+334 > 0`; with `+338` zero, `+334 /` definition `+1d4` (1.0 when that is not above); else `(+334 − +338) /` definition `+1dc`, or 0 |
| `d1410` (`unit.rs`) | `esi` the animation state (saved) | 1.0 for none; by the state's kind `+7c`: `1c6e30(esi)`, or `1 −` it, or 0 or 1 when the state is empty |
| `de3b0` biped (`biped.rs`) | `esi` the name (saved) | a state byte `+3dc` equal to 2, a flag byte `+39d`, and the length of body `+88` (x87 `fsqrt`) over definition `+2dc` |
| `f6d10` vehicle (`vehicle.rs`) | `esi` the body, `ebp` the definition (the tag of body `+0`), `edi` the name (a binary-search tree of signed compares; `sub edi` chains leave it zero), `bl` set when the name is active at any value; scratch floats at `E-18`..`E-1` and in the object and name argument slots once read | ratios `f78b0 (x, y) -> xmm0` of a speed-like quantity (`24590`, a vector length, x87 dot products of body vectors, `11d580` differences) over a definition limit (`f78e0` by index, or a field); flag tests `f5d70`, `e6fe0`, `f8070`; `d1210`. Clamped to `[0, 1]` at `f785a`; an unknown name returns `al` 0 and stores nothing |
| `ff5f0` weapon (`weapon.rs`) | `ebp` the body, `edi` the object, `esi` the name or a trigger cursor, `ebx` the definition or a state cursor; the definition over the saved `ecx` slot | barrel, trigger, magazine and flag values; `102070`, `10cf50`, `162b10`, `162b70`. Answered names end at `ffc28` with `eax` 1 |
| `fbe70` projectile (`projectile.rs`) | `bl` 1 | flags `+12c` bits 1, 3, 8, body `+158`, `+168` over definition `+e0`, and `(+168 −` definition `+18c) ×` definition `+194` clamped (1 with flag bit 8, or definition `+17c` or `+180` zero) |
| `106d70` device (`device.rs`) | `bl` 1, `xmm1` 0, `xmm2` 1 | the power and position values (`+134`, `+140`), a timer over definition `+10c`, the powered test through the device groups at `[4e0328]`, and two x87 ratios |
| `11bd60` machine and crate (`machine.rs`) | `ebx` saved, `bl` 0; `esi` the component record | the component's import `1d3550 (record: esi, name: ecx, value: edx, active: stack 0) -> al`, `ret 4`; `al` 1 when it answered |

### Slice 3 bodies: Callees and not yet a leaf

**Callees.** Slice 3's guest callouts and their cleanup: `10aac0`
`ret 0c`; `13bb90`, `f78b0` `ret 8`; `b8bd0`, `bff60`, `1588b0`,
`bc970`, `102070`, `162b70`, `1d3550`, `f78e0`, `f8070` `ret 4`;
`131c20`, `bca70`, `bcbb0`, `bcad0`, `bcb60`, `bcb10`, `c18e0`,
`10cf50`, `162b10`, `1c6e30`, `f5d70`, `e6fe0`, `24590`, `11d580` `ret`.

**Not yet a leaf.** The evaluator becomes a leaf kernel (section 5 of
the design) only when every callee it can reach is Rust. Function
lookups `13bb90`, the scripted override `10aac0`, the colour
interpolation `131c20`, and the type handlers' callees above are still
guest code, so every slice 3 body is a segment.

### Slice 4 bodies: shared rules detail

same pool). Each body file's header records its frame and the order of
its calls; this section records what a caller or a later slice relies
on.

**Shared rules.** The bodies keep all eight integer registers, the `xmm`
registers and MXCSR as retail leaves them at every call and return
(scalar SSE through `sim_objects::sse`). The arithmetic flags are
reproduced at every return from the last instruction that sets them
(`add esp` at the end of `bdef0` and `bc5e0`, a `test` or `cmp`
elsewhere, the callee's after a call that ends the function): `b7430`,
`b7740`, `b9dd0`, `b9fc0` and `ba1d0` are called from all over the
image. `push`/`pop` pairs around a lookup (`b9dd0`'s `edi`, `b7740`'s
`ebp`) are made, so the dead stack holds what retail leaves there.

### Slice 4 bodies: node-matrix rows detail

**Node-matrix rows** (`xform.rs`). `bdef0`, `b9dd0` and `b9fc0` inline
one sequence: each component `(m_z·z + m_y·y) + m_x·x (+ t)` through a
`34`-byte node matrix (`+0` scale, `+4` forward, `+10` left, `+1c` up,
`+28` translation), `x`, `y`, `z` in `xmm0` to `xmm2`, the row in `xmm3`
and `xmm4`. A point is first multiplied by the matrix scale when
`ucomiss scale, [45dbc0]` (the image's 1.0) is not equal; `lahf` then
leaves `ah` changed in `eax` where retail's does. `sim_objects::bounds`
is the same arithmetic as a pure kernel (exact or declining, through
`sim_core::det::Fp`), for a speculated pass.

### Slice 4 bodies: bounds bdef0

**Bounds `bdef0`** (`bounds.rs`). Stores the first sphere (`+30`,
`+3c`) and the second (`+50`, `+5c`) on every path except a crate's
component answer, which supplies the first; then the registered sphere
(`+40`, `+4c`) only when it stores (not in the partition, or moved).
Moved is `comiss` of the squared distance against `[45e02c]` (`ja`); a
changed radius is `ucomiss` (unordered counts as changed). Callees:
`1d44f0` (`ret 0c`), `b87b0` (`ret 4`), `b8600` (`ret 8`).

### Slice 4 bodies: node matrices bd090 detail

**Node matrices `bd090`** (`pose.rs`). The port
`sim_physics::node_matrices` run as a body (design section 6: phase 1
owns the arithmetic, this phase the orchestration). Its sixteen sites
are the body's; the after-root dispatcher `1090d0` fuses. The pose
matrix `1421f0`, the composition `142a60` and the root transform
`bdc40` run inline through `sim_native`, so they are no callouts: the
harness steps them on the lifted side as part of the tree (`INLINED`).
The skeleton walk from node 1 is phase 1b's: `sim_anim::build_matrices`
through `engine::sim_anim::skeleton` (`impl.sim.native.anim.skeleton`),
with the port's per-node loop as the fallback when it declines
([skeleton-walk.md](../skeleton-walk.md)). The frame and the callouts here
are the same either way.

### Slice 4 bodies: bef30, be1d0, bc5e0, setters and getters

**Attachment refresh `bef30`** (`post.rs`). Loops over siblings by
storing the next sibling over its own object argument (`[esp+14]`) and
jumping back, and recurses into the first child through its own entry
(fused). Callees: `b9ce0` (`ecx` object, `ret`), `c3220` (`eax` light,
`ret`), `c2d00` (`ret 4`), `c1720` (`ret 0c`).

**Node-matrix notify `be1d0`** (`post.rs`). `push ecx` makes the count
local `b8c00` fills; `109140` gets `(count, matrices)` with the object
in `ebx`.

**Component transform `bc5e0`** (`post.rs`). Callees: `1d1c10` (`esi`
record, `ret`), `1d1ca0` (`edi` record, `ret`), `1d4360` (`edi` record,
`ret 4`), `b91d0` (`eax`, `esi`, `edi`; `ret`), `b7430` and `b7740`
(bodies, fused), `1d09d0` and `1d0ad0` (`eax` 0, `edx` record, `ecx`
out, kept: `bc5e0` passes it on to `b7740`; `ret`), `1d96d0` (`eax` handle, `ret`). The network mode test is a
byte `cmp al, 4`.

**Setters and getters** (`transform.rs`). `b7430` keeps a one-dword
local (`push ecx`) whose byte 3 is the result; its position check is six
`comiss` against `[45e4ac]` and `[45dffc]`. Callees: `b87b0`, `b58c0`
(`ret 8`), `1c4a80` (`ret 8`), `bd020` (`ret 4`), `b8600`. `b7740`
calls `b58c0`; `ba1d0` calls `1d09d0` and `1d0ad0`. `b9dd0` and `b9fc0`
call nothing.

### Not verified for slice 4: detail

- Captures and run parity (the slice's testable result): registers and
  RAM at the n-th call of `bc820` on Waterworks and a campaign level,
  with `impl.sim.native.objects` on and off. The harness covers
  synthetic pools.
- The harness compares `xmm` registers at callouts for the slice 4 roots
  except `bd090`, whose port was differential-tested without them
  (`tests/sim_physics_differential.rs`).
- Ret sizes of `b9ce0`, `c3220`, `1d1ca0`, `b91d0`, `1d09d0`, `1d0ad0`,
  `1d96d0`, `e1670`, `bf5a0`, `bfc90`, `bff10`, `bfe80`, `bfed0`,
  `bfe20`, `b6780`, `1c6440`, `a98b0`, `a9940` are read from their
  first `ret`, the rest likewise and checked against their arguments.

### Slice 8 body: Frame slots

**Frame** (`engine::sim_objects::damage`). `sub esp, 70`, then `ebx`,
`ebp`, `esi`; `edi` is pushed only once the damage information `d5b60`
returns is non-zero, so the early exit (`d664f`) pops three and returns
`al` = `dl` (0). With `S` the `esp` after the four pushes: `S+13` the
changed byte (the result), `S+14` the actor's record or 0, `S+18` the
damage information, `S+1c` the section cursor and then the shield
ceiling, `S+20` the header offset and then float temporaries, `S+24`
the section count and temporaries, `S+4c` a 13-dword record zeroed with
`rep stosd` for `da110`, `S+84` the object. `ebp` holds the body from
the entry to the return and `edi` the object from `d5fe9`. The body is
the retail code block by block, keyed by retail address, so the lifted
original can take over at any of its 26 return sites and the reverse.

### Slice 8 body: Floating point detail

**Floating point.** `xmm3` holds the image's 1.0 from `d6095` across the
calls to `14de90`, `13a6e8` and `a7a30`, and `xmm1` holds 0 from `d6159`
into the regeneration and decay blocks; the body keeps every XMM
register as retail leaves it at each call. The section decay rounds
with `fld` / `fistp` (`d6551`, `d6555`) under the guest's x87 control
word, and leaves the x87 state as `fistp` does (FIP `d6555`, its FOP
and data pointer).

### Slice 8 body: Callees with ret sizes

**Callees**, every one a callout, with the bytes `ret` pops: `d5b60`
the damage information (`ecx` the object; 0), `d9f70` (4), `da110`
(`14`), `dbc80` (`c`) and `d6bc0` (4) per section, `1e4a10` the actor
(0), `d66d0` the damage owner (8) and `d6800` the depletion (`10`) for a
pending kill (flags bit 13), `dbfb0` pending damage (`c`), `1588b0` a
player's recharge rate (4), `14de90` (0) and `13a6e8` (4) the
overshield's drain, `a7a30` replication (4), `1e9720` a team's rate
(0), `ba7f0` (`10`) and `176780` (`10`) the shield effects, `138880`
whether stun timers hold (0), and `d8b50` the recent-damage accumulators
(4).

### Slice 8 body: event field lists

- `Damage` (`sim_objects::damage::damage`, at `DAMAGE_REPORT`): the
  report (`guest_layout::DamageReport`: damage code, flags, effect tag,
  owner player, object and team, direction, origin, and body and shield
  damage, L), the victim's handle, type, controlling player and position,
  and the game tick. A report that does not read records nothing.
- `Kill` (`sim_objects::damage::kill`, at `PLAYER_KILLED`): the attacker
  and victim players (`[esp+4]`, `[esp+8]`), the damage code (`[esp+10]`'s
  low byte, L), each player's unit and its position, and the tick.

### Slice 11 bodies: intro detail

order of its calls; this section records what a caller or a later slice
relies on.

### Slice 11 bodies: Shared rules detail

**Shared rules.** The bodies keep the `xmm` registers, MXCSR and x87
state as retail leaves them, as well as the eight integer registers:
float arithmetic goes through `sim_objects::sse` (the `simd` model, or
the host's binary32 operation when MXCSR rounds to nearest with PE
sticky and both operands are ordinary), x87 sequences through
`x87::abi` on the guest's x87 state. A body stores into its caller's
argument slots where retail does: `f8de0` replaces the low byte of its
object argument with the mover flag and pushes the whole dword,
`118b80` replaces it with `118b10`'s `al`, `115a40` stores the record
base over it. Flags are reproduced at every transfer from the last
instruction that sets them; where that is a shift by more than one, OF
and AF are left undefined, as the IR does.

### Slice 11 bodies: per-body walkthroughs and callee cleanup

**Item `10b4d0`.** The early-mover follow for items woken by a mover
(body `+c1` bit 0), the fall and damping of body `+94..+9c`, the wake
for post (`bba20`, fused), and the garbage timer at body `+12e`: when it
reaches 0 outside network mode 4 the item deletes itself (`bc190`, an
effect when definition `+110` is set, `176780`, then `OBJECT_DELETE`
`b8540` on its own handle). The deletion is a callout in retail order;
the walk's slot scan reads the freed slot afterwards as retail does.
Every object then calls `10b850(object)`, which reads the early mover's
record ("Who reads it, and when" above).

**Scenery `10a2f0`.** Advances the animation block at body `+12a`
(`1cb710`) and, when the block's animation has more than one frame
(`1daea0`'s answer `+14`), wakes the scenery for the post walk.

**Device `106790`, machine `1152e0`, `115a40`.** The device update
moves the power (`+140`) and position (`+134`) toward their targets or
groups (`[4e0328]`, `c` bytes a group) and wakes the machine through
`bd020` and `115b10`; the machine update runs after it for machines
(type ancestry `obje`, `devi`, `mach`): a cycling machine's position
(x87 rate, `fstp` spill), a door's blocking test (the spatial visit
`bb050(1, 3, ..)` into a `10`-entry buffer, then `b8d30`/`b9fc0` per
result) and its cluster bit words. `115a40` is a leaf: exit `eax`,
`ecx` (the range end), `edx` and the flags as retail's.

**Crate `11bce0`.** With body `+c0` bit 12 and a physics component,
`1d24a0(component record, 1.0)`; unless body `+c1` bit 0, the tick
count into body `+bc`.

**Creature `118700`, `118b80`.** The AI calls stay callouts in retail
order (`1e5bb0` with 21 arguments, `1e6360`, `1e5af0`, `1e6120`,
`1e55d0`; after physics `1ec500`, `1e68a0`, `1e5a60`). `118700` keeps
`fld`/`fistp` for the timer at body `+204` on the guest's x87 state.

**Projectile `f8de0`.** Asks the waking mover for its displacement
(`109a00`, `ebx` = the velocity copy), runs the step `f8eb0` (a body,
so `drive` fuses the call), and adds the displacement to body `+88`
when `109a00` answered.

**Projectile step `f8eb0`** (`projectile_step.rs`). `1247` bytes, 39
calls. The body is retail's basic blocks keyed by address, one
statement per instruction, so every instruction that sets flags, an
`xmm` register, MXCSR or the x87 state is reproduced and the lifted
original can take over at any return site. Frame `sub esp, 118` and
four pushes; the file header records the slots. In retail order: the
attachment (`fd410`, `17c880`, `16ba40`, `fd460`) unless body `+12c`
bit 1; the timers at `+158..+164` and `+1aa`; then sub-steps while the
tick's time lasts (at most ten hits): the speed through x87 `fsqrt`;
homing on the target `+144` (`badc0`, `1e9700`, `f87f0`, `30bf0`, an
x87 `fsin`/`fcos` rotation, `b75a0`); drag (`24590`, `faa30`);
gravity through `[51e9c4]`; the collision `fc030` and on a hit
`1ca9f0`, `faa60`, `1ca690`; the flyby test per local player
(`1897c0`); the orientation from the velocity (`30bf0`, `11d000`) or
the spin; the new transform and velocity (`b75a0`, `b7740`, `1c4b00`,
and `b9b90`, `b7360`, `bba20` when it moved); the attachment's update
(`17bbd0`). After the loop, by the state word `+130`: detonation
`fc330` or deletion `OBJECT_DELETE` `b8540` (a callout in retail
order), or in network mode 4 the attachment refresh (`b9d20`, `bef30`,
`b8b70`).

Float registers across calls: `24590` returns the length in `st(0)`,
which the `fadd` after it consumes; `30bf0` and `1e9700` return in
`xmm0`, and `xmm1` holds a value across `1e9700` (`mulss xmm0, xmm1`
after it); `faa30` is entered with `xmm0` set. Callee cleanup: `fd410`,
`17c880`, `16ba40`, `30bf0`, `24590`, `faa30`, `11d000`, `b9d20`,
`b8b70` `ret`; `fd460`, `badc0`, `1e9700` (through its tail jump to
`1e96a0`), `f87f0`, `fc030`, `b7740`, `b9b90`, `b8540` `ret 4`;
`17bbd0`, `1ca9f0`, `1c4b00` `ret 8`; `b75a0`, `fc330` `ret 0c`;
`1897c0` `ret 10`; `faa60`, `1ca690`, `bef30` `ret 14`.

### Not verified for slice 11: detail

- Captures from campaign (the slice's testable result): registers and
  RAM at the n-th call of each slice 11 point, on a level with machines
  and doors, creatures, scenery animations, crates, and items that reach
  their garbage timer, compared with `impl.sim.native.objects` off. The
  harness covers synthetic pools only.
- The harness's stubs answer in a float register only for `30bf0`,
  `1e9700` (`xmm0`) and `24590` (`st(0)`). Other callees that return a
  float in a register leave it as the caller had it, so a body's reading
  of their answer is checked only as far as that value goes.
- The projectile step's harness pool is synthetic: the stub for `f87f0`
  (the target's point) does not fill its out buffer, and the stub for
  `fc030` fills only the collision record's fraction (`+4`), so the
  homing and the rest of the hit arithmetic run on what the stack held.
  A capture from a level with homing projectiles and ricochets covers
  those values.
- Callee `ret` sizes for `1d24a0` and `1c6620` are read from their tails
  (`1c6620` jumps to `1c6560`, `ret 4`); the rest from their first `ret`.

### Slice 5 bodies: Shared rules detail

**Shared rules.** Integer registers, `esp` and the frame slots match
retail at every transfer. The arithmetic flags are undefined at every
call and return: the liveness table shows no flag reader at any slice 5
entry or site, nor after any call of these entries from `bef30`. Scalar
SSE goes through `sim_objects::sse`; the colour scaling in `c0b00` runs
on the guest's x87 state through `x87::abi`, so its last-instruction
pointer and data pointer are retail's (`c0c48` and the blue channel).
The light lists themselves stay guest: `1cac60` adds, `1cae40` removes,
`16ba40` frees the datum, `16bc00` is the datum iterator.

### Slice 5 bodies: Walk c1400 detail

linked light whose owner or marker parent is a live object with POST set
is removed (`1cae40`, when active) and relinked (`c2d00`); a timed light
past its definition's lifetime expires (`c3260(handle, 1)`). The array
header, high-water mark and bitmap are re-read after each light, so
lights a callee creates or deletes are seen as retail sees them.

### Slice 5 bodies: Record, Phase, Link/unlink/expire detail

**Record `c0c80`.** The body is the per-function port of
`LIGHT_RECORD_UPDATE` (kept as the oracle `oracle.light_record` in
`sim_physics::ORACLES`) with its transfers returned to `drive`, so its
calls to `c0b00`, `baf80`, `bab40`, `c2d00` and `c3220` are fused. It
links a light that turns on (with no owner, or when `b9ce0` answers no
and `b9d20` yes) only while the object globals' first byte
`[[4de2f4]]` is set, and unlinks a linked light that turns off. The body
takes every light through the same arithmetic, so the old placed-light
bypass has no switch of its own.

**Phase `c0b00`.** The phase is game time `+8` × `+4`, or a per-light
offset `((handle × 1387) & 7fffffff) mod (ticks per second × 3c)` added
to game time `+8`, × `+4`. A zero ticks-per-second is the guest's divide
error, reported as a fault at the `idiv` (`c0b3b`). Then up to three
function blocks: brightness (`13b390`, clamped and mapped unless the
function's flags byte `+1` has a bit of `f0` set), colour (`13b390`, then
`13bc00`, whose packed RGB scales the three channels), and two
`13bb90` values into `[out]` and `[out + 4]`. The colour bytes are stored
over the input's argument slot, high first, as retail does.

**Link `c2d00`, unlink `c3220`, expire `c3260`.** `c2d00` computes the
world position from the owner's node matrices or marker (`b8c40`,
`16640f`, `b8d30`) or a marker parent's node matrix (`142570`, then the
two local directions rotated by its rows in retail's operand order),
finds the cluster (`16a8e0`, `11bed0`, `ba300`) and adds the light
(`1cac60`), then sets flags bit 3. `c3220` removes an active light and
clears bit 3. `c3260` removes a linked, active light, updates the marker
parent's body flags `+4` bit 6 when `notify` is set (scanning the other
lights with `16bc00`), and frees the light (`16ba40`).

### Slice 5 bodies: Callee ret sizes

**Callee `ret` sizes.** None: `16bc00`, `bad50`, `17ca10`, `131c20`,
`b9ce0`, `b9d20`, `b8c40`, `c28b0`, `11bed0`, `ba300`, `16ba40`. `4`:
`13bc00`, `142570`. `8`: `1cae40`, `131d60`, `13b390`, `13bb90`,
`c0840`. `c`: `b8d30`. `10`: `16640f`. `1c`: `16a8e0`. `24`: `1cac60`.

### Not verified for slice 5: detail

- Captures from campaign and multiplayer: registers and RAM at the n-th
  call of `c0c80` and `c2d00` on a level with lights that have a function
  source (owner node matrices and markers) and marker-parent lights,
  compared with `impl.sim.native.objects` off. The synthetic record fixture
  exercises a function source through both colour calls. Captures cover the
  owner-marker path (`b8c40`, `16640f`, `b8d30`) with real attachments.
- The harness answers `16bc00` from the synthetic bitmap; the iterator's
  own behaviour is not compared.

### Slice 6a bodies: Transfers detail

**Transfers.** The eight integer registers, the `xmm` registers, MXCSR
and the x87 state are retail's at every call and return. Flags are left
undefined at every transfer of `c60c0`, `cba50` and `c85c0`: no caller
and no callee reads them before writing them (`scripts/sim-liveness.py`).
`c6b30` and `c7840` reproduce them, as the ports they come from do. x87
sequences (the `_ftol2` arguments, the six "moved" tests, the clamp's
`fpatan`, `fsqrt`, `fsin` and `fcos`) run through `x87::abi` on the
guest's x87 state, with the last instruction and data pointers retail's
last x87 instruction leaves.

### Slice 6a bodies: Shell c6b30 detail

**Shell `c6b30`.** Calls `c49b0`, `c58f0` and `c60c0` with the unit on
the stack, then `c5eb0`, `c6740`, `114c60`, `c6990`, `c6810` and `c50a0`
with it in `eax`; `al` is the OR of the first five answers. `c60c0`,
`c49b0`, `c58f0` and `c5eb0` ("Slice 6b bodies") and slice 6c's `c6740`,
`114c60`, `c6990`, `c6810` and `c50a0` are bodies (fused). The port it
comes from (`sim_physics::objects::unit`) is an oracle.

### Slice 6a bodies: Movement, Clamp, Idle test detail

**Movement `c60c0`.** Returns 0 at once when definition `+bc` bit 10 or
body `+10a` bit 2 is set. Otherwise it moves the aim (body `+168`)
toward the desired aim (`+15c`) and the look (`+18c`) toward the desired
look (`+180`). With both of a pair's rates zero (definition `+144`,
`+148`; `+150`, `+154`; times `+14c` when body `+1bc` is 1) the
direction is copied and clamped (`cba50`, fused; for the aim only when
`c4970` answers) and its velocity (`+174`, `+198`) set from `[4687a4]`.
Otherwise `c7840` turns it in the unit's frame (`b9fc0`) when
the limits record's (body `+ [body+33e]`) bit 5 (aim) or 6 (look) is
set, or `11f0d0` turns it in the world. The aim's two rate fractions
(`11ce20`, `1201a0`, clamped to 0..1) become the bytes at `+243` and
`+1f8` through `_ftol2` (`322b0c`), which pops the x87 stack. When any
of the six components moved by more than `[45dbdc]` the unit is woken
for the post walk (`bba20`, fused) and `al` is 1.

**Clamp `cba50`.** Returns 0 when the limits record's bit for the
direction (5, or 6 for the look) is clear. Otherwise it takes the unit's
frame (`b9fc0`; for the look re-based on the aim, normalised by `30bf0`
and `11d000`), builds a matrix from it (`1420f0`), takes the direction's
yaw and pitch in it and clamps them to the record's ranges (`+4..+10`,
or `+14..+20` for the look). When either clamped it writes the direction
back from the clamped angles. The x87 compare of the yaw against its
minimum leaves the x87 status and pointers that a later x87 user sees.

**Idle test `c85c0`.** 0 when the unit has a parent (body `+14`), when
bit 26 or 27 of its record word (body `+ [body+346] + 4`) is set, or when
any of seven `e6fe0(unit, n)` tests answers; then 0 when a held weapon
(the two index bytes at body `+212` into the handles at `+218`) answers
`100880`. Else 1. The result is also stored in the top byte of the
caller's `ecx`, saved and restored in the frame, so `ecx` returns with
it.

### Slice 6a bodies: Aim c7840 detail

**Aim `c7840`.** `sim_physics::unit_aim::step` computes joint-space
direction and velocity with scalar SSE and x87. It returns a call or
return to its driver. The object update can fuse the step when it owns
aim. With the animation subsystem selected, the object update emits the
call and `sim_anim::unit_aim` plans and commits the segment. Both
drivers use the guest frame across all nine return sites.

### Not verified for slice 6a: detail

- Captures from Waterworks and a campaign level: registers and RAM at
  the n-th call of `c6b30`, `c60c0`, `cba50`, `c85c0` and `c7840`,
  compared with `impl.sim.native.objects` off. The harness covers
  synthetic pools only.
- The harness compares the eight integer registers, the flags, memory,
  `xmm` and the x87 state at callouts. `_ftol2`'s stub pops the x87
  stack without storing the truncation, so the `+243` and `+1f8` bytes
  are checked only against stub answers.
- That the x87 crate's transcendentals do not depend on the rounding
  mode is read from its source, not checked in a test.

### Slice 6b bodies: Transfers and Constants detail

**Transfers.** The eight integer registers, the `xmm` registers, MXCSR
and the x87 state are retail's at every call and return, and every store
retail makes is made, stack stores included (the `push 1` arguments of
`10f720`, the record `189210` takes, the flag word `c58f0` stores as a
word and reads back as a dword with the stale upper half). Flags are
left undefined at every transfer: `scripts/sim-liveness.py` shows no
caller and no callee of the four reading a flag before writing it (and
`d1080` reads only DF, which no body touches). x87: `c49b0`'s `fld;
fistp` (`c4c08`, `c4c0c`) and `ce0c0`'s three `fld; fabs; fstp`
(`ce254`..`ce2ec`) run through `x87::abi` on the guest's x87 state with
the pointers the last one leaves.

**Constants.** String IDs (`c4cf3`, `c4cfb`, `c4dfb`, `c4e03`, `c5c03`)
and the `0.5` pushed at `c59e1` and `c59fa` are read from the
instructions that carry them in the loaded image. Float constants are
read from their image addresses.

### Slice 6b bodies: Control, State, Seats, Callees detail

**Control `c49b0`.** The desired aim and look (`+15c`, `+180`) come from
`d0080`'s facing when body `+134` bit 21 is set, otherwise (bit 1 clear)
from the parent's seat node or the unit's own `+70`; then the timed bits
(`+1ec`, `+1f0`, `+148` bit 16 toggled every `[45dc20]` × ticks a second
ticks, `+1c0`), the two linked objects at `+248` and `+24c`, and the
`+334`/`+338` pair. The `idiv` by that tick count is the guest's divide
error when the count is 0; the body faults at `c4c19` as the lifted code
does.

**State `c58f0`.** Timers `+1f5`, `+1fa`, `+1fb`; unit states through
`e68c0`; held weapons' time stamps and states; `+23c` through `110ab0`
and `cbec0`; the idle test `c85c0` (a slice 6a body, fused) choosing
`+240` and the name `189210` plays; per held weapon `100430` with a flag
word from body `+148`; then `d1540`.

**Seats `c5eb0` and refresh `ce0c0`.** `c5eb0` calls `d0c10` and
`ce0c0`, then moves each of the definition's powered seats (`+1b8`
records at `+1bc`) toward 1 or 0 by one tick over the seat's time, into
body `+25c + 4n`. Its result is the top byte of the `ecx` it pushed, so
`ecx` returns with it, and the Halo 1 drive model's `before` hook on
`ce0c0` sets that byte through the return address `c5f04`. `ce0c0`
clears body `+134` bits 25 and 26, asks `d1080` for the control frame,
and stores the position's change (`+270..+284`), its changes in the
basis (`+294..+29c`) and their blend (`+288..+290`); bits 25 and 26 are
set when an absolute change passes a threshold. The drive model's hook
makes `UNIT_CONTROL_REFRESH` a guarded body: `c5eb0` reaches it through
the runtime, which runs the hook, then the body.

**Callees**, every one a callout except `c85c0`, `ce0c0` and slice 7a's
`e68c0` and `e4050` (bodies, fused), with the bytes `ret` pops: `d0d20`,
`d0080`, `10f5f0`, `c7070`, `1143d0`, `f42f0`, `cc380`, `113da0`,
`113df0`, `cbec0`, `189210`, `d0c10`, `138880` (0); `10f720`, `113d20`,
`113d60`, `110ab0`, `d1540` (4); `d1080` (8); `100430` (`c`).

### Not verified for slice 6b: detail

- Captures from Waterworks and a campaign level with vehicles and
  turrets (the slice's testable result): registers and RAM at the n-th
  call of each of the four, compared with `impl.sim.native.objects`
  off. The harness covers synthetic pools with stubbed callees.
- The harness dresses only units whose linked objects are pool objects;
  a linked object of another type, and `10f5f0`'s names beyond the two
  the bodies test for, are read only from the code.
- The Halo 1 drive model's hook with the body guarded is read from the
  installer (`hooks::install::guard_shared_bodies`), not run.

### Slice 9 body: One body, three files detail

callout like every root. Field names are ours and **L**.

**One body, three files** (`engine::sim_objects::weapon`). A body is
one row in `BODIES`; its arms are split by state group and the
dispatcher in `weapon/mod.rs` picks the file by retail address:
`state.rs` (`fdfb0..fe5ff`: the frame, the owner, the animation block,
the timers, the charge toward body `+190`, the heat at `+180..+188`,
the effect at `+244`, the definition's `+2c0` entries), `triggers.rs`
(`fe5ff..fefee`: the trigger loop, states 0 to 7 through `ff57c`,
`ff59c`, `ff5b4`, `ff5c4`) and `barrels.rs` (`fefee..ff576`: the barrel
loop through `ff5d4`, heat, spin and the return). Each file's header
records its fields; `weapon/mod.rs` records the frame and the registers
kept across calls. The jump tables are read as retail-address arrays
(their targets, not their bytes).

### Slice 9 body: Frame, Floating point, Callees

**Frame.** `sub esp, 38`, then `ebx`, `ebp`, `esi`, `edi`. With `S` the
`esp` after the pushes: `[S+13]` the result, `[S+24]` the header offset,
`[S+28]` the trigger or barrel index, `[S+2c]` the definition, `[S+30]`
the owner's handle, `[S+34]` the body pointer as read at entry,
`[S+38]` `d1490`'s answer, the object at `[S+4c]`. The barrel loop
indexes the body through `[S+34]` even after a callee moved the body,
as retail does; the state group and the trigger loop re-read it through
the header (`[S+24]`) where retail does.

**Floating point.** Scalar SSE through `sim_objects::sse`; no XMM value
is live across a call. Two x87 sequences run on the guest's x87 state
through `x87::abi`: the heat value `fld; fadd; fmul` left in `st(0)`
for `_ftol2` `322b0c` (FIP `ff404`), and the spin clamps `fld qword;
fld dword; fcomip; fstp st(0); fld; fstp dword` (the stores at
`ff46b` and `ff508`, into `[esi+20]`). `fcomip` sets ZF,
PF and CF and clears OF, SF and AF, as the `jbe` after it reads.

**Callees**, every one a callout, with the bytes `ret` pops and the
register arguments retail passes: `101690` (4), `d1490` (0; `eax`,
`esi`, answers in `xmm0`), `101fb0` (0; `eax`), `badc0` (4; `eax`),
`105e80` (4), `1daea0` (4; `edx`), `1cb710` (`14`), `105740` (0; `eax`),
`105800` (0; `ecx`), `10d4e0` (0; `edx`), `105840` (0; `ecx`), `1039a0`
(`c`; `ecx`), `101f20` (0; `eax`), `c86e0` (4), `105fa0` (4), `1685a6`
(8; `ecx`), `103a60` (0; `eax`), `177610` (0; `eax`), `177260` (4;
`edi`), `fdd50` (8); in the trigger loop `102540` (`c`), `102f70` (0;
`eax`, `cx`), `106280` (8; `eax`), `103c40` (4; `eax`), `103a90` (4;
`eax`), `103ce0` (4; `ecx`), `1027b0` (`c`; `ecx`), `103b10`, `103bd0`
and `103b70` (4; `eax`), `102d60` (0; `eax`, `ecx`), `102e10` and
`102f10` (0; `eax`, `edx`), `1061c0` (8; `eax`); in the barrel loop
`1023d0` (4), `103e60` (4; `eax`), `102100` (`c`; `eax`), `102fb0` (8),
`103f60` (8), `103dd0` (4; `eax`), `ba7f0` (`10`) and `322b0c` (0;
`st(0)`, answers in `edx:eax`). None is a body yet.

### Not verified for slice 9: detail

- Captures (the slice's testable result): registers and RAM at the n-th
  call of `WEAPON_UPDATE` from a Waterworks match (charging and
  overheating weapons, dual wielding, a vehicle turret) and from a
  campaign level, compared with `impl.sim.native.objects` off. The
  harness covers synthetic pools only.
- The x87 state at callouts: the harness compares integer registers,
  flags, XMM and memory, not `st(0)`. `_ftol2` takes its argument
  there, so a wrong heat value shows only in what `322b0c` answers into
  body `+175` in a real run.
- Callee `ret` sizes are read from the pushes at each call site and the
  callee's first `ret`.
- The register-liveness tables below do not cover this body's sites
  (`scripts/sim-liveness.py` lists slices 1 to 5); the body keeps all
  eight registers exact, so it does not rely on them.

### Slice 10 body: Frame, Flags and floats, type physics, Callees

**Frame.** The stack probe is a callout, as in `109580`; the body
resumes at `efdea` with `esp` 1984 bytes down, then pushes `ebx`,
`ebp`, `esi`, `edi`. All 39 return sites are at that `esp`. The object
argument is read back from the frame (`[esp+1998]`) where `ebp` was
reused for the matg record (`[[4e034c]+144]`), so the lifted code and
the body can take over from each other at any site.

**Flags and floats.** The body reproduces the arithmetic flags at every
transfer from the last instruction that sets them (rule 3), as the
slice 11 bodies do, through `sse.rs` and `alu.rs`. The scalar SSE work
(the component's rate, the mean of the contact records' floats into
body `+3fc`, the drag and pull toward the structure's point, the timer
at body `+2c4`) runs on the guest's `xmm` registers, keeping the upper
lanes and MXCSR as retail does. Three x87 sequences run on the guest's
x87 state with their last-instruction pointers: the two loads before
`372d48` (`fld qword [45e610]`, `fld dword [game time +4]`; the callee
returns in `st(0)`, stored by `fstp` at `f01f2`), `fsqrt` at `f02f5`,
and the `fabs`/`fcomip` test at `f04e0`..`f04f2`.

**The type physics.** Definition `+1f0` (`i16`) selects, through the
seven-entry table at `f069c` (read from the image), `f1cd0`
(`VEHICLE_TREAD_UPDATE`, `ebx` the object), `f1f80`
(`VEHICLE_WHEEL_UPDATE`), nothing, `f2360`, `f3010` (with `[S+14]`),
`f4800` or `2056e0` (`VEHICLE_CONTACT_SOLVE`); above 6 none. These stay
callouts, so the Halo 1 drive model's hooks on them run as before.

**Callees**, every one a callout except the leaves, `bba20`, the unit
action update `e6830` and the unit animation state `111650` (slice 6c, fused), with the
bytes `ret` pops: `320560` the probe; `1d2460`, `efc30`, `ef870`,
`efb60`, `205510`, `1cfb90`, `ba3d0` (a tail jump into `b7360`),
`f06c0`, `f6010`, `cc380`, `372d48`, `113e40`, `b9ef0` (0); `b9b90`,
`ba160`, `ba1d0`, `f1cd0`, `f6bd0`, `b77d0`, `d6660`, `e6830` (4);
`1d24a0`, `ef4f0`, `f6670`, `205be0`, `f1f80`, `f2360`, `f4800` (8); `f3010` (`c`); `ba7f0` (`10`); `2056e0` (`14`); `d7b80`
(`18`).

### Slice 10 body: Not ported detail

**Not ported.** The rest of the tree stays guest: the type physics
`f1cd0`, `f1f80`, `f2360`, `f3010` (162 samples on Waterworks),
`f4800`, `2056e0`; the contact gather `205be0`; `ef4f0`, `ef870`,
`efb60`, `f6010`, `f6670`, `f6bd0`; and `eec90` (175 samples), which
`efde0` does not call directly. Each is a later lane's body; the
segment keeps every frame slot and register they read.

### Not verified for slice 10: detail

- Captures from a match with vehicles (the slice's testable result):
  registers and RAM at the n-th call of `VEHICLE_UPDATE`, one per
  vehicle type the table dispatches (tread, wheel, the flier types,
  turrets), on the ground and woken by an early mover, compared with
  `impl.sim.native.objects` off. The harness covers synthetic pools,
  with stubbed callees.
- The harness does not compare x87 state at callouts. `372d48` takes
  its two operands on the x87 stack and returns in `st(0)`; the body's
  x87 sequences are checked only through what they store.
- Callee `ret` sizes are read from each callee's first `ret` (`ba3d0`
  from the `ret` of `b7360`, which it tail-jumps to).

### Slice 6c bodies: transfers, per-body walkthroughs, callee ret sizes

**Transfers.** The eight integer registers, the `xmm` registers, MXCSR
and the x87 state are retail's at every call and return, and every
store retail makes is made, stack slots included. Flags are left
undefined at every transfer: no entry and no call site of these bodies
reads them (`python3 -I scripts/sim-liveness.py 0xc6740 0x114c60 0xc6990
0xd0620 0xd0690 0xc6810 0xc50a0 0x111650`). Scalar SSE runs through
`sse.rs` on the guest's registers. The x87 work is three `fld`/`fistp`
pairs (`114cce`, `c5214`, `c5281`) under the guest's control word, with
the last-instruction pointers the `fistp` leaves. `111650` touches no
x87 state.

**Timers `c6740`.** Counts down body `+2e8` (zeroing `+2e4` at 0) and
`+2ca` (at 0, the deferred injury notice `1c9c80(unit, +2c8, +2cc, 0,
1)`, then `+2c8` = 0, `+2d0` = none, `+2cc` = 0); either running makes
`al` 1. Then the byte `+1f4`: at 0, `cffc0` (`edi` the unit); while it
runs `al` is 1.

**Dialogue `114c60`.** The dialogue block is body `+ [body+342]`. Its
timer `+3c` is refilled from the tick rate (game time `+2`) through
`fld`/`fistp` when `+3e` is left; `+42`, `+40`, `+4e`, `+4c` (clearing
the handle `+54`) and `+50` count down. While a line plays (`+c`
positive) it starts once (`+48`: `20b140` without a sound `+1c`,
`1891d0` with the record `[E+c..18]` for the handle `+54`, `20d220`),
calls `20d570` once before and once after (`+49`), `20fd50` with a
sound, and clears `+c`. Last, body `+208` moves toward zero by a step
clamped between two constants. The Halo 1 search dialogue hooks this
point (`before`), so the body is guarded under Halo 1.

**Team `c6990`, seats `d0620` and `d0690`.** Unless body `+10a` bit 2,
the team word `+138` is the player's (`+13c`; `i8` at `+c0` of its
`21c`-byte record), else the actor's (`+12c`; `+24` of its `888`-byte
record), else for a vehicle (`+aa` 1) the seated units' teams through
the seat iterator in the frame (`E+4..1c`): a unit whose seat record
(vehicle definition `+1cc`, `b0` bytes, bit 11) is flagged gives one
candidate, the others another; the team is that one when they agree or
the flagged one is none, else none. `d0620` climbs from the unit to its
top biped or vehicle ancestor (while body `+4` bit 26 is set), storing
the 1 it shifts over its saved `ecx`; `d0690` walks that ancestor's
children (siblings `+c`, children `+10`, entering a biped or vehicle
with bit 26) and stores the next biped or vehicle (`+10`), its seat
(`+14`, body `+1fc`) and its parent (`+c`). Its found byte is the top
byte of its saved `ecx`, so `ecx` returns it. Both are leaves: no call.

**Camouflage `c6810`.** In the campaign with the Assassins skull, an AI
unit whose team is none, outside `0..10`, or not friendly to the local
player's (`bfe60`, `al`) gets body `+134` bit 3. Outside the campaign the
body passes the mode test only where `engine::skull_gate` answers it for
Assassins (a Firefight host, [skulls.md](../../game/skulls.md#the-campaign-gate)),
as the lifted code does with the hook at its `jne` (`c683f`). The level `+2b0` moves
by `+2b8` times the tick toward 1 (bit 3) or 0, clamped; reaching 1 with
an effect at `+d4` calls `b58c0(+d4, mask)`. The level `+2b4` moves by
the tick times a constant toward 1 (bit 4) or 0, clamped.

**Light `c50a0`.** For a player's unit in the campaign only. Without
the player record's flag `+88` and without the Envy skull, the toggle
(body `+148` bit 2) switches `+134` bit 29 with `c5890(4)` or
`c5890(5)` (the latter starting the timer `+268`); the timer counts
down by the tick, and bit 29 clears when the unit has a parent, is
dying (`+10a` bit 2), or the timer ran out while the game runs
(`[[5107e8]]`). The level `+264` then moves toward 1 or 0 by one tick
over a tick-rate-scaled time. With the flag or Envy: the camouflage
toggle through `c5740(unit, 1)` and `+134` bit 15, and the timer `+2be`
ending in `c5890(3)` (`+2bc` 2, then cleared) or `c5740(unit, 0)`
(`+2bc` 1). The Halo 1 flashlight and scope hook this point
(`before`), so the body is guarded under Halo 1.

**Animation state `111650`.** It picks two animation names: A (against
the animation block's `+70`) and B (against `+7c`), from the record
argument (`+0`, `+4`, each unless none or 0; with B its byte `+8`),
else from defaults: with no parent, a name `10eef0` gives (`dl` the
record's byte `+9`), fixed names for a dying unit (`+10a` bit 2) or one
whose byte `+1f4` runs, and the limits block's (`body + [body+33e]`)
`+2c` when set. When the animation block (`body + [body+12a]`) plays
(`+0`, `+6` not none) it calls `1cb710`, `e6a20` and `1c6440` (whose
answer's word `+14` must exceed 1), then `e6b00` for the names, and a
signed `cmp` ladder on B before chooses the seat vector (`bfc90`,
`1c7380`, `10eea0`, `10ee70`, `13e420`, `ba160`, `1c73a0`, `142570`,
`b8bd0`, then `e5840`), the three channel marks (`or byte [ebp+11|31|51],
1`), `e4300`, `11b710`, or the kill test (`10f5f0`, `a7670`, `d6bc0`).
Then `111010` and, with a parent, `c7070` and the parent's seat flags.
A change of names restarts them (`1103a0` unless given, `10f430`,
`1ccba0`, `10ee20`); otherwise once in 16 ticks per unit (the counter
`[55e70c]`) `1ce180` and `1060a0` for each held weapon (`+218..224`).
The limits block's three channels (`+dc`, `+9c`, `+bc`) each get
`1c68c0`, `e6a20`, `11b710`, `e6b00`, `110fc0`, `1c63f0`, `114330` and
`ba350` as their flags ask. `al` is the OR of `113910`'s, `10dc70`'s and
`10eaf0`'s answers and a channel playing. The string IDs it compares and
stores, and the `imm32` arguments it pushes, are read from the
instructions that carry them.

**Callees**, every one a callout, with the bytes `ret` pops: `cffc0`,
`c5890`, `20fd50`, `113910`, `1c6440`, `bfc90`, `1c7380`, `10eea0`,
`10ee70`, `e4300`, `10f5f0`, `a7670`, `111010`, `c7070`, `1ccba0`,
`10ee20`, `1060a0`, `1c63f0`, `114330`, `110fc0`, `10dc70`, `10eaf0`
(0); `bfe60`, `c5740`, `1891d0`, `10eef0`, `142570`, `b8bd0`, `e5840`,
`11b710`, `ba160`, `d6bc0`, `ba350` (4); `b58c0`, `20b140`, `20d570`,
`e6a20`, `13e420`, `1c73a0` (8); `20d220`, `e6b00`, `1103a0`, `1ce180`,
`1c68c0` (`c`); `1c9c80` (`10`); `1cb710` (`14`); `10f430` (`1c`).

### Not verified for slice 6c

- Captures from Waterworks and a campaign level (the slice's testable
  result): registers and RAM at the n-th call of each body, compared
  with `impl.sim.native.objects` off. The harness covers synthetic
  pools with stubbed callees; the player, actor, skull and dialogue
  paths are reached only with dressed fields.
- `111650`'s field names and what its callees do are read from data
  flow, not from the callees. Its callees' `ret` sizes come from each
  callee's own `ret`.
- That the stub answers reach every arm: `111650`'s ladder is driven by
  the names read from the image, so which arm a case reaches depends on
  their order, which the test does not assert.

### Slice 7a bodies: dd360 frame, flags, dead stores, callees

**Frame of `dd360`.** `sub esp, 16c`, then `ebp`, `esi`, `edi` pushed;
`ebx` is pushed only once the biped has a physics component (body `+b4`),
so the early return pops three registers and every site after the first
is at `esp` four bytes lower. The definition at `[S+14]` and the
animation block at `[S+24]` are overwritten on the seated path (the
`fistp` out and its float spill), so the body reads them where retail
does and nowhere else. The record at `[S+18..20]` is built from two
words read from the instructions at `dd3df` and `dd3e7`; under a biped
parent its second word is computed from two more (`dd51f`, `dd524`).

**Flags and floats.** Every body reproduces the arithmetic flags at
every transfer from the last instruction that sets them (rule 3),
through `sse.rs` and `alu.rs`, as slices 10 and 11 do; `lahf` writes `ah`
on the two `ucomiss` tests (`dd572`, `e4cc5`). Scalar SSE runs on the
guest's `xmm` registers. Three `fld`/`fistp` pairs (`dd476`, `e409a`,
`146a0a`) run on the guest's x87 state under its control word and leave
the `fistp`'s last-instruction and data pointers.

**Stores that look dead.** `1469f0` stores the product over its
argument slot and the integer over its saved `ecx`, so `ecx` returns
the integer. `e4c50` stores its float argument over its saved `ecx`
slot, so `ecx` returns that float on the paths that call `e4d90`.
`de190` reads `[U+9c..a4]`, which only `b8d30` writes: when neither
marker is found the values are whatever earlier frames left there, and
the bodies make every stack store retail makes so they match.

**Callees** that stay callouts, with the bytes `ret` pops: `e3700`,
`e5930`, `1faf30`, `e6900`, `10f820`, `de080` (0); `e3380`, `e0ef0`,
`bfa40` (4); `e01d0`, `dc5c0`, `e3c90`, `e40d0`, `e4450` (8); `e24f0`,
`e4d90` (`c`); `10f430` (`1c`); and the shared `1e5af0` (4),
`b58c0` (8), `b8d30` (`c`), `30bf0`, `113e40` (0). Fused bodies in the
tree: `bba20`, `ba1d0` (slice 4), `cba50` (slice 6a), `111650` (slice
6c) and `e6830`.

### Slice 7a bodies: split block, not ported list

**A split block.** Function discovery takes `de336`, the look update's
third clamp step, for a function start (`h2 analyze --explain de336`:
"found by TailCall", from the `jmp short` at `de32c` and `de331`), so
the lifter marks those jumps as tail calls. The body runs it as its own
block; the harness's lifted side steps on through it (`SPLITS`).

**Not ported.** The movement and animation steps `e01d0`, `dc5c0`,
`e24f0`, `e0ef0`, `e3c90`, `e40d0`, `e4450` (with `df380` below them;
`e0dc0`, also below them, is a slice 7b body), the seat updates `e3380` and `e3700`, and `e4d90`, `e6900`,
`de080`, `e5930`, `10f430`, `bfa40`, `1faf30`, `10f820`. Each is a later
lane's body; the segments keep every frame slot and register they read.

### Not verified for slice 7a

- Captures from a match and a campaign level (the slice's testable
  result): registers and RAM at the n-th call of `BIPED_UPDATE`, for a
  biped on foot, airborne, seated in a vehicle and riding a biped,
  compared with `impl.sim.native.objects` off. The harness covers
  synthetic pools, with stubbed callees.
- `1469f0` has callers outside the biped update (one returns to
  `167507`), and the leaves may have others; the bodies run there too
  when the switch is on. The harness enters them only as roots and from
  `dd360`.
- Callee `ret` sizes are read from each callee's last `ret` before its
  padding.

### Slice 7b bodies: dd990 frame, contact switch, flags, image reads

**Frame of `dd990`.** `sub esp, b8`, then `ebp`, `esi`, `edi` pushed;
`ebx` is pushed only once the biped has a physics component (body `+b4`),
so the early return pops three registers (`ddf46`) and the rest four
(`ddf45`, or the bit 0 exit after `ddf16`). `[S+18]` holds the crouch
step as a float, then its low byte the jump update's answer, and the
whole dword is pushed to `dd0d0`; `[S+14]` holds the crouch height's out
float, then the contact record. The output `1e5a60` writes at `[S+20..28]`
overlays the root velocity at `[S+1c..24]`.

**The contact switch.** The output's index is checked against the
component's count `+8c` with `jle`, so an index equal to the count reads
the record past the last; a negative index compares the count with 0.
The record's word `+4` is none, or a kind in bits 29 to 31: 1 a surface
index alone, 4 an object through the per-index table `[51e9cc]` and its
model's part surfaces (`1efb40`, then the shape `1efd80` answers), 5 a
structure surface through `[4e0348]` `+144` and `+13c`; any other kind,
and none, take the record's own object `+18` when it is in the world
(body `+4` bit 22). Each kind stores the five contact words `+354..364`
in retail's order.

**Flags and floats.** Every body reproduces the arithmetic flags at
every transfer from the last instruction that sets them (rule 3),
through `sse.rs` and `alu.rs`; the `inc`, `neg` and `cmp` idioms the
slice adds are in the module (`inc` keeps CF). The crouch height's
`ucomiss` takes its parity branch as retail does. Scalar SSE runs on the
guest's `xmm` registers; `e4330` takes its speed in `xmm2` and the
post-update hands it `[+45c]`, plus the component's `+14` when `1cfdf0`
answers. Five `fld`/`fistp` pairs (`e441a`, `e46e2`, `e2ffa`, `e30a5`,
`e3314`) run on the guest's x87 state under its control word and leave
the `fistp`'s last-instruction and data pointers.

**Read from the image.** The `.rdata` constants (`45dbc0`, `45dbd8`,
`45dbdc`, `45dbbc`, `45dbfc`, `45dc0c`, `45dc68`, `45dd30`, `45dd74`,
`44ae90`, `43ff08`), the down vector `[4687bc]`, `1201a0`'s `[4687b0]`,
the probe's animation name, time and collision flags (the immediates at
`e314e`, `e3159`, `e3237`, `e323e`), and `dd0d0`'s jump table `dd2e4`,
read by the state less 1. The table has no bounds check: a state of 0 or
above 6 jumps through whatever follows it in retail, and the body
returns an unowned fault there instead. The harness keeps the state in
1 to 6.

### Slice 7b bodies: dead stores, callees, not ported

**Stores that look dead.** `e4330` stores body `+348` bit 6 at
`[E+3]`, which nothing reads. `dd990` sets the damage packet's word at
`[S+bc]` and its byte at `[S+c4]` before `d66d0`, and `dd0d0` reuses the
object's argument slot `[S+58]` for three flag bytes in turn and then the
early mover; the bodies make every stack store retail makes.

**Callees** that stay callouts, with the bytes `ret` pops: `1ec500`,
`1d1c10`, `1e5a60`, `1cfdf0`, `1201a0`, `1c9c00`, `1e20b0` (answers in
`xmm0`), `1d0870` (0); `d6660`, `b9b90`, `ba160`, `badc0`, `1efb40`,
`25aad0`, `e4770`, `1fb6f0` (4); `d66d0`, `1c4b00`, `1efd80` (8);
`114b60` (`c`); `d7b80`, `1697c0` (`18`); `1e68a0` (`28`). Fused bodies
in the tree: `baf80`, `b7360`, `bba20` (slice 1), `ba1d0`, `b7740`,
`b9dd0` (slice 4), `1469f0`, `e4050` (slice 7a). None reaches `bd090`, so
no root is in `FLOAT_OPT_OUT`. Function discovery finds no start inside
any of the ten (`h2 analyze --functions`), so none needs `SPLITS`.

**Not ported.** The AI hooks `1ec500` and `1e5a60`, the damage packet
`d6660`, `d66d0` and `d7b80`, the jump `e4770`, the physics step
`1e68a0` and the queries below the contact switch and the probe. Each is
a later lane's body; the segments keep every frame slot and register they
read.

### Not verified for slice 7b

- Captures from a match and a campaign level (the slice's testable
  result): registers and RAM at the n-th call of `BIPED_POST_UPDATE`, for
  a biped standing, crouching, jumping, landing and on each contact kind,
  compared with `impl.sim.native.objects` off. The harness covers
  synthetic pools, with stubbed callees.
- The leaves and segments may have callers outside the post-update; the
  bodies run there too when the switch is on. The harness enters them
  only as roots and from `dd990`.
- The `fistp`s assume the guest's control word masks the precision
  exception, as in slice 7a.
- Callee `ret` sizes are read from each callee's last `ret` before its
  padding.

### Register liveness: walk rules and limitations

The walk follows both arms of every branch and stops a path at `ret`,
`int3`, an indirect `jmp`, a `jmp` out of the function (below the entry,
to a listed entry, or past `int3` padding), when everything is written, or
after 400 instructions; a visited set keys on the address and the written
state. Sub-registers count by byte (`al` is not `eax`). `xor r,r`,
`sub r,r`, `or r,-1` and `and r,0` are writes only. A `call` is not
entered: the walk records which registers were unwritten there, then
continues as if the callee wrote no register (over-reporting reads after a
call, never under-reporting) and as if it wrote the flags.

Limitations:

- Indirect jumps (jump tables) end the path; rows with one say so. `bcbb0`,
  `1588b0`, `f78e0`, `13b390` and `13bc00` have one.
- Only integer registers and EFLAGS. `xmm0` is an argument of `13bb90` and
  a result of it and of `ba8c0`'s callees; x87 state is not tracked.
- Saves are recognised by tracking `esp` through pushes, pops, `add`/`sub
  esp`, and callees' `ret N`; an indirect call is assumed to pop the pushes
  that immediately precede it (stdcall). Where `esp` is lost, a
  straight-line look-ahead decides.
- "Reads before writing" after a call includes registers the callee may
  have written (the walk does not enter callees). `1cd4e0`'s `al` is such
  a case: it is the result of its own callee `1d9320`.
- `baeb0`'s `edi[31:16]`: it increments `edi` after loading only `di`, so
  the caller's upper half is read and discarded.
- Rows marked "truncated" hit the state bound (`b7b40`, `1d96d0`,
  `16a8e0`); their sets are a lower bound.
- `esp` is always live and is left out.

### Entries and callees: table

DF in "Flags read" comes from string instructions (`rep movsd`,
`rep stosd`): the direction flag must be clear on entry, as the ABI
requires.

| Entry | Reads before writing | Flags read | Saved and restored | Reaches `ret` with unwritten | Notes |
|---|---|---|---|---|---|
| `b6f10` OBJECTS_PRE_PHYSICS | none | none | ecx ebx ebp esi edi | edx |  |
| `b7060` OBJECTS_POST_PHYSICS | none | none | ebx ebp esi edi | ecx |  |
| `b7150` OBJECTS_FINALIZE | none | none | ebx ebp esi edi | none | tail `bf380` |
| `bc470` OBJECT_PRE_UPDATE | none | none | ebx ebp esi edi | edx ebp |  |
| `bc820` OBJECT_POST_UPDATE | none | DF | ebx ebp esi edi | ebx |  |
| `108c60` type update dispatch | none | none | ebx ebp esi edi | none |  |
| `108cd0` type post dispatch | edi | none | esi | ebx ebp esi edi |  |
| `109050` type pose dispatch | ebx | none | ebp esi edi | ebx |  |
| `1090d0` type after-root dispatch | ebx | none | ebp esi edi | ebx |  |
| `108d90` OBJECT_FUNCTION_IMPORT | ebx edi | none | ebp esi | ebx edi |  |
| `108a90` type new dispatch | ebx | none | ebp esi edi | ebx |  |
| `109140` type node-matrix dispatch | ebx | none | ebp esi edi | ebx |  |
| `b7360` wake | eax | none | ebx esi | eax ebp edi |  |
| `baf80` root ancestor | ecx | none | none | ecx edx ebx ebp esi edi |  |
| `bba20` wake for post | eax | none | ebx esi edi | edx ebp |  |
| `bc7b0` wake children | none | none | esi | ebx ebp edi |  |
| `109580` early-mover wake | none | none | ebx ebp esi edi | esi edi |  |
| `bab40` OBJECT_FUNCTION_VALUE | none | none | ecx ebx ebp esi edi | none |  |
| `ba8c0` function evaluate | eax | none | ebx ebp esi edi | edi |  |
| `bcc20` BASE_OBJECT_IMPORT | none | none | ecx ebx esi | ebp edi |  |
| `be8e0` OBJECT_CHANGE_COLORS | none | none | ebx ebp esi edi | ebx ebp edi |  |
| `c6b90` unit import | none | none | ebx esi | ebp esi edi |  |
| `de3b0` biped import | none | none | esi | ebx ebp edi |  |
| `f6d10` vehicle import | none | none | ebx ebp esi edi | none |  |
| `ff5f0` weapon import | none | none | ecx ebx ebp esi edi | none |  |
| `fbe70` projectile import | none | none | ebx | ebp esi edi |  |
| `106d70` device import | none | none | ebx | ebp esi edi |  |
| `11bd60` machine and crate import | none | none | ebx esi | ebp esi edi |  |
| `bdef0` bounds | none | none | ebx ebp esi edi | none |  |
| `bef30` attachment refresh | none | none | ebx ebp esi edi | none |  |
| `be1d0` node-matrix notify | eax | none | ecx ebx | ebp esi edi |  |
| `bc5e0` component to object | none | none | ebx ebp esi edi | ebp esi |  |
| `bd090` OBJECT_NODE_MATRICES | none | DF | ebx ebp esi edi | none |  |
| `c1400` lights walk | none | none | ebx ebp esi edi | ebp edi |  |
| `c0c80` LIGHT_RECORD_UPDATE | none | none | ebx ebp esi edi | none |  |
| `c0b00` light record | eax ecx | none | ebx ebp esi edi | ecx edx |  |
| `c2d00` light list | none | none | ebx ebp esi edi | esi |  |
| `c3220` light list | eax | none | esi edi | eax ebx ebp edi |  |
| `c3260` light list | none | none | ebx ebp esi edi | none |  |
| `b7b40` object_new | none | DF | ebx ebp esi edi | ecx edx esi edi | truncated |
| `b83b0` delete | none | none | ebx esi edi | ebp |  |
| `b8460` delete | none | none | ebx ebp esi edi | none |  |
| `b7430` transform set | none | none | ecx ebx ebp esi edi | edx |  |
| `b7740` transform set | eax ecx edi | none | ebx ebp esi | ecx ebp edi |  |
| `fd910` deferred weapon fire | none | none | none | ecx edx ebx ebp esi edi |  |
| `bf380` object memory collection | none | none | ebx ebp esi edi | ecx edx edi |  |
| `109660` early-mover record | eax ecx | DF | ebx ebp esi edi | none |  |
| `a9570` network transform apply | eax | none | ebx esi edi | ebx ebp |  |
| `1cd4e0` animation | al esi | none | ebx | eax edx ebp esi edi |  |
| `1ced00` component | edi | none | ebx ebp esi | ecx edx ebx ebp esi edi |  |
| `1d3920` component nodes | none | none | ebx ebp esi edi | ecx edx |  |
| `1d44f0` component sphere | none | none | ebx ebp esi edi | eax ecx edx |  |
| `bb050` OBJECT_SPATIAL_VISIT | none | none | ebx ebp esi edi | ecx |  |
| `d5de0` OBJECT_DAMAGE_UPDATE | none | DF | ebx ebp esi edi | edi |  |
| `b87b0` partition disconnect | none | none | ebx ebp esi edi | none |  |
| `b8600` partition connect | none | DF | ebx ebp esi edi | none |  |
| `b8c00` node matrices of | eax | none | none | ebx ebp esi edi |  |
| `13bb90` function source | eax | none | edi | ecx edx ebx ebp esi |  |
| `baeb0` object iterator next | edi[31:16] | none | ebx ebp esi edi | none |  |
| `fd980` weapon fire triggers | none | none | ebx ebp esi edi | esi |  |
| `320560` | eax | none | ecx | eax ecx edx ebx ebp esi edi |  |
| `10aac0` | ebx | none | ebp esi edi | eax edx ebx esi edi |  |
| `b8bd0` | eax | none | none | ebx ebp esi edi |  |
| `bff60` | none | none | none | eax ecx edx ebx ebp esi edi |  |
| `bca70` | none | none | none | edx ebx ebp esi edi |  |
| `bcbb0` | eax | none | ecx | edx ebx ebp esi edi | 1 indirect jmp |
| `bcad0` | none | none | esi | ebx ebp edi |  |
| `bcb60` | none | none | esi | ecx edx ebx ebp edi |  |
| `bcb10` | none | none | esi | ecx edx ebx ebp edi |  |
| `1588b0` | ecx | none | esi | ebx ebp edi | 1 indirect jmp |
| `c18e0` | none | none | esi | edx ebx ebp edi |  |
| `bc970` | eax ecx | none | ebx esi | eax ecx ebp esi edi |  |
| `131c20` | eax ecx esi edi | none | ebx | ecx edx ebp esi edi |  |
| `d1210` | eax | none | none | ebx ebp esi edi |  |
| `d1410` | eax | none | esi | eax edx ebx ebp esi edi |  |
| `f5d70` | eax | none | none | ebx ebp esi edi |  |
| `e6fe0` | eax ecx | none | esi edi | ebx ebp |  |
| `f78e0` | eax | none | ecx | none | 1 indirect jmp |
| `f78b0` | none | none | none | eax ecx edx ebx ebp esi edi |  |
| `24590` | eax | none | none | eax ecx edx ebx ebp esi edi |  |
| `f8070` | cl | none | none | ebx ebp esi edi |  |
| `11d580` | eax ecx edx esi | none | none | eax ecx edx ebx ebp esi edi |  |
| `102070` | eax | none | esi | ebx ebp edi |  |
| `10cf50` | ecx | none | none | eax ebx ebp esi edi |  |
| `162b10` | ecx | none | esi | eax ecx ebx ebp edi |  |
| `162b70` | none | none | none | ecx edx ebx ebp esi edi |  |
| `1d3550` | ecx edx esi | none | none | eax edx ebx ebp esi edi |  |
| `b9ce0` | ecx | none | esi | ebx ebp edi |  |
| `c1720` | none | none | ebx ebp esi edi | ecx edx |  |
| `1d1c10` | esi | none | ecx ebx ebp edi | edx esi edi |  |
| `1d1ca0` | edi | none | esi | eax edx ebx ebp esi edi |  |
| `1d4360` | edi | none | esi | ebx ebp esi edi |  |
| `b91d0` | eax esi edi | none | ebx | ecx edx ebp esi edi | tail `11d090` |
| `1d09d0` | eax ecx edx | none | none | ecx ebx ebp esi edi |  |
| `1d0ad0` | eax ecx edx | none | none | ecx ebx ebp esi edi |  |
| `1d96d0` | eax | none | ebx ebp esi edi | edx ebp edi | truncated |
| `e1670` | eax | none | none | ebx ebp esi edi |  |
| `16d940` | none | none | ebx ebp esi edi | edx ebx ebp edi |  |
| `bf5a0` | eax | none | none | ebx ebp esi edi |  |
| `bfc90` | ecx | none | none | eax ecx edx ebx ebp esi edi |  |
| `bff10` | ecx | none | none | ecx ebx ebp esi edi |  |
| `bfe80` | eax | none | none | eax ebx ebp esi edi |  |
| `bfed0` | eax esi | DF | ebx edi | ecx ebp esi edi |  |
| `bdb60` | eax ecx edx | none | ebx ebp esi edi | eax ecx edx |  |
| `bfe20` | eax cl dl | none | none | eax ecx ebx ebp esi edi |  |
| `b6780` | eax | none | none | ebx ebp esi edi |  |
| `1c6440` | ecx | none | none | ecx edx ebx ebp esi edi |  |
| `1421f0` | none | none | ebp | edx ebx esi edi |  |
| `a98b0` | ecx esi | none | ebx | ecx ebp esi edi |  |
| `a9940` | ecx esi edi | none | ebx | ecx ebp esi edi |  |
| `bdc40` | eax ecx bl | DF | ebp esi edi | ebx |  |
| `142a60` | ecx edx | none | none | ecx edx ebx ebp esi edi |  |
| `16bc00` | ecx edx | none | ebx esi edi | ecx edx ebx ebp esi edi |  |
| `1cae40` | edi | none | ebx ebp esi | edx ebx esi edi |  |
| `bad50` | eax esi edi | none | ebx | ebp esi edi |  |
| `131d60` | ecx ebx | none | ebp esi edi | edx ebx |  |
| `17ca10` | ax | none | esi | eax ecx edx ebx ebp esi edi |  |
| `b9d20` | eax | none | esi | ebx ebp edi |  |
| `13b390` | edi | none | ebx ebp esi | eax ecx edx edi | 1 indirect jmp |
| `13bc00` | ecx | none | ebp esi | ecx edx ebx edi | 1 indirect jmp |
| `c0840` | eax esi | none | ebx ebp edi | eax ecx edx ebx ebp esi edi |  |
| `b8c40` | ax ecx | none | esi | ebx ebp edi |  |
| `16640f` | eax | DF | ebx ebp esi edi | ecx edx edi |  |
| `b8d30` | cl ebx | DF | ebp esi edi | ebx |  |
| `142570` | eax edx | none | none | edx ebx ebp esi edi |  |
| `c28b0` | eax | none | ebx ebp esi edi | none |  |
| `16a8e0` | none | none | ebx ebp esi edi | ebx esi | truncated |
| `11bed0` | esi | none | edi | eax ecx edx ebx ebp esi |  |
| `ba300` | eax ecx | none | esi edi | eax ebx ebp esi |  |
| `1cac60` | eax | DF | ebx ebp esi edi | esi |  |
| `16ba40` | eax edx | DF | ebx ebp esi edi | edx ebp |  |

### Call sites: table of call sites

"Call" is the call instruction, "Returns to" the retail return site a
body lists in `Body::sites`. "Reaches `ret` with unwritten" names the
registers whose value at the caller's own return may be the callee's.

| In | Call | Returns to | Callee | After return, reads | Flags | Reaches `ret` with unwritten |
|---|---|---|---|---|---|---|
| `b6f10` | `b6f69` | `b6f6e` | `bc470` | ebx ebp esi edi | none | edx |
| `b6f10` | `b6f7a` | `b6f7f` | `109580` | ebx ebp | none | edx |
| `b6f10` | `b7031` | `b7036` | `bc470` | none | none | none |
| `b7060` | `b70a9` | `b70ae` | `bc820` | ebx ebp esi edi | none | none |
| `b7060` | `b70b2` | `b70b7` | `109660` | ebx ebp edi | none | ecx |
| `b7060` | `b70f1` | `b70f6` | `fd910` | none | none | eax ecx |
| `b7060` | `b713d` | `b7142` | `bc820` | none | none | none |
| `b7150` | `b719b` | `b71a0` | `fd910` | none | none | none |
| `b7150` | `b723b` | `b7240` | `bc470` | ebx ebp esi edi | none | none |
| `b7150` | `b724b` | `b7250` | `bc820` | none | none | none |
| `b7150` | `b727d` | `b7282` | `b83b0` | esi | none | none |
| `b7150` | `b7285` | `b728a` | `b8460` | none | none | none |
| `bc470` | `bc4c5` | `bc4ca` | `bba20` | bl ebp esi edi | none | none |
| `bc470` | `bc4e8` | `bc4ed` | `a9570` | bl ebp esi edi | none | none |
| `bc470` | `bc517` | `bc51c` | `1cd4e0` | al ebp edi | none | ecx edx |
| `bc470` | `bc51f` | `bc524` | `108c60` | al bl ebp edi | none | ecx edx |
| `bc470` | `bc527` | `bc52c` | `d5de0` | al bl ebp edi | none | ecx edx |
| `bc470` | `bc52f` | `bc534` | `be8e0` | al bl ebp | none | ecx edx |
| `bc470` | `bc541` | `bc546` | `bc470` | al bl ebp esi | none | none |
| `bc470` | `bc595` | `bc59a` | `1ced00` | bl ebp | none | eax ecx edx |
| `bc820` | `bc84e` | `bc853` | `bc5e0` | ebp esi edi | DF | ecx edx ebx |
| `bc820` | `bc862` | `bc867` | `bd090` | al ebp esi edi | none | ecx edx ebx |
| `bc820` | `bc8dc` | `bc8e1` | `1d3920` | al ebp esi | none | ecx edx |
| `bc820` | `bc8e6` | `bc8eb` | `108cd0` | al ebp esi edi | none | ecx edx ebx |
| `bc820` | `bc911` | `bc916` | `bdef0` | al ebp esi edi | none | ecx edx ebx |
| `bc820` | `bc929` | `bc92e` | `bef30` | esi edi | none | ecx edx ebx |
| `bc820` | `bc937` | `bc93c` | `bc820` | esi edi | none | ebx |
| `bc820` | `bc95c` | `bc961` | `be1d0` | none | none | eax ecx edx ebx |
| `108c60` | `108cab` | `108cad` | indirect | al bl ebp esi edi | none | none |
| `108cd0` | `108d11` | `108d13` | indirect | esi edi | none | edx ebx ebp edi |
| `109050` | `1090a5` | `1090a7` | indirect | ebx ebp esi edi | none | ebx |
| `1090d0` | `10911b` | `10911d` | indirect | ebx ebp esi edi | none | ebx |
| `108d90` | `108de5` | `108de7` | indirect | al ebx ebp esi edi | none | eax ecx edx ebx edi |
| `108a90` | `108ae4` | `108ae6` | indirect | al ebx ebp esi edi | none | eax ebx |
| `109140` | `109190` | `109192` | indirect | ebx ebp esi edi | none | edx ebx |
| `bba20` | `bba44` | `bba49` | `baf80` | eax ebx | none | ecx ebp |
| `bba20` | `bba5e` | `bba63` | `b7360` | esi edi | none | eax ecx edx ebp |
| `bba20` | `bba68` | `bba6d` | `bc7b0` | none | none | eax ecx edx ebp |
| `bc7b0` | `bc7e1` | `bc7e6` | `b7360` | esi | none | eax edx ebx ebp edi |
| `bc7b0` | `bc807` | `bc80c` | `bc7b0` | esi | none | edx ebx ebp edi |
| `109580` | `109585` | `10958a` | `320560` | none | none | esi edi |
| `109580` | `1095c2` | `1095c7` | `bb050` | ax ebx | none | ecx edx esi edi |
| `109580` | `10961d` | `109622` | `b7360` | ebx ebp esi edi | none | ecx |
| `109580` | `109633` | `109638` | `bba20` | ebx ebp esi | none | ecx edx |
| `bab40` | `bab8e` | `bab93` | `10aac0` | al ebx ebp esi edi | none | eax edx |
| `bab40` | `babdd` | `babe2` | `108d90` | al ebp esi | none | eax edx |
| `bab40` | `bac37` | `bac3c` | `ba8c0` | al ebx ebp esi edi | none | eax edx |
| `bab40` | `bac6f` | `bac74` | `bab40` | none | none | eax edx |
| `ba8c0` | `ba927` | `ba92c` | `10aac0` | al ebp esi | none | eax ecx edx edi |
| `ba8c0` | `ba949` | `ba94e` | `108d90` | ebp esi | none | eax edx |
| `ba8c0` | `ba98c` | `ba991` | `13bb90` | ebp esi | none | eax edx |
| `ba8c0` | `baa22` | `baa27` | `13bb90` | ebp esi | none | eax edx |
| `ba8c0` | `baa45` | `baa4a` | `bab40` | al ebp esi | none | eax edx |
| `ba8c0` | `baac7` | `baacc` | `108d90` | none | none | eax edx |
| `bcc20` | `bccbf` | `bccc4` | `b8bd0` | eax bl | none | eax edx ebp edi |
| `bcc20` | `bccf5` | `bccfa` | `bff60` | bl | none | eax edx ebp edi |
| `bcc20` | `bcd2b` | `bcd30` | `bca70` | bl | none | eax edx ebp edi |
| `bcc20` | `bcd35` | `bcd3a` | `bcbb0` | bl | none | eax edx ebp edi |
| `bcc20` | `bcda5` | `bcdaa` | `bcad0` | bl | none | eax edx ebp edi |
| `bcc20` | `bcdd4` | `bcdd9` | `bcb60` | bl | none | eax edx ebp edi |
| `bcc20` | `bcde1` | `bcde6` | `bcb10` | bl | none | eax edx ebp edi |
| `bcc20` | `bcec6` | `bcecb` | `1588b0` | bl esi | none | eax edx ebp edi |
| `bcc20` | `bcef6` | `bcefb` | `c18e0` | bl | none | eax edx ebp edi |
| `bcc20` | `bcfa7` | `bcfac` | `bc970` | al | none | eax edx ebp edi |
| `be8e0` | `be9bc` | `be9c1` | `bab40` | al ebx ebp edi | none | none |
| `be8e0` | `be9df` | `be9e4` | `131c20` | ebx ebp | none | none |
| `be8e0` | `bea27` | `bea2c` | `bab40` | al ebx ebp esi edi | none | edx |
| `c6b90` | `c6c21` | `c6c26` | `d1210` | bl | none | ebp esi edi |
| `c6b90` | `c6c28` | `c6c2d` | `d1410` | bl | none | ebp esi edi |
| `c6b90` | `c6ca6` | `c6cab` | `d1410` | none | none | none |
| `c6b90` | `c6dc8` | `c6dcd` | `ff5f0` | none | none | eax ecx edx ebp edi |
| `f6d10` | `f6dc0` | `f6dc5` | `d1210` | bl | none | none |
| `f6d10` | `f6dcc` | `f6dd1` | `f5d70` | al bl esi | none | none |
| `f6d10` | `f6e1c` | `f6e21` | `e6fe0` | al bl | none | none |
| `f6d10` | `f6f28` | `f6f2d` | `f78e0` | bl esi | none | none |
| `f6d10` | `f6fff` | `f7004` | `f78b0` | bl ebp esi | none | none |
| `f6d10` | `f7015` | `f701a` | `f78b0` | bl | none | none |
| `f6d10` | `f7079` | `f707e` | `24590` | ecx bl | none | none |
| `f6d10` | `f7082` | `f7087` | `f78b0` | bl | none | none |
| `f6d10` | `f70c1` | `f70c6` | `f8070` | al ebx esi edi | none | none |
| `f6d10` | `f70dc` | `f70e1` | `f8070` | al ebp esi edi | none | none |
| `f6d10` | `f7105` | `f710a` | `f78e0` | ebx ebp esi edi | none | none |
| `f6d10` | `f712e` | `f7133` | `f78b0` | ebx esi edi | none | none |
| `f6d10` | `f713a` | `f713f` | `f78e0` | esi edi | none | none |
| `f6d10` | `f7151` | `f7156` | `f78b0` | esi edi | none | none |
| `f6d10` | `f71b8` | `f71bd` | `f78b0` | esi edi | none | none |
| `f6d10` | `f7205` | `f720a` | `f78e0` | bl | none | none |
| `f6d10` | `f7283` | `f7288` | `f78e0` | bl esi | none | none |
| `f6d10` | `f748c` | `f7491` | `24590` | ecx bl | none | none |
| `f6d10` | `f7495` | `f749a` | `f78b0` | bl | none | none |
| `f6d10` | `f7554` | `f7559` | `24590` | ecx bl | none | none |
| `f6d10` | `f755d` | `f7562` | `f78b0` | bl | none | none |
| `f6d10` | `f75f6` | `f75fb` | `11d580` | ecx bl esi | none | none |
| `f6d10` | `f7602` | `f7607` | `24590` | ecx bl | none | none |
| `f6d10` | `f760b` | `f7610` | `f78b0` | bl | none | none |
| `f6d10` | `f76a9` | `f76ae` | `f78b0` | eax ecx bl esi | none | none |
| `f6d10` | `f7705` | `f770a` | `f78b0` | bl | none | none |
| `f6d10` | `f771c` | `f7721` | `f78e0` | bl esi | none | none |
| `f6d10` | `f7755` | `f775a` | `f78e0` | bl esi | none | none |
| `f6d10` | `f77b7` | `f77bc` | `f78e0` | ecx bl esi | none | none |
| `f6d10` | `f77c8` | `f77cd` | `24590` | ecx bl esi | none | none |
| `f6d10` | `f77d1` | `f77d6` | `f78b0` | ecx bl esi | none | none |
| `f6d10` | `f77ec` | `f77f1` | `f78b0` | bl | none | none |
| `f6d10` | `f7839` | `f783e` | `f78e0` | bl esi | none | none |
| `f6d10` | `f7852` | `f7857` | `f78b0` | bl | none | none |
| `ff5f0` | `ff745` | `ff74a` | `102070` | ebx ebp esi edi | none | none |
| `ff5f0` | `ff7ea` | `ff7ef` | `10cf50` | al | none | none |
| `ff5f0` | `ff887` | `ff88c` | `10cf50` | al ebp | none | none |
| `ff5f0` | `ff940` | `ff945` | `162b10` | al esi edi | none | none |
| `ff5f0` | `ff94e` | `ff953` | `162b70` | esi | none | none |
| `ff5f0` | `ffa65` | `ffa6a` | `162b10` | al edi | none | none |
| `ff5f0` | `ffa73` | `ffa78` | `162b70` | none | none | none |
| `ff5f0` | `ffa9c` | `ffaa1` | `102070` | none | none | none |
| `11bd60` | `11bdaa` | `11bdaf` | `1d3550` | al bl | none | eax ecx edx ebp edi |
| `bdef0` | `bdf7f` | `bdf84` | `1d44f0` | al ebx ebp esi edi | none | edx |
| `bdef0` | `be189` | `be18e` | `b87b0` | ebx ebp esi edi | none | ecx edx |
| `bdef0` | `be1b3` | `be1b8` | `b8600` | none | none | eax ecx edx |
| `bef30` | `bef80` | `bef85` | `b9ce0` | al esi edi | none | eax ecx edx |
| `bef30` | `befcf` | `befd4` | `c3220` | bl ebp esi edi | none | ecx |
| `bef30` | `befdf` | `befe4` | `c2d00` | bl ebp esi edi | none | ecx |
| `bef30` | `bf004` | `bf009` | `c1720` | dh+edx[31:16] bl edi | none | ecx edx |
| `bef30` | `bf024` | `bf029` | `bef30` | edi | none | eax ecx edx |
| `be1d0` | `be225` | `be22a` | `b8c00` | eax | none | eax edx ebp esi edi |
| `be1d0` | `be230` | `be235` | `109140` | none | none | eax edx ebp esi edi |
| `bc5e0` | `bc634` | `bc639` | `1d1c10` | al ebx ebp edi | none | eax ecx |
| `bc5e0` | `bc673` | `bc678` | `1d1ca0` | al ebx ebp | none | none |
| `bc5e0` | `bc685` | `bc68a` | `1d4360` | ebp | none | none |
| `bc5e0` | `bc6b2` | `bc6b7` | `b91d0` | ebx ebp | none | none |
| `bc5e0` | `bc6d1` | `bc6d6` | `b7430` | ebp esi | none | none |
| `bc5e0` | `bc6e8` | `bc6ed` | `1d09d0` | ebp esi | none | none |
| `bc5e0` | `bc6ff` | `bc704` | `1d0ad0` | ecx esi | none | none |
| `bc5e0` | `bc71e` | `bc723` | `b7740` | none | none | eax ecx |
| `bc5e0` | `bc755` | `bc75a` | `1d96d0` | bl edi | none | eax ecx edx ebp esi |
| `bd090` | `bd18c` | `bd191` | `e1670` | ebx esi edi | DF | none |
| `bd090` | `bd1a7` | `bd1ac` | `1d3920` | ebx esi | DF | none |
| `bd090` | `bd247` | `bd24c` | `b8bd0` | eax ebx ebp esi edi | DF | none |
| `bd090` | `bd2a1` | `bd2a6` | `16d940` | ebx ebp | DF | ecx edx |
| `bd090` | `bd2af` | `bd2b4` | `bf5a0` | al ebx ebp esi | DF | ecx edx |
| `bd090` | `bd2be` | `bd2c3` | `bfc90` | al ebx ebp | DF | ecx edx |
| `bd090` | `bd307` | `bd30c` | `bff10` | al ebx ebp esi edi | DF | none |
| `bd090` | `bd323` | `bd328` | `bfe80` | ebx ebp | DF | none |
| `bd090` | `bd331` | `bd336` | `bfed0` | ebx ebp | DF | none |
| `bd090` | `bd353` | `bd358` | `bdb60` | ebx ebp esi | DF | edx |
| `bd090` | `bd365` | `bd36a` | `bfe20` | ebx ebp | DF | ecx edx |
| `bd090` | `bd372` | `bd377` | `b6780` | eax ebx ebp | DF | ecx edx |
| `bd090` | `bd37d` | `bd382` | `1c6440` | eax ebx | DF | ecx edx |
| `bd090` | `bd3c6` | `bd3cb` | `1421f0` | ebp | none | ecx edx |
| `bd090` | `bd432` | `bd437` | `a98b0` | al ebx ebp | none | none |
| `bd090` | `bd462` | `bd467` | `a9940` | al ebx ebp | none | none |
| `bd090` | `bd4cc` | `bd4d1` | `bdc40` | ebp | none | none |
| `bd090` | `bd4de` | `bd4e3` | `1421f0` | ebp | none | none |
| `bd090` | `bd4f6` | `bd4fb` | `142a60` | ebp | none | ecx edx |
| `bd090` | `bd507` | `bd50c` | `1090d0` | ebp | none | ecx edx |
| `bd090` | `bd559` | `bd55e` | `1421f0` | edx ebx ebp esi edi | none | none |
| `bd090` | `bd56c` | `bd571` | `142a60` | edx ebx ebp esi | none | none |
| `bd090` | `bd5f0` | `bd5f5` | `bdc40` | none | none | eax ecx edx |
| `c1400` | `c141b` | `c1420` | `16bc00` | eax esi | none | eax ecx edx ebp edi |
| `c1400` | `c1484` | `c1489` | `c0c80` | ebx ebp esi edi | none | ecx edx |
| `c1400` | `c14d9` | `c14de` | `1cae40` | ebx ebp esi | none | ecx edx |
| `c1400` | `c14e3` | `c14e8` | `c2d00` | ebx esi | none | ecx edx |
| `c1400` | `c1521` | `c1526` | `c3260` | ebx ebp | none | ecx edx |
| `c0c80` | `c0dc4` | `c0dc9` | `bab40` | ebx ebp | none | none |
| `c0c80` | `c0dd5` | `c0dda` | `bad50` | ebx ebp esi | none | none |
| `c0c80` | `c0df7` | `c0dfc` | `131d60` | ebp esi | none | none |
| `c0c80` | `c0e17` | `c0e1c` | `131d60` | ebp esi | none | none |
| `c0c80` | `c0e87` | `c0e8c` | `17ca10` | ebx ebp | none | none |
| `c0c80` | `c0eba` | `c0ebf` | `131c20` | ebx ebp | none | none |
| `c0c80` | `c0ed6` | `c0edb` | `131c20` | ebx ebp | none | none |
| `c0c80` | `c0f70` | `c0f75` | `c0b00` | ecx[31:16] ebx ebp esi edi | none | ecx edx |
| `c0c80` | `c0f90` | `c0f95` | `baf80` | eax ebx ebp esi edi | none | none |
| `c0c80` | `c1270` | `c1275` | `b9ce0` | al esi | none | eax ecx edx |
| `c0c80` | `c127b` | `c1280` | `b9d20` | al | none | eax ecx edx |
| `c0c80` | `c1297` | `c129c` | `c2d00` | none | none | eax ecx edx |
| `c0c80` | `c12b4` | `c12b9` | `c3220` | ebp | none | eax ecx edx |
| `c0b00` | `c0b63` | `c0b68` | `13b390` | ebx ebp esi edi | none | ecx edx |
| `c0b00` | `c0bc6` | `c0bcb` | `13b390` | eax ebx ebp esi edi | none | none |
| `c0b00` | `c0bd3` | `c0bd8` | `13bc00` | eax ebx ebp esi | none | none |
| `c0b00` | `c0c5d` | `c0c62` | `13bb90` | ebx ebp esi | none | ecx edx |
| `c0b00` | `c0c6f` | `c0c74` | `13bb90` | edi | none | eax ecx edx |
| `c2d00` | `c2d6c` | `c2d71` | `c0840` | ebx ebp edi | none | none |
| `c2d00` | `c2d86` | `c2d8b` | `b8c40` | eax ebx ebp edi | none | none |
| `c2d00` | `c2dac` | `c2db1` | `16640f` | al ebx esi edi | none | none |
| `c2d00` | `c2e02` | `c2e07` | `b8d30` | edi | none | none |
| `c2d00` | `c2ed8` | `c2edd` | `142570` | edx ebx ebp esi edi | none | none |
| `c2d00` | `c3040` | `c3045` | `c28b0` | ebx ebp edi | none | none |
| `c2d00` | `c307c` | `c3081` | `16a8e0` | ebx ebp esi edi | none | none |
| `c2d00` | `c309a` | `c309f` | `11bed0` | ebx ebp esi edi | none | none |
| `c2d00` | `c30bb` | `c30c0` | `ba300` | ebp esi edi | none | none |
| `c2d00` | `c30cd` | `c30d2` | `11bed0` | ebp esi edi | none | none |
| `c2d00` | `c30f0` | `c30f5` | `1cac60` | edi | none | eax ecx edx |
| `c3220` | `c324e` | `c3253` | `1cae40` | esi | none | eax ecx edx ebx ebp |
| `c3260` | `c3296` | `c329b` | `1cae40` | ebx ebp esi | none | ecx |
| `c3260` | `c32d5` | `c32da` | `16bc00` | eax bl ebp esi edi | none | none |
| `c3260` | `c3312` | `c3317` | `16ba40` | none | none | eax ecx edx |
| `c3260` | `c3328` | `c332d` | `16ba40` | none | none | eax ecx edx |

### Lines condensed on the page

Single lines whose names or values the page no longer carries.

- 1 health model's `before` and `after` on `OBJECT_DAMAGE_UPDATE` make
- dialogue's `before` on `UNIT_DIALOGUE_UPDATE`, and the Halo 1
- flashlight's and scope's `before`s on `UNIT_LIGHT_UPDATE`.
- slot `+4` with the object and action index. On return at `e6890`,
- dressing plus the post-update's fields: the state `+3dc`, the parent,
- component whose contact records (`+88`, `48` bytes each, `+8c` of them)
