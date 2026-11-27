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

## Pages

| Page | Holds |
|---|---|
| [The lab: player consent, game opens and memory limits](lab-consent.md) | When a job waits for the player, what happens when the player opens a game, and the memory limits. |
| [The lab: driving runs, hosts, benchmarks and recordings](lab-runs.md) | The agent API on a run, own-data play, the dedicated System Link host, sim benchmarks, Theater runs and recordings. |

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
`h2 lab host update` on the laptop and `h2 lab update --host laptop`
on the driver run `git pull --ff-only` on the checkout's tracked branch.
They then build the checkout's new `HEAD` commit the way an image job
builds a commit: the commit is checked out in `src/current/` and
`cargo build --release -p cli --bin h2` runs there into the lab's
`target/`. Updates and image jobs share one source tree and one Cargo
target, so a commit that both of them build compiles once. Uncommitted
changes in the checkout are not part of an update. The update then
swaps the stable copy and restarts the service with
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
| `runs/<id>/` | one job: `meta.json`, `build.log`, `run.log`, `frames.csv`, `profile/`, `traces/`, `flights/`, `inputs/`, `mem/`; a sim job has `sim/` and `sim-summary.*` instead of the frame files |
| `a/<id>.sock` | the run's game's agent API socket, removed when the run ends |
| `takeover-declined-until` | the player's consent cool-down, kept across lab host restarts |
| `recordings/<name>/` | `manifest.json` and `input.bin` |
| `pgo/<name>` | PGO profiles uploaded with `h2 lab put --pgo` |
| `src/current/` | the detached Git worktree shared by branch builds and host updates |
| `target/` | the cargo target directory those builds share |
| `variants/<key>/` | one build variant's image work directory: the built `h2`, game module, generated sources, object pins and build stamp (`image.json`) |
| `../obj/` | `H2_OBJ_CACHE` for every image build: `obj` beside the root, which for the default root is `~/Library/Caches/h2recomp/obj`, the object cache the Mac's own `h2 build`s use ([AGENTS.md](../../AGENTS.md)) |
| `data/` | the data root runs use unless they pass `--own-data`; `data/films/` is the film library `theater.play` reads |
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
colour is enabled on a terminal. See `crates/apps/cli/src/lab/watch.rs`
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
the requested revision. Cargo and the image compiler run at nice level
+15 with a worker budget of the available CPU count minus two (minimum
one). Code: `ensure_image` in `crates/apps/cli/src/lab/host.rs` and
`crates/apps/cli/src/lab/build_cache.rs`.

**The Rust build.** Every build, and every host update, runs
`cargo build --release -p cli --bin h2` in `src/current/` into the one
`target/`. One source tree has one target, so Cargo's fingerprints stay
valid between builds, and a commit already built (by a job or an update)
is fresh. The `--feature` list is the only per-job input. The host's own
`H2_*` variables are dropped and the job's build variables are not
passed, because they pick the game image, not the Rust build. Cargo reads
its configuration as it does in a Terminal, so a machine-wide
`build.rustc-wrapper` such as kache in `~/.cargo/config.toml` applies to
lab builds too. The lab never sets `RUSTC_WRAPPER` itself
([Rust dependency builds](../lifter/BUILD-CACHE.md#rust-dependency-builds)).

**The game image.** Each build variant has its own work directory,
`variants/<key>/`. The key hashes the sorted features, the build
variables and the content hash of the selected PGO profile: everything
that picks an image except the commit. Every commit of a variant passes
the same `--out` to `h2 build`, with `H2_OBJ_CACHE` set to the shared
object cache beside the root. Emission rewrites only the C files that
changed, clang compiles only those shards (the rest come from the work
directory's pins or the shared cache, including objects the Mac's own
builds made) and the analysis comes from the boot cache
([The `h2 build` and `h2 play` path](../lifter/BUILD-CACHE.md#the-h2-build-and-h2-play-path)).
That path has no emission receipt: lifting and emission run on every
build, which takes seconds when nothing changed, and the work they skip
is the file writes and clang.
Another `H2_OPT`, feature set or profile builds in another directory.
The work directory is also the published image: the host reuses it when
its binary, module and matching `image.json` stamp are present. The
stamp identifies the commit as well as the variant. Switching one variant
from commit A to B and back to A rebuilds twice, each time reusing the
unchanged C, Cargo artifacts and compiler objects. Alternating two
variants on one commit builds each once. The host invalidates the stamp
before a replacement and publishes a new one after the build and cache
cleanup succeed. A build holds `lab-build.lock` in its work directory,
so a second builder of the same directory is refused even though the
activity lock already allows one build at a time. Each run receives the
build's environment. A job's `meta.json` records its commit, image key,
cache hit or miss, and build variables.

The job's `build.log` says what each build reused: whether the work
directory is new or which commit it last held, how many crates Cargo
compiled and the rustc wrapper in effect (and where it is configured),
and from `h2 build`'s phase lines whether the analysis was cached, how
many C files were rewritten and how many shards clang reused, for example
`lab: image reuse: analysis cached, emit rewrote 12/1043 C files, clang
reused 1031/1043 shards (12 compiled)`.

After each build attempt the host removes least recently used objects
above the object-cache budget (8 GiB by default). The cache is shared
with the Mac's own builds, so the budget bounds them too, and objects
they add during pruning are logged rather than failing the job. Each
build's pins keep its own objects linkable. It retains the shared
Cargo target up to its separate budget (32 GiB by default). If it
exceeds that budget the host clears the whole target so Cargo can
recreate a consistent set of artifacts on the next build. The published
images remain usable. Both budgets support zero. It keeps the four image
work directories used last (by their stamp's time, which a cache hit
renews) and removes older ones. The host and install flags are in
[Switches](switches.md#tools).

### The free-disk floor and `h2 lab clean`

Every cache above lives on the laptop's disk, and a nearly full macOS
disk starves swap, so the game is then killed for memory pressure. The host
keeps a floor of free space on the lab root's volume (20 GiB by default;
`--disk-floor SIZE`, zero turns it off). `h2 lab status` reports the volume
as `disk: {free_bytes, total_bytes}` beside `disk_floor_bytes`, and each
job's `meta.json` records `disk_free_bytes_at_start`.

Before an uncached image build, if the free space is under the floor, the
host prunes in this order and stops at the first step that clears it:

1. finished job runs past the newest 30 (oldest first; a run whose job is
   not final is never removed);
2. image work directories, least recently used first, never the one being
   built;
3. the object cache down to half its budget;
4. the Cargo target.

If the space is still under the floor the job fails with `lab: only X GiB
free on the lab disk (floor Y GiB) after pruning; free space on the laptop`,
in `job.error` and `build.log`.

`h2 lab clean --host NAME [--all]` runs that prune on request, through the
host's socket with the same pairing as `h2 lab stop`. Without `--all` it
removes everything the floor's order can: runs past the newest 30, every
image work directory but the one used last, the object cache down to half
its budget and the Cargo target. `--all` empties them completely and also
removes the source checkout; recordings, PGO profiles, the data root and
the host's `bin/h2` stay. The reply lists the bytes freed per category
(`freed`, `freed_bytes`), `disk_before` and `disk_after`, and the log lines.
While a job is active only finished runs are removed, and the reply's
`skipped` says so: the active job's image, caches, checkout and run
directory are in use. A clean holds the activity lock, so a job submitted
meanwhile is refused as busy.

At startup the host takes the build/run activity lock and refuses to start
while an interrupted job still has children writing its logs; it then
recovers the unfinished jobs. Job results and recordings are kept.

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
| `--dedicated` | a headless System Link host instead of a game window (see [Dedicated System Link host](#dedicated-system-link-host)) |
| `--replay NAME` | `H2_INPUT_REPLAY=recordings/NAME/input.bin`; level and difficulty from its manifest unless given |
| `--record NAME` | `H2_INPUT_RECORD=recordings/NAME/input.bin` and its manifest |
| `--film NAME` | `h2 play --film <data root>/films/NAME.h2film`: the run records a film into the film library its game plays from (see [Theater runs](lab-runs.md#theater-runs)) |
| `--perf` | `H2_PROFILE=runs/<id>/profile`, `H2_PERF_FROM=level`, and the `perf` feature |
| `--env KEY=VALUE` | one of `H2_FRAME_STATS`, `H2_FRAME_STATS_FROM`, `H2_BOOT_STOP_AT_PRESENT`, `H2_PERF_FROM`, `H2_PROFILE_HZ`, `H2_HOST_PERF`, `H2_AUDIO_STATS`, `H2_FPS`, `H2_RESOLUTION`, `H2_HUD_POSITION`, `H2_LOD_DISTANCE`, `H2_RENDER_SCALE`, `H2_WATERMARK`, `H2_MAPPACK`, `H2_HALO1`, `H2_FEATURES`, `H2_LOCKSTEP_TICKS`, `H2_ICONS`, `H2_MEM_SAMPLE`, `H2_PREWARM_PROGRAMS`; `H2_PIPELINE_CACHE=off` (no pipeline cache) or `H2_PIPELINE_CACHE=fresh` (an empty one at `runs/<id>/pipeline-cache.jsonl`), never a path |
| `-- FLAGS` | `h2 play` flags from a fixed list: `--no-audio`, `--no-vsync`, `--unpaced`, `--fps N`, `--set KEY=VALUE`, ... |
| `--fresh-data` | an empty data root for this run, removed after it |
| `--own-data` | play on this Mac's own data: no `--data`, so the game takes the default data root, its `settings.toml` and Live accounts as `h2 play` there would (see [Playing on your own data](lab-runs.md#playing-on-your-own-data)); refused with `--fresh-data` |
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

Before the lab starts a run or an uncached build, it checks the playing Mac for a game window. Detail: [The lab: player consent, game opens and memory limits](lab-consent.md).

## When the player opens a game

The player comes first. Detail: [The lab: player consent, game opens and memory limits](lab-consent.md).

## Memory limits and reports

The host samples the running game's physical footprint at launch and every two seconds. Detail: [The lab: player consent, game opens and memory limits](lab-consent.md#memory-limits-and-reports).

## Driving a run: the agent API

See [The lab: driving runs, hosts, benchmarks and recordings](lab-runs.md).

### Playing on your own data

A lab run plays on the host's `data/`, not the player's. Detail: [The lab: driving runs, hosts, benchmarks and recordings](lab-runs.md#playing-on-your-own-data).

## Dedicated System Link host

`h2 lab run --dedicated` starts a dedicated System Link host on the lab Mac. Detail: [The lab: driving runs, hosts, benchmarks and recordings](lab-runs.md).

## Sim benchmarks

`h2 lab sim` runs [`h2 sim`](sim.md) on the lab host and keeps its reports, so a headless bot match's speed is measured on the quiet second Mac rather than on the driving Mac. Detail: [The lab: driving runs, hosts, benchmarks and recordings](lab-runs.md).

## Not verified

The test suite exercises the Unix socket and paired TLS on loopback,
job lifecycle, the image cache, Git bundle sync, artifact transfers, and
`lab agent`, `bench` and `sim` against a fake game and a fake `h2 sim`
that hands out staged reports. No test runs the real `h2 sim` through the
lab. Unit tests cover pairing
expiry, single use, rejected keys and replays, and LaunchAgent generation
and lifecycle commands through a fake controller. Synthetic log feed tests
cover job transitions, overlapping jobs, log growth and rotation, and
stream framing.

Live checks remaining include Bonjour across two Macs, macOS Firewall,
LaunchAgent startup after login, and the game's window, Metal device and
agent API under the service.

## Related

- [sim.md](sim.md): `h2 sim`, the match and batch files, the report
- [agent-api.md](agent-api.md): the running game's own API (`h2 serve`,
  `h2 agent`)
- [level-boot.md](level-boot.md): `--level`, `H2_PERF_FROM=level`
- [BOOT.md](BOOT.md): the environment variables
- [gameplay-budget.md](../perf/notes/gameplay-budget.md): reading frame stats
- [profiling.md](../perf/profiling.md): the profiler (`--features perf`)
- [high-framerate.md](../game/high-framerate.md): `lockstep` and `xapi::tap`
- [BUILD-CACHE.md](../lifter/BUILD-CACHE.md): `H2_OBJ_CACHE`
