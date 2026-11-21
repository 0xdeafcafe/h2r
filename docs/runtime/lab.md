# The lab: runs on another Mac over the LAN

Status: Built. See [Not verified](#not-verified) for live Mac coverage.
Code: `crates/apps/cli/src/lab/`. Tests: `crates/apps/cli/tests/lab.rs`.
Switch: [Switches](switches.md#tools).

`h2 lab` lets a driving Mac start levels on a second Mac, replay input
recordings there, and collect frame stats, profiles and logs. The lab host
runs in the playing Mac's graphical session so games can open a window
and use Metal.

```text
driving Mac                         playing Mac (logged-in desktop)
h2 lab run --name City ...    --TLS-->  h2 lab host (Aqua LaunchAgent)
                                          |-- one detached worktree, FIFO job queue
                                          |-- cargo build, h2 build (cached)
                                          `-- h2 play --level ... (a window)
h2 lab fetch <id>              <--TLS--  archive of runs/<id>
h2 lab logs current --follow   <--TLS--  job and LaunchAgent logs
h2 lab agent <id> <call>        --TLS-->  h2 agent <call> --socket a/<id>.sock
```

## Setting up the laptop

1. Clone the repository on the laptop, put your disc at
   `disc/halo2.iso` in the checkout, and build `h2` once.

   ```sh
   cargo build --release -p cli --bin h2
   ```

2. From a Terminal on the laptop's logged-in desktop, install the host.
   When macOS Firewall first prompts for `h2`, allow incoming connections.
   Both Macs need to be able to reach each other on the LAN.

   ```sh
   target/release/h2 lab host install --name laptop
   ```

3. On the driving Mac, pair using the code printed by install. Then check
   the host status.

   ```sh
   h2 lab pair laptop 'PASTE_CODE_HERE'
   h2 lab status --host laptop
   ```

The installed host listens on `0.0.0.0:29422` by default and advertises
`_h2lab._tcp` over Bonjour. The name defaults to the Mac's LocalHostName.
`--listen IP:PORT` selects an address and port. A client can also connect
directly to a paired host by IP or `IP:PORT`. The host must present the
TLS public key saved at pairing. An entry saved for that exact address
takes precedence over pairings saved under other names.

The code pins the host's TLS public key and authorizes a single pairing
for ten minutes. Every LAN connection proves possession of the saved
driver key inside TLS. The host caps pending handshakes per IP and
rate-limits failed attempts. To issue another code, run
`h2 lab host pair-code` on the laptop. A successful
pairing replaces the previous driver. Run `h2 lab host unpair` on the
laptop to revoke that key and any unused code. Host credentials live under
`<root>/auth/` and the driving Mac stores its pinned peer under
`~/Library/Application Support/h2recomp/lab/peers/`. Key files have mode
0600. Give a code only to a driver you trust to send and build source code
on the laptop.

### Running and updating the LaunchAgent

`install` writes
`~/Library/LaunchAgents/com.0xdeafcafe.h2recomp.lab.plist` and loads it
into the user's GUI session. The Aqua LaunchAgent starts when the user
logs in after reboot and launchd restarts it if it exits. Keep the user
logged in and the laptop awake for remote runs. With the lid open, you
can run `caffeinate -i` on the laptop to prevent idle sleep while that
command runs.

`h2 lab host status` reports whether the service is installed, loaded and
running, with its PID and build version when available. `h2 lab host
uninstall` revokes pairing, unloads the idle job and removes the plist.
`h2 lab host` runs in the foreground for a Terminal session and serves
the private Unix socket. Add `--listen IP:PORT` to enable LAN access for
a foreground host, and use `h2 lab host pair-code` to issue a code. Pass
the foreground host's `--root` to `pair-code` when using a custom root.

The LaunchAgent runs a stable copy at `<root>/bin/h2`. Every 15 seconds
while idle, it checks `<repo>/target/release/h2` in the installed checkout.
It validates and atomically adopts a newer compatible build when the
executable's contents change, then exits so launchd starts the copy.
Ordinary game build jobs use a separate target directory.
`h2 lab host update` on the laptop and `h2 lab update --host laptop`
on the driver run `git pull --ff-only` on the checkout's
tracked branch and `cargo build --release -p cli --bin h2` into
`<repo>/target`, then swap the stable copy and restart the service with
launchctl. Both commands require a running, idle LaunchAgent. A build or
run in progress blocks updates, and maintenance refuses new jobs until
it finishes. Automatic and explicit restarts finish in-flight fetches,
uploads and branch syncs before restarting. The host closes stalled
connections after two minutes without socket I/O progress.

### Host files and flags

The host keeps its files under `--root`, by default
`~/Library/Caches/h2recomp/lab`.

| Path | What |
|---|---|
| `bin/h2` | stable LaunchAgent executable |
| `logs/` | LaunchAgent stdout and stderr, host updates, and pairing offers, successful pairings and revocations. Each log has a current file and one rotated copy of up to 8 MiB each. |
| `auth/` | TLS identity and pairing state |
| `runs/<id>/` | one job: `meta.json`, `build.log`, `run.log`, `frames.csv`, `profile/`, `traces/`, `flights/`, `inputs/`, `mem/` |
| `a/<id>.sock` | the run's game's agent API socket, removed when the run ends |
| `takeover-declined-until` | the player's consent cool-down, kept across lab host restarts |
| `recordings/<name>/` | `manifest.json` and `input.bin` |
| `pgo/<name>` | PGO profiles uploaded with `h2 lab put --pgo` |
| `src/current/` | the detached Git worktree shared by branch builds |
| `target/` | the cargo target directory those builds share |
| `image/` | the built `h2`, game module, generated sources and build stamp (`image.json`) |
| `obj-cache/` | `H2_OBJ_CACHE` for every image build |
| `data/` | the data root runs use |
| `scratch/<id>/data` | the empty data root of a `--fresh-data` run, removed when it ends |

Host and install flags take the same paths. `--repo` sets the checkout
and defaults to the one used to build the CLI. `--disc` defaults to
`$H2_DISC`, then the checkout's `disc/halo2.iso`. `--h2` and `--out`
select the binary and image for `--current` runs. They default to the
host executable and the usual `h2 play` image directory. `--data` sets
the persistent data root and `--cargo` selects the cargo executable.
`--max-jobs 1` is accepted for compatibility; any other cap is refused
because the host runs one job at a time. Install saves the Terminal's
`PATH` for the build tools.

The local socket is `~/Library/Caches/h2recomp/lab.sock` (mode 0600),
or a path set by `--socket` or the lab socket environment setting in
[Switches](switches.md#tools). To stop a foreground host, stop the
active and queued jobs (`h2 lab stop <id>`), then press Ctrl-C. During startup, the
host waits for exclusive access to the lab root before marking any
unfinished jobs failed.

### Watching jobs

Run `h2 lab host watch [--root DIR]` on the laptop. The watcher reads
`runs/<id>/meta.json`, `build.log`, `run.log` and the logs in `logs/`
directly from the lab root, so it works while the host is busy or
restarting. On attachment it shows every active job, or the latest job
when the host is idle. It follows each new job and keeps streaming older
jobs until they finish. Each job shows its build log before its run log.
State changes include the job ID, kind, level, short commit, elapsed time
and note. A job using the host's own image shows `commit=current`. Host events
use the `[host]` prefix and include pairing offers, successful pairings,
revocations and updates. On attachment the watcher starts with up to 16
KiB per log, then prints new lines as the files grow. Prefix
color is enabled on a terminal. See `crates/apps/cli/src/lab/watch.rs`
and `crates/apps/cli/src/lab/service.rs`.

On the driving Mac, use the paired connection:

```sh
h2 lab logs current --follow --host laptop
h2 lab logs 0007 --follow --host laptop
h2 lab logs 0007 --host laptop
```

`current --follow` follows the same feed as `host watch`. A named job
shows its own state and logs, and `--follow` keeps the stream open until
that job finishes, fails, is skipped or is cancelled. Omit `--follow`
for a snapshot of the current feed or a named job. Run the command again
to resume following after a host restart. The stream uses the existing lab
connection and transfer framing, with at most 64 KiB of payload per
frame. See `crates/apps/cli/src/lab/proto.rs`,
`crates/apps/cli/src/lab/host.rs` and `crates/apps/cli/src/lab/transfer.rs`.

## The agent's side

Every client command takes `--host NAME|IP[:PORT]|local`. LAN commands
use the paired TCP connection for the lab protocol and file transfers.
`--host local` uses the private Unix socket on the playing Mac. Follow
it with `--socket PATH` if the local host uses a custom socket.

```sh
h2 lab status --host laptop
h2 lab run --name "City baseline" --host laptop --level 03a_oldmombasa --difficulty heroic \
    --frames 3000 --stats-from 600 --build-env H2_OPT=2 --wait
h2 lab runs --host laptop
h2 lab fetch --host laptop 0007
```

### Builds go by branch

A run (and `h2 lab build`) names what to build:

- **`--branch NAME`**, or nothing: this checkout's current branch. The
  client sends committed objects in a Git bundle and updates the
  laptop's `lab/NAME` branch. It then asks for that exact commit. The
  client reports uncommitted changes, which stay on the driving Mac.
  `--no-sync` uses the existing `lab/NAME` branch.
- **`--rev REV`** names a commit or branch the laptop already has. The host resolves it to a commit when accepting the request, including when the job must queue.
- **`--current`**: no build: the host's own `h2` and image as they are.

`h2 lab sync [--branch NAME]...` transfers branches without starting a
run. It can import a branch while a game job runs.

For a commit, the host switches its detached `src/current/` worktree to
the requested revision. Each build uses the same Cargo target and passes
the same `--out <root>/image` directory to `h2 build`. Cargo and the
image compiler run at nice level +15 with a worker budget of the
available CPU count minus two (minimum one).

The host reuses the published image when its binary, module and matching
`image/image.json` stamp are present. The stamp identifies the commit,
sorted features, build variables and content hash of the selected PGO
profile. Changing any input rebuilds in place. Switching from A to B
and back to A rebuilds twice, reusing Cargo artifacts and compiler objects.
The host invalidates the stamp before a replacement and publishes a new
one after the build and cache cleanup succeed. Each run receives the
build's environment. A job's `meta.json` records its commit, image key,
cache hit or miss, and build variables.

After each build attempt the host removes least recently used objects
above the object-cache budget (8 GiB by default). It retains the shared
Cargo target up to its separate budget (32 GiB by default). If it
exceeds that budget the host clears the whole target so Cargo can
recreate a consistent set of artifacts on the next build. The published
image remains usable. Both budgets support zero. The host and install
flags are in [Switches](switches.md#tools).

At startup the host takes the build/run activity lock before removing
the old commit-named worktrees and 16-hex-key image directories. Job
results and recordings are kept.

`H2_PGO=use:NAME` names a profile uploaded with
`h2 lab put FILE --pgo --as NAME`.

### Runs

Every game job (`run`, `bench` and `record`) requires `--name TEXT`, a
printable label of up to 60 characters. Leading and trailing spaces are
trimmed. The window title reads `h2 · lab <id> · <name> · <short commit>`.
Status, runs, the job log and memory reports carry the name even when
the game has no window. Use `--note` for additional context.

`h2 lab run` flags:

| Flag | Becomes |
|---|---|
| `--name TEXT` | Required job label, kept in the metadata and window title |
| `--level MAP [--difficulty D]` | `h2 play --level MAP --difficulty D` |
| `--frames N` | `H2_BOOT_STOP_AT_PRESENT=N` |
| `--stats-from N` | `H2_FRAME_STATS_FROM=N` |
| (always, unless `--no-stats`) | `H2_FRAME_STATS=1`, `H2_FRAME_STATS_CSV=runs/<id>/frames.csv` |
| (always) | `H2_AGENT=<root>/a/<id>.sock` (see [Driving a run](#driving-a-run-the-agent-api)) |
| `--serve` | the same run with no frame limit (`--frames` is refused): it runs until `h2 lab stop`, for `lab agent` |
| `--replay NAME` | `H2_INPUT_REPLAY=recordings/NAME/input.bin`; level and difficulty from its manifest unless given |
| `--record NAME` | `H2_INPUT_RECORD=recordings/NAME/input.bin` and its manifest |
| `--perf` | `H2_PROFILE=runs/<id>/profile`, `H2_PERF_FROM=level`, and the `perf` feature |
| `--env KEY=VALUE` | one of `H2_FRAME_STATS`, `H2_FRAME_STATS_FROM`, `H2_BOOT_STOP_AT_PRESENT`, `H2_PERF_FROM`, `H2_PROFILE_HZ`, `H2_HOST_PERF`, `H2_AUDIO_STATS`, `H2_FPS`, `H2_RESOLUTION`, `H2_HUD_POSITION`, `H2_LOD_DISTANCE`, `H2_RENDER_SCALE`, `H2_WATERMARK`, `H2_MAPPACK`, `H2_HALO1`, `H2_FEATURES`, `H2_LOCKSTEP_TICKS`, `H2_ICONS`, `H2_MEM_SAMPLE`, `H2_PREWARM_PROGRAMS`; `H2_PIPELINE_CACHE=off` (no pipeline cache) or `H2_PIPELINE_CACHE=fresh` (an empty one at `runs/<id>/pipeline-cache.jsonl`), never a path |
| `-- FLAGS` | `h2 play` flags from a fixed list: `--no-audio`, `--no-vsync`, `--unpaced`, `--fps N`, `--set KEY=VALUE`, ... |
| `--fresh-data` | an empty data root for this run, removed after it |
| `--wait [--to DIR]` | wait for the end, then fetch |

Anything else is refused: no other variables (paths the game writes are
the host's to choose), no `h2 play` flags that take paths or reach the
network, no names with `/` or a leading `.`. The host also drops every
`H2_*` variable of its own environment from the game's, so a run means the
same regardless of the host's environment. A paired driver can send
source code for the host to build and run as the logged-in user.

One activity lock covers each job from build through game exit, so the
host never builds while a game runs and never runs two builds or two
games at once. New run and build requests receive job IDs immediately
and wait in arrival order. `h2 lab status` shows the active job in
`active` and again as the only entry of `jobs_active`, the queued IDs in
`queued`, and `max_jobs`, which is always 1. `--max-jobs` accepts only 1.
`h2 lab wait <id> [--timeout S]` exits 0 when the job completes
or is skipped, 1 when it fails or is stopped, and 3 when it is still
going. `h2 lab stop <id>` stops a pending request or sends SIGTERM to
the job's process group, then SIGKILL 5 s later.

A lab game closes its window when its guest thread returns, whether at the
`--frames` limit or on a crash, and the job ends when the process exits
(`close_on_guest_exit`, `crates/apps/cli/src/play_cmd.rs`). As a backstop,
the host watches the run log for the shell's `app: guest thread exited:`
line. A game still running 60 s after that line gets the same SIGTERM and
SIGKILL as `h2 lab stop`, and the job ends with the outcome the line gave: `done` for `Returned`, else `failed` with exit 1.
A note in the job's metadata and log records the stop (`wait_game`,
`crates/apps/cli/src/lab/host.rs`).

`h2 lab runs` lists jobs with their name, state, exit, duration, commit, level
and the frame stats report from the log: the last `frame stats:` line,
the three worst frames and the `outcome:` line. `h2 lab fetch <id>` copies
`runs/<id>` here as `<DIR>/<id>` (`--to DIR`; default `$RUSH_KEEP/lab`,
else `./lab`) using an archive streamed over the lab connection.
The host sets the game's [agent output root](switches.md#tools) to
`runs/<id>`. The archive includes the game's `traces/` from `trace.start`,
`flights/` from `flight.dump` and `inputs/` from `input.record`. Each job
gets its own directories, so jobs can reuse the same file names.
Use `trace.stop` and `input.stop` to finish writing before stopping the
job or fetching its files.

## Player consent before a job

Before the lab starts a run or an uncached build, it checks the playing
Mac for a game window. It asks the game's [agent API](agent-api.md) on
the per-process sockets under `~/Library/Caches/h2recomp/agents/` and
on its own run sockets. It also checks the local process list for
recognizable game launches, including the bundled macOS app.
A headless serve and jobs already running under this host leave the Mac
available for new lab jobs. An open player game window whose agent cannot
show the prompt leaves the job skipped.
Service updates use the separate maintenance path.

The game shows the requesting lab driver, the run or build, the commit or
installed image, and an estimated duration if one is available.
The dialog opens over the current screen and pauses local campaign play.
Networked games keep running while the dialog is up.
**Allow** lets the lab wait for the player to finish, save if needed,
and quit. The lab waits up to 60 seconds for the game window to close
after approval. Saving and quitting use the game's normal controls.

**Not now**, Back, and an unanswered prompt all decline the request.
The job finishes with `job.state` set to `"skipped"` and
`job.skip_reason` set to `"declined by the player"`. Gate callers such
as boot-check report it as skipped. Other conditions that leave the
lab without consent also skip the job and carry a specific reason.

The host defaults to a 60-second answer timeout and a 30-minute
cool-down. During the cool-down, an open game window skips later jobs
without another prompt. With no window, jobs run as usual. Set the two
durations on `h2 lab host` or `h2 lab host install` using the flags in
[Switches](switches.md#tools).

## Memory limits and reports

The host samples the running game's physical footprint at launch and every
two seconds. `h2 lab status` includes the daemon's current and peak footprint
and its pressure reading. The active job, `h2 lab runs` and the fetched
`meta.json` carry the game's last and peak readings in bytes. A missing
reading means the host has not measured that process.

The game child has a 12 GiB lab cap. For example,
`h2 lab run --name "Memory check" --mem-limit 10G --level ascension` sets a 10 GiB cap.
`--mem-limit 0` disables the per-job cap. The game and daemon also run their
own process watchdogs. Machine memory pressure stops the active job, including
a build, and holds queued jobs until pressure clears.

A cap or pressure stop fails the job with its reason in `meta.json`.
The host asks a running game's agent for a memory report before terminating
the job's process group. It saves its own observations in `mem/lab-host.json`.
If the game cannot answer, the host also attempts OS diagnostics from its
side. The game's watchdog writes its reports in the same `mem/` directory.
Both `h2 lab fetch <id>` and `h2 lab run --name "Memory check" --wait --to DIR` bring that directory
back with the rest of the run, including on failure.

To investigate a finished job, run `h2 lab mem <id> [--to DIR]`. The host
sends the job's `mem/` directory. The driver computes the footprint change
and prints up to twenty growing counters, heap tags and sampled stacks from
the fetched files. The default destination matches `fetch`. The
`postmortem` field in `meta.json` carries the same summary for a game that
exits abnormally, a job stopped under memory pressure or a run the host
finds interrupted on restart. If the process died before producing evidence,
the field records why attribution is unavailable.
See [Finding a leak after the fact](agent-api.md#finding-a-leak-after-the-fact).

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
In offline runs at the default distance of 1x, higher resolutions also retain
more detailed geometry. Film and input-tape runs use a fixed geometry
multiplier. [Readback and LOD policy](../graphics/high-resolution.md#determinism).
For a comparison against timings taken with 480-line LOD choices, pass
`--env H2_LOD_DISTANCE=retail` at each scale in an ordinary offline run.
To measure the cost of retaining higher detail, keep the resolution fixed
in two unrecorded offline runs and compare
`--env H2_LOD_DISTANCE=retail` with `--env H2_LOD_DISTANCE=1`.

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

## Not verified

The test suite exercises the Unix socket and paired TLS on loopback,
job lifecycle, the image cache, Git bundle sync, artifact transfers, and
`lab agent` and `bench` against a fake game. Unit tests cover pairing
expiry, single use, rejected keys and replays, and LaunchAgent generation
and lifecycle commands through a fake controller. Synthetic log feed tests
cover job transitions, overlapping jobs, log growth and rotation, and
stream framing.

Live checks remaining include Bonjour across two Macs, macOS Firewall,
LaunchAgent startup after login, and the game's window, Metal device and
agent API under the service.

## Related

- [agent-api.md](agent-api.md): the running game's own API (`h2 serve`,
  `h2 agent`)
- [level-boot.md](level-boot.md): `--level`, `H2_PERF_FROM=level`
- [BOOT.md](BOOT.md): the environment variables
- [gameplay-budget.md](../perf/notes/gameplay-budget.md): reading frame stats
- [profiling.md](../perf/profiling.md): the profiler (`--features perf`)
- [high-framerate.md](../game/high-framerate.md): `lockstep` and `xapi::tap`
- [BUILD-CACHE.md](../lifter/BUILD-CACHE.md): `H2_OBJ_CACHE`
