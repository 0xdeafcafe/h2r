//! The shader draw's render-state list `0x0001a170`
//! ([`engine::render_loop::states`]) as the native world's BSP walk runs
//! it: redundant `SetRenderState_Simple` calls skipped against the D3D
//! shadow `0x00407498`.
//!
//! Stage 7a also placed the Rust `render_views`, the layer draw and its
//! entry loop, `render_game_view`, `render_scene_passes` and the state list
//! itself as `impl.render.*` implementations. None saved measurable time
//! and each cost a hooked call, so they were removed
//! (docs/graphics/render-loop.md).

use engine::render_loop::states::{Mode, RenderStates};
use engine::render_loop::{Regs, Step};
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

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
        xmm1: u128::from_le_bytes(cpu.xmm[1]),
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
    cpu.xmm[1] = r.xmm1.to_le_bytes();
}

fn fault(f: guest_layout::memory::MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { Access::Write } else { Access::Read },
        reason: FaultReason::Unmapped,
    }
}

/// The BSP shader's state list. A state it must set through a guest call
/// is a gap.
pub(crate) fn world_states(cpu: &mut CpuAbi, mem: &mut Guest) -> Result<TransferAbi, GuestFault> {
    let mut r = regs(cpu);
    let (states, step) = RenderStates::start(&mut r, mem, cpu.x87.control, Mode::SkipRedundant)
        .map_err(fault)?;
    timeline::native::Stage::World.count(timeline::native::Metric::StateCallsSkipped,
        u64::from(states.counts.skipped));
    store(cpu, &r);
    match step {
        Step::Return { to } => Ok(TransferAbi::ret(to)),
        Step::Call { target, site, .. } => {
            unimplemented!("render: guest body {target:#x} render-state list call (site {site:#x})")
        }
    }
}

