# font

The packed bitmap fonts of Halo 2 (Xbox): a parser, a glyph decoder, an
encoder and a CPU text rasteriser. The layout was read from the retail
binary and checked against every font file on the disc.

## Where it sits

Domain `formats`; no workspace dependencies. `boot` and `game` use it to
draw host text, `hud-edit` and `h1objects` to read and write fonts, and
`cli` to inspect them.

## Layout

- `src/lib.rs`: the file layout, parser and glyph decoder.
- `src/raster.rs`: `render`, `RenderOptions`, `Bitmap`.
- `src/encode.rs`: font writer.
- `src/patch.rs`, `src/mod_marks.rs`: patching a font file and the marks
  that identify one of ours.
- `src/gear.rs`, `src/icons/`: the button and gear icon glyphs.

## Tests

`cargo nextest run -p font`. The tests that read real fonts are `#[ignore]`d
and need `H2_DISC` set to your own disc image
(`cargo test -p font -- --ignored`).

## Docs

- [docs/formats/fonts.md](../../../docs/formats/fonts.md): format and evidence.
- [docs/input/button-icons.md](../../../docs/input/button-icons.md)
