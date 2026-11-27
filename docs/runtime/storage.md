# Xbox storage boundary

Status: Built.
Code: `crates/runtime/vfs/src`, `crates/formats/host-dirs/src/lib.rs`. Tests: `crates/runtime/vfs/tests/`, `crates/runtime/vfs/src/host_write_behind_tests.rs`.
Switch: `H2_MAP_DIR` (unset), listed in [switches.md](switches.md).

`crates/runtime/vfs` owns storage names, mounted filesystems, open file objects and
filesystem status results. It is safe Rust and has no dependency on guest
memory, `kernel`, `runtime`, or `cli`. The kernel translates guest pointers,
ANSI object strings, handles, information classes and IO_STATUS_BLOCKs at its
boundary. Nothing here extracts, copies or embeds the user's game files.

## Pages

| Page | Holds |
|---|---|
| [Xbox storage: write metadata and write-behind](storage-writes.md) | How a guest write reaches the host: metadata, then write-behind. |
| [Xbox storage: file objects and kernel integration](storage-nt.md) | File objects, NT behaviour and the kernel entry points. |

## Host directories

Every per-user path h2recomp keeps on the host is under one of three
directories, chosen by `host_dirs::Convention`
(`crates/formats/host-dirs/src/lib.rs`). `Convention::HOST` is `Apple` on
Apple targets (macOS, iOS, tvOS), `Windows` on Windows and `Xdg` on every
other target. **Code**

| Directory | Apple | Windows | Xdg |
| --- | --- | --- | --- |
| Data (`host_dirs::data`) | `~/Library/Application Support/h2recomp` | `%LOCALAPPDATA%\h2recomp` | `$XDG_DATA_HOME/h2recomp`, else `~/.local/share/h2recomp` |
| Cache (`host_dirs::cache`) | `~/Library/Caches/h2recomp` | `%LOCALAPPDATA%\h2recomp\cache` | `$XDG_CACHE_HOME/h2recomp`, else `~/.cache/h2recomp` |
| Config (`host_dirs::config`) | `~/Library/Application Support/h2recomp` | `%LOCALAPPDATA%\h2recomp` | `$XDG_CONFIG_HOME/h2recomp`, else `~/.config/h2recomp` |

A variable that is unset, empty or a relative path counts as unset. With
no `HOME` (or `LOCALAPPDATA` on Windows) a directory does not resolve;
Windows never falls back to `HOME`. Each convention is tested on every
host (`host_dirs` tests, `every_directory_under_every_convention`).
**Test**

What lives where:

- Data: the default data root `data/` (`content::store::default_data_root`,
  `H2_DATA` names another), the movie transcodes `movies/`
  (`movies::install::app_movies_dir`), Live accounts `live/` (`H2_LIVE_DIR` names
  another), `coop-test/` (`h2 coop test`), and the root
  `Config::for_title` uses. Under the data root, `game-variants/` is the
  game-variant store ([rules-engine.md](../game/rules-engine.md#live-game-variants-in-the-game)).
  `tags/` is the asset store `h2 build` and the pre-boot step keep current, and
  `disc/` the disc's files that are not maps, copied once per image
  ([asset-store.md](../engine/asset-store.md), "Extraction on compile").
- Cache: the agent socket and its `inputs/` and `captures/`
  (`agent_proto::cache_dir`), the pipeline cache
  (`d3d::pipeline_store`), `snapshots/` (`H2_SNAPSHOTS` names another), the
  bots' shared nav cache `bots/`, the lab socket and `lab/`, `follow/`, `watch/` and
  `xtask-ledger/`.
- Config: the lab driver's paired hosts, `lab/peers/`.

A UWP host has none of these: it passes its package's LocalFolder to
`Config::new`.

## Namespace and default drives

An Xbox file name is an object-manager name. `\??\D:` is a symbolic link,
whereas `\Device\CdRom0` names the DVD device. The hard disk exposes
`\Device\Harddisk0\PartitionN`. Partition1 contains TDATA/UDATA,
Partition2 is the dashboard partition, and Partition3–5 are utility/cache
partitions. These names and the T/U/Z roles are documented in the
[Cxbx storage implementation](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/support/EmuFile.cpp).

`Vfs::with_storage(Config)` mounts the device objects only and creates no
links: the kernel provides devices, while XAPI creates the drive letters itself
and treats an existing T: as a fatal collision. Neither the boot path nor
`with_disc` creates links.

Host tools without XAPI call `Vfs::create_standard_links(&Config)` afterwards,
which creates exactly D:, T:, U: and Z:. It does not roll back: if it fails
partway (for example `ObjectNameCollision` on a link that already exists), the
links created before the failure remain. The E:/C:/X:/Y: rows below are the
reference mapping and are not created by either call.

`Config::for_title` chooses the app data directory
([Host directories](#host-directories)). `Config::new(root, title_id)`
selects an explicit root. A data directory that does not resolve is a
configuration error (`InvalidParameter`), not a guest path error. Halo 2's
title ID is `0x4D530064`.

| Link | Resolved device path | Host path relative to data root |
| --- | --- | --- |
| D: | `\Device\CdRom0` | User's XISO; mounted separately or by `with_disc` |
| E: | `\Device\Harddisk0\Partition1` | `disk/Partition1` |
| C: | `\Device\Harddisk0\Partition2` | `disk/Partition2` |
| T: | `\Device\Harddisk0\Partition1\TDATA\4D530064` | `disk/Partition1/TDATA/4D530064` |
| U: | `\Device\Harddisk0\Partition1\UDATA\4D530064` | `disk/Partition1/UDATA/4D530064` |
| X: | `\Device\Harddisk0\Partition3` | `disk/Partition3` |
| Y: | `\Device\Harddisk0\Partition4` | `disk/Partition4` |
| Z: | `\Device\Harddisk0\Partition5` by default | `disk/Partition5` |

The title suffix follows `Config.title_id`. Installed content (downloaded,
or installed from a disc such as the Map Pack, [map-pack.md](../game/map-pack.md))
lives under `T:\$c\<offer ID as 16 hex digits>\`, and so do new maps from
map packages ([maps.md](../game/maps.md) §4, offer IDs with bit 31 of the low half
set). `Config::title_data_dir`
and `Config::content_dir(offer)` give the host paths, and `vfs::offer_id`
builds an offer ID from a title ID and content number. X/Y and the default Z mapping
are this runtime's reference layout. A retail title's utility partition is
selected by XAPI; it is not permanently guaranteed to be Partition5. Select
3, 4 or 5 with `Config.utility_partition`, or delete/create the Z: link later.
No cache is automatically erased. Partition0 is the raw disk/config area,
not a FATX directory, and is intentionally unmounted. Additional partition
devices can be mounted explicitly; an unmounted device fails with
`ObjectPathNotFound`.

`Namespace::{create_symbolic_link, delete_symbolic_link, query_symbolic_link,
resolve}` implement the name portion of IoCreateSymbolicLink and related
exports. Links may target directories or other links. Longest matching
component prefix wins; `CdRom0` never matches `CdRom01`. Resolution is ASCII
case-insensitive, follows up to 32 links and bounds expansion length. A cycle
returns `ReparsePointNotResolved`. Link deletion/remapping affects subsequent
opens; an open file retains its original mount and path.

The API accepts both `D:\maps\file.map` and `\??\D:\maps\file.map`, as well
as full device paths. `D:`/`D:\` mean the drive root. There is no per-drive
current directory: `D:relative` is rejected. Backslash is the only separator;
forward slashes, NUL/control bytes, repeated interior separators, dot and
dot-dot components are rejected. A trailing separator requires a directory
in the absolute-name API. Relative opens take an existing directory handle through
`open_relative`, do not interpret drive letters, and reject traversal.

## Filesystem implementations

`Backend: Send` exposes a cheap `read_only` property and root-relative
`stat`, `read_directory`, `read_at`,
`volume_info`, `create`, `write_at`, `set_len`, `set_basic_info`, `remove`,
`flush` and final-object `cleanup`, plus owned initial-allocation hooks.
Mutation defaults return `MediaWriteProtected`. VFS owns share
checks, access checks, file positions, handle lifetime and enumeration state.
Backend methods independently validate their paths. A custom backend can
implement the same interface without any guest-memory integration. Backends
are trusted primitives: their write/resize/remove methods do not recheck a
file's READONLY attribute; permission decisions belong to the VFS open and
disposition paths. Host OS permissions still apply.

`Vfs::format_device(device)` quick-formats one mounted device by exact name
through `Backend::format` (default `MediaWriteProtected`). The format fails
with `SharingViolation` while any file object or pending delete exists on
that volume. `HostBackend::format` swaps the data and metadata roots for
staged empty siblings by rename. It rolls back completed renames if a
swap fails and clears caches and pending metadata after both swaps succeed.
Cleanup of the old trees is best effort. A tree left aside stays outside
the guest namespace for the next mount to sweep.

`Vfs::guard_writes(scratch)` guards every mounted volume's writes while a
film plays, and `Vfs::end_guard` ends it. `HostBackend::guard_writes` first
lands queued writes and pending write metadata, clones the data and metadata
roots under `scratch` (`clone_or_copy`: shared blocks on APFS, a copy
elsewhere), builds a second backend of the same volume on the clones and
takes its place; the real backend is kept whole and does no I/O until the
guard ends. Reads after a write see it. `end_guard` drops the copy and its
directory and puts the real backend back, so a file created under the guard
does not exist after, and a dropped backend removes its copy. A backend with
nothing writable (the disc layers) has nothing to guard; `PackageBackend` and
`VariantFiles` forward the call and forget their open-file cache. Take the
guard with no guest running. Theater's slice 4
([theater-design.md](../game/theater-design.md#slices)) is its user.

On Unix, the format code in `crates/runtime/vfs/src/host_format.rs` holds
descriptors for each root and its parent from mount time. It identifies the
mounted root by device and inode, finding it under its new name if it moves
within that parent. If the root leaves the held parent, the format fails.
Staging, renames, rollback, directory sync, and cleanup use the held
descriptors. A moved ancestor therefore leaves format anchored to the
original parent. Format syncs the parent directories before cleanup,
which opens directories without following symlinks and unlinks the links
themselves. Windows uses paths for swaps and rollback within the app's
private directories and deletes old trees without following links inside
them.

`Vfs::device_of(name)` gives the mounted device a name finally resolves
into. XAPI uses it to distinguish the [Z: and N: cache volumes](xapi.md).

`DiscBackend<R: Read + Seek + Send>` uses `xiso::Xiso` to discover the game
partition and index directory records. `DiscBackend::open(path)` uses a
read-only host file; `new(Cursor<Vec<u8>>)` works for synthetic tests. Both
plain XISO and Redump offsets follow `crates/formats/xiso`. Reads seek directly to
`partition_offset + sector * 2048 + requested_offset`, and allocate no
full-file buffer. The original disc is never modified. Writes, creation,
truncation, deletion and writable opens fail with `MediaWriteProtected`.
The existing XISO API exposes directory/file kind and size, not all raw
attributes or timestamps: the VFS synthesizes READONLY/DIRECTORY attributes,
returns zero times and serial, and estimates volume extent from the indexed
records. Those values are not a physical DVD geometry or volume ID. Unaddressable
record names, and descendants of an unaddressable directory, are skipped
without failing the whole mount. Structural XISO corruption still fails.

`RewriteBackend::new(inner, rewrite)` serves some of another backend's files
with different contents (`Vfs::with_disc_rewrite` wraps the disc in one). For
a path the `Rewrite` wants, `Rewrite::rewrite_streamed` gets the inner file
as a `content::patch::Source` and reads what it needs. What it returns
(`vfs::Rewritten`) gives the file's size in `stat` and listings and its
bytes in `read_at`:

| Result | Held | Served | Outside the VFS (`file_source`) |
|---|---|---|---|
| `Whole(bytes)` | the bytes | from memory | `FileSource::Memory` |
| `Over(overlay)` | the overlay's runs | runs from memory, the rest from the inner file by offset (`Overlay::read_at`) | `FileSource::Spliced`: the inner file's host region with the overlay, or none without one |

| Rewrite | Returns |
|---|---|
| the default (fonts, mainmenu's level pictures) | reads the whole file, `Whole` |
| `workspace::PackageRewrite` on a map | `Over`: only what the package build changes ([studio-redesign.md](../editor/studio-redesign.md), "How the game consumes packages") |
| `Chain(a, b)` | `b` reads `a`'s output; two overlays compose into one (`Overlay::then`) |

There is no size limit: a map of any size takes its packages. A short
source read is rejected instead of rewriting a truncated file. A failed
inner read fails the `stat` or `read_at` with the inner backend's error.
The backend is read-only and never writes the inner one.

Idle level payloads are kept up to 64 MiB of held bytes
(`assets.rewritten_map_cache_bytes`); open levels and the shared maps are
kept apart. An evicted map keeps its size for listings and is rebuilt, or
read from the package cache, at its next read. Test:
`crates/runtime/vfs/src/rewrite.rs`
(`an_overlay_serves_the_unchanged_ranges_from_the_inner_file`).

### Map overrides

`MapOverrideBackend::new(inner, dir)` serves `maps\<name>.map` from
`<dir>/<name>.map` when the inner backend (the disc) has that map and the
directory holds a file of that name. Boot mounts the startup override
layer when `H2_MAP_DIR` names a directory. A writable full host can also
attach an editor override to its mounted DVD on the first file reload.
Both use the existing drive links and open-file table.
See `crates/extensions/game/src/map_override.rs`.

- Only names exactly one level under `maps` ending in `.map` are
  overridden, and only ones the disc has: a new name stays missing,
  everything else falls through. Names match ASCII case-insensitively on
  both sides, like the disc.
- An override's size is reported by `stat` and in listings of `maps`
  (sector-rounded allocation, the disc's READONLY attributes, zero times).
  `read_at` streams from the host file (seek and read, no buffering, no
  size cap below the 4 GiB XDVDFS limit). `host_file` gives the override
  file, so telemetry and the speaker index read the edited map.
- Listing the maps directory selects and caches the startup override just
  like the first open. Guest reads use the selected file descriptor across
  an atomic replacement. A name absent at selection stays on the disc. A paired
  editor can explicitly select another file with `map.reload {file}`.
  The first private generation reaps abandoned copies from dead processes.
  A worker prepares the generation, checks the slot type and
  capacity, parses the tag index and verifies the payload checksum.
  An RPC timeout releases the pending claim immediately. The worker checks
  for cancellation between copy and checksum chunks.
  An open guest file prevents reservation. After publication, size,
  bytes, host file and map identity come from the replacement. A private
  file stays available to native readers that hold its generation across
  another save. The live renderer waits for the guest to activate a
  published file before it opens that generation for the world
  (`crates/extensions/game/src/native_map.rs`). Publishing a replacement
  or shutting down the host releases the mount's reference. The last
  reader releases the private copy.
  Each rebind logs one line.
  A successful binding excludes external Live sign-in until restart.
  A bound editor file prevents machine snapshots and film keyframes.
  A server-map mount accepts only its startup inventory.
- Refused with a warning, falling back to the disc's map: a symlink whose
  target leaves the directory (a symlink inside it is followed), anything
  but a regular file, two host names that differ only in case, a file
  over 4 GiB, a file that cannot be opened. A missing or unusable
  directory is one warning and the disc throughout.
- Read-only: writes, creation, resizing and removal fail with
  `MediaWriteProtected`, and the directory is never written.

### Map packages

At boot, `boot::new_maps` scans `<data>/maps/` and registers a VFS
content path for each generated offer that fits in the multiplayer list.
The paths live under `T:\$c\<offer>\<name>.map`, with a placeholder
file on the data partition. A retail-based mod's generated name keeps
its path separate from the disc and Map Pack base paths.
[Generated offers](../game/maps-new-maps.md#package-offers)

The VFS resolves the manifest at the mod path against its base from the
disc, installed Map Pack or a local whole-map layer. PPF and IPS reads
stream byte runs over the base. BPS reads use the materialised
`<data>/maps/cache/<MapId>.map` file. The resolved size is available
through `stat`, and a streamed overlay can be read through the VFS even
though it has no contiguous host file. A damaged layer or mismatched
base fails the mod read. [VFS content maps](../../crates/runtime/vfs/src/content_maps.rs) ·
[Patch resolution](../game/maps.md#2-applying-layers-at-load)

Live installs and removals publish a new registry and trigger a content
scan on the next main-menu tick. An open file keeps its resolved bytes
until the last handle closes. The normal boot mounts the disc, campaign
additions, any explicit `H2_MAP_DIR` override and the font rewrite
independently of the generated offers. [Live refresh](../game/map-select.md#changes-during-a-session) ·
[Boot mounts](../../crates/extensions/game/src/map_override.rs)

`Vfs::with_disc_backend(config, backend)` is `with_disc` for a disc
backend built by the caller. Tests: `crates/runtime/vfs/src/map_override.rs` and
`crates/runtime/vfs/tests/map_override.rs` (sparse 300 MiB streaming, listings,
case, fall-through, symlinks, a no-match mount identical to `with_disc`,
and a model of the cache copy with its checksum).

`HostBackend::new(root, metadata_root)` presents a host directory as FATX.
The metadata tree must be separate, with neither root inside the other.
Each parent directory has a cached index of ASCII-insensitive names,
so lookup and create collisions behave consistently on APFS, case-sensitive
APFS and Linux filesystems. Original spelling is preserved. Two preexisting
host entries that differ only by case report `FileCorrupt` rather than
arbitrarily choosing one. Host symlinks, sockets/FIFOs/special files, files
over the FATX size limit, and (on Unix) multiply linked files are hidden from
listings; direct access to such objects fails. Names outside the supported
FATX rules are also hidden. These neighbours do not poison ordinary directory
queries or creation. Entry limits count eligible names/kinds without reading
sidecar metadata.

Mac host litter (`.DS_Store`, `._*`, `.Spotlight-V100`, `.fseventsd`) is reserved
and hidden at every level, including entire litter directory subtrees. It
does not count toward item limits or logical allocation. It cannot be
created or addressed through guest paths. Deleting a guest-empty directory
first removes these recognized litter entries, including their directory
subtrees. Litter symlinks are unlinked without following them. Other invisible
objects are left untouched and can still prevent physical removal; that
failure preserves the directory's logical metadata.

Use one VFS instance under a kernel mutex for all guest threads and one
backend per physical directory. Ordinary guest file operations resolve host
paths for each request and rely on the mounted paths staying stable. Share
reservations apply within a single VFS instance.
Directory indexes refresh on a parent-directory modification timestamp change
and are updated incrementally by VFS create/write/resize/remove. Repeated lookups do not enumerate
the parent again. Logical allocation is scanned once and updated incrementally
on successful creates, writes, resizes and removals. External changes can
stale the usage cache or same-timestamp indexes: recreate the backend after
editing the data root externally. This ownership constraint also avoids
share/identity races with host replacements.

### FATX rules and logical metadata

`open_allocated` and `open_relative_allocated` carry NtCreateFile's initial
allocation hint. HostBackend reserves cluster-rounded logical capacity on
create/overwrite without extending EOF. VFS passes a unique file-object identity
to `Backend::create_allocated` / `overwrite_allocated`, then calls
`release_allocation` when that object's last handle closes. Independent opens
cannot trim another object's reservation; duplicates share the owner's lifetime.
Writes consume the reservation; EOF changes or its owner's release trim unused
capacity. Ordinary `cleanup` still handles flushing and timestamps separately.
An ordinary existing-file open ignores the hint. Capacity failures precede
creation/truncation. These are session
reservations, not physical host disk preallocation. See [checkpoint acceptance](checkpoints.md).

The FATX on-disk directory record has a 42-byte name and a 32-bit size; see
[XboxDev's FATX definitions](https://github.com/XboxDev/fatx/blob/master/libfatx/fatx.h)
and [raw record layout](https://github.com/XboxDev/fatx/blob/master/libfatx/fatx_internal.h).
VFS enforces 1–42 ASCII bytes per component, including the extension, and
`u32::MAX` bytes per file (4 GiB minus one byte). **The 42-byte limit does not
apply to the XDVDFS disc backend**; its directory records have an 8-bit name
length, and VFS reports a 255-byte maximum there.

The supported printable ASCII set excludes `" * + , / : ; < = > ? \ |`.
In particular, **`+ , ; =` remain invalid**. This follows the actual
`FatxIllegalCharTable` bitmap in [nboxkrnl's original-Xbox FatxIsNameValid](https://github.com/ergo720/nboxkrnl/blob/master/nboxkrnl/io/hdd/fatx.cpp#L164),
not its incomplete comment: word `0xFC009C04` sets the bits for these four
characters as well. A regression decodes that bitmap across all printable
ASCII characters. The [FATXplorer author](https://fatxplorer.eaton-works.com/fatx-file-system-limitations-reference)
also states that its validation follows Xbox/Xbox 360 rules. The looser
[OXOS validator](https://github.com/kooscode/oxos/blob/main/kernel/src/filesystems/hddfs/volume.cpp#L296)
disagrees; it is not used as evidence for broadening this character set.
The earlier Velocity-only citation has been replaced with original-Xbox
evidence. Control characters and non-ASCII bytes remain unsupported.
Names preserve trailing spaces and periods; they are not Windows-normalized.
`.` and `..` cannot be stored. Xbox object strings are byte strings; complete
OEM/codepage decoding and uppercasing of bytes above 0x7f is not yet modeled.
Rejecting them is an explicit compatibility limit, not a claim that Xbox
uses Unicode or that every high byte is illegal on hardware.

The emulation also limits a volume-relative path to 250 bytes
(excluding device prefix) and a directory to 4,096 visible items. Public
references disagree about full-path and per-directory limits across Xbox
versions/tools. These are explicit VFS policies based on the
[FATXplorer author's limitations reference](https://fatxplorer.eaton-works.com/fatx-file-system-limitations-reference),
not a claim that every original Xbox kernel enforces identical limits.
Limits include expanded TDATA/UDATA prefixes, so aliases cannot bypass them.

Metadata uses NT FILE_ATTRIBUTE bit values: READONLY `0x01`, HIDDEN `0x02`,
SYSTEM `0x04`, DIRECTORY `0x10`, ARCHIVE `0x20`; NORMAL `0x80` is accepted as
input and has no stored bit. Unsupported attributes fail. DIRECTORY is
structural and cannot be toggled via basic information. New files have
ARCHIVE; successful writes/truncation set it. READONLY blocks writable opens
of existing files and new delete requests. It does **not** revoke rights already granted to
an open handle: creating a new readonly file with WRITE and then writing,
or setting READONLY after opening a writer, both work. WRITE_ATTRIBUTES can
clear the flag. Readonly directory attributes do not gate child creation.
These are logical attributes, independent of host POSIX permission bits; a host permission
failure still propagates. The supported original-Xbox attribute set is also
listed by the [FATXplorer author](https://fatxplorer.eaton-works.com/fatx-file-system-limitations-reference).

`Timestamp(u64)` is NT time (100 ns since 1601 UTC). FATX's packed timestamp
format uses a 2000 epoch and two-second precision, as shown in the
[XboxDev FATX date/time definitions](https://github.com/XboxDev/fatx/blob/master/libfatx/fatx_internal.h).
VFS rounds down to two seconds and clamps to 2000–2127, using UTC so behaviour
is deterministic across host timezones. Creation/access/write times persist;
change time is an additional emulated field. Reads update access time in
memory and coalesce it per path; the record is persisted on explicit flush,
file-object cleanup/close, a later metadata write, or backend drop. This
avoids a write+fsync per read. Write/truncation updates write/change times
and sets the archive bit. Those stay pending in memory too ("Write
metadata" below); `stat` sees them at once. Imported host files start
from host timestamps; subsequent VFS timestamps are logical, not a promise
to rewrite host birthtime or mtime.

### Write metadata

A write or truncation does not store the metadata record on the spot. Detail: [Xbox storage: write metadata and write-behind](storage-writes.md).

### Write-behind

All guest threads run on one host thread, so a host `write` stalls the whole game, not just the guest thread that issued it. Detail: [Xbox storage: write metadata and write-behind](storage-writes.md).

## File objects and NT behaviour

The numeric access/share/disposition constants agree with [nxdk's public Xbox kernel header](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.h). Detail: [Xbox storage: file objects and kernel integration](storage-nt.md).

## XAPI save containers

The filesystem permits XAPI's save layout without implementing XCreateSaveGame,
XFindFirstSaveGame, XDeleteSaveGame or save signing. The `xapi` crate's opt-in
native save games (docs/runtime/xapi.md, "Opt-in native save games") do implement
them over this VFS: a save directory is the 12-hex-digit name hash holding
`SaveMeta.xbx`, in the host `UDATA` title directory. T: maps title data such as
configuration/DLC, and U: maps user saves. Each title owns an eight-hex-digit
directory in both areas. A representative layout is:

```text
E:\
  TDATA\4D530064\                 (T:\)
    <title-defined files>
  UDATA\4D530064\                 (U:\)
    TitleMeta.xbx
    TitleImage.xbx
    SaveImage.xbx                 (optional shared save image)
    <save-directory>\
      SaveMeta.xbx
      SaveImage.xbx               (optional per-save override)
      <game-defined payload>
```

TitleMeta.xbx describes the title (`TitleName=...`); SaveMeta.xbx describes a
save (`Name=...`). Metadata is text, commonly UTF-16LE; a save scanner should
also account for ASCII and localisation sections, as demonstrated by the
[OGXBL homebrew metadata reader](https://github.com/OGXBL/XBLCloudGameSave/blob/main/README.md#save-metadata).
TitleImage.xbx and SaveImage.xbx are Xbox texture resources. A save-specific
image overrides the shared image in the title directory. This organisation
and image precedence are described in the
[Xbox savegame format notes](https://xboxdevwiki.net/Xbox_Savegame_System).
VFS only creates the TDATA/UDATA title roots: it does not invent save IDs,
write metadata/images, decode textures, verify signatures or erase existing
containers. These files are ordinary byte payloads to the filesystem.

## Validation

`cargo test -p vfs` builds all its own fixtures. Host tests create private
RAII temp directories under TMPDIR and delete them afterwards; synthetic
XISO tests build a small in-memory image, including a disc name longer than
42 bytes. Tests exercise every disposition for existing/missing files,
case lookup/collisions, aliases, shares, directory search/restart/retry,
duplicate positions, EOF/append/growth, persistent attributes/times, deletion,
quota, directory count limits and host link rejection. Review regressions add
readonly create/existing-handle rights, buffering hints, damaged records and
unrepresentable neighbours, host litter, metadata retirement failure, deferred
atime and scan-count assertions, cleanup-time deletion, supersede permissions,
original-Xbox punctuation, mask semantics, invalid disc names and low-level
status/configuration cases. Re-review tests cover litter-only rmdir through
guest handles, preserved attributes on failed rmdir, non-followed litter
symlinks, orphan metadata on recreation, conditional cleanup/close errors,
and injected post-create sync failure. No game data is
needed by the default suite.

The ignored `real_disc_lists_d_root` test opens the user's existing
`disc/halo2.iso` (or H2_DISC) and prints only the top-level names. Run with
`cargo test -p vfs real_disc_lists_d_root -- --ignored --nocapture`. It does
not extract anything. Builds can use `CARGO_TARGET_DIR="$TMPDIR/vfs-target"`
to keep scratch output outside the shared checkout.
