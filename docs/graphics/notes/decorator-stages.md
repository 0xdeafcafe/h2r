# Decorator passes the stage leaves on D3D

Evidence for [native-decorators.md](../native-decorators.md) ("Rules and limits": the lightmap and engine-driven states). Nothing here states what the code does today. Entries are newest first.

## 2026-11-22: the engine-state bindings' byte layout, read

Commit: `a6ff6f83c`. Method: `h2 disasm` of `0x0001ab50` (to `0x0001ac58`) and of `0x0001a170`'s other-source branch `0x0001a89c`.

The sampler-state applier does not share the render-state layout the entry below assumed. A sampler binding is `[stage, target, source, slot]`: `0x0001ab94` takes the target from byte 1 (into `0x46703c`), `0x0001ab98` compares byte 2 with 1, `0x0001ab9c` takes the slot from byte 3, and `0x0001abe2` the stage from byte 0 (the state's cache at `0x404dd0 + 4·(32·stage + state)`). A render binding is `[target, source, -, slot]`. A source other than 1 reads the colour at `ctx + 0x108 + 12·slot` (`0x0001a89c`: `(slot + 0x16)·12`), packs it through `0x00131ed0` and keeps the state's alpha byte (`and 0xffffff` with the cached word). `tags::material::EngineState` decodes both.

## 2026-11-22: the lightmap page a decorator samples, and what engine-driven states need

Commit: `4072032fe`. Method: `h2 disasm` of the decorator draw `0x0004e260`, the geometry draw `0x0004d0b0` around its lightmap bind (`0x0004d28b`..`0x0004d428`), the pass setup `0x0004d9c0`, shader draw begin `0x0001bbf0`, the stage setup `0x00018c30`..`0x00018d1f` and the engine render-state applier `0x0001a170`; the tables `crates/formats/tags/src/shader.rs` (`ANIMATED_RENDER_STATES`, `ANIMATED_STAGE_STATES`, `render_state_name`, `stage_state_name`). Re-run by repeating the reads at a later commit.

### The lightmap page

**The decorator draw does not inherit a page.** Its first store (`0x0004e277`) is `mov dword ptr [0x00485a60], -1`: the current lightmap page, which the stage setup reads, is reset before the shader draw begins (`0x0004e4b8`, `0x0001bbf0`). So a decorator pass that samples the lightmap does not sample what the previous draw left.

- The geometry draw sets the same word per record (`0x0004d420`): a cluster's page and palette through `0x00015c90` for a record without a model datum, -1 for an object's record (native-objects.md, "The lightmap stage").
- With the page at -1, the stage setup binds the globals' rasterizer data element's bitmap at `+0x64` (`[[0x00485a80] + 0x64]`) and no palette (`0x00018c43`..`0x00018c4c`, then `0x00018d1f`). `0x00018cc2` caches the page in the binding's slot (`ctx + 0x1338 + 0x40·stage`) as a float.
- The decorator draw adds no lightmap coordinate stream. Its inputs are the set's stream (`0x0004e47a`: `0x0001c620`, then `SetVertexShaderInput` through `0x0001c710`) and the per-instance constants. A program reading a lightmap coordinate register gets whatever the NV2A holds for an unsupplied input.

**What a native path would do.** Use the objects' approach: the lit build's `EngineStages::lightmap` (`native_objects_glue::object_lightmap`), which loads that bitmap and takes it only when it is uniform (every texel of every level the same). A uniform image samples the same wherever the coordinates fall, so the missing stream does not matter. In `decorators_load::decorator_look` the refusal "a stage samples a lightmap" would give way to binding that image for `Stage::Lightmap`, with the same uniform check and the same reason when it fails. A non-uniform bitmap stays on D3D, and it would also need the coordinates the NV2A holds, which nothing reads.

Not verified: that no code between `0x0004e277` and `0x0004e4b8` writes `[0x00485a60]` again (the disassembly from `0x0004e260` to `0x0004ebc3` has the one store); and the uniformity of the globals' bitmap on retail maps, which the objects' load prints.

### Engine-driven render and stage states

**What they are.** An implementation's `+0x11e` (render states) and `+0x120` (stage states) are ranges into the `spas` engine bindings (`tags::material::Implementation::engine_render_states`, `engine_stage_states`). `0x0001a170` applies the render states per draw, and `0x0001ab50` the stage states. Per 4-byte binding, as `0x0001a170` reads it:

- byte 0: the target, an index into `0x467020` (render states) or `0x46703c` (stage states), whose entries are `(state, component, kind)`; kind 0 is a colour, 1 a float, 2 an integer;
- byte 1: the source. Byte 1 = 1 reads a float at `ctx + 0x5c + 4·byte3` (`0x0001a1cd`): the shader's evaluated parameter slot, which `0x00017550` fills per draw through `0x0001b230` and `0x0013b390`. Other sources branch to `0x0001a89c`, not read;
- the value is converted by kind (an integer through `fistp` at `0x0001a1f8`) and set by the state's class. States below `0x5c` go through the setter table `0x43e6f0` and are cached at `0x407498`; states up to `0x88` are deferred (dirty bits `0x407490`, table `0x43e4d0`); `0x88` and up each have their own setter (`0x0001a24d`..).

The targets:

| Table | Targets |
|---|---|
| render (`0x467020`) | `BLENDCOLOR` (two entries), `ALPHAREF`, `POLYGONOFFSETZSLOPESCALE`, `POLYGONOFFSETZOFFSET`, `LINEWIDTH`, `FOGCOLOR` |
| stage (`0x46703c`) | `MIPMAPLODBIAS`, `COLORKEYCOLOR`, `BORDERCOLOR` (two entries), `BUMPENVMAT00`, `BUMPENVMAT01`, `BUMPENVMAT10`, `BUMPENVMAT11`, `BUMPENVLSCALE`, `BUMPENVLOFFSET` |

**What a native path would need**, for decorators and the other stages that refuse these (`object_program`, `look`, `fog_gpu`, `light_look`, `screen_look`):

1. Decode each binding into (target state, component, kind, source slot). This is a table lookup over the two arrays above.
2. Evaluate the source per draw. The parameter slot is a shader animated parameter. Its value needs the inputs `0x0001b230` reads (render time `0x004858a0` and the function inputs through `[0x00485a58]`'s table). `scene_extract::layout::layer_inputs` already reads the render time for the world layers' animated properties. A pass whose parameter reads a per-object input is per-draw, as `per_draw_inputs` refuses today.
3. Map each target to what the translated pass holds:
   - `BLENDCOLOR` is a dynamic blend constant (`set_blend_constant`);
   - `ALPHAREF` is the alpha-test reference the pixel program compares against;
   - `FOGCOLOR` is the fog uniform;
   - the polygon offsets are the pipeline's depth bias, so each value is part of the pipeline key;
   - `BORDERCOLOR` is a sampler border, part of the sampler key;
   - `MIPMAPLODBIAS` has no wgpu sampler field, so it would be a bias in the translated fetch;
   - `BUMPENV*` are the texture-stage bump matrix, a pixel uniform for `texbem`-style fetches;
   - `COLORKEYCOLOR` and `LINEWIDTH` are not drawn by these passes. A binding to them can be checked to be inert.
4. Keep the state-stability rule: after the pass the device holds the values `0x0001a170` set, so a skipped guest draw must leave them as it would. Either the device already holds them (`layer_pass::skip_holds`'s check), or the stage writes them into the device's shadow, which no stage does today.

Not verified: the stage-state applier `0x0001ab50`'s byte layout (assumed to match `0x0001a170`'s, as both index the same bindings block); the other source kinds after `0x0001a89c`; how many decorator passes on retail maps have either range non-empty (the load prints each refusal reason once per map: `native-decorators: …`).
