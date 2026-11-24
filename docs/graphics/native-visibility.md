# Visibility in Rust

Status: Built, not verified. Every game view's visible clusters and
objects (modes 0, 1 and 2), the run-time-computed visible sets, the
window frusta objects are tested against, the main view's records and
the sphere-mode and frustum-mode light views are computed in Rust and compared with the
guest's. The guest's view setup, portal flood and object gather still
run and stay the fallback.
Code: `crates/rewrites/scene-extract/src/visibility/` (`compute`, `arm`,
`view`, `light`), the arms in `crates/rewrites/render-bridge/src/scene_step.rs`
(`view_record_arm`, `visibility_arm`, `light_visibility_arm`). Tests:
`crates/rewrites/scene-extract/src/visibility/` (`tests.rs` and each
module's own).
Switch: none of its own; it runs with the scene step under
`impl.render.native`, listed in [switches.md](../runtime/switches.md).

Slice R4 of [retire-d3d.md](retire-d3d.md). For every game view the
scene step builds, beside the guest's, the view's records (its render
camera, projection, start cluster and mode), which BSP clusters the view
sees and which objects it lists; at each dynamic light's passes it does
the same for the light's view. At first-person replay it compares the
ordered light slots and their records. Each comparison counts differences
and names the first. The scene uses Rust camera records, main-view sets
and recorded light handles only on equality. Nothing here writes guest memory, so the sim's
hashes cannot move.

In single-copy play, the Rust comparison uses the horizontal extent set by
the main-view frustum hook. In exact mode, the scene step projects the portals
and builds object planes with both lenses. It removes IDs in either the
guest lists or the Rust gather at the retail lens. It tests rigid BSP
placements against their clusters' portal windows and deduplicates placements
referenced by several clusters. The native passes receive the additional
IDs through `View::display_visibility`.
[Widescreen projection](high-resolution.md#widescreen-projection).

## How it works

### The graph

`visibility::graph::read` decodes the active structure
(`[0x004e0348]`) into a `PortalGraph`: per cluster its portal indices and
whether its visible set is computed at run time (flags byte `+0x80`
bit 1); per portal its two clusters, its plane (from the structure's
collision BSP plane block), its flags, its bounding sphere (`+8`) and its
polygon; and the potentially visible set, one bit row of
`(clusters + 31) / 32` words per cluster (`+0x58`). When the set's byte
size (`+0x54`) is above one table, a second table of the same shape
follows the first, and a computed row starts from it (`0x0016e150`); a
size above one table but short of two declines (`layout`). `GraphCache`
keeps the graph for the map epoch, the structure's address and its
cluster and portal counts; a structure that declines for its layout is
not decoded again.

### The view records

`visibility::view::build` builds the main view's records the way
`render_game_views` `0x002234e0` does through `0x00223080` and
`0x0002f800`:

- The view is the record at `0x0047f210 + 0x118 · [0x004ba050]`. A
  record whose `+0` is set is a cinematic view (`cinematic`). Its `+8`
  names the local player; none, or no structure (`[0x004686c4]` is
  `0xffff`), takes `0x0002f800`'s default camera (`no_observer`). An
  active camera override (`[0x004e6948]`, byte `+0x1120` set and `+8`
  equal to 3) declines (`override`).
- The camera is the player's observer camera (`0x004e9c8c + 0x358 ·
  player`): position `+0`, forward `+0x20`, up `+0x2c`, and the field of
  view `+0x4c` times a factor. The factor is `[0x0045083c]`, or
  `[0x0045dbbc]` and `[0x0044ae88]` for a view whose shape ratio
  (`width share · 100 / height share`, in whole numbers) is 200 or 50.
  The shares come from the span in grid cells divided by the grid size,
  before pixel rounding. The near and far planes are `[0x00485ad4]` and
  `[0x00485ad8]`.
- The display flag forces one view. Otherwise the local player count at
  `[[0x004e8c20] + 8]` is clamped to one through four. The split uses
  columns on widescreen and rows on a standard screen or under the lens
  override. `0x0002f600` sizes the grid and `0x0002f640` places each
  view. With three players the first view spans two cells. For the
  render camera viewport (`+0x30`), `0x0002f6b0` divides the inner
  rectangle at `0x00485a92` into whole-pixel cells and takes the
  outside edges from the screen rectangle at `0x00485a8a`.
- The projection's scales and offsets are `0x0002fb70` with the default
  extents (`[min, one, min, one]` from `0x0045dbcc` and `0x0045dbc0`):
  `t = tan(fov · half)` (`0x0045dbbc`, the x87's `fptan`, stored once),
  the aspect `width / height`, times the anamorphic factor `0x0045dfdc`
  when the widescreen byte `0x00485ac2` is set and the override's lens
  test does not hold, and then `t` times that factor and `0x0045e108`
  under the display flag (`[0x00510c50] + 5`). The x scale is
  `one / ((xr · aspect) · t)`, the y scale `one / (yr · t)`, and the two
  offsets `0 - (e1 + e0) / xr · s` and `0 - (e2 + e3) / yr · s` (`s` from
  `0x0045dbc4`), as `0x0002fd90` stores them at `0x004b9e14 + 0x78`,
  `+0x8c`, `+0x98` and `+0x9c`.
- The start cluster is `0x0011bed0` from the camera's position: the walk
  `0x0014a280` over the collision BSP at `[0x004e033c]` from node 0 takes
  the front child when `((nz·z + ny·y) + nx·x) − d ≥ 0`, and a leaf's
  cluster is the `i16` at `[[0x004e0348] + 0x30] + 8 · leaf`. Solid is
  `0xffff`.
- The mode is `0x00132b80`'s: 2 for a view of kind 0 under view flag
  `0x80`; 1 with no frusta, or with a sphere radius under `[0x0045dbd4]`
  and no flag `0x100`; else 0.

`view::compare` compares the record with the render camera
(`0x004b9da0`), the visibility struct's start cluster (`+8`) and mode
(`+0x2a7c`) field by field, bit for bit. `scene_step::view_record_arm`
counts each view and keeps the first difference; the visibility below
takes the Rust camera when the whole record is equal, the guest's
otherwise.

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
   whose nearest corner exceeds the view frustum's `+0x88` distance is
   culled when its `+0x84` byte is set.
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
the union.

The flood's arithmetic is its own. Screen units are the projection's
normalised device coordinates from `RenderCamera::projection`'s terms.
The near clip is the camera's near plane, and the polygon clip has no
tolerance. The three values the guest's code reads from its data are read
from the running image each frame (`guest::tuning`), never committed: the
straddle distance (`0x0045e060`, a double), the widening (`0x0045dfa0`)
and the merge factor (`0x0043ff08`).

### The window frusta

`0x00172ee0` builds one frustum per window left after the merge with
`0x00163db0`, from the window's rectangle, not its polygon.
`visibility::frusta::Frustum::build` does the same from the rectangle in
tangent units (`Lens::tangent`) and the lens as the view matrix (scale
one; axes right, up and back), out to the distance `k` at view frustum `+0x88`:

- An empty rectangle (max not above min, or NaN) makes no frustum.
- The box is the six floats `[0x004687e0]` points at, grown by the eye
  and the rectangle's four corners `(x·k, y·k, −k)` taken to the world.
- The side planes are local `(c, 0, −x0)`, `(u, 0, x1)`, `(0, c, −y0)`
  and `(0, u, y1)`, each normalised by `0x00030bf0` (left as is when its
  length is under the epsilon `0x0045dbdc`, else scaled by `u / length`);
  `u` is `[0x0045dbc0]` and `c` `[0x0045dbcc]`. The eye plane is local
  `(0, 0, u)`, or, when the view frustum's byte `+0x6c` is set, its clip
  plane `+0x74` negated. The far plane is local `(0, 0, c)` at `k`.
  A local plane goes to the world as `n = (r2·z + r1·y) + r0·x`,
  `d = ((t.y·n.y + d) + t.z·n.z) + t.x·n.x`.

In mode 0 an object is listed when its sphere reaches a frustum of the
cluster it is referenced from (`0x00165010` over `0x00181800`): it is out
when wholly past a side of the box, or when
`((nz·cz + ny·cy) + nx·cx) − d > r` for a plane. NaN does not reject.
`guest::frustum_inputs` reads the image's values each frame.

### The sphere mode

Mode 1 is `0x00132d60`. A start cluster of `0xffff` lists none. A radius
(`+0x2a74`) not above `[0x0045dbd8]` lists the start cluster alone.
Otherwise `visibility::sphere::sphere_clusters` is the cluster flood
`0x0014a5b0` from the start, capacity `0x200`: depth first, through each
portal of a cluster to a cluster not yet reached, in the tag's order,
when the sphere (`+0x2a68`) straddles the portal's plane (its distance
under the radius either way), reaches the portal's bounding sphere, and
its cross-section on the plane is not wholly outside an edge of the
portal's polygon (`0x0023a160`), dropped onto the two axes the image's
projection rows (`0x00440b94`, read each frame) pick for the normal's
largest component. The plane distance and the cross-section radius are
taken in double precision, the rest in single, in the guest's order.

An object is listed in mode 1 when `(r + R)² ≥ d²`, `d` from the view's
centre to the object's, `R` the view's radius (`0x00132e60`).

### Computed visible sets

Mode 2 is `0x001733e0` over `0x0016de20`: the start cluster's row,
ascending, at most 128 clusters. For a cluster whose visible set is
computed at run time, `visibility::pvs::row` computes it as `0x0016de20`
does. The row starts from the cluster's row of the second table. Each
pass walks the clusters the row holds, ascending (`0x00044690`), and each
of their portals not yet looked at. A portal with flag `0x4` that is open
adds the union of its two clusters' second-table rows, masked by the
start cluster's row of the first table. Each portal is looked at once. A
pass that adds nothing ends it. The result is the smallest row closed
under that rule, whatever the walk's order.

At the mode-2 producer (`0x001733e0`, called by `0x00132152`),
`render_bridge::native_scene_lists::regions` uses the caller's start
argument. The sentinel selects an empty list even when the view has a
resolved start cluster. On the first call for a start, the original
producer runs. Rust compares the ordered region IDs, the sorted ordinal
table, both counts and the link from each region to its window. On a
match, Rust owns those records on subsequent calls with the same start,
computed row and open-portal bits. A BSP activation or reload clears
the comparison. Each window's complete camera-dependent payload still
comes from the guest frustum builder at `0x00163db0`.
`native_scene_lists_pvs_regions_` counters report comparisons,
mismatches, native calls and fallbacks.

### Objects

`visibility::objects::object_set` is `0x00132220` with its per-object
test `0x00133050`. For each visible cluster in the order reached, the
first reference partition (heads `[0x004de2e0]`, pool `[0x004de2e4]`) and
then the second (`[0x004de2d4]`, `[0x004de2d8]`) are walked. Under view
flag `0x400` the first keeps only objects of type 7 (body `+0xaa`). An
object is listed once, by its datum index. It is listed when the mode's
clip admits its sphere (body `+0x30`, `+0x3c` for the main view; `+0x50`,
`+0x5c` for a view of another kind): the window frusta in mode 0, the
view's sphere in mode 1, always in mode 2, and always under view flag
`0x80`. A rejected object can be listed through a later cluster. The sky
cluster's objects (`0x004b9ee9`, `0x004686c4`, `0x004b9eec` and the
structure's table at `+0xb0`) are added unclipped. View flag `0x40`
lists no objects. The view's player unit (`0x004b9ed8`) is appended when
it is not listed, as `0x0003f33c` does. `guest::object_world` reads the
references and spheres into Rust-owned maps first.

### The light views

The light frame (`0x00031c80`) runs the gather `0x001320f0` with EAX 1
for each drawn light, so one struct, `0x0054aa68`, holds the light being
drawn. `0x00132b80` fills it with kind 1, the flags
`(definition flags >> 12) & 0x400`, the frusta `0x000c1d90` builds (six
cube faces for a volume of kind 0, else one projective frustum), the
light's cluster (or the one its centre is in), its centre and its
radius. A light never takes mode 2: no frusta, or a radius under
`[0x0045dbd4]`, is mode 1; otherwise mode 0.

In mode 0, `visibility::light_frusta` derives the six cube lenses or
the projected lens from the pose and volume passed to the light's
draw. Each lens floods from the light's cluster. The region list is
the union across faces, and a sphere can reach any window for its
cluster. A projected light with an unusable tangent rectangle declines.
The source-window intersection at `0x00132900` writes overlap records
at visibility struct `+0x2a98`. The set comparison covers region IDs
and object handles, which the gather walks from the light's own regions.

`visibility::light::compute` takes mode 1: the sphere mode's clusters,
the objects whose second sphere reaches the light's, no sky cluster and
no player unit. Under flag `0x400` step 13 (`0x00031c20`) appends the
owner's first ancestor of type 0, or with no parent, when the list has
room, and so does the Rust set. `scene_step::light_visibility_arm` runs
at the start of each light's passes (`scene_step::light`), after that
light's gather and before the next light's, and compares the sets with
the struct's region list and object list.

### First-person lights

The loop at `0x000404b0` replays each nonempty slot in order. It has
no BSP traversal of its own. At the dynamic passes' constants call
(`0x000311c8` return site), the source record tail includes the
attachments that the item walk processes. Rust assigns one slot per
dynamic light in order, copies each bit-10 record, and stops copying
when the slot fills. Lights past the slot cap contribute records to the
last slot. At first-person entry, `first_person::slots` compares the
number of slots, light handles and complete record sequences with the
guest table. The E3 light choice uses the Rust handles when equal and
the guest handles otherwise. The replay and its mode filter are described
in [native-light-gather.md](native-light-gather.md#the-first-person-lights-e-0x000404b0-0x000402b0-0x0003fd70).

### The differential

The guest's sets are its region list (`[vis] + 0xa6e`, `0x1a` apart) and
its object list (`[vis + 0x14]`), read by `extract::cluster_gather` at
the view's base layer 3 (the main view) or `light::guest_lists` (a
light). `parity::compare` walks the two ascending sets and counts what
only the guest has and what only Rust has, clusters before objects,
naming the first. `visibility::arm` returns the gather with the Rust sets
(ascending, `source: rust`) when they are equal, and the guest's
untouched otherwise. `scene_step::visibility_arm` counts each view and
keeps the first difference since the map loaded. A light's comparison is
counted only: no light list is carried into the scene.

## Interfaces

- Called from `render_bridge::scene_step::open_kind_at_layer` when a game
  view's base layer 3 is read, after `extract::cluster_gather`
  (`view_record_arm`, then `visibility_arm`), and from
  `scene_step::light` at each dynamic light's passes. First-person slot
  collection runs at the dynamic constants call and compares at replay.
- Reads, all through `GuestRead`: the visibility structs `0x00547f88` and
  `0x0054aa68` (`+8` start cluster, `+0x14` object list, `+0x2a60`
  flags, `+0x2a64` frustum count, `+0x2a68` and `+0x2a74` the sphere,
  `+0x2a78` view kind, `+0x2a7c` mode), `[0x004c0b80]` (the active
  struct), the structure tag and its collision BSP, the open-portal bits,
  the object and reference datum arrays, the light records, the sky
  globals, the view's player unit, the view records, the observer
  cameras, the render camera, and the image's values named above.
- Writes nothing.
- `render.scene` reports it
  ([agent-api.md](../runtime/agent-api.md#framescene-checks-compare-capture-scene-parity-hashes)):
  per game view `visibility_source` and `visibility_objects`; the top
  level `visibility_first_mismatch`, `light_visibility_first_mismatch`,
  `first_person_visibility_first_mismatch` and `view_record_first_mismatch`;
  and the `scene_visibility_*`, `scene_light_visibility_*`,
  `scene_first_person_visibility_*` and `scene_view_record_*` counters.

## Rules and limits

- **Sets, not orders.** The guest sorts its regions by the comparator
  `0x00134950` and lists objects in its walk order; the comparison and
  the Rust lists are ascending. A consumer that needs the guest's order
  reads it from a `guest` view.
- **The guest stays the fallback.** A view whose sets or records differ,
  or for which the computation declines, keeps the guest's.
- **Declines, by name.** For the sets: memory that does not read
  (`unread`); counts, links or indices past the guest's arrays (more
  than 512 clusters or portals, more than 64 corners a portal, a short
  second table, projection rows past 2; `layout`); an unknown mode (`mode`);
  an unsupported frustum count (`frusta`); a limit
  where the guest drops by its own order (256 nodes, 255 queued, the
  corner pools, 128 regions, 512 windows, a full object list; `overflow`);
  a camera that makes no lens (`camera`); a struct other than the one the
  arm is for (`not_main`, also for a light). For the records: `unread`,
  `layout`, `cinematic`, `no_observer`, `override`.
- **Not ported.** The overlap records for light windows against source-view
  windows (`0x00132900`); cinematic and override cameras; off-centre extents passed to
  `0x0002fb70`.
- **Own arithmetic.** The flood, the window rectangles in tangent units,
  and the sphere flood's projection normal (the
  structure's own BSP plane, where the guest reads the active BSP's)
  are Rust's; a difference they make shows by name.

## Evidence

- The flood, the projection, the facing test, the merge, the mode 2
  copy, the computed row, the window frusta, the sphere mode, the view
  setup and the camera and lens builders: **Code**, read in the
  disassembly (`h2 disasm`) at the addresses above.
- The open-portal bits are indexed by portal: **Code** (`0x00172927`).
- `0x004b9ed8` holding the view's player unit: **S** (occlusion-culling.md).
- That the item lists `0x00040e90` leave the light struct's lists alone
  between a light's gather and its passes: **S**.
- Every behaviour above on invented fixtures: **Test**
  (`visibility::tests`, and the `flood`, `frusta`, `sphere`, `pvs`,
  `objects`, `view`, `light`, `parity` and `graph` tests).

## Not verified

- That the sets and records equal the guest's on real scenes: the
  acceptance of R4 is `scene_visibility_differ`,
  `scene_light_visibility_differ`, `scene_first_person_visibility_differ`
  and `scene_view_record_differ` at
  zero, and `scene_visibility_rust_taken` equal to
  `scene_visibility_checked`, over the gate corpus, with all four
  `*_first_mismatch` fields absent.
- That the render camera and projection at layer 3 are the lens the
  gather used: the display copy can change the projection after the
  gather (occlusion-culling.md).
- Which maps use modes 1 and 2, how often a start cluster's set is
  computed, how many lights take mode 0, and whether any view has more
  than one frustum.
- The time per view (`scene_visibility_ns` over `scene_visibility_checked`
  plus the declines) and per light (`scene_light_visibility_ns`).

## Related

- [retire-d3d.md](retire-d3d.md): slice R4 and the "Visibility" and
  "Views and cameras" rows.
- [occlusion-culling.md](occlusion-culling.md): the gather's call tree,
  the object fields and the camera.
- [render-loop.md](render-loop.md): `render_game_views` and the view
  records.
- [native-light-gather.md](native-light-gather.md): the light frame whose
  step 11 fills the light's struct.
- [native-renderer.md](native-renderer.md#scene-list-builders): the Rust
  scene lists that consume the guest's regions.
- [framescene.md](framescene.md): the scene and its `cluster_gathers`.
- [collision-queries.md](../engine/collision-queries.md): the sphere flood
  (entry G) and the location (entry B) over the same structure, for the
  sim.
