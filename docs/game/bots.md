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
- "stuck" detection, which re-plans after 1 s without progress and jumps
  once before re-planning.

A move becomes a left-stick vector relative to the bot's facing. A bot
never warps.

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

- the 16-byte name field at block `+0x48` (single-byte characters:
  `0x190001` copies 16 and zeroes the 16th) set to the bot's name, which is
  ASCII and has no glyph;
- the control fields set so the brain's sticks mean the same on every
  machine (block `+0xfc` = slot `+0x114`, the record `0x190c34` reads):
  - flags bits 0 and 2 cleared (look inversion and the other flag `0x190c34`
    reads);
  - layout `+0x100` = 0 (Default);
  - thumbstick `+0x101` = 0;
  - look sensitivity `+0x102` = 3.

  The brain's aim spring is tuned against that sensitivity.
- The name the game shows comes from slot `+0xc1c` (UTF-16, 32 units,
  `0x1902c1`). The driver writes it after the sign-in.

Because the file is `-1`, nothing is saved. The bot leaves through
`0x18fc08`, called the same way, when the bot is removed or the game
returns to the main menu.

Whether a signed-in slot with its mask bit is enough for the game to make a
local player in Split Screen, System Link and Live custom games is the
first Run item of stage 1.

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

**Rumble.** The bots backend has no motors, so rumble to a bot port is
dropped.

### 4.4 What a local slot cannot do

- On **Live**, extra local players are guests of a signed-in account. Run
  item: whether the game's matchmaking allows guests at all, and how many.
  Retail OptiMatch allowed split-screen in some playlists. Custom games
  allow guests.
- No more than 3 bots per machine.
- The scoreboard shows guests as "Name(1)" style names under the host's
  account (Run: check how our Live names guests). Bots are then not
  separate accounts, so they cannot have their own rank. Clankermatch,
  which needs that, uses phantoms.

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
| `+0x4c` | membership update number |
| `+0x54` | peer count |
| `+0x58` | peers, 16 × `0x10c` (filled by `0x5fbd0`) |
| `+0x1118` | player count; `+0x111c` player mask |
| `+0x1120` | players, 16 × `0x13c` (added by `0x600f0`) |
| `+0x4980` | must be 4 for the player diff |
| `+0x4990` | most peers |
| `+0x72d8` | the local peer (skipped for views) |
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

**Recipe on the host, as built** (`bots::phantom::session`): the writes
the game's own functions make, done from the game thread before each
`RENDER_FRAME`, only while a multiplayer game runs, the simulation mode is
5 (distributed server), the session state is 5 and its class 4.

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
  lobby yet), what `0x62e30` and the veto object at `[S+0x78a8]` refuse,
  and whether `+0x72d8` is the host or the local peer.
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
| Own identity and rank | no (a guest of the host) | yes (§8) |
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
(`crates/live-telemetry`) records them as players. Their flag (§8) lets the
web and the stats split them out.

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
- `bot: true` on each bot's player entry, keyed by its `xuid`.

Per-player stats are the same as a human's. Persistent bots are ranked
from these by the server's normal per-playlist rank update, which treats
bots like humans. Whether ranked arbitration should count a bot's report
is sbmm-ranked's call: the host is the only reporter for its bots.

### 9.3 Trust

The roster is signed by the server like `MatchProposed`. A game record with
`bot: true` entries is accepted only for xuids that the server marked as
bots in that match's roster, or in the `/games/bots` answer. A console
cannot claim a human as a bot, or a bot as a human.

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
     has no profile, the bot taps A (6 frames in every 60).
     - The A reaches the screen's handler for a controller with no profile,
       `0x252a67` (`PLAYERS_SCREEN_UNJOINED`; its caller `0x252ad7` sends a
       controller with a profile to `0x25286d` instead).
     - For a bot's port, a hook answers in the handler's place. It copies
       the human's block with the bot's name and fixed controls, calls the
       game's `0x18fb34(port, block, -1)`, writes the slot's display name,
       and sets the port's bit in the screen mask.
     - Once signed in, the bot never presses A again, so it cannot move the
       human's menus on.
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
   - **The file.** `h2 bots nav <map|all> [--disc ISO] [--data DIR]` writes
     `<data>/bots/<map>.<bsp>.bots` (mesh and walls; magic `H2BT`). The
     data comes from the disc, so it stays on this machine and out of git.
     When the loaded map's name (`0x547898`) changes, the glue loads
     `<map>.0.bots` on a thread of its own. Until then, or with no file,
     the bots walk straight at their goal and see through walls, as in
     stage 1, and the log says to run `h2 bots nav <map>`.
   - Tests (`cargo test -p bots`): synthetic collision through the real
     generator (two floors sharing an edge, an L-shaped corridor, a ledge
     dropped off but not climbed, a low ledge jumped onto, a narrow gap
     jumped and a wide one not); locating the floor under a biped; the
     funnel turning at the inner corner; the follower walking the
     waypoints, jumping at a jump, and jumping once then re-planning when
     stuck; the file round trip, refusing damaged files.
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
     1. Once: `h2 bots nav all` (or `h2 bots nav lockout`). It prints each
        map's polygons, links and wall triangles, and writes
        `bots/<map>.0.bots` under the data root.
     2. `H2_BOTS=1 h2 play`, Split Screen, Slayer on Lockout. The log says
        `bots: lockout: N polygons, ...` when the map loads (not "no nav
        mesh").
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
       local game, they fail ("not the host"); with no room, the one that
       does not fit fails.
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
       the start; in force at the start for a join); an issued bot as a
       human is refused. `session_id` is allowed on custom games now (a
       game with bots names the session its roster came from); still not
       on campaign. `game::Player` has `bot: Option<bool>`.
     - Nobody else writes `bot-*`/`bots-*` or the two kinds.
     - Tests: `cargo test -p live-server --test bots`, `-p live-proto
       bots`/`game` (issued xuids and looks, host only, bounds, the
       roster in force at a game's start, refusals at start and on a join,
       persistent bots from the operator first in playlist rosters, their
       gamertags held).
   - **Still to do (6c), the console side:** the host asks for the roster
     (live-hle, with Live on) and turns each entry into a
     `bots::phantom::Identity` and a brain at its skill; telemetry marks
     phantom players `bot: true` with their rostered xuid. Clankermatch's
     `MatchProposed` carries the same roster (stage 8, with sbmm-ranked).
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
     Map) gets a "Clankers" row with action 11, under Quick Options and
     the Halo 1 Options row's 10, above the "Switch To:" rows (retail
     actions 6 to 8; `h1_options_row::insert_row` writes the rows'
     actions back in that order). It uses the same three points, and each
     hook is ordered after the Halo 1 row's. A press opens the page on the dialog's own
     stack. The row's text is the page's title slot.
   - **The bots.** While `enabled` is on, a driver keeps `min(count, free
     ports)` brains in the process-wide local bots, on ports 1 to 3 that no
     other bot asked for (an `H2_BOTS` or `sim` bot keeps its port). They
     join on the players screen like `H2_BOTS` bots. A change on the page
     applies at once:
     - a new difficulty reaches the bots already there at their next
       frame;
     - a higher count adds bots;
     - a lower count, or Off, takes the last added off. Taking a bot off
       unplugs its pad, which in a game may bring up the game's
       "controller disconnected" prompt, so change the count in the lobby.

     The setting persists, so with `enabled` left on, the bots are added
     at the next boot too, and join any players screen, campaign
     included.
   - Past the free ports the count adds phantom peers (stage 6b), which
     join once a hosted game runs; without phantoms (tests) it adds what
     fits and says so in the log.
   - Tests:
     - the settings in `setting_defs::ALL`, and off by default;
     - Mixed by port;
     - the driver: ports no one else asked for, the last added off first,
       only its own taken off, a difficulty change reaching the bots
       already there, and following the store's changes;
     - the page: a row per setting, steps saved in the store, wrapping;
     - the row: only a custom game's list, once, beside the Halo 1 row;
       on the plan after the Halo 1 row's hooks;
     - the kit's own checks over the page (`ui_kit::screens`: every
       screen, every input, every state; ids; layout).
   - **Game run (the user):**
     1. Run `h2 play` (no `H2_BOTS`). Choose Split Screen (or System Link),
        then a custom game's Game Setup. Below Change Map and the others,
        a "Clankers" row shows (after Halo 1 Options on a Halo 1 map).
     2. Press A on it: the Clankers page opens over the dialog, with
        seven rows and a description of the focused row. B goes back to
        Game Setup.
     3. Set Clankers to On and Count to 2 (the log says `clankers:
        Clanker 1 on (Normal)`). Bots join only on the players screen, so
        back out of the lobby to it: "Clanker 1" and "Clanker 2" join by
        themselves; go on to the lobby again. Note whether the lobby
        itself shows them without backing out (a run decides whether the
        lobby needs a join path of its own).
     4. Start Slayer: the two bots play as in stages 1 to 3, at the
        difficulty chosen.
     5. Quit to the lobby, open the page, set Difficulty to Legendary,
        and play again: the bots play harder.
     6. Set Count to 1: one bot leaves (its slot stays on the players
        screen until the game exits). Set Clankers to Off: none play.
     7. Quit and start `h2 play` again: the page shows what you left it
        at; `h2 settings` lists the `clankers.*` values.
     8. Send the `clankers:` and `ui:` log lines if the row is missing,
        draws no text, or opens the page under the lobby.
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
- `bots::local::remove(handle)` unplugs the bot's pad at the next input
  sync. Its slot stays signed in until the game exits.
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
5. `remove` on a local bot in a game unplugs its pad, which may bring up
   the controller-disconnected prompt.

## 11. What needs a game run

Each item is for the user to play, or for a coordinator run.

1. A bot port signing in through the players screen with an injected guest
   profile (§4.2), on Split Screen, System Link and a Live custom game.
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
