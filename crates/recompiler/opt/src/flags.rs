//! Flag liveness, and the dead-flag pass built on it.
//!
//! A flag write is *dead* when, on every path from it, the flag is written
//! again before anything can observe it. Observers are deliberately broad:
//!
//! - a read of the flag (`ReadFlag`), or of all of EFLAGS (`ReadEflags`);
//! - anything that can leave the handler, because every exit spills EFLAGS
//!   and the defined mask to `CpuAbi` where the runtime, a guest exception
//!   handler or a resumed handler sees them: loads and stores (they may
//!   fault; not with [`Observers::IMPRECISE_FAULTS`]), `FaultIf`, `StopIf`,
//!   intrinsics, unsupported statements, atomic instructions (which also
//!   hand the flags to the runtime), and every control transfer that is not
//!   a local `goto`;
//! - a `Repeat`, whose exits restore the flags it started with;
//! - the start of a loop header, where generated code polls for
//!   preemption and may leave with the state canonical.
//!
//! A write only kills a flag when it is unconditional on the path being
//! walked: writes inside an `If` kill along that branch, writes inside a
//! `Repeat` body never do. Removing a dead write leaves the defined mask
//! exact too: the write that kills it sets or clears the same bit.
//!
//! The analysis is per function and backwards, to a fixed point over the
//! local CFG. A block entered from outside (entry, interior entry, call
//! continuation, REP restart, loop resume) gets its flags from `CpuAbi`;
//! that is a definition, not an observation, so start selectors do not
//! affect it.

use std::collections::BTreeMap;

use ir::{Control, Flag, IrBlock, IrFunction, IrInsn, Stmt, Target};

use crate::Shape;

/// What observes the flags besides reads, exits and polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observers {
    /// A load or store, which may fault and leave with the flags spilled.
    pub memory: bool,
}

impl Observers {
    /// Every fault exit is exact.
    pub const PRECISE: Observers = Observers { memory: true };
    /// Imprecise memory faults (docs/runtime/block-clock-imprecise-faults.md):
    /// a memory fault's exit keeps exact registers and EIP, not EFLAGS, so a
    /// load or store observes nothing. `FaultIf`, intrinsics, atomics and
    /// `Repeat` still do.
    pub const IMPRECISE_FAULTS: Observers = Observers { memory: false };
}

/// Every flag the IR models, as EFLAGS bits.
pub const ALL: u32 = 0x0cd5;

pub const fn bit(f: Flag) -> u32 {
    1 << f.eflags_bit()
}

/// Flags live before `stmts`, given the flags live after them. With
/// `edit`, dead top-level and `If`-nested writes are removed on the way
/// (removing a dead write does not change what is live before it).
fn stmts(list: &mut Vec<Stmt>, mut live: u32, edit: bool, removed: &mut usize, obs: Observers) -> u32 {
    let mut i = list.len();
    while i > 0 {
        i -= 1;
        match &mut list[i] {
            Stmt::WriteFlag { flag, .. } => {
                let b = bit(*flag);
                if live & b == 0 {
                    if edit {
                        list.remove(i);
                        *removed += 1;
                    }
                } else {
                    live &= !b;
                }
            }
            Stmt::WriteEflags { mask, .. } => live &= !(*mask & ALL),
            // A lazy group goes only when all of it is dead: keeping dead
            // flags in a live one costs nothing (the inputs are captured
            // anyway) and spares the backend settling the flags it would
            // otherwise leave pending from an earlier group.
            Stmt::LazyFlags(l) => {
                let m = l.formula.mask();
                if live & m == 0 {
                    if edit {
                        *removed += l.formula.flags.len();
                        list.remove(i);
                    }
                } else {
                    live &= !m;
                }
            }
            Stmt::ReadFlag { flag, .. } => live |= bit(*flag),
            Stmt::ReadEflags { .. } => live |= ALL,
            Stmt::Load { .. } | Stmt::Store { .. } => {
                if obs.memory {
                    live = ALL;
                }
            }
            Stmt::FaultIf { .. }
            | Stmt::StopIf { .. }
            | Stmt::Intrinsic { .. }
            | Stmt::Unsupported { .. }
            | Stmt::Repeat { .. } => live = ALL,
            Stmt::If { then, otherwise, .. } => {
                live = stmts(then, live, edit, removed, obs) | stmts(otherwise, live, edit, removed, obs);
            }
            Stmt::Assign { .. }
            | Stmt::ReadReg { .. }
            | Stmt::WriteReg { .. }
            | Stmt::ReadXmm { .. }
            | Stmt::WriteXmm { .. }
            | Stmt::ReadMmx { .. }
            | Stmt::WriteMmx { .. } => {}
        }
    }
    live
}

/// Flags live before `insn`, given those live after its statements.
fn insn(insn: &mut IrInsn, live_after: u32, edit: bool, removed: &mut usize, obs: Observers) -> u32 {
    if insn.atomic.is_some() {
        // The runtime performs it from the spilled state and may fault.
        return ALL;
    }
    stmts(&mut insn.stmts, live_after, edit, removed, obs)
}

/// Flags live after the last instruction's statements, from its control.
fn control(c: &Control, next_pc: u32, live_in: &dyn Fn(u32) -> u32) -> u32 {
    let side = |d: &ir::Dest| if d.tail { ALL } else { live_in(d.pc) };
    match c {
        Control::Next => live_in(next_pc),
        Control::Jump { target: Target::Direct(t), tail: false, .. } => live_in(*t),
        Control::Branch { taken, fallthrough, .. } => side(taken) | side(fallthrough),
        // Switch: an unmatched target stops; everything else leaves.
        _ => ALL,
    }
}

/// Flags live on entry to `b` (before any poll), and optionally edit it.
fn block(
    b: &mut IrBlock,
    polled: bool,
    live_in: &dyn Fn(u32) -> u32,
    edit: bool,
    removed: &mut usize,
    obs: Observers,
) -> u32 {
    let n = b.insns.len();
    let mut live = 0;
    for (k, i) in b.insns.iter_mut().enumerate().rev() {
        let after = if k + 1 == n {
            control(&i.control, i.next_pc(), live_in)
        } else if matches!(i.control, Control::Next) {
            live
        } else {
            // An unsupported stop in the middle of a block.
            ALL
        };
        live = insn(i, after, edit, removed, obs);
    }
    if polled { ALL } else { live }
}

/// Live-in flags of every block of `f`, every fault exit exact.
pub fn live_in(f: &mut IrFunction, shape: &Shape<'_>) -> BTreeMap<u32, u32> {
    live_in_with(f, shape, Observers::PRECISE)
}

/// [`live_in`] with the given observers.
pub fn live_in_with(f: &mut IrFunction, shape: &Shape<'_>, obs: Observers) -> BTreeMap<u32, u32> {
    let mut live: BTreeMap<u32, u32> = f.blocks.keys().map(|&k| (k, 0)).collect();
    let mut scratch = 0;
    loop {
        let mut changed = false;
        for (&start, b) in f.blocks.iter_mut().rev() {
            let get = |pc: u32| live.get(&pc).copied().unwrap_or(ALL);
            let v = block(b, shape.polled.contains(&start), &get, false, &mut scratch, obs);
            if live[&start] != v {
                live.insert(start, v);
                changed = true;
            }
        }
        if !changed {
            return live;
        }
    }
}

/// The dead-flag pass: remove every flag write nothing can observe.
/// Returns the number of writes removed.
pub fn dead_flags(f: &mut IrFunction, shape: &Shape<'_>) -> usize {
    dead_flags_with(f, shape, Observers::PRECISE)
}

/// [`dead_flags`] with the given observers.
pub fn dead_flags_with(f: &mut IrFunction, shape: &Shape<'_>, obs: Observers) -> usize {
    let live = live_in_with(f, shape, obs);
    let get = |pc: u32| live.get(&pc).copied().unwrap_or(ALL);
    let mut removed = 0;
    for (&start, b) in f.blocks.iter_mut() {
        block(b, shape.polled.contains(&start), &get, true, &mut removed, obs);
    }
    removed
}

/// Flag writes in `f` (at any depth), for reports.
pub fn count_writes(f: &IrFunction) -> usize {
    fn walk(list: &[Stmt]) -> usize {
        list.iter()
            .map(|s| match s {
                Stmt::WriteFlag { .. } => 1,
                Stmt::If { then, otherwise, .. } => walk(then) + walk(otherwise),
                Stmt::Repeat { body, .. } => walk(body),
                _ => 0,
            })
            .sum()
    }
    f.insns().map(|i| walk(&i.stmts)).sum()
}
