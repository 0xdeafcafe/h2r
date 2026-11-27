//! The island integrate `0x00316a20` as a take-over arm (`parity`,
//! docs/engine/havok-step.md, "The island integrate"). The model is
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
use super::physical_span;
use runtime::{CpuAbi, Guest, guest::Access};
use sim_core::det;
use sim_havok::island_integrate::{self as integrate, Fallback};
use sim_havok::jacobian::Fpu;
use sim_havok::mem::Journal;
use std::collections::BTreeMap;

/// The stack bytes the entry's `ret 4` pops, besides the return address.
pub const ARGS: u32 = 4;

/// The answer: every word the entry stores outside its dead stack and the
/// solver buffer's scratch, and the registers it leaves.
pub struct Prediction {
    writes: BTreeMap<u32, u32>,
    /// ecx, ebx, ebp, esi, edi.
    saved: [u32; 5],
    end_sp: u32,
    /// The island went through the solve.
    pub solved: bool,
}

impl Prediction {
    /// At the guest's return: the registers and every predicted word.
    pub fn matches(&self, cpu: &CpuAbi, g: &Guest) -> bool {
        cpu.esp == self.end_sp
            && [cpu.ecx, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi] == self.saved
            && self.writes.iter().all(|(&at, &v)| g.u32(at).is_ok_and(|w| w == v))
    }

    /// Answer the call: the stores, then the frame popped. The return PC is
    /// the dispatcher's. Refused, with nothing written, when a store is not
    /// plain writable memory inside one mapping or two stores share
    /// physical bytes, whose last value would then depend on an order the
    /// journal does not keep.
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
    let writes = j.finish();
    if !writable(&writes, g) {
        return Err(Fallback::Memory);
    }
    Ok(Prediction {
        writes,
        saved: [cpu.ecx, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi],
        end_sp,
        solved: outcome.solved,
    })
}
