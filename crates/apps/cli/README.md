# cli

The `h2` binary: building and running the game, the importers, the agent client, simulation, co-op and bot drivers, and the Live admin commands. The offline map, tag and script tools are the `h2-tools` binary (`crates/apps/h2-tools`); `h2 <tool>` runs it ([cli-tools.md](../../../docs/runtime/cli-tools.md)).

## Where it sits

`apps`, the top of the dependency graph. It depends on every domain and nothing depends on it.

## Layout

- `src/main.rs`: the command table.
- `src/tools.rs`: the commands `h2-tools` runs, and running it.
- `src/play_cmd*`, `src/boot_cmd.rs`, `src/host_cmd*`, `src/theater_cmd.rs`: running the game.
- `src/agent_*.rs`: the agent API client (`h2 agent`).
- `src/sim_cmd*`, `src/lab/`, `src/bots_*`, `src/coop_*`: simulation, the bench lab, bot and co-op drivers.
- `src/h1_*`, `src/maps.rs` (`h2 maps install`), `src/mappack_cmd.rs`: Halo 1 import and map packs.
- `src/live_*.rs`: Live service commands.
- `src/emit_cache.rs`, `src/emit_output.rs`: the build and emit output.

## Features

- `default` = `desktop`.
- `host`: the runtime commands, the agent client, simulation and the dedicated-host entry point.
- `desktop`: the windowed client and the authoring, import and diagnostic commands (includes `host`).
- `perf`: the sampling profiler (includes `desktop`).

## Tests

`cargo nextest run -p cli`. Tests that need a disc or built data are `#[ignore]`d and read `H2_DISC` or `H2_XBE`; the lab tests take `H2_PROFILE`, `H2_FPS`, `H2_INPUT_RECORD` and `H2_FRAME_STATS_CSV`.

## Docs

- [agent-api.md](../../../docs/runtime/agent-api.md)
- [switches.md](../../../docs/runtime/switches.md)
- [building.md](../../../docs/runtime/building.md)
- [cli-tools.md](../../../docs/runtime/cli-tools.md)
- [crate-structure.md](../../../docs/dev/crate-structure.md)
