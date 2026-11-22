---
name: h2-lab
description: Use h2 lab to test on the laptop, run on the other Mac, replay a recorded run, collect logs or profiles, or benchmark frame rate on a paired Mac.
---

# Run and measure on another Mac

The lab host runs as an Aqua LaunchAgent in the playing Mac's logged-in
desktop session. The driving Mac submits jobs and fetches results over
paired TLS on the LAN. Setup uses Bonjour or an address on the LAN and
requires no SSH. Lab runs open a real `h2 play` window and use Metal.
See the [lab reference](../../../docs/runtime/lab.md).

Use a lab job when the task calls for a run on the other Mac. For a game
that is already running, inspect it through the agent API. See the
[agent API skill](../h2-agent-api/SKILL.md).

## Pair once

Use an existing pairing when available. For a new host, install from
Terminal on the playing Mac's logged-in desktop using its built `h2`.
Allow the macOS Firewall prompt if it appears and keep that Mac logged
in and awake. Substitute a Bonjour name for the example `lab-mac`.

```sh
h2 lab host install --name lab-mac
```

On the driving Mac, use the single-use code printed by install. If it
expires, issue another on the host with `h2 lab host pair-code`.
A new pairing replaces the previous driver.

```sh
h2 lab pair lab-mac 'PAIRING_CODE'
h2 lab status --host lab-mac
```

Use `--host NAME` explicitly when choosing a remote host. The default
comes from `H2_LAB_HOST` when set. On the playing Mac, `--host local`
uses the private socket. `h2 lab host status` instead checks the local
LaunchAgent's installation and process state.

## Check the shared host

```sh
lab_host=lab-mac
h2 lab status --host "$lab_host"
```

The host runs one job at a time. Give every `run`, `bench` and `record`
a required `--name TEXT` (up to 60 printable characters). Use a short
label that identifies the experiment or the A/B arm to a person looking
at the game window. The name also appears in job and memory reports. If it's busy, arrange a slot with the
person using it. Leave their job alone. Say what you're about to run
and share your job ID and current phase once it starts. Keep the ID so
you can stop your own job and fetch its results.

## Choose the source and start a bounded run

- `--branch NAME` syncs that local branch's committed objects to the
  host as `lab/NAME`, then runs the exact commit. With no source flag
  this is the current branch. Commit the changes you want to test
  before submitting. `--no-sync` uses the last synced `lab/NAME`.
- `--rev REV` resolves on the host. Use it to pin a commit the host
  already has. Use a branch sync first when the commit is only local.
- `--current` uses the host's existing `h2` and image. Use it for a
  quick check of that image. Build options require a branch or revision.

These source flags are mutually exclusive. The host reuses one checkout and one image directory. An image is cached
when its commit, features and build variables match.

Set `lab_map` to the level identifier for the task and choose a local
destination for artifacts. `--level` selects the level. `--frames`
sets the stop present and `--stats-from` sets the first present to
measure. They both count presents. Pick a stop point after the warmup
and with enough time for the scene you want to measure. `--note` puts
the purpose of the run in its metadata.

```sh
lab_map=MAP
lab_results="${RUSH_KEEP:-$TMPDIR}/lab"
h2 lab run --name "Frame timing baseline" --host "$lab_host" --branch perf-work --level "$lab_map" \
  --frames 1800 --stats-from 300 --build-env H2_OPT=2 --perf \
  --note 'Baseline frame timing and profile' --wait --to "$lab_results"
```

Replace `perf-work` with your branch. `--perf` enables profiling from
level start and requests a `perf` build for a branch or revision. With
`--current`, profiling depends on the installed binary's features.
`--wait --to DIR` waits for the job, fetches its artifacts into
`DIR/<id>` and returns failure if the job failed. Omit `--wait` to get
the ID immediately and manage the wait and fetch separately.

## Pass build and run variables

Use `--build-env H2_OPT=2` with a branch or revision. The host stores
build variables as part of the image key and applies them again when
running it. Matching `H2_OPT` is required to reuse the image.

Repeat `--env KEY=VALUE` for runtime settings. The lab admits only
these names from [RUN_ENV in the protocol](../../../crates/apps/cli/src/lab/proto.rs).

| | | |
|---|---|---|
| `H2_FRAME_STATS` | `H2_FRAME_STATS_FROM` | `H2_BOOT_STOP_AT_PRESENT` |
| `H2_PERF_FROM` | `H2_PROFILE_HZ` | `H2_HOST_PERF` |
| `H2_AUDIO_STATS` | `H2_FPS` | `H2_RENDER_SCALE` |
| `H2_RESOLUTION` | `H2_HUD_POSITION` | `H2_LOD_DISTANCE` |
| `H2_WATERMARK` | `H2_MAPPACK` | `H2_HALO1` |
| `H2_FEATURES` | `H2_LOCKSTEP_TICKS` | `H2_ICONS` |

The host chooses the paths for the agent socket, frame CSV and profiles.
Pass supported `h2 play` flags after `--`, for example
`-- --unpaced --no-vsync` when the measurement calls for unpaced output.

## Drive a run and release the slot

Use `--serve` for a run you control through the agent API. It opens the
same graphical session as a bounded lab run and stays up until stopped.
`--frames` and `--wait` are refused with `--serve`. Keep
`H2_BOOT_STOP_AT_PRESENT` unset for this workflow.

```sh
h2 lab run --name "Live inspection" --host "$lab_host" --current --level "$lab_map" --serve \
  --note 'Live frame inspection'
```

Set `lab_id` to the returned ID. Check status and call the API once the
job is running. The relay uses the run's own socket and CLI. Full RPC
names, shortcuts and stdin scripts work here too. Give `--timeout` in
seconds enough room for the RPC timeout, which is in milliseconds.

```sh
lab_id=0007
h2 lab status --host "$lab_host"
h2 lab agent --host "$lab_host" "$lab_id" session.methods
h2 lab agent --host "$lab_host" "$lab_id" wait \
  '{"phase":"gameplay","map_ticks":30}' 120000 --timeout 130
h2 lab agent --host "$lab_host" "$lab_id" session.status
h2 lab agent --host "$lab_host" "$lab_id" render.stats
```

Stop your job when done, including after a failed check. Wait for the
job to exit so its logs and metadata are final before fetching. `wait`
returns a nonzero exit for a stopped job.

```sh
h2 lab stop --host "$lab_host" "$lab_id"
h2 lab wait --host "$lab_host" "$lab_id"
```

## Read the results

```sh
h2 lab fetch --host "$lab_host" "$lab_id" --to "$lab_results"
```

Use `h2 lab runs --host "$lab_host"` to recover a job ID or see recent
outcomes. `fetch` copies the host's `runs/<id>/` to `<DIR>/<id>/`.
The default destination is `$RUSH_KEEP/lab` when set, otherwise `./lab`.

- `meta.json` records the job's state, source, options and image cache
  details.
- `build.log` and `run.log` show build and game output as applicable.
- `frames.csv` contains per-present frame stats when stats are enabled
  and the run has reached the measurement window.
- `profile/` contains profile output when the run collects it.
- `mem/` holds the game's memory reports and flight recorder. A job
  stopped for memory, or a game that failed, also has a `postmortem`
  summary in `meta.json`. `h2 lab mem --host "$lab_host" "$lab_id"`
  fetches only `mem/` and prints the largest growers. Add
  `--env H2_MEM_SAMPLE=1M` to a run to record allocation stacks. See
  [Finding a leak after the fact](../../../docs/runtime/agent-api.md#finding-a-leak-after-the-fact).

If the job fails, use its state and the last relevant section of the
logs to tell a build failure from a runtime failure. Keep the fetched
artifacts out of Git.

## Benchmark

`h2 lab bench` handles the gameplay wait, tick-window capture, render
stats and counter reads, then stops the job and fetches results. Use it
for a short measurement on the quiet host. Hold the level, measurement
window, resolution, LOD distance and pacing settings fixed across A/B
runs. Busy percentiles remove pacing time. FPS reports the wall rate.

```sh
h2 lab bench --name "Frame pacing benchmark" --host "$lab_host" --rev "$lab_rev" --level "$lab_map" \
  --build-env H2_OPT=2 --ticks 600 --to "$lab_results"
```

Set `lab_rev` to a revision already on the playing Mac. The fetched
directory also contains `capture.json`, `render-stats.json` and
`profile-counters.json`. Report the job IDs and settings with the
comparison. See the [benchmark reference](../../../docs/runtime/lab.md#--serve-and-bench)
for scale, pacing and LOD details.

## Record once and replay

On the playing Mac, use `h2 lab record` with the level set. Have the
player play the sequence and quit to finish the recording. The command
uses the local lab host if available, or records directly into the local
lab root. The recording contains `manifest.json` and `input.bin`.

```sh
h2 lab record --level "$lab_map" --name run-sample
```

From the driver, list host recordings and select one for a run. The
manifest supplies the level when you omit `--level`.

```sh
h2 lab recordings --host "$lab_host"
h2 lab run --name "Replay baseline" --host "$lab_host" --current --replay run-sample --frames 1800 \
  --wait --to "$lab_results"
```

Replay answers controller reads by index from a fresh boot. Treat
comparisons as approximate if the build or starting state can change
the sequence of reads. See [recording and replay](../../../docs/runtime/lab.md#recordings).

## Update the host

Update an installed host while idle. This fast-forwards the host's
tracked checkout, builds the lab CLI, and restarts the LaunchAgent.
Use `h2 lab host update` on the host itself for the same maintenance.

```sh
h2 lab status --host "$lab_host"
h2 lab update --host "$lab_host"
h2 lab status --host "$lab_host"
```
