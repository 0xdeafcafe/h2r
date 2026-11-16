//! `render-loop`: stage 7a's Rust orchestrators placed as implementations
//! (docs/render-loop.md). Each is switched by `impl.render.<name>`, falling
//! back to `impl.render`; all are off by default.
//!
//! Built: `impl.render.views`, the Rust `render_views`
//! ([`engine::render_loop::Machine`]). The driver runs the port to each
//! guest call, makes it with `call_guest` (the callee returns into a
//! continuation that resumes the port), and returns to the caller when the
//! port does. The only guest-visible difference from the original is the
//! return address the callees see (the continuation token), as for every
//! chain with an after hook.

use std::sync::Arc;

use engine::points;
use engine::render_loop::{Machine, Regs, Step};
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "render-loop";

pub struct RenderLoop;

impl Feature for RenderLoop {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Rust render orchestrators (stage 7a)").doc("docs/render-loop.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if b.switches().implementation("render.views", false) {
            b.raw()
                .implement(&points::RENDER_VIEWS, "render_views", |cpu, ctx, _| {
                    views(cpu, ctx)
                });
        }
        Ok(())
    }
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

fn store(cpu: &mut CpuAbi, r: &Regs) {
    cpu.eax = r.eax;
    cpu.ecx = r.ecx;
    cpu.edx = r.edx;
    cpu.ebx = r.ebx;
    cpu.esp = r.esp;
    cpu.ebp = r.ebp;
    cpu.esi = r.esi;
    cpu.edi = r.edi;
    cpu.xmm[0] = r.xmm0.to_le_bytes();
}

fn fault(f: engine::memory::MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { Access::Write } else { Access::Read },
        reason: FaultReason::Unmapped,
    }
}

/// The `render_views` implementation: entered at the function's first
/// instruction, as the original would be.
fn views(cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let mut r = regs(cpu);
    let (m, step) = Machine::start(&mut r, &mut *mem).map_err(fault)?;
    store(cpu, &r);
    follow(m, step, cpu, ctx, &mut mem)
}

/// Makes the call `step` asks for, or returns.
fn follow(
    mut m: Machine,
    step: Step,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mem: &mut Guest,
) -> Result<TransferAbi, GuestFault> {
    match step {
        Step::Return { to } => Ok(TransferAbi::ret(to)),
        Step::Call { target, .. } => ctx.call_guest(cpu, mem, target, move |cpu, ctx| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let mut r = regs(cpu);
            let step = m.resume(&mut r, &mut *mem).map_err(fault)?;
            store(cpu, &r);
            follow(m, step, cpu, ctx, &mut mem)
        }),
    }
}
