# User-interface tags: screen and list classes

The code classes behind screens and lists, split from [ui.md](ui.md).

| Section |
|---|
| [Screen and list classes (code, not tags)](#screen-and-list-classes-code-not-tags) |

Part of [ui.md](ui.md).

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
3. sets byte `screen + 0x6c` to 1 and pushes the screen with
   `SCREEN_PUSH` (EDI = the screen, EBX = the request, plain `ret`).
   The push calls screen slot 21.

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
(`speaker_row`, `custom_games`). "Add items" links as many of the
embedded items (slot 18) as slot 19 says; the Controls list's slot 19
(`0x2b198a`) says six. `0x24bc12` then lays the children out from
the list's anchor, up to the list block's visible count, each one item
height lower. The height is the item's bounding box (`0x2bafa4`, top minus
bottom) over its children, leaving out bitmaps whose definition has flag
bit 0 set (`0x2b0a48`; shader bitmaps have their own test, `0x2b12ba`, not
read). An item's bitmap covers its point's `y` up to `y` plus its
height (Y up), so the Controls skin's items are 26 units apart (its row
bar), as the game draws them, and the default skin's 22
(docs/game/ui-screens.md "Item spacing"). `0x2bacbc` places an item's
children at a point: for each of the skin's bitmaps (`+0x24`, `0x38`
bytes each) and then each of its shader bitmaps (`+0x2c`, `0x24` bytes
each) it takes the widget's rectangle (`0x22e9c6`), adds the
rectangle's own height (top minus bottom, as a magnitude) to its top
and bottom, adds the point, and writes it back (`+0x20`). A shader
bitmap's rectangle is so moved up by its height like a bitmap's. Its
loop over the skin's text blocks (`+0x1c`, through `0x2baeb1`) is not
read.
Children past the visible count stay linked unless the layout's argument
is set (then `0x24c102` removes them); the pane builder (`0x22f828`)
passes 0. Rows go to every child in order (`0x24c0c4`), so a child past
the visible count gets a row it never shows, and the focus (`0x24c1c5`,
to the next child with a row, `0x24ac76`) and A (`0x24c024`) reach it
([ui-builder.md](../game/ui-builder.md) "Item count").
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
