//! The integrate phase `0x0030fef0` as a take-over arm (`parity`,
//! docs/engine/havok-integrate.md, "The integrate phase"). The model is
//! `sim_havok::integrate_phase`, which runs the deactivation test and the
//! island integrate of every island in the phase's order on one journal.
//!
//! Registers: ebx, ebp, esi and edi are as on entry, and esp is past the
//! return and the one argument. On the path the model covers retail
//! leaves eax and ecx as the monitor words `[0x00480158]` and
//! `[0x00480154]` (`0x0031017c`) and edx as `0xffffffff` (the empty step
//! listener count less one, `0x0031872b`); the answer sets all three. The
//! arithmetic flags are scratch: the one caller's resume `0x00311116`
//! pops and returns. The out byte of each deactivation test lands in the
//! phase's own argument slot, dead at the return, and is not stored. The
//! x87 and XMM state is treated as for the island integrate.
use super::alloc::View;
use super::answer::{Answer, Reg, callee_saved};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_havok::integrate_phase::{self as model, Fallback, Outcome};
use sim_havok::jacobian::Fpu;
use sim_havok::mem::Journal;

/// The stack bytes the entry's `ret 4` pops, besides the return address.
pub const ARGS: u32 = 4;

const MONITOR_HEAD: u32 = 0x0048_0154;
const MONITOR_END: u32 = 0x0048_0158;

pub struct Prediction {
    answer: Answer,
    pub outcome: Outcome,
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
    if !super::world::monitor_off(g) {
        return Err(Fallback::Monitor);
    }
    if !det::x87_env(cpu.x87.control, cpu.x87.status, cpu.x87.tag, 8) {
        return Err(Fallback::Float);
    }
    g.u32(cpu.esp).map_err(|_| Fallback::Memory)?;
    let step = g.u32(cpu.esp.wrapping_add(4)).map_err(|_| Fallback::Memory)?;
    let end_sp = cpu.esp.checked_add(4 + ARGS).ok_or(Fallback::Memory)?;
    let monitor = |at: u32| g.u32(at).map_err(|_| Fallback::Memory);
    let (head, end) = (monitor(MONITOR_HEAD)?, monitor(MONITOR_END)?);
    let view = View(g);
    let mut j = Journal::new(&view);
    let fpu = Fpu { mxcsr: cpu.mxcsr, control: cpu.x87.control };
    let outcome = model::phase(&mut j, cpu.ecx, step, fpu)?;
    let mut regs = vec![(Reg::Eax, end), (Reg::Ecx, head), (Reg::Edx, u32::MAX)];
    regs.extend(callee_saved(cpu));
    let answer = Answer::new(j.finish(), regs, end_sp, g).ok_or(Fallback::Memory)?;
    Ok(Prediction { answer, outcome })
}
