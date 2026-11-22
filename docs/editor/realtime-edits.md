# Realtime edits: geometry, placements, collision and textures in the running game

Status: In progress. Slices R1 (the raw overlay in both readers, `raw` and `evict` in `edit.batch`, `edit.raw.append`, `edit.stage.put`, `edit.overlay` and their journal), R2 (the native refresh) and R3 (placement transactions, `world.placement.edit`) are built; R4 to R7 are design, and what they build on is cited as **Code**.
Code: built in `crates/runtime/vfs/src/map_override/overlay.rs`, `crates/runtime/vfs/src/lib.rs` (`Vfs::read`), `crates/platform/assets/src/resource.rs`, `crates/extensions/game/src/native_map.rs`, `crates/extensions/hot-tags/src/raw.rs`, `crates/extensions/agent/src/hot.rs`, `crates/extensions/game/src/hot_glue.rs`, `crates/rewrites/render-bridge/src/native_edits.rs`, `crates/rewrites/native-render`, `crates/extensions/agent/src/sim/edits.rs`, `crates/extensions/agent/src/sim/calls/placement/edit.rs` and `crates/extensions/agent-proto/src/sim/edits.rs`; planned in `crates/apps/studio/src/play`. Tests: named per slice under "Build slices".
Switch: none. With no editor paired and no edit sent, none of it runs.

A paired editor's Modeling, Scenario and Textures edits reach the running
game without a map load: moved and grown render geometry, added and removed
placements, rebuilt collision and painted pixels. This page says, for each,
where the game holds the data, which write makes the change visible, and
where a live path does not exist and the play copy reload
([live-mirror.md](live-mirror.md), "Reload") stays the fallback. It extends
the tag and object edits of live-mirror.md and sits between the editor's
save lists ([modeling-targets.md](modeling-targets.md), "The session and
the save") and the game's memory. The rule that must hold: **a live edit is
the same bytes the save would write, applied at one tick boundary through a
named agent method that `SessionKind::writes` admits**, so reloading the
saved map shows what the live game showed.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## How it works

### Who holds what

Every edited thing has up to three holders, and a live edit has to reach
each one that a frame or a tick reads.

| Data | The file | Guest memory | Host copies |
|---|---|---|---|
| Tag fields (bounds, counts, block info, placements, collision blocks) | metadata | resident for the whole map at the addresses its pointers use; block references are absolute `(count, address)` pairs | none, except those below |
| Section vertices and indices (`sbsp` clusters, instanced definitions, `mode` sections) | a raw block per section | the guest's geometry cache: paged in by the resource lookup, evictable, found through the block info's loaded flag and handle ([structure-bsp.md](../formats/structure-bsp.md), "Geometry block info") | the D3D device's `VertexCache` and `IndexCache`; `native_render::gpu::GpuWorld` and `Ready::part_bounds`; `LitObjects::models` |
| Bitmap pixels | three LOD chains of raw data per image | the guest's texture cache, evictable | the D3D device's `CachedTexture`s; `native_render::stage_images::StageImages` and the GPU textures made from it |
| Collision | the eight blocks at `sbsp+0x14`, the leaf table, the MOPP; the same blocks per `coll` permutation | read in place by the lifted collision code and by Havok shapes that point into the tag | `sim_collision::CollisionWorld` (lazy, write-watched), the debug overlay's cache, the bots' `los::Walls` |
| Placed objects | scenario placement blocks | the object pool; each object keeps its placement index | the editor's stream (`live_mirror::Store`) |

Two facts decide the design (**Code**):

- **The host copies come in two kinds.** The D3D device keys its copies on
  guest address ranges and re-reads a range whose pages were written
  (`d3d::dirty::WriteTracker::dirty_since`, fed by `Guest::write_bytes`
  through `d3d_glue::install_write_barrier`). The native renderer's copies
  are built from the **map file** through the VFS, once per map and BSP
  (`native_world_glue::Glue::load_bsp`, `native_render::load::load_world`,
  `native_objects_glue`, `StageDecoder::decode`), and have no invalidation
  but a whole reload (`native_map_work::reset`, the `BSP_SWITCH` observer).
  A guest write alone therefore updates the D3D path and the sorted BSP
  path that reads live section bytes
  (`Device::prepare_section_vertices`), and leaves every native copy
  stale.
- **Raw data in guest memory is a cache of the file.** A section or a
  texture the guest evicts is read again from the map file, so a write
  only to the resident copy is lost at the next eviction.

### The edit overlay

The host keeps one **raw overlay** per map: replacement runs and
appended extents past the end of each file, by the file's lowercase name
(the loaded map's cache header name, `mainmenu`, `shared`,
`single_player_shared`). It is `vfs::RawOverlay::global`
(`crates/runtime/vfs/src/map_override/overlay.rs`). Both readers of a
map's raw data serve through it (**Code**):

- the guest, which reads raw data from the **cache slot file** the map
  was copied into (`cache%03d.map` on Z: for slots 0 and 1, on N: for 2
  to 5; [cache-slots.md](../runtime/cache-slots.md), "The slot files"),
  not from the map itself. `Vfs::read` (`crates/runtime/vfs/src/lib.rs`)
  applies the overlay to an object the hot-edit glue aliased to a map
  (`RawOverlay::alias`), past the file's end too, and `Vfs::try_map_read`
  refuses to map an aliased file so its reads take that path. The glue
  aliases each selector's slot file before every raw act
  (`hot_glue::RawGlue::ready`): the current slot for the loaded map,
  slots 0, 1 and 2 for the shared maps;
- the native loaders, whose live opens (`native_map::open`) give each
  selector of `assets::ResourceFiles` a `ResourceOverlay` over the same
  overlay, read at each read and extending the file's length over its
  appended extents.

The overlay is never applied to a slot copy's source: an overlay served
on `maps\<name>.map` would be written into the slot file at the next copy
and outlive the session, since the game reuses a slot by its path alone.
The private generation of a play copy, a slot file and a sim batch's
cache template all stay free of overlay bytes. The overlay is dropped at
the first tick of a new map (the `edit.overlay` ticker in
`crates/extensions/agent/src/hot.rs`) and when a play-copy reload binds
(`ReservedMapRebind::commit`). With the overlay in place a re-paged
section, a re-decoded texture and a native reload all read the edited
bytes, so the file view, the guest cache and the host copies cannot
disagree for longer than one refresh.

### Four write primitives

The editor's save is four lists (`scenario_edit::Changes`,
`GeometryChanges`, `RawPatch`). Each has one live primitive, and one
extended `edit.batch` carries any mix of them to a single tick boundary
under one journal id.

| Save list | Live primitive | What the host does at the boundary |
|---|---|---|
| tag patches (`patches`, `bsp_patches`) | `writes` of `edit.batch` (built) | `hot_tags::Editor::batch`: `was` check, `Guest::write_bytes`, journal |
| `raw_patches` | `raw` | resolves the owner's raw pointer from the guest's own tag bytes; checks `was` against the overlay view; adds the overlay entry; writes the resident copy if the owner is resident; queues a native refresh |
| `raw_new` (grown raw) | `edit.raw.append`, then `writes` and `evict` | the append reserves an extent past the file's end and is invisible until a tag points at it; the batch writes the block info and counts and evicts the owner, so the guest pages the new block in from the overlay |
| grown tag blocks (`grown`, `tag_grown`) | `moves` | copies the new block into the **edit arena**, then writes the root's `(count, address)`; later writes inside the block are ordinary `writes` |

**The resident copy.** A section's cache buffer is the file block with its
struct's pointers fixed up (the resource base is the buffer plus 8 plus
the struct size, structure-bsp.md), so a raw-block offset maps to
`buffer + offset` (**S**). The host does not trust that: it compares the
resident bytes with `was` before writing and answers `MISMATCH` otherwise.
The resident buffer is found through the block info's loaded flag
(`+0x1e`) and its runtime section block (at the record plus the `i16` at
`+0x1c`), whose pointer names the section struct 8 bytes into the buffer
(`hot_tags::raw::resident_base`). A texture's resident pixels would be
found from the image record's cache handles (`tags` schema `BitmapImage`,
the three words after the LOD sizes) through the guest texture cache's
handle table; that read is not done (**S**, the handle check of
`texture_cache_initialize`'s consumers), so a `pixels` owner takes the
overlay entry alone and reports `resident: false`. An owner that is not
resident takes only the overlay entry, which is enough: the next page-in
reads it.

**Eviction** clears the section's loaded flag, sets its cache handle to
`0xffffffff` and zeroes its runtime section block, so the guest's next
resource lookup misses and reads the block from the file view
(`hot_tags::raw::evict_section`). The texture cache's own entry for the
old buffer is left to its eviction (**S**, R5's grown check).

**The edit arena.** Guest memory for blocks that outgrow their place. It is
a range of the HLE reservation (`kernel::memory::HLE_RESERVED`) reserved
with `Builder::guest_alloc` when the hot-edit feature installs
(`crates/extensions/game/src/hot_glue.rs`), managed by a first-fit extent
allocator in `hot-tags`, and released whole at a map load. The engine
follows any mapped pointer, so a block there reads as one in the metadata
does (**S**, the same supposition as modeling-targets.md's "block pointer
into ordinary metadata"). Its size is bounded by the free physical pages
the guest's own arena leaves (`Guest::find_free_physical`), which is not
measured; `edit.arena` reports it. An arena extent a move made belongs to
the moved block's tag for `edit.batch`'s "inside one loaded tag" rule and
for `hot_tags::path::locate`.

**The native refresh.** `render_bridge::native_edits` is a queue with one
share per drainer: the world's glue drains at the frame boundary once its
world is resident, the objects' glue at the next walk once its models
are; each applies a drain on a worker, one at a time, under the lock of
the GPU objects the frame's passes use. The hot-edit glue pushes one refresh per owner
after a batch or a revert applies (`hot_glue::native_edit`). A new map
load drops what is queued:

| Refresh | Does |
|---|---|
| `Section` same counts | decodes the section from the overlaid file as the load does (`native_render::load::load_sections`; a definition once per rigid placement), calls `GpuWorld::replace_section` (a `queue.write_buffer` of the section's run of both vertex buffers; the buffers carry `COPY_DST`), and recomputes the draw and world bounds and `Ready::part_bounds` (**Code**, `native_world_glue::refresh_world`) |
| `Section` with changed counts, or any refusal above | `native_map_work::reload_bsp`: the world reloads on its worker from the overlaid file and is published when ready; until then `BspKey::current` fails and the parts keep their D3D draws from guest memory (**Code**) |
| `Model { tag }` | decodes the `mode` from the overlaid file as the load does, uploads it into a staging set and moves it into `LitObjects` under the draw set's lock with its new coverage; a model that no longer loads natively leaves the native path and draws from guest memory; covers moved and grown sections alike. `GpuModels::replace_model` does the same for the editor's and Theater's models (**Code**, `native_objects_glue::refresh_models`) |
| `Image { bitmap, image }` | re-decodes the image at each stage that samples it (`stage_images::decode_stages`), and `queue.write_texture`s every level of every material program texture made from it, each signed variant biased (`GpuWorld::rewrite_image`, `LitObjects::rewrite_image`); a changed size, level count or format is refused with a log line, all or nothing (**Code**) |

### Geometry

- **Moved vertices, same counts** (the in-place class). One batch:
  `raw` for the changed stream bytes, `writes` for part spheres and boxes,
  the cluster bounds and a `mode`'s compression box. The D3D path and the
  sorted BSP path redraw from the written pages on the next frame; the
  native world and objects redraw after the refresh, the same frame or the
  next. The guest's visibility walk reads the cluster bounds and placement
  spheres from the tag (`engine::render_loop::world_walk`), which the same
  batch wrote. The editor sends a drag at its push interval (one batch per
  200 ms, `play/sync.rs`) and the released state last. **Shows: next
  frame.**
- **A compression box that grew** requantises every section of the model:
  one batch with a `raw` entry per section. Same path, larger batch.
- **Changed counts** (the grown-raw class: extrude, delete, loop cut). The
  editor encodes the section (`tags::geometry_write::encode_section`),
  appends it, then sends the block info, section info counts, resource
  entries and bounds as `writes` with `evict` naming the section. The
  guest's next resource lookup for it misses and reads the new block; the
  native side reloads the world, or replaces the model. **Shows: within a
  few frames**; the section is absent from the guest's draws between the
  evict and the page-in, and the native world keeps its old copy until the
  reload publishes. Limits are modeling-targets.md's: a section a lightmap
  bucket reaches keeps its vertex count and order and is refused; a sky's
  strips are refused.
- **A tag block that must grow** (a new material at `sbsp+0xa4`): a
  `moves` entry. The native world reloads.
- **Lightmaps** stay as they were, stale on the edited faces, as in the
  save.
- **Not live:** a new section, cluster, definition, region or permutation,
  and a new tag. Fallback: reload.

### Placements

`world.placement.edit` is one transaction on one scenario block, computed
by the editor from its document diff, so add, remove and their undos are
the same call.

At the tick boundary, in this order: check `was_count` and each record's
`was`; delete the live occupants of `delete`; rewrite the placement index
(`guest_layout::object::Object::PLACEMENT_INDEX`) of the occupants named
by `relink`; write the records, the object-name repoints and the count;
create an object for each index in `create`.

- **Add.** The record goes in slot `count` and the count rises. A block
  has room while its count is under its capacity: its loaded count, which
  the host records at the block's first edit of a map activation
  (`placement::edit::BlockState`), or the capacity of the arena block it
  moved to. With no room the host will move the block into the arena with
  twice the capacity, bounded by the per-type limit the editor enforces
  ([scenario-editor.md](scenario-editor.md) section 6, "Add"), and refuses
  past it. Until the arena (slice R4) exists, no room is a refusal with
  `data.need` and `data.capacity`.
- **Remove** is the save's swap-remove: the last record is written over
  the removed slot, the count drops, the removed entry's objects are
  deleted and the moved entry's objects are relinked to their new index,
  so the engine's re-create pass on a BSP switch still finds each object
  under its own placement.
- **Creation is linked.** The object is made by the engine's
  placement-data and `OBJECT_NEW` sequence that `world.object.respawn`
  uses (`crates/extensions/agent/src/sim/calls.rs`, `prepare_spawn`), with
  the placement index written into the placement data before the call,
  from where `object_new` copies it into the object (**Code**:
  `guest_layout::object`, the placement-index evidence in
  [live-telemetry.md](../live/live-telemetry.md)). A linked object is not
  a preview: it is what a load would make, so it does not block snapshots
  the way `Previews` do.
- **What a created object honours.** The fields `placement::read` reads:
  palette entry, position and rotation. A named record, nonzero placement
  flags, a scale and type-specific fields are written to the block and the
  entry is reported `created: false` with the reason; that entry shows
  after a reload. Bipeds, light fixtures and sound scenery are the same
  until `placement::BLOCKS` covers them. Calling the engine's own
  create-from-scenario-record entry removes these limits and stays a
  research point (**S**: its register and stack convention; the by-name
  caller is `object_create` at `0x000bb670`, which hands the record to the
  builder `0x000d5060`, per live-telemetry's `BODY_PLACEMENT_INDEX`).
- **Blocks the engine walks itself.** Starting locations, netgame
  equipment and starting equipment are walked from zero to the root count
  on each use (**E**, scenario-editor.md section 6), so the record and the
  count are the whole edit and `create` is empty: a new spawn point is
  used at the next spawn, a new netgame equipment entry is spawned by the
  engine's own per-tick spawner, within its 100 entries. Netgame flags are
  walked too, but their results are cached per game (**E**, same
  section): **shows next game**.
- **Not live:** trigger volumes, kill triggers, AI squads and device
  groups (their readers are not disassembled; **S**), a palette entry for
  a tag the map does not hold, and a new object name. They are written to
  the block where it has room and marked After reload.

### Collision

The lifted game reads collision in place by default
(`impl.sim.native.collision` is off, `crates/hooking/hooks/src/switches/table.rs`),
so live collision is a pointer swap in guest memory, staged off the tick
and committed on it.

1. **Stage.** The editor compiles with `collision-build`
   (modeling-targets.md, "Structure collision") and uploads the eight
   blocks, the per-leaf table and the MOPP code. The host writes them into
   new arena extents. Nothing points at them, so this is safe at any
   boundary and in several requests.
2. **Commit**, at one tick boundary: write the eight `(count, address)`
   pairs, the leaf table's pair and the MOPP's pair and bounds; then run
   the activation the request names.
3. **Free** the extents the old pairs named, if they were arena extents,
   after the next activation.

Two readers see a commit at different times. The lifted BSP walks (ray
and sweep tests, point location) follow the pairs on every query, so they
answer from the new blocks on the next tick. Havok's structure shape is
built at a BSP activation over the collision BSP and the MOPP code (**E**,
[multiplayer.md](../halo1/multiplayer.md), "Blocker: the structure's Havok
MOPP (`sbsp+0x1e0`)"), so Havok bodies collide with the old shape until
the next activation, and the old extents stay allocated until then.

| Mode | When | Activation | Shows |
|---|---|---|---|
| `append` | solids added by `scenery_collision::merge`: every existing plane, surface and leaf keeps its index | none; or, with `activate: true`, the switch below | rays next tick; Havok bodies after an activation |
| `rebuild` | surfaces moved or deleted: indices change | always: a BSP switch to the active BSP through `BSP_SWITCH` (`scenario_switch_structure_bsp`), which rebuilds the Havok structure, re-creates the placed objects and fires the Rust observers | after a hitch of one BSP switch; players and their state stay, as across a campaign switch |

The editor sends `append` with `activate: true` on every commit, dragging
included, so Havok bodies always match the collision the rays see. Each
commit costs one BSP-switch hitch; the editor commits once per drag step
it sends, not per mouse event.

What follows the commit without more work (**Code**):
`sim_collision`'s page watches over the header, the eight blocks and the
leaf table start a new generation when a block is repointed (test
`a_warm_point_walk_observes_aliased_edits_and_relocated_planes`,
`crates/rewrites/engine/src/sim_collision/sources.rs`); the `BSP_SWITCH`
observers invalidate the collision world and reload the native world.
What the commit must do itself: drop the debug collision overlay's cache
(`debug_static.rs`), and mark the bots' `los::Walls` stale, which are built
offline per map and BSP ([bots.md](../game/bots.md)).

What holds an index across ticks and so decides `append` against
`rebuild`: each object's location (leaf, cluster, BSP;
`guest_layout::object::ObjectHeader::LOCATION`), Havok's cached MOPP keys
that name surfaces ([havok-runtime.md](../engine/havok-runtime.md)), and
pathfinding's per-surface references (`tags::pathfinding`). `append` keeps
them all valid. `rebuild` relies on the switch to refresh the first two;
pathfinding is regenerated by the editor (`pathfind::generate`) and moved
in the same commit, and only for converted maps, as modeling-targets.md
step 3 limits the save.

- **Collision models (`coll`).** The same stage and commit on one
  permutation's BSP. Ray and sweep tests read the tag each query, so
  bullets hit the new shape on the next tick. BSP-physics slots hold
  pointers and vtables the load fixup wrote ([collision.md](../formats/collision.md)),
  and a live body's Havok shape points at the old blocks, so the editor
  keeps the old extents alive and respawns the objects of that definition
  (`world.object.respawn`); the old extents are freed when no object made
  before the commit remains. **Shows: bullets next tick, bodies on
  respawn.**
- **Physics models (`phmo`).** Primitive sizes are in-place `writes`.
  Effect `next_spawn` with Respawn now: an existing rigid body keeps the
  shape it was built with (**S**).
- **Not live:** portals, clusters and visibility (not editable at all),
  retail-map pathfinding, and a `rebuild` on a retail map until the disc
  survey of index holders that modeling-targets.md asks for.
- **Design, later:** when `sim_collision` answers every query
  ([sim-rewrite-collision.md](../engine/sim-rewrite-collision.md)), the
  swap becomes a replaced `Structure` in `CollisionWorld` with no arena
  and no pointer writes. The agent method keeps its shape.

### Textures

A stroke changes stored bytes of an image's LOD chains. The editor encodes
the touched levels in the image's own format (`assets` encoder,
[modeling.md](modeling.md), "Texture editing") and sends, per LOD chain
that contains the level, a `raw` entry with owner `pixels`.

- The overlay takes it; the resident copy, if any, is written through
  `Guest::write_bytes`, and `Device::resolve_texture` re-decodes the
  texture at the next draw that samples it because its pages are dirty
  (**Code**: `CachedTexture`, `dirty_since`). That covers every D3D draw:
  HUD, effects, decals, first person.
- The `Image` refresh rewrites the native renderer's texture for the
  world and objects.
- During a stroke the editor sends mip 0's changed bytes at the push
  interval, and the rebuilt mips at the stroke's end. A request line is at
  most 1 MiB (`MAX_LINE`, `crates/extensions/agent/src/server.rs`); a
  level larger than that goes through `edit.stage.put`.
- A `P8` lightmap page is the same path with palette-quantised bytes.

**Shows: next frame, same size and format.** A new size, format or mip
count changes the image record and what the guest's texture cache
allocated for it: After reload. Evicting and re-paging a resized texture
is not designed here because the cache's behaviour with a changed record
is not known (modeling.md, "Not verified").

### The tick boundary and the session

Every primitive above runs where `edit.batch` and `world.object.*` run:
on the game thread before `game_tick`, drained by `CallsFeature` from
`calls::Queue` (`crates/extensions/agent/src/sim/calls.rs`). A new method
calls `Sim::host_write` or `hot::permit` first, then `calls::queue_edit`
with an `EditCheck` that re-checks the epoch, the map identity, the
session and the peer count on the game thread. Staging
(`edit.stage.put`, `edit.raw.append`, collision stage) touches no byte the
game reads and needs only the epoch.

The batch is all or nothing: every `was` is checked before the first
write. The journal entry records tag bytes, overlay entries, arena moves
and evictions, and `edit.revert` undoes them in reverse: old tag bytes,
overlay entries removed with the resident copy restored and the refresh
queued, `(count, address)` pairs put back. `world.placement.edit` and a
collision commit are not reverted by the journal, since objects were
made and deleted: the editor undoes them with the inverse call, and "Put
the game back as it loaded" with either outstanding is a reload.

## Interfaces

### The game's agent API

Wire types in `agent_proto::hot` and `agent_proto::sim::edits`, listed by
`session.methods`, documented in
[agent-api.md](../runtime/agent-api.md) when built. Every act method takes
`epoch` and `confirm_peers` as `edit.batch` does. Bytes are base64, or
`{stage, at, len}` naming staged bytes; `was` is the old bytes or
`{hash}`, their FNV-1a 64.

| Method | Access | Params | Result |
|---|---|---|---|
| `edit.batch` (extended) | act | adds `raw: [{owner, offset, bytes, was?}]`, `moves: [{root: {tag, path} \| {at}, stride, count, capacity?, bytes, was: {count, address}}]`, `evict: [owner]` | adds `raw: [{owner, resident, refresh}]`, `moves: [{address, capacity}]`, `evicted`; `MISMATCH` lists raw entries by `owner` and `offset` |
| `edit.raw.append` | act | `{file?, bytes, align?}` | `{id, pointer, len}`: a raw pointer for tag writes; journalled, dropped by revert |
| `edit.stage.put` | act | `{stage?, bytes}` | `{stage, len}`: appends to a host buffer that dies with the connection or its first use |
| `edit.arena` | read | none | `{base, len, used, largest_free, moves: [{tag, root, address, capacity}]}` |
| `edit.overlay` | read | none | `{entries, bytes, appended, owners: [owner]}` |
| `world.placement.edit` | act | `{block, was_count, count, records: [{index, bytes, was?}], names?: [{name, placement}], delete?: [index], relink?: [{from, to}], create?: [index]}` | `{at, count, capacity, moved?: {address, capacity}, deleted: [id], relinked: [id], created: [{index, object?, linked, created, why?}]}` |
| `edit.collision.stage` | act | `{target, blocks: {name: {count, bytes}}, leaves?, mopp?, pathfinding?}` | `{stage, bytes, arena: {used, largest_free}}` |
| `edit.collision.commit` | act | `{stage, mode: append \| rebuild, activate?, was: {name: {count, address}}}` | `{at, mode, activated, freed}`; a commit that activates answers after the switch |

`owner` is `{section: {tag, kind: cluster | instance_def | mode, index}}`
or `{pixels: {tag, image, lod}}`; `offset` is from the start of the raw
block or of the LOD chain. `target` is `{structure: tag}`,
`{instance_def: {tag, index}}` or `{coll: {tag, region, permutation}}`.
`blocks` names the eight collision blocks by the fields of
`tags::collision::Bsp`, in block order (`nodes`, `planes`, `leaves`,
`refs`, `nodes_2d`, `surfaces`, `edges`, `vertices`), each its elements'
bytes as `Bsp::encode_blocks` lays them out
(`collision_build::coll::stage_blocks`).
`block` is a `placement::BLOCKS` name or `starting_locations`,
`netgame_equipment`, `netgame_flags`, `starting_equipment`.

Refusals use `REFUSED` with `data.why`: the session, a stale epoch, an
owner whose tag is not loaded, a size or format change, a full arena
(`data.need`, `data.largest_free`), a per-type limit, a count change with
other machines in the game.

### The native refresh

`render_bridge::native_edits::push(Edit)`, with
`Edit::{Section { bsp: u32, kind: u8, index: u32, grown: bool }, Model { tag: u32 }, Image { bitmap: u32, image: u32 }}`,
called by the hot-edit glue after a batch applies. `bsp`, `tag` and
`bitmap` are tag datums (matched by index). `kind` is `KIND_CLUSTER` (0),
`KIND_INSTANCE_DEF` (1) or `KIND_MODE` (2, with `bsp` the render model's
datum: queued as `Model`). Edits of one owner merge, a grown section
staying grown. The glue for the world and for objects drains its share
(`native_edits::drain(Consumer::World | Consumer::Objects)`;
`native_world_glue`, `native_objects_glue`); an `Image` goes to both.
Plain integers keep `render-bridge` free of the agent's types.

### The editor

- `play::tag_sync::Effect` keeps its five chips. Geometry in place, same-size
  pixels, an added or removed placement of a supported shape, and `append`
  collision are `now`; grown geometry and `rebuild` collision are
  `now` with a note in Pending ("the level rebuilds for a moment");
  everything under a "Not live" bullet stays `reload`.
- `play/sync.rs` sends a block whose layout revision differs from the
  loaded one as a `world.placement.edit` instead of putting its fields
  behind reload, and advances the loaded revision on the receipt.
- A new `play/raw_sync.rs` diffs `GeometryChanges` and the bitmap
  document against a baseline of acknowledged raw entries, as
  `sync::Baseline` does for fields, so undo sends the old bytes.
- `mirror.pending` and `mirror.apply` ([agent-api.md](agent-api.md)) list
  and send these with `doc` `geometry`, `bitmaps` and `collision`.

## What is live and what is not

| Edit | Live path | Shows | Fallback |
|---|---|---|---|
| Move vertices of a cluster, definition or `mode` section | `raw` + bounds `writes` | next frame | none needed |
| Change a section's topology | append, `writes`, `evict` | a few frames | reload when the section is bucket-lit or a sky |
| New material, section, cluster, tag | none, or `moves` for a material | | reload |
| Instance placement, marker | `writes` | next frame; the native world after its refresh | |
| Add or remove a placement | `world.placement.edit` | next tick | reload for the shapes creation refuses |
| Add a spawn point, netgame equipment | `world.placement.edit` | next spawn | |
| Netgame flag | `world.placement.edit` | next game | |
| Add a solid to level collision | stage, commit `append` with activation | rays and bodies after one BSP switch, on every commit | reload if the arena is full |
| Move or delete level collision | commit `rebuild` | after one BSP switch | reload on retail maps |
| `coll` BSP | stage, commit | bullets next tick, bodies on respawn | |
| `phmo` primitive | `writes` | on respawn | |
| Paint pixels, same size and format | `raw` | next frame | |
| Resize or reformat a bitmap | none | | reload |
| Lightmap rebake | `raw` on the page, when the relighter exists | next frame | |

## Rules and limits

- **Sessions.** Every method refuses by `SessionKind::writes`
  (`crates/extensions/agent-proto/src/sim/mod.rs`): local games and the
  host of a System Link or Live custom game; never co-op, a client,
  Theater, an unknown session or matchmaking. On a host with other
  machines,
  `confirm_peers` admits render geometry, pixels and collision, which stay
  local to the host as tag pokes do. A `world.placement.edit` that changes
  a count is refused there while other people are in the game: network
  object creation sends a placement index that each client checks against
  its own count (scenario-editor.md section 6). Phantom bots are not other
  people (bots.md section 5: each is a peer on the host's own simulation),
  so the host counts its peers less itself and the phantoms that have
  joined, and allows the change when none are left. A Live custom game
  this console hosts is a host like a System Link one, with the same
  prompt and the same people rule; the Live check
  (`agent_glue::live_edit_refusal`) refuses matchmaking of any kind, a
  Live game another console hosts, and a Live session it cannot read. The
  live stream still refuses every Live game.
- **Rust collision world.** The sim rewrite's collision world
  ([sim-rewrite.md](../engine/sim-rewrite.md), phase 2) is a static,
  content-addressed cache per BSP and per collision model. A collision
  commit drops and rebuilds the entry for the BSP or model it changed at
  the same tick boundary, so native and guest queries agree.
- **Determinism.** Writes happen only before `game_tick`, never from the
  stream, a render hook or a worker. Film recording and playback exclude
  them, as they exclude object edits.
- **Snapshots.** A non-empty overlay or a used arena extent is host state
  a machine snapshot does not hold, so both prevent snapshots, rewind and
  film keyframes until the map changes, as a bound play copy does
  ([snapshots.md](../runtime/snapshots.md)).
- **Checkpoints.** A campaign revert restores objects with the placement
  indices they had. After a `world.placement.edit` that removed or moved
  an entry, a `GAME_STATE_REVERT` puts that block behind reload in the
  editor.
- **Identity.** Raw offsets and block addresses are the loaded file's, so
  every primitive but `writes` requires the editor's baseline checksum to
  equal `edit.map.checksum`, as field writes do.
- **Limits.** The save's limits hold live (65,535 vertices a section, the
  per-type placement maximums, the compiler's packed-field limits). The
  arena's size bounds grown blocks and collision; a refusal names the
  bytes needed.
- **Never a quiet fallback.** A primitive that cannot apply refuses with
  its reason and the edit stays in Pending under Reload; the host does not
  apply half of a batch or substitute a preview.

## Build slices

Lanes write code and tests; the coordinator compiles and runs them.
"Fake game" is the agent crate's server over invented memory, as
`play::tests` and `hot-tags/src/tests.rs` use. Each slice documents its
methods in its own section of the runtime agent API page.

| Slice | Goal | Owns | Tests without the game | Live check | After |
|---|---|---|---|---|---|
| **R1** Overlay and raw writes | the raw overlay in both file readers; `raw` and `evict` in `edit.batch`; `edit.raw.append`, `edit.stage.put`, `edit.overlay`; the journal over raw entries; the `native_edits::push` call | `crates/runtime/vfs/src/map_override*`, `crates/platform/assets/src/resource.rs`, `crates/extensions/game/src/native_map.rs`, `crates/extensions/hot-tags`, `crates/extensions/agent/src/hot.rs`, `crates/extensions/agent-proto/src/hot.rs`, `crates/extensions/game/src/hot_glue.rs` | overlay reads across entry edges and past the end; a raw write changes the file view and a synthetic resident copy and nothing else; `was` mismatch writes nothing; revert restores both; a non-resident owner takes the overlay only; co-op, client and Live refuse | `edit.batch` with one `raw` vertex write on a visible cluster, then `edit.overlay` and `render.frame` with `snap` before and after; `edit.revert` restores the picture | nothing |
| **R2** Native refresh (built) | `native_edits`; section bounds recompute; world reload request; model replace; image rewrite | `crates/rewrites/native-render`, `crates/rewrites/render-bridge/src/native_edits.rs`, `crates/extensions/game/src/native_world_glue.rs`, `crates/extensions/game/src/native_objects_glue.rs` | `GpuWorld` after a `Section` edit equals a world loaded from the edited bytes (vertices and bounds), beside `crates/rewrites/native-render/tests/gpu.rs`; a replaced model draws its new vertices (`tests/models_gpu.rs`); an image rewrite changes the sampled texel; a size change is refused | with R1: the frame columns `native_world_opaque_prereserved_draws` and `native_world_opaque_guest_indexed_draws` name which path drew the edited part, and both pictures change | nothing |
| **R3** Placement transactions | `world.placement.edit`; linked creation; relink; name repoints; capacity from the loaded count; the count-change refusal with peers | `crates/extensions/agent/src/sim/edits.rs`, `crates/extensions/agent/src/sim/calls.rs`, `crates/extensions/agent/src/sim/calls/placement.rs`, `crates/extensions/agent-proto/src/sim/edits.rs` | fake game: add into a dead tail slot, remove with swap, and the undo of each leave the block byte-equal to the editor's save of the same edit; an occupant's index follows a relink; a refused shape writes the record and reports `created: false`; a `was` mismatch changes nothing | `world.placement.edit` adding a scenery entry after a removal, then `world.object` shows the new object with its placement; a BSP switch or `map.reload {}` keeps one object per entry | nothing |
| **R4** Edit arena and block moves | the arena, its allocator, `moves`, `edit.arena`; arena capacity for R3's growth | `crates/extensions/hot-tags/src/arena.rs` (new) and the batch, `crates/extensions/agent/src/hot.rs`, `crates/extensions/game/src/hot_glue.rs`, the capacity call in `calls/placement.rs` | allocator: first fit, free, coalesce, full; a move then a path read resolves through the new address; revert puts the pair back and frees; an `at` write inside a moved block is admitted | `edit.arena` on a loaded multiplayer map records the free size (a note in `docs/editor/notes/`); a ninth scenery entry on a block of eight appears and survives a BSP switch | R1, R3 |
| **R5** Editor: geometry and pixels live | `play/raw_sync.rs`; the effect chips; Pending rows; dirty spans from `GeometrySession`; the stroke push | `crates/apps/studio/src/play/raw_sync.rs` (new), `crates/apps/studio/src/play/tag_sync.rs`, `crates/apps/studio/src/play.rs`, `crates/tools/geometry-edit` | fake game: a `mesh.transform` is one batch with `raw` and bounds; undo sends the old bytes; View mode sends nothing; a mismatch leaves the batch pending; a grown edit sends append, `writes`, `evict` in that order | `mesh.transform` through the editor's API on a paired game, `render.frame` with `snap` shows the moved faces; `mirror.pending` is empty after | R1; R2 for the native picture; modeling slices 5 to 7 for pixels and grown sections |
| **R6** Editor: placements live | add, remove and their undo as `world.placement.edit`; the layout-revision rule; Pending text | `crates/apps/studio/src/play/objects.rs`, `crates/apps/studio/src/play/objects`, `crates/apps/studio/src/play/sync.rs` | fake game: add, remove, undo and redo each send one transaction and leave `mirror.pending` empty; a refused shape lands under Reload with the game's reason; a lost reply does not send twice | add and delete a crate in the editor and watch `mirror.objects` follow within a second | R3 |
| **R7** Collision live | `edit.collision.stage` and `.commit`, both modes; overlay cache drop; the editor's send | `crates/extensions/agent/src/hot/collision.rs` (new), `crates/extensions/agent-proto/src/hot.rs`, `crates/apps/studio/src/play/collision_sync.rs` (new) | fake game: a commit repoints all pairs at one boundary or none; `sim_collision` over synthetic memory answers from the new blocks after it; freed extents return to the arena; `rebuild` is refused on a map with pathfinding the stage did not replace | `debug.view collision_geometry` shows the new solid after an `append` commit; after `activate: true`, `sim.state.set` puts a player above it and `sim.expect` finds the player resting on its top | R4; modeling slices 8 and 11 |

R1, R2 and R3 start together, beside modeling.md's slices 5 to 8, which
own `bitmap-edit`, `assets`' encoder, `map`'s raw writers and
`collision-build`. R4, R5 and R6 start when R1 and R3 are merged and run
together. R7 is last. R1 and R4 both edit `hot.rs` and `hot_glue.rs`,
which is why R4 follows R1.

## Evidence

- **Code**: the native world and objects load from the map file through
  the VFS and are keyed by map, BSP and generation
  (`native_world_glue::BspKey`, `native_map::open_resolved`); the only
  rebuild hooks are `native_map_work::reset` and the `BSP_SWITCH` and
  `GAME_STATE_REVERT` observers.
- **Code**: `GpuWorld::set_section_vertices`
  (`crates/rewrites/native-render/src/gpu.rs`) writes a section's run and
  leaves bounds alone; its callers are the editor's viewport only.
  `GpuWorld::replace_section` writes the run and recomputes the bounds;
  the native refresh calls it (tests
  `crates/rewrites/native-render/tests/edits_gpu.rs`,
  `world::tests::a_replaced_section_equals_a_world_built_from_it`).
- **Code**: the D3D device re-reads written ranges
  (`crates/platform/d3d/src/vertex_cache.rs`, `dirty.rs`); `Lock` is not a
  hook.
- **Code**: `hot_tags::value::writable` refuses block, data and struct
  kinds by path, and an `at` write must sit inside one loaded tag; nothing
  allocates guest memory after install (`hooks::arena`).
- **Code**: `placement::record` refuses an index past the loaded count
  because no bytes exist there; `prepare_spawn` does not write the
  placement index, which is why a respawned object is unlinked.
- **Code**: no collision compiler exists; `h1convert` copies and merges
  compiled trees (modeling.md, "What exists to build on").
- **E**, cited from their pages: the block info's loaded flag and handle
  and the resident buffer's layout (structure-bsp.md); the readers that
  walk scenario blocks by count and the per-type limits
  (scenario-editor.md section 6); the MOPP activation on a BSP activation
  (multiplayer.md).
- **Design**: the overlay, the four primitives in one batch, the arena,
  the block transaction for placements, stage and commit for collision,
  same-size-only pixels.
- **Code**: the refresh queue and its drainers
  (`render_bridge::native_edits`, `native_world_glue::refresh_world`,
  `native_objects_glue::refresh_models`); tested without the game, not
  yet driven by a batch (R1 makes the `push` call).
- **S**: a resident section buffer is the file block at the same offsets.
  The `was` comparison of slice R1 on a real map verifies it.
- **S**: the texture cache's handle leads to the resident pixels'
  address. Reading the handle check with `h2 disasm` verifies it.
- **S**: the engine accepts a block in the arena, for scenario blocks and
  for collision. R4's live check verifies the first, R7's the second.
- **S**: enough physical memory is free for a useful arena. `edit.arena`
  on each map verifies it.
- **S**: evicting a section through the guest's unload callback and
  letting the lookup re-read it is safe mid-map. R5's grown check
  verifies it.
- **S**: a BSP switch to the active BSP runs the whole switch rather than
  returning early, and Havok's structure shape reads the collision blocks
  through the pairs and not through addresses it kept. Reading
  `scenario_switch_structure_bsp` and the new-BSP callback verifies both.
- **S**: writing the placement index into the placement data yields a
  linked object equal to a loaded one for the supported shapes. R3's
  comparison with `map.reload {}` verifies it.

## Not verified

- Every **S** above.
- The time from a batch to the picture for a drag on a large cluster, and
  the cost of a world reload after a grown edit.
- Whether a section missing for the frames between evict and page-in is
  visible, and whether drawing the old copy until then is needed.
- Havok contacts on appended collision: resting bodies, vehicles, a biped
  standing where a solid appears, and how odd the gap between rays and
  bodies looks before the activation.
- Bots after a collision change, with stale `los::Walls`.
- A System Link host with clients after local geometry, pixel and
  collision edits.

## Related

- [live-mirror.md](live-mirror.md): pairing, View and Edit, the effect
  table, the session table, reload.
- [modeling.md](modeling.md), [modeling-targets.md](modeling-targets.md):
  the tools, the save lists and the write classes this page sends live.
- [scenario-editor.md](scenario-editor.md) section 6: add, remove and what
  refers to entries by index.
- [hot-iteration.md](../graphics/hot-iteration.md): `edit.*`, the write
  barrier, what shows when.
- [agent-api.md](../runtime/agent-api.md): "`edit` and hot `render`",
  "Object edits", "Test maps".
- [native-world.md](../graphics/native-world.md),
  [native-objects.md](../graphics/native-objects.md): the native copies.
- [sim-rewrite-collision.md](../engine/sim-rewrite-collision.md),
  [collision.md](../formats/collision.md): who reads collision.
- [forge-design.md](../game/forge-design.md): the kit, which reserves
  capacity at build time where this page moves blocks at run time.
