# sim-state

Typed, read-only views of the simulation for consumers that are not the simulation: theater, stats, Live and the agent API. Views borrow guest memory for the call and cache nothing.

## Where it sits

`rewrites` domain, guest-free. Depends on `guest-layout`, `sim-objects` and `sim-players`. Used by `agent`.

## Layout

- `src/lib.rs`: typed ids, kinds and the read views.
- `src/players.rs`: players, local users, first-person state and animation channels.
- `src/tests.rs`: tests over synthetic object pools.

## Tests

`cargo nextest run -p sim-state`. No disc needed.

## Docs

- [engine/sim-rewrite-objects.md](../../../docs/engine/sim-rewrite-objects.md)
- [engine/object-update.md](../../../docs/engine/object-update.md)
- [runtime/agent-api.md](../../../docs/runtime/agent-api.md)
