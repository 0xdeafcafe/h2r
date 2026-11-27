//! The convex agent's process call `0x002eeb40` as a take-over arm
//! (`parity`, docs/engine/havok-convex.md). The model is
//! `sim_havok::convex::process`: [`capture`] runs the separated-pair path
//! on a journal and holds the bound's store and the registers the call
//! leaves; every other path declines to the guest.
//!
//! Registers: eax is the agent's point-count address (`0x002eee48`), ecx
//! the monitor head (`0x002eee58`), edx the agent's manager word
//! (`0x002eee44`); ebx, ebp, esi and edi are restored, and esp is past the
//! return and the four arguments. The arithmetic flags are scratch: the
//! callers (the pair pass's `vt[+0x10]` calls, `0x002db5e0`) write before
//! they test. The frame is below esp at the return. The x87 stack is empty
//! on both sides; the x87 condition codes, the sticky exception flags and
//! the XMM registers are not modelled, as for the island integrate.
use super::alloc::View;
use super::answer::{Answer, Reg, callee_saved};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_havok::convex::{self as model, Args, Fallback};
use sim_havok::jacobian::Fpu;
use sim_havok::mem::Journal;

/// The stack bytes the call's `ret 0x10` pops, besides the return address.
pub const ARGS: u32 = 0x10;

/// The monitor stream's head, which the call leaves in ecx.
const MONITOR_HEAD: u32 = 0x0048_0154;

pub struct Prediction {
    answer: Answer,
    /// The bound stored.
    pub bound: u32,
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
    let a = Args { agent: cpu.ecx, a: arg(0)?, b: arg(1)? };
    let end_sp = cpu.esp.checked_add(4 + ARGS).ok_or(Fallback::Memory)?;
    let head = g.u32(MONITOR_HEAD).map_err(|_| Fallback::Memory)?;
    let view = View(g);
    let mut j = Journal::new(&view);
    let fpu = Fpu { mxcsr: cpu.mxcsr, control: cpu.x87.control };
    let separated = model::process(&mut j, &a, fpu)?;
    let mut regs = vec![
        (Reg::Eax, a.agent.wrapping_add(model::field::POINTS)),
        (Reg::Ecx, head),
        (Reg::Edx, separated.manager),
    ];
    regs.extend(callee_saved(cpu));
    let answer = Answer::new(j.finish(), regs, end_sp, g).ok_or(Fallback::Memory)?;
    Ok(Prediction { answer, bound: separated.bound })
}
