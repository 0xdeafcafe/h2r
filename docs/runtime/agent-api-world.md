# Agent API: the world, Havok and Superbounce views

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

## `world`: what is in the level

Objects come from `live-telemetry::memory` and `live_telemetry::things`,
read at the tick boundary. Ids are `player.<index>` or `<kind>.<datum>`
with the datum handle as 8 hex digits, salt included, so a slot reused by
another object answers `GONE` and never another object's data.

| Method | Params | Result |
|---|---|---|
| `world.count` | `{group_by: "kind" \| "team"}` | `{at, total, groups}`: the cheap first call |
| `world.query` | `{kind?, near?, tag?, fields?, limit?, offset?}` | `{at, total, offset, objects, more}` |
| `world.object` | `{id}` | `{at, object}`: every field the reader has for one object |

- **Kinds** (`agent_proto::Kind`): `player`, `actor` (AI biped), `vehicle`,
  `machine`, `body` (a dead biped), `scenery`, `weapon` and `equipment`
  (lying in the world; held ones are their unit's `weapon`), `control` (a
  device's switch or panel), `crate`, `creature`. The last six come from a
  second pass over the object header table by the header's type byte
  (`Kind::of_object_type`). Projectiles, effects, particles, garbage,
  light fixtures and sound scenery are not listed.
- **Query.** `near` is `{point | player, radius}` and sorts nearest first,
  adding `distance`. `fields` trims each object (`id` and `kind` stay).
  `tag` keeps one definition tag: a datum (8 hex digits, `0x` optional) or
  text its name contains (any case, `/` for `\`). Pages default to 50, at
  most 500.
- **Tag names.** An object with a `tag` also has `tag_name`, the
  definition's path from the loaded map file's names (as `edit.tags`
  reads them); it is absent when the names cannot be read. A name filter
  then matches nothing. The map file is found as `boot::map_override::map_file`
  finds it: by file name first, then by the cache header's name (`+0x198`)
  among the maps folder's files, so a converted map served under a disc
  map's file name is the one read. With two files of the same header name
  the first in folder order wins. A map installed as content on the data
  partition is not searched.
- **Fields.** `pos`, `velocity`, `team`, `tag`, `tag_name`, `vehicle`,
  `weapon`, `name` and `local_user` and `unit` (players), `mounted_on` (a
  turret is a vehicle with `mounted_on` set; its `pos` and `forward` are
  relative to that vehicle), `occupied`, `placement` (the scenario
  placement that made it), `position` (a machine's device position, 0 to
  1) and `physics`.
- **Facing.** `forward` is where an object faces. For a unit it is the
  aiming vector (unit `+0x168`, the vector the held weapon's targeting ray
  follows, `0x000cf3d0`), which turns with the camera; a biped's body turns
  only as it moves, so its own forward (`+0x70`) is `body_forward`. `yaw`
  (degrees about +z from +x, 0 to 360) and `pitch` (degrees up) are
  `forward` as angles. A unit whose aim is not finite or not unit length
  falls back to the body's forward.
- **`physics`** (units): `crouch`, `crouch_request` (unit flags bit 23,
  which stays set after release until there is room to stand), `mode`,
  `airborne` (biped `+0x348` bit 0, likely) and `airborne_ticks`
  (`+0x399`), `hitbox_offset_ticks` and `hitbox_offset` (the offset a melee
  lunge leaves, `+0x3aa`; [superbounce.md](../game/superbounce.md)),
  `unit_flags`, `biped_flags`, `havok_pos` and `havok_velocity` (the first
  Havok body; [`havok`](#havok-the-physics-world) has every body and
  contact), and for bipeds `support`: what the biped physics update
  (`0x000dd990`) keeps of the ground (layout in `live-telemetry`'s
  `layout.rs`, `BIPED_SUPPORT_*` and `CHARACTER_CONTACT_*`). `support.flags`
  is biped `+0x349` (`copied`: its bit 4, set when this tick's update copied
  a support contact, `0x000ddeda`); `words` are `+0x354..+0x364` (all
  `ffffffff` for nothing); `point`, `normal`, `contact_point` are `+0x368`,
  `+0x374`, `+0x380`; `ground_state` is `+0x3ec`, the vector the ground
  test `0x001f19c0` measures first; `contacts` are the character contacts
  that test walks (physics component `+0x88`, count `+0x8c`, `0x48` each, at
  most 32): `key`, `other`, `point`, `normal`, `value` (the first pass
  wants at least 1e-4), `flags`. The airborne bit is decided from these,
  unlike `havok.contacts`, which lists the solver's contact constraints.
  Bipeds also get `standing_fits`: whether a standing capsule fits at the
  origin, the test that decides whether letting go of crouch clears
  `crouch_request`. It is the superbounce mode's capsule test against the
  loaded map's collision (`crates/extensions/game/src/superbounce_capsule.rs`),
  so it is there only while that mode is on and its scan is ready
  ([superbounce.md](../game/superbounce-landings.md#charge)).
  Poll `object player.0` on one `h2 agent -` connection and keep one reply
  per stamped tick for a movement trace, or use [a trace](agent-api-checks.md#tick-traces-and-the-flight-recorder).

```sh
h2 agent count team
h2 agent query '{"kind":["scenery"],"tag":"<part of a tag name>","near":{"player":0,"radius":500},"fields":["pos","tag_name"]}'
h2 agent object player.0
```

## `world.breakable`: broken glass

`world.breakable` reads one structure BSP's breakable-surface state at the
tick boundary: which pieces of breakable glass are broken. The bots'
break step reads the same state (`bots::guest::breakable`;
[bots.md](../game/bots.md#breaking-the-glass)).

| Params | Result |
|---|---|
| `{bsp?}` (default 0) | `{bsp, enabled, broken, mask}` |

- `enabled` is the byte `breakable_surfaces_enable` writes, the first byte
  of the state at `[0x4ed280]`.
- `mask` is the BSP's 32 bytes from `[0x4ed280] + 1 + 32 * bsp`, lowest
  breakable index first, in hexadecimal. A set bit is a whole surface.
  The game fills the mask with ones at map reset and clears a bit when its
  surface breaks ([collision.md](../formats/collision.md#what-the-engine-does-with-a-bsp)).
- `broken` lists the indices whose bits are clear, ascending. A surface's
  index is its collision surface's `+5` byte, used when flag bit 3 is set.
- `UNAVAILABLE` while the state's pointer is null (no map) or the read
  faults. A stray parameter is `INVALID_PARAMS`.

```sh
h2 agent world.breakable '{}'
```

## `world.pools`

`h2 agent world.pools` returns `{at, actors, decals, particles}`: the live
count in each data array's header (`crates/extensions/agent/src/pools.rs`).
It takes no parameters and reads three headers, so it is cheap to poll. A
pool that is not allocated, or whose header fails its signature or capacity
check, is `null`. The watermark's `full` level reads it for its AI, decal and
particle counts ([app.md](app.md#watermark)).

## `world.ai`

`h2 agent world.ai` returns `{at, globals, actors}` at one game boundary and takes
no parameters (`crates/extensions/agent/src/ai.rs`). `globals` is `{on, initialised}`,
the two bytes of the AI globals (`[0x4f55d0]`, `+0` and `+1`) that
`ai_update` (`0x1c83c0`) needs set before it runs any actor, or null
before the globals exist. Each allocated actor has its
salted AI `datum`, its `unit` ID (for `world.object`), character tag
datum, team, scenario squad index, current behaviour ID, its retail name
when available (`behavior_name`), and combat status. A missing unit,
squad or behaviour is JSON null. The actor datum and unit ID have separate
lifetimes: compare the same unit across ticks to measure movement, weapon
and vehicle changes.

`sector` is the actor's cached sector index in the active BSP when in
range, otherwise null; `location_cached` distinguishes a pending refresh
from a completed lookup. The observer only reads this cache. Retail
`0x0026c180` refreshes it, and `0x00297e15` passes its sector to the mesh
walk at `0x0026c590`, which bounds it against the mesh's sector count. The
behaviour ID comes from the top of the four-frame stack used at
`0x001a813d`; its label from the loaded executable's parallel name table
at `0x0046f0c8`. Combat status is the signed value the `ai_combat_status`
builtin reads per actor (`0x00276070`). `focus_object` follows the
actor's selected perception record to the object datum used at
`0x001b368a`; the fight behaviour reads the same record at `0x001af0c2`.
Compare it with the player's `unit` in `world.query` to see attention on
the player. A remembered object can stay selected after contact is lost.

## `net.replication`

`h2 agent net.replication` takes no parameters and returns, at one game
boundary, what the distributed simulation is replicating
(`crates/extensions/agent/src/net.rs`). It is the measuring method for the
probes of [ai-networking.md](../game/ai-networking.md); the types and
fields it names are in that page's
[notes](../game/notes/ai-networking.md).

| Field | Meaning |
|---|---|
| `world_mode`, `mode` | the simulation world's mode (`[0x4cf77c]+8`) and its name: `none`, `local`, `sync-server`, `sync-client`, `dist-server`, `dist-client`; null before the simulation exists |
| `distributed` | true in modes 4 and 5 |
| `objects` | per object tag group (`bipd`, `vehi`, `proj`, ...; `other` for a type outside the table): `{objects, with_entity}`, the live objects and how many have a replication entity (object body `+0xd4` is not -1) |
| `entities` | the sum of `with_entity` |
| `creature_type` | the registered creature probe slot, 17, or null when the definition is absent |
| `creatures` | live bindings `[{entity, type, object, updates}]`. `entity` includes the generation, and `object` is the local salted handle in the format used by `sim.objects`. `updates` counts movement operations the host writes to peer replication batches or the client applies over this binding's lifetime |
| `events` | `{total, by_type: [{type, name, count}]}`: replication events queued since the process started, by the engine's event type; types never seen are left out, and a type outside the table is counted under `other` with a null `type` |
| `traffic` | `{sent, sent_bytes, received, received_bytes, largest}`: the title's datagrams since the process started (`live_hle::traffic`) |

Event and traffic counts accumulate over the process lifetime. Divide
the difference between two calls by the ticks between their `at` stamps
to get a rate:

```sh
h2 agent net.replication > before.json
h2 agent wait '{"map_ticks":900}' 60000
h2 agent net.replication > after.json
```

The event counts come from the `agent-net` feature, an observer on the
engine's event entry (`SIM_EVENT_GENERATE`, `0xb5a70`);
`H2_FEATURES=-agent-net` leaves them at zero and the rest of the answer
unchanged. The engine calls that entry only in a distributed game. On a
host and a client of one game, `objects` answers whether an object the
host made reached the client: the host's `with_entity` rises and so does
the client's `objects` for that group.

## `actors.net`

`h2 agent actors.net` takes no parameters and returns the actor channel
of [ai-networking.md](../game/ai-networking.md) as this machine ran it
on the last tick (`crates/extensions/agent/src/actors.rs`). The game
publishes the report once a tick (`game::actor_net::report`): a
distributed host after it schedules its peers, a distributed client
after it takes the tick's packets.

| Field | Meaning |
|---|---|
| `role` | `host` on a distributed server, `client` on a distributed client, `off` elsewhere or with `-actor-net` |
| `tick` | the simulation tick of the report; null when `role` is `off` |
| `peers` | on the host, each Live tunnel peer with the channel, in id order: `{peer, bytes_per_second, packets, bytes, keys, digests, records, dropped}`. `bytes_per_second` is the bytes charged in the last 30 host ticks, headers included; the rest count since the peer connected. `records` includes repeated copies of lines and cues; `dropped` counts offered records the budget left out. Empty on a client and on a System Link host |
| `client` | on a client, `{actors, puppets, waiting, lines_waiting, mode_state_bytes, digest_misses, key_requested, diverged, host_log_head}`; null elsewhere |

On a client, `actors` counts the actors in the replica and `puppets`
lists those whose body has arrived through retail replication as
`{entity, character, squad, team, posture, unit}`, with `unit` the local
salted handle. `waiting` is the actors whose body has not arrived and
`lines_waiting` the dialogue lines waiting for their speaker.
`digest_misses` is the current run of disagreeing digests,
`key_requested` a key wanted and not yet asked for, and `diverged` the
channel closed for the match (`actor_channel_diverged`).
`host_log_head` is the last digest's host log head in 64 hex digits,
null before the first digest.

To check that both ends of a Live Firefight agree, read the host's
`firefight.state` `log.hash` and the client's `client.host_log_head`
within 30 ticks of each other:

```sh
h2 agent --socket "$HOST" actors.net
h2 agent --socket "$CLIENT" actors.net
```

## `world.raw`: an object's bytes, before and after

A game method, read only, no claim (`agent::raw::install`; reader
`crates/live/live-telemetry/src/raw.rs`). It exists to look for state a forced
crouch leaves behind ([superbounce.md](../game/superbounce.md)).

`world.raw {id, parts?, len?}` returns `id`, `datum` and `parts`, each
`{part, base, hex}` or `{part, missing}` (why: an unset pointer, a fault).
Each part is found through the one before it ([havok-runtime.md](../engine/havok-runtime.md)):

| Part | Where | Bytes |
|---|---|---|
| `object` | the object's body (`player.N`: its unit) | the header's size (`+0x06`), else `len` (default `0x400`), at most `0x2000` |
| `physics_component` | body `+0xb4` into the component array | `0xa0` |
| `entity` | the component's first rigid body (`+0x70`, `+0x40`) | `0x200` |
| `motion` | entity `+0x3c` | `0x140` |
| `shape` | entity `+0x0c` | `0x80` |

`h2 agent raw-diff a.json b.json` needs no game: per part it prints the
32-bit words that differ, with their offset from the part's base, and
both as floats where they read as ones. A part on one side only, or whose
base moved, is listed, not compared. It reads what `h2 agent raw` prints,
or a `-` script's last line (`crates/apps/cli/src/agent_raw.rs`).

```sh
h2 agent raw player.0 > before.json             # world.raw {"id":"player.0"}
# ... force a crouch, walk out, stand ...
h2 agent raw player.0 > after.json
h2 agent raw-diff before.json after.json
h2 agent raw player.0 entity,shape              # some parts
h2 agent world.raw '{"id":"player.0","parts":["object"],"len":2048}'
```

## `world.poke`: replace bytes in one object part

`world.poke {id, part?, offset, hex}` (access: claim) writes 1 to 64 bytes
at a tick boundary. `id` and `part` use `world.raw`'s names; `part`
defaults to `object`. `offset` is a nonnegative byte offset from the
part's base; `hex` is two hex digits per byte. The whole range must fit
in the part `world.raw` reads with its default lengths.

The connection must hold `session.claim`. `sim.session` reports whether
the session allows host writes; a session refusal is `INVALID_REQUEST`
with `data.session`, and a request served between slices is `UNAVAILABLE`
when the session allows writes. The result has `id`, `datum`, `part`,
`base` (the part's first address), `offset`, `at`, and `before` and
`after` (the bytes at the offset, lowercase hex).

```sh
h2 agent poke <id> <part> <offset> <hex>    # takes a claim for its connection; offset decimal or 0x
```

## `havok`: the physics world

Game methods (`agent::havok::install`), read only, answered at a tick
boundary. Layout and evidence: [havok-runtime.md](../engine/havok-runtime.md).

| Method | Params | Result |
|---|---|---|
| `havok.bodies` | `{near?, radius?, limit?}` | rigid bodies: `entity` (address, the key contacts use), `state` (`active`, `inactive`, `fixed`), `object` and `datum`, `pos`, `velocity`, `angular_velocity`, `inv_mass`, `mass`, `friction`, `restitution`, and `shape` where it reads: `addr`, `vtable`, `kind` (`capsule`: `a`, `b`, `radius`, `height`; `sphere`: `radius`; `transform`: `translation`, `child`; else `other`). `near` (an object id) keeps bodies within `radius` (default 5) of its body, nearest first, with `distance`; `limit` defaults to 64 |
| `havok.agents` | `{object}` | collision pairs and cached state ([below](#havokagents)) |
| `havok.contacts` | `{object?}` | contact constraints: `a`, `b` (`entity`, `object`, `datum`, `fixed` for the level and static bodies) and `points`: `pos`, `normal` (from `b` toward `a`, **S**), `distance` (negative is penetration), `impulse` (what the solver applied on its last step), `friction`, `restitution`. With `object`, only its contacts, with it as `a` |
| `havok.digest` | `{}` (any field is refused) | the ordered heap walk ([havok-step.md](../engine/havok-step.md#the-digest) lists what it covers): `digest` (16 hex digits, FNV-1a over every live byte and address), `bodies`, `constraints`, `pairs`, and `heap_used` (bytes in occupied pool slots). The arena hash does not cover the Havok heap, so this digest is what proves a physics rewrite; the sim report carries the same walk as its `havok.world` component |
| `havok.islands` | `{}` (any field is refused) | the island solve's counters since the process started ([havok-step.md](../engine/havok-step.md#the-island-solve)): `solved` (islands whose solve ran in Rust, at its entry or inside an answered integrate), `declined` (islands sent to the lifted solve) and `fallbacks`, every reason in order with its `count`: `memory`, `overflow`, `motion`, `constraint`, `tables`, `solver`, `float`; and eight answering arms ([havok-integrate.md](../engine/havok-integrate.md), [havok-structure.md](../engine/havok-structure.md), [havok-convex.md](../engine/havok-convex.md)), each with `answered` (every call its model captures, from the first), `declined` (calls that ran the lifted entry) and its `fallbacks` by reason: `integrate`, the island integrate (`memory`, `action`, `motion`, `float`, `solve`, `monitor`, `unobserved`); `deactivation`, the deactivation test (`memory`, `deactivator`, `object`, `float`, `monitor`, `unobserved`); `phase`, the integrate phase (`memory`, `pending`, `deactivate`, `listener`, `split`, `island`, `float`, `monitor`, `unobserved`); `polygon`, the structure polygon builder (`memory`, `ring`, `float`, `unobserved`); `convex`, the convex agent's process call (`memory`, `closest`, `points`, `float`, `monitor`, `unobserved`); `query`, the closest-feature query (`memory`, `shape`, `float`, `monitor`, `unobserved`); `point`, the add-point call (`memory`, `commit`, `float`, `unobserved`); and `commit`, the contact commit to its listener walk (`memory`, `full`, `first`, `grow`, `motion`, `island`, `float`, `unobserved`) |

```sh
h2 agent havok.agents '{"object":"player.0"}'
h2 agent havok.contacts '{"object":"player.0"}'
h2 agent havok.bodies '{"near":"player.0","radius":3}'
```

For a superbounce, poll `havok.contacts {object: player.0}` once a tick on
one `h2 agent -` connection next to `object player.0`: the contact with
`b.fixed` is the level, and `impulse` over the body's `mass` is the
velocity it gave. A type-`0x17` wrapper reports `kind: "other"` in
`havok.bodies`; its current pill is the pointer at `shape+0x30`
([Shapes](../engine/havok-runtime.md#shapes)). A direct biped capsule's
`shape.height` tells standing from crouching.

### `havok.agents`

`object` is required. The reply has `object`, a stamp (`at`) and `agents`.
Each entry is a pair with an allocated agent on any rigid body of that
object. A live object with no such pair returns an empty list. A
malformed selector is `INVALID_PARAMS`; an absent object or a player
between lives is `GONE`.

- `entry`, `island` and `collidables: [a,b]` are guest addresses in native
  pair order. `object_side` is `a` or `b`, and `entity` is the requested
  object's participating rigid body. `other` identifies the partner
  entity, with its `datum`, object ID and fixed-island flag where
  available; it is omitted for a partner without an entity. `pair_word`
  keeps the last four bytes of the pair record.
- `cache` has `base`, `vtable`, `manager`, `kind` and `hex`. Addresses,
  keys and raw words are eight lowercase hex digits. `hex` is two digits
  per guest byte from `base`, including the allocation header and padding.
- A `bv_tree` or `welded_bv_tree` also has `tree`: `min` and `max` (the
  cached query box in the tree's frame) and, per immediate child, `entry`,
  `key`, raw `word_04` and its own `cache`. A type-1 structure MOPP key
  also has `surface`, the numeric collision BSP surface index. A filtered
  child is `kind: "empty"` with its shared header.
- A `convex` or `swept_convex` cache has a `convex` block: `vertices_a`
  and `vertices_b` (active support vertex IDs in that agent's dispatch
  order; for convex against tree, A is the convex body and B the selected
  tree shape), `contact_ids` (the child manifold's IDs in the shared
  manager), `separating_axis` and `separating_distance` (the bound used to
  skip a closest-feature search while positive; non-finite values are
  null). `swept_sides` on the swept agent records A in bit 0 and B in
  bit 1.
- Unrecognised agents are `kind: "other"` with their inline bytes. The
  bounds are 32,768 scanned pairs, 512 children per tree, 4 KiB per
  allocation and 256 KiB of raw cache bytes per request; a failed read or
  an exceeded bound is `UNAVAILABLE`.

To compare ticks around a crouch, match a child by its partner and
complete `key`, and record both agent bases to see replacements.
Allocators can reuse an address between samples, and the raw dump
contains pointers, unused slots and padding. A cached surface with an
empty manifold is still a candidate in the query box.

### `havok.pair.set`

`havok.pair.set {object, entry, word}` (access: claim) overwrites one
pair's word (pair `+0x0c`, [havok-runtime.md](../engine/havok-runtime.md))
at a tick boundary. `entry` is the pair's `entry` from `havok.agents` for
the same object; without it, the write goes to the object's one pair
against a fixed body (the level). `word` is eight hex digits, as
`pair_word` prints them. It writes only a pair `havok.agents` would list
for that object at that tick; another `entry`, or no single level pair,
is `GONE`, and so is an absent object or a player between lives. The reply has `object`, `entry`, `before` and
`after`. It needs the same claim and host-write session as `world.poke`
(`agent::havok::install_writes`; writer `set_pair_word` in
`crates/live/live-telemetry/src/havok_agents.rs`). It exists to test
whether the word a forced crouch leaves behind is what makes a superbounce
([superbounce.md](../game/superbounce.md)).

```sh
printf '%s\n' claim 'havok.pair.set {"object":"player.0","word":"fffffffd"}' | h2 agent -
```

## `superbounce`: the superbounce game type

`superbounce.state` (access: read; it takes no parameters and refuses any
with `INVALID_PARAMS`) answers from the state the
`superbounce` feature publishes each tick
(`crates/extensions/game/src/superbounce_mode.rs`; wire types in
`crates/extensions/agent-proto/src/superbounce.rs`; registered in
`crates/extensions/game/src/agent_glue.rs`). The mode is described in
[superbounce.md](../game/superbounce.md#the-superbounce-game-type).

| Field | Meaning |
|---|---|
| `active` | the mode is on: the game's script requires `hud.superbounce` (Superbounce, Bounce Practice) and `superbounce.set` has not forced it off. Everything below is empty or `idle` while it is off |
| `primed` | the local player's level pair word is -3 |
| `word`, `word_signed` | the pair word as eight hex digits and as a signed number; absent without a level pair (between lives) |
| `position` | the local player's position, world units |
| `nearest` | the bounce spot nearest the player: `distance` to its edge, `launch` (units/s), `fall` (units) and its `edge` |
| `scan` | `status` (`idle`, `scanning`, `ready`, `failed`), `map`, `error`, and the `bounce` and `crouch` spot counts |
| `airborne` | the local player's biped is off the ground (biped flags bit 0, as `world.object`'s `physics.airborne`); absent without a player |
| `vz` | the local player's vertical velocity, world units a second, up positive |
| `yaw` | the way the local player's unit aims, radians from +X toward +Y: the first-person camera's yaw |
| `velocity` | the local player's velocity, world units a second; absent without a player |
| `options` | the HUD's pieces this game, the Superbounce script's options as the variant set them: `targets` (`common`, `likely`, `all`) and the bools `velocity`, `alignment`, `miss_info`, `medals`, `second_bounce`, `approach_guide`, `charge_segments` ([superbounce.md](../game/superbounce.md#the-huds-options)) |
| `targets` | the seam targets ([superbounce.md](../game/superbounce.md#targets)): `mode` (`common`, `likely` or `all`: the option's value, or the kinds the player switched to with a double tap of the flashlight), `count` (the seams `mode` shows on this map), `tag` (the tagged seam's point; absent with none) `marked` (the seams with a target navpoint, two at most: `second`'s in the flight after a bounce, none otherwise; the set's other seams are lines on the map) and `second` (absent when empty: in the flight after a bounce, the seams it can reach, the best first, each with `at`, `time`, `steer`, `reach`, `speed`, `need`, `cost` and `caught`; [superbounce.md](../game/superbounce-game-type.md#the-second-bounce)) |
| `charge` | the charge meter: `phase` (`near`, `under`, `held`, `primed`, `lost`), `fill` (0, 1/3, 2/3, then 1 less `still_ticks` over 38 while primed, 0 while lost) and `still_ticks`; absent while hidden ([superbounce.md](../game/superbounce-landings.md#charge)) |
| `verb` | the prompt line's verb: `let_go_of_crouch` while `under`, `walk_out_to_stand` while `held`, `keep_moving` while primed and still for 19 ticks or more, `charge_again` for 150 ticks after a loss while no crouch step has begun |
| `lost_reason` | why the charge went, while `lost` and while the verb is `charge_again`: `stood_still` (the word left −3 after 30 still ticks or more), `crouched` (a crouch while primed), `unknown` (the word left −3 for another reason); absent otherwise ([superbounce.md](../game/superbounce-landings.md#charge)) |
| `nav_target` | the navpoint's target: `at` (the spot's stand point), `kind` (`crouch` while not primed, `strip` while primed: the chosen strip, [superbounce.md](../game/superbounce.md#floor-marks)), `distance` (world units), `fall` (a strip's drop, world units; 0 for a crouch spot) |
| `takeoff` | while primed on the ground, where to leave from for the chosen strip, on its floor ([The takeoff](../game/superbounce.md#the-takeoff)); absent when none is found. The world's bar is drawn there |
| `guide` | the [approach guide](../game/superbounce.md#the-approach-guide) on the seam the player is going for, absent while it is not shown: `seam` (its landing point), `call` (`off_line`, `lined_up`, `too_shallow`, `too_high`, `off_edge`, `no_drift`, `crouching`, `will_bounce`), `jump` (the player is on the ground: the landing is the jump's from the takeoff's line), `impact` (the pipper), `row` (the last free row), `ticks` to it and `speed` on it (units/s down), `caught` (within the catch), `deep` with `depth` (the margin under the depth cut; negative is too shallow) and `past` (the row's place past the edge along the drift), `across` (the speed along the drift), `chance` (the bounce's predicted [chance](../game/superbounce-guide.md#the-chance) in a fall, 0 to 1: the call's confidence) and `chance_capped` (the seam has no measured landing and `chance` was cut to 0.6), on the ground the [jump cue](../game/superbounce.md#the-jump-cue): `jump_in` (game ticks until the box says `Jump now`, 0 on the line, while it counts), `cue` (the box's words, settled: `Climb up`, `Walk to line`, `Jump in 0.7 s`, `Jump now` or `Walk off`) and `takeoff` (the [takeoff](../game/superbounce.md#the-takeoff): `at`, the line's centre on its floor, `heading`, the way to run over it, `half`, half the band along it, `speed`, the ground speed it is for, and `walk`, the cue is not a count), and `strength` (the fade, 0 to 1). The fields from `impact` to `speed` are absent with no landing by the seam, `deep`, `depth`, `past` and `across` on a seam with no drift, `chance` on the ground, while not primed, on a seam with no drift, with no landing by the seam and in a fall slower than 5.2 units/s, `impact` to `speed` on the ground too unless the cue counts, `takeoff` and `cue` in the air, while not primed and where no takeoff is found for the seam, and `jump_in` too while the cue is not a count. The guide is absent while not primed |
| `landing` | while airborne and falling toward a strip, the landing the assist predicts: `at` (at the strip's floor height), `verdict` (`on_slot`, `fixable`, `miss`), `speed` (the touchdown speed, units/s), the strip's `need`, `miss` (world units outside the slot, 0 inside) and `locked` (in the slot at `need` or faster: the [lock-on](../game/superbounce.md#lock-on) is red) |
| `last_landing` | the last touchdown within 0.5 of a strip, or primed bounce anywhere, filed when its launch window (the touchdown tick and the 3 after) closes, or a primed fall through a strip's floor, filed on the tick it crosses: `result` (`bounce`, `not_primed`, `missed`, `too_slow`, `no_bounce`, `through`), the touchdown's game `tick` (for `through`, the touchdown's it followed, else the crossing's) and the origin `at` on the tick before ([superbounce.md](../game/superbounce-landings.md#landings)) |
| `off_spot` | the local player's last 16 bounces no marked strip accounts for (none within 0.5, or outside the catch of the one there is), oldest first; absent when none: the touchdown's `tick`, the origin `at`, the `edge` of the scanned bounce spot within 0.5 (absent with none), `speed_in` and `launch` ([superbounce.md](../game/superbounce-landings.md#landings)) |
| `standing_fits` | whether the standing capsule fits at the player's origin, as `world.object`'s `physics.standing_fits`; absent before the scan is ready |
| `medals` | every player's medals this game, bots included ([superbounce.md](../game/superbounce.md#medals)): `players`, a row for each slot that has a chain or a medal (`player` the slot, `chain` its last chain's count, `live` whether its next bounce adds to it, `counts` each medal id's total), and `recent`, the last 8 awards, oldest first (`player`, `tick`, `chain`, `medals`: the ids, lowest first; and for a bounce `strips`, how many bounce strips were within 0.5 of its touchdown). The ids are `superbounce`, `double-bounce`, `triple-bounce`, `quad-bounce`, `chain-bounce`, `bouncetacular`, `bounced` and `monster-bounce` |

```sh
printf '%s\n' 'superbounce.state' | h2 agent -
```

`superbounce.set {"on": false|null}` (access: act) forces the mode off in
any game (`false`), or follows the game again (`null` or `{}`: its
script). It cannot turn the mode on: `true` is refused with
`invalid_params`, because only a game whose script requires
`hud.superbounce` runs the mode. The answer is `{"mode": "off"|"game"}`.
The tick's on/off check runs every 30 ticks, so `superbounce.state` shows
the change within a second.

```sh
h2 agent superbounce.set '{"on":false}'
```

`superbounce.medal_test {"chain": N}` or `{"bounced": true}` (access: act)
files a bounce at chain count `N` (1 and up), or a Bounced, for the first
local player with a unit, by the chain's rule
(`superbounce_chain::bounce_medals_with`); `"monster": true` with `chain`
files it as a Monster Bounce. The chain is left at `N` and live,
as after a real bounce; a Bounced ends it. The award reaches the HUD (its
medal line and notification) and `superbounce.state`'s `medals` as a real
one does. The answer is `{"player", "chain", "medals", "line"}`: the slot,
the count, the ids filed and the HUD's line (the highest medal's glyph and
words). It refuses with the mode off, with no local player, with `chain`
0, with both or neither of `chain` and `bounced`, or with `monster` and
`bounced`.

```sh
h2 agent superbounce.medal_test '{"chain":6}'
# {"player":0,"chain":6,"medals":["superbounce","bouncetacular"],"line":"\ue2a5 Bouncetacular!"}
h2 agent superbounce.medal_test '{"chain":2,"monster":true}'
# {"player":0,"chain":2,"medals":["superbounce","double-bounce","monster-bounce"],"line":"\ue2a8 Monster bounce!"}
h2 agent superbounce.medal_test '{"bounced":true}'
```

`superbounce.assist` (access: read; no parameters) says what the hold-X fall
assist did at its last pad read
([superbounce.md](../game/superbounce.md#hold-x-the-fall-assist);
`crates/extensions/game/src/superbounce_assist.rs`).

| Field | Meaning |
|---|---|
| `active` | the superbounce mode is on; the assist works only then |
| `enabled` | the player's `superbounce.fall_assist` setting is on; the assist is opt in and never steers while it is off |
| `test_hold` | `superbounce.assist_test` is holding the button |
| `engaged` | the assist steered the player at its last read |
| `reason` | why. Engaged, how the strip was chosen: `kept` (the strip the read before steered onto, still in reach), `marked` (the strip the HUD marks) or `nearest` (the strip the predicted landing misses by least, one the fall is fast enough for first). Held and not engaged: `not_falling`, `no_strip` (none within 6 units), `refused` (every strip near was) or `no_body`. Else `not_held`, `off` (the setting) or `inactive` (the mode) |
| `target` | the chosen strip's stand point, world units |
| `point` | the point of that strip being landed on: the stand point moved along the strip to where the fall comes down |
| `aim` | the point being aimed at, a little before `point` along the drift (X, Y) |
| `landing` | where the player lands if the stick is let go (X, Y) |
| `fall_time` | seconds until touchdown at the strip's height |
| `stick` | the left stick sent to the guest, `[x (right), y (forward)]`, -1 to 1 |
| `from`, `velocity` | the player's origin and velocity at the read; absent while the button is up |
| `candidates` | the strips judged at the read, while the button is held: the chosen one, the others in reach, then the refused, nearest first, at most 12 |
| `chose` | the read the assist last engaged on, the choice at the press: `target`, `point`, `reason`, `from`, `velocity` and `candidates`. Kept after the button is let go, until the next engagement |

`target` to `stick` are absent while not engaged. A candidate:

| Field | Meaning |
|---|---|
| `stand` | the strip's stand point, world units |
| `verdict` | `chosen`; `passed` (in reach, another was chosen); or why it was refused: `above` (not below the player), `long` (a fall over 3 s), `far` (over 3 units sideways), `out_of_reach` (the stick cannot bring the landing onto it in the time left), `covered` (a floor is between the player and it) or `walled` (something solid is between) |
| `distance` | from the player to the strip's edge, sideways, world units |
| `speed`, `need` | the touchdown speed the fall to it gives, and the speed it needs, units/s |
| `point`, `fall_time`, `miss` | in reach only: the point of the strip to land on, the seconds to it, and how far the coasting landing misses the aim |

A wrong pull shows in `chose`: the strip it took, why, and what it made
of the strip the player meant.

`superbounce.assist_test {"hold": true|false}` (access: act) holds the
assist's button as if it were pressed, so an agent can try the assist
without a pad: with the mode and the setting on, a player in the air above
a strip in reach is steered while it is `true`. The answer is
`{"hold": bool}`. It holds until set `false`. The setting is turned on and
off with `settings.set`, live.

```sh
h2 agent settings.set '{"key":"superbounce.fall_assist","value":true}'
h2 agent superbounce.assist_test '{"hold":true}'
h2 agent superbounce.assist
```

`superbounce.verify {kind, ...}` (access: act) starts a batch of trials of
the loaded map's spots in the game, run tick by tick on one seat by an
agent ticker (`Agent::add_ticker`), and answers at once with the plan
([superbounce.md](../game/superbounce.md#verifying-spots);
`crates/extensions/game/src/superbounce_verify.rs`; wire types
`agent_proto::superbounce::Verify*`). One batch runs at a time. It needs
the mode on and its scan ready, except over explicit `points`; the
`player.<n>` seat needs the `superbounce-assist` feature's pad tap, and
every kind but `route` needs the `agent-calls` feature's teleport
(`UNAVAILABLE` otherwise). Every param but `kind` is optional:

| Param | Meaning |
|---|---|
| `kind` | `crouch` (crouch outside the lip, walk in, let go under it, walk out: does the word turn −3?), `drop` (a fall onto a strip with the word set: does it launch?) or `route` (a bot's whole `bots.bounce_to` route) |
| `spots` | the scan's spots of that kind by index, best score first (the order `dry_run` lists); default the first `max_spots` (12) that have not failed |
| `points` | explicit spots instead: `{stand, out?, edge?, drift?, need?}` (a drop's hit is the one catch either side of `edge`, [superbounce.md, "The target: the collision edge"](../game/superbounce.md#the-target-the-collision-edge)); never saved |
| `seat` | `player.<n>` (default `player.0`, driven through the first controller's pad) or a brain-driven bot by gamertag, handle or `player.<n>` (driven by holding its pad); `route` needs a bot |
| `turns`, `distances`, `offsets` | crouch: approach turns in degrees from straight in (default `[0,25,-25]`), starts outside the lip along it (default `[0.3,0.6]`), and X/Y offsets (default `[[0,0]]`) |
| `across`, `along`, `heights` | drop: offsets across the spot's target line toward the drift (default `[-0.1,-0.05,0,0.05,0.1]`), along it (`[0]`), and heights over the strip (`[1.5,2.5,3.5]`) |
| `prime` | drop: `set` (−3, default), `none` (0, the control) or `keep` |
| `drift_stick` | drop: the stick along the drift while falling, 0 to 1 (default 0) |
| `settle_ticks`, `hold_ticks`, `out_ticks`, `fall_ticks`, `route_ticks` | 20, 15, 30, 120, 3600 |
| `to`, `mode`, `repeat` | route: the target (default 6 up from each strip's stand), `help` or `fun` (default), and the routes a spot (3) |
| `trace` | keep the per-tick trace of this trial index (`phase`, `pos`, `vz`, `airborne`, `request`, `crouch_button`, `crouch`, `standing_fits`, `word`) |
| `save` | write the map's verified file at the end (default true; never for `points` or `route`) |
| `dry_run` | plan and list, run nothing |

The answer is `{job, kind, seat, map, spots, trials, estimated_ticks,
dry_run}`: each spot's `index`, `kind`, `stand`, `status` and `score`, and
each trial's `spot`, `start`, `yaw`, `distance`, `offset` and `height`.

`superbounce.verify_status {wait_ticks?, brief?}` (access: read) reports
on the newest batch: `state` (`none`, `running`, `done`, `failed`,
`cancelled`), `job`, `kind`, `seat`, `map`, `done` and `total` trials,
`current` (the trial and its phase), `spots`, `results` (each spot's
`trials`, `ok`, `status` `proven`, `failed` or `untried`, its `best`
trial and `why` the others failed, by count), `trials` (left out with
`brief`), `trace`, `lines` (one a spot), `saved` (the verified file
written) and `error`. With `wait_ticks` (at most 290) it answers when the
batch ends or that many ticks pass. A crouch trial carries
`crouched_first`, `under`, `forced`, `primed_tick`, `primed_at` and
`closest`; a drop `contact_tick`, `speed_in`, `vz_out`, `landed_at`,
`word_before` and `landing`; a route `route`, `steps` and its
touchdown. `superbounce.verify_cancel` (access: act; no params) stops the
batch, lets go of the seat and answers with the report; a cancelled batch
writes nothing.

```sh
h2 agent superbounce.verify '{"kind":"crouch","dry_run":true}'
h2 agent superbounce.verify '{"kind":"crouch","spots":[0,1,2],"trace":0}'
h2 agent superbounce.verify_status '{"wait_ticks":290,"brief":true}'
h2 agent superbounce.verify '{"kind":"drop","prime":"none","spots":[0]}'
```

`superbounce.view` (access: read; no parameters) says what the bounce
view shows ([superbounce.md](../game/superbounce.md#the-bounce-view);
`crates/extensions/game/src/superbounce_view.rs`; wire types in
`crates/extensions/agent-proto/src/superbounce_view.rs`), as of the last
tick, timed now:

| Field | Meaning |
|---|---|
| `active` | the superbounce mode is on; everything below is empty while off |
| `mode` | `none`, `pulse` (a tap's pulse still shows) or `hold` |
| `button` | the flashlight action at the last tick |
| `test_hold` | `superbounce.view_test` is holding |
| `pulse_age`, `pulse_front` | seconds since the pulse left, and its ring's distance from the player in world units; absent without a pulse |
| `grow` | the tracker's growth into the map, 0 to 1 |
| `spots`, `marks`, `hidden`, `tested` | the spots the view knows, those marked in the world now, of those the ones behind a wall, and those with a line-of-sight answer |
| `tracker` | `enabled` (the variant shows the tracker), `range` (world units) and `blips`, the next spot and then the one after it, at most two: each `kind` (`bounce` or `crouch`), `proven`, `x` and `y` (right and ahead over the range; on the unit circle when out of range), `level` (`same`, `above`, `below`), `rim` (out of range) and `nav` (the next spot) |
| `map` | while held or shrinking: `floors`, `edges` and `pairs` drawn, and its `blips` over the map's 50 m |

`superbounce.view_test {"mode": "pulse"|"hold"|"none"}` (access: act)
drives the view with no pad: `pulse` fires a pulse at the next tick,
`hold` holds until `none`, `none` lets go and stops the pulse. The answer
is `{mode, test_hold}`. Refused while the mode is off.

```sh
h2 agent superbounce.view_test '{"mode":"hold"}'
h2 agent superbounce.view
h2 agent superbounce.view_test '{"mode":"none"}'
```

`superbounce.ghosts` (access: read; no parameters) lists the loaded
map's ghost runs: recorded runs that bounced off a spot, the best of
each spot, kept in `<map>.ghosts` in the bots directory
([superbounce.md](../game/superbounce.md#ghost-runs)). Wire types in
`crates/extensions/agent-proto/src/superbounce.rs`; the game side is
`crates/extensions/game/src/superbounce_ghost.rs`.

| Field | Meaning |
|---|---|
| `active` | the mode is on |
| `map` | the map the runs are for, once the mode has ticked on one |
| `file` | the ghosts file's path, once there is a bots directory |
| `error` | why the file could not be read (another version, a bad line); it is not written over then |
| `ghosts` | each run: `index` (what `superbounce.ghost_show` takes), `at` (the spot's stand point), `landing` (the touchdown origin), `source` (`bot`, `verify` or `player`), `ticks` (30 a second), `launch` (units/s up out of the bounce), `crouch_primed` (it primed itself with a crouch), `presses` (`frame` and `button`, `crouch` or `jump`) |
| `looked_at` | the run whose spot the first local player looks at |
| `prompt` | the prompt line's ghost verb, as the HUD writes it (X's glyph, U+E102; B's, U+E101, on the Bumper Jumper layout) |
| `playing` | the playback: `index`, `seconds` since it started by the clock, the run's `tick` then (with the fraction to the next), the figure's `at`, and `done` once it holds on its last frame; absent when none plays |
| `recorded` | runs kept (a new best for their spot) since the game started |

`superbounce.ghost_show {spot?, stop?}` (access: act) plays run `spot`
once from its start, or the run looked at without `spot`; `{"stop":
true}` stops the playback. It answers `playing`, the `index` and the
run's length in `ticks`. Refused while the mode is off, with no map, for
an index the map has no run at, or (to play one) without the
`superbounce-ghost` feature, which is off by default.

```sh
h2 agent superbounce.ghosts
h2 agent superbounce.ghost_show '{"spot":0}'
h2 agent superbounce.ghost_show '{"stop":true}'
```

`superbounce.reload {map?}` (access: act) re-reads a map's `<map>.spots`,
`.learned`, `.verified` and `.ghosts` from the bots directory, for the
loaded map without `map`, and republishes them the way a map load does,
with no restart: the bots' spot table and its generation (the brains read
their spots again), and the mode's own copy of the loaded map's spots,
which its marks, HUD, assist and tracker read. The `.spots` file is taken
when it is the current scanner's (its map fingerprint is not checked); a
missing, damaged or other-version one leaves the scan as it was, and
`spots_file` says so. The mode keeps its own scan of the loaded BSP, so a
changed `.spots` reaches the bots and a changed `.verified` or `.learned`
reaches everything. The call runs on the game thread at a tick boundary.
The answer is `map`, `dir`, `spots_file` (`read`, `absent` or
`unread: <why>`), `before` and `after` (the scan as the bots keep it with
the files applied: `total`, `crouch`, `slot`, `crease`, `vee`, `busy`,
`likely`, `proven`, `failed` and `marked`), `verified_entries`,
`learned_entries`, `ghosts_before`, `ghosts_after` and `mode_updated`.
Refused with no map and no `map`, or with no bots directory. Game side:
`crates/extensions/game/src/superbounce_reload.rs`,
`bots::spots::reload_in`.

`superbounce.tune {bounce_speed?, catch?, launch_gain?}` (access: act)
sets live overrides of the model's constants: the touchdown speed every
seam needs (`superbounce::speed::BOUNCE_SPEED`, above 0), the catch
(`superbounce::CATCH`, world units, 0 up to but not including 1) and the
launch gained per unit/s over the threshold (`speed::EXCESS_GAIN`, 0 or
more). A key left out stays as it is and `null` clears its override. Every
key is checked before any is applied, and an unknown key is refused. The
model reads each constant through one accessor
(`superbounce::speed::bounce_speed`, `excess_gain` and
`superbounce::catch`, over `superbounce::tune`), so an override reaches
the scan's `need`, the launch, the landing verdict, the assist and the
bots at once. Overrides live in memory only and are gone with the
process. The answer is `bounce_speed`, `catch` and `launch_gain`, each
`{value, source}` with `source` `default` or `override`.

```sh
h2 agent superbounce.reload
h2 agent superbounce.reload '{"map":"zanzibar"}'
h2 agent superbounce.tune '{"bounce_speed":5.0,"catch":0.15}'
h2 agent superbounce.tune '{"bounce_speed":null,"catch":null,"launch_gain":null}'
```

## `pogo`: Superbounce's Pogo Stick

`pogo.state` (access: read; no parameters) says what the Pogo Stick is
doing ([pogo-stick.md](../game/pogo-stick.md);
`crates/extensions/game/src/pogo_agent.rs`; wire types in
`crates/extensions/agent-proto/src/pogo.rs`), as of the last tick:

| Field | Meaning |
|---|---|
| `active` | the superbounce mode is on; `riders` is empty while off |
| `option`, `style` | the script's `pogo` and `pogo_style` options as the variant set them: whether a spawn is given a stick, and which look (`unsc`, `covenant`, `forerunner`) |
| `map` | the loaded map's name; absent without one |
| `weapons` | the three sticks' weapon tags in the loaded map, in the order `unsc`, `covenant`, `forerunner`; empty when the map does not carry the kit (the `pogo` feature off, or the map refused), and then no stick is given |
| `riders` | every player holding a stick, the local players first: `player` (the slot), `local`, `unit` and `weapon` (object handles for `world.object`), `style`, `phase` (`carried`, `mount`, `ride`, `compress`, `spring`, `dismount`), `mounted` (0 in the hands to 1 on the stick), `compress` (0 the piston out to 1 in), `flare` (1 on a bounce's tick, fading), `tuck`, `trick` (absent without one: `id`, `name`, `progress` 0 to 1, half at the apex) and `posed` (the riding pose is drawn over the game's own) |
| `camera` | the chase camera: `on` (a local player holds a stick), `drawn` (the last frame was drawn from it; false headless, in split screen and under Forge's or Theater's camera), `room` (the share of the boom the level leaves room for), `share` (the boom's share now, eased) and `fallback_frames` (frames placed from the body's eye and aim because the game's own view did not read as a camera, since the game started) |
| `tricks` | the ids `pogo.test` takes |

`pogo.test {"trick": "<id>"}` (access: act) shows a trick from its start
on the first local player's stick, whatever that player is doing: on the
ground the player is shown on the stick for the trick's length. It is
presentation only: no bounce is filed, and nothing is sent or recorded.
The answer is `{player, trick, name}`. Refused while the superbounce mode
is off, for an id that is no trick, and when no local player holds a
stick.

```sh
h2 agent pogo.state
h2 agent pogo.test '{"trick":"backflip"}'
```
