# Simulation rewrite: subsystem sketches

Status: Partly built (phases 1 to 7 are partly built, see the table in [sim-rewrite.md](sim-rewrite.md)).
Switch: `impl.sim.native.<subsystem>`, listed in [switches.md](../runtime/switches.md).

What each phase of [sim-rewrite.md](sim-rewrite.md) takes over: its data, its entries, its guest boundary and its exactness bar. Effects (phase 6), the seats entries (phase 7) and replication stay in section 4 of that page.

| Phase | Subsystem | Detail page |
|---|---|---|
| 1 | Animation and pose | [anim-sampling.md](anim-sampling.md), [skeleton-walk.md](skeleton-walk.md), [animation-manager.md](animation-manager.md) |
| 2 | Collision and spatial queries | [sim-rewrite-collision.md](sim-rewrite-collision.md), [collision-queries.md](collision-queries.md) |
| 3 | Objects and units | [sim-rewrite-objects.md](sim-rewrite-objects.md), [object-update.md](object-update.md) |
| 4 | Sound | [sound-sim.md](sound-sim.md) |
| 5 | Havok world | [sim-rewrite-havok.md](sim-rewrite-havok.md), [havok-step.md](havok-step.md) |
| campaign | AI, pathfinding, HaloScript | [hs_vm.md](hs_vm.md) |

**Animation and pose (phase 1).**

- *Data.* Clips are decoded once into SoA per channel. Static frames are
  dense `[i16; 4]` or `[f32; 3]` runs. Keyframed channels become
  `times: [u16]` plus `values` with a per-channel key range. Flag arrays are
  replaced by precomputed node-to-slot maps. The output is the retail 32-byte
  local record, so scatter is a plain copy.
- *Passes.* Sample a mask of nodes in one call, with no codec table dispatch
  per channel and none of the evaluation globals `0x504460..0x5044c0`. Then
  build node matrices for a whole object in one SIMD pass over the parent
  order. Later, batch across all objects in the post-physics walk.
- *Parallelism.* Per object, once the walk is batched (phase 3).
- *Manager and graph (slice 1d).* The channel manager (`1c6440..1c7450`,
  `1cb000..1ce000`) and graph resolve (`1daea0`, `1dd9d0`, `1ddb40`,
  `1dd560`) cost 3.4% of the thread, as much as sampling and codecs. They
  come over once sampling and matrices are Rust: resolve becomes a table
  built per graph tag, and the per-tick channel advance runs over the
  object's channels in one call. The cache request `1236f0` and its RNG
  draw at `1dd6e1` keep retail order.
- *Benefits.* Theater and the web viewer sample the same store; Halo 1 models
  convert into the same codecs.

**Collision and spatial queries (phase 2).**

- *Data.* A Rust collision world per loaded BSP and per collision model:
  - surfaces, planes and leaves as flat arrays;
  - MOPP programs pre-decoded (the `mopp` crate already holds the interpreter
    as a query model and an offline compiler);
  - a decoded portal graph and a per-query traversal of the live object
    reference lists, preserving visit order and guest stamps.
- *Entries:*
  - collision query `1697c0`;
  - BSP segment `1de630` and `1de6d0`;
  - point to leaf `14a280`;
  - geometry test `11e5e0` at the whole query's object-reference call site;
  - float MOPP ray `3009e0` with its Havok path `244980` → `2df720` →
    `301800` (1.80% of the thread, no caller inside the world step); the
    native walk (`sim_collision::mopp::ray`) follows `301800` as a parity
    arm that counts matches and the first mismatch, while the guest's
    answer stands until it shows parity
    ([collision-queries.md](collision-queries.md), "Entry I");
  - the datum walk `b89b0`;
  - spatial query `14a370` and the cluster walk `14a5b0`.
- *Havok boxes.* The collision world answers the box wrapper `300150`.
  Its inner integer interpreter `2ff8b0` and contact-agent callers also
  belong to the Havok world-step census in phase 5.
- *Callers.* The observer camera asks for 32% of the queries and 82% of the
  ray casts (`16f280` → `170fd0` → `171830`), sound 19% (listener leaf
  `11bed0`, obstruction `221e30`), the object spatial marks 24% and aim
  assist 11%. So this phase also pays for most of the per-seat camera cost.
- *Exactness.* Bar E: hit choice and `t` must match bit for bit. That is
  feasible because the integer MOPP is exact by construction and the float
  paths are short SSE sequences.
- *Parallelism.* Little: queries arrive synchronously from guest callers. The
  win is cost per query, and later batched queries once phase 3 owns the
  callers.

**Objects and units update (phase 3).**

- *Orchestration.* The walk
  table `engine::sim_objects::native_walk` runs the pre-physics `b6f10`
  and post-physics `b7060` walks, `bc470` and `bc820` with `bd090` per
  object, the type dispatchers, the wake leaves, and the unit `+0x40`
  (`c6b30`), biped (`dd360`, `dd990`) and weapon (`fdfb0`) callbacks
  with their trees, and answers every segment; nothing is compared.
  Finalize `b7150` and the other types' callbacks (the item level
  `10b4d0`, vehicles, the small types) are guest calls through the runtime
  ([feature-engine.md](feature-engine.md), "Calling code that is still
  recompiled"); they join type by type. The one refusal is `bd090` or a
  type callback outside its float environment (`float_env`): the guest
  runs that call.
  `sim.arms` reports `objects.walk.<entry>`
  ([object-update.md](object-update.md#the-walks-in-rust)).
- *Also in this phase:* the object-function evaluator (`bab40`, `108d90`,
  `bcc20`) and lights (`c1400`, `c0c80`, which subsume
  `impl.sim.static-light`'s bypass); the markers `b8d30` belong to
  phase 1c ([markers.md](markers.md)).
- *The function evaluator.* `sim-objects/src/functions/evaluator.rs` is
  `bab40`, `ba8c0`, `108d90`, `bcc20` and `13bb90`'s output range as plain
  Rust over Rust-owned data, on the curve kernel `functions/curve.rs`:

  | Data | Owner | Lifetime |
  |---|---|---|
  | Exports and function blobs per definition | `Library` | decoded at first use; again when the count or table moves; dropped at map load |
  | Shape (8) and wave (12) tables, image constants and string IDs | `Library` | read at the map's first call |
  | Body fields, parent, type levels, world globals, game time | guest | read per call |

  It answers a call only when every step is Rust: the built-in names, the
  base level's own values, every matching export with its scale and gate,
  the parent. It declines, by reason, an override (body `+c0` bit 9), an
  import a type level or a base callout answers, curve kinds 9 and 10, an
  operand off the exact path, a fault and a recursion past 16.
  `engine::sim_objects::functions::arm` answers at each
  `bab40` entry the objects driver runs:

  ```
  entry -> evaluate -> declined?  -> guest answers                  (declined[reason])
                    -> else       -> Rust answers: out, al, ecx, xmm0  (answered)
  ```

  `sim.arms` reports it as `objects.function`.
- *Parallelism.* Pose and matrix passes run per object in hierarchy waves.

**Sound sim (phase 4).**

- *Why here.* 8.37% of the thread all-in plus 2.47% in the audio clock and
  DirectSound HLE, on every silent host. No float exactness problem beyond
  its x87 gain curves (0.13 ms), and its BSP queries arrive with phase 2.
- *Scope.* The source and listener update (`125b40`, `127400`, `1293f0`,
  `128ca0`, `129aa0`), game sound for objects and loops (`18a7b0`, `18c060`,
  `18d730`), gain and filter conversion (`2195f0`, `21fa80`, `21a250`), and
  environment and obstruction (`221a70`, `221da0`).
- *Guest boundary.* Sound records and the listener stay in guest layout.
  Voice start, pitch, completion order and the callback grid must match,
  because completion feeds the guest ([sim-budget.md](../perf/sim-budget.md),
  "Headless guest dependencies").
- *Implementation.* `sim-sound` owns the working data for a sound update,
  including listener axes, channel levels, track selection and obstruction
  weights. The engine adapters gather each set of inputs from the current
  guest state. They submit the results to the existing mixer and reload live
  records across callbacks. The driver fuses calls between sound bodies and
  uses the original stack at calls to other systems. Reverting an arena
  restores the persistent sound records directly.
- *Channel choice.* The census (`128500`), class ranking (`128b90`), pick
  (`128700`), pressure scan (`128920`) and per-class ceiling (`128a60`) run
  as bodies in `native_sound_sources/channels.rs`, deciding through
  `sim_sound::{sources::VoicePriority, scheduling::CappedSelection}`. Their
  synthetic differential cases are in `tests/sim_sound_differential/channel_choice.rs`.
- *Voice update.* `sim_sound::voices::Voices` owns the running voices: a
  one-shot plays out on `end_tick(start, ms)`, a loop (or flags `+4` without
  bits 9 and 6) never does, and a stop or a finish for another reason than
  2 interrupts it. Each tick it reports its starts in admission order and
  its play-outs in pool slot order, the order of the walk `128020`. The arm
  (`sim_sound/voice_arm.rs`) feeds it the admissions (`126000`, after), the
  stops (`126360`) and the finishes (`127320`) and compares per tick:

  | Tick | Turn | Counted |
  |---|---|---|
  | `tick % 64 == 0` | compare the whole report | `matched`, `mismatched`, `quiet` (no voices), `declined` (a voice it never saw start, or of unknown length) |
  | other, not proven | the update steps only | `skipped` |
  | other, proven (64 matched ticks with voices, no difference) | the update finishes its play-outs at the service `125b40`, then the guest's report must hold the rest | `answered`, `stale`, `mismatched` |

  Interruptions are counted (`interrupted`) and are inputs, never
  differences. One difference ends the answers for the process. The voices
  are saved with snapshots; the tick going back forgets them.
- *Verification.* Full sim reports with arena hashes include the ordered
  `sound.voices` component. Compare complete reports from the lifted and
  native arms with `h2 sim diff`. A quiet tick has an empty event list.
  The clock-only arm still submits the room and filter parameters and
  delivers any callbacks at the setters. [Sound reporting](../runtime/sim.md#finding-the-first-differing-state)
- *Hosts.* Per-voice DirectSound submissions still drive the silent clock. Replacing
  a submission requires preserving its completion and callback order.

**Havok world (phase 5).**

- *Data.* A Rust rigid-body world in SoA, with its own broadphase, contact
  manifolds against the phase-2 collision world, and a solver: in place of
  `1c3d30` → `311100` → `30fef0` → `316a20` → `31c100`, the welding and
  BV-tree agents (`2fa2f0`, `2faa60`), the convex agents (`2ee610`,
  `3048a0`), the integer MOPP box query (`2ff8b0`) and the game-side glue:
  object bodies `1ce000..1d5000`, structure shape callbacks
  `1edd00..1eff40` and character physics `1f19c0`, `1f2250`. By stage:
  convex agents 1.91% of the thread, weld agent 1.24%, solver 0.96%,
  character physics 0.80%, box query 0.79%, shape callbacks 0.76%, other
  library 2.61%, other glue 2.52%.
- *Guest boundary.* The guest-visible body and contact layout
  ([havok-runtime.md](havok-runtime.md)) stays the arena truth, written at
  step end. Islands are solved in parallel with fixed ordering.
- *Exactness.* Bar E by decision 8.1. Must also pass the superbounce
  regression and the vehicle, grenade and ragdoll behaviour cases. Havok
  holds 0.16 ms of x87 work that stays unless proven equal in double.
- *Risk.* The biggest win and the biggest risk. It opens up physics features
  and, eventually, a host with no Havok code at all.

**AI, actors, pathfinding and HaloScript (campaign; slot set by census B).**

- *HaloScript.* Under `impl.sim.native.hs` the [Rust VM](hs_vm.md#live-comparison)
  compares supported whole threads, scheduling order, expression results,
  global cells and builtin argument dispatch against the guest. The guest
  is authoritative. Promotion requires engine-call and suspension coverage
  plus campaign arena-hash parity through the verification ladder.
- *Pathfinding.* `crates/formats/pathfind` decodes and generates the data.
- *AI.* `AI_TARGET_SCORE` `265d30` is already a named hot point.
- *Order.* Set by the phase 0 campaign census, not guessed here.

## Seats and rules (phase 7): kernels

The entries of the seats table in section 4 of [sim-rewrite.md](sim-rewrite.md) and the kernels behind them.

*The aim-assist cone.* `1a50a0` is not a seat entry. The kernel
`sim-objects/src/aim_assist.rs` is the cone and its three callees (the zoom
magnification `101090`, the tracking test `1011d0`, the acquisition test
`101160`) as plain Rust over Rust-owned data. Only the zoom's x87 sequence
around the image's `pow` comes from the caller (`Magnify`), run on the x87
model.

| Data | Owner | Lifetime |
|---|---|---|
| The unit's two slots, each weapon's cone, class, flags and zoom range, the primary's trigger states and kinds | `Input` | copied from the guest per call |
| The image's 1.0, the x87 state the zoom's `pow` runs on | `Input`, the arm's x87 model | per call |

`engine::sim_seats::aim::arm` runs it as a shadow arm at each `1a50a0`
entry, behind `impl.sim.aim-assist` (off; installed by
`sim-bridge/src/present.rs`):

```
entry -> predict -> declined?  -> guest answers                  (declined[float, triggers, fault])
                 -> unproven or slot % 64 == 0
                               -> guest answers, compare at return (checked, matched, mismatched)
                 -> else       -> Rust answers: record, al, eax, ecx, edx, xmm0-3, x87, ret 4 (answered)
```

An answer leaves what retail leaves except the arithmetic flags, the x87
last-instruction pointers and the dead stack below the return: `ecx` is the
first bend (retail's `push ecx` slot holds it), `eax` and `edx` the values
the exit path last loaded. Proof is 4096 matches with no mismatch, per map;
a mismatch hands every call back to the guest until the next map.
`sim.arms` reports it as `seats.aim`. `aim_assist_matches_lifted`
(`engine/tests/sim_seats_differential.rs`) runs the arm's applied answer
against the lifted original with its real callees, `pow` included. The
register-level port stays as the differential oracle
`sim_seats::PARAMS_ORACLE`.

*The bound controller's device read.* At `185be0`'s bound branch
(`0x185c6c`, a seat whose port has a controller) the kernel
`engine/src/sim_seats/device.rs` reads the port as plain Rust over a copy
(`Device`) and stops where retail's next call is: the look curve
unblocked, the visibility walk blocked.

| Step | Retail | Kernel |
|---|---|---|
| Rates | port rates times `[0x45dc54]` | `rates` |
| Vehicle seat | `t = clamp((\|v\| - lo) / (hi - lo), 0, 1)`, then `pow(t, e)` unless `e == 0`; a rate pair with a positive end becomes `(a + (b - a) * t) * scale` | `vehicle` |
| Unblocked | the stick pair over its larger axis, `s = hypot`, each axis times `s` clamped to `[0x45dbcc]..1` | `steer` |
| Blocked | wanted buttons not pressed leave both halves; the rest get the block's state and hold | `select` |

| Data | Owner | Lifetime |
|---|---|---|
| The port's rates and 20-word block, the vehicle seat's fields and velocity, the blocked flags, the wanted buttons, five image constants | `Device` | copied from the guest per read |
| The x87 state the length, `pow`, magnitudes and hypot run on | the arm's x87 model (`Fpu`) | per read |

`engine::sim_seats::device::arm` runs it as a shadow arm inside the seat
body (no hook of its own; the proof resets at map load):

```
0x185c6c -> predict -> declined?  -> guest answers                   (declined[float, fault, unseen, alias])
                    -> unproven or slot % 64 == 0
                                  -> guest answers, compare at the curve's entry
                                     or the walk's return 0x1865c4   (checked, matched, mismatched)
                    -> else       -> Rust answers: frame, record, input slot, buttons,
                                     registers, x87, then the call    (answered)
```

At the curve's entry the audit compares every register, `xmm0`-`xmm7`, the
x87 stack and the frame; at the walk's return, the callee-saved registers,
the frame and the x87 control word. Both compare the record, the input slot
and the buttons. A blocked read that skips the walk has no transfer to
compare at, so its audit is declined as `unseen`; answered, it is laid down
like the rest. `sim.arms` reports it as `seats.device`.
`bound_control_matches_lifted` (`engine/tests/sim_seats_differential.rs`)
puts the applied answer against the lifted original with its real `24590`
and `pow`; `bound_control_audits_through_the_guest` runs the audit path.

*The leaf callees.* Eight guest functions the seat bodies call, which read
only what their registers name, are kernels in
`engine/src/sim_seats/leaf.rs`, each behind its own arm:

| Leaf | Guest | Returns to | Answer |
|---|---|---|---|
| `visible` | `e4050` | `0x1865c4` | the biped takes assisted aim: alive, look enabled, rank at or over the tick's threshold (`fistp` on the x87 model), standing or seated with no parent |
| `player_index` | `b67a0` | `0x1863a6` | the player's slot when the record's count holds it, else `-1` |
| `direction` | `11df30` | `0x1863f7`, `0x186403` | a vector's yaw and pitch (`fpatan`, `fsqrt` on the x87 model) |
| `user_test` | `14ddc0` | the HUD update's `USER_TESTED` | the user slot has a player |
| `user_player` | `14de70` | the HUD update's `PLAYER_FOUND` | the user slot's player |
| `submit` | `187b30` | `0x186a94` | the input record's flags into the control globals' held word, the latches and the record's latch bits, the yaw's sign (x87) and the axis clamps |
| `weapon_ready` | `100f70` | `0x1866b3` | the weapon can be readied: no magazine holds 1 to 6 rounds (its callee `100880`, in the same kernel) and the definition has a zoom level |
| `init_packet` | `c6ef0` | the action's `0x150673`, the neutral's `0x1507b4` (`sim_seats/control.rs`) | the whole `0x7c`-byte unit-control packet as a block: zeroed, the header word, the `-1` selections and words, the vector at `[4687a8]` in all three directions |

An effect carries up to six word stores and one block, a run of `0x7c`
bytes at an address, held inline (`Effect::block`, no allocation). With no
block the six-store path pays one tag test in `apply` and `at_site`. The
block is laid down first, in four-byte pieces, since a segment's store log
holds four bytes a store; the action segment's log is 64 entries and the
neutral's 48, room for the block and the body's own stores.

At the call the kernel predicts `eax`, `ecx`, `edx`, `xmm0`, the x87
stack, and the leaf's stores and block. Unproven, the guest's leaf runs
and the body compares at the return site; after 4096 matches the kernel
answers in place and the body goes on at the site in the same segment, one
call in 64 still the guest's. A mismatch returns that leaf to the guest
until the next map.

Declines are counted by name: `float` (the float environment, an
operand, or the direction flag set under `init_packet`'s `rep stosd`) and
`fault` (a read that faults, a store that would, a `weapon_ready` magazine
count over `0x7fff`, which `100880`'s 16-bit index would wrap, or an
`init_packet` packet over the vector or its pointer, which the guest reads
after its first stores). An answer leaves what retail leaves except the arithmetic
flags and the dead frame below the return. `sim.arms` reports
`seats.leaf.<name>`; `seat_leaves_match_lifted` and
`weapon_ready_matches_lifted` (magazine counts, rounds on both sides of 1
to 6, zoom levels) put each applied answer against the lifted function,
and `weapon_ready_declines_a_wrapping_count` and
`init_packet_declines_an_overlap` the declines.

The bodies' other callees (the look curve and the bodies that are seats
themselves aside) call further guest code and stay guest calls.

*The respawn gate.* `15dd40` (`PLAYER_RESPAWN_READY`, `eax` the slot, `al`
the answer) is called by `players_before` for each candidate. Unlike the
leaves it has its own shadow arm at its entry
(`engine/src/sim_seats/respawn.rs`, `impl.sim.respawn-ready`, off), with
its callees in Rust: `158a50` (takes part: flags `+2`, `+0xc0`, unit slot
`+0x2c`, then `15db30`, out of lives: the lives setting
`[[4e6948] + 0x1b8]`, no unit, `+0x1ac` zero) and `15b2f0` (a local user
mapped, `[game + 0x6c] == 1`, and a client or `[game + 0xc04] == 1`). The
body answers no on a negative timer `+0x164`, and on a zero timer takes the
staggered check (ticks since `[game + 0x70]` over 3, their remainder by 32
against the slot's).

A timer above zero is the countdown (`respawn/countdown.rs`):

| Step | Function | What it does |
|---|---|---|
| 1 | `15dd40` | on the ticks at 3, 2 and 1 seconds (`(i16)[[510c54] + 2]` ticks each) the event kind `0x14`, on the last tick `0x15`: the death event built on the dead stack |
| 2 | `19eb90` | nothing on a client (mode 4); else `a7c50` and `19eb30` |
| 3 | `a7c50` -> `b5a70` | mode 5 only; returns unless the network state `[[4cf77c] + 8]` is 4 or 5 |
| 4 | `19eb30` -> `19de80` | the event's player, by its subjects in order (player `+0xc`, `+0x14`, team `+0x10`, `+0x18`, anyone) |
| 5 | `19df10` | the first matching row of the event type's block in the globals tag (`[[4e034c] + 0x16c]`, two jump tables) |
| 6 | `19e890` | returns for a player with no local user (`+0x28` is `-1`), and once `[[4e6948] + 0x1128]` is set for all but type 0 kind `0x13` |
| 7 | `15de0a` | timer minus 1, the fade `+0x170` (seconds left, held to `0..=0x3ff`) |
| 8 | `a7840` -> `b58c0` | the player's entity `[game + 0x2c + 4 * slot]`: in network state 4, when its row is replicated, `0x80` ORed into its update word |
| 9 | `15de51` | a timer now at zero takes the staggered check |

The kernel reads through `Mem`, which holds the call's own writes: the
event (scratch, never applied), and the timer, the fade and the update word
(live). The live words ride in the prediction, land in `apply` after each
address is written back as it is (so a fault lands none), and an audit
compares them after the guest's return. Registers are left as retail
leaves them.

The paths that reach further guest systems are declined by name and the
guest runs the call:

| Decline | Where | The guest goes on to |
|---|---|---|
| `post` | `b5a70`, network state 4 or 5 | `8b4d0`, the event message (virtual sends, an allocator) |
| `announce` | `19e890`, a local player | the HUD text (`1a0180`, `19e0f0`, `24caac`, `24cbee`, `24cc73`) and the announcer `19e770` |
| `every-player` | `19eb30`, event player `-1` | the iterator `19f240`; `15dd40` never builds such an event |
| `fault` | any read, or a seconds divisor of 0 | the whole call |

So the arm does not replace the guest function (`replaces_guest`): on a
networked host and for a local player the event ticks stay guest. `sim.arms`
reports `seats.respawn`. `respawn_ready_and_its_callees_match_lifted` puts
the body and each callee against the lifted functions, and checks each
decline by the original reaching the decline's address.

*The pickup scan.* `1508e0` (`PLAYER_PICKUP_SCAN`, `ret 4`, the argument a
player index) is called by `players_after` for each player. Its shadow arm
(`engine/src/sim_seats/player_objects.rs`, `impl.sim.player-objects`, off)
covers the whole body except the pickup call:

| Part | Guest | Rust |
|---|---|---|
| no unit, or the unit's object has a parent | return | the two exits |
| object query, capacity 16 | `bb050`, `14a5b0`, `14a370`, `23a160`, `b8940`, `b8a10` | `sim_collision::plan_visit_at` (the entry H kernel, planned with the arguments the scan pushes) and `visit_writes` |
| type 2 or 3 | `150e30` | `candidate_exits`: no unit, a parent that is not a machine or crate, the object held by the unit (`+0x14c`), or out of reach |
| type 7 or 11 | the `+0x10` / `+0xc` link walk, up to 16 handles | the same walk |

`eax`, `ecx` and `edx` are the scan's callee scratch and are not reproduced
(`players_after` reloads them); the callee-saved registers and `esp` are, and
the return compare reads back every word the query writes (the flood epoch,
the cluster and object marks, the generation and the two active bytes).

The arm refuses by name and the guest answers: `visit` (the collision kernel
declines the plan), `candidate` (a type 2 or 3 handle: the Halo 1 prompt
feature has a `before` hook on `150e30` that sees every call, so skipping
even an early-exit call changes what it records), `pickup` (a `150e30` past
its early exits, which loops the unit's seats through `ffc60`, posts events
and calls `badc0`, `ccba0`, `ccb80`, `138880`, `151ea0`, `cd7b0`, `150dd0`,
`a8f10` and `15e8e0`; only reachable with `Candidates::Port`) and `fault`.
So the arm is not `replaces_guest`. `sim.arms` reports `seats.player-objects`;
`player_objects_match_lifted` puts the body, the query writes and
`candidate_exits` against the lifted functions.
