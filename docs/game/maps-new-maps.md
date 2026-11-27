# Maps: new maps and the Map Pack

New maps and the Map Pack. Part of [maps.md](maps.md).

## 4. New maps and the Map Pack

1.0 builds its multiplayer map list from the disc's maps plus each
installed content package's `patch.lvl` (version 2). The map pack's own
`patch.lvl` only names Lockout; its new maps are listed in `patch_v3.lvl`,
which only the title update reads ([map-pack.md](map-pack.md)).

So the route for both **new maps** and the **Map Pack**: a content package
under `T:\$c` per map, with a `patch.lvl` listing it. The Map Pack is
installed this way by `h2 import-mappack` ([map-pack.md](map-pack.md) §5,
§8). New maps are below.

### Package offers

Every installed package that fits has its own offer. The boot writes
the offers before the game scans `T:\$c`. The command
`h2 maps install [--data DIR] [--disc disc/halo2.iso]` stages the same
records for the next boot.

| File under `T:\$c\<offer>` | Purpose |
|---|---|
| `<map>.map` | A zero-byte placeholder for the path whose bytes come from the VFS |
| `patch.lvl` | The version 2 list record containing the package's texts and preview |
| `ContentMeta.xbx` | The signed content metadata |
| `h2recomp-map.toml` | The source package, map name and content ID |

A mod on a retail base uses a short alias containing its list ID and
part of its content hash. Standalone maps keep their name where unique.
Aliases identify the exact package to Live. The match report uses the
retail base name for grouping and carries the package identity in
`map_id`. [Map identity in telemetry](../live/live-telemetry.md)
Generated offers are cleaned up when their package disappears or
exceeds the slot budget, and a materialised copy in an offer is replaced by
a placeholder. The source package keeps its layers and image.
A whole-map package stores the full source map, with no second full copy
in the content offer. The Map Pack's original files stay in place.

#### The record's fields

Layout in [map-pack.md](map-pack.md) 8.1. The manifest gains an optional
`[list]` table, which is display data and not part of the `MapId`:

```toml
[list]
base = "lockout"       # a retail map: its own list entry gives the rest
sort = 300             # optional
name.ja = "..."        # optional, per language: en ja de fr es it ko zh pt
description.ja = "..." # optional
```

| field | source |
|---|---|
| type | 1 |
| scenario path | `scenarios\multi\<map>\<map>`. 1.0 uses only its basename, and opens `<content dir>\<map>.map` |
| map id | below |
| `+0x110` | `-1`, as in every shipped record; 1.0 never reads it |
| names (9 x 32 UTF-16 units) | `[list] name.<lang>`, else `[package] name`, in every language; cut to 31 units at a character boundary |
| descriptions (9 x 128) | `[list] description.<lang>`, else `[package] description`, else empty |
| sort order | `[list] sort`, else 1000: after every retail map (10 to 120 on the user's disc) and Map Pack map (130 to 210) |
| flags, team counts | The `[list] base` entry, the package's retail base entry, or the first disc entry by sort order |
| preview | the package image; else the base's own picture; else a plain card |

The language order is the game's: on the user's disc only index 7 holds
Han characters on the retail and Map Pack entries (Chinese), which fits
en, ja, de, fr, es, it, ko, zh, pt (the Japanese and Korean entries use the
Latin names). That is the order `ContentMeta.xbx` names use too.

**The preview** is 207 rows of 220 pixels, top row first, 32 bits each in
the D3D `A8R8G8B8` linear layout (bytes B, G, R, A): 1.0 copies it row by
row into the slot's bitmap (`0x370` bytes a row), and rendering a Map Pack
record that way gives the right picture. Every shipped record has the same
alpha: a one-pixel opaque frame, `0xbf` inside, and a 16-pixel transparent
triangle cut from the bottom-right corner, its edge opaque.
`map_list::preview_alpha` draws that shape and the ignored test below
checks it against all nine Map Pack records. A PNG or JPEG (`png`,
`zune-jpeg`) is scaled to cover 220x207 (bilinear) and cropped to its
centre. The frame's colour is in a retail preview's pixels too (the pale
border Select Map shows), so a preview made from an image or a plain card
takes the colours of the list's first retail map's preview on that
one-pixel frame (`map_list::with_frame`), read from the user's disc at
install. The base's picture is the level element's bitmap (tag reference at
`+0x4`, datum `+0x8`), decoded by `assets::decode_image` with
`mainmenu.map` and `shared.map` as its resource files, then given the same
treatment. The plain card is a vertical gradient in a colour from the
`MapId`.

#### Map ids and offer IDs

- **Map ids** are `0x10000 + (the MapId's first four bytes, little-endian)
  mod 0x7ffe_ffff`, so in `0x0001_0000..=0x7fff_fffe`. Retail ids on the
  user's disc are `0xa` to `0x320`, the Map Pack's `0x3e8` to `0x516`, so
  the range is clear of both (asserted by the ignored test). On a collision
  with a retail id from the disc's own list or another new map, the next
  free id up is taken, wrapping in the range; choices are made in `MapId`
  order, so they don't depend on directory order. 1.0 compares ids for
  equality only (map-pack.md 8.3). The 50 slots include 12 on disc, so
  38 are free before extra content and 29 after the nine-map Map Pack.
  Content beyond the budget is reported in the Mods category. Editing
  map bytes typically changes its list ID and offer. Display changes
  keep both.
- **Offer IDs**: Halo 2's title ID in the high half, `0x8000_0000 | map id`
  in the low half (`4d530064800NNNNN`-style). The Map Pack uses
  `0x0101..0x03ff` and the disc packages `1..3`, so the top bit keeps them
  apart.

#### ContentMeta.xbx

Synthesised in the shape of the Map Pack's (map-pack.md §7): `XCSF`,
header size `0x88`, `+0x1c` = 1, title ID, the offer ID at `+0x28`, `+0x30`
= 1, the names (UTF-16 INI with a byte order mark: `[Default]`, then `[EN]`
`[JA]` `[DE]` `[FR]` `[ES]` `[IT]` `[KO]` `[TW]`, each `Name=`) at `0x88`
with their size and SHA-1, and the file table 32-byte aligned after them
with its size and SHA-1. The table lists no files (count 0): 1.0 never
hashes a package's files (map-pack.md 8.4); XAPI checks the table against
its digest, which holds. When the Map Pack is installed, the header's
words at `+0x1c`, `+0x30` and `+0x6c..0x88` come from one of its
`ContentMeta.xbx` instead. Then `kernel::content::sign` sets the installed
flag and signs it for this console. Whether anything in 1.0
minds an empty file table is not verified. The Map Pack's packages carry a table that does
not match their files, which the import leaves as it is
([map-pack.md](map-pack.md) 8.4).

`ContentImage.xbx` (an XPR icon for the dashboard) isn't made: 1.0's
list path reads only `ContentMeta.xbx` and `patch.lvl`.

#### Tests

- `content` `map_list::tests`: a record built from a manifest passes 1.0's
  check (version, CRC, type, path) and reads back field for field; names
  and descriptions per language with fallbacks and cutting; `[list]`
  validation and that it isn't in the `MapId`; the alpha shape; PNG
  decoding, cover scaling, cropping and BGRA order; the preview fallback
  (image, base, card); ids in range, independent of order, around
  collisions and wrapping; offers apart from the Map Pack's; base entries
  read from a level element.
- `boot` `new_maps::tests` covers legacy installs, simultaneous VFS reads
  of the retail and mod paths, slot allocation, overflow, signatures,
  image fallback and converted-map sandboxes.
- `boot` `real_disc_base_entry_and_record` (ignored; reads the user's disc
  and, if beside it, `mappack.iso`; writes nothing): 50 level elements, 12
  used, all retail ids below the new range; Lockout's element decodes to a
  preview; a record for a derived map with Lockout as its base passes 1.0's
  check with Lockout's flags and team counts; every Map Pack record's alpha
  is `preview_alpha`'s. Passes on the user's disc.
  `H2_DISC=disc/halo2.iso cargo test -p boot --lib real_disc -- --ignored --nocapture`

### The map select by category

With more than one kind of map installed, Change Map opens the categories
first (Halo 2, Map Pack, Mods, Halo: CE). A map's category is read from
its list path: [map-select.md](map-select.md).
