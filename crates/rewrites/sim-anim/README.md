# sim-anim

Native animation sampling: each guest animation is copied out once into a `Clip` and then sampled into the guest's node records with the guest's arithmetic in the guest's order. `build_matrices` turns those records into an object's node matrices, as `bd090`'s skeleton walk does.

## Where it sits

`rewrites` domain, guest-free. Depends on `sim-core`. Used by `engine` (`engine::sim_anim` and `engine::sim_anim::skeleton` are the adapters).

## Layout

- `src/clip.rs`: `Clip` and the `ClipStore`.
- `src/codec.rs`: the per-channel codecs.
- `src/kernel.rs`: `sample`.
- `src/skeleton.rs`: `build_matrices`, with the pose matrix and composition leaves.
- `src/store.rs`: decoded-clip storage.

## Tests

`cargo nextest run -p sim-anim`. No disc needed; the differential tests against the lifted original are in `engine` (`sim_anim_differential`; the skeleton walk through `sim_physics_differential::object_node_matrices`, and against the register-level loop in `engine::sim_anim::skeleton`'s unit tests).

## Docs

- [engine/anim-sampling.md](../../../docs/engine/anim-sampling.md)
- [engine/skeleton-walk.md](../../../docs/engine/skeleton-walk.md)
- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md)
- [engine/sim-rewrite-objects.md](../../../docs/engine/sim-rewrite-objects.md)
