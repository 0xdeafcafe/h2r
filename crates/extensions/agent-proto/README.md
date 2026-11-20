# agent-proto

The agent API's wire types: JSON-RPC 2.0 over a Unix socket, one JSON object per line. The server and every client share these definitions.

## Where it sits

`extensions`. No workspace dependencies. Used by `agent`, `boot`, `cli`, `game`, `studio` and `studio-proto`.

## Layout

- `src/lib.rs`: the request and response envelope.
- `src/client.rs`, `src/endpoint.rs`: the client and the socket endpoints.
- One module per method group: `state.rs`, `render.rs`, `hot.rs`, `trace.rs`, `shadow.rs`, `sim/`, `pad.rs`, `raw.rs`, `theater.rs`, `test_maps.rs` and more.
- `src/expect.rs`: expectations for checks.

## Tests

`cargo nextest run -p agent-proto`.

## Docs

- [agent-api.md](../../../docs/runtime/agent-api.md)
- [snapshots.md](../../../docs/runtime/snapshots.md)
- [test-maps.md](../../../docs/game/test-maps.md)
