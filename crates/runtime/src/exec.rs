//! Per-thread diagnostics and Rust callback continuations.
use crate::{CpuAbi, Guest, GuestFault, TransferAbi};
use std::sync::Arc;

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
    /// Must stay second (RUNTIME-CONTRACT.md, "Direct calls"): `direct`'s
    /// block, or null while direct calls are disabled.
    pub(crate) direct_ptr: crate::direct::DirectPtr,
    pub(crate) direct: crate::direct::Direct,
    runtime: Arc<Runtime>,
    pub(crate) profile: Option<crate::perf::Profile>,
    pub(crate) fast_memory: Option<crate::ffi::OwnedFastAccess>,
    pub(crate) fast_memory_enabled: bool,
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
    /// Recently resolved transfers, so a hot call or return skips the
    /// dispatch lock and hash lookups (`resolve_cached`).
    resolved: ResolveCache,
}

/// Entries in the resolve cache. The game makes tens of thousands of
/// runner transfers a frame, to thousands of distinct function entries and
/// return sites; 256 entries missed often enough that the dispatch lock and
/// hash lookups were a measurable part of a frame.
const RESOLVE_WAYS: usize = 4096;

#[derive(Clone)]
struct Resolved {
    generation: u64,
    /// The guest layout under which `pc` last passed the runner's execute
    /// check (0: not yet), see [`ExecCtx::check_execute_cached`].
    layout: u64,
    route: u32,
    pc: u32,
    handler: crate::dispatch::Handler,
    selector: u32,
    function: u32,
}

/// What [`ExecCtx::resolve_cached`] found for a transfer.
pub(crate) struct Resolution {
    pub handler: crate::dispatch::Handler,
    pub selector: u32,
    pub function: u32,
    pub generation: u64,
    /// The cache slot holding it, and the layout its execute check last
    /// passed under there.
    slot: usize,
    layout: u64,
}

/// Direct-mapped by target pc. An entry is only used while its dispatch
/// generation is current, so any `edit_dispatch` invalidates the whole cache.
struct ResolveCache(Box<[Option<Resolved>]>);

impl Default for ResolveCache {
    fn default() -> Self {
        Self(vec![None; RESOLVE_WAYS].into_boxed_slice())
    }
}

/// The cache slot for `pc`: a multiplicative hash, as function entries are
/// 16-byte aligned and return sites cluster.
#[inline]
fn resolve_slot(pc: u32) -> usize {
    const BITS: u32 = RESOLVE_WAYS.trailing_zeros();
    const _: () = assert!(RESOLVE_WAYS.is_power_of_two());
    (pc.wrapping_mul(0x9e37_79b1) >> (32 - BITS)) as usize
}
const _: () = assert!(std::mem::offset_of!(ExecCtx, preempt) == 0);
const _: () = assert!(std::mem::offset_of!(ExecCtx, direct_ptr) == size_of::<*const u32>());
impl ExecCtx {
    pub fn new(runtime: Arc<Runtime>, thread_id: u32) -> Self {
        let direct = crate::direct::Direct::new(&runtime);
        Self::with_caches(runtime, thread_id, direct, ResolveCache::default())
    }

    /// Start an independent invocation on the same runtime and thread ID.
    /// Only the dispatch caches survive. Callback tokens, CPU diagnostics and
    /// preemption state start fresh, just as with a new context.
    pub fn restart(self) -> Self {
        let Self {
            runtime,
            thread_id,
            mut direct,
            resolved,
            ..
        } = self;
        direct.restart();
        Self::with_caches(runtime, thread_id, direct, resolved)
    }

    fn with_caches(
        runtime: Arc<Runtime>,
        thread_id: u32,
        direct: crate::direct::Direct,
        resolved: ResolveCache,
    ) -> Self {
        let (preempt, word) = crate::preempt::new_word(&runtime);
        let mut ctx = Self {
            preempt,
            direct_ptr: direct.as_ptr(),
            direct,
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
            resolved,
            profile: None,
            fast_memory: None,
            fast_memory_enabled: true,
        };
        if ctx.runtime.coverage.get().is_some() {
            ctx.enable_direct_calls(false);
        }
        ctx
    }
    /// Enable the optional generated-C RAM view for subsequent handler calls.
    /// Disabling it keeps the same generated module on checked Rust helpers,
    /// which is useful for comparisons and diagnosing a fast-path discrepancy.
    pub fn enable_fast_memory(&mut self, enabled: bool) {
        crate::ffi::sync_accesses(self);
        self.fast_memory_enabled = enabled;
    }
    /// [`Runtime::resolve`] through this context's cache, with the dispatch
    /// generation the answer belongs to. Failures are not cached: they end
    /// the run with a fault anyway.
    #[inline]
    pub(crate) fn resolve_cached(&mut self, t: TransferAbi) -> Option<Resolution> {
        let slot = resolve_slot(t.target_pc);
        let generation = self.runtime.dispatch_generation();
        if let Some(e) = &self.resolved.0[slot]
            && e.generation == generation
            && e.pc == t.target_pc
            && e.route == t.route
        {
            return Some(Resolution {
                handler: e.handler.clone(),
                selector: e.selector,
                function: e.function,
                generation,
                slot,
                layout: e.layout,
            });
        }
        self.resolve_miss(t, slot)
    }
    #[inline(never)]
    fn resolve_miss(&mut self, t: TransferAbi, slot: usize) -> Option<Resolution> {
        let (generation, found) = self.runtime.resolve_with_generation(t);
        let (handler, selector, function) = found?;
        self.resolved.0[slot] = Some(Resolved {
            generation,
            layout: 0,
            route: t.route,
            pc: t.target_pc,
            handler: handler.clone(),
            selector,
            function,
        });
        Some(Resolution {
            handler,
            selector,
            function,
            generation,
            slot,
            layout: 0,
        })
    }
    /// The runner's execute check of a resolved target `pc`
    /// ([`Runtime::check_execute`]), made only when the guest layout has
    /// changed since `pc` last passed it from this cache entry. A layout
    /// names one set of mappings and permissions and is never reused
    /// (`Guest::layout_id`), so the answer under it cannot have changed; a
    /// fault is never remembered.
    #[inline]
    pub(crate) fn check_execute_cached(
        &mut self,
        r: &Resolution,
        pc: u32,
    ) -> Result<(), GuestFault> {
        // Read before checking: a change racing the check moves the layout
        // past this value, and the next transfer checks again.
        let layout = self.runtime.layout_epoch();
        if r.layout == layout {
            return Ok(());
        }
        self.runtime.check_execute(pc)?;
        if let Some(e) = &mut self.resolved.0[r.slot]
            && e.pc == pc
            && e.generation == r.generation
        {
            e.layout = layout;
        }
        Ok(())
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
    pub fn memory(&mut self) -> crate::ffi::MemoryGuard<'_> {
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
        self.call_guest_named(
            cpu,
            memory,
            target,
            std::any::type_name::<F>(),
            continuation,
        )
    }
    /// [`ExecCtx::call_guest`], naming the continuation for snapshot
    /// quiescence reports ([`ExecCtx::live_continuations`]). Unnamed ones
    /// are reported by their closure's type name.
    pub fn call_guest_named<F>(
        &mut self,
        cpu: &mut CpuAbi,
        memory: &mut Guest,
        target: u32,
        origin: &'static str,
        continuation: F,
    ) -> Result<TransferAbi, GuestFault>
    where
        F: FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<TransferAbi, GuestFault> + Send + 'static,
    {
        self.push_continuation(cpu, memory, target, origin, None, Box::new(continuation))
    }
    /// [`ExecCtx::call_guest_named`] for a continuation a snapshot load can
    /// make again: its closure must depend only on `origin`, `target`,
    /// `state` and the context it runs on (its thread id, the runtime), so
    /// that whoever owns `origin` can rebuild it at the same token. Snapshot
    /// quiescence lists it as rebuildable instead of as a blocker
    /// (docs/runtime/snapshots.md, "Rebuildable continuations").
    pub fn call_guest_rebuildable<F>(
        &mut self,
        cpu: &mut CpuAbi,
        memory: &mut Guest,
        target: u32,
        origin: &'static str,
        state: [u32; 4],
        continuation: F,
    ) -> Result<TransferAbi, GuestFault>
    where
        F: FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<TransferAbi, GuestFault> + Send + 'static,
    {
        self.push_continuation(
            cpu,
            memory,
            target,
            origin,
            Some(state),
            Box::new(continuation),
        )
    }
    fn push_continuation(
        &mut self,
        cpu: &mut CpuAbi,
        memory: &mut Guest,
        target: u32,
        origin: &'static str,
        state: Option<[u32; 4]>,
        continuation: Continuation,
    ) -> Result<TransferAbi, GuestFault> {
        let Some(token) = self
            .continuations
            .insert(continuation, origin, target, state)
        else {
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
    /// Callback continuations still waiting for their guest call to
    /// return. Each is Rust state a machine snapshot cannot save.
    pub fn live_continuations(&self) -> Vec<crate::continuation::ContinuationInfo> {
        self.continuations.live_list()
    }
    /// The continuation pool as plain data, for a machine snapshot: refuses
    /// while a continuation that is not rebuildable is live.
    pub fn continuation_layout(
        &self,
    ) -> Result<crate::ContinuationLayout, crate::continuation::LayoutError> {
        self.continuations.layout()
    }
    /// Replace the continuation pool with a saved one, rebuilding each live
    /// continuation through `rebuild(origin, target, state)`, so its token,
    /// which is on the guest stack, resolves again and later tokens match
    /// the saved run's. Nothing changes on an error.
    pub fn restore_continuations(
        &mut self,
        layout: &crate::ContinuationLayout,
        rebuild: impl FnMut(&str, u32, [u32; 4]) -> Option<(&'static str, crate::Continuation)>,
    ) -> Result<(), crate::continuation::LayoutError> {
        self.continuations.restore(layout, rebuild)
    }
    /// A delivered interrupt callback is running on this context: the
    /// interrupted CPU state is held host-side until it returns.
    pub fn interrupt_in_service(&self) -> bool {
        self.interrupt_token.is_some()
    }
    /// A fault is waiting to end this context's next run.
    pub fn fault_pending(&self) -> bool {
        self.pending_fault.is_some()
    }
    pub(crate) fn continuation_live(&self, token: u32) -> bool {
        self.continuations.live(token)
    }
    /// The runner asks at every RETURN; only token addresses can hold one,
    /// so the usual return answers inline.
    #[inline]
    pub(crate) fn take_continuation(&mut self, pc: u32) -> Option<Continuation> {
        if !crate::continuation::is_token_address(pc) {
            return None;
        }
        self.continuations.take(pc)
    }

    /// The guest call chain as the runner has recorded it, innermost first,
    /// with frame 0 at `pc`. Right after a run stops this is the chain at the
    /// stopping instruction. It is diagnostic only and may be incomplete
    /// across stack switches.
    pub fn backtrace(&self, pc: u32) -> Vec<GuestFrame> {
        self.backtrace_with_direct(pc)
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
    use crate::guest::{Access, Permissions};
    use crate::{Dispatch, Event, Runner, abi};

    /// The runner remembers that a cached target passed its execute check
    /// only for the guest layout it passed under: every protect, unmap,
    /// map or whole-Guest replacement after it is seen at the next
    /// transfer, with the C RAM view on or off, from this thread or another.
    #[test]
    fn a_remembered_execute_check_never_outlives_a_layout_change() {
        const CODE: u32 = 0x0001_0000;
        for fast in [false, true] {
            let mut dispatch = Dispatch::default();
            let _ = dispatch.register_override(CODE, "stop", |cpu, _, _| {
                cpu.eax += 1;
                Ok(TransferAbi::stop(0))
            });
            let rt = Runtime::new(Guest::new(), dispatch);
            rt.memory()
                .map_zeroed(CODE, 0x1000, Permissions::RX)
                .unwrap();
            if fast {
                rt.fast_memory();
            }
            let mut ctx = ExecCtx::new(rt.clone(), 1);
            let mut cpu = CpuAbi::default();
            let mut run = |ctx: &mut ExecCtx| {
                let before = cpu.eax;
                let t = Runner::default().run(&mut cpu, ctx, CODE);
                if t.kind == abi::STOP {
                    assert_eq!(cpu.eax, before + 1);
                    true
                } else {
                    assert!(
                        matches!(ctx.event(t.event_id), Some(Event::MemoryFault { fault, .. })
                            if fault.access == Access::Execute && fault.addr == CODE),
                        "{t:?} {:?}",
                        ctx.events()
                    );
                    assert_eq!(cpu.eax, before);
                    false
                }
            };
            assert!(run(&mut ctx));
            assert!(run(&mut ctx), "cached and remembered");
            rt.memory().protect(CODE, Permissions::READ).unwrap();
            assert!(!run(&mut ctx), "protected after the check was remembered");
            assert!(!run(&mut ctx), "a fault is not remembered");
            rt.memory().protect(CODE, Permissions::RX).unwrap();
            assert!(run(&mut ctx));
            rt.memory().unmap(CODE, 0x1000).unwrap();
            assert!(!run(&mut ctx), "unmapped");
            rt.memory()
                .map_zeroed(CODE, 0x1000, Permissions::RX)
                .unwrap();
            assert!(run(&mut ctx));
            let other = rt.clone();
            std::thread::spawn(move || other.memory().protect(CODE, Permissions::RW).unwrap())
                .join()
                .unwrap();
            assert!(!run(&mut ctx), "protected by another thread");
            rt.memory().protect(CODE, Permissions::RX).unwrap();
            assert!(run(&mut ctx));
            // A whole new Guest, with nothing executable: a fresh layout.
            *rt.memory() = Guest::new();
            assert!(!run(&mut ctx), "replaced Guest");
            rt.memory()
                .map_zeroed(CODE, 0x1000, Permissions::RX)
                .unwrap();
            assert!(run(&mut ctx));
            // A guard that changes nothing leaves the layout, and the memo.
            let layout = rt.layout_epoch();
            let _ = rt.memory().u32(CODE);
            assert_eq!(rt.layout_epoch(), layout);
            assert!(run(&mut ctx));
        }
    }

    /// Many distinct targets through one context: every slot of the
    /// direct-mapped cache is answered for its own pc and route, whatever
    /// else shares it.
    #[test]
    fn the_resolve_cache_answers_each_target_with_its_own_handler() {
        const BASE: u32 = 0x0010_0000;
        const N: u32 = 3 * RESOLVE_WAYS as u32;
        let mut dispatch = Dispatch::default();
        for i in 0..N {
            let _ = dispatch.register_override(BASE + i * 16, "id", move |cpu, _, selector| {
                cpu.eax = i;
                cpu.ecx = selector;
                Ok(TransferAbi::stop(0))
            });
        }
        let rt = Runtime::new(Guest::new(), dispatch);
        rt.memory()
            .map_zeroed(BASE, N * 16, Permissions::RX)
            .unwrap();
        let mut ctx = ExecCtx::new(rt, 1);
        let mut cpu = CpuAbi::default();
        for round in 0..2 {
            for i in (0..N).rev().step_by(7).chain(0..N) {
                let t = Runner::default().run(&mut cpu, &mut ctx, BASE + i * 16);
                assert_eq!(t.kind, abi::STOP, "round {round} target {i}");
                assert_eq!((cpu.eax, cpu.ecx), (i, BASE + i * 16));
            }
        }
    }

    #[test]
    fn restarting_keeps_dispatch_storage_but_discards_invocation_state() {
        const CODE: u32 = 0x10000;
        let runtime = Runtime::new(Guest::new(), Dispatch::default());
        runtime.edit_dispatch(|d| {
            d.register_override(CODE, "first", |cpu, _, _| {
                cpu.eax = 1;
                Ok(TransferAbi::stop(0))
            });
        });
        runtime
            .memory()
            .map_zeroed(CODE, 0x1000, Permissions::RX)
            .unwrap();
        let mut ctx = ExecCtx::new(runtime.clone(), 7);
        let mut cpu = CpuAbi::default();
        let runner = Runner::default();
        assert_eq!(runner.run(&mut cpu, &mut ctx, CODE).kind, abi::STOP);
        assert_eq!(cpu.eax, 1);

        let cache = ctx.resolved.0.as_ptr();
        let slot = resolve_slot(CODE);
        let retained_generation = ctx.resolved.0[slot].as_ref().unwrap().generation;
        ctx.emit(Event::InvalidTransfer);
        ctx.arm_preemption(1);
        ctx.wake(crate::preempt::REQUEST);
        ctx.enable_profiling(true);

        let mut ctx = ctx.restart();
        assert_eq!(ctx.thread_id, 7);
        assert_eq!(ctx.resolved.0.as_ptr(), cache);
        assert_eq!(
            ctx.resolved.0[slot].as_ref().unwrap().generation,
            retained_generation
        );
        assert!(ctx.events().is_empty());
        assert!(!ctx.preemption_armed());
        assert_eq!(ctx.preempt_pending(), 0);
        assert!(ctx.profile().is_none());

        runtime.edit_dispatch(|d| {
            d.register_override(CODE, "second", |cpu, _, _| {
                cpu.eax = 2;
                Ok(TransferAbi::stop(0))
            });
        });
        assert_eq!(runner.run(&mut cpu, &mut ctx, CODE).kind, abi::STOP);
        assert_eq!(cpu.eax, 2);
        runtime.memory().protect(CODE, Permissions::READ).unwrap();
        assert_eq!(runner.run(&mut cpu, &mut ctx, CODE).kind, abi::FAULT);
    }

    #[test]
    fn the_dispatch_census_counts_hle_and_why_a_slice_stopped() {
        const CODE: u32 = 0x10000;
        let mut dispatch = Dispatch::default();
        dispatch.register_override(CODE, "charge", |_, ctx, _| {
            ctx.charge_accesses(1);
            Ok(TransferAbi::jump(CODE))
        });
        let mut runtime = Runtime::new(Guest::new(), dispatch);
        Arc::get_mut(&mut runtime).unwrap().dispatch_census = Some(Default::default());
        runtime
            .memory()
            .map_zeroed(CODE, 0x1000, Permissions::RX)
            .unwrap();
        let first = runtime.dispatch_counts().unwrap();
        let mut ctx = ExecCtx::new(runtime.clone(), 1);
        let mut cpu = CpuAbi::default();
        let runner = Runner::default();

        ctx.arm_preemption(1);
        assert_eq!(runner.run(&mut cpu, &mut ctx, CODE), TransferAbi::stop(0));
        let resume = ctx.take_preempted().unwrap();
        ctx.arm_preemption(1);
        ctx.wake(crate::preempt::REQUEST);
        assert_eq!(runner.run_from(&mut cpu, &mut ctx, resume), TransferAbi::stop(0));

        let counts = runtime.dispatch_counts().unwrap();
        assert_eq!(
            counts.since(first).filter(|(_, n)| *n > 0).collect::<Vec<_>>(),
            [("hle", 1), ("slice_preempt", 1), ("request_preempt", 1)]
        );
    }

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
