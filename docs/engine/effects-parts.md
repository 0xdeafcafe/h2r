# The effects phase in Rust: the effect walk and the part arms

Status: Partly built. The walk, its four per-effect callees, an arm for every part class the switch handles (damage, object, sound, character, `coln`, decal, light), the returned arms and the scene events are written; nothing draws the scene events yet.
Code: `crates/rewrites/sim-effects` (pure), `crates/rewrites/engine/src/effects_phase.rs` (the walk over the guest), `crates/rewrites/engine/src/effect_update` (the per-effect bodies and their arm), `crates/rewrites/sim-bridge/src/sim_effects_parts.rs` (the part switch), `crates/rewrites/sim-bridge/src/effect_update.rs` (the bodies' hooks). Tests: `effects_phase_ledger_is_zero` (`engine::sim_tick`), `visual_events_leave_hash_unchanged` and `object_and_sound_parts_answer_after_their_own_proof` (`sim_effects::dispatch`), `the_object_answer_calls_the_leaves_in_retails_order`, `the_sound_answer_makes_retails_call`, `the_character_answer_pushes_as_retail`, `the_decal_answer_draws_between_the_leaves` and `the_light_answer_makes_retails_call` (`sim_bridge::sim_effects_parts`), `the_other_arms_answer_after_their_own_proof` (`sim_effects::dispatch`), `a_draw_is_retails_lcg_and_scale` (`sim_effects::random`), `damage_parts_match_lifted` (`crates/rewrites/engine/tests/sim_effects_differential.rs`), the per-effect bodies' tests (`engine::effect_update::tests`), `the_switch_installs_every_body_with_its_sites_and_the_resets` and `the_native_effects_port_keeps_the_update` (`sim_bridge::effect_update`).
Switch: none. The effects phase always runs in Rust (`SimEffectsParts` is always on, `sim_bridge::tick::BODIES`); a part an arm declines is a named, counted refusal that runs the guest's arm for that part. The differential tests call the lifted phase directly as the reference.

The tick driver runs the effects phase (phase 15, `0x176e40`) as a Rust body, so `sim.ledger` counts no guest call for it and `tick-ledger.toml` allows 28 guest phases. Every part stays exact: each class's arm is compared with the guest until proven, and the draws keep their order. Visual parts become events in Rust for the renderer's scene, outside the arena. No RNG draw changes place.

## How it works

```text
tick driver ── phase 15 ──> effects_phase::run (Rust)
                              │ per live effect, in index order
                              ├─ 0x1794e0  alive?        body ──> 0x1774f0 when the object is gone
                              ├─ 0x179730  culled?       body
                              ├─ 0x1774f0  delete        body    (flag bit 6, lifetime -1)
                              └─ 0x1792b0  events        body ──> part switch 0x17a8a0
                                 (segment arm: compare until proven, then answer)
                                                                    │
                    sim_effects_parts::enter (Rust) <───────────────┘
                     ├─ jpt!              damage arm (audit or answer)
                     ├─ obje              object arm (audit or answer)
                     ├─ snd!              sound arm  (audit or answer)
                     ├─ char, coln        character and coln arms (audit or answer)
                     ├─ deca, ligh        decal and light arms (audit or answer) + scene event
                     ├─ lens, MGS2, tdtl  return from Rust, flare event
                     └─ others            guest arm (named refusal)
```

1. **The walk** (`sim_effects::walk`, run by `engine::effects_phase::run`). It reads the tick length once (`[[0x510c54] + 4]`), then visits the live slots of the array at `[0x4ea93c]` by the bitmap at `+0x48`, re-reading the header after each effect as retail does. The four callees get retail's registers: `EBX` = handle for each, `EAX` = handle for the delete, the handle and the tick length on the stack for the alive test and the update.
2. **The part switch** (`sim_bridge::sim_effects_parts::enter`). An implementation of `EFFECT_PART_CREATE` decodes the effect, the part record and the three location words, and `sim_effects::dispatch::Parts::enter` decides.
3. **The arms** (`sim_effects::arm`). Each class has its own `Arm` over its own prediction, so one arm's parts never move another's audit slot and one arm's mismatch leaves the others answering. Until an arm has matched 4096 parts the guest's arm runs and observers at its leaves (calls from the switch only, by return address) compare. After that Rust answers, from the switch's depth (entry `ESP - 0x110`, the four saved registers written where the switch saves them), and one answerable part in 64 is still the guest's and compared. One difference returns that arm's parts to the guest until the next map load.
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
7. **The character arm** (`sim_effects::character`). The tag `-1`, or a node word with its top bit set other than `0xffff`, returns from Rust with retail's `EAX`. Otherwise `0x1469f0([effect+0x64])` (seconds to ticks), called with the other five arguments already pushed under it as retail pushes them, then `0x1ca290(ticks, [effect+0x48], node, [part+0x14], [part+0x18], location+8)` with `ESI` the tag. The observer on `EFFECT_PART_CHARACTER` (from `0x17a942`) compares the second call; the switch's return compares an early return's `EAX`.
8. **The `coln` arm** (`sim_effects::coln`). One draw from word `+4` (`0x259d0` with the part's `+0x28` and `+0x2c`), the value dropped. Rust makes it in the switch's entry on the guest's own x87 state (step 10), writes the seed and returns with retail's `EAX`, `ECX` and `EDX`. The switch's return compares the seed, `EAX` and the x87 registers, status word and tags.
9. **The decal arm** (`sim_effects::decal`). `S` is entry `ESP - 0x110`, `R` the stream pointer.

   | Step | Leaf | Rust's part |
   |---|---|---|
   | 1 | `0x172750` when the part's byte `+6` has bit 4 clear (`EAX` 1, `EDI` the position, `XMM0` `[0x45dc08]`) | `AL` 0: a draw from `R+4`; return unless `[0x45dc20]` is above it |
   | 2 | `0x177d10(R+4, direction, S+0xf8, S+0x104, five part words)`, which draws from `R+4` | |
   | 3 | | a draw from `R+4`, `[part+0x28]` to `[part+0x2c]`, stored as an `f32` |
   | 4 | `0x17e670(tag, S+0x104, the f32, 0, -1, 0)`, `ECX` `[0x510c78]` | |

   Rust's draws run on a copy of the entry's x87 state, and the arm answers only when no x87 exception is pending and all six are masked. The audit is staged: `EFFECT_PART_PLACE`'s entry (from `0x17aad9`) checks the stream after the gate and the call, its return predicts step 3, `DECAL_CREATE`'s entry (from `0x17ab10`) checks the stream and the call, and the switch's return checks a part the gate stopped.
10. **The draws** (`sim_effects::random`). `0x259a0` and `0x259d0` are the LCG `seed * 0x19660d + 0x3c6ef35f` and `fild(seed >> 16) * [0x45dc50]`, scaled to the range. Rust runs them with the recompiler's x87 helpers (`x87::abi`), the same code the lifted functions call.
11. **The light arm** (`sim_effects::light`). One call to `0xc0350` with the scale in `XMM0`: on the object (the tag, the object, its node, location `+0x24`) or at a point (the tag, -1, -1, the source). The observer on `LIGHT_CREATE` (from `0x17acb0` and `0x17ace2`) compares it.
12. **The per-effect bodies** (`engine::effect_update`): see below.
13. **Scene events** (`sim_effects::scene`). Decal, light and flare parts are queued in a Rust queue (4096, oldest dropped and counted). The frame step drains it into `FrameScene::part_events` at each seal (`render_bridge::scene_step::seal`).

## The per-effect bodies

The walk's four callees are Rust bodies (`sim_objects::Body`): an entry and the return addresses of its guest calls, run from either to its next transfer on the retail frame, so the runtime can stop at any callout and resume at its site.

| Entry | Body | Its guest calls |
|---|---|---|
| `0x1794e0` | `alive`: the effect's object (`+48`): gone, delete and return true; else its offset and marker, the function values `+6c`, `+70` and colour `+24`, the end, stop or restart by the definition's bit 0 and the effect's bits 2 and 4, and the light record `+104`, `+108` | `badc0`, `baf80`, `1794a0`, `bab40` twice, `bad50`, `177260`, `177610`, `177310`, `3ddd0`; `1774f0` is a body |
| `0x179730` | `culled`: the visibility bit 3 of `+2` from the cluster, split-screen and the definition's bit 2; turning visible may end the effect | `3eb20`, `11c080` or `11c050`, `177610` |
| `0x1774f0` | `delete`: the salt check, the attachment's bit, the three unlinks, a tail jump to the free | `177590`, `1771a0`, `1773a0`, `177460`, `16ba40` |
| `0x1792b0` | `update`: up to 8 passes of the event clock `+60` towards `+64`; in an event the emitters, locations and the next event; at a delay's end the event's start, its length `+64` fixed or drawn, its parts and beams | `179020`, `179fb0`, `179190`, `1782a0`, `177610`, `177c20`, `259d0`, `17a380`, `17add0` |

Every segment's flags are its last flag-setting instruction's, so a callee that reads them, and the return, see retail's. The draw `259d0` stays the guest's, called where retail calls it; Rust stores its `fstp` with the recompiler's x87 helpers. No draw moves.

Rust answers these four segments (`effect_update::arm`), fusing from `1794e0` into `1774f0` when that turn is an answer too. An unsupported float environment is counted as `declined`.

The effect ports always install and port `1792b0` too (`engine::sim_effects::events`); that port keeps the point and the update body is not installed.

## Interfaces

| Point | Address | Hook | Order |
|---|---|---|---|
| `GAME_TICK` | `0x137fe0` | the tick driver's implementation, now with the effects body (`sim_tick::Bodies`) | unchanged |
| `EFFECT_PART_CREATE` | `0x17a8a0` | `implement` "effect part arm" | the only implementation; `host_audit`'s observers run outside it |
| `DAMAGE_AREA_APPLY` | `0xd6c80` | `observe_before` "effect damage arm: compare" | the point's only hook |
| `EFFECT_PART_CREATE` | `0x17a8a0` | `observe_after` "effect part arms: compare at the return" | runs after the implementation through the chain's shadow |
| `EFFECT_PART_PLACE` | `0x177d10` | `observe_before` "effect object and decal arms: compare the placing", `observe_after` "effect object and decal arms: predict the sums and the draw" | the point's only hooks |
| `EFFECT_PART_VELOCITY` | `0x177f60` | `observe_before` "effect object arm: compare the velocity" | the point's only hook |
| `SOUND_START_ON_OBJECT` | `0x189060` | `observe_before` "effect sound arm: compare on the object" | the point's only hook |
| `SOUND_START_AT_POINT` | `0x1895f0` | `observe_before` "effect sound arm: compare at a point" | the point's only hook |
| `EFFECT_PART_CHARACTER` | `0x1ca290` | `observe_before` "effect character arm: compare" | the point's only hook |
| `DECAL_CREATE` | `0x17e670` | `observe_before` "effect decal arm: compare the create" | the point's only hook |
| `LIGHT_CREATE` | `0xc0350` | `observe_before` "effect light arm: compare" | the point's only hook |
| `CACHE_TAGS_LOAD` | | `observe_before` "effect part arm: reset" | beside the other arms' resets; observers need no order |
| `EFFECT_ALIVE`, `EFFECT_CULLED`, `EFFECT_DELETE`, `EFFECT_UPDATE` | `0x1794e0`, `0x179730`, `0x1774f0`, `0x1792b0` | `implement` "effect bodies", with the sites as resumes | the only implementation; guarded when another feature hooks the point |
| `CACHE_TAGS_LOAD`, `GAME_STATE_REVERT`, `GAME_STATE_RESTORE` | | `observe_before` "effect bodies: reset the arm", "effect bodies: slots at a revert", "effect bodies: slots at a restore" | last |

`sim.arms` reports the subsystem `effects-parts`: `effects.damage`, `effects.object`, `effects.sound`, `effects.character`, `effects.coln`, `effects.decal` and `effects.light` (each arm's counters) `effects.parts` (`answered` the parts returned from Rust, `declined` the refusals), and `effects.body.<entry>` for each per-effect body (faulted, stopped and yielded guest segments count as `declined`).

## Rules and limits

- **RNG order.** Unchanged. The damage, sound, light, character and returned arms draw nothing in the switch. The object arm's draws (word 0) are inside `0x177d10` and `0x177f60`, which Rust's answer calls in retail's order. The `coln` and decal draws (word `+4`) are Rust's, made where retail makes them: the decal's gate draw before `0x177d10`, which draws from `+4` too, and its range draw after it.
- **Refusals** are named counters in `sim_effects::dispatch::Stats`: `guest_other` (a group with no arm) and `declined_fault` (an unreadable input, or arithmetic that could raise an unmasked exception). The walk's are in `sim_effects::walk::Stats` (`declined_fault`: the array header could not be read, so the driver calls the guest phase).
- **What an answered part leaves different.** The return address under each leaf's frame is the driver's, not the switch's; it is dead stack below the caller's `ESP`, outside the arena. `0xd6660`'s hooks do not see effect parts once the damage arm answers (the one hook there, `h1-projectiles`, acts only on the projectile's call). The object and sound arms' Rust float operations leave MXCSR's sticky flags as they were, where retail's `addss`, `subss` and `mulss` may set them; nothing in the sim reads them. The point sound's answer does not call `0x188dd0`. A `coln` answer leaves the x87 state exactly as retail's; a decal answer's draws run on a copy, so the guest's x87 status word and freed registers stay as the leaves left them, where retail's draws set the precision flag and leave their values there. The return through the chain's shadow keeps the part switch off the pipelined render thread.
- **The per-effect bodies** answer with retail's registers, flags, x87 and XMM state at each transfer and retail's stores; the proof is the guest's segments compared byte for byte, not a model of the callees.
- **The walk** leaves the phase's own frame (saved registers, the tick length) unwritten below the driver's stack, and the callees' return addresses are the driver's.
- **The proof is per map.** A map load resets the arms and empties the scene queue.

## Evidence

- The switch's arms, the record layout and the frame offsets: disassembly of `0x17a8a0`, `0xd6660`, `0x188dd0`, `0x259a0`, `0x259d0`, `0x1ca290` and `0x176e40` (`h2 disasm`).
- The per-effect bodies: disassembly of `0x1794e0`, `0x179730`, `0x1774f0` and `0x1792b0` (`h2 disasm`).
- `damage_parts_match_lifted` runs the lifted `0x17a8a0` on the interpreter over 64 seeded parts and compares the record it passes to `0xd6c80`, its place in the frame and the call's depth.

## Not verified

- Whole-run parity: `h2 sim diff` of `docs/runtime/sim-examples/lockout-2v2.toml` against the lifted build.
- That each arm reaches 4096 matched parts on a real match, and its timing; likewise each per-effect body's 4096 matched segments (`sim.arms` `effects.body.*`).
- The object, sound, character, decal and light answers run against the lifted leaves: the bridge tests drive them on a fake guest only. The `coln` answer's x87 state against the lifted `0x259d0`'s.

## Related

[sim-effects.md](sim-effects.md) (the exact ports under the phase), [sim-rewrite.md](sim-rewrite.md), [engine-map.md](engine-map.md).
