//! The closest-feature query `0x003048a0` as a take-over arm (`parity`,
//! docs/engine/havok-convex.md, "The query arm"). The model is
//! `sim_havok::closest::query`: [`capture`] runs the whole query on a
//! journal, the shapes' virtuals, the triangle case and its penetration
//! call in Rust, and holds every store outside the dead stack with the
//! registers the call leaves.
//!
//! Registers: eax, ecx and edx are the query's (`gjk::Query`); ebx, ebp,
//! esi and edi are restored, and esp is past the return and the five
//! arguments. The six callers (the convex agents at `0x002ee47a` to
//! `0x002ef02a`) test eax and write the arithmetic flags before they read
//! them. The frames of the query and its callees are below esp at the
//! return, so their stores are left out ([`DEAD_STACK`]). The x87 stack
//! is empty on both sides; the x87 condition codes, the sticky exception
//! flags and the XMM registers are not modelled, as for the convex arm.
use super::alloc::View;
use super::answer::{Answer, Reg, callee_saved};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_havok::closest::{self as model, Fallback};
use sim_havok::gjk::{Args, Query};
use sim_havok::jacobian::Fpu;
use sim_havok::mem::Journal;

/// The stack bytes the call's `ret 0x14` pops, besides the return address.
pub const ARGS: u32 = 0x14;

/// The bytes below the entry's esp the query and its callees use as
/// frames: the query's `0x80`, the triangle case's `0x140`, the
/// penetration call's `0x70` and the solver's `0x3004`, with their pushes,
/// fit with room.
pub const DEAD_STACK: u32 = 0x1_0000;

pub struct Prediction {
    answer: Answer,
    pub query: Query,
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
    let args = Args { a: arg(0)?, b: arg(1)?, transform: arg(2)?, cache: arg(3)?, out: arg(4)? };
    let end_sp = cpu.esp.checked_add(4 + ARGS).ok_or(Fallback::Memory)?;
    let dead = cpu.esp.checked_sub(DEAD_STACK).ok_or(Fallback::Memory)?;
    let view = View(g);
    let mut j = Journal::new(&view);
    let fpu = Fpu { mxcsr: cpu.mxcsr, control: cpu.x87.control };
    let query = model::query(&mut j, cpu.ecx, cpu.esp, args, fpu)?;
    let mut writes = j.finish();
    writes.retain(|&at, _| !(dead..cpu.esp).contains(&at));
    let mut regs = vec![(Reg::Eax, query.eax), (Reg::Ecx, query.ecx), (Reg::Edx, query.edx)];
    regs.extend(callee_saved(cpu));
    let answer = Answer::new(writes, regs, end_sp, g).ok_or(Fallback::Memory)?;
    Ok(Prediction { answer, query })
}
