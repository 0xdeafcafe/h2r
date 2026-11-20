# frame-scene

`FrameScene`: the owned, typed snapshot of one rendered frame that the native renderer reads. It holds no guest addresses and no GPU handles, so the editor and the viewer can build scenes too.

## Where it sits

`rewrites` domain, guest-free. No workspace dependencies. Used by `scene-extract`, `native-render`, `render-bridge`, `game` and `boot`.

## Layout

- `src/builder.rs`: `SceneBuilder`, run on the sim thread after the frame's ticks.
- `src/scene.rs`: the sealed `FrameScene` published as an `Arc`.
- `src/camera.rs`, `src/light.rs`, `src/light_group.rs`, `src/object.rs`, `src/node.rs`, `src/layer.rs`, `src/decorator.rs`, `src/late.rs`: the scene's parts.
- `src/summary.rs`: the scene summary hash, so two runs can tell whether they extracted the same frame.

## Tests

`cargo nextest run -p frame-scene`. No disc needed.

## Docs

- [graphics/framescene.md](../../../docs/graphics/framescene.md)
- [graphics/native-lights.md](../../../docs/graphics/native-lights.md)
- [graphics/native-decorators.md](../../../docs/graphics/native-decorators.md)
