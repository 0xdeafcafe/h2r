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
  spawns, so a job's workers each lower themselves (`bots::vis::build_with`
  calls `lower_current` at the top of each scoped worker).
- `background::workers()` is how many threads one background job may
  split across: `H2_BACKGROUND_THREADS` when it is a positive number (at
  most the cores), else a quarter of the cores, at least one
  (`workers_from`).

## What runs on it

| Thread | Code | Work |
|---|---|---|
| `bots-nav` and its grid workers | `crates/extensions/game/src/bots_nav_auto.rs`, `bots::vis::Threads::background` | nav meshes and visibility grids ([bots.md](../game/bots.md) section 10) |
| `bots-map-data` | `crates/extensions/game/src/bot_glue.rs` (`load_map_data`) | reading the loaded map's `.bots` and `.vis` |
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
  runner waits for it on an unresolved entry.
- `gfx compile` (`crates/platform/gfx/src/compile.rs`) and `nv2a prewarm`
  (`crates/platform/d3d/src/nv2a.rs`): the render thread waits for a
  pipeline or program it needs.
- The native renderer's per-map loaders (`native-world load`,
  `native-objects load`, `native-first-person load`, `native-lights
  models`, `native-post load`): the frame draws from them as soon as they
  land, on D3D until then; one thread each.
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

Command-line tools that run in the foreground (`h2 build`, `h2 bots nav`,
the live art and geometry exporters) keep their full thread counts.

## Evidence

- **Test**: a spawned thread runs at utility QoS,
  `a_spawned_thread_runs_at_utility_qos` (Apple systems).
- **Test**: `the_default_is_a_quarter_of_the_cores_and_at_least_one`,
  `the_switch_sets_the_cap_within_the_cores`,
  `a_zero_or_bad_value_keeps_the_default`.

## Not verified

- The CPU a boot with every visibility grid stale takes, and the map load
  time beside it, before and after.
- Whether Rust's `Mutex` on macOS lends a waiting game thread's priority
  to a lowered holder (the bots' VFS reads hold the VFS a 256 KiB chunk at
  a time).

## Related

[bots.md](../game/bots.md) section 10 (the nav worker),
[maps.md](../game/maps.md) section 2 (packages resolved inside the VFS
lock), [storage.md](storage.md).
