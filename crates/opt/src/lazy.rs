//! Lazy flags: flag writes that are not *read* before they are written
//! again, other than by a `ReadFlag` in the same block, become one
//! [`Stmt::LazyFlags`] per instruction, a formula over the values the flags
//! are computed from.
//!
//! Most flags are never read: x86 sets five or six on every arithmetic
//! instruction and a branch reads one. The ones the dead-flag pass must keep
//! are kept because something could *observe* them: any load or store may
//! fault, every loop header may be preempted, and every exit spills EFLAGS.
//! Those observers are all exits from the handler, and they are cold. A
//! backend can therefore keep the formula's inputs and compute the flags only
//! on the way out (the C emitter does it in the handler's shared epilogue),
//! instead of on every trip round a loop.
//!
//! A *read* is a `ReadFlag` of the flag, `ReadEflags`, a `Repeat` (whose
//! exits restore the flags it started with), or an atomic instruction (the
//! runtime reads the spilled flags). A write is lazy when, on every path
//! from it, the flag is written again before any read, except a `ReadFlag`
//! in its own block: there the formula that is pending is known, so the
//! backend computes that one flag from it in place (`jnz` after `dec` needs
//! only `ZF`, not all five), leaving the rest pending. Reads in other blocks
//! keep the flag eager, since paths may meet there with different formulas.
//! The analysis is the dead-flag liveness with exits removed from the
//! observers, so it needs no knowledge of polls or selectors: entering a
//! handler starts with nothing pending, and every way out materialises what
//! is. None of this is needed for correctness (a backend settles pending
//! flags before any read), only to keep the settling off the hot path.
//!
//! Only top-level writes of a defined, non-constant value are candidates
//! (constants and undefined flags are already cheap), and an instruction's
//! candidates are grouped into one statement at the last of them, so nothing
//! that could observe the flags may sit between the first and the last. The
//! formula is cut at temps the instruction also uses for something else (the
//! result of an `add`, say), so the inputs it keeps are values the hot path
//! computes anyway. Formulas with more than [`MAX_ARGS`] inputs, or inputs
//! wider than 64 bits, stay eager.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use ir::{Control, Expr, FlagFormula, FlagValue, IrFunction, IrInsn, LazyFlags, Stmt, Target, Temp, Value, Width};

use crate::flags::{ALL, bit};

/// Most inputs one formula may capture: the C backend keeps this many
/// 64-bit slots per handler.
pub const MAX_ARGS: usize = 4;

/// Flags read before `list`, given those read after it.
fn reads(list: &[Stmt], mut live: u32) -> u32 {
    for s in list.iter().rev() {
        live = read_step(s, live);
    }
    live
}

fn read_step(s: &Stmt, live: u32) -> u32 {
    match s {
        Stmt::WriteFlag { flag, .. } => live & !bit(*flag),
        Stmt::WriteEflags { mask, .. } => live & !(*mask & ALL),
        Stmt::LazyFlags(l) => live & !l.formula.mask(),
        Stmt::ReadFlag { flag, .. } => live | bit(*flag),
        Stmt::ReadEflags { .. } | Stmt::Repeat { .. } => ALL,
        Stmt::If { then, otherwise, .. } => reads(then, live) | reads(otherwise, live),
        _ => live,
    }
}

fn insn_reads(insn: &IrInsn, after: u32) -> u32 {
    if insn.atomic.is_some() { ALL } else { reads(&insn.stmts, after) }
}

/// Like [`read_step`], but a `ReadFlag` is not a read: inside its block a
/// backend knows which lazy group a flag is pending from and can compute the
/// value it reads from the captured inputs without settling anything. Only
/// whole-EFLAGS reads (and, through the block's end, reads in other blocks)
/// need the flags in EFLAGS.
fn hard_step(s: &Stmt, live: u32) -> u32 {
    match s {
        Stmt::ReadFlag { .. } => live,
        Stmt::If { then, otherwise, .. } => hard(then, live) | hard(otherwise, live),
        s => read_step(s, live),
    }
}

fn hard(list: &[Stmt], mut live: u32) -> u32 {
    for s in list.iter().rev() {
        live = hard_step(s, live);
    }
    live
}

fn insn_hard(insn: &IrInsn, after: u32) -> u32 {
    if insn.atomic.is_some() { ALL } else { hard(&insn.stmts, after) }
}

/// Flags read on entry to each block.
fn read_in(f: &IrFunction) -> BTreeMap<u32, u32> {
    let mut live: BTreeMap<u32, u32> = f.blocks.keys().map(|&k| (k, 0)).collect();
    loop {
        let mut changed = false;
        for (&start, b) in f.blocks.iter().rev() {
            let mut v = 0;
            for (k, i) in b.insns.iter().enumerate().rev() {
                let after = if k + 1 == b.insns.len() { successors_read(i, &live) } else { v };
                v = insn_reads(i, after);
            }
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

/// Flags read after the last instruction of a block: what its local
/// successors read. Exits read nothing (they materialise).
fn successors_read(last: &IrInsn, live: &BTreeMap<u32, u32>) -> u32 {
    let get = |pc: u32| live.get(&pc).copied().unwrap_or(0);
    let side = |d: &ir::Dest| if d.tail { 0 } else { get(d.pc) };
    match &last.control {
        Control::Next => get(last.next_pc()),
        Control::Jump { target: Target::Direct(t), tail: false, .. } => get(*t),
        Control::Branch { taken, fallthrough, .. } => side(taken) | side(fallthrough),
        Control::Switch { cases, .. } => cases.iter().map(side).fold(0, |a, b| a | b),
        _ => 0,
    }
}

/// Whether a statement could leave the handler or read flags: nothing like
/// that may separate an instruction's lazy writes.
fn observes(s: &Stmt) -> bool {
    !matches!(
        s,
        Stmt::Assign { .. }
            | Stmt::ReadReg { .. }
            | Stmt::WriteReg { .. }
            | Stmt::WriteFlag { .. }
            | Stmt::ReadXmm { .. }
            | Stmt::WriteXmm { .. }
            | Stmt::ReadMmx { .. }
            | Stmt::WriteMmx { .. }
    )
}

/// Temps a statement reads.
fn operands(s: &Stmt) -> Vec<u32> {
    let mut out = Vec::new();
    let mut v = |x: &Value| {
        if let Value::Temp(t) = x {
            out.push(t.id);
        }
    };
    match s {
        Stmt::Assign { expr, .. } => expr.operands().for_each(&mut v),
        Stmt::WriteReg { value, .. }
        | Stmt::WriteXmm { value, .. }
        | Stmt::WriteMmx { value, .. }
        | Stmt::WriteEflags { value, .. } => v(value),
        Stmt::WriteFlag { value: FlagValue::Defined(x), .. } => v(x),
        Stmt::Load { addr, .. } => v(&addr.offset),
        Stmt::Store { addr, value } => {
            v(&addr.offset);
            v(value);
        }
        Stmt::FaultIf { cond, .. } | Stmt::StopIf { cond, .. } => v(cond),
        Stmt::Intrinsic { args, .. } => args.iter().for_each(&mut v),
        Stmt::LazyFlags(l) => l.args.iter().for_each(&mut v),
        Stmt::If { cond, then, otherwise } => {
            v(cond);
            for s in then.iter().chain(otherwise) {
                out.extend(operands(s));
            }
        }
        Stmt::Repeat { body, .. } => {
            for s in body {
                out.extend(operands(s));
            }
        }
        _ => {}
    }
    out
}

fn control_operands(c: &Control) -> Vec<u32> {
    let t = |v: &Value| if let Value::Temp(t) = v { vec![t.id] } else { vec![] };
    match c {
        Control::Jump { target: Target::Dynamic(v), .. } | Control::Call { target: Target::Dynamic(v), .. } => t(v),
        Control::Branch { cond, .. } => t(cond),
        Control::Switch { target, .. } | Control::Return { target, .. } => t(target),
        _ => vec![],
    }
}

/// Build the formula for `cands` (statement index, flag, value temp) of
/// `insn`, or `None` if it would need too many or too wide inputs.
fn formula(insn: &IrInsn, cands: &[(usize, ir::Flag, Temp)]) -> Option<(FlagFormula, Vec<Value>)> {
    // Top-level pure definitions.
    let defs: HashMap<u32, (usize, Expr)> = insn
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match s {
            Stmt::Assign { dst, expr } => Some((dst.id, (i, *expr))),
            _ => None,
        })
        .collect();
    let cand_stmts: HashSet<usize> = cands.iter().map(|c| c.0).collect();
    // Temps the rest of the instruction uses: those are inputs, not part
    // of the formula, so the hot path keeps computing them once.
    let mut leaves: HashSet<u32> = HashSet::new();
    loop {
        // The formula's temps: reachable from the candidates through
        // pure definitions, stopping at leaves.
        let mut inside: HashSet<u32> = HashSet::new();
        let mut stack: Vec<u32> = cands.iter().map(|c| c.2.id).collect();
        while let Some(t) = stack.pop() {
            if leaves.contains(&t) || !inside.insert(t) {
                continue;
            }
            if let Some((_, e)) = defs.get(&t) {
                stack.extend(e.operands().filter_map(|v| if let Value::Temp(x) = v { Some(x.id) } else { None }));
            }
        }
        let inside_stmts: HashSet<usize> = inside.iter().filter_map(|t| defs.get(t).map(|d| d.0)).collect();
        let mut used_outside: HashSet<u32> = HashSet::new();
        for (i, s) in insn.stmts.iter().enumerate() {
            if !cand_stmts.contains(&i) && !inside_stmts.contains(&i) {
                used_outside.extend(operands(s));
            }
        }
        used_outside.extend(control_operands(&insn.control));
        let new: Vec<u32> =
            inside.iter().copied().filter(|t| defs.contains_key(t) && used_outside.contains(t)).collect();
        if new.is_empty() {
            // Inputs: temps in the closure with no pure definition, and the
            // cut points.
            let mut params: Vec<Width> = Vec::new();
            let mut args: Vec<Value> = Vec::new();
            let mut map: HashMap<u32, Temp> = HashMap::new();
            let width_of = |id: u32| -> Option<Width> {
                insn.stmts.iter().find_map(|s| match s {
                    Stmt::Assign { dst, .. }
                    | Stmt::ReadReg { dst, .. }
                    | Stmt::ReadFlag { dst, .. }
                    | Stmt::ReadEflags { dst }
                    | Stmt::ReadXmm { dst, .. }
                    | Stmt::ReadMmx { dst, .. }
                    | Stmt::Load { dst, .. } => (dst.id == id).then_some(dst.width),
                    Stmt::Intrinsic { dsts, .. } => dsts.iter().find(|d| d.id == id).map(|d| d.width),
                    _ => None,
                })
            };
            // Visit in statement order so parameters number deterministically.
            let mut body_stmts: Vec<usize> = inside_stmts.iter().copied().collect();
            body_stmts.sort_unstable();
            let param = |id: u32, map: &mut HashMap<u32, Temp>, params: &mut Vec<Width>, args: &mut Vec<Value>| {
                if map.contains_key(&id) {
                    return Some(());
                }
                let w = width_of(id)?;
                let t = Temp { id: params.len() as u32, width: w };
                params.push(w);
                args.push(Value::Temp(Temp { id, width: w }));
                map.insert(id, t);
                Some(())
            };
            for &i in &body_stmts {
                let Stmt::Assign { expr, .. } = &insn.stmts[i] else { unreachable!() };
                for v in expr.operands() {
                    if let Value::Temp(t) = v
                        && !(inside.contains(&t.id) && defs.contains_key(&t.id))
                    {
                        param(t.id, &mut map, &mut params, &mut args)?;
                    }
                }
            }
            for c in cands {
                if !(inside.contains(&c.2.id) && defs.contains_key(&c.2.id)) {
                    param(c.2.id, &mut map, &mut params, &mut args)?;
                }
            }
            if params.len() > MAX_ARGS || params.iter().any(|w| w.bits() > 64) {
                return None;
            }
            let n = params.len() as u32;
            let mut body = Vec::new();
            for (k, &i) in body_stmts.iter().enumerate() {
                let Stmt::Assign { dst, expr } = &insn.stmts[i] else { unreachable!() };
                let t = Temp { id: n + k as u32, width: dst.width };
                map.insert(dst.id, t);
                body.push((t, *expr));
            }
            let remap = |v: Value| match v {
                Value::Temp(t) => Value::Temp(map[&t.id]),
                c => c,
            };
            for (_, e) in body.iter_mut() {
                *e = match *e {
                    Expr::Unary { op, a } => Expr::Unary { op, a: remap(a) },
                    Expr::Binary { op, a, b } => Expr::Binary { op, a: remap(a), b: remap(b) },
                    Expr::Compare { op, a, b } => Expr::Compare { op, a: remap(a), b: remap(b) },
                    Expr::Cast { op, a } => Expr::Cast { op, a: remap(a) },
                    Expr::Select { cond, a, b } => Expr::Select { cond: remap(cond), a: remap(a), b: remap(b) },
                };
            }
            let flags = cands.iter().map(|c| (c.1, remap(Value::Temp(c.2)))).collect();
            return Some((FlagFormula { params, body, flags }, args));
        }
        leaves.extend(new);
    }
}

/// Make one instruction's unread flag writes lazy; returns how many.
fn instruction(insn: &mut IrInsn, read_after: u32, interned: &mut HashMap<FlagFormula, Arc<FlagFormula>>) -> usize {
    if insn.atomic.is_some() {
        return 0;
    }
    // Flags read after each top-level statement.
    let n = insn.stmts.len();
    let mut after = vec![0u32; n];
    let mut live = read_after;
    for i in (0..n).rev() {
        after[i] = live;
        live = hard_step(&insn.stmts[i], live);
    }
    let cands: Vec<(usize, ir::Flag, Temp)> = insn
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match s {
            Stmt::WriteFlag { flag, value: FlagValue::Defined(Value::Temp(t)) } if after[i] & bit(*flag) == 0 => {
                Some((i, *flag, *t))
            }
            _ => None,
        })
        .collect();
    let (Some(first), Some(last)) = (cands.first(), cands.last()) else { return 0 };
    // Only the result flags (PF, ZF, SF; parity above all) are worth
    // deferring. A group of carry and overflow alone (a rotate, `bt`) is a
    // bit or two of what the instruction computes anyway, and making it
    // lazy would settle whatever another group left pending.
    if !cands.iter().any(|c| matches!(c.1, ir::Flag::Pf | ir::Flag::Zf | ir::Flag::Sf)) {
        return 0;
    }
    if insn.stmts[first.0..last.0].iter().any(observes) {
        return 0;
    }
    let Some((formula, args)) = formula(insn, &cands) else { return 0 };
    let formula = interned.entry(formula.clone()).or_insert_with(|| Arc::new(formula)).clone();
    let at = last.0;
    insn.stmts[at] = Stmt::LazyFlags(LazyFlags { formula, args });
    for c in cands.iter().rev().skip(1) {
        insn.stmts.remove(c.0);
    }
    cands.len()
}

/// The lazy-flag pass over `f`; returns the number of flag writes made lazy.
pub fn lazy_flags(f: &mut IrFunction) -> usize {
    let live = read_in(f);
    let mut interned = HashMap::new();
    let mut made = 0;
    for b in f.blocks.values_mut() {
        let n = b.insns.len();
        // Hard reads after each instruction, from the block's end
        // backwards: any read at all in another block, whole-EFLAGS reads
        // in this one.
        let mut after = vec![0u32; n];
        let mut v = 0;
        for k in (0..n).rev() {
            after[k] = if k + 1 == n { successors_read(&b.insns[k], &live) } else { v };
            v = insn_hard(&b.insns[k], after[k]);
        }
        for (k, i) in b.insns.iter_mut().enumerate() {
            made += instruction(i, after[k], &mut interned);
        }
    }
    made
}
