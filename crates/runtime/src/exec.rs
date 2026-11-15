//! Per-thread diagnostics and Rust callback continuations.
use crate::{CpuAbi, Guest, GuestFault, TransferAbi};
use std::sync::{Arc, MutexGuard};

// Keep previous module paths working while the implementation is split.
pub use crate::events::{
    Event, FaultReport, GuestException, GuestFrame, KernelCall, ShimEvent, StopReason,
};
pub use crate::handler::OriginalHandler;
pub use crate::runner::{HARNESS_EXIT, Runner};
pub use crate::services::Runtime;

use crate::continuation::Continuations;
pub(crate) use crate::continuation::{
    BASE as CONTINUATION_BASE, Continuation, END as CONTINUATION_END,
};

/// `repr(C)` for its first field only: generated code reads `preempt`
/// through the `ExecCtxOpaque *` it is given (`preempt.rs`). Every other
/// field stays private to Rust.
#[repr(C)]
pub struct ExecCtx {
    /// Must stay first (RUNTIME-CONTRACT.md, "Loop safe points").
    preempt: crate::preempt::WordPtr,
    runtime: Arc<Runtime>,
    pub(crate) profile: Option<crate::perf::Profile>,
    pub thread_id: u32,
    events: Vec<Event>,
    event_ids: Vec<u32>,
    next_event_id: u32,
    pub(crate) pending_fault: Option<u32>,
    continuations: Continuations,
    pub(crate) frames: crate::frames::ShadowStack,
    pub(crate) interrupts_masked: bool,
    /// Return token of the delivered interrupt callback, if one is running.
    pub(crate) interrupt_token: Option<u32>,
    /// This context's own preempt word, which `preempt` points into.
    pub(crate) word: Arc<crate::preempt::PreemptWord>,
    pub(crate) slice: crate::preempt::Slice,
    /// The transfer a preemption interrupted (`take_preempted`).
    pub(crate) preempted: Option<TransferAbi>,
}
const _: () = assert!(std::mem::offset_of!(ExecCtx, preempt) == 0);
impl ExecCtx {
    pub fn new(runtime: Arc<Runtime>, thread_id: u32) -> Self {
        let (preempt, word) = crate::preempt::new_word(&runtime);
        Self {
            preempt,
            word,
            runtime,
            thread_id,
            events: Vec::new(),
            event_ids: Vec::new(),
            next_event_id: 1,
            pending_fault: None,
            continuations: Continuations::default(),
            frames: Default::default(),
            interrupts_masked: false,
            interrupt_token: None,
            slice: Default::default(),
            preempted: None,
            profile: None,
        }
    }
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }
    /// Borrowing the context for a memory operation prevents its simultaneous
    /// use by the runner. Read/write/call/ret helpers are on the guard's Guest.
    ///
    /// ```compile_fail
    /// use runtime::{ExecCtx, Event};
    /// fn cannot_reenter(ctx: &mut ExecCtx) {
    ///     let memory = ctx.memory();
    ///     ctx.emit(Event::TransferLimit);
    ///     let _ = memory.u32(0x10000);
    /// }
    /// ```
    pub fn memory(&mut self) -> MutexGuard<'_, Guest> {
        self.runtime.memory()
    }
    pub fn events(&self) -> &[Event] {
        &self.events
    }
    /// Lookup is valid until a record is drained or evicted. Event IDs are not
    /// reused on drain; the u32 counter wraps only after skipping retained IDs.
    pub fn event(&self, id: u32) -> Option<&Event> {
        self.event_ids
            .iter()
            .position(|retained| *retained == id)
            .map(|i| &self.events[i])
    }
    /// Retain the most recent diagnostics without stopping guest execution.
    /// The first FFI fault is pinned until the invocation propagates its id.
    /// HarnessExit has normal retention; later events may evict it.
    pub fn emit(&mut self, event: Event) -> u32 {
        if self.events.len() == crate::events::MAX_EVENTS {
            let index = usize::from(self.pending_fault == self.event_ids.first().copied());
            self.events.remove(index);
            self.event_ids.remove(index);
        }
        while self.next_event_id == 0 || self.event_ids.contains(&self.next_event_id) {
            self.next_event_id = self.next_event_id.wrapping_add(1);
        }
        let id = self.next_event_id;
        self.next_event_id = id.wrapping_add(1);
        self.events.push(event);
        self.event_ids.push(id);
        id
    }
    /// Acknowledge retained diagnostics after consuming the stopped transfer.
    /// An unreported, latched FFI fault stays in the context until propagated.
    /// Draining changes neither CPU state nor callback continuations.
    pub fn take_events(&mut self) -> Vec<(u32, Event)> {
        let events = std::mem::take(&mut self.events);
        let ids = std::mem::take(&mut self.event_ids);
        let mut taken = Vec::with_capacity(events.len());
        for (id, event) in ids.into_iter().zip(events) {
            if self.pending_fault == Some(id) {
                self.events.push(event);
                self.event_ids.push(id);
            } else {
                taken.push((id, event));
            }
        }
        taken
    }
    pub fn fault(&mut self, pc: u32, fault: GuestFault) -> TransferAbi {
        TransferAbi::fault(self.emit(Event::MemoryFault { pc, fault }))
    }

    /// Using the caller's guard, save owned Rust state, push a return token, and yield
    /// a normal entry CALL. The runner consumes the closure only on RET to
    /// that token. Kernel stops leave it available for a later run_from.
    pub fn call_guest<F>(
        &mut self,
        cpu: &mut CpuAbi,
        memory: &mut Guest,
        target: u32,
        continuation: F,
    ) -> Result<TransferAbi, GuestFault>
    where
        F: FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<TransferAbi, GuestFault> + Send + 'static,
    {
        let Some(token) = self.continuations.insert(Box::new(continuation)) else {
            return Ok(TransferAbi::fault(self.emit(Event::ContinuationLimit)));
        };
        match memory.call(cpu, target, token) {
            Ok(transfer) => Ok(transfer),
            Err(fault) => {
                // Even a failed push consumes its generation, so no identity
                // can accidentally become valid again after cancellation.
                self.cancel_continuation(token);
                Err(fault)
            }
        }
    }
    /// Drop saved Rust state after a guest unwind abandons this return token.
    /// This does not repair the guest stack. Old copies of this token remain
    /// invalid even when the underlying slot holds a newer continuation.
    pub fn cancel_continuation(&mut self, token: u32) -> bool {
        self.take_continuation(token).is_some()
    }
    /// Drop all saved callback state during thread termination or a complete
    /// unwind. The caller must discard the corresponding guest stack frames.
    pub fn clear_continuations(&mut self) -> usize {
        self.continuations.clear()
    }
    pub(crate) fn continuation_live(&self, token: u32) -> bool {
        self.continuations.live(token)
    }
    pub(crate) fn take_continuation(&mut self, pc: u32) -> Option<Continuation> {
        self.continuations.take(pc)
    }

    /// The guest call chain as the runner has recorded it, innermost first,
    /// with frame 0 at `pc`. Right after a run stops this is the chain at the
    /// stopping instruction. It is diagnostic only and may be incomplete
    /// across stack switches.
    pub fn backtrace(&self, pc: u32) -> Vec<GuestFrame> {
        self.frames.snapshot(pc)
    }
    /// Forget the recorded call chain, e.g. after a thread's stack is reset.
    pub fn clear_backtrace(&mut self) {
        self.frames.clear();
    }

    /// Record an `h2shim_v1_event`. Exceptions and unknown codes are latched
    /// like a failed memory access, so a handler that ignores the id still
    /// faults. Stops are not latched, so they stay stops.
    pub(crate) fn shim_event(&mut self, pc: u32, saved_eip: u32, code: u32, arg: u32) -> u32 {
        let (event, latch) = match ShimEvent::decode(code, arg) {
            ShimEvent::Exception(exception) => (
                Event::GuestException {
                    pc,
                    saved_eip,
                    exception,
                    backtrace: self.backtrace(pc),
                },
                true,
            ),
            ShimEvent::Stop(reason) => (
                Event::GeneratedStop {
                    pc,
                    saved_eip,
                    reason,
                    backtrace: self.backtrace(pc),
                },
                false,
            ),
            ShimEvent::Unknown { code, arg } => (
                Event::UnknownShimEvent {
                    pc,
                    saved_eip,
                    code,
                    arg,
                },
                true,
            ),
        };
        let id = self.emit(event);
        if latch {
            self.pending_fault.get_or_insert(id);
        }
        id
    }

    /// Record an `h2shim_v1_unsupported` instruction. It is a stop.
    pub(crate) fn unsupported(&mut self, pc: u32, mnemonic: String, detail: String) -> u32 {
        let backtrace = self.backtrace(pc);
        self.emit(Event::Unsupported {
            pc,
            mnemonic,
            detail,
            backtrace,
        })
    }

    /// Describe a retained event: summary, PC, instruction and guest
    /// backtrace. Events that carry their own backtrace use it; the others
    /// use the chain recorded now, which is the one at the stop if this is
    /// called before running again.
    pub fn report(&self, id: u32) -> Option<FaultReport> {
        let event = self.event(id)?;
        let (pc, mnemonic, summary, backtrace) = match event {
            Event::GuestException {
                pc,
                saved_eip,
                exception,
                backtrace,
            } => (
                Some(*pc),
                Some(exception.mnemonic().to_owned()),
                format!("guest exception {exception} (reported EIP {saved_eip:#010x})"),
                Some(backtrace),
            ),
            Event::GeneratedStop {
                pc,
                saved_eip,
                reason,
                backtrace,
            } => (
                Some(*pc),
                Some(reason.mnemonic().to_owned()),
                format!("generated code stopped: {reason} (reported EIP {saved_eip:#010x})"),
                Some(backtrace),
            ),
            Event::Unsupported {
                pc,
                mnemonic,
                detail,
                backtrace,
            } => (
                Some(*pc),
                Some(mnemonic.clone()),
                format!("unsupported instruction `{mnemonic}`: {detail}"),
                Some(backtrace),
            ),
            Event::UnknownShimEvent {
                pc,
                saved_eip,
                code,
                arg,
            } => (
                Some(*pc),
                None,
                format!(
                    "unknown h2shim_v1_event code {code} (arg {arg:#x}, reported EIP {saved_eip:#010x})"
                ),
                None,
            ),
            Event::MemoryFault { pc, fault } => (
                Some(*pc),
                None,
                format!(
                    "memory fault: {:?} of {} bytes at {:#010x} ({:?})",
                    fault.access, fault.len, fault.addr, fault.reason
                ),
                None,
            ),
            Event::UnsupportedTarget {
                pc,
                target,
                kind,
                route,
            } => (
                Some(*pc),
                None,
                format!(
                    "no handler for target {target:#010x} (transfer kind {kind}, route {route})"
                ),
                None,
            ),
            Event::UnrecordedFault { pc } => (
                Some(*pc),
                None,
                "handler returned FAULT without an event".to_owned(),
                None,
            ),
            Event::KernelCall(call) => (
                Some(call.cpu.eip),
                None,
                format!("kernel call {} (ordinal {})", call.name, call.ordinal),
                None,
            ),
            other => (None, None, format!("{other:?}"), None),
        };
        let backtrace = match backtrace {
            Some(frames) => frames.clone(),
            None => self.backtrace(pc.unwrap_or(0)),
        };
        Some(FaultReport {
            id,
            pc,
            mnemonic,
            summary,
            backtrace,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrapped_event_ids_skip_retained_records_and_zero() {
        let mut ctx = ExecCtx::new(Runtime::new(Guest::new(), crate::Dispatch::default()), 1);
        let first = ctx.emit(Event::TransferLimit);
        ctx.next_event_id = u32::MAX;
        let last = ctx.emit(Event::InvalidTransfer);
        let wrapped = ctx.emit(Event::InvalidAbi);
        assert_eq!(last, u32::MAX);
        assert_ne!(wrapped, 0);
        assert_ne!(wrapped, first);
        assert_eq!(ctx.event(first), Some(&Event::TransferLimit));
        assert_eq!(
            ctx.take_events(),
            vec![
                (first, Event::TransferLimit),
                (last, Event::InvalidTransfer),
                (wrapped, Event::InvalidAbi)
            ]
        );
    }
}
