# Agent API: driving the running game without pixels (design)

Status: design. Nothing on this page is built unless it says so. Rust and
JSON blocks are sketches of an interface, not code that compiles today.

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
             │  MCP tools          │  `h2 agent …` CLI      │  JSONL scenario files
             └──────────┬──────────┴────────────┬───────────┘
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
- **Three front ends, one protocol.** An MCP server (`h2 agent mcp`) so
  agents get typed tools; a CLI (`h2 agent <method> [json]`) for shells and
  the user; and a scenario runner (`h2 agent run scenario.jsonl`) for
  repeatable experiments. All three are thin; the protocol is the contract.
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

1. **Raw**: `input.pad {player, at_tick, buttons, sticks, triggers,
   hold_ticks}`; a queue of timed packets.
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
| Protocol types, method table, JSON schema | new `agent-proto` | no runtime deps; the MCP server and CLI use it too |
| Server thread and tick-boundary executor | new `agent` | installed through the feature registry as feature `agent`, `sim: Host` |
| World reads | `live-telemetry::memory`, grown | shared with the Live service; one place for guest layouts |
| UI stack and HUD | `agent` + `tags::ui` | static trees from tags, live state from the screen stack |
| Profile | `perf`, `timeline` | new spans as hooks; owner attribution behind its own switch |
| Snapshots | `runtime`, `kernel`, `xapi` | serialisable host state; the biggest piece |
| HS eval | `hs` | compile one expression; inject via the guest evaluator |
| Overrides | `feature` | live-settable switches |
| CLI, MCP, scenario runner | `cli` (`h2 agent`, `h2 serve`) | |

## Phases

Each phase is useful on its own, and each is proven with tests over
synthetic guest memory and synthetic tags, as now.

1. **Socket and status.** `H2_AGENT`, `session.*` (status, pause, step,
   wait, subscribe), `world.count`/`world.query` over what `live-telemetry`
   reads, `profile.counters` from frame stats, switches settable live, the
   `fade` and `phase` stamp. CLI and MCP front ends. This alone fixes the
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

## Open decisions

- **Who runs the game the API talks to.** Today agents never build or run
  the game; the user does. The API keeps "never build" but needs a running
  image. Options: (a) only the user's own `h2 play --agent` session, so
  agents watch and drive what the user started; (b) one shared headless
  `h2 serve` that the user starts from an image they built, which agents
  queue on, one experiment at a time, never rebuilding; (c) agents may start
  `h2 serve` themselves from an existing image. (b) is the design's
  assumption.
- **MCP versus CLI first.** MCP gives agents typed tools; the CLI is
  easier to test and for the user to try. Both are thin; order only.
- **Snapshot depth.** Full machine snapshots (phase 5) are a large
  runtime change; the checkpoint fallback may be enough for a long time.

## Related

- [feature-engine.md](../engine/feature-engine.md): switches, hooks, Sim
  labels, native UI screens
- [high-framerate.md](../game/high-framerate.md): `lockstep` and `xapi::tap`
- [perf.md](../perf/perf.md), [frame-budget.md](../perf/frame-budget.md)
- [level-boot.md](level-boot.md), [checkpoints.md](checkpoints.md)
- [hs_vm.md](../engine/hs_vm.md)
- [docs/live](../live/): the Live service, the other reader of
  `live-telemetry`
