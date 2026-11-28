# Playtest: headless matches on the map being edited

Status: Design, in part built. `h2 sim`, its match and batch files, its reports, the spatial trace and its measures, and the bots are built ([sim.md](../runtime/sim.md), "The spatial trace"; [bots.md](../game/bots.md)), and so are the static checks and `playtest.static`, and the runner's logic and `playtest.run`, `status`, `result` and `cancel` over a `Source` (the build, the bots' mesh and the sim) that tests fake. The overlays and the panel are built over the result the runner publishes, and so are `playtest.compare` and the difference layers. The real `Source`, the run on save and the pool are not built; with no `Source` connected `playtest.run` is refused.
Code: `crates/apps/studio/src/app/playtest_static.rs` (the static checks), `crates/apps/studio/src/api/playtest.rs`, `crates/live/sim-bench/src/spatial.rs` (the trace and its measures), `crates/extensions/game/src/sim_glue.rs` (the observers), `crates/apps/studio/src/app/playtest.rs` (the runner, its `Source`, the baseline and `compare_runs`), `crates/live/sim-bench/src/spatial/compare.rs` (the comparison), `crates/apps/studio/src/app/playtest_shown.rs` (the result on show), `crates/apps/studio/src/viewport/playtest.rs` (the overlays), `crates/apps/studio/src/ui/playtest.rs` (the panel). Tests: `app::playtest_static::tests`, `api::tests::playtest_tests`, `sim::spatial::tests`, `app::playtest::tests`, `sim::spatial::compare::tests`, `viewport::playtest::tests`, `ui::playtest::tests`.
Switch: none.

While a map is being made, Studio plays it: bot matches on the current
build, with no window, many at once and faster than real time. What
happened in them comes back as layers over the viewport: where players
die, which spawns are unfair, where they walk and where they never go,
how far apart they fight, which weapons are left lying, where a bot got
stuck. An edit makes the result stale and a new run compares itself
with the last. The rule that must hold: a result names the map build,
the match file and the seeds it came from, and the same three give the
same result, so a difference between two results is the edit's and not
the dice's.

## How it works

### What a run is

`playtest.run` does, in order (`app::playtest::Runner`):

1. **Build.** The open project is built
   ([tag-workspace.md](tag-workspace.md), "Projects"). Its build key
   names the map.
2. **Static checks** (below). A map that cannot be played says why and
   nothing is started.
3. **Nav.** The bots' mesh for this build is made if none is current
   (`h2 bots nav`'s builder, the one `nav.bots` runs in memory), keyed
   by the build key.
4. **Batch.** A match file and a batch file are written under
   `<cache>/playtest/<run>/` and `h2 sim batch` is started on them
   ([sim.md](../runtime/sim.md), "Parallel runs, A/B and tuning"): one
   child process per seed and game type, each with its own data
   directory and report, the map being the project's package.
5. **Gather.** As each child ends, its report and its spatial trace are
   read and folded into the run's result. Partial results show while
   the rest play.

The seats are phantoms, as every bot is. The match file is the sim's
own; a run's preset fills it:

| Preset | Matches | Length | For |
|---|---|---|---|
| `probe` | 4 seeds of the map's first game type | 2 game minutes | every save |
| `standard` | 8 seeds per game type the map is ready for | 5 game minutes | a look before sharing |
| `soak` | 32 seeds, sides swapped | to the score limit | balance |

The runner is `app::playtest::Runner`, driven through a `Source`: its
`build` gives the build key and the map's name, `nav` makes the mesh,
`start` takes the written `Batch` (the batch file, the match file and
the jobs: game type, seed and which leg), and `poll` gives each match
that ended with its `Trace` or why it failed. `Runner::start` chooses
the game types (the preset's among the ready ones, or those named, each
of which must be ready), builds, marks runs of another build stale,
answers from the cache when a finished run with no failed match has the
same key (build, map, image, bots' version, game types, seed count,
legs, length and seats), otherwise cancels a running run, makes the mesh
and writes `match.json` (`detail = "spatial"`, `bot_threads = "off"`,
`max_ticks` from the minutes) and `batch.json` (`arm_swap` with two
arms for `soak`) under `<cache>/playtest/<run>/`, in the sim's own
formats. `pump` folds in each ended match, once; a run is done when
every match ended, and failed when none gave a trace. A result is
`sim_bench::spatial::measure` over the traces of one game type so far.

### What is measured

The sim's tally has scores, the kill log, shots, hits and each bot's
stuck ticks and repaths ([sim.md](../runtime/sim.md), "Metrics"). It
has no places. `detail = "spatial"` adds a trace beside the report,
taken by the observers the tally already uses:

- every player's position and facing five times a second;
- each spawn: the starting location, the nearest enemy and whether one
  had a line to it;
- each damage and each kill: both places, the weapon, the distance;
- each pickup: the item's placement and the time since it appeared;
- each tick a bot is stuck or a path fails: where, and where it was
  going.

`sim::spatial` turns traces into measures, each a pure function tested
on invented traces:

| Measure | From | Overlay |
|---|---|---|
| Kills and deaths | kill events, binned on the nav mesh's polygons | two heat layers, by team or weapon |
| Spawn fairness | per starting location: uses, seconds lived, deaths within 3 s, enemies in sight at spawn | each spawn coloured, with its numbers; spawn kills as marks |
| Traffic and flow | position samples: time per polygon, and crossings of polygon edges with their direction | a density layer and flow lines; polygons never entered are hatched |
| Sightlines and engagement distance | damage and kill segments | the longest and the commonest lines drawn; a distance histogram per weapon in the panel |
| Time to first engagement | per life, spawn to the first damage given or taken | per spawn, and the match's opening as one number per team |
| Team balance | wins, score difference and first-engagement side, with sides swapped | a panel table; no layer |
| Weapon use | per item placement: pickups a minute, time to first pickup, kills made with it | each placement sized by use; never-picked items ringed |
| Stuck spots and nav holes | stuck and failed-path events; mesh islands no spawn reaches | marks at the spots; the unreachable islands outlined |

### The layers and the panel

**Code**, `viewport::playtest` and `ui::playtest`. Each `playtest.*` call
publishes the newest run's measures for one game type to
`App::playtest` (`app::playtest_shown::Shown`); the viewport draws a
layer from them when it is shown, in the colour of `theme::playtest_color`:

| Layer | Marks |
|---|---|
| `playtest_kills`, `playtest_deaths` | a square and a post a bin, larger and brighter with its count |
| `playtest_spawns` | a diamond and post a starting location, green to red by the larger of its spawn-death share and its seen share, grey if unused; a red cross a spawn kill |
| `playtest_traffic` | a square a bin by time spent |
| `playtest_flow` | an arrow a crossing, brighter the more crossed; the 200 most |
| `playtest_unvisited` | each polygon the players never entered, outlined and hatched on the bots' mesh |
| `playtest_sightlines` | the longest lines in gold, the commonest in white |
| `playtest_weapon_use` | a diamond a placement sized by pickups a minute; a never-picked one red and crossed |
| `playtest_stuck` | a cross, post and arrow to where it was going a stuck spot; the unreached islands outlined red |

A polygon is the bots' mesh polygon of the trace's index, so those layers
draw only once the mesh has loaded (showing a playtest layer loads it).
A measure refused for too few events draws nothing and `view.layers`
gives its text in `refused`. A result marked stale keeps its marks at
40% strength. The **Playtest** panel (a tab of the bottom dock) lists
the run, its flags, each refusal, the spawn rows, the distance histogram
per weapon and a summary of flow, items and stuck, with a switch for each
layer. A layer's marks are the bins of the measure, not the game's
polygons, so a bin off the mesh still draws at its mean place.

### Static checks

**Code**, `app::playtest_static`. Some of this needs no match and runs
on the open map as edited, saved or not (`playtest.static`;
[agent-api.md](agent-api.md), "Playtest's static checks"). `check` is
pure: it takes a `Layout` (the starting locations, netgame items and
netgame flags with their teams, the bots' mesh, the level collision's
triangles and the start gate's census) and gives each problem with its
target, and each game type's readiness:

- the bots' mesh is there (`app::bot_nav`'s load, read or built in
  memory), and every starting location, item and flag stands on it:
  over a polygon, from 0.1 units under its floor to 1 over it
  (`locate`);
- every starting location reaches every other along the mesh's links,
  per team and across;
- islands of the mesh no starting location reaches, grouped by their
  links (1 square unit or more); links from the reached mesh into
  polygons with no way back, one per trap (a drop off a ledge into a
  pit);
- lines between the eyes (0.6 units up) of starting locations not of
  one team, at most 40 units apart, through the level collision's
  triangles, for spawns in sight of each other;
- what each built-in game type needs that the map lacks: the start
  gate's check on the census of the scenario's flags and starting
  locations for teams 0 and 1 ([rules-start-gate.md](../game/rules-start-gate.md);
  `Census::of_game` with no Forge document, so no tag paths), two
  starting locations on the mesh, and for Capture the Flag and Assault
  what `ctf.rules` declares with no least count since one script serves
  both: a `ctf_flag_spawn` flag per team, an `assault_bomb_spawn` per
  team (or a neutral one) and an `assault_bomb_return` per team, and a
  starting location each team can use.

The sight lines ignore the placed objects' collision. The thresholds
are constants of `app::playtest_static`; none is measured yet.

### How an edit gets in

A sim match plays one build from its first tick, so an edit never goes
into a running match. A save makes a new build key; the shown result is
marked stale and keeps its layers, dimmed. With **Playtest on save** on,
a `probe` starts after the save's build, and a newer save cancels it. A
hot edit to the own game ([game-view.md](game-view.md)) changes no
result until it is saved. The static checks follow every edit, saved or
not.

### How many and how fast

One match is one process: the guest's tick on one thread, with bot
thinking on workers. A batch runs as many processes as the machine has
performance cores, less two for Studio, with `bot_threads` off so each
match keeps to one core. A tick with bots costs between a third and a
half of real time on the retail maps measured
([sim.md](../runtime/sim.md), "Bot navigation performance";
[sim-budget.md](../perf/sim-budget.md)), so one process plays about two
to three game minutes a minute, and eight of them about twenty. The
[sim rewrite](../engine/sim-rewrite.md) lowers the tick and this page
gains from it with no change.

The fixed cost is the start: a sim process boots and sets up its lobby
before the first tick, some twenty seconds in the reports. Two things
cut it. Children are kept after their match and given the next one
(`match.start` on a waiting sim, new), so a run pays the boot once per
process and a later run not at all while the pool lives. And a `probe`
is sized so its matches outlast their set-up.

Targets until `notes/playtest.md` (new) records them (**S**): static
checks under a second; a `probe` back in under a minute with a warm
pool; a `standard` run of one game type in under five.

### Seeds and determinism

The same image, switches, match file and bot parameters give the same
match tick for tick ([sim.md](../runtime/sim.md), "Determinism"). A run
adds the build key. So:

- a preset's seeds are fixed (1 to N); a result is cached by build
  key, match file, seeds, the bots' version and the image's stamp, and
  asking again reads the cache;
- the nav mesh is built before the batch and read on the game thread,
  as sim mode requires;
- matches end on `max_ticks` or the game's own end, never on the wall
  clock.

### Before and after

An edit changes the map, so the two builds' matches differ from the
first tick and are not compared tick by tick. `playtest.compare {a, b}`
compares results over the same seeds and match file (two runs that
differ in seeds, length, seats or sides are refused):

- each scalar measure (`deaths_per_min`, `kills_per_min`,
  `spawn_kills_per_min`, `mean_life_s`, `mean_in_sight`,
  `pickups_per_min`, `stuck_ticks_per_min`, `sight_median`) is taken per
  seed, a seed's swapped leg counting with it, and averaged; a change is
  called out only when the two means differ by more than the larger of
  the sides' standard deviations across seeds. A side with fewer than two
  seeds that could tell has no spread and calls nothing out;
- each layer as a difference layer (`sim-bench/src/spatial/compare.rs`):
  a mark where the count per trace moved by more than two standard
  deviations of a counted event (of a share, for a spawn's fairness),
  the most changed first. Kills, deaths, traffic, flow lines, the spawns'
  fairness, the weapons' pickups and the stuck spots are compared by
  place; the never-entered polygons by index, which is refused when the
  two meshes differ in size. The sightlines have no difference layer. A
  layer whose measure was refused on either side says so;
- what the static checks gained or lost: the problems each run started
  under, as `kind: text`.

The first run of a session with a match that ended is kept as the
baseline (`baseline`); any two runs can be compared by id. `playtest.run
{compare_to}` names the run a result is compared with by default, and
`view.layers {diff}` draws the difference layers, red where there is more
and blue where there is less (a polygon opened is green). Eight seeds is
the least that gives a spread worth trusting, which is why `probe`
results are marked coarse.

### On a map of ours

A retail map with edits has everything a match needs. A fresh map (the
stage, a level made in Studio) needs, and the static checks name what
is missing:

- collision the bots' mesh builder accepts: floors, with the level
  closed or bounded by kill volumes so a fall ends;
- starting locations for each team, enough for the seats, on the mesh;
- weapons and equipment as netgame items; the game type's objectives
  and the labels its script declares;
- a nav mesh that links its parts: the builder's jump and drop links,
  and for lifts and teleporters the links the bots learn or are given
  ([bots.md](../game/bots.md));
- a build the sim can load: a package, the light host's map reads, and
  no tag the headless run faults on.

The game's own pathfinding data in the `sbsp` is not needed: phantoms
steer by the bots' mesh. The stage is the first fresh map run this
way; its ramps, steps and ledges are known shapes for the mesh builder
and the stuck-spot measure to be tested against.

## Interfaces

| Method | Access | Params | Result |
|---|---|---|---|
| `playtest.static` (**Code**) | read | `{wait?}` | `{ok, nav, problems: [{kind, text, target}], ready: [{game_type, engine, ok, missing}]}`; a placement `target` goes to `nav.go`, a polygon's to `nav.pick`. It checks the open map; a project param comes with the workspace's projects |
| `playtest.run` (**Code**) | act | `{preset?, game_types?, seeds?, minutes?, bots?, compare_to?}` | `{run, build, matches, parallel, cached}` at once |
| `playtest.status` (**Code**) | read | `{run?}` | `{run, state, preset, build, done, total, speedup, stale, coarse, failed: [{game_type, seed, swapped, why}]}` |
| `playtest.result` (**Code**) | read | `{run?, game_type?, measure?}` | the run's build, match file and seeds, and the measures each `ok` or refused; or one in full with its bins |
| `playtest.compare` (**Code**) | read | `{a?, b?, game_type?}`: `a` is the run's `compare_to`, else `baseline`; `b` the newest run | `{compared, a, b, game_type, measures: [{name, a, b, spread, n_a, n_b, changed}], changed, traces, layers: {layer: {marks, top, refused?}}, static: {gained, lost}}`; `{compared: false, why}` with nothing to compare |
| `playtest.cancel` (**Code**) | act | `{run?}` | `{cancelled}` |
| `view.layers` (**Code**) | act | the playtest layers by name (`playtest_kills`, `playtest_deaths`, `playtest_spawns`, `playtest_traffic`, `playtest_flow`, `playtest_unvisited`, `playtest_sightlines`, `playtest_weapon_use`, `playtest_stuck`), the `playtest` preset; `diff` (`true`, a run id, `baseline`, or `false`) | as now, and `diff: {a, b, game_type}` or null; a playtest row's `count` is its marks (the difference's with `diff`) and its `refused` the refusal |
| `match.start` | act (the sim) | `{file}` on a sim whose match is over | `{queued}` |

`h2 sim` takes `detail = "spatial"` and writes `spatial.bin` beside the
report ([sim.md](../runtime/sim.md), "The spatial trace").
A match in the pool can be watched: the viewport's live mirror takes a
child's world stream as it takes a paired game's.

## Rules and limits

- A run starts `h2 sim` processes of its own. It never uses a game the
  user started and never builds an image; with none current it refuses
  with the command that builds one.
- Results, traces and pools live under the cache directory. A run is
  refused when the free space is under its estimate.
- The bots are the players. What a layer shows is how these bots play
  this map; the panel says which bots, skill and count.
- A measure with too few events says so instead of drawing a layer.
- Only game types the static checks call ready are run.

## Evidence

- The match file, the batch, per-run data directories, reports and the
  A/B comparison exist (**Code**, `crates/live/sim-bench`, `crates/apps/cli/src/sim_cmd.rs`).
- The tally's kill log has attacker, victim and tick, and the bots'
  stuck ticks and repaths are counted (**Code**, `crates/live/sim-bench/src/metrics.rs`).
- A sim match names an installed package as its map (**Code**, the
  Firefight match in [sim.md](../runtime/sim.md), "Firefight").
- Studio builds the bots' mesh in memory for a map with no cache
  (**Test**, `api::tests::nav_bots_builds_the_mesh_when_there_is_no_cache`).
- A change inside the spread is not called out and one outside it is; each layer's marks are the places that moved beyond counting noise; a spawn that became unfair, a polygon opened and a resized mesh are found or refused (**Test**, `sim::spatial::compare::tests`).
- The baseline, a run compared with it over the same seeds, the problems gained and lost, other seeds refused, `compare_to`, `view.layers {diff}` counting and refusing, and the difference marks' colours (**Test**, `app::playtest::tests`; `api::tests::playtest_tests`; `viewport::playtest::tests`).
- A run's files are the sim's: the batch planner reads them and plans
  every match (**Test**, `app::playtest::tests`).
- Partial results, a failed match, a cancel, a stale mark on a new build
  key, a cache hit and a newer run replacing one going, over a fake sim
  (**Test**, `app::playtest::tests`; `api::tests::playtest_tests`).
- The speed figures are arithmetic on the cited measurements, not a
  measurement of a batch (**S**: PT7's note).

## Not verified

- That the real `Source` (the build, the mesh, `h2 sim batch`) gives the runner what the fake does.
- That `h2 sim batch` ends a child's trace file before reporting it, and that a cancel stops its children.
- That the bots' mesh builder accepts a hand-made level's collision.
- That a sim process can take a second match without a restart.
- The number of processes a machine runs before they slow each other.
- That matches with `bot_threads` off are the matches with it on.
- How many seeds make each measure's spread small enough to act on.
- That the difference layers' two-deviation rule calls out the changes a person would on a real edit, such as a moved spawn.

## Build slices

| # | Slice | Owns | Tests | Accepted when |
|---|---|---|---|---|
| PT1 | Static checks and `playtest.static` | `studio/src/app/playtest_static.rs`, `studio/src/api/playtest.rs`, [agent-api.md](agent-api.md) | on the invented map with sectors: an off-mesh spawn, an island, two spawns in sight and a missing flag are each found | the checks list the stage's layout as ready for Slayer |
| PT2 | The spatial trace: `detail = "spatial"`, the observers, the file | `crates/live/sim-bench/src/{spatial.rs, config.rs}`, `crates/extensions/game/src/sim_glue.rs`, [sim.md](../runtime/sim.md) | the file round-trips; over fake guest memory each event is written once with its places; the tally is unchanged with the trace on | a sim match writes a trace whose kills are the report's |
| PT3 | The measures | `sim-bench/src/spatial/{heat.rs, spawns.rs, flow.rs, sight.rs, items.rs, stuck.rs}` | each measure on invented traces, with too few events refused | `h2 sim` prints the measures of a trace |
| PT4 (the runner's logic, its API and its tests are built; its real `Source` and the save hook are not) | The runner: build, nav, the batch, the gather, the cache, `playtest.run`, `status`, `result`, `cancel`, on save | `studio/src/app/playtest.rs`, `studio/src/api/playtest.rs` | a fake batch: the files written, partial results, a cancel, a stale mark on a new build key, a cache hit | `playtest.run` on a retail map gives a result |
| PT5 (built) | The overlays and the panel | `studio/src/viewport/playtest.rs`, `studio/src/ui/playtest.rs`, `view.layers` | each layer counts its marks and changes the snapshot's pixels on an invented result | the layers draw over a map in the window |
| PT6 (built) | Before and after: `playtest.compare`, difference layers, the baseline, swapped sides | `sim-bench/src/spatial/compare.rs`, `studio/src/app/playtest.rs` | two invented results: a change inside the spread is not called out and one outside it is | moving a spawn shows its fairness change |
| PT7 | The pool: `match.start`, kept children, the sizing, the measured speeds | `crates/extensions/game/src/sim_glue.rs`, `crates/apps/cli/src/sim_cmd.rs`, `studio/src/app/playtest.rs`, `docs/editor/notes/playtest.md` | a second match on a fake sim starts from a clean tally; the pool is sized from cores and memory | the note has matches a minute, cold and warm |
| PT8 | Fresh maps: the readiness list, the mesh on a hand-made level, the stage as the test | `studio/src/app/playtest_static.rs`, the bots' mesh builder, `content-kit` (its `stage` module) (a playable bay) | the stage's ramps and ledges give the links the layout expects | a probe plays on the stage |

PT1, PT2 and PT3 need nothing. PT4 needs PT2 and the workspace's W2.
PT5 needs PT3. PT6 needs PT4 and PT5. PT7 needs PT4. PT8 needs PT1 and
the stage's S2.

## Related

- [sim.md](../runtime/sim.md), [sim-budget.md](../perf/sim-budget.md), [sim-rewrite.md](../engine/sim-rewrite.md): the sim, its cost and where the cost goes
- [bots.md](../game/bots.md): the players and their mesh
- [tag-workspace.md](tag-workspace.md): the build a run plays
- [content-stage.md](content-stage.md): the first fresh map
- [game-view.md](game-view.md): the game played by hand, beside this
- [rules-start-gate.md](../game/rules-start-gate.md): what a game type needs of a map
