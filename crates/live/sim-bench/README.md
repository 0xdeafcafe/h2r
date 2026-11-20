# sim-bench

Sim mode's pure parts: match and batch files, bot tuning files, the paced clock and speed-up, the metrics tally and report, and the A/B comparison of a batch. Nothing here touches the game, so all of it is tested without a run.

## Where it sits

`live`. No workspace dependencies. Used by `boot`, `cli` and `game`.

## Layout

- `src/config.rs`, `src/config/`: match, batch and tuning files.
- `src/batch.rs`: batch definitions.
- `src/clock.rs`: pacing and speed-up.
- `src/metrics.rs`: the tally and the match report.
- `src/compare.rs`, `src/diff.rs`: the A/B comparison.

## Tests

`cargo nextest run -p sim-bench`.

## Docs

- [sim.md](../../../docs/runtime/sim.md)
- [dedicated-host.md](../../../docs/live/dedicated-host.md)
- [crate-structure.md](../../../docs/dev/crate-structure.md)
