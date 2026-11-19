//! Handler invocation and boundary validation shared by all dispatch paths.
use crate::{CpuAbi, Event, ExecCtx, TransferAbi, abi, dispatch::Handler, exec::Continuation};

pub struct OriginalHandler(pub(crate) Handler);
impl OriginalHandler {
    /// Invoke one generated owner. CALL results still belong to the runner;
    /// use call_guest for an override that needs a Rust continuation afterward.
    pub fn invoke(&self, cpu: &mut CpuAbi, ctx: &mut ExecCtx, selector: u32) -> TransferAbi {
        execute(Invocation::Handler(self.0.clone()), cpu, ctx, selector)
    }
}

pub(crate) enum Invocation {
    Handler(Handler),
    Continuation(Continuation),
}
pub(crate) fn execute(
    invocation: Invocation,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    selector: u32,
) -> TransferAbi {
    if let Some(id) = ctx.pending_fault.take() {
        return TransferAbi::fault(id);
    }
    if !cpu.valid_abi() {
        return TransferAbi::fault(ctx.emit(Event::InvalidAbi));
    }
    let result = match invocation {
        Invocation::Handler(Handler::Native(f)) => {
            if let Some(coverage) = ctx.runtime().coverage.get() {
                coverage.record(f, selector);
            }
            Ok(crate::ffi::call_native(f, cpu, ctx, selector))
        }
        Invocation::Handler(Handler::Rust(f)) => {
            let _t = timeline::span(timeline::Cat::Hle, timeline::Track::Host, "hle call")
                .arg("pc", cpu.eip as u64);
            f(cpu, ctx, selector)
        }
        Invocation::Continuation(f) => f(cpu, ctx),
    };
    // The first memory/access fault has priority over malformed output.
    if let Some(id) = ctx.pending_fault.take() {
        return TransferAbi::fault(id);
    }
    let transfer = match result {
        Ok(t) => t,
        Err(f) => ctx.fault(cpu.eip, f),
    };
    if !transfer.valid_abi() || !cpu.valid_abi() {
        return TransferAbi::fault(ctx.emit(Event::InvalidAbi));
    }
    if transfer.kind == abi::FAULT && transfer.route == abi::ENTRY && transfer.event_id == 0 {
        return TransferAbi::fault(ctx.emit(Event::UnrecordedFault { pc: cpu.eip }));
    }
    transfer
}
