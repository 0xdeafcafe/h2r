//! Lower decoded x86 into the semantic IR (`crates/ir`).
//!
//! Two layers:
//!
//! - [`lower()`] turns one [`iced_x86::Instruction`] into one
//!   [`ir::IrInsn`]. It needs only image-wide facts ([`Env`]: kernel import
//!   slots) and, for transfers, what the CFG decided about that site
//!   ([`Site`]). Fixture tests drive it directly.
//! - [`Lifter`] walks the analyzer's per-function CFGs
//!   ([`analyze::cfg`]) and lowers every block, filling in each transfer's
//!   [`Site`] from the block's edges.
//!
//! Coverage is reported, never assumed: an instruction the lowering does
//! not implement becomes an [`ir::Stmt::Unsupported`], and [`LiftStats`]
//! counts them.

mod builder;
pub mod crt;
pub mod harness;
pub mod lower;

use std::collections::{BTreeMap, HashMap, HashSet};

use analyze::cfg::{Block, CfgBuilder, CfgError, EdgeKind};
use analyze::Analysis;
use ir::{CallKind, Control, DescentError, IrBlock, IrFunction, IrInsn, StopReason, Target};
use xbe::Xbe;

pub use lower::{Env, Site, lower, mnemonic_name};

/// Lifts functions found by discovery.
pub struct Lifter<'a> {
    analysis: &'a Analysis,
    cfgs: CfgBuilder<'a>,
    env: Env,
    /// Functions whose IR is written by hand (`crt`), not lowered.
    crt: crt::Replacements,
}

impl<'a> Lifter<'a> {
    pub fn new(analysis: &'a Analysis, xbe: &Xbe<'a>) -> Result<Self, xbe::Error> {
        Ok(Lifter {
            analysis,
            cfgs: CfgBuilder::new(analysis, xbe),
            env: Env::of(xbe)?,
            crt: crt::Replacements::of(xbe),
        })
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn analysis(&self) -> &'a Analysis {
        self.analysis
    }

    /// Lift the function starting at `entry`, or `None` if discovery has
    /// no function there.
    pub fn lift(&self, entry: u32) -> Option<IrFunction> {
        let cfg = self.cfgs.build(entry)?;
        if let Some(f) = self.crt.function(entry) {
            return Some(f);
        }
        let mut blocks = BTreeMap::new();
        for block in cfg.blocks.values() {
            let mut insns = Vec::with_capacity(block.insns.len());
            for (i, &va) in block.insns.iter().enumerate() {
                let Some(insn) = self.cfgs.code().decode(va) else {
                    // The CFG decoded it a moment ago; this can't happen for
                    // the same image, but a missing instruction must still
                    // be visible.
                    insns.push(unsupported_at(va, "undecodable"));
                    continue;
                };
                let last = i + 1 == block.insns.len();
                let site = if last { self.site(block) } else { Site::Plain };
                let mut ir = lower(&insn, &self.env, site);
                if last {
                    apply_edges(&mut ir, block);
                }
                insns.push(ir);
            }
            blocks.insert(block.start, IrBlock { start: block.start, insns });
        }
        let errors = cfg
            .errors
            .iter()
            .map(|e| match *e {
                CfgError::Decode(a) => DescentError::Decode(a),
                CfgError::OutsideCode(a) => DescentError::OutsideCode(a),
                CfgError::IntoInsnBody(a) => DescentError::IntoInsnBody(a),
                CfgError::OverlapsInsn(a) => DescentError::OverlapsInsn(a),
            })
            .collect();
        Some(IrFunction { entry, blocks, interior_entries: cfg.interior_entries, errors })
    }

    /// What the CFG says about a block's final transfer. Every edge of a
    /// block comes from its last instruction, so the first edge's kind
    /// names the terminator (the others are the rest of a branch's or
    /// switch's targets).
    fn site(&self, block: &Block) -> Site<'a> {
        let Some(first) = block.edges.first() else { return Site::Plain };
        match first.kind {
            EdgeKind::Tail | EdgeKind::TailKernel { .. } | EdgeKind::TailIndirect => Site::Tail,
            EdgeKind::Jump => Site::LocalJump,
            EdgeKind::Switch { site, .. } => self.analysis.jump_tables.get(&site).map_or(Site::Plain, Site::Switch),
            EdgeKind::CallContinuation => Site::Call { returns: true },
            EdgeKind::NoReturnCall | EdgeKind::KernelNoReturn(_) => Site::Call { returns: false },
            EdgeKind::Fallthrough | EdgeKind::Taken | EdgeKind::NotTaken | EdgeKind::Return | EdgeKind::Trap => {
                Site::Plain
            }
        }
    }

    /// Lift every discovered function and count what lowered.
    pub fn stats(&self) -> LiftStats {
        let mut s = LiftStats::default();
        let mut seen: HashMap<u32, bool> = HashMap::new();
        let mut unsupported: HashMap<String, (usize, HashSet<String>)> = HashMap::new();
        for &entry in self.analysis.functions.keys() {
            let Some(f) = self.lift(entry) else { continue };
            s.functions += 1;
            if let Err(e) = f.validate() {
                s.invalid.push(format!("function {entry:#x}: {e}"));
            }
            let mut full = true;
            for insn in f.insns() {
                let ok = insn.is_supported();
                full &= ok;
                if seen.insert(insn.pc, ok).is_none() {
                    s.insns += 1;
                    if ok {
                        s.supported += 1;
                    } else if let Some((m, d)) = insn.unsupported() {
                        let e = unsupported.entry(m.to_owned()).or_default();
                        e.0 += 1;
                        e.1.insert(d.to_owned());
                    }
                }
            }
            if full {
                s.functions_full += 1;
            }
        }
        let mut list: Vec<_> = unsupported
            .into_iter()
            .map(|(m, (n, d))| {
                let mut d: Vec<String> = d.into_iter().collect();
                d.sort();
                (m, n, d)
            })
            .collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        s.unsupported = list;
        s
    }

    /// Mnemonic frequencies over every instruction of every discovered
    /// function, each address counted once (shared code is not doubled).
    pub fn histogram(&self) -> Vec<(String, usize)> {
        let mut seen = HashSet::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for &entry in self.analysis.functions.keys() {
            let Some(cfg) = self.cfgs.build(entry) else { continue };
            for va in cfg.blocks.values().flat_map(|b| b.insns.iter().copied()) {
                if !seen.insert(va) {
                    continue;
                }
                if let Some(insn) = self.cfgs.code().decode(va) {
                    let mut name = mnemonic_name(&insn);
                    if insn.has_lock_prefix() {
                        name = format!("lock {name}");
                    } else if insn.has_repe_prefix() || insn.has_repne_prefix() {
                        name = format!("rep {name}");
                    }
                    *counts.entry(name).or_default() += 1;
                }
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }
}

fn unsupported_at(pc: u32, detail: &str) -> IrInsn {
    IrInsn {
        pc,
        len: 1,
        atomic: None,
        stmts: vec![ir::Stmt::Unsupported { mnemonic: "?".into(), detail: detail.into() }],
        control: Control::Stop(StopReason::Unsupported),
    }
}

/// Apply the CFG's verdict on a block's last instruction: branch sides,
/// fall-throughs and switch cases that land on another function's entry
/// become tail transfers, and a block the descent could not continue
/// stops instead of falling into nothing.
fn apply_edges(insn: &mut IrInsn, block: &Block) {
    let exits = |kind: EdgeKind| block.edges.iter().any(|e| e.kind == kind && e.exits);
    let next = insn.next_pc();
    match &mut insn.control {
        Control::Next => {
            if exits(EdgeKind::Fallthrough) {
                insn.control = Control::Jump { target: Target::Direct(next), tail: true, kind: CallKind::Direct };
            } else if !block.edges.iter().any(|e| e.kind == EdgeKind::Fallthrough) {
                // No fall-through edge: the descent stopped here on an error
                // (or the CFG ended the block on an instruction the lowering
                // treats as falling through).
                insn.control = Control::Stop(StopReason::DescentError);
            }
        }
        Control::Branch { taken, fallthrough, .. } => {
            taken.tail = exits(EdgeKind::Taken);
            fallthrough.tail = exits(EdgeKind::NotTaken);
        }
        Control::Switch { cases, .. } => {
            let switch_edges = block.edges.iter().filter(|e| matches!(e.kind, EdgeKind::Switch { .. }));
            for (c, e) in cases.iter_mut().zip(switch_edges) {
                debug_assert_eq!(Some(c.pc), e.target);
                c.tail = e.exits;
            }
        }
        _ => {}
    }
}

/// Coverage of a whole-image lift.
#[derive(Debug, Default, Clone)]
pub struct LiftStats {
    /// Functions lifted.
    pub functions: usize,
    /// Functions with no unsupported instruction.
    pub functions_full: usize,
    /// Distinct instruction addresses lifted.
    pub insns: usize,
    /// Of those, how many lowered without an `Unsupported`.
    pub supported: usize,
    /// Unsupported mnemonics: (name, distinct instructions, reasons),
    /// most frequent first.
    pub unsupported: Vec<(String, usize, Vec<String>)>,
    /// IR validation failures (lowering bugs). Should be empty.
    pub invalid: Vec<String>,
}

impl std::fmt::Display for LiftStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pct = |a: usize, b: usize| if b == 0 { 100.0 } else { 100.0 * a as f64 / b as f64 };
        writeln!(
            f,
            "functions:    {:>8} lifted, {:>8} fully lowered ({:.2}%)",
            self.functions,
            self.functions_full,
            pct(self.functions_full, self.functions)
        )?;
        writeln!(
            f,
            "instructions: {:>8} distinct, {:>8} lowered ({:.2}%)",
            self.insns,
            self.supported,
            pct(self.supported, self.insns)
        )?;
        writeln!(f, "IR validation failures: {}", self.invalid.len())?;
        for e in self.invalid.iter().take(10) {
            writeln!(f, "  {e}")?;
        }
        // Reasons start with the ISA extension ("x87: ...").
        let ext = |why: &[String]| why.first().and_then(|w| w.split(':').next()).unwrap_or("?").to_owned();
        let mut by_ext: BTreeMap<String, usize> = BTreeMap::new();
        for (_, n, why) in &self.unsupported {
            *by_ext.entry(ext(why)).or_default() += n;
        }
        writeln!(f, "unsupported by extension: {by_ext:?}")?;
        writeln!(f, "top unsupported mnemonics (all integer ones, then the 30 most frequent others):")?;
        let integer = self.unsupported.iter().filter(|u| ext(&u.2) == "integer");
        let limit = if f.alternate() { usize::MAX } else { 30 };
        let others = self.unsupported.iter().filter(|u| ext(&u.2) != "integer").take(limit);
        for (m, n, why) in integer.chain(others) {
            let why = if why.len() > 3 { format!("{} (+{} more)", why[..3].join("; "), why.len() - 3) } else { why.join("; ") };
            writeln!(f, "  {m:<12} {n:>7}  {why}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use analyze::cfg::{Block, Edge, EdgeKind};
    use ir::{CallKind, Control, Dest, IrInsn, StopReason, Target, Value};

    use super::apply_edges;

    fn block(edges: Vec<Edge>) -> Block {
        Block { start: 0x1000, end: 0x1002, insns: vec![0x1000], edges }
    }

    fn insn(control: Control) -> IrInsn {
        IrInsn { pc: 0x1000, len: 2, atomic: None, stmts: vec![], control }
    }

    fn edge(kind: EdgeKind, target: u32, exits: bool) -> Edge {
        Edge { kind, target: Some(target), exits }
    }

    #[test]
    fn edges_into_other_entries_become_tail_transfers() {
        let mut i = insn(Control::Branch {
            cond: Value::bool(true),
            taken: Dest::local(0x2000),
            fallthrough: Dest::local(0x1002),
        });
        apply_edges(&mut i, &block(vec![edge(EdgeKind::Taken, 0x2000, true), edge(EdgeKind::NotTaken, 0x1002, false)]));
        assert_eq!(
            i.control,
            Control::Branch { cond: Value::bool(true), taken: Dest { pc: 0x2000, tail: true }, fallthrough: Dest::local(0x1002) }
        );

        let mut i = insn(Control::Next);
        apply_edges(&mut i, &block(vec![edge(EdgeKind::Fallthrough, 0x1002, true)]));
        assert_eq!(i.control, Control::Jump { target: Target::Direct(0x1002), tail: true, kind: CallKind::Direct });

        let mut i = insn(Control::Next);
        apply_edges(&mut i, &block(vec![edge(EdgeKind::Fallthrough, 0x1002, false)]));
        assert_eq!(i.control, Control::Next);
    }

    #[test]
    fn a_block_the_descent_could_not_continue_stops() {
        let mut i = insn(Control::Next);
        apply_edges(&mut i, &block(vec![]));
        assert_eq!(i.control, Control::Stop(StopReason::DescentError));
    }

    #[test]
    fn canonical_owner_prefers_the_function_the_code_sits_in() {
        assert_eq!(ir::canonical_owner(0x1500, [0x2000, 0x1000, 0x1400]), Some(0x1400));
        assert_eq!(ir::canonical_owner(0x0500, [0x2000, 0x1000]), Some(0x1000));
        assert_eq!(ir::canonical_owner(0x0500, []), None);
    }
}
