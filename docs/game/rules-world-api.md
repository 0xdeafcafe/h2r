# Rules world API: a synced world for scripts

Status: Design, with the first slices landing. Messages as cues (W1), the script HUD's declarations and show state (W2), objects in motion (W3), the physics reads, `on touchdown` and the sight table on the fake (W4, without `push` and `launch`; the host half and the Superbounce script wait), the `sound_to` audio entries (W5, partly), the bot orders (W7), vehicles (W8, on the fake with host hover and unload), the data reads with `on damage` (W9), modules (W10) and the System Link show carrier (W11, carrier only) are built; the rest extends the rules engine of [rules-engine.md](rules-engine.md) and is built after that page's host runtime runs in a game.
Code: slice W1: `crates/formats/rules/src/show.rs` (the cue and its codec), `crates/extensions/game/src/rules/world/show.rs` (the host and client halves), `crates/live/actor-net/src/wire.rs` and `budget.rs` (the `Cue` record and its three-packet repeat), `crates/extensions/game/src/actor_net/host.rs` and `client.rs` (offered to every peer; shown once). Slice W2: `crates/formats/rules/src/show/state.rs` (the show state and `view`), the `hud`, `waypoint` and `section` declarations of `crates/formats/rules/src/lang`, the `hud_*`, `mark`, `unmark`, `side_of` and `slot_of` entries of `caps.rs`, `RulesExt::show` in `vm.rs`, `crates/extensions/game/src/rules/world/hud.rs` (the feed of the HUD kit's objective HUDs, `crates/rewrites/hud-kit-core/src/views/objective.rs`). Slice W9: the entries `object_health` to `squad_strength` of `crates/formats/rules/src/caps.rs`, `on damage` (`ir::Event::Damage`), `crates/formats/rules/src/fields.rs` (the tag read's field paths and load check), `crates/extensions/game/src/rules/world/data.rs`. Slice W7: the `bot_*` entries of `crates/formats/rules/src/caps.rs`, the dispatch in `crates/extensions/game/src/rules/runtime.rs`, `crates/extensions/game/src/rules/world/bots.rs`, `crates/live/bots/src/brain.rs` and `decisions.rs`. The plan names `crates/formats/rules/src/caps.rs` (the entries), `crates/formats/rules/src/world/` and `crates/extensions/game/src/rules/world/` (one module per capability group; `bots` and `show` are built), and `crates/live/actor-net/src/wire.rs` (two records; `Cue` is built). Tests: named per slice in "Implementation plan".
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

## Decisions

Agreed by the user, under one rule: **secure and hardened, but performant
and clean**. Where two of those pull apart, a check moves to the
verifier or to load time, so that it costs nothing in a tick.

1. **A script never runs on a distributed client.** Its world reaches
   clients through the engine's own replication. What replication does
   not carry (HUD, messages, waypoints, sounds, music) travels on one
   host-to-client stream, the show channel, which is authenticated,
   bounded and validated on the client.
2. **A synchronous peer runs the script itself**, as it runs every other
   part of the simulation. No call log is sent, and the lockstep hash is
   the check.
3. **Places are labels first.** A script moves a thing to a labelled
   thing or along a labelled path. A `vec` exists for offsets,
   velocities and reads. Every handle is typed; no script-facing API
   takes a datum, an index into engine memory or a size.
4. **Physics is read, and written only as velocity** at a tick boundary
   through the engine's own entry, inside a stated bound. A script never
   writes Havok memory.
5. **Bots are ordered, not driven.** An order is a hint to a phantom's
   brain on the host; it returns nothing to the script.
6. **Splitting a script:** sections and an outline with the first slice
   that touches the editor; `use` modules later; no node editor.

Every slice of "Implementation plan" is held to
[rules-world-limits.md](rules-world-limits.md): its security rules,
its caps and its bench gates.

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
| a retail game event with its retail line and announcer sound | the game engine event (type 7) | **E** the type and the record's fields ([hud-kit.md](hud-kit.md#game-events-and-medals-0x0019de80)); the host posts a round-start event through `GAME_EVENT_POST` (`0x19eb90`) |
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
  four typed hole values, audience; **Code**, `rules::show::Cue`), a
  Firefight or Siege round-start line (the declared type byte), a sound
  (tag-table index, an entity or a point or an audience) and a
  medal line. A cue is not state: a machine that joins later does not
  get it. One cue is the exception, since it carries state: the walls
  cue (`rules::show::Cue::Walls`, kind 2) holds the whole list of areas
  the script shows (`show_area`; at most 8, each a netgame flag's index
  in 12 bits and a team in 4, 17 bytes at most). The authority sends it
  when the list changes, an emptied list included, and again every 90
  ticks while the list is not empty, so a machine that joins late has it
  within a second or two. A client that gets it reads each zone's shape
  from its own copy of the map (`rules/world/walls.rs`, `client`),
  draws the walls through the markers' world-line source and takes an
  older or repeated cue for nothing
  (`crates/extensions/game/src/rules/world/show.rs`, `present_walls`).

An audience is a 16-bit mask of player slots. A text is an index into
the blob's string table and a sound an index into its tag table, so a
record names only what the verified blob holds; a `Viewer` has the blob
from the lobby fetch.

The actor channel (`crates/live/actor-net`, ai-networking.md "How it
works") carries both, with two more record tags:

| Record (tag) | When | Payload |
|---|---|---|
| `Show` (7) | the state a peer's players are the audience of changed, at most 5 times a second; whole in every `Key` | length (u8), the state filtered to that peer's players |
| `Cue` (8) (**Code**, `crates/live/actor-net/src/wire.rs`) | a cue for that peer's players, in three consecutive packets as `Line` is (**Code**, `crates/live/actor-net/src/budget.rs`) | cue tick (u32), sequence (u8), kind (u8), length (u8), at most 24 payload bytes |

The `Show` record is not built (it needs `crates/live/actor-net`); the
state it would carry is built (`rules::show::State`, at most 244 bytes), and
every machine that runs the VM draws it. Nor is the 64-bit hash of the
peer's filtered state the `Digest` gains, whose mismatch asks for a
`Key`. The host filters the state per peer, so a team's waypoint is not
sent to the other team's machines. A message cue carries its audience as a mask
of player slots and goes to every peer; each peer shows it to its own
players in the audience and to no one else (**Code**,
`crates/extensions/game/src/rules/world/show.rs`).

A round-start clip goes to every peer. The viewer accepts only a type
with a custom start line that matches its running script or Firefight
director. The one-byte payload selects a local clip (`rules::show::Cue`).

**Authenticated** (not built; the records ride unauthenticated until it
is). The tunnel and XNet's UDP mapping give no transport
security of their own (**Code**, [dedicated-host.md](../live/dedicated-host.md#routing-assignment-and-arbitration)),
so the channel carries its own:

- The host makes an Ed25519 key pair for the session and puts the public
  key on its Live session beside `rules` (`NewSession.show_key`), which
  a guest reads over its authenticated Live connection.
- A peer's first channel packet is a hello with an X25519 public key.
  The host answers with its own, signed by the session key. Both derive
  one key for that peer (HKDF-SHA-256 over the shared secret, the
  session id and the peer id).
- Every packet that holds a `Show` or `Cue` record ends with an 8-byte
  HMAC-SHA-256 tag over the whole packet. A packet's tick orders it, and
  an older or repeated tick is dropped, so a recorded packet cannot be
  replayed.

One signature per peer per session and one HMAC per packet keep the cost
off the tick. Another guest cannot forge a record, since
it holds neither the host's key nor the peer's. The tag proves who sent a
record, never that the record is right: a `Viewer` validates every field
([rules-world-limits.md](rules-world-limits.md#the-show-channel-on-a-client)).

**Batched.** A peer gets at most one show packet a tick, and none when
nothing is due. A tick's cues share one packet with any `Show` and any
actor records due, under one header (and one tag once the channel is
authenticated); the two repeat copies of a cue ride the next two packets
that are sent anyway (**Code**, `crates/live/actor-net/src/budget.rs`).
`Show` is coalesced to the last state, at most 5 times a second. A
`Viewer` applies records before the
game tick and draws through `rules::show::view`, a pure function of the
state bytes, the blob's tables and the local roster slot, the same
function the `Authority` draws its own HUD with (the pattern of
`crates/extensions/game/src/firefight/hud.rs`). Repeated cue copies are
dropped by (tick, sequence) (**Code**,
`crates/extensions/game/src/actor_net/client.rs`).

**System Link.** The actor channel rides the Live tunnel, and a System
Link session has none (ai-networking.md "Decisions"). The show channel
gets a LAN carrier of datagrams of ours (slice W11): the `show-lan` crate
(**Code**, `crates/platform/show-lan`). It has the Live channel's
handshake and tag, over a plain datagram socket the caller owns:

- A client sends a hello (its X25519 key and a nonce); the host answers
  with its own key and the peer id, signed by the session's Ed25519 key.
  Both derive one key (HKDF-SHA-256 over the shared secret, the session id
  and the peer id). Only the answer to the latest hello is taken.
- Every data datagram carries one actor-channel packet and an 8-byte
  HMAC-SHA-256 tag; a tick not newer than the last is dropped, so a
  recorded datagram cannot be replayed. The host reads nothing but a hello.
- With no Live session to vouch for it, the client is *given* the host's
  public key by its caller and trusts it for that session only; the carrier
  never learns a key from the wire.
- A host answers 15 peers. A client counts each dropped datagram by reason
  and closes the channel at 30 drops in a second.

`Host::peers` and `Host::send` have the shape of the Live tunnel's
`actor_tunnel_peers` and `send_actor_tunnel_packet`, so the per-peer
`actor_net::Budget` and the batching above feed either carrier.

Not built, and each named: which addresses are the game's peers and how a
guest finds the host (research row R10: whether the HLE's sockets carry a
datagram of ours is not settled, so the carrier takes addresses and a
socket from its caller and opens none); where the discovery reply carries
the host's key (it is not known to have room, "Not verified"); the
lobby's status line for a script that declares a show capability. Until the
carrier is wired, a script that declares one plays on System Link with its
world synced and its HUD, messages and sounds on the host only, and the
lobby's status line says so by name; nothing is faked on the guest.

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
  runtime in playback (**Code**, `GuestGame`'s `film`). This is safe
  because an order returns nothing.

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

## The world API

Every entry is a row of `caps::ENTRIES` with its capability, kind, order
and status, appended to the table (nothing is published, so the API
version stays 1). Reads need no capability. Every action here is
`Order::Queued`: made by the `before` on the next `GAME_TICK`, in
program order, returning nothing.

**Code layout.** One module per capability group, on both sides: the
entries, signatures and fake of a group in
`crates/formats/rules/src/world/<group>.rs`, and its guest half in
`crates/extensions/game/src/rules/world/<group>.rs`, for `objects`,
`vehicles`, `squads`, `bots`, `audio`, `hud`, `physics`, `data` and
`show`. The host's zone test (`world/zones.rs`, the labelled flags as
cylinders and hill polygons, behind `inside`, `ticks_held`, `on enter`
and `on leave`), `set_loadout` (`world/loadout.rs`), the spawn rules
(`world/spawns.rs`) and the bodies (`world/body.rs`) are guest halves of the core entries, with no group
of their own
([rules-engine.md](rules-engine.md#the-host-runtime)).
`rules::vm::Game` gains one sub-trait per group, so a fake or a
host implements a group whole or not at all. The traits take typed
handles (`ObjectRef`, `PlayerSlot`, `SquadSlot`, `VehicleRef`,
`WidgetId`, `WaypointId`, `TagRef`, `Vec3Fixed`), never a `u32`; a
handle becomes a salted datum in one place,
`crates/extensions/game/src/rules/guest.rs`, which checks it is live
first. Each engine routine an entry reaches is one named point in
`crates/hooking/guest-layout/src/points.rs` with its signature; a
`world/` module holds no address, and an entry is **named**, and so
hostable, only once its point exists.

### Types

All are front-end types, erased before the IR as `zone` and `time` are
([rules-language-types.md](rules-language-types.md#front-end-types)):

| Type | What it is | In the IR |
|---|---|---|
| `vec` | three `fixed`: `vec(x, y, z)`, `.x`, `.y`, `.z`, `+`, `-`, `v * k` (`k` a `fixed`; a bare literal may lead), `length(v)`, `distance(a, b)`. World units in 16.16, so a coordinate is within ±32,768. It is the value of an expression or a `let`; it is not stored in a variable, option, table, parameter or result, and it has no `==`, order or `/` (the checker says so by name) | three `Fixed` registers kept together (`lang/lower.rs`, `vec_expr`); a `vec` read is three calls with an axis argument; `length` is the VM's `vec_length`, taken whole in 128 bits so no square overflows |
| `vehicle` | an object whose tag is a vehicle; `as_vehicle(o)` gives a `vehicle?`. Not built: the vehicle entries take an `object` and the game refuses a call on one that is no vehicle ("Vehicles and drop ships") | `Object` |
| `sound` | a tag path that must be a sound's, as `weapon` is a weapon's | `Tag`, with the tag table's group |
| `line` | a dialogue vocalization by name, checked at load against the map's dialogue names | `Text` |
| label kind `path` | an ordered set of at least two labelled points, numbered from 0 with no gap and none twice; the start gate checks it (`labels::check`) and only `fly` takes it | `LabelKind::Path` in the label table, with the `numbered` flag |
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
| `o.position`, `o.velocity` (`vec`), `o.speed` (`fixed`) | `object_pos(Object, Int) -> Fixed`, `object_vel(Object, Int) -> Fixed`, reads; `o.speed` is `vec_length` of the velocity | the object datum's position and velocity, the root ancestor's for a child (`rules/world/objects.rs`; the engine's own are `OBJECT_WORLD_POSITION` `0xb9dd0`, `OBJECT_ROOT_VELOCITY` `0xba1d0`) | named (**Code**) | a read on the machine that runs the VM |
| `teleport(o, at)`, `teleport(o, at, offset)` | `teleport(Object, Fixed, Fixed, Fixed)`, queued: `at` is a `vec`, or an object, whose position the compiler reads and adds the offset to | `OBJECT_SET_TRANSFORM` (`0xb7430`) with the object's own forward and up kept, `al` false when the position is outside the world, which is a refused call, recorded, that moves nothing (`rules/world/objects.rs`) | named (**Code**); **S** that it marks position (R2) | position flag |
| `face(o, at)` | | | refused by name: whether the facing flags carry a host-set facing to clients is R2 | |
| `teleport(p, at)` | `teleport_player` | HaloScript `object_teleport`'s body `0x29ffb0` through `PLAYER_PLACE_NEAR` (`0x14e970`), which takes the unit out of a vehicle first (**Code**, `crates/extensions/agent/src/sim/calls.rs`, `crates/extensions/game/src/coop_players.rs`) | refused by name: the checker says a player is not moved, citing R9 | position flag; **S** how a client's own predicted biped takes it |
| `move_to(o, at, t)`, `stop(o)`; `on arrive(o)` | `move_to(Object, Fixed, Fixed, Fixed, Int)` and `stop(Object)`, now; the VM's own event | a **mover**: a `RulesExt` record (object, from, to, start tick, duration; `vm::Mover`, at most 16) the VM steps every tick in the order started, telling the game the place on the straight line from where the object was when `move_to` was called, and `to` exactly on the end tick, when `on arrive(o)` runs. `t` of 0 or less is one tick. A second `move_to` for an object replaces its mover with no `on arrive`; `stop` ends it with none; a seventeenth mover is a script error; the round's end clears them. The host puts the object at each step (`OBJECT_SET_TRANSFORM`) at the next boundary; the velocity of a step is not pushed (`OBJECT_SET_VELOCITY` needs two vectors in guest memory, and what marks the entity is R2) | named (**Code**); **S** that a client sees the move (R2, R3) | position flag |
| `move_along(o, path, t)` | | | refused by name: it needs the `path` label kind, which W8 builds; `move_to` takes one point | |
| `attach(o, to)`, `detach(o)` | `attach`, `detach` | no entry read; the object datum has a parent field, and HaloScript's `objects_attach` evaluator is not traced | untraced | nothing generic: only a unit's parent vehicle replicates (**E**). Until traced, `move_to` with the parent's `at` each tick is the script's follow |
| `set_power(o, f)`, `set_position(o, f)` on a device | `device_power`, `device_position` | Forge's device sinks ([forge-features.md](forge-features.md#teleporters-buttons-and-triggers)); the engine setters are not traced | untraced | the device entity (type 16, **E**) |

`create` and `delete` are the existing entries. A created object has no
handle the script can hold, since a queued call returns nothing; a
script that must move what it makes labels a placement and moves that.

### Vehicles and drop ships

Capability `vehicles.drive` ("flies and loads vehicles").

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `vehicle_riders(v)` (`int`), `vehicle_driver(v)` (`player?`) | reads | the unit's seat iterator (`UNIT_SEATS_BEGIN` and `UNIT_SEATS_NEXT`) | host reads pending | a read |
| `fly(v, path, t)` | `fly` | a notified transform reaches eligible Havok bodies, which retain velocity | not built | the vehicle entity (type 12) |
| `hover(v, on)` | `hover` | `VEHICLE_HOVER` (`0xf0fd0`) toggles the hold bit and refreshes the physics component on a change | named | vehicle replication unverified |
| `seat(p, v)`, `eject(p)` | `seat`, `eject` | the vehicle entry routine `0x11ade0` behind `unit_enter_vehicle` and the biped exit action at `0xe68c0` | traced | events 24 and 11 |
| `place_squad_in(s, class, n, v)` | `place_squad_in` | `AI_SQUAD_PLACE` selects by the scenario's vehicle palette and can leave actors unseated | refused | unit entities with the parent-vehicle flag |
| `unload(v)` | `unload` | `VEHICLE_UNLOAD` (`0x11b2b0`) requests exit from occupied seats with the all-seats mapping | named | event 11 |
| `on board(p, v)`, `on leave_vehicle(p, v)` | events | observers on the enter and exit events the engine queues (`SIM_EVENT_GENERATE` `0xb5a70`, types 24 and 11) | named | read on the `Authority` |

The host queues `hover` and `unload` for the tick boundary. It resolves a
labelled vehicle placement to a live datum and rechecks the datum before
calling retail. The fake also implements the entries below. A host
refuses scripts using them at start. [Retail traces for R6 and R7](notes/2026-11-24-vehicle-actions.md).

| Entry | Held back by |
|---|---|
| `fly` | W3's mover and verification of the path after physics steps with riders and contacts |
| `place_squad_in` | needs a seat-limited spawn into the supplied vehicle. Retail placement chooses or creates its own seat vehicle. |
| `seat`, `eject`, `vehicle_riders`, `vehicle_driver` | host seat selection, player actions and synchronous seat reads |
| the kit's ship and paths | R8: the kit's stage 2 (`crates/tools/firefight-kit`, FF12) is not built, so no map carries a ship or a labelled path |

Four choices of the fake are the API's contract, and a host that gives
different answers is wrong until a run says the engine does:

- A **vehicle** is an `object` the game checks at the call. `vehicle` is
  not a front-end type, and there is no `as_vehicle`, `v.driver` or
  `v.riders` (the entries above read them): a type would add no safety,
  since a call on a crate is a refused call either way, and it would
  edit `lang/check.rs` in the same waves as W2, W3 and W5.
- A **refused call** changes nothing and is recorded (the fake's
  `refused`): a vehicle entry on an object that is no vehicle, a seat
  with none free, a dead player, a flight of no ticks or over a path with
  a point not placed.
- **Seats** fill driver first. A player who boards another vehicle leaves
  the first, and one who dies or quits leaves at once.
  `place_squad_in` seats `min(n, free seats)` actors and is refused when
  none fit; `unload` takes everyone out, players and actors.
- A **flight** is `vehicles::Flight`: one speed along the polyline of the
  path's points in number order, so the vehicle is at the last point on
  the tick `start + t` and covers equal lengths in equal ticks; its
  facing is the unit tangent of its segment (a corner belongs to the
  second). The math is 16.16 integers, so every machine and a film agree.
  The fake reports each flight's end once (`take_arrived`); `on arrive`
  is W3's event, and until it lands a script times the end itself
  (`now()` against the tick it started on, as `tests/vehicles.rs` does).

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
| a map's own squads, by name: `label wardens: squad = "<squad name>"` | a label kind `squad` | the scenario's squad block (`scnr+0x160`), read by name as the kit's slots are | not built; see "Advanced data" |

### Bots

Capability `bots.orders` ("gives bots orders"). A bot is a phantom: a
player seat driven by a brain on the host ([bots.md](bots.md) section
5). To a script it is a `player`; a `p.is_bot` read of the phantom
registry (`bots::phantom::list`) is design, not built.

**Code** (slice W7). An order is a `bots::brain::Order` — `Go`, `Hold`,
`Follow` or `Attack` — kept on the brain's decisions
(`crates/live/bots/src/decisions.rs`). While one stands it is the bot's
goal in place of the objective's (`crates/live/bots/src/mind/strategy.rs`):
`Go` and `Hold` walk the mind's own nav follower, `Follow` and `Attack`
are its `Escort` and `Hunt` goals with the target's position from the
roster each tick. The order survives a death; only `bot_free` clears it.
The call is queued like every world action and joins the tick boundary
as a `rules::runtime::Call::BotOrder`, whose async body only the machine
`GAME_ENGINE_AUTHORITATIVE` answers true for runs. There it delivers
through `bots::phantom::order`; an order to a seat no joined phantom
holds is a refused call, recorded. A run playing a film back makes none
(`GuestGame`'s `film`), as "Films and Theater" says. The game side is
`crates/extensions/game/src/rules/world/bots.rs`: it resolves `bot_go`'s
and `bot_hold`'s zone through the census's labelled things and the
scenario's netgame flags: the centre is the mean of the zone's members'
positions, the reach the farthest member's distance from it and at
least 4 world units (`bots::objective::DEFEND_RADIUS`) while no zone's
capture volume is read.

| Script | Entry | Backed by | Status |
|---|---|---|---|
| `bot_go(p, at)`, `bot_hold(p, zone)` | `bot_go`, `bot_hold` | the order as the goal (`Go`, `Hold`), walked by the mind's nav follower over the map's nav mesh (`crates/live/bots/src/mind.rs`) | named |
| `bot_follow(p, q)`, `bot_attack(p, q)` | `bot_follow`, `bot_attack` | the brain's own escort and target choice (`crates/live/bots/src/decisions.rs`), with the order as its goal (`Escort`, `Hunt`) | named |
| `bot_free(p)` | `bot_free` | clears the order | named |

An order returns nothing. Whether the bot got there is a world read
(`inside(p, zone)`, `distance`), so a script's state never depends on a
brain, which runs off the game thread (bots.md section 5.5). The effect
is the phantom's pad, which every session kind already carries as a
player's input. Seats are added in the lobby (bots.md section 7); a
script adds and removes none.

### Zones

No capability: core entries with a host half but no group of their own
(`world/zones.rs`). A zone is a labelled netgame flag — the host's census
has no Forge document at a game's start, so no zone is a trigger volume.
A zone's shape is an area round its flag
([gametype-markers.md](gametype-markers.md), "In a script"), from the
first of three that applies: the flag's own area, stored in the Forge
document of a map variant and carried by the census (`Thing::area`), a
radius or a path at its own heights; else the flags that share a label
and a number, three or more of them (a King of the Hill hill's corners),
as one zone, `Area::of_corners`: the convex hull of their positions
across the floor; else `Area::default` around the flag, an upright
cylinder. `rules.zones` reports each zone's source (`area`, `corners`
or `default`). The default's sizes are ours ("Not verified"). A body is a living
player's unit; the event order and the 64 zone events a tick are the
host runtime's ([rules-engine.md](rules-engine.md#the-host-runtime)).

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `inside(p, zone)` | `inside`, a read | the zone test above, against the living player units' positions | named | a read on the machine that runs the VM |
| `player_of(o)` | `player_of`, a read | the roster's units (the player datum's unit), so no unit field is read | named | a read |
| `ticks_held(zone, team)` | `ticks_held`, a VM read | the VM's own counters over `Game::zone_holder`: the one team whose living players alone are inside, `none` when the zone is empty or contested | named | the state blob |
| `on enter(zone, o)`, `on leave(zone, o)`; `on spawn(p)`, `on death(p)` | VM events | the roster's and the zones' changes since the last tick (`world::zones::events`), in the order the host runtime tells them | named | the state blob |
| `show_area(zone, team)`, `hide_area(zone)` | `show_area`, `hide_area` | no engine call: the zone is kept in the VM's state (`RulesExt::shown`, at most eight, in the order first shown; a ninth is a script error; `none` for the team is neutral; the round's end clears them), and the runtime hands the zones' shapes to the world-line source after each tick (`world/walls.rs`), which draws each wall alone in the team's colour ([gametype-markers.md](gametype-markers.md), "When a marker shows"). No capability: it is a display | named | the state blob; a machine that runs the VM draws it, a film's re-run included. A client that does not run the script draws it from a walls cue ([the show channel](#the-show-channel)) |

### Loadouts

Capability `player.weapons`, as `give` and `take`. A script names a
weapon by its tag path, but the host gives only the multiplayer weapon
list's (`rules::weapons`, the starting-weapon values the variant editor
offers); a path off the list is refused by name, and nothing named gives
nothing.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `set_loadout(p, primary, secondary)` | `set_loadout` | `STARTING_WEAPON_PICK` (`0x19eda0`) answers the value's tag from the map's multiplayer globals, as the spawn's weapon pass asks it; the weapon is made and added as a starting profile does (`placement_data_new`, `object_new`, `unit_add_weapon`, and `object_delete` when the unit refuses it) — the primary in mode 2, which empties the unit's weapon slots first and readies it, the secondary in mode 1 beside it (mode 2 when there is no primary) | named | the unit's weapon entities |

### Spawns

Capability `player.spawn`. A team's spawn rule is in the VM's state
until the round's end; the host reads it at each spawn, so neither call
waits for a tick boundary.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `spawn_areas(team, label, n)` | `spawn_areas` | `SPAWN_LOCATION_FREE` (`0x23bb70`), the free-spot test of `PLAYER_SPAWN_LOCATION` (`0x14ef00`), answers "taken" for each starting location not labelled `label` and numbered `n`; the chooser's own weights pick among the rest | named | the spawned unit |
| `spawn_near(team, zone, n)` | `spawn_near` | as above, keeping the `n` starting locations nearest the zone's middle; `none` or 0 lifts the rule | named | the spawned unit |

### Bodies

Capability `player.disguise`. A team's body is in the VM's state until
the round's end, and the host writes it at each spawn of the team's
players.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `set_body(team, tag)` | `set_body` | the model byte (player `+0x88`) is the body's (0 `masterchief_mp`, 1 `elite_mp`) while `PLAYER_SPAWN` (`0x14f2e0`) runs, and the player's own after it; the representation pick `0x14f040` reads it for the unit type (`rules/world/body.rs`). `none` lifts it; another tag is a script error | named | the spawned unit; **S** a client's first-person hands |

### Audio

Capability `audio.play` ("plays sounds, dialogue and music").

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `sound_to(s, p)`, `sound_to_team(s, t)`, `sound_to_all(s)`: a sound in the ears of a player, a team or everyone | `sound_to`, `sound_to_team`, `sound_to_all` | the game's own sound start at each audience player's living unit, at its head marker: `SOUND_START_AT_MARKER` (`0x1891d0`), the call a replicated dialogue line and `audio.play` make (**Code**, `crates/extensions/game/src/rules/world/audio.rs`, `crates/extensions/game/src/actor_net/client/guest.rs`). The one-shot allocator (`SOUND_IMPULSE_ALLOCATE`, `0x126000`) is the game's own behind it; `sound_impulse_start`'s body (`0x2a9e00`) is not used | named | a sound cue (`rules::show::SoundCue`, kind 2) with the audience mask and the sound's index in the blob's tag table, which a `Viewer` checks is a sound-group entry before it plays it at its own players |
| `say(u, line)`: a unit speaks a line of its own voice | `say` | the unit's voice bank resolves the vocalization (`0x1144a0`), `SOUND_START_AT_MARKER`, `BIPED_DIALOGUE_LIP_SYNC` (`0x109220`): the path a client plays a replicated line with (**Code**, `actor_net/client.rs`) | not built: no table of a map's dialogue names gives the `line` type its checked index | the actor channel's `Line` record, which exists |
| `sound_at(s, o)`: a sound at an object or a point | `sound_at` | `SOUND_IMPULSE_ALLOCATE` admits a one-shot; the scripts' `sound_impulse_start` evaluator is `0x2a9e00`; its body is not read (R4) | traced | a `Cue` with the object's entity or the point |
| `music(s)`, `music_stop()` | `music`, `music_stop` | a looping sound's start and stop: not traced (R5) | untraced | the show state's music slot, so a late machine starts it at the right offset |
| `announce(event, p)`: a retail game event, with its retail line and announcer sound | `announce` | the game-engine event record and its responses (`0x19de80`, `0x19e890`, `0x19e770`, hud-kit.md); how a host raises one with a player is R5. The first argument is the retail event's kind number until R5 names them | traced | the game engine event (type 7), on System Link too |

A sound is a tag the loaded map holds, checked like any tag path: a `sound`
is a string interned with the sound group (`TagGroup::Sound`), and the
start gate blocks a map whose tag index lacks that path by name
(`rules::labels::check`, `Census::sounds`). A census that does not read tag
paths (the running game's) checks none. A script brings no audio of its own.

The three `sound_to*` entries are spelled by name, not as one `sound`
overloaded on its audience as `message` is. A call through `none` plays
nothing, and a team with no seated players is an empty audience. The call is
queued and made at the next tick boundary; on the authority the boundary
sends the cue and plays the sound at the host's own players, and a machine
that is not the authority makes neither. The running game holds no tag paths, so
the host finds a sound by the loaded map's file: at each game start
`world::audio::activate` reads the file through the VFS (the file the
start gate's census reads) and installs a lookup from each `snd!` tag's
path to its datum, the datum the game loaded the tag under
(`world::audio::set_resolver`). A host with no lookup, or a loaded map that
holds no sound tag at a path the script plays, refuses the script at the
start, by the sound's path. A sound of a shared map (`shared.map`) is not
in the loaded map's file and is refused the same way. A host that runs a
film makes the same sounds.

A script that names `say`, `sound_at`, `music`, `music_stop` or `announce`
is refused at the start by the entry's name: R4 and R5 have not settled
what each does on the host. `sound_at` waits on R4: the body of
`sound_impulse_start` (`0x2a9e00`) is not read, so a positioned one-shot has
no traced call to make.

### HUD

Capabilities `hud.message` (exists), `hud.widgets` ("draws on the HUD")
and `hud.waypoints` ("marks places and things").

A script declares its HUD once, as the HUD kit describes one
([hud-kit.md](hud-kit.md#the-model-cratesrewriteshud-kit-core)): a fixed
list of widgets and waypoints, each a kind and a place. A state only
shows, hides and words them.

```text
hud hill_row: text at koth.hill
hud moved_row: banner at koth.banner
waypoint hill_mark at koth.mark
```

A place is a kit element's id: the HUD's name, a dot and the element
(`koth.hill`), so a script gives no pixels. A declaration's name is its
number, an `int` constant, which the calls below take. The kind is
`text`, `banner` or `meter`. At most 16 widgets and 8 waypoints; a place
taken twice is a compile error (`Refusal::HudSlot` in the verifier), and
a declaration without its capability is one too. The slot's HUD is the
one the game feeds from the state ([the HUDs a script feeds](#the-huds-a-script-feeds)).
The text's width against the element's is not checked.

A `section "Waves"` line splits a script: it compiles to nothing, the
outline nests the declarations after it under it (`ide::symbols`) and
editors fold it (`ide::fold`).

The state a widget keeps is its value, fill (0 to 255 for 0 to 1), style,
text (an index into the blob's strings) and audience, set by the calls
below; a waypoint keeps the object it marks; one widget keeps a value
for each roster slot (`hud_value_for`). `rules::show::view` makes
what one viewer sees of it, a pure function of the state, the blob and
the viewer's slot and team, and nothing in it reads the game. The state
is part of `RulesExt` (`show`), so a checkpoint, a film's keyframe and the
sim hash carry it, and a round's end clears it. A call that names a
widget or waypoint the script does not declare stops the script
(`ErrorKind::Hud`).

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `message(to, "…{n}")` | exist | the notification insert (`hud_kit::NOTIFY`, E `0x24cbee`), per local user (**Code**, `crates/extensions/game/src/rules/world/show.rs`); the timed centred text (`0x24cc73`) is not used | named | a `Cue` (**Code**) |
| `hud_text(w, "…")`, `hud_value(w, n)`, `hud_fill(w, f)`, `hud_style(w, style)`, `hud_show(w, on)` | `hud_text`, `hud_value`, `hud_fill`, `hud_style`, `hud_show` | the VM's show state; `hud_kit::Handle::publish` draws it (**Code**, `crates/extensions/game/src/rules/world/hud.rs`). The text is a literal: message holes and an audience (`to`) are not built | named | `Show` (not built) |
| `hud_value_for(w, p, n)` | `hud_value_for` | the widget's value for the player in roster slot `p` (0 to 255), kept in the state's per-viewer table (`rules::show::PerViewer`: the widget's number and 16 bytes). `rules::show::view` gives each viewer the entry of its own slot in place of the widget's `value`; the fill, style and text stay shared. One widget at a time reads per viewer, and a second is a script error (`ErrorKind::Hud`); a `none` player sets nothing | named | `Show` (not built) |
| `side_of(p)`, `slot_of(p)` | `side_of`, `slot_of` | the roster: the team's index in a game with teams, 16 and the slot in one without; the slot | named | none: a read |
| `countdown(w, t)` | `hud_countdown` | the state holds the end tick; each machine counts its own ticks | not built | `Show` |
| `mark(wp, at)`, `unmark(wp)` | `mark`, `unmark` | the object's place through `Handle::navpoint_at`, one navpoint a HUD; the text is the distance from the user's unit. The place is a zone's middle, else the unit of the player who carries the objective (the objective registry, `objectives::carrier`), else the object's origin (`Object::POSITION`, through `engine::view::objects::get`): `hud::place`. A mark is the same for every viewer, and the navpoint is the first local user's, as the kit's navpoints are shared. `mark(wp, at, to)` and the text form are not built | named | `Show` (not built) |
| `medal(p, "…")`: a line with a medal glyph, counted in the film and on Live | `medal` | the notification queue, as the superbounce medals (hud-kit.md "The medals"); `GAME_MEDAL` (`0x1970a0`) takes only the game's own medal rows | traced | a `Cue` |

Scoreboard labels and the post-game report's columns stay the carrier's
(rules-engine.md "Not verified").

#### The HUDs a script feeds

The first word of a slot names a HUD of the kit's: `ctf`, `assault`,
`oddball`, `koth` or `territories`
(`hud_kit_core::views::objective`, `content/modes/huds/`). The rules
feature registers them hidden when it installs
(`crates/extensions/game/src/rules/world/hud.rs`, `register`), and after
each tick, for each local user, the game makes the user's view of the
state, turns it into the objective views' `Fed` (`hud::fed`) and
publishes `objective::feed`'s values for each HUD the script's slots
name. A row the script leaves unset or hidden stays hidden. A side is a
team's index in a game with teams and 16 and a roster slot in one
without (`side_of`); a viewer is on a side (`Viewer::on_side`), so one
state reads "yours" to one team and "the enemy's" to the other. Each
HUD's contract with the script:

| HUD | Widget (slot) | Reads |
|---|---|---|
| `koth` | `koth.hill`; `koth.time`; `koth.banner`; waypoint `koth.mark` | style 0 open, 1 held by the side in the value, 2 contested; the seconds in the value; the banner's text; the navpoint over a zone, with the distance |
| `territories` | `ter.owned`; `ter.banner`; waypoint `ter.mark` | the count in the value and the total in the style (the viewer's own, by `hud_value_for`: the territories its side holds, a team's in a game with teams and the player's own without); the banner's text; a navpoint over a zone |
| `oddball` | `odd.holder`; `odd.time`; `odd.banner` | style 0 loose, 1 carried, the value 16 and the carrier's slot (`slot_of`), worded with the player's name; the seconds in the value; the banner's text; the navpoint (`odd.mark`) over the ball a player holds, else the first one lying, with the distance |
| `ctf` | `ctf.flag0`, `ctf.flag1`; `ctf.banner` | each team's flag: style 0 at home, 1 carried (the value 16 and the carrier's slot), 2 dropped; the viewer's team's is "your flag"; the navpoint (`ctf.mark`) over the first flag away from its spawn |
| `assault` | `asl.bomb0`, `asl.bomb1`; `asl.banner` | each team's bomb as a flag, and style 3 armed with the seconds to detonation in the value; the viewer's team's is shown; the navpoint (`asl.mark`) over the first bomb away from its spawn |

A banner is timed by the kit from the moment its text changes
(`BANNER_TICKS`), so a script shows it for two ticks and hides it. The
`base` HUD is not fed from a script; its feed is slice GE5 of
[game-types-without-carriers.md](game-types-without-carriers.md).

### Physics

Halo 2's physics is Havok at an exact 30 Hz tick
([havok-runtime.md](../engine/havok-runtime.md),
[high-framerate.md](high-framerate.md)). Reads need no capability.
Writes need `physics.push` ("pushes objects") or `player.push` ("pushes
players").

| Script | Entry | Engine | Status |
|---|---|---|---|
| `o.velocity`, `o.speed` | as "Objects" | the object datum | named |
| `p.airborne`, `p.crouching` | `airborne`, `crouching`, reads of a player | the biped's airborne bit and crouch state the superbounce tracker reads (`crates/extensions/game/src/superbounce_charge.rs`; `BIPED_AIRBORNE_TEST` `0xe4050`) | named; the rules half is built on `FakeGame`, the host half is not, so a host refuses a script that reads either |
| `on touchdown(p, down, up)`: a landing, with the fall speed before and the vertical speed after, `fixed` world units a second (`down` positive for a fall, `up` positive for rising) | `ir::Event::Touchdown`, `Input::Touchdown`, at most 16 a tick | the touchdown rule of `superbounce_charge::touchdown`, run for every player ([superbounce.md](superbounce.md#the-rule)) | named; built on the fake, which lands the player; the host does not deliver it |
| `p.primed`: the player's level pair is in the state a landing bounces from | `primed`, a read of a player | the pair word of the biped's one pair against the level (`live_telemetry::havok_agents::fixed_pair`, **Code**) | named; its meaning is **Trace** in superbounce.md; host half not built |
| `sees(a, b)`: nothing solid between two objects | `sees`, a VM read of the sight table (`RulesExt::sight`) | `AI_LINE_OF_SIGHT` (`0x29dd50`) over the collision query `0x1697c0`, made in the tick boundary's async body as Firefight's sight pass is (`firefight::guest::line_of_sight`), through `Game::trace_sight` | named; the VM half is built on `FakeGame`, a host refuses a script that calls it. A player's unit is not nameable as an object yet, so it takes two objects; a point or a player waits on an entry for a unit |
| `touching(a, b)`, a contact's normal and impulse | reads | Havok's contact constraints (havok-runtime.md "Contacts"), read statically and never in a run | traced |
| `set_velocity(o, v)`, `push(o, v)`; `launch(p, v)` | `set_velocity`, `push`; `launch` | `OBJECT_SET_VELOCITY` (`0xb7740`), `quiet` clear | named; **S** for a client's own predicted biped |

`sees` cannot make a guest call from a handler, so it reads a **sight
table** in `RulesExt` (`vm::Sight`, at most 32 pairs, a 33rd is a script
error, `ErrorKind::Sight`): a pair asked for in one tick is traced at
that tick's end (`Vm::update`, after `on tick`, through
`Game::trace_sight`) and answers from the next tick on, false until its
first trace, and every kept pair is traced again at each tick's end. A
pair is ordered, `sees(a, b)` from `a` to `b`, and not asked for in 30
ticks (`SIGHT_KEEP`) is dropped. The table is in the state's bytes, so a
checkpoint, a film's keyframe and the sim hash carry it, and the round's
end clears it. Firefight's `zone_seen` and
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
events, script to physics as a bounded velocity. A superbounce stays
observed, never made.

### Advanced data

Reads and one event, no capability. Slice W9.

| Script | Entry | From | Status |
|---|---|---|---|
| `o.health`, `o.shield` (`fixed`, 0 to 1) | `object_health`, `object_shield`, reads | the object's vitality fields (`guest-layout` `object.rs`), through `world::data::vitality` (**Code**, `crates/extensions/game/src/rules/world/data.rs`); an overshield reads 1, an object that is gone reads 0 | named |
| `o.tag`, `p.weapon` (`weapon?`) | none | a tag value is a table index of the script's own blob, and a map's tag is not in it; the held weapon needs the same lookup. No entry names either, so a script that writes them is a compile error | not built |
| `tag_int(t, "field")`, `tag_fixed(t, "field")` | `tag_int`, `tag_fixed`, reads | both arguments are constants written in the call (the verifier's `field_reads` holds each to the two `Const` ops before it). The field is a path of our own names (`vitals.maximum_shield`), resolved at load against the typed tag definitions (`tags::defs`) to an offset and a type by `world::data::DefsSchema`, and refused by name by `rules::fields::check` when they lack it. The game holds no tag paths, so `world::map_tags::activate` reads the loaded map's tag index through the VFS at `GAME_ACTIVATE` (**Code**, `crates/extensions/game/src/rules/world/map_tags.rs`; with `map_areas::activate`, from `EditorSources::activated`): each tag's path, group and datum. `Runtime::start` calls `rules::fields::check` after the label check, with `DefsSchema` over that index, and blocks the game on each problem (a tag the map lacks, a group or field the definitions lack, a float read by `tag_int`; one problem when the index could not be read). `Runtime::swap` and a film follow the label check's rules. The VM's read (`map_tags::read`) finds the tag's entry by its datum (`engine::tags::tag_entry_checked`, group and salt must match), reads the field's bytes at its defs offset from the entry's data address, and answers 0 for a tag the guest has not loaded | built; the rules half is also tested on `FakeGame` |
| `squad_alive(s)` | exists | `actor_net::squads::count_by_squad` | named |
| `squad_fighting(s)`, `squad_strength(s)` (`fixed`, 0 to 1) | reads | actor state beyond what the agent API's `world.ai` snapshot reads (`crates/extensions/agent/src/ai.rs`); no engine field is traced | untraced: the fake answers, a host refuses a script that reads either |
| `on actor_kill`, `on actor_assist`, `on squad_dead` | exist | `SimEvent::ActorKilled` (`crates/rewrites/sim-objects/src/events.rs`) | named |
| `on damage(o, by, amount)` | an event | `SimEvent::Damage`, the same stream: `o` the object, `by` the player it is credited to (`none` for none), `amount` the body and shield damage together as a fraction of full vitality. `world::data::damage_inputs` maps a tick's events to `Input::Damage`, the first 16 in the order the game applied them. `Runtime::post_tick` takes the tick's sim events (`sim_state::events`, filled while the object update runs in Rust, `impl.sim.native.objects`), fires the damage inputs before the roster's changes and `on tick`, and drains them only for a script with an `on damage` handler | built; a tick with no events fires none |
| a map's own squads, by name: `label wardens: squad = "<squad name>"` | none | the scenario's squad block (`scnr+0x160`) is not read, and a kit squad slot is `squad(n)`; no research row settles the name lookup | not built; no label kind `squad` exists, so the declaration is a compile error |

Every machine in a game loads the same map, so a tag read is the same
on each. A float field is rounded once to `fixed`. A script names a
field by our schema's name; it sees no offset.

## Firefight and Superbounce as scripts

[rules-world-modes.md](rules-world-modes.md) lists what each mode needs
from this API, what of each is engine code today, and a sketch script
for each. Firefight's generated director
(`firefight::ScriptDirector`, `crates/formats/firefight/src/script.rs`)
is what a hand-written `firefight.rules` must match, command for command
over the director's own test scenarios, and the medal chain
(`crates/extensions/game/src/superbounce_chain.rs`) is what a
Superbounce script must match award for award. Engine code is deleted
only when its script gives the same output over that code's own test
scenarios, and until then both run in tests side by side.

## Splitting a script

A built-in is 30 to 120 lines today; a hand-written Firefight director
is several hundred. Three ways to keep one readable:

| Way | What it is | Cost | Recommendation |
|---|---|---|---|
| Sections | `section "Waves"` on a line of its own, between declarations. It compiles to nothing. `ide::symbols` nests the declarations after it under it, editors fold it, and the workbench shows the outline as a side list that jumps to a section | a keyword, the outline and the formatter | build it with slice W2 |
| Modules (**built**, W10) | `use "waves.rules"` pulls another file's declarations in before checking. One blob, one hash, and the bundle stores every file in `script.modules`. The CLI and `h2 rules lsp` read paths beside the script; Live and the workbench keep a script as a small set of named files | the bundle format, Live's script routes, the workbench's tabs, diagnostics with a file | the language, CLI, editor and bundle are built (rules-engine.md "Modules"); Live's routes and the workbench's tabs follow |
| A node view | a read-only graph of handlers, the variables they touch and the calls they make, drawn from the checked script | a new web view | not now. Forge's logic graph is the visual front end and compiles to the same IR (rules-engine.md "A Forge graph is the same IR"); a second node editor would be a third front end |

## Security and limits

[rules-world-limits.md](rules-world-limits.md) is the contract every
slice is held to, and each slice's row below names the tests that show
it. In short:

- **Who controls what** is named for each direction: script to engine,
  host to client, client to host, Live to host and Live to client.
- **The verifier refuses; the runtime does not clamp.** A constant out
  of range, an index outside the blob's tables and a worst tick that
  passes a write cap are refusals before the game. A computed value out
  of range is a refused call, recorded; a cap passed at run time is a
  script error.
- **Every world write has a cap a tick and a cap a second**, and a
  `Viewer` enforces the show channel's caps on what it receives.
- **No script value reaches engine memory unchecked**: no entry takes a
  size, an offset or an address, and a handle is checked live in one
  place.
- **A show record is used only after its tag and every field check**; a
  bad one is dropped and counted.
- **On Live**, the hopper gate checks the verifier's estimates, the
  denied capabilities, the map's labels and sounds, and the HUD layout.
- **Every new entry has a property test**, and the show codec a fuzz
  target.

## Performance

The same page gives each entry's cost in VM ops and what the host pays
outside the VM, the rule that nothing allocates in a tick, the show
channel's batching, the bandwidth table and the bench gates: three
targets common to every slice, and the one in each slice's row below.

## Implementation plan

Every slice leaves the tree green, lands by itself and ends with
something to play. Paths under `rules/` are in
`crates/formats/rules/src`, under `game/` in
`crates/extensions/game/src`. Each slice adds its rows to
`caps::ENTRIES`, its signatures to `lang/sig.rs`, its fake to `fake.rs`
and its scenario events to `scenario.rs`, and updates
[rules-engine.md](rules-engine.md#the-engine-api)'s tables and the
`h2-rules` skill in the same branch. A slice whose engine entry is not
named starts with its research row below, and ends with the point named
in `points.rs`. Every slice's tests include a row of
`tests/world_props.rs` for each entry it adds, the verifier's refusals
and caps of [rules-world-limits.md](rules-world-limits.md) for those
entries, and that page's zero-allocation test. The **Bench gate** is the slice's
own target on top of the three common ones, as `rules.world_ns` at the
99th percentile unless it says otherwise.

| # | Slice | Owns | Tests | Done when | After | Bench gate |
|---|---|---|---|---|---|---|
| W0 | Roles: `Authority`, `Replica`, `Viewer` | `game/rules/runtime.rs`, `game/rules/feature.rs`; the blob as a start condition in `game/rules/gate.rs` | over invented guest memory: a mode 3 peer keeps its VM and makes the host's calls; a mode 5 client makes none; a peer without the blob blocks the start by name | a script game scores the same on both machines of a synchronous session | the host runtime running in a game; R1 | a script Slayer game against the lifted one: the tick's 99th percentile within 0.1 ms |
| W1 | Messages everywhere: cues (**built**) | `rules/show.rs` (`Cue`, the codec), `game/rules/world/show.rs`, `crates/live/actor-net/src/wire.rs` and `budget.rs` (`Cue`), `game/actor_net/host.rs`, `client.rs`; `message*` named | the codec round-trips and refuses each bad bound; a cue in three packets shows once; the budget drops the rest the same way twice; the fake logs each message with its audience | `message(all, "…")` from Siege shows on the host and on a Live guest | W0 | 8 cues every tick to 15 peers: 0.05 ms, and 600 bytes a second to each |
| W2 (built but for the `Show` record, the digest, `mark`'s audience and the workbench's HUD strip) | Script HUD: show state, `hud` and `waypoint` declarations, sections | `rules/show.rs` (`State`, `view`), `lang/*` (the declarations, `section`), `ir.rs` (the header table), `ide/*` (outline, folding), `game/rules/world/hud.rs` (new, over `game/hud_kit.rs`), `wire.rs` (`Show`, the digest), the workbench's HUD strip in `web/src/stats/RulesTest.tsx` | a declared layout that overlaps is a compile error; `view` of the same bytes is the same on host and client at every tick of a fake match; a `Key` restores the state after loss | a script's wave counter and a waypoint draw on host and guest; Firefight's HUD view reads the show state | W1; the HUD kit's runtime drawing | 32 HUD writes every tick: 0.05 ms; `Show` at most 5 a second in `actors.net` |
| W3 (**built**, but a client's view of a host move waits on R2, R3 and R9) | Objects in motion: `vec`, position reads, `teleport`, movers, `on arrive` | `lang/types.rs`, `lang/check.rs`, `lang/lower.rs` (`vec`), `vm.rs` (movers in `RulesExt`), `game/rules/world/objects.rs` (new) | `vec` arithmetic and each type error; a mover reaches its end on the stated tick and survives a checkpoint revert; a teleport outside the world is a refused call; a film of a mover replays to the same `RulesExt` hash | Race with moving platforms and a teleporter pad plays on Live | W0; R2, R3 | 16 movers: 0.15 ms |
| W4 (the rules half built on the fake, as `tests/world_physics.rs`; `push` and `launch` wait on R9; the host half and the script ports are not built) | Physics reads and pushes: `airborne`, `primed`, `on touchdown`, the sight table, `push`, `launch` | `vm.rs` (the sight table), `game/rules/world/physics.rs` (new), `game/superbounce_charge.rs` (the touchdown for every player), the `sb_strip` label source in `game/rules/census.rs`; `content/modes/rules/superbounce.rules`, `content/modes/rules/bounce_hunt.rules` | the sketch above scores the chain's recorded scenarios as `superbounce_chain_tests.rs` does; a `sees` pair answers one tick after it is asked and never from a handler; a push over the bound is a refused call | the Superbounce game type's chain and medals come from the script; `on bounce`, `on bounced` and the Rust chain are deleted only once the script gives the chain's awards on every scenario of its tests | W2, W3; R9 | 32 traces every tick: 0.25 ms |
| W5 | Audio (**partly built**): `sound_to`, `sound_to_team`, `sound_to_all`, with the host finding a sound by the loaded map's tag index; `say`, `sound_at`, `music`, `announce` refused by name until R4 and R5 | `game/rules/world/audio.rs` (new), the `sound` and `line` types in `lang/*`, the tag table's sound group in `ir.rs` and `verify.rs` | a sound that is not in the map blocks the start by name; a `sound_to` cue reaches only its audience; the music slot starts a late machine at the right tick | a wave's stinger and an Elite's taunt play on every machine of a Live Firefight | W1; R4, R5 | 4 sounds every tick: 0.05 ms |
| W6 | AI orders and the hand-written director: `go_to`, `hold`, `set_skull`, `firefight.rules` | `crates/tools/firefight-kit` (an order per zone), `game/actor_net/squads.rs`, `builtin/firefight.rules` (new), `crates/formats/firefight/src/script.rs` (fills tables) | every scenario of `crates/formats/firefight/tests` passes against the hand-written script; a squad sent to a zone gets that zone's order on the fake squad entry | Firefight runs from `firefight.rules`; the `ff_*` entries, `zone_seen`, `zone_distance` and `firefight.director` are deleted only once the script matches the generated director on every scenario | W2, W4, W5; Firefight's runs R1 to R4 | the script director with 24 actors: `rules.vm_ns` at most 0.4 ms, and the tick within 2% of the generated director's |
| W7 | Bot orders (**built**) | `crates/live/bots/src/brain.rs`, `decisions.rs` (an outside order as the goal), `pad_source.rs`, `lib.rs`, `phantom.rs` (delivery), `game/rules/world/bots.rs` (new) | a phantom with `bot_hold` plans to the zone on a nav fixture (`bots::mind::tests`); playback makes no order (`rules::runtime::tests`); an order changes no `RulesExt` byte (`rules::vm::tests`, `rules::runtime::tests`) | Clankers hold the zones a Firefight script sends them to | W0 | 16 orders every tick: 0.02 ms on the game thread |
| W8 | Vehicles and drop ships: `vehicle`, `path`, `fly`, `seat`, `place_squad_in`, `unload` (built on the fake, with host hover and unload; see "Vehicles and drop ships") | `rules/labels.rs` (`path`), `game/rules/world/vehicles.rs`, the kit's stage 2 and its labelled ship and paths in `crates/tools/firefight-kit` | a `path` with a gap blocks the start; a ship flies a path in the stated ticks, facing its tangent; riders are placed and unloaded on the fake | a Firefight wave arrives by drop ship on a kit map | W3, W6; R6, R7, R8 | a drop ship with 8 riders in flight: 0.2 ms |
| W9 | Advanced data: tag reads, squad state, `on damage`, squads by name | `game/rules/world/data.rs` (new), the field resolver over `tags::schema` | a field the schema lacks is a load error naming it; a tag read is the same on two fake machines | a boss wave's bar shows the boss's shield | W2 | 64 tag reads every tick: 0.02 ms |
| W10 | Modules: `use` (language, CLI, editors and bundle **built**) | `lang/*`, `crates/formats/variants` (the bundle's files), Live's script routes, the workbench's tabs | two files compile to the blob of the same text in one; a cycle is an error naming both files | Firefight's director and its wave tables are two files | W6 | none in the game; `h2 rules check` on 256 KiB of source in under 50 ms |
| W11 | The show channel on System Link | a LAN carrier beside `crates/live/live-hle/src/tunnel_host.rs` and `tunnel_client.rs`: `crates/platform/show-lan` (**built**, carrier only; wiring waits on R10) | two invented peers over an in-memory LAN and over loopback UDP exchange a `Key` and a `Cue`; a forged tag, a replay, a foreign host key and a flood are dropped and counted (`show_lan::tests`) | a System Link guest sees a script's HUD and hears its sounds | W2; R10 | W2's gate over the LAN carrier |

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
| R4 | The body behind `sound_impulse_start` (`0x2a9e00`) for a positioned and an unpositioned one-shot; a table of a map's dialogue names for `line` | W5 |
| R5 | The start and stop of a looping sound; how a retail game event is raised on the host with a player | W5 |
| R6 | [Traced](notes/2026-11-24-vehicle-actions.md). Load (232) calls `0x11b0c0`, unload (233) calls `0x11b2b0`, hover (621) calls `0xf0fd0`. Squad placement selects a seat vehicle by palette and can spawn one. The start's emitter only defers placement. | W8 |
| R7 | [Traced](notes/2026-11-24-vehicle-actions.md). Notified transforms reach eligible Havok bodies through `0x1c4a80`. The next physics step can change the pose. Exact path following with riders and contacts needs a run. | W8 |
| R8 | The kit's stage 2 transplant of a drop ship and its tag headroom on each map | W8 |
| R9 | How a client's own predicted biped takes a host-set position or velocity | W3, W4 |
| R10 | A datagram of ours between System Link peers through the HLE's sockets | W11 |
| R11 | `objects_attach`, the device setters and an order's follow target | `attach`, devices and `follow`, which no slice above needs |

## Not verified

- Driver-first seat order, hover replication and end-of-tick path following
  with riders and contacts.
- The open questions in "Research" above.
- That a generic object's client extrapolates by the replicated
  velocity between updates, so a mover looks smooth at 10 updates a
  second.
- That 16.16 holds every coordinate of every map, converted Halo 1
  levels included.
- That the game's navpoint list draws more than one slot of ours.
- How many labelled strips a map's scan yields against the 256-member
  bound of a label.
- That a `Replica`'s VM and the host's stay equal through a join in
  progress in a synchronous session.
- Every cap and every bench target of rules-world-limits.md: each is a
  first value to set from a run and from probe P4.
- That the tunnel and the LAN carrier deliver a peer's hello to the host
  unchanged, and where a System Link discovery reply has room for the
  host's key.
- The cost of one HMAC per packet and of 32 traces on a dedicated
  host's core.

## Related

- [rules-world-limits.md](rules-world-limits.md): security, caps, per-entry costs and bench gates
- [rules-world-modes.md](rules-world-modes.md): Firefight and Superbounce as scripts over this API
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
- [dedicated-host.md](../live/dedicated-host.md), [lab.md](../runtime/lab.md): the host tunnel, and the bench the gates run on
