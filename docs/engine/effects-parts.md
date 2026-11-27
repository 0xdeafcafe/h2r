# The effects phase in Rust: the effect walk and the part arms

Status: Partly built. The walk, the damage, object and sound arms, the returned arms and the scene events are written; decal, light, character and `coln` parts still run the guest's arm, and nothing draws the scene events yet.
Code: `crates/rewrites/sim-effects` (pure), `crates/rewrites/engine/src/effects_phase.rs` (the walk over the guest), `crates/rewrites/sim-bridge/src/sim_effects_parts.rs` (the part switch). Tests: `effects_phase_ledger_is_zero` (`engine::sim_tick`), `visual_events_leave_hash_unchanged` and `object_and_sound_parts_answer_after_their_own_proof` (`sim_effects::dispatch`), `the_object_answer_calls_the_leaves_in_retails_order` and `the_sound_answer_makes_retails_call` (`sim_bridge::sim_effects_parts`), `damage_parts_match_lifted` (`crates/rewrites/engine/tests/sim_effects_differential.rs`).
Switch: `impl.sim.effects-parts` (on), listed in [switches.md](../runtime/switches.md). Off, the guest runs the phase and every part: the A arm of `h2 sim diff`.

The tick driver runs the effects phase (phase 15, `0x176e40`) as a Rust body, so `sim.ledger` counts no guest call for it and `tick-ledger.toml` allows 28 guest phases. Parts that change the sim (damage, spawned objects, sounds) stay exact: each of their arms is compared with the guest until proven, and every other sim part is still the guest's. Visual parts become events in Rust for the renderer's scene, outside the arena. No RNG draw changes place.

## How it works

```text
tick driver ── phase 15 ──> effects_phase::run (Rust)
                              │ per live effect, in index order
                              ├─ 0x1794e0  alive?        guest
                              ├─ 0x179730  culled?       guest
                              ├─ 0x1774f0  retire        guest   (flag bit 6, lifetime -1)
                              └─ 0x1792b0  events        guest ──> part switch 0x17a8a0
                                                                    │
                    sim_effects_parts::enter (Rust) <───────────────┘
                     ├─ jpt!              damage arm (audit or answer)
                     ├─ obje              object arm (audit or answer)
                     ├─ snd!              sound arm  (audit or answer)
                     ├─ lens, MGS2, tdtl  return from Rust, flare event
                     ├─ deca, ligh        guest arm + scene event
                     └─ char, coln, others   guest arm (named refusal)
```

1. **The walk** (`sim_effects::walk`, run by `engine::effects_phase::run`). It reads the tick length once (`[[0x510c54] + 4]`), then visits the live slots of the array at `[0x4ea93c]` by the bitmap at `+0x48`, re-reading the header after each effect as retail does. The four callees get retail's registers: `EBX` = handle for each, `EAX` = handle for the delete, the handle and the tick length on the stack for the alive test and the update.
2. **The part switch** (`sim_bridge::sim_effects_parts::enter`). An implementation of `EFFECT_PART_CREATE` decodes the effect, the part record and the three location words, and `sim_effects::dispatch::Parts::enter` decides.
3. **The arms** (`sim_effects::arm`). Each of `jpt!`, `obje` and `snd!` has its own `Arm` over its own prediction, so one arm's parts never move another's audit slot and one arm's mismatch leaves the others answering. Until an arm has matched 4096 parts the guest's arm runs and observers at its leaves (calls from the switch only, by return address) compare. After that Rust answers, from the switch's depth (entry `ESP - 0x110`, the four saved registers written where the switch saves them), and one answerable part in 64 is still the guest's and compared. One difference returns that arm's parts to the guest until the next map load.
4. **The damage arm** (`sim_effects::damage`). `damage::record` builds the `0x88`-byte record the `jpt!` arm passes to the area damage `0xd6c80`: `0xd6660`'s clear and empty fields, then the effect's owner (`+0x4c..+0x58`), `+0x1c`, `+0x20`, the position twice, the direction twice and the scale. The observer on `DAMAGE_AREA_APPLY` (from `0x17aa4f`) compares the record and the excepted object. An answer writes the record where retail's frame holds it (entry `ESP - 0xdc`) and calls `0xd6c80(record, -1)`.
5. **The object arm** (`sim_effects::object`). Retail's arm calls seven leaves, and both of its random draws are inside two of them, so Rust's answer makes the same calls in the same order with retail's registers and arguments:

   | Step | Leaf | Rust's part |
   |---|---|---|
   | 1 | `0xb7930(tag, object, effect+0x4c)`, `ESI` = the placement at `S+0x34` | |
   | 2 | `0xa7640` (`EAX` = placement); `AL` 0 returns | |
   | 3 | | copies the position, direction and source vectors to `S+0x50` |
   | 4 | `0x177d10` (draws), 9 arguments, writes `S+0x74` | |
   | 5 | | `S+0x74..0x80` += the effect's `+0x30` vector, `addss` under the entry MXCSR |
   | 6 | `0x177f60` (draws), 4 arguments, `XMM0` the last sum | |
   | 7 | `0xb7b40(placement)`, `-1` returns; `0xbb950(1, 0)`; `0xa7870(object)` | |

   `S` is entry `ESP - 0x110`. The answer reads the vectors, the stream pointer `[0x4e7408]` and the part words after step 2, where retail reads them. The audit is staged: `EFFECT_PART_PLACE`'s entry (from `0x17abe7`) checks the copies, arguments and registers; its return predicts the sums from what it placed; `EFFECT_PART_VELOCITY`'s entry (from `0x17ac3f`) checks the sums, arguments and registers. A part under an MXCSR with an exception unmasked is refused (`declined_fault`).
6. **The sound arm** (`sim_effects::sound`). One leaf call, no draw. With an object (`[effect+0x48]` not -1) and `DL` 0 it is `0x189060` with the location's `+0xc` and `+0x30`, the tag, the object, its node and the scale. Otherwise it is `0x1895f0` with a `0x24`-byte record at entry `ESP - 0x100`: the position, the direction packed by `0x188dd0` (ported exactly: the clamp, `subss` and `mulss` under the entry MXCSR, `fistp` under the x87 control word), the vector at `[0x4687a4]`, the effect's `+0x1c` and `+0x20`; the scale in `XMM0`. The observers on `SOUND_START_ON_OBJECT` (from `0x17ad29`) and `SOUND_START_AT_POINT` (from `0x17ad9f`) compare the call.
7. **Scene events** (`sim_effects::scene`). Decal, light and flare parts are queued in a Rust queue (4096, oldest dropped and counted). The frame step drains it into `FrameScene::part_events` at each seal (`render_bridge::scene_step::seal`).

## Interfaces

| Point | Address | Hook | Order |
|---|---|---|---|
| `GAME_TICK` | `0x137fe0` | the tick driver's implementation, now with the effects body (`sim_tick::Bodies`) | unchanged |
| `EFFECT_PART_CREATE` | `0x17a8a0` | `implement` "effect part arm" | the only implementation; `host_audit`'s observers run outside it |
| `DAMAGE_AREA_APPLY` | `0xd6c80` | `observe_before` "effect damage arm: compare" | the point's only hook |
| `EFFECT_PART_PLACE` | `0x177d10` | `observe_before` "effect object arm: compare the placing", `observe_after` "effect object arm: predict the sums" | the point's only hooks |
| `EFFECT_PART_VELOCITY` | `0x177f60` | `observe_before` "effect object arm: compare the velocity" | the point's only hook |
| `SOUND_START_ON_OBJECT` | `0x189060` | `observe_before` "effect sound arm: compare on the object" | the point's only hook |
| `SOUND_START_AT_POINT` | `0x1895f0` | `observe_before` "effect sound arm: compare at a point" | the point's only hook |
| `CACHE_TAGS_LOAD` | | `observe_before` "effect part arm: reset" | beside the other arms' resets; observers need no order |

`sim.arms` reports the subsystem `effects-parts`: `effects.damage`, `effects.object` and `effects.sound` (each arm's counters) and `effects.parts` (`answered` the parts returned from Rust, `declined` the refusals).

## Rules and limits

- **RNG order.** Unchanged. The damage, sound and returned arms draw nothing. The object arm's draws (word 0) are inside `0x177d10` and `0x177f60`, which Rust's answer calls in retail's order. The other arms that draw (decal and `coln`: word +4) still run in the guest.
- **Refusals** are named counters in `sim_effects::dispatch::Stats`: `guest_decal`, `guest_light`, `guest_character`, `guest_coln`, `guest_other`, `declined_fault`. The walk's are in `sim_effects::walk::Stats` (`declined_fault`: the array header could not be read, so the driver calls the guest phase).
- **What an answered part leaves different.** The return address under each leaf's frame is the driver's, not the switch's; it is dead stack below the caller's `ESP`, outside the arena. `0xd6660`'s hooks do not see effect parts once the damage arm answers (the one hook there, `h1-projectiles`, acts only on the projectile's call). The object and sound arms' Rust float operations leave MXCSR's sticky flags as they were, where retail's `addss`, `subss` and `mulss` may set them; nothing in the sim reads them. The point sound's answer does not call `0x188dd0`.
- **The walk** leaves the phase's own frame (saved registers, the tick length) unwritten below the driver's stack, and the callees' return addresses are the driver's.
- **The proof is per map.** A map load resets the arm and empties the scene queue.

## Evidence

- The switch's arms, the record layout and the frame offsets: disassembly of `0x17a8a0`, `0xd6660`, `0x188dd0` and `0x176e40` (`h2 disasm`).
- `damage_parts_match_lifted` runs the lifted `0x17a8a0` on the interpreter over 64 seeded parts and compares the record it passes to `0xd6c80`, its place in the frame and the call's depth.

## Not verified

- Whole-run parity: `h2 sim run docs/runtime/sim-examples/lockout-2v2.toml` with `H2_FEATURES=-impl.sim.effects-parts` and without, then `h2 sim diff` of the two reports.
- That each arm reaches 4096 matched parts on a real match, and its timing.
- The object and sound answers run against the lifted leaves: the bridge tests drive them on a fake guest only.

## Related

[sim-effects.md](sim-effects.md) (the exact ports under the phase), [sim-rewrite.md](sim-rewrite.md), [engine-map.md](engine-map.md).
