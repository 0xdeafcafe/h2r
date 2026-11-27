# Bots: stages

What each build stage covers and the library surface (sections 10 and 10.1). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 10. Stages

The checks for live play are in §11 and §13. Clankers use phantom peers
(§5). Controller ports are for local players.

| Stage | What it is | Code | Status | Sections |
|---|---|---|---|---|
| 1 Player slots | A phantom registers with the session and the host feeds it actions | `crates/extensions/game/src/bot_glue.rs` | Built | §5, §6 |
| 2 Navigation | Nav mesh, paths, walls, visibility grid, and the worker that builds them | `bots::nav`, `bots::los`, `bots::cache`, `bots::autonav` | Built | §3.2 |
| 3 Combat | Seeing, deciding and aiming by skill | `bots::mind` | Built | §3.3, §3.4 |
| 4 Emotes | The taunt burst after a dumb death | `bots::emote` | Built | §3.5 |
| 5 Objectives | CTF, Oddball, King of the Hill, Assault, Territories, Juggernaut | `bots::objective` | Built | §3.7 |
| 6 Phantom peers | Phantom books, the session recipe, the roster (server and console) | `bots::phantom`, `live_server::bots` | Partly built | §5.1, §9 |
| 7 Clankers page | Settings, the Game Setup row, seats | `boot::clankers` | Built | §7 |
| 8 Clankermatch | Menu row, search and lobby fill | `boot::clankers`, `live_server::bots` | Partly built | §8 |
| 9 Vehicles | Seat entry, driving, Banshee flight | `vehicle::`, `mind::ride::` | Built | §3.9 |

Combat, weapon skill, natural feel and team decisions are stage 3
follow-ups, under their own headings below.

### Stage 1: Player slots

The game registers a phantom with the session,
then the host feeds it actions through `crates/extensions/game/src/bot_glue.rs`.
Sections 5 and 6 describe the join and action path.

### Stage 2: Navigation

`bots::nav`, `bots::los`, `bots::cache`,
`h2 bots nav`, and the map loading in `boot::bot_glue`.

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
  ([realtime-edits.md](../editor/realtime-edits.md), "Collision").
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
    of the cores ([background-work.md](../runtime/background-work.md)).
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
    ([maps.md](maps.md) section 2).
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
    ([storage.md](../runtime/storage.md#host-directories)).
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

### Movement (stage 2 and 3)

The feet (§3.2). Weapons, targets and grenades are
below.

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

### Stage 3: Combat

`bots::mind` (`Mind`, the phantom's decision loop).

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

### Stage 4: Emotes

`bots::emote`, in every brain (`Mind::emote`).

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

### Stage 5: Objectives

CTF, Oddball, King of the Hill, Assault, Territories and Juggernaut (§3.7).

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

## Where the sections live

| Page | Covers |
|---|---|
| [bots-stages-late.md](bots-stages-late.md) | stages 6 to 9, combat decisions, weapon skill and the library surface |

Each heading below is cited from code and moved to the page named after it.

### Stage 6: Phantom peers

Moved to [bots-stages-late.md](bots-stages-late.md).

#### Phantom books (6a)

Moved to [bots-stages-late.md](bots-stages-late.md).

#### Session recipe (6b)

Moved to [bots-stages-late.md](bots-stages-late.md).

#### Server side, both homes (6c)

Moved to [bots-stages-late.md](bots-stages-late.md).

#### Console side (6c)

Moved to [bots-stages-late.md](bots-stages-late.md).

#### Hosting the game (6c)

Moved to [bots-stages-late.md](bots-stages-late.md).

#### Session handoff (6c)

Moved to [bots-stages-late.md](bots-stages-late.md).

### Stage 7: Clankers page (Game Setup)

Moved to [bots-stages-late.md](bots-stages-late.md).

### Combat decisions

Moved to [bots-stages-late.md](bots-stages-late.md).

### Weapon skill

Moved to [bots-stages-late.md](bots-stages-late.md).

### Natural feel

Moved to [bots-stages-late.md](bots-stages-late.md).

### Team decisions

Moved to [bots-stages-late.md](bots-stages-late.md).

### Stage 8: Clankermatch

Moved to [bots-stages-late.md](bots-stages-late.md).

### Stage 9: Vehicles

Moved to [bots-stages-late.md](bots-stages-late.md).

## 10.1 Library surface (for the agent API)

Moved to [bots-stages-late.md](bots-stages-late.md).
