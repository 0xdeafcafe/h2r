# Agent API: driving the running game without pixels (design)

Status: phase 1 built ([below](#phase-1-what-was-built)): the socket,
`session.*`, `world.count`/`query`/`object`, `profile.counters`,
`features.list`, `h2 serve` and `h2 agent`. Raw and recorded input and
`profile.capture` built too ([below](#phase-3-and-4-what-was-built)).
Phase 5 ([plan and what was built](#phase-5-testing-in-the-lightest-way-plan)):
`render.frame`, `render.stats`, spike causes, `profile.fallbacks`,
`load.timeline`, `h2 testmap` and `h2 agent run`.
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
  render optional, and a speed control (`time_scale`, or "as fast as the
  sim allows"). It is the shared instance agents use; see
  [the decision](#open-decisions) on who starts it.

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
| `session.speed {time_scale \| "max"}` | headless only: run faster than real time |
| `session.wait {until: <predicate>, timeout_ticks}` | block until a predicate holds: `{"screen": "skulls"}`, `{"phase": "gameplay"}`, `{"tick": 50000}`, `{"hs_global": ["x", ">", 3]}` |
| `session.subscribe {events}` | stream events |

`wait` is the replacement for every "sleep and screenshot" loop: it is
evaluated at the tick boundary, so it never misses a one-tick state.

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
- `world.count {group_by: "kind" | "squad" | "bsp" | "zone"}` is the cheap
  first call. It answers "how much is alive" before anything is listed.
- `world.object {id}` gives everything about one object, including its tag
  path and the effects attached to it.
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
saved and loaded in well under a second.

| Method | Does |
|---|---|
| `state.save {name}` | guest RAM, the runtime's host-side state (kernel handles, timers, audio voices, render state that the guest can't rebuild), switch values, input cursor |
| `state.load {name}` | back to exactly that tick; `lockstep` hash checked against the saved one |
| `state.list`, `state.drop` | |
| `state.fork {name}` | load and give the copy a new name, for branching experiments |

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
- `state.give {player, weapon}`, `state.teleport {player, to}`.
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
doesn't wrap it. The menu bar app is out of scope.

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
  "live_menu_row", "age_ms"}`, docs/live/live-coop.md 12.6). The input
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
h2 agent frame draws               # ... and every draw
h2 agent frame snap 0,3            # ... and passes 0 and 3 read back as PNGs
h2 agent render                    # interp, pipelined render, native paths
```

Methods `render.frame` and `render.stats` (host methods: boot's agent glue
adds them; types in `crates/agent-proto/src/render.rs`, logic in
`crates/agent/src/render.rs`). How it behaves (**Code**, tested in
`crates/agent`, `crates/boot`'s `d3d_diag` and `render_probe`, and
`crates/feature/tests/render_loop.rs`; not run against the game):

- **Recording.** `render.frame {"arm": true, "draws", "max_draws",
  "snap"}` arms the recorder; the next present starts the frame and the
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
  `render_scene_passes`, `layer_sort`, `render_states`,
  `transparent_sort`); `counters` the frame-stats window's line. Each is
  `null` where that piece isn't running.

### Hitch causes and fallback hits

```sh
h2 agent capture 900               # spikes now say why, and "causes" counts them
h2 agent fallbacks 20              # where the fallback interpreter runs
```

- **Causes.** Each spike in `profile.capture` has `causes`, and the reply
  has `causes` counting every frame over budget (listed or not) by
  cause (`crates/agent/src/capture.rs`, `causes`):
  `shader_compile` and `pipeline_compile` (any on the critical path),
  `texture_uploads`, `file_io` and `gpu_readback` (more than twice the
  window's median, and at least one), `gpu`, `render_thread` and
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
  only), and per-frame I/O stalls are the `read_bytes` and `file_io`
  spike cause above.

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
`exists`, `missing`) and `keep` (put the last answer in the verdict).
The verdict is `{"pass", "failed", "ms", "calls": [{line, call, ms, ok,
error}], "expects": [{line, expect, want, pass, got}], "kept",
"stopped"}`. A failed expect is recorded and the script goes on; a failed
call stops it. Answers aren't printed except kept ones, so a long
scenario costs a few lines of context. Tested with a fake caller; not run
against the game.

Not built in phase 5 so far: per-subsystem guest time (engine-phase
spans), A/B of feature sets, features set live, kernel I/O seeks and
cache hits, `load.level`.

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
- [perf.md](../perf/perf.md), [frame-budget.md](../perf/frame-budget.md)
- [level-boot.md](level-boot.md), [checkpoints.md](checkpoints.md)
- [lab.md](lab.md): `h2 lab`, runs on a second Mac over SSH with replays and frame stats
- [hot-iteration.md](../graphics/hot-iteration.md): `edit.*` (live tag edits) and `render.*` (hot WGSL overrides, parameters, toggles)
- [debug-menu.md](debug-menu.md): the in-game debug menu, an in-process client of this API; `settings.list`/`settings.set`, `perf.overlay` and `cheat.list`/`cheat.set`
- [hs_vm.md](../engine/hs_vm.md)
- [docs/live](../live/): the Live service, the other reader of
  `live-telemetry`
