# Bitmap tags -> gfx textures (`crates/assets`)

`h2 bitmap <map> <tag> [--image N] [--out DIR] [--mips] [--raw]` and
`h2 bitmap --all <map> [--list] [--out DIR]` decode `bitm` tags to RGBA8 PNGs.
Mapping rules live (with evidence labels) in the module docs of
`crates/assets/src/bitmap.rs`; this page records what the disc runs showed.
No pixels or names of game assets are stored in the repo.

Findings (retail disc, three maps: the main menu and two single-player maps;
all 3 maps: zero decode failures, 100% of images satisfy
"LOD size == layout size rounded up to 128 bytes"):

* Each of the 3 LOD pointers addresses a complete mip chain starting at mip
  0/1/2. `mip_count` = levels below the base.
* DXT chains stop at the first level that fits one 4x4 block; the 2x2/1x1
  levels D3D would pad to a block each are not stored. `gfx` layout needs
  the level count capped (done in `assets`).
* Linear images have rows padded to 64 bytes.
* Cube maps: faces 128-byte aligned, as `gfx` assumes (4x4 XRGB cube and DXT1
  cubes decode with coherent faces).
* Format pairings carry a typed `Evidence` (`Pixels` / `LayoutOnly` /
  `Hypothesis`) in `FORMAT_TABLE`. `V8U8`/`G8B8` are hypotheses (a size check
  cannot separate same-width formats); channel order for every other format is
  also pinned by synthetic test vectors.
* DXT volumes are refused: the stored-chain rule is only evidenced for 2D and
  cube textures (none exist on the disc).
* Descriptors are validated, not clamped: zero volume depth, excess mips, mips
  on linear images, both/neither storage flags and non-square cubes are errors.
* `P8` (lightmaps) and `P8Bump` carry no palette in the tag; they are shown as
  raw-index luminance (layout verified, not colour).
* Raw pointer selectors Local / MainMenu / Shared / SinglePlayerShared were all
  observed and resolve to sibling maps on the disc.

For lightmap colours, `h2 lighting <map> --bsp N --page N --out DIR`
resolves the `ltmp` palette instead of displaying index luminance. See
[lighting.md](lighting.md) for palette selection and shading limits.

Halo 1 bitmaps decode through the same code: [h1-bitmaps.md](h1-bitmaps.md).
