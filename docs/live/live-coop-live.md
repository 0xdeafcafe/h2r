# Live co-op: over Live, latency and prediction

Latency, and prediction.

## 10. Co-op over Live (built)

Built on tests, on the owner's decision to assume the probe's approach
(the retail synchronous campaign path, two consoles, one player each)
works and to test it all at the end. **No game was built or run for it.**
If the retail path breaks, only the start part (10.3) is redone; the
screens, the session plumbing, the text and the web stay.

### 10.1 What a player sees

**Host.** Xbox Live menu: Quickmatch, OptiMatch, Clankermatch, **Co-op
Campaign**, Custom Games, Forge, Firefight, Custom Game Browser, Mods
([live-custom-games.md](live-custom-games.md) §5). Seven rows show and
the list scrolls to the rest. Co-op Campaign has its own pane (the
Custom Game picture, our help text).

It opens the co-op screen:

| row | value | press |
|---|---|---|
| Level | the level's name as the level select shows it (read from the disc's `mainmenu.map` at boot), else its map stem | the next level |
| Difficulty | Easy, Normal, Heroic, Legendary | the next one |
| Invite a Friend | | opens the friends screen |
| Status | `Invite a friend`, `Waiting for <gamertag>`, `<gamertag> is in; ready`, `... the lobby holds the level`, or what blocks Start | logs the status |
| Ping | the round trip to the guest, `42 ms`, or `-` with no guest (10.6) | logs it |
| Start | Ready / Not ready | when ready: the campaign menu's setters put the level and difficulty on the session (as the probe's tick setup does) and the screen closes, back to the lobby; otherwise logs why not |

Start is enabled when the squad session is in a lobby or game state with
exactly two consoles and at most two players, and the level has campaign
and map ids. The friends screen lists the host's friends (online first, at
most 20) with Online, Playing, their presence activity or Invited; a press
on an online friend sends a Live game invitation to the host's advertised
lobby. B on any of these screens closes it as on every screen of ours.

Another Live menu row ends hosting: the lobby becomes a custom game again
and the hooks disarm.

**Guest.** A console that joined a host's co-op session (an accepted
invite, joining a friend from the friends list, or the custom games
browser) is a co-op guest. In the browser, **Y** opens filters for
**Campaign co-op**, campaign title and difficulty.
[Browser controls](live-custom-games.md#filter-games)

While the guest shares a squad with the host, a watcher asks the HLE
once a second which session it joined. When the server reports a `coop`
session, the console arms (10.3), takes the host's skulls for
the campaign start, and opens the join screen at the next tick
(`H2_COOP_JOIN_SCREEN=0` leaves that to the Co-op Campaign row, which opens
the join screen for a guest): Host (`<gamertag>, 42 ms` once a ping came
back), Level, Difficulty, Skulls (`N from the host`, ours counted),
Handshake (`OK: the lobby holds the host's campaign`, or `Waiting
for the host's campaign`, `The lobby's difficulty differs`, `N consoles in
the lobby`, `No lobby`) and Ready (closes the screen; the guest waits for
the host's start in the lobby).

**Text.** The retail lobby line saying co-op needs two players on one
console (string id `0x0e0001ec`,
`coop_takes_two` in `mainmenu.map`'s strings) is the only text of that kind
on this path: the code names that id once, in the lobby's status
line (`0x24ffc9`, at `0x2500ab`), for start state 7 in co-op mode
(`0x199994`). Start Game (`0x251050`) acts only on state 9 and shows no
dialog otherwise, so the status line is where a refused start explains
itself. On the Live co-op path it reads ours (`coop_live::line_for`):

| When | Line |
|---|---|
| Co-op Campaign chosen, no session read yet | `Co-op over Xbox Live: 2 players, one per console.` / `Press Y in the lobby to invite a friend.` (`up to N players` above 2) |
| the host alone in its session | `Invite a friend first: co-op over Xbox Live` / `needs at least 2 consoles. Press Y for Friends.` |
| more players than consoles | `Co-op over Xbox Live: one player per console.` / `Sign out the extra players to start.` |
| more consoles than the limit | `Co-op over Xbox Live takes up to N consoles;` / `M are in this lobby.` |
| a guest (joined, or in a host's networked lobby before it knows) | `Co-op over Xbox Live: one player per console.` / `Waiting for the host to start the game.` |

It is ours from the moment the Co-op Campaign row is chosen (the role is
the host's before the row's original runs), and, with Live on, on any
console in a networked squad session (class not 0): a friend who accepted
an invitation sits in the host's co-op lobby before the host's
advertisement says co-op, and read the retail line until then. The tick
reads the session, picks the line and writes it, in that order.

Split screen keeps the game's own text: no session, or a local one (class
0), and not hosting or joined. So does a system link lobby with Live off.
Every text of the co-op screens is ours: guest memory slots of 128 bytes under ids `0x20ffd000` up, rewritten when a row
is drawn and every tick.

Not `0x20ffe000`, the first choice: the community
variants answer index `0xffe0xx` whatever the top byte, so the co-op screen
showed their names (a test now checks every id against every private range).

**Skulls.** The host's own campaign selection (collected and on, for the
signed-in profiles: the Skulls page's rule, so the host has them available)
is advertised on the co-op session in place of the multiplayer set, and the
host's campaign start applies it as for any campaign. A co-op guest's
campaign start applies exactly the host's skulls instead of its own
profiles': the engine's into the array (`skulls::coop_start`), and our own
(from 15, `coop_live::ours_of`) to `skulls_custom`, which acts on them as on
the host. The host advertises both (`coop_live::host_campaign_skulls`).

### 10.2 Session plumbing

- **Advertising.** Opening the co-op screen calls `Hle::set_coop(campaign,
  skulls)`. The lobby the game advertises (and any it already advertised,
  through a signed `SessionUpdated`) becomes `flags: "coop"` with a
  `campaign` (`live_proto::session::Campaign`: `level` stem, `name`,
  `difficulty` 0 to 3) and the host's campaign skulls. A public lobby is
  made friends-only (`coop_visibility`); invite-only stays invite-only.
  Leaving sends `flags: "custom"`, which drops the campaign.
- **Presence.** The HLE sends `activity: "Co-op: <level>"` with every
  presence change while armed (`live_proto::valid_activity`, at most 64
  characters); the server keeps and serves it ([live-api.md](live-api.md)).
- **Invites.** `Hle::invite_to_lobby` sends the same social `Send` the
  title's `XOnlineGameInviteSend` does, for this console's advertised
  session. The guest accepts with the retail invite flow and joins through
  the existing XNet/session path.
- **The guest's read.** `Hle::joined` (what `host_skulls` now uses too)
  returns the joined session as the server last described it, asking again.
- **Server and web.** `GET /v1/sessions?flags=coop`; finished co-op games
  at `GET /v1/coop/games` (a closed `coop` session that a guest joined: the
  host and everyone who joined, the campaign, start and end, the session's
  visibility), shown on the web's Games page (Campaign) and a player's
  Co-op tab. A friends-only game shows only to who played in it and the
  host's friends, an invite-only one only to who played in it, admins see
  all (live-api.md, "Co-op games"); the same holds for the game's telemetry
  in the games list and player pages, for the log (`/v1/events`,
  `/v1/stream`: a hidden game's and session's events go only to who may
  see them) and for the stats summary (non-admins get the public one). A
  running game follows its session's visibility when the host changes it,
  and keeps the last one once it ends. The web reads all of these as the
  signed-in account (a `read` token, and `?auth=1` on the stream), and
  reads again at once on a sign-in or sign-out. Presence follows the
  same rule: a reader who may not see the host's or guest's co-op game
  sees them "In a game", with no game id and no "Co-op: <level>".
  Visible co-op sessions show as "Co-op campaign" with level, difficulty,
  player count and the host's ping in the stats home's Open Games table.
  Friends' activity shows in the friends lists.

- **Ping** (10.6): the host's `ping_ms` on the session (`Update { ping_ms }`,
  0 clears it; left out until measured), shown on the web's co-op rows.

### 10.3 The hooks (the probe as a feature)

The probe's hooks of 9.1 are now also the feature `coop-live`, on the plan
whenever Live is on (`H2_FEATURES=-coop-live` takes them out), armed only
for a co-op session (`coop_probe::Arm`): the host arms with its level, a
guest with none. Disarmed, every hook lets the game run as retail and
nothing is logged or hashed. Armed, they are exactly the probe's: the
peer-count window on the session start `0x6ec80` / `OPTIONS_NEW`
`0x138110`, the peer limit back to 2 after `0x6e720`, the joinable
predicate `0x63e00`, the campaign setters `0x19a6f2` and `0x19a76d` on the
host's lobby once a second console is in (three attempts, reset when the
host changes level or difficulty), and all of 9.2's logging, with the hash
files in `<data>/coop-probe/`. `H2_COOP_PROBE=1` still forces the probe on
for the whole run (System Link), and then the co-op screens only advertise
and invite.

### 10.4 Your test (two instances, or two Macs)

Same build on both; nothing here needs a build of ours beyond your usual
`h2`. One Live server for both. Make the two accounts friends.

```sh
# Terminal 1: a server for both (loopback: clients send tokens over
# plain HTTP only to loopback).
h2 live serve
IP=$(ipconfig getifaddr en0)      # the XNet address below, not the server's
h2 live account create Alpha
h2 live account create Bravo
h2 live friend add Bravo --as Alpha
h2 live friend accept Alpha --as Bravo

# Terminal 2: the host.
H2_XNET_ADDR=$IP:31000 H2_XNET_LOOPBACK_PEERS=$IP:31010 \
h2 play disc/halo2.iso --data ~/h2-coop/host --live-server http://127.0.0.1:8074 \
  --gamertag Alpha 2> >(tee ~/h2-coop/host.log)

# Terminal 3, once the host is up: the guest.
H2_XNET_ADDR=$IP:31010 H2_XNET_LOOPBACK_PEERS=$IP:31000 \
h2 play disc/halo2.iso --data ~/h2-coop/guest --live-server http://127.0.0.1:8074 \
  --gamertag Bravo --no-audio 2> >(tee ~/h2-coop/guest.log)
```

On two Macs: drop `H2_XNET_LOOPBACK_PEERS`, use each Mac's own address with
port 3074 (`H2_XNET_ADDR=<this Mac's IP>:3074`), and open UDP 3074 to 3080.
The server must be reachable over HTTPS, since a client will not send a
token over plain HTTP to a LAN address: `h2 live serve --tunnel`
(or `h2 play --live-tunnel` on the host), and `--live-server
https://<that URL>` on both ([live-hosting.md](live-hosting.md)).
`h2 play --live` (the default service) works the same way when both
accounts are on it and friends there.

Checklist (stderr lines in brackets):

1. Both: sign in to Live. stderr: `coop-live: N campaign levels (M named)`
   (N about 15 on the retail disc; 0 means the disc read failed and the
   Level row says None).
2. Host: Xbox Live menu. Co-op Campaign sits directly above Custom Games (row 3).
   Moving onto Co-op Campaign shows the Custom Game
   picture and our help line. [`custom games: xbox live menu tag has 10
   panes and shows 7 of 9 rows`]
3. Host: Co-op Campaign. The screen shows Level, Difficulty, Invite a
   Friend, Status, Ping (`-` for now), Start. [`coop-live: hosting a co-op
   campaign`]. Step the
   level and difficulty [`coop-live: level ...`, `coop-live: difficulty
   ...`]. The friends list on the web shows Alpha's activity
   `Co-op: <level>` once the game advertises its lobby.
4. Host: Invite a Friend, press Bravo [`coop-live: invited Bravo to session
   <id>`]. `could not invite ... is the lobby advertised yet?` means the
   game had not advertised a lobby at that moment: note where you were.
5. Guest: accept the invitation (retail invite flow). The join screen opens
   [`coop-live: guest: joined Alpha's co-op session ...`, `coop-live:
   guest: opening the join screen`]. Check its rows. Press Ready.
6. Both: within a few seconds of the guest joining, the host's Ping row
   and the guest's Host row show the round trip [`coop-live: ping N ms`
   on both; `coop-live: ping: no answer` means the guest's pings to the
   host's port + 2 got nothing back: a firewall on UDP base+2, or the
   session's endpoint is not the host's reachable address]. The web's co-op
   session row shows the same number.
   Host: Status says `Bravo is in; ready`, Start says Ready. Press Start
   [`coop-live: host: LOBBY_SET_MAP returned 1`, `... LOBBY_SET_DIFFICULTY
   returned 1`, `coop-live: host: start <level> on <difficulty>; back to the
   lobby`]. `returned 0 (refused)` is the first thing to send back: the
   setters' own host check (`0x63f00`, session class at `+0x4980`) may
   refuse a Live (not System Link) session.
7. The lobby line about co-op on one console reads ours on both.
8. Host: start the game from the lobby. From here it is section 9's
   probe: `coop-probe: session start ...`, `campaign with 2 peers: showing
   the start 1 peer`, `session start returned 1`, then `game start: options
   mode 1, simulation mode sync-server (2)` on the host and `sync-client
   (3)` on the guest, and the hash files. 9.4 says what each failure looks
   like. Play a few minutes: both move, a fight, a checkpoint, a cutscene.
9. Skulls: with a skull on in the host's profile, the guest's stderr says
   `skulls: co-op campaign start with the host's skulls: ...`; with one of
   ours on (Low Gravity, say), also `skulls: ours on this campaign: ...`,
   and the guest feels it.
10. Afterwards the web's Games page (Campaign) lists the co-op game with
    both players once the host's lobby closes.

What to send back: `host.log` and `guest.log` (or `grep -E
'coop-(live|probe)|skulls:|custom games' *.log`), the newest `.hashes` from
each `<data>/coop-probe/`, `h2 coop-probe diff <host>.hashes
<guest>.hashes`, and a sentence on what you saw and where it stopped.

### 10.5 What is tested

Since the first cut: the ping (`qos.rs` unit tests, the server test
below pinging the host's responder and seeing `ping_ms` arrive,
`coop_live` tests of both sides), our skulls carried, and every family
of our string ids checked against every other (`boot/src/string_ids.rs`).
`cargo test -p boot --lib coop_live` and the custom games tests: the
level list (start order, Halo 2's campaign only, the level select's
names, from synthetic globals; `#[ignore]` against `$H2_DISC`), the text
slots, the host flow (arming, advertising level, difficulty and skulls,
stepping, leaving), Start's checks and the status lines, the friends screen
and invites, the guest's arming, join screen and handshake lines, a custom
session or the host itself never being a guest, the lobby line only in a
co-op session, the guest's skulls at the campaign start, the six-row Live
menu and its sixth pane on a synthetic tag, and the hooks on the plan, off
until armed. `cargo test -p live-hle --test server
a_coop_lobby_advertises_its_campaign_and_the_invited_guest_reads_it`
against the embedded server: a friends-only `coop` advertisement with the
campaign and the campaign skulls, a stranger's browser not listing it, the
friends list, the invitation reaching the guest, the guest's join reading
the campaign, presence activity, and leaving back to `custom`. Server and
web: `crates/live/live-server/tests/coop_sessions.rs`, `web/test/coop.test.ts`.

Not tested, because only the game can: whether our screens open over the
Live lobby and look right, whether the join screen opens cleanly from the
tick, whether the setters take on a Live session (10.4, step 6), and
everything from the session start on (section 9).

### 10.6 Ping

**Mechanism.** The HLE's QoS side channel: every responder (the host's
XNet base port + 2, already there for the title's QoS probes) also answers
a co-op ping, `H2PNG001`, nonce, last round trip (u16 ms), with
`H2PNR001` and the nonce, for any sender. The guest's pinger thread pings
the host every 2 s (1 s timeout) at the session's endpoint as the server
saw it; each ping carries the previous round trip, so the host learns the
guest's number from the next ping, and both show the same one. The host
keeps its responder up while co-op is armed (`Hle::guest_ping`), takes the
slowest report of the last 6 s, and advertises it as `ping_ms` when it
moved by at least 5 ms and a tenth (`live_hle::ping_moved`).

Why this one: it measures the path the game's own traffic takes (the same
host and port family, peer to peer), with no new port and no server in the
middle. A server round trip would measure each console's way to the
server, not to each other; the phase-0 transport (`coop::link`) is not
running in the retail-synchronous path co-op over Live uses. The friends
screen shows no ping: friends not in the lobby have no endpoint to ping.

## 15. Latency, and prediction

The question (2026-11-18): can the network game state be made to lag
less, and how does the game predict? Co-op over Live runs the retail
synchronous path: one console builds every tick's update from everyone's
actions, the others run only the updates they receive. A guest's own
simulation action therefore takes a round trip.

First-person look has a
local camera path as well (15.4). This section is a static
reading (`h2 disasm`) and our own code; no game run. The `coop-live:
latency` line (15.4) is how a run will put numbers on it.

### 15.1 Which console does what

[local] By what the code does (the simulation globals `*0x004cf77c`,
mode at `+8`):

- **The update builder** (world mode 2, the sync-server, in the "after" step `0x000684e0`):
  `0x00069880` walks the peer views and `0x000860b0` sends each the
  whole tick's record (message `0x27`) **at once**, through the channel
  send `0x00075e80`; a view still joining gets it queued instead
  (`0x000869a0`). No batching of ticks at this layer.
- **A follower** (world mode 3, the sync-client): `0x00069040` sends this console's actions for
  the tick (message `0x28`, `0x180` bytes: a sequence number, the update
  number minus one, up to four local players' `0x5c` byte actions) at
  once through `0x00085fc0`, and takes its records from the queue the
  network fills (`0x0006ac30`). The `0x27` handler `0x00094990` calls
  the catch-up `0x000698e0`, which runs `game_tick` while ticks are
  pending: an update is simulated as soon as the game reads it.
- [local] The name table at `0x00468c64` puts sync-client at index 2 and
  sync-server at 3, the reverse of these behaviours, because the table
  names the game mode and this is the world's (section 1.1: the jump table
  at `0x00068da8` maps game 2, 3 to world 3, 2). Settled.

[local] The update record is bit-packed (`0x000b2710` encodes through
`0x00087d00`, field by field), so the `0x4048` passed to the channel is
a ceiling, not the datagram size.

**Built. Converted Halo 1 cinematic ticks.** `game_skip_ticks` requests
up to 15 extra ticks. In world modes 1 and 2 the authority dispatches
the extra native simulation updates after the normal batch while the
cinematic is active. In mode 2 each tick takes the ordinary sequenced
`0x27` path, including the joining-view queue.

Mode 3 consumes those
updates through the existing catch-up path. A local script on the follower
has no authority to schedule an additional batch.

In the host-level protocol, wire version 2 carries a first frame and a
contiguous window of one-byte cinematic tick counts, with a separate
cumulative acknowledgement. Both peers use `Peer::begin_cinematic_frame`
from frame zero, including a zero count on ordinary frames. The first host
decision for a frame is final.

The guest holds until it has that count and
both pads. Unacknowledged counts travel again with the next datagram.
The peer test exercises loss, reordering and conflicting guest requests.

### 15.2 The budget of one press

A guest presses a button; how long until its own screen shows it. Times
for a LAN, a 30 Hz tick (33 ms) and the window at 60 Hz.

| hop | what waits | size or period | whose | tunable |
|---|---|---|---|---|
| 1 | the press waits for the game's pad read (`XInputGetState`, once a tick) | 0 to 33 ms, 17 on average | retail [guess: once a tick] | no |
| 2 | our pad layer: host devices feed the XInput state as they report (the Steam pad thread polls every 4 ms, `input::steam_pad`) | 0 to 4 ms | ours [local] | not worth it |
| 3 | the follower sends its actions (`0x00085fc0`, at once) | 0 | retail [local] | |
| 4 | the channel and link layers (`0x00075e80` to `0x00095580`) packetise and hand to XNet | [guess] same frame | retail | no |
| 5 | our Winsock HLE (`live_hle::network`): one `sendto` per datagram, no queue, no timer | microseconds | ours [local] | `H2_XNET_LOWDELAY` (15.3) |
| 6 | the wire: direct UDP between the XNet ports (base to base+6); the Live relay is not on this path (it carries only the phase-0 pad link) | LAN under 1 ms wired, 2 to 10 ms Wi-Fi | network | wired beats Wi-Fi |
| 7 | the builder reads its sockets once per main-loop pass, then builds the next tick's update from the actions it has (it does not wait for late ones) | 0 to 33 ms | retail [guess for the poll; local that the build does not wait] | no; measured as "guest input waited" |
| 8 | the update goes out (`0x000860b0`, at once) and back over the wire | as 4 to 6 | | |
| 9 | the follower reads its sockets once per main-loop pass, then the catch-up simulates at once | 0 to 33 ms | retail [guess for the poll] | no |
| 10 | the frame is drawn and presented (our render thread, vsync) | one display frame, 17 ms at 60 Hz | ours [local, high-framerate.md] | `graphics.fps` |

Split screen on one console pays 1 and 10: about 35 ms. The synchronous
path adds 6 twice, 7 and 9: on a wired LAN about 35 ms more on
average (up to 70), so about 70 ms in all; over the internet add the
round trip. [guess] None of the added part is ours: our layers add
microseconds (5) and a display frame we already pay offline (10). There
is no fixed input delay in the synchronous path [local: the follower
sends at once, the builder builds without waiting]; the waits are the
two 30 Hz polls and the wire.

### 15.3 What was changed (ours, tested)

- **The latency line** (`boot::coop_latency`, feature `coop-latency`,
  on the co-op arm; off with `H2_COOP_LATENCY=0`). Once a second:
  `coop-live: latency guest: 30 updates/s, apart 33/33/34 ms; input to update 41/52/80 ms (min/median/max); net 31 in / 30 out datagrams/s, 6.2 / 2.1 KiB/s, largest 900 B`
  on a guest, and on the host
  `coop-live: latency host: 30 updates out/s; guest input waited 2/17/33 ms for the next update (min/median/max); net ...`.
  "Input to update" is from the guest sending its actions to the next
  update arriving: hops 4 to 9 minus the guest's own poll. The points
  are `SIM_ACTIONS_SEND` (`0x85fc0`), `SIM_ACTIONS_RECEIVE`
  (`0x85f50`), `SIM_UPDATE_SEND` (`0x860b0`) and `SIM_CATCH_UP`
  (`0x698e0`), observed only. Tests: the window's min/median/max, a
  guest's and a host's measurements on made-up timelines, the traffic
  rate, the switch, the plan.
- **Traffic counters** (`live_hle::traffic`): datagrams and bytes each
  way and the largest datagram, for the line. A datagram near or over
  the path MTU (about 1400 bytes) is fragmented, and one lost fragment
  loses the update: the line shows whether that happens.
- **A kept receive buffer.** Our `recv`/`recvfrom` allocated and zeroed
  a 64 KiB buffer on every call, and the game polls its sockets every
  pass, mostly finding nothing; a buffer kept per thread now does. Work
  removed, timing unchanged, so no switch.
- **`H2_XNET_LOWDELAY=1`** marks the title's UDP sockets IP_TOS `0xb8`
  (DSCP EF): on Wi-Fi with WMM they go in the voice queue, the one that
  waits least when the air is busy. Opt-in (it changes queuing), best
  effort (a refused option is ignored).

Checked and left: there is no relay hop on the game path (6), no
Nagle (the game's sockets are UDP; `TCP_NODELAY` passes through for
any TCP), and our `select` waits exactly as long as the game asks.

### 15.4 Prediction

**Retail's distributed modes** (multiplayer). [local] In the "after"
step, mode 4 calls `0x00069a00` with the record's `+8`/`+0xc` and
`0x00069b50` with the player mask `+0x5cc` and the per-player entries
`+0x610` (`0x7c` bytes each), where the synchronous modes send the
whole record. The image names "simulation distributed view",
"replication-entity", "replication-event", "replication-game-results"
and a "predicted data" pool (`.rdata` strings; docs/game/theater.md).
[community] Halo 2 multiplayer is host-authoritative: a client
simulates its own player from its own input at once and sends its
control to the host, the host replicates objects (entities) and events
back, and the client's own player is corrected when the host's state
disagrees. [guess] The per-player entries are that control, and the
"predicted data" pool the client's speculative copies. Not traced: the
correction (how a client reconciles its own biped with the host's).

**The synchronous path has none.** [local] A follower's ticks run only
from received updates (the catch-up); nothing runs ahead of them.

**Could campaign run distributed?** [guess] Not cheaply. Replication
carries objects and events; campaign also needs AI (actors, squads,
encounters), the scripts (hs threads, cinematics, the camera, objectives,
BSP switches and zone sets, checkpoints) and device machines, which
multiplayer never replicates because it never has them. In distributed
mode a client would show replicated units without the state that drives
them, and every script effect that is not an object (a cutscene camera,
a BSP switch, fade, music, a checkpoint) would need replicating. That is
a new replication layer for most of campaign: months, with the
synchronous path's exactness given up.

**Prediction on the synchronous path.** Look prediction is in progress.
The synchronous simulation and its input schedule still provide the
authoritative aim.

1. **Guest look prediction (implemented, game measurement pending).**
   `coop.look_prediction` defaults to true. `H2_COOP_LOOK_PREDICTION=0`
   disables it. Only an armed synchronous guest (world mode 3) draws
   predicted look. The host uses its ordinary camera.

   The outgoing action already contains the result of the look-rate rule.
   `0x1870a0` runs local player control, and `0x186ab0` writes absolute
   yaw and pitch in radians to action `+4/+8`. Sampling the actual
   `0x28` in its channel encoder (`0xb2790`) uses the same response curve, axis speeds,
   acceleration, pitch limits, aim assist and zoom that produced the
   host's input. A second stick integrator would have to reproduce all
   of those, including per-profile settings and map tags.

   Each local user has a separate queue. A send carries its sequence and
   the last update the guest saw. When `0x683a0` applies a newer numbered
   update, the guest reads its own player slot's action from the record.

   The wire carries that action and omits the sender's sequence. The
   guest matches the earliest eligible send within one wire cell (13-bit
   yaw, 12-bit pitch). Identical looks are ambiguous but give the same
   orientation.

   Earlier unmatched sends are superseded. After 30 newer
   updates an unmatched send expires, with a hard limit of 64 sends.
   Replaying absolute aims leaves the newest pending aim as the target.

   [local] The on-foot first-person director (`0x23c110`) already reads
   local player-control angles. A network wait for a simulated action
   does not establish an equal wait for the drawn look. `boot::coop_look`
   compares the drawn camera with the turn already sent and adds only the
   remaining rotation. A camera already showing the sent look needs zero
   adjustment.

   Only `render_game_view` receives the private view record,
   with its two camera bases rotated together. Small corrections settle
   over two draws. Errors of 30 degrees or more snap.

   Death, loss of local
   control, a changed unit or zoom, a teleport, a vehicle or a cinematic
   discard pending look. Tests cover the packet and update layouts on a
   fake guest, private camera copies, wrap, loss, blending and the gates.
2. **Full prediction (rollback).** Run the guest's game ahead on its own
   input and roll back to each received update, replaying. It would
   remove the round trip from movement too. Cost: a game-state snapshot
   (`0x3fe000` bytes) per tick and re-simulating several ticks per
   frame (each tick is real guest work); sound, effects and the render
   would replay; the snapshot work (docs/runtime/snapshots.md) is the
   base. Months, high risk; not before 1 is measured.

### 15.5 What needs a game run

Measure before changing the input schedule. Take a prediction-off
baseline with both consoles on the same map and repeat with controlled
delay and jitter. Record RTT, guest input-to-update, host wait and
update spacing.

Compare prediction on and off on the same routes and
look inputs. Measure the time to the first displayed turn and the size
and frequency of corrections when the authoritative aim catches up.
Check that a cut or reconnect clears stale prediction and that the
simulation sees the same inputs.

Ping-aware host selection is being
added to SBMM for distributed matches. [Matchmaking](live-matchmaking-search.md#9-ping-aware-optimatch-assignments)

`h2 coop test --players 2 --auto`. In a level, read the two `coop-live: latency`
lines once a second and the `coop-live: ping` line:

- the guest's "input to update" median should be close to ping plus
  the host's "guest input waited" median; the rest is the two polls;
- "apart" near 33 ms says one update per tick; larger gaps are the
  host's frame rate or loss;
- "largest" over about 1400 B says updates fragment;
- then the same with `H2_XNET_LOWDELAY=1` on both, on Wi-Fi, and
  compare the medians and maxima.

**Look prediction run checklist (code-only lane, pending).**

1. Start `H2_COOP_LATENCY=1 h2 coop test --players 2 --auto`, join the
   same campaign level on both instances and wait for an on-foot view.
   Save both latency lines and ping. On the guest socket, record a turn
   with `input.record {"name":"coop-look"}` and `input.stop`. Include a small deflection,
   a held full turn, a reversal, vertical motion and a zoomed turn.
2. Read `render.coop_look` on both sockets during the sequence. On Bravo,
   compare `predicted` with `confirmed` while `pending` is positive.
   Compare `drawn` with `camera` to measure how much retail already
   showed. Record the lag in guest frames between the first drawn turn
   and the confirming update, and the maximum `correction_rad`. Alpha
   should report `host`, zero pending and identical camera angles.
3. Stop the stick and let the last changed look reach a newer update.
   The angles should converge within wire precision. Repeated stationary
   sends may remain pending. Check zoom, death/respawn, vehicle entry
   and exit, a cinematic, and a checkpoint revert. The first view after
   a cut should show the new camera without carrying the old turn.
4. Repeat on the same level with `H2_COOP_LOOK_PREDICTION=0` and the
   recorded pad sequence. Compare drawn-turn timing and the latency lines.
   Record a zero visual gain if retail already showed the turn in both
   runs. Check the retail desync latch and compare host/guest simulation
   pools at common ticks with `h2 coop-probe diff` as in §9. Report any
   new divergent simulation pool and the first differing tick.

For the recorded-stick regression, save one Bravo `render.coop_look` JSON
reply covering the on-foot sweep and its echoes. The test consumes that
file and the corresponding XAPI input log offline. Set
`H2_COOP_LOOK_CAPTURE` to the JSON path and `H2_COOP_LOOK_INPUT` to the
`inputs/coop-look.input` path reported by the recorder.

Include
`cargo test -p boot --lib recorded_stick_turns -- --ignored` after the
capture. The ordinary `cargo test -p boot --lib coop_look` tests use
synthetic packets and require neither a disc nor a recording.
