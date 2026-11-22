# actor-net

The actor presentation channel for host-replicated sessions: versioned records for each actor, a per-client replica set and a bandwidth budget with per-client scheduling. All identifiers come from the peers' shared map or the replicated entity. It has no guest, game-mode or transport dependency.

## Where it sits

`live` domain. The host runtime (`crates/extensions/game/src/actor_net`) captures and applies the records; this crate holds the data structures and rules.

## Layout

- `src/wire.rs`: the versioned record encoding.
- `src/set.rs`: `ActorSet` and `Replica`.
- `src/budget.rs`: `Budget` and the per-client scheduling.
- `tests/convergence.rs`: host and clients converge.

## Tests

`cargo nextest run -p actor-net`.

## Docs

- [game/ai-networking.md](../../../docs/game/ai-networking.md)
