# scene-extract

Guest memory to `FrameScene`: the only render code that knows guest offsets. It reads the guest's records on the sim thread and fills a scene from them.

## Where it sits

`rewrites` domain. Depends on `frame-scene`. Used by `native-render` and `render-bridge`.

## Layout

- `src/layout/`: guest offsets, one file per guest record.
- `src/extract.rs`: builds a scene's views from the layouts.
- `src/guest.rs`: the `GuestRead` trait that the stages share.
- `src/light_groups.rs`, `src/decorators.rs`, `src/late.rs`: extraction per scene part.
- `src/testing.rs`: synthetic guest memory for tests.

## Tests

`cargo nextest run -p scene-extract`. No disc needed.

## Docs

- [graphics/framescene.md](../../../docs/graphics/framescene.md)
- [graphics/native-lights.md](../../../docs/graphics/native-lights.md)
- [graphics/native-decorators.md](../../../docs/graphics/native-decorators.md)
