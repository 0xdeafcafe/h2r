# Bots: Clankers and Clankermatch

**Status:** built for System Link and Xbox Live hosted matches; a few menu
surfaces still need an input-driven game run (see the note).

Clankers are multiplayer bots that play Halo 2 multiplayer by the game's own
rules. Each one joins the session as a phantom peer with one player, so
controller ports stay reserved for people. The brain emits a pad, and the
retail input reader and action builder turn it into the same action a human
produces. Clankers can be added to a custom game from Game Setup, or fill a
Live match in **Clankermatch**, a matchmaking row that plays exactly like
OptiMatch. The retail controller traces are in §4, implementation notes in
§10 and game checks in §11.

Navigation generator 5 routes through lifts and teleporters, including
converted Halo 1 one-way exits. See [Navigation](#32-navigation),
[Belief and map knowledge](#belief-and-map-knowledge) and
[Personality and temperament](#personality-and-temperament).

Labels: **traced** means read from the retail XBE with `h2 disasm`;
**Repo** means already in this repository; **Design** means our decision;
**Run** means only a game run can settle it. The page holds addresses and
paraphrase only, no game data. Addresses, traces and test lists are in the
[detail note](notes/2026-11-26-bots-detail.md).

## Clanker appearance and Live player options

A Clanker's full gamertag determines its emblem and armour colours in System
Link, simulations and Live rosters, so the same name looks the same on any
host. The profile model still comes from the host, and team games can
override the visible armour colour.

`live_proto::bots::appearance` lowercases and whitespace-collapses the tag,
hashes it with FNV-1a and picks one of 64 foregrounds and 32 backgrounds.
The server recomputes looks when projecting every `BotIssued`. The web mock
uses a C mirror (`web/src/lib/clanker_appearance.c`), regenerated with
`python3 scripts/clanker-web-kernel.py`. Details: see the
[detail note](notes/2026-11-26-bots-detail.md), "Clanker appearance and Live
player options".

Retail trace: the player options screen is screen 31, reached from the lobby,
Y menus, friends list and Recent Players through the loaders at `0x2b70a3`
and `0x2b71a6` (shared constructor `0x2b6106`). Hooks leave name-only rows
and human accounts on the retail path. Recent Players rows for a reserved
Clanker XUID open a read-only card (`clanker_player_options`) with Gamertag,
Difficulty, Style and Back; a remote or recent bot resolves through
`GET /v1/games/bots/{xuid}`. The card has no mute or feedback, and both
server homes refuse friend requests and mailbox writes to bot XUIDs with
HTTP 403. Traces: see the [detail note](notes/2026-11-26-bots-detail.md),
"Clanker appearance and Live player options".

Still needs a game run: which handler each lobby, in-game, recent and friends
row exercises. The menu parity check is described in
[Crawling the menus](../runtime/agent-api.md#crawling-the-menus).

## 1. Decision

A Clanker's brain emits sticks, triggers and buttons. The host puts the
Clanker in the session as a phantom machine with one player. Each tick, the
retail input reader and action builder turn its arrived pad into an action for
the remote gather, and the game applies its usual movement, aim assist and
combat rules.

Difficulty never multiplies damage. A Legendary Clanker and a human deal the
same damage; difficulty changes decisions and controller skill (reaction
time, aim, shot placement, grenade timing). Read a running host with
`h2 agent bots.stats`; uploaded simulations show results in Clanker Training.

Custom games, Clankermatch and headless sims all use phantom peers. A match
has up to 16 players, and a playerless sim has room for 15 bots. See
[the phantom path](#5-way-in-2-phantom-peers).

AI actors (`actr` with a biped) are a different retail feature: they do not
take part in scoring, objectives or the carnage report. The navigation mesh
uses the project's pathfinding code (§3.2).

## 2. What we traced

Addresses, field tables and walkthroughs for this section are in the
[detail note](notes/2026-11-26-bots-detail.md), "2. What we traced".

### 2.1 Players and machines (traced)

The players array is built by `players_initialize` `0x14b4b0` (16 entries,
stride `0x21c`, header at `[0x4e8c24]`).

- **Players globals at `[0x4e8c20]`:** `+0x2c` is the bitmask of valid
  machines.

One machine has at most four local users, and each phantom occupies its own peer
slot. Local slots are 4 entries of `0xc70` bytes at `0x54e8e0`; flag `0x10`
means a profile is loaded and `0x20` means signed in to Live (§4.2).

### 2.2 The retail action path (traced)

Each tick `0x1870a0` calls `0x186ab0` per local user, which runs the
player-control update `0x185be0` and fills a `0x5c`-byte action record.
`0x69040` submits the actions to the simulation, and `0x847d0` stores the
record per slot mode. Simulation globals are at `[0x4cf77c]`, mode at `+8`;
in multiplayer the host is distributed server and clients are distributed
client.

A phantom supplies its pad when the host gathers the remote slot. After uplink
delivery the same retail reader, layout mapper and action builder run in the
phantom's private local-user view. See §5.1, "The action from a pad".

#### Multiplayer round start (Built)

The retail engine can be in round state 1 while the local view is black.
`bots::guest::round::playing` is true once the published and requested states
are 1 and shared input (`[0x4e8c20]+6`) is enabled. While it is false the
phantom gather hook (`Phantoms::slot_action`) returns no action and clears the
seat's queue, so a late action cannot cross into the next round. The opening
fade stays with `guest::match_opening_fading`. Walkthrough: see the
[detail note](notes/2026-11-26-bots-detail.md), "Multiplayer round start
(Built)".

### 2.3 Views (traced)

`render_game_views` `0x2234e0` draws one view per local user. Phantom players
have no host local-user entry, so the host's views, HUD and sound listeners
use the humans' mappings only.

### 2.4 AI (traced)

`ai_update` `0x1c83c0` is gated by the enable byte at `[0x4f55d0]`
([ai-control.md](../halo1/ai-control.md)). MP scenarios carry no characters,
squads or `pathfinding` blocks (Repo: the h1ai and pathfind work generate
them for converted maps), so a retail-AI route would need a whole AI layer.

### 2.5 Menus (traced, Repo)

The main-menu row handler is `0x230827`; the players screen is `0x1e`
(constructor `0x2524cc`). The Live menu is screen `0xba`, with rows
Quickmatch, OptiMatch, Clankermatch, then Co-op Campaign, Custom Games,
Forge, Firefight, Custom Game Browser and Mods. `crates/extensions/game/src/custom_games.rs`
adds our rows. Matchmaking is in
[live-matchmaking.md](../live/live-matchmaking.md); the server-side matcher is
designed but not built. Game Setup (`squad_settings_dialog`, screen 23)
carries our **Halo 1 Options** row, which opens the shared `h1-options`
dialog (docs/game/ui-builder.md "The settings dialog"; store in
`crates/extensions/game/src/h1_options.rs`). Row tables and hook addresses:
see the [detail note](notes/2026-11-26-bots-detail.md), "2.5 Menus".

## 3. The brain

The brain is a pure function, run once per tick for each bot:

    (perception, memory, params) -> (pad, memory')

It runs on the host's game thread after the tick's world state is final and
before `0x1870a0` reads input. It does not know which way in it uses.

### 3.1 Perception

The brain reads game state the way telemetry does
([live-gamestate.md](../live/live-gamestate.md)): players and their units,
pickups in the world, the objective (flag, bomb, ball, hill, territories),
nearby grenades, shots and kills for sound and emote triggers (§3.5), and in
Firefight the AI's living actors as enemies (§3.10).

**Line of sight** is a ray against the map's BSP collision. A bot sees an
enemy only when a ray reaches the head or chest, that part is inside the view
cone and visual range, and the reaction delay (§3.4) has passed. The bot
aims at the part it sees (`mind::sight_point`).

#### Visual range

A bot sees what a person sees on screen (`perceive::notice_range`), for
players and Firefight actors alike. The cone is 40 degrees off the aim
(`mind::NOTICE_CONE`); past it only a player within 3 units
(`mind::NOTICE_NEAR`) is noticed. Range falls to 0.4 at the cone edge
(`EDGE_SIGHT`) and to 0.75 for a crouched player (`CROUCHED_SIGHT`).

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| sight range (world units, `perceive::sight_range`) | 25 | 35 | 45 | 55 |
| camouflage, still (`perceive::camo_range`) | 1.5 | 2 | 2.5 | 3 |
| camouflage tracking error (times the skill's, `look::camo_aim`) | 2.5 | 2.2 | 1.9 | 1.6 |

Scopes (`SCOPE_SIGHT`, `SCOPE_CONE`) and active camouflage (`CAMO_MOVING`,
`look::CAMO_ENGAGE`) adjust the range. Rules: see the
[detail note](notes/2026-11-26-bots-detail.md), "Visual range".

#### Visibility grid (`bots::vis`)

The visibility grid answers sight rays between places that can never see each
other without casting a ray. It is built once per map, offline, with the nav
mesh, and shared read-only by every bot. Each bit of a cell pair means
**never** (proved against one collision triangle, not sampled); unset means
maybe. `los::Walls::clear` consults it first, so a match plays the same with
or without it. The file is `<data>/bots/<map>.<bsp>.vis` beside the `.bots`
file (magic `H2VS`, `vis::VERSION`); bump `vis::VERSION` when the cells, the
proof or the margins change. The nav worker builds it resumably
(`vis::rebuild_resumable`, progress in `.vis.part`), and `autonav::offer_vis`
installs a grid built mid-match. Build, loading, log lines and tests: see the
[detail note](notes/2026-11-26-bots-detail.md), "Visibility grid
(`bots::vis`)".

The host gives each bot its own view, teammate HUD markers, audible shots, its
motion tracker, radio calls and public match information. Each brain keeps a
short tick-stamped history of samples and thinks from the newest one at least
`perception_ticks` old; a repeated poll in the same tick reuses the previous
pad, and the first live sample after a respawn starts a fresh history.

#### The motion tracker

A phantom has no local-user HUD slot, so `bots::guest::radar` builds the dots
its tracker would show, from the variant's motion sensor flag, the map's
`hudg +0x290` range and the speed threshold read by `0x1a1d55`. A hostile
player shows when it moves fast or fired within `sound::HEARD_TICKS`. The
brain gets up to `radar::DOTS` (4) nearest dots, with no identity or height;
a dot never picks a target, only sight does. `mind::tracker` remembers dots
for `KEEP_TICKS` and feeds alert sightings, support, glances, behind-checks
and floor checks. Rules and tests: see the
[detail note](notes/2026-11-26-bots-detail.md), "The motion tracker".

### 3.2 Navigation

Every map gets a nav mesh, built offline from the BSP collision and cached by
map fingerprint. `bots::autonav::build_map` reads the scenario and builds one
`<map>.<bsp>.bots` per BSP. `bots::cache::build` starts from the sectors of
`pathfind::generate::generate` and adds links and collision probes. Generator
version 7 regenerates earlier caches.

The mesh holds walkable surfaces (upward-facing collision planes joined across
shared edges, with step-up and headroom checks for the biped) and links:

| Link | Meaning |
|---|---|
| drop | a ledge to a lower surface within the fall-damage limit |
| jump | a gap or ledge reachable on a swept jump arc; a standing jump climbs up to 0.6 (`nav::Build::max_jump_up`, a **[guess]**) |
| crouch-jump | takeoff and landing for ledges up to 0.6 + 0.225; the follower holds crouch after takeoff |
| teleporter | directed, each sender to all receivers of its channel on this BSP |
| lift | directed, a gravity lift, launcher or elevator to a BSP landing |
| vehicle | the ground-vehicle planner filters the walk graph (§3.9) |

Checks (`nav::stands_on`, `nav::crossing_clear`, `nav::ledge_lift`), jump
numbers and ride behaviour: see the [detail
note](notes/2026-11-26-bots-detail.md), "3.2 Navigation".

**In the map.** Each polygon is in the map or out of it
(`nav::NavMesh::mark_out_of_map`, run by `autonav::build_map`). A polygon is
in when a player start or weapon pad reaches it over links a player takes
without a glitch; a superbounce is never a link. `autonav::mark_in_map` runs
the same classification for the Firefight kit ([firefight.md](firefight.md),
"Sectors the AI can use"). Out-of-map polygons are kept, because a bounce
route (§3.8, "Superbounce routes") can land a bot there. The rules:

- a search that starts in the map never ends in or passes through an
  out-of-map polygon (`nav::search`, `NavMesh::in_map`);
- a search that starts out of the map goes anywhere, so the bot plans back in;
- goal picking leaves them out (`NavMesh::nth_in_map`).

Paths come from A\* over the sector graph, smoothed with a funnel pass and
followed by steering (look-ahead point, corner slowdown, stuck recovery). A
move becomes a left-stick vector relative to the bot's facing; a bot never
warps. The agent API's [routes](../runtime/agent-api.md#routes) use the same
search, follower, pad steering and jump arc to drive a local player.

**Traversal evidence and estimates.** Scenario offsets for teleporter flags,
scenery, machines and crates, lift identities, clearances and the cost
allowances (teleporter and lift ride times, all estimates) are in the [detail
note](notes/2026-11-26-bots-detail.md), "3.2 Traversal evidence and
estimates". Graphs with these links search with a zero distance heuristic.

#### Traversal audit by map

Directed teleporter and lift edge counts per retail H2 scenario, Map Pack
scenario and converted H1 scenario come from the ignored test
`bots/tests/map_corpus.rs`, which rebuilds all 34 from source. Tables:
see the [detail note](notes/2026-11-26-bots-detail.md), "Traversal audit by
map". A landing that clears the BSP still needs a game ride to confirm the
deck. There is no separate ladder action.

#### Belief and map knowledge

**What can reach a decision.** The phantom pad source passes host candidates
through `perceive::observe` before combat, movement, radio or memory. The
paired-world harness `perceive::honesty` runs seeded brains with separate
radios and compares inputs, controller output, activity, kit and outgoing
enemy call. Enemies need view cone and BSP sight; the motion tracker gives
approximate dots only (§3.1, "The motion tracker"). The per-input table is in
the [detail note](notes/2026-11-26-bots-detail.md), "Belief and map
knowledge".

A visible power item has probability 1 and a visibly empty pad 0. The
return time comes from a witnessed period, then a placement override, then the
collection default, then a 60-second prior. Indirect reports keep their
original observation tick. The cache stores lanes, sightlines, vantages,
player starts and power pads beside the mesh. A calm sniper can hold a
vantage; patience extends the wait at a projected return. Hit with nobody in
view, the bot faces the newest shot that is not its own for `HIT_LOOK_TICKS`
(45), and its next sighting in that time is an alert one (§3.4). Per-bot
experience records (supply visits, kills) live in the brain's memory for its
lifetime. Vantage scoring, spawn-time values and the experience snapshot: see
the [detail note](notes/2026-11-26-bots-detail.md), "Belief and map
knowledge".

**Getting unstuck** (`crates/live/bots/src/stuck.rs`). A pure state machine,
run every tick the bot walks somewhere (not while it strafes in a fight). A
20-tick window with under 0.25 world units of gain along the wanted direction
escalates one step:

| Step | What the bot does for its window |
|---|---|
| sidestep | backs off and strafes to one side, then the other if blocked |
| jump | walks on and presses A for 3 ticks |
| crouch-jump | jumps, then holds crouch from the third tick |
| repath | blacklists the waypoint, drops the path and goal, plans again |

Gaining 0.25 is recovery; stuck again within 3 s resumes from the step that
freed it. Two slower checks go straight to repath: **penned in** (within 1
unit of one point for 4 s) and **no closer** (goal not 0.5 closer for 10 s).
A blacklisted point is avoided for 10 s within 0.75 units, 16 at most
(`NavMesh::path_avoiding`).

**Bodies.** A body ahead on the walk line turns the walk away, keeping speed
(`moves::around_bodies_safe`); the fight target is not avoided. The
follower's own stall check (`nav::Follower`) runs alongside. Numbers: see the
[detail note](notes/2026-11-26-bots-detail.md), "Bodies and follower".

**Corners and doorways** (`nav::NavMesh::portal`). Portal ends at mesh corners
are pulled in by `nav::CLEARANCE` (0.3), and doorway portals are cut to the
part the far polygon lies along. `mind::Mind::go` checks the cornering
direction.

**Off the mesh** (`nav::NavMesh::nearest`). A start or goal off the mesh snaps
to the nearest polygon within 3 units whose floor is at most 0.6 over it or 3
under it. A bot whose next waypoint is more than 1.2 over it replans at once.

**Moving in a fight** (`crates/live/bots/src/moves.rs`). Footwork by
difficulty:

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| holds a strafe (ticks) | 40 to 80 | 20 to 50 | 12 to 35 | 8 to 26 |
| a new strafe is standing still | 35% | 15% | 5% | never |
| on a hit: reverses the strafe (juke) | never | 20% | 50% | 70% |
| on a hit beyond 8: crouches a moment | never | never | 10% | 15% |
| backs off inside its range | no | 0.6 | 0.85 | full |
| backs off to cover | no (straight back) | yes | yes | yes |

A hit is a shield drop over 0.03 in a tick. The range comes from
`moves::preferred_range` (2.5 to 8 for every weapon). An enemy more than
`moves::HEIGHT_GAP` (1) above or below is reached by the nav route
(`Mind::go`), and a bot with shields down backs away to cover. Hops use
`moves::FIGHT_HOP` (0.5); grenade dodges are in
`crates/live/bots/src/incoming.rs`. Footing probes, cover search and stuck
rules: see the [detail note](notes/2026-11-26-bots-detail.md), "Moving in a
fight".

**Roaming** (`crates/live/bots/src/roam.rs`). With nobody to fight, each
candidate place is scored and the best reachable, unblacklisted one wins:

- a place it saw an enemy in the last 60 s (4.5, fading to 0.5), including a
  teammate's call and a guessed respawn;
- a weapon lying in the world worth taking (1.5);
- the map's spawn points (1.0);
- six random in-map polygons (0.5);
- less 2.5 for a place visited in the last 40 s, and 2 for one under 4 away;
- plus a random wobble: 8 Easy, 2 Normal, 1 Heroic, 0.4 Legendary.

#### Pace and hops

A named bot's stick pace and run hops come from its name profile
(`mind::stride`); an unnamed bot keeps its thumb's pace times
`nav::CRUISE_PACE` (0.94) and never hops. A named bot travels at full stick,
keeps its strafe weight in a fight, and rolls a hop every `HOP_EVERY` (30 to
90) ticks on open straights with `HOP_CHANCE` (0.6) times the jump weight.
Tests: `a_named_bot_runs_at_full_stick_and_hops_as_its_name_likes`
(`mind/tests.rs`) and `mind/stride.rs`.

### 3.3 Decision

The design calls for a utility layer running about five times a second. Its
inputs:

| Action | Driven by |
|---|---|
| fight | a visible enemy; my health; my weapon against theirs at that range |
| chase | an enemy seen recently, its last known position |
| hold angle | a patient bot keeps aiming at its last sighting for a moment |
| pick up weapon or powerup | improvement over the held gun, distance, estimated availability and position strength |
| seek health | shields down, no enemy in view (cover, then a wait for recharge) |
| play the objective | gametype; my role (carrier, escort, defend, attack); team state |
| flee | outgunned, low health, a grenade nearby |
| emote | §3.5 |
| roam | the default: move toward the likely fight (spawns, weapon spots) |

**In play today.** `Mind::step` chooses an activity per tick. Carriers move to
their scoring point and suppress weapon switches; objective players stay on
assignment. A bot can fire while walking to the hill, and in a held zone keeps
its fight footwork (`objective::within`, `FOOTWORK_ROOM`). Without a target in
Slayer, `Mind::between_fights` (`crates/live/bots/src/mind/strategy.rs`) takes
in order: recover shields, regroup, its own lead, a teammate's call, supply
or grenades, a heard shot, move up, a firing position, roam. A team call alone
never authorizes a shot. Detail: see the [detail
note](notes/2026-11-26-bots-detail.md), "3.3 Decision".

#### Finishing and breaking off

A softened target is run down (`tactics::finish_push`, `weapons::Combo::softened`)
and the bot withdraws when outnumbered beyond its margin; the Juggernaut never
withdraws. Recovering shields, it backs away for
`mind::RECOVER_BACK_TICKS` (1.5 s) and then watches the last sighting.

**Full shield.** Every shield line reads the shield as a fraction of the full
shield (`guest::full_shield`, `Me::shield_full`, `mind::relative_shield`),
so handicaps and overshields do not break the lines. Without shields,
vitality stands in. Tests:
`mind::tests::shields_down_is_a_fraction_of_the_full_shield`,
`without_shields_it_backs_off_on_health_alone` and
`guest::tests::the_full_shield_follows_the_variant_and_the_handicap`. The
games that showed it and `bots.stats` `combat.standing_decisions.recover`:
see the [detail note](notes/2026-11-26-bots-detail.md), "Finishing and
breaking off".

Reload (X) and switch (Y) take 5 to 13 ticks of stable intent, then hold 2 to
4; retries are at least 2 s (reload) and 1.3 s (switch) apart. Vehicles (§3.9)
take the whole pad in a vehicle; on foot, boarding comes after the order above.

**Warm-up.** For its first 2 to 4 s of a match (`mind::warmup`,
`WARMUP_MIN_TICKS` 60 to `WARMUP_MAX_TICKS` 120) a bot picks no target; the
length comes from its name hash, and damage ends it. `Brain::reset_mind`
starts it again each match.

Grenades: a bot throws at 3 to 12 units at a cluster, a closing enemy
(`grenades::Mark::closing_on`) or a vanished one, with an eight-second
cooldown. Rates and gates: see the [detail
note](notes/2026-11-26-bots-detail.md), "Finishing and breaking off".

#### Personality and temperament

The full gamertag fixes a Clanker's combat profile; each host computes it in
memory. `crates/live/bots/src/name_profile.rs` hashes the UTF-8 gamertag with
64-bit FNV-1a (ASCII folded to lowercase), giving one try-hard score and one
draw per trait. Each trait blends them and scales into the difficulty band;
the custom-game style shifts tendencies within it. Host, seat, match seed and
account ID do not change the profile; the seed controls choices in play.

Traits: aggression, caution, teamwork, objective focus, composure, patience,
showboating, weapon loyalty, chattiness. Per-difficulty bands, the favourite
weapon families and `h2 agent bots.stats` weights: see the [detail
note](notes/2026-11-26-bots-detail.md), "Personality and temperament".

Confidence, tilt and fear run from 0 to 1, decay with game time (half-lives of
about 8 to 17 s by composure), and make a bot commit, throw or withdraw more
or less. Tracking noise from tilt and fear is capped at 10%.

The map decision layer uses `Personality::wait_weight()` and
`Personality::weapon_interest(role)`. See [the agent bot
view](../runtime/agent-api.md#sim-simulated-players-and-direct-state) for the
live traits and mood.

### 3.4 Aim

Aim feeds the biped's current view back into a model of a human thumb
on a stick.

| Part | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| fresh reaction (ticks from first sight) | 10–18 | 7–13 | 5–9 | 5–8 |
| perception delay (ticks) | 3 | 3 | 2 | 2 |
| motor/controller delay (ticks) | 2 | 2 | 2 | 1 |
| continuous delay (ms at 30 Hz) | 167 | 167 | 133 | 100 |
| button-edge timing σ (ms) | 8 | 6 | 4.5 | 3 |
| tracking error (std. dev. of the angle at 5 units, rad) | 0.10 | 0.06 | 0.035 | 0.02 |
| lead (fraction of the needed lead: flight and own delay) | 0.3 | 0.6 | 0.85 | 1.0 |
| turn asked per radian of error (rad/s) | 4.8 | 7.5 | 10.5 | 13.5 |
| flick overshoot (mean, test plant) | 10% | 7% | 6% | 5% |
| automatic inside the gun's band | held | held | held | held |
| automatic past the band (ticks on, off) | held | 24, 8 | 16, 8 | 12, 8 |
| grenades (arc solve) | rarely, poor | sometimes | often, good | often, good |
| weapon choice by range | clear mismatches | yes | yes | yes |

These are tuning defaults for playtesting. The mind sees the world
`perception_ticks` late and a report lands `motor_ticks` after it. The hand
steers by a model of the pad's response (`mind::look::RESPONSE`), predicts
its own facing across the delay (`Hand::ahead`), rests on a still target
(`mind::look::rest_band`), and flicks at `mind::look::FLICK_STEP` per tick.
Aim error shrinks with range (`mind::look::spread_scale`), and the aim point
leads a moving target by the skill's `lead` (`mind::look::lead_time`).
Detail: see the [detail note](notes/2026-11-26-bots-detail.md), "3.4 Aim".

**The fire cone** (`mind::look::fire_cone`). The trigger is pulled while the
aim is within `clamp(0.35 × (1 + looseness) / d, 0.012, fire_cone + 0.3)`
radians of the aim point. Looseness is per weapon role: sniper 0, precise
0.15, blast and unknown 0.5, spray 0.7, shotgun and sword 1.

**Holding the trigger** (`tactics::hold_pattern`, `tactics::Hold`). An
automatic inside its gun's band is held until the target or magazine is
gone, at every skill. Past the band Easy holds; Normal and up fire the
skill's bursts (the table's on/off ticks).

The reaction gates only the trigger, melee and grenades. Until it is over
the bot keeps walking and turns toward the enemy at `REACTION_GAIN`; an
alert bot reacts two ticks sooner (`Tactics::focus_alert`). Tests:
`it_keeps_moving_and_starts_turning_before_it_may_fire`,
`shot_from_behind_it_turns_to_the_shooter_within_ten_ticks` and
`a_moment_out_of_view_does_not_freeze_it_again`.

At the controller boundary each stick stays in its unit circle and moves at
most 0.45 stick radii a tick. The right thumb does one job a tick. Button
edges draw seeded Gaussian timing errors, so the same seed, samples and
tuning reproduce the same input.

A tuning file's `burst_on` and `burst_off` set the bursts past the band;
`burst = false` holds the trigger at every range, and `burst = true` on
Easy uses Normal's 24 on, 8 off.

`bots::tuning::Tuning` exposes `perception_ticks`, `motor_ticks`,
`button_sigma_ms`, `stick_step`, `thumb_switch_ticks`, `phantom_ping_ms`
and `phantom_jitter_ms`. The sim accepts these in bot tuning files
([sim.md](../runtime/sim.md#bot-parameters-from-a-file)). Grenades are flown
against the map's collision and thrown only when one lands (`bots::grenades`,
"Built: weapon skill" in §10). The table's numbers are starting points;
tuning them needs runs (§11).

#### Phantom aim diagnosis and next steps

A phantom's hits reach the host, but its aim missed more than a local bot's.
The host runs the retail local builder (`0x186ab0`) on each arrived phantom
pad and validates the full action at `0x14d0a0` (§5.1). To check a phantom in
a hosted match, read `aim.assist_player` and `aim.assist_strength` from
`sim.player.list` as a target enters and leaves the reticle. For aim
comparisons, run matched phantom rosters with the same maps and seeds
(§3.4.1). Replay numbers and the retail addresses: see the
[detail note](notes/2026-11-26-bots-detail.md), "Phantom aim diagnosis and
next steps".

#### 3.4.1 The aim net (compared: the code aim stays)

**Result.** The code aim won 60% of 80 comparison matches to the net's 40%.
The net lost on accuracy, damage and grenades, settles 1.3 s slower and
jitters more. `code` stays the default; the net stays in the tree as a
starting point. Numbers: see the [detail note](notes/2026-11-26-bots-detail.md),
"3.4.1 The aim net (compared: the code aim stays)".

`bots::aim_net` is a small net with hand-written `const` weights: nothing is
trained and there is no ML dependency.

**The switch.** `aim_model = "code" | "net"` in a bot's tuning
(`bots::tuning::Tuning::aim_model`, `sim::config::BotParams::aim_model`).
`H2_BOT_AIM_MODEL=net` sets the default for bots whose tuning does not name
one. The default is `code`.

**What it replaces.** Only the step that turns the hand's view of a tracked
target into the stick it wants (with `code`: tracking lag, flick offset and
`gain × error` through the stick curve; with `net`: one forward pass). The
view, the hand's error, the thumb, the controller, target choice, the fire
cone, movement and the action builder are shared. A look with no tracked
target uses the code's curve under either model.

**Inputs, hidden units, outputs.** 18 inputs (error, error rate, target
rate, stick, pending turn, time since acquired, apparent size, weapon
looseness, the skill's `gain`, `error` and `lead`, and the hand's noise,
yaw and pitch), 24 ReLU hidden units in mirrored pairs (pull, boost cap,
breakout, breakout cap, lead, lead cap, follow-through), and 2 outputs
(yaw, pitch turn, clamped to -1..1) that become the right stick. No fire
gate: the trigger stays with the shared fire cone. Unit table and the
delay accounting: see the [detail note](notes/2026-11-26-bots-detail.md),
"3.4.1 The aim net (compared: the code aim stays)".

**Tests.** `cargo test -p bots aim_net` covers the network alone.
`cargo test -p bots look` runs both models through one kinematic harness and
holds the code aim's own tests, among them
`a_flick_on_a_late_view_does_not_swing_past`,
`on_a_still_target_the_thumb_rests_and_on_a_moving_one_it_tracks` and
`a_large_flick_takes_the_thumb_to_full_stick_at_once`.

**The comparison.**

```sh
H2_OPT=2 h2 sim batch docs/runtime/sim-examples/aim-net-vs-code.toml --disc disc/halo2.iso --parallel 3 --pace unpaced
```

Midship and Lockout, four skills, five seeds, two against two, each match
played twice (`mode = "arm_swap"`) for 80 matches; only `aim_model` differs
between arms. `compare.txt` under
`docs/runtime/sim-examples/results/aim-net-vs-code/` has net − code with a
95% interval per row (`accuracy`, `kd`, `win`, `kills`, `deaths`, `ttk_s`,
`aim_error`, `overshoot`, `settle_s`, `jitter`, `reversals`).

**The decision rule.** The net wins if, at three skills or more, it is
better with the interval clear of zero on one of `kd`, `accuracy` and
`ttk_s`, worse on none at any skill, and no less like a hand (`jitter`,
`reversals`, `overshoot` in 0.03 to 0.25). The code wins on the reverse.
Anything else is close, and the code stays. Each model's `accuracy` must
also rise from easy to legendary. The loser is deleted, not kept behind the
switch; the aim-motion rows and `arm_swap` stay either way.

### 3.5 Emotes (the teabag layer)

A short burst of crouch presses over a body, made with the real crouch
input. The kill feed supplies the body position and the event. Self-kills
(including world deaths) and betrayals can prompt a taunt; showboats also
taunt over their own kills, at a lower rate.

The bot may emote only when:

- the body is within 1.5 world units (about 4.5 m: `emote::NEAR`, the
  game's teabag radius, so every emote is a teabag for the Incel medal,
  [teabag.md](teabag.md#the-rule));
- no enemy has been in the bot's view for 1.5 s;
- the bot is not carrying, defending or contesting a live objective;
- it has not emoted in the last 30 s.

It presses crouch 3 to 6 times (130 to 200 ms each) for 2 s at most, and
stops at once if an enemy appears, an objective takes priority, or taunts
are turned off.

    chance = clamp(taunt × severity(event) × showboating_weight, 0, 1)

The Clankers page's **Taunts** switch sets `taunt` to 0 when off. A
persistent bot has its own base taunt value. Severities: see the
[detail note](notes/2026-11-26-bots-detail.md), "3.5 Emotes (the teabag
layer)".

### 3.6 Weapons and ammunition (traced)

The bot reads what it carries and what lies about
(`crates/live/bots/src/arms.rs`), and a pure decision (`arms::plan`) turns
that into a reload, a weapon switch, a walk to a weapon, or a melee.

**Fields.** Traced offline from the XBE with `h2 disasm` (static reading, no
game run). `U` is the unit's object body, `W` a weapon's, `D` the weapon's
`weap` tag metadata. The key offsets, all confirmed:

| Field | Where | Type |
|---|---|---|
| held weapon slot | `U+0x212` | `i8`, -1 none |
| weapon datums | `U+0x218` | `u32[4]` |
| magazine count, pointer | `D+0x2c0`, `D+0x2c4` | `i32`, `u32` |
| rounds a magazine holds, most in reserve | magazine definition `+0xa`, `+0xc` | `i16` |
| reserve, loaded rounds | `W+0x22a`, `W+0x22c` | `i16` |
| barrel count, pointer | `D+0x2d0`, `D+0x2d4` | `i32`, `u32` |
| battery used (0 fresh, 1 dead) | `W+0x184` | `f32` |
| a weapon's holder | `W+0x14` | `u32` datum |

The remaining rows, with their evidence: see the
[detail note](notes/2026-11-26-bots-detail.md), "3.6 Weapons and ammunition
(traced)".

**How the bot reads them.** `arms::read` resolves the unit's four weapon
datums. A weapon whose definition ages is a **battery**; one with magazines
and no aging barrel is **rounds**. `arms::pickups` walks the objects array
(at most 2048) for live weapon objects with no parent. A fault while reading
leaves the bot without the weapons layer (`Me::arms` is `None`); it never
blinds it.

**Handling.** The gun's kind comes from fields of its `weap` tag and first
barrel's projectile (`arms::WeaponDef::profile`, `arms::Profile`). The
**role** (`arms::Role`): two zoom levels a **sniper**; once per pull with a
projectile under 100 a second a **blast** weapon; 4 or more projectiles a
shot a **scatter** gun; once per pull otherwise **precise**; held or charged
**spray**; no trigger but aim assist a **blade**. A fault reading the first
trigger or barrel leaves the gun weapon-blind. Each role has a band it is
best at and a range it shoots to (world units):

| Role | Band (`arms::preferred_range`) | Shoots from, to | Rating |
|---|---|---|---|
| unknown (not read) | 2.5 to 15 | 0 to 18.75 | 1 |
| blade | 0 to 1.5 | (melee) | 4 |
| scatter | 0 to 4 | 0 to 5 | 4 |
| spray | 1.5 to 9 | 0 to 11.25 | 2 |
| precise | 5 to 28 | 0 to 35 | 3 |
| sniper | 12 to 60 | 0 to 75 | 5 |
| blast | 5 to 25 | 2.5 to 31.25 | 5 |

Every gun stops shooting at 1.25 times its band's far end
(`weapons::FIRE_PAST_BAND`); the band comes from the tag (`weapons::reach`)
and the table is the fallback. Slow bolts (plasma rifle and pistol, any
automatic under 150 world units a second) keep the target within 6 and fire
to 7.5 (`weapons::SLOW_BOLT_BAND`, `SLOW_BOLT_FIRE`; [round 1 on
Midship](notes/2026-11-23-bots-natural-round1.md)). Past the band the
trigger works in bursts from Normal up (§3.4).

**The decision** (`arms::plan`, pure, in order): a blade stays out in lunge
range; a dry gun switches, melees or seeks a pickup; an empty magazine with a
reserve reloads (Normal and up switch instead if the other gun is loaded and
in band); a better-suited gun is drawn; calm bots reload tactically
(`arms::TACTICAL_RELOAD`); calm bots seek a power weapon within 25, a useful
pickup within 12 when low, or a higher-rated gun within 15. Seeking walks
the nav mesh; a gun of a kind already carried is walked over (the game moves
the ammunition, `0x101740`), another kind is taken by holding X for at most
1 s. Thresholds: see the [detail note](notes/2026-11-26-bots-detail.md),
"3.6 Weapons and ammunition (traced)".

**Trips that fail** (`crates/live/bots/src/mind/errand.rs`). A walk to a
weapon that gives nothing, or a held X that takes nothing in 1 s, ends the
trip: the weapon is refused for 30 s and its spot blacklisted for 10 s.

The presses are Halo 2's Default layout (X reload and action, Y switch, B
melee, A jump, left stick click crouch), given to every bot's profile
(`guest::bot_block`, §4.2).

**Dual wielding and power items.** Clankers pair two SMGs, a plasma rifle or
pistol with an SMG, or two needlers on any difficulty; each name sets its
chance of wanting a second gun. Off-hand eligibility is `D+0x12c` bits 22 and
23. Pickup timing of power weapons and equipment is read from the placement
table and two read-only observers (weapon taken at `0x10ca80`, equipment
taken at `0x151ea0`); a bot dates a pickup only from a pad it was watching,
and guesses are marked **[guess]**. Field table, observers and policy: see
the [detail note](notes/2026-11-26-bots-detail.md), "3.6 Weapons and
ammunition (traced)".

### 3.7 Objectives (traced)

Stage 5. The bot reads the gametype, the objective objects and the
scenario's netgame flags (`crates/live/bots/src/objective.rs`), and a pure
decision (`objective::plan`) gives it a goal.

**Fields.** Traced offline from the XBE with `h2 disasm` (static, no game
run). The field table and its evidence are in the [detail
note](notes/2026-11-26-bots-detail.md), "3.7 Objectives (traced)". The
entry points a reader needs:

| What | Where |
|---|---|
| game-engine globals `E` | `[0x4e9ae8]`; engine index `E+0xc14` |
| objective records (at most 8, `0x10` each) | `E+0xc18` count, `E+0xc1c` records; register `0x15e050`, unregister `0x15e130` |
| netgame flags (`0x20` each) | `[[0x4e0350]+0x11c]`; query `0x19ec40` |
| CTF and Assault globals | `[0x51ec80]` |
| Oddball, King of the Hill, Territories globals | `[0x51ecc4]`, `[0x51ecc8]`, `[0x51eccc]` |
| Juggernaut identity | `[0x510c9c]` |

**How the bot reads them** (`objective::read`, from `guest::perceive`). Slayer
and unsupported engines read as no objective. Each record gives an item (the
object, its team, the carrier's player index); a carried item is where its
carrier's unit is. The netgame flags the gametype uses become spots, each
with the game team its designator stands for. King of the Hill takes its
destination from the active corner list and its scorers from the live
occupancy mask.

A fault leaves `Perception::objective` `None`: the bot plays as in Slayer.
Script games ride a retail engine index (rules-engine.md, "Which engine
index a script rides") and keep its objective registry, so the same reads
serve a `.rules` game on the CTF or Assault engine.

**Without the engine's globals.** A rules script that keeps no per-engine
globals (docs/game/rules-engine.md, "Porting the seven engines") still
leaves the bots an objective, read from the netgame flags
(`objective/zones.rs`): Oddball uses the neutral type-4 spawns, King of the
Hill the lowest-numbered hill's corners, Territories `State::Markers` (the
first type-10 flag per identifier; a player within `TERRITORY_REACH` of a
marker stands in it).

**What a bot knows of it.** Perception (`perceive::observe_for_brain`)
keeps a flag or bomb only while the bot sees it, carries it, or a teammate
carries it. The Oddball ball is kept wherever it is, as its waypoint is on
every Oddball HUD. Hill and territory occupancy is masked to the players the
bot knows of.

**Roles** (`objective::role_of`, `objective::Lean`). Every bot sorts its
living teammates and itself by player index; its rank and the team's size
give its slot, so a team's bots agree with nothing said between them. One
alone attacks; of two, the higher index defends; from three, a third
(rounded down) defend, the highest ranks. Humans count in the ranks.

Within the slot the bot plays its name. `Lean::of` takes four personality
draws (aggression, caution, teamwork, objective focus; section 3.3):

| Leaning | From | What it changes |
|---|---|---|
| escorts | teamwork over the mean of aggression and focus | an attacker with a nearer attacker covers them (**escort**) instead of racing; the nearest always goes |
| guard radius | 4 × (1 + aggression − caution), 3 to 8 | how far a defender strays from our flag or plate |
| return range | 25 × (0.5 + teamwork) | how far an attacker turns back to return our flag |
| carrier pull | 1.5 − focus | when an attacker hunts our flag's carrier |

A neutral leaning (all 0.5) gives the ranked plan unchanged.

**Belief** (`objective::belief`, CTF and Assault). The planner works from
what the bot believes (`Belief::know`, `mind::strategy`), since perception
keeps a loose item or an enemy carrier only while in view; bases are public.

- A loose item seen is remembered for 45 s, and forgotten when the spot is
  in view within 15 units and it is not there.
- An enemy carrying our flag is followed to where it was last seen for 4 s;
  then our flag is missing. A missing item stays missing until seen or 20 s
  pass.
- An item never seen is at its spawn, so attackers head for the enemy base
  whether or not the flag is in view.
- A destination the stuck machine repaths at 3 times (section 3.2), or an
  item stood on for 3 s without taking it, is given up for 15 s
  (`Belief::pursue`).

**The decision** (`objective::plan_with`, pure; `plan` with a neutral
leaning and nothing missing). CTF, in order:

1. carrying the enemy flag: **carry** it to our return point, else our
   flag's spawn, along the nav mesh;
2. an enemy carries our flag: **hunt** the carrier (defenders always;
   attackers by the carrier pull);
3. our flag lies more than 2 units from its spawn: **go** and touch it
   (defenders always; attackers within the return range);
4. a teammate has the enemy flag: the rest divide escort and guard duties;
5. attackers: **go** for the enemy flag, or escort a nearer attacker;
6. defenders: **hold** our flag's spawn within the guard radius.

Every bot's goal also takes its name's temper (`objective::Temper`).

**Oddball** (`zones::ball`). Carrying: **keep** it, taking cover and
meleeing at arm's length. Loose: the two nearest (`RUSHERS`) **go**, the rest
**hold** within `BALL_COVER` (6 units). Enemy carrier: **hunt** as a group,
staging by `SHOWDOWN`, `STAGE` and `GROUP`. Teammate carrier: **escort** as a
screen `SCREEN` (3.5 units) out. Between balls: the closest neutral type-4
spawn.

**King of the Hill** (`zones::hill`). Approach the current corners' centre;
in the hill **hold** within 0.4 to 0.65 of its radius by caution
(`HILL_KEEP`, `HILL_KEEP_CAUTION`). A bot of caution `EDGE_CAUTION` or more
covers from just outside when two teammates already hold it. An enemy in
the hill is the preferred target and sends the cautious in.

**Assault.** Take a friendly or neutral bomb to an active enemy plate. In
one-bomb, only the designated defender's plates score. The arming counter
advances on foot inside the arming volume toward `G+0x244` seconds and falls
by three per tick outside. A planted bomb is nobody's target: any goal
within `BLAST_CLEAR` (6 units) of it becomes a **hold** 7.5 out
(`objective::clear_of`). Defenders guard the plate or intercept the carrier.

**Territories** (`zones::territory`). Every bot ranks each teammate and site
pair the same way (contested friendly site, then not ours, then ours, then
nearer), and each site takes one teammate, two where an enemy stands in it.
In its site a bot **holds** within `TERRITORY_KEEP` (1.5 units). FFA
ownership follows the player slot and every bot plays alone.

**Juggernaut.** A hunter's goal is the Juggernaut wherever it is, seen by its
waypoint; hunters ignore each other. The Juggernaut hunts the nearest hunter
in its view and roams otherwise (`juggernaut_quarry`,
`crates/live/bots/src/mind/strategy.rs`); it never withdraws.

**Picking up.** Hold X while the item is loose within 0.5 units in XY and 1
in height (`TAKE_HEIGHT`). A goal counts as reached within its radius (at
least `nav::REACHED`), so a bot does not walk on at an item it stands on.
The held-X path accepts interaction kinds 1 and 7 (`0x15195c`); the
Assault predicate at `0x2415d3..0x241611` rejects a planted bomb or another
team's bomb. Pickup trace: see the [detail
note](notes/2026-11-26-bots-detail.md), "3.7 Objectives (traced)".

### 3.8 Glitches (built, not checked in a game)

The Clankers page has a **Glitches** row (`clankers.glitches`): **Off** (the
default), **Some** (the occasional show-off) and **Lots** (whenever there is
an opening). A glitch is the same button presses a player makes, with the same
timing, on the bot's pad. Nothing in the engine is changed or called. The
tricks work because the recomp runs retail 1.0's code (title-updates.md). No
trick has been seen working in a game yet. Tricks, sources and the full
timings: see the [detail note](notes/2026-11-26-bots-detail.md), "3.8 Glitches
(built, not checked in a game)".

**The code.** `bots::glitches` is pure: each trick is a `Macro` of `Beat`s
(`glitches::plan`), with the rules for when one fits (`glitches::opening`)
and the show-off clock (`glitches::ShowOff`). Fight tricks run in the noob
combo's scheduler (`weapons::Combo::step`, its `ComboIn::glitch` gate);
show-offs run in the mind's roaming (`Mind::step`, `Stunt`). The setting
follows the Taunts path: store, `clankers::Want::read`, the driver's shared
`AtomicU8`, then `Mind::glitches` at the phantom's next pad sample.

**The tricks.** Presses at 30 ticks a second; `MELEE_CONNECT` (5) and
`BURST` (4) are untraced starting points to tune in a game run.

| Trick | Presses | When |
|---|---|---|
| BXR | B, X (held), R | in melee reach, gun magazine full or no reserve |
| BYYR | B, Y, Y, R | as BXR, part-empty magazine and a second gun |
| BXB | B, X, B | as BXR, empty magazine and nothing to load |
| Double shot | R, R (held), X | battle rifle, 6 rounds and a reserve, target in range |
| Quad shot | double shot, Y, Y, double shot | as above, 12 rounds and a second gun |
| Sword flying | Y, X, R, A | sword as the other weapon, gun on target, up to 20 units |
| Crouch jump | A, crouch | roaming, now and then |
| Super bounce | prime at a crouch spot, then bounce at a spot | roaming, a spot within 60 units |

**Skill.** Every skill requests the same sequence. The pad pipeline scatters
its edges (§3.4), so a miss is an ordinary melee, switch, reload, shot or
jump. Skill also scales how often a bot tries: Easy 40% of the setting's
chance, Normal 70%, Heroic 90%, Legendary all of it. Pass rates of the pad
timing: see the note.

**How often.** A fight trick is rolled for at most once a second while one
fits (15% on Some, 60% on Lots, times the skill's share), then not for 2 s. A
show-off comes every 45 to 90 s of calm on Some and every 8 to 20 s on Lots.
A super bounce gets 3 tries on Some and 8 on Lots, then the bot gives the
spot up.

**Never:**

- with the setting Off;
- carrying the flag, bomb or ball, or with an objective (show-offs);
- when it would obviously lose: shields down, withdrawing, or another enemy
  within 8 units (2.5 for a sword fly);
- before its reaction to the target has passed;
- doing anything but fighting or roaming on foot (`mind::glitch_doing`, an
  allowlist). The vehicle work's seat pad returns early, so it must call
  `Mind::drop_glitches` first;
- a show-off with anyone in view, or within 1.5 s of seeing someone.

While a trick runs the mind's own melee, reload, switch and grenades wait.

**The super bounce spots** (`glitches::spots::SPOTS`) hold only measured
spots: a map, the stand, a nudge spread, a drift direction, and for a spot a
standing jump can't reach the takeoff (`Bounce::takeoff`) and landing
(`Bounce::landing`). There is one, on `ascension` (a drop from a running jump,
`spots::MEASURED_FALL`). A bounce needs the player primed first (the pair
word at -3, [superbounce.md](superbounce.md), "The pair word"). On a standing
site the bot steps onto the spot, stands still 4 ticks, jumps and pushes
toward the drift; it knows it bounced when it rises 0.4 units after starting
to fall. Details: see the note.

#### Superbounce routes

`bots::spots` holds what a bot knows of its map's spots: the measured ones,
then the scan of the map's collision geometry (`crates/live/superbounce`:
**bounce** and **crouch** spots). The nav worker scans a map with the nav
mesh (`autonav::Worker::keep_spots`) and saves `<map>.spots`, stamped with
the nav generator, `spots::SCAN` and the map's fingerprint. A scanned bounce
spot becomes a `glitches::Bounce` (source `scan`, spread 0.03).

**Verified spots.** `<data>/bots/<map>.verified` (from `superbounce.verify`)
is read by `spots::verified_for`; a proven spot scores 1 and a failed one is
left out. `spots::store_verified` bumps `spots::generation`.

**Learned knobs.** `<data>/bots/<map>.learned` (a sim training run,
[bounce-search.md](bounce-search.md)) is read by `spots::learned_for`;
`Known::for_map` gives each site its knobs (`Site::knobs`) and line state
(`Site::line`). A `dead` line drops an unproven site; a site with no line
plays as without the file. Details: see the note.

`bounce_route::plan` plans a route purely, from a walk-length function over
the nav mesh:

1. walk to the crouch spot's start;
2. **prime** (`glitches::prime::PrimeRun::at`): crouch, crouch-walk in under
   the lip, hold, let go, walk out; done when the word reads -3, three tries
   then `prime_failed`;
3. a site with no fall: walk 0.8 units behind it, then the bounce run
   (`glitches::BounceRun`). A site that needs a fall: walk to its launch
   point (`bounce_route::launch_from`, `launch_floor`), then drop by the
   flight model (`bounce_route::drop_takeoff`, `air_reach`, `air_stick`),
   gated by `bounce_route::judge_takeoff` and `superbounce::rule::predict`
   (`takeoff_rejected`). Three drops, then `no_bounce`;
4. fly: steer toward the goal at full stick until down;
5. walk the rest.

Each touchdown is classified (`bounce_route::Touchdown`: `bounce`,
`not_primed`, `missed`, `too_slow`, `no_bounce`); each jump or drop is a
`bounce_route::Flight` (`OrderRun::flights`, the last 8). Step detail: see the
note.

Modes: **Help** (the default) bounces only where the walk can't reach or the
route is 4 units shorter; **Fun** takes the highest launch within 60 units
and goes on to the goal. A bounce needs a crouch spot unless the pair word
already reads -3.

**Walks to spots off the mesh.** A walk (`nav::RouteWalk`) puts its goal on
the mesh floor (`nav::floor_goal`) and may end in a **last stretch**
(`nav::TAIL_REACH`); failures are `tail_stuck` and `goal_off_mesh`. A walk to
a crouch spot's start (`RouteWalk::ducking`) crouches under low ceilings.
Limits: see the note.

**A lost walk.** The route plans the walk again 3 times
(`bounce_route::WALK_REPLANS`), then fails `path_lost_<check>_<leg>`
(`bounce_route::lost_on`; checks `stuck`, `fell`, `jump`, `link`, `ended`;
legs `crouch`, `approach`, `nav`). A walk lost at a jump or drop link is
planned around it (`RouteWalk::failed_link`, `nav::route::AVOID_REACH`). The
planner measures walks as they are taken (`bounce_route::path_length`,
`nav::walk_length`).

A plan is bounded: at most 32 path queries (`bounce_route::MAX_QUERIES`),
the 4 nearest crouch spots and 8 likeliest sites. A show-off plans for the
nearest site it has not given up on (`Stuck::blocked`). The arc model (gravity
5.2, air speed 2.5, a **[guess]**) is in the note.

**Priming state.** The brain reads the pair word through a reader the host
installs (`bots::set_pair_reader`; the game's is
`live_telemetry::havok_agents`'s `fixed_pair_word`), only while a route is
asked for or under way (`Mind::wants_pair`). Without it the bot assumes it
is primed after the stand-up; `bots.bounce_status` says which
(`primed_read`). `Mind::primed` is true from the priming to the bounce.

**Orders.** `Mind::order_bounce(to, mode)` (through `PadSource::order_bounce`,
`Phantoms::order_bounce`) asks for a route, played as the bot's whole pad
(`mind::order`, `RoutePad`). A death or a map change ends it (`died`,
`map_changed`); it gives up after 200 s. The agent API's `bots.bounce_to` and
`bots.bounce_status` drive and read it (docs/runtime/agent-api.md).
`PadSource::hold_pad` (`Phantoms::hold_pad`) holds a brain's pad for the
verifier and the sim's seam drops. With Glitches on, a show-off at a spot
with a crouch spot known is a **Fun** route back to where the bot began.

#### Breaking the glass

A crouch spot's walk in can go through glass: Zanzibar's window bounce
crouches on the sill past the pane
([superbounce.md](superbounce.md#zanzibars-window-trace-lab-play)). The glass
is the structure BSP's **breakable surfaces** (flag bit 3, a breakable
index); the game skips a piece once its bit in the live breakable mask is
clear (docs/formats/collision.md, "Breakable mask").

The nav build keeps the panes (`glass::panes`, `los::Walls::glass`, in the
`.bots` file). Before a prime, the route finds the pieces in the way
(`bounce_route::glass_in_the_way`, `glass::in_the_way`). The bot reads BSP 0's
mask each tick (`guest::breakable`, `[0x4ed280]`). With a piece whole, the
bot shoots it from the prime's start (`mind::RoutePad::report_shooting`;
phase `break`, log `break_glass`, then `glass_broken`); after 150 ticks
(`bounce_route::BREAK_TICKS`) or an unreadable mask the route fails
`glass_whole`. Per-line rules: see the note.

`h2 agent world.breakable` reads the same mask
([agent-api.md](../runtime/agent-api.md#worldbreakable-broken-glass)). Tests
(`cargo test -p bots`): `glass::tests`,
`guest::tests::the_breakable_state_reads_each_bsps_mask_after_the_enable_byte`,
`bounce_route::tests`, `cache::tests`.

#### Practising bounces

While the superbounce mode is on (a script requiring `hud.superbounce`; the
game calls `bots::set_practice` from `superbounce_mode`) a bot fights no one.
Its whole pad is a practice loop (`mind::practice`, ahead of everything else
in `Mind::step`):

1. stand a moment, looking round, stepping out of any player's way
   ([Letting players go first](#letting-players-go-first));
2. pick a site within 120 units that a route reaches, by weight
   (`practice::weight`: distance, proven, learned, other bots' claims, recent
   tries, players using it, the bot's tastes, Monster pull, retry, random).
   A try that failed on the way blacklists the site (`Stuck::blacklist`); a
   landing that did not bounce is shrugged off ([Fumbles](#fumbles)). Weights:
   see the note;
3. run the bounce route to the site (`bounce_route::plan_site`), going on
   into a chain when one is planned ([Chains](#chains)), and tally it.

The memory of recent tries (16 sites, 16 crouch spots) is the bot's own.

**Which sites a route reaches.** A pick pairs a site with a crouch spot
(`bounce_route::pair_site`) and keeps the pairing; a site with no approach or
no reachable crouch spot is never tried. An unreachable pick is passed over
for 600 ticks (`practice::OUT_OF_REACH_TICKS`), so a try is never spent
finding out that a site is out of reach.

**Giving up what keeps failing.** A failed try is a strike against its crouch
spot or site (`Mind::practice_strike`; `practice::STRIKE_TICKS`,
`CROUCH_STRIKES`, `SITE_STRIKES`, `GIVE_UP_TICKS`). Failed primes also count
across bots (`practice::PRIME_FAILS`). A proven crouch spot is never given up;
when a pick finds nothing, everything given up comes back. A pick that finds
nothing logs `bots: bounce practice: nothing to try: ...`.

##### How each bot practises

Each bot has a practice style (`mind::practice::style::Style`): every trait a
draw from its name's hash (`NameProfile::seed`) inside its skill's band, so a
name practises the same way every match. Traits: fumbles, chains, Monster
pull, slot and crease taste, roam, novelty, retry, patience. Bands: see the
note.

##### Fumbles

At the start of a try, with its fumble rate's chance, the bot sets one knob a
little wrong (`practice::fumble`: `off_line`, `early`, `late`, `lazy_air`)
and flies it as it is. After a landing that did not bounce the bot shrugs: it
stands 60 to 120 ticks, then with its retry chance goes straight back once
(weighed +8); otherwise the site rests. The try's line ends `; fumbled <how>`.
Offsets: see the note.

##### Letting players go first

A practising bot never blocks a player and never treats one as an obstacle
(`mind::practice::yielding`). It tracks every living player and treats a
site's or crouch spot's floor as used while a player is on it or heading
onto it.

- **Before the jump**: a player using the floor stops the route (`yield`; no
  try, no strike). The bot waits (`yielding` in `bots.bounce_status`) and
  replans once the floor has been clear 20 ticks; past its patience it gives
  the site up for now. Nothing interrupts a prime, a jump, a drop or a
  flight.
- **Between tries**: it steps out of a moving player's course, and off a
  site another bot is on.
- **Between bots**: a bot on its own try goes first; otherwise the older claim
  does (`practice::claim_as`).

Distances and timings: see the note.

##### Chains

A chain is the 2nd to 6th bounce with no ground between
([superbounce.md, "The rule"](superbounce.md#the-rule)). At the start of a
try the bot may plan one (`bounce_route::chain`): at most 3 links, from the
sites a flight can come down on. The route steers each link
(`bounce_route::Step::Chain`, `chain_onto`); a link that comes down without
launching ends the chain. The flight is a trait (`chain::Trajectory`; the
built-in `chain::Ballistic` is conservative). Whether a seam bounces a
landing at a chain's speed is open
([superbounce.md, "What is known about a second bounce"](superbounce.md#what-is-known-about-a-second-bounce));
`Ballistic::max_speed_in` is where a measured window goes. The try's line
ends `; chain M of N`.

##### Monsters

A Monster Bounce launches over 16 units/s ([superbounce.md, "The
rule"](superbounce.md#the-rule); "Monster bounces", under [Single
creases](superbounce.md#single-creases)). A site is a Monster site
(`Site::monster`) when it is a V or a busy point, a slot or crease scoring
0.85 or more (`spots::MONSTER_SCORE`), or its launch is over 16. A bot
weighs one up by +3 times its Monster pull. No approach of its own is
documented.

**The drill.** A sim match's `[drill]` ([sim.md, "The bounce
drill"](../runtime/sim.md#the-bounce-drill)) makes the practice try one site
only (`bots::drill`, `Mind::set_drill`): it walks to the drill's start, asks
the host for its word (`Mind::drill_wants_arm`, `drill::request_arm`), then
runs the route and tallies it. A route started from a word read at -3 reports
`primed_read`.

**Training.** A sim match's `[train]` ([bounce-search.md](bounce-search.md),
"Scheduling a batch") makes the practice fly the run's board
(`Mind::set_train`, `learn::current`, `learn::shared`, `learn::Board::pick`).
One flight is a try (`learn::Try::from_leap`); ends that say nothing about
the knobs are void. A folded round (`Board::fold`) stores its lines with
`spots::store_learned`. `bots.bounce_status` gives the round
(`practice.train`).

**The try's line.** Every tallied try logs one line
(`mind::practice::try_line`, `bots: bounce try: bot 0x.. drill site ...`),
ending with the last flight, the touchdown and its result, plus `; fumbled`
and `; chain M of N` where they apply. A sample line and the drill's Ascension
drop numbers
([the bounce-practice notes](notes/2026-11-21-bot-bounce-sim.md), "The drill
four ways"): see the note.

Bots spread over sites by claims on a shared board (`practice::CLAIMS`; a
claim lapses 300 ticks after its last renewal). A route an agent asks for
(`bots.bounce_to`, `superbounce.verify`) is not tallied. `bots.bounce_status`
gives the tally (`practice`: state `idle`, `route`, `yielding` or `waiting`,
the site, attempts, bounces, reasons), so a game of practising bots is a live
verifier. The game records each bounce as a ghost run, kept in `<map>.ghosts`
([superbounce.md](superbounce.md#ghost-runs)); the bots crate records
nothing. `mind::PracticeEvent` (`Started`, `Intent`, `Primed`, `Chained`,
`Tried`, `Yielded`, `SteppedAside`; at most 64 between takes) is drained by
`Mind::take_practice_events` into sim's report
([sim.md, "Bounce practice"](../runtime/sim.md#bounce-practice)).

#### Tuning the bounce

The route's numbers are one struct, `bots::tuning::BounceTuning`, with
defaults in `bots::tuning::BOUNCE_DEFAULT`
(`crates/live/bots/src/tuning.rs`). A brain's `Tuning::bounce` (a tuning
file's `[bounce]`) replaces it for that brain; `tuning::bounce()` holds it to
what the route can play (`BounceTuning::sane`). A route reads it once, when
it starts. Fields (approach, settle, still, jump, drift, crouch, release,
line-up, drop and fly values): see the note.

A tuning file's `[bounce]` (`sim::config::BounceParams`) sets them for a
sim's bots; `h2 sim sweep` ranks values by bounce rate, then launch height
([sim.md, "Sweeping the bounce tuning"](../runtime/sim.md#sweeping-the-bounce-tuning)).
A test in the game's `sim_glue` holds the schema defaults equal to
`BOUNCE_DEFAULT`, and one in `glitches` that the default tuning plays as no
tuning does.

#### Driving a player

The same planner and route runner drive the local player, sent somewhere by
an agent (`player.goto`, docs/runtime/agent-api.md). `bounce_route` holds
`path_length`, `launch_floor`, `prime_steps`, `approach` and `plan_site`;
`goto::plan` picks from these by target (a point, a crouch spot, a bounce
site) and what to do there (`nav`, `crouch_recipe`, `bounce`).

`OrderRun` plays the plan, its last walk ending within `within`
(`OrderRun::arrive_within`). `goto::Goto` adds a watchdog for a body the
brain does not own: it plans again on `strayed`, `stuck`, `path_lost`,
`stalled` or `pair_lost`, and gives up after 6 replans or 3600 ticks. A jump
is a fresh press. The game's glue (`player_goto`) steps the `Goto` on an
agent ticker and holds the first controller's pad; with no bot in the game
it loads the nav mesh itself. Watchdog thresholds: see the note.

**Untested.** The pair-word read, the priming's timings, the air speed and
the whole route are not checked in a game; the pure planner and the state
machines are unit tests (`bounce_route/tests.rs`, `glitches/tests.rs`,
`mind/order/tests.rs`, `mind/practice/tests.rs`, `goto/tests.rs`).

**Not built:** Butterfly (a two-player trick); BXL, LX, RRBX; super bounces
off moving vehicles (Burial Mounds' Warthog launch). BXR may work only off
host, and Clankers play on the host; check that first.

**Tests.** `cargo test -p bots glitches` checks the requested timing, the
openings and setting, the show-off clock and super-bounce state machine.
`cargo test -p bots human_input -- --nocapture` checks the delivered timing.
`cargo test -p boot clankers` covers the setting and driver.

**Game run (checklist).** For the user, with one Clanker on Legendary,
Glitches on Lots, read through the agent API ([agent-api.md](../runtime/agent-api.md)):
see the note.

### 3.9 Vehicles

The bot sees each seat and its rider. It can enter a free vehicle, board a
slow enemy, pilot a Banshee, drive ground vehicles and man a turret. A rocket
launcher holds its lock on a vehicle before firing. Code:
`crates/live/bots/src/vehicle.rs`, `vehicle/` and `mind/ride.rs`.
Field traces: see the [detail note](notes/2026-11-26-bots-detail.md), "3.9
Vehicles".

**Fields.** `O` is an object's body, `U` a unit's, `V` a `vehi` tag's
metadata and `S` one of its seat definitions. The key ones:

| Field | Where | Notes |
|---|---|---|
| parent (rider's vehicle, turret's vehicle) | `O+0x14` | datum; confirmed |
| position, forward, up | `O+0x64`, `+0x70`, `+0x7c` | relative to the parent when there is one |
| body vitality, wrecked | `O+0xec`, `O+0x10a` bit 2 | `0xd6800` (`guest::BODY_DEPLETED`) |
| the unit's seat | `U+0x1fc` | `i16`, -1 none; set at `0xea405` |
| seats block | `V+0x1c8` count, `+0x1cc` pointer | `0xb0` each |
| seat flags | `S+0` | driver bit 2, gunner bit 3, boarding bit 11 |
| vehicle type | `V+0x1f0` | tank 0, jeep 1, scout 4, fighter 5, turret 6 |

**Reading.** `vehicle::read::sight` runs once a frame per bot in a
multiplayer game. It lists live, unwrecked `vehi` objects with their class
and seats, and seats each listed player's biped by `U+0x1fc`. A fault leaves
the bot on foot. The result reaches the mind through `Mind::see_vehicles`,
apart from `brain::Perception`. `guest::perceive` puts a rider where its
vehicle is.

**Lock-on** (`vehicle::lock`). With a rocket launcher in hand
(`lock::locks_on`) and the target riding a vehicle, the bot aims at the
vehicle's middle with no lead and fires only after holding it in the reticle
for the lock time. The game's lock time is not traced; the bot assumes 1 s
(`lock::LOCK_TICKS`) and adds its margin. The bot does not zoom (§12).

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| waits for the lock | no | yes | yes | yes |
| holds past 1 s | | 0.4 s | 0.2 s | 0.07 s |
| off the vehicle forgiven | | 3 ticks | 6 ticks | 8 ticks |

**Ground-route performance.** Ground-vehicle A* is cached: failed searches
remember reached polygons, successful ones reuse their corridor and each
driver caches portal clearance. Profile: see the detail note, "Ground-route
performance".

**Which vehicles.** By class (`vehicle::Class::spec`).

| Class | Seat taken | Controller behaviour |
|---|---|---|
| Jeep (Warthog) | Driver | Ground routing |
| Scout (Ghost, Spectre, Wraith) | Driver | Ground routing and primary fire |
| Tank (Scorpion) | Driver | Ground routing and primary fire |
| Fighter (Banshee) | Driver | Altitude control, attack runs, boost, evasive rolls, landing |
| Turret | Gunner, standalone or on a teammate's vehicle | Track and fire, dismount if flanked |
| Boat or plane | None | |

The bot looks up the seat's entry markers in the vehicle's render model,
finds a stand point on the floor within the tag's radius and facing limits,
and holds X in range. Stand-point offsets and the 0.6 eye height are
**[guess]**.

**Go and take** (`vehicle::plan::take`, `Mind::mount_plan`). A phantom with
no enemy in view for 1.5 s, no objective and no weapon sought within 20
units picks the nearest valid vehicle within the skill's range: a driven
class, whole (over 0.4), upright, standing still, with a free driver's seat,
nobody of the other side aboard, no nearer teammate, and not shunned in the
last 10 s. It tries X for up to 40 ticks, retries closer, then shuns the
vehicle for 10 s. It confirms entry from `U+0x1fc` and the parent, and
abandons after 12 s.

**Turrets.** A standalone turret or a free gun on a teammate's vehicle is
taken when it has a clear line to recent enemy contact or the objective. An
enemy within 1.8 (or within 5 and off the line) can make the gunner dismount
**[guess]**.

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| takes a vehicle within | 12 | 20 | 28 | 35 |
| mans a teammate's gun | no | yes | yes | yes |
| boards within (XY) | never | 1.5 | 2.2 | 2.8 |
| boards going slower than (units a second) | | 1.5 | 3 | 4.5 |
| runs infantry over | no | yes | yes | yes |

**Board** (`vehicle::plan::board`). An enemy driving a non-Banshee vehicle
within the skill's range and speed is approached at a boarding marker, with X
held at admission. The bot lets go after 3 s or when it speeds off. Speeds
come from `vehicle::Memory`, not `O+0x88`.

**Drive** (`vehicle::drive`, `Mind::ride`). The driver steers by the camera
towards the objective, else its enemy, else the last sighting, else a roam
goal (§3.2). It plans over walk edges whose portals fit the vehicle, probes
floor and walls, splatters infantry within 15 if the skill allows, and shoots
with the vehicle's guns. It backs up when slow for 1.5 s, and the third time
within 15 s it is stuck for good. A driver with no usable steering for 4 s
gets out. Rules: see the detail note, "Drive".

**Fly** (`vehicle::flight`). The Banshee climbs to a cruise height, makes
strafe runs, breaks off and boosts at close range, rolls when damaged and
requests a fuel rod bomb on an aligned run. Its heights, ranges and cooldowns
are **[guess]**. Left trigger boosts and right trigger fires
(`input::touch_layout::DRIVER`); A for the roll and B for the bomb are
**[guess]** (§5.1). Gunners and passengers shoot as on foot (§3.4).

**Getting out** (`drive::leave`): the vehicle under 0.2, flipped for 1 s,
stuck for good, nowhere to drive for 4 s, nobody driving for 4 s (gunner,
passenger), or within 3 of an objective point it must reach on foot. It holds
X 15 ticks in every 20 and avoids the vehicle for 10 s (30 s when it had
nowhere to drive).

**Seeing it in a game.** §11.4 is the checklist.

**Open** (§12): phantom observations of each seat's entry and exit, the lock
time and zoom requirement, and measured lift and flight behaviour. A Warthog
driver waiting for its gunner remains open.

### 3.10 Firefight

Code: `crates/live/bots/src/firefight.rs`, tests in
`crates/live/bots/src/firefight/tests.rs`. The mode is
[firefight.md](firefight.md).

Clankers play Firefight as players on a player team, never the AI's. With one
player team they fill seats beside people; with two they fill either, keeping
the teams even.

**Seats.** `bots::firefight::set_seat_teams` (1 or 2) sets the player team
count. A new phantom's lobby team (`phantom::session::lobby_team`) is
`firefight::seat_team`; `rebalance` moves a phantom on any other team onto a
player team.

**The match.** The runtime calls `firefight::set_context(Some(Context))` and
clears it with `None`.

| `Context` field | Meaning |
|---|---|
| `player_teams` | the player teams' engine team indices |
| `ai_team` | the actors' engine team |
| `bases` | each team's home base point (`ff_base_00`, `ff_base_01`) |
| `lives` | each team's lives left, `None` for unlimited |
| `phase` | `Fighting` or `Between` |
| `drop_zones` | where waves arrive |

**Enemies.** `firefight::read_actors` adds living actors to the perception as
`Seen` with id `ACTOR_ID_BASE` (`0x100`) plus the datum index.
`firefight::normalise` makes the game a team game, and the mind treats actors
as enemies (`firefight::hostile`), so they use the same sight, target, aim
and grenade path as players. Details: see the detail note, "3.10 Firefight".

**Behaviour.** `firefight::State::plan` gives the goal: hold home within
`REGROUP_RADIUS` (3) when the team's lives are at or under its living
players, within `FALLBACK_RADIUS` (2.5) when shield plus health is under the
name's fall-back line, no goal between waves near home, else hold home within
the name's hold radius (`HOLD_RADIUS` 8, scaled by `bots::name_profile`).
`ERRAND` (2), `DROP_NEAR` (8) and `COMPANY` (5) govern trips to drop zones. A
hold goal leashes a fight to the radius (§3.3). `State::blocks` leaves
weapons outside the free area. A bot on no player team still fights actors.

**Not verified.** Actor-pool positions against a client's view, the sight
filter's chest height for every AI class, and the radii in a played match.

## 4. Way in 1: local slots

This section records the retired local-slot design and its retail
controller, render and audio traces. Current bots use §5. Traces: see the
[detail note](notes/2026-11-26-bots-detail.md), "4. Way in 1: local slots".

### 4.1 The bot's pad (traced, Repo)

A bot's controller was a pad from a bots `HostBackend` on a free port, added
last in `input::InputManager::sync`.

### 4.2 Signing a bot in (traced)

The driver signed a bot in itself with the game's `0x18fb34(port, block, -1)`
guest call (no saved profile), as an Xbox Live guest or a leased bot account,
and joined in the pregame lobby.

### 4.3 Hiding the views (traced)

The views, listeners, speaker, rumble, voice and players-screen panes were
kept off the bots' ports.

### 4.4 What a local slot cannot do

A local slot is capped at three bots by the controller ports and, on Live, is
a guest unless it has a roster account (§9.5).

## 5. Way in 2: phantom peers

### 5.1 What has to be made up (traced)

A phantom is a remote machine with one user, made in memory on the host by
driving the game's own membership code. Nothing goes on the wire: no fake
XNet peer and no fake join messages.

**The game builds machines and players from the session membership**, not
from the update record. Every console builds its own record from its own
copy of the session, and clients learn of a phantom through the membership
the host replicates. So a phantom is a session peer and a session player,
and the game does the rest.

**Session S** (`*SQUAD_SESSION`; the membership block is `S+0x4c`,
`0x5a680`). The fields a phantom touches:

| Offset | Field |
|---|---|
| `+0x40` | host peer; `+0x72d8` the local peer |
| `+0x4c` | membership update number; `+0x7618` change counter |
| `+0x54`, `+0x58` | peer count; peers, 16 x `0x10c` (`0x5fbd0`) |
| `+0x1118`, `+0x111c`, `+0x1120` | player count, mask, players 16 x `0x13c` (`0x600f0`) |
| `+0x4dac` | protocol: 0 local, 1 synchronous, 2 distributed |
| `+0x72dc` | peer channels, 16 x `0x14` (`0x5f900`); `+1` connected |
| `+0x741c` | state: 5 in the lobby and in a running game |

A peer with channel `+1` = 0 is never sent to, never dropped and gets no
simulation view: that is a phantom's silence. The peer's MAC at `+0xa` is
the machine id.

**From the session to players.** Each frame (`0x68140` -> `0x83220` ->
`0x83f00`) the machine set is rebuilt densely from the peers, so a peer is
required. The record build (`0x69260`) makes a kind-3 event for a new
session player; its apply (`0x85140`) creates the player (`0x14bc00`) and
its simulation slot (`0x694c0`; mode 4, remote).

**Actions.** `0x6a3b0` finds the phantom's slot and `0x847d0` stores a
`0x5c`-byte action record in it, validated by `0x14d0a0`. On the server
`0x84990` reuses a stored action for up to 2 s, so a missed tick is
harmless.

**The session state.** A running game stays in state 5; states 6 to 8 make
`0x83220` free the simulation views. "In a game" is state 5 with a game
loaded.

Traces: see the [detail note](notes/2026-11-26-bots-detail.md), "5.1 What has
to be made up (traced)".

**Hosting the game.** The game's mode and the simulation world's mode are
numbered differently.

| | Local | Sync client | Sync server | Dist. client | Dist. server |
|---|---|---|---|---|---|
| Game mode (`[0x4e6948]+0xc`) | 1 | 2 | 3 | 4 | 5 |
| World mode (`[0x4cf77c]+8`) | 1 | 3 | 2 | 5 | 4 |

The game start `0x6ec80` picks the game mode; the new-map callback `0x67ee0`
maps it to the world mode through `0x68c00`. `host_session` takes world mode
4 (`WORLD_DIST_SERVER`), and a waiting phantom names the world mode in its
log line. The start hook (`bots::phantom::session::game_start`) makes this
console the host peer when it is not, with phantoms planned, protocol 2,
state 5 to 8 and no other real console; no run has needed it.

**Recipe on the host** (`bots::phantom::session`): the writes the game's own
functions make, from the game thread before each `RENDER_FRAME`, only in a
multiplayer game whose world mode is 4 and whose session is state 5.

1. **Peer** (as `0x5fbd0`, `0x5f900`): count + 1, MAC the machine id, name
   at `+0x28`, user slots -1, channel entry valid 1 and connected 0; both
   counters + 1.
2. **Player** (as `0x60000`, `0x600f0`): lowest free slot, refused when the
   id is in the session or the player cap is reached; controller 0, never -1.
3. **Configuration**: fresh, with the phantom's name and look, its team and
   the host's level (`bot_config`).
4. The game makes machine, player and slot over the next frames.
5. **Actions.** The simulation gather (`0x69137` -> `0x84990`) samples the
   phantom's pad once per simulation tick and stamps the record with the
   tick. The live player needs the same machine id, a nonzero salt, a clear
   leaving flag and a unit datum (`engine::view::players::Player`).
6. **Removal** (as `0x5fe20`, without its tail call `0x5a2e0`): the peer's
   players out, later indices down one, last peer cleared, counters + 1.
   Phantoms leave last first.

**A phantom's player configuration.** A phantom's requested configuration
(`0x90` bytes) is built from a fresh start (`bot_config`), so nothing of the
host's profile reaches a bot. Only the level stays the host's (the
matchmaking start `0x66050` checks it), and the team is the host's until the
lobby balances. The look comes from `guest::bot_look`.

| Bytes | Field | A phantom's |
|---|---|---|
| `+0x00..+0x40` | name | gear mark and gamertag |
| `+0x40..+0x48` | look | `guest::bot_look` |
| `+0x7c` | team | its own, else the host's |
| `+0x7d` | handicap | 0 |
| `+0x81` | level | the host's |
| others | profile and host-side fields | 0 or -1 |

- **The handicap.** It scales a player's shield (`0x1588b0`) and the damage
  the player deals (`0x15dec0`). A host's handicap once left every Clanker
  with it. Each frame a seated phantom whose handicap is not 0 has it put
  back (`write_no_handicap`, counted as `handicap_writes` in `bots.stats`).
- **What stays the host's.** The level and the peer properties
  (`sync_props`).

Tests: `phantom::session::tests`
(`a_hosts_handicap_and_profile_never_reach_a_bots_configuration`,
`lobby::a_hosts_handicap_is_never_a_phantoms_then_or_later`,
`lobby::a_seated_phantoms_handicap_goes_back_to_none`).

**Phantoms in the pregame lobby.** Phantoms join the lobby as a real peer
does, by the same writes as in a game (`Phantoms::tick`, `host_lobby`).

- **Join.** A real peer's join runs the host's `0x5c720`; the phantom path
  writes the same records directly, counting reservations as `0x62e30` does.
- **Properties.** `sync_props` copies this console's `0xc8` peer properties
  with the phantom's gamertag in both names, every frame.
- **Effective configurations.** The host recomputes them (`0x65cb0`); the
  phantom's requested team is what the lobby shows, and both are written.
- **Start checks.** The countdown (`0x6e410`) and matchmaking (`0x66050`)
  read each peer's map status, hopper status, level and party size; a peer
  with the host's properties passes.
- **Idle lobby.** A settled roster is not reconciled until the occupied
  tables, incarnation, local peer or team flag change.
- **One bump per frame.** All repairs of a frame move the membership
  counters once (`Repairs`); `bots.stats` counts each kind
  ([agent-api.md](../runtime/agent-api.md#bot-combat)). Thirty writing
  frames in a row are logged once.
- **Teams.** A phantom with no team joins the smallest team (`lobby_team`);
  one on a team with two more players than the smallest moves
  (`rebalance`). A difference of one moves no one.
- **Staying.** A phantom stays from lobby into game and back. It leaves when
  the session is no longer one this console hosts live (`owned_session`), and
  rejoins when the game took its peer out. Lowering the count removes it
  through `remove_peer`.
- **The switch.** `H2_PHANTOM_LOBBY=0` keeps phantoms out of the lobby.
  On by default.

**Phantoms in the game.** Two hooks make a phantom play.

- **Spawning.** `MACHINE_IN_GAME` (`0x686e0`) is hooked in `boot::bot_glue`:
  for a phantom with a peer in the session (`Phantoms::stands_for`) it
  answers 1, so the spawn pass makes a unit.
- **Loading.** A server world waits for every machine in its mask to have a
  view (`0x69c80`). `session::unwait_phantoms` clears the phantoms' bits
  from the wait mask once per start; real consoles are still waited for.
- **Actions.** A spawned phantom's unit takes the action in its slot (step 5).

**Phantom audio.** `crates/extensions/game/src/bot_glue/audio.rs` excludes
Phantom entries from the local-user map while `SOUND_OWNER_USER`,
`SOUND_LISTENERS_UPDATE` and `SOUND_UPDATE` run, and restores it on return.
Human slots keep their user indices. The
[agent's active listener list](../runtime/agent-api.md#audio-voice-routing)
shows the slots and camera positions.

**The user's Halo 1 run.** Peer reuse after a session not live, the
`movaps` crash in a converted map's animation data and the `0x80155100`
roster refusal are in the note.

**The action from a pad.** The phantom uses the Default layout that local
Clanker profiles use ([controller layout trace](../input/controller-layouts.md)).
The builders are `0x185be0`, `0x186ab0` and `0x84990`; the record is written
at `0x187029..0x187083` and consumed at `0x14d6d4`.

| Default pad input | Record field |
|---|---|
| A, jump (k0) | flags `+0` mask `0x2` |
| Left stick click, crouch (k10) | flags `0x1` |
| B, melee (k4) | flags `0x30` |
| White, flashlight (k5) | flags `0x02100004` |
| RT, fire (k7) | flags `0x00010040`, pressure `+0x14` |
| LT, grenade (k6) | pressure `+0x18`, flags `0x00224800` |
| X, reload or use (k2) | word `+0x1c` bits `0x01`, `0x02` |
| Y, switch weapon (k3) | word `+0x1c` bits `0x10`, `0x20` |
| Black, swap grenades (k1) | signed word `+0x22` |
| Right stick click, zoom (k11) | signed word `+0x24` |
| D-pad (k12, k13) | flags `0x80`, `0x100` |

**Tap and hold timing.** The pad conversion keeps pressure history, and X and
Y send levels each tick; the host derives press and release bits
(`0x14f722..0x14f81a`). The hold threshold is read from the globals'
player-control block and measured in simulation time. Exact agreement with
an asynchronously polled controller is **[guess]**.

**Selectors, target and motion.** The builder packs weapon indices at
`+0x20/+0x21`, the target at `+0x28`, and the shot-assist block at `+0x34`
(nine dwords, copied at `0x187074`); the phantom supplies the assist block
like a local player. The seat runs the retail builder `0x186ab0` against a
copy-on-write snapshot of the host world with the phantom as local user, and
only the validated action crosses back. It samples once per game tick and
advances the button clock by simulation milliseconds.

**The facing release.** After a melee lunge the biped's flag bit `0x20` at
`+0x348` makes the builder reseed the look from the unit's aim and drop the
right stick, which froze a bot's aim. After a built action, `slot_action` in
`crates/live/bots/src/phantom/action.rs` clears the bit and counts it in
`bots.stats` `maintenance.facing_releases`. Test:
`a_built_action_releases_the_bipeds_facing_flag`.

The private builder skips animation prefetch, retries a cold read once on
the game thread, and on a further miss revalidates the previous action for
one tick or returns zero. Image-backed test:
`retail_pad_sequence_matches_local_angles_target_and_fire` (`H2_DISC`,
`H2_RETAIL_MODULE`, `H2_PAD_CHECKPOINT` from `state.save`).

**Open, for a run**:

- **Clients and the phantom's address.** A real client opens a channel to
  every peer's XNADDR and its drop loop (`0x5a090` -> `0x5fda0`) runs in
  state 5, so a phantom may be dropped on a client. The host is safe.
- The listener veto at `[S+0x78a8]` vtable `+0xc` is not read; phantoms
  never go through it.
- The profile appearance starts at configuration `+0x40`; phantoms set
  colours and emblem from their full tag.

**Phantom ping.** The host gathers phantom controller reports on the tick
that requests them. A sim can set `phantom_ping_ms` and `phantom_jitter_ms`
(default 4 ms) to model a remote controller: one-way delay is half the ping
plus jitter, rounded up to ticks, and trigger changes arrive in order. Zero
delivers at once. The host always builds the retail action at gather.

Midship replay figures: see the [detail note](notes/2026-11-26-bots-detail.md),
"Midship replay".

**Training coverage.** A sim match hosts a private System Link game, and each
phantom's brain, uplink, remote gather and action builder run as in a hosted
custom game. [Headless simulation](../runtime/sim.md#the-playerless-system-link-host)

### 5.2 Why phantoms are needed

A phantom joins as its own machine and player. It can carry a bot account
with a gamertag, emblem and rank, with all host controller ports available
for people. A playerless sim host still occupies one of the 16 machine slots,
leaving 15 slots for single-player phantoms.

### 5.3 CPU per bot

Per bot, per tick: perception rays, combat decisions and controller input,
path following (one A\* per re-plan) and aim. The estimate is under 0.1 ms
per bot per tick. Detail: see the
[detail note](notes/2026-11-26-bots-detail.md), "5.3 CPU per bot".

### 5.4 Waiting for the black screen

Phantoms send a neutral pad while the opening screen fades in and each time
they respawn. Their brains still observe and plan on every gathered tick.
The gather drops queued input while the fade is active and publishes the
current pad on the first clear tick.

At match opening the host reads each mapped human's black overlay opacity
(`[GAME_ENGINE] + 0xe4 + 4 * user`). A phantom models the fade from its own
spawn age (player record `+0x194`). A playerless host holds the opening gate
for `round(1.5 * tick_rate)` ticks (`0x15c4c6`). Addresses and arithmetic:
see the [detail note](notes/2026-11-26-bots-detail.md), "5.4 Waiting for the
black screen".

### 5.5 Brains off the game thread

`bots::think`. The simulation's gather (`0x84990`) asks for one phantom's
pad at a time, on the game thread. Three modes, set with
`Phantoms::set_think_mode`, or from the environment for the process's
phantoms:

| `H2_BOT_THREADS` | `H2_BOT_PIPELINE` | Mode |
|---|---|---|
| unset, `0`, `off` | unset, off | **Serial** (the default): each phantom sampled whole at its own slot. |
| `N` or `auto` | unset, off | **Batched**: every phantom at the tick's first gather, the thinks across `N` workers. |
| `N`, `auto` or unset | `1`, `on` | **Pipelined**: batched, and the thinks overlap the guest's tick. |

`auto` takes one worker fewer than the cores, at most 8. The game thread
also runs queued thinks while it waits for them.

Serial is the default. `h2 sim run` sets the variables only from the match
file, so a match with no `bot_threads` thinks serially; see
[sim.md](../runtime/sim.md#bot-thinking-across-cores).

A batched sample has three phases (`PadSource::observe`, `think`,
`conclude`): read on the game thread in slot order, think on the workers
against the brain's own data, conclude on the game thread (radio calls).
Radio order is kept with `radio::Seat` and `Radio::publish_seated`, so
batched and serial give the same pads, minds and radio at any thread count.

**Pipelined.** The thinks staged at tick N run while the guest simulates N,
and their pads are tick N+1's. `Uplink::think_ticks` credits that tick
against a simulated delay. Stale thinks (input reset, tick gap) are dropped
and the seat sends a neutral pad.

The batch covers phantoms the gather would sample this tick, and the
phantom outside it is sampled whole at its slot. Phases, batch membership,
pipelining and teardown: see the
[detail note](notes/2026-11-26-bots-detail.md), "5.5 Brains off the game
thread".

The boot sources (`clankers::Clanker`, `sim_glue::Metered`) forward the
three phases. `Metered` counts the sim tally at the conclusion, on the game
thread.

Tests: `bots::think::tests` (serial against batched, pipelined timing, drops,
the pool) and `phantom::action::tests` (the fade and latency with the
pipeline).

## 6. Phantom players in the game record

Bots run on the host. Clients see them as remote players, with their state
distributed by the host. Recomp clients recognise the gear mark in the
player configuration's name (§8.2). The bots leave when the host ends the
game.

**Connection bars.** `crates/extensions/game/src/lobby_quality.rs` replaces
the badge's bar count (`0x1634d0`): six bars for the local console and
phantoms, otherwise the last acknowledged round trip from the reliable
packet queue, scaled from six bars at 60 ms to one at 800 ms. The agent
reads the same ratings through
[`ui.lobby.players`](../runtime/agent-api.md#ui-what-the-menus-show). Detail:
see the [detail note](notes/2026-11-26-bots-detail.md), "6. Phantom players
in the game record".

**Scoreboard and stats.** Bots appear in the scoreboard, the carnage report
and post-game stats. Telemetry records them by game slot with `bot: true`
(`live_telemetry::Bots`, from `clankers::Driver::bots_in_force`). A phantom
on the active server roster carries its rostered XUID (§9). The web shows a
BOT mark beside a bot's name.

**Bots are game-only.** The game record keeps every bot with its kills,
deaths and score. Career stats do not count kills of or deaths to bots (nor
sprees, weapons or medals); games, wins and time played still count. This
holds in `live_server::stats` and the web's `derive.ts` (`careerKills`,
`serviceRecord`). A bot account's own page (§9.5) counts all its knockouts.
Ranks are unchanged. Detail: see the
[detail note](notes/2026-11-26-bots-detail.md), "6. Phantom players in the
game record".

## 7. Clankers in custom games (the Game Setup page)

Game Setup gets a **Clankers** row next to the Halo 1 Options row
(`crate::h1_options_row` is the model). The row opens a **Clankers** page.
Live custom game advertisements report whether the host plans Clanker seats,
so the browser's **Clankers** filter can find them
([Browser controls](../live/live-custom-games.md#filter-games)).

The page is a native ui_kit page (`ListPage` or `Page` impl,
`crates/extensions/game/src/ui_kit/page.rs`). Its rows:

| Row | Values | Setting |
|---|---|---|
| Clankers | Off, On | `clankers.enabled` |
| Count | 1 .. 15 (capped at the variant's max players minus humans) | `clankers.count` |
| Difficulty | Easy, Normal, Heroic, Legendary, Mixed | `clankers.difficulty` |
| Teams | Auto-balance, Red, Blue, … (team games), Free (FFA) | `clankers.team` |
| Aggression | Calm, Normal, Wild | `clankers.aggression` |
| Aim style | Steady, Twitchy | `clankers.aim_style` |
| Taunts | On, Off | `clankers.taunts` |
| Glitches | Off, Some, Lots | `clankers.glitches` (§3.8) |

A host can change Aggression and Aim style on an existing roster; the next
input sample picks up the edit. Calm and Wild shift the bot's drive to press
by 0.35, Twitchy raises stick engagement and flick overshoot. **Mixed**
draws each bot's difficulty around Normal. The settings live in the settings
store through `setting_defs` and appear in `h2 settings`. Effects and kit
notes: see the [detail note](notes/2026-11-26-bots-detail.md), "7. Clankers
in custom games (the Game Setup page)".

**Joining the lobby.** When the host's System Link or Xbox Live custom game
lobby opens with `clankers.enabled`, the driver adds phantoms as peers
(§5.1), capped by the available session slots. Changing the page in the
lobby adds or removes phantoms.

**Leaving the lobby.** When the party leader is the only human, Back in the
custom-game lobby follows the solo leave path and skips the party leader
picker. `game::bot_glue::party_leave` scopes the squad peer count
(`0x199f34`) to the lobby Back and pause Leave Game checks, and the confirmed
leave paths (`0x199e3c`, `0x252150`) remove phantoms through
`bots::phantom::Phantoms::retire_solo_session`. Check with `h2 agent ui` for
`dialog.name = party_leader_picker`. Hook addresses: see the
[detail note](notes/2026-11-26-bots-detail.md), "7. Clankers in custom games
(the Game Setup page)".

## 8. Clankermatch

### 8.1 What the player sees

**Clankermatch** is an Xbox Live menu row, after OptiMatch, that plays like
OptiMatch:

1. Pick a playlist.
2. The search runs, with the retail screen, for 4 to 20 s drawn from the
   playlist's population.
3. The lobby fills with bots one by one, each with a gamertag, emblem and
   rank. Teams are balanced.
4. The match runs as a normal Live matchmade game, with the same hopper
   playlists, variants and carnage report.
5. Leaving ends the game.

The player's own console hosts. The bots join as phantom peers up to the
playlist's player limit.

### 8.2 Decided by the user

1. **Stats.** A Clankermatch game is its own kind: `GameRecorded` carries
   `kind: "clankermatch"`. It counts in career totals. A game in a clanker
   playlist whose server mark says `ranked` also moves that playlist's own
   rank and leaderboard; unranked training games move none.
2. **Bot identities.** Bots get made-up gamertags marked by **U+2699 GEAR**
   (⚙), drawn from our own glyph added with `font::patch`
   (`crates/formats/font/src/patch.rs`). The gamertag stays inside the Live
   rules (`live_proto::Gamertag`: ASCII letters, digits, single spaces, 1 to
   15 characters); the glyph is not part of the account name. In the game the
   host writes `⚙ ` plus the tag into the phantom's player configuration
   (`bots::phantom::session::marked_name`, `CONFIG_NAME_UNITS` 16); on the
   site it is drawn from the server's bot flag. `font::gear` draws the gear,
   and `boot::bot_mark::GearRewrite` adds it to every file under `maps\fonts`
   after `boot::icons`. A font it cannot patch keeps a box and says so with
   `H2_BOOT_VERBOSE=1`. The game record names a bot without the mark
   (`live_proto::bots::unmarked_units`). Name budget and glyph geometry: see
   the [detail note](notes/2026-11-26-bots-detail.md), "8.2 Decided by the
   user".
3. **Bot rank.** Bots are matched to the player's rank. The level maps to the
   aim table's difficulty (§3.4) on a continuous scale: reaction, error and
   lead are interpolated.

### 8.3 Menu row and playlists (traced, Repo; built)

**The row.** The Live menu's extra rows come from `custom_games.rs`'s hooks
(list count, add items, pane, row text `0x230f36`, row chosen `0x230f92`).
Clankermatch is row 2, right after OptiMatch:

| Row | What | Press |
|---|---|---|
| 0 | Quickmatch | retail |
| 1 | OptiMatch | retail |
| 2 | **Clankermatch** (ours, pane 4) | retail row 1's press, `clankermatch::active` set |
| 3 | Co-op Campaign (ours, pane 6; with Live), directly above Custom Games | co-op |
| 4 | Custom Games (ours, pane 7) | retail row 2's press, CUSTOM GAMES asked for |
| 5 | Forge (ours, pane 8; with `forge`) | retail row 2's press, FORGE asked for |
| 6 | Firefight (ours, pane 9; with `firefight`) | retail row 2's press, FIREFIGHT asked for |
| 7 | Custom Game Browser (ours, pane 5) | the Live browser |
| 8 | Mods (retail's Content Download, row 3, moved and renamed; pane 3) | Mods screen or the Map Pack dialog |

A row that is off leaves the list and the rows under it close up
(`custom_games::Menu::layout`; `H2_NO_COOP_ROW=1` drops the co-op row). The
lobby switches to the mode a row asks for at its first panel update
([party-lobby.md](party-lobby.md), "The modes"). `h2 bots test` drives
Custom Games at row 4. Every Live menu press sets `clankermatch::active`
(Clankermatch) or clears it. Cursor restore for rows 4 to 7 is a `before`
on `0x00231170` (`engine::points::LIVE_MENU_CURSOR`,
`custom_games::LiveMenuCursor`, feature `live-menu-cursor`). Press and cursor
detail: see the [detail note](notes/2026-11-26-bots-detail.md), "8.3 Menu row
and playlists (traced, Repo; built)".

**The playlists.** OptiMatch's press (`0x231025`) opens the playlist screen
(`0x149f1e`, loader `0x00230c8d`, item builder `0x002b1ff4`). Each row keeps
its hopper index `i` at `+4`, so skipping a hopper leaves no hole.

**The hook** is a `before` on `0x00192db0` (`engine::points::HOPPER_LISTED`):
`clankermatch::listed(i, marks, active)` lists a hopper only when its kind
matches the row that opened the screen. The marks come from the server
(`GET /v1/hoppers/clankers`, §9.4 item 1), asked for at each logon. With no
marks OptiMatch is retail and Clankermatch's list is empty.

**Quickmatch (built).** Two hooks keep Quickmatch (`0x00231045`) on human
hoppers (`clankermatch::PlaylistFilter`): a `before` on `0x001902fc`
(`engine::points::PLAYLIST_REMEMBER`) saves nothing during Clankermatch or
for a clanker hopper, and an `after` on `0x001902de`
(`engine::points::QUICKMATCH_HOPPER`) answers -1 for a saved clanker hopper.

**Switch To: Clankermatch (built).** `clankermatch::SwitchRow` (feature
`clankermatch-switch`) adds a **"Switch To: Clankermatch"** row (action 12,
`clankermatch::SWITCH_ACTION`) under the lobby's "Switch To: Matchmaking"
while the server has clanker hoppers. Its press sets `clankermatch::active`
and calls `0x002bba8c`, so the party goes to the clanker playlists (§8.4).
Retail's Switch To: Matchmaking clears `active` first
(`clankermatch::switch_press`). In the party lobby `lobby_mode::arrange`
drops every Switch To row, and Change Playlist lists both kinds
(`clankermatch::Scope::All`;
[party-lobby.md](party-lobby.md#the-modes)). Traces, log lines and hook
detail: see the [detail note](notes/2026-11-26-bots-detail.md), "8.3 Menu row
and playlists (traced, Repo; built)".

### 8.4 Search and lobby (built: the empty search)

The game searches through `XOnlineMatchSearch`, which is ours in live-hle. For
a clanker hopper (`live_hle::clankermatch`):

1. The search returns no sessions after a uniform 4 to 20 s delay, drawn once
   per search run (live-matchmaking.md §7.3). The game then hosts, as OptiMatch
   does when a search finds nothing.
2. The host's Clankers driver sees a clanker hopper's lobby
   (`Hle::clanker_hosted`) and fills it (`clankers::Driver::tick_match`, twice
   a second). It asks Live for one roster (`POST /v1/games/bots`, playlist from
   `Rosters::playlist`, no skill, `teams: true` in a team playlist; the server
   sets skill from the party's level, §8.2.3). It seats the bots one at a time
   on `clankermatch::schedule` (0.5 to 3 s apart), up to the playlist's most
   players less the party. The party size is the squad session's player count
   (`bots::lobby::party_players`, `+0x1118`), never a phantom. Each bot joins
   the target session as a phantom peer with its rostered account and skill
   (§5.1, "Phantoms in the pregame lobby"). If Live refuses the roster, the
   host supplies the identities. In a team playlist `clankermatch::balance`
   puts each phantom on the team with the fewest players, then the lowest
   total skill, and the party stays on the host's team
   (`bots::phantom::session::host_team`).
3. The match starts on the hopper's normal countdown once the phantoms reach
   the required player count. A lobby that has not started 20 s after its last
   bot was due logs why (`Driver::stalled`, `bots::lobby::match_wait`). A
   handoff keeps the bots and carries the roster (§9.5). With no hosted session
   for 30 s the fill ends and the phantoms leave. The 30 s count only while
   this console does not serve a multiplayer game
   (`bots::phantom::session::serves_game`, `clankers::Driver::set_hosts_game`).
4. The game record says `kind: "clankermatch"` with the playlist and session
   (`telemetry::Playlist::clankers`); the server keeps it as its own kind
   (live-matchmaking.md §6.8).

The lobby's "searching for players" state is the game's own: it stays until the
session is full. The phantoms join the target session before the countdown,
with the Clankers page on or off.

Traces: see the [detail note](notes/2026-11-26-bots-detail.md), "8.4 Search and
lobby".

**Tracing the matchmaking lobby.** The matchmaking lobby is a pregame lobby:
the playlist press creates the squad session (`0x002b2149`, `0x002b21b9`), and
a phantom's peer copies the host's hopper and map status so the start check
`0x66050` and the countdown end `0x6e410` pass. Details: see the
[detail note](notes/2026-11-26-bots-detail.md), "8.4 Tracing the matchmaking
lobby".

**The matchmaking host's lobby.** The game keeps a squad session
(`[0x527364]`) and a target session (`[0x52736c]`); `0x65340` picks the game
session by life-cycle type (6 to 9 the target). `bots::lobby::game_session`
mirrors it, and `lobby::matchmaking_host` is a matchmaking lobby this console
hosts. The phantoms' `host_lobby`, `live_session`, `owned_session` and
`host_session` read the game session, so they join the target session and stay
for the matchmade game (type 8).

| Item | Where |
| --- | --- |
| Re-search after the hopper timeout (`+0x59c`) | the game's; the fill follows it |
| Host update, start wait | `0x71730`, `0x1931d0` |
| Arbitration round, register, poll | `0x73ae0`, `0x73bc0`, `0x755d0` |
| Failed registration, result `0x11` | `0x72260`, `0x706f0` (back to the party) |
| Matchmaking trace | `live.matchmaking.trace`, [Agent API](../runtime/agent-api.md#matchmaking-transitions) |
| Countdown text "Setting up bots…" | `0x0f00188e`, `lobby_status::MatchCountdown` |

The Live HLE logs each refused arbitration call, and the game logs the
matchmaking result. The match's bots are told only to the roster's session
(§9.5). Traces: see the [detail note](notes/2026-11-26-bots-detail.md), "8.4
The matchmaking host's lobby".

**Phantoms only.** Clankermatch sets `Phantoms::set_match_only(true)` so its
phantoms join life-cycle types 6 to 9 and wait while the party's lobby is up.
The flag clears when the fill ends; no controller-backed bot is seated.

**Grep the run's log.** Nine log steps, from the clanker hopper marks at logon
to the stall line, trace a Clankermatch run: see the
[detail note](notes/2026-11-26-bots-detail.md), "8.4 Grep the run's log".

**Run:** whether the game hosts after an empty search in every playlist type,
or needs the server's matcher to say "you host". **Design:** use
`MatchProposed` with `host = me` and the bots as members when the matcher
exists (the sbmm-ranked lane). The empty search is what is built.

**Tests (no game):** `cargo test -p boot --lib custom_games`, `-p boot --lib
coop_live`, `-p boot --lib clankermatch`, `-p game --lib
clankers::tests::a_clankermatch`, `-p boot --lib bot_mark`, `-p font gear`,
`-p bots phantom::session`, `-p bots --lib lobby`, `-p live-hle --lib
players_tests`, `-p live-hle --lib clankermatch`, `-p live-hle --test server
a_clanker_doubles_lobby`, `-p live-proto bots hopper::clankers`,
`-p live-telemetry games_say_their_kind a_bots_name_reads_without_its_mark`,
`-p cli bots_drive coop_drive`, `-p live-server --test clankermatch` and
`-p live-worker a_clankermatch_game`. What each covers: see the
[detail note](notes/2026-11-26-bots-detail.md), "8.4 Tests".

### 8.5 Game run (checklist)

`h2 play --live-local` (or a Live server with the new default hoppers), signed
in on port 0, Clankers page off. `h2 bots test --clankermatch` (§13.1) covers
the lobby and match transitions. The checklist (the Xbox Live menu rows,
Clankermatch's playlists, the lobby fill, team balance, the in-game actions,
leaving, and a server with no clanker marks) is in the
[detail note](notes/2026-11-26-bots-detail.md), "8.5 Game run (checklist)".

## 9. The server interface (for sbmm-ranked)

The sbmm-ranked lane owns the server side: the matcher with skill, ranked
arbitration, per-playlist rank, and the account side of **persistent bots**.
The host runs the bots. This is the one interface between them.

### 9.1 The roster: server to host

The server tells a host which bots to run with a **bot roster**. In
Clankermatch it comes in `MatchProposed` on `match-<id>`, as `bots`. In a
custom game the host asks `POST /v1/games/bots` with `{session_id, playlist?,
count, skill?, teams}`. Without a server (System Link, offline) the host makes
its own from the Clankers page.

Each roster entry:

| Field | Meaning |
| --- | --- |
| `xuid` | issued by the server, in a bot range of its own |
| `gamertag` | passes `live_proto::Gamertag` |
| `persistent` | a persistent (ranked) bot, or made up for this match |
| `emblem`, `colors` | foreground, background, colours; primary, secondary |
| `rank` | `{level, playlist}`, what the lobby shows |
| `team` | team index, or null for the host to auto-balance |
| `skill` | 0..1, continuous; maps onto §3.4 (0 Easy to 1 Legendary) |
| `personality` | `seed`, `aggression`, `caution`, `objective`, `teamwork`, `taunt` (0..1; `taunt` 0 is never, §3.5) |

Rules:

- `skill` is the only difficulty input. The four named difficulties are 0.0,
  0.33, 0.67 and 1.0. For Clankermatch the server sets it from the human's
  level in that playlist.
- `team: null` means the host balances by total skill.
- A made-up bot still gets an `xuid`, so the carnage report and stats can name
  it. The server owns name, emblem and rank for every bot.
- The host never changes identity, rank or skill; it may only lower `count` if
  the session is smaller.

### 9.2 Results: host to server

These go in the normal game record (`games.rs`), with `kind: "clankermatch"`,
`"custom"` or `"matchmade"` on the game, and `bot: true` on each bot's player
entry, keyed by its `xuid` when it has one. A rostered phantom carries its
account's XUID (§9.5); a host-made phantom has `bot: true` and no `xuid` (§6).
Per-player stats are the same as a human's, and the server's normal
per-playlist rank update ranks persistent bots. Whether arbitration counts a
bot's report is sbmm-ranked's call.

### 9.3 Trust

The roster is signed by the server like `MatchProposed`. A game record with
`bot: true` entries is accepted only for xuids the server marked as bots in
that match's roster or in the `/games/bots` answer, so a console cannot claim
a human as a bot or the reverse. A `bot: true` entry with no `xuid` is
accepted with or without a session: it names no account and counts toward no
one's career (§6).

### 9.4 Interface with ranks (answers to live-matchmaking.md §8.4)

This lane owns the bot accounts and their events (`POST /games/bots`, the
roster, `bot: true` on a game's entries, the `0x000E` range); sbmm-ranked owns
how bots are ranked and arbitration.

1. **Marking a clanker hopper.** The mark is in the server's hopper source, not
   the game's `0x614`-byte descriptor: `clankers: Option<Clankers>` on the
   playlist, with `Clankers { ranked: bool, ranks_as: Option<u32> }`. `ranked`
   says whether finished games move a standing; `ranks_as` is the playlist id
   of the human hopper whose standing it shares. The server serves the mark
   with the hopper list, and live-hle reads it for the menu filter (§8.3) and
   the search (§8.4). The open question of duplicate playlist ids is in the
   [detail note](notes/2026-11-26-bots-detail.md), "9.4 Disagreement, small".
2. **The read-model lookup.** `live_server::bots::Bots::bot` answers whether
   the server issued an xuid as a bot (`BotIssued` on `bot-<xuid>`).
   `rostered(session_id, xuid, before)` answers from `BotsRostered` on
   `bots-<session_id>`: the latest roster before a log position, which
   `rank_game` takes from the game's `GameStarted` position. The roster is
   keyed by the session id (`NewGame.session_id`).
3. **Ranking only reads the acceptance result.** Each `bot: true` entry must be
   in that session's `BotsRostered`, and a rostered xuid cannot be claimed as a
   human; a record that fails is refused whole. Host-made xuids
   (`live_proto::bots::is_local_xuid`) are refused in any record that reaches
   the server.

Also agreed: `rank::bot_skill(level)` as the roster's `skill`, and
`Ranks::nearest(playlist, level, n, bots)` for choosing persistent bots, with
gaps made up at the humans' mean level.

### 9.5 Bot accounts

Every bot the server rosters is a **bot account**, shown like an account with a
BOT mark, for matchmaking and for Live custom games.

**What a bot account is** (`live_server::bots`, both homes). It has an xuid in
the bot range `0x000B…` (humans `0x0009`, host-made `0x000E`), issued once as
`BotIssued` on `bot-<xuid>` with a gamertag from the word lists, emblem,
colours and personality. `GET /v1/accounts/{gamertag}` answers for a bot with
`bot: true` (`AccountSummary::bot`); `GET /v1/accounts` never lists bots. A bot
has no keys, so it cannot sign in or post, and nobody else may write `bot-*` or
`bots-*`. A roster (`POST /v1/games/bots`, `BotsRostered` on `bots-<session>`)
is the lease: the server accepts a game record's `bot: true` entry with an XUID
only from the session host and for a bot on the roster in force at the start
(§9.3).

**Where the bots come from.** The driver asks Live for a roster covering every
requested bot. A matchmade session includes its playlist so the server can
pick persistent bots near the humans' level (§9.4); a custom game requests
made-up bots at the page's difficulty. A change in count, difficulty or session
can fetch a new roster. System Link and rosterless Live sessions use host-made
phantom identities.

**The session's players.** Retail fills no slot counts in its advertisement, so
the host says who plays. Every second (`clankers::frame`) the driver hands the
HLE the joined rostered phantoms (`Driver::session_bots`), and
`Hle::set_session_players` sends `PlayerJoined` and `PlayerLeft` on
`session-<id>` for changes only, to the roster's session alone
(`Driver::roster_in_force`; `sessions::session_players`, `players_diff`;
`bots::leased_to`). Detail: see the
[detail note](notes/2026-11-26-bots-detail.md), "9.5 The session's players".

**One identity across a session handoff.** Halo 2 hands its lobby to a new Live
session as a game starts and ends. The server's `live_server::bots::may_carry`
rosters the old session's bots again for the new one when a roster request
names `carry_from` (window `CARRY_WINDOW_S`, 120 s). The console
(`clankers::ask_for`) keeps the roster in force across the gap, asks with
`carry_from` when count, difficulty and playlist are unchanged, and asks for a
new roster when the server refuses or they change. A phantom's machine id marks
it as a bot (`live_proto::bots::is_phantom_machine`, the `C1 A4` prefix); only a
listed one with its roster in force carries its xuid. Rules and the log lines:
see the [detail note](notes/2026-11-26-bots-detail.md), "9.5 One identity
across a session handoff".

**Web.** A bot account has a player page with a BOT mark, its service record
and game history, and no social actions; scoreboards and carnage reports link a
bot with an xuid to it.

**Careers.** Humans' careers stay as §6 says: a kill or death with a bot on
either side is game-only. A bot's own service record counts all its kills and
deaths. No bot is on a leaderboard.

**Staged (not built):** `MatchProposed` carrying the roster (stage 8, with
sbmm-ranked), bot presence, a sweep of made-up bot accounts, and filtering bots
from web search lists.

**Tests.** `live-server`'s `bots` integration tests, `live-hle`'s
`players_tests`, `live-worker`'s `rostered_bots_read_as_bot_accounts`, `boot`'s
`clankers` tests and the web's `test/bots.test.ts`.

**Game run.** Host a Live custom game with Clankers On and Count 3: the lobby
shows three rostered gamertags, and the game page shows three BOT marks linking
to the bot accounts. `h2 bots test` (§13) checks it too.

## 10. Stages

The checks for live play are in §11 and §13. Clankers use phantom peers
(§5). Controller ports are for local players. The build diary, test lists and
game-run checklists are in the
[detail note](notes/2026-11-26-bots-detail.md), "10. Stages".

| Stage | Exists | Left |
|---|---|---|
| 1 Player slots | phantom registration, `bot_glue.rs` action feed | none |
| 2 Navigation | `bots::nav`, `los`, `cache`, `autonav`, `h2 bots nav` | none |
| 3 Combat | `bots::mind`: seeing, deciding, aim, skills | utility layer grew in §3.3 |
| 4 Emotes | `bots::emote`, dumb-death taunts | stuck, misses, out of ammo |
| 5 Objectives | `bots::objective`, six objective gametypes | none |
| 6 Phantom peers | books, session recipe, Live rosters, telemetry | System Link run checks |
| 7 Clankers page | `boot::clankers`, `clankers` screen, row 11 | none |
| 8 Clankermatch | first slice, follow-ups | party of several humans, search delay, arbitration rank, `MatchProposed`, published-hopper marks |
| 9 Vehicles | §3.9 seat entry, lock-on, Banshee | none |

1. **Player slots.** The game registers a phantom with the session, then the
   host feeds it actions through
   `crates/extensions/game/src/bot_glue.rs`. Sections 5 and 6 describe the
   join and action path.
2. **Navigation.** Built: `bots::nav`, `bots::los`, `bots::cache`,
   `h2 bots nav`, and the map loading in `boot::bot_glue`.
   - **The mesh** (`nav::NavMesh`) comes from `crates/formats/pathfind`'s
     generator over each structure BSP's collision. Edges with no polygon
     beyond them become walk, **drop** or **jump** links (crouch jumps and
     gap jumps included); the builder adds teleporter and lift links and
     checks standing headroom (§3.2).
   - **Paths**: A\* over the polygons, then the funnel over the portals;
     `nav::Follower` steers along the waypoints, jumps at a jump, and
     re-plans after 30 frames without progress.
   - **Walls** (`los::Walls`): collision triangles on a 2 m grid for line of
     sight. A live collision commit (`edit.collision.commit`) rebuilds them
     through `autonav::collision_edited` and `cache::walls_of`
     ([realtime-edits.md](../editor/realtime-edits.md), "Collision").
   - **Visibility grid** (`bots::vis`, `<map>.<bsp>.vis`): cells that never
     see each other, so `Walls::clear` skips the ray (§3.1, "Visibility
     grid"). Versioned by `vis::VERSION`.
   - **The file.** `<data>/bots/<map>.<bsp>.bots`: magic `H2BT`, format
     version 4, the stamp (`cache::GENERATOR` and the source map's
     fingerprint), the mesh (`H2NV`, version 2), walls, breakable glass and
     tactical places. It comes from the disc, so it stays out of git.
   - **Built for you** (`bots::autonav`, `game::bots_nav_auto`). Each boot
     owns a `bots-nav` worker that builds stale multiplayer meshes and grids
     on a map pick, a Clankers switch or a map load. `autonav::map_use`
     gives `Never` for the menu and shared maps, `LoadOnly` for campaign and
     `Build` for multiplayer. `bots::autonav::build_soon(map)` asks early.
     The fingerprint is FNV-1a over the map's 2 KiB cache header; the shared
     cache (`autonav::shared_cache`) keys on SHA-256 of the source map
     ([storage.md](../runtime/storage.md#host-directories)). Writes are
     atomic and BSP 0 is written last. Bump `cache::GENERATOR` when a
     generator change alters output; `generator_output_is_pinned` enforces it.
   - **Off**: `H2_BOTS_NAV_AUTO=0` (or `off`), or `H2_FEATURES=-bots`.
   - **By hand**: `h2 bots nav <map|all> [--force] [--disc ISO] [--data DIR]`
     runs the same worker in the foreground.
   - Tests: `cargo test -p bots`, `cargo test -p game --lib bots_nav_auto`,
     and the ignored `cargo test -p cli bots_nav -- --ignored`.

   **Built: movement boost** (§3.2). Corners and doorways (`bots::nav`), off
   the mesh snapping, fight footwork (`bots::moves`) and roaming
   (`bots::roam`); `mind.rs` holds the hooks `Moves::fight` and
   `Roam::pick`. No mesh change: `cache::GENERATOR` stays 1. Tests:
   `cargo test -p bots --lib nav`, `moves`, `roam`, `mind`.
3. **Combat.** Built: `bots::mind` (`Mind`, the phantom's decision loop). An
   enemy is seen inside 40 degrees of the aim, or within 3 units, within the
   skill's visual range (§3.1) and with a clear line. Each frame the bot
   fights, chases the last place it saw someone (forgotten after 10 s) or
   roams (§3.3). Aim uses `mind::Skill` (Easy, Normal, Heroic, Legendary);
   reaction, error, lead, stick gain, fire cone, automatic bursts and
   grenade rates are in the note. Tests: `cargo test -p bots`.
4. **Emotes.** Built: `bots::emote`, in every brain (`Mind::emote`). Deaths
   come from `PLAYER_KILLED` (`0x15ce70`), observed in `boot::bot_glue`
   (`emote::kills()`). `emote::classify` marks self or world deaths
   (severity 0.8) and betrayals (1.0) as dumb. Each rolls `taunt × severity`;
   the bot emotes with 3 to 6 left-stick clicks when the body is within 8 m,
   no enemy is in view for 1.5 s, no objective is asked and 30 s have
   passed. Taunt is 0.3 (§9.1); Clankers page Taunts off makes it 0. Tests:
   `cargo test -p bots emote`.
5. **Objectives.** CTF, Oddball, King of the Hill, Assault, Territories and
   Juggernaut (§3.7). `bots::objective` has `read`, `role_of` and `plan`;
   `Mind::step` (§3.3) puts a carrier first and the objective in place of
   chasing and roaming. Tests: `objective::tests`, `mind::tests`,
   `guest::tests::objective_reads`. Game run: §11.2.
6. **Phantom peers.** Built through 6c; the checks need two consoles.
   - **6a**: `bots::phantom::Phantoms` (the books: `add`, `refresh`, machine
     id `C1 A4` plus the handle, never reused), `Frame::machine`,
     `live_proto::bots` (roster, `local_xuid`, high bits `0x000E`).
   - **6b**: `bots::phantom::session` (the recipe of §5.1 as memory writes).
     `Phantoms::tick` keeps membership before each `RENDER_FRAME`; phantoms
     join only once the game runs and wait while this console is not the
     host (`NotNow::NotHost`). The Clankers page count past the free ports
     adds phantoms ("Clanker 4" and up).
   - **6c**: server side `live_server::bots` (xuid range `0x000B`,
     `BotIssued`, `POST /v1/games/bots`, `POST /v1/admin/bots`, acceptance
     rules of §9.3) and console side (`live_hle::Hle::roster_bots`,
     `boot::clankers::Driver::with_rosters`, `clankers::roster_seats`,
     `live_telemetry::Bots`). Without a roster, phantoms are host-made.
   - World mode 4 is the distributed server (`world_mode`,
     `WORLD_DIST_SERVER`); `SESSION_GAME_START` (`0x6ec80`) makes a lone
     host peer this console's; `LOBBY_ROW_PRESS` (`0x250fb3`) logs the start
     state. The roster carries across a session handoff
     (`RosterRequest::carry_from`, §9.5).
   - **`bots::phantom::add` and `remove`** return once the phantom is on the
     books; the machine, peer and player are made at the next game-thread
     tick. `list() -> Vec<PhantomInfo>` reports `Pending`, `Joined` or
     `Failed(reason)`; `player(handle)` serves `sim`'s `phantom_player`.
   - Tests: `cargo test -p bots phantom`, `-p bots --lib phantom::session`,
     `-p bots --lib lobby`, `-p boot --tests clankers`,
     `-p live-server --test bots`, `-p live-proto bots`,
     `-p live-telemetry --lib session`, `-p live-hle --test server roster`.
7. **Clankers page (Game Setup).** Built: `boot::clankers`. Settings
   `clankers.*` live in `setting_defs::ALL`: `enabled` (default Off),
   `count` (1 to 15), `difficulty` (Easy, Normal, Heroic, Legendary, Mixed),
   `team`, `aggression`, `aim_style`, `taunts` (default On), `glitches`
   (default Off, §3.8).
   - **The page** is the shared settings dialog `clankers`
     (`content/ui/screens/clankers.yaml`; docs/game/ui-builder.md, "The
     settings dialog"); preview with `h2 ui preview clankers`.
   - **The row.** A custom game's Game Setup dialog gets a "Clanker
     Options..." row with action 11 (`h1_options_row::insert_row`).
   - **Seats.** The match host creates the peers as the game starts
     (`crates/extensions/game/src/clankers.rs`, `bot_glue.rs`); see §5 and
     §9.1. Bots sit only in a custom game's lobby (`clankers::admits_page`,
     `Driver::set_lobby`, [party-lobby.md](party-lobby.md#clankers-by-tab)).
   - **Nav meshes early.** `clankers::map_picked` calls
     `bots::autonav::build_soon`.
   - **Game check.** Host a lobby, open Clanker Options, set Count 2, start
     and confirm two phantoms join. Combat checks are in §11, the hosted
     harness in §13.

   **Combat decisions.** `bots::mind` uses `bots::arms` and `bots::tactics`
   to pick weapons, targets and positions: melee, trigger rhythm, no dry
   fire, weapon choice, `tactics::choose` targets, projectile-speed lead,
   pre-aim, and the signals `fight.band` and `fight.wants_cover`. Tests:
   `-p bots --lib arms`, `tactics`, `mind::fight_tests`. Not traced: hit
   and aimed-at events, lobbed projectile gravity, the sword's lunge range.

   **Built: weapon skill.** `bots::grenades` (`fly`, `solve`, `Thrower`),
   `bots::weapons` (`reach`, `Combo`, `LeftHand`), `bots::arms`,
   `bots::tactics`, `Walls::hit` in `bots::los`. Grenade physics are
   assumed, not read from the game (`ASSUMED_GRAVITY`, `FRAG`, `PLASMA`);
   the addresses a tracing lane needs are in the note. Tests:
   `-p bots --lib grenades`, `weapons`, `arms`, `los`, `mind::fight_tests`.
   Game run: §11.1, "Weapon skill".

   **Built: natural feel (sol).** Aim, walking and buttons follow the
   simulation tick with separate seeded random streams: aim flicks and
   overshoot, world-space acceleration, look-ahead, crouch-walking, weapon
   taps, idle fidgets, and `bots::incoming` for dodging live grenades. Unit
   filters: `mind`, `tactics`, `moves`, `nav::tests`, `incoming`, `sound`,
   `phantom::session` with `cargo test -p bots --lib`. Game run: §11.1,
   "Natural feel".

   **Built: decisions boost (astra)** (§§3.3 and 3.7). Same-team bots
   exchange target calls that expire after 2.5 s, trade a recent friendly
   death for 3 s, withdraw from a visible numerical disadvantage and mark
   dangerous approaches for up to 20 s. See §3.2 and §3.6. Filters:
   `decisions`, `objective::tests`, `arms::tests::decisions_`,
   `guest::tests::decisions_`, `mind`. Game run: §11.3.

8. **Clankermatch.** First slice built (§8.3, §8.4; run checklist §8.5): the
   Live menu row, the playlist filter at `0x00192db0`, the clanker hoppers
   and `GET /v1/hoppers/clankers`, lobby fill from the playlist's roster,
   and `kind: "clankermatch"` in the record and read model. Left: a party
   of several humans, the search delay and continuous skill (§8.2.3), an
   arbitration round for rank, `MatchProposed` with the bots as members
   (sbmm-ranked), and clanker marks for a published hopper file.
9. **Vehicles.** Built (§3.9): seat entry and boarding from live tag
   markers, rocket lock-on, chassis clearance for ground routes, Banshee
   flight and turret take/leave decisions. Fixtures live under `vehicle::`
   and `mind::ride::`. The game-run checklist is in §11.4.

## 10.1 Library surface (for the agent API)

- `bots::phantom::add(Identity, team: Option<u8>, Box<dyn PadSource>)`
  registers a phantom. It joins a System Link or Xbox Live session this
  console hosts (§5.1). `Identity` holds the XUID and full gamertag, which
  determine the phantom's appearance.
- `bots::phantom::remove(handle)`, `list()` and `player(handle)` use the
  process-wide registry. `list()` returns a `PhantomInfo` per phantom.
- `bots::pad_source::PadSource` produces one `HostPadState` from a `Frame`,
  sampled once per fresh simulation tick. A source finds its player with
  `bots::guest::perceive` and keeps registry calls out of `pad`.
- Sources: `Scripted`, `Recorded`, `bots::Brain` (nav mesh and collision
  data), or any closure taking `&Frame`.

Detail: see the [detail note](notes/2026-11-26-bots-detail.md), "10.1 Library
surface (for the agent API)".

See [the agent API](../runtime/agent-api.md#sim-simulated-players-and-direct-state)
for script, recording and brain sources.

## 11. What needs a game run

Twelve items are open for the user to play or for a coordinator run: lobby
joins, a second human, Live bot accounts, bot movement on every MP map, aim
and combat feel, emotes, objective games, a real System Link client, the
Clankers page, Clankermatch end to end (§8.5), empty-search hosting (§8.4)
and the glitches (§3.8). The list: see the
[detail note](notes/2026-11-26-bots-detail.md), "11. What needs a game run".

### 11.1 The brain in play (checklist)

Two or three Clankers on Lockout, Midship and one big map, on Normal and
Legendary, judged yes or no for stuck recovery (§3.2), ammunition (§3.6),
cheap wins (§3.3), weapon skill, natural feel and finishing fights. Run it
with `h2 bots test` and its log. The 36 checks: see the
[detail note](notes/2026-11-26-bots-detail.md), "11.1 The brain in play
(checklist)".

### 11.2 Objectives in play (checklist)

Two teams of two or three (a human and Clankers), Normal, covering CTF,
Oddball, King of the Hill, Assault, Territories and Juggernaut (§3.7). The
20 checks: see the [detail note](notes/2026-11-26-bots-detail.md), "11.2
Objectives in play (checklist)".

### 11.3 Decisions boost in play (checklist)

Heroic, at least two Clankers on a team, on a map with a nav mesh and more
than one route. It covers focus fire, odds, CTF and danger memory, ball and
hill, Assault, supply timing and outdoor map knowledge. The seven checks:
see the [detail note](notes/2026-11-26-bots-detail.md), "11.3 Decisions boost
in play (checklist)".

### 11.4 Vehicles in play (checklist)

Two or three Clankers on Coagulation or Headlong (Slayer), Normal then
Legendary: boarding, driving, gunning, bailing out, rocket lock and the
object fields (`+0x1fc`, `+0x1f0`). The nine checks: see the
[detail note](notes/2026-11-26-bots-detail.md), "11.4 Vehicles in play
(checklist)".

### 11.5 Traversal and sandbox movement in play (checklist)

Teleporters, lifts, ledge jumps, Banshee and turret use, vehicle seat entry
and the H1 sandbox maps, checked through `h2 agent` and the `sim.*` methods
(§10.1). The nine checks: see the
[detail note](notes/2026-11-26-bots-detail.md), "11.5 Traversal and sandbox
movement in play (checklist)".

## 12. Next tracing (offline)

Stage 1 tracing (§4.1 to §4.3) is done. Open work: the phantom lobby ready
check (`0x5f970`), join room check (`0x62e30`), `0x5fe20` and `0x14c090`
(§5.1); the per-tick hold, zoom and grenade transitions (§8.5); lobby font
slots; objective pickup and hill checks (§3.7, §11.2); vehicle rocket lock,
`O+0x88` units and lift trajectories (§3.9, §3.2); and plasma heat and
service-phase checks (§3.6). Detail: see the
[detail note](notes/2026-11-26-bots-detail.md), "12. Next tracing (offline)".

## 13. Testing (`h2 bots test`)

`h2 bots test` runs §9.5's game run with no one at the controller and ends
in a verdict. It reuses `h2 coop test --drive` (docs/live/live-coop.md
section 12): the process supervisor, account making and `input.pad` presses.

```sh
h2 bots test [--count 5] [--map lockout] [--keep] [--timeout 360]
             [--wait-clang] [--settings-from DIR] [--no-drive]
h2 bots verdict <bots.log>   # the verdict of a log again
```

**What it does.** It starts a Live server in the harness process on
`127.0.0.1:8077` (`--server-port`) with a throwaway store under
`$TMPDIR/h2-bots-test/run-<time>/` (`--root`), makes the account **Tester**,
and opens one `h2 play` window signed in as Tester with
`--set clankers.enabled=true --set clankers.count=N` (`--count` defaults to
5). `--wait-clang` passes `--tier clang`. The window's `[graphics]` settings
come from `--settings-from`. The driver
(`crates/apps/cli/src/bots_drive.rs`) presses through the menus over the
agent socket, with `H2_UI_WATCH=1`, to Xbox Live, Custom Game and Start
Game. It reads the game's `bots:`, `clankers:` and Live lines, `session.status`,
`GET /v1/sessions?host=Tester` and `GET /v1/games` into `bots-test:` log
lines. It stops when every check has passed, the game closes, or `--timeout`
runs out (counted from the agent socket coming up), prints the verdict, deletes
the run's data and exits non-zero on a failure. `--keep` keeps the game and
data. Step by step: see the [detail note](notes/2026-11-26-bots-detail.md),
"13. Testing (`h2 bots test`)".

**`--no-drive` and `progress.json`.** `--no-drive` launches and watches but
presses nothing; the timeout defaults to 900 s (360 s otherwise). The log
directory gets `progress.json` (`finished`, `passed`, `next`, `phase`,
`mode`, `map`, `last_ui`, `last_bots`, `checks[]` with `step`, `name`, `state`
`pass|pending|fail` and `detail`), rewritten through a temporary file. An
outside driver reads it, looks with `h2 agent --socket <sock> ui` and
`frame snap 1`, and presses with `h2 agent --socket <sock> press ...`.

**The checks**, in the order a run reaches them. The verdict names the first
failure and lists panics, guest faults, refusals, stuck drives and timeouts
under `trouble:`.

| # | Check | Passes on |
|---|---|---|
| 1 | Signed in to Xbox Live | `live: playing as Tester` |
| 2 | Clankers on | `clankers: G (phantom...) on` per bot |
| 3 | On Xbox Live | the Xbox Live players screen or menu |
| 4 | Hosting a Live custom game | a session advertised in lobby mode 2 |
| 5 | Live roster of bot accounts | `clankers: Live rostered bots for session S` |
| 6 | Gameplay on a map | `session.status` multiplayer gameplay, on `--map` if given |
| 7 | Phantoms joined the pregame lobby | `bots: phantom G joins (...)` per bot |
| 8 | The Live session holds 1 + N players | filled slots or host-reported players |
| 9 | The game record has the bots | at least N players with `bot: true` |

**Where things go.** The merged log is `<root>/logs/<time>/bots.log` and the
game's stderr `<root>/logs/<time>/Tester.log`. `h2 bots verdict bots.log`
prints a verdict again.

**Notes.**

- `--map` only checks the map played; the lobby's default map is played.
- Step 8 counts the most filled slots and joined players seen (§5.1). Retail
  Halo 2 fills no slots, so joined players decide a Live run.
- Run `h2 play` with the same `H2_OPT` as the build, or the window
  recompiles first. The timeout starts once the game is up.
- A second `h2 bots test` at once needs its own `--server-port` and `--root`.

### 13.1 Clankermatch (`h2 bots test --clankermatch`)

The same harness runs §8's Clankermatch end to end, so §8.5's steps 4 to 9
have a verdict.

```sh
H2_OPT=2 ~/.cargo/bin/h2 bots test --clankermatch [--playlist slayer|teams|doubles] \
    --disc disc/halo2.iso --wait-clang [--keep] [--timeout 900] [--no-drive]
```

`--playlist` implies `--clankermatch` (slayer by default); `--count` and
`--map` are refused. The merged log's `bots-test: clankermatch plan: ...`
line selects the verdict. The run's fresh store serves the built-in lineup
(`live_server::default_hopper`), whose hoppers 8..15 are the clanker
playlists (`GET /v1/hoppers/clankers`); the game runs with
`--set clankers.enabled=false`, so only the fill seats bots.

| `--playlist` | Hopper | Row | Players | Phantoms |
|---|---|---|---|---|
| slayer | 8 Rumble Pit (FFA) | 0 | 2 to 8 | 7 |
| teams | 9 Team Slayer (2 teams) | 1 | 2 to 8 | 7 |
| doubles | 10 Team Skirmish (2 teams) | 2 | 2 to 4 | 3 |

The drive (`crates/apps/cli/src/bots_drive/clankermatch.rs`) boots, signs in,
opens row 2 (Clankermatch), presses A on the playlist's row, then presses
nothing until the game ends and it backs out of the menus. The timeout is
900 s (1200 with `--no-drive`). Stall rules and exit sequence: see the
[detail note](notes/2026-11-26-bots-detail.md), "13.1 Clankermatch (`h2 bots
test --clankermatch`)".

**The checks** (14): sign-in; clanker hoppers listed by server and game; the
Clankermatch row opening the clanker playlists; the search starting; the
search ending with this console hosting; phantoms planned; no bot in the
party; phantoms rostered; the lobby reaching the playlist's fewest players
(counted from the game's own session, not the Live server); the countdown
and start; gameplay on a map; phantoms joined before the start; a game
record of kind `clankermatch` with the planned `bot: true` entries; and the
phantoms leaving. Log lines per check: see the
[detail note](notes/2026-11-26-bots-detail.md), "13.1 Clankermatch (`h2 bots
test --clankermatch`)".

**Tests:** `cargo test -p cli bots_` (the verdict from log lines, the
driver's presses, the arguments; Clankermatch's under `bots_drive::clankermatch`,
`bots_verdict::clankermatch` and `bots_test_cmd`).
