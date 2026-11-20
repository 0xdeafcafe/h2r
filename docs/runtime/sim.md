# Sim mode: the game's simulation without rendering

Status: designed and built (2026-11-19, branch `sim-engine`), unit-tested
over pure inputs and fake guest memory. Nothing on this page has been run
against the game yet; [What needs a run](#what-needs-a-run) lists what
only a run can settle, and [First runs](#first-runs) gives the commands.

Where it is: `crates/sim` (the match, batch and tuning files, pacing, the
tally and report, the comparison), `crates/bots/src/tuning.rs`,
`crates/feature/src/sim_frame.rs`, `crates/boot/src/sim_glue.rs` and its
System Link lobby, `crates/cli/src/sim_cmd.rs`, and the examples in
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

1. **The light host can skip the render front-end** in world frames. The `sim-frame`
   feature implements `RENDER_FRAME` (`0x00223240`). At each entry it
   decides as the prototype does (the world path, the countdowns,
   `render_every`). Not skipping: the generated original. Skipping: the
   frame wait on the game thread, then the stubs from the table above,
   then a plain `ret`. Every other feature's hooks on `RENDER_FRAME` (the
   bots' pads before it, their views back after it) run as they would
   around the original. The host's present count (`d3d_diag::presents`,
   which `H2_BOOT_STOP_AT_PRESENT`, the input script and the agent stamp
   count) moves on by one per skipped frame. Full headless runs
   (`-host-light`) keep the render entry and can explicitly enable the
   [render worker for hash comparisons](../graphics/pipelined-render.md#tick-ordering-and-hash-parity).
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

### The playerless System Link host

The headless host opens a System Link squad and adds each bot as a
phantom peer, with one player per peer. The host occupies one of the
sixteen peer slots and leaves all controller ports free, so a match
can have up to fifteen bots. File order gives the logical seat numbers
starting at zero.
Each phantom receives its own machine identity, marked name, appearance
and team, because the host has no local profile to copy.

In a `[[bot]]` entry, omit `kind` or set `kind = "phantom"`. A match
file with `kind = "local"` fails validation because controller ports
are reserved for people.

The host creates a System Link squad (`0x199df9(0,0)`, `0x199a03(2)`)
through retail's lobby setters for the map and variant (`0x19a6f2`,
`0x19a728`), and the lobby's readiness checks and machine load request
(`0x5a220`) stand in for the controller countdown. Each process
reserves its own loopback XNet port block. Headless runs reach the
regular phantom uplink and remote gather.

| Match field | How it reaches the game |
|---|---|
| `map` | the lobby's map row, resolved against the disc |
| `game_type` | the lobby's variant built from the type's built-in list index (`VARIANT_DEFAULTS`, `0x19d220`) |
| `time_limit_s`, `score_limit`, `team_play`, more `settings` | edits of the variant record with `variants::GameVariant::set` (setting 10 the time limit, the type's score setting, 91 team play, any other id) |
| `seed` | written to the engine's shared random seed `[0x004e7408]+0` at the first tick, and folded into each brain's seed |

### The playerless start

**Code**: `sim_glue/hosted.rs` and
`host_lobby/retail.rs`. One step per game tick at the start of the tick:

1. **Open.** `PROFILE_SIGN_IN` (`0x18fb34`) is refused for the run, so no
   port is ever signed in. The host creates the System Link squad
   (`0x199df9(0,0)`, custom mode `0x199a03(2)`), sets the map
   (`0x19a6f2`) and the variant (`0x19a728`) with the player limit at the
   bot count, and waits until the squad shows them
   (`sim: System Link host Create -> Start`).
2. **Gate.** The squad must be live (state 5), hosted here, distributed,
   on the match's map, with one player per seat and one peer per machine:
   this host's own peer plus one per phantom (`start_gate`).
3. **Eligibility.** The lobby's own checks (`Eligibility::blocker`): the
   pregame phase, party leader, map, protocol, player limit, the variant
   open applied, and every peer's map status above 2. The last is the
   countdown finisher's test (`0x6e410` at `0x6e4b8`); retail's start
   check lets a status still loading through for the party leader, and
   the playerless path stands in for the finisher.
4. **Retail's start check** `0x19a2ce(0)` must answer 9.
5. **Start.** The controller countdown `0x19a78e` (EAX port 0) refuses on
   this host (port 0's local-user index is −1), so the host makes the
   finisher's own request: `0x5a220` with the session in ESI and stage 2
   in EDI (register arguments, plain `ret`, as `0x6e4d7` calls it). Session
   `+0x4980` becomes 2 and the machines load the map
   (`sim: System Link host Start -> Loading`).

Every wait says why, once per change:
`sim: System Link host waits at Start: <reason>`, for example
`peer 0 has map status 2 (...)` or
`retail start state 7, blocked by the teams or co-op check (...)`.
The co-op hooks (`SESSION_GAME_LIMITS` "peer limit 2",
`SESSION_GAME_START` "one peer for the campaign check",
`LOBBY_START_STATE` "co-op over 2 to N consoles") act on campaign
sessions only and leave this lobby as retail has it (tested).

Needs a run: `h2 sim run` on an all-phantom match (for example four
phantoms, Slayer, Lockout). Expect `Create -> Start`, the phantoms'
joins, possibly `waits at Start: peer N has map status 2` while the map
loads, then `Start -> Loading`, `sim: match started at tick N` and
`bots: phantoms: the game's start waits for real consoles only`.
`match.status` must reach `playing` with every logical seat in
`bound_seats`, and the report must list just those players. A run that
still stalls names the check in its last `waits at Start` line.

`match.status` enters `binding` on map activation. Combat sampling
starts once the distributed server has a player and a spawned unit for
every phantom machine. A run that cannot bind the roster writes its
diagnostic report and exits with an error.
Startup allows up to 180 wall-clock seconds, subject to a shorter
`max_wall_s`. Distributed runs boot a fresh session for each match.
Verify that `match.status` reaches `playing` with every logical seat in
`bound_seats` and that the match report lists just those players.

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
  `phantom_jitter_ms` (0–50 ms bounded one-way variation);
- host work intervals in game ticks (1–30 each): `foot_plan_ticks`
  (default 6), `vehicle_plan_ticks` (8) and `vehicle_lookahead_ticks` (3).
  Older files can still specify `player_los_ticks`. Player sight is
  sampled each tick. See [Bot navigation performance](#bot-navigation-performance)
  for the active intervals;
- `aim_model`: `code` (the hand-written tracking, the default) or `net`
  (the aim net; [bots.md §3.4.1](../game/bots.md#341-the-aim-net-compared-the-code-aim-stays)).

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
| `WEAPON_FIRE` | a shot credited to the weapon's holder or to the player in the firing seat of a vehicle or turret |
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
            "process_speedup": 4.76, "ms_per_tick": 4.58,
            "peak_rss_mib": 981, "footprint_mib": 143,
            "guest_written_pages": 2100, "guest_pages": 16384 },
  "players": [
    { "index": 0, "name": "A1", "bot": true, "seat": 0, "arm": "base", "team": 0,
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

With `H2_DISPATCH_STATS=1`, the report includes `dispatch_per_tick`
for the observed match interval. It records the CALLs, JUMPs, RESUMEs,
RETURNs, STOPs and FAULTs that native generated handlers hand back to the
runner. HLE entries and slice or host-request preemptions are counted
alongside those exits. A loop resume can also cause a preemption, so
these counts can overlap. `H2_ENTRY_STATS=1` adds a separate, more
expensive census of direct-slot misses and hot runner targets on stderr.

At `WEAPON_FIRE`, a handheld shot is credited to the player who owns
the parent biped. The next tick's held-weapon snapshot provides a
fallback. For mounted fire, `bots::vehicle::read::firing_player` looks
up the player in the firing seat at the time of the shot. For example,
a Ghost's plasma fire counts towards its driver's shots and a
Warthog's chaingun fire counts towards its gunner's shots.

A hit is a damage report against another player's unit. Damage sums
the report's body and shield amounts. Each shotgun pellet can count
as a hit, and melee and splatter can land hits without firing a shot,
so accuracy can exceed 1.

Each bot row includes its logical `seat`. Readers can continue to load
earlier `h2-sim-report/1` reports.

The `wall` block carries the process's memory: `peak_rss_mib`
(`getrusage`'s `ru_maxrss`), `footprint_mib` on macOS
(`proc_pid_rusage`'s `ri_phys_footprint` — dirty pages, the number a
fleet pays for), and, when the run loaded a shared copy-on-write guest
RAM base (§6.2), `guest_written_pages` of `guest_pages`: the census of
what the match wrote over the base, so batches record memory per run.
Measured 2026-11-20, one waterworks 2v2 sim process, macOS: peak RSS
about 981 MiB (791 MiB of it the resident pages of the whole mmapped
disc image), footprint (dirty) 143 MiB (103 MiB Malloc Large, of which
guest RAM is 64 MiB; 35 MiB Malloc Small), shared `__TEXT` 414 MiB.

### 6.1 Sim stores: the map without the map

The retail map is never streamed in a sim or light host that uses
stores. A recording pass notes every byte range the guest reads through
the VFS; the build packs those ranges per map into one small file; the
run serves the map from it, byte for byte at the same offsets, and a
read of a byte no recording covered is a named error (`NotInStore`,
with the range on stderr) — never a silent fall back to the disc, so a
miss is found, recorded, and the store rebuilt:

```sh
# 1. Record (runs merge, so play several seeds for coverage):
H2_SIM_STORE_RECORD=reads.txt h2 sim run match.toml
# 2. Build (verified byte for byte against the disc):
h2 sim store build waterworks --disc disc/halo2.iso --out store --reads reads.txt
#    ...or every map the recording holds:
h2 sim store build all --disc disc/halo2.iso --out store --reads reads.txt
# 3. Serve (default off until measured):
H2_SIM_STORE=store h2 sim run match.toml   # or h2 serve --system-link --host-light
```

The store file (`<map>.h2store`) is a 24-byte header, a sorted range
index and the packed bytes of the ranges — nothing else. The serving
side (`vfs::SimStoreBackend`, layered under `H2_MAP_DIR` and
`H2_SERVER_MAP_DIR`) keeps only the index in memory, maps the file
read-only and copies straight into the guest's own read buffer, so
every process on one host shares the store's page-cache pages and
nothing private is retained. `stat` keeps the map's logical length; a
store-served map has no `host_file` (like a streamed overlay); an
unreadable store file makes the map's reads fail, never fall back.
Two games on different maps still share `shared.map`'s store, the
common tag data both read, and the dylib's code pages.

Boot still opens the disc image for the XBE and small header reads;
with a store covering every map the sim reads, the image's resident
pages stop mattering. Without `H2_SIM_STORE`, the light profile maps
the disc read-only and drops a bulk read's pages again after the copy
out (`vfs::disc`), so a map copied to the title's cache partition
leaves no pages behind either.

### 6.2 Sharing guest RAM with machine snapshots

Halo copies a map's tag data into guest RAM at load, so each process
holds its own dirty copy. `state.save` with `"cow": true` writes, beside
the snapshot, a sparse raw RAM image (`<name>.ram`: zero pages are
holes); `state.load` with `"cow": true` maps that file private
copy-on-write in place of decoding RAM in — pages the match never
writes stay one shared set across every process on the same image
(including the zero pages), and only written pages become private.
The [agent snapshot methods](snapshots.md) support this mode. The
match-end census in the report's `wall` block (`guest_written_pages`)
counts pages written. `vmmap` (macOS) or `/proc/<pid>/smaps` (Linux)
shows the private and shared memory per process.

Sim batches boot a fresh System Link host for each run. Machine
snapshots omit the phantom peers' host state and can't seed these
batches.

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
for returning to it after a seek. The training page shows measured simulation
speed. Use `--pace realtime` for normal game speed.

Each batch streams at most two matches at once. Other running matches appear
in the list with replay available after the match. They buffer packed
telemetry locally and take turns uploading after they finish. The batch prints
its Live batch ID and writes it to `batch.json`.

The same game stream becomes the replay at the end. Uploads use the
game's telemetry frames, which include machine travel when sampled.
Uploaded players have bot flags and no Live account XUIDs.
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
need them.

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
| Bot thinking threads (`H2_BOT_THREADS`, `H2_BOT_PIPELINE`) | the guest is read on the game thread only and the thinks are independent, so pads do not depend on the thread count. Batched equals serial; pipelined is a different match (pads a tick later, latency kept) that is equally reproducible ([bots.md §5.5](../game/bots.md)) |
| The bots' randomness | seeded per bot: SplitMix64 of the match seed and logical seat (`sim::config::bot_seed`) |
| `HashMap` iteration in the bots (`mind.rs` tracks, `los`, `nav`) | lookups and `retain` only, as read (**S**: worth a check in review) |
| The nav mesh | built in the background on a thread at the wall clock's mercy (`bots_nav_auto`): **off in sim mode** (`H2_BOTS_NAV_AUTO=0`). A map with no current mesh is reported (`"nav": "missing"`); build them first with `h2 bots nav <map>` |
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

### Post-tick state hashes

Each new sim report records `state_hashes_v1` in guest tick order. A row
has the guest `tick` and a 16-digit hexadecimal `gameplay` digest. The
digest covers the shared engine RNG, BSP, cinematic and stats flags, team
scores, and each player slot's datum, score line, pose, aim, velocity,
vitality, vehicle, seat, inventory and animation channel. The encoding
uses fixed-width little-endian words, option presence tags, and the float
bits from telemetry. Player names and wall time have no bearing on it.
Repeat observations replace the same tick. Reloading a snapshot restarts
the list with the tally. A failed snapshot leaves a gap. Older reports
have no series.

For a stricter comparison, `H2_FEATURES=impl.sim.arena-hash` adds an
`arena` field to each row using the existing lockstep hash of the
`0x3fe000`-byte checkpoint arena. It follows the pointer on every tick
and reuses a read buffer. An unreadable arena leaves the field absent
and adds a report note. Compare the tick sequence, check every expected
digest is present, and find the first differing tick. The default digest
can detect player or RNG divergence before the first kill, while the
arena check also covers hidden state inside that allocation.
`H2_LOCKSTEP` provides the wider whole-RAM check. Time matches with arena
verification off so copying and hashing the arena does not enter the budget.

### Finding the first differing state

`h2 sim diff <a.json> <b.json>` says which state differs first, not only
that a digest does. Each report also carries `state_components_v1`: the
same ticks as `state_hashes_v1`, broken into named components with their
values as text. The first row lists every component; each later row lists
only those that changed (a component that goes away reads `-`). It is on in
every `h2 sim run` and is bounded at 250,000 recorded values; a longer match
keeps its first ticks and sets `truncated_after`.

Components: `time.tick`, `rng.seed`, `world.bsp`, `world.flags`,
`team.NN.score`, and for each player slot `player.NN.{id, stats, pos,
velocity, unit_velocity, facing, aim, health, weapon, vehicle, anim,
motion}` (or `player.NN.object` when the unit is gone). They come from the
same telemetry snapshot as the gameplay digest, so they cost one pass of
string formatting per tick and no extra guest reads. Floats print so they
read back exactly.

The object table and every other hidden allocation are covered by the
checkpoint arena, hashed in fixed 64 KiB blocks as `arena.NN` (hex digest).
That needs the 4 MiB read and hash each tick, so it stays behind
`H2_FEATURES=impl.sim.arena-hash` (the same switch as the aggregate `arena`
digest, which it replaces with a single pass). `arena_base` in the series is
the guest address of block 0.

`diff` replays both series and prints the first tick where any component
differs, then those components sorted by name with both values; for an arena
block it adds the block index and guest address range. A tick that only one
run observed shows as component `tick`. If either report lacks the series
(an older report), it falls back to the first tick whose aggregate
`gameplay` or `arena` digest differs, and says so. Reports with neither are
reported as not comparable. A series cut short at its size bound with no
difference so far reports that later ticks are unverified.

The silent audio packet clock and the sim's reduced telemetry reads can
be toggled separately with `-impl.audio.packet-clock` and
`-impl.sim.metrics-projection` in `H2_FEATURES`. The post-bot-change
Waterworks all-local run measured 14.12 ms/tick, with 70.76% of main-thread
samples in guest code. The caller table, skip boundaries and A/B procedure
are in the [simulation budget](../perf/sim-budget.md).

### Bot navigation performance

On 2026-11-20, a five-minute all-local Lockout match took about 10 ms/tick.
Across 12 retail maps the baseline was 10.4–19.5 ms/tick, including
Zanzibar at 19.5, Colossus at 17.9, Headlong at 17.4 and Foundation at 17.1.
A 15-second macOS sample of unpaced, light-profile Waterworks 2v2 (two
local bots and two phantoms) put 3,241 of 5,201 game-thread samples in
bot glue and 2,917 in ground-vehicle pathfinding. Wall checks accounted
for 1,866 top-of-stack samples across threads. The hash-table growth
under those checks came from the temporary set of triangle IDs per ray.

The first optimisation pass added corridor, unreachable-result and portal
clearance reuse. It also removed the temporary triangle set from wall
rays, reused A* search records and let the sim tally take ownership of
each tick sample. The all-local profile in the [simulation budget](../perf/sim-budget.md)
is a separate measurement. Guest code accounted for 70.76% of its main
thread, so a host-side bot change alone cannot halve that workload.

#### Per-tick work

| Work | Before scheduling | Current frequency |
|---|---|---|
| Bot pads | Phantom action gather samples once per fresh simulation tick with a bound unit, then builds and publishes the retail action. | Same cadence for pads, kill events and life transitions. |
| World perception | Per pad sample, read up to 16 players, arms, scoreboard and objective state. A living bot scanned up to 2,048 object slots for weapons, supplies and grenades, then scanned the table again for vehicles and players again for seat occupants. | Per fresh sample, reuse the ground scan's vehicle headers and read occupants when vehicles exist. Perception, vehicle seat markers, recent cues and the delayed observation queue reuse buffers. |
| Player LOS | Per candidate in the notice cone (or near the bot), cast an eye-to-chest wall ray. The delayed mind sample repeated the opponent rays. | Cast against the observer and targets' positions in each fresh sample. Target selection and aim use that sample's visibility and positions. The mask is shared within the sample. The existing human-input timing still applies. |
| Other visibility and wall probes | Ray-test in-cone pickups, supply sites, objective items and non-owned vehicles and eligible events. Foot steering can probe for clearance, headroom, cover and footing. Hearing, grenade arcs and danger checks add conditional probes. | Same sample or action cadence. Aim-point, projected lead and immediate movement clearance use current geometry for the sample. |
| Radio and event cues | Each sample advances the radio at most once per guest tick, filters allies' reports, and gathers recent kills and shots. Fresh thinking ticks update supply and combat memory before issuing the pad. | Same cadence with reused inbox and cue buffers. |
| Target selection and aim | Each fresh mind tick ranks the visible enemies and evaluates tactics, objective and equipment. Some support and engagement checks cast additional rays. Aiming, lead, trigger and pad update while tracking every tick. On-foot combat can cast up to two additional rays for the preferred aim point and lead. | Same cadence and scoring, using the filtered sample. Aim and input timing continue on each mind tick, including ticks without a plan. |
| Foot routes and roaming | A missing route, changed carrier threat or destination movement over one unit could trigger a replan each tick. A plan can try up to three A* searches (safety, blacklist and unrestricted). Picking a roam goal can test reachability of multiple candidates. Ball shelter already caches for 60 ticks or until the threat moves 1.5 units. | Pick a needed roam goal immediately. Plan immediately when a foot route is missing or the destination has moved over one unit. Refresh a usable route on the bot's `foot_plan_ticks` phase. Steering stays tick-rate and committed traversal keeps control. Boarding uses foot routes until entry. |
| Ground vehicle route | For a driver, missing routes and destination movement over three units could query the route every tick. A fresh corridor can check up to three vertical rays per uncached portal. | Active ground drivers with a destination plan immediately when the route is missing, blocked or over three units from the new destination. Refresh a usable route on the bot's `vehicle_plan_ticks` phase. Chassis portals are indexed by nav edge. Corridor and route point storage is reused. |
| Ground vehicle clearance | Each eligible driving tick checked the span to the first point, up to three lookahead points and advancement past nearby waypoints. Each span checks ground and up to three wall rays at each 0.45-unit sample across three lanes. Splatter and reversing can check additional spans. | Full route lookahead on the `vehicle_lookahead_ticks` phase and when a new or reached carrot needs checking. Between full checks, validate the immediate span every driving tick before steering toward the last carrot. That span covers up to 1.8 units or 150 ms of travel, whichever is greater. Splatter and reversing retain live clearance. |
| Fighter steering | Checked floor twice per pilot tick and ran a flip clearance probe even without a newly eligible hit. | Reuse the floor result. Probe flip clearance on an eligible hit. Forward clearance and steering stay tick-rate. |
| Map and session glue | The host seats phantom peers and refreshes session state. Nav decoding or generation is tied to map loads and missing meshes, with the auto-nav worker handling background work. | Same hook cadence. Each brain only copies the map name on a change. |

Refreshes of usable routes are phase-aligned to the in-game player
slot. A lost target or new damage can bring a driver's refresh forward.
On foot, a change in a carrier's threat can bring a route refresh forward.
Urgent refreshes split across even and odd slots, and each planner can
refresh at most once per tick. A route needed for immediate movement
bypasses the slot phase. Timing follows the simulation tick and slot
order, with the same random seeds as the rest of the brain.

The per-sample visibility mask, array scans, perception buffers, driver
portal records and path workspace avoid rebuilding per-tick temporary
collections in the common path. Initial buffer growth, new objectives
and some conditional tactical or route outputs can still allocate.

#### Bot thinking across cores

The brains can think off the game thread ([bots.md §5.5](../game/bots.md)):
`H2_BOT_THREADS=auto` batches every phantom's think across workers, and
`H2_BOT_PIPELINE=1` also overlaps the thinks with the guest's tick. The
default is serial. To measure, run the same match file (Waterworks and
Lockout, phantoms, seed 7) serial, batched and pipelined. Compare
`state_hashes_v1`: batched must match serial at every tick, and pipelined
must match itself across thread counts. Then compare ms/tick and the
game-thread share of bot glue.

#### Matched performance and combat comparison

Use `H2_BEFORE` and `H2_AFTER` for the absolute paths to the two
release `h2` binaries. Both must support phantom peers. Set
`H2_DISC`, `H2_IMAGE` and `H2_DATA` to
absolute paths to the same disc, compatible clang image built at
`H2_OPT=2`, and data directory with both nav meshes. From the repo root,
create two fixtures with four legendary phantoms, identical teams,
seed 7 and a 9,000-tick limit.

```sh
bench_dir="$RUSH_KEEP/bot-speed-phantom"
mkdir -p "$bench_dir"
awk '/^map = / {$0="map = \"waterworks\""}
     /^time_limit_s = / {$0="time_limit_s = 0\nscore_limit = 0\nseed = 7"}
     {print}' \
  docs/runtime/sim-examples/midship-seats.toml > "$bench_dir/waterworks.toml"
sed 's/^map = "waterworks"/map = "lockout"/' \
  "$bench_dir/waterworks.toml" > "$bench_dir/lockout.toml"
```

Run both fixtures on both binaries with the same unpaced light profile
and write separate reports.

```sh
for arm in before after; do
  case "$arm" in
    before) sim_h2="$H2_BEFORE" ;;
    after) sim_h2="$H2_AFTER" ;;
  esac
  for map in waterworks lockout; do
    env -u H2_PROFILE -u H2_TIMELINE -u H2_LOCKSTEP -u H2_BOOT_GFX \
      -u H2_HOST_MODULE -u H2_SERVER_MAP_DIR H2_OPT=2 \
      H2_FEATURES=host-light,-impl.sim.arena-hash \
      "$sim_h2" sim run "$bench_dir/$map.toml" --disc "$H2_DISC" --out "$H2_IMAGE" \
      --data "$H2_DATA" --tier clang --pace unpaced --report "$bench_dir/$map-$arm.json" \
      || break 2
  done
done
```

Verify a loaded mesh, four bound phantom seats, 9,000
ticks and a `tick_limit` exit. Compare `wall.ms_per_tick` and per-seat
kills, shots, hits and accuracy. Vehicle ticks, stuck ticks and replans
help explain a change in kill or accuracy results.

```sh
jq '{map: .match.map, seed: .match.seed, ticks, end: .result.end, nav,
     ms_per_tick: .wall.ms_per_tick,
     players: [.players[] | {seat, name, kills, deaths, shots, hits,
                             accuracy, vehicle_ticks, stuck_ticks, repaths}]}' \
  "$bench_dir"/waterworks-before.json "$bench_dir"/waterworks-after.json \
  "$bench_dir"/lockout-before.json "$bench_dir"/lockout-after.json
```

Repeat each arm with the same inputs and compare `state_hashes_v1` to
test same-build determinism before attributing a difference to the
change. Each run uses fresh processes. Match trajectories can differ
between arms because decisions that require a fresh route can move to a
different tick. Compare Waterworks driving and combat across paired
seeds if a single pair shows a quality change.

## Host audit corpus

Build the full clang image with the usual optimisation setting. Prepare
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

For a deployable module, export the owner/selector pairs with
`h2 sim audit-coverage OUTPUT AUDIT [AUDIT ...]` and use
`h2 build --profile host --coverage OUTPUT`. Omitted owners use the
counted interpreter fallback. See [Slim host](../live/dedicated-host-build.md#slim-host)
for recording and comparison.

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

1. The skip itself: the System Link host runs gameplay frames with the
   front end skipped, ticks at one per frame, and does not stall
   (the frame wait returns; the countdowns stay at or below 0).
2. The tick-hash comparison: does skipping change anything the tick
   reads beyond the seed? (`render_every` as the fallback.)
3. Phantom seating on a playerless host. Check that each peer acquires
   a player and unit before combat sampling starts.
4. `VARIANT_DEFAULTS` with types other than Slayer. Check that the host
   starts the requested variant.
5. Phantom team assignments holding through respawns.
6. The end latch at the time or score limit ending the run.
7. The speed-up.
8. The playerless start ([The playerless start](#the-playerless-start)): an
   all-phantom match reaches `Start -> Loading` and binds every seat.

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
# Look for: `sim: skipping the world render` and `match.status` reaching `playing`,
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
a_tuned_brain` (the overrides), `cargo test -p feature sim_frame` (the skip's decision
and stubs over a fake guest), `cargo test -p boot sim_glue` (snapshots to
samples, limits and phantom tuning), and `cargo test -p cli
sim_cmd` (child arguments, per-run data, the results directory).
