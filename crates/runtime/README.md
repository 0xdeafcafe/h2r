# Runtime ABI v1

Generated C11 includes `include/h2abi.h` and returns explicit transfers to the
Rust runner. Registers and memory addresses keep their guest widths. Ext80
registers and XMM registers are bytes, so the ABI does not depend on native
floating point or vector layouts.

| Type | Bytes | Alignment | Offsets |
| --- | ---: | ---: | --- |
| CpuAbi | 288 | 4 | GPRs 8, EIP 40, FS 44, EFLAGS 48, validity 52, x87 56, XMM 156, MXCSR 284 |
| X87Abi | 100 | 4 | registers 0, control 80, status 82, tag 84, opcode 86, FIP 88, FDP 92, selectors 96/98 |
| TransferAbi | 28 | 4 | version 0, size 4, kind 8, route 12, target 16, continuation 20, event 24 |

Generated objects exported for Rust registration use
`H2_DEFINE_GENERATED_ABI_NAMED(name)` with a unique symbol per object. The
zero-argument `H2_DEFINE_GENERATED_ABI()` form is translation-unit-local, so
multiple objects can use it safely inside C. Pass the named probe and handler
to `Dispatch::register_generated_object`. Runtime version queries use the
separate `RuntimeAbiVersion` identity; the Rust convenience query cannot coerce
to a generated C probe. The native `h2abi_v1_version` scalar ABI is unchanged.
Registration compares the object's compiled-in version with the runtime's.
The older explicit-version registration remains available. Duplicate generated
entries and resumes fail. Registering an override returns the displaced
`Override`, which can be restored with `restore_override`.

`Runner::run` enters a function. `run_from` accepts an explicit transfer after a
stop: use a Return for a guest return, or an explicit Resume for a generated
continuation. The exact resume map bypasses entry overrides; an entry transfer
applies them. Dispatch requires executable guest memory except for recognized
kernel tokens. Only Return to `HARNESS_EXIT` terminates the harness. Generated
code must spill the current EIP before an observable operation; otherwise a
handler's diagnostic PC is its entry PC. Native handlers must not retain CPU
or context pointers or unwind through a C frame.

Rust overrides save callback state in an owned continuation closure. With a
memory guard already held, `ctx.call_guest(cpu, &mut memory, target, closure)`
pushes a distinct guest return token and yields an ordinary Call. On Return to
that token the runner consumes the closure. Its captured state survives kernel
stops and later `run_from` calls. It does not use host recursion or process-exit
tokens. There are 1024 slots, with FIFO reuse and a generation in every token.
The control-only range is `0xb0000000..0xf0000000`; tokens use a 10-bit slot and
20-bit generation and need no PC alignment. A stale token cannot run or cancel
a newer closure in the same slot. Generations never wrap: slots retire at their
last generation, and exhaustion reports ContinuationLimit. The finite namespace
has over a billion distinct identities; this fails closed instead of allowing
old tokens to alias new state. Use `cancel_continuation(token)` after an
unwind or `clear_continuations()` during thread termination. The token is the
Call transfer's `resume_pc`. These APIs drop saved Rust state but do not repair
the guest stack. Slot reuse does not make abandoned token copies valid again. JUMP to
a live token cancels it and faults, while only RET invokes it. Release the
memory guard before handing the transfer to the runner.

`ExecCtx::runtime()` exposes shared services; `ExecCtx::memory()` borrows the
context for the lifetime of its memory guard. Reads, writes, `call` and `ret`
are Guest methods on that guard. These helpers acquire no additional locks.
Do not hold any Runtime memory guard while entering a runner or callback.
Every generated C access uses the same backing mutex as HLE. Atomic RMW,
XCHG, CMPXCHG and CMPXCHG8B hold it for the entire operation, including flags
and results. Atomic outputs may alias CpuAbi fields: the CPU borrow ends before
the final output store, which wins if it targets a field modified by the helper.
Output must remain disjoint from ExecCtx. Scalar widths are 1/2/4 bytes; CMPXCHG8B uses eight. XADD uses ADD
and the returned old operand. Bit operations use the decoded effective word
address. Ordinary accesses allow 1/2/4/8/10/16 bytes and explicit alignment.
Failure leaves memory and output unchanged and records an access fault.

Memory has 64 MB of physical backing. Sorted mappings preserve aliases and
holes; accesses use binary search without allocating. Cross-mapping writes
validate the entire span before changing bytes. NV2A addresses report an
unsupported-device fault. The loader maps page-rounded headers and sections,
combines shared-page permissions, and honors each section's head/tail read-only
flags. The union can make an RX section share an RWX page with writable data or
headers. This is intentional for Halo 2's packed sections; each widened page
is recorded in `LoadedImage::permission_notes` with its effective permissions
and contributing regions' requests. Unused mapped page bytes start zero. TLS is read from loaded memory;
its index is validated after protections are final. `tls_data_size` records
aligned payload bytes plus the leading pointer slot. Guest startup still
performs its own TLS stores.

Kernel stop handlers preserve the ordinal, name, CPU, return word and fixed
stack argument words. DATA thunks point at shared guest storage; opaque object
storage and key cells remain unimplemented zeroed placeholders. Stack top
`0x70000000`, DATA base `0x80000000` and dispatch-token ranges are harness
choices. Full TLS/KPCR setup, callbacks listed in an XBE directory and kernel
service behavior remain outside this loader milestone.

A context retains at most 1024 recent events, evicting older diagnostics without
stopping execution. `take_events()` returns owned `(id, event)` records after a
stop; drain before eviction when complete logging is required. IDs remain valid
until drained or evicted, are not reset on drain, and wrap past zero and live
IDs. The first failing FFI access is pinned through eviction and drains until
its invocation propagates it, even if native output is malformed. Generated code
must stop at that first failure; subsequent helper faults from a broken handler
are ordinary, evictable diagnostics. HarnessExit is recorded normally and may
later be evicted or drained; it is not permanently pinned. EventLimit remains
as an API variant but retention no longer emits it. An unrecorded Fault gets an
explicit event.
The runtime denies unsafe code except in `ffi.rs`. The C probe is a development
dependency, so ordinary runtime builds do not compile or export test handlers.

Tests cover layouts and C struct returns on both Macs, page sharing, aliases,
atomics and flags, callback continuations, diagnostics and import metadata. No
nxdk export-definition file with `@N` decorations was available offline during
this review. Stack-word counts are checked mechanically against the documented
signatures; that is not an independent decoration check. The ignored local-disc
test reads `disc/default.xbe` directly without copying it:

```sh
cargo test -p runtime --test loader -- --ignored
```
