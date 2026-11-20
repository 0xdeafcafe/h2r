# Native GPU renderer

Every stage has defaulted on since the all-native A/B on 2026-11-19/20
(below, "The all-native push"). The frames matched D3D and the frame rate
was lower. `H2_FEATURES=impl.render.native=off` is the one switch: it
returns every stage, walk and the scene lists to the D3D path. The
per-stage, per-walk and diagnostic switches the push used are gone. The goal is every frame drawn from GPU-resident data, with the game's recompiled
render front-end left doing only what the GPU path cannot do yet. Evidence
labels as in [feature-engine.md](../engine/feature-engine.md): **Code**
(read in the disassembly, `h2 disasm`), **Test**, **S** (supposed). No
game data: addresses and offsets are research points, everything else is
read from the user's disc at run time.

## Ordered replacement handoff

World and objects use the device reservation below. Held post packets can pass their saved slots and viewport to the same transaction. Geometry
hooks for particles, decals, HUD, gel producers and world layers can register
`Device::add_draw_admission(Arc<dyn DrawAdmission>)`. The device calls admissions
in registration order at the resolved draw, for immediate, inline, indexed and
array geometry. Scope ownership to the current producer and match the complete
state. Returning `None` leaves the draw available to the next admission. A
failed preparation of a claimed draw falls through to the original D3D draw.

For a helper that emits one quad, call
`Device::begin_draw_scope(mem, Arc<dyn DrawAdmission>) -> Result<Option<DrawScope>, D3dError>`
on entry. Keep the handle across the guest call. At return,
`Device::end_draw_scope(mem, scope) -> Result<bool, D3dError>` drains that
call's pending packets and reports whether the replacement committed. Earlier
packets drain before entry. Refusal consumes the candidate, and dropping the
handle disarms it. Nesting supersedes the outer candidate. A persistent
admission is appropriate for an ordered stream of independently owned draws.

```rust
trait DrawAdmission: Send + Sync {
    fn admit(&self, snapshot: &DrawSnapshot<'_>)
        -> Option<(ExternalPlan, Box<dyn ExternalPass>)>;
}

// At a hook outside draw submission, drain the preceding guest commands.
let reservation = device.prepare_external_pass(mem, plan, pass)?;
if let Some(reservation) = reservation {
    // Apply reversible mask edits here. Drop the reservation to cancel.
    reservation.commit();
}
```

`DrawSnapshot::draw` borrows the exact `ResolvedDraw` that D3D can still issue.
It includes vertex declaration, streams, indices, shader inputs (microcode,
constants, defaults, pixel definition and combiner state), raw and translated
render and stage state, texture headers and formats, viewport and scissors.
The snapshot adds raw shader handles, bound texture headers, the sampled-stage mask, bound colour/depth headers and palette headers and entries.
The device replaces only the draw packet. The guest's shader setup and
stencil tail still run, including when adjacent draws have different owners.

`ExternalPlan { stage, view, ranges, slots }` promises a positive number of
geometry or effect ranges on **each** participating copy. `slots` is an indexed
`Vec<Option<ExternalSlot>>`; each populated slot names a guest colour surface
header and whether it will be written. `ExternalPass::writes()` declares writes
to the bound colour and depth/stencil. In `record_ranges(ExternalTarget) -> usize`,
`target.copy` is `Reference` or `Display`, `target.stages` holds the four resident
stage views for a resolved draw, and `target.slots[i]` supplies the live surface's
attachment, sample view, format and texel size for this copy. Record only in
the supplied encoder. A queue submission or write to a live texture would
escape cancellation. A declared sampled header or intermediate surface that
cannot be resolved on one required copy refuses the entire replacement.

Recording takes place during prepare. Partial recordings are discarded.
Commit splices only previously sealed command buffers into the D3D submission
order. Commit has no remaining stage callbacks or target lookups. With interpolated
presentation active, confirmed replacements keep the guest draw for replay.

A confirmed admission on the threaded backend needs a synchronous worker
round trip. `encode_pending` closes the preceding merged D3D pass, and each
physical copy gets a private command buffer. World and object hooks batch a
view into one reservation. A packet holder can batch consecutive native draws
on the same targets in a single `ExternalPlan` and encode their ranges in
guest order. End the batch before any guest-owned draw, target/state change
that the batch does not reproduce, or guest-visible read. A one-draw helper
pays the full admission cost.

At a drained boundary, the held-packet entry is
`Device::prepare_external_bound_at(mem, plan, pass, slots, rect)`. It compares
each saved `Option<B::Surface>` against its slot's current guest header. The
rectangle is `(x, y, width, height)` in guest pixels. Each `ExternalColor`
provides both `guest_size` and this copy's physical `size`. A scaled frame
whose intermediate bloom slot exists only at 1× keeps the entire guest
sequence. With the display copy asleep, the 1× reference can run alone.

`Backend::prepare_external(surfaces, request, textures, &plan, copy, pass)`
returns `Result<Option<Vec<ExternalRecording>>, String>`. Each copy first
calls `ExternalPass::prepare_copy(&ExternalTarget) -> Result<(), &'static str>`.
Only when every copy agrees does the backend call `record_ranges` on private
encoders. Validation scopes include resource lookup, native preparation,
recording and encoder finish. A failed recording discards every copy.
`Backend::commit_external(recording)` takes one sealed copy. The device
reservation also preserves deferred display copies and invalidates guest
colour, depth and alias caches for the declared writes. Code that holds guest
packets should use the device entry for those effects. A private gel producer
or probe can use the default read-only `writes()` while using `colour_only()`
to request a scratch depth attachment.

Use `ExternalTarget::encoder_uploads()` for per-view constants, dynamic rows
and indirect arguments. Its upload arena reserves a fresh source range for
each write and records a copy immediately before the consuming draw. Target
copies and multiple runs may share a submission. The queue completion callback
returns the leased storage to its pool after the last submission using it.
World placement transforms are initialized once at map load.

**Frame counters.** `H2_FRAME_STATS`, its CSV, `profile.counters` and
`profile.capture` use the same stage names. Prefix each suffix below with
`native_<stage>_`, where stage is `world`, `objects`, `transparents`,
`lights`, `post`, `particles`, `decals`, `hud`, `gels`, `fog`, `water` or `ui`.

| Suffix | Measurement |
| --- | --- |
| `examined`, `suppressed` | Guest items considered and items committed. Multi-range passes override `ExternalPass::items(ranges)` to report their logical item count. |
| `refused_ownership`, `refused_resources`, `refused_inputs` | Unowned items, unavailable surfaces or bindings, and rejected stage preflight. |
| `refused_ranges`, `refused_masks`, `refused_validation`, `refused_replay` | Incomplete range count, failed reversible guest mask edit, wgpu validation error, and original draws retained for interpolation. |
| `prepare_ns`, `encode_ns` | Host wall nanoseconds in native selection/resource preparation and private command recording, respectively. |
| `buffers`, `bind_groups`, `upload_bytes`, `pipeline_misses` | Actual creations and bytes charged through the submission arena and counted `gfx::churn` operations during the stage. |
| `cache_hits`, `cache_misses` | Draws whose pipeline and bindings were already resident, and draws that created one. |
| `command_buffers` | Committed recordings with a private command buffer (`record_ranges`). Each also ends the open encoder and adds a render pass. Packets (`record_draws`), encoded commands (`record_encoded`) and bundles add none. |
| `v<V>_<C>_recorded`, `v<V>_<C>_ranges` | Ranges the stage recorded on this copy, including discarded attempts, and ranges committed. |
| `v<V>_invariants`, `v<V>_<C>_invariants`, `v<V>_<C>_refused` | Range-count mismatches across copies, mismatches on this copy, and transactions refused by this copy. Each stage/view/copy mismatch logs once. |

The frame-wide `command_buffers` counter counts every command buffer the
wgpu backend hands to `queue.submit`, next to `submits` and `passes`.
`ordered_writes` counts packet runs whose persistent rows (the world's
view rows) could not go at the front of the submission, because it
already held other bytes for them or a command wrote them in draw order.
Each one copies the rows in order and starts a render pass, so it should
stay near the number of extra views per submission (split screen, two
target copies).

`V` is player view `0` through `3`, or `other`. `C` is `reference` or
`display`. Confirmed passes account for both physical copies and count a
suppressed guest item once. Stage ownership filters count items they reject
before calling the device. Single-stage draw admissions can override
`DrawAdmission::stage() -> Option<timeline::native::Stage>` to meter their
selection time and ownership refusals automatically. New stage implementations use
`gfx::churn::{create_buffer, create_buffer_init, create_bind_group, create_render_pipeline, upload_bytes}`
for resource accounting. Legacy enqueues have no suppression promise.

## Why a new shape

Today a frame on Old Mombasa costs about 25 ms of game-thread time in the
render front-end ([render-loop.md](render-loop.md), "Where the scene
passes' time goes"): the opaque layer 3 alone is 8.26 ms, nearly all of it
per-draw guest work in the shader draw `0x00016b90` (parameter evaluation,
render-state lists, texture binding) before a D3D call is made. Those
calls become push-buffer words that `d3d` decodes and translates back into
wgpu draws on the render thread (decode, then about 6 to 8 ms of encode
and 4 ms of submit).

Stage 7a ported the orchestrators (`render_views`, `render_scene_passes`,
the layer loop, the sorts) and saved nothing: it moved code, not work.
Every Rust port that still calls the pass bodies pays a runner round trip
per call ([layer-draw.md](layer-draw.md), "Hooked calls leave the direct
path"). The native renderer does the opposite: it takes the *data* the
game draws from (tags, at map load) and the *camera* (per frame), draws
it with its own GPU code, and has the game skip the subtree that would
have drawn it. A skipped subtree costs nothing on the game thread, in
`d3d` decode, or in encode.

## Stage plan

| Stage | Draws natively | What the game skips (CPU removed) | Shared with the D3D path |
|---|---|---|---|
| 1. Static world | Structure BSP clusters and instanced geometry, layer 3 (lightmap) | Layer 3's structure entries: their builds, shader draws (`0x0001bbf0`/`0x00016b90`), geometry submission (`0x00041040`'s callbacks `0x0004cbf0`/`0x0004d0b0`) and the push-buffer words; most of the 8.26 ms layer-3 site, the matching `d3d` decode, encode and submit | Colour target and depth/stencil, viewport, the camera |
| 1b. World, all layers | The world's layers 1 (base depth / texaccum), 5, 7 (self-illumination) and 8 (environment map) | The rest of the world's entries in the base layers | Texaccum target (when kept), depth |
| 2. Objects and skinning | Render models with rigid and skinned sections, permutations and LODs | Supported layer-3 draw handlers, object list building and whole-call object setup | Node matrices, object and render-record layouts |
| 3. Transparents and particles | Sorted transparents (`0x0001e370`, 2.58 ms), particle systems, decals, contrails | Persistent native vertex bindings for sorted indexed geometry. Particle and effect handlers still run through D3D. | The guest's sort, resolved per-draw state and the live depth target |
| 4. Lights and shadows | Dynamic lights (layers 10 to 14, `0x00031c80`, 1.88 ms of bookkeeping), shadow maps; E3 lighting becomes a pass ([e3-lighting.md](e3-lighting.md)) | The per-light passes and their gathering | Light list (read) |
| 5. Post and HUD | Fog, bloom, active camouflage, screen effects, HUD and text, then the present | `render_screen_effects` (`0x00013870`: the colour pass, screen effects, `0x0002df00`), `0x0002dd30`, layer 17, the fade `0x00040890`, the UI and HUD `0x00147af1` (see "Stage 5: post and HUD") | Nothing left: the D3D path is only a fallback |

Each stage falls back to D3D for the materials or draws it cannot handle.

### Scene list builders

The scene lists are on by default and go off with the rest of the native
renderer (`impl.render.native=off`). This stage publishes the engine's original
visibility lists and render records. A draw can consume the same lists
through either renderer. The game supplies the camera, clipped cluster
frusta, live datum arrays and current tag definitions.

| Rust publication | Original work bypassed when the inputs resolve |
|---|---|
| Camera and light visibility | `0x00134390` publishes visible BSP clusters and placements. `0x00132220` walks both object reference partitions and the light reference partition. Both start after the portal or sphere cluster pass. |
| Object order and setup | `0x00134240` promotes model-linked entries with the original swaps of all four columns. Rust returns directly from accepted `OBJECT_RENDER_SETUP` calls (`0x00133790`). It calculates the projection, opacity and flags, selects sections and handles residency (`0x0004b5d0`), builds palettes and walks children (`0x00133ab0`), then publishes the visibility bit and AL retry result. |
| Object render records | Whole-call setup uses the same Rust writer as the standalone append hook at `0x001341d0`. It publishes the model arm of `0x0002cbf0`, including attachment and group slots, projected scissors, lighting bytes and subpart allocation. The standalone hook also serves guest setup calls that reach the append after a refusal. |
| Dynamic light order and affected geometry | The pass at `0x0002c03e` calls `0x00031910`, which collects candidates, updates per-view history, ranks lights and fades their weights. Rust replaces this call and its helpers `0x0004a7c0` and `0x0004ac30`. After the per-light visibility pass, Rust applies the caster and receiver flags from `0x00031660` and the optional owner append from `0x00031c20`. |
| Transparent and effect lists | Rust publishes the geometry append at `0x00041381` and the effect and polygon records from `0x0001e4e0` and `0x0001e5e0`. In the pass at `0x0002c19a`, Rust fills the active u16 index interval and skips `TRANSPARENT_SORT` at `0x0001e3b2`. The original comparator's plane rules, partition swaps and short-sort ties determine the order, including entries with a null callback. |

The object walk uses the live salt, original partition order and visit
epoch. It writes the accepted object and placement stamps, light record
stamps, cluster ordinals, visibility bits and masks in guest memory.
The callers still advance the epochs. The light history retains its
candidate mask, removal swaps, protected entries and fade accumulators.
The transparent queue retains the guest's depth calculation, handlers
and payloads. Consumers read the same addresses and counts.

Object setup touches the requested sections in region order before
consulting the previous permutations. It tries the cached LOD without
requesting I/O when selection at the desired LOD fails. It preserves
the guest's per-view permutation bytes, LOD and palette tokens, resource
epochs, miss counts, lighting cache data, and object render stamps. New
palettes use the same transposed node matrices and per-section remaps.
The object walk emits each parent's sections before descending through
its child and sibling links. The no-draw exit updates the low byte of
the original visibility column. The existing caller uses AL to process
a kind-1 retry, including the retry call's different render flags.

Setup stages writes across the entire object tree. A refusal restarts at
the original entry with the incoming registers and memory. New resource
I/O, synchronous waits, request reprioritization, resource relocation,
live light-probe work, animated pose evaluation, and auxiliary model
materials use that path. Portal traversal and dynamic light frustum
setup (`0x000c1d90`) supply the inputs to the Rust gathers. Effect
producers decide when effects are ready to queue.

Frame stats record `native_scene_lists_<stage>_native`, `_fallback` and
the associated `_memory`, `_layout` or `_unsupported` reason. Object
setup also reports `_residency`, `_lighting`, `_palette` and
`_object_model` for the corresponding guest service paths. Counts are
per list, setup or append, depending on the stage.

For timing, compare `impl.render.native=off` with the default, using
`H2_FEATURES=render-timing` in both captures. Include the
`game_view` stage for the main-camera object gather, and sites
`0x0002bd2d`, `0x0002c03e` and `0x0002c19a` for transparent production,
dynamic lights and the sorted dispatch. The Old Mombasa census measured
1.88 ms/frame for the entire dynamic-light bookkeeping site and about
0.01 ms/frame for the two old sort leaves combined. The census does not
isolate the object setup or transparent allocation costs. A net saving
needs a matching A/B. See [render-loop.md](render-loop.md).

### Maps with no native work

The world stands down when its BSP loads yield no drawable native parts.
The lights stand down when the native world is disabled or its BSPs yield
no native lit parts. Fog and water stand down when their translated passes
yield no drawable parts. A BSP-backed stage stays eligible while any BSP
has work or has yet to be checked.

Objects stand down only when the map's tag index contains no object
definitions. Model loads and uploads stay lazy when definitions exist.
An empty map result logs once per stage. The hooks then return at an
atomic flag check until the next map activation or snapshot restore.

## Native scene walks for effects and overlays

The walks are on whenever the native renderer is; `impl.render.native=off`
disables every walk with its stage. The effects walk shares the particles
renderer.

| Stage | Instruction boundary and guest work skipped |
| --- | --- |
| Transparents | At `0x00041020` the saved sorted-loop ordinal resolves through the live u16 order array. The data pointer must match the handler argument. At `0x0004dad0` the standard emitter has a contiguous range in EDI/ESI. Skip the indexed submission and resume at `0x0004dae0`. |
| First person | At the layer-3 call returning to `0x00040136` the model pass can suppress the sole in-layer item before its build. Mixed lists keep the guest sort. At `0x0004dad0`, the ordinary sorted draw stack and the first-person list can supply contiguous ranges in layers 1, 3, 7, 8 and 20. |
| Decals | At `0x00044161` ESI names one record in the callback's linked list. Read its live allocation and quad count, then skip the array submission through `0x00044190`. The guest has already marked duplicates and selected the bitmap and attribute defaults. |
| Particles | At `0x0023ed5c` the guest allocator has succeeded and the emitter inputs are still live. Replace the one quad and skip its attribute writes and geometry publication in the guest. Restore EBP and return with the original stack cleanup. |
| Effects | The four-vertex copy callback's source is available at `0x0004b252`. Read it directly and skip the copy and publication. For contrails, read the completed callback output at `0x0004b28c` and skip publication. Both use the live inline declaration, then return `-1` with the original stack cleanup. |
| UI and HUD | The sorted widget dispatch, HUD and loading callbacks delimit ownership. Known fans and inline rectangles bypass their guest submission as described under “Native UI”. |
| Post | At `0x0002d565` the screen strip's Begin is published. Read the active pass and two corner rows and evaluate them with the call's MXCSR. Supply those vertices to the ordered post transaction and resume at `0x0002d8b2`. |

A direct native draw drains preceding work and resolves the same streams,
program, sampler inputs, depth and colour attachment as the guest issue
path. Native recordings for every participating copy must be sealed
before the hook skips guest instructions. A refused draw returns to the
guest at the same place in the list. The inline paths retain the last
effective register values on commit and restore their old values on
refusal. Post retains the assembled original strips for exact D3D replay
if its complete call fails validation or recording.

Program selection, material evaluation and target changes still run for
indexed geometry and HUD shader quads. Multi-range indexed branches,
unknown effect builders, query draws and unsupported states still use
their original paths. Particle system iteration, property evaluation and
contrail ribbon construction stay in the guest. Glow and convolution
quads still use their existing whole-call recording and replay. The
single-item first-person mask avoids moving a native draw across the
guest's sort when other layer items participate.

### Post colour state and framebuffer aliases

A load-time screen program does not capture the inherited raster state
at issue. The screen strip validator used to reject the whole call when
the live colour state differed, including a blend constant that could
differ without participating in blending. The strip pipeline now uses the
resolved blend, blend constant, colour writes, winding, cull and scissor.
It still requires the compiled alpha test to match, and checks the actual
strip attributes against the plan by bits. Each strip in one pass must
have the same raster state. Other shaders, effects and raster states keep
their original draws when the native pass cannot reproduce them.

A linear X8 alias of the framebuffer samples alpha as one. The native
feedback copy applies that alpha rule on its own scratch texture, as
D3D's framebuffer snapshot does. Slot identity, sampled dimensions and
the original replay path stay in the transaction. Mixed alpha
interpretations of the same framebuffer in one pass are refused.

### Diagnostics and A/B

`H2_FRAME_STATS=1` and the agent's frame counters include
`native_<stage>_walk_examined`, `walk_suppressed`,
`walk_refused_inputs` and `walk_refused_reservation`. For post,
`walk_suppressed` counts guest strip loops skipped. Post's ordinary
`suppressed` and reference/display range counts measure committed GPU
coverage. The effects and first-person ranges have their own stage
columns.

Regenerate the retail image with the updated emitter. The emitter routes
the researched instruction starts through generated entry dispatch.
Keep the same image across A/B runs and vary only the walk switches.
Use the same replay or save and input, then capture matching presents
with the agent API. Include Old Mombasa gameplay and explosions,
first-person weapon changes, a menu with blurring and glyphs, and a
two-view run at 1x and 2x. Compare pixels, native and replay counters,
guest render CPU, D3D encode and render-worker waits. Also capture the
master-off run as the D3D reference.

## Open

- Seams: the drawn world is welded (no T-junctions, shared edges
  bit-identical; `viewer`'s `bsp_cracks`), so no load-time T-junction
  repair is needed. Draw every material's parts from one vertex buffer
  with one position transform, and don't snap to a 1/16 grid at scale, so
  shared edges match bit for bit (high-resolution.md, "Subpixel snapping
  and cracks"). Re-run `bsp_cracks` on the instanced geometry (not covered
  yet) and on the campaign BSPs before relying on it.
- What reads the stencil marks of modes 1 and 2.
- E1 reprojection: a replayed frame must redraw the native pass with its
  warped camera; the interpolating backend records draws, not passes.
- Split screen: one native pass per view, with the view's camera (the
  camera record is per view; the hook runs per view already).
- Which 03a templates have a layer-1 texaccum pass that layer 3 samples
  in screen space; those need the texaccum target bound to the native
  pass, or stage 1b.

## Residual render callers

A runtime census of these callers (by view scope, through
`render.stats`) was used for the inventory below and has been removed.

### Retail call-site inventory

These are counts of reachable sites from the retail image, with
conditional sites included once. The number of runtime invocations
depends on visibility, state caches, materials and which stage
admissions succeed. All-stage runtime counts are pending integration.

| View path | Reachable residual sites | Notes |
|---|---|---|
| Scene kind 1 | 5 direct target selections in the base sequence, 1 direct layer-1 clear, 1 late layer-2 overlay, 1 sky call, 1 visibility issuance call and 1 visibility result reader | Layer begins can select and clear targets too. Sky has an independent model path. Flare tests issue once per eligible flare and poll the three-frame set. |
| Scene kind 2 | The same 5 base target and 1 layer-1 clear sites, and 1 late layer-2 overlay | This path exits after the first transparent site. Sky and the late visibility pair are on the other path. |
| Scene kinds 0, 3, 4, 6, 7 | 4 general layer begins before the late sequence, including 1 layer-2 overlay site, plus the late sky and visibility sites | Layers 1, 3, 5 and 2 use the general begin. The main scene's five explicit target selections are outside this branch. |
| Scene kind 5 | 1 common scene-exit site | The scene leaves after its early setup. The common exit may restore target 0 and depth clip. |
| Alternate view | 1 view setup at `0x0002c863`, 1 UI call at `0x0002c884`, 1 view finish at `0x0002c889` | This is the alternate view record's separate path. Its target and UI draw stream depends on the view. |
| Outside game views | 1 fallback-view call at `0x0022326a`, 2 frame-finish calls after the view loop, 1 present call at `0x00223296` | Load screens, video and menu composition, the final blit and display resource handling are outside the normal scene admission. |

The five base target sites are `0x0002bde2`, `0x0002be83`,
`0x0002bf0e`, `0x0002bf7f` and `0x0002bfe3`. The direct clear is
`0x0002be01`; the clear helper calls `D3DDevice_Clear` at
`0x00014bb2`. Target changes converge on `0x0001cd30`, which calls
the same-size D3D bind at `0x0001cd63` or the viewport-resetting bind at
`0x0001cd6c`. The scene also reaches layer, light, camera and post targets
through their own wrappers. The scene exit `0x00013630` can restore a
target at `0x0001366b`.

The flare writer `0x00020bb0` polls an older result at `0x00020cbb`,
begins the count with inline NV2A methods, draws its callback and ends the
test at `0x00020d0f`. The late reader `0x00020770` polls the two-frame
slot at `0x00020840`. The native GPU counter must preserve sample counts,
readiness and timestamps for both callers. That report is separate from
the optional previous-frame depth grid used for object culling.

The all-stage capture should cover Old Mombasa's cinematic and gameplay,
a main menu, a movie and an alternate view, against the same run with
`impl.render.native=off`. What stays on D3D: unsupported materials,
exceptional handlers, clears, target changes, query reports, menu/video
and final display traffic, and any shader or geometry setup retained by
a stage that replaces only resolved draws.

### Residual geometry admission

With the native renderer on, the residual admission takes the sky model's indexed strip
at `0x0004c976`, and standard layer-2 geometry at the main-scene and
alternate-scene layer sites. Their guest order, transforms, shader
inputs and stencil setup stay in place. The native pass consumes the
resolved packet and records the full range on both live target copies
before the handoff replaces the D3D draw. Other layer-2 handlers and
unknown callers retain their own D3D submission.

### Attachment and visibility entry paths

The native renderer also takes the native entry path for clears,
target binds and visibility finish/results. Their ordering boundary is
the runtime's existing drain. A one-use marker lets these handlers
consume that drain instead of reading the push cursor a second time.
Callbacks, dirty-range processing, deferred destruction and ABI return
writes still run through the common adapter.

A whole-viewport, unmasked clear uses the native attachment load
operations directly, without building a rectangle list. Partial or
channel-masked clears use the existing rectangle renderer. Target
binds among device-owned surfaces skip reference staging while retaining
the surface lookup, write-back order and viewport behavior. Title-owned
surfaces retain the usual reference, parent and destruction rules.

Visibility issuance and results use the existing GPU pixel counter and
report slots. The inline begin still drains before the counted draw.
Finish and result calls retain index validation, pending answers, GPU
timestamps and the normal write order. Counted geometry stays with the
counting renderer. Draw admission only replaces ranges for which a
complete recording can be reserved, including every live target copy.
