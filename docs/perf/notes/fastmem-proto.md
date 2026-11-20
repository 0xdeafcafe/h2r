# fastmem write-watch prototype (Mach exceptions, macOS arm64)

Evidence for the write-watch question in [../guest-memory.md](../guest-memory.md); the prototype is `crates/recompiler/fastmem-proto`.

`crates/recompiler/fastmem-proto` is the isolated prototype that the fastmem design
("Write watch and fault handling", `docs/fastmem.md` on the `perf` branch)
asks for. It answers one question: can write watch move from a per-store
dirty mark to page protection plus a fault handler on macOS arm64, with 16 KiB
pages, without breaking crashes, debuggers or teardown? It has no
dependencies at all and nothing else in the workspace uses it.

The platform references are Apple's
[exception delivery](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/exception.c)
in xnu and the SDK headers `mach/exception_types.h`, `mach/exc.h`,
`mach/thread_act.h` and `mach/arm/thread_status.h`. No emulator code was used.

## Shape

- `WatchServer::start()` allocates a receive right and starts one handler
  thread (`fastmem-exc`, QoS user-interactive) that loops on `mach_msg`.
- `register(base, len)` adds a page-aligned region with a dirty bitmap.
  `arm` write-protects the region; `rearm_dirty` takes the dirty set and
  write-protects just those pages, one `mprotect` per contiguous run.
  Both assume writers are quiesced (the design's exclusive memory ownership).
- `attach_current_thread()` installs the server as the calling thread's
  `EXC_BAD_ACCESS` port (`EXCEPTION_DEFAULT | MACH_EXCEPTION_CODES`) and
  saves what was there. Dropping the attachment restores it.
- `src/bin/fastmem-probe.rs` runs scenarios in a fresh process, including
  ones that must crash, and the cost benchmark.

## Thread-level ports, not task-wide filtering

xnu triages an exception thread port first, then task, then host, and stops
at the first handler that replies `KERN_SUCCESS` (or whose port died). Any
other reply moves on to the next level, and when nothing handles it the
thread takes the BSD signal. The prototype installs **thread-level** ports
on the threads that run generated code:

- **Debuggers keep working.** lldb's debugserver owns the task-level port of
  the process it debugs and swaps it when it attaches. A task-wide handler
  would be displaced by attaching, or would have to fight debugserver for
  it. A thread port sits in front of debugserver: handled write faults never
  reach lldb, and everything we decline reaches it as a normal
  `EXC_BAD_ACCESS` stop.
- **Forwarding is the kernel's job.** Declining with `KERN_FAILURE` is all it
  takes to reach the task handler, crash reporter or signal that would have
  run without us. A task-wide handler has to relay every foreign fault by
  hand, including faults from Metal, audio and other runtime threads it
  never wanted to see.
- **Filtering is free.** Only attached threads come to us, so an unattached
  thread that writes an armed page crashes like it always did. That is the
  right failure for "host code wrote guest RAM without the host guard".

The cost is one attach per runtime thread. Threads that write guest RAM have
to be known anyway, because of the memory lease the design already requires.

If a thread already had its own thread-level handler, its saved port is
relayed to as a MIG `exception_raise` or `mach_exception_raise` RPC, and its
answer is passed back. Previous handlers that ask for state-carrying
behaviours (`EXCEPTION_STATE*`) are not relayed; the prototype declines and
lets the kernel fall through to the task level instead. The test
`foreign_write_is_relayed_to_the_previous_thread_handler` stacks two servers
on one thread to prove the relay.

## Deciding whether a fault is ours

A fault is a dirty write only when all of these hold:

1. the exception is `EXC_BAD_ACCESS` and `code[0]` is `KERN_PROTECTION_FAILURE`
   (a null or unmapped access is `KERN_INVALID_ADDRESS` and goes on);
2. `thread_get_state(ARM_EXCEPTION_STATE64)` reports ESR class 0x24 (data
   abort from EL0), with WnR set and CM clear;
3. `code[1]`, the fault address, lies in a registered region.

Then the handler sets the page's dirty bit, **then** `mprotect`s that one
page read-write, then replies `KERN_SUCCESS`. Mark-before-open means a drain
that finds the page writable also finds its bit. The thread resumes at the
unchanged PC, so the store runs again and succeeds. If `mprotect` fails, the
fault is declined rather than letting an untracked write through.

## arm64 retry semantics (proven by tests)

Each of these stores faults once on an armed page, is retried, and lands:
`str`, `strb`, `stp`, `str q` (NEON), LSE `ldaddal`, an `ldxr`/`stxr` loop,
`dc zva`, and a libc whole-page memset. LSE atomics and `dc zva` report
WnR = 1, so they count as writes. An unaligned 8-byte store that straddles
two armed pages faults once per page; the second retry completes the whole
store and both pages are dirty. Nothing in the retry path needed emulating
an instruction or advancing the PC.

## Rules the handler path keeps

Everything reachable from `serve` uses only atomics, Mach traps
(`mach_msg`, `thread_get_state`) and the `mprotect` syscall. It:

- takes no lock: region and thread tables are fixed arrays of atomics, and
  owner-side bookkeeping sits behind a mutex the handler never touches;
- does not allocate: the receive buffer is on the handler's stack, and the
  tables and bitmaps are allocated at registration;
- does not unwind: no indexing, `unwrap` or overflowing arithmetic, and the
  faulting thread itself never leaves the instruction it was on.

Teardown of a region or attachment clears its slot, then waits for an
in-flight counter the handler holds around each request, before freeing
anything the handler could read.

## Proven by the tests

`cargo test -p fastmem-proto` runs `tests/inprocess.rs` (thread-level only,
safe in the parallel harness) and `tests/process.rs` (child processes):

| Requirement | Evidence |
| --- | --- |
| Port installed per thread | `thread_bad_access_port()` equals the server's port while attached and is restored after |
| Write faults: mark, open, retry | every store kind above; values land, dirty sets match, `handled` counts are exact |
| Re-arm faults again | `later_writes_do_not_fault_until_rearmed`, `rearm_cycles_repeat` (100 faults over 50 cycles) |
| Foreign faults forwarded | relay to a previous thread handler; decline reaches a previous task-wide handler (`chain-task`) |
| Crashes stay crashes | null deref ends in SIGSEGV; write outside regions, read of a `PROT_NONE` region page, and a write from an unattached thread end in SIGBUS |
| Clean teardown and reinstall | five rounds leave the task's port-name count unchanged; reinstall on one thread five times |
| Concurrency | four attached threads fault on 16 pages at once |
| 4 KiB guest pages | one fault reports all four 4 KiB subpages of the host page; a re-armed sibling faults again |
| lldb | `works_under_lldb` (ignored by default) |

## lldb

Nothing special is needed. Under `lldb -- fastmem-probe all`, every handled
fault is invisible to the debugger and the process exits 0. A foreign fault
(`fastmem-probe unregistered-write`) stops lldb with
`EXC_BAD_ACCESS (code=2, ...)` on the faulting `str`, as it would without the
server. lldb never needs `process handle SIGSEGV/SIGBUS` settings, because
handled faults never become signals. Attaching lldb to a process that is
already running should behave the same, since debugserver only replaces
task-level ports, but only launch under lldb was tested.

## Measured cost

`fastmem-probe bench 256 40`, release build, M-series host at load average
about 90 on 10 cores (about 15 lanes building at once). Per-sample
percentiles, in ns:

| Pattern | p10 | p50 | p90 |
| --- | --- | --- | --- |
| first-write fault alone (page already re-armed) | 7 600 to 10 500 | 8 800 to 30 000 | 31 000 to 35 000 |
| re-arm one page (`mprotect`) plus its first-write fault | 9 000 to 12 500 | 12 300 to 34 000 | 37 000 to 39 000 |

Means over a run: `mprotect` read-only plus read-write on one page with no
fault, 1.2 to 2.9 µs. `rearm_dirty` of 256 contiguous dirty pages, 50 to 200
ns per page (one coalesced `mprotect`). A store to an open page, about 5 ns.

So a first-write fault plus its re-arm costs **about 9 to 12 µs** at best,
and 30 to 40 µs when the box is busy. The two modes are two thread
switches: the faulting thread blocks in the kernel while the handler thread
wakes, decides, and replies. Raising the handler's QoS did not visibly change
the busy-machine numbers.

Faulting every page of a region armed with one big `mprotect` cost more per
fault (46 to 120 µs per page, noisy). The likely cause, not yet confirmed
with a profile, is that each first write splits the VM map entry, so a fully
dirty region has one entry per page until `rearm_dirty` merges it again. Watched regions should be armed at the granularity the
consumers actually need, and re-armed with coalesced runs.

## 4 KiB guest pages on 16 KiB host pages

The Xbox has 4 KiB pages; this host has 16 KiB pages, and `mprotect` only
works on whole host pages. Write watch therefore protects and faults per
host page, whichever guest addressing scheme sits on top.

**What one fault means.** The first store into a protected host page marks
it dirty and opens all 16 KiB. Stores to the other three 4 KiB guest pages in
that host page then run without faulting until the next re-arm. So the
watch can only promise that every written guest page is reported, not that
only written pages are. `rearm_dirty_subpages(id, 4096, visit)` reports
every 4 KiB subpage of each dirty host page, and the test
`guest_4k_subpages_are_reported_conservatively` pins the contract: writes
to guest pages 1 and 3 (one fault) report pages 0 to 3, and a re-armed
sibling faults again. Mapping a host page back to guest pages is the
caller's job, through whatever reverse map it keeps. A subscriber that only
watches some of the four guest pages filters there. Splitting a host page
into four separately protected 4 KiB pages is not possible; calling
`mprotect` four times just protects the same host page four times.

The false-positive rate is bounded by layout. Watched guest data (vertex
buffers, textures, push buffers, sound buffers) is mostly larger than 16 KiB
and contiguous, so the extra uploads come from the ragged first and last
host pages of each buffer and from small objects that share a host page with
hot unwatched data. The latter is the worst case: a hot unwatched store next
to a watched buffer re-dirties it, and pays one fault, every epoch. The A/B
should count both the faults per frame and the extra uploads.

**Fixed guest base (the 16 KiB eligibility design).** Each guest page is
reached through host mappings of the backing, so one physical host page can
be visible at several host addresses. Write watch has to protect every
writable alias of a watched host page, which is the reverse map the design
already asks for. The handler then has to accept a fault at any alias and
mark the physical page. The prototype keys regions by host address, so this
means registering, or reverse-mapping, each alias. Guest pages whose 4 KiB
neighbours are not physically contiguous are already ineligible for direct
access, and they don't need faults: their stores go through the checked
path, which can mark exact 4 KiB dirt itself.

**4 KiB translation table (the variant the fastmem lane is measuring).**
Generated code looks each guest page up in a table of host pointers. All
entries point into one backing mapping of guest physical RAM, so there are
no host-level aliases. Protecting the backing host page covers every guest
virtual alias at once, and the fault address maps to a physical frame as
`(addr - backing_base) >> 12`. The same 16 KiB conservatism applies, because
the backing still has 16 KiB host pages.

The table also gives an exact alternative that needs no faults at all. Add
a write-permission bit to each 4 KiB entry, or keep a separate write table,
and clear it for watched pages. The first store to a watched page then takes
the checked slow path, which marks that exact 4 KiB page dirty and sets the
bit back. That costs a slow-path call, tens of nanoseconds, instead of a
roughly 10 µs Mach fault round trip, and it is exact per guest page. The
cost is that stores need their own lookup (or bit test), separate from
loads. The Mach service then only has to cover host code and anything else
that writes the backing directly without going through the table. If the
translation-table variant wins its measurement, prefer the table bit for
guest stores and keep page protection only as the backstop for direct host
writes, or drop it if every host writer marks dirt while holding the host
guard.

Either way, a store that crosses a 4 KiB guest page boundary goes slow per
the design, so the split-store retry above is a safety net for host code,
not a path generated code relies on. A crossing store within one host page
cannot fault twice in any case; there is only one host page to open.

## Consequences for the fastmem design

No blockers found:

- **SIP / hardened runtime:** setting a thread's exception port needed no
  entitlement and no SIP change for the ad-hoc-signed test binaries, with
  SIP on. xnu's `exception_deliver` has a `hardened` path that limits
  thread-state changes to the PC; the prototype never sets thread state, so
  it is unaffected. A hardened, notarised build should be re-checked before
  shipping, because newer xnu also has "identity protected" behaviours for
  hardened processes. In that case the `EXCEPTION_DEFAULT` task port is a read
  port (`convert_task_read_to_port` outside developer mode), which the handler
  never uses.
- **Retry semantics:** clean for every store kind tried (above).
- **Page size:** 4 KiB guest pages on 16 KiB host pages are workable, with
  host-page-granular, conservative dirt (section above). With a 4 KiB
  translation table, a write bit in the table is exact and far cheaper than
  a fault.
- **Budget:** at about 10 µs per first write per watched page per epoch, the
  design's "later stores run without marking until the next rearm" matters.
  A watch that re-arms hundreds of pages per frame costs milliseconds. Bulk
  host writes should keep pre-marking and unprotecting once per operation,
  as the design says, rather than faulting.

Open points the prototype leaves for the real implementation:

- One handler thread serialises all faults. That is enough for a handful of
  guest threads, but a second server, or a port set with more receivers, can
  come later.
- Drains assume quiesced writers. A concurrent re-arm would need the epoch
  scheme the design describes.
- Relaying to a previous thread handler that uses `EXCEPTION_STATE*`
  behaviours is not implemented. No such handler is expected on runtime
  threads.
- Crash scenarios leave macOS crash reports for the probe child, which is
  expected.
