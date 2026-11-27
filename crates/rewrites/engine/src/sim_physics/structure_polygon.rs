//! The structure polygon builder `0x001edd00` as a take-over arm
//! (`parity`, docs/engine/havok-structure.md). The model is
//! `sim_havok::structure::build`: [`capture`] runs it on a journal and
//! holds the shape's words and the registers the builder leaves.
//!
//! Registers: eax is the shape (ecx), ecx and edi are as on entry, edx is
//! the tail argument, ebx, ebp and esi are restored, and esp is past the
//! return and the five arguments. The arithmetic flags are scratch: every
//! caller (`0x001ee4f3`, `0x001ee519`, `0x001ef9a0`) pops or returns. The
//! first two argument slots, which the builder uses for the surface record
//! and the first edge, and its own frame are dead at the return. The XMM
//! registers it uses are not modelled, as for the island integrate.
use super::alloc::View;
use super::answer::{Answer, Reg};
use runtime::{CpuAbi, Guest};
use sim_havok::mem::Journal;
use sim_havok::structure::{self as model, Args, Fallback};

/// The stack bytes the builder's `ret 0x14` pops, besides the return address.
pub const ARGS: u32 = 0x14;

pub struct Prediction {
    answer: Answer,
    /// The ring's vertex count.
    pub vertices: u32,
}

impl Prediction {
    pub fn matches(&self, cpu: &CpuAbi, g: &Guest) -> bool {
        self.answer.matches(cpu, g)
    }

    pub fn apply(&self, cpu: &mut CpuAbi, g: &mut Guest) -> bool {
        self.answer.apply(cpu, g)
    }
}

/// The builder's arguments at its entry.
pub fn args(cpu: &CpuAbi, g: &Guest) -> Result<Args, Fallback> {
    let arg = |n: u32| g.u32(cpu.esp.wrapping_add(4 + 4 * n)).map_err(|_| Fallback::Memory);
    Ok(Args {
        shape: cpu.ecx,
        transform: cpu.edi,
        bsp: arg(0)?,
        flag: arg(1)? as u8,
        owner: arg(2)?,
        surface: arg(3)?,
        tail: arg(4)?,
    })
}

/// The model's answer for the call at its entry, or the reason it declines.
pub fn capture(cpu: &CpuAbi, g: &Guest) -> Result<Prediction, Fallback> {
    g.u32(cpu.esp).map_err(|_| Fallback::Memory)?;
    let a = args(cpu, g)?;
    let end_sp = cpu.esp.checked_add(4 + ARGS).ok_or(Fallback::Memory)?;
    let view = View(g);
    let mut j = Journal::new(&view);
    let vertices = model::build(&mut j, &a, cpu.mxcsr)?;
    let regs = vec![
        (Reg::Eax, a.shape),
        (Reg::Ecx, a.shape),
        (Reg::Edx, a.tail),
        (Reg::Ebx, cpu.ebx),
        (Reg::Ebp, cpu.ebp),
        (Reg::Esi, cpu.esi),
        (Reg::Edi, cpu.edi),
    ];
    let answer = Answer::new(j.finish(), regs, end_sp, g).ok_or(Fallback::Memory)?;
    Ok(Prediction { answer, vertices })
}
