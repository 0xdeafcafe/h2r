//! Per-thread diagnostics and Rust callback continuations.
use crate::{CpuAbi, Guest, GuestFault, TransferAbi};
use std::sync::{Arc, MutexGuard};

// Keep previous module paths working while the implementation is split.
pub use crate::events::{Event, KernelCall};
pub use crate::handler::OriginalHandler;
pub use crate::runner::{HARNESS_EXIT, Runner};
pub use crate::services::Runtime;

use crate::continuation::Continuations;
pub(crate) use crate::continuation::{
    BASE as CONTINUATION_BASE, Continuation, END as CONTINUATION_END,
};

pub struct ExecCtx {
    runtime: Arc<Runtime>,
    pub thread_id: u32,
    events: Vec<Event>,
    event_ids: Vec<u32>,
    next_event_id: u32,
    pub(crate) pending_fault: Option<u32>,
    continuations: Continuations,
}
impl ExecCtx {
    pub fn new(runtime: Arc<Runtime>, thread_id: u32) -> Self {
        Self {
            runtime,
            thread_id,
            events: Vec::new(),
            event_ids: Vec::new(),
            next_event_id: 1,
            pending_fault: None,
            continuations: Continuations::default(),
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
    pub(crate) fn take_continuation(&mut self, pc: u32) -> Option<Continuation> {
        self.continuations.take(pc)
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
