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

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::Arc;

/// The continuation an async entry's body is parked in while its guest
/// call runs; named in snapshot quiescence reports.
pub const ASYNC_ENTRY: &str = "feature: async entry";
/// Owned render entries use the memory of the context executing each poll.
/// The pipeline can move these callbacks with the suspended render stack.
pub const RENDER_ENTRY: &str = "feature: portable render entry";

use engine::entry::{self, Cx, EntryError, EntryFn, EntryFuture, Mem, Regs, Step};
use engine::memory::GuestMemory;
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, Runtime, TransferAbi};

/// Guest memory through the runtime's lock, taken per closure.
struct RtMem(Arc<Runtime>);

impl Mem for RtMem {
    fn with(&self, f: &mut dyn FnMut(&mut dyn GuestMemory)) {
        let mut g = self.0.memory();
        f(&mut *g)
    }
}

// A guest call unwinds out of the poll before the executor starts it.
// At the next poll (possibly on the game thread after hand-back), bind
// memory to that poll's context. No Runtime or memory guard is parked in
// a render continuation.
struct RenderMem;

#[derive(Clone, Copy)]
struct RenderScope {
    memory: NonNull<()>,
    access: unsafe fn(NonNull<()>, &mut dyn FnMut(&mut dyn GuestMemory)),
}

thread_local! {
    static RENDER_MEMORY: RefCell<Option<RenderScope>> = const { RefCell::new(None) };
    static WRITEBACK: std::cell::Cell<Option<runtime::guest::Writeback>> =
        const { std::cell::Cell::new(None) };
}

/// Lend the current memory guard, or its checked writeback view, for one
/// synchronous poll. The concrete borrow never outlives this call.
fn with_render_memory<M: GuestMemory, T>(guest: &mut M, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<RenderScope>);

    impl Drop for Restore {
        fn drop(&mut self) {
            RENDER_MEMORY.replace(self.0.take());
        }
    }

    unsafe fn access<M: GuestMemory>(memory: NonNull<()>, f: &mut dyn FnMut(&mut dyn GuestMemory)) {
        // SAFETY: the installing frame created this pointer from &mut M,
        // and RenderMem holds the exclusive thread-local RefMut through f.
        f(unsafe { memory.cast::<M>().as_mut() });
    }

    let scope = RenderScope { memory: NonNull::from(guest).cast(), access: access::<M> };
    let _restore = Restore(RENDER_MEMORY.replace(Some(scope)));
    run()
}

impl Mem for RenderMem {
    fn with(&self, f: &mut dyn FnMut(&mut dyn GuestMemory)) {
        RENDER_MEMORY.with_borrow_mut(|current| {
            let scope = current.as_ref().expect("render Cx outside its entry");
            // SAFETY: scope's matching access function and typed pointer
            // were installed together. The frame restores them even on panic.
            unsafe { (scope.access)(scope.memory, f) };
        });
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

/// A register-only, void setter implemented by the renderer's device.
/// The driver only calls this with the original D3D override enabled.
pub struct RegisterSetter {
    pub address: u32,
    pub name: &'static str,
    /// Addresses on pages whose guest register writes may be folded until
    /// the next observable guest boundary. An empty slice writes through.
    pub cache_pages: &'static [u32],
    pub apply: Box<dyn Fn(&mut dyn GuestMemory, u32, u32) -> bool + Send + Sync>,
}

struct Running {
    cx: Cx,
    fut: EntryFuture,
    entry: Regs,
    render: bool,
    setter: Option<RegisterSetter>,
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
        xmm0: u128::from_le_bytes(cpu.xmm[0]),
    }
}

fn fault(e: EntryError, at: u32) -> GuestFault {
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
    drive(cpu, ctx, Running { cx, fut, entry, render: false, setter: None }, at)
}

/// Like [`start_with`], for an owned render operation which may migrate
/// with its guest stack between the render executor and the game thread.
/// All accesses to this entry's Cx must be made by the entry's body.
pub fn start_render(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    at: u32,
    make: impl FnOnce(Cx) -> EntryFuture,
) -> Result<TransferAbi, GuestFault> {
    start_render_with(cpu, ctx, at, None, make)
}

pub(crate) fn start_render_with(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    at: u32,
    setter: Option<RegisterSetter>,
    make: impl FnOnce(Cx) -> EntryFuture,
) -> Result<TransferAbi, GuestFault> {
    let entry = regs(cpu);
    let cx = Cx::new(RenderMem, entry);
    let rt = Arc::clone(ctx.runtime());
    let generation = setter_generation(&setter, ctx);
    let mut mem = rt.memory();
    let fut = with_render_memory(&mut *mem, || make(cx.clone()));
    let mut r = Running { cx, fut, entry, render: true, setter };
    let step = poll_render(&mut r, cpu, &rt, &mut mem, generation);
    transfer(cpu, ctx, &mut mem, r, at, step)
}

/// Resolve the installed override before taking the memory lock. A pipeline
/// hand-back changes runtimes, so each new guest return checks its executor.
fn setter_generation(setter: &Option<RegisterSetter>, ctx: &ExecCtx) -> Option<u64> {
    let setter = setter.as_ref()?;
    if ctx.profile().is_some() { return None; }
    let rt = ctx.runtime();
    rt.read_dispatch(|d| {
        (d.override_name(setter.address) == Some(setter.name)
            && d.override_handler(setter.address).is_some()).then(|| rt.dispatch_generation())
    })
}

/// Keep immediate device writes inside the current poll's memory guard.
/// Any callback, changed override or exceptional device state takes the
/// ordinary call path with its full continuation and argument registers.
fn poll_render(
    r: &mut Running, cpu: &mut CpuAbi, rt: &Runtime, mem: &mut Guest, generation: Option<u64>,
) -> Step {
    let pages = r.setter.as_ref().map_or(&[][..], |s| s.cache_pages);
    if pages.is_empty() || mem.has_trap() {
        return poll_memory(r, cpu, rt, mem, generation);
    }
    // No outstanding bytes or host pointers cross transfer(). Nested polls
    // on the same thread can take another buffer while this one is in use.
    let mut cache = WRITEBACK.take().unwrap_or_default();
    let step = cache.with(mem, pages, |view| poll_memory(r, cpu, rt, view, generation));
    WRITEBACK.set(Some(cache));
    step
}

fn poll_memory<M: GuestMemory>(
    r: &mut Running, cpu: &mut CpuAbi, rt: &Runtime, mem: &mut M, generation: Option<u64>,
) -> Step {
    loop {
        let step = with_render_memory(mem, || entry::poll(&mut r.fut, &r.cx));
        let (Step::Call(c), Some(setter)) = (&step, &r.setter) else { return step };
        if c.target != setter.address || !c.args.is_empty()
            || c.eax.is_some() || c.ebx.is_some() || c.esi.is_some() || c.edi.is_some()
            || c.xmm0.is_some() || c.xmm1.is_some() || c.xmm2.is_some()
        {
            return step;
        }
        let (Some(ecx), Some(edx)) = (c.ecx, c.edx) else { return step };
        if generation != Some(rt.dispatch_generation()) || !(setter.apply)(mem, ecx, edx) {
            return step;
        }
        // The void HLE leaves its input registers intact. The next guest
        // call sees them even when it supplies no ECX or EDX of its own.
        cpu.ecx = ecx;
        cpu.edx = edx;
        entry::resume(&r.cx, regs(cpu));
    }
}

fn drive(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut r: Running,
    at: u32,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    if r.render {
        let generation = setter_generation(&r.setter, ctx);
        let mut mem = rt.memory();
        let step = poll_render(&mut r, cpu, &rt, &mut mem, generation);
        transfer(cpu, ctx, &mut mem, r, at, step)
    } else {
        // Regular async entries can take their own memory guard.
        let step = entry::poll(&mut r.fut, &r.cx);
        transfer(cpu, ctx, &mut rt.memory(), r, at, step)
    }
}

fn transfer(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mem: &mut Guest,
    r: Running,
    at: u32,
    step: Step,
) -> Result<TransferAbi, GuestFault> {
    match step {
        Step::Done(Ok(ret)) => {
            let e = r.entry;
            (cpu.eax, cpu.ebx, cpu.esi, cpu.edi, cpu.ebp, cpu.esp) =
                (ret.eax, e.ebx, ret.esi.unwrap_or(e.esi), e.edi, e.ebp, e.esp);
            mem.ret(cpu, u32::from(ret.pop))
        }
        Step::Done(Err(e)) => {
            eprintln!("feature: entry at {at:#010x}: {e}");
            Err(fault(e, at))
        }
        Step::Call(c) => {
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
            if let Some(v) = c.xmm0 {
                cpu.xmm[0] = v.to_le_bytes();
            }
            if let Some(v) = c.xmm1 {
                cpu.xmm[1] = v.to_le_bytes();
            }
            if let Some(v) = c.xmm2 {
                cpu.xmm[2] = v.to_le_bytes();
            }
            let origin = if r.render { RENDER_ENTRY } else { ASYNC_ENTRY };
            ctx.call_guest_named(cpu, mem, c.target, origin, move |cpu, ctx| {
                entry::resume(&r.cx, regs(cpu));
                // Whatever the callee popped, the stack is ours again.
                cpu.esp = sp;
                drive(cpu, ctx, r, at)
            })
        }
    }
}
