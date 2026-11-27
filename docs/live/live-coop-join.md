# Live co-op: more players and join in progress

Join in progress.

## 13. More than two players

Co-op over Live (sections 10 and 11) takes two consoles. This section is
what holds a campaign at two, what is lifted, and what is left. Built on
tests only; **no game was built or run for it**.

Evidence as at the top:
**[local]** read in `h2 disasm`, counted on the disc, or in this repo;
**[guess]** inferred.

### 13.1 Where the game assumes two

The session and lobby:

| what | where | bound | evidence |
|---|---|---|---|
| a new session's limits | `0x59d0d`, `0x59d13` (and `0x6102e`): peers and players 16 | 16 | [local] |
| join check | `0x5af70` (called at `0x6304a`): peers + 1 within `S+0x4990`, players + n within `S+0x4994` | the limits | [local] |
| Live presence "joinable" | `0x63e00`: players under `S+0x4994` (`0x63e4f`), never for a campaign (`0x63e7a`) | campaign: none | [local] |
| max-players setter | `0x5c490`: writes only in states 5 to 8, so a lobby keeps 16 | | [local] |
| the host's game-start limits | `0x6e720`: a campaign gets player limit 2 (`0x6e8aa`) and peer limit 1 (`0x6e8eb`) | 2 | [local] |
| session start | `0x6ec80`: player count read once at `0x6ee69`; more than two refuses (`0x6ee9b`), more than one peer refuses (`0x6eea5`); the options' co-op byte is `count > 1` (`0x6ef61`); 16 session players copied into 16 options players (`0x6ee12`, `0x6efc8`) | 2 | [local] |
| lobby start check | `0x19a2ce`: the co-op check `0x19a3ed`..`0x19a41c` loops 16 slots, present (`0x19a951`) and local (`0x19ab77`), and wants exactly 2, both local | 2 on one console | [local] |
| lobby text | `coop_takes_two` (`0x0e0001ec`) for start state 7 in co-op mode (`0x2500ab`) | | [local] |
| lobby player table | 16 cells (11.1) | 16 | [local] |

The game itself:

| what | where | bound | evidence |
|---|---|---|---|
| players table | `PLAYERS` (`0x4e8c24`): 16 datums of `0x21c`; `players_globals` (`0x4e8c20`) holds 4 *local* player handles at `+0xc` (loops `0x12b8c0`, `0x138b70`) | 16 players, 4 local | [local] |
| per-tick player update | `0x14d3a0` (from `game_tick`, `0x13801f`): every player; a first spawn pending (flags `+2` bit 3) goes to the spawn each tick until it succeeds | 16 | [local] |
| spawn | `0x14f2e0`: unit type, location, unit; clears bit 3 (`0x14f6c1`); AL 0 when no location was free | | [local] |
| location pick | `0x14ef00`: best of `scnr+0x100` (`0x34` each) that passes the collision test `0x23bb70`, so a spot a biped stands on is skipped; `0xffff` when all are; no wrap | | [local] |
| starting locations | 00a: 1; every other campaign level: 2 | **2** | [local], counted on the disc |
| a third player at the start | waits, retried each tick, until P1 or P2 steps off a spot; nothing crashes | | [local] behaviour, [guess] timing |
| character | `0x14bc80`, `0x14f040`: every campaign player gets the representation of the first typed starting location (`matg+0x138`), so players 3+ are the same Chief or Arbiter as P1 | none | [local] |
| starting profile | `0x1529a0`: the 16 options players | 16 | [local] |
| co-op respawn | gates `0x138820` (co-op byte) and `0x1388a0` (not Iron, not Legendary); `0x14cdd0` walks every player: blocked while a delay (3), a game flag (2), the lost timer (8), `0xcc170` (4) or `0x1ca670` (5) holds; otherwise the anchor is the last player in table order whose unit is not busy, and every dead player is spawned then placed next to it with `0x14e970` (`0x14cfeb`, `0x14cffd`) | 16 | [local] |
| respawn "reason 6" | written in the anchor loop for a busy player (`0x14cf9a`), but the loop goes on and the respawn runs if *any* player can anchor; it only says why when none can. One busy player does not block the others | 16 | [local] |
| dead flags and game over | `0x152340`: `players_globals+4` all dead, `+5` some dead, over all 16; `0x138e40`: co-op loses when all are dead, solo, Iron and Legendary when any is | 16 | [local] |
| checkpoint safe | `game_safe_to_save` (`0x2a9bb0`) is `0x226190`: a run of checks, among them `0xcc170` (combat, as respawn reason 4), `0x14df40` and `0x14deb0` (each walks every player; `0x14deb0` refuses when any unit has object flag `+0x10a` bit 3), `players_globals+5` (some player dead) and the lost timer | every player | [local] |
| BSP switch volumes | `0x14d8b0` (from `0x138078`): every player's unit is tested against `scnr+0x130`; any player can switch | 16 | [local] |
| hs `players` | `0x2a0d10` calls `0x29f480`: every player *with a unit*, in table order | 16 | [local] |
| `volume_teleport_players_not_inside` | `0x2a0df0` calls `0x29f5b0`: every player outside the volume goes to the flag (`0x29ffb0`) | 16 | [local] |
| `object_teleport` | `0x2a24e0` calls `0x29ffb0` for its one object | 1 | [local] |
| the scripts' players | `player0` and `player1` in all 15 levels, `player2`/`player3` in none; `players` in 15, `player_count` in 13, `game_is_cooperative` in 11. Cutscene teleports (`object_teleport (player0) ...`, flags like `bridge_player0/1`, `boss_player0/1` in 04b, 05a, 05b, 08b) move two players | **2** | [local] tokens; [guess] that `playerN` is `(list_get (players) N)`, so with player 1 dead the script's `player1` is player 2 |
| `cinematic_stash_players` | 01b only | | [local] tokens |
| synchronous update | actions: 16 entries of `0x5c` (`0x138db0`; `0x14d6c3`); per-player entries 16 of `0x7c` at `+0x610` (1.3) | 16 | [local] |
| bandwidth | 16 × `0x5c` × 30 ticks ≈ 44 KB/s from the host to *each* client, ~0.66 MB/s up for 15 clients, if the encoder `0xb2710` sends entries raw; ~5.5 KB/s at 2 | | [guess] |
| difficulty, HUD by player count | none seen; only the co-op byte and difficulty are read on these paths | | [local], absence only |
| AI targeting | unread | | |

So the engine runs 16 players already. What holds at two is the data
(two starting locations) and the scripts (`player0`, `player1`).

### 13.2 What the code does for N players

**Stage 1a** (`coop.max_players`, `H2_COOP_MAX_PLAYERS`, 2 to 16, default
2, the host's value counts; `coop_probe`, `coop_live`):

- the session start's window shows one peer and, for more than two
  players, two players until `OPTIONS_NEW`, then puts both back (the
  co-op byte `count > 1` comes out the same);
- after `0x6e720`, a campaign's peer and player limits go to N;
- the joinable answer, the host setup and the lobby's co-op check take 2
  to N consoles with one player each; the blocker and the lobby line say
  "up to N";
- a guest takes the limit as 16: the host's lobby already held the start.

**Stage 1b** (feature `coop-players`, `crates/extensions/game/src/coop_players.rs`,
on the co-op arm, so off outside a co-op session; `H2_FEATURES=-coop-players`):

- *Spawn.* A first spawn of a player of index 2 or more runs the spawn,
  and when its location pick found every spot taken, the pick answers
  spot `index % count`; the spawn is then followed by `0x14e970` next to
  player 0's unit (player 1's when player 0 is in a vehicle), all inside
  the one spawn call, as the co-op respawn does after its own spawn.
- *Stragglers.* A teleport (`0x29ffb0`) of the scripts' `player0` or
  `player1` (the first two players with a unit), a BSP switch or a
  cinematic's end marks the squad; at the next player update, outside a
  cinematic, every player of index 2 or more on foot and more than 8
  world units from the anchor is placed next to it with `0x14e970`.
- The hooks never read a per-console setting, only the game: they act on
  players of index 2 or more, who exist only when the host let a third
  console in. Every console runs them alike, which lockstep needs.

New points: `PLAYER_UPDATE` `0x14d3a0`, `PLAYER_SPAWN` `0x14f2e0`,
`PLAYER_SPAWN_LOCATION` `0x14ef00`, `PLAYER_PLACE_NEAR` `0x14e970`,
`SCRIPT_TELEPORT` `0x29ffb0`; global `SCENARIO` `0x4e0350`.

Tests (`cargo test -p boot --lib coop`): the fallback spot for players
3+ only; the tables read (players, objects, campaign, start count); the
anchor (player 0 on foot, else player 1); who straggles (index 2+, on
foot, past the leash; not player 1, not a driver, not the dead); the
scripts' `player0`/`player1` as the first two with units; the spawn
entry (original with the fallback armed, then the placement at player
0's position, nothing when the spawn failed); the gathering entry (the
stragglers placed, then the update); nothing moves with two players; the
feature on the plan with the co-op hooks.

### 13.3 What needs a game run

Three consoles (or instances), `H2_COOP_MAX_PLAYERS=3` on the host,
section 10.4's setup with a third account. Expect, besides 10.4's lines:

- host: `coop-live: co-op takes up to 3 players (coop.max_players)`;
  `coop-live: lobby start state 7 (co-op wants one console) answered 9 for 3 consoles`;
- every console: `coop-probe: session start: campaign with 3 peers: showing the start 1 peer until OPTIONS_NEW`,
  `coop-probe: session start: 3 players: showing the start 2 until OPTIONS_NEW`,
  `... peer count back to 3 before OPTIONS_NEW`, `... player count back to 3 before OPTIONS_NEW`,
  `session start returned 1 (the game starts)`;
- host: `coop-probe: game limits: campaign peer limit 1 -> 3`, `... campaign player limit 2 -> 3`;
- at the start of the level, every console:
  `coop-players: player 2: every starting location is taken; spawning on 0 and moving it`,
  `coop-players: player 2 spawned; placing it next to the anchor`,
  `coop-players: player 2 placed next to the anchor` (`REFUSED` means
  `0x14e970` found no room: it stays on the spot);
- after a cutscene or a script teleport, when the third player was far:
  `coop-players: 1 player(s) of index 2 or more left behind; bringing them over`.
- with the third player dead, or far away, at a checkpoint:
  `coop-players: checkpoint checks leave out player(s) 2 (dead, or past 8 units from the anchor)`,
  and the checkpoint is taken (`coop-probe: checkpoint save at tick T`
  on every console, at the same tick).

Then: does the third player appear next to player 0, does a dead third
player respawn (retail's co-op respawn, no line of ours), do all three
stay in sync (`OUT OF SYNC`, `h2 coop-probe diff` on three hash files,
pairwise), and do 03a's and 05a's opening cutscenes leave anyone behind.
Players 3+ are the same character as player 1.

### 13.5 Risks and open items

- **Checkpoints with more players (built, option (a)).** `0x226190`
  (`game_safe_to_save`, point `SAVE_SAFE`) refuses a save while any
  player is dead (`players_globals+5`) or any unit is unsafe (`0x14df40`,
  `0x14deb0`, which walk the players' in-use bits). `coop-players` now
  counts players 0 and 1 always, and a player of index 2 or more only
  while alive and within the leash (8 units) of the anchor. For the
  length of the check, the players that do not count are taken out of
  the in-use bits (the datum iterator `0x16bc00` reads them too) and the
  dead flag is recomputed over the rest (only ever lowered); both go
  back when it returns. An unsafe player 3 near the anchor still blocks;
  a player 3+ in a vehicle, or with no anchor on foot, counts as retail.
  With two players nothing is left out: retail. Stderr, once per change
  of who is left out:
  `coop-players: checkpoint checks leave out player(s) 2 (dead, or past 8 units from the anchor)`.
  Tests: `coop_players::tests` (two players unchanged; a dead player 3 no
  longer blocks; a dead player 1 still does; a far player 3 does not
  block, a near or driving one counts; the bits and flag are put back).
- **The scripts know two players.** The leash covers being left behind;
  it does not put players 3+ into cutscene positions, and a script that
  checks `(volume_test_objects ... (players))` counts every player, so a
  straggler inside a volume can trigger it (as retail with two).
- **Two start spots.** Players 3+ start inside player 0's spot for no
  tick at all (placed within the spawn call), but if `0x14e970` refuses
  they stay overlapped until they move.
- **Respawn anchor** is the last player in table order who is not busy,
  so a respawn can land next to player 15 rather than player 0. Retail
  behaviour, now with more choices.
- **Bandwidth at 16** (13.1) and the lockstep pace.
- **Unread:** AI targeting with many players, the engine's handling of
  stragglers outside a new BSP, cinematic player handling beyond the
  stash, the update encoder.

## 14. Join in progress

The owner's pick (2026-11-17): a friend can join a running co-op
campaign **without anyone reverting**. The host, at a tick boundary,
holds, snapshots the game state and sends it (compressed) to the
newcomer, who has loaded the same level, restores it, and picks up the
synchronous stream on the next tick. A hold of a second or two on the
host is acceptable.

Behind `coop.join_in_progress` (default on,
`H2_COOP_JOIN_IN_PROGRESS`, docs/game/settings-menu.md).

What is here: a static trace (14.1), the design (14.2 to 14.6), what is
built on tests (14.7), what needs a game run (14.8), and the second
pass (14.9): the retail path traced end to end and wired by lifting one
check, with a `coop-jip:` line per stage. Our own protocol (14.7) stays
the fallback.

### 14.1 What the retail engine already has

The surprise: **retail has a synchronous join that sends the game state.**
It never shipped in a mode anyone used, like the rest of section 1.3.

- [local] The server's per-client send loop `0x000865d0` drains a ring of
  typed records for one client (ring at view `+0x9C`: size `+0`, head
  `+8`, count `+0xC`). A record of type 0 goes out as synchronous-join
  (message `0x29`, 4 bytes), type 1 as synchronous-gamestate (`0x2a`,
  the piece plus an 8 byte header), anything else is built by `0x00068670`
  into a `0x4048` byte update and goes out as synchronous-update (`0x27`).
  All three go through `0x00075e80` with channel `+0x20` of the view, in
  queue order. So join, state and updates reach a client **in one ordered
  stream**: exactly the "pick up from the next tick" this design needs.
- [local] `0x000862e0` (eax: the client's view) starts a join: it bumps an
  attempt count at view `+0x90`, allocates a `0x80000` byte and a
  `0x40000` byte buffer (`0x0012d2f0`, retrying up to 30 times), then
  `0x00086800` passes the game-state block (`*0x004e6080`) and the
  `0x40000` buffer to `0x001995a0` (which allocates a `0x4B000` work
  area; [guess] a compressor, level 9) and queues the result in pieces of
  at most `0x400` bytes as type 1 records. It then calls the game-state
  table entry at `0x0046e330` with 0, the same table the update's
  `+0x403C` flag calls (1.3).
- [local] Its one caller (`0x0006a141`, in a routine that also drives
  `0x00085a70`) runs it for a view whose state (`+0x2C`) is 3 and whose
  mode word (`+2`) is 2: it counts ready peers (the `0x00069470` walk,
  `+0x94`) against `*0x004ced48`, checks a timer (`0x00075890` against
  `*0x004ced54`), calls `0x000862e0`, and once the attempt count reaches
  `*0x004ced50` gives up.
  [guess] This is the "blocking" and "catchup-fail" of the view reason
  table (1.3): the game blocks while a joiner receives the state, then
  the joiner catches up through `0x000698e0`.
- [local] `0x0014c630` (called from the update apply `0x000683a0` when
  the record's `+0xDD0` flag is set, with `+0xDD4` a player mask (bits 0
  to 15)
  and `+0xDD8` sixteen 6 byte player identifiers) is the **player set
  change carried in the update**: it matches the new identifiers to the
  old ones in `players_globals+0x30` (by 6 byte compare), stores the new
  mask at `players_globals+0x2C` and the identifiers at `+0x30`, walks
  the players table (`0x004e8c24`, in-use bits) and calls `0x00152df0`
  and `0x0014c880` for players that changed. [guess] Adding a player
  mid-level is a field in the next update, applied on the same tick on
  every machine. That is where the newcomer's player enters.
- [guess] 4 MiB of state into a 256 KiB buffer: retail expected the block
  to compress about 16:1 (most of it is free pool space). Our own
  numbers come from the first run (14.8).

The rest (the client side, who fills `+0xDD0`, the callback tables, the
gate) is traced in 14.9.

### 14.2 Two ways to build it

1. **The retail join.** Lift the session's "not joinable while in game"
   (section 11 lifted it for the lobby only), let the newcomer join the
   network session, and let the server's view machine send the state by
   itself. Cheapest if it works, like the probe of section 9, and it
   keeps the transfer inside the game's own ordered stream. Unknowns: the
   client receive side, the timers at `0x004ced48`..`54`, and whether a
   campaign session ever reaches view state 3.
2. **Our own join** (built, 14.7): the host holds its tick, snapshots
   with the checkpoint machinery, and sends the state over the co-op
   transport (crates/live/coop, the same direct/relay link as phase 0);
   then the newcomer is admitted through the update's player-set field.
   Independent of the unknowns above; costs a hold of the host's tick.

Decided (14.9): 1 is wired, since its client side turned out complete;
2 is the fallback if a run shows the retail join failing. Either way the newcomer's
player enters through the update's player-set change, so every console
adds it on the same tick.

### 14.3 What the snapshot covers, and what the newcomer rebuilds

| state | in the `0x3fe000` block? | how the newcomer gets it |
|---|---|---|
| objects, units, players table, AI, physics, effects, scripts' globals and threads | [local] yes, the checkpoint block (docs/runtime/checkpoints.md) | restore |
| the update number, tick, random seed | [guess] yes (game time, `*0x004e7408` live in it); the update's `+0x4040`/`+0x4044` check catches a mismatch (1.3) | restore; first update checks |
| the level, its loaded BSP and zone set | no: map data | loads the host's level and BSP **before** asking (the accept repeats both); a BSP switch while joining is refused by the hold |
| sound | no | starts silent; looping sounds the state names start on the next update ([guess]) |
| HLE state (Live session, XNet, invites) | no | its own: it accepted the invite and is in the session |
| network/session tables (peers, session players, simulation globals `0x4CF77C`) | no | the newcomer joins the network session as a sync-client (world mode 3) at update T+1; the session player array (`S+0x4F90`) comes from the session join, as for a lobby guest |
| local player mapping (which slot is "me") | no, per machine | set from the accept's slot before the first update |
| our features' host-side state (coop-players leash, skulls) | no | skulls are sim settings carried in the session (Sim scope); the leash is recomputed each tick |

[guess] The hard part is that the network session and the simulation
globals are outside the block: restoring a checkpoint is designed for a
machine that is already in the game. The newcomer must be a sync-client
at update T with an empty queue before the restore, so the restore does
not undo its session.

### 14.4 The flow

```
newcomer                        host                          other guests
accept invite (host in level)
load level + BSP from advert
Request ----------------------> at next tick boundary:
                                 Hold (no update T+1 yet)      wait for T+1
                                 snapshot end of T (before
                                   the player exists)
<------------------------------ Accept {T, slot, level, bsp, len, chunks, sum}
<------------------------------ Chunk 0..63 (window)
Ack next ---------------------> more chunks ...
all chunks, checksum ok:
restore, local player = slot
Ready {T} --------------------> Admit: update T+1 carries the
                                 player-set change (+0xDD0)
                                Resume
sync-client from T+1  <-------- update T+1 -----------------> everyone adds the
                                                               player on T+1
```

The snapshot is taken **before** the player is added, so it is the
state every console already has, and a failed join has nothing to undo.

### 14.5 Transfer format and size

- [local] Our datagram (`coop::join`): magic `h2cj`, version, session,
  kind, nonce; chunks of 1024 state bytes (1044 bytes on the wire, under
  the relay's 1200 byte cap and the MTU); a window of 64 chunks (under
  the relay's 256 message queue); cumulative acks; FNV-1a 64 checksum of
  the whole state.
- [local] Retail's own pieces are `0x400` bytes too (14.1).
- Size: the block is `0x3fe000` bytes (4088 chunks raw). [guess] It
  compresses to 256 KiB or less (retail's buffer), which is 256 chunks:
  four round trips of the window. At a 50 ms relay round trip that is
  about 0.2 s plus the snapshot, compression and restore; on a LAN, well
  under a second. The newcomer takes up to twice the raw block
  (`MAX_STATE`) in case compression gains nothing.
- Compression is the caller's (the `snapshot` crate's codec); the
  protocol carries bytes.

### 14.6 Failure paths

| what | host | newcomer |
|---|---|---|
| `coop.join_in_progress` off, host not in a level, all slots taken, another join under way | refuses; never holds | back to the lobby, says why |
| host never answers (5 s, request resent every 250 ms) | nothing | back to the lobby |
| no progress for 3 s (chunks or acks lost for good) | abort, resume without the player | back to the lobby |
| the whole hold passes 15 s | abort, resume | back to the lobby |
| checksum mismatch | abort, resume | back to the lobby |
| restore fails | gets the abort, resumes | back to the lobby |
| the ready is lost | repeats the last chunk; the newcomer answers; the ready is repeated | repeats the ready until the first update; admitted once |

Log lines: newcomer `coop-jip: joined in progress at tick T as player N`
or `coop-jip: back to the lobby: <why>`; host
`coop-jip: admitted player N at tick T (K KiB sent, held M ms)`.

### 14.7 The join code and its tests

- `crates/live/coop/src/join.rs`: the messages (encode, decode, refusal of
  short, long, unknown and oversized input), the host state machine
  (`Host`: request, hold, snapshot, windowed resend, ready, admit,
  timeouts, cancel) and the newcomer's (`Joiner`: ask, receive in any
  order, checksum, restore, ready until streaming, timeouts). Pure: time
  and the state bytes are passed in. Tests (`coop::join::tests`): a whole
  join over a perfect link (one pass, admit on T+1), over a link losing
  one message in five, a lost ready, every refusal, a silent host, a
  silent newcomer, the hold ceiling, a host that cannot snapshot, a
  failed restore, damaged state, out-of-order and short chunks, an
  insane plan, the log lines.
- `coop.join_in_progress` (`boot::setting_defs`), default on, live.
- `h2 coop test --players 3 --auto --late`: the last guest's accept waits
  `H2_COOP_AUTO_ACCEPT_DELAY` (120 s) after its invitation
  (`live_hle::auto`), and the merged log names it `# late`. The
  checklist leaves it out of the lobby steps and adds a "join in
  progress" part: `Charlie joined in progress at tick T (player 3)`, or
  why not, and the host's admissions.

Superseded by 14.9 (the retail path is wired); for the fallback, in order: (1) on the host, a feature
that holds the tick (stop sending updates, as a sync-server whose
client queue is full) and calls the checkpoint writer `0x123e20` into a
buffer; (2) on the newcomer, load the level from the advertisement,
restore through the full-restore path `0x124380` and set its local
player; (3) the player-set change in update T+1 (14.1); (4) carry the
join datagrams on the co-op link beside the pad datagrams
(`Datagram::is_join` tells them apart).

### 14.8 What needs a game run

See 14.9.4: the run and the lines to expect, stage by stage.

### 14.9 The retail join, traced and wired (second pass)

Static reading with `h2 disasm`, no run. Every address below is
[local] unless marked.

**14.9.1 The client side is there.** The session's message dispatch
`0x00093aa0` (a jump table at `0x00093eb8`, message id minus 6) sends
the four synchronous messages, each only while the session's channel
state (`+0x54`) is 5, to small handlers that find the simulation view
(`0x0006adc0`):

| id | handler | what it does |
|---|---|---|
| `0x27` update | `0x00094990` | sync-client (view mode word 1): `0x000698e0`, the catch-up |
| `0x28` actions | `0x000949d0` | sync-server (mode word 2): `0x00085f50` |
| `0x29` join | `0x00094a10` | client: `0x00085dc0`, which (world mode 3 or 5 excluded) calls `0x0006a8a0` (a `0x40000` byte receive buffer at world `+0x1204`, count `+0x1200`), stores the server's update number at `+0x28` and `+0x120c`, and in mode 3 resets through `0x0006ab10` |
| `0x2a` state | `0x00094a50` | client: `0x00085ed0`; a piece with bytes goes to `0x0006a990` (appended, at most `0x40000`), the empty last piece to `0x0006aa20` |

`0x0006aa20` decompresses what arrived (`0x00199740`) **straight into
the game-state block** (`*0x004e6080`), between `0x00199520` and
`0x00199540`, which call the callback tables at `0x0046e334` (five
entries) and `0x0046e348`; it succeeds only for exactly `0x3fe000`
bytes. A failure in either client handler calls `0x00085a70(0, -1)` and
sets the view's state `+0x28` to 5 ([guess] leave the game). So retail's
client restores the host's state in place and then runs from the queued
updates.

This replaces our own snapshot and restore for the retail path.

**14.9.2 The server side.** `0x0006a860` runs the four callbacks at
`0x0046e320` (pointers to `0x001c3540`, `0x00068090`, `0x0016eff0`,
`0x001e6af0`), marks world `+0x11fc` and answers the size `0x3fe000`;
`0x000862e0` then compresses and queues (14.1) and calls the single
callback at `0x0046e330` (`0x001c3590`). The update's `+0x403C` flag
runs the same four and the same one on a server
(`0x00068491`..`0x000684b6`). [guess] These are the "before and after
the game state is read" hooks (the checkpoint writer `0x123e20` runs
them too): the flag makes every console run them on the tick the state
is taken, so the clients stay in step with the snapshot.

**14.9.3 Who adds the player.** `0x00068350` (from the record build
`0x00069260`, outside sync-server mode) copies a pending player set
from the simulation world (`*0x004cf780`: mask `+0x1c`, sixteen 6 byte
identifiers `+0x24`, pending byte `+0x84`) into the record's `+0xDD0`,
`+0xDD4`, `+0xDD8` and clears the pending byte. The pending byte is set
at `0x00083160`, `0x0008340f`, `0x00083434` and `0x0008424e`, from the
session's player array (stride `0x10c`, identifier at `+0x16`). So a
player the session admits reaches the update stream by itself, and
`0x0014c630` adds it on every console on the same update (14.1).
[guess] On the sync-server the record comes from its queue
(`0x0006ac30`), built by the same path.

**What blocks it today.** The host's join check `0x00062fe0` (session
in EAX, request in EDI) answers 3 unless the session state `S+0x741c`
is 5, the lobby: states 6, 7 and 8 refuse. Its other answers (2 invite
only, 4 no room, through `0x0005af70` and `0x0005afc0`) do not depend
on the state. The view machine's gates at `0x0006a0b7`
(`*0x004ced48` ready peers, `*0x004ced54` a timer, `*0x004ced50`
attempts) are in BSS but not zero: the network setup at
`0x000679ea..0x00067a08` sets them to 8, 3 and 2000 (and `0x004ced40`
to 10000).

Found while tracing phantom peers (docs/game/bots.md §5.1);
an earlier note here guessed they stayed zero.

**14.9.4 What is wired** (`crates/extensions/game/src/coop_jip.rs`, feature
`coop-jip`, on the co-op over Live arm):

- **The lift.** On the host of a co-op campaign with
  `coop.join_in_progress` on and a free slot (`coop.max_players`), a
  join check made while the game runs (state 6 to 8) sees state 5 for
  the length of retail's call, and the real state goes back after it,
  whatever retail answered. Off: `H2_COOP_JOIN_IN_PROGRESS=0`, or
  `H2_COOP_JIP_LIFT=0` for this part alone.
- **A line per stage** (off with `H2_COOP_JIP_LOG=0`), on the points
  `SESSION_JOIN_CHECK`, `SIM_JOIN_SEND_STATE`, `SIM_JOIN_RECEIVE`,
  `SIM_STATE_RECEIVED` and `SIM_UPDATE_APPLY` (all optional: a missing
  one drops its line, not the feature).
- `H2_FEATURES=-coop-jip` drops all of it.
- Tests (`coop_jip::tests`, debug): the stage switches; the lift's
  decision (host, campaign, running, not full, setting and switch);
  the lifted check sees state 5 and puts 7 or 8 back, and retail's
  other refusals stand; the host's lines (admitted once, only after a
  send, not after a failed queue); the newcomer's (joined on its first
  update after a restore, once; back to the lobby when the restore
  fails); the feature on the plan and off by `H2_FEATURES`.

**The user's run:** `h2 coop test --players 3 --auto --late`. Alpha
(host) and Bravo start a co-op level; Charlie accepts 120 s after its
invitation, so have the level running by then. The lines, in order:

1. Alpha: `coop-jip: host: a console joins the running game (session state N read as the lobby's for the join check)`.
   Instead `... still refused (C: why)`: retail's other checks
   (2 invite only, 4 no room: check `coop.max_players` is 3). No line at
   all: the request never reached the check ([guess] Charlie's own
   session refused to join a running game first; look at Charlie's
   `session` and `coop-live` lines).
2. Alpha: `coop-jip: host: sending the game state to a joining console (attempt 1)`
   then `... game state queued ahead of the joiner's updates`.
   Missing: the view machine never ran `0x862e0` (its gates, 14.9.3).
3. Charlie: `coop-jip: guest: join message; the host's game is at update U`.
4. Charlie: `coop-jip: guest: game state restored (K KiB compressed)`
   (K is the number 14.5 guesses at), or
   `coop-jip: back to the lobby: the game state did not restore`.
   Charlie needs the level loaded for the restore to mean anything:
   [guess] retail's session start loads it when the join is accepted.
5. Charlie: `coop-jip: joined in progress at tick T as player 3`, and
   Alpha: `coop-jip: admitted player 3 at tick T (M ms after the state went out)`.
6. The checklist ends with `[x] Charlie joined in progress at tick T (player 3)`.

Then play a minute: a desync shows as the out-of-sync latch (1.3) and
the existing `coop-live`/`coop-probe` lines. If the retail path stops at
2 to 4, the fallback is 14.7's protocol.
