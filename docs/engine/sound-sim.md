# The sound simulation

Status: Partly built. The schedule, the events, the host feed, the guest hooks, the tag duration reader and the marker start's parameter layout are written; the mixer's clip source is open, and `h2 sim diff` has not been run on it.
Code: `crates/rewrites/sim-sound/src/{events,schedule,feed}.rs`, `crates/rewrites/sim-bridge/src/sim_sound.rs`, `crates/rewrites/sim-bridge/src/sim_sound/{mixer_feed,tag_durations}.rs`. Tests: the same files (`sound_phase_ledger_is_zero`, `script_wait_ends_on_computed_tick`, `null_sink_host_hash_equals_audible`, and the schedule's unit tests).
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

The sound simulation's own counter is `SoundLedger` (`sim_sound.rs`): `guest_calls` counts the guest calls its hooks make, and none of its entry points holds a guest-call capability, so it is 0. `service_runs` counts the main-loop runs, which fall with the frame rate and not the tick count, so the step that decides ends runs in the tick and the service only delivers. Test: `sound_phase_ledger_is_zero`. The ledger cannot show the win: the cost removed is the service and its HLE (about a tenth of the loop on a phantom host), which a whole-loop ms/tick bench shows.

## Interfaces

- Reads from guest memory: the tick (`GAME_TIME` + 8), the start entries' stack arguments (tag, source), and for a marker start the tag from the parameter block (`Durations::marker_tag`).
- `Durations` (`sim_sound.rs`) is what the sim reads of a tag: `guest_def(guest, tag) -> SoundDef` at a start. `GuestDurations` (`sim_sound/tag_durations.rs`) is the install's reader; `NoDurations` (every sound ends on its start tick) is for tests.

## Tag durations

The engine takes a sound's length from one word: the `snd!` metadata's `+0x10`, the maximum play time in milliseconds ([sound.md](../formats/sound.md)), read at `0x18a13b` (the dialogue tracker a start feeds) and `0x18a1c0`. It reads no permutation or pitch range; the word already holds the longest permutation's length. `tag_durations::read_def` finds the tag through the entry table (`0x10` bytes per entry, group `+0`, metadata address `+8`, indexed by the datum's low 16 bits) and reads that word. A `snd!` does not loop; a tag of the looping sound group `lsnd` is a loop and has no end tick. A tag of any other group, with no metadata or no datum has no duration. Tests: `durations_equal_the_engines_for_a_fixture_of_tags`, `ticks_round_to_nearest_as_the_engine_does`.

`SOUND_START_AT_MARKER`'s one stack argument points to a block of four words (`0x1890c0` reads them): `+0` a datum resolved through `0x18d5b0` (`u32::MAX` for a dialogue line), `+4` the sound tag, `+8` an `f32` scale (`1.0`), `+0xc` a word (`0`). The start's tag is `+4`. Test: `the_marker_block_names_its_tag_at_word_one`.
- `Clips` (`mixer_feed.rs`) is what the mixer plays for a tag. It is presentation: the permutation choice is the host's.
- With the switch on, the exact port `impl.sim.native.sound` stands down (`crates/apps/boot/src/registration.rs`).

## Rules and limits

- Sounds the guest still starts see a sim handle where they expect a guest instance. A stop by handle is routed to the schedule (`SOUND_STOP_INSTANCE`). Lip sync's instance argument (`0x109220`) and the dialogue end callbacks still get a handle the guest pool does not hold. Design.
- **Script waits** (`sound_impulse_time`, `sound_impulse_language_time` at `0x2a9f70`, `0x18a2f0`) read the guest's own line table (`[0x4ed288] + 0x128`, 16 slots of tag, start tick, instance, chosen permutation), which `0x189ee0` stamps at the start of a script line from the tag and the chosen permutation's length (`0x20b140`, `0x20b3c0`): `(start - now + length_ticks) * tick_seconds`, floored at 0. The wait never reads a voice, so it is the same with the switch on or off. The sim does not touch the table; the schedule's end tick (`snd!` `+0x10`, the longest permutation) can pass the table's end for a line whose permutation is shorter, and nothing in the guest reads it.
- Which sounds are culled under load, and when a sound ends relative to the audio device, no longer match retail. These are what `h2 sim diff` can flag.
- The step runs once per tick number; a tick run twice steps once.

## Not verified

- The marker block's `+0` and `+0xc` words: named by use only. The retail tag-table walk (`0x221810` and the datum-index check) is not salt-checked here.
- That the tag durations match the engine's end on a running map (`h2 sim diff`); the readers are tested on fixtures.
- That `h2 sim diff` matches over a run with the switch on.
- Which guest callers of the two start entries use the returned instance, and what they do with a handle that is not in the guest pool.
- The mixer wiring at boot (`MixerHost` needs a `Clips` over the decoded sound data).

## Related

[sim-rewrite.md](sim-rewrite.md) (the exact port and the tick driver), [dsound.md](../audio/dsound.md) (the retail sound frame), [switches.md](../runtime/switches.md).
