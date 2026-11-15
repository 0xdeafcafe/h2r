# Graphics: D3D8 on wgpu

`crates/gfx` is the graphics foundation. It replaces Direct3D 8 (Xbox flavour)
at the API level; it does not emulate the NV2A GPU. The HLE layer (owned by
other crates) will call into it from the recompiled D3D entry points and from
the inlined state/submission regions described in `docs/LIFTER.md` section 7.

Nothing here needs the disc. `cargo test -p gfx` covers everything except a
handful of GPU smoke tests (`tests/headless.rs`), which run on any machine with
a wgpu adapter and skip themselves otherwise.

## Layers

```
 guest bytes / raw state words
          |
          v
 +-------------------------------+   pure Rust, no GPU, unit tested
 | format  swizzle  dxt  convert |   Xbox D3DFORMAT, Morton, DXT, texel expansion
 | texture                       |   header parse, mip/cube/volume layout, decode
 | state   vertex                |   typed render/stage state, FVF, declarations
 | translate  pipeline           |   state -> wgpu descriptors, pipeline cache key
 +-------------------------------+
          |
          v
 +-------------------------------+   wgpu, still window-agnostic
 | context  renderer  upload     |   device, draw path, texture upload
 | presenter                     |   wgpu::Surface in, frames out
 +-------------------------------+
          ^
          |  wgpu::Surface + pixel size
 +-------------------------------+
 | platform layer                |   examples/window.rs (winit, macOS)
 |                               |   later: tvOS/UIKit shell, Xbox shell
 +-------------------------------+
```

The platform layer is the only code that knows about windows. It builds a
`wgpu::Surface` from whatever window object it has (`Instance::create_surface`
accepts anything implementing `raw-window-handle`), passes it to
`GfxContext::new` (to pick a compatible adapter) and `Presenter::new`, forwards
resize events to `Presenter::resize`, and drives `acquire` / `present` from its
own redraw loop. `Presenter` and `OffscreenTarget` both yield a `RenderTarget`,
so the renderer draws to a window or a texture identically (the tests use the
latter). A tvOS target means writing a new `examples/`-style shell and nothing
else; note that tvOS GPUs have no BC compression, which is why the CPU DXT
decoder exists (`DecodeOptions::bc_supported = false`).

## Texture handling

Pipeline: header words -> `TextureDesc` -> `layout()` (where each level/face
lives) -> `decode_texture` (unswizzle or strip pitch, convert) -> `upload_texture`.

* **Header** (`TextureDesc::from_header`): the `Format` and `Size` words of
  `X_D3DPixelContainer`. Swizzled and compressed textures have a zero `Size`
  and carry log2 dimensions in the format word; linear ones carry
  `width-1`, `height-1`, `pitch/64-1` in `Size`.
* **Swizzle** (`swizzle.rs`): Morton order. Bit positions are dealt out
  round-robin x, y, z from the LSB, skipping an axis once its bits are
  exhausted; so 2D is the classic interleave, and a non-square texture has the
  longer axis' surplus bits on top. Works for 2D, volumes, and (per face) cubes.
  Dimensions must be powers of two.
* **Mip chains**: levels are packed back to back with no padding, each level
  halving (minimum 1) in x, y and, for volumes, z.
* **Cube maps**: six faces (+X -X +Y -Y +Z -Z), each holding a full mip chain,
  with the face stride rounded up to 128 bytes.
* **DXT**: swizzled textures are never used for DXT; blocks are stored in
  row-major block order, 8 bytes (DXT1) or 16 bytes (DXT3/5), and a level
  smaller than 4x4 occupies one block. Blocks are bit-identical to S3TC, so
  they pass through to BC1/2/3 when the device has `TEXTURE_COMPRESSION_BC`
  (Metal on macOS does) and the base size is a multiple of 4. Otherwise the CPU
  decoder in `dxt.rs` produces RGBA8.

### Xbox D3DFORMAT -> wgpu

The Xbox `D3DFORMAT` values are the NV2A texture format codes, not the PC D3D8
values. Channel names read most-significant to least-significant in the
little-endian texel (`A8R8G8B8` is bytes `B,G,R,A`). "Sampled as" is what the
shader sees, following xemu's per-format swizzle table. Everything without an
exact wgpu format is expanded to RGBA8 on the CPU with those semantics baked in
(wgpu has no texture-view swizzle; a later optimisation can use R8/RG8 plus a
swizzle uniform).

| Hex | Xbox format | Layout | Bytes | Sampled as (r,g,b,a) | wgpu format | Path |
|---|---|---|---|---|---|---|
| 00 | L8 (Y8) | swizzled | 1 | (l,l,l,1) | Rgba8Unorm | expand |
| 01 | AL8 (AY8) | swizzled | 1 | (v,v,v,v) | Rgba8Unorm | expand |
| 02 | A1R5G5B5 | swizzled | 2 | argb | Rgba8Unorm | expand |
| 03 | X1R5G5B5 | swizzled | 2 | rgb, a=1 | Rgba8Unorm | expand |
| 04 | A4R4G4B4 | swizzled | 2 | argb | Rgba8Unorm | expand |
| 05 | R5G6B5 | swizzled | 2 | rgb, a=1 | Rgba8Unorm | expand |
| 06 | A8R8G8B8 | swizzled | 4 | argb | Bgra8Unorm | copy |
| 07 | X8R8G8B8 | swizzled | 4 | rgb, a=1 | Bgra8Unorm | copy, a=255 |
| 0B | P8 | swizzled | 1 | palette entry (A8R8G8B8) | Rgba8Unorm | palette expand |
| 0C | DXT1 | compressed | 8/block | rgba (1-bit alpha) | Bc1RgbaUnorm | passthrough or CPU |
| 0E | DXT2/DXT3 | compressed | 16/block | rgba | Bc2RgbaUnorm | passthrough or CPU |
| 0F | DXT4/DXT5 | compressed | 16/block | rgba | Bc3RgbaUnorm | passthrough or CPU |
| 10 | LIN_A1R5G5B5 | linear | 2 | as 02 | Rgba8Unorm | strip pitch, expand |
| 11 | LIN_R5G6B5 | linear | 2 | as 05 | Rgba8Unorm | strip pitch, expand |
| 12 | LIN_A8R8G8B8 | linear | 4 | as 06 | Bgra8Unorm | strip pitch, copy |
| 13 | LIN_L8 | linear | 1 | as 00 | Rgba8Unorm | strip pitch, expand |
| 16 | LIN_R8B8 | linear | 2 | (b1,b0,b0,b1) | Rgba8Unorm | expand |
| 17 | LIN_G8B8 (V8U8) | linear | 2 | (b0,b1,b0,b1) | Rgba8Unorm | expand |
| 19 | A8 | swizzled | 1 | (1,1,1,a) | Rgba8Unorm | expand |
| 1A | A8L8 | swizzled | 2 | (l,l,l,a), byte0=l | Rgba8Unorm | expand |
| 1B | LIN_AL8 | linear | 1 | as 01 | Rgba8Unorm | expand |
| 1C | LIN_X1R5G5B5 | linear | 2 | as 03 | Rgba8Unorm | expand |
| 1D | LIN_A4R4G4B4 | linear | 2 | as 04 | Rgba8Unorm | expand |
| 1E | LIN_X8R8G8B8 | linear | 4 | as 07 | Bgra8Unorm | copy, a=255 |
| 1F | LIN_A8 | linear | 1 | as 19 | Rgba8Unorm | expand |
| 20 | LIN_A8L8 | linear | 2 | as 1A | Rgba8Unorm | expand |
| 24 | YUY2 | linear | 2 | BT.601 -> rgb, a=1 | Rgba8Unorm | CPU convert |
| 25 | UYVY | linear | 2 | BT.601 -> rgb, a=1 | Rgba8Unorm | CPU convert |
| 27 | R6G5B5 (L6V5U5) | swizzled | 2 | rgb, a=1 | Rgba8Unorm | expand |
| 28 | G8B8 (V8U8) | swizzled | 2 | (b0,b1,b0,b1) | Rgba8Unorm | expand |
| 29 | R8B8 | swizzled | 2 | (b1,b0,b0,b1) | Rgba8Unorm | expand |
| 32 | L16 | swizzled | 2 | (1, byte1, byte0, 1) as xemu's `SZ_Y16` | Rgba8Unorm | expand (8-bit loss) |
| 35 | LIN_L16 | linear | 2 | as 32 | Rgba8Unorm | RG8 view, no loss |
| 37 | LIN_R6G5B5 | linear | 2 | as 27 | Rgba8Unorm | expand |
| 38 | R5G5B5A1 | swizzled | 2 | rgba | Rgba8Unorm | expand |
| 39 | R4G4B4A4 | swizzled | 2 | rgba | Rgba8Unorm | expand |
| 3A | A8B8G8R8 (Q8W8V8U8) | swizzled | 4 | bytes R,G,B,A | Rgba8Unorm | expand |
| 3B | B8G8R8A8 | swizzled | 4 | bytes A,R,G,B | Rgba8Unorm | expand |
| 3C | R8G8B8A8 | swizzled | 4 | bytes A,B,G,R | Rgba8Unorm | expand |
| 3D-41 | LIN_ twins of 38-3C | linear | 2/4 | as twins | Rgba8Unorm | strip pitch, expand |
| 2A,2B,2C,2D | D24S8, F24S8, D16, F16 | swizzled | 4/4/2/2 | depth | Depth24PlusStencil8 / Depth16Unorm (`depth_format_to_wgpu`) | surfaces only |
| 2E-31 | LIN_ depth twins | linear | | depth | as above | surfaces only |
| 33, 36 | V16U16, LIN_V16U16 | | 4 | signed bump | none | rejected (`Unsupported`) |

`V8U8`, `Q8W8V8U8` and `L6V5U5` share their codes with `G8B8`, `A8B8G8R8`
and `R6G5B5`, so they decode unsigned like their twins. Signed reads come
from `D3DTSS_COLORSIGN` (biased upload plus a conversion in the pixel
shader), not from the format; see `docs/shaders.md`, "Signed texture
formats".

Unknown or unverified details, flagged so nobody trusts them blindly:

* `YUY2`/`UYVY` byte order and BT.601 limited range are inferred from the
  format names in xemu (`CR8YB8CB8YA8`, `YB8CR8YA8CB8`) and the XDK; no
  capture has confirmed the colour matrix.
* `R6G5B5` bit placement (6 bits red on top) follows the name; xemu converts
  it to RGBA8 on the CPU and we did not read that converter.
* `L16` follows xemu's `SZ_Y16` swizzle `(ONE, GREEN, RED, ONE)` over an RG8
  upload: output `(1, byte1, byte0, 1)`. It is not a luminance ramp.
* DXT *volume* textures are decoded on the CPU (BC passthrough would need
  `TEXTURE_COMPRESSION_BC_SLICED_3D`). Their block layout in guest memory is
  unverified: xemu has a dedicated 3D S3TC path and we assume plain
  slice-by-slice blocks.
* Pre-transformed (`XYZRHW`) and vertex-program window z is taken as
  `[0, D3DZ_MAX]` of the depth format (`gfx::renderer::d3dz_max`: 16777215
  for D24S8, 65535 for D16, 511.9375 for F16, 3.4027977e38 for F24S8, the
  largest value of the Xbox float formats) and x/y get the D3D8 half-pixel
  offset. The built-in shader (`Renderer::set_pretransformed_z_max`) and
  `d3d::nv2a` use the same scale. Both are to be verified against captures;
  F24S8 z near zero falls into f32 denormals on the host.
* Depth bias (`POLYGONOFFSETZOFFSET`) is in depth-buffer steps and passed
  through unscaled (xemu hands it to `glPolygonOffset` as is). It is dropped on
  line/point topologies and gated by `SOLID/WIREFRAME/POINTOFFSETENABLE`.
* Depth textures sampled as colour (shadow maps) are not handled; xemu
  routes them through a depth-aware swizzle (see its `kelvin_color_format_gl_map`).
* Volume mip levels halve depth (resolved: xemu's per-level upload loop also
  halves depth; `pgraph_get_texture_length` only over-approximates the total).
* Compare-func values `0x200..0x207` are confirmed by Cxbx's
  `EmuXB2PC_D3DCMPFUNC`; the PC `1..=8` fallback is kept only defensively.

## State model

`state.rs` has typed enums with the Xbox raw values for every state that
affects pipeline construction (`CompareFunc`, `BlendFactor`, `BlendOp`,
`StencilOp`, `CullMode`, `FrontFace`, `FillMode`, `DepthBufferMode`,
`ColorWriteMask`, texture address/filter/op/arg), a `RenderState` struct with
D3D8 defaults, and `RenderStateType` / `TextureStageStateType` slot-index enums
so the HLE layer can forward raw `(index, value)` writes through
`RenderState::set_raw` / `TextureStageState::set_raw`.

Two hazards for the HLE layer:

* Render-state *indices* are XDK-build dependent. `RenderStateType` uses the
  canonical numbering (Cxbx's `X_D3DRS_*`, equal to 5849/5933 below index 154).
  `xdk::RenderStateTable` maps guest indices to canonical ones; it defaults to
  build 5849 (`HALO2_BUILD`, every library Halo 2 links). Cxbx keeps one
  canonical table annotated with the first build each state appears in (and
  the build it was removed), and derives a per-build index by counting the
  states present. For 5849 the only effect is that `MULTISAMPLETYPE` (154,
  removed in 4034) is absent, so guest states >= 155 sit one slot lower. Use
  `RenderState::set_guest(&table, guest_index, value)` from the HLE layer.
  Texture-stage indices for 5849 are assumed to follow the header numbering.
* The *values* are NV2A/OpenGL constants (`D3DBLEND_SRCALPHA = 0x302`), not the
  PC ones. `CompareFunc::from_xbox_or_pc` additionally accepts the PC numbering
  `1..=8` defensively; the Xbox values `0x200..0x207` are confirmed.

`vertex.rs` covers primitive types (including the Xbox-only line loop, quad
list/strip and polygon, which `PrimitiveType::to_host` rewrites to wgpu
topologies with index lists), FVF decoding, and `D3DVSD_*` declaration parsing
into stream layouts. Vertex data types with no wgpu equivalent
(`SHORT1/3`, `NORMSHORT1/3`, `PBYTE1/3`, `NORMPACKED3`, `FLOAT2H`) are handled
by `repack.rs`: `RepackPlan::new(layout, strides)` plans a per-stream
conversion and `StreamPlan::repack` returns a `Cow` (borrowed when the stream
is an identity, otherwise compacted with 4-byte-aligned strides). Shorts are
converted to Float32 because Xbox vertex shaders consume floats. The
`NORMPACKED3` field order (x in the low bits) is verified. The register number is the wgpu
shader location. Multi-stream draws are supported via `Draw::streams`.

### Render targets and surfaces (`surface.rs`, `encode.rs`, `blit.rs`, `readback.rs`)
`SurfaceStore` owns colour and depth/stencil surfaces. Tiling is a recorded
property (pitch plus `TileRegion`), not emulated; readbacks are encoded to the
guest format and layout (`encode_color`, `encode_depth`). Operations: set
render target, `CopyRects`/`Resolve` (texture copy, or the blit pipeline when
format or size differs or alpha is forced opaque), stretch, and `Swap` (ring
`[back0.., front]` rotated on flip; COPY leaves it alone; FINISH waits for the
GPU). Depth uses `Depth32FloatStencil8` when supported so depth and stencil
can be read back. MSAA surfaces keep a single-sample image plus an attachment.
A depth surface larger than its colour surface (common on the Xbox) is
supported through `prepare_target`/`finish_target`: the colour surface gets a
depth-sized stand-in attachment, seeded from and copied back to the real
surface, with draws clipped by viewport and scissor to the colour size (not
available for MSAA colour). Guest uploads and readbacks both use the surface's
own `SurfaceLayout`.
Render-to-texture: `create_texture_target(desc, levels)` makes a colour
surface with a mip chain that is both an attachment and a texture;
`level_target(id, level, depth)` draws into one level (level 0 also through
`target`), and `Surface::view` samples the whole chain. No MSAA there.
`write_color(ctx, id, &TextureDesc, bytes)` fills level 0 of a colour
surface from guest texels laid out as the `TextureDesc` says (its format,
and its pitch for linear formats, which may differ from the surface's own),
so a render-to-texture level starts from its texture's memory.
Limits: `F16` and
`F24S8` depth cannot be encoded; tile compression and bank interleave are not
modelled.

### Shader hook
`ShaderProgram` plus `Renderer::register_shader(ShaderId, ...)`; a draw picks
one with `Draw::shader`, and `PipelineKey::shader` keys the cache. Extra
resources go in `Draw::bind_groups` (`BoundGroup`: index, group and its
dynamic offsets). `program::ProgramBindings` implements
the resource contract of `crates/shaders` (docs/shaders.md): group 0 holds
the vertex and pixel uniforms (dynamic offsets into one upload ring; bytes
equal to the previous draw's reuse its copy), group 1 a texture and sampler
per sampled stage (cached by views and sampler keys, dropped after four
frames unused). It builds and caches the layouts per set of stage dimensions,
wraps a linked module into a `ShaderProgram`, caches samplers by
`SamplerKey`, and binds a 1x1 opaque-black stand-in for a stage with nothing
usable bound. `d3d::nv2a::Nv2aShaders` is the user: it translates each draw's
vertex program and combiner program (or the fixed-function texture stages)
and registers one program per distinct translation.

### Per-draw raster state and clears
* `Draw::viewport` (`DrawViewport`: rectangle plus depth range) is a wgpu
  viewport; it must lie inside the target. `None` is the whole target.
  Shaders that produce window coordinates (the built-in one, translated
  NV2A programs) leave it `None`: the D3D viewport has already been applied
  to their positions, so they clip with a scissor instead.
* `Draw::scissor` (`ScissorRect`) is clamped to the target; an empty one
  draws nothing.
* `Draw::indices` draws a `u32` index list over the streams (each index
  below `vertex_count`); Xbox-only primitive types are rewritten on top of
  it.
* The built-in shader runs the alpha test from `RenderState`
  (`ALPHATESTENABLE`, `ALPHAFUNC`, `ALPHAREF`, comparing the rounded 8-bit
  alpha) with a per-draw uniform slot: a dynamic offset into the
  renderer's uniform ring, so draws recorded before one submit keep their
  own values.
* An indexed strip (`TriangleStrip`/`LineStrip` with `Draw::indices`) sets
  the pipeline's strip index format (`Uint32`).
* `Renderer::clear_rects(target, MaskedClear, rects)` clears rectangles and
  any subset of colour channels, depth and stencil in one pass: a whole
  target with every channel uses load operations; anything else draws a
  scissored full-screen triangle whose pipeline writes the channel mask,
  replaces depth (compare always) and replaces stencil.

### Batching (`Renderer::record`, `encode_pass`, `ring.rs`)
`Renderer::render` encodes a pass of draws at once; `GfxBackend` instead
records draws one at a time and encodes a whole pass later.
`Renderer::record(ctx, target, draw)` validates a draw, fetches its pipeline
and stages its vertices, indices and built-in uniforms in upload rings
(`ring::UploadRing`: CPU-side staging packed into one buffer, written with
one `queue.write_buffer` per flush) and returns a `RecordedDraw`.
`Renderer::encode_pass(ctx, encoder, target, clear, draws, counter)` encodes
them into one render pass, or a new one at each draw marked with
`RecordedDraw::start_pass` and at each counted draw (see `occlusion.rs`),
and returns how many it opened. Within a render pass it skips viewport,
scissor, pipeline, blend-constant and bind group calls that repeat the
previous draw's. A counted draw that writes neither depth nor stencil
replays its count pass on the real depth/stencil attachment instead of a
scratch copy. The owner calls
`Renderer::flush_uploads(queue)` (and `ProgramBindings::flush_uploads`)
before the submit that runs the draws, and `submitted()` after it, which
lets the rings start over: queued writes are ordered after work already
submitted, so a region reused after the reset never races a draw still
reading it. A ring that fills moves to a new, larger buffer; draws recorded
earlier keep the old one.

### Translation (`translate.rs`) and pipeline cache (`pipeline.rs`)

| D3D state | wgpu |
|---|---|
| `ALPHABLENDENABLE`, `SRCBLEND`, `DESTBLEND`, `BLENDOP` | `BlendState` (same factors on colour and alpha) |
| `BLENDOP = MIN/MAX` | factors forced to `One/One` (wgpu requires it) |
| `BLENDCOLOR` | `set_blend_constant` at draw time; `CONSTANTALPHA` approximated by splatting alpha |
| `COLORWRITEENABLE` | `ColorWrites` |
| `ZENABLE`, `ZWRITEENABLE`, `ZFUNC` | `DepthStencilState`; disabled = `Always`, no write; W-buffer treated as Z |
| `STENCIL*` | front = back `StencilFaceState`, masks; `STENCILREF` is a draw-time `set_stencil_reference`; ignored on depth-only formats |
| `POLYGONOFFSET*` | `DepthBiasState`; the constant is scaled by `2^depth_bits`, approximate and unvalidated |
| `CULLMODE`, `FRONTFACE` | as D3D programs the NV2A (`SetRenderState_CullMode`, `0x3f53d0`): `front_face` is `FRONTFACE`, `cull` is `Front` when `CULLMODE == FRONTFACE`, else `Back`. Culling depends on `CULLMODE` alone; `FRONTFACE` decides `front_facing` (two-sided lighting) |
| `FILLMODE` | `PolygonMode` (`Line`/`Point` need `POLYGON_MODE_LINE/POINT`; falls back to `Fill`) |
| `BACKFILLMODE` | not expressible in a pipeline; ignored |
| `ALPHATEST*` | built-in shader: per-draw uniform and `discard`; translated programs: `PixelOptions::alpha_test` |
| `FOG*`, `SHADEMODE`, `TEXTUREFACTOR` | shader inputs (`d3d::nv2a` feeds them to the translated programs) |
| stage address/filter/mip/aniso | `SamplerKey` -> `SamplerDescriptor` (hashable; `MIPMAPLODBIAS` not representable); Border address needs `ADDRESS_MODE_CLAMP_TO_BORDER` and the colour snaps to one of wgpu's three presets; quincunx and gaussian-cubic filters approximate to linear |
| stage ops/args (`COLOROP`, ...) | typed; `d3d::nv2a::fixed_function_def` turns them into a register-combiner program |

`PipelineKey` is built from the *translated* wgpu descriptors (`PrimitiveState`,
`DepthStencilState`, `ColorTargetState`, vertex layouts) plus a shader id and
sample count, so states that translate identically share one pipeline: changing
the stencil reference, blend constant, alpha ref or fog colour never rebuilds
one, and a `QUADLIST` draw reuses the `TRIANGLELIST` pipeline. `PipelineCache<V>`
is generic over the stored value so its behaviour is unit tested without a GPU;
`Renderer` instantiates it with `wgpu::RenderPipeline`.

## What exists end to end

`Renderer::render` takes `Draw { shader, streams, vertex_count, layout,
primitive, state, bind_groups, viewport, scissor, indices }` (`Draw::new`
fills the last four with defaults); the one built-in shader is for
pre-transformed `XYZRHW | DIFFUSE` vertices (`shaders/xyzrhw_diffuse.wgsl`). The shader inverts
the viewport mapping and keeps the guest's `rhw` interpolation. The tests draw
offscreen on Metal and read pixels back to prove cull direction and alpha
blending, including an Xbox `QUADLIST`. `examples/window.rs` opens a window,
draws a triangle and a blended quad, and sets all state via raw Xbox values
(`C` cycles cull, `W` wireframe, `B` blending; `--frames N` exits after N
frames for smoke tests). `tests/draw_features.rs` checks viewport, scissor,
indexed draws, the built-in alpha test, partial and channel-masked clears and
render-to-texture levels by pixel.

Translated shaders: `d3d::GfxBackend` renders every draw through
`d3d::nv2a::Nv2aShaders` (vertex programs and register combiners from
`crates/shaders`, and the fixed-function texture stages as a synthesized
combiner program) unless the built-in shader reproduces the whole state
(`d3d::nv2a::builtin_equivalent`: no texture, pixel shader, fog, specular,
flat shading or two-sided lighting, stages passing diffuse through).
Fixed-function stages follow D3D: `CURRENT` is diffuse until a stage writes
it (a stage writing `TEMP` leaves it), results saturate to [0, 1],
`MULTIPLYADD` is `arg0 + arg1 * arg2`, and an operation reading an unbound
texture passes `CURRENT` through (as Wine documents D3D doing). With
`FOGENABLE` off a combiner program's fog register reads factor 1. `crates/d3d/tests/shaders_render.rs` draws a
UI-like quad (vertex program with constants and a default input, combiner,
linear texture, alpha blend and alpha test) and a fixed-function textured
quad through the device and checks pixels; `cargo run -p d3d --example
window` shows the same through `crates/app` in a real window.

`crates/viewer` is the first consumer of real assets: it decodes a `mode` tag
(`tags::model`, see map-format.md "Render-model geometry") and draws it with
its own small WGSL pipeline through `GfxContext`/`Presenter`, or into an
`OffscreenTarget` for `--png` screenshots. Flat, vertex-normal or normal-colour
shading only; no shaders or textures yet. Its `bsp` binary validates and
software-renders structure BSPs through the same shared geometry decoder
(`tags::geometry`, see structure-bsp.md).

## Not supported yet (future work)

The two sections below are history: the translators now exist in
`crates/shaders` (docs/shaders.md) and are wired in by `d3d::nv2a`. What is
still missing there: untransformed fixed-function vertices (transform,
lighting, texgen), fixed-function bump-env ops, `MIPMAPLODBIAS`, colour key,
and point sprites.

### Pixel processing: register combiners

The NV2A has no programmable pixel shaders. D3D8 `.psh` programs and the
fixed-function texture stages both compile to 8 general register-combiner
stages plus a final combiner, which the D3D runtime writes into the `PS*`
render-state slots (indices 0..56 in `RenderStateType`'s numbering:
`PSALPHAINPUTS*`, `PSRGBINPUTS*`, `PSFINALCOMBINERINPUTS*`, `PSCONSTANT*`,
`PSCOMBINERCOUNT`, `PSTEXTUREMODES`, ...). `RenderState` currently ignores these
and `TextureStageState` only stores the fixed-function ops.

To do: decode the combiner words into a typed description (input mapping, A*B
+ C*D, mux/sum, scale/bias, output mapping, final combiner `A*B + (1-A)*C + D`,
and the texture-mode / dot-product / bump-env modes that feed it), generate WGSL
per distinct combiner state, and cache the shader module alongside the
pipeline (the shader id in `PipelineKey` is already there for this). How others
do it:

* Cxbx-Reloaded translates to D3D9 pixel shader assembly/HLSL:
  `src/core/hle/D3D8/Direct3D9/PixelShader.cpp` and
  `src/core/hle/D3D8/XbPixelShader.cpp` (commit `585c49a5`).
* xemu generates GLSL from the combiner registers:
  `hw/xbox/nv2a/pgraph/glsl/psh.c` (commit `4bdad199`); it also has a Vulkan
  renderer under `hw/xbox/nv2a/pgraph/vk/` sharing that generator.

Both are GPL; read them for behaviour, do not copy.

### Vertex processing: vertex shader microcode

Xbox vertex shaders are NV2A microcode: 128-bit instructions, a 136-slot
program, 192 constant registers (`c[-96..95]`), `v0..v15` inputs and a fixed
output register set, with its own instruction set (MAD/ADD/MUL/MOV/DP3/DP4/DPH/
RCP/RSQ/EXP/LOG/LIT/DST/MIN/MAX/SLT/SGE/ARL, one scalar and one vector op per
instruction). It is neither SM1.1 bytecode nor anything wgpu consumes.

To do: a microcode decoder, then a WGSL generator (scalar+vector co-issue to
sequential statements; constants in a uniform/storage buffer; `ARL` address
register; output register to varying mapping, including `oFog`, `oPts`,
`oB0/oB1`). Plus the fixed-function vertex pipeline when no shader is set
(transform, lighting, texgen, fog), which on hardware is *also* a vertex
program generated by the D3D runtime; Cxbx and xemu both synthesise one.
Pointers:

* Cxbx: `src/core/hle/D3D8/Direct3D9/VertexShader.cpp`,
  `src/core/hle/D3D8/XbVertexShader.cpp` (microcode -> D3D9 vertex shader
  assembly, plus the declaration remapper).
* xemu: `hw/xbox/nv2a/pgraph/glsl/vsh.c` (microcode -> GLSL),
  `hw/xbox/nv2a/pgraph/glsl/vsh-ff.c` (fixed-function replacement program),
  `hw/xbox/nv2a/pgraph/vsh_regs.h` (field layouts).

Halo 2 almost certainly uses both programmable vertex shaders and the pixel
combiner path heavily, so these are the largest remaining pieces of the
renderer, not polish.

### Smaller gaps

* Palettes are passed in by the caller as a `&[u32]`; locating them from the
  guest `D3DPalette` is the HLE layer's job.
* Multisampling (`MULTISAMPLE*` states, `D3DMULTISAMPLE_TYPE` quincunx etc.),
  point sprites/size, line width, edge AA, dithering.
* Occlusion counts (`gfx::occlusion::PixelCounter`, `Draw::count_samples`)
  re-draw each counted draw into an additive `R16Float` target, against a
  scratch copy of depth/stencil with the draw's own writes, and sum it with
  a compute pass, because wgpu occlusion queries are boolean on Metal.
  The count pass does not run the draw's fragment program, so alpha test and
  `discard` do not reject samples. See "Visibility tests" in `d3d.md`.
* Push-buffer/NV2A method-stream paths: out of scope by design. Inlined D3D
  fast paths must be intercepted at the call/region level (LIFTER.md section 7;
  Cxbx's `XbPushBuffer.cpp` shows what a draw submission has to preserve).
* Texture border colour sources and `D3DTSS_COLORKEY*` are not modelled
  (no retail Halo 2 material sets `COLORKEYOP`). `ALPHAKILL` and signed
  colour channels (`COLORSIGN`) are handled in the translated pixel
  programs (docs/shaders.md).

## References (pinned)

* Cxbx-Reloaded `585c49a50af1255ab155099e06f24505f9c5a800`:
  `src/core/hle/D3D8/XbD3D8Types.h` (format table, header masks, `X_D3DRS_*`,
  `X_D3DTSS_*`, `X_D3DTOP_*`, `X_D3DPT_*`, `X_D3DFVF_*`, `X_D3DVSD*`).
* xemu `4bdad199102b22ed0cfa998e9af6b35d76238b3e`: `hw/xbox/nv2a/nv2a_regs.h`
  (`NV097_*` constants), `hw/xbox/nv2a/pgraph/texture.c` (layout and sizes),
  `hw/xbox/nv2a/pgraph/swizzle.c`, `hw/xbox/nv2a/pgraph/gl/constants.h`
  (per-format channel swizzles).

Both projects are GPL-licensed; this crate contains no code from
either. Constants and layouts are facts about the hardware and SDK ABI that we
re-derived from their descriptions and cite for traceability. The repository
license is `MIT OR Apache-2.0`; keep it that way by describing, not copying,
when reading these sources.
