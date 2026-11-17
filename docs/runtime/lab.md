# The lab: runs on another Mac over SSH

`h2 lab` lets an agent on one Mac start levels on a second Mac (the one
you play on), replay input recordings there, and bring back frame stats,
profiles and logs. It is a CLI over SSH: no web page, no MCP. Code:
`crates/cli/src/lab/`. Tests: `crates/cli/tests/lab.rs` (a fake game, a
fake cargo and a stand-in for ssh, all on one machine).

```
this Mac (agent)                      the laptop (you play)
h2 lab run --host laptop ...  --ssh-->  h2 lab pipe  --unix socket-->  h2 lab host
                                                                         |-- git worktree per commit
                                                                         |-- cargo build, h2 build (cached)
                                                                         `-- h2 play --level ... (a window)
h2 lab fetch <id>  <--ssh: tar of runs/<id>--
```

## Setting up the laptop

1. **Remote Login.** System Settings, General, Sharing, Remote Login on,
   for your user only.
2. **The agent's key.** Append this Mac's public key to the laptop's
   `~/.ssh/authorized_keys`. Check from this Mac that `ssh laptop true`
   works without a prompt: the lab runs ssh in batch mode and never asks
   for a password.
3. **A checkout.** Clone the repository on the laptop, put your disc at
   `disc/halo2.iso` in it, and build `h2` once:
   `cargo build --release -p cli --bin h2`.
4. **The host.** In a Terminal window on the laptop, logged in at the
   desktop, run `target/release/h2 lab host`. Leave it running. It has to
   be a Terminal in your session, not something started over ssh: a game
   launched from sshd's session may get no window and no GPU.

The host keeps everything under its root, by default
`~/Library/Caches/h2recomp/lab`:

| Path | What |
|---|---|
| `runs/<id>/` | one job: `meta.json`, `build.log`, `run.log`, `frames.csv`, `profile/` |
| `recordings/<name>/` | `manifest.json` and `input.bin` |
| `pgo/<name>` | PGO profiles uploaded with `h2 lab put --pgo` |
| `src/<commit>/` | a detached git worktree of each commit built |
| `target/` | the cargo target directory those builds share |
| `images/<key>/` | a built `h2` and its image (`out/`), per commit and build |
| `obj-cache/` | `H2_OBJ_CACHE` for every image build |
| `data/` | the data root runs use (not your own saves) |
| `h2` | a link to the `h2` running the host, which `ssh` calls |

The socket is `~/Library/Caches/h2recomp/lab.sock` (mode 0600; next to the
agent API's `agent.sock`), or `--socket`, or `$H2_LAB_SOCKET`. Host flags:
`--root`, `--repo` (the checkout builds come from; default the one this
`h2` was built in), `--disc` (default `$H2_DISC`, else `<repo>/disc/halo2.iso`),
`--h2` and `--out` (the `h2` and image directory for `--current` runs;
default this `h2` and `h2 play`'s own), `--data`, `--cargo`.

Stopping the host (Ctrl-C) leaves a game it started running; stop the job
first (`h2 lab stop <id>`). A job a host left unfinished is marked failed
the next time a host starts on that root.

## The agent's side

Every client command takes `--host [user@]HOST` (or `$H2_LAB_HOST`). It
runs `ssh HOST <remote-h2> lab pipe`, which bridges ssh's stdin and stdout
to the host's socket, so no socket forwarding is needed. `<remote-h2>` is
`--remote-h2`, else `$H2_LAB_REMOTE_H2`, else the link the host keeps at
`Library/Caches/h2recomp/lab/h2` relative to the remote home: it works as
soon as the host has started once. `--host local` talks to a socket on this
machine. `$H2_LAB_SSH` replaces the ssh command (the tests use it).

```
h2 lab status --host laptop
h2 lab run --host laptop --level 03a_oldmombasa --difficulty heroic \
    --frames 3000 --stats-from 600 --build-env H2_OPT=2 --wait
h2 lab runs --host laptop
h2 lab fetch --host laptop 0007
```

### Builds go by branch

A run (and `h2 lab build`) names what to build:

- **`--branch NAME`**, or nothing: this checkout's current branch. The
  client first pushes it into the laptop's checkout as `lab/NAME`
  (`git push --force` over the same ssh; never to GitHub), then asks for
  that exact commit. Uncommitted changes here are not included; the client
  says so. `--no-sync` skips the push and uses whatever `lab/NAME` was last.
- **`--rev REV`**: a commit or branch the laptop's checkout already has.
- **`--current`**: no build: the host's own `h2` and image as they are.

`h2 lab sync [--branch NAME]...` pushes without running.

For a commit, the host makes a detached worktree at `src/<commit>` (your
own checkout is never checked out or touched), builds its `h2` with
`cargo build --release -p cli --bin h2` (plus `--features perf` for
`--perf` or `--feature perf`), then `h2 build` with the build variables and
the shared object cache. The image is kept under `images/<key>`, the key
being the commit, the features and the build variables (`H2_OPT`,
`H2_DEBUG`, `H2_CPU`, `H2_HOT`, `H2_OPT_COLD`, `H2_PASSES`, `H2_PGO`; a
`use:` profile counts by its contents). So A/B runs of two branches build
each once. The build variables are set again on every run of the image,
since an `h2` whose `H2_OPT` or `H2_PGO` differs from its image's rebuilds
it. A job's `meta.json` records its commit, image key, whether the image
was cached and the build variables. `h2 lab status` shows a job `building`,
then `running`.

`H2_PGO=use:NAME` names a profile uploaded with
`h2 lab put FILE --pgo --as NAME`.

### Runs

`h2 lab run` flags:

| Flag | Becomes |
|---|---|
| `--level MAP [--difficulty D]` | `h2 play --level MAP --difficulty D` |
| `--frames N` | `H2_BOOT_STOP_AT_PRESENT=N` |
| `--stats-from N` | `H2_FRAME_STATS_FROM=N` |
| (always, unless `--no-stats`) | `H2_FRAME_STATS=1`, `H2_FRAME_STATS_CSV=runs/<id>/frames.csv` |
| `--replay NAME` | `H2_INPUT_REPLAY=recordings/NAME/input.bin`; level and difficulty from its manifest unless given |
| `--record NAME` | `H2_INPUT_RECORD=recordings/NAME/input.bin` and its manifest |
| `--perf` | `H2_PROFILE=runs/<id>/profile`, `H2_PERF_FROM=level`, and the `perf` feature |
| `--env KEY=VALUE` | one of `H2_FRAME_STATS`, `H2_FRAME_STATS_FROM`, `H2_BOOT_STOP_AT_PRESENT`, `H2_PERF_FROM`, `H2_PROFILE_HZ`, `H2_HOST_PERF`, `H2_AUDIO_STATS`, `H2_FPS`, `H2_RENDER_SCALE`, `H2_WATERMARK`, `H2_MAPPACK`, `H2_HALO1`, `H2_FEATURES`, `H2_LOCKSTEP_TICKS`, `H2_ICONS` |
| `-- FLAGS` | `h2 play` flags from a fixed list: `--no-audio`, `--no-vsync`, `--unpaced`, `--fps N`, `--set KEY=VALUE`, ... |
| `--fresh-data` | an empty data root for this run, removed after it |
| `--wait [--to DIR]` | wait for the end, then fetch |

Anything else is refused: no other variables (paths the game writes are
the host's to choose), no `h2 play` flags that take paths or reach the
network, no names with `/` or a leading `.`. The host also drops every
`H2_*` variable of its own environment from the game's, so a run means the
same whatever Terminal the host was started in. None of this is a sandbox:
whoever reaches the socket can already log in as you. It keeps runs
comparable.

One job runs at a time; another `run` or `build` is refused with the busy
job's id. `h2 lab wait <id> [--timeout S]` exits 0 when the job finished
well, 1 when it failed or was stopped, 3 when it is still going.
`h2 lab stop <id>` sends SIGTERM to the job's process group, then SIGKILL
5 s later.

`h2 lab runs` lists jobs with their state, exit, duration, commit, level
and the frame stats report from the log: the last `frame stats:` line,
the three worst frames and the `outcome:` line. `h2 lab fetch <id>` copies
`runs/<id>` here as `<DIR>/<id>` (`--to DIR`; default `$RUSH_KEEP/lab`,
else `./lab`), through `tar` over the same ssh.

## Recordings

The intended loop: you record one heavy fight on the laptop, once; the
agent replays it as often as it likes with frame stats and the profiler
on, on as many builds as it likes.

On the laptop: `h2 lab record --level 03a_oldmombasa --difficulty heroic
--name fight-1`, play the fight, quit the game. Through the host when it
runs (so the agent sees the run), else directly into the default root. A
recording made elsewhere goes up with `h2 lab put FILE --as NAME --level
MAP`; `h2 lab recordings` lists them.

A recording is a directory with a manifest, not a file:

```json
{ "name": "fight-1", "level": "03a_oldmombasa", "difficulty": "heroic",
  "commit": null, "input": "input.bin", "snapshot": null,
  "created": 1795000000, "run": "0004" }
```

`commit` is the image's commit when the host built it (null for
`--current`).

### What replay does and doesn't promise

The input log is `xapi::tap`'s: every `XInputGetState` the game made,
answered on replay by read index (crates/boot/src/lockstep_glue.rs). A
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
- **Planned: replay from a snapshot.** The `snapshot` field is for a full
  machine snapshot taken mid-level (agent API phase 5), which does not
  exist. When it does, a recording will carry one and replay will start
  from it, without changes to these commands. A host refuses to replay a
  recording whose `snapshot` is set.

## Not verified without a second Mac

Tested here: the protocol and its refusals, the host's job lifecycle, stop,
the image cache, worktrees, `git push` into the host's checkout, `pipe`
through a stand-in for ssh, and fetch and put through it. Not tested: real
ssh to another machine, and the real game under the host (that a game
started from a Terminal-launched host gets a window and the GPU, and that
`h2 play` exits on its own at `H2_BOOT_STOP_AT_PRESENT`).

## Related

- [agent-api.md](agent-api.md): the running game's own API (`h2 serve`,
  `h2 agent`); snapshots are its phase 5
- [level-boot.md](level-boot.md): `--level`, `H2_PERF_FROM=level`
- [BOOT.md](BOOT.md): the environment variables
- [frame-budget.md](../perf/frame-budget.md): reading frame stats
- [perf.md](../perf/perf.md): the profiler (`--features perf`)
- [high-framerate.md](../game/high-framerate.md): `lockstep` and `xapi::tap`
- [BUILD-CACHE.md](../lifter/BUILD-CACHE.md): `H2_OBJ_CACHE`
