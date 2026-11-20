# bots

Multiplayer bots, called Clankers: the brain that turns what a bot sees into controller input, and the phantom peers that play through the host simulation. A bot is driven by a pad source and plays as a remote console's player, never as a local seat.

## Where it sits

`live`. Uses `live-proto`, `input`, `engine`, `tags`, `map`, `pathfind` and `xiso`. Used by `boot`, `cli` and `game`.

## Layout

- `src/brain.rs`, `src/think/`, `src/decisions/`, `src/tactics/`, `src/mind/`: the brain.
- `src/perceive.rs`, `src/vis/`, `src/los.rs`, `src/nav/`, `src/autonav/`: perception and navigation.
- `src/pad_source/`, `src/human_input/`: what drives a bot (brain, script, recording).
- `src/phantom/`, `src/lobby/`, `src/incoming/`: phantom peers.
- `src/weapons/`, `src/grenades/`, `src/vehicle/`, `src/aim_net/`: combat and vehicles.
- `src/tuning.rs`: tuning parameters.

## Tests

`cargo nextest run -p bots`. `tests/map_corpus.rs` is `#[ignore]`d: it reads `H2_DISC` and the installed map pack and Halo 1 conversions in `H2_DATA`.

## Docs

- [bots.md](../../../docs/game/bots.md)
- [sim.md](../../../docs/runtime/sim.md)
- [dedicated-host.md](../../../docs/live/dedicated-host.md)
