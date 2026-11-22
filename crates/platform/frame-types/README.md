# frame-types

The frame types the guest's Direct3D device and `FrameScene` share: the carried part of a frame by slot, the render camera, and the view and instance keys. Plain data, with no guest addresses and no GPU handles.

## Where it sits

`platform` domain. No workspace dependencies. Used by `d3d`, which records the carried part and classifies its draws against the scene's cameras, and by `frame-scene`, which re-exports `camera`, `carried` and the keys under its own paths.

## Layout

- `src/carried.rs`: slots, the slot tracker, carried ops, targets and readbacks.
- `src/camera.rs`: a view's render camera and the view and projection the native pass draws with.
- `src/keys.rs`: `ViewKind`, `ViewKey` and `InstanceKey`.

## Tests

`cargo nextest run -p frame-types`. No disc needed.

## Docs

- [graphics/framescene.md](../../../docs/graphics/framescene.md)
