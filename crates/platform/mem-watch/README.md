# mem-watch

A process-local memory watchdog shared by `h2`, the editor and the lab daemon: footprint history, soft and hard limits, diagnostic reports, sampled allocation stacks and a flight recorder for explaining a leak afterwards.

## Where it sits

`platform` domain. Depends on `serde`, `backtrace`, `wgpu` and platform libraries (`libc`, `objc2-metal`).

## Layout

- `src/limits.rs`, `src/sample.rs`, `src/gauge.rs`: limits, footprint samples and gauges.
- `src/heap.rs`, `src/gpu.rs`: allocation tracking and GPU memory.
- `src/diagnostics.rs`, `src/recorder.rs`, `src/recorder/`: reports and the rotating flight recorder with its analysis and archive.
- `src/agent_activity.rs`: agent-API activity kept with the history.

## Tests

`cargo nextest run -p mem-watch`.

## Docs

- [runtime/switches.md](../../../docs/runtime/switches.md): `H2_MEM_SOFT`, `H2_MEM_HARD`, `H2_MEM_SAMPLE` and `H2_MEM_REPORT_DIR`
- [runtime/agent-api.md](../../../docs/runtime/agent-api.md): "Finding a leak after the fact"
