//! Explicit guest transfers and callbacks use a bounded host stack.
use crate::{
    CpuAbi, Event, ExecCtx, TransferAbi, abi,
    dispatch::Handler,
    handler::{Invocation, execute},
    kernel,
};
pub const HARNESS_EXIT: u32 = 0xffff_fffc;

pub struct Runner {
    pub max_transfers: u64,
}
impl Default for Runner {
    fn default() -> Self {
        Self {
            max_transfers: 1_000_000,
        }
    }
}
impl Runner {
    pub fn run(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx, entry: u32) -> TransferAbi {
        self.run_from(cpu, ctx, TransferAbi::jump(entry))
    }
    /// Resume a stopped run with its actual control contract. In particular,
    /// RET consults generated resumes before applying an entry override.
    pub fn run_from(
        &self,
        cpu: &mut CpuAbi,
        ctx: &mut ExecCtx,
        transfer: TransferAbi,
    ) -> TransferAbi {
        // A runner nested in a handler (an override running a callback)
        // hands the direct-call budget back as the outer one set it.
        let outer = ctx.direct_budget();
        let result = ctx.measure(crate::perf::Scope::Runner, |ctx| {
            self.run_inner(cpu, ctx, transfer)
        });
        ctx.set_direct_budget(outer);
        result
    }
    fn run_inner(
        &self,
        cpu: &mut CpuAbi,
        ctx: &mut ExecCtx,
        mut transfer: TransferAbi,
    ) -> TransferAbi {
        let mut remaining = self.max_transfers;
        // H2_ENTRY_STATS: what returned `transfer`.
        let stats = crate::entry_stats::enabled();
        let mut came = crate::entry_stats::Came::Start;
        loop {
            if let Some(id) = ctx.pending_fault.take() {
                return TransferAbi::fault(id);
            }
            if transfer.valid_abi()
                && transfer.kind == abi::FAULT
                && transfer.route == abi::ENTRY
                && transfer.event_id != 0
                && ctx.event(transfer.event_id).is_some()
            {
                return transfer;
            }
            if !transfer.valid_abi() || !cpu.valid_abi() {
                return TransferAbi::fault(ctx.emit(Event::InvalidAbi));
            }
            let valid = match transfer.kind {
                abi::CALL => transfer.route == abi::ENTRY,
                abi::JUMP => matches!(transfer.route, abi::ENTRY | abi::RESUME),
                abi::RETURN => matches!(
                    transfer.route,
                    abi::ENTRY | abi::RESUME | abi::RETURN_TARGET
                ),
                abi::FAULT | abi::STOP => {
                    if transfer.route != abi::ENTRY
                        || (transfer.event_id != 0 && ctx.event(transfer.event_id).is_none())
                    {
                        return TransferAbi::fault(ctx.emit(Event::InvalidTransfer));
                    }
                    if transfer.kind == abi::FAULT && transfer.event_id == 0 {
                        return TransferAbi::fault(
                            ctx.emit(Event::UnrecordedFault { pc: cpu.eip }),
                        );
                    }
                    return transfer;
                }
                _ => false,
            };
            if !valid {
                return TransferAbi::fault(ctx.emit(Event::InvalidTransfer));
            }
            // Safe point: the last handler has returned and CPU state is
            // canonical. The wake bits are taken before the queue is looked
            // at, so a raise in between is never lost (preempt.rs). A due
            // interrupt runs first, as a call that resumes this transfer
            // when it returns. Then a preempted context stops with
            // `transfer` still undispatched and resumes with it as is.
            if let Some(p) = &mut ctx.profile {
                p.runner_safe_points += 1;
            }
            let reasons = ctx.take_preempt();
            if let Some(call) = crate::interrupt::deliver(cpu, ctx, transfer) {
                ctx.defer_preempt(reasons);
                transfer = call;
                came = crate::entry_stats::Came::Interrupt;
                continue;
            }
            if let Some(stop) = ctx.preempt_decide(reasons, transfer) {
                return stop;
            }
            // Diagnostic call chain only; the guest stack decides control.
            // Every transfer first drops the records whose return slots ESP
            // has passed, so returns, `ret n` and longjmp-style ESP restores
            // followed by a JUMP all forget the frames they abandoned. A tail
            // jump leaves ESP at its own return slot and keeps its frame.
            ctx.frames.unwind(cpu.esp);
            match transfer.kind {
                abi::CALL => ctx
                    .frames
                    .call(transfer.target_pc, transfer.resume_pc, cpu.esp),
                abi::RETURN => {}
                _ => ctx.frames.enter(transfer.target_pc),
            }
            if transfer.kind == abi::RETURN && transfer.target_pc == HARNESS_EXIT {
                cpu.eip = HARNESS_EXIT;
                return TransferAbi::stop(ctx.emit(Event::HarnessExit));
            }
            if remaining == 0 {
                return TransferAbi::fault(ctx.emit(Event::TransferLimit));
            }
            remaining -= 1;
            if let Some(p) = &mut ctx.profile {
                p.transfers += 1;
            }
            if transfer.kind == abi::RETURN
                && let Some(continuation) = ctx.take_continuation(transfer.target_pc)
            {
                cpu.eip = transfer.target_pc;
                if stats {
                    crate::entry_stats::note(crate::entry_stats::CONTINUATION, &transfer);
                    came = crate::entry_stats::Came::Rust;
                }
                ctx.direct_begin(remaining);
                transfer = execute(
                    Invocation::Continuation(continuation),
                    cpu,
                    ctx,
                    transfer.target_pc,
                );
                remaining = ctx.direct_end(remaining);
                continue;
            }
            // Tokens belong exclusively to the continuation namespace. A stale
            // generation must not fall through to an entry override or a mapped
            // executable page. JUMP cancels only an exactly matching live token.
            if crate::continuation::is_token_address(transfer.target_pc) {
                if transfer.kind == abi::JUMP {
                    ctx.cancel_continuation(transfer.target_pc);
                }
                return TransferAbi::fault(ctx.emit(Event::UnsupportedTarget {
                    pc: cpu.eip,
                    target: transfer.target_pc,
                    kind: transfer.kind,
                    route: transfer.route,
                }));
            }
            let Some(resolution) = ctx.resolve_cached(transfer) else {
                return TransferAbi::fault(ctx.emit(Event::UnsupportedTarget {
                    pc: cpu.eip,
                    target: transfer.target_pc,
                    kind: transfer.kind,
                    route: transfer.route,
                }));
            };
            // Only recognized kernel dispatch tokens bypass instruction-page
            // permissions. An arbitrary high-address override is not a token.
            let token = transfer
                .target_pc
                .checked_sub(kernel::KERNEL_TOKEN_BASE)
                .filter(|v| v.is_multiple_of(4))
                .and_then(|v| kernel::function(v / 4));
            if token.is_none()
                && let Err(fault) = ctx.check_execute_cached(&resolution, transfer.target_pc)
            {
                return ctx.fault(transfer.target_pc, fault);
            }
            let crate::exec::Resolution {
                handler,
                selector,
                function,
                generation,
                ..
            } = resolution;
            // Generated code may now invoke this entry's handler directly
            // (direct.rs) while the dispatch generation stays the same.
            if transfer.route == abi::ENTRY && selector == transfer.target_pc {
                ctx.note_direct(transfer.target_pc, &handler, generation);
            }
            cpu.eip = transfer.target_pc;
            ctx.frames.enter(function);
            let generated = matches!(handler, Handler::Native(_));
            if stats {
                use crate::entry_stats as es;
                let c = if generated {
                    let routed = transfer.route == abi::ENTRY && matches!(transfer.kind, abi::CALL | abi::JUMP);
                    let slot = routed.then(|| ctx.direct_slot_state(transfer.target_pc, &handler, generation));
                    es::cause(came, &transfer, slot)
                } else {
                    es::RUST_HANDLER
                };
                es::note(c, &transfer);
                came = if generated { es::Came::Native } else { es::Came::Rust };
            }
            // Direct calls the handler makes spend this run's transfers.
            ctx.direct_begin(remaining);
            transfer = execute(Invocation::Handler(handler), cpu, ctx, selector);
            remaining = ctx.direct_end(remaining);
            if generated
                && transfer.kind == abi::STOP
                && transfer.valid_abi()
                && let Some(recover) = ctx.runtime().stop_recovery()
            {
                transfer = recover(cpu, ctx, transfer);
            }
        }
    }
}
