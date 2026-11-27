# Sim mode: bounce practice and tuning

Status: Built. The bounce drill, sweeps, site training and seam drops. Part of [Sim mode](sim.md).

### Bounce practice

A match with `[rules] script = "superbounce"` is a practice: every bot practises
superbounces ([bots.md, "Practising bounces"](../game/bots.md#practising-bounces))
and nobody fights. `sim_glue::Sim::add_bots` puts the built-in Superbounce script's block
on the game's record at its start (`game_variants::inject::set_sim_block`),
so the script turns the superbounce mode on, installs the pair reader the
route's prime reads (`sim_bots::install_pair_reader`), keeps the medal
chain's awards for the sim (`superbounce_mode::collect_awards`) and turns
the tally's bounce counting on. [sim-examples/superbounce-practice.toml](sim-examples/superbounce-practice.toml)
is four legendary bots on Ascension for 18000 ticks (ten game minutes),
unpaced and unrendered:

```sh
h2 sim run docs/runtime/sim-examples/superbounce-practice.toml
h2 sim batch docs/runtime/sim-examples/superbounce-batch.toml --parallel 4
```

The map needs its nav mesh and its spot scan in `--data`'s `bots/`:
the run makes both before it starts, as `h2 bots nav <map>` does (the
scan is `<map>.spots`, beside the mesh; see "The nav mesh" below). The
cached nav load
(`bots::autonav::load_cached`) publishes the cached scan for the
practice to pick sites from.

The tally takes three things (`sim::bounce`, pure, tested):

- the bots' practice events (`bots::mind::PracticeEvent`, drained by
  `Metered` each tick): a route started, its word reached −3 (read or
  assumed; at a crouch spot or already primed), the try ended (bounced as
  the route saw it, or why not);
- the medal chain's awards (`superbounce_chain::Award`, the ones the HUD
  and Live get): a bounce with its count in the chain, and a Bounced
  death, ending a chain;
- every player's height each tick, for the launch: from the lowest of the
  last four heights before a chain bounce to the top of the flight
  (closed 0.1 under the top, or 150 ticks after).

A try is **bounced** when the chain filed a bounce during it; the
**bounce rate** is bounced tries over tries. The report gets a `bounce`
block (`h2-sim-bounce/1`), the totals, a line per bot and a line per
site (the stand point the try was at):

```json
"bounce": {
  "format": "h2-sim-bounce/1",
  "totals": { "attempts": 40, "primes": 31, "primes_read": 29, "armed": 0,
              "practice_bounces": 14, "bounced_tries": 12, "bounce_rate": 0.3,
              "chain_bounces": 17, "chains": { "1": 8, "2": 3 }, "best_chain": 3,
              "bounced_deaths": 1, "launch": { "n": 17, "best": 6.1, "mean": 4.2 },
              "why": { "too_slow": 9, "prime_failed": 6, "missed": 5 },
              "habits": { "repeats": 2, "distinct_sites": 14, "fumbles": 3,
                          "fumble_bounces": 0, "retries": 2, "monster_tries": 5,
                          "monster_bounces": 0, "chain_links_planned": 9,
                          "chain_links_tried": 4, "chain_links_made": 0, "yields": 1,
                          "yields_given_up": 0, "yield_ticks": 64, "stepped_aside": 3 } },
  "players": [ { "index": 0, "seat": 0, "name": "Bouncer 1", "arm": "base",
                 "first_bounce_ticks": 812, "stats": { "...": "..." } } ],
  "sites": [ { "site": [12.1, -3.4, 2.0], "stats": { "...": "..." } } ]
}
```

(The numbers above show the shape, not a measurement.) `why` uses the
practice's names: the route's errors (`prime_failed`, `pair_lost`,
`no_route`, `no_drop`, `no_bounce`, `timeout`, `stalled`, and the walk's
`no_path`, `goal_off_mesh`, `tail_stuck` and `path_lost_<check>_<leg>`)
and the landing's results (`not_primed`, `missed`, `too_slow`). A walk's
errors are `nav::RouteWalk`'s: `no_path` (no corridor without a
teleporter or crouch-jump link), `goal_off_mesh` (the goal farther off
the mesh than a last stretch reaches), `tail_stuck` (the last stretch to
a goal off the mesh stopped getting nearer; a practice's walks end there
instead, so it is not a practice reason in practice), and a walk the
follower gave up on, planned again three times first: `path_lost_` and
the check (`stuck`, `fell`, `jump`, `link`, `ended`) and the leg
(`crouch`: to the crouch spot, `approach`: to the bounce's approach or
launch floor, `nav`: another walk), such as `path_lost_stuck_approach`
([bots.md, "Superbounce routes"](../game/bots.md#superbounce-routes)).
The practice tries only sites a route reaches (a site paired with a
crouch spot a walk reaches it from, and a walk from the bot to that
crouch spot), so a site out of reach adds no try and no reason;
`no_route` is a route that failed to plan from where the bot stood.
`first_bounce_ticks` counts from the match's start to the bot's first
chain bounce. The block is absent from a match without the Superbounce script.

`habits` is how the bots practised (`sim_bench::bounce::Habits`, from
the practice's own events; [bots.md, "How each bot
practises"](../game/bots.md#how-each-bot-practises)): `repeats`, tries
at the same site as the bot's try before; `distinct_sites`, the sites a
bot tried (in the totals, summed over the bots); `fumbles`, tries set
up with a fumble, and `fumble_bounces`, those the chain still filed a
bounce in; `retries`, tries going straight back to a site just missed;
`monster_tries` at a Monster site and `monster_bounces` the chain filed
as Monsters; `chain_links_planned`, `chain_links_tried` (in the air off
a launch) and `chain_links_made` (launched again); `yields`, times a bot
let a player go first, `yields_given_up` of them, and `yield_ticks`
waited; `stepped_aside`, steps out of a player's way between tries.

A batch's comparison carries them per arm (`compare.json`'s `bounce`:
the runs, the totals and each map's, pooled over every bot) and the
columns `attempts`, `bounce_rate`, `primes`, `chain_bounces`,
`launch_mean`, `launch_best`, `bounced_deaths` and `first_bounce_s` in
its arm table. `compare.txt` adds a `bounce practice` table: per arm and
map, tries, bounced, rate, primes, chain bounces, launch mean/best,
Bounced deaths and the top three reasons.

**Proofs and ghosts.** In sim mode boot sets the verified directory to
the run's data `bots/` (`bots::spots::set_verified_dir`, which the nav
worker sets in a game), so `<map>.verified` and `<map>.ghosts` are
written as in a game. A superbounce batch run plays on copies of them:
its `bots/` is a directory of its own, each shared file linked but those
two, which are copied in (`sim_cmd::make_bounce_data`). So no run sees
another's proofs (a proven site weighs more in the practice's pick) and
every run of a sweep starts alike. The batch keeps
`<out>/runs/<id>/harvest/start` (what the run began with) and
`harvest/end` (what it ended with), even when it removes the data.
`h2 sim harvest <results dir> [--data DIR] [--dry-run]` merges what each
run added into `--data`'s `bots/` (`bots::harvest::harvest_run`): the
verified entries it changed, merged as the game merges them, and every
ghost run the shared file does not beat. A single `h2 sim run` writes to
`--data`'s `bots/` itself.

### The bounce drill

A practice match with a `[drill]` tries one site, each try from a start
near it with the word already at −3, so a run shows the final approach
and the jump alone, apart from priming and the walk there:

```toml
[rules]
script = "superbounce"

[drill]
site = [-10.62, 3.88, -3.19]   # the known site within 0.5 of this
start = [-8.4, 2.5, -0.76]     # optional; unset: the site's approach
```

`sim::config::DrillConfig` (tested) refuses a drill without
`[rules] script = "superbounce"`. `sim_glue::Sim::add_bots` hands it to the bots
(`bots::drill::set`); each practising bot walks to the start, and
standing there unprimed asks for its word
([bots.md, "Practising bounces"](../game/bots.md#practising-bounces),
"The drill"). A `before` on the game tick (`sim: drill word`) takes the
asks (`bots::drill::take_arm_requests`) and writes −3 over each unit's
level pair word (`live_telemetry::havok_agents::set_pair_word` on the
pair `fixed_pair` finds), the write `havok.pair.set` makes
([agent-api.md](agent-api.md#havokpairset)); a unit with no single
level pair (asleep) is asked for again. The notes say `drill: the word
was set to -3 at the start` once it has been. The tries go into the
`bounce` block as any practice's; `primes` and `primes_read` stay 0 (they
count primes at a crouch spot, and a drill's start is not one), and
`armed` counts the tries that began with the word read at −3, so a
drill's `armed` is its tries that got the word. A try that fails here failed in the approach or the jump:
`pair_lost` is the word read as something else on the ground between the
start and the jump.

What to read in the run's log: `bots: bounce drill: site ...` once the
bot has found the site (its aim and the start; a `no known site` before
it is the spots still coming in), and a `bots: bounce try:` line per try
with the last jump's takeoff, ground and vertical speed, word, air
stick, touchdown and result
([bots.md, "Practising bounces"](../game/bots.md#practising-bounces),
"The try's line").

[sim-examples/superbounce-ground-truth.toml](sim-examples/superbounce-ground-truth.toml)
is one bot on Ascension drilling the measured spot from its takeoff, the
recipe's running jump ([superbounce.md](../game/superbounce.md), "The
forced crouch leaves state"), for 9000 ticks:

```sh
h2 sim run docs/runtime/sim-examples/superbounce-ground-truth.toml
```

### Sweeping the bounce tuning

`h2 sim sweep <sweep file>` (`sim::sweep`, pure and tested; the CLI runs
the batches) plays one batch per candidate `[bounce]` tuning and ranks
them:

```toml
out = "results/bounce-sweep"      # relative to this file
batch = "superbounce-batch.toml"  # its match plays the Superbounce script
method = "coordinate"             # grid (default), random, coordinate
samples = 24                      # random: draws
rounds = 2                        # coordinate: passes over the axes
seed = 0                          # random: the draws' seed
min_attempts = 20                 # fewer tries: listed last, unranked
[base]                            # optional: what every candidate starts from
drift = 0.15
[axes]                            # bounce fields and the values to try
approach = [0.6, 0.8, 1.0]
still_ticks = [2, 4, 8]
```

- **Candidates.** `c000` is the base, the control. `grid`: every
  combination of the axes' values (at most 512). `random`: `samples`
  draws, each axis uniform between its least and most value (whole
  fields rounded). `coordinate`: from the base, one axis at a time, each
  of its values with the other fields at the best so far, `rounds`
  passes. A tuning that plays as one already listed (a field set to its
  built-in value plays as the field left out; `sweep::same_play`) is not
  played again.
- **Each candidate** gets `<out>/candidates/<id>/batch.toml`: the sweep's
  batch with its paths made absolute, its results beside it, and one arm,
  named the id, whose tuning is arm A's with the candidate's `[bounce]`
  over it. The CLI runs `h2 sim batch` on it; flags after the sweep file
  (`--parallel`, `--disc`, `--out`, `--data`, `--tier`, `--pace`,
  `--keep-data`) go to each batch.
- **Ranking.** Candidates with at least `min_attempts` tries (default 10)
  first, by bounce rate, then mean launch height, then tries, then id;
  the rest after, unranked. `<out>/sweep.json` (`h2-sim-sweep/1`: every
  candidate's tuning, runs, totals and per-map stats, and the best),
  `sweep.csv` (a row a candidate, every bounce field a column) and
  `sweep.md` (the top 20 as a table and the best's `[bounce]` block to
  paste).
- `--dry-run` writes the candidates' batch files and lists them (for
  `coordinate`, the first pass from the base only).

The winning numbers go into the game at one place,
`bots::tuning::BOUNCE_DEFAULT` ([bots.md, "Tuning the bounce"](../game/bots.md#tuning-the-bounce)).
The commands and the tables to fill are in
[notes/2026-11-21-bot-bounce-sim.md](../game/notes/2026-11-21-bot-bounce-sim.md).

### Training the bounce sites

`h2 sim train <train file>` searches each bounce site's knobs
([bounce-search.md](../game/bounce-search.md)) by playing the practice match
as parallel runs, and reports what the sites learned. `sim::train` (pure and
tested) plans the runs and builds the report; the CLI runs the processes.

```toml
out = "results/bounce-train"            # relative to this file
match = "superbounce-train-match.toml"  # the Superbounce script and the bots
maps = ["ascension"]
seed = 1        # the match's seed and the search's draws
shards = 4      # runs a map; fixed here, not by --parallel
budget = 120    # scored tries a site
```

- **Runs.** One run a map and shard, `<map>-shard<i>`, each the match
  file's match with its own `[train] shard = i, shards = n, budget = b,
  seed = s` (`sim::config::MatchConfig::train`, `sim::train::TrainConfig`).
  A run owns the sites whose spot id modulo `shards` is its `shard`
  (`sim::train::owns`), so the runs of a batch never fly the same site and
  the result does not depend on `--parallel`. `[train]` is refused without
  the Superbounce script, with a `[drill]`, with `shard` not under `shards`
  and with a budget of 0.
- **The game side.** `Sim::add_bots` hands the table to the bots
  (`bots::learn::set`). The word hook the drill uses (`sim: drill word`) runs
  in a training match too, and the match ends as soon as every site of the
  run's shard is closed (`bots::learn::done`, read each tick; the report's
  end is `stopped`), or at `max_ticks`.
- **Data.** Each run plays on its own copy of the map's `.learned`,
  `.verified` and `.ghosts` (`make_bounce_data`), kept in
  `runs/<id>/harvest/{start,end}`. Unless `--no-merge`, the runs' files are
  merged into `--data`'s `bots/` at the end, as `h2 sim harvest` does it;
  with it, `h2 sim harvest <out>` merges them later.
  Z:'s cache slots come from the image's template as in a batch
  (`play_runs`; [cache-slots.md](cache-slots.md#sim-batches-a-warm-z)).
- **Flags.** `--map NAME` (repeatable) and `--budget N` go over the file's
  values. `--dry-run` lists the runs and, from the map's `.spots` file, each
  shard's sites. `--reopen` first turns the `dead` and `blocked` lines of
  `--data`'s `.learned` back to `open`, their search restarted. `--status`
  prints the report table from the runs' `.learned` copies (the running
  run's own data, else what it kept, else the shared file), so it can run
  while a batch plays. `--parallel`, `--disc`, `--out`, `--data`, `--tier`,
  `--pace` and `--keep-data` are a batch's.
- **Resuming.** Run the same command again: closed sites are skipped and
  open ones carry on from their line.
- **Report.** `<out>/train.json` (`h2-sim-train/1`: each map's sites,
  states and tries, and a row a site with its state, `how`, tries, hits, the
  centre's `n` and `ok`, `score`, `best`, `deaths`, `why`, the knobs set and
  the search's cursor) and `train.md` (a heading a map, `map ascension: 166
  sites: learned 41, ...; 9480 tries`, and a table a site with `rate` =
  `ok / n`). Each run's lines are read from its own files and only the sites
  of its shard are taken.

`docs/runtime/sim-examples/superbounce-train.toml` and its match
`superbounce-train-match.toml` are the example (eight bots, thirty minutes
of game time a run).

### Seam drops

`h2 sim drops <match file>` measures superbounces without walking or nav:
each seat is put above a point of a candidate collision edge, given the
-3 pair word, dropped, and its outcome recorded. A match file with a
`[drops]` table is the input; `sim::drops` (pure and tested) plans,
shards, steps each drop and builds the report, `game::sim_drops` reads and
writes the guest, and the CLI runs the processes.

```toml
map = "ascension"
max_wall_s = 3600

[drops]
targets = "scan"      # the map's .spots bounce candidates, or a list
max_targets = 60      # scan: the first 60
heights = [0.5, 1.0, 2.0, 3.5, 5.0, 8.0]          # over the target's floor
across = { from = -0.12, to = 0.12, step = 0.02 } # or a list
along = 5             # points along each edge
repeats = 1
approach = { stick = 0.3, lead = 0.2, from = ["+", "-"] }  # optional
# carry = [[0.0, 0.0], [1.5, 0.0]]  # or touchdown velocities, not with approach
# carry_along = [0.1, 0.3, 0.6]     # or speeds along each target's drift
```

- **The table.** Every key is optional. `heights` (world units of the
  origin over the floor, a list or `{ from, to, step }` like `across`)
  default to `[0.5, 1, 2, 3.5, 5, 8]`; `across`
  (offset from the edge toward its drift side) to -0.12 to 0.12 in steps
  of 0.02; `along` to 5 points an edge (a point target has one);
  `repeats` to 1. `approach` pushes the stick at `stick` of full through
  the fall, the drop starting `lead` units back against the push, once
  from each side in `from`; without it the seat falls straight. `carry`
  is a list of horizontal velocities (world X and Y, units/s, each finite
  and at most 3 long) a straight drop touches down with, each drop once
  with each; it defaults to `[[0, 0]]`, the plumb drop, and is refused
  beside `approach` (`drops: carry and approach: one or the other`).
  `carry_along` is a list of speeds (units/s, each finite and at most 3
  either way) along each target's own drift direction, for a scan whose
  seams drift different ways; it is refused beside `carry` or `approach`
  (`drops: carry_along and carry: one or the other`, `drops: carry_along
  and approach: one or the other`). `seats`
  (1 to 15, default 15) is the seat count when the file lists no
  `[[bot]]`, `bounce_vz` (default 3.5, `sim::drops::BOUNCE_VZ`, the
  game's landing classifier's too) the launch that counts as a bounce,
  `settle_ticks`, `fall_ticks` and `after_ticks` (240, 300, 15) the
  phases' limits, `retries` (2) how often a setup failure is tried again,
  `out` the results directory (default `results/<file stem>` beside the
  file). A listed target is `{ id, shape, edge = [[x,y,z],[x,y,z]] }` or
  `{ point = [x,y,z] }`, with optional `drift` and `stand`.
  `targets = "scan"` reads the map's `.spots` from `--data` and keeps the
  slot, crease, vee and busy sites, ids `s000-slot` and on.
- **The match.** A drops table fills what the file leaves out: the map
  from `drops.map`, 15 phantom seats named `Dropper 1` and on, no time
  limit, team play (free-for-all sims end at tick 0) and the training
  report. It is refused with the Superbounce script, a `[drill]` or a
  `[train]`. The match ends (`stopped`) when every drop is done, or at
  `max_ticks` or `max_wall_s`.
- **Seats.** A shared queue feeds the seats: an idle seat takes the next
  drop whose start is 1.2 units clear of every busy seat and drop
  (`sim::drops::CLEAR`), else waits at its spawn; after 60 ticks with
  every seat idle, one takes a drop regardless. Up to 15 seats drop at
  once in a process, and `--parallel N` runs N processes.
- **The prime.** Each drop runs through the phases of
  `sim::drops::DropRun`. Place: a teleport (`Call::Teleport`, through the
  calls queue; a run with no `--socket` registers its own) to the rest
  point, just over the floor beside the target. Settle: the word is
  written 0 every tick and the body stopped until it is still for 3 ticks,
  so the teleport carries no prime and no speed. Arm: -3 on the still
  body and, in the same tick, the lift teleport straight up to the drop
  height. Fall and after: -3 is written over every fixed-body pair of the
  body (`live_telemetry::havok_agents::level_pairs` and `set_level_words`)
  every tick until `after_ticks` past touchdown, so a pair rebuilt in the
  air or by a sleep is primed again before the next tick. Each tick's
  row keeps the pairs' agents and words before the write. Through every
  phase the seat's phantom pad is held (`Phantoms::hold_pad`, forwarded
  by the sim's `Metered` seat), so its brain never steers; a seat whose
  pad can't be held is a note in the report.
- **Falling plumb.** A straight drop (no `approach`) lands under its
  release point: every fall row writes the body's horizontal velocity to
  0 (`Act::level`, the object's and each Havok motion's X and Y, Z kept),
  and the touchdown's velocity is the one the touchdown tick ran from. A
  fall row's `fixed` is the horizontal speed it took off, and the
  record's `drift_fixed` the most any row took: near 0 on a clean drop,
  large when something pushed the body sideways. An approach drop's
  horizontal motion is its held stick's and is left alone (no `level`,
  no `drift_fixed`).
- **Carrying.** A drop with a non-zero `carry` writes the carry instead
  of 0 on each fall row, so the touchdown tick runs from exactly that
  horizontal velocity. The drop starts back along the carry by its
  travel, the carry times the rows the fall covers past the first 5
  (`sim::drops::carry_travel`): the fall model (`sim::drops::fall_model`,
  fitted to the control's rows: rows 1 to 5 barely move, then the speed
  is 0.234 units/s gaining 4.171/30 a row) puts the touchdown on the last
  row still more than 0.03 over the floor, so 4.5 lands from row 46 at
  5.935, 5.5 from row 51 at 6.630 and 3.65 from row 42 at 5.378. The rest
  point is under the start, lifted by the travel's length as well as the
  across offset. Until the body falls faster than -1 the carry is
  written unchecked; from then on the first row whose horizontal velocity
  is more than 0.05 off the carry (`sim::drops::CARRY_TOLERANCE`: the body
  was hit) or that is on the ground stops the writes for the rest of the
  drop, and the record's `carry_cut` is that row. A row's `fixed` is how
  far off the carry it was; a carried drop's `drift_fixed` counts the
  rows from falling on. `[drops.control]` takes one `carry` (default
  `[0, 0]`).
- **Carrying along.** `carry_along` plans like `carry`, with each drop's
  carry the speed times the target's unit drift vector
  (`sim::drops::Target::drift`, the `drift` a listed target gave or the
  scan spot's, normalised): the fall model's travel, the start, the
  contact gate and the records are the carry's. The record's `carry` is
  that resolved world vector and `carry_along` the speed. A target with no
  drift (a scan spot whose drift is zero, a listed target with none) gets
  no drops, and a note in the report names the targets skipped. The
  control keeps its own `carry`.
- **Outcomes.** A touchdown is the vertical speed coming back over -0.5
  after falling faster than -1. `bounced`: the best vertical speed of the
  touchdown tick and the 3 after (`sim::drops::LAUNCH_TICKS`, the game's
  landing window too) reached `bounce_vz`; the record gives the
  launch speed and height (apex minus the touchdown's low point, the rest
  estimated ballistically if the apex comes after `after_ticks`).
  Otherwise the touchdown tick's prime decides: the pairs read the tick
  before must come out of it with the same agents, all still -3, and no
  new pair that isn't -3. When they don't, the drop is `prime_lost`, with
  the reason `no_pair`, `rebuilt`, `reset (word w)` or `new_pair (word w)`;
  when they do, `no_bounce`. `setup_failed` names the step (`teleport:
  ...`, `did not settle in 240 ticks`, `lift: never ran`, `no touchdown in
  300 ticks`) and is tried again. A failed prime is never counted as
  `no_bounce`.
- **The control.** Every run starts with the control drop, alone: on
  Ascension the slot seam at (-10.54, 4.0, -3.19), on the seam's line
  (across 0), from 4.5 and 5.5 over its floor: touchdowns near 5.9 and
  6.6 units/s, clearly over the 5.4 that bounces (a drop from 3.6
  touches down at 5.38, under it). When none bounces, the run stops with `control did not
  bounce: <outcome>` (the outcomes and reasons seen), the report is still
  written, and `h2 sim drops` exits with that error. Another map needs
  `[drops.control] point = [x,y,z]` (with optional `heights`, `edge`,
  `drift`, `carry`) or `no_control = true`.
- **Records.** Each drop's record (`sim::drops::Record`) gives the target
  id and shape, point, height, across and along offsets, side, carry,
  seat, attempts, release tick, outcome and reason, the touchdown (tick,
  position, velocity, speed, contacts), `aim_miss` (the touchdown's X and
  Y from the point), the launch, whether the prime held, `drift_fixed`,
  `carry_cut`, the words of the fall run-length, and the first
  tick of each event. The control's drops and the first drop of each outcome keep
  every row from the arm on. They are the report's `drops` block. Every
  position is the unit's origin (object `+0x64`, `sim_drops::body`),
  the point a teleport places, not the bounding sphere's centre
  (`+0x30`), which is about 0.4 units over it.
- **Sharding.** Targets are split over the runs by their drop counts
  (largest first, onto the lightest run), `<map>-drops<i>`, each run's
  match with its targets listed and its bots' tunings inline.
- **Report.** `<out>/drops.json` (`h2-sim-drops-report/1`) and `drops.md`
  pool the runs: outcome counts and the first of each; the control's
  drops; a row a target with its rate, catch half-width (the furthest
  touchdown from the edge that bounced), slowest bounced touchdown, best
  launch, its own launch fit, its largest `drift_fixed` and a cell a
  height, across offset and carry (`drops.md`'s columns name a non-zero
  carry as `c x,y`, and a `carry_along` drop by its speed instead, as
  `+0.00 a 0.30`); the drop with the largest `drift_fixed`
  over every run; and the three model numbers. `catch` is the catch half-width over every
  target (`superbounce::CATCH`). `bounce_speed` is the slowest touchdown
  that bounced beside the fastest that didn't (`BOUNCE_SPEED`). `gain` is
  the launch's slope over the excess speed, fitted with each target's own
  intercept (`speed::launch`'s gain) ([superbounce.md,
  Speed](../game/superbounce.md#speed)).
- **The verified file.** Each run plays on its own copy of the map's
  `.verified` (`make_bounce_data`) and writes a line for every target that
  bounced: its stand, the best bounce's offset and height, trials and
  hits. `--merge` merges them into `--data`'s `bots/` at the end; without
  it, `h2 sim harvest <out>` does.
- **Flags.** `--parallel N` (default half the cores), `--dry-run` (the
  plan: targets, drops and runs), `--merge`, `--keep-data`, and a batch's
  `--disc`, `--data`, `--tier` and `--pace`.

```sh
h2 sim drops docs/runtime/sim-examples/superbounce-drops.toml --parallel 4
```

`docs/runtime/sim-examples/superbounce-drops.toml` is the example: the
first 60 targets of the Ascension scan at the default heights and offsets.
`match.status` gives a drops run's progress (`drops`: planned, done, the
control's state and the error; [agent-api.md](agent-api.md)).

`h2 sim bounces` runs trials on the same targets from a saved machine,
seven ticks a wave, and ranks the seams by the error a player may make
([bounce-search.md](bounce-search.md)).
