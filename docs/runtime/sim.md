# Sim mode: the game's simulation without rendering

Status: Built. Unit-tested over pure inputs and fake guest memory, and run
against the game: [simulation budget](../perf/sim-budget.md) has the measured
matches. [Not verified](#not-verified) lists what a run has not settled.
Switch: `H2_SIM`, `sim.render_every` and the `impl.sim.*` switches, listed in
[switches.md](switches.md).

Code: `crates/live/sim-bench` (the match, batch and tuning files, pacing, the
tally and report, the comparison, the spatial trace), `crates/live/bots/src/tuning.rs`,
`crates/rewrites/sim-bridge/src/sim_frame.rs`, `crates/extensions/game/src/sim_glue.rs` and its
System Link lobby, `crates/extensions/game/src/sim_census.rs`, `crates/apps/cli/src/sim_cmd.rs`, and the examples in
[sim-examples/](sim-examples/).

The goal: train and test the bots offline. Run the retail game's own
simulation (physics, weapons, damage, AI, game types) with Clankers in it,
but:

- no rendering and no present: no NV2A work, no GPU, no window;
- no audio output;
- faster than real time;
- several matches in parallel;
- each match set up by a script and scored by metrics, so bot changes can
  be compared and tuned offline.

Labels, as elsewhere: **Code** is read from this repository (file given),
**Doc** is stated by another page (linked), **S** is a guess or a design
choice to check before building on it.

## Pages

| Page | Holds |
|---|---|
| [Sim mode: setting up a match](sim-match-setup.md) | The playerless host and start, the match file, rules scripts, Firefight and bot parameters. |
| [Sim mode: metrics and the spatial trace](sim-metrics.md) | The tally and report, the spatial trace, sim stores and shared guest RAM. |
| [Sim mode: parallel runs and A/B](sim-parallel.md) | Batches, arm swaps, Uplink watching and batches on hosts. |
| [Sim mode: bounce practice and tuning](sim-bounce.md) | The bounce drill, sweeps, site training and seam drops. |
| [Sim mode: state hashes and bot navigation cost](sim-determinism.md) | Post-tick hashes, finding the first differing state, bot navigation performance and the host audit corpus. |

## 1. What a headless run does today

`h2 run` and `h2 serve` are headless: `boot::run_hosted` with
`Host::default()` (crates/extensions/game/src/run_opts.rs). Per frame of gameplay, this is
what runs on the one host thread that drives every guest thread
(**Code**, numbers **Doc** from [gameplay-budget.md](../perf/notes/gameplay-budget.md),
Old Mombasa, "Render front-end versus simulation"):

| Work | Where | Runs headless? | Old Mombasa, ms/frame |
|---|---|---|---|
| The tick: players, AI, scripts, objects, Havok (`game_tick` `0x00137fe0`) and the per-frame work around it (input poll, frame timing, network, director, observer) | guest code | yes | 6.2 (1000 draws) to 7.8 (2000 draws) |
| The render front-end: `render_frame_and_present` `0x00223240` and everything under it (views, passes, objects, lights, the push buffer) | guest code | **yes** | 16.3 to 18.9 |
| Our D3D layer called from it: entry wrappers, state resolve, stream copies, push-buffer decode | `crates/platform/d3d` HLE | **yes**, into a `RecordingBackend` that forgets every call | 5.2 to 7.7 |
| GPU: encode, submit, present, read-backs | `crates/platform/gfx`, wgpu | no (only with `H2_BOOT_GFX` or a window) | 0 |
| Vblank interrupt (60 Hz DPC, `0x0012b2a0` bumps the vblank counter) | kernel periodic source | yes | tiny |
| The frame wait (`0x0012b2e0`, `FrameWait::HALO2_CLOCKED`) | d3d HLE | yes, but on the logical clock: an idle advance, no wall time | 0 |
| DirectSound: the mixer rendered on a logical 48 kHz grid into a `NullSink`, packets completed | `crates/platform/dsound`, `crates/platform/audio` | **yes** | part of "kernel, XAPI, scheduler, DirectSound": 1.5 to 2.1 |
| The sound service (`0x00125b40`, guest) | guest code | yes | in the tick's share |
| XAPI input pump between slices | `crates/platform/xapi` | yes | small |
| The kernel trace file (`--out/kernel-trace.tsv`, one row per kernel call) | `boot::trace` | yes in `h2 run` | 0.9 |
| Telemetry, lockstep, agent observers | features | only when switched on | small |

So a headless gameplay frame is about 28 to 34 ms of host time, and
**about three quarters of it is the render front-end and the D3D layer
under it**, which the simulation does not need. The GPU is already out.

### Where the frame loop meets time

**Doc** ([high-framerate.md](../game/high-framerate.md) §1,
[d3d.md](../graphics/d3d.md) "Vertical blank",
[BOOT.md](BOOT.md)):

- The kernel clock is logical (`kernel::clock::Clock`). It never reads the
  host clock. It moves on by executed work (guest accesses, charged per
  slice) and jumps to the next deadline when every guest thread waits
  ("idle advance"). `rdtsc` and `KeQueryPerformanceCounter` follow it.
- Vblanks are a 60 Hz periodic source on that clock. Halo's frame wait
  (`0x0012b2e0`, inside `render_frame_and_present`) yields until the
  vblank counter reaches its frame target. Frame timing (`0x0012b0e0`)
  turns the vblanks since the last target into `dt`; the tick budget
  (`0x00146660`) turns `dt` into whole ticks. With the interval word at 2,
  a frame is two vblanks, 1/30 s of game time, and exactly one tick.
- `H2_BOOT_REALTIME=1` sets `kernel::pacer::Pacer::real_time()`, which
  sleeps the host until wall time catches up with logical time. **It is
  off by default: headless runs are already unpaced.** They go as fast as
  the host executes the guest's work; idle time costs nothing.
- Logical time per frame is the guest's charged work (about 9.4 ms on Old
  Mombasa, with the render) plus idle advances to the vblank. While the
  charged work stays under two vblanks (33.3 ms), every frame is exactly
  one tick. Skipping the render lowers the charged work, so the cadence
  stays one tick a frame, and becomes sturdier, not weaker.

So "faster than real time" needs no clock work: the clock is already
fixed and unpaced. What limits speed is host work per tick, and three
quarters of that is rendering.

## 2. What can be skipped, and what can't

The pipelined-render census measured exactly which guest memory the render
front-end writes and the simulation then reads
([pipelined-render-census.md](../graphics/notes/pipelined-render-census.md), "Every dependency
line" and "The new dependency sites"). Skipping the render is the extreme
case of running it on another thread: it never runs. Each dependency line,
and what sim mode does with it:

| What the render writes that the sim reads | Census class | In sim mode |
|---|---|---|
| **Frame timing**: the frame wait writes the vblank count at the wait (`0x004e63a8..`), which frame timing turns into `dt` | (c), tick-visible | **Made in Rust.** A skipped frame makes the wait's writes and registers itself (`engine::render_loop::frame_wait`, from `sim_bridge::sim_frame`) and yields its slice to the kernel (`NtYieldExecution`) between checks, so the clocked vblank source moves the counter. Without it `dt` is wrong. |
| **Present counter** `0x00485aa0` (64-bit), added to by `rasterizer_present`; the tick's effect code reads it | (b) | **Made in Rust**: +1 per skipped frame (`engine::render_loop::tick_writes`, the render frame step's source too), checked on rendered frames ([below](#render_every)) |
| **Render-frame counter** `0x004ba034`, added to per `render_views`; the particle update reads it | (b), cosmetic | **Made in Rust**: +1 per skipped frame |
| One-shot byte `0x004e6098` the render clears per view, the sim sets | (c) | **Made in Rust**: cleared per skipped frame |
| Object visit marks (counters `0x004de2fc`, `0x004e0308`, `0x004e7414`, `0x004e7c1c` and their stamps) | (a) | Nothing to do: the stamps only need to differ from the counter, and with no render only the tick advances them |
| Object `+0xcc` render-state handle, light render fields, the render-state cache | (a), false sharing; the tick's effect update calls `0x0003ddd0` too | Nothing to do: a frame writes the render's own copy and puts the tick's bytes back (`game::render_scratch`, [sim-render-split.md](../engine/sim-render-split.md), "6. Render scratch and stamps") |
| Object node matrices: the setup's pose request evaluates a dirty pose (`0x000bd7b2`); marker lookup reads the matrices | (c) | **Made in the tick**: the tick builds every dirty pose at its end (`engine::tick_poses`), so a skipped and a rendered run hold the same matrices ([sim-render-split.md](../engine/sim-render-split.md), "2. Node matrices") |
| Texture, geometry and animation caches, and the async I/O pool behind them | (c), structural | **The open risk.** The render drives texture and geometry streaming, which the sim does not use (collision is in the tags). The animation cache is used by both: the tick *requests* (`0x001236f0`), the render *gets* and loads (`0x001237e0`). Whether a sim that never gets still has every animation resident is **Run**. The fallback is `render_every` (below). |
| Particle systems (`0x00174420` updates them while drawing) | (c), cosmetic | Particles do not feed gameplay. They age only through the sim's own updates: the view stamps stay in a render-private table and the location move reads every location as seen ([sim-render-split.md](../engine/sim-render-split.md#6-render-scratch-and-stamps)). |
| **The shared random seed** `[0x004e7408]+0`: the render steps it (object code `0x001032a0` that the render reaches) | (c) | **Changes the random stream.** A sim run is reproducible against another sim run, never against a rendered run: the render's draws are no longer in the sequence. See [Determinism](#8-determinism). |
| Game-time bracket (`0x00146de0` on entry to `render_views`, `0x00146b80` on exit) | (d), not identified | **Not verified**: the one census item that could be real sim state. A tick-hash comparison between a skipped and a rendered run answers it ([Not verified](#not-verified)). |

Outside the render front-end:

- **Menus and loading screens are not skipped.** The skip only takes the
  world path of `0x00223240` (the same test as the prototype's
  `serial_reason`: engine globals active, not the UI's world skip, both
  resource-cache countdowns at or below 0). Menus, the loading screen and
  Bink keep the original, so loading drives the caches exactly as before.
- **The vblank source stays.** It is the clock the frame wait counts.
- **DirectSound stays.** The game's sound code polls buffer positions and
  waits on completion events, and the logical mixer is what moves them
  ([dsound.md](../audio/dsound.md) "Boot integration"). Output already goes
  to a `NullSink`: no device, no sound. The mixing itself (resampling,
  filters, reverb) is host work that only produces samples nobody hears;
  a "positions only" mixer that advances voices without making samples
  would save it, but it changes `crates/platform/audio`'s voice cursors and needs
  its own A/B. **Not built**: it is under 2 ms a frame (row 10 above).
- **The kernel trace file is off** in sim mode (`trace: None`): 0.9 ms a
  frame of writes nobody reads.
- **Movies are skipped** (headless and level boots already skip them).

### `render_every`

`sim.render_every = N` runs the original `render_frame_and_present` on
every Nth world frame and skips the rest (0, the default: never in a
world frame). It is the fallback if a run shows the sim depends on the
render more than the census says (animation residency, the game-time
bracket): N = 30 renders once a second, keeping the caches warm at about
3% of the render's cost.

Each world frame the original renders also checks the skip's writes:

| Step | Where |
|---|---|
| read the three tick words before the frame | `SimFrame::decide` |
| read them after it, compare with `tick_writes::Words::predict` | `SimFrame::check`, an after-observer on `RENDER_FRAME` |
| skip the check when a cache countdown runs (no present) | `SimFrame::check` |
| count a difference by word; render every later world frame | `fell_back` |

The report's `frames` carries `checked`, `mismatch_frame_counter`,
`mismatch_one_shot`, `mismatch_present_counter` and `fell_back`. With
N = 0 nothing is checked.

## 3. Pacing

| Pace | `sim.pace` | What it does |
|---|---|---|
| Unpaced (default) | `"unpaced"` | No pacer. The host runs the guest as fast as it can. |
| Real time | `"realtime"` | `Pacer::real_time()`: logical time held to wall time, as `H2_BOOT_REALTIME=1`. For watching a sim run through the agent API at game speed. |
| Scaled | `"x4"`, `"x0.5"` | A pacer at that multiple of real time (`Pacer::ticks_per_second` = 10 MHz times the factor): the run never goes faster than that. |

The pacer only makes the host wait; schedules and guest state are the same
paced or not (`crates/runtime/kernel/src/pacer.rs`). So a paced and an unpaced run
of the same match are the same match. `sim::clock` holds the parsing and
the arithmetic (tested); boot sets `Machine::pacer` from it. Pass
`--pace realtime` to `h2 sim run` or `h2 sim batch` to override the match
file for this invocation. A real-time light host can group its wall waits
on the pack slot’s 30 Hz phase. Socket readiness releases the grouped wait
at the ordinary deadline. The guest still observes each intervening vblank,
audio completion and thread deadline. [Switches](switches.md)

The report's speed-up is game time over wall time from the match's first
tick to its end (`sim::clock::SpeedMeter`), and also over the whole
process (boot and load included).

## 4. Sim mode

`H2_SIM=<match file>` on the boot path, or `h2 sim run <match file>`.
What it changes (**Code**, `crates/extensions/game/src/sim_glue.rs`,
`crates/rewrites/sim-bridge/src/sim_frame.rs`):

1. **The light host can skip the render front-end** in world frames. The `sim-frame`
   feature implements `RENDER_FRAME` (`0x00223240`). At each entry it
   decides as the prototype does (the world path, the countdowns,
   `render_every`). Not skipping: the generated original. Skipping: the
   frame wait in Rust on the game thread (yielding to the kernel until
   the vblank), then the tick writes from the table above,
   then a plain `ret`. Every other feature's hooks on `RENDER_FRAME` (the
   bots' pads before it, their views back after it) run as they would
   around the original. The host's present count (`d3d_diag::presents`,
   which `H2_BOOT_STOP_AT_PRESENT`, the input script and the agent stamp
   count) moves on by one per skipped frame. Full headless runs
   (`-host-light`) keep the render entry.
2. **No GPU and no window.** `Host::gfx` stays `None`; `H2_BOOT_GFX` is
   refused. The D3D layer stays installed over a `RecordingBackend`, so
   the game's own D3D calls outside the render (device creation, the
   loading screen, the sim's few calls) are answered as before.
3. **No audio output**: the logical mixer into a `NullSink`, as headless
   already is.
4. **No keyboard pad.** The headless host uses phantom peers for all bot
   input and leaves the controller ports free.
5. **Unpaced** unless the match says otherwise (above).
6. **The agent API keeps working**, and gets `match.status`,
   `match.report` and `match.stop` (below). `session.pause` and
   `session.step N` work as in any run: step N ticks of a sim match.
7. **The match ends the run**: at the game's end latch, at the match's
   tick limit, or at its wall-clock limit, the boot loop's
   `between_slices` returns false and `h2 sim run` writes the report.

## 5. Setting up a match by script

See [Sim mode: setting up a match](sim-match-setup.md).

### The playerless System Link host

The headless host opens a System Link squad and adds each bot as a phantom peer, with one player per peer. Detail: [Sim mode: setting up a match](sim-match-setup.md).

### Match file

See [Sim mode: setting up a match](sim-match-setup.md).

### Rules scripts

See [Sim mode: setting up a match](sim-match-setup.md).

#### Rounds

The report's `rounds` lists the score of every player and every team at the start of each round of the script's game, in round order (`sim_bench::metrics::RoundScores`; absent without a script). Detail: [Sim mode: setting up a match](sim-match-setup.md).

### Firefight

See [Sim mode: setting up a match](sim-match-setup.md).

### Bot parameters from a file

See [Sim mode: setting up a match](sim-match-setup.md).

## 6. Metrics

See [Sim mode: metrics and the spatial trace](sim-metrics.md).

### The spatial trace

`detail = "spatial"` in a match or batch file records where things happened, for Studio's playtest ([playtest.md](../editor/playtest.md)). Detail: [Sim mode: metrics and the spatial trace](sim-metrics.md).

### 6.1 Sim stores: the map without the map

The retail map is never streamed in a sim or light host that uses stores. Detail: [Sim mode: metrics and the spatial trace](sim-metrics.md).

## 7. Parallel runs, A/B and tuning

See [Sim mode: parallel runs and A/B](sim-parallel.md).

### Watching in Uplink

See [Sim mode: parallel runs and A/B](sim-parallel.md).

### Bounce practice

See [Sim mode: bounce practice and tuning](sim-bounce.md).

### The bounce drill

See [Sim mode: bounce practice and tuning](sim-bounce.md).

### Sweeping the bounce tuning

See [Sim mode: bounce practice and tuning](sim-bounce.md).

### Training the bounce sites

`h2 sim train <train file>` searches each bounce site's knobs ([bounce-search.md](../game/bounce-search.md)) by playing the practice match as parallel runs, and reports what the sites learned. Detail: [Sim mode: bounce practice and tuning](sim-bounce.md).

### Seam drops

See [Sim mode: bounce practice and tuning](sim-bounce.md).

## 8. Determinism

What a sim run's result depends on, and what breaks reproducibility:

| Source | Status in sim mode |
|---|---|
| The kernel clock, `rdtsc`, scheduling | logical: the same guest work gives the same schedule (**Code**, `kernel::clock`, `kernel::pacer`) |
| The epoch (system time) | fixed at 0 unless a host sets it |
| The pacer | waits only; never changes the schedule |
| The render front-end | **skipped**, so it does not step the shared seed: deterministic, but a different stream from a rendered run |
| The render thread | none: no GPU backend |
| Audio | logical pacing into a `NullSink` |
| Input | Each phantom observes guest state on the game thread during the simulation tick’s input gather |
| Bot thinking threads (`H2_BOT_THREADS`, `H2_BOT_PIPELINE`) | the guest is read on the game thread only and the thinks are independent, so pads do not depend on the thread count. Batched equals serial. Pipelining is reproducible with pads delivered a tick later, crediting that tick toward any configured wire delay ([bots.md §5.5](../game/bots.md)) |
| Spawn locations | `0x14ef00` mixes the player index and spawn count with two game-options words that the session fills anew for each game, not the shared seed. Sim writes them from the match seed ahead of the first spawn (`crates/extensions/game/src/sim_glue.rs`); without that, two runs start their players in different places |
| The bots' randomness | SplitMix64 of the match seed and logical seat drives individual choices (`sim::config::bot_seed`). The gamertag fixes the profile |
| `HashMap` iteration in the bots (`mind.rs` tracks, `los`, `nav`) | lookups and `retain` only, as read (**S**: worth a check in review) |
| The nav mesh | built in the background on a thread at the wall clock's mercy (`bots_nav_auto`): **off in sim mode** (`H2_BOTS_NAV_AUTO=0`). So a run with bots makes its map's nav mesh and visibility grid in its own `--data` before it starts (`cli::bots_nav::ensure`, the worker `h2 bots nav` runs, in the foreground): a current file there is kept, a finished build in the shared cache (`bots::autonav::shared_cache_dir`, keyed by map file and generator version) is copied, anything else is built; a `firefight-<base>` map uses its base map's. A build that fails stops the run with an error naming the map. A map whose mesh still did not load is reported (`"nav": "missing"`). A built mesh is read on the game thread in sim mode (`H2_BOTS_MAP_INLINE=1`): read on a worker, the bots of a loaded host acted without it for their first ticks, and two runs split soon after the opening hold |
| Wall-clock limits (`max_wall_s`) | can end a match at a different tick on a slower host: the report says `"end": "wall_limit"`. Use `max_ticks` for reproducible ends |
| Our own observers | read-only; the agent's tick observer adds dispatch transfers, the same in every run that has it |

**How far it goes**: the same image, `H2_OPT`, switches, match file and
bot params give the same match, tick for tick: that is the claim, and
`H2_LOCKSTEP` checks it (two runs, `h2 lockstep A B`). Changing a bot's
tuning changes its pads and so the match from the first tick it differs.
A sim run never matches a rendered run of the same match (the seed).

## 9. Agent API

Host methods while `H2_SIM` is on (`boot::sim_glue::Sim::add_methods`,
[agent-api.md](agent-api.md), "Sim matches"):

| Method | Access | What |
|---|---|---|
| `match.status` | read | the match file, phase (`waiting`, `binding`, `playing`, `over`), lobby setup step and session state, ticks, game seconds, wall seconds and speed-up so far, the end reason once over |
| `match.report` | read | the report as it would be written now (players, kills so far) |
| `match.stop` | act | end the match at the next slice boundary, with `"end": "stopped"` |
| `sim.guest_census` | read | the guest census read now (`now`) and the report's `guest_census` block so far (`match`), §6 |

`h2 agent match.status`, `h2 agent match.report`, `h2 agent match.stop`,
`h2 agent sim.guest_census`.
Stepping is the existing `session.pause` and `session.step N`.

## 10. Expected speed-up

Clanker Training uses the [dedicated light profile](../live/dedicated-host-light.md#light-host).
The default skips steady-state world draws, uses descriptor-only texture
handles in the D3D shim, runs the audio clock without mixing, and maps the
disc read-only. Verified server-map copies are picked up from
`H2_SERVER_MAP_DIR`. Distributed System Link seats and packed telemetry
use their ordinary paths. Sim uses the match's pace, with unpaced as the
default. Dedicated serve waits for real-time deadlines.

The per-process cost is the guest tick, replication, telemetry and bot
thinking, with 64 MiB of guest RAM; the module and immutable maps are shared
across processes. Size a batch from the smaller of the available effective
cores and the available memory divided by the measured incremental RAM, and
read `wall.speedup` from an ordinary batch and `host.metrics` for RSS and
CPU. The measured ms/tick per map is in the
[simulation budget](../perf/sim-budget.md); the earlier estimate, and the
memory of one run, are in [notes/sim.md](notes/sim.md).

### Post-tick state hashes

Each new sim report records `state_hashes_v1` in guest tick order. Detail: [Sim mode: state hashes and bot navigation cost](sim-determinism.md).

### Finding the first differing state

`h2 sim diff <a.json> <b.json>` says which state differs first, not only that a digest does. Detail: [Sim mode: state hashes and bot navigation cost](sim-determinism.md).

`h2 sim blind [--ticks N] [--no-arena] <arm=file>...` holds every arm's hash series (a report, or a `sim.hashes` reply) against the first and names the first differing tick and arm: the comparison of the render-blind gate ([sim-render-split.md](../engine/sim-render-split.md), "Running the gate").

#### Bot thinking across cores

See [Sim mode: state hashes and bot navigation cost](sim-determinism.md).

## Host audit corpus

Build the full clang image with the usual optimisation setting. Detail: [Sim mode: state hashes and bot navigation cost](sim-determinism.md).

## Running a match

- `h2 sim` never builds: it stops with `h2 build` when the image is not
  current for this `h2` (`crates/apps/cli/src/sim_cmd.rs`), as `h2 serve` does.
- `h2 lab sim` runs a match or batch on the lab host's image and keeps the
  reports, with per-arm medians and a state-hash comparison, for timings
  taken on the other Mac ([Sim benchmarks](lab.md#sim-benchmarks)).
- `h2 sim run` makes a map's nav mesh and visibility grid in its data
  before a run with bots (`crates/apps/cli/src/bots_nav.rs`, `ensure`);
  the game itself builds none in sim mode.
- Each batch run has its own data directory (`<out>/runs/<id>/data`, its
  `bots/` linked to `--data`'s, Z:'s cache slots cloned from the image's
  template). For supported Map Pack runs the batch stages the selected
  offer from the shared data directory.
- The match's pace replaces `H2_BOOT_REALTIME` (unpaced unless the match
  says otherwise).
- A superbounce batch run's `bots/` is its own, the verified and ghosts
  files copied ([Bounce practice](#bounce-practice)).
- Example match, batch and sweep files are in [sim-examples/](sim-examples/).

## Not verified

- Same-build determinism beyond the example batch: repeated runs of its
  15 matches (Lockout, Midship, Ascension; four phantoms; 9000 ticks) give
  identical components at every tick ([notes/sim.md](notes/sim.md)).
  Other maps, rosters and game types are unchecked.
- Whether skipping the render changes anything the tick reads beyond the
  seed: compare a skipped run with the same match at `render_every = 1`.
  `render_every` is the fallback if it does.
- Cross-profile equality between the light and full profiles
  ([Host audit corpus](#host-audit-corpus)).
- `VARIANT_DEFAULTS` with game types other than Slayer: that the host starts
  the requested variant.
- Phantom team assignments holding through respawns.
- The end latch at the time or score limit ending a run.
- The bounce practice in a sim run: that the cached scan reaches the
  practice through `autonav::load_cached`, that an award's slot is the
  report's player index, and that the practice events' ticks and the
  tally's agree (so `first_bounce_ticks` is right).
- The `guest_census` reads and the `wall.phantom_*` timers in a run: the
  array pointers and names come from the retail XBE, and the reads are
  tested over a fake guest only.
- How many seeds make each measure's numbers steady; the refusal
  thresholds are guesses (**S**).
- The spatial trace in a run: that the starting locations and equipment
  read at the game's start are the map's (tested over a fake guest only),
  that a pickup's placement by distance is the item's, and that a run's
  trace agrees with its report's kills (the check runs at every traced
  run's end).
- A Firefight match end to end: that the game options' scenario path for
  a kit package is its map list slot path (what the sim waits for), that
  the bots load the base map's nav mesh on the kit map, and that the
  phantoms' teams hold in a team game the runtime places actors in.

## Tests

`cargo test -p sim-bench` (config, metrics, compare, clock, the examples
here, and Firefight: `a_firefight_match_toml_parses`,
`firefight_bots_are_on_player_teams_only`,
`two_team_firefight_balances_omitted_teams`,
`the_report_has_rounds_and_score`), `cargo test -p game sim_glue`
(`a_firefight_match_finds_its_kit_package`,
`a_firefight_table_becomes_the_pages_choices`), `cargo test -p bots tuning` and `cargo test -p bots
a_tuned_brain` (the overrides), `cargo test -p sim-bridge sim_frame` (the skip's decision
and stubs over a fake guest), `cargo test -p game sim_glue` (snapshots to
samples, limits and phantom tuning), `cargo test -p game sim_census` (the
census reads over a fake guest and `sim.guest_census`'s answer), and `cargo
test -p cli sim_cmd` (child arguments, per-run data, the results directory,
the Z: template's key, saving, populating, gate and cleanup).
The bounce practice: `cargo test -p sim-bench bounce` and `sweep` (the
tally from synthetic events, the schema, the comparison, the sweep's
candidates and ranking), `cargo test -p bots tuning`,
`the_default_bounce_tuning_plays_as_no_tuning_does`,
`the_practice_reports_each_route_it_starts` and `harvest`, `cargo test
-p game sim_glue` (the `[bounce]` mapping and the events) and
`superbounce_mode`, and `cargo test -p cli sim_cmd` (the run's own
proofs). Training: `cargo test -p sim-bench train` (the `[train]` table, the
plan and shard rule, the report from `.learned` lines) and `cargo test -p cli
sim_cmd` (the arguments, `--reopen`). Seam drops: `cargo test -p sim-bench drops` (the
table, the plan and shards, the queue, each outcome over a fake body
including every `prime_lost` reason, the control gate, the fits and the
verified lines), `cargo test -p live-telemetry havok_agents` (the level
pairs) and `cargo test -p game sim_drops` (the constants it shares).
The spatial trace: `cargo test -p sim-bench spatial` (the file's round
trip and refusals, each event once with its places, the kills against the
tally's, and each measure on invented traces with too few events
refused) and `cargo test -p game sim_glue` (the scenario's places over a
fake guest, `the_tally_is_unchanged_with_the_trace_on`,
`a_spatial_match_writes_a_trace_whose_kills_are_the_reports`), and `cargo
test -p cli sim_cmd` (`spatial_finds_the_traces_of_a_results_directory`).
