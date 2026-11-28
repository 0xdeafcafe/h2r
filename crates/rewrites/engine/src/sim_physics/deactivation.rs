//! The deactivation test `0x00316b30` as a take-over arm (`parity`,
//! docs/engine/havok-integrate.md). The model is
//! `sim_havok::deactivation`: [`capture`] runs it on a journal and holds
//! the stores and the registers the entry leaves.
//!
//! Registers: eax is the out pointer, which the one caller `0x0030fef0`
//! reads at `0x0030ffee`; ebx, ebp, esi and edi are as on entry, and esp
//! is past the return and the two arguments. ecx, edx and the arithmetic
//! flags are scratch: the caller writes ecx (`0x0030fff4` or
//! `0x0030fffd`) and edx (`0x0030ffd9`) before reading them, and
//! `cmp byte [eax],0` sets the flags. The step argument's slot, which the
//! entry uses for the second timer and the deactivators' out bytes, is
//! dead at the return and not stored. The x87 stack is empty on both
//! sides; the x87 condition codes, the sticky exception flags and the XMM
//! registers are not modelled, as for the island integrate.
use super::alloc::View;
use super::answer::{Answer, Reg, callee_saved};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_havok::deactivation::{self as model, Fallback};
use sim_havok::jacobian::Fpu;
use sim_havok::mem::Journal;

/// The stack bytes the entry's `ret 8` pops, besides the return address.
pub const ARGS: u32 = 8;

pub struct Prediction {
    answer: Answer,
    /// The out byte: the island deactivates.
    pub deactivates: bool,
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
    let arg = |n: u32| g.u32(cpu.esp.wrapping_add(4 + 4 * n)).map_err(|_| Fallback::Memory);
    let (out, step) = (arg(0)?, arg(1)?);
    let end_sp = cpu.esp.checked_add(4 + ARGS).ok_or(Fallback::Memory)?;
    let view = View(g);
    let mut j = Journal::new(&view);
    let fpu = Fpu { mxcsr: cpu.mxcsr, control: cpu.x87.control };
    let deactivates = model::test(&mut j, cpu.ecx, Some(out), step, fpu)?;
    let mut regs = vec![(Reg::Eax, out)];
    regs.extend(callee_saved(cpu));
    let answer = Answer::new(j.finish(), regs, end_sp, g).ok_or(Fallback::Memory)?;
    Ok(Prediction { answer, deactivates })
}
