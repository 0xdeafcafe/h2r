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
//!
//! The leave word (docs/runtime/contract-thin-transfers-flat-ram.md,
//! section 4.2). A direct call between generated handlers may go ahead only
//! where the runner's safe point would do nothing: today that is two loads,
//! this word and the runtime's queued-interrupt count. [`IRQ_QUEUED`] folds
//! the second into the first. Once [`Runtime::enable_leave_word`] has been
//! called, every context's word has [`IRQ_QUEUED`] set exactly while the
//! interrupt queue holds anything: it is set and cleared under the queue's
//! lock, and a word registered later starts with it if the queue is not
//! empty. Taking the bits never clears it. So "the word is zero" means what
//! "the word is zero and nothing is queued" meant. It is not a wake reason:
//! [`ExecCtx::take_preempt`] and both `preempt_pending`s leave it out.
//!
//! The runtime sets it only once asked, because generated code built before
//! it polls loop headers with `word != 0`: such a loop would leave on every
//! trip while an interrupt for another thread waits. A module whose loop
//! polls and `h2_fast_movs` mask [`IRQ_QUEUED`] out turns it on.

use crate::{ExecCtx, Runtime, TransferAbi};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// The armed context's access budget ran out.
pub const SLICE: u32 = 1;
/// A guest interrupt was queued; deliver it at the next safe point.
pub const INTERRUPT: u32 = 2;
/// The host asked the running guest thread to give up its slice.
pub const REQUEST: u32 = 4;
/// Sticky, not a wake reason: the runtime's interrupt queue is not empty.
/// Kept only after [`Runtime::enable_leave_word`]; see the module doc.
/// (8 is kept for `PARK`, contract change slice F2.)
pub const IRQ_QUEUED: u32 = 16;

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
    fn clear(&self, bits: u32) {
        self.0.fetch_and(!bits, Ordering::Release);
    }
    /// Every bit, [`IRQ_QUEUED`] included: what generated code loads.
    fn pending(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
    /// Clear the wake bits and return them; [`IRQ_QUEUED`] stays as it is.
    fn take(&self) -> u32 {
        self.0.fetch_and(IRQ_QUEUED, Ordering::Acquire) & !IRQ_QUEUED
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
    /// Set or clear [`IRQ_QUEUED`] on every word. Called only with the
    /// interrupt queue's lock held, so the bit follows the queue exactly.
    pub(crate) fn set_queued(&self, queued: bool) {
        self.each(|w| {
            if queued {
                w.request(IRQ_QUEUED);
            } else {
                w.clear(IRQ_QUEUED);
            }
        });
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
    // Under the queue's lock, so no raise or drain falls between the
    // registration and the initial IRQ_QUEUED (lock order: queue, words).
    let queue = runtime.interrupts.lock();
    runtime.words.register(&word);
    if runtime.interrupts.leave_word() && !queue.is_empty() {
        word.request(IRQ_QUEUED);
    }
    drop(queue);
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
        let reasons = reasons & !IRQ_QUEUED;
        self.words.each(|w| w.request(reasons));
    }
    /// The wake bits set on any live context's word (0 when none), for
    /// diagnostics. [`IRQ_QUEUED`] is not one.
    pub fn preempt_pending(&self) -> u32 {
        let all = AtomicU32::new(0);
        self.words.each(|w| {
            all.fetch_or(w.pending() & !IRQ_QUEUED, Ordering::Relaxed);
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
    /// Charge `accesses` made elsewhere (a second executor's call, at its
    /// join) to this context, as if its own generated code had made them:
    /// they count toward [`slice_spent`](Self::slice_spent) and may expire
    /// the slice.
    pub fn charge_accesses(&mut self, accesses: u64) {
        self.spend_accesses(accesses);
    }
    /// The wake bits set on this context's own word ([`IRQ_QUEUED`] is not
    /// one).
    pub fn preempt_pending(&self) -> u32 {
        self.word.pending() & !IRQ_QUEUED
    }
    /// The whole word as generated code loads it, [`IRQ_QUEUED`] included.
    /// Once the leave word is enabled, zero means a direct call may go
    /// ahead (`h2_direct_quiet`).
    pub fn leave_word(&self) -> u32 {
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
        crate::ffi::sync_accesses(self);
        self.spend_accesses(1);
    }
    pub(crate) fn preempt_ptr(&self) -> *const AtomicU32 {
        &self.word.0
    }
    pub(crate) fn spend_accesses(&mut self, count: u64) {
        if let Some(p) = &mut self.profile {
            p.memory_accessors += count;
        }
        if self.slice.armed {
            self.slice.spent += count;
            if self.slice.left != 0 && count >= self.slice.left {
                self.slice.expired = true;
                self.word.request(SLICE);
            }
            self.slice.left = self.slice.left.saturating_sub(count);
        }
        if let Some(view) = &mut self.fast_memory {
            view.set_budget(if self.slice.armed && self.slice.left != 0 {
                self.slice.left
            } else {
                u64::MAX
            });
        }
    }
    /// Accesses the context may make before its slice expires, for
    /// generated code that counts its own: `u64::MAX` when nothing would
    /// expire (not armed, armed for host requests only, or already spent).
    #[inline]
    pub(crate) fn access_budget(&self) -> u64 {
        if !self.slice.armed || self.slice.left == 0 { u64::MAX } else { self.slice.left }
    }
    /// First step of the runner's safe point: take this context's wake bits
    /// before the interrupt queue is looked at. One relaxed load while
    /// nothing is set; [`IRQ_QUEUED`] alone is nothing to take and stays.
    #[inline]
    pub(crate) fn take_preempt(&self) -> u32 {
        if self.word.pending() & !IRQ_QUEUED == 0 {
            return 0;
        }
        self.word.take()
    }
    /// A delivered interrupt pre-empted this safe point: keep the reasons
    /// it did not handle for the next one.
    pub(crate) fn defer_preempt(&self, reasons: u32) {
        let rest = reasons & !(INTERRUPT | IRQ_QUEUED);
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
