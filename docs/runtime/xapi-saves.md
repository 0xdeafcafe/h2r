# XAPI: saves and the N: volume

Status: Built. Generated saves and config, opt-in native save games, the second cache volume. Part of [XAPI boundary](xapi.md).

## Generated saves and config

The ignored [checkpoint acceptance harness](checkpoints.md) executes these
generated save APIs, signatures and the game's checkpoint writer/revert/restore
against synthetic state and private host storage, including a fresh Runtime.

XCreateSaveGame at `00330df2` retains the original UTF-16 name hash and ordinary
file operations. Metadata emission at `00330f8f` writes UTF-16LE: code unit FEFF
(bytes FF FE), then `Name=<name>\r\n`, followed by `NoCopy=1\r\n` when bit 0 is
set. The relevant strings are at `00416fb8`, `00417020`, `00417070`, `00417078`.
No Rust Storage contract duplicates this format; saves are generated.

The scanner at `00330c15`, first/next at `00331125` / `0033122f`, and shared
close at `00331276` own save metadata, search lifetime and buffer packing.
XGAME_FIND_DATA has a 320-byte WIN32_FIND_DATAA prefix, an ANSI directory at
+320 (260 bytes), and UTF-16 name at +580 (128 units): 836 bytes total. Preserve
NUL termination for both find-data name arrays and the added strings; no raw
Rust prefix is accepted anymore. Empty/end enumeration must return error 18
through the generated file/search path. Retain generated XCalculateSignature*
and the kernel crypto HLE; this crate supplies no signing keys.

XGetLanguage at `0033039a` queries nonvolatile index 7, returns zero on query
failure, preserves values 0..9, and maps values above 9 to 1 (`003303b9`).
XGetVideoStandard at `00330310` queries index 0x103 and returns bits 8..15.
XGetVideoFlags at `00330339` queries index 8 and starts with
`(value >> 16) & 0x5f`; only AV pack 1 with standard !=3 retains every flag,
otherwise apply `flags & ~0x0e`. XGetAVPack at `003321bd` dereferences imported
HalBootSMCVideoMode; XGetGameRegion at `00331385` queries index 0x104 and
XGetParentalControlSetting at `002d5813` queries index 10. These derivations
remain in the generated originals; Config has no competing locale/video fields.

### Profile creation errors

The saved-game files code leaves an error in `[0x55c154]` and `0x148ca8`
shows it as a ui error (the `wigl` error table): 1 → `0x4a` (OUT OF BLOCKS,
"no free slots"), 2 → `0x4e` (OUT OF BLOCKS, "need more space"), 4 → `0xc`
(CAN'T CREATE PROFILE, whose text is "You have reached the maximum number
of profiles..."). So that dialog is error 4, not a count:

- Error 1 is a count: `0x215f40` probes `Profile: Halo%04d` names with
  XCreateSaveGame OPEN_EXISTING up to 0x1000, and `0x215c00` refuses when
  the saves counted by `0x217590` (XFindFirst/NextSaveGame) or the cached
  list (`[0x51ea14]+0xbef8`) reach 0x1000.
- Error 4 is a failed file call with free space left. `0x215c00` creates
  the save (XCreateSaveGame CREATE_NEW at `0x215cde`, then the content
  file with CreateFile CREATE_ALWAYS through `0x1367d0`); on a failure it
  deletes the save again and asks `0x216ce0` (GetDiskFreeSpaceEx) whether
  0x1e0 blocks fit: yes gives 4, no gives 2. The new profile's first write
  (`0x1a0511` → `0x216240` → the task at `0x2acde0`, `0x2ae090`) opens the
  content file OPEN_EXISTING, GENERIC_WRITE, share 0 (`0x136970`); failing
  that open leaves 4 too, and `0x1a051c` deletes the save.

### Damaged save metadata

XCreateSaveGame opens `SaveMeta.xbx` with FILE_OVERWRITE_IF and then
writes it, so a process killed in between leaves it empty or torn. The
scanner then skips the directory (the profile is not listed), and making
the profile again finds the directory with no matching `Name`, which the
game shows as error 4 (CAN'T CREATE PROFILE). A meta file is damaged when
it is not UTF-16LE with the `FF FE` mark and a `Name=` line (`TitleName=`
in `TitleMeta.xbx`); the game never writes one so.

`vfs::Vfs::with_storage` mends U: before mounting it
(`crates/runtime/vfs/src/save_meta.rs`): for each `<12 hex digits>` directory
whose `SaveMeta.xbx` is present and damaged, it looks in the directory's
other files (first 1 MiB of each) for a UTF-16LE or 8-bit run whose name
hash (`003309b8`) is the directory's name. That is the save's own name,
so the file is rewritten as the game would (`FEFF`, `Name=<name>` CR LF;
a `NoCopy=1` line is not recovered). With no such run the directory moves
to `<data root>/damaged-saves/<unix ms>-<dir>`, so the profile can be made
again. Each repair is a `vfs:` line on stderr. Cloud saves never upload a
damaged meta file nor write one downloaded
(docs/live/live-cloud-saves.md, rule 6).

## Opt-in native save games

`Config::save_games` (default false) makes `register` also install four
overrides, `register::SAVE_OVERRIDES`. With it off nothing changes: the five
generated routines run, `Xapi::invoke` answers `Outcome::Original`, and
`OVERRIDES` stays the 16 above. They exist so saves can be exercised and
inspected without the generated scanner, and as a reference for the format.

Call sites (found by scanning the image for `call rel32`): XCreateSaveGame
`0x215cde`, `0x216018`, `0x2161ad`, `0x2ad0eb`; XDeleteSaveGame `0x215e29`,
`0x215f20`, `0x2ad4ff`, `0x2ad58a`; XFindFirstSaveGame `0x216537`, `0x2175e8`;
XFindNextSaveGame `0x216736`, `0x2175fb`; XFindClose `0x08d677`, `0x216748`,
`0x217605`. XDK 5849 statically links these, so there is no import slot.

| Address | Function | Frame | Override |
| --- | --- | --- | --- |
| `00330df2` | XCreateSaveGame(root, name, disposition, flags, path, path_len) | S24 | yes, returns the Win32 error (0 = success) |
| `00331055` | XDeleteSaveGame(root, name) | S8 | yes, same return |
| `00331125` | XFindFirstSaveGame(root, XGAME_FIND_DATA*) | S8 | yes: search block, or -1 with last error |
| `0033122f` | XFindNextSaveGame(search, XGAME_FIND_DATA*) | S8 | yes: BOOL, last error 18 at the end |
| `00331276` | XFindClose(handle) | S4 | no: shared with every find family |

What the original does, which the overrides reproduce:

- The save directory is `root` + 12 uppercase hex digits (`003309b8`):
  `h = (h * 0x10000 + unit) % 0xFFFFFFFFFFC5` over the UTF-16 name units,
  printed as the 48-bit value. `root` is the caller's string, normally `U:\`;
  no separator is added. Names over 127 units are rejected (87) where the
  original overruns its buffers.
- `SaveMeta.xbx` is `FEFF`, `Name=<name>` CR LF and, when flags bit 0 is set,
  `NoCopy=1` CR LF, in UTF-16LE (`00330f8f`). The reader (`00330a92`)
  discards the first two bytes, reads `key=value` lines and compares `Name`
  to the requested name exactly (`00330d16`: the open error, or 0x491 on a
  mismatch).
- Create: OPEN_EXISTING (3) only checks the name. Otherwise it creates the
  directory, then the meta file; a failure removes the directory again
  (`00331042`). An existing directory whose meta names this save has its
  last-write time touched (`00330d96`) and gives 183 for CREATE_NEW (1),
  success otherwise; one that does not gives 0x52. On success `path` gets
  `root + hash + "\"`, at most `path_len` bytes (backslash only if it fits).
- Delete checks the name, removes the directory tree (`002d6989`) and returns
  the Win32 mapping of the result.
- Find: the scanner (`00330c15`) accepts a directory whose `SaveMeta.xbx` has
  a `Name`, and fills XGAME_FIND_DATA: attributes 0x10, the times and
  cFileName in the WIN32_FIND_DATAA prefix, `root + dir + "\"` at +320 and
  the UTF-16 name at +580. Entries come in the VFS's name order. A first
  search with no hit returns -1 with last error 18.
- The search object XFindFirstSaveGame returns is a 0x110-byte block from the
  game's allocator (`00012280(0x110, 0x24830000)`): signature `EVBS`
  (`0x53425645`) at +0, a handle word at +4, root length at +8, root string
  at +0xC. The generated XFindClose frees it (and passes the handle word to
  CloseHandle), so XFindClose is not overridden. The override reaches the
  allocator with `ExecCtx::call_guest`, then makes the handle word with a
  guest call to NtCreateEvent (a real kernel handle, so the generated
  CloseHandle succeeds), and writes the block in the continuation; a failure
  of either gives -1 and last error 8. Retail keeps a live FindFirstFile
  handle at +4; here a host table (`Xapi::adopt_search`, at most 64, oldest
  dropped) maps that handle to the listing snapshot taken at
  XFindFirstSaveGame and a cursor, so saves created or deleted mid-search
  neither repeat nor skip entries. A deleted save in the snapshot is skipped
  when its meta no longer reads.
- Output path: the guest's bounded copy (`002d1404`) always NUL-terminates; a
  directory that fills `path_len` loses its last byte to the NUL. A failed
  timestamp touch is ignored (`00330d96` is void).

Not reproduced: the original's substring key search (a `TitleName=` line would
match `Name`), and FATX timestamp details beyond what the VFS applies. The
original calls no XCalculateSignature for saves, so there is no signature or
hash check beyond the name hash above.

Storage: `Storage::saves` returns an optional `SaveStore`; `VfsStorage`
implements it over the shared `Vfs`, so `U:\` is the host directory
`<data root>/disk/Partition1/UDATA/<title id>` (the VFS `Config::data_root`,
by default `~/Library/Application Support/h2recomp`, never the repository).
A `Storage` without it makes the four calls fail with 50. Tests:
`crates/platform/xapi/tests/saves.rs` (helpers, the adapter over a private temp-dir
VFS, registration, the runner with a stand-in allocator) and
`tests/save_pins.rs` (`H2_XBE`, ignored: decoded instructions only).

## The second cache volume, N:

Halo has two HDD cache volumes. XapiInitProcess mounts Z: (`002d1c40 ->
002d5b77`). Game bootstrap then mounts N: with `0x00012003 -> 002d5d0a`,
whose result it ignores. `cache%03d.map` slots 0-1 live on Z: and slots 2-5
on N:. Slot 2 is the mainmenu resource source (`00214cc0`), so the menu needs
a writable N:. `002d5d0a` is stdcall with no arguments. It ends in a plain
`ret` at `002d5f64` and returns BOOL in EAX. These are its steps, read from
the instructions with `h2 disasm`; the import ordinals come from the XBE's
kernel thunk table:

1. `[00461d80]` is Z:'s database row. If it is -1, it calls
   XapiSetLastNTError(STATUS_UNSUCCESSFUL) and returns FALSE (`002d5d21`).
   N:'s row `[00461d84]` is not written.
2. It opens `\Device\Harddisk0\partition0` (NtOpenFile 202) and reads 512
   bytes at 0x800 (NtReadFile 219). The signature `97315286` at +0, version 2
   at +4 and marker `aa550000` at +0x1f8 are all checked. If any fails, the
   status is STATUS_INTERNAL_DB_CORRUPTION.
3. Rows are 12 bytes at +8: title, partition index (`partition - 3`) and a
   valid flag. `n = min([[00411610]], 41)`, where `00411610` is the import
   slot of HalDiskCachePartitionCount (ordinal 40). The candidate is the
   highest index below `n` that is not Z:'s and that no valid row holds
   (`002d5e01..002d5e2a`).
4. N:'s row is `count - 1`, or `count - 2` if that row is Z:'s. It is stored
   at `002d5e38`. If no partition was free, N: takes the partition that row
   already names. Otherwise the row gets the candidate. The row's title
   becomes the certificate title (`[[00010118] + 8]`) and its valid flag 0.
   The sector is written back (NtWriteFile 236), and the status is ignored.
5. It formats `\Device\Harddisk0\Partition<n>` through `002d6c04`, which uses
   NtDeviceIoControlFile 196 via ESI with `0x70000` and `0x74004`, then
   NtFsControlFile 200 with `0x90020`, and sets the last error itself on
   failure. The cluster size comes from init flags bits 30-31: 64 KiB for
   Halo. `002d15f0` verifies it, and then IoCreateSymbolicLink 67 creates
   `\??\N:` at `002d5f2a`.
6. Any failure after step 1 stores -1 at `00461d84` (`002d5f44`, `002d5f52`),
   sets the last error and returns FALSE.

`00461d84` is referenced only inside this function. The native override
(`Function::XMountCacheDrive`, `crates/platform/xapi/src/cache.rs`) keeps steps 1-4
and 6 exactly. The `Storage` trait supplies the database and the volume:

- `read_cache_database` / `write_cache_database` operate on the kernel's raw
  disk. `crates/runtime/kernel/src/rawdisk.rs` owns that disk and is only read here.
- `mount_cache(partition)` stands in for step 5. "Format" means emptying the
  partition's host directories (`vfs::Vfs::format_device`), and then N: is
  linked to the partition. The cluster check is skipped, because host volumes
  report 16 KiB clusters.
- `Config::cache_drive.partition` is `CachePartition::Select` (the retail
  rule above) or `Fixed(3..=5)`. With the kernel's seed (this title on
  Partition5, row 0, count 3), Select gives Partition4, row 2.
- `Config::cache_drive.globals` (`CacheGlobals`) holds the four guest
  addresses. Its defaults are the retail values in `register::addr`.

Native bounds that differ from retail: a partition count below 2 or above 41,
a Z: row outside the table, or a choice equal to Z:'s partition (from the
database or `Config::utility_partition`) fails with ERROR_GEN_FAILURE (31)
and the -1 slot, where retail would index past the table. Last errors are
31 (STATUS_UNSUCCESSFUL), 1358 (STATUS_INTERNAL_DB_CORRUPTION) and the
storage's own Win32 code. The kernel's `dos.rs` table has neither of those
two NTSTATUS values. The generated original would stop in
RtlNtStatusToDosError on these failure paths; the override does not call it.
A storage without the new methods (their defaults return 50) gets FALSE and
-1, and N: is missing.

`xapi::VfsStorage` (`crates/platform/xapi/src/vfs_storage.rs`) implements the whole
`Storage` trait over the kernel's one `Arc<Mutex<vfs::Vfs>>`. Z: and N: are
links to `\Device\Harddisk0\Partition<n>`, and a remount replaces the link.
Neither letter can be put on, or format, the volume the other resolves to.
Volumes are compared with `Vfs::device_of`, so an alias chain or a link
into a subdirectory counts. Either format fails with 32 while a file on that
volume is open. So N:'s format cannot empty Z:, and XFormatUtilityDrive
cannot empty N:. `HostBackend::format` swaps each root directory for a
staged empty sibling by rename and rolls back completed renames if a swap
fails. On Unix the format follows the mounted roots when their parents
move or when the roots are renamed within those parents. It uses directory
descriptors for the swaps and cleanup, so symlinks cannot redirect either
operation. See [filesystem implementations](storage.md#filesystem-implementations).
