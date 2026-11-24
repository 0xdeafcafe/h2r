# Native GPU renderer

Status: Built. Every stage draws in the default configuration.
Code: `crates/rewrites/native-render/src` (wgpu passes and loaders), `crates/rewrites/render-bridge/src/native_*.rs` (the hooks that hand guest draws to them), `crates/platform/d3d/src/backend.rs` and `crates/platform/d3d/src/device.rs` (admission). Tests: `crates/rewrites/render-bridge/tests/native_defaults.rs`, `crates/hooking/hooks/src/switches.rs`, and the per-module tests in `crates/rewrites/native-render`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

The native renderer draws the frame from GPU-resident data: the static world, objects, transparents, lights, effects, fog, post and the HUD. It does not
replace the recompiled front-end wholesale. The guest's render code still
runs and still fills the push buffer. At each draw, a hook offers the draw to
a stage; a stage that can reproduce it records a wgpu packet into the same
frame as the D3D draws and the device drops the original draw. Anything a
stage refuses stays a D3D draw. Every stage must give the same pixels on
the reference copy and the display copy of the frame, and a refusal must
leave the guest exactly as if the hook had not run. Evidence labels as in
[feature-engine.md](../engine/feature-engine.md): **Code**, **Test**, **S**.
Addresses and offsets are research points; no game data is in the repo.

This page describes the default stage-by-stage renderer. On the opted-in
GPU path, the scene sink retains repeatable typed stages and resolved guest
draws at their original slots. The display worker can present a complete
game view from that seal, and the capture worker can draw the same inputs
into private targets. The ownership rules and the gate for changing the
default are in [FrameScene slice 6](framescene.md#slice-6-the-renderer-owns-game-view-frames).

## How a frame flows

1. **Scene lists** ([Scene list builders](#scene-list-builders)). The
   engine's visibility lists and render records are built in Rust from the
   live camera, datum arrays and tags, so the guest's gather and setup calls
   are skipped when the inputs resolve.
2. **Walks.** Stage hooks sit at researched instruction boundaries in the
   guest's layer loops ([Native scene walks](#native-scene-walks-for-effects-and-overlays)).
   Each reads the live list entry and either skips guest work for it or
   leaves it.
3. **Admission.** A draw that reaches `d3d` is shown to every registered
   `d3d::DrawAdmission` ([Admission](#admission)). The first that returns a
   plan and a pass wins. Preparation records every participating copy; commit
   splices the sealed command buffers into the D3D submission order.
4. **Fallback.** A refusal, a failed validation, a material or state the
   stage cannot reproduce, or a stood-down stage leaves the original D3D draw.
   Clears, target changes, query reports, menu and video traffic and the
   final display stay on D3D ([Residual render callers](#residual-render-callers)).

`impl.render.native=off` returns every stage, walk and scene list to the D3D
path together. `crates/hooking/hooks/src/switches.rs` (`NATIVE`, `native_render_on`)
reads it, looked up exactly: the `impl.render` group does not reach it.
`the_native_master_switch_gates_every_stage_and_walk` checks that every
stage gate follows it. There are no per-stage switches.

## Stages

| Stage | Page | Modules | What it replaces |
|---|---|---|---|
| Stage 1: the static world (and 1b, all layers) | [native-world.md](native-world.md) | `native-render`: `world`, `load`, `gpu`, `cull`, `skip`, `camera`, `world_layers`, `world_layers_gpu`, `world_ranges`. `feature`: `native_world`, `native_world_layers`, `native_map_work` | The structure BSP's layer-3 entries and the other base layers: builds, shader draws, submission |
| Stage 2: objects and skinning | [native-objects.md](native-objects.md) | `native-render`: `model`, `model_load`, `model_gpu`, `models`, `objects`, `object_walk`, `object_lighting`, `object_nodes`, `object_program`, `object_program_gpu`, `object_state`, `object_streams`. `feature`: `native_objects`, `native_first_person` | Layer-3 object draws and the guest's object setup |
| Stage 3: transparents | [native-transparents.md](native-transparents.md) | `native-render`: `transparent`, `transparent_streams`. `feature`: `native_transparents` | Indexed submission of the guest's sorted geometry |
| Particles, decals, effects | [native-particles.md](native-particles.md), [native-decals-research.md](native-decals-research.md) | `native-render`: `particles`, `particle_gpu`, `decal`, `decal_gpu`, `effect_geometry`, `effect_immediate`, `glow`. `feature`: `native_particles`, `native_decals`, `native_effects` | Quad, decal and contrail submission |
| Stage 4: lights and shadows | [native-lights.md](native-lights.md), [native-shadows.md](native-shadows.md), [native-shadow-layers.md](native-shadow-layers.md), [native-gels.md](native-gels.md) | `native-render`: `lights`, `light_gpu`, `light_list`, `light_look`, `light_scene`, `shadows`, `shadow_gpu`, `shadow_look`, `shadow_scene`, `shadow_filter_gpu`, `gels`, `gels_gpu`. `feature`: `native_lights`, `native_shadows`, `native_gels` | The per-light passes, shadow maps and shader gels |
| Stage 4b: water, fog and camouflage | [native-water-fog.md](native-water-fog.md) | `native-render`: `fog`, `fog_gpu`, `water_gpu`, `object_program_gpu`. `feature`: `native_fog`, `native_water`, `native_layers` | Layer 16 fog, layer 6 water edge blend on clusters and rigid placements, sorted late geometry ranges, the indexed water composite, layers 18 and 19 camouflage and layer 21 hologram |
| Stage 5: post and HUD | [native-post-hud.md](native-post-hud.md) | `native-render`: `post`, `screen_gpu`, `screen_look`, `flares`, `hud`, `ui_gpu`. `render-bridge`: `native_post`, `native_flares`, `native_hud` | Screen effects, bloom, lens-flare colour fans, the HUD and UI quads |
| Scene list builders | this page | `feature`: `native_scene_lists` | The engine's visibility gathers, object setup and render records |

Shared modules: `native-render`: `program`, `look`, `pass_state`, `instances`. Sections that code comments cite and on the stage pages: "Object lighting" and "Object programs" in [native-objects.md](native-objects.md); "Skip only the replaced work" in [native-world.md](native-world.md). "What a world pass reads" is a subsection of "Next: matching the look" in the same page.

Stage 1: the static world, Stage 2: objects and skinning, Stage 3:
  transparents, Stage 4: lights and shadows, Stage 4b: water, fog and
  camouflage, Stage 5: post and HUD are the table rows above.
## Map resource lifetime

`render_bridge::native_map_work::reset` invalidates the prior generation and
detaches its BSP, object, light-model, viewmodel, screen-effect and indexed
draw caches before requesting the next BSP. A same-map BSP switch or revert
also detaches the old BSP and its indexed and packed draw caches through
`reload_bsp`. Object and screen data for the map remain available. The packed
transparent binding cache holds up to 4,096 bindings and 16 MiB of payload
(`game::native_transparents_glue::Cache`). A queued draw can retain its own resources
until it finishes. A late loader can publish only to its own generation
and mailbox.

The native map loader workers serialize their bulk decode and upload work
through `game::native_map::begin_load`. They wait for queued retirement on
the worker thread. First-person loading opens the map for a batch of model
requests and releases its map readers and decoded images when the batch
ends. Material images share their backing allocation across passes. Object
and shader readers share ordinary tag metadata within a model load. Sun
caster and receiver lookup holds weak references to the owning map state.

Memory probes and loader scopes are listed in
[Explain memory growth](../runtime/agent-api.md#explain-memory-growth).
The play-run measurements are in
[Map-load memory](notes/2026-11-24-map-load-memory.md).

## Admission

World and objects use the device reservation below. Held post packets can pass their saved slots and viewport to the same transaction. Geometry
hooks for particles, decals, HUD, gel producers and world layers can register
`Device::add_draw_admission(Arc<dyn DrawAdmission>)`. The device calls admissions
in registration order at the resolved draw, for immediate, inline, indexed and
array geometry. Scope ownership to the current producer and match the complete
state. Returning `None` leaves the draw available to the next admission. A
failed preparation of a claimed draw falls through to the original D3D draw.

The decal callback at `0x00044040` owns list order, duplicate stamps,
bitmap selection and attribute defaults. Each array draw enters
`crates/extensions/game/src/native_decals_glue.rs` once through
`DrawAdmission`. An admitted packet uses the live depth comparison and
write state, the resolved W clip and polygon offset, and the live sampler's
border addressing. Reference and display copies select their
own vertex constants, UI transform and scissors. A refusal draws the
same ordered packet through D3D.

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
order. Commit has no remaining stage callbacks or target lookups.

A confirmed admission on the threaded backend needs a synchronous worker
round trip. `encode_pending` closes the preceding merged D3D pass, and each
physical copy gets a private command buffer. World and object hooks batch a
view into one reservation. When the render thread takes scene stages
(`Device::queues_scene_stages`), the world, objects, fog and water queue
one instead (`Device::reserve_scene_stage`): no round trip, and their
preparation and recording run on the render thread
([framescene.md](framescene.md), "Slice 2" and "Slice 5"). A draw whose
pass retains its resolved D3D packet (`ExternalPass::resolved_fallback`)
and makes the same colour and depth writes as that packet is queued too
(`Backend::queue_external_draw`): the render thread records the pass or
draws the packet in its queue slot. A native producer
(`Device::try_native_*_draw`) commits at the queue, so the guest skips its
own emission without waiting; the particle, effect, single-draw UI and
transparent producers work this way ([render-thread.md](render-thread.md), "Keeping
guest-visible semantics exact"). A packet holder can batch consecutive native draws
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
World placement transforms and layer-3 lighting implementations are resolved
at map load. The accumulation texture sampled by combined materials is
resolved on each target copy at the draw. See [the static world](native-world.md).

## Frame counters

`H2_FRAME_STATS`, its CSV, `profile.counters` and
`profile.capture` use the same stage names. Prefix each suffix below with
`native_<stage>_`, where stage is `world`, `objects`, `transparents`,
`lights`, `post`, `particles`, `decals`, `hud`, `gels`, `fog`, `water` or `ui`.

| Suffix | Measurement |
| --- | --- |
| `examined`, `suppressed` | Guest items considered and items committed. Multi-range passes override `ExternalPass::items(ranges)` to report their logical item count. |
| `refused_ownership`, `refused_resources`, `refused_inputs` | Unowned items, unavailable surfaces or bindings, and rejected stage preflight. |
| `refused_ranges`, `refused_masks`, `refused_validation`, `refused_replay` | Incomplete range count, failed reversible guest mask edit, wgpu validation error, and original draws retained for interpolation. |
| `prepare_ns`, `encode_ns` | Host wall nanoseconds in native selection/resource preparation on the game thread and private command recording, respectively. |
| `render_prepare_ns` | The same preparation on the render thread (`timeline::native::mark_render_thread`): scene stages, and passes the threaded backend prepares on its worker. |
| `extract_ns` | Game-thread time a scene stage spends deciding what the guest skips: its guest reads and the checks that can refuse. |
| `scene_stages`, `scene_dropped` | Scene stages queued to the render thread, and those it could not record after the guest skipped their items (`d3d::scene_stage`; the stage then reserves on the game thread). |
| `buffers`, `bind_groups`, `upload_bytes`, `pipeline_misses` | Actual creations and bytes charged through the submission arena and counted `gfx::churn` operations during the stage. |
| `cache_hits`, `cache_misses` | Draws whose pipeline and bindings were already resident, and draws that created one. |
| `command_buffers` | Committed recordings with a private command buffer (`record_ranges`). Each also ends the open encoder and adds a render pass. Packets (`record_draws`), encoded commands (`record_encoded`) and bundles add none. |
| `v<V>_<C>_recorded`, `v<V>_<C>_ranges` | Ranges the stage recorded on this copy, including discarded attempts, and ranges committed. |
| `v<V>_invariants`, `v<V>_<C>_invariants`, `v<V>_<C>_refused` | Range-count mismatches across copies, mismatches on this copy, and transactions refused by this copy. Each stage/view/copy mismatch logs once. |

`native_shader_animation_{layers,objects,layer3}_{drawn,refused}` count
material passes with animated constants
([shader-instances.md](../formats/shader-instances.md#animated-constants))
per path: the structure layer planner's items, the object passes' draws
(layer 3 and the world layers) and the world layer-3 selection's
materials. `drawn` counts passes planned with their registers sampled at
the render clock; `refused` counts those the sampler refused, which stay
on the D3D path.

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

## Scene list builders

The scene lists go off with the rest of the native renderer. This stage publishes the engine's original
visibility lists and render records. A draw can consume the same lists
through either renderer. The game supplies the camera, clipped cluster
frusta, live datum arrays and current tag definitions.

| Rust publication | Original work bypassed when the inputs resolve |
|---|---|
| Camera and light visibility | `0x00134390` publishes visible BSP clusters and placements. `0x00132220` walks both object reference partitions and the light reference partition. Both start after the portal or sphere cluster pass. |
| Object order and setup | `0x00134240` promotes model-linked entries with the original swaps of all four columns. Rust returns directly from accepted `OBJECT_RENDER_SETUP` calls (`0x00133790`). It calculates the projection, opacity and flags, selects sections and handles residency (`0x0004b5d0`), builds palettes and walks children (`0x00133ab0`), then publishes the visibility bit and AL retry result. |
| Object render records | Whole-call setup uses the same Rust writer as the standalone append hook at `0x001341d0`. It publishes the model arm of `0x0002cbf0`, including attachment and group slots, projected scissors, lighting bytes and subpart allocation. The standalone hook also serves guest setup calls that reach the append after a refusal. |
| Dynamic light order and affected geometry | The pass at `0x0002c03e` calls `0x00031910`, which collects candidates, updates per-view history, ranks lights and fades their weights. Rust replaces this call and its helpers `0x0004a7c0` and `0x0004ac30`. After the per-light visibility pass, Rust applies the caster and receiver flags from `0x00031660` and the optional owner append from `0x00031c20`. The three are `engine::render::light_gather` ([native-light-gather.md](native-light-gather.md)); the seams here call them. |
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
setup (`0x000c1d90`) supply the inputs to the Rust gathers. The main
view's traversal and object set also run in Rust beside the guest's and
are compared with them ([native-visibility.md](native-visibility.md)).
Effect producers decide when effects are ready to queue.

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

The world stands down when its BSP loads yield no drawable native parts.
The lights stand down when the native world is disabled or its BSPs yield
no native lit parts. Fog and water stand down when their translated passes
yield no drawable parts. A BSP-backed stage stays eligible while any BSP
has work or has yet to be checked.

Objects stand down only when the map's tag index contains no object
definitions. Model loads and uploads stay lazy when definitions exist.
An empty map result logs once per stage. The hooks then return at an
atomic flag check until the next map activation or snapshot restore.

The structure's layer-3 pass accepts an explicit One/Zero/Add blend as
opaque. Layer-3 object and first-person draws also accept that combination
when the live device supplies its inherited fields. Scene object packets
carry the resolved stencil reference. Object templates can leave blend and
depth writes to the live state check. They take render and
sampler bindings driven by the shader instance's scalar input. The model
loader retains the effective state writes for the device-state checks.

Structure, scene layer-3 objects, decorators, water and fog can issue a
pass with all colour writes masked even when its blend enable or factors
are inherited. Depth writes and alpha testing still use the pass state.
Layers 6 and 16 check the stencil enable word at issue. When stencil is
off, unread comparison or operation words leave those layers eligible.

Layer 5 accepts a PRT declaration with the compiled registers and compatible
fetches. A three-component packed byte, short or normalized short can feed
a four-component input by supplying the encoded one in W. A shader that
forces W to one also accepts a narrower input with the same numeric type.

## Native scene walks for effects and overlays

The walks are on whenever the native renderer is. The effects walk shares the particles renderer.

| Stage | Instruction boundary and guest work skipped |
| --- | --- |
| Transparents | At `0x00041020` the saved sorted-loop ordinal resolves through the live u16 order array. The data pointer must match the handler argument. At `0x0004dad0` the standard emitter has a contiguous range in EDI/ESI. The subrange branches at `0x0004db40`, `0x0004dbdc` and `0x0004dc3a` supply topology, count and indices on the stack. The resolved indexed draw also covers an object whose lighting owner is unavailable. The hook checks the live handler, shader and part arguments. Each admitted range skips one indexed call and resumes at its return address. A refusal runs that call in sorted order. |
| First person | At the layer-3 call returning to `0x00040136` the model pass can suppress the sole in-layer item before its build. Mixed lists keep the guest sort and request uploads for their record models across layers 1, 2, 3, 7, 8 and 20. A part can use a compiled pass when a sibling material fails. At `0x0004dad0`, the ordinary sorted draw stack and the first-person list supply both full parts and sorted subranges in layers 1, 2, 3, 7, 8 and 20. |
| Particles | At `0x0023ed5c` the guest allocator has succeeded and the emitter inputs are still live. Replace the one quad and skip its attribute writes and geometry publication in the guest. Restore EBP and return with the original stack cleanup. |
| Effects | The four-vertex copy callback's source is available at `0x0004b252`. Read it directly and skip the copy and publication. For contrails, read the completed callback output at `0x0004b28c` and skip publication. Both use the live inline declaration, then return `-1` with the original stack cleanup. |
| UI and HUD | The sorted widget dispatch, HUD and loading callbacks delimit ownership. The HUD, UI and loading census rows are partial. The overlay takes supported widget fans and inline screen quads with their bound textures. Shared quads inside the loading callback count as loading draws. Refused draws follow the ordered UI packet or guest path. |
| Post | At `0x0002d565` the screen strip's Begin is published. Read the active pass and two corner rows and evaluate them with the call's MXCSR. Each sampled framebuffer stage reads the alpha interpretation selected by its binding. Glow quads take their blend, blend constant and channel mask from the resolved packets. Supply the strip vertices to the ordered post transaction and resume at `0x0002d8b2`. |

A direct native draw drains preceding work and resolves the same streams,
program, sampler inputs, depth and colour attachment as the guest issue
path. Native recordings for every participating copy must be sealed
before the hook skips guest instructions. A refused draw returns to the
guest at the same place in the list. The inline paths retain the last
effective register values on commit and restore their old values on
refusal. Post retains the assembled original strips for exact D3D replay
if its complete call fails validation or recording.

`world_layers_gpu::IndexedPass` translates each distinct resolved shader
and creates its GPU module on a compile worker. The frame requests the
pipeline for the live vertex layout, render state and attachment formats.
Until the pipeline has crossed a guest swap, the ordered draw takes its
original D3D path. A cold variant consumes no native geometry upload.

Program selection, material evaluation and target changes still run in the
guest for indexed geometry and HUD shader quads. Multi-range indexed
branches, unknown effect builders, query draws and unsupported states keep
their original paths. Particle iteration, property evaluation and contrail
ribbon construction stay in the guest.

### Post colour state and framebuffer aliases

A load-time screen program does not capture the inherited raster state
at issue, so the strip pipeline uses the resolved blend, blend constant, colour writes, winding, cull and scissor.
It still requires the compiled alpha test to match, and checks the actual
strip attributes against the plan by bits. Each strip in one pass must
have the same raster state. Other shaders, effects and raster states keep
their original draws when the native pass cannot reproduce them.
The replacement uses one depth scale across its pinned targets. A change
in scale between held draws replays the call, including its side-target writes.
A call that only writes side targets can run without a framebuffer slot when
its stages and convolutions are independent of framebuffer pixels and size.

A linear X8 alias of the framebuffer samples alpha as one. The native
feedback copy applies that alpha rule on its own scratch texture, as
D3D's framebuffer snapshot does. Slot identity, sampled dimensions and
the original replay path stay in the transaction. Mixed alpha
interpretations of the same framebuffer in one pass are refused.

## Checking a frame

`H2_FRAME_STATS=1` and the agent's frame counters carry the
`native_<stage>_walk_examined`, `walk_suppressed`, `walk_refused_inputs` and
`walk_refused_reservation` counters; for post, `walk_suppressed` counts
guest strip loops skipped.

1. Use the agent API (`h2 agent`, [agent-api.md](../runtime/agent-api.md)):
   run the same replay or save and input with the default and with
   `H2_FEATURES=impl.render.native=off`, on the same image.
2. Capture matching presents and compare pixels; compare the native and
   replay counters, guest render CPU, D3D encode and render-worker waits.
3. Cover Old Mombasa gameplay and explosions, first-person weapon changes, a
   menu with blur and glyphs, a movie, an alternate view, and a two-view run
   at 1x and 2x.

Numbers from the all-native A/B are in
[notes/native-ab.md](notes/native-ab.md).

## Residual render callers

What stays on D3D: unsupported materials, exceptional handlers, clears,
target changes, query reports, menu and video traffic, final display, and
any shader or geometry setup a stage keeps when it replaces only the
resolved draws. The reachable sites in the retail image (conditional sites
counted once; runtime counts depend on visibility and admissions):

| View path | Residual sites |
|---|---|
| Scene kind 1 | 5 direct target selections, 1 layer-1 clear, 1 late layer-2 overlay, 1 sky call, 1 visibility issuance and 1 result reader |
| Scene kind 2 | the same 5 target sites, the layer-1 clear and the layer-2 overlay; exits after the first transparent site |
| Scene kinds 0, 3, 4, 6, 7 | 4 general layer begins (one is the layer-2 overlay), then the late sky and visibility sites |
| Scene kind 5 | 1 common exit; it may restore target 0 and the depth clip |
| Alternate view | view setup `0x0002c863`, UI call `0x0002c884`, view finish `0x0002c889` |
| Outside game views | fallback-view call `0x0022326a`, 2 frame-finish calls, present `0x00223296` |

The frame tail at `0x00013b90` conditionally copies the view target and composites it.
The bridge scopes each helper by its caller and drains pending draws at entry and
return. Its four-vertex pushed fans use the native screen-space pass with the
resolved shader, textures, viewport and target. Refused draws use the original
packet. State setup, optional surface readback and `Swap` run on the guest path.

The five base target sites are `0x0002bde2`, `0x0002be83`, `0x0002bf0e`,
`0x0002bf7f` and `0x0002bfe3`. The direct clear is `0x0002be01`; the clear
helper calls `D3DDevice_Clear` at `0x00014bb2`. Target changes converge on
`0x0001cd30`, which binds at `0x0001cd63` (same size) or `0x0001cd6c`
(resets the viewport). The scene exit `0x00013630` can restore a target at
`0x0001366b`.

The flare writer `0x00020bb0` polls an older result at `0x00020cbb`, begins
the count with inline NV2A methods, draws its callback and ends the test at
`0x00020d0f`. The late reader `0x00020770` polls the two-frame slot at
`0x00020840`. The GPU counter must preserve sample counts, readiness and
timestamps for both. That report is separate from the previous-frame depth
grid used for object culling.

**Lens-flare colour draws.** At each researched End call the bridge checks the saved caller and admits one fan. The guest supplies the projected corners, visibility fade, shader state and target order. The native pass uses the resolved stream and bindings for that draw, including the alternating filter targets. A failed preparation leaves the original packet available. There is no separate scene-list extraction. The [retail trace](notes/2026-11-24-lens-flares.md) identifies the six colour draw sites and the visibility callback.

With a flare in view, sample `h2 agent render.scene` twice. An increase in `counters.scene_submitted_flares_reference_draws` or `counters.scene_submitted_flares_display_draws` counts committed native colour fans on that copy. One flare can contribute several fans. Private captures also contribute, so take the live samples during ordinary gameplay. The original GPU query supplies visibility feedback.

**Alternate scene.** The layer planner draws admitted structure and object items in layers 1, 5 and 2. Layer 3 uses the BSP mask reservation and the sorted object walk with an alternate view key and the current render camera. First-person calls use their own camera and models. View setup, target selection, composition and finish still run in guest order.

**Object shadows.** With `impl.render.native`, the caster call identifies its object and fade from the ranked row in its own stack frame. A matching row admits the object to the existing shadow path. Layer 10 draws the mask and layer 11 applies it to admitted receivers. The guest sets up the targets and draws unsupported geometry. See [native-shadow-layers.md](native-shadow-layers.md).

**Residual geometry.** Layer 2's items at the main scene's call
(`0x0002c05b`), the alternate view's (`0x0002bda3`) and the first person's (`0x00040284`) draw from the scene's
plan as layers 1, 5, 7, 8 and 20 do (`frame-scene.layers`,
`layer_pass::OVERLAY_LAYER`; [framescene.md](framescene.md), slice 4); its
outcomes are `layer_plan_2_*`, `layer_plan_alt2_*` and `layer_plan_fp2_*` in `render.scene`.
The standard layer-2 geometry the plan leaves at the main-scene and alternate-scene sites is admitted with its resolved inputs.
The pass records the full range on both live target copies before the device
drops the D3D draw. Other layer-2 handlers and unknown callers stay on D3D.

**Sky.** At the indexed strip call in the sky model renderer (`0x0004c976`),
`scene_extract::layout::sky::SkyItem` resolves the scenario sky for this view,
its model section, part, shader and pass ordinal. It reads the section's rigid
node or the region's remapped palette from the current view's node block, and
reconstructs position and UV decompression from the model bounds and section flags.
`IndexedPass::for_sky` checks those rows against the draw's vertex constants
and supplies them to the vertex program. The material uses the program,
textures and other evaluated constants bound at issue. The scene sets stencil
mode 2 before the sky call at `0x0002c0bc` and restores mode 0 afterwards.
The native strip requires the mode-2 state and a stencil attachment, and
replaces stencil on passing fragments. Replay retains the sky pass beside its
original strip and records it against the replay's own attachments and textures.
If extraction, admission or recording fails, the original strip draws in order.

**Clears, binds and visibility.** A whole-viewport, unmasked clear uses an
attachment load. For up to eight explicit colour rectangles, the native
planner clips their union to the viewport and target. A full union can use
a load, and a masked clear writes only its selected channels. Depth and
stencil tiles keep their individual scissors because the depth surface may
extend beyond the colour target. Longer lists use the linear rectangle path.

Device-owned targets skip reference staging. Title-owned surfaces reuse
transaction storage while reading current guest reference counts, parent
links and GPU lock stamps on each bind. Final releases queue the original
resource destructor. Both bind forms retain their viewport behaviour.
Visibility issuance and results
use the GPU pixel counter and report slots; the inline begin still drains
before the counted draw, and finish and result calls keep index validation,
pending answers, timestamps and write order. A one-use marker lets these
handlers consume the runtime's drain rather than read the push cursor again.
Callbacks, dirty ranges, deferred destruction and ABI return writes run
through the common adapter.

## Not verified

- Seams: the drawn world is welded (a removed tool, `bsp_cracks`, checked
  shared edges), so no load-time T-junction repair is needed. Shared
  edges must also match bit for bit on instanced geometry and the campaign
  BSPs, which it was not run over
  ([high-resolution.md](high-resolution.md), "Subpixel snapping and cracks").
- What reads the stencil marks of modes 1 and 2.
- Display frames between scenes (framescene.md, section 7): typed
  stages other than the indexed world, object and transparent passes are
  drawn at their tick's camera (`render.stats`' `display_rate.held_stages`).
- Split screen: one native pass per view with the view's camera.
- All-stage runtime counts of the residual callers
  ([retire-d3d.md](retire-d3d.md), "Measurement plan").

## Related

[framescene.md](framescene.md) (the forward design),
[retire-d3d.md](retire-d3d.md) (retiring the D3D path),
[render-loop.md](render-loop.md) (the retail front-end and where its time
goes), [high-resolution.md](high-resolution.md),
[notes/native-ab.md](notes/native-ab.md), [switches.md](../runtime/switches.md).
