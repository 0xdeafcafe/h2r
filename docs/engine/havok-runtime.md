# Havok at run time: world, bodies and contacts

Status: Reference. Code: `crates/live-telemetry/src/havok.rs`,
`crates/live-telemetry/src/havok_agents.rs`. Switch: none.

How the retail game keeps its Havok world in guest memory, as far as
`havok.bodies`, `havok.contacts` and `havok.agents` read it
([agent-api.md](../runtime/agent-api.md), "havok"). It serves
the superbounce work ([superbounce.md](../game/superbounce.md)): which
surface pushed the biped's body, and how hard.

Static reading of `default.xbe` with `h2 disasm`; nothing here was
checked in a running game for these layout notes. Synthetic fixtures for
the two readers live in `crates/live-telemetry/src/havok.rs` and
`crates/live-telemetry/src/havok_agents/tests.rs`.
No Havok SDK headers or code were used; names (island, entity, motion,
contact constraint) are descriptive. No game data is on this page.

Evidence labels: **Code** (read from the disassembly at the address
given), **S** (inferred or assumed; check before building on it).

## Summary

- The world pointer is `0x0051e9a4`. The world keeps its rigid bodies
  (entities) in simulation islands: an active array, an inactive
  (sleeping) array and one fixed island for the level. **Code**
- An entity maps back to its object through a property (`0x2001`, the
  physics component index) and the component's object handle. **Code**
- Contacts are contact constraints, one per touching pair of entities,
  in their island's constraint array. Each has an array of points
  (position, normal, separation distance) and a parallel array of point
  properties (the impulse the solver applied, friction, restitution).
  **Code**
- An entity's shape is at `+0x0c`. A biped's is a capsule: its tag's
  standing or crouching pill, picked when the body is built
  ([Shapes](#shapes)). **Code**
- The island pair list leads to the BV-tree agent's cached child shape keys.
  For the structure MOPP, type-1 keys name BSP surface polygons. Each known
  convex child exposes its support vertices, separating bound and contact
  IDs. See [Collision agents](#collision-agents). **Code**

## The world

| Field | Offset | Evidence |
|---|---|---|
| world pointer | `0x0051e9a4` | stored at `0x001c2f07` after the constructor `0x003101c0` (**Code**) |
| active islands | world `+0x08`: pointer, count `+0x0c` | `0x0030ccd0` moves an island here from the inactive array, sets island `+0x29` = 1 (**Code**) |
| inactive islands | world `+0x14`: pointer, count `+0x18` | same function (**Code**) |
| fixed island | world `+0x2c` | built at `0x0031071a` (island constructor `0x003171a0`, `0xb0` bytes); `0x0030d2b0` puts a constraint with one fixed side in the other side's island (**Code**) |

Arrays here are Havok's: pointer, count, capacity (bit 31 set when the
storage is not owned). The world constructor initialises them as
`0, 0, 0x80000000` (`0x00310204`-`0x00310306`). **Code**

`0x001c4040` walks the fixed island's entities, then the active or
inactive array's islands' entities, which is how the offsets above and
the island's entity array were found. **Code**

## Islands

| Field | Offset | Evidence |
|---|---|---|
| world | `+0x20` | constructor `0x003171be` (**Code**) |
| index in its world array | `+0x24` | `0x0030cd0b`-`0x0030cd24` (**Code**) |
| active | `+0x29` (byte) | `0x0030cd27`, tested at `0x0030d2d5` (**Code**) |
| entities | `+0x3c`: pointer, count `+0x40` | `0x001c4082` (**Code**) |
| constraints | `+0x58`: pointer, count `+0x5c` | `0x00316cf0` appends one and sets the constraint's island (`+0x14`) (**Code**) |

## Entities (rigid bodies)

| Field | Offset | Evidence |
|---|---|---|
| vtable | `0x00414310` for a rigid body | its constructor `0x00311bc8` (**Code**) |
| shape | `+0x0c`: the collidable's shape (`+0x10` shape key, -1; `+0x14` points into the motion, `0x00311c1f`) | the world object constructor `0x00312590` copies the construction info's shape (info `+0x04`) to `+0x0c` and takes a reference on it (`0x003125ca`, `0x00312609`) (**Code**) |
| properties | `+0x30`: pointer to (key, value) pairs of 8 bytes, count `+0x34` | `0x001c3f33`-`0x001c3f42` (**Code**) |
| motion | `+0x3c` (pointer) | `0x001d08f6` (**Code**) |
| does not move (byte) | `+0x40`: non-zero, `0x001d09dd` reports zero velocity | (**Code** for the read; the meaning is **S**) |
| island | `+0x44` | `0x0030d2b9` (**Code**) |
| material | `+0x48` response type (byte), `+0x4c` friction, `+0x50` restitution | `0x00278c07` indexes contact manager factories by the two response bytes; `0x00318d1c` combines the frictions as `sqrt(a × b)`, `0x00318d3f` the restitutions (**Code**) |

**Owner.** Property key `0x2001` holds the physics component index
(`0x001c3f42`, `0x001c401e`); `0x001c3f90` indexes the component array
(`0x0051e9b8`, element `0xa0`) with its low 16 bits and reads the object
datum handle at component `+0x08` (`0x001c3fa4`), then the object table
(`0x004e0300`). The level's body has no such property (**S**: it is the
body in the fixed island without an owner). The other way round, object
`+0xb4` to component, component `+0x70` bodies, is `0x001d08e0`
(`layout::BODY_PHYSICS`). The component's rigid bodies are an array at
`+0x70`, count `+0x74`, `0x60` each with the entity at `+0x40` (the
release loop at `0x001d022a`). The capacity is at `+0x78`
(`0x001cfa9a..0x001cfabb`). Its shapes are another array at `+0x7c`, count `+0x80`,
`0x0c` each with the shape at `+0x08` (`0x001d01e0`, which drops a
reference from each). **Code**

## Shapes

A shape is a reference-counted object: vtable `+0x00`, reference count
`+0x06` (16 bits; tag shapes hold `0x80`, `0x001c23bb`), user data
`+0x08`. Its type is its virtual `+0x14` (`0x001d6f52`), a getter that
returns a constant. **Code**

| Type | Getter | Vtables (Halo's tag shape, Havok's) | Fields | Evidence |
|---|---|---|---|---|
| 4 sphere | `0x0009cd90` | `0x004554c0`, `0x00455418` | radius `+0x0c` | bounding box `0x002ded6f` (**Code**) |
| 7 capsule | `0x0009b910` | `0x00455500`, `0x00455458` | radius `+0x0c`; end points `+0x10` and `+0x20`, each with the radius in `w` | constructor `0x002de0a0`; bounding box `0x002de0f0` (radius plus tolerance, both points) (**Code**) |
| `0x15` transform | `0x002de7a0` | `0x00455498`, `0x004553f0` | child `+0x0c`; transform `+0x10` (rotation columns), translation `+0x40` | bounding box `0x002de850`; `0x001d6fe3` takes `+0x40` as the centre (**Code**) |

Halo's vtables differ from Havok's in slot 0 (`0x001c2360`, `0x001c2370`,
`0x001c2380` against Havok's `0x00244930`); the other slots seen are the
same functions. `havok.bodies` tells types by the getter, so both read.
A capsule's centre is (a + b) × 0.5 (`0x001d6f5a`, constant
`0x0045dbbc`). **Code**

### A biped's shape

`0x001cf120` builds an object's physics by its type; a biped's
(`0x001d6b80`) calls `0x001d6d00` with the `bipd` character physics
(`bipd +0x264`), which picks the shape (`0x001d6dd6`-`0x001d6e01`):

- pills (`bipd +0x28c`, `0x50` each, the shape at `+0x20`) when there are
  any: pill 1 when the crouch request (unit flags bit 23, `0x001d6bc6`)
  or a melee lunge (`0x000e0dc0`) is on, else pill 0;
- otherwise sphere `0` or `1` of `bipd +0x294` (`0x80` each, the
  transform at `+0x30`);
- when biped `+0x3dc` is 3 (what 3 is: **Open**), sphere `[+0x3ff]` of
  `bipd +0x284` (its transform, `+0x30`).

**Code.** The selected pill comes from the tag. A body's shape can also
wrap that pill: the type-`0x17` shape (vtable `0x004122f0`, constructor
`0x002dbfe7`) delegates its bounding box and support queries to the child
at `+0x30` (`0x002dbe24`, `0x002dbd52`). The capsule's height (|b − a| + 2 ×
radius) is that pill's height (`bipd +0x268` standing, `+0x26c`
crouching; read from the tags in
[tag-definitions.md](../formats/tag-definitions.md), "Bipeds from
definitions").

The shape is chosen only when the body is built. `0x001c3770` rebuilds
an object's body (out of the world, `0x001d01c0` releases bodies and
shapes, `0x001cf120` builds, back in); the biped update calls it when it
sets or clears the crouch request (`0x000e293c`) and when a lunge starts
(`0x000e29a8`), among 17 direct callers. **Code.** So a body built
crouching stays on the crouching pill until something rebuilds it. The
clear at `0x000e2928` (the standing capsule fits) goes through the same
rebuild, so a forced crouch that ends there should leave the standing
pill (**S**); whether every way out does is **Open**, and
`havok.bodies`' `shape.height` shows it directly.

## Motion

| Field | Offset | Evidence |
|---|---|---|
| type | virtual `+0x18`; Halo code compares it with 6 and 7 (`0x001c430a`) | (**Code**; the names of 6 and 7 are **Open**) |
| inverse mass | `+0x2c` | `0x00318e1c`: `1 / (a + b + ε)` over both bodies as the pair's effective mass (**Code**); with inverse inertia in `+0x20`-`+0x28` (**S**) |
| linear velocity | `+0x40` | `0x000dc67d`, `0x001d09e4` (**Code**) |
| angular velocity | `+0x50` | `0x001c4f51`, read after the linear velocity by the same Halo function (**Code**) |
| transform | `+0x80` rotation columns, `+0xb0` position | `0x001d08f9` (**Code**) |
| point velocity | virtual `+0x58` (point, out) | `0x00318ca2` (**Code**) |

## Contacts

### The contact manager and its constraint

A pair's collision agent reports points to a contact manager. The
factory for response type 1 (`0x00319080`, vtable `0x0041506c`, registered
at `0x0030c821`) creates a `0x120`-byte manager (`0x003191c0`), whose
constructor `0x00319110` builds a contact constraint at manager `+0x10`
(`0x0031c000`) and sets its two entities. **Code**

The manager's add-point method (`0x00318c10`, vtable `0x00415074` slot 1)
adds the constraint to the world (`0x0030d2b0`) when it gets its first
point. That puts it in an island's constraint array (`0x00316cf0`): the
first entity's island, or the second's when the first is in the fixed
island. **Code**

| Constraint field | Offset | Evidence |
|---|---|---|
| vtable | `0x004153c8` | `0x0031c00f` (**Code**); the readers use it to tell contacts from other constraints |
| entity A, entity B | `+0x0c`, `+0x10` | `0x0031bac0` sets A; `0x0030d2b6`, `0x0030d2c0` read both (**Code**) |
| island | `+0x14` | `0x00316d46` (**Code**) |
| point id map | `+0x20`: bytes, id → point index (`0xff` free), count `+0x24` | `0x0031be20`, `0x0031bca2` (**Code**) |
| points | `+0x34`: pointer, count `+0x38`, element `0x20` | `0x0031bedb`-`0x0031bf1d` (**Code**) |
| point properties | `+0x90`: pointer, count `+0x94`, element `0x1c`, 4 inline at `+0x9c` | `0x0031bf1f`-`0x0031bf66`; at most 250 (`0x00318c22`) (**Code**) |

The properties array keeps points' properties at the same index as the
points (`0x0031bf28`: a new property is inserted at the old point count),
and can hold more entries than there are points. The readers take one per
point and treat fewer properties than points as a torn read. **Code**

### A point (`0x20` bytes)

| Field | Offset | Evidence |
|---|---|---|
| position | `+0x00` | copied from the agent's point at `0x00318d5c` (**Code**) |
| separation distance | `+0x0c` (the position's `w`) | the jacobian builder `0x0031f620` reads it as the position error (`0x0031f810`); the add-point method folds it into property `+0x14` (`0x00318e10`) (**Code**). Negative is penetration (**S**, Havok's convention) |
| normal | `+0x10` | `0x00318cc1`, `0x0031f6b0` (**Code**) |

Normal direction: the add-point method computes the separating speed as
(velocity of B − velocity of A at the point) · normal (`0x00318cb7`-`0x00318ceb`)
and its impulse estimate is positive for an approaching pair when the
normal points from B toward A. **S**

### Point properties (`0x1c` bytes)

| Field | Offset | Evidence |
|---|---|---|
| impulse applied | `+0x00` | the add-point method seeds it with effective mass × (1 + restitution) × separating speed × 0.2 (`0x00318e49`); the solver's export (`0x0031e4f7`) writes its accumulated impulse here for each point (`0x0031e530`, `0x0031e577`), stepping by the header's stride, which the jacobian builder set to `0x1c` with this array as the base (`0x0031f64e`, `0x0031f65d`) (**Code**) |
| velocity term | `+0x04` | restitution × a step factor × separating speed × 1.3 at add (`0x00318e09`); overwritten by the solver export (`0x0031e540`) (**Code** for the writes; meaning **Open**) |
| friction | `+0x08` | `0x00318d3c` (**Code**) |
| restitution | `+0x0c` | `0x00318d52` (**Code**) |
| `+0x14` | velocity term + distance at add (`0x00318e13`) | (**Code**; meaning **Open**) |
| flag | `+0x18` (16 bits) | `0x0031bfc4`-`0x0031bfd0` (**Code**; meaning **Open**) |

The impulse is in mass × velocity. Divided by the body's mass it is the
velocity change the contact gave that body on the solver's last step.
Whether the world step (`0x00311100`) solves more than once per game tick
is not established, so a per-tick sum is **Open**.

## Collision agents

**Code.** The persistent pair list belongs to a *simulation island*. Its constructor
clears the array at island `+0x68` (data), `+0x6c` (count), `+0x70` (capacity with
ownership flag) at `0x00317211`. The destructor frees capacity × 16 bytes at
`0x0031733a..0x0031734c`. `0x00278b60` walks these 16-byte records. During an
island step (`0x00317d55..0x00317d71`), both update paths fetch the pair array
and call the agent's virtual `+0x10` with the two collidables in entry order
(`0x003179a8..0x003179d4`, `0x00317a2c..0x00317a3e`,
`0x00317c5f..0x00317cae`). An object's
component can own several bodies. The reader matches each body's embedded
broadphase handle against either side of the pair, across active, inactive and
fixed islands.

- **Code.** Pair `+0x00/+0x04` are pointers to broadphase handles. Subtract
  `0x10` to reach each collidable (`0x00278b90..0x00278ba0`). The rigid body's
  collidable starts at entity `+0x0c`, where its shape pointer is stored
  (`0x003125ca`). Thus the handle lives at entity `+0x1c`. For collidable type
  1 at `+0x18`, the owner is the entity pointer at collidable `+0x20`
  (`0x00278bb8..0x00278bc2`, back-pointer set at `0x00312604`).
- **Code.** Pair `+0x08` points to a separately allocated agent. Both shape
  types select the factory at `0x00278c33..0x00278c6d`, and its result is
  stored at `0x00278c6f`. A pair can exist without an agent (filter failure
  at `0x00278be6..0x00278bf5`, cleanup at `0x00278e0b`). Pair `+0x0c`
  receives 0 or −1 on the creation paths at `0x00278d0f..0x00278d34`.
  One processing mode uses nonnegative values to set the input threshold
  to `world[+0x244] + pair_word * world[+0x248]` and uses −1000 when negative
  (`0x003179eb..0x00317a29`, `0x00317af1..0x00317b06`). It can increment
  or decrement this word (`0x00317ab9`, `0x00317ac9`). The API preserves its bits.
- **Code.** Agent `+0x00` is the vtable, `+0x04` is the allocation size
  (u16), `+0x08` is the contact manager. The BV-tree factory stores all
  three at `0x002e9633..0x002e9644`. Destruction passes the size to the
  allocator (`0x002e95af..0x002e95bf`, also `0x002f438f..0x002f439f`
  for a convex agent). `cache.hex` covers the inline allocation including
  this header. A filtered child can instead point to the shared empty agent
  at `0x004805a4` (`0x002e9a42`, `0x00301a40`). Its constructor writes
  vtable `0x0041357c` and a null manager at `0x00301a52..0x00301a59`;
  the reader takes only the 12-byte header.

### BV-tree, MOPP and the welded structure

**Code.** There are direct and wrapped paths to the same child list.

- The BV-tree agents are `0x40` bytes. Vtable `0x00412a98` takes side B as
  the tree (`0x002e9620`, `0x002ea472..0x002ea4c1`). Vtable `0x00412adc`
  swaps A and B (`0x002eb4e0`, `0x002eb9a6..0x002eb9b0`). The two-MOPP
  factory installs `0x00412b08` or `0x00412b28` at
  `0x002ebb57/0x002ebb96`, with the same two dispatch orders.
- The level builder sets the structure MOPP's vtable to `0x00456160` at
  `0x001c3b4d`. When the switch at `0x0047f06d` is on, the level body gets
  a type-`0x14` wrapper (vtable `0x00455540`, MOPP at `+0x0c`,
  `0x001c3bd7..0x001c3bff`, type getter `0x0009fce0`). The convex
  welding agents are `0x00412f5c` for tree B and `0x00412f7c` for tree A
  (`0x002f9bd1..0x002f9bfa`, `0x002fa091..0x002fa0ba`). The former
  unwraps B and calls the ordinary BV-tree update at `0x002fa349..0x002fa37b`;
  the latter swaps the arguments at `0x002faa6e..0x002faa80`. The sphere
  variants `0x00412fb0/0x00412fd0` take the same orders
  (`0x002fb13a`, `0x002fb9ba`, `0x002fb18a..0x002fb1ac`, `0x002fb930`).
- Agent `+0x0c/+0x10/+0x14` is the data/count/capacity array of 12-byte
  child entries (`0x002e95f7..0x002e95fd`, `0x002ea464..0x002ea46f`,
  `0x002e9fd0..0x002e9fe8`). Child `+0x00` is the complete 32-bit key,
  `+0x08` is the child agent pointer. Both are written after dispatch
  (`0x002e9a29..0x002e9a50`), then the key selects the shape and the child
  receives the process call (`0x002ea4b5..0x002ea4e7`). The welded path
  reads the same fields at `0x002fa3f0..0x002fa41a`. Child `+0x04` is
  copied when entries move (`0x002e98ae`), meaning **Open**.
- Parent `+0x20/+0x30` hold the last query AABB's min and max in the tree's
  frame. `0x002e96cf..0x002e9716` gets the transformed box. With caching
  enabled, the comparison at `0x002e9730..0x002e9784` can reuse the list.
  Otherwise the update stores both corners (`0x002e9793`, `0x002e9797`)
  and calls the box query (`0x002e97ce`). A key is a candidate even when
  its child has zero contact points.
- For the structure MOPP, type 1 (`key >> 29`) names a collision BSP surface
  by `key & 0xffff` (`0x001ef3e0..0x001ef3fa`, `0x001ef451`). Its shape
  is the polygon made from that surface's vertex ring (`0x001edd00`).
  Other key types and other trees retain the full key without a surface index.
  See [MOPP keys](../formats/mopp.md#keys).

### Convex child state

**Code.** The ordinary convex factory allocates `0xf0` bytes at
`0x002ee3b0..0x002ee3da` and installs `0x00412c48` at `0x002ee313`.
Halo's optional type-`0x17` factory allocates `0x100`, calls the same
constructor and installs `0x00412290` (`0x002dae61..0x002daea7`).
The biped wrapper vtable `0x004122f0` has type getter `0x0009f550`
(returning `0x17`). Both its support query and its lookup by cached vertex
ID delegate to the pill currently at shape `+0x30`
(`0x002dbd52..0x002dbd59`, `0x002dbdc6..0x002dbdcd`).

- Agent `+0x0c..+0x13` stores four u16 support vertex IDs. Bytes
  `+0x14/+0x15` give the number for shape A and B in *child dispatch order*.
  B's IDs start after A's. `0x002ee34f/0x002ee360` pass the embedded cache
  to initialisation. `0x002ff050..0x002ff09d` seeds both IDs and counts.
  `0x003048ad..0x003048f5` uses the counts to reconstruct the vertices
  from each shape. For a structure polygon these IDs select ring vertices
  at `shape+0x20+16*id` (`0x001ee3f0..0x001ee3fa`).
- Agent `+0x18` is the manifold count. Slots at `+0x30 + 0x30*i` contain
  the manager's u16 contact ID at slot `+0x1c`. The manager returns the
  ID at `0x00302e2f`, and `0x00302e32..0x00302e4f` stores it and raises
  the count. The insertion path tests against four at `0x00302d73`.
  The removal loop (`0x002eee44..0x002eee50`, `0x003029e4..0x003029fd`)
  consumes these IDs in reverse. They index the contact constraint's
  [point ID map](#the-contact-manager-and-its-constraint).
- Agent `+0x20` (xyz) is the cached separating axis and `+0x2c` is the
  cached separation bound. Construction sets the bound to −1
  (`0x002ee374`). A positive bound enables a motion projection onto the
  axis that reduces the bound and can skip a fresh closest-feature query
  (`0x002eeb7c..0x002eeb9c`, `0x002eebda..0x002eec29`).
  `0x002eee2b..0x002eee41` stores an axis and a tolerance-adjusted bound.
  Axis bytes can survive after the bound has been invalidated.
- In `0x00412290` only, `+0xf0` saves which side used the swept support
  offset (A adds 1, B adds 2, `0x002db40a`, `0x002db52d..0x002db537`,
  stores `0x002db5ec` and `0x002db284`). The offset comes from motion and a type-`0x17`
  shape threshold (`0x002db331..0x002db36c`, `0x002db3e7..0x002db406`).
  Both its ordinary and welding paths can reseed the feature IDs and set
  the bound to −1 when the side mask changes or a sweep is active
  (`0x002db551..0x002db5cb`, `0x002db1e4..0x002db25e`).
- The welding pass also builds temporary `0x80`-byte records with child
  agent, shape, key and flags (`0x002fa380..0x002fa397`,
  `0x002fa47d..0x002fa4e2`). It tests B's feature count against 1 and 2
  at `0x002fa59c..0x002fa5d5`. **S**: those cases distinguish vertex or
  edge candidates for welding. Which state, if any, persists across an
  actual forced crouch and predicts a bounce remains **Open**.

### Physics component character contacts

**Code.** At component `+0x88` the game keeps a different array, populated
by Halo's contact callback (`0x00277b80`, append `0x001d0400`). Count is
`+0x8c` (`0x001d0407`), while `+0x90 & 0x7fffffff` gives *capacity*.
The append compares capacity with count + 1, grows by `0x48`-byte elements,
then stores the new count (`0x001d0507..0x001d0539`). Capacity can stay
high after entries are removed.
The component constructor initialises these three words at
`0x001cf9c6..0x001cf9d2`. The existing
`world.object` field `physics.support.contacts` decodes their keys, points,
normals and flags; the offsets and ground-test reads are listed in
[the agent API](../runtime/agent-api.md). Use the island pair list to inspect
the collision agents themselves.

## Halo's listeners

Halo registers a listener object (`0x002784c0`) with the world, which
appends it to an array at world `+0x6c` (`0x0030cb30`; that this is the
collision listener array is **S**). The callbacks seen (`0x002789f0`,
`0x00278a70`, vtable `0x0045a768`) mark the owner's physics component
through property `0x2001`; none reads contact points. **Code**

## Open

- **Crouch and the shape.** Whether any path leaves a standing biped's
  body on the crouching pill (see "A biped's shape"). `world.raw` and
  `havok.bodies` show it on a trace.

- **Cache lifetime.** Which pairs and children survive a forced crouch and
  whether their feature IDs or separation bounds predict a later bounce.
  The child entry's second word and per-step welding decisions also remain open.
- **Motion types** 6 and 7 and entity `+0x40`: which is keyframed and
  which fixed.
- **Property `+0x04`, `+0x14`, `+0x18`**: the solver's use.
- **Substeps**: how many solver passes a game tick runs, and so whether
  the impulse read at a tick boundary is all of that tick's push.
