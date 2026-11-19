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
| `0x0015ce70` | Read attacker, victim and victim position at entry, then invoke the original kill handler unchanged. The kind is told from the players: a betrayal is a teammate killed in a team game or campaign (`live_proto::game::is_betrayal`); the third argument is not a betrayal flag. Scores are reconciled at a later tick. |
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
  it keeps. The ticks they were on go out as `fire` (see "Velocity, seat,
  vitality and fire").

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

Version 6 adds, from the same scan ([live-gamestate.md](live-gamestate.md),
object types 3 and 5):

- Vehicles' `vel`: body `+0x88`, as units' (a ridden vehicle's is the one
  its rider read: the vehicle's, or a gunner's mount's). Absent past
  `MAX_FRAME_SPEED` or not finite.
- `projectiles`: every projectile object (type 5) slower than
  `MAX_PROJECTILE_SPEED` (100 units a second), or stuck to something:
  grenades, rockets, plasma bolts, needles. Bullets (400 a second and
  more) live a tick or two; the viewer draws them from the shooter's `fire`
  and `aim`, so they are left out. Each has its datum `id`, its `proj` tag
  stem as `kind`, `pos` and `vel` (`+0x64`, `+0x88`, what the projectile
  update moves). A stuck one (a plasma grenade on a unit or a vehicle) has
  a parent and a parent-relative transform, so it is sent with `parent`
  (that object's id), its parent's position and no `vel`. At most 64; past
  that the nearest to a player.
- `items`: every weapon (type 2) or equipment (type 3) object with no
  parent that no scenario placement or netgame spawn made: dropped guns, a
  dropped flag, ball or bomb, powerups a game type creates. Placed and
  spawned ones stay in `objects` ([Scenario objects](#scenario-objects)).
  `kind` is the `weap` stem, or the `eqip` stem with `equipment`; `pos`,
  `forward`, `up`. At most 128; past that the nearest to a player. They
  are listed in object table order, so a delta matches each in place, and
  one lying still costs nothing after its first frame. The loose test
  reuses the placement read the scan makes anyway (parent, `+0x1a`,
  `+0xaf`), so only loose ones cost a pose read.

A carried flag, ball or bomb is its carrier's `weapon` (`flag`, `ball`,
`assault_bomb`), so carrier and drop are both covered; the objective's
game state (score, which team's flag) is not read yet.

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
- `machines`: machine objects (type 7) whose world position, forward or
  up changed by 0.001 or more since last sent. The first sample includes
  the pose. `node` is the `mach` stem and scenario placement index, which
  identify a placed machine in map.glb. The viewer applies the pose to
  the whole machine, including free movers.
- `machine_positions`: placed machines with a valid position animation
  channel. The object body holds the scenario index at `+0x1a`, normalized
  travel at `+0x140`, and the channel at `+0x18c` (see
  [live-gamestate.md](live-gamestate.md#unit-stance-actions-damage-and-machines)).
  The sender rounds travel to 0..255 and sends each index once at first
  sight, then on changes to that byte. This works independently of the
  object's root pose and tag name. The viewer samples the device animation
  with those values. Earlier recordings keep their world-pose playback.

For a retail-based mod, the game start reports the retail base in
`map` and the installed package in `map_id`. The cache header's short alias
resolves to that package. The join uses the content ID and the report can
group with games on the same base. [Map packages](../game/maps.md#package-offers)

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
multiplayer map or the main menu ends it as before. The revert winds the
engine's clock back; the game's does not: from then on every tick the game
sends (frames, kills, scores, its end) is the engine's plus an even offset
that puts the first one past the last tick sent. Before, the reverted run's
ticks overlapped the first's, a replay sorted them together and the player
flicked between the two runs.

## Cinematics

In a campaign cutscene the scripts own the world: they teleport the
players, hide or freeze them, move AI bipeds and vehicles as puppets and
kill whom the story needs. Before, telemetry sent all of it as play, so a
replay or the live view showed players jumping about, frozen in place or
gone, and a scripted death counted. Now telemetry reads, every tick, the
byte the game itself asks whether a cinematic is playing: `[0x00510c50]+5`,
set by HaloScript's `cinematic_start` and cleared by `cinematic_stop`
([live-gamestate.md](live-gamestate.md), "Cinematics (cutscenes)"). It is
in the game state, so it is right across checkpoint reverts too.

While it is set:

- Frames keep coming on frame ticks, on the game's clock (reverts and all,
  as above), but each is only the tick, the BSP, a `cut` if one is due and
  `cinematic: true`: no players, actors, vehicles, hits, machines or
  objects. The game thread skips the world scan for them. A frame of
  nothing costs a few bytes, and keeps the live view and recorded chunks advancing through the cutscene.
- Kills are not sent (the kill hook reads the byte as the kill happens, and
  the worker also drops one queued in a cutscene); hits and shots counted
  in it are dropped, and actors' shots with them.
- Scores and the roster are read as ever: they are the game's own record.
- Each unit's last frame is forgotten, so the first frame after the
  cutscene works out no speed or actions across it (a `pickup` for the
  weapon the scripts handed over, a run at the speed of a teleport).
  Machines are sent from their last sent state, so the doors the cutscene
  moved are sent in that first frame.

The web viewer ([live-api.md](live-api.md) frames) draws a "Cutscene" card
over the map and the 3D view for a cinematic frame and never draws the
entities of the frames around it into it. A replay marks each cutscene on
the scrubber and, playing, skips from its start to the first frame after
it; seeking into one shows the card.

The packed format carries the mark as a header bit: version 6 has it on
every frame ([Packed frames](#packed-frames)); before it, version 5 was
written for cinematic frames alone, so a server or page from before it
still read every other frame.

Tag names are (likely) not in guest memory: nothing in the XBE refers to
the cache header's tag-name fields, and the loader reads the tag index and
metadata. On map activation telemetry asks the boot for the
map's file (`\Device\CdRom0\maps\<short name>.map` through the VFS),
reads its tag names with `crates/map`, and keeps only `bipd` and `weap`
rows. The table is used only if the file's scenario datum equals the
guest's (tag index header `[0x00547f00]+0x0c`), since both indexes hold the
same salted datums. Without a match, or without a locator (tests, a raw
XBE), `species` and `weapon` are left out.

## Velocity, seat, vitality and fire

Players and actors carry the protocol's version 3 unit fields from capture
(docs/live/replay-fidelity.md, step 4). Offsets and evidence are in
`crates/live-telemetry/src/layout.rs`; all are confirmed by static reading
of the retail XBE.

- `vel`: body `+0x88`, sent as read, in world units **a second**. The
  engine keeps the field per second, not per tick: the projectile update
  (from `0x000f8ee2`) loads the seconds per tick (`T+0x04`), takes gravity
  times it off the velocity's z and moves the position (`+0x64`) by the
  velocity times it (`0x000f97df`..`0x000f9803`), and projectile creation
  adds the tag's initial velocity (`proj +0x17c`, 400 for a bullet, world
  units a second) straight in (`0x000f8487`). It is an object field, so
  bipeds and vehicles hold the same unit. A rider sends the velocity of what
  its `pos` is: the vehicle's, or the mount's for a gunner. Absent when the
  read faults or is not finite, or faster than `MAX_FRAME_SPEED` (1000 a
  second); a standing unit sends zeros, which cost nothing in a delta once
  sent. A dead player sends none.
- `seat`: unit `+0x1fc` (`i16`, -1 none), the index in the vehicle
  definition's seats block. `unit_new` sets it to -1 (`0x000c44c1`), the
  enter-vehicle routine (`0x000ea1f0`) indexes the seats block with it and
  stores it (`0x000ea405`), leaving stores -1 (`0x000ea995`). Sent only
  with `vehicle`, at most 63.
- `body` and `shield`: `+0xec` and `+0xf0` (`f32`, 1.0 full), as
  `object_get_health` (`0x002a1a20`) and `object_get_shield`
  (`0x002a1a90`) read them: both 0 while the body-depleted damage flag is
  set. Sent clamped to 0..1 and 0..4 (an overshield charges past 1).
  Bodies (stance `dead`) carry none.
- `fire`: each tick's fire-hook count for the held weapon also sets bit 0
  of a 32-bit mask, after moving the mask older by the ticks since the
  last shot (bits past 32 ticks drop off). The frame sends the mask as of
  its tick, so bit `k` is a shot on `tick - k`, beside `shots`, the count;
  both reset at each frame. Actors' masks are kept by weapon datum, like
  their counts. A weapon change, death or campaign revert drops them.

`actions_ago` and hits' `ago` are not filled yet.

### Rate: 15 Hz, players included

Frames stay on even ticks (15 Hz) for everything, players included. A
players-only frame on odd ticks does not fit the current protocol: a
frame's lists are complete, so an odd frame without actors, vehicles and
machines says they are gone. The viewer would drop them for a tick, and the
packed delta of the next even frame would carry every actor again whole
(no previous entity to match), a keyframe's worth of bytes 15 times a
second. Sending everything every tick doubles the world scan's game-thread
cost (it runs on frame ticks only, `pipeline.rs`) and the bytes, while the
server forwards at most 15 frames a second (`FRAME_INTERVAL`, 66 ms,
`crates/live-server/src/games.rs`) and records the first frame of each
wall-clock second, so the extra frames would be dropped or folded before
anyone saw them. With `vel` and `fire` filled, the viewer can place each
shot on its tick and interpolate between 15 Hz samples with the velocity
(Hermite) at no extra rate.

What 30 Hz players would need: a frame flag (packed version 5, JSON
`partial: true`) meaning "these lists are unchanged since the last full
frame", so an odd frame carries players only and the delta encoder matches
against the last full frame's other lists; the session building one each
odd tick (the snapshot already reads players every tick); the server's
throttle letting players-only frames through (or the chunk recorder of
step 5 taking every frame); and the 15-delta keyframe interval and `seq`
counting the odd frames too. `tick_offset` staying even keeps full frames
on even ticks either way.

### Bandwidth (estimates, until the bench runs)

From the packed codes (`pack.rs`): a delta sends a changed `vel` as three
`seg(4)` differences in 1/64 unit a second. A unit walking a curve at about
4 units a second changes it by some 17 steps a component a frame, about
22 bits with the field's flag (about 2.75 bytes); a still or steadily
moving one costs nothing after the first frame. `body` and `shield` cost
about 2 bytes a unit in a keyframe and nothing until they change; `fire`
about half a byte on a frame its unit fired; `seat` a few bits on entering a
vehicle.

| scene (bench fixture) | before, B a frame | estimate with vel, fire, seat, vitality | per minute |
|---|---:|---:|---:|
| `mp16` (16 players circling) | 325 | about 375 | about 330 KiB (286 before) |
| `mp16_1000` | 407 | about 455 | about 400 KiB (357 before) |
| `campaign100` (all 100 actors circling) | 1330 | about 1610 | about 1.4 MiB (1.2 before) |

The fixture moves every unit on a curve, so these are an upper end; in a
real campaign most actors stand still and cost nothing extra. The bench
fixture now sets the units' velocity in units a second (it was a per-tick
guess) and full vitality. `cargo test --release -p live-telemetry --lib
bench -- --ignored --nocapture` measures them; replace these estimates with
its numbers.

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
the account key through `live-proto` (a `GameEnded` always goes in a batch of
its own). Empty batches and messages over 16 KiB are rejected. The account
key is never included in debug output.

Frames go as [packed frames](#packed-frames): one record per binary
message, a stream per connection (a [reconnect](#reconnect) starts a new one), a keyframe
every 150 frames (ten seconds) and deltas between. A frame the format
cannot carry (it never should: the same checks the server makes) goes as
the JSON `frame` message instead and the stream carries on. The offline
JSONL capture stays JSON.

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
  new connection is a new [packed](#packed-frames) stream, so its first frame
  is a keyframe, which is what the server's new decoder needs. A frame
  refused with `frame: ...` (the server lost its decoder, as after a Durable
  Object wakes from hibernation) also makes the next frame a keyframe.
- **Frames** go now or never: while there is no connection, or before the
  game is created, they are dropped.
- **The outbox.** Game starts and events are kept in order in a bounded
  outbox (4096) until the server has answered for them. A `GameStarted` is
  created with `POST /v1/games` when it reaches the front; the game's events
  wait behind it. Past the bound the oldest `PlayerKilled` or
  `ScoreChanged` is dropped; starts, ends, joins and leaves never are.
- **Answers.** A batch leaves the outbox only on its answer. Replies come in
  the order the server took the messages, a batch is always answered and a
  frame only when refused, so `appended` is the batch's, and a refusal is
  the batch's once half a second passes with nothing after it (refusals
  carry no id). No answer in 10 s counts as a lost connection.
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
- **Exit.** On shutdown the sink makes one last delivery on a live
  connection, without reconnecting; anything left is logged as not
  delivered. The outbox is in memory only.

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
roster entries add `bot: true`. The boot's bots locator (asked at each
multiplayer start) names the bots: a local-slot Clanker by its controller
port (player `+0x24`, read as `PlayerState::controller`), a phantom by its
machine id. Only a phantom from a server roster in force carries an `xuid`
(its rostered one, with the roster's `session_id` on the game); every
other bot has none, so no account, stats page, career or rank. A bot is
never given the signed-in account, even on local slot 0. A local bot added
after the game started is not known as a bot until the next start.

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

### Version 8 live wire

The game reads discovery when it opens or reconnects its ingest socket.
It selects v8 if the server advertises `ingest_frame_versions: [8]` and
falls back to v7 otherwise. Native and Worker servers both advertise v8.
The viewer requests `frames=packed&frame_version=8`. Subscribers that
omit the version keep receiving v7. Readers dispatch on each record's
version nibble, including when a replay contains records from different
versions. The current H2CK writer and standalone frame pages still use v7.
[Discovery and stream API](live-api.md#streams-websocket)

V8 keeps the length-prefixed container and v7 field grids. Each key
embeds a complete v7 key including the known machine travel. Between
keys, a forward byte-range coder writes one packet per sample, using
causal field predictions, grouped unchanged fields and adaptive symbol
frequencies. The key resets the field history, dictionary and symbol
tables. A subscriber starts at a key and receives the following deltas
in order. An existing stream has a key at least every 150 samples.

The seeded 16-player match used about 81 bytes per player per second on
v8 and about 145 on v7, including keys and record lengths.
[Measurement workload](../engine/live-telemetry.md#match-and-measurement)

After the usual ULEB record length, the first byte is
`8 | (key << 4) | (min(sid, 7) << 5)`. Slot 7 adds ULEB `sid - 7`.
A ULEB sequence follows. A key carries one bare v7 key of stream 0,
sequence 0. A delta carries ULEB tick increment and one terminated range
payload. Each stream maintains independent prediction and probability
state. A successful key supersedes earlier versions for that stream ID.

Frames travel bit-packed: game to server (`/v1/ingest`, binary messages),
server to readers (`/v1/stream?frames=packed`), recorded frames at rest
(frame chunks, [live-api.md](live-api.md#frame-chunks); before them
`GameFrame.data.packed`) and the replay fetch (`/v1/games/{id}/chunks`,
and `/frames` for older games). JSON readers can request `?format=json`
on an individual chunk or the legacy frame page. Live JSON readers use
`/v1/stream?frames=json`. See the [precision and transfer measurements](../engine/live-telemetry.md).
`crates/live-proto/src/pack.rs` writes and reads it;
`web/src/api/pack.ts` reads it; the golden vectors in
`web/test/fixtures/pack` (written by the Rust tests from invented frames,
read by the TypeScript tests) keep the two in step.

### Bits and numbers (v1–v7)

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
| position | three `seg(16)` | 1/1024 world unit (about 3 mm), coordinates under 10^9 |
| position in a delta | each axis gets one nonzero bit followed by `seg(1)` for a nonzero residual against the base | same grid |
| yaw | `u(14)` absolute | a turn in 16,384 steps, read back in (-π, π] |
| direction (aim, forward, up) | two absolute `u(14)` components | octahedral x and y, folded at z < 0, mapped to 0..16382. The midpoint 8191 gives exact cardinal axes |
| speed | `eg(6)` | 1/16 unit a second |
| hit amount | `eg(6)` | 1/256 of a vitality |
| shots | `eg(1)` | |
| stance | `u(3)` | `stand`, `crouch`, `airborne`, `vehicle`, `dead` |
| actions | `eg(0)` count (at most 8), then `u(3)` each | `grenade`, `melee`, `reload`, `jump`, `switch`, `pickup` |
| grenade | `u(1)` | `frag`, `plasma` |
| legacy device position | `u(10)` | 0..1 in 1023 steps |
| velocity | three `seg(8)` absolute. With a base, each axis gets one nonzero bit and `seg(1)` for a nonzero difference | 1/128 unit a second |
| fire | `eg(1)` | a tick mask, bit k a shot k ticks before the frame's tick (0 is not sent) |
| body | `u(6)` | 0..1 in 63 steps |
| shield | `eg(3)` | 1/32 of a shield, at most 4 |
| seat | `eg(0)` | at most 63 |
| ticks ago | `eg(0)` count (at most 8), then `eg(1)` each | one per action, at most 900 |
| hit ago | `eg(1)` | at most 900 |
| anim name | `str` | `[a-z0-9_:-]{1,64}` |
| anim frame | `eg(4)` | 1/16 of a frame, at most 32767 |
| string | `eg(3)`: index into the stream's string table | |
| id | `u(2)` form: 0 datum, `u(32)` (exactly 8 lowercase hex digits in JSON); 1 player `p<n>`, `eg(2)`; 2 anything else, a string index | |
| object kind | `u(4)` | the order of the kind table above, with `bipd` 0, `vehi` 1, `weap` 2, `eqip` 3, `scen` 4, `mach` 5, `ctrl` 6, `lifi` 7, `ssce` 8, `bloc` 9, `crea` 10, `netgame` 11 |
| object index | `eg(6)` | at most `0x7fff` |

An optional field (marked `opt` below) is a presence bit, then the value
when present.

### A frame

| field | wire |
|---|---|
| version | `u(4)`, 7 (6 has the same lists with the earlier quantisers and delta grammar. 5 still reads: a cinematic frame of version 4's, with the bit; 4: no projectiles, items or vehicle velocity, no prediction; 3: no anim; 2: none of version 3's fields either; 1: vehicles without parent and aim) |
| keyframe | `u(1)` |
| stream id | `eg(0)` |
| sequence | `eg(8)`, one more each frame of the stream |
| **keyframe:** game id | `str` literal |
| tick | `eg(20)` |
| bsp | `u(1)` present, then `eg(1)` |
| **delta:** tick | `eg(2)`, the difference from the previous frame (never negative) |
| bsp | `u(1)` changed; if so `u(1)` present, then `eg(1)` |
| **both (version 3):** cut | `u(1)` present, then `eg(16)`: the engine tick a campaign revert or restart came back to; on that one frame only |
| **both (versions 5–7):** cinematic | `u(1)`: a cinematic is playing ([Cinematics](#cinematics)); every version 6 or 7 frame has it, version 5 was written only for cinematic frames (always 1) |
| new strings | `eg(1)` count (at most 1024), then each a `str` literal, appended to the table (a keyframe starts an empty one) |
| players | list |
| vehicles | list |
| actors | list |
| hits | always whole: `eg(2)` count, then each entity |
| machines | list |
| objects | list |
| projectiles (version 6) | list |
| items (version 6) | list |
| machine travel (v7) | optional section, then zero padding to the byte boundary |

A keyframe list starts with an `eg(2)` count followed by each entity's
fields in schema order. A delta also starts with the count. V7 follows with
these opcodes, shown in the order the decoder reads their bits.

| bits | entity |
|---|---|
| `1` | Edit the previous entity at the cursor. |
| `00` | Run of predicted or unchanged entities beginning at the cursor. `eg(2)` gives the run length minus one. |
| `010` | New entity with all its fields. |
| `011` | Edit a previous entity at a signed `seg(0)` displacement from the cursor. |

After an edit or run the cursor moves past the last referenced entity.
Before v7, a run starts with `0`, a new entity with `10`, and every edit
with `11` and an explicit displacement. Each edit sends its mask and the
values of the selected fields in schema order.

Entity keys match an edit to its base even after a reorder. V7 remembers the
previous matched edit mask in each list, initially zero. Runs and new entities
leave that mask intact. For players and actors each five-bit group carries four
bits of the XOR of the current and previous masks, least significant group
first. The fifth bit requests another group. The last of five possible groups
must end the mask. An unchanged mask costs a single zero group. The field
orders, using the zero-based indices from the entity table, are:

| list | four fields per group, first group on the left |
|---|---|
| player | 14,16,3,8 / 13,2,6,11 / 20,9,10,7 / 1,4,5,12 / 15,17,18,19 |
| actor | 14,16,3,6 / 13,2,4,11 / 20,9,10,5 / 1,7,8,12 / 15,17,18,19 |

For the other lists a zero bit reuses the preceding edit mask and a one bit
starts an explicit mask, one bit per non-key field in schema order.

**Version 7 prediction.** For a gap of 1–30 ticks, advance the positions of
players, actors, vehicles and projectiles with known velocity. With velocity
`a` in 1/128-unit/s quanta, a constant velocity gives a displacement of
`round_half_away(a * 8 * dt / 30)` position quanta per axis.

For players and datum-ID actors, two equal successive spans allow an additional
prediction when the same numeric key occupied the previous slot both times.
Keep that slot's actual yaw, aim and velocity from one sample earlier. Predict
each field as twice the previous value minus the older one. Wrap yaw modulo
16384 and clamp each oct coordinate to 0..16382. Aim and velocity each require
two present values. When velocity can be predicted, use the mean of the last
actual velocity and predicted velocity for displacement, or
`round_half_away((a + b) * 4 * dt / 30)`. A keyframe clears all history.
A gap greater than 30 ticks disables prediction for that span.

Runs inherit the predicted state. Edits correct every difference from the
quantised endpoint. A changed yaw or oct component with a base uses
`0, seg(0)` for differences in −64..63 or `1, u(14)` for an absolute
component. Without a base it uses `u(14)`. Yaw differences wrap at one turn.
A matched unit's present animation phase with a present base and a span of
1–30 ticks uses one bit after the presence bit. A one advances the base by
16 quanta per tick. A zero precedes an absolute `eg(4)`.

V7 hit positions can reuse the exact quantised position of a player in the
same frame. When there are players, a one bit followed by a four-bit ordinal
selects one from the current wire-order list. A zero bit precedes the three
absolute coordinates for other hits. When the player list is empty the three
absolute coordinates follow directly.

**Versions 1–6.** Position uses 1/512 unit, velocity 1/64 unit/s, yaw 12
bits and each oct component 12 bits (maximum 4094). All edits send an
explicit mask. Plain position differences use three `seg(7)`, velocity
differences three `seg(4)`, and directions and yaw carry absolute codes.

**Prediction (version 6).** In a delta, the changed position of an entity
whose previous version had a velocity (players, actors, vehicles,
projectiles) is coded as its error from a prediction, three `seg(3)`, when
the frame's tick difference `dt` is at most 30 and each of the previous
velocity's components is at most 2^23 (in 1/64 unit a second); otherwise
as the plain difference, three `seg(7)`. Both sides know which before the
bits: it depends only on the previous entity and `dt`. The prediction, in
integer quanta, per axis: `p + floor((4 (a + b) dt + 15) / 30)`, where `p`
is the previous position, `a` the previous velocity and `b` the entity's
velocity in this frame (written later in the entity, so a decoder reads the
error first and adds the prediction once the entity is read; `a` again when
the new one is absent or past 2^23). That is the mean velocity over `dt`
ticks, rounded half up: exact for constant acceleration (a grenade's arc),
and a unit walking steadily codes 0 or ±1 per axis (4 bits) where the
plain difference of 4 units a second at 15 Hz is 10 bits, and a still axis
4 bits where it was 8. `web/src/api/pack.ts` does the same arithmetic in
doubles (exact: every term stays under 2^31).

| list | fields (key in bold) |
|---|---|
| player | **index `eg(2)`**, alive `u(1)`, pos, yaw, opt vehicle id, opt species string, opt aim, opt weapon string, opt shots, opt stance, opt speed, opt actions, opt grenade; version 3 adds opt velocity, opt fire, opt body, opt shield, opt seat, opt ticks ago; version 4 opt anim name, opt anim frame (both or neither) |
| vehicle | **id**, kind string, pos, forward, up, occupied `u(1)`, opt parent id, opt aim (version 2; version 1 ends at occupied), opt velocity (version 6) |
| actor | **id**, species string, pos, yaw, opt aim, opt weapon string, opt shots, opt team string, opt vehicle id, opt stance, opt speed, opt actions, opt grenade; version 3 adds the same six as a player |
| hit | opt from id, opt to id, opt weapon string, pos, opt amount, opt kind string; version 3 adds opt hit ago |
| machine | opt node string, **id**, kind string, pos, forward, up, opt legacy device position |
| object | **kind**, **index**, gone `u(1)`, opt pos, opt forward, opt up |
| projectile (version 6) | **id**, kind string, pos, opt velocity, opt parent id |
| item (version 6) | **id**, kind string, pos, forward, up, equipment `u(1)` |

### Machine travel in v7

Machine travel is an optional section after the items list at the current
bit cursor. Its first field is `eg(2)` count minus one (1–255 entries).
Each entry is `eg(3)` placement gap, then its absolute `u(8)` position.
Placements are strictly ascending from 0 to 32767. The gap is
`index - previous_index - 1` with previous index initially -1.

The section is absent when empty and takes at least 15 bits when present.
A decoder uses the current record's length. With at most seven bits left,
only zero padding is allowed. With more it reads the section, then
requires at most seven zero padding bits. Old v7 records end with the
same padding and still decode. The legacy 10-bit field in the
`machines` list remains part of the body.

A v7 keyframe replaces the known travel baseline. An absent section on a
keyframe means unknown travel for all devices. Travel of zero is explicit.
On a delta, an omitted index holds its latest value. A campaign cut
starts a new baseline. The server carries changes from throttled frames
and seeds the next keyframe after a reconnect and at each recorded chunk
start, including keyframes used to seek. Playback samples adjacent
delivered frames and holds quiet devices. It draws no telemetry during
cinematics, though a keyframe can still carry the known baseline for
playback afterward.

An idle frame adds zero bytes. A single change at placement 0–7 uses
15 bits, or 1–2 additional frame bytes depending on padding. The furthest
placement in a 206-machine map takes 23 bits as a single change, or 2–3
additional bytes. In a batch, a gap of 0–7 costs 12 bits per entry plus the
shared count.

### Streams, keyframes and gaps

An encoder starts with a keyframe and sends one again every N frames (the
game: 150, v7 subscriber stream: 15, v8 subscriber stream: 150). It also
sends one when the game ID changes, ticks go backwards, a checkpoint cuts
the recording, or the string table would pass 4096 entries.
A decoder keeps the last frame and string table per stream id; a delta
whose sequence is not the next one it expects (a dropped frame, or joining
mid-stream) is a gap: it drops deltas until the next keyframe. The reader
stream sends a joining or lagging reader every frame since the last
keyframe first, so it starts decoding at once. A frame that cannot be
packed (non-finite numbers, unknown names, too many strings) leaves the
encoder as it was.

### Compatibility

The first four bits select the decoder. Version 7 writes all seven
entity lists introduced through v6. A delta requires the preceding sequence
number and a base of the same version. Live subscribers recover at a
keyframe, and each recorded chunk begins with one. Rust and web fixtures
cover versions 1 through 8, including old v7 records without machine travel
and v8 keys and deltas. Recorded chunks keep their original bytes.
An old JSON `GameFrame` and a later base64-packed one are both readable
through the legacy frame page.

Deploy the web reader first, then the server and Worker, then reinstall the
game sender. Existing senders work with the updated readers. Clients that
consume JSON can keep using the JSON representations.

### Finished recordings

The viewer requests the archive-capable chunk index when a match ends.
Until publication it uses the original recording. Publication switches the
reader to independently decodable H2AR segments and keeps the current
playhead, speed and pause state. Each segment index entry may carry a
`machine_entry` seed with the last known device travel from its run. A cut,
rewind or complete travel baseline starts fresh. The seed restores quiet
doors and lifts on a seek without inserting synthetic frame rows. The
index maps source ticks across checkpoint rewinds, so a time link can fetch its target segment directly. The viewer
loads a neighbour near a segment edge to interpolate through it and releases
distant segments after a seek.

Local servers run the offline codec on the blocking pool. The Worker runs
one bounded segment on its single-threaded Durable Object per alarm or admin
step. The raw originals and their signed index remain available after
publication and rollback. [Archive format, limits and measurements](../engine/live-telemetry.md#finished-replay-archives)
and [migration procedure](replay-migration.md).

## Telemetry cost

Measured on the deterministic invented games in
`crates/live-proto/src/samples.rs` (a 16-player Slayer: 16 players, 4
vehicles, hits, weapon spawns; a campaign scene: 1 player, 60 actors, 8
vehicles, 12 machines, 25 crates moved, 16 objects gone), 450 frames each,
release build on the development Mac. Run
`cargo test --release -p live-proto frame_sizes -- --nocapture` and
`cargo test --release -p live-proto measure -- --ignored --nocapture`, and
`cargo test --release -p live-telemetry measure_frame_cost -- --ignored --nocapture`.

### Version 6 (2026-11-19, estimated)

Written on a machine with no cargo runs, so these are worked out from the
codes above, not measured; `cargo test --release -p live-proto
frame_sizes -- --nocapture` and the bench below print the real ones.

| per frame, live (deltas) | version 4 | version 6, estimate | what moved |
|---|---:|---:|---|
| samples, 16-player MP | 324 B | about 295 B | 16 players' positions predicted (about 28 to 12 bits each); adds a grenade in flight two thirds of the time, 4 to 5 dropped weapons and the flag (a run), 4 vehicles' velocity |
| samples, campaign | 931 B | about 850 B | 46 moving units predicted; adds plasma grenades (one or two in flight), 12 dropped weapons, 8 vehicles' velocity |
| bench `mp16` | about 375 B | about 340 B | 16 players and 12 driven vehicles predicted; adds 4 grenades in flight and 8 dropped weapons |
| bench `campaign100` | about 1610 B | about 1425 B | about 95 moving units and 16 vehicles predicted; adds 8 grenades, 40 dropped weapons |
| keyframe, samples MP / campaign | | +140 B / +290 B | the new lists whole (an item is about 18 B) |

At 15 Hz that is about 4.4 KB/s for a 16-player game (90 KB/s as JSON,
4.9 KB/s on version 4) and about 12.8 KB/s for a busy
campaign (14 KB/s on version 4): a viewer gets grenades, rockets, dropped
weapons and the flag for less than it paid before. The bench fixture's
units now carry their motion's own velocity (it set a fixed 4 units a
second on a path whose speed swings from 2 to 4, which a predictor would
pay for); the game's velocity is what the engine moves positions by.

Game thread: the world scan reads, for each projectile, its definition and
pose (40 bytes), velocity (12) and parent (4), and for a stuck one its
parent's position; for each loose weapon or equipment one pose read; the
loose test reuses the placement read. A few dozen reads more in a busy
scene, about 1 µs; no allocation once the kept lists have grown
(`the_game_threads_tick_does_not_allocate` covers the new lists). Worker:
the prediction is a few integer operations per moving entity.

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
  vehicles and 6 turrets, 30 machines, 600 scenery, 930 objects). Since
  version 6 each also has grenades in flight (4, 4, 8) and dropped weapons
  (8, 16, 40). Each
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

The telemetry-store lane (2026-11-18) took the last of the game thread's
per-tick costs off it. It was not measured on the machine it was written
on (no cargo runs there); the numbers below are what to expect and what to
check with the bench command above:

| scene | allocs per tick before | after | frame tick before | expected | other tick before | expected |
|---|---:|---:|---:|---:|---:|---:|
| mp16 | 4 | 0 | 10.6 | 8–10 | 4.9 | 4–5 |
| mp16_1000 | 4 | 0 | 21.2 | 18–21 | 4.7 | 4–5 |
| campaign100 | 4 | 0 | 25.6 | 22–25 | 1.0 | 1.0 |

The four allocations were the roster walk's list (`Datums::live` collects
the live player slots; the read now walks the slots in place) and the shots
map's B-tree node (a `BTreeMap` frees its nodes on `clear`; `event::Shots`
is a sorted list that keeps its room, iterated in the same order, so frames
are unchanged). The frame tick also no longer wakes the worker (the worker
polls in a game, see "Threads"): a futex wake is 1–5 µs on a loaded
machine, more when it lands on a sleeping core. An unchanged tick now reads
the clock and stops, where it read the whole roster first. The zero is
enforced by `bench::the_game_threads_tick_does_not_allocate`; the µs are
expected, so read them from `guest.json` (`frame_tick.total.p50_us`,
`other_tick.total.p50_us`, `*.allocs_per_tick`).

The fourth lane worked on the server and the web page. Server, before and
after, both at a load average of 177 to 188, so read the numbers as
directions, not measurements (the CPU-only `open+verify` step, whose code
did not change, ran 3x faster in the after run):

| stage | before | after |
|---|---:|---:|
| ingest, 64 games at 15 Hz | 1874 µs/frame (534/s) | 672 µs/frame (1489/s) |
| ingest, one game flat out | 681 µs/frame | 343 µs/frame |
| packed keyframe decode | 618 µs | 413 µs |
| events, batch of 1 | 2150 µs/event | 1487 µs/event |
| events, batch of 32 | 922 µs/event (29.5 ms/batch) | 621 µs/event (19.9 ms/batch) |
| fan-out, packed, 1 / 10 / 100 readers | 64.7 / 18.6 / 11.5 µs/frame/reader | 29.7 / 12.8 / 11.9 |

What changed: ingest takes a lock per game rather than one for all games,
and the read model's write lock once a second, not every frame. A stream
reader's token and batch timers are made once. The packed decoder reads a
word at a time, moves unchanged entities out of the previous frame instead
of cloning them, and sizes its lists up front. A batch's JWS is stored
once (`batches`) rather than in each of its events' rows: a batch of 32
writes about 15 KiB of JWS, not 490 KiB, and in memory its events share
one copy.

Web (`bun`, unloaded): decode 110 → 72 µs a frame (keyframe 232 → 159),
push 7 µs either way, repaint 160 → 95 µs (vehicles 33 → 12, machines
36 → 8, objects 43 → 26; actors 39 µs either way). Converted entities are
cached on the decoded entity, and a frame's sampled lists are made once
per frame and shared across repaints.

The fifth lane batches packed frames once for every reader: ingest appends
each game's packed frames to a pending buffer, and every 100 ms the server
publishes one tick (every game's batch, and their bytes back to back) to
readers of all games, and each game's batch alone to readers of that game,
who are not woken by the others (`crates/live-server/src/packed.rs`). A
reader that may see every game and follows every stream sends the tick's
bytes as they are; otherwise it copies the records that follow what it was
sent. Readers never get a frame twice or an older one (the sequence rule
in `Follow`), so a reader that joins mid-game or falls behind is sent each
stream from its keyframe, and the ticks that repeat it are trimmed; the
wire is unchanged. Fan-out CPU per frame above the no-reader run, 8 games
at 15 Hz, before and after back to back at a load average of 11 to 15:

| packed readers | before µs/frame/reader | after | total before | after |
|---|---:|---:|---:|---:|
| 1, every game | 34.3 | 17.7 | 34 | 18 |
| 10, every game | 26.8 | 11.9 | 268 | 119 |
| 100, every game | 17.5 | 9.3 | 1747 | 933 |
| 8, one game each | 14.1 | 8.2 | 113 | 65 |
| 80, one game each | 14.4 | 4.2 | 1155 | 340 |

JSON readers are unchanged (60 to 67 µs a frame a reader at 100). What
remains for a packed reader is a WebSocket send each tick and, in this
in-process bench, the reader's own receive.

## Runtime validation needed first

Unit tests use synthetic guest memory and do not execute the game. Before
relying on live output, check in this order:

1. Successful start/end and tick continuation behavior in a local Slayer game,
   including original return values and no lost guest calls.
2. Player name decoding, roster changes, and statborg values against the
   in-game scoreboard; verify kill attribution and score coalescing.
3. Vehicle-parent position/facing and the `vehicle` identifier.
4. Server discovery, game creation, token refresh and signed-batch
   acceptance against the live ingest implementation. Reconnect, the outbox
   and resends are tested against the embedded server through a proxy that
   cuts, refuses and mutes the socket (`crates/live-telemetry/src/server_tests.rs`);
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
   multiplayer weapon (the map-file lookup by short name), `aim` following
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
