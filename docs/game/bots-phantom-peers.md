# Bots: way in 2, phantom peers

What a phantom peer has to make up (section 5.1). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

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

**Hosting the game** (traced, `h2 disasm`). Runs
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

**A phantom's player configuration** (traced from the
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
- **A ranked hopper's rank.** The lobby row's rank is the player's
  host-side fields (`+0x84..+0x90`), which a person's console fills from
  its stats read. A phantom's stay -1, which shows no rank, unless its
  hopper is a ranked clanker one (`ClankerHopper.ranked`): then the driver
  gives each phantom a rank (`Phantoms::set_rank`) and its join writes it
  under the host's hopper index (`session::config_in_ranked`: `+0x88` and
  `+0x8a` the level as stored, 0 to 49, `+0x8c` the experience that level
  starts at). The level is the roster's `rank.level`: a persistent bot's
  standing in the hopper's rank playlist (`ranks_as` or its own, the
  playlist the roster endpoint looks up and games rank in), or the humans'
  for a made-up bot. A host-made Clanker, which Live has no record of,
  starts at `clankers::starting_level`: a stable draw from its name inside
  its difficulty's band (Easy 1 to 10, Normal 11 to 25, Heroic 26 to 40,
  Legendary 41 to 50), never saved. An unranked hopper's phantoms keep
  -1. Tests: `a_ranked_phantoms_lobby_row_has_its_rank` (`bots`),
  `a_ranked_clankermatchs_phantoms_carry_a_rank` and
  `an_unranked_clankermatchs_phantoms_carry_no_rank` (`game`).
- **Kept none.** Each frame a seated phantom whose requested or
  effective handicap is not 0 (a seat taken back from an older run, a
  reclaimed peer) has both put back to 0, nothing else of the record
  touched (`write_no_handicap`); it is a repair like a name the host
  rebuilt, counted in `bots.stats` as `handicap_writes`. The host
  changing its own handicap later changes nothing of a phantom's.
- **Telemetry.** Live telemetry and the match summary show no handicap
  per player, so there is nothing of a bot's to hide there.

Tests: `phantom::session::tests`
(`a_hosts_handicap_and_profile_never_reach_a_bots_configuration`,
`lobby::a_hosts_handicap_is_never_a_phantoms_then_or_later`,
`lobby::a_seated_phantoms_handicap_goes_back_to_none`).

**Phantoms in the pregame lobby** (traced from the
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
  wrote and each kind of repair ([agent-api.md](../runtime/agent-api.md#bot-combat)).
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

**Phantoms in the game** (traced after a
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

**Phantom audio.** `crates/extensions/game/src/bot_glue/audio.rs` uses the
Phantom machine ID on the player datum to exclude any Phantom entry from the
local-user map while `SOUND_OWNER_USER`, `SOUND_LISTENERS_UPDATE` and
`SOUND_UPDATE` run. It restores the map on return. Human slots keep their
original user indices, including in splitscreen.

The listener refresh runs both inside the sound frame and from the effect
update. Both entry points clear the active flag for Phantom and unmapped
slots.

The sound owner check leaves a Clanker's attached weapon and unit sounds
positioned. The retail mixer selects a human listener and handles
distance, panning and obstruction. A human's first-person sound uses the
retail owner check. The [agent's active listener list](../runtime/agent-api.md#audio-voice-routing)
shows the slots and camera positions the game can select.

**The user's Halo 1 run (Live custom game on
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

#### The action from a pad

The offsets below are relative to one `0x5c`-byte action, in little-endian
order. The local and remote paths can be followed from the generated
functions for `0x185be0`, `0x186ab0` and `0x84990`. The Default layout
initializer (`0x218420`) assigns each abstract action slot `k` a physical
button.

The input pass at `0x217e40` copies each button's down count and elapsed
milliseconds into its slot. The control builder copies those arrays at
`0x186570` and tests the relevant `k`. Finally `0x187029..0x187083` writes
the record that the host consumes at `0x14d6d4..0x14d7e7`.

The host translates that record to unit controls at `0x1505f0`. This is the
same Default layout local Clanker profiles use. [Controller layout
trace](../input/controller-layouts.md)

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
and trigger pressure. The XInput reader at `0x124c53..0x124cc9` tests the
last pressure byte against a per-button threshold and uses the new byte to
update that threshold. A phantom uses the same pressure history and
publishes the new sample for the current tick.

While down, the threshold can rise to pressure minus 32. While up, it can
fall to pressure plus 64. Stick clicks and the D-pad are digital.

X and Y send their levels on every tick. The host compares them to player
`+0x3c` and derives the press/release bits in `0x14f722..0x14f81a` (X
`0x04/0x08`, Y `0x40/0x80`). The threshold comes from the active globals'
player-control block, `[[0x4e034c]+0xf4]` at `+0x7c`, in seconds.

The local builder multiplies by 1000, truncates, and compares to the pad's
accumulated milliseconds. A phantom measures the hold in simulation time.

Exact agreement at the threshold with an asynchronously polled physical
controller is **[guess]**. The named button fields above are traced.

**Selectors, target and motion.** At `0x187036` the local builder packs a
weapon-set generation into `+0x1e` (u16) and the requested primary and
secondary indices into `+0x20/+0x21` (i8, −1 or 0–3). Unit `+0x212/213` are
the actual hands and `+0x216/217` are the requests. At `0xc6e2b` the unit
accepts the two indices only if the generation matches unit `+0x214`.

An objective weapon returned by `0xcfe40` can override the requested primary
(`0x186c0e`). The weapon handles at unit `+0x218` also feed the zoom check
for a deflected aim stick (`0x100f70`). Player flags at `+2` tell Y release
whether an interaction consumed the press.

Weapon, grenade and zoom ranges are checked by `0x14d0a0`. The local
interaction finder `0x151110(player, result)` returns the 12 bytes copied at
`+0x28`.

These include the interaction kind (i16), detail at `+0x2a` (seat for a seat
interaction), object datum at `+0x2c` and melee target at `+0x30`
(`0x151d41`, `0x15166c`, `0x14fd37`). An empty target has kind zero and both
datums −1.

The separate shot-assist block starts at `+0x34`. The local action builder
copies nine dwords there at `0x187074..0x187083`. Previously the phantom
always supplied the empty block from `0x14d040`. Both strengths were
zero, so the barrel's target check at `0x1a6a8b..0x1a6aa5` skipped
shot magnetism even when the reticle was close enough for a local player.

Both seats use the retail XInput reader `0x124c00`, profile/layout routines
`0x190c34` and `0x217c40`, and the full player action builder `0x186ab0`.
The builder indexes the user's 0x94-byte control history by player +0x28. It
calls `0x185be0` for look response (including acceleration, slowdown and
follow) and `0x1a4900` for weapon parameters and target search (`0x1a50a0`,
`0x1a5370`).

It writes yaw/pitch at +4/+8, trigger flags at +0 and the selected target
and strengths at +0x34..+0x57. The host validates the returned record at
`0x14d0a0`.

The retail tables have four entries. At remote gather, a second executor
uses a copy-on-write snapshot of the host world. In that view local user and
controller zero, the two player maps at `[0x4e8c20]+0xc/+0x1c`, and the
player's +0x24/+0x28 all refer to the phantom.

Its virtual XInput handle supplies the arrived pad to `0x409b6c`. The seat
brings its own raw input, button and control history between ticks. A fresh
binding clears the previous user's target and strengths before retail binds
the unit at `0x1872d0`.

The private profile slot has the profile bit and local-user index zero even
if the host has nobody signed in. `0x1557c0` supplies the unit or vehicle
camera origin for the observer at `0x4e9c8c`. Only the validated action
crosses back to the host.

A repeated gather in one tick reuses that action. A phantom samples once per
game tick and advances the button clock by elapsed simulation milliseconds.

**The facing release.** One write of the builder's must reach the live
world. When something other than the control turns a biped, such as a melee
lunge, the biped's facing update `0xe01d0` sets bit 5 (`0x20`) of biped
flags `+0x348` (`0xe0c47`). For a biped on foot with that bit set, the
control update in `0x186ab0` reseeds the control yaw and pitch from the
unit's aim `+0x168` with `fpatan` and then clears the bit
(`0x186f8b..0x18700b`).

The reseed overwrites the look delta that `0x187510` added from the right
stick. For a local player the clear lands in the live unit, so the reseed
happens once. The phantom's private build discards it.

With the bit left set, every later build reseeds the look from the aim and
drops the right stick, so the bot's aim freezes until it dies, while its
movement, fire and other buttons still work. After a built action,
`slot_action` in `crates/live/bots/src/phantom/action.rs` therefore clears
the bit on the phantom's live biped and counts it in `bots.stats`
`maintenance.facing_releases`. Film replay takes the same path.

A missed build leaves the bit set for the next build. The test is
`a_built_action_releases_the_bipeds_facing_flag`.

In the Clanker Team Slayer game `09af571f9fec7f1a33ed37edb397a140`,
before the release, every bot's aim stayed within 0.001 of one vector
from its first melee until death or the end of the match. That covers
31% of all bot frames. In those frames the bots scored 2.6 hits a minute,
against 19.3 in the rest, and their median angle between walking and aim
was 90–115°. Live telemetry shows the release working when no bot's aim
stays unchanged for more than 60 frames after an action that includes
`melee`.

The first 12 sims with a phantom seat stopped between ticks 91 and 1945. In
the Lockout log, `0x2d6307` is the return from kernel ordinal 234 inside
XAPI's `WaitForSingleObjectEx`. Thread 3’s `0x120c4f` is the return from
waiting on the async I/O work semaphore at `[0x4e0360]`.

The two stacks identify the same wait wrapper. The log does not identify the
object the private builder waited on.

The builder’s inventory check (`0xcdeb0`) can look up a weapon animation
at `0x1cc3f0` and predict its resources at `0x1236f0`. A cold read
allocates a job under the pool mutex (`0x1208b0`) and queues it
(`0x1209c0`). A pose lookup can also demand animation bytes through
`0x1dd82b` → `0x1237e0` and wait for an outstanding read at `0x1237a0`.
Which branch triggered the original refusal is **inferred, needs a run**.

The private builder skips animation prefetch. For a demanded resource it
uses the retail getter if the bytes are resident. On a cache miss or pending
I/O it checks the animation and resource against their live graph, calls the
retail getter on the scheduled game thread, then retries the same pad once
against fresh RAM. The live worker can complete that job.

A further miss or refused call discards the private build. The hook logs the
first failure of each kind and revalidates the same unit’s previous action
for one tick. With no eligible action it returns zero, which leaves the
player out of the gather mask.

Repeated gathers reuse that tick’s decision. A later successful build
resumes normal input.

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

**Phantom ping.** The host gathers its phantom seats' controller reports
on the simulation tick that requests them. Perception and motor delays
come from the brain (§3.4). Real network clients receive the host's
normal distributed player updates.

A sim can set `phantom_ping_ms` to model a remote controller. With a
positive round-trip time, the host samples a one-way delay of half the ping
plus uniform jitter bounded by `phantom_jitter_ms` (default 4 ms), rounded
up to simulation ticks. A zero or omitted ping delivers the current report
immediately.

In delayed mode the latest arrival supplies the stick position. The host
delivers each trigger's pressure changes in send order, at most one per tick
after arrival.

A new unit or a break in the stream clears buffered reports. The host always
builds and stamps the retail action at gather with the current weapon and
facing.

**Midship replay.** In game
`531b5da3ceab388417c044afcf18e01a`, the 2,672 recorded frames attribute
2,231 shots to the four phantoms. Fourteen bullet hits and nine melee hits
from them dealt positive damage to opposing players. The three local bots
recorded 1,372 shots and 236 opposing bullet hits.

A team with no kills has no assisted kills either. The damage reports
establish that phantom fire reached the host.

The old phantom adapter treated a half-stick on either axis as about 54°/s.
The local Midship table gives about 9°/s yaw or 5°/s pitch for that input
before aim assist. One phantom's pitch swept from +34° at tick 1284 to −26°
at 1300 while the human's bearing stayed within 6° of level.

Using the local response removes this excess gain from the delayed control
loop. Post-change hit rates require another hosted match.

**Training coverage.** A sim match hosts a private System Link game.
Each phantom's brain, uplink, remote gather and retail action builder
run as they do in a hosted custom game. Training waits for every
configured seat to bind to a player and unit before counting combat.
[Headless simulation](../runtime/sim.md#the-playerless-system-link-host)
