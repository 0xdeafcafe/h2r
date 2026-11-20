# live-telemetry

Non-invasive guest-state telemetry for the retail game, and its live ingest. Guest hooks always run the original function; the transport is asynchronous.

## Where it sits

`live`. Uses `hooks`, `engine`, `guest-layout`, `runtime`, `timeline`, `map`, `tags`, `live-proto` and `live-server`. Used by `agent`, `boot` and `game`.

## Layout

- `src/hooks.rs`, `src/hooks/`: the observing hooks.
- `src/layout.rs`, `src/memory.rs`, `src/tags.rs`: guest readers.
- `src/event.rs`, `src/pipeline.rs`, `src/sink.rs`: events and their transport.
- `src/session.rs`, `src/server.rs`: sessions and the ingest.
- `src/havok.rs`, `src/havok_agents.rs`, `src/motion.rs`, `src/impacts.rs`: physics observation.
- `src/bench.rs`: ingest benchmarks.

## Tests

`cargo nextest run -p live-telemetry`. The `#[ignore]`d session and bench tests are measurements and need no disc.

## Docs

- [live-telemetry.md](../../../docs/live/live-telemetry.md)
- [havok-runtime.md](../../../docs/engine/havok-runtime.md)
- [agent-api.md](../../../docs/runtime/agent-api.md)
