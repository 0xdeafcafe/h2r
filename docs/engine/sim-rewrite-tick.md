# The tick driver

Status: Built. There is no switch.
Code: `crates/rewrites/engine/src/sim_tick.rs` (`engine::sim_tick::run`), `crates/rewrites/sim-bridge/src/tick.rs` (`TickDriver`, `LEDGER`). Tests: `crates/rewrites/engine/tests/sim_tick_differential.rs`, `crates/rewrites/sim-bridge/src/tick.rs`.

The game tick runs from Rust, in the original's order. The flow is the diagram under "The tick driver" in [sim-rewrite.md](sim-rewrite.md).

`GAME_TICK` (`0x00137fe0`) runs from Rust: `engine::sim_tick::run`, installed
by `sim_bridge::tick::TickDriver` as the body of the point with
`implement_async`. There is no switch. It does what the original does, in the
original's order:

0. the events row: `damage::tick_starts` empties the last tick's event buffer
   (what a hook on `GAME_TICK`'s entry did);
1. the x87 control word through the CRT (`0x003212d6`, `_controlfp` with new
   word `0x9001f` and mask `0xfffff`);
2. the `0x4048`-byte update record on the guest stack below the entry `ESP`,
   built by `0x000682e0` and applied by `0x000683a0`;
3. the run flag, the byte at record `+4` (`RUN_FLAG`). When it is clear the
   phases and the clock step are skipped;
4. the 29 phases of [the sim review](../reviews/2026-11-20-sim.md) section 2.1
   (`engine::sim_tick::SYSTEMS`, one ordered table, every row named). Phase 1 (`0x0014d3a0`) takes the record as
   its one stack argument (`ret 4`) with `ECX` = record; phases 2-29 take no
   argument and see `EBX` = record (`Form`; `0x00138db0` reads it);
5. the clock step (`0x001380b4`), inside the run branch: `[*TICK_CLOCK + 0x14]`
   falls by one to zero and `[*TICK_CLOCK + 4]` gains the f32 tick length
   `[*GAME_TIME + 4]`;
6. the record's close `0x000684e0` (`ESI` = record), which runs whether or
   not the flag was set;
7. the tick count at game time `+8`, which advances only if the flag is still
   set after the close.

**Names.** The 14 rows that had only an address are named from the lifted
code (`input_random` `0x138db0` steps the shared seed per action axis;
`periodic_sound_walk` `0x20b660` runs the `snd!` group every N ticks;
`game_over_check` `0x138e40`; `game_save_update` `0x2268e0` and
`game_save_policy` `0x226050` (the save module); `screen_shake_players`
`0x153950`; `net_flag_decay` `0x225730`; the rest carry the global they
walk: `pool_walk_4f5724`, `state_4f55ec_update`, `time_clock_13c680`,
`object_list_update`, `script_globals_step`, `input_record_apply`,
`stats_update`). `the_rows_are_retail_order` pins the names in order.

**The rate.** `TICK_RATE_HZ` is 30 and `tick_length(rate)` is the one place
a rate becomes a step; no row holds a literal 30.

Each phase has an owner. A guest phase is a guest call (`GuestCall`, awaited,
with the record's bytes reserved below the call's stack); a Rust phase is a
body in the driver. Five phases are Rust; the other 24 are guest calls,
and `tick-ledger.toml` counts them:

| Phase | Rust body | What it still calls in the guest |
|---|---|---|
| 3 `ai` `0x1c83c0` | `engine/src/ai_update.rs` | the six AI systems and the teardown |
| 8 `script_scheduler` `0x209310` | `engine/src/hs_update.rs` | `HS_THREAD_EXECUTE` `0x209850` per thread; its tail call, the object-list sweep `0x1deed0`, is `engine/src/hs_object_lists.rs` |
| 15 `effects` `0x176e40` | `engine/src/effects_phase.rs` | the walk's nine callees |
| 17 `post_game` `0x15ca10` | `engine/src/post_game.rs` | eleven functions; the round state machine `0x15c550` is `engine/src/round_state.rs`, whose state change still calls its leaves and the engine's state virtual |
| 29 `countdown_requests` `0x16f200` | `engine/src/countdown_requests.rs` | nothing |

`ai_update_matches_lifted`, `hs_update_matches_lifted` and
`post_game_matches_lifted` (`engine/tests/sim_tick_differential.rs`) hold
the bodies to the lifted functions with the guest callees stubbed. The in-place
call that avoids the unwind per phase is a later speed-up, not a
prerequisite.

**Presentation phases.** Phases 26 (first person `0x165dc1`), 28 (HUD
`0x1917b2`) and 29 (the countdown requests `0x16f200`, in Rust: `engine/src/countdown_requests.rs`) are the seats'
views (`impl.sim.present`, on; `Present::LocalOnly`):

| Local users mapped | The three phases | Their random draws |
|---|---|---|
| none (phantoms, hosts) | not called; 0 calls and 0 ns in the ledger | none |
| one or more | called in retail order | from `PRESENT_SEEDS`, the presentation's own copy of both shared seeds `[0x4e7408]` `+0` and `+4`, swapped in before each phase and the sim's put back after |

So the sim's draws, and the hash, are the same with 0 and 4 views
(`hash_equal_with_0_and_4_views`, `sim-bridge/src/present.rs`). The stream
starts as the sim's seeds, lives for the process and is not saved. A phase
whose seed words do not read runs on the sim's seeds, counted by
`sim_tick::unisolated`.

The remaining hooks on `GAME_TICK` run as before (the events row above is the
one that moved into the table): the driver takes the original's place
in the chain, so each `before`, `after` and
`observe_after` hook (lockstep's hash, telemetry, the agent's queued calls,
actor-net, cheats, skulls, co-op, firefight) fires at the same point around
it, and a `before` that returns skips the body. `existing_tick_hooks_still_fire`
(`sim-bridge/src/tick.rs`) pins the order in the plan text.

**The ledger.** `sim_bridge::tick::LEDGER` counts each guest call per phase
(`engine::sim_tick::Ledger`) and times it: nanoseconds per phase as a
running total and for the last tick, the tick body's, and the loop's (tick
entry to the next entry, which holds the sound that runs outside the 29
phases).

The clock is `Instant`, read once after each phase and three times per tick;
the phases' times are consecutive spans, so they sum to at most the tick's.
`sim.ledger` ([the agent API](../runtime/agent-api.md)) reads
it, `profile.counters` carries it as `sim_tick_phase_<n>_ns`, and the sim
report's `tick_phases` block holds the per-match means. Wave benches read
the phases there. The ratchet is
`crates/rewrites/sim-bridge/tick-ledger.toml`: the number of guest phases per
map kind, which only falls. `ledger_ratchet_does_not_grow` fails when the
phase table owns more guest phases than the file allows, and a slice that
moves a phase to Rust lowers the number in the same commit. The goal is 0:
only our own simulation.

**Proof.** `tests/sim_tick_differential.rs` (engine, `differential`) runs the
lifted body on the IR interpreter: `phases_run_in_retail_order` compares its
calls with `SYSTEMS` (the odd entries `0x00165dc1` and `0x001917b2` are the
targets the body reaches), and `tick_matches_lifted` compares the
envelope's registers and stack arguments (the control word's mask included),
the phases' `ECX`, `EBX` and stack argument, and the game-time page, which
holds the clock step's writes. Both runs set the run flag at the before step.
`sim-bridge/src/tick.rs` covers a clear flag (no phases, the close still
runs, the tick holds) and a close that clears it.
