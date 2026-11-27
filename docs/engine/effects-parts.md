# The effects phase in Rust: the effect walk and the part arms

Status: Partly built. The walk, the damage arm, the returned arms and the scene events are written; object, sound, decal, light, character and `coln` parts still run the guest's arm, and nothing draws the scene events yet.
Code: `crates/rewrites/sim-effects` (pure), `crates/rewrites/engine/src/effects_phase.rs` (the walk over the guest), `crates/rewrites/sim-bridge/src/sim_effects_parts.rs` (the part switch). Tests: `effects_phase_ledger_is_zero` (`engine::sim_tick`), `visual_events_leave_hash_unchanged` (`sim_effects::dispatch`), `damage_parts_match_lifted` (`crates/rewrites/engine/tests/sim_effects_differential.rs`).
Switch: `impl.sim.effects-parts` (on), listed in [switches.md](../runtime/switches.md). Off, the guest runs the phase and every part: the A arm of `h2 sim diff`.

The tick driver runs the effects phase (phase 15, `0x176e40`) as a Rust body, so `sim.ledger` counts no guest call for it and `tick-ledger.toml` allows 28 guest phases. Parts that change the sim (damage, impulses, spawned objects) stay exact: the damage arm is compared with the guest until proven, and every other sim part is still the guest's. Visual parts become events in Rust for the renderer's scene, outside the arena. No RNG draw changes place.

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
                     ├─ lens, MGS2, tdtl  return from Rust, flare event
                     ├─ deca, ligh        guest arm + scene event
                     └─ obje, snd!, char, coln, others   guest arm (named refusal)
```

1. **The walk** (`sim_effects::walk`, run by `engine::effects_phase::run`). It reads the tick length once (`[[0x510c54] + 4]`), then visits the live slots of the array at `[0x4ea93c]` by the bitmap at `+0x48`, re-reading the header after each effect as retail does. The four callees get retail's registers: `EBX` = handle for each, `EAX` = handle for the delete, the handle and the tick length on the stack for the alive test and the update.
2. **The part switch** (`sim_bridge::sim_effects_parts::enter`). An implementation of `EFFECT_PART_CREATE` decodes the effect, the part record and the three location words, and `sim_effects::dispatch::Parts::enter` decides.
3. **The damage arm** (`sim_effects::damage`, `sim_effects::arm`). `damage::record` builds the `0x88`-byte record the `jpt!` arm passes to the area damage `0xd6c80`: `0xd6660`'s clear and empty fields, then the effect's owner (`+0x4c..+0x58`), `+0x1c`, `+0x20`, the position twice, the direction twice and the scale. Until the arm has matched 4096 parts, the guest's arm runs and an observer on `DAMAGE_AREA_APPLY` (calls from `0x17aa4f` only) compares the record and the excepted object. After that Rust answers: it writes the record where retail's frame holds it (entry `ESP - 0xdc`), the four registers the arm saves, and calls `0xd6c80(record, -1)` at the arm's stack depth. One answerable part in 64 is still the guest's and compared. One difference returns every damage part to the guest until the next map load.
4. **Scene events** (`sim_effects::scene`). Decal, light and flare parts are queued in a Rust queue (4096, oldest dropped and counted). The frame step drains it into `FrameScene::part_events` at each seal (`render_bridge::scene_step::seal`).

## Interfaces

| Point | Address | Hook | Order |
|---|---|---|---|
| `GAME_TICK` | `0x137fe0` | the tick driver's implementation, now with the effects body (`sim_tick::Bodies`) | unchanged |
| `EFFECT_PART_CREATE` | `0x17a8a0` | `implement` "effect part arm" | the only implementation; `host_audit`'s observers run outside it |
| `DAMAGE_AREA_APPLY` | `0xd6c80` | `observe_before` "effect part arm: compare" | the point's only hook |
| `CACHE_TAGS_LOAD` | | `observe_before` "effect part arm: reset" | beside the other arms' resets; observers need no order |

`sim.arms` reports the subsystem `effects-parts`: `effects.damage` (the arm's counters) and `effects.parts` (`answered` the parts returned from Rust, `declined` the refusals).

## Rules and limits

- **RNG order.** Unchanged. The damage arm and the returned arms draw nothing. The arms that draw (object: word 0 through `0x177d10` and `0x177f60`; decal and `coln`: word +4) still run in the guest.
- **Refusals** are named counters in `sim_effects::dispatch::Stats`: `guest_object`, `guest_sound`, `guest_decal`, `guest_light`, `guest_character`, `guest_coln`, `guest_other`, `declined_fault`. The walk's are in `sim_effects::walk::Stats` (`declined_fault`: the array header could not be read, so the driver calls the guest phase).
- **What an answered damage part leaves different.** The return address under the area damage's frame is the driver's, not `0x17aa4f`; it is dead stack below the caller's `ESP`, outside the arena. `0xd6660`'s hooks do not see effect parts once the arm answers (the one hook there, `h1-projectiles`, acts only on the projectile's call).
- **The walk** leaves the phase's own frame (saved registers, the tick length) unwritten below the driver's stack, and the callees' return addresses are the driver's.
- **The proof is per map.** A map load resets the arm and empties the scene queue.

## Evidence

- The switch's arms, the record layout and the frame offsets: disassembly of `0x17a8a0`, `0xd6660` and `0x176e40` (`h2 disasm`).
- `damage_parts_match_lifted` runs the lifted `0x17a8a0` on the interpreter over 64 seeded parts and compares the record it passes to `0xd6c80`, its place in the frame and the call's depth.

## Not verified

- Whole-run parity: `h2 sim run docs/runtime/sim-examples/lockout-2v2.toml` with `H2_FEATURES=-impl.sim.effects-parts` and without, then `h2 sim diff` of the two reports.
- That the arm reaches 4096 matched parts on a real match, and its timing.

## Related

[sim-effects.md](sim-effects.md) (the exact ports under the phase), [sim-rewrite.md](sim-rewrite.md), [engine-map.md](engine-map.md).
