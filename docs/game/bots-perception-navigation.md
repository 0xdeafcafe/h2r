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
  order, proof limits and serialised grid bytes are identical to the
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
sight, nearby grenades, sound, deaths and teammate callouts. It thinks from
the newest sample at least `perception_ticks` old. A repeated poll in the
same tick reuses the previous pad.

If the host skips a sample, the brain can use an older observation. The
first live sample after a respawn starts a fresh history.

### The motion tracker

A phantom has no local-user HUD slot, so `bots::guest::radar` builds the
dots its own tracker would show. It uses the active variant's motion sensor
flag (setting 20, or setting 73 on the Juggernaut), the map's `hudg +0x290`
horizontal range and the player speed threshold read by `0x1a1d55`. A
hostile player shows when it moves faster than that threshold or fired in
the last `sound::HEARD_TICKS` (6): the shot sound names the shooter's slot
(`ShotSound::shooter`, from the players table's unit).

A player who walks crouched and holds fire does not show, nor do allies. A
mounted player's movement comes from its vehicle.

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

## Where the sections live

| Page | Covers |
|---|---|
| [bots-navigation.md](bots-navigation.md) | navigation, traversal, belief and pace (3.2) |

Each heading below is cited from code and moved to the page named after it.

## 3.2 Navigation

Moved to [bots-navigation.md](bots-navigation.md).

### Traversal audit by map

Moved to [bots-navigation.md](bots-navigation.md).

### Belief and map knowledge

Moved to [bots-navigation.md](bots-navigation.md).

### Pace and hops

Moved to [bots-navigation.md](bots-navigation.md).
