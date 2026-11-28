# Bots: Clankers and Clankermatch: detail moved off the page

Evidence and detail for [bots.md](../bots.md).

## 2026-11-26: detail moved off the design page

Read at commit `c7ed1e40c`. These blocks are the page's text at that commit, verbatim, moved here when the page was cut down to its design. Reproduce with `git show c7ed1e40c:docs/game/bots.md`. They describe the code as it was at that commit; the page states the current design.

### Clanker appearance and Live player options: appearance hash and web kernel

`live_proto::bots::appearance` lowercases the tag, collapses whitespace
and hashes its UTF-8 bytes with 32-bit FNV-1a and a fixed final mix. The
low bits select one of 64 foregrounds and 32 backgrounds. The two
foreground colours come from one brightness group and the two background
colours from the other, using four different retail palette indices.
Rusty Lugnut has foreground 1, background 30 and profile colours
10, 11, 13, 16. The server recomputes looks when projecting every
`BotIssued`, including old signed events, so existing accounts migrate
without rewriting their log or changing their personality. The game and
both Live homes use the pure Rust implementation. The web mock uses a C
mirror compiled as inline WebAssembly. Rust and browser tests read the
same 8,192-name fixture and check its fingerprint. After changing
`web/src/lib/clanker_appearance.c`, regenerate the browser module
with `python3 scripts/clanker-web-kernel.py` using LLVM clang and wasm-ld.
The web tests check the source digest.

### Clanker appearance and Live player options: retail trace, card, needs a game run

**Retail trace.** The main-menu tags identify the pregame lobby as
`0x0e`, the expanded/collapsed Live Y menus as `0x1a/0x1b`, and their
`player_selected_dialog` as `0x1f`. Zanzibar's in-game Live menu is
`0x1c` and also has `0x1f`. There is a separately named
`player_selected_dialog` tag `0x20` in the main menu. The presence of
that tag alone does not establish a loader for it.

- The friend-list press at `0x2b3870` copies the selected friend (record
  kind 2), commits it at `0x2b38ed` and opens through `0x2b71f0`.
  The list handler at `0x2b3e4d` builds kind 1 from the selected row and
  opens through `0x2b7212`. Another player selection at `0x23403c`
  copies the XUID and name at `0x234100` and uses `0x2b7223`.
- Both options loader families reach `0x2b70a3` or `0x2b71a6`.
  They take the request in EAX, source in ESI, and return the screen in
  EAX. Their shared constructor `0x2b6106` builds screen 31 with its
  action list at screen + `0x614`. The list is populated at `0x2b55ae`
  and its press handler is `0x2b5d3b`.
- `0x148893` writes the selected-player record at `0x54dc6c`.
  For kinds 1 and 2, +4 holds the eight-byte XUID, +12 the flags and
  +16 the ASCII gamertag. Kind 3 has only a name. The hooks leave
  name-only rows and human accounts on the retail path. A remote
  phantom needs a matching, gear-marked session slot.
- Recent Players uses retail's history. `RECENT_PLAYERS_BUILD` copies
  its records into the list, and `clanker_card` restores the display
  prefix on a reserved Clanker XUID. Selecting the row opens the
  read-only card with Gamertag, Difficulty, Style and Back. Its large
  dialog borrows the Controls screen's localized string list so its
  labels resolve even after the phantom leaves the session. Skin 6 of
  the current interface contains the Y menu's player table. Its nested
  skin's first shader bitmap uses the medium emblem material in the
  main menu, shared map and Zanzibar. The card copies that definition
  and feeds the profile to the retail emblem setter at `0x2b12ca`.
- The profile line sits inside the dialog above the emblem and uses the
  large panel's animation. It shows the gamertag’s try-hard band and
  strongest combat tendencies. The host shows the active brain’s
  difficulty and strongest temperament trait. A remote or recent server
  bot resolves its full name and last rostered skill through
  `GET /v1/games/bots/{xuid}`. The client derives the baseline temperament
  from that name and skill. While a queued lookup runs, Difficulty and
  Style show “Loading...” and the profile uses the available name. If the
  lookup fails or no skill is known, those rows show “Unavailable”. On
  the host, a scripted or recorded seat with no brain shows “Unavailable”
  for its combat profile. The card's page model is available in the UI
  builder as `clanker_player_options`.
- Mute and feedback are omitted from the card. A Clanker has no voice
  account to mute and no human player to report. Both server homes
  refuse friend requests and mailbox writes to either reserved XUID
  range with HTTP 403. Retail text and voice sends share the HLE outbox,
  which refuses the entire recipient list if it includes a bot.

**Needs a game run.** The common constructors and record layout are
confirmed statically. Which handler each retail lobby, in-game, recent
and friends row exercises still needs input-driven confirmation.
Select a bot and a human from each available surface, including a
recent bot that has left the session. Check `h2 agent ui` for
`dialog.name = clanker_player_options`, the rendered emblem, difficulty
and style, and return to the invoking list with B or Back. Repeat with a
remote client and with a human guest signed in on a second controller.
Check the same name after restart and in a non-team System Link game
against its Live card and web emblem. Use a legacy friends
entry if available, since new friend requests to bots cannot create one.

### 1. Decision: Normal Clanker behaviour detail

A Normal Clanker uses the motion tracker to investigate movement, draws
a suitable weapon when rushed, melees at close range and looks for a
safe grenade against a group, an approaching enemy or recent cover.
The game decides whether each swing, shot, throw or pickup succeeds.

### 2.1 Players and machines: field tables and slots

- **Players array.** Built by `players_initialize` `0x14b4b0`: 16 entries,
  stride `0x21c`, header at `[0x4e8c24]`. Fields per player:

  | Offset | Field |
  |---|---|
  | `+0x14` | machine identifier, 6 bytes |
  | `+0x1a` | machine index |
  | `+0x1c` | machine user |
  | `+0x20` | machine controller |
  | `+0x24` | local controller (-1 for a remote player) |
  | `+0x28` | local user 0..3 (-1 for a remote player) |
  | `+0x2c` | unit |
  | `+0x44` | name: UTF-16, 32 units |
  | `+0xc0` | team |
  | `+0xd4` | a copy of the player's configuration |

  Players are created at `0x14c090`, from `0x14bba8`, `0x14bc17` and
  `0x14bf84`: the membership and configuration update path. That path is
  only partly read.
- **Players globals at `[0x4e8c20]`:**

  | Offset | Field |
  |---|---|
  | `+0x08` | local player count (i16) |
  | `+0x0c` | local-user map: 4 salted player datums, -1 when empty |
  | `+0x1c` | controller mappings |
  | `+0x2c` | bitmask of valid machines |
  | `+0x30` | 16 machine identifiers × 6 bytes |
  | `+0x98` | local machine index |

  One machine has at most four local users. Each phantom occupies a
  separate peer slot.
- **Local slots.** 4 entries of `0xc70` bytes at `0x54e8e0`. Flag `0x10`
  (bit 4) means a profile is loaded, and `0x20` (bit 5) means signed in to
  Live (§4.2). The profile block is at `+0x18`.

### 2.2 The retail action path: per-tick walkthrough

Each tick, `0x1870a0` does this:

1. For every local user `u` in 0..3 whose map entry is not -1, it calls
   `0x186ab0(player, …, &actions[u])`.
2. `0x186ab0` runs `0x185be0`, the player-control update. It reads the pad
   and the per-user control state (`[0x4ed284] + u*0x94 + 0x14`) and the
   input settings (`0x51ea18 + u*0x1c`, `0x51ea88 + u*0x50`). It then fills
   a `0x5c`-byte **action record**, which includes the weapon index
   (unit `+0x212`/`+0x213`, via `0xcdeb0`).
3. It builds a mask of users, then calls `0x69040(mask, actions)`. This
   happens when `[0x4cf772] == 0` and the simulation's `+8` is non-zero.

`0x69040` submits the actions to the simulation:

- In mode 3 it sends message `0x28` through `0x85fc0`.
- Otherwise, for each user bit, `0x6a3b0(world, machine id at world+0xd, user)`
  finds the player. It matches the 6-byte machine id against the valid
  machines in `[0x4e8c20]+0x30`. Then `0x847d0(slot, action)` stores the
  `0x5c` record; `0x847d0` branches on the slot's mode (1 or 2).
- Another caller at `0x6a81c` copies 23 dwords to a mode-3 slot's
  `+0x2c` and stamps `+0x28` with game time. The caller at `0x978bb`
  instead reads the slot's player and gathers unit state, including position.
- `0x847d0`'s modes are the simulation slot's: 1 and 2 are client-side; 4
  (a remote player on the distributed server) quantizes and stores the
  record (§5.1).

Simulation globals live at `[0x4cf77c]`; the mode is at `+8`. The modes are
none, local, sync-client, sync-server, dist-client and dist-server. In
multiplayer the host is distributed server (5) and clients are distributed
client (4).

### Multiplayer round start (Built): fade and gate walkthrough

The retail engine can already be in round state 1 while the local view is
black. Map startup writes the fade alpha at `0x1585a4` and publishes
state 1 at `0x1585f4`. The engine reduces the alpha during its tick and
draws the overlay while it is positive (`0x15c4cc` and `0x1599b0`). The
player action consumer also honours the shared input-disable byte at
`[0x4e8c20]+6` (`0x14d765`).

`bots::guest::round::playing` is true once the published and requested
states are 1 and shared input is enabled. The phantom gather hook
(`Phantoms::slot_action`) returns no action and clears the seat's input
queue, uplink and source while it is false, so a delayed action from the
last round cannot arrive in the next. The opening fade itself stays
with `guest::match_opening_fading`: the phantom keeps observing and
sends a neutral pad while the first local view is black, and a respawn
fade later in the round does not close the round. A dedicated host has
no local view and uses the engine state and input byte. The gate reads
the same engine fields on retail and converted maps.

### 2.3 Views: render_game_views detail

`render_game_views` `0x2234e0` takes its view count from `[0x4e8c20]+8`,
clamped to 1..4. The split layout comes from `[0x4e6948]` and `[0x485ac2]`
(two views give layout 1 or 2). For each view it takes the next local user
whose map entry is not -1, and calls
`0x223080(view i of n, layout, …, 0x4e9c8c + u*0x358)`.

### 2.4 AI: actor array and scenario detail

at `[0x4e6948]+0xc`, where 4 suppresses updates. The actor array is at
`[0x4f55f0]`, stride `0x888`. MP scenarios carry no characters, squads or
`pathfinding` blocks (Repo: the h1ai and pathfind work generate exactly
these for converted maps). This is why option (c) would need a whole AI
layer before its bots could even stand still.

### 2.5 Menus: main menu, players screen, Live menu, matchmaking, Game Setup

**Main menu.** The row handler is `0x230827` and the chosen row is at
`0x510a14`. This table shows the retail indices. With Theater installed,
it occupies row 4, after System Link, and Settings moves to row 5.

| Row | Name | What it opens |
|---|---|---|
| 0 | Campaign | |
| 1 | Xbox Live | when signed in, the players screen `0x1e` in mode 3 (`0x25245a`) |
| 2 | Split Screen | `0x1e` in mode 1 |
| 3 | System Link | `0x1e` in mode 2 |
| 4 | Settings | |

**Players screen `0x1e`.** Constructor `0x2524cc`, vtable `0x45a490`, events
`0x25286d`.

- Joining at `0x25290f` adds the controller's bit to the screen mask and
  picks a profile (`0x18fc44`, `0x24b779`, `0x24b6ea`). Extra human
  players join this way.

**Live menu.**

- Screen `0xba`, list "xbox live menu list" `0x458c80`.
- Rows: Quickmatch 0, OptiMatch 1, Clankermatch 2, then Co-op Campaign 3
  (directly above Custom Games), Custom Games 4, Forge 5, Firefight 6
  (each retail row 2's press, the lobby in its mode), Custom Game
  Browser 7, Mods 8 (retail's Content Download). Rows close up by one
  without co-op. Seven show at once; the list scrolls.
- `crates/extensions/game/src/custom_games.rs` adds our rows: row text `0x230f36`, row
  chosen `0x230f92`, last row `0x510a18`.

**Matchmaking** ([live-matchmaking.md](../../live/live-matchmaking.md)).

- The playlist UI `0x65530` keeps descriptor types {2,4,5}.
- The start flow is `0x231066`; the session state machine is `0x192eb0`
  (states 6, 7, 8).
- Search goes through the `0x8fa80`, `0x8fb30` and `0x8fc5c` wrappers, and
  QoS through `0x7b4c0`.
- The server-side matcher (§3.4 there) writes `MatchProposed` on
  `match-<id>`. It is designed but not built.

**Game Setup** (`squad_settings_dialog`, screen 23, window 3; row chosen
`0x2bb801`, jump table `0x2bb884`) already carries our **Halo 1 Options**
row. That row opens the shared settings dialog `h1-options` (Sandbox and
HUD; docs/game/ui-builder.md "The settings dialog"), whose values are
kept in the settings store (`crates/extensions/game/src/h1_options.rs`).

### 3.1 Perception: inputs list and line of sight detail

The brain reads game state the way telemetry already does
([live-gamestate.md](../../live/live-gamestate.md)):

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
- in Firefight, the AI's living actors from the actor pool, as enemies
  (§3.10).

**Line of sight** is a ray against the map's BSP collision, the same data
the nav mesh comes from. A bot sees an enemy only when all three hold:

- a ray reaches the enemy's head or chest;
- that part is inside the bot's view cone and its visual range;
- a reaction delay has passed (§3.4).

A head over a parapet is seen. The bot aims at the part it sees: the
chest, or the head when only the head is clear (`mind::sight_point`).

### Visual range: view and distance rules

- **The view.** The cone is 40 degrees off the aim
  (`mind::NOTICE_CONE`), the corner of Halo 2's 70-degree, 4:3 frame.
  Past it only a player within 3 units (`mind::NOTICE_NEAR`) is noticed.
- **Distance.** In the centre of the view (15 degrees, `perceive::CENTRE`)
  a player is noticed out to the skill's range; toward the cone's edge
  the range falls to 0.4 of it (`EDGE_SIGHT`). A crouched player is
  noticed at 0.75 of the range (`CROUCHED_SIGHT`).

### Visual range: scope and camouflage rules

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

### Visibility grid (bots::vis): cells, proof, runtime, file, build, progress, loading, tests

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
- **Build.** A row reuses the ordered collision candidates for center
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

### The motion tracker: dots, glance rules, tests

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

### 3.2 Navigation: stands_on and crossing_clear checks

  (`nav::stands_on`): at its centre, or at a corner pulled 0.3 in. A floor
  that runs in under a low lip, as a crouch spot's does, keeps its links
  by the standing room beside the lip; a recess wholly under one has none.
  No walk, step or drop link crosses a wall standing on the polygon side
  (`nav::crossing_clear`): rays across the side's middle at 0.25 and 0.5
  over the higher floor, 0.15 to each side, must be clear. A wall planted
  on a side (an invisible boundary wall over continuous terrain, a
  building's face at a floor's edge) lies in the plane of the vertical
  headroom ray, which does not see it;

### 3.2 Navigation: link kinds in detail (drop, jump, crouch-jump, teleporter, lift)

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

### 3.2 Navigation: In the map classification detail and out-of-map reasons

without a glitch. A superbounce is never a mesh link, so the terrain past
a map's boundary walls and cliffs (Coagulation's snowfields) is out.
Reachability is directed: a plateau that drops back into the map is out,
a pit one drops into and cannot leave is in. A BSP no start or pad is on
gets no flags. Every polygon is in until a classification marks some
out. `autonav::mark_in_map` runs the same classification on one BSP's
mesh for another caller: the Firefight kit keeps only in-map sectors
([firefight.md](../firefight.md), "Sectors the AI can use").

Out-of-map polygons are kept, flagged, because a bot can still end up
there: a superbounce route (§3.8, "Superbounce routes") lands it there
and walks the mesh from its landing, and a bot a bounce launched out has
to find its way back in. The rules:

### 3.2 Traversal evidence and estimates: evidence table, clearances, cost allowances

| Input | Evidence |
|---|---|
| Teleporter flag position, type and channel | `scnr+0x118`, stride `0x20`, position `+0`, types `7` and `8` at `+0x10`, identifier `+0x14`. See `map::scenario_layout`, `h1convert/src/apply.rs` and the retail checks in `h1convert/tests/h2_scenario.rs`. |
| Scenery, machine and crate placement | `scnr+0x50`/ `+0x58` (scenery block / palette, strides `0x5c` / `0x28`) and `+0xa8` / `+0xb0` (machine, `0x48` / `0x28`). Palette index `+0`, flags `+4` (bit 0 suppresses automatic placement), position `+8`, yaw `+0x14`, palette datum `+4`. See the object-type table `0x00468630`, palette access `0x000a743b` and `map::scenario_layout`. Crates use the placement at `+0x328` (stride `0x4c`) and palette at `+0x330`. |
| Lift object identity and travel | The retail machine palette places `power_boost` on Ascension, `grav_colossus` four times on Colossus and `elevator_cyclotron` on Cyclotron. Name patterns for modded placements, the launch direction, travel envelope and exit timing are **[guess]**. The generator scans for a landing that fits a standing biped and checks the path against the BSP. The reverse axis used for Ascension projects onto elevated playable BSP geometry. |
| Jump and standing clearance | Retail multiplayer Chief has standing height 0.725 and crouch height 0.5. See [superbounce.md](../superbounce.md) and its traces of `0x000df66e`, `0x000dd8e0` and `0x000e2a06`. The sampled arc, shoulder offsets, and takeoff timing are **[guess]**. |

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

### Traversal audit by map: tables and ledge notes

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
unverified. See [Title updates and maps](../../runtime/title-updates.md).

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

### Belief and map knowledge: honesty harness and host input table

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

### Belief and map knowledge: pad estimates, spawn times, vantages, sound, experience records

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
Patience extends the wait at a projected return, and a favorite weapon can
tip the choice between two nearby pads. A patient long-range gunner or one
carrying its favorite rifle gives a stronger sightline more weight.
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

### Getting unstuck: blacklist rules

A blacklisted point is avoided for 10 s within 0.75 world units, 16 at
most: new paths skip the nav mesh edges whose portal passes near one
(`NavMesh::path_avoiding`; when no such path exists, the plain one), roam
goals skip polygons whose centre is one, and weapon pickups at one are not
sought. A pickup it holds the action button on for 1 s without getting it
(on a ledge, in a wall) is blacklisted the same way.

### Getting unstuck: bodies and follower stall check

within 0.35 of its line (not on another floor) turns the walk away from
them, keeping its speed. In a fight the target itself is not avoided.
`moves::around_bodies_safe` keeps the original direction if the turn
fails its footing or clearance check.

The follower's own stall check (`nav::Follower`, a jump after 30 frames
without getting closer to the waypoint, then a fresh path) runs alongside
the state machine. Both limit recovery hops on walking legs to directions
with footing out to 1.6 world units.

### Corners and doorways detail

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

### Off the mesh detail

mid-jump) snaps to the nearest polygon within 3 world units whose floor
is at most 0.6 over it or 3 under it: the bot walks (or jumps up) onto
the mesh first, and a chase to someone seen in the air goes to the floor
under them instead of giving up. A bot whose next waypoint's floor is
more than 1.2 (plus its distance) over it has fallen below its path and
plans again at once.

### Moving in a fight: hit, range, level, shields, footing, hops

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

### Roaming: first bullet detail

  teammate's contact call is kept as a sighting the moment it is heard,
  and so is the bot's own guess at where a killed opponent respawned
  (the kill feed retires the target, §3.3). On a large map these are
  what bring phantoms to a fight across maps like Waterworks. A
  support walk answers a call only within tens of units. Roaming
  scores the sighting from wherever the phantom stands;

### Pace and hops: bullets

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

### 3.3 Decision: In play today detail

A bot can fire at a visible opponent while walking toward the hill,
guarding its flag or escorting a carrier. Inside a held zone in Oddball,
King of the Hill and Territories (one of at least `FOOTWORK_ROOM`, 1
world unit) it keeps its fight footwork; past half the zone's radius the
step loses the part that leads out (`objective::within`), so it strafes
in the hill rather than standing still and never fights its way out. Target choice combines the
existing range, turn and stickiness costs with the objective, teammates'
calls, recent kills and visible openings. A bot that loses sight plays its
objective first. In Slayer (`Mind::between_fights`,
`crates/live/bots/src/mind/strategy.rs`) it then takes, in order: recover
shields; regroup with the nearest teammate after withdrawing or while
afraid; its own lead (hold the angle it lost the target round, then chase
the last sighting); a teammate's call or a trade; a due supply or loose
grenades; a heard shot or a guessed respawn; move up with the lower-slot
teammate when that one is more than 10 to 19 units away; a firing
position; roam. A teammate's call never pulls a bot off a fresh lead of
its own, and trailing a teammate comes only once nothing else is left. A
team call alone never authorizes a shot.

### Finishing and breaking off, Full shield

A target whose shield is down is run down, not strafed at while its
shield comes back. The bot reads the shield from the visible contact (at
or under 0.25), or counts it as down after twice the melee finish's pulls
on target (`weapons::Combo::softened`: 6 at Legendary, about 1.6 s of
automatic fire). With an automatic, a shotgun or nothing loaded it runs
straight in from within 6 units; with a rifle only from within 2.5,
since its headshot finishes further out; never with a sniper, splash gun
or sword (`tactics::finish_push`). Running in uses the rush footwork and
the plasma reach for the melee (1.17 units), so the melee lands. It
does not run in while carrying or withdrawing, and it keeps firing on
the way.

The bot withdraws when it sees more enemies within 8 plus 12 times its
awareness units than it has teammates there with a clear line, plus a
margin of one or two from its caution, aggression and drive. A softened
target overrides that when the bot's own shield is at least 0.5 and it is
outnumbered by no more than one beyond the margin. The Juggernaut never
withdraws (posture Press).

Recovering shields, it backs away from the last sighting for 1.5 s after
losing sight (`mind::RECOVER_BACK_TICKS`), then stands and watches that
way until the shield is back. Fear sends a bot off on its own only while
its shield is under the recovery line (0.9); with a full shield a fearful
bot regroups with a teammate if there is one, else follows its own lead.

**Full shield.** Every shield line in the brain (the recovery lines,
the fight's back-off and the decision layer's 0.4 to 0.86) reads the
shield as a fraction of the bot's full shield. The perception reads that
full shield as `0x1588b0` computes it for a spawn
(`guest::full_shield`, `Me::shield_full`): the variant's shields (1, 0,
or `[0x45dbd4]` for overshields) times the handicap's factor (1, 0.75,
0.5, 0.25). `mind::relative_shield` divides by it, capped at 1, so an
overshield still reads above 1. In a variant without shields the body's
vitality stands in for the shield, so the bot still backs off when hurt
and comes back when healed. With absolute lines, a bot whose full shield
was under 0.25 was always "shields down". In game
`09af571f9fec7f1a33ed37edb397a140` the bots carried a handicap of 3 and
spent the match recovering. The median angle between their walk and their
aim while firing was 178–180°, so they backpedalled through every fight.
In game `c9a3aa1a3f867c298a6ae32b56e50399`, with full shields, it was 90°,
a strafe. `bots.stats` `combat.standing_decisions.recover` shows the time
spent recovering. The tests are
`mind::tests::shields_down_is_a_fraction_of_the_full_shield`,
`without_shields_it_backs_off_on_health_alone` and
`guest::tests::the_full_shield_follows_the_variant_and_the_handicap`.

### Warm-up detail

`NameProfile::seed`; the brain's seed without a name), so it is the same
every match and differs between bots. Taking damage ends it at once: a
bot that is shot fights back. A brain makes a fresh mind each match
(`Brain::reset_mind`), which is when the warm-up is due again; a respawn
does not start it. A bare `Mind::new` (the tests') has none.

### 3.3 Decision: grenades

0.8 a second, heading within about 37° of it: `grenades::Mark::closing_on`), or the last position
where an enemy disappeared within two seconds. Both chasing and holding
the corner present that opportunity. In these short windows Normal has
about one chance per second before temperament, collision, aiming and the
eight-second cooldown. Otherwise a single visible target gets the skill's
ordinary per-minute rate, tripled on Normal and up when pushing with a
short gun. The thrower checks the predicted blast and checks the actual
aim and friendly positions again before pressing LT. A dual-wielding bot
drops the left gun to open its throwing hand only against a group, or
when the left gun is dry with nothing to load.

### Personality and temperament: traits table and moods

A keen player tends to check radar more often, attempt the plasma pistol
overcharge and headshot follow-up, strafe and jump more, and react with
less delay. A casual player is more likely to take a compatible second
gun. A weight of 0.3 on the combo means roughly three attempts in ten
eligible openings. Holding the same opening for ten ticks gives it one
chance. The card summarizes the most distinctive tendencies.
`h2 agent bots.stats` shows their numeric weights for the current
difficulty and style.

| Trait | What changes in play | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|---|
| Aggression | Takes openings and closes distance | .05–.95 | .12–.92 | .24–.90 | .35–.88 |
| Caution | Respects being outnumbered, routes around recent danger and plays a hill's edge | .05–.90 | .16–.88 | .30–.84 | .46–.82 |
| Teamwork | Responds to radio calls and helps teammates | .08–.85 | .18–.88 | .32–.88 | .46–.87 |
| Objective focus | Stays closer to the assignment during a fight, and holds a hill, a territory or the ball carrier's fight one enemy longer | .05–.90 | .18–.88 | .35–.90 | .48–.89 |
| Composure | Has smaller swings in mood and settles faster | .08–.50 | .28–.70 | .50–.86 | .73–.96 |
| Patience | Holds a strafe or a corner longer before chasing, and waits for teammates before pushing the ball carrier | .05–.48 | .22–.72 | .42–.85 | .65–.91 |
| Showboating | Attempts riskier melee and sword lunges and taunts more often | .10–.95 | .10–.87 | .12–.75 | .15–.67 |
| Weapon loyalty | Favors a weapon family and may take a short detour for it during a lull | .05–.95 | .13–.88 | .20–.80 | .28–.72 |
| Chattiness | Refreshes radio sightings more often. New targets and urgent help go out immediately | .05–.95 | .12–.90 | .25–.83 | .36–.80 |

The narrower Legendary bands give bots more consistent discipline while
leaving room for different fighting styles. Favorites are drawn
from automatic guns, precision rifles and pistols, shotguns, snipers, splash
weapons, and swords. Loyalty biases existing pickup choices and can justify
a short detour for a visible favorite when the bot is calm and free to rearm.

Confidence, tilt, and fear each run from 0 to 1. Enemy kills build confidence,
with a larger gain as the streak grows. A death breaks the streak, drops
confidence, and raises tilt and fear. Betrayals and losing a recent favored
duel add extra tilt. A duel is favored when the bot sees one opponent with no
nearby teammate in the fight, holds a weapon ready at that range, and has a
shield lead of at least 0.30. While dead, the bot can read two crouch requests
by its killer within 8 m of the body in the first 3 s as a taunt.

The three mood values decay with game time, with half-lives of about 8 to
17 s depending on composure. Confident bots commit to more fights. Tilt
can make a bot press and throw grenades more often. It remembers the killer
for up to 10 s and may favor that opponent in view or pursue its last
sighting. A frightened bot is quicker to withdraw and regroup with a
teammate. These choices respect combat range, objective and input gates.
The human-input layer caps extra tracking noise from tilt and fear at 10%.
Each match pad still passes through the controller's stick slew, button
timing and thumb coordination limits before reaching the phantom's action
gather.

### 3.4 Aim: delays, hand model, fire cone, trigger, reaction

The continuous delay covers seeing a rendered change and getting a new
controller report into the game. Easy and Normal share a five-tick budget
at 30 Hz. Their hesitation and timing precision differ. Heroic sees the
change one tick sooner, and Legendary also returns a report one tick
sooner. These are tuning defaults for playtesting.

The first-sight reaction remains the time to notice and commit to a new
target after its first visible sample reaches the brain. The five-tick
floor (167 ms) applies to that reaction. The first input arrives after the
perception delay, one reaction wait and the motor delay, with up to a
tick of scatter on a button edge. Later decisions on the same target use
the same continuous pipeline. A brief occlusion keeps the reaction
clock and clears the velocity sample.

Both virtual pads use the game's stick response, look acceleration and
aim assist at sensitivity 3 (§5.1). The hand steers by a model of that
response (`mind::look::RESPONSE`): on each axis, stick travel against
turn rate, piecewise linear from the 0.13 dead zone. Its rows are seeded
from the one measured point, about 9°/s in yaw and 5°/s in pitch at half
stick before aim assist (§5.1), and the nominal 3 rad/s at full yaw
stick, with pitch at 5/9 of yaw throughout. The full-stick ends are a
guess until a stick sweep on each axis, read from `sim.player.list`'s
`aim`, replaces the rows. The hand asks for the skill's `gain` × 3 rad/s
per radian of error and reads the stick for that off the table; under
0.012 rad/s the thumb rests. The right thumb takes several ticks to cross
the stick: the skill's travel a tick (0.15 to 0.25 of the stick), and on a
tracked target more than 0.3 rad off, 0.4 a tick at every skill
(`mind::look::FLICK_STEP`), under the pad's 0.45, so a large flick is at
full stick on its third tick.

On a still target the thumb settles instead of correcting every tick
(`mind::look::rest_band`). Once the error the hand sees is under
`clamp(0.2 / d, 0.004, 0.04)` radians it lets the stick go, and takes it
up again only past twice that. A target whose bearing moves faster than
0.05 rad/s gets no rest. A steady hand mostly rests on a still target;
a shaky one's own error keeps it outside the band and correcting, as a
player's thumb stays still on a target it is on.

The view is late. The mind sees an observation `perception_ticks` old,
its own facing included, and a report lands `motor_ticks` later. The hand
keeps the modelled turn of its last 16 reports and adds the turn of the
last `perception_ticks + motor_ticks` of them to the facing the view
shows (`Hand::ahead`) before it measures the error, so it does not swing
past a target it has already turned to. The same predicted facing turns
the walk into the left stick and aims a look with no target. A small
integral term takes off a standing offset that the table's error leaves:
it gathers the error the hand sees at 1/s while that error is under
0.1 rad and the thumb is not resting, adds at most 0.05 rad, and clears
when the target is lost.

Large flicks carry a little past the target before settling. Small
correlated errors and the occasional larger misread vary by skill.
`error` is their spread at 5 world units. Further out the angle shrinks
to `error × (0.2 + 0.8 × 5 / d)` (`mind::look::spread_scale`), so a hand
misses a far target by a smaller angle and a larger distance.

The aim point leads a moving target by the skill's `lead` fraction of
its velocity times the shot's flight time plus the hand's own delay,
`perception_ticks + motor_ticks` (`mind::look::lead_time`). The hand
adds no lag of its own.

**The fire cone** (`mind::look::fire_cone`). The trigger is pulled while
the aim is within `clamp(0.35 × (1 + looseness) / d, 0.012, fire_cone + 0.3)`
radians of the aim point: half a body's width as an angle, wider for a
forgiving gun (looseness as in §3.4.1's inputs: sniper 0, precise 0.15,
blast and unknown 0.5, spray 0.7, shotgun and sword 1). With a rifle that
is about 1.2° at 20 units and 7.7° at 3. The skill's `fire_cone` only caps
it at point blank.

**Holding the trigger** (`tactics::hold_pattern`, `tactics::Hold`). An
automatic inside its gun's band is held until the target or the magazine
is gone, at every skill. Past the band Easy holds anyway; Normal, Heroic
and Legendary fire the skill's bursts, the on time 0.7 to 1.5 times the
table's as the name's spray (`tactics::spray`: half its aggression, half
its reaction jitter) runs from 0 to 1. Once down, a held trigger stays
down while the aim is within 2.2 fire cones, and through 4 ticks outside
that or with the shot blocked or out of range. A burst let go before its
end is finished by the next press, without waiting out its gap.

The reaction gates only the trigger, melee and grenades. Until it is
over the bot keeps walking where it was going (its route, else last
tick's walk) and the right thumb already turns toward the part of the
enemy it sees, at `REACTION_GAIN` (half) of the angle. An alert bot
reacts two ticks sooner (`Tactics::focus_alert`): one that saw the enemy
within the grace period, has a tracker dot near (§3.1, "The motion
tracker") or was just hit from out of view. Tests:
`it_keeps_moving_and_starts_turning_before_it_may_fire`,
`shot_from_behind_it_turns_to_the_shooter_within_ten_ticks` and
`a_moment_out_of_view_does_not_freeze_it_again`.

### 3.4 Aim: controller boundary and timing errors

At the controller boundary, each stick stays within its unit circle and
can travel at most 0.45 stick radii per tick. A full reversal takes at
least five reports. The model uses a standard two-thumb grip. The right
thumb can press one of A/B/X/Y/Black/White while the right stick is
centred, or use the stick or its click. It can switch between those jobs
once per tick (33 ms) by default. A face-button tap waits for the stick
to return to centre, then occupies at least one controller report. The left thumb can move its stick and click it.
The index fingers can use the triggers during a face-button press.

Button and trigger transitions draw independent, centred Gaussian timing
errors using a per-bot seed. An error of, for example, 6 ms has a 6/33.3
chance to shift the report by a tick in the direction of the error.
Reports slip by at most one tick. At zero motor delay an early edge can
only land on the current tick. Two edges on the same boundary can erase a
short tap. Sticks use the nominal motor delay. The same seed, samples and
tuning reproduce the same input.

### 3.4 Aim: lead and grenades

Lead comes from the target's velocity, the weapon's projectile speed,
read from the tag at load, and the hand's delay. A grenade's throw is
flown against the map's collision (bounces, the fuse, a plasma sticking)
and its aim searched until it goes off at the target; the bot throws only
when one does (built: `bots::grenades`, "Built: weapon skill" in §10).
The numbers in the table are starting points; tuning them needs runs
(§11).

### Phantom aim diagnosis and next steps

In the Midship replay on 2026-11-19
(`531b5da3ceab388417c044afcf18e01a`), four phantoms fired 2,231 shots.
They dealt positive damage to opponents on 14 bullet hits and 9 melee
hits. Three local Clankers fired 1,372 shots and landed 236 bullet hits.
The hits show that phantom actions reached the host.

The old adapter turned a half-stick input at about 54°/s. A local bot at
the same deflection turned about 9°/s in yaw or 5°/s in pitch before assist.
The old adapter also sent an empty shot-assist block. The contribution
of each difference to the observed miss rate is still unknown.

Retail builds local actions at `0x186ab0`. The control update at
`0x185be0` calls `0x1a4900`, which gets the held weapon’s cone from
`0x1a50a0` and searches for a target at `0x1a5370`. The local builder
copies nine dwords from the per-user control record into action
`+0x34..+0x57` at `0x187074..0x187083`. The remote gather at `0x84990`
expects that search to have run on the sender. An empty record has no
target and zero strengths for the barrel's magnetism check.

The host now runs the retail local builder on each arrived phantom pad.
Its private copy of user zero has the phantom's unit, camera origin,
profile binding and control history, including on a host with no local
profile. The host validates the full action at `0x14d0a0` (§5.1).
A historical ten-seed Midship batch measured four players for 9,000 ticks
per original leg. Red locals averaged 0.165 opposing hits per shot and
Blue phantoms 0.155. The swapped legs never bound all four players, so
team and transport were confounded.

For a phantom in a hosted match, check `sim.player.list` for
`aim.assist_player` and `aim.assist_strength` as a target enters and
leaves the reticle. For aim comparisons, run matched phantom rosters
with the same maps and seeds (§3.4.1).

### 3.4.1 The aim net: result and background

**Result (2026-11-20).** `aim-net-vs-code.toml` ran all 80 matches
(Midship and Lockout, easy to legendary, 5 seeds, both arm-swap legs,
5-minute 2 v 2, using the local seats available at the time). The code
aim won 60% of games to the net's 40%, and the net lost on accuracy (13.1% against 15.8%), damage (9.8
against 11.8 a game) and grenades (0.3 against 1.1). The net barely
overshoots (0.03 rad against 0.17), but it settles 1.3 s slower and its
stick jitters more. It is cautious, and it loses the first shot. The gap
grows with skill: at heroic the code won 78% of games, at easy 50%. Both
stay in the tree. `code` remains the default, and the net is a starting
point if its settle time can be cut without bringing the overshoot back.


The user asked for a small neural net with hand-written weights for aim
and tracking, tested in simulation against the code above, the winner to
stay. `bots::aim_net` is that net. Nothing is trained and there is no ML
dependency: the weights are `const` arrays built from named constants, so
each hidden unit has one job.

### 3.4.1 The aim net: what it replaces

**What it replaces.** Only the step that turns the hand's view of a
tracked target into the stick it wants. With `code` that is the tracking
lag, the flick offset and `gain × error` through the stick curve. With
`net` it is one forward pass. Both models share everything around that
step:

- the view, as late as `perception_ticks` makes it;
- the hand's wandering error and misreads, from the same seeded process,
  scaled by `error` and by mood through `human_input::tracking_noise`;
- the thumb, which moves at most the skill's travel a tick (0.15 easy to
  0.25 legendary) inside the stick's circle;
- the controller: motor delay, the 0.45 stick step, the one-thumb rule;
- target choice, the fire cone, the trigger, movement and the shared
  action builder.

A look with no tracked target (travel, a glance at a sound, a pilot's
line) uses the code's curve under either model.

### 3.4.1 The aim net: inputs and hidden units

**Inputs (18).** Yaw then pitch where there are two.

| Input | Unit | From |
|---|---|---|
| error | rad | aim to the led aim point, before the hand's error |
| error rate | rad/s | this view's error less the last one's |
| target rate | rad/s | the target's bearing from the eye, view to view |
| stick | -1..1 | the thumb now, turn-positive |
| pending turn | rad | the nominal turn of the last `perception_ticks + motor_ticks` reports: asked for, not in the view yet |
| since acquired | s | 0..2 |
| size | rad | 0.25 / distance: the target's apparent half width |
| looseness | 0..1 | the weapon: sniper 0, precise 0.15, unknown and blast 0.5, spray 0.7, shotgun and sword 1 |
| gain | 0..1.5 | the skill's `gain`: 0 at easy's 1.6, 1 at legendary's 4.5 |
| spread | 0..1.5 | the skill's `error` / 0.10 |
| lead | 0..1 | the skill's `lead` |
| noise | rad | the hand's wandering error this tick |

The three skill scalars come from the tuned aim table, so `gain`, `error`
and `lead` in a tuning file move the net as they move the code.

**Hidden units (24, ReLU).** They come in mirrored pairs, one unit a
direction. Skill enters only through biases: it widens or narrows a dead
band or a clamp. With `seen = error + noise`,
`predicted = seen + 0.02 × error rate + 0.12 × target rate − 0.9 × pending`
and `tolerance = 0.006 × spread + 0.06 × size + 0.004 × looseness`:

| Units | Name | Computes | Job |
|---|---|---|---|
| 0, 1 | yaw pull | `±predicted − tolerance` | The PD term. The error rate is the D. The target rate carries the error forward over the view's age, and the pending turn takes off what is already on its way. Out at 3.9 stick/rad. Inside the tolerance the thumb rests. |
| 2, 3 | yaw boost cap | `2.0 × pull − 0.43 × gain` | Takes the extra 2.0 stick/rad away again once it has given `0.43 × gain` of stick. Easy gets 1.9 stick/rad everywhere; legendary 3.9 for the first 0.2 rad. |
| 4, 5 | yaw breakout | `±seen − tolerance` | The micro-correction. Out at 8 stick/rad on the error as seen, so a small miss gets a stick past its dead zone at once. |
| 6, 7 | yaw breakout cap | `8 × breakout − 0.17` | Stops the micro-correction at 0.17 of stick, just past the 0.13 dead zone, where the view creeps. |
| 8, 9 | yaw lead | `±target rate` | Velocity feed-forward: 0.35 stick per rad/s, so the thumb holds the turn a moving target needs with no error to drive it. |
| 10, 11 | yaw lead cap | `0.35 × lead unit − 0.6 × lead` | Stops the feed-forward at `0.6 × lead` of stick. Easy keeps up with 0.5 rad/s, legendary with 1.7. |
| 12, 13 | yaw follow-through | `±stick − 0.5 − 1.2 × since + 0.3 × spread` | The flick overshoot. A thumb already hard over early in an engagement stays over (0.8 out), so the aim carries past and comes back. It fades within 0.4 to 0.7 s and is wider for a wider spread. |
| 14–21 | pitch pull, boost cap, breakout, breakout cap | as yaw's | The same four pairs on pitch. |
| 22, 23 | pitch lead | `±target rate` | Pitch feed-forward, 0.25 stick per rad/s, no cap. |

### 3.4.1 The aim net: outputs and delays

**Outputs (2).** The yaw and pitch turn, each the weighted sum of its
units clamped to −1..1. The hand maps `[yaw, pitch]` to the right stick
`(−yaw, pitch)`, keeps it in the unit circle and moves the thumb there.
No fire gate: the trigger stays with the shared fire cone, so the
comparison measures aim alone.

A view is `perception_ticks` old and a report lands `motor_ticks` later:
five ticks at easy and normal, four at heroic, three at legendary. Both
models count the turn they have asked for that the view does not show
yet. The net takes 0.9 of it off its error, counted on the nominal radial
curve its weights were designed against (3 rad/s at full stick, about
54°/s at half). The code takes all of it off, counted on the response
table (§3.4). The 2026-11-20 comparison ran before the code counted it:
that code steered on the old view as if it were current, and in the test
plant a 0.95 rad flick swung 37–54% past a still target where the code
now goes 5–10% past.

### 3.4.1 The aim net: test detail

**Tests.** `cargo test -p bots aim_net` covers the network alone: the
same bits for the same inputs, no stick for no error or inside the dead
band, sign and axis, saturation, the gain order by skill, each named
unit's effect, and the table's shape. `cargo test -p bots look` runs both
models through one kinematic harness (the view as late as the skill's
delays; the code on the response table, the net on the nominal curve):
determinism, the code aim untouched by the switch, the net landing a
0.95 rad flick and settling sooner and closer at each higher skill,
tracking a strafing and a crossing target, and the mood scaling of its
error. The code aim's own tests run on a plant with the response table
and the late view: a flick that does not swing past
(`a_flick_on_a_late_view_does_not_swing_past`), pitch held as well as yaw
with a height gap, the error's angle shrinking with range, a
strafing target led for the hand's delay, the thumb resting on a still
target and not on a crossing one
(`on_a_still_target_the_thumb_rests_and_on_a_moving_one_it_tracks`), and
a large flick at full stick by its third tick
(`a_large_flick_takes_the_thumb_to_full_stick_at_once`). The code's
thumb, resting on target, is no busier than the net's on a flick.

### 3.4.1 The aim net: comparison and decision rule

Midship and Lockout, four skills, five seeds, two against two, every bot
at the run's skill. Each match is played twice (`mode = "arm_swap"`): the
second leg gives every bot the other arm, so each model plays every seat,
team, spawn and brain seed. 80 matches. Only `aim_model` differs between
the arms. `compare.txt` under
`docs/runtime/sim-examples/results/aim-net-vs-code/` has one table over
all runs and one per skill, with net − code and its 95% interval:

| Row | Meaning | Better |
|---|---|---|
| `accuracy` | hits / shots | higher |
| `kd`, `win`, `kills`, `deaths` | the fights | higher K/D |
| `ttk_s` | seconds from the killer's first hit of a fight to the kill | lower |
| `aim_error` | mean angle off the aim point while tracking, rad | lower |
| `overshoot` | how far past the target a flick of 20° or more goes, as a fraction of the flick | 0.03 to 0.25 reads as a hand |
| `settle_s` | seconds for such a flick to stay within 0.05 rad | lower |
| `jitter` | right-stick travel a tick while tracking | lower |
| `reversals` | changes of the stick's yaw direction a second | lower |

**The decision rule.** Read the per-skill tables.

1. The net wins if, at three skills or more, it is better with the
   interval clear of zero on at least one of `kd`, `accuracy` and
   `ttk_s`, worse with the interval clear of zero on none of them at any
   skill, and no less like a hand: `jitter` and `reversals` not higher
   with the interval clear of zero, and `overshoot` inside 0.03 to 0.25.
2. The code wins if the reverse holds.
3. Anything else is close, and the simpler one stays. That is the code:
   it is the incumbent and carries no extra state.

Also check that each model's own `accuracy` still rises from easy to
legendary. A net that makes easy bots sharp has lost the difficulty
ladder whatever its K/D.

The loser is deleted, not kept behind the switch. If the net wins, the
default becomes `net`, then the code's flick offset and integral term go.
If the code wins, `bots::aim_net`, the switch and the hand's net state
go. The aim-motion rows and `arm_swap` stay either way.

### 3.5 Emotes: intro

The kill feed supplies the body position and the event. Self-kills (including
world deaths) and betrayals can prompt a taunt. Showboats can also taunt over
their own ordinary kills, at a lower rate.

### 3.5 Emotes: severity

Severity is 1 for a betrayal, 0.8 for a self-kill, and 0.35 for the bot's own
ordinary kill. Showboating and current confidence set the weight. The
Clankers page's **Taunts** switch sets `taunt` to 0 when off. A persistent bot
has its own base taunt value.

### 3.6 Weapons and ammunition: fields table

**Fields.** Traced offline from the XBE with `h2 disasm` (static reading,
no game run). `U` is the unit's object body, `W` a weapon's, `D` the weapon's
`weap` tag metadata (`W+0` is its tag datum; the tag entry's `+8` the
metadata address, `tag_entry_checked`).

| Field | Where | Type | Confidence | Evidence |
|---|---|---|---|---|
| held weapon slot | `U+0x212` | `i8`, -1 none | confirmed | docs/live/live-gamestate.md ("Unit aim, held weapon"): `unit_new` `0xc4410` sets -1; `0xc58f0`, `0xc85c0`, `0xcf3d0` index `+0x218` with it |
| weapon datums | `U+0x218` | `u32[4]` | confirmed | as above; multiplayer fills two |
| grenade counts | `U+0x23e` | `i8[2]` (frag, plasma) | confirmed offsets, names unverified | live-gamestate.md ("Unit stance"): `0xcdff0` and the HUD index it with `U+0x23c` |
| magazine count, pointer | `D+0x2c0`, `D+0x2c4` | `i32`, `u32` | confirmed | `0x101a10` (sets a weapon's ammunition from a fraction): `mov eax,[edi+2C0h]; test eax,eax; jle`, then `mov ecx,[edi+2C4h]`; `0x101740` (ammunition between two weapons of one kind) loops over the block with stride `0x5c` (`add esi,5Ch`) |
| rounds a magazine holds | magazine definition `+0xa` | `i16` | confirmed | `0x101a10`: `movsx edx,word ptr [ecx+0Ah]`, times the fraction, rounded (`fistp`), is stored as the rounds loaded (`mov [esi+22Ch],dx`) |
| most rounds in reserve | magazine definition `+0xc` | `i16` | confirmed | `0x101a10` caps the reserve with it (`movsx ecx,word ptr [ecx+0Ch]; cmp ax,cx; cmovg`); `0x101740` tops a reserve up to it |
| reserve rounds | `W+0x22a` (magazine state `+2`) | `i16` | confirmed | `0x101a10`: reserve = reserve + loaded - new loaded (`mov ax,[esi+22Ah]; sub ax,dx; add ax,[esi+22Ch]`), clamped to 0..`+0xc`; `0x101740` moves rounds into it from the other weapon's reserve and loaded |
| rounds loaded | `W+0x22c` (magazine state `+4`) | `i16` | confirmed | as above; magazine states are `0x10` apart (`0x101740`: `add eax,10h; add ebx,10h`) |
| barrel count, pointer | `D+0x2d0`, `D+0x2d4` | `i32`, `u32` | confirmed | `0x101d20` loops `D+0x2d0` barrels from `D+0x2d4` with stride `0xec` (`imul ecx,0ECh`) |
| battery used per shot | barrel definition `+0xa8` | `f32` | confirmed | `0x101d20` returns 1 (the weapon "ages") when `D+0x2c0` is 0 or any barrel's `+0xa8` is over 0; the fire path at `0x1036d1` loads `[esi+0A8h]` and calls `0x101db0`, which adds it to `W+0x184` (`addss xmm0,[esi+184h]`, `0x101e39`) and clamps at 1.0 (`[0x45dbc0]`) |
| battery used | `W+0x184` | `f32`, 0 fresh to 1 dead | confirmed | as above; `0x101a10` writes `1 - fraction` there for an aging weapon; `0x101740` swaps it with a same-kind weapon's when theirs is fresher (`comiss`, then the swap). Skulls' Unlimited Ammo already keeps it at 0 (docs/game/skulls.md) |
| a weapon's holder | `W+0x14` | `u32` datum | confirmed | live-gamestate.md: the parent; none for a weapon on the ground |

### 3.6 Weapons and ammunition: how the bot reads them

**How the bot reads them.** `arms::read` takes the unit's four weapon
datums, each resolved like any object (salt and type 2, weapon) and its
definition through the tag entry (datum compared). A weapon whose
definition ages (`0x101d20`'s test) is a **battery**: `1 - W+0x184`. One
with magazines and no aging barrel is **rounds**: the first magazine,
loaded `W+0x22c` of `+0xa`, reserve `W+0x22a` of `+0xc`. The retail
energy sword has no magazines or barrels and uses battery age. On
exhaustion `0x101e69..0x101e74` replaces its definition with the tag at
`D+0x304`, the depleted blade on Zanzibar. The multiplayer weapon type
excludes the flag, bomb, ball and Juggernaut item from supplies. The two
hands use slots `U+0x212` and `U+0x213`. The first other occupied slot
is the stowed weapon.

`arms::pickups` walks the objects array (at most 2048) for live weapon
objects with no parent: placed weapons from the netgame spawns and dropped
ones alike. A pickup is **useful** when it is a gun with something in it.
A fault while reading weapons leaves the bot playing without the weapons
layer (`Me::arms` is `None`); it never blinds it.

### 3.6 Weapons and ammunition: handling field table

**Handling.** What kind of gun a weapon is comes from fields of its
`weap` tag and its first barrel's projectile
(`arms::WeaponDef::profile`, `arms::Profile`):

| Field | Where | Confidence | From |
|---|---|---|---|
| zoom levels | `D+0x1fe` (`i16`) | checked on Zanzibar's tags | `boot::touch_glue` (sniper and beam rifle 2; battle rifle, carbine, rocket launcher 1) |
| aim assist reach | `D+0x20c` `cone_range`, `D+0x214` `pull_range` (`f32`, world units) | confirmed (engine) | [aim-assist.md](../aim-assist.md) (`0x1a50a0`) |
| triggers block | `D+0x2c8` count, `+0x2cc` pointer, `0x40` each; `+6` the behaviour (`u16`: 0 while held, 1/4/5 once per pull, 2/3 charge) | checked on Zanzibar's tags | `boot::touch_glue`; [halo1/objects.md](../../halo1/objects.md). None on the energy sword |
| rounds a second | barrel `+8` (`f32`, the most) | checked on Zanzibar's tags | `boot::touch_glue` |
| projectiles per shot | barrel `+0x6a` (`i16`) | field order from data, value pattern on retail | [halo1/objects.md](../../halo1/objects.md) |
| projectile | barrel `+0x8c` (tag reference, datum at `+0x90`) | field order from data, value pattern on retail | [halo1/objects.md](../../halo1/objects.md); the bot accepts it only when the entry's group is `proj` |
| projectile speed | `proj+0x17c` (`f32`, world units a second) | confirmed (engine) | `live-telemetry` (`0xf8487`) |
| projectile reach | `proj+0x16c` air damage range end, `proj+0xe0` maximum travel (`f32`, world units) | engine and disc | `0xfc016` copies the air endpoint into the live projectile. Blood Gulch's converted shotgun carries Halo 1's endpoint, 3, below its 40-unit travel limit. Coagulation's shotgun has 8 in both fields. |

### 3.6 Weapons and ammunition: roles prose

The first trigger and the first barrel speak for the weapon. A fault
reading any of them leaves the gun weapon-blind (as before), not unread.
The **role** (`arms::Role`) follows: two zoom levels a **sniper**; once
per pull with a projectile under 100 a second a **blast** weapon (rocket
launcher, fuel rod, brute shot); 4 or more projectiles per shot a
**scatter** gun (shotgun), including converted tags with a held trigger;
once per pull otherwise
**precise** (magnum, battle rifle, carbine); held or charged **spray**
(SMG, plasma rifle, needler, plasma pistol); no trigger but aim assist a
**blade** (energy sword). The Zanzibar flag, ball and bomb each have a trigger. Each role has a band it is best at and a range

### 3.6 Weapons and ammunition: fire-past-band rule

One rule sets where every gun stops shooting: 1.25 times its own band's
far end (`weapons::FIRE_PAST_BAND`), whatever its class. A person sprays
a little past where a gun is good and never across the map, so an SMG
with an 8-unit band fires to 10, a battle rifle with a 20-unit band to
25. The band comes from the gun's own tag data (`weapons::reach`), and
the role table above is the fallback when the tag has none. Past the
band the trigger works in bursts from Normal up (§3.4). A plasma rifle or plasma pistol (by identity), or any
automatic whose projectile is slower than 150 world units a second,
keeps its target within 6 and so fires to 7.5 at most
(`weapons::SLOW_BOLT_BAND`, `SLOW_BOLT_FIRE`): a strafing target steps out
of slow bolts ([round 1 on Midship](../notes/2026-11-23-bots-natural-round1.md):
the plasma rifle hit 0.42 of its shots under 3 units, 0.17 from 3 to 12
and almost none beyond). Beyond that the movement closes distance or the player draws
another weapon. For a visible approaching enemy, the draw considers half
a second of observed motion, capped at two units of advance.

### 3.6 Weapons and ammunition: the decision

**The decision** (`arms::plan`, pure, in order):

1. a blade in hand stays out in lunge range. With the enemy out of reach
   and a better-suited ready gun in the other slot, **switch**;
2. the held gun is dry (no rounds anywhere, or a battery under 2%):
   - the other has something: **switch** (Y);
   - an enemy in view within 2.5 world units: **melee** (run in, B);
   - a useful pickup within 30 world units, not blacklisted: **seek**
     the best near one (each rating point counts as 3 units nearer);
   - an enemy in view: **melee**; else nothing;
3. an empty magazine with a reserve: **reload** (X), in a fight or not;
   on Normal and up, with the other gun loaded and the enemy in its band,
   **switch** instead (faster than a reload);
4. draw a ready sword in lunge range or a shotgun in its close band,
   including where the current gun is also usable. A sniper takes over in
   its band beyond 80% of a rifle's preferred reach. **Switch** when
   the other gun alone can reach or it improves relative range fit by more than 0.3
   (0.6 on Easy). With similarly suitable guns the bot keeps its choice;
5. **reload**: calm (no enemy in view for 1.5 s) and under 35% of a
   magazine with a reserve; or calm, no enemy in view at all and under
   90% (`arms::TACTICAL_RELOAD`: the reload after a fight); or a lull
   (no enemy in view for 0.5 s) and under 60%; or the enemy beyond the
   gun's range and under 35%;
6. no enemy in view, calm or a lull, and no power weapon (rating 4 and
   up) carried: a power weapon within 25 it does not carry: **seek** it;
   holding the better of two guns, **switch** first, so the one dropped
   for it is the worse;
7. calm, the held gun under 20% of all it can carry and the other one too
   (or none): **seek** a useful pickup within 12;
8. a calm bot with a free slot seeks a higher-rated visible gun within
   15 units. A calm bot low on a specific grenade type (zero or one)
   walks over a visible pickup of that type within 12 units.

Seeking walks there along the nav mesh. A weapon of a kind it already
carries is walked over: the game moves its ammunition across by itself
(`0x101740`). One of another kind is taken by holding X on it (within 0.5
world units) for at most 1 s; the dry one in hand is dropped for it.

### 3.6 Weapons and ammunition: trips that fail

**Trips that fail** (`crates/live/bots/src/mind/errand.rs`). A walk to a
weapon may take 3 s plus 20 ticks per world unit of the straight line at
its start. A bot within 1 unit of it for 1.5 s while the weapons decision
still wants it has not got it: a walk-over that gives nothing, a weapon
on a ledge or in a wall. A held X that takes nothing in 1 s ends the same
way. The weapon (its object) is then refused for 30 s and its spot
blacklisted for 10 s, so the bot goes back to fighting and roaming
instead of walking the same trip again.

The presses are Halo 2's Default layout (X reload and action, Y switch,
B melee, A jump, left stick click crouch), which every bot's profile is
given whatever the human uses (`guest::bot_block`, §4.2).

### 3.6 Weapons and ammunition: dual wielding and power items

**Dual wielding and power items.** The following fields were checked against
retail code and Zanzibar tag metadata. The weapons list belongs to the
loaded map, so object handles and tag datums can change between games.

| Field | Where | Trace |
|---|---|---|
| Off-hand eligibility | `D+0x12c`, u32 bits 22 or 23 | `0xcd8a8..0xcd8d7` tests the pickup. `0xcdaee..0xcdb0d` disables both off-hand choices if the primary fails the same test. Actions 5/6 recheck both objects through `0x101640` at `0xcd1a6` and `0xcd1b5`. Bit 23 also suppresses the primary pickup choices. The preferred retail pairs all have bit 22. |
| Weapon identity | `matg=[0x4e034c]`, mulg datum at `matg+0x16c`, universal block at mulg `+8/+0xc`, weapon list at universal `+0x68/+0x6c` | `0x15eb80` searches the eight-byte entries for the weapon datum at `+4`. Zanzibar entries 1, 6, 7, 8 and 9 resolve to SMG, plasma pistol, plasma rifle, brute plasma rifle and needler. Unrecognized definitions skip the pairing policy. |
| Multiplayer weapon type | `D+0x290`, i16 | Getter `0x101280`; the Assault carrier check at `0x24044d` compares with 1. Zanzibar has 0 for an ordinary gun or sword, 1 for a flag or bomb, 2 for a ball and 4 for the Juggernaut item. |
| Placement and nominal interval | object `+0xaf`, signed index into scenario `+0x120/+0x124`, stride `0x90`; spawn XYZ at placement `+0x40` | `0x15ae00` indexes the placement from the object. `0x15ac50` reads signed seconds at placement `+0xe`, then collection `+8` through the `itmc/vehc` reference at placement `+0x58/+0x5c`; zero falls through to 30 seconds. |
| Variant interval | placement class byte `+0x14`; game globals `G+0x211` for class 0, `G+0x208` for classes 1–7 | `0x15b0ef..0x15b134` applies option 1 disabled, 2 half, 3 double; other values keep the interval. |
| Live placement record | `E+0x7dc+8i`, 100 records; `+0` object datum, `+4` i16 seconds remaining, `+6` vehicle abandonment count | `0x15ace0` initializes the table, `0x15b24f` clears the deleted object. `0x15b03f..0x15b1aa` starts, decrements and spawns on zero. A sampled zero means the next service checks the start condition (placement i16 `+0x12`, mode 0 on use/movement, mode 1 on depletion, or a deleted object in either mode). Negative disables it. `0x15b005` increments the separate vehicle count. |
| Clock phase | `[0x510c54]+2` tick rate, `+8` game tick | `0x15aef2..0x15af07` services a placement when its index and the tick have the same remainder modulo tick rate. A deadline is the next service tick plus (remaining seconds minus one) more periods. **[guess]** The bot samples after the service on a coincident tick. Before that service the projected deadline can be one service period late. The engine can also postpone a spawn. |
| Overshield and camouflage | `eqip+0x12c` i16 values 2 and 3; `+0x130` effect seconds | `0x151ef5..0x151f99` converts seconds to ticks and dispatches. Overshield calls `0xd6af0` (unit `+0x10a` bit 4 while charging, shield `+0xf0` above 1 when charged). Camouflage calls `0x152000` with effect index zero, storing ticks at player `+0x198`. |

On Default controls a held Y asks for the secondary interaction at
`0x14f8ff..0x14f911`. `0x151940` selects weapon actions 5/6 for that
hand and 3/4 for the primary interaction (hold X). A short Y release
goes through `0x14f96f` and `0x151aa0` to drop the weapon selected by
`U+0x213`. The fallback action 19 goes through `0xe8de0`, clearing
the hand at `0xe84f1` and removing the inventory entry through `0xce920`.
A consumed hold suppresses that release. Clankers can pair two SMGs,
a plasma rifle or plasma pistol with an SMG, or two needlers on any
difficulty. Each Clanker's name sets its chance of wanting a second gun.
It makes one choice per primary weapon type and compatible ground weapon
each life. Staying near a declined gun or seeing it again keeps that
choice. A respawned pickup gets a fresh roll. An active plasma-pistol
combo or grenade throw keeps priority. In combat they take a pair only
within pickup distance and inside both guns' effective bands. Either hand can charge the pistol.
Only against a group (another enemy within 2.5 of the target), or with
the left gun dry and nothing to load, does the bot trade the left gun for
a grenade: the thrower first solves a safe grenade arc, taps Y and waits
for the left slot to clear before using the grenade trigger. A lone enemy
walking in, out of the left gun's band or lost for a moment keeps both
guns on it.

The placement table has no last-taken timestamp. The host keeps the last
successful take per placement, with these two read-only gameplay observers:

| Event | Source | Checks |
|---|---|---|
| Weapon taken | `0x10ca80`, EAX item, first stack argument new unit. The attach branch sets item `+0x12c` bits 0 and 2, stores the unit at `+0x154` and registers carried objectives at `0x10cb8f`. | Ignore detach (unit -1). Match the salted object to the live placement record and its position to the pad. Resolve the biped's `+0x13c` player and verify the player's `+0x2c` unit. Record game tick `[0x510c54]+8`. |
| Equipment taken | `0x151ea0` dispatches the effect. On success it calls `0xa89c0` with ECX unit and EDX equipment definition, then deletes the item at `0x151fe7`. Rejected pickups skip this call. | At the notification entry verify return PC `0x151fd2`. The pickup function's second stack argument is now at `ESP+0x20`. Resolve the equipment datum and check its definition, placement and player as above. |

Only a bot watching that pad can use the event to date its removal.
The salted object must match and the event tick must fall between its
last sighting on the pad and the first empty scan. If the observer is
unavailable, **[guess]** the first empty scan dates the pickup (up to 30
ticks late). After a gap in sight the bot leaves the time unknown.
For a witnessed disturbance or consumed power-up, the bot estimates the
next eligible service from the event tick, placement index, tick rate
and learned or public interval. The host's live countdown remains available
as an offline trace. The bot cannot tell if the engine postpones the spawn.
The travel policy budgets 12 ticks per world unit
plus a 20-tick lead at Legendary, scaled down at lower skills. Bots fight from a due pad,
and yield to objective or retreat orders. Rockets, both sniper-class
rifles and the live sword qualify by their handling. A visible hand
holding the same salted weapon object identifies the wielder. For a
consumed power-up the success event identifies the player and unit. The
bot remembers a visible recipient. Without an event, a sole visible
player within one world unit with the matching effect is **[guess]**
ownership. Ambiguous fallback observations remain unassigned. A later sighting refreshes the memory; death, a new unit,
a visibly expired effect or ten seconds without confirmation clears it.

### 3.7 Objectives (traced): Fields table and evidence

**Fields.** Traced offline from the XBE with `h2 disasm` and a reading of
its data section (static, no game run). `E` is the game-engine globals,
`G` the game globals, `V` the variant (`G+0x13c`), as in
docs/live/live-gamestate.md. Live telemetry reads none of these yet: it
names the gametype by `V+0x44` and the weapons by tag name.

| Field | Where | Type | Confidence | Evidence |
|---|---|---|---|---|
| game-engine globals | `[0x4e9ae8]` | pointer | confirmed | live-gamestate.md; `0x1574c0` zeroes `0x327` dwords of it |
| engine index | `E+0xc14` | `i32` | confirmed | `0x1574c0` copies `[G+0x180]` (`V+0x44`) there, and writes 0 when the engine has no object or the globals tag lacks its multiplayer block (`0x157578`) |
| engines | `0x55e4d0[index]` | pointer to a static object whose only dword is its vtable | confirmed | filled by `0x378500`: 1 CTF and 9 Assault share one (`[0x470a5c]` = `0x47f788`, vtable `0x459d18`); 2 Slayer `0x47fc80`, `0x45c6d8`; 3 Oddball `0x47fc84`, `0x45c7a8`; 4 KotH `0x47fc88`, `0x45c878`; 7 Juggernaut `0x47d8f0`, `0x453d38`; 8 Territories `0x47fc8c`, `0x45c948`. 51 thiscall slots each; the slot map is in [notes/2026-11-21-engine-vtables.md](../../engine/notes/2026-11-21-engine-vtables.md) |
| objective object count | `E+0xc18` | `i32`, at most 8 | confirmed | `0x15e050` (register) bumps it after `cmp eax,8; jae`; `0x15e130` (unregister) moves the last record down and drops it |
| objective records | `E+0xc1c`, `0x10` each | | confirmed | `0x15e050`: `shl eax,4; lea edi,[eax+ebx+0C1Ch]` |
| record: the object | `+0` | `u32` datum | confirmed | `0x15e050` stores its first argument; every lookup (`0x15e180`, `0x15e2b4`, `0x15e327`) compares it |
| record: its team | `+4` | `i16` | confirmed | `0x15e050`'s second argument; the CTF flag spawn `0x2422d0` passes the flag's team (8 is the neutral one), Oddball's `0x2bc1f0` the ball's number; `0x15e410` finds a record by it; also written to the weapon's `+0x17e` |
| record: carrying unit | `+8` | `u32` datum, -1 none | confirmed | `0x15e250` (picked up) writes its second argument; `0x15e460` (dropped) writes -1 |
| record: carrying player | `+0xc` | `u32` datum, -1 none | confirmed | `0x15e250` copies the unit's `+0x13c` (its player, live-gamestate.md); `0x15e300` refreshes it; `0x15e460` writes -1 |
| carried flag on the weapon | weapon `+0x16c` bit 7 (bit 6: registered) | `u8` | confirmed | `0x15e2d4` `or 80h`, `0x15e49c` `and 7Fh`; `0x15e0a5` `or 40h`, `0x15e1b6` `and 0BFh` |
| who registers | | | confirmed | callers of `0x15e050`: CTF and Assault (`0x2423d9`), Oddball (`0x2bc26c`), `0x2c0a7d` (Territories registers the marker object with its territory index), and the network and object-create paths `0xa228f`, `0xd0ab5` |
| team designators in use | `E+8` | `u16` mask | confirmed | `0x158990` tests `1 << i` against it for `i` 0 to 8 |
| each designator's team | `E+0x10 + 2i` | `i16`, 9 | confirmed | `0x157790` builds the active-team mask at `E+6` and calls `0x157670` at `0x157a30`. The latter clears the mapping to -1, pairs a designator from `E+4` with an active team, updates masks `E+8/E+0xa` and writes the entry at `0x15770b..0x15771a`. `0x158990` performs the inverse lookup |
| netgame flags | `[[0x4e0350]+0x11c]`, count `[..+0x118]`, `0x20` each | | confirmed | `0x243b69`: `mov ecx,[4E0350h]; mov edx,[ecx+11Ch]; shl ...,5` |
| flag type, team, identifier | `+0x10`, `+0x12`, `+0x14` | `u16` each | confirmed | `0x19ec40`, the engine's flag query, compares the three with its arguments (-1 matches any) |
| flag game flags | `+0x16` | `u16` | likely | `0x243b8f`: bits 1, 2 and 4 against `V+0x104` (`G+0x240`), the CTF variant's flag setting |
| type 0, 1: CTF flag spawn, flag return | | | confirmed | `0x243b10(spawn?)`: type `al ? 0 : 1` for CTF |
| type 2, 3: bomb spawn, bomb return (the plate) | | | confirmed | as above, `al ? 2 : 3` when the engine is 9 |
| type 4: ball spawn | | | confirmed | Oddball init `0x2bc2e0..0x2bc32a` queries type 4, designator 8, any identifier and at most three indices into its globals at `+0xc` |
| type 11 to 18: hill `0` to `7`'s corners | | | confirmed | KotH's init `0x2bd140` collects the types `0xb..0x12` it finds, as `type - 11`, into `0x5092f0` (count `0x5092e8`); `0x2bdc70` asks `0x19ec40` for type `0xb + hill` |
| CTF and Assault globals | `[0x51ec80]` (= `E+0xfc`) | pointer | confirmed | `0x240160` zeroes `0x82` dwords from `E+0xfc` and stores the address there |
| ...: per-designator flag spawn, return | `+0x02 + 2i`, `+0x14 + 2i` | `i16` netgame flag index | confirmed | `0x240160`'s loop `0x2401a0`; `0x2422d0` takes `+0x02` at `0x158990(team)` |
| ...: neutral flag or bomb spawn | `+0x38` | `i32` index | confirmed | `0x2401e6`; `0x2422d0` for team 8 |
| ...: per-team state bits | `+0x170 + team` | `u8`, 9 | confirmed | Mask 8 marks armed at `0x2405a0`. Mask 2 marks planted at `0x242d25` after `0x15e360` releases the bomb at the scoring plate. |
| ...: per-team timer | `+0x1a0 + 2·team` | `i16`, 9 | confirmed | the Assault arming count (`0x240575..0x24058d`); -1 at a flag spawn (`0x2422e6`) |
| One-bomb defending team | `A+0x1f4`, A is the CTF/Assault globals | i32 game team | confirmed | `0x2402a4` selects it when `G+0x240 == 1`. `0x243a80` spawns the bomb for the other side. The scoring check at `0x242fc6..0x242fd6` requires the plate team to equal this field and differ from the carrier team. |
| Bomb team and active plates | record `+4` / weapon `+0x17e`; `A+0x14+2d`, `A+0x26+2d` | i16 team or neutral 8; two i16 flag indices per designator | confirmed | `0x2422d0..0x2423d9` uses the spawn team as the registration argument. Both the arming-volume check `0x244680` and scoring check `0x242ef0` consult the two selected plates. `0x242f65..0x242f7f` requires the game team in the `E+0xc` mask. A score needs the carrier within one world unit of a qualifying plate. |
| Oddball globals | `[0x51ecc4]` | pointer | confirmed | the only global of Oddball's functions besides the shared ones |
| ...: each ball's spawn | `+0xc + 4·ball` | `i32` netgame flag index | confirmed | `0x2bc1bb` |
| King of the Hill globals | `[0x51ecc8]` (= `E+0xfc`) | pointer | confirmed | `0x2bd157` |
| ...: the hill's corners | `+0x00`, `4` each, count `+0x60` | `i32` netgame flag index | confirmed | `0x2bdc96` asks for at most 16 into it; the count is made even and at least 4 is needed (`0x2bdc9b..0x2bdca7`) |
| ...: the hill in play | `+0x1a0` | `i32`, 0 to 7 | confirmed | `0x2bd20b` sets 0 at start; `0x2bd77f` moves it on (`0x2bd0b0` picks the next), when `+0x1a4` runs out |
| ...: ticks until the hill moves | `+0x1a4` | `i32` | confirmed | `0x2bd747..0x2bd772`: counted down, reset to `V+0xf4` (`G+0x230`) seconds × the tick rate; never when that is 0 |
| ...: players in the hill | `+0x1a8` | `u16` mask by player index | confirmed | `0x2be880` checks the live unit against the current hill bounds and geometry. `0x2bd304` increments player `+0x1b8` inside and clears it outside. `0x2bd6d0..0x2bd71d` builds the scoring mask from nonzero counters before the hill-move update |
| Territories globals and markers | `T=[0x51eccc]=E+0xfc`, `T+0x114` i16 count, `T+0x60+2i` i16 flag index | up to eight | confirmed | `0x2bf310` initializes T. `0x2bf5b0` queries netgame type 10, any designator, identifier i; `0x2bf61d` stores the first matching flag. Additional markers size the capture volume. |
| Territory owner | `T+0x70+4i` | salted player datum | confirmed | `0x2c075d` writes it; `0x2bfa3f` checks its salt. `0x2c0006..0x2c0047` compares team through player `+0xc0`. A stale owner gives no team. |
| Territory occupants | `T+0x95+8p` | signed byte by player slot | confirmed | `0x2bfc40` tests the unit against the marker and the radius and height values at `T+0/+0x20/+0x40+4i`. `0x2bfd83` stores the territory index on entry, `0x2bfe1d` stores -1 on exit. Multiple players can occupy one site. |
| Juggernaut identity | `J=[0x510c9c]=E+0xfc`, `J+0` | u16 player-slot mask | confirmed | Engine 7 initializes J at `0x1921d0`. `0x1922db..0x192339` tests the victim and transfers the role to the killer through `0x192970`. |
| the flag, ball, bomb tags | globals tag (`[0x4e034c]`) `+0x16c` the multiplayer block: `+0xc` → `+4` flag, `+0xc` ball, `+0x3c` bomb | `u32` tag datum | confirmed | `0x242261` (bomb, engine 9) and `0x24228b` (flag) create the object from it; Oddball `0x2bc231` the ball |

### 3.7 How the bot reads them: per-engine read detail

**How the bot reads them** (`objective::read`, from `guest::perceive` once
it has found itself):

- the gametype is `E+0xc14`; Slayer and unsupported engines read as no objective;
- each record gives an item: the object, its team, and the carrier's
  player index (as `Seen::id`). A carried item is where its carrier's unit
  is; otherwise where the object is. A record whose object is gone is
  skipped;
- the netgame flags the gametype uses become spots (CTF: types 0 and 1;
  Assault: 2 and 3; Oddball: 4), each with the game team its designator
  stands for (`E+8`, `E+0x10`). An unbound designator has no team;
- in King of the Hill the active corner list supplies a destination.
  The live occupancy mask determines who is scoring. Geometry remains an
  approximation for loose pickups and observations without a scoring mask.

### 3.7 Without the engine's globals: per-gametype list

- Oddball with `[0x51ecc4]` zero: the neutral type-4 spawns (designator
  8, at most three). The balls themselves come from the objective
  records, which a script's `spawn_objective` fills;
- King of the Hill with `[0x51ecc8]` zero: the lowest-numbered hill's
  corners (types 11 to 18). Who is in it is judged by the corners;
- Territories with `[0x51eccc]` zero: `State::Markers`, for each
  identifier 0 to 7 the first type-10 flag with it (`+0x14`), as
  `0x2bf5b0` picks them. No owner is known; a player within
  `TERRITORY_REACH` (3 units XY, 2 in height) of a marker stands in it.

### 3.7 Roles: first copy

**Roles** (`objective::role_of`). Every bot sorts its living teammates
and itself by player index; its rank and the team's size give its role,
so all of a team's bots agree with nothing said between them. One alone
attacks; of two, the higher index defends; from three, a third (rounded
down) defend, the highest ranks. Humans count in the ranks: the bots do
not know who is a bot.

### 3.7 Belief: spawn and missing rules

- a flag or bomb never seen (or forgotten) is at its spawn (CTF types 0;
  Assault type 2, a spawn with no team being the neutral bomb's). A spawn
  in view within 15 with nothing on it makes that team's flag or bomb
  missing;
- a missing one stays missing until it is seen or 20 s pass (a return or a
  capture puts it back at its spawn);
- a destination the stuck machine repaths at 3 times (section 3.2), or a
  loose item the bot stands on for 3 s without taking it, is given up for
  15 s (`Belief::pursue`): the item drops out of the plan, and a return
  point or plate drops out while another of its team remains.

So from the spawn the attackers head for the enemy base whether or not
the enemy flag is in view, and a bot looking at an empty spawn plays on
rather than waiting.

### 3.7 Roles: neutral leaning and round-1 names

A neutral leaning (all 0.5) gives the ranked plan unchanged. Of the eight
round-1 names at Legendary, three escort and five grab.

### 3.7 Oddball: full step list

**Oddball** (`zones::ball`):

1. carrying, **keep** it: cover away from the threat, melee at arm's
   length;
2. a loose ball: the two of the team nearest it (`RUSHERS`) **go** for
   it, the rest **hold** within `BALL_COVER` (6 units) of it. In a fight
   the runners keep walking onto it, shooting on the way, and hold X on
   it; in free-for-all everyone runs;
3. an enemy has it: **hunt** the carrier as a group. A hunter between 6
   and 18 units from it (`SHOWDOWN`, `STAGE`), with no teammate within 8
   (`GROUP`) and one coming more than a lead behind it (6 to 20 units, the
   patient the shorter), holds where it is (3 units) until they close
   up; a teammate more than twice `STAGE` further off (respawning) is not
   waited for. The carrier is the hunters' first target, and since it
   cannot shoot it does not count as a gun when a hunter weighs whether
   it is outnumbered;
4. a teammate has it: **escort** as a screen, `SCREEN` (3.5 units) out
   from the carrier towards the nearest enemy it knows within 25, the
   escorts fanned out by index (0, ±0.6, ±1.2 rad) so each stands in its
   own spot; with no threat, by the carrier. An escort's first target is
   an enemy within 12 of the carrier;
5. between balls, the closest neutral type-4 spawn.

### 3.7 King of the Hill: full detail

**King of the Hill** (`zones::hill`). Approach the current corners'
centre. In the hill (the engine's mask, else the corners), **hold** the
centre within a share of its radius, 0.4 to 0.65 by caution
(`HILL_KEEP`, `HILL_KEEP_CAUTION`), so the cautious play its edge; where
the mask counts the bot in but the corners do not, hold the spot. Out of
it with two teammates already holding it and nobody contesting, a bot of
caution 0.6 or more (`EDGE_CAUTION`) covers it from just outside, within
`1 + caution` radii of the centre. Everyone else goes in, and an enemy in
the hill sends the cautious in too, to clear it; an enemy standing in it
is the preferred target. Holding the hill, a focused bot stays in a fight
one enemy longer before backing off. Moving the hill rebuilds its
corners and the bot follows the next scoring mask.

### 3.7 Assault: arming, planting and blast detail

**Assault.** Take a friendly or neutral bomb to an active enemy plate.
In one-bomb, only the designated defender's plates score. Carrying the
bomb on foot inside the arming volume advances the counter toward
`G+0x244` seconds times tick rate. Outside, it falls by three per tick
to a floor of -1. Stay on the plate through arming; when armed, close to
score. The engine releases the planted bomb, records the planter at
`A+0x17c+4·bombTeam` and sets the planted bit; the team's point comes when
it goes off, about 3 s later, killing whoever stands near (round 1 on
Midship: the planter and an escort 3 units away). Escorts cover the
arming from the plate. A planted bomb is nobody's target: any goal whose
point lies within `BLAST_CLEAR` (6 world units, XY) of it, or a bot with
no goal standing there, becomes a **hold** within 1.5 of a spot 7.5 out
along the line from the bomb to the bot (`objective::clear_of`).
Defenders guard the plate within the guard radius or intercept the enemy
carrier. An ordinary dropped friendly bomb can be recovered by the
nearest teammate; a loose one elsewhere goes to the nearest attacker, the
escorting names covering it.

### 3.7 Territories: ranking detail

**Territories** (`zones::territory`). A team spreads over the sites.
Every bot ranks each pair of teammate and site the same way: a friendly
site with an enemy in it, or one the teammate stands in and is taking,
first; then a site not ours; then ours; nearer first. Going down that
list, each site takes one teammate, two where an enemy stands in it, so
two relieve a site under pressure while the rest take the others; a bot
the sites have no room for goes where it would alone. In its site it
**holds** within `TERRITORY_KEEP` (1.5 units) of the marker, fighting from
there; a site taken drops to the bottom of the list, so its taker moves
on to the next unless nothing else is left. Holding a territory, a
focused bot stays in a fight one enemy longer. FFA ownership follows the
player slot and every bot plays alone.

### 3.7 Juggernaut: detail

**Juggernaut.** Every other player sees the Juggernaut by its waypoint,
so a hunter's goal is the Juggernaut wherever it is: it walks to it with
no one in view and fights it on sight. Hunters ignore fellow hunters. The
Juggernaut has no marks on the others: it hunts the nearest hunter in its
own view, and roams otherwise (`juggernaut_quarry`,
`crates/live/bots/src/mind/strategy.rs`). It never withdraws, runs at a
visible hunter while firing, then melees up close. Movement bonuses come
from the game variant.

### 3.7 Picking up: address trace

**Picking up.** Approach the chosen item and hold X while it remains
loose within pickup range: 0.5 world units in XY and 1 in height
(`TAKE_HEIGHT`), as an item on a step or lip sits off the unit's origin.
A goal counts as reached in XY within its radius (at least
`nav::REACHED`) and in height within its radius (at least 1), so a bot
does not walk on at an item it is standing on. `0x1517ec..0x1518d6` gives a nonzero
multiplayer weapon type the contextual interaction kind 7. The held-X
path accepts kinds 1 and 7 at `0x15195c` and requests primary pickup.
There is also a proximity path (`0x15098f`) whose primary pickup is
gated by inventory at `0x150dd0`. In Assault the pickup predicate
`0x2415d3..0x241611` rejects a planted bomb or a non-neutral bomb
belonging to another team. A bot returning its own CTF flag just
touches it. Once possession changes the carrier releases X.

### 3.8 Glitches: the tricks table

**The tricks.** Nominal requested presses at 30 ticks a second:

| Trick | Presses (tick: button) | When | Source |
|---|---|---|---|
| BXR | 0 B, 5 X (held), 7 R | in melee reach with a gun whose magazine is full or has no reserve | [Halopedia, BXR](https://www.halopedia.org/BXR): B, then X once the melee lands (it cancels the melee's recovery), then R; full magazine only, else X reloads; [Bungie.net forum](https://www.bungie.net/en/Forums/Post/414304): "hold X while pressing R" |
| BYYR | 0 B, 5 Y, 8 Y, 10 R | the same with a part-empty magazine and a second gun | Halopedia, BXR: "press Y twice instead of pressing X once" |
| BXB | 0 B, 5 X, 7 B | the same with nothing in the magazine and nothing to load | Bungie.net forum: "like BXR, but after X press B again" |
| Double shot | 0 R, 4 R (held), 6 X | a battle rifle, 6 rounds loaded and a reserve, the target in its range | Bungie.net forum: RRX, "holding the second R while pressing X"; "on a last clip it just shoots twice" |
| Quad shot | a double shot, 9 Y, 12 Y, then a double shot from 15 | as the double shot, 12 rounds and a second gun | Bungie.net forum: "RRX, YY, RRX"; YY swaps back before the reload |
| Sword flying | 0 Y, 1 X, 3 R, 11 A | the sword the other weapon, the gun in hand on target (its reticle red) past the sword's lunge, up to 20 units | [Halopedia, Sword flying](https://www.halopedia.org/Sword_flying): Y, X, RT "quickly in that order", the reticle red on the gun first; the window is 0.066 s at 30 fps (two ticks); A in the lunge goes higher |
| Crouch jump | 0 A, 7 crouch (10 ticks) | roaming, now and then | superbounce.md, "The crouch height term": a crouch change in the air lifts the feet |
| Super bounce | prime at a crouch spot (crouch, let go under the low ceiling), then at a spot: stand still, A, hands off but a light push toward +y, land; again from a nudged place | roaming, a spot within 60 units | superbounce.md (traced; the spot's table is `glitches::spots`, the scan `spots`) |

### 3.8 Glitches: skill and pass-rate table

**Skill.** Tricks request the same sequence at every skill. The pad
pipeline scatters its edges (§3.4), so a miss is an ordinary melee,
switch, reload, shot or jump. The trigger's charge and release in the
noob combo and short pulls on other weapons go through the same pipeline.
Skill also scales how often it tries. Easy takes 40% of the setting's
chance, Normal 70%, Heroic 90% and Legendary all of it.

These are timing-window pass rates from 4,000 seeded controller-model
trials per skill and technique, with the right stick at rest. The
criterion is that the delivered presses arrive in order and within a
tick of each nominal gap, keeping required holds such as X at R for BXR.
A melee cancel or second BR pull can't precede its nominal opening. The
sword fly needs X one or two ticks after Y. The plasma overcharge row
checks the nominal 24-tick hold. These rates measure pad timing. Whether
the retail engine accepts an attempt depends on the weapon, target,
movement and host behavior. Super-bounce landings need a physics run.

| Delivered sequence | Easy | Normal | Heroic | Legendary |
|---|---:|---:|---:|---:|
| BXR | 80.5% | 85.7% | 89.3% | 93.4% |
| BYYR | 66.4% | 74.8% | 80.5% | 87.2% |
| BXB | 79.7% | 85.3% | 89.0% | 93.2% |
| Double shot | 80.4% | 85.6% | 89.2% | 93.4% |
| Quad shot | 49.7% | 61.5% | 69.8% | 80.3% |
| Sword fly | 56.6% | 65.1% | 73.0% | 81.2% |
| Crouch jump | 98.0% | 99.0% | 99.5% | 99.8% |
| Plasma overcharge hold | 84.6% | 87.7% | 90.6% | 93.7% |

### 3.8 Glitches: super bounce spots table and standing procedure

**The super bounce spots** (`glitches::spots::SPOTS`) hold only measured
spots. Each spot has a map, the place to stand, a spread to nudge within
between tries, a drift direction, a source and, for a spot a standing
jump can't reach, the takeoff of the running jump onto it
(`Bounce::takeoff`) and where its traced bounces landed, which a route
aims at (`Bounce::landing`, the site's `Site::aim`). There is one:

| Map | Stand at (x, y, z) | Drift | Takeoff | Aim | Source |
|---|---|---|---|---|---|
| `ascension` | (−10.62, 3.88, −3.19), nudged up to 0.04 | +y | (−8.4, 2.5, −0.76) | (−10.45, 3.985, −3.19) | superbounce.md: the crease where the ramp meets the floor; every traced bounce fell onto it at −5.26 to −5.40 from the recipe's running jump off the upper level, a 2.43 drop, landing on x −10.2 to −10.7, y 3.93 to 4.04 (the aim is the middle, which the takeoff heads at along the recipe's (−0.8, 0.6)); the launch is a pinch between faces at y 4.12 and 3.94 |

A bounce needs the player primed first: a forced crouch leaves the
level pair word at -3 and only a landing with it at -3 bounces
([superbounce.md](../superbounce.md), "The pair word"), and the landing
must come in fast enough: a standing jump lands at about −3 units/s and
never bounces on Ascension's patch, which needs −5.4. The measured spot
is therefore a drop from its takeoff (`spots::Site::measured`: fall 2.43,
`spots::MEASURED_FALL`), the recipe's running jump (below). The bot
primes before it goes to a spot (below). On a standing site it walks to
0.8 units short of the spot, steps onto it, stands still for 4 ticks,
jumps with the stick let go, and pushes the stick toward the drift in
the air. Not every landing bounces, so it tries again from a nudged
place. It knows it bounced when it rises 0.4 units again after starting
to fall: a plain landing never does. Community
guides ([Halopedia, Super bouncing](https://www.halopedia.org/Super_Bouncing))
give spots on most maps by landmark only. A spot goes in the table only
once a game run has found and traced it; the scan below finds the rest.

### Superbounce routes: scan files, verified spots, learned knobs

`bots::spots` holds what a bot knows of its map's spots: the measured
ones first, then the scan of the map's collision geometry
(`crates/live/superbounce`: **bounce** spots, narrow slots a primed
landing launches out of, and **crouch** spots, where a standing player is
forced into a crouch). The nav worker scans a map when it builds or
restores the nav mesh (`autonav::Worker::keep_spots`) and saves the
result beside it as `<map>.spots` (after the mesh is published and saved,
and the scan is caught if it panics; a failed scan leaves an empty
stamped file so it is not repeated every launch), stamped with the nav generator, the
scan version (`spots::SCAN`) and the map's fingerprint. The worker also
reads the map's floor polygons and publishes them in memory
(`spots::read_floors`, `spots::publish_floors`) for the takeoff gate; they
are not another cache file. A scanned bounce spot becomes a
`glitches::Bounce` (source `scan`, spread 0.03) with its launch speed
(`spots::Site`); one at a measured spot's place is dropped.
A site with no fall (0.5 or less) and no takeoff is jumped onto in place;
one that needs a fall is dropped onto from its takeoff or a ledge
(below).

**Verified spots.** The map's verified file (`<data>/bots/<map>.verified`,
written by `superbounce.verify`, [superbounce.md](../superbounce.md),
"Verifying spots") is read beside the scan (`spots::verified_for`,
cached; `spots::store_verified` merges a batch in and bumps
`spots::generation`, so a bot's spots are rebuilt). A proven spot is
looked at first and scores 1; a failed one, scanned or measured, is left
out. A proven crouch spot carries where its working trial started and
its way in (`Crouch::start`); a proven strip carries the working landing
point (`Site::aim`) and drop height (`Site::height`). The game sets the
file's directory when it starts (`spots::set_verified_dir`, from
`bots_nav_auto`).

**Learned knobs.** The map's learned file (`<data>/bots/<map>.learned`,
written by a sim's training run, [bounce-search.md](../bounce-search.md)) is
read from the same directory (`spots::learned_for`, cached;
`spots::store_learned` replaces a site's line after a training round and
bumps `spots::generation`). `Known::for_map` gives each site its line's
knobs (`Site::knobs`, by `Site::learned`: the knobs of an `open`,
`partial` or `learned` line, `superbounce::learned::knobs_for`) and the
line's state (`Site::line`). A site whose line is `dead` is left out, as a
failed one is, unless it is proven (measured, or proven by a verified
entry); a `blocked` line changes nothing. A knob no line sets is the
route's own: `bounce_route::flown(site, from, tune)` makes each of a
site's numbers (`Flown`: the `how`, the target, the heading off the
launch-to-target line by `yaw`, the run-up `back`, the run's stick, the
jump's `early`, the air stick's `gain` and let-go distance, a standing
jump's `drift` and `still`) from the knob, else the expression below, and
the drop, its air steering and the standing bounce read `Flown` alone for
them. A line's `fx fy fz` is the launch point ahead of the takeoff and the
launch floor (`bounce_route::launch_from`), its `ax ay` the target ahead
of the verified aim and the stand (`Site::target`; `Site::own_target` is
the target without the line), and its `how` says whether the site is a
standing jump or a drop (`Site::standing`). A site with no line plays
exactly as it would without the file.

### Superbounce routes: the route plan in detail

`bounce_route::plan` plans a route from the bot to a point, purely, from
a walk-length function over the nav mesh:

1. walk to the crouch spot's start: outside the lip on the approach,
   0.35 units from `stand` against the way out, or where its verified
   trial started;
2. **prime** (`glitches::prime::PrimeRun::at`), the recipe: crouch and
   stand still 6 ticks, crouch-walk in under the lip to 0.06 past
   `stand`, hold 12 ticks, let go (still pushing in, 6 ticks), then walk
   out along the way out. Done when the word reads -3 on the way out.
   A word that reads something else for 30 ticks out, or no arrival,
   tries again from the start, along the approach turned 25 degrees and
   then −25 (3 tries, the first along a verified way in), then fails
   (`prime_failed`). When the word cannot be read it is assumed primed
   0.5 units out. A crouch spot with no way out known primes in place:
   crouch 12 ticks, let go, stand up;
3. for a site with no fall, walk to 0.8 units behind it, never
   crouching, then the bounce run (`glitches::BounceRun`) until it
   launches. The run settles onto the stand and jumps; settling that
   moves the body less than 0.002 units a tick for 12 ticks
   (`glitches::SETTLE_STILL_TICKS`) jumps from where it is, since a
   primed body that stands still 38 ticks sleeps and wakes with its word
   at 0 ([superbounce.md](../superbounce.md), "Charge"). For a site that needs a fall, walk to its **launch
   point** (`bounce_route::launch_from`): its takeoff when it has one,
   else its **launch floor** (`bounce_route::launch_floor`: the nav
   mesh's point nearest the site's aim sideways, within 3 units, whose
   takeoff gives a 3.45 apex-to-floor drop within the 3.35 to 3.60
   window; a run-off from that height, else a running jump from a jump's
   rise lower), then **drop**. An old measured fall height does not
   choose the floor; an explicit measured or learned takeoff still wins.
   How a drop leaves the floor comes from the flight model
   (`bounce_route::drop_takeoff`): gravity 4.15, a jump 3.08 up, the
   ground speed of the run's stick (`bounce_route::ground_speed`: the
   drill runs' map, nothing in the dead zone, about 1.05 units/s at 0.6
   to 0.85 of stick, 2.0 at 1, **[needs more points]**), and what the air
   stick can do over the fall (`bounce_route::air_reach`: it drives the
   sideways speed toward the stick times 1.2 at 1.05 units/s², so it can
   brake a flight to a stop and push one to 1.2 at most). A **run-off**
   at full stick (no jump) is taken when it comes down at least as fast
   as the site needs (`need`: a fall from rest lands at `sqrt(2 g h)`)
   and the air stick can land it on the aim; else a run-off at the
   tuning's slower `run_off_speed` that can; else a **running jump** at
   full stick, then at `run_off_speed`, that comes down fast enough and
   gets there; failing all, of those fast enough the one whose aim is
   least out of reach (a learned `how` picks the run-off or the jump, and
   `run` the stick). Once lined up, before the run or jump,
   `bounce_route::judge_takeoff` flies the takeoff and asks
   `superbounce::rule::predict` for its outcome. The aim is at most 0.05
   from the nearest floor-polygon edge (moved to 0.02 from it when
   needed), with 0.3 units/s held across it; a takeoff the rule does not
   call a bounce ends as `takeoff_rejected`. Training tries are not
   gated, so they can measure the failures. Either way the drop lines up
   0.6 behind the launch point (a learned `back`; 45 ticks at most) and
   runs at the aim (turned
   by a learned `yaw`) at that stick, steered
   back onto the line through the launch point (60 ticks to leave the
   floor, else `no_drop`). A running jump presses A 0.05 short of the
   launch point plus the ground the body covers in the 5 ticks it takes
   to act on the press (`bounce_route::INPUT_LAG`), holds it 3 ticks and
   keeps running until the body leaves the floor. Ascension's measured
   spot is a full-stick running jump: a run-off from its 2.43 lands at
   4.49, under the 5.4 the traced bounces came in at, and the jump's
   flight, 2 units/s off the floor, reaches 1.9 to 2.8 against the aim's
   2.5. In the air the stick steers where the landing comes down
   (`bounce_route::air_stick`): with the sideways speed held, from where
   the body is once the stick acts and the fall left then, against the
   aim, it asks for the speed that, reached at 1.05 units/s² and then
   held, lands on the aim, 1.2 times the change that is just enough
   (`air_gain`, or a learned `gain`); within 0.03 (`AIR_DEAD`, or a
   learned `letgo`) it holds the speed, and a landing more than
   0.3 past the aim, or a miss it can't make up in the time, gets the
   full stick. Every stick is at least 0.2 or none: under the move dead
   zone (0.15) the body doesn't move. A measured spot aims at its traced
   landing even once a verified drop says where one bounce landed
   (`Site::verified`). It launched when it rises 0.4 over
   its lowest point after the top; a landing that did not launch walks
   back to the launch point and drops again (3 drops, then `no_bounce`);
4. fly: steer toward the goal at full stick until down
   (vertical speed under 0.3 for 2 ticks after the top);
5. walk the rest.

### Superbounce routes: touchdowns and flights

A touchdown during a bounce or a drop is kept (`bounce_route::Touchdown`:
the tick, the origin before, the speeds in and out, how far outside the
slot, the word) and named as the HUD's landings are: `bounce`,
`not_primed`, `missed`, `too_slow` or `no_bounce`. Each jump or drop is a
**flight** (`bounce_route::Flight`, `OrderRun::flights`, the last 8):
from the tick the step goes airborne (the jump pressed, or a run-off
leaving the floor), where it left (for a jump, where A was pressed,
about a quarter of a unit before the body leaves the floor at full
stick), the ground speed then, the best vertical speed in the 10 ticks
after, the word on that tick, the stick
asked in the air (mean, most, and the ticks let go), and the touchdown
it ended in with its result (`no_landing` when none was seen).

### Superbounce routes: Help and Fun modes

The plan is the best bounce under the asker's mode. **Help** (the
default) takes a bounce only where it reaches the goal and the plain walk
cannot (no path, such as a ledge above a jump), or where the whole route,
with 8 units for the priming, is 4 units shorter than the walk. **Fun**
takes, of the spots within 60 units of walking, the highest launch (the
cheaper on a tie) and then goes on to the goal. With no bounce that
qualifies it is the plain walk. A bounce needs a crouch spot to prime at,
unless the bot's pair word reads -3 already.

### Superbounce routes: walks off the mesh and lost walks

**Walks to spots off the mesh.** A scanned stand sits on a seam or an
edge, so the point a walk goes to (a prime's start, a bounce's
approach, a drop's launch floor) may be just off the nav mesh, or a
little under the floor beside it. A walk (`nav::RouteWalk`) first puts
its goal on the mesh's floor where a polygon under it has one from 0.3
under it to a standing height (0.725) over it (`nav::floor_goal`): an
approach 0.8 behind a crease, on the ramp rising from it, keeps the
crease's height otherwise, under the ramp. It has arrived within its
tolerance sideways and 0.6 units up or down (`nav::route::ARRIVE_Z`).
A goal off the mesh within 1.5 units (`nav::TAIL_REACH`) of the
corridor's end, 0.5 units over it to 0.8 under it, with nothing solid
between them at knee height and the collision's floor under the line
every 0.25 units (within 0.3 over it to 0.5 under it, so never off a
ledge or a sill), is walked to by a **last stretch**: the corridor to
its end, then straight at the goal, slowing as it nears it. A last
stretch that gets no 0.05 units nearer in 20 ticks (under the 38 still
ticks a primed body sleeps after) ends a walk to a prime's start or a
bounce's approach where the body got to (the prime and the bounce steer
the rest themselves), and fails a walk to a goal (`tail_stuck`). A goal
farther off is `goal_off_mesh`. Within 0.5 units of its goal on its
last leg a walk does not stop for the standing capsule's headroom
ahead (`nav::route::END_HEADROOM`): the probes reach past the goal, under
the lip a crouch spot's start stands beside. A walk to a crouch spot's
start (`RouteWalk::ducking`) does not stop for headroom at all: under a
ceiling the standing body does not fit it crouches and walks on, since
the prime crouches next anyway. Other walks stop there, standing.

**A lost walk.** When the follower gives a walk up, the route plans the
walk again from where the bot is, 3 times a step
(`bounce_route::WALK_REPLANS`, each logged with the check), then fails
naming the check and the leg (`bounce_route::lost_on`):
`path_lost_<check>_<leg>`. The check is the follower's: `stuck` (no
nearer in 30 ticks, a jump, and no nearer in 30 more), `fell` (its
waypoint above it past 1.2 units plus the distance), `jump` (stuck at a
jump or drop link), `link` (a lift, teleporter, elevator or crouch jump
that did not carry it) or `ended` (no waypoint left). A walk lost at a
jump or drop link (`jump`: a rail or lip the mesh lacks) is planned
again around it: every later walk of the route skips a jump, drop or
crouch-jump link within 0.5 units sideways (`nav::route::AVOID_REACH`)
and 1 unit in height of where it gave up (`RouteWalk::failed_link`,
`RouteWalk::new_avoiding`), so it takes another way down or fails
`no_path` rather than pushing at the same link. The leg is
`crouch` (to a crouch spot's start), `approach` (to a bounce's approach
or a drop's launch floor) or `nav` (any other walk). The planner measures walks the way a walk is
taken (`bounce_route::path_length`, `nav::walk_length`): no teleporter
or crouch-jump link, and a goal off the mesh only within a last
stretch's reach, so a plan holds no walk that fails `no_path` or
`goal_off_mesh` when it is walked.

### Superbounce routes: plan bounds and the arc

A map can have hundreds of spots, so a plan is bounded: spots farther
than the limit in a straight line (the walk it must beat in Help, 60
units in Fun) are dropped before any path query; of the rest the 4 crouch
spots nearest the bot and the 8 likeliest sites are looked at; each crouch
spot's walk from the bot is asked once, each site is paired with the
crouch spot nearest it; and a plan makes at most 32 path queries
(`bounce_route::MAX_QUERIES`). A show-off plans for the one nearest site
it has not given up on (`Stuck::blocked`), and gives it up when the route
fails.

The arc: up at the site's launch speed, gravity 5.2 units a second
squared (`nav::route`), sideways at 2.5 units a second in the air. The
flight reaches a goal when the horizontal gap is within the time it takes
to come down through the goal's height, times that speed. The air speed
is a **[guess]**; the launch speeds are the scan's estimates and, for the
measured spot, the 12.3 of superbounce.md.

### Superbounce routes: priming state and orders

**Priming state.** The brain asks for the player's level pair word
through a reader the host installs (`bots::set_pair_reader`: the unit's
datum in, the word out; the game's reader is `live_telemetry::havok_agents`'s
`fixed_pair_word`). It is read only while a route is asked for or under
way (`Mind::wants_pair`), not on every sample. With it a priming ends when the word reads
-3, a word that reads anything else on the ground after priming
(walking, lining up or running off a drop, or settling on a bounce's
stand before the jump) sends the bot back to the crouch spot (twice at
most), and a priming whose
word never reaches -3 fails (`prime_failed`). Without it the bot assumes
it is primed once the stand-up has had its 12 ticks; `bots.bounce_status`
says which (`primed_read`). `Mind::primed` is true from the priming to
the bounce, and while a route runs its pad has no crouch button.

**Orders.** `Mind::order_bounce(to, mode)` (through
`PadSource::order_bounce`, `Phantoms::order_bounce`) asks for a route;
it is planned at the next sample from where the bot stands and played as
the bot's whole pad (`mind::order`, through `RoutePad`, as an
`input.route` is; a death or a change of map ends it, recorded as `died`
or `map_changed`): the bot fights and picks up nothing until it is over
(the agent API's `bots.bounce_to`, docs/runtime/agent-api.md). A route
gives up after 200 s. `bots.bounce_status` gives its step and the step's
phase and try, the last touchdown, and a log of each step and phase
change (the last 64). `PadSource::hold_pad` (`Phantoms::hold_pad`) holds
a brain's pad at a given state in place of its own until let go: the
verifier and the sim's seam drops drive a bot seat with it. A wrapper
source (a Clanker, the sim's `Metered` seat) forwards it to its brain. With the Glitches setting on, a show-off at a spot
with a crouch spot known is a **Fun** route back to where the bot began;
it ends when the bot sees an enemy.

### Breaking the glass: panes, mask, break step

- **Kept with the walls.** The nav build keeps every breakable surface,
  its index and outline, beside the walls (`glass::panes`,
  `los::Walls::glass`, in the `.bots` file). Their triangles stay in the
  walls, solid, so line of sight and the jump sweeps treat glass as whole.
- **In the way.** Before a prime, the route finds the pieces a crouched
  walk from the prime's start to the stand passes through
  (`bounce_route::glass_in_the_way`, `glass::in_the_way`): lines at 0.1,
  0.25 and 0.4 over the floor, the walk's centre and 0.15 either side,
  on 0.175 past the stand. They all run under the crouched capsule's top
  (0.5), so the piece the crouch is forced by stays.
- **Whole or broken.** The bot reads BSP 0's mask each tick while a route
  is asked for or under way, with the pair word (`guest::breakable`:
  `[0x4ed280]`, the enable byte, then 32 bytes a BSP from `+1`). A set bit
  is a whole piece.
- **The break step.** With a piece in the way whole, the prime waits: the
  bot stands at the prime's start, faces the lowest whole piece, aims at
  its centre from eye height and holds the trigger while the aim is
  within 4° (`mind::RoutePad::report_shooting`). The route's phase reads
  `break`, and its log `break_glass`, then `glass_broken` when every
  piece is broken and the prime begins. After 150 ticks
  (`bounce_route::BREAK_TICKS`), or when the mask can't be read, the
  route fails `glass_whole`. A practising bot only shoots glass; it never
  fights.
- **Which spots.** The pieces come from the map's geometry, so any crouch
  spot behind breakable glass gets the step, scanned or measured. A crouch
  spot with no known way out primes in place and has none. With no mask
  read (no pair reader host, `player.goto` routes) the prime runs as
  before.

`h2 agent world.breakable` reads the same mask
([agent-api.md](../../runtime/agent-api.md#worldbreakable-broken-glass)).
Tests (`cargo test -p bots`): `glass::tests` (only flagged, indexed
surfaces are panes; the walk in goes through the low piece and under the
lip's; a piece is whole while its bit is set),
`guest::tests::the_breakable_state_reads_each_bsps_mask_after_the_enable_byte`,
`bounce_route::tests` (the window primes from the room floor; a whole
piece is shot out before the prime; glass that never breaks fails
`glass_whole`), `cache::tests` (the glass round trips in the file).

### Practising bounces: the practice loop and weights

**Practising bounces**

While the superbounce mode is on (a game whose script requires
`hud.superbounce`: Superbounce, Bounce Practice, or a variant of
either; the game calls
`bots::set_practice` from `superbounce_mode`, and each brain follows it
at its next sample) a bot fights no one: it picks no target, shoots,
throws and melees nothing, and being shot changes nothing. Its whole pad
is a practice loop (`mind::practice`, ahead of everything else in
`Mind::step`):

1. stand a moment, looking round (20 to 60 ticks at first, then 45 to
   150 between tries, drawn each time), stepping out of any player's
   way ([Letting players go first](../bots.md#letting-players-go-first));
2. pick a site within 120 units that a route reaches, the best weighed
   first (`practice::weight`):

   | What | Weight |
   |---|---|
   | distance | −1 a 60 units, times the bot's roam (0.4 to 1.6) |
   | a proven site | +2 |
   | a site the learned file calls `learned` | +1 |
   | another bot's claim on it | −3 for each; a claim near it −1.5 next to it, down to 0 at 6 units (`practice::crowd`) |
   | the bot's recent tries of it | −1.6 times the bot's novelty (0.6 to 1.4) for each, fading as `exp(−age / 2700 ticks)`, its paired crouch spot's tries counting half (`practice::recency`) |
   | a player using it or its crouch spot | −2.5 on it, −2.0 on the way onto it |
   | the bot's taste for its shape | −1.2 to +1.2: slots, or creases, Vs and busy points |
   | a Monster site | +3 times the bot's pull to Monsters |
   | the site it goes again at after a miss | +8 |
   | the random part | 0 to 0.8 |

   A try that failed on the way (a walk, a prime, no route) blacklists
   the site for a while (`Stuck::blacklist`), so the next try is
   somewhere else, and counts against what it failed at (below); a
   landing that did not bounce is shrugged off ([Fumbles](../bots.md#fumbles)).
   With no crouch spot to prime at (and the word not −3) or no site a
   route reaches, it waits 300 ticks and looks again (`waiting`);
3. run the bounce route to the site (`bounce_route::plan_site`: that one
   site, priming at its paired crouch spot, then the jump or the drop,
   the touchdown classified, over once down from the launch; going on
   into a chain when it planned one, [Chains](../bots.md#chains)), and tally
   it: a try, a bounce when the route ended done by the site, else why
   (the route's error, or the landing's result for one that landed
   without launching).

The memory of recent tries is the bot's own (its last 16 sites and 16
crouch spots), so a bot works round a map's sites rather than cycling
the same few, and the claims spread the bots over it.

### Practising bounces: which sites, giving up

**Which sites a route reaches.** The first time a pick looks at a site
it pairs the site with a crouch spot (`bounce_route::pair_site`), and
the bot keeps the pairing while the map and its spots stay the same:
the site's approach (behind it, or its launch floor), and of the 6
crouch spots nearest that approach within 60 units, proven first, the
one whose start a walk reaches and from which a walk reaches the
approach, proven first and then the shortest walk. A site with no approach (a drop with no launch floor, no
launch) or no crouch spot a walk reaches it from is never tried (a
primed bot needs no crouch spot). A paired site is picked when the walk
from the bot to its crouch spot's start (primed: to its approach)
exists; one with none is passed over for 600 ticks
(`practice::OUT_OF_REACH_TICKS`). When the walk that fails is the one
to the paired crouch spot's start, the crouch spot is what is out of
reach (a floor a walk drops down from to the site but none climbs to
from the bot): it is set aside for the same 600 ticks, its sites pair
again without it, and the pick looks again 2 ticks later. A pick makes at
most 4 pairings and 4 of those walks, and with more to look at picks
again 2 ticks later. A practice try is therefore never spent finding
out that a site is out of reach: `no_route` is a site whose route
failed to plan from where the bot stood, which a pairing rules out but
for a bot that moved between the pick and the plan.

**Giving up what keeps failing.** A failed try counts against what it
failed at (`Mind::practice_strike`): a walk to the crouch spot that
failed (`no_path`, `goal_off_mesh`, `tail_stuck`, `path_lost_*`), or
its prime, against the crouch spot; a walk to the approach that failed,
against the site. Strikes lapse: one more than 3600 ticks (2 min,
`practice::STRIKE_TICKS`) after the last starts the count again. A
crouch spot with 2 (`practice::CROUCH_STRIKES`) is given up and the
sites paired with it pair again without it; a site with 3
(`practice::SITE_STRIKES`) is given up. Either comes back after 5400
ticks (3 min, `practice::GIVE_UP_TICKS`). A failed prime also counts
for every bot on the map (`practice::PRIME_FAILS`, by the crouch spot,
lapsing the same way): the geometry that refuses one bot's crouch
refuses them all, so a crouch spot whose primes failed twice between
the bots is given up by each of them. A proven crouch spot
(`superbounce::PROVEN`, or verified) is never given up. When a pick
finds nothing while something is given up, everything given up comes
back at once and the pick looks again: giving up never leaves a map
with nothing to try. A landing that did not bounce counts against
neither. A pick that finds nothing says why once a map in the log
(`bots: bounce practice: nothing to try: ...`: the sites and crouch
spots known, how many crouch spots are set aside as out of reach, and
how many sites had no crouch spot to prime at, no
approach, no walk to them, were resting, given up or too far).

### How each bot practises: trait table

**How each bot practises**

Each bot practises its own way (`mind::practice::style::Style`): every
trait a continuous draw from its name's hash (`NameProfile::seed`, the
brain's seed without a name; lanes 40 to 48 of the profile's draws),
placed inside its skill's band, so a name practises the same way every
match and two names differently. The fumbles blend half the name's
try-hard (the profile's own), so a keen name fumbles less, and the
chains half too, so a keen name chains more. Match randomness picks the
moments; the name sets how often.

| Trait | What it changes | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|---|
| Fumbles | share of tries set up a little wrong | .15–.30 | .10–.20 | .06–.14 | .03–.10 |
| Chains | chance it goes for the next link when one is in reach | .15–.45 | .25–.60 | .40–.80 | .55–.95 |
| Monster pull | times +3 on a Monster site | .10–.60 | .15–.75 | .20–.85 | .25–.95 |
| Slot taste, crease taste | 0.5 neutral; times ±1.2 on a site of that shape | 0–1 | 0–1 | 0–1 | 0–1 |
| Roam | the distance weight's factor: 1.6 stays near, 0.4 ranges far | 0.4–1.6 | 0.4–1.6 | 0.4–1.6 | 0.4–1.6 |
| Novelty | how much its recent tries weigh a site down | 0.6–1.4 | 0.6–1.4 | 0.6–1.4 | 0.6–1.4 |
| Retry | chance it goes straight back to a site it just missed | .20–.70 | .20–.70 | .20–.70 | .20–.70 |
| Patience | how long it waits for a player before going elsewhere, ticks | 90–360 | 90–360 | 90–360 | 90–360 |

### Fumbles

**Fumbles**

A person misses now and then, and a bot does too: at the start of a
try, with its fumble rate's chance, the bot sets the try up a little
wrong (`practice::fumble`), one of the site's knobs off, and then flies
it as it is. Nothing scripts the fall; a fumbled try can still bounce.

- `off_line`: the aim 0.12 to 0.25 units across the seam (along its
  drift, either way; any way round a busy point, which has none). A
  landing more than 0.1 off the seam line does not bounce (the
  2026-11-23 sweep of 60 Ascension seams);
- `early`, `late` (a running jump off a launch floor): the takeoff 0.15
  to 0.30 units of run early, coming down short, or 0.10 to 0.20 late,
  coming down long;
- `lazy_air` (any drop): the air stick let go 0.15 to 0.30 units from
  the target, so the landing drifts off.

A standing jump can only be off the line. The drop's height is the
launch floor's, so a fumble never picks a wrong height. After a landing
that did not bounce (`missed`, `too_slow`, `no_bounce`, `not_primed`),
fumbled or not, the bot shrugs: it stands 60 to 120 ticks facing the
site and, with its retry chance, goes straight back to it once (weighed
+8, not blacklisted); otherwise the site rests (`Stuck::blacklist`) and
the next try is elsewhere. The try's line ends `; fumbled <how>`.

### Letting players go first

**Letting players go first**

A practising bot never blocks a player, a person or another bot, and
never treats one as an obstacle to flee or path round
(`mind::practice::yielding`). It tracks every living player's position
and speed tick to tick. A site's floor is the line from its approach
over its target and 1 unit on (the landing), 1.5 units round it; a
crouch spot's, the line from its prime's start to its stand, 1.2 units
round. A player uses a floor while on its level within it (from 1.5
under to 3 over, or in the air over it, a flight off it), or on the way
onto it (moving faster than 0.8 units/s, their course enters it within
1.5 s).

- **Before the jump**: while the route walks to a crouch spot or to the
  approach, and the bot is within 3 units of that floor's reach, a
  player using the floor stops the route (`yield`: neither a try nor a
  strike, and no rest for the site). The bot waits (`yielding` in
  `bots.bounce_status`), facing the player with a glance about now and
  then and a nudge of the stick every 30 ticks so the body does not
  sleep; standing in the floor's reach, it first walks back out of it,
  2 units past. Once the floor has been clear 20 ticks it plans the try
  again from where it stands; past its patience it gives the site up
  for now (counted as a recent try, so the pick looks elsewhere).
  Nothing interrupts a prime, a jump, a drop or a flight.
- **Between tries**: standing, it steps 2 units across the course of a
  moving player whose course passes within 1 unit of it in the next
  1.5 s (from within 8 units, ahead of them), and off the floor of a
  site another bot is on a try at; at most once in 30 ticks, a plain
  walk.
- **Between bots**: a bot on its own try always goes first. Two bots
  waiting for each other do not wait forever: the claims say who waits
  (`practice::claim_as`: the bot's player slot, since when it claimed,
  whether it waits), and the older claim goes first.

A player on a site also weighs it down in the pick (above), so a bot
usually picks another.

### Chains

**Chains**

A chain is the 2nd to 6th bounce with no ground between
([superbounce.md, "The rule"](../superbounce.md#the-rule)). At the start
of a try the bot may plan one (`bounce_route::chain`): from the site's
launch, the sites a flight can come down on without touching the
ground, the least steering first (the site itself among them, straight
back down), and with its chain chance a link at a time, at most 3, one
of the 3 easiest, another site than the one it is off 3 times in 4. The
route goes on after the first bounce with a `chain` step for each link
(`bounce_route::Step::Chain`, `chain_onto`): in the air off the last
launch it steers the landing onto the link's target as a drop's air
does, and the link is made when it is thrown up again. A link that comes
down without launching ends the chain; the rest are passed over. The
route's view counts the links tried and made (`chain`), and the try's
line ends `; chain M of N`.

The flight is a trait (`chain::Trajectory`). The one built in,
`chain::Ballistic`, is conservative: the site's launch estimate
straight up, the measured fall gravity (4.15), the measured air control
(1.05 units/s² up to 1.2 units/s) with three quarters of that reach
counted, a touchdown at or over the link's `need`, and a V's arm only
from across its bisector. No flight after a bounce has been traced, and
whether a seam bounces a landing at a chain's speed (8 to 13 units/s
in, against the 5.3 the measured drops come in at) is open
([superbounce.md, "What is known about a second bounce"](../superbounce.md#what-is-known-about-a-second-bounce)):
`Ballistic::max_speed_in` is where a measured window goes, and until
then a bot tries chains the game may not file.

### Monsters: drill, training, try's line, Ascension drop, claims, events

**The drill.** A sim match's `[drill]` ([sim.md, "The bounce
drill"](../../runtime/sim.md#the-bounce-drill)) makes the practice try one
site only (`bots::drill`, `Mind::set_drill`): the known site within 0.5
of the drill's `site`. The bot walks to the drill's start (its `start`,
else the site's approach: a drop's takeoff or launch floor) as a plain
walk, and standing there unprimed asks the host for its word
(`Mind::drill_wants_arm`; the brain passes its unit's datum to
`drill::request_arm`), for 20 ticks before walking 1 unit off and back,
since a body that sleeps has no level pair to write. Primed, it runs the
route by the site from there (no prime: the walk to the approach, then
the jump or the drop), tallied as a practice try, pauses 10 ticks and
walks back to the start. The word is read between tries too
(`Mind::wants_pair`). A route that starts primed from a word read at −3
says so (`OrderRun::primed_read`: `primed_read`, and a practice event
`Primed` with `read` and not `own`). The log says once a drill which
site it found (`bots: bounce drill: site ... (a drop), aim ..., start
..., N sites known`), after any `no known site` said while the map's
spots were still coming in.

**Training.** A sim match's `[train]` ([bounce-search.md](../bounce-search.md),
"Scheduling a batch") makes the practice fly the run's board instead
(`Mind::set_train`, from `learn::current`; `learn::shared` keeps the
board, built at the first pick from the known sites once the map's scan
is in, `practice::site_in`: each site's allowed `how`s and the route's own
knobs from `flown`, and its line of the learned file). A free bot takes
the board's pick (`learn::Board::pick`) and flies the round's slots one
after another, each as a drill try: the try's site is the known site
with the search's knobs over its own, the bot walks to its approach (a
drop's launch point), asks for the word as the drill does (the same
`drill_wants_arm` and `drill::request_arm`), and runs the route by the
site from there. One flight is a try: a landing that did not launch ends
the route (`try_over`). The try is the route's first flight
(`learn::Try::from_leap`, the miss from `Site::own_target`); a `died` is a
scored try with a death; an end that says nothing about the knobs is
void (`map_changed`, `practice_off`, `pair_lost`, `no_route`, a walk that
failed, and the training's own `no_site`, `no_launch_floor`, `no_path`
after 3 walks to the start, `not_armed` after 3 wake-ups). When the
search has no slot left the round folds (`Board::fold`), the log says
`bots: train: site (...) round N ...`, and the board's lines go to the
map's learned file (`spots::store_learned`). Each try logs its line with
`train` in place of `drill` and the knobs it flew appended (`; knobs
how=runoff ax=... back=...`, `practice::knobs_text`). `bots.bounce_status`
gives the round (`practice.train`).

**The try's line.** Every tallied try, drilled or practised, logs one
line (`mind::practice::try_line`):

```text
bots: bounce try: bot 0x2f drill site (-10.62, 3.88, -3.19) aim (-10.45, 3.99, -3.19): missed; jumps 3, the last: takeoff (-8.41, 2.49, -0.76) ground 1.21 vz +3.08 word -3; air 62 ticks stick mean (-0.05, +0.04) max 0.31 let go 20; down (-10.30, 4.11, -3.19) miss 0.204 in 5.38 out +0.00: missed; before: missed, too_slow
```

The bot (its seed), drill or practice, the site and its aim, what the
try came to, then the last flight: where it took off, the ground and
vertical speed, the word then (`unread` when it could not be read), the
air stick in world axes, the touchdown (origin the tick before, the
miss, the speeds in and out) and its result; then the results of the
flights before it. A try with no flight ends `no jump, ended at step N
(name)`. A fumbled try adds `; fumbled <how>` and a try that chained
`; chain M of N` (links made of tried). (The numbers above show the
shape, not a run.)

**The drill's drop at Ascension.** What lands in the slot there is a
running jump at full stick: off the upper level at about 2 units/s and
3.08 up, 60 ticks in the air, braked by the air stick to come down
about 0.5 units/s sideways at −5.4 near (−10.13 to −10.3, 3.85 to
4.07). A jump at half the speed (a walk, about 1.05 units/s at 0.6 to
0.85 of stick) comes down on the floor short of the slot at −5.26
whatever the air stick asks, since the stick can't push a flight past
about 1.2 units/s. A landing braked to a standstill over the crease
fell through it once. The numbers are in
[the bounce-practice notes](../notes/2026-11-21-bot-bounce-sim.md), "The
drill four ways"; the route's plan for it is the drop above.

The bots spread over the sites by claims on a board the process shares
(`practice::CLAIMS`): a bot renews its claim each tick of its route, and
a claim lapses 300 ticks after it was last renewed. A route an agent
asks for (`bots.bounce_to`, `superbounce.verify`'s routes) runs as usual
and is not tallied. Turning the mode off ends a practice route
(`practice_off`). `bots.bounce_status` gives the tally while practising
(`practice`: the state, the site, attempts, bounces, and by site the
attempts, bounces and reasons; the state is `idle`, `route`,
`yielding` or `waiting`), so a game of practising bots is a live
verifier too.

Each bounce a practising bot makes is recorded by the game as a ghost
run, and the best run of each spot is kept in `<map>.ghosts` beside the
spots and verified files, proving the spot as it goes
([superbounce.md](../superbounce.md#ghost-runs)). Nothing in the bots crate
records it: the game reads the bots' units each tick.

The practice also queues an event for each route it starts, the first
time the route's word reads −3, and each try it tallies
(`mind::PracticeEvent`: `Started`, then `Intent` with `monster`, the
chain's `links` planned, its `fumble` and whether it goes `again`;
`Primed` with `read` and `own`; `Chained` with the links `tried` and
`made`; `Tried` with `bounced` and why not), each wait for a player
(`Yielded` with the ticks `waited` and whether it `gave_up`) and each
step out of a player's way (`SteppedAside`); at most 64 between takes,
the oldest dropped. `Mind::take_practice_events` drains them; sim mode counts them
into its report ([sim.md, "Bounce practice"](../../runtime/sim.md#bounce-practice)).

### Tuning the bounce: field table

| Field | Default | What it moves |
|---|---:|---|
| `approach` | 0.8 | the walk to a standing site ends this far behind it (units; `Bounce::from`) |
| `approach_speed` | 1.0 | the stick on that walk (`OrderRun`) |
| `settle` | 0.05 | within this of the site it stands (units) |
| `settle_ticks` | 90 | settling gives up after this long and jumps |
| `settle_gain`, `settle_min`, `settle_max` | 4.0, 0.12, 0.6 | the settling stick: gain a unit off, held between min and max |
| `still_ticks` | 4 | still this long before the jump |
| `jump_ticks` | 3 | A held this long for the jump (a standing site's, and a drop's running jump) |
| `drift` | 0.15 | the stick toward the drift in the air: the aim onto the crease (at 0.15, inside the move dead zone, the bounce-practice runs show no sideways speed at all) |
| `bounce_tries` | 8 | jumps at a site before giving it up |
| `crouch_first` | 6 | the prime: crouch first, standing still (ticks) |
| `crouch_ticks` | 12 | crouch held under the lip (walking and in place) |
| `release_ticks` | 6 | pushing in after letting go |
| `in_speed` | 0.6 | the stick in |
| `push_in` | 0.06 | how far past the stand it pushes (units) |
| `start_out` | 0.35 | how far out a try starts (units) |
| `approach_turn_deg` | 25 | the second and third tries' turn |
| `out_speed` | 1.0 | the stick back out |
| `line_up` | 0.15 | the drop: lined up on its run-up within this (units) ... |
| `line_up_ticks` | 45 | ... or after this long |
| `run_off_speed` | 0.6 | the stick of a drop's slower run, taken where a full-stick one overshoots (`drop_takeoff`) |
| `air_gain` | 1.2 | the drop's air steering: the change in sideways speed it asks for over the one that just lands it (`air_stick`) |
| `drop_tries` | 3 | drops before giving up |
| `fly_speed` | 1.0 | the stick toward the goal riding a launch |

### Driving a player: watchdog and glue

The plan is played by `OrderRun` as a bot's would be, its last walk
ending within the asker's `within` (`OrderRun::arrive_within`).
`goto::Goto` adds a watchdog for a body the brain does not own:

- a walk that strays more than 1.5 units sideways from the leg it walks
  (from the waypoint passed, or where the walk began, to the next one;
  jumps, drops and lifts are left to the follower) plans again from where
  the body is (`strayed`);
- a walk whose distance left along its waypoints has not come down by
  0.5 units in 150 ticks plans again (`stuck`), except while the next
  waypoint is a lift, an elevator or a teleporter;
- a route the runner ends with `path_lost` (the follower's own stuck and
  fallen checks), `stalled` or `pair_lost` plans again, the word read
  again or, unread, the last run's priming kept;
- after 6 plans made again, or the timeout (3600 ticks unless asked),
  it gives up with the last reason (`strayed` reads `stuck`).

A jump is asked for as a fresh press: the pad never holds A two ticks
running. The game's glue (`player_goto`) reads `player.0`, steps the
`Goto` each tick on an agent ticker and holds the first controller's pad
with the motion, the walk turned into the stick against the player's
aim (the player does not turn while it walks). With no bot in the game
nothing has loaded the map's nav mesh (the bots' glue reads it only
while a phantom plays), so the glue asks for it the same way: the nav
worker's load (built when stale), else the `.bots` file in the cache
directory.

### Not built: host note

One community note matters here. The Bungie.net post says BXR "works as
a reload glitch from any range, but only off host". Clankers play on the
host. Whether the cancels work for a host-side player is the first thing
to check.

### Game run checklist

**Game run (checklist).** For the user. Use one Clanker against you on
Legendary with Glitches on Lots. The agent API reads what happened:
`world.object player.N physics` (havok_pos, havok_velocity, airborne,
crouch), `havok.contacts {object: player.N}` and `trace.start player.N`
for a row every tick ([agent-api.md](../../runtime/agent-api.md)). Each line
is a yes or no:

1. **Off.** Glitches Off: in five minutes no bot does any of the below.
2. **BXR.** Give the bot a battle rifle and let it get close. It melees
   and fires at once, with no melee recovery between. Your shields drop
   on the B and you die on the R. If the shot waits out the melee
   anyway, try `MELEE_CONNECT` 3 to 8; if it never cancels, note "host".
3. **BYYR and BXB.** The same with a part-empty magazine and a second
   gun (two switches, then the shot), and with an empty SMG and no
   reserve (two melees close together).
4. **Double and quad shot.** At mid range with a battle rifle: two bursts
   leave almost together (six rounds), then it reloads. With a second
   gun, two switches and two more bursts. Tune `BURST` if the second pull
   is ignored.
5. **Sword flying.** Give it a battle rifle in hand and the sword as the
   other weapon. Stand 5 to 15 units off. It switches and flies at you
   much further than the sword's own lunge, higher with the A. On Easy
   it mostly just switches.
6. **Crouch jump.** Calm on Lots: now and then a jump with the legs
   tucked at the top.
7. **Super bounce.** Ascension, calm on Lots, nobody near the ramp: the
   bot walks to the crease, jumps in place a few times, and one landing
   throws it up (trace: two opposing contacts against the fixed body on
   the launch tick, then +8 to +14 units a second up). With Some it gives
   up after three tries.
8. **Never.** It does none of these carrying the flag or the ball, with
   its shields down, or with a second enemy close by.

### 3.9 Vehicles: user quote

The user, after playing the Clankers: "clankers also don't seem to
interact with vehicles, try and board, go and take, lock on with

### 3.9 Vehicles: fields table

| Field | Where | Type | Confidence | From |
|---|---|---|---|---|
| parent: a rider's vehicle, a mounted turret's vehicle | `O+0x14` | datum | confirmed | live-gamestate.md; `engine::view::objects::Body::parent`; `live-telemetry` `object_in` (riders) and `vehicle_mount` (turrets) |
| position, forward, up | `O+0x64`, `+0x70`, `+0x7c` | `f32[3]` each | confirmed; relative to the parent when there is one | `engine::view::objects::Body`; live-telemetry.md (a mounted turret's `+0x64` is relative to its vehicle). A rider's own position is not used: as telemetry does, a rider is where its vehicle is |
| body vitality | `O+0xec` | `f32`, 0 to 1 | confirmed | `object_get_health` `0x2a1a20` (live-telemetry `layout::BODY_VITALITY`) |
| wrecked | `O+0x10a` bit 2 | `u16` | confirmed | `0xd6800` (`guest::BODY_DEPLETED`) |
| the unit's seat | `U+0x1fc` | `i16`, -1 none | confirmed | live-telemetry `layout::UNIT_SEAT`: the enter-vehicle routine `0xea1f0` stores it (`0xea405`), leaving stores -1 (`0xea995`) |
| seats block | `V+0x1c8` count, `+0x1cc` pointer | `i32`, `u32` | confirmed | `crates/formats/tags/src/defs/unit.txt` (`group vehi`, `seats`, `0xb0` each; the seat label at `+4` checked against 05a's scripts, `h1scripts::seats`); the stride is `0xea1f0`'s `imul edi,0xb0` (`0xea330`) |
| seat flags | `S+0` | `u32` | traced and disc-checked | Driver bit 2, gunner bit 3, boarding bit 11, forbids players bit 16. See `h1objects::rebuild` and `h1objects/tests/vehicle_rebuild_disc.rs` (`0x000cc810`, `0x000c8c87`, `0x000c92c0`). The first unflagged passenger is used as fallback driver **[guess]**. |
| boarding target seat | `S+0x3e` | `i16` | traced | `0x000c92c0`, `h1objects::rebuild::SEAT_BOARDING` |
| entry marker and admission limits | `S+0xc`, `+0x98`, `+0x9c`, `+0xa0`, `+0xa4` | string ID, four `f32`s | traced and disc-checked | `0x000c6fb0` resolves the marker, with the object's origin for an empty ID. `0x000c9040` checks eye-to-marker radius, marker facing, look cone and distance to the vehicle. See `h1objects/tests/vehicle_ground_disc.rs`. |
| render-model reference and marker data | `V+0x38` → `hlmt+4` → `mode` | tag datums | traced and disc-checked | `defs/obje.txt` and `defs/hlmt.txt`. In `defs/mode.txt`, `mode+0x58` is the `0xc`-byte group block (ID `+0`, markers at `+4`); the `0x24`-byte marker holds variant indices at `+0/+1`, a node at `+2`, translation `+4` and quaternion `+0x10`. |
| live marker transform | `O+0x114/+0x116` | block size and signed relative offset | traced | [native-renderer.md](../../graphics/native-objects.md#research-points-code-unless-marked-h2-disasm). Stride `0x34`; matrix scale `+0`, forward `+4`, left `+0x10`, up `+0x1c`, position `+0x28`. |
| chassis dimensions | `mode+0x14` compression block | position bounds | disc-checked | `defs/mode.txt`; each record begins with six floats giving xyz min/max. The planner uses lateral extent and vertical span, with margin and class fallbacks **[guess]**. |
| vehicle type | `V+0x1f0` | `u16` | traced and disc-checked | `0xef545` compares it with 5 and 6. `h1objects::rebuild::H1_VEHICLE_TYPE` and its disc test compare the classes by vehicle (tank 0, jeep 1, scout 4, fighter 5, turret 6). |
| fighter secondary weapon | `V+0x1c0`, `weap+0x2c8/+0x2d0` | weapon references, trigger and barrel blocks | traced and disc-checked | `defs/unit.txt` and `bots::arms`. The retail Ascension Banshee weapon has two triggers and barrels. Barrel 1's projectile is `banshee_bomb`. The tag establishes availability in the weapon definition. The game decides whether the variant will fire it. |
| the gun mounted on a Warthog | a vehicle object whose parent is the Warthog; its gunner's parent is the gun | | confirmed | live-telemetry.md (`parent` on a frame vehicle); `live-telemetry` `vehicle_mount` |
| a rocket launcher | `weap` zoom levels `+0x1fe` 1, and the blast role (§3.6) | | checked on Zanzibar's tags (zoom) | §3.6's handling table: the rocket launcher is the only blast weapon with a zoom level |

### 3.9 Vehicles: how the bot reads them

How the bot reads them (`vehicle::read::sight`, once a frame for each bot
in a multiplayer game, after the perception): the objects array (at most
2048 slots) for live vehicle objects that are not wrecked and whose
definition is a `vehi` tag, each with its class and seats; a mounted
turret's pose is made a world one from its vehicle's; then each listed
player's living biped whose parent is one of those vehicles fills the
seat `U+0x1fc` names, and the bot's own is its `riding`. A fault leaves
the bot on foot, never blind. It is kept apart from `brain::Perception`
(the mind is handed it with `Mind::see_vehicles`), so every other test
and caller plays as before.

`guest::perceive` puts a rider where its vehicle is (its own
`+0x64` is relative to it): a Warthog's driver and an enemy on its gun are
seen at the Warthog and at the gun, not near the world's origin.

### 3.9 Vehicles: lock-on prose

**Lock-on** (`vehicle::lock`). With a rocket launcher in hand
(`lock::locks_on`) and the target riding a vehicle, the bot aims at the
vehicle's middle (0.3 over its origin) with no lead (a locked rocket
follows it), and fires only once it has held the vehicle in the reticle
(the vehicle's apparent size, 0.8 world units, plus the skill's fire
cone) for the lock time. At infantry nothing changes: fired unlocked, at
the feet for Heroic and Legendary (§3.4). The game's own lock time is not
traced; the bot assumes 1 s (`lock::LOCK_TICKS`) and adds its margin:

### 3.9 Vehicles: zoom note

The bot does not zoom: Halo 2's launcher is taken to lock without it
(§12). "A blast weapon with a zoom" stands for the rocket launcher; the
fuel rod's zoom was not looked at (it is not on Zanzibar). If it has one,
a bot with it waits a second at a vehicle and then fires: slower, not
wrong.

### 3.9 Vehicles: ground-route performance

**Ground-route performance.** On 2026-11-20, ground-vehicle A* accounted
for 2,917 of 5,201 game-thread samples in a Waterworks 2v2 sim. Only a
ground driver asks for this path. A failed plan leaves it free to choose
again next think, including picking a new roam goal. An exhausted search
now remembers which polygons it reached from that start. Further goals
outside that set fail immediately. A successful search can reuse its
corridor for the same start and goal polygons and append the current
snapped endpoint. Each driver caches portal clearance for its loaded map,
vehicle datum and exact hull dimensions. Steering still checks the next
leg from the vehicle's current position. A blocked leg drops the cached
corridor for the next plan and keeps the normal steering retry.

### 3.9 Vehicles: entry markers

The bot looks up the chosen seat's entry markers in the actual vehicle's
render model and places them using the live node matrices. Scorpion and
Wraith driver seats can offer two positions. It searches for a stand point
on the floor within the tag's radius and facing limits, looks at the marker
and holds X while in range of the seat. The stand-point offsets and the
0.6 world-unit eye height used in this check are **[guess]**. A retry
uses a closer point covered by the same entry data.

### 3.9 Vehicles: go and take detail

**Go and take** (`vehicle::plan::take`, `Mind::mount_plan`). A phantom
can choose a vehicle after 1.5 s with no enemy in view if it carries no
objective and seeks no weapon within 20 units. A weapon farther away
can become a vehicle errand. The phantom drives to within 3 units of it
and finishes on foot. In an objective game it chooses a vehicle only
when the objective point is more than 20 units away. It takes the
nearest vehicle by distance (plus 3 per unit of height) within the
skill's range that:

- is a class it drives, whole (over 0.4), upright, standing still (under
  0.5 a second);
- has its driver's seat free and nobody of the other side aboard;
- no teammate on foot is clearly nearer to (within 6, and nearer than 0.8
  of the bot's distance): they have it;
- it has not given up on, or got out of, in the last 10 s.

The bot walks to a valid entry point and tries X (action, Default layout)
for up to 40 ticks. It retries closer, then shuns the vehicle for 10 s.
It confirms entry using `U+0x1fc` and the parent. It abandons the attempt
after 12 s, when the seat is taken, or when an enemy gets in.

### 3.9 Vehicles: turrets prose

**Turrets.** A standalone turret or a free gun on a nearby teammate's
vehicle is worth taking when it has a clear line to recent enemy contact
or the objective the bot is defending or hunting. On the gun, an enemy
within 1.8, or within 5 and off the firing line on the same floor, can
make the bot dismount **[guess]**.

### 3.9 Vehicles: board

**Board** (`vehicle::plan::board`). An enemy (in free-for-all, anyone)
driving a vehicle (not a Banshee) on the bot's level (within 1 in height),
within the skill's boarding range, slower than its boarding speed: in a
fight or not, the bot approaches an authored boarding marker targeting
the occupied driver's seat and holds X when it meets the admission checks.
It lets go when the vehicle speeds off or after 3 s. Speeds come from the vehicles' positions tick to tick

### 3.9 Vehicles: drive

**Drive** (`vehicle::drive`, `Mind::ride`). Halo 2 steers a Ghost or a
Warthog by the camera: the left stick says where to go relative to where
the player looks. The driver:

- goes to the objective's point, else the enemy it fights, else where an
  enemy was last seen, else the weapon it seeks (the errand it mounted
  for), else a roaming goal (§3.2);
- plans over walk edges whose portals fit the vehicle and routes through
  their clear spans. Centre and wheel-line probes check floor, overhead
  room and walls before steering, cutting a corner, reversing or trying
  to splatter someone. After drifting off the mesh it keeps its goal and
  looks for nearby ground it can steer back onto. With an objective on
  an unreachable road it dismounts to finish on foot. If the chassis
  cannot reach a roaming goal, the phantom blacklists it for 10 s. A
  driver that goes 4 s without usable steering or a fight gets out and
  continues on foot. A failed roam plan counts as 16 ticks towards that
  limit, and a fight or usable steering clears the count. While it
  waits for a route, the driver sweeps the view;
- looks at the carrot (or the enemy it fights) and pushes the stick that
  way, full ahead when the camera is on it, down to 0.35 at a turn of 1.6
  radians or more;
- with infantry in front within 15, drives at them (splatter), the skill
  allowing;
- shoots with the vehicle's guns when its seat has them (a Ghost's, a
  Scorpion's; not a Warthog driver's);
- under throttle and slower than 0.4 a second for 1.5 s, backs up for 1 s;
  the third time within 15 s it is stuck for good.

### 3.9 Vehicles: fly

**Fly** (`vehicle::flight`). The Banshee aims and climbs toward a cruise
height, uses hull-width collision probes and pitches into a visible
target for a strafe run. It breaks off at close range, boosts away, and
uses a lateral evasive roll when damaged and high enough. It requests a
fuel rod bomb on an aligned downward run when the fighter tag has a
secondary trigger and barrel. It descends for an objective dismount or
a damaged vehicle. Cruise height, collision envelope, combat ranges,
cooldowns and landing tolerances are **[guess]**.

The Default Xbox pad uses left trigger for boost (also recorded in
`input::touch_layout::DRIVER`) and right trigger for primary fire.
The original manual describes the Banshee's secondary fuel rod weapon
and its ability to roll and loop. A with the left stick for the roll,
and B for the bomb, are **[guess]** in the phantom action path pending a
game run. The phantom lane already maps these pad buttons and triggers
to actions (§5.1). [Original Xbox manual](https://halo.wiki.gallery/images/7/76/H2_Human_Manual.pdf)

### 3.9 Vehicles: gunner and getting out

A gunner aims and shoots (the gun's trigger held within 15 units and in
the skill's bursts beyond, §3.4); a
passenger shoots its own gun as on foot. With nothing to shoot they look
where the enemy was last seen, else ahead.

**Getting out** (`drive::leave`): the vehicle under 0.2, lying flipped
for 1 s, stuck for good (driver), nowhere to drive for 4 s (driver; see
above), nobody driving for 4 s (gunner, passenger), or within 3 of an
objective point it must reach on foot (the flag or ball it is going
for). It holds X 15 ticks in every 20, so a press and a hold both work,
and stops driving. It avoids the vehicle it left for 10 s. When it
leaves because it has nowhere to drive, it avoids the vehicle for 30 s
and roams on foot.

### 3.9 Vehicles: seeing it in a game

**Seeing it in a game.** Two or three Clankers on a map with vehicles
(Coagulation, Headlong, Waterworks, Zanzibar), Slayer, Normal or
Legendary. §11.4 is the checklist.

### 3.10 Firefight: seats and team

Clankers play Firefight as players. They are always on a player team,
never on the AI's. With one player team they fill seats on it alongside
people; with two they fill seats on either, keeping the teams even, and
fight the AI and the other player team.

**Seats.** While the lobby is set up for Firefight the game sets the
player team count (`bots::firefight::set_seat_teams`, 1 or 2). A new
phantom's lobby team (`phantom::session::lobby_team`) is then
`firefight::seat_team`: team 0 with one player team, else whichever of 0
and 1 has fewer players, 0 on a tie. `rebalance` moves a phantom on any
other team onto a player team. The host gives the actors a team no player
holds (firefight.md, "The host runtime").

### 3.10 Firefight: Context table

| `Context` field | Meaning |
|---|---|
| `player_teams` | the player teams' engine team indices, Firefight team 0 first (one or two) |
| `ai_team` | the actors' engine team |
| `bases` | each player team's home base point: the kit's zones `ff_base_00` and `ff_base_01` (the host's `Kit.bases`) |
| `lives` | each player team's lives left, `None` for unlimited |
| `phase` | `Fighting` during a wave, `Between` before and between waves |
| `drop_zones` | the kit's drop zones, where waves arrive |

### 3.10 Firefight: enemies

**Enemies.** Each sample, while a context is set and the bot is alive,
`firefight::read_actors` appends the actor pool's living actors to the
perception's others. Each is a `Seen` with id `ACTOR_ID_BASE` (`0x100`)
plus the actor's datum index, its unit's origin (a rider's is its
vehicle's) and its team. A unit whose body is depleted is left out.
`firefight::normalise` then makes the game a team game; with one player
team it puts every player on that team whatever the engine's team byte
says; and it gives actors the AI's team, or `ACTOR_TEAM` when the AI's
team is a player team or the bot's own. The sight filter keeps an actor
only while it is in view (an actor has no player bit), and the mind treats
an actor as an enemy whatever its team (`firefight::hostile`). So actors
go through the same sight, target choice, aim and grenade path as
players. A teammate is never a target. With two teams the other team's
players are enemies as in any team game.

### 3.10 Firefight: behaviour and not verified

| Condition | Goal |
|---|---|
| the team's lives at or under its living players (the bot and teammates in view or marked) | hold home within `REGROUP_RADIUS` (3) |
| shield (at most 1) plus health under the name's fall-back line | hold home within `FALLBACK_RADIUS` (2.5) |
| between waves, within the errand radius of home, and not at a drop zone alone | none: the bot rearms, collects grenades and roams as in any game |
| otherwise | hold home within the name's hold radius |

The errand radius is `ERRAND` (2) hold radii. A point within `DROP_NEAR`
(8) of a drop zone outside the hold radius is the drop zone's, and the
bot goes there only with a teammate within `COMPANY` (5). Once out of
its free area, it walks back inside the hold radius before it is free
again. A hold goal leashes a fight to the radius (§3.3), so the bot
fights from home and never chases out to a drop zone. Weapons and
ammunition outside the free area are left where they lie
(`State::blocks`, in the weapon plan's filter).

Two numbers come from the name (`bots::name_profile`): the hold radius
is `HOLD_RADIUS` (8) times 0.75 to 1.25 by the name's aggression, and
the fall-back line is 0.4 to 1.0 of vitality by its retreat.

A bot on no player team (never seated so) asks nothing of Firefight and
still fights the actors.

**Not verified.** Whether the actor pool's positions match what a
player sees on a client, whether the sight filter's chest height suits
every AI class, and the leash and radii in a played match.

### 4.1 The bot's pad

A bot's controller is a pad on a free port, not a change to the agent tap.
`input::InputManager::sync` (`crates/platform/input/src/device.rs`) polls every
`HostBackend` in order and puts each new pad on the lowest free port. So a
**bots backend**, added last, makes its pads appear on ports 1 to 3 after
the human's real pads. They show up in `XGetDevices`, so the game sees an
inserted controller and opens it with `XInputOpen`, and every
`XInputGetState` reads it. Two more points:

- `supports_rumble` is false, so rumble is dropped.
- The agent tap's `input.pad` still merges over the bot's pad. Tests and
  scripts can use that to steer a bot.

The brain gives the local backend a pad report after its motor delay.
The backend's `poll` hands it to the host's regular controller input path
without an additional network delay.

### 4.2 Signing a bot in

**The slot flags.** In a local slot (`0x54e8e0 + i*0xc70`), `+0` holds two
flags:

- `0x10`: a profile is loaded. It is read by `0x1902c1`, which returns the
  name at `+0xc1c`, and set by `0x18fb34`.
- `0x20`: the slot is signed in to Live. It is set and cleared by
  `0x190074`. The Live account record is at `+0x470`; `0x1900a5` tests its
  `+8 & 3`.

**The sign-in function.** `0x18fb34(slot, block, file)`, stdcall, `ret 0xc`,
signs a profile into a slot:

- If the slot had no profile, it initialises `+8` (`0x14b410`) and sets
  `+4` from `0x64610`.
- It copies the 0x1e0-byte block (0x78 dwords) to `+0x18`, sets `0x10`, and
  stores `file` at `+0x1f8`. `-1` means no saved file.
- It then builds the controls table with `0x190c34` (eax = slot), and runs
  `0x190eb3`, `0x53810` and `0x54fc0` with values from the block (`+0x148`,
  `+0x14c`).

Its callers are all "a profile was chosen" paths: the profile UI
(`0x24b6ba`, reached only when the chooser's file at `+4` is not -1, and
`0x24b75b` after loading a file), the settings commit (`0x18fd0c`,
`0x18fd59`) and `0x148b6b`. The sign-out is `0x18fc08`.

**The file, and what "in use" means (traced 2026-11-19).** `file` is a
saved-file handle from the saved-file manager (`[0x51ea14]`: the
built-in content first, at most `0x65` records of `0x1e4` bytes, then per
device a list of up to `0x1000` saved files of `0x40` bytes from `+0xbef8`,
`0x40004` apart). A slot's handle at `+0x1f8` is everything that ties it to
a saved profile:

- **Chooser lists.** The list builder `0x24a190` takes the saved files of
  a type (`0x215900`) and leaves out each one a signed-in slot's `+0x1f8`
  holds (`0x18fc75`; a handle with bit 21 set, as -1 has, never matches).
  So a slot holding a saved profile hides it from every chooser.
- **The open-profile list** (`0x55c160` count, entries of `0x44` bytes at
  `0x55c164`, room for four before `0x55c274`). The sign-in appends the new
  handle when it is not -1 and differs from the slot's (`0x18fbd1`, no
  bound check); `0x2172a0` removes one (the sign-in's old handle,
  `PROFILE_SIGN_OUT`). The menus' memory mode keeps the saved-file manager
  (`0x18ef00`'s table at `0x453c00`: `0x2157e0` builds it in mode 1,
  `0x215810` frees it and freezes the list in the others); back in the
  menus `0x18f5e3` re-finds each slot's handle (`0x2170e0`).
- **Saving.** A profile commit (`0x18fcc4`: the block changed) signs the
  block in again with the slot's own handle and sets slot flag `0x40`;
  the save passes `0x190e37` and `0x190e71` write each such slot's block to
  its file (`0x1a0660`, `0x2153dd`), and `0x1a0660` returns at once for -1.

A bot's `file` is always -1 (`guest::NO_FILE`), so it never hides, holds
or writes a saved profile. Nothing on the sign-in or sign-out path deletes
a save: the game's `XDeleteSaveGame` callers are a failed create's clean-up
(`0x215c00`), the profile UI's delete (`0x215e60`) and `0x2ad480`. A new
profile is refused with error 1 (dialog `0x4a`, `0x148ca8`) when a
device's list or its `XFindFirstSaveGame` count is `0x1000` or more
(`0x215c55`, `0x215c65`, `0x217590`).

**Never a saved profile (2026-11-19).** The user's report: the profile
list showed only "Default" (a built-in) and a new profile was refused,
and they thought the bots had taken their profiles. The data root's saves
were untouched (one profile, unchanged since before the bots signed in;
no save directory made or removed since). The guards that keep it so:

- `guest::sign_in_then` and `guest::sign_in` never call the sign-in on a
  slot that has a profile: over a saved one it would detach the user's
  file and put the bot's block in its place;
- a bot's slot that holds a saved profile anyway (a chooser opened for
  its controller and took a stray A, as bots used to tap A before the
  glue signed them in) is signed out once no game runs
  (`local::release_step`, `guest::saved_profile`): `SLOT_SIGN_OUT` gives
  the handle back to the open-profile list and saves nothing, so the
  profile is offered again before a save pass can write the bot's block
  over it, and the join gate signs the bot in again with its own block.

**The players screen does not help.** In `0x1e`'s join path (`0x2528e5`):

- `0x1900ff(port)` counts the other slots that are signed in to Live with
  the same account (`+0x470`). If there are any, it opens dialog `0x88`.
- Otherwise it opens the profile UI for that port: `0x24b6ea` (or
  `0x24b779` in mode 4).
- Then it sets the port's bit in the screen's mask (`or [screen+8], 1<<port`).

So "press A" on a bot port opens a profile chooser that the bot would have
to navigate, and it could pick a real profile.

**Design:** the bot driver signs the bot in itself. On the players screen
`0x1e`, it calls the game's `0x18fb34(port, block, -1)` with a guest call
(`ExecCtx::call_guest`, as `coop_live` and `level_boot` use), then sets
the port's bit in the screen mask.

The block is a copy of the human's block (slot 0 `+0x18`) with these
changes:

- the profile name at block `+8` (UTF-16, 32 units; the profile file's
  `+8`, "Halo0001" for the user's) set to the bot's name. The game rebuilds
  the slot's display name `+0xc1c` from it (`0x190eb3`: slot `+0x20`, 32
  units) at sign-in and whenever the slot's Live state or account changes
  (`0x190074`, `0x18fee9`), so the name the driver writes to `+0xc1c` alone
  did not last. In the user's Xbox Live lobby every bot showed as the
  human's "Halo0001", and the session's unique-name pass (`0x7f660`, from
  `0x65e37`) renamed the copies from the game's default names (`0x19fd00`,
  string ids `0x02000320` "Shadow" and `0x0200032b` "Wilshire");
- the 16-byte field at block `+0x48` (single-byte characters: `0x190001`
  copies 16 and zeroes the 16th; the profile UI's commit fills it at
  `0x24b9ce`, and a saved profile has it empty) also set to the bot's name;
- the control fields set so the brain's sticks mean the same on every
  machine (block `+0xfc` = slot `+0x114`, the record `0x190c34` reads):
  - flags bits 0 and 2 cleared (look inversion and the other flag `0x190c34`
    reads);
  - layout `+0x100` = 0 (Default);
  - thumbstick `+0x101` = 0;
  - look sensitivity `+0x102` = 3.

  The brain's aim spring is tuned against that sensitivity.
- the look at block `+0x118` (colours and emblem) set to the bot's own,
  by full gamertag (see “Clanker appearance and Live player options”).
- The name the game shows comes from slot `+0xc1c` (UTF-16, 32 units,
  `0x1902c1`). The driver writes it after the sign-in.

Because the file is `-1`, nothing is saved. The bot leaves through
`0x190d4b` (`SLOT_SIGN_OUT`, slot in EAX), called the same way, once
it is removed and no game runs; its pad is unplugged only after that
(stage 7, "a lower count, or Off"). `0x18fc08` alone (`PROFILE_SIGN_OUT`)
is not enough: it only zeroes the block, clears `0x10` and sets `+0x1f8`
to -1, so the slot keeps its local user and Live identity, and the
lobby's session keeps its player ("Leaving the lobby (traced)" below).

Whether a signed-in slot with its mask bit is enough for the game to make a
local player in Split Screen, System Link and Live custom games is the
first Run item of stage 1.

**Xbox Live guests (traced).** On an Xbox, every profile in an Xbox Live
lobby is signed in to Live: the account's own player, or a guest of it
("afr(1)"). The profile UI's Live commit (`0x24b971`, once the logon task
succeeded) runs `0x190074(port, 1)` (sets the slot's `0x20`), the profile
sign-in, `0x18fee9(port)` with the account record in EAX, `0x190001`
(the gamertag into block `+0x48`; nothing for a profile with no file) and
`0x190728`. `0x18fee9` copies the `0x70`-byte record (`XONLINE_USER`) to
slot `+0x470`, rebuilds the display name (`0x190eb3`), takes a local user
if the slot has none (`0x64610`), copies the record's first 12 bytes (the
XUID: user id, then user flags) to slot `+8`, and gives them to the slot's
local user (`0x64820`: local users at `0x4cd904`, 4 of `0xd0`, `+0` in use,
`+1` the id). A guest's record is its host's with the guest number (1 to
3) in the user flags' low two bits: `0x1900a5` tests `flags & 3`,
`0x190eb3` names a guest "gamertag(n)" (string `0x2d000231`), the
duplicate-account check `0x1900ff` (dialog `0x88`) skips guests, and when
the host's logon fails the profile UI signs out every Live slot with the
same user id (`0x190186`), guests included.

**Design: the bots are the human's Live guests.** When a human is on Xbox
Live as their own account (`guest::live_host`) and no game runs, the glue
signs each bot whose slot has a profile and is not on Live in as the
human's guest (`guest::go_live`, every 30 frames before `SOUND_UPDATE`,
`bot_glue::to_go_live`). It writes what `0x190074` and `0x18fee9` write:
the `0x20` flag, the host's record with the lowest free guest number
(`guest::free_guest_number`) at `+0x470`, its XUID at `+8` and in the
slot's local user. It leaves out the name rebuild, so the bot stays
"Clanker 1" (not "afr(1)"), and makes no logon: the Xbox Live service, and
our server through the Live HLE, only ever see the human's account (the
HLE's `XOnlineChangeLogonUsers` maps a guest to its host's account by user
id and keeps the guest number; it makes no accounts). Offline and in
System Link with no one on Live, nothing changes. Telemetry finds the bots
by their controller ports (`Bots { ports }`), so a guest goes out as
`bot: true` with no xuid, never as the human. The log says it once per
change: `bots: Clanker 1 on port 1 is on Xbox Live as guest 1 of port 0's
account`.

**Design (§9.5): a bot with a bot account is its own Live player.** The
guest sign-in above is now only the fallback until the server leases the
bot an account (or when it leases none). Once the Clankers driver has a
roster for the hosted session, each local bot takes one of its bot
accounts (`Local::set_account`), and the glue signs it in as that account
(`guest::go_live_as`): the same writes as for a guest, with
`guest::bot_account_record` (the host's record with the bot's xuid, user
flags with **no guest number**, and the bot's gamertag at `+0xc`, at most
15 bytes and a NUL) in place of the guest's. A bot on Live as a guest, or
as an older roster's account, is moved to the new one
(`bot_glue::to_switch_account`); the slot's name becomes the gamertag.
Game-side checks, from the traces above:

- **Guest numbering** (`free_guest_number`, the game's own) counts only
  slots with the host's user id, so a bot account takes none: a real
  guest ("afr(1)") still gets guest 1, with up to three bots beside it
  (all four ports are still the cap).
- **The duplicate-account dialog** (`0x1900ff`, dialog `0x88`) compares
  user ids; each bot's is its own, and a roster never repeats one.
- **A failed host logon** (`0x190186`) signs out only the slots with the
  host's user id, guests included, so it misses bot accounts: the glue
  does it instead. Out of a game, with no human on Live any more, a bot
  on Live as its account leaves (`bot_glue::to_leave_live`,
  `guest::leave_live`: the flag cleared, the record zeroed, and the
  player id its profile sign-in gave it, kept before it went on, back at
  `+8` and in its local user; its name back to "Clanker N").
- **The Live HLE's `XOnlineChangeLogonUsers`** maps each user id to an
  account in the accounts file and refused any other. It now also takes a
  bot xuid (only in the server's `0x000B` range) that a roster this logon
  leased to this console (`State::leased`, filled when `roster_bots`
  answers, cleared at sign-out), as an account's own player with no key.
  The service is never asked anything for it: no logon, no presence, no
  friends (`State::service_user` is 0 for a leased bot). Any other xuid is
  still `INVALID_USER`.

Offline and in System Link nothing changes (no human on Live: no host to
stand beside, and no roster).

**Joining in the pregame lobby (traced, the user's run 2026-11-18).**
Live, Clankers On, Count 5, hosted from the Live menu: the game started
on Ivory Tower with no bots. The players screen is never shown on that
path (Clankers is set in Game Setup, over the lobby), and the join gate
opened only on it, so every port logged `join gate port N: top none,
dialog none, signed_in false (slot 0x1): waits`. `ui_watch` reads the
lobby as no screen: it shares its window and slot with the screens over
it.

- **What retail does when a second controller joins a Live lobby.** The
  lobby's input handler `0x250b1e` (ECX the screen) takes A or Start from
  a controller with no profile (`0x250b53`: slot `+0` bit `0x10` clear)
  to `0x250c80`. With room (`0x199fd6`: 16 less the session's players),
  outside matchmaking (`0x19a902`) and in a pregame lobby (`0x19a279` 3,
  which session types 1 and 2 both give), it opens the profile chooser
  for that controller (`0x1905bf(port, …)`, screen constructors
  `0x18f42d`/`0x18f474`); otherwise error dialog 2 (`0x250ccd`). The
  chooser's commit signs the profile in (`0x18fb34`, which takes a local
  user, `0x64610`) and, on Live, logs it on (`0x24b971`: `0x190074`,
  `0x18fee9`). **Nothing calls the session from there.** The squad
  session's per-frame local-player pass (`0x65770`) walks the four local
  users (`0x4cd904`) and asks each one's status (`0x646b0`); its jump
  table (`0x65898`) is:

  | status | meaning (`0x646b0`) | action |
  |---|---|---|
  | 0 | no local user, or on a Live session (`0x6cd50` 1) a user with the offline id's flags `0xBAD00000` (`0x14b49f`, `0x646df`) | `0x65b80`: its session player, if any, out |
  | 1 | the session is not in states 3 to 8 | none |
  | 2 | no session player for it, none asked for (`+0x4cd9d0`) | `0x65a60`: ask to add it (`0x5a880`), with room (`+0x54` peers against `+0x4990`, `+0x1118`+1 players against `+0x4994`), rate-limited by `[0x4cecc4]` |
  | 3 | asked for, not there yet | none |
  | 4, 5 | a session player whose id or properties differ | `0x65c10`: update it |
  | 6 | in step | none |

  So the session adds any signed-in local user by itself; in System Link
  at once, on Live once the user has its Live identity.
- **Design: the gate opens in the pregame lobby too.** The gate reads the
  lobby from the squad session, not the screens
  (`bots::lobby::pregame`: `[0x4d8ba0]`, networking `[0x527330]`,
  session type `[0x527334]` 1 or 2, session `+0x741c` 3 to 8), and only
  out of a game (a running game keeps the session in those states).
  `bot_glue::screen` names what is up: `players` (the players screen,
  which wins, and gets its join mask), `lobby`, or the top screen's id
  (`none` when none). In the lobby the bot is signed in with the same
  `PROFILE_SIGN_IN` guest call and no mask write
  (`guest::sign_in_then(…, None, …)`); whatever sits over the lobby (the
  Clankers page, Game Setup) does not hold it back, since the sign-in
  presses nothing. Within 30 frames `guest::go_live_as` (or `go_live`)
  gives it its Live identity, and `0x65770` adds it on the next frame it
  runs. No game function past the sign-in is called: none is called on
  an Xbox either.
- The gate line names the screen:
  `bots: join gate port 1: screen lobby (top none), dialog none,
  signed_in false (slot 0x1): signs in`, then `bots: signing Clanker 1
  in on port 1 in the pregame lobby`.

**The start check (traced).** Start Game (`0x251050`, from the lobby's
handler `0x250b1e`: A or Start from a controller with a profile, A on row
0 at `0x250fb3`) acts only when the start state `0x19a2ce` is 9, and says
nothing otherwise; the lobby's status line (`0x24ffc9`) shows the state's
text. With state 9 it asks for the countdown (`0x25106c` → `0x19a78e`:
the pressing slot's local user at `+4` must not be -1, then `0x64900` →
`0x5bf10` → `0x5df30` sets session `+0x49a8`), and only when that fails
outside a lobby of session type 2 or 3 does it raise error dialog 2
(`0x236299`). A or Start from a controller with **no** profile raises the
same dialog (`0x250ccd`) when it cannot open the chooser. `0x19a2ce`'s
checks, in order (`bots::lobby`):

| state | check | text |
|---|---|---|
| 0 | `[0x4d8ba0]` and the session type `[0x527334]` (`0x19a279`), then a squad session in state 3 to 8 (`0x59670`, `0x58d70`) | "No established session" |
| 1 | the session type is not 1 or 2 | "Session not in the pregame mode" |
| 11+ | lobby mode 6, matchmaking (`0x199971`): the hopper's states (`0x19a5fd`) | "Guests are not allowed in ranked matchmade games" and the rest |
| 4, 5, 6 | each peer's map status (peer `+0x88`, `0x652a0`): loading, a peer failed, this console failed | "Waiting for everyone to load the map...", "A player in your party failed to load the map" |
| 7 | co-op modes (0, 2, 4): exactly two session players, both local; a team game: 1 to the map's most teams (`0x251139`) | "coop_takes_two", "You have too many teams (N maximum)" |
| 10 | a countdown runs (`0x64a60`) | the countdown |
| 8 | this console does not lead the party (`0x592f0`: session `+0x72d8` against `+0x50`) | "Waiting for ... to start the game" |
| 9 | ready | "Waiting for you to start the game" |

The custom lobby's status line passes a countdown string to `TEXT_SET`
at `0x25012a`. While the host's countdown is active at zero, pending
phantoms or seated phantoms in the load and simulation-start stages
select **Setting up bots…** through the shared string table
(`crates/extensions/game/src/lobby_status.rs`, `layout_glue.rs`). The
asynchronous nav cache load can finish after play starts.

So the earlier reading, that a bot with a profile but no Live sign-in
keeps the state from 9, does not hold: no check reads a slot's `0x20` or
`+0x470`. `0x59670` and `0x58d70` are about the session, and
`LOBBY_ALL_READY` (`0x592f0`) compares two peer indices (the leader
check), not player masks. A bot can only reach state 7 (as a session
player with a team) or, as a phantom peer, the map states. Signing the
bots in as guests is still what an Xbox does, and the only off-retail
state a bot was in; whether it was the cause is the game run's question.
For that, a Start press (A or Start from a controller with a profile, on
`PREGAME_LOBBY_INPUT`; or A on the lobby's Start Game row, which the
list's row handler takes before the screen sees it: `LOBBY_ROW_PRESS`,
`0x250fb3`, registered at `0x24fb40`, `ecx` the screen, stack 0 the event
record whose `+4` is the controller, stack 1 a pointer to the row word,
0 for Start Game, which calls `0x19a2ce` at `0x251051`; `ret 8`) while
bots are in logs, before the next
`SOUND_UPDATE`, the game's own start state (a guest call to
`LOBBY_START_STATE`) with the check that holds it and each bot slot:
`bots: start state 8, blocked by the party leader check: ...; port 1 0x30
Live guest 1, port 2 0x30 Live guest 2`.

### 4.3 Hiding the views

The host's views and HUD follow the local-user map. Each Clanker is a
remote peer (§5.1), so it has no local view. The local-user mapping used by
the Phantom's input builder exists only in its private copy of guest RAM.

**Listeners.** `SOUND_LISTENERS_UPDATE` fills one record per mapped user
from the user's camera and marks it active. For an unmapped user, the
update clears the unit and cluster fields but leaves the active byte.
The listener selector trusts that byte when it picks the closest listener
for a world sound. A stale active record can use its last camera position.

**Whose sound.** An attached sound starts as kind 1 (positioned).
`SOUND_OWNER_USER` walks the local-user map and tests each first-person
user's unit against the source object and its attachment chain. A match
makes the sound kind 0 (unpositioned), which plays at distance zero. The
attached sound part of an effect and a weapon's animation sound event
both pass through this check. See §5.1 for the Phantom audio hooks.

**The controller speaker, haptics and rumble (Repo).** Nothing there
goes by port:

- The controller speaker (`dsound::padroute`, `boot::pad_speaker`)
  routes by **sound tag name**, never by player: one DualSense or DS4
  speaker, chosen by device name, gets every routed sound, and the Steam
  Controller's trackpads (`input::steam_pad`, "as haptics") play the
  routed tone on every connected controller. There is no port-to-pad
  map for sound. A Clanker's attached sounds stay positioned in the
  world and play on the main output.
- **Rumble** goes by port: `XInputSetState` on a port goes to the pad on
  it (`input::device`, `HostBackend::set_rumble` of the backend that owns
  it). A bot's pad says it has no motors (`supports_rumble: false`), so
  the game's rumble for a bot's port is refused (`NotSupported`, as an
  Xbox pad with none) and never reaches the human's pad.
- **Voice (the communicator).** With a microphone chosen, every port with
  a pad used to get a communicator, bots' included, so the game opened a
  headphone for each bot and played the other players' voices to it on
  the one host output. Now a bot's port gets none
  (`voice_glue::player_pads`).

**The players screen (traced, the user's ask 2026-11-19).** "I don't want
bots to appear in the other player slots on the Xbox Live screen." The
screen's per-frame update `0x252554` (`PLAYERS_SCREEN_UPDATE`, vtable
`0x45a490` slot 3) lays out one pane per port: a port is joined (its name,
no prompt) when its controller handle is non-zero (`[0x4e61cc + 4 *
port]`, `0x18f8b8`; `0x1249d0` reads it too) and its slot has a profile
(`+0` bit `0x10`); with a controller and no profile it says "PRESS A TO
JOIN" (`0x0f00028a`); with none, "CONNECT CONTROLLER" (`0x1100028b`).

- **Design:** around that update the bots' controller handles read as 0
  (`guest::hide_pads`, put back after unless the game wrote one
  meanwhile). A bot's pane is then an empty port's, "CONNECT
  CONTROLLER", whether or not the bot is signed in: what a one-pad Xbox
  shows. Nothing else is touched, so the bots stay joined.
- **Where bots join.** The players screen is a join screen only in Split
  Screen (mode 1, `bot_glue::SPLIT_SCREEN`). In Xbox Live (3), System
  Link (2) or an invitation's mode (4) no bot signs in there: they join
  in the pregame lobby that follows (§4.2, "Joining in the pregame
  lobby"), as opponents, which is where the game wants them anyway.
  With a pregame session up under the players screen it reads as the
  lobby (no mask write). Split Screen has no networked lobby
  (`0x199df9(1, 0)` makes a local session), so its bots still join on
  the players screen, with their mask bits, and are hidden from its
  panes like any others: the screen shows the human alone, the lobby
  and the game show the bots.
- Not changed: B on the players screen (`0x25286d`) backs out only with
  one profile signed in (`0x18f911` counts slots with `0x10`, bots
  included); with bots joined it opens the human's profile menu
  instead, as before. A game run says whether it matters.

### 4.4 What a local slot cannot do

- On **Live**, extra local players are guests of a signed-in account. Run
  item: whether the game's matchmaking allows guests at all, and how many.
  Retail OptiMatch allowed split-screen in some playlists. Custom games
  allow guests.
- With a human on port zero, at most three bots can use local ports.
  All-phantom mode also reserves those three ports for humans.
- With a roster (§9.5) a local bot is its own bot account, not a guest,
  so it takes no guest number and has a page of its own on the web. Still
  only 3 per machine: the controller ports are the cap. More bots than
  free ports are phantoms, which take the roster's later bot accounts.

### 5.1 Session S table and peer, channel, player structures

**Session S** (`*SQUAD_SESSION`; the membership block is `S+0x4c`,
`0x5a680`):

| Offset | Field |
|---|---|
| `+0x40` | the host peer: this console's own index (`+0x72d8`) when it hosts |
| `+0x4c` | membership update number |
| `+0x54` | peer count |
| `+0x58` | peers, 16 × `0x10c` (filled by `0x5fbd0`) |
| `+0x1118` | player count; `+0x111c` player mask |
| `+0x1120` | players, 16 × `0x13c` (added by `0x600f0`) |
| `+0x4980` | must be 4 for the player diff |
| `+0x4990` | most peers |
| `+0x4dac` | protocol: 0 local, 1 synchronous, 2 distributed (Live, System Link) |
| `+0x72d8` | the local peer: this console's index (skipped for views) |
| `+0x72dc` | peer channels, 16 × `0x14` (`0x5f900`) |
| `+0x741c` | state: 5 in the lobby **and in a running game**; 6 to 8 [guess] leaving, host migration and disbanding |
| `+0x7618` | membership change counter |

- **Peer**: `+0x00` XNADDR (`0x24` bytes), whose 6-byte MAC at `+0xa` **is
  the machine id**; `+0x28` the `0xc8`-byte properties (name strings at
  `+0x28` and `+0x48`); `+0xf8` its player count; `+0xfc + 4*user` the
  user's session player, -1 when empty.
- **Channel entry** (`+0x72dc + 0x14*peer`): `+0` valid, `+1` connected,
  `+4` the observer channel. Every send, timeout and drop loop on the host
  gates on `+1`. A peer with `+1` = 0 and channel -1 is never sent to,
  never dropped and gets no simulation view: that is a phantom's silence.
- **Session player**: `+0x00` 12-byte id, `+0x0c` peer, `+0x10` user on
  the peer, `+0x14` controller (-1 until set; the diff skips -1),
  `+0x18` requested configuration (`0x90`), `+0xa8` effective
  configuration (`0x90`; team at configuration `+0x7c`).

### 5.1 From the session to players, actions, session state

**From the session to players**, each frame (`0x68140` → `0x83220` →
`0x83f00`): the machine set is rebuilt **densely from the peers** (mask
`(1 << peers) - 1`, ids from each peer's `+0xa`), so a machine written
straight into `[0x4e8c20]+0x2c` would be overwritten; a peer is required.
The record build (`0x69260` → `0x68350`, `0x83610`) diffs the session
players against the simulation's copy and makes a kind-3 event for a new
one. Its apply (`0x85140`) refuses a machine id not in the machine set, a
user or controller outside 0..3, and a duplicate, then creates the player
(`0x14bc00` → `0x14c090`) and its simulation slot (`0x694c0`; mode 4,
remote, on the distributed server).

**Actions**: `0x6a3b0(sim, &machine_id, user)` (stdcall, `ret 0xc`) finds
the phantom's simulation slot; `0x847d0(slot, &action)` (`ret 8`) quantizes
and stores a `0x5c` action record in it, stamped with the tick. On the
server, `0x84990` reuses a stored action for up to 2 s (`[0x4ced44]`), so
a missed tick is harmless. The record (written at `0x187012..0x187083`,
validated by `0x14d0a0`): `+0` flags, `+4` yaw and `+8` pitch (finite),
`+0xc`/`+0x10` throttle (-1 to 1), `+0x14` trigger (0 to 1), `+0x20` and
`+0x21` weapon indices (-1 or 0 to 3, different), `+0x22` grenade (-1, 0,
1). The button masks, hold word and zoom selector are traced below.

**The session state.** A running game stays in state 5. The per-frame
membership code (`0x83220`, `0x8327b..0x83299`) goes on only in state 5
when the state is 5 to 8; otherwise it frees every simulation view and
drops the simulation's network state (`0x6b040`). 6 is set after the
leave message `0xd` (`0x61450`), 7 by `0x614a0` and 8 at `0x61646`. So
"in a game" is state 5 with a game loaded, never 6 to 8.

### 5.1 Hosting the game: modes, host peer, start hook

**Hosting the game** (traced 2026-11-19, `h2 disasm`). Runs
20261119-005001 and 20261119-014620 had their phantoms wait on "the
simulation is not the distributed server (mode 4 ...)". It was: the
phantom check compared the simulation world's mode with the game's
number for the distributed server, and the two number their modes
differently.

- **The game mode.** The game start (`0x6ec80`, `SESSION_GAME_START`, EDX
  the session; reached from the in-game life-cycle state's enter,
  `0x6eb90` → `0x6ec10` → `0x6ec80` on the squad session `[0x527364]`)
  picks it from the protocol at `S+0x4dac` and whether this console
  hosts, `is_host = [S+0x72d8] == [S+0x40]` (`0x6ee60..0x6ee71`):
  protocol 0 gives 1, 1 gives 2 + is_host, 2 gives 4 + is_host
  (`0x6eeba..0x6eecd`). It goes to the options `+4` and the game globals
  `[0x4e6948]+0xc`: 1 local, 2 synchronous client, 3 synchronous server,
  4 distributed client, 5 distributed server. **Confirmed.**
- **The simulation world's mode** (`[0x4cf77c]+8`, what the phantoms
  check). The new-map callback `0x67ee0` reads the game mode (`0x67efd`)
  and builds the world through `0x68c00`, whose jump table at `0x68da8`
  maps game 1, 2, 3, 4, 5 to world 1, 3, 2, 5, 4. So the world's
  distributed server is **4** and its client 5; its synchronous server 2
  and client 3. **Confirmed**, and every other writer and reader agrees:
  becoming the host (`0x6aee0`) sets world 4 and game 5 together;
  becoming a client (`0x6ae20`) sets game 2 or 4 and rebuilds the world
  through `0x68c00`; the after-tick step (`0x684e0`) does nothing in
  world 3 and 5 and sends updates in 2 (`0x69880`) and the distributed
  view in 4 (`0x69a00`, `0x69b50`); the action store (`0x847d0`) treats
  world 1 and 2 as authoritative. The names at `0x468c64` are the game's
  numbering; live-coop.md §1.1 applies them to the world's field, which
  reads a host as a client (`coop_probe::mode_name`,
  `agent_proto::sim::SessionKind::from_sim_mode` do the same).
- So both runs' lone Live host was the distributed server (game mode 5,
  world mode 4) all along. `bots::phantom::session::host_session` now
  takes world mode 4 (`WORLD_DIST_SERVER`; `world_mode` is the table), and
  a phantom that waits names the world mode: `(world mode 5, distributed
  client; the server's is 4; session host peer 0, this console's peer 0,
  peer count 1)`.
- **The session's host peer.** `+0x40` is the host peer and `+0x72d8` the
  local one (the earlier notes had them the other way round): a client's
  join (`0x61050`, from `0x5a07d`) puts the host's address in peer 0 and
  its own XNADDR (`0x4cf793`) in peer 1, then sets `+0x40` 0 and
  `+0x72d8` 1 and state 1; the host's create (`0x59d92`, `0x59d95`) sets
  both 0; entering state 5 as the host (`0x61390`) copies `+0x72d8` to
  `+0x40` (`0x6141a`); `0x58d20` (states 5 to 8) is the "this console
  hosts" test. Other writers of `+0x40`: `0x5e507` (handing the session to
  a sending peer, then state 3 through `0x612c0`), `0x5d2dc` (a client's
  membership update) and `0x5ff7d` (removal).
- **The start hook.** Before `0x6ec80`, with phantoms planned, protocol
  2, state 5 to 8, this console's peer below the peer count and no other
  real console in the session (phantom peers are not consoles), a host
  peer that is not this console's is set to it
  (`bots::phantom::session::game_start`). No run has needed it (run
  -014620 read host peer 0, local 0); it stays for a session that does.
  Each start logs what it read: `bots: game start: session state 5,
  protocol 2, peer count 1 (0 other consoles), host peer 0, this
  console's peer 0: game mode 5 (simulation world mode 4, distributed
  server)`, then `bots: game start: session as it is (why)` or the line
  that it made this console the host.

### 5.1 Recipe on the host, as built

**Recipe on the host, as built** (`bots::phantom::session`): the writes
the game's own functions make, done from the game thread before each
`RENDER_FRAME`, only while a multiplayer game runs, the simulation world's
mode is 4 (distributed server; the game's 5), the session state is 5 and
its class 4.

1. **Peer** (as `0x5fbd0(S, n, &xnaddr, 0, -1, NULL)` and `0x5f900`):
   `S+0x54` + 1; the peer cleared; its MAC (`+0xa`) the phantom's machine
   id; its name at `+0x28` (15 units); its four user slots -1. The channel
   entry: valid 1, connected 0, channel -1, `+8` and `+0xc` -1. Then
   `[S+0x4c]` + 1 and `S+0x7618` + 1, as the host's join (`0x5c720`).
2. **Player** (as `0x60000`/`0x600f0` and the add-player message
   `0x5f05d`): the lowest free slot of `S+0x111c`, refused when the id is
   in the session already or `S+0x1118` is at `S+0x4994`. Id (12 bytes:
   the xuid, then the machine id's last four), peer, user 0, controller 0
   (never -1: the diff skips -1), both configurations, `+0x138` 0; the
   peer's slot and player count; mask and count; both counters + 1.
3. **Configuration**: a fresh one with the phantom's name and look, its
   team at `+0x7c` (the host's when it has none) and the host's level at
   `+0x81`; no handicap and nothing else of the host's profile
   (`bot_config`; "A phantom's player configuration" below).
4. The game does the rest over the next frames: machine, player, slot.
5. **Actions.** In a hosted game the simulation gather (`0x69137` →
   `0x84990(slot, out)`) samples the phantom's pad once per simulation
   tick. It writes the record at slot `+0x2c`, stamps slot `+0x28`
   with the tick and supplies the record to the gather. This keeps a tap
   visible for its full simulation tick even when render and game rates
   differ. The slot has the player index at `+0`, remote mode 4 at
   `+8` and the six-byte machine ID at `+0x18` (`0x694c0`). The live
   player must have the same ID at `+0x14`, a nonzero salt at `+0`,
   a clear leaving flag (bit 1 at `+2`) and a unit datum at `+0x2c`
   (`engine::view::players::Player`). Game time
   supplies seconds per tick at `[GAME_TIME]+4` and the tick at `+8`.
6. **Removal** (as `0x5fe20`): the peer's players out (mask, count, the
   peer's slots), later players' peer indices and the session's peer
   indices (`+0x40`, `+0x72d8`, `+0x50`) down one, the peers and channel
   entries above it moved down, the last cleared, `S+0x54` - 1, counters
   + 1. Not done: its tail call `0x5a2e0`. Phantoms leave last first.

### 5.1 A phantom's player configuration and handicap

**A phantom's player configuration** (traced 2026-11-23 from the
disassembly, `h2 disasm`; tests only, no run yet). Each console builds
its own players' requested configuration (`0x18fd94`, from slot
`0x54e8e0 + 0xc70 * port`): `0x1537f0` clears the `0x90` bytes, then the
profile fills its fields. A phantom's is built the same way from a fresh
start (`bot_config`), so nothing of the host's profile reaches a bot.
Live telemetry showed why: a host who set a handicap and then removed it
left all seven Clankers with it (they copied the host's whole `0x90`
bytes), and they finished with 2 kills to the host's 24.

| Bytes | Field | Source on a console (`0x18fd94`) | A phantom's |
|---|---|---|---|
| `+0x00..+0x40` | name, 32 UTF-16 units | slot `+0xc1c` | the gear mark and its gamertag |
| `+0x40..+0x48` | look: four colours, model (`+0x44`, -1..3), emblem foreground and background, emblem flags; checked by `0x153750` | profile appearance, slot `+0x130` | `guest::bot_look` |
| `+0x48..+0x50` | the rest of the profile's 16-byte appearance block; no reader found | slot `+0x138` | 0 |
| `+0x50..+0x70`, `+0x70..+0x7c` | the account's Xbox Live team (clan), likely: 32 bytes and 12 bytes of a record an XOnline task fills (slot `+0x4ec` and `+0x4e0`, read through `0xabdb0`); 0 without one | slot `+0x4ec`, `+0x4e0` | 0 |
| `+0x7c` | team, -1 none (`0x14bc80` sets -1 for a team outside the game's team mask) | slot `+0x1fc`, clamped to 0..16 | its own, else the host's |
| `+0x7d` | **handicap**, 0 none to 3 | slot `+0x200`, set by the handicap screen's handler `0x2b4c45` (items 0..3, then `0x18fe9e` rebuilds the configuration) | 0 |
| `+0x7e`, `+0x7f` | host-side bytes, -1 unset, else clamped 0..0x7f | not set | -1 |
| `+0x80` | a flag `0x225880` sets on the console's own player after repeated game events (a count against a threshold in the network configuration, `[0x4cf754]`) and `0x225730` clears as it decays | slot `+0x46c` | 0 |
| `+0x81` | level, clamped to 0..7 (`0x14bc80`) | slot `+0x204` | the host's |
| `+0x84..+0x90` | host-side fields (an index 0..15 at `+0x84`, two `i16` at `+0x88` and `+0x8a`, a dword at `+0x8c`), all -1 when `+0x84` is out of range (`0x14bc80`) | not set | -1 |

- **The handicap.** `0x1588b0(player, mode)` returns, in a
  multiplayer game (mode 2), the player's shield scale: the variant's
  shield setting (game options `+0x1c4`: 1, 0 or 3) times the active
  configuration's handicap factor (player `+0x44 + 0x7d` = `+0xc1`,
  through the table at `0x15897c`): 1, 0.75 (`[0x45dc2c]`), 0.5
  (`[0x45dbbc]`), 0.25 (`[0x45dc20]`). The spawn sets the unit's shield
  vitality (`+0xf0`) to it (`0x14cc79`), the shield update compares the
  unit's shields with it (`0xd60c5`, `0xbcec6`), and a change of handicap
  in `0x14be90` calls `0x1583d0`, which clamps the shields down to a
  higher handicap's scale at once or starts a recharge for a lower one.
  These four are `0x1588b0`'s only callers. The handicap also scales the
  damage a player deals: the damage step calls `0x15dec0` at `0xd9cb8`
  with the attacker's player (`ecx`, damage data `+8`) and the victim's
  (`eax`, from `0xd7ae0`), and multiplies the damage by its result. It
  reads the attacker's handicap at player `+0xc1` through the jump table
  at `0x15e00c`, with the same factors 1, 0.75, 0.5 and 0.25. It then
  applies the attacker's extra-damage trait (`0x15ff50` with 2, times
  `[0x43ff08]`), the victim's resistance trait (`0x15ff50` with 3, times
  0.5), the victim's player `+0x194` (times `[0x45dc68]` while above 0)
  and, without friendly fire, the engine's team check `0x15e020`, which
  can make it 0. In Clanker Team Slayer game `09af571f9fec7f1a33ed37edb397a140`
  the bots carried the host's handicap 3. Their shields never passed
  0.25, and every bot SMG hit did 0.0156 against the human's 0.055–0.066.
  In game `c9a3aa1a3f867c298a6ae32b56e50399`, with no handicap, bot and
  human SMG hits both did 0.066 and BR hits both 0.086. Live telemetry's
  per-hit `amount` by weapon, bot against human, checks this.
- **What stays the host's.** Only the level: the matchmaking start
  (`0x66050`) refuses a player whose level byte is below the hopper's
  (`+0x5e4`), and the host's passes it. The team is the host's only until
  the lobby's balancing gives the phantom one. Everything a lobby or a
  game checks of a joining player is otherwise in its peer properties,
  which stay the host's (`sync_props`).
- **Kept none.** Each frame a seated phantom whose requested or
  effective handicap is not 0 (a seat taken back from an older run, a
  reclaimed peer) has both put back to 0, nothing else of the record
  touched (`write_no_handicap`); it is a repair like a name the host
  rebuilt, counted in `bots.stats` as `handicap_writes`. The host
  changing its own handicap later changes nothing of a phantom's.
- **Telemetry.** Live telemetry and the match summary show no handicap
  per player, so there is nothing of a bot's to hide there.

### 5.1 Phantoms in the pregame lobby

**Phantoms in the pregame lobby** (traced 2026-11-19 from the
disassembly, built; no run yet). The user's ask: "it won't let me add 7
clankers, only 3 max in a pregame lobby". The Count row was never capped
(`clankers.count` is 1 to 15, every value a choice); the lobby showed only
the 3 local bots because phantoms entered the session only once a game
ran. They now join the lobby as a real peer does.

- **What a real peer's join does on the host.** The join request message
  (`0x630f0`, from `0x63080`, which first asks the session's listener at
  `[S+0x78a8]`, vtable `+0`) calls the host's join `0x5c720(S, &xnaddr,
  n, ids, …)`: an existing peer with that MAC (`0x5f6f0`) is checked and,
  if stale, removed (`0x5fe20`); its players' reservations are claimed
  (`0x62f40`, `0x62eb0`); the peer is added (`0x5fbd0(S, count, &xnaddr,
  1, channel, &nonce)`: cleared, both names empty, map status `+0x88` and
  `+0x8c` 0, the hopper statuses `+0xac..+0xec` 0, the 8 bytes at `+0xf0`
  from the request, the user slots -1; then the channel entry `0x5f900`);
  both counters + 1. The matchmaking join (`0x6d4c0`) calls the same
  `0x5c720`.
- **Its properties.** Every console builds its own `0xc8`-byte peer
  properties (`0x658c0`, throttled by `[0x4cecc0]`): `+0` name (15 units,
  `0x4cd86a`), `+0x20` a second name (31 units, `0x4cd88a`; both set by
  `0x645d0` from the lobby's profiles), `+0x60` the map status
  (`[0x4cd8f8]`: 1 failed, 2 loading, 3 and 4 loaded), `+0x64` its
  progress (`[0x4cd8fc]`), `+0x68` `[0x4cf968]`, `+0x70` from `0x64480`,
  `+0x84` sixteen hopper statuses (`0x192df0` each), `+0xc4` from
  `0x73b10`. On the host `0x5a790` writes them into its own peer (`+0x24`
  = 1, `+0x28` the `0xc8` bytes); a client sends them as message `0x1a`,
  and the host's handler `0x5ed90` copies them into the sender's peer
  whole, sets its `+0x24` and bumps both counters.
- **Its players.** `0x60000(peer, user, id)`: a slot holding another id
  is cleared; an id already in the session is refused (`0x5f890`); the
  listener's veto (`[S+0x78a8]` vtable `+0xc`, the object at
  `0x527fe8`, set by `0x58ee0`; another creator `0x59ad0` sets its own)
  may refuse; without a reservation for the id (`0x62f40`) a free slot is
  needed: `0x62e30` = most players `+0x4994` less players `+0x1118` less
  the reservations held and not taken (`+0x7668`, 16 × `0x24`: `+0`
  held, `+1` taken). Then `0x600f0`, counters + 1. The phantom path
  writes the same records directly (never through the listener: the
  in-game phantoms of runs 6c never went through it either), and now
  counts the reservations as `0x62e30` does.
- **Effective configurations.** The host recomputes every session
  player's effective configuration from its requested one (`0x65cb0`,
  from `0x65770`) whenever the lobby's variant (`S+0x4db0`, a `0x130`-byte
  `variants::record`) or the membership number changes: the name stays the
  effective one once set (made unique by `0x7f660` the first time),
  `+0x40..+0x7b` and `+0x7d..+0x81` (look, Live team, handicap, level)
  are the requested ones, and the team (`+0x7c`) is the requested one
  clamped to 0..7 in a team game (`variant +0x48` bit 0) or the player's
  slot in free for all, with `+0x84..+0x90` the requested ones, when
  `[S+0x14]` is 1; otherwise the effective team and `+0x84..+0x90` stay.
  So a phantom's requested team is what the lobby shows; both are
  written.
- **What the lobby's start reads of each peer.** The start state's map
  check (`0x652a0`) skips status 0, but the countdown's end (`0x6e410`,
  life-cycle state 1 via `0x6e1e0`'s table at `0x6e2e0`) wants every
  peer's map status above 2, else it calls `0x5bf10` with the unready
  peers' mask instead of starting (`0x5a220(2)`); in matchmaking
  (`S+0x49a4` > 1) it asks `0x66050`, which wants each peer's status for
  the hopper `[peer + 0xac + hopper*4]` to be 1 (0 gives 14, 3 gives 2),
  each player's level byte (`+0x129`, the effective configuration's
  `+0x81`) at least the hopper's `+0x5e4`, and the party (`S+0x1118`)
  within the hopper's bounds (`0x193250`: type 2 at most 1, unranked free
  for all and teams the playlist's size, so a clanker playlist's bots fit).
  A phantom's peer with the host's properties passes all of these.
- **Recipe, as built** (`bots::phantom::session`, `Phantoms::tick`). In a
  pregame lobby (`lobby::pregame`: session type 1 or 2, state 3 to 8) in
  state 5, distributed (protocol 2: System Link or Xbox Live; Split
  Screen's session is protocol 0) and this console's (`+0x40` =
  `+0x72d8`), with no game running (`host_lobby`): steps 1 and 2 above,
  with the team and level read from this console's first session
  player's requested configuration (`config_in`; no player datum exists
  before the first game), then the properties (`sync_props`): this console's own `0xc8`
  bytes with the phantom's gamertag in both names, `+0x24` 1, both
  counters + 1, kept equal every frame (a map load on the host is the
  phantom's too). The join line is `bots: phantom G joins (peer N, in the
  pregame lobby)`.
- **Idle custom lobby.** The host compares the occupied peer and player
  tables, session incarnation, local peer and team-play flag before
  reconciling a settled roster. Changes to an effective profile or the
  host's map status take effect on the next frame, including edits that
  leave the membership revision alone. A pending join and a matchmaking
  party follow their respective join checks. The host looks up player
  datums only during a hosted game.
- **One bump per frame.** A frame's repairs of seated phantoms (their
  properties following the host's, a name or look the host rebuilt, a
  handicap put back to none, a team move) move the membership counters once, after all of them, not
  once per phantom, so the game rebuilds what depends on the membership
  once (`Phantoms::tick`, `Repairs`). `bots.stats` counts the frames that
  wrote and each kind of repair ([agent-api.md](../../runtime/agent-api.md#bot-combat)).
  Thirty writing frames in a row mean the game undoes a repair every
  frame; the log says so once per run (`bots: phantoms: the session's
  membership was rewritten on 30 frames in a row (...)`), with the
  property chunks that differed.
- **Teams.** A phantom with no team asked for (the Clankers page's
  bots; a Clankermatch's come balanced) goes, in a team game, to the team
  with the fewest session players of red, blue and any other a player is
  on, the lowest on a tie (`lobby_team`). Each lobby frame, a phantom on
  a team with two more players than the smallest moves there
  (`rebalance`, `bots: phantom G changes team A -> B to keep the teams
  even`), so a human changing team or the variant turning into a team
  game is followed. A difference of one never moves anyone.
- **Staying.** A phantom stays in the session from the lobby into the
  game (the game's player diff makes its player at the start, as for any
  remote peer) and back to the lobby after it. It leaves when the session
  is no longer one this console hosts live (`owned_session`: state 5,
  distributed, `+0x40` = `+0x72d8`): leaving the lobby, a host migration.
  The first frames of a game, which are not hosted yet, keep it. A
  phantom whose peer the game itself took out joins again (`... is no
  longer in the session: it joins again`), and a stale peer with its
  machine id is taken out before it joins, so it is never in twice.
  Lowering the count or Off removes it through `remove_peer` (§5.1 step
  6), in the lobby as in a game.
- **The switch.** `H2_PHANTOM_LOBBY=0` (or `off`, `false`, `no`) keeps
  the old way: phantoms join only a running game this console hosts and
  leave when it ends. On by default.
- **Not traced; for a run:** what a real client does with a phantom peer
  in the lobby (it opens channels to every peer's XNADDR: "Clients and
  the phantom's address" below now applies in the lobby too); whether the
  matchmaking state machine starts the countdown on a session-player count
  it polls (the lobby summary `0x7e210`) or only on join events; whether
  the lobby roster draws anything from a peer's second name (`+0x48`).

### 5.1 Phantoms in the game

**Phantoms in the game** (traced 2026-11-19 after the user's
Clankermatch: "the GAME OVER scoreboard lists all 7 clankers; only local
Clankers 1 to 3 were ever in the world; map loading got slower"; built,
no run yet).

- **Why they never spawned.** Each tick the player update `0x14d2a0`
  (from `0x14d3a0`, in turn from `0x137fe0`) sets every player's flag bit
  0 (`+2`) from `0x686e0(&player +0x14)`, "is this machine in the game":
  1 with no simulation (`[0x4cf770]` 0), 1 for this console's own machine
  (the world's `+0xd`), else the byte `+0x78` of the machine's
  simulation view (`0x6ad40`: the world's entities of kind `0x14` whose
  `+0x14` is the machine id), 0 with no view. The spawn pass
  (`0x14d3a0`) takes only players with bit 0 set, not leaving (bit 1),
  with no unit (`+0x2c` -1), and, on anything but a distributed client
  (`[0x4e6948]+0xc` not 4), whose respawn check `0x15dd40` passes; it
  spawns one of them at random through `0x14f2e0`. A phantom has no
  channel, so no view, so bit 0 stayed 0: its player was made (it was on
  the scoreboard) but never spawned. **Fix:** `MACHINE_IN_GAME`
  (`0x686e0`, its only caller is `0x14d2a0`) is hooked
  (`boot::bot_glue`): for a machine id that is a phantom of the books
  with a peer in the session (`Phantoms::stands_for`), it answers 1 at
  once. Every other machine goes to the game's own code.
- **Why loading got slower.** When a server's world starts (state 3 at
  `+0x18`, entered by `0x6b1f0`, which sets the wait mask `+0x20` to the
  whole machine set `[[0x4cf780]+0x1c]` and the time `+0x1c`), `0x69c80`
  holds it until every machine in the mask but its own (`+0x14`) has a
  joined view, or until the timeout `[0x4ced4c]` (a run-time value) has
  passed, and only then moves to state 4 (`0x6b2a0`). The machine set is
  the session's peers (`0x83f00`), phantoms included, and no phantom ever
  has a view: every game start waited the whole timeout. **Fix:** each
  frame `Phantoms::tick` first clears the phantoms' bits from the wait
  mask of a starting server world (mode 2 or 4, state 3;
  `session::unwait_phantoms`, machine `i` is the set's id at `+0x24 +
  6i`), once per start: `bots: phantoms: the game's start waits for real
  consoles only (machines 0x0007 -> 0x0005)`. Real consoles are still
  waited for.
- **Actions.** Unchanged: a spawned phantom's unit takes the action the
  bots write in its simulation slot (step 5 above). Until now no phantom
  had a unit, so none of it was seen. Input starts when the phantom has
  a spawned unit.

### 5.1 Phantom audio

**Phantom audio.** `crates/extensions/game/src/bot_glue/audio.rs` uses the
Phantom machine ID on the player datum to exclude any Phantom entry from the local-user map
while `SOUND_OWNER_USER`, `SOUND_LISTENERS_UPDATE` and `SOUND_UPDATE`
run. It restores the map on return. Human slots keep their original user
indices, including in splitscreen. The listener refresh runs both inside
the sound frame and from the effect update. Both entry points clear the
active flag for Phantom and unmapped slots.

The sound owner check leaves a Clanker's attached weapon and unit sounds
positioned. The retail mixer selects a human listener and handles
distance, panning and obstruction. A human's first-person sound uses the
retail owner check. The [agent's active listener list](../../runtime/agent-api.md#audio-voice-routing)
shows the slots and camera positions the game can select.

### 5.1 The user's Halo 1 run

**The user's Halo 1 run (2026-11-19: Live custom game on
`h1_bloodgulch`, 3 local Clankers and 4 phantoms, a crash a while into
the game).** Read from the log tail and the disassembly; no run yet.

- **"Every phantom got peer 4."** The raw log has all four `bots:
  phantom Clanker N joins (peer 4, in the pregame lobby)` (N 4 to 7),
  after `waits: not a lobby it can join (the lobby's session is not live
  (state 7, live is 5))`, and the start found 5 peers, 4 of them
  phantoms. The phantoms had joined before; state 7 made them unmade
  (no session this console hosts live) but a session not live is not
  written, so their peers stayed. Back in state 5 each rejoin found its
  own peer, took it out (the peers above it moving down) and added it
  again at the end: index 4 each time, every phantom's peer moved under
  the game. Their machine ids were distinct all along (`machine_id` of
  each handle, host-made bots and rostered alike). **Fix:** a phantom
  whose peer is still in the session with its player takes it back in
  place (`bots: phantom G is still in the session (peer K): it stays`);
  only a peer the game emptied is taken out and made anew. Tests
  `phantom::session::tests::lobby::host_made_phantoms_back_from_a_session_not_live_keep_their_peers`,
  `a_peer_with_no_player_is_made_anew`,
  `four_phantoms_in_one_frame_get_a_peer_each`.
- **The crash is not the peers.** `#GP` at `0x28c589` is `movaps xmm1,
  [eax]` in `0x28c530`, codec 8's (`blend_screen`) interpolated rotation
  (the codec table in docs/engine/engine-map.md), called from the
  evaluation setup `0x2798a0` (return `0x279d2c`). `eax` was
  `0x820f922c`: not 16-byte aligned, so `movaps` faults. The address is
  the stream (`[0x504480]`) plus `0x20` plus channel × stride plus frame
  × 16, so a blend-screen stream or its stride in the animation data that
  map loaded is off 16-byte alignment: the converted map's data, not a
  session structure. Every unit with an aim screen evaluates it each
  frame, bots or not; which animation it was is for the converted map's
  owners (h1convert / h1map), with the stream's address in the dump.
- **The roster refusal** (`no roster from Live (the server refused:
  0x80155100)`). Live HLE gives `0x80155100` for a 404 or 409 on `POST
  /v1/games/bots` (`live_hle::client`, `Worker::roster`). The roster
  handler's only 404 is `no session of yours by that id` (the session
  looked up by id with the caller as its host, `live_server::bots`); it
  has no 409. Not a 7-bot limit (the count is cut to the room, and none
  is a 400), and custom games ask without a playlist, which is allowed.
  Not an ordering race: the driver learns the session id only from the
  server's create reply (`Intent::Put` in `live_hle::sessions`), the
  create is applied before it answers (`Live::wait_applied`, on the
  Workers hub too, which is one `Live`), and the client's one worker
  thread runs the roster request after it, as the account that made the
  session (`Active::matches`). Tests hold that for both homes
  (`live-server/tests/bots.rs`, `live-hle/tests/server.rs`). What stays
  is a 404 the client could not tell apart: the same status comes from a
  route the server does not have (`no such endpoint`: a server older than
  the roster endpoint, or a Worker hub still on an older release, see
  docs/live/live-worker.md), which the client reported as a refused
  session. The roster request now goes out after a read of the session as
  its host confirms the server has it (a bounded wait under `Retry`),
  and the 404's message decides the result: a missing route is
  `CANNOT_ACCESS_SERVICE`, a missing session `0x80155100`. Either way the
  game log carries a `live: roster:` line with the status, the session,
  the account and the server's message. If the refusal persists, that
  line says which it is; for a missing route, probe `POST /v1/games/bots`
  on the deployed Live and redeploy the hub.
- **The local bots in the game.** `join gate port 1..3: screen 0xe7 ...
  signed_in true: waits` is the gate saying the bots are signed in
  already, so it has nothing to do: `waits` is every gate line out of a
  lobby. It is no sign they were left out of the game.

### 5.1 The action from a pad, timing, selectors, facing release, builder waits

**The action from a pad.** The offsets below are relative to one
`0x5c`-byte action, in little-endian order. The local and remote paths
can be followed from the generated functions for `0x185be0`, `0x186ab0`
and `0x84990`. The Default layout initializer (`0x218420`) assigns each
abstract action slot `k` a physical button. The input pass at
`0x217e40` copies each button's down count and elapsed milliseconds into
its slot. The control builder copies those arrays at `0x186570` and tests
the relevant `k`. Finally `0x187029..0x187083` writes the record that the
host consumes at `0x14d6d4..0x14d7e7`. The host translates that record
to unit controls at `0x1505f0`. This is the same Default layout local
Clanker profiles use. [Controller layout trace](../../input/controller-layouts.md)

| Default pad input | Record field and timing | Local evidence |
|---|---|---|
| A / South, jump (k0) | Flags dword `+0`, mask `0x2` while down. A also drives k14, which sets word `+0x1c` bit `0x100` on the first tick. | `0x1868cb..0x186918` (jump inhibit latch, `[0x510c50]+5`), `0x186996` (k14). |
| Left stick click, crouch (k10) | Flags `0x1` while latched. First click admits crouch if movement magnitude squared is below 0.25 or the biped check admits it. Release clears the latch. | `0x1865bf..0x18662b`, including the biped predicate `0xe4050` (object header kind zero at `+3`). |
| B / East, melee (k4) | Flags `0x30` while down. | `0x18691b..0x186942`. The host filters repeats at `0x14f70d`. |
| White / LB, flashlight (k5) | Flags `0x02100004` while down. The mask is shared across contexts. | `0x186898..0x1868cb`. A visible flashlight effect depends on game mode and unit. |
| RT, fire (k7) | Flags `0x00010040` while down, plus pressure (0–1) at `+0x14`. | `0x1867bc`, `0x186945`, `0x187058`. |
| LT, grenade or left gun (k6) | Pressure at `+0x18`. Flags `0x00224800` while active plus `0x04000000` for a grenade. Dual wielding suppresses the grenade bit. Letting go rearms a trigger still down when the left weapon was dropped. | Default LT slot selected at `0x217ed3..0x217f01`. Dual check and four LT flags at `0x186751..0x18684a`, grenade bit at `0x18684d`. The host dispatches a grenade action at `0x14fe58..0x14fe99`. |
| X / West, reload or use (k2) | Word `+0x1c`, bit `0x01` while down, also `0x02` after the hold threshold. | `0x1869de..0x186a2c`. On press the host tries the contextual action and then reload (`0x14f86d`, `0x14f9d0..0x14fa38`). A held X takes a seat or board target through `0x151c90`, or a weapon through `0x151940` (`0x14f89d..0x14f8fb`). The exit path tests pressed or held at `0x14fae1`. |
| Y / North, switch weapon (k3) | Word `+0x1c`, bit `0x10` down, also `0x20` after the same threshold. An unused release selects the next eligible primary at `+0x20`. | `0x186a2c..0x186a7d`, `0x186bcb..0x186c0e`. The local builder calls the four-slot inventory search `0xcdeb0`. A handled Y interaction sets player flag `0x20` at `0x14f922`. |
| Black / RB, swap grenades (k1) | Signed word `+0x22` = requested type (0 frag, 1 plasma, −1 none). Cycle once on press, using whichever types have stock. | `0x18695f` tests down count equal to one. `0x186c46..0x186ce9` checks unit `+0x23d` and the two signed counts at `+0x23e`. `0xc6e41` applies a valid selection. |
| Right stick click, zoom (k11) | Signed word `+0x24` = zoom level (−1 off, 0 first zoom). A tap cycles once. On a long hold, release restores the level from before the click. | `0x18662b..0x18670c` forms the click and down signals. `0x186d45..0x186de0` checks eligibility, calls `0xc8960` to cycle and restores after more than 15 subsequent held updates. `0xc6e51` writes the requested level to unit `+0x241`. |
| D-pad left / right (k12/k13) | Flags `0x80` / `0x100` while down. | `0x186867..0x186895`. |

**Tap and hold timing.** The Default pad conversion preserves face-button
and trigger pressure. The XInput reader at `0x124c53..0x124cc9` tests
the last pressure byte against a per-button threshold and uses the new
byte to update that threshold. A phantom uses the same pressure history
and publishes the new sample for the current tick. While down, the
threshold can rise to
pressure minus 32. While up, it can fall to pressure plus 64. Stick clicks
and the D-pad are digital. X and Y send their levels on every tick. The host
compares them to player `+0x3c` and derives the press/release bits in
`0x14f722..0x14f81a` (X `0x04/0x08`, Y `0x40/0x80`). The threshold comes
from the active globals' player-control block, `[[0x4e034c]+0xf4]` at
`+0x7c`, in seconds. The local builder multiplies by 1000, truncates,
and compares to the pad's accumulated milliseconds. A phantom measures
the hold in simulation time. Exact agreement at the threshold with an
asynchronously polled physical controller is **[guess]**. The named button fields above are traced.

**Selectors, target and motion.** At `0x187036` the local builder packs a
weapon-set generation into `+0x1e` (u16) and the requested primary and
secondary indices into `+0x20/+0x21` (i8, −1 or 0–3). Unit `+0x212/213`
are the actual hands and `+0x216/217` are the requests. At `0xc6e2b`
the unit accepts the two indices only if the generation matches unit
`+0x214`. An objective weapon returned by `0xcfe40` can override the
requested primary (`0x186c0e`). The weapon handles at unit `+0x218`
also feed the zoom check for a deflected aim stick (`0x100f70`). Player
flags at `+2` tell Y release whether an interaction consumed the press.
Weapon, grenade and zoom ranges are checked by `0x14d0a0`.
The local interaction finder `0x151110(player, result)` returns the
12 bytes copied at `+0x28`. These include the interaction kind (i16),
detail at `+0x2a` (seat for a seat interaction), object datum at `+0x2c`
and melee target at `+0x30` (`0x151d41`, `0x15166c`, `0x14fd37`).
An empty target has kind zero and both datums −1.

The separate shot-assist block starts at `+0x34`. The local action builder
copies nine dwords there at `0x187074..0x187083`. Previously the phantom
always supplied the empty block from `0x14d040`. Both strengths were
zero, so the barrel's target check at `0x1a6a8b..0x1a6aa5` skipped
shot magnetism even when the reticle was close enough for a local player.

Both seats use the retail XInput reader `0x124c00`, profile/layout routines
`0x190c34` and `0x217c40`, and the full player action builder `0x186ab0`.
The builder indexes the user's 0x94-byte control history by player +0x28.
It calls `0x185be0` for look response (including acceleration, slowdown
and follow) and `0x1a4900` for weapon parameters and target search
(`0x1a50a0`, `0x1a5370`). It writes yaw/pitch at +4/+8, trigger flags
at +0 and the selected target and strengths at +0x34..+0x57. The host
validates the returned record at `0x14d0a0`.

The retail tables have four entries. At remote gather, a second executor
uses a copy-on-write snapshot of the host world. In that view local user
and controller zero, the two player maps at `[0x4e8c20]+0xc/+0x1c`,
and the player's +0x24/+0x28 all refer to the phantom. Its virtual XInput
handle supplies the arrived pad to `0x409b6c`. The seat brings its own
raw input, button and control history between ticks. A fresh binding
clears the previous user's target and strengths before retail binds the
unit at `0x1872d0`. The private profile slot has the profile bit and
local-user index zero even if the host has nobody signed in. `0x1557c0`
supplies the unit or vehicle camera origin for the observer at `0x4e9c8c`.
Only the validated action crosses back to the host. A repeated gather
in one tick reuses that action. A phantom samples once per game tick
and advances the button clock by elapsed simulation milliseconds.

**The facing release.** One write of the builder's must reach the live
world. When something other than the control turns a biped, such as a
melee lunge, the biped's facing update `0xe01d0` sets bit 5 (`0x20`) of
biped flags `+0x348` (`0xe0c47`). For a biped on foot with that bit set,
the control update in `0x186ab0` reseeds the control yaw and pitch from
the unit's aim `+0x168` with `fpatan` and then clears the bit
(`0x186f8b..0x18700b`). The reseed overwrites the look delta that
`0x187510` added from the right stick. For a local player the clear lands
in the live unit, so the reseed happens once. The phantom's private build
discards it. With the bit left set, every later build reseeds the look
from the aim and drops the right stick, so the bot's aim freezes until it
dies, while its movement, fire and other buttons still work. After a
built action, `slot_action` in `crates/live/bots/src/phantom/action.rs`
therefore clears the bit on the phantom's live biped and counts it in
`bots.stats` `maintenance.facing_releases`. Film replay takes the same
path. A missed build leaves the bit set for the next build. The test is
`a_built_action_releases_the_bipeds_facing_flag`.

In the Clanker Team Slayer game `09af571f9fec7f1a33ed37edb397a140`,
before the release, every bot's aim stayed within 0.001 of one vector
from its first melee until death or the end of the match. That covers
31% of all bot frames. In those frames the bots scored 2.6 hits a minute,
against 19.3 in the rest, and their median angle between walking and aim
was 90–115°. Live telemetry shows the release working when no bot's aim
stays unchanged for more than 60 frames after an action that includes
`melee`.

The first 12 sims with a phantom seat stopped between ticks 91 and 1945. In the
Lockout log, `0x2d6307` is the return from kernel ordinal 234 inside
XAPI's `WaitForSingleObjectEx`. Thread 3’s `0x120c4f` is the return from
waiting on the async I/O work semaphore at `[0x4e0360]`. The two stacks
identify the same wait wrapper. The log does not identify the object the
private builder waited on.

The builder’s inventory check (`0xcdeb0`) can look up a weapon animation
at `0x1cc3f0` and predict its resources at `0x1236f0`. A cold read
allocates a job under the pool mutex (`0x1208b0`) and queues it
(`0x1209c0`). A pose lookup can also demand animation bytes through
`0x1dd82b` → `0x1237e0` and wait for an outstanding read at `0x1237a0`.
Which branch triggered the original refusal is **inferred, needs a run**.

The private builder skips animation prefetch. For a demanded resource it
uses the retail getter if the bytes are resident. On a cache miss or
pending I/O it checks the animation and resource against their live graph,
calls the retail getter on the scheduled game thread, then retries the
same pad once against fresh RAM. The live worker can complete that job.
A further miss or refused call discards the private build. The hook logs
the first failure of each kind and revalidates the same unit’s previous
action for one tick. With no eligible action it returns zero, which leaves
the player out of the gather mask. Repeated gathers reuse that tick’s
decision. A later successful build resumes normal input.

The image-backed test `retail_pad_sequence_matches_local_angles_target_and_fire`
uses `H2_DISC`, its matching `H2_RETAIL_MODULE`, and a multiplayer checkpoint
in `H2_PAD_CHECKPOINT`. Capture the checkpoint with the agent API's
`state.save` while port zero is on foot, the menu is closed and an opponent
is inside the weapon’s assist cone. The test compares the full actions and
requires movement of the aim angles, an object with a controlling player, a
positive assist strength and a fire bit.

### 5.1 Phantom ping details

normal distributed player updates.

A sim can set `phantom_ping_ms` to model a remote controller. With a
positive round-trip time, the host samples a one-way delay of half the
ping plus uniform jitter bounded by `phantom_jitter_ms` (default 4 ms),
rounded up to simulation ticks. A zero or omitted ping delivers the
current report immediately. In delayed mode the latest arrival supplies
the stick position. The host delivers each trigger's pressure changes
in send order, at most one per tick after arrival. A new unit or a break
in the stream clears buffered reports. The host always builds and stamps
the retail action at gather with the current weapon and facing.

### 5.1 Midship replay

**Midship replay, 2026-11-19.** In game
`531b5da3ceab388417c044afcf18e01a`, the 2,672 recorded frames
attribute 2,231 shots to the four phantoms. Fourteen bullet hits and
nine melee hits from them dealt positive damage to opposing players.
The three local bots recorded 1,372 shots and 236 opposing bullet hits.
A team with no kills has no assisted kills either. The damage reports
establish that phantom fire reached the host.

The old phantom adapter treated a half-stick on either axis as about
54°/s. The local Midship table gives about 9°/s yaw or 5°/s pitch
for that input before aim assist. One phantom's pitch swept from +34°
at tick 1284 to −26° at 1300 while the human's bearing stayed within
6° of level. Using the local response removes this excess gain from the
delayed control loop. Post-change hit rates require another hosted match.

### 5.3 CPU per bot: per-tick costs and estimate

Per bot, per tick:

- perception: about 10 to 20 rays, cheap against the BSP;
- combat decisions and controller input: every simulation tick;
- path following: one A\* per re-plan, on a graph of 1 to 10 thousand
  sectors;
- aim: constant cost.

Our estimate is under 0.1 ms per bot per tick on the host's CPU. The game's
own cost for the bot's player is the same as for a human: one more biped
simulated. A phantom adds a remote player's distributed updates to
each client. A real lobby of the same size has the same update count.
Measure in a run (§11).

### 5.4 Waiting for the black screen: opacity addresses and fade arithmetic

At match opening, the host reads the black overlay's opacity for each
mapped human player from `[GAME_ENGINE] + 0xe4 + 4 * user`. Map
activation sets it to 1 (`0x1585a4`), and the renderer draws the overlay
while it is positive (`0x1599b0`). The opening gate clears once all
those views are clear. Later spawns use each phantom's own clock.

Retail keeps opacity for local users. A phantom uses its own spawn age
from the player record at `+0x194`, which starts at the guest tick rate
(`0x14f68e`) and counts down by one per player update (`0x15bb4f`).
It models the black fade retail starts at 1 on a local spawn
(`0x1584e6`) and reduces by `3 / tick_rate` per simulation tick, down
to zero (`0x15c4d6..0x15c4f9`). At 30 Hz this takes ten updates.
A playerless System Link or dedicated host has no local views. Its
opening gate applies the retail hold through `round(1.5 * tick_rate)`
(`0x15c4c6`). Since action gather precedes the update, the gate opens
at tick 56 at 30 Hz if the phantom's own fade has cleared.

### 5.5 Brains off the game thread: history and measured sample

the game thread. Three modes, set with `Phantoms::set_think_mode`, or
from the environment for the process's phantoms:

### 5.5 Brains off the game thread: phases, radio order, batch membership, pipelining

A batched sample has three phases (`PadSource::observe`, `think`,
`conclude`; a plain `pad` runs all three in a row):

1. **Read**, on the game thread in slot order. Each phantom reads the
   guest's state, the kill feed, recent shots, the loaded map and the radio
   into its own staged observation. Every guest read happens here, before
   any think starts.
2. **Think**, on the workers. Perception filtering is already done.
   Memory, decisions, routes, aim and the human-input controller run here.
   A think touches its own brain, the shared map data (`Arc`, read-only)
   and thread-local scratch: the A\* workspace, grid visit marks and
   the tuning scope. Each of these is reset per query. No think reads
   another brain or guest memory.
3. **Conclude**, on the game thread in slot order. Each brain publishes
   its radio calls.

Radio order is kept. Each brain prunes calls with its own deaths, so a
serial gather could prune a call published by an earlier slot in the
same tick. A batch records each listener's place and the deaths it pruned
with (`radio::Seat`, `Radio::publish_seated`). A late publish is then
pruned only by later slots, and only while the board was on the call's
tick for that slot. That is what the serial order did. Batched and serial
give the same pads, minds and radio at any thread count.

The batch covers the phantoms the gather would sample this tick. These
are joined phantoms whose own slot is remote mode 4 with a listed player
of its machine ID and a live unit. An interrupted stream is reset first,
as at the slot. Slots go in address order. The batch is held per hosted
game and tick, and later slots take their pad from it. A phantom outside
it (the checks disagreed) is sampled whole at its slot. Building the
retail action in a private executor writes nothing a perception read
looks at. Between two slots, main memory only gets the slot's action
record and stamp, the `0x14d0a0` check of `out`, and possibly an
animation-cache fill. Reading every phantom at the first gather therefore
reads what each later slot would have read. The sim's state hashes should
confirm this (§11).

**Pipelined**, the thinks staged at tick N run while the guest simulates
N, and their pads are tick N+1's: the bot acts on what it saw a tick ago.
Each completed think carries its pad, transport timing and observed target
range together. Serving later slots can proceed while workers compute the
following tick’s reports. With immediate
transport, the pipelined brain's pad reaches the host one tick later than
the serial brain's. With simulated transport, the uplink credits that
tick against the requested delay (`Uplink::think_ticks`). A script or
recording supplies a current pad and gets its full configured delay. A think from before an input reset (generation) or a tick
gap is dropped, and the seat sends a neutral pad. When the hosted game
ends, the host discards queued decisions and unpublished reports. Workers can
finish an old decision while the shell draws. If the next match already needs
input, the host supplies neutral pads until those workers finish and resets
each source before sampling it again. During the fade (§5.4), brains observe
and plan, the uplink sends a
neutral pad while it is up, and the first clear tick sends the newest
pad at once.

### 6. Phantom players in the game record: connection bars detail

RTT and three traffic metrics, so a zeroed rate field can give one red
bar even at low latency. `crates/extensions/game/src/lobby_quality.rs`
keeps the game's badge visibility and uses the player's session peer for
its bar count. The local console and phantom machines get six bars.
Remote peers use the last acknowledged round trip from the game's
reliable packet queue (`0x96dc7`, timestamp at `+0x95c` and milliseconds
at `+0x960`). One bar is the fallback while the viewer has no sample for
that peer. The scale uses retail's latency calculation, interpolating
from six bars at 60 ms to one at 800 ms with nearest-even rounding.
The pregame lists show voice activity. The agent reads the same
connection ratings for their members through
[`ui.lobby.players`](../../runtime/agent-api.md#ui-what-the-menus-show).

### 6. Phantom players in the game record: BOT mark and game-only stats detail

The web shows a small BOT mark beside a bot's name (the live scoreboard,
the carnage report, the map legend, the map and 3D viewer labels, the
viewer's follow list). A bot with an xuid (a bot account, §9.5) links to
its player page, BOT mark and all; a host's own Clanker with no xuid links
nowhere. A bot keeps the name the game sent.

**Bots are game-only.** The game record keeps every bot with its kills,
deaths and score, and every knockout between a bot and a human. Career
stats do not: the conservative choice. A human's kill of a bot is not a
career kill, and a death to a bot is not a career death (nor do they count
toward sprees, weapons or medals); games, wins and time played still count.
This holds in the server's stats summary (`live_server::stats`, both homes;
the web mock's fold mirrors it) and in the web's derived service record
(`derive.ts` `careerKills`, `serviceRecord`). Bots get no stats-summary row
and are nobody's "played with". A bot account's own page (§9.5) is the
one place its knockouts count: its service record counts all its kills
and deaths, against humans and bots alike. Ranks are unchanged: a matchmade game ranks
accounts only, and a rostered persistent bot's own rank is sbmm-ranked's
(§9.4).

### 7. Clankers in custom games: aggression and aim style effects

A host can change Aggression and Aim style on an existing custom-game
roster. The next input sample picks up the edit. Normal aggression uses
each bot's seeded traits and current mood. Calm subtracts 0.35 from its
drive to press, Wild adds 0.35, and the difficulty's awareness and the
fight still decide whether it advances. Steady uses the difficulty's
usual hand movement. Twitchy raises that hand's stick engagement by 45%
and the overshoot on a fresh flick by 35%. The bot keeps its skill's
tracking error and lead. Reaction timing stays within the skill's window.
Clankermatch uses its roster's difficulty and each gamertag's profile. Simulation scenarios use their declared tuning.

### 7. Clankers in custom games: Mixed and settings store notes

**Mixed** gives each bot a difficulty drawn around Normal. Per-bot
difficulty can come later, as a sub-page per bot; the first page does not
have it.

The settings are kept in the settings store through `setting_defs`, like
`HALO1_SANDBOX`. They persist and appear in `h2 settings`. The page is a
user of the kit. If it needs anything the kit lacks, that goes to the
ui-builder agent, which owns `ui_kit`, through the coordinator. As far as
read, `OptionRow`'s two-choice rows need a many-choice sibling for Count
and Difficulty; that is the one kit ask expected.

### 7. Clankers in custom games: leaving the lobby hooks and agent check

`game::bot_glue::party_leave` scopes the squad peer count at `0x199f34`
to the lobby Back and pause Leave Game checks. It uses the live squad and,
in matchmaking, the target membership to identify the sole human. The
leader picker and the automatic successor check both validate squad
player slots at `0x19a951`. A slot owned by a phantom machine fails
those checks.

The confirmed leave paths at `0x199e3c` and `0x252150` remove phantom
players and peers through `bots::phantom::Phantoms::retire_solo_session`.
The pause and lobby Back calls at `0x232ccc` and `0x25125c` target the
frame at `0x199e3c`, which follows the tail jump at `0x199e37` from
`UI_LEAVE_SESSIONS`. The hook verifies the frame prologue at that entry.
Retirement also drops their matchmaking roster entries and holds off new
seating until the departing session closes. The Clankers page settings
still apply in the next lobby.

Use `h2 agent ui` to check for `dialog.name = party_leader_picker` while
driving Back through the agent API. A custom lobby with a solo human
leader and Clankers exits without that screen. A party with two humans
on separate consoles and Clankers keeps the leader picker with the other
human as its only choice.

### 8.2 Decided by the user: bot identity glyph, build and name budget

2. **Bot identities.** Bots get made-up gamertags, marked by a glyph that
   says "bot". We pick **U+2699 GEAR** (⚙). Common system fonts on the web carry
   it, and it is in the Basic Multilingual Plane, as Halo 2's UTF-16 names
   require (Repo: keyboard-charset.md). The game's fonts lack it; the
   keyboard's dingbats (star, flag, check, ballot x) are the only symbols
   close to it, and none of them reads as "bot". So we draw our own glyph
   and add it with `font::patch` (Repo: `crates/formats/font/src/patch.rs`, already
   used by `boot::icons` to add our icon glyphs at load). No disc glyph is
   copied.

   The gamertag itself stays inside the Live rules: ASCII letters, digits
   and single spaces, 1 to 15 characters, as `live_proto::Gamertag` checks.
   The glyph is not part of the account name. It is drawn before the name
   wherever a bot's name appears:
   - in the game, the host writes `⚙ ` plus the tag into the phantom's
     player configuration. The lobby, scoreboard and carnage report
     draw that name.
   - on the site, it is drawn from the server's bot flag.

   The server knows which accounts are bots (§9). Every client patches the
   font, since every client is our recomp. A box on a stray unpatched font
   slot is the failure mode.

   **Built (2026-11-19).** `font::gear` draws the gear in code (a wheel,
   a hole and eight teeth; six below 14 pixels), square, as tall as the
   font's 'H' above the baseline (seven tenths of the ascent without
   one), standing on the baseline with a one-pixel gap before it.
   `boot::bot_mark::GearRewrite` adds it to **every** file under
   `maps\fonts` as the VFS serves it, after the icon sets
   (`boot::icons`), unless the font has its own U+2699. The font-slot
   audit is therefore not needed for the mark: whichever slot a field
   uses (keyboard-charset.md, "Which font draws which field", still not
   traced), its file has the gear. A font the rewrite cannot patch (over
   the VFS rewrite limit, or one that does not parse) keeps a box there
   and says so with `H2_BOOT_VERBOSE=1`.

   The name budget: the configuration name holds 15 UTF-16 units and the
   NUL (`CONFIG_NAME_UNITS` 16). `bots::phantom::session::marked_name`
   writes the gear, a space and the tag; a 14-character tag loses the
   space (the glyph's own gap stays), a 15-character one also its last
   character. Host-made names are at most 14 ASCII characters and fit whole. The game record names a bot without the mark
   (`live_proto::bots::unmarked_units`, in `live-telemetry`'s player
   read), so the stats read model keeps the account's gamertag and the
   site draws its own mark from the bot flag. No player can type U+2699,
   so only a bot's name loses anything.

### 8.3 Menu row and playlists: layout, press and cursor detail

A row that is off leaves the list and the rows under it close up
(`custom_games::Menu::layout`): without the co-op row
(`H2_NO_COOP_ROW=1`) Custom Games is 3 and Mods 7. The lobby switches to
the mode a row asks for at its first panel update
([party-lobby.md](../party-lobby.md), "The modes"). `h2 bots test` drives
Custom Games at row 4.

A moved retail row's press rewrites the press record's row (`[[esp+8]]`)
to its retail row and runs the original, as the co-op row does; after it
returns, the cursor row `0x510a18` is set to the row shown (the retail
press stored the retail row there). The browser's and co-op's presses
store their rows too (`custom_games::remember_row`, rows 0 to 7; Mods
opens over the menu and stores none). The menu restores its cursor only
to rows 0 to 3 (`0x231195`: `cmp eax,4; jge`), so for rows 4 to 7 a
`before` on the menu's build `0x00231170`
(`engine::points::LIVE_MENU_CURSOR`, vtable `0x00458b48` slot
`0x00458bec`, thiscall, no arguments, plain `ret`) does the build itself:
`[0x54e7bc] = -1`, `byte [0x54e7b8] = 0`, `0x22ee92(screen,
0x1901fc())`, `0x22f672(screen)`, then `0x24c591` (EAX the list at
screen `+0x818`, the row on the stack, `ret 4`) with the stored row
(`custom_games::LiveMenuCursor`, feature `live-menu-cursor`). Rows 0 to 3
and "none" (-1) go to the retail build. The co-op row's retail-lobby
press (which runs retail row 2's press, storing 2) now sets its own row
after it, as the moved rows do. Every Live menu press sets `clankermatch::active` (Clankermatch)
or clears it (any other row). `h2 bots test` and `h2 coop test --drive`
navigate by the new rows (`agent_proto::sim::menu`, `bots_drive`).

### 8.3 Menu row and playlists: playlist trace, hook, Quickmatch, Switch To detail

**The playlists (traced 2026-11-19, confirmed from the disassembly).**
OptiMatch's press (`0x231025`) opens the playlist screen through
`0x149f1e` with loader `0x00230c8d` (screen `0xdb`; vtable `0x00458c10`;
the list handler at `+0x614`, constructor `0x002b1e08`, vtable
`0x0045b138`, datum array "playlist saved game file list" `0x0045b190`).
Its add-items (`0x002b1ed5`) calls the item builder `0x002b1ff4` (EDI the
handler), which walks descriptor indices 0..16 and adds a row for index
`i` when:

1. `0x00192db0(i)` (ECX `i`, AL the answer, no stack arguments; the
   range, two gates, then a tail `jmp` to the validity check
   `0x001934f0`) is true; and
2. `0x00194610(i)` (list whatever the rank) or the player's rank is at
   least `0x001945c0(i)`; and `0x001943d0(i)` loads the hopper's block.

Each row's datum keeps `i` at `+4` (`0x002b20cb: mov [eax+4],ebx`). The
press (`0x002b2149`) reads it back (`mov edi,[eax+4]`) and starts
matchmaking through `0x002b2181`; the row text (`0x002b1f5f`) and the
initial cursor (`0x002b1eef`, the hopper at `0x00510990`) read it too. So
rows are compacted but never renumbered into indices: skipping a hopper
leaves no hole and breaks no mapping.

**The hook** is a `before` on `0x00192db0` (`engine::points::HOPPER_LISTED`;
its only caller is `0x002b2064`, return address `0x002b2069`):
`clankermatch::listed(i, marks, active)` is "the hopper is a clanker
hopper exactly when the Clankermatch row opened the screen"; a hopper of
the other kind gets AL 0 at once, the rest go to the game's own check.
The marks are the server's (`GET /v1/hoppers/clankers`, §9.4 item 1),
which the HLE asks for at each logon. With no marks (an older server) the
OptiMatch list is retail and Clankermatch's is empty.

Other readers stay unfiltered: the best-skill pass `0x00065530` and the
stats rank cache use `0x00192df0`/`0x001934f0` directly.

**Quickmatch (built).** Quickmatch (row 0, `0x00231045`) starts the
hopper `0x001902de` answers (ECX the local player; `[0x54e9dc +
player*0xc70]`, or -1 when flag bit 4 of `[0x54e8e0 + player*0xc70]` is
clear; its one caller `0x00231056`), through `0x001943d0` at
`0x00231066`, when the saved hopper is usable. The Live menu opens the playlist picker for an unset, out-of-range or Clanker selection. A playlist press
saves its hopper there (`0x002b21e3`: `push edi` (the hopper), `push
[ebp+0Ch]` (the player), `call 0x001902fc`, `ret 8`; its only caller).
Two hooks keep Quickmatch on human hoppers
(`clankermatch::PlaylistFilter`):

- a `before` on `0x001902fc` (`engine::points::PLAYLIST_REMEMBER`)
  returns at once (`ret 8`, nothing saved) during Clankermatch or for a
  clanker hopper (`clankermatch::remembered`), so the human playlist
  picked before stays;
- an `after` on `0x001902de` (`engine::points::QUICKMATCH_HOPPER`)
  answers -1 for a saved clanker hopper (a profile saved before the
  server marked it; `clankermatch::quickmatch_hopper`).

With no marks (an older server) both are retail.

**Switch To: Clankermatch (built 2026-11-19, the user's ask).** The
playlist screen's second opener (`0x002bbf06`, from `0x002bba93`) is the
lobby's **"Switch To: Matchmaking"**: a Game Setup (squad settings)
row whose press, `0x002bba8c` (EAX the press's player chain, the list
handler on the stack, `ret 4`), opens the playlist screen for the
chain's player and closes the dialog (`0x0022eeee`, `0x0022e957`). The
list constructor (`0x002bb4d2`) gives it action 8 in lobby kinds 4 (2,
3, 7, 8) and 5 (Change Map, Change Rules, Quick Options, Switch To:
Co-Op Game, Switch To: Matchmaking), and action 9 in kind 6 (9, 6); the
row-chosen jump table (`0x002bb884`) sends 8 and 9 to the same press.
`clankermatch::SwitchRow` (feature `clankermatch-switch`) adds **"Switch
To: Clankermatch"** (action 12, `clankermatch::SWITCH_ACTION`, past the
Halo 1 Options row's 10 and the Clankers row's 11) right under it, while
the server has clanker hoppers (`clankermatch::wants_switch_row`,
`switch_placed`), drawn with `custom_games::SWITCH_CLANKERMATCH_TEXT`
(string id `ID_BASE + 0x3b`, the Live menu rows' family). Its press sets
`clankermatch::active` and calls `0x002bba8c` itself, so the party goes
to the clanker playlists and on to the empty search and the fill (§8.4),
as the Live menu's row does. Retail's Switch To: Matchmaking now clears
`active` first (`clankermatch::switch_press`), so it always lists the
human playlists, whatever the Live menu chose last. The log says
`clankermatch: Game Setup list: Switch To: Clankermatch row added (N
rows)` at each Game Setup, then `clankermatch: Switch To: Clankermatch:
opening the clanker playlists` and `clankermatch: the playlists are the
clanker hoppers` at the press. The party lobby's Game Setup then drops
every Switch To row, this one with retail's (`lobby_mode::arrange`, the
constructor's outermost `after`, so it runs once this row is added):
the GAME MODE picker changes mode, and its MULTIPLAYER mode is the one
matchmaking lobby. There, action 9 reads Change Playlist... and lists
the clanker and the human hoppers on one page (`clankermatch::Scope::All`),
so the playlist picked is the Clanker or non-Clanker choice
([party-lobby.md](../party-lobby.md#the-modes)).

### 8.4 Search and lobby: the four steps in full

1. The search returns no sessions after the delay, a uniform 4 to 20 s
   (not yet drawn from the playlist's population). The game widens and
   searches again after each answer (live-matchmaking.md §7.3), so the
   delay is the search run's: the first call of a run draws it, every
   call answers no sooner than the run's start plus it, and a call more
   than 10 s after the last answer (or of another hopper) starts a new
   run. The game then hosts, as OptiMatch does when a search finds
   nothing. The log says `live: clankermatch: searching clanker playlist
   P: no one to find; the console hosts in N s`.
2. The host's Clankers driver sees it host a clanker hopper's lobby
   (`Hle::clanker_hosted`: the advertised session's hopper is marked) and
   fills it (`clankers::Driver::tick_match`, twice a second): it asks
   Live for one roster of the whole lobby (`POST /v1/games/bots` with the
   clanker hopper's index as the playlist (`Rosters::playlist`, which
   falls back to `Hle::clanker_hosted` when no server match names one),
   no skill, so the server sets skill from the party's level, §8.2.3;
   `teams: true` in a team playlist) and seats the bots one at a
   time on `clankermatch::schedule` (each 0.5 to 3 s after the last,
   seeded by the session id), up to the playlist's most players less the
   party. The party is the squad session's player count
   (`bots::lobby::party_players`, `+0x1118`: every console of the party
   and its guests, never a phantom), read twice a second by the frame
   hook (`Driver::set_party`) and followed until the first bot sits; one
   person while it cannot be read. Team Skirmish is three bots for one
   person and two for a party of two: a bot for every human but the host
   would take the seat of the party's second console, whose join the full
   session then refuses. The log says `clankermatch: 4 seats, the party
   of 2 people, 2 bots` (or `the party of N people fills clanker playlist
   P's N seats: no bots`). Each bot joins the match's target session as a phantom peer
   with its rostered account and skill (§5.1, "Phantoms in the pregame
   lobby"). The driver waits for the roster before seating any bot.
   If Live refuses the roster, the host supplies the identities.
   In a team playlist each phantom without a roster team goes to the team
   with the fewest players, then the lowest total skill
   (`clankermatch::balance`), around the party, all of it on the host's
   team (a party keeps together): the host's first
   local listed player's configuration team (player `+0xd4 + 0x7c`,
   `bots::phantom::session::host_team`), read twice a second by the bots'
   frame hook (`clankers::frame`) and followed until the first bot sits,
   then kept so no bot changes team; 0 when unknown or not one of the
   playlist's teams. The frame hook now runs this with no bot in (it used
   to return first, so a Clankermatch with the Clankers page off never
   started its fill).
3. The match starts on the hopper's normal countdown once the phantoms
   bring it to the required player count (below, "Tracing the
   matchmaking lobby"). A lobby that has not started 20 s after its last
   bot was due says why (`Driver::stalled`, reading
   `bots::lobby::match_wait`: the host update's checks in its order),
   once per reason and again every 30 s while it holds: `clankers:
   clankermatch: clanker playlist P's match has not started N s after its
   last bot was due: <reason> [players 3/4, roster 3/4 present, peers 3
   (bots 2), maps [..], hopper statuses [..], arbitration [..], round
   none, matchmaking result 0x3, arbitration state 0x0]`. The reasons:
   the match session short of players, the matchmaking roster's players
   not all in the session, a peer's hopper status not 1, a peer that has
   not loaded the map, no arbitration round, a failed or pending
   arbitration registration, this console not hosting the match lobby,
   or Live not having answered the roster; "every traced check reads
   ready" when none holds. A handoff (the lobby's session deleted and the game's
   advertised) keeps the bots and carries the roster (§9.5); with no
   hosted session for 30 s, or another kind of session hosted, the fill
   ends and the phantoms leave. The game deletes the lobby's
   advertisement once its match session takes over, before the
   countdown ends, so the 30 s count only while this console does not
   serve a multiplayer game (its simulation world the distributed
   server, world mode 4: `bots::phantom::session::serves_game`,
   `clankers::Driver::set_hosts_game`; the game options' own mode is 2
   for every multiplayer game): the bots stay for the whole
   match and go 30 s after it.
4. The game record says `kind: "clankermatch"` with the playlist and
   session (`telemetry::Playlist::clankers`), and the server keeps it as
   its own kind (live-matchmaking.md §6.8).

### 8.4 Search and lobby: initial implementation

The initial implementation joined phantoms only during gameplay.
That left the lobby below the playlist minimum and kept the countdown
from starting. The current lobby join path adds the phantoms to the
target session before the countdown. It also runs with the Clankers
page disabled.

### 8.4 Tracing the matchmaking lobby

**Tracing the matchmaking lobby (2026-11-19, from the disassembly; no
run yet).**

- **The lobby is a pregame lobby.** A playlist press (`0x002b2149` →
  `0x002b21b9`) creates the squad session as the Live custom game row
  does (`0x59890(2, 0, 0)`), then sets lobby mode 3 (`0x199a03(3)`) and
  `0x63fb0`. The start state (`0x19a2ce`) tests "a pregame session"
  (`0x19a279` gives 3 for life-cycle types 1 and 2) **before** its
  matchmaking branch (`0x199971`: lobby kind 6, session `+0x49a4` 2), so
  the party's lobby passes it before the search. Once the search hosts,
  the life cycle is 6 and the lobby is the target session (cm-run2,
  below).
- **What the start counts.** The matchmaking branch's state comes from
  `0x66050(session, hopper)` (through `0x63e90`, hopper `+0x49c8` via
  `0x641f0`), mapped to the status texts by `0x19a5fd`. It reads, per
  session player (mask `S+0x111c`, records `S+0x1120`, `0x13c` each): an
  offline id (`0xBAD00000` at record `+8`) gives state 13; in ranked types
  (2, 4, 5) a guest flag (`+0x10dc & 3` from the record base) gives 12; a
  level byte (`+0x129`) under the hopper's `+0x5e4` gives 15. Per peer it
  wants the peer's hopper status `[peer + 0xac + hopper*4]` (peers at
  `S+0x58`, `0x10c` each) to be 1: 0 gives 14, 3 gives 2, anything else
  11. Then the party: the session's player count `S+0x1118` against the
  hopper's party bounds (types 1 to 3: at least 1; 4 and up `+0x5fc` or
  `+0x5f0`; at most `0x193250`), 6 when short. The match check
  (`0x7e2df`, in the lobby summary `0x7e210`) compares the lobby's player
  count with the hopper's fewest (`0x1932c0`) and most (`0x193300`), as
  `0x7067a` and `0x724a6` do for the matchmaking states. Phantoms
  count once they have joined the target session.
- **`S+0x4980` is the squad's matchmaking state**, set only through
  `0x5a220(state)`: 1, 2, 4, 5 around the lobby and the game (`0x6e596`
  and `0x727e1` set 4 as a game starts), 6 to `0x12` in matchmaking
  (`0x705f0` reads 11 to 13 as the searching states). So the phantom
  check's "4" holds in any running game, a matchmade one too: a
  Clankermatch's phantoms join its game like a custom game's.
- **`0x5f970` is no ready check**: it works on one peer's channel entry
  (`S+0x72dc + peer*0x14`: `+1` valid, `+2` connected, `+4` the
  channel) in states 5 to 8 and walks the other peers' entries; a
  phantom's (valid, not connected, channel -1) is what the host's join
  writes. `[S+0x78a8]` is set to the global `0x527fe8` at the session's
  setup (`0x58f07`); what it refuses is not traced.
- **Phantoms in the lobby: built** (§5.1, "Phantoms in the pregame
  lobby"). A phantom's peer carries this console's own properties, so its
  hopper status is the host's (1) and `0x66050` passes; its map status is
  the host's, so the countdown's end (`0x6e410`) does too. `S+0x4980` is
  not read for the lobby join (it matters only to the in-game player
  diff, and is 4 once the game runs). The fill's first line now says `N
  phantoms join the lobby as peers`.
- **To see in a run:** whether the game changes team assignments when
  a matchmade team game starts.

### 8.4 The matchmaking host's lobby (cm-run2)

**The matchmaking host's lobby (traced 2026-11-19 from the user's
Clankermatch, cm-run2; built, no run yet).** The search hosted (`session
893954eb98f2e2fe advertised`) and the log said `bots: lobby facts: lobby
true, networking true, session type 6, session state 5: no pregame
lobby`; every local bot's gate said `screen 0xd2 (top 0xd2), dialog
none, signed_in false (slot 0x1): waits`, no phantom joined, and the
lobby never started.

- **Two sessions.** Beside the squad session `[0x527364]` (the party's,
  whose Live session `84ad6bf112ae91df` was advertised when the playlist
  was pressed) the game keeps a target session `[0x52736c]`: the
  session a matchmaking search joins or hosts.
  `0x65340` picks the session the game plays in by the life-cycle type
  `[0x527334]` (table `0x65520`): types 1 to 4 the squad session, 6 to 9
  the target session, 5 none. `0x59780` prefers the target session
  whenever it has a state; `0x593e0` tears both down.
- **The life cycle.** `[0x527334]` indexes ten state objects
  (`[0x527338 + 4·type]`, set up by `0x58f22`): type 6 is the object at
  `0x527500` (`0x70190`, the matchmaking one: its timers, `0x705f0`'s
  searching states). `0x19a279` maps the types to lobby kinds: 1 and 2
  give 3 (pregame), 6 and 7 give 2, 3 and 8 give 4 (a game: the in-game
  player diff `0x83220` runs only for types 3 and 8), 4 and 9 give 5, 5
  gives 7. So 6 is the hosted matchmaking lobby, 8 its game; the custom
  game's are 1 or 2 and 3.
- **The screen.** `0xd2` (constructor `0x2519f7`, vtable `0x45a398`)
  is the matchmaking lobby's. Its input `0x251afa` takes B and Back
  (buttons 1 and 13, `0x25217b`) and X (3), nothing else: a second
  controller's A or Start does nothing there. The custom lobby's join
  press (`0x250c80`) needs `0x19a279` 3 and refuses in a countdown or in
  types 2 and 3 (`0x19a902`), so on retail a guest joins a matchmaking
  party in its lobby before the search, never after it.
- **But the session adds any local user.** The per-frame pass `0x63d80`
  runs `0x65770` on every session with a state (the three at
  `[0x4ce03c]`), the target session too, and `0x646b0` and `0x65a60` read
  the session's state (3 to 8) and room (`+0x54` against `+0x4990`,
  `+0x1118`+1 against `+0x4994`), never the type. A bot signed in while
  the target session is up is added to it like any local user, as in the
  custom lobby. The party's squad session is in state 5 too, so the pass
  adds it there as well, as it would a guest signed in before the search.
- **Built.** `bots::lobby::game_session` mirrors `0x65340` (types 6 to 9
  the target session, else the squad's). `Facts` reads the target
  session's state and host in types 6 and 7, and
  `lobby::matchmaking_host` is a matchmaking lobby this console hosts
  (type 6 or 7, the target session in a lobby state, its host peer this
  console's). (The join gate opened there for local bots for a time;
  "Phantoms only" below closed it.) The
  phantoms' `host_lobby`, `live_session`, `owned_session` and the in-game
  `host_session` read the game session too, so a Clankermatch's phantoms
  join the target session (its hopper statuses are the host's, so
  `0x66050` passes them) and stay in it for the matchmade game (type 8).
  A matchmaking lobby another console hosts, or a search with no target
  session yet, takes none. The facts line says it: `bots: lobby facts:
  lobby true, networking true, session type 6, session state 5, match
  session state 5 (this console hosts it): a matchmaking lobby this
  console hosts`. `bots: lobby players: N in the game's session` says
  the target session's player count (`+0x1118`) on each change: what the
  matchmaking start counts.
- **The re-search every ~4 minutes** is the game's: the hopper's search
  timeout (`+0x59c`, read at `0x71c7a` against the time the lobby was
  hosted; 150 000 ms in the small playlists) runs out on a lobby that
  never reached its start, the game deletes its target session, searches
  again and hosts a new one (`80f478...`, then `86334c...`). The fill
  already follows it (`the game moved from session ... asking Live to
  carry its bots there`). With the bots in the target session the lobby
  reaches its fewest players within the fill's 15 s or so, far inside the
  timeout. At the hopper's maximum, `0x1931d0` gives a zero start wait.
  The host update (`0x71730`) compares the target membership count and IDs
  to its expected matchmaking roster (`+0x4a00`), then checks each peer's
  map status (`0x6e0f0`) and arbitration status (`0x72140`). For a full
  lobby that returns to the party, `live.matchmaking.trace` retains these
  inputs, the guest matchmaking result and the target key release.
  [Agent API](../../runtime/agent-api.md#matchmaking-transitions)
- **Arbitration starts when the lobby fills.** Only a full, map-ready
  lobby reaches `0x71ac2`: it makes the round (`0x73ae0`,
  `XOnlineArbitrationCreateRoundID`) and sets it on the target
  (`0x5c1c0`, `+0x49e8`). The update's every-frame `0x72170` then
  registers it (`0x73bc0` → `0x924e0` → `XOnlineArbitrationRegister`), and
  `0x755d0` polls the task: success sets the arbitration state
  (`0x510518`) to 2, a failure calls `0x73dc0(1)` (state 0, result 1).
  This console's peer status (`0x73b10`, peer `+0xec`) reads 1 while
  registering, 2 registered, 5 after that failure. The phantoms copy
  it with the rest of the host's properties, so `0x72140` passes when
  the host's own registration does. `0x72260` checks the target peers
  that are also in the party (this console's own): a status above 2
  sets the matchmaking result to `0x11`, and `0x706f0` (`0x70932`)
  sends any result but 2 and 3 back to the party lobby (life cycle 1)
  with MATCHMAKING FAILED. The target key release and the session
  delete follow. Nothing before the fill registers, so a full
  Clankermatch lobby that drops to the party within a second of filling
  most likely took this branch; the logged result says which.
  The Live HLE logs each refused arbitration call (`live: arbitration
  <session>/<round>/register for <gamertag> failed: ...`) and the game
  logs the result (`live: matchmaking ended with result 0x11: this
  console's arbitration registration failed ...`). Registration and
  report retry through an outage, because the server writes a second
  one as nothing (live-matchmaking.md, "Arbitration (built)").
- **Live.** The match's bots went to the party's Live session too, and
  each was refused there (§9.5): now only the roster's session is told.

The matchmaking progress screen's countdown (`0x0f00188e`, token U+E45B)
reads the target session through `0x59590`. It shows **Setting up bots…**
at zero under the same host setup conditions as the custom lobby.
`lobby_status::MatchCountdown` replaces the temporary text used to
measure and draw the widget at `0x22d2ee`.

### 8.4 Phantoms only: the 2026-11-19 run

**Phantoms only.** In the 2026-11-19 run on `f5d3010c`, the retired
controller-backed bots appeared in the player's party. The local-player
pass (`0x63d80` → `0x65770`) put those signed-in users into both sessions.
The matchmade start then refused to proceed because their controller
ports had no device.

### 8.4 Grep the run's log

**Grep the run's log** (`h2 play ... 2>&1 | tee run.log`), in order:

1. `live: clankermatch: clanker hoppers [8, 9, 10, 11, 12, 13, 14, 15]`
   at logon (or `the server sent no clanker hoppers` / `no clanker hoppers
   from the server`): the marks.
2. `clankermatch: the playlists are the clanker hoppers`: the row's (or
   the switch's) press.
3. `live: clankermatch: searching clanker playlist 8: no one to find; the
   console hosts in N s`: the held search. Missing: the search was not
   the clanker hopper's (check `live: optimatch` lines).
4. `live: session <id> advertised for ...` after it: the console hosted
   a new session. (The game may update a session it already advertises
   instead, which says nothing; step 5 decides.) Neither: the game never
   hosted after the empty search (live-matchmaking.md §7.3 says
   "likely").
5. `clankers: clankermatch: hosting clanker playlist 8 (session <id>): 7
   bots join over N s`: the fill started. Missing with 4 present: the
   advertisement carries no hopper attribute (`Hle::clanker_hosted`).
6. `clankers: asking Live for 7 bots for session <id>`, then `clankers:
   Live rostered bots for session <id>` (or `no roster from Live (...):
   host-made bots`).
7. `clankers: <gamertag> (phantom, rostered) on (...)` for each rostered
   phantom. The lobby facts should identify the matchmaking target
   session this console hosts. `bots: lobby players: N in the game's
   session` reports its growing player count.
8. `bots: phantom <gamertag> joins (peer N, in the pregame lobby)` in
   the lobby, before the countdown. `bots: phantom ... waits: not a lobby
   it can join (...)` names why a lobby refused one, `... waits: this
   console is not the game's host yet (...)` why a running game did.
9. `clankers: clankermatch: clanker playlist P's match has not started
   N s after its last bot was due: ...` (item 3 above): the lobby filled
   as far as it could and the game did not start; the reason names the
   check that holds it. `live: matchmaking ended with result 0x..: ...`
   says a match that failed back to the party, and `live: arbitration
   <session>/<round>/register for <gamertag> failed: ...` a refused
   registration.

### 8.4 Tests (no game)

**Tests (no game):** `cargo test -p boot --lib custom_games` (the rows,
panes, texts, cursor rows), `-p boot --lib coop_live` (the seven-row
menu), `-p boot --lib clankermatch` (the filter, the schedule, the
balance, Quickmatch's hopper, the Switch To: Clankermatch row's place,
press and plan order), `-p game --lib clankers::tests::a_clankermatch`
(the fill: one roster, phantoms only, one bot at a time, no controller
port, balanced teams around the host, the handoff, the end, a refused
roster, the match-only flag, Team Skirmish for a party of two, the
party followed until the first bot sits, the stall line;
`a_full_party_plays_a_clankermatch_without_bots`), `-p boot --lib
bot_mark`, `-p font gear` (the glyph on a synthetic font),
`-p bots phantom::session` (the marked name within 15 units, the host's
team; `phantom::session::tests::lobby`: the lobby this console hosts, the
reservations, the peer properties, the configuration, the teams, the
switch, and a phantom joining the lobby, staying for the game, leaving,
rejoining and waiting; `phantom::session::tests::in_game`: the start's
wait without phantom machines and a phantom standing in the game;
`lobby::a_clankermatch_...`: the target session),
`-p bots --lib lobby` (the game session by life cycle, the matchmaking
host's lobby, its player count, the party's count; `lobby::match_wait`:
each stall reason in the host update's order), `-p live-hle --lib players_tests` (the
bots told to their roster's session only), `-p live-hle --lib clankermatch` (the delay and the run),
`-p live-hle --test server a_clanker_doubles_lobby` (a Doubles match
session read as 2 v 2 and its round registered for both consoles of a
party),
`-p live-proto bots hopper::clankers`, `-p live-telemetry
games_say_their_kind a_bots_name_reads_without_its_mark`, `-p cli
bots_drive coop_drive`, and the server's: `-p live-server --test
clankermatch` (Team Skirmish from roster to ranked game for a party
of one and of two), `-p live-worker a_clankermatch_game`.

### 8.5 Game run checklist

1. The Xbox Live menu has nine rows (eight without co-op): Quickmatch,
   OptiMatch, Clankermatch, Co-op Campaign, Custom Games, Forge,
   Firefight, Custom Game Browser, Mods. Seven show above the hints and
   the list scrolls to the rest; each shows its own help pane
   (Clankermatch: "Matchmaking against bots...").
2. Custom Games opens the custom game lobby; back lands on Custom
   Games. Forge and Firefight open it in their mode. Custom Game Browser
   opens as before, and back from it lands on Custom Game Browser
   (`custom games: opening the browser`); the same for Co-op Campaign.
   Mods opens as Content Download did.
3. OptiMatch lists human and Clanker playlists in descriptor order.
4. Clankermatch lists Clanker Rumble, Clanker Slayer, Clanker Doubles, Clanker Snipers,
   Clanker BTB, BTB Heavies, Solo Training and Team Training (the
   log: `clankermatch: the playlists are the clanker hoppers`; at logon
   `live: clankermatch: clanker hoppers [8, 9, 10, 11, 12, 13, 14, 15]`).
5. Pick Clanker Rumble: the retail search screen runs for 4 to 20 s
   (`live: clankermatch: searching clanker playlist 8 ...`), then the
   game hosts its own lobby (does it? in every type: FFA and teams).
6. The match lobby fills with rostered phantoms (seven for Slayer), the
   countdown starts, and port 0 remains the only local user. Check each
   gear and gamertag in the lobby, scoreboard and carnage report.
7. In Team Slayer check 4 v 4, with phantoms on both teams. In Team
   Skirmish check 2 v 2 with three phantoms. Returning to the party should
   leave only the human there.
8. Let the match start on its own. Check the carnage report lists the
   bots and the site shows the Clankermatch playlist and BOT marks.
   Follow one phantom by gamertag for the gameplay checks below. For
   tick-level movement use the existing agent API's player physics and
   trace views (§3.8).
   - At a jump or obstruction, watch the phantom leave the ground. On a
     crouch approach or jump, watch it lower and stand again. With Taunts
     enabled and a qualifying death nearby, look for distinct crouches
     and releases over the body (§3.5).
   - At close range watch a melee connect. At grenade range look for an
     actual throw and a decrease in the selected grenade count. With both
     types stocked, check a swap selects the other type once.
   - With a partly empty magazine and reserve ammo, watch X reload. Give
     it two weapons and watch a Y tap complete one switch, including the
     plasma-pistol release and rifle follow-up of the noob combo. To check
     zoom and flashlight directly, add a scripted phantom in a hosted
     custom game with `sim.player.add`. Send right-stick clicks on a scoped
     weapon and White presses where a flashlight is available with
     `sim.player.pad`. See the [agent API](../../runtime/agent-api.md).
   - Put a pickup, an empty vehicle and a slowly moving enemy vehicle in
     reach in separate attempts. Watch held X take the pickup, enter the
     free seat and board the occupied vehicle. Check the bot's unit parent
     and seat after entry (§11.4).
9. Leave after the game: the bots go within 30 s (`clankermatch: the game
   is over: the bots go`). Back on the Live menu, Quickmatch starts the
   human playlist picked last in OptiMatch, or opens its playlist picker if none
   (`clankermatch: hopper 8 not saved for Quickmatch` at the pick).
10. With a server that has no clanker marks: OptiMatch as retail,
    Clankermatch's list empty (say how the empty list looks).
11. In a Live lobby on MULTIPLAYER, open Game Setup: Change Playlist...
    is its one row, with no Switch To: Clankermatch (the log:
    `clankermatch: Game Setup list: Switch To: Clankermatch row added`,
    then `lobby mode: Game Setup list in Multiplayer (1 rows)`). Change
    Playlist... lists the human and the clanker playlists on one page.
    On CUSTOM GAMES, Game Setup has no Switch To row either.

### 9.1 The roster: JSON example

```json
{
  "match_id": "…",
  "bots": [
    {
      "xuid": "000b…",            // issued by the server; bots have a range of their own
      "gamertag": "Rusty Lugnut", // passes live_proto::Gamertag
      "persistent": true,         // a persistent bot (ranked) or made up for this match
      "emblem": { "foreground": 12, "background": 3, "colors": [1, 7, 0] },
      "colors": { "primary": 4, "secondary": 9 },
      "rank": { "level": 17, "playlist": "…" }, // what the lobby shows
      "team": 1,                  // team index, or null for the host to auto-balance
      "skill": 0.62,              // 0..1, continuous; maps onto §3.4 (0 Easy … 1 Legendary)
      "personality": {
        "seed": 1234567,          // match RNG for this account; the gamertag supplies its combat profile
        "aggression": 0.5,        // 0..1
        "caution": 0.5,
        "objective": 0.7,
        "teamwork": 0.5,
        "taunt": 0.3              // 0..1; scaled by severity; 0 = never (§3.5)
      }
    }
  ]
}
```

### 9.4 Disagreement, small

   **Disagreement, small:** §8.4 has the clanker hopper carry the *same*
   descriptor playlist id (`+0x08`) as the human hopper. Two descriptors
   with one playlist id in one file is untested against the validator and
   the playlist UI (`0x65530`). With `ranks_as` the clanker hopper keeps an
   id of its own, and the server answers its stat read (`GET
   /v1/rank-stats` for that playlist id) with the `ranks_as` standing, so
   the lobby shows the same rank either way. If a test of the validator
   shows duplicate ids are fine, `ranks_as` can default to the
   descriptor's own id and nothing else changes.

### 9.4 The read-model lookup

2. **The read-model lookup.** A persistent bot is in many matches, so the
   level a roster showed belongs to the rostering, not the bot. Two
   lookups:
   - `bot(xuid) -> Option<&Issued>`: the server issued this xuid as a bot
     (`BotIssued` on `bot-<xuid>`, written by the server; built in 6c as
     `live_server::bots::Bots::bot`).
   - `rostered(session_id, xuid, before) -> Option<(&Rostered,
     &RosteredBot { persistent, level, team, skill })>`: from
     `BotsRostered { session_id, host, playlist?, bots }` on
     `bots-<session_id>`, written by the server when it answers `POST
     /v1/games/bots` or proposes a Clankermatch. A session can roster more
     than once (a later roster replaces the earlier), so `before` is a log
     position and the answer is the latest roster before it: `rank_game`
     passes the game's `GameStarted` position (`Game::position`), the
     roster in force when the game started. Also `roster_at(session_id,
     before)` for the whole roster. **The roster is keyed by the
     session id** (the roster's `match_id` is the session id), so
     `rank_game` finds it from `NewGame.session_id`, the same key
     arbitration uses. `POST /games/bots` already requires the host's
     session.

### 9.5 Read as an account, never a person

- **Read as an account**: `GET /v1/accounts/{gamertag}` answers for a bot
  with `bot: true` (`AccountSummary::bot`), `online: false`, its
  `created_at` (the `BotIssued` time) and its recent games. A human
  account of the same name comes first (a made-up bot does not hold its
  name). The accounts list (`GET /v1/accounts`) never has bots, and the
  web keeps them out of the Players list and the account count.
- **Never a person**: it has no keys (`/keys` 404s), so it cannot sign in
  or post anything; recovery is by an account's gamertag and a bot is not
  one; nobody else may write `bot-*` or `bots-*`. The HLE sends no

### 9.5 The session's players

**The session's players.** Retail Halo 2 fills no slot counts in its
advertisement (`XOnlineMatchSessionCreate`/`Update`, `0x8fcc0`/`0x8fe20`,
slots at ad `+0x3c..+0x48`, filled by a virtual call): run 20261119-005001
showed 0 filled with six players in the game. So the host says who plays.
Every second (`clankers::frame`) while a roster is in force, the driver
hands the HLE the rostered phantoms that have joined the game
(`Driver::session_bots`). `Hle::set_session_players` tells the session the roster is for
(`Driver::roster_in_force`) of them, and each session the console hosts of
its signed-in users, as `PlayerJoined` and `PlayerLeft` on `session-<id>`
for only what changed (`sessions::session_players`, `players_diff`; zero
and host-made xuids never). Only that session: a Clankermatch host
advertises its party's lobby session (made before the search) beside the
match the search hosted, and the user's run (cm-run2) told both, so every
`PlayerJoined` and `PlayerLeft` on the party's (`84ad...`) came back 400
(`unknown player`: the bots are leased to the match alone) while the
match's were taken. One more call after the roster ends sends the
bots' leaves. The server takes a `PlayerJoined` for a human account, or
for a bot of the session's latest roster when its host posts it
(`bots::leased_to`), and a `PlayerLeft` for anyone in the session, so a
replaced roster's bots can leave but not join again. The worker shares
the code (`live-worker` `rostered_bots_read_as_bot_accounts`).

### 9.5 One identity across a session handoff

- **Server** (`live_server::bots::may_carry`, both homes: the worker runs
  the same code). A roster request may name `carry_from`, the host's
  previous session. The server rosters that session's latest bots again,
  in order and with their levels and skills, for the new session (a
  `BotsRostered` on `bots-<new>`), so they are leased to it: the host may
  report them as the new session's players and its game records may name
  them. "The same host's next session" means: `carry_from` was this
  author's session, it is no longer open, it ended (its last activity, the
  delete) no more than `CARRY_WINDOW_S` (120 s) before the request, its
  latest roster is this author's, and it is not the new session itself.
  A count above the old roster's is topped up with new bots after the
  carried ones; a smaller one takes the first. Anything else is a 400.
- **Console** (`clankers::ask_for`). Between the delete and the next
  advertisement (no hosted session), the roster in force stays and the
  bots do not change. When the hosted session changes and the same bots
  are wanted (count, difficulty and playlist unchanged), the driver asks
  with `carry_from` and keeps the old roster meanwhile: nothing is taken
  off, no bot falls back to host-made, the phantoms stay joined. The log
  says `clankers: the game moved from session A to B: asking Live to
  carry its bots there`. Until the server answers, the roster is not the
  new session's: no game record names it and the session is told of no
  bots (`roster_in_force` is `None`). If the server refuses the carry,
  the driver asks for a new roster, as it did before. A changed count,
  difficulty or playlist asks for a new roster, not a carry.
- **The record.** Telemetry asks for the bots once, at the game's start.
  A phantom made later (by a roster change mid-game) was then a machine
  the start's list did not name, so it went out as a human: run -020015's
  "6 players, 3 bots". A phantom's machine id now marks it as a bot
  whether listed or not (`live_proto::bots::is_phantom_machine`, the
  `C1 A4` prefix); only a listed one, with its roster in force, carries
  its xuid.

### 9.5 Staged (not built)

**Staged (not built):**

1. Clankermatch's `MatchProposed` carrying the roster (stage 8, with
   sbmm-ranked): the matchmaking path itself. The playlist roster above
   is what it will send.
2. Bot presence ("Playing Slayer on Lockout" while its host's game runs):
   would need the server to derive it from the host's presence and the
   roster, not a bot's own signed event.
3. Made-up bot accounts pile up: one per bot per roster. A sweep (or
   reusing a session's made-up bots) when that matters; their pages stay.
4. Bots in the web's search lists and finder: kept for now (bots have
   pages), filter them there if the user prefers.

### 9.5 Game run

**Game run.** Sign into Live and host a custom game with Clankers On
and Count 3. The lobby should show the three rostered gamertags.
Sign in a second human on a free controller, start Slayer and check
the game page for three BOT marks linking to the bot accounts. A
System Link game uses host-made Clanker names. The Live custom game
can also be checked with `h2 bots test` (§13).

### 10. Stages, 2 Navigation: mesh rules, file, autonav, tests

   - **The mesh** (`nav::NavMesh`) comes from `crates/formats/pathfind`'s generator
     over each structure BSP's collision: its sectors are the walkable
     polygons, its links the edges walked across. Then:
     - polygon edges that do not meet another polygon look for a floor
       beyond them: a floor up to 0.12 higher is walked onto (a seam or a
       step), up to 1.6 lower is a **drop**, up to 0.6 higher is a
       **jump** (up to 0.825 a **crouch jump**), and a floor up to 0.8
       across a gap is a **jump**;
     - a drop goes one way only; a jump links both ways only if each way
       qualifies.

     The builder adds scenario teleporter and lift links, sweeps
     standing and crouched jump arcs, and checks standing headroom
     (§3.2).
   - **Paths**: A\* over the polygons, then the funnel (simple stupid
     funnel) over the edges' portals, with a drop or jump as a forced
     corner. `nav::Follower` steers along the waypoints: it moves on 0.3
     from each, jumps at a jump, and when no progress is made for 30 frames
     (1 s), jumps once, then asks for a new path.
   - **Walls** (`los::Walls`): every collision surface as triangles on a
     2 m grid, for line of sight (stage 3). A live collision commit on the
     active structure (the editor's `edit.collision.commit`) rebuilds them
     from the committed BSP for the running map
     (`autonav::collision_edited`, `cache::walls_of`), with no visibility
     grid, and leaves the nav mesh as it was
     ([realtime-edits.md](../../editor/realtime-edits.md), "Collision").
   - **Visibility grid** (`bots::vis`, `<map>.<bsp>.vis`): which cells of
     walkable space never see each other, so `Walls::clear` skips the ray
     (§3.1, "Visibility grid"). Built and written by the same worker,
     versioned on its own (`vis::VERSION`).
   - **The file.** `<data>/bots/<map>.<bsp>.bots`: magic `H2BT`, format
     version 4, the **stamp** (the generator version
     `cache::GENERATOR` and the source map's fingerprint), then the mesh
     (`nav::NavMesh::to_bytes`, magic `H2NV`, version 2: each polygon's
     vertices and a flag byte, bit 0 out of the map; then the links),
     the walls, the breakable glass (`glass::Pane`: each breakable
     surface's index and outline) and the tactical places. The data comes from the disc, so it stays on this
     machine and out of git. The frame hook tracks the loaded short name
     and source checksum. For automatic multiplayer builds it queues that
     load on `bots-nav`. The worker checks the mounted source header before
     publishing a cached mesh or building a replacement. For sim, campaign
     or manual meshes, `bots-map-data` reads the local file. This worker
     sleeps between loads and coalesces pending reads. Until a mesh is
     ready the bots can walk toward known goals. Host visual contacts
     require a loaded BSP. On an editor-bound multiplayer map with
     automatic navigation enabled, `bots-map-data` builds in memory from
     the activated play copy and leaves the on-disk meshes untouched.
     A bound campaign map or a play copy with automatic navigation off
     has no mesh.
   - **Built for you** (`bots::autonav`, `game::bots_nav_auto`). Each
     boot owns its `bots-nav` worker and its mounted map sources. The
     worker sleeps until a map selection with
     Clankers on, or a map load requests a map. It looks up that map as
     the VFS serves it, using the active cache slot's recorded path on
     load. This includes disc overrides, installed Map Pack
     files and generated content packages. It rebuilds stale multiplayer
     meshes and grids and leaves other maps for their own requests.
     - **Background work.** The worker runs on `background::spawn` and
       splits collision probes, tactical place scoring and grid rows
       across `background::workers` lowered threads
       (`vis::Threads::background`). The budget defaults to a quarter
       of the cores ([background-work.md](../../runtime/background-work.md)).
       `h2 bots nav` uses half the cores, up to eight, at the default
       priority (`vis::Threads::foreground`).
     - **Cold builds.** `cache::build_with` reads each collision surface
       ring once for both pathfinding and walls. Jump probes reuse arc
       samples and the feet and chest sweeps shared by both postures.
       `nav::ordered_build_chunks` gathers completed probes in input
       order, and the polygon spatial lookup is built once.
     - **Maps no bot plays.** `autonav::map_use` gives `Never` for
       `mainmenu`, `shared` and `single_player_shared` by name, and for a
       header of those kinds (`+0x140` 2, 3, 4); `LoadOnly` for campaign;
       `Build` for multiplayer. `Handle::want` skips a `Never` map.
       The frame hook checks the loaded cache type (`0x547840`) and
       clears the previous map. Its worker releases the retired mesh,
       including when returning to the menu. A campaign map loads a
       hand-built file if there is one.
     - **Packaged maps.** A map a package serves resolves on its first
       look, reading its whole base inside the VFS lock. The lookup checks
       `Vfs::resolves_on_use` and defers a cold package until the game
       opens it. On Unix the worker pins a detached reader for the mounted
       disc, file, store or overlay and does its IO outside the VFS lock
       ([maps.md](../maps.md) section 2).
     - **The fingerprint** is FNV-1a (64-bit) over the map's 2 KiB cache
       header, which holds its size, the tag index's offsets, its name and
       the XOR checksum of the rest (every map writer here recomputes it).
       Checking a requested mesh costs 2 KiB of source reads. A converted
       Halo 1 map's two sandbox variants have separate cache identities.
     - **Shared cache.** The game and `h2 bots nav all` can restore
       completed files to a fresh data directory. `autonav::shared_cache`
       keys a nav bundle by SHA-256 of the whole source map and the nav
       format and generator versions. A visibility entry is per BSP,
       keyed by SHA-256 of its exact `.bots` bytes and the grid version.
       Entries are checksummed and disposable. They live under `bots`
       in the cache directory
       ([storage.md](../../runtime/storage.md#host-directories)).
       A cache miss or inaccessible cache uses the local build path.
       Every file the game loads is still written under `<data>/bots`.
     - **The generator version** (`cache::GENERATOR`) is bumped whenever
       `bots::nav`, `bots::los`, `cache::build` or `crates/formats/pathfind`'s
       generator change what a map builds. The test
       `generator_output_is_pinned` records its output on a small scene
       with the version, so changing the output without the bump fails.
     - **Order.** A request (`Handle::want`) moves its map to the front.
       Another menu pick for the same grid lets its current pass finish.
       An active slot request can interrupt a speculative grid to publish
       the mesh. Boot and menu requests populate the cache. Only a result
       carrying the active load's generation can publish
       (`autonav::offer`), including after returning to the same named
       map. The visibility worker verifies the wall fingerprint
       outside the lock used by bot ticks and attaches the grid to the
       matching mesh.
     - **Picked with Clankers on.** `bots::autonav::build_soon(map)` asks
       for a map the same way, from the Clankers page and the map pick
       (Clankers turned on, or a map picked with them on), so a stale
       mesh is usually built by the time the game starts.
     - **Writes** are atomic: each file is written to
       `.<file>.<pid>.<id>.tmp` beside it, synced and renamed over it, the
       other BSPs first and `<map>.0.bots` (the one checked) last. A crash
       leaves the old file or the new one, never half of one.
     - **Log**: one line a map built (`bots: built the nav mesh for
       lockout (none yet): 590 polygons (0 out of the map), ... in 0.0 s`), one when its
       grid is (`bots: built the visibility grid for lockout ...`), and one summary
       after the pass with the count and elapsed time. Shared cache
       restores are identified in the per-map log.
     - **Off**: `H2_BOTS_NAV_AUTO=0` (or `off`). The worker also stays
       off when the bots feature is (`H2_FEATURES=-bots`).
   - **By hand**: `h2 bots nav <map|all> [--force] [--disc ISO] [--data DIR]`
     runs the same worker in the foreground over the disc. `all` builds
     the disc's stale multiplayer maps and uses the shared cache.
     `--force` and a named map or map file bypass the shared cache and
     rebuild the mesh. A current local grid can still be reused. Use an
     empty data directory to time both build phases.
   - Tests (`cargo test -p bots`): synthetic collision through the real
     generator (two floors sharing an edge, an L-shaped corridor, a ledge
     dropped off but not climbed, a low ledge jumped onto, a 0.5 sill
     jumped onto standing, a floor running in under a lip linked and a
     recess under it not, a narrow gap jumped and a wide one not, a wall
     on a seam or a ledge not walked or dropped through); the in-map
     flags on an arena with a plateau only a bounce reaches (the plateau
     out, no path from the arena ends or passes there, a bot on it plans
     back in by the drop, roam picks and places stay in the arena, no seed
     marks nothing, the flags round trip in the file); locating the floor under a biped; the
     funnel turning at the inner corner; the follower walking the
     waypoints, jumping at a jump, and jumping once then re-planning when
     stuck; the file round trip, refusing damaged files and version 1;
     the generator's output pinned to `GENERATOR`. The automatic build
     (`bots::autonav` tests): the off switch; the header's name, kind and
     fingerprint; staleness (missing, fresh, other map, old generator,
     old format, damaged); the atomic write leaving no temporary file and
     the old file when the rename fails; BSP 0 written last; the queue
     (the booted map first, one source per map); a pass building the
     stale multiplayer maps only, then nothing, then only an edited one;
     a map installed after boot built when wanted; delayed builds and
     cached files checked against the active load. `cargo test -p game --lib
     bots_nav_auto`: finding a requested content map through the VFS, a
     map read through the VFS in chunks, nothing started when off, the
     lookup skipping the menu
     (`only_the_requested_content_map_is_opened`). `map_use` and the
     worker leaving those maps alone:
     `the_menu_and_shared_maps_are_never_used_and_multiplayer_is_built`,
     `the_worker_never_wants_scans_or_builds_the_menu_and_shared_maps`.
     Against the disc (ignored, `--ignored`): `cargo test -p cli
     bots_nav -- --ignored` builds every multiplayer map into a scratch
     directory, then finds them all up to date.
   - **Game run (the user):** see stage 3, which uses the same run.

### 10. Stages, Built: movement boost details, tests, game run

   - **Corners and doorways** (`bots::nav`): paths keep 0.3 off wall
     corners, a seam or step link's portal is cut to the polygon beyond
     (no path through the wall beside a door), and the follower curves
     into a turn. No mesh file changes: `cache::GENERATOR` stays 1.
   - **Off the mesh**: a start or goal off the mesh snaps to the nearest
     floor (3 across, 0.6 up, 3 down); a bot on a crate walks off it, a
     chase to someone seen mid-jump goes on. A bot that falls below its
     path plans again at once, not after the 2 s stall.
   - **Fight footwork** (`bots::moves`): strafe rhythm by skill, jukes,
     jumps and crouches when hit, range keeping, backing to cover with the
     shields down, moving (not standing) while reloading, no strafing off
     ledges or into walls, a jump and the other way when pinned.
   - **Roaming** (`bots::roam`): to recent sightings and weapons, away
     from where it has just been, not to random polygons.
   - `mind.rs` holds two hooks only: the fight branch's movement calls
     `Moves::fight`, and `roam_goal` asks `Roam::pick`.
   - Tests (`cargo test -p bots --lib nav`, `moves`, `roam`, `mind`): the
     L corridor turning off its corner; corners found (not inside a ring);
     a doorway walked through, not around; a start and a goal off the mesh;
     a start below a floor's edge jumping up; the follower's bend and not
     into a jump; a fall below the path; the skills' styles in order;
     Legendary changing direction more than Easy, Easy standing; range
     kept, and the range given honoured; jukes and jumps when hit
     (none on Easy); a crouch at range; never off a bridge; a wall turning
     the strafe; cover behind a wall (Easy straight back); moving while
     reloading; pinned, it flips and jumps; roaming to a fresh sighting,
     forgetting an old one, Easy wandering; not back where it has been;
     the memory bounded; and in the mind, roaming to where it last saw
     someone, chasing someone seen in the air, and a fight on a bridge.
   - **Game run (the user):** a hosted game with three Clankers,
     Slayer on Lockout, then Ivory Tower and Midship.
     1. Doorways and corners: bots walk through doors and round corners
        without scraping or stopping on the frame; none stands stuck on a
        corner for more than a moment.
     2. Fight a bot at Legendary difficulty: it strafes in
        short, uneven bursts, jumps or reverses when you hit it, and with
        its shields popped backs round a wall rather than straight back
        in the open. An Easy bot strafes in long, even sweeps and sometimes
        just stands.
     3. On a bridge or ledge (Ivory Tower's, Midship's walkways) a bot in
        a fight does not strafe off the edge.
     4. Empty its magazine (or watch one reload): it keeps moving, backing
        off, while it reloads.
     5. Hide after a fight: the bots come back to where they saw you, and
        between fights they visit weapon spawns and do not pace one room.
     6. Knock a bot off a ledge or let one stand on a crate: it finds the
        floor and goes on.
     7. Note any spot where a bot still gets stuck for good (map, roughly
        where) and send the `bots:` lines.

### 10. Stages, 3 Combat: seeing, deciding, aim table, tests, game run

   - **Seeing.** An enemy is seen when it is inside 40 degrees of the
     aim, or within 3 units anywhere, within the skill's visual range
     (§3.1, "Visual range"), and the walls leave a clear line from the
     bot's eye to its chest or head. Teammates in a team game are never
     targets.
   - **Deciding**, every frame: fight the target it has while it is still
     seen, else the nearest seen; with no one seen, chase the last place it
     saw someone (forgotten after 10 s); else roam to a random polygon it
     can reach. The full utility layer (§3.3: weapons, health, objectives,
     flee) waits for stages 4 and 5.
   - **Aim** (`mind::Skill`: Easy, Normal, Heroic and Legendary,
     selected in Clanker Options). The right stick turns by the aim error
     times a gain, plus a slowly wandering error; the bot leads a moving
     target by part of the lead a 90 unit/s shot needs.

     | | Easy | Normal | Heroic | Legendary |
     |---|---|---|---|---|
     | reaction (ticks; 30 a second) | 14 | 10 | 7 | 5 |
     | error spread (radians, at 5 units) | 0.10 | 0.06 | 0.035 | 0.02 |
     | lead | 0.3 | 0.6 | 0.85 | 1.0 |
     | stick gain | 1.6 | 2.5 | 3.5 | 4.5 |
     | fire cone cap at point blank | 10° + 0.3 rad | 7° + 0.3 rad | 5° + 0.3 rad | 4° + 0.3 rad |
     | automatic past its band | held | 24 on, 8 off | 16 on, 8 off | 12 on, 8 off |
     | grenades a minute | 0.5 | 1 | 2 | 3 |

     Until its reaction time has passed the bot walks on and turns at
     half gain, but does not fire (§3.4). It fires within its gun's range (§3.6), melees (B) within 0.9, and throws a
     grenade (left trigger) at a target 3 to 12 units away, at most once
     every 8 s. While fighting it strafes, kept on the nav mesh.
   - Tests (`cargo test -p bots`): the skills get better in order; a wall
     hides an enemy, and so does the bot's back; it turns and walks on
     during its reaction and fires after it; firing on target and not off
     it; the trigger held inside the band and burst past it;
     more lead at a higher skill, with a fixed seed; chasing the last
     known position, then forgetting it; roaming to a reachable polygon; a
     teammate never a target; a dead bot lets go of every input; grenades
     rare and spaced.
   - **Game run (the user), stages 2 and 3:**
     1. Select a multiplayer map with Clankers on. A stale mesh produces
        `bots: built the nav mesh for <map> (...)` or a shared-cache
        restore. Selecting a map with current local files skips the build.
     2. Host Slayer on Lockout with one Clanker. The log says
        `bots: lockout: N polygons, ...` when the map loads (not "no nav
        mesh"). If the map loads before its mesh is built, the log says
        `building it`, then `bots: lockout: the new nav mesh is in use`.
     3. Hide behind a wall: the bot does not turn to you or shoot through
        the wall. It walks around the wall to where it last saw you.
     4. Stand still out of its sight: within a few seconds it roams the
        map along floors and ramps, drops off ledges, and does not stay
        stuck on a wall for more than a second or two (it jumps, then picks
        a new path).
     5. Step into its view: it starts turning at once and fires after its
        reaction, then strafes. Get close: it melees. At mid range it sometimes
        throws a grenade.
     6. Repeat with Difficulty set to Easy and Legendary:
        Easy is slow to react, sprays and misses a lot; Legendary
        turns fast and hits most shots.
     7. Run Ivory Tower and Midship too (jumps and drops). Note any spot a
        bot gets stuck in for good, with the map and roughly where.
     8. Send the `bots:` log lines, and anything that looks wrong: the eye
        and chest heights (0.6 and 0.4 over the unit's origin) and the
        stick's direction are guesses a run settles.

### 10. Stages, 4 Emotes: deaths, dumb, burst, tests, game run

   - **Deaths** come from the game's kill function (`PLAYER_KILLED`,
     `0x15ce70`): `boot::bot_glue` observes it on the game thread and logs
     the attacker and victim players, whether it was a betrayal and where
     the victim's unit lies (`emote::kills()`); each brain reads the log
     from its own cursor. A betrayal is a teammate killed in a team game,
     told from the players' teams and the variant's team flag: the kill
     function's third argument is not a betrayal flag (it is set in
     free-for-all too).
   - **Dumb** (`emote::classify`): killed by themselves or by the world
     (own grenade, fall, suicide: the kill function does not tell them
     apart; severity 0.8) and a betrayal (1.0). Not yet: stuck on
     geometry, point-blank misses, out of ammo and the lost 1v1 with a big
     advantage, which need the shots hook, ammo and shields.
   - Each dumb death with a body rolls `taunt × severity` once. The bot
     emotes over it only when the body is within 8 m (2.67 units), no enemy
     has been in its view for 1.5 s, no objective is asked of it
     (stage 5: in Slayer, never), and it has not emoted for 30 s; a death
     waits at most 5 s for that.
   - **The burst**: the bot stands still and clicks the left stick
     (crouch, Halo 2's default layout) 3 to 6 times, a press every 4 to 6
     ticks (130 to 200 ms) held for half of it, 2 s at most, and stops at
     once when an enemy comes into view.
   - **Taunt**: 0.3 (§9.1's default personality) for every brain; the
     Clankers page's Taunts off makes it 0 for its bots at their next
     frame. A phantom's burst reaches the host as alternating crouch and
     standing ticks (§5.1).
   - Tests (`cargo test -p bots emote`): the classification; the log's
     cursors and size; taunts off never, the chance following taunt and
     severity; the burst's presses, rhythm and length; each gate (near,
     clear for 1.5 s, objective, 30 s, the 5 s window); an enemy stopping
     the burst; a kill read from the kill function's arguments over
     synthetic players and objects; the mind crouching and standing still.
     In boot: Taunts reaching the bots already there; the kill hook on the
     render-thread plan as a game-thread point.
   - **Game run (the user)**: host Slayer with one Clanker. Throw
     a grenade at your own feet or jump off the map near the bot, out of
     its view's fight: within a few seconds it may walk on and crouch 3 to
     6 times over the spot (about one time in four: 0.3 × 0.8). It never
     does it while it sees you shooting at it, and not twice in 30 s. With
     Taunts off on the Clankers page it never does.

### 10. Stages, 5 Objectives: trace, tests

   - **Traced** (§3.7): the engine index `E+0xc14`; the objective records
     at `E+0xc18`/`E+0xc1c` (flag, ball, bomb; team; carrier unit and
     player); the netgame flags and their types (0/1 CTF, 2/3 bomb and
     plate, 4 ball, 11 to 18 hill corners) and the designator-to-team map
     `E+8`/`E+0x10`; King of the Hill's hill in play (`[0x51ecc8]+0x1a0`).
   - `bots::objective`: `read` (perception: `Perception::objective`, a
     fault gives `None`), `role_of` (roles across a team by player index)
     and `plan` (the goal, pure).
   - `Mind::step` (§3.3): a carrier carries before everything; a ball
     carrier keeps it; then rearm, fight, recover as before; then the
     objective in place of chasing and roaming. No reload or switch
     while carrying (a switch drops it). No emote while an objective
     is asked of the bot.
   - **Tests** (no game): `objective::tests` (roles; CTF goals with our
     flag home, taken and dropped and theirs home, dropped and carried;
     Oddball with the ball free and held by me, an enemy, a teammate and
     in free-for-all; the hill, its inside and it moving; the engine
     indices); `mind::tests` (the carrier walks home along the nav mesh;
     it neither backs off, rearms, reloads nor switches and melees who
     blocks it; everyone else still rearms first; the objective comes
     before chasing and roaming; the ball carrier runs and melees; the
     hill held and followed); `guest::tests::objective_reads` (CTF
     records, carriers and spawns against a FakeGuest, the designator
     map, Slayer and faults, the hill in play and its moving).
   - **Game run (the user)**: §11.2.

### 10. Stages, 6 Phantom peers: 6a, 6b, 6c, runs, tests

   - **Built (6a):**
     - The membership path is traced (§5.1: the session layout, the
       recipe, and what is open).
     - `bots::phantom::Phantoms`: the books. `add` checks the identity (a
       gamertag and a non-zero xuid), refuses a second phantom with the same
       xuid and more than 15, and puts the phantom on `Pending`. Each
       phantom's machine id is `C1 A4` and its handle, big-endian; `C1` has
       the multicast bit set, so no adapter's address is ever one.
       Handles are never reused. `refresh` finds each phantom's player by
       machine id in the player records (`Joined`, with the index), and
       sends it back to `Pending` when the player goes; `Failed` keeps its
       reason.
     - `Frame::machine` identifies the phantom's player in the brain's perception.
     - `sim`'s `phantom_player` reads `bots::phantom::player`.
     - `live_proto::bots`: the roster (§9.1) and its request, checked by
       §9.1's rules, and `local_xuid` for host-made identities (high bits
       `0x000E`; server accounts carry `0x0009`).
   - **Built (6b):**
     - `bots::phantom::session`: the session recipe of §5.1 as memory
       writes (peer, player, removal), the configuration, the simulation
       slot, the action record and the action from a pad.
     - In the original game-only configuration, `Phantoms::tick` maintains
       membership before each `RENDER_FRAME`. In play the server's action
       gather samples each joined source with its machine ID. A removed phantom's peer comes out. With the pregame
       join disabled, **phantoms join only once the game runs**. When
       it ends they leave the session and wait for the next game, so the lobby never holds one. On a client, or a
       local game, or before this console is the game's host (the first
       frames of a game), they wait and try again each frame, logged once
       per phantom and reason (`bots: phantom Clanker 4 waits: this
       console is not the game's host yet (the simulation is not the
       distributed server (world mode 5, …))`; `NotNow::NotHost` says which
       check: no network session, the simulation mode, the session state
       or its class). They used to fail for good (the user's run:
       `phantom Clanker 4 failed: not the host`, logged in the menus
       before the game). With no room, the one that does not fit fails.
     - `bots::phantom::add` works (the `NotBuilt` error is gone).
     - The Clankers page's count past the free ports adds phantoms:
       "Clanker 4" and up, host-made xuids (`live_proto::bots::local_xuid`),
       the page's difficulty, the host's team. A lower count takes them
       off first.
   - **Game run (the user), 6b**: needs two consoles, a host and a real
     System Link client.
     1. On the host, `h2 play`, System Link, host a custom game; on the
        Clankers page set On and Count 5 (3 local Clankers and 2 phantoms;
        the log says `clankers: Clanker 4 (phantom) on`).
     2. The client joins; start Slayer. The host's log says `bots: phantom
        Clanker 4 joins (peer N)` once the game runs.
     3. On the host's scoreboard: Clanker 4 and 5, wearing the host's
        colours. Do they spawn, turn, walk and shoot? Watch the same
        phantoms for the button actions in §8.5 step 8.
     4. On the client: are they on the scoreboard, do they move, and do
        they stay for the whole game (§5.1, "Clients and the phantom's
        address")?
     5. Back in the lobby they are gone; the next game they join again.
     6. Send the `bots:` and `clankers:` lines, and anything odd in the
        turning or lateral movement. Compare the initial full-stick turn with
        the sensitivity-3 rates in §5.1, then check the sustained turn
        against a local Clanker on the same map.
     7. Also useful: on the host, the session state and the network life
        cycle (`0x527334`) during the game.
   - **Built (6c), the server side, both homes** (`live_server::bots`,
     on the shared router and read model):
     - Bot xuids have a range of their own, `0x000B`
       (`live_proto::bots::SERVER_XUID_MARK`): apart from humans' `0x0009`
       and host-made `0x000E`. Each bot is a server-signed `BotIssued` on
       `bot-<xuid>` (version 0, so an xuid is issued once) with its
       gamertag, emblem, colours and personality (a seeded splitmix, so a
       persistent bot's quirks are fixed).
     - `POST /v1/games/bots` (`RosterRequest`, a `games.write` token):
       only the host of the request's Live session; `count` 1 to 15,
       cut to the session's room. With a `playlist` (the playlist id): the
       persistent bots `Ranks::nearest` the humans' mean level there, each
       at its own level and `bot_skill`; the gap made-up bots at the mean
       level. Without one: made-up bots at the request's `skill` (Normal
       when absent). `team` stays `null`; the host balances. The answer is
       the §9.1 roster with `match_id` = the session id, and the server
       writes `BotsRostered` on `bots-<session_id>`. With `carry_from`
       (the host's previous session, ended within 120 s): that session's
       bots first, as they were (§9.5, "One identity across a session
       handoff"). The session's players that are bots do not count as
       humans for the room or the mean level.
     - `POST /v1/admin/bots {count}` (the operator, up to 64): persistent
       bots, the pool rosters draw from. A persistent bot reserves its
       gamertag as an account would (no account can take it). A made-up
       bot does not: it only avoids names taken at the time, so made-up
       bots never use up the namespace.
     - Acceptance (§9.3), in `POST /v1/games` and in `PlayerJoined`: a
       host-made xuid is refused; a `bot: true` entry needs the game's
       `session_id` and an xuid in that session's roster (in force now at
       the start; in force at the start for a join), a roster the game's
       host made itself (`Rostered.host`; another account naming the
       session cannot use it); an issued bot as a human is refused. `session_id` is allowed on custom games now (a
       game with bots names the session its roster came from); still not
       on campaign. `game::Player` has `bot: Option<bool>`.
     - Nobody else writes `bot-*`/`bots-*` or the two kinds.
     - Tests: `cargo test -p live-server --test bots`, `-p live-proto
       bots`/`game` (issued xuids and looks, host only, bounds, the
       roster in force at a game's start, refusals at start and on a join,
       persistent bots from the operator first in playlist rosters, their
       gamertags held).
   - **Built (6c), the console side:**
     - `live_hle::Hle::hosted_session` (the advertisement this console made
       last, while it stands) and `Hle::roster_bots`: `POST
       /v1/games/bots` on the Live worker with a `games.write` token of the
       account that made the session; the answer is
       `Roster::check`ed and handed to a callback on the worker. Never
       waits.
     - `boot::clankers::Driver::with_rosters` (installed with Live): when
       the page wants more bots than free ports and this console hosts a
       Live session, the driver asks for a roster of the phantoms' count,
       at the page's difficulty (`request_skill`: Easy 0, Normal 0.33,
       Heroic 0.67, Legendary 1; Mixed asks for none, so Normal). It asks
       again when the count, the difficulty or the session changes (the
       bots' frame hook checks the session about once a second, so a
       session made after the page was set gets one); an answer to an
       older ask is dropped.
     - Until a roster comes, or when none does (refused, Live off, System
       Link, offline), phantoms are host-made, as in 6b. Once it comes,
       they all go and the roster's bots replace them
       (`clankers::roster_seats`): xuid and gamertag as the `Identity`; the roster's team when not null; a brain at the
       nearest named skill (`skill_of`), with the gamertag's profile and the
       roster seed for individual choices. Taunts off suppresses taunting. The
       page's difficulty never changes a roster bot's skill.
     - Telemetry (`live_telemetry::Bots`, from the driver's
       `bots_in_force` through `Telemetry::set_bots_locator`, asked at
       each multiplayer start): a custom game names the roster's session
       as `NewGame.session_id`, and each player whose machine id
       (`PlayerState::machine`, player `+0x14`) is a rostered phantom's
       goes out with its xuid and `bot: true`, on `GameStarted` and on a
       later `PlayerJoined`. Local-port Clankers, and phantoms with a
       host-made xuid, go out with `bot: true` and no xuid (§6): a
       `0x000E` xuid is never sent.
     - Not yet: `teams` is always false in the request (the page does not
       know the gametype). Local-port Clankers are rostered now, as bot
       accounts (§9.5).
       Clankermatch's `MatchProposed` carries the same roster (stage 8,
       with sbmm-ranked).
     - Tests: `cargo test -p boot --tests clankers` (roster to identities
       and brains; the skill mapping; the driver asking, running the
       roster, asking again on a count or difficulty change, dropping an
       old answer, keeping host-made bots on a refusal or another
       session's roster, asking once a session appears);
       `-p live-telemetry --lib session` (rostered phantoms as bots with
       their xuids at the start and on a join, no local xuids, the
       session named; none without a roster); `-p live-hle --test server
       roster` (a real roster from the embedded server for the advertised
       session, refused for another).
   - **Game run (the user), 6c**: as 6b, with Live (`h2 play
     --live-local`, or a Live server). Host a custom game over Live; on
     the Clankers page set On and Count 5.
     1. The log says `clankers: asking Live for 2 bots for session …`,
        then `clankers: Live rostered bots for session …` and two
        `… (phantom, rostered) on` lines with made-up gamertags (not
        "Clanker 4").
     2. Start Slayer: the phantoms join under those gamertags.
     3. After the game, the web's game page shows them as bots, with the
        same names; the server log shows no refused game record.
     4. Set Count to 4 in the lobby: a new ask, a new roster of one.
     5. Over System Link the phantoms are still "Clanker 4" and up.
   - In-memory phantom machines, peers and players; action conversion and
     storage; network drop and keep-alive.
   - **`bots::phantom::add` and `remove`** keep the signatures they have
     now (§10.1); `sim.*` already calls them (`boot::sim_bots`, the xuid
     given as hex). `add` returns once the phantom is on the books. The
     phantom's machine, peer and player are made at the next tick on the
     game thread, so `add` never waits on the game. A phantom asked for
     outside a host's session fails then, not at `add`; `list` reports
     that, as below.
   - **From a phantom to its player.** `bots::phantom::list() ->
     Vec<PhantomInfo>` gives each phantom's handle, identity, team, and
     its game player once the membership update has made it (`player:
     Option<u32>`, the player datum's index, as `sim.view`'s `player.<i>`
     uses), or why it has none (`state: Pending | Joined | Failed(reason)`).
     `bots::phantom::player(handle) -> Option<u32>` is the short form, for
     `sim`'s `phantom_player`. The index is found by the phantom's machine
     id in the player records, not guessed from the order of joining.
   - **Identities from the local embedded Live server.** Bots are accounts
     (§9.1), so a phantom's xuid has to be one the server issued:
     - `live_server` gets `POST /games/bots` (§9.1) on the embedded server
       first (`h2 play --live-local`). It makes `count` bot accounts for the
       session (made-up gamertags from a word list, xuids in a range of
       their own, `bot: true` on the account), and answers with the roster.
       It refuses unless the asking host has a session.
     - Bot accounts are flagged on the server, so a game record's `bot:
       true` is accepted only for an xuid the server issued as a bot.
     - The host asks for the roster when the Clankers page wants more bots
       than free ports (or always, with Live on). Each roster entry becomes
       a `bots::phantom::Identity` (xuid and gamertag) and a
       brain at the entry's skill.
     - `sim` can ask the same endpoint for identities and pass them to
       `sim.player.add {kind: phantom, identity}`. The bots crate never
       talks to the server; the host glue does.
     - System Link and offline games make their own identities, with xuids
       in a local-only range that the server never accepts.
   - Tests: the action conversion and store as pure functions over
     synthetic memory, as the co-op work did; the machine-id and peer
     bookkeeping; the phantom-to-player lookup over synthetic player
     records; the server endpoint (issued xuids, refusing without a
     session, `bot: true` only for issued xuids); the roster to identities.
     Built so far (`cargo test -p bots phantom`, `-p live-proto bots`):
     the books (pending on add, refusals, handles not reused), machine ids
     round trip and never a real adapter's, the player lookup by machine
     id and not by order, a leaving player back to pending, failed kept;
     a phantom perceiving itself by machine id; the documented roster
     parsing and §9.1's checks; local xuids never an account's.
   - **Built (6c, after run 20261119-005001):** three of five bots in the
     game, the phantoms waiting on simulation mode 4.
     - Run 20261119-014620 showed why (§5.1, "Hosting the game"): the
       world's mode 4 is the distributed server (the game numbers it 5),
       and the phantom check wanted 5. It takes 4 now (`world_mode`,
       `WORLD_DIST_SERVER`); test `cargo test -p bots --lib
       phantom::session`.
     - `SESSION_GAME_START` (`0x6ec80`): with phantoms planned and no
       other console in the session, the session's host peer is made this
       console's before the game picks its mode, so a lone Live host runs
       the distributed server (§5.1, "Hosting the game"). Each start logs
       `bots: game start: ...` with the fields it read.
     - `LOBBY_ROW_PRESS` (`0x250fb3`): A on the lobby's Start Game row
       logs the start state (§4.2), as a Start press does.
     - The session's players (§9.5): the host reports its bot accounts
       as `PlayerJoined`, which the server takes for its leased bots.
     - Tests: `cargo test -p bots --lib phantom::session` (the start
       facts, the host fix and when it stays away, the hook's lines),
       `-p bots --lib lobby` (the Start Game row press), `-p boot --lib
       clankers` (the session's bot accounts), `-p live-hle --lib
       players_tests`, `-p live-server --test bots
       a_host_reports_the_bots_it_leased`, `-p cli bots_verdict`.
   - **Built (6c, after run 20261119-020015):** 11 of 12, the phantoms
     joined, but the roster churned at the session handoff (new names for
     every bot mid-game, six phantom joins for two) and the record had 3
     of 5 bots.
     - The roster carries across the handoff (§9.5, "One identity across
       a session handoff"): `RosterRequest::carry_from` on the server and
       worker, `clankers::ask_for` on the console.
     - The record marks every phantom-machine player as a bot.
     - Tests: `cargo test -p live-server --test bots
       a_hosts_next_session_carries_its_bots`, `-p live-worker
       rostered_bots_read_as_bot_accounts` (the carry on the worker),
       `-p boot --lib clankers` (`the_bots_keep_who_they_are_...`,
       `the_record_knows_a_phantom_...`), `-p live-telemetry --lib
       a_phantom_the_start_did_not_list_is_still_a_bot`, `-p live-proto
       bots`.

### 10. Stages, 7 Clankers page: settings, page, row, seats, tests

   - **Settings** `clankers.*`, kept in the profile store
     (`setting_defs::ALL`):
     - `enabled`: Off or On; default Off;
     - `count`: 1 to 15;
     - `difficulty`: Easy, Normal, Heroic, Legendary or Mixed. Mixed is
       Normal, Heroic and Easy on ports 1, 2 and 3;
     - `team`: Auto-balance, Red or Blue;
     - `aggression`: Calm, Normal or Wild;
     - `aim_style`: Steady or Twitchy;
     - `taunts`: Off or On; default On;
     - `glitches`: Off, Some or Lots; default Off (§3.8, used).

     The custom-game driver applies Aggression, Aim style and Taunts to
     existing seats as described in §7.
   - **The page** is the shared settings dialog, `clankers`
     (`content/ui/screens/clankers.yaml`; docs/game/ui-builder.md "The
     settings dialog"): eight `setting:` rows over the settings above, on
     the lobby's own settings dialog, Quick Options, inside and out:
     Quick Options' panel, header ("CLANKERS"), list (its `setting_lists`
     skin, the name left and the value alone in the value column), and
     the legend in its footer. Its top is the same at every focus: the
     Spartan icon in Quick Options' game type picture, "Bots in this
     game", and the Game Setup row's help. The focused row's help is
     under the rows. A on a row opens the value picker, which explains
     the option and its choices (the row's `explain`). Its texts are the
     `clankers` id family. In the preview it is `h2 ui preview clankers`.
   - **The row.** A custom game's Game Setup dialog (any list with Change
     Map) gets a "Clanker Options..." row with action 11, under Quick Options and
     the Halo 1 Options row's 10, above the "Switch To:" rows (retail
     actions 6 to 8; `h1_options_row::insert_row` writes the rows'
     actions back in that order). It uses the same three points, and each
     hook is ordered after the Halo 1 row's. A press opens the page on the dialog's own
     stack. The row's text has a slot of its own after the page's footer
     (`KitScreen::row_label`, "Clanker Options..."), so the page's title
     stays "CLANKERS" (in capitals, as Quick Options' header is). The
     Halo 1 row does the same with its id 2 (`h1_options::ROW_LABEL_ID`,
     "Halo 1 Options..."), its screen's title staying "HALO 1 OPTIONS";
     both read like the dialog's other "...Options..." rows.
   - **Seats.** Clanker Options configures the requested phantom roster.
     The match host creates the peers as the hosted game starts
     (`crates/extensions/game/src/clankers.rs`,
     `crates/extensions/game/src/bot_glue.rs`). See §5 for peer admission
     and §9.1 for the Live roster. The page's bots sit only in a custom
     game's lobby (retail mode 2: the party lobby's CUSTOM GAMES, FORGE
     and FIREFIGHT modes, `clankers::admits_page`). In the co-op and the
     matchmaking lobby they stand down and come back in a custom mode
     (`Driver::set_lobby`, [party-lobby.md](../party-lobby.md#clankers-by-tab)).
   - **Nav meshes early.** A map picked in the lobby's map list with
     Clankers on, or Clankers coming on after a map was picked, asks
     `bots::autonav::build_soon` for its mesh (`clankers::map_picked`).
   - Tests:
     - the settings in `setting_defs::ALL`, and off by default;
     - mixed difficulties and stable names in the phantom roster;
     - the page: a row per setting, steps saved in the store, wrapping;
       the row's text in its own slot, not the title's;
     - the row: only a custom game's list, once, beside the Halo 1 row;
       on the plan after the Halo 1 row's hooks;
     - the kit's own checks over the page (`ui_kit::screens`: every
       screen, every input, every state; ids; layout).
   - **Game check.** In a System Link or Live host lobby, open
     **Clanker Options**, enable Clankers and set Count to 2. Start the
     match and confirm two phantom players join the roster. Section 11
     gives the combat checks, and §13 covers the hosted harness.

### 10. Stages, Combat decisions: melee, trigger, weapon choice, tests, game run

   - **The weapon is known** (§3.6, "Handling"): role, band, the range
     it shoots to, its trigger, its rate and its projectile's speed.
   - **Melee.** Within arm's reach all difficulties attempt a beatdown
     when facing the enemy. A visible shield break or a plasma weapon in
     hand favors a strike at the outer edge. A sword uses B for a quick
     slash at arm's length and RT to lunge at a target in its lunge band. The chest direction gates the strike with any gun.
   - **The trigger.** A once-per-pull weapon (magnum, battle rifle,
     carbine, shotgun, sniper, rockets, the plasma pistol's quick
     shots) pulls the trigger for 2 ticks and let go, every `30 / rate` ticks (at least
     4; 8 when the rate is unknown) plus the skill's pace (Easy 12,
     Normal 6, Heroic 3, Legendary 1 tick). Automatics are held inside
     the gun's band and burst past it from Normal up (§3.4). A blade uses
     the sword slash and lunge inputs above.
   - **No dry fire.** The trigger stays up with an empty magazine or a
     dead battery; it reloads or switches instead. It fires only in the
     weapon's range: a shotgun no further than 8, rockets never nearer
     than 2.5.
   - **Weapon choice**: every difficulty switches to the gun that suits
     the distance. Normal and up can draw the other gun instead of reloading;
     puts rockets away close up and the sword away for a far enemy;
     reloads in a lull or when the enemy is out of reach; goes for a
     power weapon (sniper, rockets, shotgun, sword) within 25 when out of
     a fight, dropping the worse gun for it; dry, goes for the best
     nearby gun, not merely the nearest.
   - **Targets** (`tactics::choose`): the nearest and most in front
     (distance plus 4 per radian off the aim), sticking with the one it
     has (its cost times 0.6), so a close threat takes over from a far
     target but two at about the same range do not flip-flop. An enemy
     back in view within 1.5 s of being seen needs no new reaction (it
     no longer froze each time someone strafed past a pillar); a new one
     in the middle of a fight takes half the reaction.
   - **Aim.** Lead uses the held weapon's projectile speed (400 a second
     for a bullet) in place of 90 for every gun, so bullets are no longer
     led four times too far. Heroic and Legendary aim at the head with a
     precise weapon or a sniper rifle, and at the feet with a blast
     weapon; Easy and Normal at the chest.
   - **Grenades** (all difficulties): none at a target with a teammate
     within 3 of it.
   - **Pre-aim** (Normal and up): chasing, it looks where the enemy went
     out of view (within 15, in clear sight, and not more than 45 degrees
     off the way it walks), else into the coming corner.
   - **Signals for the movement** (`Mind::fight`): `fight.band`, the
     range to keep the target at (the held gun's band; arm's length with
     nothing to shoot), also `arms::preferred_range(&arms)`; and
     `fight.wants_cover`: shields down, or reloading or switching with
     the enemy within the band.
   - Tests: `-p bots --lib arms` (`roles_come_from_the_handling`,
     `the_preferred_range_is_the_held_guns_band`,
     `by_range_it_switches_to_the_gun_that_suits_the_distance`,
     `rockets_are_put_away_close_up`,
     `an_empty_magazine_in_a_fight_switches_when_that_is_faster`,
     `a_lull_reloads_a_half_empty_magazine`,
     `calm_with_nobody_in_view_tops_up_a_used_magazine`,
     `an_enemy_out_of_reach_is_time_to_reload`,
     `a_power_weapon_nearby_is_taken_and_the_worse_gun_dropped`,
     `dry_it_goes_for_the_better_gun_a_little_further_off`,
     `a_blade_is_kept_close_up_and_put_away_for_a_far_enemy`,
     `the_handling_reads_from_the_weapon_and_projectile_tags`), `-p bots
     --lib tactics`, `-p bots --lib mind::fight_tests`; the rest of
     `-p bots --lib mind` unchanged.
   - **Not traced, wanted next:** whether a shot hit or was aimed at the
     bot, a projectile's
     gravity for lobbed weapons, the energy sword's lunge range from its
     tag, and the flag, ball and bomb tags' aim assist (assumed none).
   - **Game run (the user):** host Slayer with Difficulty set to Heroic on
     Lockout and on a map with a sniper rifle and rockets.
     1. A bot with a battle rifle or magnum fires steadily (bursts at
        its rate), not one shot and then nothing.
     2. A bot never clicks an empty gun: it reloads behind cover or
        draws its other gun; it does not stand reloading in your face
        when its other gun is loaded.
     3. At range it uses its battle rifle, close up its SMG or shotgun;
        it does not fire rockets at its feet.
     4. Out of a fight, a bot near the sniper rifle or rockets goes and
        takes them.
     5. Strafe in and out of a bot's view behind a pillar: it keeps
        turning to you without a pause each time.
     6. In a team game a bot does not throw a grenade where its
        teammate stands.
     7. Easy still sprays and misses; Legendary is sharp but still has
        its reaction time.

### 10. Stages, Built: weapon skill details, tables, tests, not traced

   know about how to use grenades well, how they arc/bounce, and also
   ranges of weapons, noob combos, using both weapons effectively". This
   slice is `bots::grenades` (new), `bots::weapons` (new), `bots::arms`,
   `bots::tactics`, `Walls::hit` in `bots::los`, and small hooks in
   `bots::mind`. The aim hand, the footwork and the big decisions are
   other lanes'.
   - **Grenades fly** (`grenades::fly`). From the eye along the aim, under
     gravity, in steps of 1/60 s against the map's collision triangles
     (`los::Walls::hit`: where along the step and the surface's normal; a
     flat floor at the target's height when the map has no data). A frag
     bounces (35% of its speed into a surface comes back, 30% of its speed
     along it is lost at each touch; slower than 0.4 on a floor it lies
     still) and goes off 1 s after its first touch; a plasma sticks to
     the first surface or player it touches (a player is an upright line
     0.1 to 0.65 over the feet, 0.25 thick) and goes off 1.2 s later. Halo
     2 has no cooking: nothing the bot does moves the fuse. **None of
     these numbers is read from the game** (below); they are named
     `ASSUMED_GRAVITY` (3.2 world units a second squared), `FRAG` (6 a
     second) and `PLASMA` (5.5 a second) for a run to check.
   - **The aim** (`grenades::solve`). Pitches from -20 to 70 degrees in 5
     degree steps, the best refined by 1 degree; the throw goes off
     nearest where it should, within 1 world unit or not at all. A moving
     target is led (its velocity times the flight time, then again with
     the flight time of that throw). With none landing straight and banks
     allowed (Heroic and up, also Normal at last-seen cover), tries 8 to 60 degrees to either side: off a
     wall, or rolled past a corner to where someone went out of view. A
     plasma that sticks the target counts as a hit. The arc blocked, it
     does not throw (tries again in 2/3 s).
   - **When and what** (`grenades::Thrower`, the mind's `grenade`). The
     ordinary rate increases for a cluster, recent cover or a closing enemy
     (§3.4). It aims between members of a group and favors a frag. Normal
     and up can stick a lone visible opponent within 8. Black changes
     grenade type when necessary. A blast within 3 of the bot or a
     teammate cancels the throw on every difficulty. It turns to the
     throw (the mind looks along it) and lets go once its aim is within
     the skill's tolerance, giving up after 1 s; a human hand: the throw's
     aim is off by the skill's noise. At most one every 8 s.

     | | Easy | Normal | Heroic | Legendary |
     |---|---|---|---|---|
     | banks and rolls | no | at last-seen cover | yes | yes |
     | sticks plasmas | no | yes | yes | yes |
     | lead | 0 | 0.5 | 0.85 | 1 |
     | aim noise (degrees, std. dev.) | 6 | 3 | 1.5 | 0.8 |
     | lets go within (degrees) | 8 | 5 | 4 | 3 |
     | spares teammates | yes | yes | yes | yes |
     | more before pushing in | no | yes | yes | yes |

   - **Weapon ranges** (`weapons::reach`). A gun's band and fire range
     are its role's (§3.6) refined by its tag's aim assist reach
     (`cone_range`, `pull_range`): a magnum, shotgun, plasma or
     rifle or SMG is good to its reach (kept between half its role's
     band end and the role's band end). A sniper or splash weapon starts
     with its role's band. Every gun fires to 1.25 times its band's end
     (`weapons::FIRE_PAST_BAND`), one rule for all classes. The first
     barrel's projectile caps all firearm ranges at the nearer positive
     value of its air damage endpoint and maximum travel. With both
     fields unset, the role and aim assist decide. The movement band
     shortens when the projectile requires a closer shot. The aim point
     needs a clear line from the bot's eye, including for the off hand
     and a passenger using a carried gun. A sword's lunge uses its aim
     assist reach (1 to 3, or 1.5 when unread), and it swings at that.
   - **Using both guns** (`arms::plan`, Normal and up, out of a fight).
     Two short guns or two long ones: a gun of the other kind within 15
     is taken, the worse of the two dropped for it (switching first when
     the better is in hand). A full magazine in hand and the other gun
     wanting a reload: switch to it, so it is reloaded out of the fight.
     In a fight, the switches by range and instead of a reload (§3.6) now
     use each gun's reach.
   - **Combos** (`weapons::Combo`). With a plasma pistol at or above 15%
     battery, a loaded precision weapon and a visible opponent 1.5 to 14
     units away, the Clanker rolls the name's combo probability once for
     the encounter. Briefly losing sight releases the controls and keeps
     the roll. If needed it draws the pistol, charges for 24 ticks, then
     releases on aim or at 36 ticks. It draws the rifle or magnum after
     the skill's switch delay (18, 10, 6 or 3 ticks on Easy through
     Legendary). A visible shield break brings that switch forward, and
     every difficulty aims its loaded precision weapon at the head if
     that point is visible. The follow-up holds the weapon choice for up
     to two seconds. The melee finish also uses recent pulls on target
     (5 on Normal, 4 on Heroic, 3 on Legendary, one per 8 ticks for an
     automatic).
   - **Dual wielding** (unit `+0x213`, the left hand's slot, traced in
     docs/live/live-gamestate.md and `boot::touch_glue`). The left gun
     reads as `Arms::left`; the left trigger fires it with the right's
     aim (`weapons::LeftHand`: a charge gun charges and lets go on target,
     a once-per-pull gun is pulled at its rate). An automatic in the left
     hand goes down and up with the right trigger when the right is held
     on an automatic too, and otherwise on the right's hold pattern and
     hysteresis (§3.4). An empty left magazine with rounds in reserve gets
     one short pull to start its reload, again after 1.5 s if it is still
     empty. No grenades then.
     The pickup policy, hold-Y action and grenade trade are traced in §3.6.
   - Tests: `-p bots --lib grenades` (`a_grenade_falls_as_gravity_says`,
     `a_frag_bounces_on_and_goes_off_its_fuse_after_the_first_touch`,
     `a_wall_throws_a_frag_back`, `a_plasma_sticks_to_a_player_in_its_way`,
     `the_aim_lands_it_at_the_target`, `a_blocked_arc_is_not_thrown`,
     `a_frag_is_rolled_round_a_corner_when_the_straight_throw_is_blocked`,
     `a_plasma_is_stuck_on_a_close_target`, `a_moving_target_is_led`,
     `never_at_its_own_feet_or_a_teammates`,
     `the_skills_throw_better_in_order`,
     `the_thrower_turns_to_its_arc_then_throws_spaced_out`,
     `a_plasma_is_chosen_to_stick_a_close_target`,
     `no_throw_dual_wielding_without_grenades_or_out_of_range`); `-p bots
     --lib weapons` (ranges, classes, the combo's phases, the draw, the
     skills' odds, the melee finish, the left hand); `-p bots --lib arms`
     (`two_short_guns_take_a_long_one_dropping_the_worse`,
     `out_of_a_fight_the_other_gun_is_drawn_to_reload_it`,
     `the_aim_assist_reach_moves_the_switch`,
     `the_unit_reads_its_left_hand_and_grenade_type`); `-p bots --lib
     los` (`a_hit_says_where_and_which_way_the_wall_faces`); `-p bots
     --lib mind::fight_tests` (`no_grenade_into_a_teammate` now turns to
     each throw, `a_noob_combo_holds_the_charge_lets_go_and_switches`,
     `dual_wielding_fires_the_left_gun_and_throws_no_grenades`).
   - **Not traced, wanted** (addresses for a tracing lane):
     - the globals (`matg`) grenades block: its offset in the globals tag,
       stride, and each entry's projectile reference (then the frag's and
       plasma's `proj +0x17c` initial velocity, already confirmed, gives
       the throw speed);
     - the gravity the projectile update takes off a projectile's z
       (`0x000f9726`, docs/live/live-telemetry.md): the global it loads
       and the `proj` field that scales it (air gravity scale);
     - in `proj`: the detonation timer and whether it starts at the first
       touch, the material response block (offset, stride, the
       perpendicular and parallel friction or restitution fields), and the
       sticky flag;
     - how a throw leaves the hand: the projectile's spawn point and
       direction against the unit's aim (`+0x168`) and whether the unit's
       velocity is added;
     - the plasma pistol trigger's charging time (the `weap` trigger
       block, `+0x40` stride; which field);
     - the damage effect radius of the frag and plasma explosions (`jpt!`
       upper radius) for the safety distance (3 assumed).
   - **Game run (the user):** §11.1, "Weapon skill".

### 10. Stages, Built: natural feel details

   the simulation tick with separate seeded random streams. The full
   pad is reused on repeated frames within a tick.

   - Aim ramps into flicks and corrects overshoot. Tracking trails a fast
     crossing more than a slow one, and brief wall occlusions discard
     velocity. A gunshot may draw a delayed, coarse glance. Visible
     enemies alone supply live targets and shot lead.
   - The feet accelerate in world space, vary strafe deflection and
     ease off near a destination. Cameras look into clear turns and
     doorways before the feet finish the turn, and on a ramp pitch toward
     the next waypoint (`look::SLOPE_PITCH`). The navigation follower
     checks the corner cut for footing and keeps gap approaches at pace.
   - Bots crouch-walk on quiet objective approaches. Hits alone can
     change the strafe or start a crouch at range. Jumps are single
     short presses on planned routes, during stuck recovery, at the
     start of a strafe, on an open run as the name likes (§3.2, "Pace and
     hops"), or while escaping a visible nearby grenade.
   - Weapon taps have a short intent delay, a hold and a release gap.
     Melee needs a sustained chance in front of the bot. Semi-auto
     pulls vary within the weapon's cadence and bursts start when the
     bot lines up a shot.
   - Small right-stick fidgets happen when an actor is standing idle
     and on signed-in local pads in a network pregame lobby or while
     awaiting a unit. They send zero left stick, triggers and buttons.
     The retail action builder applies the turn curve and translates the
     hand's button timing as described in §5.1.
   - Seeing a live grenade and getting clear of it is `bots::incoming`
     (named `grenade` on the lane; renamed at the merge so it does not
     read as `bots::grenades`, which owns throwing). The live grenades
     come from the same object scan as the pickups and supplies
     (`arms::ground`, with the bot's feet). In a fight the aim
     hand also carries a throw: while `grenades::Thrower` aims, the hand
     tracks the throw's line instead of the target. Only a throw, not the
     left gun's trigger when dual wielding, holds off the right trigger
     and the weapon taps.
   - Focused unit-test filters are `mind`, `tactics`, `moves`,
     `nav::tests`, `incoming`, `sound` and `phantom::session` with
     `cargo test -p bots --lib`. The game-run checklist is §11.1,
     "Natural feel".

### 10. Stages, Built: decisions boost details

   Local slots and hosted phantoms on the same team exchange target
   calls. Each bot hears completed ticks, so polling order and repeated
   rendered frames at one tick give the same claims. Calls expire after
   2.5 seconds. If two healthy bots have comparable targets, the higher
   player slot yields the contested target. A hurt teammate can call for
   focus fire. A recent friendly death makes its visible killer a trade
   opportunity for three seconds. In free-for-all, a visible pair aiming
   at each other presents an opening to a third player.

   A visible numerical disadvantage prompts withdrawal. Weapon minimum
   ranges still apply. CTF and Assault carriers try routes away from
   recent sightings. The bot's own death and visible teammate deaths can
   mark a dangerous approach for up to 20 seconds across respawns.

   See §3.2 for the host perception boundary and supply beliefs, and §3.6
   for the pickup observers. Map loads and tick rewinds clear the radio,
   supply observations and danger memory. The coordinator's unit-test
   filters are `decisions`, `objective::tests`, `arms::tests::decisions_`,
   `guest::tests::decisions_` and `mind` under `cargo test -p bots --lib`.
   The in-game checks are in §11.3.

### 10. Stages, 8 Clankermatch: built, follow-ups, next

   - Built: the Live menu row (row 2; the rest moved down), the playlist
     filter at `0x00192db0`, the clanker hoppers in the default set and
     `GET /v1/hoppers/clankers` (both homes), the empty search with its
     delay, the lobby fill from the playlist's roster (one at a time,
     balanced teams, skill from the server), `kind: "clankermatch"` in the
     record and in the server's read model and stats (both homes).
   - Built since (follow-ups): the ⚙ glyph in every font and the
     marked bot names (§8.2.2), Quickmatch kept off clanker hoppers
     (§8.3), the cursor back on the browser's and co-op's rows (§8.3),
     the balance around the host's real team (§8.4).
   - Next:
     1. A party of more than one human (the balance takes the host's
        player only).
     2. The search delay from the playlist's population; continuous skill
        in the brain (§8.2.3: the brain still takes the nearest of four).
     3. A clanker game's rank needs an arbitration round as ranked
        matchmaking does: check whether the console registers one in an
        unranked clanker hopper, or have the server rank without (with
        sbmm-ranked).
     4. `MatchProposed` with `host = me` and the bots as members, when
        the matcher exists (sbmm-ranked).
     5. Clanker marks for a published hopper file (an admin mark, as
        pins have): today only the default lineup's clanker playlists,
        kept where a published file has the same playlist id at the same

### 10.1 Library surface: registry, PadSource and source details

  which determines the phantom's appearance.
- `bots::phantom::remove(handle)`, `list()` and `player(handle)` use the
  process-wide registry. `list()` returns each `PhantomInfo` with its
  handle, identity, team, machine id, optional player index and
  pending, joined or failed state.
- `bots::pad_source::PadSource` produces one `HostPadState` from a
  `Frame`. Phantom gather samples a spawned player's source once per
  fresh simulation tick. `Frame.frame` counts samples and `Frame.tick`
  carries the simulation tick. The source can find its player through
  the phantom's machine id with `bots::guest::perceive`. A source must
  keep registry operations out
  of its `pad` callback.
- `bots::pad_source::Scripted` holds pads for timed steps and can loop.
  `bots::pad_source::Recorded` plays each supplied pad for one sample,
  then idles. `bots::Brain` uses the loaded map's nav mesh and collision
  data. A closure taking `&Frame` can also implement `PadSource`.

### 11. What needs a game run: the twelve-item list

Each item is for the user to play, or for a coordinator run.

1. All configured phantoms join the hosted System Link or Xbox Live
   pregame lobby. The controller sign-in screen lists human controllers.
2. A second human signs in on a free port while the phantoms are in
   the lobby. Check the human views, HUD and audio during play.
3. Rostered Live phantoms appear under their own bot accounts.
4. Bot movement on real maps: the nav mesh and stuck recovery on every
   shipped MP map, jumps and lifts included.
5. Aim and combat feel per difficulty against a human; tuning of the §3.4
   table.
6. Emote triggers and gates in real play.
7. CTF, Oddball and King of the Hill with bots (§11.2).
8. A phantom peer seen correctly by a real System Link client: lobby,
   in-game, scoreboard, carnage report. The host's CPU with 15 phantoms.
9. The Clankers page in Game Setup: layout on the real screen, and its
   values persisting.
10. Clankermatch end to end: §8.5's checklist (the glyph on every name
    surface once it is built).
11. Whether an empty search makes the game host in each playlist type, or
    `MatchProposed` with `host = me` is needed (§8.4).
12. Every glitch (§3.8): none has been seen working in a game. Its
    checklist is in §3.8.

### 11.1 The brain in play (checklist)

For the user, or a coordinator run with `h2 bots test` and its log. Two
or three Clankers on Lockout, Midship and one big map (Coagulation or
Waterworks), on Normal and Legendary. Each line is a yes or no.

Stuck (§3.2):

1. Over a few minutes no bot stands pressed against a wall, a railing or
   a corner for more than about 3 s.
2. A bot that walks into a step or low ledge sidesteps, then jumps, then
   crouch-jumps, and gets over it or turns away. It does not jump in open
   floor while walking freely.
3. A bot that cannot get somewhere goes somewhere else within about 3 s
   and does not come back to the same spot for about 10 s.
4. No bot walks back and forth between two points for more than about
   4 s.
5. Two bots meeting in a corridor pass each other; a bot does not push
   into a teammate standing still.

Ammunition (§3.6):

6. A bot that empties a magazine starts its reload after a short input
   delay (the reload animation plays).
7. A bot with a part-empty magazine reloads after a fight, not during it.
8. A bot whose gun runs dry with a loaded second weapon switches to it
   (the switch animation plays).
9. A bot with both guns dry walks to the nearest weapon on the map and
   takes it (another kind: it swaps; the same kind: its ammunition goes
   up). Check with Famine on, or a variant with low starting ammunition.
10. A bot with both guns dry and a human within a couple of metres runs
    in and melees instead.
11. A bot does not stop on the flag, the ball or the bomb trying to pick
    it up (they read as not guns).
12. Plasma weapons: a bot with a dead plasma rifle or pistol switches or
    goes for another weapon; it does not try to reload it.
13. The fields themselves: on the human's own unit, `h2 agent object`
    reads of the held weapon's `+0x22c` and `+0x22a` match the HUD's
    rounds counter, and `1 - (+0x184)` the plasma pistol's battery meter.

Cheap wins (§3.3):

14. A bot with its shields popped backs away from the human while
    shooting, and does not chase out of sight until they recharge.
15. Bots throw a grenade into two humans standing together more often
    than at one; never with no grenades left.
16. A bot that loses sight of someone round a corner sometimes lobs a
    grenade there.

Weapon skill ("Built: weapon skill" in §10), Heroic and Legendary, Slayer
on Lockout and Midship with a human:

17. Before throwing, a bot looks up (or down) along its throw for a
    moment, then throws; the grenade lands at or rolls to where the
    human stands, more often than it falls short or long. Note the map,
    the distance and where it landed against where you stood: the throw
    speed, gravity, bounce and fuse are assumed numbers.
18. Behind a pillar or round a corner, a Heroic or Legendary bot's frag
    sometimes comes off a wall or rolls round the corner to you. It does
    not throw into a wall in front of its face.
19. Within about 8 a bot with plasmas sometimes sticks you (it presses
    Black first when frags are selected). A bot never kills itself or,
    in a team game, its teammate with its own grenade.
20. A bot with a shotgun or SMG throws a grenade before closing in.
21. A bot with a plasma pistol and a battle rifle or magnum charges the
    pistol (its glow), fires the overcharge at you, switches and shoots
    for your head; Legendary does it nearly every fight, Easy almost
    never.
22. A bot that has hit you a few times and is at arm's length melees.
23. A bot with two short guns walks to a battle rifle nearby after a
    fight, and one with two long guns to an SMG or shotgun.
24. After a fight a bot draws its other gun when that one's magazine is
    low, and reloads it.
25. Put a second SMG beside a Normal or higher SMG bot, then try plasma
    rifle + SMG, needler + needler and plasma pistol + SMG in both orders.
    Check the left-slot pickup, both triggers and the charged pistol shot.
    At long range the bot should keep its rifle. Present a group at grenade
    range and check that a short Y drops the left gun before the throw.
    Repeat with a friendly in the blast area and with a wall across the arc.
26. A magnum bot backs off to its rifle at a shorter range than before;
    a bot with an SMG stops spraying at you across the map.

Natural feel ("Built: natural feel (sol)" in §10):

27. Compare Easy and Legendary on the same map. Peek from the edge of a
    doorway, then cut sideways across the bot's view. Watch for a pause
    at first sight, a quick turn with a small correction, and more lag
    on fast crossings. Easy takes longer to settle.
28. Strafe behind a pillar and change direction while hidden. Watch the
    bot hold or scan the opening, then pick you up when you reappear.
    Fire from behind a nearby wall. A short look toward the report can
    happen before the bot has a target.
29. Follow a bot through a doorway and around a corner. Look for small
    changes of pace, a brief look into the opening, and a path that
    leaves the frame and railings clear. Compare two bots strafing.
30. Throw a grenade into plain view near a bot on level ground. Look for
    one quick escape movement and, up close, a single jump when the
    route is clear. In an ordinary gunfight, hits lead mostly to strafes
    or crouches. At a genuine gap the bot takes a run-up.
31. Watch a phantom in a quiet CTF or Hill approach, near an empty
    magazine and during a close melee encounter. Look for a slower
    crouch-walk, a brief delay before X or Y, and spaced melee swings.
    Leave it waiting and check the lobby selection and respawn remain
    under the human's control.

Finishing fights (§3.3, "Finishing and breaking off"), Slayer on Midship
and Lockout:

32. A bot that has your shield down closes in and melees instead of
    strafing at range while your shield comes back.
33. A plasma rifle bot does not fire at you across the map; it closes to a
    few metres first.
34. A bot that loses you round a corner holds the corner or follows you
    before it answers a teammate's call or walks back to a teammate.
35. In Juggernaut, hunters head for the Juggernaut even when it is out of
    sight, and the Juggernaut does not back away from two hunters.
36. No bot stands on a weapon, or walks back and forth to one, for more
    than a couple of seconds.

### 11.2 Objectives in play (checklist)

For the user, or a coordinator run. Two teams of two or three (a human
and Clankers), Normal. Each line is a yes or no.

CTF on Lockout or Beaver Creek:

1. With both flags home and out of view, the attackers leave the spawn
   for the enemy base; the nearest goes for the flag, escorting names
   follow it, and (with three or more on the team) one stays near its
   own flag. They fight whoever they meet on the way.
2. A bot reaching the enemy flag takes it even with a full inventory.
   Check the X hold while the flag is loose and its release on pickup.
3. A bot carrying the flag runs home along the floor, does not shoot,
   does not drop it (no weapon switch) and does not stop to reload or
   for a gun, even with its shields down. It melees a human standing in
   its way.
4. Reaching home with our flag there, it scores (the score goes up).
5. Take a bot team's flag: its defender, and an attacker nearer to you,
   come after you; when you drop it, the nearest bot walks over it and it
   goes home. Break line of sight with it: the defender looks where you
   were, then heads for your base.
6. A bot whose teammate has the enemy flag runs near them.
7. Spawns and returns are right on both maps: the bots' "home" is their
   own base, not the enemy's (the designator map, `E+8`/`E+0x10`).

Oddball (any small map):

8. When the ball spawns, the bots run for it; in teams two of a side run
   and the others stay near it.
9. A bot with the ball takes cover or moves away from an approaching enemy.
   It keeps its trigger up and melees a human who comes close.
10. When a human has the ball, the bots come after them, together rather
    than one at a time, even with the human out of sight; in teams, a
    bot's teammate with it is screened, the escorts standing between it
    and you.

King of the Hill (Midship or Lockout):

11. The bots go to the hill and stay in it, fighting from it and strafing
    inside it, rather than standing still or chasing out of it. With the
    hill held, a cautious bot covers it from its edge.
12. When the hill moves, they go to the new one within a few seconds.
13. A bot in the hill counts as in it (the hill's colour and the score
    say so). Check the score on a different floor over the same XY and after a move.

Assault (if tried):

14. Check which side picks up a team bomb and a neutral bomb, and whether
    the carrier scores at the plate its escorts defend (§3.7). After the
    plant nobody, of either side, stays within about 6 world units of the
    bomb when it goes off.

All:

15. No bot teabags while carrying, guarding or in the hill.
16. Rotate a one-bomb round. Check both teams swap roles, the carrier
    stays until armed and plants on the active plate, and escorts cover
    a planted bomb while recovering a loose one.
17. In team Territories, contest a friendly site while a neutral site
    is closer. Watch two bots relieve it and hold through a capture. A
    team's bots take different sites rather than stacking on one. In
    FFA, check that another player's territory counts as hostile.
18. Kill the Juggernaut in front of two hunters. Check they switch to
    the new Juggernaut and leave one another alone. As Juggernaut the
    bot should close on a visible opponent and melee.
19. In Oddball check the first spawn and a reset against a neutral type-4
    marker on the map.
20. In Oddball, kill the bot carrier: the nearest bots of both teams run
    onto the dropped ball and one picks it up.

### 11.3 Decisions boost in play (checklist)

Use Heroic with at least two Clankers on one team. For the route and
Oddball checks, use a map with a nav mesh and more than one way through.

1. **Team Slayer.** Present two similarly distant targets. Watch the bots
   split fire, then focus fire when one teammate takes damage. Break the
   receiver's line of sight. It may move toward the call. Its trigger
   should stay up until it can see an opponent. Kill the caller in view
   of the other bot and look for an immediate target switch after its
   normal reaction delay.
2. **Odds and openings.** Isolate one bot against two nearby opponents.
   Watch it back off, then regroup when sight breaks. Try a one-on-one
   with a depleted shield or an empty magazine hidden from the bot. Changing
   those values alone should produce the same decision. In free-for-all, fight
   another player in sight of a third bot and watch for it to close.
3. **CTF.** Drop the home flag while a teammate is carrying the enemy
   flag. Check the return, then the escort and home guard assignments.
   Cause a death at one entrance and watch a carrier try the other one.
   Repeat later to see the danger expire.
4. **Oddball and King.** A ball holder should settle behind cover and a
   hunter should favor the visible carrier. Shoot from outside the hill.
   Watch for a bot to enter while returning fire, remain inside while
   scoring and move when the hill changes.
5. **Assault.** Try a team bomb and a neutral bomb. Follow one carrier to
   the opposing plate. Watch teammates cover the plant and defenders
   intercept the carrier. Drop the bomb on the plate and check who goes
   to it.
6. **Supplies.** Let a calm bot see you take a sniper rifle, rockets,
   overshield or camouflage. Watch it return near the next spawn. Repeat
   with the pickup hidden. An unseen disappearance cannot date a return.
   Contest a due pad with a sword or rockets and check the bot can fight
   from pickup range. Try half, double and disabled respawns. Compare its
   arrival with the actual service tick. Hand a sniper to another player
   in view and check the bot recognizes the new wielder. Repeat with a
   hidden handover and check that the old sighting decays.
   Repeat on Easy with the same roster to compare the timing.
7. **Outdoor map knowledge.** On H1 Blood Gulch and Sidewinder, give a
   Clanker a sniper and watch which lane it faces. Repeat from the other
   base, with a lead and with a visible opponent. Verify both flanks are
   reachable, a lead can produce a hold, and combat interrupts it.
   Let it inspect an empty sniper pad and turn away. Swap the hidden
   respawn state while preserving all cues, then expose the pad and
   look for a change only when the bot can inspect it.

### 11.4 Vehicles in play (checklist)

For the user, or a coordinator run. Two or three Clankers on Coagulation
or Headlong (Slayer), Normal, then Legendary. Each line is a yes or no.

1. Soon after spawning, a bot with no enemy in view walks to a nearby
   Ghost or Warthog and gets in from the side.
2. Two bots on one team do not both run to the same vehicle; with a
   teammate driving a Warthog, another bot gets on its gun.
3. A bot driving a Ghost follows the map's roads and paths, turns
   without circling, and does not sit against a wall for more than a few
   seconds (it backs up, then gets out).
4. A driving bot drives at a human on foot in front of it and runs them
   over, firing the Ghost's guns; a Warthog driver does not fire.
5. A bot gunner shoots at the human; when its driver gets out, it gets
   out within about 4 s.
6. A bot in a vehicle about to blow up, or flipped, gets out.
7. A bot holding the rocket launcher, at a human driving a vehicle, keeps
   the reticle on the vehicle for about a second and fires a rocket that
   follows it; at a human on foot it fires at once.
8. The human driving a Ghost slowly past a Normal bot within about 1.5
   (about 3 on Legendary) gets boarded; driving fast past it, not.
9. The fields: on the human's own unit in a Warthog, `h2 agent object`
   reads `+0x1fc` 0 as the driver and the gun's seat on the gun; the
   Warthog's `vehi` seats' `+0` flags have bit 2 on the driver's seat and
   the gun's bit 3; `+0x1f0` reads 1 for the Warthog, 4 for the Ghost, 5
   for the Banshee.

### 11.5 Traversal and sandbox movement in play (checklist)

1. On Coagulation or Beaver Creek, put a goal closer through a
   teleporter and watch a bot enter the sender, arrive and resume walking
   on the correct floor. Repeat with the chosen receiver blocked or on a
   map with two receivers on one channel.
2. On Ascension and Colossus, start below the lift exit. Check that the
   bot enters the launch area, steers after pickup and walks off at the
   upper landing. On Cyclotron, repeat for the moving elevator and check
   the bot stays aboard while it rises.
3. On a low-ledge route, check that A starts the jump, crouch follows
   takeoff and the bot reaches its landing. On a low-roof route, check
   that the bot turns away from the standing-height obstruction.
4. On Ascension, watch a bot enter the Banshee, climb, make a strafing
   run and boost out. Damage it above the ground and check the evasive
   roll. Check secondary fire with a variant that enables the fuel rod
   bomb. Check the bot descends before exiting for an objective.
5. On Zanzibar or Headlong, watch a bot take a standalone gun covering
   the fight, then dismount when approached from the side. Repeat with a
   teammate driving a Warthog using a gauss or chaingun turret.
6. On Coagulation or Headlong, approach a Scorpion or Wraith from either
   side and a Warthog from beside the driver door. Compare the bot's X
   press to the actual seat marker in each case. In a Ghost or Warthog,
   choose a goal with a pedestrian shortcut and a wide detour. Check
   the bot follows the wide route and clears the turns.
7. Repeat traversal jumps, Banshee button actions and vehicle entry and
   exit in a hosted custom game and Clankermatch. Inspect playback through
   `h2 agent` and the existing `sim.*` and world methods (§10.1).
8. On H1 Blood Gulch verify both outbound portals from their bases. On
   Sidewinder exercise both directions between the paired pads and each
   outbound-only pad. Try a crouch-jump route above standing reach on
   both maps. Check whether Sidewinder's blast-door scenery moves.
9. On Containment watch both high receivers through free fall. Check
   the actual player exit on all four Warlock lifts, the Lockout tube and
   Midship's midpillars. Try Gemini's columns with a player. On Relic
   compare traversal before and after activation of the portal switch.

### 12. Next tracing (offline)

Done for stage 1 (§4.1 to §4.3): the pad seam, the slot flags and the
sign-in function, the players screen's join path, and the readers of the
local-user map.

Still open:

- For phantoms (§5.1, "Open"): the lobby's ready check `0x5f970`, the
  join room check `0x62e30` and the veto object, `0x5fe20` on a peer with
  no channel, and `0x14c090`'s body.
- Verify the phantom's per-tick hold transition, zoom release and
  grenade throw after dropping a dual weapon (§5.1 and §8.5). The button
  masks and selection fields have static traces.
- Which font slots draw player names in the lobby, scoreboard and carnage
  report.
- For objectives, the traced fields and their writers are in §3.7. The
  outstanding checks are pickup behavior, rotated one-bomb rounds, hill
  transitions and capture volumes in play (§11.2).
- For vehicles (§3.9): verify the rocket launcher's lock time and zoom
  requirement, the units of `O+0x88` (velocity), the Banshee's secondary
  fire and roll in phantom play, and whether tag dimensions need a
  larger margin on tight turns. For lifts (§3.2), measure the launch
  trajectory and ride time on the retail maps.
- For weapons (§3.6), trace plasma heat and compare the predicted service
  phase with a witnessed pickup and actual return on phantom pads,
  including alternate H1 sandboxes and multiplayer variants.

### 13. Testing: what it does, in full

**What it does.**

1. Starts a Live server in the harness process on `127.0.0.1:8077`
   (`--server-port`), with a throwaway store under
   `$TMPDIR/h2-bots-test/run-<time>/` (`--root`), and makes the account
   **Tester** there.
2. Opens one `h2 play` window signed in as Tester (`--live-server`, its own
   data and Live directory, `--no-mappack --no-halo1`), with
   `--set clankers.enabled=true --set clankers.count=N`. So Clankers is On
   at Count N for this run with no menu work (the page's rows show it
   locked). `--count` defaults to five phantom peers. The window builds
   the game into `--out` first when `--out`
   has none, as `h2 play` does. `--wait-clang` passes `--tier clang`, so
   the run uses clang's full build and never the Cranelift fast tier; it
   first waits while a background clang build of `--out` is still writing
   `clang-build.log`. The run's data starts with the `[graphics]` table of
   `--settings-from`'s `settings.toml` (by default `~/Library/Application Support/h2recomp/data`,
   the data root `h2 play` uses), so the window renders with the user's
   own video settings.
3. Presses through the menus over the game's agent socket
   (`crates/apps/cli/src/bots_drive.rs`). The game runs with `H2_UI_WATCH=1`, and
   every press waits on a `ui: screen` line, a game line or the agent API's
   phase. A fresh data folder stops on the title screen and the new
   profile's dialogs: each gets A once it has held 4 s. The main menu is
   the window 5 screen the game names with `coop-live: lobby mode
   4294967295`; its cursor row is the low 16 bits of the `list [mask/value]`
   value on its `ui: screen none` line. Xbox Live is row 1. If Split Screen
   or System Link opens, the driver uses the players' screen mode to
   return to Xbox Live. If the profile needs to sign
   in, Xbox Live opens the game's account picker (`dialog_live_signin`,
   dialog `0x14`, loader `0x24b4a9`), as a new profile's boot does. Its
   row 0 is the no-account row. The driver selects Tester and opens
   the Xbox Live menu. It moves to Custom Game on row 4 and checks
   that the game advertises a session in lobby mode 2. Once the
   phantoms have joined the lobby, or after 90 seconds, it presses A
   on Start Game. Each press is a `bots-drive:` line.
4. Watches the game's `bots:`, `clankers:` and Live lines, the agent API's
   `session.status` (phase, mode, map), the server's session for Tester
   (`GET /v1/sessions?host=Tester`: filled slots) and, once in gameplay,
   the newest game record (`GET /v1/games`: players with `bot: true`).
   What it reads goes into the merged log as `bots-test:` lines.
5. Stops once every check has passed (3 s more for the last lines), the
   game closes or stops, or after `--timeout`, counted from the game's
   agent socket coming up (a build before that is not counted, up to an
   hour). It prints the verdict,
   closes the game, deletes the run's data and exits non-zero if a check
   failed. With `--keep` the game stays open, and the command with it (it
   is the Live server), until the window closes; the data is kept.

### 13. Testing: --no-drive and progress.json

**`--no-drive` and `progress.json`.** `--no-drive` launches and watches
exactly as above but presses nothing: the scripted driver is off, and the
verdict is the same. At the start it prints the agent socket and a hint
(`h2 agent --socket <sock> ...`). `--timeout` still counts from `agent:
listening`; a drive by hand or by a model needs longer, so it defaults to
900 s with `--no-drive` (360 s otherwise; an explicit `--timeout` wins).
Either way the run's log directory gets `progress.json`, rewritten
whenever a check, the status or the last lines change (written through a
temporary file, so a poll never reads half of it):

```json
{ "finished": false, "passed": false,
  "next": { "step": 3, "name": "on Xbox Live ..." },
  "phase": "menu", "mode": "-", "map": "-",
  "last_ui": "ui: screen none (slot 4: ...)",
  "last_bots": "bots: ...",
  "checks": [ { "step": 1, "name": "...", "state": "pass|pending|fail",
                "detail": "..." } ] }
```

`checks` contains the nine checks below. `state` is `pending` while
the run goes and `fail` for what did not pass once it has ended. `next`
is the first check not passed. An outside driver loops: read
`progress.json` (stop on `finished`), `h2 agent --socket <sock> ui` and
`h2 agent --socket <sock> frame snap 1` to see the screen, then
`h2 agent --socket <sock> press down` (or `a`, `b`, `up`) as the first
pending check and `last_ui` call for, and read `progress.json` again.

### 13. Testing: the checks table, log lines

| # | Check | Passes on |
|---|---|---|
| 1 | Signed in to Xbox Live | `live: playing as Tester` |
| 2 | Clankers on | `clankers: G (phantom...) on` for each bot |
| 3 | On Xbox Live | the Xbox Live players screen or menu |
| 4 | Hosting a Live custom game | the game advertises a session in lobby mode 2 |
| 5 | Live roster of bot accounts | `clankers: asking Live for N bots for session S`, then `clankers: Live rostered bots for session S` |
| 6 | Gameplay on a map | `session.status` reports multiplayer gameplay, on the requested map if given |
| 7 | Phantoms joined the pregame lobby | `bots: phantom G joins (peer K, in the pregame lobby)` for each bot |
| 8 | The Live session holds 1 + N players | the greater of the filled-slot count and the number of players the host reported |
| 9 | The game record has the bots | at least N players with `bot: true` in the server's game record |

A failed check includes the relevant lines, such as the phantoms'
`waits` messages if they never joined. The verdict also lists panics,
guest faults, refusals, stuck drives and timeouts under `trouble:`.

### 13. Testing: notes

**Notes.**

- `--map` only checks the map played. The menus are not driven through
  Game Setup's Change Map, so the game plays the lobby's default map; a
  `--map` other than that fails the gameplay check and names the map played.
- The phantoms join the pregame lobby (§5.1, "Phantoms in the pregame
  lobby"). Step 8 counts the most filled slots and the most joined
  players, the server saw over the whole run. Retail Halo 2 fills no slots (0 in run
  20261119-005001), so on a Live run the joined players are what counts.
- `h2 play` builds the game into `--out` when `--out` has none, or has one
  from another image. Run it with the same `H2_OPT` as that build, or the
  window recompiles first. The timeout starts once the game is up, so a
  build doesn't eat it.
- One game at a time: the server port and Tester's account are the run's
  own, but a second `h2 bots test` needs another `--server-port` and
  `--root`.

### 13.1 Clankermatch: server state

**Server state.** None to set up: the run's fresh store has no published
hopper file, so the server serves the built-in lineup
(`live_server::default_hopper`), whose hoppers 8..15 are the eight clanker
playlists, marked on `GET /v1/hoppers/clankers`. The harness reads the marks
before it starts the game, checks the playlist asked for is marked and on
the row the plan expects (the Clankermatch list shows only the clanker
hoppers, in index order), and stops at once if not. The game runs with
`--set clankers.enabled=false`: the page's Clankers stay off and only the
fill seats bots.

### 13.1 Clankermatch: the drive

**The drive** (`crates/apps/cli/src/bots_drive/clankermatch.rs`): boot, sign-in
and the Xbox Live menu as above, then row 2 (Clankermatch), A. The playlist
list is the first settled screen after that A; its row is read off its
list value (or counted from the DOWNs sent, when the value reads as no
row), and A goes on the playlist's row. Then nothing is pressed: the
search (4 to 20 s), the lobby this console hosts, the fill and the
countdown run by themselves. A human playlist's search, no search 15 s
after A (A again, 4 tries), no hosted lobby 90 s into the search, or a
lobby that has not started in 180 s stops the drive, saying which. Once
the game has played and the phase is back in the menus, it leaves: B
(A on a dialog, and DOWN, A on the next one; A after a B that changed
nothing, as on the carnage report) until this console hosts no session,
then waits for the fill to end (30 s, `FILL_GONE`). A game that starts
again first is waited out. The game runs to its own end: Rumble Slayer's
6 minutes, Team Slayer's 8, or the score, so the timeout is 900 s
(1200 with `--no-drive`).

### 13.1 Clankermatch: the checks table with log lines

**The checks:**

| # | Check | Passes on |
|---|---|---|
| 1 | Signed in to Xbox Live | `live: playing as Tester` |
| 2 | Clanker hoppers listed (server and game) | `bots-test: server clanker hoppers [8, 9, 10, 11, 12, 13, 14, 15]` and `live: clankermatch: clanker hoppers [8, 9, 10, 11, 12, 13, 14, 15]`, both with the plan's hopper |
| 3 | The Clankermatch row opened the clanker playlists | `clankermatch: the playlists are the clanker hoppers`, and no `the server named no clanker hoppers: the list is empty` |
| 4 | The clanker search started | `live: clankermatch: searching clanker playlist H` with the plan's H |
| 5 | The search ended with this console hosting | `live: session S advertised` after the search, and `clankers: clankermatch: hosting clanker playlist H (session S)` |
| 6 | Phantoms planned for the lobby | `clankers: clankermatch: N phantoms join the lobby` with the expected count |
| 7 | No bot in the party | no `bots: signing ...` line for a controller port |
| 8 | The phantoms have rostered accounts | one `clankers: G (phantom, rostered) on (...)` line per planned phantom |
| 9 | The lobby reached the playlist's fewest players | the most players the game's own session had (`bots: lobby players: N in the game's session`, the game session's `+0x1118`) between the hosting line and the start; the Live server's count (`bots-test: session ...`) is what the host told it and no longer counts (cm-run2 listed "4 joined" with no bot in the lobby) |
| 10 | The countdown ran and the game started | `session.status` phase `loading` or `gameplay` on a multiplayer map after the hosting line: not `map mainmenu` or mode `main_menu` (cm-run2's `loading` was the main menu coming back) |
| 11 | Gameplay on a map | phase `gameplay`, not campaign, after the start |
| 12 | Phantoms joined the lobby before the start | `bots: phantom G joins (peer K, in the pregame lobby)` for each phantom before the start, and no phantom failure |
| 13 | Game record: kind clankermatch | a game record from this console has kind `clankermatch` and at least as many `bot: true` entries as planned phantoms |
| 14 | After the game, the phantoms leave | the fill ends and each phantom has a `clankers: G off` line |

A failed check includes the relevant server, game or driver lines. The
join check includes any phantom wait or failure reasons.

### Lines condensed on the page

Single lines whose names or values the page no longer carries.

- (`0x1c83d8`, [ai-control.md](../../halo1/ai-control.md)) and the game state
- (`Observations::moving`, `firing`); perception keeps only the bits of
- the body is within 1.5 world units (about 4.5 m: `emote::NEAR`, the
- (`crates/live/bots/src/arms.rs`), and a pure decision (`arms::plan`) turns
| Role | Band (`arms::preferred_range`) | Shoots from, to | Rating |
- patience and caution from the personality, `plan_with`).
- surfaces with flag bit 3 (`0x08`) and a breakable index (the surface's
- `+5` byte, `0xff` none), each piece of a pane its own surface. The game
- bot plays it. A brain's `Tuning::bounce` (`Some`: a tuning file's
- named site (`plan_site`: the crouch spot `plan_with` would pair with it,
- proven first, no reach limit and no walk to beat; `why` is `asked`).
- `+0xDD0` player-set change) is never sent: its encoder `0x87d00` is only
- every peer's XNADDR (`0x5cb80` → `0x76aa0`, `0x5d237`) and its drop loop
- never go through it ("Phantoms in the pregame lobby"). `0x5f970` is a
- badge with `0x1634d0`. Retail's `0x68a90` takes the lowest score across
- `{session_id, playlist?: playlist_id, count, skill?, teams}` and gets one
- A made-up bot (`persistent: false`) still gets an `xuid` from the server,
- /games/bots`, the roster, `bot: true` on a game's entries, the `0x000E`
- (`0x193560`) reads, and the game never needs to know. The mark is in
- bool, ranks_as: Option<u32> }`. `ranked` says whether finished games move
- whose standing it shares (`None`: its own rank playlist). The server
- ever sees accepted bot entries, and reads `rostered` for `persistent`
- and `level`. Host-made xuids (`live_proto::bots::is_local_xuid`) are
- `Bots::persistent`) for choosing persistent bots, which
- `FIRST` × `SECOND`, a number past a few clashes), emblem, colours and
- `8894d40b...` deleted, `83d9415d...` advertised a few seconds later).
- `<root>/logs/<time>/bots.log` and the game's whole stderr
- `<root>/logs/<time>/Tester.log`; both paths print at the start and the
- end. `h2 bots verdict bots.log` prints a log's verdict again (and exits
- `--playlist` implies `--clankermatch` (slayer by default); `--count` and
- `--map` are refused, since the playlist sets both. Everything else
- `--dry-run`, `--no-drive`, `progress.json`, `h2 bots verdict`) is as
- above; the merged log's plan line is `bots-test: clankermatch plan: ...`,
- which is how `h2 bots verdict` knows which verdict to print.
| `--playlist` | Hopper | Row | Players | Phantoms |
- Tests:** `cargo test -p cli bots_` (the verdict from log lines, the
- driver's presses, the arguments; Clankermatch's under `bots_drive::clankermatch`,
- `bots_verdict::clankermatch` and `bots_test_cmd`).
