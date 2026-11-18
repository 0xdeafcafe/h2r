# Halo 1 text: `ustr`, `hmt `, and their conversion to Halo 2

Halo 1 keeps its user-facing text in two tag groups, read here by
`crates/h1text` through the [`h1map`](map-format.md) reader, and
converted into Halo 2 localized text ([localization.md](../formats/localization.md)):
string IDs, locale rows in the `matg` globals, and one `unic` tag per
Halo 1 list. This page covers both sides.

Evidence labels, as in map-format.md:

- **V — verified-on-disc:** checked against the owner's retail Halo 1 disc
  by the ignored tests `real_disc_every_string_list_decodes` (every
  `ustr` and `hmt ` on every map: 10 campaign levels, 13 multiplayer
  maps and `ui.map`, `$H1_ISO`) and
  `real_disc_a30_converts_and_reads_back_through_halo_2`.
- **S — unverified:** not yet independently checked here.
- **Design:** an implementation choice.

No game text, string dumps or community definition files are included in
the crate or its tests; the tests build invented string lists in memory.

## `ustr`: unicode string list

**V** (all 418 tags on the disc, 5,397 strings): the tag is a block
(count, address, unused) of 20-byte elements — the same data-reference
shape the `sbsp` material layout uses — followed by the strings, packed
in element order.

Element (`0x14` bytes):

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | u32 | Byte size of the string, its NUL included; always even, at least 2 | V |
| `04` | u32 | Flags; 0 everywhere on the disc | V |
| `08` | u32 | Offset from the tag's data address, **unpadded** (below) | V |
| `0c` | u32 | Tag-data address of the string's bytes | V |
| `10` | u32 | Unused; 0 everywhere on the disc | V |

The strings are little-endian UTF-16, `size/2 − 1` code units and a NUL.

**V**, the offset field at `08`: it is the sum of the earlier elements'
sizes from the tag start, without padding. Campaign maps store the
strings unpadded, so it equals the address there (a30: every element);
multiplayer and `ui.map` dword-pad the string data, so the address leads
it by 2 per padded string (disc-wide 1,262 elements match exactly, 4,135
lead; e.g. `ui\hud\hud_item_messages` deltas 0, 2, 4, 4, 4, 6…). The
engine uses the address; the reader does too.

## `hmt `: HUD message text

**V** (all 34 tags on the disc, 400 messages): one UTF-16 string blob,
one block of 2-byte elements, one block of named messages. The tag opens
with a 20-byte data reference to the blob (same shape as a `ustr`
element), then two blocks:

| Offset | Field | Evidence |
|---|---|---|
| `00` | Data reference (size, flags, offset-from-tag-start, address, unused) to the UTF-16 blob | V |
| `14` | Block of elements, 2 bytes each | V |
| `20` | Block of messages, `0x40` bytes each | V |
| `28`..`80` | Zero padding (the blob starts at the tag's `+0x80` on every map on the disc) | V |

Element (`2` bytes): `type` (u8), `value` (u8).

- Type 0, **text** (**V**): the value is a length in UTF-16 units, NUL
  included. The lengths of all type-0 elements in tag order sum to
  exactly the blob (checked per tag on every map): the messages consume
  the blob sequentially, in element order.
- Type 1, **glyph** (**V** for the shape, S for the meaning): a
  substituted glyph — a controller button or keyword — by index (the
  shared `ui\hud\hud messages` uses 18, 32, 33, 24…; a30's level
  messages use one glyph, 26). Counts vary per tag, and no type other
  than 0 and 1 occurs on the disc; the blob sums leave no bytes for
  glyphs, so they carry no text of their own.

Message (`0x40` bytes):

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | char[32] | Message name, NUL-padded, ASCII on the disc | V |
| `20` | u16 | Unresolved: observed values (0, 4, 12, 70…) match no layout here | V for the bytes, meaning open |
| `22` | u16 | First element of the message | V |
| `24` | u32 | Element count | V |
| `28`..`40` | — | Zero on the disc | V |

**V:** the messages tile the element block in order — message `i` starts
where message `i−1` ends, at element 0, and together they cover every
element. A message's text is its parts in order: text runs and glyphs
(`HudMessage::plain_text` drops the glyphs).

Example shape (invented text): a message `tutorial_x` takes elements 0–2
(text "Press ", glyph 26, text " to do X"), and the next message takes
element 3, a single text run.

## The scenario's text references

**V**, every campaign map has these three fields; `ui.map`'s scenario
has none of them set:

| Offset | Field | Evidence |
|---|---|---|
| `574` | `ustr`: names scripts give placed objects (`custom_object_names`); null on a10, c20, d20 | V |
| `584` | `ustr`: in-game help text (`player_help_text_<level>` / `ingame help text`) | V |
| `594` | `hmt `: the level's HUD messages | V |

A non-null reference that names a tag of the wrong group is refused.

## Conversion to Halo 2

Halo 2 localized text is one global structure per map
([localization.md](../formats/localization.md)): per language, an index of
`(packed string ID, blob offset)` rows over a NUL-terminated UTF-8-like
blob, described by a descriptor in the `matg` at `+0x188`; a `unic` tag
is `0x34` bytes whose nine `(start, count)` ranges select rows in its
language's index. Halo 1 has only English, and addresses its lists by
tag and index rather than by ID.

`h1text::convert::Locales` builds the Halo 2 side, following the
`Converted` pattern ([models.md](models.md)):

- `Locales::add_ustr` mints one string ID per string, named
  `<Halo 1 tag path>\<index>`, and interns it into the `TagSet` so other
  tags can name the same text; `Locales::add_hmt` names each message
  `<Halo 1 tag path>\<message name>` and drops its glyphs (Halo 2 has
  its own button glyphs; the mapping is a later problem). Rows append to
  one English table: `(key, blob offset)` and the UTF-8 bytes plus NUL.
- Each list returns a `Converted` `unic`: 16 zero build bytes, the
  English range `[start, start + n)`, eight empty ranges. `install`
  places it like any other tag.
- The blob is UTF-8, and the engine's converter narrows four-byte
  sequences to their low 16 bits, so text outside the Basic Multilingual
  Plane would not read back; the converter refuses it (none occurs on
  the disc). Halo 1's `»`, `\r\n` and private-use characters are
  BMP-sized and pass through byte-identical.
- `Locales::patch_matg` writes the English descriptor (row count, blob
  size, index and blob file offsets) into a `matg` block. The offsets
  are plain file offsets of the two raw resources, so the set is built
  once to learn them (`BuildLayout::raw`), the descriptor is patched,
  and the map is built again: only field values changed, so the layout —
  and the raw offsets — stay put (asserted in the tests).

Verified end to end by the ignored disc test: a30's 21 string lists (19
`ustr`, 2 `hmt `) become 21 `unic` tags over 239 rows; every row reads
back through `tags::Localization` byte-identical, and synthetic tests
check the empty string, non-ASCII text, range isolation between two
`unic` tags, and the malformed inputs the reader refuses.

## Commands

None yet: the reader and converter are library code, exercised by the
tests below. (The `h2 h1 tags` command lists `ustr`/`hmt ` tags like any
other group.)

## Verifying on the disc

```
H1_ISO=/path/to/halo1.iso cargo test -p h1text -- --ignored --nocapture
```

`real_disc_every_string_list_decodes` prints, per map, the `ustr`/`hmt `
counts, string and message totals, and glyph counts; disc-wide:
**418 `ustr` tags, 5,397 strings; 34 `hmt ` tags, 400 messages; 0
failures.** `real_disc_a30_converts_and_reads_back_through_halo_2`
prints the unic/row counts and the strings-checked count (239).
