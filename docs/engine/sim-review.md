# Simulation state and the experimental compiler tier

Status: Partly built. The typed state views and the leaf compiler exist.
Code: `crates/rewrites/sim-state`, `crates/recompiler/emit/src/sim.rs`.
Switch: none.

## Simulation state

`crates/hooking/guest-layout` names the retail layouts and globals.
`crates/rewrites/sim-state` uses them to expose typed simulation state
across the guest boundary. The [simulation rewrite design](sim-rewrite.md)
describes where Rust subsystems take ownership of that state.

## 4.2 Sim-tier contract

`emit::sim::Tier` compiles a closure of direct guest calls for
experiments. It infers the guest register and flag contract for each
function, lowers direct guest calls to C calls and uses a flat memory
mapping. The support header `crates/recompiler/emit/c/h2tier.h` defines
the boundary and the rare-event results.

An unsupported path or out-of-range result stops the tier. The caller
must restore all state the attempted execution can modify before running
the reference tier. [The write census](sim-tier-writes.md) describes how
the experiment identifies what can change outside the simulation arena.
The normal player uses the [desktop module tiers](../lifter/cranelift-tier.md).

## 6. Leaf experiment

[Sim-tier experiment](sim-tier-experiment.md) gives the five roots, the
comparison with the reference and Rust ports, and the acceptance checks.
