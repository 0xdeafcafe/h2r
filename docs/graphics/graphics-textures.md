# Graphics: texture decoding

Status: Built. Sub-page of [graphics.md](graphics.md).
Code: `crates/platform/gfx/src/` (`texture.rs`, `swizzle.rs`, `dxt.rs`, `convert.rs`, `format.rs`). Tests: `crates/platform/gfx/tests/`.
Switch: none.

| Stage | Function | What it does |
|---|---|---|
| 1 | `TextureDesc::from_header` | reads the format and size words of `X_D3DPixelContainer` |
| 2 | `layout()` | where each mip level and cube face lives |
| 3 | `decode_texture` | unswizzles or strips the pitch, converts to a wgpu format |
| 4 | `upload_texture` | writes the levels to the GPU |

## Layout rules

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

## Xbox D3DFORMAT to wgpu

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
shader), not from the format; see `docs/graphics/shaders.md`, "Signed texture
formats".

## Not verified

* `YUY2`/`UYVY` byte order and BT.601 limited range are inferred from the
  format names in xemu (`CR8YB8CB8YA8`, `YB8CR8YA8CB8`) and the XDK; no
  capture has confirmed the colour matrix.
* `R6G5B5` bit placement (6 bits red on top) follows the name; xemu converts
  it to RGBA8 on the CPU; the converter was not read.
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
* Depth-format textures (`D24S8`, `F24S8`, `D16`, `F16`, swizzled or linear)
  decode to their raw bits as RGBA8 (`convert::depth_channels`): 32-bit
  formats give the depth bytes, most significant first, in red, green and
  blue and the stencil in alpha; 16-bit formats give the two depth bytes in
  red and green, blue and alpha zero. `round(c * 255)` recovers each byte
  under point sampling. The NV2A does not sample them as colour: they are
  shadow maps, compared with the texture coordinate under `SHADOWCTL`'s
  function (D3D's `D3DRS_SHADOWFUNC`), giving 1 or 0 in every channel; xemu's
  `psh_append_shadowmap` compares with 0 for PROJECT2D and with `r/q`,
  clamped to the format's depth range (16777215, 65535, or 511.9375 for
  float), for PROJECT3D, and short-cuts NEVER and ALWAYS. `crates/formats/shaders`
  compares per texel and then filters (2x2 PCF under linear filtering; see
  docs/graphics/shaders.md, "Shadow maps"), with `d3d::nv2a` passing each stage's
  format, function, filter and addressing. A colour texture over depth memory is not a depth texture:
  it reads the same bits through its own colour format.
* Volume mip levels halve depth (resolved: xemu's per-level upload loop also
  halves depth; `pgraph_get_texture_length` only over-approximates the total).
* Compare-func values `0x200..0x207` are confirmed by Cxbx's
  `EmuXB2PC_D3DCMPFUNC`; the PC `1..=8` fallback is kept only defensively.
