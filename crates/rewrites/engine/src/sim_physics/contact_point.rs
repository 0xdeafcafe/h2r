//! The add-point call `0x00302c70` as a take-over arm (`parity`,
//! docs/engine/havok-convex.md, "The add-point arm"). The model is
//! `sim_havok::contact_point::answer`: [`capture`] runs the call on a
//! journal when the new point moves the slot it matches, which makes no
//! call out, and holds the slot's stores with the registers the call
//! leaves; a point that needs the manager's add declines (`commit`).
//!
//! Registers: eax is the slot, ecx and edx the point's distance and weight
//! words (`0x00302d5d`, `0x00302d63`); ebx, ebp, esi and edi are restored,
//! and esp is past the return only (cdecl). The two callers (`0x002eede4`,
//! `0x002fa965`) write eax, ecx, edx and the arithmetic flags before they
//! read them. The x87 stack is empty on both sides; the x87 condition
//! codes, the sticky exception flags and the XMM registers are not
//! modelled, as for the convex arm.
use super::alloc::View;
use super::answer::{Answer, Reg, callee_saved};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_havok::contact_point::{self as model, Fallback, Moved};
use sim_havok::jacobian::Fpu;
use sim_havok::manifold::PointArgs;
use sim_havok::mem::Journal;

/// The bytes below the entry's esp the call uses as its frame.
const FRAME: u32 = 0x1000;

pub struct Prediction {
    answer: Answer,
    pub moved: Moved,
}

impl Prediction {
    pub fn matches(&self, cpu: &CpuAbi, g: &Guest) -> bool {
        self.answer.matches(cpu, g)
    }

    pub fn apply(&self, cpu: &mut CpuAbi, g: &mut Guest) -> bool {
        self.answer.apply(cpu, g)
    }
}

/// The model's answer for the call at its entry, or the reason it declines.
pub fn capture(cpu: &CpuAbi, g: &Guest) -> Result<Prediction, Fallback> {
    if !det::x87_env(cpu.x87.control, cpu.x87.status, cpu.x87.tag, 8) {
        return Err(Fallback::Float);
    }
    g.u32(cpu.esp).map_err(|_| Fallback::Memory)?;
    let arg = |n: u32| g.u32(cpu.esp.wrapping_add(4 + 4 * n)).map_err(|_| Fallback::Memory);
    let args = PointArgs {
        a: arg(0)?,
        b: arg(1)?,
        input: arg(2)?,
        point: arg(3)?,
        manager: arg(4)?,
        slots: arg(5)?,
        count: arg(6)?,
        esp: cpu.esp,
    };
    let end_sp = cpu.esp.checked_add(4).ok_or(Fallback::Memory)?;
    let dead = cpu.esp.checked_sub(FRAME).ok_or(Fallback::Memory)?;
    let view = View(g);
    let mut j = Journal::new(&view);
    let fpu = Fpu { mxcsr: cpu.mxcsr, control: cpu.x87.control };
    let moved = model::answer(&mut j, &args, fpu)?;
    let mut writes = j.finish();
    writes.retain(|&at, _| !(dead..cpu.esp).contains(&at));
    let mut regs = vec![(Reg::Eax, moved.slot), (Reg::Ecx, moved.ecx), (Reg::Edx, moved.edx)];
    regs.extend(callee_saved(cpu));
    let answer = Answer::new(writes, regs, end_sp, g).ok_or(Fallback::Memory)?;
    Ok(Prediction { answer, moved })
}
