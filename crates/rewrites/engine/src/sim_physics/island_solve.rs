//! The island solve `0x0031c100` (docs/engine/havok-step.md, "The island
//! solve"). The model is `sim_havok::island`: [`supported`] runs it on a
//! journal and counts the reason when it declines, so the lifted entry
//! runs for that island; [`run`] runs it again and stores the journal.
//!
//! Registers: ebx, esi, edi, ebp and esp are as retail leaves them (cdecl,
//! the caller pops the nine arguments). eax is the entry's return. ecx,
//! edx and the arithmetic flags are scratch and left undefined. The x87
//! stack is empty on both sides; the sticky exception flags of the x87
//! status word and the MXCSR are not modelled (every exception is masked,
//! or the island declines).
use super::*;
use super::alloc::View;
use sim_havok::island_solve::{self as island, Args};
use sim_havok::jacobian::Fpu;
use sim_havok::mem::Journal;
use std::collections::BTreeMap;

fn model(c: &CpuAbi, g: &Guest) -> std::result::Result<(BTreeMap<u32, u32>, u32), island::Fallback> {
    let view = View(g);
    let mut j = Journal::new(&view);
    let args = Args::read(&j, c.esp).ok_or(island::Fallback::Memory)?;
    let fpu = Fpu { mxcsr: c.mxcsr, control: c.x87.control };
    let eax = island::solve(&mut j, &args, fpu)?;
    Ok((j.finish(), eax))
}

/// Whether the model answers this island. A decline is counted.
pub(super) fn supported(c: &CpuAbi, g: &Guest) -> bool {
    let reason = match model(c, g) {
        Ok((writes, _)) => {
            if writes.keys().all(|a| g.check(*a, 4, runtime::guest::Access::Write, 1).is_ok()) {
                return true;
            }
            island::Fallback::Memory
        }
        Err(reason) => reason,
    };
    island::count(reason);
    false
}

pub(super) fn run(c: &mut CpuAbi, g: &mut Guest, _pc: u32) -> Result<TransferAbi> {
    let (writes, eax) = model(c, g).expect("supported() ran the model first");
    for (addr, value) in writes {
        g.set_u32(addr, value)?;
    }
    island::count_solved();
    c.eax = eax;
    c.eflags_defined &= !ARITH;
    g.ret(c, 0)
}
