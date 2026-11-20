# Bots: Clankers and Clankermatch

This document records the original design, implementation traces and
outstanding game checks. The player-options checks are below. Section 11
lists the original multiplayer checks.

Navigation generator 5 routes through lifts and teleporters, including
converted Halo 1 one-way exits. Bots build pickup beliefs from sight and
dated teammate reports, choose firing positions and investigate sounds.
Seeded personality traits shape those choices and match events change
temperament. See [Navigation](#32-navigation),
[Belief and map knowledge](#belief-and-map-knowledge) and
[Personality and temperament](#personality-and-temperament).

What we want: multiplayer bots ("Clankers") that play Halo 2 multiplayer by
the game's own rules. They can be added to a custom game from Game Setup.
They can also fill a Live match in **Clankermatch**, a matchmaking row that
plays exactly like OptiMatch.

Labels: **traced** means read from the retail XBE with `h2 disasm`, at the
addresses given. **Repo** means it is already in this repository's code or
docs. **Design** means it is our decision. **Run** means only a game run can
settle it. No game data is in this document: it has addresses and paraphrase
only.

## Clanker appearance and Live player options

A Clanker's full gamertag determines its emblem and armour colours across
local slots, System Link, simulated players and Live rosters. Switching
controller, host or account XUID keeps the same look for the same name.
The profile model still comes from the host. Team games can override the
visible armour colour.

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
  name-only rows and human accounts on the retail path. A local guest
  sharing a human XUID must match the bot slot's full 12-byte player id.
  A guest on another console needs a matching, gear-marked session slot.
- The card opens on the kit's large dialog and offers Back. Skin 6 of
  the current interface contains the Y menu's player table. Its nested
  skin's first shader bitmap uses the medium emblem material in the
  main menu, shared map and Zanzibar. The card copies that definition
  and feeds the profile to the retail emblem setter at `0x2b12ca`.
- The host shows the active brain's difficulty and strongest personality
  trait. A remote or recent server bot resolves by XUID through
  `GET /v1/games/bots/{xuid}`, which returns its full tag, seed and last
  rostered skill. A scripted player with no brain, or a remote player
  whose details cannot be retrieved, shows “Unavailable”.
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
remote client, a human guest next to an unleased local bot and two
controllers. Check the same name after restart and in a non-team System
Link game against its Live card and web emblem. Use a legacy friends
entry if available, since new friend requests to bots cannot create one.

## 1. Decision

**One brain, two ways in.** A bot is a brain that outputs controller input
and nothing else: two sticks, triggers and buttons, one frame at a time. It
uses no cheats and does not write game state. The game applies its own rules
to that input: movement, aim assist, weapon switching, grenades, melee,
crouch, vehicles.

The brain's output reaches the game in one of two ways:

1. **Local slot (stage 1).** The brain drives an unused local player's pad.
   That gives up to 3 bots per machine. Their views are not drawn.
2. **Phantom peer (stage 6).** The brain drives a remote player who does not
   exist. The host makes up a machine for it and stores that player's actions
   each tick, as if they had arrived over the network. That gives up to 16
   players in total.

Both ways run on the host. Custom games currently fill free local ports
first. Set `H2_BOTS_ALL_PHANTOM=1` before launch to keep every local port
available for humans in a hosted System Link or Live game. Clankermatch
already seats its whole roster as phantoms. The default stays at 0 until
a paired sim batch shows that phantom hit rates match local hit rates (§3.4).

**Option (c) is rejected:** AI actors on MP maps are not players. An `actr`
(the game's AI actor) with a biped is not in the players array. It has no
scoreboard row and no team score, it cannot carry the flag or ball, and kills
of it are not kills in the carnage report. MP maps also have no characters,
squads or pathfinding (§5.3). What we keep from that work is
`crates/pathfind`, used to build the nav mesh (§3.2).

### 1.1 The three options, compared

| | (a) local slots | (b) phantom peers | (c) AI actors |
|---|---|---|---|
| Is a player (scoreboard, objectives, carnage) | yes | yes | no |
| Most bots | 3 per machine (4 users per machine, one human) | 16 minus humans | memory-bound |
| Engine work | hide the bot views; sign a guest in | fake a machine and player; store its actions; quiet the network for it | load chars and squads into MP; place actors; teams |
| Live/System Link with humans | allowed only if guests are (Run) | yes; the remote humans see a normal player (§6) | clients see bipeds that are not players |
| CPU per bot | one extra local player's simulation; views skipped | the same as one remote player | an actor update |
| Nav | ours (§3.2) | ours | needs pathfinding data, which MP maps lack |
| Fits "only controller input" | yes, natively | yes, through the action conversion | no |

## 2. What we traced

### 2.1 Players and machines (traced)

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

  One machine has at most 4 users. More than 3 bots on one host therefore
  needs extra machines, which is a second reason for phantom peers.
- **Local slots.** 4 entries of `0xc70` bytes at `0x54e8e0`. Flag `0x10`
  (bit 4) means a profile is loaded, and `0x20` (bit 5) means signed in to
  Live (§4.2). The profile block is at `+0x18`.

### 2.2 The action path, shared by both ways in (traced)

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

What this means for bots:

- A **local-slot bot** feeds the pad. Steps 1 to 3 then run for it as for a
  human, so nothing else is needed.
- A **phantom bot** supplies its brain's pad when the host gathers the
  remote slot for simulation. After uplink delivery the same retail reader, layout mapper and action
  builder run against a private local-user view of that seat. See §5.1, “The action from a pad”, for the fields and timing.

### 2.3 Views (traced)

`render_game_views` `0x2234e0` takes its view count from `[0x4e8c20]+8`,
clamped to 1..4. The split layout comes from `[0x4e6948]` and `[0x485ac2]`
(two views give layout 1 or 2). For each view it takes the next local user
whose map entry is not -1, and calls
`0x223080(view i of n, layout, …, 0x4e9c8c + u*0x358)`.

**Design:** hide bot views with a window around `0x2234e0`. On entry,
lower the count and set the bot users' map entries to -1; restore both on
exit. The human then gets a full-screen view. Aim assist uses the same kind
of observe window. The HUD, sound listeners and the per-local-player
director (`0x4e8c44`, stride `0x140`) read the same map and may also need
the window. That is checked offline next (§10), and confirmed in a run.

### 2.4 AI (traced)

`ai_update` `0x1c83c0` runs only when AI is enabled (`[0x4f55d0]+1`) and the
game-engine state at `[0x4e6948]+0xc` is not 4. The actor array is at
`[0x4f55f0]`, stride `0x888`. MP scenarios carry no characters, squads or
`pathfinding` blocks (Repo: the h1ai and pathfind work generate exactly
these for converted maps). This is why option (c) would need a whole AI
layer before its bots could even stand still.

### 2.5 Menus (traced, Repo)

**Main menu.** The row handler is `0x230827`; the chosen row is at
`0x510a14`.

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
  picks a profile (`0x18fc44`, `0x24b779`, `0x24b6ea`). Extra local players
  join this way, so bot local slots do too.

**Live menu.**

- Screen `0xba`, list "xbox live menu list" `0x458c80`.
- Rows: Quickmatch 0, OptiMatch 1, Clankermatch 2, then Co-op Campaign 3
  (directly above Custom Game), Custom Game 4 (retail row 2, moved), Custom
  Game Browser 5, Content Download 6 (rows close up by one without co-op).
- `crates/boot/src/custom_games.rs` adds our rows: row text `0x230f36`, row
  chosen `0x230f92`, last row `0x510a18`.

**Matchmaking** ([live-matchmaking.md](../live/live-matchmaking.md)).

- The playlist UI `0x65530` keeps descriptor types {2,4,5}.
- The start flow is `0x231066`; the session state machine is `0x192eb0`
  (states 6, 7, 8).
- Search goes through the `0x8fa80`, `0x8fb30` and `0x8fc5c` wrappers, and
  QoS through `0x7b4c0`.
- The server-side matcher (§3.4 there) writes `MatchProposed` on
  `match-<id>`. It is designed but not built.

**Game Setup** (`squad_settings_dialog`, screen 23, window 3; row chosen
`0x2bb801`, jump table `0x2bb884`) already carries our **Halo 1 Options**
row. That row opens an `OptionsScreen` of `OptionRow`s (Sandbox, Assets,
HUD) whose values are kept in the settings store (`crates/boot/src/h1_options.rs`).

## 3. The brain

The brain is a pure function, run once per tick for each bot:

    (perception, memory, params) -> (pad, memory')

It runs on the host's game thread, after the tick's world state is final and
before `0x1870a0` reads input. It has no knowledge of which way in it uses.

### 3.1 Perception

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
  and kills (`0x15ce70`) for emote triggers (§3.5).

**Line of sight** is a ray against the map's BSP collision, the same data
the nav mesh comes from. A bot sees an enemy only when all three hold:

- a ray reaches the enemy's head or chest;
- the enemy is inside the bot's view cone;
- a reaction delay has passed (§3.4).

The host gives each bot its own view, teammate HUD markers, audible shots,
radio calls and public match information. Motion-tracker blips would be
another legitimate input once a reader for the game's displayed blips exists.

Each brain keeps a short, tick-stamped history of world samples, vehicle
sight, nearby grenades, sound, deaths and teammate callouts. It thinks
from the newest sample at least `perception_ticks` old. A repeated poll
in the same tick reuses the previous pad. Local slots sample through the
host's ordinary input path. If the host skips a sample, the brain can use
an older observation. The first live sample after a respawn starts a
fresh history.

### 3.2 Navigation

Every map gets a nav mesh, built offline from the BSP collision by our own
tool and cached by map fingerprint. `bots::autonav::build_map` reads the
scenario and builds one `<map>.<bsp>.bots` per BSP. `bots::cache::build`
starts with the sectors from `pathfind::generate::generate`. The bot
generator adds its links and collision probes to those sectors. Generator
version 5 regenerates earlier caches and includes tactical places.

The mesh holds:

- walkable surfaces: collision planes with an upward-facing normal, joined
  across shared edges, with step-up and headroom checks for the biped's size;
- links:
  - **drop**: a ledge to a lower surface within the fall-damage limit;
  - **jump**: a gap or ledge reachable at the biped's jump arc, checked by
    sweeping the arc against collision;
  - **crouch-jump**: a separate takeoff and landing for ledges reachable
    with the smaller capsule. The follower presses A, detects the rise,
    then holds crouch two simulation ticks after takeoff until it reaches
    the landing.
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

Paths come from A\* over the sector graph, smoothed with a funnel pass and
followed by steering:

- a look-ahead point;
- a corner slowdown;
- "stuck" detection and recovery (below).

A move becomes a left-stick vector relative to the bot's facing. A bot
never warps.

**Traversal evidence and estimates.**

| Input | Evidence |
|---|---|
| Teleporter flag position, type and channel | `scnr+0x118`, stride `0x20`, position `+0`, types `7` and `8` at `+0x10`, identifier `+0x14`. See `scenario-edit/src/layout.rs`, `h1convert/src/apply.rs` and the retail checks in `h1convert/tests/h2_scenario.rs`. |
| Scenery, machine and crate placement | `scnr+0x50`/ `+0x58` (scenery block / palette, strides `0x5c` / `0x28`) and `+0xa8` / `+0xb0` (machine, `0x48` / `0x28`). Palette index `+0`, flags `+4` (bit 0 suppresses automatic placement), position `+8`, yaw `+0x14`, palette datum `+4`. See the object-type table `0x00468630`, palette access `0x000a743b` and `scenario-edit/src/layout.rs`. Crates use the placement at `+0x328` (stride `0x4c`) and palette at `+0x330`. |
| Lift object identity and travel | The retail machine palette places `power_boost` on Ascension, `grav_colossus` four times on Colossus and `elevator_cyclotron` on Cyclotron. Name patterns for modded placements, the launch direction, travel envelope and exit timing are **[guess]**. The generator scans for a landing that fits a standing biped and checks the path against the BSP. The reverse axis used for Ascension projects onto elevated playable BSP geometry. |
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

#### Traversal audit by map

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

The jump probe found ledges above standing-jump reach with a clear
crouched arc on Lockout (0.40 units), H2 Beaver Creek (0.36), H1 Blood
Gulch (0.57) and H1 Sidewinder (0.40). These are candidates for the
existing crouch-jump follower. Check takeoff, ceiling clearance and
landing with a player-sized biped. There is no separate ladder action.

#### Belief and map knowledge

**What can reach a decision.** The local and phantom pads both pass host
candidates through `perceive::observe` before combat, movement, radio or
memory. The paired-world harness in `perceive::honesty` runs seeded brains
with separate radios, comparing projected inputs, controller output,
chosen activity, kit and outgoing enemy call. One fixture puts a hidden
enemy and power weapon behind a base wall on an H1-shaped valley. A
second pair starts at the production guest reader, changing a hidden
biped's state in the synthetic object table before either brain runs.

| Host input audited | Information the bot receives |
|---|---|
| Living unit table, aim and weapon slots | Enemies must pass view cone and BSP sight. Teammate positions act as HUD markers. Aim and the visible held weapon require sight on either team. Exact opponent shields, reserves and magazine counts are unavailable. Body avoidance, weapon seeking and group decisions use the filtered list. |
| Ground objects and placement records | A visible weapon supplies its model. Its remaining ammo becomes known when held. Occupied power sites require sight. A successful scan establishes emptiness only at a known pad the bot can inspect. A failed scan supplies no observation. |
| Vehicles and seats | A visible hull or the bot's own ride supplies the model, pose and seat layout. Identifying a rider on another vehicle also requires sight of that player. Remote vehicle health uses a neutral estimate. The bot can read its own ride's health. |
| Objective globals | Game mode, objective geometry, bases and the active hill are public. A loose objective or enemy carrier requires sight. Friendly carriers can be followed by their teammate marker. |
| Shots, projectiles and death log | Audible shots give a direction for a look or search. A grenade dodge requires a close, visible projectile. The kill feed supplies identities. A fresh visible body or the bot's own death can supply a position. |
| Scoreboard, radio and map tags | Scores and team membership are public. Radio distributes dated observations on completed ticks. Static placement periods and player starts describe the map. No variant countdown or actual enemy spawn selection feeds the planner. |

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
The sound gives it neither a combat target nor a reason to fire. A kill
announcement retires the old sighting and may prompt a look toward a
plausible out-of-view map start. Search confidence expires on the existing
enemy-memory clock.

Per-bot experience records verified supply visits and confirmed kills
from a recent firing position in coarse map regions. A versioned serde
snapshot can be restored to another brain. A future Live profile adapter
can store it under the persistent ranked bot's account and map revision
on the local server and worker. Within this lane the brain owns it in
memory. It survives match resets for that brain's lifetime.

**Getting unstuck** (`crates/bots/src/stuck.rs`, built). A pure state
machine, run every tick the bot is walking somewhere (chase, roam, going
for a weapon; not while it strafes in a fight). It measures progress the
way the bot wants to go: the position's gain along the wanted direction
since the start of a window. A window of 20 ticks (2/3 s) with less than
0.25 world units of gain escalates one step:

| Step | What the bot does for its window |
|---|---|
| sidestep | strafes to one side (alternating) and backs off a little, still facing the way it wanted to go |
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

The follower's own stall check (`nav::Follower`, a jump after 30 frames
without getting closer to the waypoint, then a fresh path) still runs
under this; the state machine is what escalates and blacklists.

**Corners and doorways** (`nav::NavMesh::portal`, built). A path never
aims at a wall corner itself: each portal end that is a corner of the
mesh (a vertex on a side nobody walks across: a wall or a ledge) is
pulled 0.3 world units in (`nav::CLEARANCE`), so a bot turns a body's
width off the corner instead of catching its shoulder on it. A link
the builder made between surfaces that do not share an edge (a doorway
off a long wall) spans the near polygon's whole side; the portal is cut
to the part the far polygon lies along, so the path goes through the
door and not through the wall beside it. Nearing a corner (0.8) the
follower bends toward the next leg, so a turn is a curve, not a stop
and a pivot (never into a jump or a drop, which start at their edge).

**Off the mesh** (`nav::NavMesh::nearest`, built). A start or goal no
polygon is under (a bot on a crate or knocked off an edge, someone seen
mid-jump) snaps to the nearest polygon within 3 world units whose floor
is at most 0.6 over it or 3 under it: the bot walks (or jumps up) onto
the mesh first, and a chase to someone seen in the air goes to the floor
under them instead of giving up. A bot whose next waypoint's floor is
more than 1.2 (plus its distance) over it has fallen below its path and
plans again at once.

**Moving in a fight** (`crates/bots/src/moves.rs`, built). The feet in a
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
  footwork reaction at most every 20 ticks. Jumping is reserved for
  navigation, getting unstuck, and visible grenade threats.
- **Range**: closes in beyond the far end of its range and backs off
  inside the near end. The range comes from `moves::preferred_range`,
  today 2.5 to 8 for every weapon: the seam for the weapons decision to
  give a shotgun a short range and a sniper a long one.
- **Shields down or reloading**: backs away, reloading at half the pace
  and never closing in. With cover allowed it looks (every 15 ticks) at
  16 spots 1.5 and 3 away, walkable in a straight line, for one where the
  enemy's eye cannot see its chest through the walls (`los::Walls`), and
  goes to the nearest such, favouring ones away from the enemy; with none,
  straight back.
- **Footing**: every step is checked 0.5 and 1 ahead on the nav mesh:
  no drop or rise over 0.5 (no strafing off a bridge) and no wall at
  knee height. Blocked, it strafes the other way, else steps forward or
  back, else stands.
- **Stuck in a fight**: pushing a way for 15 ticks without moving 0.15
  reverses the strafe and jumps.

**Roaming** (`crates/bots/src/roam.rs`, built). With nobody to fight or
chase, a bot goes to likely places: each candidate is scored, the best
it can reach and has not been blacklisted wins.

- a place it saw an enemy in the last 60 s (4.5, fading to 0.5);
- a weapon lying in the world worth taking (1.5);
- six polygons picked at random (0.5), so it still explores;
- less 2.5 for a place within 5 it was at in the last 40 s (fading), and
  2 for one under 4 away;
- plus a random wobble: 8 on Easy (it wanders nearly at random), 2 on
  Normal, 1 on Heroic, 0.4 on Legendary (it goes where the fight was).

### 3.3 Decision

The design calls for a utility layer running about five times a second.
These are the inputs to that design.

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

**In play today.** `Mind::step` chooses an activity per tick. A flag or
bomb carrier moves toward its scoring point. A ball carrier looks for
cover and melees anyone in reach. Both suppress weapon switches.
An armed objective player stays on assignment. A dry player can still
detour to get a usable weapon. In King, it only collects guns inside the
active hill.

A bot can fire at a visible opponent while walking toward the hill,
guarding its flag or escorting a carrier. Target choice combines the
existing range, turn and stickiness costs with the objective, teammates'
calls, recent kills and visible openings. A bot that loses sight plays its
objective first. In Slayer it can recover shields, respond to a call,
regroup after withdrawing, collect a due supply, hold an angle, investigate
its last sighting, or roam. A team call alone never authorizes a shot.

Reload (X) and switch weapons (Y) take 5–13 ticks of stable intent,
depending on skill, then hold 2–4 ticks. Retries are at least 2 s apart
for reload and 1.3 s for switch. Melee also needs a target in reach and
near the crosshair for a short time before the press.

Vehicles (§3.9) sit across this order: in a vehicle the vehicle layer
makes the whole pad; on foot, after the order above, boarding a close and
slow enemy vehicle takes over in a fight or not, and taking a free one
takes over only when calm and not carrying or rearming.

Grenades (left trigger) need one in hand (unit `+0x23e`, §3.6, when read).
At the skill's rate in a fight at 3 to 12 world units, four times that rate
when another enemy is within 2.5 of the target, three times before pushing
in with a short gun; and at twice the rate at where an enemy went out of
view in the last 2 s, while chasing. Each throw is flown against the map's
collision and aimed to land (`bots::grenades`, "Built: weapon skill" in
§10). In a dual-wield fight the bot can drop its left-hand gun for a
throw it has already lined up, then waits for the hand to be free.

#### Personality and temperament

A Clanker's identity seed fixes its trait draws and favorite weapon family.
Offline bots use a seat seed. Live roster bots use the server's personality
seed in both local and phantom seats. A difficulty change maps the same
draws into the new bands. At the same difficulty, a persistent bot starts
every match with the same traits and a fresh mood.

| Trait | What changes in play | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|---|
| Aggression | Takes openings and closes distance | .05–.95 | .12–.92 | .24–.90 | .35–.88 |
| Caution | Respects being outnumbered and routes around recent danger | .05–.90 | .16–.88 | .30–.84 | .46–.82 |
| Teamwork | Responds to radio calls and helps teammates | .08–.85 | .18–.88 | .32–.88 | .46–.87 |
| Objective focus | Stays closer to the assignment during a fight | .05–.90 | .18–.88 | .35–.90 | .48–.89 |
| Composure | Has smaller swings in mood and settles faster | .08–.50 | .28–.70 | .50–.86 | .73–.96 |
| Patience | Holds a strafe or a corner longer before chasing | .05–.48 | .22–.72 | .42–.85 | .65–.91 |
| Showboating | Attempts riskier melee and sword lunges and taunts more often | .10–.95 | .10–.87 | .12–.75 | .15–.67 |
| Weapon loyalty | Favors a weapon family and may take a short detour for it during a lull | .05–.95 | .13–.88 | .20–.80 | .28–.72 |
| Chattiness | Refreshes radio sightings more often. New targets and urgent help go out immediately | .05–.95 | .12–.90 | .25–.83 | .36–.80 |

Each trait is drawn independently and uniformly inside its band's lower and
upper bounds. The narrower bands give Legendary bots more consistent
discipline while leaving room for aggressive Clankers. Favorites are drawn
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
timing and thumb coordination limits before reaching a local port or the
phantom's delayed uplink.

The map decision layer uses `Personality::wait_weight()` for patience
near a predicted spawn and `Personality::weapon_interest(role)` to value a
favorite weapon there. A patient sniper with a favorite rifle will also travel
farther for a firing position that overlooks a stronger lane. See [the agent bot view](../runtime/agent-api.md#sim-simulated-players-and-direct-state)
for the live traits and mood.

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
| tracking error (std. dev. of the angle) | large | medium | small | very small |
| lead (fraction of the needed lead) | 0.3 | 0.6 | 0.85 | 1.0 |
| flick overshoot | large | some | little | little |
| recoil and burst control | sprays | bursts | paced | paced |
| grenades (arc solve) | rarely, poor | sometimes | often, good | often, good |
| weapon choice by range | no | partly | yes | yes |

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

The right thumb takes several ticks to cross the stick well. Large flicks
carry a little past the target before settling. Small correlated errors and
the occasional larger misread vary by skill. Fast crossings leave more
tracking lag than slow ones. Both virtual pads use the game's stick response, look acceleration and
aim assist at sensitivity 3 (§5.1).

At the controller boundary, each stick stays within its unit circle and
can travel at most 0.45 stick radii per tick. A full reversal takes at
least five reports. The model uses a standard two-thumb grip. The right
thumb can press one of A/B/X/Y/Black/White while the right stick is
centred, or use the stick or its click. It can switch between those jobs
once per tick (33 ms) by default. A short press can be missed while the
stick returns to centre. The left thumb can move its stick and click it.
The index fingers can use the triggers during a face-button press.

Button and trigger transitions draw independent, centred Gaussian timing
errors using a per-bot seed. An error of, for example, 6 ms has a 6/33.3
chance to shift the report by a tick in the direction of the error.
Reports slip by at most one tick. At zero motor delay an early edge can
only land on the current tick. Two edges on the same boundary can erase a
short tap. Sticks use the nominal motor delay. The same seed, samples and
tuning reproduce the same input.

`bots::tuning::Tuning` exposes `perception_ticks`, `motor_ticks`,
`button_sigma_ms`, `stick_step`, `thumb_switch_ticks`, `phantom_ping_ms`
and `phantom_jitter_ms`. The sim accepts these in bot tuning files
([sim.md](../runtime/sim.md#bot-parameters-from-a-file)).

Lead comes from the target's velocity and the weapon's projectile speed,
read from the tag at load; hitscan needs no lead. A grenade's throw is
flown against the map's collision (bounces, the fuse, a plasma sticking)
and its aim searched until it goes off at the target; the bot throws only
when one does (built: `bots::grenades`, "Built: weapon skill" in §10).
The numbers in the table are starting points; tuning them needs runs
(§11).

#### Phantom aim diagnosis and next steps

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
**Same-view assist parity is inferred, needs a run.** Hit-rate parity
remains unmeasured. Compare matched Midship
seats with `H2_BOTS_ALL_PHANTOM=0` using the ten-seed
[seat-swap batch](../runtime/sim.md#local-and-phantom-seat-swaps).
Check `sim.player.list` for `aim.assist_player` and `aim.assist_strength`
as a target enters and leaves the reticle. Compare opposing hits per shot
and damage across the two legs before changing the default. Then compare
the hand-written aim and the hand-built aim net on matched maps, lineups
and seeds (§3.4.1).

#### 3.4.1 The aim net (built, not compared in a game yet)

The user asked for a small neural net with hand-written weights for aim
and tracking, tested in simulation against the code above, the winner to
stay. `bots::aim_net` is that net. Nothing is trained and there is no ML
dependency: the weights are `const` arrays built from named constants, so
each hidden unit has one job.

**The switch.** `aim_model = "code" | "net"` in a bot's tuning
(`bots::tuning::Tuning::aim_model`, `sim::config::BotParams::aim_model`).
`H2_BOT_AIM_MODEL=net` sets the default for bots whose tuning does not
name one. The default is `code`.

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

**Outputs (2).** The yaw and pitch turn, each the weighted sum of its
units clamped to −1..1. The hand maps `[yaw, pitch]` to the right stick
`(−yaw, pitch)`, keeps it in the unit circle and moves the thumb there.
No fire gate: the trigger stays with the shared fire cone, so the
comparison measures aim alone.

The pending-turn input is what separates the two models most. A view is
`perception_ticks` old and a report lands `motor_ticks` later: five ticks
at easy and normal, four at heroic, three at legendary. The code aim
steers on the old view as if it were current. The net subtracts the turn
it has already asked for. In a scratch model of the nominal stick curve
with that delay, written while the weights were designed, the code aim
swung past a still target and at heroic and legendary gain kept swinging.
No test asserts that and no game run has shown it: the real stick
response has look acceleration and aim assist that the nominal model
leaves out. The comparison is what says.

**Tests.** `cargo test -p bots aim_net` covers the network alone: the
same bits for the same inputs, no stick for no error or inside the dead
band, sign and axis, saturation, the gain order by skill, each named
unit's effect, and the table's shape. `cargo test -p bots look` runs both
models through one kinematic harness (the nominal stick curve, the view
as late as the skill's delays): determinism, the code aim untouched by
the switch, the net landing a 0.95 rad flick and settling sooner and
closer at each higher skill, tracking a strafing and a crossing target,
and the mood scaling of its error.

**The comparison.**

```sh
H2_OPT=2 h2 sim batch docs/runtime/sim-examples/aim-net-vs-code.toml --disc disc/halo2.iso --parallel 3 --pace unpaced
```

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
default becomes `net`, then the code's tracking lag and flick offset go.
If the code wins, `bots::aim_net`, the switch and the hand's net state
go. The aim-motion rows and `arm_swap` stay either way.

### 3.5 Emotes (the teabag layer)

The user asked: "make them teabag if you do something dumb". The emote is a
short burst of crouch presses over a body, made with the real crouch input.
The kill feed supplies the body position and the event. Self-kills (including
world deaths) and betrayals can prompt a taunt. Showboats can also taunt over
their own ordinary kills, at a lower rate.

The bot may emote only when:

- the body is within 8 m;
- no enemy has been in the bot's view for 1.5 s;
- the bot is not carrying, defending or contesting an objective that is live
  (a flag at risk, a bomb armed, a hill or territory contested);
- it has not emoted in the last 30 s.

When it does, it presses crouch 3 to 6 times at a human rhythm (built:
130 to 200 ms per press), for 2 s at most, and stops at once if an enemy
appears, an objective takes priority, or taunts are turned off.

How often it happens:

    chance = clamp(taunt × severity(event) × showboating_weight, 0, 1)

Severity is 1 for a betrayal, 0.8 for a self-kill, and 0.35 for the bot's own
ordinary kill. Showboating and current confidence set the weight. The
Clankers page's **Taunts** switch sets `taunt` to 0 when off. A persistent bot
has its own base taunt value.

### 3.6 Weapons and ammunition (traced)

The user, after playing the Clankers: "they don't know when they have no
ammo". The bot now reads what it carries and what lies about
(`crates/bots/src/arms.rs`), and a pure decision (`arms::plan`) turns
that into a reload, a weapon switch, a walk to a weapon, or a melee.

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

**Handling.** What kind of gun a weapon is comes from fields of its
`weap` tag other docs already traced; nothing new was traced for it
(`arms::WeaponDef::profile`, `arms::Profile`):

| Field | Where | Confidence | From |
|---|---|---|---|
| zoom levels | `D+0x1fe` (`i16`) | checked on Zanzibar's tags | `boot::touch_glue` (sniper and beam rifle 2; battle rifle, carbine, rocket launcher 1) |
| aim assist reach | `D+0x20c` `cone_range`, `D+0x214` `pull_range` (`f32`, world units) | confirmed (engine) | [aim-assist.md](aim-assist.md) (`0x1a50a0`) |
| triggers block | `D+0x2c8` count, `+0x2cc` pointer, `0x40` each; `+6` the behaviour (`u16`: 0 while held, 1/4/5 once per pull, 2/3 charge) | checked on Zanzibar's tags | `boot::touch_glue`; [halo1/objects.md](../halo1/objects.md). None on the energy sword |
| rounds a second | barrel `+8` (`f32`, the most) | checked on Zanzibar's tags | `boot::touch_glue` |
| projectiles per shot | barrel `+0x6a` (`i16`) | field order from data, value pattern on retail | [halo1/objects.md](../halo1/objects.md) |
| projectile | barrel `+0x8c` (tag reference, datum at `+0x90`) | field order from data, value pattern on retail | [halo1/objects.md](../halo1/objects.md); the bot accepts it only when the entry's group is `proj` |
| projectile speed | `proj+0x17c` (`f32`, world units a second) | confirmed (engine) | `live-telemetry` (`0xf8487`) |

The first trigger and the first barrel speak for the weapon. A fault
reading any of them leaves the gun weapon-blind (as before), not unread.
The **role** (`arms::Role`) follows: two zoom levels a **sniper**; once
per pull with a projectile under 100 a second a **blast** weapon (rocket
launcher, fuel rod, brute shot); once per pull with 4 or more
projectiles a **scatter** gun (shotgun); once per pull otherwise
**precise** (magnum, battle rifle, carbine); held or charged **spray**
(SMG, plasma rifle, needler, plasma pistol); no trigger but aim assist a
**blade** (energy sword). The Zanzibar flag, ball and bomb each have a trigger. Each role has a band it is best at and a range
it shoots to (world units):

| Role | Band (`arms::preferred_range`) | Shoots from, to | Rating |
|---|---|---|---|
| unknown (not read) | 2.5 to 15 | 0 to 25 | 1 |
| blade | 0 to 1.5 | (melee) | 4 |
| scatter | 0 to 4 | 0 to 8 | 4 |
| spray | 1.5 to 9 | 0 to 18 | 2 |
| precise | 5 to 28 | 0 to 40 | 3 |
| sniper | 12 to 60 | 0 to 80 | 5 |
| blast | 5 to 25 | 2.5 to 35 | 5 |

**The decision** (`arms::plan`, pure, in order). "By range" is Normal
skill and up:

1. a blade in hand: keep it; by range, with the enemy out of its reach
   and a ready gun in the other hand, **switch**;
2. the held gun is dry (no rounds anywhere, or a battery under 2%):
   - the other has something: **switch** (Y);
   - an enemy in view within 2.5 world units: **melee** (run in, B);
   - a useful pickup within 30 world units, not blacklisted: **seek**
     the best near one (each rating point counts as 3 units nearer);
   - an enemy in view: **melee**; else nothing;
3. an empty magazine with a reserve: **reload** (X), in a fight or not;
   by range, with the other gun loaded and the enemy in its band,
   **switch** instead (faster than a reload);
4. by range: the enemy clearly outside the held gun's band (by more than
   30% of it) and inside the other's: **switch**;
5. **reload**: calm (no enemy in view for 1.5 s) and under 35% of a
   magazine with a reserve; or a lull (no enemy in view for 0.5 s) and
   under 60%; or the enemy beyond the gun's range and under 35%;
6. no enemy in view, calm or a lull, and no power weapon (rating 4 and
   up) carried: a power weapon within 25 it does not carry: **seek** it;
   holding the better of two guns, **switch** first, so the one dropped
   for it is the worse;
7. calm, the held gun under 20% of all it can carry and the other one too
   (or none): **seek** a useful pickup within 12;
8. otherwise nothing.

Seeking walks there along the nav mesh. A weapon of a kind it already
carries is walked over: the game moves its ammunition across by itself
(`0x101740`). One of another kind is taken by holding X on it (within 0.5
world units) for at most 1 s; the dry one in hand is dropped for it.

The presses are Halo 2's Default layout (X reload and action, Y switch,
B melee, A jump, left stick click crouch), which every bot's profile is
given whatever the human uses (`guest::bot_block`, §4.2).

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
A consumed hold suppresses that release. Normal and above
seek two SMGs, plasma rifle plus SMG, plasma pistol plus SMG, or two
needlers. In combat they take a pair only within pickup distance and
inside both guns' effective bands. Either hand can charge the pistol.
Against a group, a target beyond the left gun's band, or a recently lost
target, the thrower first solves a safe grenade arc. It taps Y and waits
for the left slot to clear before using the grenade trigger.

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

### 3.7 Objectives (traced)

Stage 5. The bot reads the gametype, the objective objects and the
scenario's netgame flags (`crates/bots/src/objective.rs`), and a pure
decision (`objective::plan`) gives it a goal.

**Fields.** Traced offline from the XBE with `h2 disasm` and a reading of
its data section (static, no game run). `E` is the game-engine globals,
`G` the game globals, `V` the variant (`G+0x13c`), as in
docs/live/live-gamestate.md. Live telemetry reads none of these yet: it
names the gametype by `V+0x44` and the weapons by tag name.

| Field | Where | Type | Confidence | Evidence |
|---|---|---|---|---|
| game-engine globals | `[0x4e9ae8]` | pointer | confirmed | live-gamestate.md; `0x1574c0` zeroes `0x327` dwords of it |
| engine index | `E+0xc14` | `i32` | confirmed | `0x1574c0` copies `[G+0x180]` (`V+0x44`) there, and writes 0 when the engine has no object or the globals tag lacks its multiplayer block (`0x157578`) |
| engines | `0x55e4d0[index]` | pointer to an object whose `+0` is a vtable | confirmed | filled by `0x378500`: 1 CTF and 9 Assault share one (`[0x470a5c]`, vtable `0x459d18`); 2 Slayer `0x45c6d8`; 3 Oddball `0x45c7a8`; 4 KotH `0x45c878`; 7 Juggernaut `0x453d38`; 8 Territories `0x45c948` |
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

**How the bot reads them** (`objective::read`, from `perceive_as` once
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

A fault leaves `Perception::objective` `None`: the bot plays as in Slayer.

**Roles** (`objective::role_of`). Every bot sorts its living teammates
and itself by player index; its rank and the team's size give its role,
so all of a team's bots agree with nothing said between them. One alone
attacks; of two, the higher index defends; from three, a third (rounded
down) defend, the highest ranks. Humans count in the ranks: the bots do
not know who is a bot.

**The decision** (`objective::plan`, pure):

CTF uses this order.

1. it carries the enemy flag: **carry** it to our return point, else our
   flag's spawn, along the nav mesh;
2. an enemy carries our flag: **hunt** the carrier (defenders always;
   attackers when the carrier is nearer than the flag they are after);
3. our flag lies more than 2 world units from its spawn: **go** and touch
   it to return it (defenders always; attackers within 25);
4. attackers: **go** for the enemy flag where it lies (at home or
   dropped). With it taken by a teammate, the remaining players divide
   escort and guard duties. A lone teammate escorts within 3 world units.
5. defenders, and attackers with nothing to do: **hold** our flag's spawn,
   within 4 world units, fighting from there.

Oddball: carrying, **keep** it; a free ball, **go** for it; an enemy's,
**hunt** them; a teammate's, **escort** them. Between balls, approach
the closest neutral type-4 spawn selected by Oddball init.

**King of the Hill.** Approach the current corners' centre, hold the spot
where the engine counts the bot inside, and fight from there. Moving
the hill rebuilds its corners. The bot follows the next scoring mask.

**Assault.** Take a friendly or neutral bomb to an active enemy plate.
In one-bomb, only the designated defender's plates score. Carrying the
bomb on foot inside the arming volume advances the counter toward
`G+0x244` seconds times tick rate. Outside, it falls by three per tick
to a floor of -1. Stay on the plate through arming; when armed, close to
score. The engine releases the planted bomb, records the planter at
`A+0x17c+4·bombTeam` and sets the planted bit. Escorts cover that bomb.
Defenders guard the plate or intercept the enemy carrier. An ordinary
dropped friendly bomb can be recovered by the nearest teammate.

**Territories.** Relieve a friendly capture site occupied by an enemy,
finish a capture already under way, then take an unowned or hostile site.
Break equal priorities by distance. The bot holds its position inside
the chosen volume while fighting. FFA ownership follows the player slot.

**Juggernaut.** Hunters engage the visible Juggernaut and ignore fellow
hunters. On a transfer, the new Juggernaut can rush a visible hunter
while firing, then melee up close. Movement bonuses come from the game
variant. The role mask alone reveals no target position to the planner.

**Picking up.** Approach the chosen item and hold X while it remains
loose within pickup range. `0x1517ec..0x1518d6` gives a nonzero
multiplayer weapon type the contextual interaction kind 7. The held-X
path accepts kinds 1 and 7 at `0x15195c` and requests primary pickup.
There is also a proximity path (`0x15098f`) whose primary pickup is
gated by inventory at `0x150dd0`. In Assault the pickup predicate
`0x2415d3..0x241611` rejects a planted bomb or a non-neutral bomb
belonging to another team. A bot returning its own CTF flag just
touches it. Once possession changes the carrier releases X.

### 3.8 Glitches (built, not checked in a game)

The user asked: "add setting so that clankers can do glitches (super
bounce, sword fly, etc) if turned on". The Clankers page has a
**Glitches** row (`clankers.glitches`): **Off** (the default), **Some**
(the occasional show-off) and **Lots** (whenever there is an opening).

A glitch is the same button presses a player makes, with the same timing,
on the bot's pad. Nothing in the engine is changed or called. The tricks
work only because the recomp runs retail 1.0's code: no title update has
patched sword flying or the super bounce out (title-updates.md). **Not one
has been seen working in a game yet.** Each needs the checklist below.

**The code.** `bots::glitches` is pure: each trick is a list of timed
presses (`glitches::plan`, a `Macro` of `Beat`s), the rules for when one
fits (`glitches::opening`) and the show-off clock (`glitches::ShowOff`).
The fight tricks run in the noob combo's scheduler
(`weapons::Combo::step`, its `ComboIn::glitch` gate); the show-offs run
in the mind's roaming (`Mind::step`, `Stunt`). The setting follows the
Taunts path: store, `clankers::Want::read`, the driver's shared
`AtomicU8`, then `Mind::glitches` at the bot's next frame, local bots and
phantoms alike.

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
| Super bounce | at a measured spot: stand still, A, hands off but a light push toward +y, land; again from a nudged place | roaming, a spot within 60 units | superbounce.md (traced; the spot's table is `glitches::spots`) |

The 5 ticks from B to the melee landing (`MELEE_CONNECT`) and the 4 for
the battle rifle's burst (`BURST`) are not traced. They are starting
points to tune in a game run. The battle rifle is told from the tag: once
per pull, one zoom level, a magazine of 30 or more.

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

**How often.** A fight trick is rolled for at most once a second while
one fits: 15% a roll on Some, 60% on Lots (times the skill's share), and
not again for 2 s after one. A show-off comes every 45 to 90 s of calm on
Some and every 8 to 20 s on Lots (longer at lower skill). A super bounce
gets 3 tries on Some and 8 on Lots, then the bot gives the spot up.

**Never:**

- with the setting Off;
- carrying the flag, the bomb or the ball, or with an objective to play
  (show-offs);
- when it would obviously lose: shields down (recovering), withdrawing,
  or another enemy within 8 units; a sword fly never at a target with
  another enemy within 2.5;
- before its reaction to the target has passed;
- doing anything but fighting or roaming on foot (`mind::glitch_doing`,
  an allowlist: getting in, driving or riding a vehicle is never on it).
  The vehicle work's seat pad returns before the fight and roam code, so
  it must call `Mind::drop_glitches` first;
- a show-off with anyone in view, or within 1.5 s of seeing someone.

While a trick runs the mind's own melee, reload, switch and grenades
wait, and the trigger is the trick's.

**The super bounce spots** (`glitches::spots::SPOTS`) hold only measured
spots. Each spot has a map, the place to stand, a spread to nudge within
between tries, a drift direction and a source. There is one:

| Map | Stand at (x, y, z) | Drift | Source |
|---|---|---|---|
| `ascension` | (−10.62, 3.88, −3.19), nudged up to 0.04 | +y | superbounce.md: the crease where the ramp meets the floor, 5 bounces in 20 standing-jump landings; the launch is a pinch between faces at y 4.12 and 3.94 |

The bot walks to 0.8 units short of the spot (lower y), steps onto it,
stands still for 4 ticks, jumps with the stick let go, and pushes the
stick 0.15 toward +y in the air, because every traced bounce came in
moving slowly toward +y. Not every landing bounces, so it tries again from
a nudged place. It knows it bounced when it rises 0.4 units again after
starting to fall: a plain landing never does. Community guides
([Halopedia, Super bouncing](https://www.halopedia.org/Super_Bouncing))
give spots on most maps by landmark only, with a forced crouch first.
superbounce.md found the crouch is not needed. A spot goes in the table
only once a game run has found and traced it.

**Not built:**

- **Butterfly**: a two-player trick (one jumps on the other's head while
  the bottom player, sword in hand, presses X and B), not one bot's
  presses.
- **BXL, LX, RRBX**: less well documented.
- Super bounces off moving vehicles (Burial Mounds' Warthog launch).

One community note matters here. The Bungie.net post says BXR "works as
a reload glitch from any range, but only off host". Clankers play on the
host. Whether the cancels work for a host-side player is the first thing
to check.

**Tests.** `cargo test -p bots glitches` checks the requested timing,
the openings and setting, the show-off clock and super-bounce state
machine. `cargo test -p bots human_input -- --nocapture` checks the
delivered timing, delay, seeded replay and controller limits.
`cargo test -p boot clankers` covers the setting and driver.

**Game run (checklist).** For the user. Use one Clanker against you on
Legendary with Glitches on Lots. The agent API reads what happened:
`world.object player.N physics` (havok_pos, havok_velocity, airborne,
crouch), `havok.contacts {object: player.N}` and `trace.start player.N`
for a row every tick ([agent-api.md](../runtime/agent-api.md)). Each line
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

### 3.9 Vehicles

The user, after playing the Clankers: "clankers also don't seem to
interact with vehicles, try and board, go and take, lock on with
rockets." The vehicle code lives in `crates/bots/src/vehicle.rs`, `vehicle/` and
`mind/ride.rs`. The bot sees each seat and its rider. It can enter a free
vehicle, board a slow enemy, pilot a Banshee, drive ground vehicles and
man a turret. A rocket launcher holds its lock on a vehicle before firing.

**Fields.** `O` is an object's body, `U` a unit's, `V` a `vehi` tag's
metadata and `S` one of its seat definitions.

| Field | Where | Type | Confidence | From |
|---|---|---|---|---|
| parent: a rider's vehicle, a mounted turret's vehicle | `O+0x14` | datum | confirmed | live-gamestate.md; `engine::view::objects::Body::parent`; `live-telemetry` `object_in` (riders) and `vehicle_mount` (turrets) |
| position, forward, up | `O+0x64`, `+0x70`, `+0x7c` | `f32[3]` each | confirmed; relative to the parent when there is one | `engine::view::objects::Body`; live-telemetry.md (a mounted turret's `+0x64` is relative to its vehicle). A rider's own position is not used: as telemetry does, a rider is where its vehicle is |
| body vitality | `O+0xec` | `f32`, 0 to 1 | confirmed | `object_get_health` `0x2a1a20` (live-telemetry `layout::BODY_VITALITY`) |
| wrecked | `O+0x10a` bit 2 | `u16` | confirmed | `0xd6800` (`guest::BODY_DEPLETED`) |
| the unit's seat | `U+0x1fc` | `i16`, -1 none | confirmed | live-telemetry `layout::UNIT_SEAT`: the enter-vehicle routine `0xea1f0` stores it (`0xea405`), leaving stores -1 (`0xea995`) |
| seats block | `V+0x1c8` count, `+0x1cc` pointer | `i32`, `u32` | confirmed | `crates/tags/src/defs/unit.txt` (`group vehi`, `block_1c8`, `0xb0` each; the seat label at `+4` checked against 05a's scripts, `h1scripts::seats`); the stride is `0xea1f0`'s `imul edi,0xb0` (`0xea330`) |
| seat flags | `S+0` | `u32` | traced and disc-checked | Driver bit 2, gunner bit 3, boarding bit 11, forbids players bit 16. See `h1objects::rebuild` and `h1objects/tests/vehicle_rebuild_disc.rs` (`0x000cc810`, `0x000c8c87`, `0x000c92c0`). The first unflagged passenger is used as fallback driver **[guess]**. |
| boarding target seat | `S+0x3e` | `i16` | traced | `0x000c92c0`, `h1objects::rebuild::SEAT_BOARDING` |
| entry marker and admission limits | `S+0xc`, `+0x98`, `+0x9c`, `+0xa0`, `+0xa4` | string ID, four `f32`s | traced and disc-checked | `0x000c6fb0` resolves the marker, with the object's origin for an empty ID. `0x000c9040` checks eye-to-marker radius, marker facing, look cone and distance to the vehicle. See `h1objects/tests/vehicle_ground_disc.rs`. |
| render-model reference and marker data | `V+0x38` → `hlmt+4` → `mode` | tag datums | traced and disc-checked | `defs/obje.txt` and `defs/hlmt.txt`. In `defs/mode.txt`, `mode+0x58` is the `0xc`-byte group block (ID `+0`, markers at `+4`); the `0x24`-byte marker holds variant indices at `+0/+1`, a node at `+2`, translation `+4` and quaternion `+0x10`. |
| live marker transform | `O+0x114/+0x116` | block size and signed relative offset | traced | [native-renderer.md](../graphics/native-renderer.md#research-points-code-unless-marked-h2-disasm). Stride `0x34`; matrix scale `+0`, forward `+4`, left `+0x10`, up `+0x1c`, position `+0x28`. |
| chassis dimensions | `mode+0x14` compression block | position bounds | disc-checked | `defs/mode.txt`; each record begins with six floats giving xyz min/max. The planner uses lateral extent and vertical span, with margin and class fallbacks **[guess]**. |
| vehicle type | `V+0x1f0` | `u16` | traced and disc-checked | `0xef545` compares it with 5 and 6. `h1objects::rebuild::H1_VEHICLE_TYPE` and its disc test compare the classes by vehicle (tank 0, jeep 1, scout 4, fighter 5, turret 6). |
| fighter secondary weapon | `V+0x1c0`, `weap+0x2c8/+0x2d0` | weapon references, trigger and barrel blocks | traced and disc-checked | `defs/unit.txt` and `bots::arms`. The retail Ascension Banshee weapon has two triggers and barrels. Barrel 1's projectile is `banshee_bomb`. The tag establishes availability in the weapon definition. The game decides whether the variant will fire it. |
| the gun mounted on a Warthog | a vehicle object whose parent is the Warthog; its gunner's parent is the gun | | confirmed | live-telemetry.md (`parent` on a frame vehicle); `live-telemetry` `vehicle_mount` |
| a rocket launcher | `weap` zoom levels `+0x1fe` 1, and the blast role (§3.6) | | checked on Zanzibar's tags (zoom) | §3.6's handling table: the rocket launcher is the only blast weapon with a zoom level |

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

`guest::perceive_as` now puts a rider where its vehicle is (its own
`+0x64` is relative to it): a Warthog's driver and an enemy on its gun are
seen at the Warthog and at the gun, not near the world's origin.

**Lock-on** (`vehicle::lock`). With a rocket launcher in hand
(`lock::locks_on`) and the target riding a vehicle, the bot aims at the
vehicle's middle (0.3 over its origin) with no lead (a locked rocket
follows it), and fires only once it has held the vehicle in the reticle
(the vehicle's apparent size, 0.8 world units, plus the skill's fire
cone) for the lock time. At infantry nothing changes: fired unlocked, at
the feet for Heroic and Legendary (§3.4). The game's own lock time is not
traced; the bot assumes 1 s (`lock::LOCK_TICKS`) and adds its margin:

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| waits for the lock | no (fires as at infantry) | yes | yes | yes |
| holds past 1 s | | 0.4 s | 0.2 s | 0.07 s |
| off the vehicle forgiven | | 3 ticks | 6 ticks | 8 ticks |

The bot does not zoom: Halo 2's launcher is taken to lock without it
(§12). "A blast weapon with a zoom" stands for the rocket launcher; the
fuel rod's zoom was not looked at (it is not on Zanzibar). If it has one,
a bot with it waits a second at a vehicle and then fires: slower, not
wrong.

**Which vehicles.** By class (`vehicle::Class::spec`).

| Class | Seat taken | Controller behavior |
|---|---|---|
| Jeep (Warthog) | Driver | Ground routing |
| Scout (Ghost, Spectre, Wraith) | Driver | Ground routing and primary fire |
| Tank (Scorpion) | Driver | Ground routing and primary fire |
| Fighter (Banshee) | Driver | Altitude control, attack runs, boost, evasive rolls and landing |
| Turret | Gunner, standalone or on a teammate's vehicle | Track and fire into the contested area, dismount if flanked |
| Boat or plane | None | |

The bot looks up the chosen seat's entry markers in the actual vehicle's
render model and places them using the live node matrices. Scorpion and
Wraith driver seats can offer two positions. It searches for a stand point
on the floor within the tag's radius and facing limits, looks at the marker
and holds X while in range of the seat. The stand-point offsets and the
0.6 world-unit eye height used in this check are **[guess]**. A retry
uses a closer point covered by the same entry data.

**Go and take** (`vehicle::plan::take`, `Mind::mount_plan`). Only calm
(no enemy in view for 1.5 s), not carrying, not going for a weapon, and
in an objective game only when the objective's point is more than 20
away. The nearest vehicle by distance (plus 3 per unit of height) within
the skill's range that:

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

**Turrets.** A standalone turret or a free gun on a nearby teammate's
vehicle is worth taking when it has a clear line to recent enemy contact
or the objective the bot is defending or hunting. On the gun, an enemy
within 1.8, or within 5 and off the firing line on the same floor, can
make the bot dismount **[guess]**.

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| takes a vehicle within | 12 | 20 | 28 | 35 |
| mans a teammate's gun | no | yes | yes | yes |
| boards within (XY) | never | 1.5 | 2.2 | 2.8 |
| boards going slower than (units a second) | | 1.5 | 3 | 4.5 |
| runs infantry over | no | yes | yes | yes |

**Board** (`vehicle::plan::board`). An enemy (in free-for-all, anyone)
driving a vehicle (not a Banshee) on the bot's level (within 1 in height),
within the skill's boarding range, slower than its boarding speed: in a
fight or not, the bot approaches an authored boarding marker targeting
the occupied driver's seat and holds X when it meets the admission checks.
It lets go when the vehicle speeds off or after 3 s. Speeds come from the vehicles' positions tick to tick
(`vehicle::Memory`), not from `O+0x88`, whose units are not checked.

**Drive** (`vehicle::drive`, `Mind::ride`). Halo 2 steers a Ghost or a
Warthog by the camera: the left stick says where to go relative to where
the player looks. The driver:

- goes to the objective's point, else the enemy it fights, else where an
  enemy was last seen, else a roaming goal (§3.2);
- plans over walk edges whose portals fit the vehicle and routes through
  their clear spans. Centre and wheel-line probes check floor, overhead
  room and walls before steering, cutting a corner, reversing or trying
  to splatter someone. With an objective on an unreachable road it
  dismounts to finish on foot;
- looks at the carrot (or the enemy it fights) and pushes the stick that
  way, full ahead when the camera is on it, down to 0.35 at a turn of 1.6
  radians or more;
- with infantry in front within 15, drives at them (splatter), the skill
  allowing;
- shoots with the vehicle's guns when its seat has them (a Ghost's, a
  Scorpion's; not a Warthog driver's);
- under throttle and slower than 0.4 a second for 1.5 s, backs up for 1 s;
  the third time within 15 s it is stuck for good.

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

A gunner aims and shoots (the gun's trigger held in the skill's bursts); a
passenger shoots its own gun as on foot. With nothing to shoot they look
where the enemy was last seen, else ahead.

**Getting out** (`drive::leave`): the vehicle under 0.2, lying flipped
for 1 s, stuck for good (driver), nobody driving for 4 s (gunner,
passenger), or within 3 of an objective point it must reach on foot (the
flag or ball it is going for). It holds X 15 ticks in every 20, so a press
and a hold both work, and stops driving. The vehicle it left is left alone
for 10 s, so it does not get straight back in.

**Seeing it in a game.** Two or three Clankers on a map with vehicles
(Coagulation, Headlong, Waterworks, Zanzibar), Slayer, Normal or
Legendary (`H2_BOTS_SKILL=legendary`). §11.4 is the checklist.

**Open** (§12): local and phantom observations of each seat's entry and
exit, the lock time and zoom requirement, and measured lift and flight
behavior. A Warthog driver waiting for its gunner remains open.

## 4. Way in 1: local slots

### 4.1 The bot's pad (traced, Repo)

A bot's controller is a pad on a free port, not a change to the agent tap.
`input::InputManager::sync` (`crates/input/src/device.rs`) polls every
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

### 4.2 Signing a bot in (traced)

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

### 4.3 Hiding the views (traced)

These read the local-user map `[0x4e8c20]+0xc` or the count `+8`:

| Reader | What for | Bot window |
|---|---|---|
| `render_game_views` `0x2234e0` (`0x223524`, `0x22359e`) | one view per user, layout from the count | **yes** |
| HUD, per view (`0x1e6fe0`: user to player via the map, from the view's draw) | that view's HUD | covered by the render window |
| sound frame service `0x125b40`, listener loop `0x125c93`..`0x125cd7` | one listener per mapped user (`0x221a70(user, listener)`) | **yes**, so only the human's ears count |
| `SOUND_LISTENERS_UPDATE` `0x127400` (also from `0x18c060`, outside the sound frame) | one listener per mapped user, marked active | **yes**, and the bots' listeners are marked inactive |
| `SOUND_OWNER_USER` `0x18d4f0` | a starting sound on a mapped user's unit plays unpositioned, in the ears | **yes**, so a bot's sounds are world sounds |
| `0x1870a0` | actions | **no**: the bot must act |
| `director_update` `0x1554b0` (`0x155939`, `0x155caf`) | one camera per user | no: harmless, small CPU |
| about 70 others (game engine, UI, scoreboard; the full list is in the scratch trace) | | no |

**The window:** at the entry of `0x2234e0` and of `0x125b40`, the driver
does three things:

1. It saves the count and the map.
2. It sets each bot user's map entry to -1, and lowers the count.
3. It puts the saved values back at exit.

The order of the map entries for the human is kept. With the pipelined
render, the render window runs against the render's own snapshot. Its write
and its restore cancel out, so nothing reaches the game thread. The
pipeline's hook plan has to list the window as a render-thread hook (its
boot test checks that every hook is listed).

**Listeners (traced, 2026-11-19).** The listener update `0x127400`
(`SOUND_LISTENERS_UPDATE`, stdcall, one argument) walks the four local
users: for each mapped one it sets the user's listener from its camera
and marks it active (the byte at `[0x4e6380] + 0x8e + 0x48 * user`); for
an unmapped one it writes the record's player fields to -1 and leaves the
active byte alone (`0x127b13`). It has two callers: `SOUND_UPDATE`
(`0x125c1e`, inside the window above) and the per-frame update `0x18c060`
(`0x18c07f`, from `0x137ef8`), which runs it **outside** the window when
the sound globals' bytes `+0x78`, `+0x79` and `+0x7a` are set. So each
bot's user was a live listener after all, at the bot's own camera.
`0x127d00`, which picks the listener a starting sound is for (the
closest active one; a sound with no position goes to the first mapped
user), then took a bot's own sounds (its reloads, jumps, landings,
melee, pain) as heard by the bot, right at its ears. Those are the
sounds the controller speaker routes (docs/audio/playstation-audio.md),
and the user heard the bots' through the controller.

- **Design:** the listener update gets the same window as the sound
  frame (`bot_glue`, before and after `SOUND_LISTENERS_UPDATE`), and at
  both points each hidden user's listener is marked inactive
  (`guest::quiet_listeners`): the update never clears the byte for a
  user it skips, so a listener made before its user was hidden would
  stay active where the bot last stood. Called from inside
  `SOUND_UPDATE`, the window finds no bot user mapped and does nothing.
- That a bot's own sound played as a plain (not 3D) voice is traced
  now: see "Whose sound" below. The listener was not the whole story.

**Whose sound (traced, 2026-11-19).** The user's run after the
listeners went quiet: "i can hear sound from every ai player, also,
sound doesn't get quieter in distance or round corners". A sound that
starts on an object asks `0x18d4f0` (`SOUND_OWNER_USER`, stdcall,
`ret 12`: object, a pointer to the sound's kind, a pointer for the user
or null) whose own sound it is. When the kind is 1 (positioned) it walks
the local-user map `[0x4e8c20] + 0xc`; for the first mapped user in
first person (`0x155760` gives 0 or 1) whose player's unit (`+0x2c`) is
the object or carries it (`0xbafb0`, both ways) it sets the kind to 0
and the user. Kind 0 has no position: `0x127d00` gives it to the first
mapped user, `0x127e20` gives it distance 0, and it plays in the ears
with no distance, no panning and no obstruction, as a player's own
gun, footsteps and breathing do. Its callers (`0x1890e5`, `0x18935f`,
`0x189441`, `0x18cf76` from the per-frame update `0x18c060`, and
`0x18d4dc`) run from the game tick and the per-frame update, not inside
`SOUND_UPDATE`'s window, so the bots were mapped: every bot's gun, footsteps, grenades, melee and voice
started as kind 0 and, given to the first mapped user, the human, played
at full level in the human's ears wherever the bot was. That is both
halves of the report: every bot is heard, and those sounds never fall
off with distance or walls (the human's own world sounds always did;
they are the game's own mix-bin volumes, docs/audio/dsound.md).
`0x189441`'s caller even checks the user's listener (`+0x8e`) and
falls back to the world path when it is inactive; the other four do
not, so quieting the listeners could not fix it.

- **Design:** `SOUND_OWNER_USER` gets the hide window (before: the bots'
  users out of the map, `guest::hide`; after: `guest::restore`), as the
  views and listeners have. The walk then finds only the humans: a
  bot's sound keeps kind 1 and starts as a world sound at the bot,
  heard (or culled) by the human's listener as a remote player's would
  be, with the game's distance, panning, cluster audibility
  (`0x221da0`) and obstruction (`0x221a70`). The human's own sounds are
  unchanged.
- The DirectSound listener is not the game's ears: `0x127400` ends by
  setting it (`0x220790`, `IDirectSound_SetPosition` at `0x2207ba`,
  `DS3D_DEFERRED`) to the origin facing +x (`[0x468788]`,
  `[0x4687a8]`), and the game places each 3D mix-in relative to the
  listener its sound belongs to. So nothing in the DirectSound layer
  knows which local user a sound is for; the choice is made here.

**The controller speaker, haptics and rumble (Repo).** Nothing there
goes by port:

- The controller speaker (`dsound::padroute`, `boot::pad_speaker`)
  routes by **sound tag name**, never by player: one DualSense or DS4
  speaker, chosen by device name, gets every routed sound, and the Steam
  Controller's trackpads (`input::steam_pad`, "as haptics") play the
  routed tone on every connected controller. There is no port-to-pad
  map for sound. So a bot's sounds reached the pad only by being started
  as a local player's (above); with the bots' listeners gone they start
  as anyone else's sounds in the world.
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

## 5. Way in 2: phantom peers

### 5.1 What has to be made up (traced)

A phantom is a remote machine with one user, made in memory on the host by
driving the game's own membership code. Nothing goes on the wire: no fake
XNet peer and no fake join messages.

**The game builds machines and players from the session membership**, not
from the update record. In a distributed game the record (with its
`+0xDD0` player-set change) is never sent: its encoder `0x87d00` is only
the synchronous-update message. Every console builds its own record from its
own copy of the session, and clients learn of a phantom through the session
membership the host replicates. So a phantom is a session peer and a session
player, and the game does the rest.

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
3. **Configuration**: the host's own player's (`+0xd4`, `0x90` bytes)
   with the phantom's name (UTF-16 at `+0`, up to 15) and its team at
   `+0x7c` when it has one.
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
  effective one once set (made unique by `0x7f660` the first time), the
  look (`+0x40..+0x7b`, `+0x7d..+0x81`) is the requested one, and the team
  (`+0x7c`) is the requested one clamped to 0..7 in a team game
  (`variant +0x48` bit 0) or the player's slot in free for all, when
  `[S+0x14]` is 1; otherwise the effective team and handicap stay. So a
  phantom's requested team is what the lobby shows; both are written.
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
  with the configuration from this console's first session player's
  requested one (`config_in`; no player datum exists before the first
  game), then the properties (`sync_props`): this console's own `0xc8`
  bytes with the phantom's gamertag in both names, `+0x24` 1, both
  counters + 1, kept equal every frame (a map load on the host is the
  phantom's too). The join line is `bots: phantom G joins (peer N, in the
  pregame lobby)`.
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
  /v1/games/bots` (`live_hle::client`, `Work::Bots`). The roster
  handler's only 404 is `no session of yours by that id` (the session
  looked up by id with the caller as its host, `live_server::bots`); it
  has no 409. So the server did not have session `8615d3735dc12a62` as
  the caller's when asked, at once after it was advertised. Not a
  7-bot limit (the count is cut to the room, and none is a 400), and
  custom games ask without a playlist, which is allowed. The server's log
  line for the request says which: a model that had not applied the new
  session yet (the Workers home reads its own copy), or another account
  asking than the one that made it.
- **The local bots in the game.** `join gate port 1..3: screen 0xe7 ...
  signed_in true: waits` is the gate saying the bots are signed in
  already, so it has nothing to do: `waits` is every gate line out of a
  lobby. It is no sign they were left out of the game.

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
Clanker profiles use. [Controller layout trace](../input/controller-layouts.md)

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
Only the validated action crosses back to the host. A repeated gather in one tick reuses that action. Local bots keep
their normal retail controller polling. A phantom samples once per game
tick and advances the button clock by elapsed simulation milliseconds.

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

**Open, for a run**:

- **Clients and the phantom's address.** A real client opens a channel to
  every peer's XNADDR (`0x5cb80` → `0x76aa0`, `0x5d237`) and its drop loop
  (`0x5a090` → `0x5fda0`) runs in state 5, which is the running game too.
  So on a client a phantom may be dropped locally, in the lobby (where
  phantoms now are from the start) or in the game. If a run shows it, our network layer has to answer for phantom
  addresses on clients. The host is safe: its loop drops only connected
  peers.
- The veto at `[S+0x78a8]` vtable `+0xc` (the object at `0x527fe8`):
  its vtable is set at run time, so what it refuses is not read; phantoms
  never go through it ("Phantoms in the pregame lobby"). `0x5f970` is a
  channel's teardown, not a ready check, and `0x62e30` is the free-slot
  count, now honoured.
- The profile appearance starts at configuration `+0x40` (traced at
  `0x18fd94`). Phantoms set its colours and emblem from their full tag.

**Phantom ping.** Each phantom has a seeded 40–80 ms round-trip ping by
default. The host samples its pad at 30 Hz and queues the report for
one-way delivery after half that ping plus uniform jitter of up to ±4 ms.
Arrival is rounded up to the next simulation sample, normally one or two
ticks later. The host builds and stamps the action record at arrival
using the current weapon and facing. While it waits, it holds the latest
arrived pad. Several packets arriving together can collapse to the newest
report. A new unit or a break in the stream clears the queue.
`phantom_ping_ms` sets round-trip time, with 0 for immediate delivery.
`phantom_jitter_ms` sets the one-way jitter bound.

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

**Training coverage.** A sim match with a `kind = "phantom"` seat hosts
a private System Link game. Its brain, uplink, remote gather and retail
action builder run as they do on a normal host. Training waits for every
configured seat to bind to a player and unit before counting combat.
The paired Midship example switches transport for each named bot while
keeping its team, tuning, map and seed. The report compares shots,
opposing hits and damage dealt and taken per seat.
[Headless simulation](../runtime/sim.md#more-than-four)

### 5.2 Why phantoms are needed

- More than 3 bots, and up to the playlist's size (16).
- Bots that are separate accounts with their own name, emblem and rank
  (Clankermatch, persistent bots).
- A host whose own humans fill all 4 local ports. With all-phantom
  seating the first bot joins through the same remote gather as the last.
  A playerless sim host still takes one of the 16 machine slots, leaving
  room for 15 single-player phantom machines.

### 5.3 CPU per bot

Per bot, per tick:

- perception: about 10 to 20 rays, cheap against the BSP;
- decision, about 5 times a second: microseconds;
- path following: one A\* per re-plan, on a graph of 1 to 10 thousand
  sectors;
- aim: constant cost.

Our estimate is under 0.1 ms per bot per tick on the host's CPU. The game's
own cost for the bot's player is the same as for a human: one more biped
simulated. A local-slot bot also costs the per-user control update and the
director; its view is skipped. A phantom costs one more remote player's
distributed updates to each client. That is the real ceiling, and it is the
same as a full human lobby. Measure in a run (§11).

## 6. Shared and different

| | Local slot | Phantom peer |
|---|---|---|
| Brain (perception, nav, decision, aim, emotes) | shared | shared |
| Nav mesh tool and cache | shared | shared |
| Pad output | to the port's pad merge | one remote-slot action per simulation tick (§5.1) |
| Signing in | guest profile in a local slot | player configuration on a made-up peer |
| Views | hidden by the render window | none: not a local user |
| Network | none | the drop and keep-alive for phantom addresses |
| Most per host | 3 | 16 minus humans |
| Own identity and rank | a bot account with a roster (§9.5); else a guest of the host | yes (§8) |
| Works with remote humans | if the mode allows guests (Run) | yes: clients see a normal remote player |

**Real remote players** (System Link or Live custom games): bots run only on
the host. Clients see local-slot bots as the host's guests and phantoms as
players of a machine they never talk to directly. Both are normal to them,
because a distributed client takes every remote player from the host
anyway. Joining clients need no change at all. Our clients do know a bot
by the bot flag in its player configuration's name (§8.2), but nothing
depends on that.

If the host leaves, the bots go with it. Host migration is not a concern:
retail Halo 2 ends the game when the host leaves.

**Scoreboard and stats.** Bots are players, so the scoreboard, the carnage
report and the post-game stats treat them like anyone else. Telemetry
(`crates/live-telemetry`) records them as players, keyed by their game slot
like anyone, with `bot: true` on their roster entries (`game::Player::bot`):
a local-slot Clanker found by its controller port, a phantom by its machine
id (`live_telemetry::Bots`, from `clankers::Driver::bots_in_force`). Only a
bot from a server roster in force has an `xuid` (§9): a phantom's
rostered one, or the bot account a local bot is on Live as
(`Bots::accounts`, by port); the rest have no account at all.

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

## 7. Clankers in custom games (the Game Setup page)

Game Setup gets a **Clankers** row next to the Halo 1 Options row. It is
added the same way (`crate::h1_options_row` is the model). The row opens a
**Clankers** page.

The page is a native ui_kit page: a `ListPage` or a `Page` impl
(`crates/boot/src/ui_kit/page.rs`). That gives it the kit's preview, its
layout checks and the `Page` table tests. These are its rows:

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

**Mixed** gives each bot a difficulty drawn around Normal. Per-bot
difficulty can come later, as a sub-page per bot; the first page does not
have it.

The settings are kept in the settings store through `setting_defs`, like
`HALO1_SANDBOX`. They persist and appear in `h2 settings`. The page is a
user of the kit. If it needs anything the kit lacks, that goes to the
ui-builder agent, which owns `ui_kit`, through the coordinator. As far as
read, `OptionRow`'s two-choice rows need a many-choice sibling for Count
and Difficulty; that is the one kit ask expected.

**Joining the lobby.** When the host's pre-game lobby opens with
`clankers.enabled`, the bot driver adds the bots one by one: as local slots
up to 3 (stages 1 to 4), and the rest as phantoms, which join the lobby
this console hosts as peers (§5.1, "Phantoms in the pregame lobby"), up to
Count 15 (the session's most players permitting). With
`H2_BOTS_ALL_PHANTOM=1` the driver starts with the first phantom and
claims no ports. The host and clients see each bot's name and team colour
in the lobby roster. Changing the page in the lobby adds or removes
bots.

## 8. Clankermatch

### 8.1 What the player sees

**Clankermatch** is a new Xbox Live menu row, after OptiMatch, that plays
exactly like OptiMatch:

1. Pick a playlist.
2. The search runs, with the retail screen. A plausible delay: 4 to 20 s,
   drawn from the playlist's population.
3. The lobby fills with bots one by one, each with a gamertag, an emblem
   and a rank. Teams are balanced.
4. The match runs as a normal Live matchmade game: the same hopper
   playlists and variants, and the same carnage report.
5. Leaving ends the game.

The player's own console is the host. The bots are phantom peers, up to the
playlist's size, so Clankermatch needs stage 6.

### 8.2 Decided by the user

1. **Stats.** A Clankermatch game is its own kind: flagged in the events
   (`GameRecorded` carries `kind: "clankermatch"`) and in the read model,
   so it can be split out later. For now it counts in career totals, rank
   and leaderboards together with human games.
2. **Bot identities.** Bots get made-up gamertags, marked by a glyph that
   says "bot". We pick **U+2699 GEAR** (⚙). Common system fonts on the web carry
   it, and it is in the Basic Multilingual Plane, as Halo 2's UTF-16 names
   require (Repo: keyboard-charset.md). The game's fonts lack it; the
   keyboard's dingbats (star, flag, check, ballot x) are the only symbols
   close to it, and none of them reads as "bot". So we draw our own glyph
   and add it with `font::patch` (Repo: `crates/font/src/patch.rs`, already
   used by `boot::icons` to add our icon glyphs at load). No disc glyph is
   copied.

   The gamertag itself stays inside the Live rules: ASCII letters, digits
   and single spaces, 1 to 15 characters, as `live_proto::Gamertag` checks.
   The glyph is not part of the account name. It is drawn before the name
   wherever a bot's name appears:
   - in the game, the host writes `⚙ ` + tag into the player configuration's
     UTF-16 name (player `+0x44`), which is what the lobby, scoreboard and
     carnage report draw. A phantom gets it in its configuration; a local
     bot (whose name comes from its profile) gets it in its session
     player's effective configuration, on the session's host, each frame
     a bot port's player lacks it (`session::mark_local_bots`, from the
     bot glue: `bots: the bot mark on local bots' names (session players
     [1, 2, 3])`). Only the effective name is written: the host's pass
     (`0x65cb0`) keeps an effective name once set and takes the rest from
     the requested configuration, so the profile cannot undo it, and the
     clients and the game's player take the effective one. Before this
     (the user's run, 2026-11-19) local Clankers had no mark;
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
   character. Every Clanker name the host makes ("Clanker 12") fits
   whole. The game record names a bot without the mark
   (`live_proto::bots::unmarked_units`, in `live-telemetry`'s player
   read), so the stats read model keeps the account's gamertag and the
   site draws its own mark from the bot flag. No player can type U+2699,
   so only a bot's name loses anything.
3. **Bot rank.** Bots are matched to the player's rank. The playlist sets
   bot skill from the human's level, so it is a fair fight. The level maps
   to the aim table's difficulty (§3.4) on a continuous scale, not just its
   four steps: reaction, error and lead are interpolated.

### 8.3 Menu row and playlists (traced, Repo; built)

**The row.** The Live menu's extra rows come from `custom_games.rs`'s
hooks: list count, add items, pane, row text `0x230f36` and row chosen
`0x230f92`. Clankermatch is row 2, right after OptiMatch; the rest move
down one:

| Row | What | Press |
|---|---|---|
| 0 | Quickmatch | retail |
| 1 | OptiMatch | retail |
| 2 | **Clankermatch** (ours, pane 4) | retail row 1's press, `clankermatch::active` set |
| 3 | Co-op Campaign (ours, pane 6; with Live), directly above Custom Game | co-op |
| 4 | Custom Game (retail row 2, moved) | retail row 2's press; text `0x0b000421`, pane 2 |
| 5 | Custom Game Browser (ours, pane 5) | the Live browser |
| 6 | Content Download (retail row 3, moved) | Mods screen or the Map Pack dialog |

Without the co-op row (`H2_NO_COOP_ROW=1`) the rows after Clankermatch close
up: Custom Game 3, the browser 4, Content Download 5
(`custom_games::row_kind_with`). `h2 bots test` drives Custom Game at row 4.

A moved retail row's press rewrites the press record's row (`[[esp+8]]`)
to its retail row and runs the original, as the co-op row does; after it
returns, the cursor row `0x510a18` is set to the row shown (the retail
press stored the retail row there). The browser's and co-op's presses
store their rows too (`custom_games::remember_row`, rows 0 to 5). The
menu restores its cursor only to rows 0 to 3 (`0x231195`: `cmp eax,4;
jge`), so for rows 4 and 5 a `before` on the menu's build `0x00231170`
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
`0x00231066`, which fails for -1 so nothing starts. A playlist press
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
clanker hoppers` at the press.

### 8.4 Search and lobby (built: the empty search)

The game searches through `XOnlineMatchSearch`, which is ours in live-hle.
For a clanker hopper (`live_hle::clankermatch`):

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
   playlist, no skill, so the server sets skill from the human's level,
   §8.2.3; `teams: true` in a team playlist) and seats the bots one at a
   time on `clankermatch::schedule` (each 0.5 to 3 s after the last,
   seeded by the session id), up to the playlist's most players less one
   human. **Phantoms only** (since 2026-11-19, below): every bot is a
   phantom with the roster's bot account and skill, joining the match's
   target session as a peer (§5.1, "Phantoms in the pregame lobby"; with
   `H2_PHANTOM_LOBBY=0`, once the game runs), as the players a search
   finds are. `clankers::MATCH_LOCAL` is 0: no bot takes a controller
   port. (Phase 2 put the first three on ports 1 to 3 as local players;
   they showed in the human's party and failed the matchmade start's
   controller check.) Nobody is seated before the roster
   comes (no host-made bot for it to replace, no local bot on Live as the
   human's guest first); if Live refuses it, host-made bots.
   In a team playlist each phantom without a roster team goes to the team
   with the fewest players, then the lowest total skill
   (`clankermatch::balance`), around the human: the host's first
   local listed player's configuration team (player `+0xd4 + 0x7c`,
   `bots::phantom::session::host_team`), read twice a second by the bots'
   frame hook (`clankers::frame`) and followed until the first bot sits,
   then kept so no bot changes team; 0 when unknown or not one of the
   playlist's teams. The frame hook now runs this with no bot in (it used
   to return first, so a Clankermatch with the Clankers page off never
   started its fill).
3. The match starts on the hopper's normal countdown, now that the local
   bots make the lobby's player count (below, "Tracing the matchmaking
   lobby"). A handoff (the lobby's session deleted and the game's
   advertised) keeps the bots and carries the roster (§9.5); with no
   hosted session for 30 s, or another kind of session hosted, the fill
   ends and the bots go: the phantoms at once, the local bots signed out
   by the bot glue's `SLOT_SIGN_OUT` once no game runs, as Clankers Off
   does.
4. The game record says `kind: "clankermatch"` with the playlist and
   session (`telemetry::Playlist::clankers`), and the server keeps it as
   its own kind (live-matchmaking.md §6.8).

The lobby's "searching for players" state is the game's own: it stays until
the session is full.

**Why no bots show (the user's run, 2026-11-19: "no bots on Clanker
Slayer").** Read from the code, not a log yet. The fill seats its bots on
the phantom books (`bots::phantom::Phantoms::add`), and a phantom enters
the session only once a multiplayer game runs: `bots::phantom::session::
host_session` answers `NotNow::Wait` while there is no game tick, and
§5.1's recipe runs only in a running game ("phantoms leave the session
when the game ends, so they are not in the lobby yet"). In a custom game
that is fine: the human presses Start and the phantoms join the game. A
matchmaking lobby has no Start: the game starts it on the hopper's
countdown once it has its players, and with one human and no phantom in
the session it never does (Clanker Slayer's fewest players is 2). So the
fill runs, its `(phantom, rostered) on` lines come, and the lobby still
shows the human alone. **Fixed (phase 2):** the fill seats local bots
first (item 2 above), which sign in to the lobby as session players, so
the lobby reaches the playlist's fewest players and the game starts it;
the phantoms join once it runs. The fill's first line says how many of
each (`clankermatch: 3 local bots join the lobby on free controller
ports; 4 phantoms join when the game starts`).

Nothing else on the path is gated on what the user might have set:

- `clankers.enabled` off does not stop it: `clankers::frame` runs
  `tick_match` every 15 frames from the bots' `RENDER_FRAME` hook whatever
  the page says, and the fill seats `Fill::bots` with `Want::OFF`.
- The hopper ids are descriptor indices 0..15 everywhere (the marks, the
  `HOPPER_LISTED` filter, the session's attribute 1, the search's hopper),
  so the admin list's 1-based 9, 10, 11 are hoppers 8, 9, 10. A
  published file that moved a clanker playlist loses its mark
  (`ClankerHoppers::of_defaults`), which would empty the Clankermatch
  list rather than misfile it; the user saw the list, so the marks hold.
- A refused roster seats host-made bots; it never blocks the fill.
- The Clankers Off fix (`SLOT_SIGN_OUT`) touches local bots only, which
  a Clankermatch never seats; the Live menu's row move keeps
  `custom_games::Row::Clankermatch` at row 2.

**Tracing the matchmaking lobby (2026-11-19, from the disassembly; no
run yet).**

- **The lobby is a pregame lobby.** A playlist press (`0x002b2149` →
  `0x002b21b9`) creates the squad session as the Live custom game row
  does (`0x59890(2, 0, 0)`), then sets lobby mode 3 (`0x199a03(3)`) and
  `0x63fb0`. The start state (`0x19a2ce`) tests "a pregame session"
  (`0x19a279` gives 3 for life-cycle types 1 and 2) **before** its
  matchmaking branch (`0x199971`: lobby kind 6, session `+0x49a4` 2), so
  a matchmaking lobby passes it: `bots::lobby::pregame` holds there and
  the bot glue's lobby gate signs local bots in, as in a custom game.
  **Wrong for the lobby the bots fill** (cm-run2, below): that holds for
  the party's lobby before the search; once the search hosts, the life
  cycle is 6 and the lobby is the target session.
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
  `0x7067a` and `0x724a6` do for the matchmaking states. So **session
  players are what count**: local bots (on Live as bot accounts, never an
  offline id) count, phantoms (absent until the game runs) do not.
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
- **To see in a run:** whether a local bot signing in while this console
  hosts restarts the search (the party changed) or just fills the lobby;
  whether the game balances a matchmade team game itself at the start
  (then the phantoms' teams are its, not `clankermatch::balance`'s).

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
  timeout, and the start wait (`+0x5a4`, 0 at the playlist's most
  players) starts it. Nothing on our side abandons the session, so
  nothing is changed for it; a run that still re-searches with the lobby
  full says the start is held elsewhere.
- **Live.** The match's bots went to the party's Live session too, and
  each was refused there (§9.5): now only the roster's session is told.

**Phantoms only (2026-11-19, the user's run on f5d3010c; built, no run
yet).** The user saw the Clankers in their own party before the search,
and once the match was made the game said "Player 2's controller is not
plugged in" and went back to the lobby.

- **Why in the party.** A local bot is a local user, and the local-player
  pass (`0x63d80` → `0x65770`) adds a signed-in local user to every
  session with a state: the party's squad session as well as the match's
  target session. A bot cannot be in the match alone that way.
- **Why the controller.** The matchmade start checks each local player's
  controller, and a bot's port has no device. Which function reads it
  (an `XGetDevices`/`XInputGetCapabilities` mask or the game's own pad
  flags) is not traced: phantoms have no local player, so nothing reaches
  the check.
- **The choice: phantoms only.** A matched player is a remote peer, and
  so is a phantom: it is listed in the match's lobby only (the target
  session), never in the party, has no controller, and carries a roster
  bot account (its gamertag, emblem, rank) as a local bot did. Hiding the
  bots from the squad pass would mean hooking the pass per session and
  the controller check besides, and would still leave the bots this
  console's own party in the game's eyes (one team in a team playlist,
  the host's guests on the scoreboard). The phantom action adapter carries
  the brain's button presses, holds and releases (§5.1), including the
  sequences used by emotes (§3.5) and glitches (§3.8).
- **Built.** `clankers::MATCH_LOCAL` = 0: the fill seats every bot as a
  phantom (`clankermatch: 0 local bots join the lobby on free controller
  ports; 7 phantoms join the lobby as peers (...), in the match only`).
  While a fill runs, `Phantoms::set_match_only(true)`: its phantoms join
  only a session of the matchmaking life cycle (types 6 to 9,
  `lobby::matchmaking_cycle`), wait in the party's lobby (`bots: phantom
  G waits: not a lobby it can join (the party's lobby: a Clankermatch's
  bots join the match the search hosted, never the party)`) and leave it
  if they were in one; the flag clears once the fill's bots are gone.
  The bot glue's join gate opens in a pregame lobby only again, never a
  matchmaking one, so the Clankers page's local bots never sign in there
  either.

**Grep the run's log** (`h2 play ... 2>&1 | tee run.log`), in order:

1. `live: clankermatch: clanker hoppers [8, 9, 10]` at logon (or `the
   server sent no clanker hoppers` / `no clanker hoppers from the
   server`): the marks.
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
7. `clankers: Clanker N on (...)` and `clankers: Clanker N is <gamertag>
   on Xbox Live (bot account ...)` for the local bots, then `bots: lobby
   facts: ... session type 6, session state 5, match session state 5
   (this console hosts it): a matchmaking lobby this console hosts` and
   `bots:` gate and sign-in lines for ports 1 to 3 in the `lobby`
   (`screen lobby`), then `bots: lobby players: N in the game's session`
   rising; `clankers:
   <gamertag> (phantom, rostered) on (...)` for the rest: on the books.
8. `bots: phantom <gamertag> joins (peer N, in the pregame lobby)` in
   the lobby, before the countdown. `bots: phantom ... waits: not a lobby
   it can join (...)` names why a lobby refused one, `... waits: this
   console is not the game's host yet (...)` why a running game did.

**Run:** whether the game hosts after an empty search in every playlist
type, or needs the server's matcher to say "you host" (`MatchProposed`
with `host = me`). **Design:** use `MatchProposed` with `host = me`, and
with the bots as members, when the matcher exists (the sbmm-ranked lane).
The empty search is what is built now.

**Tests (no game):** `cargo test -p boot --lib custom_games` (the rows,
panes, texts, cursor rows), `-p boot --lib coop_live` (the seven-row
menu), `-p boot --lib clankermatch` (the filter, the schedule, the
balance, Quickmatch's hopper, the Switch To: Clankermatch row's place,
press and plan order), `-p boot --lib clankers::tests::a_clankermatch`
(the fill: one roster, phantoms only, one bot at a time, no controller
port, balanced teams around the host, the handoff, the end, a refused
roster, the match-only flag), `-p boot --lib
bot_mark`, `-p font gear` (the glyph on a synthetic font),
`-p bots phantom::session` (the marked name within 15 units, the host's
team; `phantom::session::tests::lobby`: the lobby this console hosts, the
reservations, the peer properties, the configuration, the teams, the
switch, and a phantom joining the lobby, staying for the game, leaving,
rejoining and waiting; `phantom::session::tests::in_game`: the start's
wait without phantom machines, a phantom standing in the game, the bot
mark on local bots; `lobby::a_clankermatch_...`: the target session),
`-p bots --lib lobby` (the game session by life cycle, the matchmaking
host's lobby, its player count), `-p live-hle --lib players_tests` (the
bots told to their roster's session only), `-p live-hle --lib clankermatch` (the delay and the run),
`-p live-proto bots hopper::clankers`, `-p live-telemetry
games_say_their_kind a_bots_name_reads_without_its_mark`, `-p cli
bots_drive coop_drive`, and the server's: `-p live-server --test
clankermatch`, `-p live-worker a_clankermatch_game`.

### 8.5 Game run (checklist)

`h2 play --live-local` (or a Live server with the new default hoppers),
signed in on port 0, Clankers page off. `h2 bots test --clankermatch`
(§13.1) covers the lobby and match transitions. Observe the in-game
actions in step 8 separately.

1. The Xbox Live menu shows seven rows (six without co-op): Quickmatch,
   OptiMatch, Clankermatch, Co-op Campaign, Custom Game, Custom Game
   Browser, Content Download. They fit above the hints, none scrolls,
   and each shows its own help pane (Clankermatch: "Matchmaking against
   bots..."; Custom Game its retail one).
2. Custom Game still opens the custom game lobby; back lands on Custom
   Game. Custom Game Browser opens as before, and back from it lands on
   Custom Game Browser (`custom games: opening the browser`); the same
   for Co-op Campaign. Content Download opens as before.
3. OptiMatch lists the eight human playlists and no Clanker one.
4. Clankermatch lists only Clanker Slayer, Clanker Teams and Clanker
   Doubles (the log: `clankermatch: the playlists are the clanker
   hoppers`; at logon `live: clankermatch: clanker hoppers [8, 9, 10]`).
5. Pick Clanker Slayer: the retail search screen runs for 4 to 20 s
   (`live: clankermatch: searching clanker playlist 8 ...`), then the
   game hosts its own lobby (does it? in every type: FFA and teams).
6. The match lobby fills with rostered phantoms (seven for Slayer), the
   countdown starts, and port 0 remains the only local user. Check each
   gear and gamertag in the lobby, scoreboard and carnage report.
7. In Clanker Teams check 4 v 4, with phantoms on both teams. In Clanker
   Doubles check 2 v 2 with three phantoms. Returning to the party should
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
     `sim.player.pad`. See the [agent API](../runtime/agent-api.md).
   - Put a pickup, an empty vehicle and a slowly moving enemy vehicle in
     reach in separate attempts. Watch held X take the pickup, enter the
     free seat and board the occupied vehicle. Check the bot's unit parent
     and seat after entry (§11.4).
9. Leave after the game: the bots go within 30 s (`clankermatch: the game
   is over: the bots go`). Back on the Live menu, Quickmatch starts the
   human playlist picked last in OptiMatch, or nothing if none
   (`clankermatch: hopper 8 not saved for Quickmatch` at the pick).
10. With a server that has no clanker marks: OptiMatch as retail,
    Clankermatch's list empty (say how the empty list looks).
11. In a Live custom game lobby, open Game Setup: "Switch To:
    Clankermatch" sits right under "Switch To: Matchmaking" (the log:
    `clankermatch: Game Setup list: Switch To: Clankermatch row added`).
    Press it: the clanker playlists open, as from the Live menu. Back,
    then Switch To: Matchmaking: the human playlists.

## 9. The server interface (for sbmm-ranked)

The sbmm-ranked lane owns the server side: the matcher with skill, ranked
arbitration, per-playlist rank, and the account side of **persistent bots**
(real bot accounts with their own rank that rises and falls like a human's).
The host runs the bots. This is the one interface between them.

### 9.1 The roster: server to host

The server tells a host which bots to run with a **bot roster**. In
Clankermatch it comes in `MatchProposed` on `match-<id>`, as `bots`. In a
custom game the host asks `POST /v1/games/bots` with
`{session_id, playlist?: playlist_id, count, skill?, teams}` and gets one
back; without a
server (System Link, offline) the host makes its own from the Clankers page.

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
        "seed": 1234567,          // fixed per persistent bot: same quirks every game
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

Rules:

- `skill` is the only difficulty input. The four named difficulties are
  0.0, 0.33, 0.67 and 1.0. For Clankermatch the server sets it from the
  human's level in that playlist.
- `team: null` means "host balances". The host balances by total skill.
- A made-up bot (`persistent: false`) still gets an `xuid` from the server,
  so the carnage report and stats can name it. The server owns name, emblem
  and rank for every bot, so the web shows the same bot the lobby did.
- The host never changes identity, rank or skill; it may only lower
  `count` if the session is smaller.

### 9.2 Results: host to server

These go in the normal game record (`games.rs`, the Live game recording),
with these additions:

- `kind: "clankermatch"`, `"custom"` or `"matchmade"` on the game;
- `bot: true` on each bot's player entry, keyed by its `xuid` when it has
  one. A local-slot Clanker on Live as its bot account carries that
  account's xuid (§9.5). A host's own Clanker with no account (a local
  slot without a roster, or a phantom with a host-made xuid) has `bot:
  true` and no `xuid` (§6).

Per-player stats are the same as a human's. Persistent bots are ranked
from these by the server's normal per-playlist rank update, which treats
bots like humans. Whether ranked arbitration should count a bot's report
is sbmm-ranked's call: the host is the only reporter for its bots.

### 9.3 Trust

The roster is signed by the server like `MatchProposed`. A game record with
`bot: true` entries is accepted only for xuids that the server marked as
bots in that match's roster, or in the `/games/bots` answer. A console
cannot claim a human as a bot, or a bot as a human. A `bot: true` entry
with no `xuid` is accepted with or without a session: it names no account,
so there is nothing to claim, and it counts toward no one's career (§6).

### 9.4 Interface with ranks (answers to live-matchmaking.md §8.4)

Ownership: this lane owns the bot accounts and their events (`POST
/games/bots`, the roster, `bot: true` on a game's entries, the `0x000E`
range); sbmm-ranked owns how bots are ranked and arbitration.

1. **Marking a clanker hopper.** Not in the game's `0x614`-byte
   descriptor: every byte there is a game field the validator
   (`0x193560`) reads, and the game never needs to know. The mark is in
   the server's hopper source, beside the descriptor it writes:
   `clankers: Option<Clankers>` on the playlist, with
   `Clankers { ranks_as: Option<u32> }`, where `ranks_as` is the playlist
   id of the human hopper whose standing it shares (`None`: its own rank
   playlist). The server serves the mark with the hopper list (per
   descriptor index), and live-hle reads it for the menu row's filter
   (§8.3) and for the search (§8.4).

   **Disagreement, small:** §8.4 has the clanker hopper carry the *same*
   descriptor playlist id (`+0x08`) as the human hopper. Two descriptors
   with one playlist id in one file is untested against the validator and
   the playlist UI (`0x65530`). With `ranks_as` the clanker hopper keeps an
   id of its own, and the server answers its stat read (`GET
   /v1/rank-stats` for that playlist id) with the `ranks_as` standing, so
   the lobby shows the same rank either way. If a test of the validator
   shows duplicate ids are fine, `ranks_as` can default to the
   descriptor's own id and nothing else changes.
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
3. **Agreed: ranking only reads the acceptance result.** The check runs
   when a game record is taken in (§9.3): each `bot: true` entry must be
   in that session's `BotsRostered`, and a rostered xuid cannot be claimed
   as a human. A record that fails is refused whole, so `rank_game` only
   ever sees accepted bot entries, and reads `rostered` for `persistent`
   and `level`. Host-made xuids (`live_proto::bots::is_local_xuid`) are
   refused in any record that reaches the server.

Also agreed: `rank::bot_skill(level)` as the roster's `skill`, and
`Ranks::nearest(playlist, level, n, bots)` (the candidates are
`Bots::persistent`) for choosing persistent bots, which
the roster endpoint calls; gaps are made-up bots shown at the humans'
mean level.

### 9.5 Bot accounts (the user's ask, 2026-11-18)

"Bots should not be guests but fake Xbox Live users marked as bots": every
bot the server rosters is a **bot account**, shown like an account with a
BOT mark, for matchmaking and for local games alike.

**What a bot account is** (server, both homes, shared router and read
model, `live_server::bots`):

- An xuid in the server's bot range `0x000B…` (humans `0x0009`, host-made
  `0x000E`), issued once by the server as `BotIssued` on `bot-<xuid>`
  (§10 stage 6c) with a gamertag from the word lists ("Rusty Lugnut":
  `FIRST` × `SECOND`, a number past a few clashes), emblem, colours and
  personality. Persistent bots (the operator's pool, ranked) hold their
  gamertag as an account does; made-up ones (one roster's) do not.
- **Read as an account**: `GET /v1/accounts/{gamertag}` answers for a bot
  with `bot: true` (`AccountSummary::bot`), `online: false`, its
  `created_at` (the `BotIssued` time) and its recent games. A human
  account of the same name comes first (a made-up bot does not hold its
  name). The accounts list (`GET /v1/accounts`) never has bots, and the
  web keeps them out of the Players list and the account count.
- **Never a person**: it has no keys (`/keys` 404s), so it cannot sign in
  or post anything; recovery is by an account's gamertag and a bot is not
  one; nobody else may write `bot-*` or `bots-*`. No presence (the HLE
  sends none for it, §4.2).
- **Leases.** A roster (`POST /v1/games/bots`, `BotsRostered` on
  `bots-<session>`) is the lease: its bots are the host's for that session.
  The console takes a bot account's xuid in a user change only if a roster
  answered this logon named it (§4.2). A game record's `bot: true` entry
  with an xuid is taken only for the session's roster in force at the
  start, made by the record's host (§9.3), local slot or phantom alike; an
  issued bot recorded as a human, or a human as a bot, is refused.

**Where the bots come from.** With Live and a hosted session, the Clankers
driver asks for a roster of the **whole count** (it used to ask for the
phantoms only): the local bots take its first bot accounts, the phantoms
the rest. A matchmade session asks with its playlist (`Rosters::playlist`,
the hopper descriptor index from `Hle::matchmade`), so the server fills it
with the persistent bots nearest the humans' level (§9.4); a custom game
asks without, for made-up bots at the page's difficulty. Each count,
difficulty or session change asks again, and a new roster's accounts
replace the old (the local bots are re-signed as the new ones). Until an
answer, or with none (refused, System Link, offline), local bots are the
human's guests on Live and phantoms are host-made, as before. Local bots
keep the page's skill; only phantoms take the roster's.

**The session's players.** Retail Halo 2 fills no slot counts in its
advertisement (`XOnlineMatchSessionCreate`/`Update`, `0x8fcc0`/`0x8fe20`,
slots at ad `+0x3c..+0x48`, filled by a virtual call): run 20261119-005001
showed 0 filled with six players in the game. So the host says who plays.
Every second (`clankers::frame`) while a roster is in force, the driver
hands the HLE the bot accounts in the game (`Driver::session_bots`: the
local bots' accounts and the rostered phantoms the game made players for);
`Hle::set_session_players` tells the session the roster is for
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

**One identity across a session handoff.** Halo 2 hands its lobby to a
new Live session as a game starts (and again as it ends): the HLE deletes
the lobby's session and advertises another (run 20261119-020015:
`8894d40b...` deleted, `83d9415d...` advertised a few seconds later).
The bots keep who they are through it, local bots and phantoms alike:

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

**Web.** A bot account has a player page: the identity strip with a BOT
mark, its service record and game history, no friend, message or other
social actions, never online. The page finds it in the event log
(`BotIssued`) or asks the server by gamertag. Scoreboards and carnage
reports link a bot with an xuid to it.

**Careers.** Humans' careers stay as §6 says: a kill or death with a bot
on either side is game-only. A bot account's own service record (its
page) counts all its kills and deaths. The server's stats summary has no
row for any bot, so no bot is on a leaderboard; persistent bots' ranks are
sbmm-ranked's.

**Staged (not built):**

1. Clankermatch's `MatchProposed` carrying the roster (stage 8, with
   sbmm-ranked): the matchmaking path itself. The playlist roster above
   is what it will send.
2. Bot presence ("Playing Slayer on Lockout" while its host's game runs):
   would need the server to derive it from the host's presence and the
   roster, not a bot's own signed event.
3. Keeping a local bot's account across roster changes (ask for only the
   new bots), so a count change does not rename the bots on the ports.
4. Made-up bot accounts pile up: one per bot per roster. A sweep (or
   reusing a session's made-up bots) when that matters; their pages stay.
5. Bots in the web's search lists and finder: kept for now (bots have
   pages), filter them there if the user prefers.

**Tests:** `cargo test -p live-server --test bots
a_rostered_bot_is_an_account` and
`a_host_reports_the_bots_it_leased_as_its_sessions_players` and
`a_hosts_next_session_carries_its_bots`, `-p live-hle
--lib players_tests`, `-p live-worker
rostered_bots_read_as_bot_accounts`, `-p live-hle --test server
a_leased_bot_account`, `-p bots guest` (the bot-account record and
writes, guest numbers left free, moving from guest to account, leaving),
`-p boot --lib clankers` (rosters for every bot, locals first, playlist)
and `bot_glue` (switching and leaving), `-p live-telemetry --lib
a_local_bot_carries_its_bot_account`, `bun test test/bots.test.ts` in
`web/`.

**Game run (the user):** `h2 play --live-local` (or a Live server), sign
in on port 0, host a Live custom game, Clankers On, Count 3.

`h2 bots test` (§13) runs steps 1, 4 and 5 with no one at the controller
and prints a verdict.

1. The log says `clankers: asking Live for 3 bots for session …`, then
   `Clanker 1 is <gamertag> on Xbox Live (bot account 000b…)` for each
   port, then `bots: <gamertag> on port 1 is on Xbox Live as its own bot
   account, beside port 0's` (maybe a guest line first, then the move).
2. The lobby shows the bots by their gamertags, not "Clanker N" and not
   "afr(1)". No duplicate-account dialog.
3. Plug in a second controller and sign in a guest of your account: it is
   "afr(1)" (guest 1), alongside the three bots. (With 3 bots all ports
   are taken; try Count 2.) The server log shows no refused user change.
4. Start Slayer: does the start state reach 9 (`bots: start state …`)?
   Play; after the game the web game page shows each bot with BOT, linked
   to a page with the BOT mark; the server log shows no refused record.
5. Count 5: two phantoms with bot accounts as well (6c's run).
6. Sign out of Live (or let the host logon fail): `bots: … leaves Xbox
   Live: no one is on it`; the bots are offline Clankers again.
7. System Link and split screen: as before, "Clanker N", no Live.

## 10. Stages

Each stage ends in tests that can be run without the game, plus a short
list for the user to play (§11).

1. **Minimal bot (local slot).** Built: `crates/bots` (the brain, the pads,
   the guest memory work) and `boot::bot_glue` (the hooks).
   - Bots live in `bots::local` (the library surface below). Its input
     backend is added after the host's own pads. Each bot pad asks for its
     port (`HostBackend::preferred_port`), and gets the lowest free one if a
     real pad holds it. `HostBackend::placed` reports where it went.
     `H2_BOTS=<n>` (1 to 3) adds `n` brains on ports 1 up at boot.
   - **Joining.** While the players screen `0x1e` is on top and a bot's slot
     has no profile, the glue signs the bot in itself, before
     `SOUND_UPDATE` (as the leaving bots are signed out), once a second at
     most. The bot presses nothing (see stage 7, "The join gate").
     - It copies the human's block with the bot's name and fixed controls,
       calls the game's `0x18fb34(port, block, -1)`, writes the slot's
       display name, and sets the port's bit in the players screen's mask
       (the screen `ui_watch` read), then runs the frame.
     - A bot's A that still reaches the screen's handler for a controller
       with no profile, `0x252a67` (`PLAYERS_SCREEN_UNJOINED`; its caller
       `0x252ad7` sends a controller with a profile to `0x25286d` instead),
       as a sim script's can, is answered the same way.
     - A bot never presses a menu button, so it cannot move the human's
       menus on.
     - The bots stay signed in until the game exits.
     - `H2_BOTS` bots also join a split-screen campaign from its players
       screen; there their pads are idle (perception reads multiplayer
       only), so leave `H2_BOTS` unset for campaign.
   - **Views and listeners.** The bots' local users are taken out of the
     local player map (§4.3) around two game-thread points:
     - `RENDER_FRAME`: with the pipelined render, its snapshot is taken
       inside this point, so it has them hidden too;
     - `SOUND_UPDATE` (`0x125b40`).
     
     Both are listed in `boot::render_thread::REFUSED`, as game-thread
     points. Both take no arguments and return with a plain `ret` (the
     main loop calls them back to back, `0x12b659` and `0x12b65e`), so the
     points now carry that signature, `fn() ret 0`, which a `before` hook
     needs.
   - **The brain** (`bots::brain`), once a frame, before `RENDER_FRAME`:
     - it turns toward the nearest living enemy (everyone in free-for-all,
       the other teams in a team game) and walks at it once it is within
       60 degrees;
     - it holds the trigger while that enemy is within 15 degrees of the aim
       and 10 world units (about 30 m);
     - outside a multiplayer game, and on any screen but the players screen,
       its pad is idle.

     Stages 2 and 3 replaced it as the `H2_BOTS` brain with `bots::mind`;
     `bots::brain` stays as the perception types and the simplest source.
   - Tests (`cargo test -p bots`, synthetic guest memory):
     - the backend: the port order and the pad the game reads;
     - the bot users and the hide/restore window: humans kept in order;
       nothing hidden without bots or without a human;
     - perception: me, the living others, teams, nothing outside
       multiplayer;
     - the bot block: name, controls, and the rest the human's;
     - the join check;
     - the async sign-in against a fake game sign-in;
     - the brain's turning, walking and firing, and `H2_BOTS` parsing.
     
     In boot: the render-thread plan test covers the new hooks.
   - **Game run (the user):**
     1. `H2_BOTS=1 h2 play`, then Split Screen. On the players screen a
        second player, "Clanker 1", joins by itself within a second or two
        (the log says `bots: signing Clanker 1 in on port 1`).
     2. Start a Slayer game on a small map (Lockout or Ivory Tower). The
        screen is not split: you see one full-screen view and your own HUD.
     3. You hear only what is near you, not what is near the bot.
     4. The bot spawns, turns to you, walks at you, and shoots when you are
        in front of it. It shows on your radar and on the scoreboard.
     5. Killing it and being killed by it count on the scoreboard and in
        the carnage report.
     6. Repeat with `H2_BOTS=3`: three bots, one view, all on the
        scoreboard.
     7. Back on the players screen, the bots are still joined; pressing A
        yourself goes on as usual.
     8. If step 1 fails (no second player, or a profile chooser appears),
        send the log lines starting `bots:` and `ui:`.
2. **Navigation.** Built: `bots::nav`, `bots::los`, `bots::cache`,
   `h2 bots nav`, and the map loading in `boot::bot_glue`.
   - **The mesh** (`nav::NavMesh`) comes from `crates/pathfind`'s generator
     over each structure BSP's collision: its sectors are the walkable
     polygons, its links the edges walked across. Then:
     - polygon edges that do not meet another polygon look for a floor
       beyond them: a floor up to 0.12 higher is walked onto (a seam or a
       step), up to 1.6 lower is a **drop**, up to 0.35 higher is a
       **jump**, and a floor up to 0.8 across a gap is a **jump**;
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
     2 m grid, for line of sight (stage 3).
   - **The file.** `<data>/bots/<map>.<bsp>.bots`: magic `H2BT`, format
     version 3, the **stamp** (the generator version
     `cache::GENERATOR` and the source map's fingerprint), then the mesh
     and the walls and tactical places. The data comes from the disc, so it stays on this
     machine and out of git. When the loaded map's name (`0x547898`)
     changes, the glue loads `<map>.0.bots` on a thread of its own. Until
     the data is ready, the bots can walk toward known goals. Host visual
     contacts require a loaded BSP.
   - **Built for you** (`bots::autonav`, `boot::bots_nav_auto`). At boot a
     worker thread (named `bots-nav`, at utility QoS on Apple systems)
     looks at every map the game can load, as the VFS serves it: the
     disc's `D:\maps\*.map` (so a package or `H2_MAP_DIR` map in a disc
     map's place is what counts) and the content packages'
     `T:\$c\<offer>\*.map` (the Map Pack, new maps, converted Halo 1
     maps). A multiplayer map whose `<map>.0.bots` is missing, damaged, of
     an older format, built by another generator version, or built from
     another map file is **stale** and is built; the rest are left alone.
     The boot never waits for it.
     - **The fingerprint** is FNV-1a (64-bit) over the map's 2 KiB cache
       header, which holds its size, the tag index's offsets, its name and
       the XOR checksum of the rest (every map writer here recomputes it).
       Reading it costs 2 KiB a map, so a boot with every file up to date
       does almost nothing. A converted Halo 1 map's two sandbox variants
       are two map files: switching sandbox builds that map again (about
       a second).
     - **The generator version** (`cache::GENERATOR`) is bumped whenever
       `bots::nav`, `bots::los`, `cache::build` or `crates/pathfind`'s
       generator change what a map builds. The test
       `generator_output_is_pinned` records its output on a small scene
       with the version, so changing the output without the bump fails.
     - **Order.** The level being booted (`H2_LEVEL`) first, then the scan
       in order. When a map loads, the glue asks for it
       (`Handle::want`): the worker scans again (a map installed since
       boot is found), looks at that map next, and builds it if stale.
       The bots walk straight until it lands, then take the new mesh
       without a reload (`autonav::offer`; a file read as the map loaded
       never replaces data the worker built after it).
     - **Picked with Clankers on.** `bots::autonav::build_soon(map)` asks
       for a map the same way, from the Clankers page and the map pick
       (Clankers turned on, or a map picked with them on), so a stale
       mesh is usually built by the time the game starts.
     - **Writes** are atomic: each file is written to
       `.<file>.<pid>.tmp` beside it, synced and renamed over it, the
       other BSPs first and `<map>.0.bots` (the one checked) last. A crash
       leaves the old file or the new one, never half of one.
     - **Log**: one line a map built (`bots: built the nav mesh for
       lockout (none yet): 590 polygons, ... in 0.0 s`), and one summary
       after the pass (`bots: nav meshes: 12 built, 0 up to date (0.5 s)`).
     - **Off**: `H2_BOTS_NAV_AUTO=0` (or `off`). The worker also stays
       off when the bots feature is (`H2_FEATURES=-bots`).
   - **By hand**: `h2 bots nav <map|all> [--force] [--disc ISO] [--data DIR]`
     runs the same worker in the foreground over the disc. `all` builds
     the disc's multiplayer maps that are stale and skips the rest;
     `--force` builds them all. A named map (or a map file) is always
     built, whatever its kind. Measured on an M-series Mac: the retail
     disc's 12 multiplayer maps build in about half a second together;
     every map on the disc, campaign included (80 BSPs), in about 20 s.
   - Tests (`cargo test -p bots`): synthetic collision through the real
     generator (two floors sharing an edge, an L-shaped corridor, a ledge
     dropped off but not climbed, a low ledge jumped onto, a narrow gap
     jumped and a wide one not); locating the floor under a biped; the
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
     a map installed after boot built when wanted; the worker's data
     winning over a late file read. `cargo test -p boot --lib
     bots_nav_auto`: the scan finding content maps through the VFS, a
     map read through the VFS in chunks, nothing started when off.
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
   - **Game run (the user):** `h2 play` with Clankers (or `H2_BOTS=3`),
     Slayer on Lockout, then Ivory Tower and Midship.
     1. Doorways and corners: bots walk through doors and round corners
        without scraping or stopping on the frame; none stands stuck on a
        corner for more than a moment.
     2. Fight a Legendary bot (`H2_BOTS_SKILL=legendary`): it strafes in
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
3. **Combat.** Built: `bots::mind` (`Mind`, the `H2_BOTS` brain).
   - **Seeing.** An enemy is seen when it is inside 65 degrees of the
     aim, or within 3 units anywhere, and the walls leave a clear line from
     the bot's eye to its chest or head. Teammates in a team game are never
     targets.
   - **Deciding**, every frame: fight the target it has while it is still
     seen, else the nearest seen; with no one seen, chase the last place it
     saw someone (forgotten after 10 s); else roam to a random polygon it
     can reach. The full utility layer (§3.3: weapons, health, objectives,
     flee) waits for stages 4 and 5.
   - **Aim** (`mind::Skill`, from `H2_BOTS_SKILL`: easy, normal, heroic,
     legendary; default normal). The right stick turns by the aim error
     times a gain, plus a slowly wandering error; the bot leads a moving
     target by part of the lead a 90 unit/s shot needs.

     | | Easy | Normal | Heroic | Legendary |
     |---|---|---|---|---|
     | reaction (ticks; 30 a second) | 14 | 10 | 7 | 5 |
     | error spread (radians) | 0.10 | 0.06 | 0.035 | 0.02 |
     | lead | 0.3 | 0.6 | 0.85 | 1.0 |
     | stick gain | 1.6 | 2.5 | 3.5 | 4.5 |
     | fires within | 10° | 7° | 5° | 4° |
     | trigger | 12 on, 18 off | 24 on, 8 off | held | held |
     | grenades a minute | 0.5 | 1 | 2 | 3 |

     Until its reaction time has passed the bot stands still and does not
     turn. It fires within 25 units, melees (B) within 0.9, and throws a
     grenade (left trigger) at a target 3 to 12 units away, at most once
     every 8 s. While fighting it strafes, kept on the nav mesh.
   - Not yet: weapon choice by range, grenade arcs against collision, and
     radar or sound awareness.
   - Tests (`cargo test -p bots`): the skills get better in order; a wall
     hides an enemy, and so does the bot's back; the reaction time before
     turning and firing; firing on target and not off it; Easy's bursts;
     more lead at a higher skill, with a fixed seed; chasing the last
     known position, then forgetting it; roaming to a reachable polygon; a
     teammate never a target; a dead bot lets go of every input; grenades
     rare and spaced.
   - **Game run (the user), stages 2 and 3:**
     1. Nothing to do first: the game builds the meshes itself. On the
        first boot after this change the log says `bots: built the nav
        mesh for <map> (...)` for each multiplayer map, then `bots: nav
        meshes: N built, ...`; on the next boot only the summary, with
        every map up to date.
     2. `H2_BOTS=1 h2 play`, Split Screen, Slayer on Lockout. The log says
        `bots: lockout: N polygons, ...` when the map loads (not "no nav
        mesh"). If the map loads before its mesh is built, the log says
        `building it`, then `bots: lockout: the new nav mesh is in use`.
     3. Hide behind a wall: the bot does not turn to you or shoot through
        the wall. It walks around the wall to where it last saw you.
     4. Stand still out of its sight: within a few seconds it roams the
        map along floors and ramps, drops off ledges, and does not stay
        stuck on a wall for more than a second or two (it jumps, then picks
        a new path).
     5. Step into its view: it pauses briefly (its reaction), then turns,
        strafes and fires. Get close: it melees. At mid range it sometimes
        throws a grenade.
     6. Repeat with `H2_BOTS_SKILL=easy` and `H2_BOTS_SKILL=legendary`:
        Easy is slow to react, sprays in bursts and misses a lot; Legendary
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
   - **Game run (the user)**: `H2_BOTS=1 h2 play`, Slayer with the bot. Throw
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
     - `Frame::machine` and `guest::perceive_as(Who::Machine(id))`: the
       brain finds a phantom's own player by machine id.
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
       gather samples each joined source (`Frame { port: 4, machine:
       Some(id) }`). A removed phantom's peer comes out. With the pregame
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
       nearest named skill (`skill_of`), seeded with the personality's
       seed and taunting at its `taunt` (Taunts off still makes it 0). The
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

     Teams, Aggression, Aim style and Taunts are saved but not used yet:
     they wait for the lobby teams (stage 6), the utility layer and aim
     styles (stages 4 and 5), and the emotes (stage 4).
   - **The page** is a kit screen, `clankers`: eight `ChoiceRow`s on the
     lobby's own settings dialog, Quick Options, inside and out
     (`KitScreen::dialog(DialogBase::QuickOptions, ..)`, as the Halo 1
     Options page is; docs/game/ui-builder.md "The Dialog kind"): Quick
     Options' panel, header ("CLANKERS"), list (its `setting_lists` skin,
     the name left and the value alone in the value column), the focused
     row's description in its one-line help line, and the legend in its
     footer. Its texts are the `clankers` id family. In the preview it is
     `h2 ui preview clankers`.
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
   - **The bots.** While `enabled` is on, a driver keeps `min(count, free
     ports)` brains in the process-wide local bots, on ports 1 to 3 that no
     other bot asked for (an `H2_BOTS` or `sim` bot keeps its port). They
     join on the players screen like `H2_BOTS` bots. A change on the page
     applies at once:
     - a new difficulty reaches the bots already there at their next
       frame;
     - a higher count adds bots;
     - a lower count, or Off, takes the last added off. Taking a bot off
       does not unplug its pad (in the user's run that brought up
       "CONTROLLER DISCONNECTED" for port 4 and the human could not move):
       the bot leaves the books with an idle pad still plugged. Out of a
       game (`guest::in_game` false: the menus and a pregame lobby),
       before `SOUND_UPDATE`, the glue signs its slot out with the game's
       `SLOT_SIGN_OUT` (`0x190d4b`, slot in EAX; a guest call, then the
       frame runs): off Xbox Live, its local user freed, its profile out,
       so a lobby's session drops its player. Once the slot holds no
       profile and no Live (`guest::slot_held`) it unplugs the pad
       (`bots::local::leave_step`). During a game nothing is removed: the
       bot stands idle until the game ends. Turning Clankers on again
       before then gives the new bot the leaving bot's pad back.

     The setting persists, so with `enabled` left on, the bots are added
     at the next boot too, and join any players screen, campaign
     included.
   - Past the free ports the count adds phantom peers (stage 6b), which
     join once a hosted game runs; without phantoms (tests) it adds what
     fits and says so in the log.
   - **No `H2_BOTS` needed.** Turning Clankers On is enough: the bots
     input backend, the bots feature and the driver are always installed
     (with the XAPI); `H2_BOTS` only adds bots of its own at boot.
   - **The join gate.** A bot is signed in only while its own slot has
     no profile, a human has a profile to copy, no game runs
     (`bot_glue::to_sign_in`) and either the players screen takes player
     one's input with nothing on windows 0 to 4 (`ui_watch::dialog`: a
     dialog, or the profile chooser that `0x24b6ea` and `0x1905bf` open
     for one controller), or a pregame lobby's session is up (§4.2,
     "Joining in the pregame lobby"; dialogs over the lobby do not
     matter there). The log says each port's gate when it changes,
     naming the screen it thinks is up (`bots: join gate port 1: screen
     players (top 0x1e), dialog none, signed_in false (slot 0x0): signs
     in`; `screen lobby (top none)` in a lobby; the slot's flags: `0x10`
     a profile, `0x20` signed in to Xbox Live) and each sign-in (`bots:
     signing Clanker 1 in on port 1 on the players screen`, or `in the
     pregame lobby`).

     Bots used to tap A through the gate and let the screen's handler sign
     them in. Every tap the game took elsewhere opened a profile chooser
     for the bot's controller, and a tap that reached a chooser picked the
     first profile, the human's. In the user's runs the players screen
     showed the human's player four times, and an Xbox Live lobby kept
     popping up "pick a profile": the pregame lobby's handler (`0x250b1e`)
     sends A or START from a controller with no profile to the chooser
     (`0x250cc3`, Live sessions), and B or Back from a profile that is not
     on Live to the profile UI (`0x2511b6`, `0x251242`, `0x24b779`). Now a
     joining bot's pad is idle (`Frame::joining`), the brain's and a sim
     script's alike.
   - **Their own look.** The bot's block is the human's with the bot's
     name, controls and look: the 16 bytes at block `+0x118` are the
     appearance (`0x18fd94` copies them, from slot `+0x130`, into the
     player properties at `+0x40`; `0x1a0540` zeroes them when
     `0x153750` rejects the first 8: four colours below 18, a model below
     4, an emblem foreground below 0x40 and background below 0x20, flags
     in the low nibble). The gamertag supplies the colours and emblem,
     and the profile supplies the model.
   - **Nav meshes early.** A map picked in the lobby's map list with
     Clankers on, or Clankers coming on after a map was picked, asks
     `bots::autonav::build_soon` for its mesh (`clankers::map_picked`).
   - Tests:
     - the settings in `setting_defs::ALL`, and off by default;
     - Mixed by port;
     - the driver: ports no one else asked for, the last added off first,
       only its own taken off, a difficulty change reaching the bots
       already there, and following the store's changes; taken off, the
       bots stay plugged as leaving, and on again they take their pads
       back;
     - leaving: nothing during a game, then sign-out, then unplug
       (`local::leave_step`); `guest::sign_out_then` calls
       `SLOT_SIGN_OUT` per port (never `PROFILE_SIGN_OUT` alone), then
       the frame; a slot is held while it has a profile or Live
       (`guest::slot_held`);
     - the gate: only the bare players screen with an empty slot
       (`bot_glue` tests, `ui_watch` dialog tests);
     - the look: valid by `0x153750`'s checks, repeatable across ports;
     - the page: a row per setting, steps saved in the store, wrapping;
       the row's text in its own slot, not the title's;
     - the row: only a custom game's list, once, beside the Halo 1 row;
       on the plan after the Halo 1 row's hooks;
     - the kit's own checks over the page (`ui_kit::screens`: every
       screen, every input, every state; ids; layout).
   - **Game run (the user):**
     1. Run `h2 play` (no `H2_BOTS`). Choose Split Screen (or System Link),
        then a custom game's Game Setup. Below Change Map and the others,
        a "Clanker Options..." row shows (after "Halo 1 Options..." on a
        Halo 1 map). The pages they open are titled "Clankers" and "Halo 1
        Options".
     2. Press A on it: the Clankers page opens over the dialog, with
        eight rows (Glitches last) and a description of the focused row.
        B goes back to Game Setup.
     3. Set Clankers to On and Count to 2 (the log says `clankers:
        Clanker 1 on (Normal)`). In Split Screen, bots join on the
        players screen, so back out of the lobby to it: "Clanker 1" and
        "Clanker 2" join by themselves (the screen's panes do not show
        them, §4.3); go on to the lobby again. In a System Link or Live
        lobby they join where they are (§4.2, "Joining in the pregame
        lobby").
     4. Start Slayer: the two bots play as in stages 1 to 3, at the
        difficulty chosen.
     5. Quit to the lobby, open the page, set Difficulty to Legendary,
        and play again: the bots play harder.
     6. Set Count to 1: one bot leaves. In a game it stands idle until
        the game ends (no "controller disconnected"); in the menus the log
        says `bots: signing the bots on ports [2] out`, then `bots: bot N
        left: its pad is unplugged`, and in a System Link or Live lobby
        the bot leaves the lobby's list. Set Clankers to Off: none play,
        and the lobby lists none.
     7. Quit and start `h2 play` again: the page shows what you left it
        at; `h2 settings` lists the `clankers.*` values.
     8. Send the `clankers:` and `ui:` log lines if the row is missing,
        draws no text, or opens the page under the lobby.
   **Built: the bots as Xbox Live guests, and the start log** (§4.2,
   "Xbox Live guests" and "The start check"). The user's run: in a Live
   custom game lobby with Clankers On, Count 3, Start showed an error or
   did nothing.
   - Out of a game, with the human on Xbox Live as their own account,
     each bot with a profile goes on Live as the human's guest
     (`guest::go_live`), keeping its name; offline and in System Link
     nothing changes.
   - A Start press in the lobby (A or Start from a controller with a
     profile) logs the game's start state, what holds it back, and each
     bot slot.
   - Tests: `-p bots --lib guest` (the guest writes: flag, record with the
     guest number, the XUID at slot `+8` and in the local user, the name
     kept; numbers skip taken ones and run out at 3; nothing offline;
     the host is a human on Live as their own account; the slot summary),
     `-p bots --lib lobby` (the leader check, the map check, states 0, 1,
     8, 9 and 10, the press filter, the line, and the guest call then the
     frame); `-p boot --lib bot_glue` (which ports and when) and `-p boot
     --lib render_thread` (the new observer listed).
   - **Game run (the user):**
     1. `h2 play --live-local` (or `--live`), Clankers On, Count 3. Sign in
        to Xbox Live, host a Custom Game. The log says `bots: Clanker N on
        port N is on Xbox Live as guest N of port 0's account` for each
        bot; the lobby shows "Clanker 1" to "Clanker 3" (not "afr(1)").
     2. Press Start (and A on Start Game). The log says `bots: start
        state N, blocked by ...; port 1 0x30 Live guest 1, ...`. With 9 or
        10 the countdown should start; send the line either way.
     3. If an error dialog shows, note its text and send the log around
        it (dialog 2 comes from the countdown request failing, `0x25109f`,
        or from a press by a controller with no profile, `0x250ccd`).
     4. In the game: the bots play; the site's scoreboard marks them BOT
        with no player link (not the human's account).
     5. Split Screen and System Link (not signed in to Live): the bots
        join and play as before, and no `on Xbox Live` line shows.
   **Built: bots in telemetry and the web** (§6). The user's ask: bots on
   the live game tracking, the scoreboard and the post-game record,
   marked as bots, without accounts.
   - **Why they were not bots before.** Telemetry already sent every
     listed player (`memory::players_into` filters only on salt and the
     leaving flag), so local-slot Clankers did go out, but as anonymous
     guests: `session::identity` marked a player as a bot only for a
     remote player on a rostered phantom's machine, and
     `clankers::Driver::bots_in_force` returned `None` unless a server
     roster was in force, so a Clanker on a local slot (or a host-made
     phantom) had no `bot` flag. The proto and server also refused `bot:
     true` without an xuid (`game::validate_new`, `bots::check_players`),
     and the web had no idea of `bot` at all: a rostered bot with an xuid
     was linked to a player page by its name, and every bot's knockouts
     went into the human's career stats.
   - **Now:** `PlayerState::controller` (player `+0x24`); `Bots` has
     `session_id: Option`, every phantom's machine and the local bot
     `ports`; `bots_in_force` answers whenever there are bots. `bot:
     true` without an xuid is accepted on both homes; the stats are
     game-only (§6); the web marks and unlinks bots.
   - Tests: `-p live-telemetry --lib` (`memory`: a local bot slot read
     with its port; `session`: local Clankers as bots with no account
     through the start, scores, frames and the end; a bot on local user 0
     never the account; a rostered xuid only with its session; the
     rostered test updated); `-p boot --tests clankers` (bots in force
     before and after a roster, and after a refusal); `-p live-proto
     game` (an xuid-less bot with or without a session); `-p live-server
     --test bots` (a host's own Clankers taken and game-only; a human
     still refused as a bot); `-p live-worker --lib sqlstore` (the same
     on the Worker's composition); `web`: `bun test test/bots.test.ts`
     (the record, the labels, the career, the mock's fold, the BOT mark
     and no player link on the scoreboard and carnage report).
   - **Game run (the user), bots in telemetry:**
     1. With Live (`h2 play --live-local`, signed in), host a custom game,
        set Clankers On, Count 2, and start Slayer.
     2. On the site's live game page: the scoreboard lists "Clanker 1"
        and "Clanker 2" with a BOT mark, not as links; the map shows
        their dots and labels ("Clanker 1 · BOT") moving; their kills and
        deaths tick up.
     3. Open the 3D viewer: their labels carry BOT, and the follow list
        says "(BOT)".
     4. Kill a bot and get killed by one. After the game, the carnage
        report lists both bots with their kills and deaths, marked BOT,
        unlinked. Your player page's service record does not count those
        kills or deaths; the game's history row does.
     5. The server log shows no refused game record.
     6. With Count 5 (phantoms past the free ports): the phantoms show
        with BOT too, before and after a roster comes.
     7. Not covered: a local bot added mid-game is marked as a bot only
        from the next game's start.
   **Built: bots join in the pregame lobby; phantoms wait for hosting**
   (§4.2, "Joining in the pregame lobby"). The user's run (Live,
   Clankers On, Count 5, hosted from the Live menu): no local bot joined
   (the gate opened only on the players screen, and the lobby reads as
   no screen), and the phantom failed for good ("not the host").
   - The gate opens in a pregame lobby, read from the squad session
     (`bots::lobby::pregame`), out of a game; the bot is signed in with
     no screen mask, and the session's own local-player pass adds it.
     The gate line names the screen (`players`, `lobby`, an id, `none`).
   - A phantom that finds this console not (yet) the game's host waits
     and tries again, logged once per reason, instead of failing.
   - Tests: `-p bots --lib lobby` (a pregame lobby: types 1 and 2, states
     3 to 8; not with no session, another type, another state, networking
     off, no lobby, or nothing mapped); `-p bots --lib guest` (the lobby
     join: a sign-in per bot, then the frame, no mask); `-p bots --lib
     phantom` (the waiting phantom: not failed, said once, said again for
     another reason, joins once hosting is up; each `NotHost` reason);
     `-p boot --lib bot_glue` (the screen: players first, then the lobby,
     never during a game; the gate open in the lobby under a dialog; each
     joining bot signed in in the lobby and none in a game; the players
     screen's cases as before; the gate and sign-in lines).
   - **Game run (the user):**
     1. `h2 play --live-local` (or `--live`), signed in. Host a Custom
        Game from the Live menu, open Game Setup, Clanker Options: On,
        Count 5. Back in the lobby, within a second the log says, per
        port 1 to 3, `bots: join gate port N: screen lobby (top none),
        …: signs in`, then `bots: signing Clanker N in on port N in the
        pregame lobby`, then `bots: … is on Xbox Live as its own bot
        account` (or as a guest).
     2. The lobby lists the three local bots by their bot accounts'
        gamertags (or "Clanker N"), not "Halo0001" or "afr(1)".
     3. Start the game. The phantoms (Count past 3) say `bots: phantom …
        waits: this console is not the game's host yet (…)` at most once
        per reason, then `bots: phantom … joins (peer N)`. No `failed:
        not the host`.
     4. In the game all five bots play. Send the `bots:` lines if a local
        bot does not show in the lobby (the `screen` word says what the
        gate saw) or a phantom keeps waiting (its reason says which check).
     5. System Link (not on Live): the same, with no `on Xbox Live` line;
        the bots show in the lobby at once. Split Screen: the bots still
        join on the players screen (`screen players`), hidden from its
        panes (§4.3).
   **Built: fixes from the first `h2 bots test` run** (2026-11-18,
   `20261118-232735`, Tester, count 5). What the run was: the drive never
   reached the Xbox Live menu (`bots-drive: stuck: no Xbox Live menu
   after 4 tries`; the game logged `mods: no list: not signed in to Live`
   throughout), and the game that was played was **Split Screen**: the
   players screen read `0x1e … mode 1`, whose A makes a local squad
   session (`0x199df9(1, 0)` → `0x591e0` → `0x59890(0, 0, 0)`) and sets
   lobby mode 2 (`0x199a03(2)`, the custom game mode every kind of
   custom game uses). `live: playing as Tester` is only the Live HLE's
   account, not a game sign-in.
   - **No bot signed in, and the lobby was never the lobby: "in a game"
     was true in the menus.** The glue took "a game runs" from a game
     tick alone, and the main menu is a game too: `mainmenu.map` goes
     through `GAME_ACTIVATE` and its game-time record ticks (the run's
     status read `mode main_menu` with a tick on every menu screen;
     options mode 3). So `to_sign_in` returned nothing on the players
     screen though every gate said `signs in`, and `screen` never named
     the lobby (it is the lobby only out of a game). Now
     `guest::in_game` is a tick in a game whose options mode
     (`[0x4e6948] + 8`) is not 3; every "no game runs" in `bot_glue`
     uses it (sign-in, Xbox Live, leaving).
   - **The tries were made only on frames that are multiples of 30**
     (`frame % 30 == 0` in `SOUND_UPDATE`, with `frame` counted in
     `RENDER_FRAME`). The two hooks do not run once each per frame, so a
     run could miss every multiple. `bot_glue::Every` makes the first
     call due and then the first one 30 or more frames after the last.
   - **Logging.** With a gate open and no one signed in, once per reason:
     `bots: the join gate is open, but no one signs in: a game runs` (or
     `no human profile to copy …`). The lobby's facts, once per change:
     `bots: lobby facts: lobby true, networking true, session type N,
     session state N: a pregame lobby`. No roster asked:
     `clankers: no roster yet: this console hosts no Xbox Live session
     (host-made bots until it does)`.
   - **No roster: as designed.** The roster is asked for the session the
     Live HLE hosts (`Hle::hosted_session`), made only when the game
     itself hosts on Xbox Live; this game never signed in. A Live session
     hosted later asks (`Driver::refresh`, every 30 frames).
   - **Phantoms: not in Split Screen.** A local squad session (session
     `+0x18` 0) gets protocol 0 (`0x64060`), so the game's simulation
     mode is 1 (`0x6ec80`, live-coop.md §1.1) and there are no peers to
     be. A hosted System Link or Live custom game is distributed whatever
     its peer count (protocol 2 for every shipped engine, `0x161c90`), so
     the host is game mode 5 (world mode 4) there with one console. Phantoms now say so in
     Split Screen (`waits: … (a local game (simulation mode 1, Split
     Screen): phantoms join System Link and Xbox Live games only)`).
     Making Split Screen distributed is not done: it would change every
     Split Screen game's simulation for the sake of phantoms.
   - Tests: `-p bots --lib guest` (`in_game`: the main menu ticks but is
     not a game; multiplayer and campaign are; a tick with no options is;
     no tick is not), `-p bots --lib lobby` (the facts line), `-p bots
     --lib phantom` (the Split Screen reason), `-p boot --lib bot_glue`
     (`Every`, the skip reasons), `-p boot --lib clankers` (the no-roster
     line).
   - **Next run:** the harness has to reach the Xbox Live menu and host a
     Custom Game there (its drive stuck at the main menu's Xbox Live row);
     on a Split Screen run expect the three local bots to sign in on the
     players screen and no phantom to join.
   **Built: bots off the players screen, and quiet** (2026-11-19, §4.3,
   "The players screen", "Listeners" and "The controller speaker,
   haptics and rumble"). The user's ask: "i don't want bots to appear in
   the other player slots on the xbox live screen ... also other bots
   sounds come out via the controller?"
   - **Where bots join:** the players screen only in Split Screen (mode
     1). In Xbox Live, System Link and an invitation's mode the gate
     reads it as the lobby (a pregame session up) or as `0x1e` (none),
     so the bots join in the lobby (`bot_glue::screen`).
   - **The panes:** around `PLAYERS_SCREEN_UPDATE` the bots' controller
     handles read as none (`guest::hide_pads`, `restore_pads`), so a
     bot's pane says "CONNECT CONTROLLER" in every mode, Split Screen
     included, joined or not.
   - **The bots' ears:** `SOUND_LISTENERS_UPDATE` gets the hide window
     too (the per-frame update `0x18c060` runs it outside
     `SOUND_UPDATE`), and at both points the hidden users' listeners are
     marked inactive (`guest::quiet_listeners`).
   - **Voice:** a bot's port gets no communicator
     (`voice_glue::player_pads`). Rumble for a bot's port was already
     refused (no motors); a test now says so.
   - Tests: `-p boot --lib bot_glue`
     (`only_split_screen_bots_join_on_the_players_screen`), `-p bots
     --lib guest` (`the_players_screen_sees_no_controller_on_a_bot_port_and_the_humans_as_it_is`,
     `no_bot_controller_hides_nothing_and_a_handle_the_game_wrote_is_kept`,
     `a_bot_users_listener_goes_quiet_and_the_humans_stays`,
     `no_sound_globals_quiets_nothing`), `-p bots --lib local`
     (`rumble_for_a_bots_port_never_reaches_the_humans_pad`), `-p boot
     --lib voice_glue` (`a_bots_port_gets_no_communicator`), `-p boot
     --lib render_thread` (the two new points listed), `-p cli --bin h2
     bots_drive` (the press label).
   - **Game run (the user):**
     1. `h2 play --live`, Clankers On, Count 3 or more. Main menu, Xbox
        Live: the players screen shows you alone; the other three panes
        say "CONNECT CONTROLLER". The log has no `bots: signing ... on
        the players screen`.
     2. Live menu, Custom Game: in the lobby, `bots: signing Clanker N in
        on port N in the pregame lobby`, then each is on Xbox Live and in
        the lobby's list.
     3. Back out to the players screen: still you alone (the bots stay
        signed in, unseen). On again to the lobby: the bots are there.
     4. In a game, with the controller speaker on (a DualSense, or a
        Steam Controller as haptics): only your own reloads, jumps,
        landings, melee and shield warnings come out of the controller;
        the bots' are heard in the world, from where they are. Send the
        `bots:` lines and say which sounds still come through if any do.
     5. With a microphone chosen (pause menu, Voice): other players'
        voices sound as they did with no bots (no doubling).
     6. Split Screen with Clankers On: the players screen shows you
        alone, the lobby and the game have the bots. Say whether B on
        the players screen backs out or opens your profile menu.
   **Built: a bot's sounds come from the bot** (2026-11-19, §4.3,
   "Whose sound"). The user's run: "i can hear sound from every ai
   player, also, sound doesn't get quieter in distance or round corners
   etc".
   - **Cause:** a sound starting on a local user's unit plays as that
     user's own, unpositioned, in the ears (`0x18d4f0`), and the bots
     are local users; the walk runs on the game tick, outside every
     window. Each bot's gun, footsteps, grenades and voice played at
     full level in the human's ears.
   - **Fix:** the window around `SOUND_OWNER_USER` (`bot_glue`, "hide
     bot owners" and "bot owners back").
   - Tests: `-p bots --lib guest`
     (`a_bots_own_sound_starts_as_a_world_sound_and_the_humans_stays_in_their_ears`),
     `-p boot --lib render_thread` (the new point listed).
   - **Game run (the user):** a match with three Clankers, no
     controller speaker needed. Stand still away from the bots: their
     shots and footsteps are quiet or silent far away, louder as they
     come closer, panned to where they are, muffled behind walls. Your
     own gun, footsteps and melee sound as before. Say whether any bot
     sound still plays as if it were yours (and which).
   **Built: Off takes the bots out of the lobby** (2026-11-19). The
   user's run: "clankers still a bit buggy, i turned them off and they
   stay in the lobby."
   - **Cause.** A bot taken off was signed out with `PROFILE_SIGN_OUT`
     (`0x18fc08`), which only zeroes the profile block, clears `0x10` and
     sets `+0x1f8` to -1. The slot kept its local user (`+4`, an entry in
     use at `0x4cd904`) with its Live identity (`0x20`, `+8`, `+0x470`).
     The squad session's local-player pass (`0x65770`) drops a player only
     when its local user goes (`0x646b0` status 0: entry `+0` clear, or on
     Live the offline flags `0xBAD00000`; then `0x65b80`), so the bot
     stayed in the lobby's list, on Live as its bot account or guest, and
     the glue unplugged its pad all the same. Clankers on again then took
     a second local user for the port (`0x18fb34` takes one when the slot
     has no profile), leaving the first one's player behind.
   - **Leaving the lobby (traced).** The game's own whole-slot sign-out is
     `0x190d4b` (`SLOT_SIGN_OUT`, slot in EAX, `ret`): `0x190da5(slot)`
     (stdcall, `ret 4`: clears `0x20`, zeroes the account record and the
     Live state around it, releases `+0xc14` and `+0xc18` through
     `0x6b640` via `0x190d0a`), then the slot's local user freed
     (`0x64690`, EDX the index: zeroes its `0xd0` bytes) and `+4` set to
     -1, then `0x18fc08`, then the flags, `+8..+0x13` and the name
     cleared and `+0x14` set to -1. The profile UI signs a profile out
     with it (`0x24b807`), and a failed host logon signs every slot of
     that account out with it (`0x190186`, at `0x1901e7`). The glue now
     calls it in place of `0x18fc08` (`guest::sign_out_then`); with the
     local user gone, the pass takes the bot's player out of the session
     on its next frame, as when a second controller's player signs out
     of a lobby. The pad still goes a frame later, once the slot holds no
     profile and no Live (`guest::slot_held`, `local::leave_step`).
   - **The server.** The game's session drops the player, and the Live
     HLE hears of it as of any player leaving. The driver already tells
     the hosted session its bot accounts every 30 frames
     (`Driver::report_players`), once more with none after Off.
   - **Phantoms** were already right: they join only while a game runs,
     and one taken off has its peer taken out at the next frame
     (`Phantoms::tick`), every one when the game ends.
   - **Not changed.** A lower count or Off during a game still waits for
     the game to end (an unplugged pad pauses it). A Clankermatch's bots
     are the playlist's, not the page's: Off on the Clankers page does
     not apply there.
   - Tests: `-p bots --lib guest`
     (`signing_out_calls_the_games_whole_slot_sign_out_per_port_then_the_frame`,
     `a_slot_is_held_while_it_has_a_profile_or_xbox_live`), `-p bots
     --lib local` (`leaving_bots_wait_out_the_game_then_sign_out_then_unplug`),
     `-p engine --lib points` (`every_address_is_named_once`), `-p boot
     --lib clankers` (`the_driver_follows_the_store`,
     `the_hosted_session_is_told_of_the_bot_accounts_in_the_game`).
   - **Game run (the user):**
     1. `h2 play --live`, host a Custom Game, Clankers On, Count 3: the
        three bots are in the lobby's list.
     2. Count 1: the log says `bots: signing the bots on ports [2, 3]
        out`, then `bots: bot N left: its pad is unplugged` twice; the
        lobby lists you and Clanker 1 only at once.
     3. Clankers Off: Clanker 1 leaves the list the same way.
     4. Clankers On, Count 3 again: three bots, each once (no leftover
        "Clanker 2" or a renamed copy beside the new one).
     5. The same in a System Link custom game.
     6. Send the `bots:` lines if a bot stays in the list, the lobby
        shows an error, or the human's own player leaves.
   **Built: combat boost** (2026-11-19). The user, after playing the
   bots: "bots kinda suck, ... don't know when they have no ammo ...
   give the ai a nice big boost". This slice is the fighting decisions
   (`bots::arms`, the new `bots::tactics`, small hooks in `bots::mind`);
   the movement (getting stuck, strafing, cover) is another lane's.
   - **The weapon is known** (§3.6, "Handling"): role, band, the range
     it shoots to, its trigger, its rate and its projectile's speed.
   - **The trigger.** A once-per-pull weapon (magnum, battle rifle,
     carbine, shotgun, sniper, rockets, the plasma pistol's quick
     shots) fired one shot and then held the trigger for nothing: now it
     is pulled for 2 ticks and let go, every `30 / rate` ticks (at least
     4; 8 when the rate is unknown) plus the skill's pace (Easy 12,
     Normal 6, Heroic 3, Legendary 1 tick). Automatics keep the skill's
     bursts. A blade never pulls; it swings (B) within 1.5.
   - **No dry fire.** The trigger stays up with an empty magazine or a
     dead battery; it reloads or switches instead. It fires only in the
     weapon's range: a shotgun no further than 8, rockets never nearer
     than 2.5.
   - **Weapon choice** (Normal and up): switches to the gun that suits
     the distance; draws the other gun rather than reload in a fight;
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
   - **Grenades** (Normal and up): none at a target with a teammate
     within 3 of it.
   - **Pre-aim** (Normal and up): chasing, it looks where the enemy went
     out of view (within 15, and not more than 100 degrees off the way it
     walks), not where its feet go.
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
   - **Game run (the user):** `H2_BOTS_SKILL=heroic`, Slayer on
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
     allowed (Heroic and up), throws 8 to 60 degrees to either side: off a
     wall, or rolled past a corner to where someone went out of view. A
     plasma that sticks the target counts as a hit. The arc blocked, it
     does not throw (tries again in 2/3 s).
   - **When and what** (`grenades::Thrower`, the mind's `grenade`). The
     skill's rate per minute, times 4 into a group (another enemy within
     2.5 of the target; it then aims between them), 2 at where a target
     went out of view (2 s at most), 3 before pushing in with a shotgun,
     SMG, plasma or sword (the target beyond its band). A plasma for a
     target in view within 8 (Normal and up), a frag otherwise; the type
     selected (unit `+0x23c`) changed first with Black (switch grenades,
     Halo 2's default layout). Never one that goes off within 3 of its own
     feet; from Normal, never within 3 of a teammate. Then it turns to the
     throw (the mind looks along it) and lets go once its aim is within
     the skill's tolerance, giving up after 1 s; a human hand: the throw's
     aim is off by the skill's noise. At most one every 8 s.

     | | Easy | Normal | Heroic | Legendary |
     |---|---|---|---|---|
     | banks and rolls | no | no | yes | yes |
     | sticks plasmas | no | yes | yes | yes |
     | lead | 0 | 0.5 | 0.85 | 1 |
     | aim noise (degrees, std. dev.) | 6 | 3 | 1.5 | 0.8 |
     | lets go within (degrees) | 8 | 5 | 4 | 3 |
     | spares teammates | no | yes | yes | yes |
     | more before pushing in | no | yes | yes | yes |

   - **Weapon ranges** (`weapons::reach`). A gun's band and fire range
     are its role's (§3.6) refined by its tag's aim assist reach
     (`cone_range`, `pull_range`): a magnum, SMG, shotgun, plasma or
     rifle is good to its reach (kept within half and one and a half
     times its role's band end) and fires to 1.6 times that, so a magnum
     backs off to the rifle sooner and an SMG is not sprayed at 18. A
     sniper or splash weapon keeps its role's. A sword's lunge is its
     reach (1 to 3; 1.5 unread), and it swings at that. The weapons
     decision, the movement's band and the fire range all use this.
   - **Using both guns** (`arms::plan`, Normal and up, out of a fight).
     Two short guns or two long ones: a gun of the other kind within 15
     is taken, the worse of the two dropped for it (switching first when
     the better is in hand). A full magazine in hand and the other gun
     wanting a reload: switch to it, so it is reloaded out of the fight.
     In a fight, the switches by range and instead of a reload (§3.6) now
     use each gun's reach.
   - **Combos** (`weapons::Combo`, the mind's `combo`). The **noob
     combo**: holding a charged plasma pistol (battery 0.15 and up) with
     a rifle, magnum or SMG in the other hand, a target 1.5 to 14 away:
     hold the trigger 24 ticks, let go when on target (36 at most: a held
     charge drains), wait the skill's delay, switch (Y), and shoot for the
     head; the weapons decision does not switch away for 2 s. With the
     rifle in hand and the pistol in the other on a fresh target, it draws
     the pistol first. Once per target, by chance: Easy 10%, Normal 35%,
     Heroic 70%, Legendary 90%; switch delay 18, 10, 6, 3 ticks. The
     **melee finish**: close (1.3) to a target it has hit with a gun
     (pulls on target: Normal 5, Heroic 4, Legendary 3; an automatic one
     each 8 ticks of fire; after a combo's overcharge, at once), melee.
     Easy never. Without traced target shields "hits" are pulls on target.
   - **Dual wielding** (unit `+0x213`, the left hand's slot, traced in
     docs/live/live-gamestate.md and `boot::touch_glue`). The left gun
     reads as `Arms::left`; the left trigger fires it with the right's
     aim (a charge gun charges and lets go on target, a once-per-pull
     gun is pulled at its rate, an automatic held); no grenades then.
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
       upper radius) for the safety distance (3 assumed);
     - a target's shield (the body's `+0xf0`, already read for the bot
       itself in `brain::Me`) in `brain::Seen`, so the melee finish and
       the noob combo's swap follow real shields, not shots on target.
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
     doorways before the feet finish the turn. The navigation follower
     checks the corner cut for footing and keeps gap approaches at pace.
   - Bots crouch-walk on quiet objective approaches. Hits alone can
     change the strafe or start a crouch at range. Jumps are single
     short presses at gaps, during stuck recovery, or on a clear
     escape line from a visible nearby grenade.
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

`crates/bots` gives other code, such as the agent API's `sim.*` calls, a
small surface. Everything in it is controller input.

- `bots::local::add(port, Box<dyn PadSource>) -> Result<BotHandle, Error>`
  puts a bot on controller port `port` (0 to 3), named "Clanker <port>".
  - Errors: `NoSuchPort`, `PortTaken` (another bot asked for it), and
    `Full` (3 bots already).
  - If a real pad holds the port, the bot goes to the lowest free one.
- `bots::local::remove(handle)` takes the bot off the books (`list` no
  longer has it); its pad stays plugged and idle. The boot glue signs its
  slot out once no game runs, then unplugs the pad
  (`Local::leaving`, `leave_step`, `Local::unplug`). Adding a bot on that
  port before then takes the pad back.
- `bots::local::list() -> Vec<BotInfo>` gives each bot's handle, name, the
  port it asked for, and the port it got (`None` until the input manager
  has placed it).
- `trait PadSource: Send { fn pad(&mut self, f: &Frame) -> HostPadState }`
  - A placed local bot is sampled once per rendered frame on the game
    thread, before `RENDER_FRAME`. A spawned phantom is sampled when
    the hosted simulation gathers its slot for a new game tick (§5.1).
    Its `Frame.frame` counts pad samples so a recording or script advances
    once, even if two ticks fall in the same render or a tick is read twice.
  - `Frame` carries the frame count, the game's tick (`tick`, 30 a
    second, `None` in the menus), the port, the guest memory and
    `joining`: the players screen is on top and the bot's slot has no
    profile, so A signs it in. For a local bot, a slow render can cover
    several simulation ticks and a fast one none. Time by `tick` to
    match the simulation.
  - A source must not call `add` or `remove` from inside `pad`.
  - Closures `FnMut(&Frame) -> HostPadState` are sources.
  - Three sources come with the crate:
    - `Scripted`: timed steps, optionally looping;
    - `Recorded`: one pad per frame, played once;
    - `bots::Brain`: the brain. It presses A while `joining`, uses its
      `mind::Mind` (with the loaded map's nav mesh and walls) in a
      multiplayer game, and is idle elsewhere.
- `bots::phantom::add(Identity, team: Option<u8>, Box<dyn PadSource>)`
  puts a phantom on the books; it joins once a multiplayer game this
  console hosts runs (§5.1). `Identity` carries the roster's XUID and full gamertag.
  The phantom applies that name's look to its player configuration.
- `bots::phantom::remove(handle)`, `list() -> Vec<PhantomInfo>` (handle,
  identity, team, machine id, `player: Option<u32>`, `state: Pending |
  Joined | Failed(reason)`) and `player(handle) -> Option<u32>` work on the
  process-wide `Phantoms` (`phantom::global()`). Errors: `Full` (15), `Duplicate(xuid)`, `BadIdentity`, `NoSuchPhantom`.
- `Frame::machine` is a phantom's machine id (`None` for a local bot; a
  phantom's `port` is 4). A source finds its own player with
  `guest::perceive_as(Who::Machine(id))`.
- `bots::local::Local` is the same API on an instance. The free functions
  use the process-wide one (`local::global()`), which the boot glue drives
  and whose backend boot adds to the input manager. Tests make their own.

Known limits, for the agent API's `sim.*` docs (agent-api.md):

1. The embedded `live_server` does not yet issue bot identities or check
   `bot: true` against them. That comes with stage 6 (§9.1, §10 stage 6);
   until then agent-api.md should say "will".
2. Script `ticks` count bot frames, not game ticks. A source that wants
   game time inside a game can use `f.tick` (for example
   `f.tick.map(u64::from).unwrap_or(f.frame)`).
3. `Recorded` plays one pad per successful read, once a frame. The game can
   call XInputGetState more than once a frame, so playback can drift from
   the recording's timing.
4. A local bot signs in by pressing A only while the players screen is on
   top. One added in a lobby or mid-game does not join until that screen
   shows.
5. `remove` on a local bot in a game leaves it standing idle until the
   game ends; it is signed out and unplugged in the menus.

## 11. What needs a game run

Each item is for the user to play, or for a coordinator run.

1. A bot port signing in through the players screen with an injected guest
   profile (§4.2), on Split Screen only; and in a System Link or Live
   pregame lobby (§4.2, "Joining in the pregame lobby"). The players
   screen shows no bot in any mode (§4.3, "The players screen").
2. The view window: the human gets a full screen, and the HUD, sound and
   camera are right with 1 to 3 bots.
3. Whether Live matchmaking accepts guests (§4.4), and how our Live names
   them.
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

Natural feel ("Built: natural feel (sol)" in §10), local virtual pads:

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
31. Watch a local bot in a quiet CTF or Hill approach, near an empty
    magazine and during a close melee encounter. Look for a slower
    crouch-walk, a brief delay before X or Y, and spaced melee swings.
    Leave it waiting and check the lobby selection and respawn remain
    under the human's control.

### 11.2 Objectives in play (checklist)

For the user, or a coordinator run. Two teams of two or three (a human
and Clankers), Normal. Each line is a yes or no.

CTF on Lockout or Beaver Creek:

1. With both flags home, one bot per team runs for the enemy flag and
   (with three or more on the team) one stays near its own flag.
2. A bot reaching the enemy flag takes it even with a full inventory.
   Check the X hold while the flag is loose and its release on pickup.
3. A bot carrying the flag runs home along the floor, does not shoot,
   does not drop it (no weapon switch) and does not stop to reload or
   for a gun, even with its shields down. It melees a human standing in
   its way.
4. Reaching home with our flag there, it scores (the score goes up).
5. Take a bot team's flag: its defender, and an attacker nearer to you,
   come after you; when you drop it, the nearest bot walks over it and it
   goes home.
6. A bot whose teammate has the enemy flag runs near them.
7. Spawns and returns are right on both maps: the bots' "home" is their
   own base, not the enemy's (the designator map, `E+8`/`E+0x10`).

Oddball (any small map):

8. When the ball spawns, the bots run for it.
9. A bot with the ball takes cover or moves away from an approaching enemy.
   It keeps its trigger up and melees a human who comes close.
10. When a human has the ball, the bots come after them; in teams, a
    bot's teammate with it is followed, not attacked.

King of the Hill (Midship or Lockout):

11. The bots go to the hill and stay in it, fighting from it, rather than
    chasing out of it.
12. When the hill moves, they go to the new one within a few seconds.
13. A bot in the hill counts as in it (the hill's colour and the score
    say so). Check the score on a different floor over the same XY and after a move.

Assault (if tried):

14. Check which side picks up a team bomb and a neutral bomb, and whether
    the carrier scores at the plate its escorts defend (§3.7).

All:

15. No bot teabags while carrying, guarding or in the hill.
16. Rotate a one-bomb round. Check both teams swap roles, the carrier
    stays until armed and plants on the active plate, and escorts cover
    a planted bomb while recovering a loose one.
17. In team Territories, contest a friendly site while a neutral site
    is closer. Watch the bot relieve it and hold through a capture. In
    FFA, check that another player's territory counts as hostile.
18. Kill the Juggernaut in front of two hunters. Check they switch to
    the new Juggernaut and leave one another alone. As Juggernaut the
    bot should close on a visible opponent and melee.
19. In Oddball check the first spawn and a reset against a neutral type-4
    marker on the map.

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
   exit in both local slots and Clankermatch. Inspect playback through
   `h2 agent` and the existing `sim.*` and world methods (§10.1).
8. On H1 Blood Gulch verify both outbound portals from their bases. On
   Sidewinder exercise both directions between the paired pads and each
   outbound-only pad. Try a crouch-jump route above standing reach on
   both maps. Check whether Sidewinder's blast-door scenery moves.
9. On Containment watch both high receivers through free fall. Check
   the actual player exit on all four Warlock lifts, the Lockout tube and
   Midship's midpillars. Try Gemini's columns with a player. On Relic
   compare traversal before and after activation of the portal switch.

## 12. Next tracing (offline)

Done for stage 1 (§4.1 to §4.3): the pad seam, the slot flags and the
sign-in function, the players screen's join path, and the readers of the
local-user map.

Still open:

- For phantoms (§5.1, "Open"): the lobby's ready check `0x5f970`, the
  join room check `0x62e30` and the veto object, `0x5fe20` on a peer with
  no channel, and `0x14c090`'s body.
- How the players screen's mask becomes the game's local users when the
  game starts. A run answers it for stage 1 (stage 1 run, step 1); a trace
  is needed only if that fails.
- Compare the phantom's per-tick hold transition, zoom release and grenade
  throw after dropping a dual weapon with a local bot in play (§5.1 and
  §8.5). The button masks and selection fields have static traces.
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
  phase with a witnessed pickup and actual return on local and phantom
  pads, including alternate H1 sandboxes and multiplayer variants.

## 13. Testing (`h2 bots test`)

`h2 bots test` runs §9.5's game run with no one at the controller: one
command that ends in a verdict. It is built on `h2 coop test --drive`'s
pieces (docs/live/live-coop.md section 12): the same process supervisor,
account making and `input.pad` presses.

```sh
h2 bots test [--count 5] [--map lockout] [--keep] [--timeout 360]
             [--wait-clang] [--settings-from DIR] [--no-drive]
h2 bots verdict <bots.log>   # the verdict of a log again
```

**What it does.**

1. Starts a Live server in its own process on `127.0.0.1:8077`
   (`--server-port`), with a throwaway store under
   `$TMPDIR/h2-bots-test/run-<time>/` (`--root`), and makes the account
   **Tester** there.
2. Opens one `h2 play` window signed in as Tester (`--live-server`, its own
   data and Live directory, `--no-mappack --no-halo1`), with
   `--set clankers.enabled=true --set clankers.count=N`. So Clankers is On
   at Count N for this run with no menu work (the page's rows show it
   locked). N is `--count`, 5 by default: 3 local bots on ports 1 to 3 and
   2 phantoms. The window builds the game into `--out` first when `--out`
   has none, as `h2 play` does. `--wait-clang` passes `--tier clang`, so
   the run uses clang's full build and never the Cranelift fast tier; it
   first waits while a background clang build of `--out` is still writing
   `clang-build.log`. The run's data starts with the `[graphics]` table of
   `--settings-from`'s `settings.toml` (by default `~/Library/Application Support/h2recomp/data`,
   the data root `h2 play` uses), so the window renders with the user's
   own video settings.
3. Presses through the menus over the game's agent socket
   (`crates/cli/src/bots_drive.rs`). The game runs with `H2_UI_WATCH=1`, and
   every press waits on a `ui: screen` line, a game line or the agent API's
   phase. A fresh data folder stops on the title screen and the new
   profile's dialogs: each gets A once it has held 4 s. The main menu is
   the window 5 screen the game names with `coop-live: lobby mode
   4294967295`; its cursor row is the low 16 bits of the `list [mask/value]`
   value on its `ui: screen none` line (rows: 0 Campaign, 1 Xbox Live, 2
   Split Screen, 3 System Link, 4 Settings). From there: to row 1, A. Not
   signed in, that opens the game's account picker (`dialog_live_signin`,
   dialog `0x14`, loader `0x24b4a9`), as a new profile's boot does. Its
   row 0 (item `0x1023`, `0x24af1a`) is the no-account row: A there closes
   it signed out, which is how the second run (20261119-001305) went round
   the main menu four times. Then one row per account `XOnlineGetUsers`
   lists (the run's `live/accounts.json`: Tester), then New Account. A on
   an account row copies it into the profile and logs on (`0x24b2ea`,
   `0x24b664`). So the drive knows the picker by the game's `ui: screen
   none` line, which names the dialog up (`, dialog 0x14
   dialog_live_signin (window 3, slot 4)`; `ui.screen` answers it as
   `dialog`), or by the bots' join gate line (`dialog 0x14 (window 3,
   ...)`). The gate line alone was not enough: a Clankermatch run places
   no local bot before the roster, so it prints none, and run
   20261119-115144 took the picker for any dialog and pressed A on its
   no-account row at boot and on each of four Xbox Live tries. It presses
   DOWN off row 0 and A on the account (A anyway when the cursor shows no
   move), once per opening, and gives up after 3 picks that did not sign
   in, or a picker still up 30 s after A. Another dialog on the way gets
   A, its press naming the dialog (`dialog 0x21 confirmation_dialog
   (window 3, slot 4) on the way to Xbox Live`), and a request for the
   account's 6-digit code stops the drive. Then A on the players' screen in
   Xbox Live mode (any other mode is backed out of; the local bots no
   longer join there, but in the lobby: §4.3, "The players screen"),
   then the Xbox Live menu. That is a window 5 `none` screen too, and
   it comes while the lobby mode is still the main menu's, so the drive
   tells them apart by the list mask (`list [0xf/...]` on the Live menu,
   `[0x1/...]` on the main menu) and by the screen address: the main menu
   is the window 5 screen the game was on, or bringing in (`pending`),
   when it set lobby mode -1. The third run (20261119-003044) took the
   Live menu for the main menu, pressed DOWN, A (OptiMatch, row 1), then
   DOWN, A on a playlist, and landed in OptiMatch's lobby (lobby mode 3,
   a session advertised). The Live menu's rows, ours included
   (`boot::custom_games`): 0 Quickmatch, 1 OptiMatch, 2 Clankermatch, 3
   Co-op Campaign, 4 Custom Game, 5 Custom Game Browser, 6 Content
   Download. The drive moves one row at a time, reading the row back off
   the list value, and presses A on row 4. The pregame lobby counts as the Live custom game
   only once a session is advertised (and not deleted) while the lobby
   mode is 2: `coop-live: lobby mode 2` alone is any custom game lobby,
   Split Screen's too, and other Live lobbies advertise as well. A lobby
   in another mode gets B, back to the Live menu and Custom Game again. Once
   every local bot is on Xbox Live, or after 90 s, it presses A on Start
   Game, and again (UP, A) if the phase has not left `menu` within 40 s.
   Each press is a `bots-drive:` line.
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

`checks` is the 12 checks of the list below; `state` is `pending` while
the run goes and `fail` for what did not pass once it has ended. `next`
is the first check not passed. An outside driver loops: read
`progress.json` (stop on `finished`), `h2 agent --socket <sock> ui` and
`h2 agent --socket <sock> frame snap 1` to see the screen, then
`h2 agent --socket <sock> press down` (or `a`, `b`, `up`) as the first
pending check and `last_ui` call for, and read `progress.json` again.

**The checks**, in the order a run reaches them. Each prints as passed or
failed with the log lines that show it, and the verdict names the first
failure:

| # | Check | Passes on |
|---|---|---|
| 1 | Signed in to Xbox Live | `live: playing as Tester` |
| 2 | Clankers on | `clankers: Clanker P on (...)` for each local port |
| 3 | On Xbox Live | the players' screen in mode 3 or the Xbox Live menu (`0xba`); a run that ended on the players' screen in Split Screen or System Link fails `not on Xbox Live: ended in Split Screen` |
| 4 | Hosting a Live custom game | `live: session ... advertised` (not since `deleted`) while the lobby mode is 2 (`coop-live: lobby mode 2`); a lobby without it says so, and so does an advertisement from another lobby mode (OptiMatch's 3) |
| 5 | Live roster of bot accounts | `clankers: asking Live for N bots for session S`, then `clankers: Live rostered bots for session S` |
| 6 | Local bots signed in (join gate) | per port, `bots: join gate port P: screen ... : signs in` and `bots: signing ... on port P ...`; it says whether on the players screen or in the lobby |
| 7 | Local bots on Xbox Live as bot accounts | per port, `clankers: Clanker P is G on Xbox Live (bot account 000b...)` and `bots: G on port P is on Xbox Live as its own bot account` |
| 8 | Start Game: start state ready | `bots: start state 9` (or 10, the countdown) |
| 9 | Gameplay on a map | `session.status` phase `gameplay`, not campaign; with `--map`, on that map |
| 10 | Phantoms joined the pregame lobby | `bots: phantom G joins (peer K, in the pregame lobby)` for each phantom, and no phantom failure; one that joined only once the game ran fails, saying so |
| 11 | The Live session holds 1 + N players | the session's filled slots, or the players the host reported joining (§9.5), reach 1 + N |
| 12 | The game record has the bots | at least N players with `bot: true` on the server's game record |

A failed check says what is missing and why, as far as the log does: for
step 6 each missing port's last join gate lines (screen, dialog,
`signed_in`); for 3 the mode it ended in, how often the account picker
opened without a sign-in, or the last presses and screen;
for 4 whether a lobby opened without a Live session; for 5 whether the
roster was asked for at all; for 7 the port's guest or offline line; for
8 the start state's blocking check, or that Start was never pressed; for
10 the phantoms' `waits` lines. Lines that look like trouble anywhere (a panic,
the guest thread exiting, a fault, a stuck driver, a refusal, a timeout)
are listed under `trouble:`.

**Where things go.** The merged log is
`<root>/logs/<time>/bots.log` and the game's whole stderr
`<root>/logs/<time>/Tester.log`; both paths print at the start and the
end. `h2 bots verdict bots.log` prints a log's verdict again (and exits
non-zero on a failure).

**Notes.**

- `--map` only checks the map played. The menus are not driven through
  Game Setup's Change Map, so the game plays the lobby's default map; a
  `--map` other than that fails step 9, saying which map was played.
- The phantoms join the pregame lobby (§5.1, "Phantoms in the pregame
  lobby"); step 11 counts the most filled slots, and the most joined
  players, the server saw over the whole run. Retail Halo 2 fills no slots (0 in run
  20261119-005001), so on a Live run the joined players are what counts.
- `h2 play` builds the game into `--out` when `--out` has none, or has one
  from another image. Run it with the same `H2_OPT` as that build, or the
  window recompiles first. The timeout starts once the game is up, so a
  build doesn't eat it.
- One game at a time: the server port and Tester's account are the run's
  own, but a second `h2 bots test` needs another `--server-port` and
  `--root`.

### 13.1 Clankermatch (`h2 bots test --clankermatch`)

The same harness runs §8's Clankermatch end to end, so the §8.5 checklist's
steps 4 to 9 have a verdict instead of a person at the controller.

```sh
H2_OPT=2 ~/.cargo/bin/h2 bots test --clankermatch [--playlist slayer|teams|doubles] \
    --disc disc/halo2.iso --wait-clang [--keep] [--timeout 900] [--no-drive]
```

`--playlist` implies `--clankermatch` (slayer by default); `--count` and
`--map` are refused, since the playlist sets both. Everything else
(`--keep`, `--root`, `--out`, `--server-port`, `--settings-from`,
`--dry-run`, `--no-drive`, `progress.json`, `h2 bots verdict`) is as
above; the merged log's plan line is `bots-test: clankermatch plan: ...`,
which is how `h2 bots verdict` knows which verdict to print.

**Server state.** None to set up: the run's fresh store has no published
hopper file, so the server serves the built-in lineup
(`live_server::default_hopper`), whose hoppers 8, 9 and 10 are Clanker
Slayer, Clanker Teams and Clanker Doubles, marked on
`GET /v1/hoppers/clankers`. The harness reads the marks before it starts
the game, checks the playlist asked for is marked and on the row the plan
expects (the Clankermatch list shows only the clanker hoppers, in index
order: rows 0, 1, 2), and stops at once if not. The game runs with
`--set clankers.enabled=false`: the page's Clankers stay off and only the
fill seats bots. The plan, from the lineup and `MATCH_LOCAL` (phantoms
only, §8.4):

| `--playlist` | Hopper | Row | Players | Bots | Local | Phantoms |
|---|---|---|---|---|---|---|
| slayer | 8 Clanker Slayer (FFA) | 0 | 2 to 8 | 7 | 0 | 7 |
| teams | 9 Clanker Teams (2 teams) | 1 | 2 to 8 | 7 | 0 | 7 |
| doubles | 10 Clanker Doubles (2 teams) | 2 | 2 to 4 | 3 | 0 | 3 |

**The drive** (`crates/cli/src/bots_drive/clankermatch.rs`): boot, sign-in
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

**The checks:**

| # | Check | Passes on |
|---|---|---|
| 1 | Signed in to Xbox Live | `live: playing as Tester` |
| 2 | Clanker hoppers listed (server and game) | `bots-test: server clanker hoppers [8, 9, 10]` and `live: clankermatch: clanker hoppers [8, 9, 10]`, both with the plan's hopper |
| 3 | The Clankermatch row opened the clanker playlists | `clankermatch: the playlists are the clanker hoppers`, and no `the server named no clanker hoppers: the list is empty` |
| 4 | The clanker search started | `live: clankermatch: searching clanker playlist H` with the plan's H |
| 5 | The search ended with this console hosting | `live: session S advertised` after the search, and `clankers: clankermatch: hosting clanker playlist H (session S)` |
| 6 | Local bots planned for the lobby | `clankers: clankermatch: N local bots join the lobby` with the plan's N (0 now: phantoms only) |
| 7 | With no local bot planned: no bot in the party | no `bots: signing ...` line anywhere (a bot signed in on a controller port shows in the party and fails the matchmade start's controller check) |
| 7 (with local bots) | Local bots seated and signed in, in the lobby | per local port, after the hosting line: `clankers: Clanker P on`, `bots: join gate port P: ...: signs in`, `bots: signing ... on port P ...`; it says whether in the lobby |
| 8 | With no local bot planned: the phantoms are the roster's bot accounts | a `clankers: G (phantom, rostered) on (...)` line per planned phantom; fewer says Live's roster was refused (host-made bots) |
| 8 (with local bots) | Local bots on Xbox Live as bot accounts | per port, `Clanker P is G on Xbox Live (bot account 000b...)` and `G on port P is on Xbox Live as its own bot account` |
| 9 | The lobby reached the playlist's fewest players | the most players the game's own session had (`bots: lobby players: N in the game's session`, the game session's `+0x1118`) between the hosting line and the start; the Live server's count (`bots-test: session ...`) is what the host told it and no longer counts (cm-run2 listed "4 joined" with no bot in the lobby) |
| 10 | The countdown ran and the game started | `session.status` phase `loading` or `gameplay` on a multiplayer map after the hosting line: not `map mainmenu` or mode `main_menu` (cm-run2's `loading` was the main menu coming back) |
| 11 | Gameplay on a map | phase `gameplay`, not campaign, after the start |
| 12 | Phantoms joined the lobby before the start | `bots: phantom G joins (peer K, in the pregame lobby)` for each phantom before the start, and no phantom failure |
| 13 | Game record: kind clankermatch | the newest `GET /v1/games` record (`bots-test: game ... kind K, playlist P`) has kind `clankermatch` and at least the local bots with `bot: true` |
| 14 | After the game, the local bots leave | `clankers: clankermatch: the game is over: the bots go` (or `this console hosts another game`) after gameplay, then `clankers: Clanker P off` for each local port |

A failed check says what the log shows instead and the likely cause: for
2 which side lacks the hopper and the HLE's own line (`no clanker hoppers
from the server`, `not asked for`); for 3 an empty list or a row never
pressed; for 4 a human playlist's search or another clanker hopper; for 5
whether a session was advertised at all (the game kept searching) or only
the fill missed it (the HLE did not read the advertisement's hopper as a
clanker one); for 7 the ports never seated (the roster) or seated but not
signed in (their last join gate lines); for 9 and 10 whether the lobby was
short of players or full and still not started (the game's matchmaking
start); for 12 phantoms that joined only once the game ran (the lobby
join off, or the lobby not seen as this console's) or not at all; for 13 the kind recorded; for 14 that the console
still hosts the session, or which bots did not sign off.

**Tests:** `cargo test -p cli bots_` (the verdict from log lines, the
driver's presses, the arguments; Clankermatch's under `bots_drive::clankermatch`,
`bots_verdict::clankermatch` and `bots_test_cmd`).
