# Notes: AI over the network

Evidence for [ai-networking.md](../ai-networking.md) and
[firefight.md](../firefight.md). Newest first.

## 2026-11-21: what retail's distributed replication carries (static read)

Commit: `af668a58f`. Method: `h2 disasm <addr> <count>` on the owner's
retail XBE, plus a scan of the image for the cross-references of the
addresses below. Static only: no game was run. To reproduce, disassemble
the named addresses; every row below is a constant a small getter returns
or a store in the routine named.

**The type table.** The routine at `0x82240` fills one table with every
replicated type: 17 entity types at `+4 .. +0x44` and 26 event types from
`+0x88`, and returns the two counts (`0x11`, `0x1a`). The entity part of
the table has room for 33 pointers, so 16 slots are unused. Each entry
points at a definition object whose vtable answers, in slot order from
the vtable's start: type index, debug name, state size, creation record
size, update flag count, initial update mask, then the encode, decode,
compare and apply routines.

Entity types, by index. Names are ours; sizes are the getters' constants.

| # | Type | Vtable | State bytes | Creation bytes | Update flags |
|---:|---|---|---:|---:|---:|
| 0 to 5 | one per game engine's globals (Slayer, CTF, Oddball, King, Territories, Juggernaut) | `0x450fa0`, `0x451058`, `0x451188`, `0x451240`, `0x451318`, `0x451410` | `0x24` to `0x84` | 0 | 5 to 22 |
| 6 | game-engine player | `0x4514e8` | `0x38` | 2 | 11 |
| 7 | statborg (scores) | `0x4516a8` | `0x1b0` | 0 | 24 |
| 8 | breakable surface group | `0x4517a8` | 4 | 2 | 1 |
| 9 | unit | `0x451b18` | `0xf8` | `0x24` | 24 |
| 10 | item | `0x451f68` | `0x94` | `0x10` | 11 |
| 11 | generic object | `0x452030` | `0x90` | `0x14` | 10 |
| 12 | vehicle | `0x4520f0` | `0x90` | `0x14` | 10 |
| 13 | projectile | `0x452210` | `0x94` | `0x20` | 11 |
| 14 | weapon | `0x452568` | `0xa0` | `0x10` | 14 |
| 15 | turret | `0x452788` | `0x90` | `0x14` | 10 |
| 16 | device | `0x452848` | `0x98` | `0x10` | 12 |

There is no biped type: a biped replicates as a unit. There is no
creature type and no actor type.

The ten flags every object type shares, from the apply routine `0xa6430`
(it tests one bit per field of the update it is handed) and the image's
field labels: dead, position, forward and up, scale, translational
velocity, angular velocity, body vitality, shield vitality, region state,
constraint state. A unit adds 14: among them its parent vehicle,
desired aiming vector, desired weapon set, weapon type, weapon state,
grenade counts, active camouflage and a control block. No field is an
animation index, a posture or a sound.

Event types, by index, with payload bytes and the number of entities each
refers to:

| # | Event | Vtable | Payload | Refs |
|---:|---|---|---:|---:|
| 0 | damage aftermath | `0x451874` | `0x38` | 2 |
| 1 | damage section response | `0x451844` | `0xc` | 1 |
| 2 | breakable surface damage | `0x451814` | `0x34` | 0 |
| 3 | projectile attached | `0x4522d0` | `0x18` | 2 |
| 4 | projectile effect | `0x4521e0` | `0x30` | 0 |
| 5 | projectile impact effect | `0x4521b0` | `0x34` | 0 |
| 6 | projectile impact effect on an object | `0x4522a0` | `0x3c` | 1 |
| 7 | game engine event | `0x4529d0` | `0x24` | 0 |
| 8 | unit boards a vehicle | `0x451ba8` | 4 | 2 |
| 9 | unit pickup | `0x451a28` | `0xc` | 1 |
| 10 | weapon effect | `0x452444` | `0x10` | 1 |
| 11 | unit exits a vehicle | `0x451bd8` | 4 | 2 |
| 12 | weapon fire | `0x452504` | `0x44` | 2 |
| 13 | weapon reload | `0x4524a4` | 0 | 1 |
| 14 | grenade throw starts | `0x451ae8` | 2 | 1 |
| 15 | melee starts | `0x4519f8` | 2 | 2 |
| 16 | weapon pickup | `0x452534` | 8 | 2 |
| 17 | weapon put away | `0x4524d4` | 8 | 1 |
| 18 | weapon drop | `0x452474` | 8 | 1 |
| 19 | vehicle flip | `0x451ab8` | 0 | 2 |
| 20 | vehicle trick | `0x451a88` | 4 | 1 |
| 21 | device touch | `0x4528d8` | 0 | 2 |
| 22 | grenade release | `0x451a58` | `0x1c` | 1 |
| 23 | melee damage | `0x451c08` | `0x1c` | 2 |
| 24 | unit enters a vehicle | `0x451c38` | 4 | 2 |
| 25 | request to boot a player | `0x4529a0` | 8 | 0 |

Payload sizes are the in-memory records, not the encoded bits; the
encoders were not read.

**Event generation.** `0xb5a70` is the one entry that queues an event:
type and reference count are its first two stack arguments, then the
reference array, the payload size, the payload and a sixth argument; it
returns with `ret 0x18`. It acts only when the simulation world's mode
(`[0x4cf77c]+8`) is 4 or 5. It has 26 call sites, all in
`0xa7c50 .. 0xa9440`. The weapon-fire site (`0xa7d50`, the call at
`0xa802c`) runs from the weapon's own fire code for any weapon: it finds
the firing unit from the weapon's parent, not from a player, and gives up
only when the unit has no entity.

**Which side runs AI.** `ai_update` (`0x1c83c0`) returns at once when the
AI globals' byte `+1` is clear, when the network mode byte
(`[0x4e6948]+0xc`) is 4 (the distributed client), or when the globals'
byte `+0` is clear (after running the teardown at `0x1e3240` once). So
retail skips AI on a distributed client and on nothing else: a
distributed server, a synchronous server and a synchronous client all run
it when the two bytes are set.

**An object's entity.** Object body `+0xd4` is the entity index, set to
-1 by object creation (`0xb7b40`, the store at `0xb7e78`) and written by
each entity type's attach routine (the unit's at `0x9dea0`).

**Scheduling.** The image's log formats for entity creations and updates
carry a relevance and, for updates, a period. So the sender ranks
entities per client and sends each at its own rate. The per-client byte
budget behind that was not read.

Not read: the encoders (bits per field), the per-client budget, which
objects are given an entity at creation, and how a client turns a unit
entity with no player into movement animation.
