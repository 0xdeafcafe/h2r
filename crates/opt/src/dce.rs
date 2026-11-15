//! Dead temp elimination inside one instruction.
//!
//! Temps are instruction-local SSA values, so a pure statement whose result
//! nothing in the instruction reads can go. Only statements without effects
//! are removed: computations and reads of architectural state. Loads (which
//! may fault), intrinsics and everything that writes state stay.

use std::collections::HashSet;

use ir::{Control, IrInsn, Stmt, Target, Value};

fn use_value(v: &Value, used: &mut HashSet<u32>) {
    if let Value::Temp(t) = v {
        used.insert(t.id);
    }
}

fn uses(stmts: &[Stmt], used: &mut HashSet<u32>) {
    for s in stmts {
        match s {
            Stmt::Assign { expr, .. } => expr.operands().for_each(|v| use_value(v, used)),
            Stmt::WriteReg { value, .. }
            | Stmt::WriteXmm { value, .. }
            | Stmt::WriteMmx { value, .. }
            | Stmt::WriteEflags { value, .. } => use_value(value, used),
            Stmt::WriteFlag { value, .. } => {
                if let ir::FlagValue::Defined(v) = value {
                    use_value(v, used);
                }
            }
            Stmt::Load { addr, .. } => use_value(&addr.offset, used),
            Stmt::Store { addr, value } => {
                use_value(&addr.offset, used);
                use_value(value, used);
            }
            Stmt::FaultIf { cond, .. } | Stmt::StopIf { cond, .. } => use_value(cond, used),
            Stmt::If { cond, then, otherwise } => {
                use_value(cond, used);
                uses(then, used);
                uses(otherwise, used);
            }
            Stmt::Intrinsic { args, .. } => args.iter().for_each(|v| use_value(v, used)),
            Stmt::Repeat { body, .. } => uses(body, used),
            Stmt::ReadReg { .. }
            | Stmt::ReadFlag { .. }
            | Stmt::ReadEflags { .. }
            | Stmt::ReadXmm { .. }
            | Stmt::ReadMmx { .. }
            | Stmt::Unsupported { .. } => {}
        }
    }
}

fn control_uses(c: &Control, used: &mut HashSet<u32>) {
    let target = |t: &Target, used: &mut HashSet<u32>| {
        if let Target::Dynamic(v) = t {
            use_value(v, used);
        }
    };
    match c {
        Control::Jump { target: t, .. } | Control::Call { target: t, .. } => target(t, used),
        Control::Branch { cond, .. } => use_value(cond, used),
        Control::Switch { target: v, .. } | Control::Return { target: v, .. } => use_value(v, used),
        Control::Next | Control::Fault(_) | Control::Stop(_) => {}
    }
}

/// The temp a pure statement defines, if it is one.
fn pure_def(s: &Stmt) -> Option<u32> {
    match s {
        Stmt::Assign { dst, .. }
        | Stmt::ReadReg { dst, .. }
        | Stmt::ReadFlag { dst, .. }
        | Stmt::ReadEflags { dst }
        | Stmt::ReadXmm { dst, .. }
        | Stmt::ReadMmx { dst, .. } => Some(dst.id),
        _ => None,
    }
}

fn sweep(stmts: &mut Vec<Stmt>, used: &HashSet<u32>) -> usize {
    let before = stmts.len();
    stmts.retain(|s| pure_def(s).is_none_or(|id| used.contains(&id)));
    let mut removed = before - stmts.len();
    for s in stmts.iter_mut() {
        match s {
            Stmt::If { then, otherwise, .. } => {
                removed += sweep(then, used);
                removed += sweep(otherwise, used);
            }
            Stmt::Repeat { body, .. } => removed += sweep(body, used),
            _ => {}
        }
    }
    removed
}

/// Remove pure statements whose results are unused; returns how many went.
/// Atomic instructions keep their validated shape and are left alone.
pub fn instruction(insn: &mut IrInsn) -> usize {
    if insn.atomic.is_some() {
        return 0;
    }
    let mut total = 0;
    loop {
        let mut used = HashSet::new();
        uses(&insn.stmts, &mut used);
        control_uses(&insn.control, &mut used);
        let n = sweep(&mut insn.stmts, &used);
        if n == 0 {
            return total;
        }
        total += n;
    }
}
