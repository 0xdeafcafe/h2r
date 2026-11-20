//! Specialize native x87 helpers on the common, committed stack effect.
//! TOP at a normal entry is predicted to be zero. Each specialized site
//! checks the architectural TOP before using that prediction. A caller
//! with a live stack, an interior resume, or an unmasked exception that
//! suppressed a push/pop therefore takes the dynamic implementation.

use std::collections::{BTreeMap, VecDeque};

use ir::x87::{X87Env, X87Op, X87Unary};
use ir::{Control, Intrinsic, IrFunction, Stmt};

fn join(a: Option<u8>, b: Option<u8>) -> Option<u8> {
    if a == b { a } else { None }
}

/// Predicted TOP after one statement. None is a disagreement or an
/// unknown stack effect; an explicit reset can recover a known value.
pub(crate) fn after(top: Option<u8>, stmt: &Stmt) -> Option<u8> {
    let delta = match stmt {
        Stmt::Intrinsic { op: Intrinsic::X87(op), .. } => match *op {
            X87Op::Load(_) | X87Op::LoadConst(_) | X87Op::LoadReg(_) | X87Op::DecStp => 7,
            X87Op::IncStp | X87Op::Ftol2 => 1,
            X87Op::StoreCommit { pop } | X87Op::StoreReg { pop, .. }
            | X87Op::Free { pop, .. } | X87Op::Arith { pop, .. } => u8::from(pop),
            X87Op::Cmp { pop, .. } => pop,
            X87Op::Init | X87Op::SaveCommit(X87Env::Save) => return Some(0),
            X87Op::LoadEnv(_) => return None,
            X87Op::Unary(u) => match u {
                X87Unary::Patan | X87Unary::Yl2x | X87Unary::Yl2xp1 => 1,
                X87Unary::Xtract => 7,
                // Large arguments can set C2 without pushing.
                X87Unary::Tan | X87Unary::SinCos => return None,
                _ => 0,
            },
            _ => 0,
        },
        Stmt::Intrinsic { op: Intrinsic::MmxEnter, .. } => return Some(0),
        Stmt::If { then, otherwise, .. } => {
            return join(then.iter().fold(top, after), otherwise.iter().fold(top, after));
        }
        Stmt::Repeat { body, .. } => return join(top, body.iter().fold(top, after)),
        _ => 0,
    };
    top.map(|t| t.wrapping_add(delta) & 7)
}

/// Instruction-entry predictions for blocks reached with a consistent
/// depth. Unreached blocks have no entry. A call continuation may start
/// with any TOP, including when the runner routes a return there.
pub(crate) fn hints(f: &IrFunction) -> BTreeMap<u32, u8> {
    let mut inputs = BTreeMap::from([(f.entry, Some(0))]);
    let mut work = VecDeque::from([f.entry]);
    while let Some(pc) = work.pop_front() {
        let Some(block) = f.blocks.get(&pc) else {
            continue;
        };
        let mut top = inputs[&pc];
        for i in &block.insns {
            top = i.stmts.iter().fold(top, after);
        }
        let mut successors = crate::backedge::successors(block, &f.blocks);
        if let Some(Control::Call { continuation, .. }) = block.insns.last().map(|i| &i.control) {
            successors.push(*continuation);
            top = None;
        }
        for next in successors {
            let merged = inputs.get(&next).map_or(top, |&prev| join(prev, top));
            if inputs.insert(next, merged) != Some(merged) {
                work.push_back(next);
            }
        }
    }
    let mut pcs = BTreeMap::new();
    for (pc, mut top) in inputs {
        let Some(block) = f.blocks.get(&pc) else {
            continue;
        };
        for i in &block.insns {
            if let Some(t) = top
                && i.stmts.iter().any(|s| matches!(s, Stmt::Intrinsic { op: Intrinsic::X87(_), .. }))
            {
                pcs.insert(i.pc, t);
            }
            top = i.stmts.iter().fold(top, after);
        }
    }
    pcs
}
