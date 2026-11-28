# perf

Performance tooling: a guest-aware sampling profiler, timeline capture and the benchmark harness. It maps generated code back to guest functions and charges each sample to guest code, runtime, kernel, D3D, gfx or other HLE.

## Where it sits

`tools` domain. Depends on `runtime`, `kernel`, `gfx`, `timeline`, `emit`, `lift` and `ir`. Used by `game`, `boot` and `cli`.

## Layout

- `src/sample.rs`, `src/profile.rs`: the sampler and `Session`.
- `src/symbolize.rs`: generated code back to guest functions.
- `src/remote.rs`: profiles from a device that cannot symbolise them.
- `src/bench.rs`, `src/budget.rs`: benchmarks and budgets.
- `src/hotloop.rs`: synthetic guest loops; no retail bytes.
- `src/cli.rs`: `h2 profile` and `h2 bench`.

## Tests

`cargo nextest run -p perf`. `tests/end_to_end.rs` reads the disc (`H2_DISC`). Profiling switches (`H2_PROFILE`, `H2_TIMELINE`) are in [switches](../../../docs/runtime/switches.md).

## Docs

- [perf/profiling.md](../../../docs/perf/profiling.md)
- [perf/sim-attribution.md](../../../docs/perf/sim-attribution.md)
- [perf/sim-budget.md](../../../docs/perf/sim-budget.md)
