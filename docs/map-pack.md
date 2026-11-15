# The Multiplayer Map Pack disc

To install it: put the map pack ISO next to `halo2.iso` as `mappack.iso`.
`h2 play` and `h2 run` do the rest at start-up ([section 4](#4-automatic-install)).

Status: `h2 import-mappack` installs the disc's content into the emulated
hard disk, signs each package for this console as the disc's installer
would ([section 7](#7-why-the-game-said-the-content-was-damaged)), and keeps
its title update aside. **The new maps won't show up in
the game yet.** Only the title update reads their map list, and we can't run
the update yet ([title-updates.md](title-updates.md)).

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
([title-updates.md](title-updates.md) section 3.6, "Guest relaunch"). Then the
update lists the content directories under `T:\$c`, reads each
`ContentMeta.xbx` and `patch_v3.lvl`, adds the maps to the multiplayer map
list, and opens them from that directory.

**Do the maps work with 1.0? No, not in the map list.** 1.0 can find a
package, but it reads `patch.lvl`, and that only names Lockout. The new maps
need the update. This is why the disc carries both files: an un-updated
console sees a package that adds nothing. Nobody has run either path here
yet, so this is a reading of the files, not an observation.

To be confirmed once the update runs:

- the offer ID. We use the title ID in the high half and the package number
  in the low half (`4d53006400000001`), the usual convention. The offer ID
  field of each package's `ContentMeta.xbx` (offset `0x28`) agrees for all
  three packages;
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
something wrong: version 2 signs `ContentMeta.xbx`, so an install made
before it (an unversioned stamp) is written again, signed, at the next
start.

It never stops the game from starting: a problem is printed as
`map pack: not installed: <why>` and the game runs on. `--no-mappack` or
`H2_MAPPACK=0` (also `false`, `no`, `off`) skips the check.

`h2 play --follow` runs `h2 play` as a child with the same disc path and
`--data`, so the followed build does the install itself; `--no-mappack`
passes through. With `--fresh-data` the data dir is emptied at every launch,
so every launch installs again (about 430 MB).

## 5. `h2 import-mappack`

The manual path, for a map pack ISO kept elsewhere or to see what would be
written.

```sh
cargo h2 import-mappack ~/Downloads/<map pack>.iso
cargo h2 import-mappack <iso> --data <data root> --disc disc/halo2.iso --dry-run
```

It reads the disc's directory tree and writes:

- every file of every `/demos/dlc/<8 hex digits>/` package to
  `<data>/disk/Partition1/TDATA/4D530064/$c/<offer ID>/`, which is the
  host side of `T:\$c\<offer ID>\` in `crates/vfs`
  (`vfs::Config::content_dir`);
- the update copy that matches your Halo 2 disc to
  `<data>/updates/4D530064/<certificate version>/default.xbe`, for example
  `.../00000303/default.xbe`.

The update is deliberately **not** put in `TDATA\4D530064\$u`. If it were
there, 1.0 would try to relaunch into an image we can't run yet
([title-updates.md](title-updates.md) 3.6 says the same). It waits under
`updates/` for `h2 image list` and `h2 image map`.

Options:

- `--data DIR` sets the data root. The default is `$TMPDIR/h2boot-data`,
  the same as `h2 play` and `h2 run`.
- `--disc` picks the Halo 2 disc (ISO or XBE) used to choose the update. The
  default is `$H2_DISC`, then the first `.iso` in `disc/`.
- `--no-update` skips the update.
- `--force` overwrites files from an earlier import.
- `--dry-run` lists what would be written.

It checks:

- the disc has at least one package, and each package has a `ContentMeta.xbx`;
- every name fits FATX (ASCII, at most 42 bytes);
- the chosen update has Halo 2's title ID, and its version's low byte
  matches the base disc's with a non-zero update byte;
- the map pack ISO itself is never written to (`safe_out`).

Every file is published whole or not at all. Nothing is read from inside
the files except each `ContentMeta.xbx` header, which is signed (all of them
before anything is written, so a malformed one leaves the data dir as it
was), and the update's certificate. Every other file is copied byte for
byte. Import before starting the game:
`vfs`'s host backend caches directory listings, so a running game won't see
files added under it.

The map pack ISO stays read-only. Only the content and the update are
copied, about 440 MB. The videos and the installer stay on the disc.

### Tests

- `cli` `mappack_cmd::tests` (synthetic XDVDFS image, XBE headers and
  ContentMeta headers): the package and update plan, refusals (no
  packages, a package without metadata, a malformed metadata header, no
  update for the base version, the wrong title or version), the written
  layout checked against `vfs::Config::content_dir`, that each
  `ContentMeta.xbx` is signed and otherwise unchanged, that nothing goes to
  `$u`, `--force`, `--dry-run`, and that an install with an unversioned
  stamp is redone and signed.
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

1. **Run the update** ([title-updates.md](title-updates.md)): an address map
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
4. **Multiplayer**. The maps are multiplayer only. Getting a local
   multiplayer game going on our runtime is a separate job, after the
   campaign work.

Until then the import is groundwork. It costs nothing at run time. 1.0
should find nothing new in the packages, since it reads only `patch.lvl`,
but that hasn't been run either.

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
