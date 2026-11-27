# Agent API: live tag edits, menu methods and the paired editor

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

## `edit` and hot `render`: live tag and shader changes

Host methods answered where the game's host registers them
(`crates/extensions/agent/src/hot.rs`, names in `crates/extensions/agent-proto/src/hot.rs`).
The design, the override directory and the rules are in
[hot-iteration.md](../graphics/hot-iteration.md). The override directory
is `$H2_RENDER_OVERRIDES`, else `render-overrides` in the cache directory.

| Method | Access | Params | Does |
|---|---|---|---|
| `edit.map` | read | `{known_epoch?}` | `{at,name,file,checksum,scenario,epoch,session,bsp}` for the loaded map. File and checksum can be null. `bsp` is the loaded structure BSP's index in the scenario's block (`-1` before one loads, null when unreadable): the one BSP whose region is in guest memory. Session includes `writes` and the refusal reason in `why`. A multiplayer host with an unknown peer count reports `writes:false` and `why:"multiplayer host peer count is unknown"`. |
| `edit.tags` | read | `{group?, name?, limit?}` | the loaded map's tags: `tag`, `group`, `name`, `address`, `size`, `defined` |
| `edit.get` | read | `{tag, group?, path?, epoch?}` | a field by path (`postprocess[0].colors[1].color`), or an element's fields; `tag` is a datum or a name |
| `edit.read` | read | `{epoch,at,len}` | Reads `len` bytes (1–524288) at numeric address `at` inside one loaded tag. Returns `{at,holds_bytes}` with lowercase hex bytes. |
| `edit.batch` | act | `{writes,label?,epoch?,confirm_peers?}` | `{id,applied,changes}` at one boundary. Each batch has one journal entry with its ID, optional label and changed `fields`. |
| `edit.set` | act | `{tag, group?, path, value, epoch?, confirm_peers?}` | writes the field now, journalled; returns the change |
| `edit.changes` | read | none | the current epoch’s journal, including original `was_bytes`, `new_bytes` and current `holds` as hex |
| `edit.revert` | act | `{id?,epoch?,confirm_peers?}` | restores a change or batch, or the whole journal in reverse order. `reverted[]` gives `id,at,tag,path,value,holds_bytes` for each restored field. `dropped[]` gives `id,at,tag,path,why,holds_bytes` for each field left alone. Addresses are numeric. Observed bytes are hex, or null when the field belongs to another map. |
| `render.overrides` | read | none | the override directory, what is loaded and how each fared |
| `render.reload` | act | `{dir?}` | reads the directory again (`dir` moves it) |
| `render.programs` | read | `{vertex?, limit?}` | the translated programs, by id |
| `render.dump` | act | `{program?, to?}` | writes translated WGSL, the start of an override |
| `render.param` | act | none, `{name}`, `{name, value}` | lists, reads, sets (`value: null` resets) a parameter |
| `render.toggle` | act | none, `{name, on}` | lists, sets (`on: null` resets) a toggle |

All three edit writes check the session at the game boundary. Co-op, clients,
matchmade or other-hosted Live games, film playback and unknown sessions
refuse writes. A System Link or Live custom game host with remote peers requires `confirm_peers` equal to the current total peer
count. A client can pass `known_epoch:null` to `edit.map` to include the loaded
tag datums in `loaded`, then send its cached epoch to receive the table only
after a cut. An epoch pins a write or field read to the map reported by `edit.map`.
A BSP switch revalidates the journal against surviving tag allocations.
A map load or state restore clears it. During reverse revert, a field changed
by the game protects its whole byte range. Older overlapping fields stay as
the game holds them and appear in `dropped`.

`edit.map.file` and the telemetry map identity follow the **source path
recorded by the active cache slot**. The host resolves a direct `T:` or
`W:` path through the guest's mounted drive link. A DVD slot can carry a
map with a different header name. An installed package with that header
name cannot shadow the DVD source. For an editor copy, `file` names the
immutable host generation for the current activation. When the slot
cannot identify an ordinary map, the host looks up the cache name. Ask again after a
reload before opening a path from an earlier reply.
After the first successful file binding, external Live sign-in gives the
reason “An editor map was loaded. Restart the game to sign in to Xbox
Live.” This applies even after switching maps.

Each batch write takes either `{tag,group?,path,value,was?}` with typed values,
or `{at,value,was?}` with hex bytes. An address is a u32 or a `0x` string.
The entire address range must fit one loaded tag, or one block moved into
the edit arena ([Block moves](#block-moves)). Overlapping writes are
refused. If the game already holds the requested value the write succeeds.
If it holds neither value, error 8 (`MISMATCH`) returns `data.fields` with
`at`, `tag`, `path`, `was`, `holds` and `holds_bytes`. The entire batch and
journal stay untouched.

```sh
h2 agent edit tags weap
h2 agent edit get <tag name> <path>
h2 agent render toggle <name> off
```

Tag names come from the map file the game loaded ([`world`](agent-api-world.md#world-what-is-in-the-level)).

### Raw edits

Section vertices and indices and bitmap pixels live in the map files' raw
blocks, not in the tags, and a structure BSP that is not loaded lives in
its region of the map file alone. These methods write them live, through the raw
overlay that both the guest's page-ins and the native loaders read. The
design is [realtime-edits.md](../editor/realtime-edits.md). The methods
are in `crates/extensions/agent/src/hot.rs`, the editor half in
`crates/extensions/hot-tags/src/raw.rs`, the overlay in
`crates/runtime/vfs/src/map_override/overlay.rs` and the game's glue in
`crates/extensions/game/src/hot_glue.rs`.

| Method | Access | Params | Result |
|---|---|---|---|
| `edit.batch` | act | adds `raw: [{owner, offset, bytes, was?}]` and `evict: [owner]`; `writes` may be left out when either is given | adds `raw: [{owner, offset, resident, refresh}]` and `evicted: [owner]` |
| `edit.raw.append` | act | `{file?, bytes, align?, epoch?, confirm_peers?}` | `{id, pointer, len}` |
| `edit.stage.put` | act | `{stage?, bytes}` | `{stage, len}` |
| `edit.overlay` | read | none | `{entries, bytes, appended, appended_bytes, metadata, metadata_bytes, files, owners}`: raw runs, appended extents and metadata runs (copies of tag writes; docs/editor/realtime-edits.md, "The edit overlay") |

`owner` is `{section: {tag, kind, index}}`, with `kind` one of `cluster`,
`instance_def` and `mode`, `{pixels: {tag, image, lod}}`, with `lod`
0 to 2, `{sound: {tag, range, permutation, chunk}}`, one sample chunk
of a `snd!` permutation, each index counted within the sound
(docs/editor/realtime-edits.md, "Sound samples"), or `{bsp: {index}}`, the
scenario's structure BSP `index`. `tag` is a datum or a name. `offset`
counts from the start of the owner's raw block, LOD chain, chunk or BSP
region (the region's virtual base, its file offset in the scenario's
descriptor), and the bytes must fit inside it. A `bsp` owner is refused
while it is the loaded BSP (`edit.map`'s `bsp`): its region is guest
memory then, written with `at`. `bytes`
is base64, or `{stage, at?, len?}` naming bytes `edit.stage.put` holds.
`was` is base64 bytes or `{hash}`, their FNV-1a 64 as a number or a hex
string.

A batch checks every field and raw `was` before its first write. A raw
write is checked against the file view (the map file through the overlay)
and, when the owner's section is paged in, against the resident copy. With
no `was`, the resident copy must equal the file view. A write the game
already holds succeeds. Otherwise error 8 (`MISMATCH`) returns
`data.fields` and `data.raw`, the second with `owner`, `offset`,
`holds_hash`, `resident_holds_hash` and `why`, and nothing is written.

At the boundary the batch writes the tag bytes, then each raw entry into
the overlay and into the resident copy when there is one, then the
evictions. `resident` says whether a resident copy was written; pixels,
sections not paged in and BSPs not loaded take the overlay alone, which
their next page-in or the switch to the BSP reads; sound chunks always take
the overlay alone. An eviction clears the section's loaded flag, cache
handle and runtime section block, or the sound chunk's runtime word, so the
guest's next lookup pages it in from the file view; pixels cannot be
evicted. `refresh` names the native refresh queued on
`render_bridge::native_edits`: `section` (with `grown` when the batch
evicts it), `model` or `image`, and is `null` for a sound chunk and for a
`bsp` owner, whose switch loads the world from the overlaid file.

`edit.raw.append` reserves an extent past the end of `file` (`local`,
`mainmenu`, `shared` or `single_player_shared`; `local` by default),
aligned to `align` (a power of two, 16 by default). It is invisible until
a tag points at it: `pointer` is the raw pointer to write into a block
info, with the file selector in its top two bits.

`edit.changes` lists raw acts under `raw`, by journal id. `edit.revert`
undoes them newest first: the overlay runs as they were before the write,
the resident bytes where they still hold the written ones, a second
eviction of an evicted section so the guest pages in the restored block,
and appended extents dropped. Reverted raw entries appear in `reverted[]`
with `raw`, `evict` or `append`.

A stage belongs to the connection that made it. `stage` in
`edit.stage.put` appends to an existing one. A request that names a stage
uses it up when it applies. At most 256 MiB is staged at once.

The overlay lives for one map. A map change drops it, the raw journal and
every stage at the first tick of the new map, and a play-copy reload drops
the overlay when it binds. The guest reads a map's raw data from the cache
slot file the map was copied into, so the overlay is served on those slot
files and never on the copy's source. A slot copy, a cache template and a
play copy never hold an overlay byte. Raw writes refuse with `UNAVAILABLE`
when the loaded map's cache slot is unknown or no VFS is mounted. The
session rules are those of the other edit writes.

```sh
h2 agent edit.batch '{"epoch":E,"raw":[{"owner":{"section":{"tag":"<sbsp>","kind":"cluster","index":0}},"offset":4096,"bytes":"AACAPw=="}]}'
h2 agent edit.overlay
h2 agent edit.revert '{"id":N}'
```

### Block moves

A tag block is a `(count, address)` pair and a run of elements with
nothing reserved after it. A block that must hold more elements than the
map gave it moves into the **edit arena**: guest memory the host maps in
the HLE reservation as the arena grows. The design is
[realtime-edits.md](../editor/realtime-edits.md), "Four write primitives". The
allocator is `crates/extensions/hot-tags/src/arena.rs`, the batch half
`crates/extensions/hot-tags/src/batch.rs`, the methods
`crates/extensions/agent/src/hot.rs` and the pages
`crates/extensions/game/src/hot_glue.rs`.

| Method | Access | Params | Result |
|---|---|---|---|
| `edit.batch` | act | adds `moves: [{root, stride, count, capacity?, bytes, was}]`; `writes` may be left out when it is given | adds `moves: [{root, tag, path, count, address, capacity}]` |
| `edit.arena` | read | none | `{base, len, mapped, used, largest_free, physical_free, physical_floor, moves: [{tag, name, root, address, capacity, stride, len}]}` |

`root` names the pair: `{tag, group?, path}` with a path that ends at a
block field (`postprocess[0].colors`) or at an 8-byte raw field
(`+0x20:u32[2]`), or `{at}`, the pair's address, inside a loaded tag or
inside a block an earlier move put in the arena. `stride` is the bytes of
one element and must equal a defined block's element size. `bytes` holds
the `count` new elements (base64 or `{stage, at?, len?}`), `capacity`
(default `count`) the elements of room. `was` is `{count, address}`, the
pair before the move; `address` is a number or a `0x` string.

The batch checks each move with its fields and raw entries: a root that
holds another pair than `was` answers error 8 (`MISMATCH`) with
`data.moves: [{root, tag, path, was, holds}]`. A write over a move's root
in the same batch is refused. Then it allocates every move's extent
before the first write. An arena without room answers `REFUSED` (11) with
`data.why`, `data.root`, `data.need` (bytes) and `data.largest_free`, and
nothing is written: the edit falls back to a reload. At the boundary the
block's bytes go into the extent, the room past `count` is zeroed, and
the root takes the new count and address, after the batch's field writes
and before its raw entries. Paths and addresses resolve against the
blocks as they are before the batch, so an edit inside a moved block goes
in its `bytes` or in a later batch.

A moved block belongs to its tag: an `{at}` write and `edit.read` inside
it are admitted as inside the tag, and a path through the pair reads and
writes it. `edit.changes` lists moves under `moves`. `edit.revert` puts
the old pair back and frees the extent, after reverting the batch's field
writes; a root pointed elsewhere since is left alone and its extent kept.
A field write inside a block that has moved back appears in `dropped`.

`edit.arena` reports the window (`base`, `len`, 4080 KiB), the bytes
mapped and held by extents, the largest extent a move could have now, the
free physical memory, and the floor the arena leaves free for the game
(`physical_floor`, 2 MiB). `world.placement.edit` grows placement blocks
into the same arena ([Placement edits](agent-api-sim.md#placement-edits)), and its extents
are listed too, booked to the scenario. The arena lives for one map: the
first tick of a new map frees every extent and unmaps its pages; a BSP
switch keeps it. While it holds an extent, machine snapshots, rewind and
film keyframes are refused, as for an unlinked placement preview.

```sh
h2 agent edit.arena
h2 agent edit.batch '{"epoch":E,"moves":[{"root":{"tag":"<shad>","path":"postprocess[0].colors"},"stride":S,"count":3,"capacity":8,"bytes":"<base64>","was":{"count":2,"address":"0x..."}}]}'
```

### Collision edits

The lifted game reads a collision BSP in place through its eight
`(count, address)` pairs, so a live collision edit stages the new blocks
in the edit arena and then repoints the pairs at one tick boundary. The
design is [realtime-edits.md](../editor/realtime-edits.md), "Collision";
the methods are `crates/extensions/agent/src/hot/collision.rs`.

| Method | Access | Params | Result |
|---|---|---|---|
| `edit.collision.stage` | act | `{target, blocks: {name: {count, bytes}}, leaves?: {count, bytes}, mopp?, epoch, confirm_peers?}` | `{stage, bytes, blocks: {name: {count, address}}, arena: {used, largest_free}, activation}` |
| `edit.collision.commit` | act | `{stage, mode: "append" \| "rebuild", activate?, was: {name: {count, address}}, epoch, confirm_peers?}` | `{at, stage, mode, pairs: {name: {count, address}}, activated, freed: [address]}` |

`target` is `{structure: tag}` (the structure's level collision,
`sbsp+0x14`), `{instance_def: {tag, index}}` (an instanced geometry
definition's BSP, at the definition's `+0x70`) or `{coll: {tag, region,
permutation, bsp?}}` (`bsp` defaults to 0); `tag` is a datum or a name.
`blocks` holds all eight blocks by the field names of
`tags::collision::Bsp` (`nodes`, `planes`, `leaves`, `refs`, `nodes_2d`,
`surfaces`, `edges`, `vertices`), each `count` elements laid out as
`Bsp::encode_blocks` lays them (`collision_build::coll::stage_blocks`).
For a structure, `leaves` is the per-leaf block (`sbsp+0x2c`, 8 bytes a
leaf, one per BSP leaf) and `mopp` the bytes of the `sbsp+0x1e0` block;
either left out keeps the structure's own, and a structure whose per-leaf
block would no longer have one element per leaf is refused. Bytes are
base64 or `{stage, at?, len?}` naming `edit.stage.put` bytes. A stage of
`pathfinding` is refused: its blocks hold pointers the host would have to
relocate.

The stage decodes the blocks and runs `Bsp::validate` against the target's
collision material count, then writes each nonempty block into its own
arena extent. Nothing points at them, so the game reads nothing new. A
full arena answers `REFUSED` (11) with `data.block`, `data.need` and
`data.largest_free`, and holds nothing. A second stage of the same target
before a commit replaces the first and frees its extents. `activation`
says whether this game can run the BSP activation a commit asks for. A
stage belongs to its connection and lives until a commit uses it or the
map changes.

The commit names in `was` every pair it repoints: the eight block names,
`leaf_clusters` for the per-leaf block and `mopp`, each `{count,
address}` before the commit (`address` a number or a `0x` string). A pair
that holds another answers error 8 (`MISMATCH`) with `data.pairs: [{name,
root, was, holds}]` and nothing is written. Every pair is checked before
the first write, and all are written at the same boundary. `append` is
the mode for added solids: every existing plane, leaf, surface, edge,
vertex and per-leaf element keeps its index, so a block with fewer
elements than the one it replaces is refused. `rebuild` changes indices:
it always activates, and is refused on a structure with pathfinding,
which names surfaces by index.

With `activate: true`, and always for `rebuild`, the commit asks the game
to activate the loaded structure BSP again and answers after it has run,
with `activated: true`. The activation is the second half of a BSP
switch, which `BSP_SWITCH` itself skips for the BSP already loaded and
active: at the next tick, through the agent's guest calls
(`Call::ReactivateStructure`, `crates/extensions/agent/src/sim/calls.rs`),
`STRUCTURE_BSP_DEACTIVATE` (`0x137da0`) then `STRUCTURE_BSP_ACTIVATE`
(`0x137d40`), so every game system rebuilds its per-BSP state (physics
its Havok world over the structure, objects their locations) and the
BSP's tag data, with the new pairs, is not loaded again. A game with no
activation (no agent guest calls) refuses these commits (`REFUSED`,
nothing written); one whose activation does not start or fails (no
structure loaded and active) puts the old pairs back, frees the stage,
and answers `REFUSED` with the reason. The arena extents the old pairs named are freed after the next
activation, listed in `freed`; until then Havok's shape may still read
them. A `coll` BSP's old extents are kept until the map changes, since a
live body's shape points at them until the object is made again
(`world.object.respawn`).

The lifted BSP walks follow the pairs on every query, so rays, sweeps
and point location answer from the new blocks on the next tick, and
`sim_collision`'s page watches over the pairs start a new generation. A
structure commit hands the new BSP to `debug.view`'s `collision_geometry`
and `invisible_barriers`, which are built from the map file and would
otherwise show the old level. A commit on the active structure also
rebuilds the bots' line-of-sight walls from the new BSP on a thread of
their own (`bots::autonav::collision_edited`); their nav mesh keeps the
old shape until the map loads again. The journal does not hold a commit: `edit.revert` leaves it, and the
editor undoes it with the inverse stage and commit. Sessions are those
of the other edit writes; with `confirm_peers` on a host the change stays
local, as tag pokes do.

```sh
h2 agent edit.collision.stage '{"epoch":E,"target":{"structure":"<sbsp>"},"blocks":{"nodes":{"count":N,"bytes":"<base64>"},...},"leaves":{"count":L,"bytes":"<base64>"},"mopp":"<base64>"}'
h2 agent edit.collision.commit '{"epoch":E,"stage":1,"mode":"append","activate":true,"was":{"nodes":{"count":N0,"address":"0x..."},...}}'
```

## Menu methods: settings, perf overlay, cheats

The in-game debug menu is a client of this API ([debug-menu.md](debug-menu.md));
the methods it added beside the agent's own are in
`crates/extensions/debug-menu/src/sources/mod.rs` (`method`).

| Method | Access | Params | Does |
|---|---|---|---|
| `settings.list` | read | `{prefix?}` | settings (`settings.toml`) and how each applies (`crates/extensions/agent/src/menu.rs`) |
| `settings.set` | act | `{key, value}` | saves the value and applies it as the setting applies; `UNAVAILABLE` when something else set it for this run |
| `perf.overlay` | act | `{}` reads, `{on}` sets | the perf overlay (the watermark) |
| `cheat.list` | read | `{}` | `{refused, pending, cheats}`: each cheat's `name`, `label`, `about`, `kind` (`bool`, or `choice` with `choices`), `value`, `refused` (why the session refuses it) and `pending` (why it does nothing yet: on while no campaign or multiplayer game runs; it takes hold when one starts); the top-level `pending` says the same with no game running (`crates/extensions/game/src/cheats.rs`) |
| `cheat.set` | act | `{name, value}` | sets one, for example `{"name":"hide_hud","value":true}` or `{"name":"game_speed","value":0.1}`; answers with `cheat.list`'s answer. Cheats: `god`, `infinite_ammo`, `bottomless_clip`, `game_speed` (0.1, 0.25, 0.5, 1, 1.5, 2, 4), `fly`, `noclip`, `hide_hud`, `ai_off` ([debug-menu.md](debug-menu.md#cheats)) |

### World debug views

| Method | Access | Params | Result |
|---|---|---|---|
| `debug.views` | read | none or `{}` | `{views, range, trace_ticks}` |
| `debug.view` | act | one of `{name, on}`, `{range}` or `{trace_ticks}` | the updated listing |

Each row has `name`, `label`, `on` and an optional `why`. The names are
`geometry_edges`, `trigger_volumes`, `kill_volumes`, `invisible_barriers`,
`collision_geometry`, `hitboxes` and `traces`. For example,
`h2 agent debug.view '{"name":"hitboxes","on":true}'` enables the same
switch as the window's Debug views section. Range accepts whole world
units from 1 to 512 (default 48). Trace lifetime accepts 1 to 300 guest
ticks (default 30).

A host without a window lists the views with a reason. Enabling a refused
view returns `UNAVAILABLE`. An enabled row can report a cache loading or
decoding issue. The socketless debug menu refreshes the same map cache
between execution slices.

The window registers the debug views as a world-line source before boot.
Changing a switch, range or lifetime affects subsequent genuine images.
A captured image keeps its geometry, including the object poses and
traces sampled for it. The overlay projects the sample with the image's
split-screen cameras and draws hidden edges at full strength. The source
and capture code live in `crates/extensions/game/src/debug_views.rs` and
`crates/extensions/agent/src/debug_views/mod.rs`. See
[Drawing world-space lines over the frame](#drawing-world-space-lines-over-the-frame).

Traces observe the named `SIM_COLLISION_QUERY` entry and its return while
enabled. A true hit draws the world position from the result record. The
query's ABI and result layout are in [Collision rewrite, §5.4](../engine/sim-rewrite-collision.md).
See [Debug views](debug-menu.md#debug-views) for the data sources, draw caps
and limits. The wire types are in `crates/extensions/agent-proto/src/debug.rs`.

## `editor`: a paired editor

`h2 play` shows a plate in the top-right corner while an editor holds a
link lease. The first line is “Editor · viewing”, “Editor · editing live”
or “Editor · edits blocked”. Optional `text` appears on the second line.
The plate expires on the wall clock, including while the game is paused.
The shell draws it after the watermark and leaves room for the banner
(`crates/apps/cli/src/play_cmd.rs`, `crates/apps/app/src/shell.rs`).
While the debug menu is open, the plate's lines show inside the menu
instead ([debug-menu.md](debug-menu.md#layout)).

| Method | Access | Params | Result |
|---|---|---|---|
| `editor.link` | read | `{editor, mode, text?, ttl_ms?}` or `{clear: true}` | `{shown, why?, expires_ms}` |
| `editor.focus` | act | none | `{focused, why?}` |
| `editor.marks` | read | `{marks, ttl_ms?}` | `{held, drawn, why?, refused}` |

`editor` is a string. `mode` is `view`, `edit` or `blocked`. Each call
replaces the lease, with a `ttl_ms` of 500 to 10000 (default 3000).
`text` accepts up to 64 characters. Invalid arguments return
`INVALID_PARAMS`. `expires_ms` gives the milliseconds remaining when the
host accepts the lease, or zero when the plate is cleared or refused.
Clearing returns `shown: false`. The link is available in every session
kind (`crates/extensions/agent-proto/src/editor.rs`,
`crates/extensions/agent/src/editor.rs`).

`editor.focus` returns `focused: true` when it queues a request for the
window. The event loop restores a minimized window and asks the window
system to give it focus (`crates/apps/app/src/shell.rs`). On `h2 serve`,
both methods are listed and answer with `shown: false` or `focused: false`
and `why: "this host has no window"` (`crates/apps/boot/src/lib.rs`).

`editor.marks` replaces the mark set for 500 to 10000 ms (default 3000).
An empty set clears it. Each of the up to 256 entries contains exactly
one geometry key: `{object: "<eight hex digits>"}`,
`{box: {origin, forward, up, size}}` or `{point: [x, y, z]}`. Box size
gives the full lengths along forward, left and up from the origin.
An entry takes `color: [r, g, b]` with components from 0 to 1,
optional `style` (`selected` or `candidate`), optional `label`, and
optional reserved `xray`. An invalid entry leaves the previous set intact.
`held` counts accepted entries. Stale object handles and object entries
without the agent tick observer are refused by index in
`refused: [{index, why}]`. The tick observer retires handles when their
objects go away and invalidates older images' mark samples. A live
stream epoch change clears the entire set
(`crates/extensions/agent/src/editor/scene.rs`,
`crates/extensions/agent/src/exec.rs`).

Each genuine guest image captured while a mark lease is active carries
the bounds sampled from the guest state that renders it, along with
each view's camera and aspect factor.
This applies to serial rendering and repeated
scanout of the same image. The window projects marks over the finished
frame. Hidden edges show at full strength, with the same styling as
visible edges. The candidate style uses dashes. `xray` has no effect
(`crates/rewrites/render-bridge/src/scene_step.rs`,
`crates/platform/d3d/src/present_camera.rs`,
`crates/apps/app/src/frame.rs`, `crates/apps/app/src/overlay/marks.rs`).

`drawn: true` reports a current genuine projection. The window presents
a genuine image with at least one shape from the current set and a
usable camera. Individual edges still depend on clipping. `drawn: false`
gives a reason such as a headless host, native rendering or scene
capture being off, a loading picture, a raw submission, an unusable
world camera, or waiting for a render with the set. Marks are also
drawn on in-between frames (60 and 120 fps), from the source image's
samples if it carries the current lease's marks, with the camera the
frame drew each view with
([high-framerate.md](../game/high-framerate.md), "World marks on made frames";
`crates/platform/d3d/src/present_camera.rs`,
`crates/apps/cli/src/play_cmd.rs`, `crates/apps/app/src/shell.rs`).

A shell pause or paused Theater playback blocks the guest loop, so
editor methods wait to be serviced. An agent pause continues to service
them, and a newly sent mark appears only with the next genuine render.
The window still expires the plate and marks on wall time
(`crates/apps/app/src/lifecycle.rs`,
`crates/extensions/agent/src/exec/poll_theater.rs`,
`crates/extensions/agent/src/exec.rs`, `crates/apps/app/src/shell.rs`).

### Content previews from the editor

A paired editor's `pair.preview` and `pair.audio` are clients of this API
on a second connection to the paired game's socket
([editor agent API](../editor/agent-api.md#previewing-content-in-the-paired-game);
`crates/apps/studio/src/api/preview/run.rs`). They add no game method. A
preview calls `session.methods`, then `sim.give` or `sim.spawn` (or
`cheat.set hide_hud`), `session.wait`, `world.object`, `world.query`,
`pogo.state`, `render.scene`, `ui.audit`, `ui.stack`, `ui.screen`,
`render.capture` or `host.screenshot`, and `audio.voices` and
`audio.loops`, each where the game lists it. A game that leaves one of
these out answers a preview without that read. The host writes keep their
own rules: `sim.give` and `sim.spawn` are refused in a co-op session and
on a multiplayer client.

### Drawing world-space lines over the frame

A windowed host registers a `WorldLineSource` with
`Host::add_world_line_source` before boot. Each source provides an
immutable sample of world-space edges, with an anchor, colour, dash
flag, optional label and style for each group. A sample can live as long as
its image or carry its own lease. Only the editor sample contributes
to `editor.marks` status. The superbounce game type is another source, and
its bounce view a third; their samples stay image-lived (`crates/extensions/game/src/superbounce_mode.rs`,
`crates/extensions/game/src/superbounce_view.rs`;
`crates/extensions/game/src/run_opts.rs`,
`crates/extensions/game/src/world_lines.rs`,
`crates/platform/gfx/src/world_lines.rs`).

A group's `LineStyle` (`crates/platform/gfx/src/world_lines.rs`) sets
how it is drawn; the default is a thin opaque line. A style has:

- a core `width` in pixels at 1080p, scaled by the output height;
- an `alpha`;
- an optional `glow`, a wider low-opacity halo around the core;
- an optional `pulse`, a period and depth read from the present
  time, so brightness breathes between full and `1 - depth`;
- an optional distance `fade`, full opacity up to its start and none
  from its end, in world units from the camera;
- optional `chevrons`, small arrowheads lying flat in the world along
  each edge and pointing along a world direction. They flow along
  the direction when their speed is not zero, moved by the window's
  clock at each present;
- `stroke`, on by default: off leaves only the chevrons, ribbon and
  label along the edges, with no line;
- an optional floor `ribbon`, a translucent strip of a world width
  along each edge, clipped at the near plane;
- a `label_face`: the 8x8 debug font (`Debug`, the default), or the
  disc's HUD font with its button glyphs (`Hud`), once a source has
  published one with `gfx::world_lines::set_hud_font`. A `Hud` label is
  drawn in the 8x8 font while none is published.

A sample may also give marks in the image's HUD space
(`ImageMarks::hud`, `gfx::world_lines::HudMark`): paths, filled convex
polygons and text, in guest pixels of the 640 by 480 image with the
host's HUD fit already applied. The window scales them with the image,
as the guest's own HUD is scaled, after the world lines. The superbounce
bounce view draws its tracker blips and map this way
(`crates/extensions/game/src/superbounce_view.rs`).

The window draws every edge as an anti-aliased screen-space quad, with the
core, halo and dashes in the fragment shader, and the ribbons, arrowheads
and label pixels as triangles in the same vertex buffer. A HUD-font glyph
is a textured quad sampling the font's atlas, bound once per draw. All of
it is one pipeline and one draw per frame
(`crates/apps/app/src/overlay/marks.rs`). A label's opacity is the
style's `alpha` times its fade and pulse.

A sample is captured once a guest frame, so motion within it belongs
in the style (pulse, chevron speed), which the window runs at every
present; geometry built from a capture-time clock steps at the tick
rate at 60 and 120 fps.

Genuine guest images captured with samples carry their own per-view
cameras and aspect factors. Repeated scanout keeps the same samples
and cameras. An in-between frame draws the samples of the newest
genuine image, or of the one before while a blended replay is under
half way, with each view's camera moved onto the one the frame drew
the world with ([high-framerate.md](../game/high-framerate.md), "World
marks on made frames"). The window draws
all visible samples together in a 2D pass over the finished frame.
There is no depth test, so hidden edges draw at full strength
(`crates/platform/d3d/src/present_camera.rs`,
`crates/apps/app/src/overlay/marks.rs`, `crates/apps/app/src/shell.rs`).

An inactive producer costs an atomic poll. When all producers are
inactive, there is no capture, guest read for the overlay, or sample
allocation. The inline collection holds two additional samples
before growing. Active sources read only the supplied guest view
and shared host data
(`crates/extensions/game/src/world_lines.rs`,
`crates/rewrites/render-bridge/src/scene_step.rs`).
