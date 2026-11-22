# background

Background work in the game process: named threads below the game's own priority (utility QoS on Apple systems, nice on Linux) and the worker cap for background jobs, so a map load never waits behind a background build for CPU.

## Where it sits

`runtime` domain. Depends on `libc`. One file, `src/lib.rs`; `spawn` and `spawn_scoped` lower the priority of every thread a background job starts.

## Layout

- `src/lib.rs`: `spawn`, `spawn_scoped`, `lower_current`, `NICE` and `workers` (the cap, from `H2_BACKGROUND_THREADS`).

## Tests

`cargo nextest run -p background`.

## Docs

- [runtime/background-work.md](../../../docs/runtime/background-work.md)
