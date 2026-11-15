//! Per-function control-flow graphs, exported for the lifter.
//!
//! Discovery ([`crate::analyze`]) decides *where functions are* and which of
//! them return. This module turns that verdict into a model a lifter can
//! lower one function at a time: basic blocks with their instruction
//! addresses, typed edges, call sites with resolved targets, jump tables with
//! every target, and the address-taken interior entries that need adapters.
//!
//! It is a deterministic post-pass: each function is re-descended from its
//! entry using the final analysis (function starts, `returns`, jump tables,
//! and the tail-call decisions discovery made), not the byte claims. So:
//!
//! - **Ownership is reachability.** A function holds exactly the
//!   instructions reachable from its entry without leaving through a call,
//!   tail jump or return. Code reachable from two entries appears in both
//!   CFGs. Nothing is inferred from "up to the next function start".
//! - **Entries are never absorbed.** Any edge into another discovered
//!   function's entry (a `jmp`, either side of a `jcc`, a fall-through, a
//!   call continuation or a switch case) leaves this function: the edge has
//!   [`Edge::exits`] set and the target is not a block here. The lowering
//!   transfers through the entry, so an override registered there is
//!   honoured on every path (`docs/LIFTER.md` §2).
//! - **Calls end blocks.** In the runner design every guest `call` is a
//!   transfer (push the continuation, yield `Call(target)`), so the
//!   continuation is a resume point and starts its own block, reached by a
//!   [`EdgeKind::CallContinuation`] edge. A call that never returns has no
//!   continuation edge at all.
//! - **Blocks are true basic blocks.** Every branch target, fall-through
//!   after a conditional branch, call continuation, switch target and
//!   interior entry starts a block, so each instruction is in exactly one
//!   block of a given function.
//!
//! Anything the descent can't make sense of (a target in the middle of an
//! instruction, bytes that don't decode, control leaving the code sections)
//! is recorded in [`FunctionCfg::errors`]; the block involved ends with no
//! edges. Discovery reports zero conflicts on the shipped image, so errors
//! here mean the two passes disagree and are worth investigating.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use iced_x86::{Code, Decoder, DecoderOptions, FlowControl, Instruction, OpKind, Register};
use xbe::Xbe;

use crate::image::mapped_bytes;
use crate::{Analysis, NORETURN_KERNEL, SwitchKind, is_code_section};

/// A jump table as discovery read it: `jmp [index*4 + table]`, optionally
/// preceded by a byte index table (`movzx index, byte [i + index_table]`).
///
/// `targets` are what the analyzer found in the image. Runtime code must
/// still evaluate the real guest load: the table may be writable, and for
/// [`SwitchKind::Unbounded`] tables the count is a heuristic, not a bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpTable {
    /// Address of the `jmp` instruction.
    pub site: u32,
    /// Address of entry 0 (the displacement of the `jmp`'s memory operand).
    pub table: u32,
    pub kind: SwitchKind,
    /// Entries read, holes included: `targets.len() + holes.len()`.
    pub count: u32,
    /// The guard's bound on the value indexing the table the code reads
    /// first: entries for [`SwitchKind::Bounded`], byte-table entries for
    /// [`SwitchKind::Indexed`], `None` when no guard was recognised.
    pub bound: Option<u32>,
    /// For [`SwitchKind::Indexed`]: (address, length) of the byte table, as
    /// the code indexes it. A bit-flag switch may aim this base before the
    /// real table data; the length is the guard's bound.
    pub index_table: Option<(u32, u32)>,
    /// One target per entry that is not a hole, in table order, duplicates
    /// kept. [`JumpTable::cases`] pairs them with their indices.
    pub targets: Vec<u32>,
    /// Indices of null entries between targets, ascending. Only an
    /// [`SwitchKind::Unbounded`] table has them: an enum switch with no
    /// range check fills the values it never sees with 0. The guest
    /// jumping through one is an unknown target.
    pub holes: Vec<u32>,
}

impl JumpTable {
    /// `(index, target)` for every entry that is not a hole.
    pub fn cases(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (0..self.count).filter(|i| self.holes.binary_search(i).is_err()).zip(self.targets.iter().copied())
    }
}

/// One function's control flow.
#[derive(Debug, Clone, Default)]
pub struct FunctionCfg {
    pub entry: u32,
    /// Basic blocks keyed by start address. The entry block is
    /// `blocks[&entry]` unless the entry itself failed to decode.
    pub blocks: BTreeMap<u32, Block>,
    /// Every call instruction in the function, in address order.
    pub calls: Vec<CallSite>,
    /// Address-taken code inside this function ([`Analysis::labels`] that
    /// land on one of its instructions). Each starts a block and needs an
    /// explicit external-entry adapter; they are not function entries.
    pub interior_entries: Vec<u32>,
    /// Problems found while descending; see the module docs.
    pub errors: Vec<CfgError>,
}

impl FunctionCfg {
    /// Number of instructions over all blocks.
    pub fn insn_count(&self) -> usize {
        self.blocks.values().map(|b| b.insns.len()).sum()
    }

    /// The block containing the instruction at `va`.
    pub fn block_containing(&self, va: u32) -> Option<&Block> {
        let (_, b) = self.blocks.range(..=va).next_back()?;
        b.insns.binary_search(&va).is_ok().then_some(b)
    }
}

/// A maximal straight-line run of instructions with one entry (its first
/// instruction) and one exit (its last).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub start: u32,
    /// One past the last byte of the last instruction.
    pub end: u32,
    /// Address of each instruction, in order. Contiguous: each starts where
    /// the previous one ends.
    pub insns: Vec<u32>,
    /// How control leaves the block. Empty only when the descent hit an
    /// error at the block's end (see [`FunctionCfg::errors`]).
    pub edges: Vec<Edge>,
}

/// A way out of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub kind: EdgeKind,
    /// For edges that stay in the function: the successor block's start.
    /// For edges that exit to a static address (a tail edge, a
    /// [`EdgeKind::NoReturnCall`]): that address, another function's
    /// entry. `None` where there is no static target.
    pub target: Option<u32>,
    /// Control leaves the function here: `target`, if any, is not a block
    /// of this function. True for tail jumps, returns, traps, non-returning
    /// calls, and for any branch, fall-through, continuation or switch case
    /// that lands on another function's entry.
    pub exits: bool,
}

/// Why control leaves a block, and so what a lowering must do there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// The block ends only because the next instruction starts a block.
    Fallthrough,
    /// A conditional branch was taken.
    Taken,
    /// A conditional branch fell through.
    NotTaken,
    /// An unconditional `jmp` to a label in this function.
    Jump,
    /// A call that returns: target is the continuation (call VA + length).
    /// The call itself is the [`CallSite`] at the block's last instruction.
    CallContinuation,
    /// A direct call to a function that never returns (target: the callee).
    NoReturnCall,
    /// A call through a kernel thunk that never returns (the ordinal:
    /// HalReturnToFirmware, KeBugCheck, KeBugCheckEx or
    /// PsTerminateSystemThread). Models process/thread termination.
    KernelNoReturn(u32),
    /// A `jmp` into another function: a tail call (target: its entry).
    Tail,
    /// A `jmp` through a kernel thunk slot.
    TailKernel { slot: u32, ordinal: u32 },
    /// A computed `jmp` with no known target (`jmp eax`, `jmp [ecx+8]`, or
    /// a table jump whose table couldn't be read). The runner evaluates it
    /// and dispatches like any other transfer.
    TailIndirect,
    /// One entry of a jump table: `index` into the table at `table`, used by
    /// the `jmp` at `site`. One edge per entry, duplicates kept.
    Switch { site: u32, table: u32, index: u32 },
    /// `ret` / `ret n`.
    Return,
    /// `int3`, `int n`, `ud2`, `hlt` and other instructions that end guest
    /// flow by faulting or trapping.
    Trap,
}

/// A `call` instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    pub va: u32,
    /// Instruction length, so the pushed return address is `va + len`.
    pub len: u8,
    pub target: CallTarget,
    /// Whether the continuation is reachable according to the analysis
    /// (the callee is known to return, or can't be ruled out).
    pub returns: bool,
}

impl CallSite {
    /// The guest return address this call pushes.
    pub fn continuation(&self) -> u32 {
        self.va + u32::from(self.len)
    }
}

/// Where a call goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallTarget {
    /// `call rel32`.
    Direct(u32),
    /// `call [slot]` where `slot` is a kernel thunk table entry. The lowering
    /// must still load the slot (it can be rewritten at runtime); the
    /// ordinal is what the slot held in the image.
    Kernel { slot: u32, ordinal: u32 },
    /// `call [mem]` other than a kernel thunk: vtables, function pointers.
    IndirectMemory,
    /// `call reg`.
    IndirectRegister(Register),
}

/// Something the descent couldn't follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfgError {
    /// Bytes at this address don't decode.
    Decode(u32),
    /// Control reaches this address, which isn't in a code section.
    OutsideCode(u32),
    /// A branch lands on this address, inside an instruction decoded from
    /// another path.
    IntoInsnBody(u32),
    /// The instruction at this address runs over the start of one decoded
    /// from another path.
    OverlapsInsn(u32),
}

/// The executable bytes of an image, by virtual address. Same regions as
/// discovery uses: code sections at their virtual size, zero-filled past
/// their raw data as the loader maps them.
pub struct CodeView<'a> {
    regions: Vec<(u32, Cow<'a, [u8]>)>,
}

impl<'a> CodeView<'a> {
    pub fn new(xbe: &Xbe<'a>) -> Self {
        let regions = xbe
            .sections
            .iter()
            .filter(|s| is_code_section(s))
            .filter_map(|s| Some((s.va, mapped_bytes(xbe, s).ok()?)))
            .collect();
        Self { regions }
    }

    /// Bytes from `va` to the end of its code section.
    pub fn bytes_from(&self, va: u32) -> Option<&[u8]> {
        self.regions.iter().find_map(|(base, b)| {
            let off = va.checked_sub(*base)? as usize;
            (off < b.len()).then(|| &b[off..])
        })
    }

    /// Decode the instruction at `va`; `None` outside code or if invalid.
    pub fn decode(&self, va: u32) -> Option<Instruction> {
        let insn = Decoder::with_ip(32, self.bytes_from(va)?, va as u64, DecoderOptions::NONE).decode();
        (insn.code() != Code::INVALID).then_some(insn)
    }
}

/// How one instruction leaves (or doesn't leave) straight-line flow.
#[derive(Debug, Clone, Copy)]
enum Flow {
    Next,
    Cond(u32),
    Jump(u32),
    Tail(u32),
    TailKernel(u32, u32),
    TailIndirect,
    Switch,
    Call(CallTarget, bool),
    Return,
    Trap,
}

impl Flow {
    fn ends_block(self) -> bool {
        !matches!(self, Flow::Next)
    }
}

/// Builds [`FunctionCfg`]s against one analysis. Construct once and reuse
/// when building many.
pub struct CfgBuilder<'a> {
    analysis: &'a Analysis,
    code: CodeView<'a>,
    thunks: HashMap<u32, u32>,
}

impl<'a> CfgBuilder<'a> {
    pub fn new(analysis: &'a Analysis, xbe: &Xbe<'a>) -> Self {
        // Discovery already parsed the thunk table successfully to produce
        // `analysis`, so this can't fail for a matching image.
        let thunks = xbe.kernel_imports().unwrap_or_default().iter().map(|k| (k.slot_va, k.ordinal)).collect();
        Self { analysis, code: CodeView::new(xbe), thunks }
    }

    pub fn code(&self) -> &CodeView<'a> {
        &self.code
    }

    fn thunk(&self, insn: &Instruction) -> Option<(u32, u32)> {
        let absolute = insn.op0_kind() == OpKind::Memory
            && insn.memory_base() == Register::None
            && insn.memory_index() == Register::None;
        let slot = insn.memory_displacement32();
        absolute.then(|| self.thunks.get(&slot).map(|o| (slot, *o))).flatten()
    }

    /// `t` is another discovered function's entry, so control reaching it
    /// leaves the function being built.
    fn is_other_entry(&self, entry: u32, t: u32) -> bool {
        t != entry && self.analysis.functions.contains_key(&t)
    }

    fn classify(&self, entry: u32, insn: &Instruction) -> Flow {
        let va = insn.ip32();
        match insn.flow_control() {
            FlowControl::Next => Flow::Next,
            FlowControl::ConditionalBranch => Flow::Cond(insn.near_branch32()),
            FlowControl::UnconditionalBranch => {
                // A far jump to a selector other than the flat one is unknown.
                let Some(t) = crate::direct_jump_target(insn) else {
                    return Flow::TailIndirect;
                };
                if self.is_other_entry(entry, t) || (t != entry && self.analysis.tail_sites.contains(&va)) {
                    Flow::Tail(t)
                } else {
                    Flow::Jump(t)
                }
            }
            FlowControl::IndirectBranch => {
                if let Some((slot, ord)) = self.thunk(insn) {
                    Flow::TailKernel(slot, ord)
                } else if crate::is_table_jump(insn) && self.analysis.jump_tables.contains_key(&va) {
                    Flow::Switch
                } else {
                    Flow::TailIndirect
                }
            }
            FlowControl::Call => {
                let t = insn.near_branch32();
                // Mirrors discovery: a target that isn't a function (outside
                // code, mid-instruction) is assumed to return.
                let returns = self.analysis.functions.get(&t).is_none_or(|f| f.returns);
                Flow::Call(CallTarget::Direct(t), returns)
            }
            FlowControl::IndirectCall => match self.thunk(insn) {
                Some((slot, ordinal)) => {
                    Flow::Call(CallTarget::Kernel { slot, ordinal }, !NORETURN_KERNEL.contains(&ordinal))
                }
                None if insn.op0_kind() == OpKind::Register => {
                    Flow::Call(CallTarget::IndirectRegister(insn.op0_register()), true)
                }
                None => Flow::Call(CallTarget::IndirectMemory, true),
            },
            FlowControl::Return => Flow::Return,
            FlowControl::Interrupt | FlowControl::Exception | FlowControl::XbeginXabortXend => Flow::Trap,
        }
    }

    /// The CFG of the function starting at `entry`, or `None` if discovery
    /// didn't find a function there.
    pub fn build(&self, entry: u32) -> Option<FunctionCfg> {
        if !self.analysis.functions.contains_key(&entry) {
            return None;
        }
        let mut cfg = FunctionCfg { entry, ..Default::default() };
        // va -> (length, flow)
        let mut insns: BTreeMap<u32, (u8, Flow)> = BTreeMap::new();
        let mut leaders = BTreeSet::from([entry]);
        let mut work = vec![entry];
        // Another function's entry is never a leader here: edges into it
        // exit (see `edges`).
        let lead = |t: u32, leaders: &mut BTreeSet<u32>, work: &mut Vec<u32>| {
            if !self.is_other_entry(entry, t) && leaders.insert(t) {
                work.push(t);
            }
        };

        while let Some(start) = work.pop() {
            let mut va = start;
            loop {
                if insns.contains_key(&va) {
                    break;
                }
                if let Some((&p, &(len, _))) = insns.range(..va).next_back()
                    && p + u32::from(len) > va
                {
                    cfg.errors.push(CfgError::IntoInsnBody(va));
                    break;
                }
                let Some(bytes) = self.code.bytes_from(va) else {
                    cfg.errors.push(CfgError::OutsideCode(va));
                    break;
                };
                let insn = Decoder::with_ip(32, bytes, va as u64, DecoderOptions::NONE).decode();
                if insn.code() == Code::INVALID {
                    cfg.errors.push(CfgError::Decode(va));
                    break;
                }
                let len = insn.len() as u32;
                if insns.range(va + 1..va + len).next().is_some() {
                    cfg.errors.push(CfgError::OverlapsInsn(va));
                    break;
                }
                let flow = self.classify(entry, &insn);
                insns.insert(va, (len as u8, flow));
                match flow {
                    Flow::Cond(t) => {
                        lead(t, &mut leaders, &mut work);
                        lead(va + len, &mut leaders, &mut work);
                    }
                    Flow::Jump(t) => lead(t, &mut leaders, &mut work),
                    Flow::Switch => {
                        for &t in &self.analysis.jump_tables[&va].targets {
                            lead(t, &mut leaders, &mut work);
                        }
                    }
                    Flow::Call(target, returns) => {
                        cfg.calls.push(CallSite { va, len: len as u8, target, returns });
                        if returns {
                            lead(va + len, &mut leaders, &mut work);
                        }
                    }
                    _ => {}
                }
                if flow.ends_block() {
                    break;
                }
                va += len;
                // Falling into another function's entry leaves this one.
                if self.is_other_entry(entry, va) {
                    break;
                }
            }
        }

        // Address-taken labels inside this function start blocks too.
        for &l in self.analysis.labels.iter() {
            if insns.contains_key(&l) {
                cfg.interior_entries.push(l);
                leaders.insert(l);
            }
        }
        cfg.calls.sort_by_key(|c| c.va);

        // Cut the instruction map into blocks.
        let mut current: Option<Block> = None;
        let mut iter = insns.iter().peekable();
        while let Some((&va, &(len, flow))) = iter.next() {
            let block = current.get_or_insert_with(|| Block { start: va, end: va, insns: Vec::new(), edges: Vec::new() });
            block.insns.push(va);
            let next = va + u32::from(len);
            block.end = next;
            let contiguous = iter.peek().is_some_and(|(n, _)| **n == next);
            if !flow.ends_block() && contiguous && !leaders.contains(&next) {
                continue;
            }
            let mut block = current.take().unwrap();
            block.edges = self.edges(entry, va, next, flow, contiguous);
            cfg.blocks.insert(block.start, block);
        }
        Some(cfg)
    }

    fn edges(&self, entry: u32, va: u32, next: u32, flow: Flow, contiguous: bool) -> Vec<Edge> {
        // A static successor: local unless it is another function's entry.
        let to = |kind, t: u32| Edge { kind, target: Some(t), exits: self.is_other_entry(entry, t) };
        let out = |kind, target| Edge { kind, target, exits: true };
        match flow {
            Flow::Next if contiguous || self.is_other_entry(entry, next) => vec![to(EdgeKind::Fallthrough, next)],
            // A non-terminator that isn't followed by a decoded instruction
            // stopped on an error.
            Flow::Next => Vec::new(),
            Flow::Cond(t) => vec![to(EdgeKind::Taken, t), to(EdgeKind::NotTaken, next)],
            Flow::Jump(t) => vec![to(EdgeKind::Jump, t)],
            Flow::Tail(t) => vec![out(EdgeKind::Tail, Some(t))],
            Flow::TailKernel(slot, ordinal) => vec![out(EdgeKind::TailKernel { slot, ordinal }, None)],
            Flow::TailIndirect => vec![out(EdgeKind::TailIndirect, None)],
            Flow::Switch => {
                let jt = &self.analysis.jump_tables[&va];
                jt.cases().map(|(index, t)| to(EdgeKind::Switch { site: va, table: jt.table, index }, t)).collect()
            }
            Flow::Call(_, true) => vec![to(EdgeKind::CallContinuation, next)],
            Flow::Call(CallTarget::Kernel { ordinal, .. }, false) => {
                vec![out(EdgeKind::KernelNoReturn(ordinal), None)]
            }
            Flow::Call(CallTarget::Direct(t), false) => vec![out(EdgeKind::NoReturnCall, Some(t))],
            Flow::Call(_, false) => vec![out(EdgeKind::NoReturnCall, None)],
            Flow::Return => vec![out(EdgeKind::Return, None)],
            Flow::Trap => vec![out(EdgeKind::Trap, None)],
        }
    }
}

impl Analysis {
    /// The CFG of the function at `entry`; `None` if it isn't a function.
    /// Building many? Use [`CfgBuilder`] or [`Analysis::cfgs`].
    pub fn function_cfg(&self, xbe: &Xbe, entry: u32) -> Option<FunctionCfg> {
        CfgBuilder::new(self, xbe).build(entry)
    }

    /// CFGs of every discovered function, keyed by entry.
    pub fn cfgs(&self, xbe: &Xbe) -> BTreeMap<u32, FunctionCfg> {
        let b = CfgBuilder::new(self, xbe);
        self.functions.keys().filter_map(|&f| Some((f, b.build(f)?))).collect()
    }
}
