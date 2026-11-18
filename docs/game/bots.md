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

When it does, it presses crouch 3 to 6 times at a human rhythm (120 to
220 ms per press), for 2 s at most, and stops at once if an enemy appears.

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

### 5.1 What has to be made up

A phantom is a remote machine with one user. What the host needs for it:

1. **A machine.** A free bit in `[0x4e8c20]+0x2c` and a 6-byte machine id
   in `+0x30` (our own bytes, e.g. a `C1A4` prefix plus the bot index).
2. **A session peer.** A slot in the session's peer array
   (S `+0x58`, 16 × `0x10c`, count at `+0x54`), marked connected and
   established, with its player in the player count (`+0x1118`).
3. **A player.** Created by the game's own membership update (`0x14c090`
   from the `0x14bb..` path), given the peer's player configuration: name,
   team, colours, emblem.
4. **Actions.** Each tick, after `0x1870a0`, the bot's pad goes through
   `0x185be0` into a `0x5c` record, stored with `0x6a3b0`/`0x847d0` under
   the phantom's machine id (§2.2).
5. **Silence.** Nothing is sent to a phantom (our `network.rs` drops sends
   to its address), and the timeout and ack checks never drop it: it always
   looks fully up to date.

**Design:** make phantoms in memory, on the host, by driving the game's own
membership functions. Do not emulate the wire: no fake XNet peer and no
fake join messages. The wire format of the session join, channels, acks and
distributed updates is big and not traced. A phantom only has to fool the
host, because only the host simulates authoritatively. The clients learn
about the phantom's player and its updates from the host's normal
distributed updates, exactly as they learn about any other player. A wire
emulation would only be needed for a phantom that lives on a client, and we
never do that.

The open part is step 2/3: which session fields mark a peer as joined, and
whether the membership update can add a peer without a join handshake.
That is the next tracing slice (§10).

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
custom game the host asks `POST /games/bots` with
`{playlist?: hopper_id, count, skill?, teams}` and gets one back; without a
server (System Link, offline) the host makes its own from the Clankers page.

```json
{
  "match_id": "…",
  "bots": [
    {
      "xuid": "0009…",            // a real account id; bots are accounts
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

## 10. Stages

Each stage ends in tests that can be run without the game, plus a short
list for the user to play (§11).

1. **Minimal bot (local slot).**
   - The bots backend puts 1 to 3 bot pads on the ports after the human's
     (§4.1).
   - On the players screen, the driver signs each bot in with `0x18fb34`
     and sets its bit in the screen mask (§4.2). It signs them out with
     `0x18fc08`.
   - The render and listener windows hide the bots' views (§4.3).
   - The brain is trivial:
     - turn toward the nearest living enemy and walk at it;
     - hold the trigger while that enemy is within 15 degrees of the aim
       and 10 world units (about 30 m);
     - there is no line of sight yet, so it walks into walls.
   - Switched on by `H2_BOTS=<n>` (1 to 3) for this stage; the Clankers
     page replaces that in stage 7.
   - Tests over synthetic guest memory:
     - the view window: count and map in, the bot users hidden, the same
       values restored after, the human's order kept;
     - the sign-in block: the human's block copied, the name and control
       fields set, everything else untouched;
     - the slot and screen-mask writes;
     - the backend: the pads come after the real ones and carry the
       brain's state;
     - the brain: stick and trigger output from synthetic positions and
       facings.
   - **Game run (the user):**
     1. `H2_BOTS=1 h2 play`, then Split Screen. On the players screen a
        second player appears, named "Clanker 1", without anyone pressing
        anything.
     2. Start a Slayer game on a small map (Lockout or Ivory Tower). The
        screen is not split: you see one full-screen view and your own HUD.
     3. You hear only what is near you, not what is near the bot.
     4. The bot spawns, turns to you, walks at you, and shoots when you are
        in front of it. It shows on your radar and on the scoreboard.
     5. Killing it and being killed by it count on the scoreboard and in
        the carnage report.
     6. Repeat with `H2_BOTS=3`: three bots, one view, all on the
        scoreboard.
     7. Back at the main menu, Split Screen shows one player again (the
        bots are signed out).
     8. Send the log lines starting `bots:`.
2. **Navigation.**
   - The nav mesh tool runs on BSP collision, with a cache.
   - Path following and stuck recovery.
   - Tests: synthetic collision (rooms, ramps, a gap to jump, a drop, a
     teleporter pair); A\* and the funnel; a steering simulation with a
     simple biped model.
3. **Combat.**
   - Perception with line of sight, the utility layer, and the aim model
     with skill levels.
   - Weapon choice and grenades.
   - Tests:
     - the aim model's error and reaction statistics per skill, against a
       fixed seed;
     - grenade arcs over synthetic collision;
     - utility choices in scripted situations.
4. **Emotes.**
   - The dumb-death detector and the emote layer (§3.5), with the Taunts
     setting.
   - Tests: each trigger from synthetic events; the safety gates; the press
     rhythm.
5. **Objectives.** CTF, Oddball, then King of the Hill, Assault and
   Territories: roles, carrier logic and defence spots from the netgame
   flags. Tests: role assignment and goal choice per gametype state.
6. **Phantom peers.**
   - Trace the membership and session-join path (§5.1, steps 2 and 3).
   - In-memory phantom machines, peers and players; action conversion and
     storage; network drop and keep-alive.
   - Tests: the action conversion and store as pure functions over
     synthetic memory, as the co-op work did; the machine-id and peer
     bookkeeping.
7. **Clankers page (Game Setup).**
   - The ui_kit page and its settings (§7); bots joining the lobby.
   - Tests: the kit's Page table tests and previews; settings round-trip.
   - It can move ahead of stage 6 if the user wants it sooner, limited to
     3 bots.
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

- The membership update `0x14bba8`/`0x14bc17`/`0x14bf84` → `0x14c090`, and
  the session fields that mark a peer joined (for phantoms).
- How the players screen's mask becomes the game's local users when the
  game starts. A run answers it for stage 1 (stage 1 run, step 1); a trace
  is needed only if that fails.
- The network receivers of remote actions, `0x6a81c` and `0x978bb`.
- The `0x5c` action record's layout, for the converter's tests (the crouch
  bit and the weapon index first).
- Which font slots draw player names in the lobby, scoreboard and carnage
  report.
