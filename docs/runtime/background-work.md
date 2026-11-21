# Background work

Status: Built.
Code: `crates/runtime/background/src/lib.rs` (`background::spawn`, `spawn_scoped`, `lower_current`, `workers`). Tests: the crate's own (`cargo test -p background`).
Switch: `H2_BACKGROUND_THREADS` (unset: a quarter of the cores), listed in [switches.md](switches.md).

Work in the game process that nothing on the frame waits for runs on
threads below the game's own priority, and a job that splits across
threads takes a few cores, not all of them. The guest, render, audio and
input threads then get the cores first, and a map load never waits for
CPU behind a background build. The rule: every thread a background job
starts lowers itself.

## How it works

- `background::spawn(name, f)` starts a named thread that calls
  `lower_current` before `f`. `spawn_scoped` does the same inside a
  `std::thread::scope`.
- `lower_current` lowers the calling thread: `QOS_CLASS_UTILITY` through
  `pthread_set_qos_class_self_np` on Apple systems (the class macOS's own
  indexing uses), nice `background::NICE` (10) through `setpriority` on
  Linux, where a nice value is the thread's own. Elsewhere it does
  nothing. A failure is ignored and the thread runs at the default.
- A QoS class set on one thread does not reliably carry to the threads it
  spawns, so each worker lowers itself. The nav build uses
  `bots::nav::ordered_build_chunks` for collision and tactical probes.
  The visibility build uses `bots::vis::fill` for row batches.
- `background::workers()` is how many threads one background job may
  split across: `H2_BACKGROUND_THREADS` when it is a positive number (at
  most the cores), else a quarter of the cores, at least one
  (`workers_from`).

## What runs on it

| Thread | Code | Work |
|---|---|---|
| `bots-nav` and its build workers | `crates/extensions/game/src/bots_nav_auto.rs`, `bots::vis::Threads::background` | nav meshes and visibility grids requested for boot, lobby selection or play ([bots.md](../game/bots.md) section 10) |
| `bots-map-data` | `crates/extensions/game/src/bot_glue.rs` (`load_map_data`) | a blocking receiver that releases retired meshes and reads manually built or sim maps |
| `native-map retire` | `crates/extensions/game/src/native_map.rs` | releasing world and object resources, including first-person models, after their last queued pass finishes |
| `native-world load` | `crates/extensions/game/src/native_world_glue.rs` | retiring BSP resources, reading and uploading world, lights, fog, water, scene layers and decorator looks |
| `native-objects load`, `native-first-person load`, `native-lights models` | `crates/extensions/game/src/native_objects_glue.rs`, `native_first_person_load.rs`, `native_lights_glue/models.rs` | model geometry and materials for object, arm and light draws |
| `native light materials` | `crates/extensions/game/src/native_lights_glue/materials.rs` | translating runtime light shaders, uploading their images and evicting cached materials |
| `native-decorators load` | `crates/extensions/game/src/native_decorators_glue.rs` | translating and uploading resident decorator materials |
| `native pipeline N`, `native pipeline archive` | `crates/rewrites/native-render/src/pipelines.rs` | compile native shader variants and persist observed descriptors. The workers sleep on their queues between requests |
| `pad-speaker-index` | `crates/extensions/game/src/pad_speaker.rs` | the controller speaker's sound index |
| `film-map-hash` | `crates/extensions/game/src/theater/metadata.rs` | the film's map digest |
| `film-upload` | `crates/extensions/game/src/theater.rs` | a host film's upload |
| `h2-mods` | `crates/extensions/game/src/mods_screen.rs` | the mod list reader ([maps.md](../game/maps.md)) |
| `cloud-saves` | `crates/live/cloud-saves/src/engine.rs` (`Worker::spawn`) | pushing changed saves |

## Rules and limits

A thread stays at the default priority when the frame or the guest can
wait on it, or when it holds a lock the game thread takes for longer than
a moment (a lowered holder would stall the game):

- `vfs-write-behind` (`crates/runtime/vfs/src/write_behind.rs`): the game
  thread waits for a path's queued writes and for room in the queue.
- `vfs-metadata` (`crates/runtime/vfs/src/host.rs`): idle, and it takes
  the lock the game's writes take.
- `fallback-analysis` (`crates/runtime/recomp/src/fallback.rs`): the
  runner waits for it on an unresolved entry. A light host invokes
  analysis on demand in the runner.
- `gfx compile` (`crates/platform/gfx/src/compile.rs`) and `nv2a prewarm`
  (`crates/platform/d3d/src/nv2a.rs`): the render thread waits for a
  pipeline or program it needs.
- The native screen-effects loader (`native-post load`) opens the
  map's post effects. D3D draws them while the worker is active.
- `h2-telemetry` (`crates/live/live-telemetry/src/pipeline.rs`): the game
  thread waits for queue room in `forward` and `settle`, and a worker that
  falls behind drops telemetry.
- `bots-think-*` (`crates/live/bots/src/think.rs`): the game thread waits
  for each tick's thinks (`H2_BOT_THREADS`, off by default).
- Network, audio, input, render and agent threads (`h2-live-http`,
  tunnels, `coop` relays, DirectSound, pads, the agent socket).
- Threads the user waits on in a menu (`h2-theater-library`,
  `h2-theater-archive`, `h2-live-mod`, `h2-live-setup`) and the
  `test map build` the agent API waits on.

Command-line tools that run in the foreground use their own thread
budgets at default priority. `h2 bots nav` uses half the cores, up to
eight, for its parallel build phases.

## Evidence

- **Test**: a spawned thread runs at utility QoS,
  `a_spawned_thread_runs_at_utility_qos` (Apple systems).
- **Test**: `the_default_is_a_quarter_of_the_cores_and_at_least_one`,
  `the_switch_sets_the_cap_within_the_cores`,
  `a_zero_or_bad_value_keeps_the_default`.

## Not verified

- Cold and warm load frame times with the requested map building on
  the quiet laptop. Built-in Unix map backends provide detached readers
  for the builder. Other backends use VFS reads of up to 256 KiB.

## Related

[bots.md](../game/bots.md) section 10 (the nav worker),
[maps.md](../game/maps.md) section 2 (packages resolved inside the VFS
lock), [storage.md](storage.md).
