# Bots: Clankers and Clankermatch

Clankers join hosted System Link and Xbox Live matches as phantom peers.
Controller ports are reserved for people. This document covers the
phantom path, the retail traces behind it and game checks in §11. The
retail controller traces are in §4 and implementation notes are in §10.

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

## Pages

| Page | Sections | What |
|---|---|---|
| [perception and navigation](bots-perception-navigation.md) | 3.1 to 3.2 | What a bot sees and hears, and how it finds its way (sections 3.1 and 3.2). |
| [decisions, aim and emotes](bots-decision-aim.md) | 3.3 to 3.5 | How a bot picks a goal, aims and emotes (sections 3.3 to 3.5). |
| [weapons and objectives](bots-weapons-objectives.md) | 3.6 to 3.7 | Weapon choice, ammunition and the objective modes (sections 3.6 and 3.7). |
| [glitches](bots-glitches.md) | 3.8 | The glitches a bot performs and the superbounce routes it knows (section 3.8). |
| [practising and driving bounces](bots-bounce-practice.md) | 3.8 (continued) | How bots practise bounces, let players go first, chain and drive a player (section 3.8, continued). |
| [vehicles and Firefight](bots-vehicles-firefight.md) | 3.9 to 3.10 | Seat entry, driving and Firefight seats (sections 3.9 and 3.10). |
| [way in 1, local slots](bots-local-slots.md) | 4 to 4.4 | The controller-port path and what retail traces say about it (section 4). |
| [way in 2, phantom peers](bots-phantom-peers.md) | 5 to 5.1 | What a phantom peer has to make up (section 5.1). |
| [phantom runtime and custom games](bots-phantom-runtime.md) | 5.2 to 7 | Why phantoms, CPU per bot, the black screen, brain threads, the game record and Game Setup (sections 5.2 to 7). |
| [Clankermatch](bots-clankermatch.md) | 8 to 8.5 | The matchmaking row, search and lobby (section 8). |
| [the server interface](bots-server.md) | 9 to 9.5 | Roster, results, trust and bot accounts (section 9). |
| [stages](bots-stages.md) | 10 to 10.1 | What each build stage covers and the library surface (sections 10 and 10.1). |
| [game runs and testing](bots-game-run.md) | 11 to 13.1 | What needs a game run, next tracing and `h2 bots test` (sections 11 to 13). |

Each numbered heading below stays here so that section numbers cited from code and other pages resolve. The text is on the page named under it.

## Clanker appearance and Live player options

A Clanker's full gamertag determines its emblem and armour colours in
System Link, simulations and Live rosters. Changing hosts or account
XUIDs keeps the same look for the same name.
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
The menu parity check visits recent entries, verifies the Clanker card
class and reports missing widget text. [Crawling the menus](../runtime/agent-api.md#crawling-the-menus)

## 1. Decision

A Clanker's brain emits sticks, triggers and buttons. The host puts the
Clanker in the session as a phantom machine with one player. Each
simulation tick, the retail input reader and action builder turn its
arrived pad into an action for the remote gather. The game applies its
usual movement, aim assist and combat rules.

Clanker difficulty never multiplies damage. A Legendary Clanker and a
human deal the same damage with the same weapon, hit location and match
traits. Difficulty changes decisions and controller skill, including
reaction time, aim, shot placement and grenade timing.

A Normal Clanker uses the motion tracker to investigate movement, draws
a suitable weapon when rushed, melees at close range and looks for a
safe grenade against a group, an approaching enemy or recent cover.
The game decides whether each swing, shot, throw or pickup succeeds.
Read a running host with `h2 agent bots.stats`. Uploaded simulations
show their results in Clanker Training.

Custom games on System Link or Xbox Live, Clankermatch and headless sims
all use phantom peers. A match has up to 16 players, subject to the
session and playlist limits. A playerless sim has room for 15 bots
because its host occupies one peer slot. See [the phantom path](#5-way-in-2-phantom-peers).

AI actors are a different retail feature. An `actr` with a biped
doesn't participate in scoring, objectives or the carnage report. The
navigation mesh uses the project's pathfinding code (§3.2).

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

  One machine has at most four local users. Each phantom occupies a
  separate peer slot.
- **Local slots.** 4 entries of `0xc70` bytes at `0x54e8e0`. Flag `0x10`
  (bit 4) means a profile is loaded, and `0x20` (bit 5) means signed in to
  Live (§4.2). The profile block is at `+0x18`.

### 2.2 The retail action path (traced)

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

A phantom supplies its pad when the host gathers the remote slot for
simulation. After uplink delivery the same retail reader, layout mapper
and action builder run in the phantom's private local-user view. See
§5.1, “The action from a pad”, for the fields and timing.

#### Multiplayer round start (Built)

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

### 2.3 Views (traced)

`render_game_views` `0x2234e0` takes its view count from `[0x4e8c20]+8`,
clamped to 1..4. The split layout comes from `[0x4e6948]` and `[0x485ac2]`
(two views give layout 1 or 2). For each view it takes the next local user
whose map entry is not -1, and calls
`0x223080(view i of n, layout, …, 0x4e9c8c + u*0x358)`.

Phantom players have no host local-user entry. The host's views, HUD
and sound listeners use the humans' local-user mappings.

### 2.4 AI (traced)

`ai_update` `0x1c83c0` checks the enable byte at `[0x4f55d0]`
(`0x1c83d8`, [ai-control.md](../halo1/ai-control.md)) and the game state
at `[0x4e6948]+0xc`, where 4 suppresses updates. The actor array is at
`[0x4f55f0]`, stride `0x888`. MP scenarios carry no characters, squads or
`pathfinding` blocks (Repo: the h1ai and pathfind work generate exactly
these for converted maps). This is why option (c) would need a whole AI
layer before its bots could even stand still.

### 2.5 Menus (traced, Repo)

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
row. That row opens the shared settings dialog `h1-options` (Sandbox and
HUD; docs/game/ui-builder.md "The settings dialog"), whose values are
kept in the settings store (`crates/extensions/game/src/h1_options.rs`).

## 3. The brain

The brain is a pure function, run once per tick for each bot:

    (perception, memory, params) -> (pad, memory')

It runs on the host's game thread, after the tick's world state is final and
before `0x1870a0` reads input. It has no knowledge of which way in it uses.


### 3.1 Perception

See [perception and navigation](bots-perception-navigation.md#31-perception).

#### Visual range

See [perception and navigation](bots-perception-navigation.md#visual-range).

#### Visibility grid (`bots::vis`)

See [perception and navigation](bots-perception-navigation.md#visibility-grid-botsvis).

#### The motion tracker

See [perception and navigation](bots-perception-navigation.md#the-motion-tracker).

### 3.2 Navigation

See [perception and navigation](bots-perception-navigation.md#32-navigation).

#### Traversal audit by map

See [perception and navigation](bots-perception-navigation.md#traversal-audit-by-map).

#### Belief and map knowledge

See [perception and navigation](bots-perception-navigation.md#belief-and-map-knowledge).

#### Pace and hops

See [perception and navigation](bots-perception-navigation.md#pace-and-hops).

### 3.3 Decision

See [decisions, aim and emotes](bots-decision-aim.md#33-decision).

#### Finishing and breaking off

See [decisions, aim and emotes](bots-decision-aim.md#finishing-and-breaking-off).

#### Personality and temperament

See [decisions, aim and emotes](bots-decision-aim.md#personality-and-temperament).

### 3.4 Aim

See [decisions, aim and emotes](bots-decision-aim.md#34-aim).

#### Phantom aim diagnosis and next steps

See [decisions, aim and emotes](bots-decision-aim.md#phantom-aim-diagnosis-and-next-steps).

#### 3.4.1 The aim net (compared: the code aim stays)

See [decisions, aim and emotes](bots-decision-aim.md#341-the-aim-net-compared-the-code-aim-stays).

### 3.5 Emotes (the teabag layer)

See [decisions, aim and emotes](bots-decision-aim.md#35-emotes-the-teabag-layer).

### 3.6 Weapons and ammunition (traced)

See [weapons and objectives](bots-weapons-objectives.md#36-weapons-and-ammunition-traced).

### 3.7 Objectives (traced)

See [weapons and objectives](bots-weapons-objectives.md#37-objectives-traced).

#### A script's goals

See [weapons and objectives](bots-weapons-objectives.md#a-scripts-goals).

### 3.8 Glitches (built, not checked in a game)

See [glitches](bots-glitches.md#38-glitches-built-not-checked-in-a-game).

#### Superbounce routes

See [glitches](bots-glitches.md#superbounce-routes).

#### Breaking the glass

See [glitches](bots-glitches.md#breaking-the-glass).

#### Practising bounces

See [practising and driving bounces](bots-bounce-practice.md#practising-bounces).

##### How each bot practises

See [practising and driving bounces](bots-bounce-practice.md#how-each-bot-practises).

##### Fumbles

See [practising and driving bounces](bots-bounce-practice.md#fumbles).

##### Letting players go first

See [practising and driving bounces](bots-bounce-practice.md#letting-players-go-first).

##### Chains

See [practising and driving bounces](bots-bounce-practice.md#chains).

##### Monsters

See [practising and driving bounces](bots-bounce-practice.md#monsters).

#### Tuning the bounce

See [practising and driving bounces](bots-bounce-practice.md#tuning-the-bounce).

#### Driving a player

See [practising and driving bounces](bots-bounce-practice.md#driving-a-player).

### 3.9 Vehicles

See [vehicles and Firefight](bots-vehicles-firefight.md#39-vehicles).

### 3.10 Firefight

See [vehicles and Firefight](bots-vehicles-firefight.md#310-firefight).

## 4. Way in 1: local slots

See [way in 1, local slots](bots-local-slots.md#4-way-in-1-local-slots).

### 4.1 The bot's pad (traced, Repo)

See [way in 1, local slots](bots-local-slots.md#41-the-bots-pad-traced-repo).

### 4.2 Signing a bot in (traced)

See [way in 1, local slots](bots-local-slots.md#42-signing-a-bot-in-traced).

### 4.3 Hiding the views (traced)

See [way in 1, local slots](bots-local-slots.md#43-hiding-the-views-traced).

### 4.4 What a local slot cannot do

See [way in 1, local slots](bots-local-slots.md#44-what-a-local-slot-cannot-do).

## 5. Way in 2: phantom peers

See [way in 2, phantom peers](bots-phantom-peers.md#5-way-in-2-phantom-peers).

### 5.1 What has to be made up (traced)

See [way in 2, phantom peers](bots-phantom-peers.md#51-what-has-to-be-made-up-traced).

### 5.2 Why phantoms are needed

See [phantom runtime and custom games](bots-phantom-runtime.md#52-why-phantoms-are-needed).

### 5.3 CPU per bot

See [phantom runtime and custom games](bots-phantom-runtime.md#53-cpu-per-bot).

### 5.4 Waiting for the black screen

See [phantom runtime and custom games](bots-phantom-runtime.md#54-waiting-for-the-black-screen).

### 5.5 Brains off the game thread

See [phantom runtime and custom games](bots-phantom-runtime.md#55-brains-off-the-game-thread).

## 6. Phantom players in the game record

See [phantom runtime and custom games](bots-phantom-runtime.md#6-phantom-players-in-the-game-record).

## 7. Clankers in custom games (the Game Setup page)

See [phantom runtime and custom games](bots-phantom-runtime.md#7-clankers-in-custom-games-the-game-setup-page).

## 8. Clankermatch

See [Clankermatch](bots-clankermatch.md#8-clankermatch).

### 8.1 What the player sees

See [Clankermatch](bots-clankermatch.md#81-what-the-player-sees).

### 8.2 Decided by the user

See [Clankermatch](bots-clankermatch.md#82-decided-by-the-user).

### 8.3 Menu row and playlists (traced, Repo; built)

See [Clankermatch](bots-clankermatch.md#83-menu-row-and-playlists-traced-repo-built).

### 8.4 Search and lobby (built: the empty search)

See [Clankermatch](bots-clankermatch.md#84-search-and-lobby-built-the-empty-search).

### 8.5 Game run (checklist)

See [Clankermatch](bots-clankermatch.md#85-game-run-checklist).

## 9. The server interface (for sbmm-ranked)

See [the server interface](bots-server.md#9-the-server-interface-for-sbmm-ranked).

### 9.1 The roster: server to host

See [the server interface](bots-server.md#91-the-roster-server-to-host).

### 9.2 Results: host to server

See [the server interface](bots-server.md#92-results-host-to-server).

### 9.3 Trust

See [the server interface](bots-server.md#93-trust).

### 9.4 Interface with ranks (answers to live-matchmaking.md §8.4)

See [the server interface](bots-server.md#94-interface-with-ranks-answers-to-live-matchmakingmd-84).

### 9.5 Bot accounts

See [the server interface](bots-server.md#95-bot-accounts).

## 10. Stages

See [stages](bots-stages.md#10-stages).

## 10.1 Library surface (for the agent API)

See [stages](bots-stages.md#101-library-surface-for-the-agent-api).

## 11. What needs a game run

See [game runs and testing](bots-game-run.md#11-what-needs-a-game-run).

### 11.1 The brain in play (checklist)

See [game runs and testing](bots-game-run.md#111-the-brain-in-play-checklist).

### 11.2 Objectives in play (checklist)

See [game runs and testing](bots-game-run.md#112-objectives-in-play-checklist).

### 11.3 Decisions boost in play (checklist)

See [game runs and testing](bots-game-run.md#113-decisions-boost-in-play-checklist).

### 11.4 Vehicles in play (checklist)

See [game runs and testing](bots-game-run.md#114-vehicles-in-play-checklist).

### 11.5 Traversal and sandbox movement in play (checklist)

See [game runs and testing](bots-game-run.md#115-traversal-and-sandbox-movement-in-play-checklist).

## 12. Next tracing (offline)

See [game runs and testing](bots-game-run.md#12-next-tracing-offline).

## 13. Testing (`h2 bots test`)

See [game runs and testing](bots-game-run.md#13-testing-h2-bots-test).

### 13.1 Clankermatch (`h2 bots test --clankermatch`)

See [game runs and testing](bots-game-run.md#131-clankermatch-h2-bots-test---clankermatch).
