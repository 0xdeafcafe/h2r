# The Multiplayer Map Pack disc

To install it: put the map pack ISO next to `halo2.iso` as `mappack.iso`.
`h2 play` and `h2 run` do the rest at start-up ([section 4](#4-automatic-install)).

Status: Partly built. `h2 import-mappack` and the automatic install are
built; playing the maps in the game on 1.0 is open ([Not verified](#not-verified)),
and running the title update is design only.
Code: `crates/apps/cli/src/mappack_cmd.rs` (`h2 import-mappack`), `crates/formats/content/src/patch_lvl.rs` (`v2_from_v3`), `crates/runtime/kernel/src/content.rs` (`sign`). Tests: `mappack_cmd::tests`, `crates/runtime/kernel/tests/misc.rs`, `crates/runtime/vfs/tests/storage.rs`.
Switch: `H2_MAPPACK` (on) and `--no-mappack`, listed in [switches.md](../runtime/switches.md).

`h2 import-mappack` installs the nine maps as nine content packages, one
map each, with a version 2 `patch.lvl` made from the map's `patch_v3.lvl`
record, so 1.0 lists them without the title update
([section 8](#8-playing-the-map-pack-on-10)). Each package is signed for
this console as the disc's installer would
([section 7](#7-why-the-game-said-the-content-was-damaged)), and the title
update is kept aside.

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
same as the retail disc's maps. `crates/formats/map` opens them without changes:
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
`patch.lvl` per map ([section 8](#8-playing-the-map-pack-on-10)). This
section is a reading of the files (**S**), not an observation.

Open until the update runs:

- the offer ID the update expects. The disc packages' own offer IDs (field
  `0x28` of each `ContentMeta.xbx`) are the title ID in the high half and
  the package number in the low half (`4d53006400000001`), the usual
  convention. Our import uses its own per-map IDs instead (section 5), the
  same in the directory name and the header (8.4);
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
and the line says so. Otherwise it prints nothing.

This is the `map-pack` package's `convert` item
(`content/map-pack/convert/map-pack.convert.toml`), run by
`mappack_cmd::package::MapPack` before boot; `h2 package prepare` runs it
alone ([studio-redesign.md](../editor/studio-redesign.md),
"Conversions"). Each map is a conversion unit whose output is
`<data>/tags/maps/<stem>.converted`. The installer records the disc in
`mappack.stamp` and the package records its units in
`<data>/packages/stamps/map-pack.toml`. With
no Map Pack disc and nothing installed before, the `map-pack` package is
off, and `packages.list` and `h2 package prepare` say why.

"Installed" means `<data>/mappack.stamp` records the import's format
version, the ISO's size, its modification time and the base disc's
certificate version. Each map must have a verified store marker and its
retail content package. Replacing the ISO, changing the game disc
version, or clearing the store's map records triggers conversion. A stamp with another format version
(`STAMP_VERSION` in `crates/apps/cli/src/mappack_cmd.rs`) also installs again.
The current format signs `ContentMeta.xbx` and installs one package per
map with our own `patch.lvl`; it removes disc-shaped packages from an
older format, whose `patch.lvl` would take over the Lockout slot (8.5).

It never stops the game from starting: a problem is printed as
`packages: map-pack item 0 <stem>: <why>` and the game runs on. `--no-mappack` or
`H2_MAPPACK=0` (also `false`, `no`, `off`) skips the check.

`h2 play --follow` runs `h2 play` as a child with the same disc path and
`--data`, so the followed build does the install itself; `--no-mappack`
passes through. With `--fresh-data` the data dir is emptied at every launch,
so every launch installs again (about 430 MB).

Without the mod repository (no Live or no map gate), the Xbox Live menu's
Mods row (retail's Content Download) opens a dialog saying where to put
the ISO (`mappack.iso` next to `halo2.iso`, then start the game):
`game::custom_games` ([live-custom-games.md](../live/live-custom-games.md)).

## 5. `h2 import-mappack`

The manual path, for a map pack ISO kept elsewhere or to see what would be
written.

```sh
cargo h2 import-mappack ~/Downloads/<map pack>.iso
cargo h2 import-mappack <iso> --data <data root> --disc disc/halo2.iso --dry-run
```

It reads the disc's directory tree and, for every `/demos/dlc/<8 hex
digits>/` package, its `ContentMeta.xbx` and `patch_v3.lvl`. It stores
each map through `workspace::converted::store_converted`, with shared
caches supplied by the Halo 2 disc. Then it writes
**one package per map** (1.0 takes one map per package, 8.1) to
`<data>/disk/Partition1/TDATA/4D530064/$c/<offer ID>/`, which is the host
side of `T:\$c\<offer ID>\` in `crates/runtime/vfs` (`vfs::Config::content_dir`).
Each holds:

| file | from |
|---|---|
| the map, under its disc name | copied byte for byte for the retail content path |
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
equal the bare package numbers (`...0001` to `...0003`) of the disc-shaped
layout. After writing, the import removes those package directories
(nothing else under `$c`), with or without `--force`.

It also writes the update copy that matches your Halo 2 disc to
`<data>/updates/4D530064/<certificate version>/default.xbe`, for example
`.../00000303/default.xbe`.

The update is deliberately **not** put in `TDATA\4D530064\$u`. If it were
there, 1.0 would try to relaunch into an image we can't run
([title-updates.md](../runtime/title-updates.md) 3.6 says the same). It waits under
`updates/` until the update can be run. `h2 image list` and `h2 image map`
are design only ([title-updates.md](../runtime/title-updates.md)
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

- `content` `patch_lvl::tests` (synthetic records): the CRC against the
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
  with an unversioned or version 2 stamp are redone.
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

### Empty directories in the image

extract-xiso writes an empty directory as one zero-filled sector, which
holds a single record with an empty name. `xiso` skips nameless records
(test `zero_filled_directory_is_empty`), so the four empty directories on
this disc (`/videos`, `/movies/movie4`, `/movies/soundtest`,
`/movies/movie2/subtitle`) do not make `h2 extract` refuse the disc as
having an unsafe path.

## 6. How the maps play on 1.0

The nine maps play on the retail 1.0 executable with no title update.
`h2 import-mappack` (section 5) writes nine packages under `T:\$c`, each
with one map, a signed `ContentMeta.xbx` (section 7) and a version 2
`patch.lvl` of one record. 1.0's content scan (8.1) reads that record,
puts the map in a free slot of the multiplayer map list, and loads the map
from the package directory. The Select Map dialog lists the maps under
**Map Pack** ([map-select.md](map-select.md)). They play in multiplayer:
custom games over Live and System Link, and Live hoppers when all matched
consoles and the host have the pack. The same hopper offers disc maps to
other players. [Map Pack rotations](../live/live-matchmaking.md#map-pack-rotations)
Section 8 holds the reading of 1.0 and the open in-game check.

### The title update (design)

Playing through the update instead is not built. It needs the update to
run (an address map from 1.0 to `0x303`, hooks placed through it, an
image option so `h2 play` boots `<data>/updates/4D530064/00000303/default.xbe`
with the 1.0 disc as `D:`; [title-updates.md](../runtime/title-updates.md)),
kernel ordinal 270 (`RtlCompareString`, which the update imports and 1.0
doesn't), and a trace of its `T:\$c` enumeration to confirm the offer ID
directory name (`vfs::offer_id`). The one-package-per-map layout does not
serve the update: its packages hold no `patch_v3.lvl`, and the update
checks that file's digest against the package's signed table (8.4). The
import would need a second layout, the disc's three packages without their
`patch.lvl`.

## Pages

| Page | What |
|---|---|
| [Map pack: playing on 1.0](map-pack-playing.md) | How the maps play on 1.0, why the game said the content was damaged, and playing the map pack on 1.0. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## 7. Why the game said the content was damaged

See [Map pack: playing on 1.0](map-pack-playing.md#7-why-the-game-said-the-content-was-damaged).

## 8. Playing the map pack on 1.0

See [Map pack: playing on 1.0](map-pack-playing.md#8-playing-the-map-pack-on-10).

## Not verified

See [Map pack: playing on 1.0](map-pack-playing.md#not-verified).
