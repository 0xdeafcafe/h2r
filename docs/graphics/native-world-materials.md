# Native world: material translation

Status: Built. Part of [native-world.md](native-world.md); the section "Material translation" there points here.
Code: `crates/extensions/game/src/native_world_glue.rs` (`material_programs`), `crates/rewrites/native-render/src/{program,look,pass_state,stage_images}.rs`.

How a world material's layer-3 pass becomes a native program, and what it refuses. A refused material keeps its D3D draws, with the reason in the load's log.

| Step | Where | Result |
|---|---|---|
| Resolve | `native_world_glue::material_programs` | The record-selected implementation of the single layer-3 pass: vertex and pixel shaders, stage textures, sampler states, instance constants |
| Upload | `gpu::Image::from_levels` | Base maps upload their decoded mip chain |
| Link | `native_render::program` | Each material's programs, uniforms and bind groups |
| Pipelines | `WorldPipelines`, `GpuWorld::prewarm_world` | See below |

During the BSP upload, `WorldPipelines` opens the material's archived pipeline descriptors. The compile workers build pipelines for those descriptors and for new combinations of target formats, stencil and inherited cull requested by a live view. At guest admission, `GpuWorld::prewarm_world` checks every selected cluster and placement. D3D draws that selection until all its bindings are present and its pipeline completions have crossed a frame boundary.

## Render states

`look::selected_bsp_pass_sampled` takes the record's implementation and its own render states (`pass_state`, as transparents do): depth test and write, colour writes, alpha test and cull. The 03a world passes set `COLORWRITEENABLE` (all four channels), `ALPHABLENDENABLE` 0 and `ZWRITEENABLE` 1. Some add an alpha test (`ALPHATESTENABLE`, `ALPHAREF`, `ALPHAFUNC`), and those that sample the texaccum (the `_combined` templates) set `ZFUNC` equal without depth writes.

A state the pass does not set is inherited from the previous draw on the D3D path (S, not traced). `ZFUNC` is taken as `LessEqual`, and an unset `CULLMODE` takes the device's at the layer-3 call (`InheritedCull`; nothing where it does not read). Engine-driven render and stage states go over the pass's own (`engine_states::apply_static`, from the material's inputs; a source other than the instance's scalar 21 refuses).

The pass refuses a blending pass. A stencil of its own panics (`PassState::device_stencil_only`): the structure draw applies an implementation's states only when the implementation changes, and an item's switch (mode 0 or 2, then mode 1) overrides them, so the pass's own holds only up to its run's first switching item, and no disc pass sets one. its own `FOGENABLE` changes nothing it draws, as the NV2A applies fog only where a combiner reads the fog register (refused, below). A polygon offset (`pass_state::DepthOffset`) is drawn in the fragment's W depth (`PixelUniforms::depth` y and z, kept by `program::frame_pixel`), and its draws keep their order: the pyramid never culls them (`ProgramSet::offsets_depth`).

## What `compile_world` refuses

`program::compile_world` translates the pass. Its inputs `v0..v6` come from the world vertex: position, uv, normal, binormal, tangent, lighting vector, lightmap uv. It fills the instance constants by their written components, `c[172] = (1, 1, 0, 0)` and identity model rows `c[50..52]` (the vertices are already placed).

| Refused | Detail |
|---|---|
| Per-draw engine inputs | |
| A program reading other inputs | Except those the pass sets itself with `SET_VERTEX_DATA4F_M` (`0x00018e3b`): the resolved `vertex_defaults`, read from `xvs.defaults[n]`. 03a's transparents read `v10` and `v11`, two colours, mask `0x0c00`. |
| A translation warning, a non-2D fetch, a sampled stage with nothing bound, signed channels on the lightmap | |
| Anything the D3D path takes from device state the native pass lacks | `world_parity`, the constants check, below |

`world_parity` refuses:

| Case | Detail |
|---|---|
| A vertex constant outside the view's `c[0..18]`, the instance's, `c[50..52]` and `c[172]` | The structure draw uploads `c[21]`, `c[24]`, `c[31]`, `c[174]` and others per draw. A constant addressed relative to `a0.x` refuses too. 03a's world programs (`lightmap_perpixel_*`, classification 0) read `c[0..3]`, `c[5..11]` (view), `c[18..20]` (instance) and `c[172]` only. |
| Fog | A combiner that reads the fog register, or a definition without a final combiner (D3D builds one from the fog and specular states). The pass does not set `FOGENABLE`, so the device's fog is whatever the previous draw left. 03a's world passes program their own final combiner and read no fog (the world's fog is a later layer), so none is refused for it. |
| Texture modes that read device state | Bump-environment matrices, the eye of the reflection modes, BRDF |
| Border addressing on a sampled stage | |

**Unbound constants.** A read of an unbound constant counts only when its value reaches an output the pass uses (`program::live_unset_read`). The shared world programs compute `oT2` from `c[20]` for every material, and a material with no stage-2 transform leaves `c[20]` unset when its pixel shader ignores `oT2`. That covers a disabled stage and a `PASSTHRU` stage whose value feeds neither a combiner nor a dependent texture operation. The check preserves implicit texture inputs and cross-stage reflection coordinates. A missing value that reaches position, an address or a consumed coordinate still refuses.

## Fit per part

`program::part_fit` checks each cluster against its world-space vertex program and each placement against its rigid program.

- The section must supply the streams its classification requires.
- A sampled lightmap needs a decoded page and palette.
- A placement using baked colour needs its declared colour stream.
- Other parts sharing the material can still draw with their own inputs.

## Implementation 3 (no lightmap group)

On a BSP without a matching first lightmap group, the record builder selects implementation 3. The worker resolves that implementation and the normalisation cube in the globals tag.

The main menu's dynamic detail shader, for example, declares five input registers in both its world and rigid classifications, and the worker checks those inputs. A lighting vector in `v5` belongs to the per-pixel lightmapped variant. Bitmap cube faces and volume slices keep their shape and mip chain through upload.

## Palettised stage bitmaps

`tex_bump`'s bump stage (stage 2) is `P8Bump`, and `P8` is palettised too. The tag has no palette, and `assets` decodes both as their index in every channel. The D3D path samples them through the device palette bound at their stage, so the native pass expands them through the same one.

The load takes the palettes bound at stages 0..3 when it starts (`Device::stage_palette`, read-only). It refuses a palettised bitmap at stage 3 or at a stage with no full 256-entry palette.

One decoder serves every pass loaded with the world (`native_render::stage_images::StageDecoder`: layer 3 and the transparents' layer-15 passes, which share its images; the objects can take it too).

- An image is expanded once, and another stage shares it only when it holds the same palette.
- A refusal for the stage's palette is not the image's, so another stage may still take it.
- Each ask checks that the palettes the load used are still bound. Otherwise it leaves the world to the D3D path (said once).
- The load names the native materials that sample a palettised bitmap (`sample a palettized stage bitmap`).

Research (Code, `h2 disasm`): `SetPalette` (`0x3f84d0`) is called with the default palette (`[0x484DBC]`) on every stage at boot and by `0x00015b70`, which also clears the per-stage cache at `0x484DC0`. `0x00015c90` binds a lightmap's palette, and its one caller is the structure draw (`0x0004d416`), at stage 3. `0x00044628` puts stage 0 back to the default. For the world's layer 3, stages 0..2 hold the default palette.

## Samplers, per-frame inputs, colour

- Samplers are made as the D3D path makes them (`gfx::translate::SamplerKey::from_stage`: anisotropy, `MAXMIPLEVEL` as the smallest LOD, address 5 clamping to the edge). `D3DTSS_MIPMAPLODBIAS` goes into the pixel uniforms' `lod_bias` as `d3d::nv2a` puts it. Lightmap pages keep their mip chains.
- Per frame the pass takes the device's `c[0..18]` (the external pass's `view_constants`), the guest and texel sizes (surface and the render-scale shift) and the W-buffer's depth scale (`w_clip`, `depth`, `depth_range`). A program batch draws over the whole target, and the game's viewport is in `c[10..11]`. A pass without the view constants draws nothing.
- Both paths sample `Rgba8Unorm` (or compressed unorm) textures and write the game's own target with no sRGB view, so there is no gamma step to match.

## Texaccum samples

Combined shaders sample the layer-1 accumulation texture in layer 3 (engine texture 3, target 1). The batched pass pins the attachment and resolves the texture header on each target copy. It checks texture identity, dimensions and linear addressing before clearing the guest draw masks.

The shader uses the guest dimensions for coordinate normalisation and for the components its size bindings write. The sorted handler resolves the same target through `native_world_materials::Source::Texaccum`.

## Not verified

- The inherited `ZFUNC` (S).
- A bitmap whose top level is not resident decodes a chain that starts lower (a smaller image, the same coordinates).

## Related

[native-world.md](native-world.md), [native-world-walk.md](native-world-walk.md), [lighting.md](../formats/lighting.md).
