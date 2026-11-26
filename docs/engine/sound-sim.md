# The sound simulation

Status: Partly built. The schedule, the events, the host feed and the guest hooks are written; the tag duration reader, the marker start's parameter layout and the mixer's clip source are open, and `h2 sim diff` has not been run on it.
Code: `crates/rewrites/sim-sound/src/{events,schedule,feed}.rs`, `crates/rewrites/sim-bridge/src/sim_sound.rs`, `crates/rewrites/sim-bridge/src/sim_sound/mixer_feed.rs`. Tests: the same files (`sound_phase_ledger_is_zero`, `script_wait_ends_on_computed_tick`, `null_sink_host_hash_equals_audible`, and the schedule's unit tests).
Switch: `impl.sim.sound-events` (off), listed in [switches.md](../runtime/switches.md).

A sound started in the sim is a record: id, source, tag, start tick and an end tick computed from the tag. There are no voices, channels, priorities or culling in the sim. The host mixer is fed from the sim's events and cannot change anything the sim reads. The rule that must hold: the sim's sound state, [`Schedule::hash`], is the same with any host or none.

## How it works

- **End tick.** `end = start + ceil(duration_ms * 30 / 1000)` (`schedule::end_tick`). A looping sound has no end tick and ends only by `stop`. A tag with no duration ends on the tick it starts. Code: `schedule.rs`.
- **Step.** `Schedule::step(tick)` returns the starts and stops since the last step in call order, then the sounds whose end tick has come, in start order. Ids count up from 1 and are not reused in a run.
- **Script waits.** A wait on a sound is `Schedule::finished(id, tick)`: true once the sound is gone or its end tick is not after `tick`. It reads the schedule and nothing from a voice, so it is the same on a host with no audio device. Test: `script_wait_ends_on_computed_tick`.
- **Save.** `Schedule::save` and `restore` are the canonical bytes of the active sounds. `SimSoundEvents` registers them as a machine-snapshot section. Test: `save_round_trips_active_sounds`.
- **Host feed.** `SoundSim::service` hands the events the ticks made to a `Host` ([`sim_sound::feed`]). `MixerHost` turns a start into a mixer buffer, a stop into its removal, and sweeps the buffers that have played out. `NullHost` hears nothing. Test: `null_sink_host_hash_equals_audible`.

## Where the sound runs

The 29 phases of the tick ([sim-rewrite.md](sim-rewrite.md), "The tick driver") hold no sound phase, so the tick driver's ledger (`sim.ledger`) does not see sound. Two retail paths carry it:

| Path | Where | Under the switch |
|---|---|---|
| Sound service `SOUND_UPDATE` (`0x125b40`) | the main loop `12b450`, once a frame, outside the tick | implemented in Rust: delivers events to the host, no guest call |
| Game sound `GAME_SOUND_UPDATE` (`0x18c060`) | inside the tick, under a guest phase | not replaced: its sources still run as guest code and start sounds through the two start entries below |
| Starts `SOUND_IMPULSE_ALLOCATE` (`0x126000`), `SOUND_START_AT_MARKER` (`0x1891d0`) | called from both paths and from the object code | implemented in Rust: the start goes to the schedule and the caller gets a sim handle (`0x7000_0000 | id`) in place of a guest instance |
| The schedule's step | after `GAME_TICK`, an observer | no guest call |

The sound simulation's own counter is `SoundLedger` (`sim_sound.rs`): `guest_calls` counts the guest calls its hooks make, and none of its entry points holds a guest-call capability, so it is 0. `service_runs` counts the main-loop runs, which fall with the frame rate and not the tick count, so the step that decides ends runs in the tick and the service only delivers. Test: `sound_phase_ledger_is_zero`. The ledger cannot show the win: the cost removed is the service and its HLE (about a tenth of the loop on a phantom host), which a whole-loop ms/tick bench shows.

## Interfaces

- Reads from guest memory: the tick (`GAME_TIME` + 8), the start entries' stack arguments (tag, source), and for a marker start the tag from the parameter block (`Durations::marker_tag`).
- `Durations` (`sim_sound.rs`) is what the sim reads of a tag: `def(tag) -> SoundDef`. `NoDurations`, the headless default, ends every sound on its start tick.
- `Clips` (`mixer_feed.rs`) is what the mixer plays for a tag. It is presentation: the permutation choice is the host's.
- With the switch on, the exact port `impl.sim.native.sound` stands down (`crates/apps/boot/src/registration.rs`).

## Rules and limits

- Sounds the guest still starts and stops itself (a stop by instance handle, lip sync's instance argument, dialogue's end callbacks) see a sim handle where they expect a guest instance. Their effect is not yet routed to the schedule. Design.
- Which sounds are culled under load, and when a sound ends relative to the audio device, no longer match retail. A script wait ends on the computed tick; retail ends it on the voice. This and the unsimulated stops are what `h2 sim diff` flags.
- The step runs once per tick number; a tick run twice steps once.

## Not verified

- The parameter block of `SOUND_START_AT_MARKER` and the sound tag's duration field: no reader exists, so `NoDurations` is the only `Durations`.
- That `h2 sim diff` matches over a run with the switch on.
- Which guest callers of the two start entries use the returned instance, and what they do with a handle that is not in the guest pool.
- The mixer wiring at boot (`MixerHost` needs a `Clips` over the decoded sound data).

## Related

[sim-rewrite.md](sim-rewrite.md) (the exact port and the tick driver), [dsound.md](../audio/dsound.md) (the retail sound frame), [switches.md](../runtime/switches.md).
