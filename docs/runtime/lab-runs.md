# The lab: driving runs, hosts, benchmarks and recordings

Status: Built. The agent API on a run, own-data play, the dedicated System Link host, sim benchmarks, Theater runs and recordings. Part of [The lab](lab.md).

## Driving a run: the agent API

Every run's game opens its agent API socket
([agent-api.md](agent-api.md)) at `<root>/a/<id>.sock`: the host sets
`H2_AGENT` itself (no `--env` can), and records the path in the job's
`meta.json` as `agent_socket`. It is not under `runs/<id>/` because a Unix
socket path is limited to about 100 bytes (a deep root falls back to
`/tmp/h2lab-<hash>-<id>.sock`) and because `fetch` tars that directory. The
host removes the socket when the run ends.

```
h2 lab agent --host laptop <id> <method|short form> [json-params] [--timeout S]
h2 lab agent --host laptop 0007 wait '{"phase":"gameplay","map_ticks":30}' 600000 --timeout 700
h2 lab agent --host laptop 0007 capture 600 --timeout 200
h2 lab agent --host laptop 0007 session.status
h2 lab agent --host laptop 0007 trace.list
h2 lab agent --host laptop 0007 flight show crouch-bug.jsonl --field physics.crouch
h2 lab agent --host laptop 0007 input.log.read '{"name":"fight","limit":4096}'
h2 lab agent --host laptop 0007 game.menu                # leave the level for the main menu
h2 lab agent --host laptop 0007 - < script.txt        # an `h2 agent -` script
```

Everything after the id is a call for `h2 agent`. The host runs the job's
own `h2` as `h2 agent <call> --socket <the run's socket> --timeout S`.
Stdout, stderr and the exit code come back to the driver, so scripts and
`expect` lines behave as with a local `h2 agent`. `--timeout S` (default 300,
at most 7200) is how long the call may take, and is the call's socket
timeout; give a `capture` or a `wait` more than it needs. The host waits up
to two minutes for a just-started game's socket to appear. The call is
refused while the job is still building and once the run has ended.

`flight show` takes a bare file name and always reads through the job's
socket. Use `trace.list` to find file names and `input.list` to find
recording names. A lab agent call can return up to 64 MiB of output.
For larger files, fetch them or call
[`trace.read`](agent-api.md#tick-traces-and-the-flight-recorder) or
[`input.log.read`](agent-api.md#input-act-like-a-player) one page at a time.
The game and lab host must both support artifact readback. The host
reserves `--socket` and `--xbox` for routing. Use `raw-diff` locally on
fetched files.

### Playing on your own data

A lab run plays on the host's `data/`, so its profiles, saves,
`settings.toml` and mappack are the lab's, not the player's. The Live sign-in
(`accounts.json`, `play-accounts.json`) is read from `$H2_LIVE_DIR`, else
`<app data>/live`, and the cloud saves of a signed-in account are merged into
the data root: a run on `data/` signs in but merges into and plays from the
lab's root. Every `H2_*` variable of the host's own environment is also
dropped from the game, so `H2_DATA`, `H2_LIVE_DIR`, `H2_LIVE_SERVER` and
`H2_CLOUD_SAVES` do not reach it.

`--own-data` plays as `h2 play` on that Mac would: the host passes no
`--data` (the game takes `content::store::default_data_root`, which honours
`H2_DATA`), so the profiles, settings and Live accounts are the player's, and it
keeps the host's `H2_DATA`, `H2_LIVE_DIR`, `H2_LIVE_SERVER`, `H2_LIVE_DB`,
`H2_LIVE_WEB` and `H2_CLOUD_SAVES` (`proto::OWN_DATA_ENV`) when the host
process has them. A LaunchAgent host has the agent's environment, not a
shell's, so a variable set in the player's shell needs setting for the
LaunchAgent too. Every other `H2_*` is still dropped.

```
h2 lab run --host laptop --name "Play" --current --level 03a_oldmombasa --serve --own-data
```

The consent prompt names the data ("own data"), and the job lines of `h2 lab logs` and
`h2 lab host watch` show `data=own`; `request.own_data` is in the job's `meta.json`.
`--film` records into that root's `films/`. Results are not repeatable, so
use `--fresh-data` for runs compared frame for frame. An older host refuses
the flag as an unknown field: `h2 lab update --host laptop` first.

### `--serve` and `bench`

`h2 lab run --name "Live inspection" --serve` is a run meant to be driven: no `--frames`, so it
stays up until `h2 lab stop <id>`, and the agent decides when to measure.
`--wait` is refused with it.

**It launches `h2 play`, not `h2 serve`.** `h2 serve` is headless: no
window, and the boot has no view backend, so it measures neither the GPU
renderer's frame cost nor the `render` and `gpu-wait` phases a player sees
(`H2_BOOT_GFX` there writes PNGs from a headless backend; it is not a
window). `h2 play` opens the window and the native GPU renderer, and a game
with `H2_AGENT` set serves the same API ([agent-api.md](agent-api.md)), so
`--serve` gets the API on a real window and GPU. The Aqua LaunchAgent
runs the host in the logged-in user's graphical session.

A `--serve` run of a level can go back to the main menu as a player
would, with [`game.menu`](agent-api.md#returning-to-the-main-menu), to
measure the menu after the level:

```
h2 lab run --host laptop --name "Menu after Ascension" --level ascension --serve --env H2_RENDER_SCALE=4
h2 lab agent --host laptop <id> wait '{"phase":"gameplay","map_ticks":3600}' 300000 --timeout 400
h2 lab agent --host laptop <id> game.menu
h2 lab agent --host laptop <id> wait '{"map":"mainmenu","map_ticks":900}' 300000 --timeout 400
h2 lab stop --host laptop <id>
```

`h2 lab bench` is the loop for one measurement:

```
h2 lab bench --name "City benchmark" --host laptop --branch perf-work --level 03a_oldmombasa --scale 4 \
    --ticks 600 --env H2_FEATURES=... [--timeout 600] [--to DIR] [-- --unpaced --no-vsync]
```

It builds (or reuses) the commit like `run`, starts a `--serve` run with
`H2_RENDER_SCALE=<scale>` and the `--env`s, waits for the run to be running
and then `h2 agent wait '{"phase":"gameplay","map_ticks":30}'` (`--timeout`
seconds, default 600, for the level to load), runs `capture <ticks>`
(default 600; 1 to 36000), `render` (`render.stats`) and `counters`
(`profile.counters`), stops the run, fetches `runs/<id>` and writes
`capture.json`, `render-stats.json` and `profile-counters.json` into it,
then prints:

```
bench 03a_oldmombasa H2_RENDER_SCALE=4: 29.9 fps, busy p50 12.31 p95 20.10 ms over 600 frames (4 over 33.3 ms); phase mean ms: render 5.10, cpu 3.20, ...
<the fetched directory>
```

`busy` is frame wall time less pacing, so it is the cost of a frame whether
or not the window is paced to 30 fps; the `fps` is the wall rate, which a
paced run pins near 30. Add `-- --unpaced --no-vsync` to let the frame rate
float. A failure (the job ends first, no gameplay in time, a call exits
non-zero) stops the run and says so; its logs stay in `runs/<id>` for
`h2 lab fetch`. For an A/B, run the two invocations sequentially on a quiet
host, varying `--env`, `--branch` or `--build-env`. A bench starts the level
at its beginning, and two runs of one level drift apart over minutes
(agent-api.md), so keep the window short and compare percentiles, not frames.

Report both render resolution and LOD Distance with results above 480 lines.
At 1x LOD Distance, higher resolutions also retain more detailed geometry
in single-copy runs. Highest requests the finest model tier. Film and
input-tape runs use a fixed geometry multiplier for the guest reference. [Readback and LOD policy](../graphics/high-resolution.md#determinism).
For a comparison against timings taken with 480-line LOD choices, pass
`--env H2_LOD_DISTANCE=retail` at each scale in an ordinary offline run.
To measure the cost of retaining higher detail, keep the resolution fixed
in two unrecorded offline runs and compare
`--env H2_LOD_DISTANCE=retail` with `--env H2_LOD_DISTANCE=1`.

## Dedicated System Link host

`h2 lab run --dedicated` starts a dedicated System Link host on the lab
Mac. A player on another Mac on the same LAN joins it from the System
Link menu, and an agent drives it with `h2 lab agent` as it drives a
`--serve` run. Code: `crates/apps/cli/src/lab/host/dedicated.rs` and the
`Dedicated` request in `crates/apps/cli/src/lab/proto.rs`.

```sh
h2 lab run --host laptop --name "Join test" --dedicated --map lockout --variant slayer \
    --max-players 8 [--host-bots] [--base-port PORT] [--branch NAME | --rev REV | --current]
```

The job builds or reuses the image like any run, then starts
`h2 serve <disc> --system-link --host-light [--host-bots]` on it with
`H2_HOST_LOBBY=1` ([Playerless lobby](../live/dedicated-host-ops.md#playerless-lobby))
and `H2_BOOT_REALTIME=1`. The host gets a data root of its own under
`scratch/<id>/data`, removed when the job ends, and skips the Halo 1 and
map pack imports. Its agent socket is the run socket at `<root>/a/<id>.sock`.
The job has no frame limit and runs until `h2 lab stop <id>`, which ends
it as `stopped`. `--wait` is refused, as are the game window's options
(`--level`, `--frames`, `--replay`, `--record`, `--film`, `--perf`,
`--serve`, `--fresh-data` and `h2 play` flags). `--env` takes the run
allowlist.

| Flag | Becomes |
|---|---|
| `--dedicated` | the host, sitting at the main menu with no local player |
| `--map MAP` | once the host reaches the menu, the job calls `host.lobby.open {"map","variant","max_players","live":false}` and opens a joinable lobby |
| `--variant NAME` | the lobby's game variant (default Slayer); needs `--map` |
| `--max-players N` | the lobby's limit, 1 to 16 (default 8); needs `--map` |
| `--host-bots` | `h2 serve --host-bots`: the host accepts phantoms |
| `--base-port PORT` | the XNet base, 1024 to 65529 (default 3174). A base whose seven ports reach 3074 to 3080 is refused |

XNet binds the IPv4 of the lab Mac's default route, as the XNet HLE picks
its own, at the base port. The host uses UDP base through base+6, and the
job refuses to start when another process holds one of them. The default
base, 3174, keeps the job off the ports a game the player opens on the lab
Mac binds (an unconfigured `h2 play`'s 3074 to 3080), so the two never
collide. `LAB_BASE_PORT` and `takes_player_ports` in
`crates/apps/cli/src/lab/proto.rs` hold the rule. The job's
`system_link` field in `h2 lab status`, `h2 lab runs` and `meta.json`
gives the address and port, and its notes say how to join. The job notes
whether the lobby opened. A failed open leaves the host at the menu. Open it
with `h2 lab agent <id> host.lobby.open '{...}'`.

To join, start `h2 play` on another Mac on the same LAN from the same
commit with `H2_XNET_ADDR=<its own LAN IPv4>:3174` (or the job's
`--base-port`), and choose System Link, then the host's game. A System
Link broadcast goes to the sender's own base, so both ends need the same
base, and an unconfigured `h2 play` uses 3074. If the default route is a
VPN, the host binds the VPN's address and a LAN joiner can't see it.
The macOS firewall may ask the lab Mac's user once to let the image's
`h2` accept incoming connections.

Drive the lobby through the agent API
([`host.lobby`](agent-api.md#hostlobby)). Add a bot to a `--host-bots`
host with [`sim.player.add`](agent-api.md#sim-simulated-players-and-direct-state):

```sh
h2 lab agent --host laptop <id> host.lobby.state '{}'
h2 lab agent --host laptop <id> sim.player.add \
    '{"kind":"phantom","identity":{"xuid":"0009000000000001","gamertag":"Clanker"},"source":"brain"}'
h2 lab agent --host laptop <id> host.lobby.start '{}'
h2 lab stop --host laptop <id>
```

The job takes the activity lock, queue place, [player
consent](lab-consent.md#player-consent-before-a-job), [the player's
choice](lab-consent.md#when-the-player-opens-a-game) and [memory
cap](lab-consent.md#memory-limits-and-reports) like a run. A game window on the lab Mac
can't join it: at the job's base both would bind the same UDP ports. A
game the player opens there with no `H2_XNET_ADDR` stays on its own ports
and doesn't collide with the job.

## Sim benchmarks

`h2 lab sim` runs [`h2 sim`](sim.md) on the lab host and keeps its
reports, so a headless bot match's speed is measured on the quiet second
Mac rather than on the driving Mac. It does not start a game or a window.
Code: `crates/apps/cli/src/lab/simbench.rs` (arguments, files, run order,
summary), `run_sim` and `sim_command` in `crates/apps/cli/src/lab/host.rs`,
and the `Sim` request in `crates/apps/cli/src/lab/proto.rs`.

```sh
h2 lab sim --host laptop --branch perf-work --wait --repeat 3 \
    --arm on=impl.sim.native --arm off=-impl.sim.native \
    --build-env H2_OPT=2 docs/runtime/sim-examples/lockout-2v2.toml
```

| Flag | Becomes |
|---|---|
| `<file>` | a match file (`h2 sim run`) or a batch file (`h2 sim batch`; a file with a `match` key is a batch) |
| `--repeat N` | runs of each arm, 1 to 100 (default 1) |
| `--arm NAME=VALUE` | one arm: its runs get `H2_FEATURES=VALUE` (`--arm base=` leaves the variable as it is). Up to 8 arms; with none, one arm named `default`. `NAME` is a plain path component |
| `--parallel N` | matches at once in a batch (default 1, since these are benchmarks; refused for a match file) |
| `--env KEY=VALUE` | run environment from the same allowlist as `lab run`, set before the arm's `H2_FEATURES` |
| `--branch`, `--rev`, `--current`, `--no-sync`, `--feature`, `--build-env` | the build, as for [a run](lab.md#builds-go-by-branch). `H2_OPT` must be the build's, and is, because the run uses the image it was built with |
| `--name TEXT`, `--note TEXT`, `--mem-limit SIZE` | label (default `sim <file stem>`), note and footprint cap, as for a run |
| `--data DIR` | the driving Mac's data root, whose `bots/` is uploaded (default: `h2`'s data root, `H2_DATA`) |
| `--wait [--to DIR]` | wait, fetch the run, print the summary |

The client sends the file and every file it names (a batch's match file
and arm tunings, each bot's `params`) in the request, at most 64 files and
2 MiB, under their paths relative to their common directory so the
references keep resolving. A batch's `out` must be a relative path without
`..`: its results land in the job's directory. The host checks the same
before it runs. A missing tuning file, a bad `--repeat` or an unknown
variable is refused on the driving Mac before anything syncs.

The bots' data goes with the files. For every map the file names (a
match's `map`, a batch's `maps`, else its match's `map`) the client
uploads `bots/<map>.<bsp>.bots` and `.vis` from the data root, up to 64
files and 60 MiB (`bots::cache::file_name`, `bots::vis::file_name`,
`simbench::bot_data`). A map whose `<map>.0.bots` is missing is refused
on the driving Mac, naming `h2 bots nav <map>`: a sim never builds a mesh,
and bots without one stand still, so the benchmark would measure an empty
match. The host keeps the uploaded keys to `bots/<plain name>.bots|.vis`
(`proto::check_sim_data_key`).

The job takes the host's activity lock, queue place, [player
consent](lab-consent.md#player-consent-before-a-job) and [memory
cap](lab-consent.md#memory-limits-and-reports) like a run (the cap applies to the `h2
sim` process; a batch's matches are its children, so with `--parallel` the
machine memory-pressure stop is the bound that covers them). It builds or
reuses the commit's image first. `h2 sim` never builds, so the sim runs on
that image: `--out` is its variant's `variants/<key>/` (or the `--current` image).
`--data` is the job's own data root, `runs/<id>/sim-data/` (`simbench::stage_data`):
every entry of the host's data root linked in except `bots/`,
and the driver's uploaded meshes and grids as its `bots/`. It is removed
when the job ends. A request without bot data (an older client) runs on
the host's own data root, whose `bots/` must then hold each map's nav mesh.

### Run order and files

Each arm runs `--repeat` times, one `h2 sim` at a time. A repeat runs every
arm once, and the arm order reverses on odd repeats (`on off | off on |
on off`), so drift in the machine lands on every arm and a linear drift
within a repeat cancels (`simbench::schedule`). Each run is a step in
`runs/<id>/sim/<arm>-r<N>/`:

| Path | What |
|---|---|
| `in/` | the uploaded files, in their layout (a batch's `out` is under it) |
| `report.json` | a match's report; a batch's are `in/<out>/runs/<id>/report.json` |
| `sim.log` | the sim's stdout and stderr |

The per-run data directories (`<out>/runs/<id>/data`: N:'s map copies of
their own, Z:'s cloned from the image's
[template](cache-slots.md#sim-batches-a-warm-z), which stays) are removed
when the step ends, including after a crash or `h2 lab stop`;
reports and logs stay. A step that exits non-zero, leaves no report or
leaves an unreadable one is a failed run: the job finishes `failed` after
the remaining steps, with the failures in its notes and the summary. A step
whose report has `nav` other than `loaded`, or whose log has the line "nav
mesh none yet", is failed too, with the reason "nav mesh missing"
(`simbench::nav_problem`): its bots stood still and its speed is not a bot
match's.

### The summary

When the steps end (or the job is stopped), the host writes
`runs/<id>/sim-summary.txt` and `sim-summary.json`, appends the text to
`run.log`, and keeps its lines as the job's `summary` (`h2 lab runs`,
`h2 lab wait`). `--wait` prints it:

```
sim benchmark sim lockout-2v2: 2 arm(s) x 3 repeat(s), 6 runs, 0 failed
arm                n   speedup      min      max    ms/tick       min       max
on                 3     4.100    4.000    4.200      8.000     7.500     8.100
off                3     3.100    3.000    3.200     10.000     9.500    10.200
off vs on: median ms/tick +25.0%, median speedup -24.4%
state hashes: identical across all runs and arms (5 report pair(s) compared)
```

Per arm, median, minimum and maximum of the reports' `wall.speedup` and
`wall.ms_per_tick`, one value per report (a batch contributes one per
match). The state-hash line compares every report with the first finished
run's, by the same path within the step, with `sim_bench::diff` (the
component series when both reports have it, else the `state_hashes_v1`
digests, as in `h2 sim diff`). It says `identical` only when every run of
every arm finished and agrees. Otherwise it names the earliest differing
tick and the two runs (`DIFFER, first at tick N (off-r2 against on-r1,
report.json)`), or says `unverifiable` when a report has no digests or a
match is missing from a run. A difference is a result, not a failure: the
job is `done`. For the component-level difference, fetch the run and use
`h2 sim diff` on the two `report.json` files.

Numbers from the lab host are the ones to quote. A run on the driving Mac,
with its builds and editors, is only a smoke check.

## Theater runs

`--film NAME` makes a run record a film. `NAME` is a file stem of 1 to 64
letters, digits, `.`, `_` or `-`, not starting with `.` and without a
`.h2film` extension. The host passes the game `--film
<data root>/films/NAME.h2film`, the directory the game's agent resolves
`theater.play {"id"}` from (`H2_FILM_DIR` is not passed through, so the
library is `<data root>/films`). The data root is the one the run plays
with, so a `--fresh-data` run records into its own scratch library. The
host refuses a name that already has a film in the library, or a pending
job, before it starts the run.

When the game exits, whether it finished or `h2 lab stop` ended it, the
host links (or copies) the film to `runs/<id>/NAME.h2film`, so `h2 lab
fetch` brings it back and a `--fresh-data` film survives the removal of
its scratch root. It reads the film's header with the `film` crate and
records the header ID as `film_id` in the job's metadata (`h2 lab wait`,
`h2 lab runs`) and in its notes. A film whose header can't be read is
named in the notes, without an ID. `-- --film PATH` is refused like any
other path flag.

Playback is the game's own: a `--serve` run's agent starts the isolated
playback process for `theater.play`, which inherits the run's disc, image
directory, data root and `H2_OPT`. The playback runs in the `--serve`
run's window, and `h2 lab agent <id> theater.*` drives it. The film
library is the persistent data root's, so record and play without
`--fresh-data` (a `--fresh-data` film plays only in the run that recorded
it, or after fetching):

```
h2 lab run --host laptop --name "Film: record" --current --level 03a_oldmombasa --film oldmombasa-1 --frames 1800 --wait
# the job's film_id is the film's ID
h2 lab run --host laptop --name "Film: play" --current --serve --no-stats
h2 lab agent --host laptop <serve-id> theater.play '{"id":"<film_id>"}'
h2 lab agent --host laptop <serve-id> theater.state
h2 lab stop --host laptop <serve-id>
```

The playback process starts with the run's environment, including its
`H2_FRAME_STATS_CSV` path, so start the playback run with `--no-stats`.
A film plays only on an image whose stamp matches the recording
(`theater.compat`), so record and play with the same `--current` or
`--rev`.

## Recordings

The intended loop: you record one heavy fight on the laptop, once; the
agent replays it as often as it likes with frame stats and the profiler
on, on as many builds as it likes.

On the laptop: `h2 lab record --level 03a_oldmombasa --difficulty heroic
--name fight-1`, play the fight, quit the game. Through the host when it
runs (so the agent sees the run), else directly into the default root.
A path-safe label such as `fight-1` doubles as the recording key. For other
labels the default is the level and a timestamp. Pass `--record KEY` to
choose the recording directory separately from the window label. A
recording made elsewhere goes up with `h2 lab put FILE --as NAME --level
MAP`; `h2 lab recordings` lists them.

To use an agent recording from a lab job with `--replay`, finish the
recording with `input.stop`, fetch the job, and upload its input log as a
lab recording. Supply the recorded level and difficulty so replay boots
with the same settings.

```sh
h2 lab fetch --host laptop 0007 --to ./lab
h2 lab put --host laptop ./lab/0007/inputs/fight.input --as fight \
    --level 03a_oldmombasa --difficulty heroic
h2 lab run --host laptop --replay fight --frames 3000
```

For a replay from level boot, start the recording at the same point in
boot so the game's read indices line up.

A recording is a directory with a manifest, not a file:

```json
{ "name": "fight-1", "level": "03a_oldmombasa", "difficulty": "heroic",
  "commit": null, "input": "input.bin", "snapshot": null,
  "created": 1795000000, "run": "0004" }
```

`commit` is the image's commit when the host built it (null for
`--current`). Wait for the recording job to finish before replaying it.

### What replay does and doesn't promise

The input log is `xapi::tap`'s: every `XInputGetState` the game made,
answered on replay by read index (crates/extensions/game/src/lockstep_glue.rs). A
replay starts from a fresh boot of the level.

- **Not verified: that a replay from a fresh level boot follows the same
  path as the recording.** Nobody has yet compared a recorded run with its
  replay (`H2_LOCKSTEP` traces, `h2 lockstep A B`) through a level boot.
  [agent-api.md](agent-api.md) notes that two headless runs of the same
  level drift apart a few minutes in; until that is understood, a replay
  of a long fight may not be the same fight, and frame stats from it
  compare builds only roughly. The lab does not take `H2_LOCKSTEP` traces
  yet; that is the check to add before trusting a replay.
- The data root changes what a run does (profiles, saves, cache
  partitions); use `--fresh-data` for runs compared frame for frame.
- A different build of the game may read input a different number of
  times, which moves every later answer. Replays across commits are
  approximate by nature.
- **Replay from a snapshot is not supported.** The recording's `snapshot`
  field (`crates/apps/cli/src/lab/proto.rs`) is reserved for a machine
  snapshot; a host refuses to replay a recording whose `snapshot` is set
  (`crates/apps/cli/src/lab/host.rs`).
