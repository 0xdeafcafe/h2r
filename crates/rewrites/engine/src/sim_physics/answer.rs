//! A take-over arm's answer for one call (`parity`): the words a journal
//! computed, the registers the entry leaves and the stack pointer past its
//! frame. [`Answer::apply`] answers the call with them; [`Answer::matches`]
//! compares them with the guest's answer at its return.
use super::physical_span;
use runtime::{CpuAbi, Guest, guest::Access};
use std::collections::BTreeMap;

/// The integer registers an answer can predict, in [`Answer::regs`] order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg {
    Eax,
    Ecx,
    Edx,
    Ebx,
    Ebp,
    Esi,
    Edi,
}

fn reg(cpu: &mut CpuAbi, r: Reg) -> &mut u32 {
    match r {
        Reg::Eax => &mut cpu.eax,
        Reg::Ecx => &mut cpu.ecx,
        Reg::Edx => &mut cpu.edx,
        Reg::Ebx => &mut cpu.ebx,
        Reg::Ebp => &mut cpu.ebp,
        Reg::Esi => &mut cpu.esi,
        Reg::Edi => &mut cpu.edi,
    }
}

pub struct Answer {
    writes: BTreeMap<u32, u32>,
    /// The registers the entry leaves that a caller may read; the others
    /// are scratch, neither compared nor set.
    regs: Vec<(Reg, u32)>,
    end_sp: u32,
}

impl Answer {
    /// The answer, or `None` when a store is not plain writable memory
    /// inside one mapping or two stores share physical bytes, whose last
    /// value would then depend on an order the journal does not keep.
    pub fn new(writes: BTreeMap<u32, u32>, regs: Vec<(Reg, u32)>, end_sp: u32, g: &Guest) -> Option<Self> {
        writable(&writes, g).then_some(Self { writes, regs, end_sp })
    }

    /// At the guest's return: the registers and every predicted word.
    pub fn matches(&self, cpu: &CpuAbi, g: &Guest) -> bool {
        let mut cpu = *cpu;
        cpu.esp == self.end_sp
            && self.regs.iter().all(|&(r, v)| *reg(&mut cpu, r) == v)
            && self.writes.iter().all(|(&at, &v)| g.u32(at).is_ok_and(|w| w == v))
    }

    /// Answer the call: the stores, the registers, then the frame popped.
    /// The return PC is the dispatcher's. Refused, with nothing written,
    /// when the stores are no longer writable.
    pub fn apply(&self, cpu: &mut CpuAbi, g: &mut Guest) -> bool {
        if !writable(&self.writes, g) {
            return false;
        }
        for (&at, &v) in &self.writes {
            if g.set_u32(at, v).is_err() {
                // Checked writable above; unreachable short of a racing remap.
                return false;
            }
        }
        for &(r, v) in &self.regs {
            *reg(cpu, r) = v;
        }
        cpu.esp = self.end_sp;
        cpu.eflags_defined &= !super::ARITH;
        true
    }
}

fn writable(writes: &BTreeMap<u32, u32>, g: &Guest) -> bool {
    let mut physical = Vec::with_capacity(writes.len());
    for &at in writes.keys() {
        if g.check(at, 4, Access::Write, 1).is_err() {
            return false;
        }
        let Some(span) = physical_span(g, at, 4) else { return false };
        physical.push(span.start);
    }
    physical.sort_unstable();
    // Word-aligned virtual addresses map to word-aligned physical ones, so
    // two stores share bytes exactly when they share a start.
    physical.windows(2).all(|w| w[0] != w[1])
}

/// ebx, ebp, esi and edi as on entry: the callee-saved set.
pub fn callee_saved(cpu: &CpuAbi) -> [(Reg, u32); 4] {
    [(Reg::Ebx, cpu.ebx), (Reg::Ebp, cpu.ebp), (Reg::Esi, cpu.esi), (Reg::Edi, cpu.edi)]
}
