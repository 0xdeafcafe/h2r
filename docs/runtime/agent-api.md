# Agent API: driving the running game without pixels (design)

Status: phase 1 built ([below](#phase-1-what-was-built)): the socket,
`session.*`, `world.count`/`query`/`object`, `profile.counters`,
`features.list`, `h2 serve` and `h2 agent`. Raw and recorded input and
`profile.capture` built too ([below](#phase-3-and-4-what-was-built)).
Phase 5 ([plan and what was built](#phase-5-testing-in-the-lightest-way-plan)):
`render.frame`, `render.stats`, spike causes, `profile.fallbacks`,
`load.timeline`, `h2 testmap` and `h2 agent run`.
`sim.*` (simulated players, host writes, expectations, lobby steps) built
over a bots stand-in ([below](#sim-simulated-players-and-direct-state)).
`trace.*` and `flight.*` (per-tick rows taken in the game, a flight
recorder with input, `h2 agent flight show`) built
([below](#tick-traces-and-the-flight-recorder)). `state.save`, `load`,
`list`, `drop` and `fork` (machine snapshots) built, not yet run against
the game ([`state`](#state-put-the-game-where-you-want-it)).
Everything else on this page is design; its Rust and JSON blocks are
sketches, not code that compiles. The front end is the `h2 agent` CLI
only: the MCP server is dropped (decided 2026-11-17).

This page designs one interface through which an agent (or a script, or a
test) can see and drive a running Halo 2. That means: what is on screen,
what is in the level, where frame time goes, and how to put the game into a
given state, act in it, and compare variants of a mission to find fps.

## Evidence labels

- **Code**: read from this repository at `51a3810c`. File given.
- **Doc**: stated by another page here; follow the link.
- **S**: speculation or a design choice. Check it before building on it.

Nothing on this page was established by running the game. No game data is
in this page.

## Why

Every agent working on this project today sees the game in one of three
ways: logs the user pastes back, screenshots taken by a headless run, or
unit tests over synthetic data. Recent work shows the cost:

- The firefight fps drop could not be profiled, because no agent can put
  the game into a firefight. It waits on the user's frame stats.
- An E3 lighting comparison came out wrong because the capture tool grabbed
  frames during a fade to black. Nothing told it the screen was black.
- Two headless runs of the same level stop matching a few minutes in, so
  comparing a run with a change against a run without it measures noise.
- The skulls page and the co-op screen were built from the game's code;
  whether the presses arrive, or which screen actually opened, comes back
  only as "send me the `skulls page:` lines".

What exists already, and what this design builds on (**Code**):

| Need | Existing piece | Gap |
|---|---|---|
| Who and what is in the level | `live-telemetry::memory::{snapshot, players, actors, world, placements, object}` | Read for the Live service, not on demand. No projectiles, effects, squads, scripts. |
| Is the run deterministic | `lockstep`: RAM hash per tick and present, first divergence | Offline comparison of traces; no "start both from the same state". |
| Recorded input | `xapi::tap` (record pad packets, replay by read index); `H2_BOOT_INPUT` scripted presses | Launch-time only. |
| Turn things on and off | `feature::Switches` (`H2_FEATURES`, `impl.<subsystem>` switches) | Read once from the environment. |
| Where time goes | `perf` sampler, `timeline` (Perfetto JSON), `H2_FRAME_STATS`, `perf::budget` | Whole-run captures; no per-object or per-squad attribution. |
| UI layout | `h2 ui` and `tags::ui`: a screen's static widget tree | Static only: nothing says which screens are open now, or what has focus. |
| Lights | `h2 lights <map>` | Static only. |
| Start a level | `--level` / `H2_LEVEL` ([level-boot.md](level-boot.md)) | Starts at the level's beginning, not at a moment in it. |
| Snapshots | the game's own checkpoint save and revert ([checkpoints.md](checkpoints.md)) | Tested, not reachable from outside. |

Nothing in the game process takes commands while it runs; every knob is an
environment variable read at launch (**Code**: no listener outside
`live-server`).

## Principles

1. **Attach, don't build.** The API is served by an already-built image
   that is already running. A client never compiles, never relaunches,
   never changes `H2_OPT`. Everything a client can change, it changes
   live: switches, state, input, overrides. A client that needs new code
   is out of scope; it writes tests instead, as now.
2. **Text first, pixels on request.** Every answer is structured JSON with
   stable ids. Screenshots exist, but come annotated with the same ids, and
   a client is never expected to read pixels to learn what a screen is.
3. **Every answer says when it is from.** Each reply carries `tick`,
   `present`, `map`, `bsp` and `phase` (menu, loading, cinematic, gameplay,
   paused, fading). A capture taken during a fade says so.
4. **Read at a tick boundary, on the game's thread.** World reads happen
   between guest ticks, inside a hook on the game's own thread, so a reply
   is one consistent moment, never half of one tick and half of the next.
   The candidate point is the post-tick hook `lockstep` already uses; no
   end-of-tick point has been verified safe yet, and co-op's per-tick
   simulation hash ([live-coop.md](../live/live-coop.md)) is the closest
   precedent (**S**). Reads from Rust are uncharged and invisible to the
   guest (as in `lockstep`). See [pitfalls](#pitfalls-from-the-live-work).
5. **Small answers by default.** Counts and summaries first, details by id,
   paging everywhere. An agent's context is the scarce resource; a 3,000
   object dump is a bug.
6. **Reproducible by construction.** An experiment is a snapshot plus
   recorded input plus a set of overrides. Same three, same result, checked
   with `lockstep` hashes. Anything that breaks that says so up front.
7. **Sim versus view.** Every override is labelled the way the feature
   engine labels features ([feature-engine.md](../engine/feature-engine.md),
   `sim: Host | View | Sim`). A `Host` or `View` override can't change the
   simulation, so A/B runs stay tick-identical. A `Sim` override changes the
   simulation on purpose, and the result says from which tick the runs
   diverge.

## Shape

```
          agent (Claude, Codex, a script, a test)
                    │  `h2 agent …` CLI      │  JSONL scenario files
                    └──────────┬─────────────┘
                        │ JSON-RPC 2.0, one Unix socket per game process
                        ▼
  ┌──────────────── game process (h2 play, or headless h2 serve) ─────────────┐
  │ agent server thread: parses, queues, never touches guest RAM itself       │
  │        │ requests                         ▲ replies, events                │
  │        ▼                                  │                                │
  │ tick-boundary executor (post-tick hook, between scheduler slices)          │
  │   reads: telemetry views, ui stack, hud, profile counters                  │
  │   writes: input queue, switches, overrides, snapshot save/load             │
  └────────────────────────────────────────────────────────────────────────────┘
```

- **Transport.** JSON-RPC 2.0 over a Unix socket the game opens when started
  with `H2_AGENT=<path>` (or `--agent`). Off by default; no listener, no
  cost. One client at a time holds control (`session.claim`); others may
  read. (**S**)
- **Two front ends, one protocol.** A CLI (`h2 agent <method> [json]`)
  for agents, shells and the user, and a scenario runner
  (`h2 agent run scenario.jsonl`) for repeatable experiments. Both are
  thin; the protocol is the contract. There is no MCP server: dropped
  (decided 2026-11-17), the CLI is what agents use.
- **Events.** `session.subscribe(["screen", "phase", "death", "checkpoint",
  "frame_budget_miss", …])` streams notifications, so a client waits on
  "the skulls page opened" instead of polling.
- **Headless.** `h2 serve` is `h2 run` with the agent socket on, no window,
  optional GPU rendering and optional real-time pacing (`H2_BOOT_REALTIME=1`).
  `h2 serve --system-link --host-light` always paces to real time. It is the
  shared instance agents use. The [dedicated host guide](../live/dedicated-host.md#light-host)
  describes the profile and the measurement procedure.

Method names are `domain.verb`. Every reply has the stamp:

```json
{"at": {"tick": 48211, "present": 96400, "map": "03a_oldmombasa", "bsp": 1,
        "phase": "gameplay", "fade": 0.0, "paused": false}}
```

## Domains

### `session`: what is running and its clock

| Method | Does |
|---|---|
| `session.status` | build id, image hash, disc map list, switches, phase, fps right now |
| `session.claim` / `release` | take or give up control |
| `session.pause` / `resume` / `step {ticks}` | freeze the sim; step exact ticks (render keeps presenting the paused frame) |
| `session.wait {until: <predicate>, timeout_ticks}` | block until a predicate holds: `{"screen": "skulls"}`, `{"phase": "gameplay"}`, `{"tick": 50000}`, `{"hs_global": ["x", ">", 3]}` |
| `session.subscribe {events}` | stream events |

`wait` is the replacement for every "sleep and screenshot" loop: it is
evaluated at the tick boundary, so it never misses a one-tick state.

### `host.metrics`

Read with `h2 agent hostmetrics`. This method takes no parameters. It
reports the latest complete window of 30 intervals between returns from
`GAME_TICK` (`0x00137fe0`). The first reading is available after 31 tick
returns. Until then it returns `UNAVAILABLE`.

| Field | Meaning |
|---|---|
| `ticks` | Tick intervals in the sample, currently 30 |
| `game_cpu_ms_per_tick` | CPU time on the game thread per tick interval, including work between ticks |
| `process_cpu_ms_per_tick` | CPU time on all process threads per tick interval |
| `wall_ms_per_tick` | Elapsed wall time per tick interval, including pacing sleeps and any pause or load crossed by the window |
| `cpu_percent` | Whole-process CPU over the wall interval. 100 means one fully used core |
| `rss_bytes` | Resident bytes at the end of the interval on macOS and Linux. This includes resident shared pages |
| `age_ms` | Time since the sample ended |
| `host_light` | Whether the process uses the light host profile |

A platform counter that is unavailable is returned as `null`. Compare
active gameplay windows after loading. A stale sample can survive a gap in
ticks. The CPU fields exclude sleeping. The wall field shows the pace the
host actually sustained. [Tick sampler](../../crates/boot/src/host_metrics.rs)

### `ui`: what the menus and HUD show

The client gets the **stack of open screens**, not pixels:

```json
{"at": {…, "phase": "menu"},
 "stack": [
   {"screen": "main_menu", "tag": "ui\\screens\\…", "layer": 0},
   {"screen": "campaign_difficulty", "tag": "…", "layer": 1,
    "focus": "row.2",
    "widgets": [
      {"id": "title",  "kind": "text",   "string_id": "…", "text": "Select Difficulty"},
      {"id": "row.0",  "kind": "button", "text": "Easy",      "enabled": true},
      {"id": "row.2",  "kind": "button", "text": "Heroic",    "enabled": true, "focused": true},
      {"id": "row.3",  "kind": "button", "text": "Legendary", "enabled": true}
    ]}],
 "native": false}
```

- **Static layout** comes from what `h2 ui` decodes today (`tags::ui`:
  widget kinds, rects, string ids). **Live state** (which screens are open,
  focus, list contents, enabled rows) comes from reading the game's screen
  stack. Where that lives in guest memory is a research point (**S**).
  Screens already moved to Rust (the Video screen, the skulls page, the
  account, co-op and sign-in screens; [feature-engine.md](../engine/feature-engine.md)
  Phase 3) report their own state directly, marked `"native": true`. They
  share one list-screen implementation (`crates/boot/src/list_class.rs`,
  **Code**), so one reader there covers all of them. Nobody reads the
  retail screens' live state yet; that is the new part.
- **Text is resolved at runtime** from the user's own disc and returned to
  the client. It is never written into the repository; tests use made-up
  strings, as `h2 ui` does today.
- **Acting on the UI** by meaning, not by stick presses:
  - `ui.focus {id}` and `ui.activate {id}` turn into the direction and
    button events that reach that widget, then wait for the stack to
    change. The game's menus receive these as menu events (kinds 1 to 4
    for directions, 5 for buttons, per the Live work), so injecting menu
    events is cleaner than faking pad packets and doesn't depend on
    repeat timing or the opening-press guard. `ui.press` keeps the pad path
    for tests of the input route itself.
  - `ui.navigate {path: ["Campaign", "Outskirts", "Heroic", "Start"]}` does
    the same across screens.
  - `ui.press {button}` stays for raw tests: "does Left reach the skulls
    page?" becomes `ui.press Left` followed by checking `widgets[i].text`.
- **HUD** is a separate view, `ui.hud`: health and shield, weapons and
  ammo, grenades, motion tracker contacts (as world ids and bearings),
  objective text, subtitles, the crosshair's target. Mostly read from the
  player and unit state `live-telemetry` already finds (**Code**:
  `PlayerState`, `Weapon`); subtitles and objectives are research points
  (**S**).
- `ui.screenshot {annotate: true}` returns a PNG path plus every widget's
  rect in pixels, so a client checking layout ("does the long skull name
  fit its row?") compares rects, not glyphs.

### `world`: what the level is and what is in it

Two layers. The **map** is static and comes from the tags. The **world** is
what is alive now.

**Map** (`world.map`). One call gives the shape of the level as a summary:

```json
{"map": "03a_oldmombasa", "bsps": [{"index": 0, "name": "…", "bounds": [[…],[…]]}, …],
 "counts": {"squads": 41, "spawns": 12, "trigger_volumes": 88, "scripts": 160,
            "device_groups": 30, "lights": 12, "cutscene_flags": 54},
 "zones": [{"id": "zone.3", "name": "…", "bsp": 1, "bounds": […],
            "squads": ["squad.12", "squad.13"], "triggers": ["tv.20"]}]}
```

Then per kind, by id or by filter: `world.map.squads {zone}`,
`world.map.scripts {name_like}`, `world.map.triggers`,
`world.map.lights` (what `h2 lights` reads), `world.map.placements`
(reusing `scenario-edit` and `live-telemetry::placements`). Zones are
derived (**S**): trigger volumes and BSP regions grouped with the squads
and scripts that name them, so "the courtyard fight" is something a client
can point at.

**World** (`world.query`). Live objects, filtered and paged:

```json
{"method": "world.query",
 "params": {"kind": ["actor", "vehicle", "projectile"], "near": {"player": 0, "radius": 30},
            "fields": ["pos", "team", "health", "squad", "state", "target"], "limit": 50}}
```

- Kinds: player, actor (AI: squad, state, target, combat status), biped,
  vehicle, weapon, projectile, equipment, scenery, machine, effect, light,
  sound source, particle system. The first six exist in `live-telemetry`
  (**Code**); effects, particles and sound sources are new reads (**S**).
  Built (**Code**): player, actor, vehicle, machine, body, scenery,
  weapon, equipment, control, crate, creature (see "Every kind of object"
  under Phase 1).
- `world.count {group_by: "kind" | "squad" | "bsp" | "zone"}` is the cheap
  first call. It answers "how much is alive" before anything is listed.
- `world.object {id}` gives everything about one object, including its tag
  path and the effects attached to it.
- **Facing** (**Code**, 2026-11-19). `forward` is where an object faces;
  for a unit (a player's, an actor, a body) it is where the unit aims:
  its aiming vector (unit `+0x168`, the vector the held weapon's targeting
  ray follows, `0x000cf3d0`), which turns with the camera. A biped's body
  turns only as it moves, so its own forward (`+0x70`) stayed put while the
  view turned; that is `body_forward` now. `yaw` (degrees about +z from
  +x, 0 to 360) and `pitch` (degrees up from level) are `forward` as
  angles, on every object with a `forward`. A unit whose aim reads as not
  finite or not unit length falls back to the body's forward.
  Units also carry `physics` (**Code**): crouch amount, the crouch request
  (unit flags bit 23, which stays set after release until there is room to
  stand), physics mode, the biped airborne bit (`+0x348` bit 0, likely) and
  its tick count (`+0x399`), the hitbox offset a melee lunge leaves
  (`+0x3aa`, ticks and units; docs/game/superbounce.md), and the raw unit
  and biped flag words. Polling
  `object player.0` on one `h2 agent -` connection and keeping one reply per
  stamped tick gives a per-tick movement trace.
  `physics.havok_pos` and `havok_velocity` are the object's first Havok
  body; [`havok.*`](#havok-the-physics-world) has every body and contact.
  Bipeds also carry `physics.support` (**Code**, layout in
  `live-telemetry`'s `layout.rs`, `BIPED_SUPPORT_*` and
  `CHARACTER_CONTACT_*`): what the biped physics update (`0x000dd990`)
  keeps of the ground. `flags` is biped `+0x349`, the high byte of the
  biped flags word at `+0x348` (`copied`: its bit 4, biped flags bit 12, set
  when this tick's update copied a support contact, `0x000ddeda`);
  `words` are `+0x354..+0x364` (what supports the biped, all `ffffffff`
  for nothing); `point`, `normal` and `contact_point` are `+0x368`,
  `+0x374` and `+0x380` (the support contact's normal and point,
  `0x000dde99..0x000ddef5`); `ground_state` is `+0x3ec`, the vector the
  ground test `0x001f19c0` measures first. `contacts` are the character
  contacts that test walks, on the biped's physics component (`+0x88`,
  count `+0x8c`, `0x48` each, at most 32): `key` (`+0x04`, the shape key),
  `other` (`+0x14`), `point` (`+0x1c`), `normal` (`+0x28`), `value`
  (`+0x38`; the first pass wants it at least 1e-4) and `flags` (`+0x44`).
  These are the contacts the airborne bit is decided from, unlike
  `havok.contacts`, which lists the solver's contact constraints. Step a
  replay (`claim`, `pause`, `step 1`) to read them tick by tick.
- `world.raycast`, `world.visible {from: "camera" | player}`: what the player
  can see, for both "is this on screen" and closed-loop movement.
- `world.overhead {bsp, size}` renders a top-down map image with numbered
  markers whose numbers are the ids in the JSON. A text form (a coarse grid
  with letters) exists for clients without vision.

Ids are stable for an object's lifetime and readable: `actor.squad12.3`,
`vehicle.warthog.1`, with the guest datum handle alongside for debugging.

### `profile`: where a frame goes, and who caused it

Frame time is broken down **twice**: by phase (what the engine was doing)
and by owner (which thing in the level made it do it).

```json
{"method": "profile.capture", "params": {"ticks": 600, "by": ["phase", "owner"]}}
→
{"frames": 1200, "ms": {"p50": 14.1, "p95": 22.8, "p99": 31.0},
 "phase": [
   {"name": "sim.ai",         "ms_p50": 3.2, "share": 0.23},
   {"name": "sim.objects",    "ms_p50": 2.0},
   {"name": "sim.effects",    "ms_p50": 1.4},
   {"name": "sim.hs",         "ms_p50": 0.3},
   {"name": "render.views",   "ms_p50": 5.1,
    "children": [{"name": "pass.shadow", "ms_p50": 1.9, "draws": 412}, …]},
   {"name": "audio.mix",      "ms_p50": 0.6},
   {"name": "present.wait",   "ms_p50": 0.0}],
 "owner": [
   {"id": "squad.12", "ms_p50": 1.8, "what": "ai.update 1.1, objects 0.4, effects 0.3"},
   {"id": "effect_class.plasma_impact", "ms_p50": 1.2, "count_p50": 37}],
 "spikes": [{"present": 96841, "ms": 48.3, "top": ["sim.effects", "pass.transparent"]}],
 "trace": "$RUSH_KEEP/…/trace.json", "samples": "$RUSH_KEEP/…/profile.folded"}
```

- **Phases** come from `timeline` spans already placed in the runtime, kernel
  and d3d (**Code**), plus new spans at the engine's own top-level update
  calls (AI, objects, effects, scripts) placed as feature-registry hooks
  (**S**). Render passes are known by name since `render_views` became Rust
  (feature-engine 7a).
- **Owners.** The cheap way: per-tick counts of what is alive (actors per
  squad, live effects per class, particles, projectiles, dynamic lights,
  draws per object) correlated with frame time across the capture. The
  precise way: a hook on the per-object update loop that times each
  object's update and charges it to its squad, effect class or tag (**S**;
  it adds dispatch cost, so the reply says it was on, like `lockstep`'s
  observer).
- **Spikes.** Each frame over budget is kept with its own breakdown. A
  p99 problem is mostly a few frames, so the average hides it.
- **Sampling** reuses `perf::Session` with guest symbols; the reply gives
  the top functions as names, and the folded stacks as a file.
- `profile.counters` is the always-on, near-free version: the last N
  frames' phase totals and counts, so a client can ask "what was that
  hitch" right after it happens.

### `state`: put the game where you want it

**Snapshots** are the core: the whole machine at one tick boundary, named,
saved and loaded in well under a second (**S**: not measured).

Built (types in `crates/agent-proto/src/state.rs`, the executor's part in
`crates/agent/src/exec/snap.rs`, the machine in
`crates/boot/src/snapshot_glue.rs`; tested over fakes, not run against
the game; [snapshots.md](snapshots.md#phase-5-what-was-built)):

| Method | Does |
|---|---|
| `state.save {name, replace?, max_ticks?}` | guest RAM, the kernel and its threads, D3D, audio, XAPI with the input cursor, every feature's section, and a RAM hash, at the first quiescent slice boundary after a tick |
| `state.load {name, max_ticks?}` | back to exactly that tick; the RAM hash (`lockstep::hash_guest`) checked against the saved one, naming the MiBs that differ |
| `state.list`, `state.drop {name}` | this build's snapshots, answered at once |
| `state.fork {from, name, replace?}` | load `from` and save it again as `name`, for branching experiments |

A save or load waits for the next tick, then retries at each tick while
something is live, for `max_ticks` (90) ticks, then fails with code 7
(`BUSY`) and the blockers in `data.blockers`. One sent while the game is
paused runs at the end of the held tick's slice; the game holds again at
the next tick. Calls other than `state.list` need control when another
connection holds it. CLI: `h2 agent state save NAME [replace]`,
`state load NAME`, `state list`, `state drop NAME`,
`state fork FROM NAME [replace]`.

Until 2026-11-19 every save on a serve answered `BUSY` with
"occlusion/native-world/native-objects/native-transparents/native-lights/native-post:
not audited for snapshots": each of those features declared its policy
after the early return it takes when its switch is off, so it counted as
unaudited. They declare it first now; switched on, each is a derived
render cache whose load drops what it holds (occlusion streaks, pending
guest-bit restores, light runs) or holds nothing past a frame
([snapshots.md, "The render features"](snapshots.md#the-render-features)).

The user plays to the fight once, presses a key (or the agent does it via
`session.wait` on a script global), and that moment becomes a fixture:
`oldmombasa.courtyard_fight`. Agents load it as often as they like. That
removes "wait for the user's frame stats" from the firefight work.

Snapshots are a superset of the game's checkpoint (which saves game state,
not the machine; [checkpoints.md](checkpoints.md)), so they need runtime
support: serialisable kernel objects, audio voices and render resources
rebuilt from guest state. That is the largest piece of work here and the
riskiest (**S**). The fallback is the game's own checkpoint plus level
boot: coarser, but it exists.

The design, the inventory of host-side state and the phases are in
[snapshots.md](snapshots.md).

Snapshots never enter the repository: they contain game data.

**Edits** change the state directly, at a tick boundary:

- `state.set {id, field, value}`: position, health, shields, ammo, team.
- `state.give {player, weapon}`, `state.teleport {player, to}`: built as
  `sim.give` and `sim.state.set {pos, yaw}`
  ([the game's own functions](#the-games-own-functions-teleport-give-spawn)),
  and `sim.spawn` for an object.
- `state.game {difficulty, skulls}`.
- `hs.eval {expr}`: run a HaloScript expression in the level, e.g.
  `(ai_place squad_12)`, `(ai_erase squad_12)`, `(object_create crate_3)`,
  `(device_set_position door_2 1)`. HaloScript is the designers' own control
  surface, so it already speaks squads, objects and devices. The native VM
  exists but isn't wired into the game ([hs_vm.md](../engine/hs_vm.md));
  the first version compiles one expression with `crates/hs` and runs it
  through the guest's evaluator. How to inject a node into the guest's
  script pool is a research point (**S**).
- `hs.globals`, `hs.threads`: read script state, so "has the courtyard
  fight started" is a global, not a guess.

### `input`: act like a player

Three levels, all ending up as pad packets through the normal input path,
so the game can't tell an agent from a person:

1. **Raw**: `input.pad {player, at_tick, steps: [{buttons, left, right,
   lt, rt, ms, ticks, reads}]}`; a queue of timed steps per player
   (**Code**, "Phase 3 and 4" below).
2. **Recorded**: `input.record {name}` / `input.replay {name}`, the
   `xapi::tap` log (**Code**), stored next to a snapshot so the pair replays
   exactly.
3. **Intent**: `input.look_at {target}`, `input.move_to {point}`,
   `input.fire {target, burst}`. These are small closed loops in the server
   that read `world` each tick and steer the sticks. They're for getting
   somewhere and shooting something, not for playing well (**S**).

Raw and recorded input keep runs deterministic. Intent input is
deterministic too, as long as it starts from the same snapshot, because
it only reads the simulation.

### `override`: replace parts of a mission

This is how a client asks "what if this weren't there". An override is a
named, reversible change, applied live:

| Override | Label | Example |
|---|---|---|
| `switch {name, on}` | as the feature says | `impl.render.views=on`, `e3_lighting=off` |
| `render.pass {name, off}` | View | drop the shadow pass |
| `render.lod_bias`, `render.scale` | View | |
| `effects.cap {class?, max}` | Sim | at most 8 live plasma impacts |
| `particles.cap {max}` | View if particles are render-only, else Sim (**S**) | |
| `ai.erase {squad}`, `ai.limit {squad, max}` | Sim | remove the second wave |
| `hs.script {name, off}` | Sim | stop a looping ambient script |
| `objects.cull {kind, beyond}` | Sim | no scenery past 80 world units |
| `tag.patch {tag, field, value}` | Sim | weaker emitter rate on one effect, in memory only (reusing `scenario-edit`'s patch model) |
| `impl {subsystem, backing: "bridge" \| "native"}` | as the subsystem says | run the Rust render loop against the recompiled one |

Overrides are switches from the feature registry, so they get its rules
for free: conflicts found when planned, ordered hook chains, one place
that touches dispatch ([feature-engine.md](../engine/feature-engine.md)).
None writes to the disc or to `.map` files.

### `experiment`: A/B a mission for fps

The loop the user asked for: same start, same input, different overrides,
compared.

```json
{"method": "experiment.run",
 "params": {
   "start": "oldmombasa.courtyard_fight",
   "input": "oldmombasa.courtyard_fight.input",
   "ticks": 1800,
   "measure": ["frame_ms", "phase", "owner", "draws"],
   "variants": {
     "base": [],
     "no_wave2": [{"ai.erase": {"squad": "squad.13"}}],
     "fx8": [{"effects.cap": {"class": "plasma_impact", "max": 8}}],
     "no_shadow": [{"render.pass": {"name": "shadow", "off": true}}]},
   "repeat": 3,
   "frames": {"every": 300, "skip_black": true}}}
```

For each variant the server loads the snapshot, replays the input, applies
the overrides, measures, and repeats. The reply is a table against `base`:

```json
{"variants": [
  {"name": "no_wave2",  "p50": -2.9, "p99": -11.4, "sim_diverges_at": 48230,
   "phase_delta": {"sim.ai": -1.6, "sim.effects": -0.7}},
  {"name": "no_shadow", "p50": -1.8, "p99": -2.0, "sim_diverges_at": null,
   "image_diff": 0.031}],
 "noise": {"base_p50_spread": 0.2},
 "frames": "$RUSH_KEEP/…/frames/"}
```

- `repeat` and `noise` give the run-to-run spread, so a 0.3 ms "win" inside a
  0.4 ms spread is reported as nothing.
- `sim_diverges_at` comes from `lockstep` hashes: `null` proves a View
  override changed nothing in the simulation.
- **Counterfactual frames.** `render.compare {variants}` renders one paused
  frame once per variant, in the same run. Only the variant differs, so the
  image diff measures the change, not noise between two runs (the E3
  lighting work needed exactly this). `skip_black` uses the `fade`
  stamp, so blank frames are never compared.
- A finished experiment is a JSONL file (`h2 agent run`) that anyone can
  run again. Its numbers live in `$RUSH_KEEP`, never in the repo.

The output of an experiment is a pointer, not a fix: "wave two costs 3 ms
at p50 because of its plasma impacts". The fix is a normal change with
tests; the experiment is how the agent found it, and how it checks the
fix later.

### Live and accounts

`session.live` reads `live-hle`'s state: sign-in, the account list, co-op
sessions, with sign-in calls coming from the in-game sign-in work. The
`live-server` HTTP API stays the way to read the server side; the API
doesn't wrap it.

## Pitfalls from the Live work

`live-telemetry` has read guest memory in a shipping feature for a while.
What it learned, and what this design takes from it:

- **Reuse the readers, not the transport.** `memory.rs` (readers),
  `layout.rs` (offsets) and `event.rs` (types) are tested and shared. The
  sink and session layer is not: it sends signed records in the background
  and drops them when its queue fills. The agent server reads on demand.
- **Check a handle's salt before trusting its slot.** Slots are reused
  within a tick. Every `world` id carries the datum handle with its salt,
  and a stale id answers `gone`, never another object's data.
- **Some data must be read before the game's function runs**, because that
  function clears it (hit reports, for example). Those reads are pre-hooks
  feeding a per-tick buffer, not reads at the tick boundary.
- **Layouts differ between campaign and multiplayer.** Readers branch on the
  game mode in the stamp, and tests cover both.
- **Hooks on shared per-tick entry points need an explicit order**, or the
  feature plan refuses to build (co-op hit this). The `agent` feature
  declares its order against `lockstep`, co-op and telemetry.

## Where the pieces go

| Piece | Crate | Notes |
|---|---|---|
| Protocol types, method table, JSON schema | new `agent-proto` | no runtime deps; the CLI uses it too |
| Server thread and tick-boundary executor | new `agent` | installed through the feature registry as feature `agent`, `sim: Host` |
| World reads | `live-telemetry::memory`, grown | shared with the Live service; one place for guest layouts |
| UI stack and HUD | `agent` + `tags::ui` | static trees from tags, live state from the screen stack |
| Profile | `perf`, `timeline` | new spans as hooks; owner attribution behind its own switch |
| Snapshots | `runtime`, `kernel`, `xapi` | serialisable host state; the biggest piece |
| HS eval | `hs` | compile one expression; inject via the guest evaluator |
| Overrides | `feature` | live-settable switches |
| CLI, scenario runner | `cli` (`h2 agent`, `h2 serve`) | no MCP server (dropped) |

## Phases

Each phase is useful on its own, and each is proven with tests over
synthetic guest memory and synthetic tags, as now.

1. **Socket and status.** `H2_AGENT`, `session.*` (status, pause, step,
   wait, subscribe), `world.count`/`world.query` over what `live-telemetry`
   reads, `profile.counters` from frame stats, switches settable live, the
   `fade` and `phase` stamp. The CLI front end. This alone fixes the
   black-frame captures and lets agents watch a session the user is playing.
2. **UI.** The screen stack, focus, `ui.press`, `ui.activate`,
   `ui.navigate`, `ui.screenshot` with rects; native screens first, since
   they already know their own state. Answers "did Left reach the skulls
   page" and "which screen did co-op open" in one call.
3. **Input and checkpoint-based state.** `input.*` raw and recorded,
   `state.*` on top of the game's own checkpoint plus level boot, `hs.eval`.
   Experiments become possible, coarsely.
4. **Profile by owner.** Engine-phase spans, per-object timing, spikes.
5. **Machine snapshots and experiments.** `state.save`/`load` of the
   whole machine, `experiment.run`, `render.compare`. The full loop.
6. **Map and intent.** `world.map` zones, overhead images, intent input.

## Phase 1: what was built

Crates `agent-proto` (wire types and a blocking client; no game
dependencies) and `agent` (the executor, the socket, the `agent` feature).
Boot wiring is `crates/boot/src/agent_glue.rs`; the CLI is
`crates/cli/src/agent_cmd.rs` and `h2 serve` in `boot_cmd.rs`.

```sh
H2_OPT=2 h2 serve disc/halo2.iso --out <the image you built>   # never builds
h2 agent status
h2 agent count
h2 agent query '{"kind":["actor"],"near":{"player":0,"radius":30},"fields":["pos","team"]}'
h2 agent watch phase map
printf 'claim\npause\nstep 30\ncount\nresume\n' | h2 agent -
```

How it behaves (**Code**, with tests in `crates/agent`):

- **Wire.** JSON-RPC 2.0, one object per line. The socket is `--socket`,
  else `$H2_AGENT`, else `~/Library/Caches/h2recomp/agent.sock`
  (`$XDG_CACHE_HOME` or `~/.cache` elsewhere), not `$TMPDIR`, which differs
  between agent sessions. A socket file nobody answers on is replaced; a
  live one is an error. `tcp://HOST:PORT` (as `--socket` or `H2_AGENT`)
  listens or connects over TCP instead (`agent_proto::Endpoint`), which
  is how the Xbox app is reached (`h2 agent --xbox HOST`, port 4720;
  [xbox.md](xbox.md) "Remote control"). TCP has no authentication. Unix
  sockets are Unix only; elsewhere use TCP.
- **Where answers are read.** The `agent` feature observes `GAME_TICK`
  last, after telemetry and lockstep (`boot/tests/feature_plan.rs`), and
  answers there with `boundary: "tick"`. Between slices it answers what
  doesn't read the world at once; world reads wait for the next tick
  while the game ticks, and are answered between slices
  (`boundary: "slice"`) once no tick has come for 500 ms.
- **Phase.** `loading` (no tick for 500 ms), `menu` (mode `main_menu`),
  `gameplay` (campaign or multiplayer), `cinematic` (between HaloScript's
  `cinematic_start` and `cinematic_stop` in a campaign or multiplayer game,
  observed; cleared when the map changes). The main menu is always `menu`:
  its background runs `cinematic_start` too. A level's first tick comes
  before its scripts start the opening cutscene, so every stamp has
  `map_ticks` (ticks since the map loaded) and `wait` takes it: ask for
  `{"phase":"gameplay","map_ticks":30}` to mean "playing". Fades to black aren't detected yet.
- **World.** Players (their units), AI bipeds, vehicles, machines and dead
  bodies, from `live-telemetry::memory`. Ids keep the datum's salt:
  a slot reused by another object answers `gone` (code 3). A turret is a
  vehicle with `mounted_on` set; its `pos` is relative to that vehicle. `near` sorts
  nearest first; `fields` trims; pages default to 50, at most 500.
- **Every kind of object** (**Code**, 2026-11-19). Besides those, a
  second pass over the object header table (`live_telemetry::things`)
  lists the engine's other object types by the header's type byte
  (`+3`): `scenery` (6), `weapon` (2), `equipment` (3), `control` (8, a
  device's switch or panel), `crate` (11) and `creature` (12), each with
  `pos`, `forward`, `yaw`/`pitch`, `tag` and `placement` (the scenario
  placement that made it). An object attached to another (a weapon a unit
  holds) is left out: its pose is relative to its holder, and the unit's
  `weapon` names it. So `weapon` and `equipment` are what lies in the
  world. `kind` filters, `near` and `world.count` cover the new kinds.
  Garbage, projectiles, light fixtures and sound scenery are not listed.
- **Tag names** (**Code**, 2026-11-19). Every object with a `tag` also
  has `tag_name`, the definition's path in the loaded map
  (`objects\vehicles\warthog\warthog`), from the map file's tag names as
  `edit.tags` reads them; it is absent when the names can't be read. A
  query's `tag` keeps one tag: a datum (8 hex digits, `0x` optional), or
  text the tag's name contains (any case, `/` for `\`). Halo 1 Blood
  Gulch's teleporter receivers, nearest first:

  ```sh
  h2 agent query '{"kind":["scenery"],"tag":"teleporter_base","near":{"player":0,"radius":500},"fields":["pos","tag_name"]}'
  h2 agent sim set player.0 'pos=[x,y,z]' 'forward=[1,0,0]'
  ```

  The names come from the file the game actually loaded. The loaded
  map's name is its cache header's (`0x00547898`); a converted map is
  served under a disc map's file name with its own name in its header
  (Blood Gulch is `h1_bloodgulch` in `H2_MAP_DIR`'s `coagulation.map`),
  so looking for `maps\h1_bloodgulch.map` found nothing, and every
  `edit.tags` name was null; now `h2 agent edit tags scen` names
  `scenery\teleporter_base\teleporter_base`.
  `boot::map_override::map_file` now tries the file name first, then the
  maps folder's files by their header's name (`+0x198`), each through
  the VFS's own layers, so the override is the file found. Telemetry,
  the native world, our skulls and `sim.give`/`sim.spawn` find the map
  the same way. If two files in the folder carry the same name, the
  first in folder order wins. A map installed as content on the data
  partition (not under the disc's `maps`) is not searched.
- **Control.** `pause`, `resume` and `step` need `session.claim`; one
  connection holds it. A pause blocks the game thread inside the tick
  observer, still answering requests, and resumes by itself after `max_ms`
  (default 60 s, at most 600 s; a second pause re-arms it), when the
  controller releases or disconnects, or when the game exits. A pause
  asked for while loading takes hold at the next tick; its limit runs
  meanwhile. `step N` runs N ticks and answers once paused again; a resume,
  a new pause or a release first answers it with `INTERRUPTED` (code 6).
  With the tick observer off, pause and step are refused (`UNAVAILABLE`).
  In `h2 play` a pause also stops the window's event loop, so it is meant
  for `h2 serve`.
- **Connections.** A client that half-closes (`nc -N`, `shutdown(SHUT_WR)`)
  still gets answers to what it sent; the connection closes after the
  last one. Closing releases control. Valid JSON that isn't a request
  gets `-32600`; unknown params fields are refused; batches aren't
  supported. Each connection queues at most 4096 lines for its client;
  a client that lets it fill isn't reading and is dropped (its control
  released), so the game thread never waits on a client.
- **Consistency between slices.** While the game ticks, answers between
  slices keep the last tick's state (and say `boundary: "tick"`) instead of
  reading mid-tick. During a load the mode reads as none, not `unknown_0`.
- **Stopping.** `h2 serve` removes its socket on Ctrl-C or SIGTERM. A
  socket left by a crash is replaced at the next start.
- **Settings.** A headless run has no settings store unless given one, and
  the menus built on it stay retail: the campaign's Game Options page
  (Start Level, the Halo 1 options, the skulls), the Video screen's rows,
  the Halo 1 options on Game Setup. Its log says `skulls: off: no settings
  store (a headless run); pickups are not kept`. `--settings DIR` (on
  `serve` and `run`) builds the store as `h2 play` builds its own
  (`boot::setting_defs::load`,
  `crates/cli/src/boot_cmd.rs`): `DIR/settings.toml`, then the `H2_*`
  variables, then the defaults, logged as `h2 serve: settings from
  DIR/settings.toml`. A setting changed in a menu is written back to that
  file, so give a copy, or a scratch directory, unless the run should
  change it. `h2 play`'s file is `<data>/settings.toml`, the same `--data`
  both commands default to (**Code**).

  ```sh
  H2_OPT=2 H2_UI_WATCH=1 h2 serve disc/halo2.iso --out <image> --settings <dir>
  ```

  For a headless System Link lobby, pass `--system-link` to `h2 serve`.
  This enables XNet with no service URL or Live accounts. The coordinator
  recipe uses one native process per console. [Dedicated host experiment](../live/dedicated-host.md#mac-system-link-spike)

  **Reaching a level select.** There is no campaign or level-select helper:
  `lobby mode campaign` (`agent_proto::sim::menu`) presses A on the main
  menu's Campaign row and expects screen `0xee`, and nothing goes further.
  Drive the rest with `h2 agent press` and read where the input went with
  `h2 agent ui` (`H2_UI_WATCH=1`). Which screen Campaign opens depends on
  the profile: the main menu's Campaign press (`0x2b16ad`..`0x2b16d4`)
  opens the level select (`0x2b130a`, the picker in its place when a
  second campaign is installed) only when bit 21 of what `0x18fb0d`
  returns is set (read from the generated code); otherwise it calls
  `0x148c3e(.., 3)`, which `boot::ui_watch`'s table of seen screens gives
  as `0xee` and then the loading screen `0xd1` (**Doc**). That the bit
  means "a campaign was played" is a guess (**S**); if so, a fresh profile
  reaches neither the picker nor Halo 1. Use a `--data` whose
  profile has the level select (the one `h2 play` uses does, once a
  campaign was played there), not `--fresh-data`. On the level select, a
  Halo 1 level's first entry is always listed (docs/halo1/campaign-picker.md
  "Only the first level listed"), and its difficulty opens Game Options
  even with no skull collected.
- **Waits.** `session.wait` settles at tick boundaries (and between
  slices for wall-clock timeouts and while loading). A condition already
  true answers at once; `timeout_ticks` and `timeout_ms` (default 30 s,
  at most 600 s) give `TIMEOUT` (code 2) with the stamp.
- **Counters.** `profile.counters` is the frame-stats rolling window
  (`timeline::phase`), the same numbers as `H2_FRAME_STATS`. When nothing
  else collects them, the glue starts a quiet 5 s window.
- **Features.** `features.list` returns the plan text the game printed at
  start. Switches can't change while the game runs yet.
- **Cost.** One more observer on the tick (dispatch transfers, as
  lockstep's), so compare runs that all have the agent on, or all off.
  `H2_FEATURES=-agent` removes the hooks; requests then get answers only
  between slices.

- **Host methods.** A host adds its own methods with
  `Agent::add_method(name, Access::Read | Access::Act, f)`; an `Act`
  method is refused (`NOT_CONTROLLER`) while another connection holds
  control. `boot::agent_glue` adds `profile.sample.start`, `.stop` and
  `.status` (the in-process sampler, with boot's `perf` feature;
  [xbox.md](xbox.md) "Profiling on the console") and `host.perf` (the
  last host perf line), and `ui.screen` (`h2 agent ui`): with
  `H2_UI_WATCH=1`, the menu screen player one's input goes to, read each
  tick (`boot::ui_watch`; `{"screen", "window", "slot",
  "live_menu_row", "mode", "dialog", "age_ms"}`, docs/live/live-coop.md
  12.6). `dialog` is the dialog up (the first screen on windows 0 to 4,
  any slot), or `null`: `{"id", "name", "window", "slot"}`, with `name`
  the dialog's tag (`dialog_live_signin` for the account picker,
  `confirmation_dialog`, `error_dialog_ok`, ...; `boot::ui_watch::
  dialog_name`) or `null` for an id not named yet. For the Clanker player
  card, `dialog.name` is `clanker_player_options`, resolved from the
  active screen's kit class. Its numeric id belongs to the retail base
  class and can also identify other screens. Dialogs read as
  `"screen": null`, so this is how a driver tells one from another; the
  `ui: screen none` line names it the same way (`, dialog 0x14
  dialog_live_signin (window 3, slot 4)`). The input
  methods are host methods too, added with the agent's input tap (phase 3
  below).

Not built in phase 1: fade detection, the UI stack, HUD, snapshots,
input, overrides, experiments, live switch changes. (The MCP front end
is dropped, not pending.)

## Phase 3 and 4: what was built

Raw and recorded input (phase 3 without `state.*` and `hs.eval`), and a
profile capture with spikes (phase 4 without owners or engine spans).
Input is `crates/agent/src/tap.rs` over `input::backend::remote`'s step
queue, the capture `crates/agent/src/capture.rs` and
`timeline::phase::capture_begin`/`capture_end`; boot puts the tap on
`xapi` in `crates/boot/src/agent_glue.rs`.

```sh
h2 agent press a                                # A for 150 ms
h2 agent pad '{"player":1,"at_tick":900,"steps":[{"buttons":["rt"],"ticks":30}]}'
h2 agent pad '{"steps":[{"left":[0,1],"ticks":90},{"buttons":["a"],"reads":2}]}'
h2 agent padstate                               # every port, open ports, mode
h2 agent record courtyard                       # ...play or pad...
h2 agent stop                                   # writes inputs/courtyard.input
h2 agent replay courtyard
h2 agent capture 900                            # 900 ticks, about 30 s
h2 agent capture 10000ms sample                 # h2 built --features perf
printf 'claim\npress start\nwait {"ticks":30}\ncapture 300\n' | h2 agent -
```

Methods: `input.pad`, `input.release`, `input.state`, `input.record`,
`input.replay`, `input.stop` (host methods the input tap adds; `Act`
access but `input.state`, so no claim is needed while nobody holds
control), and `profile.capture` (core, read-only).

How it behaves (**Code**, with tests in `crates/agent`, `crates/xapi` and
`crates/timeline`):

- **Where input goes in.** The agent's tap is an `xapi::tap::InputTap`,
  the layer `H2_INPUT_RECORD` and `H2_INPUT_REPLAY` use: it sees each
  `XInputGetState` result just before the guest gets it, so the game reads
  agent input exactly as it reads a pad. Every host that runs the game
  through boot's XAPI has it: `h2 serve` (headless pad), `h2 play` and
  the Xbox app. (The first Xbox build merged steps into the app shell's
  player-one pad, `KeyboardFallback`, which `h2 serve` doesn't use; the
  tap replaced it so there is one mechanism.) It wraps the tap those env
  vars set: a replay runs first (it stands in for the pad) and agent
  steps go on top; a recording runs last, so it logs agent steps too.
  Taps are told which port each handle opened on (`InputTap::opened`),
  which is how a step for `player` 1 finds controller port 1. Without
  XAPI input there is no tap, and the input methods aren't listed.
- **Pad steps.** `input.pad {player, at_tick, steps, append}`, each step
  `{buttons, left, right, lt, rt, ms, ticks, reads}`. `player` is the
  controller port (0 to 3, default 0). Each port has a queue of steps
  (`input::backend::remote::Remote`). A step holds for `ms` of wall time,
  `ticks` of game time (the stamp's `tick`, the same on every run), or
  both, and until the game has read the pad `reads` times (default once),
  so a short press is never lost to a slow frame; with neither `ms` nor
  `ticks` it holds until replaced. `at_tick` (a tick, or `"now"`, the
  default) holds the first step back until the game's tick reaches it.
  `append` adds to the queue, otherwise it replaces it; `input.release
  {player}` empties one port's queue, or all of them. A step merges over
  what the read returns (`input::backend::fallback::merge`, as the
  keyboard merges over a pad): buttons are or'd, a stick or trigger the
  step moves wins. Buttons are `a b x y black white lt rt` (analog, fully
  pressed), `up down left right start back ls rs`; sticks `[x, y]` from
  -1 to 1 (up is +y); triggers 0 to 1. The packet number moves whenever
  the pad the game gets changes, as a real pad's does; until the first
  step, reads pass untouched, so a run with the agent on and no steps is
  byte for byte the same input. Queues outlive the connection that filled
  them (so a one-shot `h2 agent press` works) and are emptied when the map
  changes, since the tick starts again. The reply says `open: false` when
  the game hasn't opened that controller: the steps are kept, but nothing
  reads them. `input.state` gives `pad` (player one), `pads` (every port:
  `active`, `queued`, `done`, what is held), `open` ports, `tick`, and the
  recording or replay below.
- **Recording and replay.** `input.record {name, replace}` logs every
  read (status, handle, the 22 bytes, the game tick) to the cache
  directory's `inputs/<name>.input` in `xapi::tap`'s text format, the one
  `H2_INPUT_RECORD` writes; it's written at `input.stop`, or when the game
  exits. Names are letters, digits, `.`, `_`, `-`; an existing log is kept
  unless `replace` is true. `input.replay {name}` answers reads from a log
  read for read (keyed by read index, as `H2_INPUT_REPLAY`). `input.state`
  and `input.stop` give its counts: `reads` in the log, `replayed`,
  `matched` (same handle as recorded), `handle_mismatches` (the runs have
  parted) and `past_end` (left live). One recording or replay at a time.
  A replay only reproduces a run from the same starting state: until
  `state.*` exists, that means starting both from the same boot.

#### `input.host`: the host controllers

`input.host` (`h2 agent hostpads`, read-only, no params) shows the
controllers the host's input layer has, before the game sees anything.
Use it when a pad, a button or the touchpad doesn't do what it should.
It answers from the last input sync (`syncs` counts them), which runs
whenever the game reads its pads. Boot adds it wherever the game has XAPI
input (`crates/boot/src/debug_menu_glue.rs`, `crates/agent/src/hostpads.rs`).

- `devices`: each controller as its own backend reads it, before any
  merging (`input::HostBackend::devices`). From Apple GameController:
  `backend` (`game-controller`), `id`, `name` (the vendor name), `profile`
  (the extended profile's class: `dualsense`, `dualshock4`, `extended`, or
  `micro` for an Apple TV remote), `category` (the framework's product
  category, such as "DualShock 4"), and the state below. Other backends
  don't describe their pads yet; theirs show under `pads`.
- `pads`: each pad the input manager polled, as the debug menu's gate is
  handed it: `backend` (the top-level one, such as `keyboard-fallback`),
  `key` (backend index and id, the gate's key), `name`, `physical`, `port`
  (0 to 3, or null) and the state. With the keyboard on, player one is a
  single pad that merges the first controller with the keyboard.
- The state, in both lists: `touchpad` (null without one, else `click`,
  and `primary` and `secondary` as `[x, y]` from -1 to 1 with +y up, or
  null for a lifted finger), `buttons` (`south east west north
  left_bumper right_bumper back start left_stick right_stick dpad_up
  dpad_down dpad_left dpad_right`; GameController's menu button is
  `start`, Options is `back`), `left_stick`, `right_stick`, `triggers`.
- `gate`: null with no debug menu, else `open`, `syncs`, and per pad `key`,
  `touchpad`, `click`, `chord` (click and Start together), `chord_pending`
  (Start held back waiting for the click), `opened_by_press` (the click
  that opened the menu is still down) and `latched` (buttons hidden until
  let go) ([debug-menu.md](debug-menu.md)).
- `notes`: where a touchpad goes missing, in words: a plain `extended`
  profile, a DualShock 4 profile without touchpad elements, a touchpad
  under `devices` but on no pad the gate sees, no debug menu.

```sh
H2_AGENT=/tmp/h2-play.sock h2 play           # then, holding the touchpad down:
h2 agent --socket /tmp/h2-play.sock hostpads
```
- **Capture.** `profile.capture {ticks | ms, budget_ms, spikes, sample}`
  keeps every frame-stats row (`timeline::phase`, the numbers
  `H2_FRAME_STATS` prints) over the window, then answers: `frames`, `fps`,
  `frame_ms` and `busy_ms` (frame less pacing) as mean, p50, p95, p99, max
  and total; the same for each phase (`cpu`, `decode`, `encode`, `submit`,
  `gpu-wait`, `pace`, `render-wait`, `d3d`, and `render` when the
  render-timing feature measures it); mean, max and total of each counter
  (`draws`, `passes`, `uploads`, …); `over_budget`, the frames longer than
  `budget_ms` (default 33.3); and `spikes`, the worst of them (20 by
  default, at most 500), each with its present number, the game tick and
  map it ended in, its own phases and counters, and its three biggest
  phases (pacing left out). Every frame is also written as a CSV row
  (present, end, tick, map, total and busy us, each phase in us, each
  counter) under the cache directory's `captures/`, and the reply gives
  its path. A frame belongs to the last tick that had ended when it did.
  By ticks the window is 1 to 36000 game ticks (it gives up after 600 s
  with `complete: false`); by `ms`, 1 to 600000. One capture at a time; it
  is read-only, so it needs no claim. With frame stats off it answers
  `UNAVAILABLE` (the agent starts a quiet collection when nothing else
  does, so they are on unless something stopped them).
- **Sampling.** `sample: true` also runs the in-process sampler
  (`profile.sample.*`, boot's `sampling`) over the window, and `sampling`
  is what `profile.sample.stop` answers: the module-relative dump
  (`profile.h2prof` under `H2_SAMPLE_DIR`, which `h2 profile remote`
  resolves) and its sample count. Only an h2 built with `--features perf`
  on a host with a sampler has it; otherwise, or while a
  `profile.sample.start` run holds it, the capture still runs and
  `sampling.unavailable` says why.
- **World counts of effects, projectiles and particles.** Not built.
  `live-telemetry` doesn't read them: its world scan reads every object
  header, so projectiles would be a cheap count by the header's type byte
  once the projectile type number is established here (only biped 0,
  vehicle 1, weapon 2 and machine 7 are); effects and particles live in
  their own datum arrays, whose guest addresses are research points.

Not built in phases 3 and 4: `state.*` (machine snapshots are being
designed separately), `hs.eval`, intent input (`look_at`, `move_to`),
per-owner attribution and engine-phase spans, events for frame-budget
misses, and the effect, projectile and particle counts above. Nothing
here was run against the game: input reaching the guest is proven at the
`xapi` tap with synthetic pads, captures with synthetic frames and a fake
sampler.

## Phase 5: testing in the lightest way (plan)

What an agent can already do per area, what's missing, and the lightest
way to provide it. The rule stays: agents never build an image or run the
game; they use the user-started shared `h2 serve` through `h2 agent`, and
tests. Everything below is a method on that serve, a CLI verb, or a
host-only tool. (**S** for the plan; the built parts say so.)

Domains `edit.*` (live tag writes in the running game) and the hot-reload
half of `render.*` (render passes and shaders reloaded in place) belong to
a separate effort; this phase leaves their names free and builds neither.

| Area | Have | Missing | Lightest way |
|---|---|---|---|
| Profiling | `profile.capture`: per-frame phases (`cpu`, `render`, `decode`, `encode`, `submit`, `gpu-wait`, `render-wait`, `d3d`), counters, spikes; the sampler | why a spike happened; where the fallback interpreter runs; per-subsystem guest time; A/B of two feature sets on one recording | spike `causes` from the frame's own counters (built); `profile.fallbacks` (built); engine-phase spans as feature-registry hooks (phase 4 plan); A/B as two `h2 lab` runs of the same input log, run by the user or coordinator |
| Render pipeline | frame counters (`draws`, `passes`, pass splits); `H2_BOOT_CAPTURE`, `H2_BOOT_DRAWLOG` at launch only | a frame's passes, draws and targets on request; interp and pipelined-render counters and hold reasons; which native paths a frame took; one pass's image | `render.frame` and `render.stats` (built) |
| Features | `features.list` (the plan text) | state as data; setting a switch live | `features.list` grows `{name, on, sim, live}` per switch; a `features.set` for the `Host`/`View` switches whose hooks read a flag each call, and for the rest the reason a restart is needed (the switch places or removes hooks at build) |
| Loading | `present N: last 100 at X fps` lines; `read_bytes` per frame | a load's timeline; stalls; loading a named level or BSP | `load.timeline` (built: stall and settle with bytes, uploads, compiles, phase times); seeks and cache hits need kernel I/O counters (later); `load.level` needs a runtime level switch (the game's own map change, through `hs.eval` once it exists) |
| Editing, test maps | `scenario-edit` (placements, undo, save), `h2-editor`, `h2 map testbuild` | a scripted, headless way to make a small test map | `h2 testmap`: a JSON edit script (clear blocks, keep, add, set, remove) over `scenario-edit`, saved to a new `.map` (built, on disk only) |
| Lightest testing | unit tests over synthetic data; `h2 agent -` scripts | assertions; one call that runs a scenario and returns a verdict | `h2 agent run script.txt`: the `-` script with `expect` lines (JSON-pointer comparisons on the last answer) and one JSON verdict with each call's timing (built) |

What to reach for first, cheapest first:

1. **A unit test** over synthetic data (as every crate here does). Seconds,
   no game.
2. **The shared serve**, read-only: `status`, `count`, `counters`,
   `render`, `loads`, `fallbacks`, `frame`. No claim, nothing changes.
3. **A scripted scenario on the shared serve** (`h2 agent run`): claim,
   pause, step, press, capture, `expect`, release. Deterministic up to the
   starting state; replays an input log for longer runs.
4. **A test map** made by `h2 testmap` and loaded by the user or
   coordinator (`H2_MAP_DIR`), for a repro that needs one thing in an
   empty room.

What would make each cheaper (not built): a test map whose BSP is one room
loads in seconds instead of a campaign level's tens; `state.*` snapshots
make replays start anywhere; `load.level` makes the shared serve switch
level without a restart.

## Phase 5: what was built

### Render frame and pipeline counters

```sh
h2 agent frame                     # the next whole frame: passes, totals, paths
h2 agent frame draws               # ... and every draw, with its depth state
h2 agent frame constants -38..-35,58,59   # ... and those vertex constants on each
h2 agent frame snap 0,3            # ... and passes 0 and 3 read back as PNGs
h2 agent render                    # interp, pipelined render, native paths
```

Methods `render.frame` and `render.stats` (host methods: boot's agent glue
adds them; types in `crates/agent-proto/src/render.rs`, logic in
`crates/agent/src/render.rs`). How it behaves (**Code**, tested in
`crates/agent`, `crates/boot`'s `d3d_diag` and `render_probe`, and
`crates/feature/tests/render_loop.rs`; not run against the game):

- **Recording.** `render.frame {"arm": true, "draws", "max_draws",
  "constants", "snap"}` arms the recorder; the next present starts the frame and the
  one after ends it, then it disarms. `render.frame {}` reads the state
  and the last frame (`frame.seq` is the arming's number). `h2 agent
  frame` does both and waits up to 10 s; a paused or loading game
  presents nothing. Unarmed, the cost is one relaxed load per draw, clear
  and present in the draw wrapper (`boot::d3d_diag`), which sits in front
  of the backend (the render thread and in-between frames are behind it,
  so the frame is the guest's, as the D3D layer handed it over).
- **The frame.** `totals` (passes, draws by outcome, vertices, clears,
  copies, reads, distinct targets); `passes`: runs of work on one colour
  surface and depth (a return to an earlier target is a new pass), each
  with its target (`640x480 X8R8G8B8`), clears, draws, vertices, outcomes,
  primitives, D3D entries, texture formats, and counts of pixel-program,
  blended and depth-writing draws; `ops`: copies and readbacks with the
  draw count before them; `paths`: calls into each native render path
  during the frame. Surfaces are the backend's handles as text. With
  `draws`, each draw (at most `max_draws`, 2000 by default; `truncated`
  says when there were more).
- **A draw.** Its pass, D3D entry, primitive, vertex and index counts,
  outcome, texture formats, whether it has a pixel program, blends or
  writes depth; `vs`, the vertex side: `fvf 0x…`, or `prog @start n=slots
  #hash` for a vertex program (the low 32 bits of the words' FNV-1a hash,
  the same in every frame, so one program's draws can be picked out);
  and `depth`, how it tests and stores depth: `mode` (`D3DRS_ZENABLE`:
  `off`, `z`, `w`), `w` (W-buffered as drawn: `SET_CONTROL0`'s
  z-perspective bit once pushed, else `mode`), `func` and `write`
  (`D3DRS_ZFUNC`, `D3DRS_ZWRITEENABLE`), `viewport` (the D3D viewport's
  `[MinZ, MaxZ]`), `scale` (`SET_VIEWPORT_SCALE` z and
  `SET_VIEWPORT_OFFSET` z), `clip` (`[SET_CLIP_MIN, SET_CLIP_MAX]`),
  `full` (the depth target's full scale, 16777215 for D24S8; 0 with no
  depth target) and `clamp` (`D3DRS_DEPTHCLIPCONTROL` clamps out-of-range
  depth rather than culling it; shown only when on). A W-buffered draw of
  a vertex program stores its w clipped (or clamped) to `clip`, over
  `full`; a z-buffered one its z over `full`.
- **Constants.** `constants` (with `draws`; at most 32 registers, D3D
  numbering -96..=95, hardware register = number + 96) records those
  vertex shader constant registers on each listed draw, as it saw them:
  `"constants": {"-38": [x, y, z, w], ...}`. Only on request: the block
  is the draw's own (shared with the backend), copied out per listed
  draw. `h2 agent frame constants -38..-35,58` takes ranges and implies
  `draws`; `max N` sets `max_draws`.
- **Snapshots.** `snap` (at most 4 pass numbers): when such a pass ends
  (the next pass starts, or the present), its colour target is read back
  as BGRA and written as a PNG under the cache directory's `frames/`
  (`frame-<seq>-pass-<n>.png`); `snaps` gives each file or error. A pass
  with no draw has no known size and says so. This is a GPU round trip,
  only when asked.
- **Counters.** `render.stats`: `interp` is `d3d::interp::InterpStats`
  with every field (frames blended and held, replays, warps, and the hold
  reasons `unsure_*`, `mismatch_*`, `dropped`), read from its `Debug`
  text so fields added there appear with no change here; `pipeline` is the
  pipelined render's totals (`pipelined`, `completed`, `serial` reasons,
  `fallbacks` by call, joins, times in ms); `paths` is calls into each
  native render path since the start (`feature::render_loop::path_calls`:
  `render_views`, `layer_draw`, `layer_entries`, `render_game_view`,
  `render_scene_passes`, `render_states`); `counters` the frame-stats window's line. Each is
  `null` where that piece isn't running.
- **Interp on a headless serve.** `interp` is there when in-between
  frames run: in `h2 play`, or in an `h2 serve` that renders on the GPU
  off-screen (`H2_BOOT_GFX`) with a switch on (its counters move while
  the frame rate is above 30).
  The in-between frames are paced on the wall clock at the frame rate
  (no display) and made and classified for real, so the hold reasons are
  the game's ([high-framerate.md](../game/high-framerate.md) "Headless").
  The serve, for E1:

  ```sh
  H2_OPT=2 H2_BOOT_GFX=$TMPDIR/h2-gfx H2_BOOT_GFX_EVERY=never \
    H2_FPS=60 H2_FEATURES=impl.render.reproject \
    h2 serve disc/halo2.iso --out <the image you built>
  ```

  `impl.render.interpolate` gives D instead. `H2_BOOT_GFX_BETWEEN=N`
  also writes the first N in-between frames as PNGs to that directory,
  with the guest frame before and after each. `interp.holds` lists the
  draws E1's screen guard held frames for, kind by kind, each with its
  guest draw and pass number (as `render.frame` numbers them), vertex
  program, transform and where it lands
  ([high-framerate.md](../game/high-framerate.md) "Held frames, draw by
  draw").
  Without `H2_BOOT_GFX`, with `H2_RENDER_THREAD=0` or under
  `H2_PIPELINE_RENDER`, `interp` stays `null`.

### Hitch causes and fallback hits

```sh
h2 agent capture 900               # spikes now say why, and "causes" counts them
h2 agent fallbacks 20              # where the fallback interpreter runs
```

- **Causes.** Each spike in `profile.capture` has `causes`, and the reply
  has `causes` counting every frame over budget (listed or not) by
  cause (`crates/agent/src/capture.rs`, `causes`):
  `shader_compile` and `pipeline_compile` (any on the critical path),
  `texture_uploads` and `gpu_readback` (more than twice the window's
  median, and at least one), `file_io` (the guest's file calls took at
  least 2 ms of host time in the frame, on any guest thread, or at least
  a tenth of its busy time: the `file_io_us` counter; bytes read alone
  never count, since a read finishes inside the call at about 38 us per
  32 KB and the I/O pool's thread is not the game's), `gpu`, `render_thread` and
  `pushbuf_decode` (that phase is at least 40% of the frame's busy time),
  else `guest_cpu` when the guest's own time is the biggest phase. These
  are what the counters show, not a proof: a frame can be slow for a
  reason nothing counts.
- **Fallbacks.** `profile.fallbacks {"top": N}` (host method; 20 by
  default, 1 to 500): the fallback interpreter's totals and its entry
  addresses ranked by instructions interpreted, each with hits, x87
  instructions and the function it is in (the nearest analysed start at
  or below it). Kept per address in `boot::fallback::Stats::by_site`, at
  most 4096 addresses (`sites_full` says when later ones count in the
  totals only).

### Load timeline

```sh
h2 agent loads                     # the last 8 loads, BSP switches and stalls
h2 agent wait '{"phase":"gameplay","map_ticks":30}' && h2 agent loads 1
```

`load.timeline {"last": N}` (core method; 8 by default, 1 to 32 kept;
`crates/agent/src/loads.rs`). How it behaves (**Code**, tested in
`crates/agent` and `crates/timeline`; not run against the game):

- **What a load is.** Read at each tick: the map changed (`map`), the BSP
  changed (`bsp`), or no tick came for 500 ms (`stall`, the same point
  the `loading` phase starts). The run's first tick ends a `boot` load
  timed from when the agent started. Each load has a `stall` segment
  (the last tick before to the first after) and a `settle` segment (the
  next 30 ticks); a load cut short by the next one says `settled: false`.
- **What each segment counted.** The difference of the frame-stats
  running totals (`timeline::phase::totals`, new) across it: frames, ms
  per phase (`cpu`, `gpu-wait`, `render-wait`, …) and each counter that
  moved (`read_bytes`, `uploads`, `shaders`, `pipelines`, `readbacks`,
  …). Frames are counted by present, so a load screen that presents
  shows its frames; one that doesn't shows the stall's time only. With
  frame stats off, `no_frame_stats` says the counts are missing.
- **Now.** While no tick has come for 500 ms, `current` is the stall so
  far, with what it has counted.
- Seeks and cache hits aren't counted (the kernel's file I/O counts bytes
  and the host time of each file call: `read_bytes` and `file_io_us`, the
  latter in microseconds), and per-frame I/O stalls are the `file_io_us`
  counter and the `file_io` spike cause above.

### Test maps on disk

```sh
h2 testmap ~/maps/zanzibar.map one-weapon.json --out $TMPDIR/tm/zanzibar.map
```

`h2 testmap <source.map> <script.json | -> --out <new.map> [--force]`
(`crates/cli/src/testmap_cmd.rs`) runs a JSON edit script
(`scenario_edit::script`, `crates/scenario-edit/src/script.rs`) over the
source's placement blocks through the editor's own `Session`, so every
edit is checked as the editor checks it, and saves a new map (the source
is never written). It prints what each operation did and the entries
left per block. Operations: `clear` (blocks, or `"all"`; a block the
editor won't empty is reported and left), `keep` (these entries of a
block, renumbered from 0), `add` (a copy of `like`, or a zeroed entry
with the palette's first item, then `set`), `set` (fields by the
editor's names: `position` `[x, y, z]`, `rotation`, `facing`, `team`,
palette `type`, …), `remove`. `allow_growth` lets a block grow past its
source size, as the editor's switch. Blocks: `scenery`, `vehicles`,
`equipment`, `weapons`, `machines`, `sound_scenery`, `crates`,
`starting_locations`, `netgame_flags`, `netgame_equipment`,
`starting_equipment`. Tested on the editor's invented fixture map
(`crates/scenario-edit`); no game data.

The user or coordinator loads the result (`H2_MAP_DIR`); agents don't.
Not covered: AI (squads and their spawn points aren't in the editor's
blocks yet), and tag references other than through a palette index.
Live edits in a running game are the separate `edit.*` effort.

### Scripted scenarios with a verdict

```sh
cat > grenade.txt <<'X'
wait {"phase":"gameplay","map_ticks":30}
expect /at/phase == "gameplay"
claim
press rt 100
capture 90
expect /over_budget <= 2
expect /causes missing
release
X
h2 agent run grenade.txt          # one JSON verdict; exit 1 unless it passed
```

`h2 agent run <script|->` (`crates/cli/src/agent_run.rs`): the `h2 agent
-` script on one connection, plus `expect <pointer> <op> [JSON]` on the
last answer (a JSON pointer; ops `==`, `!=`, `<`, `<=`, `>`, `>=` on
numbers, `contains` for a substring, an array element or an object key,
`exists`, `missing`, and `near {"to", "within"}` since `sim`) and `keep`
(put the last answer in the verdict). The expectations are
`agent_proto::expect`, which `sim.expect` judges in the game too.
The verdict is `{"pass", "failed", "ms", "calls": [{line, call, ms, ok,
error}], "expects": [{line, expect, want, pass, got}], "kept",
"stopped"}`. A failed expect is recorded and the script goes on; a failed
call stops it. Answers aren't printed except kept ones, so a long
scenario costs a few lines of context. Tested with a fake caller; not run
against the game.

Not built in phase 5 so far: per-subsystem guest time (engine-phase
spans), A/B of feature sets, features set live, kernel I/O seeks and
cache hits, `load.level`.

## `sim`: simulated players and direct state

Built 2026-11-18, tested over synthetic guest memory, fakes and a socket;
not run against the game. Agreed with the Live and co-op coordinator
(bed6c8dc), who reviews this section.

`sim.*` lets an agent test co-op, multiplayer and Live flows from one
running game: players that aren't people, game state written directly,
and assertions that answer pass or fail with what was seen. It never
starts another game instance.

### Methods

| Method | Access | What |
|---|---|---|
| `sim.player.add` | act | `{kind: local\|phantom, source: script\|recording\|brain, team?, steps?, recording?, port?, identity?}`: a simulated player, `sim.<n>` |
| `sim.player.remove` | act | `{id}` or `{all: true}` |
| `sim.player.list` | read | `players` lists simulated players and their game player (`player.<i>`). `clankers` lists the host's brain-driven bots and their current brain state. |
| `sim.player.pad` | act | `{id, steps, append?}`: pad steps (`input.pad`'s) for one |
| `sim.state.set` | act | `{player, pos?, yaw?, pitch?, facing? (or forward?), team?, health?, shield?, grenades?, ammo?}`: host writes; `pos` and the facing are the game's own teleport, answered after the next tick |
| `sim.give` | act | `{player, weapon, ammo?, keep?}`: the game makes a weapon from a `weap` tag (datum or name) and gives it to the player's unit; answered after the next tick |
| `sim.spawn` | act | `{tag, pos, yaw?}`: the game makes an object from its tag at `pos`; answered after the next tick |
| `sim.view` | read | `{roots?}`: the game as one JSON document |
| `sim.expect` | read | `{checks}`: expectations judged against `sim.view`, at one tick |
| `sim.session` | read | the session kind and whether writes are allowed |

For a brain-driven simulated player, `sim.player.add`, `sim.player.list`
and the `sim` root of `sim.view` include `brain` when the backend provides
it. The list response's `clankers` array includes brain-driven local and
phantom bots added by the game, headless matches, and sim. Its entries have
`name`, `kind`, `brain`, and `port` or `player` once placed or joined.
Script and recording sources have no brain state.

For a joined phantom, `h2 agent sim list` includes `clankers[].aim` once
the host has gathered input for its current unit. `tick` is the gather
tick. `stick` (right, up) and `trigger` are the brain's latest report.
`arrived_stick` and `arrived_trigger` are the report delivered by the
uplink for that gather. `action_deg` contains the accepted slot record's
absolute [yaw, pitch] in degrees, or null if the gather was rejected.
`fire_primary` is the accepted primary fire bit and `assist_strength` holds
the two weapon strengths. A rejected gather reports false and zeros.
`unit_forward` is the unit's actual aiming vector at query time.
`assist_player` identifies the player controlling the selected aim-assist
target when one exists. To verify damage, use `h2 agent sim view player`
to read the target's shield and health.

The `brain` object has `skill`, `personality` and `mood`. Personality
contains the nine 0–1 traits in [the Clankers trait table](../game/bots.md#personality-and-temperament)
and `favorite_weapon` (`spray`, `precise`, `scatter`, `sniper`, `blast`,
or `blade`). Mood has `confidence`, `tilt`, `fear`, `streak`, and an
optional `revenge_target` (`player.<i>`) while a killer is remembered.

They are game methods (`Agent::add_game_method`, `crates/agent/src/sim/`):
answered on the game's thread with guest memory at hand, and, like
`world.*`, at a tick boundary while the game ticks. The wire types are
`agent_proto::sim`.

**`sim.view`'s roots.** `at` (the stamp), `session`, `ui` (`ui.screen`'s
answer, `null` without `H2_UI_WATCH=1`), `player` (by index: the
`world` object plus `health`, `shield`, `grenades {frag, plasma}` and the
held weapon's `ammo {loaded, reserve}`), `object` (every `world` object by
id; only when asked for), `count` (by kind, `team` counting players per
team, `total`), `sim` (simulated players by `<n>`, and `count`) and
`input` (`input.state`). Pointers read straight off it:
`/player/1/team`, `/object/vehicle.e2000003/occupied`, `/count/team/1`,
`/sim/1/player`.

**Expectations** are `h2 agent run`'s: `"<pointer> <op> [JSON]"` or
`{"pointer", "op", "want"}`, ops `==` `!=` `<` `<=` `>` `>=` `contains`
`exists` `missing` and `near {"to": number or array, "within": d}` (for
positions and vitality, read from `f32`s). One implementation,
`agent_proto::expect`, serves both: `h2 agent run` judges the last
answer on the client, `sim.expect` judges `sim.view` in the game. Its
answer is the verdict's fields, `{"pass", "failed", "expects": [{expect,
pass, want, got}], "at"}`.

### `h2 agent sim` and scripts

There is one script format: `h2 agent run`'s. Any line may be a `sim`
verb, and `h2 agent sim run <script>` is `h2 agent run` that also removes
the simulated players the script added (`"cleanup"` in the verdict).

```sh
cat > bot-team.txt <<'X'
sim session
expect /kind == "mp_host"
sim add local brain
expect /port exists
wait {"phase":"gameplay","map_ticks":30}
sim until 600 /sim/1/player exists
sim set sim.1 team=1 health=0.5 pos=[12.0,-3.5,2.0]
expect /wrote contains "pos"
sim view player sim
expect /sim/1/player == "player.1"
expect /player/1/team == 1
expect /player/1/health near {"to":0.5,"within":0.01}
sim pad sim.1 [{"left":[0,1],"ticks":60}]
X
h2 agent sim run bot-team.txt
```

Verbs (`crates/cli/src/agent_sim.rs`): `add <local|phantom> [source]
[key=JSON...]`, `remove <sim.N|all>`, `list`, `pad <sim.N> <steps>
[append]`, `set <player.N|sim.N> key=JSON...`, `give <player.N|sim.N>
<weap tag> [loaded=N] [reserve=N] [keep]`, `spawn <tag> pos=[x,y,z]
[yaw=DEG]`, `view [root...]`,
`expect <expectation>`, `until <ticks> <expectation>` (`sim.expect` every
5 ticks until it passes; a timeout fails with the last verdict),
`session`, `lobby ...` and `run`. In `h2 agent run`, a failed `sim
until` is a failed call, so it stops the script.

**Lobby steps** (`agent_proto::sim::menu`) press player one's pad
(`input.pad`) and check each step on `ui.screen`, with a timeout, on the
client, since a step spans many ticks:

- `lobby mode <campaign|xbox_live|split_screen|system_link>`: from the
  main menu, down to the row and A; expects the campaign screen or the
  players' screen in that mode.
- `lobby host live_coop`: from the players' screen in Xbox Live mode (or
  the Live menu): A, then down from the restored row to Co-op Campaign
  (row 4), A; expects the Live menu to go.
- `lobby invite [ROW]`: Y in a lobby, then A on friend row ROW.
- `lobby start`: A on Start Game (row 0); expects the screen to go.
- `lobby press <b[+b...]> [JSON expect]`: any presses, then wait for
  `{"screen", "main_menu", "mode", "not"}`.

Their screen ids and rows are the ones `h2 coop test --drive` uses,
moved to `agent_proto::sim::menu` and re-exported by `coop_drive`. Like
the driver, they assume a menu opens on row 0; a wrong row shows as the
wrong screen, and the step fails saying what it saw. `invite` and `start`
follow the driver and `boot::coop_live`'s docs; neither has been pressed
through by `sim` in a run.

### Determinism

`sim.session` reads the simulation mode (`coop_probe::read_sim`, docs/live/live-coop.md
1.1) and the squad session (`coop_probe::read_session`) through boot's
probe (`boot::agent_glue::session_view`):

| Mode | Kind | `sim.state.set` |
|---|---|---|
| 0 | `none` (menus) | allowed (no players to write) |
| 1 | `local` (campaign, split screen) | allowed |
| 2, 3 | `coop_host`, `coop_guest` (synchronous) | **refused**: a write on one machine desyncs the others and trips the sim hash |
| 4 | `mp_host` (distributed server world) | allowed: host-authoritative |
| 5 | `mp_client` (distributed client world) | refused: the host owns the state and overwrites it |
| unreadable | `unknown` | refused |

A refusal is `INVALID_REQUEST` with the session in `data`. In co-op, drive
players with pads instead (`sim.player.pad`, `input.pad`): pads are
inputs, which the pad schedule carries to every machine. Writes happen at
a tick boundary only (`UNAVAILABLE` between slices), and every value is
checked before the first write, so a refused set writes nothing. A
`h2 lockstep` comparison run is local (mode 1) and isn't detected: a
write there makes the runs differ by design.

### What `sim.state.set` writes

At the addresses Live telemetry reads (`live_telemetry::layout`, now
public, and `engine::view`): the player record's team (`+0xc0`); the
unit's team (`+0x138`), body and shield vitality (`+0xec`, `+0xf0`,
docs/halo1/health.md; added to `engine::view::objects::Body`), grenade
counts (`+0x23e`) and the held weapon's first magazine (`+0x22c` loaded,
`+0x22a` reserve). Not built: spawning a player between lives (that needs
the game's respawn, not a write).

### The game's own functions: teleport, give, spawn

Built 2026-11-19 (`crates/agent/src/sim/calls.rs`), tested over fake
guests with the callees played by the tests; a live check is pending.

**Why.** Until then `pos` wrote the body's position (`+0x64`) and zeroed
its velocity: the answer said `"wrote": ["pos"]`, and the biped's physics
put the body back at the next tick, so the player never moved. A move
has to go through the game, so its physics, cluster and vehicle seat
hear of it.

**How.** A game method answers in the tick observer, after the tick, where
nothing can call into the guest. So it queues a call and defers its
answer (`GameCtx::defer`, the executor's deferred answers: polled at each
tick boundary from the next, `TIMEOUT` after 30 ticks, held while
paused). The `agent-calls` feature (a `before` on `game_tick`, first,
`H2_FEATURES=-agent-calls` turns it off) finds the queue full at the start
of the next tick, makes the calls, then runs the tick itself, as our
skulls do. The answer comes after that tick. A snapshot is refused while
calls are queued.

- **Teleport** (`sim.state.set` with `pos`, `yaw`/`pitch` or `facing`) is
  HaloScript `object_teleport`'s body (`0x29ffb0`) for a player's unit,
  with our position in place of a cutscene flag's. The position goes to
  the co-op placement `PLAYER_PLACE_NEAR` (`0x14e970`; EBX the player,
  ESI the point, anchor -1, as `0x2a01ec` calls it), which takes the unit
  out of a vehicle (`0xe68c0` action `0x1e`) and places it through
  `0x14e200` a little above the point. The facing: the unit's desired
  facing, aiming and looking vectors (`+0x150`, `+0x15c`, `+0x180`, as
  `0x2a019e..0x2a01d9` writes them), a local player's view (`0x1874b0`:
  yaw and pitch into the player controls, `[0x4ed284] + 0x14 + 0x94 *
  user`, `+0x10`, `+0x14`), then `object_set_position` (`0xb75a0`) with no
  position for the orientation. A seated unit's facing is left (its
  seat's); the answer says so. Other fields of the same call are written
  at once. The answer: `wrote` (with `pos`, `facing`) and `teleport`
  (`pos` and `body_forward` where the game put it, `faced`, `unit`).
  `yaw` is degrees about +z from +x and `pitch` degrees up (-89 to 89),
  as `world.query` reports them; or `facing: [x,y,z]` (also taken as
  `forward`, `world.query`'s name for it), so a check can turn the
  player to face what a query found.
- **`sim.give`** gives a weapon as a starting profile does (`0xcce78`)
  and as an AI's equipment does (`0x1e05c8`): `placement_data_new`
  (`0xb7930`, owner the unit), `object_new` (`0xb7b40`), then
  `unit_add_weapon` (`0xcd0c0`, stdcall `(unit, weapon, mode)`), deleting
  the weapon (`0xb8540`) if the unit refuses it. Default mode 2: the
  unit's weapon slots are emptied first (`0xccff0`) and it readies the
  new one. `keep: true` is mode 1, the starting profile's: added, held
  only if the unit's own weapon choice (`0xcdeb0`) picks it. `ammo`
  writes the new weapon's first magazine once it is made. The tag is a
  datum or a name with `/` or `\` (names come from the map file, as
  `edit.tags` lists them: `h2 agent edit tags weap`). A bare name is
  looked for among `weap` tags only: a weapon shares its name with its
  model, render model, collision and animations (`hlmt mode coll jmad`),
  so on Halo 1 Blood Gulch `weapons\sniper rifle\sniper rifle` named five
  tags and was refused. `group:name` (`weap:weapons\sniper rifle\sniper
  rifle`) or `name.group` gives the group outright. The answer:
  `weapon` (`tag`, `group`, `name`), `object`, `mode`, `held`.
- **`sim.spawn`** makes an object as the netgame equipment spawn does
  (`0x15ab94..0x15abe6`): `placement_data_new` with no owner, the
  position at placement `+0x1c`, forward and up at `+0x28`/`+0x34`, then
  `object_new`. Any object group `object_new` knows (`bipd vehi weap eqip
  garb proj scen mach ctrl lifi ssce bloc crea`). A bare name is looked
  for among those groups, so `vehicles\warthog\warthog` finds the `vehi`
  and not its model's tags; if it names tags in more than one of them
  the answer asks for the group (`vehi:vehicles\warthog\warthog`). The
  answer: `object`, `pos` (where it was made), `tag`.

All three are host writes: refused in a co-op session or on a
multiplayer client, and only at a tick boundary, as `sim.state.set`.

```sh
h2 agent sim set player.0 'pos=[36.7,-80.8,1.7]' yaw=90 pitch=-10
h2 agent query '{"kind":["player"],"fields":["pos","forward","yaw","pitch"]}'
h2 agent edit tags weap
h2 agent sim give player.0 objects/weapons/rifle/battle_rifle/battle_rifle reserve=108
h2 agent sim spawn objects/vehicles/warthog/warthog 'pos=[40,-80,2]' yaw=180
```

### The split with docs/game/bots.md

What sim reuses from bed6c8dc's lane:

- **Local players** are bots.md stage 1's local slots:
  `bots::local::add(port, PadSource)` and `remove`. **Phantoms** are
  stage 6's: `bots::phantom::add(identity, team, PadSource)`.
- **The seam** is `agent::sim::bots::Bots` (`add_local`, `add_phantom`,
  `remove`, `phantom_player`), shaped like those calls, and
  `PadSource { Script(Remote), Recording { name, path }, Brain }`. A
  script source is an `input::backend::remote::Remote`, the step queue
  `input.pad` uses: sim fills it, the backend reads it each poll with
  `state_at_tick` (then `state_read`). So a sim script is just another
  pad source, as bots.md says the brain is.
- **In the game** `boot::sim_bots::LocalBots` implements `Bots` over the
  process-wide `bots::local::global()`, so a local sim player is a
  Clanker like an `H2_BOTS` one: on the lowest port from 1 that no bot
  asked for (or `port`), named "Clanker N", signed in by the bot glue on
  the Split Screen players screen or in a pregame lobby, never while a
  game runs (`bot_glue::to_sign_in`). A script source is read each rendered frame, so a
  step's `ticks` count bot frames there, not game ticks (`ms` is wall
  time). A recording is an input log's successful reads of its first
  pad, one per frame. The brain plays at `H2_BOTS_SKILL`. The bots
  feature has to be on (it is unless `H2_FEATURES` turns it off), or the
  bot is placed but never asked for a pad. Phantoms go to
  `bots::phantom::add`, which refuses until stage 6; the xuid is parsed
  as hex. `sim.player.remove` only takes off bots sim added. Tests use
  `FakeBots`; `NotLanded` refuses everything, for a game without bots.
- A local bot with another source can still be steered: `sim.player.pad`
  sends its steps to `input.pad` on the bot's port, which merges over the
  bot's pad (bots.md 4.1). A phantom's pad comes only from its source.
- A local bot's `team` is kept and reported (`team_pending`): the lobby
  or a `sim.state.set` decides it in game.
- **No local player mid-game.** `sim.player.add {kind: local}` while a
  game runs (`bots::guest::in_game`: a tick, and not the main menu's) is
  refused with `UNAVAILABLE` and the reason, where it used to answer
  success for a pad that never became a player. Two things stand in the
  way. The glue signs a bot in only on the players screen or in a pregame
  lobby. And the game makes a player for a newly signed-in local user
  only through a squad session's local-player pass (`0x65770` ->
  `0x5a880`, bots.md 4.2), which a game started by level boot (`h2 serve`)
  doesn't have: there is no session at all (level-boot.md), so
  `sim.session` says `local`. Phantoms need a networked game this console
  hosts (`bots::phantom::session::host_session`), so they don't join a
  serve's game either. On a serve, drive the existing player
  (`input.pad`, `sim.state.set`, `sim.give`). Whether retail's pass would
  seat a local user mid-game in a hosted System Link game (`0x58d90`
  passes session state 5, which is both the lobby and a game) is untested.

### Live

Sim never talks to a Live server and never makes accounts or sessions. A
phantom needs `identity: {xuid, gamertag}`, and the only place to get one
is the local embedded server (`h2 play --live-local`,
`live_server::start_embedded`), which issues bot identities and accepts
`bot: true` only for xuids it issued. Server-side bot accounts are
bed6c8dc's. Never point a sim run at live.forbes.red.

## Sim matches

Built 2026-11-19, tested over pure inputs; not run against the game.
Host methods while sim mode is on (`H2_SIM=<match file>`, `h2 sim run
<match> --socket PATH`; `boot::sim_glue::Sim::add_methods`,
[sim.md](sim.md)). Sim mode is a whole headless match with bots and no
rendering, ending in a report; these let a client watch it, read its
numbers so far and end it. (The `sim.*` methods above are a different
thing: simulated players added to an ordinary run.)

| Method | Access | What |
|---|---|---|
| `match.status` | read | `match` (the file), `map`, `game_type`, `bots`, `phase` (`waiting` until the match's game starts, `binding` until every distributed seat has a player and unit, `playing`, `over`), `bound_seats` (logical seat to player index for a distributed match), `ticks` (game ticks counted since the start), `max_ticks`, `game_seconds`, `wall_s` (since the boot), `match_s` (since the first tick), `speedup` (game over match wall time), `frames` (`skipped` world frames, `rendered_world` by `render_every`, `other` menu and loading frames, `stub_faults`), `end` (`game_over`, `tick_limit`, `wall_limit`, `stopped`; `null` while it plays) |
| `match.report` | read | the report as it would be written now (`h2-sim-report/1`, sim.md §6): players, kills so far, a note while the match has not ended (its `result.end` is then `aborted`) |
| `match.stop` | act | end the match: `end` becomes `stopped` (unless it had ended), the boot loop stops at the next slice and the report is written. `{stopping, end, already_over}` |

```sh
h2 sim run docs/runtime/sim-examples/lockout-2v2.toml --socket /tmp/sim.sock &
H2_AGENT=/tmp/sim.sock h2 agent match.status
H2_AGENT=/tmp/sim.sock h2 agent match.report
H2_AGENT=/tmp/sim.sock h2 agent match.stop
```

Stepping a match is `session.pause` and `session.step N` as in any run.
`h2 sim batch --fork` drives these and `state.save`/`state.load`: it
waits on `match.status` for `playing` before it saves or loads.

## `havok`: the physics world

Built 2026-11-18, tested over synthetic guest memory; not run against the
game. Layout and evidence: [havok-runtime.md](../engine/havok-runtime.md).
Game methods (`agent::havok::install`), answered at a tick boundary.

| Method | Access | What |
|---|---|---|
| `havok.bodies` | read | `{near?, radius?, limit?}`: rigid bodies: `entity` (address, the key contacts use), `state` (`active`, `inactive`, `fixed`), `object` (agent id) and `datum`, `pos`, `velocity`, `angular_velocity`, `inv_mass`, `mass`, `friction`, `restitution`, and `shape` where it reads: `addr`, `vtable`, `kind` (`capsule`: `a`, `b` in the body's frame, `radius`, `height`; `sphere`: `radius`; `transform`: `translation`, `child`; else `other`). `near` (an object id) keeps bodies within `radius` (default 5) of its body, nearest first, with `distance`; `limit` default 64 |
| `havok.agents` | read | `{object}` lists collision pairs and cached state. Details below. |
| `havok.contacts` | read | `{object?}`: contact constraints, each `a`, `b` (`entity`, `object`, `datum`, `fixed` for the level and static bodies) and `points`: `pos`, `normal` (from `b` toward `a`, **S**), `distance` (negative is penetration), `impulse` (what the solver applied on its last step), `friction`, `restitution`. With `object`, only its contacts, with it as `a` |

```sh
h2 agent havok.agents '{"object":"player.0"}'
h2 agent havok.contacts '{"object":"player.0"}'
h2 agent havok.bodies '{"near":"player.0","radius":3}'
```

For a superbounce, poll `havok.contacts {object: player.0}` once a tick
on one `h2 agent -` connection next to `object player.0`: the contact
with `b.fixed` is the level, and `impulse` over the body's `mass` is the
velocity it gave. Read the matching agent's child keys and contact IDs
to see which structure surfaces have a cached manifold at that tick.

For a direct biped capsule, `shape.height` distinguishes the standing and
crouching pills (0.725 and 0.5 for `masterchief_mp`). A type-`0x17`
wrapper reports `kind: "other"` in `havok.bodies`; its current pill is the
pointer at `shape+0x30`. See [Shapes](../engine/havok-runtime.md#shapes).

### `havok.agents`

`object` is required. The reply contains `object`, a tick stamp (`at`) and
`agents`. Each entry covers a pair with an allocated agent on any rigid
body of that object. A live object with no such pair returns an empty list.
A malformed selector returns `INVALID_PARAMS`. An absent object or a player
between lives returns `GONE`.

- `entry`, `island` and `collidables: [a,b]` are guest addresses in native
  pair order. `object_side` is `a` or `b`, and `entity` is the requested
  object's participating rigid body. `other` identifies the partner entity,
  with its `datum`, object ID and fixed-island flag where available. It is
  omitted for a partner without an entity. `pair_word` preserves the last
  four bytes of the pair record.
- `cache` contains `base`, `vtable`, `manager`, `kind` and `hex`. Addresses,
  keys and raw words are eight lowercase hex digits. `hex` is two digits per
  guest byte starting at `base`, including the allocation header and padding.
- A `bv_tree` or `welded_bv_tree` also has `tree`. Its `min` and `max` are the
  cached query box in the tree's frame. Each immediate child has `entry`,
  `key`, raw `word_04` and its own `cache`. A type-1 structure MOPP key also
  has `surface`, the numeric collision BSP surface index. A filtered child
  appears as `kind: "empty"` with its shared header.
- A `convex` or `swept_convex` cache has a `convex` block. `vertices_a` and
  `vertices_b` are active support vertex IDs in that agent's dispatch order.
  For convex against tree, child A is the convex body and child B is the
  selected tree shape. `contact_ids` are the child manifold's IDs in the
  shared manager. `separating_axis` and `separating_distance` describe the
  separation bound used to skip a closest-feature search while positive.
  Nonfinite decoded vectors or distances are null. `swept_sides` on the
  swept agent records A in bit 0 and B in bit 1.
- Unrecognised agents report `kind: "other"` and their inline bytes. Bounds
  include 32,768 scanned pairs, 512 children per tree, 4 KiB per allocation
  and 256 KiB of raw cache bytes per request. A failed memory read or exceeded
  bound returns `UNAVAILABLE`.

To compare ticks around a crouch, match a child by the partner and complete
`key`. Record both agent bases to see replacements and inspect the active
vertices, contact IDs and separation bound. Allocators can reuse an address
between samples. The raw dump also contains
pointers, unused slots and padding. A cached surface with an empty manifold
is still a candidate in the query box.

## `world.raw`: an object's bytes, before and after

Built 2026-11-19 to look for state a forced crouch leaves behind (the
superbounce, [superbounce.md](../game/superbounce.md)); tested over
synthetic guest memory, not run against the game. A game method
(`agent::raw::install`; reader `crates/live-telemetry/src/raw.rs`), read
only, no claim.

| Method | Access | What |
|---|---|---|
| `world.raw` | read | `{id, parts?, len?}`: `id`, `datum`, and `parts`, each `{part, base, hex}` or `{part, missing}` (why: an unset pointer, a fault) |

The parts, each found through the one before it
([havok-runtime.md](../engine/havok-runtime.md)):

| Part | Where | Bytes |
|---|---|---|
| `object` | the object's body (`player.N`: its unit) | the header's size (`+0x06`), else `len` (default `0x400`), at most `0x2000` |
| `physics_component` | body `+0xb4` into the component array | `0xa0` |
| `entity` | the component's first rigid body (`+0x70`, `+0x40`) | `0x200` |
| `motion` | entity `+0x3c` | `0x140` |
| `shape` | entity `+0x0c` | `0x80` |

`h2 agent raw-diff a.json b.json` (no game needed) prints, per part, the
32-bit words that differ, with their offset from the part's base, and
both as floats where they read as ones. A part on one side only, or
whose base moved (another structure), is listed, not compared. It reads
what `h2 agent raw` prints, or a `-` script's output (its last line).

```sh
h2 agent raw player.0 > before.json             # world.raw {"id":"player.0"}
# ... force a crouch, walk out, stand ...
h2 agent raw player.0 > after.json
h2 agent raw-diff before.json after.json
h2 agent raw player.0 entity,shape              # some parts
h2 agent world.raw '{"id":"player.0","parts":["object"],"len":2048}'
```

## `world.poke`: replace bytes in one object part

| Method | Access | What |
|---|---|---|
| `world.poke` | claim | `{id, part?, offset, hex}` |

Writes 1 to 64 bytes at a tick boundary. `id` and `part` use the same
names as `world.raw`, and `part` defaults to `object`. `offset` is a
nonnegative byte offset from the part's base. `hex` takes two hex digits
per byte. The entire range must fit in the part that `world.raw` reads
with its default lengths.

The calling connection must hold `session.claim`. `sim.session` reports
whether the current session allows host writes. A session refusal
returns `INVALID_REQUEST` with `data.session`, and a request served
between slices returns `UNAVAILABLE` when the session allows writes.

The result includes `id`, `datum`, `part`, `base`, `offset`, and `at`.
`base` is the first address of the part. `before` and `after` give the
bytes at the requested offset as lowercase hex.

The standalone short form takes a claim for the life of its connection.
Its offset can be decimal or prefixed with `0x`. In a script, send
`claim` on the same connection first.

```sh
h2 agent poke <id> <part> <offset> <hex>
```

## Tick traces and the flight recorder

Polling `world.object` from outside (`h2 agent -`) drops ticks when the
game or the client is busy. These take the rows inside the game, in the
tick observer, before queued requests are served, so each tick gives
exactly one row. Code: `crates/agent/src/trace.rs` (tests in
`trace/tests.rs`), wire types and `show` in `crates/agent-proto/src/trace.rs`,
CLI in `crates/cli/src/agent_flight.rs`.

```sh
h2 agent trace start player.0 physics,pos          # rows to traces/trace-<ms>.jsonl
h2 agent trace stop                                # rows, bytes, first/last tick
h2 agent flight start player.0 physics,pos,velocity 20
h2 agent flight dump crouch-bug.jsonl              # flights/crouch-bug.jsonl
h2 agent flight show ~/Library/Caches/h2recomp/flights/crouch-bug.jsonl \
    --from 4100 --to 4160 --field physics.crouch --field physics.airborne --field input
```

How it behaves (**Code**, proven with a fake reader and the input tap fed
synthetic packets; nothing here was run against the game):

- **Rows.** One JSON line per tick: `{tick, ticks, map, objects: {id:
  Object}, gone, error}`. `tick` is the stamp's game tick, `ticks` the
  agent's count (never resets, one more each row). `objects` holds each
  asked-for id in the world, trimmed to `fields` (`world.query`'s names;
  `id` and `kind` always kept; all when absent). Ids not in the world that
  tick are in `gone`; a failed world read gives `error` and still a row.
  The world is read once a tick when either recorder runs, and not at all
  otherwise. A paused game writes no rows; each `session.step` tick does.
- **`trace.start {objects, fields?, path?}`.** 1 to 64 ids. Streams rows
  to `path` (a bare file name in the cache directory's `traces/`, since
  the socket may be unauthenticated TCP; default `traces/trace-<ms>.jsonl`; an existing file is replaced) through
  one buffered writer, flushed every 30 rows, at `trace.stop` and at exit:
  memory doesn't grow with length. `trace.stop` and `trace.status` give
  `rows`, `bytes`, `first_tick`, `last_tick`, `path` and `error` (a write
  failure ends the trace there). One trace at a time; the last one's
  counts stay for `trace.status`.
- **`flight.start {objects, fields?, seconds?}`.** Keeps the last
  `seconds` (default 30, 1 to 600) × 30 rows in memory, at most 64 MiB of
  JSON (the oldest go first). Each row also has `input`: every
  `XInputGetState` result the game got since the last tick, taken at the
  agent's input tap after agent steps merge in (`{port, handle, status,
  packet, buttons, left, right, raw}`, `raw` the 18 pad bytes in hex), at
  most 64 a tick (`input_dropped` counts the rest, as during a load).
  Without XAPI input there is no tap and rows have no `input`.
  `flight.dump {path?}` writes the ring (default `flights/flight-<ms>.jsonl`)
  and keeps recording; `flight.stop` drops it; `flight.status` gives
  `capacity`, `rows`, `bytes`, `ticks` seen and the ring's tick range.
- **Scrub.** `h2 agent flight show <file>` reads a trace or flight file
  without a game: whole rows, or with `--field` one line per tick of each
  object's dotted paths (`physics.crouch`) and `input`'s reads, between
  `--from` and `--to` game ticks. It doesn't re-simulate; replaying a
  stretch needs machine snapshots (separate work).
- **Access.** Read-only (no claim), answered between slices too. Files
  are written where asked, as `input.record`'s are.

## Open decisions

- **Who runs the game the API talks to.** Decided (2026-11-17): one
  shared headless `h2 serve` that the user starts from an image they
  built. Agents connect to it, one controller at a time, and never build.
- **MCP versus CLI.** Decided (2026-11-17): the `h2 agent` CLI only. The
  MCP front end is dropped.
- **Snapshot depth.** Full machine snapshots (phase 5) are a large
  runtime change; the checkpoint fallback may be enough for a long time.

## Related

- [feature-engine.md](../engine/feature-engine.md): switches, hooks, Sim
  labels, native UI screens
- [high-framerate.md](../game/high-framerate.md): `lockstep` and `xapi::tap`
- [profiling.md](../perf/profiling.md), [gameplay-budget.md](../perf/gameplay-budget.md)
- [level-boot.md](level-boot.md), [checkpoints.md](checkpoints.md)
- [lab.md](lab.md): `h2 lab`, runs on a second Mac over SSH with replays and frame stats
- [hot-iteration.md](../graphics/hot-iteration.md): `edit.*` (live tag edits) and `render.*` (hot WGSL overrides, parameters, toggles)
- [debug-menu.md](debug-menu.md): the in-game debug menu, an in-process client of this API; `settings.list`/`settings.set`, `perf.overlay` and `cheat.list`/`cheat.set`
- [hs_vm.md](../engine/hs_vm.md)
- [docs/live](../live/): the Live service, the other reader of
  `live-telemetry`
