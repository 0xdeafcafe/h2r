# Sim mode: parallel runs and A/B

Status: Built. Batches, arm swaps, Uplink watching and batches on hosts. Part of [Sim mode](sim.md).

## 7. Parallel runs, A/B and tuning

`h2 sim batch <batch file>` (`crates/apps/cli/src/sim_cmd.rs`):

```toml
out = "results/aggr-vs-base"   # results directory
parallel = 6                   # processes at once (default: cores / 2)
match = "matches/lockout-2v2.toml"
maps = ["lockout", "midship", "ascension"]   # overrides the match's map, one run each
seeds = [1, 2, 3, 4, 5]

[a]
name = "base"
params = "tuning/base.toml"
[b]
name = "aggressive"
params = "tuning/aggressive.toml"
```

- Each run is a child process, `h2 sim run`, with its own data directory
  (`<out>/runs/<id>/data`, its `bots/` nav meshes linked to the shared
  data's) and its own report, log and agent socket. Each combination of
  map, game type, skill and seed gets a run. IDs are
  `<map>[-<gametype>][-<skill>]-s<seed>`, with the game type and skill
  segments included when those axes are set. Separate mode appends the
  arm name. In either swap mode the second leg appends `-swapped`.
- **The map cache.** A run's data directory starts with Z:'s cache slots
  (`shared`, `single_player_shared`) and the title's `preferences.dat`
  cloned from the image's template in `<image out>/sim-cache/utility/`.
  N:'s slots (the main menu and the match's map) come from the same
  template: every boot formats N:, and the run's VFS clones them in
  straight after the format (`H2_SIM_CACHE_SEED`, set by the batch). A
  boot that finds them skips their copy, and the System Link lobby does
  not wait for slot 3. The first run of each map that misses a part of the
  template makes it while the batch's other runs of that map wait. Each
  run's status line says `warm Z:` or `cold Z:` and how many N: slots
  were seeded; `H2_SIM_CACHE_TEMPLATE=0` turns the whole template off
  ([cache-slots.md](cache-slots.md#sim-batches-a-warm-n)). On a light
  host the lobby's own background copy also runs several steps a frame
  (`map-copy.lobby`), which is what a cold run waits on.
- **Arms.** By default each run plays config A against config B in the
  same match: half the bots A, half B (2 v 2 with teams A and B in a team
  game; in free-for-all, alternating). That is the fairest comparison: the
  same map, seed, spawns and weapons for both. `mode = "separate"` runs
  every match twice, all bots A, then all bots B, and compares the two
  sets of matches.
- **Comparison** (`sim::compare`, tested): per arm, the mean and standard
  deviation over runs of kills, deaths, K/D, accuracy, damage, score,
  stuck time, the win rate, and the difference B - A with a 95% interval
  (Welch). When the reports have them, also the time to kill (`ttk_s`)
  and the aim motion (`aim_error`, `overshoot`, `settle_s`, `jitter`,
  `reversals`), each pooled over what it counts. `compare.json` and a
  text table in the results directory. `h2 sim compare <dir>` prints it
  again.
- **Game types.** Set `game_types = ["slayer", "ctf", "assault", "juggernaut"]`
  to run each listed type. CTF and Assault enable team play, Juggernaut
  disables it, and the other types use the match file's team-play setting.
  Omit the axis to use the match file's game type and team-play setting.
- **Skills.** `skills = ["easy", "normal", "heroic", "legendary"]` adds
  a run per skill and overrides each bot's skill. When the runs cover
  multiple skills, `compare.json` has `by_skill` and the table repeats
  per skill.
- **Detail.** `detail = "training"` or `"spatial"` in the batch file
  sets every run's `detail` (the match file's own is the default; see
  "Finding the first differing state" and [The spatial trace](#the-spatial-trace)).
  A spatial run writes `spatial.bin` beside its `report.json`.
- `h2 sim batch --dry-run` prints the runs and stops.

### Arm swaps

`mode = "arm_swap"` plays each map, game type, skill and seed twice.
The first leg is the versus split. The second (`<id>-swapped`) gives
every bot the other arm. Each bot keeps its seat, name, team, transport
and brain seed, so each arm plays every seat once. The run's skill
applies to both legs when the skill axis is set. Otherwise, an arm can
override the bot's skill through its tuning. Both legs feed the same
per-arm statistics. The mode needs two arms. The checked-in example
compares the two aim models at each skill on Midship and Lockout:

```sh
H2_OPT=2 h2 sim batch docs/runtime/sim-examples/aim-net-vs-code.toml --disc disc/halo2.iso --parallel 3 --pace unpaced
```

How to read it and the rule for which model stays are in
[bots.md §3.4.1](../game/bots.md#341-the-aim-net-compared-the-code-aim-stays).

### Watching in Uplink

```sh
h2 sim run docs/runtime/sim-examples/lockout-2v2.toml --upload
```

```sh
h2 sim batch docs/runtime/sim-examples/batch.toml --parallel 4 --upload
```

The commands use your stored Live account and its server. When multiple
accounts match, choose an admin with `--as GAMERTAG` and optionally narrow
the selection with `--server URL`. HTTPS and loopback HTTP are supported.
Use the same disc, image and nav meshes you use for local runs.

Sign into that server as an admin in h2 Uplink. Open **Halo Admin →
Clanker Training** and select a running match to watch it. The viewer follows
the live edge on the simulation's clock and shows its usual **Live** control
for returning to it after a seek. The live clock runs at the stream's
measured pace (`streamPace` in `web/src/state/frames.ts`): a sim streaming
at 3× plays smoothly at 3× near its live edge. A finished run's replay opens
at 8× and its speed menu goes from 0.25× to 16×. The training page shows
measured simulation speed. Use `--pace realtime` for normal game speed.

Above the results table, the page watches the runs it lists
(`web/src/admin/SimWatch.tsx`). With no batch picked, it draws them as
**One map**, described below. Pick a batch (the **Batch** filter, or a
batch heading in the list) to watch it as a grid with one map tile per
run, or switch the batch to **One map** with the **Grid** / **One map**
switch in the replay controls. The grid's tiles are ordered by map, then
seed, then tuning arms, so the arms played on one map and seed sit side by
side. Each tile shows the map, seed, arms by team, game clock and score,
and opens the run's page when clicked.

- **Finished runs** replay on one shared clock, in sync from each run's
  first frame. **Speed** picks 1×, 4×, 8× or 16× (8× to start). **Pause**,
  **Play** and **Restart** apply to all tiles. A run that ends early holds
  its last frame, and the clock stops when the longest run ends. Each tile
  downloads its full recording once it nears the screen, at most three at
  a time, and plays it in the browser, so speed needs no server support.
- **Running runs** stream live at the sim's own pace. Up to six on-screen
  tiles stream at once. The others show the latest score from the list,
  which refreshes every five seconds.
- **Deferred runs** say *Replay after match* until their recording is
  uploaded.

The maps are the stats site's small map renderers on the shared animation
frame loop. A tile off screen draws nothing.

**One map** draws every run on a map together in one large viewer, titled
with the map and its run count. A page that lists runs on several maps has
a viewer per map; the **Map** filter leaves one. The viewer uses the map's
top view when the server has one, else its outline. Every run plays on the
same clock and controls as the tiles, each at its own tick: finished runs
replay their recordings, and running runs stream live under the same six
slots, shared by the viewers on screen. Runs that wait for their recording
are left out, with a note.

Each run has its own colour from a fixed palette, in tile order, and its
players and death marks are drawn in that colour instead of the team's. The
viewer merges the runs' frames and rosters into one frame and one roster
(`mergeFrames` and `mergeRosters` in `web/src/admin/batchWatch.ts`). Each
run's players are keyed `slot * 1000 + index`, so players from different
runs never meet, and the merged roster gives them the run's colour. The
map renderers are the same as the tiles'.

A legend beside the viewer has a row per run with its colour, seed, arms,
live score and a link to the run. Pointing at a row, or at the run's row in
the results table, draws that run at full strength and dims the others
(their roster colour and marks turn translucent). Clicking a legend row
holds the highlight; clicking it again lets go.

Each batch streams at most two matches at once. Other running matches appear
in the list with replay available after the match. They buffer packed
telemetry locally and take turns uploading after they finish. The batch prints
its Live batch ID and writes it to `batch.json`.

The same game stream becomes the replay at the end. Uploads use the
game's telemetry frames, which include machine travel when sampled.
Uploaded players have bot flags and no Live account XUIDs.
The finished-game viewer switches to the archive when it is published.
[Replay playback](../live/replay-fidelity.md#current-recording-and-playback)

During a streamed simulation, the Clanker Training bot stats panel reads
the latest completed decision per bot roughly every five wall seconds.
It shows the current goal, weapon and target range, the last action,
grenade and melee use, weapon switches, mean attack reaction in ticks,
think time and weapon accuracy by firing range. The preview carries the
64 most-used weapon/range pairs per bot. The full breakdown and recent
decision history appear in the uploaded report for streamed and deferred
runs. The in-game agent exposes the current sample during play. If the
private upload exceeds 512 KiB, the host omits the per-tick determinism traces from the upload and records that in the notes.
The CLI saves the full local report, including when an upload fails. Training games are private
and excluded from public stats and ranks. The
[Live API](../live/live-api-theater-stats.md#clanker-training) describes the storage limits.

### Batches on hosts

```sh
h2 sim batch docs/runtime/sim-examples/batch.toml --provider cloud \
  --live https://live.example.net --max-parallel 4 --budget-usd 0.50
```

`--provider local` is the default. It runs the same child processes as
`h2 sim batch <file>`. `--max-parallel` also works as `--parallel` on
local batches. Local runs have zero estimated host cost.

Cloud batches use the Cloudflare or Fly provider configured on Live. The
CLI sends the self-contained match file, including the batch's seed, bot
seats and inline tuning, and Live starts one ad hoc host for each admitted
run. Each host plays unpaced with the provisioned game image and data,
streams to the private **Clanker Training** view, uploads its report and
exits. Use retail map file stems such as `lockout`. The host's data bundle
carries every multiplayer map's bot nav mesh and visibility grid, and the
host places them in the child's data root before the run, since a sim
never builds them.
[Game data bundle](../live/dedicated-host-ops.md#game-data-bundle)

`--live` defaults to `H2_LIVE_SERVER` or `http://127.0.0.1:8074`. Use a
stored admin account for that Live server, optionally selected by
`--as GAMERTAG`, or pass a fresh Live admin `games.write` JWT via `--token`
or `H2_LIVE_TOKEN`. Live exchanges the single-use JWT for a credential
limited to starting and polling this batch, which expires after 24 hours.
HTTPS and loopback HTTP are supported. `--dry-run` prints the plan without
contacting Live.

Cloud parallelism defaults to the batch file's `parallel` setting or four.
`--max-parallel` overrides that setting, up to Live's fleet-wide
`LIVE_MAX_HOSTS`. Live enforces its monthly budget separately. The CLI reserves the
estimated cost in plan order before each launch. At the first run that
would exceed `--budget-usd`, it stops queueing and waits for active runs.
The estimate uses the shorter of the configured game-time and tick limits
at 2.8× game speed, plus 60 seconds for setup and upload, capped by the host
lease's wall limit. With no positive game-time or tick limit it reserves
for the full lease. Small hosts (up to four bots) use default compute rates
of $0.090/hour on Cloudflare or $0.046/hour on Fly. Large hosts use twice the
Small rate. This estimates compute cost. Actual duration, storage and
transfer can change the bill. [Dedicated host cost assumptions](../live/dedicated-host.md#where-the-game-could-run)

As reports arrive, the CLI saves them to `<out>/runs/<id>/report.json` and
uses the same comparison code as local batches for `compare.json` and
`compare.txt`. The comparison lists budget-skipped and unsuccessful runs
as missing. `batch.json` carries the Live batch ID for Uplink. A failed
host makes the command exit unsuccessfully after it writes the comparison.
