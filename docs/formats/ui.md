# User-interface tags

This describes the menu UI data model in `crates/tags/src/ui.rs`: the tags that
define screens and widgets, how the retail engine reads them, and what a Rust
renderer needs to draw a screen. Fonts and localized text are separate pieces
of work; this model stops at string IDs and font indices.

No tag dumps, tag-name lists, strings or other disc-derived data are in this
document or in the tests. The tests build invented tags in memory.

## Evidence and provenance

Every interpreted field below was found by reading the retail XBE's UI code
(SHA-256 `03215919…7935d`, see [engine-map.md](../engine/engine-map.md)) and then checked
against the disc. No outside tag definition was consulted for these layouts.
Field names describe what the engine does with a value.

Labels used in the tables:

- **E** — engine-read: an instruction at the cited address reads this offset
  and the stated use follows from the surrounding code.
- **V** — verified on the disc: the structural property holds for every
  instance in all 30 retail maps (see [Validation](#validation)).
- **raw** — preserved but not interpreted. Either no reader was found or its
  effect was not traced. A renderer must not rely on these.

Disassembly came from a linear sweep of the analyzer's function starts with
`iced-x86`, kept in scratch space (`$TMPDIR`). Addresses are guest virtual
addresses. The walkers are listed in [engine-map.md](../engine/engine-map.md#ui-tag-walkers).

## The tag graph

```text
matg +0x110 interface block
  └─ element +0x80/+0x88/+0x90: wgtz per scenario type (main menu / campaign / multiplayer)
wgtz  UI globals
  ├─ +0x00 wigl  shared UI globals
  │     ├─ +0x120 animation collections   (widget animation indices point here)
  │     ├─ +0x138 skins → skin tags      (list skin indices point here)
  │     └─ header fonts, colour and layouts, UI sounds, meter colour
  ├─ +0x08 screens → wgit tags           (searched by screen ID)
  └─ +0x18 unic
wgit  screen
  ├─ +0x18 unic: every string ID in the screen resolves through this tag
  ├─ +0x20 panes
  │     ├─ buttons, lists, text blocks, bitmaps, model scenes, shader bitmaps, tables
  │     └─ lists and tables draw their items from skin tags
  ├─ +0x30 string lists, +0x38 preloaded bitmaps
skin  list skin: text blocks, bitmaps, shader bitmaps, tables, per-item animations
```

`0x001482e8` returns the `wgtz` for the current scenario: it reads the scenario
type at `scnr +0x10` and picks the interface element's reference at `+0x84`
(type 2, main menu), `+0x8c` (type 0) or `+0x94` (type 1). `0x00148098` walks
`wgtz +0x8` and returns the first screen whose `wgit +4` equals the requested
screen ID. Every map on the disc carries exactly one `wigl`. Most carry one
`wgtz`; `shared.map` and `single_player_shared.map` carry two, both naming
that map's single `wigl` at `+0`.

### What a screen, pane and widget are

A **screen** is a `wgit` tag. At run time the engine creates a screen widget
(type 0, constructor `0x0022f5ca`) that keeps the screen ID at `+0x70` and the
current pane index at `+0x5f0`.

A **pane** is one page of a screen. `0x0022f899` returns the current pane
definition and `0x0022fc08` builds its widgets, walking the pane's blocks in a
fixed order: bitmaps, shader bitmaps, tables, text blocks, model scenes.
Buttons and lists are created by per-screen code and read their definitions
by index (`0x00253cc8`, `0x0024bbf3`).

A **widget** is a runtime object with a common header (`0x0022e27b`): type at
`+4`, parent at `+0x10`, first child `+0x14`, next sibling `+0x18`, rectangle
`+0x20`, animation index `+0x68`, render depth bias `+0x6a`, animated offset
`+0x58` and alpha `+0x64`. Widget types:

| Type | Widget | Definition | Constructor |
|---:|---|---|---|
| 0 | screen | `wgit` | `0x0022f5ca` |
| 1 | list | pane list (`0x18`) | `0x0024bb00` |
| 3 | list item / button | pane button (`0x3c`) | `0x00253c8b` |
| 5 | (not traced) | — | `0x002b41bc` |
| 6 | text | text block (`0x2c`) | `0x00253746` |
| 7 | model scene | model scene (`0x4c`) | `0x002b0b5e` |
| 8 | bitmap | bitmap block (`0x38`) | `0x002b01eb` |
| 9 | shader bitmap | shader bitmap (`0x24`) | `0x002b1111` |
| 10 | table | table (`0x18`) | `0x002afd97` |

Draw order: `0x002353a5` collects up to 256 widgets and sorts them with
`0x002356d8` by depth (`+0x60`, the Z of the animated offset, copied by `0x0022e9aa`; values within 0.001 tie),
then by render depth bias, then by widget type, ascending.

### Coordinates

Rectangles are four `i16` values in wire order **top, left, bottom, right**.
The origin is the screen centre and Y points up, so `top >= bottom`. The
bounding-box code at `0x002bafa4` confirms the orientation: it starts from
`top = -32768, bottom = 32767` and keeps the maximum top and minimum bottom.
Full-screen model-scene viewports on the disc are `[240 -320 -240 320]`, a
640×480 frame. Points are `x, y` in the same space.

### Animations and transitions

A widget's animation field is a 1-based index into the `wigl` animation
collections (zero means none). `0x0022e65e` selects a track by transition kind:

| Kind | Track | Direction |
|---:|---|---|
| 0 | `primary` | forwards |
| 1 | `secondary` | backwards |
| 2 | `primary` | backwards |
| 3 | `secondary` | forwards |
| 4 | `ambient` | looping, style from `+0x20` |

Which UI event raises which kind is not traced. A track is a period in
milliseconds and a list of keyframes. The keyframes are evenly spaced over
the period. `0x0022e3cd` interpolates each keyframe's alpha and position
offset linearly; the keyframe's first word is not read. The millisecond unit
follows from the clock at `0x0054d5b8`, which the sprite code divides by
`1000 / fps`.

Shader bitmaps are the exception: their constructor stores the field as is,
without subtracting one. The model keeps that value raw (`animation_raw`).

### Strings

Text blocks, buttons, screen headers and string lists hold `StringId`s. They
are resolved against the screen's own `unic` reference (`wgit +0x18`) by
`0x00230134`, which calls the localized-string lookup `0x001a0180`. This model
validates every string ID against the map's string-ID table (index and encoded
length) but never touches localized text.

## Layouts

Offsets are hexadecimal. Tag references are eight bytes, group FourCC then
datum; a null datum means no tag (some retail null references keep a group).
Blocks are eight bytes, count then virtual address.

### `wgtz` UI globals, `0x20` bytes

| Offset | Field | Evidence |
|---|---|---|
| 00 | shared globals (`wigl`) | E `0x00148350`; V |
| 08 | screens: block of `wgit` references, stride 8 | E `0x00148098`; V |
| 10 | variant-settings tag (`goof`) | E `0x00236a7f`; V |
| 18 | strings (`unic`) | V |

Some screens appear more than once in a screen list (87 duplicate listings
over the disc), so a lookup by screen ID is not a lookup by list position.

### `wgit` screen, root `0x68` bytes

| Offset | Field | Evidence |
|---|---|---|
| 00 | flags: bit 2 hides the header; bits 4, 3, 0 select header layout 1, 2, 3 (else 0); bit 5 copied to the screen widget; bit 1 builds the first pane of a multi-pane screen up front | E `0x0022fda6`, `0x0022f8df` |
| 04 | `i16` screen ID | E `0x00148098` |
| 06 | `i16` | raw |
| 08 | four floats | raw; looks like a colour, no reader found |
| 18 | strings (`unic`) | E `0x00230134`; V |
| 20 | panes, stride `0x4c` | E `0x0022f899`; V |
| 28 | `u16` 1-based shape group index into `wigl +0x128` | E `0x0022facd` |
| 2a | `u16` | raw |
| 2c | header string ID | E `0x0022fda6`; V |
| 30 | string lists, stride `0xc` | E `0x00253c3a`, `0x00230195`; V |
| 38 | preloaded bitmaps: block of `bitm` references, stride 8 | E `0x0022f28c`, `0x0022f672`; V |
| 40 | `0x28` bytes | raw; nonzero in one retail screen only |

Every retail `wigl` has an empty shape-group block, so the 71 screens with a
nonzero `+0x28` index fall outside it and `0x0022facd` skips them.

### Pane, `0x4c` bytes

| Offset | Field | Element | Evidence |
|---|---|---|---|
| 00 | `u16` flags | | raw |
| 02 | animation (copied to the screen widget) | | E `0x0022f71c` |
| 04 | buttons | `0x3c` | E `0x00253cc8`; V |
| 0c | lists (only the first is used) | `0x18` | E `0x0024bbf3`; V |
| 14 | block | — | empty on the whole disc; refused if populated |
| 1c | text blocks | `0x2c` | E `0x0022fc08`; V |
| 24 | bitmaps | `0x38` | E `0x0022fc08`; V |
| 2c | model scenes | `0x4c` | E `0x0022fc08`; V |
| 34 | block | — | empty on the whole disc; refused if populated |
| 3c | shader bitmaps | `0x24` | E `0x0022fc08`; V |
| 44 | tables, one widget per cell (`+0x11`) | `0x18` | E `0x0022fc08`; V |

### Button, `0x3c` bytes

| Offset | Field | Evidence |
|---|---|---|
| 00 | flags: bit 0 → justification 0, else bit 1 → 1, else 2; bit 2 font option | E `0x00253cff` |
| 04 | animation | E |
| 0a | `i16` font index | E |
| 10 | text colour, three floats (r, g, b) | E |
| 1c | rectangle | E |
| 24 | bitmap (`bitm`) | E `0x00253ea8`, `0x00254173`; V |
| 2c | bitmap offset (point) from the rectangle | E `0x00253ea8` |
| 30 | label string ID | E; V |
| 34 | `i16` render depth bias | E |
| 38 | flags: bits 0, 1 copied to the item | E |
| 06, 08, 0c, 36 | — | raw |

### List, `0x18` bytes

| Offset | Field | Evidence |
|---|---|---|
| 00 | flags: bit 0 copied to the list (`+0x7c`) | E `0x0024bc12` |
| 04 | `i16` skin index into `wigl +0x138` | E `0x0014837a`; V (always in range) |
| 06 | `i16` visible items | E |
| 08 | anchor (point) | E |
| 0c | animation | E |
| 0e | `i16` copied to every item (`+0x74`) | E; effect not traced |
| 10 | block | empty on the whole disc; refused if populated |

Items are laid out from the anchor using the skin's bounding box
(`0x0024bc12`, `0x002bafa4`).

### Text block, `0x2c` bytes

| Offset | Field | Evidence |
|---|---|---|
| 00 | flags: bits 0/1 justification as for buttons; bit 2 font option; bit 3 forces font 1 and applies `wigl +0x4c`; bit 4 selects the larger widget allocation | E `0x0022f9bf`, `0x00253765` |
| 04 | animation | E |
| 06 | `i16` copied to the widget (`+0x70`) | E; effect not traced |
| 0a | `i16` font index | E |
| 10 | text colour, three floats | E |
| 1c | rectangle | E |
| 24 | string ID | E `0x00253765`; V |
| 28 | `i16` render depth bias | E |
| 08, 0c, 2a | — | raw |

### Bitmap block, `0x38` bytes

| Offset | Field | Evidence |
|---|---|---|
| 00 | flags: bit 2 makes a meter | E `0x002b0a68` |
| 04 | animation | E `0x002b01eb` |
| 08 | `i16` draw mode: value 1 selects the second draw mode | E `0x002b0565` |
| 0a | `i16` image index, used when in range | E `0x002b01eb`, `0x002b0b19` |
| 0c | top-left (point) | E |
| 10 | texture scroll per second (u, v), wrapped to [0, 1) | E `0x002b0328` |
| 18 | bitmap (`bitm`) | E; V |
| 20 | `i16` render depth bias | E |
| 24 | sprite frames per second; below 0.001 the image is static | E `0x002b0328` |
| 28 | meter extent: right, bottom | E `0x002b0328` |
| 06, 22, 2c–37 | — | raw |

The rectangle is `[y, x, y - height, x + width]` from the selected image's
dimensions (image `+4`, `+6`), recomputed every update. A meter instead has
`bottom` from `+0x2a` and a width that grows from zero to `+0x28 - x` with a
runtime 0..1 value; it is filled with the `wigl +0x50` ARGB colour
(`0x002b07e2`).

### Model scene, `0x4c` bytes

| Offset | Field | Evidence |
|---|---|---|
| 04 | animation | E `0x002b0b5e` |
| 08 | `i16` render depth bias | E |
| 0c | scenario object names, `0x20`-byte strings | E `0x002b0baa` (resolver `0x000bb760`) |
| 14 | second name list, `0x20`-byte strings | E (resolver `0x000c38e0`) |
| 1c | three floats, scaled and subtracted from the camera | E |
| 28 | camera position | E |
| 34 | field of view, clamped to at least 1 | E |
| 38 | viewport rectangle | E |
| 00, 06, 0a, 40–4b | — | raw |

A model scene draws only while `0x0054d598` equals 4, which this survey did
not identify.

### Shader bitmap, `0x24` bytes

| Offset | Field | Evidence |
|---|---|---|
| 04 | animation, stored without the usual minus one | E `0x002b1111` |
| 08 | `i16` render depth bias | E |
| 0a | `i16` passed to the draw call | E `0x002b11c9` |
| 0c | bitmap (`bitm`) | E; V |
| 14 | shader (`shad`) | E `0x002b11c9`; V |
| 1c | rectangle | E |

### Table, `0x18` bytes

| Offset | Field | Evidence |
|---|---|---|
| 04 | skin (`skin`) | E `0x002afdbc`; V |
| 0c | anchor (point) | E |
| 10 | `i8` order: zero walks rows first, nonzero columns first | E |
| 11 | `i8` cells; the pane builder makes one widget per cell | E `0x0022fc08` |
| 12, 13 | `i8` columns, rows | E |
| 14, 16 | `i16` row spacing (Y), column spacing (X) | E |

`0x002afdbc` first clamps `columns` and `rows` to at least 1 (many retail
tables store 0). Cell `i` then has grid position
`(a, b) = (i / columns, i % columns)` when ordered, else `(i % rows, i / rows)`,
and the engine places it only if `0 <= a < rows` and `0 <= b < columns`;
other cells get no position. A placed cell sits at the anchor plus the
parent's offset plus `(column_spacing * a, row_spacing * b)`. The engine does
this in 16-bit arithmetic, which wraps; `Table::cell_origin` uses checked
arithmetic instead and returns `None` for a skipped cell or a result outside
`i16`. `Table::cell_grid` returns just the grid position.

### String list, `0xc` bytes

`+4` is a block of string IDs, stride 4 (E `0x00230195`; V). `+0` is raw.

### `skin` list skin, root `0x3c` bytes

| Offset | Field | Evidence |
|---|---|---|
| 04 | bitmap (`bitm`) | V |
| 0c | two points | raw |
| 14 | per-item animations, stride `0x10`: raw word, then period and keyframes. The list code reads entries 0, 1 and 2 directly, with no bound check (`0x0024ac9b`, `0x0024ad63`, `0x0022e78a` through `0x0024bd3b`) | E; V |
| 1c | text blocks, `0x2c` | E `0x002bacbc`; V |
| 24 | bitmaps, `0x38` | E; V |
| 2c | shader bitmaps, `0x24` | E; V |
| 34 | tables, `0x18` | E; V |

### `wigl` shared UI globals, root `0x1c4` bytes

| Offset | Field | Evidence |
|---|---|---|
| 4c | float applied to flag-8 text blocks | E `0x00253765` |
| 50 | meter fill colour (a, r, g, b) | E `0x002b07e2` |
| 88 | block, element layout not reviewed; populated on the disc | E `0x0023661f` reads it; raw, untrusted |
| 90 | sixteen UI sound references (`snd!`) | E `0x00236299`; V |
| 110 | bitmap (`bitm`) | E `0x00236235`; V |
| 118 | strings (`unic`) | E `0x0023620d`; V |
| 120 | animation collections, `0x2c` | E `0x0022e65e`; V |
| 128 | shape groups, `0x18` (empty on the disc) | E `0x0022facd` |
| 130 | block, no reader found; populated on the disc | raw, untrusted |
| 138 | skins: block of `skin` references, stride 8 | E `0x0014837a`; V |
| 140, 148 | strings (`unic`) | E `0x0022d456` (`+0x148`); V |
| 150 | tag reference (null on the disc) | V |
| 158 | block, layout not reviewed; populated on the disc | E `0x00149ead` reads it; raw, untrusted |
| 160 | four `i16` header fonts, one per layout slot | E `0x0022fda6` |
| 168 | header text colour (a, r, g, b) | E `0x0022fda6` |
| 178 | four header layouts, 16 bytes each: header rectangle, second rectangle | E `0x0022fda6`, `0x0022ff53` |
| 1b8 | looping sound (`lsnd`) | E `0x00235b46`; V |
| 00–4b, 60–87, 1c0 | — | raw |

The three raw blocks are nonempty in all 30 `wigl` tags, so they cannot
use the empty-block guard. They are kept as untrusted `Block` descriptors:
never followed, never bounds-checked, and documented as such on the fields.

### Animation collection, `0x2c` bytes

| Offset | Field | Evidence |
|---|---|---|
| 04 | primary: period (ms), keyframes block | E `0x0022e65e` |
| 10 | secondary: period, keyframes block | E |
| 1c | ambient period | E |
| 20 | `i16` ambient loop style (engine maps 0→0, 1→3, 2→1, 3→0) | E |
| 24 | ambient keyframes block | E |
| 00, 22 | — | raw |

### Keyframe, `0x14` bytes

`+4` alpha, `+8` position offset (x, y, z) (E `0x0022e3cd`); `+0` raw.

### Shape group, `0x18` bytes

`+8` model scenes (`0x4c`) and `+0x10` bitmaps (`0x38`) (E `0x0022facd`);
`+0` raw. Retail `wigl` tags have none.

## Screen and list classes (code, not tags)

How per-screen code is reached, read from the retail XBE for the Controls
screen (screen 39) and checked against the Settings and Button Layout
lists (2026-11-16, static; labels as above, all **E**). This is what a
screen class of ours has to provide; `engine::view::ui` names the offsets
and docs/engine/feature-engine.md "UI step b" uses them.

**Loader.** A screen is opened by `0x149f1e(player mask, loader)`, which
builds a 0x20-byte request and calls the loader (stdcall, the request,
`ret 4`). The Controls loader `0x2b7714`:

1. allocates 0x9b4 bytes with `0x1a47fd` (stdcall, the size, `ret 4`);
2. calls the screen constructor `0x2b7752` with EDI = the screen and the
   request's `+4`, `+8` and `+2` (word) as three stack arguments
   (`ret 0xc`; it returns the screen in EAX). The constructor runs the
   base screen constructor `0x230451` with screen id 39, stores the screen
   vtable `0x45be30`, then the list handler constructor `0x2c84b8` with
   ESI = `screen + 0x614`;
3. sets byte `screen + 0x6c` to 1 and pushes the screen with `0x147f6d`
   (EDI = the screen, EBX = the request), which calls screen slot 21.

**Screen vtable** (`0x45be30`, 28 slots, the next table starts at
`0x45bea0`). Two slots matter to a class of ours:

| Slot | Offset | Controls | Role |
|---:|---:|---|---|
| 18 | `+0x48` | `0x23047e` | set up the current pane (thiscall, one stack argument, `ret 4`): calls `0x22f8df`, which builds the header (`0x22fda6`) and the pane's widgets |
| 26 | `+0x68` | `0x2b7784` | returns the screen's loader (`mov eax, loader; ret`). The screen manager calls it (`0x2352e8`, `0x2354d0`) |

The header is a text widget embedded at `screen + 0x80`. `0x22fda6` sets its
string with `0x253b1a` (EDI = the text widget, the string id on the stack,
`ret 4`) from the tag's header id (`wgit +0x2c`). So a screen's title can
be set after slot 18 has run, by calling `0x253b1a` on `screen + 0x80`;
nothing has to be swapped while the header is built.

**List handler** (at `screen + 0x614`; the Controls constructor `0x2c84b8`):

| Offset | Field |
|---:|---|
| `+0x00` | vtable (`0x45d240` for Controls) |
| `+0x70` | the rows' datum array (made by `0x1a480d` with a name, element size and count; one datum per row, created at once) |
| `+0x84` | the list of press records (`0x22f4cd` links one) |
| `+0x88` | the item widgets, 0x80 bytes each (six for Controls, built by `0x24abae`) |
| `+0x388` | the press record (Controls; `+0x28c` for Button Layout): vtable `0x45bdb0` at `+0`, the handler at `+0x10`, the row-chosen function at `+0x14` |

**List handler vtable** (`0x45d240`, 22 slots; the base list widget's
`0x45a030` has the same first 18 and a pure-call stub `0x32131e` in 18 to 20,
so those three are what a list class supplies):

| Slot | Offset | Controls | Role |
|---:|---:|---|---|
| 0 | `+0x00` | `0x2b7563` | destructor |
| 17 | `+0x44` | `0x24bb60` | add items: links the item widgets as children |
| 18 | `+0x48` | `0x19b516` | the item widgets (`lea eax, [ecx+0x88]`) |
| 19 | `+0x4c` | `0x2b198a` | the item widget count (6) |
| 20 | `+0x50` | `0x2c8548` | row text (thiscall, the item and a word of the list's definition, `ret 8`; the row is the datum handle at `item + 0x70`) |

**Row text** is reached through the vtable on every draw: an item's update
(`0x24abe0`) reads its list at `item + 0x10` and calls slot 20 with the
item and `0x24c0b3`'s value (the `i16` at `+4` of the list's definition). **Row chosen** is not a vtable slot: the press
record's vtable slot 0 (`0x2b27f9`) does `ecx = [record+0x10]; jmp
[record+0x14]`, so the function is the pointer stored in the record
(thiscall, the player chain and a pointer to the row, `ret 8`). Both are
read at each call, never cached: a handler's vtable pointer and its press
record's function can be replaced after the constructor has run, and
before the screen is pushed nothing has drawn or pressed it.

**Item widgets, scrolling and spacing** (static, 2026-11-16; used by the
Controls and Xbox Live menu rows). A list shows one row per child item
widget. When it has more rows than children it scrolls: `0x24c515`
compares the row count with `0x24c177`'s count of the handler's children.
The handler links its embedded items as children in "add items" (slot 17,
`0x24bb60`), so a row can be added without scrolling by linking one more
item after that runs: a zeroed 0x80 bytes, the item constructor
`0x24abae`, the local player (`0x22ee92`), then `0x22eb18`
(`speaker_row`, `custom_games`). `0x24bc12` then lays the children out from
the list's anchor, up to the list block's visible count, each one item
height lower. The height is the item's bounding box (`0x2bafa4`, top minus
bottom) over its children, leaving out bitmaps whose definition has flag
bit 0 set (`0x2b0a48`; shader bitmaps have their own test, `0x2b12ba`, not
read). An item's bitmap covers its point's `y` up to `y` plus its
height (Y up), so the Controls skin's items are 26 units apart (its row
bar), as the game draws them, and the default skin's 22
(docs/game/ui-screens.md "Item spacing").
Anything the tag draws under a list does not move with it, so a screen
whose hint text sits under its list needs its tag adjusted, as
`custom_games::extend_tag` does for the Xbox Live menu.

**Dialogs.** The game's error dialogs take an error id, look it up in
the shared UI globals (`0x23661f`, the `wigl +0x88` block; an unknown id
falls back to error 1) and show the entry's string ids through the
localized-string lookup. `0x19b527(1, error, 4, player mask, on press, on
update)` shows the one-button dialog (screen 8, `ret 0x18`);
`0x19b5af(1, error, 4, mask, on first, on second, on update)` the
two-button one (screen 7, `ret 0x1c`). A null press callback just closes
the dialog. A dialog of ours is an error id the table lacks plus a wrapper
on `0x23661f` that writes our string ids (title `+0xc`, message `+0x10`)
into its entry ([live-custom-games.md](../live/live-custom-games.md) §5).

**A text of our own on a screen** (read for the Skulls screen's
description, 2026-11-16, static, **E**). The pane builder (`0x22fc08`)
makes each text block's widget with `0x22f9bf`: EBX the screen, stdcall
(the block's index, a pointer to the `0x2c`-byte block), `ret 8`. It
allocates `0xf8` or `0x2b8` bytes by block flag bit 4, constructs the text
widget, links it as a child of the screen (`0x22eb18`, EAX the widget, ESI
the screen) and fills it from the block, string id included (`0x253765`);
the widget comes back in EAX (0 when allocation failed). The block need not
be in a tag: a block of ours in arena memory, handed to `0x22f9bf` after
the retail pane set-up, gives the screen one more text widget, whose string
`TEXT_SET` (`0x253b1a`) changes later. Its lifetime is the pane's: a pane
change (`0x230427`) tears the pane's widgets down (`0x22fba9`) before it
builds the next.

**A bitmap of our own on a screen** (read for the Skulls screen's
picture, 2026-11-16, static, **E**). The pane builder's bitmap step
`0x22fa30` takes ESI the screen and a pointer to a `0x38`-byte bitmap
block on the stack (`ret 4`): it allocates `0x90` bytes, constructs the
bitmap widget (`0x2b01eb`), sets `+0x6c` and links it to the screen
(`0x22eb18`); the widget comes back in EAX. A copy of a retail screen's
block, found through `0x148098(screen id)` (stdcall, the `wgit` tag's
datum handle or -1) and the tag entries at `[0x4e3b44]`, keeps that map's
own `bitm` reference. The bitmap draw (`0x2b0565`) makes its colour from
the widget's alpha (`+0x64`) and three floats at `+0x28`, `+0x2c` and
`+0x30` that the widget header (`0x22e27b`) sets to 1, so writing them
tints the image.

**The focused row.** `0x24c447` (EAX the list, plain `ret`, ECX kept)
walks the list's children (`0x24bae6`, first child at `+0x14`, next at
`+0x18`) to the first item for which `0x22ed7a` holds, and returns that
item's row handle (`+0x70`), or -1. The base list code calls it when the
focus moves and, for a list whose definition flag bit 0 is set (copied to
`list + 0x7e`; the difficulty list's is, the Controls list's is not), hands
the row to the pane change `0x230427`; that is how the difficulty screen
changes its description pane per row.

## Aliased blocks, budget and nesting

Block addresses can point into another tag's payload. Two retail screens use
another screen's pane array. Across the disc, 4,407 nonempty blocks lie
outside the tag being decoded: 1,267 under screens, 3,137 under skins and 3
under shared globals. The 3 under shared globals are animation keyframe
blocks, and they lie in a tag outside the four UI groups. `MapSource` resolves a block against
whichever ordinary-metadata tag holds the whole range. `Stats` counts these
as `aliased_blocks`, and those in a non-UI tag also as `foreign_aliases`. The
target group is counted, not enforced, because retail data does it. BSP and
lightmap regions are excluded because their address spaces overlap ordinary
metadata.

Payload spans may nest or overlap. A virtual address maps to one file
offset, so every span holding a range has the same bytes for it. A read
uses the containing span with the greatest start, found by walking back over
a running maximum of span ends. The retail disc has no overlapping spans.

Because aliased blocks can be followed many times, every decode has a
budget. `Source::budget()` is the number of metadata bytes the source holds
(for `MapSource`, the sum of ordinary payload sizes). One decode may charge
at most that many bytes and `budget / 4` elements, 4 being the smallest
stride. The root and every followed block are charged each time they are
followed, before they are read. Going over is `Error::Budget`, wrapped in the
usual field context. Element views borrow from one shared, reference-counted
copy of each payload (`ui::Bytes`), so following an alias allocates only
the decoded records. The largest retail decode charges 13,536 bytes against
a budget of about 1.4 MB.

Lists and tables draw from skins, and a skin's tables name further skins.
`ui::resolve_skin` decodes a skin and the skins its tables reach, sharing one
budget across the whole tree. It refuses anything deeper than
`ui::MAX_SKIN_DEPTH` (4), counting the root skin as depth 1. Retail data
nests at most 2 deep. No engine limit was found, so this bound is ours, and
it also stops a reference cycle.

## Using it

```rust
use tags::ui::{self, MapSource, UiTag};

let mut map = map::Map::open("/path/to/your/example.map")?;
let id = map.tag(map::TagQuery::NameInGroup("ui\\screens\\example_screen", ui::SCREEN))?.id;
// Build the source once per map; it indexes payload spans and caches reads.
let mut src = MapSource::new(&mut map);
if let UiTag::Screen(screen) = src.decode_tag(id)?.tag {
    for pane in &screen.panes {
        for b in &pane.bitmaps { /* b.rect(image_size), b.bitmap, b.animation */ }
    }
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

`ui::decode(source, group, address)` takes any `ui::Source`, which is how the
tests feed synthetic tags. Decoding is strict: block bounds and alignment,
tag-reference classes, string IDs, the budget and the three unreviewed
pane/list blocks are all checked, and errors carry the record and field path.
`ui::read(map, id)` is a one-off convenience that builds a fresh `MapSource`.
`SharedGlobals::animation`, `skin` and `shape_group` turn widget indices into
entries, returning `None` when an index is out of range.

Print a screen's widget tree:

```sh
cargo h2 ui example 'ui\screens\example_shell\example_screen' --disc disc/halo2.iso
```

`h2 ui <map> <screen tag> [--disc disc.iso]` prints widget kinds, rectangles
as `[top left bottom right]`, bitmap tag paths with the image used, animation
indices, render depth bias, scroll rates and string IDs with their string-ID
names. List and table entries show their skin's widgets relative to the item
origin, nested table skins indented below them. Each skin is decoded once
per run. The command refuses a map without exactly one `wigl`, and a `wigl`
that fails to decode is reported as such. It never prints localized text.
Keep redirected output in `$TMPDIR`.

`h2 ui --all <map|all> [--disc disc.iso]` runs the map check below on one map
(a disc map name or a local `.map` file) or on every map on the disc, and
prints counts. It exits non-zero on any failure.

## Validation

```sh
cargo test -p tags -p cli
cargo run -p tags --example verify_ui -- disc/halo2.iso
target/release/h2 ui --all all --disc disc/halo2.iso
```

Both disc commands run `ui::check::check_map` on every map. It decodes every
`wgit`, `wgtz`, `wigl` and `skin` tag in strict mode and resolves every skin
tree. It also checks what the engine indexes without bounds:

- the map holds exactly one `wigl`, and every `wgtz` names it;
- widget animation indices fall inside its animation collections;
- shader-bitmap animation values are at most their count;
- list skin indices fall inside its skin list;
- every skin a list uses has at least 3 item animations;
- every table cell the engine places has a 16-bit origin.

A map without UI tags fails. The check also counts aliased and foreign
blocks, skipped table cells, out-of-range shape groups (which the engine
ignores), duplicate screen listings and overlapping spans.

On the retail disc: 30 maps, all with UI tags; 1,220 screens, 32 UI globals,
30 shared globals and 904 skins; 36,610 followed blocks, 16,631 of them
nonempty, and 20,254 tag references. There are 87 duplicate screen listings,
71 ignored shape-group indices and no overlapping spans. Skin nesting reaches
depth 2.

The last whole-disc run of the current checks found two kinds of problem, and
both are fixed:

- **Table cells.** The first cell-origin check rejected tables that store
  zero rows or columns. The engine clamps those to 1, and `cell_grid` now
  does the same.
- **Foreign aliases.** The keyframe blocks that alias into a non-UI tag were
  rejected. They are now counted instead.

The whole-disc rerun of the final code (`h2 ui --all all` and `verify_ui`)
reports **0 failures**: no skipped table cells, 215 nonzero shader-bitmap
animation values (all in range), and a largest decode of 13,536 bytes against
a 1,368,656-byte budget. `h2 ui` prints every one of mainmenu's 84 screens
without an error.

## Open questions

- Which UI events raise which transition kind (0–4), and the meaning of the
  ambient loop styles.
- Widget type 5, and the second widget type that `wgit` flag bit 4 selects for
  text blocks (`0xf8` versus `0x2b8` bytes).
- The draw modes behind bitmap `+8`, justification values 0–2 and the font
  options in text and button flags. The font work owns the last two.
- The raw fields listed in each table, the populated `wigl +0x88`, `+0x130`
  and `+0x158` blocks, and the screen floats at `+8`.
- Whether shader-bitmap `+4` is 0- or 1-based. The check accepts either.
- The trigger `0x0054d598 == 4` for model scenes.
- Item placement inside a list beyond the anchor and skin bounding box.
