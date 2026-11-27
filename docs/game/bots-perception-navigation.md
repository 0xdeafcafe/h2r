# Bots: perception and navigation

What a bot sees and hears, and how it finds its way (sections 3.1 and 3.2). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 3.1 Perception

The brain reads game state the way telemetry already does
([live-gamestate.md](../live/live-gamestate.md)):

- players and their units: position, facing, aim (`+0x168`), team
  (`+0x138`), health and shield, current weapon (`+0x212`) and weapons
  (`+0x218[4]`);
- the weapons, grenades and powerups lying in the world;
- the objective: flag, bomb, ball, hill and territories, from the
  game-engine globals and the scenario's netgame flags (built, §3.7);
- nearby grenade projectiles, identified by the loaded map's grenade
  definitions and checked against view and collision;
- shots (the existing `0x104150` point) for a brief look toward the sound,
  a turn toward the shooter when hit with nobody in view (the HUD's
  damage arrow), and the shooter's tracker dot; and kills (`0x15ce70`)
  for emote triggers (§3.5);
- in Firefight, the AI's living actors from the actor pool, as enemies,
  less the actors on a player team: an ally squad's (§3.10).

**Line of sight** is a ray against the map's BSP collision, the same data
the nav mesh comes from. A bot sees an enemy only when all three hold:

- a ray reaches the enemy's head or chest;
- that part is inside the bot's view cone and its visual range;
- a reaction delay has passed (§3.4).

A head over a parapet is seen. The bot aims at the part it sees: the
chest, or the head when only the head is clear (`mind::sight_point`).

### Visual range

A bot sees what a person sees on the screen (`perceive::notice_range`),
applied to players and Firefight actors alike before the mind gets
them:

- **The view.** The cone is 40 degrees off the aim
  (`mind::NOTICE_CONE`), the corner of Halo 2's 70-degree, 4:3 frame.
  Past it only a player within 3 units (`mind::NOTICE_NEAR`) is noticed.
- **Distance.** In the centre of the view (15 degrees, `perceive::CENTRE`)
  a player is noticed out to the skill's range; toward the cone's edge
  the range falls to 0.4 of it (`EDGE_SIGHT`). A crouched player is
  noticed at 0.75 of the range (`CROUCHED_SIGHT`).

  | | Easy | Normal | Heroic | Legendary |
  |---|---|---|---|---|
  | sight range (world units, `perceive::sight_range`) | 25 | 35 | 45 | 55 |
  | camouflage, still (`perceive::camo_range`) | 1.5 | 2 | 2.5 | 3 |
  | camouflage tracking error (times the skill's, `look::camo_aim`) | 2.5 | 2.2 | 1.9 | 1.6 |

- **A scope.** A held gun with zoom levels reaches 1.5 times as far per
  level (`SCOPE_SIGHT`), only within 6 degrees of the aim
  (`SCOPE_CONE`): a sniper sees a far player down its own line once it
  has looked there, never off to the side, and never through a wall.
- **Active camouflage.** A player with camouflage on (player `+0x198`,
  §3.6) is noticed only at arm's length, twice that while it moves
  faster than the motion tracker's threshold (`CAMO_MOVING`). Firing
  (within `sound::HEARD_TICKS`) or a broken shield shows it at the full
  range, as the camouflage flickers in Halo 2. A tracked camouflaged
  target is aimed at with the larger error in the table and engaged at
  0.6 of the thumb's pace (`look::CAMO_ENGAGE`). The motion tracker
  still shows it moving (§3.1, "The motion tracker").

The host reads each living player's motion and recent fire as slot masks
(`Observations::moving`, `firing`); perception keeps only the bits of
players it sees.

### Visibility grid (`bots::vis`)

Most sight rays on a busy host are cast between places that can never see
each other: two rooms with a floor or a wall between them. The visibility
grid answers those without a ray. It is built once per map, offline, with
the nav mesh, and shared read-only by every bot in the process.

- **Cells.** The space a player's feet, body or eye can be in: from 0.3
  below to 0.9 above every walkable polygon of the nav mesh, cut into
  boxes 1 x 1 x 0.5 world units (about 3 x 3 x 1.5 m). A big map whose
  cells would pass `vis::MAX_CELLS` (2048) gets boxes 1.25 times bigger
  until it fits.
- **Pairs.** One bit for each pair of cells, packed as the upper triangle
  of the pair matrix. Set means **never**: no segment from any point of
  one box to any point of the other is clear. Unset means maybe.
- **Never is proved, not sampled.** For a pair, the builder looks for one
  collision triangle that every segment between the two boxes must pass
  through: the boxes on opposite sides of its plane, and the plane
  crossings of all 64 corner-to-corner segments well inside the triangle
  (the crossings of every other segment lie in their convex hull). If no
  single triangle does, it cuts the bigger box in half and proves each
  half, up to 24 pieces and 8 cuts deep. A clear ray found on the way
  ends the pair as maybe. Margins (1 cm from each edge, away from the
  ends of the segment, not grazing) keep the proof true for the `f32` ray
  test, and two triangles are never combined, because that test can let a
  ray slip along a shared edge.
- **At run time** `los::Walls::clear` looks the two ends up in the grid
  first. Both in cells, different cells, bit set: blocked, no walk. Any
  other case walks the collision as before. So every caller of `clear`
  (perception, aim, decisions, incoming grenades, nav probes) gets the
  same answer with fewer rays, and a match plays the same with the grid
  or without it. `Walls::hit` (a grenade's flight) does not use it.
  On a light host the dense voxel lookup has a 256 KiB budget. Larger
  grids find the same cell by binary search in the file's sorted list
  of occupied voxels.
- **File.** `<data>/bots/<map>.<bsp>.vis` beside the `.bots` file: magic
  `H2VS`, `vis::VERSION`, the `.bots` file's stamp, a hash of its walls,
  then the cells and the bits. The nav worker writes a map's `.bots`
  files first, then builds each BSP's grid from them
  (`vis::rebuild_resumable`; the game's worker on `background::workers`
  lowered threads, `h2 bots nav` on half the machine's threads, at most
  8; the grid is the same either way,
  `the_grid_is_the_same_on_any_threads_and_at_background_priority`) and
  writes each `.vis` as it finishes, BSP 0 last. A map whose `.bots` is
  fresh but whose `.vis` is missing, older, damaged or from another build
  gets only its grid built
  (`bots: built the visibility grid for lockout (none yet): ...`); a BSP
  whose `.vis` is current (`vis::current`: this version, the `.bots`
  stamp, its walls) is not built again.
- **Build.** A row reuses the ordered collision candidates for centre
  rays with the same XY projection and the intersection terms that
  depend only on the ray's origin (`los::BuildRays`). The proof
  precomputes each triangle's plane and edges. It factors the 64
  corner-crossing checks into two groups of eight projections per edge
  and uses the direct projection at rounding boundaries. Candidate
  order, proof limits and serialized grid bytes are identical to the
  scalar proof (`visibility_build_matches_legacy_grid_bytes`).
- **Progress.** A grid is built in about 64 chunks of rows of pairs
  (`vis::fill`). The workers take rows from the current chunk and
  reuse their scratch space for the whole grid. After a chunk the
  build saves the grid so far and the
  first row not done to `<map>.<bsp>.vis.part` (magic `H2VP`) at most
  every `vis::SAVE_EVERY` (10 s), and the next build of that BSP carries
  on from it when it was made for the same `.bots` stamp and the same
  cells (`vis::read_part`); the part goes when the `.vis` is written.
  The game's worker also stops between chunks when a map is asked for
  (`Handle::want`) or it is stopped, saves, builds the map asked for, and
  carries on with the grid once its queue is empty
  (`bots: visibility grid for lockout: paused after 12.0 s, progress saved`).
  `autonav::RunningWorker` requests a stop when its host exits. If the
  process exits before the next progress save, a quit loses at most
  `SAVE_EVERY` of grid work. Tests:
  `a_stopped_grid_build_keeps_its_progress_and_carries_on_to_the_same_grid`
  (`vis/tests.rs`), `a_built_grid_is_saved_where_the_game_loads_it_and_used`
  (`autonav/tests.rs`: written to `cache::dir`, where the glue loads).
  Bump `vis::VERSION` when the cells, the proof or the margins change.
- **Loading.** The glue reads the `.vis` with the `.bots` and attaches it
  only when its stamp and walls hash match. Otherwise it logs why and
  every sight ray is cast:
  `bots: lockout: visibility grid not used (lockout.0.vis: missing); casting every sight ray`.
  A used grid logs its size:
  `bots: lockout: visibility grid: 812 cells of 1.00 x 0.50, 41.2% of pairs never visible`.
  A grid built while its map runs goes into the loaded map's data in
  place (`autonav::offer_vis`, the walls hold it in a `OnceLock`): the
  `Arc<MapData>` the bots hold is not replaced, so they keep their routes
  and memory (`bots: lockout: the new visibility grid is in use`).
- **Tests** (`vis/tests.rs`): pair numbering, the file round trip and
  damage, a wall between cells giving never and open space giving maybe,
  a wall of several triangles, random and cell-boundary point pairs
  checked against the walk (never must agree), perception asking the
  grid first and walking without one, the named log lines for a missing,
  damaged, older, other-build or other-walls file, and a grid built for a
  fresh `.bots` that had none.

The host gives each bot its own view, teammate HUD markers, audible shots,
its motion tracker, radio calls and public match information.

Each brain keeps a short, tick-stamped history of world samples, vehicle
sight, nearby grenades, sound, deaths and teammate callouts. It thinks
from the newest sample at least `perception_ticks` old. A repeated poll
in the same tick reuses the previous pad. If the host skips a sample, the
brain can use an older observation. The first live sample after a respawn starts a
fresh history.

### The motion tracker

A phantom has no local-user HUD slot, so `bots::guest::radar` builds the
dots its own tracker would show. It uses the active variant's motion
sensor flag (setting 20, or setting 73 on the Juggernaut), the map's
`hudg +0x290` horizontal range and the player speed threshold read by
`0x1a1d55`. A hostile player shows when it moves faster than that
threshold or fired in the last `sound::HEARD_TICKS` (6): the shot sound
names the shooter's slot (`ShotSound::shooter`, from the players table's
unit). A player who walks crouched and holds fire does not show, nor do
allies. A mounted player's movement comes from its vehicle.

The brain gets up to `radar::DOTS` (4) dots, nearest first, each a
horizontal point rounded to half a world unit at the bot's own height.
A dot has no identity and no height. It never picks a target or aims a
shot: only sight does that.

The bot reads the tracker at a glance. Between fights a glance is due
every reaction time, in a fight every two. Each glance is taken with the
Clanker's radar-use trait (without a profile, `0.3 + 0.65 ×` the skill's
awareness). `mind::tracker` remembers each dot a taken glance showed for
`KEEP_TICKS` (45) after the last glance that showed it, so a missed
glance does not wipe them; a dot within `SAME` (1.5) of a remembered one
is that dot, moved. What the dots do:

- **On guard.** A remembered dot inside `NEAR` (half) of the disc makes
  the next sighting an alert one (§3.4), and the bot is not calm: no
  breather reload, no calm detours.
- **Where to go.** Between fights, after helping a teammate and before
  supplies, vantage points and roaming, the bot goes to the nearest
  dot (`Doing::Support`).
- **Where to look.** A new dot turns the head for `GLANCE_TICKS` (12 to
  20) while travelling elsewhere. On a route toward it, it looks into the
  coming corner or at the dot. Standing, it looks at the dot.
- **Behind it in a fight.** A dot read within `BEHIND` (3) units, more
  than a right angle off its aim, with no enemy in view near it, turns
  the bot to check it for `CHECK_TICKS` (8 to 12), trigger released.
- **Another floor.** At a dot with nothing in view, the player is above
  or below: the dot moves to the nav mesh's nearest floor 1 to 8 units
  over or under it, once, then it is dropped.

Tests: `a_firing_stander_shows_and_a_crouch_walker_does_not`
(`guest/tests.rs`), `a_shot_names_the_slot_whose_unit_fired_and_shows_it_as_firing`
(`sound/tests.rs`), `mind/tracker/tests.rs`, and in `mind/tests.rs`
`a_missed_glance_keeps_the_dot`, `a_dot_behind_stops_the_reload_and_turns_it`,
`a_dot_makes_the_next_sighting_alert`, `nothing_at_the_dot_checks_the_floor_above`
and `a_glance_at_the_tracker_ends`.

## 3.2 Navigation

Every map gets a nav mesh, built offline from the BSP collision by our own
tool and cached by map fingerprint. `bots::autonav::build_map` reads the
scenario and builds one `<map>.<bsp>.bots` per BSP. `bots::cache::build`
starts with the sectors from `pathfind::generate::generate`. The bot
generator adds its links and collision probes to those sectors. Generator
version 7 regenerates earlier caches; it includes tactical places, the
measured jump's reach, the breakable glass, the crossing check and the
in-map flags (below).

The mesh holds:

- walkable surfaces: collision planes with an upward-facing normal, joined
  across shared edges, with step-up and headroom checks for the biped's size.
  A surface takes links when a standing body fits on it somewhere
  (`nav::stands_on`): at its centre, or at a corner pulled 0.3 in. A floor
  that runs in under a low lip, as a crouch spot's does, keeps its links
  by the standing room beside the lip; a recess wholly under one has none.
  No walk, step or drop link crosses a wall standing on the polygon side
  (`nav::crossing_clear`): rays across the side's middle at 0.25 and 0.5
  over the higher floor, 0.15 to each side, must be clear. A wall planted
  on a side (an invisible boundary wall over continuous terrain, a
  building's face at a floor's edge) lies in the plane of the vertical
  headroom ray, which does not see it;
- links:
  - **drop**: a ledge to a lower surface within the fall-damage limit;
  - **jump**: a gap or ledge reachable at the biped's jump arc, checked by
    sweeping the arc against collision. A standing jump climbs ledges up
    to 0.6 (`nav::Build::max_jump_up`). A jump leaves at 3.08 units/s and
    falls under 4.15 units/s², so it tops out 1.14 over its takeoff
    (`nav::JUMP_APEX`); 0.6 leaves room for the lip and the takeoff, and is
    a **[guess]** until a game run climbs taller ledges. The swept arc tops
    out 0.2 over the ledge (`nav::ledge_lift`). Up to 0.35 that is the
    flat arc (rise 0.36) the generator always used;
  - **crouch-jump**: a separate takeoff and landing for ledges reachable
    with the smaller capsule, up to 0.6 + 0.225. The follower presses A,
    detects the rise, then holds crouch two simulation ticks after takeoff
    until it reaches the landing. A route walk takes no crouch jump, so a
    crouch spot on a ledge only a crouch jump reaches has no walk to it.
  - **teleporter**: directed links from each sender to all of its
    channel's receivers on this BSP. The bot walks into the trigger,
    keeps its approach direction briefly and resumes at the receiver.
    An unexpected receiver or a timeout causes a replan.
  - **lift**: directed links from a placed gravity lift, launcher or
    elevator to a BSP landing. The bot steps on, lets the lift pick it
    up, then steers toward the landing. On a moving platform it waits
    until the platform is level with the exit before stepping off.
  - **vehicle**: the ground-vehicle planner filters the walk graph by
    chassis clearance and checks each steering span (§3.9).

**In the map.** Each polygon is in the map or out of it
(`nav::NavMesh::mark_out_of_map`, run by `autonav::build_map` after the
scenario's rides are linked). A polygon is in when a player start or a
weapon pad reaches it over the mesh's links: the walks, steps, drops,
jumps, crouch jumps, teleporters, lifts and elevators a player takes
without a glitch. A superbounce is never a mesh link, so the terrain past
a map's boundary walls and cliffs (Coagulation's snowfields) is out.
Reachability is directed: a plateau that drops back into the map is out,
a pit one drops into and cannot leave is in. A BSP no start or pad is on
gets no flags. Every polygon is in until a classification marks some
out. `autonav::mark_in_map` runs the same classification on one BSP's
mesh for another caller: the Firefight kit keeps only in-map sectors
([firefight.md](firefight.md), "Sectors the AI can use").

Out-of-map polygons are kept, flagged, because a bot can still end up
there: a superbounce route (§3.8, "Superbounce routes") lands it there
and walks the mesh from its landing, and a bot a bounce launched out has
to find its way back in. The rules:

- a search that starts in the map never ends in or passes through an
  out-of-map polygon (`nav::search`, `NavMesh::in_map`): a goal out there
  has no path, found at once without searching the whole map;
- a search that starts out of the map goes anywhere, so the bot plans
  back in over the drops at the boundary;
- goal picking leaves them out: the roam's random floors
  (`NavMesh::nth_in_map`) and the places' lanes, traffic and vantages
  are in-map polygons only.

Paths come from A\* over the sector graph, smoothed with a funnel pass and
followed by steering:

- a look-ahead point;
- a corner slowdown;
- "stuck" detection and recovery (below).

A move becomes a left-stick vector relative to the bot's facing. A bot
never warps. The agent API's [routes](../runtime/agent-api.md#routes) use
the same bot path search, follower, pad steering and jump arc to drive a
local player.

**Traversal evidence and estimates.**

| Input | Evidence |
|---|---|
| Teleporter flag position, type and channel | `scnr+0x118`, stride `0x20`, position `+0`, types `7` and `8` at `+0x10`, identifier `+0x14`. See `map::scenario_layout`, `h1convert/src/apply.rs` and the retail checks in `h1convert/tests/h2_scenario.rs`. |
| Scenery, machine and crate placement | `scnr+0x50`/ `+0x58` (scenery block / palette, strides `0x5c` / `0x28`) and `+0xa8` / `+0xb0` (machine, `0x48` / `0x28`). Palette index `+0`, flags `+4` (bit 0 suppresses automatic placement), position `+8`, yaw `+0x14`, palette datum `+4`. See the object-type table `0x00468630`, palette access `0x000a743b` and `map::scenario_layout`. Crates use the placement at `+0x328` (stride `0x4c`) and palette at `+0x330`. |
| Lift object identity and travel | The retail machine palette places `power_boost` on Ascension, `grav_colossus` four times on Colossus and `elevator_cyclotron` on Cyclotron. Name patterns for modded placements, the launch direction, travel envelope and exit timing are **[guess]**. The generator scans for a landing that fits a standing biped and checks the path against the BSP. The reverse axis used for Ascension projects onto raised playable BSP geometry. |
| Jump and standing clearance | Retail multiplayer Chief has standing height 0.725 and crouch height 0.5. See [superbounce.md](superbounce.md) and its traces of `0x000df66e`, `0x000dd8e0` and `0x000e2a06`. The sampled arc, shoulder offsets, and takeoff timing are **[guess]**. |

The generator checks headroom at walk-sector centres and portals. The
steering layer checks the standing capsule ahead on the floor, including
within a single polygon and when strafing. A jump samples the swept arc
at the centre and both shoulders for roof and body clearance. Teleporter
cost includes walking to the sender and walking from the receiver. Lift
cost includes boarding and travel time plus a 25% allowance for exposure during the ride. On
graphs with these links the search uses a zero distance heuristic so a
shortcut remains admissible even when its entrance is away from the goal.
At six ticks per walking unit, the teleporter transit allowance is six
ticks. A lift budgets eight ticks to board plus 2.7 ticks per unit of
travel. A platform elevator budgets 24 ticks to board plus 4.8 ticks per
unit. The exposure allowance applies to travel. All rates are estimates.

### Traversal audit by map

Counts are **directed edges**. The source scan used the retail Xbox disc,
the nine installed Multiplayer Map Pack scenarios and thirteen installed
H1 conversions. It read scenario flags and automatic scenery, machine and
crate placements. An independent offline probe checked endpoints against
fingerprint-matched cached BSP triangles and walk polygons. The counts
below are the expected output of the new Rust generator. The ignored
`bots/tests/map_corpus.rs` test rebuilds all 34 from source and checks
those expectations. A landing that clears the BSP still needs a ride in
the game to establish which deck the engine actually reaches.

| Retail H2 scenario | Teleporter | Lift | Placement or remaining gap |
|---|---:|---:|---|
| ascension | 1 | 1 | One-way channel and angled `power_boost`. |
| beavercreek | 2 | 0 | Reciprocal channels. |
| burial_mounds | 0 | 0 | No matching placement. |
| coagulation | 2 | 0 | Two outbound base channels. |
| colossus | 0 | 5 | Four `grav_colossus` machines and the high power-lift crate. |
| cyclotron (Ivory Tower) | 0 | 4 | Central elevator and three HVAC lifts. |
| foundation | 0 | 4 | Four short HVAC lifts. |
| headlong | 1 | 2 | One-way channel and two steam lifts. |
| lockout | 0 | 1 | Tube with an overhanging exit. Check the landing deck. |
| midship | 0 | 5 | Two Covenant power lifts and three midpillar placements. Check which bodies the midpillars actually move. |
| waterworks | 2 | 0 | Two outbound channels. The crusher is a dynamic hazard. |
| zanzibar | 0 | 0 | The wheel and gates move. Their changing surfaces have no timed edges. |

| Map Pack scenario | Teleporter | Lift | Placement or remaining gap |
|---|---:|---:|---|
| containment | 2 | 0 | Both receivers are about 3.0 units above their landing. Gate state is dynamic. |
| warlock | 4 | 4 | Reciprocal channels and four lift crates. Check the exit height on all four. |
| deltatap (Sanctuary) | 0 | 0 | No matching placement. |
| turf | 0 | 0 | No matching placement. |
| backwash | 0 | 4 | Four central lift machines. |
| dune (Relic) | 2 | 0 | Reciprocal channels. The static graph can suggest the switch-controlled portal while it is disabled. A failed ride triggers recovery. |
| elongation | 0 | 0 | Conveyor surfaces have no timed ride edges. |
| gemini | 4 | 0 | No clear ascent found for the two named gravity columns. The effect and upper crate provide no verified player ride. |
| triplicate (Terminal) | 0 | 0 | Train and crossing gates are dynamic hazards. |

The later Blastacular maps Tombstone (`highplains`) and Desolation
(`derelict`) were absent from the available assets. Their link counts are
unverified. See [Title updates and maps](../runtime/title-updates.md).

| Converted H1 scenario | Teleporter | Lift |
|---|---:|---:|
| h1_beavercreek | 2 | 0 |
| h1_bloodgulch | 2 | 0 |
| h1_boardingaction | 8 | 0 |
| h1_carousel | 4 | 0 |
| h1_chillout | 3 | 0 |
| h1_damnation | 2 | 0 |
| h1_hangemhigh | 0 | 0 |
| h1_longest | 0 | 0 |
| h1_prisoner | 0 | 0 |
| h1_putput (Chiron TL-34) | 30 | 0 |
| h1_ratrace | 4 | 0 |
| h1_sidewinder | 4 | 0 |
| h1_wizard | 4 | 0 |

The converter writes type 7 senders and type 8 receivers. Matching their
channel preserves Blood Gulch's two one-way base exits and Sidewinder's
mixed layout. On Sidewinder, channels 2 and 3 have opposite endpoints.
The scan found no automatically placed machines or lift-family objects
on the thirteen conversions. Sidewinder has blast-door collision scenery.
Whether any converted scenery moves under a script requires a game run.

Ledges on Lockout (0.40 units), H2 Beaver Creek (0.36), H1 Blood Gulch
(0.57), H1 Sidewinder (0.40) and Zanzibar (0.40, the floor of the second
proven crouch spot) are within the standing jump's 0.6, so they are jump
links. The crouch-jump follower takes ledges from 0.6 to 0.825. Check
takeoff, ceiling clearance and landing with a player-sized biped. There
is no separate ladder action.

### Belief and map knowledge

**What can reach a decision.** The phantom pad source passes host
candidates through `perceive::observe` before combat, movement, radio or
memory. The paired-world harness in `perceive::honesty` runs seeded brains
with separate radios, comparing projected inputs, controller output,
chosen activity, kit and outgoing enemy call. One fixture puts a hidden
enemy and power weapon behind a base wall on an H1-shaped valley. A
second pair starts at the production guest reader, changing a hidden
biped's state in the synthetic object table before either brain runs.

| Host input audited | Information the bot receives |
|---|---|
| Living unit table, aim and weapon slots | Enemies must pass view cone and BSP sight. Teammate positions act as HUD markers. Sight reveals aim, the held weapon and a broken shield. Partial shield strength and opponent ammunition stay private. Body avoidance, weapon seeking and group decisions use the filtered list. |
| Motion tracker | An unzoomed player reads up to four hostile dots inside the sensor disc: players moving fast enough or firing. Each is an approximate horizontal position through walls, with no identity or height. The bot needs sight to identify a player or fire. |
| Ground objects and placement records | Sight reveals loose grenade equipment and a weapon’s model. Its remaining ammo becomes known when held. Occupied power sites require sight. A successful scan establishes emptiness only at a known pad the bot can inspect. A failed scan supplies no observation. |
| Vehicles and seats | A visible hull or the bot's own ride supplies the model, pose and seat layout. Identifying a rider on another vehicle also requires sight of that player. Remote vehicle health uses a neutral estimate. The bot can read its own ride's health. |
| Objective globals | Game mode, objective geometry, bases and the active hill are public. A loose objective or enemy carrier requires sight. Friendly carriers can be followed by their teammate marker. Out of sight, a CTF or Assault bot plans from its own memory and from the public spawns (section 3.7, "Belief"). |
| Shots, projectiles and death log | Audible shots give a direction for a look or search, and when hit with nobody in view, the shooter's bearing. A grenade dodge requires a close, visible projectile. The kill feed supplies identities. A fresh visible body or the bot's own death can supply a position. |
| Scoreboard, radio and map tags | Scores and team membership are public. Radio distributes dated observations on completed ticks. Static placement periods and player starts describe the map. No variant countdown or actual enemy spawn selection feeds the planner. |

The tracker's dots and what the bot does with them are in "The motion
tracker" (§3.1).

A visible power item has probability 1 and a visibly empty pad has
probability 0 for this tick. Away from the pad, an empty observation
gradually loses weight. Estimate the return using a witnessed period,
then a placement override, then the item collection default, with a
60-second prior if all are unknown. Only a continuously watched pickup
and return of a new object can teach a period. Last seeing the item
elsewhere in someone's hands or hearing a teammate call the pad empty
also changes the estimate. A carried model is attributable only when it
matches one known pad. A recognizable object in either hand is specific
to its placement. Indirect reports always retain uncertainty and retain
the original observation tick through perception and radio delays.
Distance, recent danger, the strength of the nearby position and past
visits all affect whether a trip is worthwhile.

`h1convert` writes the H1 spawn time to the H2 scenario equipment record
at `+0x0e` in seconds. The converted Blood Gulch has explicit 120-second
sniper and 90-second rocket placements. Every inspected Sidewinder
placement has zero there. Its rocket collection supplies 120 seconds,
and its sniper collection also has zero. Before inspection the cache gives
the bot its 60-second prior. Once it inspects the placement, the traced
engine default of 30 seconds and the public variant setting refine that estimate.
These values describe the bot's expectation.

The cache stores common lanes, sightlines, vantage positions, player
starts and power pads alongside the mesh and BSP walls. Up to sixteen
sample origins estimate which sectors lie on many shortest paths. A
vantage scores sightlines to those lanes, elevation over them, nearby
solid cover and distinct exits. Spatial sampling and successive spacing
passes distribute candidates across the map. A replay of the scorer on
older fingerprint-matched geometry selected 32 positions each on Blood
Gulch and Sidewinder, spanning about 115 × 134 and 99 × 102 world units
in plan respectively. The corpus test checks coverage after regeneration.

A calm sniper or precision gunner can seek a position and face the lane
it covers. A bot ahead on the scoreboard can hold a nearby vantage.
Patience extends the wait at a projected return, and a favourite weapon can
tip the choice between two nearby pads. A patient long-range gunner or one
carrying its favourite rifle gives a stronger sightline more weight.
Skill and tuned awareness affect how many places and pad locations it
recognizes, how far it will go, and how accurately it estimates arrival.
A once-visible enemy can be chased at its last observed position. When an
idle bot hears a gunshot, it can turn toward the coarse bearing after a
5–8 tick reaction delay, then walk a short distance in that direction.
The sound gives it neither a combat target nor a reason to fire. Hit
with nobody in view, the bot faces the newest shot that is not its own at
once, for `HIT_LOOK_TICKS` (45), as a player follows the HUD's damage
arrow, and its next sighting in that time is an alert one (§3.4). A kill
announcement retires the old sighting and may prompt a look toward a
plausible out-of-view map start. Search confidence expires on the existing
enemy-memory clock.

Per-bot experience records verified supply visits and confirmed kills
from a recent firing position in coarse map regions. A versioned serde
snapshot can be restored to another brain. A future Live profile adapter
can store it under the persistent ranked bot's account and map revision
on the local server and worker. Within this lane the brain owns it in
memory. It survives match resets for that brain's lifetime.

**Getting unstuck** (`crates/live/bots/src/stuck.rs`, built). A pure state
machine, run every tick the bot is walking somewhere (chase, roam, going
for a weapon; not while it strafes in a fight). It measures progress the
way the bot wants to go: the position's gain along the wanted direction
since the start of a window. A window of 20 ticks (2/3 s) with less than
0.25 world units of gain escalates one step:

| Step | What the bot does for its window |
|---|---|
| sidestep | backs off and strafes to one side while facing the intended route. If the step is blocked, it tries the other side, then resumes the route. |
| jump | walks on and presses jump (A) for 3 ticks |
| crouch-jump | jumps, then holds crouch from the third tick (the jump's higher clearance) |
| repath | the waypoint it was walking to is blacklisted; the path and (for roaming) the goal are dropped; it plans again |

Gaining 0.25 at any step is recovery: back to walking. Stuck again within
3 s carries on from the step that freed it, not from the sidestep. Two
slower checks catch what the window does not:

- **penned in**: never more than 1 world unit from one point for 4 s while
  trying to move: oscillating between two waypoints, circling, sliding to
  and fro along a wall;
- **no closer**: the goal not 0.5 closer for 10 s.

Either goes straight to repath, blacklisting the goal and the waypoint.

A blacklisted point is avoided for 10 s within 0.75 world units, 16 at
most: new paths skip the nav mesh edges whose portal passes near one
(`NavMesh::path_avoiding`; when no such path exists, the plain one), roam
goals skip polygons whose centre is one, and weapon pickups at one are not
sought. A pickup it holds the action button on for 1 s without getting it
(on a ledge, in a wall) is blacklisted the same way.

**Bodies.** Someone within 0.8 world units ahead of the walk direction and
within 0.35 of its line (not on another floor) turns the walk away from
them, keeping its speed. In a fight the target itself is not avoided.
`moves::around_bodies_safe` keeps the original direction if the turn
fails its footing or clearance check.

The follower's own stall check (`nav::Follower`, a jump after 30 frames
without getting closer to the waypoint, then a fresh path) runs alongside
the state machine. Both limit recovery hops on walking legs to directions
with footing out to 1.6 world units.

**Corners and doorways** (`nav::NavMesh::portal`, built). A path never
aims at a wall corner itself: each portal end that is a corner of the
mesh (a vertex on a side nobody walks across: a wall or a ledge) is
pulled 0.3 world units in (`nav::CLEARANCE`), so a bot turns a body's
width off the corner instead of catching its shoulder on it. A link
the builder made between surfaces that do not share an edge (a doorway
off a long wall) spans the near polygon's whole side; the portal is cut
to the part the far polygon lies along, so the path goes through the
door and not through the wall beside it. Nearing a corner (0.8) the
follower bends toward the next leg when both legs are walks and the bend
has footing. `mind::Mind::go` checks walking and cornering directions up
to the current waypoint. A step across a planned drop or jump follows
the route directly. Traversal links with separate entry and landing
points control the approach and crossing themselves.

**Off the mesh** (`nav::NavMesh::nearest`, built). A start or goal no
polygon is under (a bot on a crate or knocked off an edge, someone seen
mid-jump) snaps to the nearest polygon within 3 world units whose floor
is at most 0.6 over it or 3 under it: the bot walks (or jumps up) onto
the mesh first, and a chase to someone seen in the air goes to the floor
under them instead of giving up. A bot whose next waypoint's floor is
more than 1.2 (plus its distance) over it has fallen below its path and
plans again at once.

**Moving in a fight** (`crates/live/bots/src/moves.rs`, built). The feet in a
fight, apart from the aim:

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| holds a strafe (ticks) | 40 to 80 | 20 to 50 | 12 to 35 | 8 to 26 |
| a new strafe is standing still | 35% | 15% | 5% | never |
| on a hit: reverses the strafe (juke) | never | 20% | 50% | 70% |
| on a hit beyond 8: crouches a moment | never | never | 10% | 15% |
| backs off inside its range | no | 0.6 | 0.85 | full |
| backs off to cover | no (straight back) | yes | yes | yes |

- A hit is a shield drop of more than 0.03 in a tick, with one
  footwork reaction at most every 20 ticks. A bot can also hop when it
  starts a new strafe, with `moves::FIGHT_HOP` (0.5) times its name
  profile's jump weight, at most every 45 ticks.
- **Range**: closes in beyond the far end of its range and backs off
  inside the near end. The range comes from `moves::preferred_range`,
  today 2.5 to 8 for every weapon: the seam for the weapons decision to
  give a shotgun a short range and a sniper a long one. The near end is
  measured in three dimensions, and an enemy more above or below than
  away is never backed from: stepping back under a ledge only hides it.
- **Another level**: with the enemy more than `moves::HEIGHT_GAP` (1)
  above or below, a strafe step that would lose sight of its head and
  chest takes the other side if that keeps sight, else the bot stands.
  An enemy that far above or below and beyond its range is reached by
  the nav mesh route (`Mind::go`), up the ramp or lift, not by walking
  under it.
- **Shields down or reloading**: backs away, reloading at half the pace
  and never closing in. With cover allowed it looks (every 15 ticks) at
  16 spots 1.5 and 3 away, walkable in a straight line, for one where the
  enemy's eye cannot see its chest through the walls (`los::Walls`), and
  goes to the nearest such, favouring ones away from the enemy; with none,
  straight back.
- **Footing** checks the nav mesh 0.5 and 1 world unit ahead for support,
  allowing a rise or drop of up to 0.5 between probes. It also checks for
  knee-height walls and standing clearance. A blocked strafe tries the
  other side, then forward or back. Backing away to recover shields uses
  the same checks.
- **Hops** check footing along the final walk direction, including any
  turn around a body, out to 1.6 world units. A grenade dodge
  (`crates/live/bots/src/incoming.rs`) uses the same distances and checks
  overhead clearance before jumping.
- **Stuck in a fight** reverses the strafe after 15 ticks of pushing
  without moving 0.15 world units. It requests a recovery hop when the
  footing check passes.

**Roaming** (`crates/live/bots/src/roam.rs`, built). With nobody to fight or
chase, a bot goes to likely places: each candidate is scored, the best
it can reach and has not been blacklisted wins.

- a place it saw an enemy in the last 60 s (4.5, fading to 0.5). A
  teammate's contact call is kept as a sighting the moment it is heard,
  and so is the bot's own guess at where a killed opponent respawned
  (the kill feed retires the target, §3.3). On a large map these are
  what bring phantoms to a fight across maps like Waterworks. A
  support walk answers a call only within tens of units. Roaming
  scores the sighting from wherever the phantom stands;
- a weapon lying in the world worth taking (1.5);
- the map's spawn points (1.0): on a large map with no sighting for a
  while, the likeliest place to find anyone is where players come from;
- six in-map polygons picked at random (0.5), so it still explores
  (never the terrain past the map's boundary, §3.2, "In the map");
- less 2.5 for a place within 5 it was at in the last 40 s (fading), and
  2 for one under 4 away;
- plus a random wobble: 8 on Easy (it wanders nearly at random), 2 on
  Normal, 1 on Heroic, 0.4 on Legendary (it goes where the fight was).

### Pace and hops

How hard a bot pushes the stick and when it hops on a run come from its
name profile (`mind::stride`). A bot without a profile keeps its thumb's
own pace (91 to 99.5% of the stick, seeded) times the route's cruise,
`nav::CRUISE_PACE` (0.94), and never hops on a run.

- **Travelling** with nothing in view, a named bot runs at full stick:
  the thumb's shortfall is closed and a route's cruise is lifted to 1.
  It still slows into a corner and onto its goal.
- **In a fight** it aims with care: its strafe weight closes that much
  of the thumb's shortfall, so a strafe-heavy name keeps the stick
  pinned while it shoots.
- **Hops on a run.** On an open straight stretch of a route (between
  fights, no corner within reach, not crouched, footing for a running
  hop and clear overhead), a roll every `HOP_EVERY` (30 to 90) ticks
  hops with `HOP_CHANCE` (0.6) times the name's jump weight. Fight hops
  are under "Moving in a fight" (§3.2).

Tests: `a_named_bot_runs_at_full_stick_and_hops_as_its_name_likes`
(`mind/tests.rs`) and the tests in `mind/stride.rs`.
