# sim-sound

The sound simulation's Rust-owned state and kernels. It reads no guest memory.

The sound table (`instances`, `chooser`, `owners`, `looping`, `starts`) is the `World`'s sound store: retail's draws in retail's order, ends on computed ticks, owners called in retail's order. `schedule`, `events` and `feed` are the sound-events arm and the host feed. The guest readers that hand the table its inputs while tags, units and class state live in guest memory are in `sim-bridge/src/sim_sound/`.

[Sound instances](../../../docs/engine/sound-instances.md) · [Sound simulation](../../../docs/engine/sound-sim.md) · [Simulation world](../../../docs/engine/sim-world.md)
