# The forked bounce search

Status: Partly built. The search, the fork and the command run on a real machine: a smoke run forked and restored with every RAM check equal. Costs and trial outcomes are not yet measured ([Not verified](#not-verified)).
Code: `crates/live/sim-bench/src/bounces.rs` (`Job`), `crates/extensions/game/src/sim_bounces.rs` (`tick`, `between_slices`), `crates/apps/cli/src/sim_cmd.rs` (`bounces`). Tests: `crates/live/sim-bench/src/bounces/tests.rs`.
Switch: none.

`h2 sim bounces <file>` runs superbounce trials on a map's candidate seams
from a saved machine. A [seam drop](sim.md#seam-drops) spends about 300
ticks placing, settling and falling before the one tick that decides it.
The search runs that setup once a seam and a Havok state, saves the whole
machine a few rows before the touchdown, and starts every trial from the
save: seven ticks a trial wave instead of 300. It samples the last airborne
row's seven inputs, finds the region that bounces, and ranks the seams by
how much error a player may make. The one rule: a trial must come out as
the same inputs do in a drop that was never forked, and the run checks it.

## How it works

### One trial

A seam's outcome is decided by the body's state on its last airborne row
([2026-11-22-seam-drop-depth.md](../game/notes/2026-11-22-seam-drop-depth.md)).
A trial sets that row directly. Its inputs are a point on seven axes
(`bounces::AXES`), in the site's own frame (`Site::place`):

| Axis | What | Default range | Grid |
|---|---|---|---|
| `across` | the origin across the edge, along its normal | -0.10 to 0.10 | 0.0005 (1.5 mm) |
| `along` | the origin along the edge | -0.05 to 0.05 | 0.0005 |
| `depth` | the origin's height against the anchor height | -0.09 to 0.09 | 0.0005 |
| `v_drift` | speed along the site's drift | 0 to 0.8 | none |
| `v_side` | speed to the drift's left | -0.1 to 0.1 | none |
| `vz` | speed down | 5.0 to 6.6 | none |
| `yaw` | facing, degrees left of the drift | -45 to 45 | none |

The anchor height is the origin's height over the site's floor on the last
airborne row of the first variant's own drop. `depth` covers one tick of
fall at 5.4 units/s (0.18), so it is the sub-tick phase: every phase a
real fall can touch down in is in the range.

`TrialRun` steps a trial a row a tick:

1. Row 0: a teleport to the start, through the agent's guest calls as a
   drop's lift is, and the pair word. The start is the last row moved back
   `flight` rows along a free fall (`bounces::back`, the drops' fall model).
2. The row the teleport lands: the origin must be within
   `PLACE_TOLERANCE` (0.005) of the start. A landing further off is asked
   again once, aimed against the miss (`TrialRun::aimed`); a second miss
   is `setup_failed` with the miss on each axis (`teleport: landed 0.012
   off (x +0.000 y +0.000 z +0.012) with its aim corrected`). Then the
   start's whole velocity is written (the object's and each Havok
   motion's, `sim_bounces::velocity`).
3. `flight` rows of fall (default 1), the horizontal velocity held; the
   last one writes the whole planned velocity, so the tick that follows
   runs from exactly the asked state.
4. The touchdown and three rows on. The verdict is the drops': the best
   vertical speed of those four rows at `BOUNCE_VZ` (3.5) or over is
   `bounced`; a pair that left the touchdown tick without its -3 is
   `prime_lost`; anything that never touched down is `setup_failed`.

The word is -3 on every row up to the touchdown (0 for an unprimed
control), and crouch is held on the pad when the point asks.

Every position, the start included, is the unit's origin (object
`+0x64`), the point the teleport places (`sim_drops::body`). The
bounding sphere's centre (`+0x30`) is about 0.4 units over it and moves
with the pose and facing, so a start read from it never lands. The
game's placement puts the origin above the point asked for:
`PLAYER_PLACE_NEAR` hands `0x14e200` the point plus the up vector
(`[0x4687b0]`, (0, 0, 1)) times `[0x47f05c]`, 0.0328 (**E**
`0x14e9ae..0x14ea02`). Each base drop's lift, a teleport into the air as
a trial's is, measures the miss, and the site's trials ask against it
(`SiteJob::aim`), so their first teleport lands. A trial that had to ask again keeps the aim it landed
with for the site's next trials, is marked `reaimed` in its record, and
its wave is no replay check's. The run's notes name a measured miss over
the tolerance.

### The fork

A fork saves the whole machine in memory and puts it back. Guest RAM goes
through the Theater ring's page pool over the write watch
(`crates/extensions/game/src/theater/ring.rs`, `Ring::capture` and
`Ring::restore_ram`): a capture copies the pages written since the last
one, a restore copies back only the pages that differ. Every other part
(the kernel and its threads, the CPU contexts, the devices, audio, the
features) is saved and loaded by its snapshot section, the way a film's
warm seek does (`BootMachine::save_ring_sections`,
`BootMachine::load_ring_sections` in
`crates/extensions/game/src/snapshot_glue.rs`). A ring entry is one Havok
variant's base state; its key is the variant's index.

The fork runs between two slices, at the point `state.save` uses
([snapshots.md, "The rule"](snapshots.md#the-rule)). The `sim: bounces`
hook is a `before` on the game tick: it reads the seats, carries out
their acts and keeps the job's ask. The tick observer `sim: bounces fork`
(last) marks the ask due once the tick has returned
(`sim_bounces::fork_due`) and ends the slice
(`Runtime::request_preempt`), and the boot loop calls
`sim_bounces::between_slices` with the machine at hand
(`crates/apps/boot/src/lib.rs`, beside `Agent::run_snapshots`). So the
saved state is a few transfers after the tick that asked, with that
tick's acts in it, and a restored machine carries on into the next tick.

The slice can't end from the `before`. While the tick runs, the game
thread is inside the tick chain's after-hooks continuation (the Rust
closure that runs the chain's `after` hooks and observers when the
original returns, `hooks::install::AFTER_HOOKS`), and the kernel refuses
every save while one is live (`thread N: continuation feature: after
hooks`). A slice that ends inside the tick for any other reason (its time
ran out) leaves the ask waiting: `between_slices` takes only a due ask.

Why this and not the others (**Design**):

- **The engine's checkpoint** does not cover Havok. The Havok heap is
  outside the checkpoint arena
  ([sim-rewrite-havok.md, 3.1](../engine/sim-rewrite-havok.md#31-the-havok-heap-is-not-in-the-checkpoint-arena)),
  and the pair word and the contact caches a trial tests live there.
- **Copy-on-write RAM alone** (the second executor's snapshot or its alias
  and journal, [second-executor.md](second-executor.md)) rewinds memory
  and nothing else. A game tick is not a leaf call: it runs the scheduler
  and kernel calls, which the second executor refuses, and rewinding RAM
  under a kernel and thread contexts that have moved on leaves the machine
  inconsistent.
- **File snapshots** (`state.save` and `state.load`,
  [snapshots.md](snapshots.md)) are the same sections with all of RAM
  encoded and decoded each time. A trial wave is seven ticks; a 64 MB
  decode a wave would cost more than the ticks.

The sections are saved in a keyframe's scope (`theater::keyframe_scope`),
so parts that refuse an ordinary snapshot serialise as they do for a film
keyframe. The fork asks each part for its blockers and skips the machine's
part-less checks (`sim_bounces::blockers`): in a keyframe's scope those
refuse a signed-in Live HLE with no film tape, which every sim has. A
fork needs no tape. No service call is in flight between two ticks of a
running match, and nothing outside the machine is rewound.

A part that is live answers `Busy`: a capture is asked again a row later
while the lead allows, then that variant has no base on the batch's sites,
by name. A restore is asked again up to `RESTORE_TRIES` times. The run's
`fork.busy` counts the busy answers and `fork.last_busy` names the last
one. A restore that fails part way stops the run (`Forked::Broken`): the
machine is part loaded.

A fork asked on a tick whose agent guest calls ran (a teleport answered)
is busy once. The `agent-calls` hook answers that tick with an async body
that runs the tick again inside itself
(`agent::sim::calls::run_batch`), and a hook that returns early skips its
observers, so the tick observer sees only the inner tick return and ends
the slice while the async body is still live. The restore is then asked
again on the next tick, one tick later. Only a wave that ends on its
teleport's row asks then: every trial failed at its teleport. A wave
whose trials fly ends on launch rows that make no calls.

### A batch, a variant, a wave

`Job::tick` drives every seat. The run's sites are split into batches of
at most one site a seat whose members are 1.2 units apart
(`bounces::batches`), so no two bodies meet. For each batch and each
variant:

1. **Calibrate.** Each seat runs the variant's base drop in full
   (`drops::DropRun`), which gives the row its fall touches down on, the
   rows between its arm and its release, and its own last airborne row.
2. **Settle and launch.** Each seat is placed and settled again and held.
   The seats are let go on their own ticks, so every body is `lead` rows
   (default 4) before its touchdown on the same tick.
3. **Capture.** The machine is saved on that tick, under the variant's key.
4. **Waves.** A restore, then one trial a seat at once, each on its own
   site, then the next restore. A wave is seven ticks at the defaults.

The variants (`[[bounces.variant]]`) are different base drops: a long
fall, a short hop, and a long fall by a body that slept first (its pair
rebuilt). They leave the body's pairs, contact caches and sleep state
different at the fork. Every variant runs the same points, so a point's
outcomes differ only by the Havok state.

### Sampling

The first round is a Latin hypercube (`sample::lhs`): each axis is cut
into as many strata as there are points and every stratum holds one, so
400 points cover each axis at 400 values. `sample = "grid"` runs a full
grid instead. Positions are rounded to their 1.5 mm grid. The points come
from the run's seed and the site's name (`sample::seed_for`), so a site
samples the same points whichever shard it lands in.

Each refinement round (`sample::refine`) takes, for every point, its
nearest point of the other outcome, and adds the midpoint of each such
pair that has no other point between its two (inside the ball whose
diameter is the pair), the widest pairs first. A pair nearer than 0.05
human steps is left alone. Three rounds halve the uncertainty of the region's edge
three times where it is widest.

### The region and the ease

A point's class is over every variant that has a base (`region::class`):
`all` (bounced on every one), `some`, `none`, or `unknown` (no outcome:
the prime was lost or the setup failed each time).

The human steps (`bounces::HUMAN`, the table's `[bounces.human]`) say what
a player's error is counted in: 0.01 units (3 cm) of position, 0.1 units/s
of speed, 10 degrees of facing.

- **The window** (`region::window`) is the largest cube, in human steps,
  centred on an `all` point, that holds no `some` or `none` point and does
  not leave the sampled ranges, with at least `min_support` points inside.
- **`ease`** is the window's radius: how many human steps every input may
  be off at once, either way, and still bounce on every variant. It is
  the number sites are ranked by.
- **Per-axis tolerances** (`Window::bounds`): each axis's own range with
  the others kept within the window, to the first sampled failure either
  way. They are never narrower than the window, and wider than the true
  region where no failure was sampled beside it.
- **The press** (`report::Press`), in what a player controls:
  `position`, the room along the drift for the place the fall starts
  from; `sideways`, the room across it; `speed`, the range of speed along
  the drift; `timing_ticks`, the position room over the distance a tick
  covers at the window's speed, so how many ticks early or late a walk-off
  may be; `phase_share`, the share of sub-tick phases that bounce at the
  window's aim; and `height`, the drop whose touchdown speed is the
  window centre's.
- **`reproducibility`** is the `all` points over the points that bounced
  on any variant: 1.0 means the Havok state never changed an outcome.

A site's verdict: `provable` (it has a window), `fragile` (points bounced
on every variant but no window has the support, or only some variants
bounced), `none`, or `untested` (no variant got a base).

## Interfaces

### The table

A match file with a `[bounces]` table (`bounces::BouncesConfig`). The
table fills what the file leaves out, as `[drops]` does: the map, 15
"Bouncer" seats, no time limit, team play and a training report.
`docs/runtime/sim-examples/ascension-bounces.toml` is the example.

| Key | Default | What |
|---|---|---|
| `targets` | `"scan"` | the map's `.spots` bounce candidates, or a list of `[[bounces.targets]]` as a drops file's |
| `max_targets` | all | scan: the first so many |
| `along` | 1 | sites an edge (1: its middle) |
| `points`, `rounds`, `refine` | 400, 3, 200 | the first sample, the refinement rounds, the points a round adds at most |
| `sample` | `"lhs"` | or `"grid"`, with `[bounces.grid]` giving the values an axis |
| `[bounces.axes]` | the table above | `name = [low, high]` or `[low, high, step]`; `[v, v]` holds an axis |
| `[bounces.human]` | 0.01, 0.1, 10 | the human step of an axis |
| `[[bounces.variant]]` | fall, hop, slept | `name`, `height`, `carry_along`, `rest` (rows at rest before the arm) |
| `crouch` | `"off"` | `"on"`, or `"both"` (every other point) |
| `replay`, `unprimed` | 8, 16 | points run a second time a variant; points run once with the word 0 |
| `lead`, `flight` | 4, 1 | rows before the touchdown the machine is saved; rows a trial falls |
| `verify` | `"first"` | which batches replay a wave and compare RAM: `"first"`, `"all"`, `"none"` |
| `bounce_vz`, `retries`, `min_support`, `seed`, `seats`, `out` | 3.5, 2, 8, 1, 15, `results/<stem>` | as named |

A file with `superbounce`, a `[drill]`, a `[train]` or a `[drops]` beside
it is refused by name (`BouncesConfig::problems`).

### The command

```sh
h2 sim bounces docs/runtime/sim-examples/ascension-bounces.toml --parallel 4
```

The command resolves a scan into explicit targets, splits them over
`--parallel N` match processes by weight (`drops::shard_targets`), runs
them as a drops batch runs, and pools their `bounces` report blocks
(`report::summarize`). `--dry-run` prints the plan; `--merge` merges the
runs' verified files into the shared bots directory; `--keep-data`,
`--disc`, `--out`, `--data`, `--tier` and `--pace` are a batch's.

A seat runs one trial a wave on its own site, so a run uses as many seats
as its batch has sites. Fifteen sites a process fills every seat: 60 sites
want `--parallel 4`.

The results directory holds:

- `bounces.json` (`report::BouncesReport`): every site ranked, with its
  verdict, ease, window, per-axis bounds, press, class counts, each
  variant's base drop and trial counts, and the fork's counters.
- `bounces.md`: the ranked table.
- `bounces.verified`: the provable sites as a verified file
  ([superbounce.md, Verifying spots](../game/superbounce.md#verifying-spots)):
  the stand, the window's aim as the offset, the height. Each run also
  writes its own into its copy of the map's `.verified`
  (`sim_bounces::save`), which `--merge` or `h2 sim harvest` merges.

`match.status` gives a run's progress in `bounces`
([agent-api.md](agent-api.md#sim-matches)).

## Rules and limits

- **The fork is checked in the run** (`verify`). The anchor trial puts a
  variant's own last airborne row back by a trial's means, and its outcome
  is set beside the full drop's (`drop = anchor` in the table). The first
  two waves of a variant are the same anchors, and guest RAM's hash at
  their ends must be equal (`fork.replay_mismatches`). After a checked
  restore RAM's hash must be the capture's (`fork.ram_mismatches`). Either
  mismatch is a report error and fails the command.
- **Replays** run sampled points a second time on a variant
  (`replay_mismatches`), and **unprimed** points run with the word 0: one
  that bounces is a bounce the word did not make (`unprimed_bounced`).
- **A trial that loses its prime or fails its setup is no outcome.** It
  is neither support nor a failure for a window.
- **A window is a statement about the points sampled.** No failure was
  seen inside it; the space between points is not proven. `support` says
  how many points it rests on.
- **The host must fork.** A host whose loop never answers a fork stops the
  run with `fork: no answer to a capture` (`FORK_WAIT`).
- **Nothing outside the machine is rewound.** The sim's tally sees the
  game tick go back at every restore. A bounce search's report has no
  meaningful match metrics, and a film must not record it.
- At most 8 variants (`MAX_VARIANTS`): the ring keeps twelve entries.

## Evidence

- **Run**: the last airborne row decides the bounce, and about half the
  tick phases bounce on the Ascension seam
  ([2026-11-22-seam-drop-depth.md](../game/notes/2026-11-22-seam-drop-depth.md)).
- **Code**: the Havok heap is outside the engine's checkpoint
  ([sim-rewrite-havok.md](../engine/sim-rewrite-havok.md)).
- **Test**: the job over a made-up machine whose fork is a copy of its
  state: one capture a variant a batch, every trial from the fork, the
  anchor and replay checks, busy and absent forks by name
  (`the_job_forks_once_a_variant_and_runs_every_trial_from_the_fork`,
  `a_busy_capture_is_asked_again_and_then_given_up_by_name`,
  `a_host_with_no_fork_stops_the_run_by_name`).
- **Test**: a slice a `before` on the tick ends stops inside the chain's
  after-hooks continuation, and one a tick observer ends stops with
  nothing live (`a_slice_asked_before_the_tick_ends_inside_its_after_hooks`,
  `a_slice_asked_by_a_tick_observer_ends_with_nothing_live` in
  `crates/hooking/hooks/tests/install.rs`); an ask is carried out only
  after its tick returned (`a_fork_waits_for_its_tick_to_return` in
  `sim_bounces.rs`).

### Cost

Arithmetic over measured tick times, not a measurement of the search. A
Lockout 2v2 sim tick on the clang tier at `H2_OPT=2` is 3.12 ms alone and
5.09 ms with four processes
([notes/sim-10x.md](../perf/notes/sim-10x.md#2026-11-22-batch-throughput-at---parallel-1-4-and-8-on-the-quiet-laptop)).
No note records a fast-tier sim tick or a ring restore.

| | Ticks a trial, 15 seats | At 5.09 ms a tick |
|---|---|---|
| a seam drop (about 300 ticks) | 20 | 102 ms |
| a forked trial (7 ticks a wave) | 0.47 | 2.4 ms, plus a fifteenth of a restore |

The example's 60 sites at four processes: 15 sites a process in one batch
each, 3,046 waves of seven ticks, so about 21,300 ticks and 3,046 restores
a process, and about 1,800 ticks of calibration. At 5.09 ms a tick that is
about two minutes a process for up to 182,760 trials, plus the restores
and a run's 12 to 15 s of setup. A restore copies the pages seven ticks
wrote and loads every section; its time is the open number.

## Not verified

- That the machine is quiet enough to save at a tick boundary in a sim
  host: the kernel's and the features' blockers, with the keyframe scope.
  A blocker is named in the base's `why`, and the run's `fork.busy`
  counts them. The features the first Ascension run named as not
  audited have policies (snapshots.md, "Feature policies"); a test shows
  a slice ended by a tick observer stops with no continuation live
  (`crates/hooking/hooks/tests/install.rs`), but no sim has forked yet.
- That skipping the part-less checks is safe for a signed-in Live HLE.
- That a restored machine runs the same ticks as the captured one: the
  replay and RAM checks measure it on every first batch.
- That a teleport and a velocity write two rows before the touchdown
  leave the pair and its word in place, and give the outcome a real fall
  from that state gives: the anchor trial against its full drop.
- That the seats' lift latency is steady enough for every body to be
  `lead` rows from its touchdown on one tick.
- The time of a restore, of a fast-tier tick with the write watch on all
  of RAM, and so the trial rate.
- That the ring's entries for eight variants fit in memory beside eight
  processes.
- The ease numbers against a person: whether the human steps are the
  right size.

## Related

- [sim.md](sim.md): sim mode, and [Seam drops](sim.md#seam-drops), whose
  drop, targets and verdict the search uses.
- [snapshots.md](snapshots.md): the sections and the quiescence rule.
- [theater-design.md](../game/theater-design.md#the-session-ring): the
  ring the fork's RAM goes through.
- [superbounce.md](../game/superbounce.md): the pair word, the touchdown
  tick and the verified file.
