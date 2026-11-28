# Realtime edits: geometry, placements, collision and textures in the running game

Status: In progress. Slices R1 (the raw overlay in both readers, `raw` and `evict` in `edit.batch`, `edit.raw.append`, `edit.stage.put`, `edit.overlay` and their journal), R2 (the native refresh), R3 (placement transactions, `world.placement.edit`), R4 (the edit arena, `moves` in `edit.batch`, `edit.arena`, placement growth into the arena; its live check has not run), R5 (the editor's geometry and pixel sends, `play/raw_sync.rs`), R6 (the editor's placement adds and removes live, `crates/apps/studio/src/play/objects/drive/placements.rs`) and R7 (`edit.collision.stage` and `.commit`, `crates/extensions/agent/src/hot/collision.rs`, the structure activation `sim::calls::Call::ReactivateStructure`, and the editor's send, `play/collision_sync.rs`, fed by the Modeling tool's collision box; its live check has not run), R8 (the tag-write notice and its subscribers), R9 (the metadata overlay), R10 (the native shader refresh) and R11 (the native refresh in every glue that holds a texture or model) and R12 (`world.object.respawn {definition}`, `crates/extensions/agent/src/sim/calls/placement/definition.rs`, and Studio's **Respawn now** for definitions, `play/objects/definitions.rs`) and R14 (block operations as `edit.batch` `moves`, `tag_sync::block_moves`) are built, the live checks of R8, R9, R10, R11, R12 and R14 not run; R13 (`applies=` evidence, `crates/formats/tags/src/defs`) is built for `nhdt`, `prt3`, `effe`, `ligh`, `obje`, `proj` and two `matg` blocks, and open for `weap`, `jpt!`, `vehi`, `bipd` and the rest of `matg` and `bitm`; R15 (Studio's viewport follows painted bitmaps, `crates/apps/studio/src/viewport/paint.rs`) is built for the level's base maps, its live check not run, and open for shaders and models; R16 (the `sound` raw owner and its eviction, `crates/extensions/hot-tags/src/raw.rs`) is built, its live check not run and the sound cache's handle not read; R17 (writes to a BSP that is not loaded, `{bsp: {index}}` owners in `crates/extensions/hot-tags/src/raw.rs` and `tag_sync::diff_live` in `crates/apps/studio/src/play/tag_sync.rs`) is built, its live check not run; what they build on is cited as **Code**.
Code: built in `crates/runtime/vfs/src/map_override/overlay.rs`, `crates/runtime/vfs/src/lib.rs` (`Vfs::read`), `crates/platform/assets/src/resource.rs`, `crates/extensions/game/src/native_map.rs`, `crates/extensions/hot-tags/src/raw.rs`, `crates/extensions/hot-tags/src/arena.rs`, `crates/extensions/hot-tags/src/batch.rs`, `crates/extensions/agent/src/hot.rs`, `crates/extensions/game/src/hot_glue.rs`, `crates/rewrites/render-bridge/src/native_edits.rs`, `crates/rewrites/native-render`, `crates/extensions/agent/src/sim/edits.rs`, `crates/extensions/agent/src/sim/calls/placement/edit.rs`, `crates/extensions/agent-proto/src/sim/edits.rs` `crates/apps/studio/src/play/raw_sync.rs`, `crates/apps/studio/src/play/objects/drive/placements.rs`, `crates/extensions/agent/src/hot/collision.rs`, `crates/extensions/agent/src/debug_views/debug_static.rs`, `crates/live/bots/src/autonav.rs` (`collision_edited`), `crates/apps/studio/src/play/collision_sync.rs`, `crates/apps/studio/src/app/modeling.rs` (`ModelAction::AddCollisionBox`), `crates/extensions/hot-tags/src/notice.rs`, for R14 `crates/apps/studio/src/play/tag_sync.rs` (`block_moves`) and `crates/apps/studio/src/app/tag_doc.rs` (`live_blocks`), and for R11 `crates/extensions/game/src/native_world_glue.rs`, `crates/extensions/game/src/native_first_person_glue.rs`, `crates/extensions/game/src/native_first_person_load.rs`, `crates/extensions/game/src/native_lights_glue.rs`, `crates/extensions/game/src/native_decorators_glue.rs` and `crates/extensions/game/src/native_post_glue.rs`; planned in the rest of `crates/apps/studio/src/play`. Tests: named per slice under "Build slices".
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
| Sound samples (`snd!` permutations) | a raw chunk per gestalt chunk record ([sound.md](../formats/sound.md)) | the guest's sound cache, found through the chunk record's runtime word | none: DirectSound re-reads guest sample memory at `Unlock` and `Play` |
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
  appended extents, and read the `Map` itself through
  `vfs::OverlaidReader`, so the metadata a live open decodes holds the
  overlay's metadata runs.

The overlay holds two kinds of runs. **Raw runs** (`RawOverlay::put`)
are bytes only the file view holds: section vertices, pixels, appended
blocks. **Metadata runs** (`RawOverlay::put_metadata`) copy tag writes:
after every applied tag write and revert, `hot_glue::MetaGlue` reads each
written span back from guest memory and puts it at its file offset in
the loaded map, translated through the tag's own region in the map's
index (`hot_glue::Spaces`: the ordinary metadata, or the region of the
structure BSP or lightmap the tag is; map-format.md, "BSP and lightmap
metadata"). A span with no file offset, as in a block moved into the
edit arena, is left out, so the native `Map` reads a moved block where
the file has it. Metadata runs are served as raw runs are, and the guest
reads them when it re-reads a BSP's region from its slot at a switch;
`edit.overlay` counts them apart (`metadata`, `metadata_bytes`).

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
| grown tag blocks (`grown`, `tag_grown`) | `moves` (built) | copies the new block into the **edit arena**, then writes the root's `(count, address)`; later writes inside the block are ordinary `writes` |

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

**The edit arena.** Guest memory for blocks that outgrow their place
(**Code**, `hot_tags::arena`). Its addresses are `hot_tags::arena::WINDOW`,
`0xa0404000..0xa0800000` (4080 KiB) of the HLE reservation
(`kernel::memory::HLE_RESERVED`): the gap between the NV2A register
window's two pages (`0xa0403000` and `0xa0800000`, `d3d::regs::PAGES` at
`d3d_glue::NV2A_BASE`), above Bink's page and below DirectSound's arena,
which no other user maps. The window is only addresses. Pages are mapped
zeroed through the runtime's memory as the arena's top grows, in 64 KiB
chunks, because physical memory bounds the arena, not the window: a
mapping that would leave less than `PHYSICAL_FLOOR` (2 MiB, provisional
until the live measurement) of physical memory free is refused, so the
game's own later allocations still fit. A first-fit allocator over
coalesced free runs hands out 16-byte-aligned extents, each booked to the
block it holds: its tag, the address of its root pair, its stride and its
capacity. The arena is bound to one map activation: the first tick of a
new map (the `edit.overlay` ticker) frees every extent and unmaps its
pages, and a BSP switch keeps it. The editor (`edit.batch`, inside the
guest lock) and `world.placement.edit` (between its memory closures,
through `hot_tags::arena::Link`, which takes the guest lock first) share
one arena, which `hot_glue::install` makes and the boot hands to the call
queue (`Queue::set_arena`). The engine follows any mapped pointer, so a
block there reads as one in the metadata does (**S**, the same
supposition as modeling-targets.md's "block pointer into ordinary
metadata"). How much physical memory is free on a loaded map is not
measured; `edit.arena` reports it (`physical_free`). An arena extent
belongs to the moved block's tag for `edit.batch`'s "inside one loaded
tag" rule, for `edit.read` and for the journal's survival of a BSP switch;
a path follows the repointed pair on its own (`hot_tags::path`).

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
  (`placement::edit::BlockState`), or the capacity of the arena extent it
  moved to. With no room the host moves the block into the arena with
  twice the capacity (at least the new count), bounded by the per-type
  limit the editor enforces ([scenario-editor.md](scenario-editor.md)
  section 6, "Add"): the extent is allocated before any write, the
  records and the dead tail are copied into it, the rest is zeroed, and
  the root's address is written with the records and the count; an
  extent the block left is freed. A full arena, or a game without one, is
  a refusal with `data.need` and `data.capacity` (and `data.need_bytes`
  and `data.largest_free` from a full arena), and the edit falls back to
  a reload.
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

The game reads collision from guest memory in place; the native
collision world, always installed, checks each resident BSP and MOPP
against its guest source by fingerprint (`collision.sources`,
[agent-api-checks.md](../runtime/agent-api-checks.md)). Live collision is a pointer swap in guest memory, staged off the tick
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
| `append` | solids added by `collision_build::merge` (`geometry_edit::level_collision::LevelEdit::stage` gives the blocks, the per-leaf table and the MOPP, `collision_build::level::LevelStage`): every existing plane, surface and leaf keeps its index | none; or, with `activate: true`, the structure activation below | rays next tick; Havok bodies after an activation |
| `rebuild` | surfaces moved or deleted: indices change | always: the structure activation below, which rebuilds the Havok world over the structure and finds every object's location again | after a hitch of one activation; players and their state stay, as across a campaign switch |

The editor sends `append` unless an indexed block shrinks, and asks for
`activate: true` whenever the stage's reply says the game has an
activation, so Havok bodies match the collision the rays see. Each
activated commit costs one activation's hitch; the editor commits once
per edit it sends, not per mouse event.

The activation is a seam the game installs (`hot::collision::Activation`,
`Agent::set_collision_activation`). `BSP_SWITCH` (`0x11c1b0`, stdcall
`(bsp, unload)`, AL the result, `ret 8`; **E**, `h2 disasm 0x11c1b0`)
cannot be asked for the active BSP: when the BSP asked for is both the
loaded one (`0x4686c4`) and the active one (game globals `+2`) it returns
at once. Its second half can: `STRUCTURE_BSP_DEACTIVATE` (`0x137da0`) runs
every game system's dispose-from-old-structure-BSP callback (the lifecycle
table's column 5, last to first) and sets the active BSP to -1, and
`STRUCTURE_BSP_ACTIVATE` (`0x137d40`) sets it to the loaded BSP again and
runs every initialize-for-new-structure-BSP callback (column 4): physics
builds its Havok world over the structure (`0x1c2690`), objects find
their locations again (`0xb6bb0`). Both are `fn() ret 0`. The game glue
installs `hot::collision::QueuedActivation` (`hot_glue::install_collision_activation`),
which queues `sim::calls::Call::ReactivateStructure` on the agent's guest
calls: at the start of the next tick, refused unless a structure is loaded
and active, the pair runs and the active index is read back. The BSP's
tag data is not loaded again (`0x122b40` is skipped), so the committed
pairs stay; the `BSP_SWITCH` observers do not fire. A game without the
agent's guest calls has no activation, and there `activate` and `rebuild`
are refused and bodies keep the old shape until the next BSP switch.

What follows the commit without more work (**Code**):
`sim_collision`'s page watches over the header, the eight blocks and the
leaf table start a new generation when a block is repointed (test
`a_warm_point_walk_observes_aliased_edits_and_relocated_planes`,
`crates/rewrites/engine/src/sim_collision/sources.rs`), so the activation
needs none of the `BSP_SWITCH` observers. What the commit does itself:
it hands the new BSP to the debug views (`debug_views::CollisionEdits`),
whose static cache replaces the structure's lines on its next refresh
(`StaticViews::replace_structure`, `debug_static.rs`), and tells the
listeners `Agent::on_structure_collision` adds (after the activation,
when it asks for one). The game glue's listener marks the bots'
`los::Walls` stale when the structure is the active one
(`[0x4e0348]`): `bots::autonav::collision_edited` rebuilds them from the
committed BSP on a thread of their own (`bots::cache::walls_of`, every
surface's ring fanned, the glass) and publishes them beside the same nav
mesh and places for the same map load. The new walls have no visibility
grid, so line of sight walks the cells until the map loads again, and the
nav mesh, built offline per map and BSP ([bots.md](../game/bots.md)),
keeps the old shape.

What holds an index across ticks and so decides `append` against
`rebuild`: each object's location (leaf, cluster, BSP;
`guest_layout::object::ObjectHeader::LOCATION`), Havok's cached MOPP keys
that name surfaces ([havok-runtime.md](../engine/havok-runtime.md)), and
pathfinding's per-surface references (`tags::pathfinding`). `append` keeps
them all valid. `rebuild` relies on the switch to refresh the first two;
pathfinding would have to be regenerated (`pathfind::generate`) and moved
in the same commit. A stage does not take pathfinding (its blocks hold
absolute pointers and no encoder exists), so a `rebuild` of a structure
with pathfinding is refused.

- **Collision models (`coll`).** The same stage and commit on one
  permutation's BSP. Ray and sweep tests read the tag each query, so
  bullets hit the new shape on the next tick. BSP-physics slots hold
  pointers and vtables the load fixup wrote ([collision.md](../formats/collision.md)),
  and a live body's Havok shape points at the old blocks, so the host
  keeps the old extents alive until the map changes, and the editor
  respawns the objects of that definition (`world.object.respawn`).
  **Shows: bullets next tick, bodies on respawn.**
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
- The `Image` refresh rewrites the native renderer's textures wherever
  one is held: the world's material programs, the lightmap pages, the fog,
  water and layer stage images, the objects', the first-person models',
  the light passes' stage textures and gels, and the decorators' stage
  textures (see "The native refresh").
- During a stroke the editor sends mip 0's changed bytes at the push
  interval, and the rebuilt mips at the stroke's end. A request line is at
  most 1 MiB (`MAX_LINE`, `crates/extensions/agent/src/server.rs`), so
  the editor splits a larger plan into batches of at most 256 KiB of raw
  bytes (`raw_sync::BATCH_RAW_BYTES`); each raw entry stands alone, so a
  split level shows in parts over a few frames. `edit.stage.put` is not
  used by the editor.
- A `P8` lightmap page is the same path with palette-quantised bytes.

**Shows: next frame, same size and format.** A new size, format or mip
count changes the image record and what the guest's texture cache
allocated for it: After reload. Evicting and re-paging a resized texture
is not designed here because the cache's behaviour with a changed record
is not known (modeling.md, "Not verified").

### Sound samples

A `snd!` tag holds no samples: its pitch ranges, permutations and chunks
are rows of the map's one `ugh!` gestalt, and each chunk record (12
bytes) names a raw block by pointer and size, bit 31 of the size a flag
([sound.md](../formats/sound.md), "Tables"). The owner of a sample write
is one chunk, `{sound: {tag, range, permutation, chunk}}`, each index
counted within the sound: `range` within its pitch ranges, `permutation`
within that range, `chunk` within that permutation
(`hot_tags::raw::Owner::Sound`). The host follows the sound's first range
and count to the gestalt's range, then its permutation run, then its chunk
run, and refuses an index past either the sound's run or the gestalt's
block; a map without an `ugh!` refuses every sound owner.

- The overlay takes the bytes. The resident copy, if any, is not written:
  the sound cache's buffer for a chunk is not found (**S**, below), so a
  sound write reports `resident: false` and `refresh: null`; no native
  copy holds samples.
- Eviction (`evict: [{sound: ...}]`) sets the chunk record's runtime
  word (`+8`) back to the file's `0xffffffff`
  (`hot_tags::raw::evict_sound`), so the next play of the permutation
  misses and reads the chunk from the file view. Revert overlays the old
  runs back and evicts again. The cache's own entry for the old samples
  is left to its eviction, as a section's is. A voice already playing
  keeps its buffer, and a WMA stream decodes the bytes it already read.
- Field edits of `snd!` and `ugh!` stay `next_use`.

**Shows: the next play, same encoding and length.** Studio sends the
chunk's bytes and its eviction in one batch. A new length, encoding or
chunk count changes the gestalt's rows: After reload (growing the
gestalt is `moves` plus `edit.raw.append`, not built).

### Inactive BSPs

Only the loaded structure BSP's region is in guest memory
(`hot_tags::raw::loaded_bsp`, the `i16` at
`engine::sim_collision::layout::BSP_INDEX`); the scenario's other BSPs
live in their regions of the map file alone, and a switch reads the
region from the guest's slot, which serves the overlay as the map
("The edit overlay"). A write to such a BSP is a raw write whose owner
is `{bsp: {index}}`, the scenario's structure BSP descriptor (`+0x210`,
`0x44` a descriptor: file offset, size, virtual base;
map-format.md, "BSP and lightmap metadata"). Its offset counts from the
region's start. The host checks `was` against the file view, puts the
bytes in the overlay as raw runs and writes no guest byte; no native
refresh is queued, since the switch loads the world from the overlaid
file. The loaded BSP is refused as an owner: its region is guest memory,
written with `at`. A revert puts the overlay's runs back, and when the
BSP has been switched to since, writes the file's bytes back into its
region at the descriptor's virtual base where guest memory still holds
the written ones. These are raw runs, so they refuse snapshots as any
raw run does ("Rules and limits").

Studio learns the loaded BSP from `edit.map`'s `bsp`
(`play::Edits::bsp`). `tag_sync::diff_live` turns each changed region of
another BSP into a `tag_sync::RegionWrite` (the runs that differ from
what the game holds, each with the file's or the game's bytes as `was`),
sent as `raw` entries in the same `edit.batch` as the field writes, with
a pending row **Next use** ("read when the game switches to this BSP").
When the game takes it, the baseline holds the region's patches
(`TagBaseline::took_bsp`). The loaded BSP's region, a region of no
descriptor and a region the game refused wait under **After reload**
with the reason; a switch clears the refusals.

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
| `edit.batch` (extended) | act | adds `raw: [{owner, offset, bytes, was?}]`, `moves: [{root: {tag, path} \| {at}, stride, count, capacity?, bytes, was: {count, address}}]`, `evict: [owner]` | adds `raw: [{owner, resident, refresh}]`, `moves: [{root, tag, path, count, address, capacity}]`, `evicted`; `MISMATCH` lists raw entries by `owner` and `offset`, moves by `root` |
| `edit.raw.append` | act | `{file?, bytes, align?}` | `{id, pointer, len}`: a raw pointer for tag writes; journalled, dropped by revert |
| `edit.stage.put` | act | `{stage?, bytes}` | `{stage, len}`: appends to a host buffer that dies with the connection or its first use |
| `edit.arena` | read | none | `{base, len, mapped, used, largest_free, physical_free, physical_floor, moves: [{tag, name, root, address, capacity, stride, len}]}` |
| `edit.overlay` | read | none | `{entries, bytes, appended, metadata, metadata_bytes, owners: [owner]}` |
| `world.placement.edit` | act | `{block, was_count, count, records: [{index, bytes, was?}], names?: [{name, placement}], delete?: [index], relink?: [{from, to}], create?: [index]}` | `{at, count, capacity, moved?: {address, capacity}, deleted: [id], relinked: [id], created: [{index, object?, linked, created, why?}]}` |
| `edit.collision.stage` | act | `{target, blocks: {name: {count, bytes}}, leaves?, mopp?, pathfinding?}` | `{stage, bytes, arena: {used, largest_free}}` |
| `edit.collision.commit` | act | `{stage, mode: append \| rebuild, activate?, was: {name: {count, address}}}` | `{at, mode, activated, freed}`; a commit that activates answers after the switch |

`owner` is `{section: {tag, kind: cluster | instance_def | mode, index}}`,
`{pixels: {tag, image, lod}}` or `{sound: {tag, range, permutation,
chunk}}`; `offset` is from the start of the raw block, the LOD chain or
the sound chunk. `target` is `{structure: tag}`,
`{instance_def: {tag, index}}` or `{coll: {tag, region, permutation}}`.
`blocks` names the eight collision blocks by the fields of
`tags::collision::Bsp`, in block order (`nodes`, `planes`, `leaves`,
`refs`, `nodes_2d`, `surfaces`, `edges`, `vertices`), each its elements'
bytes as `Bsp::encode_blocks` lays them out
(`collision_build::coll::stage_blocks`).
`leaves` is `{count, bytes}` of the structure's per-leaf block
(`sbsp+0x2c`, 8 bytes a leaf) and `mopp` the bytes of the `sbsp+0x1e0`
block; for an added solid all three come from
`collision_build::level::LevelStage`, the bytes the map's save writes.
`block` is a `placement::BLOCKS` name or `starting_locations`,
`netgame_equipment`, `netgame_flags`, `starting_equipment`.

Refusals use `REFUSED` with `data.why`: the session, a stale epoch, an
owner whose tag is not loaded, a size or format change, a full arena
(`data.need`, `data.largest_free`), a per-type limit, a count change with
other machines in the game.

### The native refresh

`render_bridge::native_edits::push(Edit)`, with
`Edit::{Section { bsp: u32, kind: u8, index: u32, grown: bool }, Model { tag: u32 }, Image { bitmap: u32, image: u32 }, Tag { group: u32, tag: u32 }, Shader { tag: u32 }}`,
called by the hot-edit glue after a batch applies. `Tag` comes from the
tag-write notice ("Tag-write notices" below); `group` is the
four-character code read big-endian. A `Tag` of a `shad`, `stem`,
`spas` or `vrtx` (`native_edits::SHADER_GROUPS`) is queued as
`Shader`. `bsp`, `tag` and
`bitmap` are tag datums (matched by index). `kind` is `KIND_CLUSTER` (0),
`KIND_INSTANCE_DEF` (1) or `KIND_MODE` (2, with `bsp` the render model's
datum: queued as `Model`). Edits of one owner merge, a grown section
staying grown. Each glue that holds a copy drains its share at a frame
boundary (`native_edits::drain(Consumer)`), once its map's copy is
resident and no refresh of its own is running:

| Consumer | Glue | `Section` | `Model` | `Image` |
|---|---|---|---|---|
| `World` | `native_world_glue` | replaced in place, or the world reloads | | material programs, lightmap pages (`GpuWorld::rewrite_page`, decoded again through each palette of the load), fog, water and layer stage images, and gels (`GpuWorld::held`, `rewrite_image`, `rewrite_gel`) |
| `Objects` | `native_objects_glue` | | model replaced | program textures |
| `FirstPerson` | `native_first_person_glue` | | the loader is asked for the model again, under its first key, from a map opened again | program textures (`native_first_person_load::refresh_images`) |
| `Lights` | `native_lights_glue` | | the light models load again (`models::Loader::invalidate`) | the built world passes' stage textures, and the edited image for passes built later (`materials::Resolver::refresh_images`) |
| `Decorators` | `native_decorators_glue` | prepared materials forgotten, made again from guest memory | the same | prepared materials' stage textures (`MaterialGpu::held`), and the edited image for materials made later |

A retained diffuse or specular light keeps its mesh, stage texture views and static gel.
A section edit replaces the retained vertex buffer. Bitmap and gel edits give later light
draws the new texels while a captured scene replays with its original views.

A section that is not a model's goes to `World` and `Decorators`; a model
to `Objects`, `FirstPerson`, `Lights` and `Decorators`; an image to all
five. `Tag` goes to `World` and `Objects`; `Shader` goes to `World`,
`Objects` and `FirstPerson`. Every holder reports its textures as
`gpu::Held` (texture, stage, signed channels) and one function writes them
(`gpu::rewrite_held`, `gpu::rewrite_pages`): every texture is checked
before the first write, so a size, level count or format change refuses
the lot and the texture keeps its pixels until the map reloads, with a log
line. The screen effects slot of `native_post_glue` is keyed by the map's
name and its load generation (`EffectsSlot`), so a map opened again loads
its effects again. Plain integers keep `render-bridge` free of the agent's
types. The world and objects glue drain `Tag` and change nothing for it:
no part of their copies follows another group's metadata field live.

A `Shader` edit refreshes what was compiled from the shader, read again
through the metadata overlay ("The edit overlay"). A material is built
from the edited tag when it is that shader or its resolved material
uses it as template, pass or vertex shader
(`tags::material::Material::uses`, matched by index):

| Consumer | What a shader edit does |
|---|---|
| world material programs | `native_world_glue::refresh_shaders` compiles each native material built from the tag again, as the load does (`material_programs`, limited to those materials), and puts it in place with `GpuWorld::refresh_programs` (`ProgramSet::refresh`). `MaterialProgram::change_to` sorts the change: `Constants` (the vertex and pixel uniforms, the rigid program's vertex constants) rewrites the material's uniform buffers and its rigid slots in place; `Program` (module, stage image, sampler, texture shape, signedness, inputs, pass state, rigid module) builds that material's `Gpu`, rigid program and batch groups again, binding the lightmap pages the load uploaded. Materials that do not use the tag are not touched. All or nothing: a material with no program or a stage that does not decode refuses the lot |
| world materials on the D3D path | nothing: their draws read the edit from guest memory |
| fog, water, layers, lights, decorators, opaque pass | they hold their own compiled copy; when one draws an edited shader (`FogView::programs`, `WaterData::programs`, `LayerData::programs`, `LightsWorld::draws_material`, `DecoratorLooks::shaders`, or an opaque pass whose programs or stage recipe differ, `opaque_materials::Library::same_for`) the world reloads (`native_map_work::reload_bsp`), naming them in the log |
| world, when a refresh cannot hold | an edited material that no longer compiles, or that would draw other parts than it does, reloads the world |
| objects | `native_objects_glue::refresh_models` loads each resident model with a part whose shader is built from the tag (`LitObjects::model_shaders`) again, as a `Model` edit does |
| first person | `native_first_person_glue` drops its map state; the next ask loads the viewmodels by map name, through the overlay |

### Tag-write notices

After an `edit.set`, an `edit.batch` or an `edit.revert` applies,
`hot_tags::Editor` sends one `hot_tags::TagWrite` per tag whose bytes
changed to the sink given by `Editor::with_tag_writes`
(`crates/extensions/hot-tags/src/notice.rs`): the tag's group, datum and
metadata allocation, the guest range from the first changed byte to past
the last, each field write's span, and whether it was a revert. A
moved block adds its root pair to the range and records the count and
address the move displaced. Unchanged bytes and refused batches send
nothing. The notice is sent with the guest still held, so a subscriber
reads the new bytes and writes its own copy at the same boundary.
`hot_glue::tag_written` hands each notice to the host copies:

| Group | Subscriber | What it does |
|---|---|---|
| every tag | `native_edits::push(Edit::Tag)` | queues the tag for the native copies |
| `jmad` | `engine::sim_anim::STORE.flush_range` | drops the clips keyed in the graph's animation record array, including arrays displaced by a move or revert. The next lookup decodes them from the guest |
| `nhdt` | `hud_kit::tag_written` | copies each retail widget block the range overlaps into the HUD kit's copy the root points at (`hud_kit::recopy`) |
| `weap`, `matg` | `aim_assist::tag_written` | adopts each edited aim-assist field as retail; later activations leave it unscaled while it holds the edit's bits |
| `sbsp` | `bots::cache::mark_stale` | marks the loaded bot data stale (`bots::cache::stale` says why) until the next map's data replaces it |

### The editor

- `play::tag_sync::Effect` keeps its five chips. Geometry in place, same-size
  pixels, an added or removed placement of a supported shape, and `append`
  collision are `now`; grown geometry and `rebuild` collision are
  `now` with a note in Pending ("the level rebuilds for a moment");
  everything under a "Not live" bullet stays `reload`.
- `play/sync.rs` computes a block whose layout revision differs from the
  loaded one as a `world.placement.edit` (`Baseline::transaction`)
  instead of putting its fields behind reload, and advances the loaded
  revision on the receipt (`Baseline::placed`);
  `play/objects/drive/placements.rs` sends it, one at a time, and keeps
  the block under Reload with the game's reason when refused or when the
  answer is lost.
- `play/raw_sync.rs` (built) diffs the Modeling tool's patches
  (`GeometrySession::changes`, each `RawWrite` naming its section and
  each patch carrying the file's bytes as `was`) and the Textures tool's
  painted layers, encoded by `assets::encode::encode_tiles` into every
  LOD chain and mip the image stores, against `raw_sync::RawBaseline`:
  each raw and tag byte the game acknowledged, beside the file's byte,
  and each grown section's appended block and pointer. A byte the
  baseline holds and the documents no longer change wants its file byte
  back, so undo sends the old bytes. Every entry carries `was`, inline up
  to 64 bytes and as `{hash}` beyond; the bounds go as `writes` in the
  same batch.
- A grown section is two requests: `edit.raw.append` with the encoded
  block (aligned to `geometry_edit::RAW_ALIGN`), then one batch with the
  pointer word the append returned, the record and bounds writes, and
  `evict` naming the section. A block appended this map is reused while
  the same block is wanted; undoing the growth writes the file's pointer
  word back and evicts again.
- Raw requests go when no field batch is waiting, one at a time, at the
  push interval. During a stroke only mip 0's bytes go; the other levels
  go at the stroke's end. A plan larger than 256 KiB of raw bytes goes as
  several batches, the tag writes and evictions with the last.
- A refusal or `MISMATCH` holds every geometry and pixel row in Pending
  (a mismatch marked `conflict`, a refusal **After reload** with the
  game's reason) until the documents change or the user sends again. A
  geometry edit the save refuses, one whose tag the game has not loaded,
  and a grown section a play copy holds (its pointer is not known) are
  **After reload** rows and are not sent. Shared-cache pixels go to the
  owner the image's pointer names, which the overlay serves.
- A play copy's baseline (`RawBaseline::of_copy`) holds the geometry the
  save wrote; pixel edits are not in the copy.
- `play/collision_sync.rs` diffs `Modeling::level_collision` (one
  `geometry_edit::level_collision::LevelEdit` per structure, set through
  `App::set_level_collision`) against what the game was last committed:
  one stage of the blocks `LevelEdit::stage` makes, then one commit naming
  the pairs the game holds as `was`, `append` unless an indexed block
  shrinks, else `rebuild`. A structure the documents no longer change
  wants the file's blocks back, so undo stages them again. Blocks over
  256 KiB go ahead in `edit.stage.put` chunks. Collision goes when no field
  batch and no raw plan is waiting. A refusal holds the row **After
  reload** with the game's reason, and a `MISMATCH` marks it `conflict`;
  sending again after a mismatch commits against the pairs the game said
  it holds. The Modeling tool's collision box sets the edit
  (`ModelAction::AddCollisionBox`: the section's Add collision box, the
  editor API's `geometry.collision.add_box`; each box goes onto the ones
  before, from the file's collision), and `ModelAction::ClearCollision`
  (`geometry.collision.clear`) drops it, which stages the file's blocks
  again ([modeling.md](modeling.md)).
- A block operation on a tag the game has loaded (`TagDoc::block_op`; not a
  structure BSP, lightmap, collision or physics tag) is a block move when
  it leaves every element's index where it was: an add, or the removal or
  copy of the last element (the definitions do not say which blocks other
  blocks index, so a removal or copy anywhere else shifts elements and
  stays **After reload**). `tag_sync::block_moves` compares the block as
  the document holds it, count and elements, with what the game was last
  given (the source's until a move is answered) and sends one `moves`
  entry per changed block in a batch of its own, the root named by raw
  segments from the tag's root (`+0x8:bytes[8]`, `+0x50/0x54[2].+0x10:bytes[8]`)
  and `was` the pair the game holds; the answer's `address` is the next
  `was`, so a later edit, or the undo of the add, moves the whole block
  again. The block's elements and pair are left out of the field diff. A
  block inside another changed block, one over 64 KiB, a play copy's block
  built where the document does not know its address, and a path no
  definition names are Shape rows under **After reload**, each with its
  reason. A refused batch holds its moves (`MoveRefusals`) until the block
  changes or the user sends again.
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
| Add a solid to level collision | stage, commit `append` with activation | rays and bodies after one structure activation, on every commit | reload if the arena is full |
| Move or delete level collision | commit `rebuild` | after one structure activation | reload on retail maps |
| `coll` BSP | stage, commit | bullets next tick, bodies on respawn | |
| `phmo` primitive | `writes` | on respawn | |
| Paint pixels, same size and format | `raw` | next frame; native copies (world, lightmap pages, fog, water, layers, objects, first person, lights, decorators) after their refresh | |
| Resize or reformat a bitmap, add an image or a sequence to one | none | | reload |
| Replace a sound's samples, same encoding and length | `raw` + `evict` on each chunk | the next play | reload for another length or encoding |
| Lightmap rebake | `raw` on the page, when the relighter exists | next frame | |

## Realtime coverage

The survey of every tag group, with the code each row is read from, is
[notes/2026-11-23-realtime-survey.md](notes/2026-11-23-realtime-survey.md).
What decides coverage beyond geometry, placements, collision and pixels
(**Code**, cited there):

- **Tag-field writes reach the lifted code and the D3D path at once,
  and the native copies at their next load.** Live opens read the map's
  metadata through the overlay's metadata runs
  (`native_map::open_resolved`), so a native load after a write decodes
  it. Every native loader reads shaders through
  `tags::material::ShaderLibrary::read` at load; a shader, template,
  pass or vertex shader edit refreshes the world's material programs in
  place and reloads the objects' models, the viewmodels, and the world
  when its fog, water, layers, lights, decorators or opaque pass draw it
  ("The native refresh"). Other groups' field edits reach the native
  copies at their next load.
- **The native refresh carries raw edits and shader edits** (`Section`,
  `Model`, `Image`, `Shader`), drained by the world, objects, first
  person, lights and decorators glue ("The native refresh"). A shader
  refresh never admits a part the load left on the D3D path.
- **Studio's own viewport rebuilds from geometry previews, markers and
  the live stream**, not from tag field edits or painted pixels.
- **Studio has no embedded game view.** Play launches the game as a
  child process and pairs with it over the agent socket, as with an
  attached game ([editor.md](editor.md), "Embedded play (not built)").
- **Respawn by definition** remakes the placed objects of the tag datum
  a `next_spawn` row names: the row's object tag, or the object tags that
  reach its `hlmt` or `phmo` (`uses`). Each placement is made linked
  ("Placements", linked creation) and its old object deleted; held,
  unoccupied and unsupported placements come back in `skipped` with the
  reason, and live objects no placement made in `engine_made`
  (`crates/extensions/agent/src/sim/calls/placement/definition.rs`).
- **Host copies outside the renderer** follow tag writes through the
  tag-write notice ("Tag-write notices"): the animation clip store, the
  HUD kit's copy of the `nhdt` widget blocks and aim assist. The bots'
  map data is marked stale and not rebuilt. The collision world follows
  writes by its page watches.

Slices R8 to R17 close these, in the survey's order: shaders first, then
completed textures and models, Studio's own view, object definitions,
block growth, HUD and UI, sounds and inactive BSPs. Sizes are the
survey's (**S** days, **M** about a week, **L** weeks with engine
reading). Whole new tags stay a reload: a new index slot, datum and
string IDs touch every cache keyed by tag index (XL, not sliced).

| Slice | Goal | Owns | Tests without the game | Live check | After |
|---|---|---|---|---|---|
| **R8** Tag-write notice (S; built, live check not run) | after a batch or revert applies, one notice per changed tag (`group`, datum, range) to host subscribers; `native_edits` gains `Tag { group, tag }`; subscribers: the clip store flushes that tag's clips, the HUD kit re-copies edited widget blocks, aim assist adopts an edited field as retail, bot data is marked stale | `crates/extensions/hot-tags/src/batch.rs`, `crates/extensions/game/src/hot_glue.rs`, `crates/rewrites/render-bridge/src/native_edits.rs`, `crates/rewrites/sim-anim/src/store.rs`, `crates/extensions/game/src/hud_kit.rs`, `crates/extensions/game/src/aim_assist.rs` | fake game: a field batch queues one notice per tag and a revert another; a flushed clip is rebuilt from the edited bytes; aim assist's next activation keeps an edited value | `edit.set` on an `nhdt` widget colour shows on the HUD within a frame | nothing |
| **R9** Metadata overlay (M; built, live check not run) | each applied tag write and revert also enters the raw overlay at the metadata's file offset (ordinary metadata, or the active BSP region's translation); live opens read the `Map` through the overlay; these entries do not refuse snapshots, since guest memory holds the same bytes | `crates/runtime/vfs/src/map_override/overlay.rs`, `crates/extensions/game/src/native_map.rs`, `crates/extensions/game/src/hot_glue.rs` | a `Map` opened after a write reads the written bytes; a revert restores the file's; a BSP-region write lands at that region's offset | `edit.set` then a BSP switch: the native world's shader library holds the edited value | R8 |
| **R10** Native shader refresh (L; built, live check not run) | `native_edits::Edit::Shader { tag }`, queued by R8 for `shad`, `stem`, `spas`, `vrtx`; a constants-only change rewrites the material's uniform buffers; any other change rebuilds the programs of the parts that use it in the world, layers, fog, water, lights, decorators, objects and first person; a consumer that cannot rebuild asks for `reload_bsp` or a model reload | `crates/rewrites/native-render`, `crates/extensions/game/src/native_world_glue.rs`, `crates/extensions/game/src/native_world_materials.rs`, `crates/extensions/game/src/native_objects_glue.rs`, the fog, water, layers, lights, decorators and first-person glue | a refreshed `ProgramSet` equals one built from the edited shader; a template change rebuilds only the parts that use it | `edit.set` on `postprocess[0].colors[0].color` of a world shader: `render.frame` with `snap` shows the colour on the native path (`native_world_opaque_*` counters non-zero) | R9 |
| **R11** Textures and models everywhere (M; built, live check not run) | the `Image` refresh reaches lightmap pages, fog, water and layer stage images, decorator looks, the lights' textures and first person; first person and light models drain `Model`; decorators drain `Section` and `Model`; the screen-effects slot is keyed by map generation | `crates/rewrites/native-render/src/gpu.rs`, `crates/extensions/game/src/native_first_person_glue.rs`, `crates/extensions/game/src/native_lights_glue.rs`, `crates/extensions/game/src/native_decorators_glue.rs`, `crates/extensions/game/src/native_post_glue.rs` | each holder's texture equals a decode of the edited image; a lightmap page rewrite changes the sampled texel | paint a lightmap page and a first-person weapon texture; both show natively | R2 |
| **R12** Respawn by definition (M; built, live check not run) | `world.object.respawn {definition}`: every placed object of a definition remade linked; held, occupied and engine-made objects listed with why and left to their next creation; Studio's **Respawn now** for `next_spawn` rows of the `obje` family, `hlmt` and `phmo` | `crates/extensions/agent/src/sim/calls.rs`, `crates/extensions/agent/src/sim/calls/placement.rs`, `crates/apps/studio/src/play/objects` | fake game: two placements of one definition both remade with their indices; a held weapon reported, not touched | edit a vehicle's `hlmt` variant and respawn by definition: `world.object` lists the new objects | R3 |
| **R13** `applies` evidence (M; built in part, no live check) | `applies=` on group, struct and block lines, with evidence, for `weap`, `proj`, `jpt!`, `vehi`, `bipd`, `effe`, `prt3`, `nhdt`, `matg`, `bitm`, `ligh` | `crates/formats/tags/src/defs` | the definitions' own tests; `builtin_applies_lines_carry_evidence` (every `applies=` has an `E` or `V` word) and `builtin_applies_timings_for_the_covered_groups` (`crates/formats/tags/src/defs/tests.rs`); the share of `not_known` fields per group reported: `nhdt` 61 of 88 typed fields carry a timing, `prt3` 21 of 70, `effe` 15 of 59, `ligh` 8 of 25, `obje` 5 of 51, `proj` 2 of 60, `matg` 3 of 184; `weap`, `jpt!`, `vehi`, `bipd` and `bitm` none, for want of recorded reader evidence | the Pending chip of an `nhdt` widget offset reads **Now**, a `prt3` emitter lifespan **Next use**, an `obje` model **Next spawn**; `edit.set` on a widget `offset_full` moves it on the HUD within a frame |
| **R14** Block growth from Studio (M; built, live check not run) | `TagDoc::block_op` add, and the removal or copy of the last element, become one `edit.batch` of `moves` (the whole block as the document holds it, into the arena, `was` the pair the game holds); an operation that shifts later elements, which other blocks may index, a block inside another changed block, one over 64 KiB, a play copy's grown block and a path the definition cannot name stay **After reload** with the reason | `crates/apps/studio/src/play/tag_sync.rs` (`block_moves`, `BlockMove`, `MoveRefusals`), `crates/apps/studio/src/app/tag_doc.rs` (`BlockEdit`, `live_blocks`), `crates/apps/studio/src/play/edits.rs` | `tag_sync::tests::block_operations_are_moves_when_no_index_shifts`, `a_removal_that_shifts_elements_stays_under_reload`, `a_refused_move_is_held_until_the_block_changes`; fake game `an_added_element_is_one_batch_with_a_move` (one batch with one move, no reload; undo is a second move of the source's elements whose `was` is the first's answer) | add a colour to a shader's postprocess block in Studio; `edit.arena` lists the move and the colour draws | R4, R13 |
| **R15** Studio's own viewport follows tags (M; built for base maps, live check not run) | an open bitmap of the Textures tool whose pixels changed is drawn again as the base map of every material of the level that took it (image 0 of the bitmap the shader names as its base map), in the live viewport and in `view.snapshot`; the copy has the upload's size and level count, a box-filtered chain, and goes through the Lighting mode's transform (Lightmap only keeps white); an image of another size is refused whole and stays as drawn; a bitmap whose edits are dropped uploads the level again. Open: a material's constants or programs follow a shader document (the viewport's base look compiles no shader tag, so R10's refresh has nothing to feed there; its layer passes would), and the placed objects' models and textures | `crates/apps/studio/src/viewport/paint.rs` (`Painted::sync`), `crates/apps/studio/src/viewport.rs` (`follow_paint`), `crates/tools/scenario-edit/src/native_view.rs` (`NativeWorld::repaint_base`, `box_chain`), `crates/tools/scenario-edit/src/render.rs` (`Viewport::set_native_base_map`), `crates/rewrites/native-render/src/gpu.rs` (`GpuWorld::rewrite_base_map`) | `native_view::tests::a_repainted_base_map_changes_the_drawn_world` (the centre pixel takes the new colour; another size writes nothing; Lightmap only writes nothing), `a_box_chain_averages_two_by_two_blocks`; `api::tests::bitmaps_tests::the_snapshot_follows_a_painted_base_map` (a fill changes the flat snapshot to the colour and undo restores it) | `bitmap.fill` on a base map, then `view.snapshot` differs; in the window, paint on the Textures canvas and the viewport floor follows | R10, R11 |
| **R16** Sound samples (L; built, live check not run) | a raw owner for a `snd!` permutation's sample data and its eviction from the guest's sound cache; field edits stay `next_use` | `crates/extensions/hot-tags/src/raw.rs`, `crates/extensions/agent/src/hot.rs` | fake game: a permutation write takes the overlay; an eviction clears the cache entry (`crates/extensions/hot-tags/src/raw/tests.rs`, the `sound` tests) | replace a footstep's samples; the next step plays them | R1; the sound cache's handles read with `h2 disasm` |
| **R17** Inactive BSPs (S; built, live check not run) | Studio sends edits of a BSP that is not active as overlay entries at its region's file offsets; the next switch reads them | `crates/extensions/hot-tags/src/raw.rs`, `crates/apps/studio/src/play/tag_sync.rs` | a write to an inactive BSP's region is held by the overlay and not written to guest memory | edit a second BSP's cluster record, switch to it, `edit.get` reads the edit | R9 |

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
  stream, a render hook or a worker. A film named with `--film` and film
  playback exclude them, as they exclude object edits. The first write in
  a session's [default recording](../game/film-format.md#recording-every-game)
  discards its tape and the game in play; films of games already ended
  stay.
- **Snapshots.** An overlay holding raw runs or appended extents
  (`RawOverlay::holds_raw`) or a used arena extent is host state a
  machine snapshot does not hold, so both prevent snapshots, rewind and
  film keyframes until the map changes, as a bound play copy does
  ([snapshots.md](../runtime/snapshots.md)). Metadata runs do not: guest
  memory, which the snapshot holds, has the same bytes. A used arena extent refuses
  through the call queue's editor refusal (`Queue::preview_refusal`), once
  an editor has adopted the queue; the overlay's refusal is not built.
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
| **R4** Edit arena and block moves (built; live check not run) | the arena, its allocator, `moves`, `edit.arena`; arena capacity for R3's growth | `crates/extensions/hot-tags/src/arena.rs` (new) and the batch, `crates/extensions/agent/src/hot.rs`, `crates/extensions/game/src/hot_glue.rs`, the capacity call in `calls/placement.rs` | allocator: first fit, free, coalesce, full; a move then a path read resolves through the new address; revert puts the pair back and frees; an `at` write inside a moved block is admitted | `edit.arena` on a loaded multiplayer map records the free size (a note in `docs/editor/notes/`); a ninth scenery entry on a block of eight appears and survives a BSP switch | R1, R3 |
| **R5** Editor: geometry and pixels live (built) | `play/raw_sync.rs`; the effect chips; Pending rows; dirty spans from `GeometrySession`; the stroke push | `crates/apps/studio/src/play/raw_sync.rs` (new), `crates/apps/studio/src/play/tag_sync.rs`, `crates/apps/studio/src/play.rs`, `crates/tools/geometry-edit` | fake game: a `mesh.transform` is one batch with `raw` and bounds; undo sends the old bytes; View mode sends nothing; a mismatch leaves the batch pending; a grown edit sends append, `writes`, `evict` in that order; a stroke sends its tiles (`crates/apps/studio/src/play/raw_sync/tests.rs`; each patch's owner and `was` in `crates/tools/geometry-edit/src/session_tests.rs`) | `mesh.transform` through the editor's API on a paired game, `render.frame` with `snap` shows the moved faces; `mirror.pending` is empty after | R1; R2 for the native picture; modeling slices 5 to 7 for pixels and grown sections |
| **R6** Editor: placements live | add, remove and their undo as `world.placement.edit`; the layout-revision rule; Pending text | `crates/apps/studio/src/play/objects.rs`, `crates/apps/studio/src/play/objects`, `crates/apps/studio/src/play/sync.rs` | fake game: add, remove, undo and redo each send one transaction and leave `mirror.pending` empty; a refused shape lands under Reload with the game's reason; a lost reply does not send twice | add and delete a crate in the editor and watch `mirror.objects` follow within a second | R3 |
| **R7** Collision live (built; live check not run) | `edit.collision.stage` and `.commit`, both modes; overlay cache drop; the structure activation; the bots' walls; the editor's send and its collision box | `crates/extensions/agent/src/hot/collision.rs` (new), `crates/extensions/agent/src/sim/calls.rs` (`Call::ReactivateStructure`), `crates/extensions/agent-proto/src/hot.rs`, `crates/extensions/game/src/hot_glue.rs`, `crates/live/bots/src/autonav.rs`, `crates/apps/studio/src/play/collision_sync.rs` (new), `crates/apps/studio/src/app/modeling.rs` | fake game: a commit repoints all pairs at one boundary or none; `sim_collision` over synthetic memory answers from the new blocks after it; freed extents return to the arena; `rebuild` is refused on a map with pathfinding the stage did not replace; the activation is one queued deactivate and activate pair, refused with no loaded and active structure; listeners hear each structure commit once; walls fan every surface; boxes chain, save and clear as Modeling actions | `debug.view collision_geometry` shows the new solid after an `append` commit; after `activate: true`, `sim.state.set` puts a player above it and `sim.expect` finds the player resting on its top | R4; modeling slices 8 and 11 |

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
  kinds by path, and an `at` write must sit inside one loaded tag or one
  arena extent; the features' guest memory is allocated at install
  (`hooks::arena`), and the edit arena maps its own pages as it grows
  (`hot_tags::arena`, tests in `arena.rs`, `batch/tests.rs` and
  `placement/edit/tests.rs`).
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
- **S**: a sound chunk record's runtime word (`+8`, `0xffffffff` in the
  file) is the sound cache's handle, and the cache reloads a chunk whose
  word is `0xffffffff`. Reading the cache's lookup
  (`sound_cache_initialize` `0x2185d0` and its consumers) with
  `h2 disasm` verifies it; R16's live check (replace a footstep's samples,
  the next step plays them) shows it.
- **S**: the engine accepts a block in the arena, for scenario blocks and
  for collision. R4's live check verifies the first, R7's the second.
- **S**: enough physical memory is free for a useful arena, and 2 MiB of
  it left free is enough for the game's own later allocations.
  `edit.arena` on each map (`physical_free`) verifies the first; the
  floor is provisional until then.
- **S**: evicting a section through the guest's unload callback and
  letting the lookup re-read it is safe mid-map. R5's grown check
  verifies it.
- **S**: a structure BSP's tag writes (cluster bounds, records) are sent
  as `{at}` writes at the BSP region's virtual addresses, which are the
  guest's addresses only while that BSP is the loaded one. The editor
  sends geometry only for targets whose tag is in `edit.map`'s `loaded`
  list and always with `was`; whether a BSP the game has not activated is
  absent from that list is not checked. R5's live check on a map with two
  BSPs verifies it.
- **S**: physics' initialize-for-new-structure-BSP callback (`0x1c2690`)
  builds the structure's Havok shape from the collision pairs and the
  MOPP pair as they are when it runs, and a deactivate and activate pair
  mid-game keeps players, vehicles and placed objects as a campaign
  switch does. R7's live check verifies both.
- **S**: writing the placement index into the placement data yields a
  linked object equal to a loaded one for the supported shapes. R3's
  comparison with `map.reload {}` verifies it.

## Not verified

- Every **S** above.
- R13's remaining groups: `weap`, `jpt!`, `vehi`, `bipd`, `bitm` and most of `matg` have no reader address in their definitions, so they stay `not_known`; each needs `h2 disasm` of its reader. `ligh` fields are `frame` for the lifted code; the native light passes read the file and follow only through R10.
- R11 on a running game: a painted lightmap page and a first-person
  weapon texture showing natively; the first-person reload of an edited
  `mode` replacing the model under its key; the lights' and decorators'
  rewrites; the decorators' re-preparation after a `Model` or `Section`
  edit (every `Section` of a BSP re-prepares them, which a long cluster
  edit may make costly).
- The light passes of model parts (`ModelLights`) hold the model load's
  images and are rebuilt only when the models load again after a `Model`
  edit; an `Image` edit rewrites the world's passes only. The lights'
  normalisation cube and attenuation volume, and the decorators'
  lightmap stage (the globals' rasterizer bitmap), are not refreshed.
- A fog, water or layer image that decodes as a linear texture is
  refused by the refresh's decode and keeps its pixels.
- The time from a batch to the picture for a drag on a large cluster, and
  the cost of a world reload after a grown edit.
- Whether a section missing for the frames between evict and page-in is
  visible, and whether drawing the old copy until then is needed.
- Havok contacts on appended collision: resting bodies, vehicles, a biped
  standing where a solid appears, and how odd the gap between rays and
  bodies looks before the activation.
- Bots after a collision change: their walls follow it, their nav mesh
  does not.
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
