# Sim mode: metrics and the spatial trace

Status: Built. The tally and report, the spatial trace, sim stores and shared guest RAM. Part of [Sim mode](sim.md).

## 6. Metrics

A feature, `sim-metrics`, observes the game on the game thread, read-only:

| Point | What it takes |
|---|---|
| `GAME_ACTIVATE` (game start) | the match starts: the tally resets |
| `GAME_TICK` (after each tick) | `live_telemetry::memory::snapshot`: every player's score, kills, deaths, assists, suicides, team, unit, vehicle, grenades carried, vitality; team scores |
| `WEAPON_FIRE` | a shot credited to the weapon's holder or to the player in the firing seat of a vehicle or turret |
| `DAMAGE_REPORT` | a hit: the owner player, the victim unit, so its player, and the damage |
| `PLAYER_KILLED` | the kill log: attacker, victim, tick; a betrayal is a teammate killed in a team game (`bots::emote::kill_from_guest`) |
| `GAME_END` | the end latch: the match is over |

and each bot's brain is wrapped (`sim_glue::Metered`) to count, per game
tick, what only the brain knows: ticks stuck (the unstick escalation
above `Moving`), repaths, and ticks per activity (`Doing`: fight, chase,
roam, support, supply, rearm, recover, carry, keep, play).

`sim::metrics::Tally` aggregates all of it (pure, tested): the kill log
classified as kill, suicide, betrayal or world; per player shots, hits on
players, damage dealt and taken, ticks alive, ticks in a vehicle,
grenades thrown (the carried count falling within a life), objective
ticks; and the final score. The report is one JSON file per match:

```json
{
  "format": "h2-sim-report/1",
  "match": { "map": "lockout", "game_type": "slayer", "seed": 7, "...": "..." },
  "result": { "end": "game_over", "winner": {"player": 2}, "teams": [] },
  "ticks": 9000, "game_seconds": 300.0,
  "wall": { "setup_s": 21.8, "match_s": 41.2, "process_s": 63.0, "speedup": 7.28,
            "process_speedup": 4.76, "ms_per_tick": 4.58, "bots_ms_per_tick": 0.45,
            "phantom_build_ms_per_tick": 0.9, "phantom_snapshot_ms_per_tick": 0.5,
            "phantom_snapshots_per_tick": 1.0, "phantom_snapshot_copy": "remap",
            "peak_rss_mib": 981, "footprint_mib": 143,
            "guest_written_pages": 2100, "guest_pages": 16384 },
  "guest_census": { "ticks": 9000,
                    "local_users": { "samples": 9000, "mean": 0.0, "peak": 0 },
                    "local_visibility": { "samples": 9000, "empty_ticks": 9000 },
                    "arrays": { "effect": { "samples": 9000, "mean": 6.1, "peak": 31 },
                                "particle_system": { "...": "..." }, "particles": { "...": "..." },
                                "decals": { "...": "..." }, "lights": { "...": "..." } } },
  "players": [
    { "index": 0, "name": "A1", "bot": true, "seat": 0, "arm": "base", "team": 0,
      "score": 12, "kills": 12, "deaths": 9, "assists": 2, "suicides": 0, "betrayals": 0,
      "shots": 820, "hits": 301, "accuracy": 0.367, "damage_dealt": 41.5, "damage_taken": 37.0,
      "alive_ticks": 8100, "vehicle_ticks": 0, "grenades": 14, "stuck_ticks": 36, "repaths": 2,
      "doing": { "fight": 2100, "roam": 3900 } }
  ],
  "kills": [ { "tick": 412, "killer": 0, "victim": 3, "kind": "kill" } ],
  "flat": { "fixups_per_tick": 0.0 },
  "nav": "loaded",
  "notes": []
}
```

`wall.bots_ms_per_tick` is the game-thread time the bot brains cost per
tick: every phantom's read, think and conclude phases, and the wait for
workers, in any think mode (`bots::think::game_thread_ns`). A think that
runs on a worker is not in it, so batched against serial shows the
game-thread saving. `flat.fixups_per_tick` is the flat RAM window's
fixups over the match, per tick (`runtime::window_process_stats`); the
report has no `flat` without a window. Both count from the tick the
match begins.

The phantoms' action build is not in `bots_ms_per_tick`
([sim-headless-skips.md](../perf/sim-headless-skips.md)). At the remote
gather each phantom's pad goes through retail's local action builder in
a private world (`bot_glue::retail_input`, `Builder::sample`): by default
on live RAM itself, every store journaled and undone before the gather
returns (`impl.sim.phantom-alias`); with `-impl.sim.phantom-alias`, or on
a flat-RAM image, in a copy-on-write copy of guest RAM
([sim-phantom-build.md](../perf/sim-phantom-build.md)).
`wall.phantom_build_ms_per_tick` is the game-thread time of those builds
per tick: every seat's setup writes, six guest calls and read back, with
the undo on the alias path, or the snapshot and the reverts between seats
on the view path. A film that replays its builds runs none.
`wall.phantom_snapshot_ms_per_tick` is the part spent in
`SecondExecutor::snapshot`, the two copy-on-write copies of guest RAM a
view-path build takes at a tick's first seat. `wall.phantom_snapshots_per_tick`
counts those snapshots; it is above 1 when a failed build or a seat reset
starts a seat from live RAM again, and the alias path takes none.
`wall.phantom_snapshot_copy` says how they copied RAM (`runtime::CopyKind`):
`remap` (a copy-on-write mapping of the same pages, macOS), `copy` (every
byte), or `mixed`. `wall.phantom_build_mismatches` is there only in check
mode (`H2_PHANTOM_BUILD_CHECK=1`, which builds every seat on both paths):
the seats whose outcome, action bytes or history differed between them,
0 when they all agree. All of these count from the tick the match begins,
as the costs above do, and a report has none of them when the run did not
measure.

The split of that build, per tick and by the same counters
(`bot_glue::retail_input`, `sim_bench::metrics::PHANTOM_CALLS`), is in
the same block:

| Field | What |
|---|---|
| `phantom_setup_ns_per_tick` | nanoseconds in the setup writes: the view's globals, the pad and the seat's history |
| `phantom_call_ns_per_tick` | nanoseconds in each guest call, keyed by role: `bind` (`0x1872d0`), `look_rates` (`0x190c34`), `camera` (`0x1557c0`), `input` (`0x124c00`), `remap` (`0x217c40`), `builder` (`0x186ab0`) |
| `phantom_call_accesses_per_tick` | guest accesses of each call (`CallOutcome::accesses`), keyed the same way |
| `phantom_revert_ns_per_tick` | nanoseconds in `written` plus `revert`, the reset between seats of a tick |
| `phantom_read_ns_per_tick` | nanoseconds reading the action and history back |
| `phantom_pages_written_per_seat` | pages a seat wrote, a mean over the seats that were reverted (a tick's last seat is not); absent when none were |
| `phantom_minor_faults_per_tick` | minor page faults inside `Builder::sample`, the process's `getrusage` `ru_minflt` delta, so faults on other threads in that window count too |

The `guest_census` block (`crates/extensions/game/src/sim_census.rs`)
counts the presentation-side state a headless match still holds. The
tick observer reads it once a tick, from the tick the match begins:

| Field | What | Read from |
|---|---|---|
| `ticks` | ticks sampled | |
| `local_users` | the local-user count | `i16` at `[0x4e8c20] + 8` (`guest_layout::PlayerGlobals`) |
| `local_visibility` | ticks the local players' visibility set was empty | the `0x40` bytes at `[0x4e6948] + 0x1178` (cleared at `0x137d5c`, built by the visibility pass at `0x138c5f`) |
| `arrays.effect` | live effects | data array `[0x4ea93c]` (initialised at `0x175b93`) |
| `arrays.particle_system` | live particle systems | `[0x510c74]` (`0x173e29`) |
| `arrays.particles` | live particles | `[0x51ec84]` (`0x173e62`) |
| `arrays.decals` | live decals | `[0x4ea950]` (`0x17d263`) |
| `arrays.lights` | live lights | `[0x4e030c]` (`0xbfffb`) |

A count carries `samples` (the ticks it was read on), `mean` and `peak`;
an array counts from its header's live count (`+0x3c`), and only when
the header has the data-array signature and the name the title gives it,
so a wrong pointer is left out, never counted. An array never read is
absent. The reads are three pointers, five headers and 64 bytes a tick.
A report has no `guest_census` before its first counted tick.

The `tick_phases` block (`sim_bench::metrics::TickPhases`) is the tick
driver's mean ms per tick over the match: `tick_ms_per_tick`,
`loop_ms_per_tick` (tick entry to the next entry, sound included) and
`phases`, the 29 phases in retail order as `{name, ms_per_tick}`
(docs/engine/sim-rewrite.md, "The tick driver").

With `H2_DISPATCH_STATS=1`, the report includes `dispatch_per_tick`
for the observed match interval. It records the CALLs, JUMPs, RESUMEs,
RETURNs, STOPs and FAULTs that native generated handlers hand back to the
runner. HLE entries and slice or host-request preemptions are counted
alongside those exits. A loop resume can also cause a preemption, so
these counts can overlap. `H2_ENTRY_STATS=1` adds a separate, more
expensive census of hot runner targets on stderr, split by why a call or
jump reached the runner (entry table live, null, or no entry).

At `WEAPON_FIRE`, a handheld shot is credited to the player who owns
the parent biped. The next tick's held-weapon snapshot provides a
fallback. For mounted fire, `bots::vehicle::read::firing_player` looks
up the player in the firing seat at the time of the shot. For example,
a Ghost's plasma fire counts towards its driver's shots and a
Warthog's chaingun fire counts towards its gunner's shots.

A hit is a damage report against another player's unit. Damage sums
the report's body and shield amounts. Each shotgun pellet can count
as a hit, and melee and splatter can land hits without firing a shot,
so accuracy can exceed 1.

Each phantom's `weapon_accuracy` rows count shots the multiplayer weapon
stats register and shots the game credits with a hostile hit. The reporting
code identifies the fired weapon. The decision range to the bot's visible
target at fire is close (under 3 world units), mid (3 to under 12), long
(12 and above) or unknown. A delayed projectile keeps its firing range.
Pellets and splash from one registered shot can earn one hit.

`crates/extensions/game/src/bot_accuracy.rs` observes `WEAPON_STAT_BUCKET`.
The shot registration at `0x15cca0` stores a 16-bit shot ID, reporting code
and tick in one of eight entries per shooter. It looks up the weapon bucket
at `0x15cd65` before incrementing weapon statistic 4. The hit path at
`0x15cd90` checks hostility and matches a pending shot ID before its lookup
at `0x15ce40`, increments statistic 5 and invalidates the entry. The rows
follow these same slots across weapon switches and report hits divided by
registered shots. The separate `combat` object holds the decisions and timing
available during play through `h2 agent bots.stats`.

Each bot row includes its logical `seat`. Readers can continue to load
earlier `h2-sim-report/1` reports.

The `wall` block carries the process's memory: `peak_rss_mib`
(`getrusage`'s `ru_maxrss`), `footprint_mib` on macOS
(`proc_pid_rusage`'s `ri_phys_footprint`, dirty pages, the number a
fleet pays for), and, when the run loaded a shared copy-on-write guest
RAM base (§6.2), `guest_written_pages` of `guest_pages`: the census of
what the match wrote over the base, so batches record memory per run.
`mapped_read_bytes` and `copied_read_bytes` count completed bytes transferred
into guest RAM by NtReadFile since boot. Copied bytes include partial pages.
The per-process memory of a Waterworks 2v2 sim run is in
[notes/sim.md](notes/sim.md).

The census and lean-report fields
(`sim_bench::metrics::WallLean`, flattened into `wall`) say where a run's
setup time, reads, memory and disk went. Each is left out when it was not
measured, and an earlier report without them still loads.

| Field | What |
|---|---|
| `setup` | seconds in each stage before the first counted tick: `boot_s` (process start to the first game tick at the main menu), `menu_s` (to the lobby being able to open: networking and the lobby subsystem), `lobby_s` (opening and filling the System Link squad), `copy_wait_s` (the first wait on a peer's map status, `peer N has map status ...`, to the start request being accepted), `load_s` (the start request to the game activating) and `bind_s` (game active to every phantom having a player and a unit). The six add up to `setup_s`. A stage a run skips reads 0; `copy_wait_s` includes any wait for players that follows the first map-status wait |
| `reads` | per guest path, `mapped_bytes`, `copied_bytes`, `count` (mapped reads, plus one per backend read on the copy path) and `refused`, the first reason a mapped read of that path was refused (`Vfs::try_map_read`: `short read`, `writable handle`, `no immutable file region`, ...). The map is kept by `vfs::reads::ReadLog`, keyed by path, at most 4096 paths |
| `heap_tags` | live Rust heap bytes by allocation tag at the end (`mem_watch::heap::totals`); only tags with live bytes |
| `guest_pages_written_4k`, `guest_pages_written_16k` | guest RAM pages whose bytes differ between the tick after counting begins and the end, counted per 4 KiB page and per aligned 16 KiB group of four (the pages a 16 KiB-page host would own). Both are hashes of the whole 64 MiB (`sim_pages::hashes`), taken under `impl.sim.page-census` (on; `-impl.sim.page-census` leaves the fields out, for a timing arm). A page written back to its old bytes is not counted, and a tick jump takes the baseline again |
| `data_dir_bytes` | the bytes of the regular files under the run's data directory at the end (symbolic links not followed) |

The stage boundaries are the sim's own lobby observations
(`sim_glue::hosted`) and the game's activation; `sim_bench::metrics::SetupClock`
only moves forward, charging the time since the last boundary to the stage
it leaves.

### The spatial trace

`detail = "spatial"` in a match or batch file records where things
happened, for Studio's playtest ([playtest.md](../editor/playtest.md)).
The report is a training one (`Detail::tally`), and beside it
(`sim_bench::spatial::trace_path`: `report.json` gives `spatial.bin`,
`<name>.report.json` gives `<name>.spatial.bin`) the run writes the trace.
The same observers that feed the tally feed a
`sim_bench::spatial::Recorder` (`sim_glue::spatial`), on the game thread
and read-only:

| Event | When | What |
|---|---|---|
| `sample` | every 6th tick from the match's start (five a second), each living player | position, nav polygon, yaw and pitch of the aim |
| `spawn` | a player's unit alive after a tick in which it was not | the place; the scenario starting location within 2 units; the nearest living enemy and its distance; how many living enemies have a clear line to it, eye to eye (the bots' walls; absent without them) |
| `damage` | `DAMAGE_REPORT` against a player's unit | both players' places at the last tick and the weapons they held, the amount |
| `kill` | `PLAYER_KILLED` | the same, classified as the tally classifies it |
| `pickup` | a weapon datum new to a living player's slots, or more grenades | the place, the weapon (its definition when it is the one now held), the netgame equipment placement within 2.5 units |
| `stuck` | a bot's stuck spell ending (the unstick escalation back at `Moving`, a death or the match's end) | where it began, its length in ticks, where the bot was going (`bots::stuck::Stuck::goal`) |
| `repath` | a bot's repath count rising | the place and where it was going |

A place is a position and the polygon of the bots' nav mesh under it
(`NavMesh::poly_near`); without a loaded mesh it has no polygon. The
header names the map, game type, seed and team play, the counted ticks,
the scenario's player starting locations (position, team) and netgame
equipment (position, respawn time), read from guest memory at the game's
start (`sim_glue::spatial::places`), and the nav mesh's polygon centres
and links. A tick jump restarts the trace as it restarts the tally.

The file is `H2SP`, a format version (1), the header as JSON, the mesh,
then the events, little-endian (`Trace::to_bytes`). At the end of the run
the trace's kill log is checked against the report's
(`Trace::agrees_with`) and the result is a note in the report and on
stderr: `spatial trace <path> (N events; its K kills are the report's)`,
or the first difference. A run with no report path writes no trace.

`h2 sim spatial <spatial.bin | run dir | results dir>...` prints the
measures of the traces named, together (a results directory is every
`runs/*/spatial.bin`): `sim_bench::spatial::measure`, each measure a pure
function over the traces in `sim-bench/src/spatial/`. A place off the
mesh is binned on a 2-unit XY grid.

| Measure | From | Refused under |
|---|---|---|
| `heat` | kills (where killers stood) and deaths (where victims fell), per bin, by team and by the killer's weapon | 10 deaths |
| `spawns` | per starting location: uses, mean life, deaths within 3 s, enemies in sight, nearest enemy, time to first engagement; the spawn kills' places; each team's seconds to its first engagement | 8 spawns |
| `flow` | player-seconds per bin, crossings from bin to bin with direction, the mesh's polygons no sample was on | 300 samples |
| `sight` | damage and kill segments: the ten longest, the ten bin pairs most fought across, the median, a histogram per weapon in 5-unit buckets | 10 segments |
| `items` | per equipment placement: pickups, pickups a minute, time to first pickup, kills with a weapon taken from it, never picked | no placement, or 3 pickups |
| `stuck` | stuck spells and repaths per bin, and the mesh's islands no starting location reaches along its links | 300 samples |

A refused measure prints what it lacked, for example
`heat: too few deaths (6, at least 10 needed)`.

### 6.1 Sim stores: the map without the map

The retail map is never streamed in a sim or light host that uses
stores. A recording pass notes every byte range the guest reads through
the VFS; the build packs those ranges per map into one small file; the
run serves the map from it, byte for byte at the same offsets, and a
read of a byte no recording covered is a named error (`NotInStore`,
with the range on stderr); never a silent fall back to the disc, so a
miss is found, recorded, and the store rebuilt:

```sh
# 1. Record (runs merge, so play several seeds for coverage):
H2_SIM_STORE_RECORD=reads.txt h2 sim run match.toml
# 2. Build (verified byte for byte against the disc):
h2 sim store build waterworks --disc disc/halo2.iso --out store --reads reads.txt
#    ...or every map the recording holds:
h2 sim store build all --disc disc/halo2.iso --out store --reads reads.txt
# 3. Serve (default off until measured):
H2_SIM_STORE=store h2 sim run match.toml   # or h2 serve --system-link --host-light
```

The store file (`<map>.h2store`) contains a 24-byte header, a sorted
range index and the recorded bytes. Version 1 of the index pads bulk
ranges to the same offset within a 4 KiB page as their logical map
offset. The reader also accepts packed version 0 indexes.
`vfs::SimStoreBackend` maps the file read-only. The configured editor
and server map layers take precedence. The guest sees the original
logical file length, and an unreadable store fails its reads.

On a light host, a read from an immutable disc map, verified server map
or contiguous store range can supply whole pages directly to guest RAM.
On macOS, boot selects a dedicated paged RAM mapping when this read path
is enabled. Ordinary play and the light profile's copy control use the
usual heap backing. Flat RAM keeps its separate aliasable mapping.
The kernel attempts one mapping for the full NtReadFile request, before
dividing a copied read into 64 KiB chunks. At least 16 whole host pages
must fit. The physical destination and file offset must have the same
remainder modulo the host page size. Partial edge pages are copied,
and guest writes to a mapped page are private to that process. Flat
RAM aliases, access traps, HLE write capture, mutable files and composed
map streams take the ordinary copy path. The host reports the first
refusal for each reason on stderr. See [the switch registry](switches.md)
for the copy control.

Two games on different maps still share `shared.map`'s store, the
common tag data both read, and the dylib's code pages.

Boot still opens the disc image for the XBE and small header reads;
with a store covering every map the sim reads, the image's resident
pages stop mattering. Without `H2_SIM_STORE`, the light profile maps
the disc read-only and drops a bulk read's pages again after the copy
out (`vfs::disc`), so a map copied to the title's cache partition
leaves no pages behind either.

### 6.2 Sharing guest RAM with machine snapshots

The snapshot shares guest RAM at a saved machine state. `state.save` with `"cow": true` writes, beside
the snapshot, a sparse raw RAM image (`<name>.ram`: zero pages are
holes); `state.load` with `"cow": true` maps that file private
copy-on-write in place of decoding RAM in, pages the match never
writes stay one shared set across every process on the same image
(including the zero pages), and only written pages become private.
The [agent snapshot methods](snapshots.md) support this mode. The
match-end census in the report's `wall` block (`guest_written_pages`)
counts pages written. `vmmap` (macOS) or `/proc/<pid>/smaps` (Linux)
shows the private and shared memory per process.

Sim batches boot a fresh System Link host for each run. Machine
snapshots omit the phantom peers' host state and can't seed these
batches.

`ttk_kills` and `ttk_ticks` are the time to kill. A fight is a player's
hits with damage on one opponent, none more than 150 ticks after the
last. When the player kills that opponent, the ticks from the fight's
first hit to the kill are one sample. `ttk_ticks` is the mean over
`ttk_kills`. A kill in one hit counts as 0 ticks. A kill with no damage
report from the killer on the victim (a fall the killer caused) has no
sample.

A bot that tracked a target has an `aim` section, from the hand's view
of its target each tick (`bots::mind::AimTrace`: the angle from the aim
to the aim point, before the hand's own error) and the right stick its
pad reported after the human-input limits (`sim::metrics::AimMotion`,
tested):

```json
"aim": { "tracked_ticks": 2100, "mean_error": 0.041, "flicks": 18,
         "overshoot_ratio": 0.09, "settled": 15, "settle_ticks": 14.2,
         "stick_jitter": 0.08, "reversals_per_s": 1.1 }
```

- An engagement is a run of views of one target no more than 3 ticks
  apart. One that starts 0.35 rad (20°) or more off is a flick.
- `overshoot_ratio`: per flick, the furthest the aim went past the
  target in its first 60 ticks, over the flick's size; the mean over
  flicks.
- `settle_ticks`: per flick that settled, the ticks from its first view
  to the first of six in a row within 0.05 rad; the mean over `settled`.
- `stick_jitter`: the mean right-stick travel between consecutive views
  of an engagement, in stick radii.
- `reversals_per_s`: how often the stick's yaw direction changes within
  an engagement (a stick under 0.05 has no direction).
- `mean_error` is in radians, over `tracked_ticks`.

All of these are additive: a reader of `h2-sim-report/1` that does not
know them is unaffected, and reports without them still load.

Sim runs are local by default. Set `H2_TELEMETRY_FILE` for a separate
JSONL capture, or use `--upload` for an Uplink recording.
