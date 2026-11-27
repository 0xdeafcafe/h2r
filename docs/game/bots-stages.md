# Bots: stages

What each build stage covers and the library surface (sections 10 and 10.1). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 10. Stages

The checks for live play are in §11 and §13. Clankers use phantom peers
(§5). Controller ports are for local players.

1. **Player slots.** The game registers a phantom with the session,
   then the host feeds it actions through `crates/extensions/game/src/bot_glue.rs`.
   Sections 5 and 6 describe the join and action path.
2. **Navigation.** Built: `bots::nav`, `bots::los`, `bots::cache`,
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
   **Built: movement boost** (2026-11-19, §3.2). The user's run: "bots
   kinda suck, they get stuck ... give the ai a nice big boost". This is
   the feet; weapons, targets and grenades are the fight lane's.
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
3. **Combat.** Built: `bots::mind` (`Mind`, the phantom's decision loop).
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
4. **Emotes.** Built: `bots::emote`, in every brain (`Mind::emote`).
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
5. **Objectives.** CTF, Oddball, King of the Hill, Assault, Territories and Juggernaut (§3.7).
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
6. **Phantom peers.** In progress.
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
7. **Clankers page (Game Setup).** Built: `boot::clankers`.
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
     (`Driver::set_lobby`, [party-lobby.md](party-lobby.md#clankers-by-tab)).
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

   **Combat decisions.** `bots::mind` uses `bots::arms` and
   `bots::tactics` to pick weapons, targets and fighting positions.
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

   **Built: weapon skill** (2026-11-19). The user: "ai doesn't really
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

   **Built: natural feel (sol).** Aim, walking and buttons are driven by
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

   **Built: decisions boost (astra)** (2026-11-19, §§3.3 and 3.7).
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

8. **Clankermatch.** First slice built (§8.3, §8.4; run checklist §8.5).
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
        index.
9. **Vehicles.** Built (§3.9): seat entry and boarding from live tag
   markers, rocket lock-on, chassis clearance for ground routes, Banshee
   flight and turret take/leave decisions. Fixtures live under
   `vehicle::` and `mind::ride::`. The game-run checklist is in §11.4.

## 10.1 Library surface (for the agent API)

- `bots::phantom::add(Identity, team: Option<u8>, Box<dyn PadSource>)`
  registers a phantom. It joins a System Link or Xbox Live session this
  console hosts (§5.1). `Identity` holds the XUID and full gamertag,
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

See [the agent API](../runtime/agent-api.md#sim-simulated-players-and-direct-state)
for script, recording and brain sources.
