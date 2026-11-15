# Public API changes from the independent runtime review

The original review below used ABI_VERSION 1; the current generated-code
contract is ABI v2 (see emit/RUNTIME-CONTRACT.md). CpuAbi, X87Abi, TransferAbi, GuestFn and all existing C
accessor signatures retain their layouts and calling convention. Existing root
exports and paths under runtime::exec remain available except the aliases named
below. No files in crates/kernel were changed by this work.

## Context and stack access

- `ExecCtx::runtime: Arc<Runtime>` is private. Its replacement is
  `fn runtime(&self) -> &Arc<Runtime>`; change `ctx.runtime` to `ctx.runtime()`.
- Added `fn ExecCtx::memory(&mut self) -> MutexGuard<'_, Guest>`.
  Its borrow prevents simultaneous context mutation or runner entry. Keep an
  existing guard when doing several operations and release it before dispatch.
- Removed `ExecCtx::{read_u32, write_u32, read<const N>, write<const N>, call, ret}`.
  Use `Guest::{u32, set_u32, read, write}` on a memory guard. Those four Guest
  methods already existed. Stack operations are now these added Guest methods:

```rust
fn Guest::call(&mut self, cpu: &mut CpuAbi, target: u32, resume: u32)
    -> Result<TransferAbi, GuestFault>;
fn Guest::ret(&self, cpu: &mut CpuAbi, cleanup: u32)
    -> Result<TransferAbi, GuestFault>;
```

Passing a guard's Guest into a helper acquires no second lock. `call_guest`
also takes the caller's memory rather than locking it internally. Code using a
cloned Runtime must still release its guards before running or invoking a guest.

## Transfers and Rust continuations

Added:

```rust
fn Runner::run_from(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx,
    initial: TransferAbi) -> TransferAbi;
fn ExecCtx::call_guest<F>(&mut self, cpu: &mut CpuAbi, memory: &mut Guest,
    target: u32, continuation: F) -> Result<TransferAbi, GuestFault>
where F: FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<TransferAbi, GuestFault>
    + Send + 'static;
```

`run` still means entry dispatch. `run_from(..., TransferAbi::ret(pc))` means a
return, including exact generated resume lookup. `call_guest` stores owned Rust
state until Return to its one-shot token. A kernel stop retains the continuation;
resume with the actual kernel return transfer. Callback tokens are distinct from
process exit and their range is reserved by the loader.

Only Return to HARNESS_EXIT exits the harness. Call/Jump to that address use
ordinary dispatch and execute permissions. Resolved dispatch destinations must
have Execute permission, except recognized Xbox kernel function tokens and live
Rust continuation returns. Synthetic HLE return gates need executable mappings
or should use call_guest's continuation contract.

## Dispatch and generated-object registration

- `Dispatch::register_override` has the same arguments and generic handler bound,
  but returns `Option<dispatch::Override>` containing the displaced override.
  Ignoring that result preserves the former replacement behavior.
- `unsafe Dispatch::register_native_override` has the same arguments but returns
  `Result<Option<dispatch::Override>, RegistrationError>`.
- Added public, Clone `dispatch::Override`; its public fields are `name: String`
  and `enabled: bool`. The stored handler remains private.
- Added `fn Dispatch::restore_override(&mut self, addr: u32, previous: Override)
  -> Option<Override>`, returning the override it displaces.
- Existing `register_generated` now rejects duplicate entries. Existing
  `register_resume` rejects duplicate resume PCs. Added RegistrationError variants
  `DuplicateGenerated(u32)` and `DuplicateResume(u32)`.
- Added `ffi::GeneratedAbiVersion = unsafe extern "C" fn() -> u32` and:

```rust
unsafe fn Dispatch::register_generated_object(&mut self, addr: u32,
    object_version: GeneratedAbiVersion, handler: GuestFn)
    -> Result<(), RegistrationError>;
```

This calls the compiled object's version probe. The older explicit-version
`register_generated(addr, version, handler)` remains; its version must describe
the compiled object, not be fetched from the runtime.

## Atomics

Added `atomic::AtomicOp`, a repr(u32) Rust enum with tags Add=1, Adc=2, Sub=3,
Sbb=4, And=5, Or=6, Xor=7, Inc=8, Dec=9, Neg=10, Not=11, Bts=12, Btr=13, Btc=14.
The native ABI receives integer tags, not Rust enum values.

Added Guest methods, intended for use under one Runtime memory guard:

```rust
fn Guest::atomic_rmw(&mut self, cpu: &mut CpuAbi, addr: u32, width: u32,
    op: AtomicOp, operand: u32) -> Result<u32, GuestFault>;
fn Guest::atomic_xchg(&mut self, addr: u32, width: u32, value: u32)
    -> Result<u32, GuestFault>;
fn Guest::atomic_cmpxchg(&mut self, cpu: &mut CpuAbi, addr: u32, width: u32,
    replacement: u32) -> Result<u32, GuestFault>;
fn Guest::atomic_cmpxchg8b(&mut self, cpu: &mut CpuAbi, addr: u32)
    -> Result<(), GuestFault>;
```

Widths are bytes, 1/2/4; bit operations require 2/4. The first three return the
old zero-extended memory operand. XADD uses Add and that old result. CMPXCHG uses
AL/AX/EAX; CMPXCHG8B uses EDX:EAX and ECX:EBX.

Added unsafe extern C Rust exports (also declared in include/h2abi.h):

```rust
h2abi_v1_atomic_rmw(cpu: *mut CpuAbi, exec: *mut ExecCtxOpaque,
    pc: u32, addr: u32, width: u32, op: u32, operand: u32, out: *mut u32) -> u32;
h2abi_v1_xchg(cpu: *mut CpuAbi, exec: *mut ExecCtxOpaque,
    pc: u32, addr: u32, width: u32, value: u32, out: *mut u32) -> u32;
h2abi_v1_cmpxchg(cpu: *mut CpuAbi, exec: *mut ExecCtxOpaque,
    pc: u32, addr: u32, width: u32, replacement: u32, out: *mut u32) -> u32;
h2abi_v1_cmpxchg8b(cpu: *mut CpuAbi, exec: *mut ExecCtxOpaque,
    pc: u32, addr: u32) -> u32;
```

All return zero or a recorded fault id. On fault memory, CPU and output remain
unchanged. Added C macros H2_ATOMIC_ADD/ADC/SUB/SBB/AND/OR/XOR/INC/DEC/NEG/NOT/
BTS/BTR/BTC with the tags above. Added C declaration
`uint32_t h2gen_abi_version(void)` and definition macros
`H2_DEFINE_GENERATED_ABI()` / `H2_DEFINE_GENERATED_ABI_NAMED(name)`.
The generated object defines these probes; the runtime does not export them.

## Diagnostics, loading, and remaining surface changes

- Added GuestFault reason variants InvalidWidth, InvalidBuffer and InvalidOperation.
  Bad C widths, operation tags and null buffers now record access faults.
- Added Event variants `UnrecordedFault { pc: u32 }`, `ContinuationLimit`,
  and `EventLimit`. Fault with event_id zero gets an UnrecordedFault record.
  The first latched access fault retains priority over invalid native output.
- Added `events::MAX_EVENTS: usize = 1024`. Event ids remain stable; the last slot
  records EventLimit and later execution faults. Contexts also bound outstanding
  Rust continuations to 1024 and stop on continuation token exhaustion.
- `ExecCtxOpaque` now has PhantomData<(*mut u8, PhantomPinned)> and is therefore
  !Send, !Sync and !Unpin. It remains an incomplete C type, used only by pointer.
- `ffi::with_context` is crate-private. External Rust overrides should use the
  safe registration adapter; it supplies references directly.
- Removed `runtime::Cpu` / `cpu::Cpu` and `guest::Fault` aliases. Use CpuAbi
  and GuestFault.
- Added `LoadedImage::tls_data_size: Option<u32>` for aligned payload bytes plus
  the leading pointer slot. TLS index writability is checked after protection.
- Added `xbe::Xbe::tls_directory(&self) -> xbe::Result<Option<TlsDirectory>>`.
  Existing Xbe::tls is a best-effort file-backed parse: None also means the
  directory was not file-backed. Xbe::parse no longer fails for that reason.
  The runtime loader parses the actual loaded bytes instead.
- Loader mappings/protections are now page-granular, combining shared-page
  section contributions and head/tail read-only flags. Logical section byte
  overlap remains invalid. Unused mapped padding is zeroed.
- Added public modules `atomic`, `events`, `runner`, `services`. Event/KernelCall
  are available from events, Runner/HARNESS_EXIT from runner, Runtime from
  services. Existing runtime::exec re-exports and root exports remain valid.
- The C probe moved into the runtime-test-probe dev dependency. Production
  runtime builds have no Clang/ar probe build and no h2_test_c_* symbols.

## Follow-up: long-lived contexts and native probe safety

This section supersedes the earlier lifetime cap and probe-linkage descriptions.
The additions below leave kernel registration/callback signatures and all native
C entry point signatures unchanged.

- Added `ExecCtx::take_events(&mut self) -> Vec<(u32, Event)>`. It acknowledges
  retained records and returns their IDs with owned events. An unreported,
  latched FFI fault remains in the context until its transfer is propagated.
- MAX_EVENTS now bounds retained history rather than context lifetime. Older
  records are evicted without a fault. HarnessExit is recorded normally, so
  retention cannot replace a new exit with EventLimit; it can later be evicted
  or drained like other events.
  `event(id)` returns None after eviction/draining. IDs are not reset by taking
  events; the counter wraps past zero and any retained IDs. Consumers must use
  `event(id)`, not treat an opaque ID as an index into `events()`.
- EventLimit remains public for source compatibility but history retention no
  longer emits it. Runner and handler invocation no longer gate on event count.
- Completed and cancelled continuation slots are recycled with new generations,
  as described below. There are 1024 slots; exhausted generations retire a slot
  rather than reissuing an identity. Failed guest stack pushes also release
  their reserved slot.
- Added `ExecCtx::cancel_continuation(&mut self, token: u32) -> bool`, returning
  whether a saved continuation was dropped, and
  `ExecCtx::clear_continuations(&mut self) -> usize`, returning the count dropped.
  Use the Call transfer's resume_pc as the token. These APIs immediately drop
  owned Rust state; they do not unwind guest frames or remove stack words.
  This earlier caller-duty restriction is superseded by the generation-checked
  identities described below; abandoned guest stack frames still need unwinding.
- JUMP to a live Rust continuation cancels it and emits UnsupportedTarget; it
  never invokes Rust saved state. RET remains the only callback return contract.
- Atomic C accessors now permit their u32 output pointer to alias a CpuAbi field.
  They finish and release the operation's Rust CPU/context borrows before the
  output store. That final store wins over any instruction update to the same
  field. Output still must be disjoint from ExecCtx. On fault no output is stored.
- Added `loader::PagePermissionNote` (Debug, Clone, PartialEq, Eq), with public
  fields `page: u32`, `effective: guest::Permissions`, and
  `contributors: Vec<(String, guest::Permissions)>`.
- Added `LoadedImage::permission_notes: Vec<PagePermissionNote>`. One note records
  each shared page whose union grants more permissions than at least one region
  requested. Headers are named "headers"; sections use their XBE section names.
  The page union remains intentional, including RX + RW yielding RWX.
- `H2_DEFINE_GENERATED_ABI()` now defines a translation-unit-local static inline
  h2gen_abi_version, preventing duplicate symbols across generated objects.
  Exported probes use the existing H2_DEFINE_GENERATED_ABI_NAMED(unique_name).
  The old unconditional external h2gen_abi_version declaration was removed;
  the named definition supplies its own declaration.
- Added `ffi::RuntimeAbiVersion`, a Copy/Debug/PartialEq/Eq repr(transparent) u32
  identity with `const fn get(self) -> u32`, and
  `ffi::runtime_abi_version() -> RuntimeAbiVersion`.
- The public Rust convenience call `ffi::h2abi_v1_version() -> u32` remains
  available, but now uses the ordinary Rust calling convention. The actual C
  symbol h2abi_v1_version is provided by a private typed export with exactly
  the same native scalar ABI as before. Neither public runtime query can coerce
  to GeneratedAbiVersion; compile-fail tests enforce this intentional restriction.
  Ordinary Rust calls and all C callers retain their source usage. The generated
  probe type and register_generated_object signature remain unchanged.

Regression tests exceed the old 262,144-call lifetime, churn more than 1024
kernel stops, drain/wrap event IDs, preserve pinned faults, interleave/nest
continuations, release cancelled captures, alias atomic CPU outputs from C,
and link default version probes in two translation units.


## Follow-up: generation-checked continuation identities

No public function signatures, native layouts, or ABI version changed.

- Continuations use 1024 FIFO slots. A token encodes a 10-bit slot index and a
  20-bit generation in the control-only range `0xb0000000..0xf0000000`:
  `0xb0000000 + (generation << 10) + slot`. Generation zero is invalid; token
  PCs need no alignment. The reserved range starts at the current kernel system
  allocator's exclusive upper bound. It consumes no physical RAM and the loader
  rejects image code in this namespace.
- The old `0xffe00000..0xfff00000` namespace was too small for both many live
  slots and long-lived generation counters. The new namespace has 1,073,740,800
  distinct issued identities. Unused slots are preferred, then released slots
  are reused in FIFO order. No token value is ever reissued by one context.
- RET and cancellation compare the full generation and slot before consuming
  state. Stale RET/JUMP/cancel cannot invoke, cancel, or otherwise affect a
  replacement closure in the same slot. The runner never falls through to
  entry dispatch for any address in the continuation namespace, even if an
  executable mapping and entry override were registered at that address.
- Generation counters never wrap. A slot retires after generation 1,048,575.
  `clear_continuations` preserves generations and retired slots. If all slots
  are live or retired, call_guest returns the existing ContinuationLimit fault.
  This finite 32-bit identity bound is intentional: exhaustion fails closed
  instead of allowing arbitrarily old guest tokens to alias newer closures.
  A hot slot may retire before other slots if they remain live indefinitely.
- Cancellation still does not unwind the guest stack, but stale token rejection
  no longer depends on the caller finding every old guest copy. Failed stack
  pushes consume their generation and release the slot safely.
- HarnessExit is not pinned permanently. It is recorded as the latest event
  when returned, and may subsequently be drained or evicted by later history.
- Only the first failing FFI access is pinned and propagated. Generated code
  must stop after that instruction; further helpers from that invocation are
  invalid handler behavior, not later architecturally executed instructions.
  Their extra diagnostics may be evicted. Keeping the first fault reports the
  original instruction rather than replacing it with a secondary failure.

Regression tests force slot reuse after both RET and cancellation, then try
stale RET and JUMP values. They also verify stale cancellation and an entry
lookup decoy cannot affect the replacement, and that the replacement still
resumes through its own guest token. Allocator tests cover FIFO reuse and
retirement at the maximum generation without wrap or reset on clear.


## Follow-up: range protection, unmap, physical reservations

No generated-module ABI, C header, native layout or existing signature
changed. Everything below is additive Rust API. Semantics are in the README
section "Range protection, unmapping and physical reservations".

```rust
// guest::Permissions
pub const NONE: Permissions; // PAGE_NOACCESS

// guest::MapError: now also Clone + Copy, and new variants
Unmapped, // part of the range has no mapping
Reserved, // physical reservation or alias-window conflict
Claimed,  // HLE-owned virtual claim
LiftedCode, // unmap, or permissions without Execute, on guarded lifted code

pub struct ReservationId(/* private */);   // Copy, Eq, Ord, Hash, Debug
pub struct ClaimId(/* private */);         // Copy, Eq, Ord, Hash, Debug
pub struct PhysicalReservation { pub id: ReservationId, pub physical_start: u32,
    pub len: u32, pub aliases: Vec<u32>, pub owner: &'static str }
pub struct VirtualClaim { pub id: ClaimId, pub virtual_start: u32, pub len: u32,
    pub owner: &'static str }

impl Guest {
    pub fn protect_range(&mut self, va: u32, len: u32, permissions: Permissions)
        -> Result<(), MapError>;
    pub fn protect_range_owned(&mut self, claim: ClaimId, va: u32, len: u32,
        permissions: Permissions) -> Result<(), MapError>;
    pub fn unmap(&mut self, va: u32, len: u32) -> Result<(), MapError>;
    pub fn unmap_owned(&mut self, claim: ClaimId, va: u32, len: u32)
        -> Result<(), MapError>;
    pub fn reserve_physical(&mut self, physical: u32, len: u32, aliases: &[u32],
        owner: &'static str) -> Result<ReservationId, MapError>;
    pub fn map_reserved(&mut self, reservation: ReservationId, va: u32, len: u32,
        permissions: Permissions) -> Result<(), MapError>;
    pub fn claim_virtual(&mut self, va: u32, len: u32, owner: &'static str)
        -> Result<ClaimId, MapError>;
    pub fn physical_free(&self, physical: u32, len: u32) -> bool;
    pub fn find_free_physical(&self, len: u32, range: std::ops::Range<u32>,
        alignment: u32) -> Option<u32>;
    pub fn physical_aliases(&self, physical: u32, len: u32) -> Vec<Mapping>;
    pub fn physical_reservations(&self) -> &[PhysicalReservation];
    pub fn virtual_claims(&self) -> &[VirtualClaim];
    pub fn guard_code(&mut self, va: u32, len: u32) -> Result<(), MapError>;
    pub fn code_ranges(&self) -> Vec<std::ops::Range<u64>>;
}

// loader (load_xbe_with is also re-exported at the crate root)
pub const HALO2_PHYSICAL_ARENA: std::ops::Range<u32>; // 0x61000..0x3145000
pub const CONTIGUOUS_ALIAS_BASE: u32;                 // 0x80000000
pub struct ReservationRequest { pub physical: std::ops::Range<u32>,
    pub aliases: Vec<u32>, pub owner: &'static str }
pub struct LoadOptions { pub physical_reservations: Vec<ReservationRequest> } // Default
impl LoadOptions { pub fn halo2() -> Self; }
pub fn load_xbe_with(bytes: &[u8], options: &LoadOptions)
    -> Result<LoadedImage, LoadError>;
```

Behavior changes to existing calls, none of which affects a guest that has
no reservations or unmaps:

- `map` returns `Reserved` for reserved physical bytes or a reserved alias
  window. Its other checks and their order are unchanged.
- `map_zeroed` validates the virtual range before it chooses physical memory,
  so a bad or overlapping VA reports InvalidRange/Overlap/Reserved even when
  RAM is exhausted. It skips reservations. When fresh RAM above every earlier
  mapping runs out, it reuses the lowest free gap and zeroes it, where it used
  to return OutOfMemory.

- `load_xbe`/`load_xbe_with` call `guard_code` for every executable section
  after setting final protections. `protect_range` without Execute, `unmap`
  and both `_owned` variants refuse such pages with `LiftedCode`. Generated
  handlers check Execute only at dispatch and jump locally across pages, so
  the runtime cannot enforce those changes. It refuses them, and doesn't
  report a success it can't honour. A distinct
  error was chosen over an `Unmodelled` event because Guest has no event sink
  and the range calls are all-or-nothing. `protect` (whole mapping) is not
  guarded. Modules for code outside the image must `guard_code` it.

Kernel integration notes:

- `kernel::memory` still finds physical memory by scanning `mappings()`
  (`find_physical`, `physical_pages`). Once a reservation exists, it can pick
  reserved pages, and its `.expect("preflighted ...")` maps would then panic on
  `Reserved`. Switch those scans to `find_free_physical`/`physical_free` before
  booting with `LoadOptions::halo2()`.
- For Halo's `XPhysicalAlloc(0x30e4000, 0x61000, ...)`, recognise a request
  wholly inside a reservation and use
  `map_reserved(id, 0x80000000 + physical, size, perms)`. Return
  `0x80061000`.
- MmSetAddressProtect: `protect_range(base & !0xfff, page-rounded size, perms)`.
  PAGE_NOACCESS maps to `Permissions::NONE`.
- MmFreeContiguousMemory: `unmap(va, size)`. The pages stay reserved if they
  belong to a reservation. NtFreeVirtualMemory decommit: `unmap` each committed
  run. `Unmapped` means a hole.
- Map `LiftedCode` from MmSetAddressProtect or a free to a named Stop.
- Register `HLE_RESERVED` with `claim_virtual` if guest frees must never touch
  it; HLE then frees with `unmap_owned`.

Tests (`tests/memory_ranges.rs`) cover split edges, multi-mapping ranges,
NONE, all-or-nothing failure for every error class, per-alias protection with
shared bytes, unmap holes and straddling accesses, top-of-address-space
ranges, freed-RAM reuse with zeroing, reservation violation attempts through
every allocator and map path, reserved remaps after protect/unmap, claims, the
lifted-code guard (loader sections, hand guards, owners, partial ranges), and
runner Execute faults after protect and unmap of a non-image adapter page.
Rejected reservations are checked against nonzero sentinels in live backing. The ignored
`real_disc_arena_reservation_and_physical_budget` decodes the instructions at
0x0012b400 with iced-x86 (operands, call target, ignored result) and checks the budget. Set `H2_XBE` to point it at a disc outside
the worktree.

## Follow-up: write watch and guest interrupts

No generated-module ABI, C header or layout changed. Everything is additive
Rust API; semantics are in the README sections "Write watch" and "Guest
interrupts".

- New modules `watch` (`WatchId`, `DirtyRange`, `WatchError`,
  `CONTIGUOUS_BASE`, and the `Guest` methods `subscribe_writes`,
  `unsubscribe_writes`, `watch_writes`, `unwatch_writes`, `has_dirty`,
  `take_dirty`, `writes_unreported`, `fold_address`, `write_watch_owners`)
  and `interrupt` (`GuestInterrupt`, `CallConvention`, `Raise`,
  `MAX_PENDING`, `Runtime::{raise_interrupt, pending_interrupts,
  clear_interrupts}`, `ExecCtx::{set_interrupts_masked, interrupts_masked,
  in_interrupt}`). The types are re-exported at the crate root.
- New `Event::InterruptImbalance { name, target, expected_esp, actual_esp }`.
  A `match` on `Event` without a wildcard needs an arm.
- `Runner::run_from` may now run a queued interrupt between two transfers.
  With nothing queued, behaviour is unchanged.
- `Guest::write_bytes`, `map_zeroed` and `reserve_physical` record changed
  pages for watching subscribers. With no subscriber they behave and cost
  as before.

Tests: `tests/write_watch.rs` (alias folding, page coalescing, faulting
stores, atomics and `h2abi_v1_write`, runtime zero fills, independent and
muted subscribers, panic safety, unwatch/unsubscribe and errors, plus two
ignored benchmarks) and `tests/interrupts.rs` (restore of the whole CPU at
the first safe point, Stdcall, imbalance, no nesting, masking and thread
filters, coalescing and the bound, an unpushable frame, and a raise from
another host thread during a run).

## Follow-up: preemption at loop safe points

ABI_VERSION is now 2 (`cpu::ABI_VERSION`, `H2_ABI_VERSION`). No layout
changed; v2 adds the context prefix below, which v2 generated loops read,
so a v1 runtime must not load v2 code and this runtime rejects v1 objects,
modules and transfers. Added `runtime::preempt` (`SLICE`, `INTERRUPT`,
`REQUEST`, `DEFAULT_SLICE`, `PreemptWord`), `Runtime::{request_preempt,
preempt_pending}` and `ExecCtx::{arm_preemption, disarm_preemption,
preemption_armed, preempt_pending, slice_spent, take_slice_spent,
take_preempted}`.

- `ExecCtx` is now `#[repr(C)]` with a private first field: the pointer to
  its own preempt word, which generated code reads. Each context has its
  own word, so one context's slice expiry or safe point never consumes
  another's. The C contract is "Loop safe points" in
  `crates/emit/RUNTIME-CONTRACT.md`. Other fields are unchanged and
  private.
- `Runtime::request_preempt` sets the bits on every live context's word;
  `Runtime::preempt_pending` is the OR over them.
- `Runner::run_from` has a safe point before each dispatch. It changes
  nothing until the word is set; an armed context then returns `STOP` with
  event 0 and `take_preempted` holds the transfer to resume with.
- `raise_interrupt` wakes the words after queueing, also when the raise is
  coalesced, and unmasking with interrupts queued wakes the context's own
  word, so a spinning loop always gets back to the runner once an
  interrupt is deliverable.
- The generated-code accessors spend one unit of an armed context's slice
  per call.
- `kernel::Machine` gained `preempt_slice: Option<u64>` (default
  `Some(DEFAULT_SLICE)`) and `preemptions()`. With a slice set, each thread
  slice charges its accesses to logical time at
  `kernel::threads::ACCESSES_PER_TICK`, settled with `take_slice_spent()`
  on entry to each kernel call and after each run; a preempted thread
  never counts as polling. `kernel::clock::Clock::charge` coalesces
  consecutive charges into one history entry.

## Optional inline RAM access (ABI v2)

`Runtime::memory` and `ExecCtx::memory` now return `ffi::MemoryGuard<'_>`,
whose deref target is `Guest`. Replace explicit `std::sync::MutexGuard<'_, Guest>` return
annotations with that type. Inferred callers and Guest methods are unchanged.
The guard remains non-Send and synchronizes compound Rust operations with
optional inline C accesses. Drop every guard before invoking guest code.

`H2_FASTMEM=1` requires the named `h2abi_v1_fast_acquire` and
`h2abi_v1_fast_release` extensions. Their invocation-scoped view is separate
from the architectural ABI v2 layouts. The default compile mode remains off;
a loading bridge must explicitly forward the extensions before enabling it.
`ExecCtx::enable_fast_memory(false)` keeps a compatible generated module on
the checked Rust accessor path for diagnostic comparisons.
