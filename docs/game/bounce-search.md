# Bounce search: the bots learn each spot's superbounce

Status: Built, not checked in a sim run. The file (`superbounce::learned`),
the search with its board (`bots::learn`), the batch (`h2 sim train`), the
route's use of a line (`bounce_route::flown`) and the training loop
(`mind::practice`) are built.
Code: `crates/live/superbounce/src/learned.rs` (the file), `crates/live/bots/src/learn.rs` (the search), `crates/live/sim-bench/src/train.rs` (the batch and its report), `crates/live/bots/src/bounce_route.rs` (`flown`), `crates/live/bots/src/spots.rs` (reading and storing the file), `crates/live/bots/src/mind/practice.rs` (the training loop). Tests: in each of those files.
Switch: none. A sim match's `[train]` table turns training on; play reads `<map>.learned` whenever it exists.

Each bounce site has a few numbers that decide whether a primed bot lands
in its groove fast enough: where the run starts, where it aims, how hard
it runs, whether it jumps, and how it steers in the air. The bounce route
([bots.md, "Superbounce routes"](bots.md#superbounce-routes)) computes
them from a flight model. The search replaces the model's guess, spot by
spot, with what headless sims measure: a practising bot flies a site with
varied knobs, each try is scored, and the best knobs are kept in
`<map>.learned` beside the map's `.spots`, `.verified` and `.ghosts`. The
one rule: a knob that no line sets plays the route's own value, so a site
with no line plays exactly as the route plays it without this page.

## Evidence labels

**Code**: read from this repository (the path). **Run**: seen in a sim run
(the note). **S**: supposed, with what would verify it. **Design**: our
choice.

## How it works

### The knobs

One vector per bounce site, `superbounce::learned::Knobs`. Every field is
an `Option`; `None` is the route's own value (the "Default" column), read
through one function, `bounce_route::flown` (see "Interfaces"). Positions
are world units in world axes; "along" is the line from the launch point
to the aim, "side" its left.

| Key | Unit | Range searched | Default (the route's own) | Used by |
|---|---|---|---|---|
| `how` | `runoff`, `jump`, `stand` | every one the site allows: a site with no measured drop (`Site::drop_known`, every scanned one) allows the stand and both drops | `bounce_route::drop_takeoff`'s choice; `stand` for `Site::standing` | all |
| `fx` `fy` `fz` | point | ±0.5 to the side of the default | the launch point: `bounce_route::launch_from` | drop |
| `ax` `ay` | point | ±0.4 along, ±0.3 to the side of the default | `Site::target` | all |
| `yaw` | degrees | ±20 | 0: the run heads along the launch-to-aim line | drop |
| `back` | units | 0.2 to 3.0 | `bounce_route::RUN_UP` (0.6): how far behind the launch point the run starts | drop |
| `run` | stick | 0.2 to 1.0 | `drop_takeoff`'s stick | drop |
| `early` | units | −0.1 to 0.4 | `bounce_route::TAKEOFF_EARLY` (0.05): the jump pressed this far short | `jump` |
| `gain` | ratio | 0 to 3 | the tuning's `air_gain` | drop |
| `letgo` | units | 0 to 0.15 | `bounce_route::AIR_DEAD` (0.03): the air stick lets go this near the aim | drop |
| `drift` | stick | 0 to 1 | the tuning's `drift` | `stand` |
| `still` | ticks | 0 to 30 | the tuning's `still_ticks` | `stand` |

The search works in offsets from the defaults (side of the launch point,
along and side of the aim) and the file holds the resulting points, so a
line reads as coordinates and does not move when the launch-floor search
changes. The prime's numbers (`crouch_ticks` and the rest of
`bots::tuning::BounceTuning`) and a crouch spot's `dx`, `dy`, `yaw`
(`superbounce::verified`) are not knobs here: a training try starts with
the word set.

### A try and its score

A training try is a drill try ([bots.md, "Practising bounces"](bots.md#practising-bounces),
"The drill"): the bot walks to the site's approach (a drop's launch
point, the drill's start), the host writes −3 over its level pair word
(`bots::drill::request_arm`), and the route flies the site once with the
try's knobs. `bots::learn::Try` is what the route's last
`bounce_route::Leap` and the brain say of it; `learn::score` turns it into
one number (Design). The miss and its vector are measured from the site's
own target (`Site::target` without the line), not from the try's aim, so
moving the aim moves the miss (`learn::Try::from_leap`):

| Outcome | Score |
|---|---|
| bounced (`Touchdown::bounced`) | 1 + 0.5 × min(`vz_out` / 20, 1) |
| touched down by the strip, no bounce | 0.6 × 1 / (1 + miss / 0.1) + 0.2 × min(`speed_in` / need, 1) |
| no touchdown by the strip (`no_drop`, `no_landing`, `timeout`) | 0 |
| the bot died before it stood again (a Bounced fall, a fall out of the map) | the above, minus 1 |

`need` is the site's (`Site::need`): a measured one, else the one
threshold every seam shares, 5.4 units/s
([superbounce.md, "Speed"](superbounce.md#speed)). Any
bounce outscores any landing that did not bounce, a deadly bounce scores
under a clean miss, and among misses the score rises as the miss shrinks
and the landing speeds up: that slope is what the search climbs before a
site's first bounce. A try that says nothing about the knobs is **void**:
the word read as not −3 at touchdown (`not_primed`, `pair_lost`), a walk to
the start that failed, `map_changed`, `practice_off`. A void try is not
scored and costs no budget; its slot is asked again, twice at most.

### The search

A coordinate search with shrinking steps (a compass search), run in
rounds, with one scatter of hashed draws for a site that never bounces
(Design). It is chosen over CMA-ES and over a grid with halving because a
site gets tens to a few hundred tries, the outcome is mostly yes or no,
and eight knobs matter unequally: one axis at a time with repeated tries
needs no population, moves only on a measured gain, stays where it is on
the flat stretches of the stick-to-speed map (`bounce_route::ground_speed`),
and leaves a line a person can read as "moved the aim 0.15 on". The whole
state fits on the site's line, so a batch resumes from the file.

`learn::Search` is pure: `Search::next(&done) -> Option<Knobs>` gives the
round's next slot from the round's results so far, and
`Search::fold(&done)` ends the round. One bot flies a round's slots in
order. The **centre** is the current best knobs; `n`, `ok` and `score` are
the count, the bounces and the mean score of every try flown at exactly
the centre. Stages, in order:

1. **`arms`**, one round. Each `how` the site allows, at each run stick of
   1.0 and the tuning's `run_off_speed` (a stand: skipped), 3 tries each.
   The best mean is the centre; the route's own choice wins a tie. The
   flight model does not prune arms: the search measures them.
2. **`shift`**, up to 3 rounds, only while the centre has no bounce. Three
   tries at the centre give a mean miss vector (aim minus touchdown, along
   and side); three more fly with the aim moved 0.7 of it (held to the
   aim's range). The move is kept when their mean beats the centre's
   pooled `score`, else the stage ends. This is the gradient step: a run
   that lands 0.15 short aims 0.1 on. In `shift` the line's `axis` counts
   the shift rounds flown.
3. **`compass`**. One round an axis: 2 tries at the centre, 2 a step under
   and 2 a step over. The centre moves to a side whose mean beats the
   centre's `score` by 0.05. Passes at `shrink` 0 take the four axes that
   move the landing most (aim along, `run`, `back`, aim side); from
   `shrink` 1 they add the launch point's side, `yaw`, `gain`, `letgo` and,
   a running jump, `early` (a stand, every pass: aim along, aim side,
   `drift`, `still`). Reading the pass from `shrink` keeps it on the line.
   A pass with no move halves every step (`shrink`); the stage ends at
   `shrink` 3. A site that has bounced does not end: it keeps polishing at
   `shrink` 2 until it is `learned` or the budget is spent. First steps:
   0.1 units (`early` too), 0.2 of stick (`run`, `drift`), 0.6 of `back`,
   5°, 0.4 of `gain`, 0.02 of `letgo`, 4 ticks. A side the range clips
   onto the centre is not flown.
4. **`scatter`**, one round, once, only when the site has never bounced:
   16 tries, each knob drawn evenly over its range and `how` over what the
   site allows. The best single try becomes the centre when its score
   beats the centre's, and
   `compass` runs again from `shrink` 1.

After every round the stop rules are read, the first that holds:

| State | Rule |
|---|---|
| `learned` | the centre has `n` ≥ 10 and `ok` / `n` ≥ 0.6 |
| `dead` | `compass` has ended after the scatter and no try ever bounced (`why=no_bounce`), or the budget is spent and none did (`why=budget`) |
| `partial` | the budget is spent and some try bounced |
| `blocked` | three rounds running were all void at a site with no scored try (`why=` the commonest reason: `no_launch_floor`, `no_path`, `not_armed`, ...; the first by name among equals). At a site with tries the count starts again: a bot that can't walk back is where its last try left it (a launch lands far off), not the site's fault. The count of void rounds is kept in memory, not on the line |
| `open` | none of the above: the next round |

A `learned`, `dead` or `blocked` site is closed: a batch flies it no more.
`partial` reopens when a batch's budget is higher than its `tries`, and a
`blocked` line with tries reopens as it is read.
`blocked` costs no budget and says the site was never flown, so a nav fix
followed by `--reopen` tries it again.

### Draws and determinism

The search draws random numbers only in `scatter`. A draw is a pure
function, `learn::draw(spot, seed, try_index, axis)`: splitmix64 over the
spot id, the batch seed, the try's index and the axis (`Axis::index`, or
`learn::HOW_AXIS` for `how`), a number in `[0, 1)` that the scatter maps
to the axis's range. The spot id is FNV-1a 64 of `"<map> <x> <y> <z>"`, the
site's `stand` as the scan gave it, printed `%.3f` as the file prints it.
The try index is the site's `tries` at the round's start plus the slot.
Nothing depends on which bot flies the round or when. The file is written
only between rounds, and a round cut short is flown again from the same
state, so the same image, the same starting `.learned`, the same train
file give the same `.learned` byte for byte, as far as the sim itself is
deterministic ([sim.md §8](../runtime/sim.md#8-determinism)).

### Scheduling a batch

A match with `superbounce = true` and a `[train]` table is a training run
(`sim::train::TrainConfig`, refused without `superbounce`, as a `[drill]`
is). Its bots share one board, `learn::Board` (process-wide, as
`practice::CLAIMS` is), built from the map's bounce sites and its
`.learned` at the first pick. Every known site is on it, the scan's
candidates (single creases, Vs and busy points, `Site::candidate`)
too: the training is what proves them. A site with no measured drop
(`mind::practice::site_in`) allows the stand first, then a run-off and a
running jump from its launch floor: the nav floor nearest over the
target at the speed model's 2.43 drop (`bounce_route::launch_floor`),
when there is one.

- **Shards.** A run owns the sites whose spot id modulo `shards` is its
  `shard`. Runs of one batch never fly the same site, so their files merge
  without conflict and the result does not depend on `--parallel`.
- **Pick.** A free bot takes the open, unclaimed site of its shard with
  the fewest rounds flown this run, then the fewest `tries` (so a resumed
  batch still starts with the sites least flown), then the nearest, then
  the lowest spot id, and holds it for one round (`Board::pick`). Fewest rounds first means every site gets its `arms`
  round before any site is polished: a batch cut short still has a line
  for every site.
- **Round's end.** `Search::fold`, the stop rules, then
  `spots::store_learned` writes the map's file (atomic, the site's line
  replaced) and the log gets one line: `bots: train: site (...) round 7
  compass aim_along: 0.71 -> 1.18, aim +0.10 along; tries 54 hits 3`.
- **End.** The run ends at `max_ticks`, or as soon as every site of its
  shard is closed (`learn::done`, read by `sim_glue` each tick).

A try is 20 to 30 game seconds, most of it the walk back to the start, and
an unpaced run plays about six game seconds a wall second (S: the
coordinator's figures; the first batch times them). A 30-minute match with
eight bots is then some 500 tries in five wall minutes; Ascension's 166
scanned spots at 60 tries a site is some 10,000 tries, about twenty such
runs.

**Resuming.** A batch starts from the `.learned` in `--data`'s `bots/`:
closed sites are skipped, open ones carry on from their line's cursor
(`stage`, `axis`, `shrink`, `moved`, `scattered`, `tries`). Running the
same command again continues; nothing else is kept between batches.

### How the route uses a line

- **Loading.** `spots::learned_for(map)` reads and keeps the file as
  `spots::verified_for` does, from the same directory
  (`spots::set_verified_dir`), and `store_learned` bumps
  `spots::generation`. `Known::for_map` gives each site its line's knobs
  (`Site::knobs`, by `Site::learned(entry)` beside `Site::verified`): the
  knobs of an `open`, `partial` or `learned` line. A `dead` site is left
  out of `Known::sites` as a verified `failed` one is, unless a verified
  entry proves it; a `blocked` line changes nothing. The training board
  keeps dead sites, to report them and for `--reopen`.
- **Planning.** `bounce_route::launch_from` takes the line's `fx fy fz`
  ahead of the spot's own takeoff and the launch floor, and
  `Site::target` takes `ax ay` ahead of the verified aim and the stand. A
  line's `how=stand` makes the site standing and `runoff`/`jump` make it
  a drop (`Site::standing`).
- **Flying.** `bounce_route::flown(site, from, tune) -> Flown` is the one
  place a knob meets the route: each field of `Flown` is the knob, else
  the expression `OrderRun::drop`, `OrderRun::drop_air` and
  `OrderRun::bounce` use today (`drop_takeoff`'s jump and stick, `RUN_UP`,
  `TAKEOFF_EARLY`, the heading, `air_gain`, `AIR_DEAD`, `drift`,
  `still_ticks`). Those three read `Flown` and nothing else for these
  numbers; `air_stick` takes the let-go distance as an argument. The
  search's centre for a site with no line is `Flown` turned back into
  `Knobs`, so a search starts from what the route plays.
- **Training.** A try's site is the known site with the board's knobs
  over its own (`Site::knobs`), so the route flies it as it flies a line.
  `mind::practice` has the training loop beside the drill's
  (`Mind::train_pad`): pick from the board, walk to the site's approach,
  ask for the word, fly once (a landing that did not launch ends the
  route, `try_over`), build the `Try` from the route's first flight in
  `OrderView::flights` and the route's end (a `died` during a training try
  is a scored try, not a dropped one), hand it to the board. Each try
  still logs its `bots: bounce try:` line, with the knobs appended.
- **Practice and play.** The practice's pick weighs a `learned` site +1
  (`practice::weight`). A route or a show-off never takes a candidate
  until a game run proved it (`Site::plays`). `.verified` and `.ghosts`
  still say what a real primed run did, and a `learned` site becomes
  proven only by one. The game's spot list takes a `learned` or
  `partial` line's aim over the scan's (`superbounce::learned::apply`,
  [superbounce.md, "Bounce"](superbounce.md#bounce)), and leaves its
  status alone.

## Interfaces

### The file: `<map>.learned`

Text, in `--data`'s `bots/`, written by `superbounce::learned::to_text`
and read by `parse`, in the line style of `superbounce::verified`:

```text
# superbounce learned v1
bounce learned -10.620 3.880 -3.190 how=runoff fx=-8.330 fy=2.540 fz=-0.760 ax=-10.540 ay=4.090 yaw=0.0 back=1.20 run=1.00 gain=1.20 letgo=0.030 tries=64 hits=19 n=12 ok=8 score=1.214 best=20.90 deaths=1 stage=compass axis=2 shrink=1 moved=0 scattered=0
bounce dead 5.870 6.360 1.060 how=stand ax=5.910 ay=6.360 drift=0.40 still=4 tries=120 hits=0 n=6 ok=0 score=0.412 why=budget stage=compass axis=1 shrink=2 moved=0 scattered=1
```

(The numbers show the shape, not a run.)

```text
file   = header NL { ( line | comment | blank ) NL }
header = "# superbounce learned v1"
line   = "bounce" SP state SP x SP y SP z { SP key "=" value }
state  = "open" | "learned" | "partial" | "dead" | "blocked"
```

- `x y z` is the site's `stand` as the scan gave it; a line is about the
  bounce spot within `verified::MATCH` and `MATCH_Z` of it.
- **Knob keys**: the table in "The knobs". One left out is the route's
  own. `to_text` writes every knob the line's `how` uses.
- **Evidence keys**: `tries` (scored tries), `hits` (of them, bounced),
  `n`, `ok`, `score` (at the centre), `best` (the best launch, units/s
  up), `deaths`, `why` (a `dead` or `blocked` line's reason).
- **Cursor keys**: `stage` (`arms`, `shift`, `compass`, `scatter`), `axis`
  (the next axis of the pass), `shrink`, `moved` (the pass has moved),
  `scattered`.
- `to_text` writes the keys in the order above and the lines sorted by
  `x`, `y`, `z`, so equal entries give equal bytes.
- **Refused**, the file read as empty with one log line naming the line:
  another version, an unknown key, an unknown state, stage or `how`, a
  value out of its range, a knob the line's `how` does not use, a key
  given twice, a point given in part (`fx` without `fy`), `hits` over
  `tries` or `ok` over `n`, a `why` on a line that is not `dead` or
  `blocked`. A mistyped knob is never skipped quietly. The version
  number rises whenever the format or what the route makes of a knob
  changes; a refused file is trained again from nothing. There is no
  migrator.

**Merging** (`learned::merge(old, new)`, used by `h2 sim harvest` through
`bots::harvest::harvest_run`, which treats `.learned` as it treats
`.verified`: the lines a run changed between `harvest/start` and
`harvest/end`). Two lines about one site are never mixed: the better one
is kept whole; two lines are about one site when their printed `x y z`
are the same (an exact key, so the merge stays order-independent). Better
is, in order: `learned` over not; some `hits` over
none; between two `learned`, the higher `ok / n`; more `tries`; the higher
`score`; the line that sorts first as text. The order is total, so merging
is the same in any order and merging twice adds nothing, and a search
that carried on from a line always beats the line it carried on from. Two
machines that learn the same site therefore agree once each has harvested
the other's file. Inside a run, `store_learned` replaces the site's line
outright: it is the same search, further on.

### The CLI and the sim

```sh
h2 sim train docs/runtime/sim-examples/superbounce-train.toml --parallel 4
h2 sim train <file> --map ascension --budget 200   # over the file's values
h2 sim train <file> --status                       # the table, from the runs' files
h2 sim train <file> --dry-run                      # the runs and each shard's sites
h2 sim train <file> --reopen                       # dead and blocked back to open first
```

```toml
out = "results/bounce-train"       # relative to this file
match = "superbounce-train.toml"   # superbounce = true and the bots
maps = ["ascension"]
seed = 1
shards = 4      # runs a map; fixed here, not by --parallel
budget = 120    # scored tries a site
```

`sim::train` (pure) plans one run a map and shard; the CLI
(`crates/apps/cli/src/sim_cmd.rs`) writes each run's match with
`[train] shard = i, shards = n, budget = b, seed = s`, runs them as a
batch does, each on its own copy of the map's `.learned`, `.verified` and
`.ghosts` (`make_bounce_data`), and at the end merges every run's
`.learned` into `--data`'s `bots/` (`--no-merge`: left in the runs'
`harvest/end` for `h2 sim harvest`). `h2 sim run` on a match with a
`[train]` table is one shard on `--data`'s own files.

What it writes: each run's `report.json` gets a `train` block beside
`bounce`, and the batch gets `<out>/train.json` (`h2-sim-train/1`) and
`<out>/train.md`:

```text
map ascension: 166 sites: learned 41, partial 17, open 60, dead 39, blocked 9; 9480 tries
site                     state    how     tries hits  n  ok rate  score  best  deaths why
(-10.62, 3.88, -3.19)    learned  runoff     64   19 12   8 0.67  1.214  20.9       1
(5.87, 6.36, 1.06)       dead     stand     120    0  6   0 0.00  0.412     -       0 budget
```

(Shape, not a run.) `rate` is `ok / n`: the bounce rate at the best knobs.
The JSON row adds the knobs and the cursor. **Progress**: `--status`
prints the same table at any time from the running runs' `.learned`
copies, which change every round, and each run's `log.txt` has the
`bots: train:` round lines.

### Agent API

The learned knobs will be asked for again (by whoever checks why a bot
flies a site as it does, in a sim or a game), so the API grows; no new
method is needed (Design, by the rule in `AGENTS.md`):

- `superbounce.spots` gives each bounce spot a `learned` object when the
  map's file has its line: `state`, `knobs` (the keys of "The knobs",
  those set), `tries`, `hits`, `n`, `ok`, `score`, `best`, `why`.
- `bots.bounce_status` gives `knobs` for the running route (the knobs it
  flies, `null` for the route's own), and while training `practice.train`:
  `site`, `stage`, `axis`, `slot`, `tries`.

Both are documented in [agent-api.md, "Bounce routes"](../runtime/agent-api.md#bounce-routes)
with the code.

## Rules and limits

- A site with no line, and a knob no line sets, play the route's own
  value. `flown` is the only reader of a knob.
- Training runs only in a sim match with `[train]`; it needs the host's
  word write. A game never writes `<map>.learned`.
- A line never proves a site. `learned` says an armed try bounces six
  times in ten; `.verified` says a primed run did.
- One bot flies a site at a time, and a shard owns a site for a batch.
- The search fixes how the body lands, not how it gets the word or walks
  there: a site whose paired crouch spot fails stays unusable in play
  however well it is learned.

Risks and open questions:

- **The plateau.** A landing on the aim, fast enough, that does not bounce
  has no slope: only the aim's compass steps and the scatter look further,
  and the scan's `stand` may be more than 0.4 from the groove. Such sites
  end `dead`; widening the aim's range is the first thing to try on them.
- **Noise against two tries a probe.** A centre that bounces half the time
  can move on luck. The 0.05 margin and the pooled centre damp it; whether
  it needs three tries a probe is for the first batch to show.
- **Which launch floor.** `launch_floor` gives one ledge a site. A site
  with a better ledge elsewhere is searched ±0.5 round the wrong one.
  Open: draw the scatter's launch point from every ledge in reach.
- **Stale lines.** A change to the route or the flight model changes what
  a knob does. The rule is the version number; forgetting to raise it
  leaves lines that no longer fit.
- **The cost of the walk back.** Placing the body at the try's start (as
  `sim.state.set` does) would cut a try to a few seconds, but a body that
  walked there arrives differently from one put there. Open: measure
  whether placed tries score as walked ones do before adopting it.
- **The armed word.** The drill showed the written word holds to the
  takeoff; a site whose real walk from its crouch spot loses the word is
  learned here and fails in practice. The practice's tally shows it.
- **Other bots.** Bots of a run share the map; one in another's flight
  path spoils a try unseen. The pick's nearest-first rule spreads them
  only loosely.

## Evidence

- **Run.** The ground-truth drill's running jump at the route's planned
  speed reached about 1.05 units/s on the ground and landed 0.27 short
  every try; the same drill as a full-stick run-off touched down at 5.40
  and bounced on 7 of 14 tries, the misses about 0.15 short and one try
  out of the map. So the flight model's choice is wrong at the one
  measured spot and a miss vector is there to climb
  ([notes/2026-11-21-bot-bounce-sim.md](notes/2026-11-21-bot-bounce-sim.md);
  the run-off entry is the coordinator's and is not in the note yet).
- **Code.** The stick-to-speed map is flat from 0.6 to 0.85 of stick and
  doubles by 1.0 (`bounce_route::ground_speed`,
  `crates/live/bots/src/bounce_route.rs`).
- **Code.** A try's takeoff, air stick, touchdown point, miss, speeds in
  and out are already recorded (`bounce_route::Leap`, `Touchdown`).

Tests that prove it, each on synthetic data:

| Test | In | Shows |
|---|---|---|
| `search_finds_the_groove_on_a_fake_landing` | `bots::learn` | on a model that bounces within 0.05 of a hidden point at a hidden run stick, with seeded noise, the search ends `learned` within the budget |
| `shift_closes_a_short_landing_before_any_bounce` | `bots::learn` | a model landing 0.2 short moves the aim on, and the score rises round by round |
| `a_site_that_cannot_bounce_ends_dead` and `all_void_rounds_end_blocked` | `bots::learn` | the stop rules, and that void tries cost no budget |
| `the_same_results_give_the_same_search` | `bots::learn` | two runs fed the same tries give equal entries; `draw` is fixed for fixed arguments and differs across spot, seed and index |
| `a_search_resumes_from_its_line` | `bots::learn` | a search written after round k and read back asks for the same slots as one never written |
| `score_orders_the_outcomes` | `bots::learn` | bounce over miss, near over far, fast over slow, a death under a clean miss |
| `text_round_trips` and `a_bad_line_is_refused` | `superbounce::learned` | `parse(to_text(x)) == x`, byte-stable order; an unknown key, state or version is an error naming the line |
| `merge_keeps_the_better_line_whole` | `superbounce::learned` | the order above; commutative, idempotent; a continued search beats its start |
| `no_knobs_fly_as_the_route_does` | `bounce_route` | `flown` with `Knobs::default()` equals today's expressions field by field, and the existing drop and stand tests pass unchanged |
| `a_knob_moves_only_its_own_number` | `bounce_route` | each knob set alone changes one field of `Flown` (the yaw its heading too) |
| `learned_knobs_change_the_plan_aim_launch_back_run_and_how` | `bounce_route` | the aim, launch point and `how` change the plan; `back`, `run` and `how` change the flight on the fake body |
| `a_training_loop_flies_the_boards_knobs_folds_its_tries_and_writes_the_file` | `bounce_route` | the board's knobs flown on the fake body, folded round by round and written between rounds |
| `a_training_bot_flies_its_rounds_folds_its_tries_and_writes_the_file_between_rounds` | `mind::practice` | the practice's training loop: drill tries from the board, the file written after the arms round and not before |
| `a_dead_site_is_left_out_unless_proven` | `spots` | `Known` with a learned file |
| `learned_lines_are_harvested_as_verified_ones_are` | `bots::harvest` | only what the run changed is merged, twice adds nothing |
| `train_plans_disjoint_shards` | `sim::train` | every site in exactly one shard; `[train]` without `superbounce` refused |

## Not verified

- That a training batch run twice gives the same `<map>.learned` byte for
  byte in the real sim.
- That knobs learned with the word written bounce after a real prime and
  walk.
- That two tries a probe are enough once a site bounces sometimes.
- The cost: tries an hour a bot, and tries a site to `learned`.
- How many of Ascension's scanned sites end `learned`, `dead` and
  `blocked`.

## Related

- [bots.md](bots.md): "Superbounce routes", "Practising bounces", "Tuning the bounce".
- [superbounce.md](superbounce.md): the mechanism, "Verifying spots", "Ghost runs".
- [sim.md](../runtime/sim.md): "Bounce practice", "The bounce drill", "Sweeping the bounce tuning" (one tuning for every site; this page is one vector a site).
- [notes/2026-11-21-bot-bounce-sim.md](notes/2026-11-21-bot-bounce-sim.md): the sim results so far.
