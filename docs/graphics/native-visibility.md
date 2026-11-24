# Visibility in Rust

Status: Partly built. The cluster and object sets of the main game view
are computed and compared with the guest's; the guest's portal flood and
object gather still run, the sphere mode (1) and computed visible sets are
not ported, and the view records (the render camera, the projection, the
view frustum) are still read.
Code: `crates/rewrites/scene-extract/src/visibility/` (`compute`, `arm`),
the arm in `crates/rewrites/render-bridge/src/scene_step.rs`
(`visibility_arm`). Tests: `crates/rewrites/scene-extract/src/visibility/`
(`tests.rs` and each module's own).
Switch: none of its own; it runs with the scene step under
`impl.render.native`, listed in [switches.md](../runtime/switches.md).

Slice R4 of [retire-d3d.md](retire-d3d.md). For every game view the
scene step computes, beside the guest's, which BSP clusters the camera
sees and which objects the view lists, from the structure's portal graph,
the camera and the sim's object references. A differential compares the
two as sets. The view's `ClusterGather` carries the Rust sets only when
they equal the guest's; otherwise it carries the guest's, which stay the
fallback. Nothing here writes guest memory, so the sim's hashes cannot
move.

## How it works

### The graph

`visibility::graph::read` decodes the active structure
(`[0x004e0348]`) into a `PortalGraph`: per cluster its portal indices and
whether its visible set is computed at run time (flags byte `+0x80`
bit 1), per portal its two clusters, its plane (from the structure's
collision BSP plane block), its flags and its polygon, and the potentially
visible set, one bit row of `(clusters + 31) / 32` words per cluster
(`+0x58`). `GraphCache` keeps it for the map epoch, the structure's address
and its cluster and portal counts; a structure that declines for its
layout is not decoded again.

### The flood

Mode 0 is `0x00173130`, whose walk is `0x00172840`.
`visibility::flood::flood` is breadth first from the view's start cluster
(visibility struct `+8`, the location `0x00132b80` stores). A node is a
cluster seen through a window: a convex polygon on the screen, its
bounding rectangle, and the set of portals its path has crossed. The root
is the start cluster with the whole screen.

For each portal of a node's cluster, in the tag's order:

1. The portal is projected once for the view (`project`, the guest's
   `0x00173520` and `0x00173890`). The camera's signed distance from its
   plane picks how it faces. Beyond the straddle distance, a camera
   behind the plane makes it `Back` and one in front `Front`. Its polygon
   is clipped to the near plane. Wholly clipped away, it is culled; one or
   two corners left make it `Straddle`. Within the straddle distance it is
   `Straddle`: when the camera's foot on the plane is inside the polygon,
   its window is the whole screen; otherwise it is the projected
   rectangle widened, as a quad. A `Back` polygon is reversed. A portal
   whose nearest corner is beyond the far plane is culled.
2. It is skipped when culled, when the path has crossed it, or when it is
   shut (the open-portal bits at `[0x004f93a4]`, which doors clear).
3. A `Back` portal is crossed only from its first cluster and only when
   not closed from the back (flag `0x2`); a `Front` one only from its
   second and not closed from the front (flag `0x10`). A `Straddle`
   portal is crossed either way.
4. Its rectangle must overlap the node's (edges touching count). The
   portal's polygon is clipped to the node's window. Three or more corners
   left make a node in the cluster across, with the path's crossed set
   plus this portal.

A cluster is visible when a node reaches it. Each cluster's windows are
then merged as `0x00172c70` does: per cluster, ascending, its nodes newest
first, each merges into the first later one whose union with it is
smaller than the merge factor times their two areas, and that one takes
the union. The windows left are the clips objects are tested against.

Mode 2 is `0x001733e0` over `0x0016de20`: the start cluster's row of the
potentially visible set, ascending, at most 128 clusters. A start cluster
whose set is computed at run time declines.

The flood's arithmetic is its own. Screen units are the projection's
normalised device coordinates from `RenderCamera::projection`'s terms.
The near clip is the camera's near plane, and the polygon clip has no
tolerance. The three values the guest's code reads from its data are read
from the running image each frame (`guest::tuning`), never committed: the
straddle distance (`0x0045e060`, a double), the widening (`0x0045dfa0`)
and the merge factor (`0x0043ff08`).

### Objects

`visibility::objects::object_set` is `0x00132220` with its per-object
test `0x00133050`. For each visible cluster in the order reached, the
first reference partition (heads `[0x004de2e0]`, pool `[0x004de2e4]`) and
then the second (`[0x004de2d4]`, `[0x004de2d8]`) are walked. Under view
flag `0x400` the first keeps only objects of type 7 (body `+0xaa`). An
object is listed once, by its datum index. It is listed when its sphere
(body `+0x30`, `+0x3c`) reaches one of its cluster's windows, between the
near and far planes and inside the four planes through the eye and the
window's edges. In mode 2 it is always listed, and so is every object
under view flag `0x80`. A rejected object can be listed through a later
cluster. The sky cluster's objects (`0x004b9ee9`, `0x004686c4`,
`0x004b9eec` and the structure's table at `+0xb0`) are added unclipped.
View flag `0x40` lists no objects. The view's player unit
(`0x004b9ed8`) is appended when it is not listed, as `0x0003f33c` does.
`guest::object_world` reads the references and spheres into Rust-owned
maps first.

### The differential

The guest's sets are its region list (`[vis] + 0xa6e`, `0x1a` apart) and
its object list (`[vis + 0x14]`), read by `extract::cluster_gather` at
the view's base layer 3. `parity::compare` walks the two ascending sets
and counts what only the guest has and what only Rust has, clusters
before objects, naming the first. `visibility::arm` returns the gather
with the Rust sets (ascending, `source: rust`) when they are equal, and
the guest's untouched otherwise. `scene_step::visibility_arm` counts
each view and keeps the first difference since the map loaded.

## Interfaces

- Called from `render_bridge::scene_step::open_kind_at_layer` when a game
  view's base layer 3 is read, after `extract::cluster_gather`.
- Reads, all through `GuestRead`: the visibility struct `0x00547f88`
  (`+8` start cluster, `+0x14` object list, `+0x2a60` flags, `+0x2a64`
  frustum count, `+0x2a78` view kind, `+0x2a7c` mode), `[0x004c0b80]`
  (the active struct), the structure tag, the open-portal bits, the
  object and reference datum arrays, the sky globals, the view's player
  unit, the three tuning values and the render camera.
- Writes nothing.
- `render.scene` reports it
  ([agent-api.md](../runtime/agent-api.md#framescene-checks-compare-capture-scene-parity-hashes)):
  per game view `visibility_source` and `visibility_objects`, the top
  level `visibility_first_mismatch`, and the `scene_visibility_*`
  counters.

## Rules and limits

- **Sets, not orders.** The guest sorts its regions by the comparator
  `0x00134950` and lists objects in its walk order; the comparison and
  the Rust lists are ascending. A consumer that needs the guest's order
  reads it from a `guest` view.
- **The guest stays the fallback.** A view whose sets differ, or for
  which the computation declines, carries the guest's lists.
- **Declines, by name.** Memory that does not read (`unread`); counts,
  links or indices past the guest's arrays (more than 512 clusters or
  portals, more than 64 corners a portal; `layout`); the sphere mode
  (`mode`); more than one view frustum (`frusta`); a limit where the
  guest drops by its own order (256 nodes, 255 queued, the corner pools,
  128 regions, 512 windows, a full object list; `overflow`); a start
  cluster whose visible set is computed (`computed_pvs`).
- **Not ported.** The sphere mode `0x00132d60`; the computed visible set
  `0x0016e150`; the light views (`0x0054aa68`); the view frusta built from
  the windows (`0x00163db0`) and the object test against them
  (`0x00132e60`), which the window wedge stands in for; the start
  cluster's lookup from the camera's position.
- **View records.** The render camera, the projection and the start
  cluster are read from the guest as the flood's inputs. Building them in
  Rust is the "Views and cameras" row of retire-d3d.md.

## Evidence

- The flood, the projection, the facing test, the merge and the mode 2
  copy: **Code**, read in the disassembly (`h2 disasm`) at the addresses
  above.
- The open-portal bits are indexed by portal: **Code** (`0x00172927`).
- `0x004b9ed8` holding the view's player unit: **S** (occlusion-culling.md).
- Every behaviour above on invented fixtures: **Test**
  (`visibility::tests`, `flood::tests`, `objects::tests`,
  `parity::tests`, `graph::tests`).

## Not verified

- That the sets equal the guest's on real scenes: the acceptance of R4 is
  `scene_visibility_differ` at zero and `scene_visibility_rust_taken`
  equal to `scene_visibility_checked` over the gate corpus, with
  `visibility_first_mismatch` absent. Edges where the guest's frusta
  (`0x00163db0`) admit an object the window wedge does not, or the other
  way, show there by name.
- That the render camera and projection at layer 3 are the lens the
  gather used: the display copy can change the projection after the
  gather (occlusion-culling.md).
- Which maps use mode 2, how often a start cluster's set is computed, and
  whether any view has more than one frustum.
- The time per view (`scene_visibility_ns` over `scene_visibility_checked`
  plus the declines).

## Related

- [retire-d3d.md](retire-d3d.md): slice R4 and the "Visibility" row.
- [occlusion-culling.md](occlusion-culling.md): the gather's call tree,
  the object fields and the camera.
- [native-renderer.md](native-renderer.md#scene-list-builders): the Rust
  scene lists that consume the guest's regions.
- [framescene.md](framescene.md): the scene and its `cluster_gathers`.
- [collision-queries.md](../engine/collision-queries.md): the sphere flood
  over the same portal graph, for the sim.
