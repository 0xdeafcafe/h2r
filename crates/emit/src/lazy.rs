//! C for [`Stmt::LazyFlags`]: flags computed on the way out.
//!
//! A handler with lazy flags keeps, besides `fl`/`fd`, the inputs of the last
//! lazy definition in 64-bit slots `lz0..`, which formula it was (`lzk`, an
//! index into the handler's formulas) and which of its flags are still
//! pending (`lzm`). For every flag in `lzm` the true value is the formula's,
//! not `fl`'s bit; every other flag is in `fl`/`fd` as usual. So:
//!
//! - a `LazyFlags` statement first settles any pending flag it does not
//!   rewrite (the slots are about to change), then captures its inputs and
//!   sets `lzk` and `lzm`;
//! - an eager write clears the flag from `lzm`;
//! - anything that reads flags (`ReadFlag`, `ReadEflags`, `Repeat`, the
//!   spill before an atomic) settles what it reads first, except a
//!   `ReadFlag` where the pending formula is known: that computes its one
//!   flag from the slots (or takes `fl`'s bit if it is not pending at run
//!   time) and leaves everything pending;
//! - the shared epilogue settles everything still pending, so every exit
//!   (transfer, fault, stop, poll) spills exact flags and defined bits.
//!
//! Where the dataflow knows which formulas may be pending (a few, as where
//! a loop's back edge meets the way in), settling computes the flags in
//! place from the slots, in a `switch` on `lzk` when there is more than
//! one, and clang keeps only what is needed; elsewhere (the epilogue, merges
//! of many formulas) it calls the handler's `h2lz_<entry>`, a switch over
//! all its formulas that returns the updated EFLAGS. The defined bits of settled flags are set (lazy flags are
//! always defined values). Handlers are
//! entered with nothing pending (`lzm = 0`), so start selectors need
//! nothing. Each settle and clear is only emitted where a forward dataflow
//! over the handler says the flags may be pending. The optimiser only makes
//! flags lazy when their reads are `ReadFlag`s in the same block, so the hot
//! path captures inputs and computes the flags branches use, nothing more.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use ir::{Control, Dest, FlagFormula, IrFunction, IrInsn, Stmt, Target};

/// Every flag bit.
pub(crate) const ALL: u32 = 0x0cd5;

/// What a handler's lazy flags need.
pub(crate) struct Lazy {
    /// The handler's distinct formulas; `lzk` is an index into this.
    pub formulas: Vec<Arc<FlagFormula>>,
    index: HashMap<FlagFormula, u32>,
    /// Slots: the most inputs any formula captures.
    pub slots: usize,
    /// What may be pending on entry to each block.
    pub entry: BTreeMap<u32, Pend>,
}

/// Flags that may be pending at a point, and which formulas they may be
/// from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Pend {
    pub mask: u32,
    /// Bit `k` for formula `k` (below 63); bit 63 for any other.
    pub kinds: u64,
}

/// Most formulas a settle or read handles in place, as a `switch` on
/// `lzk`; more than that calls the handler's `h2lz_` function.
const INLINE_KINDS: u32 = 3;

impl Pend {
    pub fn of(mask: u32, kind: u32) -> Pend {
        Pend { mask, kinds: 1 << kind.min(63) }
    }

    pub fn join(self, o: Pend) -> Pend {
        Pend { mask: self.mask | o.mask, kinds: self.kinds | o.kinds }
    }

    /// Without the flags in `m`.
    pub fn without(self, m: u32) -> Pend {
        let mask = self.mask & !m;
        Pend { mask, kinds: if mask == 0 { 0 } else { self.kinds } }
    }

    /// The formulas the pending flags may be from, if few enough (and all
    /// known) to handle in place.
    pub fn known(&self) -> Option<Vec<u32>> {
        let ok = self.kinds & (1 << 63) == 0 && self.kinds.count_ones() <= INLINE_KINDS;
        ok.then(|| (0..63).filter(|k| self.kinds & (1 << k) != 0).collect())
    }
}

impl Lazy {
    /// `None` when `f` has no lazy flags.
    pub fn of(f: &IrFunction) -> Option<Lazy> {
        let mut formulas = Vec::new();
        let mut index = HashMap::new();
        let mut slots = 0;
        fn walk(list: &[Stmt], out: &mut Vec<Arc<FlagFormula>>, index: &mut HashMap<FlagFormula, u32>) {
            for s in list {
                match s {
                    Stmt::LazyFlags(l) => {
                        if !index.contains_key(&*l.formula) {
                            index.insert((*l.formula).clone(), out.len() as u32);
                            out.push(l.formula.clone());
                        }
                    }
                    Stmt::If { then, otherwise, .. } => {
                        walk(then, out, index);
                        walk(otherwise, out, index);
                    }
                    Stmt::Repeat { body, .. } => walk(body, out, index),
                    _ => {}
                }
            }
        }
        for i in f.insns() {
            walk(&i.stmts, &mut formulas, &mut index);
        }
        if formulas.is_empty() {
            return None;
        }
        for fm in &formulas {
            slots = slots.max(fm.params.len());
        }
        let mut lazy = Lazy { formulas, index, slots, entry: BTreeMap::new() };
        lazy.entry = lazy.dataflow(f);
        Some(lazy)
    }

    pub fn kind(&self, fm: &FlagFormula) -> u32 {
        self.index[fm]
    }

    /// What may be pending at each block entry, to a fixed point. Blocks
    /// start unreached (nothing pending, formula unknown only once two
    /// different ones meet).
    fn dataflow(&self, f: &IrFunction) -> BTreeMap<u32, Pend> {
        let mut entry: BTreeMap<u32, Pend> = f.blocks.keys().map(|&k| (k, Pend::default())).collect();
        loop {
            let mut changed = false;
            for (&start, b) in &f.blocks {
                let mut p = entry[&start];
                for i in &b.insns {
                    p = self.insn(i, p);
                }
                for s in successors(b.insns.last().unwrap()) {
                    if let Some(e) = entry.get_mut(&s) {
                        let j = e.join(p);
                        if j != *e {
                            *e = j;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                return entry;
            }
        }
    }

    /// What may be pending after one statement.
    pub fn stmt(&self, s: &Stmt, p: Pend) -> Pend {
        // A flag read from known formulas is computed in place and stays
        // pending; anything else that reads settles first.
        let inline = matches!(s, Stmt::ReadFlag { .. }) && p.known().is_some();
        let p = if inline { p } else { p.without(reads(s)) };
        match s {
            Stmt::WriteFlag { flag, .. } => p.without(1 << flag.eflags_bit()),
            Stmt::WriteEflags { mask, .. } => p.without(*mask),
            Stmt::LazyFlags(l) => Pend::of(l.formula.mask(), self.kind(&l.formula)),
            Stmt::If { then, otherwise, .. } => self.stmts(then, p).join(self.stmts(otherwise, p)),
            _ => p,
        }
    }

    fn stmts(&self, list: &[Stmt], mut p: Pend) -> Pend {
        for s in list {
            p = self.stmt(s, p);
        }
        p
    }

    /// After an instruction. An atomic one spills (reads) everything first.
    fn insn(&self, i: &IrInsn, p: Pend) -> Pend {
        if i.atomic.is_some() { Pend::default() } else { self.stmts(&i.stmts, p) }
    }
}

/// Local successors of a block's last instruction.
fn successors(last: &IrInsn) -> Vec<u32> {
    let local = |d: &Dest| (!d.tail).then_some(d.pc);
    match &last.control {
        Control::Next => vec![last.next_pc()],
        Control::Jump { target: Target::Direct(t), tail: false, .. } => vec![*t],
        Control::Branch { taken, fallthrough, .. } => {
            [local(taken), local(fallthrough)].into_iter().flatten().collect()
        }
        Control::Switch { cases, .. } => cases.iter().filter_map(local).collect(),
        _ => vec![],
    }
}

/// Flags a statement reads (and so must be settled first).
pub(crate) fn reads(s: &Stmt) -> u32 {
    match s {
        Stmt::ReadFlag { flag, .. } => 1 << flag.eflags_bit(),
        Stmt::ReadEflags { .. } | Stmt::Repeat { .. } => ALL,
        _ => 0,
    }
}
