//! Async entries in the game: drives an [`engine::entry`] body on the
//! runtime. Each guest call the body awaits is made with
//! `ExecCtx::call_guest`; the body is parked in the continuation, and polled
//! again with the callee's registers when it returns. Nothing is held across
//! the call but the future itself.
//!
//! Two ways in: [`Builder::entry`](crate::Builder::entry) places a body at a
//! fresh address of the feature arena (a function of ours the guest can be
//! handed: a loader, a vtable slot, a callback), and
//! [`Raw::implement_async`](crate::Raw::implement_async) makes one the body
//! of a chain at a point.

use std::sync::Arc;

/// The continuation an async entry's body is parked in while its guest
/// call runs; named in snapshot quiescence reports.
pub const ASYNC_ENTRY: &str = "feature: async entry";

use engine::entry::{self, Cx, EntryError, EntryFn, EntryFuture, Mem, Regs, Step};
use engine::memory::GuestMemory;
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, ExecCtx, GuestFault, Runtime, TransferAbi};

/// Guest memory through the runtime's lock, taken per closure.
struct RtMem(Arc<Runtime>);

impl Mem for RtMem {
    fn with(&self, f: &mut dyn FnMut(&mut dyn GuestMemory)) {
        let mut g = self.0.memory();
        f(&mut *g)
    }
}

/// One entry placed in the arena.
#[derive(Clone)]
pub struct Entry {
    pub feature: &'static str,
    pub label: &'static str,
    pub addr: u32,
    pub run: Arc<EntryFn>,
}

struct Running {
    cx: Cx,
    fut: EntryFuture,
    entry: Regs,
}

fn regs(cpu: &CpuAbi) -> Regs {
    Regs {
        eax: cpu.eax,
        ecx: cpu.ecx,
        edx: cpu.edx,
        ebx: cpu.ebx,
        esp: cpu.esp,
        ebp: cpu.ebp,
        esi: cpu.esi,
        edi: cpu.edi,
    }
}

fn fault(e: EntryError, at: u32) -> GuestFault {
    if let EntryError::IsolatedCall(message) = &e {
        eprintln!("guest entry {at:#x}: {message}");
    }
    match e {
        EntryError::Fault(m) => GuestFault {
            addr: m.addr,
            len: m.len as usize,
            access: if m.write { Access::Write } else { Access::Read },
            reason: FaultReason::Unmapped,
        },
        EntryError::Stalled | EntryError::IsolatedCall(_) => GuestFault {
            addr: at,
            len: 0,
            access: Access::Execute,
            reason: FaultReason::InvalidOperation,
        },
    }
}

/// Runs `f` for the guest call that just reached `at`. Returns to the
/// caller with the body's EAX, its `ret n`, and the caller's ESP, EBX, ESI,
/// EDI and EBP.
pub fn start(
    f: &EntryFn,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    at: u32,
) -> Result<TransferAbi, GuestFault> {
    start_with(cpu, ctx, at, |cx| f(cx))
}

/// [`start`] for a body made once (a `before` hook answering with one).
pub fn start_with(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    at: u32,
    make: impl FnOnce(Cx) -> EntryFuture,
) -> Result<TransferAbi, GuestFault> {
    let entry = regs(cpu);
    let cx = Cx::new(RtMem(Arc::clone(ctx.runtime())), entry);
    let fut = make(cx.clone());
    drive(cpu, ctx, Running { cx, fut, entry }, at)
}

fn drive(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut r: Running,
    at: u32,
) -> Result<TransferAbi, GuestFault> {
    match entry::poll(&mut r.fut, &r.cx) {
        Step::Done(Ok(ret)) => {
            let e = r.entry;
            (cpu.eax, cpu.ebx, cpu.esi, cpu.edi, cpu.ebp, cpu.esp) =
                (ret.eax, e.ebx, e.esi, e.edi, e.ebp, e.esp);
            let rt = Arc::clone(ctx.runtime());
            rt.memory().ret(cpu, u32::from(ret.pop))
        }
        Step::Done(Err(e)) => {
            eprintln!("feature: entry at {at:#010x}: {e}");
            Err(fault(e, at))
        }
        Step::Call(c) => {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let sp = cpu.esp;
            for a in c.args.iter().rev() {
                cpu.esp = cpu.esp.wrapping_sub(4);
                mem.set_u32(cpu.esp, *a)?;
            }
            for (v, reg) in [
                (c.eax, &mut cpu.eax),
                (c.ecx, &mut cpu.ecx),
                (c.edx, &mut cpu.edx),
                (c.ebx, &mut cpu.ebx),
                (c.esi, &mut cpu.esi),
                (c.edi, &mut cpu.edi),
            ] {
                if let Some(v) = v {
                    *reg = v;
                }
            }
            ctx.call_guest_named(cpu, &mut mem, c.target, ASYNC_ENTRY, move |cpu, ctx| {
                entry::resume(&r.cx, regs(cpu));
                // Whatever the callee popped, the stack is ours again.
                cpu.esp = sp;
                drive(cpu, ctx, r, at)
            })
        }
    }
}
