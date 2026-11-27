//! The memory system's four virtuals (docs/engine/havok-step.md,
//! "Allocator"), which Halo's world builders call as well as the step.
//! The model is `sim_havok::mem`; this adapter reads the words it asks
//! for, runs it on a journal, and stores the journal when the model
//! covers the route. A route it does not cover (the overflow object, an
//! unexpected vtable) is `supported == false` and the guest runs.
//!
//! Registers: ebx, esi, edi, ebp and esp are as retail leaves them. eax is
//! the pointer (allocations); frees return nothing a caller reads. ecx, edx
//! and the arithmetic flags are caller-saved scratch in retail and are
//! left undefined here.
use super::*;
use sim_havok::mem::{self, Journal, System, Words};
use std::collections::BTreeMap;

pub(super) struct View<'a>(pub &'a Guest);

impl Words for View<'_> {
    fn get(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// An entry's arguments, from the frame: the pool in ecx, then stack words.
struct Call {
    pool: u32,
    args: [u32; 3],
}

impl Call {
    fn read(c: &CpuAbi, g: &Guest) -> Option<Self> {
        let at = |n: u32| g.u32(c.esp.wrapping_add(4 + 4 * n)).ok();
        Some(Self { pool: c.ecx, args: [at(0)?, at(1)?, at(2)?] })
    }
}

/// What the entry computes: the stores, `eax`, and the stack cleanup.
struct Outcome {
    writes: BTreeMap<u32, u32>,
    eax: u32,
}

fn model(point: u32, call: &Call, g: &Guest) -> Option<Outcome> {
    let view = View(g);
    let mut j = Journal::new(&view);
    let [a0, a1, _] = call.args;
    let eax = match point {
        // allocate(size, tag)
        0x0022_c300 => mem::directory_allocate(&mut j, call.pool, a0)?,
        // free(block, size, tag)
        0x0022_c320 => mem::directory_free(&mut j, call.pool, a0)?,
        // lifo_allocate(request)
        0x002d_7020 => {
            let sys = System::read(&j)?;
            mem::lifo_allocate(&sys, &mut j, call.pool, a0)?
        }
        // lifo_free(block, request)
        0x002d_7090 => {
            let sys = System::read(&j)?;
            mem::lifo_free(&sys, &mut j, call.pool, a1)?;
            0
        }
        _ => return None,
    };
    Some(Outcome { writes: j.finish(), eax })
}

fn cleanup(point: u32) -> u32 {
    match point {
        0x0022_c300 | 0x002d_7090 => 8,
        0x0022_c320 => 12,
        _ => 4,
    }
}

/// Whether the model covers this call: it runs the entry on a journal and
/// discards it.
pub(super) fn supported(point: u32, c: &CpuAbi, g: &Guest) -> bool {
    let Some(call) = Call::read(c, g) else { return false };
    let Some(out) = model(point, &call, g) else { return false };
    // The stores must be writable before any of them happens.
    out.writes.keys().all(|a| g.check(*a, 4, runtime::guest::Access::Write, 1).is_ok())
}

pub(super) fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    let call = Call::read(c, g).ok_or(GuestFault {
        addr: c.esp, len: 4, access: runtime::guest::Access::Read,
        reason: runtime::guest::FaultReason::Unmapped,
    })?;
    let out = model(pc, &call, g).expect("supported() ran the model first");
    for (addr, value) in out.writes {
        g.set_u32(addr, value)?;
    }
    c.eax = out.eax;
    c.eflags_defined &= !ARITH;
    g.ret(c, cleanup(pc))
}
