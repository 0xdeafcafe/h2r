# Live telemetry

`crates/live/live-telemetry` observes the retail game state described in
[live-gamestate.md](live-gamestate.md) and produces the events and frames in
[live-api.md](live-api.md). It is on when Live is on or `H2_TELEMETRY_FILE` is
set ([Server ingest](#server-ingest)), and `H2_TELEMETRY=0` turns it off. The crate does
not change guest state or return telemetry failures to the game.

## Sampling and hooks

Five overrides are installed after the generated module is registered (the last is optional):

| Guest entry | Observation |
|---|---|
| `0x0018f000` | After the original map activation returns success, read map/mode/variant, team play, score/time limits, tick, and the initial roster. |
| `0x00137fe0` | After the original tick, sample only when the game-time counter changed. Frames are emitted every second changed tick (up to 15 Hz). |
| `0x0015ce70` | Read the credited attacker, victim, victim position and packed damage code at entry. A teammate kill in a team game or campaign is a betrayal (`live_proto::game::is_betrayal`). Scores are reconciled at a later tick. |
| `0x001389c0` | After the original end-latch function, publish the final scoreboard and a unique high-score winner (or none for a tie). |
| `0x00104150` | Weapon barrel fire: read the weapon object datum at `[esp+4]`, count one shot for it, then invoke the original unchanged. Optional: if the entry is missing or already overridden, telemetry installs without it and frames carry no `shots`. |

The start, tick and end wrappers use shadow entries and runtime continuations to
observe after the original returns. If continuation setup fails, the wrapper
restores the guest stack and invokes the original directly. The kill wrapper
invokes the original immediately after its read.

At `guest_layout::points::PLAYER_KILLED`, the killer is the player who
receives credit. The weapon and headshot flag come from the damage report
that depleted the victim's body. These can have different sources. For
example, a player can earn credit for an opponent's fall after shooting
them. The kill then carries the shooter's slot and the source `fall`,
with no headshot flag. The observer reads the raw stack at this opaque hook
point, preserving the hook signature recorded in player films.
The same read identifies a delayed projectile after a weapon switch or a
thrower's death, and preserves the grenade type on a self-kill. A missing
or unrecognised source leaves the damage fields absent and preserves the kill.
[Kill hook and damage code](live-gamestate-objects.md#7-killdeath-and-damage-hooks)

Per-tick work follows only live player records and their current unit/vehicle
objects. It does not scan all 2,048 objects or the one-MiB object pool. A full
16-player selective snapshot is approximately 2–4 KiB of guest reads, including
592 bytes of player/team stat rows. Scores are compared every tick but emitted
only on change and at most once per 30 ticks. Names and match options are sent
on match/roster changes rather than in every frame. Vehicle passengers use the
parent vehicle's world transform rather than the biped's seat-local transform.
A replacement activation or a main-menu transition emits `GameEnded` with
reason `quit` before clearing the old session; post-end kills are ignored.

Fields rated **likely** by the research document are listed only in
`crates/live/live-telemetry/src/layout.rs::LIKELY_FIELDS`; they are not required by
the initial event contract. Confirmed offsets used by the readers share that
file so runtime validation has one correction point.

## Frame species, aim, weapon and shots

Moved to [Live telemetry: frame species, cinematics and appearance](live-telemetry-frames.md#frame-species-aim-weapon-and-shots).

**Campaign.** What a campaign checkpoint does to the tick stream: [live-telemetry-frames.md](live-telemetry-frames.md#frame-species-aim-weapon-and-shots).

## Cinematics

Moved to [Live telemetry: frame species, cinematics and appearance](live-telemetry-frames.md#cinematics).

## Velocity, seat, vitality and fire

Moved to [Live telemetry: frame species, cinematics and appearance](live-telemetry-frames.md#velocity-seat-vitality-and-fire).

### Rate: 15 Hz, players included

### Bandwidth (estimates, until the bench runs)

## Player appearance

Moved to [Live telemetry: frame species, cinematics and appearance](live-telemetry-frames.md#player-appearance).

## Threads

The game thread only reads: on each changed tick it fills a snapshot (the
roster, team scores, the players' objects, and on frame ticks the world
scan's actors, vehicles, machines, placed objects and bodies, all plain
values) and queues it with the shots and hits the hooks counted. A start,
kill or end is read and queued the same way, and the playlist and skull
settings are queued in order with them. One worker thread
(`crates/live/live-telemetry/src/pipeline.rs`) runs the session over the queue
in order: it builds the events and frames (ids, species, weapon, team and
stance names, actor order, placements), loads the map's tag names at a
match start, and hands everything to the sink, which diffs, packs, signs,
serialises and sends on that thread too.

Ticks do not wake the worker: on the game thread a wake is a system call
(a futex wake on Linux, `pthread_cond_signal` with a waiter on macOS). In
a game the worker polls the queue every 33 ms (one tick), so a tick waits
at most that long before the worker takes it, and a batch is usually two
ticks. After a second with no work (a menu, a paused game) it parks, and
the next tick wakes it once. Starts, kills, ends and settings always wake
it; they are a few a minute.

The worker hands each snapshot back cleared and the game thread fills it
again, so a steady game allocates nothing for it: the lists keep their
room, and the roster keeps its names for the next read while the players'
datums and names are unchanged.

What the game thread does per tick, and nothing else:

| step | what | cost |
|---|---|---|
| weapon-fire hook | add one to the weapon's count: a sorted list that keeps its room (`event::Shots`) | a lock nobody else takes, a binary search |
| damage hook | read the report's fields, push one record (newest 256 kept) | a few guest reads |
| tick hook | read the game clock; an unchanged tick stops here | one read |
| | take a cleared snapshot from the worker's spares | an uncontended lock (the worker holds it for one push) |
| | read the roster (no names while unchanged), team scores, the players' objects | about 5 µs for 16 players |
| | frame ticks: the object-header scan into kept lists (actors, vehicles, machines, placed objects, bodies) | 5–25 µs (bench) |
| | swap the hooks' shots and hits into it, push it | an uncontended lock (the worker holds it to swap the queue out) |

No allocation once the lists have grown (the first few ticks of a game),
no system call, and no lock the worker holds for more than a swap or a
push. The guest is read as raw values (positions, datums, flags);
everything that turns them into a frame (names, ids, stance, speed,
actions, actor order, diffs against the last frame, packing) is the
worker's. Starts, kills and ends allocate (they read the roster and, at a
start, the scenario's placements); they are events, not ticks.
`bench::the_game_threads_tick_does_not_allocate` (not ignored, in
`cargo test -p live-telemetry`) fails if a steady tick allocates on the
game thread in any bench scene, and if an unchanged tick does.

The queue holds every start, kill, end and setting. Ticks are bounded: with
four waiting (the worker behind, say a slow sink), a new tick is folded
into the last queued one rather than queued. The later of the two is kept,
except that a frame tick (even) outlasts a later other tick, so a worker
behind still sends frames; the one kept takes the other's shots (added)
and hits (in order, at most 256). The roster and scores of a tick that goes
are read again the next tick, so only its moment is lost.

## Failure behaviour

Observer errors and panics are caught, logged once, and disable telemetry. A
sink error disables later observations without affecting the guest. Override
installation refuses to replace an existing hook; boot logs the error and
continues without telemetry.

The worker's queue is bounded (256 entries). Ticks past four waiting are
folded (see "Threads") and counted. A start, kill or end is never dropped
to make room: if the queue is full, the observer reports an error and
disables itself rather than blocking the guest thread. The worker finishes
everything queued during shutdown. TCP connect, TLS/WebSocket handshake, HTTP,
and socket operations are time-bounded, and the worker finishes its event drain
before shutdown completes.

## Offline JSONL capture

Set `H2_TELEMETRY_FILE` before `h2 play` or another boot entry point:

```sh
H2_TELEMETRY_FILE="$TMPDIR/h2-telemetry.jsonl" h2 play
```

The file contains one JSON object per line. Events use
`{"type":"PlayerKilled","game_id":"...","data":{...}}`; live frames use
`{"type":"GameFrame","data":{...}}`. These are local capture records, not
server `WireEvent` objects: they deliberately have no server positions, hash
chain, or signatures.

`ActorKilled` comes from the damage-report observer when a hit depletes
a unit with a live actor link and no controlling player. The stream
records its salted unit and character datums, the credited player's slot
and the damage-source key when available. The same row goes into the
public stats stream alongside the recording.

`FirefightScore` and `FirefightEnded` come from the Firefight host
runtime ([firefight.md](../game/firefight.md#scoring-and-stats)) through
`Telemetry::firefight_score` and `Telemetry::firefight_ended`: the score
for each `Score` command the director credits, the result when it
finishes the match. With no game running they are dropped. They go to the
public stats stream and the film (as an `ObjectiveScore` of game type
`firefight`), not to the private import rows.

`GameMedal` (`tick`, `player`, `medal`) comes from the observer on the
game's medal award function `GAME_MEDAL` (`0x1970a0`,
[live-gamestate.md](live-gamestate.md#medals)). The observer looks the
medal index up in the official table (`medals::official_at`), drops an
index outside it, and queues the session's `medal`, which sends the event
at the match clock for a player of the match. The id is the game's own
(`double-kill`, `carjack`, `flag-save`); Live counts it per player
([live-api.md](live-api-theater-stats.md#medals)). An image without the hook drops the
observer with a plan note.

`Superbounce` (`tick`, `player`, `chain`) and `Bounced` (`tick`, `player`)
come from the superbounce mode through `Telemetry::superbounce` and
`Telemetry::bounced`, which read the game tick and queue the session's
`superbounce` and `bounced` ([superbounce.md](../game/superbounce.md#recording)).
They are sent at the match clock for a player of the match, and not in a
cinematic or after the end. `Teabag` (`tick`, `player`, `victim`) comes
the same way from the teabag watch through `Telemetry::teabag` and the
session's `teabag`, for two players of the match who are not the same one
([teabag.md](../game/teabag.md#recording)).

## Server ingest

Starting `h2 play` with Live on (`--live`, `--live-server` or
`--live-local`) enables telemetry automatically: boot reads the
first account in the Live `accounts.json`, uses its private JWK, and uses the
Live `server_url`. No telemetry-specific account flags or key environment
variables are needed. Set `H2_TELEMETRY=0` to disable both Live and file
telemetry.

Plaintext credentials are accepted only for loopback URLs; remote servers must
use HTTPS/WSS. The worker performs discovery lazily, creates each game through
`POST /v1/games`, opens `/v1/ingest`, authenticates with a fresh
`games.write` token and refreshes it before expiry. The sink in
`crates/live/live-telemetry/src/server.rs` sends unsigned frames and signs
up to 16 queued events from one game into a single JWS. It splits a group
if the signed message would exceed 16 KiB and sends `GameEnded` by itself.
The server rejects empty event batches and messages over 16 KiB.

With `ingest_frame_batch` advertised in discovery, the sink uses
`auth_batched` and combines [packed frame records](live-telemetry-packed.md#packed-frames) in
order. It flushes after 500 ms, at 16 records or the 16 KiB message
limit, before signed events or a JSON fallback frame, and at shutdown.
The worker wakes for the flush deadline even when the game is idle.
Older servers receive one packed record per message. Each connection
has its own stream with a keyframe every 150 frames and deltas between.
[Ingest wire format](live-api-social.md#streams-websocket)

### Reconnect

The ingest socket can drop at any time: a Worker deploy restarts the hub's
Durable Object and closes every socket to it, and networks fail. A drop does
not stop telemetry. Before this, the sink kept the dead socket, the worker
latched the first error, and the process sent nothing more: a game could
stay live on the server with its `GameEnded` never sent, and later games
were never created.

- **Backoff.** On a send, receive or connect failure the sink drops the
  connection and waits before the next attempt: 250 ms, doubling up to 30 s,
  each wait jittered by up to a fifth. A connection that stayed up 30 s
  before it dropped, or a batch or game the server takes, starts the wait
  over at 250 ms. The attempt runs on the telemetry worker, never the game
  thread.
- **A new connection.** Discovery runs again (a redeployed server may name a
  new API), then the socket opens and sends `auth` with a fresh token. The
  server keeps nothing per socket that a game needs: frames and batches
  carry the server's game id, and the game admits its host on any socket. A
  new connection is a new [packed](live-telemetry-packed.md#packed-frames) stream, so its first frame
  is a keyframe, which is what the server's new decoder needs. A frame
  refused with `frame: ...` (the server lost its decoder, as after a Durable
  Object wakes from hibernation) also makes the next frame a keyframe.
- **Frames.** A dead connection drops its pending frames. The sink also
  drops frames made during the outage or before the game is created.
- **The outbox.** Game starts and events are kept in order in a bounded
  outbox (4096) until the server has answered for them. A `GameStarted` is
  created with `POST /v1/games` when it reaches the front; the game's events
  wait behind it. Past the bound the oldest `ActorKilled`, `PlayerKilled`,
  `ScoreChanged`, `GameMedal`, `Superbounce`, `Bounced` or `Teabag` is dropped; starts, ends, joins
  and leaves never are.
- **Answers.** A signed batch leaves the outbox on its answer. A batched
  socket reports frame refusals as `frame_error`, so the sink can handle
  them while it waits for an event acknowledgement. Legacy sockets use
  `error` for both. A refusal settles after half a second with no further
  reply. No event answer in 10 s counts as a lost connection.
- **Resends.** A batch is kept as signed, so a resend repeats its nonces.
  If the first copy landed and only the answer was lost, the server refuses
  the copy with `conflict` "nonce already used" and the sink counts the
  batch delivered. The server seeds its nonce set from the log, so this
  holds across a restart, within the five minute `at` window. A batch
  signed more than four minutes ago is signed again (new nonces) before it
  is sent, since the server refuses an `at` over five minutes off.
- **Refusals.** `rate_limited` and `unavailable` are retried like a drop. A
  batch refused for good (`invalid`, `forbidden`, `not_found`, `conflict`
  "the game has ended") is dropped and logged; it could never land.
- **Giving up.** Only the account being refused stops the sink, and with
  it telemetry for the process: a `banned` refusal (HTTP, a batch reply, or
  a close naming the ban), or three auth refusals in a row (close 4401 or
  HTTP 401) with nothing accepted between.
- **Exit.** On shutdown the sink sends pending frames and tries to drain
  the event outbox over the open connection. It logs unsent events. The
  outbox is in memory only.

What a duplicate does on the server, when a resend is not the same signed
batch:

| Duplicate | What the server does |
|---|---|
| `POST /v1/games` retried after a lost answer | Makes a second game. The first gets no writes and the server's idle sweep ends it as `lost` |
| `GameEnded`, new nonce | Refused, `conflict` "the game has ended" |
| `PlayerJoined`, new nonce | Refused, `invalid` (the player is already in the game) |
| `PlayerKilled` / `ScoreChanged`, new nonce | Appended again: a kill counted twice, a score repeated |
| Any batch, same nonces | Refused, `conflict` "nonce already used"; nothing appended |

Sign-out does not stop telemetry: the sink signs with the account boot
read from `accounts.json`, for the life of the process, so a game's
`GameEnded` still goes after a Live sign-out mid-game. Revoking that key on
the server would refuse it, and count towards giving up.

The player on local slot 0 is the signed-in account: its roster entries
(`GameStarted`, `PlayerJoined`) carry that account's `xuid`, while `name`
stays the in-game profile name. Other local slots and remote players have no
`xuid` until their accounts are known. Offline capture links no one.

Clanker bots ([bots.md](../game/bots.md) §6) are players like any other,
keyed by their game slot (`index`): roster, scores, kills, frames and the
final standings carry them under their own names ("Clanker 1"). Their
roster entries add `bot: true`. On multiplayer start, the boot's bot
locator identifies phantoms by machine id. A phantom from the active
server roster carries its rostered `xuid`, and the game carries the
roster's `session_id`. A host-made phantom has no Live account, stats
page, career or rank.

For a streamed Clanker Training run, the frame sender reads completed
brain diagnostics every five wall seconds and sends one `sim_combat`
message per bot on the authenticated ingest socket. Discovery advertises
this optional message through `sim_combat`. Each sample carries the
bot's decision tick, current goal, weapon and target range, last action,
combat counters and weapon accuracy. The server keeps the latest sample
per player for the private run detail. Deferred runs deliver these
figures in the final report. [Clanker Training](live-api-theater-stats.md#clanker-training)

## Scenario objects

`Frame.objects` lists the scenario's placed objects that are not where the
scenario put them: moved (with their pose) or gone. An object is matched to
its placement by kind and index in that kind's scenario block, as map.glb's
`scenario` node names them (docs/live/live-art.md). Everything not listed is at
its placement, so a frame with no `objects` means nothing has moved.

| kind | block | stride | what |
|---|---|---:|---|
| `bipd` | `scnr+0x60` | `0x54` | bipeds |
| `vehi` | `scnr+0x70` | `0x54` | vehicles |
| `weap` | `scnr+0x90` | `0x54` | weapons lying in the level |
| `eqip` | `scnr+0x80` | `0x38` | equipment |
| `scen` | `scnr+0x50` | `0x5c` | scenery |
| `ctrl` | `scnr+0xb8` | `0x44` | controls (switches, panels) |
| `lifi` | `scnr+0xc8` | `0x54` | light fixtures |
| `bloc` | `scnr+0x328` | `0x4c` | crates and physics props |
| `crea` | `scnr+0x368` | `0x34` | creatures |
| `netgame` | `scnr+0x120` | `0x90` | multiplayer weapon and vehicle spawns (netgame equipment) |

Machines are left out (they travel in `Frame.machines`), and so is sound
scenery. A placement record holds its position at `+0x8` and yaw, pitch,
roll (radians) at `+0x14`; forward and up are `Rz(yaw)·Ry(-pitch)·Rx(roll)`
applied to +x and +z, as map.glb's placements are built. A netgame
equipment entry holds its position at `+0x40` and orientation at `+0x4c`.
The scenario is `*(0x004e0350)`; the blocks are read once a game, when the
map has loaded. A block that does not read is left empty and its objects
are not tracked.

Which object is which placement:

| field | offset | type | status | evidence |
|---|---|---|---|---|
| placement index | object `+0x1a` | `i16` | confirmed | `object_create` by name (`0x000bb670`) looks up the name's type (`+0x20`) and index (`+0x22`), and passes the index to `0x000bf0f0` with the record (block base `+0xa`, index times stride `+0xe`; `0x000bb71e..0x000bb74c`). `0x000bf0f0` hands it to the placement-data builder `0x000d5060` (call at `0x000bf152`), which stores it at placement data `+0x10` (`0x000d520b`, `0x000d522c`). `object_new` (`0x000b7b40`) copies placement data `+0x10` to object `+0x1a` (`0x000b7c87`); the placement data default is -1 (`0x000b7a31`); dynamic objects get -1 (`0x000b7c63`), and a scripted re-create sets -1 (`0x000bb6cc`) |
| netgame equipment index | object `+0xaf` | `i8` | confirmed | The netgame spawner `0x0015aad0` creates the object (`object_new` call at `0x0015abe6`) and writes the spawn's index at `0x0015ac15`; `object_new` sets `0xff` (`0x000b7db1`); deletion (`0x000b85c3`) calls `0x0015b220`, which clears the spawn's object. The spawner walks `scnr+0x120`, stride `0x90` (`0x0015aaf3`), and converts the orientation at `+0x4c` with `0x0011df60` |
| object type | header `+3` | `u8` | confirmed | The type table at `0x00468630`: bipd 0, vehi 1, weap 2, eqip 3, garb 4, proj 5, scen 6, mach 7, ctrl 8, lifi 9, ssce 10, bloc 11, crea 12 |

Each frame (15 a second, in the object-table scan the frame makes anyway)
every object with `+0x1a >= 0` counts for its type's block, and every
object with `+0xaf` not `0xff` for `netgame` too. Per placement:

- no object: **gone** (picked up, destroyed, never spawned);
- a weapon, equipment or netgame object with a parent (a player holds
  it): **gone**; another kind with a parent (a passenger seat, a turret) is
  left out: its transform is parent-relative;
- one with no parent wins over a held copy; if it is more than 0.02 world
  units from the placement, or either axis is turned more than 2 degrees,
  it is **moved**: `p`, `f`, `u` (unit length);
- otherwise nothing is sent.

At most 2048 entries, ordered by kind then index. A campaign mission with a
few dozen crates knocked about sends a few dozen entries, and the packed
stream sends only the ones that changed.

## Packed frames

Moved to [Live telemetry: packed frames](live-telemetry-packed.md#packed-frames).

### Version 8 live wire

### Bits and numbers (v1–v7)

### Values

### A frame

### Machine travel in v7

### Streams, keyframes and gaps

### Compatibility

### Finished recordings

## Telemetry cost

The cost of a frame is measured on the deterministic invented games in
`crates/live/live-proto/src/samples.rs`: bytes per frame and per hour for
JSON and each packed version, CPU per frame on the game's sink worker and
the server, and the game thread's scan. The numbers, the commands that
reproduce them and `scripts/telemetry-bench.sh` are in
[the telemetry cost note](notes/2026-11-19-telemetry-cost.md). Recorded
frames are not compressed at rest.

## Runtime validation needed first

Unit tests use synthetic guest memory and do not execute the game. Before
relying on live output, check in this order:

1. Successful start/end and tick continuation behaviour in a local Slayer game,
   including original return values and no lost guest calls.
2. Player name decoding, roster changes, and statborg values against the
   in-game scoreboard; verify kill attribution and score coalescing.
3. Vehicle-parent position/facing and the `vehicle` identifier.
4. Server discovery, game creation, token refresh and signed-batch
   acceptance against the live ingest implementation. Reconnect, the outbox
   and resends are tested against the embedded server through a proxy that
   cuts, refuses and mutes the socket (`crates/live/live-telemetry/src/server_tests.rs`);
   a real Worker deploy mid-game is still to watch.
5. Whether end-latch reasons other than `finished` can be distinguished; the
   current hook cannot reliably identify `quit` or `lost`.
6. The likely appearance path: local player slots at `0x0054e8e0` (profile
   copy at `+0x18`, appearance at `+0x118` inside it) joined by player
   `+0x28`, checked against the profile editor's colours and emblem; and
   where remote players' appearance lives in the `0x21c` player record.
7. Velocity, seat and vitality: `vel` against `speed` (a running Spartan
   a few units a second, as the static reading says, not 30 times that), riders' `vel` the vehicle's, `seat` 0 for a Warthog
   driver and the gunner's on the turret, `body`/`shield` falling with
   hits and recharging, an overshield above 1.
8. Frame models: `species` for both bipeds, `weapon` names for each
   multiplayer weapon (the active cache slot's tag names), `aim` following
   the reticle (and not lagging it: `+0x15c` is the other candidate),
   `shots` matching trigger pulls (battle rifle 3 per burst, shotgun 1 per
   pull, plasma weapons and the sentinel beam counted at all), and the fire
   hook installing (stderr says when it does not).
9. Campaign: one game per mission across deaths and reverts; actors'
   `+0x12c`/`+0x13c` split (allies, enemies, Flood, the Arbiter's levels);
   the team names (likely: tag-definition order); vehicle `up` for banshees;
   that dropship-mounted turrets (vehicles parented to vehicles) look right
   (their `+0x64` is parent-relative and is sent as is).
10. Stance and actions: crouch, airborne and the biped physics mode values
   (value names unverified); that a jump and a melee
   (hit and miss) each show once; that reloads, weapon swaps, grenade
   throws and pickups each show once and not when firing; the grenade type
   names (likely 0 frag, 1 plasma).
11. Hits: that `0x000d9640` runs once per damage instance on the host
    (its four callers include a network path, `0x0009c76e`); the reporting
    type order (unverified); body and shield amounts
    as vitality fractions (likely); a grenade's or vehicle's weapon falls
    back to the damage effect's stem.
12. Machines: in-game comparison of travel and viewer poses for a door,
    a lift and a looping device. Check a replay seek after a long idle
    period and a campaign checkpoint cut. A FakeGuest test covers the
    traced offsets and rejects inactive channels and out-of-range travel.
13. Scenario objects: crates knocked about in campaign show as moved in the
    3D viewer and settle where they stop; a weapon picked up off the floor
    and a multiplayer spawn's weapon after a pickup go gone and come back
    on respawn (`+0xaf`); scenery destroyed (for example glass or barrels)
    goes gone; that `+0x1a` indexes match map.glb's `scenario` children (a
    moved crate moves the right model).
13. Cinematics: that `[0x00510c50]+5` is set for the whole of each
    cutscene (`00a_introduction`, the opening and closing cinematics of a
    mission, the mid-level ones) and only then; that a level's first ticks,
    before its opening cinematic starts, send ordinary frames; that a
    skipped cutscene ends the cinematic frames; and that the player is
    where they should be in the first frame after one.
