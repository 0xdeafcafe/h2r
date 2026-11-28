# timeline

Near-zero-cost timeline events for performance work: frames, guest thread
slices, kernel waits, interrupt and DPC delivery, push-buffer drains and GPU
submits. Recording writes Chrome trace-event JSON that Perfetto and
`chrome://tracing` open. A hook is one relaxed atomic load and a branch
while recording is off. It also keeps the frame counter and per-frame
stats.

## Where it sits

Domain `runtime`; no dependencies, so the runtime, kernel and `gfx`
can all call it. `hooks`, `agent`, `perf`, `render-bridge`, `native-render`
and `live-telemetry` use it too.

## Layout

- `src/lib.rs`: events, recording and the trace writer.
- `src/phase.rs`: frame phases.
- `src/host.rs`, `src/native.rs`: host-side and native-renderer events.

## Tests

`cargo nextest run -p timeline`. One benchmark is `#[ignore]`d.

## Docs

- [docs/perf/profiling.md](../../../docs/perf/profiling.md): `H2_FRAME_STATS`
  and the trace.
- [docs/runtime/xbox.md](../../../docs/runtime/xbox.md)
