# assets

Glue from decoded game tags to `gfx` texture decoding. It covers bitmap
(`bitm`) tags: it maps a decoded `tags::Image` to a `gfx::TextureDesc`,
fetches the raw pixel bytes from the map files the image points into, runs
the CPU decode to RGBA8, and writes PNGs. No game data is embedded.

## Where it sits

Domain `platform`; it depends on `gfx`, `map` and `tags`. `viewer`, the
editors, `h1convert`, `h1-assets`, `studio`, `game`, `boot` and `cli` use
it.

## Layout

- `src/bitmap.rs`: `texture_desc`, `decode_image`, `decode_image_blocks`,
  `d3d_format`.
- `src/resource.rs`: `ResourceFiles` and `Region`, the raw-pointer fetch.
- `src/xpr.rs`: XPR resource headers.
- `src/png.rs`: PNG output.

## Tests

`cargo nextest run -p assets`.

## Docs

- [docs/formats/bitmaps.md](../../../docs/formats/bitmaps.md)
- [docs/graphics/graphics.md](../../../docs/graphics/graphics.md)
