# sim-effects

The effects phase of the simulation rewrite, as pure Rust over plain data. `walk` is the effect array walk of `0x176e40`; `parts` names each part's class by its tag group; `damage` builds a damage part's record exactly as `0x17a8a0` does; `arm` is the damage arm's proof (compared with the guest until it has matched 4096 parts, then answering with one audit in 64); `scene` queues the visual parts for the renderer's scene. Nothing here reads guest memory: `engine::sim_effects::phase` and `sim-bridge` decode and call in.

[Effects phase](../../../docs/engine/effects-parts.md) · [Simulation rewrite](../../../docs/engine/sim-rewrite.md)
