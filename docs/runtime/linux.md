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
