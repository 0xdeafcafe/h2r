# Native decorators: the BSP's grass and rocks in layer 3

Status: Built, not verified in a run. The read, the build's implementation lookup, extraction, the plan, the loader, the recording and the glue that reserves the pass and skips the guest's items are built and unit-tested; no frame has been compared with the D3D path.
Code: `crates/rewrites/scene-extract/src/layout/decorators.rs` (`read`, `registers`, `implementation`, `set_geometry`), `crates/rewrites/scene-extract/src/decorators.rs` (`extract`, `clear_slots`), `crates/rewrites/frame-scene/src/decorator.rs`, `crates/rewrites/native-render/src/decorators.rs` (`plan`), `crates/rewrites/native-render/src/decorators_load.rs` (`decorator_look`, `compile`, `buffers`), `crates/rewrites/native-render/src/decorators_gpu.rs` (`MaterialGpu`, `bind`, `encode`), `crates/rewrites/render-bridge/src/native_decorators.rs` (the observer), `crates/extensions/game/src/native_decorators_glue.rs` (the host). Tests: the unit tests of those files.
Switch: `impl.render.native` (the stage; extraction runs with the frame step, `frame-scene`).

Model decorators are the structure BSP's placed scatter: each placement is
one item of the layer list, and its draw sets five vertex registers and
issues one indexed draw per instance. They have no render record and are
not objects, so neither the world nor the object stage takes them, and
layer 3 keeps their D3D draws ([framescene.md](framescene.md), slice 4).
Here the frame step types each placement's draw into the scene with the
registers the guest would set, and the decorator stage draws each run of
instances of one model as one instanced draw. The rule: an instance's
`v11..v15` in the scene are the values `0x0004e260` would write, from the
same reads, in the same order.

## How it works

1. **The list** (`0x00041640`, Code). Per visible cluster (count
   `[0x00509438]`, records of `0x10` bytes at `0x004c5700`, the origin at
   `+4`), each placement of the cluster (BSP decorator block
   `[[0x004e0348] + 0x238]`: clusters at `+0x24`, `0x18` bytes, a run of
   placements at `+0x14` and `+0x12`) that passes the culling is added to
   list 0 by `0x00040e30`. The build and draw come from the decorator
   class's type (class `+4`): type 0, models, build `0x0004de20` and draw
   `0x0004e260`; types 1 and 2 (decals) `0x0004dfa0` and `0x0004ebd0`;
   types 3 to 5 (quads) `0x0004e0d0` and `0x0004ede0` or `0x0004efa0`.
   The item's `+8` is the permutation's shader, its layer mask the
   shader's (`0x0001bf50`), its argument the placement index `<< 8`, the
   cluster's slot in the low byte, the permutation's detail level (`+9`)
   in bits 30 and 31.
2. **The build** `0x0004de20` takes nothing in layer 1. It asks the
   geometry cache (`0x0012de70`, flags 3) for the placement's geometry
   record and the set's geometry (set `+0x38`) and declines when either is
   not given, then fills the entry with the implementation the shader's
   template gives for the layer (vertex type 1, `0x0004cbb0`;
   `layout::decorators::implementation`): the tag table entry of the
   shader's index (`movsx` of the item's `+8`; a `PRTM` or `prt3` shader
   reaches its template through a virtual call and is not read), its
   template (shader `+0x24`), the template's postprocess (`+0x5c`), the
   first layer of the detail level (`+4`, 10-byte records, low 9 bits),
   the first pass of the layer (`+0xc`, 2-byte), the pass (`+0x14`,
   10-byte records, the `spas` datum at `+4`) and the pass's
   implementation 0 (`spas` `+0x20`, then `+4`). It declines when the
   implementation's `vrtx` (`+0x100`) is none, has no classification 1,
   or classification 1's program size (`+0xc` of the `0x1c`-byte record)
   is below 16; the guest then draws nothing for the item.
3. **The draw** `0x0004e260`, once per placement:
   `TwoSidedLighting` on; `c[53]` = (`[0x0052598c + 4·lod]`,
   `[0x00525998 + 4·lod]`, 0, 1); the set's vertex stream (set `+0x34`,
   `0x0001c590`, `0x0001c620`, `0x0001c710`); the shader begin
   `0x0001bbf0`. For each instance `n` below the placement's count
   (`+0xa`) whose index `first + n` (`+8`) is inside the geometry record's
   instance block (`[record + 0x28]`: count, base; `0x18` bytes each) and
   whose permutation names a model below the set's count with indices:
   `SET_VERTEX_DATA4F` of `v11..v15` (push buffer methods `0x1ab0` to
   `0x1af0`), the stage textures (`0x0001cf50`) and
   `DrawIndexedVertices(TRIANGLELIST, count, [set + 0x2c] + 2·first)` of
   the model's range (set `+0x1c`, `8` bytes: `+4` first, `+6` count).
   Then `TwoSidedLighting` off.
4. **The registers** (`layout::decorators::registers`). From the
   instance (`+0` packed: class bits 0..2, permutation 3..8, turn 9..12,
   tint step 13..15, scale step 16..19, up axis 20..31; `+4` position,
   10, 10 and 12 bits; `+8`, `+0xc` colours; `+0x10` normal, 11, 11 and
   10 bits; `+0x14` fade input), its permutation (set `+0x14` class,
   `0x14` bytes, permutations at `+0x10`, `0x28` bytes) and the set:
   - `v11` = (the normal, `0x0004dd70`; `clamp((fade − set[+8]) ·
     set[+0xc], 0, 1)`);
   - `v12..v14` = the rows of the basis times the scale, with the world
     position (`0x0003f220` plus the cluster origin less `[0x0045dc10]`)
     in `w`. The scale is the permutation's (`+0xc`, `+0x10`) at the scale
     step. The basis: with permutation flag bit 0, the up axis
     (`0x0004dcd0`), a perpendicular (`0x0011d000`) and their cross
     product, the up axis made unit (`0x0004efb0`), turned about it by the
     turn; else the yaw of the turn (`0x00141ce0`);
   - `v15` = (A·B, 1): the colours `+0xc` and `+8` (`0x00131bb0`), B
     tinted between the permutation's tints (`+0x14`, `+0x18`) at the tint
     step and each pulled to its floor by the permutation's weights
     (`+0x1c`, `+0x20`) (`0x0004ca40`).

   SSE steps are computed in `f32`, x87 steps in `f64` rounded where the
   guest stores; the constants are read from guest memory
   (`layout::decorators::Constants`).
5. **Extraction** (`scene_extract::decorators::extract`, called by
   `render_bridge::scene_step` at the scene's layer 3 once the view is
   open). Each item of the current list in the layer whose build is
   `0x0004de20` becomes a `DecoratorDraw` (slot, set datum, shader, the
   item's bytes `+0x15` and `+0x14`, argument, `c[53]`) with its
   `DecoratorInstance`s (index, model, index range, `v11..v15`) in the
   guest's order. A placement whose memory does not read, or whose
   geometry is not resident (block info `+0x1e` clear), is the view's
   `decorator_remainder`.
6. **The plan** (`native_render::decorators::plan`). The draws whose
   material (set, shader, the two item bytes, layer) the stage has; each
   run of consecutive instances with the same material, `c[53]` and index
   range is one batch. Joining only neighbours keeps the guest's order.
   `Plan::slots` are the items the guest would skip.
7. **The loader** (`native_render::decorators_load`, called from boot's
   glue). At the world's load (`native_world_glue`'s worker): the shaders
   the map's decorator sets name (`DECR` root `+0`, 8-byte references,
   the datum at `+4`), each resolved (`tags::material`), and each of its
   layer-3 passes at each detail level as the build picks it
   (`decorator_look`: implementation 0, its `vrtx`'s classification-1
   program and input semantics), with its stage bitmaps decoded. At a
   material's first sight in a view (`Host::prepare`, once per load): the
   set's geometry from guest memory (`set_geometry`: the vertex buffer
   descriptor at set `+0x34`, type byte `+0`, stride `+1`, offset `+8`,
   the D3D buffer at `+0x10` whose data is at `+4`; the indices, count
   `+0x28` and u16 pointer `+0x2c`; the vertex count the largest index
   plus one), the stream type's format (`object_streams::StreamFormats`,
   the table at `0x0043f408`). The guest takes two bulk reads for the
   index and vertex data. A single utility worker translates the pass
   for the stream with `v11..v15` supplied and two-sided lighting on
   (`compile`), gathers the vertices (`buffers`) and uploads the module,
   buffers and textures (`MaterialGpu::new`). Each load can have one
   material outstanding. Its first decorator view after a swap collects
   the result. Other views of that frame use the same ready materials.
8. **The recording** (`native_render::decorators_gpu::encode`). The
   plan's instances staged once per copy (80 bytes each), bound as the
   last vertex buffer with instance step (`instance_layout`, locations 11
   to 15), and one `draw_indexed` per batch over the set's u16 indices.
   The program is the material's pass translated with
   `decorators::PROVIDED_INPUTS` (`v11..v15`) supplied; the batch's
   bindings (`Bound`) carry its uniforms with `c[53]`. The pass asks the
   pipeline compiler for each copy's target formats during reservation.
   A pending compile keeps the slots on D3D through the next swap.
   The load retains the uploaded material and the compiled pipelines. The viewport is the whole target, the view's
   rectangle the scissor.
9. **The glue** (`render_bridge::native_decorators`, an observer before
   the layer draw, after the frame step's and the native world's, and
   `native_decorators_glue`). At the scene's layer 3, the open view's
   materials are made ready, the view is planned over the ready ones,
   and the host reserves one external pass (stage `World`) as the fog's
   does: the device must be W-buffered with the stencil off, layer 3's
   bit is cleared on every planned item through a preview
   (`clear_slots`, all or nothing: each slot an item of build
   `0x0004de20` with the bit set), and the masks are applied and the
   reservation committed only once the pass has recorded on every copy.
   The next layer draw sets each bit back on the items that still hold
   the mask it left; a map activation forgets them.

## Interfaces

- Read at the layer draw `0x00044550` (the frame step's observer, scene
  site, layer 3): the current list `[0x00467138]`, its items, the BSP
  decorator block, the scenario's decorator palette (`[0x004e0350] +
  0x37c`, `8` bytes, the set datum at `+4`), the tag table `[0x004e3b44]`,
  the visible clusters, the set and permutation records, the shader,
  template, pass and `vrtx` records of the lookup, the set's vertex
  buffer and indices, the stream format table `0x0043f408`, the constants
  (`0x0045dbb4`, `0x0045dbbc`, `0x0045dbc0`, `0x0045dbd8`, `0x0045dc10`,
  `0x0045dc18`, `0x0045dd34`, `0x0045dd38`, `0x0045dd3c`, `0x0045dd44`,
  `0x005259b4`, `0x0043fde4`) and the detail tables.
- Written: layer 3's bit (`1 << 3`) of the drawn items' layer mask (item
  `+0x10`), cleared before the layer draw and set back at the next.
- `render.scene` reports per view `decorators`, `decorator_instances` and
  `decorator_remainder`, and the counters `scene_decorator_draws`,
  `scene_decorator_instances`, `scene_decorator_remainder`, and the
  stage's `scene_decorator_views` (views with decorators),
  `scene_decorator_views_native` (views the stage drew),
  `scene_decorator_items_skipped` (items the guest skipped),
  `scene_decorator_draws_unready` (draws left to the guest: their
  material is not ready) and `scene_decorator_views_declined` (views the
  host left to the D3D path) ([agent-api.md](../runtime/agent-api.md)).
- The load prints one line (`native-decorators: … layer-3 passes load;
  on the D3D path: …`), and each reason a material stays on the D3D path
  is printed once.
- What a replacement of the draw must still do: the geometry cache's
  request and stamp (`0x0012de70`) for the placement's record and the set,
  a residency write the entry census does not count.

## Rules and limits

- The stage draws all of a view's planned decorators or none: a refusal
  at the ask or on any copy leaves every item to the D3D path. Draws
  whose material is not ready stay the guest's while the rest draw
  natively.
- A pass stays on the D3D path when it has engine-driven render or
  stage states, leaves the cull to the device, samples a lightmap, has a
  palettized stage bitmap, computes anything per draw, reads a constant
  the stage does not set (it sets `c[0..18]`, `c[53]` and the pass's
  own), or its set's stream supplies one of `v11..v15`.
- The pipelines test no stencil, so a layer 3 drawn with the stencil on
  is the D3D path's.
- The pipeline draws with two-sided lighting on, as the draw sets it.
- Extraction repeats the build's implementation lookup: an item whose
  build declines is not in the scene, as the guest draws nothing for it.
- A skipped item gets no entry, so its build does not run and the
  geometry cache's request and stamp for its record and set are not made
  that frame. A placement whose geometry is not resident is a remainder,
  so the guest's build asks for it again; whether resident geometry is
  evicted sooner without the stamp is not known.
- Decals and quads (the other decorator types) are not typed: their
  items are in the transparent layers.

## Evidence

- The 15,580 "other builds" the object walk met in layer 3 were all
  `0x0004de20` (native-objects.md, "Other builds in layer 3").
- The read above is Code (`h2 disasm` of `0x00041640`, `0x00040e30`,
  `0x0004de20`, `0x0004e260` and its helpers). The register arithmetic is
  checked by synthetic tests against hand-computed values, not against
  the lifted original.

## Not verified

- That the `f64` steps give the x87 results bit for bit under the game's
  precision control.
- Whether `0x0012de70` ever gives a block that is not marked resident
  without loading it in the same call (a remainder here either way).
- A frame comparison of the stage against the D3D path, and the
  counters in a run.
- That the shader word at item `+8` and the tags' shader index agree
  (the load keys passes by the tag index, as the build reads the word).
- The geometry cache's behaviour for skipped items (above).

## Related

[framescene.md](framescene.md), [native-objects.md](native-objects.md),
[native-world.md](native-world.md), [layer-draw.md](layer-draw.md).
