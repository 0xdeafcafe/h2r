# bitmap-edit

The Textures tool's canvas in `h2-editor`: an RGBA8 image in shared tiles, a brush and eraser, a clone stamp, fill, levels, hue and invert, channel copy, flips and quarter turns, resampling and PNG in and out, with undo and a dirty-rectangle log. The edits are `BitmapOp` values, so the window's tools and the `bitmap.*` agent methods perform the same thing.

## Where it sits

`tools` domain. Depends on no workspace crate, only `serde` and `png`: it knows nothing of Halo formats. The editor (`crates/apps/studio`) decodes a bitmap's images through `assets::decode_image` into `Canvas`es, performs ops, shows the canvas and reads `Bitmap::take_dirty` for the live preview. The save encodes the changed tiles back into the map (`assets::write`).

## Layout

- `src/canvas.rs`: `Canvas`, tiles of 256 pixels as `Arc`s, rectangle read and write, `changed_tiles` by pointer.
- `src/bitmap.rs`: `Bitmap` (layers per image face or slice, undo, stroke in progress, dirty log), `BitmapOp`, `Applied`, `LayerId`.
- `src/brush.rs`: `Brush`, `stamp`, `stamp_with` (a stamp whose colour comes from a sampler: the clone stamp), `Stroke` (spacing carried across calls), `paint`.
- `src/fill.rs`: `fill_rect` and `flood` by tolerance, limited to a selection.
- `src/adjust.rs`: `Adjust` (invert, levels, hsv) over a rectangle and channels.
- `src/transform.rs`: `copy_channel` (a channel or the Rec. 601 luma into other channels) and `turn` (a flip or a quarter turn of a rectangle).
- `src/color.rs`: sRGB and linear light, `Channels`, `blend`.
- `src/resample.rs`: box down, bilinear up, in premultiplied linear light.
- `src/png_io.rs`: `encode_png` and `decode_png`.

## Tests

`cargo xtask test -p bitmap-edit`. All synthetic: stamp, line, clone, fill, channel copy and turn pixels; an undo snapshot sharing the tiles a stroke did not touch; the dirty log; resampling and PNG round trips; ops through JSON.

## Docs

- [editor/modeling.md](../../../docs/editor/modeling.md), "Texture editing"
- [editor/realtime-edits.md](../../../docs/editor/realtime-edits.md), the pixel push the dirty log feeds
