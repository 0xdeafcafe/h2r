# The sound simulation

Status: Partly built. The schedule, the events, the host feed, the guest hooks, the tag duration reader and the marker start's parameter layout are written; the mixer's clip source is open, and `h2 sim diff` has not been run on it.
Code: `crates/rewrites/sim-sound/src/{events,schedule,feed}.rs`, `crates/rewrites/sim-bridge/src/sim_sound.rs`, `crates/rewrites/sim-bridge/src/sim_sound/{mixer_feed,tag_durations,owners}.rs`. Tests: the same files (`sound_phase_ledger_is_zero`, `script_wait_ends_on_computed_tick`, `null_sink_host_hash_equals_audible`, and the schedule's unit tests).
Switch: `impl.sim.sound-events` (off), listed in [switches.md](../runtime/switches.md).

A sound started in the sim is a record: id, source, tag, start tick and an end tick computed from the tag. There are no voices, channels, priorities or culling in the sim. The host mixer is fed from the sim's events and cannot change anything the sim reads. The rule that must hold: the sim's sound state, [`Schedule::hash`], is the same with any host or none.

## How it works

- **End tick.** `end = start + duration_ticks(duration_ms)` (`schedule::end_tick`): the engine's own rounding (`0x18a13b`, `0x18a1c0`), `fistp(f32(ms) * 0.001f32 * 30.0f32)`, nearest and ties to even, so 16 ms is 0 ticks and 17 ms is 1. A looping sound has no end tick and ends only by `stop`. A tag with no duration ends on the tick it starts. Code: `schedule.rs`.
- **Step.** `Schedule::step(tick)` returns the starts and stops since the last step in call order, then the sounds whose end tick has come, in start order. Ids count up from 1 and are not reused in a run.
- **Script waits.** A wait on a sound is `Schedule::finished(id, tick)`: true once the sound is gone or its end tick is not after `tick`. It reads the schedule and nothing from a voice, so it is the same on a host with no audio device. Test: `script_wait_ends_on_computed_tick`.
- **Save.** `Schedule::save` and `restore` are the canonical bytes of the active sounds. `SimSoundEvents` registers them as a machine-snapshot section. Test: `save_round_trips_active_sounds`.

## The host feed

`SoundSim::service` hands the events the ticks made to a `Host` ([`sim_sound::feed`]). `MixerHost` turns a start into a mixer buffer, a stop into its removal, and sweeps the buffers that have played out. `NullHost` hears nothing. Test: `null_sink_host_hash_equals_audible`. The feed reads the events and never the sim, so the hash is the same with any host or none.

## Where the sound runs

The 29 phases of the tick ([sim-rewrite.md](sim-rewrite.md), "The tick driver") hold no sound phase, so the tick driver's ledger (`sim.ledger`) does not see sound. Two retail paths carry it:

| Path | Where | Under the switch |
|---|---|---|
| Sound service `SOUND_UPDATE` (`0x125b40`) | the main loop `12b450`, once a frame, outside the tick | implemented in Rust: delivers events to the host, no guest call |
| Game sound `GAME_SOUND_UPDATE` (`0x18c060`) | inside the tick, under a guest phase | not replaced: its sources still run as guest code and start sounds through the two start entries below |
| Stop `SOUND_STOP_INSTANCE` (`0x126360`, instance in EBX) | called by a script stop, a retrigger of a playing line (`0x189f1f`) and the object code | a `before` hook: a sim handle ends its sound on the schedule and the guest's stop is skipped; a guest instance runs the guest's stop |
| Starts `SOUND_IMPULSE_ALLOCATE` (`0x126000`), `SOUND_START_AT_MARKER` (`0x1891d0`) | called from both paths and from the object code | implemented in Rust: the start goes to the schedule and the caller gets a sim handle (`0x7000_0000 | id`) in place of a guest instance |
| The schedule's step | after `GAME_TICK`, an observer | no guest call |
| Owner callbacks | before `GAME_TICK`, first in its chain | the finish on guest memory; the lip-sync update as a guest call per owned sound |

The sound simulation's own counter is `SoundLedger` (`sim_sound.rs`): `guest_calls` counts the guest calls its hooks make, and its entry points hold no guest-call capability, and the owner-callback hook is the one place that calls (the lip-sync updates), so it is 0 until an owned sound plays. `service_runs` counts the main-loop runs, which fall with the frame rate and not the tick count, so the step that decides ends runs in the tick and the service only delivers. Test: `sound_phase_ledger_is_zero`. The ledger cannot show the win: the cost removed is the service and its HLE (about a tenth of the loop on a phantom host), which a whole-loop ms/tick bench shows.

## Owner callbacks

A start's request block (`SOUND_IMPULSE_ALLOCATE`'s third argument) names an owner (`+0x50`, when bit `0x08` of its first byte is set) and a callback table (`+0x58`, bit `0x20`); a marker start's owner is its object and its table is `0x444afc`. The mixer calls the table for each instance: `+8` on every update with `(owner, payload, tag, pitch range, permutation record, audio seconds)` (`0x129593`), and `+0x10` once at the finish with `(owner, instance, 4)` (`0x127360`, reason from `0x1294b7`). `owners.rs` keeps the owned sounds and makes both from the schedule, in a `before` hook on `GAME_TICK` that runs first:

- **Finish.** The tick after a sound's end or stop event. The finishes `0x18c8c0` and `0x18c910` read the instance's tag from the guest pool, which a sim handle does not index, so `owners::finish` makes their effect on guest memory word for word: with the dialogue switch (`[0x4ed28c] + 0x29`) on and the owner a unit, a unit whose dialogue word (`unit + i16[unit + 0x33e] + 0x38`) holds the tag has it set to `-1`; `0x18c910` also frees the dialogue-table slot holding the handle. A table holding another function is counted (`SoundLedger::unsupported_owners`), not called.
- **Update.** Once a tick for each running owned sound whose `+8` is the lip-sync update `0x18c6a0` (it reads no instance): a guest call with the six arguments, the audio time being the ticks since the start over 30, then the tick itself is called again (the hook finds nothing due for that tick number and continues). `SoundLedger::guest_calls` counts these calls; it is 0 until an owned sound plays.
- **Permutation.** The update is told a permutation of the tag's first pitch range, `hash(tag, start) % count`. The engine's admission (`0x126000`) draws it, and the rest of the start, from two guest random streams in a fixed order: the sound stream `[0x4e7408] + 4` for the gain jitter (`0x125260`, before the admission checks that can reject the start), the stream at `RANDOM_SEEDS + 4` for the pitch jitter (`0x1252f0`), then the sound stream again for the permutation (`0x219070`, with a played-permutation mask in the gestalt). Starts can be rejected after the first draw (gain at the floor, `0x1260a8`; no free instance, `0x12609a`), so a replay of the draws without the engine's own admission cannot keep the streams identical.
- **Fade.** A stopped sound ends when the engine's ramp (`0x12a810`: `[inst + 0xb4]` and `[inst + 0xb8]` against the mixer clock `[0x4e6380] + 0x84`) reaches silence in the update loop (`0x1294a9`, finish reason 4); `0x126360` only starts the ramp. The finish here comes the tick after the stop.
- **Update time.** The engine calls `+8` from the mixer's update loop (`0x129593`) once per channel per main-loop run, with the channel's playback position (`[channel + 0x10]`, `[channel + 0x1c]`, from `0x129aa0`) and the mixer clock. The sim calls it at tick start with the ticks since the start.

Design for the three. Make the engine's own admission run and keep its instance: wrap `SOUND_IMPULSE_ALLOCATE` (a `before` hook that calls it again, as the owner-callback hook calls the tick) instead of replacing it, register the returned instance with the schedule, and drop the sim handle. The permutation, the draws and the rejects are then the engine's, the host reads the permutation from the instance, and the owner callbacks read a real instance, so `owners::finish` goes away: the sim calls the engine's finish `0x127320` on the end tick. A stop runs the engine's `0x126360`, and the sim ends the sound when the ramp does; that needs the mixer clock, which the sim holds no copy of. The `+8` call moves to the `SOUND_UPDATE` replacement, with the sim's clock for the channel's position. None of this can be checked against the engine without `h2 sim diff` on a map with dialogue.

Tests: `a_request_block_names_its_owner_table_and_permutation`, `the_update_is_the_mixers_six_arguments_with_the_elapsed_seconds`, `the_unit_finish_clears_the_dialogue_word_only_for_this_tag`, `the_dialogue_finish_also_frees_the_slot_holding_the_handle`, `a_finish_the_sim_does_not_know_is_counted_not_called` (`owners.rs`); `an_owned_sounds_finish_is_due_on_the_tick_after_its_end_event`, `owners_survive_a_save`, `the_tick_hook_is_first_and_only_under_the_switch`, `the_updates_are_called_in_order_with_their_arguments_then_the_tick` (`sim_sound.rs`).

## Interfaces

- Reads from guest memory: the tick (`GAME_TIME` + 8), the start entries' stack arguments (tag, source), and for a marker start the tag from the parameter block (`Durations::marker_tag`).
- `Durations` (`sim_sound.rs`) is what the sim reads of a tag: `guest_def(guest, tag) -> SoundDef` at a start. `GuestDurations` (`sim_sound/tag_durations.rs`) is the install's reader; `NoDurations` (every sound ends on its start tick) is for tests.

## Tag durations

The engine takes a sound's length from one word: the `snd!` metadata's `+0x10`, the maximum play time in milliseconds ([sound.md](../formats/sound.md)), read at `0x18a13b` (the dialogue tracker a start feeds) and `0x18a1c0`. It reads no permutation or pitch range; the word already holds the longest permutation's length. `tag_durations::read_def` finds the tag through the entry table (`0x10` bytes per entry, group `+0`, metadata address `+8`, indexed by the datum's low 16 bits) and reads that word. A `snd!` does not loop; a tag of the looping sound group `lsnd` is a loop and has no end tick. A tag of any other group, with no metadata or no datum has no duration. Tests: `durations_equal_the_engines_for_a_fixture_of_tags`, `ticks_round_to_nearest_as_the_engine_does`.

`SOUND_START_AT_MARKER`'s one stack argument points to a block of four words (`0x1890c0` reads them): `+0` a datum resolved through `0x18d5b0` (`u32::MAX` for a dialogue line), `+4` the sound tag, `+8` an `f32` scale (`1.0`), `+0xc` a word (`0`). The start's tag is `+4`. Test: `the_marker_block_names_its_tag_at_word_one`.
- `Clips` (`mixer_feed.rs`) is what the mixer plays for a tag. It is presentation: the permutation choice is the host's.
- With the switch on, the exact port `impl.sim.native.sound` stands down (`crates/apps/boot/src/registration.rs`).

## Rules and limits

- Sounds the guest still starts see a sim handle where they expect a guest instance. A stop by handle is routed to the schedule (`SOUND_STOP_INSTANCE`); the owner callbacks are made from the schedule ("Owner callbacks"). `BIPED_DIALOGUE_LIP_SYNC` (`0x109220`) runs as retail: it stores the handle in the unit's face state (`0x114710`, `+0x54`), and nothing reads an instance through it.
- **Script waits** (`sound_impulse_time`, `sound_impulse_language_time` at `0x2a9f70`, `0x18a2f0`) read the guest's own line table (`[0x4ed288] + 0x128`, 16 slots of tag, start tick, instance, chosen permutation), which `0x189ee0` stamps at the start of a script line from the tag and the chosen permutation's length (`0x20b140`, `0x20b3c0`): `(start - now + length_ticks) * tick_seconds`, floored at 0. The wait never reads a voice, so it is the same with the switch on or off. The sim does not touch the table; the schedule's end tick (`snd!` `+0x10`, the longest permutation) can pass the table's end for a line whose permutation is shorter, and nothing in the guest reads it.
- Which sounds are culled under load, and when a sound ends relative to the audio device, no longer match retail. These are what `h2 sim diff` can flag.
- The step runs once per tick number; a tick run twice steps once.

## Not verified

- That the lip-sync update's arguments move a unit's mouth as the engine's do on a running map (the `+8` call is made from the tick's start, not the main loop's update), and that `h2 sim diff` matches with owned sounds.
- The `GuestCall` to `GAME_TICK` re-enters the chain (the hook relies on it finding nothing due).
- The marker block's `+0` and `+0xc` words: named by use only. The retail tag-table walk (`0x221810` and the datum-index check) is not salt-checked here.
- That the tag durations match the engine's end on a running map (`h2 sim diff`); the readers are tested on fixtures.
- That `h2 sim diff` matches over a run with the switch on.
- Which guest callers of the two start entries use the returned instance, and what they do with a handle that is not in the guest pool.
- The mixer wiring at boot (`MixerHost` needs a `Clips` over the decoded sound data).

## Related

[sim-rewrite.md](sim-rewrite.md) (the exact port and the tick driver), [dsound.md](../audio/dsound.md) (the retail sound frame), [switches.md](../runtime/switches.md).
