# Kernel contracts used by generated XAPI

Generated XAPI save, disk-space, config and launch-info wrappers continue to run
as guest code. These are their kernel services and the integration APIs for
other HLE crates. Unsupported variants retain the original call frame and stop.

## NTSTATUS conversion (301)

`dos::NTSTATUS_TO_DOS` lists the explicit numeric mappings, with symbolic names
in comments. `dos::from_ntstatus` also covers all 64 possible successful WaitAny
indices. The adapter is stdcall with one word, returns a DWORD in EAX, and leaves
EDX unchanged. Any other status stops; customer-bit, HRESULT and NTWIN32
pass-through rules are deliberately not generalized into success.

The mapping facts were checked on 2026-11-15 against
[Wine's generated ntdll mapping](https://github.com/wine-mirror/wine/blob/master/dlls/ntdll/error.h),
[ReactOS's independent RTL mapping](https://github.com/reactos/reactos/blob/master/sdk/lib/rtl/error.c),
and the numeric definitions in
[Microsoft's public WinSDK winerror.h](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/shared/winerror.h).
This is an authored subset of standardized status/error pairs, not an extracted
Xbox kernel table. Every constant in `status.rs` and every `vfs::Error` variant
has a regression case. Wait indices 1/2/3/63 map to ERROR_WAIT_1/2/3/63; 4–62
have no named message mapping and explicitly produce 317. Unknown codes outside
this supported set stop instead of using that fallback.

Xbox's STATUS_TIMEOUT maps to WAIT_TIMEOUT (258), unlike desktop Wine's
ERROR_TIMEOUT (1460). This exception follows the
[Cxbx Xbox correction b080e4e](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded-legacy/commit/b080e4e87dadac46f0294c3ecaf158ff39fb161f).
The compatibility table still needs hardware-oracle comparison; it does not
claim to reproduce every historical Xbox build's conversion table.

## Console profile (24 and read-only option 6 of 2)

`Config::console: ConsoleConfig` configures language, video standard, factory
video flags, user video flags, AV pack, game region and parental game/movie
limits/password. Defaults are English (1), NTSC-M, 60 Hz, HDTV pack, 480p enabled,
North America game region (1), and parental values zero (restrictions disabled).
`set_console_config` / `console_config` allow explicit adjustment between slices.
No host locale, EEPROM, secrets or persistent settings are read or modified.

ExQueryNonVolatileSetting accepts eight DWORD indices: 7 (language), 8 (video
flags), 10/11/12 (parental values), 0x11 (XC_MISC flags), 0x103 (standard |
factory flags), and 0x104 (game region). XC_MISC is `ConsoleConfig::misc_flags`,
default zero: auto power-down off (0x01, XC_MISC_FLAG_AUTOPOWERDOWN) and DST
adjustment on (0x02 disables it). XAPI's auto power-down setup reads it at
0x2d6f79. Unknown/unconfigured indices stop, including raw EEPROM dumps.
It returns REG_DWORD and four-byte ResultLength; ResultLength may be null.
Short buffers report BUFFER_TOO_SMALL and required length without writing the
value or type. Full output spans are checked before effects, and a longer value
buffer has its tail cleared. Invalid guest pointers fault before any output.

An AV pack is not an EEPROM setting. AvSendTVEncoderOption (2), option 6 with
Param zero, writes the configured pack | standard | factory flags | video flags
to Result and returns VOID. Other encoder options stop: this implements profile
readout, not display-mode changes or GPU programming. Sources are
[nxdk's declarations and setting indices](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.h),
[nxdk's video flags](https://github.com/XboxDev/nxdk/blob/master/lib/hal/video.h),
and the independent
[Cxbx query contract](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlEx.cpp)
and [AV settings composition](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlAv.cpp).

## LaunchDataPage (164 DATA) and free (171)

The exported cell is initialized to null, including with `for_image`; a synthetic
runtime gets an internal cell if none was supplied. `data_export(164)` gives its
guest address. The original XGetLaunchInfo can therefore return ERROR_NOT_FOUND
(1168) without entering its non-null cleanup path.

`set_launch_data(Some(&launch::LaunchData { ... }))` explicitly supplies a page;
`None` resets the cell to null. The layout follows
[nxdk's LAUNCH_DATA_PAGE declaration](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.h):
kind at +0, title ID +4, NUL-terminated ANSI path at +8 (520 bytes), flags +528,
zero padding through +1024, then 3072 payload bytes. Oversize/invalid input is
rejected before publication. The setter reuses its own page, clears omitted
bytes, and refuses to overwrite a guest-changed foreign page pointer. Its page
is kernel-owned RAM retained until Runtime teardown, not quick-reboot storage.

**171 remains a stop.** Runtime has no guest unmap/remap API. Claiming to free a
mapped contiguous allocation would leave allocation/reuse behavior incorrect.
Consequently non-null launch data can be inspected/copied by generated XAPI,
but a wrapper that then frees it stops at 171. Null launch info is the supported
end-to-end default. The existing requested `Guest::unmap(va, len)` API is still
needed; no runtime or VFS source was changed to hide this limitation.

## Disk-space query (202 + 218)

GetDiskFreeSpaceEx (retail 0x2d15fe..0x2d163b) opens the queried directory with
NtOpenFile(SYNCHRONIZE | FILE_LIST_DIRECTORY, share read|write, options
0x800021) and reads FileFsSizeInformation (class 3, 24 bytes) from the handle.
0x800000 is FILE_OPEN_FOR_FREE_SPACE_QUERY. The kernel admits it for a FILE_OPEN
that requests no modifying access (write/append/EA/attributes/delete/DAC/owner,
GENERIC_WRITE/ALL) and strips it before VFS: the handle is an ordinary
synchronous handle and 218 answers from its mount's volume information. The
flag is not reported by FileModeInformation. Other uses of the flag stop. Any
other valid create option VFS reports as unsupported is a diagnostic stop, not
STATUS_NOT_SUPPORTED. `tests/files.rs` replays the exact retail arguments.

## Utility-cache partition (raw Partition0)

XMountUtilityDrive (0x2d5b77) calls XapiSelectCachePartition (0x2d592c), which
opens `\Device\Harddisk0\partition0` (GENERIC_READ | GENERIC_WRITE |
SYNCHRONIZE, share 3, FILE_SYNCHRONOUS_IO_ALERT) and reads 512 bytes at 0x800.
The kernel's disk driver owns that device: `set_vfs`/`set_shared_vfs` mount
`rawdisk::RawDisk` at `rawdisk::RAW_DISK_DEVICE` unless the VFS already
resolves the name. The disk is zero except the cache database, whose layout is
derived from 0x2d59af..0x2d5b5e:

| Offset | Field |
| --- | --- |
| 0x000 | signature 0x97315286 |
| 0x004 | version 2 |
| 0x008 | 41 entries × 12 bytes: title ID, partition index (partition = index + 3), valid |
| 0x1f8 | marker 0xaa550000 |

XAPI scans `min(HalDiskCachePartitionCount, 41)` entries. HLE sets
HalDiskCachePartitionCount (40 DATA) to 3. The synthetic database holds one
valid entry assigning `Config::utility_partition` (default 5) to
`Config::title_id` (read from the XBE certificate by `for_image`), so XAPI
selects that partition without asking for a format, moves the entry to the
front and writes the sector back; writes are kept for the session, up to
`limits::MAX_RAW_DISK_SECTORS` distinct sectors (STATUS_DISK_FULL beyond). With no
title ID the database is valid and empty and XAPI picks a partition itself
and requests a format. The device size is the sum of the VFS partition sizes
plus a 512 KiB config area; nothing beyond the database is synthesised.

XeImageFileName (326 DATA) is the ANSI_STRING `Config::image_file_name`,
default `\Device\CdRom0\default.xbe`, from which XAPI creates D:.

## Drive letters and symbolic-link objects (203, 215)

XAPI startup creates `\??\D:`, `\??\T:` and `\??\U:` itself (IoCreateSymbolicLink
at 0x2d1bea; a collision on T: makes it reboot to the dashboard), and
XMountUtilityDrive creates Z:. `Vfs::with_storage` and `Vfs::with_disc` mount
device objects only and create no `\??\` links, so a booted title starts with
no drive letters, as on retail and in the golden census fixture, and XAPI
creates all four. `Vfs::create_standard_links(&Config)` creates exactly D:, T:,
U: and Z: for host tools that run without XAPI; the boot path never calls it.

NtOpenSymbolicLinkObject opens a namespace link by name (null root, only
OBJ_CASE_INSENSITIVE) and captures its target; a missing link returns
STATUS_OBJECT_NAME_NOT_FOUND, so XAPI's 0x2d0e12 falls back to its built-in
`\Device\CdRom0`. NtQuerySymbolicLinkObject follows the NT object manager:
it copies the target plus a NUL, which needs MaximumLength >= Length + 1,
otherwise STATUS_BUFFER_TOO_SMALL with nothing copied; ReturnedLength receives
Length + 1 either way. Link handles close with 187 and are not files or events.

## Sections (327, 328)

`for_image` reads the XBE section table and mirrors the retail loader's
accounting: PRELOAD sections start with SectionReferenceCount 1 and their
head/tail shared-page counts incremented; on-demand sections ($$XTIMAGE,
$$XSIMAGE, .XTLID in the retail XBE) start at zero and their pristine bytes
are captured. XeLoadSection taking a section from 0 to 1 rewrites its range
with those bytes (through each page's permissions) and counts its shared
pages; XeUnloadSection of the last reference uncounts them, and unloading at
zero returns STATUS_INVALID_PARAMETER. Unloaded sections stay mapped (no
Guest::unmap). Reloading a released PRELOAD section and unknown headers stop.
The preload accounting follows Cxbx-Reloaded's loader, not a hardware trace.

## Shutdown notifications (47) and persistent memory (178)

HalRegisterShutdownNotification records or removes a HAL_SHUTDOWN_REGISTRATION
{routine, priority, LIST_ENTRY}, highest priority first
(`Kernel::shutdown_notifications`). The list lives in Rust and the guest's
LIST_ENTRY is not written. No shutdown path exists yet (49/360 stop), so no
routine is called. MmPersistContiguousMemory marks or clears whole pages of a
contiguous allocation (`Kernel::persistent_pages`); with no quick reboot that
record is the whole effect. Ranges outside contiguous memory stop.

## Shared VFS

`set_vfs(Vfs)` remains supported and wraps the instance in Arc/Mutex.
`set_shared_vfs(Arc<Mutex<Vfs>>)` attaches an existing shared instance;
`vfs() -> Option<Arc<Mutex<Vfs>>>` returns exactly that instance. Attachment is
one-time. All users share namespace links, open/share checks and backend data.
Native VFS handles and guest NT handles are separate identities; don't confuse
them. Kernel services acquire kernel state, then guest memory, then VFS. External
HLE code must drop guest-memory/VFS guards before calling another kernel API,
except `DispatcherReference::defer_signal` (below), which is safe under them.

## Completion events and object lifetime

`Kernel::signal_event(handle) -> Result<i32, Error>` validates and signals a guest
event atomically with respect to NtClose; its result is the previous signal
state. It readies waiters without executing guest code or delivering a DPC:
a waiting KTHREAD's State is Ready (1) as soon as the call returns.

Both `Kernel::signal_event` and `DispatcherReference::signal_event` take kernel
state and then guest memory. **Never call them while holding
`runtime.memory()` or the VFS guard**: the thread would block on the mutex it
already holds. Two safe completion paths:

1. Signal after releasing the guard. XAPI's `SignalBatch::deliver(&mut impl
   Events)` already does this; an `Events::signal(guest_handle)` adapter maps
   the handle to the `DispatcherReference` pinned at submission, calls
   `pin.signal_event()` and then releases the pin.
2. Defer while holding the guard: `pin.defer_signal()` (consumes the pin). It
   takes only the kernel's completion-queue mutex, a leaf lock under which
   nothing else is ever acquired, and neither signals nor drops the pin there.
   The queue owns the pin until the signal is applied, at the start of the
   next `Machine::run` slice or by `kernel.drain_deferred_signals()` (call it
   with no guard held; it returns one `DeferredSignal { address, result }` per
   queued pin, in order). Failures during `Machine::run` are kept for
   `kernel.take_deferred_signal_failures()` (at most 256; see
   `dropped_deferred_signal_failures`). `pending_deferred_signals()` counts
   queued, unapplied signals.

The lock order stays kernel state -> guest memory -> VFS; the queue lock is a
leaf below all three. Dropping or releasing a `DispatcherReference` takes the
kernel-state lock, so do that after releasing a Guest guard too (or hand the
pin to `defer_signal`).

`Arc<Kernel>::reference_dispatcher(handle)` returns a `DispatcherReference` to an
HLE event, semaphore or thread object. `address()` exposes its guest address;
`signal_event()` works for an event even after all original handles close.
`release(self)` or Drop releases the reference. These pins participate in object
collection alongside handles and blocked waits, preventing pool reuse while a
rumble completion is outstanding. Invalid handles, wrong types and changed guest
headers fail; every use revalidates the guest header. These APIs support real
handles, not the context-dependent NtCurrentThread pseudo-handle. Guest
ObReferenceObjectByHandle/ObfDereferenceObject exports remain stops.

## Last-error TLS cell

The original helper at 0x2d1d3e computes:

```text
stack_base = FS:[4]
slot       = stack_base + signed(_tls_index) * 4
payload    = guest_u32(slot)
last_error = payload + 8
```

The kernel does not seed `_tls_index`: entry code computes it. For the supplied
retail 0x94-byte template it is -37 (0xffffffdb). `Kernel::last_error_cell(tid,
index: i32)` performs this lookup using that thread's saved TIB (or live TIB if
running), validates the implemented single-template slot and payload, and returns
a writable guest DWORD address. It therefore works for a suspended/non-current
thread without reading another thread's shared KPCR. XAPI owns writing the
converted DOS error into that cell. Tests isolate two threads' cells and reject
bad indices, absent threads and a changed TLS pointer.

## Auto power-down timers

XAPI's power-down setup (0x2d6049..0x2d601e) now runs on the kernel's timers:
KeInitializeDpc (107), KeInitializeTimerEx (113), KeSetTimer (149) with the
retail six-hour relative due time, plus KeSetTimerEx (150) and KeCancelTimer
(97). See README.md for the timer and DPC model. With logical time, the timer
expires only when the scheduler advances to it (idle or polling threads) or
the host injects time; its DPC then runs at DISPATCH_LEVEL on a kernel stack.
XC_MISC is read during this setup; how the DPC routine treats its
auto-power-down bit has not been traced.
