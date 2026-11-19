# Bots: Clankers and Clankermatch

Status: design (slice 1). Nothing is built. The decisions below are
final unless a game run proves one of them wrong. Everything that needs a run
is listed in §11.

What we want: multiplayer bots ("Clankers") that play Halo 2 multiplayer by
the game's own rules. They can be added to a custom game from Game Setup.
They can also fill a Live match in **Clankermatch**, a matchmaking row that
plays exactly like OptiMatch.

Labels: **traced** means read from the retail XBE with `h2 disasm`, at the
addresses given. **Repo** means it is already in this repository's code or
docs. **Design** means it is our decision. **Run** means only a game run can
settle it. No game data is in this document: it has addresses and paraphrase
only.

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

Both ways run on the host. Local slots come first: they need no network work
and can be tested on one machine. Phantom peers come after; Clankermatch
needs them.

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
- `0x6a3b0` has two more callers, `0x6a81c` and `0x978bb`. These are likely
  where remote players' actions arrive from the network. They are not read
  yet.
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
- A **phantom bot** has no pad. Its brain's pad state must become an action
  record before it is stored through `0x6a3b0`/`0x847d0`, keyed by the
  phantom's machine id.

  There are two ways to do the conversion. We could call `0x185be0` with a
  spare control-state slot as scratch, or we could write our own converter.
  **Design:** call the game's. Run it on one of the 4 per-user control states
  that no local user holds. If all 4 are taken, swap one out and back around
  the call. This keeps turn rates, sensitivity and aim assist the game's own.
  It is the same "only controller input" rule as local slots.

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
- Rows: Quickmatch 0, OptiMatch 1, Custom Game 2, then ours: Custom Game
  Browser 3, Co-op Campaign 4, Content Download 5.
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
  game-engine globals;
- shots (the `0x104150` hook) and kills (`0x15ce70`), for "who is shooting
  me" and for the emote triggers (§3.5).

**Line of sight** is a ray against the map's BSP collision, the same data
the nav mesh comes from. A bot sees an enemy only when all three hold:

- a ray reaches the enemy's head or chest;
- the enemy is inside the bot's view cone;
- a reaction delay has passed (§3.4).

The bot knows nothing it could not see or hear: a gunshot, a radar blip or a
teammate's callout. The radar uses the game's own rule: an enemy that
moves, and is not crouched, inside the radar range.

### 3.2 Navigation

Every map gets a nav mesh, built offline from the BSP collision by our own
tool and cached by map checksum. It is not committed, because it is derived
from the disc. The tool is `crates/pathfind`'s generator
(`pathfind::generate::generate`): today it emits H2 pathfinding sectors from
collision for converted maps. Bots need its sector graph, not the tag.

The mesh holds:

- walkable surfaces: collision planes with an upward-facing normal, joined
  across shared edges, with step-up and headroom checks for the biped's size;
- links:
  - **drop**: a ledge to a lower surface within the fall-damage limit;
  - **jump**: a gap or ledge reachable at the biped's jump arc, checked by
    sweeping the arc against collision;
  - **crouch-jump**: as jump, a little higher;
  - **teleporter**: from the scenario's teleporter netgame flags;
  - **lift**: man cannons and gravity lifts, from scenery and device
    positions;
  - **vehicle**: a later stage.

Paths come from A\* over the sector graph, smoothed with a funnel pass and
followed by steering:

- a look-ahead point;
- a corner slowdown;
- "stuck" detection and recovery (below).

A move becomes a left-stick vector relative to the bot's facing. A bot
never warps.

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

### 3.3 Decision

A utility layer runs about 5 times a second. It scores each action and
keeps the best, with hysteresis so the bot does not flicker between two.

| Action | Driven by |
|---|---|
| fight | a visible enemy; my health; my weapon against theirs at that range |
| chase | an enemy seen recently, its last known position |
| pick up weapon or powerup | how much better it is than what I hold; distance; the respawn timer |
| seek health | shields down, no enemy in view (cover, then a wait for recharge) |
| play the objective | gametype; my role (carrier, escort, defend, attack); team state |
| flee | outgunned, low health, a grenade nearby |
| emote | §3.5 |
| roam | the default: move toward the likely fight (spawns, weapon spots) |

**Built so far** (`Mind::step`, a fixed order rather than scores):

1. the weapons decision (§3.6) says go for a weapon: **rearm**;
2. an enemy in view: **fight**. With nothing to shoot it runs straight in
   and melees (B within 1.2 world units); with its shields under 0.25 it
   backs off while it shoots, until they are back to 0.9;
3. an enemy seen in the last 10 s, shields down: **recover**, backing
   away from where it was seen, facing that way;
4. an enemy seen in the last 10 s: **chase** its last known position;
5. otherwise **roam**.

Reload (X) and switch weapons (Y) are taps of 2 ticks, at most one every
2 s (reload) or 1.3 s (switch), on top of whichever of these runs.

Grenades (left trigger) need one in hand (unit `+0x23e`, §3.6, when read).
At the skill's rate in a fight at 3 to 12 world units, four times that rate
when another enemy is within 2.5 of the target; and at twice the rate at
where an enemy went out of view in the last 2 s, while chasing.

The **personality** parameters (aggression, caution, objective focus, teamwork)
scale the scores. Difficulty does not scale them: a Legendary bot can be
timid, and an Easy bot can rush.

### 3.4 Aim

Aim is a model of a human hand on a stick, not a snap to the target.

| Part | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| reaction time (see to start turning) | 450 ms | 320 ms | 240 ms | 180 ms |
| tracking error (std. dev. of the angle) | large | medium | small | very small |
| lead (fraction of the needed lead) | 0.3 | 0.6 | 0.85 | 1.0 |
| flick overshoot | large | some | little | little |
| recoil and burst control | sprays | bursts | paced | paced |
| grenades (arc solve) | rarely, poor | sometimes | often, good | often, good |
| weapon choice by range | no | partly | yes | yes |

Tracking runs a second-order spring that turns the right stick toward the
target, plus noise. A fully deflected stick turns no faster than the game's
own turn rate. The game's aim assist then applies, exactly as it does for a
human.

Lead comes from the target's velocity and the weapon's projectile speed,
read from the tag at load; hitscan needs no lead. Grenade arcs solve the
throw speed against gravity, and the bot accepts the throw only when the
arc misses collision. The numbers in the table are starting points; tuning
them needs runs (§11).

### 3.5 Emotes (the teabag layer)

The user asked: "make them teabag if you do something dumb". The emote is a
short burst of crouch presses over a body, made with the real crouch input.
Nothing else is changed.

These events count as a **dumb** death. The detector reads them from the
kill event, the shots hook and perception:

- the human died to their own grenade, to a fall, or by suicide, with the bot
  near;
- the human got stuck: no progress for 3 s against geometry, in the bot's
  view;
- the human missed a shot at point-blank range (under 3 m, three or more
  shots, no hit);
- the human ran out of ammo mid-fight (an empty magazine and no reserve
  while an enemy was in view);
- the bot killed the human in a 1v1 that the human started with a big
  advantage: shields full against the bot's broken, a power weapon, or
  attacking from behind;
- the human betrayed a teammate.

The bot may emote only when:

- the body is within 8 m;
- no enemy has been in the bot's view for 1.5 s;
- the bot is not carrying, defending or contesting an objective that is live
  (a flag at risk, a bomb armed, a hill or territory contested);
- it has not emoted in the last 30 s.

When it does, it presses crouch 3 to 6 times at a human rhythm (built:
130 to 200 ms per press), for 2 s at most, and stops at once if an enemy
appears. Built in stage 4 for self-kills and betrayals (§10).

How often it happens:

    chance = personality.taunt × severity(event)

`taunt` is 0..1 and comes from the personality. It is 0 when the Clankers
page's **Taunts** setting is off. A persistent bot has its own taunt value.

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
loaded `W+0x22c` of `+0xa`, reserve `W+0x22a` of `+0xc`. One with neither
is **not a gun**: the flag, the ball and the bomb (likely; their tags
were not looked at) and possibly the energy sword (not checked: if its
barrel ages it reads as a battery). The held weapon is the slot `U+0x212`
names; the other is the first other slot with a weapon.

`arms::pickups` walks the objects array (at most 2048) for live weapon
objects with no parent: placed weapons from the netgame spawns and dropped
ones alike. A pickup is **useful** when it is a gun with something in it.
A fault while reading weapons leaves the bot playing without the weapons
layer (`Me::arms` is `None`); it never blinds it.

**The decision** (`arms::plan`, pure, in order):

1. the held gun is dry (no rounds anywhere, or a battery under 2%):
   - the other has something: **switch** (Y);
   - an enemy in view within 2.5 world units: **melee** (run in, B);
   - a useful pickup within 30 world units, not blacklisted: **seek** it;
   - an enemy in view: **melee**; else nothing;
2. an empty magazine with a reserve: **reload** (X), in a fight or not;
3. calm (no enemy in view for 1.5 s) and under 35% of a magazine with a
   reserve: **reload**;
4. calm, the held gun under 20% of all it can carry and the other one too
   (or none): **seek** a useful pickup within 12;
5. otherwise nothing.

Seeking walks there along the nav mesh. A weapon of a kind it already
carries is walked over: the game moves its ammunition across by itself
(`0x101740`). One of another kind is taken by holding X on it (within 0.5
world units) for at most 1 s; the dry one in hand is dropped for it.

The presses are Halo 2's Default layout (X reload and action, Y switch,
B melee, A jump, left stick click crouch), which every bot's profile is
given whatever the human uses (`guest::bot_block`, §4.2).

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

The brain writes one `Gamepad` per bot into shared state each tick. The
backend's `poll` hands it back.

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
(`0x24b6ba` with `file = -1` for an unsaved profile, and `0x24b75b` after
loading a file), the settings commit (`0x18fd0c`, `0x18fd59`) and
`0x148b6b`. The sign-out is `0x18fc08`.

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
  by port (stage 7, "Their own look"), so it does not wear the human's.
- The name the game shows comes from slot `+0xc1c` (UTF-16, 32 units,
  `0x1902c1`). The driver writes it after the sign-in.

Because the file is `-1`, nothing is saved. The bot leaves through
`0x18fc08` (`PROFILE_SIGN_OUT`, slot in EAX), called the same way, once
it is removed and no game runs; its pad is unplugged only after that
(stage 7, "a lower count, or Off").

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
- [guess] That a sound at an active listener plays as a plain (not 3D)
  voice is read from the symptom, not traced: a 3D voice is never
  routed to the controller. The game run says.

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
- No more than 3 bots per machine.
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
1). Not yet named: which flag bits are crouch and jump.

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
5. **Actions**, each frame: the slot is the simulation slot whose MAC
   (`+0x18`) is the phantom's; the action goes in at `+0x2c` and the
   stamp at `+0x28` is the game tick, as `0x847d0`'s server path stores
   it, without its rounding. The server reuses it for 2 s, so ticks
   between frames are covered.
6. **Removal** (as `0x5fe20`): the peer's players out (mask, count, the
   peer's slots), later players' peer indices and the session's peer
   indices (`+0x40`, `+0x72d8`, `+0x50`) down one, the peers and channel
   entries above it moved down, the last cleared, `S+0x54` - 1, counters
   + 1. Not done: its tail call `0x5a2e0`. Phantoms leave last first.

**The action from a pad** (`session::action_from_pad`): yaw and pitch
are absolute world angles (`0x186f99`, `0x824d0`), so the right stick
turns from the unit's facing: yaw minus x, pitch plus y, at 3 rad/s
[guess, tuned in a run]. The left stick is the throttle (forward; strafe
[guess] positive to the left), the right trigger `+0x14` (fire), the left
`+0x18` (grenades), the weapon the unit's current (`+0x212`). The rest is
what passes `0x14d0a0`: the second weapon, grenade type and `+0x24` -1,
target and aim block zero. **Buttons are not mapped**: the flag bits come
from abstract buttons k0..k15 in `0x185be0` (k0 → `0x2`, k4 → `0x30`, k5
→ `0x4|0x100000|0x2000000`, k6 → `0x4000000`, k7 → `0x10040`, k12 →
`0x80`, k13 → `0x100`; `+0x1c` bits from k2 and k3), and which pad button
each k is is not traced. So a phantom does not jump, crouch or melee yet.

**Open, for a run**:

- **Clients and the phantom's address.** A real client opens a channel to
  every peer's XNADDR (`0x5cb80` → `0x76aa0`, `0x5d237`) and its drop loop
  (`0x5a090` → `0x5fda0`) runs in state 5, which is the running game too.
  So on a client a phantom may be dropped locally, in the lobby or in the
  game. If a run shows it, our network layer has to answer for phantom
  addresses on clients. The host is safe: its loop drops only connected
  peers.
- What the lobby's ready check (`0x5f970`) reads from a peer's properties
  (phantoms leave the session when the game ends, so they are not in the
  lobby yet), and what `0x62e30` and the veto object at `[S+0x78a8]`
  refuse. (`+0x72d8` is the local peer, `+0x40` the host: see "Hosting the
  game".)
- Colours and emblem in the configuration ([guess] `+0x40..+0x7b`); a
  phantom wears the host's until then.

### 5.2 Why phantoms are needed

- More than 3 bots, and up to the playlist's size (16).
- Bots that are separate accounts with their own name, emblem and rank
  (Clankermatch, persistent bots).
- A host whose own humans fill all 4 local ports.

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
| Pad output | to the port's pad merge | through `0x185be0` into a record, stored with `0x847d0` |
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
up to 3 (stages 1 to 4), and as phantoms after stage 6. The host, and every
client, sees them in the lobby's player list with their names and team
colours. Changing the page in the lobby adds or removes bots.

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
     carnage report draw;
   - on the site, it is drawn from the server's bot flag.

   The server knows which accounts are bots (§9). Every client patches the
   font, since every client is our recomp. A box on a stray unpatched font
   slot is the failure mode; the font-slot audit
   (keyboard-charset.md, "Which font draws which field") tells us which
   slots to patch.
3. **Bot rank.** Bots are matched to the player's rank. The playlist sets
   bot skill from the human's level, so it is a fair fight. The level maps
   to the aim table's difficulty (§3.4) on a continuous scale, not just its
   four steps: reaction, error and lead are interpolated.

### 8.3 Menu row (traced, Repo)

The Live menu's extra rows come from `custom_games.rs`'s hooks: list count,
add items, pane, row text `0x230f36` and row chosen `0x230f92`. Co-op
Campaign and Content Download were added this way. Clankermatch is one more
row placed after OptiMatch.

The row's choice runs the OptiMatch path (`0x231066`) with a flag set,
`clankermatch = true`, for the rest of the flow. The playlist UI
(`0x65530`) shows the server's hopper file. The server publishes clanker
playlists in that file as their own hoppers (§9), and the row filters to
them. That filter is the one new hook, at the playlist list's item
builder.

Row order shifts our later rows by one. `LAST_ROW` and the row numbers in
`custom_games.rs` move with it, and its tests cover that.

### 8.4 Search and lobby

The game searches through `XOnlineMatchSearch`, which is ours in live-hle.
For a clanker hopper:

1. The search returns no sessions after the delay. The game then hosts, as
   OptiMatch does when a search finds nothing.
2. The host then asks the server for the match's bot roster (§9) and adds
   the bots as phantom peers, one at a time, at random 0.5 to 3 s
   intervals, until the playlist's size is reached. Each one appears in the
   lobby as it is added.
3. The match starts on the hopper's normal countdown.

The lobby's "searching for players" state is the game's own: it stays until
the session is full.

**Run:** whether the game hosts after an empty search in every playlist
type, or needs the server's matcher to say "you host" (`MatchProposed`
with `host = me`). The second also works and is cleaner. **Design:** use
`MatchProposed` with `host = me`, and with the bots as members, when the
matcher exists (the sbmm-ranked lane). Use an empty search before that.

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
`Hle::set_session_players` tells each session the console hosts of them
and of its signed-in users, as `PlayerJoined` and `PlayerLeft` on
`session-<id>` for only what changed (`sessions::players_diff`; zero and
host-made xuids never). One more call after the roster ends sends the
bots' leaves. The server takes a `PlayerJoined` for a human account, or
for a bot of the session's latest roster when its host posts it
(`bots::leased_to`), and a `PlayerLeft` for anyone in the session, so a
replaced roster's bots can leave but not join again. The worker shares
the code (`live-worker` `rostered_bots_read_as_bot_accounts`).

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
`a_host_reports_the_bots_it_leased_as_its_sessions_players`, `-p live-hle
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

     Not yet: teleporters, lifts and crouch-jumps (later, with objectives);
     headroom (a ceiling over a floor is walked under as if it were not
     there).
   - **Paths**: A\* over the polygons, then the funnel (simple stupid
     funnel) over the edges' portals, with a drop or jump as a forced
     corner. `nav::Follower` steers along the waypoints: it moves on 0.3
     from each, jumps at a jump, and when no progress is made for 30 frames
     (1 s), jumps once, then asks for a new path.
   - **Walls** (`los::Walls`): every collision surface as triangles on a
     2 m grid, for line of sight (stage 3).
   - **The file.** `<data>/bots/<map>.<bsp>.bots`: magic `H2BT`, format
     version 2, the **stamp** (the generator version
     `cache::GENERATOR` and the source map's fingerprint), then the mesh
     and the walls. The data comes from the disc, so it stays on this
     machine and out of git. When the loaded map's name (`0x547898`)
     changes, the glue loads `<map>.0.bots` on a thread of its own. Until
     then, or with no file, the bots walk straight at their goal and see
     through walls, as in stage 1.
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
     the attacker and victim players, the betrayal flag and where the
     victim's unit lies (`emote::kills()`); each brain reads the log from
     its own cursor.
   - **Dumb** (`emote::classify`): killed by themselves or by the world
     (own grenade, fall, suicide: the kill function does not tell them
     apart; severity 0.8) and a betrayal (1.0). Not yet: stuck on
     geometry, point-blank misses, out of ammo and the lost 1v1 with a big
     advantage, which need the shots hook, ammo and shields.
   - Each dumb death with a body rolls `taunt × severity` once. The bot
     emotes over it only when the body is within 8 m (2.67 units), no enemy
     has been in its view for 1.5 s, no objective is live (always, until
     stage 5), and it has not emoted for 30 s; a death waits at most 5 s
     for that.
   - **The burst**: the bot stands still and clicks the left stick
     (crouch, Halo 2's default layout) 3 to 6 times, a press every 4 to 6
     ticks (130 to 200 ms) held for half of it, 2 s at most, and stops at
     once when an enemy comes into view.
   - **Taunt**: 0.3 (§9.1's default personality) for every brain; the
     Clankers page's Taunts off makes it 0 for its bots at their next
     frame. Phantoms do not emote yet: their actions carry no buttons
     (§5.1).
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
5. **Objectives.** CTF, Oddball, then King of the Hill, Assault and
   Territories: roles, carrier logic and defence spots from the netgame
   flags. Tests: role assignment and goal choice per gametype state.
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
     - `Phantoms::tick`, from `boot::bot_glue` before each `RENDER_FRAME`:
       while a multiplayer game this console hosts runs, pending phantoms
       get a peer and a player; each joined one's source is asked for its
       pad (`Frame { port: 4, machine: Some(id) }`) and the pad becomes its
       action. A removed phantom's peer comes out. **Phantoms join only
       once the game runs**: when it ends they leave the session and wait
       for the next game, so the lobby never holds one. On a client, or a
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
        colours. Do they spawn, turn, walk and shoot? They do not jump or
        melee yet.
     4. On the client: are they on the scoreboard, do they move, and do
        they stay for the whole game (§5.1, "Clients and the phantom's
        address")?
     5. Back in the lobby they are gone; the next game they join again.
     6. Send the `bots:` and `clankers:` lines, and anything odd in the
        turning: 3 rad/s and the strafe sign are guesses.
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
       writes `BotsRostered` on `bots-<session_id>`.
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
       (`clankers::roster_seats`): xuid, gamertag, emblem and colours as
       the `Identity`; the roster's team when not null; a brain at the
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
       a `bots::phantom::Identity` (xuid, gamertag, emblem, colours) and a
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
     - `taunts`: Off or On; default On.

     Teams, Aggression, Aim style and Taunts are saved but not used yet:
     they wait for the lobby teams (stage 6), the utility layer and aim
     styles (stages 4 and 5), and the emotes (stage 4).
   - **The page** is a kit screen, `clankers`: seven `ChoiceRow`s, laid
     out as the Halo 1 Options page is (the level select's list, the
     description pane, the legend in the footer). Its texts are the
     `clankers` id family. In the preview it is `h2 ui preview clankers`.
   - **The row.** A custom game's Game Setup dialog (any list with Change
     Map) gets a "Clanker Options..." row with action 11, under Quick Options and
     the Halo 1 Options row's 10, above the "Switch To:" rows (retail
     actions 6 to 8; `h1_options_row::insert_row` writes the rows'
     actions back in that order). It uses the same three points, and each
     hook is ordered after the Halo 1 row's. A press opens the page on the dialog's own
     stack. The row's text has a slot of its own after the page's footer
     (`KitScreen::row_label`, "Clanker Options..."), so the page's title
     stays "Clankers". The Halo 1 row does the same with its id 2
     (`h1_options::ROW_LABEL_ID`, "Halo 1 Options..."), its screen's title
     staying "Halo 1 Options"; both read like the dialog's other
     "...Options..." rows.
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
       game (`guest::game_tick` is `None`), before `SOUND_UPDATE`, the glue
       signs it out of its slot with the game's `PROFILE_SIGN_OUT`
       (`0x18fc08`, slot in EAX; a guest call, then the frame runs), and
       once the slot is empty it unplugs the pad
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
     in the low nibble). Each port has its colours and emblem
     (`guest::BOT_LOOKS`), an armour colour the human wears is moved one
     on, and the model is the human's.
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
       `PROFILE_SIGN_OUT` per port, then the frame;
     - the gate: only the bare players screen with an empty slot
       (`bot_glue` tests, `ui_watch` dialog tests);
     - the look: valid by `0x153750`'s checks, never the human's colours
       or emblem, distinct per port;
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
        seven rows and a description of the focused row. B goes back to
        Game Setup.
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
        left: its pad is unplugged`. Set Clankers to Off: none play.
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
8. **Clankermatch.**
   - The Live menu row, the playlist filter, the fake search and the lobby
     fill.
   - The bot glyph in the font, the roster (§9), and recording games as
     Clankermatch.
   - Tests: the row hooks (as `custom_games.rs` tests them); the roster
     parser; the fill schedule; team balance; the font patch on a
     synthetic font.
   - The server side is sbmm-ranked's.
9. **Vehicles** (later). Mounting, driving along the nav mesh's vehicle
   graph, and gunner roles.

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
  - It is called once a frame on the game thread, before `RENDER_FRAME`,
    only for placed bots.
  - `Frame` carries the frame count, the game's tick (`tick`, 30 a
    second, `None` in the menus), the port, the guest memory and
    `joining`: the players screen is on top and the bot's slot has no
    profile, so A signs it in. Frames are rendered frames; a slow frame
    runs several ticks and a fast one none, so time by `tick` to match the
    simulation.
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
  console hosts runs (§5.1). `Identity` is the roster's xuid, gamertag,
  emblem and colours (§9.1); emblem and colours are not applied yet.
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
7. CTF and Oddball roles on two or three maps.
8. A phantom peer seen correctly by a real System Link client: lobby,
   in-game, scoreboard, carnage report. The host's CPU with 15 phantoms.
9. The Clankers page in Game Setup: layout on the real screen, and its
   values persisting.
10. Clankermatch end to end: row, playlist, search delay, lobby fill, match,
    carnage report, the glyph on every name surface, the game on the site
    flagged as Clankermatch.
11. Whether an empty search makes the game host in each playlist type, or
    `MatchProposed` with `host = me` is needed (§8.4).

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

6. A bot that empties a magazine reloads at once, in a fight or not
   (the reload animation plays).
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
- The network receivers of remote actions, `0x6a81c` and `0x978bb`.
- The `0x5c` action record's flag bits (crouch and jump) and `+0x24`
  (zoom?); the layout is in §5.1.
- Which font slots draw player names in the lobby, scoreboard and carnage
  report.
- For the weapons layer (§3.6): whether the flag, ball, bomb and energy
  sword's `weap` tags have no magazines and no aging barrel (the sword
  should read as a battery to be sought); the `weap` field naming the
  multiplayer objective kind, to rule them out by name rather than by
  shape; the heat of plasma weapons (overheating reads as nothing today);
  and whether a pickup's respawn timer is readable (the netgame equipment
  index `+0xaf`, docs/live/live-gamestate.md).

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
   `--settings-from`'s `settings.toml` (by default `$TMPDIR/h2boot-data`,
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
   `0x24b664`). So the drive knows the picker by the bots' join gate line
   (`dialog 0x14 (window 3, ...)`), presses DOWN off row 0 and A on the
   account (A anyway when the cursor shows no move), once per opening, and
   gives up after 3 picks that did not sign in, or a picker still up 30 s
   after A. Another dialog on the way gets A, and a request for the
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
   (`boot::custom_games`): 0 Quickmatch, 1 OptiMatch, 2 Custom Game, 3
   Custom Game Browser, 4 Co-op Campaign, 5 Content Download. The drive
   moves one row at a time, reading the row back off the list value, and
   presses A on row 2. The pregame lobby counts as the Live custom game
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
| 10 | Phantoms joined | `bots: phantom G joins (peer K)` for each phantom, and no phantom failure |
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
- The phantoms join when the game starts, not in the lobby (§5.1); step 11
  counts the most filled slots, and the most joined players, the server
  saw over the whole run. Retail Halo 2 fills no slots (0 in run
  20261119-005001), so on a Live run the joined players are what counts.
- `h2 play` builds the game into `--out` when `--out` has none, or has one
  from another image. Run it with the same `H2_OPT` as that build, or the
  window recompiles first. The timeout starts once the game is up, so a
  build doesn't eat it.
- One game at a time: the server port and Tester's account are the run's
  own, but a second `h2 bots test` needs another `--server-port` and
  `--root`.

**Tests:** `cargo test -p cli bots_` (the verdict from log lines, the
driver's presses, the arguments).
