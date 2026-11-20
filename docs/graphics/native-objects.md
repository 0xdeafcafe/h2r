# Native renderer stage 2: objects and skinning

Part of [native-renderer.md](native-renderer.md).

## Stage 2: objects and skinning (design)

Bipeds, vehicles, weapons, equipment, scenery, machines and crates: every
object whose render model the game queues in the scene list. Geometry is
resident per render-model tag, loaded once per map; each frame the CPU
hands over only the drawn objects' node matrices, read from the game's own
computed transforms in guest memory. Skinning runs in the vertex shader.

### Native scene object walk

The walk is on whenever the native renderer is
(`impl.render.native=off` turns it off with every other stage).

The walk covers the third-person base-layer call to `LAYER_DRAW`
(return `0x0002bed5`). It reads the private sorted entries at the first
`LIGHT_GEOMETRY_DRAW` (`0x0004d0b0`) reached from `LAYER_ENTRIES`
(return `0x0003bc90`, or the layer port's authenticated continuation).
The guest has selected visibility, objects, regions, permutations, LODs
and shader implementations. The draw handler has yet to upload the
mesh streams and node rows, evaluate shader parameters, set render
state, or issue any D3D draw for that part.

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

The activation hook starts an eager scan of the mounted map's object
definitions. The worker decodes each distinct render model once,
including all its sections, and queues its GPU upload during the load.
A later spawn uses the same resident model. A scene that arrives before
its models or bindings are ready draws those objects on the guest.
A map activation or snapshot reload changes the generation, so work for
a previous load cannot suppress any of the new load's draws.

#### Frame stats and gameplay A/B

The `native_objects_walk_native` and `native_objects_walk_fallback`
columns count objects per view. Fallback reason columns have the same
prefix and end in `unsupported`, `order`, `model`, `inputs`, `state`
or `reservation`. `walk_unreadable` counts views whose sorted list or
ownership could not be established. These views may have an unknown
object count.

Use the same 03a Old Mombasa firefight with at least six visible enemies,
the same camera, input and render scale, and a warmed map load for both
captures. Keep the other render switches fixed. The walk's own A/B
used a per-walk switch that is gone; compare `H2_FEATURES=render-timing`
against `H2_FEATURES=render-timing,impl.render.native=off`.

Enable `H2_FRAME_STATS=1` for both. Collect the same window with
`profile.capture` through the [agent API](../runtime/agent-api.md), or
stream it with `H2_FRAME_STATS_CSV`. Compare the `render` and `d3d`
phases, layer-3 and shader-begin time, object walk counters, and the same
view's images. Check biped animation, held weapons and vehicles through
fog, and the depth, stencil and draw order against the original path.
The `impl.render.native=off` capture is the all-D3D reference.

### Research points (Code unless marked; `h2 disasm`)

| Point | What |
|---|---|
| `0x004e0300` (`points::OBJECTS`) | The object datum array header `H`. Header of handle `h`: `[H+0x44] + 0xc·(h & 0xffff)`; salt `u16 +0` must equal `h >> 16`; body `[header+8]` ([live-gamestate.md](../live/live-gamestate.md)). The pool compacts, so a body address is read every frame, never kept. |
| body `+0x00` | The object definition tag datum (`bipd`, `vehi`, `weap`, `scen`, ...). |
| body `+0x30`, `+0x3c` | World bounding sphere: centre (3 floats), radius ([occlusion-culling.md](occlusion-culling.md)). |
| body `+0x114` (u16), `+0x116` (i16) | **The node-matrix block**: its size in bytes and its offset from the body. `object_new` `0x000b7b40` reserves it at `0x000b800b..0x000b801e` with `0x000bc380(field 0x114, 0x34 · nodes, align 0)`, the node count from hlmt `+0x78` (at least one). `0x000bc380` writes a block reference as `+0` size and `+2` offset (`0x000bc427`, `0x000bc430`). The other blocks it places: `+0x10c` and `+0x110` (32 bytes per node, when animated), `+0x118` (10 bytes per region), `+0x11c` (8 per attachment), `+0x120`, `+0x124` (`0x18` per change colour), `+0x128`. |
| node matrix (`0x34` bytes) | The game's `real_matrix4x3`: `+0` scale, `+4` forward, `+0x10` left, `+0x1c` up, `+0x28` position. A local point `p` goes to `scale · (p.x·forward + p.y·left + p.z·up) + position`: the bounding-sphere update `0x000bdf90..0x000be068` applies the root node exactly so, reading the block through `[body+0x116]`. The matrices are world space (the sphere it produces is the world sphere at `+0x30`). |
| mode node `+0x28` (`0x34` bytes) | The node's inverse rest frame in the same layout (scale, then the inverse's three columns, then its position; `crates/h1models/src/tests.rs`, `node_inverse_frames_are_stored_as_columns`, and the retail check `retail_h2_mode_layouts`). A retail hlmt node is its mode node's first `0x5c` bytes (V, `crates/tags/src/defs/hlmt.txt`), so object node `i` is mode node `i`. |
| skinning | Vertex world position `Σ wₖ · N[nₖ] · I[nₖ] · p`, with `N` the object's node matrix, `I` the mode node's inverse rest frame, `nₖ` the section's node map applied to the vertex's node index (S: that the game composes the same `N·I` for its node constants; "rest frame times inverse is the identity" is Test in h1models, the draw side is checked by the A/B). Sections without vertex nodes (classification 1) are bound to the first node of their node map, else node 0 (S). |
| `0x0002cbf0` | Fills render record `0x004ba138 + 0x20·i` (at most `0x352` per frame, counter `0x004c0b78`): `+4` its second stack argument, the render model (mode) tag datum (`-1` for the BSP), `+8` a word from the caller's per-section state, `+0x10` its fifth, the object's lighting (below), `+0`, `+0xc` and `+0x1a..+0x1b` bit-packed. Its caller for objects is `0x00133f70`, which walks a per-object render state built under `0x00133ab0` (`+0` the object handle, count `+0x166`, per-section words from `+0x48` with stride 4: the lighting pointer at `+0x48 + 4i`, the mode datum at `+0x5c + 4i`, both filled by `0x0003d4f0` and `0x0004b5d0`). **The record does not hold the object handle** (the first live run's walk met 209,131 object records and resolved none by reading `+4` as a handle). Which bits are the section (LOD and permutation) is not read yet. |
| record `+0x10` → cache record → owner | How an object's render record names its object (`objects::record_owner`): `+0x10` points at `+0x44` of the object's record in the render-state cache (`0x0004b5d0`: `lea ebx, [record+0x44]`), or is 0; the cache record's `+4` is the owner's handle; the owner's body `+0xcc` holds the cache record's handle, which `0x0003ddd0` checks (`[data + 0x100·(body[+0xcc] & 0xffff) + 4] == handle`). The resolve requires all of it (pointer at a record's `+0x44`, index below the cache's high water `+0x38`, owner live by `0x000badc0`'s rules, `+0xcc` naming the record back); a record that fails stays on D3D. Code; Test on synthetic memory, Live pending. |
| `0x000badc0` | The object lookup: slot `handle & 0xffff` below `[H+0x38]`, element `[H+0x24]·slot + [H+0x44]` (12 bytes; `0x0003ddd0` uses a fixed 12), live when its `u16` salt is non-zero and equals `handle >> 16`, body at `+8`. `objects::body` follows it. |
| `0x0002bed5` | The return address of the base layers' layer-3 call (`call 0x00044550` at `0x0002bed0` in `render_scene_passes`, view kinds 1 and 2). The object skip acts only there, so first-person or other-kind layer draws are never touched. |
| `0x00040136` | The return address of the other layer-3 caller (about a third of layer-3 calls in the first live run): `call 0x00044550` at `0x00040131` in the first-person draw `0x0003fdf0`, which runs after the transparents for its own record list (`0x004c8740`, `[0x0050943c]` records) under the first-person camera and projection, then `0x000445d0`. With the native renderer on, `feature::native_first_person` owns this caller's eligible items with the device's first-person view constants; the objects observer leaves it alone. |
| `0x0004baf0`, `0x0004b5d0` | LOD choice by pixel size inside `0x00133790` ([occlusion-culling.md](occlusion-culling.md)). |
| `0x0016d764` | Region permutation choice (Likely): the variant's region map, its regions at `+0x18`, then `0x0016dad0` ([objects.md](../formats/objects.md)). |
| `0x00165f16` (`points::FIRST_PERSON_RENDER`) | One local player's first-person weapon and arms, after the transparents, with their own projection (camera `0x0023c110`, FOV `0x00187130`). |
| `0x0003ddd0`, `[0x00509434]`, body `+0xcc` | The per-object render-state (lighting) cache: records of `0x100` bytes, lit per frame by `0x0003dfe0`, lighting at `+0x44..` and `+0xc0..` (layout not read; [pipelined-render.md](pipelined-render.md)). |

### Resolved-draw path with the walk disabled

With the walk off, the layer observer plans items before the layer draw
and pairs them with their eventual indexed D3D submissions. Guest shader
and geometry preparation runs in this mode.

The original mask design used the same layer-draw observers as the world (`LAYER_DRAW`, an opaque
point), under the same plan rules: **before** the layer draw, when the
current layer is 3 and the caller is the base layers' call (`[esp] ==
0x0002bed5`), walk the current list; an item is an object item when its
build is `0x0004cbf0` and its render record's `+4` is a live object
handle. It is skipped when the host draws that object's definition and
the item's shader (`+8`) is one of the host's native materials: layer 3's
bit cleared, the item remembered, the handle added to the view's drawn
set (once). The bits go back **before the next layer draw** (5 and up
follow 3 in the same view), on items that still hold the mask the skip
left, as the world's do. The host gets one
external pass per view carrying, per drawn handle, the definition and the
node matrices copied out of the body (`0x34 · count` bytes) and the
bounding sphere.

Only layer 3 in the first cut, as in stage 1: the objects' layer 1, 5, 7,
8 and 20 entries and their dynamic-light layers (10 to 13) keep drawing
through D3D, so self-illumination, environment maps, specular, change
colour and dynamic lights still come from the game's own passes on top of
the native base. Those passes depth-test `LessEqual` against the depth the
native pass wrote; skinned on the GPU in float, the native positions match
the translated vertex programs to within an ulp or two, not bit for bit,
so an equal-depth pass can lose pixels on curved surfaces (the A/B shows
how much). The skipped layers are one mask
(`native_render::objects::REPLACED_LAYERS`); adding layers later is a mask
change plus the shading those layers did.

What the native pass draws is exactly the set of objects whose layer-3
items it skipped. The game's own visibility, object list, attachment
recursion and "not drawn" decisions (`0x00133790`, occlusion) have already
run, so a held weapon, a vehicle's driver or a culled crate need no logic
of ours. A conservative sphere-frustum test runs anyway (cheap, and a
guard against a stale sphere).

### Transform source

Node matrices are read once per view at the hook, from the body
(`[body+0x116]`, `[body+0x114] / 0x34` nodes), after the game has finished
the frame's animation and attachment updates (the records exist, so
`0x00133ab0`'s node pass ran). They are uploaded raw (13 floats a node)
into one storage buffer per pass, with one small instance record per
object (node base, inverse base). The inverse rest frames are resident
per model. No matrix product is done on the CPU. An object whose node
count disagrees with its model's stays on the D3D path.

### LODs

The game picks a LOD per object by pixel size, and each render record
names the section it draws (the key's bits 9..17). The parts drawn
through the game's programs are drawn by that section: an object's parts
come from the sections its records name, nothing else. So the load takes
every detail level's section of the permutations picked
(`model_load::pick_lod_sections`, highest first per region), and a
distant object draws the lower level the game picked. Before, only the
highest level loaded, and a distant object's records named a section the
pass had no parts for: its items were skipped and nothing drawn (a thin
object on 03a's rubble, gone at frames 3000 and 4000).

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

The first-person weapon and arms are drawn by the player chain after the
transparents (`0x0002afa0`, `FIRST_PERSON_RENDER`) with their own
projection, not by the base layers' layer-3 call, which the caller check
pins. With the native renderer on (the default),
`feature::native_first_person` plans the layer-3 call returning to
`0x00040136` in each of the two `0x0003fdf0` invocations when it has one
in-layer item. The native path reserves the draw before clearing its flag
and restores the flag at the next layer draw. Mixed lists can issue native
contiguous ranges after the guest sort, as described below. The `0x000404b0` light step and the transparent call
after the second model draw stay on their original paths. The local
player's third-person body draws natively like any biped when the game
queues it.

`native_first_person_glue` and `native_first_person_load` are separate from
the objects stage's cache, but use its public model, stream and object
program code. `begin_external_boundary` drains preceding packets before the
entry states and first-person view constants are read (the camera itself is
built by `0x0023c110` upstream), rather than copying the scene camera. The
pass requires a known W-buffered depth mode and a full-target viewport with
a `0.0..1.0` depth range (split or partial views stay on D3D), uses the
device's W-clip and per-call inherited cull, and loads depth/stencil: first person's flag
disables stencil writes, so no scene stencil is replaced. Record `+4`,
not the owner definition's third-person model, names the `mode` to load;
every section of that mode is built because the record key selects the
permutation and LOD. The observer pins the item array `0x004c8740` and
bounds each record index by `[0x0050943c]`; the records themselves use the
same `0x004ba138` array as the other geometry draws. The owner's lighting
and the record's node rows are resolved as for native objects, without
requiring the body and mode node counts to match. Admission is per item:
`ItemKey` selects the exact section, geometry part, shader and
implementation, and `LitObjects::plan` resolves every constant and every
inherited `EntryStates` field before `Host::draw` returns. The mask walk
runs on a `MaskPreview`, leaving guest RAM untouched while each required
copy is prepared. `prepare_external_bound` validates every model and index
range; only then does `ItemMasks::apply` select the planned items and
`ready.commit` release the copies. A refused copy, changed generation or
failed mask application restores the entire candidate list. Mixed record
models under one owner are therefore independent draws. An incomplete or
non-geometry list leaves the whole call on D3D, and a mode with a refused
material or a texture-accumulation dependency stays there too. Loader
publication is pinned to a map generation, including A → B → A transitions.
The first-person lights themselves are not native yet.

The path logs native draws,
objects, skipped items and queue refusals every 1,800 first-person layer-3
calls; a failed reservation leaves every candidate on D3D rather than
dropping a planned item. Synthetic tests cover caller selection, record/body
model divergence, per-item masks and rows, mixed record modes,
incomplete-list refusal, immediate and next-layer restoration,
and rebuilt-list handling. The live 03a A/B is still pending.

### Shading

Like the world's first cut: the material's base map (the same name rule,
`load::base_map`) times a lighting term. The game's per-object lighting
lives in the render-state cache (`0x0003ddd0`, body `+0xcc`, lighting at
record `+0x44..`), whose layout is not read; until it is, the term is a
fixed ambient plus one directional light (`ambient + diffuse · max(n·l,
0)`), so objects read as lit solids, not as their final look. Reading the
cache's lighting (ambient, the dominant light's direction and colour, the
probe's spherical harmonics, [lighting.md](../formats/lighting.md)) is the
first look step. Materials without a decodable base map keep their D3D
draws (the skip leaves their items alone) and are not drawn natively.
Transparent parts (kind 4) are not in layer 3 and are left out.

Base maps go through the world's stage decoder (`StageDecoder`, one per
map's model worker): a palettized one (P8, P8 bump) is expanded through
the device palette bound, when the map's first object was asked for, at
the stage its material binds it (the first layer-3 implementation that
names the bitmap; stage 0 otherwise), as the D3D path samples it, not
drawn as its index in grey. The objects draw only while the palettes the
expansions used stay bound (checked at each layer-3 view); a stage
without a full palette refuses the material.

### Object lighting

The first lit run (03a, objects native, 4,677 of 7,202 layer-3 calls
drawn) drew the wall vent flat, bright grey-white where the D3D path
shades it dim: the fixed term above is not the game's lighting, so a
material that reads the object's lighting must not be drawn with it.
How the game lights an object's layer-3 pass (Code, `h2 disasm`;
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
- **The refusal.** The native models pass does not apply them: its
  program is ours, not the game's `lightmap_dynamic`/`sh_dynamic`. So no
  opaque material is drawn by it (below): one none of whose layer-3
  implementations reads only what the object path gives is refused at
  load and keeps its D3D draws (`object_program::program_refusal`, which
  names what each implementation reads that the path cannot give, or says
  it has no layer-3 pass).

### Object programs

Every opaque object material, whether or not its layer-3 pass reads the
object's lighting, is drawn with the game's own vertex program and pixel
shader (`native_render::object_program`, `object_program_gpu`), or left
to D3D, each piece with synthetic tests. The models pass's base-map
stand-in (base map, fixed ambient and directional light, posed from the
object's own node matrices) is no longer used for opaque parts: on 03a's
intro (frame 02000) the scarab's materials that read no lighting were
drawn by it, flat and dark where the game lights them, and posed apart
from its parts drawn through their programs, which use the rows its
records upload and matched D3D pixel for pixel. The native set accepts a
shader per definition (`NativeSet::shaders` holds `(definition, shader)`):
a material one model draws through its programs may be refused for
another whose sections take other programs, and that one's items keep
their D3D draws.

A material with no layer-3 pass ("it has no layer-3 pass" in the load
log) is not drawn at layer 3 at all: its items sit in other layers, which
the objects stage does not touch, so they keep their D3D draws while the
definition's layer-3 materials go native. On 03a masterchief
(0xf40a1293) and 0xe703058e each have one, beside a material whose every
implementation samples engine texture 3.

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
  and winding (before, it culled nothing). S: the layer's earlier draws
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
  `ResolvedImplementation::vector_bindings`, the scalar ones through the
  new `scalar_bindings`, both keeping only what later bindings leave);
  `c[50..]` the node rows; `c[170..173]` the decompression.
  `inputs_supported` refuses an implementation that reads any other
  per-draw input (time, the camera, object functions) or puts a lighting
  input into a combiner constant (that would need a per-object pixel
  block).
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
  on). They were expanded to floats on the CPU before, and a CPU division
  can round another way than the GPU's normalised conversion: a part
  whose layer-3 pass tests depth `Equal` against the depth its D3D pass
  wrote with the same program (03a's `concrete_rebar`, part 2 of
  `concrete_chunk_j`, at frames 3000 and 4000) then failed wherever w
  differed in the last bit, as the world's packed normals did.
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
  do), is drawn; one that does not is left out with its reason, and a
  material with none stays on D3D whole. An object whose records pick an
  implementation some lit part of its definition lacks stays on D3D.
- **The normalization cube.** A stage sampling engine texture 1 is bound
  to the vector normalization cube the lights load
  (`native_lights_glue::light_textures`, the globals' rasterizer data),
  sampled as a cube without a bias, as the lights sample it.
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
  which the binding now keeps.
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
  root value inputs 40 and 37) read; else that item stays on D3D.
- **The draw** (`LitObjects::encode`). One render pass over the same
  targets: per planned item, in the list's order, its vertex
  block (`object_vertex_uniforms` over the view's `c[0..18]`) and pixel
  block in one buffer at dynamic offsets, as the lights do; the viewport
  is the whole target, as the programs' own fix-up expects.

### Pipelined render

The observer reads the object array and bodies during the render. With
`impl.render.pipelined_frame` the render runs one frame behind the simulation on
its own thread, while the next tick moves the same bodies and may compact
the pool. That is already safe: on the render thread the observer's
memory is the render's copy-on-write snapshot of the frame
(docs/runtime/second-executor.md; `Obs::guest` is the executor's own
runtime), so the handles, bodies and node matrices it reads are frame
`k`'s, the same frame the rest of the render draws, and the tick's writes
to live RAM never reach it. The node matrices are copied out of that
snapshot at the hook into the pass (`ObjectFrame`), and nothing reads
guest memory after it, so the snapshot copy the design asked for is the
pipeline's own snapshot plus that copy. The observer is the one hook
(context-only, nothing after the body, the restore at the next layer
draw), so the pipelined render runs it on its own thread like the
world's. The host's writes are its map state (a mutex only the observer
takes, with the model loader on host threads) and a reservation on the device
for the current view. The reservation commits in render order at the hook.

### Per frame on the CPU

Per view: one walk of layer 3's list, one object lookup per drawn object,
a copy of its nodes (a biped has about 50: 2.6 KB) and one sphere test.
The render thread writes two storage buffers and issues one indexed draw
per (object, opaque part), grouped by model and material. Nothing per
vertex and no matrix work on the CPU.

## Stage 2 as built

Not compiled or run by the lane that wrote it (the coordinator builds and
tests); evidence labels below are what the tests will show once run.

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
  directional term, the W depth). Tests: the unit tests in each module on
  invented models and synthetic guest memory; `tests/models_gpu.rs` draws
  an invented two-node model whose square hangs on a turned node, checks
  colour (lit by the skinned normal), W depth, one object culled and one
  with the wrong node count left out.
- **The feature `native-objects`** (`crates/feature/src/native_objects.rs`,
  on by default with the rest of the native renderer).
  One observer before `LAYER_DRAW`, inside `render-timing`'s and the
  world's (`after_feature` on both), context-only. First it sets back the
  bits the last base-layer layer 3 cleared (items still holding the mask
  it left). Then, only for layer 3 called from `0x0002bed5`: asks the host
  for its native set (definitions with their node counts, shaders),
  clears the accepted items, asks the host to load any definition it has
  not seen, copies the cleared objects' nodes and spheres, gives back the
  bits of objects it could not read or whose node count disagrees, and
  asks for the pass (all bits back if the device cannot). Tests:
  `crates/feature/tests/native_objects.rs` (the plan with `render-timing`,
  deep, the layer-draw port and the world; with the world alone the chain
  runs on the render thread; the switch off; the observer on
  `FlatMemory`, with the restore at the next layer draw and a rebuilt
  list left alone). Boot plans it (`registration.rs`, in `ALL_ON` of the
  registration tests); `render_thread`'s test checks the layer draw is
  render-thread safe with the world and objects on (see "Pipelined
  render").
- **Boot glue** (`crates/boot/src/native_objects_glue.rs`). Per map, a
  worker with the map, its shared maps and the object tag context open;
  definitions arrive one at a time from the feature and are loaded once;
  a model is queued for upload before its definition is published, so a
  pass that sees it finds it. Materials whose base map does not decode
  leave the model and keep their D3D draws. The render thread uploads the
  queue at the next pass. Log lines: `native-objects: <map>: definition
  <datum>: D draws, N nodes, in T ms (L models native, R left to D3D)`, or
  `... left to the D3D path (<why>)`; every 1,800 views
  `native-objects: D of V layer-3 views drawn natively, O objects, I
  object items skipped`.
- **Diagnostics** (added after the first live run drew nothing and
  printed no `native-objects:` line at all). Every layer-3 call of the
  observer ends in one counted `Stop`: first person (`[esp]` is
  `0x00040136`, left to D3D), wrong caller (`[esp]` is neither that nor
  `0x0002bed5`), no host, no camera, no map, no list, no object items (no
  item's record names a live object), none accepted (models not loaded,
  refused, or shaders not native), none read (node count), draw refused,
  drawn. The first of each prints `native-objects: first layer-3 call
  ending in "<stop>": <detail>`; each distinct wrong caller prints its
  `[esp]` (up to four); every 1,800 layer-3 calls one line counts them
  all, with the definitions wanted and the summed walk (items, in layer,
  geometry builds, object records, live, accepted, failed writes, and up
  to four other build functions met in layer 3). A run that draws nothing
  says which return it took.
- **The first diagnosed run** (03a, world D3D) printed `items 4 in layer
  4 geometry 4 with handles 0 live 0`, then over 7,202 layer-3 calls
  209,131 records "with handles" and none live, and 2,521 calls from
  `[esp]` `0x00040136`. Both had one cause each: the walk read a render
  record's `+4` as the object handle, but it is the render model datum,
  so no lookup could succeed; the object is named through the lighting
  pointer at `+0x10` instead (research table, `record +0x10`). The other
  caller is the first-person draw, now counted on its own and left to
  D3D. The transparents' entries (stage 3) read their objects the same
  way.

### The live A/B

Old Mombasa, E1 off, frame stats and `render-timing` on, the same spot
with the native renderer off and on (the stage-alone A/B used per-stage
switches that are gone). What passes:

- the load lines name the map's models, most as native; the refused ones
  give a reason (the run-time permutations: marines, some scenery);
- the views line shows every base-layer layer-3 view drawn natively, and
  objects and items per view in the tens;
- the frame shows every object in place and posed: bipeds animate (a
  T-pose or exploded limbs mean the `N · I` reading is wrong), held
  weapons sit in hands, vehicles and their turrets turn; flat-lit bases
  with the game's own self-illumination, specular and dynamic lights on
  top (those layers still draw through D3D);
- the first-person weapon and arms are unchanged (still D3D);
- the `0x0002bed0` site (layer 3) and `cpu` drop by the objects' share of
  layer 3's shader draws (below), `encode` and `submit` with them;
  `external_passes_dropped` stays 0 (a growth there is an object hole).

What this stage should remove: layer 3 is 8.26 ms on the census run,
about two thirds of it per-draw guest work in `0x00016b90` and the rest
D3D; objects are roughly a third to a half of layer 3's 1,882 D3D calls
on a populated 03a view (S: the split between world and object draws is
not measured; `render-census` can give it). So with the world already
native, expect about 2 to 4 ms off `0x0002bed0` and `cpu`, plus their
`encode`/`submit` share on the render thread. Layers 1, 5, 7, 8 and the
dynamic-light passes keep their object draws in this cut.

### Other builds in layer 3

The walk's `other builds [0x0004de20 x15580]` are decorators (Likely):
`0x0004de20` is the build the layer draw's handler table (`0x00041640`)
gives the BSP's decorator placements (`[[0x004e0348] + 0x238]`, entries
of `0x18` bytes whose first byte indexes the scenario's decorator palette
at `+0x37c`, 8 bytes each with the set's datum at `+4`). They have no
render record, are not objects, and neither skip touches them (both take
only build `0x0004cbf0`): they stay on D3D, a later world stage.
