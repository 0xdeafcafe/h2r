# Effect parts: damage as sim state, visual events for the renderer

Status: Partly built. The split, the damage chain, the visual queue and the guest hook are written. The renderer does not yet read the queue, and `h2 sim diff` has not been run on it.
Code: `crates/rewrites/sim-effects/src/lib.rs`, `crates/rewrites/sim-bridge/src/sim_effects_parts.rs`. Tests: `damage_parts_match_lifted`, `visual_events_leave_hash_unchanged`, `effects_phase_ledger_is_zero` in `sim-effects`.
Switch: `impl.sim.effects-parts` (off), listed in [switches.md](../runtime/switches.md).

Each part an effect event creates goes through the part creation switch `EFFECT_PART_CREATE` (`0x17a8a0`, [engine-map.md](engine-map.md)). The split sorts parts by their tag group into those that change sim state and those only the renderer reads. It is separate from the exact ports of [sim-effects.md](sim-effects.md) and works with or without them.

## How it works

| Group | Class | Under the switch |
|---|---|---|
| `jpt!` | damage | runs retail's creation; the part extends the damage chain |
| `obje`, `snd!`, `char`, `coln`, others | sim state | runs retail's creation, untouched |
| `deca`, `ligh` | visual | queues a `VisualEvent`; runs retail's creation, because the decal and light arms draw from the effects stream (`[0x4e7408] + 4`) and write the arrays the renderer reads today |
| `lens`, `MGS2` | visual | queues a `VisualEvent`; returns without the call, since retail's arm is the epilogue (`0x17adc3`) |

- **Damage chain.** `EffectsSim::hash` folds each damage part's tick, effect, tag, location and flags in dispatch order. It is saved as a machine-snapshot section. Visual events are not in it and the queue is not saved. Test: `visual_events_leave_hash_unchanged`.
- **Visual queue.** `EffectsSim::drain_visual` hands the renderer the events since its last call, in order. The queue holds 4096; older events are dropped and counted in `EffectsLedger::dropped`. `guest_made` says whether the guest also created the part.
- **No guest call.** The hook is a `before` hook that reads the stack arguments and the part record and either continues or returns. `EffectsLedger::guest_calls` stays 0. Test: `effects_phase_ledger_is_zero`.

## Where the time goes

The calls skipped are the `lens` and `MGS2` parts, a small share of the disc's parts. The phase that falls is `effects` (`0x176e40`) in `sim.ledger`, by the guest calls and instructions those parts took. Decals and lights keep their guest creation until the renderer reads the queue and a decal or light arm is proven by `h2 sim diff` to draw nothing the later draws depend on.

## Rules and limits

- The reference for `damage_parts_match_lifted` is the dispatch table read from `0x17a8a0`; the live comparison is `h2 sim diff` of a run with the switch off and on.
- The decal arm (`0x17aa5c`) and the object arm draw from the effects stream. Removing a draw reorders every later one, so only an arm with no draw leaves the guest.
