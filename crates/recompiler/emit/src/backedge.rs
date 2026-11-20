//! Loop back-edges, and the loop headers where generated code polls for
//! preemption.
//!
//! A handler's internal control flow is `goto`s between its blocks, so a
//! guest loop that makes no call never comes back to the runner on its
//! own: a spin-wait on a flag another guest thread sets would hang the
//! cooperative scheduler. Every loop header therefore gets a safe point
//! (`H2_LOOP_POLL` in `h2gen.h`): one relaxed load of the runtime's preempt
//! word and a branch, which clang rotates onto the back-edge. When the word
//! is set the handler leaves with `JUMP`/`RESUME` to the header, which
//! [`crate::plan`] registers as a resume of the owner, and the runner
//! resumes the loop there after its own safe point. A resume enters just
//! past the poll, so each handler call makes at least one trip round the
//! loop whatever the word says (RUNTIME-CONTRACT.md, "Loop safe points").
//!
//! Detection is conservative: a depth-first search over local edges, from
//! the entry and then from every block not yet reached, and every edge to a
//! block still on the search stack is a back-edge. Any cycle in any graph
//! contains at least one such retreating edge for any DFS order, so every
//! loop, reducible or not, and every self-loop has a polled header on it.
//! Edges that leave the handler (calls, returns, tail jumps, computed
//! jumps) are not followed: those already pass through the runner, which
//! is a safe point itself, so a loop that contains a call needs no poll.

use std::collections::{BTreeMap, BTreeSet};

use ir::{Control, Dest, IrBlock, IrFunction, Target};

/// The back-edges of one owner and the loop headers they target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loops {
    /// `(block start, header)`: the block whose last instruction jumps,
    /// branches, switches or falls through to `header`, a block that is
    /// still on the DFS stack.
    pub back_edges: BTreeSet<(u32, u32)>,
    /// Every back-edge target: each polls, and is a resume selector.
    pub headers: BTreeSet<u32>,
}

/// Local successors of a block: the targets its last instruction can reach
/// without leaving the handler.
pub(crate) fn successors(b: &IrBlock, blocks: &BTreeMap<u32, IrBlock>) -> Vec<u32> {
    let Some(last) = b.insns.last() else { return Vec::new() };
    let local = |d: &Dest| (!d.tail).then_some(d.pc);
    let out: Vec<u32> = match &last.control {
        Control::Next => vec![last.next_pc()],
        Control::Jump { target: Target::Direct(t), tail: false, .. } => vec![*t],
        Control::Branch { taken, fallthrough, .. } => {
            [local(taken), local(fallthrough)].into_iter().flatten().collect()
        }
        Control::Switch { cases, .. } => cases.iter().filter_map(local).collect(),
        _ => Vec::new(),
    };
    out.into_iter().filter(|t| blocks.contains_key(t)).collect()
}

/// Find every back-edge of `f`.
pub fn find(f: &IrFunction) -> Loops {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Open,
        Done,
    }
    let mut mark: BTreeMap<u32, Mark> = f.blocks.keys().map(|&k| (k, Mark::New)).collect();
    let mut loops = Loops::default();
    let roots = std::iter::once(f.entry).chain(f.blocks.keys().copied());
    for root in roots {
        if mark.get(&root) != Some(&Mark::New) {
            continue;
        }
        // Iterative DFS: (block, its successors, next successor to visit).
        let mut stack = vec![(root, successors(&f.blocks[&root], &f.blocks), 0usize)];
        mark.insert(root, Mark::Open);
        while let Some((block, succ, next)) = stack.last_mut() {
            let Some(&t) = succ.get(*next) else {
                mark.insert(*block, Mark::Done);
                stack.pop();
                continue;
            };
            *next += 1;
            let from = *block;
            match mark[&t] {
                Mark::Open => {
                    loops.back_edges.insert((from, t));
                    loops.headers.insert(t);
                }
                Mark::New => {
                    mark.insert(t, Mark::Open);
                    stack.push((t, successors(&f.blocks[&t], &f.blocks), 0));
                }
                Mark::Done => {}
            }
        }
    }
    loops
}

#[cfg(test)]
mod tests {
    use super::*;
    use ir::{IrInsn, Value, Width};

    fn insn(pc: u32, len: u8, control: Control) -> IrInsn {
        IrInsn { pc, len, atomic: None, stmts: vec![], control }
    }

    fn func(blocks: Vec<(u32, Vec<IrInsn>)>) -> IrFunction {
        IrFunction {
            entry: blocks[0].0,
            blocks: blocks.into_iter().map(|(s, insns)| (s, IrBlock { start: s, insns })).collect(),
            interior_entries: vec![],
            errors: vec![],
        }
    }

    fn cond() -> Value {
        Value::Const { width: Width::W1, bits: 1 }
    }

    fn branch(pc: u32, taken: u32, fall: u32) -> IrInsn {
        insn(pc, 2, Control::Branch { cond: cond(), taken: Dest::local(taken), fallthrough: Dest::local(fall) })
    }

    fn ret(pc: u32) -> IrInsn {
        insn(pc, 1, Control::Return { target: cond(), cleanup: 0 })
    }

    #[test]
    fn a_spin_loop_has_one_back_edge_to_its_header() {
        // 10: cmp; jne 10 (fall 20)   20: ret
        let f = func(vec![(0x10, vec![branch(0x10, 0x10, 0x20)]), (0x20, vec![ret(0x20)])]);
        let l = find(&f);
        assert_eq!(l.back_edges, BTreeSet::from([(0x10, 0x10)]));
        assert_eq!(l.headers, BTreeSet::from([0x10]));
    }

    #[test]
    fn straight_line_and_diamonds_have_none() {
        // 10: br 20/30   20: jmp 40   30: next -> 40   40: ret
        let f = func(vec![
            (0x10, vec![branch(0x10, 0x20, 0x30)]),
            (
                0x20,
                vec![insn(
                    0x20,
                    2,
                    Control::Jump { target: Target::Direct(0x40), tail: false, kind: ir::CallKind::Direct },
                )],
            ),
            (0x30, vec![insn(0x30, 0x10, Control::Next)]),
            (0x40, vec![ret(0x40)]),
        ]);
        assert_eq!(find(&f), Loops::default());
    }

    #[test]
    fn a_loop_through_a_call_is_left_to_the_runner() {
        // 10: call x (continuation 15)   15: br 10/20   20: ret
        let call = insn(
            0x10,
            5,
            Control::Call {
                target: Target::Direct(0x900),
                continuation: 0x15,
                returns: true,
                kind: ir::CallKind::Direct,
            },
        );
        let f = func(vec![(0x10, vec![call]), (0x15, vec![branch(0x15, 0x10, 0x20)]), (0x20, vec![ret(0x20)])]);
        // The call edge is not followed: 0x10 (the entry) has no local
        // successor and is finished before 0x15 is searched, so 0x15 -> 0x10
        // is no back-edge. The cycle goes through the runner at the call.
        assert_eq!(find(&f), Loops::default());
    }

    #[test]
    fn nested_and_irreducible_loops_get_a_check_on_every_cycle() {
        // Nested: 10 -> 20 -> 20 (inner), 20 -> 30 -> 10 (outer), 30 -> 40.
        let f = func(vec![
            (0x10, vec![insn(0x10, 0x10, Control::Next)]),
            (0x20, vec![branch(0x20, 0x20, 0x30)]),
            (0x30, vec![branch(0x30, 0x10, 0x40)]),
            (0x40, vec![ret(0x40)]),
        ]);
        let l = find(&f);
        assert_eq!(l.back_edges, BTreeSet::from([(0x20, 0x20), (0x30, 0x10)]));
        // Irreducible: 10 -> 20 | 30, 20 <-> 30.
        let f = func(vec![
            (0x10, vec![branch(0x10, 0x20, 0x30)]),
            (0x20, vec![insn(0x20, 0x10, Control::Next)]),
            (0x30, vec![branch(0x30, 0x20, 0x40)]),
            (0x40, vec![ret(0x40)]),
        ]);
        let l = find(&f);
        assert_eq!(l.back_edges.len(), 1, "{l:?}");
    }

    #[test]
    fn tail_edges_are_not_followed() {
        // 10: br tail(10)/20: a branch back to its own start that is marked
        // tail leaves through dispatch, so it is no back-edge.
        let b = insn(
            0x10,
            2,
            Control::Branch { cond: cond(), taken: Dest { pc: 0x10, tail: true }, fallthrough: Dest::local(0x20) },
        );
        let f = func(vec![(0x10, vec![b]), (0x20, vec![ret(0x20)])]);
        assert_eq!(find(&f), Loops::default());
    }
}
