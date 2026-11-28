# The Havok step: entries, writes and classes

Status: Partly built. The digest, its agent method and its sim-report component are built, and so are the four allocator entries (slice 1), the island solve (slice 2) and slice 3's answering arms for the island integrate, the deactivation test and the integrate phase (below, "The island solve"; [havok-integrate.md](havok-integrate.md)). The step's other entries are contracts only, ported by the slices of [sim-rewrite-havok.md](sim-rewrite-havok.md) §11.
Code: `crates/rewrites/sim-havok/src/mem.rs` and `crates/rewrites/engine/src/sim_physics/alloc.rs` (the allocator), `crates/rewrites/sim-havok/src/island_solve.rs` and `crates/rewrites/engine/src/sim_physics/island_solve.rs` (the island solve), `crates/rewrites/sim-havok/src/motion_solve.rs` (`forces`, `pose`, shared with the island integrate), `crates/rewrites/engine/tests/sim_havok_differential.rs` (their differential tests), `crates/live/live-telemetry/src/havok_agents/digest.rs` (`read`), `crates/extensions/agent/src/havok.rs` (`havok.digest`), `crates/extensions/agent-proto/src/havok.rs` (`Digest`, `DigestParams`), `crates/extensions/game/src/sim_glue.rs` (the `havok.world` component). Tests: the unit tests of those three files.
Always installed: the physics entries have no switch.

This is the contract page for the world step `0x311100` and everything
under it. It records each planned entry's callers, what the step writes
that outlives it, which classes the step dispatches to, the float
instruction mix, what a checkpoint save and revert do to the world, and
what `havok.digest` covers. Each slice's step 0 adds its own findings
here before code. The rule: a ported entry leaves every byte listed here
as retail leaves it, in retail's order.

Evidence: **E** read from the owner's `default.xbe` with `h2 disasm` and
a linear scan of `.text` (the guest address given); **Code** this
repository; **S** supposed, with what would check it. Names are
descriptive. No Havok SDK source or headers were used. Class records are
cited by address, not by the names the image carries.

## The step tree

`W` is the world (`[0x0051e9a4]`), `I` an island. The tree and its order
are [sim-rewrite-havok.md](sim-rewrite-havok.md) §2; this table adds who
else calls each node and what each writes that persists (**E**).

| Node | Callers outside the step | Persistent writes |
|---|---|---|
| `0x00311100` step (thiscall world, `&{dt, 1/dt}`, `ret 4`) | `0x001c3d30` (the tick's wrapper); `0x001c2b10` (the save, revert and rebuild bracket, below), which steps once with the fixed step read from `0x0045dc7c`/`0x00445420` | through its two children only |
| `0x0030fcb0` collide | none | `W+0x150..+0x1a7` (step copy and derived terms), `W+0x12c` lock, pending operations (`0x0030f6b0`), the island loop below, the listener walks `0x003188c0` |
| `0x00317d30` island collide | none | via `0x003175e0` (AABBs, broadphase, agent creation and destruction `0x003173f0`, `I+0x30 = 1`), the pair walk `0x00317950`/`0x00317c00`, Halo's `0x00278e60`/`0x00278e80` (`I+0xae`, `[0x0055e4fc]`) |
| `0x0030fef0` integrate | none | the reverse island loop, deactivation `0x0030cc60`, island split `0x0030e720`, `0x0030fc30`, `0x0030cda0`, `0x0030d400` over `W+0x114` |
| `0x00316b30` deactivation test | none | the island's timers and counter, the deactivators' references ([havok-integrate.md](havok-integrate.md#the-deactivation-test)) |
| `0x00316a20` island integrate | none | the actions' `vt[+0x14]` (none exist, below), the motions' `vt[+0x08]`/`vt[+0x04]` when `I+0x5c == 0`, else `0x0031c100`; its caller then stores `I+0x30 = 0` (`0x0031000b`) |
| `0x0031c100` island solve (cdecl, nine arguments) | none | `motion+0x08` per entity, `W+0x190`/`+0x194`, the contact points' impulse and velocity words, the motions' velocities and transforms via `vt[+0x10]`, the solver buffer's busy byte |
| `0x0030cc60` island deactivate | `0x001c50c0` (Halo, from `0x001c2a10`), `0x0030ce20` | island arrays and the entities' island pointers |

The four memory-system virtuals (`0x0022c300`, `0x0022c320`, `0x002d7020`,
`0x002d7090`, vtable `0x004576a4`) have no direct callers; every caller
loads `[0x00480118]` and calls through the vtable. Of the 479 instructions
that read `[0x00480118]` in `.text`, 74 are in Halo code (51 functions, among them
the world builder `0x001c2d40`, the body builders `0x001c3b70`,
`0x001cf9f0`, `0x001d01c0`, and the phantom owners), 309 in the step's
closure and 96 in Havok code outside it. Slice 1's allocator entries
therefore run for Halo callers outside the step, as retail.

**Absolute globals.** The step's closure (below) stores to no absolute
address on its path except the monitor stream head `[0x00480154]`, which
only advances while the stream is on (an entry condition makes it off,
§3.3), and Halo's own globals `0x0055e4fc..0x0055e50c` through its hooks.
Everything else it writes is reached through `W`, the islands, the heap
and the solver buffer. The one-time solver table initialiser
`0x0031c7d0` (`[0x00480c60]`) is in the closure but runs once. **E**,
over-approximate closure; the per-store census below is the check.

## Entries and the writes they own

The entries are [sim-rewrite-havok.md](sim-rewrite-havok.md) §7.8's.
What each must still write, beyond §7.8's list:

- `0x00311100`: `0x001c2b10` calls it once per checkpoint save, once per
  revert and once per memory-pressure rebuild (`0x001c27a0`), outside the
  tick, with the image's fixed step. A port at this entry covers both
  callers; the float environment check runs for both.
- `0x0030cc60`: Halo calls it directly (`0x001c50c0`) when it rebuilds
  bodies after a revert. Slice 3's integrate entry contains it but must
  not assume it runs only inside the step.
- `0x0031c100`, `0x00316a20`, `0x00316b30`, `0x0030fef0`, `0x00317d30`,
  `0x0030fcb0`: each has exactly one direct caller, its parent in the
  tree, and no address-taken reference anywhere in `.text`. **E**

## Censuses

All by vtable install (`mov [reg+off], vtable`) and by constructor
callers, read from the whole of `.text`. Slot offsets are byte offsets.
"Class record" is the pointer a class's `vt[+0x0c]` returns; records are
built at start-up by `0x002d9750` (record, name, parent, ...) and the
world's object add `0x003109b0` dispatches on them (`0x002d9710`
tests the parent chain). **E**

### Motions

The factory `0x00311940` builds all six concrete motions; callers
`0x00311ba0` (rigid body constructor) and `0x00311e40` (motion type
change). The type is `vt[+0x18]`; `vt[+0x14]` returns `0x80`, the
accumulator size (**S**: read with `0x0031c100`'s use in slice 2).

| Vtable | Type | `vt[+0x04]` | `vt[+0x08]` | `vt[+0x0c]` accumulator | `vt[+0x10]` from accumulator |
|---|---|---|---|---|---|
| `0x00414328` | abstract (all pure) | | | | |
| `0x004143a8` | 2 | `0x00319f70` | `0x00319cd0` | `0x00319df0` | `0x0031a0d0` |
| `0x00414428` | 3 | `0x00319f70` | `0x00319cd0` | `0x00319df0` | `0x0031a500` |
| `0x004152b0` | 4 | `0x0031b060` | `0x0031b1c0` | `0x0031b300` | `0x0031b4c0` |
| `0x00415230` | 5 | `0x0031b060` | `0x0031b1c0` | `0x0031b300` | `0x0031a8c0` |
| `0x00415130` | 6 | `0x003195d0` | `0x0024dc50` | `0x00319430` | `0x00319490` |
| `0x004151b0` | 7 | `0x00072c70` | `0x0024dc50` | `0x0031a770` | `0x0031a6a0` |

`vt[+0x04]` is the pose (step; `ret 4`) and `vt[+0x08]` the forces (step,
gravity; `ret 8`); `0x0024dc50` and `0x00072c70` are a bare `ret 8` and
`ret 4`. Types 2 and 3 are dynamic motions that §2 did not list. Type 7 is the one
the rigid body constructor builds directly (`0x00311ba0`), so it is the
level's (**S**). Which types a match builds is a run question.

### Constraints

Base vtable `0x00415360` (record `0x00480b90`, constructor `0x0031b9e0`).
`vt[+0x24]` appends the Jacobian rows and schemas. Six rigid joint
classes share destructor `0x003142c0` and are built only by Halo's
`0x001d77d0` (from `0x001d6ad0`); their kind is `vt[+0x20]`.

| Vtable | Record | Kind | `vt[+0x24]` | Built by |
|---|---|---|---|---|
| `0x004153c8` contact | none | `0xb` | `0x0031bdf0` | the contact manager `0x00319110` |
| `0x004146fc` | `0x00480658` | 7 | `0x00312fe0` | `0x001d77d0` |
| `0x00414860` | `0x00480680` | 2 | `0x00313810` | `0x001d77d0` |
| `0x0041490c` | `0x004806a8` | 1 | `0x00313ef0` | `0x001d77d0` |
| `0x0041498c` | `0x004806d0` | 0 | `0x00314220` | `0x001d77d0` |
| `0x00414b4c` | `0x00480720` | 8 | `0x003149e0` | `0x001d77d0` |
| `0x00414ca4` | `0x00480748` | 6 | `0x00314fc0` | `0x001d77d0` |
| `0x00414ce0`, `0x00414d0c` | none | `0xb` | `0x00315620`, `0x00315740` | start-up initialisers only (`0x00373b6a`) |

The contact class is the only one the step creates. The six joints come
from Halo's physics model builder, so any object with a jointed physics
model (ragdolls, **S**) puts them in islands. `sim_havok::joint` ports
kinds 0 (ball and socket: three linear rows along the world axes between
the two world pivots, `0x0031f160`), 1 (hinge: two angular rows from
`0x0031e7b0` keeping the axes parallel, then the ball-and-socket rows,
`0x0031f3e0`) and 8 (stiff spring: one linear row, `0x0031ec30`, its
right-hand side the rest length `+0x40` less the length, `0x0031e740`).
Each writes the header `0x0031e770` (output `c+0x40`, `+0x70`, `+0x44`,
stride 8), free linear rows `0x0003040d` and free angular rows
`0x0004040c`. Kind 2 (limited hinge, `0x00313810`, output `c+0x90`) adds to the hinge's
rows an angular range row (`0x0031f000`, schema `0x0004100a`) about the
axis, its value the angle of B's base about it (`0x003725e0`, a cubic
approximation folded by quadrant) kept within a turn of the last angle,
which it stores back at `c+0xd4`; then, when `c+0xd0` is nonzero, a
capped angular friction row (`0x0031e900`, `0x00040809`). Kind 6
(prismatic, `0x00314fc0`, output `c+0xac`) writes three angular rows
locking the orientation, two linear rows across the slide axis, a linear
range along it (`0x0031ee10`, `0x00030c0b`) and, when `c+0xa8` is
positive, a capped linear friction row (`0x0031ea90`, `0x00030806`).
Kind 7 (ragdoll, `0x00312fe0`, output `c+0x98`) writes up to two cone
ranges (`0x00312e00`, each skipped when its cross product is shorter than
`0x004146e4`), a twist range about the mean twist axis, the
ball-and-socket rows and, when `c+0x94` is positive, three capped
friction rows about A's basis.

### Collision agents

Halo registers every agent group once, at `0x001c3a40`, called from the
world builder: 21 registration functions in a fixed order, and the
type-`0x17` group (`0x002daf00`) last, only when the byte `[0x0047f06f]`
is set. 39 agent vtables; `vt[+0x10]` is the process call, `vt[+0x18]`
is `0x002fe820` for the direct order and `0x0031bc80` for the swapped
one (**S**: a swapped agent's `+0x18` slot is the one shown).

| Registration | Vtables (`vt[+0x10]`) |
|---|---|
| `0x002e6cd0` | `0x00412988` (`0x002e6690`), `0x004129a8` (`0x002e6950`) |
| `0x002e7d10` | `0x004129dc` (`0x002e6e90`), `0x00412a30` (`0x002e79d0`) |
| `0x002e9030` | `0x00412a54` (`0x002e7dc0`), `0x00412a74` (`0x002e8da0`) |
| `0x002eb540` BV tree | `0x00412a98` (`0x002ea410`), `0x00412adc` (`0x002eb990`) |
| `0x002ebbb0` two trees | `0x00412b08` (`0x002ea410`), `0x00412b28` (`0x002eb990`) |
| `0x002ecbf0` | `0x00412b60` (`0x002ebc80`), `0x00412b80` (`0x002ec940`) |
| `0x002ed700` | `0x00412ba4` (`0x002ecc80`), `0x00412bc4` (`0x002ed460`) |
| `0x002ed9b0` | `0x00412be4` (`0x002ed7f0`) |
| `0x002ee240` | `0x00412c08` (`0x002eda30`), `0x00412c28` (`0x002edf90`) |
| `0x002eee90` convex | `0x00412c48` (`0x002eeb40`) |
| `0x002ef900` | `0x00412c90` (`0x002ef3b0`) |
| `0x002f1ab0` | `0x00412d48` (`0x002f1080`), `0x00412d68` (`0x002f1a50`) |
| `0x002f7a70` | `0x00412e24` (`0x002f4440`), `0x00412e44` (`0x002f7a10`) |
| `0x002f8900` | `0x00412e94` (`0x002f7c40`), `0x00412eb4` (`0x002f88a0`) |
| `0x002f96d0` | `0x00412f14` (`0x002f90a0`), `0x00412f34` (`0x002f94a0`) |
| `0x002faac0` welding | `0x00412f5c` (`0x002fa2f0`), `0x00412f7c` (`0x002faa60`), over the BV-tree agent |
| `0x002fb9d0` welding, sphere | `0x00412fb0` (`0x002fb150`), `0x00412fd0` (`0x002fb910`), over the BV-tree agent |
| `0x002daf00` type `0x17` | `0x00412290` (`0x002db2b0`), over the convex agent |
| `0x002efe20`, `0x002f0e00`, `0x002f2800`, `0x002f42b0` | not resolved by the scan; the remaining vtables are `0x00412cc0` (`0x002efe60`), `0x00412cf4` (`0x002f0100`), `0x00412d14` (`0x002f0da0`), `0x00412da0` (`0x002f1b80`), `0x00412dc0` (`0x002f27a0`), `0x00412df0` (`0x002f3030`) (**S**) |
| none: the shared empty agent | `0x0041357c` (`0x002ed7f0`), built once at start-up |

A row pairs a registration function with the vtables its creation
functions construct. Which shape-type pair each serves is slice 5 and 6
step 0. Which agents appear in a match is `havok.agents`' `kind`
and `vtable` (a run question).

### Shapes

The type is `vt[+0x14]`. Halo subclasses differ from Havok's in slot 0
([havok-runtime.md](havok-runtime.md), "Shapes").

| Type | Havok's vtable | Halo's vtables (constructor) |
|---|---|---|
| 2 collection | `0x00455598` (`0x001c2910`) | `0x004560f8` (`0x001eed40`, the level), `0x00456194` (`0x00246c60`) |
| 3 BV tree | `0x00455dc8` | |
| 4 sphere | `0x00455418` | `0x004554c0` |
| 5 triangle | `0x00455e78` | `0x00456048` |
| 6 box | `0x00455d88` | `0x00455ef0` |
| 7 capsule | `0x00455458` | `0x00455500` |
| 8 convex vertices | `0x00455d48` | `0x00456008` |
| 9 multi-sphere | `0x00455e44` | `0x00456088` |
| `0xa` list | `0x00455e00` | `0x00455fc0` |
| `0x11` bounding-volume wrapper | `0x00455f30` | |
| `0x13` MOPP tree | `0x00455eb8` | `0x00455f58`, `0x00456160` (structure, `0x001c3b20`) |
| `0x14` welding wrapper | `0x00455540` | |
| `0x15` transform | `0x004553f0` | `0x00455498` |
| `0x16` phantom callback | | `0x00455f8c` |
| `0x17` biped wrapper | | `0x004122f0` (`0x002dbfc0`, from `0x001d7390`) |
| `0x18` convex polygon | | `0x004560b8` (`0x001edd00`, the structure polygon builder) |
| `0x1b` | | `0x00456134` (`0x001ef8d0`) |

### Actions

None. The world's object add (`0x003109b0`) sends an object whose record
descends from the action record `0x00480b68` to `0x0030e3b0`, the only
path into an island's action array (`0x00316d60`; its other callers
`0x0030f380`, `0x0030f960` move actions between islands). No record has
`0x00480b68` as its parent and no `vt[+0x0c]` in the image returns it.
So `I+0x4c` is always empty and `0x00316a20`'s action loop never calls
out. **E**

### Listeners

| Array | Added by (caller) | Removed by (caller) | Read in the step |
|---|---|---|---|
| `W+0x6c` | `0x0030cb30` (`0x002784c0`) | `0x0030c470` (`0x00278ae0`) | **S**: not on the step's path |
| `W+0x78` | `0x0030cab0` (`0x001c2d40`, the object with vtable `0x0045a7a4`) | `0x0030c3e0` (`0x001c34a0`) | **S** |
| `W+0xfc` | `0x0030ca30` (`0x001d1260`) | | `0x0030cda0` after collide and after integrate |
| `W+0x9c` (count `+0xa0`) | no adder found | | `0x00318830`, per island in integrate |
| `W+0xa8` (count `+0xac`) | no adder found | | `0x003188c0`, per island in collide |

The two per-island arrays have no adder in `.text` by a scan for array
growth on those offsets (**S**: `havok.digest`'s walk of `W` in a match
would show them non-empty).

### Phantoms and deactivators

| Vtable | Record | What | Built by (Halo) |
|---|---|---|---|
| `0x004137cc` | `0x00480630` | box phantom, `vt[+0x14]` 0 | `0x001c5710`, `0x001d48f0`, `0x001d56f0`, `0x002784c0` |
| `0x0041382c` | `0x004805b8` | shape phantom, `vt[+0x14]` 1 | `0x00183910`, `0x001c5210`, `0x001d48f0`, `0x001d5120`, `0x001d56f0` |
| `0x00414abc` | `0x004806f8` | entity deactivator | `0x00311690` |
| `0x00455950` | `0x004806f8` | Halo's deactivator subclass | `0x001d1820` (from `0x001d1540`) |
| `0x00415100` | `0x00480c38` | the always-active deactivator | start-up initialisers only |

Halo adds phantoms through `0x0030e4b0` (six Halo callers), which appends
to `W+0x120` (count `+0x124`), gives the phantom the id at `+0x20` from
the counter `W+0x130`, and inserts it in the broadphase. Its overlap
lists are written by the broadphase inside collide (**S**: slice 7, step
0).

## Allocator

Every Havok and Halo allocation goes through the dispatcher `[0x00480118]`
(`0x004798b0`, vtable `0x0045378c`: allocate `0x001472c0`, free
`0x00147320`, both `ret 8`/`ret 0xc` with a class word that the pools
ignore). `0x00146a20` builds three pool objects inside the block
`0x00146b30` reserves (`[0x00479888]`): the directory pool `[0x00479894]`
(vtable `0x004576a4`, a 0x15000-byte directory at `+0x30`), the class pool
`[0x00479898]` (vtable `0x00457620`, built by `0x0022c3e0`, 17 classes
over 0x76 pages of 0x2000 bytes), and later the borrowed block's directory
pool `[0x0047989c]`. Which pool answers is `[0x00479890]`. **E**

- Mode 1: allocate goes to the directory pool alone.
- Other modes: the class pool; a null answer goes to the borrowed
  directory pool; a null from that goes to the overflow object built by
  `0x00146e30` (a lazily allocated third pool). Free routes by address
  range, signed: class pages `[P+0x4340, P+0xf0340]`, the directory
  `[D+0x30, D+0x15030]`, the borrowed block `[base, base+size]`, else the
  overflow object. Halo's rebuild stores the mode (`0x001c2741`,
  `0x001c27c5`).

The four entries, each with every persistent write (**E**, **Code**
`sim_havok::mem`):

| Entry | Does | Writes |
|---|---|---|
| `0x0022c300` (this = directory pool; size, tag; `ret 8`) | forwards to `vt[+8]` `0x0022c0d0` with mode `0x10`: first fit over the directory, 16-byte-aligned offsets. The new block's end must stay below the directory grown by one entry; the end of the last block is tested first | the live count `+0x20`, the new (offset, bytes) slot and every later slot shifted one place toward lower addresses; slot `i` sits at `base + bytes - 8*(i+1)`. Returns `base + offset`, or 0 |
| `0x0022c320` (this; block, size, tag; `ret 0xc`) | forwards to `vt[+0xc]` `0x0022c280`: the first entry whose address equals the block goes | the count and the slots after the entry, shifted up; the last slot keeps its stale words; no match writes nothing |
| `0x002d7020` (this; request; `ret 4`) | asks the dispatcher for `max(request, 0x1000) + 0x10` bytes, copies `this+0x08..+0x18` into the block's first 16 bytes, then points the descriptor at the block | `this+0x0c` remaining (`max - request`), `+0x10` block, `+0x14` payload (`block+0x10`), `+0x08` cursor (`payload+request`); the block header; whatever the dispatcher wrote. Returns the payload; a null block is not checked |
| `0x002d7090` (this; unused; request; `ret 8`) | restores the descriptor from the block header at `payload-0x10` and frees `remaining + request + 0x10` bytes there through the dispatcher | `this+0x08..+0x18`; the dispatcher's writes |

The class pool's allocate (`0x0022c830`, `size > 0x2000` is null with no
write): the call counter `+0x2e8+4c` is incremented first, even when the
answer is null; a free-list head is popped (`FREE[c]` falls by one);
otherwise a block is cut from the bump page (`+0x20` cursor, `+0x1c`
limit; a new page needs `[0x0f0340] < 0x76` and sets `+0x18/+0x1c/+0x20`,
`+0x2d0`), and a class whose block is under `0x100` bytes (or under a page
for the large classes) also threads the following blocks onto its free
list, raising `CHARGED[c]` and `FREE[c]` for each, then the call counter is
restored. `CHARGED[c]` rises once for the returned block. Free (`0x0022ca00`)
pushes the block on its size's list, `FREE[c]` up, the counter down; a size
over `0x2000` calls an import. Tables: size to class bytes `+0xac`, large
classes `+0x2b0`, class sizes `+0x68`. **E**

`sim_havok::mem` models all of it over guest words on a write journal
(`Journal`); the engine adapter `sim_physics/alloc.rs` runs a call on the
journal first and answers natively only if the model covers the route.
Not covered, so the guest runs: the overflow object, a class free over
`0x2000` bytes, a null block under the LIFO push, a vtable other than the
two above, an unaligned or unmapped word. ecx, edx and the arithmetic
flags are scratch and left undefined; the callee-saved registers and
`eax` (allocations) are as retail. Halo calls these too (74 sites above),
so the entries are installed for every caller.

## The island solve

`0x0031c100`, cdecl, nine stack arguments in this order: step `W+0x150`,
solver parameters `W+0x160`, pair words `W+0x190`, gravity `W+0xe0`, the
island, its entity array and count, its constraint array and count; the
caller (`0x00316b16`) pops `0x24`. Callee-saved registers are restored;
`eax` is 0 when the island fits the buffer; after a LIFO block it is
what the release leaves: the descriptor's remaining bytes when inline,
else the dispatcher free's `eax` (the caller passes it on unread). **E**

**The buffer.** The solver buffer object `[0x00484b38]` holds base `+0x08`,
size `+0x0c` and the busy byte `+0x10` (set to 1 on entry, 0 before the
return). From the base, four areas, sums wrapping (`0x0031c142`):

| Area | Bytes |
|---|---|
| accumulators | `I+0x14 + 0x90`: a leading zeroed one, one per entity, the end byte 3 |
| rows | `I+0x0c` |
| impulses | `I+0x18 × 4 + 8`, zeroed from the start for `I+0x18` words (`0x0031c3c0`) |
| schemas | `I+0x10 + 4`, the last word the end schema `0x400` |

The island fits when the schemas' end is at or below the buffer's end,
unsigned (`0x0031c175`). Otherwise retail takes a LIFO block from the
descriptor at `[0x00480118]+0x08` (inline when the remaining bytes allow,
else `vt[+0x24]`, which is `0x002d7020`): for everything when even the
rows do not start inside the buffer, for impulses and schemas when the
rows fit, and for the rows' tail plus `2 × I+0x08` (with impulses and
schemas) when only the rows overflow; the Jacobian cursor's limit is then
the buffer's end less `I+0x08`. The block is released before the return,
inline or through `vt[+0x28]` when it is the descriptor's own payload or
the descriptor has none. **E**

**Accumulators.** `0x80` bytes. Byte 0 is the kind 2; byte `+0x0c` the
identity-basis flag the contact builder tests (`0x0031f6be`); `+0x10`
linear velocity, `+0x20` angular velocity, `+0x30` inverse inertia with
inverse mass in `w`, `+0x40..+0x7f` the centre and the transposed basis
while the rows are built. Per entity in array order, `motion+0x08`
becomes the accumulator's offset from the base, stored only when it
differs (`0x0031c2f0`); then `vt[+0x0c]` (step, gravity, accumulator;
`ret 0xc`) fills it and returns the next. Only bytes 0 and `+0x0c` of the
first 16 are ever written, so the rest keep the buffer's older contents:

- `0x0031a770` (type 7) zeroes `+0x10..+0x7f` and leaves the flag as the
  buffer held it, as the leading accumulator does.
- `0x00319df0` (types 2 and 3) first integrates the pending force
  (`motion+0xc0`, times `dt` and the inverse mass `+0x2c`), gravity and
  the torque (`+0xd0`, times `dt` and the scalar inverse inertia `+0x10`)
  into the velocities `+0x40` and `+0x50`, each damped by
  `1 - dt × +0x14` (`+0x18` for angular) raised to zero, and zeroes both
  buckets. The accumulator takes the velocities, the inertia splatted
  with the inverse mass in `w`, the centre `+0x70`, the identity basis and
  flag 1.
- `0x0031b300` (types 4 and 5) does the same from the force `+0xf0`; the
  torque `+0x100` is applied only while its `w` is nonzero, through the
  body frame (`0x0031af90`, inertia `+0xe0`), and then zeroed. The
  accumulator holds the angular velocity in the body frame, the inertia
  vector `+0xe0`, the transposed basis (`0x002da7a0`) and flag 0.
- `0x00319430` (type 6) copies `motion+0x40`, `+0x50` and `+0x70` to
  `+0x10`, `+0x20` and `+0x40`, zeroes `+0x30`, writes the identity at
  `+0x50`, `+0x64`, `+0x78`, and sets the flag to 1.

An entity outside the island (the level, in the fixed island) keeps
whatever offset its motion holds, usually 0: its accumulator is the
leading zeroed one.

**Constraints.** In array order: the pair words get A's and B's
accumulator addresses (`[c+0x0c]` and `[c+0x10]`, through their motions'
offsets), then the cursor moves to the overflow block if its row pointer
is at or past the limit, then `vt[+0x24]` (pair words, cursor; `ret 8`)
appends. The cursor is the frame's `+0x1c..+0x2c`: row pointer, schema
pointer, limit, overflow block. The contact class's `0x0031bdf0` calls
`0x0031f620` with the points `c+0x34`, their count `c+0x38`, the
properties `c+0x90`, `c+0x50`, the pair words and the cursor; its first
schema is `0x00011801` followed by the row pointer, the properties, A, B
and `0x1c`. **E**

**The contact builder.** `0x0031f620`, cdecl: points (`0x20` each,
position with the separation in `w`, normal), count, properties (`0x1c`
each), the friction block `c+0x50`, the pair words, the cursor. The pair
words past the two accumulators are scales: `+0x0c` the right-hand side,
`+0x10` the recovery cap, `+0x18` the pair coupling, `+0x1c` the friction
limit. After the header, per point in order (`0x0031f6b0`): a linear row
(`0x30` bytes: normal with the right-hand side in `w`, A's angular part
with the inverse effective mass in `w`, B's with the effective mass in
`w`; each angular part rotated into the body's frame unless the
accumulator's flag is set) and the schema `0x00030402`. When the point's
property word `+0x18` (16 bits) is nonzero, it is coupled with the point
before it: that schema becomes `0x00050803` followed by the off-diagonal
term, and both rows' `w` take the 2×2 inverse. The point writes property
`+0x14` (the recovery the next frame subtracts). **E**

The friction limit is the summed impulses (`+0x00`) times the mean
coefficient (`+0x08`) (`0x0031f9f6`). At or below zero, the tail
`0x00320280` stores the cursor and returns. Otherwise the friction normal
is the first point's normal, the normalised sum of the normals, or `+y`
(limit zero), by the constants `0x0045dc78`, `0x0045dc68` and
`0x0045dc84`. The block's axes `+0x20` and `+0x30` are re-orthogonalised
against it (`0x0031fb50`; a short second axis replaces the first from the
normal's components, limit zero). Two linear rows at the points' mean
follow, with the schema `0x00051407` (output `c+0x54`, stride 8). With
more than one point, an angular row about the normal (`0x20` bytes) turns
it into `0x00061808`, scaled by the twist radius `+0x1c`, which is
measured from the points while flag bit 2 is set and then cleared, unless
it comes out below `0x004155a8`, which leaves the two-row form. **E**

**Solve and export.** `0x0031c940` (parameters, accumulators, schemas,
impulses). At each pass's end it walks every accumulator of kind 2 from
the base to the end byte (`0x0031e0c0`), those no constraint names too,
setting the sums `+0x40`, `+0x50` to the velocities on the first pass and
adding them after. When it returns 1, each entity in array order gets
`vt[+0x10]` (step, `[info+0x20]`, accumulator; `ret 0xc`), which returns
the next accumulator. `0x0031a6a0` (type 7) writes nothing. Types 2 to 5
set the velocities to the iteration sums `+0x40`, `+0x50` times the
scale (the angular sum through the old basis for 4 and 5), masked by
`0x00415120` for 3 and 5, and save them (`+0xe0`, `+0xf0` for 2 and 3;
`+0xc0`, `+0xd0` for 4 and 5). Every type but 7 then integrates the
pose: the previous centre with the swept radius `+0x0c × dt × |angular|`
in `w`, the centre, the orientation by its rate (`0x00319300`) and one
Newton step on `rsqrtss` (`0x001d8d60`), the basis (`0x00372500`), and
the translation, the local centre `+0x20` taken off through the new basis
(types 2, 4) or the quaternion (`0x002db7a0`; 3, 5, 6). Types 2 and 4
finish by restoring the solved velocities `+0x10`, `+0x20` (the angular
one through the new basis for 4), masked. After the
iterations a write-back pass walks the schemas again and, for each row,
stores the impulse and the corrected right-hand side times `[info+0x10]`:
the per-point forms at the header's output pointer stepping by its
stride, the friction forms at their own pointer and stride. **E**

**The port.** `sim_havok::island_solve` runs the entry on a write journal
in stages: gather (every word read and checked before a write), the
accumulator offsets, the accumulators laid into a Rust-owned buffer
(`jacobian::Scratch`), the pair words and rows per constraint
(`jacobian::contact` for the contact class), the schema decode into
`solver::Stream` (`solver::decode`), `Stream::solve`, the write-back
pass, the exports and the busy byte. An island past the buffer's end
takes its LIFO block as retail does (`island_solve::place`): inline from
the descriptor, or through `0x002d7020` and `0x002d7090` as `sim_havok::mem`
models them, the scratch then covering the buffer and the block and the
decode following the rows into the block. The scratch records every word
it is given; after the solve it takes the solved accumulators, each row's
corrected right-hand side (`Decoded::row_words`) and the impulses, and
every recorded word goes to the journal before the block is given back,
so the buffer and the block hold what retail's solve leaves there. Words
no stage writes keep the guest's older bytes, as in retail; a later
island's fixed and leading accumulators read their flag from them. The
decline is decided before any write, so the guest runs from untouched
memory. An island falls back to the lifted entry, counted by reason
(`island_solve::fallbacks`, read with `havok.islands`), when its block would come from a route the
allocator model does not cover or its rows run past the buffer or the
block,
holds a motion whose two solver slots are not one of the six classes
(`motion_solve`, every type), holds a constraint whose
`vt[+0x24]` is not one of the seven ported classes, finds the solver
tables unbuilt, runs under an unmasked float exception or would load a
non-finite float into the x87 unit from memory (`float`), names an
accumulator outside the island, or is a contact retail would corrupt or
spin on (a paired first point, two paired points in a row, a friction
axis that stays short). `solver` counts a stream `Stream::solve`
declines.

## The island integrate

`0x00316a20`, its unconstrained virtuals and its port are in
[havok-integrate.md](havok-integrate.md#the-island-integrate), with the
integrate phase `0x0030fef0` and the deactivation test `0x00316b30` that
run it.

## Floats

`h2 fpstats --root 0x311100` (direct calls and jumps only): 86 functions
past the image's entry closure, 604 x87, 105 scalar SSE and 1,045 packed
SSE instructions, no `fldcw` or `ldmxcsr`. Following every vtable slot
of the classes above as well (an over-approximation): 847 functions in
`0x2d0000..0x325000`, about 4,600 x87 and these SSE counts:

| Instruction | Sites | Note |
|---|---:|---|
| `rcpps`, `rsqrtps`, `rcpss` | 0 | |
| `rsqrtss` | 24 | in 18 functions, the contact Jacobian builder `0x0031f620` (2) among them |
| `sqrtss` | 17 | the motions (`0x003194f7`, `0x0031a17d`, ...) and inertia setup |
| `divss` | 2 | `0x0030386a`, `0x00303a25` |
| `fsqrt`, `fdiv`, `fdivr` | 23, 73, 39 | x87 under PC=53 |
| `mulps`, `addps`, `subps`, `shufps`, `movmskps` | 1,230, 582, 421, 1,717, 71 | |

So §12's open risk resolves this way: the step uses `rsqrtss`, never the
packed forms. `simd::rsqrtss` is the definition the recompiled build
uses (FLOAT.md: held to the architectural bound, no Pentium III table),
so the port calls that function at each site and `Fast` cannot use a
host estimate there. Bit-for-bit means bit-for-bit against the lifted
build.

## Save, revert and rebuild

A checkpoint save (`0x00123e20`) runs the pre-save list at `0x0046e320`
(four entries) with flags 0, copies the arena, then calls `[0x0046e330]`.
A revert (`0x00123f60`) runs the five-entry list at `0x0046e334`
(`0x00199520`), copies the slot into the arena, then runs the 20-entry
list at `0x0046e348` (`0x00199540`). Physics owns the first entry of each
(`0x001c3540` before, `0x001c3590` after). **E**

- `0x001c3540` (flags): unless bit 0, `0x001c2b10` takes every object's
  physics out of the world and steps it once (above); with bit 1,
  `0x001c29d0` and `0x001c2890` tear the level's physics down.
- `0x001c3590` (flags): with bit 1, `0x001c2910` rebuilds the level body
  and `0x001c39c0` re-adds it; unless bit 0, `0x001c2a10` rebuilds the
  objects' bodies (pressure check `0x00146bf0`, `0x001c35f0`,
  `0x00278f00`, deactivation `0x001c50c0`).

So retail does not restore Havok from a copy: the heap (§3.1 of the
design) and the physics component array stay as they are, and every
object body is rebuilt from the reverted arena (**S** for the meaning of
the flag bits). The component array is not in the arena: `0x001cec30`
takes it from the allocator object `[0x00468758]`, whose allocate
(`0x0011ca20`) is its own virtual allocation. **E**

Consequences: heap addresses after a revert depend on the heap's history
before it, which the exact allocator reproduces; the derived Rust caches
drop on the world epoch (design §5) and need nothing more; a save also
removes and rebuilds every body, so a save changes `havok.digest`.

## The digest

`havok_agents::digest::read` walks, in order: the world pointer; `W+0x24`,
`+0x10c`, `+0x12c`, `+0x150..+0x1a7`, `+0x238..+0x257`; the active, then
inactive island arrays and the fixed island, each island's `0xb0` bytes,
entities (the entity to `+0x54`, its shape pointer's first word, its
properties, its whole motion by the motion's allocation size), constraints
(`0x18` bytes; for the contact class also the id map, points and
properties), pairs (each agent's allocation, a tree's children
recursively), the action array; the phantom array at `W+0x120` and each
phantom's `0x24`-byte header; the counter `W+0x130`; the pool's
bookkeeping and free-list order; the three overflow stacks' headers; the
solver buffer's base, size and busy byte. Addresses are hashed as they
are. **Code**

Not covered, by design: freed payloads, unused capacity, scratch (solver
buffer and LIFO contents), the monitor stream. Not covered yet: a joint
constraint past `0x18` bytes (slice 2, step 0, reads their sizes), a
phantom past its header (slice 7), the listener arrays (empty, above).

`havok.digest` (`{}` → `{digest, bodies, constraints, pairs, heap_used}`)
answers it at a tick boundary; any parameter is refused. The sim report
carries the same walk as the `havok.world` row of `state_components_v1`
when the report has components, so `h2 sim diff` names the first tick
where physics differs.

## Rules and limits

- The step entry has two callers. Whatever runs at `0x00311100` also runs
  in the save, revert and rebuild bracket, outside the tick.
- A class this page does not list reaching a ported entry is a fallback
  (design §8), counted, never a guess.
- `rsqrtss` goes through `simd::rsqrtss`; no other estimate is allowed.
- The digest changes when a save rebuilds the bodies. A/A comparisons
  compare runs with the same saves.

## Not verified

- Which motion types, joint classes, agents and shapes a multiplayer
  match and a co-op campaign level build: `havok.bodies`, `havok.agents`
  and `havok.contacts` in a run.
- That the per-island listener arrays stay empty, and that `W+0x6c` and
  `W+0x78` are not read in the step.
- The meaning of the save and revert flag bits.
- A store-by-store list per entry: `emit::sim::census`
  ([sim-tier-writes.md](sim-tier-writes.md)) rooted at each entry instead
  of `game_tick`, which also lists the indirect calls this page resolved
  by hand.
- Slice 0's exit: two A/A runs give equal `havok.world` components at
  every tick, and the digest differs when one body is nudged (the unit
  test `the_walk_counts_bodies_and_constraints_and_hashed_order` shows
  the second on synthetic data).

## Related

- [sim-rewrite-havok.md](sim-rewrite-havok.md): the design and the slices.
- [havok-runtime.md](havok-runtime.md): the object layouts.
- [FLOAT.md](../lifter/FLOAT.md): the float contract.
- [agent-api.md](../runtime/agent-api-world.md#havok-the-physics-world): `havok.*`.
