# Havok at run time: world, bodies and contacts

How the retail game keeps its Havok world in guest memory, as far as
`havok.bodies` and `havok.contacts` read it
([agent-api.md](../runtime/agent-api.md), "havok"). Written 2026-11-18 for
the superbounce work ([superbounce.md](../game/superbounce.md)): which
surface pushed the biped's body, and how hard.

Static reading of `default.xbe` with `h2 disasm`; nothing here was
checked in a running game. The readers (`crates/live-telemetry/src/havok.rs`,
constants in `layout.rs`) are tested over synthetic guest memory only.
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
- Which triangle of the level a point is on is not in the constraint; it
  would be in the collision agents. Not read. **Open**

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
(`layout::BODY_PHYSICS`).

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

## Halo's listeners

Halo registers a listener object (`0x002784c0`) with the world, which
appends it to an array at world `+0x6c` (`0x0030cb30`; that this is the
collision listener array is **S**). The callbacks seen (`0x002789f0`,
`0x00278a70`, vtable `0x0045a768`) mark the owner's physics component
through property `0x2001`; none reads contact points. **Code**

## Open

- **The surface.** A point does not say which triangle of the level it
  is on. The shape key is with the collision agent (the pair array at
  `0x00278b60` keeps an agent per pair at `+0x08`; the level's MOPP agent
  has a child per triangle). Reading it means decoding the agents.
- **Motion types** 6 and 7 and entity `+0x40`: which is keyframed and
  which fixed.
- **Property `+0x04`, `+0x14`, `+0x18`**: the solver's use.
- **Substeps**: how many solver passes a game tick runs, and so whether
  the impulse read at a tick boundary is all of that tick's push.
