# Rules world API: a synced world for scripts

Status: Design, with the first slices landing. Messages as cues (W1), the script HUD's declarations and show state (W2), objects in motion (W3), the physics events with their host half, the Superbounce chain as a script, and the host's `airborne`, `crouching`, `primed` and `sees` reads (W4), the `sound_to` audio entries (W5, partly), the bot orders (W7), vehicles (W8, on the fake with host hover and unload), the data reads with `on damage` (W9), modules (W10) and the System Link show carrier (W11, carrier only) are built; the rest extends the rules engine of [rules-engine.md](rules-engine.md) and is built after that page's host runtime runs in a game.
Code: slice W1: `crates/formats/rules/src/show.rs` (the cue and its codec), `crates/extensions/game/src/rules/world/show.rs` (the host and client halves), `crates/live/actor-net/src/wire.rs` and `budget.rs` (the `Cue` record and its three-packet repeat), `crates/extensions/game/src/actor_net/host.rs` and `client.rs` (offered to every peer; shown once). Slice W2: `crates/formats/rules/src/show/state.rs` (the show state and `view`), the `hud`, `waypoint` and `section` declarations of `crates/formats/rules/src/lang`, the `hud_*`, `mark`, `unmark`, `side_of` and `slot_of` entries of `caps.rs`, `RulesExt::show` in `vm.rs`, `crates/extensions/game/src/rules/world/hud.rs` (the feed of the HUD kit's objective HUDs, `crates/rewrites/hud-kit-core/src/views/objective.rs`), and the state's halves of `crates/extensions/game/src/rules/world/show.rs` (the authority's offer and a `Viewer`'s hold and draw). Slice W9: the entries `object_health` to `squad_strength` of `crates/formats/rules/src/caps.rs`, `on damage` (`ir::Event::Damage`), `crates/formats/rules/src/fields.rs` (the tag read's field paths and load check), `crates/extensions/game/src/rules/world/data.rs`. Slice W7: the `bot_*` entries of `crates/formats/rules/src/caps.rs`, the dispatch in `crates/extensions/game/src/rules/runtime/guest_game.rs` (`act`) and `runtime/work.rs` (`make_calls`), `crates/extensions/game/src/rules/world/bots.rs`, `crates/live/bots/src/brain.rs` and `decisions.rs`. The plan names `crates/formats/rules/src/caps.rs` (the entries), `crates/formats/rules/src/world/` and `crates/extensions/game/src/rules/world/` (one module per capability group; `bots` and `show` are built), and `crates/live/actor-net/src/wire.rs` (two records, both built). Tests: named per slice in "Implementation plan".; shown once). Slice W2: `crates/formats/rules/src/show/state.rs` (the show state and `view`), the `hud`, `waypoint` and `section` declarations of `crates/formats/rules/src/lang`, the `hud_*`, `mark`, `unmark`, `side_of` and `slot_of` entries of `caps.rs`, `RulesExt::show` in `vm.rs`, `crates/extensions/game/src/rules/world/hud.rs` (the feed of the HUD kit's objective HUDs, `crates/rewrites/hud-kit-core/src/views/objective.rs`). Slice W9: the entries `object_health` to `squad_strength` of `crates/formats/rules/src/caps.rs`, `on damage` (`ir::Event::Damage`), `crates/formats/rules/src/fields.rs` (the tag read's field paths and load check), `crates/extensions/game/src/rules/world/data.rs`. Slice W4: `on touchdown`, `on launch` and `on grounded` (`ir::Event`), `crates/extensions/game/src/superbounce_chain.rs` (`Physics`, what the chain sees), `crates/extensions/game/src/rules/world/physics.rs` (the host's inputs), `content/modes/rules/superbounce.rules` (the chain as a script). Slice W7: the `bot_*` entries of `crates/formats/rules/src/caps.rs`, the dispatch in `crates/extensions/game/src/rules/runtime/guest_game.rs` (`act`) and `runtime/work.rs` (`make_calls`), `crates/extensions/game/src/rules/world/bots.rs`, `crates/live/bots/src/brain.rs` and `decisions.rs`. The plan names `crates/formats/rules/src/caps.rs` (the entries), `crates/formats/rules/src/world/` and `crates/extensions/game/src/rules/world/` (one module per capability group; `Cue` is built). Tests: named per slice in "Implementation plan".
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
`RULES` record (`sim_world::ext::RULES`, in the sim hash) and makes the same
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
  get it. Two cues are the exceptions, since they carry state. The walls
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
  The pickers' offer cue (`rules::show::Cue::Pick`, kind 4) holds each
  picker's offer the same way, for the players a client would ask
  ([Pickers](#pickers)).

An audience is a 16-bit mask of player slots. A text is an index into
the blob's string table and a sound an index into its tag table, so a
record names only what the verified blob holds; a `Viewer` has the blob
from the lobby fetch.

The actor channel (`crates/live/actor-net`, ai-networking.md "How it
works") carries both, with two more record tags:

| Record (tag) | When | Payload |
|---|---|---|
| `Show` (7) (**Code**, `crates/live/actor-net/src/wire.rs`) | the state a peer's players are the audience of changed, at most 5 times a second; whole in every `Key` | length (u8), the state (`rules::show::State::encode`, at most 255 bytes) |
| `Cue` (8) (**Code**, `crates/live/actor-net/src/wire.rs`) | a cue for that peer's players, in three consecutive packets as `Line` is (**Code**, `crates/live/actor-net/src/budget.rs`) | cue tick (u32), sequence (u8), kind (u8), length (u8), at most 24 payload bytes |

The `Show` record is built (**Code**, `crates/live/actor-net/src/wire.rs`,
`budget.rs`, `game/src/actor_net/host.rs` and `client.rs`): the authority
offers the state it holds after each tick
(`rules/world/show.rs::offer_state`), the budget coalesces the offers to
at most 5 a second and one rides every key whole, an empty record
retracts the last state, and the digest beside the actor set's carries
the hash of the state the peer was last offered, whose mismatch asks for
a `Key` (**Code**, `rules::show::State` is at most 244 bytes; a `Viewer`
holds the newest record that answers its blob and draws the HUDs from
it, `rules/world/show.rs::receive`). The state carries each audience as
a mask of player slots, and each machine shows it only to its own
players in it, so a team's waypoint draws on its team's machines alone;
which players a tunnel peer has is not known to the channel, so the
whole state goes to every peer until it is. A message cue carries its
audience as a mask of player slots and goes to every peer; each peer
shows it to its own players in the audience and to no one else (**Code**,
`crates/extensions/game/src/rules/world/show.rs`).

A round-start clip goes to every peer. The viewer accepts only a type
with a custom start line that matches its running script or Firefight
director. The one-byte payload selects a local clip (`rules::show::Cue`).

**Authenticated** (not built; the records ride unauthenticated until it
is). The tunnel and XNet's UDP mapping give no transport
security of their own (**Code**, [dedicated-host.md](../live/dedicated-host-ops.md#routing-assignment-and-arbitration)),
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
(**Code**, `crates/live/show-lan`). It has the Live channel's
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

## Pages

| Page | What |
|---|---|
| [Rules world API: the API](rules-world-api-reference.md) | The world API. |
| [Rules world API: scripts, limits and plan](rules-world-api-limits.md) | Firefight and Superbounce as scripts, splitting a script, security, performance and the plan. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## The world API

See [Rules world API: the API](rules-world-api-reference.md#the-world-api).

### Objects

See [Rules world API: the API](rules-world-api-reference.md#objects).

### Vehicles and drop ships

See [Rules world API: the API](rules-world-api-reference.md#vehicles-and-drop-ships).

### Zones

See [Rules world API: the API](rules-world-api-reference.md#zones).

### Loadouts

See [Rules world API: the API](rules-world-api-reference.md#loadouts).

### Pickers

See [Rules world API: the API](rules-world-api-reference.md#pickers).

### Audio

See [Rules world API: the API](rules-world-api-reference.md#audio).

### HUD

See [Rules world API: the API](rules-world-api-reference.md#hud).

#### The HUDs a script feeds

See [Rules world API: the API](rules-world-api-reference.md#the-huds-a-script-feeds).

### Advanced data

See [Rules world API: the API](rules-world-api-reference.md#advanced-data).

## Firefight and Superbounce as scripts

See [Rules world API: scripts, limits and plan](rules-world-api-limits.md#firefight-and-superbounce-as-scripts).

## Splitting a script

See [Rules world API: scripts, limits and plan](rules-world-api-limits.md#splitting-a-script).

## Security and limits

See [Rules world API: scripts, limits and plan](rules-world-api-limits.md#security-and-limits).

## Performance

See [Rules world API: scripts, limits and plan](rules-world-api-limits.md#performance).

## Implementation plan

See [Rules world API: scripts, limits and plan](rules-world-api-limits.md#implementation-plan).

## Not verified

See [Rules world API: scripts, limits and plan](rules-world-api-limits.md#not-verified).

## Related

See [Rules world API: scripts, limits and plan](rules-world-api-limits.md#related).
