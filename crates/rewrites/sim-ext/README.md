# sim-ext

`SimExt`, the side arena for simulation state that has no retail home: a versioned container of named, versioned records (`RulesExt`, `ForgeExt`, and any a feature registers). It has one canonical byte form with a CRC-32 and a size cap. Checkpoint slots and a snapshot section carry it; `lockstep::hash_sim` folds its digest into the arena hash when it holds a record.

## Where it sits

`rewrites` domain, guest-free, no dependencies. The guest hooks are `sim_bridge::sim_ext`. Used by `sim-bridge` and `lockstep`.

## Layout

- `src/lib.rs`: kinds, the container, its byte form, CRC-32 and the FNV-1a digest.
- `src/checkpoint.rs`: the per-slot copies kept for checkpoint save and revert, the snapshot section, the process-wide state.
- `src/tests.rs`: round trips, refusals, record isolation, hashes, checkpoint sequences.

## Tests

`cargo nextest run -p sim-ext`. No disc needed.

## Docs

- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md), section 2
- [game/rules-engine.md](../../../docs/game/rules-engine.md), "Roadmap"
- [game/forge-design.md](../../../docs/game/forge-design.md), slice F7
