# studio-proto

The studio agent API's names, short forms and client: JSON-RPC 2.0 over a local Unix socket, as the game's agent API. `h2 studio <call>` and the editor's socket both use it.

## Where it sits

`apps`. Uses `agent-proto`. Used by `cli` and `studio`.

## Layout

- `src/lib.rs`: method names and parameter types.
- `src/short.rs`: short method forms.
- `src/cli.rs`: the command-line client.
- `src/tests.rs`: unit tests.

## Tests

`cargo nextest run -p studio-proto`.

## Docs

- [agent-api.md](../../../docs/editor/agent-api.md)
- [agent-api.md](../../../docs/runtime/agent-api.md)
