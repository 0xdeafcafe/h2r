# The lab: player consent, game opens and memory limits

Status: Built. When a job waits for the player, what happens when the player opens a game, and the memory limits. Part of [The lab](lab.md).

## Player consent before a job

Before the lab starts a run or an uncached build, it checks the playing
Mac for a game window. It asks the game's [agent API](agent-api.md) on
the per-process sockets under `~/Library/Caches/h2recomp/agents/` and
on its own run sockets. The agents directory is a field of the host's
configuration (`HostConfig::agents`, the cache directory's `agents/` in
production), so tests point it at a temp directory and never see a game
the user is playing. It also checks the local process list for
recognizable game launches, including the bundled macOS app.
A headless serve and jobs already running under this host leave the Mac
available for new lab jobs. An open player game window whose agent cannot
show the prompt leaves the job skipped.
Service updates use the separate maintenance path.

The game shows a "Lab request" dialog with a short body:

```
Lab driver: build, then run (about 4 min)
"Menu bands check"
lab/menu-bands at cb9b315, level ascension
```

The first line names the requesting lab driver, what the job does
(build, run, build then run, run a sim, or build then sim) and the
estimated duration, or "time varies". The second line is the job's
`--name`. The third is its source: `lab/NAME at COMMIT` for
`--branch` (or the driving checkout's branch when none is given),
`REV at COMMIT` for `--rev`, or `installed image at COMMIT` for
`--current`, then the level when the run names one and "own data" when
the run uses `--own-data`. Long fields are cut so every line stays on one
line, and three lines are all the room above the dialog's rows. A build
with no `--name` of its own is
called "Image build". The host sends these as the `name`, `source`,
`operation`, `level` and `own_data` fields of the game's
[takeover request](agent-api.md), and `h2 lab` sends the branch it
resolved as `build.branch`.
The dialog opens over the current screen and pauses local campaign play.
Networked games keep running while the dialog is up. Its rows are
**Not now**, **Start alongside** and **Close and start**, with **Not now**
focused first. A job whose numbers need a quiet machine (`h2 lab bench`, a
sim benchmark, a profiled run) leaves out **Start alongside**: the dialog
has the two rows **Not now** and **Close and start**, and the request
carries `"alongside": false`.

Every game drawn by its own D3D with XAPI pads, outside lockstep, the
light host and the sim, plans the dialog's hooks, with a window or
without one. Only a windowed game that is not a Theater replay takes a
request; in the others the hooks stay idle. A film's replay therefore
installs the hooks its recording did, and its hook digest matches
(`registration::lab_takeover` in `crates/apps/boot/src/registration.rs`,
[film-format.md](../game/film-format.md#determinism-and-verification)).

**Start alongside** (the request's `alongside` answer) starts the job at
once and leaves the player's game open. The lab doesn't call `host.quit`.
The job's `player` field, run log and summary say
`player: kept their game open; the job ran alongside it`. That game counts
as already asked, with the answer [Keep running](#when-the-player-opens-a-game), so the
running-job question doesn't come up for it mid-job and the queue doesn't
wait for it. Start alongside isn't a decline: it doesn't start the
cool-down below. Lab games bind different XNet ports from the player's
(see below), so the two can run together. A second consent for the same
job, after an uncached build, doesn't ask about that game again.

**Close and start** is the player's consent (the request's `allowed`
answer). The lab closes the game for them: it calls the game's
[`host.quit`](agent-api.md#close-the-window), which closes the window as
its close button does, then waits up to 60 seconds for the process to
exit, and starts the job once it has. The game has no separate save step:
a campaign keeps what its last checkpoint saved, as when the player
closes the window themselves. Code: `close_allowed` in
`crates/apps/cli/src/lab/takeover.rs`.

When the game doesn't close (its agent doesn't take `host.quit`, it
refuses, or it is still open after the 60 seconds), the job isn't
skipped. It goes back to the front of the queue as `queued`, and the
queue waits for that game to close, as after
[Stop and pause](#when-the-player-opens-a-game). The installed host (the
LaunchAgent from `h2 lab host install`) shows a macOS
notification on the playing Mac, "Lab job waiting: your game is still
open", telling the player to quit the game from its menu or close its
window, and that the job starts once it has closed. The job's `player`
field and the job lines of `h2 lab logs` and `h2 lab host watch` carry the
reason, for example
`player: allowed the job, but their game did not close within 60 seconds
of being asked; quit it from its menu or close its window, and the job
starts once it has closed`. Once the game is gone the job starts with no
second prompt, since no game window is open.

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

## When the player opens a game

The player comes first. While a job runs or builds an uncached image, the
host looks for the player's games every 4 seconds, the same way the
consent check finds them. A game it finds asks the player once what the
job should do. Code: `crates/apps/cli/src/lab/host/player.rs` (the
host's side) and `PlayerGames` in `crates/apps/cli/src/lab/takeover.rs`
(the decisions).

The game shows a "Lab request" dialog over the current screen:

```
Lab driver is running
"Menu bands check"
while you play. What now?
```

| Choice | Effect |
|---|---|
| **Finish this one** | The job runs to its end. The rest of the queue waits while the game is open |
| **Keep running** | No change: the job and the queue carry on alongside the game |
| **Stop and pause** | The host stops the job as `h2 lab stop` does (SIGTERM, then SIGKILL after the grace). The job goes back to the front of the queue as `queued`, and the queue waits while the game is open |

Finish this one has the initial focus. Back and an unanswered dialog both
pick it after the consent timeout (60 seconds by default). A game that
can't show the dialog within that time also gets Finish this one. The
host asks each game once and keeps the answer until that game closes.
The choice goes into the job's `player` field, its run log and the end of
its `summary`, for example
`player: opened a game and chose Stop and pause: ...`.

A job never takes the ports of the player's game. Every lab game binds
XNet at base 3174: a windowed run gets `H2_XNET_ADDR=<LAN IPv4>:3174`
(`lab_xnet_addr` in `crates/apps/cli/src/lab/host.rs`), and a [dedicated
host](lab-runs.md#dedicated-system-link-host) defaults to it. A game the player
opens binds 3074 to 3080, so the two never collide.

While the queue waits, `h2 lab status` shows `"held_for_player": true`.
The held jobs start when the player's game closes, and each still asks
for [consent](#player-consent-before-a-job) as usual. The host sends the
question as a [takeover request](agent-api.md) with `"ask": "running"`;
the game answers `stop_and_pause`, `keep_running` or `finish_this`.

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
a build, and holds queued jobs until pressure clears. The stop records the
lab disk's free bytes at that moment as `disk_free_bytes` in
`mem/lab-host.json`; under the [free-disk floor](lab.md#the-free-disk-floor-and-h2-lab-clean)
the reason reads `machine memory pressure (disk nearly full: X GiB free)`.

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
