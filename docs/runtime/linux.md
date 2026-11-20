# Linux x86-64 game module

Status: Built. A reference for the Linux x86-64 module and the headless host build.
Code: `crates/emit/src/compile.rs` (`Arch::X86_64Linux`, `linux_target_args`), `deploy/host/build.sh`.
Switch: `H2_LINUX_SYSROOT`, `H2_OPT`, see [switches.md](switches.md).

The dedicated host runs an ELF64 shared object, `libh2game.so`, built for
`x86_64-unknown-linux-gnu`. The native handlers and the Rust host use the
System V AMD64 C ABI. The image uses baseline x86-64 instructions. The
`--arch x86_64-linux` selector chooses this target even on a Mac. On a
64-bit Intel or AMD Linux machine, `--arch host` selects it automatically.

## Headless host build

From the repository root, cross-build the Linux x86-64 `h2` binary with
cargo-zigbuild and Zig.

```sh
cargo zigbuild -p cli --no-default-features --features host --target x86_64-unknown-linux-gnu
```

The binary supports `h2 serve --system-link --host-light`, `h2 host run`,
`h2 sim` and `h2 agent`. Supply the matching Linux `libh2game.so` and the
owner's game data separately. See the
[dedicated-host build contract](../live/dedicated-host-build.md#10-linux-x86-64-build-owned-by-dh-linux-).
Keep `H2_OPT` consistent with the module build.

The `host` feature keeps the software D3D recorder, the DirectSound clock,
the Bink movie-start skip stub and the guest input model (including bots
and agent-injected input). The default `desktop` feature adds the windowed
app, wgpu presenter and native GPU passes, physical controllers, cpal
devices, movie playback, app icon and editor client commands.

## Cross-build on macOS

Use LLVM clang, ld.lld and llvm-ar on the Mac, with an **amd64 Debian
sysroot from the release used by the host container**. The emitter already
uses clang's driver and dependency scans for its object cache. Using the
container's headers, startup objects and libraries also pins the glibc ABI
used at link time. A usable sysroot contains glibc development files
(including libm), Linux headers and the target GCC startup and support
files under `usr/lib/gcc/x86_64-linux-gnu/<version>/`. Preserve Debian's
multiarch directory layout and ensure library symlinks resolve inside the
sysroot.

With clang, ld.lld and llvm-ar on PATH, and `H2_LINUX_SYSROOT` set to the
absolute sysroot directory, run the following with the Mac build of `h2`.

```sh
h2 build disc/default.xbe --arch x86_64-linux --tier clang --out "$TMPDIR/h2-linux"
h2 build disc/default.xbe --arch x86_64-linux --tier clang --out "$TMPDIR/h2-linux" --check
```

`H2_EMIT_TARGET=x86_64-linux` selects the same target for `h2 build`.
Clang receives `-target x86_64-unknown-linux-gnu`, `--sysroot=<dir>` and
`--gcc-toolchain=<dir>/usr` for compilation and linking. A native Linux
build can leave the variable unset and use its installed development
toolchain. The fast tier uses the running process's architecture. Use the
clang tier for a cross-build.

Ship `libh2game.so` and its adjacent `libh2game.so.stamp` together. Build
the host and the module from the same generator revision and with matching
code-generation settings, including `H2_OPT`. The stamp includes the
target triple. The sysroot path is specific to the build machine.

## Linking and loading

The Linux clang config selects `-fPIC -fvisibility=hidden`. Generated
handlers are reachable through the exported `h2gen_module` registry.
The boot host also looks up `h2gen_module_abi_version`,
`h2boot_install_v3` and `h2x87bridge_install`. LLVM's archiver writes a
GNU archive of the shards. For the loadable image clang invokes lld with
`-shared`, sets the SONAME to the published basename, and links libm.

The boot and x87 bridges are linked into the module. The host installs
their Rust callbacks through the bridge tables. The shared link also
permits undefined host imports. ld.lld's shared-output defaults permit
references from the module's objects, and `--allow-shlib-undefined`
permits unresolved references in input shared libraries. At load time
libloading requests `RTLD_NOW | RTLD_LOCAL`. A missing import therefore
fails the load immediately, and module symbols stay local to its handle.

The module defines the generated code's `h2abi_v1_*`, `h2shim_v1_*`
and `h2x87_v1_*` shims as hidden ELF symbols. The shipped bridge tables
work without exporting symbols from the executable. To support modules that import a host function **by name**,
the Linux **h2 executable** must export that function from its dynamic
symbol table. Pass `-rdynamic` to its final C linker driver to export the
executable's visible globals, or export the individual symbol. With a
clang or GCC linker driver, the CLI build script
can emit `cargo:rustc-link-arg-bin=h2=-rdynamic` for the Linux target.
A directly imported Rust function needs an unmangled C ABI name and
default visibility, and its defining object must be linked into the
executable. Check the deployed binary's defined dynamic symbols with
`nm -D --defined-only h2`. Bridge callbacks passed by address continue
to use the installed tables.

## Module checks

This lane has not built or run the Linux image. On the coordinator's
cross-build, inspect the ELF header, SONAME, dependencies and exported
bridge entry points with `readelf -h -d -Ws libh2game.so`. On the target
Debian image, check the stamp, load with the release headless `h2`, and
exercise the bridge, x87 helpers and generated dispatch by booting to a
joinable lobby. Use the game agent API to observe the lobby on that run.

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
