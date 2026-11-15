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
use std::sync::atomic::{AtomicUsize, Ordering};

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
    queue: Mutex<VecDeque<GuestInterrupt>>,
}
impl InterruptQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<GuestInterrupt>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn take_for(&self, thread: u32) -> Option<GuestInterrupt> {
        if self.pending.load(Ordering::Acquire) == 0 {
            return None;
        }
        let mut queue = self.lock();
        let index = queue
            .iter()
            .position(|i| i.thread.is_none_or(|t| t == thread))?;
        let interrupt = queue.remove(index);
        self.pending.store(queue.len(), Ordering::Release);
        interrupt
    }
}

impl Runtime {
    /// Queue `interrupt` for the next safe point of a matching runner. Safe
    /// to call from any thread, including while a runner is executing.
    pub fn raise_interrupt(&self, interrupt: GuestInterrupt) -> Raise {
        let mut queue = self.interrupts.lock();
        if interrupt.coalesce
            && queue
                .iter()
                .any(|i| i.target == interrupt.target && i.thread == interrupt.thread)
        {
            return Raise::Coalesced;
        }
        if queue.len() >= MAX_PENDING {
            return Raise::Full;
        }
        queue.push_back(interrupt);
        self.interrupts
            .pending
            .store(queue.len(), Ordering::Release);
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
        self.interrupts.pending.store(0, Ordering::Release);
        n
    }
}

impl ExecCtx {
    /// Block delivery on this context (masked) or allow it again. Queued
    /// interrupts wait; nothing is dropped.
    pub fn set_interrupts_masked(&mut self, masked: bool) {
        self.interrupts_masked = masked;
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
pub(crate) fn deliver(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    transfer: TransferAbi,
) -> Option<TransferAbi> {
    if !matches!(transfer.kind, abi::CALL | abi::JUMP | abi::RETURN)
        || (transfer.kind == abi::RETURN && transfer.target_pc == HARNESS_EXIT)
        || ctx.interrupts_masked
        || ctx.in_interrupt()
    {
        return None;
    }
    let runtime = ctx.runtime().clone();
    let interrupt = runtime.interrupts.take_for(ctx.thread_id)?;
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
