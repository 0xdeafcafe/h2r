# The sound simulation

Status: Built. The sound simulation is the Rust sound table on every run ([sound-instances.md](sound-instances.md)); the game's own sound pool, admission and mixer loop never run.
Code: `crates/rewrites/sim-sound/src/`, `crates/rewrites/sim-bridge/src/sim_sound/`, `crates/rewrites/sim-bridge/src/world.rs`.

A sound is a record in `sim_sound::instances::Sounds`: its tag, owner, source, start tick and the end tick computed from the tag. There are no voices, channels, priorities or culling in the sim. The host mixer is fed from the table's events and cannot change anything the sim reads, so the sim's sound state is the same with any host or none.

| Piece | Where |
|---|---|
| Every outside sound entry, answered in Rust | `sim_sound/entries.rs` (`ENTRIES`, `SoundTable`) |
| The sound phase, after the tick's 29 | `tick::run`, `entries::phase` |
| The save | the `World`'s sound section (`world::SimWorld`, [sim-world.md](sim-world.md)) |
| The mixer feed | `sim_sound/mixer_feed.rs`, `game/src/sound_feed.rs` |

## End ticks

`end = start + duration_ticks(duration_ms)` (`sim_sound::schedule::duration_ticks`): the engine's own rounding (`0x18a13b`, `0x18a1c0`), `fistp(f32(ms) * 0.001f32 * 30.0f32)`, nearest and ties to even, so 16 ms is 0 ticks and 17 ms is 1. A looping sound has no end tick and ends only by a stop.

## Script waits

`sound_impulse_time` and `sound_impulse_language_time` (`0x2a9f70`, `0x18a2f0`) read the guest's own line table (`[0x4ed288] + 0x128`, 16 slots of tag, start tick, instance, chosen permutation), which `0x189ee0` stamps at the start of a script line from the tag and the chosen permutation's length (`0x20b140`, `0x20b3c0`). A wait never reads a voice, so it is the same on a host with no audio device.

## Related

[sound-instances.md](sound-instances.md) (the table), [sim-world.md](sim-world.md) (the save), [dsound.md](../audio/dsound.md) (the retail sound frame).
