//! Explicit guest transfers and callbacks use a bounded host stack.
use crate::{
    CpuAbi, Event, ExecCtx, TransferAbi, abi,
    dispatch::Handler,
    entry_stats::{self as es, DispatchCount},
    handler::{Invocation, execute_dispatched},
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
        let diag = diagnostics(ctx);
        self.run_from_instance(cpu, ctx, transfer, diag)
    }
    /// [`run_from`](Self::run_from) on the instance named by `diag`, not
    /// the one the switches pick: for tests that hold the lean runner to
    /// the diagnostic one.
    #[doc(hidden)]
    pub fn run_from_instance(
        &self,
        cpu: &mut CpuAbi,
        ctx: &mut ExecCtx,
        transfer: TransferAbi,
        diag: bool,
    ) -> TransferAbi {
        // A runner nested in a handler (an override running a callback)
        // hands the direct-call budget back as the outer one set it.
        let outer = ctx.direct_budget();
        let result = ctx.measure(crate::perf::Scope::Runner, |ctx| {
            if diag {
                self.run_inner::<false, true>(cpu, ctx, transfer, (0, 0))
            } else {
                self.run_inner::<false, false>(cpu, ctx, transfer, (0, 0))
            }
        });
        ctx.set_direct_budget(outer);
        ctx.census_run_end(&result);
        result
    }
    /// Run a scoped guest call on its existing stack. Stop when its actual
    /// return has popped the entry slot, before dispatching the caller.
    pub(crate) fn scoped(
        &self,
        cpu: &mut CpuAbi,
        ctx: &mut ExecCtx,
        transfer: TransferAbi,
        entry_esp: u32,
        return_pc: u32,
    ) -> TransferAbi {
        let outer = ctx.direct_budget();
        let scope = (entry_esp, return_pc);
        let result = if diagnostics(ctx) {
            self.run_inner::<true, true>(cpu, ctx, transfer, scope)
        } else {
            self.run_inner::<true, false>(cpu, ctx, transfer, scope)
        };
        ctx.set_direct_budget(outer);
        ctx.census_run_end(&result);
        result
    }

    /// The runner's loop: validate the transfer, take the safe point, then
    /// [`step`]. `DIAG` adds the entry stats, the dispatch census, the
    /// profile counters, coverage, the HLE census and the CPU ABI check on
    /// every transfer (the lean one checks it on the first only);
    /// without them the routing, the safe point, every event and every
    /// transfer count are the same.
    fn run_inner<const SCOPED: bool, const DIAG: bool>(
        &self,
        cpu: &mut CpuAbi,
        ctx: &mut ExecCtx,
        mut transfer: TransferAbi,
        scope: (u32, u32),
    ) -> TransferAbi {
        let mut remaining = self.max_transfers;
        let mut diag = Diag::new::<DIAG>(ctx);
        // The lean instance checks the CPU's ABI header once, at the run's
        // first transfer, in the same place the diagnostic one checks it
        // every time: a broken header handed in is a clean InvalidAbi.
        let mut check_cpu = true;
        loop {
            if let Some(id) = ctx.pending_fault.take() {
                return TransferAbi::fault(id);
            }
            if !transfer.valid_abi() {
                return TransferAbi::fault(ctx.emit(Event::InvalidAbi));
            }
            if transfer.kind == abi::FAULT
                && transfer.route == abi::ENTRY
                && transfer.event_id != 0
                && ctx.event(transfer.event_id).is_some()
            {
                return transfer;
            }
            if (DIAG || check_cpu) && !cpu.valid_abi() {
                return TransferAbi::fault(ctx.emit(Event::InvalidAbi));
            }
            check_cpu = false;
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
            if DIAG && let Some(p) = &mut ctx.profile {
                p.runner_safe_points += 1;
            }
            let reasons = ctx.take_preempt();
            if let Some(call) = crate::interrupt::deliver(cpu, ctx, transfer) {
                ctx.defer_preempt(reasons);
                transfer = call;
                diag.came = es::Came::Interrupt;
                continue;
            }
            let slice_expired = DIAG && diag.census && ctx.slice.expired;
            if let Some(stop) = ctx.preempt_decide(reasons, transfer) {
                if DIAG && let Some(census) = &ctx.runtime().dispatch_census {
                    census.note(if slice_expired {
                        DispatchCount::SlicePreempt
                    } else {
                        DispatchCount::RequestPreempt
                    });
                }
                return stop;
            }
            // Guest execution goes on here: from a memory fault's state, if
            // the last run on this context ended with one (fault_census).
            ctx.census_continue();
            match step::<SCOPED, DIAG, false>(cpu, ctx, transfer, scope, &mut remaining, &mut diag) {
                Step::Next(next) => transfer = next,
                Step::Done(done) => return done,
            }
        }
    }
}

/// Whether this run takes the diagnostic instance: always in a debug build,
/// and whenever a switch that exists asks for something only it records.
/// Chosen once per run; a switch turned on mid-run applies from the next.
pub(crate) fn diagnostics(ctx: &ExecCtx) -> bool {
    cfg!(debug_assertions)
        || es::enabled()
        || ctx.profile.is_some()
        || ctx.runtime().dispatch_census.is_some()
        || ctx.runtime().coverage.get().is_some()
        || crate::trap::hle_census()
}

/// The diagnostic instance's per-run state; inert in the lean one.
pub(crate) struct Diag {
    /// H2_ENTRY_STATS.
    stats: bool,
    /// H2_DISPATCH_STATS.
    census: bool,
    /// H2_ENTRY_STATS: what returned the transfer about to be dispatched.
    came: es::Came,
}
impl Diag {
    pub(crate) fn new<const DIAG: bool>(ctx: &ExecCtx) -> Self {
        Self {
            stats: DIAG && es::enabled(),
            census: DIAG && ctx.runtime().dispatch_census.is_some(),
            came: es::Came::Start,
        }
    }
    /// The host-call thunk's (direct.rs): what it routes, a generated
    /// handler returned.
    pub(crate) fn thunk<const DIAG: bool>(ctx: &ExecCtx) -> Self {
        Self {
            came: es::Came::Thunk,
            ..Self::new::<DIAG>(ctx)
        }
    }
}

/// What one routed transfer led to.
pub(crate) enum Step {
    /// The transfer the handler returned, for the next safe point.
    Next(TransferAbi),
    /// The run ends with this.
    Done(TransferAbi),
}

/// One transfer past the safe point: the diagnostic frame record, the scope
/// and transfer-limit tests, then the continuation, token, resolution and
/// execute checks, the handler and stop recovery. The runner's loop calls
/// it after every safe point; the host-call thunk (`THUNK`, direct.rs)
/// calls it where its generated caller has just seen the leave word zero.
/// The thunk has replayed the transfer's frame record itself, and its
/// caller's runner counts the transfer and any its handler made directly,
/// so with `THUNK` neither is recorded here.
pub(crate) fn step<const SCOPED: bool, const DIAG: bool, const THUNK: bool>(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    transfer: TransferAbi,
    scope: (u32, u32),
    remaining: &mut u64,
    diag: &mut Diag,
) -> Step {
    // Diagnostic call chain only; the guest stack decides control.
    // Every transfer first drops the records whose return slots ESP
    // has passed, so returns, `ret n` and longjmp-style ESP restores
    // followed by a JUMP all forget the frames they abandoned. A tail
    // jump leaves ESP at its own return slot and keeps its frame.
    if !THUNK {
        ctx.frames.unwind(cpu.esp);
        match transfer.kind {
            abi::CALL => ctx
                .frames
                .call(transfer.target_pc, transfer.resume_pc, cpu.esp),
            abi::RETURN => {}
            _ => ctx.frames.enter(transfer.target_pc),
        }
    }
    let returned = if SCOPED {
        cpu.esp > scope.0 && transfer.target_pc == scope.1
    } else {
        transfer.target_pc == HARNESS_EXIT
    };
    if transfer.kind == abi::RETURN && returned {
        cpu.eip = HARNESS_EXIT;
        return Step::Done(TransferAbi::stop(ctx.emit(Event::HarnessExit)));
    }
    if *remaining == 0 {
        return Step::Done(TransferAbi::fault(ctx.emit(Event::TransferLimit)));
    }
    *remaining -= 1;
    if DIAG && !THUNK && let Some(p) = &mut ctx.profile {
        p.transfers += 1;
    }
    if transfer.kind == abi::RETURN
        && let Some(continuation) = ctx.take_continuation(transfer.target_pc)
    {
        cpu.eip = transfer.target_pc;
        if DIAG && diag.stats {
            es::note(es::CONTINUATION, &transfer);
            diag.came = es::Came::Rust;
        }
        ctx.direct_begin(*remaining);
        let next = execute_dispatched::<DIAG>(
            Invocation::Continuation(continuation),
            cpu,
            ctx,
            transfer.target_pc,
        );
        *remaining = ctx.direct_end(*remaining, !THUNK);
        return Step::Next(next);
    }
    // Tokens belong exclusively to the continuation namespace. A stale
    // generation must not fall through to an entry override or a mapped
    // executable page. JUMP cancels only an exactly matching live token.
    if crate::continuation::is_token_address(transfer.target_pc) {
        if transfer.kind == abi::JUMP {
            ctx.cancel_continuation(transfer.target_pc);
        }
        return Step::Done(TransferAbi::fault(ctx.emit(Event::UnsupportedTarget {
            pc: cpu.eip,
            target: transfer.target_pc,
            kind: transfer.kind,
            route: transfer.route,
        })));
    }
    let Some(resolution) = ctx.resolve_cached(transfer) else {
        return Step::Done(TransferAbi::fault(ctx.emit(Event::UnsupportedTarget {
            pc: cpu.eip,
            target: transfer.target_pc,
            kind: transfer.kind,
            route: transfer.route,
        })));
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
        return Step::Done(ctx.fault(transfer.target_pc, fault));
    }
    let crate::exec::Resolution {
        handler,
        selector,
        function,
        ..
    } = resolution;
    let native = matches!(handler, Handler::Native(_) | Handler::NativeAdapter(_));
    let generated = matches!(handler, Handler::Native(_));
    cpu.eip = transfer.target_pc;
    ctx.frames.enter(function);
    if DIAG && diag.stats {
        let c = if generated {
            let routed =
                transfer.route == abi::ENTRY && matches!(transfer.kind, abi::CALL | abi::JUMP);
            let slot = routed.then(|| ctx.live_state(transfer.target_pc));
            es::cause(diag.came, &transfer, slot)
        } else {
            es::RUST_HANDLER
        };
        es::note(c, &transfer);
        diag.came = if generated { es::Came::Native } else { es::Came::Rust };
    }
    // Direct calls the handler makes spend this run's transfers.
    ctx.direct_begin(*remaining);
    // The access trap's HLE census (trap.rs): which Rust handler runs.
    let hle = (DIAG && !native && crate::trap::hle_census()).then(|| {
        let target = transfer.target_pc;
        let thread = timeline::phase::running_guest_thread().unwrap_or(crate::trap::NO_THREAD);
        (crate::trap::hle_enter(target), target, thread)
    });
    let mut next = execute_dispatched::<DIAG>(Invocation::Handler(handler), cpu, ctx, selector);
    if let Some((prev, target, thread)) = hle {
        crate::trap::hle_leave(prev, target, thread, next.kind == abi::STOP);
    }
    *remaining = ctx.direct_end(*remaining, !THUNK);
    // The census counts generated handlers handing control back to the
    // runner; one the thunk invoked hands it back to generated code.
    if DIAG && diag.census && !(THUNK && generated) {
        let count = if generated {
            DispatchCount::native_exit(next)
        } else {
            DispatchCount::Hle
        };
        if let Some(census) = &ctx.runtime().dispatch_census {
            census.note(count);
        }
    }
    if native
        && next.kind == abi::STOP
        && next.valid_abi()
        && let Some(recover) = ctx.runtime().stop_recovery()
    {
        next = recover(cpu, ctx, next);
    }
    Step::Next(next)
}
