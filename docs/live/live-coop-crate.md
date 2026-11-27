# Live co-op: the coop crate, probe and lobby

Probe (phase 2); 11. The retail lobby (co-op over Live, as the game's own co-op).

## 8. The coop crate

All on tests; nothing here was run in the game. It is the part of phase 0
that does not need the settings layer.

**`crates/live/coop`** (no guest, no game data):

- `wire`: the one datagram. It carries the sender's acknowledgements and
  repeats every pad frame and tick hash the other side has not
  acknowledged (up to 32 of each), so delivery is in
  order whatever the link drops, and nothing is retransmitted on a timer.
  The datagram (version 3, 1159 bytes at most, under the relay's cap) also
  carries the Forge lane: a run of an ordered, acknowledged byte
  stream that a shared Forge session sends its messages on - as many
  bytes as fit beside the datagram's pads and hashes (`forge_room`, at
  most `MAX_FORGE`, never past the wire cap), with `FORGE_WINDOW` runs a
  flush so a welcome crosses in a few round trips - and the beat,
  the 48 bytes of log head and stream position a machine reports in every
  datagram as the session's keep-alive
  ([forge-design.md](../game/forge-design.md), "F5 in detail").
  A datagram of another session, another version or another shape is
  counted and dropped.
- `link`: `Link` is an unreliable datagram path. `UdpLink` is the direct
  one, on the XNet base port + 4 (`live_hle::NetworkConfig::coop_endpoint`;
  base+0, +1, +5, +6 are the title's channels and +2 is QoS, so a host
  that opens base..base+6 for System Link already opens it). `UdpPort` is
  the many-peer form of it: one socket bound at the machine's own channel,
  a `Link` to each peer, datagrams filed by their source address - the
  form the game's Forge link serves a host's guests and a guest's host
  through (`Hle::coop_endpoints` names them from XNet). `Fallback`
  sends on the direct path always and on the relay too while nothing has
  been heard on direct for a timeout. `sim_pair` is a simulated link on a
  fake clock: latency, jitter, loss, reordering, a switch to take it down,
  repeatable by seed.
- `relay`: `RelayLink`, the relay path as a `Link`, on a thread of its
  own. The game's Forge co-op link serves one lane through it beside the
  direct channel (`coop::link::Fallback`), for two machines that cannot
  reach each other directly.
- `schedule`: the two-port pad schedule of 2.2. A machine's sample taken
  when it starts frame *f* is frame *f + delay* of its port; the first
  `delay` frames are neutral; the delay is per machine and can change
  during a game without notice to the other side. Packet numbers are
  derived from the agreed stream.
- `peer`: one machine's end. `begin_frame` starts a frame only when both
  ports' pads are in hand (`Hold::Waiting` otherwise); `report_hash` takes
  a tick's hash, and the first tick whose hashes differ latches
  `Hold::Desync` on both machines. `forge_send`, `forge_recv` and
  `forge_set_beat` are the Forge lane's ends; a queue of 24 MiB
  unacknowledged refuses more bytes.
- `tap`: `ScheduleTap`, an `xapi::tap::InputTap`. It overwrites the live
  packet for ports 0 and 1, answers "not connected" for anything else,
  and counts a read that came with no frame started.

**`crates/extensions/lockstep`**: `SimRegion` and `hash_sim`, the simulation hash:
one word over the game-state block (`SimRegion::HALO2`: `0x3fe000` bytes
behind the base word at `0x004e6080`, from the static notes, unchecked in
a running game) instead of all of RAM.

**Live server**: `GET /v1/sessions/{id}/relay` ([live-api.md](live-api.md),
"Co-op relay"): a WebSocket for the two members of an open session
advertised with `flags: "coop"` or `flags: "forge"` (a shared Forge
session's lobby). The upgrade uses a `sessions.relay`
bearer token. The relay caps messages at 1200 bytes and rate-limits each socket
(`Policy::relay_rate`).

It forwards bytes and never reads or stores them.

**What the tests prove** (`cargo test -p coop -p lockstep -p live-server`):

| claim | test |
|---|---|
| both machines' guests read the identical per-frame pads under latency, jitter, loss and reordering, equal to a zero-latency run | `coop/tests/peers.rs`: `loss_and_reordering_change_nothing_the_guest_reads` (8 seeds), `over_the_budget_frames_wait_and_the_stream_does_not_change` |
| inside the delay budget no frame waits | `inside_the_delay_budget_no_frame_waits` |
| a dead link stalls both machines within their delay's worth of frames, and they resume with the same stream | `a_dead_link_stalls_both_and_they_resume` |
| the relay carries the game while direct is dead, direct takes over when it works, and the relay then goes quiet | `the_relay_carries_the_game_while_direct_is_dead`; with the real server and real sockets, `live-server/tests/relay.rs`: `two_coop_peers_play_through_the_relay_when_direct_is_dead` |
| a differing tick hash stops both machines at that tick | `a_differing_hash_stops_both_machines_at_that_tick` |
| per-machine delays, and a change of delay during a game | `each_machine_has_its_own_delay_and_may_change_it` |
| real UDP on loopback | `two_peers_over_real_udp_on_loopback` |
| two toy guests (`lockstep::synthetic`) reading through the tap over a bad link give identical whole-RAM traces, and a word changed on one machine stops both at that tick | `coop/tests/toy.rs` |
| a read never falls back to the local device | `coop::tap` unit test |
| only the session's two members get into the relay; size and rate caps close the socket | `live-server/tests/relay.rs` |

**Left for the rest of phase 0 and after**

- Wiring into `boot` and `app`: the game's Forge co-op link runs a `Peer`
  per other machine of a synchronous network game
  (`game/src/forge_glue/link.rs`, pumping it each tick and offering its
  Forge lane to a shared Forge session,
  [forge-design.md](../game/forge-design.md) "F5 in detail"), over the
  direct channel or the relay (`Fallback`), but no
  runner holds a game's frames on a `Peer` yet: the hold at the scheduler
  point after a present, the tap's handle-to-port map from `xapi` (the
  tap is given a handle only), and the tick observer feeding
  `report_hash` are all still to come.
- The tap covers `XInputGetState` only. Device arrival and capabilities
  still come from the local backend (2.9).
- The handshake of 2.3 (build, fingerprint, disc, epoch, start-state
  pack) and the session data directory. The datagram's session number is
  the only check today.
- Round-trip measurement and choosing the delay from it; the "waiting
  for <gamertag>" notice; carry-on-alone (2.7).
- The relay client speaks `ws://` and `wss://` through tungstenite's own
  connector. `live-telemetry` builds its TLS connector by hand for the
  ingest socket; the two should share one. Token refresh on a long
  session is on the server side only: the client does not send a fresh
  token yet, so a relay socket lasts one token lifetime.
- UDP hole punching. Direct needs a reachable port, as System Link does
  ([live-matchmaking.md](live-matchmaking.md) 3.6); otherwise the relay.
- The retail path's own packets through the relay (section 1): the relay
  forwards any bytes, but the XNet HLE does not use it.
- A `coop` session shows in `GET /v1/sessions` like any other. The HLE's
  custom-game search and the web session list should leave it out or
  label it.
- The settings-layer gaps of 5.4, when `settings-slice` lands: the co-op
  `Def`s (`coop.input_delay` and the rest), a store fingerprint by scope
  for the handshake, releasing forced values, the scope filter on
  in-guest rows, `Action` rows for the lobby, and the tick observer
  chaining through `Dispatch::override_handler` instead of refusing.

## 9. Probe (phase 2)

Built on tests; **no game was built or run for it**. The probe tries the
retail synchronous path of section 1 with two instances over System Link
and writes enough on each machine to say how far it got and where the two
parted. It is off unless `H2_COOP_PROBE=1`.

Code:
`crates/extensions/game/src/coop_probe.rs`, `crates/extensions/lockstep/src/pools.rs`,
`h2 coop-probe diff` (`crates/apps/cli/src/coop_probe_cmd.rs`).

### 9.1 What it changes

All through the feature plan (feature `coop-probe`, points in
`engine::points`), nothing patched by hand:

| retail block | what the probe does | why this way |
|---|---|---|
| a campaign start with more than one peer is refused (`0x6eea5`) | a `before` on the session start `0x6ec80` writes 1 into the peer count `S+0x54` when the session is campaign, has exactly two peers and at most two players; a `before` on `OPTIONS_NEW` `0x138110` puts the real count back; an `after` on `0x6ec80` puts it back if the start refused for another reason first | `0x6ec80` reads `S+0x54` once (`0x6ee7e`) and makes no call between its entry and `OPTIONS_NEW`, the first call after the checks. Everything after that, the game start `0x18e8b0` and the simulation world it builds, sees the real count (`0x6e798` builds a peer mask from the same field, so it is the peer count). Porting all of `0x6ec80` (a `0x115c`-byte frame, the options block, five callees) would be a copy of retail logic with room for mistakes; the window trick changes one word for the length of the checks |
| more than two players (`0x6ee9b`) | left alone | two machines with one player each pass it; the probe stops at two anyway |
| the host's game-start parameters set a campaign session's peer limit `S+0x4990` to 1 (`0x6e720`, state 5) | an `after` sets it back to 2 | [guess] the guest is already in by then, but the limit checks (`0x5af70`, `0x65ab0`) compare the peer count with it, and two peers over a limit of one may be dropped |
| no lobby for a networked campaign | on the host, once a second machine is in the System Link lobby, a `before` on `game_tick` calls the campaign menu's own setters: `0x19a6f2` (campaign 1, the level's map id in ESI) then `0x19a76d` (difficulty in AX), and re-enters the tick | these are what the campaign menu calls (`0x199af6`, `0x199b00`); `0x19a76d` also clears the variant, which selects the synchronous protocol, and both replicate to the guest as session parameters. Up to three attempts, 60 ticks apart |
| a campaign session is never joinable (`0x63e7a`) | a `before` on `0x63e00` answers as retail would without that clause while fewer than two players are in | the predicate's only caller feeds Live presence (`0x19104a`, bit `0x10`); [guess] System Link joins are gated by the peer and player limits instead. It is cheap, and a Live co-op lobby will need it |

The level comes from `H2_COOP_PROBE_MAP` (a map file stem, default
`01a_tutorial`) and `H2_COOP_PROBE_DIFFICULTY` (`easy`, `normal`,
`heroic`, `legendary`; default `normal`), resolved against the disc at
boot like `--level`. Only the host uses them; setting the same values on
both machines is harmless. If an address the probe needs already has an
override outside the plan, it prints `coop-probe: refused: ...` and
stays off; the game boots as usual.

### 9.2 What it writes

Every line on stderr starts `coop-probe:`.

| line | meaning |
|---|---|
| `on; the host sets 01a_tutorial normal (campaign 1, map N)` | the probe is on and the level resolved. `... will not set the lobby's level` means it did not (a bad name, or not a campaign map); that machine can still join and log |
| `refused: SESSION_GAME_START (0x0006ec80) has ...` | another override holds an address; the probe is off |
| `session 0x...: state S, peers P (max M), players N (max K), host\|guest (...), protocol X (synchronous), difficulty D, campaign C map M, ...` | the squad session, printed whenever any of it changes. A campaign lobby shows `protocol 1 (synchronous)` and a difficulty |
| `host: lobby setup: waiting for a second machine (1 of 2)` | the host waits for the guest to join |
| `host: setting the lobby to ... attempt 1 of 3`, `host: LOBBY_SET_MAP returned 1`, `host: LOBBY_SET_DIFFICULTY returned 1` | the setters ran. `returned 0 (refused)` names the one that refused |
| `host: the session holds ... press Start in the lobby` | the session now reads campaign with the right map and difficulty |
| `joinable: true (retail false; ...)` | the presence predicate's answer changed |
| `session start: session ...`, then `campaign with 2 peers: showing the start 1 peer until OPTIONS_NEW`, `peer count back to 2 before OPTIONS_NEW`, `session start returned 1 (the game starts)` | the session start on that machine, the bypass and its undo. `returned 0 (refused)` means it refused for some other reason (the line before it shows the session) |
| `game limits: campaign peer limit 1 -> 2` | the host's game-start parameters were corrected |
| `game start: options mode 1, simulation mode sync-server (2), scenarios\solo\...` | a map activated: options mode 1 is campaign; the simulation mode is the dword at simulation globals `+8`, the world's numbering (section 1.1). The host should say `sync-server (2)` and the guest `sync-client (3)`. `local (1)` means the start did not take the synchronous path |
| `simulation mode ... at tick T` | the mode changed |
| `hash log <data>/coop-probe/<time>-<pid>-<role>.hashes` | the per-tick hash file for this game; `hashing the game-state block at 0x...: N pools` follows on the first tick |
| `OUT OF SYNC: the retail latch (simulation globals +0x2c) is set at tick T, update U` | **the retail desync latch fired** (update number, tick or random seed mismatch, or an update out of order; section 1.3). The most important line |
| `the simulation's ended latch (0x4cf772) is set at tick T` | the simulation ended itself (on a server, after the latch) |
| `stall: N ms without a tick before tick T (last T', update U)` | more than half a second of wall time between two ticks while a game runs: a peer waiting for updates (or a load) |
| `checkpoint save`, `checkpoint revert`, `bsp switch`, `cinematic start`, `cinematic stop` `at tick T` | those events, also noted in the hash file |
| `game end at tick T`, `wrote N ticks to ...` | the game ended and the hash file is closed |

The hash file starts `# h2 coop-probe hashes v1`, then `#` lines with the
role, modes, scenario and session, then one line per tick and pool:
`<tick> <pool> <16 hex digits>`. The tick is the game's own tick count
(`game_time +8`), so both machines number ticks alike. Pool `*` is the
whole game-state block (the `0x3fe000` bytes behind `0x4e6080`, what
checkpoints save).

The other pools cut the block at every datum array
header (`'d@t@'`), named by the array, with `(head)` before the first; a
name seen twice gets `#2`. Events are `# tick T ...` lines. It is flushed
every 300 ticks and at every event, so a crash loses little.

About 100
bytes per pool per tick: expect a few MB a minute.

`h2 coop-probe diff <host.hashes> <guest.hashes>` prints the ticks both
cover, the first tick where they differ with the pools that differ there
and any event noted at that tick, and how many ticks each pool differs on.
It exits 0 when every common tick agrees.

## 11. The retail lobby (co-op over Live, as the game's own co-op)

Section 10's list screens were ours. The Live menu's Co-op Campaign row now
opens the game's own pregame lobby in co-op mode instead: its own level and
difficulty screens, its player slots and gamertags, its ready state and its
Start Game. Live adds two things only: the second player can be a Live
friend (the lobby's Y opens the friends list and invites), and the guest
lands in the same lobby, showing the host's level and difficulty.

The list
screens stay behind `H2_COOP_LIST_UI=1`.

### 11.1 Static trace

- Live menu row chosen: `0x230f92`. Its Custom Game branch (row 2, at
  `0x231001`) calls `0x199e2e(1)`, then `0x59890(2,0,0)` to create the Live
  squad session, and on success `0x199a03(2)`.
- `0x199a03(mode)` (`LOBBY_MODE_SET`, stdcall, `ret 4`) sets the lobby
  mode. Mode 0 (co-op) calls `0x63fb0`, then `0x199a92`, the campaign setup
  that calls `LOBBY_SET_MAP` and `LOBBY_SET_DIFFICULTY`. Mode 2 (custom
  game) calls `0x19a864`.
- The game's own way into co-op from a lobby is the squad settings dialog
  (row chosen `0x2bb801`, jump table `0x2bb884`). "Switch To: Co-Op Game"
  (`0x2bba01`) is gated on byte `[0x54D5A0]` (error dialog `0x32`). After
  the gate it calls `0x199a57`, `0x199a03(0)`, `0x121100` and `0x148d42`,
  clears `[0x54E7CC]`, calls `0x19a76d` and closes the dialog. Error `0x32`
  is also raised from Live menu code (`0x230888`), so the byte is not a
  plain "networked" flag. Its meaning is not traced; the row does not go
  through it.
- Start state `0x19a2ce` (`LOBBY_START_STATE`, stdcall, `ret 4`): 9 is
  ready, 7 means the co-op/teams check failed. At `0x19a3ed`..`0x19a41c`
  co-op wants exactly two players, both on this console (`0x19ab77`). The
  tail (`0x19a4c5`) gives 10 while the countdown runs (`0x64a60` >= 0,
  `LOBBY_COUNTDOWN`), otherwise 9 when `0x592f0` (`LOBBY_ALL_READY`) is
  true, else 8.
- The status line (`0x24ffc9`) shows `coop_takes_two` (`0x0e0001ec`) for
  state 7 in co-op mode (`0x199994`), at `0x2500ab`.
- `pregame_lobby` (screen 14): pane 0 is multiplayer, pane 1 is co-op
  (`level_format` `0x0c00181a`, `difficulty_format` `0x1100181b`, the level
  description `0x1d00181c`). The player table has 16 cells on
  `player_skin_default`. The help text says to press Y for the friends
  list to invite. The level and difficulty screens are `load_level_lobby`
  and `difficulty_lobby`.

### 11.2 What the hooks do

- `ROW_CHOSEN` (layout glue): the Co-op Campaign row arms co-op as host
  (`Lobby::open_retail`), asks for co-op mode, rewrites the press record's
  row to 2 and runs the original handler. The game then creates its Live
  squad session and opens its lobby as for Custom Game.
- `LOBBY_MODE_SET`, "co-op mode for the co-op row": within 5 s of that
  press, the next mode 2 becomes mode 0, once. Every other row drops the
  request.
- `LOBBY_START_STATE`, "co-op over 2 to N consoles": while co-op is armed,
  a 7 is answered with the tail's own result (10, 9 or 8) when the
  session is a networked campaign of 2 to `coop.max_players` consoles
  with one player each (section 12). The
  session start that follows is the probe's synchronous path (section 9).
- The pinger thread's tick (`Lobby::sync_retail`): the host advertises
  the level and difficulty the lobby holds, so the guest's join reads
  them.

Ping: the lobby has no natural spot for it (the `active_protocol` block,
`0x0f001818`, is the nearest, and it says what the session is, not how
fast). Ping stays on the web.

### 11.3 What needs a game run, and the lines to look for

1. Pick Co-op Campaign on the Live menu. Expect
   `coop-live: Live menu row 3 opens the custom game lobby (row 2) in co-op mode`,
   `coop-live: hosting a co-op campaign in the game's own lobby` and
   `coop-live: the Co-op Campaign row's lobby: mode 2 (custom game) set as 0 (co-op campaign)`.
   The lobby should show pane 1 (level, difficulty, description).
2. Pick a level and difficulty in the game's own screens. Expect
   `coop-live: the lobby holds <level> on <difficulty>; advertised`.
3. Press Y, invite a friend; the friend accepts and lands in the same
   lobby, showing the host's level and difficulty and both gamertags.
4. Start Game. Expect
   `coop-live: lobby start state 7 (co-op wants one console) answered 9 for 2 consoles` (the number of consoles in the lobby),
   then the probe's session start lines (section 9.2).

Also to check: whether the Live menu's cursor comes back on Custom Game
(the press record was rewritten), and whether `[0x54D5A0]` matters on
this path.

### 11.4 Previews

`the_retail_co_op_lobby_on_live_drawn_from_the_disc` (in
`crates/extensions/game/src/coop_live/tests.rs`) draws the lobby as host and as
guest, plus the level and difficulty screens, with
`ui_kit::preview::Disc::render_retail`. It needs `H2_DISC` and writes to
`H2_UI_PREVIEW_DIR`; never into git. The texts the game fills at run time
(gamertags, status, level) are stand-ins, and the level screen's list is
not drawn.
