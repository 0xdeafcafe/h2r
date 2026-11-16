# Fonts (`crates/font`, `h2 font`)

Halo 2's fonts are packed files under `/maps/fonts/`, not map tags. `crates/font`
parses them (bounds-checked, safe on hostile input), decodes glyph bitmaps, and
rasterises UTF-16 text to RGBA on the CPU. `h2 font` drives it:

```sh
h2 font <iso> conduit-16 'Halo 2' --out text.png [--color RRGGBB[AA]] [--glyph-colors] [--force]
h2 font <iso> conduit-16 <button-text> --icons keyboard --glyph-colors --out "$TMPDIR/keys.png"
h2 font --all <iso>      # parse + fully validate every font; non-zero exit on failure
```

No glyph sheet, dump or render is stored in the repo. Output PNGs go wherever
`--out` says (the exporter refuses to follow symlinks and will not overwrite
without `--force`, via `crates/cli/src/safe_out.rs`).

## Provenance

Every field below was read from the retail `default.xbe` (disassembly with
`h2 disasm`) and then checked against all 32 font files on the disc. No community
tool or table was consulted or transcribed. Where the disc corroborates a claim
the evidence is named: **E** = the engine code reads it exactly this way,
**D** = holds on every disc font (32/32, every record), **U** = unknown.

## Files

| File | Role |
|---|---|
| `font_table.txt` | 11 lines (CRLF, no final newline): the font file for each engine font slot, in slot order. Duplicates are allowed (lines 2, 6 and 8 are all `conduit-12`). |
| `font_table_{cht,jpn,kor}.txt` | Same 11 lines with the CJK file substituted; line 1 stays `fixedsys-9`, the only Latin file kept. |
| `arial-13/14`, `conduit-12/13/16`, `fixedsys-9`, `handel_gothic-11/13/14/24`, `MSLCD-14` | Latin fonts, 270 to 1234 glyphs. `arial-*` is not named by any font table. |
| `cht_DFT_R5_*`, `jpn_DFGHSMGoth_anti_*`, `kor_SdGD_M_anti_*` | CJK sets of 13,000 to 17,000 glyphs, each in the same sizes as the Latin face it replaces (`con` is `conduit`, `h_g` is `handel_gothic`). They carry Latin glyphs too. |

The font table is parsed by `0x00121fc0` (below): it splits the file on line
ends, and for each name compares against the names already seen (`0x1221c6`
loop), reusing the earlier slot on a match, otherwise loading it once
(`0x1223a0`). The 11 lines are therefore 11 font ids (`0x4e28f4`, 11 dwords)
mapped onto 8 distinct files, held in a 10-entry slot array. Which UI element
uses which id was not traced.

## File layout (little-endian)

```
0x00000  0x200 bytes of zero (the engine seeks past them)
0x00200  header, 0x1c4 bytes
0x00400  character map: 0x10000 x u32, indexed by UTF-16 code unit   (0x40000 bytes)
0x40400  glyph records: glyph_count x 16 bytes
         compressed glyph data, addressed by absolute file offset
```

**E**: the loader (`0x1222d0`) does `SetFilePointer(handle, 0x200)` then reads
`0x1c4` bytes into the font slot. Glyph lookups (`0x140dc0`) read 4 bytes at
`0x400 + 4 * unit`, then (`0x140f50`) 16 bytes at `(index + 0x4040) << 4`
(= `0x40400 + 16 * index`), then the pixel data at the record's offset.

### Header (offsets from 0x200)

| Off | Type | Meaning | Evidence |
|---|---|---|---|
| 0x00 | u32 | magic `0xf0000001` | E (`0x122480`) |
| 0x04 | i16 | ascent | E; line height is `+4 + +6 + +8` (`0x122540`); equals every glyph's `y_offset` on the disc |
| 0x06 | i16 | descent | E |
| 0x08 | i16 | leading | E |
| 0x0a | u16 | 0 | U (always 0) |
| 0x0c | u32 | glyph record count | D: records end exactly where the first glyph's data begins |
| 0x10 | u32 | largest `data_len` of any record | D |
| 0x14 | u32 | 1710, 2100 or 3240 | U |
| 0x18 | u32 | total bytes of glyph data | D: file length = `0x40400 + 16 * count + this` |
| 0x1c | u32 | total `width * height` over all records | D |
| 0x20 | u32 | kerning pair count, at most 96 | E (`0x122570`); 96 is `(0x1a4 - 0x24) / 4` |
| 0x24 | 4 x n | kerning pairs: `u8 first, u8 second, i16 amount` | E |
| 0x1a4 | 8 x u32 | bit set of `first` values (units 0 to 255) that have pairs | E; consistent with the pairs on every disc font |

### Glyph record (16 bytes)

| Off | Type | Meaning | Evidence |
|---|---|---|---|
| 0 | i16 | advance | E |
| 2 | u16 | compressed byte count (engine clamps to 1..=0x1000) | E (`0x140e40`), D (next record's offset minus this) |
| 4 | u16 | bitmap width (clamped to 0..=128) | E |
| 6 | u16 | bitmap height (0..=128) | E |
| 8 | i16 | x offset | E |
| 10 | i16 | y offset | E |
| 12 | u32 | absolute file offset of the pixel stream | E, D |

Record 0 is a blank space on the Latin fonts. A character-map entry of 0 means
"no glyph" and the engine draws record 0 (nothing in the font says otherwise).
63 records in `arial-13` (and some in the other fonts) are placeholders for the engine's
private-use range with width 0, `data_len` 0 and a bogus shared offset; they
have no pixels and are accepted.

### Pixels

**E** (`0x122610`, the decompressor; called from `0x140960` with the glyph's
`data_len`). The output is an ARGB4444 word per pixel, row-major,
`width * height` pixels, with colour 0xfff (white) until a literal changes it.
Each input byte, by its top two bits:

| Byte | Output |
|---|---|
| `00 000000`, then 2 bytes | one literal word, big-endian; its low 12 bits become the colour |
| `00 nnnnnn`, n > 0 | n pixels, current colour, alpha 0 |
| `01 nnnnnn` | n pixels, alpha 15 (n may be 0, emitting nothing) |
| `10 aaa rrr` | one pixel with 3-bit alpha `aaa`, then by `rrr`: 0 none; 1, 2, 3 = 4, 3, 2 more opaque pixels; 4, 5, 6, 7 = 5, 4, 3, 2 transparent pixels |
| `11 aaa bbb` | two pixels with 3-bit alphas `aaa` and `bbb` |

A 3-bit alpha `a` becomes the 4-bit value `(a << 1) | (a & 1)` (7 gives 15).
**D**: every non-empty glyph of all 32 fonts decodes to exactly `width * height`
pixels with no run crossing either end; a wrong run length would break that
count almost at once, so this is the strongest check on the table above.

Literal words appear only in a minority of records. In `conduit-16`, 167
records carry literals with colours other than white; the first characters
mapping to them are U+2610, U+2611 and then `0xe007`, `0xe010` onward (the
private-use icon range the text scanner also special-cases), and in `arial-13`
37 records start at `0xe100`. The list was inspected only from the start, not
exhaustively. This indicates that literals carry per-pixel colour for icon
glyphs (button and checkbox images) while letters are plain white with alpha.
`RenderOptions::glyph_colors` (`--glyph-colors`) draws each pixel in its own
colour; by default text is tinted with one colour.

### Layout rules used by the rasteriser

From the measuring and drawing routines (`0x13ef40`, `0x13f0e0`, `0x13f700`):

* The bitmap's top-left is drawn at `x = pen + kerning + x_offset`,
  `y = baseline - y_offset`; then the pen moves by
  `kerning + x_offset + advance`. Lines are `ascent + descent + leading` apart.
* Kerning (`0x122570`) applies only when both units are 1 to 255 and the
  previous unit is in the header bit set; the amount is added before the glyph.
* The text scanner (`0x13f5a0`) treats `0x0d` as a line break, `0x0a` as
  ignorable, `0x09` as a tab, `0xe405..0xe407` as alignment tokens, `|` as the
  start of an inline markup escape (`0x13fbd0`) and `0xe000..0xe3ff` as the
  private-use icon range.

`render` implements line breaks (`\r`, `\n`, `\r\n`), kerning, per-glyph
offsets and alpha blending. Not implemented: markup escapes, tab stops, alignment
tokens, icons, and the per-draw scale factor the engine
multiplies into every metric (`1.0` here). Control units below 0x20 are skipped.

## Glyph cache texture

How the engine draws text, from the disassembly (**E**):

* At init (`0x1eff0`) it makes one bitmap through `0x1358e0`: 256x256,
  format 9 (A4R4G4B4), flags `0x810` (bit 4 is linear, bit 11 no CPU
  copy), and stores it at `0x4b62ac`. Its D3D texture (`0x1d000`, kept at
  bitmap `+0x50`) is therefore a `LIN_A4R4G4B4` texture of one level.
* A glyph not yet cached (`0x1f5c0`) gets a place by shelf packing
  (`0x4b62a6`/`0x4b62a8` the pen, rows of up to 256), evicting older
  entries from a 512-entry ring (`0x4b62b0`). The glyph is decompressed
  (`0x140960`, the decoder above) into a stack buffer, the texture is
  locked (`0x1d5e0`, `LockRect` at `0x3fdbc0`), a one-texel border around
  the slot is cleared to `0x0fff` (white, alpha 0) and the ARGB4444 words
  are copied in by rows of the lock's pitch.
* Strings are then drawn as quads sampling that texture.

So every glyph on screen is a 1x bitmap in one texture, and the D3D layer
can recognise it: `d3d::glyph` checks the texture's shape and that its
header is the one the bitmap at `0x4b62ac` names. Above 1x the view
redraws those draws with a filter that rebuilds the edges from the alpha
([high-resolution.md](../graphics/high-resolution.md), "Text"). No glyph data is
stored or shipped; it is all the texture the game fills at run time.

## Engine addresses

| Address | What |
|---|---|
| `0x00121fc0` | font init: loads the font table, loads each distinct file once (`0x1223a0`) |
| `0x001222d0` | open file, seek to `0x200`, read the 0x1c4-byte header (slots at `0x4e2920`, 0x1d0 bytes each, 10 of them) |
| `0x00122480` | header magic check |
| `0x001224c0` | slot lookup by font id (waits for the async header read) |
| `0x00122540` | line height: `ascent + descent + leading` |
| `0x00122570` | kerning lookup |
| `0x00122610` | glyph pixel decompressor |
| `0x00140960` | glyph cache fill: run the decompressor on the I/O buffer |
| `0x00140dc0` | cache miss, step 1: read the character-map entry (`0x400 + 4 * unit`) |
| `0x00140e40` | record clamp: size 1..0x1000, width and height 0..0x80 |
| `0x00140f50` | step 2: read the 16-byte record (`(index + 0x4040) << 4`) |
| `0x0013ef40` | glyph advance and word wrap |
| `0x0013f700` | glyph draw (position from `x_offset`, `y_offset`, kerning) |
| `0x0001eff0` | glyph cache texture: 256x256 linear A4R4G4B4 bitmap, kept at `0x4b62ac` |
| `0x0001f5c0` | put a glyph in the cache: pack, decompress, lock, copy |

## Validation

* `cargo test -p font` runs synthetic-font tests: header and record rejection,
  every truncation point, each run kind of the decoder, overrun and underrun,
  layout (kerning, lines, offsets) and size limits.
* `cargo test -p font -- --ignored` with `H2_DISC` set parses and fully validates
  every font on the disc (character map in range, every record decodes to
  `width * height`). `h2 font --all <iso>` prints the same check per file:
  32 fonts, 0 failed.
* Rendered strings were inspected by eye for Latin, Japanese, Korean and
  Traditional Chinese text, with correct glyphs, spacing and kerning.

## Open

* Header `+0x14` (1710, 2100, 3240) and `+0x0a`.
* Which engine font slot serves which UI element, and how the font table picks
  the CJK variant at run time.
* The inline markup escapes. The private-use icon glyphs are listed in
  [button-icons.md](../input/button-icons.md), which also covers `font::encode` (the
  compressor) and `font::patch` (adding glyphs to a font file).
