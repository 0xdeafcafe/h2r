# AI pathfinding data (`sbsp` +0xc4) and generating it

Halo 2's AI walks on *sectors*: convex polygons joined by *links*. A
structure BSP carries them. Halo 1 maps don't, so a Halo 1 converter has to
generate them from collision. `h2 h1 convert` writes generated
pathfinding into each converted campaign BSP (`crates/h1campaign`,
[campaigns.md](../game/campaigns.md) §9); multiplayer maps still leave
`sbsp+0xc4` empty ([h1-multiplayer.md](../game/h1-multiplayer.md)). This page is the layout as retail levels
carry it, how it relates to the collision BSP, and the generator in
`crates/pathfind`.

Code:

- `crates/tags/src/pathfinding.rs`: decoder, ring walk, surface lookup,
  validator (`PathfindingData`).
- `crates/tags/src/collision.rs`: `Bsp::decode_structure` decodes the
  structure BSP's collision BSP (the `coll` BSP layout without its leading
  word).
- `crates/pathfind`: generator (`generate`), comparison metrics
  (`compare`), and the `map::build` block tree (`build::element`).

Evidence labels as in [map-format.md](map-format.md): **V** checked on the
owner's disc (all 64 campaign BSPs that carry pathfinding, unless a level
is named), **S** from public sources, not checked. No engine code was read
for this page (see "Open questions"), so nothing here is labelled E. Field
names are ours. No disc bytes, dumps or per-map listings are committed;
tests use invented data, and the disc test is `#[ignore]`.

## Where it lives

| Field | Meaning | Evidence |
|---|---|---|
| `sbsp+14` block, stride `40`, count 1 | Collision BSP: eight blocks (nodes, planes, leaves, 2D refs, 2D nodes, surfaces, edges, vertices) with the strides of [collision.md](collision.md) | V: each child block starts where the previous ends; `Bsp::validate` passes on every BSP |
| `sbsp+c4` block, stride `74`, count 0 or 1 | Pathfinding data | V: the first child block starts `0x74` bytes after the element |

The pathfinding data is inside the `sbsp` tag, not a separate resource.
Multiplayer maps, `00a_introduction`, `mainmenu` and the shared caches
have none (count 0) (V). So the engine runs without it, at least where no AI
spawns.

## The element (`0x74` bytes)

| Offset | Block / field | Stride | Evidence |
|---|---|---|---|
| `00` | sectors | 8 | V: span / count |
| `08` | links | `10` | V |
| `10` | refs, **one per collision surface** | 4 | V: count equals the collision BSP's surface count on every BSP |
| `18` | 2D BSP nodes | `14` | V |
| `20` | unknown block | ? | V: empty on every BSP; the decoder refuses a non-empty one |
| `28` | vertices, f32 x, y, z | `c` | V |
| `30` | object refs (child blocks) | `1c` | V: span to the first child pointer; not interpreted, kept raw |
| `38` | compiled hints | `14` | V: span; not interpreted, kept raw |
| `40` | instanced-geometry refs, u32 | 4 | V: count equals the BSP's placement count (`sbsp+140`); u16 index or `ffff`, then u16 0 |
| `48` | u32 structure checksum | | V: equals `sbsp+08` on every BSP |
| `4c`..`6c` | four blocks | | V: always empty; refused when not |
| `6c` | user-placed hints, one element | `48` | V: count is 1 on every BSP; nine child blocks of editor source geometry (points, rays, jump-like records). Not interpreted |

S: community tag definitions call these blocks sectors, links, refs, BSP2D
nodes, surface flags, vertices, object refs, pathfinding hints, instanced
geometry refs, structure checksum, and user-placed hints, in this order.
The order matches the disc; the `+20` block would be "surface flags".

### Sector (8)

| Offset | Field | Evidence |
|---|---|---|
| `0` u16 | flags | V: histograms below |
| `2` u16 | hint index, `ffff` none | V: `ffff` on most sectors |
| `4` u32 | first link of the sector's ring | V: every ring closes |

### Link (`10`)

| Offset | Field | Evidence |
|---|---|---|
| `0`, `2` u16 | vertex 1, vertex 2 | V: in range |
| `4` u16 | flags | V: histograms below |
| `6` u16 | hint index, `ffff` none | V |
| `8` u16 | next link around the left sector | V: ring rule |
| `a` u16 | next link around the right sector | V: ring rule |
| `c`, `e` u16 | left sector, right sector; `ffff` none | V |

**Ring rule** (V: every sector ring on every BSP closes within the link
count): from `first_link`, on a link whose left sector is this sector
continue at `+8`, otherwise at `+a`. The sector's polygon is vertex 1 of
links where it is the left sector, vertex 2 where it is the right one.
This is the collision-edge rule of [collision.md](collision.md) with the
roles renamed. Floor sectors wind counter-clockwise seen from above (V:
9442 of the 11191 sectors of `01a_tutorial` are convex and CCW in XY;
1706 are convex and CW and belong to walls, ceilings and objects; 43 are
neither).

Retail data also keeps **orphan links** (no sector either side; 0 to 181
per BSP) and **stale links** that name a sector, are on no ring, and have
next links of `ffff`. No ring reaches them. The validator skips both.

### Refs and the 2D BSP

A ref is a u32: bit 31 set means a 2D BSP node (low 31 bits), clear means a
sector index, `ffffffff` nothing (V: values without bit 31 never exceed the
sector count, values with it never exceed the node count).

`refs[i]` belongs to collision surface `i`. A walkable surface covered by
one sector names it directly; a surface split between sectors names the
root of a 2D BSP over XY. A node (`14` bytes) is a line `a, b, d` (f32 at
`0`, `4`, `8`) and two refs: `+c` where `a*x + b*y - d >= 0`, `+10` where it
is negative. V: with this side rule, the centroid of a surface's
fan triangles lands in a sector whose polygon contains it for 99.1% to
100% of samples per BSP; with the sides swapped, most land outside. Nodes
are stored children first (V: every child node index is below its
parent's).

So the lookup is: find the collision surface under a point (the collision
BSP query), then `refs[surface]`, then walk the 2D BSP with the point's x
and y. `PathfindingData::sector_for_surface` does exactly that.

### What the sectors are (V, from `crates/pathfind/examples/analyze.rs`)

- Only part of the sectors come from the structure's collision. On
  `01a_tutorial`, 3115 of 11191 sectors are reachable from refs; the rest
  belong to instanced geometry and objects (object refs). Sector flag bit 3
  (`0x8`) is set on exactly those 3115 and on 4 others.
- **Walkable** means slope: the steepest surface with a ref and the
  shallowest surface without one meet at a normal z of 0.707 (cos 45°) on
  `01a_tutorial`; on the other levels a few steeper surfaces carry partial
  2D BSP coverage.
- Some sectors lie on walls and ceilings (refs from surfaces with normal z
  near 0 or −1). Their purpose is open.
- Coplanar neighbours are often merged: of the sectors a surface names
  directly, 926 are one surface, 388 cover 2 to 50 surfaces.
- Only 4563 of 15327 vertices on `01a_tutorial` sit at a collision vertex
  (1 cm): object sectors and 2D BSP cuts make new ones.

Flag bits, `01a_tutorial`, for observation only. Sector: bits 0, 3, 4 on
the common floor value `0x19`; bits 2, 5 only on sectors not reached from
collision; bit 13 almost only on sectors that are. Link: bits 6 and 7 only
on two-sided links; bits 0, 1, 9 to 13 on both kinds. S: community
definitions name sector bits (pathfindable, walkable, floor, wall
directions, crouchable, stairs, ...) and link bits (from collision edge,
intersection, threshold, ledge, wall base, start/end corner, ...). Which
bit is which is not established here.

## Generator (`pathfind::generate`)

Design, kept simple so the engine's lookups have nothing unusual to meet:

1. A collision surface is walkable when its plane normal (bit 15 of the
   plane index flips it) has z ≥ cos 45°, its edge ring is well formed, and
   its XY area is not tiny.
2. Each walkable surface becomes one sector with the surface's polygon.
   Collision surfaces are convex, and with z ≥ 0.707 their XY projection
   stays convex and CCW (V: collision rings wind with their plane normal
   on 15662 of 15663 surfaces of `01a_tutorial`).
3. Each collision edge of a walkable surface becomes one link, oriented as
   the edge (start → end), left/right sector from the edge's left/right
   surface when walkable, `ffff` otherwise. An unused next-link field
   points at the link itself (in range, never walked).
4. `refs[surface]` names the sector directly; no 2D BSP nodes.
5. Sector flags `0x19`, link flags `0x1`, hints `ffff`; instanced refs
   `ffff` for every placement; checksum `sbsp+08`; one zeroed user-hints
   element; no object refs or compiled hints.

`build::element` turns the result into a `map::build::Block` of `0x74`
bytes with a `Block` fixup per table; a caller writes
`sbsp.block(0xc4, 1, element)` ([map-builder.md](map-builder.md)). It
refuses object refs and non-zero user hints, whose child pointers it would
not know how to relocate. A synthetic test builds a whole map with it and
reads the BSP back through `map::Map` to the same data; the disc test does
the same for every generated BSP of three levels.

## Comparison on retail levels

`cargo run --release -p pathfind --example compare [map ...]` (needs
`H2_DISC` or `disc/halo2.iso`). Samples are the centroids of the fan
triangles of every upward-facing collision surface, weighted by XY area.

- **coverage**: share of the area retail covers (the sample resolves through
  refs and the 2D BSP to a sector containing it) that generated data also
  covers. **excess**: share of the generated area that retail leaves
  uncovered.
- **misplaced**: samples that resolve to a sector that does not contain them.
- **reach agreement**: 20,000 random pairs of samples covered by both; are
  they in the same connected component (sectors joined by two-sided links)
  in both datasets?

Results on the owner's disc, for the levels asked for (all 64 BSPs: generated data
passes the validator on every one, coverage ≥ 0.991, misplaced 0 except
one sample on `06a_sentinelwalls` BSP 1):

| Level, BSP | Retail sectors / links | Generated | Coverage | Excess | Misplaced (retail) | Reach agreement |
|---|---|---|---|---|---|---|
| 01a_tutorial 0 | 11191 / 27488 | 3733 / 10395 | 1.000 | 0.000 | 57 of 13429 | 0.914 |
| 03a_oldmombasa 0 | 6396 / 17346 | 3775 / 10136 | 1.000 | 0.008 | 101 of 11459 | 0.934 |
| 03a_oldmombasa 1 | 5552 / 12377 | 2697 / 5314 | 1.000 | 0.000 | 41 of 5112 | 0.976 |
| 03a_oldmombasa 2 | 6309 / 13308 | 2629 / 6372 | 0.999 | 0.025 | 93 of 9604 | 1.000 |
| 05a_deltaapproach 0 | 5151 / 13854 | 5131 / 10707 | 1.000 | 0.001 | 42 of 11556 | 0.998 |
| 05a_deltaapproach 1 | 3641 / 9632 | 3117 / 7173 | 1.000 | 0.001 | 57 of 8116 | 0.966 |

Where reach disagrees it is nearly always retail connecting what
generated data leaves apart (e.g. 1688 of 1716 disagreeing pairs on the
tutorial). Retail joins areas through sectors on instanced geometry and
objects, and through links across steps and ledges that do not share a
collision edge. Lowest agreement: `05b_deltatowers` BSP 1 (0.693),
`04b_floodlab` BSP 5 (0.735). Largest excess: `06b_floodzone` BSP 4
(0.398), where retail leaves large walkable areas without sectors.

## Engine invariants checked (`PathfindingData::validate`)

The retail lookup code has not been read (see below). These are the
invariants a lookup can only rely on if the data keeps them, and retail
data keeps every one on all 64 BSPs: u16 counts; refs count equals the
collision surface count; every ref and 2D child in range; 2D nodes children
first; finite planes and vertices; every sector ring closes; link vertices
and sectors in range on links that border a sector; no link with the same
sector on both sides. Generated data passes on all 64.

## What only a game run can prove

That the engine accepts generated data, and AI walks on it. To check: patch
one retail level's `sbsp` with a generated element (the BSP region would
grow, so it needs the map builder, not an in-place patch), drop the map in
`H2_MAP_DIR`, play `05a_deltaapproach` (best match: reach 0.998) and watch
Elites and Grunts move, follow, and take cover. Expect them not to jump
gaps, climb, use vehicles' paths or cross instanced-geometry bridges: the
generator has no hints and no instanced or object sectors.

## Open questions

- The engine's pathfinding code was not located in the XBE (no symbols; it
  needs a scan for the `0x14`-stride node walk or `sbsp+c4` reads). Until
  then the side rule, ring rule and invariants are V only.
- Sector and link flag meanings, and which of them AI movement needs
  (walls, ledges, corners, crouch).
- Wall and ceiling sectors: what uses them, and whether their absence
  matters.
- Instanced geometry and objects: their sectors (most retail sectors) are
  not generated. A Halo 1 level has no instanced geometry, but scenery
  that AI should walk on would need object refs.
- Hints (jump, climb, well) and user-placed hints are not decoded or
  generated.
- Merging coplanar sectors (retail does) would cut sector and link counts
  by about half; the engine's limits are unknown.
- The `+20` block's stride and the four empty blocks at `+4c`.
