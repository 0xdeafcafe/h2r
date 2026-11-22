# Rules world API: a synced world for scripts

Status: Design. Nothing on this page is built; it extends the rules engine of [rules-engine.md](rules-engine.md) and is built after that page's host runtime runs in a game.
Code: none yet. The plan names `crates/formats/rules/src/caps.rs` (the entries), `crates/formats/rules/src/show.rs` and `crates/extensions/game/src/rules/world/` (new), and `crates/live/actor-net/src/wire.rs` (two records). Tests: named per slice in "Implementation plan".
Switch: none of its own; the `rules` feature of `H2_FEATURES` ([switches.md](../runtime/switches.md)).

A rules script moves objects, flies drop ships, orders AI squads and
phantom bots, plays dialogue and sounds, draws HUD elements and reads
physics, and every machine in the game sees and hears the same thing.
With that surface Firefight and Superbounce are scripts over a general
API, and their directors, detectors and HUD views leave the engine code.
The rule that must hold is the rules engine's: the same IR, options,
seed and inputs give the same engine calls at the same ticks, and a
machine that does not run the script changes nothing in its world.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
Everything not marked **E**, **Code** or **S** is **Design**. Addresses
are guest addresses of the retail XBE, named as research points. A
status word in an API table is `caps::Status`: **named** (a point in
`crates/hooking/guest-layout/src/points.rs`), **traced** (an address in
a doc), **untraced**, **not built** (the game-side part is ours and does
not exist). No game data is on this page.

## Decisions to confirm

1. **A script never runs on a distributed client.** Its world reaches
   clients through the engine's own replication; what replication does
   not carry (HUD, messages, waypoints, sounds, music) travels on one
   new host-to-client stream, the show channel.
2. **A synchronous peer runs the script itself**, as it runs every other
   part of the simulation. No call log is sent.
3. **Places are labels first.** A script moves a thing to a labelled
   thing or along a labelled path. A `vec` exists for offsets,
   velocities and reads.
4. **Physics is read, and written only as velocity** at a tick boundary
   through the engine's own entry. A script never writes Havok memory.
5. **Bots are ordered, not driven.** An order is a hint to a phantom's
   brain on the host; it returns nothing to the script.
6. **Splitting a script:** sections and an outline with the first slice
   that touches the editor; `use` modules later; no node editor.

## How it works

### Who runs the script

**Code** (today): the start observer makes a runtime on every machine,
and the first tick boundary drops it on a machine
`GAME_ENGINE_AUTHORITATIVE` (`0x15b2f0`) answers false for
(rules-engine.md "The host runtime", `crates/extensions/game/src/rules/runtime.rs`).
A guest is idle and learns the game from replicated state.

**Design.** The runtime takes a role from the simulation world's mode,
as Firefight's does (`crates/extensions/game/src/firefight/runtime.rs`;
modes 3 and 5 in [live-coop.md](../live/live-coop.md)):

| Session | Machine | Role | Runs the VM | Makes world calls | Draws the script's HUD from |
|---|---|---|---|---|---|
| alone, split screen | the one machine | `Authority` | yes | yes | its own show state |
| distributed: Live custom and hopper games, System Link, a dedicated host | the host | `Authority` | yes | yes | its own show state |
| distributed | a client (mode 5) | `Viewer` | no | no | the show channel |
| synchronous: campaign-style co-op sessions, Firefight's synchronous fallback | the host | `Authority` | yes | yes | its own |
| synchronous | a peer (mode 3) | `Replica` | yes: the same blob, options and seed | yes: the same calls at the same ticks | its own |
| a host's film in Theater | the viewer | `Authority` of the replay | yes, re-run | yes | its own |
| a client's film | the viewer | `Viewer` | no | no | the recorded show records |

A `Replica` is not a second authority. In a synchronous session every
machine simulates everything, so a call only the host made would split
the arenas. Each machine's VM reads the same arena, holds the same
`RULES` record (`sim_ext::RULES`, in the sim hash) and makes the same
calls, and the lockstep hash (`lockstep::hash_sim`) is the check, as it
is for the rest of the simulation. A late joiner gets the record with
the snapshot and `Vm::resume` continues from it. A `Replica` needs the
blob before the game starts: the lobby fetch a guest already makes for
the display part (rules-engine.md "Carrying a script") becomes a start
condition in a synchronous session, checked beside the start gate
([rules-start-gate.md](rules-start-gate.md)).

Squad commands stay host log entries (`crates/formats/host-log`,
[ai-networking.md](ai-networking.md)): each `Replica` appends the same
entries and the keep-alive's log head comparison covers them. This
replaces the `Replica` row of firefight.md's role table, where a peer
applies entries it receives and runs no director; no transport for
those entries exists, and with the director a script a peer runs it.

### How a change reaches every machine

In a distributed session the `Authority` makes each call at a tick
boundary through an engine entry, and the change travels by one of three
paths:

| Change | Path | Evidence |
|---|---|---|
| score, lives, round and game end | the statborg, player and engine-globals entities (types 7, 6, 0 to 5) | **E**, [notes/ai-networking.md](notes/ai-networking.md) |
| an object made or deleted | the entity of its group: unit 9, item 10, generic object 11, vehicle 12, weapon 14, turret 15, device 16 | **E** the types; **S** that a host-made object of each group is enrolled (forge-design.md slice F3). `SIM_ENROLL_OBJECT` (`0xa7870`) enrols one explicitly |
| position, facing, velocity | the shared object flags: position, forward and up, both velocities (`0xa6430`) | **E**; `OBJECT_SET_VELOCITY` (`0xb7740`) marks the entity unless its `quiet` argument is set (**Code**, `points.rs`); **S** that `OBJECT_SET_TRANSFORM` (`0xb7430`) marks position |
| AI bodies, shots, grenades, deaths | the unit entity and the weapon, grenade and damage events | **E**; probes P1 and P3 of ai-networking.md |
| a unit enters, leaves or boards a vehicle | events 24, 11 and 8; the unit's parent-vehicle flag | **E** |
| an actor's dialogue line | the actor channel's `Line` record | **Code**, `crates/live/actor-net/src/wire.rs` |
| a retail game event with its retail line and announcer sound | the game engine event (type 7) | **E** the type and the record's fields ([hud-kit.md](hud-kit.md#game-events-and-medals-0x0019de80)); generating one is traced only as `SIM_EVENT_GENERATE` (`0xb5a70`) |
| HUD values, waypoints, countdowns, script messages, sounds, music | nothing in retail: no entity field and no event carries them | **E**; the show channel below |
| a bot order | nothing: a phantom's pad is a player's input on the host, and its body replicates as any player's | **Code**, `crates/live/bots/src/phantom.rs` |

### The show channel

**Design.** What a player sees and hears from a script, and that the
engine does not replicate, is one of two things:

- **Show state**: values that hold until changed. Up to 16 HUD widgets
  (value, fill, style, text index, audience), 8 waypoints (an object's
  entity index or a point, text index, audience), 4 countdowns (the end
  tick) and one music slot (a tag-table index and its start tick). It is
  a table in `RulesExt`, so checkpoints, snapshots, films and the sim
  hash carry it on every machine that runs the VM. It encodes to at most
  255 bytes.
- **Cues**: one-shot events with a tick. A message (text index, up to
  four typed hole values, audience), a sound (tag-table index, an entity
  or a point or an audience) and a medal line. A cue is not state: a
  machine that joins later does not get it.

An audience is a 16-bit mask of player slots. A text is an index into
the blob's string table and a sound an index into its tag table, so a
record names only what the verified blob holds; a `Viewer` has the blob
from the lobby fetch.

The actor channel (`crates/live/actor-net`, ai-networking.md "How it
works") carries both, with two more record tags:

| Record (tag) | When | Payload |
|---|---|---|
| `Show` (7) | the state a peer's players are the audience of changed, at most 5 times a second; whole in every `Key` | length (u8), the state filtered to that peer's players |
| `Cue` (8) | a cue for that peer's players, in three consecutive packets as `Line` is | cue tick (u32), sequence (u8), kind (u8), at most 24 bytes |

`Digest` gains a 64-bit hash of the peer's filtered state; a mismatch
asks for a `Key`. The host filters per peer, so a team's waypoint is not
sent to the other team's machines. A `Viewer` applies records before the
game tick and draws through `rules::show::view`, a pure function of the
state bytes, the blob's tables and the local roster slot, the same
function the `Authority` draws its own HUD with (the pattern of
`crates/extensions/game/src/firefight/hud.rs`). Repeated cue copies are
dropped by (tick, sequence).

**System Link.** The actor channel rides the Live tunnel, and a System
Link session has none (ai-networking.md "Decisions"). The show channel
gets a LAN carrier: datagrams of ours between the same peers the game's
own packets reach (slice W11). Until it exists, a script that declares a
show capability plays on System Link with its world synced and its HUD,
messages and sounds on the host only, and the lobby's status line says
so by name; nothing is faked on the guest.

**Join in progress.** A distributed session takes joins in the lobby
only (ai-networking.md "Join in progress"). When that lifts, the first
`Key` to the new peer holds the show state.

### Films and Theater

- **A host's film** replays the host's machine, and the VM re-runs
  (rules-engine.md "Films"). Movers, the show state and the sight table
  are `RulesExt` records, so a keyframe restores them and a seek
  re-derives the HUD. The feature's hooks are in `simulation_hooks`.
- **Cues and show changes are also recorded** as tagged records at their
  tick in the film's `TICK` stream, beside the actor channel's records
  (ai-networking.md "Theater"), so Web Theater captions a message
  without a VM.
- **A client's film** holds the `Show` and `Cue` records it received.
- **A bot order is not replayed.** Playback reads each seat's recorded
  input; the order call is made by the re-run VM and ignored by a
  runtime in playback. This is safe because an order returns nothing.

What keeps the `Authority` replayable:

- A world call is made at a tick boundary through an engine entry, in
  program order, never by writing object or actor records.
- A mover's position is computed in Rust under the float rules of
  [sim-rewrite.md](../engine/sim-rewrite.md#float-rules-for-every-rust-sim-kernel-enforced-in-sim-core),
  from ticks, never from a clock.
- A physics read is of the state after the last finished tick, rounded
  once to 16.16.
- A read never comes from a brain, a renderer, a socket or a thread
  other than the game's.

### Bandwidth

| Item | Per client | Host with 15 clients | Evidence |
|---|---:|---:|---|
| Show channel cap: its own `actor_net::Budget`, 600 bytes in any 30 ticks | 0.6 KB/s | 9 KB/s | **Design** |
| One moved object: about 20 bytes at 10 updates a second | 200 B/s | 3 KB/s | **S**, ai-networking.md "Bandwidth" |
| 16 movers and one drop ship with 8 riders | 5 KB/s | 75 KB/s | **S** |
| Actors, as budgeted | 8 KB/s | 120 KB/s | **Design**, ai-networking.md |

Retail's sender shares one per-client budget between entities by
relevance (**E**, notes), so more moving objects lower every object's
update rate before they raise the byte rate. Probe P4 of
ai-networking.md measures it; the caps in "Rules and limits" move with
that result.

## The world API

Every entry is a row of `caps::ENTRIES` with its capability, kind, order
and status, appended to the table (nothing is published, so the API
version stays 1). Reads need no capability. Every action here is
`Order::Queued`: made by the `before` on the next `GAME_TICK`, in
program order, returning nothing.

### Types

All are front-end types, erased before the IR as `zone` and `time` are
([rules-language-types.md](rules-language-types.md#front-end-types)):

| Type | What it is | In the IR |
|---|---|---|
| `vec` | three `fixed`: `vec(x, y, z)`, `.x`, `.y`, `.z`, `+`, `-`, `* fixed`, `length(v)`, `distance(a, b)`. World units in 16.16, so a coordinate is within ±32,768 | three `Fixed` registers; a `vec` read is three calls with an axis argument |
| `vehicle` | an object whose tag is a vehicle; `as_vehicle(o)` gives a `vehicle?` | `Object` |
| `sound` | a tag path that must be a sound's, as `weapon` is a weapon's | `Tag`, with the tag table's group |
| `line` | a dialogue vocalization by name, checked at load against the map's dialogue names | `Text` |
| label kind `path` | an ordered set of at least two labelled points, numbered from 0 with no gap; the start gate checks it | the label table's `numbered` range |
| `hud` and `waypoint` declarations | a fixed list of HUD elements and waypoint slots the script owns (below) | a header table |

An action's `at` argument is an `object` (any handle, or a labelled
thing with no object, which already has a handle of its own) or a `vec`;
the compiler picks the entry form by type, as `message` does
(`caps::spelled`).

### Objects

Capability `objects.move` ("moves objects"); `player.move` ("moves
players") for a player's unit, so a hopper can deny one without the
other.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `o.position`, `o.velocity` (`vec`), `o.speed` (`fixed`) | `object_pos(Object, Int) -> Fixed`, `object_vel(Object, Int) -> Fixed`, reads | the object datum's position and velocities, the root ancestor's for a child (`guest-layout` `object.rs`; the engine's own are `OBJECT_WORLD_POSITION` `0xb9dd0`, `OBJECT_ROOT_VELOCITY` `0xba1d0`) | named | a read on the machine that runs the VM |
| `teleport(o, at)`, `teleport(o, at, offset)`, `face(o, at)` | `teleport`, `teleport_point`, `face` | `OBJECT_SET_TRANSFORM` (`0xb7430`): position, forward and up, false when the position is outside the world, which is a refused call | named | position and facing flags |
| `teleport(p, at)` | `teleport_player` | HaloScript `object_teleport`'s body `0x29ffb0` through `PLAYER_PLACE_NEAR` (`0x14e970`), which takes the unit out of a vehicle first (**Code**, `crates/extensions/agent/src/sim/calls.rs`, `crates/extensions/game/src/coop_players.rs`) | named | position flag; **S** how a client's own predicted biped takes it |
| `move_to(o, at, t)`, `move_along(o, path, t)`, `stop(o)`; `on arrive(o)` | `move_to`, `move_along`, `stop`; a VM event | a **mover**: a `RulesExt` record (object, from, to or path, start tick, duration) the runtime steps each tick boundary with `OBJECT_SET_TRANSFORM` and `OBJECT_SET_VELOCITY`, so a client extrapolates between updates | not built | position and velocity flags |
| `attach(o, to)`, `detach(o)` | `attach`, `detach` | no entry read; the object datum has a parent field, and HaloScript's `objects_attach` evaluator is not traced | untraced | nothing generic: only a unit's parent vehicle replicates (**E**). Until traced, `move_to` with the parent's `at` each tick is the script's follow |
| `set_power(o, f)`, `set_position(o, f)` on a device | `device_power`, `device_position` | Forge's device sinks ([forge-features.md](forge-features.md#teleporters-buttons-and-triggers)); the engine setters are not traced | untraced | the device entity (type 16, **E**) |

`create` and `delete` are the existing entries. A created object has no
handle the script can hold, since a queued call returns nothing; a
script that must move what it makes labels a placement and moves that.

### Vehicles and drop ships

Capability `vehicles.drive` ("flies and loads vehicles").

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `as_vehicle(o)`, `v.driver` (`player?`), `v.riders` (`int`) | reads | the unit's seat list (`UNIT_SEATS_BEGIN` `0xd0620`, `UNIT_SEATS_NEXT` `0xd0690`) | named | a read |
| `fly(v, path, t)`, `hover(v, on)` | `move_along` on a vehicle; `hover` | the mover above, facing along the path's tangent; `vehicle_hover` is known by its evaluator number only ([scripts.md](../halo1/scripts.md)) | not built; untraced | the vehicle entity (type 12) |
| `seat(p, v)`, `eject(p)` | `seat`, `eject` | the vehicle entry routine `0x11ade0` behind `unit_enter_vehicle` (`0x2a3ad0`, [script-builtins.md](../halo1/script-builtins.md)); the biped action `0x1e` the teleport path uses (`0xe68c0`) | traced | events 24 and 11 |
| `place_squad_in(s, class, n, v)` | `place_squad_in` | `AI_SQUAD_PLACE` (`0x201520`): its third stack word gates a vehicle-seat attempt ([firefight.md](firefight.md#guest-boundary)); which vehicle it tries is not read. `vehicle_load_magic` is not traced | traced | unit entities with the parent-vehicle flag |
| `unload(v)` | `unload` | `vehicle_unload`: not traced | untraced | event 11 |
| `on board(p, v)`, `on leave_vehicle(p, v)` | events | observers on the enter and exit events the engine queues (`SIM_EVENT_GENERATE` `0xb5a70`, types 24 and 11) | named | read on the `Authority` |

**A drop ship is a vehicle on a keyed path.** The kit places it, parked,
with a label (`label ship: object`), and labels the run-in as a `path`.
The script loads a squad, flies the path in a stated time, hears
`on arrive`, unloads and flies out. Three other ways are weighed:

| Way | Why not first |
|---|---|
| An AI pilot | engine flight needs airborne firing positions and a pathing kit a multiplayer map lacks; the result differs per run of the AI |
| A recorded path (`recording_play`) | the recording player (`0x1fb630`) applies unit control streams through `UNIT_APPLY_CONTROL` (`0xc6de0`, **Code**, `crates/extensions/game/src/h1_recordings.rs`); the streams are scenario data a multiplayer map does not hold, and authoring one needs a recorder |
| A device machine with an animation | no tag of ours can be authored for it (no game data) |

The Pelican and Phantom tags are not on a multiplayer map. They come
with the kit's stage 2 (`tags::schema::transplant` from the player's own
campaign maps, firefight.md "The kit"), so every machine builds them and
tag indices agree.

### AI squads

Capability `ai.squads` (exists: "places and commands AI"). A script
commands by squad and never per actor (rules-engine.md "Firefight, AI
and bounces").

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `place_squad`, `place_squad_at`, `hunt`, `erase_squad`, `squad_alive` | exist | `actor_net::squads::SquadEntry` | named | the host log; bodies by unit entities |
| `go_to(s, zone)`, `hold(s, zone)` | `squad_go`, `squad_hold` | `AI_SET_ORDERS` (`0x275cb0`) with an order the kit authors for each drop zone and each labelled zone, as it authors `ff_hunt` (firefight.md "Guest boundary") | named point; the kit's orders not built | the host decides; bodies replicate |
| `follow(s, p)` | `squad_follow` | an order's follow target, a field the kit writes empty; how the engine resolves it is not read | untraced | as above |
| `attack(s, o)` | `squad_attack` | no entry; `AI_TARGET_SCORE` (`0x265d30`) is the point a preference would answer | traced | as above |
| `set_squad_team(s, t)` | `squad_team` | `AI_SET_TEAM` (`0x275b60`) | named | unit team in the entity |
| a map's own squads, by name: `label wardens: squad = "<squad name>"` | a label kind `squad` | the scenario's squad block (`scnr+0x160`), read by name as the kit's slots are | not built | none needed |

### Bots

Capability `bots.orders` ("gives bots orders"). A bot is a phantom: a
player seat driven by a brain on the host ([bots.md](bots.md) section
5). To a script it is a `player`, and `p.is_bot` reads the phantom
registry (`bots::phantom::list`).

| Script | Entry | Backed by | Status |
|---|---|---|---|
| `bot_go(p, at)`, `bot_hold(p, zone)` | `bot_go`, `bot_hold` | the planner and route runner that drive a seat with no brain of its own (`bots::goto::Goto`, `OrderRun`; bots.md "Driving a player"; `crates/extensions/game/src/player_goto.rs` is its glue for the local player) | not built: a brain takes no outside order |
| `bot_follow(p, q)`, `bot_attack(p, q)` | `bot_follow`, `bot_attack` | the brain's own escort and target choice (`crates/live/bots/src/decisions.rs`), with the order as its goal | not built |
| `bot_free(p)` | `bot_free` | clears the order | not built |

An order is made on the `Authority` only and returns nothing. Whether
the bot got there is a world read (`inside(p, zone)`, `distance`), so a
script's state never depends on a brain, which runs off the game thread
(bots.md section 5.5). The effect is the phantom's pad, which every
session kind already carries as a player's input. Seats are added in the
lobby (bots.md section 7); a script adds and removes none.

### Audio

Capability `audio.play` ("plays sounds, dialogue and music").

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `say(u, line)`: a unit speaks a line of its own voice | `say` | the unit's voice bank resolves the vocalization (`0x1144a0`), `SOUND_START_AT_MARKER` (`0x1891d0`), `BIPED_DIALOGUE_LIP_SYNC` (`0x109220`): the path a client plays a replicated line with (**Code**, `crates/extensions/game/src/actor_net/client.rs`) | named | the actor channel's `Line` record, which exists |
| `sound(s, at)`: a sound in the world | `sound_at` | `SOUND_IMPULSE_ALLOCATE` (`0x126000`) admits a one-shot; the scripts' `sound_impulse_start` evaluator is `0x2a9e00`; its body is not read | traced | a `Cue` with the object's entity or the point |
| `sound(s, to)`: a sound in the ears of a player, a team or `all` | `sound_to` | as above, with no position | traced | a `Cue` to the audience |
| `music(s)`, `music_stop()` | `music`, `music_stop` | a looping sound's start and stop: not traced | untraced | the show state's music slot, so a late machine starts it at the right offset |
| `announce(event, p)`: a retail game event, with its retail line and announcer sound | `announce` | the game-engine event record and its responses (`0x19de80`, `0x19e890`, `0x19e770`, hud-kit.md) | traced | the game engine event (type 7), on System Link too |

A sound is a tag the loaded map holds, checked at load like any tag
path. A script brings no audio of its own.

### HUD

Capabilities `hud.message` (exists), `hud.widgets` ("draws on the HUD")
and `hud.waypoints` ("marks places and things").

A script declares its HUD once, as the HUD kit describes one
([hud-kit.md](hud-kit.md#the-model-cratesrewriteshud-kit-core)): a fixed
list of elements, each a kind and a place. A state only shows, hides and
words them.

```text
hud wave_line: text at top_right
hud lives_line: text at under_score
hud left_bar: meter at above_weapon
waypoint lz
```

A place is one of a fixed list of named slots the kit lays out against
the retail HUD in every screen mode; a script gives no pixels. The
checker builds the `hud_kit_core::Hud` and runs the kit's layout checks,
so two elements in one slot, or a text wider than its slot, is a compile
error. The game reads the declarations from the blob's header before
`GAME_ACTIVATE`, where the kit installs (`hud_kit::register`), on a
`Viewer` too.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `message(to, "…{n}")` | exist | the notification queue (`0x24cbee`) and the timed centred text (`0x24cc73`), per local user (hud-kit.md "Messages") | traced | a `Cue` |
| `hud_text(w, "Wave {n}")`, `hud_fill(w, f)`, `hud_style(w, style)`, `hud_show(w, to, on)` | `hud_text`, `hud_fill`, `hud_style`, `hud_show` | `hud_kit::Handle::publish` (**Code**, `crates/extensions/game/src/hud_kit.rs`); the text uses the message holes ([rules-language-types.md](rules-language-types.md#messages)) | named once the kit's runtime draws | `Show` |
| `countdown(w, t)` | `hud_countdown` | the state holds the end tick; each machine counts its own ticks | not built | `Show` |
| `mark(wp, at, to)`, `mark(wp, at, to, "…")`, `unmark(wp, to)` | `mark`, `unmark` | the game's navpoint list (`0x24da7d`) through `Handle::navpoint`; the kit holds one slot, and more than one is **S** | named for one | `Show` |
| `medal(p, "…")`: a line with a medal glyph, counted in the film and on Live | `medal` | the notification queue, as the superbounce medals (hud-kit.md "The medals"); `GAME_MEDAL` (`0x1970a0`) takes only the game's own medal rows | traced | a `Cue` |

Scoreboard labels and the post-game report's columns stay the carrier's
(rules-engine.md "Not verified").

### Physics

Halo 2's physics is Havok at an exact 30 Hz tick
([havok-runtime.md](../engine/havok-runtime.md),
[high-framerate.md](high-framerate.md)). Reads need no capability.
Writes need `physics.push` ("pushes objects") or `player.push` ("pushes
players").

| Script | Entry | Engine | Status |
|---|---|---|---|
| `o.velocity`, `o.speed` | as "Objects" | the object datum | named |
| `p.airborne`, `p.crouching` | `airborne`, `crouching`, reads | the biped's airborne bit and crouch state the superbounce tracker reads (`crates/extensions/game/src/superbounce_charge.rs`; `BIPED_AIRBORNE_TEST` `0xe4050`) | named |
| `on touchdown(p, down, up)`: a landing, with the fall speed before and the vertical speed after | a VM event | the touchdown rule of `superbounce_charge::touchdown`, run for every player ([superbounce.md](superbounce.md#the-rule)) | named |
| `p.primed`: the player's level pair is in the state a landing bounces from | `primed`, a read | the pair word of the biped's one pair against the level (`live_telemetry::havok_agents::fixed_pair`, **Code**) | named; its meaning is **Trace** in superbounce.md |
| `sees(a, at)`: nothing solid between two things | `sees`, a read of the sight table | `AI_LINE_OF_SIGHT` (`0x29dd50`) over the collision query `0x1697c0`, made in the tick boundary's async body as Firefight's sight pass is (`firefight::guest::line_of_sight`) | named |
| `touching(a, b)`, a contact's normal and impulse | reads | Havok's contact constraints (havok-runtime.md "Contacts"), read statically and never in a run | traced |
| `set_velocity(o, v)`, `push(o, v)`; `launch(p, v)` | `set_velocity`, `push`; `launch` | `OBJECT_SET_VELOCITY` (`0xb7740`), `quiet` clear | named; **S** for a client's own predicted biped |

`sees` cannot make a guest call from a handler, so it reads a **sight
table** in `RulesExt`: a pair asked for in one tick is traced at the
next boundary and answers from then on, false until its first trace; a
pair not asked for in 30 ticks is dropped. Firefight's `zone_seen` and
`zone_distance` are this read and `distance`.

**Read-only against write.** A read costs nothing in sync: it is the
host's own state, rounded. A write is safe only where the engine already
replicates the result and keeps its own invariants, which is velocity
through its setter at a tick boundary: the solver integrates it next
tick like any impulse, and the velocity flag carries it. Writing
positions of bodies in contact, the pair word, contact points, per-body
gravity or collision filters is refused by having no entry: the Havok
heap is outside the checkpoint arena
([sim-rewrite-havok.md](../engine/sim-rewrite-havok.md#31-the-havok-heap-is-not-in-the-checkpoint-arena)),
a write there is not in a snapshot, and nothing replicates it. So the
API is one way in each direction: physics to script as rounded reads and
events, script to physics as a clamped velocity. A superbounce stays
observed, never made.

### Advanced data

Reads only, no capability.

| Script | Entry | From | Status |
|---|---|---|---|
| `o.tag`, `o.health`, `o.shield` (`fixed`, 0 to 1), `p.weapon` (`weapon?`) | reads | the object and unit datums (`guest-layout` `object.rs`, `unit.rs`); the held weapon as [bots.md](bots.md) section 3.6 reads it | named |
| `tag_int(t, "field")`, `tag_fixed(t, "field")` | reads | the field path is resolved at load against our tag schema (`tags::schema`) to a group, offset and type, and refused by name when the schema lacks it; the value is read from the loaded map | not built |
| `squad_alive(s)`; `squad_fighting(s)`, `squad_strength(s)` | exists; reads | `actor_net::squads::count_by_squad`; for the other two, actor state beyond what the agent API's `world.ai` snapshot reads (`crates/extensions/agent/src/ai.rs`) | named; untraced |
| `on actor_kill`, `on actor_assist`, `on squad_dead` | exist | `SimEvent::ActorKilled` (`crates/rewrites/sim-objects/src/events.rs`) | named |
| `on damage(o, by, amount)` | an event | `SimEvent::Damage`, the same stream | named while the object update runs in Rust |

Every machine in a game loads the same map, so a tag read is the same
on each. A float field is rounded once to `fixed`. A script names a
field by our schema's name; it sees no offset.

## Firefight and Superbounce as scripts

### What is engine code today

| Mode | Engine-specific code | Entries only it uses |
|---|---|---|
| Firefight | the director and scoring (`crates/formats/firefight/src/director.rs`, `scoring.rs`); the generator that builds its IR (`src/script.rs`); the host and its log (`crates/extensions/game/src/firefight/host.rs`); the mode blob and HUD view (`firefight/hud.rs`); the sight pass | `ff_round`, `ff_wave`, `ff_skull`, `ff_lives`, `ff_score`, `ff_respawn`, `ff_finish`, `zone_seen`, `zone_distance`, `place_squad_at`; capability `firefight.director` |
| Superbounce | the medal chain and its rule (`crates/extensions/game/src/superbounce_chain.rs`); the mode's HUD states and navpoint choice (`superbounce_mode.rs`, `superbounce_hud.rs`); the mode switch behind `hud.superbounce`, a capability that gates no entry | `on bounce`, `on bounced` |

### Firefight

| The mode needs | From the API |
|---|---|
| waves, rounds, gaps, the reserve and the cap | tables, timers and variables: the VM alone |
| a squad placed at an unseen, far zone | `place_squad_at`, `sees`, `distance` |
| squads sent at the players, erased in cleanup | `hunt`, `go_to`, `erase_squad` |
| waves by drop ship | `place_squad_in`, `fly`, `on arrive`, `unload` |
| kill points and assists | `on actor_kill`, `on actor_assist`, `score` |
| shared lives and respawn | `set_lives`, `respawn`, `on death` |
| skulls by round | `set_skull(id, on)`, a new entry of capability `game.skulls` over the skull flags Firefight's `Skull` command writes |
| round, wave, enemies left and lives on every HUD | `hud_text`, `hud_fill`: the show state replaces the mode blob |
| the wave's stinger and an Elite's taunt | `sound`, `say` |
| the end of the game | `end_round` |

```text
rules "Holdout" 1
carrier slayer
teams 1
requires ai.squads, vehicles.drive, hud.widgets, hud.message, audio.play

table waves: (class: Class, per_player: int, min: int, max: int, gap: time, by_air: bool) = [ ... ]
table points: (class: Class, score: int) = [ ... ]
const per_round = 5

label ff_drop: zones at least 6
label ship: object
label run_in: path
label run_out: path

hud wave_line: text at top_right
hud lives_line: text at under_score

option start_lives: int 1..20 = 7

global var wave: int = 0
global var pool: int = 7
global var next: timer = 5s

fn hidden(z: zone) -> bool = count(players p where p.alive and sees(p, z)) == 0

fn show() {
    let r = wave / per_round + 1
    let w = wave % per_round + 1
    hud_text(wave_line, "Round {r}, wave {w}")
    hud_text(lives_line, "Lives {pool}")
}

on init { pool = start_lives; show() }

on timer(next) {
    let w = waves[wave]
    let s = squad(wave % 16)
    let n = clamp(count(players) * w.per_player, w.min, w.max)
    if w.by_air {
        place_squad_in(s, w.class, n, ship)
        move_along(ship, run_in, 12s)
    } else if let z = first(ff_drop z where hidden(z)) {
        place_squad_at(s, w.class, n, z.id, 0)
        hunt(s)
    }
    message(all, "Wave incoming")
    show()
}

on arrive(o) {
    if o == ship {
        unload(ship)
        hunt(squad(wave % 16))
        move_along(ship, run_out, 12s)
    }
}

on squad_dead(s) {
    wave = wave + 1
    if wave == len(waves) { end_round(team 0) } else { next = waves[wave - 1].gap }
    show()
}

on actor_kill(killer, class) {
    if killer != none { score(killer, points[class].score) }
}

on death(p) {
    if pool > 0 { pool = pool - 1; respawn(p) }
    else if count(players q where q.alive) == 0 { end_round(none) }
    show()
}
```

The shipped director is larger (the reserve, cleanup, the zone ranking,
exact kill multipliers; firefight.md "The director"). It is written by
hand as `firefight.rules` with the wave set as its tables, and
`firefight::script::compile` fills the tables from a `Def` instead of
building the whole IR. The Rust director stays the oracle in tests until
the script has matched it on every scenario of
`crates/formats/firefight/tests/script.rs`; then the `ff_*` entries and
`firefight.director` are deleted.

### Superbounce

| The mode needs | From the API |
|---|---|
| a landing, with its speeds | `on touchdown(p, down, up)` |
| whether the player was primed the tick before | `p.primed`, kept in a player variable each tick |
| whether the landing was on a strip | the scanner's strips as a built-in label (`label strip: zones = "sb_strip"`), a label source beside the netgame flags in `rules/census.rs`; `distance` |
| chains and a fall out of the map | player variables, `on kill` |
| the priming pill, the next spot's navpoint | `hud_show`, `mark` |
| medals | `medal` |

```text
rules "Superbounce" 1
carrier slayer
list tweakers
requires hud.widgets, hud.waypoints

label strip: zones = "sb_strip"

hud primed_pill: text at above_weapon
waypoint next_spot

option bounce_points: int 0..5 = 1
option monster_double: bool = true
const launch = 8.0
const monster = 13.0

player var was_primed: bool = false
player var chain: int = 0
player var flying: bool = false

on init { hud_text(primed_pill, "Primed") }

on touchdown(p, down, up) {
    if let s = first(strip z where distance(p, z) < 0.5) and p.was_primed and up > launch {
        p.chain = p.chain + 1
        p.flying = true
        let n = p.chain
        let points = bounce_points * min(n, 6)
        if up > monster and monster_double { score(p, points * 2) } else { score(p, points) }
        medal(p, "Superbounce x{n}")
    } else {
        p.chain = 0
        p.flying = false
    }
}

on kill(victim, killer) {
    if killer == none and victim.flying { score(victim, -1); medal(victim, "Bounced") }
    victim.chain = 0
    victim.flying = false
}

on tick {
    for each players p {
        hud_show(primed_pill, p, p.primed)
        if let s = first(strip z where distance(p, z) < 20.0) { mark(next_spot, s, p) } else { unmark(next_spot, p) }
        p.was_primed = p.primed
    }
}
```

The two speeds are placeholders for the values superbounce.md measures.
What stays Rust is what is not a rule: the scanner
(`crates/live/superbounce`), the floor marks and lock-on drawn over the
map (`superbounce_world.rs`), the fall assist, ghosts and the verifier.
The marks stay behind `hud.superbounce` until a script can draw lines in
the world, which this page does not design.

## Splitting a script

A built-in is 30 to 120 lines today; a hand-written Firefight director
is several hundred. Three ways to keep one readable:

| Way | What it is | Cost | Recommendation |
|---|---|---|---|
| Sections | `section "Waves"` on a line of its own, between declarations. It compiles to nothing. `ide::symbols` nests the declarations after it under it, editors fold it, and the workbench shows the outline as a side list that jumps to a section | a keyword, the outline and the formatter | build it with slice W2 |
| Modules | `use "waves.rules"` pulls another file's declarations in before checking. One blob, one hash, and the bundle stores every file. The CLI reads paths beside the script; Live and the workbench keep a script as a small set of named files | the bundle format, Live's script routes, the workbench's tabs, diagnostics with a file | after Firefight is a script and the need is measured; not before |
| A node view | a read-only graph of handlers, the variables they touch and the calls they make, drawn from the checked script | a new web view | not now. Forge's logic graph is the visual front end and compiles to the same IR (rules-engine.md "A Forge graph is the same IR"); a second node editor would be a third front end |

## Rules and limits

**The budget.** A world call is one op in the VM and work in the engine,
so each entry carries a cost the VM charges against the tick's 100,000
ops (`caps::Entry` gains `cost`): 1 for a datum read, 8 for an action,
64 for a new `sees` pair, 2 for a tag read. The worst-tick estimate uses
the same costs.

**Caps**, each a script error or a refused call when passed, the same
way on every run:

| What | Cap |
|---|---|
| movers alive | 16, four of them vehicles |
| teleports, pushes and velocity sets a tick | 16 each |
| a pushed or set velocity | clamped to 50 world units a second |
| `sees` pairs in the sight table | 64; 32 traces a tick boundary |
| cues a tick | 8; 30 a second to any one player |
| script sounds playing for one player | 4, and one music slot |
| HUD elements, waypoints, countdowns | 16, 8, 4 |
| a HUD text | 32 bytes after its holes are filled |
| the show state | 255 bytes; 600 bytes a second to a client |
| bot orders a tick | one per phantom |
| objects alive from `create` | 64, as rules-engine.md "Budgets" |

**What a script cannot do.** Run on a client. Read a gamertag, an XUID,
an input, a camera or another machine's state. Write Havok memory, a tag
or a datum. Name a coordinate system other than the world's. Play audio
or show text the blob and the map do not hold. Add or remove a player
seat. Reach the campaign's HaloScript.

**On Live.**

- Each new capability has its line of plain words in `caps::CAPABILITIES`,
  shown in the lobby and on the variant's page before the game.
  `player.move` and `player.push` are their own lines so a hopper's deny
  list can refuse them alone (`rules::policy`, rules-engine.md "Roles,
  powers and capabilities").
- Ranked play stays behind the allow-list; nothing here loosens it.
- A `Viewer` decodes show records with fixed bounds, and a record names
  text and sounds by index into the verified blob. A modified host can
  send a wrong value; it cannot send a string, a sound or a size the
  blob does not hold.
- A teleport outside the world is refused by the engine and kept in
  `rules.state`'s `errors`. A script can still be unfair to its players:
  it is the game the host chose, and the hopper builder is where a
  curated list keeps such a script out.
- Cue rates are per audience player, so a script cannot flood one
  player's screen or ears.

## Implementation plan

Every slice leaves the tree green, lands by itself and ends with
something to play. Paths under `rules/` are in
`crates/formats/rules/src`, under `game/` in
`crates/extensions/game/src`. Each slice adds its rows to
`caps::ENTRIES`, its signatures to `lang/sig.rs`, its fake to `fake.rs`
and its scenario events to `scenario.rs`, and updates
[rules-engine.md](rules-engine.md#the-engine-api)'s tables and the
`h2-rules` skill in the same branch. A slice whose engine entry is not
named starts with its research row below.

| # | Slice | Owns | Tests | Done when | After |
|---|---|---|---|---|---|
| W0 | Roles: `Authority`, `Replica`, `Viewer` | `game/rules/runtime.rs`, `game/rules/feature.rs`; the blob as a start condition in `game/rules/gate.rs` | over invented guest memory: a mode 3 peer keeps its VM and makes the host's calls; a mode 5 client makes none; a peer without the blob blocks the start by name | a script game scores the same on both machines of a synchronous session | the host runtime running in a game; R1 |
| W1 | Messages everywhere: cues | `rules/show.rs` (new: `Cue`, the codec), `game/rules/world/show.rs` (new), `crates/live/actor-net/src/wire.rs` and `budget.rs` (`Cue`), `game/actor_net/host.rs`, `client.rs`; `message*` to named | the codec round-trips and refuses each bad bound; a cue in three packets shows once; the budget drops the rest the same way twice; the fake logs each message with its audience | `message(all, "…")` from Siege shows on the host and on a Live guest | W0 |
| W2 | Script HUD: show state, `hud` and `waypoint` declarations, sections | `rules/show.rs` (`State`, `view`), `lang/*` (the declarations, `section`), `ir.rs` (the header table), `ide/*` (outline, folding), `game/rules/world/hud.rs` (new, over `game/hud_kit.rs`), `wire.rs` (`Show`, the digest), the workbench's HUD strip in `web/src/stats/RulesTest.tsx` | a declared layout that overlaps is a compile error; `view` of the same bytes is the same on host and client at every tick of a fake match; a `Key` restores the state after loss | a script's wave counter and a waypoint draw on host and guest; Firefight's HUD view reads the show state | W1; the HUD kit's runtime drawing |
| W3 | Objects in motion: `vec`, position reads, `teleport`, movers, `on arrive` | `lang/types.rs`, `lang/check.rs`, `lang/lower.rs` (`vec`), `vm.rs` (movers in `RulesExt`), `game/rules/world/objects.rs` (new) | `vec` arithmetic and each type error; a mover reaches its end on the stated tick and survives a checkpoint revert; a teleport outside the world is a refused call; a film of a mover replays to the same `RulesExt` hash | Race with moving platforms and a teleporter pad plays on Live | W0; R2, R3 |
| W4 | Physics reads and pushes: `airborne`, `primed`, `on touchdown`, the sight table, `push`, `launch` | `vm.rs` (the sight table), `game/rules/world/physics.rs` (new), `game/superbounce_charge.rs` (the touchdown for every player), the `sb_strip` label source in `game/rules/census.rs`; `builtin/superbounce.rules`, `builtin/bounce_practice.rules` | the sketch above scores the chain's recorded scenarios as `superbounce_chain_tests.rs` does; a `sees` pair answers one tick after it is asked and never from a handler; a push is clamped | the Superbounce game type's chain and medals come from the script; `on bounce` and `on bounced` are deleted | W2, W3; R9 |
| W5 | Audio: `say`, `sound`, `music`, `announce` | `game/rules/world/audio.rs` (new), the `sound` and `line` types in `lang/*`, the tag table's sound group in `ir.rs` and `verify.rs` | a sound that is not in the map blocks the start by name; a `sound_to` cue reaches only its audience; the music slot starts a late machine at the right tick | a wave's stinger and an Elite's taunt play on every machine of a Live Firefight | W1; R4, R5 |
| W6 | AI orders and the hand-written director: `go_to`, `hold`, `set_skull`, `firefight.rules` | `crates/tools/firefight-kit` (an order per zone), `game/actor_net/squads.rs`, `builtin/firefight.rules` (new), `crates/formats/firefight/src/script.rs` (fills tables) | every scenario of `crates/formats/firefight/tests/script.rs` passes against the hand-written script; a squad sent to a zone gets that zone's order on the fake squad entry | Firefight runs from `firefight.rules`; the `ff_*` entries, `zone_seen`, `zone_distance` and `firefight.director` are deleted | W2, W4, W5; Firefight's runs R1 to R4 |
| W7 | Bot orders | `crates/live/bots/src/brain.rs`, `decisions.rs` (an outside order as the goal), `game/rules/world/bots.rs` (new) | a phantom with `bot_hold` plans to the zone on a nav fixture; playback makes no order; an order changes no `RulesExt` byte | Clankers hold the zones a Firefight script sends them to | W0 |
| W8 | Vehicles and drop ships: `vehicle`, `path`, `fly`, `seat`, `place_squad_in`, `unload` | `rules/labels.rs` (`path`), `game/rules/world/vehicles.rs` (new), the kit's stage 2 and its labelled ship and paths in `crates/tools/firefight-kit` | a `path` with a gap blocks the start; a ship flies a path in the stated ticks, facing its tangent; riders are placed and unloaded on the fake | a Firefight wave arrives by drop ship on a kit map | W3, W6; R6, R7, R8 |
| W9 | Advanced data: tag reads, squad state, `on damage`, squads by name | `game/rules/world/data.rs` (new), the field resolver over `tags::schema` | a field the schema lacks is a load error naming it; a tag read is the same on two fake machines | a boss wave's bar shows the boss's shield | W2 |
| W10 | Modules: `use` | `lang/*`, `crates/formats/variants` (the bundle's files), Live's script routes, the workbench's tabs | two files compile to the blob of the same text in one; a cycle is an error naming both files | Firefight's director and its wave tables are two files | W6 |
| W11 | The show channel on System Link | a LAN carrier beside `crates/live/live-hle/src/tunnel_host.rs` and `tunnel_client.rs` | the two-socket System Link pair of ai-networking.md's probes exchanges a `Key` and a `Cue` | a System Link guest sees a script's HUD and hears its sounds | W2; R10 |

W0 is first. W1, W3 and W7 then run side by side; W2 follows W1; W4 and
W5 follow W2; W6 closes Firefight, W8 adds the drop ship, and W9 to W11
follow in any order. W2, W3 and W4 each edit `vm.rs` and `lang/check.rs`
and are in different waves.

Research, each a row that makes an entry named before its slice builds
on it:

| # | Question | Settles |
|---|---|---|
| R1 | What `GAME_ENGINE_AUTHORITATIVE` answers on a synchronous peer, and whether its `SCORE_ADD` applies there | W0 |
| R2 | Whether `OBJECT_SET_TRANSFORM` marks the position flag for a host-moved object, by its `notify` argument | W3 |
| R3 | Which object groups get an entity when the host makes or moves one (forge-design.md slice F3) | W3, W8 |
| R4 | The body behind `sound_impulse_start` (`0x2a9e00`) for a positioned and an unpositioned one-shot | W5 |
| R5 | The start and stop of a looping sound; how a retail game event is raised on the host | W5 |
| R6 | `vehicle_load_magic`, `vehicle_unload` and `vehicle_hover`: their evaluators and bodies; which vehicle `AI_SQUAD_PLACE`'s seat attempt uses | W8 |
| R7 | Whether a vehicle's Havok body accepts a transform each tick without fighting it | W8 |
| R8 | The kit's stage 2 transplant of a drop ship and its tag headroom on each map | W8 |
| R9 | How a client's own predicted biped takes a host-set position or velocity | W3, W4 |
| R10 | A datagram of ours between System Link peers through the HLE's sockets | W11 |
| R11 | `objects_attach`, the device setters and an order's follow target | `attach`, devices and `follow`, which no slice above needs |

## Not verified

- Everything in "Research" above.
- That a generic object's client extrapolates by the replicated
  velocity between updates, so a mover looks smooth at 10 updates a
  second.
- That 16.16 holds every coordinate of every map, converted Halo 1
  levels included.
- That the game's navpoint list draws more than one slot of ours.
- How many labelled strips a map's scan yields against the 256-member
  bound of a label.
- The cost of 32 line-of-sight traces at a tick boundary on a dedicated
  host.
- That a `Replica`'s VM and the host's stay equal through a join in
  progress in a synchronous session.
- The 50 units a second clamp and the show channel's 600 bytes a second:
  both are first guesses to set from a run and from probe P4.

## Related

- [rules-engine.md](rules-engine.md): the language, the VM, capabilities, the engine API and the authority model this page extends
- [rules-language-types.md](rules-language-types.md): front-end types, labels and message holes
- [rules-start-gate.md](rules-start-gate.md): the check a `path` label and a sound tag join
- [ai-networking.md](ai-networking.md): session models, the actor channel, the host log, budgets and probes
- [firefight.md](firefight.md): the director, the kit, the guest boundary and the HUD's mode blob
- [superbounce.md](superbounce.md): the touchdown and bounce rules, the scanner and the medals
- [hud-kit.md](hud-kit.md): the HUD model, the message and navpoint channels
- [bots.md](bots.md): phantoms, the planner and the route runner
- [havok-runtime.md](../engine/havok-runtime.md), [sim-rewrite-havok.md](../engine/sim-rewrite-havok.md): what of Havok is readable and why it is not written
- [film-format.md](film-format.md), [theater.md](theater.md): tagged records and what a film replays
