# The sound simulation

Status: Partly built. The schedule, the events, the host feed and the instance-based guest hooks are written; the mixer's clip source is open, and `h2 sim diff` is identical with the switch off and on ([note](notes/2026-11-27-sound-events-parity.md)).
Code: `crates/rewrites/sim-sound/src/{events,schedule,feed}.rs`, `crates/rewrites/sim-bridge/src/sim_sound.rs`, `crates/rewrites/sim-bridge/src/sim_sound/owners.rs`. Tests: the same files (`sound_phase_ledger_is_zero`, `script_wait_ends_on_computed_tick`, `null_sink_host_hash_equals_audible`, and the schedule's unit tests).
Switch: `impl.sim.sound-events`, off in play (boot reads it with a default of off), listed in [switches.md](../runtime/switches.md). With it on the game is silent: see [sound-instances.md](sound-instances.md), "DirectSound cut", stage D0.

A sound started in the sim is a record: id, source, tag, start tick and an end tick computed from the engine's own instance for it. There are no voices, channels, priorities or culling in the sim. The host mixer is fed from the sim's events and cannot change anything the sim reads. The rule that must hold: the sim's sound state, [`Schedule::hash`], is the same with any host or none.

## How it works

- **End tick.** `end = start + duration_ticks(duration_ms)` (`schedule::end_tick`): the engine's own rounding (`0x18a13b`, `0x18a1c0`), `fistp(f32(ms) * 0.001f32 * 30.0f32)`, nearest and ties to even, so 16 ms is 0 ticks and 17 ms is 1. A looping sound has no end tick and ends only by `stop`. A tag with no duration ends on the tick it starts. Code: `schedule.rs`.
- **Step.** `Schedule::step(tick)` returns the starts and stops since the last step in call order, then the sounds whose end tick has come, in start order. Ids count up from 1 and are not reused in a run.
- **Script waits.** A wait on a sound is `Schedule::finished(id, tick)`: true once the sound is gone or its end tick is not after `tick`. It reads the schedule and nothing from a voice, so it is the same on a host with no audio device. Test: `script_wait_ends_on_computed_tick`.
- **Save.** `Schedule::save` and `restore` are the canonical bytes of the active sounds. `SimSoundEvents` registers them as a machine-snapshot section. Test: `save_round_trips_active_sounds`.

## The host feed

`SoundSim::service` hands the events the ticks made to a `Host` ([`sim_sound::feed`]); a host reads them and never the sim, so the hash is the same with any host or none (`null_sink_host_hash_equals_audible`). Boot gives this arm `NullHost`, so it plays no sound; the audible feed is the table's ([sound-instances.md](sound-instances.md), "Mixer feed").

## Where the sound runs

The 29 phases of the tick ([sim-rewrite.md](sim-rewrite.md), "The tick driver") hold no sound phase, so the tick driver's ledger (`sim.ledger`) does not see sound. The sim keeps the engine's own instances: the guest pool holds every sound, and the sim holds the schedule that says when each ends.

| Path | Where | Under the switch |
|---|---|---|
| Admission `SOUND_IMPULSE_ALLOCATE` (`0x126000`) | called from both paths and from the object code | wrapped (`sim_sound::admit`): a `before` hook calls the engine's admission as a guest call, so its random draws (`0x125260`, `0x1252f0`, `0x219070`) and rejections are the engine's, then enters the returned instance in the schedule with the end tick from its permutation length (`owners::read_admitted`, u16 at the permutation record `+6`, via `0x218c60`) |
| Sound service `SOUND_UPDATE` (`0x125b40`) | the main loop `12b450`, once a frame, outside the tick | implemented (`sim_sound::service`): events to the host, then the lip-sync update `+8` (`0x18c6a0`) for each running owned instance, with the six arguments of `0x129593` and the seconds since its start |
| Tick start `GAME_TICK`, before, first in its chain | inside the main loop | `sim_sound::deliver`: sets the mixer clock `[[0x4e6380] + 0x84]` from the tick, polls each ramping instance, runs the finishes, then calls the tick again (the hook finds nothing due for that tick number) |
| The schedule's step | after `GAME_TICK`, an observer | no guest call |
| Game sound `GAME_SOUND_UPDATE` (`0x18c060`), the marker start, the stop | inside the tick and the object code | not touched: they run as guest code against the engine's real instances |

The sound simulation's own counter is `SoundLedger` (`sim_sound.rs`): `guest_calls` counts the guest calls the finish and ramp make, so it is 0 until an instance is entered; the update is the native unit mouth (`sim_bridge::unit_mouth`) and makes none. `service_runs` counts the main-loop runs, which fall with the frame rate and not the tick count, so the step that decides ends runs in the tick and the service only delivers. Test: `sound_phase_ledger_is_zero`.

Refusals are counters of the same ledger, never a quiet fallback: `refused_unreadable` counts admissions whose returned handle or tick could not be read (the sound is not entered; the guest keeps it), and `refused_no_duration` counts sounds whose permutation length could not be read from the tag (entered, ending on their start tick). Both are 0 on a run that reads every tag. No new hook is added; the order of the existing ones is the hooks' list above.

## Instances

An entered sound is the engine's record (`SOUND_INSTANCES`, `0xBC` bytes, salted datum handle; `owners::record`). The sim reads its tag (`+0xC`), owner (`+0x10`), callback table (`+0x14`), payload (`+0x5C`), pitch range (`+0x9C`), permutation (`+0x9D`) and, for a stop, the fade fields (`+0x20`, `+0xB4`, `+0xB8`).

- **End.** On the end tick the sim calls the engine's finish `0x127320` with reason `owners::ENDED_REASON` (2, the engine's reason for a one-shot whose channel has stopped: the service `0x128040` at `0x12810f`), if the guest has not freed the record already (`SoundLedger::stale`). The engine's finish runs the owner's `+0x10` callback (`0x18c8c0`, `0x18c910`), so the dialogue word and slot are cleared by the engine's code.
- **Stop ramp.** `0x126360` starts the ramp; the sim polls each instance with the ramp flag (`+4` bit 7) through `0x12a810` and ends it when `[+0x20] + ramp` is at or under the floor (`0x44a0b4`) and `[+0xB4] > [+0xB8]` (signed), with reason 4 (`owners::ramp_done`). The mixer clock the ramp reads is milliseconds in retail and wall-clock driven; the sim writes `tick * 1000 / 30` each tick (`sim_sound::clock_ms`).
- **Update.** `+8` is called from the service, once per running owned instance per main-loop run, the way `0x129593` calls it; its audio time is the ticks since the start over 30 (`owners::seconds`). The engine's value is the channel's `+0x10`, which `0x12a450` accumulates per service run (`0x12a5c6`) as the format's playback ratio (`0x21f650`) times the run's wall-clock step (`[mixer + 0x1e0]`, from the timer at `0x126a34`). That step is wall time and differs between runs of the engine itself, so no sim value can equal it bit for bit; the sim's is the sim time elapsed, the engine's at ratio 1. The only reader is the lip-sync update (`0x18c720`).

Tests: `the_admission_is_the_engines_and_its_instance_is_entered`, `a_tick_start_sets_the_clock_polls_ramps_and_finishes_then_runs_the_tick`, `an_instance_the_guest_freed_is_not_finished_twice`, `the_service_makes_the_owners_update_with_the_seconds_since_the_start`, `an_ended_sounds_finish_is_due_on_the_tick_after_its_end_event`, `running_sounds_survive_a_save` (`sim_sound.rs`); the instance readers in `owners.rs`.

## Interfaces

- Reads from guest memory: the tick (`GAME_TIME` + 8), the admission's stack arguments and returned instance, and the instance records above.
- Writes guest memory: the mixer clock.
- `Durations` (`sim_sound.rs`) is what the sim reads of a tag when a sound is started without an instance (tests); `NoDurations` ends every such sound on its start tick.
- With the switch on, the exact port `impl.sim.native.sound` stands down (`crates/apps/boot/src/registration.rs`).

## Rules and limits

- **Script waits** (`sound_impulse_time`, `sound_impulse_language_time` at `0x2a9f70`, `0x18a2f0`) read the guest's own line table (`[0x4ed288] + 0x128`, 16 slots of tag, start tick, instance, chosen permutation), which `0x189ee0` stamps at the start of a script line from the tag and the chosen permutation's length (`0x20b140`, `0x20b3c0`). The wait never reads a voice, so it is the same with the switch on or off.
- Which sounds are culled under load, and when a sound ends relative to the audio device, no longer match retail. These are what `h2 sim diff` can flag.
- The step runs once per tick number; a tick run twice steps once.

## Not verified

- That every natural end is the engine's reason 2: a stopped channel with flag bits 9 and 6 unset, or a looping source, leaves the instance to another path (reason 3 when the owner's `+0` callback returns false, 0xF).
- The playback ratio of the sound formats (`0x21f650`): the sim assumes 1.
- That a guest call to `SOUND_IMPULSE_ALLOCATE` and `GAME_TICK` from a hook re-enters the chain (the admission guards with a busy flag, the tick with the per-tick delivery).
- That `h2 sim diff` matches over a run with the switch on, on a map with dialogue.

## Related

[sound-instances.md](sound-instances.md) (the design that replaces the guest pool with a Rust table), [sim-rewrite.md](sim-rewrite.md) (the exact port and the tick driver), [dsound.md](../audio/dsound.md) (the retail sound frame), [switches.md](../runtime/switches.md).
