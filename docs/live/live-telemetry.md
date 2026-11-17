# Live telemetry

`crates/live-telemetry` observes the retail game state described in
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
| `0x0015ce70` | Read attacker, victim, betrayal and victim position at entry, then invoke the original kill handler unchanged. Scores are reconciled at a later tick. |
| `0x001389c0` | After the original end-latch function, publish the final scoreboard and a unique high-score winner (or none for a tie). |
| `0x00104150` | Weapon barrel fire: read the weapon object datum at `[esp+4]`, count one shot for it, then invoke the original unchanged. Optional: if the entry is missing or already overridden, telemetry installs without it and frames carry no `shots`. |

The start, tick and end wrappers use shadow entries and runtime continuations to
observe after the original returns. If continuation setup fails, the wrapper
restores the guest stack and invokes the original directly. The kill wrapper
invokes the original immediately after its read.

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
`crates/live-telemetry/src/layout.rs::LIKELY_FIELDS`; they are not required by
the initial event contract. Confirmed offsets used by the readers share that
file so runtime validation has one correction point.

## Frame species, aim, weapon and shots

Each frame player carries what the web needs to draw a model
([live-api.md](live-api.md), frames; offsets and evidence in
[live-gamestate.md](live-gamestate.md), "Unit aim, held weapon, species and
shots"):

- `aim`: the unit's aiming vector (`+0x168`, rated likely), sent only when
  finite and within 10% of unit length, then normalised. A bad offset sends
  nothing.
- `weapon`: the current weapon index (`+0x212`) picks one of the four
  weapon datums (`+0x218`); the weapon must be a live weapon object
  (salt-checked, type 2) whose parent is this unit. Its definition datum is
  named by the map's tag table and sent as the file stem.
- `species`: the unit's own definition datum (the biped's, also while in a
  vehicle) named the same way: `masterchief` paths are `spartan`, `elite`
  and `dervish` paths `elite`, any other biped its directory name.
- `shots`: the weapon-fire hook counts calls by weapon datum; each tick's
  counts go to the players holding those weapons and add up until their
  next frame. A weapon change or death drops the pending count. Shots of a
  tick the worker folds into another (see "Threads") are added to the one
  it keeps.

Frames also carry `vehicles` and `actors` ([live-gamestate.md](live-gamestate.md),
"AI bipeds, teams and vehicles"):

- Actors: on frame ticks the object header table is read once (12 bytes a
  slot up to the high-water mark, at most 2,048) and every biped with an
  actor datum (`+0x12c`), no player (`+0x13c`) and no body-depleted flag
  is read like a player's unit (position, or its vehicle's; aim; held
  weapon). Nearest to any player first, at most 128. A fault on one biped
  skips it; a failed scan sends no actors.
- Bodies: the same scan keeps every biped with the body-depleted flag (the
  game leaves a body in the world until it clears it away). A body that
  was never a player's unit (an AI's, or one the level starts with) is
  sent after the living actors as an actor with stance `dead` (no aim,
  weapon or actions), nearest first, at most 32 and within the 128; it
  keeps the id it had alive. A dead player's `pos` and `yaw` are their
  body's while it lasts, then where they fell (the origin before this).
- `team` is the unit's `+0x138`; in campaign it is named (likely names),
  elsewhere sent as the number.
- `species` is the biped tag's directory; vehicles' `kind` the `vehi`
  stem; both `unknown` without the map's names.
- Actors' shots: every weapon's fire-hook count since the last frame,
  given to the actor holding that weapon at frame time.
- Vehicles: every vehicle the scan finds (object type 1), occupied when a
  biped's parent is it, occupied first, at most 64; a vehicle a player or
  sent actor rides uses the transform the rider read. A vehicle whose
  vectors are not finite and non-zero is left out. A vehicle parented to
  another object (a turret on the scarab or a dropship) is sent with its
  parent-relative `+0x64`. A turret (a vehicle whose parent is a vehicle:
  the Warthog's chaingun, the Scorpion's cannon, the Wraith's mortar) also
  sends `parent`, that vehicle's id, and every vehicle its own `aim`, the
  unit aiming vector at `+0x168` (vehicles are units; dropped unless
  finite and near unit length): where a turret points. A gunner's
  `vehicle` is the turret; their position and the turret's as a rider
  reads it are the vehicle the turret is mounted on.

And, from the same scan and the units' own memory
([live-gamestate.md](live-gamestate.md), "Unit stance, actions, damage and
machines"):

- `stance`: `dead` without a live unit, `vehicle` in one, `airborne` when
  the biped physics mode is 2, else `crouch` when the crouch amount
  (`+0x2c4`) is at least 0.5, else `stand`.
- `speed`: horizontal distance between this frame's and the previous
  frame's position over the game time between them; not sent for the first
  frame of a unit, over gaps of more than a second, or above 100 units a
  second (respawns, teleports).
- `actions`, from what changed since the unit's previous frame (none for a
  unit seen the first time): a weapon datum new to its four slots is
  `pickup`; the held weapon becoming one it already carried is `switch`; the
  held weapon's first magazine gaining loaded rounds while its reserve
  drops is `reload`; fewer grenades is `grenade`, more is `pickup`; the
  physics mode going from ground (1) to flying (2) with the velocity's z
  above 0 is `jump`; entering the melee mode (6), or dealing melee damage,
  is `melee`.
- `grenade`: the selected grenade type (`+0x23c`).
- `hits`: a hook on the damage report function `0x000d9640` (custom ABI,
  `EDI` = report, the victim datum on the stack; always runs the original)
  records each report; frames send those since the last frame, newest 64.
  The damage reporting type (the report code's low 6 bits) gives `kind` and,
  for hand weapons, `weapon`; otherwise `weapon` is the `jpt!` tag's stem.
  The attacker is the report's owner player (`p<index>`), else its owner
  object. The hook is optional: without that entry frames carry no hits.
- `bsp`: the game globals' active structure BSP (`G+0x02`).
- `machines`: every machine (object type 7) whose position, forward, up or
  device position changed by 0.001 or more since it was last sent; all of
  them in the game's first frame. `node` is the `mach` stem and the
  object's scenario placement index (`+0x1a`), matching map.glb's
  `machines` children, which are named after the scenario's machine block
  index.

Game starts say their `kind`: `campaign` for campaign mode, with
`difficulty` from the active options' `+0x12a` word (0 easy to 3
legendary; anything else leaves it out); multiplayer games are `custom`
unless the host has called `Telemetry::set_playlist`, which makes them
`matchmaking` with that playlist name and id. Nothing calls it yet:
live-hle has no matchmaking (its sessions are all advertised `custom` with
no hopper, `crates/live-hle/src/sessions.rs`), so the playlist source is
the HLE's search/match state once phase 2 of
[live-matchmaking.md](live-matchmaking.md) lands. `online` is never sent.

Games also say their `skulls` (docs/game/skulls.md): ascending indices, left
out when none are on (and by every sender before the field, which the
server reads as none). A campaign game reads the skull array at `0x4f55dc`
when the map activates, after our campaign start wrote the profiles'
choice into it; a checkpoint revert continues the game and does not read it
again. A multiplayer game cannot use the array: every reader of it tests
for a campaign, and it still holds whatever the last campaign turned on.
Its skulls are the set the boot applied at that game's start
(`Telemetry::set_multiplayer_skulls`, from `boot::skulls_mp`): the host's
choice, or on a guest the host's advertised one; only 4 (Famine) and 6
(Blind) are ever named. Indices run to 63, so skulls of our own past the
retail fifteen fit. The server keeps the field in `GameStarted`, returns it
in game summaries, details and recent games, and filters by it
(`GET /v1/games?skulls=4,6`, [live-api.md](live-api.md)).

Campaign: the map activation `0x0018f000` also runs on every checkpoint
revert and mission restart (`0x0018e700` re-activates with a copy of the
active options). A re-activation of the campaign map the current game is on
continues that game instead of ending it and starting another (which also
spent one `POST /v1/games` per revert against the server's write rate
limit, and a refused creation stops the sink). A different mission, a
multiplayer map or the main menu ends it as before.

Tag names are (likely) not in guest memory: nothing in the XBE refers to
the cache header's tag-name fields, and the loader reads the tag index and
metadata. On map activation telemetry asks the boot for the
map's file (`\Device\CdRom0\maps\<short name>.map` through the VFS),
reads its tag names with `crates/map`, and keeps only `bipd` and `weap`
rows. The table is used only if the file's scenario datum equals the
guest's (tag index header `[0x00547f00]+0x0c`), since both indexes hold the
same salted datums. Without a match, or without a locator (tests, a raw
XBE), `species` and `weapon` are left out.

## Player appearance

Each player's emblem and colours are read from the signed-in profile of
their local player slot ([live-gamestate.md](live-gamestate.md), "Local
player slots and profile appearance"): palette indices for the four profile
colours and the two emblem layers. Those offsets are rated likely, so the
reader range-checks every value (colours 0..17, foreground 0..63,
background 0..31) and omits what fails; an unvalidated offset cannot put
garbage in events. Only the four local slots hold profile copies, so remote
players' appearance is absent until the in-record location is validated.
The game's rank is an Xbox Live attribute, not guest memory, and is never
sent.

Appearance rides the roster events only — never frames or scores — and is
never re-sent unchanged:

- `GameStarted` `players[i]` and `PlayerJoined` `player` gain an
  `appearance` object, omitted when nothing is known:

```json
{ "index": 0, "name": "Sneaky Pete", "team": 1, "local": true,
  "appearance": { "primary": 5, "secondary": 9, "tertiary": 2,
                  "quaternary": 11, "emblem_foreground": 30,
                  "emblem_background": 7 } }
```

- `primary`..`quaternary` are indices into the art manifest's 18-colour
  `palette`; `emblem_foreground`/`emblem_background` index
  `emblem/fg/<0..63>` and `emblem/bg/<0..31>`. Any field may be missing on
  its own.
- A mid-game appearance change to a player already in the roster cannot be
  announced with the current event types (the server rejects a
  `PlayerJoined` for a known player); a `PlayerUpdated`-style type would be
  needed if that ever matters.

The server's live-proto validation ignores the extra `appearance` keys
until its `Player` type gains the field.

## Threads

The game thread only reads: on each changed tick it fills a snapshot (the
roster, team scores, the players' objects, and on frame ticks the world
scan's actors, vehicles, machines, placed objects and bodies, all plain
values) and queues it with the shots and hits the hooks counted. A start,
kill or end is read and queued the same way, and the playlist and skull
settings are queued in order with them. One worker thread
(`crates/live-telemetry/src/pipeline.rs`) runs the session over the queue
in order: it builds the events and frames (ids, species, weapon, team and
stance names, actor order, placements), loads the map's tag names at a
match start, and hands everything to the sink. It sleeps until woken:
everything queued wakes it except a tick between frames (odd), which waits
for the next frame tick or anything else, at most 100 ms (a wake costs the
game thread a system call).

The worker hands each snapshot back cleared and the game thread fills it
again, so a steady game allocates nothing for it: the lists keep their
room, and the roster keeps its names for the next read while the players'
datums and names are unchanged.

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
`games.write` token, refreshes it before expiry, sends frames unsigned as the
contract allows, and batches up to four game events into one JWS signed with
the account key through `live-proto`. Empty batches and messages over 16 KiB
are rejected. The account key is never included in debug output.

Frames go as [packed frames](#packed-frames): one record per binary
message, a stream per connection (a reconnect starts a new one), a keyframe
every 150 frames (ten seconds) and deltas between. A frame the format
cannot carry (it never should: the same checks the server makes) goes as
the JSON `frame` message instead and the stream carries on. The offline
JSONL capture stays JSON.

The player on local slot 0 is the signed-in account: its roster entries
(`GameStarted`, `PlayerJoined`) carry that account's `xuid`, while `name`
stays the in-game profile name. Other local slots and remote players have no
`xuid` until their accounts are known. Offline capture links no one.

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

Frames travel bit-packed: game to server (`/v1/ingest`, binary messages),
server to readers (`/v1/stream?frames=packed`), recorded frames at rest
(`GameFrame.data.packed`) and the replay fetch
(`/v1/games/{id}/frames?format=packed`). JSON stays for the public read
API (`?format=json`, the default, decoded on the server) and for events.
`crates/live-proto/src/pack.rs` writes and reads it;
`web/src/api/pack.ts` reads it; the golden vectors in
`web/test/fixtures/pack` (written by the Rust tests from invented frames,
read by the TypeScript tests) keep the two in step.

### Bits and numbers

Bits are written least significant first into bytes, first byte first.

| code | bits |
|---|---|
| `u(n)` | `n` bits, unsigned |
| `eg(k)` | Exp-Golomb of order `k`: for `v`, let `x = v + 2^k` have `z + k + 1` bits; `z` zeros, a one, then the low `z + k` bits of `x` (least significant first). `v` below `2^44` |
| `seg(k)` | signed: zig-zag (`0, -1, 1, -2, ...` to `0, 1, 2, 3, ...`) then `eg(k)` |
| `str` | literal: `eg(3)` byte length (at most 255), then each byte `u(8)`; UTF-8 |

A container holds several records: each a LEB128 length then that many
bytes. Binary socket messages and the `application/x-h2-frames` replay
body are containers; `GameFrame.data.packed` is one record, base64url
without padding.

### Values

| value | wire | quantisation |
|---|---|---|
| position | three `seg(16)` | 1/512 world unit (about 6 mm), coordinates under 10^9 |
| position in a delta | three `seg(7)`: the difference from the previous frame's quantised value | |
| yaw | `u(12)` | a turn in 4096 steps; read back in (-π, π] |
| direction (aim, forward, up) | two `u(12)` | octahedral: x and y of `v / (|x|+|y|+|z|)`, folded when z < 0, each mapped from -1..1 to 0..4094 (2047 is 0, so the axes are exact); read back unit length |
| speed | `eg(6)` | 1/16 unit a second |
| hit amount | `eg(6)` | 1/256 of a vitality |
| shots | `eg(1)` | |
| stance | `u(3)` | `stand`, `crouch`, `airborne`, `vehicle`, `dead` |
| actions | `eg(0)` count (at most 8), then `u(3)` each | `grenade`, `melee`, `reload`, `jump`, `switch`, `pickup` |
| grenade | `u(1)` | `frag`, `plasma` |
| device position | `u(10)` | 0..1 in 1023 steps |
| string | `eg(3)`: index into the stream's string table | |
| id | `u(2)` form: 0 datum, `u(32)` (exactly 8 lowercase hex digits in JSON); 1 player `p<n>`, `eg(2)`; 2 anything else, a string index | |
| object kind | `u(4)` | the order of the kind table above, with `bipd` 0, `vehi` 1, `weap` 2, `eqip` 3, `scen` 4, `mach` 5, `ctrl` 6, `lifi` 7, `ssce` 8, `bloc` 9, `crea` 10, `netgame` 11 |
| object index | `eg(6)` | at most `0x7fff` |

An optional field (marked `opt` below) is a presence bit, then the value
when present.

### A frame

| field | wire |
|---|---|
| version | `u(4)`, 2 (1 still reads: vehicles without parent and aim) |
| keyframe | `u(1)` |
| stream id | `eg(0)` |
| sequence | `eg(8)`, one more each frame of the stream |
| **keyframe:** game id | `str` literal |
| tick | `eg(20)` |
| bsp | `u(1)` present, then `eg(1)` |
| **delta:** tick | `eg(2)`, the difference from the previous frame (never negative) |
| bsp | `u(1)` changed; if so `u(1)` present, then `eg(1)` |
| new strings | `eg(1)` count (at most 1024), then each a `str` literal, appended to the table (a keyframe starts an empty one) |
| players | list |
| vehicles | list |
| actors | list |
| hits | always whole: `eg(2)` count, then each entity |
| machines | list |
| objects | list |

A list in a keyframe is `eg(2)` count, then each entity whole: its fields in
order. In a delta it is `eg(2)` count, then, until that many entities:

- `0`, `eg(2)` run-1: the next run of entities are the previous frame's,
  unchanged, taken in order from the cursor (the cursor moves past them);
- `1`, `1`, `seg(0)` index of the previous entity relative to the cursor,
  then one bit per non-key field (changed), then each changed field (a
  position as a difference); the cursor moves past that entity;
- `1`, `0`: a new entity, whole.

Entities are matched by their key fields (below). A field "changed" is a
change in its quantised value, so jitter below the quantum costs nothing.

| list | fields (key in bold) |
|---|---|
| player | **index `eg(2)`**, alive `u(1)`, pos, yaw, opt vehicle id, opt species string, opt aim, opt weapon string, opt shots, opt stance, opt speed, opt actions, opt grenade |
| vehicle | **id**, kind string, pos, forward, up, occupied `u(1)`, opt parent id, opt aim (version 2; version 1 ends at occupied) |
| actor | **id**, species string, pos, yaw, opt aim, opt weapon string, opt shots, opt team string, opt vehicle id, opt stance, opt speed, opt actions, opt grenade |
| hit | opt from id, opt to id, opt weapon string, pos, opt amount, opt kind string |
| machine | opt node string, **id**, kind string, pos, forward, up, opt device position |
| object | **kind**, **index**, gone `u(1)`, opt pos, opt forward, opt up |

### Streams, keyframes and gaps

An encoder starts with a keyframe and sends one again every N frames (the
game: 150; the server's reader stream: 15), when the game id changes, when
the tick goes backwards, and when the string table would pass 4096 strings.
A decoder keeps the last frame and string table per stream id; a delta
whose sequence is not the next one it expects (a dropped frame, or joining
mid-stream) is a gap: it drops deltas until the next keyframe. The reader
stream sends a joining or lagging reader every frame since the last
keyframe first, so it starts decoding at once. A frame that cannot be
packed (non-finite numbers, unknown names, too many strings) leaves the
encoder as it was.

### Compatibility

The version is the first four bits; a decoder refuses versions it does not
know. Version 2 (vehicles' parent and aim) is written now; decoders still
read version 1 (recorded frames, an older game's stream), and a delta only
applies to a frame of its own version. `web/test/fixtures/pack/v1-multiplayer`
is a version 1 stream both decoders are tested on. Recorded `GameFrame` events written before packed frames hold the
JSON frame (no `packed` field) and stay readable: the server's
`?format=json` returns them as they are, `?format=packed` packs them on
the fly, and the web's `recordedFrame` reads both. The ingest socket takes
the JSON `frame` message as before, and `/v1/stream` sends JSON frames
unless asked for `?frames=packed`. A recorded frame is signed by the
server's event signature over `{tick, source, packed}`, so the signature
covers the packed bytes.

## Telemetry cost

Measured on the deterministic invented games in
`crates/live-proto/src/samples.rs` (a 16-player Slayer: 16 players, 4
vehicles, hits, weapon spawns; a campaign scene: 1 player, 60 actors, 8
vehicles, 12 machines, 25 crates moved, 16 objects gone), 450 frames each,
release build on the development Mac. Run
`cargo test --release -p live-proto frame_sizes -- --nocapture` and
`cargo test --release -p live-proto measure -- --ignored --nocapture`, and
`cargo test --release -p live-telemetry measure_frame_cost -- --ignored --nocapture`.

| per frame | JSON (before) | packed | packed + deflate |
|---|---:|---:|---:|
| 16-player MP, live frame | 6026 B | 219 B (keyframe every 15) | |
| 16-player MP, keyframe / recorded | | 511 B | 514 B |
| campaign 60 actors, live frame | 24678 B | 650 B | |
| campaign, keyframe / recorded | | 2098 B | 2089 B |
| MP ingest at 15 Hz | 90 KB/s | 3.3 KB/s | |
| campaign ingest at 15 Hz | 370 KB/s | 9.8 KB/s | |
| MP stored `GameFrame` (signed event, 1 Hz) | 8629 B | 1498 B | |
| campaign stored `GameFrame` | 33105 B | 4332 B | |
| MP storage per hour | 31 MB | 5.4 MB | |
| campaign storage per hour | 119 MB | 15.6 MB | |

General-purpose compression of a packed record gains nothing (deflate:
+1% to -0.5%; the bits are already near their entropy), so recorded frames
are not compressed at rest and zstd was left out. The stored event is
bigger than its keyframe because of the JWS around it (base64 of the
payload and the signature).

| CPU per frame | JSON (before) | packed |
|---|---:|---:|
| MP, game's sink worker (serialise / pack) | 12.2 µs | 7.7 µs |
| MP, server (parse or decode, validate, pass on) | 40.3 µs | 17.8 µs |
| campaign, game's sink worker | 59.0 µs | 38.7 µs |
| campaign, server | 206 µs | 146 µs |

Packed frames are encoded once per frame for every reader, and since
2026-11-17 so is a JSON reader's `frame` message (`LiveFrame::json_message`):
before, each JSON reader cloned and serialised the frame itself, about
160 µs a frame a reader for a 47 KB campaign-sized frame.

On the game thread, for a campaign scene of 60 actors, 8 vehicles, 12
machines and 90 placed objects (synthetic guest memory): the object-table
scan takes 38 µs without the placement reads and 54 µs with them, the frame
build 14 µs. Since the third telemetry-perf lane the frame build is on the
worker too (see "Threads"), with serialising.
At 15 frames a second that is under 0.1% of a core.

### Benchmarks

`scripts/telemetry-bench.sh [out dir]` runs every stage and writes its
JSON to the directory (default `$TMPDIR/telemetry-bench`). No game data and
no game run are needed. Each stage runs alone as well:

- Game thread, sink worker and wire:
  `cargo test --release -p live-telemetry --lib bench -- --ignored --nocapture --test-threads=1`
  (`crates/live-telemetry/src/bench.rs`, `guest.json`). Three synthetic
  scenes in fixture guest memory: `mp16` (16 players, 2 weapons each, 12
  vehicles and 4 turrets, 5 riders, 200 placed scenery and 24 placed
  weapons, 296 objects), `mp16_1000` (the same at 984 objects) and
  `campaign100` (1 player, 100 armed actors with 6 riding, 30 bodies, 16
  vehicles and 6 turrets, 30 machines, 600 scenery, 930 objects). Each
  scene runs a minute of 30 Hz ticks through the hooks' own entry points
  (weapon fire and damage hooks, then the tick's observation and publish),
  timed and allocation-counted on the calling thread; the best of five
  runs by median frame tick is kept. After each tick, untimed, the bench
  waits for the worker to finish it (real ticks are 33 ms apart), and
  records the worker's time apart (`worker`). Then the frames are packed as
  the server sink packs them.
- Server: `cargo bench -p live-server --bench telemetry`
  (`crates/live-server/benches/telemetry.rs`, `server.json`): packed
  ingest at 15 Hz a game, signed event batches of 1 and 32, and stream
  fan-out to 1, 10 and 100 loopback readers (packed and JSON).
- Web: `cd web && bun run bench:telemetry` (`web/bench/telemetry.bench.ts`,
  `web.json`): packed decode, the frame hub and a repaint's sampling.

`scenes_produce_the_golden_output` (not ignored) hashes every frame of the
first twelve ticks of each scene, so a speed-up cannot change what is sent.

Game thread, median µs per tick and allocations per tick, on a loaded
development Mac (load average 40 to 80, so absolute numbers vary by 10 to
20%), before and after the 2026-11-17 telemetry-perf lane:

| scene | frame tick before | after | allocs before | after | other tick before | after | µs per second at 30 Hz before | after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| mp16 | 60.4 | 27.7 | 332 | 284 | 16.4 | 10.2 | 1151 | 567 |
| mp16_1000 | 135.5 | 53.5 | 397 | 344 | 16.0 | 10.4 | 2272 | 958 |
| campaign100 | 213.0 | 103.5 | 922 | 891 | 2.1 | 1.6 | 3227 | 1576 |

What changed: guest scalar reads inline their fast-page case
(`runtime::Guest::read_bytes`), a vector is one 12-byte read, the object
scan reuses its lists between frame ticks (it allocated and freed about
200 KiB a frame), tag names hold their frame ids instead of parsing the
tag path per entity, and frame objects match placements in one pass. The
remaining allocations are the frame itself (an id, species, weapon and
team string per entity).

Wire on that lane's fixture, packed with a keyframe every 150 frames: `mp16` 316 B a frame (278
KiB a minute; JSON would be 7.4 MiB), `mp16_1000` 397 B (349 KiB),
`campaign100` 1251 B (1.1 MiB a minute; JSON 31 MiB). The worker packs a
frame in 13, 20 and 65 µs.

The second lane (same day) moved the scenes' fixture off `sin`/`cos`
(release builds fuse them into `sincos`, a ulp apart from debug, so the
golden hash differed between profiles), which shifts every number a
little; its before is measured on the new fixture, at a load average of
10 to 25:

| scene | frame tick before | after | allocs before | after | other tick before | after | allocs before | after | µs per second at 30 Hz before | after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| mp16 | 26.3 | 22.0 | 284 | 219 | 9.1 | 5.7 | 85 | 20 | 531 | 416 |
| mp16_1000 | 51.7 | 46.0 | 344 | 279 | 9.5 | 5.9 | 85 | 20 | 918 | 779 |
| campaign100 | 92.2 | 77.7 | 891 | 883 | 1.5 | 1.2 | 21 | 13 | 1406 | 1184 |

What changed: object lookups read the objects array's header once per
observation, not once per datum; the session compares the roster in place
and copies it only when it changed; the roster read keeps each name from
the last tick while the player's datum and name units are unchanged; frame
ids are written without the formatter and actors lend their object state
instead of copying it. The remaining allocations are the frame itself (an
id, species, weapon, stance and team string per entity).

Wire, packed with a keyframe every 150 frames: `mp16` 325 B a frame (286
KiB a minute; JSON would be 7.4 MiB), `mp16_1000` 407 B (357 KiB),
`campaign100` 1330 B (1.2 MiB a minute; JSON 30 MiB). The worker packs a
frame in 13, 20 and 63 µs.

The third lane moved the frame build to a worker thread (see "Threads"),
so the game thread's tick is the guest reads and a queue push. Same
fixture; before is master at bbbbfe40, both at a load average
of 60 to 120 (p99 is mostly preemption at that load: master's own swings
from 0.1 to 4 ms between runs):

| scene | frame tick before | after | allocs before | after | other tick before | after | allocs before | after | µs per second at 30 Hz before | after | worker per frame |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| mp16 | 22.8 | 10.6 | 219 | 4 | 5.9 | 4.9 | 20 | 4 | 431 | 233 | 14.7 |
| mp16_1000 | 47.3 | 21.2 | 279 | 4 | 6.1 | 4.7 | 20 | 4 | 801 | 389 | 22.2 |
| campaign100 | 78.4 | 25.6 | 883 | 4 | 1.2 | 1.0 | 13 | 4 | 1194 | 399 | 54.8 |

Also in that lane: a body's position, forward and up are one guest read
and a placed object's parent and placement index another; placed kinds
come from a table; and the frame's placed-object test skips normalising
an object that is in place beyond doubt (a squared-cosine test with a
1e-9 margin; near the edge the exact test decides, so frames are
unchanged). The world scan, timed into kept lists: 4.6, 15.5 and 22.4 µs.
What is left on the game thread is reading the guest: the snapshot (about
5 µs for 16 players) and the scan, which in the fixture is mostly the
first touch of each object's body (the fixture spaces bodies 4 KiB apart;
the game packs them).

## Runtime validation needed first

Unit tests use synthetic guest memory and do not execute the game. Before
relying on live output, check in this order:

1. Successful start/end and tick continuation behavior in a local Slayer game,
   including original return values and no lost guest calls.
2. Player name decoding, roster changes, and statborg values against the
   in-game scoreboard; verify kill attribution and score coalescing.
3. Vehicle-parent position/facing and the `vehicle` identifier.
4. Server discovery, game creation, token refresh, signed-batch acceptance,
   and reconnect behavior against the live ingest implementation.
5. Whether end-latch reasons other than `finished` can be distinguished; the
   current hook cannot reliably identify `quit` or `lost`.
6. The likely appearance path: local player slots at `0x0054e8e0` (profile
   copy at `+0x18`, appearance at `+0x118` inside it) joined by player
   `+0x28`, checked against the profile editor's colours and emblem; and
   where remote players' appearance lives in the `0x21c` player record.
7. Frame models: `species` for both bipeds, `weapon` names for each
   multiplayer weapon (the map-file lookup by short name), `aim` following
   the reticle (and not lagging it: `+0x15c` is the other candidate),
   `shots` matching trigger pulls (battle rifle 3 per burst, shotgun 1 per
   pull, plasma weapons and the sentinel beam counted at all), and the fire
   hook installing (stderr says when it does not).
8. Campaign: one game per mission across deaths and reverts; actors'
   `+0x12c`/`+0x13c` split (allies, enemies, Flood, the Arbiter's levels);
   the team names (likely: tag-definition order); vehicle `up` for banshees;
   that dropship-mounted turrets (vehicles parented to vehicles) look right
   (their `+0x64` is parent-relative and is sent as is).
9. Stance and actions: crouch, airborne and the biped physics mode values
   (likely, from Project Cartographer's names); that a jump and a melee
   (hit and miss) each show once; that reloads, weapon swaps, grenade
   throws and pickups each show once and not when firing; the grenade type
   names (likely 0 frag, 1 plasma).
10. Hits: that `0x000d9640` runs once per damage instance on the host
    (its four callers include a network path, `0x0009c76e`); the reporting
    type order (likely, from Project Cartographer); body and shield amounts
    as vitality fractions (likely); a grenade's or vehicle's weapon falls
    back to the damage effect's stem.
11. Machines: object type 7 is a machine and the scarab is one (its `mach`
    tag is in `03a`/`03b`); `+0x1a` matching map.glb's machine names; the
    device position `+0x140` moving 0 to 1 as doors open and elevators
    travel (likely); the active BSP switching between campaign zones.
12. Scenario objects: crates knocked about in campaign show as moved in the
    3D viewer and settle where they stop; a weapon picked up off the floor
    and a multiplayer spawn's weapon after a pickup go gone and come back
    on respawn (`+0xaf`); scenery destroyed (for example glass or barrels)
    goes gone; that `+0x1a` indexes match map.glb's `scenario` children (a
    moved crate moves the right model).
