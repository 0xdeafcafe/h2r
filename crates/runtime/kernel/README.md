# Xbox kernel HLE

Safe Rust adapters for the native runtime. This crate adds 106 declared export
contracts: 102 have implemented paths, and four are explicit stop policies. With
VFS configured, 84 of Halo's 136 imported functions have implemented paths;
52 still always stop. Supported functions can also stop on unsupported variants.
KeTickCount, the null LaunchDataPage, HalDiskCachePartitionCount and
XeImageFileName are initialized among the 17 imported DATA exports.

The startup milestone is demonstrated with synthetic guest handlers, including
ordinal 255, its guest SystemRoutine, the created thread, waits, TLS, and return
or termination. This is **not evidence that Halo's real startup thread, main
loop or first frame executes**. The lifter and device subsystems remain separate
lanes. No runtime, XBE, VFS or other lane's source was edited here.

## Attach to a loaded image

```rust,ignore
// Halo 2: reserve its title arena (physical 0x61000..0x3945000, alias
// 0x80061000) before anything else gets backing.
let mut loaded = runtime::load_xbe_with(
    &user_supplied_xbe,
    &runtime::loader::LoadOptions::halo2(),
)?;
// Register generated code and its exact resume PCs as usual.
let kernel = kernel::Kernel::for_image(&mut loaded)?;
// Or `Kernel::for_image_with(&mut loaded, config)` to pass a console profile,
// epoch, utility partition or image name; TLS, DATA and TitleID still come
// from the image.
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
| 2 | AV encoder: configured settings query (6); blank screen (9), flicker filter 0-5 (11) and luma filter (14) recorded in `Kernel::encoder_state()`; the field guess (15) and other options stop. |
| 24 | Configured DWORD nonvolatile settings including XC_MISC and XC_AUDIO (9, stereo/no passthrough by default), checked buffers and unknown-index stops; XC_MAX_ALL (0xffff) and XC_MAX_OS (0xff) are STATUS_OBJECT_NAME_NOT_FOUND (no EEPROM image). |
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
| 165, 166, 171, 173, 179, 180 | Contiguous guest RAM with physical bounds/alignment/protection, free (unmap), physical translation and allocation queries. Bounds wholly inside a Runtime physical reservation map onto it (Halo's XPhysicalAlloc arena returns 0x80061000). |
| 169, 170 | Kernel stacks in the internal range with an unmapped guard page; returns the top; delete unmaps. |
| 182 | MmSetAddressProtect on contiguous or committed pages via `Guest::protect_range` (exact pages; other aliases keep theirs); queries report the new value. |
| 184, 199, 217 | VM reserve/commit/query, MEM_TOP_DOWN placement, decommit (unmaps committed pages) and release. |
| 143 | KeSetBasePriorityThread: NT base/saturation rules, writes KTHREAD Priority/BasePriority/Saturation, returns the old increment. The scheduler runs the highest-priority ready thread. |
| 192, 221 | Unnamed mutants: initial owner, recursive acquire, release counts, MUTANT_NOT_OWNED, abandonment on thread exit (STATUS_ABANDONED_WAIT_0 + index). |
| 210 | NtQueryFullAttributesFile: FILE_NETWORK_OPEN_INFORMATION by path (root 0, -3 or a directory handle) without leaving a handle open. |
| 246, 250 | ObReferenceObjectByHandle (thread/event types, null type, current-thread pseudo-handle) and ObfDereferenceObject; references pin the object. |
| 252, 253 | PhyInitialize / PhyGetLinkState on the offline path; contract below. |
| 269, 304, 305 | RtlCompareMemoryUlong (whole leading DWORDs; unaligned source stops), RtlTimeFieldsToTime (validated, BOOLEAN) and RtlTimeToTimeFields (proleptic Gregorian from 1601; negative times stop). |
| 335–337 | XcSHAInit/Update/Final: SHA-1 (FIPS 180-4) in the caller's 116-byte context. |
| 340 | XcHMAC: HMAC-SHA1 (RFC 2104) over input then input2 (a zero length is not read); keys over 64 bytes stop. |
| 323 DATA | XboxHDKey: filled with `crypto::XBOX_HD_KEY` (all zero, this console's key); `content` signs installed content with it (docs/game/map-pack.md). |
| 187, 197 | Close/duplicate typed guest handles, including file handles; current-thread pseudo-handle close is a no-op and duplication creates a real thread handle. |
| 190, 202 | VFS create/open, root-directory-relative and ObDosDevices (root -3) opens, synchronous and asynchronous file objects, free-space-query opens, guest handle and IO_STATUS_BLOCK outputs. |
| 198, 219, 236 | VFS flush/read/write in bounded 64 KiB host chunks, byte counts, explicit/current/append offsets, EOF and backend NTSTATUS errors; an Event argument is reset then signalled on completion. |
| 207 | One ANSI directory record per call, classes 1 and 12; peek before packing, consume only after fit/write. |
| 211 | File basic/standard/access/position/mode/network-open information, classes 4/5/8/14/16/34, answered whatever the handle's access (retail behavior). |
| 218 | Volume size/full-size information, classes 3/7, from actual VFS volume data. |
| 224, 238, 255, 258 | Suspended start, resume/count output, explicit yield, system-thread creation and noreturn termination. |
| 226 | Set basic information/position/EOF/delete-pending/allocation, classes 4/14/20/13/19; preserve zero/-1 timestamp sentinels. Allocation below EOF truncates; otherwise it is a no-op reservation. |
| 260, 308 | ASCII ANSI/UTF-16 conversions with allocated or sufficiently large caller buffers. |
| 277, 291, 294, 306 | Xbox 28-byte RTL critical sections, recursive ownership, try-enter and cooperative contention. |
| 279, 286, 287, 289, 290 | Counted string equality/init/free with guest-width descriptors. |
| 164 DATA | Null launch page by default; explicit `set_launch_data` API. |
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
Physical placement asks Runtime (`Guest::find_free_physical`), which counts
every mapping and physical reservation, so image pages, aliases, DATA, stacks,
kernel storage and reserved arenas cannot be allocated twice. The only way into
a reservation is a contiguous request whose [Lowest, Highest] lies wholly
inside it; that maps through `Guest::map_reserved` at `0x80000000 + physical`
(the reservation must have that alias, else a stop). Freeing such memory unmaps
it and the pages stay reserved, bytes intact. MEM_TOP_DOWN places a new
reservation at the highest vacant 64 KiB-aligned address below 0x7fff0000 (or
the ZeroBits limit); with an explicit base it changes nothing. MEM_NOZERO stops.
No-access, read-only, RW, RX and RWX protections are supported. PAGE_WRITECOMBINE and
PAGE_NOCACHE preserve their query bits and use coherent guest RAM with the same
base access permissions; there is no CPU cache/device timing model. Combining
both modifiers is invalid. No guest-requested PAGE_GUARD, physical pinning or
quick-reboot persistence is claimed. NtFreeVirtualMemory decommit covers
[base & ~0xfff, round_up(base + size)), or the rest of the allocation for size
0; a range past the allocation, and release at an interior address, stop.
Every committed page is checked before any is unmapped, so a free that would
touch an HLE claim, a hole or lifted code stops with the region unchanged.
No kernel allocator places anything in a reservation's alias windows (only
`Guest::map_reserved` can fill them); a fixed virtual reserve or commit there
fails with STATUS_CONFLICTING_ADDRESSES.
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
Blocking calls park the current thread. The next thread is the highest KTHREAD
Priority ready (as KeSetBasePriorityThread left it), FIFO among equals, so equal
priorities round-robin; there is no preemption, so a thread made ready never
interrupts the running one. Event/semaphore
waiters wake FIFO, while simultaneous timed sleepers tie-break by thread ID.
NtYieldExecution follows NT (KiSelectReadyThread with a floor of 1): if any
other thread is ready, whatever its priority, the best of them runs next and the
yield returns SUCCESS; the yielder goes to the back of the queue and is passed
over once. Alone, it returns NO_YIELD_PERFORMED: at once when nothing is
pending, otherwise as a poll (below).
IRQL survives waits. WaitAll tests every object before consuming any signal.
A run with no ready thread advances logical time to the next deadline; with no
deadline it reports Deadlock. If every ready thread has just used Delay(0) or
NtYieldExecution, advance to the next future deadline too. With priorities,
"every ready thread" means the ones that would be scheduled: the next pick and
every ready thread of its priority or higher. A lower-priority thread that never
gets a turn behind high-priority pollers does not hold the clock still, so the
timer or timeout that releases the pollers still arrives. A newly ready thread
gets a turn before being classified as polling, and an advance clears every
ready thread's polling mark: each of them gets a slice before the clock can move
again. Without that, a lone thread yielding while a periodic timer is armed
(Halo's movie loop at 0x1639c2 calls SwitchToThread with XNet's 20 ms timer
set) would watch the clock and its DPC run forever without a slice of its own. This deterministic policy lets
a polling parent observe a sleeping child without tying time to host speed. Transfer/slice budgets are diagnostics, not preemption.

Generated code adds one preemption point, so a guest spin-wait on another
thread cannot hang this schedule (`runtime::preempt`, and "Loop safe points"
in `crates/recompiler/emit/RUNTIME-CONTRACT.md`). Each slice arms the thread's context
with `Machine::preempt_slice` guest memory accesses (default
`runtime::preempt::DEFAULT_SLICE`, 65536). Once they are spent, or when the
host calls `Runtime::request_preempt(REQUEST)`, the thread stops at its next
loop header or transfer, goes to the back of the ready queue as if it had
called NtYieldExecution (passed over once while any other thread is ready,
whatever its priority; like NtYieldExecution this does not stop two
higher-priority spinners alternating while a lower-priority thread waits),
and resumes exactly there on its next slice. At DISPATCH_LEVEL or above it
keeps the CPU and starts a fresh slice instead. The count is of accesses,
not host time, so schedules stay reproducible; `Machine::preemptions()`
counts these switches and `preempt_slice = None` restores the purely
cooperative schedule.

Time and work. A preempted thread is runnable, never polling: the clock
does not jump for it. Instead, with a slice set, the accesses a thread's
generated code makes are charged to logical time at
`threads::ACCESSES_PER_TICK` (100 per 100 ns tick, one a nanosecond),
carrying the remainder. Work is settled on entry to every kernel call, so
what the call reads of time and any relative deadline it sets start after
the work before it, and again at the end of every run, including when a
thread at DISPATCH_LEVEL or above keeps the CPU for another slice. The
rate is a harness choice, not a calibrated Xbox timing model: simple,
reproducible, about 66 us per default slice (15 slices per guest
millisecond); accesses are not cycles, and register-only work, access
width and cache behaviour are not costed. A productive loop moves the
clock by its own cost and no more, however alike its states look, and a
spin-wait (on a flag, a counter, the performance counter or the TSC)
carries the clock to its deadline by running. Jumping to the next deadline
stays reserved for threads that are blocked in the kernel or have asked to
be: no thread ready, or every scheduled thread waiting, sleeping or polling
through Delay(0)/NtYieldExecution as above. Rust overrides and DPCs make
no charged accesses. The guest TSC follows this clock (and its
per-read minimum step), so an `rdtsc` loop stays reproducible and ends.

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
A DPC queued while the caller is below DISPATCH_LEVEL runs before that caller
continues, as on retail, where it runs as soon as IRQL drops: when a kernel call
completes with DPCs queued, the thread is put back at the head of the ready
queue and the DPCs run first. So KeSetTimer with a reached due time has run its
DPC by the instruction after the call (Halo's voice engine, 0x332b6d, arms its
20 ms DPC 0x332f4c that way), and a timer that expired at raised IRQL delivers
on the KfLowerIrql. Otherwise DPCs run one per slice before any thread. A drain
runs until the queue is empty, so a DPC that queues a different DPC has it run
before any thread continues. Only a DPC that comes round again in the same drain
(say by re-arming its own timer due now) stays queued until a ready thread has
had a slice; DPCs queued behind it still run first, in order. So DPC delivery
cannot starve a thread either. An idle advance starts a new drain: a DPC
that ran at the previous deadline runs again at the next one before the
thread that deadline wakes (a periodic timer or vblank landing on a sleeper's
deadline). They run on a 16 KiB kernel stack, under
ExecCtx thread `threads::DPC_THREAD_ID` (0) at DISPATCH_LEVEL; the interrupted
IRQL is restored when the routine returns. A routine that blocks, delays,
yields or needs a KTHREAD stops: `RunOutcome::Stopped { thread: 0, .. }`,
inspectable with `Machine::dpc_thread()`, and not retryable. An inserted
timer or queued DPC whose guest header changed is a guest fault.

Mutants (192/221 and waits) keep their owner in Rust by guest KTHREAD and the
guest KMUTANT (type 2, Size 8, OwnerThread, Abandoned, MutantListEntry) up to
date; a terminating thread abandons what it owns. Named mutants stop.
KeSetBasePriorityThread priorities order the ready queue (above).

This does not model Xbox priority boosts or decrements or wall-clock
quantum ends. Generated loops are preempted by access count (above); Rust
override code that never blocks/yields, and a generated loop that touches no
memory, can still starve other threads. Kernel waits above APC_LEVEL, file
I/O above PASSIVE_LEVEL, alertable waits/delays, APC/DPC/interrupt delivery, and
KeSetEvent/KeReleaseSemaphore WaitNext handoffs stop. Dispatcher wait-list blocks
are held in Rust; only the documented guest header/state fields are maintained.
SEH chain storage works, but exception dispatch/unwind stays stopped.

## Periodic host interrupts and pacing

`kernel::periodic` delivers a host device's periodic interrupt on logical
time; the display's vertical blank is the one that matters. On the Xbox the
GPU interrupt's DPC calls the D3D vblank callback 60 times a second, and
Halo's frame time comes from counting them (`0x12b0e0` divides the vblanks
since the last frame by the word at `0x485AC0`, 60 when unset), so the
count has to follow the same clock as KeTickCount and the timers.

```rust,ignore
let id = kernel.add_periodic(kernel::periodic::Source {
    name: "vblank",
    rate: kernel::periodic::Rate::HZ_60,           // or NTSC (59.94), PAL
    max_catch_up: 4,
    handler: Box::new(d3d::vblank::clocked_vblanks(device.clone())),
})?;
kernel.periodic_stats(id);  // delivered, dropped, owed, next_deadline
kernel.remove_periodic(id);
```

- **Deadlines.** Period `n` is due at `origin + ceil(n * 10 MHz / rate)`,
  `origin` being the time of `add_periodic`, so the count of periods always
  matches elapsed logical time (no drift from rounding one period).
- **Delivery.** The wake pass queues a due source on the DPC queue behind
  the timer DPCs, once however many periods it owes. `Machine::run` drains
  it like a KDPC, before the next thread slice: for each period the Rust
  handler gets `(guest, scratch, missed)` and returns the guest call
  (`runtime::GuestInterrupt`: target, arguments, cdecl or stdcall), which
  runs on the DPC stack under `DPC_THREAD_ID` at DISPATCH_LEVEL with the
  interrupted IRQL restored afterwards. `scratch` is 64 bytes on that stack
  above the arguments, for a record the call points at (D3D's
  `D3DVBLANKDATA`). A callback that returns with the wrong ESP for its
  convention stops the DPC with `Event::InterruptImbalance`. The handler
  runs with guest memory locked and kernel state unlocked and must not call
  the kernel. A thread at DISPATCH_LEVEL or above holds delivery off, as on
  the one Xbox CPU.
- **Catch-up, spread out.** Periods that come due between two wake passes
  (a long slice kept at DISPATCH_LEVEL, a jump to a timer while threads were
  busy, `Kernel::advance_time`) all stay owed: Halo's callback (`0x14280`)
  adds one to `0x485AB0` per call and reads nothing else of the time, so a
  dropped period is game time lost against the clock. A drain delivers at
  most `max_catch_up` (at least 1) and queues the source again; the rest
  come in later drains, after a slice of whatever thread is ready, so a
  stall never becomes one burst of callbacks. No idle advance happens while
  periods are owed. Only a debt beyond `periodic::MAX_DEBT_SECONDS` (10 s)
  is dropped, counted in `Stats::dropped` and reported to the next delivery
  as `missed`.
- **Idle time.** A source's next deadline takes part in the idle advance
  and in NtYieldExecution's poll, so a thread that waits by yielding (or
  every thread polling) jumps straight to the next vblank, and a sleeping
  machine steps through each vblank on the way to its wake-up. Only idle
  polls count for it: a yield or Delay(0) after at least
  `threads::POLL_WORK` (one tick, 100 accesses) of charged work since the
  thread's last poll is a productive handoff, and while any thread that
  would be scheduled made one, the next vblank comes by charged work, not
  a jump. So workers handing off between units of work are not charged a
  display period per round (timers and sleeps still advance as before).
  Uncharged work (Rust overrides) looks idle. Nor does the clock move
  between a drain and the next slice of a ready thread: a poller sees each
  callback before time goes on. On its own a source never keeps a machine
  alive: with no ready thread and no other deadline, `run` still reports
  Deadlock instead of ticking vblanks forever.
- **A spin on the count.** A generated loop that waits for the callback to
  bump a counter (Halo's frame wait spins at `0x12b330`) gets there by
  running: its charged work carries the clock to the deadline, the next
  slice boundary runs the callback, and the loop sees it. That is exact
  but costs a period's worth of charged accesses (16.7 million per 60 Hz
  vblank), which is why D3D's frame-wait override yields instead
  (`FrameWait::HALO2_CLOCKED`, docs/graphics/d3d.md "Vertical blank").

Halo registers no swap callback: in device `+0x1DA0..+0x1E00` game code
touches only `+0x1DB8`, and `+0x1DBC` is a KEVENT that only D3D's replaced
display-mode path (`0x3f9b80`) waits on. Swaps reach the callback through the record's `Swap` count and
`D3DVBLANK_SWAPDONE`, which `0x14280` records at `0x485AB8`/`0x55E6B8`.

`kernel::pacer::Pacer` (`Machine::pacer`, default None) holds logical time
to wall time for a machine being watched: after any idle advance and
before the DPC or thread slice that runs at that time, it sleeps until as
much wall time has passed as logical time, so what the guest does after a
sleep or at a vblank happens no earlier in wall time than in logical time,
and fast guest work cannot run the game ahead of real time. It never hurries the
clock: a host that falls more than `max_lag` (100 ms) behind starts counting
again from the present (`reanchors()`). It only makes the host wait, so
schedule, history and guest state are the same paced or not.

One time base, then: KeTickCount, the performance counter, the TSC, timer
and sleep deadlines and the vblank count all read the one logical clock.
Script sleeps and physics take their time from frame time (vblanks), the
loading loop (`0x163890`, which yields through SwitchToThread) moves the
clock to the next vblank or timer when it is the only runnable work, and
nothing advances one of them without the others.

## Clock and replay

`Clock` measures 100 ns ticks at a configured virtual frequency of 10 MHz, not a
host or measured Xbox frequency. Its default NT epoch is explicitly zero;
`Config::epoch_100ns` supplies another replay epoch. Reads do not advance time.
The scheduler advances to idle/polling deadlines and by charged work
(above); the harness may inject time using
`Kernel::advance_time`. Tick updates occur on advances and service boundaries.
`Clock::advances`, `Clock::replay` and the Machine schedule expose ordered inputs
and outcomes. History retains at most 4096 absolute advances, dropping the older
half when full; `dropped_advances()` reports truncation. `Kernel::clock()` uses
an Arc-backed copy-on-write snapshot instead of copying history on every read.
Replaying a retained suffix recovers current time, but a full long-run replay
requires capturing advances externally before truncation. Charged work
(`Clock::charge`) does not fill the history: consecutive charges share one
entry, moved to the latest time, since the steps between them follow from
replaying the execution; any other advance starts a new entry. The
timestamps alone cannot tell the two kinds apart, so `Clock::replay`
recovers the time but treats every entry as an explicit advance: a charge
after it starts a new entry where the original would have moved its last
one. Record `Clock::charging()` with the history and continue with
`Clock::resume(epoch, advances, charging)` to get the original's grouping
(and an equal `Clock`) back.
Overflow or backwards
replay fails explicitly.

This is clock/schedule determinism, not a full-memory/kernel snapshot facility.
VFS host metadata and external I/O are additional inputs; host storage timestamps
are currently supplied by VFS's host clock and need recording or clock injection
for full replay. A guest polling KeTickCount in an unyielding loop makes time
progress only through the work it is charged (generated code with a
preemption slice set); Rust override loops need an external injection.

## VFS boundaries

The kernel uses an explicitly attached shared VFS. `set_shared_vfs` attaches an
Arc/Mutex supplied by another HLE crate; `vfs()` returns that exact instance. It does not create host directories
until the caller constructs a storage-backed VFS. The kernel preflights guest
output/IO_STATUS_BLOCK buffers before effects, copies write inputs before I/O,
and converts every completed backend error using Error::ntstatus(). VFS
NotSupported becomes a diagnostic stop. Pre-I/O validation failures leave the
IO_STATUS_BLOCK untouched. DUPLICATE_CLOSE_SOURCE cleanup failures do not turn
a published successful duplicate into an error. Native VFS handles never enter guest RAM.

`Kernel::read_at(guest_handle, offset, out)` is one positioned read through a guest file handle, the VFS read `NtReadFile` makes without the guest call or the file position: the native texture streaming reads (`render-bridge`'s `texture_io::pump`) use it.

FILE_SYNCHRONOUS_IO_NONALERT and FILE_SYNCHRONOUS_IO_ALERT are admitted and
behave the same: every transfer completes on the host thread without a guest
wait, so no alert or APC can interrupt it. Asynchronous file objects (neither
bit) are admitted too, and complete synchronously; see "Asynchronous I/O"
below. FileModeInformation reports the bit
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
failure reports STATUS_INSUFFICIENT_RESOURCES. APC routines, unsupported
object attributes, non-ASCII paths, unsupported information classes and
unverified short-buffer/truncation contracts stop. Directory short-buffer stops
retain the cursor for retry. File-object waits/guest FILE_OBJECT layouts, IRPs,
IOCTLs, notifications, rename and symbolic-link object handles remain
unimplemented. OBJECT_ATTRIBUTES RootDirectory -3 (ObDosDevices) resolves the
name under `\??\`; a rooted or empty name under it stops.

### PHY / link state (252, 253)

No NIC is modelled. The contract is decoded from the retail call sites (net.rs;
the `#[ignore]`d `retail_phy_call_sites_have_the_documented_shapes` re-checks it
against `$H2_XBE`):

- `PhyInitialize(BOOLEAN forceReset, PVOID reserved)` (253), stdcall 2, returns
  an NTSTATUS. Sites: `0x36c4a1` (inside the link wrapper `0x36c47d`, called
  only while the flag at `0x48481c` and a per-processor word are zero; result
  ignored) and `0x3d6a7d` (adapter bring-up, `cmp eax,edi; jl` at `0x3d6a83`:
  a negative status aborts). Both push (0, 0). It succeeds regardless of cable
  state and records that the PHY was probed; a nonzero second parameter is
  unobserved and stops.
- `PhyGetLinkState(BOOLEAN update)` (252), stdcall 1, returns a mask. Sites:
  `0x36c4a8` (argument 0; the wrapper returns the mask only if bit 0 is set,
  else 0, `0x36c4b8..0x36c4c0`), `0x3d5f6b` (argument `edi == 0`; bit 2 picks
  the speed value at `0x3d5f88`, bit 3 the duplex write at `0x3d5f96`) and
  `0x3dabc6` (argument 0; `test al,1` at `0x3dabcc`, clear sets the link-down
  status `0x80000010` and fails the call). Bit 0 = link up, bit 2 = 100 Mbps,
  bit 3 = full duplex. The kernel returns 0 (no cable) before PhyInitialize and
  by default after it; `ConsoleConfig::phy_link` can configure a link, and only
  the three tested bits are reported, none without bit 0. With link down the
  caller skips XNetStartup, so no network hardware is required.

### Asynchronous I/O

NtCreateFile also accepts an initial allocation size through VFS's allocated
opens. It reads the LARGE_INTEGER before writing the IOSB, since generated
XCreateSaveGame aliases them when reserving SaveMeta.xbx. Host storage reserves
logical cluster capacity without changing EOF; disk-full create/overwrite
reservations preserve existing data. AllocationSize is only a hint, as on NT:
it never changes EndOfFile, and a backend without reservation support (the
`Backend` default) ignores it rather than failing the create. See [checkpoint acceptance](../../../docs/runtime/checkpoints.md)
for the generated-code harness and its boundaries.

Synchronous completion is equivalent for what Halo does. The 62 opens without a
synchronous bit (options 0x4040) are all XAPI DeleteFile (0x2d6b7e): open
DELETE, set FileDispositionInformation, close, with no I/O on the handle. XAPI
ReadFile (0x2d2241) and WriteFile (0x2d232e) with an OVERLAPPED pass Event =
hEvent, ApcRoutine = 0 and IoStatusBlock = the OVERLAPPED. ApcContext is NULL
when hEvent bit 0 is set and the OVERLAPPED otherwise (decoded: `and cl,1; neg
cl; sbb; not; and esi`). XAPI
treats any status other than STATUS_PENDING as complete. The kernel does the
transfer before returning, writes the IO_STATUS_BLOCK, resets and then signals
the event, and never returns STATUS_PENDING, which is a legal outcome on
retail. On an asynchronous handle the ByteOffset is required (NULL is
STATUS_INVALID_PARAMETER and FILE_USE_FILE_POINTER_POSITION stops), and the
file position is left unchanged. ApcContext, NULL or OVERLAPPED, is ignored on purpose: without an
ApcRoutine it only keys a completion port, which Xbox lacks. An ApcRoutine stops,
as does an Event handle with its low bit set (NT's completion-port suppression
convention, unverified on Xbox; it would otherwise be an invalid handle), and waiting on the file
handle itself is unsupported. Host backend calls execute synchronously on
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
launch layout and its free, shared VFS, event signaling and owned dispatcher
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
extra stop policies 56–58 and 177, and the conditional VFS exports. Unknown
or unimplemented functions retain KernelCall events and unmodified frames. No
handler returns a placeholder success for unavailable behavior.

DATA exports other than KeTickCount and LaunchDataPage retain Runtime's zero
placeholders. ObReferenceObjectByHandle compares its type argument with the
addresses of DATA 259 (PsThreadObjectType) and 16 (ExEventObjectType), never
their contents. The
runtime has no data-read hook or no-access permission with which HLE could turn
those reads into a named stop. They are not configured console data or initialized
OBJECT_TYPE/IDE objects. This is a remaining integration limitation, not support.

The runtime review API migration is complete: private `runtime()`, Guest guard
helpers, `run_from`, owned `call_guest`, CpuAbi/GuestFault names and override
replacement semantics are used. No additional callback/resume API is needed.
The range API (`Guest::protect_range`, `unmap`, `Permissions::NONE`,
`reserve_physical`/`map_reserved`, `find_free_physical`/`physical_free`) is
used by 171, 182, 199, 170 and every physical allocator. Terminated thread
stacks are not yet reclaimed with it. The following changes are requested from
the owning lanes; none were made here:

```rust,ignore
// Runtime permissions: deny unsupported DATA reads (a read hook).
```

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

Validation on 2026-11-15, against the runtime-mem range API: `cargo test -p
kernel --all-targets` passed every test, with two local-disc tests ignored by
default. They were run separately with `H2_XBE=<absolute path> ... --ignored`
against the supplied retail XBE and passed: section accounting and the seeded
DATA exports, and under `LoadOptions::halo2()` that kernel placements avoid
the title arena and XPhysicalAlloc's request gets 0x80061000. Kernel Clippy with
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
The compatibility contracts in [LIFTER.md](../../../docs/lifter/LIFTER.md),
[kernel-imports.md](../../../docs/runtime/kernel-imports.md),
[Runtime API changes](../runtime/API-CHANGES.md), and
[VFS storage contract](../../../docs/runtime/storage.md) remain the integration references.
These moving references and synthetic tests are not hardware trace validation.

## Campaign cache integration

The ignored `campaign_cold_warm_cache_slots` gate lifts the supplied retail
copy/publication path and executes it through kernel/VFS/runtime against a
read-only ISO. It covers slots 4/5, SP-shared slot 1, their menu/shared
prerequisites, warm reopen, failure counts, and staging/resource-buffer
lifetime. See [cache-slots.md](../../../docs/runtime/cache-slots.md) for the command,
fixture boundaries, and remaining full-activation checks.
## Performance instrumentation

With `ExecCtx::enable_profiling(true)`, kernel adapters count and time every
invocation (including failed and stopped calls) in `perf::Scope::Kernel`.
The duration is inclusive host wall time, unrelated to the guest clock. See
[performance](../../../docs/perf/guest-memory.md) for aggregation and interpretation.
