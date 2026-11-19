# Native shadows

`impl.render.native_shadows` defaults off. With
`H2_NATIVE_SHADOWS_COMPARE=1` the headless GPU run captures the
layer-10 inputs, produces a private map alongside the guest map, and reports
the number of differing texels. The first producer takes validated,
GPU-resolved draw packets. This checks attachment handling and sampling
inputs. It still pays the guest gather, build, shader binding and submission.

## Caster and receiver lists

Read in the retail executable through `h2 disasm`. The descriptor lookup at
`0x00044383` chooses list 2 for layer 10, at `0x004c1a78`. Its OR word at
`+8` is also the word tested as scene flag bit 10. Layer 11 uses list 1
at `0x004c1a60`. Each list has its own active interval
`[first, count)`. The diagnostic logs the candidates in that interval,
the full shader datum, variant, part argument, geometry owner and chooser
result. The build can reject a candidate and sorts the successful entries.
The entry loop drops a sorted entry whose section is unavailable.
Packet counts refer to the GPU batches after those decisions and scissor splits.

For geometry built by `0x0004cbf0`, record bit 10 excludes a caster.
`0x00030cd0` chooses the colour-mask implementation for models on the small
target, the depth implementation for other models, and a separate
implementation for BSP geometry. The template must contain the layer. The
pixel program can alpha-kill a caster, including one whose colour writes
are disabled. A solid silhouette loses such holes.

The game uploads the projection computed by `0x00048ee0` to hardware
constants 40 through 48. The diagnostic reads all 36 words of the current
setup. The program, streams, material constants, clip, winding, depth bias,
alpha tests, stencil and scissor on each recorded draw are those the backend
resolved at issue. In particular, layer 10's setup at `0x000495e0` writes
solid polygon offset enable and the offset and slope from image scalars. It
also sets depth clip control from pass bit 4. Layer end calls `0x000496a0`
to restore state. The main view's W range alone cannot describe a caster.

## Light-graph interface

`native_render::shadow_gpu::Producer::new` starts one generation using the
caster attachments' actual formats and extents. Feed `record` every ordered
batch on the same encoder while its uniforms and vertex ranges are valid.
A refusal discards the whole private generation. Call `finish` before
another light can reuse the source. Its `SampledMap` owns the texture view
and size for this physical copy of the view. Give the light graph this
value as a per-light dependency. A guest header alone cannot identify a
generation. A counted caster has query side effects and is refused.

- Target 9 yields raw D24 bytes in RGB and stencil in alpha via
  `gfx::depth_bits::DepthBits`. Bind the result for engine texture 13.
  `SampledMap::shadow_stage` supplies the D24 GREATER comparison and the
  receiver's filtering/address modes to the existing NV2A pixel translator.
  The comparison is stored depth greater than the projected reference.
  Retail one- and multi-tap materials and the E3 view's hard-shadow option
  use the same translation as D3D.
- With pass bit 4 set and bit 5 clear, target 10 yields the ordinary colour
  mask for engine texture 8. Its clear has white RGB and zero alpha.
  The producer crops any host padding before exposing this view.

There is no implicit current shadow map. Keep the main light, any
`0x00030ed0` subpass, each object-shadow call and each physical render copy
separate. Keep the full D3D chain for an unresolved consumer. The comparison
path leaves every guest draw in place. Zero differing texels validates a
packet replay. It does not establish independent native geometry admission
or the equivalence of folding layer 11 into layer 12.

## Comparison

Filter stderr for `native-shadows`. The sequence number and present number
identify each source call. Each physical map reports its extent, recorded
packet count, differing texel count and first mismatch. A missing map or
a zero-packet replay supplies no evidence of caster coverage. Use
`H2_BOOT_GFX_SHADOWS` to capture actual shadowed frames.
