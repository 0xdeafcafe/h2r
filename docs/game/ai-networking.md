# AI over the network

Status: Partly built. The probe methods (AN0), actor channel (AN2), distributed host capture (AN3) and Live tunnel transport (AN5) are implemented. Client puppets are a separate slice.
Code: `crates/extensions/agent/src/net.rs` (`net.replication`), `crates/extensions/agent/src/ai.rs` (`world.ai`), `crates/live/actor-net/src/wire.rs`, `set.rs` and `budget.rs` (actor channel), `crates/extensions/game/src/actor_net/host.rs` (capture), `crates/extensions/game/src/actor_net/link.rs` (per-peer scheduling) and `crates/live/live-hle/src/tunnel_host.rs` and `tunnel_client.rs` (transport). Tests: each crate's own, on invented data.
Switch: `agent-net` and `actor-net`, listed in [switches.md](../runtime/switches.md).

AI actors in a network game: who runs them, what reaches the other
machines, and what it costs. It serves three users: campaign co-op,
[Firefight](firefight.md), and Forge maps with squads
([forge-design.md](forge-design.md), slice F15). The rule that must hold:
**one machine decides for every actor**. In a synchronous session every
machine reaches the same decision from the same state; in a distributed
session only the host runs AI and a client never does.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
The static read behind the **E** rows is in
[notes/ai-networking.md](notes/ai-networking.md).

## Which model, by mode

| Mode | Model | Players | Why |
|---|---|---|---|
| Campaign co-op (Halo 2 and converted Halo 1 levels) | **Synchronous**, as built ([live-coop.md](../live/live-coop.md) sections 10 to 14) | 2 to 4 in practice, 16 allowed | Scripts, cutscenes, checkpoints and devices have no replication; every machine must run them. Nothing in this page changes it. |
| Firefight; a Forge map with squads | **Distributed, host actors**: the host runs AI, clients get each actor's body through retail's replication and the rest through the actor channel | 2 to 16 | Retail already skips AI on a distributed client and nowhere else (**E**), a body is a unit entity (**E**), and firing, grenades, melee and vehicles are events (**E**). Players keep their prediction. One slow client holds nobody. It is the model dedicated hosts run. |
| Firefight when the probes fail; Forge editing sessions | Synchronous, through the same host log | 2 to 4 | The fallback. The director and the log are written against the host log, not against a protocol, so the mode is a session setting. |

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
| Creatures (the `crea` object type) | no entity type | **E** | not supported in a distributed session; a kit leaves them out |

So the host needs nothing new to *run* actors in a distributed game:
`ai_update` returns early only when the network mode byte is 4, the
distributed client (**E**, `0x1c83c0`). What is new is data (a
multiplayer map has no characters, squads or sectors;
[firefight.md](firefight.md), "The kit") and the three gaps above.

## How it works

Three layers, in a distributed session.

**1. Retail replication.** Unchanged. The host's actors are units like
any other, so their bodies, shots and deaths ride the entity and event
types above, ranked by relevance and sent at each entity's period per
client (**E**, notes). Hits are the host's to judge, as for players.

**2. The actor channel** (`crates/live/actor-net`). A host-to-client
stream for what layer 1 lacks. It is unreliable and ordered by tick, sent
on the [Live datagram tunnel](../live/dedicated-host.md#host-tunnel) next
to the game's own packets. The host schedules one budget per negotiated
tunnel peer and enqueues packets after capture. The client drains a bounded
inbox before the game tick. A new tunnel peer is owed a key, and repeated
requests coalesce into at most one answer every 30 ticks. A System Link
session or a Live client without the channel gets layer 1 only.

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
| `Line` (4) | a line starts | entity, vocalization (u16), original tick (u32) | 11 |
| `State` (5) | mode state changes | length (u8), whole mode blob | 2 + mode bytes |
| `Digest` (6) | every 30 ticks | host log head (32 SHA-256 bytes), mode hash (u64), live actor count (u16) | 43 |

An actor row carries the full replication entity index (u32, object body
`+0xd4`, **E**), character class and squad (u16 each), team and posture
(u8 each). The all-ones entity index is invalid. A key holds up to 32
unique actors and the mode blob holds up to 63 bytes. `Control::quantize`
packs forward and side throttle into bits 0–4 and 5–9, crouch into bit 10,
and facing delta into bits 11–15. Each five-bit field maps codes 0–30 to
31 equally spaced values across ±1 for throttle or ±π radians for facing.
Code 15 means zero. Code 31 is reserved.

`ActorSet::key` snapshots membership, posture and mode state in entity order.
`Replica::apply` replaces that state on a key and applies each packet atomically.
It ignores duplicate and older ticks using wrapping 32-bit ordering. A
delivered key restores the live actor set and mode state after earlier packet loss. Dialogue playback uses the original
(entity, vocalization, tick) to recognize repeated copies of a line.

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
   on either path. Controls, when requested, follow distance to the
   client's player with entity ID as the tie-breaker. The installed
   observer leaves controls off pending P3.

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

A client writes nothing that decides an outcome: no damage, no position,
no death.

## Budgets

Numbers marked **S** are estimates until probes P4 and P5 put a note
behind them. The caps are design choices and move with the measurements.

### Bandwidth, 30 ticks a second

Retail's sender shares one per-client budget between all entities by
relevance and period (**E**, notes). More actors therefore lower update
rates before they raise the byte rate. The budget itself is unread;
probe P4 measures the rate with 0, 8, 16 and 24 actors.

One `actor_net::Budget` per client admits at most 1000 actor-channel bytes
in any 30 consecutive simulation ticks, including packet headers. Tunnel
framing is accounted separately. Call `Budget::schedule` each tick with the
client's candidates, ordered nearest first among controls. The scheduler
reserves the next two copies before admitting a dialogue line, so it
appears in three consecutive packets. A host offers the current key when
`Budget::key_due` is true and retries a requested resync key until sent.

| Item | Per client | Host with 15 clients | Evidence |
|---|---:|---:|---|
| A 16-player match with no actors | measured by P4 | measured by P4 | [dedicated-host.md](../live/dedicated-host.md) assumes 1 Mbit/s for the whole match |
| One moving actor's unit updates, about 20 bytes at 10 a second | 200 B/s | 3 KB/s | **S** |
| 24 actors | 4.8 KB/s | 72 KB/s | **S** |
| Their shots, grenades and projectiles | 2 KB/s | 30 KB/s | **S** |
| Actor channel, cap | 1 KB/s | 15 KB/s | **Code**, `actor_net::Budget` |
| Target for actors in all | 8 KB/s | 120 KB/s (0.96 Mbit/s) | **Design** |

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
| The actor channel's capture, 32 actors, 15 clients | under 0.1 | **Design**; the ignored release benchmark `capture_32_actors_for_15_clients` checks this target |
| Host target, p99 | 28 | **Design**: the pack's admission closes at 36 |

Live actor caps by player count (**Design**, provisional until P5):

| Players | Live actors |
|---:|---:|
| up to 4 | 32 |
| up to 8 | 24 |
| up to 16 | 16 |

The engine's actor pool holds 256 (**Code**, `ai.rs`). At 16 seats the
match is already at the host target with no actors, so 16-player
Firefight needs either an unpacked host with a whole core or the object
and animation phases of [sim-rewrite.md](../engine/sim-rewrite.md).
Clients pay only for the bodies, as they do for 16 more players.

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
| Probes | `net.replication`, `world.ai` | F3's replication and AI questions | probes P1 to P6 |

## Interfaces

### Agent API

| Method | Access | Purpose |
|---|---|---|
| `net.replication` | read | the simulation world's mode, objects by kind with and without an entity, cumulative events queued by type, cumulative datagram and byte counters ([agent-api.md](../runtime/agent-api.md#netreplication)) |
| `world.ai` | read | the actor snapshot; also the two AI globals bytes `ai_update` gates on |
| `actors.net` (slice AN8) | read | per client: channel bytes a second, keys and deltas sent, records dropped by the budget; on a client: puppets, records waiting, digest mismatches |

### Guest boundary

| Point | Address | Used for |
|---|---|---|
| Event queue entry | `0xb5a70` (`SIM_EVENT_GENERATE`) | counting events by type (`net.replication`) |
| `ai_update` | `0x1c83c0` (`AI_UPDATE`) | nothing is hooked; its gate is read |
| AI globals | `[0x4f55d0]` (`AI_GLOBALS`): `+0` on, `+1` initialised | `world.ai`; Firefight's host sets `+0` |
| Entity index | object body `+0xd4` | naming actors on the channel |
| Dialogue queue entry | `0x20d0c0` (`AI_DIALOGUE_ENQUEUE`, [halo1/ai.md](../halo1/ai.md)) | AN3 observes it for `Line` |
| Entity to object | not traced; `0xa5930` and `0xa5980` translate event references and are the place to start | AN4 |

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
| **P5** | What do actors cost in CPU? | The same four runs: `HOST: host.metrics` and `host.perf` | tick time by actor count; the per-actor slope sets the caps |
| **P6** | Can a machine join a distributed game in progress? | With `H2_FEATURES=+mp-jip-probe` (AN9) on the host: start, then join from the second instance. `HOST: net.replication`; `CLIENT: sim.session`, `sim.objects` | the client reaches `mp_client` and its object counts match the host's entity counts |

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
| **AN5** | Transport: the tunnel frame kind, send on the host's post-tick, receive queue on clients, `Key` on join and on request | S | AN2 | `crates/live/live-hle/src/tunnel*.rs`, `crates/extensions/game/src/actor_net/link.rs` | `frames_of_another_kind_are_ignored`, `a_key_request_is_answered_once_a_second_at_most`, `no_tunnel_means_no_channel`, `lost_actor_frames_converge_on_the_next_key`, `reordered_actor_frames_cannot_rewind_a_newer_key`, `a_tunnel_down_retires_the_peer_and_rejoin_gets_a_key`, `a_client_without_the_channel_keeps_receiving_retail_datagrams` | two fake peers over the simulated link (`crates/live/coop`'s) converge under loss and reorder |
| **AN6** | Actor events: `SimEvent::ActorKilled {unit, character, killer, weapon}`, the film `Event` variant, the telemetry row | S | nothing | `crates/rewrites/sim-objects/src/events.rs`, `crates/live/live-proto/src/film/events.rs`, `crates/live/live-telemetry/` | `a_fatal_damage_to_an_actor_unit_is_one_actor_killed`, `a_player_death_is_not_an_actor_kill`, `actor_killed_round_trips_in_a_film` | on a fake guest, one fatal damage report on a unit with an actor yields exactly one event with the killer's player |
| **AN7** | Films and digests: the host-log record tag, recording and playback apply, `Digest` build and check, the client's resync | M | AN1, AN2, AN5 | `crates/live/live-proto/src/film/stream.rs`, `crates/live/film/`, `actor_net/digest.rs` | `a_film_replays_entries_at_their_ticks`, `playback_never_runs_a_producer`, `a_wrong_head_asks_for_a_key`, `three_mismatches_end_the_session` | a recorded fake session replays to the same log head and mode state hash |
| **AN8** | `actors.net`, its doc section and `h2 agent` examples | S | AN3, AN4, AN5 | `crates/extensions/agent/src/actors.rs`, `agent-api.md` | `host_reports_per_client_rates`, `client_reports_puppets_and_waiting` | the method answers on a fake host and a fake client |
| **AN9** | `mp-jip-probe`: lift the in-game join refusal on a distributed host behind a feature, log each stage | S | P1 | `crates/extensions/game/src/mp_jip_probe.rs` | `the_refusal_is_lifted_only_for_a_distributed_session_with_the_feature_on` | P6 can be run |
| **AN10** | Distributed join in progress, built from P6's answer | M | AN9, P6 | decided by P6 | decided by P6 | a third fake peer joining gets a `Key` and equal puppet tables |

AN0, AN1, AN2 and AN6 start at once and touch no existing behaviour.
AN3, AN4 and AN5 follow AN2 in parallel. Probes P1 and P6 need only AN0
(and AN9); P2 to P5 wait for the probe map.

## Rules and limits

- A client never runs `ai_update` and never decides damage.
- A distributed session with actors needs every machine on the same map
  package; the capability mark of forge-design.md refuses a joiner
  without it.
- No creatures in a distributed session.
- The actor channel is Live-only. On System Link, actors have no
  dialogue and no posture.
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

- Everything probes P1 to P6 ask.
- That generated sectors are accepted by the engine and actors walk on
  them ([pathfinding.md](../formats/pathfinding.md)).
- Who judges a client's shot at an actor, and with what lag.
- That an actor on a team no player holds fights players under a
  multiplayer game engine.
- Retail's encoder bit widths and per-client send budget (P4).

## Related

- [firefight.md](firefight.md): the mode built on this.
- [forge-design.md](forge-design.md), [forge-features.md](forge-features.md): the host log's other user, and F15.
- [live-coop.md](../live/live-coop.md): the synchronous model.
- [bots.md](bots.md): Clankers are players, not actors.
- [theater.md](theater.md), [input-film.md](input-film.md), [film-format.md](film-format.md): films.
- [dedicated-host.md](../live/dedicated-host.md): host cost.
- [halo1/ai.md](../halo1/ai.md): the scenario's AI blocks.
