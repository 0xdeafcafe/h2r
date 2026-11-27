# Xbox storage: file objects and kernel integration

Status: Built. File objects, NT behaviour and the kernel entry points. Part of [Xbox storage boundary](storage.md).

## File objects and NT behaviour

The numeric access/share/disposition constants agree with
[nxdk's public Xbox kernel header](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.h).
`OpenOptions::from_nt` expands generic rights and decodes supported creation
flags. It validates share bits, mutually exclusive directory/type and sync
options, SYNCHRONIZE for synchronous opens, and DELETE for delete-on-close.
FILE_WRITE_THROUGH (`0x2`) and FILE_NO_INTERMEDIATE_BUFFERING (`0x8`) are
accepted as hints without changing the public options struct. Kernel code
must enforce guest alignment if required; VFS byte-slice operations remain
buffered/synchronous, and these hints do not promise per-write host durability.
Use `flush` for durable completion. Open-by-ID and reparse-point flags remain
`NotSupported`. Accepted hints do not provide asynchronous I/O, host ACLs,
caching guarantees or a guest event implementation.

| Disposition | Missing leaf | Existing file | Success information |
| --- | --- | --- | --- |
| FILE_SUPERSEDE (0) | Create | Truncate and replace attributes | Created (2) / Superseded (0) |
| FILE_OPEN (1) | ObjectNameNotFound | Open unchanged | Opened (1) |
| FILE_CREATE (2) | Create | ObjectNameCollision | Created (2) |
| FILE_OPEN_IF (3) | Create | Open unchanged | Created (2) / Opened (1) |
| FILE_OVERWRITE (4) | ObjectNameNotFound | Truncate, add requested attributes | Overwritten (3) |
| FILE_OVERWRITE_IF (5) | Create | Truncate, add requested attributes | Created (2) / Overwritten (3) |

A missing parent always fails; VFS does not implicitly create trees. Create
options select the directory/file type, not the DIRECTORY attribute input.
Truncating a directory fails. Overwrite requires WRITE. Supersede of an
existing file no longer requires the caller to explicitly request DELETE.
The operation checks implied DELETE against existing share reservations,
then grants only the requested handle rights. Supersede of a missing leaf
creates it without implied deletion. This follows FastFat's distinction
between operation access and granted access, not a claim that retail Xbox
behaviour has been measured: `FatOpenExistingFcb` records `AddedAccess`, adds
DELETE for supersede, checks access/sharing, then clears `AddedAccess` before
`IoSetShareAccess`; `FatOpenExistingFile` follows the same pattern.
See [Microsoft FastFat create.c, pinned revision](https://github.com/microsoft/Windows-driver-samples/blob/96eb96dfb613e4c745db6bd1f53a92fe7e2290fc/filesys/fastfat/create.c#L3332)
and [removal of implied access before share registration](https://github.com/microsoft/Windows-driver-samples/blob/96eb96dfb613e4c745db6bd1f53a92fe7e2290fc/filesys/fastfat/create.c#L3775).
A WRITE-only supersede therefore succeeds when deletion is permitted by
existing shares, but its resulting handle cannot set delete disposition.
The previous explicit-DELETE rejection was too strict. Supersede is still
replacement in place, without a detached old inode or ACL replacement.

Each independent open acquires a share reservation for data-read/execute,
data-write/append and delete access. Existing share flags must permit the new
access, and the new share flags must permit every existing access. Checks
happen **before truncation**. Metadata-only handles do not acquire data
read/write reservations. All drive aliases use the resolved mount and
lowercase volume path as identity. Successful opens return a `Handle` and
`CreateInformation`; the latter is already the numeric IO_STATUS_BLOCK
Information value, not an NTSTATUS.

`duplicate` references the same file object: byte position, search expression,
search cursor and share reservation are shared. A separate `open` creates a
new position/search cursor. `close` frees the reservation when the last
handle to its object closes. Handles are opaque Rust values; kernel handles
must live in the kernel object table. An identifier is valid only in the VFS
instance that created it. Handle values are opaque and never reused within
that VFS instance, including after close. Do not serialize or reconstruct them.

`read`/`write` return the actual transferred byte count. Operations are
synchronous; Current and Absolute offsets update the shared position after
success. APPEND-only rights force writes to EOF regardless of supplied
offset. `Offset::EndOfFile` requests append for a writable handle; it is
invalid for reads. Negative NT sentinel offsets must be translated by the
kernel; ordinary offsets must fit signed LARGE_INTEGER. A nonempty read at
or beyond EOF returns EndOfFile; a read crossing EOF returns the available
prefix successfully. A zero-length read succeeds with zero bytes. EOF resize
does not move the position. Position behaviour follows the synchronous model
in [Microsoft's read contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-zwreadfile).

`set_delete_pending` requires DELETE and rejects read-only files, volume roots
and nonempty directories. An explicit disposition request becomes pending
immediately and can be cancelled before cleanup. FILE_DELETE_ON_CLOSE instead
records an intent on its file object: other compatible opens are allowed
until its last duplicated handle closes and cleanup runs. Cleanup makes the
pending request sticky while other file objects remain; a different object
cannot cancel that completed cleanup by setting disposition false. Pending
paths reject new opens (including beneath pending directories); existing
handles remain usable until all file objects for the path close. This file-object distinction follows [Microsoft's delete-on-close explanation](https://devblogs.microsoft.com/oldnewthing/20160108-00/?p=92821).
The last close removes the host entry. Final-object cleanup uses
`Backend::cleanup(path, write_access)`. Its default calls `flush` only for
file objects opened with WRITE/APPEND; directory and read-only opens avoid
unconditional sync. HostBackend also tracks dirty data and deferred
atime: atime-only cleanup persists its record without syncing file data;
write-capable or data-dirty cleanup syncs file data and persists the record
any write left. Duplicate-handle closes
do not trigger cleanup until the last reference closes. A cleanup-sync
failure is logged to stderr with a `vfs:` prefix and does not turn an already
closed handle into a failed close. Actual removal errors still propagate.
Call `flush` before close to receive a durability failure synchronously.
Session drop is best effort. There is no crash-time delete replay.

Directory enumeration is a sorted, case-insensitive snapshot per file object,
without `.`/`..` entries. The first expression is captured; later expressions
are ignored, including on restart. Restart rebuilds and rewinds the snapshot.
`*` and `?` work case-insensitively; `?` consumes exactly one byte and a dot
is literal. Consequently `ab?` does not match `ab`, and `a*.*` does not match
`abc`. This kernel-facing behaviour follows [OXOS's original-Xbox matcher](https://github.com/kooscode/oxos/blob/main/kernel/src/filesystems/hddfs/volume.cpp#L327).
Cxbx [forwards the mask to NtQueryDirectoryFile](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlNt.cpp#L1197),
with a specific `*.*` to `*` rewrite that VFS retains, so `*.*` does include
extensionless names. These implementation sources do not establish broader
Win32 DOS wildcard translation on original hardware. In particular,
[nboxkrnl's FATX driver table](https://github.com/ergo720/nboxkrnl/blob/master/nboxkrnl/io/hdd/fatx.cpp#L78)
registers `IopUnimplementedDeviceRequest` for IRP_MJ_DIRECTORY_CONTROL; that
source has a name validator, but no FATX enumeration/name matcher to cite.
**Kernel integration compatibility note:** VFS's `a*.*` will not match `abc`.
If retail Xbox applies DOS-style semantics that make this a match, the
current VFS mask behaviour will miss the file and needs a hardware-backed
matcher correction; desktop forwarding or a reimplementation does not prove
retail behaviour. DOS_STAR, DOS_QM and DOS_DOT encodings remain unsupported; callers must not silently translate
ordinary `?` into DOS_QM based on desktop FindFirstFile behaviour. An absent
or initially empty expression means `*`. Initial no-match returns
NoSuchFile; exhaustion returns NoMoreFiles. See the expression/cursor model
in [Microsoft's directory query contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntquerydirectoryfile).

Use `peek_directory` to inspect an entry without consuming it, pack the guest
record, then `consume_directory` after it fits. A buffer-too-small retry must
not consume or restart the cursor. `query_directory` combines those calls for
callers needing one complete entry. VFS does not know the guest buffer size,
information class or NextEntryOffset layout. Querying a file as a directory
returns InvalidParameter; using a file as an intermediate path component
returns ObjectPathNotFound (an explicit directory-type open on that same
final file still returns NotADirectory).

## Kernel integration

The kernel depends on `vfs` and translates the NT file exports onto it; the
boot runner configures it (docs/runtime/BOOT.md, `--data`).

`Vfs::set_open_observer` is told about every file (not directory) opened,
under the VFS lock, with `OpenedFile::host`: where its bytes sit on the host
(`Backend::host_file`: an entry's region of the disc image, a hard-disk file,
`None` for a rewritten one). `Vfs::host_file(name)` answers the same for any
name. The controller-speaker sound index uses both to read the maps the game
loads ([playstation-audio.md](../audio/playstation-audio.md)).
`Vfs::set_change_observer` is told when a change is finished: `ChangeKind::Written`
at the last close of a file object that created, overwrote, wrote or
truncated a file (after its cleanup, so the bytes are on the host), and
`ChangeKind::Removed` when a deletion is carried out. Cloud saves watch U:
with it ([live-cloud-saves.md](../live/live-cloud-saves.md)). Basic setup:

```rust
use vfs::{Config, OpenOptions, Offset, Vfs};

let config = Config::for_title(0x4D530064)?;
let mut storage = Vfs::with_disc(config.clone(), "disc/halo2.iso")?;
storage.create_standard_links(&config)?; // host tool only; on the boot path XAPI creates D:
let opened = storage.open(r"D:\default.xbe", OpenOptions::default())?;
let mut header = [0u8; 64];
let transferred = storage.read(opened.handle, Offset::Current, &mut header)?;
storage.close(opened.handle)?;
```

| Xbox export / operation | Rust API |
| --- | --- |
| NtCreateFile / NtOpenFile | `OpenOptions::from_nt`, `open` / `open_relative` |
| NtReadFile / NtWriteFile | `read` / `write`, translating ByteOffset to `Offset` |
| NtClose / NtDuplicateObject | `close` / `duplicate` |
| NtQueryInformationFile | `query_information` returns basic/standard/position/access/name data |
| NtQueryFullAttributesFile | `query_path` |
| NtSetInformationFile EOF / position / basic / disposition | `set_end_of_file`, `set_position`, `set_basic_information`, `set_delete_pending` |
| NtQueryDirectoryFile | `peek_directory` + `consume_directory`, or `query_directory` |
| NtQueryVolumeInformationFile | `query_volume_information` |
| NtFlushBuffersFile | `flush` |
| IoCreateSymbolicLink / IoDeleteSymbolicLink | `namespace.create_symbolic_link` / `delete_symbolic_link` |

The kernel validates guest buffers **before** operations with effects (file
creation, I/O, cursor consumption), maintains guest object references, and
marshals Xbox-width structures. Validate per-information-class rights at that
boundary; `query_information` is a combined typed snapshot, not an unchecked
implementation of every class. `set_basic_information` replaces a full
`BasicInfo`; merge NT zero/-1 unchanged-time sentinels and unchanged attributes
with current values before calling it. Pack Xbox names as byte strings, not
Windows UTF-16 names. Async completion, IRPs, events/APCs, notifications, raw
sector/device IOCTLs, file IDs, rename, EAs, oplocks,
byte-range locks, volume formatting and ACLs are not implemented here.

`Backend::cleanup(&mut self, path: &str, write_access: bool) -> Result<()>`
handles deferred metadata independently of `release_allocation(path, owner)`.
The initial-allocation methods receive that same owner identity; backend
wrappers implementing reservations must forward it and its release to their
inner backend. Default methods reject unsupported nonzero reservations before
mutation. VFS logs cleanup/release errors separately from close. Existing
open/close signatures, OpenOptions and Handle are unchanged; allocated-open
variants carry the initial allocation hint.
`set_basic_information` remains full replacement and must be merged by the
kernel; `Handle` remains opaque and nonreusable. Rename is still unavailable.
FileRenameInformation within a volume is a useful next addition for XAPI
MoveFile/save handling, but should be designed with handle/share identity,
replacement rules and metadata movement together; it is not emulated by
unlink/copy in this patch.

Every failure is a typed `vfs::Error`; `error.ntstatus()` is the numeric status
for the guest. Success is STATUS_SUCCESS (`0`), with byte count or create
information written separately. Storage errors include:

| Error | NTSTATUS |
| --- | --- |
| InvalidHandle / InvalidParameter | `C0000008` / `C000000D` |
| NoSuchFile / EndOfFile | `C000000F` / `C0000011` |
| AccessDenied | `C0000022` |
| ObjectNameInvalid / ObjectNameNotFound / ObjectNameCollision | `C0000033` / `C0000034` / `C0000035` |
| ObjectPathNotFound / SharingViolation / DeletePending | `C000003A` / `C0000043` / `C0000056` |
| DiskFull / MediaWriteProtected | `C000007F` / `C00000A2` |
| FileIsADirectory / NotSupported | `C00000BA` / `C00000BB` |
| DirectoryNotEmpty / FileCorrupt / NotADirectory | `C0000101` / `C0000102` / `C0000103` |
| IoDeviceError / ReparsePointNotResolved / FileTooLarge | `C0000185` / `C0000280` / `C0000904` |
| NoMoreFiles (warning severity) | `80000006` |

Host errors are mapped by category; unexpected I/O becomes IoDeviceError,
malformed XISO becomes FileCorrupt. The typed error does not retain a private
host path. Partial host writes return their transferred count. A metadata
failure after a data write can report failure after data has changed; this
backend is not a journaled filesystem. `flush` explicitly persists deferred
atime and syncs file data. Close performs conditional cleanup as described
above. A successful create whose final host-parent sync fails still reports
success (the entry and metadata already exist) and logs a durability warning
instead of returning an error that encourages a duplicate create. Earlier
metadata setup errors still fail creation and attempt rollback. Sidecar
records/directories are synced, but file data and logical metadata still
are not a single crash-atomic transaction. Failed host deletion after metadata retirement can leave intact
data with default logical attributes; out-of-band replacement may also retain
old metadata until it is explicitly removed.
