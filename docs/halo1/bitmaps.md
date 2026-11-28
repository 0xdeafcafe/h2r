# Halo 1 Xbox bitmaps (`bitm`)

How Halo 1's Xbox maps store bitmaps, read by `h1map::bitmap`, decoded by
`crates/tools/h1-assets` with Halo 2's texture decoder, and converted to Halo 2
`bitm` tags for the Halo 1 → Halo 2 map converter
([multiplayer.md](multiplayer.md)). The cache file around them
is in [map-format.md](map-format.md); Halo 2's bitmaps are in
[bitmaps.md](../formats/bitmaps.md). All integers are little-endian, offsets
hexadecimal.

Evidence labels, as in map-format.md:

- **V - verified-on-disc:** checked on every map of the owner's Halo 1 disc
  by the `#[ignore]`d tests below (24 maps, 9,327 `bitm` tags, 14,815
  images).
- **S - unverified:** the reader relies on the field but the disc can't
  confirm its meaning.
- **Design:** an implementation choice.

No pixels, tag dumps or tag-name lists are in the repository. Tests invent
their images; the disc tests read `$H1_ISO` at run time.

## Where the pixels are

**V:** each image's pixel data offset (`+0x18`) is a **file offset in the
decompressed map** (the `0x800`-byte header followed by the inflated body),
and `+0x1c` is its size. On every map the pixels sit after the structure
BSP regions and before the tag data: one run per map (beavercreek
`0xb1800..0x16e7080`, BSP region `0x800..0xb1800`, tag data from
`0x22ab800`), with no image overlapping another, a BSP region or the tag
data. Xbox Halo 1 has no `bitmaps.map`; every image's pixels are in the map
that uses them (shared bitmaps are stored once per map).

## `bitm` header (`0x6c` bytes)

| Offset | Type | Field | Evidence |
|---|---|---|---|
| `00` | u16 | Type: 2D textures, 3D textures, cube maps, sprites, interface bitmaps | S |
| `02` | u16 | Format (the import's compression choice) | S |
| `04` | u16 | Usage: alpha blend, default, height map, detail map, light map, vector map | S |
| `06` | u16 | Flags | S |
| `08`, `0c`, `10` | f32 | Detail fade factor, sharpen amount, bump height | S |
| `14`, `16` | u16 | Sprite budget size, count | S |
| `18`, `1a` | u16 | Colour plate width, height | S |
| `1c`, `30` | data refs (20 bytes) | Compressed colour plate, processed pixel data (empty in cache files) | S |
| `44`, `48` | f32 | Blur filter size, alpha bias | S |
| `4c` | u16 | Mipmap count requested | S |
| `4e`, `50` | u16 | Sprite usage, sprite spacing | S |
| `54` | block | Sequences, `0x40` each | V (every block resolves on every map) |
| `60` | block | Bitmap data (images), `0x30` each | V |

Sequence (`0x40`): name (32 bytes) `00`, `i16` first image `20`, `i16`
image count `22`, sprites block `34` (`0x20` each). Sprite: `i16` image
`00`, left, right, top, bottom (f32) `08`, registration point (2 × f32)
`18`. **S** for the meanings; **V** for the strides (every block resolves).
These are Halo 2's sequence and sprite layouts with 12-byte blocks.

## Bitmap data element (`0x30` bytes)

| Offset | Type | Field | Evidence |
|---|---|---|---|
| `00` | FourCC | `bitm` | V: every element |
| `04`, `06` | u16 | Width, height | V: decodes at this size |
| `08` | u16 | Depth (volume slices; 1 otherwise) | V |
| `0a` | u16 | Type: 0 2D, 1 3D, 2 cube, 3 white | V for 0..2; 3 never seen |
| `0c` | u16 | Format, Halo 2's numbers: 0 A8, 1 Y8, 2 AY8, 3 A8Y8, 6 R5G6B5, 8 A1R5G5B5, 9 A4R4G4B4, 10 X8R8G8B8, 11 A8R8G8B8, 14 DXT1, 15 DXT3, 16 DXT5, 17 P8 bump | V by size for all; colour as Halo 2's (below) |
| `0e` | u16 | Flags: 1 power of two, 2 compressed, 4 palettized, 8 swizzled, `0x10` linear | V: DXT `0x3`, P8 bump `0xd`, other swizzled `0x9`, linear `0x11`; `0x80` unknown, set on lightmaps and some others. Halo 2 adds `0x200` on every image (the texture cache loads it; without it nothing is bound, E `0x0012ce81`); the converter sets it (`IMAGE_CACHED`) |
| `10`, `12` | i16 | Registration point | S |
| `14` | u16 | Mipmap levels **below** the base | V: by size |
| `18` | u32 | Pixel data file offset | V (above) |
| `1c` | u32 | Pixel data size | V |
| `20` | u32 | The bitmap tag's ID | S (not used) |
| `24`..`2f` | | Runtime pointer, padding | S (not used) |

## Layout of the pixels

**V, every image on the disc:** an image's pixel size is exactly Halo 2's
layout for the same descriptor rounded up to 128 bytes
(`assets::texture_desc` + `gfx` layout). Since a size can't tell apart
two layouts of the same size, this together with the decodes below pins:

- the base level then each mip, smallest last, in one chain;
- DXT chains stop at the first level that fits one 4×4 block (no 2×2 or
  1×1 levels), as Halo 2;
- cube maps: six faces (+X, −X, +Y, −Y, +Z, −Z), each a whole chain,
  128-byte aligned;
- volumes: each level holds its slices one after another;
- linear images: one level, rows padded to 64 bytes;
- swizzled images use the Xbox swizzle (interleaved x and y bits, x
  lowest), the same as Halo 2: decoded by `gfx`'s unswizzle.

So Halo 1's chain bytes are exactly what a Halo 2 LOD 0 holds for the same
format, size and flags: conversion copies them.

## Census of the disc

`every_bitmap_on_the_disc_decodes`, 24 maps, 14,815 images, every one
decoded, no size mismatch:

| Images | Bytes | Format | Type | Storage |
|---:|---:|---|---|---|
| 5363 | 226,555,136 | DXT1 | 2D | |
| 305 | 6,038,016 | DXT1 | cube | |
| 2816 | 180,569,344 | DXT3 | 2D | |
| 157 | 31,495,808 | DXT5 | 2D | |
| 2263 | 122,925,056 | R5G6B5 | 2D | swizzled |
| 65 | 8,028,672 | R5G6B5 | cube | swizzled |
| 1269 | 8,229,632 | AY8 | 2D | swizzled |
| 24 | 900,096 | AY8 | 3D | swizzled |
| 628 | 10,476,544 | X8R8G8B8 | 2D | swizzled |
| 155 | 4,688,640 | X8R8G8B8 | cube | swizzled |
| 96 | 36,864 | X8R8G8B8 | 3D | swizzled |
| 616 | 32,236,928 | A8Y8 | 2D | swizzled |
| 23 | 1,725,184 | A8Y8 | 3D | swizzled |
| 422 | 31,011,072 | A8R8G8B8 | 2D | swizzled |
| 24 | 792,576 | A8R8G8B8 | cube | swizzled |
| 305 | 61,530,880 | P8 bump | 2D | swizzled, palettized |
| 173 | 29,334,016 | A1R5G5B5 | 2D | swizzled |
| 55 | 1,258,368 | Y8 | 2D | swizzled |
| 23 | 3,020,544 | Y8 | cube | swizzled |
| 24 | 196,608 | A4R4G4B4 | 2D | linear |
| 2 | 65,536 | A4R4G4B4 | 2D | swizzled |
| 7 | 611,968 | A8 | 2D | swizzled |

No image uses P8 (format 18), the float formats or type 3 ("white"). No DXT
volume. 476 images decode to a single colour; the 264 of them whose stored
bytes vary are all 4×4 rasterizer defaults (solid colours with mips).
Larger images all decode to more than one colour.

**P8 bump** (bump maps): the tag carries no palette; the Xbox engine binds
one of its own. Like Halo 2's P8 images (`assets`, `Evidence::Hypothesis`),
the decode shows the raw index as luminance: the layout is verified, the
colours are not.

**Lightmaps** (`every_bsp_lightmap_page_resolves`): 82 structure BSPs, each
with its own lightmap bitmap (none shared); 2,288 lightmaps, 89 of them
with no page (`-1`). The other 2,199 use 2,198 distinct images: one BSP
(c10's `c10_exteriora`) has two lightmaps on one page and one page no
lightmap names. So the structure writer can't assume one lightmap per page. Every lightmap page is **R5G6B5 2D, swizzled,
no mips**, flags `0x89`. Bit `0x80` is set on other images too (the
rasterizer's normalisation cube map has it); its meaning is unknown, Halo
2's decoder ignores it and the conversion keeps it. The census above counts
flags masked to the low five bits.

## Decoding (`crates/tools/h1-assets`)

```rust
use h1_assets::bitmaps::BitmapExt;
let bm = h1map::bitmap::Bitmap::read(&map, tag)?;   // header, sequences, images
let img = bm.decode(&map, index, mip)?;            // Rgba8Image { width, height, rgba }
let levels = bm.decode_levels(&map, index)?;       // every face and level (assets::Level)
let bytes = bm.pixels(&map, index)?;               // the stored chain
```

`decode` stacks a cube's six faces (+X, −X, +Y, −Y, +Z, −Z) or a volume's
slices top to bottom, so `height` is the face height × 6 or × depth.
`h1_assets::bitmaps::decode::h2_image` restates a Halo 1 image as a `tags::Image`
and Halo 2's `assets::texture_desc` and `assets::bitmap::decode_bytes` do
the rest: one decoder for both games, with Halo 2's descriptor checks
(both or neither storage flag, mips on a linear image, non-square cubes,
DXT volumes are refused). Formats are paired with Xbox formats by
`assets::FORMAT_TABLE`, so its evidence column applies here too.

`h2 h1 bitmap <iso|map> [map] <tag> --out DIR [--mips] [--force]` prints a
bitmap's images and writes each as PNGs (one per image, cube face and
volume slice; every level with `--mips`).

## Conversion to Halo 2

`h1_assets::bitmaps::convert_bitmap(&map, tag)` returns the Halo 1 bitmap, an
`h1_assets::models::Converted { group: bitm, root, raw }`, and per image what
was cut (`ConvertedImage`). `Converted::install(&mut set, name)` adds it to a
`map::build::TagSet`. Three more entry points start from the same images:
`convert_bitmap_opaque` forces every image's alpha opaque
(`force_opaque`), `bitmaps::mask` bakes a model shader's reflection mask
into its base map's alpha as DXT5, and `bitmaps::atlas::compose` builds the
per-cluster lightmap pages ([multiplayer.md](multiplayer.md), "Lightmap").

| Halo 2 | From Halo 1 |
|---|---|
| Header `00`..`1a` | Type, format, usage, flags, detail fade, sharpen, bump height, sprite budget, colour plate size: the same fields |
| `2c`, `30`, `34`, `36`, `38` | Blur, alpha bias, mipmap count, sprite usage, sprite spacing |
| `1c`, `24` (data refs), `3a` forced format | Empty, 0 |
| Sequences (`0x3c`), sprites (`0x20`) | Field for field |
| Image `04`..`14` | Width, height, depth (a byte in Halo 2; larger is refused), type, format, flags, registration, mip count: the same numbers |
| Image LOD 0 (`1c`, size at `34`) | A raw resource: the Halo 1 chain, at the Halo 2 layout's size rounded up to 128 bytes (on the disc that is always exactly the Halo 1 size: nothing cut or padded) |
| LODs 1 and 2 | Absent (size 0), as Halo 2 leaves them on small textures |
| Image `4c` runtime ID | The tag's own datum, as every retail Halo 2 image (V, 5,009 images on three Halo 2 multiplayer maps) |
| Image `09`, `16`, `17`, `18`, `60`..`6e` | 0: runtime flags, two bytes that vary on retail with no pattern found (`lod_bias` and `cache_usage` in the schema), pixel offset, low-detail image |

Nothing is re-encoded: every Halo 1 format has the same Halo 2 format
number and layout, P8 bump included.

**Proof** (`crates/tools/h1-assets/tests/bitmaps/real_disc.rs`,
`beavercreek_bitmaps_convert_and_decode_identically` and
`a30_lightmaps_convert_and_decode_identically`): every bitmap of beavercreek (357 tags,
501 images) and both of a30's lightmap bitmaps (59 pages) are converted,
installed in one built map, read back with Halo 2's tag decoder
(`tags::DecodedTag`) and decoded with Halo 2's bitmap decoder; every face
and level is **byte-identical** to the Halo 1 decode, every LOD size is the
layout rounded to 128 and every runtime ID names its tag. The unit test
`a_conversion_reads_back_through_halo_2s_decoders`
(`crates/tools/h1-assets/src/bitmaps/tests.rs`) does the same on an
invented cube map and sprite sheet, and checks every header, sequence and
sprite field.

## Lightmaps for the structure writer

`h1_assets::bitmaps::lightmap_pages(&map, &structure_bsp)` returns the
`sbsp`'s lightmap bitmap reference (`sbsp+0x00`), its tag, and per Halo 1
lightmap (the `sbsp+0x104` block, in the order `StructureBsp::lightmaps`
returns) the page it samples, `None` for `-1`. `convert_bitmap` on that tag
gives the Halo 2 lightmap bitmap, one image per page, R5G6B5 2D. The
editor's Halo 1 view reads pages this way
(`crates/tools/editor-core/src/halo1.rs`).

The structure writer does not copy those pages. A Halo 2 cluster samples
one page and a Halo 1 cluster's surfaces sample several, so
`h1convert::clusters` builds a page per lit cluster from pieces of the
Halo 1 pages (`h1_assets::bitmaps::atlas`) and writes the render info
entry `{page, -1}`, which the native world decodes direct as a colour page ([lighting.md](../formats/lighting.md)); unlit clusters keep `{-1, -1}`. The design is in
[multiplayer.md](multiplayer.md), "Lightmap"; the target layouts are in
[lighting.md](../formats/lighting.md) and
[structure-bsp.md](../formats/structure-bsp.md). Pages are R5G6B5, never
P8: the renderer stores the page whatever the palette and binds one only
when named (E `0x0004d40a`), and the CPU query that lights objects looks a
sample up in a palette only when the image is P8 (E `0x000d309a`). One BSP
has two lightmaps on one page, so no code may assume one lightmap per page.

## Tests

```
cargo xtask test -p h1map -p h1-assets
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo xtask test --disc -p h1map -p h1-assets
```

The second line runs the `#[ignore]`d disc tests of both crates
(`every_bitmap_on_the_disc_decodes`, `every_bsp_lightmap_page_resolves`,
`halo2_image_fields_survey`, `real_disc_every_bitmap_reads`).

Synthetic tests: every format's known texel, swizzle addresses on square
and non-square images, DXT1/3/5 blocks (colour codes, explicit and
interpolated alpha), the DXT chain rule, mip order, cube face alignment and
order, volume slices, refusals, and a conversion read back through Halo 2's
decoders.
