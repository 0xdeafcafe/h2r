//! The sim tier: a second way to compile the same lifted IR, under the
//! looser contract of `docs/engine/sim-review.md` section 4.2. The image
//! built by [`crate::write_module`] stays the *reference tier*; this one is
//! for the functions the tick reaches. The experiment that measures it is
//! `docs/engine/sim-tier-experiment.md`.
//!
//! What changes against the reference contract:
//!
//! - **Signatures.** [`Tier::build`] recovers, per function, which
//!   registers, flags, XMM registers and x87 state it may read before
//!   writing (backward liveness, callees included) and which it may write,
//!   and the x87 TOP change from entry to return. The image is
//!   link-time optimised, so this is a dataflow fixpoint over the closure,
//!   not a calling-convention match. Functions it cannot settle are
//!   refused and stay on the reference tier: a call to one is a rare event.
//! - **Values.** Registers, flags and XMM registers are C locals, which
//!   clang turns into SSA values; dead flag and register writes go with
//!   them. The x87 stack is eight `double` locals indexed by the static
//!   TOP, so every index is a constant and the array is scalarised.
//! - **Calls.** A direct guest call is a C call. The guest's own return
//!   address is still pushed and popped (memory effects stay identical);
//!   the caller checks the popped target equals its continuation.
//! - **Memory.** One unchecked mapping: an access is `base + address`, the
//!   base a parameter. No page table, gate, accounting or dirty marking.
//! - **Floats.** SSE arithmetic is host binary32; a NaN result is a rare
//!   event. x87 values are binary64 while they stay in the subset
//!   `docs/lifter/x87-native.md` proves exact at PC=53 (zero, or normal with
//!   exponent in [-400, 400]); PE and C1 are rebuilt exactly. Anything else
//!   (a NaN, a denormal operand, a division by zero, a stack fault) is a
//!   rare event.
//! - **Rare events.** A tier function returns a nonzero `H2T_RARE_*` code
//!   at once and every caller passes it up. The tier never continues past
//!   one, so it never computes with a value it cannot vouch for. The caller
//!   of the tier restores guest memory and re-runs the reference tier.
//!
//! Each compiled root gets an exported boundary function
//! (`h2tier_<entry>`, [`entry_symbol`]) with the signature
//! `uint32_t (CpuAbi *, uint8_t *base, uint32_t *rare_pc)`: it checks the
//! FP environment, moves the root's signature between `CpuAbi` and the
//! tier's state, and runs the root. MXCSR's sticky flags are not kept.

pub mod census;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ir::x87::{GUEST_CS, GUEST_DS, X87Arith, X87Const, X87Format, X87Op, X87Src, X87Unary};
use ir::{Control, Dest, Flag, FlagValue, Gpr, Intrinsic, IrFunction, IrInsn, RepKind, Segment, Slice, SseOp, Stmt, Target, Temp, Value, Width};

use crate::c::{expr, ty, val};

/// The C support code, pasted after `#include "h2gen.h"`.
pub const HEADER: &str = include_str!("../c/h2tier.h");

/// Resource bits of a [`Signature`]: GPRs are bits 0-7 ([`Gpr::index`]).
const FLAG_SHIFT: u32 = 8;
/// The seven modelled flags, in [`Flag::ALL`] order, start at bit 8.
pub const FLAGS: u32 = 0x7f << FLAG_SHIFT;
/// EFLAGS bits outside the seven modelled flags (IF, bit 1, ...).
pub const EFLAGS_REST: u32 = 1 << 15;
const XMM_SHIFT: u32 = 16;
/// XMM0-7.
pub const XMMS: u32 = 0xff << XMM_SHIFT;
/// The whole x87 state: registers, TOP, status, last-instruction pointers.
pub const X87: u32 = 1 << 24;
/// Every resource.
pub const ALL: u32 = 0xff | FLAGS | EFLAGS_REST | XMMS | X87;
/// EFLAGS bits the tier keeps as locals (`H2T_MODELLED`).
const MODELLED: u32 = 0xcd5;

const REG: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
const FLAG: [&str; 7] = ["cf", "pf", "af", "zf", "sf", "df", "of"];

fn gpr(g: Gpr) -> u32 {
    1 << g.index()
}

fn flag_index(f: Flag) -> usize {
    Flag::ALL.iter().position(|&x| x == f).expect("flag in Flag::ALL")
}

fn flag_bit(f: Flag) -> u32 {
    1 << (FLAG_SHIFT + flag_index(f) as u32)
}

fn xmm(r: u8) -> u32 {
    1 << (XMM_SHIFT + u32::from(r & 7))
}

/// The modelled flags an EFLAGS mask covers, as resource bits.
fn modelled(mask: u32) -> u32 {
    Flag::ALL.iter().filter(|f| mask & (1 << f.eflags_bit()) != 0).fold(0, |m, &f| m | flag_bit(f))
}

/// The functions the tier compiles: the roots and everything they reach
/// through direct calls and tail transfers.
#[derive(Debug, Default)]
pub struct Closure {
    pub functions: BTreeMap<u32, IrFunction>,
    /// Direct targets discovery has no function for.
    pub missing: BTreeSet<u32>,
    /// The walk stopped at its limit.
    pub truncated: bool,
}

/// Where control can leave a function for another.
#[derive(Debug, Default)]
pub struct Edges {
    pub calls: BTreeSet<u32>,
    pub tails: BTreeSet<u32>,
    /// Calls and jumps through a register or memory (switches excluded).
    pub indirect: Vec<u32>,
}

/// [`Edges`] of `f`.
pub fn edges(f: &IrFunction) -> Edges {
    let mut e = Edges::default();
    for i in f.insns() {
        match &i.control {
            Control::Call { target, continuation, .. } => {
                match target {
                    Target::Direct(t) => {
                        e.calls.insert(*t);
                    }
                    Target::Dynamic(_) => e.indirect.push(i.pc),
                }
                // A continuation that is another function's entry runs as a
                // tail transfer into that function (`crate::plan`).
                if !f.blocks.contains_key(continuation) {
                    e.tails.insert(*continuation);
                }
            }
            Control::Jump { target: Target::Direct(t), tail: true, .. } => {
                e.tails.insert(*t);
            }
            Control::Jump { target: Target::Dynamic(_), .. } => e.indirect.push(i.pc),
            Control::Branch { taken, fallthrough, .. } => {
                for d in [taken, fallthrough] {
                    if d.tail {
                        e.tails.insert(d.pc);
                    }
                }
            }
            Control::Switch { cases, .. } => {
                for d in cases.iter().filter(|d| d.tail) {
                    e.tails.insert(d.pc);
                }
            }
            _ => {}
        }
    }
    e
}

impl Closure {
    /// Lift `roots` and everything they reach by direct calls and tail
    /// transfers, up to `limit` functions.
    pub fn build(roots: &[u32], limit: usize, mut lift: impl FnMut(u32) -> Option<IrFunction>) -> Closure {
        let mut c = Closure::default();
        let mut seen: BTreeSet<u32> = roots.iter().copied().collect();
        let mut work: VecDeque<u32> = roots.iter().copied().collect();
        while let Some(pc) = work.pop_front() {
            if c.functions.len() >= limit {
                c.truncated = true;
                break;
            }
            let Some(f) = lift(pc) else {
                c.missing.insert(pc);
                continue;
            };
            let e = edges(&f);
            for t in e.calls.into_iter().chain(e.tails) {
                if seen.insert(t) {
                    work.push_back(t);
                }
            }
            c.functions.insert(pc, f);
        }
        c
    }
}

/// A recovered signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Signature {
    /// Resources the function may read before writing them, callees
    /// included (live in at entry).
    pub reads: u32,
    /// Resources it or a callee may write.
    pub writes: u32,
    /// Resources its own statements write (what it spills at an exit).
    pub own: u32,
    /// x87 TOP change from entry to every return (mod 8), `None` if the
    /// returns disagree or a callee's is unknown.
    pub x87_delta: Option<u8>,
    /// The `ret n` immediate, when every return agrees.
    pub cleanup: Option<u16>,
    /// The function's own statements touch x87 state.
    pub uses_x87: bool,
}

/// Why a function stays on the reference tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// An instruction form the tier has no exact lowering for.
    Unsupported { pc: u32, what: String },
    /// An x87 instruction whose stack depth is not static.
    X87Depth { pc: u32 },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unsupported { pc, what } => write!(f, "{what} at {pc:#010x}"),
            Refusal::X87Depth { pc } => write!(f, "x87 depth not static at {pc:#010x}"),
        }
    }
}

// ---- effects and liveness -------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
struct Effect {
    uses: u32,
    kills: u32,
    writes: u32,
}

impl Effect {
    fn read(&mut self, m: u32) {
        self.uses |= m & !self.kills;
    }
    fn write(&mut self, m: u32, sure: bool) {
        if sure {
            self.kills |= m;
        }
        self.writes |= m;
    }
    fn then(&mut self, next: Effect) {
        self.uses |= next.uses & !self.kills;
        self.kills |= next.kills;
        self.writes |= next.writes;
    }
}

fn touches_x87(op: Intrinsic) -> bool {
    matches!(
        op,
        Intrinsic::X87(_)
            | Intrinsic::X87ReadControl
            | Intrinsic::X87ReadStatus
            | Intrinsic::X87Wait
            | Intrinsic::MmxEnter
            | Intrinsic::Emms
    )
}

/// `sure` is false inside `If` and `Repeat` bodies: a write there may not
/// happen, so it does not end a register's liveness.
fn stmts_effect(list: &[Stmt], e: &mut Effect, sure: bool) {
    for s in list {
        match s {
            Stmt::ReadReg { reg, .. } => e.read(gpr(*reg)),
            Stmt::WriteReg { reg, slice, .. } => {
                // A partial write keeps the other bits: it reads them.
                if *slice != Slice::Full {
                    e.read(gpr(*reg));
                }
                e.write(gpr(*reg), sure && *slice == Slice::Full);
            }
            Stmt::ReadFlag { flag, .. } => e.read(flag_bit(*flag)),
            Stmt::WriteFlag { flag, .. } => e.write(flag_bit(*flag), sure),
            Stmt::ReadEflags { .. } => e.read(FLAGS | EFLAGS_REST),
            Stmt::WriteEflags { mask, .. } => {
                e.write(modelled(*mask), sure);
                e.read(EFLAGS_REST);
                e.write(EFLAGS_REST, false);
            }
            Stmt::ReadXmm { reg, .. } => e.read(xmm(*reg)),
            Stmt::WriteXmm { reg, .. } => e.write(xmm(*reg), sure),
            Stmt::ReadMmx { .. } | Stmt::WriteMmx { .. } => {
                e.read(X87);
                e.write(X87, false);
            }
            Stmt::If { then, otherwise, .. } => {
                stmts_effect(then, e, false);
                stmts_effect(otherwise, e, false);
            }
            Stmt::Repeat { kind, body } => {
                e.read(gpr(Gpr::Ecx));
                stmts_effect(body, e, false);
                if *kind != RepKind::Rep {
                    e.read(flag_bit(Flag::Zf));
                }
                e.write(gpr(Gpr::Ecx), false);
            }
            Stmt::Intrinsic { op, .. } if touches_x87(*op) => {
                e.read(X87);
                e.write(X87, false);
            }
            Stmt::LazyFlags(l) => {
                for (f, _) in &l.formula.flags {
                    e.write(flag_bit(*f), sure);
                }
            }
            _ => {}
        }
    }
}

fn callee_effect(e: &mut Effect, sigs: &BTreeMap<u32, Signature>, t: u32) {
    if let Some(s) = sigs.get(&t) {
        e.read(s.reads);
        e.write(s.writes, false);
    }
}

fn insn_effect(i: &IrInsn, f: &IrFunction, sigs: &BTreeMap<u32, Signature>) -> Effect {
    let mut e = Effect::default();
    stmts_effect(&i.stmts, &mut e, true);
    match &i.control {
        Control::Call { target, continuation, .. } => {
            match target {
                Target::Direct(t) => callee_effect(&mut e, sigs, *t),
                Target::Dynamic(_) => {
                    e.read(ALL);
                    e.write(ALL, false);
                }
            }
            if !f.blocks.contains_key(continuation) {
                callee_effect(&mut e, sigs, *continuation);
            }
        }
        Control::Jump { target: Target::Direct(t), tail: true, .. } => callee_effect(&mut e, sigs, *t),
        Control::Jump { target: Target::Dynamic(_), .. } => {
            e.read(ALL);
            e.write(ALL, false);
        }
        Control::Branch { taken, fallthrough, .. } => {
            for d in [taken, fallthrough] {
                if d.tail {
                    callee_effect(&mut e, sigs, d.pc);
                }
            }
        }
        Control::Switch { cases, .. } => {
            for d in cases.iter().filter(|d| d.tail) {
                callee_effect(&mut e, sigs, d.pc);
            }
        }
        _ => {}
    }
    e
}

/// Local successors of a block, call continuations included.
fn successors(f: &IrFunction, b: &ir::IrBlock) -> Vec<u32> {
    let mut out = crate::backedge::successors(b, &f.blocks);
    if let Some(Control::Call { continuation, .. }) = b.insns.last().map(|i| &i.control)
        && f.blocks.contains_key(continuation)
    {
        out.push(*continuation);
    }
    out
}

/// (live in at entry, everything written, written by own statements).
fn liveness(f: &IrFunction, sigs: &BTreeMap<u32, Signature>) -> (u32, u32, u32) {
    let mut own = Effect::default();
    for i in f.insns() {
        stmts_effect(&i.stmts, &mut own, true);
    }
    let effects: BTreeMap<u32, Effect> = f
        .blocks
        .iter()
        .map(|(&pc, b)| {
            let mut e = Effect::default();
            for i in &b.insns {
                e.then(insn_effect(i, f, sigs));
            }
            (pc, e)
        })
        .collect();
    let succ: BTreeMap<u32, Vec<u32>> = f.blocks.iter().map(|(&pc, b)| (pc, successors(f, b))).collect();
    let mut live: BTreeMap<u32, u32> = effects.iter().map(|(&pc, e)| (pc, e.uses)).collect();
    loop {
        let mut changed = false;
        for (&pc, e) in effects.iter().rev() {
            let out = succ[&pc].iter().map(|s| live.get(s).copied().unwrap_or(0)).fold(0, |a, b| a | b);
            let inn = e.uses | (out & !e.kills);
            if live[&pc] != inn {
                live.insert(pc, inn);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let writes = effects.values().fold(0, |a, e| a | e.writes);
    (live.get(&f.entry).copied().unwrap_or(0), writes, own.writes)
}

// ---- x87 stack depth -------------------------------------------------------

fn join(a: Option<u8>, b: Option<u8>) -> Option<u8> {
    if a == b { a } else { None }
}

fn add(t: Option<u8>, d: Option<u8>) -> Option<u8> {
    Some(t?.wrapping_add(d?) & 7)
}

/// The TOP change since entry at the start of every reachable instruction,
/// and at every way out of the function.
#[derive(Debug, Default)]
struct Tops {
    at: BTreeMap<u32, Option<u8>>,
    blocks: BTreeMap<u32, Option<u8>>,
    exits: BTreeSet<Option<u8>>,
}

fn tops(f: &IrFunction, sigs: &BTreeMap<u32, Signature>) -> Tops {
    // A call to anything outside the tier is a rare event: nothing after it
    // runs, so its delta is immaterial. Assume none.
    let delta = |t: u32| sigs.get(&t).map_or(Some(0), |s| s.x87_delta);
    let mut input: BTreeMap<u32, Option<u8>> = BTreeMap::from([(f.entry, Some(0))]);
    let mut work = VecDeque::from([f.entry]);
    let mut exits = BTreeSet::new();
    while let Some(pc) = work.pop_front() {
        let Some(b) = f.blocks.get(&pc) else { continue };
        let mut top = input[&pc];
        for i in &b.insns {
            top = i.stmts.iter().fold(top, crate::x87::top::after);
        }
        let mut next: Vec<(u32, Option<u8>)> =
            crate::backedge::successors(b, &f.blocks).into_iter().map(|s| (s, top)).collect();
        match b.insns.last().map(|i| &i.control) {
            Some(Control::Call { target, continuation, .. }) => {
                let after = match target {
                    Target::Direct(t) => add(top, delta(*t)),
                    Target::Dynamic(_) => top,
                };
                if f.blocks.contains_key(continuation) {
                    next.push((*continuation, after));
                } else {
                    exits.insert(add(after, delta(*continuation)));
                }
            }
            Some(Control::Return { .. }) => {
                exits.insert(top);
            }
            Some(Control::Jump { target: Target::Direct(t), tail: true, .. }) => {
                exits.insert(add(top, delta(*t)));
            }
            Some(Control::Branch { taken, fallthrough, .. }) => {
                for d in [taken, fallthrough] {
                    if d.tail {
                        exits.insert(add(top, delta(d.pc)));
                    }
                }
            }
            Some(Control::Switch { cases, .. }) => {
                for d in cases.iter().filter(|d| d.tail) {
                    exits.insert(add(top, delta(d.pc)));
                }
            }
            _ => {}
        }
        for (s, t) in next {
            let merged = input.get(&s).map_or(t, |&prev| join(prev, t));
            if input.insert(s, merged) != Some(merged) {
                work.push_back(s);
            }
        }
    }
    let mut at = BTreeMap::new();
    for (pc, &start) in &input {
        let Some(b) = f.blocks.get(pc) else { continue };
        let mut top = start;
        for i in &b.insns {
            at.insert(i.pc, top);
            top = i.stmts.iter().fold(top, crate::x87::top::after);
        }
    }
    Tops { at, blocks: input, exits }
}

// ---- what the tier can lower -----------------------------------------------

fn x87_supported(op: X87Op) -> bool {
    match op {
        X87Op::Load(f) => matches!(f, X87Format::F32 | X87Format::F64 | X87Format::I16 | X87Format::I32 | X87Format::I64),
        X87Op::LoadConst(c) => matches!(c, X87Const::One | X87Const::Z),
        X87Op::StoreConvert { to, .. } => matches!(to, X87Format::F32 | X87Format::F64),
        X87Op::Unary(u) => matches!(u, X87Unary::Sqrt | X87Unary::Abs | X87Unary::Chs),
        X87Op::LoadReg(_)
        | X87Op::StoreCommit { .. }
        | X87Op::StoreReg { .. }
        | X87Op::Arith { .. }
        | X87Op::Cmp { .. }
        | X87Op::Xch(_)
        | X87Op::Mark { .. } => true,
        _ => false,
    }
}

fn refusal_in(list: &[Stmt]) -> Option<String> {
    for s in list {
        let what = match s {
            Stmt::Unsupported { mnemonic, .. } => Some(format!("unsupported {mnemonic}")),
            Stmt::LazyFlags(_) => Some("lazy flags".into()),
            Stmt::ReadMmx { .. } | Stmt::WriteMmx { .. } => Some("mmx".into()),
            Stmt::Load { dst, .. } if dst.width == Width::W80 => Some("80-bit load".into()),
            Stmt::Store { value, .. } if value.width() == Width::W80 => Some("80-bit store".into()),
            Stmt::Intrinsic { op, .. } => match op {
                Intrinsic::MxcsrRead | Intrinsic::MxcsrWrite => Some("ldmxcsr/stmxcsr".into()),
                Intrinsic::MmxEnter | Intrinsic::Emms => Some("mmx".into()),
                Intrinsic::ReadTsc => Some("rdtsc".into()),
                Intrinsic::Sse(SseOp::Cvtpi2ps | SseOp::Cvtps2pi | SseOp::Cvttps2pi) => Some("mmx conversion".into()),
                Intrinsic::X87(x) if !x87_supported(*x) => Some(format!("x87 {x:?}")),
                _ => None,
            },
            Stmt::If { then, otherwise, .. } => refusal_in(then).or_else(|| refusal_in(otherwise)),
            Stmt::Repeat { body, .. } => refusal_in(body),
            _ => None,
        };
        if what.is_some() {
            return what;
        }
    }
    None
}

fn static_refusal(f: &IrFunction) -> Option<Refusal> {
    f.insns().find_map(|i| refusal_in(&i.stmts).map(|what| Refusal::Unsupported { pc: i.pc, what }))
}

fn any_stmt(list: &[Stmt], p: &impl Fn(&Stmt) -> bool) -> bool {
    list.iter().any(|s| {
        p(s) || match s {
            Stmt::If { then, otherwise, .. } => any_stmt(then, p) || any_stmt(otherwise, p),
            Stmt::Repeat { body, .. } => any_stmt(body, p),
            _ => false,
        }
    })
}

fn uses_x87(f: &IrFunction) -> bool {
    f.insns().any(|i| any_stmt(&i.stmts, &|s: &Stmt| matches!(s, Stmt::Intrinsic { op, .. } if touches_x87(*op))))
}

fn uses_fp(f: &IrFunction) -> bool {
    f.insns().any(|i| {
        any_stmt(&i.stmts, &|s: &Stmt| matches!(s, Stmt::Intrinsic { op, .. } if touches_x87(*op) || matches!(op, Intrinsic::Sse(_))))
    })
}

fn cleanup(f: &IrFunction) -> Option<u16> {
    let mut seen = f.insns().filter_map(|i| match &i.control {
        Control::Return { cleanup, .. } => Some(*cleanup),
        _ => None,
    });
    let first = seen.next()?;
    seen.all(|c| c == first).then_some(first)
}

// ---- the tier --------------------------------------------------------------

/// The result of [`Tier::build`]: what compiles, what does not and why,
/// and every recovered signature.
#[derive(Debug, Default)]
pub struct Tier {
    pub signatures: BTreeMap<u32, Signature>,
    pub refused: BTreeMap<u32, Refusal>,
}

impl Tier {
    /// Settle refusals and signatures over `closure`.
    pub fn build(closure: &Closure) -> Tier {
        let mut refused: BTreeMap<u32, Refusal> = BTreeMap::new();
        for (&pc, f) in &closure.functions {
            if let Some(r) = static_refusal(f) {
                refused.insert(pc, r);
            }
        }
        loop {
            let sigs = signatures(closure, &refused);
            let mut more = Vec::new();
            for (&pc, f) in &closure.functions {
                if refused.contains_key(&pc) || !uses_x87(f) {
                    continue;
                }
                // Only an x87 instruction needs a static depth; a spill or
                // reload at an unknown depth is emitted as a rare event.
                let t = tops(f, &sigs);
                let at = f.insns().find(|i| {
                    t.at.get(&i.pc).is_some_and(|top| top.is_none())
                        && any_stmt(&i.stmts, &|s: &Stmt| matches!(s, Stmt::Intrinsic { op, .. } if touches_x87(*op)))
                });
                if let Some(i) = at {
                    more.push((pc, Refusal::X87Depth { pc: i.pc }));
                }
            }
            if more.is_empty() {
                return Tier { signatures: sigs, refused };
            }
            refused.extend(more);
        }
    }

    pub fn compiled(&self) -> impl Iterator<Item = u32> + '_ {
        self.signatures.keys().copied()
    }
}

/// The signature fixpoint over the functions not refused.
fn signatures(closure: &Closure, refused: &BTreeMap<u32, Refusal>) -> BTreeMap<u32, Signature> {
    let live: Vec<(&u32, &IrFunction)> = closure.functions.iter().filter(|(pc, _)| !refused.contains_key(*pc)).collect();
    let mut sigs: BTreeMap<u32, Signature> = live
        .iter()
        .map(|&(&pc, f)| (pc, Signature { x87_delta: Some(0), cleanup: cleanup(f), uses_x87: uses_x87(f), ..Signature::default() }))
        .collect();
    // Reads and writes only grow; a delta that changes twice in a row is
    // not settling and becomes unknown.
    for round in 0..64 {
        let mut changed = false;
        for &(&pc, f) in &live {
            let (reads, writes, own) = liveness(f, &sigs);
            let t = tops(f, &sigs);
            let mut delta = match t.exits.len() {
                0 => Some(0),
                1 => *t.exits.iter().next().unwrap(),
                _ => None,
            };
            let old = sigs[&pc];
            if round > 32 && delta != old.x87_delta {
                delta = None;
            }
            let new = Signature { reads: reads | old.reads, writes: writes | old.writes, own, x87_delta: delta, ..old };
            if new != old {
                sigs.insert(pc, new);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    sigs
}

/// The handler symbol of a tier function.
pub fn symbol(entry: u32) -> String {
    format!("h2t_{entry:08x}")
}

/// The exported boundary function for a root:
/// `uint32_t h2tier_<entry>(CpuAbi *, uint8_t *base, uint32_t *rare_pc)`.
pub fn entry_symbol(root: u32) -> String {
    format!("h2tier_{root:08x}")
}

/// The generated C, and what went into it.
#[derive(Debug)]
pub struct Module {
    /// One C source: `#include "h2gen.h"`, [`HEADER`], the functions and the
    /// boundary functions. Compile it next to a module written by
    /// [`crate::write_module`] (for the support headers), for example as an
    /// extra source of [`crate::compile::host_dylib`].
    pub c: String,
    pub tier: Tier,
    /// The roots that got a boundary function.
    pub roots: Vec<u32>,
}

/// Write the tier for `roots` over `closure`.
pub fn write(closure: &Closure, roots: &[u32]) -> Module {
    let tier = Tier::build(closure);
    let mut c = String::from(
        "/* Generated by crates/emit/src/sim.rs from a user-supplied game image.\n * Derived from the disc: never edit, never commit. */\n#include \"h2gen.h\"\n",
    );
    // Every function loads every local; most go unused.
    c.push_str("#pragma clang diagnostic ignored \"-Wunused-variable\"\n#pragma clang diagnostic ignored \"-Wunused-label\"\n#pragma clang diagnostic ignored \"-Wunused-but-set-variable\"\n");
    c.push_str(HEADER);
    c.push('\n');
    for pc in tier.compiled() {
        let s = &tier.signatures[&pc];
        c.push_str(&format!(
            "/* reads {:#09x} writes {:#09x} x87 {:?} ret {:?} */\nstatic uint32_t {}(H2TState *restrict s, uint8_t *restrict base);\n",
            s.reads,
            s.writes,
            s.x87_delta,
            s.cleanup,
            symbol(pc)
        ));
    }
    c.push_str("static uint32_t h2t_dispatch(H2TState *restrict s, uint8_t *restrict base, uint32_t target, uint32_t pc) {\n    switch (target) {\n");
    for pc in tier.compiled() {
        c.push_str(&format!("    case {pc:#010x}u: return {}(s, base);\n", symbol(pc)));
    }
    c.push_str("    default: H2T_RARE(H2T_RARE_CALL, pc);\n    }\n}\n\n");
    let mut any_fp = false;
    for pc in tier.compiled() {
        let f = &closure.functions[&pc];
        any_fp |= uses_fp(f);
        let t = tops(f, &tier.signatures);
        let mut e = Emit { out: String::new(), f, sigs: &tier.signatures, tops: &t, sig: tier.signatures[&pc] };
        e.function();
        c.push_str(&e.out);
        c.push('\n');
    }
    let mut written = Vec::new();
    for &root in roots {
        let Some(sig) = tier.signatures.get(&root) else { continue };
        c.push_str(&boundary(root, sig, any_fp));
        written.push(root);
    }
    Module { c, tier, roots: written }
}

/// The exported boundary function for `root`.
fn boundary(root: u32, sig: &Signature, fp: bool) -> String {
    let load = sig.reads | sig.writes;
    let mut s = format!(
        "H2_EXPORT uint32_t {}(CpuAbi *cpu, uint8_t *base, uint32_t *rare_pc) {{\n    H2TState st_;\n    H2TState *s = &st_;\n",
        entry_symbol(root)
    );
    if fp {
        s.push_str("    if (!h2t_sse_mode(cpu)) return H2T_RARE_ENTRY;\n");
    }
    if load & X87 != 0 {
        s.push_str("    if (!h2t_x87_enter(s, &cpu->x87)) return H2T_RARE_ENTRY;\n");
    }
    for (i, r) in REG.iter().enumerate() {
        if load & (1 << i) != 0 {
            s.push_str(&format!("    s->r[{i}] = cpu->{r};\n"));
        }
    }
    if load & (FLAGS | EFLAGS_REST) != 0 {
        s.push_str("    h2t_flags_enter(s, cpu);\n");
    }
    for i in 0..8u8 {
        if load & xmm(i) != 0 {
            s.push_str(&format!("    memcpy(&s->x[{i}], cpu->xmm[{i}], 16);\n"));
        }
    }
    s.push_str(&format!(
        "    s->fs_base = cpu->fs_base;\n    s->ret = 0;\n    s->rare_pc = 0;\n    uint32_t rc = {}(s, base);\n    if (rc) {{\n        if (rare_pc) *rare_pc = s->rare_pc;\n        return rc;\n    }}\n",
        symbol(root)
    ));
    for (i, r) in REG.iter().enumerate() {
        if sig.writes & (1 << i) != 0 {
            s.push_str(&format!("    cpu->{r} = s->r[{i}];\n"));
        }
    }
    if sig.writes & (FLAGS | EFLAGS_REST) != 0 {
        s.push_str("    h2t_flags_leave(s, cpu);\n");
    }
    for i in 0..8u8 {
        if sig.writes & xmm(i) != 0 {
            s.push_str(&format!("    memcpy(cpu->xmm[{i}], &s->x[{i}], 16);\n"));
        }
    }
    if sig.writes & X87 != 0 {
        s.push_str("    h2t_x87_leave(s, &cpu->x87);\n");
    }
    s.push_str("    cpu->eip = s->ret;\n    return H2T_OK;\n}\n\n");
    s
}

/// One function's C.
struct Emit<'a> {
    out: String,
    f: &'a IrFunction,
    sigs: &'a BTreeMap<u32, Signature>,
    tops: &'a Tops,
    sig: Signature,
}

fn width_bits(w: Width) -> u32 {
    match w {
        Width::W1 | Width::W8 => 8,
        w => w.bits(),
    }
}

impl Emit<'_> {
    fn line(&mut self, depth: usize, s: impl AsRef<str>) {
        for _ in 0..depth {
            self.out.push_str("    ");
        }
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }

    fn x87(&self) -> bool {
        self.sig.uses_x87
    }

    fn function(&mut self) {
        let f = self.f;
        self.line(0, format!("static uint32_t {}(H2TState *restrict s, uint8_t *restrict base) {{", symbol(f.entry)));
        let regs: Vec<String> = REG.iter().enumerate().map(|(i, r)| format!("{r} = s->r[{i}]")).collect();
        self.line(1, format!("uint32_t {};", regs.join(", ")));
        let flags: Vec<String> = FLAG.iter().enumerate().map(|(i, n)| format!("f_{n} = s->f[{i}], d_{n} = s->d[{i}]")).collect();
        self.line(1, format!("uint8_t {};", flags.join(", ")));
        self.line(1, "uint32_t efr = s->efr, fdr = s->fdr;");
        let xmms: Vec<String> = (0..8).map(|i| format!("x{i} = s->x[{i}]")).collect();
        self.line(1, format!("h2u128 {};", xmms.join(", ")));
        if self.x87() {
            self.line(1, "uint32_t ftop0 = s->ftop;");
            self.line(1, "double fx[8];");
            self.line(1, "uint8_t fv[8], fw[8];");
            self.line(1, "uint16_t fsw = s->fsw, fcw = s->fcw, fop = s->fop, fcs = s->fcs, fds = s->fds;");
            self.line(1, "uint32_t fip = s->fip, fdp = s->fdp;");
            self.line(1, X87_LOAD);
        }
        self.line(1, format!("goto L_{:08x};", f.entry));
        let reachable: Vec<u32> = self.tops.blocks.keys().copied().filter(|pc| f.blocks.contains_key(pc)).collect();
        for pc in reachable {
            let b = &f.blocks[&pc];
            self.line(0, format!("L_{pc:08x}:;"));
            let n = b.insns.len();
            for (k, i) in b.insns.iter().enumerate() {
                self.insn(i, k + 1 == n);
            }
        }
        // Every block ends in a transfer; this is never reached.
        self.line(1, Self::rare("STOP", f.entry));
        self.line(0, "}");
    }

    fn insn(&mut self, i: &IrInsn, last: bool) {
        let top = self.tops.at.get(&i.pc).copied().flatten();
        self.line(1, format!("{{ /* {:#010x} */", i.pc));
        let end = self.stmts(&i.stmts, i, 2, top);
        self.control(i, last, end);
        self.line(1, "}");
    }

    fn stmts(&mut self, list: &[Stmt], insn: &IrInsn, d: usize, mut top: Option<u8>) -> Option<u8> {
        for s in list {
            self.stmt(s, insn, d, top);
            top = crate::x87::top::after(top, s);
        }
        top
    }

    fn rare(code: &str, pc: u32) -> String {
        format!("H2T_RARE(H2T_RARE_{code}, {pc:#010x}u);")
    }

    fn addr(a: &ir::Addr) -> String {
        match a.seg {
            Segment::Flat => val(&a.offset),
            Segment::Fs => format!("(uint32_t)(s->fs_base + {})", val(&a.offset)),
        }
    }

    fn stmt(&mut self, s: &Stmt, insn: &IrInsn, d: usize, top: Option<u8>) {
        let pc = insn.pc;
        match s {
            Stmt::Assign { dst, expr: e } => {
                let rhs = expr(*dst, e);
                self.line(d, format!("{} t{} = {rhs};", ty(dst.width), dst.id));
            }
            Stmt::ReadReg { dst, reg, slice } => {
                let r = REG[reg.index()];
                let rhs = match slice {
                    Slice::Full => r.to_string(),
                    Slice::Low16 => format!("(uint16_t){r}"),
                    Slice::Low8 => format!("(uint8_t){r}"),
                    Slice::High8 => format!("(uint8_t)({r} >> 8)"),
                };
                self.line(d, format!("{} t{} = {rhs};", ty(dst.width), dst.id));
            }
            Stmt::WriteReg { reg, slice, value } => {
                let r = REG[reg.index()];
                let v = val(value);
                self.line(
                    d,
                    match slice {
                        Slice::Full => format!("{r} = {v};"),
                        Slice::Low16 => format!("{r} = ({r} & 0xffff0000u) | (uint32_t){v};"),
                        Slice::Low8 => format!("{r} = ({r} & 0xffffff00u) | (uint32_t){v};"),
                        Slice::High8 => format!("{r} = ({r} & 0xffff00ffu) | ((uint32_t){v} << 8);"),
                    },
                );
            }
            Stmt::ReadFlag { dst, flag } => {
                self.line(d, format!("uint8_t t{} = f_{};", dst.id, FLAG[flag_index(*flag)]));
            }
            Stmt::WriteFlag { flag, value } => {
                let n = FLAG[flag_index(*flag)];
                match value {
                    FlagValue::Defined(v) => self.line(d, format!("f_{n} = (uint8_t)({} & 1u); d_{n} = 1;", val(v))),
                    // Normative: an undefined flag stores 0 and is not defined.
                    FlagValue::Undefined => self.line(d, format!("f_{n} = 0; d_{n} = 0;")),
                }
            }
            Stmt::ReadEflags { dst } => {
                let bits: String = Flag::ALL
                    .iter()
                    .enumerate()
                    .map(|(i, f)| format!(" | ((uint32_t)f_{} << {})", FLAG[i], f.eflags_bit()))
                    .collect();
                self.line(d, format!("uint32_t t{} = efr | 2u{bits};", dst.id));
            }
            Stmt::WriteEflags { value, mask } => {
                let v = val(value);
                for (i, f) in Flag::ALL.iter().enumerate() {
                    let b = f.eflags_bit();
                    if mask & (1 << b) != 0 {
                        self.line(d, format!("f_{n} = (uint8_t)(({v} >> {b}) & 1u); d_{n} = 1;", n = FLAG[i]));
                    }
                }
                let rest = mask & !MODELLED;
                if rest != 0 {
                    self.line(d, format!("efr = (efr & ~{rest:#x}u) | ({v} & {rest:#x}u); fdr |= {rest:#x}u;"));
                }
                self.line(d, "efr |= 2u;");
            }
            Stmt::ReadXmm { dst, reg } => self.line(d, format!("h2u128 t{} = x{};", dst.id, reg & 7)),
            Stmt::WriteXmm { reg, value } => self.line(d, format!("x{} = {};", reg & 7, val(value))),
            Stmt::Load { dst, addr } => {
                let a = Self::addr(addr);
                self.line(d, format!("{} t{} = h2t_ld{}(base, {a});", ty(dst.width), dst.id, width_bits(dst.width)));
            }
            Stmt::Store { addr, value } => {
                let a = Self::addr(addr);
                self.line(d, format!("h2t_st{}(base, {a}, {});", width_bits(value.width()), val(value)));
            }
            Stmt::FaultIf { cond, .. } => self.line(d, format!("if ({}) {}", val(cond), Self::rare("FAULT", pc))),
            Stmt::StopIf { cond, .. } => self.line(d, format!("if ({}) {}", val(cond), Self::rare("STOP", pc))),
            Stmt::If { cond, then, otherwise } => {
                self.line(d, format!("if ({}) {{", val(cond)));
                self.stmts(then, insn, d + 1, top);
                if !otherwise.is_empty() {
                    self.line(d, "} else {");
                    self.stmts(otherwise, insn, d + 1, top);
                }
                self.line(d, "}");
            }
            Stmt::Repeat { kind, body } => {
                self.line(d, "while (ecx != 0u) {");
                self.stmts(body, insn, d + 1, top);
                self.line(d + 1, "ecx = ecx - 1u;");
                match kind {
                    RepKind::Rep => {}
                    RepKind::RepE => self.line(d + 1, "if (!f_zf) break;"),
                    RepKind::RepNe => self.line(d + 1, "if (f_zf) break;"),
                }
                self.line(d, "}");
            }
            Stmt::Intrinsic { op, args, dsts } => self.intrinsic(*op, args, dsts, pc, top, d),
            // Refused before emission ([`refusal_in`]); never reached.
            Stmt::LazyFlags(_) | Stmt::Unsupported { .. } | Stmt::ReadMmx { .. } | Stmt::WriteMmx { .. } => {
                self.line(d, Self::rare("STOP", pc));
            }
        }
    }

    fn intrinsic(&mut self, op: Intrinsic, args: &[Value], dsts: &[Temp], pc: u32, top: Option<u8>, d: usize) {
        match op {
            Intrinsic::Sse(o) => self.sse(o, args, dsts, pc, d),
            Intrinsic::X87Wait => self.line(d, "/* fwait: nothing can be pending under the tick's control word */"),
            Intrinsic::X87ReadControl => self.line(d, format!("uint16_t t{} = fcw;", dsts[0].id)),
            Intrinsic::X87ReadStatus => {
                let rt = top.expect("x87 depth is static in a compiled function");
                self.line(d, format!("uint16_t t{} = (uint16_t)((fsw & ~0x3800u) | (((ftop0 + {rt}u) & 7u) << 11));", dsts[0].id));
            }
            Intrinsic::X87(x) => {
                let rt = top.expect("x87 depth is static in a compiled function");
                self.x87_op(x, args, dsts, pc, rt, d);
            }
            _ => self.line(d, Self::rare("STOP", pc)),
        }
    }

    fn sse(&mut self, o: SseOp, args: &[Value], dsts: &[Temp], pc: u32, d: usize) {
        use SseOp::*;
        let a = |i: usize| val(&args[i]);
        let t = format!("t{}", dsts[0].id);
        let rare = Self::rare("FP", pc);
        let line = match o {
            AddSs | SubSs | MulSs | DivSs | AddPs | SubPs | MulPs | DivPs | SqrtSs => {
                format!("h2u128 {t}; if (!h2t_{}(&{t}, {}, {})) {rare}", o.name(), a(0), a(1))
            }
            SqrtPs => format!("h2u128 {t}; if (!h2t_sqrtps(&{t}, {})) {rare}", a(0)),
            MinSs | MaxSs | MinPs | MaxPs => format!("h2u128 {t} = h2t_{}({}, {});", o.name(), a(0), a(1)),
            RcpSs | RsqrtSs => format!("h2u128 {t} = h2s_{}({}, {});", o.name(), a(0), a(1)),
            RcpPs | RsqrtPs => format!("h2u128 {t} = h2s_{}({});", o.name(), a(0)),
            CmpSs | CmpPs => format!("h2u128 {t} = h2t_{}({}, {}, {});", o.name(), a(0), a(1), a(2)),
            Comiss | Ucomiss => format!("uint32_t {t} = h2t_comi({}, {});", a(0), a(1)),
            Cvtsi2ss => format!("h2u128 {t} = h2t_cvtsi2ss({}, {});", a(0), a(1)),
            Cvtss2si | Cvttss2si => format!("uint32_t {t} = h2t_{}({});", o.name(), a(0)),
            // Refused before emission.
            Cvtpi2ps | Cvtps2pi | Cvttps2pi => Self::rare("STOP", pc),
        };
        self.line(d, line);
    }

    /// One x87 op at static depth `rt`: ST(i) is local slot `(rt + i) & 7`.
    fn x87_op(&mut self, op: X87Op, args: &[Value], dsts: &[Temp], pc: u32, rt: u8, d: usize) {
        let st = |i: u8| rt.wrapping_add(i) & 7;
        let s0 = st(0);
        let push = rt.wrapping_add(7) & 7;
        let rare = Self::rare("X87", pc);
        let a = |i: usize| val(&args[i]);
        let t = |i: usize| format!("t{}", dsts[i].id);
        // `b_ = ` the source operand of an arithmetic or a compare.
        let source = |src: X87Src| -> String {
            match src {
                X87Src::St(j) => format!("if (!fv[{}]) {rare} b_ = fx[{}];", st(j), st(j)),
                X87Src::F32 => format!("if (!h2t_x87_f32({}, &b_)) {rare}", a(0)),
                X87Src::F64 => format!("if (!h2t_x87_f64({}, &b_)) {rare}", a(0)),
                X87Src::I16 => format!("b_ = (double)(int16_t){};", a(0)),
                X87Src::I32 => format!("b_ = (double)(int32_t){};", a(0)),
            }
        };
        let pushed = |value: String, from: Option<u8>| -> String {
            let live = from.map_or(String::new(), |s| format!("!fv[{s}] || "));
            format!(
                "{{ double v_ = 0.0; {value} if ({live}fv[{push}]) {rare} fx[{push}] = v_; fv[{push}] = 1; fw[{push}] = 1; fsw &= (uint16_t)~0x200u; }}"
            )
        };
        match op {
            X87Op::Load(fmt) => {
                let value = match fmt {
                    X87Format::F32 => format!("if (!h2t_x87_f32({}, &v_)) {rare}", a(0)),
                    X87Format::F64 => format!("if (!h2t_x87_f64({}, &v_)) {rare}", a(0)),
                    X87Format::I16 => format!("v_ = (double)(int16_t){};", a(0)),
                    X87Format::I32 => format!("v_ = (double)(int32_t){};", a(0)),
                    // FILD m64 holds 64 bits; only |v| <= 2^53 is binary64.
                    X87Format::I64 => format!(
                        "{{ int64_t i_ = (int64_t){}; if (i_ > INT64_C(9007199254740992) || i_ < -INT64_C(9007199254740992)) {rare} v_ = (double)i_; }}",
                        a(0)
                    ),
                    X87Format::F80 | X87Format::Bcd => Self::rare("STOP", pc),
                };
                self.line(d, pushed(value, None));
            }
            X87Op::LoadConst(c) => {
                let value = match c {
                    X87Const::One => "v_ = 1.0;".to_string(),
                    X87Const::Z => "v_ = 0.0;".to_string(),
                    _ => Self::rare("STOP", pc),
                };
                self.line(d, pushed(value, None));
            }
            X87Op::LoadReg(i) => {
                let from = st(i);
                self.line(d, pushed(format!("v_ = fx[{from}];"), Some(from)));
            }
            X87Op::StoreConvert { to, src } => {
                let s = st(src);
                let (b, sw, w) = (t(0), t(1), t(2));
                match to {
                    X87Format::F32 => self.line(
                        d,
                        format!("uint32_t {b} = 0; uint16_t {sw} = 0; uint8_t {w} = 1; if (!fv[{s}] || !h2t_x87_st_f32(fx[{s}], fsw, &{b}, &{sw})) {rare}"),
                    ),
                    X87Format::F64 => self.line(
                        d,
                        format!("uint64_t {b} = 0; uint16_t {sw} = 0; uint8_t {w} = 1; if (!fv[{s}]) {rare} {b} = h2t_bits64(fx[{s}]); {sw} = (uint16_t)(fsw & ~0x200u);"),
                    ),
                    _ => self.line(d, Self::rare("STOP", pc)),
                }
            }
            X87Op::StoreCommit { pop } => {
                self.line(d, format!("if (!{}) {rare} fsw = (uint16_t)({} & ~0x3800u);", a(1), a(0)));
                if pop {
                    self.line(d, format!("fv[{s0}] = 0;"));
                }
            }
            X87Op::StoreReg { dst, pop } => {
                let to = st(dst);
                self.line(d, format!("if (!fv[{s0}]) {rare} fx[{to}] = fx[{s0}]; fv[{to}] = 1; fw[{to}] = 1; fsw &= (uint16_t)~0x200u;"));
                if pop {
                    self.line(d, format!("fv[{s0}] = 0;"));
                }
            }
            X87Op::Arith { op: kind, dst, src, pop } => {
                let n = match kind {
                    X87Arith::Add => 0,
                    X87Arith::Sub => 1,
                    X87Arith::SubR => 2,
                    X87Arith::Mul => 3,
                    X87Arith::Div => 4,
                    X87Arith::DivR => 5,
                };
                let to = st(dst);
                self.line(
                    d,
                    format!(
                        "{{ double b_ = 0.0; {} if (!fv[{to}] || !h2t_x87_arith({n}u, fx[{to}], b_, &fx[{to}], &fsw)) {rare} fw[{to}] = 1; }}",
                        source(src)
                    ),
                );
                if pop {
                    self.line(d, format!("fv[{s0}] = 0;"));
                }
            }
            X87Op::Unary(u) => match u {
                X87Unary::Sqrt => {
                    self.line(d, format!("if (!fv[{s0}] || !h2t_x87_sqrt(fx[{s0}], &fx[{s0}], &fsw)) {rare} fw[{s0}] = 1;"))
                }
                X87Unary::Abs => self.line(
                    d,
                    format!("if (!fv[{s0}]) {rare} fx[{s0}] = __builtin_fabs(fx[{s0}]); fw[{s0}] = 1; fsw &= (uint16_t)~0x200u;"),
                ),
                X87Unary::Chs => {
                    self.line(d, format!("if (!fv[{s0}]) {rare} fx[{s0}] = -fx[{s0}]; fw[{s0}] = 1; fsw &= (uint16_t)~0x200u;"))
                }
                _ => self.line(d, Self::rare("STOP", pc)),
            },
            X87Op::Cmp { src, pop, to_flags, .. } => {
                let (payload, suppressed) = (t(0), t(1));
                let result = if to_flags {
                    format!("{payload} = h2t_x87_eflags(r_);")
                } else {
                    format!("{payload} = h2t_x87_cc(r_); fsw = (uint16_t)((fsw & ~0x4500u) | {payload});")
                };
                self.line(
                    d,
                    format!(
                        "uint32_t {payload} = 0; uint8_t {suppressed} = 0; {{ double b_ = 0.0; {} if (!fv[{s0}]) {rare} unsigned r_ = h2t_x87_rel(fx[{s0}], b_); fsw &= (uint16_t)~0x200u; {result} }}",
                        source(src)
                    ),
                );
                for k in 0..pop {
                    self.line(d, format!("fv[{}] = 0;", st(k)));
                }
            }
            X87Op::Xch(i) => {
                let other = st(i);
                self.line(
                    d,
                    format!(
                        "if (!fv[{s0}] || !fv[{other}]) {rare} {{ double t_ = fx[{s0}]; fx[{s0}] = fx[{other}]; fx[{other}] = t_; }} fw[{s0}] = 1; fw[{other}] = 1; fsw &= (uint16_t)~0x200u;"
                    ),
                );
            }
            X87Op::Mark { fop, mem } => {
                self.line(d, format!("fip = {pc:#010x}u; fcs = {GUEST_CS:#x}u; fop = {:#x}u;", fop & 0x7ff));
                if mem {
                    self.line(d, format!("fdp = {}; fds = {GUEST_DS:#x}u;", a(0)));
                }
            }
            // Refused before emission ([`x87_supported`]).
            _ => self.line(d, Self::rare("STOP", pc)),
        }
    }

    /// Store the locals in `mask` that this function writes; x87 at `rt`.
    fn spill(&mut self, mask: u32, rt: Option<u8>, d: usize, pc: u32) {
        let mask = mask & self.sig.own;
        for (i, r) in REG.iter().enumerate() {
            if mask & (1 << i) != 0 {
                self.line(d, format!("s->r[{i}] = {r};"));
            }
        }
        for (i, n) in FLAG.iter().enumerate() {
            if mask & (1 << (FLAG_SHIFT + i as u32)) != 0 {
                self.line(d, format!("s->f[{i}] = f_{n}; s->d[{i}] = d_{n};"));
            }
        }
        if mask & EFLAGS_REST != 0 {
            self.line(d, "s->efr = efr; s->fdr = fdr;");
        }
        for i in 0..8u8 {
            if mask & xmm(i) != 0 {
                self.line(d, format!("s->x[{i}] = x{i};"));
            }
        }
        if mask & X87 != 0 && self.x87() {
            match rt {
                Some(rt) => {
                    self.line(d, X87_SPILL);
                    self.line(
                        d,
                        format!("s->ftop = (ftop0 + {rt}u) & 7u; s->fsw = fsw; s->fop = fop; s->fcs = fcs; s->fds = fds; s->fip = fip; s->fdp = fdp;"),
                    );
                }
                None => self.line(d, Self::rare("X87", pc)),
            }
        }
    }

    /// Reload the locals in `mask` after a call; x87 must come back at `rt`.
    fn reload(&mut self, mask: u32, rt: Option<u8>, d: usize, pc: u32) {
        for (i, r) in REG.iter().enumerate() {
            if mask & (1 << i) != 0 {
                self.line(d, format!("{r} = s->r[{i}];"));
            }
        }
        for (i, n) in FLAG.iter().enumerate() {
            if mask & (1 << (FLAG_SHIFT + i as u32)) != 0 {
                self.line(d, format!("f_{n} = s->f[{i}]; d_{n} = s->d[{i}];"));
            }
        }
        if mask & EFLAGS_REST != 0 {
            self.line(d, "efr = s->efr; fdr = s->fdr;");
        }
        for i in 0..8u8 {
            if mask & xmm(i) != 0 {
                self.line(d, format!("x{i} = s->x[{i}];"));
            }
        }
        if mask & X87 != 0 && self.x87() {
            match rt {
                Some(rt) => {
                    self.line(d, format!("if (((s->ftop - ftop0) & 7u) != {rt}u) {}", Self::rare("X87", pc)));
                    self.line(d, X87_LOAD);
                    self.line(d, "fsw = s->fsw; fop = s->fop; fcs = s->fcs; fds = s->fds; fip = s->fip; fdp = s->fdp;");
                }
                None => self.line(d, Self::rare("X87", pc)),
            }
        }
    }

    fn goto_local(&mut self, t: u32, pc: u32, d: usize) {
        if self.tops.blocks.contains_key(&t) && self.f.blocks.contains_key(&t) {
            self.line(d, format!("goto L_{t:08x};"));
        } else {
            self.line(d, Self::rare("STOP", pc));
        }
    }

    /// Leave for function `t`, which returns to this function's caller.
    fn tail(&mut self, t: u32, rt: Option<u8>, pc: u32, d: usize) {
        if self.sigs.contains_key(&t) {
            self.spill(ALL, rt, d, pc);
            self.line(d, format!("return {}(s, base);", symbol(t)));
        } else {
            self.line(d, Self::rare("CALL", pc));
        }
    }

    fn dest(&mut self, dest: Dest, rt: Option<u8>, pc: u32, d: usize) {
        if dest.tail { self.tail(dest.pc, rt, pc, d) } else { self.goto_local(dest.pc, pc, d) }
    }

    fn control(&mut self, i: &IrInsn, last: bool, rt: Option<u8>) {
        let pc = i.pc;
        let d = 2;
        match &i.control {
            Control::Next => {
                if last {
                    self.goto_local(i.next_pc(), pc, d);
                }
            }
            Control::Jump { target: Target::Direct(t), tail: false, .. } => self.goto_local(*t, pc, d),
            Control::Jump { target: Target::Direct(t), tail: true, .. } => self.tail(*t, rt, pc, d),
            Control::Jump { target: Target::Dynamic(v), .. } => {
                self.spill(ALL, rt, d, pc);
                self.line(d, format!("return h2t_dispatch(s, base, {}, {pc:#010x}u);", val(v)));
            }
            Control::Branch { cond, taken, fallthrough } => {
                self.line(d, format!("if ({}) {{", val(cond)));
                self.dest(*taken, rt, pc, d + 1);
                self.line(d, "} else {");
                self.dest(*fallthrough, rt, pc, d + 1);
                self.line(d, "}");
            }
            Control::Switch { target, cases, .. } => {
                self.line(d, format!("switch ({}) {{", val(target)));
                let mut seen = BTreeSet::new();
                for c in cases.iter().filter(|c| seen.insert(c.pc)) {
                    self.line(d, format!("case {:#010x}u: {{", c.pc));
                    self.dest(*c, rt, pc, d + 1);
                    self.line(d, "}");
                }
                self.line(d, format!("default: {}", Self::rare("STOP", pc)));
                self.line(d, "}");
            }
            Control::Call { target, continuation, .. } => self.call(target, *continuation, rt, pc, d),
            Control::Return { target, .. } => {
                self.spill(ALL, rt, d, pc);
                self.line(d, format!("s->ret = {}; return H2T_OK;", val(target)));
            }
            Control::Fault(_) => self.line(d, Self::rare("FAULT", pc)),
            Control::Stop(_) => self.line(d, Self::rare("STOP", pc)),
        }
    }

    fn call(&mut self, target: &Target, cont: u32, rt: Option<u8>, pc: u32, d: usize) {
        let (call, touches, delta) = match target {
            Target::Direct(t) => match self.sigs.get(t) {
                Some(sig) => (format!("{}(s, base)", symbol(*t)), (sig.reads | sig.writes, sig.writes), sig.x87_delta),
                None => {
                    self.line(d, Self::rare("CALL", pc));
                    return;
                }
            },
            Target::Dynamic(v) => (format!("h2t_dispatch(s, base, {}, {pc:#010x}u)", val(v)), (ALL, ALL), Some(0)),
        };
        // What the callee may read or write must be current in the state;
        // what it may write comes back.
        self.spill(touches.0 | touches.1, rt, d, pc);
        self.line(d, format!("{{ uint32_t rc_ = {call}; if (rc_) return rc_; }}"));
        self.line(d, format!("if (s->ret != {cont:#010x}u) {}", Self::rare("RETURN", pc)));
        let after = add(rt, delta);
        self.reload(touches.1, after, d, pc);
        if self.f.blocks.contains_key(&cont) {
            self.goto_local(cont, pc, d);
        } else {
            self.tail(cont, after, pc, d);
        }
    }
}

const X87_LOAD: &str = "for (unsigned k_ = 0; k_ < 8u; k_++) { unsigned p_ = (ftop0 + k_) & 7u; fx[k_] = s->fx[p_]; fv[k_] = s->fv[p_]; fw[k_] = s->fw[p_]; }";
const X87_SPILL: &str = "for (unsigned k_ = 0; k_ < 8u; k_++) { unsigned p_ = (ftop0 + k_) & 7u; s->fx[p_] = fx[k_]; s->fv[p_] = fv[k_]; s->fw[p_] = fw[k_]; }";

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u32, width: Width) -> Temp {
        Temp { id, width }
    }

    /// `mov eax, [esp+4]; ret 4` as IR, with no lifter.
    fn leaf(entry: u32) -> IrFunction {
        let load = IrInsn {
            pc: entry,
            len: 4,
            atomic: None,
            stmts: vec![
                Stmt::ReadReg { dst: t(0, Width::W32), reg: Gpr::Esp, slice: Slice::Full },
                Stmt::Assign {
                    dst: t(1, Width::W32),
                    expr: ir::Expr::Binary { op: ir::BinOp::Add, a: t(0, Width::W32).into(), b: Value::u32(4) },
                },
                Stmt::Load { dst: t(2, Width::W32), addr: ir::Addr { seg: Segment::Flat, offset: t(1, Width::W32).into() } },
                Stmt::WriteReg { reg: Gpr::Eax, slice: Slice::Full, value: t(2, Width::W32).into() },
            ],
            control: Control::Next,
        };
        let ret = IrInsn {
            pc: entry + 4,
            len: 3,
            atomic: None,
            stmts: vec![
                Stmt::ReadReg { dst: t(0, Width::W32), reg: Gpr::Esp, slice: Slice::Full },
                Stmt::Load { dst: t(1, Width::W32), addr: ir::Addr { seg: Segment::Flat, offset: t(0, Width::W32).into() } },
                Stmt::Assign {
                    dst: t(2, Width::W32),
                    expr: ir::Expr::Binary { op: ir::BinOp::Add, a: t(0, Width::W32).into(), b: Value::u32(8) },
                },
                Stmt::WriteReg { reg: Gpr::Esp, slice: Slice::Full, value: t(2, Width::W32).into() },
            ],
            control: Control::Return { target: t(1, Width::W32).into(), cleanup: 4 },
        };
        IrFunction {
            entry,
            blocks: BTreeMap::from([(entry, ir::IrBlock { start: entry, insns: vec![load, ret] })]),
            interior_entries: vec![],
            errors: vec![],
        }
    }

    #[test]
    fn a_leaf_reads_esp_and_writes_eax_and_esp() {
        let closure = Closure::build(&[0x1000], 16, |pc| (pc == 0x1000).then(|| leaf(0x1000)));
        let tier = Tier::build(&closure);
        let sig = tier.signatures[&0x1000];
        assert_eq!(sig.reads, gpr(Gpr::Esp));
        assert_eq!(sig.writes, gpr(Gpr::Eax) | gpr(Gpr::Esp));
        assert_eq!(sig.cleanup, Some(4));
        assert_eq!(sig.x87_delta, Some(0));
        assert!(!sig.uses_x87);
        let m = write(&closure, &[0x1000]);
        assert_eq!(m.roots, vec![0x1000]);
        assert!(m.c.contains("H2_EXPORT uint32_t h2tier_00001000(CpuAbi *cpu"));
        assert!(m.c.contains("h2t_ld32(base, t1)"));
        // The boundary moves only the signature.
        assert!(m.c.contains("s->r[4] = cpu->esp;"));
        assert!(!m.c.contains("s->r[1] = cpu->ecx;"));
    }

    #[test]
    fn a_partial_write_reads_the_rest_of_the_register() {
        let mut e = Effect::default();
        stmts_effect(&[Stmt::WriteReg { reg: Gpr::Ebx, slice: Slice::Low8, value: Value::constant(Width::W8, 1) }], &mut e, true);
        assert_eq!(e.uses, gpr(Gpr::Ebx));
        assert_eq!(e.kills, 0);
        assert_eq!(e.writes, gpr(Gpr::Ebx));
    }

    #[test]
    fn unsupported_forms_are_refused_with_a_reason() {
        let mut f = leaf(0x2000);
        f.blocks.get_mut(&0x2000).unwrap().insns[0].stmts.push(Stmt::Intrinsic {
            op: Intrinsic::ReadTsc,
            args: vec![],
            dsts: vec![t(9, Width::W64)],
        });
        let closure = Closure::build(&[0x2000], 16, |_| Some(f.clone()));
        let tier = Tier::build(&closure);
        assert_eq!(tier.refused[&0x2000], Refusal::Unsupported { pc: 0x2000, what: "rdtsc".into() });
        assert!(write(&closure, &[0x2000]).roots.is_empty());
    }
}
