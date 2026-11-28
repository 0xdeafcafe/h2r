# Native objects: fade, the shader flag and the item's argument

Status: Built. Part of stage 2 ([native-objects.md](native-objects.md)); the page holds the guest state an object item's draw touches beyond its material, and when the native draw may take the item.
Code: `crates/rewrites/scene-extract/src/layout/object_walk.rs` (`read_part`, `layer_part`; its module notes list the addresses), `crates/extensions/game/src/native_objects_glue.rs` (`device_stipple`, `record_stipple`, `stipple_at_rest`, `structure_at_rest`), `crates/rewrites/engine/src/render_loop/opaque_material.rs` (`faded_rows`), `crates/rewrites/native-render/src/object_program_gpu.rs`. Tests: `crates/rewrites/render-bridge/tests/native_objects.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

| Input | Where | Part flag | Declines as | Native draw takes the part when |
|---|---|---|---|---|
| Fade byte | record `+0x1a` | `PartFlags::FADE` | `object_fade`, `object_stipple` | the engine's cached fade and the device's pattern are the rest values, and the byte's pattern is the same in every x87 mode |
| Shader flag bit 0 | shader `+0x16` | `PartFlags::SHADER_FLAG` | `object_shader_flag`, `object_shader_flag_target` | before the frame's target copy, a flag bit 20 program only without a structure parameter block and ahead of every kept item; no structure lightmap index in layer 3 |
| Structure parameter block | structure `+0x214` | `PartFlags::STRUCTURE_PARAMETERS` | `object_shader_flag` | the device and the engine already hold every word of the upload |
| Item argument | item `+0xc`, `+0x14`, `+0x15` | none | none for the projected size | always (see below) |

The geometry draw `0x0004d0b0` takes three things from an object item beyond its material and its record's `+0` bits (**Code**, `h2 disasm`). Extraction reads them for layer 3's walk (`object_walk::read_part`), the world layers' items and the first person's (`object_walk::layer_part`).

## The fade byte

Record `+0x1a`. Below `0xff` the prologue `0x0004d640` scales it by `0x0045dbb4`. Where the result differs from the engine's cached fade `0x004b8494` by more than `0x0045dbdc`, it sets the polygon stipple pattern `0x0001c290` picks (one of 17 at `0x00487288`) through `SetStipple` (`0x003f8cc0`) and caches the fade. The epilogue `0x0004d720` does the same for the engine's one (`0x0045dbc0`), and so does the layer end `0x000445d0`.

The pattern masks the draw only while the device's stipple is on. The target mode call `0x00015680` turns it on (mode 1) before layers 1 and 3 outside view kinds 4 to 7, and off (mode 0) before layers 5, 8 and 20 (`0x0002be17`..`0x0002bfef`).

No device object holds the pattern: `SetStipple` is never answered, and the pattern a draw holds is the table's entry for the fade the engine caches (`native_objects_glue::device_state::stipple_pattern`, `opaque_material::fade_stipple_table`), since the prologue sets and caches the two together. The native draw takes a faded part only where the engine caches a fade of exactly one (`native_objects_glue::stipple_at_rest`, `opaque_material::rest_stipple_table`). From there every faded record's epilogue puts the cache and the pattern back, so skipping the draws leaves what drawing them would.

Under the stipple the draws hold (`native_objects_glue::device_stipple`) every object draw masks as the guest's would (`record_stipple`):

| Record | Rows |
|---|---|
| faded | the prologue's pattern from the rest state (`opaque_material::faded_rows`, run under both precisions and the four rounding modes; the rest rows where the fade is within the tolerance of one) |
| not faded | the held pattern (the cached fade's table entry) |
| either, every row set | none: the draw masks nothing |

A masked draw carries its rows (`PlannedDraw::stipple`) and draws with its program's stipple variant: the same module with the pixel shader translated with `PixelOptions::stipple` (`object_program::stipple_wgsl`), compiled at its first request and keyed apart in the program's pipelines. Layer 3's walk, the world layers' objects and the first person all take it.

## The shader's flag bit 0

Shader `+0x16`. The draw then takes no lightmap in layer 3 (`0x0004d2ab`) and no PRT stream in layer 5 (`0x0004d237`). It asks for residency of the stage bitmaps of the shader's templates (`0x0004d486`..`0x0004d55d`: `0x0012ccf0`, then `0x0012ce00` for one not resident), which sets no device state.

With its program's flag bit 20 (the pass implementation's `+0x118`, `0x0004d561`..`0x0004d572`) it calls `0x0003bea0`. The first time a frame (`0x00509430` clear; `0x0002c0f5` clears it) that call clears the shader cache (`0x00016b10`, which first writes the cached program's restore states). When the active structure has a parameter block it also copies and binds the structure's target (`0x000352e0`, `0x00014bc0`).

- Layer 3's walk reads the entry's program and refuses bit 20 itself.
- A world layer's or the first person's entry is built under the layer draw. Extraction gives the part `PartFlags::TARGET_UNCOPIED` while the frame's copy is not made.
- The host knows the programs of the passes it plans (`ResolvedImplementation::copies_target`, the implementation's bit 20) (`object_program_gpu::makes_target_copy`). An item whose programs lack it makes no call, so skipping its draw skips nothing else. A world layer's item with the bit draws when it comes ahead of every kept item of its layer (it is never order-free, `native_layers_glue::object_skip`): its call then finds the cache as the body's `0x00016b10` cleared it and leaves the frame's copy flag `0x00509430`, which the layer's commit sets (`native_layers_glue::TARGET_COPIED`). With a structure parameter block it also leaves the copy `0x000352e0(0x16, 1)`: the pass downsamples the bound colour into the refraction target's own colour, a written slot of its plan, before the item's first draw (`native_layers_glue::STRUCTURE_COPY_TARGET`, `LitObjects::structure_copy`, `snapshot_gpu::Snapshots::into_target`). The copy's own device writes (four stages' states at `0x00404de8`, the combiner words at `0x00484f68`) are not made, so a later guest draw finds them as the draws before the item left them. The first person keeps such items.
- A kept item counts as one of unknown effect for the order of those after it (`layer_pass::clear_layer_items`).
- A flagged layer-3 record with a structure lightmap index is refused, since the native draw would bind it and the guest's leaves the stage as the device holds it.
- Extraction reads no PRT stream for a flagged layer-5 item.

## The structure's parameter block

When the active structure (`[0x004e0348]`) has one (`+0x214` above 0), a flagged shader's prologue runs `0x0003c650` and `0x0003d000` on its first element (`[+0x218]`).

| Call | Does |
|---|---|
| `0x0003c650` | Rotates the up vector `[0x004687b0]` by the angle of the view words `0x004b9dac`, `0x004b9db0` (`fpatan`) plus the block's `+0x8c`. Stores thirteen words at `0x004c1b9c`. Sets the bump matrices of stages 1 to 3 (`0x003f5e30`) from four of them times `+0x68`, `-(+0x6c)` and `+0x70` |
| `0x0003d000` | Sets `c[21..25]` (`0x003f71d0`) and the fog colour (`0x003f5370`, `+0x58` packed by `0x00131ed0`) |

`engine::render_loop::structure_parameters` computes all of it in the original's order, x87 and SSE steps included. Extraction gives such a part `PartFlags::STRUCTURE_PARAMETERS` and its view the upload (`View::structure_parameters`; the item refuses `object_shader_flag` when it does not compute).

An object pass compiles a program reading rows of `c[21..25]` its pass does not set and records them (`ObjectPass::structure_reads`). The draw puts the upload's rows there and its bump matrices into stages 1 to 3 (`object_program_gpu::with_structure_parameters`), and an item without the upload refuses such a program. The draw takes the fog colour from the device at the call.

The host takes the part only where the device and the engine already hold every word of the upload (`native_objects_glue::structure_at_rest`), so skipping the draws that would run it leaves what running it would.

## The item's argument

The argument (`+0xc`) and the `+0x14` and `+0x15` bytes. The sorted dispatch passes them to the draw (`0x0003bc7b`..`0x0003bc8d`).

- **High byte, projected size.** `0x0004d9c0` scales it by the part's `+0x2c`, `0x004670e4` and `0x0045dc10`, and the shader begin `0x00016b90` turns it into the distance words at the shader context's `+0x28` and `+0x2c` (`0x00016c6e`..`0x00016cb0`). Only the texture-stage resolve reads them (`0x0001b0a0`), for the level of detail it asks residency for. The byte changes no state, constant or binding of the draw, and the native draw ignores it.
- **`+0x15` and `+0x14`.** The shader level and the pass within the layer that `0x00016b90` selects (`[template+0x5c]+4` indexed by the level, the layer's first pass there plus the pass). The guest's list holds one item per pass, and each draws that pass alone. The scene item carries both (`frame_scene::LayerItem::level`, `pass`). The native draw plans the item's pass only (`object_program_gpu::plan_layer_part`), from the model's part of the item's level. The world layers' object programs compile at every level the material has passes in (`object_program::object_layer_levels`).
- **Low byte, the part.** `0x0004d1d8` indexes the section's parts by it (`0x48` bytes each), so the scene item's key takes its part from it (`scene_extract::extract`, `layer_item`), not from the record key's bits 0..3.

## What still declines

| Name | Cause |
|---|---|
| `object_fade` | the stipple's switch does not read, a display override stands in for its pattern, or the fade's pattern differs between x87 modes |
| `object_stipple` | the cached fade or the pattern is not what the epilogue leaves |
| `object_shader_flag` | the structure's parameter block's upload does not compute, or the device and the engine do not hold it, or in layer 3 a structure lightmap index |
| `object_shader_flag_target` | the target copy is not made yet, a pass's program has flag bit 20 and the structure has a parameter block (or, in the first person, any such item) |

In layer 3's walk these are `walk_unsupported` (extraction) or `State` (the stipple). In the first person's layer 3 they are its log reasons.
