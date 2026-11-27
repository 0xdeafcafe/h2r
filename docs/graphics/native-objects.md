# Native renderer stage 2: objects and skinning

Status: Built. Resident object passes and authenticated indexed ranges draw natively in the default configuration; the first-person lights' layers 12 and 13 draw from the scene ([native-lights.md](native-lights.md)), their layers 11 and 14 on D3D.
Code: `crates/rewrites/native-render/src/{model,model_load,model_gpu,objects,instances,object_*}.rs`, `crates/rewrites/render-bridge/src/{native_objects,native_edits}.rs`, `crates/extensions/game/src/native_objects_glue.rs`. Tests: `crates/rewrites/native-render/tests/models_gpu.rs`, `crates/rewrites/render-bridge/tests/native_objects.rs`, `crates/rewrites/native-render/tests/object_lighting_differential.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

Stage 2 of [native-renderer.md](native-renderer.md). Bipeds, vehicles, weapons,
equipment, scenery, machines and crates: every object whose render model the
game queues in the scene list. Geometry is resident per render-model tag,
loaded once per map; each frame the CPU hands over only the drawn objects'
node matrices, read from the game's own computed transforms in guest
memory. Skinning runs in the vertex shader. The rule that must hold: an
object draws natively only when its material, state and inputs can be
reproduced exactly; otherwise it is a named drop (below), and no D3D draw
stands in for it.

## Stage 2: objects and skinning

### Native scene object walk

The walk covers the third-person base-layer call to `LAYER_DRAW`
(return `0x0002bed5`). It reads the private sorted entries at the first
`LIGHT_GEOMETRY_DRAW` (`0x0004d0b0`) reached from `LAYER_ENTRIES`
(return `0x0003bc90`, or the layer port's authenticated continuation).
The guest has selected visibility, objects, regions, permutations, LODs
and shader implementations. The draw handler has yet to upload the
mesh streams and node rows, evaluate shader parameters, set render
state, or issue any D3D draw for that part.

The display gather supplies additional object handles for the wider view.
At the scene's layer-3 call, `native_scene_lists::display_model` runs
section selection, lighting, palette construction and record emission
against a private overlay of guest memory. The display uses the record's
chosen permutation, LOD, implementation and skinning rows. Hidden objects
and the first-person owner are suppressed. `native_objects_glue/display.rs`
submits resident opaque parts only to the display attachments. An object
whose setup needs an animation update, effect overlay or attachment walk,
or whose selected section has an unsupported material, can still be
absent at the sides in protected sessions. When the unit pose tail
(`0x000bd934`) can adjust either of its selected nodes, the shared
object setup leaves palette construction to the guest. This also
applies when both earlier actor pose overlays are idle.

When the native pass leaves a run to the guest and the guest's D3D draws
are the display — single-copy rendering, so no distinct reference copy
takes the readbacks — the Highest choice swaps the record's section,
palette block and part index range at the sorted geometry handler itself
(`native_scene_lists/display_swap.rs`, after the native objects walk has
taken its runs): the substituted section must classify and material
identically, its geometry must be resident, and its remap rows must
already be in the palette, or the retail draw stands. No pose step runs
for the substituted tier; the walk and every resource request stay
retail. In every other configuration the guest's D3D draws are the
reference and keep the retail geometry
([projected size and level of detail](occlusion-culling.md#projected-size-and-level-of-detail)).

The stack holds the remaining entry count at handler ESP + `0x28`, with
the array immediately after it. The entry contains its item index, the
selected implementation byte at `+4`, vertex classification at `+8` and
implementation pointer at `+0x10`. The dispatcher's exclusion is the vertex shader datum at
implementation `+0x100` equal to `-1`. The item supplies the render
record index and the part. The record supplies the model datum and the
chosen section, including its permutation and LOD. The live section count
and block are at model `+0x24` and `+0x28`. The draw reads the section's
geometry through `+0x34` and uses a `0x48`-byte part element. The walk
checks the resulting index offset, count and topology against the uploaded
part. It uses the record's node rows and the owner's live pose and lighting
cache. The render target provides the view constants and W clip.

A native run contains every live layer-3 dispatch of each object it
covers. It may include parts interleaved with other eligible objects or
slots the dispatcher excludes. An intervening guest draw, an earlier
part, a special handler or an unsupported part can make the whole span
run on the guest. The host checks the material and vertex program,
render-state writes and restore lists, view and clip, buffer ranges,
textures, and current map generation. The layer's first shader begin
runs on the guest because it initializes the shader and texture caches.
A pass transition is eligible when its render-state changes are
already reflected in the live state.

Fog uses the D3D fog mode, start, end, density and colour from the device
and the implementation. The vertex program supplies `oFog`. An implicit
final combiner uses the fog factor and D3D's specular-enable bit, with
normal alpha semantics. The walk handles the scene's zero-reference
stencil state and its temporary stencil-off bracket when mode 1 is
already set. Draws outside those conditions use the original handler.

After every required target copy records the whole run, the hook adjusts
ESI and the saved loop count and returns with the handler's normal stack
cleanup. The guest loop advances to the next entry in its original order.
If planning or reservation fails, the cursor stays put and the original
handler runs. The scene list builders supply visibility, LOD inputs and
supported records. The object setup tail supplies the palettes consumed
by the walk, lights, shadows and later passes. The draw savings come from
skipping `0x0004d0b0` and its per-part shader and geometry preparation,
including `0x0001bbf0` and
`0x00016b90`, for committed runs.

The activation hook starts a utility-priority worker for the mounted
map. The worker reads every object palette in the mounted scenario,
including entries with no placement, and walks those definitions first,
followed by the remaining object tags. It decodes each distinct render
model once, creates its GPU resources and submits its texture uploads.
It publishes the completed definition index and resident models
together. The first swap after publication makes them eligible for
native draws. Later spawns share those models. The indexed emitter handles resident-plan misses while the
models, bindings and target pipelines warm. Map activation and
snapshot reload invalidate the previous load's work. A pass that already
committed holds its models until recording ends. The last reference sends
their GPU resources to the shared map retirement worker for destruction.
First-person models use the same worker.

A paired editor's live edit changes the map file as the raw overlay
serves it ([realtime-edits.md](../editor/realtime-edits.md), "The native
refresh"). The hot-edit glue pushes each edited render model and bitmap
image onto `render_bridge::native_edits`. When models are resident, the
objects' glue drains its share at the next walk or preload, one refresh
at a time, onto a worker (`native_objects_glue::refresh_models`). An
edited model the load made native is loaded again from the file as the
load does (`load_model`), uploaded into a staging set and moved into the
draw set under its lock (`LitObjects::move_model_into`), with its new
coverage and the load's residency. A model that no longer loads natively,
or whose node count changed, leaves the native path
(`LitObjects::remove_model`), so its parts keep their D3D draws from
guest memory, and the log says why. An edited image is decoded again at
each stage that samples it and every program texture made from it is
rewritten in place (`LitObjects::rewrite_image`); another size, level
count or format keeps the loaded pixels until the map reloads. A shader,
template, pass or vertex shader edit (`Edit::Shader`) loads each resident
model with a part whose shader is built from it again, the same way
(`LitObjects::model_shaders`). First person follows the same rules on its
own set (`native_first_person_glue::refresh_edits`): an edited model is
asked of its loader again under its first key, from a map opened again,
and an edited image is decoded again and rewritten
(`native_first_person_load::refresh_images`). The
editor's and Theater's `GpuModels` replace a model under its index with
`GpuModels::replace_model`. The sun's caster models
(`native_render::sun_casters`) keep the geometry they loaded.

#### Shared program resources

Because the load walks every object tag, a multiplayer map uploads a few
thousand material programs. They repeat a small set of generated
shaders, stage layouts and samplers, and a material's passes bind the
same bitmaps. One `LitObjects` set makes each of these once
(`object_program_gpu::Shared`): a module with its pipeline layout and
pipeline table per WGSL and stage layout, a bind group layout per entry
list, a sampler per descriptor, and a stage texture per stage index,
bitmap image (or the objects' lightmap stage) and signed channels. Each
program keeps only its own bind group. A stage whose image is missing
gets a black texel that is never shared. A staging set
(`LitObjects::empty_with_layout`) starts with nothing shared, so a
refreshed model uploads its edited images again. An image rewrite
(`LitObjects::rewrite_image`) writes the shared texture, so every
program that binds it sees the edit. `LitObjects::created` counts what a
set made. With one resource set per program, Ascension's load reached
about 9,000 textures, 12,800 samplers and 3,600 modules and layouts in
lab run 0076.

The palette status (`scene_palette_*` in `render.scene`) distinguishes
pending definitions, explicit null model references, failures, partial
native coverage and resident definitions. A resident definition has a
published model and a layer-3 material program for every drawn part and
declared implementation across the model's sections. Material LODs with
different base passes and parts outside the native object renderer's
coverage keep the definition partial. Extraction checks the model's
published upload as well as the selected part's native coverage. A
failed GPU lookup for a covered part is a named drop
(`native_objects_walk_unresident`).

#### Object setup

`OBJECT_RENDER_SETUP` (`0x00133790`) is answered by `native_scene_lists`'
seam (`setup_call`) wherever it can build the whole call's output from the
call's registers: the render records, the palettes and node rows, the
lighting bytes, the animation and geometry cache stamps and the residency
requests, all staged and published in one turn in the guest's visit order,
with AL and the "not drawn" registers. Such a call is counted
`native_scene_lists_object_setup_native`.

A call the setup cannot build (a refusal of `Refusal`: memory, layout,
x87 or MXCSR state, object model, lighting, palette, residency) runs the
guest body, counted `native_scene_lists_object_setup_fallback` and under
its cause (`..._memory`, `..._layout`, `..._unsupported`,
`..._object_model`, `..._lighting`, `..._palette`, `..._residency`). The
body issues no draw: it writes the records the native scene then draws.
It owns what the build does not reproduce:

- a root object's fresh light sample (`lighting::refresh`,
  `keeps_light_sample` false: `..._lighting`). The first person's arms are
  such a root, and the weapon copies its light from them, so taking the
  "not drawn" exit here loses the arms and leaves the weapon black on a
  sample never taken;
- a view other than the main view (kind 0) and the light view (kinds 1
  and 2), such as a menu's model view (Choose Profile's Spartan:
  `..._unsupported`);
- a locked pose or a first-person model of more than 64 nodes
  (`..._palette`).

The setup is therefore not a cut (`render_bridge::cut::CUTS`), and the
census row `object_setup` is partial (test
`a_setup_the_build_refuses_runs_the_guest_body_and_is_not_dropped`).

A residency request that needs a new queue entry, a priority raise or a
wait (`residency_request` returns `NeedsGuestService`) is the guest
cache's own service, so that call runs the body too; once the request is
queued the next frame's setup is native. A resident block, or a request
already queued, is made by the setup: the cache stamp and the
structure-miss count (`render_bridge::native_scene_lists::residency_request`;
test `object_setup_makes_the_guests_residency_requests`).

#### Frame stats

The `native_objects_walk_native` column counts the objects drawn per view.
`walk_unsupported` and `walk_order` count the objects of a span the walk did
not admit, `walk_reservation`, `walk_model`, `walk_inputs`, `walk_state` and
`walk_unresident` the objects of a run the host did not take. All of them
are drops: the walk consumes the handlers of those objects
(`native_objects::walk_geometry`) and counts `native_objects_scene_dropped`
with them, so `walk_fallback` no longer rises. The run draws in
`native_objects::SLOT` (`ViewSlot::OpaqueBase`, slot 1), and the pass
carries the slot. `walk_unresident` counts the objects in a run whose
requested model agrees with the loader's part coverage but has no uploaded
geometry and materials. `walk_model` includes unresolved definitions,
mismatched models and loader lock contention.
`walk_unreadable` counts views whose sorted list or
ownership could not be established. These views may have an unknown
object count, so their layer-3 handlers are the one place the guest's
draws remain. The `walk_part_*` counters count the parts (not objects)
behind `walk_unsupported` by their cause
(`scene_extract::layout::object_walk::Refusal`,
[framescene.md](framescene.md), "Slice 3"): `aux_state` a bit-11 part
whose toggle's inputs (`scene_extract::layout::aux_depth`) do not read,
`subparts` a bit-6 part whose visible-subpart mask does not read or
names a negative first subpart, `shader_flag` a flagged shader on a
record with a structure lightmap index, `argument` an item's `+0x14` or
`+0x15` set or an argument's low byte other than the key's part,
`program` a program with flag bit 20, and `layout` every other refusal
(the item's layout, a check against the guest's choice, memory that does
not read). Every refused part is in exactly one, so their sum is what
the walk drops.

The method for comparing the walk with `impl.render.native=off` is in
[notes/native-ab.md](notes/native-ab.md).

### Research points (Code unless marked; `h2 disasm`)

| Point | What |
|---|---|
| `0x004e0300` (`points::OBJECTS`) | The object datum array header `H`. Header of handle `h`: `[H+0x44] + 0xc·(h & 0xffff)`; salt `u16 +0` must equal `h >> 16`; body `[header+8]` ([live-gamestate.md](../live/live-gamestate.md)). The pool compacts, so a body address is read every frame, never kept. |
| body `+0x00` | The object definition tag datum (`bipd`, `vehi`, `weap`, `scen`, ...). |
| body `+0x30`, `+0x3c` | World bounding sphere: centre (3 floats), radius ([occlusion-culling.md](occlusion-culling.md)). |
| body `+0x114` (u16), `+0x116` (i16) | **The node-matrix block**: its size in bytes and its offset from the body. `object_new` `0x000b7b40` reserves it at `0x000b800b..0x000b801e` with `0x000bc380(field 0x114, 0x34 · nodes, align 0)`, the node count from hlmt `+0x78` (at least one). `0x000bc380` writes a block reference as `+0` size and `+2` offset (`0x000bc427`, `0x000bc430`). The other blocks it places: `+0x10c` and `+0x110` (32 bytes per node, when animated), `+0x118` (10 bytes per region), `+0x11c` (8 per attachment), `+0x120`, `+0x124` (`0x18` per change colour), `+0x128`. |
| node matrix (`0x34` bytes) | The game's `real_matrix4x3`: `+0` scale, `+4` forward, `+0x10` left, `+0x1c` up, `+0x28` position. A local point `p` goes to `scale · (p.x·forward + p.y·left + p.z·up) + position`: the bounding-sphere update `0x000bdf90..0x000be068` applies the root node exactly so, reading the block through `[body+0x116]`. The matrices are world space (the sphere it produces is the world sphere at `+0x30`). |
| mode node `+0x28` (`0x34` bytes) | The node's inverse rest frame in the same layout (scale, then the inverse's three columns, then its position; `crates/tools/h1-assets/src/models/tests.rs`, `node_inverse_frames_are_stored_as_columns`, and the retail check `retail_h2_mode_layouts`). A retail hlmt node is its mode node's first `0x5c` bytes (V, `crates/formats/tags/src/defs/hlmt.txt`), so object node `i` is mode node `i`. |
| skinning | Vertex world position `Σ wₖ · N[nₖ] · I[nₖ] · p`, with `N` the object's node matrix, `I` the mode node's inverse rest frame, `nₖ` the section's node map applied to the vertex's node index (S: that the game composes the same `N·I` for its node constants; "rest frame times inverse is the identity" is Test in h1models, the draw side is checked by the A/B). Sections without vertex nodes (classification 1) are bound to the first node of their node map, else node 0 (S). |
| `0x0002cbf0` | Fills render record `0x004ba138 + 0x20·i` (at most `0x352` per frame, counter `0x004c0b78`): `+4` its second stack argument, the render model (mode) tag datum (`-1` for the BSP), `+8` a word from the caller's per-section state, `+0x10` its fifth, the object's lighting (below), `+0`, `+0xc` and `+0x1a..+0x1b` bit-packed. Its caller for objects is `0x00133f70`, which walks a per-object render state built under `0x00133ab0` (`+0` the object handle, count `+0x166`, per-section words from `+0x48` with stride 4: the lighting pointer at `+0x48 + 4i`, the mode datum at `+0x5c + 4i`, both filled by `0x0003d4f0` and `0x0004b5d0`). **The record does not hold the object handle**. |
| record `+0x10` → cache record → owner | How an object's render record names its object (`objects::record_owner`): `+0x10` points at `+0x44` of the object's record in the render-state cache (`0x0004b5d0`: `lea ebx, [record+0x44]`), or is 0; the cache record's `+4` is the owner's handle; the owner's body `+0xcc` holds the cache record's handle, which `0x0003ddd0` checks (`[data + 0x100·(body[+0xcc] & 0xffff) + 4] == handle`). The resolve requires all of it (pointer at a record's `+0x44`, index below the cache's high water `+0x38`, owner live by `0x000badc0`'s rules, `+0xcc` naming the record back); a record that fails uses the geometry handler. Code; Test on synthetic memory. |
| `0x000badc0` | The object lookup: slot `handle & 0xffff` below `[H+0x38]`, element `[H+0x24]·slot + [H+0x44]` (12 bytes; `0x0003ddd0` uses a fixed 12), live when its `u16` salt is non-zero and equals `handle >> 16`, body at `+8`. `objects::body` follows it. |
| `0x0002bed5` | The return address of the base layers' layer-3 call (`call 0x00044550` at `0x0002bed0` in `render_scene_passes`, view kinds 1 and 2). The object skip acts only there, so first-person or other-kind layer draws are never touched. |
| `0x00040136` | The return address of the other layer-3 caller: `call 0x00044550` at `0x00040131` in the first-person draw `0x0003fdf0`, which runs after the transparents for its own record list (`0x004c8740`, `[0x0050943c]` records) under the first-person camera and projection, then `0x000445d0`. The call is cut ("First person" below): its body never runs, so this return address is never reached. |
| `0x0004baf0`, `0x0004b5d0` | LOD choice by pixel size inside `0x00133790` ([occlusion-culling.md](occlusion-culling.md)). |
| `0x0016d764` | Region permutation choice (Likely): the variant's region map, its regions at `+0x18`, then `0x0016dad0` ([objects.md](../formats/objects.md)). |
| `0x00165f16` (`points::FIRST_PERSON_RENDER`) | One local player's first-person weapon and arms, after the transparents, with their own projection (camera `0x0023c110`, FOV `0x00187130`). |
| `0x0003ddd0`, `[0x00509434]`, body `+0xcc` | The per-object render-state (lighting) cache: records of `0x100` bytes, lit per frame by `0x0003dfe0`, lighting at `+0x44..` and `+0xc0..` (layout not read; [notes/pipelined-render.md](notes/pipelined-render.md)). |

### Layer-3 masks

Items the walk does not take are planned by the layer observer before the
layer draw and paired with their indexed D3D submissions, under the same
plan rules as the world (`LAYER_DRAW`, an opaque point): **before** the
layer draw, when the current layer is 3 and the caller is the base layers'
call (`[esp] == 0x0002bed5`), walk the current list; an item is an object
item when its build is `0x0004cbf0` and its render record names a live
object (see "Research points", record `+0x10`). It is skipped when the host
draws that object's definition and the item's shader (`+8`) is one of the
host's native materials: layer 3's bit cleared, the item remembered, the
object added to the view's drawn set (once). The bits go back **before the
next layer draw** (5 and up follow 3 in the same view), on items that still
hold the mask the skip left, as the world's do. The host gets one external
pass per view carrying, per drawn object, the definition and the node
matrices copied out of the body (`0x34 · count` bytes) and the bounding
sphere.

The base walk owns layer 3. The scene layer planner handles object layers
1, 2, 5, 7, 8, 17 and 20. The sorted camouflage and hologram handlers
select layers 18, 19 and 21. Each layer retains its own pass order,
depth test and blending. Dynamic lighting has a separate planner.
See [native-lights.md](native-lights.md).

What the native pass draws is exactly the set of objects whose layer-3
items it skipped. The game's own visibility, object list, attachment
recursion and "not drawn" decisions (`0x00133790`, occlusion) have already
run, so a held weapon, a vehicle's driver or a culled crate need no logic
of ours. A conservative sphere-frustum test runs anyway (cheap, and a
guard against a stale sphere).

### Fade, the shader flag and the item's argument

The geometry draw `0x0004d0b0` takes three things from an object item
beyond its material and its record's `+0` bits (**Code**, `h2 disasm`).
Extraction reads them for layer 3's walk (`object_walk::read_part`), the
world layers' items and the first person's (`object_walk::layer_part`);
the module notes of `scene_extract::layout::object_walk` list them with
their addresses.

- **The fade byte** (record `+0x1a`). Below `0xff` the prologue
  `0x0004d640` scales it by `0x0045dbb4` and, where that differs from the
  engine's cached fade `0x004b8494` by more than `0x0045dbdc`, sets the
  polygon stipple pattern `0x0001c290` picks (one of 17 at `0x00487288`)
  through `SetStipple` (`0x003f8cc0`) and caches the fade. The epilogue
  `0x0004d720` does the same for the engine's one (`0x0045dbc0`), and so
  does the layer end `0x000445d0`. The pattern masks the draw only while
  the device's stipple is on: the target mode call `0x00015680` turns it
  on (mode 1) before layers 1 and 3 outside view kinds 4 to 7, and off
  (mode 0) before layers 5, 8 and 20 (`0x0002be17`..`0x0002bfef`). The
  part carries `PartFlags::FADE`. The native draw applies no stipple, so
  it takes the part only where the device's stipple is off
  (`native_objects_glue::stipple_off`), where the guest's draw is not
  masked either, and only where the engine caches a fade of exactly one
  and the device holds the table's pattern for one
  (`native_objects_glue::stipple_at_rest`,
  `opaque_material::rest_stipple_table`): from there every faded record's
  epilogue puts both back, so skipping the draws leaves what drawing them
  would. Layer 3's walk already requires the stipple off for every run.
- **The shader's flag bit 0** (`+0x16`). The draw then takes no lightmap
  in layer 3 (`0x0004d2ab`) and no PRT stream in layer 5 (`0x0004d237`).
  It asks for residency of the stage bitmaps of the shader's templates
  (`0x0004d486`..`0x0004d55d`: `0x0012ccf0`, then `0x0012ce00` for one
  not resident), which sets no device state. With its program's flag bit
  20 (the pass implementation's `+0x118`, `0x0004d561`..`0x0004d572`) it
  calls `0x0003bea0`, which the first time a frame (`0x00509430` clear;
  `0x0002c0f5` clears it) clears the shader cache (`0x00016b10`, which
  first writes the cached program's restore states) and, when the
  active structure has a parameter block, copies and binds the
  structure's target (`0x000352e0`, `0x00014bc0`). The part carries
  `PartFlags::SHADER_FLAG`. Layer 3's walk reads the entry's program and
  refuses bit 20 itself. A world layer's or the first person's entry is
  built under the layer draw, so extraction gives the part
  `PartFlags::TARGET_UNCOPIED` while the frame's copy is not made, and
  the host, which knows the programs of the passes it plans
  (`ResolvedImplementation::copies_target`, the implementation's bit
  20), keeps the item when one of them has the bit
  (`object_program_gpu::makes_target_copy`); an item whose programs lack
  it makes no call, so skipping its draw skips nothing else. A kept item
  counts as one of unknown effect for the order of those after it
  (`layer_pass::clear_layer_items`). A flagged layer-3 record with a
  structure lightmap index is refused, since the native draw would bind
  it and the guest's leaves the stage as the device holds it. Extraction
  reads no PRT stream for a flagged layer-5 item.
- **The structure's parameter block.** When the active structure
  (`[0x004e0348]`) has one (`+0x214` above 0), a flagged shader's
  prologue runs `0x0003c650` and `0x0003d000` on its first element
  (`[+0x218]`). The first rotates the up vector `[0x004687b0]` by the
  angle of the view words `0x004b9dac`, `0x004b9db0` (`fpatan`) plus the
  block's `+0x8c`, stores thirteen words at `0x004c1b9c`, and sets the
  bump matrices of stages 1 to 3 (`0x003f5e30`) from four of them times
  `+0x68`, `-(+0x6c)` and `+0x70`. The second sets `c[21..25]`
  (`0x003f71d0`) and the fog colour (`0x003f5370`, `+0x58` packed by
  `0x00131ed0`). `engine::render_loop::structure_parameters` computes
  all of it in the original's order, x87 and SSE steps included.
  Extraction gives such a part `PartFlags::STRUCTURE_PARAMETERS` and its
  view the upload (`View::structure_parameters`; the item refuses
  `object_shader_flag` when it does not compute). An object pass
  compiles a program reading rows of `c[21..25]` its pass does not set
  and records them (`ObjectPass::structure_reads`); the draw puts the
  upload's rows there and its bump matrices into stages 1 to 3
  (`object_program_gpu::with_structure_parameters`), and an item without
  the upload refuses such a program. The draw takes the fog colour from
  the device at the call. The host takes the part only where the device
  and the engine already hold every word of the upload
  (`native_objects_glue::structure_at_rest`), so skipping the draws
  that would run it leaves what running it would.
- **The item's argument** (`+0xc`) and its `+0x14` and `+0x15` bytes. The
  sorted dispatch passes them to the draw (`0x0003bc7b`..`0x0003bc8d`).
  The argument's high byte is the item's projected size: `0x0004d9c0`
  scales it by the part's `+0x2c`, `0x004670e4` and `0x0045dc10`, and the
  shader begin `0x00016b90` turns it into the distance words at the shader
  context's `+0x28` and `+0x2c` (`0x00016c6e`..`0x00016cb0`). Only the
  texture-stage resolve reads them (`0x0001b0a0`), for the level of
  detail it asks residency for, so the byte changes no state, constant or
  binding of the draw, and the native draw ignores it. `+0x15` and
  `+0x14` are the shader level and the pass within the layer that
  `0x00016b90` selects; the native passes are the first level's from the
  layer's first pass, so either set, or a low byte other than the key's
  part, keeps the item on D3D.

What still declines, by name: `object_fade` (the device's stipple is on:
layers 1 and 3 outside view kinds 4 to 7), `object_stipple` (the cached fade or the
pattern is not what the epilogue leaves), `object_shader_flag` (the
structure's parameter block's upload does not compute or the device
and the engine do not hold it, or in layer 3 a structure lightmap
index),
`object_shader_flag_target` (the target copy not made yet and a pass's
program with flag bit 20) and
`object_argument` (another pass, level or part). In layer 3's walk these
are `walk_unsupported` (extraction) or `State` (the stipple); in the
first person's layer 3 they are its log reasons.

### Transform source

Node matrices are read once per view at the hook, from the body
(`[body+0x116]`, `[body+0x114] / 0x34` nodes), after the game has finished
the frame's animation and attachment updates (the records exist, so
`0x00133ab0`'s node pass ran). They are uploaded raw (13 floats a node)
into one storage buffer per pass, with one small instance record per
object (node base, node count, and an RGBA8 highlight the lit colour
mixes towards by its alpha; zero for the game, the selection in the
editor). The inverse rest frames are resident
per model. No matrix product is done on the CPU. An object whose node
count disagrees with its model's stays on the D3D path.

### LODs

The game picks a LOD per object by pixel size, and each render record
names the section it draws (the key's bits 9..17). An object's parts come from the sections its records name, nothing else.
So the load takes
every detail level's section of the permutations picked
(`model_load::pick_lod_sections`, highest first per region), and a
distant object draws the lower level the game picked. A distant object
therefore draws the lower level the game picked.

An item whose record names a (section, part) none of the native parts
is (another permutation than the one loaded, or a part that did not
load) is not offered at all (`NativeSet::parts`, below): it keeps its D3D
draw. The base-map stand-in for transparent parts still draws the
highest level's.

### Permutations and regions

The variant is the object definition's default (`0x0016d180`'s rule,
`tags::object::Context::resolve_model`); its regions map to the render
model's regions by name, and each region's permutation to the render
model's by name. A model is drawn natively only when every region the
variant lists has **exactly one** permutation. Models with a random or
state-driven choice (marine heads, damage states, a variant changed at run
time through `+0xb1`) stay on the D3D path until the record's section is
read. Regions the variant does not list draw their first permutation, as
the viewer does (S: some may be hidden in game).

### First person versus third person

The first-person weapon and arms are drawn by `0x0003fdf0`, called twice
from `render_scene_passes` (`0x0002c2b3` with argument 0, `0x0002c2c2`
with argument 1, the first-person lights `0x000404b0` between them), after
the transparents, not by the base layers' layer-3 call, which the caller
check pins. They have no camera of their own: `0x0003fdf0` and its call
site (`0x0002c1d0`..`0x0002c2c2`) set none, so they draw with the render
camera record (`0x004b9da0`) and the projection the game view drew with.
In first person that record is the player's: the first-person camera
update `0x0023c110` takes its field of view from `0x00187130` (its only
caller), so the weapon and the world share one field of view. The arms and
weapon sit in front of it in world space, posed by their node matrices. The local player's third-person body draws natively like any
biped when the game queues it.

#### First person

`0x0003fdf0` is a cut producer (`render_bridge::cut::CUTS`, row
`first_person`, entry `0x0003fdf0`, `ret 4`, AL = 1; the mode is in `eax`
and the call, 0 or 1, is the stack argument). Its producer is
`feature::native_first_person` (`cut::OWN_PRODUCER`), and the census row is
`partial`: the cut is held until a frame proves the native path, then
released for the rest of the run (`native_first_person::held`).

- **Held.** The entry runs the body as a guest call of itself
  (`held_call`), so the guest draws the arms and weapon and its layer draws
  put their parts in the first person's view. Before the body it counts the
  parts the guest draws (`guest_parts`: the item build's list-0 items, once
  per layer the call draws that each holds); after it, the native path
  plans the view's parts of the call without reserving or drawing
  (`probe`, `Host::plan`). A call passes its `Check` when the view holds the
  guest's count, the plan would draw all of them and drops none. A frame
  (`Gate`: call 0, then call 1) whose two calls pass and draw at least one
  part releases the cut.
- **Released.** The entry makes the frame step's writes for the row,
  answers the call and draws from the scene (`cut_call`); the body never
  runs for either argument.

- **Items.** Each call builds its items before it draws, and the answer
  builds them in Rust (`engine::render::light_gather::first_person_items`):
  the prologue (`prologue`: list 0 at `0x004c1a48` saves its layers at
  `+0x10`, clears them and starts at its count; the sorted list's mark
  `0x004b6284` takes its count), the first-person effects (below), then
  (`records`, reading list 0 and the sorted list as the effects left them)
  each record index of `0x004c8740` (`[0x0050943c]` of
  them) that passes the mode's filter (mode 0 all, 2 `0x10`, else `0x20`),
  part by part as `0x000449e0`, `0x00045ce0`, `0x000460d0` and
  `0x00041040` in kind 0 make it: one list-0 item per part with layers, and
  a sorted entry (`0x0001e2d0`, depth along the view's forward vector less
  the record's bias) for a part with layer 15. `native_first_person::build`
  applies those writes, sets the current layer and list for each layer the
  call draws and puts its items in the first person's view, then makes the
  epilogue (list 0's layers back, its first item 0). A record or shader the
  build does not read (a structure record, a virtual shader, a level table
  with no levels, a record with no node matrices) is the drop `items`.
- **Effects.** `0x00174220` with argument 1, which the body calls after its
  prologue, is ported (`engine::render::light_gather::first_person_effects`):
  it walks the particle systems (`[0x00510c74]`) in index order and selects
  each one whose effect's owner is the current user (`[0x004b9ed8]`) with
  the first-person camera (`0x00155760` answers 0), whose location reads
  (`+0x30`), with no parent (`+0x4c`), in a visible cluster or with the
  definition's bit 9. The per-view update `0x00174420` of each selected
  system stays the guest's (its stamps and particle draws,
  [framescene.md](framescene.md)) and is a guest call made at the stack
  depth the guest makes it (`RESERVE`), with the selection bytes' upper
  stack bytes as the guest pushes them. The selection is read again after
  each update. A particle model's definition (`PRTM`, `prt3`) is the
  guest's `0x001751d0`, called once per system. A system whose definition
  does not read stops the walk: the drop `effects`.
- **Source.** The first-person `View` of the frame step's scene
  (`ViewKind::FirstPerson`). Its parts, in the view's order, are the arms and
  weapon. A part is readable when its instance and rows are in the scene.
- **Slot.** `native_first_person::SLOT`, `ViewSlot::FirstPerson` (slot 10),
  after the world's transparents (slot 9) and before the flare query. The
  first-person lights (`0x000404b0`, `native-lights.md`) and the sorted pass
  `0x00040870` have their own slot-10 entries.
- **Calls.** The split is the guest's (`0x0003fdf0` tests the stack
  argument at `0x0003ffa0`). Argument 0 draws layers 1, 3, 8, 20 and 7, then
  layer 2 when `0x00044370` says so (`0x00040284`). Argument 1 draws layer 16
  only (`0x0003ffbf` sets it, `0x00040010` jumps to the same draw at
  `0x00040284`). `native_first_person::LATE_LAYER` is 16.
- **No fallback.** There is no D3D draw and no mask edit. A part that does
  not draw is a named drop (`native_first_person::Dropped`): `items`,
  `effects`, `no_view`, `no_host`, `unreadable` (its instance, rows or order range are not in the
  scene), `unresident` (its model is asked for), `unsupported` (the model
  is resident, not this part or implementation) and `reservation` (the
  host's). Each is counted as `first_person_dropped_<cause>`
  (`native_first_person::drops`) and in the stage's `scene_dropped`.
  `render.scene` reports them with `first_person_calls`,
  `first_person_drawn`, `first_person_checked`,
  `first_person_check_passed` and `first_person_held`
  (`native_first_person::counters`).
- **Host.** `native_first_person_glue` loads the models
  (`native_first_person_load`: a utility-priority worker opens the map,
  waits for requested record models, and uploads each into private GPU
  storage) and implements `Host::draw` for the offered parts. Record `+4`,
  not the owner definition's third-person model, names the `mode` to load;
  every section of that mode is built because the record key selects the
  permutation and LOD. `Host::draw` draws the offered parts as one external
  pass, the way the scene's objects draw theirs: it reads the device's entry
  state at the call, plans each part from its instance and rows as
  extraction copied them (`plan_parts`, `LitObjects::plan_part_of`), warms
  the pipelines, reserves the pass on the bound attachments and commits it
  while the load it planned against is current. A part the plan refuses is
  `unsupported` (its reason logged once); a pass that cannot be reserved
  drops its parts as `reservation`.
- **Per-part rules** the pass keeps, as for scene objects: the topology and
  subparts the record picks (`part_ranges`), the fade byte and the flagged
  shader ("Fade, the shader flag and the item's argument"), the auxiliary
  depth state (`aux_depth::read`) and the texture accumulation target
  (`sampled_texaccum`).

Tests: `the_plan_places_the_cut_when_on`,
`a_frame_passes_when_both_calls_draw_every_part_the_guest_would`,
`a_frame_with_a_dropped_or_missing_part_holds_the_cut`,
`the_cut_is_released_by_the_first_passing_frame_and_counted`,
`the_guests_parts_are_its_items_in_the_layers_the_call_draws`,
`first_person_draws_after_the_world_transparents`,
`each_call_draws_its_layers`, `an_unreadable_part_is_a_named_drop`,
`the_first_persons_view_takes_the_render_camera_the_game_view_drew_with`
and the cut row's (`render-bridge/tests/native_first_person.rs`);
`a_committed_pass_draws_every_planned_part_and_names_each_drop`
(`native_first_person_glue`);
`first_person_items_match_the_lifted_original`
(`engine/tests/light_gather_differential.rs`) runs `0x0003fdf0` lifted to
the end of its record loop against the item build, byte for byte; the unit
tests of `first_person_effects` drive the walk with `entry::testing::run`
(index order, an update that retires a later system, a third-person camera,
a particle model's guest definition). The first-person
lights are the lights stage's ([native-lights.md](native-lights.md)).

### Shading

Opaque parts are drawn with the game's own vertex program and pixel shader
(see "Object programs"). `model_gpu::GpuModels` and `models.wgsl` draw the
base map times a fixed ambient plus one directional term
(`ambient + diffuse · max(n·l, 0)`); only the theater viewer uses that pass
(`crates/extensions/game/src/theater/host_view.rs`). The sorted handler submits transparent parts (kind 4) at its indexed emitter. See [native-transparents.md](native-transparents.md).

Base maps go through the world's stage decoder (`StageDecoder`, one per
map's model worker): a palettized one (P8, P8 bump) is expanded through
the device palette bound, when the map's first object was asked for, at
the stage its material binds it (the first layer-3 implementation that
names the bitmap; stage 0 otherwise), as the D3D path samples it. The
objects draw only while the palettes the expansions used stay bound
(checked at each layer-3 view); a stage without a full palette refuses the
material.

### Object lighting

A fixed ambient and directional term is not the game's lighting, so a
material that reads the object's lighting is drawn only with the game's
own inputs. How the game lights an object's layer-3 pass (Code, `h2 disasm`;
`native_render::object_lighting` has the table):

- **The constants.** The object geometry draw `0x0004d0b0` calls
  `0x0004d9c0`, which hands the render record's lighting pointer
  (`+0x10`, cache record `+0x44`) to `0x0001bd50`: that makes it the
  current lighting `[0x00485a58]` and marks the engine inputs that read
  it dirty. The shader draw then evaluates the implementation's dirty
  inputs (`0x0001ae70`: scalars at `0x000336f0`, vectors at
  `0x00033980`), and `0x00019be0` writes each of the implementation's
  engine vertex constants (implementation `+0x124`, records of the pass's
  `+0x38`: register bits 0..3, bank bit 4, mode bits 5..9, input byte 3)
  into bank 1, uploaded as hardware `c[18 + i]` by `0x00018e80` (bank 0
  is the vertex attribute defaults, `0x00018d70`). So the lit vertex
  programs read their lighting from `c[18 + i]`, the registers their
  implementation names, computed from vector inputs 3, 4, 14..17, 21..24,
  35, 40 and 42..45 and scalars 23 and 37 (the table in
  `object_lighting`).
- **The implementation.** The build `0x0004cbf0` picks which of the
  pass's implementations an object draws from its lighting: 0 when the
  byte `+0x76` is not `0xff`, 1 when it is, 2 for a shader flag. On 03a
  the object passes' programs are the `lightmap_dynamic*` and `sh_dynamic`
  vertex shaders (`h2 tags 03a_oldmombasa vrtx`).
- **The transform.** `0x0004d830` uploads a single-node object's matrix
  (record `+0x1c`) as three registers at hardware `c[50]`.
- **The port.** `object_lighting::vector_input` and `scalar_input` give
  every lighting input. Vector 40 (all three components) and scalar 37
  are `object_lighting::root_value` (`0x00033670`): the object whose
  handle is the lighting's `+0` (`0x000badc0`, mask -1: index below the
  array's count, salt non-zero and equal), walked up its parents
  (`+0x14`) while its byte `+0xaa` is set, the elements read straight
  from `[0x004e0300] + 0x44` at a stride of 12; the root's `+0x2b0`
  clamped to 0..1, or 0 when a handle does not resolve. Both read
  `[[0x00485a58]]` unchecked, so without a lighting they are not given.
  The value is taken with the lighting's words at hook time.
  `tests/object_lighting_differential.rs` checks every input bit for bit
  (NaN for NaN) against the lifted `0x00033980` and `0x000336f0` on
  seeded states, 40 and 37 over a seeded object array of parents,
  salts and empty elements. The hook copies each drawn object's `0x78` lighting
  bytes and the inputs' defaults into its `ObjectFrame`
  (`objects::object_lighting`: body `+0xcc`, the record still its own),
  so the pass has the frame's values whatever the cache does next.

### Object programs

The map worker compiles each material's base and scene-layer passes
independently. Models with only overlay or sorted passes can supply those
draws. For a shader substituted by a render record, the worker compiles
its layers 18, 19 and 21 over the model's drawn parts. The layer lookup
uses the shader, section and part selected by the record. Its base-layer
coverage and display-only section list still require matching base passes.

A transparent-only model needs no base program. Its layer-15 draw uses
the shader bindings and index range at the sorted emitter. A model made
only of part kinds 0 and 5 has no drawable range. Both definitions can
finish loading with an empty base-part list.

If a scene or sorted object misses the resident plan, the standard
geometry handler sets up the material and streams. At the indexed
emitter, `render_bridge::native_transparents` checks the layer caller,
the handler's saved render-record pointer, the live record index and
its model datum. An accepted full-part or subpart range submits through
`world_layers_gpu::IndexedPass` with the bound state and counts under
the objects stage. The handler continues with its ordinary draw when
the indexed reservation fails.

In layers 1 and 17 an unbound direct read of `c[18]` inherits the
hardware register. At layer entry the objects glue reads its four words
after draining the device, together with row 0 and its dirty bit in
shader material bank 1. A pending upload or a material write that changes
the row runs through the binder and indexed emitter. If an earlier
structure pass can write the row before an object that may inherit it,
the layer planner preserves that upload in list order.

Layer 18 evaluates the pass's Xbox `BLENDCOLOR` writes in order. The
pass can supply the starting colour or inherit it. A binding can change
RGB while preserving alpha, or change alpha while preserving RGB.
When the write must persist for later guest work, the handler performs
it before native indexed submission. Layer 19 takes fog vector 41 and
scalar 39 from the view captured at layer begin. Object vertex and pixel
bindings use the same values, including the view's disabled-fog defaults.

- **The texture accumulation target** (engine texture 3, Code). The
  binding `0x00019530` takes for 3 (`0x000195b2`) binds render target 1
  through `0x00014f60` (its kind, the byte at `0x00015154 + 1`, takes
  `0x00015044`: the texture `0x0001dcc0(1)` returns, the descriptor
  `0x004b4b58 + 0x98` and its texture at `+4`, valid while `+0x88` is set
  and `+0x95` clear) and writes the target's size (`+0x80`, `+0x84`) as
  the stage's size, which engine vertex constants of modes 5..20 read.
  Layer 1's `_combined` passes draw into it; layer 3 samples it in screen
  space, a linear texture addressed in texels.
  - `object_look_stages` takes the stage out of the look
    (`EngineBound::texaccum`); it compiles as a 2D stage without a bias.
  - The pass asks the device for the level (`ExternalPass::sampled`, the
    header `TEXACCUM_HEADER`); after the drain the device finds the
    render-to-texture level over it as a draw binding it would
    (`Device::rtt_level`, in place, nothing written back) and the backend
    gives the pass that copy's view (`ExternalTarget::sampled`, the scaled
    copy's under the render scale). A view whose objects need it is left
    to D3D when levels are not sampled in place
    (`Device::samples_levels_in_place`), and an object whose parts need
    it is left to D3D at the hook, before its items' skip is kept, when
    the target is not bound (`object_nodes::texaccum_bound`, the check
    `0x0001dcc0` makes; `NativeSet::texaccum`). If the device still finds
    no level after the drain, those parts go undrawn for that view: logged
    once and counted.
  - Per draw the stage's coordinate scale is one over the level's guest
    size (`texaccum_scale`), and the engine vertex constants that read
    the stage's size (`ResolvedImplementation::dimension_bindings`, which
    the resolver left 0 by 0 for an engine texture) get it
    (`texaccum_dimensions`).
- **By item, in the list's order.** The skip is per item, not per
  object or section: `objects::clear_objects` reads each item's key
  (`objects::ItemKey`: the record, the item's shader, the record key's
  section and part, bits 9..17 and 0..3, and the implementation its `+0`
  picks), and offers it only when the definition's native part at that
  (section, part) has that shader and implementation
  (`NativeSet::parts`, published per (definition, section, part) with
  the implementations that sample the texture accumulation target). A
  lit part is one geometry part of a section (`LitPart::part`; the
  section's parts share its programs). The feature hands the host the
  accepted items in the list's order (`objects::ItemDraw`: the item, its
  object's frame, its key and its record's node rows,
  `objects::record_rows`); one whose object, rows or texture
  accumulation target (`object_nodes::texaccum_bound`, for an
  implementation sampling it) do not read goes back first.
- **Planned before the skip is kept.** `Host::draw` plans every item
  before the hook returns (`plan_view` in the glue,
  `LitObjects::plan`/`object_program_gpu::plan_part`): its model on the
  GPU, exactly its part and implementation, every per-object constant
  (lighting, defaults, rows, decompression) and every state field
  resolved, the device W-buffered, and for a part sampling the texture
  accumulation target a device that samples levels in place. It says
  per item whether it can draw it. Each physical copy reserves that exact
  sequence and `LitObjects::encode` returns the issue count. A missing
  texaccum level, a partial encode, or a validation error cancels all copies.
  After full reservation the host rechecks the load generation and calls
  `ItemMasks::apply(mem, &drawn)`. This runs `objects::keep_items` on the
  original mask overlay. Only confirmed items change guest memory, and the
  feature retains their full identities for restoration. A shadow comparison
  keeps its guest draw.
- **The state, field by field** (`object_state::resolve`). The host
  reads the render states the device holds at the layer-3 call from the
  guest's render-state array (`object_state::EntryStates`: depth
  enable, function and write; alpha test, function and reference;
  blend and its factors; colour writes; cull and winding; fill modes;
  stencil enable, function, reference, masks and operations; fog;
  solid offset and its biases; depth clip control). A draw takes its
  implementation's own value for each field it sets and the device's
  for the rest, nothing from defaults: a field it depends on that does
  not read or decode refuses it, as does one the pass does not draw
  (blending, fog, the stencil test with its reference and writes, a
  depth bias, a fill mode other than solid, Z-buffering its own
  `ZENABLE` sets) or an alpha test other than the one its program was
  compiled with. The W clip's clamp is the draw's
  (`D3DRS_DEPTHCLIPCONTROL`); its range the device's at the issue. This
  takes in the inherited cull: 03a's `concrete_rebar` part of
  `concrete_chunk_j` sets no `CULLMODE` and draws with the device's cull
  and winding. S: the layer's earlier draws
  leave those fields as at the layer-3 call; the comparison prints the
  D3D draws' own.
- **One load per map, BSP and generation.** The host keys a load by the
  map's name, the active BSP's address and its index (as the world's);
  another key starts a new load (new models, a new worker, a new
  generation the set carries). A worker publishes only to the load it
  was started for (its own `Models`), so a worker of an earlier load of
  the same map (A, B, A again) publishes nothing, and `Host::draw`
  refuses a view planned against another generation.

- **The vertex block.** What the game's upload would hold for one drawn
  object: `c[0..18]` the view's; `c[18 + i]` the implementation's
  constants with every engine binding that reads the lighting put in from
  the port (`object_vertex_uniforms`: the vector inputs through
  `ResolvedImplementation::vector_bindings`, the scalar ones through `scalar_bindings`, both keeping only what later bindings leave);
  `c[50..]` the node rows; `c[170..173]` the decompression.
  Layer passes also resolve bound view inputs. A binding that targets a
  pixel combiner writes the per-object pixel block.
- **The node rows** (`node_rows`, Code): a record with a matrix at
  `+0x1c` uploads it transposed, without its scale, as three registers at
  hardware `c[50]` (`0x0004d830`). Otherwise `0x0004d8e0` reads the
  record's `+8` node block handle (-1: nothing is uploaded, so the object
  is refused): the block at `0x00487b1c + (handle & 0x0fffffff)` holds per
  part (`+0xc & 0xf`) a first matrix and a count at `+4 + 4·part`, and the
  matrices at `+0x44 + 0x30·n`, twelve floats in register order. A model
  record (`+0xc` bits 29..31 = 1) whose section names a node
  (`0x0004cb80`: the `mode`'s sections, stride `0x5c`, the i16 at
  `+0x2c`) uploads one matrix (the part's first when it has one, else
  that node's); anything else uploads the part's whole palette
  (`0x000152a0`), `c[50 + 3·k]` for matrix `k`, which the skinned program
  indexes with `a0.x`. A palette that would run past `c[170]` is refused.
- **The decompression** (`decompression_rows`, `0x0001cdd0` from the
  section setup `0x0004d7c0`): `c[170]` half the position bounds' extent
  (w 1), `c[171]` their centre (w 0), `c[172]` the texture coordinate's
  scale and offset; the identity where the section's flag is clear.
- **The program** (`object_look`, `compile_object`): the implementation's
  look with its engine constants allowed, translated as the world's are;
  every constant read directly must be one of the above, and a read
  relative to `a0.x` must start in the node rows.

- **The vertex streams** (`object_streams`, Code). The draw sets its
  streams through `0x0001caa0`: the section's `vrtx` classification
  (`0x0001cb20`: the geometry classification at section `+0x18`, 0..2 as
  they are, 3 the influences byte `+0x14` plus 2 when the tag has that
  many, else 0), its program, and the streams (`0x0001c8c0`): the
  section's vertex buffer descriptors in order, each stream's format the
  record of its type in the table at `0x0043f408` (`0x15` bytes a type:
  the type, then up to ten (semantic, format) pairs; semantic `0xfe` skips
  bytes, any other negative one ends it). The classification lists per
  register `v<i>` the semantic it reads; the declaration puts each
  attribute in the register reading its semantic. The format codes are
  the Xbox vertex types in order (float 1..4, packed bytes 1..4, shorts
  1..4, normalised shorts 1..4, normalised packed 11:11:10, `D3DCOLOR`).
  The table is read from guest memory when the map is first seen, never
  copied. A section's streams are gathered once per declaration, each
  register's guest bytes in a 16-byte slot (`object_streams::gather`),
  and the vertex unit converts them with the formats and shader fetch the
  D3D path's default (GPU) fetch uses (`object_program_gpu::vertex_fetch`,
  `input_fetch`; `NORMSHORT3` as `snorm16x4` read `(x, y, z, 1)`, and so
  on). Conversion stays on the GPU because a CPU division can round another way than the GPU's normalised conversion, and a part whose layer-3 pass tests depth `Equal` against the depth its D3D pass wrote with the same program then fails wherever w differs in the last bit.
- **The implementation** (`record_implementation`, Code `0x0004cbf0`).
  The build of one record's draw switches on its third argument, the
  draw (the jump table at `0x0004d054`, indexed by the bytes at
  `0x0004d068` from draw 3). At draw 3, layer 3, it takes the record's
  `+0` bits 29..31 (`0x0004cc70`, the table at `0x0004d07c`: 0..4 as they
  are, 5..7 give 3), overriding what `0x00015d70` chose. The lighting's
  `+0x76` and the template's layer-15 bit (byte `+3` of its level's
  layer mask) pick the implementation only at draw 18
  (`0x0004cc9f`..`0x0004cd46`), not at layer 3. The hook copies each
  section's implementation with its rows. Per material each implementation
  of its layer-3 pass that compiles for every section it is in, with its
  stage bitmaps decoded (linear textures refused, as the world programs
  do), is available to the resident walk. The emitter can submit a range
  whose selected implementation could not be preloaded.
- **The normalization cube.** A stage sampling engine texture 1 is bound
  to the vector normalization cube the lights load
  (`native_lights_glue::light_textures`, the globals' rasterizer data),
  sampled as a cube without a bias, as the lights sample it.
- **The shader's property bitmap** (engine texture 26, Code). The material
  resolver binds the bitmap parameter the drawn shader's postprocess names,
  or the globals' rasterizer shader's for -1
  (`tags::material::PROPERTY_BITMAP_TEXTURE`, docs/formats/shader-instances.md),
  so the stage compiles as an ordinary bitmap stage. When the parameter
  falls back to the rasterizer default texture the pass is refused as any
  default-texture stage is (`DefaultTexture`); when the index does not read
  it stays `EngineTexture(26)`.
- **Animated bitmap frames** (bitmap animation component 0, Code). A
  stage whose slot's frame is animated binds every image of its bitmap at
  compile (`object_program_gpu::frame_images`), and each draw picks one
  from the render clock's frame with `layer_pass::frame_image`, the rule of
  `0x0001b0a0` (`object_program::animated_blocks`, `PlannedDraw::frames`).
  The encode caches a bind group per image choice. Components 8 and 9 are
  never read and are skipped; the proof and the refusals are in
  [shader-instances.md](../formats/shader-instances.md#animated-bitmap-frames-and-components).
- **The lightmap stage** (`object_program::OBJECT_LIGHTMAP`, Code). An
  object's draw binds no lightmap page. The draw (`0x0004d0b0`, draw 3)
  sets the current page `[0x00485a60]` to -1 for every record with a
  model datum at `+4` (`0x0004d2c4`..`0x0004d2de`, `0x0004d362`, then
  `0x0004d420`); only records without one (the BSP's instanced geometry,
  `0x0004d367`) get a cluster's page and palette through `0x00015c90`.
  With the page at -1 the stage setup (`0x00018c43`..`0x00018c4c`, then
  `0x00018d1f`) binds the globals' rasterizer data element's bitmap at
  `+0x64` (`[[0x00485a80] + 0x64]`) and no palette. The coordinates the
  draw adds for it come from the BSP: for a record whose implementation
  is not 2 or 3 and whose `+0xc` bits 18..28 are not `0x7ff`, the BSP's
  `+0x64` element (stride 12) at that index holds per section a u16 that
  `0x0004dc60` turns into the vertex offset of a stream of type `0x2e` in
  the BSP geometry block its `+2` names (`+0x44`, stride `0x38`); the
  streams step (`0x0001c8c0`, `0x0001ca25`) adds it with its own type's
  format. The host loads that bitmap as stage 3 decodes it and takes it
  only when it is uniform (every texel of every level the same): then it
  samples the same wherever the coordinates fall, and the lit build feeds
  the stream as zeros instead of reading the BSP. A bitmap that is not
  uniform keeps implementation 0 on D3D, with the reason; reading the
  BSP's stream per object section is what that would need.
- **Combiner constants** (`object_pixel_uniforms`). A lighting vector
  bound to a pixel combiner constant is packed per object as `0x00019ca0`
  packs an engine value (`pack_engine_colour`: alpha 0, `(x + 1) / 2`
  first where the binding halves it, each channel rounded to 8 bits) into
  the byte lanes the binding keeps, the rest as compiled. A lighting
  scalar goes through its binding's transform first
  (`tags::material::engine_scalar`, the one `0x00019ca0` applies: the
  complement, or a cosine or sine wave over one period clamped to 0..1),
  which the binding keeps.
- **Which implementations a map draws.** The observer counts live layer-3
  object items by their record's implementation and logs the counts once
  per map, after 600 views.
- **The node rows at the hook** (`objects::record_rows`). The observer
  copies, for each accepted item, the rows its record's draw would
  upload; an item whose rows do not read keeps its D3D draw. Under
  the pipelined render this reads the render's snapshot, as the rest of
  the observer does (below): the node blocks at `0x00487b1c..` are guest
  RAM, so they are frame `k`'s. An object of a definition with lit parts
  is drawn only where its implementation's lighting inputs (and the
  root value inputs 40 and 37) read. An unavailable input leaves setup to
  the geometry handler.
- **The draw** (`LitObjects::encode`). One render pass over the same
  targets: per planned item, in the list's order, its vertex
  block (`object_vertex_uniforms` over the view's `c[0..18]`) and pixel
  block in one buffer at dynamic offsets, as the lights do; the viewport
  is the whole target, as the programs' own fix-up expects.

### Memory and host state

The observer reads the object array and bodies during the render. The node
matrices are copied out of guest memory at the hook into the pass
(`ObjectFrame`), and nothing reads guest memory after it. The observer is
one hook (context-only, nothing after the body, the restore at the next
layer draw). The host's writes are its map state (a mutex only the
observer takes, with the model loader on host threads) and a reservation
on the device for the current view, which commits in render order at the
hook.

### Per frame on the CPU

Per view: one walk of layer 3's list, one object lookup per drawn object,
a copy of its nodes (a biped has about 50: 2.6 KB) and one sphere test.
The render thread writes two storage buffers and issues one indexed draw
per (object, opaque part), grouped by model and material. Nothing per
vertex and no matrix work on the CPU.

## Modules

- **`native-render`.** `model` (the game's node matrix, `Model::build`:
  one vertex and index buffer per model in rest space, vertex node indices
  through the section's node map, one draw per opaque part sorted by
  material, the inverse rest frames; `Model::skin`, the shader's
  arithmetic on the CPU), `model_load` (definition -> `hlmt` -> `mode`,
  the section pick of "Permutations and regions", the inverse frames from
  node `+0x28`), `objects` (the handle lookup, the node block at body
  `+0x114`/`+0x116`, the sphere; `clear_objects`, `keep_only`, `restore`
  on the base layers' layer 3), `instances` (per view: the node array,
  one instance record per object, a draw per (object, part) sorted by
  model and material, the sphere-frustum cull, the node-count guard),
  `model_gpu` (`GpuModels`: models added one at a time, per view one
  uniform and two storage writes, one pass with an indexed draw per
  (object, part), `instance_index` the object) and `models.wgsl`
  (skinning `Σ w · N · I · p`, base map times ambient plus one
  directional term, mixed towards the instance's highlight, the W
  depth). Tests: the unit tests in each module on
  invented models and synthetic guest memory; `tests/models_gpu.rs` draws
  an invented two-node model whose square hangs on a turned node, checks
  colour (lit by the skinned normal), W depth, one object culled and one
  with the wrong node count left out.
- **The feature `native-objects`** (`crates/rewrites/render-bridge/src/native_objects.rs`).
  One observer before `LAYER_DRAW`, inside `render-timing`'s and the
  world's (`after_feature` on both), context-only. Layer 3 at the main or alternate scene call opens the
  sorted object walk with that view's kind. The geometry handler consumes contiguous admitted runs
  after reservation. The packets carry the current view's constants and a scissor scaled to each copy. Tests:
  `crates/rewrites/render-bridge/tests/native_objects.rs` (the plan with `render-timing`,
  deep, the layer-draw port and the world; with the world alone the chain
  runs on the render thread; the switch off; the observer on
  `FlatMemory`, with the restore at the next layer draw and a rebuilt
  list left alone). Boot plans it (`registration.rs`, in `ALL_ON` of the
  registration tests); `render_thread`'s test checks the layer draw is
  render-thread safe with the world and objects on (see "Pipelined
  render").
- **Boot glue** (`crates/extensions/game/src/native_objects_glue.rs`).
  The map worker owns the definition scan and GPU upload, palette
  definitions first. A validated model contributes its supported parts to
  draw admission. The worker reports how many definitions it indexed and
  how many failed to preload. Actual native submissions are counted at
  the scene walk or the indexed emitter.
- **Diagnostics.** Every layer-3 call of the observer ends in one counted
  `Stop`: first person (`[esp]` is `0x00040136`, left to D3D), wrong caller
  (`[esp]` is neither that nor `0x0002bed5`), no host, no camera, no map, no
  list, no object items (no item's record names a live object), none
  accepted (models not loaded, refused, or shaders not native), none read
  (node count), draw refused, drawn. The first of each prints
  `native-objects: first layer-3 call ending in "<stop>": <detail>`; each
  distinct wrong caller prints its `[esp]` (up to four); every 1,800
  layer-3 calls one line counts them all, with the definitions wanted and
  the summed walk (items, in layer, geometry builds, object records, live,
  accepted, failed writes, and up to four other build functions met in
  layer 3). A run that draws nothing says which return it took.

### Other builds in layer 3

The walk's `other builds [0x0004de20 x15580]` are decorators (Likely):
`0x0004de20` is the build the layer draw's handler table (`0x00041640`)
gives the BSP's decorator placements (`[[0x004e0348] + 0x238]`, entries
of `0x18` bytes whose first byte indexes the scenario's decorator palette
at `+0x37c`, 8 bytes each with the set's datum at `+4`). They have no
render record, are not objects, and neither skip touches them (both take
only build `0x0004cbf0`): they stay on D3D. Their own stage is
[native-decorators.md](native-decorators.md).

## Not verified

- Whether the game composes the same `N · I` for its own node constants
  (the draw side is checked by a frame A/B).
- Which regions the game hides that the viewer-style first permutation
  draws.
- Which split of layer 3's D3D calls belongs to the world and which to
  objects (`render-census` can give it).
- A frame comparison of the first-person path, its lights' layers 12
  and 13 included.

## Related

[native-renderer.md](native-renderer.md), [native-world.md](native-world.md),
[objects.md](../formats/objects.md), [lighting.md](../formats/lighting.md),
[notes/native-ab.md](notes/native-ab.md).
