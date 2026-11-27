//! The island integrate `0x00316a20` as a take-over arm (`parity`,
//! docs/engine/havok-integrate.md, "The island integrate"). The model is
//! `sim_havok::island_integrate`: [`capture`] runs it on a journal and
//! holds the stores and the registers the entry leaves; [`Prediction::apply`]
//! answers the call with them, [`Prediction::matches`] compares them with
//! the guest's answer at its return.
//!
//! Registers: ecx (the entry pushes and pops it), ebx, ebp, esi and edi
//! are as on entry, and esp is past the return and the one argument. eax,
//! edx and the arithmetic flags are scratch: the only caller, `0x0030fef0`,
//! stores `I+0x30` and loads eax from `W+0xa0` at `0x0031000f` before any
//! read, and edx and ecx are written before they are read on every path
//! out of it. The x87 stack is empty on both sides; the condition codes
//! and the sticky exception flags of the x87 status word and the MXCSR are
//! not modelled (every exception is masked, or the call declines), nor are
//! the XMM registers, which the entry's callees leave as scratch: the
//! island solve's entry treats them alike.
use super::alloc::View;
use super::answer::{Answer, Reg, callee_saved};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_havok::island_integrate::{self as integrate, Fallback};
use sim_havok::jacobian::Fpu;
use sim_havok::mem::Journal;

/// The stack bytes the entry's `ret 4` pops, besides the return address.
pub const ARGS: u32 = 4;

/// The answer: every word the entry stores outside its dead stack and the
/// solver buffer's scratch, and the registers it leaves (ecx, ebx, ebp,
/// esi, edi).
pub struct Prediction {
    answer: Answer,
    /// The island went through the solve.
    pub solved: bool,
}

impl Prediction {
    /// At the guest's return: the registers and every predicted word.
    pub fn matches(&self, cpu: &CpuAbi, g: &Guest) -> bool {
        self.answer.matches(cpu, g)
    }

    /// Answer the call ([`Answer::apply`]).
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
    // The return address too, which the arm reads when it answers.
    g.u32(cpu.esp).map_err(|_| Fallback::Memory)?;
    let input = g.u32(cpu.esp.wrapping_add(4)).map_err(|_| Fallback::Memory)?;
    let end_sp = cpu.esp.checked_add(4 + ARGS).ok_or(Fallback::Memory)?;
    let view = View(g);
    let mut j = Journal::new(&view);
    let fpu = Fpu { mxcsr: cpu.mxcsr, control: cpu.x87.control };
    let outcome = integrate::integrate(&mut j, cpu.ecx, input, fpu)?;
    let mut regs = vec![(Reg::Ecx, cpu.ecx)];
    regs.extend(callee_saved(cpu));
    let answer = Answer::new(j.finish(), regs, end_sp, g).ok_or(Fallback::Memory)?;
    Ok(Prediction { answer, solved: outcome.solved })
}
