# Native renderer stage 1: the static world

Status: Built. The static world draws natively in the default configuration.
Code: `crates/rewrites/native-render/src/{world,load,camera,cull,skip,gpu,program,look}.rs`, `crates/rewrites/render-bridge/src/native_world.rs`, `crates/extensions/game/src/native_world_glue.rs`. Tests: `crates/rewrites/native-render/tests/gpu.rs`, `crates/rewrites/render-bridge/tests/native_world.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

Stage 1 of [native-renderer.md](native-renderer.md). The structure BSP's opaque
geometry is read from the map's tags once, uploaded once, and drawn each
frame from the game's render camera with frustum-culled indirect draws,
while the game skips its own layer-3 entries for the parts drawn natively.
Stage 1b adds the world's other layers (1, 5, 7 and 8). The rule that must
hold: a part the native pass draws is exactly what the D3D path would have
drawn, including depth, stencil and the state the layer leaves behind.

## Stage 1: the static world

### Load once

At map load (and on a BSP switch: the active BSP is `0x004e0348`, the
lightmap `0x004e0344`, [structure-bsp.md](../formats/structure-bsp.md)):

- **Geometry.** Every cluster section and every instanced-geometry
  placement (`tags::structure::StructureRender`), decoded by
  `tags::geometry` exactly as the viewer and editor already do. One shared
  vertex buffer (position, base-map UV, lightmap UV, normal) and one
  index buffer. Instances are placed at load (rigid transform + uniform
  scale) so every draw is in world space; the static world never moves.
- **Draws.** One draw per drawn part (`Part::is_drawn`, not kind 4
  transparent): `(cluster or instance, material, lightmap page and
  palette, index range, world bounds)`. Draws are sorted by batch
  (material, lightmap page) at load, so a batch is a contiguous slot range
  of the indirect buffer.
- **Lightmaps.** The `ltmp` group whose checksum matches the BSP's; each
  cluster's and instance's render info picks a P8 page and palette
  ([lighting.md](../formats/lighting.md)). A page is expanded through its
  palette *before* filtering (the palette is an index lookup, not a
  multiplier), into an RGBA8 texture array, one layer per (page, palette)
  pair used.
- **Materials.** The part's material indexes `sbsp+0xa4` (stride `0x20`,
  shader at `+0xc`). Each shader's template and bitmaps decide the
  native program (below). A material the native path does not support is
  marked *fallback* and keeps its D3D draws.
- **Pipelines** are made once per supported template.

### Per frame

- **Camera.** The render camera record `0x004b9da0` (0x74 bytes, filled by
  `0x0002f800` per view): position `+0`, forward `+0xc`, up `+0x18`,
  vertical FOV `+0x28`, viewport `+0x30` (i16 top, left, bottom, right),
  near `+0x40`, far `+0x44` ([occlusion-culling.md](occlusion-culling.md),
  "Camera and view"). From it, a view matrix and a projection with the
  game's own aspect rule (`0x0002fb70`). Read once per view, at the layer-3
  hook (below), so it is the camera the game's own draws would use.
- **Cull.** Frustum planes from the view-projection, tested against each
  draw's world bounds on the CPU (a few thousand AABBs: microseconds). The
  result is `instance_count` 0 or 1 in the indirect buffer (one 20-byte
  slot per draw), the only per-frame upload besides one uniform block. The
  frustum alone is conservative: it draws a superset, and depth sorts out
  the rest. The rules that keep the pass equal to the D3D path:
  - Only the four sides and the eye's plane are tested
    (`Frustum::sides`). The render camera's `near`/`far` are not the range
    the programs clip to: distant geometry lies past the camera's `far`.
  - The programs clip w to the device's pushed clip range
    (`SET_CLIP_MIN`/`MAX`, `nv2a::w_clip_range`) and
    `D3DRS_DEPTHCLIPCONTROL`'s clamp, which the game sets per view
    (`0x00015720`). The device gives the pass both at issue
    (`ExternalPass::depth_clip`, `Device::depth_clip`) and the world's
    programs draw with them (`ViewInputs::w_clip`). The objects
    (`ObjectsPass::depth_clip`), the transparent pass and the stage 4
    cluster light pass take the device range too.
  - A view whose draws are not W-buffered (`SET_CONTROL0` without
    z-perspective) is left to the D3D path (counted in the log): the
    programs write and clip w.
  - The world is keyed and loaded by the game's active BSP index (game
    globals `+2`, `i16`; `native_world_glue::active_bsp_index`), which must
    load at that address: a level's BSPs share one address across views.
    A placement's placed copy is unscaled, as the game draws it (`load`).
  - The lighting vector (`v5`) and the frame vectors (`v2..v4`) are
    `NORMPACKED3` words in the stream format table (`0x0043f408`, type
    `0x30`). A world vertex carries them as words (`WorldVertex::packed`)
    and the material programs fetch and decode them as the D3D path does
    (`program::INPUTS`, `WORLD_FETCH`), so a world program's module is the
    D3D path's for the same draw, text for text
    (`a_world_program_s_module_is_the_d3d_path_s`). The CPU's midpoint
    decode (`load::program_lighting_vector`) is not used for the lighting.
  - Stage bitmaps keep their BC blocks (`assets::decode_image_blocks`,
    `Image::with_blocks`) and upload them where the device takes BC, on the
    D3D path's rule (no signed channel: `bias_signed` drops them). The
    CPU's DXT decode truncates interpolated colours and would be one
    level darker. `program::describe_image` prints a stage's native upload
    (format, size, levels, a hash of level 0) beside the D3D texture.
  - A pass that does not set `CULLMODE` inherits the device's
    (`look::WorldPass::inherits_cull`): the pass takes the device's
    `CULLMODE` and `FRONTFACE` at the layer-3 call (`Device::cull_state`,
    read from the guest's render-state array as the stencil is;
    `program::InheritedCull`). Clusters and placements both take it.
  - Pixel-shader fields that change nothing are not matched:
    `explicit_final` (only when both final-combiner input words are zero,
    which a world pass's are not), an all-ones `stipple`, and `fog_color`
    and `bump_mat`/`bump_lum` (read only by a fog-reading combiner and by
    the bump-environment texture modes, which `world_parity` refuses).
- **Draw.** Per batch: bind the batch's textures, then
  `multi_draw_indexed_indirect` over its slot range. One render pass into
  the game's own colour target and depth, with the game's viewport.

### Skip only the replaced work (Code)

The scene list `0x004c1a48` (layer lists at `0x004c1a48 + 0x18·n`) holds
items of `0x18` bytes at `[list+0x14]`, appended by `0x0003b9d0`:

| Item | Meaning |
|---|---|
| `+0x00` | build function (called by `0x0003bab0` per layer) |
| `+0x04` | draw function (called by `0x0003bc30` per entry) |
| `+0x08` | shader datum |
| `+0x0c` | argument: render record index `<< 8`, low byte a part index |
| `+0x10` | layer mask (bit `n`: drawn in layer `n`); the list's `+8` is the OR of all |
| `+0x14`, `+0x15` | bytes passed to both functions |

Structure and model geometry are queued by `0x00041040` through
`0x00040e30` with build `0x0004cbf0` and draw `0x0004d0b0` (pushed at
`0x000412fa`/`0x0004132b`; the transparent path instead records
`0x00041020`). The render record is `0x004ba138 + 0x20·index`
(`0x0002cbf0` fills it); its `+4` is an object's render model tag datum
(not its handle; see "Stage 2" for how a record names its object), `-1`
for the BSP:
the cluster records (`0x00133390`, call at `0x00133504`) and the
instanced-geometry records (`0x00133520`, call at `0x001336d3`) both pass
`-1`, and the build function itself tests `[record+4] != -1`
(`0x0004cc2c`).

So a structure item in layer 3 is: build `0x0004cbf0`, `[0x004ba138 +
0x20·(item[+0xc] >> 8) + 4] == -1`, mask bit 3 set. The skip is one hook
per layer draw, not one per item:

- **before** the layer draw `0x00044550` with the current layer
  `[0x00467130] == 3`: walk the current list (`[0x00467138]`; `+4`
  count, `+0xc` first, `+0x14` items), clear bit 3 of every structure
  item's mask, remember them, and ask `d3d` for the native pass. The
  list's OR word `+8` is left alone: the layer still walks its items and
  finds the world's masks clear;
- **before the next layer draw** (5 and up follow 3 on the scene list,
  in the same view): set the bits back, on each item that still holds
  the mask the skip left. Not after the layer itself: something after the
  body makes the chain call it through a Rust continuation, which the
  pipelined render thread cannot run (docs/graphics/pipelined-render.md,
  "Render-safe hooks").

The layer draw then builds no entry for the world: no build call, no
shader draw, no geometry submission, no push-buffer words. Objects,
transparents, lights and the HUD keep their entries. Guest memory after
the layer is as without the switch except for what the skipped draws
would have written (the shader draw's state cache, D3D's shadows), which
is render state only; the simulation never reads it, and co-op lockstep
hashes nothing of it. The native renderer itself never writes guest
memory.

The structure draw also switches the stencil around an item in layers 1
and 3 (`0x0004d0b0`; modes by `0x00015370`'s table at `0x00015658`):

- before the item (`0x0004d640`): either flag byte `0x00485a75`,
  `0x00485a76` set, mode 0 (stencil off); else, the render record's `+0`
  bit 14, mode 2 (`ALWAYS`, reference `0xff`, `REPLACE` on pass, masks
  `0xff`). Bit 14 is the item's visibility flag `0x200` moved up
  (`0x001332f0`), the same for clusters and placements;
- after it (`0x0004d720`), on either condition, mode 1 (the same with
  reference 0).

An item with neither draws with the stencil as the layer found it.

The feature reads how layer 3's items switch it before asking the host
(`skip::stencil_plan`: the flags, the marked BSP sections by cluster or
placement, whether any item switches; unreadable, the D3D path draws the
view, `native_world::stencil_views`). The native pass draws its parts
before the layer's items and clears their items, so their switches do
not run; the glue (`native_world_glue::pass_stencil`) makes that exact:

- no item switches: every part draws with the stencil the device holds
  when asked (`Device::stencil_state`, translated as the D3D path's
  pipeline translates it, `gpu::PassStencil`);
- some item switches: an item leaves mode 1, so the layer must start in
  mode 1 (`PassStencil::is_structure_mode`) for the items left to D3D to
  draw as they would; else the D3D path draws the view (counted in the
  log). Then with a flag every part draws with the stencil off (mode 0),
  and otherwise a marked section's parts draw with reference `0xff`
  (mode 2, its own run in the batch, `PassStencil::reference_of`), the
  rest with mode 1.

Each part writes what its own item would where it is drawn (`ALWAYS`,
`REPLACE` on pass): which part is last at a pixel is the depth test's,
as on the D3D path (S: coplanar ties may differ). The native pass sets
no device state, so the layer's D3D items and everything after it see
the state the layer started in: mode 1 whenever an item switches, which
is what the game's last switching item leaves (`0x0004d720`).
`native_world_glue`'s test checks the per-mode writes and that state
against `engine::render::stencil::for_mode` over every stored value.
Nothing after layer 3 tests the stencil from engine code: every caller
of `0x00015370` passes mode 0, 1 or 2 (the light passes 0), and only
`0x00015370` and the quad `0x0003bf20` set `STENCILFUNC`
(e3-lighting.md, "Stencil: every use, named"). The transparents and
objects still draw without the stencil (S).

**Only the world's layer 3.** The layer draw also runs for the
first-person render (`0x0003fdf0`, after the transparents): layers 1,
3, 8, 20 and 7 of a list holding the player's first-person models only,
under its own projection, with `0x00485a76` set by its caller (so the
stencil plan says "off") and no depth clear under that flag
(`0x0004004f`..`0x00040066`). About a third of all layer-3 draws are
these. The feature asks for the native pass only when the layer's list
holds a BSP item (`skip::has_structure_items`); otherwise the whole
world was drawn again over the finished scene with the first-person
constants, nearer than the scene's depth: decals, the dynamic lights'
`ZFUNC` equal passes and the transparents (03a's fire and smoke) were
painted over with bare world while the walls looked right. Drawing it would have painted bare world over those.

### Where the native pass goes, and depth

The ask is recorded in order with the draws: the device issues it at the next
drain of the push buffer, which is the first D3D entry inside the layer
draw, so it lands after layer 1's draws and before layer 3's object
draws. At that moment the device knows the bound colour surface, depth
surface and viewport; the backend gets `(colour, depth, viewport, pass)`
and runs the pass's encoder on the render thread with the wgpu views of
those surfaces (`Backend::external_pass`). With the render scale on it
runs in the scaled view; with E1 reprojection the interpolating backend
must record it with the frame so a replay redraws it with the warped
camera (S: not built; see "Not verified").

Depth must agree with what the D3D path writes, because later passes
depth-test against it (layers 5, 7, 8 on the same surfaces, objects,
transparents). The game W-buffers: the stored depth is `floor(w · zs /
far) / zs`, with `zs` the D24 full scale and `far` the camera's far plane
([occlusion-culling.md](occlusion-culling.md), "The W scale"). The
translated pixel programs write exactly that as `frag_depth`
(`shaders::psh::wgsl`, `xps_w_depth`), and so does the native fragment
shader, from the same perspective-correct `1 / position.w`. A world draw
whose layer-1 pass already wrote depth is tested `LessEqual` against it;
one without writes it. Equal-depth passes the D3D path still draws over
the world (layers 7 and 8 in stage 1) only line up when the native clip
position equals the translated vertex program's bit for bit; stage 1b
removes that dependency by drawing those layers natively too.

Every world template also has, after layer 3, shadow application (11),
the lights (12, 13) and planar fog (16), all `ZFUNC` equal (`0x202`)
without depth writes (`h2 shader`), so the native pass must match the
D3D path's depth per part:

- **Clusters**: classification 0 of the pass's vertex program, `v0`
  the section's position. The world keeps a cluster's position bits as
  decoded (`WorldBuilder::add_section`, no identity arithmetic: `0 + -0`
  is `+0`, `x + 0·inf` is NaN), and the program, view constants
  (`c[0..18]` at the issue) and W depth (`floor(w)/16777215` for D24,
  `xps_w_depth`) are the D3D path's.
- **Instanced placements**: the structure draw takes the rigid
  classification for them (render record `+0` bits 3 and 12, set by the
  placement caller `0x001336d3` of `0x0002cbf0`; the cluster caller
  `0x00133504` sets neither), whose program decompresses `v0` with
  `c[170..171]` and places it with `c[50..52]`; `0x0004d830` uploads
  those rows from the matrix at render record `+0x1c`, which for a
  placement is the placement's tag element (`[sbsp+0x144] + 0x58·i`):
  forward, left and up by column and the position, copied bit for bit,
  the scale at `+0` not applied. The native pass does the same: the
  load reads each placement's rows from its tag element
  (`world::model_rows`, `World::rows`), the material's pass is compiled
  with classification 1 as well (`look::rigid_program`,
  `program::compile_rigid`, which requires that it read `c[50..52]`;
  `program::rigid_fit`), and each placement's parts draw with it from
  the definition's vertices as their streams hold them
  (`World::program_vertices`), the placement's uniforms at its slot's
  dynamic offset in one buffer per material (`program::rigid_vertex`,
  the frame's view then the rows). Within a batch the draws sort by
  owner, so the clusters draw as one run and each placement as another.
  `tests/model_rows_differential.rs` runs the lifted `0x0004d830` on
  seeded placements and checks the floats it hands the constant upload
  `0x003f7330` (at `c[50]`, twelve) equal the port's, bit for bit. A
  material without a rigid program (or whose placements lack a stream
  it reads) keeps its placements on the D3D path, by reason in the
  load's log; the lights take only materials with none left there.

### Match the look

The world's layer-3 passes are lightmap passes: the material's base colour
(base map and detail maps, by template) times the lightmap irradiance, with
fog applied later by layer 16. The native pass draws every native material
with its own translated pass: the pass's vertex program (`vrtx`) and pixel
shader definition (`spas`) are what `shaders` translates for the D3D path,
compiled once per material with the view constants the device holds at
issue. A material whose pass does not translate stays on the D3D path;
there is no hand-written look in between.

### How correctness is checked

- **Unit tests** (no disc): world building from invented sections (draw
  ranges, batches, instance placement, bounds), the camera read from an
  invented guest record, frustum culling against known boxes, the scene
  list walk and restore on invented guest memory.
- **GPU tests** (headless wgpu, skip without an adapter): a native draw
  of an invented world into an offscreen target; its W depth equals the
  formula.
- **Frame A/B** (the game): the same 03a frame with and without
  the native renderer (`impl.render.native=off`), compared by pixel diff, through the headless
  capture (`H2_BOOT_GFX`) or the agent API's `capture`. Report the share of
  differing pixels, the largest difference, and an image of the
  differences. Frame stats `cpu`, `d3d`, `encode`, `submit` and the
  `render-timing` site `0x0002bed0` (layer 3) give the CPU saved.

### Modules

`crates/rewrites/native-render/src`:

- `world`: one vertex and index buffer for the whole BSP, one draw per
  opaque part in world space (instances placed at load), sorted into
  batches of one material and one lightmap page; transparent parts and
  bad triangles are counted and left out (Test).
- `load`: the scenario's (BSP, lightmap) pairs, the BSP's material shaders,
  every cluster and placed instance through `tags::structure`, each with
  its lightmap page and palette from the group whose checksum matches; P8
  pages expanded through their palette (Test for the expansion and the
  render-info rules; the map read needs the disc).
- `camera`: the render camera record read from guest memory (rejects NaN,
  empty viewports, `far <= near`), the view, the projection by the game's
  aspect rule, and the W-buffer depth formula (Test).
- `cull`: frustum planes, the conservative box test, one indirect slot per
  draw (Test).
- `skip`: the current list's walk, the structure-item test, clear and
  restore (Test, on invented guest memory).
- `gpu`: buffers and bind groups made once, one pipeline per (colour,
  depth) format pair, `prepare` (cull, uniform, slots) and `encode` (one
  pass that loads and keeps the game's target and depth, one
  `multi_draw_indexed_indirect` per batch). `tests/gpu.rs` draws an
  invented wall at view depth 10 and a culled one behind the camera; the
  colour is base times page and the depth is `w_depth(10, far, D24)`
  within one step (GPU test, skipped without an adapter).
- `program`, `look`, `pass_state`: the material translation below.

### Wired into the game

- **`d3d` external pass.** World and object replacements use the ordered
  reservation in [native-renderer.md](native-renderer.md), "Admission".
  `Device::request_external_pass` schedules legacy passes at the next
  push-buffer drain. `crates/platform/d3d/tests/external_pass.rs` covers both entry
  points, direct and threaded recording, scaled attachment refusal, and
  replay.
- **The feature `native-world`** (`crates/rewrites/render-bridge/src/native_world.rs`).
  `LAYER_DRAW` is an opaque point, so the hook is one observer before it:
  it first sets back the bits the last layer 3 cleared (on items that
  still hold the mask it left), then, with layer 3 current, reads the
  camera and the active BSP (`0x004e0348`) and asks the host; when the
  host accepts it clears layer 3's bit on the BSP items whose part the
  world draws (`skip::clear_parts`). It sits inside `render-timing`'s
  observers (`after_feature`). One observer call per layer draw, nothing
  per draw. It runs on the pipelined render thread (context-only, nothing
  after the body). It touches the guest memory its context sees (item
  masks, camera, active BSP, map name), the published host resources and
  the D3D device's current targets. The loader opens the map through the
  VFS and works from the map and the captured device palettes.
  The host drains previous commands, previews the list and confirms one
  batched reservation for the view before applying masks.
  Tests: `crates/rewrites/render-bridge/tests/native_world.rs` (the plan with
  `render-timing`, deep, and the layer-draw port all on; alone, the chain
  runs on the render thread; on by default, keeping pipelining;
  `impl.render.native=off` places nothing; the skip and its restore at the
  next layer draw on synthetic memory, a rebuilt list left alone, and a
  part left to D3D keeping its bit though its shader is drawn);
  `crates/extensions/game/src/render_thread.rs` (the default plan's `LAYER_DRAW` is
  render-thread safe).
- **Boot glue** (`crates/extensions/game/src/native_world_glue.rs`). On a new (map
  name, active BSP address) a worker opens the loaded map through the VFS
  with its shared maps, picks the scenario BSP whose tag data sits at that
  address, builds the world, decodes each lightmap page (with its mip
  chain), and translates each material's layer-3 pass. After translating
  the material, the worker checks each part against the inputs of its
  vertex classification and its lightmap page. Parts that lack an input
  keep their D3D draws. The skip goes by part: the host returns each
  drawn part, by cluster or placement and part index, with its material's
  shader, and `skip::clear_parts` clears an item only when its part is one
  of them with that shader. An item names its part exactly as the build
  `0x0004cbf0` reads it (`0x0004cec1`..`0x0004cf21`): render record `+0`
  bit 12 clear, the cluster at `+0xc` bits 9..17 (`[sbsp+0xa0] +
  0xb0·cluster`, its section at `+0x50`); set, the placement at bits
  18..28 (`[sbsp+0x144] + 0x58·placement`, its definition at `+0x34`,
  `[sbsp+0x13c] + 0xc8·definition`, section at `+0x50`); then the item
  argument's low byte indexes the section's parts (`[section+4] + 0x48·part`,
  the order `tags::geometry` decodes and `native_render::load` keeps,
  undrawn parts included). A material that shares its shader with one
  left to D3D is native too. The lights' split clears by shader
  (`feature::native_lights`), so the lights take only the materials whose
  shader is native whole. Each layer-3 view then asks the device for a
  `WorldPass` with the view's camera. The map worker uploads the world
  on the backend's device and queue, then makes the light, fog, water,
  scene-layer and decorator resources. Each ready stage transfers at
  `RENDER_VIEWS` before the first view. A busy GPU world defers that
  transfer until another frame. A BSP switch or revert opens a fresh
  receiver and D3D draws its parts until the new stage is ready.
  The final packet releases the BSP's resources to a sleeping utility
  worker for destruction. Log lines per load: `native-world: N of M materials
  draw natively with their own programs (P of Q parts); K stay on the D3D
  path for R reasons`, one line per reason, most parts first; `native-world:
  <map> BSP at <addr>: N draws in B batches, P parts native, in S s`; and
  every 1,800 layer-3 views a drawn-natively tally. A pass that comes
  without the device's view constants draws nothing and says so once.

### Diagnosing a frame that differs

On native surfaces only (the D3D ones are the game's own):

- **speckle or holes on world surfaces**, or objects showing through:
  depth. Layer 1 (D3D) wrote the world's depth and the native layer 3
  tests `LessEqual` against it, so the native W depth must be the D3D
  path's bit for bit; one step deeper fails the test;
- **a whole material too dark, too bright or flat**: its lightmap (the P8
  page through its palette, bound at stage 3), its bump map (`P8Bump`
  through the device palette at stage 2) or a constant;
- **shimmer or blur at a distance**: sampling (the bump stage is 4x
  anisotropic; mip chains);
- **cut-out foliage or grates filled in or missing**: the pass's alpha
  test;
- **stencil-driven effects** after layer 3: see "Skip only the replaced
  work";
- **decals, lights, fire and smoke gone, walls right**: the world drawn
  over the finished scene by a native pass in a layer 3 that is not the
  world's (the first-person render's; `skip::has_structure_items`).

### What a world pass reads (Code, `h2 disasm`, `h2 shader`, `vsh` disassembly)

A world material's layer-3 pass, as `tags::material` resolves it: on 03a
the opaque world templates are `tex_bump` and its variants (`_env`,
`_detail_blend`, `_three_detail_blend`, `_two_detail`, ...). A `tex_bump`
layer 3 is one pass (`lightmap_tex_detail`) with three implementations:
per-pixel lightmap (stage 3 the lightmap), per-vertex, and dynamic. The
engine picks the implementation per draw; the BSP's lightmapped sections
take the per-pixel one (S, from the lightmap bind at `0x0004d40f`).

The pass's vertex program comes from its `vrtx` tag's geometry
classification. Classification 0 is world space (S: its program reads
`v0` as a world position and applies no model transform). For the
per-pixel `tex_detail` program it reads:

| Input | Meaning | Our source |
|---|---|---|
| `v0` | world position | section positions |
| `v1` | texture coordinate | section texcoords |
| `v2`, `v3`, `v4` | the tangent frame (normal, binormal, tangent) | section tangent-frame stream |
| `v5` | the lightmap bucket's signed lighting vector (stream `30`, [lighting.md](../formats/lighting.md)) | bucket stream's words, decoded in the shader as the D3D path does (`NORMPACKED3`, not the CPU's midpoint decode) |
| `v6` | lightmap coordinate | bucket stream `1f` |

and these constants (hardware numbering, D3D register + 96):

| Constants | What | Set by | For the native pass |
|---|---|---|---|
| `c[0..3]` | view-projection rows (`dph` with `v0`) | per view, in one block upload | the device's values at the pass (below) |
| `c[5].w`, `c[6]` | the scale that packs a unit vector into a colour (S: `0.5`, `n·s + s`); a world-space vector (dotted with the tangent frame into `oD0`) | per view | the device's values |
| `c[10]`, `c[11]` | the viewport scale and offset (`oPos` fix-up) | per view | the device's values |
| `c[12..17]` | six registers in one block | `0x00025c70` | the device's values |
| `c[18..20]` | stage 0..2 texture-coordinate scale (xy) and offset (zw) | the instance (`0x00018e80`) | `tags::material`'s instance constants |
| `c[50..52]` | model transform rows | `0x0004d830` from the render record's matrix, skipped for clusters (`[record+8] == -1`) | identity for clusters; for a placement (rigid program) its rows per draw, `world::model_rows` |
| `c[170..172]` | position and texcoord decompression (scale, offset) | `0x0001cdd0`; `0x0004d7c0` passes no compression for the BSP (record `+4` is `-1`), so the default upload (`0x0001cf29`, scale 1, offset 0) | `c[170]` all 1, `c[171]` 0, `c[172]` `(1, 1, 0, 0)`: BSP vertices are uncompressed |

Where `v5` and `v6` come from depends on the render record
(`0x004ba138 + 0x20·i`). `0x0002cbf0` packs its `+0xc` as bits 0..3 an
argument, 4..8 an index into the `0x24`-byte table at `0x004c152c` (`0x1f` none), 9..17 the cluster index, 18..28 a placement
index; record `+0` bit 12 says which (the structure draw's choice at
`0x0004d18b`). The cluster records (`0x00133390`) pass `-1` for the
placement, so bits 18..28 are `0x7ff`. The structure draw's layer-3 part
(`0x0004d285`..`0x0004d420`) then does:

- **A cluster** (bits 18..28 `0x7ff`, `[record+4] == -1`): no bucket. The
  section's own streams carry `1f` and `30` (structure-bsp.md: `30` is
  present exactly when the section's lighting flag bit 1 is set), and the
  cluster render info (group `+0x2c`) at the cluster index picks the
  lightmap page and palette.
- **An instanced placement** (`[record+4] == -1`, bits 18..28 the
  placement): `instance_bucket_refs[placement]` (group `+0x54`), its first
  offset times 4 bytes written into the bucket's first descriptor when
  bucket flag bit 0 is set (`0x0004d3a1`), then `0x0004dc60` binds the
  bucket's streams; page and palette from `instance_render_info[placement]`
  (group `+0x4c`). The definition's section is shared by every placement,
  so its lightmap coordinates and lighting vectors are the placement's,
  in the bucket.
- **An object** (`[record+4] != -1`, bits 18..28 not `0x7ff`):
  `cluster_bucket_refs[bits 18..28]` (group `+0x64`; despite the name, the
  engine reaches it only for object records here), the offset at entry
  bits 9..17. Not the world's concern.

A cluster reads its section's own `lightmap_texcoords`. A definition has no `1f` of its own, so a placement reads its run of the bucket (`load::instance_run`, `load::bucket_slice`); the lighting vectors (`30`) the same way (S: the bucket's `30` stream is indexed like its `1f`).

The per-view constants (`c[0..17]`) are the same for every draw of a
view, and nothing in layer 3 writes them per draw (the per-draw uploads
found are `c[18..]`, `c[21]`, `c[22]`, `c[24]`, `c[25]`, `c[31]`,
`c[50..52]`, `c[170..172]`, `c[174]`). So the native pass reads them
from the device's constant state when the pass is issued (after the
drain, so the view's own upload is in): exact by construction, and no
camera maths of ours to keep in step with the game's.

The pixel side is the implementation's final pixel shader definition
(variants built, instance colours patched), translated by `shaders::psh`.
For `lightmap_tex_detail` it combines base (stage 0) and detail (stage 1,
`x2`), the bump map (stage 2) dotted with the tangent-space lighting
vector (`v1`) and view vector (`v0`), and the lightmap (stage 3). Its
`c0` constants of the specular stages (4 and 5) are marked engine-driven
(`h2 shader` prints "engine constants", mask `0x30`), traced (Code,
`h2 disasm`, the 03a pass's bindings read off the disc):

- The per-pixel implementation has one spas `+0x38` binding through
  `0x00019ca0`: mode 3 (the alpha channel), no transform, engine scalar
  21. Its input masks (spas `+0x104..+0x114`, the inputs `0x0001ae70`
  evaluates before the draw, bit `n` input `n`) are scalar 21 and integer
  5 (the fragment selector), nothing else.
- Scalar 21 is `0x000337e0`, which returns `0x485a6c`; integers 4 and 5
  are `0x485a64/0x485a68`. `0x0001bbf0` copies all three from the `shad`
  (`+0x44`, `+0x3e`, `+0x40`) before each draw. So the "engine" constant
  is the instance's own specular scalar, the same on every draw of the
  shader, and `tags::material` already resolves it into the pixel shader
  definition (`EngineInputs::for_shader`). The stage-5 `c0.rgb` is an
  instance colour.
- `ResolvedImplementation::per_draw_inputs` says when an implementation
  reads anything else (another scalar, any vector, another integer, by
  mask, binding or fragment selector); only those stay on the fallback.
  Of the pass's other implementations the per-vertex one has the same
  masks; the dynamic one reads engine vectors (lighting), but the world
  pass uses the per-pixel one.

### Material translation

The device hands the external pass its vertex constants at issue (a
read-only snapshot). `native-render::load` resolves, per material, the resolved layer-3 pass and
implementation 0: the classification-0 program, the final pixel shader
definition, stage textures (instance bitmaps with their image, the
lightmap), sampler states and instance constants. Materials without
one (no layer 3, more than one pass, an engine texture source other
than the lightmap, per-draw engine inputs) stay on the D3D path.
Base maps upload their decoded mip chain (`gpu::Image::from_levels`).
`native_render::program` links each material's programs and fills its
uniforms and bind groups. During the BSP upload, `WorldPipelines` opens
the material's archived pipeline descriptors. The compile workers build
pipelines for those descriptors and for new combinations of target
formats, stencil and inherited cull requested by a live view. At guest
admission, `GpuWorld::prewarm_world` checks every selected cluster and
placement. D3D draws that selection until all its bindings are present
and its pipeline completions have crossed a frame boundary.

- `look::layer3_pass` takes the pass's implementation as above and
  its own render states (`pass_state`, as transparents do): depth test
  and write, colour writes, alpha test and cull. The 03a world passes
  set `COLORWRITEENABLE` (all four channels), `ALPHABLENDENABLE` 0 and
  `ZWRITEENABLE` 1; some add an alpha test (`ALPHATESTENABLE`,
  `ALPHAREF`, `ALPHAFUNC`), and those that sample the texaccum (the
  `_combined` templates) set `ZFUNC` equal without depth writes. A
  state the pass does not set is inherited from the previous draw on
  the D3D path (S, not traced): `ZFUNC` is taken as `LessEqual`, and
  an unset `CULLMODE` takes the device's at the layer-3 call
  (`InheritedCull`, above; nothing where it does not read). It refuses: engine-driven render or stage states, a
  blending pass, fog enabled, stencil, depth bias.
- `program::compile_world` translates the pass (inputs `v0..v6` from
  the world vertex: position, uv, normal, binormal, tangent, lighting
  vector, lightmap uv) and fills the instance constants by their
  written components, `c[172] = (1, 1, 0, 0)` and identity model rows
  `c[50..52]` (the vertices are already placed). It refuses:
  per-draw engine inputs, a program reading other inputs (but those
  the pass sets itself with `SET_VERTEX_DATA4F_M`, `0x00018e3b`: the
  resolved `vertex_defaults`, read from `xvs.defaults[n]`; 03a's
  transparents read `v10` and `v11`, two colours, mask `0x0c00`), a
  translation warning, a non-2D fetch, a sampled stage with nothing
  bound, signed channels on the lightmap; and (`world_parity`, the
  constants check) anything the D3D path takes from device state the
  native pass does not have:
  - a vertex constant outside the view's `c[0..18]`, the instance's,
    `c[50..52]` and `c[172]` (the structure draw uploads `c[21]`,
    `c[24]`, `c[31]`, `c[174]` and others per draw), or one addressed
    relative to `a0.x`. 03a's world programs (`lightmap_perpixel_*`,
    classification 0) read `c[0..3]`, `c[5..11]` (view), `c[18..20]`
    (instance) and `c[172]` only;
  - fog: a combiner that reads the fog register, or a definition
    without a final combiner (D3D builds one from the fog and specular
    states). The pass does not set `FOGENABLE`, so the device's fog is
    whatever the previous draw left; 03a's world passes program their
    own final combiner and read no fog (the world's fog is a later
    layer), so none is refused for it;
  - texture modes that read device state: bump-environment matrices,
    the eye of the reflection modes, BRDF;
  - border addressing on a sampled stage.
- `program::part_fit` checks each cluster against its world-space
  vertex program and each placement against its rigid program.
  Every input the program reads must be present in the section.
  A sampled lightmap needs a decoded page and palette. Other parts
  sharing the material can still draw with their own inputs.
- On a BSP without a matching first lightmap group, the record
  builder selects implementation 3. The worker resolves that
  implementation and the normalization cube in the globals tag.
  For example, the main menu's dynamic detail shader declares five
  input registers in both its world and rigid classifications.
  The worker checks those inputs. A lighting vector in `v5` belongs
  to the per-pixel lightmapped variant. Bitmap cube faces and volume
  slices retain their shape and mip chain through upload.
- Palettized stage bitmaps. `tex_bump`'s bump stage (stage 2) is
  `P8Bump`, and `P8` is palettized too: the tag has no palette, and
  `assets` decodes both as their index in every channel. The D3D path
  samples them through the device palette bound at their stage, so the
  native pass expands them through the same one: the load gets the
  palettes bound at stages 0..3 when it starts (`Device::stage_palette`,
  read-only), and refuses a palettized bitmap at stage 3 or at a stage
  with no full 256-entry palette. One decoder does this for every pass
  loaded with the world (`native_world_glue::StageDecoder`: layer 3
  and the transparents' layer-15 passes, which share its images; the
  objects can take it too): an image is expanded once, and another
  stage shares it only when it holds the same palette; a refusal for
  the stage's palette is not the image's, so another stage may still
  take it. Each ask checks that the palettes the load used are still
  bound, and otherwise leaves the world to the D3D path (said once). Research (Code, `h2 disasm`): `SetPalette`
  (`0x3f84d0`) is called with the default palette (`[0x484DBC]`) on
  every stage at boot and by `0x00015b70`, which also clears the
  per-stage cache at `0x484DC0`; `0x00015c90` binds a lightmap's
  palette and its one caller is the structure draw (`0x0004d416`), at
  stage 3; `0x00044628` puts stage 0 back to the default. So for the
  world's layer 3, stages 0..2 hold the default palette. The load names the native
  materials that sample a palettized bitmap (`sample a palettized stage
  bitmap`).
- Samplers are made as the D3D path makes them
  (`gfx::translate::SamplerKey::from_stage`: anisotropy, `MAXMIPLEVEL`
  as the smallest LOD, address 5 clamping to the edge), and
  `D3DTSS_MIPMAPLODBIAS` goes into the pixel uniforms' `lod_bias` as
  `d3d::nv2a` puts it. Lightmap pages keep their mip chains.
- Per frame the device's `c[0..18]` (the external pass's
  `view_constants`), the guest and texel sizes (surface and the
  render-scale shift) and the W-buffer's depth scale (`w_clip`,
  `depth`, `depth_range`). A program batch draws over the whole
  target: the game's viewport is in `c[10..11]`. A pass without the
  view constants draws nothing.
- Colour: the D3D path and the native pass both sample `Rgba8Unorm`
  (or compressed unorm) textures and write the game's own target, with
  no sRGB view on either side, so there is no gamma step to match.
- Left (S): the inherited `ZFUNC`; the decoded chain of a bitmap whose top level is
  not resident starts lower (a smaller image, the same coordinates).

Of 03a's scenario shaders with a layer 3, about a third sample the
texaccum (engine texture 3, the `_combined` templates) and stay on the
D3D path; most of the rest are `tex_bump` and its variants, one pass
with the per-pixel lightmap implementation, and translate.

### Native opaque layers

The main scene draws ordinary BSP clusters and placements in sorted order in
layers 1, 3, 5, 7 and 8. The default front end takes these entries at the
standard geometry handler (`0x0004d0b0`). Native objects and entries left
with the guest can still use the native world layers' indexed admission. Both paths preserve the original subpart
ranges, topology, live colour/depth attachments and per-record stencil state.

Layer 1 replaces a draw only while target 1's attachment header is
bound. Texture inputs must resolve to views the program can sample.
The two-copy reservation records the entire indexed range on every
live target copy before replacing the D3D submission. Color and
depth/stencil writes use the original attachments and feed the usual
RTT and depth tracking. Alternate views and special handlers remain
with their own D3D setup.

The layer-3 replacement uses the texture view from the resolved
D3D draw. It also pins target 1's colour attachment and resolves the
engine texture 3 header through the RTT path. All three views must
hold the same GPU texture on each live copy. The guest dimensions
must agree and the texture must use linear texel addressing. This works
with either owner of layer 1, because both write the same attachment.

The sorted BSP path prepares the vertex program for each classification
and the stage binding recipe for each shader, LOD, layer, pass and
implementation when the BSP loads. Section preparation reads the live
section's declaration, bitmap residency and palette, index windows and
target identities. Render-target samples resolve against their current
contents in submission order. The menu's unlightmapped implementation
uses its own declaration and program. A part can draw without a lighting
vector when its selected implementation has its required inputs.

### Native BSP scene walk

The walk goes off with the rest of the native renderer.

| Guest boundary | Work done in Rust | State handed to the next stage |
|---|---|---|
| `WORLD_CLUSTER_RECORDS` and `WORLD_PLACEMENT_RECORDS`, from the main camera gather | Walk the current visibility lists in their original order. Read projection depth, pixel scale, cluster bounds and placement spheres. Compute the screen-size byte, placement fade and fog flags. | Publish the same record indices, lightmap implementation, placement matrix pointer, subpart bitmap allocations and resident resource stamps. A cold resource can enqueue I/O. A mixed walk falls back before publication and retries each resident row at `WORLD_VISIBILITY_FLAGS`. The guest handles each cold row in place. The following object gather sees both record counters. |
| `WORLD_PART_MATERIAL`, first part at the main scene loop in `0x44ac0` | Examine **every visible part in one BSP record**. Compute its material visibility, subpart visibility, shader LOD and layer mask. An eligible record appends its items in guest order, bypassing the remaining material queries, `GEOMETRY_SUBMIT`, its LOD queries and item appends. | The ordinary shader, argument, callbacks, selector, LOD, mask and aggregate list mask remain available to every later layer. The per-part centre and clipping plane live in caller scratch. Only the transparent producer exports them. A visible transparent producer, shader override or unknown material keeps the complete section on the guest path. |
| `LAYER_BUILD` and `WORLD_GEOMETRY_BUILD` | Emit the standard BSP sort slots for opaque, water and fog entries. An unsupported callback runs with the original caller frame and can mutate the list. | Rejected slots retain their writes, including undefined words inherited by later candidates. The original sort still orders ties and layer-5 entries. Objects and custom handlers keep their place. |
| `LAYER_ENTRIES`, after layer setup | When the world, water or fog renderer has committed its reservation and every active item has its layer bit clear, return the empty dispatch result immediately. | The GPU reservation already covers the claimed ranges. One remaining bit keeps the dispatcher. Layer setup and end still establish the shared device state. No handler, shader, stencil switch or indexed draw would run in the empty dispatcher. |
| `RENDER_STATES` and `WORLD_PART_EMIT`, inside an opaque BSP draw | Use the render-state port through the first necessary guest call. Elide repeated simple states when the shadow matches and the push header is unique. Use the one-range emitter when visible subparts tile the part's indices. | Changed states and shared push headers still reach the device. The emitter flushes textures and emits the same indices and topology. A masked strip needs exactly one subpart. The placement constants, shader cache, lightmap binding and stencil tail still execute. |

#### Sorted handler and device handoff

The sorted hook matches the standard builder and draw callback, the scene
layer, the handler arguments and the caller's current sort slot and counter.
A native run ends at the first object, custom callback, excluded slot or
refused section. Each accepted entry reads its own implementation,
classification, record and visible subparts. Triangle lists coalesce
contiguous visible index ranges. A masked strip restarts at each subpart.
The native run returns with ESI on the last consumed slot. The caller
performs its normal final increment and can dispatch the next guest draw.

The run's `handoff` applies material and device transitions in draw order.
The portable entry holds the outgoing shader and device cache words in a
bounded writeback buffer. Native reads through any alias see the most recent
write. Before a real guest call, a fault or the return to the outer loop,
the memory guard publishes the final dirty bytes. D3D write watches see
logical writes at store time. Resource retains, releases and push callbacks
still execute in device order.
The shader cache tracks the previous and current shader and implementation,
dirty parameter banks, live texture headers and sizes, sampler stages and
the full pixel-shader definition. An implementation switch consumes the
outgoing restore list before publishing the new states. Component writes
preserve untouched vector lanes and PS fields. Frame inputs, animation and
record constants are evaluated against the current view and guest floating
point control words.

A lit placement updates both offsets in each selected lightmap stream
descriptor. Its palette, shader, declaration, stream and texture changes
reach the D3D device in their original order, including device references
and callbacks when a resource loses its last reference. A missing vertex
input can suppress emission after the binder has already changed state.
Stencil, fade and transient target operations bracket the actual draw.
The next native or guest entry therefore starts with the outgoing device
and RAM caches already installed. Applying only the last part's bindings
would lose writes to earlier descriptors and intermediate resource releases.

The per-range submission uses the prepared geometry and validated texture
and program hints. Stale or dynamic resources still use the ordered D3D
resolver. A preflight refusal keeps the original sorted handler live. A
late refusal before the first range can issue the indexed draws under the
state already applied. A device error after emission follows the fault
path so a submitted prefix cannot be drawn twice.

The A/B method for this walk is in [notes/native-ab.md](notes/native-ab.md).

The frame columns `native_world_walk_sections`, `native_world_sections_prepared`,
`native_world_builds_skipped`, `native_world_entry_passes_skipped` and
`native_world_state_calls_skipped` count published render records, sections
appended in Rust, guest build callbacks replaced in Rust, whole entry
dispatches bypassed and redundant Simple state calls respectively.

Frame stats distinguish pre-reserved BSP parts
(`native_world_opaque_prereserved_draws`) from ordinary world ranges
that reach the indexed call after guest handler setup
(`native_world_opaque_guest_indexed_draws`). The latter includes
ranges later accepted by the native GPU backend. The corresponding
`_sections` columns count each cluster or placement once per view and
BSP on each path. A section with parts on both paths counts in both.
The indexed counters require the world layer hooks. Prepared opaque
submissions and their device preflight are timed in the D3D phase.

At the sorted handler boundary, `native_world_opaque_handlers_taken` counts
every ordinary BSP geometry handler consumed by the native path. This is the
skipped guest handler count. `native_world_opaque_handler_draws` counts
all ranges those handlers submitted. A masked strip can emit several ranges.
Within these totals, `native_world_opaque_handlers_indexed` and
`native_world_opaque_handler_indexed_draws` count the handlers and ranges
that needed the indexed backend for emission. `native_world_opaque_handlers_fallback`
counts each whole geometry handler left to the guest once. The
`native_world_opaque_materials` field in `H2_FRAME_STATS_CSV` breaks down
each frame by full shader datum, including the reason and handler count
for indexed emission and for whole-handler refusal. The frame-stats exit
report sums the same breakdown from `H2_FRAME_STATS_FROM`.

`native_world_opaque_taken_sections` and `native_world_opaque_fallback_sections`
count distinct clusters and placements for consumed and refused handlers
respectively. A section with mixed materials can appear in both. Each
frame counts the section once per BSP, view and outcome across all sorted
layers. The material field also gives `refused_sections` per shader and
reason when the render record identifies a valid cluster or placement.

`native_world_opaque_gpu_draws` counts prepared packets recorded on the
required GPU copies. `native_world_opaque_backend_draws` counts prepared
packets the ordinary backend drew after a recording refusal. Deferred
packets report the recording outcome on the worker. The material
breakdown includes the backend refusal reason. Compare these counters
with skipped handlers to distinguish CPU preparation savings from changes
in GPU recording.

### Target 1's headers

Target 1 has two guest headers. The scene's main layer-1 site
(`0x0002be45`) asks `0x00014bc0` for it before the layer, and
`0x0001dcf0(1)` gives the attachment header at descriptor `+0x18`
when the descriptor is enabled and `+0x78` is positive.
`0x0001dcc0(1)` returns the texture header at `+4`. Equal guest
dimensions alone do not establish the same target. The native stage
suppression counters show committed draws. Frame and pass color checks
use the agent API (`render.frame` and `render.stats`).

### Scene and exceptional draw sites

The retail return sites distinguish uses of the same layer number.

| Site | Draw calls | State and ownership |
|---|---|---|
| Scene kinds 1 and 2 | Layer 1 `0x0002be45`, layer 3 `0x0002bed0`, layer 5 `0x0002bf34`, layer 8 `0x0002bfa5` | Layer 1 binds target 1 and usually clears. Layers 3, 5 and 8 select the scene target at `0x004858b8`. Layers 1 and 3 start with stencil mode 1. Layers 5 and 8 start with mode 0. Layer 8 also requires the byte at `0x004ba022`. |
| Scene middle | Layer 7 `0x0002c034`, layer 2 `0x0002c05b` | Layer 7 follows `0x000335e0` and precedes the dynamic lights. Layer 2 draws after the lights and after `0x0002c370(1)`. Both take their setup through `0x00044370`. |
| Alternate scene kinds | Layers 1, 3, 5, 2 at `0x0002bd57`, `0x0002bd6f`, `0x0002bd87`, `0x0002bda3` | All use the general layer begin. The main scene's target-1 and clear sequence is elsewhere. |
| First person | Layers 1, 3, 8, 7 at `0x000400a6`, `0x00040131`, `0x000401a2`, `0x0004026c` | The first-person list and projection are separate. The two scene calls to this chain straddle the first-person light step. |
| Sky | `0x0002c0bc` to `0x0003ede0` to `0x0004c690` | The sky has its own setup inside a stencil-mode-2 bracket and its renderer issues an indexed strip at `0x0004c976`, with shader layer 15 and explicit cull disable. This path sits outside the layer loop and the diagnostic labels its indexed draw `Sky`. |

Within the normal geometry handler, the layer-5 object branch at
`0x0004d22f` can select a subpart mask after resolving the object's
geometry. The emitter at `0x0004daa0` can issue multiple indexed slices
from that mask, and layer 5 sorts with `0x0003bbd0`. Preserve both the
slices and their order. The geometry handler switches stencil immediately
before the emitter in layers 1 and 3, then runs its stencil tail after
the emitter. Its prefix also handles per-record fade and geometry state.
The same layer's entries may use other handlers from the live list, so
a replacement needs the observed standard geometry call as well as the
layer ID.

## Not verified

- Coplanar ties between parts at a pixel may differ from the D3D path
  (S: which part is last is the depth test's).
- The transparents and objects draw without the stencil (S).
- A tag's or engine render-state list that tests the stencil after layer 3.
- Reprojected frames redrawing the native pass with the warped camera.

## Related

[native-renderer.md](native-renderer.md), [native-objects.md](native-objects.md),
[occlusion-culling.md](occlusion-culling.md), [lighting.md](../formats/lighting.md),
[structure-bsp.md](../formats/structure-bsp.md),
[notes/native-ab.md](notes/native-ab.md).
