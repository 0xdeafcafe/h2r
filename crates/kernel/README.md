# Xbox kernel HLE

Safe Rust adapters for the native runtime. This crate adds 92 declared export
contracts: 87 have implemented paths, and five are explicit stop policies. With
VFS configured, 69 of Halo's 136 imported functions have implemented paths;
67 still always stop. Supported functions can also stop on unsupported variants.
KeTickCount, the null LaunchDataPage, HalDiskCachePartitionCount and
XeImageFileName are initialized among the 17 imported DATA exports.

The startup milestone is demonstrated with synthetic guest handlers, including
ordinal 255, its guest SystemRoutine, the created thread, waits, TLS, and return
or termination. This is **not evidence that Halo's real startup thread, main
loop or first frame executes**. The lifter and device subsystems remain separate
lanes. No runtime, XBE, VFS or other lane's source was edited here.

## Attach to a loaded image

```rust,ignore
let mut loaded = runtime::load_xbe(&user_supplied_xbe)?;
// Register generated code and its exact resume PCs as usual.
let kernel = kernel::Kernel::for_image(&mut loaded)?;
// Optional: explicit disc/storage configuration, never an implicit host path.
kernel.set_vfs(vfs::Vfs::with_disc(storage_config, disc_path)?)?;
let mut machine = kernel::Machine::new(
    loaded.runtime.clone(), kernel, loaded.cpu,
    loaded.stack_bottom, loaded.stack_top,
)?;
let outcome = machine.run(10_000)?;
```

Attach before running entry: Machine relocates the pristine entry return word
below its FX/TLS reservation. It does not relocate an already active guest call
stack. `Machine::from_image` is the convenience form without attached VFS.
`Kernel::install` supports synthetic runtimes and independent service fixtures.
Attachment maps a read-only synthetic kernel PE image at 0x80010000. The retail
probe checks its last section for INIT before changing an x86 code-segment limit;
see [KERNEL-IMAGE.md](KERNEL-IMAGE.md) for disassembly evidence and DATA relocation.
Both `for_image` and `Machine::from_image` now take `&mut LoadedImage` so relocated
thunks and DATA metadata remain consistent.

A `Stopped` outcome names the thread and its context-local event ID. Inspect
`machine.thread(id).ctx.event(id)` and `kernel.stop_reasons()`. After installing
a missing handler, `machine.retry_kernel_call()` retries the untouched frame;
faults, firmware exits, and bugchecks cannot be retried through that method.
Pending owned thread completions survive an unsupported import stop/retry.

## ABI and shared state

`abi::Export` declares ordinal, name, convention, fixed argument word count and
variadic status. `Args` reads the actual guest return word and fixed arguments
before any handler effect. Guest stdcall pops four bytes per argument; fastcall
uses ECX/EDX and pops only later stack words; cdecl pops only the return word.
Returns reread the guest stack after service writes, including an output pointer
aliasing the saved return word. A by-value 64-bit stdcall argument occupies two words. Current fastcall exports
have only register-eligible 32-bit arguments; this is not an adapter for arbitrary
mixed-width fastcall prototypes.

`Value::U32`, `U64`, and `Void` control EAX, EDX:EAX, or preserve both respectively.
Other CPU fields are preserved. `abi::register` wraps a safe handler at a known
runtime kernel token; `register_at` supports an explicitly mapped executable
entry. The kernel crate forbids unsafe Rust.

Services lock kernel state, Runtime memory and (for file operations) shared VFS,
in that order, once per operation. The deferred-completion queue's mutex is a
leaf below all three. Atomic
Interlocked read/modify/write sequences retain the same guard, including through
physical aliases. Neither lock survives a transfer, guest callback or guest wait.
Handle values are monotonic guest integers, never host handles or pointers.
Duplicates refer to the same Rust object; closed values cannot become new handles.
VFS duplicates also use `Vfs::duplicate`, preserving position/search/share state.
Closing the final event/semaphore/file handle releases its pool object after any
wait/completion references finish. Thread backing cannot yet be reclaimed (see API requests).

## Implemented paths

| Ordinals | Behavior |
| --- | --- |
| 1, 4 | Saved guest AV pointer, initially null, with get/set round trip. |
| 47 | Shutdown-notification registration records in priority order (nothing calls them yet). |
| 97, 107, 113, 149, 150 | KTIMER/KDPC: initialize, set (one-shot/periodic), cancel; expiry signals and wakes on logical time and runs the DPC at DISPATCH_LEVEL. |
| 178 | Persist/unpersist pages of contiguous allocations (recorded; no quick reboot). |
| 203, 215 | Open a namespace symbolic link as a handle; query its captured target. |
| 327, 328 | XBE section reference counts, pristine reload of on-demand sections, shared-page counts. |
| 2 | Read configured AV encoder settings (option 6); other options stop. |
| 24 | Configured DWORD nonvolatile settings including XC_MISC, checked buffers and unknown-index stops. |
| 301 | Explicit NTSTATUS-to-DOS conversion for kernel/VFS outputs; unknown status stops. |
| 8 | DbgPrint guest cdecl varargs to an ordered byte log. |
| 14, 15, 17, 23 | Aligned pool allocation, size/tag tracking, free/reuse, actual block size. |
| 51–55 | Fastcall compare/exchange, decrement, increment, exchange and exchange-add; wrapping LONG arithmetic. |
| 67, 69 | VFS symbolic-link creation/deletion using guest counted ANSI names. |
| 99 | Non-alertable relative/absolute delay of only the calling guest thread. |
| 101, 122 | Nested per-thread kernel APC disable counts for critical regions; unbalanced leave stops. |
| 103, 104 | Guest IRQL and current KTHREAD lookup. |
| 108, 138, 145, 189, 225 | Initialize/reset/set/create events; notification and synchronization reset rules. |
| 112, 132, 193, 222 | Semaphore initialization/creation/release, bounds checks and previous-count outputs. |
| 126, 127, 128 | Replayable 64-bit counter, matching frequency, and NT system time. |
| 129, 160, 161 | IRQL raise/lower state, including low-byte fastcall arguments and invalid-transition stops. |
| 158, 159, 233–235 | Pointer/handle WaitAny and atomic WaitAll, timeouts, deterministic wake ordering and signal consumption. |
| 165, 166, 173, 179, 180 | Contiguous guest RAM with physical bounds/alignment/protection, physical translation and allocation queries. |
| 184, 199, 217 | VM reserve/commit/query and release of wholly uncommitted reservations. Committed release/decommit stops. |
| 187, 197 | Close/duplicate typed guest handles, including file handles; current-thread pseudo-handle close is a no-op and duplication creates a real thread handle. |
| 190, 202 | VFS create/open and root-directory-relative opens, synchronous alert/non-alert, free-space-query opens, guest handle and IO_STATUS_BLOCK outputs. |
| 198, 219, 236 | VFS flush/read/write in bounded 64 KiB host chunks, byte counts, explicit/current/append offsets, EOF and backend NTSTATUS errors. |
| 207 | One ANSI directory record per call, classes 1 and 12; peek before packing, consume only after fit/write. |
| 211 | File basic/standard/access/position/mode/network-open information, classes 4/5/8/14/16/34, answered whatever the handle's access (retail behavior). |
| 218 | Volume size/full-size information, classes 3/7, from actual VFS volume data. |
| 224, 238, 255, 258 | Suspended start, resume/count output, explicit yield, system-thread creation and noreturn termination. |
| 226 | Set basic information/position/EOF/delete-pending, classes 4/14/20/13; preserve zero/-1 timestamp sentinels. |
| 260, 308 | ASCII ANSI/UTF-16 conversions with allocated or sufficiently large caller buffers. |
| 277, 291, 294, 306 | Xbox 28-byte RTL critical sections, recursive ownership, try-enter and cooperative contention. |
| 279, 286, 287, 289, 290 | Counted string equality/init/free with guest-width descriptors. |
| 164 DATA | Null launch page by default; explicit `set_launch_data` API (free 171 still stops). |
| 40, 326 DATA | Three cache partitions; XeImageFileName `\Device\CdRom0\default.xbe` (configurable). |
| 156 DATA | Wrapping 32-bit millisecond boot counter maintained in Runtime's existing guest storage. |

DbgPrint supports `%%`, `%s`, `%c`, `%d/%i/%u/%x/%X/%o/%p`, integer `l`, width,
precision, star width/precision and ordinary flags. Format, individual field and
output limits are 4096 bytes. Unsupported formats (including FP, `%n`, wide or
counted strings, and 64-bit lengths) stop before logging. `log_bytes()` preserves
all guest bytes; `log()` is a lossy UTF-8 display convenience.

Pool storage rounds live sizes to 16 bytes and retains page capacity for reuse;
free pool bytes remaining mapped is intentional (use-after-free is not a defined
pool contract). VM reserve placement is 64 KiB aligned; commits are 4 KiB pages
and may have scattered physical backing. Querying an existing mapping without
allocation-type provenance stops, rather than claiming it is image memory.
Allocation consults all Runtime mappings
so image pages, aliases, DATA, stacks and kernel storage cannot be allocated twice.
RW, read-only, RX and RWX protections are supported. PAGE_WRITECOMBINE and
PAGE_NOCACHE preserve their query bits and use coherent guest RAM with the same
base access permissions; there is no CPU cache/device timing model. Combining
both modifiers is invalid. MEM_TOP_DOWN and MEM_NOZERO stop until implemented.
No PAGE_NOACCESS, guest-requested PAGE_GUARD, physical pinning or quick-reboot
persistence is claimed.
Contiguous addresses use the `0x80000000 + physical` guest alias; internal storage
uses a vacant `0xa1000000..0xb0000000` range. These are harness policies.
`memory::HLE_RESERVED` (`0xa0000000..0xa1000000`, also `HLE_RESERVED_START` /
`HLE_RESERVED_END`) is reserved for HLE-owned guest memory such as the Bink
movie arena: no kernel allocator places pool, contiguous, virtual or internal
memory there, and a fixed NtAllocateVirtualMemory request touching it fails
with STATUS_CONFLICTING_ADDRESSES. Other crates map that range themselves.

## Threads, TLS and cooperative scheduling

Each guest thread owns its CpuAbi and ExecCtx. There is one guest KPCR, switched
by the single host thread. The saved TIB includes each thread's FS:0 exception
chain, stack bounds and arbitrary/fiber fields. FS+0x24 is IRQL, FS+0x28 the
current KTHREAD, and KTHREAD+0x28 its TLS cell. ETHREAD identity, create/exit time,
exit status, stack fields, termination flag and dispatcher signal state are
populated. Child stacks have a reserved, unmapped guard page between stack and
thread object, which later pool/internal allocation cannot occupy. RTL critical
section OwningThread holds the guest KTHREAD pointer. These are skeletons, not an implemented full kernel/driver object.

Threads reserve a 0x210-byte FX area, followed downwards by TLS and the guest
stack. The TLS payload is 16-byte aligned, with a leading guest pointer, a copy
of the XBE raw template, and zero-fill. FS stack base excludes the FX area, so
Halo's guest-computed index -37 reaches the cell for its 0x94-byte TLS block.
HLE does not seed the TLS index or the startup global: guest entry still computes
those values. Nonempty TLS callback arrays, debugger threads and nonzero thread
extensions stop. Template initialization timing and the provisional CpuAbi Xbox
FP startup policy still need Xbox/xemu trace validation.

A new thread enters its guest SystemRoutine(StartRoutine, StartContext), or its
StartRoutine(Context) when SystemRoutine is null. `ExecCtx::call_guest` supplies
an owned return completion; returning terminates that guest thread with success.
PsTerminateSystemThread honors its explicit exit status and never returns.
Suspended threads cannot run until NtResumeThread decrements their count to zero.

Cooperative scheduling comes first because the lifter already spills canonical
CPU state at explicit transfers, but has no validated interrupt/quantum safepoints
or instruction-count timing. One host runner gives reproducible kernel-boundary
schedules, atomic wait consumption and alias-coherent memory without host races.
Blocking calls park the current thread; ready threads run FIFO. Event/semaphore
waiters wake FIFO, while simultaneous timed sleepers tie-break by thread ID.
IRQL survives waits. WaitAll tests every object before consuming any signal.
A run with no ready thread advances logical time to the next deadline; with no
deadline it reports Deadlock. If every ready thread has just used Delay(0) or
NtYieldExecution, advance to the next future deadline too. A newly ready thread
gets a turn before being classified as polling. This deterministic policy lets
a polling parent observe a sleeping child without tying time to host speed. Transfer/slice budgets are diagnostics, not preemption.

Timers: an inserted KTIMER is held in Rust by address with its deadline in
clock ticks; the guest object keeps Header (type 8/9, Size 10, Inserted,
SignalState), DueTime, Dpc and Period. Each wake pass expires due timers one
at a time in deadline order (ties by address): signal, re-arm a periodic timer
(missed periods coalesce), and queue its DPC with the expiry system time as
SystemArgument1/2 (a DPC already queued is not queued twice). Waiters are
tested before each expiry against what has been signalled so far and against
sleep/wait timeouts due strictly earlier, so when the clock jumps past several
deadlines a WaitAny returns the index of the timer that expired first, and a
synchronization timer is consumed by the first wait it satisfies. Timer deadlines take part in the idle/polling
time advance like sleeps, so a game that only polls can jump straight to its
next timer. A due time already reached expires during KeSetTimer itself.
DPCs run one per slice before any thread, on a 16 KiB kernel stack, under
ExecCtx thread `threads::DPC_THREAD_ID` (0) at DISPATCH_LEVEL; the interrupted
IRQL is restored when the routine returns. A routine that blocks, delays,
yields or needs a KTHREAD stops: `RunOutcome::Stopped { thread: 0, .. }`,
inspectable with `Machine::dpc_thread()`, and not retryable. An inserted
timer or queued DPC whose guest header changed is a guest fault.

This does not model Xbox priority boosts or preempt a guest spin loop. Code that
never blocks/yields can starve other threads. Kernel waits above APC_LEVEL, file
I/O above PASSIVE_LEVEL, alertable waits/delays, APC/DPC/interrupt delivery, and
KeSetEvent/KeReleaseSemaphore WaitNext handoffs stop. Dispatcher wait-list blocks
are held in Rust; only the documented guest header/state fields are maintained.
SEH chain storage works, but exception dispatch/unwind stays stopped.

## Clock and replay

`Clock` measures 100 ns ticks at a configured virtual frequency of 10 MHz, not a
host or measured Xbox frequency. Its default NT epoch is explicitly zero;
`Config::epoch_100ns` supplies another replay epoch. Reads do not advance time.
The scheduler advances to idle/polling deadlines; the harness may inject time using
`Kernel::advance_time`. Tick updates occur on advances and service boundaries.
`Clock::advances`, `Clock::replay` and the Machine schedule expose ordered inputs
and outcomes. History retains at most 4096 absolute advances, dropping the older
half when full; `dropped_advances()` reports truncation. `Kernel::clock()` uses
an Arc-backed copy-on-write snapshot instead of copying history on every read.
Replaying a retained suffix recovers current time, but a full long-run replay
requires capturing advances externally before truncation. Overflow or backwards
replay fails explicitly.

This is clock/schedule determinism, not a full-memory/kernel snapshot facility.
VFS host metadata and external I/O are additional inputs; host storage timestamps
are currently supplied by VFS's host clock and need recording or clock injection
for full replay. A guest polling KeTickCount in an unyielding loop will not make
time progress without an external injection/safepoint policy.

## VFS boundaries

The kernel uses an explicitly attached shared VFS. `set_shared_vfs` attaches an
Arc/Mutex supplied by another HLE crate; `vfs()` returns that exact instance. It does not create host directories
until the caller constructs a storage-backed VFS. The kernel preflights guest
output/IO_STATUS_BLOCK buffers before effects, copies write inputs before I/O,
and converts every completed backend error using Error::ntstatus(). VFS
NotSupported becomes a diagnostic stop. Pre-I/O validation failures leave the
IO_STATUS_BLOCK untouched. DUPLICATE_CLOSE_SOURCE cleanup failures do not turn
a published successful duplicate into an error. Native VFS handles never enter guest RAM.

FILE_SYNCHRONOUS_IO_NONALERT and FILE_SYNCHRONOUS_IO_ALERT are admitted and
behave the same: every transfer completes on the host thread without a guest
wait, so no alert or APC can interrupt it. FileModeInformation reports the bit
the caller chose. FILE_OPEN_FOR_FREE_SPACE_QUERY is admitted for read-only
FILE_OPEN (XAPI.md). Valid create options VFS does not implement stop.
NtQueryInformationFile applies no per-class access check, matching retail
(which answers FileBasicInformation for XAPI's GENERIC_WRITE handle).
Reads and writes move at most `limits::IO_CHUNK` (64 KiB) through fallibly
allocated host scratch per step, whatever the guest length (a virtual buffer
can alias the same RAM many times). Later chunks continue from the position
the previous synchronous chunk left; a short chunk ends the transfer, and an
end-of-file after some bytes is a successful short read. A backend error after
some bytes were transferred is reported as that error. A scratch allocation
failure reports STATUS_INSUFFICIENT_RESOURCES. Nonzero file event/APC/context
arguments, asynchronous opens, allocation-size reservations, unsupported
object attributes, non-ASCII paths, unsupported information classes and
unverified short-buffer/truncation contracts stop. Directory short-buffer stops
retain the cursor for retry. File-object waits/guest FILE_OBJECT layouts, IRPs,
IOCTLs, notifications, rename, symbolic-link object handles and full-attribute
path queries remain unimplemented. Host backend calls execute synchronously on
the scheduler's host thread; they do not overlap other guest execution.

## Host budgets

Guest code decides how often host-side state grows, so each accumulating
structure has a lifetime bound (`kernel::limits`). History buffers drop their
older half when full and count what they dropped; live tables refuse more.
Calls that create a handle check handle capacity before any side effect, so a
full table fails NtCreateFile before VFS opens (or truncates) the file, and
PsCreateSystemThreadEx before it maps a stack.

| State | Budget | Overflow |
| --- | --- | --- |
| DbgPrint log | 4096 messages, 1 MiB | oldest half dropped; `dropped_log_messages()` |
| Stop reasons | 1024 | oldest half dropped; `dropped_stop_reasons()` |
| Machine schedule | 65536 | oldest half dropped; `Machine::dropped_schedule()` |
| Deferred-signal failures | 256 | oldest half dropped; `dropped_deferred_signal_failures()` |
| Live guest handles | 65536 | STATUS_INSUFFICIENT_RESOURCES |
| RTL critical sections | 65536 | diagnostic stop |
| File I/O scratch | 64 KiB per chunk | STATUS_INSUFFICIENT_RESOURCES if unallocatable |
| Raw-disk written sectors | 4096 (2 MiB) | STATUS_DISK_FULL; the write changes nothing |

Pool, thread stacks and contiguous memory are bounded by guest RAM. Internal
zero-filling uses a fixed page of scratch. Guest addresses are offset with
checked arithmetic: a header or field that would cross 2^32 is a guest fault.

## XAPI and completion integration

[XAPI.md](XAPI.md) documents the conversion-table provenance, console settings,
launch layout and 171 limitation, shared VFS, event signaling and owned dispatcher
references, the completion API usable while a Guest guard is held, per-thread
`last_error_cell`, the disk-space and cache-partition paths, symbolic-link
objects, sections, and the power-down timer/DPC path. Generated XAPI wrappers
continue to run above these services.

Completion summary (details in XAPI.md): `kernel.signal_event(handle)` and
`pin.signal_event()` take kernel state then guest memory, so call them only
with no Guest/VFS guard held (XAPI's `SignalBatch::deliver` does). Under a
guard, use `pin.defer_signal()`; the signal is applied at the next
`Machine::run` slice or by `kernel.drain_deferred_signals()`. Inline dispatcher headers are decoded on every use (type and Size);
borrowed stack addresses are never cached as live kernel-owned objects.

## Remaining stops and API requests

[STOPS.md](STOPS.md) lists every remaining function by ordinal, including the
extra stop policies 56–58, 171 and 177, and the conditional VFS exports. Unknown
or unimplemented functions retain KernelCall events and unmodified frames. No
handler returns a placeholder success for unavailable behavior.

DATA exports other than KeTickCount and LaunchDataPage retain Runtime's zero placeholders. The
runtime has no data-read hook or no-access permission with which HLE could turn
those reads into a named stop. They are not configured console data or initialized
OBJECT_TYPE/IDE objects. This is a remaining integration limitation, not support.

The runtime review API migration is complete: private `runtime()`, Guest guard
helpers, `run_from`, owned `call_guest`, CpuAbi/GuestFault names and override
replacement semantics are used. No additional callback/resume API is needed.
The following changes are requested from the owning lanes; none were made here:

```rust,ignore
// Runtime Guest: transactional removal/splitting, including partial ranges.
// Preserve backing bytes still referenced through an alias.
pub fn unmap(&mut self, va: u32, len: u32)
    -> Result<(), runtime::guest::MapError>;

// Runtime permissions: enforce PAGE_NOACCESS and deny unsupported DATA reads.
pub const NONE: runtime::guest::Permissions;
```

Unmap is necessary for committed NtFreeVirtualMemory/decommit (199),
MmFreeContiguousMemory (171), and reclaiming terminated thread stacks. Until
then those frees stop. Whole uncommitted reservations and pooled objects can
already be released correctly. Partial installation/allocation failures that
need mapping rollback also depend on unmap.

The loader/runner inventory only knows Halo's original imports. Supplemental
exports such as Interlocked helpers currently use mapped executable adapters
from `Kernel::entry(ordinal)`; existing Halo thunks retain their runtime tokens.
To load other images importing these exports without changing static metadata:

```rust,ignore
pub fn load_xbe_with_exports(
    bytes: &[u8],
    functions: &[runtime::kernel::FunctionImport],
    data: &[runtime::kernel::DataImport],
) -> Result<runtime::LoadedImage, runtime::loader::LoadError>;
```

For initial-image VM query provenance, also expose the loader-validated fields
`LoadedImage::image_base: u32` and `LoadedImage::image_size: u32`. HLE can combine
those with known stack bounds instead of guessing allocation types from addresses
or current page permissions.

The function inventory passed to this loader must also govern its Runtime's
kernel-token recognition/execute-permission exemption. Current `load_xbe`
should retain its default inventory. This is not needed for Halo's current thunks.

As on the retail kernel, a title boots with
device objects only: `Vfs::with_storage` mounts Partition1..5 (Partition0 is
the kernel's rawdisk) and `Vfs::with_disc` adds the CD-ROM device, and neither
creates `\??\X:` links. XAPI creates D:, T: and U: itself and treats a T:
collision as fatal, and XMountUtilityDrive creates Z:. Host tools without XAPI
call `Vfs::create_standard_links(&Config)`, which creates exactly D:, T:, U:
and Z:; the boot path does not call it.

For VFS information classes currently stopped, the requested additions are:

```rust,ignore
// Add to vfs::FileInformation (volume-relative rooted name, not device_path).
pub volume_relative_name: String;
// Add to vfs::VolumeInfo; do not invent these in ABI packing.
pub creation_time: vfs::Timestamp;
pub filesystem_attributes: u32;
pub device_type: u32;
pub characteristics: u32;
// Allow deterministic timestamp production in HostBackend/BasicInfo creation.
pub fn with_clock(
    self,
    clock: impl Fn() -> vfs::Timestamp + Send + 'static,
) -> Self; // on vfs::HostBackend, used by every host timestamp producer
```

These unblock file-name information and volume creation/label/device/attribute
classes and deterministic storage time. Their absence does not block the current
synchronous file subset. No VFS API change is needed for that subset.

Known performance follow-ups: memory allocation/commit can scan O(pages ×
mappings); dispatcher collection scans owned objects against handles and thread
wait lists on each wake. Range indexes and reverse reference counts are deferred
until traces establish their cost. Ordinals and guest layout offsets are named
in `ord.rs` and `layout.rs`.

## Validation and sources

Validation on 2026-11-15: `cargo test -p kernel --all-targets` passed every
test, with one local-disc test ignored by default. That test was run separately
with `--ignored` against the supplied retail XBE and passed (it also checks
section accounting and the seeded DATA exports). Kernel Clippy with
`--all-targets --no-deps -- -D warnings` and `cargo fmt -p kernel -- --check`
passed.

Tests cover every declared export's argument layout, bad return/argument spans,
cleanup and EDX:EAX return rules using synthetic safe handlers. Behavioral tests
cover each implemented family and error/stop paths: physical aliases, read-only
pages, scattered commit, transactional preflight, reuse/stale handles, WaitAll,
recursive contention, IRQL, TLS, suspended starts, deadlines, noreturn termination,
owned child completion across kernel stop/retry, and VFS IO/cursors/access checks.
File tests use a deterministic Rust memory backend; no disc or host writes occur.

```sh
cargo test --workspace
cargo clippy -p kernel --all-targets --no-deps -- -D warnings
```

Public declarations and layouts were checked 2026-11-14 against
[nxdk exports](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.exe.def),
[nxdk kernel types](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.h),
[nxdk thread startup](https://github.com/XboxDev/nxdk/blob/master/lib/winapi/thread.c),
and the independent
[Cxbx guest layout definitions](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/types.h).
The compatibility contracts in [LIFTER.md](../../docs/LIFTER.md),
[kernel-imports.md](../../docs/kernel-imports.md),
[Runtime API changes](../runtime/API-CHANGES.md), and
[VFS storage contract](../../docs/storage.md) remain the integration references.
These moving references and synthetic tests are not hardware trace validation.
