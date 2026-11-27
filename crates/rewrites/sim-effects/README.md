# sim-effects

The effect part split of the simulation rewrite. Each part an effect event creates is classified by its tag group. Damage parts are sim state: the order they are applied in is folded into the sim hash. Visual-only parts (decals, lights, lens flares) are events for the renderer and never reach the hash. Nothing in the crate reads guest memory; `sim-bridge` decodes the part and calls in.

[Effect parts](../../../docs/engine/effects-parts.md) · [Simulation rewrite](../../../docs/engine/sim-rewrite.md)
