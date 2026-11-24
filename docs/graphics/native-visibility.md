# Visibility in Rust

Status: Built, not verified. Every game view's visible clusters and
objects (modes 0, 1 and 2), the run-time-computed visible sets, the
window frusta objects are tested against, the main view's records and
the sphere-mode light views are computed in Rust and compared with the
guest's. The guest's view setup, portal flood and object gather still
run and stay the fallback; a light's own frusta (mode 0) are not built.
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
the same for the light's view. Three differentials compare them with
what the guest wrote. Each counts, names its first difference, and hands
on the Rust result only when it equals the guest's; otherwise the
guest's stays in use. Nothing here writes guest memory, so the sim's
hashes cannot move.

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
  The near and far planes are `[0x00485ad4]` and `[0x00485ad8]`. The
  viewport is read from the record (`+0xc + 0x30`); the shares are the
  viewport's against the screen rectangle `0x00485a8a`.
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
one; axes right, up and back), out to the far distance `k`:

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

`visibility::light::compute` takes mode 1: the sphere mode's clusters,
the objects whose second sphere reaches the light's, no sky cluster and
no player unit. Under flag `0x400` step 13 (`0x00031c20`) appends the
owner's first ancestor of type 0, or with no parent, when the list has
room, and so does the Rust set. `scene_step::light_visibility_arm` runs
at the start of each light's passes (`scene_step::light`), after that
light's gather and before the next light's, and compares the sets with
the struct's region list and object list.

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
  `scene_step::light` at each dynamic light's passes.
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
  level `visibility_first_mismatch`, `light_visibility_first_mismatch`
  and `view_record_first_mismatch`; and the `scene_visibility_*`,
  `scene_light_visibility_*` and `scene_view_record_*` counters.

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
  second table, projection rows past 2; `layout`); an unknown mode, or a
  light's mode 0 (`mode`); more than one view frustum (`frusta`); a limit
  where the guest drops by its own order (256 nodes, 255 queued, the
  corner pools, 128 regions, 512 windows, a full object list; `overflow`);
  a camera that makes no lens (`camera`); a struct other than the one the
  arm is for (`not_main`, also for a light). For the records: `unread`,
  `layout`, `cinematic`, `no_observer`, `override`.
- **Not ported.** A light's own frusta (mode 0, `0x000c1d90` and the
  restriction `0x00132900` to the source view's clusters); the
  first-person lights (`0x000404b0`); the view layout that places each
  viewport (`0x0002f600`, `0x0002f640`, `0x0002f6b0`), whose viewport is
  read; cinematic and override cameras; off-centre extents passed to
  `0x0002fb70`.
- **Own arithmetic.** The flood, the window rectangles in tangent units,
  the frusta's far distance (the camera's far plane, not the view
  frustum's `+0x88`), the sphere flood's projection normal (the
  structure's own BSP plane, where the guest reads the active BSP's) and
  the field-of-view shares (the viewport's, where the guest uses its view
  grid) are Rust's; a difference they make shows by name.

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
  `scene_light_visibility_differ` and `scene_view_record_differ` at
  zero, and `scene_visibility_rust_taken` equal to
  `scene_visibility_checked`, over the gate corpus, with the three
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
