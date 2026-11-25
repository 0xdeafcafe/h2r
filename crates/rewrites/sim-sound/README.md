# sim-sound

Sound simulation kernels for the source and channel walks, listener and environment updates, loop tracks, impulse scheduling and mixer submissions. Updates consume Rust-owned inputs. Guest addresses and DirectSound callbacks live in the engine adapters.

The subsystem table is in `engine/src/sim_sound.rs`. The guest records carry persistent state for checkpoints. The lifted differential tests compare the call order, live frame and result of each body over synthetic records. Full sim reports with arena hashing also record voice events in dispatch order.

[Simulation rewrite](../../../docs/engine/sim-rewrite.md#sketches) · [DirectSound](../../../docs/audio/dsound.md)
