# lockstep

The determinism harness: a hash of guest RAM at every guest tick and present, a trace of those hashes, and a comparison of two traces that names the first record and the RAM region where they part. It checks that host presentation changes (60, 120, uncapped) never change the guest's run.

## Where it sits

`extensions`. Uses `runtime`, `hooks`, `engine`, `xapi` and `input`. Used by `app`, `boot`, `cli`, `game`, `coop` and `snapshot`.

## Layout

- `src/lib.rs`: `RamHash`, `hash_guest`, `hash_sim`, the traces and their comparison.
- `src/pools.rs`: the RAM pools hashed.
- `src/synthetic.rs`: synthetic traces for tests.

## Tests

`cargo nextest run -p lockstep`. `tests/synthetic.rs` needs no disc.

## Docs

- [high-framerate.md](../../../docs/game/high-framerate.md)
- [live-coop.md](../../../docs/live/live-coop.md)
- [theater.md](../../../docs/game/theater.md)
