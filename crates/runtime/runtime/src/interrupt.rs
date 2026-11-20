//! Asynchronous guest callbacks delivered at runner safe points.
//!
//! Any host thread may queue a [`GuestInterrupt`] on the shared [`Runtime`].
//! A runner delivers it between two transfers: after one handler (generated
//! code, an override or a continuation) has returned a CALL, JUMP or RETURN
//! and before the next one is dispatched. CPU state is canonical there, so
//! the runner snapshots it, pushes the arguments and a callback token, and
//! runs the target as an ordinary guest call. When the target returns to the
//! token, the snapshot is restored whole and the interrupted transfer
//! continues as if nothing had happened.
use crate::{CpuAbi, Event, ExecCtx, Runtime, TransferAbi, abi, runner::HARNESS_EXIT};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// At most this many interrupts wait at once; [`Runtime::raise_interrupt`]
/// refuses more.
pub const MAX_PENDING: usize = 64;
const DIRECTION_FLAG: u32 = 1 << 10;

/// Who removes the arguments, which fixes the ESP the callback must return
/// with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallConvention {
    /// The caller pops: the callback returns with a plain `ret`.
    Cdecl,
    /// The callee pops: the callback returns with `ret 4 * args.len()`.
    Stdcall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestInterrupt {
    /// Guest function to call. It is dispatched like any CALL target, so it
    /// needs a handler and an executable page (or an override).
    pub target: u32,
    /// Stack arguments; `args[0]` ends up at `[esp + 4]` on entry.
    pub args: Vec<u32>,
    pub convention: CallConvention,
    /// Deliver only on the [`ExecCtx`] with this `thread_id`; None is any.
    pub thread: Option<u32>,
    /// While an interrupt with the same target and thread is still queued,
    /// a new one is dropped instead of queued (one pending vblank, not a
    /// backlog after a long stall).
    pub coalesce: bool,
    /// Diagnostic name.
    pub name: &'static str,
}

/// What [`Runtime::raise_interrupt`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raise {
    Queued,
    /// `coalesce` was set and an equivalent interrupt was already pending.
    Coalesced,
    /// [`MAX_PENDING`] interrupts are already waiting.
    Full,
}

#[derive(Default)]
pub(crate) struct InterruptQueue {
    pending: AtomicUsize,
    /// Keep [`IRQ_QUEUED`](crate::preempt::IRQ_QUEUED) on every context's
    /// word in step with the queue ([`Runtime::enable_leave_word`]). Read
    /// and written only under the queue's lock.
    leave_word: AtomicBool,
    queue: Mutex<VecDeque<GuestInterrupt>>,
}
impl InterruptQueue {
    /// The queue. Lock order: this lock, then the runtime's word list.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<GuestInterrupt>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Whether words carry `IRQ_QUEUED`; call with the queue's lock held.
    pub(crate) fn leave_word(&self) -> bool {
        self.leave_word.load(Ordering::Relaxed)
    }
    /// The queued count, for generated code's direct calls (`direct.rs`).
    pub(crate) fn pending_ptr(&self) -> *const AtomicUsize {
        &self.pending
    }
    /// Whether anything is queued: one load, for the runner's every-transfer
    /// check (the queue's lock and the runtime handle are taken only then).
    pub(crate) fn any_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) != 0
    }
}

impl Runtime {
    /// Keep [`IRQ_QUEUED`](crate::preempt::IRQ_QUEUED) on every context's
    /// preempt word, from now on, set exactly while the interrupt queue is
    /// not empty, so that the word alone answers "would the runner's safe
    /// point do anything". Only for generated code whose loop polls ignore
    /// that bit (preempt.rs, "The leave word"); there is no way back.
    pub fn enable_leave_word(&self) {
        let queue = self.interrupts.lock();
        self.interrupts.leave_word.store(true, Ordering::Relaxed);
        self.words.set_queued(!queue.is_empty());
    }
    /// After the queue changed, with its lock still held: bring
    /// `IRQ_QUEUED` in step.
    fn queue_changed(&self, queue: &VecDeque<GuestInterrupt>) {
        self.interrupts
            .pending
            .store(queue.len(), Ordering::Release);
        if self.interrupts.leave_word() {
            self.words.set_queued(!queue.is_empty());
        }
    }
    fn take_interrupt_for(&self, thread: u32) -> Option<GuestInterrupt> {
        if self.interrupts.pending.load(Ordering::Acquire) == 0 {
            return None;
        }
        let mut queue = self.interrupts.lock();
        let index = queue
            .iter()
            .position(|i| i.thread.is_none_or(|t| t == thread))?;
        let interrupt = queue.remove(index);
        self.queue_changed(&queue);
        interrupt
    }
    /// Queue `interrupt` for the next safe point of a matching runner. Safe
    /// to call from any thread, including while a runner is executing.
    pub fn raise_interrupt(&self, interrupt: GuestInterrupt) -> Raise {
        let mut queue = self.interrupts.lock();
        if interrupt.coalesce
            && queue
                .iter()
                .any(|i| i.target == interrupt.target && i.thread == interrupt.thread)
        {
            drop(queue);
            // The queued one may have lost its wake to a context that
            // could not take it; wake again.
            self.request_preempt(crate::preempt::INTERRUPT);
            return Raise::Coalesced;
        }
        if queue.len() >= MAX_PENDING {
            return Raise::Full;
        }
        queue.push_back(interrupt);
        self.queue_changed(&queue);
        drop(queue);
        // Bring a generated loop that calls nothing back to the runner's
        // safe point, where `deliver` runs (preempt.rs). Set after queueing,
        // so the safe point that takes the bit sees the interrupt.
        self.request_preempt(crate::preempt::INTERRUPT);
        Raise::Queued
    }
    /// Interrupts queued and not yet delivered.
    pub fn pending_interrupts(&self) -> usize {
        self.interrupts.pending.load(Ordering::Acquire)
    }
    /// Drop every queued interrupt, returning how many there were. One that
    /// is already running is not affected.
    pub fn clear_interrupts(&self) -> usize {
        let mut queue = self.interrupts.lock();
        let n = queue.len();
        queue.clear();
        self.queue_changed(&queue);
        n
    }
}

impl ExecCtx {
    /// Block delivery on this context (masked) or allow it again. Queued
    /// interrupts wait; nothing is dropped.
    pub fn set_interrupts_masked(&mut self, masked: bool) {
        self.interrupts_masked = masked;
        if !masked && self.runtime().pending_interrupts() > 0 {
            self.wake(crate::preempt::INTERRUPT);
        }
    }
    pub fn interrupts_masked(&self) -> bool {
        self.interrupts_masked
    }
    /// True while a delivered callback has not yet returned to its token.
    /// Delivery does not nest: nothing else is delivered meanwhile. A token
    /// cancelled by an unwind ends the service.
    pub fn in_interrupt(&self) -> bool {
        self.interrupt_token
            .is_some_and(|t| self.continuation_live(t))
    }
}

/// At a safe point before `transfer` is dispatched: if an interrupt is due,
/// push its frame and return the CALL that starts it. None leaves `cpu` and
/// `transfer` untouched.
///
/// Every runner transfer comes here, and almost always nothing is queued:
/// that one load is tested inline, first. The rest of the tests are the
/// same side-effect-free conditions in [`deliver_queued`], so the order
/// changes nothing.
#[inline]
pub(crate) fn deliver(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    transfer: TransferAbi,
) -> Option<TransferAbi> {
    if !ctx.runtime().interrupts.any_pending() {
        return None;
    }
    deliver_queued(cpu, ctx, transfer)
}

#[inline(never)]
fn deliver_queued(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    transfer: TransferAbi,
) -> Option<TransferAbi> {
    if !matches!(transfer.kind, abi::CALL | abi::JUMP | abi::RETURN)
        || (transfer.kind == abi::RETURN && transfer.target_pc == HARNESS_EXIT)
        || ctx.interrupts_masked
        || ctx.in_interrupt()
        || !ctx.runtime().interrupts.any_pending()
    {
        return None;
    }
    let runtime = ctx.runtime().clone();
    let interrupt = runtime.take_interrupt_for(ctx.thread_id)?;
    let saved = *cpu;
    let mut memory = runtime.memory();
    for &arg in interrupt.args.iter().rev() {
        let esp = cpu.esp.wrapping_sub(4);
        if let Err(fault) = memory.set_u32(esp, arg) {
            drop(memory);
            *cpu = saved;
            return Some(ctx.fault(saved.eip, fault));
        }
        cpu.esp = esp;
    }
    let expected_esp = match interrupt.convention {
        CallConvention::Cdecl => cpu.esp,
        CallConvention::Stdcall => saved.esp,
    };
    // The callback starts with the ABI's clear direction flag.
    cpu.eflags &= !DIRECTION_FLAG;
    let (target, name) = (interrupt.target, interrupt.name);
    let call = ctx.call_guest(cpu, &mut memory, target, move |cpu, ctx| {
        if let Some(token) = ctx.interrupt_token {
            let track = timeline::Track::Guest(ctx.thread_id);
            timeline::async_end(timeline::Cat::Interrupt, track, name, token as u64);
        }
        ctx.interrupt_token = None;
        let actual_esp = cpu.esp;
        *cpu = saved;
        if actual_esp != expected_esp {
            return Ok(TransferAbi::fault(ctx.emit(Event::InterruptImbalance {
                name,
                target,
                expected_esp,
                actual_esp,
            })));
        }
        Ok(transfer)
    });
    drop(memory);
    match call {
        Ok(call) if call.kind == abi::CALL => {
            let track = timeline::Track::Guest(ctx.thread_id);
            timeline::async_begin(timeline::Cat::Interrupt, track, name, call.resume_pc as u64);
            ctx.interrupt_token = Some(call.resume_pc);
            Some(call)
        }
        Ok(failed) => {
            *cpu = saved;
            Some(failed)
        }
        Err(fault) => {
            *cpu = saved;
            Some(ctx.fault(saved.eip, fault))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preempt::{INTERRUPT, IRQ_QUEUED, REQUEST, SLICE};
    use crate::{Dispatch, Guest};

    /// Two runtimes fed the same queue, mask and raise sequence, one with
    /// the leave word and one without. At every step, on every context: the
    /// leave word is non-zero exactly when the old two-load test
    /// (`h2_direct_quiet`: word zero and nothing queued) fails, the wake
    /// bits are the old word's, and taking them gives the same reasons.
    #[test]
    fn leave_word_matches_old_quiet() {
        const CONTEXTS: usize = 3;
        for seed in 1..=64u64 {
            let mut rng = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
            let mut next = move |n: u64| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng % n
            };
            let old = Runtime::new(Guest::new(), Dispatch::default());
            let new = Runtime::new(Guest::new(), Dispatch::default());
            let mut olds: Vec<ExecCtx> = (0..CONTEXTS as u32)
                .map(|t| ExecCtx::new(old.clone(), t))
                .collect();
            let mut news: Vec<ExecCtx> = (0..CONTEXTS as u32)
                .map(|t| ExecCtx::new(new.clone(), t))
                .collect();
            // Enabled part-way through, sometimes with a queue already
            // waiting, as a module registered after boot would.
            let enable_at = next(40);
            for step in 0..600u64 {
                if step == enable_at {
                    new.enable_leave_word();
                }
                let i = next(CONTEXTS as u64) as usize;
                match next(9) {
                    0 | 1 => {
                        let interrupt = GuestInterrupt {
                            target: 0x1000 + 0x10 * next(3) as u32,
                            args: Vec::new(),
                            convention: CallConvention::Cdecl,
                            thread: [None, Some(0), Some(1), Some(2)][next(4) as usize],
                            coalesce: next(2) == 0,
                            name: "fixture",
                        };
                        assert_eq!(
                            old.raise_interrupt(interrupt.clone()),
                            new.raise_interrupt(interrupt)
                        );
                    }
                    2 | 3 => {
                        // The queue side of `deliver_queued`.
                        let (o, n) = (&olds[i], &news[i]);
                        if !o.interrupts_masked() {
                            assert_eq!(
                                old.take_interrupt_for(o.thread_id),
                                new.take_interrupt_for(n.thread_id)
                            );
                        }
                    }
                    4 => {
                        let masked = next(2) == 0;
                        olds[i].set_interrupts_masked(masked);
                        news[i].set_interrupts_masked(masked);
                    }
                    5 => assert_eq!(olds[i].take_preempt(), news[i].take_preempt()),
                    6 => {
                        let reasons = [SLICE, REQUEST, INTERRUPT | REQUEST][next(3) as usize];
                        olds[i].defer_preempt(reasons);
                        news[i].defer_preempt(reasons);
                        old.request_preempt(reasons & REQUEST);
                        new.request_preempt(reasons & REQUEST);
                    }
                    7 => {
                        if next(8) == 0 {
                            assert_eq!(old.clear_interrupts(), new.clear_interrupts());
                        } else {
                            // A fresh context's word starts in step.
                            let (o, n) = (olds.swap_remove(i), news.swap_remove(i));
                            olds.push(o.restart());
                            news.push(n.restart());
                        }
                    }
                    _ => {
                        // Fill to the limit sometimes, so Full is reached.
                        for _ in 0..next(MAX_PENDING as u64 + 2) {
                            let interrupt = GuestInterrupt {
                                target: 0x2000,
                                args: Vec::new(),
                                convention: CallConvention::Stdcall,
                                thread: Some(next(CONTEXTS as u64) as u32),
                                coalesce: false,
                                name: "burst",
                            };
                            assert_eq!(
                                old.raise_interrupt(interrupt.clone()),
                                new.raise_interrupt(interrupt)
                            );
                        }
                    }
                }
                assert_eq!(old.pending_interrupts(), new.pending_interrupts());
                for (o, n) in olds.iter().zip(&news) {
                    let old_word = o.leave_word();
                    let word = n.leave_word();
                    let old_quiet = old_word == 0 && old.pending_interrupts() == 0;
                    assert_eq!(
                        old_word & IRQ_QUEUED,
                        0,
                        "seed {seed} step {step}: never set without the leave word"
                    );
                    assert_eq!(
                        word & !IRQ_QUEUED,
                        old_word,
                        "seed {seed} step {step}: wake bits"
                    );
                    assert_eq!(o.preempt_pending(), n.preempt_pending());
                    if step >= enable_at {
                        assert_eq!(
                            word != 0,
                            !old_quiet,
                            "seed {seed} step {step}: leave word against old test"
                        );
                        assert_eq!(word & IRQ_QUEUED != 0, new.pending_interrupts() != 0);
                    } else {
                        assert_eq!(
                            word & IRQ_QUEUED,
                            0,
                            "seed {seed} step {step}: not yet enabled"
                        );
                    }
                }
                assert_eq!(old.preempt_pending(), new.preempt_pending());
            }
        }
    }
}
