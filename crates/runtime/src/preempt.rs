//! Preemption: how a guest loop that never calls anything gets back to the
//! runner.
//!
//! Generated code polls one word at every loop header
//! (`crates/emit/RUNTIME-CONTRACT.md`, "Loop safe points"; ABI v2). Each
//! [`ExecCtx`] owns its own word and carries a pointer to it as its first
//! field, so a handler loads the pointer once on entry and each trip round a
//! loop is one relaxed load and a branch. While the word is zero nothing
//! else happens. Once it is non-zero the loop leaves the handler with
//! `JUMP`/`RESUME` to its header, which the module registered as a resume
//! of its owner. That transfer reaches the runner's safe point like any
//! other and, if nothing there wants it, simply resumes the loop, which then
//! makes at least one trip before it can leave again.
//!
//! Bits say why the word was set ([`SLICE`], [`INTERRUPT`], [`REQUEST`]).
//! They are wake hints for the context's own generated loops; the state
//! behind them is authoritative and kept elsewhere: the context's slice
//! (per context, so no other context can consume its expiry) and the
//! interrupt queue. The [`Runtime`] keeps a weak list of every context's
//! word so that host requests and interrupt raises can wake them all.
//!
//! Safe point protocol (runner, before each dispatch): take this context's
//! bits first ([`ExecCtx::take_preempt`]), then deliver a due interrupt,
//! then decide on preemption ([`ExecCtx::preempt_decide`]). A raise that
//! lands after the take sets the word again and the next poll sees it; one
//! that landed before it is already in the queue when delivery looks. No
//! wake is lost. An interrupt that cannot be delivered here (masked, in
//! service, or for another thread) becomes deliverable only through a path
//! that returns to the runner (unmasking, the callback's return, another
//! context), and every runner safe point checks the queue.
//!
//! The time slice is counted in guest memory accesses, not host time, so a
//! run is reproducible: every `h2abi_v1_*` accessor call of an armed context
//! spends one unit, and spending the last one marks the slice expired and
//! sets [`SLICE`] on the context's own word. A spin-wait reads memory each
//! time round, so it always runs out. A loop that touches no memory at all
//! only stops for an interrupt or an explicit [`Runtime::request_preempt`].
//!
//! Only a context that has been armed ([`ExecCtx::arm_preemption`], which
//! `kernel::Machine` does for each thread slice) gives up its thread: the
//! runner stashes the transfer it was about to dispatch and returns `STOP`
//! with event 0, and [`ExecCtx::take_preempted`] hands the stashed transfer
//! back for a later `run_from`. A context that is not armed (a DPC, a test's
//! runner) ignores [`SLICE`] and [`REQUEST`] and carries on.

use crate::{ExecCtx, Runtime, TransferAbi};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

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

/// The word one context's generated code polls. Heap-allocated, so its
/// address is fixed however the context moves.
#[derive(Debug, Default)]
pub struct PreemptWord(AtomicU32);

impl PreemptWord {
    fn request(&self, reasons: u32) {
        self.0.fetch_or(reasons, Ordering::Release);
    }
    fn pending(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
    fn take(&self) -> u32 {
        self.0.swap(0, Ordering::Acquire)
    }
}

/// Every live context's word, for wakes that are not addressed to one.
#[derive(Debug, Default)]
pub(crate) struct Words(Mutex<Vec<Weak<PreemptWord>>>);

impl Words {
    fn register(&self, word: &Arc<PreemptWord>) {
        let mut words = self.0.lock().unwrap_or_else(|e| e.into_inner());
        words.retain(|w| w.strong_count() > 0);
        words.push(Arc::downgrade(word));
    }
    fn each(&self, f: impl Fn(&PreemptWord)) {
        let words = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for w in words.iter().filter_map(Weak::upgrade) {
            f(&w);
        }
    }
}

/// `ExecCtx`'s first field: the address of the context's own word.
/// Generated code reads `*(const uint32_t *const *)ctx`; Rust only keeps it.
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct WordPtr(*const AtomicU32);

// SAFETY: the pointer targets the AtomicU32 inside the Arc<PreemptWord> that
// the same ExecCtx owns, so it is valid wherever the context is, and an
// atomic may be shared between threads.
#[allow(unsafe_code)]
unsafe impl Send for WordPtr {}
// SAFETY: as above; Rust never dereferences it.
#[allow(unsafe_code)]
unsafe impl Sync for WordPtr {}

/// A new context's word, registered with its runtime, and the pointer to
/// it that becomes the context's first field.
pub(crate) fn new_word(runtime: &Runtime) -> (WordPtr, Arc<PreemptWord>) {
    let word = Arc::new(PreemptWord::default());
    runtime.words.register(&word);
    (WordPtr(&word.0), word)
}

/// A context's slice: armed or not, what is left, and whether it expired.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Slice {
    pub armed: bool,
    /// Accesses left; reaching zero sets `expired` once.
    pub left: u64,
    /// The budget ran out and the context has not yet given up its thread.
    pub expired: bool,
    /// Accesses made since the context was armed, past the budget too.
    pub spent: u64,
}

impl Runtime {
    /// Wake every context's loops for a safe point: `reasons` is any of
    /// [`SLICE`], [`INTERRUPT`], [`REQUEST`]. Callable from any host thread.
    /// With [`REQUEST`] an armed context gives up its slice at its next loop
    /// header or transfer.
    pub fn request_preempt(&self, reasons: u32) {
        self.words.each(|w| w.request(reasons));
    }
    /// The bits set on any live context's word (0 when none), for
    /// diagnostics.
    pub fn preempt_pending(&self) -> u32 {
        let all = AtomicU32::new(0);
        self.words.each(|w| {
            all.fetch_or(w.pending(), Ordering::Relaxed);
        });
        all.into_inner()
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
            expired: false,
            spent: 0,
        };
    }
    /// Stop counting and ignore preemption requests (the default).
    pub fn disarm_preemption(&mut self) {
        self.slice = Slice::default();
    }
    pub fn preemption_armed(&self) -> bool {
        self.slice.armed
    }
    /// Guest memory accesses generated code has made since the context was
    /// last armed (0 when it is not armed). A scheduler can charge them to
    /// guest time.
    pub fn slice_spent(&self) -> u64 {
        self.slice.spent
    }
    /// [`slice_spent`](Self::slice_spent), resetting it to 0 (the budget
    /// left is unchanged), so a kernel call can settle the work before it
    /// and the scheduler then settles only what came after.
    pub fn take_slice_spent(&mut self) -> u64 {
        std::mem::take(&mut self.slice.spent)
    }
    /// The bits set on this context's own word.
    pub fn preempt_pending(&self) -> u32 {
        self.word.pending()
    }
    /// After a run ended in `STOP` with event 0 because this context was
    /// preempted: the transfer to resume it with (`Runner::run_from`).
    pub fn take_preempted(&mut self) -> Option<TransferAbi> {
        self.preempted.take()
    }
    /// Ask this context's loops for a safe point.
    pub(crate) fn wake(&self, reasons: u32) {
        self.word.request(reasons);
    }
    /// One guest memory access by generated code.
    #[inline]
    pub(crate) fn spend_access(&mut self) {
        if !self.slice.armed {
            return;
        }
        self.slice.spent += 1;
        match self.slice.left {
            0 => {}
            1 => {
                self.slice.left = 0;
                self.slice.expired = true;
                self.word.request(SLICE);
            }
            _ => self.slice.left -= 1,
        }
    }
    /// First step of the runner's safe point: take this context's wake bits
    /// before the interrupt queue is looked at. One relaxed load while
    /// nothing is set.
    #[inline]
    pub(crate) fn take_preempt(&self) -> u32 {
        if self.word.pending() == 0 {
            return 0;
        }
        self.word.take()
    }
    /// A delivered interrupt pre-empted this safe point: keep the reasons
    /// it did not handle for the next one.
    pub(crate) fn defer_preempt(&self, reasons: u32) {
        let rest = reasons & !INTERRUPT;
        if rest != 0 {
            self.word.request(rest);
        }
    }
    /// Last step of the safe point, after interrupt delivery: returns the
    /// runner's result if this context gives up its thread before
    /// `transfer`.
    #[inline]
    pub(crate) fn preempt_decide(
        &mut self,
        reasons: u32,
        transfer: TransferAbi,
    ) -> Option<TransferAbi> {
        if !self.slice.armed || !(self.slice.expired || reasons & REQUEST != 0) {
            return None;
        }
        self.slice.expired = false;
        self.preempted = Some(transfer);
        Some(TransferAbi::stop(0))
    }
}
