# Sim mode: the game's simulation without rendering

Status: designed and built (2026-11-19, branch `sim-engine`), unit-tested
over pure inputs and fake guest memory. Nothing on this page has been run
against the game yet; [What needs a run](#what-needs-a-run) lists what
only a run can settle, and [First runs](#first-runs) gives the commands.

Where it is: `crates/sim` (the match, batch and tuning files, pacing, the
tally and report, the comparison), `crates/bots/src/tuning.rs`,
`crates/feature/src/sim_frame.rs`, `crates/boot/src/sim_glue.rs` (and the
level boot's `MpSetup`), `crates/cli/src/sim_cmd.rs`, and the examples in
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
choice to check before building on it, **Run** needs a game run.

## 1. What a headless run does today

`h2 run` and `h2 serve` are headless: `boot::run_hosted` with
`Host::default()` (crates/boot/src/lib.rs). Per frame of gameplay, this is
what runs on the one host thread that drives every guest thread
(**Code**, numbers **Doc** from [gameplay-budget.md](../perf/gameplay-budget.md),
Old Mombasa, "Render front-end versus simulation"):

| Work | Where | Runs headless? | Old Mombasa, ms/frame |
|---|---|---|---|
| The tick: players, AI, scripts, objects, Havok (`game_tick` `0x00137fe0`) and the per-frame work around it (input poll, frame timing, network, director, observer) | guest code | yes | 6.2 (1000 draws) to 7.8 (2000 draws) |
| The render front-end: `render_frame_and_present` `0x00223240` and everything under it (views, passes, objects, lights, the push buffer) | guest code | **yes** | 16.3 to 18.9 |
| Our D3D layer called from it: entry wrappers, state resolve, stream copies, push-buffer decode | `crates/d3d` HLE | **yes**, into a `RecordingBackend` that forgets every call | 5.2 to 7.7 |
| GPU: encode, submit, present, read-backs | `crates/gfx`, wgpu | no (only with `H2_BOOT_GFX` or a window) | 0 |
| Vblank interrupt (60 Hz DPC, `0x0012b2a0` bumps the vblank counter) | kernel periodic source | yes | tiny |
| The frame wait (`0x0012b2e0`, `FrameWait::HALO2_CLOCKED`) | d3d HLE | yes, but on the logical clock: an idle advance, no wall time | 0 |
| DirectSound: the mixer rendered on a logical 48 kHz grid into a `NullSink`, packets completed | `crates/dsound`, `crates/audio` | **yes** | part of "kernel, XAPI, scheduler, DirectSound": 1.5 to 2.1 |
| The sound service (`0x00125b40`, guest) | guest code | yes | in the tick's share |
| XAPI input pump between slices | `crates/xapi` | yes | small |
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
  stays one tick a frame, and becomes more robust, not less.

So "faster than real time" needs no clock work: the clock is already
fixed and unpaced. What limits speed is host work per tick, and three
quarters of that is rendering.

## 2. What can be skipped, and what can't

The pipelined-render census measured exactly which guest memory the render
front-end writes and the simulation then reads
([pipelined-render.md](../graphics/pipelined-render.md), "Every dependency
line" and "The new dependency sites"). Skipping the render is the extreme
case of running it on another thread: it never runs. Each dependency line,
and what sim mode does with it:

| What the render writes that the sim reads | Census class | In sim mode |
|---|---|---|
| **Frame timing**: the frame wait writes the vblank count at the wait (`0x004e63a8..`), which frame timing turns into `dt` | (c), tick-visible | **Kept.** The frame wait runs on the game thread at every frame (`call_guest` to `0x0012b2e0`, the D3D layer's clocked wait), as the prototype's launch does. Without it `dt` is wrong. |
| **Present counter** `0x00485aa0` (64-bit), added to by `rasterizer_present`; the tick's effect code reads it | (b) | **Stubbed**: +1 per skipped frame, as the prototype's launch predicts it |
| **Render-frame counter** `0x004ba034`, added to per `render_views`; the particle update reads it | (b), cosmetic | **Stubbed**: +1 per skipped frame |
| One-shot byte `0x004e6098` the render clears per view, the sim sets | (c) | **Stubbed**: cleared per skipped frame, as the prototype's launch does |
| Object visit marks (counters `0x004de2fc`, `0x004e0308`, `0x004e7414`, `0x004e7c1c` and their stamps) | (a) | Nothing to do: the stamps only need to differ from the counter, and with no render only the tick advances them |
| Object `+0xcc` render-state handle, light render fields, heap render states | (a), false sharing | Nothing to do: only the render reads them |
| Texture, geometry and animation caches, and the async I/O pool behind them | (c), structural | **The open risk.** The render drives texture and geometry streaming, which the sim does not use (collision is in the tags). The animation cache is used by both: the tick *requests* (`0x001236f0`), the render *gets* and loads (`0x001237e0`). Whether a sim that never gets still has every animation resident is **Run**. The fallback is `render_every` (below). |
| Particle systems (`0x00174420` updates them while drawing) | (c), cosmetic | Particles do not feed gameplay. They age only through the sim's own updates. |
| **The shared random seed** `[0x004e7408]+0`: the render steps it (object code `0x001032a0` that the render reaches) | (c) | **Changes the random stream.** A sim run is reproducible against another sim run, never against a rendered run: the render's draws are no longer in the sequence. See [Determinism](#7-determinism). |
| Game-time bracket (`0x00146de0` on entry to `render_views`, `0x00146b80` on exit) | (d), not identified | **Run**: the one census item that could be real sim state. The tick-hash comparison in [First runs](#first-runs) answers it. |

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
  would save it, but it changes `crates/audio`'s voice cursors and needs
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

## 3. Pacing

| Pace | `sim.pace` | What it does |
|---|---|---|
| Unpaced (default) | `"unpaced"` | No pacer. The host runs the guest as fast as it can. |
| Real time | `"realtime"` | `Pacer::real_time()`: logical time held to wall time, as `H2_BOOT_REALTIME=1`. For watching a sim run through the agent API at game speed. |
| Scaled | `"x4"`, `"x0.5"` | A pacer at that multiple of real time (`Pacer::ticks_per_second` = 10 MHz times the factor): the run never goes faster than that. |

The pacer only makes the host wait; schedules and guest state are the same
paced or not (`crates/kernel/src/pacer.rs`). So a paced and an unpaced run
of the same match are the same match. `sim::clock` holds the parsing and
the arithmetic (tested); boot sets `Machine::pacer` from it. Pass
`--pace realtime` to `h2 sim run` or `h2 sim batch` to override the match
file for this invocation.

The report's speed-up is game time over wall time from the match's first
tick to its end (`sim::clock::SpeedMeter`), and also over the whole
process (boot and load included).

## 4. Sim mode

`H2_SIM=<match file>` on the boot path, or `h2 sim run <match file>`.
What it changes (**Code**, `crates/boot/src/sim_glue.rs`,
`crates/feature/src/sim_frame.rs`):

1. **The render front-end is skipped** in world frames: the `sim-frame`
   feature implements `RENDER_FRAME` (`0x00223240`). At each entry it
   decides as the prototype does (the world path, the countdowns,
   `render_every`). Not skipping: the generated original. Skipping: the
   frame wait on the game thread, then the stubs from the table above,
   then a plain `ret`. Every other feature's hooks on `RENDER_FRAME` (the
   bots' pads before it, their views back after it) run as they would
   around the original. The host's present count (`d3d_diag::presents`,
   which `H2_BOOT_STOP_AT_PRESENT`, the input script and the agent stamp
   count) moves on by one per skipped frame.
2. **No GPU and no window.** `Host::gfx` stays `None`; `H2_BOOT_GFX` is
   refused. The D3D layer stays installed over a `RecordingBackend`, so
   the game's own D3D calls outside the render (device creation, the
   loading screen, the sim's few calls) are answered as before.
3. **No audio output**: the logical mixer into a `NullSink`, as headless
   already is. The pipelined render is off (it implements the same point).
4. **No keyboard pad.** The host's input manager has no keyboard, so all
   four ports are free for bots: a sim match can be four bots and no
   human.
5. **Unpaced** unless the match says otherwise (above).
6. **The agent API keeps working**, and gets `match.status`,
   `match.report` and `match.stop` (below). `session.pause` and
   `session.step N` work as in any run: step N ticks of a sim match.
7. **The match ends the run**: at the game's end latch, at the match's
   tick limit, or at its wall-clock limit, the boot loop's
   `between_slices` returns false and `h2 sim run` writes the report.

## 5. Setting up a match by script

### The direct path

The level boot already starts a multiplayer map in place of the main menu:
`H2_LEVEL=ascension` builds mode 2 options for a local game, one default
player and the game's default Slayer variant, and queues them
([level-boot.md](level-boot.md), "Multiplayer"). That is the direct path
into a game the user asked for, and sim mode extends it:

| Match field | How it reaches the game |
|---|---|
| `map` | `H2_LEVEL`'s name, resolved against the disc as now |
| `players` (bots, 1 to 4) | `OPTIONS_PLAYERS(count, options)` (`0x138640`) with the bot count in place of 1 |
| `game_type` | `VARIANT_DEFAULTS(variant, type)` (`0x19d220`) with the type's built-in list index (`variants::GameType::ALL`: Slayer 0, Oddball 1, Juggernaut 2, King 3, CTF 4, Assault 5, Territories 6) |
| `time_limit_s`, `score_limit`, `team_play`, more `settings` | a new `VariantEdit` step after `VARIANT_DEFAULTS`: the `0x130`-byte record read, edited with `variants::GameVariant::set` (setting 10 the time limit, the type's score setting, 91 team play, any other id), written back |
| `teams` | written to each player's record (`+0xc0`) and unit (`+0x138`) at the first tick, as `sim.state.set` writes them |
| `seed` | written to the engine's shared random seed `[0x004e7408]+0` at the first tick, and folded into each brain's seed |

All four local players are bots. Halo 2 has four local players at most,
so **the direct path is four bots**: free-for-all, or 2 v 2 with teams.

### More than four

A `[[bot]]` with `kind = "phantom"` selects a distributed System Link
match for the whole roster. `kind = "local"` is the file default. A match
can have up to four local pads, fifteen phantom peers and sixteen players
in total. File order gives the logical seat numbers starting at zero.
Local seats receive controller ports in order. Each phantom receives its
own machine identity.

With `H2_BOTS_ALL_PHANTOM=1` (or `clankers.all_phantom = true` in the
settings file) the run and batch planners convert every
configured seat to a phantom before assigning ports. Names, teams, seeds
and tuning follow the logical seat. A roster of only phantoms uses a
playerless System Link host. The host occupies one peer slot and signs
in no controller. The other fifteen peer slots are available for bots.
Each simulated peer supplies its own marked name, appearance and team
because the host has no local profile to copy.
The sim uses the dedicated lobby's readiness checks and machine load
request (`0x5a220`) when the controller countdown has no signed-in user.
Playerless sim startup is inferred, needs a run. The lobby waits for one
host peer plus one peer per bot. Verify that `match.status` reaches
`playing` with every logical seat in `bound_seats` and that the match
report lists just those players.

For a mixed roster the headless host signs in the local pads (`0x18fb34`), creates a System
Link squad (`0x199df9(0,0)`, `0x199a03(2)`), and applies the map and
variant with retail's lobby setters (`0x19a6f2`, `0x19a728`). It waits
for the roster and retail start state 9 before requesting the countdown
(`0x19a78e`). Each process reserves its own loopback XNet port block.
Headless runs reach the regular phantom uplink and remote gather.

`match.status` enters `binding` on map activation. Combat sampling
starts once the distributed server has a player and a spawned unit for
every configured controller and phantom machine. A run that cannot
bind the roster writes its diagnostic report and exits with an error.
Startup allows up to 180 wall-clock seconds, subject to a shorter
`max_wall_s`. Distributed runs boot a fresh session for each match.

### Match file

TOML (or JSON), parsed by `sim::config::MatchConfig` (tested):

```toml
map = "lockout"
game_type = "slayer"        # slayer oddball juggernaut king ctf assault territories
time_limit_s = 300          # 0: none
score_limit = 25
team_play = false
seed = 7
max_ticks = 9000            # stop after this many game ticks (5 min at 30 Hz)
max_wall_s = 600            # and after this long on the clock
render_every = 0
pace = "unpaced"

[settings]                  # any variant setting by id (variants::record)
83 = 1                      # weapons on map

[[bot]]
name = "A1"
skill = "legendary"
team = 0
params = "tuning/aggressive.toml"   # optional; relative to this file

[[bot]]
name = "B1"
skill = "legendary"
team = 1
[bot.tuning]                # or inline
aggression = 0.9
reaction = 5
```

### Bot parameters from a file

`sim::config::BotParams` (TOML or JSON) holds what a bot is tuned by:

- `skill`: easy, normal, heroic, legendary (`bots::mind::Skill`);
- the aim table's numbers for that skill, each optional: `reaction`
  (ticks), `error` (radians), `lead`, `gain`, `fire_cone_deg`,
  `burst_on`/`burst_off` (ticks; `burst = false` holds the trigger),
  `grenades_per_minute`;
- `awareness` (0..1; the decisions' sense of the fight, per skill now);
- the personality, each optional (0..1): `aggression`, `caution`,
  `teamwork`, `objective_focus` (seeded per bot today);
- human input timing: `perception_ticks` and `motor_ticks` (0–30 ticks),
  `button_sigma_ms` (0–12 ms), `stick_step` (0.1–0.6 stick radii per
  tick), and `thumb_switch_ticks` (1–6 ticks);
- phantom transport: `phantom_ping_ms` (0–1000 ms round trip) and
  `phantom_jitter_ms` (0–50 ms bounded one-way variation). These fields
  apply when the source drives a phantom;
- `aim_model`: `code` (the hand-written tracking, the default) or `net`
  (the aim net; [bots.md §3.4.1](../game/bots.md#341-the-aim-net-built-not-compared-in-a-game-yet)).

Anything left out keeps the built-in value. The bots crate takes them as
`bots::tuning::Tuning` through `Brain::with_tuning`. The aim table and awareness are
read through `Skill::aim()` and `awareness()`, which consult the tuning of
the brain that is thinking (a scoped override on the game thread, so the
call sites stay as they are); the personality replaces the seeded one,
and again after each new life's mind. So a sweep edits a file, not the
code, and never rebuilds the image or `h2`.

## 6. Metrics

A feature, `sim-metrics`, observes the game on the game thread, read-only:

| Point | What it takes |
|---|---|
| `GAME_ACTIVATE` (game start) | the match starts: the tally resets |
| `GAME_TICK` (after each tick) | `live_telemetry::memory::snapshot`: every player's score, kills, deaths, assists, suicides, team, unit, vehicle, grenades carried, vitality; team scores |
| `WEAPON_FIRE` | a shot: the weapon's holder unit, so its player |
| `DAMAGE_REPORT` | a hit: the owner player, the victim unit, so its player, and the damage |
| `PLAYER_KILLED` | the kill log: attacker, victim, tick; a betrayal is a teammate killed in a team game (`bots::emote::kill_from_guest`) |
| `GAME_END` | the end latch: the match is over |

and each bot's brain is wrapped (`sim_glue::Metered`) to count, per game
tick, what only the brain knows: ticks stuck (the unstick escalation
above `Moving`), repaths, and ticks per activity (`Doing`: fight, chase,
roam, support, supply, rearm, recover, carry, keep, play).

`sim::metrics::Tally` aggregates all of it (pure, tested): the kill log
classified as kill, suicide, betrayal or world; per player shots, hits on
players, damage dealt and taken, ticks alive, ticks in a vehicle,
grenades thrown (the carried count falling within a life), objective
ticks; and the final score. The report is one JSON file per match:

```json
{
  "format": "h2-sim-report/1",
  "match": { "map": "lockout", "game_type": "slayer", "seed": 7, "...": "..." },
  "result": { "end": "game_over", "winner": {"player": 2}, "teams": [] },
  "ticks": 9000, "game_seconds": 300.0,
  "wall": { "setup_s": 21.8, "match_s": 41.2, "process_s": 63.0, "speedup": 7.28,
            "process_speedup": 4.76, "ms_per_tick": 4.58 },
  "players": [
    { "index": 0, "name": "A1", "bot": true, "port": 0, "arm": "base", "team": 0,
      "score": 12, "kills": 12, "deaths": 9, "assists": 2, "suicides": 0, "betrayals": 0,
      "shots": 820, "hits": 301, "accuracy": 0.367, "damage_dealt": 41.5, "damage_taken": 37.0,
      "alive_ticks": 8100, "vehicle_ticks": 0, "grenades": 14, "stuck_ticks": 36, "repaths": 2,
      "doing": { "fight": 2100, "roam": 3900 } }
  ],
  "kills": [ { "tick": 412, "killer": 0, "victim": 3, "kind": "kill" } ],
  "nav": "loaded",
  "notes": []
}
```

A shot is attributed to the firing weapon's parent unit at `WEAPON_FIRE`,
with the next tick's held-weapon snapshot as a fallback. Damage is the damage
report's body plus shield numbers (**S**: read as amounts, as telemetry's
`hits` are); a hit is a report with another player's unit as the victim
(a shotgun's pellets each count, so accuracy can pass 1).

Each bot row includes its logical `seat` and transport `kind`. Only a
local bot has a controller `port`. `opposing_hits` counts positive body
or shield damage reports against another player on an opposing team, or
any other player in free-for-all. Multiple pellets, splash and melee can
produce separate reports. The original `hits`, `accuracy`,
`damage_dealt` and `damage_taken` fields keep their previous definitions.
Readers can continue to load earlier `h2-sim-report/1` reports.

`ttk_kills` and `ttk_ticks` are the time to kill. A fight is a player's
hits with damage on one opponent, none more than 150 ticks after the
last. When the player kills that opponent, the ticks from the fight's
first hit to the kill are one sample. `ttk_ticks` is the mean over
`ttk_kills`. A kill in one hit counts as 0 ticks. A kill with no damage
report from the killer on the victim (a fall the killer caused) has no
sample.

A bot that tracked a target has an `aim` section, from the hand's view
of its target each tick (`bots::mind::AimTrace`: the angle from the aim
to the aim point, before the hand's own error) and the right stick its
pad reported after the human-input limits (`sim::metrics::AimMotion`,
tested):

```json
"aim": { "tracked_ticks": 2100, "mean_error": 0.041, "flicks": 18,
         "overshoot_ratio": 0.09, "settled": 15, "settle_ticks": 14.2,
         "stick_jitter": 0.08, "reversals_per_s": 1.1 }
```

- An engagement is a run of views of one target no more than 3 ticks
  apart. One that starts 0.35 rad (20°) or more off is a flick.
- `overshoot_ratio`: per flick, the furthest the aim went past the
  target in its first 60 ticks, over the flick's size; the mean over
  flicks.
- `settle_ticks`: per flick that settled, the ticks from its first view
  to the first of six in a row within 0.05 rad; the mean over `settled`.
- `stick_jitter`: the mean right-stick travel between consecutive views
  of an engagement, in stick radii.
- `reversals_per_s`: how often the stick's yaw direction changes within
  an engagement (a stick under 0.05 has no direction).
- `mean_error` is in radians, over `tracked_ticks`.

All of these are additive: a reader of `h2-sim-report/1` that does not
know them is unaffected, and reports without them still load.

Sim runs are local by default. Set `H2_TELEMETRY_FILE` for a separate
JSONL capture, or use `--upload` for an Uplink recording.

## 7. Parallel runs, A/B and tuning

`h2 sim batch <batch file>` (`crates/cli/src/sim_cmd.rs`):

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
[bots.md §3.4.1](../game/bots.md#341-the-aim-net-built-not-compared-in-a-game-yet).

### Local and phantom seat swaps

`mode = "seat_swap"` schedules two legs per map, game type, skill and
seed. The second leg exchanges every local and phantom seat. Both
lineups must fit on a host. Each bot keeps its name, team, tuning, arm
and brain seed. The checked-in Legendary Midship example plays ten seed
pairs, two red against two blue, with normal phantom latency.

```sh
H2_BOTS_ALL_PHANTOM=0 H2_OPT=2 h2 sim batch docs/runtime/sim-examples/midship-seat-swap.toml --disc disc/halo2.iso --parallel 3 --pace unpaced
```

Read `compare.txt` or `compare.json` under
`docs/runtime/sim-examples/results/midship-seat-swap/`. The
`seat_swap` section reports `pairs`, per-seat/game `local` and `phantom`
means, and their signed `local_minus_phantom` gap for shots, opposing
hits, damage dealt and damage taken. Each pair of legs contributes one
sample. Its two observations for each logical seat are matched across
the legs. Only full rosters ending at game over or the tick limit count.
Incomplete pairs appear in `excluded`; absent reports appear in
`missing`. These are counts per game, so game length affects the result.

Keep all-phantom mode off for this comparison. The planner refuses a
seat swap when every seat is forced to use the same transport. Compare
opposing hits divided by shots for matched seats with shots, and check
damage dealt across the same seeds. The sim counts positive opposing
damage events, including melee. To compare directly with the November 19
bullet-only baseline, filter the replay damage events to bullets.
The default changes to all-phantom once the paired runs show matching
local and phantom hit rates.

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
for returning to it after a seek. The training page shows measured simulation
speed. Use `--pace realtime` for normal game speed.

Each batch streams at most two matches at once. Other running matches appear
in the list with replay available after the match. They buffer packed
telemetry locally and take turns uploading after they finish. The batch prints
its Live batch ID and writes it to `batch.json`. Fork seed runs stay local.

The same game stream becomes the replay at the end. Uploads use the
game's telemetry frames, which include machine travel when sampled.
The finished-game viewer switches to the archive when it is published.
[Replay playback](../live/replay-fidelity.md#current-recording-and-playback)

The CLI still saves the
local report, including when an upload fails. Training games are private
and excluded from public stats and ranks. The
[Live API](../live/live-api.md#clanker-training) describes the storage limits.

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
exits. Use retail map file stems such as `lockout`. Supply the maps' bot nav meshes with the host's data for runs that
need them. Snapshot forks are local only.

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

### Forking from spawn (snapshots)

Machine snapshots (`state.save`, `load`, `fork`; [snapshots.md](snapshots.md))
are in master as alpha: unit-tested, never run against the game. The
batch can use them, and falls back to a fresh boot per run when they
refuse or are not ready:

1. `--fork`: one seed process per map boots the match, waits for
   gameplay (`session.wait {"phase":"gameplay","map_ticks":30}`) and
   saves `sim.<map>.<game type>.spawn` (`state.save`), then exits.
2. Each run boots the same match with its agent socket on; once
   `match.status` says `playing` the batch sends `state.load` of that
   snapshot, and the run's own seed and teams are written again from
   there. A `BUSY` or a failed load is logged in the batch's line for the
   run (`own boot (...)`), and the run carries on from its own fresh boot
   (the fallback). The seed run's data directory is `runs/seed-<name>/`;
   the snapshots are in `<out>/snapshots` (`H2_SNAPSHOTS` for every child).

Snapshots carry the machine, not the host's bots. The tally sees the load
as a jump in the game's tick (backwards, or more than 30 ticks forwards:
`sim::metrics::JUMP_TICKS`) and starts again there, with the speed-up's
clock, and the seed and team writes run again. The brains keep their few
ticks of memory from before the load (their tracks age out; the radio
resets only when the tick goes backwards). The snapshot's build key covers the
switches and features, so the seed process and the runs must use the
same image, `H2_OPT` and switches (the batch passes its own environment
to every child). Saves are refused while Live is signed in, in co-op, and
during a Bink movie; sim runs are offline and headless, so none applies.
**Run**: whether a level-booted multiplayer game reaches a quiescent tick
boundary at all.

## 8. Determinism

What a sim run's result depends on, and what breaks reproducibility:

| Source | Status in sim mode |
|---|---|
| The kernel clock, `rdtsc`, scheduling | logical: the same guest work gives the same schedule (**Code**, `kernel::clock`, `kernel::pacer`) |
| The epoch (system time) | fixed at 0 unless a host sets it |
| The pacer | waits only; never changes the schedule |
| The render front-end | **skipped**, so it no longer steps the shared seed: deterministic, but a different stream from a rendered run |
| The render thread | none: no GPU backend |
| Audio | logical pacing into a `NullSink` |
| Input | bots only, no keyboard. Bots read guest memory on the game thread before `RENDER_FRAME`, once a frame |
| The bots' randomness | seeded per bot: SplitMix64 of the match seed and logical seat (`sim::config::bot_seed`) |
| `HashMap` iteration in the bots (`mind.rs` tracks, `los`, `nav`) | lookups and `retain` only, as read (**S**: worth a check in review) |
| The nav mesh | built in the background on a thread at the wall clock's mercy (`bots_nav_auto`): **off in sim mode** (`H2_BOTS_NAV_AUTO=0`). A map with no current mesh is reported (`"nav": "missing"`); build them first with `h2 bots nav <map>` |
| Wall-clock limits (`max_wall_s`) | can end a match at a different tick on a slower host: the report says `"end": "wall_limit"`. Use `max_ticks` for reproducible ends |
| Our own observers | read-only; the agent's tick observer adds dispatch transfers, the same in every run that has it |
| Snapshots (`--fork`) | the RAM hash is checked at every load; replay after a load is unproven tick for tick (snapshots.md) |

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
| `match.status` | read | the match file, phase (`waiting`, `playing`, `over`), ticks, game seconds, wall seconds and speed-up so far, the end reason once over |
| `match.report` | read | the report as it would be written now (players, kills so far) |
| `match.stop` | act | end the match at the next slice boundary, with `"end": "stopped"` |

`h2 agent match.status`, `h2 agent match.report`, `h2 agent match.stop`.
Stepping is the existing `session.pause` and `session.step N`.

## 10. Expected speed-up

Clanker Training uses the [dedicated light profile](../live/dedicated-host.md#light-host).
The default skips steady-state world draws, uses descriptor-only texture
handles in the D3D shim, runs the audio clock without mixing, and maps the
disc read-only. Verified server-map copies are picked up from
`H2_SERVER_MAP_DIR`. Distributed System Link seats and packed telemetry
use their ordinary paths. Sim uses the match's pace, with unpaced as the
default. Dedicated serve waits for real-time deadlines.

The prior two-bot run took about 11.9 ms/tick. Its entire kernel, XAPI and
DirectSound bucket was 1.5–2.1 ms/tick, so removing the mixer alone could
only save part of that bucket. Allow **10–12 ms/tick and 1 GiB RSS per
process** as a starting point for a comparable unaudited workload.
Guest RAM accounts for 64 MiB. Keep the module and immutable maps shared
across processes. The guest tick, replication, telemetry and bot
thinking remain, and the distributed configuration needs a fresh
measurement. For 5,000 games at 1,800 ticks, that range is about 25–30
core-hours of tick work before loading and match setup. Use the smaller
of available effective cores and available memory divided by measured
incremental RAM. With eight effective cores and 12 GiB available, start
sizing from at most eight workers before allowing for headroom. Use
`wall.speedup` from an ordinary batch and `host.metrics` for RSS and CPU.

### Bot navigation performance

On 2026-11-20, a five-minute all-local Lockout match took about 10 ms/tick.
Across 12 retail maps the baseline was 10.4–19.5 ms/tick, including
Zanzibar at 19.5, Colossus at 17.9, Headlong at 17.4 and Foundation at 17.1.
A 15-second macOS sample of unpaced, light-profile Waterworks 2v2 (two
local bots and two phantoms) put 3,241 of 5,201 game-thread samples in
bot glue and 2,917 in ground-vehicle pathfinding. Wall checks accounted
for 1,866 top-of-stack samples across threads. The hash-table growth
under those checks came from the temporary set of triangle IDs per ray.

Drivers now reuse corridors, unreachable results and portal clearance.
Wall probes within one grid cell test its triangles directly. Longer
probes use reusable visit marks. A* reuses its heap and polygon records,
and the tally takes ownership of each tick sample. The 2× target still
needs a matched before/after run. Compare ms/tick and per-tick hashes
with the same build profile, map, seed, seats and tick limit, including
an all-local Lockout control.

## Host audit corpus

Build the full clang image with the usual optimization setting. Prepare
bot navigation data and install the owned Map Packs in `H2_DATA`. Set
`H2_DISC`, `H2_IMAGE` and `H2_DATA` to absolute paths. Run the matrix
with the original caches so it can report all observed guest reads.

```sh
env -u H2_HOST_MODULE -u H2_SERVER_MAP_DIR H2_OPT=2 h2 sim batch docs/runtime/sim-examples/light-host-corpus.toml --disc "$H2_DISC" --out "$H2_IMAGE" --tier clang --data "$H2_DATA" --parallel 4 --pace unpaced
```

The matrix plans 483 matches across 23 Xbox multiplayer maps, seven game
types, and three seeds with four legendary distributed bots. Missing
assets, unsupported Map Packs and incomplete matches are reported in
`docs/runtime/sim-examples/results/light-host-corpus/audit-missing.json`.
Add deployed mod maps and targeted matches for equipment the bots miss.
The report records the weapon fired, the weapon/vehicle/equipment tag
inventory, player vehicle occupancy, and observed powerup pickup. A
vehicle seat observation does not establish that a bot drove the vehicle
or used its weapon. A map and game-type matrix alone cannot cover every
weapon, vehicle, powerup, objective or lobby transition.

Find the per-run `audit.json` under `runs/<id>/` and the merged
`audit.json` beside `audit-missing.json`. The batch also writes
`audit-equipment-gaps.json` with indexed datums lacking an observed use. Reports from the same image,
profile and registry union their guest owners, entry/resume selectors,
tag lookup requests and byte ranges. Combine additional matches with
`h2 sim audit-merge OUTPUT INPUT [INPUT ...]`. Tag spans are relative to each datum and
are traced at 64-byte line granularity after load or BSP fixation.
The watch observes checked guest reads, including observer reads, and
can miss early loading reads, raw physical access and out-of-line data.
Unidentified tag epochs and missing hooks are recorded.

CPU totals are inclusive across the object, physics, AI, effects,
particles, sound, camera, first-person, HUD/UI and network functions
with mapped hooks. Bot and telemetry times come from their Rust entry
points. The telemetry worker has a separate bucket for consumed samples.
Divide each total by report ticks for an inclusive per-tick
attribution. Nested buckets overlap, and totals include lobby and load
work where the hook runs. Auditing bypasses generated native direct
calls to count visible entries. Inlined blocks with no selector cannot
be counted separately. Use an unaudited batch for throughput.

The experimental `H2_HOST_MODULE` build option accepts a complete
`audit.json` from the matching full clang image. Use a separate output
directory and keep that same report path set when launching it.

```sh
H2_OPT=2 H2_HOST_MODULE="$H2_AUDIT_REPORT" h2 build "$H2_DISC" --out "$H2_SMALL_IMAGE" --tier clang
```

Cold bodies keep their registry and selectors and stop the guest with a
logged address on first entry. Re-run that case with the full light
image, add it to the corpus and rebuild. An audit with missing runs or
hooks cannot build the small module. The full image remains the default.
Compare a candidate against the full light image with the existing
`H2_LOCKSTEP`/`h2 lockstep` check before relying on any prune. Same-profile
repeatability needs to pass first. Cross-profile equality is still open
because the render reads and advances shared state and the hash includes
guest buffers. The light tick keeps guest cosmetic updates and all tag
headers and blocks while those dependencies are being traced.

## What needs a run

1. The skip itself: a level-booted multiplayer game runs gameplay frames
   with the front-end skipped, ticks at one per frame, and does not stall
   (the frame wait returns; the countdowns stay at or below 0).
2. The tick-hash comparison: does skipping change anything the tick
   reads beyond the seed? (`render_every` as the fallback.)
3. `OPTIONS_PLAYERS` with 2 to 4: four local players, each on its own
   controller port, each a bot's pad.
4. `VARIANT_DEFAULTS` with types other than Slayer, and the edited
   variant passing the game's options check (`level boot: the game's
   options check passes`).
5. Teams written at the first tick holding through respawns.
6. The end latch at the time or score limit ending the run.
7. The speed-up.
8. `--fork`: a snapshot of a sim match at spawn, loaded into another
   process.

## First runs

On the coordinator's machine, with an image built as usual (`h2 build`;
`H2_OPT` the same as the build):

```sh
# 0. Nav meshes for the maps (once; sim mode never builds them).
H2_OPT=2 h2 bots nav lockout

# 1. One match, two bots, short: does it start, tick and end?
cat > /tmp/sim-smoke.toml <<'EOF'
map = "lockout"
game_type = "slayer"
seed = 1
max_ticks = 1800
[[bot]]
skill = "normal"
[[bot]]
skill = "normal"
EOF
H2_OPT=2 h2 sim run /tmp/sim-smoke.toml --disc disc/halo2.iso --report /tmp/sim-smoke.json
# Look for: `sim: skipping the world render`, `level boot: options queued`,
# `sim: match started at tick N`, `sim: match over (tick_limit) at 1800
# ticks, N.Nx real time`, then the per-bot lines and `sim: report ...`.
# Watch it live: add --socket /tmp/sim.sock and
# `H2_AGENT=/tmp/sim.sock h2 agent match.status`.

# 2. Determinism: two runs, tick hashes compared.
H2_LOCKSTEP=/tmp/a.lock H2_OPT=2 h2 sim run /tmp/sim-smoke.toml --disc disc/halo2.iso --report /tmp/a.json
H2_LOCKSTEP=/tmp/b.lock H2_OPT=2 h2 sim run /tmp/sim-smoke.toml --disc disc/halo2.iso --report /tmp/b.json
h2 lockstep /tmp/a.lock /tmp/b.lock

# 3. The skip against the render: the same match with the render kept on
#    every frame (render_every = 1). Expect the runs to differ (the seed),
#    and compare the reports' shape and the speed-up.
sed 's/^seed = 1/seed = 1\nrender_every = 1/' /tmp/sim-smoke.toml > /tmp/sim-render.toml
H2_OPT=2 h2 sim run /tmp/sim-render.toml --disc disc/halo2.iso --report /tmp/r.json

# 4. Four bots, 2 v 2, then the example batch (fifteen runs on three
#    maps: build the other two nav meshes first; --dry-run lists the runs).
H2_OPT=2 h2 sim run docs/runtime/sim-examples/lockout-2v2.toml --disc disc/halo2.iso
H2_OPT=2 h2 bots nav midship && H2_OPT=2 h2 bots nav ascension
h2 sim batch docs/runtime/sim-examples/batch.toml --dry-run
H2_OPT=2 h2 sim batch docs/runtime/sim-examples/batch.toml --disc disc/halo2.iso --parallel 3

# 5. The same batch forked from spawn snapshots (falls back per run).
H2_FEATURES=-host-light H2_OPT=2 h2 sim batch docs/runtime/sim-examples/batch.toml --disc disc/halo2.iso --parallel 3 --fork
h2 sim compare docs/runtime/sim-examples/results/aggressive-vs-base
```

Notes for the runs:

- `h2 sim` never builds: it stops with `h2 build` when the image is not
  current for this `h2` (as `h2 serve` does).
- Each batch run has its own data directory (`<out>/runs/<id>/data`, its
  `bots/` linked to `--data`'s). For supported Map Pack runs the batch
  stages the selected offer from the shared data directory.
- The match's pace replaces `H2_BOOT_REALTIME` (unpaced unless the match
  says otherwise).

## Tests

`cargo test -p sim` (config, metrics, compare, clock, the examples
here), `cargo test -p bots tuning` and `cargo test -p bots
a_tuned_brain` (the overrides), `cargo test -p bots sim_mode_takes`
(four bot ports), `cargo test -p feature sim_frame` (the skip's decision
and stubs over a fake guest), `cargo test -p boot sim_glue` (snapshots to
samples, the limits, the tuning mapping) and `cargo test -p boot
level_boot` (the match options, the variant edit), `cargo test -p cli
sim_cmd` (child arguments, per-run data, the results directory).
