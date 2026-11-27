# Native world: the other layers and the BSP scene walk

Status: Built. Part of [native-world.md](native-world.md) (stage 1b).
Code: `crates/rewrites/native-render/src/{world_layers,world_layers_gpu,world_ranges,layer_items}.rs`, `crates/rewrites/render-bridge/src/{native_layers,cut}.rs`, `crates/rewrites/scene-extract/src/layout/world_layers.rs`. Counters: `render.stats` and `render.scene` ([agent-api.md](../runtime/agent-api.md)).

The world's other layers draw natively, and the guest's BSP scene walk and sorted handler run in Rust. Layers 1, 3, 5, 7, 8 and 20 and layer 2 draw from the scene's plan, and the guest builds no entry for them.

| Layers | Slot | Producer |
|---|---|---|
| 1, 3, 5, 8, 20 | 1, opaque base (`ViewSlot::OpaqueBase`) | `native_world` (3), `native_layers` (1, 5, 8, 20) |
| 7 | 3, after the object shadows (`ShadowsAndLayer7`) | `native_layers` |
| 2 | 6 (`Layer2`), with its stencil bracket and non-standard handlers | `native_layers` |

## The world draws only from the scene

The world layers are cut per layer (`render_bridge::cut::LAYER_CUTS`), because the layer draw `0x00044550` serves every layer and no whole-function hook applies. Each draws only from the scene, in the slot the traced order gives it (`native_render::layer_items::world_slot`).

The observer before the layer draw clears the layer's bit on every geometry item the native pass does not draw, as well as on those it does (`layer_items::clear_scene_layer`). The guest builds no entry for any of them and none reaches the device. There is no order rule: the pass draws its items in the list's order and no kept guest draw is left to precede or follow them.

| Counter | Counts |
|---|---|
| `scene_dropped_world_refused` | a structure item the pass does not draw (a named drop) |
| `scene_dropped_world_unreadable` | an item whose render record or list does not read |
| `scene_dropped` | both of the above, on the world stage |
| `layer_plan_<layer>_refused_*` | the plan's per-layer causes |

Layer 3's objects stay the sorted walk's (`native_objects`), and the bits go back at the next layer draw. The first person's layers and the sorted handlers' 18, 19 and 21 keep their own refusals until their cuts. The resolved-scene arm (`Host::use_resolved_scene`) draws from inside the guest's entries, so it still reaches the opaque handler hooks' own refusals.

## Native opaque layers

The main scene draws ordinary BSP clusters and placements in sorted order in layers 1, 3, 5, 7 and 8. The default front end takes these entries at the standard geometry handler (`0x0004d0b0`). Native objects and entries left with the guest can still use the native world layers' indexed admission. Both paths preserve the original subpart ranges, topology, live colour and depth attachments and per-record stencil state.

| Layer | Rule |
|---|---|
| 1 | Replaces a draw only while target 1's attachment header is bound. Texture inputs must resolve to views the program can sample. The two-copy reservation records the entire indexed range on every live target copy before replacing the D3D submission. Colour and depth/stencil writes use the original attachments and feed the usual RTT and depth tracking. Alternate views and special handlers keep their own D3D setup. |
| 1, stencil shader | A shader can move an immediate `v10` (mask `0x0400`) to diffuse colour. For ordinary projected 2D fetches the translator accepts an absent immediate when its only output is diffuse and neither the general nor the final combiner reads it. Position and consumed colour still require a supplied value. |
| 3 | Uses the texture view from the resolved D3D draw. It pins target 1's colour attachment and resolves the engine texture 3 header through the RTT path. All three views must hold the same GPU texture on each live copy, the guest dimensions must agree and the texture must use linear texel addressing. This works with either owner of layer 1, because both write the same attachment. |

The sorted BSP path prepares the vertex program for each classification and the stage binding recipe for each shader, LOD, layer, pass and implementation when the BSP loads. Section preparation reads the live section's declaration, bitmap residency and palette, index windows and target identities. Render-target samples resolve against their current contents in submission order. The menu's unlightmapped implementation uses its own declaration and program. A part can draw without a lighting vector when its selected implementation has its required inputs.

The ordered indexed cache in `world_layers_gpu.rs` holds up to 4,096 resident section windows and charges their CPU geometry and GPU buffers against a 128 MiB budget. A periodic frame sweep releases inactive windows, and a BSP change clears the section cache. A draw that exceeds the residency budget uses the transient upload path.

## Native BSP scene walk

The walk goes off with the rest of the native renderer.

| Guest boundary | Work done in Rust | State handed to the next stage |
|---|---|---|
| `WORLD_CLUSTER_RECORDS` and `WORLD_PLACEMENT_RECORDS`, from the main camera gather, and a light's in the light frame body ([native-light-gather.md](native-light-gather.md), L3) | Walk the current visibility lists in their original order. In a light's kind only the record cursor advances; the main view's count is left. Read projection depth, pixel scale, cluster bounds and placement spheres. Compute the screen-size byte, placement fade and fog flags. | Publish the same record indices, lightmap implementation, placement matrix pointer, subpart bitmap allocations and resident resource stamps. A cold resource can enqueue I/O. A mixed walk falls back before publication and retries each resident row at `WORLD_VISIBILITY_FLAGS`. The guest handles each cold row in place. The following object gather sees both record counters. |
| `WORLD_PART_MATERIAL`, first part at the main scene loop in `0x44ac0` | Examine every visible part in one BSP record. Compute its material visibility, subpart visibility, shader LOD and layer mask. An eligible record appends its items in guest order, bypassing the remaining material queries, `GEOMETRY_SUBMIT`, its LOD queries and item appends. | The ordinary shader, argument, callbacks, selector, LOD, mask and aggregate list mask remain available to every later layer. The per-part centre and clipping plane live in caller scratch, and only the transparent producer exports them. A visible transparent producer, shader override or unknown material keeps the complete section on the guest path. |
| `LAYER_BUILD` and `WORLD_GEOMETRY_BUILD` | Emit the standard BSP sort slots for opaque, water and fog entries. An unsupported callback runs with the original caller frame and can mutate the list. | Rejected slots retain their writes, including undefined words inherited by later candidates. The original sort still orders ties and layer-5 entries. Objects and custom handlers keep their place. |
| `LAYER_ENTRIES`, after layer setup | When the world, water or fog renderer has committed its reservation and every active item has its layer bit clear, return the empty dispatch result immediately. | The GPU reservation already covers the claimed ranges. One remaining bit keeps the dispatcher. Layer setup and end still establish the shared device state. No handler, shader, stencil switch or indexed draw would run in the empty dispatcher. |
| `RENDER_STATES` and `WORLD_PART_EMIT`, inside an opaque BSP draw | Use the render-state port through the first necessary guest call. Elide repeated simple states when the shadow matches and the push header is unique. Use the one-range emitter when visible subparts tile the part's indices. | Changed states and shared push headers still reach the device. The emitter flushes textures and emits the same indices and topology. A masked strip needs exactly one subpart. The placement constants, shader cache, lightmap binding and stencil tail still execute. |

### Sorted handler and device handoff

The sorted hook matches the standard builder and draw callback, the scene layer, the handler arguments and the caller's current sort slot and counter. A native run ends at the first object, custom callback, excluded slot or refused section. Each accepted entry reads its own implementation, classification, record and visible subparts. Triangle lists coalesce contiguous visible index ranges, and a masked strip restarts at each subpart. The native run returns with ESI on the last consumed slot, so the caller performs its normal final increment and can dispatch the next guest draw.

The run's `handoff` applies material and device transitions in draw order:

- The portable entry holds the outgoing shader and device cache words in a bounded writeback buffer. Native reads through any alias see the most recent write.
- Before a real guest call, a fault or the return to the outer loop, the memory guard publishes the final dirty bytes. D3D write watches see logical writes at store time. Resource retains, releases and push callbacks still execute in device order.
- The shader cache tracks the previous and current shader and implementation, dirty parameter banks, live texture headers and sizes, sampler stages and the full pixel-shader definition. An implementation switch consumes the outgoing restore list before publishing the new states.
- Component writes preserve untouched vector lanes and PS fields. Frame inputs, animation and record constants are evaluated against the current view and guest floating point control words.
- A lit placement updates both offsets in each selected lightmap stream descriptor. Its palette, shader, declaration, stream and texture changes reach the D3D device in their original order, including device references and callbacks when a resource loses its last reference.
- A missing vertex input can suppress emission after the binder has already changed state. Stencil, fade and transient target operations bracket the actual draw.

The next native or guest entry therefore starts with the outgoing device and RAM caches already installed. Applying only the last part's bindings would lose writes to earlier descriptors and intermediate resource releases.

The per-range submission uses the prepared geometry and validated texture and program hints. Stale or dynamic resources still use the ordered D3D resolver. A preflight refusal keeps the original sorted handler live. A late refusal before the first range can issue the indexed draws under the state already applied. A device error after emission follows the fault path so a submitted prefix cannot be drawn twice.

### Fade

The fade the display draws a record with is the placement's, rescaled to the display's pixel count.

| Detail setting | Fade |
|---|---|
| Highest display | every placement fade solid |
| A multiplier | the placement's fade recomputed at that many output pixels per guest pixel (`world_walk::placement_display_alpha`) |
| Retail detail | each record's byte ([occlusion-culling.md](occlusion-culling.md), "Projected size and level of detail") |

Where the fade lands depends on the copies (`FadePlan`):

- Single-copy rendering has the guest's draws render the display, so the fade replaces the byte the guest's `SetStipple` takes.
- Two-copy rendering (a reference copy at guest resolution plus the display copy; always so for a network, film or lockstep run, which includes Theater's autorecord) leaves the guest the record's retail byte, so the reference copy and every guest readback stay retail. The display copy gets its pattern alone: `OpaqueDraw::display_stipple` sets `HwState::display_stipple` for that draw, and the pixel uniforms take it on a target larger than the guest's (`HwState::stipple_rows`).
- A draw that falls back to the guest's indexed emitter takes the retail pattern on both copies.

`render.scene`'s `scene_world_stipple_*` counters count the records entered with a fade and how the display resolved them. In two-copy rendering `solid` counts the display copy's solid draws.

### Counters

The A/B method for this walk is in [notes/native-ab.md](notes/native-ab.md).

| Frame column | Counts |
|---|---|
| `native_world_walk_sections` | published render records |
| `native_world_sections_prepared` | sections appended in Rust |
| `native_world_builds_skipped` | guest build callbacks replaced in Rust |
| `native_world_entry_passes_skipped` | whole entry dispatches bypassed |
| `native_world_state_calls_skipped` | redundant Simple state calls |
| `native_world_opaque_prereserved_draws` | BSP parts pre-reserved |
| `native_world_opaque_guest_indexed_draws` | ordinary world ranges that reach the indexed call after guest handler setup, including ranges later accepted by the native GPU backend |
| `native_world_opaque_handlers_taken` | ordinary BSP geometry handlers the native path consumed (the skipped guest handler count) |
| `native_world_opaque_handler_draws` | all ranges those handlers submitted (a masked strip can emit several) |
| `native_world_opaque_handlers_indexed`, `native_world_opaque_handler_indexed_draws` | handlers and ranges that needed the indexed backend for emission |
| `native_world_opaque_handlers_fallback` | whole geometry handlers left to the guest, once each |
| `native_world_opaque_taken_sections`, `native_world_opaque_fallback_sections` | distinct clusters and placements for consumed and refused handlers; a section with mixed materials can appear in both; once per BSP, view and outcome across all sorted layers |
| `native_world_opaque_gpu_draws` | prepared packets recorded on the required GPU copies |
| `native_world_opaque_backend_draws` | prepared packets the ordinary backend drew after a recording refusal |

The `_sections` columns of the indexed counters count each cluster or placement once per view and BSP on each path, and a section with parts on both paths counts in both. The indexed counters require the world layer hooks. Prepared opaque submissions and their device preflight are timed in the D3D phase.

`native_world_opaque_materials` in `H2_FRAME_STATS_CSV` breaks down each frame by full shader datum: the reason and handler count for indexed emission and for whole-handler refusal, `refused_sections` per shader and reason when the render record identifies a valid cluster or placement, and the backend refusal reason. The frame-stats exit report sums the same breakdown from `H2_FRAME_STATS_FROM`. Deferred packets report the recording outcome on the worker. Compare the draw counters with the skipped handlers to tell CPU preparation savings from changes in GPU recording.

## Target 1's headers

Target 1 has two guest headers. The scene's main layer-1 site (`0x0002be45`) asks `0x00014bc0` for it before the layer. `0x0001dcf0(1)` gives the attachment header at descriptor `+0x18` when the descriptor is enabled and `+0x78` is positive, and `0x0001dcc0(1)` returns the texture header at `+4`.

Equal guest dimensions alone do not establish the same target. The native stage suppression counters show committed draws. Frame and pass colour checks use the agent API (`render.frame` and `render.stats`).

## Scene and exceptional draw sites

The retail return sites distinguish uses of the same layer number.

| Site | Draw calls | State and ownership |
|---|---|---|
| Scene kinds 1 and 2 | Layer 1 `0x0002be45`, layer 3 `0x0002bed0`, layer 5 `0x0002bf34`, layer 8 `0x0002bfa5` | Layer 1 binds target 1 and usually clears. Layers 3, 5 and 8 select the scene target at `0x004858b8`. Layers 1 and 3 start with stencil mode 1. Layers 5 and 8 start with mode 0. Layer 8 also requires the byte at `0x004ba022`. |
| Scene middle | Layer 7 `0x0002c034`, layer 2 `0x0002c05b` | Layer 7 follows `0x000335e0` and precedes the dynamic lights. Layer 2 draws after the lights and after `0x0002c370(1)`. Both take their setup through `0x00044370`. |
| Alternate scene kinds | Layers 1, 3, 5, 2 at `0x0002bd57`, `0x0002bd6f`, `0x0002bd87`, `0x0002bda3` | All use the general layer begin. The main scene's target-1 and clear sequence is elsewhere. |
| First person | Layers 1, 3, 8, 7 at `0x000400a6`, `0x00040131`, `0x000401a2`, `0x0004026c` | The first-person list and projection are separate. The two scene calls to this chain straddle the first-person light step. |
| Sky | `0x0002c0bc` to `0x0003ede0` to `0x0004c690` | The sky has its own setup inside a stencil-mode-2 bracket, and its renderer issues an indexed strip at `0x0004c976`, with shader layer 15 and explicit cull disable. This path sits outside the layer loop and the diagnostic labels its indexed draw `Sky`. |

Within the normal geometry handler, the layer-5 object branch at `0x0004d22f` can select a subpart mask after resolving the object's geometry. The emitter at `0x0004daa0` can issue multiple indexed slices from that mask, and layer 5 sorts with `0x0003bbd0`. A replacement preserves both the slices and their order.

The geometry handler switches stencil immediately before the emitter in layers 1 and 3, then runs its stencil tail after the emitter. Its prefix also handles per-record fade and geometry state. The same layer's entries may use other handlers from the live list, so a replacement needs the observed standard geometry call as well as the layer id.

## Related

[native-world.md](native-world.md), [native-world-materials.md](native-world-materials.md), [native-light-gather.md](native-light-gather.md), [native-objects.md](native-objects.md), [occlusion-culling.md](occlusion-culling.md).
