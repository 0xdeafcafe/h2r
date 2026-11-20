# Native decorators: the BSP's grass and rocks in layer 3

Status: Partly built. The read, the scene types, extraction and the stage's plan and recording are built; the loader of the sets' geometry and programs and the glue that queues the stage and skips the guest's items are not, so decorators still draw on the D3D path.
Code: `crates/rewrites/scene-extract/src/layout/decorators.rs` (`read`, `registers`), `crates/rewrites/scene-extract/src/decorators.rs` (`extract`), `crates/rewrites/frame-scene/src/decorator.rs`, `crates/rewrites/native-render/src/decorators.rs` (`plan`), `crates/rewrites/native-render/src/decorators_gpu.rs` (`encode`). Tests: the unit tests of those files.
Switch: none (extraction runs with the frame step, `frame-scene`).

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
   template gives for the layer (vertex type 1, `0x0004cbb0`).
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
7. **The recording** (`native_render::decorators_gpu::encode`). The
   plan's instances staged once per copy (80 bytes each), bound as the
   last vertex buffer with instance step (`instance_layout`, locations 11
   to 15), and one `draw_indexed` per batch over the set's u16 indices.
   The program is the material's pass translated with
   `decorators::PROVIDED_INPUTS` (`v11..v15`) supplied; the batch's
   bindings (`Bound`) carry its uniforms with `c[53]`.

## Interfaces

- Read at the layer draw `0x00044550` (the frame step's observer, scene
  site, layer 3): the current list `[0x00467138]`, its items, the BSP
  decorator block, the scenario's decorator palette (`[0x004e0350] +
  0x37c`, `8` bytes, the set datum at `+4`), the tag table `[0x004e3b44]`,
  the visible clusters, the set and permutation records, the constants
  (`0x0045dbb4`, `0x0045dbbc`, `0x0045dbc0`, `0x0045dbd8`, `0x0045dc10`,
  `0x0045dc18`, `0x0045dd34`, `0x0045dd38`, `0x0045dd3c`, `0x0045dd44`,
  `0x005259b4`, `0x0043fde4`) and the detail tables. Nothing is written.
- `render.scene` reports per view `decorators`, `decorator_instances` and
  `decorator_remainder`, and the counters `scene_decorator_draws`,
  `scene_decorator_instances` and `scene_decorator_remainder`
  ([agent-api.md](../runtime/agent-api.md)).
- What a replacement of the draw must still do: the geometry cache's
  request and stamp (`0x0012de70`) for the placement's record and the set,
  a residency write the entry census does not count.

## Rules and limits

- Not built: the loader that gives a set's geometry (vertex stream and
  index buffer) and a material's decorator program with its pipeline
  (`Bound`), and the glue that queues the stage on the render thread and
  clears the drawn items' layer bit as the world's skip does. Until then
  the plan is not placed and the guest draws every decorator.
- The pipeline must draw with two-sided lighting on, as the draw sets it.
- Extraction does not repeat the build's implementation lookup: an item
  whose build declines for its shader is in the scene though the guest
  draws nothing for it. The stage's material check is where such an item
  is refused.
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
- A frame comparison of the stage against the D3D path.

## Related

[framescene.md](framescene.md), [native-objects.md](native-objects.md),
[native-world.md](native-world.md), [layer-draw.md](layer-draw.md).
