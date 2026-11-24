# Vehicle actions and Havok transforms

Evidence for [Vehicles and drop ships](../rules-world-api.md#vehicles-and-drop-ships).

## 2026-11-24 — retail Xbox, dbafa82d1

Static reading of the owner's retail executable. To reproduce, read HS
descriptor entries 232, 233 and 621 with the layout in
`crates/formats/hs/src/catalog.rs`. Run `h2 disasm /path/to/default.xbe
0x2a40e0 70` for the first two evaluators and repeat at the addresses below.
The addresses identify code. No game session or network capture was used.

### R6. Loading, unloading and hovering

| HaloScript | Evaluator | Body and result |
|---|---|---|
| `vehicle_load_magic` (232) | `0x2a40e0`, call at `0x2a411c` | `0x11b0c0` takes the target in EDX and the seat mapping and HS object list on the stack. It walks eligible units in the list, tests empty matching seats for compatibility at `0x11b1ab`, requests entry at `0x11b1ea` and consumes a seat on success. AX counts accepted entries. |
| `vehicle_unload` (233) | `0x2a4140`, call at `0x2a4177` | `0x11b2b0` takes the target in EDX and mapping in ECX. It resolves the occupant at `0x11b2ee`, requests its exit at `0x11b326` and counts accepted requests in AX. An empty vehicle gives zero. |
| `vehicle_hover` (621) | `0x2aa480`, call at `0x2aa4ae` | `0xf0fd0` takes the vehicle in EAX and the boolean in CL. It sets or clears the hold bit at object `+0x348`. A change with a physics component enters the update path at `0xf1012`. Repeating the current setting skips that work. |

Load and unload share the seat collector at `0x2116f0`. Its enumeration
at `0xc8a40` includes eligible attached units and bounds the result to 64
seat records. The predicate at `0x211830` accepts every seat for mapping
-1. Zero has a zero-length mapping and matches none. The occupancy tests
at `0x2117bb` and `0x2117cb` select empty and occupied seats respectively.
Unload requests exits from all matching occupants in this bounded scan.
The returned count measures requests accepted at dispatch.

### R6. Which vehicle a squad uses

The counted HS caller at `0x2a5052` supplies no vehicle datum to
`AI_SQUAD_PLACE` (`0x201520`). Two vehicles can take part:

- **Emitter.** The chosen start's emitter index at `+0x3c` goes to
  `0x201330`. It finds a live vehicle by scenario placement identity.
  Unless forced, `0x201725` sends it a deferred placement action with
  the squad and start. The timer at `0xed7dd` re-enters placement for
  one actor at that start with force set.
- **Seat owner.** At `0x201744` the start's vehicle palette index
  (`+0x28`) takes precedence over the squad's (`+0x34`). `0x201010`
  searches the vehicles made in this placement call by tag and compatible
  opening, preferring fewer occupied seats. On a miss, `0x201800`
  creates a vehicle. The selected seat can belong to an attached unit.
  Actor creation and counting at `0x201912` and `0x201926` precede the
  seat checks. The entry request at `0x201984` uses the selected owner
  and seat. A missing seat or rejected entry still reaches registration
  at `0x2019b9`.

Neither path implements the rules contract of spawning at most the free
seats of a particular existing ship. Loading an HS list after spawning
would leave surplus actors in the world.

### R7. Pose acceptance and the following tick

`OBJECT_SET_TRANSFORM` (`0xb7430`) writes the object pose. With its
`notify` argument set, `0xb755a` calls `0x1c4a80`. This resolves the
physics component at object `+0xb4` and checks its flags. A missing
component or its bit 8 skips the body update. On the multi-body path
`0x1d3fc0` applies the change of pose to each body through `0x1d0c20`.
The motion setter at `0x3199c0` writes the basis and position, preserving
linear and angular velocity. The transform's success flag alone does not
establish that a body moved.

The next world step (`0x1c3ddd`) can move the body again. A dynamic
integrator advances the centre by velocity times elapsed time at
`0x31a996`. The reverse path at `0xbc5e0` reads a body pose through
`0x1d4360` and feeds it back to the object with notification off at
`0xbc6d1`. The proposed mover's velocity setter (`0xb7740`) updates
object velocity. The wrapper at `0xb77d0` also reaches the body velocity
path at `0x1c4b00`.

Eligible bodies can accept successive notified transforms. Dynamics still
own the pose between calls. The static trace does not establish exact
end-of-tick path positions, contact response or rider stability.
