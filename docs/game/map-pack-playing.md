# Map pack: playing on 1.0

How the maps play on 1.0, why the game said the content was damaged, and playing the map pack on 1.0. Part of [map-pack.md](map-pack.md).

## 7. Why the game said the content was damaged

The game lists a package only if its `ContentMeta.xbx` has the `XCSF`
magic and a valid HMAC-SHA1 signature for the console, and it treats the
package as usable only with flags bit 0 (installed) set. The disc's files
are unsigned with the flag clear, so the disc's installer signs each one
for the console it installs on. `kernel::content::sign` does the same for
`h2 import-mappack` and the automatic install, with the console key
`kernel::crypto::XBOX_HD_KEY` (all zero: there is no EEPROM, and no real
console's key is in the repo) that the kernel also writes into the
`XboxHDKey` export, so the guest and the importer use one key. The
details below were read from Halo 2 1.0's XAPI and XONLINE code with
`h2 disasm` and `h2 analyze` (**E**). No public description of the layout
was found, so the field names are ours. Public references for the pieces:
the kernel exports `XcHMAC`, `XboxHDKey`, `XboxSignatureKey` and
`XboxAlternateSignatureKeys` in the [xboxdevwiki kernel export
list](https://xboxdevwiki.net/Kernel); the per-console hard disk key in the
[EEPROM](https://xboxdevwiki.net/EEPROM) page; HMAC in RFC 2104, with the
test vectors in RFC 2202.

### The header

| offset | size | field |
|---|---|---|
| `0x00` | 20 | HMAC-SHA1 signature |
| `0x14` | 4 | magic `XCSF` |
| `0x18` | 4 | header size, `0x88` in all three packages |
| `0x20` | 4 | flags. Bit 0 is the "installed" flag |
| `0x24` | 4 | title ID |
| `0x28` | 8 | offer ID |
| `0x34`... | | offset, size and SHA-1 of the localized names, then of the file table (which holds a SHA-1 of every file) |

The SHA-1 digests are plain hashes, and on the disc they are already right.
Only the signature and the flag are left for the installer.

### The check

- **Halo 2's content list** (`0x19c120`, which calls `0x8d5a0` and
  `0x8d620`, which call `0x33080a`/`0x33095a`, which call `0x330659`) opens
  `T:\$c\<16-digit offer ID>\ContentMeta.xbx` and reads 0x88 bytes. It
  checks the `XCSF` magic, takes the offer ID, and treats the package as
  usable only if **flags bit 0** is set. A package with the flag clear is
  still listed, but as unusable. An unsigned package therefore shows
  as damaged.
- **XAPI's readers** (`0x2d2f6d` and `0x2d0f25`) read the header through
  `0x2d2a1d` with verification on, then require the same flag bit:
  - `0x2d2a1d` reads 0x1c bytes, then the rest up to `min(header size,
    0x88)`, and checks the magic. It calls `0x2d2938` and compares that
    digest with bytes `0..20`.
  - `0x2d2938` makes the per-title key with `0x2d2919`, which is
    `XcHMAC(XboxHDKey, 16, &title ID, 4, NULL, 0)`. The title is the
    caller's, or the header's own when that is 0. Then it computes
    HMAC-SHA1 with that 20-byte key over header bytes `0x14..header size`.
    XAPI's own HMAC (`0x2d6459`, `0x2d64bb`, `0x2d64c0`) is RFC 2104 built
    on `XcSHAInit/Update/Final`, with pads `0x36`/`0x5c`.
  - After that, `0x2d2afe` checks the file table against its digest in the
    header.
- **XAPI's installer** (`0x2d2ce1`) is the other half. It copies a
  `ContentMeta.xbx`, writes the title ID, sets or clears flag bit 0 as its
  caller asks, and writes a new signature with the same `0x2d2938`. A disc
  installer does exactly this.

`XboxSignatureKey` and `XboxAlternateSignatureKeys` are not part of this
check. They key `XCalculateSignatureBegin` (`0x2d65a7`), which signs saves.
The content key comes from `XboxHDKey`, the per-console key. That is why
the signature can't ship on the disc: it would differ on every console.
This is the "non-roamable" signature.

### Our signing

- `kernel::crypto::XBOX_HD_KEY` is this console's hard disk key (above).
- The kernel implements `XcHMAC`: HMAC-SHA1 over both input spans. A key
  longer than 64 bytes stops, since nothing here needs that case.
- `kernel::content::sign` sets flags bit 0 and writes
  `HMAC-SHA1(HMAC-SHA1(XBOX_HD_KEY, title ID), bytes 0x14..header size)`
  into bytes `0..20`. Everything else in a package is copied byte for
  byte.

The guest's check is not patched, and no original console signature is
faked: none ships on the disc.

### Tests

- `kernel` `crypto::tests::rfc_2202_hmac_sha1_vectors`, and `misc`
  `xc_hmac_macs_both_guest_spans`: `XcHMAC` through the guest ABI, split
  inputs, a NULL second input, a stop for a long key, and faults before any
  write.
- `kernel` `misc` `xbox_hd_key_export_holds_the_console_key`.
- `kernel` `misc` `signed_content_meta_passes_the_guest_signature_check`:
  a synthetic header signed by `content::sign` is checked the way the
  guest checks it. It reads the key from the DATA export, derives the title
  key with `XcHMAC`, and runs the HMAC on `XcSHA*`. The disc's unsigned
  form fails. So do another title, a changed header byte, and a good
  signature without the flag.
- `boot` `content_sign` (ignored, needs `$H2_XBE`): runs the game's own
  `0x2d2938` on the fallback interpreter, with the kernel HLE behind it, on
  a synthetic header. It computes exactly the signature the import writes,
  and rejects the unsigned header.
  `H2_XBE=disc/default.xbe cargo test -p boot --test content_sign -- --ignored`

## 8. Playing the map pack on 1.0

Retail 1.0 lists and loads the nine maps without the title update, given
a suitable `patch.lvl` per package. This section is a static reading of
1.0's executable, the update (`default.xbe.03`, certificate version
`0x303`) and the user's files.

Labels: **E** read from an XBE, **V** measured on the user's files,
**S** speculation.

1.0 has the whole mechanism: it reads `patch.lvl` from every package,
fills free slots in the multiplayer map list, and loads maps from `t:\`.
A `patch.lvl` made from a `patch_v3.lvl` record is a pure field mapping.
1.0 takes **one map per package**, so nine maps need nine packages. Nothing
found in 1.0's path refuses the DLC maps, and the update's loader is the
same code; the update adds checks, but they only make it stricter than
1.0.

### 8.1 Where 1.0 reads `patch.lvl`

- `0x19c190` (E) refreshes the UI level lists and calls `0x19c120`.
- `0x19c120` (E) runs only when the loaded cache is the main menu (cache
  type word `== 2`). It walks the installed packages with `0x8d5a0` and
  `0x8d620`, which wrap `XFindFirstContent`/`XFindNextContent` (`0x33080a`,
  `0x33095a`) on the root `t:\`, flag filter 0. Each result's
  `szContentDirectory` (`t:\$C\<offer ID>`) is copied out by `0x8d550` and
  passed to `0x19bfd0`. If any package fails, it sets the byte at `0x54e7f8`.
- `0x19bfd0` (E) opens `<content dir>\patch.lvl` and reads exactly
  `0x2d3fc` bytes (it fails if fewer come back). It never reads further, so
  **a package contributes one record**. Then it calls `0x19be00`.
- `0x19bdc0` (E) validates the buffer: `+0x0 == 2`, and a CRC at `+0x4`.
  The CRC is `0x163ba0`: reflected CRC-32, polynomial `0xedb88320`, initial
  value `0xffffffff`, no final XOR, over all `0x2d3fc` bytes with `+0x4`
  zeroed. The disc's `patch.lvl` passes (V).
- `0x19be00` (E) requires `+0x8 == 1`, then looks up the slot with
  `0x19c5f0(map id)`. If no slot has that id, it takes `0x19c5f0(-1)`, the
  first free slot. It fails if neither exists. The slot is cleared (its
  bitmap reference at `+0x4`/`+0x8` is kept) and filled from the record.
  The preview pixels are copied into that slot's own bitmap, 207 rows of
  `0x370` bytes (220 pixels at 32 bits). Finally the slot's path becomes
  `sprintf("%s\%s", content dir, basename of the record's scenario path)`.
- The list (E, V): `0x19c5f0` reads the globals tag (pointer at
  `0x4e034c`), takes its block at `+0x178` (UI level data, one element),
  and walks that element's block at `+0x10`: the multiplayer levels, elements
  of `0xc64` bytes. In the disc's `mainmenu.map` that block has **50 slots:
  12 retail maps and 38 empty ones**. Each empty slot has map id `-1` and its
  own preview bitmap already assigned (V). The element's other blocks
  (`+0x0`, 4 elements, and `+0x8`, 20 elements; the campaign lists, S) are
  never touched by the patch path (E).
- Loading (E): `0x214db0` builds the map file name from a slot's path. A
  path starting with `t:\` or `w:\` (lower case) becomes `"%s.map"` of the
  whole path. Anything else becomes `d:\maps\<basename>.map`. The content
  root is the literal `t:\` at `0x450f94`, so the slot path is
  `t:\$C\<offer ID>\<map>`, and the map opens from the package directory.

#### The v2 record (the whole of `patch.lvl`)

| offset | size | field | slot offset |
|---|---|---|---|
| `0x000` | 4 | version, 2 | |
| `0x004` | 4 | CRC (above) | |
| `0x008` | 4 | record type, must be 1 | |
| `0x00c` | `0x100` | scenario path; only its basename is used | `0xb4c` (rewritten) |
| `0x10c` | 4 | map id | `0x000` |
| `0x110` | 4 | unused by 1.0 (`-1` in the file, V) | |
| `0x114` | `0x240` | names, 9 languages of 32 UTF-16 units | `0x00c` |
| `0x354` | `0x900` | descriptions, 9 languages of 128 UTF-16 units | `0x24c` |
| `0xc54` | 4 | sort order | `0xc4c` |
| `0xc58` | 1 | flags byte | `0xc50` |
| `0xc5c` | 16 | per-game-type team counts | `0xc54` |
| `0xc6c` | `0x2c790` | preview, 207 rows of 220 pixels, 32 bits | slot bitmap |

Slot `+0x4`/`+0x8` is the slot's bitmap tag reference, kept.

### 8.2 v2 against v3

`patch_v3.lvl` (E: update `0x19f4c0`; V: all three files) is an 8-byte
header (version 3, record count) and then `count` records of `0x2d400`
bytes. Each record has the v2 layout with **one 4-byte field inserted at
`+0x10c`**. Everything from the map id on moves up by 4. The update's
applier `0x19f2e0` is 1.0's `0x19be00` with every source offset from `+0x10c`
up raised by 4 and nothing else changed, except one extra call at the end
(below). Its validator `0x19f270` is 1.0's `0x19bdc0` with version 3 and
length `0x2d400`. Every v3 record's CRC checks with the same algorithm over
`0x2d400` bytes (V).

The inserted field is the map's cache header checksum (header `+0x2d0`).
It equals that header field in all nine maps (V). The update keeps
`(map id, checksum)` pairs in a 64-entry table (`0x1a0200`, table at
`0x4f5ab8`). `0x19f080` refuses a downloaded map whose computed checksum
doesn't match it, in the cache-file verification state machine around
`0x19e770`.

So a v2 record can be made from a v3 record by pure mapping:
`v2[0x0] = 2`, `v2[0x8..0x10c] = v3[0x8..0x10c]`,
`v2[0x10c..0x2d3fc] = v3[0x110..0x2d400]`, then the CRC into `+0x4`.
Nothing comes from the map file. The map file doesn't hold the names,
descriptions, preview or map id (S: we didn't look for them, and they
aren't needed). The nine map ids are distinct from each other and from
the 12 retail ids (V).

### 8.3 Do the maps load in the 1.0 engine?

- **Header checks** (E): 1.0's `0x122870` (update `0x123fc0`, same code)
  wants `head`/`foot`, version 8, a file size up to the per-type limit
  (`0x214ed0`: multiplayer and main menu `0x5000000`), a type under 5 and a
  name shorter than 32. All nine pass: version 8, type multiplayer, build
  `02.09.27.09809`, largest about 55 MiB (V).
- **Checksums** (V): every DLC map's XOR checksum matches its header, as the
  retail maps' do.
- **Shared maps** (V): each multiplayer map's header records two creation
  stamps of the maps it was built against (`+0x180` matches `mainmenu.map`'s
  own stamp at `+0x178`, `+0x188` matches `shared.map`'s). The DLC maps
  carry the same two values as the retail maps. They were built against
  1.0's own `mainmenu.map` and `shared.map`, which is all the update has
  too: the map pack ships no shared map. The DLC maps' own stamps (`+0x17c`)
  and the word at `+0x2bc` (1 on retail, larger on DLC) differ. No reader
  of either was found in 1.0's cache module (S: searched by displacement
  only).
- **Tag groups** (V): every map, retail and DLC, has the same 118-entry
  group table. The groups the DLC maps use are a subset of those the retail
  multiplayer maps use (equal, in fact). Retail XBEs have no tag
  definitions, so field layouts can't be compared from the executable.
- **Metadata size** (V, S): three DLC maps (Turf, Gemini, Triplicate)
  have a larger total metadata size (`+0x1c`) than any retail multiplayer
  map, Turf by about 380 KB. There's no constant limit. The loader takes it
  from the physical arena (`push 0x30e4000` at `0x12b423`, update
  `0x12cb93`), the same in both. On hardware the update loads these maps,
  so 1.0 should too, unless 1.0 leaves less of the arena free at load time.
  That isn't measured.
- **Loader shape** (E): comparing every `.text` function of 1.0 with the
  update, normalised (addresses masked), the cache module
  (`0x120000`-`0x123500`) is identical apart from two functions that sign a
  saved block (`0x121120`, `0x121350`; S, they look like settings saves). The map path module (`0x214a60`-`0x214ed0`)
  moved to `0x19ebd0` unchanged. The map list code (`0x19c120`-`0x19d220`)
  is identical apart from the three `patch.lvl` functions above. Over the
  whole image about 10,400 of about 11,150 functions match. About 700
  differ, spread through gameplay, networking and UI. They weren't
  attributed. One of them could be an engine fix a DLC map relies on (S).
- **Limits** (E, V): 38 free slots for 9 maps. The update's new checksum
  table holds 64. No map id range check was found on the 1.0 list path
  (`0x19c5f0` compares ids for equality only).

### 8.4 Content checks on a served package

- The game's content list goes through `XFindFirstContent` and reads
  `ContentMeta.xbx`'s first `0x88` bytes. It needs the `XCSF` magic and
  flag bit 0, and the signature is checked as in
  [section 7](#7-why-the-game-said-the-content-was-damaged) (E, per that
  section). `kernel::content::sign` already does this.
- 1.0 does **not** hash the package's files. Its only callers of
  `XLoadContentSignaturesEx` (`0x2d30d5`) are in XONLINE (`0x3b1d11`,
  `0x3c6bd2`). Its ten `XCalculateSignatureBegin` callers are saves and
  XONLINE (E). `patch.lvl` is checked only by its CRC.
- The update adds a digest of `patch_v3.lvl` (`0x19ff70`:
  `XCalculateSignatureBegin` flag 2 over the header and records, compared
  with a 20-byte value fetched by `0x216d80`, S: from the package's
  signature table) and the map checksum gate above. Neither exists in 1.0.
- The VFS can satisfy all of 1.0's checks. Not known (S): whether 1.0
  compares the directory name with the header's offer ID, and whether an
  out-of-date file table in a `ContentMeta.xbx` matters anywhere outside
  XONLINE. The safe choice is matching names and a rebuilt or copied
  table.

### 8.5 Why the disc's `patch.lvl` is never installed

1.0 reads the `patch.lvl` of every package. The disc's one has Lockout's
id, so 1.0 would overwrite the **retail Lockout slot** and set its path to
`t:\$C\<offer ID>\lockout`; no package holds a `lockout.map` (E for the
rewrite, V for the files). Lockout may then fail to load (S). On a real
Xbox this never happens, because the installer always puts the update in
`$u`. The import serves its own `patch.lvl` instead, so the retail slot
is untouched.

### 8.6 The layout and the check

`h2 import-mappack` (section 5) writes nine packages with offer IDs
`4d530064000001NN` to `...03NN`: a `ContentMeta.xbx` made from the source
package's with the offer ID changed and signed by `kernel::content::sign`,
the map file, and a v2 `patch.lvl` made by `content::patch_lvl::v2_from_v3`
from that map's `patch_v3.lvl` record (8.2). The file table is copied and
the disc's `patch.lvl` is never installed. On the user's disc the
`real_map_pack_gives_nine_packages_1_0_can_read` test finds nine packages
whose `patch.lvl` passes 1.0's check (V).

A hook on `0x19bfd0` that reads `patch_v3.lvl`, converts each record and
calls `0x19be00` would keep the three real packages and need no new
metadata, but it is guest code to maintain; it is not built.

## Not verified

The in-game check: start 1.0 with the import done (the ISO beside the disc
as `mappack.iso`, then `h2 play`) and open a local custom game's map
choice.

- The nine new maps are listed with their names and previews (their sort
  orders are all above the retail ones, V).
- A game on **Turf** reaches spawn. It has the largest metadata of the
  nine, so it also tests the arena (8.3). If it spawns, 1.0 plays the map
  pack.
- **Lockout** loads once, to confirm 8.5.
- Custom games over Live and system link on these maps.
- Whether 1.0 compares the package directory name with the header's offer
  ID, and whether a stale file table in a `ContentMeta.xbx` matters outside
  XONLINE (8.4).
- The offer ID, drive and file-hash checks of the update (section 3).
