# Collision models (`coll`) and physics models (`phmo`)

Implementation: `crates/tags/src/collision.rs` (hand-written codecs, not in
`schema.rs`; synthetic tests in `collision_tests.rs`). CLI: `h2 collision`
in `crates/cli/src/collision.rs`. Engine addresses are in
[engine-map.md](../engine/engine-map.md) ("Collision and physics model layouts").
No game data, tag names or dumps are committed; tests build invented payloads.

```sh
h2 collision somemap 'objects\example\widget\widget'                       # summary + validation
h2 collision somemap 'objects\example\gadget\gadget' --group phmo --obj gadget.obj
h2 collision --all mainmenu          # validate every coll and phmo tag in one map
h2 collision --all all               # ... in every map on the disc (non-zero exit on any failure)
```

`--obj FILE` writes a Wavefront OBJ to the path you choose: coll gives every BSP
surface as fan-triangulated polygons (one object per region/permutation/BSP);
phmo gives the convex shapes in shape-local space (polyhedron hulls, boxes,
UV-tessellated spheres and pills; rigid-body node placement is not applied).
The file is only written when the model passes validation (a failing model
neither creates nor, with `--force`, replaces a file). It goes through
`safe_out::OutDir` with `create_new` semantics, one path component at a time
from `.` (relative) or `/` (absolute), so nothing is overwritten without
`--force` and a symlink anywhere in the parent chain is refused (canonicalise a
path such as `/tmp` first); `..` is rejected.
When a name exists as both `coll` and `phmo`, `--group` is required for `--obj`.

## Provenance

Evidence labels as in [map-format.md](map-format.md): **V** verified on the
owner's retail disc, **E** established by the engine's own code (XBE
disassembly/decompilation, addresses in engine-map.md), **D** inferred from the
shape of the data only.

- Every layout below was derived from the disc and from engine code. No
  community layout file, XML, schema or SDK header was opened or transcribed
  for this work. Field and block names are descriptive names of our own
  (region, permutation, plane, surface, rigid body ...); they are not claimed
  to match anyone's naming.
- Halo 2 embeds Havok. The only Havok knowledge used is what the engine itself
  shows: class-name strings in `.rdata`, the shape-type switch at `0x001eaf30`,
  the post-process at `0x001ea9c0` (which patches vtable pointers into elements
  at known offsets) and the data. Where a Havok object sits inside a `phmo`
  element, its fields are identified from the numbers they hold (a unit-plane
  test, a convexity test, a symmetric tensor), not from any SDK.
- Both tags are one contiguous payload: a fixed root followed by every child
  block. A block is `{count, address}` (two `u32`) and the address is a virtual
  address inside the same payload. Blocks may alias; decoding is bounded by a
  work budget proportional to the payload (64x its length).
- The padding word `0xaaaaaaaa` that follows some blocks pads the block to a
  16-byte multiple relative to its start (this is why raw spans between blocks
  read as 12 or 16 for the same 12-byte element). Element sizes below are the
  real ones.

## `coll` root (0x34 bytes)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00..+0x14` | five words, zero on every retail tag | V (kept as `reserved`) |
| `+0x14` | materials: 4-byte elements, a string ID each. `surface.material` indexes this | D + V (every material index of every surface is in range) |
| `+0x1c` | regions, 12 bytes: `name`, `permutations {count, addr}` at `+4` | E (`0x002122b0`) |
| `+0x24` | pathfinding spheres, 0x14 bytes: `i16 node`, `u16 flags`, `center[3]`, `radius` | D: node in range, radius finite and non-negative on all 2,842 tags. No engine consumer located. |
| `+0x2c` | nodes, 12 bytes: `name`, `i16 region`, `i16 parent`, `i16 next_sibling`, `i16 first_child` | D: a tree (parent/child/sibling indices in range). No engine consumer located. |

Permutation, 0x14 bytes: `+0` name, `+4` bsps `{count, addr}` (0x44-byte
elements), `+0xc` BSP physics `{count, addr@+0x10}` (0x70-byte Havok slots;
the engine loops `+0xc` times pairing slot `i` with BSP `i`; below).

### BSP physics (0x70 bytes)

How Havok sees a static scenery object that has a `coll` and no `phmo`:
a Havok shape over the collision BSP, inside a MOPP tree. Havok moves bipeds (their
shapes, bipd `+0x264`, get Havok vtables in `0x001c2390`), vehicles and
loose items, so scenery with neither a `phmo` nor these slots is not
there for them: the player walks through it (Guess for the walk itself:
inferred from the biped's Havok shapes, not traced through the biped's
movement code).

**The rule (V, `retail_static_scenery_carries_bsp_physics`, every map on
the disc: 7,033 permutations, 280 slots):** a `coll`'s permutations have
one slot per BSP exactly when a scenery object (`scen`) with no `phmo`
uses it, and none otherwise. Weapons, equipment, devices, bipeds and
crates (`bloc`) without a `phmo` have no slots; nothing with a `phmo`
has any. (A weapon's `coll` has slots on a map that also places the
weapon as scenery.)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | Halo BSP shape: vtable (`0x00456194` written at load), `+0x06` u16 `0x81` in the tag (`0x80` written), `+0x08` zero, `+0x0c` the BSP (element `+4`, written at load) | E `0x00246c60` |
| `+0x10` | f32×3 (+pad) centre of the BSP's bounding box | E: the shape's `getAabb` (`0x001ef040`, vtable `+0x18`) passes `+0x20` and `+0x10`; V: the box of the BSP's vertices |
| `+0x20` | f32×3 (+pad) half extents | E, V as above |
| `+0x30` | u32 a tag datum (retail: a tool-time datum, the same for every slot of a tag; not read by anything found) | D |
| `+0x40` | 16-byte wrapper (vtable `0x00455540`) pointing at `+0x50`; `+0x46` u16 `0x80` in the tag | E `0x00246c60` |
| `+0x50` | MOPP tree shape (vtable `0x00455f58`): `+0x5c` = the slot's shape, `+0x60` = the code (copied from `+0x68`); `+0x56` u16 `0x81` in the tag | E `0x00246c60` |
| `+0x64` | byte block `{count, address}`: a MOPP code object as in [mopp.md](mopp.md), the count its size rounded to 16 | E (`+0x68` read before the rest is written), V (every retail slot's block parses and validates) |

The code's keys are plain surface indices: on every retail slot it emits
exactly `0..surface_count`, each once (V). The shape's child count is the
BSP's surface count (`0x001ee470` reads BSP `+0x28`, one less when the
last surface has flag `0x10`). The engine fills in every pointer and
vtable at load (`0x002122b0` → `0x00246c60`, which skips a BSP with no
surfaces); the tag's own bytes that matter are the box and the code.
`tags::collision::BspPhysics` decodes them; `h1objects::coll` writes them
for converted Halo 1 objects.

### `hlmt` region bytes

`hlmt` regions (`0x10`, `+0x70`) carry `i8` collision region at `+4` and
`i8` physics region at `+5`; their permutations (`8`) `i8` collision
permutation at `+5` and physics permutation at `+6`. -1 is none. D: on
every map, a region of an object with no `coll` holds -1 in its collision
byte (1,224 of 1,230 regions, counted per object), and the physics byte is never -1 on an
object with no `phmo`; a two-region object whose second region uses the
`coll`'s region 1 holds 1. The engine's reading of them is not traced.

### BSP element (0x44 bytes)

`+0` is a word that varies 0..=7 (unknown). Eight blocks follow at `+4 + 8*i`.
The engine passes a pointer to element `+4`, so its own offsets (`0x00` nodes ...
`0x3c` vertices) are the block offsets minus 4.

| Block (elem offset) | Element | Layout | Evidence |
|---|---|---|---|
| nodes (`+0x04`) | 8 | `u16 plane`, `back` = 24-bit at `+2`, `front` = 24-bit at `+5`; bit 23 = leaf, `0xffffff` = none | E (`0x001de6d0`, `0x0014a280`): child read as `i32 at +1 + 3*side >> 8`, side 1 when `n.p - d >= 0`. V |
| planes (`+0x0c`) | 16 | `nx, ny, nz, d`; `n.p - d = 0` | E, V: all unit length within 1e-3 |
| leaves (`+0x14`) | 4 | `u8 flags`, `u8 ref_count`, `u16 first_ref` (0xffff when empty) | E (`0x001ddea0`) |
| refs (`+0x1c`) | 4 | `u16 plane` (bit 15 flip), `u16 node` (bit 15 set = surface, low 15 bits) | E (`0x001ddea0`, `0x001dea00`) |
| 2d nodes (`+0x24`) | 16 | `a, b, d` floats (`a*x + b*y - d`), `back` `i16` at `+0xc`, `front` `i16` at `+0xe`, bit 15 = surface | E (`0x001de220`) |
| surfaces (`+0x2c`) | 8 | `u16 plane` (bit 15 flip), `u16 first_edge`, `u8 flags`, `u8 breakable_surface` (0xff none), `u16 material` | E (`0x001de2c0`, `0x001de6d0`) |
| edges (`+0x34`) | 12 | `start_vertex, end_vertex, forward_edge, reverse_edge, left_surface, right_surface` (all `u16`; `0xffff` surface = none) | E (`0x001de2c0`): ring rule below |
| vertices (`+0x3c`) | 16 | `x, y, z`, `u32 edge` | E (position), D (edge field: always a valid edge) |

Ring rule (the engine's surface walk): starting at `first_edge`, if
`edge.right_surface == surface` take `end_vertex` and continue at
`reverse_edge`, otherwise take `start_vertex` and continue at `forward_edge`,
until the first edge comes round again. This yields each surface's polygon.

Names `back`/`front`, `left`/`right` and `forward`/`reverse` describe what the
engine does with them (child chosen when the point is on the negative/positive
side; which edge/vertex is followed). They are not claims about winding.

### Structural validation (`Collision::validate`)

All run on every tag by `h2 collision --all`, and have **zero failures across
all 30 maps** (2,842 `coll` tags, 11,972 BSPs, 293,460 surfaces; shared tags are counted once per map they appear in):

- every node/leaf/ref/surface/edge/vertex/material index in range, including
  `0xffff` rules (empty leaf, no surface on an edge);
- plane normals unit length (`|n| - 1| <= 1e-3`), all floats finite;
- the 3D tree and the 2D tree acyclic (iterative three-colour DFS; shared
  children are allowed, back edges are not);
- every surface's edge ring closes within the edge count and each edge on it
  belongs to that surface;
- every ring vertex lies within `1e-2` of its surface's plane (a strong check
  that plane, edge and vertex layouts and the ring rule are right);
- region/node/sphere indices in range.

Work and output are budgeted so a hostile tag cannot stall the tool: all ring
walks of a BSP share a step budget of `4*edges + surfaces + 64` (legitimate data
walks each edge at most twice), and an OBJ export stops with an error past
`MAX_OBJ_TRIANGLES` (2,000,000).

## `phmo` root (0x110 bytes)

`+0` flags (zero on retail), `+4` mass (float), `+8`/`+0xc` two floats (1,1 or
2,2). The rest is block descriptors, each `{count, address}` at the offset
shown. **E** = stride from `0x001ea9c0`/`0x001eaf30`; **V** = stride confirmed
by the spacing of consecutive blocks across all 2,273 retail tags.

| Offset | Block | Element | Evidence |
|---|---|---|---|
| `+0x28` | unmodelled, stride varies (nested children) | count kept | D |
| `+0x30` | unmodelled (element 0x18 plus nested children) | count kept | D |
| `+0x38` | rigid bodies | 0x90 | E, V |
| `+0x40` | materials | 12 (name, word, `i16`, `u16`) | V |
| `+0x48` | spheres | 0x80 | E, V |
| `+0x50` | type 5 | 0xb0 | E only; **no instance on the disc** |
| `+0x58` | pills | 0x50 | E, V |
| `+0x60` | boxes | 0x90 | E, V |
| `+0x68` | triangles | 0x60 | E only; **no instance on the disc** |
| `+0x70` | polyhedra | 0x100 | E, V |
| `+0x78` | four vectors | 0x30 | E, V |
| `+0x80` | plane equations | 0x10 | E, V |
| `+0x90` | lists | 0x38 | E, V |
| `+0x98` | list children | 8 | E, V |
| `+0xa0` | mopps | 0x14 | E, V |
| `+0xa8` | mopp code bytes | 1 | E (cursor rule), V |
| `+0xb8` | constraints (kept raw, name decoded) | 0x94 | V (span) only |
| `+0xc0` | regions, 12 bytes: `name`, permutations `{count, addr}`; permutation 12 bytes (`name` + 8 words kept raw) | 12 | V |
| `+0xc8` | nodes, 12 bytes: name, four `u16` (kept raw) | 12 | V |
| `+0xe8` | constraints (raw) | 0x84 | V (span) only |
| `+0xf0` | constraints (raw) | 0x78 | V (span), 4 instances |
| `+0x108` | type 6 shapes (raw) | 0x20 | E, V |

### Shapes

Spheres, pills, boxes, triangles and polyhedra start with a 0x20-byte header:
`+0` name, `+4 u16` material (index into `+0x40`; V: always in range). The rest
is decoded as `unknown_*` fields named by offset (`+6 u16`, floats at `+8`,
`+0xc`, `+0x10`, `+0x14`, `+0x18`, `+0x1c u16`, `+0x1e u8`, `+0x1f u8`): the
values look like a mass-scale/friction/restitution/volume/mass group but no
consumer in the code examined reads them, so no semantic name is claimed (D
only; all floats are checked finite). The Havok object follows at `+0x20` (E: the engine
writes the vtable pointer and a reference count of `0x80` at `+0x26`).

| Shape | Fields | Evidence |
|---|---|---|
| sphere (0x80) | radius `+0x2c`; transform shape at `+0x30` with rotation rows `+0x40/+0x50/+0x60` and translation `+0x70` | D (radius and centre are plausible on all instances); transform offset E (`0x001eaf30` returns `+0x30`) |
| pill (0x50) | radius `+0x2c`, end points `+0x30` and `+0x40` | D; V: the radius is repeated in the `w` of both end points on 1,242 of 1,263 pills |
| box (0x90) | half extents `+0x30`, convex radius `+0x3c`, transform shape at `+0x40` with rotation `+0x50..` and translation `+0x80` | D; V: the three rotation rows are unit length on all 1,117 boxes; transform offset E (`0x001eaf30` returns `+0x40`) |
| polyhedron (0x100) | AABB half extents `+0x30`, centre `+0x40`; four-vector groups `+0x54` (capacity `+0x58`, bit 31 flag); vertices `+0xf0`; planes `+0xf8` | E for the cursors and capacity rule; V for the vertex layout and plane convention (see below) |
| list (0x38) | child count `+0x10`, capacity `+0x14`; children `{u16 type, u16 index, u32 runtime}` inline at `+0x18` (capacity <= 4) or in the `+0x98` block | E, V |
| mopp (0x14) | child `{u16 type, u16 index}` at `+0xc` (a list); its code is the next run of the code block, size `u32` at code `+0x20` rounded up to 16 | E (`0x001ea9c0` cursor), V |

Four-vector groups hold four vertices transposed: `x[4]`, `y[4]`, `z[4]`
(0x30 bytes). External groups are taken from the `+0x78` block, and planes
from the `+0x80` block, in polyhedron order; the totals must equal the block
counts (checked). Plane equations are `[nx, ny, nz, w]` with `n.p + w <= 0`
inside (V: every vertex of every polyhedron satisfies this, and every plane
touches a vertex).

Shape types (E, `0x001eaf30`): 0 sphere, 1 pill, 2 box, 3 triangle, 4
polyhedron, 5 (0xb0 block, never seen), 6 (0x20 block), 0xe list, 0xf mopp.
Names for 5 and 6 are positional only.

### Rigid bodies (0x90 bytes)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00 i16` | node (-1 = none) | E (`0x001ef070` uses it as a node-matrix index with -1 test), V (in range) |
| `+0x08` x3, `+0x14` | bounding-sphere offset and radius | D |
| `+0x38` | shape reference `{u16 type, u16 index}`; the engine overwrites it with the resolved pointer | E (`0x001ea9c0`, `0x001eaf30`, `0x001ef070`), V (in range) |
| `+0x3c` | mass | D; V (finite, non-negative) |
| `+0x40` | centre of mass (xyz) | D |
| `+0x50`, `+0x60`, `+0x70` | inertia tensor, three rows of xyz (w unused) | D; V: symmetric on every body |

The element is kept verbatim in `RigidBody::raw`; `+0x18..+0x38` (four words,
a 1.0 and a 0.05-like value) are not interpreted by the reader. What the
engine does with the first words when it adds a body to the Havok world:

| Offset | Meaning | Evidence |
|---|---|---|
| `+0x18 u16` bit 1 | the body goes on collision layer 2 instead of its default layer | E: `0x001d5bf0` passes the bit to `0x001d70d0`, which takes 2 over the default from `0x001d69d0` |
| `+0x18 u16` bit 0 | read at `0x001d5d81` (set, or the object is a vehicle, sets a local flag); effect not established | E (read only) |
| `+0x1a u16` | motion: 0..5 become Havok motions 2..7 (7 fixed, 6 keyframed) | E: jump table `0x001d5bd4` in `0x001d5940` |
| `+0x1e u16` | not established (1 on every retail biped body) | V |

How a body's motion and layer are chosen (E, `0x001d5940` and callers):

- An object whose header byte `+2` bit 0 is clear, other than a machine,
  gets every body fixed (motion 7).
- Object creation (`0x001cf19c`) asks for keyframed bodies: a body that is
  not fixed becomes keyframed (6) unless its node (`+0xc8` block, `+4` bit 0)
  is flagged. The other callers (`0x001d56c0`, `0x001d5747`, `0x001d6c2d`)
  do not ask, and the motion stays as `+0x1a` says; which of them is the
  ragdoll is not established.
- The default layer (`0x001d69d0`): fixed bodies 1 (12 on a vehicle);
  otherwise by object type through the tables at `0x001d6ab0` and
  `0x001d6ac4`. A biped's is 9, or 11 when its `bipd` byte `+0x10a` has
  bit 2.
- `0x001c302d` enables every layer pair, then disables a fixed list
  (through `0x002df990`). Layer 2 collides only with layers 0, 1 and 2.
  Layer 1 collides with everything but 7, 8 and 14; layer 9 with 0, 1, 3,
  5 to 9, 12 and 14.

V (`h1objects/tests/biped_physics_disc.rs`, `retail_rigid_body_words_by_kind`):
every body of the Chief's and the Covenant's ragdolls (`masterchief`,
`elite`, `brute`, `hunter`, `jackal`, `bugger`; 59 bodies) holds flags 3,
motion 2 and `+0x1e` 1. The grunt's hold flags 1, the marines' 0. Vehicles,
devices and scenery mostly hold flags 0.

So a living Chief's bodies sit on layer 2 whether fixed or keyframed, and
meet only layers 0 to 2. Converted Halo 1 bipeds held flags 0 until
converter 58, which put their bodies on layer 1 (fixed) or 9 (keyframed),
where they meet most other layers. They now take the Chief's 3
(`rebuild::BIPED_BODY`). Which layer the structure and the player's own
movement queries use is not established, so whether this caused the
standing bounce needs a run.

### Structural validation (`PhysicsModel::validate`)

Zero failures across all 30 maps (2,273 `phmo` tags): every shape reference in
range for its type; masses finite and non-negative; inertia tensors symmetric;
material and node indices in range; polyhedron vertex counts match their
groups; plane normals unit length; every polyhedron convex (no vertex outside a
plane, each plane touches a vertex); external four-vector, plane, list-child and
mopp-code cursors consume their blocks exactly; all numeric fields the model
and exporter use (masses, transforms, extents, bounds, header floats) finite.
Compound shapes are checked as one graph: lists, and mopps (which wrap a list),
must not form a cycle, e.g. list -> mopp -> list. As the engine consumes them, a
mopp's child must be a list with a valid index (`0x001eae11`..`0x001eae24` index
the list block unconditionally) and each type-6 element's child `{type, index}`
at `+0xc` must be a valid reference (`0x001eaeb6`..`0x001eaed3`). Each mopp code
run is checked independently while decoding: at least `0x24` bytes and wholly
inside the remaining code block. Polyhedra are limited to 4,096 planes and
vertices, and plane x vertex work is budgeted (`MAX_VALIDATE_WORK`); exported
hull faces are deduplicated by vertex set and capped by `MAX_OBJ_TRIANGLES`.
Capsules are built along their end-point axis, not a fixed one.

## What the engine does with a BSP

How the engine answers "which surface" from the blocks above; modelled in
`h1convert::surface_query` and checked on the disc by
`crates/h1convert/tests/collision_engine.rs`. The same code serves object
and structure BSPs.

| Step | Rule | Evidence |
|---|---|---|
| Vector test `0x001de6d0` | Recursive over the 3D tree with `t` in `[t0, t1]`. Both ends on one side (`d >= 0` is front): that child. Else split at `t = -(n.o - d)/(n.v)`, near child first (back when `n.v > 0`), then the far child, remembering the crossed plane. Leaf states: solid 3, a leaf with flag bit 0 2, any other leaf 1. A front-facing hit (query flag bit 0) is state 1 or 2 followed by 3, the surface looked up in the previous leaf; a back-facing one (bit 1) 3 followed by 1 or 2; unless query bit 2, 2 followed by 2 is a two-sided hit. A surface with flag bit 1 is skipped under query bit 3, flag bit 3 under bit 4 | E |
| Fall-through | When the lookup finds no surface (`-1`) the walk goes on: the segment passes into solid unreported | E `0x001de8f1`, `0x001de921` |
| Reference matcher `0x001dea00` | Every reference of the leaf whose `plane & 0x7fff` is the crossed plane (the flip bit is not compared); the point is projected and the 2D tree walked from the reference's node; a surface is accepted when `0x001debd0` puts the point inside its ring (and, for a breakable surface, its bit is set in the breakable mask) | E |
| Projection | Dominant axis: z when `|nz| >= |ny|` and `|nz| >= |nx|`, else y when `|ny| >= |nx|`, else x. Sign `(n[axis] > 0) != flip`. Table `0x00440b94` (three `i16` per entry, `(axis, sign)`): x `(z, y)`, `(y, z)`; y `(x, z)`, `(z, x)`; z `(y, x)`, `(x, y)` | E `0x001dea5c`..`0x001deb4c`, `0x001de0c2`..`0x001de1e1` |
| 2D point walk `0x00296520` | Front child (`+0xe`) when `a*u + b*v - d >= 0`, else back (`+0xc`); a child `0xffff` is none | E |
| Volume query `0x001ddea0` (sphere) | At a node the sphere straddles, the plane is recorded with bit 15 set before the back child and clear before the front child; in a leaf only references whose `plane` (flip bit included) equals a recorded entry are walked, with the 2D sphere walk `0x001de220`. So a reference's flip bit must say the leaf is on its plane's back | E |
| Breakable mask `0x4ed280` | Filled with ones at map reset (`0x00183ec0`): every breakable surface starts intact; Havok's structure key filter `0x001ef451` and the matcher skip a breakable surface only once its bit is cleared | E |
| Havok surface shape `0x001edd00` | Havok's child shape for a structure key is the surface's ring as one polygon, its vertices written inline after a `0x20`-byte header with no count check | E |

V (every retail structure BSP on the Halo 2 disc, 2,427,337 surfaces
reached through 2D references): each surface's centroid, projected by
its reference's plane and flip, walks its reference's 2D tree back to
it, every time; every reference is on its leaf's path on the side its
flip bit says (1,596,791 references); leaf flag bit 0 is set exactly on
leaves whose references reach a two-sided surface (one leaf of 611,684
differs); rings have at most 8 vertices. Halo 1's data meets all of it
(below and [map-format.md](../halo1/map-format.md)).

## Not established

- `coll`: the BSP leading word (V: on every retail object it is a node
  index in range, e.g. `0, 2, 3` on a five-node weapon; not proven
  against the engine), the BSP physics slot's `+0x30` datum,
  `flags` of pathfinding spheres and leaves/surfaces beyond the bits the engine
  tests, and the engine's use of regions, nodes and pathfinding spheres.
- `phmo`: blocks at `+0x28` and `+0x30`; the three constraint blocks beyond
  their strides; the type-5/triangle layouts beyond stride and Havok offset
  (no disc instance); the type-6 shape; rigid-body bytes `+0x18..+0x38` and
  `+0x80..+0x90`; whether the stored rotations act on rows or columns (the OBJ
  export assumes `R * v`); the mass-distribution table.
- Structure-BSP (`sbsp`) collision: the BSP at `sbsp+14` has the same eight
  blocks without the leading word (element `0x40`) and decodes with
  `Bsp::decode_structure`; see [pathfinding.md](pathfinding.md). Its other
  `sbsp` neighbours are not decoded here.
