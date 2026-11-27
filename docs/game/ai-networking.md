# AI over the network

Status: Partly built. The probe methods (AN0), shared host log (AN1), actor channel (AN2), distributed host capture (AN3), client puppet adapter (AN4), Live tunnel transport (AN5), actor kill events (AN6) the client's receive path with digests and resync (AN7), the `actors.net` readout (AN8) and the host log in films with the mode state in it (AN14) are implemented. AN11 traces the creature slot. AN12 implements the creature probe and replication-version gate. P7 is pending.
Code: `crates/extensions/agent/src/net.rs` (`net.replication`), `crates/extensions/agent/src/actors.rs` and `crates/extensions/game/src/actor_net/report.rs` (`actors.net`), `crates/extensions/agent/src/ai.rs` (`world.ai`), `crates/formats/host-log` (host log), `crates/live/actor-net/src/wire.rs`, `set.rs`, `budget.rs` and `digest.rs` (actor channel), `crates/extensions/game/src/actor_net/host.rs` (capture), `crates/extensions/game/src/actor_net/client.rs` (client puppets), `crates/extensions/game/src/actor_net/link.rs` (per-peer scheduling) and `crates/live/live-hle/src/tunnel_host.rs` and `tunnel_client.rs` (transport), and `crates/rewrites/sim-objects/src/damage.rs` (actor kills), `crates/extensions/game/src/actor_net/creature.rs` (slot 17), `crates/extensions/game/src/replication_admission.rs` (guest admission) and `crates/live/live-hle/src/qos.rs` (Live QoS). Tests: each crate's own, on invented data.
Switch: `agent-net`, `actor-net` and `creature-probe`, listed in [switches.md](../runtime/switches.md).

AI actors in a network game: who runs them, what reaches the other
machines, and what it costs. It serves three users: campaign co-op,
[Firefight](firefight.md), and Forge maps with squads
([forge-design.md](forge-design.md), slice F15). The rule that must hold:
**one machine decides for every actor**. In a synchronous session every
machine reaches the same decision from the same state; in a distributed
session only the host runs AI and a client never does.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
The retail reads behind the **E** rows cover [stock replication](notes/ai-networking.md)
and [the creature slot](notes/creature-entity.md).

## Decisions

- **System Link.** Distributed System Link games get host-run AI through
  entity and event replication. Dialogue and posture require the actor
  channel on the Live tunnel, so System Link omits them.
- **Creatures.** h2recomp reserves entity type 17, the first of 15 unused
  slots in the retail registry built at `0x82240`. Every
  h2recomp machine registers the same type, and the session check refuses
  mixed replication protocol versions before gameplay.

## Which model, by mode

| Mode | Model | Players | Why |
|---|---|---|---|
| Campaign co-op (Halo 2 and converted Halo 1 levels) | **Synchronous**, as built ([live-coop.md](../live/live-coop.md) sections 10 to 14) | 2 to 4 in practice, 16 allowed | Scripts, cutscenes, checkpoints and devices have no replication; every machine must run them. Nothing in this page changes it. |
| Firefight; a Forge map with squads | **Distributed, host actors**: the host runs AI, clients get bodies through entity replication, and Live adds the actor channel | Firefight ships for 1 to 4, with a 16-player playlist gated on [P5](#probes). Forge supports 2 to 16. | Retail skips AI on a distributed client (**E**), a biped body is a unit entity (**E**), and firing, grenades, melee and vehicles are events (**E**). Players keep their prediction. One slow client holds nobody. It is the model dedicated hosts run. |
| Firefight when the probes fail; Forge editing sessions | Synchronous, through the same host log | Firefight 1 to 4, Forge 2 to 4 | The director and the Forge editor both issue host-log entries, which each machine applies at the entry's tick. |

There is no mixed session: a session is synchronous or distributed for
its whole life. "Hybrid" here means the choice is made per mode.

Why not synchronous for everything: at 16 players the synchronous update
is about 44 KB/s to each client if sent raw (**S**, live-coop.md 13.1),
the slowest machine gates every tick, every client pays the whole AI and
physics cost, and a dedicated light host gains nothing because clients
simulate everything anyway. Why not distributed for campaign: scripts and
cutscenes would need a replication layer of their own (live-coop.md
section 3).

## What retail's replication carries

From the type table at `0x82240` (**E**): 17 entity types and 26 event
types. The ones an actor needs:

| Need | Carried by | Evidence | Gap |
|---|---|---|---|
| The body exists on clients | unit entity (type 9; creation record `0x24` bytes) | **E** the type; **S** that a host-made biped with no player is given one (probe P1) | none expected |
| Position, facing, velocity | the shared object flags: position, forward and up, both velocities | **E** `0xa6430` | none |
| Aim | unit flag: desired aiming vector | **E** | none |
| Health and shields | body and shield vitality flags; damage aftermath event | **E** | none |
| Weapon in hand, camouflage, grenade counts | unit flags | **E** | none |
| Firing | weapon fire event (type 12), generated in the weapon's fire code from the weapon's parent unit | **E** `0xa7d50` | **S** that it fires for a unit without a player (probe P3) |
| Projectiles | projectile entity (type 13) and four projectile events | **E** | none |
| Grenades | throw start and release events (14, 22) | **E** types; **S** for actors | none expected |
| Melee | start and damage events (15, 23) | **E** types; **S** for actors | none expected |
| Vehicles | vehicle entity; enter, exit and board events (24, 11, 8); parent vehicle flag | **E** types | **S** that an actor's driving input reaches clients as motion only |
| Death | the dead flag; damage aftermath | **E** | none |
| Ragdoll | nothing: each client runs its own from the death | **E** (no field) | bodies land differently per machine; accepted, as retail does for players |
| Movement animation | nothing explicit; a client animates a unit from its control state | **E** (no animation field) | **S**: an actor's body may slide without a walk cycle (probe P3). The actor channel carries the control if so. |
| Posture and AI-only animation (berserk, flee, cower, sleep, alert) | nothing | **E** | actor channel |
| Dialogue | nothing | **E** (no sound field or event) | actor channel |
| Perception, targets, orders | nothing, and nothing needed: the host decides | **E** `0x1c83c0` | none |
| Creatures (the `crea` object type) | creature entity in an unused retail slot | **E** retail has no creature type; **Design** extension | slot 17 fits the creation codec. AN12 supplies creature scheduling, P7 exercises it and AN13 builds the full entity |

So the host needs nothing new to *run* actors in a distributed game:
`ai_update` returns early only when the network mode byte is 4, the
distributed client (**E**, `0x1c83c0`). What is new is data (a
multiplayer map has no characters, squads or sectors;
[firefight.md](firefight.md), "The kit"), the actor channel and the
creature entity.

## How it works

Three layers, in a distributed session.

**1. Entity and event replication.** The host's biped actors are units,
so their bodies, shots and deaths ride the retail entity and event types
above. Creatures use the registered creature type in the same stream.
The sender ranks entities by relevance and sends them at each entity's
period per client (**E**, notes). The host judges hits.

**2. The actor channel** (`crates/live/actor-net`). A host-to-client
stream for what layer 1 lacks. It is unreliable and ordered by tick, sent
on the [Live datagram tunnel](../live/dedicated-host-ops.md#host-tunnel) next
to the game's own packets. The host schedules one budget per negotiated
tunnel peer and enqueues packets after capture. The client drains a bounded
inbox before the game tick. A new tunnel peer is owed a key, and repeated
requests coalesce into at most one answer every 30 ticks. A System Link
session or a Live client without the channel gets layer 1 only, without actor-channel dialogue and posture.

Version 1 begins with a version byte, a little-endian 32-bit host tick and an
8-bit record count. Each record starts with a one-byte tag. Multibyte fields
are little endian. A packet holds at most 64 records and 1000 bytes including
its six-byte header.

| Record (tag) | When | Payload in wire order | Bytes including tag |
|---|---|---|---:|
| `Key` (0) | on join, resync and every 150 ticks | actor count (u8), actor rows, mode state length (u8), mode bytes | 3 + 10 per actor + mode bytes |
| `Actor` (1) | creation, replacement or removal | action byte, then entity (remove, 0) or actor row (upsert, 1) | 6 or 12 |
| `Posture` (2) | on change | entity, posture (u8, includes AI-only animation) | 6 |
| `Control` (3) | up to 10/s per actor if P3 requires it | entity, packed control (u16) | 7 |
| `Line` (4) | a line starts | entity, vocalisation (u16), original tick (u32) | 11 |
| `State` (5) | mode state changes | length (u8), whole mode blob | 2 + mode bytes |
| `Digest` (6) | every 30 ticks | host log head (32 SHA-256 bytes), mode hash (u64), live actor count (u16), show-state hash (u64) | 51 |
| `Show` (7) | the script's show state, at most 5 a second when it changes and whole beside every key ([rules-world-api.md](rules-world-api.md#the-show-channel)) | length (u8), the state bytes (`rules::show::State::encode`, at most 255) | 2 + state bytes |
| `Cue` (8) | a rules script's show cue, in three consecutive packets as `Line` is; tag 7 is the show channel's `Show` ([rules-world-api.md](rules-world-api.md#the-show-channel)) | cue tick (u32), sequence (u8), kind (u8), length (u8), at most 24 payload bytes | 8 + payload bytes |

An actor row carries the full replication entity index (u32, object body
`+0xd4`, **E**), character class and squad (u16 each), team and posture
(u8 each). The all-ones entity index is invalid. A key holds up to 32
unique actors and the mode blob holds up to 63 bytes. `Control::quantize`
packs forward and side throttle into bits 0–4 and 5–9, crouch into bit 10,
and facing delta into bits 11–15. Each five-bit field maps codes 0–30 to
31 equally spaced values across ±1 for throttle or ±π radians for facing.
Code 15 means zero. Code 31 is reserved.

When a script grants a role, the mode blob carries tag `0x81`, the
selected script's eight-byte reference, a little-endian player-slot
mask (16 bits) and one byte of trait flags. Bits 0, 1 and 2 enable camo,
extra damage and damage resistance. The Firefight HUD bytes follow when
the director is active. After the last holder loses the role, a retail
Juggernaut record sends a zero mask so a guest can release the role
before the native mask catches up. Other records send the HUD alone or
an empty state. A late joiner gets the
current holders and trait values in its first key. The guest checks the
script reference before answering a player's trait query
(`crates/extensions/game/src/actor_net/roles.rs`).

Posture combines a requested animation mode and the crouch request.
The eight bare mode bytes are `84`, `85`, `86`, `89`, `cb`, `2c`,
`39` and `c9` in hex. Each is the low byte of a mode handled by the
unit animation selector at `0x10eef0`. Bare `ff` selects the default.
XOR the bare byte with `10` hex to request crouch. The client ignores
other posture bytes.

`ActorSet::key` snapshots membership, posture and mode state in entity order.
`Replica::apply` replaces that state on a key and applies each packet atomically.
It ignores duplicate and older ticks using wrapping 32-bit ordering. A
delivered key restores the live actor set and mode state after earlier packet loss. Dialogue playback uses the original
(entity, vocalisation, tick) to recognise repeated copies of a line.

**3. The host log** (`crates/formats/host-log`). An ordered, hash-chained
list of entries `{seq, tick, op, prev_hash}`, generic over the op.
Forge's multiplayer integration uses `host_log::Entry<forge_doc::Op>`
with the authenticated `author` field, matching the F1 entry encoding
([forge-design.md](forge-design.md), "The edit log and multiplayer").
Firefight uses `host_log::Entry<firefight::Command>` and omits `author`.
The chain hashes each entry's canonical JSON with SHA-256. A trusted
checkpoint supplies the starting sequence, tick and hash for a suffix.
An operation that changes the world on purpose (place a squad, start a
round, switch a skull) is a log entry.
In a distributed session only the host applies entries and clients see
their effects through layers 1 and 2. In a synchronous session the
entries travel to every machine and each applies them at the entry's
tick.

### On the host, each tick

Boot registers `game::actor_net::Capture`. Its tick observer reads only
when the simulation world is a distributed server. A new match, a
restored snapshot or a clock rewind starts a fresh capture and resets
the connected peers’ budgets.

1. `sim_state::visit_actors` resolves each salted actor/unit pair in both
   directions. It skips player-controlled units and units without a
   replication entity. The character is the map-local character tag slot.
2. The actor's requested animation mode feeds its unit control update.
   The posture byte identifies one of the eight modes the retail animation
   selector handles. Capture checks the complete string ID against the
   corresponding selector operand in the loaded image before emitting its
   low byte. An unsupported or absent mode uses the default code `0xff`.
   A crouch request toggles bit 4 of that code or of the selected mode
   byte. This carries a cower transition even with movement controls off.
3. `actor_net/host.rs` diffs the completed pool by full entity ID. Removals
   precede upserts. A change confined to posture produces a `Posture`.
   A pool read fault, duplicate entity or roster above the 32-actor wire
   limit skips the tick and preserves the last complete pool.
4. `actor_net/dialogue.rs` observes the named candidate on entry to
   `AI_DIALOGUE_ENQUEUE` and queues it when the call succeeds. At tick end
   the speaker's salted unit resolves to an entity. A line keeps its
   original tick and enters each budget once, even if the engine queues
   the same line twice that tick.
5. Boot passes the completed capture to `HostLink::post_tick` for each
   negotiated tunnel peer. The link owns one budget per connection.
   A due or requested key contains the current actor set. Otherwise the
   budget receives the membership and posture deltas. Lines are offered
   on either path. On every host tick that is a multiple of 30, each
   peer is offered a `Digest` of the set after the tick's changes, with
   the head of the host log the running mode writes
   (`actor_net::publish_log_head`, set by the Firefight host runtime after
   each tick; all zeros when no mode keeps a log). The mode hash is 64-bit
   FNV-1a over the mode bytes (`actor_net::digest::state_hash`). Controls, when requested, follow distance to the
   client's player with entity ID as the tie-breaker. The installed
   observer leaves controls off pending P3.

### On a client, each tick

`crates/extensions/game/src/actor_net/client.rs` owns one `Client` per
match. `Capture::with_client` adds it to the `actor-net` feature through
`client::install`, and boot (`registration::register_core_film`) passes a
receive callback that drains the Live tunnel's actor inbox
(`actor_net_link::pre_tick`) into `Client::receive` before `GAME_TICK` on
a distributed client. The adapter resets puppet state on game activation
and on a snapshot restore. A System Link session has no actor inbox, so
its clients see layer 1 only.

1. Apply packets in host-tick order. Resolve each named entity through the
   retail entity table, matching the full entity ID and the salted local
   unit. Pending presentation records expire 30 local ticks after receipt
   if the body has yet to arrive.
2. Keep each resolved unit's character class and squad in the client's
   puppet table for the HUD and agent API.
3. Submit the requested mode, crouch and optional movement controls through
   `UNIT_APPLY_CONTROL`. Leave existing aim, weapons and action inputs to
   the engine. Pose and control updates skip depleted bodies.
4. Resolve the vocalisation in the unit's voice bank. Start the sound on
   its head marker and give a biped's sound instance to lip sync. The
   client remembers each original line tuple for the match, including
   across keys and repeats that arrive while the body is still missing.
5. Check each `Digest` against the replica once its whole packet is
   applied (`actor_net::digest::Check`). A digest that disagrees, or a
   packet that cannot be applied, makes the receive callback ask the host
   for a key (`actor_net_link::request_key`). A disagreement counts
   towards closing the channel when it is the first of a run or a key was
   applied since the previous one, so a key that is slow or lost is asked
   for again. The third such disagreement in a row logs the named error
   `actor_channel_diverged` and closes the channel for the match: the
   client drops its puppets and mode state and ignores further packets
   until the next game activation. The retail entity stream still
   carries the bodies. The client keeps the log head of the last digest
   (`Client::status`).

The actor channel writes presentation fields. The entity stream carries
the host's authoritative damage, position and death state.

### Creature entity

The [creature probe](#creature-slot-probe), on by default, registers slot 17
at simulation initialisation and sets the entity count to 18. Both peers
use the existing five-bit type field. The host enrolls eligible
creatures, including those created by `sim.spawn`. Creation and update
relevance use extension-owned creature tuning. Stock tuning ends at
type 16.

Both sides of a Live or System Link join require the same explicit
replication version, even on maps with zero creatures. Version 1
identifies the stock entity definitions, version 2 the probe and version 3
is reserved for the full creature definition. A missing or unequal
version raises `replication_version_mismatch` before membership.
System Link discovery and Live QoS carry the version in the guest game
description's version word. The join request carries it in the message
header, and the host retains it in the request through deferred admission.
The Live HLE also checks the version at QoS before returning the title's
admission data.

AN13's full definition carries these fields.

| Record | Fields |
|---|---|
| Create | map-local `crea` definition index, initial position, forward and up, scale, linear and angular velocity, body and shield vitality, and dead flag |
| Update | a change mask for position, forward and up, scale, linear and angular velocity, body and shield vitality, and dead flag |
| Delete | the replication entity identity, through the existing entity removal path |

The host creates, simulates, damages and removes the creature. A client
creates a replica from the locally resolved definition, applies the
host's state and animates its body from that state. Perception, targets,
dialogue and posture stay out of this entity. The creature's object tick
and post-physics callbacks leave simulation decisions to the host. AN13 uses
the compatible codecs and pins the chosen masks, units, ranges and
bit widths with synthetic vectors.

**Risks.** Using stock scheduler indexing for type 17 reads unrelated
network settings. A stock receiver discards the entire replication batch
on a slot-17 creation. A peer that assigns a different codec to slot 17
can lose the next record boundary. Retail's build and join-message
versions cover neither case. The replication version check guards both
transports. A client that runs creature behaviour can fight the host's
updates. Too many relevant creatures can starve player and projectile
updates in the shared sender budget.

## Pages

| Page | What |
|---|---|
| [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md) | Budgets, join in progress, Theater, determinism, Forge, interfaces, probes and slices. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Budgets

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#budgets).

### CPU on the host

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#cpu-on-the-host).

## Join in progress

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#join-in-progress).

## Theater

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#theater).

## Determinism and digests

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#determinism-and-digests).

## Sharing with Forge

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#sharing-with-forge).

## Interfaces

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#interfaces).

### Guest boundary

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#guest-boundary).

## Probes

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#probes).

### Creature slot probe

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#creature-slot-probe).

## Slices

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#slices).

## Rules and limits

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#rules-and-limits).

## Evidence

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#evidence).

## Not verified

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#not-verified).

## Related

See [AI networking: budgets, join, determinism and interfaces](ai-networking-budgets.md#related).
