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
elements), `+0xc` bsp-physics `{count, addr@+0x10}` (0x70-byte runtime Havok
slots; the engine loops `+0xc` times pairing slot `i` with BSP `i`). The slots
are zero or absent in nearly every tag and their content is not decoded.

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
a 1.0 and a 0.05-like value) are not interpreted.

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

## Not established

- `coll`: the BSP leading word, the contents of the 0x70-byte physics slots,
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
