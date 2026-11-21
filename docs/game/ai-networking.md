# AI over the network

Status: Design. AN12 supplies the creature probe and replication-version gate. P7 is pending.
Code: `crates/extensions/agent/src/net.rs` (`net.replication`), `crates/extensions/agent/src/ai.rs` (`world.ai`), `crates/extensions/game/src/actor_net/creature.rs` (slot 17), `crates/extensions/game/src/replication_admission.rs` (guest admission) and `crates/live/live-hle/src/qos.rs` (Live QoS). Planned: `crates/formats/host-log` and `crates/live/actor-net`. Tests: each crate's own, on invented data.
Switch: `agent-net` (the event counter behind `net.replication`), listed in [switches.md](../runtime/switches.md). The `actor-net` feature of `H2_FEATURES` (off until probe P3 is seen in the game) joins it with the code.

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
on a frame kind of the Live datagram tunnel next to the game's own
packets (`crates/live/live-hle`, the tunnel the film side stream uses,
[input-film.md](input-film.md), "The action side stream"). System Link
uses layer 1 for AI. It has no actor channel records, including dialogue
and posture.

| Record | When | Holds | Size (design) |
|---|---|---|---|
| `Key` | on join, on resync, every 150 ticks | every live actor: entity index, character class, squad, team, posture; the mode state blob (Firefight's wave, lives and counts) | 8 bytes an actor + blob |
| `Actor` | an actor is made or removed | entity index, character class, squad, team | 8 bytes |
| `Posture` | on change | entity index, posture id | 3 bytes |
| `Control` | at most 10 a second per actor, only when probe P3 shows it is needed | entity index, throttle (2 × 5 bits), crouch, facing delta | 4 bytes |
| `Line` | a dialogue line starts | entity index, vocalization id, tick | 6 bytes, sent in three consecutive packets |
| `State` | the mode state changes | the mode's blob, whole | under 64 bytes |
| `Digest` | every 30 ticks | host log head, mode state hash, actor count | 20 bytes |

Actors are named by **entity index**, the number retail's replication
already gives both sides (object body `+0xd4`, **E**), so the channel
needs no id map of its own.

**3. The host log** (`crates/formats/host-log`). An ordered, hash-chained
list of entries `{seq, tick, op, prev_hash}`, generic over the op. It is
the log Forge's edit log describes ([forge-design.md](forge-design.md),
"The edit log and multiplayer"); `forge-doc`'s `Entry` is
`host_log::Entry<forge_doc::Op>` and Firefight's is
`host_log::Entry<firefight::Command>`. Whatever changes the world on
purpose (place a squad, start a round, switch a skull) is a log entry.
In a distributed session only the host applies entries and clients see
their effects through layers 1 and 2. In a synchronous session the
entries travel to every machine and each applies them at the entry's
tick.

### On the host, each tick

`crates/extensions/game/src/actor_net/host.rs`, after the game tick:

1. Read the actor pool through `sim_state` (a new `ActorView` over
   `global::ACTORS`, the record `crates/extensions/agent/src/ai.rs`
   reads today).
2. Diff against the last tick: new and removed actors, posture changes.
3. Take dialogue lines from an observer on the engine's dialogue queue
   entry (`AI_DIALOGUE_ENQUEUE`, slice AN3).
4. For each client, fill at most the channel's byte budget for this tick
   in the order: `Key` if owed, `Actor`, `Line`, `Posture`, `State`,
   `Digest`, then `Control` by the actor's distance to that client's
   player.

### On a client, each tick

`crates/extensions/game/src/actor_net/client.rs`, before the game tick:

1. Find each named entity's local object (the entity-to-object lookup,
   a research point, slice AN4). A record for an entity that has not
   arrived yet waits up to 30 ticks, then is dropped.
2. Mark the unit as a puppet in an extension table (not in guest
   memory), with its character class and squad for the HUD and the
   agent API.
3. `Posture` and `Control`: write the unit's control and posture fields
   through the engine's own setters at the tick boundary.
4. `Line`: start the sound through the engine's sound entry on the
   unit's head marker.

The actor channel writes presentation fields. The entity stream carries
the host's authoritative damage, position and death state.

### Creature entity

The opt-in [creature probe](#creature-slot-probe) registers slot 17
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

## Budgets

Numbers marked **S** are estimates. P4 measures actor traffic, P5
measures host CPU work and P7 measures traffic for the minimal creature
definition. The caps are design choices and move with the measurements.

### Bandwidth, 30 ticks a second

Retail's sender shares one per-client budget between all entities by
relevance and period (**E**, notes). More actors therefore lower update
rates before they raise the byte rate. The budget itself is unread;
probe P4 measures the rate with 0, 8, 16 and 24 actors.

| Item | Per client | Host with 15 clients | Evidence |
|---|---:|---:|---|
| A 16-player match with no actors | measured by P4 | measured by P4 | [dedicated-host.md](../live/dedicated-host.md) assumes 1 Mbit/s for the whole match |
| One moving actor's unit updates, about 20 bytes at 10 a second | 200 B/s | 3 KB/s | **S** |
| 24 actors | 4.8 KB/s | 72 KB/s | **S** |
| Their shots, grenades and projectiles | 2 KB/s | 30 KB/s | **S** |
| Actor channel, cap | 1 KB/s | 15 KB/s | **Design**, enforced by `actor_net::Budget` |
| Target for actors in all | 8 KB/s | 120 KB/s (0.96 Mbit/s) | **Design** |
| Creatures, additional steady-state target | 1 KB/s | 15 KB/s | **Design**, includes replication overhead attributable to creatures |

The creature budget starts with four relevant movers per client at up to
5 updates/s each (**Design**). At an estimated 32 bytes per update
(**S**), that uses 640 B/s and leaves 360 B/s for framing and lifecycle
traffic. The sender gives creation and deletion priority over movement
and defers movement when it runs out of room. AN13 checks the budget on
an encoded synthetic stream and P7 records the two-peer wire measurement
for the probe's minimal definition.

The target roughly doubles the egress dedicated-host.md assumes, so that
page's bandwidth line is revised from P4's result before a Firefight
host class is priced.

### CPU on the host

| Item | ms per tick | Evidence |
|---|---:|---|
| The tick | 33.3 | |
| A 16-seat match today | 20 to 29 | dedicated-host.md, "Cost and budget" |
| A light match | 6 to 10 | dedicated-host.md, "Packed hosts" |
| One actor (object, animation, physics, AI) | 0.3 to 0.6 | **S**; no census has an actor in it ([sim-attribution.md](../perf/sim-attribution.md), census B) |
| The actor channel's capture, 32 actors, 15 clients | under 0.1 | **Design**; a bench in AN3 holds it |
| Host CPU target, p99 | 28 | **Design**, game-thread CPU time per tick. The pack uses a separate wall-clock admission threshold. |

Live actor caps by player count (**Design**, provisional until P5):

| Players | Live actors |
|---:|---:|
| up to 4 | 32 |
| up to 8 (probe load) | 24 |
| up to 16 (playlist after P5) | 16 |

The engine's actor pool holds 256 (**Code**, `ai.rs`). At 16 seats the
match is already at the host target with no actors, so 16-player
Firefight needs either an unpacked host with a whole core or the object
and animation phases of [sim-rewrite.md](../engine/sim-rewrite.md).
P5 sets the 16-seat cap from the measured p99 on the intended Firefight
host class. The 16-player playlist opens only once that configuration
meets the 28 ms p99 target at its chosen live-actor cap. Clients pay for
the replicated bodies.

## Join in progress

- **Synchronous.** As built: the host holds, snapshots and sends the
  game state (live-coop.md section 14). Actors are in the arena, so they
  arrive with it. The host log's head and the entries after the snapshot
  go with the join.
- **Distributed.** Retail accepts a join only in the lobby (**E**,
  `SESSION_JOIN_CHECK`: the session state must be 5). Whether the
  distributed world can bring a machine in mid-game once that is lifted
  is unknown: a new view should receive a creation for every live
  entity, since that is how it is filled at the start, but nobody has
  seen it (**S**, probe P6). Until P6 answers, a distributed session
  takes joins in the lobby only, and a player who drops is not replaced.
  When it works, the actor channel sends a `Key` to the new client and
  nothing else is needed: bodies come from layer 1.

## Theater

- **The host's film** (Tier B, [theater.md](theater.md)) replays the
  host's machine from its recorded receives and random draws. Actors run
  inside the host's guest, so the film already holds them: no pose
  stream is added.
- **Host log entries are recorded**, one tagged record per entry at its
  tick in the film's `TICK` stream, beside the inputs (a tag allocated
  in `live_proto::film::stream`, next to Forge's entry tag). Playback
  applies the recorded entries and never runs the director. A seek
  restores a keyframe, whose arena holds every squad the entries placed,
  and replays entries after it.
- **Actor channel records** (`Line`, `Posture`, the mode blob) are
  recorded too, so Theater's HUD shows the wave and the web player can
  caption dialogue without re-deriving them.
- **Web Theater** already draws actors: telemetry frames carry up to 128
  actor rows (**Code**, `live_proto::game::FrameActor`).
- **A client's own film** shows what that client saw: puppets.

## Determinism and digests

| Session | What must agree | Checked by |
|---|---|---|
| Synchronous | the whole arena, every tick | the sim hash, as co-op does (`lockstep::hash_sim`) |
| Synchronous | the host log | each machine's log head in the keep-alive; a head that differs ends that machine's session |
| Distributed, host | replay equals the recording | the film's `HASH` chunks; actors and squads are in the arena they hash |
| Distributed, client | the host log head and the mode state | `Digest` every 30 ticks; a mismatch makes the client ask for a `Key`, and three in a row end its session with a named error |
| Distributed, client | puppet set | `Digest`'s actor count against the client's puppet table; a mismatch asks for a `Key` |

Rules that keep the host replayable:

- The director and every log producer are pure functions of the tick,
  values read from the arena, and a seed drawn once from the engine's
  shared random seed at match start. No wall clock, no host RNG.
- An entry is applied at a tick boundary through an engine entry, never
  by writing actor records.
- Actor channel capture reads and never writes. A host with the channel
  off produces the same arena hashes (a test in AN3).
- State of ours that decides anything lives in the `SimExt` blob once it
  exists (sim-rewrite.md section 2). Until then it is rebuilt by
  replaying the log from the start, and seeking relies on the recorded
  entries.

## Sharing with Forge

Forge's slice F15 lists "actor puppets". This page is that part; F15
keeps mirrors, schedules and the capability mark.

| Piece | Home | Forge uses it for | Firefight uses it for |
|---|---|---|---|
| Host log | `crates/formats/host-log` | the edit log (F1, F5) | director commands |
| Squad place and erase entry | `crates/extensions/game/src/actor_net/squads.rs` | the "squad place" sink (F11) | waves |
| Actor channel | `crates/live/actor-net`, `actor_net/` | puppets in host-replicated sessions (F15) | the same |
| Sectors on a multiplayer map | `pathfind::generate`, called by the kit builder | "sectors at materialise" (F11) | [firefight.md](firefight.md), "The kit" |
| Probes | `net.replication`, `world.ai` | F3's replication and AI questions | probes P1 to P7 |

## Interfaces

### Agent API

| Method | Access | Purpose |
|---|---|---|
| `net.replication` | read | the simulation world's mode, objects by kind with and without an entity, cumulative events queued by type, cumulative datagram and byte counters ([agent-api.md](../runtime/agent-api.md#netreplication)) |
| `net.replication` creature fields (AN12) | read | `creature_type`: registered slot or null. `creatures`: `{entity, type, object, updates}` per live creature entity. `object` is the local salted object handle and `updates` counts sent updates on the host and applied updates on the client for that entity's lifetime. |
| `world.ai` | read | the actor snapshot; also the two AI globals bytes `ai_update` gates on |
| `actors.net` (slice AN8) | read | per client: channel bytes a second, keys and deltas sent, records dropped by the budget; on a client: puppets, records waiting, digest mismatches |

### Guest boundary

| Point | Address | Used for |
|---|---|---|
| Event queue entry | `0xb5a70` (`SIM_EVENT_GENERATE`) | counting events by type (`net.replication`) |
| `ai_update` | `0x1c83c0` (`AI_UPDATE`) | nothing is hooked; its gate is read |
| AI globals | `[0x4f55d0]` (`AI_GLOBALS`): `+0` on, `+1` initialised | `world.ai`; Firefight's host sets `+0` |
| Entity index | object body `+0xd4` | naming actors on the channel |
| Replication type registration | `0x82240` | AN12 registers slot 17 and publishes the entity count before world setup. AN13 uses the same slot |
| Dialogue queue entry | `0x20d0c0` (`AI_DIALOGUE_ENQUEUE`, [halo1/ai.md](../halo1/ai.md)) | AN3 observes it for `Line` |
| Entity to object | `0xa58d0` checks the full entity identity and returns the local object handle or −1 | AN4 |

## Probes

The coordinator runs these; each result goes in
[notes/ai-networking.md](notes/ai-networking.md) with the commit. `HOST`
and `CLIENT` are the two agent sockets of a System Link pair
(dedicated-host.md, "Compare against ordinary headless serve"):
`h2 agent --socket <path> ...`. P2 to P5 need a map with a squad: the
`firefight-probe` test map (firefight.md, slice FF3).

| # | Question | Steps | Reads as |
|---|---|---|---|
| **P1** | Does a host-made biped with no player get a unit entity and reach a client? | Start a Slayer game on the pair. `HOST: sim.session` (expect `mp_host`). `HOST: net.replication`. `HOST: sim.spawn {tag: <a bipd tag on the map>, pos}`. `HOST: wait {map_ticks: 30}`. `HOST: net.replication`; `CLIENT: net.replication`; `CLIENT: sim.objects {kinds: ["biped"]}` | yes when the host's `biped.with_entity` and the client's `biped.objects` each rise by one |
| **P2** | Does AI run on a distributed host on a multiplayer map? | `HOST: maps.test.load firefight-probe`; start the game. `HOST: world.ai` | `globals.on` and `globals.initialised` both true and `actors` non-empty with a `behavior`; if `on` is false, FF4's host switch is needed and the probe is repeated with `world.poke` on the byte |
| **P3** | What does a client see of an actor? | With P2 running: `CLIENT: sim.objects {kinds: ["biped"]}` twice 30 ticks apart (positions change); `CLIENT: render.capture` while the actor walks, fires, throws and dies; `HOST` and `CLIENT: net.replication` before and after a firefight | which of walk cycle, firing, grenade, melee and death look right decides whether `Control` is built; the host's event counts by type show which events actors raise |
| **P4** | What do actors cost in bytes? | 16 seats (phantoms, `sim.player.add`), then 0, 8, 16 and 24 actors: `HOST: net.replication`, `wait {map_ticks: 900}`, `HOST: net.replication` | `traffic.sent_bytes` difference over 30 s, per client, per actor count |
| **P5** | What do actors cost in CPU? | The same four runs. On the intended Firefight host class, take `HOST: host.metrics` after 30 steady gameplay seconds at each count. Read `game_cpu_ms_per_tick_p99` (FF11). Repeat at the chosen cap if it differs from the sampled counts. Keep 16 seats occupied for the confirmation run. | game-thread CPU time by actor count sets the caps. FF13 requires p99 at or below 28 ms at the 16-seat cap |
| **P6** | Can a machine join a distributed game in progress? | With `H2_FEATURES=+mp-jip-probe` (AN9) on the host: start, then join from the second instance. `HOST: net.replication`; `CLIENT: sim.session`, `sim.objects` | the client reaches `mp_client` and its object counts match the host's entity counts |
| **P7** | Does the reserved creature slot carry a creation and a movement update while stock replication continues? | Run [the creature slot probe](#creature-slot-probe) on System Link | both peers observe the same new creature entity of the AN11 type, the client receives its motion, and player entities continue updating |

### Creature slot probe

The probe's creation payload is 304 bits. It carries a 16-bit local
definition index and nine binary32 coordinates for position, forward and
up. Movement carries three binary32 position coordinates in 96 bits.
The retail envelope adds its own framing and can attach the initial
position delta. The scheduler gives creation
a score of 1.0 and movement a score of 0.5, with a 200 ms minimum period
per peer for movement.

Use two fresh pairs of h2recomp processes. Run both control peers with
the probe disabled and both candidate peers with it enabled
([switches](../runtime/switches.md)).
Start the same two-player System Link Slayer game for each pair, with a
local player on the host. Use the
[System Link socket setup](../live/live.md#client-hle-sockets-and-system-link).
Use the same locally built map with a resolvable `crea` definition, zero
creature placements and a clear drop point high enough to observe motion
across several update periods.

Set `P7_HOST_SOCKET` and `P7_CLIENT_SOCKET` to the two Agent API
endpoints. Set `P7_SPAWN` to the private fixture JSON
`{"tag":"crea:<local definition>","pos":[x,y,z],"yaw":0}`
and `P7_MOVE` to `{"player":"player.0","pos":[x,y,z]}` with a clear
ground destination. Use the same two JSON objects in both runs.
Set `P7_OUT` to a fresh output directory for each pair.

```sh
mkdir -p "$P7_OUT"

p7_snapshot() {
    h2 agent --socket "$P7_HOST_SOCKET" net.replication '{}' > "$P7_OUT/$1.host.net.json" &&
    h2 agent --socket "$P7_CLIENT_SOCKET" net.replication '{}' > "$P7_OUT/$1.client.net.json" &&
    h2 agent --socket "$P7_HOST_SOCKET" sim.objects '{"kinds":["crea","bipd"]}' > "$P7_OUT/$1.host.objects.json" &&
    h2 agent --socket "$P7_CLIENT_SOCKET" sim.objects '{"kinds":["crea","bipd"]}' > "$P7_OUT/$1.client.objects.json"
}

p7_wait() {
    h2 agent --socket "$P7_HOST_SOCKET" session.wait '{"until":{"ticks":'"$1"'},"timeout_ms":10000}'
}

p7_ready() {
    h2 agent --socket "$1" session.methods '{}' &&
    h2 agent --socket "$1" session.wait '{"until":{"phase":"gameplay","map_ticks":300},"timeout_ms":30000}'
}

p7_ready "$P7_HOST_SOCKET" && p7_ready "$P7_CLIENT_SOCKET"
```

Run these steps in order. Stop if a command fails or a required field is
missing.

1. Take the starting snapshot. Require `dist-server` on the host and
   `dist-client` on the client. Both `creatures` arrays start empty.
   Check that both `creature_type` fields are null for the control and
   17 for the candidate.

   ```sh
   p7_snapshot start
   ```

2. Record an empty-window traffic baseline.

   ```sh
   p7_wait 30 && p7_snapshot empty
   ```

3. Spawn one creature and sample its movement through the drop.

   ```sh
   h2 agent --socket "$P7_HOST_SOCKET" sim.spawn "$P7_SPAWN" > "$P7_OUT/spawn.json" &&
   p7_snapshot spawn &&
   p7_wait 30 && p7_snapshot moved &&
   p7_wait 120 && p7_snapshot settled
   ```

   In the control, the host has one new local creature and the client has
   zero. In the candidate, the host's `objects.crea.with_entity` and the
   client's `objects.crea.objects` each rise by one. An absent kind
   counts as zero. Require one matching full `entity` value with
   `type: 17` on each peer. Match each `object` to the corresponding
   peer's `sim.objects` row and the host handle to `spawn.json`. Compare
   handles as hexadecimal values. The host writes and the client applies
   at least one movement update. The client position follows the drop
   and settles at the host position within 0.001 world units on each axis.

4. Move the host's biped and look for that movement among the client's
   biped rows in both runs.

   ```sh
   h2 agent --socket "$P7_HOST_SOCKET" sim.state.set "$P7_MOVE" > "$P7_OUT/teleport.json" &&
   p7_wait 30 && p7_snapshot player-moved
   ```

5. Compare the matching intervals on the two pairs. For each interval,
   divide the host `traffic.sent_bytes` and client
   `traffic.received_bytes` deltas by the respective elapsed
   `at.map_ticks` and multiply by 30. Report aggregate datagram bytes
   per second and the creature payload sizes from the synthetic codec
   tests. Keep the spawn and settled snapshots as the identity and
   movement evidence.

The mixed-version check is a lobby test with synthetic peers in AN12.
P7 uses matched pairs.

## Slices

Lanes write code and tests on invented data and run nothing. Sizes: S is
under a day, M one to three days, L a week.

| # | Slice | Size | Needs | Files | Named tests | Acceptance |
|---|---|---|---|---|---|---|
| **AN0** | `net.replication`; `world.ai` reports the AI globals; the `SIM_EVENT_GENERATE` point and `AI_GLOBALS` global | S | nothing | `crates/extensions/agent/src/net.rs`, `crates/extensions/agent/src/ai.rs`, `crates/hooking/guest-layout/src/{points,global}.rs`, `crates/extensions/game/src/agent_glue.rs` | `net::tests::{counts_objects_by_kind_with_and_without_an_entity, reports_no_mode_and_no_objects_before_the_simulation_exists, a_local_game_is_not_distributed, event_counts_are_cumulative_and_named, unknown_event_types_land_in_other, traffic_is_passed_through, the_method_is_registered}`, `ai::tests::reports_the_ai_globals_bytes` | both methods answer over a fake guest; `session.methods` lists `net.replication` |
| **AN1** | `host-log`: `Entry<Op>`, `Log::push`, `head`, `verify`, canonical bytes, `since(seq)` | S | nothing | `crates/formats/host-log/` | `chain_breaks_on_a_changed_entry`, `since_returns_the_tail`, `golden_head_for_three_entries`, `out_of_order_seq_is_refused` | golden head vector; a tampered entry fails `verify` |
| **AN2** | `actor-net` wire: the seven records, quantisers, `encode`/`decode`, `Key` build and apply over a plain `ActorSet`, `Budget` | M | nothing | `crates/live/actor-net/src/{wire,set,budget}.rs` | `every_record_round_trips`, `key_then_deltas_equals_host_set`, `lost_packets_converge_after_the_next_key`, `budget_never_exceeds_its_bytes`, `line_survives_two_lost_packets_in_three`, `decode_never_panics_on_fuzz` | 10,000 seeded lossy runs converge within one `Key` period; no packet over the budget |
| **AN3** | Host capture: `sim_state::ActorView`, the per-tick diff, per-client fill order, the observer on `AI_DIALOGUE_ENQUEUE` | M | AN2 | `crates/rewrites/sim-state/src/actors.rs`, `crates/extensions/game/src/actor_net/host.rs`, `points.rs` | `diff_reports_new_removed_and_posture`, `nearest_actors_get_control_first`, `capture_leaves_the_arena_hash_unchanged`, `a_line_is_queued_once` | over a fake guest with a scripted actor pool, the packets decode to the pool; the arena hash is equal with the channel on and off |
| **AN4** | Client puppets: the entity-to-object lookup (traced first), the puppet table, posture and control writes, the dialogue start | M | AN2; P3's answer for `Control` | `crates/extensions/game/src/actor_net/client.rs`, `points.rs` | `a_record_waits_for_its_entity_then_applies`, `an_unknown_entity_is_dropped_after_30_ticks`, `a_client_writes_no_damage_or_position`, `key_replaces_the_puppet_table` | over a fake guest, a `Key` and deltas produce the expected engine calls and no other write |
| **AN5** | Transport: the tunnel frame kind, send on the host's post-tick, receive queue on clients, `Key` on join and on request | S | AN2 | `crates/live/live-hle/src/tunnel*.rs`, `crates/extensions/game/src/actor_net/link.rs` | `frames_of_another_kind_are_ignored`, `a_key_request_is_answered_once_a_second_at_most`, `no_tunnel_means_no_channel` | two fake peers over the simulated link (`crates/live/coop`'s) converge under loss and reorder |
| **AN6** | Actor events: `SimEvent::ActorKilled {unit, character, killer, weapon}`, the film `Event` variant, the telemetry row | S | nothing | `crates/rewrites/sim-objects/src/events.rs`, `crates/live/live-proto/src/film/events.rs`, `crates/live/live-telemetry/` | `a_fatal_damage_to_an_actor_unit_is_one_actor_killed`, `a_player_death_is_not_an_actor_kill`, `actor_killed_round_trips_in_a_film` | on a fake guest, one fatal damage report on a unit with an actor yields exactly one event with the killer's player |
| **AN7** | Films and digests: the host-log record tag, recording and playback apply, `Digest` build and check, the client's resync | M | AN1, AN2, AN5 | `crates/live/live-proto/src/film/stream.rs`, `crates/live/film/`, `actor_net/digest.rs` | `a_film_replays_entries_at_their_ticks`, `playback_never_runs_a_producer`, `a_wrong_head_asks_for_a_key`, `three_mismatches_end_the_session` | a recorded fake session replays to the same log head and mode state hash |
| **AN8** | `actors.net`, its doc section and `h2 agent` examples | S | AN3, AN4, AN5 | `crates/extensions/agent/src/actors.rs`, `agent-api.md` | `host_reports_per_client_rates`, `client_reports_puppets_and_waiting` | the method answers on a fake host and a fake client |
| **AN9** | `mp-jip-probe`: lift the in-game join refusal on a distributed host behind a feature, log each stage | S | P1 | `crates/extensions/game/src/mp_jip_probe.rs` | `the_refusal_is_lifted_only_for_a_distributed_session_with_the_feature_on` | P6 can be run |
| **AN10** | Distributed join in progress, built from P6's answer | M | AN9, P6 | decided by P6 | decided by P6 | a third fake peer joining gets a `Key` and equal puppet tables |
| **AN11** | Static read of creature registration, registry and scheduling bounds, serializers, unknown-type receive and session admission | M | nothing | [notes/creature-entity.md](notes/creature-entity.md) | `entity_table_bounds_trace`, `entity_type_codec_trace`, `unknown_type_receive_trace` (static checks) | slot 17 fits the existing codec. The host needs a creature scheduling record. A stock client rejects the batch on creation and refuses later motion for the missing identity |
| **AN12** | P7: slot-17 definition, host creature enrollment, separate creation and update tuning, replication-version admission and agent readout | M | AN11, AN0, P1 | `crates/extensions/game/src/actor_net/creature.rs`; `crates/hooking/guest-layout/src/points.rs`; `crates/live/live-hle/` and the guest session join check; `crates/extensions/agent/src/net.rs`; [agent-api.md](../runtime/agent-api.md#netreplication) | `probe_definition_uses_the_selected_slot`, `creature_scheduling_preserves_stock_tuning`, `probe_creation_and_position_round_trip`, `creature_entities_report_objects_and_updates`, `mixed_versions_are_refused_before_replication` | P7 shows one enrollment per spawn and a scheduled position update with stock player replication intact. Synthetic joins on Live and System Link refuse both directions of a mixed-version pair before membership. Direct, invited and deferred joins enforce the same check |
| **AN13** | Creature entity: full creation, masked updates and removal, host authority, creature scheduling and bandwidth | L | AN12 admission tests and matched-pair P7 pass | planned `crates/extensions/game/src/actor_net/creature.rs` and `crates/extensions/game/src/actor_net/creature_wire.rs`; `crates/hooking/guest-layout/src/points.rs`; `crates/apps/boot/src/registration.rs` | `creature_create_update_delete_round_trip`, `a_creation_resolves_only_a_local_crea_tag`, `invalid_masks_and_truncated_packets_leave_state_unchanged`, `unknown_and_stale_entity_updates_are_rejected`, `retail_entities_keep_their_wire_encoding`, `only_the_host_advances_creature_behaviour`, `creature_updates_fit_the_sender_budget`, `the_full_definition_refuses_the_probe_version` | the full codec fits the baseline scratch and mask bounds, uses AN12's creature tuning, and covers lifecycle, loss and slot reuse with no stale writes. A 30-second synthetic stream at the design load fits 1 KB/s per peer with the stock entity budget preserved. Both transports use the same definition and version gate |

AN0, AN1, AN2 and AN6 start at once and touch no existing behaviour.
AN3, AN4 and AN5 follow AN2 in parallel. Probes P1 and P6 need only AN0
(and AN9); P2 to P5 wait for the probe map. AN12 uses the AN11 result.
AN13 requires a passing P7.

## Rules and limits

- A client never runs `ai_update` and never decides damage.
- A distributed session with actors needs every machine on the same map
  package; the capability mark of forge-design.md refuses a joiner
  without it.
- Distributed sessions require matching replication protocol versions.
  Session admission checks the version before accepting entity traffic.
- Ragdolls and other client-only effects differ between machines.
- The channel's records name nothing from the disc: ids are indices
  into tables each machine builds from its own map.

## Evidence

- **E** The replicated types, their sizes, the event entry, the object
  flags, the AI gate and the entity index:
  [notes/ai-networking.md](notes/ai-networking.md).
- **E** The lobby-only join check: `SESSION_JOIN_CHECK`,
  `crates/hooking/guest-layout/src/points.rs`.
- **Code** The actor pool and record: `crates/extensions/agent/src/ai.rs`.
- **Code** Datagram counters: `crates/live/live-hle/src/traffic.rs`.
- **S** Every row marked so above, with the probe that settles it.

## Not verified

- The runtime outcomes of P1 to P7, including scheduled creature motion
  alongside stock player replication and the measured traffic delta.
- That generated sectors are accepted by the engine and actors walk on
  them ([pathfinding.md](../formats/pathfinding.md)).
- Who judges a client's shot at an actor, and with what lag.
- That an actor on a team no player holds fights players under a
  multiplayer game engine.
- The encoders' bit widths and the sender's per-client budget.

## Related

- [firefight.md](firefight.md): the mode built on this.
- [forge-design.md](forge-design.md), [forge-features.md](forge-features.md): the host log's other user, and F15.
- [live-coop.md](../live/live-coop.md): the synchronous model.
- [bots.md](bots.md): Clankers are players, not actors.
- [theater.md](theater.md), [input-film.md](input-film.md), [film-format.md](film-format.md): films.
- [dedicated-host.md](../live/dedicated-host.md): host cost.
- [halo1/ai.md](../halo1/ai.md): the scenario's AI blocks.
