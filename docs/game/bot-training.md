# Bot training: tuning the Clanker table from sim matches

Status: Design. Nothing here is built; it sits on `h2 sim batch` and the Clanker Training admin tab, which are.
Code: `crates/live/sim-bench` and `crates/apps/cli/src/sim_cmd`
provide match runners. The training search, service API and admin
page below describe proposed interfaces.
Switch: none. The design needs a per-process table-file override.

**Can the sim train the bots, fast, and show it on the Live admin? Yes.**
One Mac plays about 300 to 450 three-minute 2 v 2 matches an hour, which is
15 to 22 hours of play per hour ([Throughput](#throughput), an estimate
until the lab measures it). That is enough to fit the numbers the bots
already run on: a few tens of parameters per block, a day or two per block.
It is not enough to train a neural net from the controller up, and a net is
not what the bots need. What it buys: each difficulty lands where it is
meant to against people, the Clanker population plays like the human
population on measured habits, and a new table reaches every host from the
admin page without a release, with a rollback.

Training here means **calibration**. The search does not make bots
stronger; it moves each difficulty to a target strength and each habit to
the human distribution. The one rule: a trained table can never take a bot
outside the human-input limits (`bots::tuning::Tuning::input_limits`,
[bots.md §3.4](bots.md#34-aim)). Those limits are bounds of the search
space, not parameters in it.

## What is trained

A parametric model, the **Clanker table**: every number that today is a
`const` per difficulty, as one versioned file.

| Block | What it holds | Where it is a constant today | Size |
|---|---|---|---|
| Aim | per skill: reaction, error, lead, gain, fire cone, burst on and off, grenades a minute | `bots::mind::Skill::base_aim` | 4 x 8 |
| Awareness | per skill, 0..1 | `bots::decisions::awareness` | 4 |
| Trait bands | per skill, the low and high bound of each personality trait | `BANDS` in `crates/live/bots/src/decisions/personality.rs` | 4 x 9 x 2 |
| Try-hard table | the correlation of each continuous trait weight (radar use, noob combo, dual wield, aggression, grenades, melee, range and weapon preference, strafe and jump, reaction jitter, retreat) with the per-name try-hard score | the personality lane's correlation table (**S**: not in the tree; this block waits for it) | about 12 |
| Decision weights | the mood weights in `Behavior::drive`, the grenade rate multipliers, the patience and spacing scales | `crates/live/bots/src/decisions/personality.rs`, `crates/live/bots/src/mind/strategy.rs` | about 15 |

Not in the table: the human-input limits (perception and motor delay,
button timing, stick slew, thumb switch), the phantom latency and the work
intervals. The first are the rule above, the others are not behaviour.

The table is JSON, `h2-clanker-table/1`: `version`, `parent` (the version it
was derived from), `source` (`builtin`, `hand` or a training run id), the
blocks above and a sha256 over the canonical encoding. The built-in table
is the constants as they stand, compiled in
(`bots::table::Table::builtin()`), and a test holds that a brain reading
the built-in table plays the same pads as one reading the constants. A
table is loaded once per process and read where the constants are read; a
bot's own `[bot.tuning]` still overrides it
([sim.md, "Bot parameters from a file"](../runtime/sim.md#bot-parameters-from-a-file)).
Loading clamps every value to its declared range and refuses a file with an
unknown format, a bad hash or a band whose low bound is over its high
bound. A refused table is a named log line and the built-in table plays;
nothing is half applied.

### Why not a neural net

**Design**, for these reasons:

- **The budget.** A Mac produces a few thousand bot-ticks a second
  ([Throughput](#throughput)). Fitting sixty numbers takes thousands of
  matches. Learning to play from the pad takes many orders more, and the
  guest tick cannot be batched on a GPU.
- **The goal is not strength.** The targets are a win rate per difficulty
  and human habits. A table moves those directly; a net trained on reward
  finds the strongest play the limits allow and has to be held back.
- **It has been tried by hand.** The hand-weighted aim net lost to the code
  aim over 80 matches and the code aim stays
  ([bots.md §3.4.1](bots.md#341-the-aim-net-compared-the-code-aim-stays)).
- **A table can be read.** An admin sees that Normal's reaction went from
  10 to 11 ticks, diffs two versions and rolls one back. Each value has a
  range, so a bad run cannot ship a bot that turns faster than a thumb.
- **It ships as 4 KiB of JSON** through the title-file path, with no
  inference code and no new dependency in the game.

The harness does not exclude a net: `aim_model = "net"` is already an arm
(`bots::tuning::AimModel`), and its weights could become one more block
later. That is not planned.

## The search

`h2 sim train <train.toml>` runs a population search over one block at a
time. Blocks are searched separately because they are measured by different
terms and because the evaluation cost grows with the dimension: Aim and
Awareness per skill (9 numbers, four runs), then the bands and the try-hard
table together (the population's shape), then the decision weights.

- **Method.** CMA-ES with the standard population (4 + 3 ln n, rounded up
  to an even number: 12 for n = 9, 18 for n = 84) in a space where every
  parameter is scaled to 0..1 over its declared range. Sampled points
  outside the range are reflected back in. `method = "es"` selects a
  plain (mu, lambda) strategy with one step size, for debugging the
  harness. Both are a few hundred lines of pure Rust in `sim::train::search`
  with no dependency; the tests fit known functions.
- **One evaluation** of a candidate is a set of paired matches against the
  reference pool on the generation's maps, game types and seeds, in
  `arm_swap` mode ([sim.md §7](../runtime/sim.md#7-parallel-runs-ab-and-tuning)):
  the candidate plays every seat once and the reference plays the other
  seats, so spawns and weapons are the same for both.
- **Common random numbers.** Every candidate in a generation plays the same
  maps, types and seeds. Differences between candidates are then
  differences in the table, not in the draw.
- **Noise.** A match is a noisy sample. The run starts at 12 pairings per
  candidate (24 matches) and the trainer doubles it, up to a cap, when the
  best and median candidates' intervals overlap for three generations
  (`sim::compare`'s Welch interval).
- **Stopping.** A generation limit, a wall limit, a budget in matches, or
  the step size under a floor.

### Determinism and seeds

A training run is defined by its file and one `seed`. From it, SplitMix64
(the function `sim::config::bot_seed` uses) derives the sampler's stream
and each generation's match seeds; each match seed is folded into each
brain's seed as in any sim match. The trainer writes `trainer.json` after
each generation (the distribution, the step size, the generation's seeds,
every candidate and its fitness), so a run resumes from its last complete
generation and never replays a finished match.

The search needs repeatable enough, not bit-exact: a fitness is a mean over
matches. The audit trail needs exact. Each evaluation records the image id,
the `h2` version, `H2_OPT`, the table hash, the match seed and the report's
last `state_hashes_v1` digest, so any match can be replayed and compared
with `h2 sim diff`. Same-build repeatability is open in
[sim.md, "Not verified"](../runtime/sim.md#not-verified); the first slice
checks it on its own matches before any fitness is trusted.

### Not fitting one map

- **Pools.** The run file names a training pool and a holdout pool of maps
  and game types. A candidate never plays a holdout map during the search.
- **Rotation.** Each generation draws its maps and types from the training
  pool, stratified (one small, one mid, one large map; Slayer and one
  objective type), and fresh seeds. No seed is used in two generations.
- **Spread counts.** A candidate's score is its mean over maps plus a share
  of its worst map, so one that is right on Lockout and wrong on Zanzibar
  loses to one that is nearly right on both.
- **The gate.** When the search stops, the best candidate and the reference
  each play the holdout pool on fresh seeds. A candidate whose holdout
  fitness is worse than its training fitness by more than the run's
  tolerance is marked `overfit` and cannot be promoted.
- **Vehicle maps count.** Per-tick cost differs by more than two times
  between maps ([sim-budget.md](../perf/sim-budget.md)), so a budget in
  matches would starve the large maps. The planner budgets in ticks.

## Fitness

A candidate's fitness is a cost, lower is better, the weighted sum of four
terms. Each term is kept beside the sum in every record, so the admin page
plots them separately.

### Strength against a frozen pool

The reference pool is frozen when the run starts: the active table at each
of the four skills, plus the tables promoted before it (so a new table is
not tuned only against its parent). From the paired matches the trainer
takes the candidate's win rate and K/D against each reference
(`sim::compare`).

The targets are a ladder, in the run file (**Design**; the defaults below
are starting points to play against):

| Candidate skill | Against the frozen table at | Target win rate |
|---|---|---|
| Easy | Normal | 0.25 |
| Normal | Easy / Heroic | 0.75 / 0.25 |
| Heroic | Normal / Legendary | 0.75 / 0.30 |
| Legendary | Heroic | 0.70 |

The term is the squared distance from the targets. A candidate that beats
the target is as wrong as one that misses it.

### Calibration against people

"Normal loses to a mid-rank player about 65% of the time" cannot be played
in a sim, because there is no person in one. It is reached in two steps.

1. **Offline, by measured output.** The human reference (below) holds, per
   rank band, the distributions of what a player's telemetry shows:
   accuracy per weapon class, time to kill, damage per life, deaths per
   minute. Each skill has a target band (**Design**: Easy the 20th
   percentile, Normal the 50th, Heroic the 75th, Legendary the 95th) and
   the term is the distance of the bot's output from that band.
2. **Online, by results.** Clankermatch and custom games already record
   human-against-bot games with `bot: true` rows
   ([bots.md §9.2](bots.md#92-results-host-to-server)). The Live server
   folds them into a win rate per table version and skill for players in
   the mid rank band. The admin page shows it beside the target
   (**Design**: a mid-rank player beats Easy 85%, Normal 65%, Heroic 50%,
   Legendary 30% of the time in a 1 v 1 share of kills). When the measured
   rate is off, the next run's percentile for that skill moves; the search
   never reads live results directly.

### Human-likeness

Distributions, not means: for each metric the term is the distance (the
1-D earth mover's distance over a fixed histogram, divided by the human
spread) between the bot population's histogram and the human one. The
population is the candidate table's bots under many names, since the bands
and the try-hard table only show across names.

| Metric | From | Exists |
|---|---|---|
| Time to kill, per weapon class | hits and the kill log | mean only: `ttk_ticks` ([sim.md §6](../runtime/sim.md#6-metrics)); needs the histogram |
| Accuracy, per weapon class | shots and hits | overall only; needs the class split |
| Grenades a minute, and a minute of fighting | the carried count falling | yes: `grenades` |
| Aim motion: flick overshoot, settle time, stick jitter, reversals | the right stick and the view | yes for bots: the report's `aim` block; needs a view-only version for people |
| Radar-driven turns: a turn of 60 degrees or more toward an enemy inside radar range with no line of sight | view, positions, visibility | no |
| Noob combo rate: a charged plasma shot that drops a shield, then a precision headshot kill inside 2 s | weapon, shots, damage, the kill log | no |
| Dual-wield time, melee kills share, weapon-class time share | the held weapons, the kill log | partly: `doing` and the kill log |
| Strafe reversals and jumps per fight second | velocity and the airborne flag | no |
| Retreats: leaving a fight under half shields and living 5 s | vitality, positions | no |

**One extractor, both sides.** People are only visible through telemetry:
the packed frames (view, weapon, shots, velocity, seat and vitality at 15
Hz, [live-telemetry.md](../live/live-telemetry.md)) and the game's events.
`sim::habits` computes every metric above from a frame stream and its
events, and nothing else. Bot matches go through it by their own telemetry
frames (the same frames `--upload` sends, taken in process), not through
the brain's private counters, so a bot is measured exactly as a person is.
The existing `aim` block stays as the brain's view for debugging.

**The human reference** is `h2-human-reference/1`: per map size, game type
and rank band, one histogram per metric and the count of games and lives
behind it. `h2 live training reference build` runs the extractor over the
server's archived human games (no sim runs, no bots, at least 200 lives per
cell or the cell is left out) and stores the result on that server. It
holds histograms only: no xuid, no gamertag, no game id. It is service
data and is never committed. Retail films are a second source through the
film reader (`crates/live/film`) once it yields the same frame fields
(**S**: which fields a film gives is not established).

### Robotic tells

Penalties, each zero for play inside the human range and growing outside
it. They catch what distributions miss, a habit no person has at all:

- a flick with no overshoot and no settle time, or stick jitter at zero;
- a first input sooner after first sight than the reaction floor;
- the view tracking an enemy through a wall for more than a few ticks;
- a turn toward an enemy who is out of sight and outside radar range;
- shots at equal intervals (a cadence variance under the human floor);
- accuracy or headshot share over the human 99th percentile for the class;
- time standing still in the open, `stuck_ticks` and repaths over a cap;
- the same route from the same spawn in more than a share of lives.

A candidate over a hard cap on any tell (the run file sets them) is
infeasible: it gets the worst fitness of its generation and is never the
best candidate.

## Throughput

**The inputs** (measured, in the notes): a four-bot sim process takes 8.5
ms a tick on Lockout and 14.1 on Waterworks, and 10.4 to 19.5 across twelve
retail maps ([sim-budget.md](../perf/sim-budget.md),
[sim.md, "Bot navigation performance"](../runtime/sim-determinism.md#bot-navigation-performance)).
A tick is 33.3 ms of play, so one process runs 1.7 to 3.9 times real time.
One process dirties about 143 MiB; the rest of its 981 MiB peak is the
shared disc image and code
([notes/sim.md](../runtime/notes/sim.md)). The 4.58 ms a tick and 7.28
times in [sim.md §6](../runtime/sim.md#6-metrics) are the report example,
not a measurement with a note; nothing here is sized from them.

**The arithmetic** (**S**, until the first slice's lab note replaces it):
brains think on the game thread by default, so a process uses one core.
The memory bound is far off (eight processes dirty about 1.2 GiB), so the
worker count is the performance cores less two. A match costs its ticks
plus the boot, lobby and load, taken as 20 s.

| Match | ms/tick | One process | Eight processes |
|---|---|---|---|
| 5,400 ticks (3 min) | 8.5 | 55 an hour | 440 an hour |
| 5,400 ticks (3 min) | 14.1 | 38 an hour | 300 an hour |
| 1,800 ticks (1 min) | 8.5 | 102 an hour | 820 an hour |

A generation of 12 candidates at 24 matches each is 288 matches: under an
hour on one Mac, and a nine-number block converges in a few tens of
generations. Numbers for a design decision come from the quiet laptop
through `h2 lab` ([lab.md](../runtime/lab.md)), as a dated note in
`docs/game/notes/`, with `wall.speedup` from the reports and `rss_bytes`
and `cpu_percent` from [`host.metrics`](../runtime/agent-api-host.md#hostmetrics).

### What is reused

| Need | Existing piece |
|---|---|
| One match, unpaced, no render, a report | `h2 sim run`, `render_every = 0`, `max_ticks`, `sim::metrics::Tally` |
| Runs over maps, types, skills and seeds; a child process, data directory and report per run | `h2 sim batch`, `crates/apps/cli/src/sim_cmd.rs` |
| A fair pairing | `mode = "arm_swap"` |
| Means, intervals, the difference of two arms | `sim::compare` |
| Tuning without a rebuild | `sim::config::BotParams`, `bots::tuning::Tuning` |
| Hosts beyond one Mac, with a budget | `h2 sim batch --provider cloud`, `sim::batch::Schedule` ([sim.md, "Batches on hosts"](../runtime/sim-parallel.md#batches-on-hosts)) |
| A match to watch | `--upload` and the Clanker Training viewer |
| The first tick two runs differ | `h2 sim diff` |

### What is missing

- **Many arms through one pool.** A batch has two arms. A sweep or a
  generation is many two-arm batches (candidate against reference) that
  must share one worker pool and one results tree. `sim_cmd` gets a planner
  that takes a list of batch plans and a pool, and the batch command
  becomes the one-plan case.
- **The table as an input.** `clanker_table = "path"` in a match or batch
  file, and per arm, beside `params`. The remaining traits (composure,
  patience, showboating, loyalty, chattiness) and the try-hard table are
  reachable only through it.
- **A budget in ticks**, and `max_ticks` set by the planner per map size.
- **Histograms in the report.** `h2-sim-report/1` gets an additive `habits`
  block from `sim::habits`.
- **More than one match per boot.** Every run boots a fresh System Link
  host, and snapshots cannot seed a batch
  ([sim.md §6.2](../runtime/sim-metrics.md#62-sharing-guest-ram-with-machine-snapshots)).
  With one-minute matches the boot is over half the cost. The host returns
  to the lobby after the end latch and starts the next match of a list in
  the same process, a report each. This is the largest single gain and has
  its own slice.
- **A lab entry point.** `h2 lab` starts levels, not sim batches. It gets
  `h2 lab sim <batch file>`, which runs the batch on the laptop and fetches
  the results directory.

### Making a match cheaper

In the order they pay: play shorter matches by tick limit (the search needs
many fights, not finished games; a score limit ends too few of them); keep
several matches per boot (above); use the sim stores so no process pages
the disc ([sim.md §6.1](../runtime/sim.md#61-sim-stores-the-map-without-the-map));
play 2 v 2 on the small maps and keep larger teams for the holdout gate
(cost grows with seats); leave `render_every` at 0, uploads deferred and
the arena hash off. `H2_BOT_THREADS` moves thinking off the game thread but
a full pool has no idle core for it, so training keeps it serial. Each
native subsystem of the sim rewrite
([sim-rewrite.md](../engine/sim-rewrite.md)) lowers ms/tick for every row
of the table above with no change here.

## Live admin

The **Clanker Training** tab of the Halo Admin
(`/games/halo2/admin?tab=training`, `web/src/admin/ClankerTraining.tsx`)
keeps its list of matches and gets three more views, picked by a row of
buttons under the title: **Matches** (what is there), **Sweeps**,
**Training** and **Tables**. They use the admin's panels, rows and buttons
(`Panel`, `adm-row`, `adm-field`, `s-btn`) and its keyboard rules. Charts
are plain SVG drawn from pure, tested shaping functions, as the usage
dashboard does (`web/src/deck/chart.ts`): no chart library.

```
+- Clanker Training ----------------------------------------- [Refresh] -+
| [Matches] [Sweeps] [Training] [Tables]                                 |
+------------------------------------------------------------------------+
| Run            Block          Gen     Best    Holdout   Status         |
| aim-normal-3   aim.normal     31/60   0.142   -         running  6.1x  |
| bands-2        bands+tryhard  80/80   0.210   0.223     finished       |
| aim-heroic-1   aim.heroic     44/60   0.305   0.512     overfit        |
+- aim-normal-3 ---------------------------------------------------------+
| Fitness by generation            | Terms (best candidate)              |
|  0.6 |.                          |  strength     0.031  ####           |
|      | '.                        |  calibration  0.048  ######         |
|  0.3 |   '-.__  best             |  human-like   0.052  #######        |
|      |        '--.___ median     |  tells        0.011  #              |
|  0.0 +----------------- 31       |  matches 8,928   game time 446 h    |
+------------------------------------------------------------------------+
| Parameter     Active   Best    Range        Population (this gen)      |
| reaction      10       11      5..18        . .:|:. .                  |
| error         0.060    0.071   0.01..0.15     .:|::.                   |
| lead          0.60     0.55    0..1         ..::|:.                    |
+------------------------------------------------------------------------+
| Bots against people             bots [####  ]   people [  ####]        |
| time to kill, precision    distance 0.21   noob combo rate    0.08     |
| radar-driven turns         distance 0.34   grenades a minute  0.12     |
+------------------------------------------------------------------------+
| Best profiles: gen 31 #4 (0.142)  gen 29 #1 (0.149)  gen 31 #9 (0.151) |
| [Open sample match]  [Diff against active]  [Save as table v8]         |
+------------------------------------------------------------------------+
```

- **Sweeps.** One row per sweep; a sweep opens to its points and, per swept
  parameter, a response curve: the chosen metric against the parameter's
  value with its 95% interval, the base value marked.
- **Training.** The run list; a run opens to the fitness curves (best,
  median and holdout by generation), the four terms, each parameter's
  active and best value with the generation's population as a strip, the
  per-trait distributions (the bot population's histogram over the human
  one for each habit), and the best profiles. A profile links to its sample
  match in the existing viewer and to a diff against the active table.
- **Tables.** Every table version: its source, parent, hash, holdout
  result, the measured win rate of people against it per skill, and which
  one is active. **Promote** makes a version active after a confirmation
  that shows the diff. **Roll back** is Promote on an earlier version.
  Promote is disabled for a table marked `overfit` or from an unfinished
  run, and an admin can upload a hand-edited table here.

### Data flow

```
h2 sim train --live URL                    Live (native or Worker)
  children: h2 sim run, no Live  ---.
  trainer:  fitness, next generation |      POST /v1/admin/training/runs
            one summary a generation '--->  PUT  .../runs/{id}/generations/{n}
            one sample match (--upload) -->  the existing sim-run stream
admin browser  <----------------------------  GET  runs, generations, tables
admin browser  -- titles.write ------------>  POST /v1/admin/clanker-tables/{v}/activate
every host at sign-in  <--------------------  GET  /v1/titles/{title}/files/clankers.json
```

| Endpoint | Token | Contract |
|---|---|---|
| `POST /v1/admin/training/runs` | `training.write` | Creates a run from its file (block, ranges, targets, pools, seed), `kind` `sweep` or `train`. Returns the run id. The same id and body can retry. |
| `PUT /v1/admin/training/runs/{id}/generations/{n}` | `training.write` | One generation: each candidate's parameters, four terms, sum and match count; the distribution's mean and step; the habit histograms of the best candidate; the sim batch id of the sample match. An identical retry succeeds; a different body for a stored generation is `409`. A sweep stores its points as generation 0. |
| `PUT /v1/admin/training/runs/{id}/result` | `training.write` | The end: the reason, the best candidate, its holdout terms and its table. Closes the run. |
| `GET /v1/admin/training/runs`, `.../{id}`, `.../{id}/generations?from=` | `read`, admin | The lists the page draws. Newest first, 50 a page. |
| `GET /v1/admin/clanker-tables`, `.../{version}` | `read`, admin | Versions and one table. |
| `POST /v1/admin/clanker-tables` | `titles.write`, admin | Stores a table from a finished run's result or an uploaded file. The server validates it with the game's own loader (`bots::table`) and assigns the next version. |
| `POST /v1/admin/clanker-tables/{version}/activate` | `titles.write`, admin | Promote or roll back. Writes `ClankerTableActivated` and publishes the table as the title file `clankers.json`. |
| `GET`, `PUT /v1/admin/training/human-reference` | `read` / `titles.write`, admin | The human reference. |

Tables and activations are events in the log (`ClankerTableStored`,
`ClankerTableActivated`), so the history, the author of each promotion and
the rollback path survive a restart and `h2 live store copy`. Generations
and the human reference are blobs under `training/` in the private store:
the native server's blob store, the Worker's `BLOBS` R2 binding, as sim
reports are ([live-api.md, "Clanker Training"](../live/live-api-theater-stats.md#clanker-training)).
The handlers live in `live-server` and both homes mount them, so there is
no Worker-only code. The Worker never runs a match: the trainer runs on a
Mac or on per-match hosts and only posts.

**The game's side.** Whoever hosts runs the bots
([bots.md §9](bots.md#9-the-server-interface-for-sbmm-ranked)), so every
host needs the table. At Live sign-in the host fetches `clankers.json`
with the other title files (`crates/live/live-hle/src/client.rs`, `fetch`),
keeps it in its data directory with the `ETag`, and loads it at the next
map load, never during a match. Offline, System Link and a failed fetch
play the cached table, else the built-in one; the log says which table
version a match played, and the game record carries it so results can be
grouped by table. A roster's `skill` and `personality.seed`
([bots.md §9.1](bots.md#91-the-roster-server-to-host)) are unchanged: the
table decides what a skill and a seed mean.

## Rules and limits

- **Sim processes do not talk to Live.** A training child is `h2 sim run`
  with no upload: it makes no account, no session and no game
  ([agent-api.md, "Live"](../runtime/agent-api-sim.md#live)). Only the trainer
  posts, and only summaries. The one sample match per generation goes
  through the existing `--upload` path, which is private, flags every
  player as a bot with no account xuid, and is excluded from public lists,
  stats and ranks.
- **The trainer cannot promote.** `training.write` is a scope of its own.
  The CLI exchanges a stored admin account's single-use token for a
  credential limited to creating runs and writing the runs it created,
  expiring after 24 hours, as the cloud batch does. It cannot store or
  activate a table, read accounts or write games. Promotion needs
  `titles.write` signed in an admin's browser or CLI.
- **The server target is explicit.** `--live` has no default for
  `h2 sim train` and `h2 sim sweep`; without it the run is local and the
  page shows nothing. HTTPS and loopback HTTP only.
- **Size limits**, the same in both homes: a generation body 256 KiB; 64
  candidates a generation; 256 parameters; 2,000 generations a run; a table
  64 KiB; the human reference 1 MiB; 20 open runs a server; one sample
  match per generation, under the sim-run archive caps. A run idle for 24
  hours is closed as `interrupted`. Finished runs keep their generations
  for 90 days and their result for good.
- **A table is validated twice**, by the server when stored and by the
  host when loaded, with the same loader. Every value is clamped to its
  range, and no table field can move a human-input limit.
- **Human data stays aggregate.** The reference holds histograms with a
  minimum cell size and no identifiers. It is never committed to the
  repository, and neither is anything read from the disc.
- **Hosts are trusted as they already are.** A host that ignores the table
  runs different bots, as a host that ignores the roster's skill does
  ([bots.md §9.3](bots.md#93-trust)).

## Slices

Each slice ships something an admin can use. Lanes write code and tests;
the acceptance runs are the coordinator's.

1. **Sweeps, and a report on the admin.** `h2 sim sweep <sweep.toml>`:
   a grid or a random sample over named `BotParams` fields, each point a
   two-arm `arm_swap` batch against the base, all through the shared pool;
   `sweep.json` (`h2-sim-sweep/1`); the run and generation-0 endpoints; the
   Sweeps view with response curves; the mock server's fixture. No change
   to the bots. *Accept:* planner and aggregation tests on synthetic
   reports; the server test passes against the native store and the Worker
   smoke (`crates/live/live-worker/smoke/smoke.ts`) reads a posted sweep;
   a sweep of `error` over five values on Lockout and Midship shows
   accuracy falling as `error` rises, on the page; two runs of one match
   file give equal `state_hashes_v1`, or the difference is recorded as a
   finding against the determinism claim.
2. **The table as data.** `h2-clanker-table/1`, `bots::table`, the
   constants read through it, `clanker_table` in match and batch files and
   per arm, selected by a table-file override. The try-hard block joins when the
   personality lane lands. *Accept:* the built-in table reproduces the
   constants' pads over the existing brain tests; a table outside its
   ranges is clamped or refused by name; an A/B batch of two table files
   runs.
3. **Tables on Live.** Store, list, activate, the title file, the host's
   fetch and cache, the Tables view with diff, Promote and Roll back, the
   table version in the game record. *Accept:* store and activation tests
   in both homes; a host test over a fake server loads a promoted table at
   the next map load and the built-in one after a refused file; activating
   an earlier version restores its bytes exactly.
4. **Habits and the human reference.** `sim::habits` over telemetry
   frames, the `habits` block, the new metrics, `h2 live training reference
   build`, the bots-against-people histograms on the page. *Accept:*
   extractor tests on synthetic frame streams, one per metric, including
   one stream scored identically whether it is labelled bot or human; the
   reference build refuses cells under the minimum; the page shows where
   the active table differs from people, before any search exists.
5. **The trainer.** `sim::train` (the space, CMA-ES, the fitness and its
   terms, the checkpoint), `h2 sim train`, the generation and result
   endpoints, the Training view. *Accept:* the search fits test functions
   and resumes from a checkpoint to the same result; fitness tests on
   synthetic reports (over-target strength costs as much as under); an Aim
   run for Normal lowers the holdout fitness against the built-in table and
   its result can be saved and promoted from the page.
6. **Throughput.** Several matches per boot, the budget in ticks,
   `h2 lab sim`, `--provider cloud` for sweeps and training. *Accept:*
   lobby-return tests over the fake guest; a lab note in
   `docs/game/notes/bot-training.md` with matches an hour, ms/tick, RSS and
   CPU per worker count, replacing the estimate above.
7. **The loop with people.** The server's win rate of people against each
   table version and skill, shown against the targets, and the percentile
   adjustment for the next run. *Accept:* fold tests over synthetic game
   records; sim runs and bot-only games never enter the rate.

## Evidence

- **Code**: the tuning surface is `bots::tuning::Tuning`
  (`crates/live/bots/src/tuning.rs`); the bands are `BANDS`
  (`crates/live/bots/src/decisions/personality.rs`); the aim rows are
  `Skill::base_aim` (`crates/live/bots/src/mind.rs`); the batch, the arms
  and the cloud schedule are `crates/apps/cli/src/sim_cmd.rs` and
  `crates/live/sim-bench/src/batch.rs`; the sim-run API and its caps are
  `crates/live/live-server/src/sim_runs.rs`; the tab is
  `web/src/admin/ClankerTraining.tsx` and `web/src/admin/tabs.ts`.
- **Doc**: ms/tick per map, [sim-budget.md](../perf/sim-budget.md); memory
  of one process, [notes/sim.md](../runtime/notes/sim.md); the aim net
  result, [bots.md §3.4.1](bots.md#341-the-aim-net-compared-the-code-aim-stays).
- **S**: every number in [Throughput](#throughput)'s table; the 20 s setup;
  that a telemetry frame stream at 15 Hz resolves each habit metric (the
  extractor's tests on real captures settle it); the try-hard table's
  shape; that a retail film yields the frame fields.
- **Design**: the targets, percentiles, caps, limits and the choice of
  CMA-ES.

## Not verified

- Same-build determinism of a sim match, which the audit trail relies on.
- That the host can return to the lobby and start a second match in one
  process with phantom peers.
- That bot habits measured from 15 Hz frames match the same habits measured
  per tick closely enough to compare with people.
- That enough human games are archived per map size, game type and rank
  band to fill the reference.
- That fitting to the offline percentile bands moves the measured win rate
  of people toward its target.
- The per-machine throughput, on the quiet laptop.

## Related

- [bots.md](bots.md): the brain, the traits and the roster.
- [sim.md](../runtime/sim.md): sim mode, match and batch files, reports.
- [agent-api.md](../runtime/agent-api.md): `match.status`, `match.report`, `host.metrics`.
- [live-admin.md](../live/live-admin.md), [live-api.md](../live/live-api.md), [web-app.md](../live/web-app.md): the admin role, the API and the page.
- [live-telemetry.md](../live/live-telemetry.md): the frames the habits are read from.
- [sim-rewrite.md](../engine/sim-rewrite.md): the native sim that lowers ms/tick.
