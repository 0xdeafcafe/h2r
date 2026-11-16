# Halo 1 Xbox cache files (version 5)

This describes the reader in `crates/h1map`, step 1 of the campaigns work
([campaigns.md](../game/campaigns.md)): Halo: Combat Evolved's Xbox maps,
read so they can later be converted into Halo 2 v8 maps
([map-format.md](map-format.md)). It is not an on-disk Rust struct ABI.
All integers are little-endian. Offsets are hexadecimal. Unless a section
says otherwise, they are offsets into the **decompressed** file: the
`0x800`-byte header followed by the inflated body.

Evidence labels, as in map-format.md:

- **V — verified-on-disc:** checked against the owner's retail Halo 1 disc
  by the ignored test `real_disc_every_map_reads`.
- **S — from-source:** public documentation describes this, and the reader
  relies on it, but it has not yet been checked on the disc.
- **Design:** an implementation choice, not a format fact.

**Status (2026-11-16): every fact below is S.** The disc wasn't available
when the reader was written. Run the check in "Verifying on the disc" and
promote what it confirms to V.

No map bytes, tag dumps, tag-name lists or extracted assets are included
here or in tests. The tests build invented maps and discs in memory
(`h1map::testing`).

## Sources

- The Reclaimers Library (c20), *Maps* page, sections "Map header", "Tag
  index header", "Tag array entry", "Compressed maps" and "Tag space":
  <https://c20.reclaimers.net/h1/maps/> (read 2026-11-16; the wiki has no
  revision IDs to pin).
- c20, *scenario* tag page, the `structure bsps` block's fields:
  <https://c20.reclaimers.net/h1/tags/scenario/>.
- SnowyMouse, *Halo map file structure*, revision 2.1, Open Carnage topic
  6693, credited by c20.
- Invader (<https://github.com/SnowyMouse/invader>) is the reference
  implementation c20 credits. It is GPL: it may be cited, but none of its
  code or tag definition files may be copied here, and none were.

Field offsets inside tags (the scenario's BSP block) are not on c20's tag
page; those are marked S with a note.

Model and animation tag layouts (`mode`, `antr`) are in
[h1-models.md](h1-models.md), checked on real maps.

## Header and compression

**S:** the header is `0x800` bytes and is stored uncompressed. `head`
(bytes `daeh`) at `0`, `foot` (bytes `toof`) at `0x7fc`. For cache version 5
(Xbox) everything after the header is one zlib stream. PC maps (version 7)
and Custom Edition (`0x261`) are not compressed and are refused by this
reader (`Error::Unsupported`).

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `000` | FourCC | `head` | S |
| `004` | u32 | Cache version, 5 | S |
| `008` | u32 | Decompressed file length, header included | S; see below |
| `00c` | u32 | "Padding length", Xbox only; kept as `field_0c` | S, unverified meaning |
| `010` | u32 | Tag data offset in the decompressed file (the tag index header) | S |
| `014` | u32 | Tag data size | S |
| `020` | char[32] | Scenario (map) name, NUL-terminated | S |
| `040` | char[32] | Build, `01.10.12.2276` on Xbox | S |
| `060` | u16 | Cache type: 0 campaign, 1 multiplayer, 2 UI | S |
| `064` | u32 | CRC32 over BSPs, model data and tag data | S |
| `7fc` | FourCC | `foot` | S |

**Design:** the reader inflates the stream and requires exactly
`file_length - 0x800` bytes out of it (one byte more or less is an error),
then keeps header + body as one buffer, so header offsets work unchanged.
Bytes after the end of the zlib stream (sector padding) are ignored. Open
question for the disc check: whether `file_length` counts the header (the
reader assumes it does) and what `0x0c` holds relative to the stored size.

**S:** size limits by type: campaign 278 MiB, multiplayer 47 MiB, UI 35 MiB
(the disk cache partition each one is inflated into). The reader caps
`file_length` at 512 MiB (Design).

## Tag index header

At the header's tag data offset.

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | u32 | Address of the tag array | S |
| `04` | u32 | Scenario tag ID | S |
| `08` | u32 | Checksum | S |
| `0c` | u32 | Tag count (at most 65535) | S |
| `10` | u32 | Model part count | S |
| `14` | u32 | Vertex data **address** (Xbox) | S |
| `18` | u32 | Model part count, repeated | S |
| `1c` | u32 | Triangle (index) data **address** (Xbox) | S |
| `20` | FourCC | `tags` | S |

**S:** on Xbox the model data is inside the tag data, so `14` and `1c` are
addresses; on PC they are file offsets, and a model data size pushes `tags`
to `24`. The reader accepts `tags` at `20` or `24` and keeps the fields
before it as `model_fields` (Design: the PC shape is tolerated, not needed).

## Addresses

**S:** the Xbox engine loads tag data at `0x803a6000`
(`XBOX_METADATA_BASE`): that address is the tag index header. So a tag
data address `a` is at file offset `tag_data_offset + (a - base)`, and must
fall inside the tag data size. The active BSP is loaded at the other end of
the 22 MiB tag space.

**Design:** the reader does not assume the base. The tag array directly
follows the index header, so it derives
`base = tag_array_address - index_header_len` from each file
(`H1Map::metadata_base`). The disc check asserts that this equals
`0x803a6000` for every map.

## Tag array

`tag count` entries of `0x20` bytes at the tag array address.

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | u32 | Group (FourCC as a u32: `scnr` is the bytes `rncs`) | S |
| `04` | u32 | Parent group, or `ffffffff` | S |
| `08` | u32 | Grandparent group, or `ffffffff` | S |
| `0c` | u32 | Tag ID: array index in the low 16 bits, salt above | S |
| `10` | u32 | Address of the tag path (NUL-terminated, no extension) | S |
| `14` | u32 | Address of the tag's data | S |
| `18` | u32 | External flag; padding on Xbox (no resource maps) | S |
| `1c` | u32 | Unused | S |

Group codes shorter than four characters are space-padded (`hmt `).

**Design:** the reader requires each ID's index to be its position in the
array, and the scenario ID to name a tag. A tag's data range runs from its
address to the next tag's data (by address), or to the end of the tag data;
external tags and addresses outside the tag data have no range. The real
size of a tag is its layout's business, which the converter will know.

## Tag blocks and references

**S:** a tag block field is 12 bytes: count, address of element 0, unused.
A tag reference field is 16 bytes: group, path address, path length, tag
ID. `H1Map::block` turns a block and an element size into the elements'
bytes; an empty block reads nothing, whatever its address.

## Structure BSPs

**S:** each BSP lives in its own region of the file, outside the tag data,
and is loaded at the end of tag space when it becomes active. The scenario's
`structure bsps` block holds, per BSP: BSP start (file offset), BSP size,
BSP address (where the region is loaded), and a reference to the `sbsp`
tag. c20 names these fields but gives no offsets.

| Where | Value | Evidence |
|---|---|---|
| Scenario tag | `structure bsps` block at `5a4` | S (community layouts; not on c20's page) |
| Element | `0x20` bytes: `00` start, `04` size, `08` address, `0c` unused, `10` `sbsp` reference | S (same) |

`H1Map::structure_bsps` reads them, checks each reference names an `sbsp`
tag and each region lies inside the file, and `H1Map::bsp_bytes` returns a
region. Reading inside a BSP region (its header, and addresses relative to
the BSP address) is left to the converter.

## The disc

| Fact | Evidence |
|---|---|
| `default.xbe`'s certificate title ID is `4D530004` | S |
| Maps are files directly under `\maps\`, named `<scenario>.map` | S |
| Campaign levels: a10, a30, a50, b30, b40, c10, c20, c40, d20, d40 | S |
| `ui.map` holds the menus; Xbox has no `bitmaps.map`/`sounds.map` | S |

`h1map::disc` checks the title ID and lists the maps. `campaign_maps()`
needs every level and adds `ui`, `bitmaps` and `sounds` when present.

## Commands

- `h2 h1 info <iso|map> [map]`: with a disc and no map, the title and each
  map's (uncompressed) header; with a map, its header, tag index, base,
  structure BSPs and tag counts by group.
- `h2 h1 tags <iso|map> [map] [--group G]`: ID, group, data range and name
  of every tag (or one group's).
- `h2 import halo1 [iso] [--data DIR] [--force] [--dry-run]`: checks the
  title and every campaign map's header, then copies the maps as stored
  (still compressed) into `<data>/campaigns/halo1/source/` and writes
  `manifest.txt` there: `<sha256>  <size>  <name>` per file. `--data`
  defaults to `$TMPDIR/h2boot-data` and the disc to `disc/halo1.iso`.

## Verifying on the disc

```
H1_ISO=/path/to/halo1.iso cargo test -p h1map -- --ignored --nocapture
```

`real_disc_every_map_reads` opens every map on the disc and asserts: the
title ID; every campaign map present; each map inflates to exactly the
header's length; the derived base is `0x803a6000`; the header name matches
the file name; the scenario tag is a `scnr`; every tag name is non-empty
ASCII; every non-UI map has structure BSPs, each naming an `sbsp` tag, with
its region outside the tag data. It prints each map's header fields, base,
tag count, index header length, model fields and BSPs, from which this
page's table rows can be marked V.
