# Live telemetry: frame species, cinematics and appearance

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
  spawned ones stay in `objects` ([Scenario objects](live-telemetry.md#scenario-objects)).
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
  above 0 is `jump`; entering the melee mode (6) is `melee`. One swing is
  one `melee`: its damage lands a few ticks into it, so dealing melee
  damage marks the swing only when no frame saw the entry (a hit within
  15 ticks of a marked entry is the same swing).
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
  channel. The object body holds the scenario index at `+0x1a`, normalised
  travel at `+0x140`, and the channel at `+0x18c` (see
  [live-gamestate.md](live-gamestate-objects.md#unit-stance-actions-damage-and-machines)).
  The sender rounds travel to 0..255 and sends each index once at first
  sight, then on changes to that byte. This works independently of the
  object's root pose and tag name. The viewer samples the device animation
  with those values. Earlier recordings keep their world-pose playback.

For a retail-based mod, the game start reports the retail base in
`map` and the installed package in `map_id`. Both come from the VFS
entry named by the active cache slot (`crates/extensions/game/src/map_slots.rs`).
A DVD override reports its own content ID even when an installed package
shares its name.

The join uses the content ID and the report can
group with games on the same base. [Map packages](../game/maps-new-maps.md#package-offers)

Game starts say their `kind`: `campaign` for campaign mode, with
`difficulty` from the active options' `+0x12a` word (0 easy to 3
legendary; anything else leaves it out); multiplayer games are `custom`
unless the host has called `Telemetry::set_playlist`, which makes them
`matchmaking` with that playlist name and id. Nothing calls it yet:
live-hle has no matchmaking (its sessions are all advertised `custom` with
no hopper, `crates/live/live-hle/src/sessions.rs`), so the playlist source is
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

**Campaign.** The map activation `0x0018f000` also runs on every checkpoint
revert and mission restart (`0x0018e700` re-activates with a copy of the
active options). A re-activation of the campaign map the current game is on
continues that game instead of ending it and starting another (which also
spent one `POST /v1/games` per revert against the server's write rate
limit, and a refused creation stops the sink). A different mission, a
multiplayer map or the main menu ends it as before.

The revert winds the
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
([live-gamestate.md](live-gamestate.md), "Cinematics (cutscenes)").

It is
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
every frame ([Packed frames](live-telemetry-packed.md#packed-frames)); before it, version 5 was
written for cinematic frames alone, so a server or page from before it
still read every other frame.

Tag names are (likely) not in guest memory: nothing in the XBE refers to
the cache header's tag-name fields, and the loader reads the tag index and
metadata. On map activation telemetry asks boot for the active cache
slot's file through the VFS (`map_override::active_map_source`): its host
region, or, for a map the VFS rewrites in memory (the pogo kit, the
screens; such a map has no contiguous host file), those bytes
(`MapFile::memory`). When the slot cannot identify an unbound
map, the boot locator uses the cache name.

Telemetry reads the tag names
with `crates/formats/map` and keeps only `bipd` and `weap` rows. The table is used only if the file's scenario datum equals the
guest's (tag index header `[0x00547f00]+0x0c`), since both indexes hold the
same salted datums. Without a match, or without a locator (tests, a raw
XBE), `species` and `weapon` are left out.

## Velocity, seat, vitality and fire

Players and actors carry the protocol's version 3 unit fields from capture
(docs/live/replay-fidelity.md, step 4). Offsets and evidence are in
`crates/live/live-telemetry/src/layout.rs`; all are confirmed by static reading
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
second.

Sending everything every tick doubles the world scan's game-thread
cost (it runs on frame ticks only, `pipeline.rs`) and the bytes, while the
server forwards at most 15 frames a second (`FRAME_INTERVAL`, 66 ms,
`crates/live/live-server/src/games.rs`) and records the first frame of each
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

Appearance rides the roster events only - never frames or scores - and is
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
