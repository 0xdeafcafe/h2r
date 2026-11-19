## Runtime

### Live sockets and native ABI checks

On Linux, XNet's low-delay UDP option sets `IPPROTO_IP/IP_TOS` to `0xb8`.
The Linux test reads the traffic class back from a datagram socket. Routing and
queue priority depend on the network.

The runtime C ABI test fixtures use the target C compiler, sysroot flags and
archiver selected by `cc` (including target-specific `CC`, `CFLAGS` and
`AR` for a cross-build). The layout test checks the Linux x86-64 triple
using Clang's freestanding headers. A Linux link test calls the ABI version
probes in both C objects.

The development-only SIMD harness reads MXCSR from glibc's x86-64 signal
context. The SSE oracle builds an ELF runner with Clang and launches it
directly. Its instruction page alternates between writable and executable
permissions. JSONL observations use backend `linux-x86_64`. The compact
fixture file keeps its Rosetta provenance.

On an x86-64 GNU/Linux development machine, run the runtime, SIMD, SSE oracle
and Live HLE tests with Clang installed. The oracle's protocol test exercises
register preservation, consecutive instruction changes, and recovery after an
invalid instruction. In the deployment image, check UDP discovery and a client
join with `H2_XNET_LOWDELAY=1` through the game agent API.

### Guest RAM and write faults

On x86-64 Linux each `Guest` reserves 64 MiB of anonymous physical RAM
between two inaccessible host pages. The runtime asks for the reservation
at `0x40000000` with `MAP_FIXED_NOREPLACE | MAP_NORESERVE`. If that
address is occupied, it tries `MAP_32BIT`, then lets the kernel choose
any free address. Generated code receives the actual RAM pointer. Guest
addresses, aliases, permissions and MMIO still go through the Xbox page
table, whose pages are 4 KiB. Linux snapshots copy the RAM bytes into a
separate guarded reservation. [Runtime storage](../../crates/runtime/src/cow.rs)

Game writes use the runtime's software dirty marks. The separate
`fastmem-probe` executable exercises hardware write watching in its own
process. On Linux it arms host pages read-only and installs a `SIGSEGV`
action. For a write from the watched thread to an armed page, it records
the fault address and RIP, opens one page and retries the instruction.
Foreign faults return to the previous signal disposition. The probe's
child-process tests cover cross-page writes, rearming, teardown, and
the signal delivered for an unhandled access.
[Fault probe](../../crates/fastmem-proto/src/bin/fastmem-probe/linux.rs)

### Host clocks and workers

The kernel pacer uses absolute `CLOCK_MONOTONIC` deadlines and retries
`clock_nanosleep` after an interrupt with the same deadline. Guest timer
interrupts and `rdtsc` continue to follow the logical Xbox clock. Native
workers receive their OS names from Rust's named thread builder. Linux
reports wall time with the monotonic clock and CPU usage with the thread
and process CPU clocks. The stack-sampling profiler reports unsupported
on Linux. The dedicated host executes the ahead-of-time game module and
keeps its physical RAM mapping read/write. [Kernel pacer](../../crates/kernel/src/pacer.rs)
[CPU counters](../../crates/timeline/src/host.rs)

For integration, run the runtime RAM, fastmem probe, kernel pacer and
perf clock tests on x86-64 Linux. Then boot the actual deployment image
to a joinable lobby and check `h2 agent hostmetrics` during a paced match.
Compare CPU and RSS against the host budget and wall time against the
33.3 ms gameplay tick. A cross-build can check types and linking. The
kernel's choice of mappings, signal frames, syscall permissions and
scheduler latency need these native checks.
