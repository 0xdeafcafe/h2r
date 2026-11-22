# live-mirror

The editor's live object and player stream, sample store and interpolation. The viewport and the `mirror.*` API read the game's samples here. All guest reads belong to the game's tick observer; this crate consumes the agent protocol's salted handles and world-space units.

## Where it sits

`tools` domain. Depends on `agent-proto`.

## Layout

- `src/lib.rs`: the sample store and interpolation.
- `src/stream.rs`: `Stream`, the live frame stream.
- `src/tests.rs`.

## Tests

`cargo nextest run -p live-mirror`.

## Docs

- [editor/agent-api.md](../../../docs/editor/agent-api.md): `mirror.*`
