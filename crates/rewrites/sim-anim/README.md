# sim-anim

Native animation sampling: each guest animation is copied out once into a `Clip` and then sampled into the guest's node records with the guest's arithmetic in the guest's order.

## Where it sits

`rewrites` domain, guest-free. Depends on `sim-core`. Used by `engine` (`engine::sim_anim` is the adapter).

## Layout

- `src/clip.rs`: `Clip` and the `ClipStore`.
- `src/codec.rs`: the per-channel codecs.
- `src/kernel.rs`: `sample`.
- `src/store.rs`: decoded-clip storage.

## Tests

`cargo nextest run -p sim-anim`. No disc needed; the differential test against the lifted original is in `engine` (`sim_anim_differential`).

## Docs

- [engine/anim-sampling.md](../../../docs/engine/anim-sampling.md)
- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md)
- [engine/sim-rewrite-objects.md](../../../docs/engine/sim-rewrite-objects.md)
