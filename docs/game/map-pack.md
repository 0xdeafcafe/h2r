# The Multiplayer Map Pack disc

To install it: put the map pack ISO next to `halo2.iso` as `mappack.iso`.
`h2 play` and `h2 run` do the rest at start-up ([section 4](#4-automatic-install)).

Status: `h2 import-mappack` installs the nine maps as nine content
packages, one map each, with a version 2 `patch.lvl` made from the map's
`patch_v3.lvl` record, so **1.0 should list them without the title update**
([section 8](#8-playing-the-map-pack-on-10)). Each package is signed for this
console as the disc's installer would
([section 7](#7-why-the-game-said-the-content-was-damaged)), and the title
update is kept aside. Tests pass on synthetic discs and on the user's map
pack ISO, and the owner has played the maps in the game (2026-11-17; the
check is [section 8.6](#86-what-to-change-and-the-check)).

The official Halo 2 Multiplayer Map Pack disc (2005) holds the nine
downloadable multiplayer maps, a Halo 2 title update, an installer and some
bonus videos. On an Xbox you boot the disc, and its installer copies the maps
and the update to the hard disk. The next time you start Halo 2 from its own
disc, the console runs the update in place of the disc's 1.0 executable, and
the update lists the new maps.

Everything below was worked out from the user's own copy with our tools
(`h2 ls`, `h2 info`, `h2 tags`, the `xiso` and `xbe` crates). Nothing from
the disc is in the repo: no bytes, hashes, tables or strings. The facts here
are file names, sizes rounded off, version numbers and addresses.

## 1. What's on the disc

It's an XDVDFS image, about 5.5 GB. The parts that matter:

| path | what |
|---|---|
| `/default.xbe` | the disc's own menu (title `4D5380CD`, cert version `0x46`), not Halo 2 |
| `/demos/update/updater.xbe` | the installer (title `4D5380CD`) |
| `/demos/update/Updates/default.xbe.03` ... `.10` | eight copies of the Halo 2 title update, about 4.9 MB each |
| `/demos/update/Updates/downloader.xbe.NN`, `contentmeta.xbx.NN` | the update's companions, one per copy |
| `/demos/update/Updates/Selection.utf.01` ... `.08`, `Confirmation.utf.*` | the installer's prompts, one per language |
| `/demos/dlc/00000001/` | content package 1: `containment.map`, `warlock.map` |
| `/demos/dlc/00000002/` | content package 2: `deltatap.map` (Sanctuary), `turf.map` |
| `/demos/dlc/00000003/` | content package 3: `backwash.map`, `dune.map` (Relic), `elongation.map`, `gemini.map`, `triplicate.map` (Terminal) |
| `/demos/dlc/maps.png` | a picture for the installer |
| `/movies/`, `/cdxmedia/` | bonus videos with their own players, the menu's media |

Each content package also holds `ContentMeta.xbx` (the package metadata:
title, offer ID, localized names and a list of the files in the package with
their SHA-1 digests; on the disc its console signature is still blank, see
[section 7](#7-why-the-game-said-the-content-was-damaged)), `ContentImage.xbx` (its icon), `patch.lvl` and `patch_v3.lvl`.
The nine maps are 39-58 MB each, about 430 MB in total.

The image has no `TDATA` or `UDATA` tree. The installer builds those on the
hard disk.

### The maps

All nine are version 8 cache files with build string `02.09.27.09809`, the
same as the retail disc's maps. `crates/map` opens them without changes:
`h2 tags` on `backwash.map` lists all 4,547 tags.

### `patch.lvl` and `patch_v3.lvl`

These are the multiplayer map list entries for a package: a version word,
a count, then one fixed-size record per map (about 185 KB each, so they
likely carry the map's preview image as well as its name and description).

- `patch.lvl` is version 2, and it's the same file in all three packages.
  Its one record is for Lockout, a map already on the disc.
- `patch_v3.lvl` is version 3, and it has one record per map in its own
  package (2, 2 and 5 records).

The 1.0 executable names `patch.lvl`. The update names `patch_v3.lvl`
instead. The rest of the content-handling names (`ContentMeta.xbx`, the
`$c` directory, the `$u` update directory) are in both.

## 2. The title update

The eight `default.xbe.NN` files are one build. They differ only in the first
~0x370 bytes of the header (signature and certificate), and their
certificate versions are `0x300 + NN`: `.03` is `0x303`, `.10` is `0x30a`.
The low byte names the base disc each copy is signed for, so the installer
picks the copy whose low byte matches the disc's certificate version. Our
retail disc's `default.xbe` has certificate version 3, so **its update is
`default.xbe.03`, certificate version `0x303`**.

| | 1.0 (disc) | update (`default.xbe.03`) |
|---|---|---|
| title | Halo 2, `4D530064` | Halo 2, `4D530064` |
| certificate version | `0x3` | `0x303` |
| header timestamp | 2004-09-28 | 2005-03-22 |
| certificate timestamp | 2004-09-30 | 2005-04-06 |
| image size | `0x5754c0` | `0x57d2a0` |
| entry point | `0x2d0aee` | `0x2d6f2e` |
| kernel thunk table | `0x411520`, 152 imports | `0x417b60`, 153 imports |
| XDK libraries | all 1.0.5849 | all 1.0.5849, the same set |

The extra import is ordinal 270 (`RtlCompareString`). The dates fit the
spring 2005 release of the first downloadable maps. This is the only update
we have, so we can't say where it sits among later ones.

The downloader is an older Xbox Live content downloader (header 2003,
re-signed with the update). It isn't needed offline.

## 3. What installing does

From the file names, the 1.0 executable and the XDK's content layout (not
from running the installer), the installer:

1. copies each `/demos/dlc/<n>/` package to
   `E:\TDATA\4D530064\$c\<offer ID>\`, which is `T:\$c\<offer ID>\` to the
   game. The offer ID is formatted as 16 hex digits;
2. copies the matching `default.xbe.NN` as `default.xbe` (with its
   `downloader.xbe` and `contentmeta.xbx`) to `E:\TDATA\4D530064\$u\`, the
   update path 1.0 itself looks in.

At boot, 1.0 finds `$u\default.xbe` and relaunches into it
([title-updates.md](../runtime/title-updates.md) section 3.6, "Guest relaunch"). Then the
update lists the content directories under `T:\$c`, reads each
`ContentMeta.xbx` and `patch_v3.lvl`, adds the maps to the multiplayer map
list, and opens them from that directory.

**Do the disc's packages work with 1.0? No, not in the map list.** 1.0 can
find a package, but it reads `patch.lvl`, and that only names Lockout. The
new maps need the update. This is why the disc carries both files: an
un-updated console sees a package that adds nothing. Our import therefore
doesn't install the disc's packages as they are: it serves 1.0 its own
`patch.lvl` per map ([section 8](#8-playing-the-map-pack-on-10)). Nobody has
run either path here yet, so this is a reading of the files, not an
observation.

To be confirmed once the update runs:

- the offer ID the update expects. The disc packages' own offer IDs (field
  `0x28` of each `ContentMeta.xbx`) are the title ID in the high half and
  the package number in the low half (`4d53006400000001`), the usual
  convention. Our import uses its own per-map IDs instead (section 5), the
  same in the directory name and the header (8.4's safe choice);
- which drive the game enumerates. The format takes a drive letter; `T:` is
  where installed content lives;
- whether the update checks the per-file hashes in `ContentMeta.xbx`. The
  maps and the other files are copied byte for byte, so they match if it
  does. 1.0's checks are in [section 7](#7-why-the-game-said-the-content-was-damaged).

## 4. Automatic install

`h2 play` and `h2 run` look in the directory of the game disc you pass
(`disc/` for `disc/halo2.iso`) before starting:

1. `mappack.iso`, in any case, is the map pack.
2. Failing that, any other `.iso` there whose directory tree has the map pack
   layout (content packages under `/demos/dlc/`, each with a
   `ContentMeta.xbx`) is. Only the disc's header and directory tables are
   read, so an unrelated image costs a few reads. The game disc itself is
   never a candidate.

If one is found and isn't installed yet, it runs the same import as
`h2 import-mappack` (with the update for your disc, and overwriting an older
import) and prints one line, such as
`map pack: installed 9 maps and title update from mappack.iso`. When this
disc's version has no update on the map pack, only the maps are installed
and the line says so. Otherwise it prints `map pack: already installed`.

"Installed" means `<data>/mappack.stamp` records the import's format
version, the ISO's size, its modification time and the base disc's
certificate version, and
`<data>/disk/Partition1/TDATA/4D530064/$c` still exists. Replace the ISO, change
the game disc version, or empty the data dir (`--fresh-data`), and the next
start installs again. The format version is bumped when older imports wrote
something wrong, so an older install is written again once, at the next
start:

- version 2 signs `ContentMeta.xbx` (an unversioned stamp copied it
  unsigned);
- version 3 installs one package per map with our own `patch.lvl`, and
  removes the three disc-shaped packages version 2 wrote, whose `patch.lvl`
  took over the Lockout slot (8.5).

It never stops the game from starting: a problem is printed as
`map pack: not installed: <why>` and the game runs on. `--no-mappack` or
`H2_MAPPACK=0` (also `false`, `no`, `off`) skips the check.

`h2 play --follow` runs `h2 play` as a child with the same disc path and
`--data`, so the followed build does the install itself; `--no-mappack`
passes through. With `--fresh-data` the data dir is emptied at every launch,
so every launch installs again (about 430 MB).

Without the ISO, the Xbox Live menu's "Content Download" row opens a dialog
saying where to put it (`mappack.iso` next to `halo2.iso`, then start the
game): `boot::custom_games` ([live-custom-games.md](../live/live-custom-games.md)).

## 5. `h2 import-mappack`

The manual path, for a map pack ISO kept elsewhere or to see what would be
written.

```sh
cargo h2 import-mappack ~/Downloads/<map pack>.iso
cargo h2 import-mappack <iso> --data <data root> --disc disc/halo2.iso --dry-run
```

It reads the disc's directory tree and, for every `/demos/dlc/<8 hex
digits>/` package, its `ContentMeta.xbx` and `patch_v3.lvl`. Then it writes
**one package per map** (1.0 takes one map per package, 8.1) to
`<data>/disk/Partition1/TDATA/4D530064/$c/<offer ID>/`, which is the host
side of `T:\$c\<offer ID>\` in `crates/vfs` (`vfs::Config::content_dir`).
Each holds:

| file | from |
|---|---|
| the map, under its disc name | copied byte for byte, the only copy of it |
| `patch.lvl` | made from the map's `patch_v3.lvl` record (8.2), version 2, one record |
| `ContentMeta.xbx` | the disc package's, with this package's offer ID at `0x28`, then signed (section 7) |
| `ContentImage.xbx` and any other file | copied byte for byte from the disc package |

The disc's `patch.lvl` (Lockout's record, 8.5) and `patch_v3.lvl` are never
installed. The record's scenario path names the map: its basename plus
`.map` must be a file in the same disc package (any case), every map in
the package must have exactly one record, and no two records may share a
map id. The `ContentMeta.xbx` file table is left as on the disc, listing
the disc package's files and digests: its own digest in the header stays
right, and 1.0 doesn't hash the files (8.4).

Offer IDs: Halo 2's title ID in the high half, and
`package number << 8 | record number` in the low half, the record number
counting from 1 in that package's `patch_v3.lvl`. So package 1's two maps
are `4d53006400000101` and `4d53006400000102`, package 2's are `...0201`
and `...0202`, and package 3's five are `...0301` to `...0305`. They never
equal the bare package numbers (`...0001` to `...0003`) the older layout
used. After writing, the import removes those older package directories
(nothing else under `$c`), with or without `--force`.

It also writes the update copy that matches your Halo 2 disc to
`<data>/updates/4D530064/<certificate version>/default.xbe`, for example
`.../00000303/default.xbe`.

The update is deliberately **not** put in `TDATA\4D530064\$u`. If it were
there, 1.0 would try to relaunch into an image we can't run yet
([title-updates.md](../runtime/title-updates.md) 3.6 says the same). It waits under
`updates/` until the update can be run. `h2 image list` and `h2 image map`
do not exist yet: they are design only ([title-updates.md](../runtime/title-updates.md)
section 3.6).

Options:

- `--data DIR` sets the data root. The default is `~/Library/Application Support/h2recomp/data`,
  the same as `h2 play` and `h2 run`.
- `--disc` picks the Halo 2 disc (ISO or XBE) used to choose the update. The
  default is `$H2_DISC`, then the first `.iso` in `disc/`.
- `--no-update` skips the update.
- `--force` overwrites files from an earlier import.
- `--dry-run` lists what would be written.

It checks:

- the disc has at least one package, and each package has a `ContentMeta.xbx`
  and a `patch_v3.lvl`;
- each `patch_v3.lvl` record passes the update's check (version 3, length,
  CRC) and record type 1, and each `patch.lvl` made from it passes 1.0's
  (8.1);
- every name fits FATX (ASCII, at most 42 bytes);
- the chosen update has Halo 2's title ID, and its version's low byte
  matches the base disc's with a non-zero update byte;
- the map pack ISO itself is never written to (`safe_out`).

Every file is published whole or not at all. Nothing is read from inside
the files except each `ContentMeta.xbx` header, each `patch_v3.lvl` and the
update's certificate, all before anything is written, so a malformed one
leaves the data dir as it was. Every other file is copied byte for byte. Import before starting the game:
`vfs`'s host backend caches directory listings, so a running game won't see
files added under it.

The map pack ISO stays read-only. Only the maps, their images and the
update are copied, about 440 MB, each map once. The videos and the
installer stay on the disc.

### Tests

- `content` `patch_lvl::tests` (synthetic records; the module moved from
  `cli` so the boot can build records too): the CRC against the
  CRC-32 check value, a v3 record's own CRC recomputed, the v2 mapping
  (copied span, dropped checksum, shift, map id, new CRC that 1.0's check
  accepts), and refusals of wrong lengths, versions, types and CRCs.
- `cli` `mappack_cmd::tests` (synthetic XDVDFS image with two packages and
  three maps, XBE headers, ContentMeta headers and `patch_v3.lvl` files):
  the plan, the offer ID scheme, one package per map, refusals (no
  packages, a package without metadata, a malformed metadata header, a
  record naming a missing map, a map without a record, a map named twice,
  a shared map id, a bad record CRC, no update for the base version, the
  wrong title or version), and the written layout end to end: the package
  directories by offer ID through `vfs::Config::content_dir`, each
  `ContentMeta.xbx` with its offer ID and a signature `kernel::content`
  accepts and otherwise unchanged, each `patch.lvl` valid for 1.0 and
  naming the map beside it, no Lockout record anywhere, nothing in `$u`,
  `--force`, `--dry-run`, removal of the older layout, and that installs
  with an unversioned or version 2 stamp are redone once.
- `cli` `real_map_pack_gives_nine_packages_1_0_can_read` (ignored, reads
  the user's map pack ISO, writes nothing): nine packages with distinct
  offer IDs and map ids, each `patch.lvl` passing 1.0's check and naming a
  map file of its own disc package, each `ContentMeta.xbx` signed with its
  offer ID, and none equal to the disc's Lockout `patch.lvl`. It passes on
  the user's disc.
  `H2_MAPPACK_ISO=disc/mappack.iso cargo test -p cli --bin h2 real_map_pack -- --ignored`
- `kernel`: see section 7.
- `vfs` `installed_content_appears_under_t_content_dir`: files put on the
  host there open and list through `T:\$c\<offer ID>\` (case-insensitive).
- `xiso` `zero_filled_directory_is_empty`: see below.

### An `xiso` fix found on this disc

extract-xiso writes an empty directory as one zero-filled sector. That
holds a single record with an empty name, which the reader used to list as
`<dir>/`, a file of size 0. This disc has four (`/videos`,
`/movies/movie4`, `/movies/soundtest`, `/movies/movie2/subtitle`), and
`h2 extract` refused the whole disc as having an unsafe path. Nameless
records are now skipped.

## 6. What's left before the maps can be played

On 1.0 (section 8), only the in-game check in 8.6. The rest of this section
is for playing them through the title update instead.

1. **Run the update** ([title-updates.md](../runtime/title-updates.md)): an address map
   from 1.0 to `0x303`, hooks placed through it, and `--image` so `h2 play`
   boots `<data>/updates/4D530064/00000303/default.xbe` with the 1.0 disc
   as `D:`. That's the big piece: about 11-17 days before files and 6-11
   after, by that page's estimate. The update's XDK build is the same as
   1.0's, which removes the worst case there.
2. **Kernel ordinal 270** (`RtlCompareString`), which the update imports and
   1.0 doesn't.
3. **Check the content path** on the update. Trace its `T:\$c` enumeration
   (kernel trace) and confirm the offer ID directory name, then fix
   `vfs::offer_id` if it's wrong.
4. **Multiplayer**. The maps are multiplayer only. Custom games over Live
   and system link are the way to play them
   ([live-custom-games.md](../live/live-custom-games.md)); none has been
   tried on these maps yet (8.6).

The one-package-per-map layout doesn't serve the update: its packages hold
no `patch_v3.lvl`, and the update checks that file's digest against the
package's signed table (8.4), so a trimmed copy wouldn't pass. When the
update runs, the import will need a second layout (the disc's three
packages, without their `patch.lvl`) for it.

## 7. Why the game said the content was damaged

With the packages copied byte for byte, Halo 2 listed them at the main menu
and then said **FILE DAMAGED**. Each package's `ContentMeta.xbx` on the disc
is unsigned: its first 20 bytes are zero and its "installed" flag is clear.
On an Xbox the disc's installer signs each one for the console it installs
on. Our import didn't, and our kernel couldn't have checked a signature
anyway: `XcHMAC` (kernel ordinal 340) was an unimplemented export.

Everything here was read from Halo 2 1.0's own XAPI and XONLINE code with
`h2 disasm` and `h2 analyze`. No public description of the `ContentMeta.xbx`
layout was found, so the field names are ours. Public references for the
pieces: the kernel exports `XcHMAC`, `XboxHDKey`, `XboxSignatureKey` and
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
  still listed, but as unusable. That fits what we saw: listed, then
  "damaged".
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

### The fix

We do what the installer does, with the keys our kernel gives the guest:

- `kernel::crypto::XBOX_HD_KEY` is this console's hard disk key. It is all
  zero: there is no EEPROM, and no real console's key is in the repo. The
  kernel writes it into the `XboxHDKey` DATA export at start-up, so the
  guest and the importer always use the same key.
- The kernel now implements `XcHMAC`: HMAC-SHA1 over both input spans. A
  key longer than 64 bytes stops, since nothing here needs that case.
- `kernel::content::sign` sets flags bit 0 and writes
  `HMAC-SHA1(HMAC-SHA1(XBOX_HD_KEY, title ID), bytes 0x14..header size)`
  into bytes `0..20`. `h2 import-mappack` and the automatic install run it
  on every package's `ContentMeta.xbx` and copy everything else byte for
  byte.

We don't patch the guest's check. We don't fake the original console
signature either: no console signature ships on the disc.

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

Can retail 1.0 list and load the nine maps without running the title update,
if we hand it a suitable `patch.lvl`? This section is a static reading of
1.0's executable, the update (`default.xbe.03`, certificate version
`0x303`) and the user's files. Nothing was built or run.

Labels: **E** read from an XBE, **V** measured on the user's files,
**S** speculation.

**Verdict: probably yes, with one catch. Unknown until one map has been
loaded in game.** 1.0 already has the whole mechanism: it reads `patch.lvl`
from every package, fills free slots in the multiplayer map list, and loads
maps from `t:\`. A `patch.lvl` made from a `patch_v3.lvl` record is a pure
field mapping. The catch is that 1.0 takes **one map per package**, so nine
maps need nine packages (or a hook). Nothing found in 1.0's path would refuse
the DLC maps, and the update's loader is the same code. The update does add
checks, but they only make it stricter than 1.0.

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

### 8.5 A side effect of the older import

With the import as it was (stamp version 2), 1.0 read the disc's `patch.lvl` from every
package. Its record has Lockout's id, so 1.0 overwrites the **retail Lockout
slot** and sets its path to `t:\$C\<offer ID>\lockout`. No package holds a
`lockout.map` (E for the rewrite, V for the files). Lockout may fail to load
on 1.0 while the map pack is installed (S, not observed). On a real Xbox
this never happened, because the installer always put the update in `$u`.
Serving our own `patch.lvl` instead of the disc's removes the problem. The
import now does (stamp version 3), and removes the older packages.

### 8.6 What to change, and the check

Built: option 1, in `h2 import-mappack` (section 5): nine packages, offer
IDs `4d530064000001NN` to `...03NN`, a v2 `patch.lvl` per map made by
`content` `patch_lvl::v2_from_v3`, the file table copied, the disc's
`patch.lvl` never installed. On the user's disc the test in section 5 finds
nine packages whose `patch.lvl` passes 1.0's check. The design as written
before it was built:

1. Serve **one package per map**, nine in all, under `T:\$C\` with distinct
   offer IDs. Each holds a `ContentMeta.xbx` made from its source package's,
   with the offer ID changed and signed by `kernel::content::sign`, the
   map file, and a `patch.lvl` made from that map's `patch_v3.lvl` record
   (8.2). This can be done in `h2 import-mappack` (bump the stamp format,
   place each map once, no copies) or in `vfs` as a view over the three
   real packages. Never serve the disc's `patch.lvl`.
2. The alternative is a hook on `0x19bfd0` that reads `patch_v3.lvl`,
   converts each record and calls `0x19be00` for each. That keeps the three
   real packages and needs no new metadata, but it's guest code to
   maintain.

The check, for the user (not run yet): start 1.0 with the import done (put
the ISO beside the disc as `mappack.iso` and `h2 play`, which redoes an
older import once and prints `map pack: installed 9 maps ...`), and open a
local custom game's map choice. The **nine new maps should be listed with
their names and previews** (their sort orders are all above the retail
ones, V). Start a game on **Turf**: the largest metadata of the nine, so it
also tests the arena. If it reaches spawn, 1.0 plays the map pack. Then
load **Lockout** once, to confirm 8.5 is gone.
