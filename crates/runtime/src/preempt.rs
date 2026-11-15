//! Preemption: how a guest loop that never calls anything gets back to the
//! runner.
//!
//! Generated code polls one word at every loop header
//! (`crates/emit/RUNTIME-CONTRACT.md`, "Loop safe points"). The word belongs
//! to the [`Runtime`](crate::Runtime) and every [`ExecCtx`] carries a pointer
//! to it as its first field, so a handler loads the pointer once on entry
//! and each trip round a loop is one relaxed load and a branch. While the
//! word is zero nothing else happens. Once it is non-zero the loop leaves
//! the handler with `JUMP`/`RESUME` to its header, which the module
//! registered as a resume of its owner. That transfer reaches the runner's
//! safe point like any other and, if nothing there wants it, simply resumes
//! the loop, which then makes at least one trip before it can leave again.
//!
//! Bits say why the word was set ([`SLICE`], [`INTERRUPT`], [`REQUEST`]).
//! They are edge-triggered hints: the first runner safe point to see them
//! takes them all (one swap), and each consumer re-checks its own state
//! (the context's slice, the interrupt queue) rather than trusting a bit.
//!
//! The time slice is counted in guest memory accesses, not host time, so a
//! run is reproducible: every `h2abi_v1_*` accessor call of an armed context
//! spends one unit, and spending the last one sets [`SLICE`]. A spin-wait
//! reads memory each time round, so it always runs out. A loop that touches
//! no memory at all cannot wait on another thread; it only stops for an
//! interrupt or an explicit [`Runtime::request_preempt`].
//!
//! Only a context that has been armed ([`ExecCtx::arm_preemption`], which
//! `kernel::Machine` does for each thread slice) gives up its thread: the
//! runner stashes the transfer it was about to dispatch and returns
//! `STOP` with event 0, and [`ExecCtx::take_preempted`] hands the stashed
//! transfer back for a later `run_from`. A context that is not armed (a DPC,
//! a test's runner) just drops the request and carries on.
//!
//! The interrupt queue shares the safe point: `raise_interrupt` sets
//! [`INTERRUPT`] after queueing, so a spinning generated loop comes back to
//! the runner, where `interrupt::deliver` runs just before this check.

use crate::{ExecCtx, Runtime, TransferAbi};
use std::sync::atomic::{AtomicU32, Ordering};

/// The armed context's access budget ran out.
pub const SLICE: u32 = 1;
/// A guest interrupt was queued; deliver it at the next safe point.
pub const INTERRUPT: u32 = 2;
/// The host asked the running guest thread to give up its slice.
pub const REQUEST: u32 = 4;

/// Accesses per Machine slice by default (`kernel::Machine::preempt_slice`).
/// A ready thread waits at most this many guest memory accesses of another
/// thread's spinning: about 1 to 3 ms through the locked accessors.
pub const DEFAULT_SLICE: u64 = 1 << 16;

/// The word generated code polls. Lives inside the `Arc<Runtime>`, so its
/// address is fixed for the runtime's lifetime.
#[derive(Debug, Default)]
pub struct PreemptWord(AtomicU32);

impl PreemptWord {
    pub(crate) fn request(&self, reasons: u32) {
        self.0.fetch_or(reasons, Ordering::Release);
    }
    pub(crate) fn pending(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
    fn take(&self) -> u32 {
        self.0.swap(0, Ordering::Acquire)
    }
    fn as_ptr(&self) -> *const AtomicU32 {
        &self.0
    }
}

/// `ExecCtx`'s first field: the address of its runtime's [`PreemptWord`].
/// Generated code reads `*(const uint32_t *const *)ctx`; Rust only keeps it.
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct WordPtr(*const AtomicU32);

// SAFETY: the pointer targets an AtomicU32 inside the Arc<Runtime> that the
// same ExecCtx owns, so it is valid wherever the context is, and an atomic
// may be shared between threads.
#[allow(unsafe_code)]
unsafe impl Send for WordPtr {}
// SAFETY: as above; Rust never dereferences it.
#[allow(unsafe_code)]
unsafe impl Sync for WordPtr {}

impl WordPtr {
    pub(crate) fn of(runtime: &Runtime) -> Self {
        Self(runtime.preempt.as_ptr())
    }
}

/// A context's slice: armed or not, and what is left of it.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Slice {
    pub armed: bool,
    /// Accesses left; reaching zero sets [`SLICE`] once.
    pub left: u64,
}

impl Runtime {
    /// Ask whichever guest thread is running to reach a safe point:
    /// `reasons` is any of [`SLICE`], [`INTERRUPT`], [`REQUEST`]. Callable
    /// from any host thread. With [`REQUEST`] an armed context gives up its
    /// slice at its next loop header or transfer.
    pub fn request_preempt(&self, reasons: u32) {
        self.preempt.request(reasons);
    }
    /// The reasons currently waiting for a safe point (0 when none).
    pub fn preempt_pending(&self) -> u32 {
        self.preempt.pending()
    }
}

impl ExecCtx {
    /// Let this context give up its thread at a safe point once it has
    /// made `accesses` guest memory accesses, or as soon as the host
    /// requests it. `accesses` of 0 arms for host requests only.
    pub fn arm_preemption(&mut self, accesses: u64) {
        self.slice = Slice {
            armed: true,
            left: accesses,
        };
    }
    /// Stop counting and ignore preemption requests (the default).
    pub fn disarm_preemption(&mut self) {
        self.slice = Slice::default();
    }
    pub fn preemption_armed(&self) -> bool {
        self.slice.armed
    }
    /// After a run ended in `STOP` with event 0 because this context was
    /// preempted: the transfer to resume it with (`Runner::run_from`).
    pub fn take_preempted(&mut self) -> Option<TransferAbi> {
        self.preempted.take()
    }
    /// One guest memory access by generated code.
    #[inline]
    pub(crate) fn spend_access(&mut self) {
        match self.slice.left {
            0 => {}
            1 => {
                self.slice.left = 0;
                self.runtime().preempt.request(SLICE);
            }
            _ => self.slice.left -= 1,
        }
    }
    /// The runner's safe point, before `transfer` is dispatched. Returns the
    /// runner's result if this context gives up its thread here.
    #[inline]
    pub(crate) fn preempt_point(&mut self, transfer: TransferAbi) -> Option<TransferAbi> {
        if self.runtime().preempt.pending() == 0 {
            return None;
        }
        self.preempt_slow(transfer)
    }
    #[cold]
    fn preempt_slow(&mut self, transfer: TransferAbi) -> Option<TransferAbi> {
        let reasons = self.runtime().preempt.take();
        // INTERRUPT needs nothing more here: reaching the safe point is what
        // lets the interrupt queue deliver.
        if self.slice.armed && reasons & (SLICE | REQUEST) != 0 {
            self.preempted = Some(transfer);
            return Some(TransferAbi::stop(0));
        }
        None
    }
}
