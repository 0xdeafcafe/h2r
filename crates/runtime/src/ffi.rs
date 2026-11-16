//! Audited native boundary. Only this module dereferences foreign pointers.
//!
//! Native handlers receive exclusive CPU/context borrows for one invocation.
//! They must not retain pointers, alias those borrows, unwind, or call helpers
//! concurrently on the same context. Memory buffers must be valid for `width`
//! bytes and disjoint from ExecCtx storage. Atomic outputs may alias CpuAbi
//! after the operation's CPU borrow ends. Optional C RAM views follow the
//! gate protocol in `memory.rs` and may not escape their invocation.

mod memory;
pub use memory::{FastAccess, MemoryGuard, h2abi_v1_fast_acquire, h2abi_v1_fast_release};
pub(crate) use memory::{Memory, MemoryShared, OwnedFastAccess, sync_accesses};

use crate::{
    CpuAbi, Dispatch, Event, ExecCtx, GuestFn, TransferAbi,
    cpu::ABI_VERSION,
    dispatch::{Handler, RegistrationError},
};

/// Incomplete in C. Its pointer is an invocation-scoped handle, not guest data.
#[repr(C)]
pub struct ExecCtxOpaque {
    _private: [u8; 0],
    _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

pub(crate) fn call_native(f: GuestFn, cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32) -> TransferAbi {
    // SAFETY: native registration promises the documented ABI; these disjoint
    // exclusive borrows remain alive for exactly the synchronous call.
    let result = unsafe { f(cpu, (ctx as *mut ExecCtx).cast(), pc) };
    sync_accesses(ctx);
    result
}

/// Adapter for handwritten native fixtures/bridges. Rust overrides should use
/// Dispatch::register_override and receive references directly.
///
/// # Safety
/// Pointers must be the live, exclusive pair supplied to the current handler.
/// `f` must not retain either reference after returning.
pub(crate) unsafe fn with_context<R>(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    f: impl FnOnce(&mut CpuAbi, &mut ExecCtx) -> R,
) -> R {
    // SAFETY: caller establishes validity, exclusivity and lifetime above.
    unsafe { f(&mut *cpu, &mut *exec.cast::<ExecCtx>()) }
}

fn status(ctx: &mut ExecCtx, pc: u32, result: Result<(), crate::GuestFault>) -> u32 {
    match result {
        Ok(()) => 0,
        Err(fault) => {
            let id = ctx.emit(Event::MemoryFault { pc, fault });
            // Generated code must stop at the first failed access. Preserve
            // that instruction's fault if a broken handler continues issuing
            // helpers; later faults are ordinary, evictable diagnostics.
            ctx.pending_fault.get_or_insert(id);
            id
        }
    }
}

fn invalid(
    ctx: &mut ExecCtx,
    pc: u32,
    addr: u32,
    width: u32,
    access: crate::guest::Access,
    reason: crate::guest::FaultReason,
) -> u32 {
    status(
        ctx,
        pc,
        Err(crate::GuestFault {
            addr,
            len: width as usize,
            access,
            reason,
        }),
    )
}

/// Read 1, 2, 4, 8, 10 or 16 little-endian bytes. Returns zero on success or
/// an ExecCtx event id on fault; failure leaves the host output untouched.
/// `alignment` is one for ordinary accesses, 16 for MOVAPS, etc.
///
/// # Safety
/// `exec` is the current exclusive handle. `out` is writable for `width`
/// bytes, nonoverlapping with the context or guest RAM, and valid until this call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_read(
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    alignment: u32,
    out: *mut u8,
) -> u32 {
    // SAFETY: the caller's contract, passed on.
    unsafe { read(exec, pc, addr, width, alignment, out, true) }
}

/// [`h2abi_v1_read`] without spending from the slice: for a module that
/// counts its own accesses ([`crate::generated::FEATURE_ACCOUNTING`]) and
/// settles them through [`h2abi_v1_charge`].
///
/// # Safety
/// As for [`h2abi_v1_read`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_read_counted(
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    alignment: u32,
    out: *mut u8,
) -> u32 {
    // SAFETY: the caller's contract, passed on.
    unsafe { read(exec, pc, addr, width, alignment, out, false) }
}

/// # Safety
/// As for [`h2abi_v1_read`].
unsafe fn read(
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    alignment: u32,
    out: *mut u8,
    charge: bool,
) -> u32 {
    // SAFETY: invocation-scoped exclusive handle, as required by the ABI.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    if charge {
        ctx.spend_access();
    }
    if !matches!(width, 1 | 2 | 4 | 8 | 10 | 16) {
        return invalid(
            ctx,
            pc,
            addr,
            width,
            crate::guest::Access::Read,
            crate::guest::FaultReason::InvalidWidth,
        );
    }
    if out.is_null() {
        return invalid(
            ctx,
            pc,
            addr,
            width,
            crate::guest::Access::Read,
            crate::guest::FaultReason::InvalidBuffer,
        );
    }
    // SAFETY: the ABI guarantees width writable bytes disjoint from context.
    // Guest preflights the entire span before copying, so faults leave output
    // untouched without staging through a second host buffer.
    let out = unsafe { std::slice::from_raw_parts_mut(out, width as usize) };
    let result = ctx.memory().read_bytes(addr, out, alignment);
    status(ctx, pc, result)
}

/// Write all bytes atomically with respect to ordinary guest/HLE accesses.
/// The status and alignment rules match h2abi_v1_read.
///
/// # Safety
/// `exec` is the current exclusive handle. `value` is readable for `width`
/// bytes and disjoint from the context and guest RAM throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_write(
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    alignment: u32,
    value: *const u8,
) -> u32 {
    // SAFETY: the caller's contract, passed on.
    unsafe { write(exec, pc, addr, width, alignment, value, true) }
}

/// [`h2abi_v1_write`] without spending from the slice; see
/// [`h2abi_v1_read_counted`].
///
/// # Safety
/// As for [`h2abi_v1_write`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_write_counted(
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    alignment: u32,
    value: *const u8,
) -> u32 {
    // SAFETY: the caller's contract, passed on.
    unsafe { write(exec, pc, addr, width, alignment, value, false) }
}

/// Spend `accesses` guest memory accesses from the context's slice at
/// once, exactly as that many charged accessor calls would: what a module
/// that counts its own accesses settles before it leaves a handler and
/// before anything that spends or observes the slice.
///
/// # Safety
/// `exec` is the current exclusive handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_charge(exec: *mut ExecCtxOpaque, accesses: u64) {
    // SAFETY: invocation-scoped exclusive handle, as required by the ABI.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    // Anything the inline RAM view charged itself comes first.
    sync_accesses(ctx);
    ctx.spend_accesses(accesses);
}

/// A handler's way out: settle the inline RAM view's count, then spend the
/// handler's own `accesses` (none when 0). One call doing what
/// `h2abi_v1_charge(accesses)` (only when non-zero) followed by
/// `h2abi_v1_fast_release` did: the charge settles the view first, and the
/// release then finds nothing left to settle. Modules from before it still
/// call those two, which stay exported.
///
/// # Safety
/// `exec` is the current exclusive handle, with no C gate held.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_exit(exec: *mut ExecCtxOpaque, accesses: u64) {
    // SAFETY: invocation-scoped exclusive handle, as required by the ABI.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    sync_accesses(ctx);
    if accesses != 0 {
        ctx.spend_accesses(accesses);
    }
}

/// How many accesses the context may make before its slice expires
/// (`UINT64_MAX` when nothing would expire): a counting module's loops
/// leave for the runner once their uncharged count reaches it, where the
/// charged accessors would have set the slice bit.
///
/// # Safety
/// `exec` is the current exclusive handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_budget(exec: *mut ExecCtxOpaque) -> u64 {
    // SAFETY: invocation-scoped exclusive handle, as required by the ABI.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    sync_accesses(ctx);
    ctx.access_budget()
}

/// # Safety
/// As for [`h2abi_v1_write`].
unsafe fn write(
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    alignment: u32,
    value: *const u8,
    charge: bool,
) -> u32 {
    // SAFETY: invocation-scoped exclusive handle, as required by the ABI.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    if charge {
        ctx.spend_access();
    }
    if !matches!(width, 1 | 2 | 4 | 8 | 10 | 16) {
        return invalid(
            ctx,
            pc,
            addr,
            width,
            crate::guest::Access::Write,
            crate::guest::FaultReason::InvalidWidth,
        );
    }
    if value.is_null() {
        return invalid(
            ctx,
            pc,
            addr,
            width,
            crate::guest::Access::Write,
            crate::guest::FaultReason::InvalidBuffer,
        );
    }
    // SAFETY: the ABI guarantees width readable bytes disjoint from context.
    // Guest validates the complete write and retains its alias-aware barrier.
    let value = unsafe { std::slice::from_raw_parts(value, width as usize) };
    let result = ctx.memory().write_bytes(addr, value, alignment);
    status(ctx, pc, result)
}

/// Typed runtime identity, distinct from a generated object's C version probe.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeAbiVersion(u32);
impl RuntimeAbiVersion {
    pub const fn get(self) -> u32 {
        self.0
    }
}

pub fn runtime_abi_version() -> RuntimeAbiVersion {
    RuntimeAbiVersion(ABI_VERSION)
}

// C retains its existing scalar query ABI. Keeping this typed export private
// prevents accidentally registering it as a generated object's version probe.
#[unsafe(export_name = "h2abi_v1_version")]
extern "C" fn runtime_version_export() -> RuntimeAbiVersion {
    runtime_abi_version()
}

/// Compatibility query for Rust callers. It deliberately uses the Rust ABI;
/// only generated object probes have the GeneratedAbiVersion C signature.
///
/// ```compile_fail
/// use runtime::ffi::{h2abi_v1_version, GeneratedAbiVersion};
/// let wrong_probe: GeneratedAbiVersion = h2abi_v1_version;
/// ```
///
/// ```compile_fail
/// use runtime::ffi::{runtime_abi_version, GeneratedAbiVersion};
/// let wrong_probe: GeneratedAbiVersion = runtime_abi_version;
/// ```
pub fn h2abi_v1_version() -> u32 {
    runtime_abi_version().get()
}

impl Dispatch {
    /// # Safety
    /// `f` must implement the ABI v1 pointer, lifetime and no-unwind contract
    /// in ffi.rs and h2abi.h. Its code must remain loaded while registered.
    /// The supplied version must describe the compiled object, not the runtime.
    pub unsafe fn register_generated(
        &mut self,
        addr: u32,
        version: u32,
        f: GuestFn,
    ) -> Result<(), RegistrationError> {
        if version != ABI_VERSION {
            return Err(RegistrationError::AbiVersion(version));
        }
        self.register_generated_handler(addr, Handler::Native(f))
    }

    /// # Safety
    /// Same obligations as `register_generated`.
    pub unsafe fn register_native_override(
        &mut self,
        addr: u32,
        name: impl Into<String>,
        version: u32,
        f: GuestFn,
    ) -> Result<Option<crate::dispatch::Override>, RegistrationError> {
        if version != ABI_VERSION {
            return Err(RegistrationError::AbiVersion(version));
        }
        Ok(self.register_override_handler(addr, name.into(), Handler::Native(f)))
    }
}

/// ABI probe exported by a generated object, compiled with that object's header.
pub type GeneratedAbiVersion = unsafe extern "C" fn() -> u32;
impl Dispatch {
    /// Register using the generated object's compiled-in version, rather than
    /// asking the runtime to compare its own version with itself.
    ///
    /// # Safety
    /// Both functions must remain loaded and obey their C ABI contracts.
    pub unsafe fn register_generated_object(
        &mut self,
        addr: u32,
        object_version: GeneratedAbiVersion,
        f: GuestFn,
    ) -> Result<(), RegistrationError> {
        // SAFETY: caller supplies a loaded object version probe.
        let version = unsafe { object_version() };
        // SAFETY: caller also establishes the native handler contract.
        unsafe { self.register_generated(addr, version, f) }
    }
}

/// End all CPU/context borrows before storing the output. A C caller may pass
/// a CpuAbi field as `out`; the final output store intentionally wins over any
/// earlier update of that field by the instruction helper.
unsafe fn atomic_output(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    out: *mut u32,
    operation: impl FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<u32, crate::GuestFault>,
) -> u32 {
    if out.is_null() {
        // SAFETY: caller supplies the live context; CPU is not borrowed here.
        let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
        return invalid(
            ctx,
            pc,
            addr,
            width,
            crate::guest::Access::Write,
            crate::guest::FaultReason::InvalidBuffer,
        );
    }
    // SAFETY: the caller supplies the exclusive invocation pair. The closure
    // returns an owned result, so no reference to CPU/context escapes this call.
    let result = unsafe {
        with_context(cpu, exec, |cpu, ctx| {
            ctx.spend_access();
            operation(cpu, ctx)
        })
    };
    match result {
        Ok(old) => {
            // SAFETY: out is writable, disjoint from ExecCtx, and may alias CPU
            // now that all reborrowed Rust CPU references have ended.
            unsafe {
                std::ptr::write_unaligned(out, old);
            }
            0
        }
        Err(fault) => {
            // SAFETY: the earlier context borrow has ended; out stays untouched.
            let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
            status(ctx, pc, Err(fault))
        }
    }
}

/// Apply locked 8/16/32-bit arithmetic/logic and return the old memory operand.
///
/// # Safety
/// CPU/context are the current pair; out is writable for a u32 and disjoint
/// from the context. It may point into CpuAbi; the output store occurs last.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_atomic_rmw(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    op: u32,
    operand: u32,
    out: *mut u32,
) -> u32 {
    // SAFETY: pointer validity and aliasing requirements are the caller's contract.
    unsafe {
        atomic_output(cpu, exec, pc, addr, width, out, |cpu, ctx| {
            let op = crate::atomic::AtomicOp::from_tag(op).ok_or(crate::GuestFault {
                addr,
                len: width as usize,
                access: crate::guest::Access::Write,
                reason: crate::guest::FaultReason::InvalidOperation,
            })?;
            ctx.memory().atomic_rmw(cpu, addr, width, op, operand)
        })
    }
}

/// Exchange an 8/16/32-bit operand under one memory guard.
///
/// # Safety
/// Same pointer and output-alias contract as h2abi_v1_atomic_rmw.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_xchg(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    value: u32,
    out: *mut u32,
) -> u32 {
    // SAFETY: caller supplies valid pointers, permitting output to alias CPU.
    unsafe {
        atomic_output(cpu, exec, pc, addr, width, out, |_, ctx| {
            ctx.memory().atomic_xchg(addr, width, value)
        })
    }
}

/// CMPXCHG compares AL/AX/EAX, updates arithmetic flags, and loads the old
/// memory operand into that accumulator slice on failure.
///
/// # Safety
/// Same pointer and output-alias contract as h2abi_v1_atomic_rmw.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_cmpxchg(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
    width: u32,
    replacement: u32,
    out: *mut u32,
) -> u32 {
    // SAFETY: caller supplies valid pointers, permitting output to alias CPU.
    unsafe {
        atomic_output(cpu, exec, pc, addr, width, out, |cpu, ctx| {
            ctx.memory().atomic_cmpxchg(cpu, addr, width, replacement)
        })
    }
}

/// CMPXCHG8B uses EDX:EAX and ECX:EBX and changes only ZF.
///
/// # Safety
/// CPU/context must be the current exclusive invocation pair.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_cmpxchg8b(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    pc: u32,
    addr: u32,
) -> u32 {
    // SAFETY: caller provides the exclusive invocation pair.
    unsafe {
        with_context(cpu, exec, |cpu, ctx| {
            ctx.spend_access();
            let result = ctx.memory().atomic_cmpxchg8b(cpu, addr);
            status(ctx, pc, result)
        })
    }
}

/// Batch local counters once on every generated handler exit. No retained pointer.
/// # Safety
/// `exec` is the live exclusive invocation context passed to the handler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_profile(exec: *mut crate::ExecCtxOpaque, loops: u64, x87: u64) {
    // SAFETY: caller guarantees the current invocation's context.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    if let Some(p) = &mut ctx.profile {
        p.generated_batches += 1;
        p.loop_safe_points += loops;
        p.x87_helpers += x87;
    }
}
