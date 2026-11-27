# Agent API: level loads, the host, lobby and menus

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

## Load timeline

`load.timeline {"last": N}` is a core method (8 by default, 1 to 32 kept;
`crates/extensions/agent/src/loads.rs`).

```sh
h2 agent loads                     # the last 8 loads, BSP switches and stalls
h2 agent wait '{"phase":"gameplay","map_ticks":30}' && h2 agent loads 1
```

- **What a load is.** Read at each tick: the map changed (`map`), the BSP
  changed (`bsp`), or no tick came for 500 ms (`stall`, the point the
  `loading` phase starts). The run's first tick ends a `boot` load timed
  from when the agent started. Each load has a `stall` segment (the last
  tick before to the first after) and a `settle` segment (the next 30
  ticks); a load cut short by the next one says `settled: false`.
- **What each segment counted.** The difference of the frame-stats running
  totals (`timeline::phase::totals`) across it: frames, ms per phase
  (`cpu`, `gpu-wait`, `render-wait`, ...) and each counter that moved
  (`read_bytes`, `uploads`, `shaders`, `pipelines`, `readbacks`, ...).
  Frames are counted by present, so a load screen that presents shows its
  frames; one that does not shows the stall's time only. With frame stats
  off, `no_frame_stats` says the counts are missing.
- **Now.** While no tick has come for 500 ms, `current` is the stall so far,
  with what it has counted.
- Seeks and cache hits are not counted (the kernel's file I/O counts bytes
  and the host time of each file call: `read_bytes` and `file_io_us`).

## Game code: full speed or the slower start

`game.code` (read, no params) says which game code this start of `h2 play`
runs ([cranelift-tier.md](../lifter/cranelift-tier.md)): clang's
full-speed module, or the Cranelift fast tier (`--tier fast`, or
`bridge` while clang builds the full-speed one in the background). Host method,
`crates/extensions/game/src/game_code.rs`; `h2 play` supplies it, other
hosts don't have it.

```json
{"tier": "fast", "full_speed": false,
 "module": "/…/h2boot/…/fast/libh2game.dylib",
 "stamp": "<the module stamp's first line>",
 "background": {"state": "building", "log": "/…/clang-build.log",
                "elapsed_s": 230, "typical_s": 900,
                "step": "emit: 2.6s, 18662 owners"},
 "restart": {"ok": false, "why": "the full-speed version is still building"}}
```

`background.state` is `building`, `ready`, `failed` (from the log: its
`clang (…): N compiled` summary line, or an `Error:` line), or `none` when
this start began no background build (it already runs clang's module, or
`--tier` forced one). `elapsed_s` is the log's age; `typical_s` is the
estimate the banner counts down from; `step` is the log's last line.

`game.code.restart` (act, no params) starts `h2 play` again in this
process: the same executable and arguments by `exec`, with the
pid-named agent socket freed first, so the new start finds clang's module
current and runs it. Unsaved progress is lost. It is refused
(`UNAVAILABLE`, with `restart.why`'s text) unless this start runs the fast
tier, the background build is ready and clang's module exists.

## Returning to the main menu

`game.menu` (act, no params) leaves the running campaign or multiplayer
game for the main menu as the title's own departure does: at the next
tick the agent's guest calls retire the network sessions
(`LEAVE_SESSIONS`) and call `request_mainmenu(0)`, so the return-to-menu
picture and the menu's load run as they do for a player
([level-boot.md](level-boot.md#returning-to-the-main-menu)). Host method,
`crates/extensions/game/src/game_menu.rs`; it needs the `agent-calls`
feature.

The answer comes once the calls are made: `{"from": <mode>, "queued":
true, "calls": 2}`, where `from` is the game's options mode (1 campaign,
2 multiplayer). At the menu, or with no game running, it is refused
(`UNAVAILABLE`, "already at the main menu"). Wait for the menu with
`session.wait`, then measure it:

```sh
h2 agent wait '{"phase":"gameplay","map_ticks":1800}' 120000
h2 agent game.menu
h2 agent wait '{"map":"mainmenu"}' 120000 && h2 agent loads 1
```

## Loading a level

`game.load {scenario, difficulty?, variant?}` (act) queues any campaign
mission or multiplayer map on the disc as the next game, from the main
menu or a running game. `scenario` is a map file's stem
(`03a_oldmombasa`, `lockout`) or a scenario path, any case, or a level of
a campaign installed under the data root (`a30`), resolved as
`h2 play --level` resolves it (`level_boot::resolve_in`). `difficulty`
(campaign only) is `easy`, `normal` (default), `heroic` or `legendary`.
`variant` (multiplayer only) is a game type, `slayer` (default),
`oddball`, `juggernaut`, `king`, `ctf`, `assault` or `territories`: a
local game on that game type's default variant, built by
`VARIANT_DEFAULTS` as the level boot builds Slayer's
([community-gametypes.md](../game/community-gametypes.md)). The options
are the level boot's ([level-boot.md](level-boot.md)), made by the
agent's guest calls at the next tick (`Call::QueueGame`), as
`maps.test.load` queues a test map. Host method,
`crates/extensions/game/src/game_load.rs`; it needs the `agent-calls`
feature.

The answer comes after the next tick: `{name, scenario, kind, difficulty?,
variant?, check, queued}`, `kind` being `campaign` or `multiplayer`,
`check` the game's options check and `queued` whether the options went to
the next transition. Wait for the level with `session.wait` on its `map`
and `phase: "gameplay"`.

Refusals: an unknown level is `INVALID_PARAMS` listing the disc's
levels; so are a `difficulty` for a multiplayer map, a `variant` for a
campaign mission and an unknown difficulty or game type. A co-op or
networked session (simulation modes 2 to 5) is `UNAVAILABLE`: every
machine plays its game, so leave it first with `game.menu`. Without the
`agent-calls` feature, `UNAVAILABLE`.

`game.levels` (read, no params) lists what `scenario` takes:
`{campaign: [{name, scenario}], multiplayer: [{name, scenario}],
difficulties, variants}`, the disc's maps sorted by name (read once and
kept). Installed campaigns' levels are not listed but load by name.

```sh
h2 agent game.levels
h2 agent game.load '{"scenario":"03a_oldmombasa","difficulty":"heroic"}'
h2 agent wait '{"map":"03a_oldmombasa","phase":"gameplay","map_ticks":30}' 120000
h2 agent game.load '{"scenario":"lockout","variant":"ctf"}'
```

## Live session links

`live.join.open {"url":"..."}` submits a website link to the same inbox as
the macOS app and `h2 play --join`. It returns `{"id":1}` on acceptance.
Malformed links return `INVALID_PARAMS`. Reopening the same pending link
returns its existing ID. A different link during a join returns `BUSY`.
The player confirms an agent-delivered join, including at the main menu.
Offline, dedicated host, simulation and film runs return `UNAVAILABLE`, as
does a player run with join links disabled.

`live.join.status` takes no parameters. Its `join` field is null until the
first request. It then contains `id`, `server`, `session` and `phase`.
`queued` means the game has yet to claim the request. `signing_in` waits
for the account picker, `resolving` checks admission, `confirm` includes
the host's gamertag when known, and `joining` waits for the title to connect.
The last outcome stays available as `joined`, `cancelled` or `failed`
with a human-readable `reason` on failure. Use `ui.press` to operate the
in-game confirmation. [Live session links](../live/live-custom-games.md#join-in-halo-2)

```sh
h2 agent live.join.open '{"url":"h2recomp://join?server=https%3A%2F%2Flive.forbes.red&session=8000000000000001"}'
h2 agent live.join.status
```

### Matchmaking transitions

`live.matchmaking.trace` takes no parameters. `transitions` holds up to 128
changes in capture order. `hopper_failures` holds the last block-read failure per descriptor during this process, with the hopper index, guest caller and message. An empty rotation names the unavailable map IDs. Call it after a match starts or returns to the
party. The history is retained across the lifecycle change.

Each row has a `sequence`, an `event` and a `state`. Events are `frame`,
`matchmaking_update` and `release_key`. A key release includes the guest
return address as `caller`. The state includes the guest lifecycle
(`6` is matchmaking progress and `1` is the party lobby), the matchmaking
update's `result` (`3` while active), and the arbitration state and result.

`result` `2` is a started match. Any other value but `3` sends the party
back to its lobby with the title's matchmaking failure, and the game logs
`live: matchmaking ended with result R: <reason> (arbitration state S,
result A)` once, from `game::agent_glue::matchmaking::result_reason`:

| `result` | set by | meaning |
|---|---|---|
| `1` | `0x72330` | the matchmaking setup failed |
| `4` | `0x72260` | the party leader cancelled (squad `+0x49fc`) |
| `5` | `0x707d0` | a party console failed to load the map |
| `6` | `0x72540` | the party changed or left the hopper |
| `7` | `0x66050`, `0x718b1` | the party or the target has no hopper or roster for it |
| `0xe` | `0x717b6` | the target session left its lobby states |
| `0x11` | `0x72260` | this console's arbitration registration failed (peer status above 2) |
| `0x13` | `0x70765` | the update's `+0x10` flag is clear |

`squad` and `target` are null when the session is absent. Each includes its
session id when it has a key, its state and class, hopper index, host and
local peer indices, player and roster counts, whether an arbitration round
is set, and countdown seconds when set. `roster_ids_present` counts the expected player IDs found in the
membership. Peers list their map, selected hopper and arbitration status
and whether they are phantoms. Unavailable readings are null. The
history contains no exchange keys, names or player IDs.

`game::agent_glue::matchmaking` samples the lifecycle at `RENDER_FRAME`,
the matchmaking result after `0x706f0`, and the target before the key
cleanup at `0x5fb60`. The hosting update `0x71730` compares the target's
player and roster counts and IDs at `0x71982` before checking map and
arbitration readiness at `0x71a58` and `0x71b0d`.
[Clankermatch lobby](../game/bots.md#84-search-and-lobby-built-the-empty-search)

```sh
h2 agent live.matchmaking.trace
```

## `host`: the process and its lobby

### `host.perf`

The newest host perf line (`boot::host_perf`): `{line, age_ms}`;
`UNAVAILABLE` until one exists (`H2_HOST_PERF=1`; one every 100 frames or
5 s). `h2 agent hostperf`.

### `host.metrics`

`h2 agent hostmetrics`; no parameters. It reports the latest complete
window of 30 intervals between returns from `GAME_TICK` (`0x00137fe0`).
The first reading is available after 31 tick returns; until then it
answers `UNAVAILABLE` ([`crates/extensions/game/src/host_metrics.rs`](../../crates/extensions/game/src/host_metrics.rs)).

| Field | Meaning |
|---|---|
| `ticks` | Tick intervals in the sample (30) |
| `game_cpu_ms_per_tick` | CPU time on the game thread per tick interval, including work between ticks |
| `process_cpu_ms_per_tick` | CPU time on all process threads per tick interval |
| `wall_ms_per_tick` | Elapsed wall time per tick interval, including pacing sleeps and any pause or load crossed by the window |
| `wall_ms_per_tick_p99` | On a light host, nearest-rank p99 of individual multiplayer tick intervals ending in the last 30 s. The first tick on entering gameplay starts the clock. An empty window gives `null`. |
| `game_cpu_ms_per_tick_p99` | On a light host, nearest-rank p99 of per-tick game-thread CPU deltas (the thread clock's change between consecutive ticks) over the same 30 s window; it clears when gameplay ends, so each match starts empty. `null` when the window is empty or the platform has no thread clock. |
| `late_ticks` | Number of intervals in that same window longer than 36 ms. Both interval fields are `null` outside the light profile. |
| `cpu_percent` | Whole-process CPU over the wall interval; 100 is one fully used core |
| `cpu_seconds` | User and system CPU seconds consumed by this game process through the request |
| `package_idle_wakeups`, `interrupt_wakeups` | Process-lifetime counts from macOS `proc_pid_rusage` (`ri_pkg_idle_wkups` and `ri_interrupt_wkups`). Each successful request samples both counters together. |
| `wakeups_at_ns` | Monotonic sample time in nanoseconds, or `null` when wakeup counters are unavailable. For two readings from the same process, divide the change in each count by the change in `wakeups_at_ns` and multiply by 1e9 for wakeups per second. |
| `rss_bytes` | Resident bytes at the end of the interval on macOS and Linux, including resident shared pages |
| `pss_bytes`, `peak_pss_bytes` | Current and highest sampled Linux proportional resident bytes. Each `host.metrics` request samples the process. |
| `private_bytes`, `shared_bytes` | Private and shared resident mappings in Linux smaps, each including clean and dirty pages |
| `private_dirty_bytes`, `shared_clean_bytes` | Linux smaps Private_Dirty and Shared_Clean in bytes, for comparing copy-on-write and file sharing |
| `age_ms` | Time since the 30-tick CPU sample ended |
| `host_light` | Whether the process uses the light host profile |

A platform counter that is unavailable is `null`. Compare active
gameplay windows after loading; a stale sample can survive a gap in ticks.
The CPU fields exclude sleeping; the wall field shows the pace the host
sustained.

### `host.memory`

`h2 agent hostmemory` reads the process watchdog. `h2 agent hostmemory report`
sends `{"report":true}`, collects a diagnostic report on the game host, and
returns its path as the last item in `reports`. Collection runs on the
connection thread so the game can continue ticking. A write failure returns
`UNAVAILABLE`. An ordinary read takes no parameters and creates no file.

| Field | Meaning |
|---|---|
| `footprint_bytes`, `peak_bytes` | Latest physical footprint and highest footprint sampled by the watchdog, in bytes |
| `soft`, `hard` | Process limits in bytes. A zero disables that limit. |
| `growth_bytes_per_min` | Estimated footprint change per minute |
| `samples` | Last 60 samples, oldest first, as `[milliseconds_since_watchdog_start, bytes]` |
| `probes` | Named subsystem counts, including pending requests, socket reply buffers and queued pad steps |
| `reports` | Recent report paths on the game host |

### Explain memory growth

The memory watchdog exposes these map-load and cache probes through
`host.memory`, its reports, and the flight recorder. Byte gauges follow
the listed owners. The process footprint also includes driver allocations,
in-flight GPU work, and heap pages the allocator keeps for reuse.

| Probe | Ownership measured |
|---|---|
| `native.map_retired_payloads` | Detached native payloads queued for or being destroyed by the retirement worker |
| `native.decoded_image_bytes` | Capacity of shared native RGBA, mip and BC payloads, counted once per allocation |
| `native.light_model_entries` | Models retained for standalone light and shadow passes |
| `native.world_cpu_geometry_bytes`, `native.object_geometry_buffer_bytes` | Retained world vertex/index vector capacity and live object vertex/index buffer size |
| `native.world_buffer_cache_bytes`, `native.world_section_payload_bytes`, `native.transparent_packed_bytes` | Indexed draw caches and packed transparent geometry |
| `native.world_section_windows`, `native.transparent_packed_bindings` | Live indexed windows and packed binding entries |
| `gfx.pipeline_cache_entries`, `gfx.shader_cache_entries`, `gfx.texture_group_cache_entries` | Entries across live renderers or binding sets |
| `gfx.pipeline_draw_jobs`, `gfx.pipeline_prewarm_jobs` | Queued compile slots, including cancelled slots awaiting removal |
| `d3d.nv2a_program_entries`, `d3d.nv2a_archive_pending_keys`, `d3d.nv2a_archive_pipeline_records` | Translated program results, unregistered archive keys and archive pipeline records |
| `gfx.upload_ring_staging_bytes`, `gfx.upload_ring_buffer_bytes` | CPU capacity and GPU buffer size for live upload rings |
| `gfx.upload_idle_cpu_bytes`, `gfx.upload_idle_gpu_bytes` | The idle subset of the corresponding upload ring totals |
| `assets.rewritten_map_cache_bytes`, `assets.rewritten_shared_map_cache_bytes` | Rewritten level and shared resource payloads in the VFS cache |

Use the memory history to compare the menu, level activation, settled
play, and the return to the menu. For a short peak between recorder rows,
read `host.memory` during the load. `host.memory.heap` breaks out live
Rust allocations from the `native.*.load` and `assets.map_rewrite`
scopes and can sample their allocation stacks.

### Finding a leak after the fact

Every watched process (`h2 play`, `run`, `serve`, `h2 lab host` and
`h2-editor`) keeps evidence that outlives it
(`crates/runtime/mem-watch/src/recorder.rs`):

- **Flight recorder.** Every 10 s the process appends one JSON line to
  `mem-<pid>.ndjson` in its report directory (`H2_MEM_REPORT_DIR`, else
  `logs/mem/` under the data directory). A row holds `t_ms` (watchdog
  uptime), `unix_ms`, `footprint_bytes`, `soft`, `probes`, `tags` (live
  Rust heap bytes per scope tag), `stacks` (sampled stacks, when
  sampling is on), `gpu` (the census below), `level`, `tick`,
  `agent_connections` and `agent_calls`. Each line is written straight
  to the file, so a kill loses at most a torn final line, which readers
  skip. Segments rotate hourly or at 8 MiB, seven per process. An orderly
  exit appends a `{"closed_cleanly":true}` footer.
- **Reports.** A soft-limit, sustained-growth or hard-limit trigger
  writes `mem-<pid>-<nanos>-<serial>.json` beside the recorder (written
  to a `.partial` file, then renamed, before the OS diagnostics run). The
  hard limit's report is written before the process exits with code 86.
  Under `h2 play --follow --fresh-data` and `--watch --fresh-data`, whose
  data dir (`<cache>/data/current`) is emptied at every launch, the
  child's report directory is `<cache>/data/mem` unless
  `H2_MEM_REPORT_DIR` is set, so a game the watchdog stopped leaves its
  evidence for `h2 mem postmortem <file>` (`crates/apps/cli/src/follow.rs`).
- **Recovery.** A starting recorder checks the directory for streams
  whose owner lock is free. A stream without the footer, or whose last
  row is over the soft limit, moves to `reports/mem-<pid>-<nanos>/` and
  the process logs `memory recorder: recovered pid N (...)`. A clean
  stream is deleted. Recorders and preserved streams share a 512 MiB
  budget per directory. When it is full, a new process runs without a
  recorder and logs where to move the evidence from.
- **Heap tags.** `h2` and `h2-editor` allocate through
  `mem_watch::TaggedAllocator`. A one-word trailer on each allocation
  names the scope tag current on the allocating thread, so a free on
  any thread credits the same tag. Scope guards
  (`let _scope = mem_watch::scope("assets.bitmap");`) sit at the
  suspects: asset and bitmap loads, pipeline creation, the native world
  pass, agent requests and replies, trace and input recording, the perf
  sampler and the editor's map loads and frames. Allocations outside a
  guard count as `unscoped`. The counts are requested bytes; foreign
  heaps (the GPU driver, C libraries) show only in the OS footprint.
- **Sampled stacks.** With `H2_MEM_SAMPLE` set to a mean interval such
  as `1M`, the allocator records the stack of a Poisson-sampled
  allocation and keeps it until the allocation is freed. Large
  allocations are proportionally more likely to be sampled. Unset, no
  stack is captured.
- **GPU census.** Each renderer registers its wgpu device. The census is
  wgpu's `generate_report` counts of live user-held resources
  (`wgpu.texture_count`, `wgpu.buffer_count` and the rest), HAL byte
  counters on Vulkan and D3D12, Metal's `currentAllocatedSize` on macOS
  as `metal.current_allocated_bytes`, and the renderer cache probes
  (`gfx.*`, `native.*`).

All five methods run on the connection thread, one at a time per process;
a second concurrent call is `UNAVAILABLE`.

| Method | Params | Result |
|---|---|---|
| `host.memory.history` | `{"since_ms"?}` | `{"records":[row],"next_since_ms"}`: up to 24 rows or 512 KiB from `since_ms`; call again with `next_since_ms` until it is `null` |
| `host.memory.heap` | `{"top"?: 1..128, default 20}` | `{"sample_interval_bytes","dropped_samples","tags":{name:{"live_bytes","live_allocations"}},"stacks":[{"live_bytes","live_allocations","frames":[{"ip","symbol"}]}]}`; `UNAVAILABLE` unless `H2_MEM_SAMPLE` was set at start |
| `host.memory.gpu` | `{}` | the census, `{name: value}`; names ending `_bytes` are bytes, the rest counts |
| `host.memory.diff` | `{"from_ms"}` | `{"from_ms","to_ms","footprint_delta","growers":[change]}` from the row at or before `from_ms` to the newest; `UNAVAILABLE` outside the retained history |
| `host.memory.postmortem` | `{"pid"?}` | the newest dead process's evidence in this process's report directory: `{"pid","file","diff","reports","report"}` |

A `change` is `{"category","name","unit","before","after","delta"}`.
`category` is `tag`, `stack`, `gpu` or `probe`; `unit` is `bytes` or
`count`. Growers are ranked by `delta` within each unit, bytes first.
Categories overlap (a texture's bytes can show in a tag, a stack and the
GPU census), so never add them together.

A postmortem prefers the newest dead flight recorder, then falls back to
watchdog and lab reports (`mem-<pid>-*.json`, `lab-host.json`) for a
process that died before its first row. `report` carries the newest
matching report's identifying fields; the full report, with `vmmap` and
`footprint` output, stays at its path in `reports`.

Offline, with no game running:

```sh
h2 mem postmortem                 # newest dead process in logs/mem/
h2 mem postmortem --pid 4242
h2 mem postmortem ~/saved/mem-4242.ndjson
h2 agent postmortem --pid 4242    # the same, through the agent CLI
```

For a lab job, `h2 lab mem <id>` fetches the job's `mem/` directory and
prints the largest growers ([lab.md](lab.md#memory-limits-and-reports)). A typical hunt:
read `host.memory` for the growth rate, `host.memory.diff` from a time
before the climb to name the growing tag or GPU count, then restart with
`H2_MEM_SAMPLE=1M` and read `host.memory.heap` for the stacks inside it.

### `host.audit`

Set `H2_HOST_AUDIT=/path/to/audit.json` before boot to register this
method (`crates/extensions/game/src/host_audit.rs`). `h2 agent hostaudit` takes no
parameters. It writes a snapshot to the configured path and returns the
path, ticks, observed and registered function counts, tag range count and
inclusive CPU timing. Each snapshot also flushes any enabled map read
recording for the private store build. An orderly exit writes the final
snapshot. Call the method before terminating a dedicated process externally. The collector
changes dispatch and memory-watch costs, so use `host.metrics` for sizing.
Report fields and corpus: [sim.md](sim.md#host-audit-corpus).

### `host.lobby`

A System Link light host started with `H2_HOST_LOBBY=1` registers these
four game methods (`crates/extensions/game/src/host_lobby.rs`, types
`crates/extensions/agent-proto/src/host_lobby.rs`). Each result also carries `at`.

| Method | Parameters | Result |
|---|---|---|
| `host.lobby.open` | `{"map":"<short name>","variant":"<name>","max_players":8,"live":false}` | System Link: an empty object. Live: the advertised `session_id` |
| `host.lobby.state` | `{}` | `{"phase":"lobby","players":[{"slot":3,"peer":1,"name":"<name>"}],"ready":true}` |
| `host.lobby.start` | `{}` | an empty object when retail accepts the start request; observe `state` for load and play |
| `host.lobby.close` | `{}` | an empty object once the network lobby has closed and the host is back at the menu |

The host looks up `map` by short name, scenario path or English display
name. The optional `variant` defaults to Slayer; it accepts a disc
variant, a signed and valid saved variant present at boot, a stock game
type or a built-in Live playlist variant key, case-insensitively. If a
name matches two different saved or disc rulesets, open is
`INVALID_PARAMS`. `max_players` is an integer from 1 to 16. The required
`live` boolean selects System Link or a Live custom game; Live needs a
working service identity that can publish sessions. Open waits for retail
to publish the requested lobby parameters. Repeating the same open in
pregame returns the existing lobby. Close it before changing parameters.
Set `"synchronous":true` with `"live":false` and omit `variant` for a
variant-free System Link probe. Open checks that the fresh squad keeps
protocol 1, engine index 0 and the difficulty sentinel for multiplayer
after the map setter. The lobby uses retail's normal readiness and start calls.

The phases are `menu`, `lobby`, `loading`, `playing` and `postgame`.
Player slots are retail membership slots and can have gaps; split-screen
players on a remote console share a peer index. An empty, joinable lobby
reports `"players":[]` and `"ready":false`. Readiness needs a remote
player, party leadership, the requested map and limit, loaded maps on every
peer, and retail's ready-to-start result. Any connection may read state;
mutations respect `session.claim`, and a concurrent mutation is `BUSY`.
[Playerless lobby and the two-console check](../live/dedicated-host-ops.md#playerless-lobby).

## `ui`: what the menus show

`ui.screen` needs `H2_UI_WATCH=1`. Other UI methods read guest widgets
or the game's UI state without UI watch. All are read-only except
`ui.press`.

- **`ui.screen`** (`h2 agent ui`): the menu screen player one's input goes
  to, read each UI update, including during a campaign pause
  (`game::ui_watch`): `{screen, window, slot, class?, live_menu_row, mode,
  dialog, age_ms, layout_findings, content_findings}`, or `UNAVAILABLE` before the first read. `class` names
  a Rust list screen whose numeric ID identifies its retail skin.
  In a paused game, Player Settings reports screen 196 or 200. Its Video
  Settings and Voice children report classes `video` and `voice` on
  screen 200. Press B or Back with `h2 agent press` (`input.pad`) to
  check that the previous screen returns on the same window and slot. See the
  [pause Settings walkthrough](../game/settings-menu-screens.md#to-check-in-game).
  The main menu's one row of ours, Theater, follows System Link, so
  Campaign, Xbox Live, Split Screen and System Link keep their retail rows
  0 to 3 and lobby scripts press them there. `main_menu_theater` and
  `main_menu_rows` (rows of ours between Xbox Live and Split Screen) are
  absent; `agent_proto::sim::menu` still reads them from older logs. `dialog` is the dialog up (the first screen on windows 0 to
  4, any slot), or `null`: `{id, name, window, slot}`, with `name` the dialog's
  recognised name (`dialog_live_signin` for the account picker,
  `confirmation_dialog`, `error_dialog_ok`, ...; `boot::ui_watch::dialog_name`)
  or `null` for an id not named yet. The party leader picker reports
  `dialog.name = party_leader_picker` with id `0xb1`. The Clanker player
  card reports `clanker_player_options`. The retail-style value picker
  used by Theater reports `kit.picker` in the main menu and
  `kit.picker.game` in game. These names identify the active kit class.
  Their numeric ids identify the shared retail base class. Dialogs read as
  `"screen": null`, so this
  is how a driver tells one from another. The `ui: screen none` log line
  names it the same way ([live-coop.md](../live/live-coop.md), 12.6).
  `layout_findings` contains visible text and row rectangles outside the
  title-safe area or their container. Each entry has `screen`, `widget`,
  `rect` ([top, left, bottom, right] in UI units), `text` when readable,
  and an `issue` with a `kind` (`unsafe`, `outside_panel` or
  `outside_container`). It samples the input screen and the active dialog.
  `content_findings` reports broken strings, dead controls and missing
  content. Every entry has `screen` and `issue: {kind, id, detail}`.
  Findings from instantiated widgets include `widget` and `text`.
  Findings from a registered Rust page model include
  `source: "page_model"` and `screen_instance`. Model findings use the
  currently focused row and the installed fonts when available. The
  read does not activate controls. The
  [UI builder](../game/ui-builder.md#content-findings) checks each named
  state and exercises offline focus and input routes.
- **`ui.stack`** (`h2 agent ui.stack`, no parameters): a tick-boundary
  read of the occupied channels, their active, pending and outgoing
  screen instances, and the game-shell channel's saved screen IDs
  (`history`, nearest parent first). Each screen has `id`, `instance`
  (a guest address valid for that screen's lifetime), and `parent` where
  a return is known. The map picker stages share an ID and have
  distinct `mode` values (`folders`, `maps`, `pack`, `variants`), with the open
  category, pack or map named in `folder`. A map's page of variants has its category's maps as its parent. A lobby variant list shows Change Game Type as its parent, including that selector's Game Setup parent when present. A pack's parent is the Mods list. All stages show the active picker row in `focused_row`
  (zero-based). Game-type and variant lists also expose their focused row.
  A custom variant list identifies its group in `variant_category` and a kit
  screen identifies its class in `class`. Both fields are null elsewhere.
  `input` uses the same settled player-one target as `ui.audit`. `back` gives the instance that closes and its parent where
  known, with action `close`, `reopen` (recreate a parent), or `wait`
  (consume Back during a replacement). It is null when there is no menu
  input target or when Back depends on a screen's custom handler. The kit
  value picker's `close` target identifies the settled screen underneath it.
  `kit` is the [kit primitive](../game/ui-screens.md#cards-and-the-card-row)
  on top, as its screen last said:

  | Field | While | Holds |
  |---|---|---|
  | `kit.cards` | a card screen is up (`firefight.wavesets`) | `screen`, `layout` (`size`, `well`, `gap`, `visible`, `top`), `cards` (how many), `focus` and `first` (zero-based), `name` (the focused card's), `offset` (the slide now, UI units; 0 at rest) |
  | `kit.floating` | a [floating menu](../game/ui-screens.md#the-floating-menu) is up (`forge.object-actions`) | `screen`, `anchor` (`world` or `screen`), `point` (the anchor's point, UI units; null behind the camera), `list` (the list's anchor), `clamped` (moved to stay in the safe area), `focus`, `actions` (the labels) |

  Each is null while no such screen is up. A floating menu's `focus` is
  the row it opened on.
  `ui.stack` is available without UI watch.
- **`ui.campaign`** (`h2 agent ui.campaign`, no parameters): a tick-boundary
  read of the campaign menu from the guest screen and focus, without UI
  watch (`crates/extensions/game/src/campaign_menu.rs`). With no installed Halo 1
  campaign it returns `{"installed":false}`. With one it reports `page`
  (`picker`, `options`, `levels`, `difficulty`, `dialog` or `other`),
  `game` (the selected campaign key on the three game-specific pages),
  `rows`, `focused_row` (zero-based) and `focused`; unknown selection or
  focus is null. On the picker the rows are installed campaign keys.
  With a focused picker row, `card.description` is the UTF-8 text in
  that row's guest string slot, or null before it is available.
  `card.picture_datum` is the selected bitmap datum, or null when the
  artwork area is empty. `card` is null on other pages or without a
  focused row. On Campaign Options the action keys are `resume`, `new_game`,
  `select_level` and `co_op`; other pages report an empty row list. For
  example, a fresh profile on Halo 2 has `"page":"options"`, `"game":"halo2"`
  and three rows. With a saved Halo 2 checkpoint, Resume is the first of
  four rows.
  On the picker and Campaign Options, `pick` identifies the most recent
  press during this visit. It is null before a game is picked. Its
  `campaign` is the picked numeric id, and `request` is null until retail
  receives the options screen request. The request gives the player
  `mask`, `window`, `slot` and `loader` arguments. The active `page`
  shows whether the options screen opened.
  On those two pages `list` reports guest addresses for the `screen`,
  its list `handler`, the `row_array` and the linked `children` in order.
  `row_count` reads the number of active datums. On Halo 2 with a checkpoint
  the four actions scroll through three linked item widgets. The handler's
  `press_record` holds a `press_target` (the list) and a `press_function`
  (the row-chosen callback). An unreadable field is null. On other pages
  `pick` and `list` are null.
  From the main menu's Campaign row, `h2 agent press a` opens the picker for
  fresh and returning profiles. Main menu row zero requests the native
  campaign dialog from `0x230888`; the picker intercepts it before the
  later profile-dependent solo branch at `0x2b16ad`. With UI watch on,
  `lobby mode campaign` also recognises the installed picker.

- **`ui.browser.filters`** (`h2 agent ui.browser.filters`, no parameters):
  reads the active Custom Game Browser preferences, even when the browser
  is closed. The result is `{"filters":{...}}` with the same field names
  and values as the [Live session query](../live/live-api-sessions.md#browser-filters).
  Unset fields are omitted. With the default choices, it returns
  `{"filters":{}}`. After opening Game Filters with Y and choosing a row
  value with A, read this method to verify the selection.

- **`ui.lobby_options`** (`h2 agent ui.lobby_options`, no parameters):
  `{"lines":[...]}` with the active custom game panel's visible Quick
  Options lines in display order, up to six. This reads the lobby's text
  widgets at a game boundary (`crates/extensions/game/src/lobby_options.rs`).
  It returns `UNAVAILABLE` when the custom game pane is unavailable.
  Open a pregame custom lobby to read its retail lines and any Clankers,
  Halo 1 Options and Skulls summaries. After changing the settings,
  close the dialog and read the panel again.

- **`host.peer_rtts`** (`h2 agent host.peer_rtts '{}'`) reads the active simulation
  membership's reliable-channel RTT at a game boundary. The result contains
  `players` with the same slot, peer, name, bars and nullable RTT as
  `ui.lobby.players`. Local players, phantoms and unsampled routes have
  null RTT. The method returns `UNAVAILABLE` without a simulation membership.
  Compare a remote client's `rtt_ms` to its matchmaking assignment's regional
  probe sample during the match.

- **`ui.lobby.players`** (`h2 agent ui.lobby.players`, no parameters):
  reads the pregame player list on a game boundary without UI watch
  (`crates/extensions/game/src/lobby_quality.rs`). Each entry has its
  membership `slot`, console `peer`, `name`, `bars` (1 to 6) and `rtt_ms`
  (milliseconds, or null when there is no network sample). Split-screen
  players on a console share its peer and bar count. Local players and
  phantoms have six bars and null RTT. Remote peers with no sample have
  one bar. The list uses the squad's membership in custom games and the
  target membership when the matchmaking lobby has one. Results are in
  slot order, with an empty list outside pregame.
  For example, `{"players":[{"slot":1,"peer":1,"name":"Clanker","bars":6,"rtt_ms":null}]}`.

- **`ui.lobby.tabs`** (`h2 agent ui.lobby.tabs`, no parameters): the
  pregame lobby's modes ([party-lobby.md](../game/party-lobby.md)),
  read on a game boundary (`crates/extensions/game/src/lobby_tabs/header.rs`).
  The result is `{place, leader, selected, header, tabs, clanker, notice}`:
  `place` is `split_screen`, `system_link` or `xbox_live`; `leader` is
  whether this console leads the party (a member has no prompt line and
  its X is retail's); `selected` is the mode the lobby is in, and
  `header` the text the lobby's update gives its header (`CUSTOM GAMES`,
  `FORGE`, ...). `tabs` lists the modes present, each `{tab, label,
  state, reason}` with `tab` one of `multiplayer`, `custom_games`,
  `forge`, `co_op`, `state` one of `selected`, `on`, `off`,
  and `reason` the status-line sentence of a mode that cannot run here
  (else null). `clanker` is, in MULTIPLAYER, whether the lobby's
  playlist is one the server marks as a Clanker playlist (null in
  another mode). `notice` is the reason the status line shows after a
  pick of a mode that is off, for four seconds, else null. It returns `UNAVAILABLE` when the pregame
  lobby is not the screen on top or its session cannot be read.
  For example, a Split Screen lobby's MULTIPLAYER entry is
  `{"tab":"multiplayer","label":"MULTIPLAYER","state":"off","reason":"Matchmaking needs Xbox Live."}`.

Screens already moved to Rust (the Video screen, the skulls page, the
account, co-op and sign-in screens; [feature-engine.md](../engine/feature-engine.md))
share one list-screen implementation (`crates/extensions/game/src/list_class.rs`).

### Menu inspection and navigation

`ui.audit {}` inspects the instantiated widgets at one game boundary.
`screen` is the underlying menu. `dialogs` contains the active dialog
on each occupied dialog channel, including dialogs belonging to another
controller. `active` is the surface that accepts player one's input.
`target` is its opaque token, and `focused_row` is the zero-based focused
item. A transition or ambiguous input receiver can leave these fields null. Each surface
has a numeric `id`, `window`, controller `slot`, current `pane` and
the kit class `name` when available. Kit dialogs can share a retail ID.

Inspect `panes[].labels` and `panes[].lists[].rows` for the text at the
widget. The audit reads the UTF-16 buffer *after* retail's localized
string lookup. A row includes its datum index, text and individual labels.
On the retail profile picker and action menu, `navigation` identifies a
verified inspection action in the current list mode.
A missing widget, an empty string and an unreadable text interface have
distinct statuses. The list's `item_count` includes off-screen items.
Rows describe the materialized viewport. Moving focus exposes the other
rows of a scrolling list. The Y menu exposes its materialized tabs and
marks the selected pane. Source and widget layout are in
[`ui_audit.rs`](../../crates/extensions/game/src/ui_audit.rs) and
[User-interface tags](../formats/ui.md).

`issues` identifies affected surfaces, panes and rows. It includes
missing or empty row text, empty labels, unreadable data, empty panes
and undersized lists. `transition` includes the startup fade time
remaining, scene fade mode and weight, and the active, pending and
outgoing layers of the menu compositor. A modal can request a blurred
backdrop for as long as it is open. `stuck_blur` means a layer requesting
that backdrop still carries its input-blocking transition more than
one second past its authored delay and duration. Other overdue animations
produce `stuck_transition`. `stuck_fade` means audits sampled the same
unfinished main-menu startup fade and clock reading at least 150 game ticks
apart. Unknown backdrop accessors give `requests_backdrop: null`.
These diagnostics describe compositor inputs. The final image still
requires a rendering check. [Transition reader](../../crates/extensions/game/src/ui_transition.rs)

`ui.press` accepts `{target, action, row?, phase?}`. Copy the token from
the latest audit. Actions are `up`, `down`, `left`, `right`, `choose` (A),
`back` (B), `friends` (Y), `open_live_menu` (A on the Live players
screen in mode 3), and `change_mode` (X in the pregame lobby: the GAME
MODE picker, for the party leader), refused with `GONE` on any other
screen ([party-lobby.md](../game/party-lobby.md#changing-mode); the
picker is then a kit dialog in `ui.audit`, and `ui.lobby.tabs` reads
the mode a pick switches to). Move focus, audit again, then send `choose` with the
observed row. A changed owner, row or requested phase invalidates the
press. The XAPI tap checks the target again when the game reads the pad.
A successful result contains `{delivered:true, target, action, row,
player, at}` after the press and release. An unread gesture expires. Read
`ui.audit` after the menu transition settles to check what the press opened.
`session.methods` lists `ui.press` when the process has an XAPI input tap.

### Crawling the menus

On a running agent-enabled game, place a signed-in profile with a recent
Clanker at the main menu. Set `H2_AGENT` to its socket path.

```sh
h2 agent --socket "$H2_AGENT" ui-crawl --depth 6 --budget-ms 120000 --require-clanker
```

Omit `--socket` to use the normal agent endpoint. The crawl claims one
connection, explores known navigation actions, observes each destination
and backs out before trying its siblings. It visits the Y menu and
Recent Players, including entries for players who left the session.
From Settings it can inspect an existing editable profile, open Controls
and back out of the choice dialogs without selecting a value.
It skips game starts, quit, unverified actions and changes to settings or
accounts. `--from-current` starts at the open menu or dialog for a
focused check. Depth zero audits only the current surface. The defaults
are six menu transitions and a two-minute wall-clock budget.

For a parity run, use `--require-clanker` so missing seed history or an
unreached card fails the run. The JSON verdict lists visited paths,
findings, skipped actions, Clanker cards and navigation errors. Findings
or a failed traversal produce a nonzero exit. Save the report outside the
repository because resolved text can contain profile names and disc strings.
[CLI implementation](../../crates/apps/cli/src/agent_ui_crawl.rs)

### `ui.attract`

`h2 agent ui.attract` takes no parameters. Boot registers it when the Bink
HLE selects playback outside a film recording or replay. It reads the menu's UI clock, idle time, music gate and
movie flags at a game boundary (`timer`). The `state` says whether the
menu is idle, fading, playing the attract movie or keeping its music.
`asset` reports preparation or unavailability and `blocked_reason` gives
the reason for keeping the music during the current idle cycle.

`last_attempt` gives the UI time, idle time and present at the deadline.
Its `outcome` says whether the wrapper started playback, the guard skipped
it as unavailable, or the wrapper failed after preparation.
`returned_ui_ms` is filled when the wrapper closes the attract movie after
input or the final frame. [Attract timer](../audio/bink-hle.md#attract-timer)
describes the sequence.
