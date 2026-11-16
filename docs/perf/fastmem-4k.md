# Opt-in 4 KiB translation experiment

Status: not on master. The experiment is on the `fastmem-4k` branch
(`894f2a3` over master `c00124d`); on master `H2_FASTMEM` takes only `0` or
`1`, and phase 1 (the inline RAM path) is on by default
([performance.md](performance.md), "Inline RAM path").
Master has since built its own boot bridge v3 (the `exit` slot,
[performance.md](performance.md#one-exit-call-and-a-callee-on-its-callers-view-2026-11-17)),
so the branch's installer v4 below would need rebasing onto it.

This extends phase 1 without changing the default memory path or the
guest's logical accounting. On the branch, `H2_FASTMEM=2` and the boot flags
in docs/runtime/BOOT.md select the experiment. No Mach protection is enabled by this change.

## Translation and lease

The eight-MB table maps each complete 4 KiB virtual page to an aligned 4 KiB
physical RAM offset and guest permissions. H2_LD/H2_ST inline the lookup and
load/store. Missing, partial, misaligned-backed and protected pages, MMIO,
invalid alignment, cross-page accesses and atomics retain checked helpers.
The checks preserve fault EIP and whole-span, all-or-nothing writes.

Mode 2 acquires the existing memory gate once across consecutive generated
accesses. A host MemoryGuard owns the Guest mutex before requesting the gate;
it sets a request bit if generated code owns a lease. Generated loop polls
release a requested lease, and memory paths check requests every 64 successful
accesses (also inside REP). Helpers, faults and exits release leases; the runtime
also releases on context memory borrows and native return. The host obtains
exclusive ownership before reading data, changing mappings or watch state.
No RAM pointer survives a host borrow without re-reading the shared view. The
view's new fields use a distinct acquire entry point and boot installer v4,
so an old boot host cannot interpret the extended view as the old layout.

Access accounting remains phase 1's exact per-access charging and exact budget
wake. It is tied to guest logical ticks, not presents. The access-count lane's
local accumulator is separate work; this experiment does not claim its savings
or introduce an accounting version or intentional schedule delta.

## Software first-write mode

Mode 3 tests a write-enable bit indexed by the translated **physical** 4 KiB
page. Maintaining this bitmap is activated only when mode 3 is requested. This extra physical bit test, instead of duplicating permission bits in
every virtual alias, makes aliases immediately coherent without reverse maps.
A set bit means at least one unmuted subscriber watches the page and has not
yet seen it dirty. The first store falls back to the checked helper, which marks
that exact page for every subscriber; later stores take no marking path until a
watch change or drain re-arms the bit. Guest write permission remains a separate
check and is never weakened. Independent drains, muting, unwatching, mapping
replacement and new subscribers all retain the existing watch contract.

Mode 2 instead accumulates dirty bits on every generated store and publishes
them at the next host guard, as phase 1 does. All host writes continue through
Guest's checked APIs under the host guard. Mode 3 needs neither host-page
protection nor conservative dirty pages while that contract is enforced.

## Composition with Mach write watch

The landed fastmem-proto uses thread-level exception ports and a handler that
marks before opening a faulted page, then retries the same host instruction.
Its 16 KiB granularity applies to the **physical RAM allocation**, not virtual
adjacency: four neighboring physical 4 KiB pages share one host page regardless
of which virtual table entries alias them. A future protected backend must
allocate RAM on 16 KiB host boundaries (the current Box allocation does not
promise that) and translate a fault's host address back to its physical offset.
A conservative dirty mark covers all four physical pages and their watchers.
The table still enforces each guest page's permissions, including aliases with
different permissions; Mach protection is only the dirty barrier.

Re-arm and watch-state changes require the same exclusive host guard/lease
handoff: no generated or host writer may race a protection epoch. The exception
handler cannot acquire that guard, since the faulting C thread holds the lease;
it only updates handler-owned atomic dirty state and protections. Bulk host
writes must pre-mark and unprotect coalesced physical ranges once under the
host guard. Attach each generated execution thread; do not fault arbitrary host
writers. Mode 3 can instead remain the normal write barrier, with page protection
reserved for detecting host code that violates the guard contract.

The census reports distinct physical 16 KiB pages first-written per frame.
Multiplying that count by 10–30 microseconds estimates faults plus re-arm if all
those pages were watched and re-armed once per frame. This is a model, not a
measured integration: actual watched subsets reduce it, multiple epochs per
frame can increase it, and bulk host writes are excluded and must pre-mark.
Multiplying 16 KiB page counts by four versus exact 4 KiB counts estimates dirty
page amplification, **not actual extra uploads**. Only integration with D3D and
DSound can measure upload bytes; the software-bit variant has no granularity
amplification.

## Measurement controls

After the rush restart, all prior timings are discarded. Two images are built
before timing: `H2_FASTMEM=2` for translated modes 2/3, and `H2_FASTMEM=1` for
phase-1 inline and checked access without experimental branches. The serial
order is translated, write-bit, phase-1, checked, then the reverse. Both images
use the same host executable, IR passes, clang O1/line tables, merge-pass setting,
input timeline and 480–560 present window. `H2_ACCESS_COUNT=0` and
`H2_X87_NATIVE=0` hold those independently owned optimizations constant. No own
build overlaps the measured windows. The physical-page census is separate.

The landed opt-in access counter is compatible: counted slow callbacks remain
uncharged and fast copies do not double-charge them. With accounting enabled,
the host-request check currently runs on each fast access, since the old view
access count remains zero. Its overhead should be measured with the local
counter before changing this handoff check. The comparison here retains the
phase-1 accounting policy to isolate the memory algorithm.

## Fresh census and build evidence

On branch `894f2a3` over master `c00124d`, the post-restart menu census
covered presents 480–500 (20 frames). There were 54,403,085 accessor operations,
or 2,720,154 per frame. The translated 4 KiB path covered 54,402,739
(**99.999364%**); 100 crossed guest pages and 246 touched partial spans.
The original contiguous 16 KiB eligibility test covered only 61.175599%.
These counts cover generated/helper accessor traffic, not host HLE writes.

Summing distinct physical pages separately for each frame gave 5,309 written
4 KiB pages and 2,564 written 16 KiB groups: **265.45** and **128.2** per frame.
If all these groups were watched and re-armed once per frame, the Mach model
is **1.282–3.846 ms per frame** at 10–30 microseconds per first write.
Conservative marking would cover 1.93 times as many 4 KiB pages; this is a
dirty-page model, not measured GPU upload traffic. The software-bit variant
retains exact 4 KiB dirty tracking. First-level coverage is not yet measured.

Both fresh images compiled all 1,001 objects (zero cache reuse). The translated
image used 378.3 seconds of clang wall time and 373.1 MB of objects; phase 1
used 328.2 seconds and 321.4 MB. This single loaded-machine pair is a 15.3%
compile-time increase and 16.1% object-size increase, not a controlled build
benchmark. Both emitted 505.1 MB of C. The translated stage's total elapsed
time was 1,213.5 seconds, including substantial gate/disk contention; that
total is not compiler time.

## Serial menu comparison

All eight fresh runs completed successfully. The code under test is `894f2a3`
on `c00124d`; subsequent commits only record evidence. Each run measures 80
presents (480–560), with fresh game data, merged passes and no PNG output.
CPU is process user plus system time across all threads, divided by presents.
Rows below are medians of **two run averages**, not per-frame medians or a
20-sample confidence estimate. FPS is presented frames per wall second.

| Path | CPU ms/frame, runs | Median CPU ms/frame | FPS, runs | Median FPS |
| --- | --- | --- | --- | --- |
| Checked | 139.885, 172.762 | 156.323 | 7.056, 3.334 | 5.195 |
| Phase-1 inline | 88.326, 82.584 | 85.455 | 11.140, 11.865 | 11.502 |
| 4 KiB translation + lease | 117.006, 92.108 | 104.557 | 5.697, 10.682 | 8.190 |
| 4 KiB + first-write bit | 117.968, 91.450 | 104.709 | 5.703, 10.863 | 8.283 |

Phase-1 inline access reduced median process CPU/frame by **45.3%** against
checked access in this run set. Translation reduced it by 33.1% against checked,
but **increased it by 22.4% against phase 1**. Adding the first-write bit changed
translation's median by only +0.15%; there is no demonstrated timing benefit
from that variant. It does preserve exact 4 KiB watch semantics. These results
do not justify enabling either translated mode by default.

Machine contention changed markedly: the translated runs ranged from 92.1 to
117.0 ms CPU/frame, and checked runs from 139.9 to 172.8. Even process CPU is
not immune to frequency, cache and scheduler effects. In the later, adjacent
phase-1/write-bit/translation runs, the two translated modes still used about
10.7%/11.5% more CPU/frame than phase 1. Thus the present evidence favors phase
1, without establishing a precise regression size or its cause. There is no
new sample profile attributing that regression to a specific instruction.

The second phase-1 job spent substantial time waiting for disk headroom and a
bench slot (854 seconds total job time); its measured window was 6.743 seconds.
Gate and boot time are excluded from the reported frame windows. All earlier
pre-restart timing files were excluded. No image compilation from this lane
overlapped these windows, but other lanes' builds and benches did.

Validation on the measured code: **425 tests passed, 16 ignored**, across
runtime, emit, boot and kernel; strict all-target clippy passed for those
crates. The new tests cover translated and checked fault behavior, aliases,
watch drains and re-arm, host handoff, guest replacement, counted macros and
the installer version boundary. The landed P1 raw-pointer ownership fix is
preserved. This menu exercise does not establish first-level/gameplay or
x86_64 parity, and does not measure graphics fidelity with merged passes off.

## Remaining work toward 30 fps

This comparison's best path is phase 1 at 11.50 median wall FPS and 85.46 ms
process CPU/frame; neither is a quiet-machine guarantee. Thirty FPS allows
33.33 ms of wall time per presented frame. At comparable parallelism, the CPU
figure suggests roughly 2.56 times more throughput is needed; process CPU is
not itself a wall-time bound. The memory target remains roughly 20 ms per
frame, about 7.35 ns per access at the census's 2.72 million accesses/frame.
We did not re-attribute total frame time by subsystem, so the remaining owner
budgets below are requirements, not newly measured allocations.

| Owner | Next evidence/change needed |
| --- | --- |
| Runtime/emit | Retain phase 1 as the faster path (it has since become the default). Profile the additional lease/mode checks before changing them, then compare specialized translated code under matching load. Fit memory work within about 20 ms; high eligibility alone has not achieved this. |
| IR / codegen-opt | Measure the landed local access accumulator jointly with memory paths; it was disabled in this isolation experiment. Accounting stays tied to logical guest ticks, not presentation frequency. Continue IR work independently. |
| wgpu / gfx-perf | Merged passes were enabled on both sides. Measure remaining submission/upload CPU and GPU critical-path time; this experiment does not isolate them. |
| D3D parsing | Measure remaining parser time with the selected memory path. Software bits add no page-granularity dirty amplification; actual upload bytes were not measured. |
| Kernel | Measure residual scheduling/HLE cost once accounting is enabled. Preserve deterministic logical scheduling and the host memory guard contract. |
| Build-perf | Investigate the zero object-cache hits and translated image's 15.3% clang-time increase; timing came from one loaded pair. |

The Mach estimate above is only a model for an alternative backend. No Mach
handler or host-page protection is installed by these modes. There is no
evidence here that adding it would recover the translated path's regression.
