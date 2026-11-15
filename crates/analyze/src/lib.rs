//! Function discovery and coverage over an XBE's code sections.
//!
//! Works in two tiers:
//!
//! 1. **Certain.** Recursive descent from the entry point and every direct
//!    call target. Each function's control flow is explored on its own, so
//!    we learn which functions never return, and a call to one of those
//!    ends the block instead of decoding whatever the compiler put after it.
//!    Switch jump tables are sized from their `cmp`/`ja` guard.
//! 2. **Speculative.** Addresses that only look like code (immediates in
//!    code, pointers in data, starts of leftover gaps) are explored into an
//!    overlay and committed only if the whole function decodes cleanly.
//!    A pointer to an instruction that's already known is a label, not a
//!    new function.
//!
//! A conflict means two certain decodes disagree about a byte. Speculative
//! candidates that would conflict are rejected and counted, not reported.

pub mod cfg;
pub mod compare;
pub mod fpstats;
mod image;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use iced_x86::{
    Code, Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpAccess, OpKind,
    Register,
};
use xbe::Xbe;

pub use cfg::JumpTable;
pub use image::{ConflictKind, is_code_section};
use image::{Byte, Image, Overlay};

/// Kernel exports that never return: HalReturnToFirmware, KeBugCheck,
/// KeBugCheckEx, PsTerminateSystemThread.
const NORETURN_KERNEL: &[u32] = &[49, 95, 96, 258];

/// Largest function a speculative trial may decode, in instructions.
const TRIAL_LIMIT: usize = 50_000;
/// Largest switch we trust a guard for.
const MAX_SWITCH: u32 = 4096;
/// Entries read from a jump table with no recognisable guard.
const MAX_UNBOUNDED_SWITCH: u32 = 512;
/// Consecutive null entries an unguarded table may skip over.
const MAX_SWITCH_HOLES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Entry,
    Call,
    /// A `jmp` to just after int3 padding: a tail call into a new function.
    TailCall,
    /// An immediate operand in known code (`push offset f`, `mov [x], offset f`).
    CodeRef,
    /// An aligned pointer in a data section (vtables, callback tables).
    DataPointer,
    /// The start of an undecoded run after padding.
    GapScan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SwitchKind {
    /// `cmp r, n; ja default; jmp [r*4 + table]`
    Bounded,
    /// As above with a byte table in between: `movzx r, byte [r + index]`.
    Indexed,
    /// No guard found; read until an entry stops pointing at code.
    Unbounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conflict {
    pub va: u32,
    pub kind: ConflictKind,
    /// The function being explored when it happened.
    pub function: u32,
    /// The function that had already claimed the bytes, if any.
    pub claimed_by: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub start: u32,
    pub source: Source,
    pub section: String,
    pub returns: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SectionCoverage {
    pub name: String,
    pub size: usize,
    pub code: usize,
    pub data: usize,
    pub padding: usize,
    pub unknown: usize,
    pub functions: usize,
}

impl SectionCoverage {
    pub fn percent(&self) -> f64 {
        if self.size == 0 {
            return 100.0;
        }
        100.0 * (self.size - self.unknown) as f64 / self.size as f64
    }
}

pub struct Analysis {
    pub functions: BTreeMap<u32, Function>,
    pub sections: Vec<SectionCoverage>,
    pub switches: BTreeMap<u32, SwitchKind>,
    /// Every jump table read, keyed by the address of its `jmp`: where the
    /// table is, how it was sized, and each target in table order. A
    /// superset of the detail in [`Analysis::switches`].
    pub jump_tables: BTreeMap<u32, JumpTable>,
    pub conflicts: Vec<Conflict>,
    /// Speculative candidates turned down, by source and reason.
    pub rejected: BTreeMap<(Source, ConflictKind), usize>,
    /// Address-taken code inside known functions.
    pub labels: BTreeSet<u32>,
    /// Kernel ordinal -> number of call sites.
    pub kernel_calls: BTreeMap<u32, usize>,
    pub kernel_imports: usize,
    /// Largest unknown runs: (va, len).
    pub gaps: Vec<(u32, usize)>,
    /// Code sections: (va, len, name).
    pub ranges: Vec<(u32, u32, String)>,
    /// Speculative starts thrown out for conflicting with better evidence.
    pub blacklisted: usize,
    /// Analysis passes it took to settle.
    pub rounds: usize,
    /// False if the last round still had losers to throw out: the result
    /// holds known-bad speculative functions.
    pub converged: bool,
    /// Per code section: (va, byte states, claiming function per byte).
    claims: Vec<(u32, Vec<Byte>, Vec<u32>)>,
    /// Speculative functions that lost a contest; blacklisted next round.
    losers: Vec<u32>,
    /// `jmp` sites the certain pass decided leave their function (tail
    /// calls). Read by [`cfg`] so the exported CFG makes the same call.
    tail_sites: HashSet<u32>,
    /// What the aligned data words that point into code ended up pointing at.
    pub pointers: PointerCensus,
}

/// Aligned words in the data sections whose value lies in a code section,
/// sorted by what the analysis decided is at the address they hold. Distinct
/// values, not words: a vtable slot shared by many classes counts once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PointerCensus {
    /// A discovered function's first instruction.
    pub entries: usize,
    /// An instruction inside a function: an interior entry or a label.
    pub interior: usize,
    /// The middle of a decoded instruction.
    pub bodies: usize,
    /// Bytes claimed as data (jump tables).
    pub data: usize,
    /// Undecoded bytes that are all padding to the next claimed byte.
    pub padding: usize,
    /// Undecoded bytes that are not padding: code nobody reached, or a
    /// value that is not a pointer.
    pub unknown: usize,
}

struct FuncWork {
    source: Source,
    todo: Vec<u32>,
    seen: HashSet<u32>,
    /// Fall-through after a call whose callee isn't resolved yet.
    pending: Vec<(u32, u32)>,
    /// Functions this one tail-jumps into.
    tails: Vec<u32>,
    returns: bool,
    done: bool,
    /// Instructions this function claimed.
    insns: usize,
    /// Table jumps first reached with no guard, read only once every other
    /// path has been explored, in case one of those reaches them guarded.
    unguarded: Vec<(u32, Instruction)>,
}

impl FuncWork {
    fn new(source: Source, start: u32) -> Self {
        Self {
            source,
            todo: vec![start],
            seen: HashSet::new(),
            pending: Vec::new(),
            tails: Vec::new(),
            returns: false,
            done: false,
            insns: 0,
            unguarded: Vec::new(),
        }
    }
}

/// A speculative exploration. Nothing it claims is kept: if it passes, the
/// function is explored again for real.
#[derive(Default)]
struct Trial {
    overlay: Overlay,
    /// How to treat calls to functions not explored yet.
    assume_return: bool,
    insns: usize,
    failed: Option<ConflictKind>,
}

/// What the instructions just before an indirect jump say about its table.
#[derive(Default, Clone, Copy)]
struct Guard {
    cmp: Option<(Register, u32)>,
    bound: Option<(Register, u32)>,
    /// `movzx dst, byte [idx + table]` with `idx` bounded to `n`:
    /// (dst, table, n). The bound is captured at the load, since the load
    /// usually overwrites the register it was checked on.
    byte_index: Option<(Register, u32, u32)>,
}

impl Guard {
    /// Whether this guard sizes a table indexed by `index`.
    fn bounds(&self, index: Register) -> bool {
        self.bound.is_some_and(|(r, _)| r == index) || self.byte_index.is_some_and(|(d, _, _)| d == index)
    }

    /// Track the guard along the fall-through path. Anything that writes a
    /// register forgets what was known about it, and anything else that sets
    /// flags makes a pending `cmp` stale.
    fn observe(&mut self, insn: &Instruction, info: &mut InstructionInfoFactory) {
        // The bound on the register a byte is loaded through, before the
        // load overwrites it.
        let bound_before = self.bound;
        let written: Vec<Register> = info
            .info(insn)
            .used_registers()
            .iter()
            .filter(|u| !matches!(u.access(), OpAccess::Read | OpAccess::CondRead | OpAccess::NoMemAccess))
            .map(|u| u.register().full_register32())
            .collect();
        let writes = |r: Register| written.contains(&r);
        if self.cmp.is_some_and(|(r, _)| writes(r)) || (insn.rflags_modified() != 0 && insn.mnemonic() != Mnemonic::Cmp) {
            self.cmp = None;
        }
        if self.bound.is_some_and(|(r, _)| writes(r)) {
            self.bound = None;
        }
        if self.byte_index.is_some_and(|(dst, _, _)| writes(dst)) {
            self.byte_index = None;
        }

        match insn.mnemonic() {
            Mnemonic::Cmp
                if insn.op0_kind() == OpKind::Register
                    && insn.op0_register().is_gpr32()
                    && matches!(insn.op1_kind(), OpKind::Immediate32 | OpKind::Immediate8to32) =>
            {
                self.cmp = Some((insn.op0_register(), insn.immediate(1) as u32));
            }
            // Only the fall-through is followed here, so only branches whose
            // fall-through is the in-range side bound the index. (`jbe`/`jb`
            // fall through on the out-of-range side.)
            Mnemonic::Ja | Mnemonic::Jae => {
                let extra = u32::from(insn.mnemonic() == Mnemonic::Ja);
                self.bound = self.cmp.map(|(r, n)| (r, n.saturating_add(extra)));
                // A direct bound on the loaded index beats the byte table's.
                if let (Some((r, _)), Some((dst, _, _))) = (self.bound, self.byte_index)
                    && r == dst
                {
                    self.byte_index = None;
                }
            }
            _ if matches!(insn.code(), Code::Movzx_r32_rm8 | Code::Mov_r8_rm8)
                && insn.op1_kind() == OpKind::Memory =>
            {
                let (base, index) = (insn.memory_base(), insn.memory_index());
                let reg = match (base, index) {
                    (b, Register::None) if b != Register::None => Some(b),
                    (Register::None, i) if i != Register::None && insn.memory_index_scale() == 1 => Some(i),
                    _ => None,
                };
                // An index table lives at a fixed address in the image; a
                // byte loaded from the stack or a small offset isn't one.
                let disp = insn.memory_displacement32();
                if let Some(reg) = reg.filter(|r| !matches!(r, Register::ESP | Register::EBP))
                    && disp >= 0x10000
                    && let Some((_, n)) = bound_before.filter(|(r, _)| *r == reg.full_register32())
                {
                    self.byte_index = Some((insn.op0_register().full_register32(), disp, n));
                }
            }
            _ => {}
        }
    }
}

struct Analyzer<'a> {
    xbe: &'a Xbe<'a>,
    image: Image<'a>,
    funcs: BTreeMap<u32, FuncWork>,
    /// Functions not fully explored yet.
    open: BTreeSet<u32>,
    queue: Vec<u32>,
    current: u32,
    trial: Option<Trial>,
    conflicts: Vec<Conflict>,
    rejected: BTreeMap<(Source, ConflictKind), usize>,
    labels: BTreeSet<u32>,
    switches: BTreeMap<u32, SwitchKind>,
    jump_tables: BTreeMap<u32, JumpTable>,
    tail_sites: HashSet<u32>,
    thunk_slots: HashMap<u32, u32>,
    kernel_calls: BTreeMap<u32, usize>,
    /// Speculative starts that lost a conflict in an earlier round.
    blacklist: &'a HashSet<u32>,
    /// Where the latest trial failed.
    last_fail: (u32, u32),
    last_trial_insns: usize,
    last_overlay: Overlay,
    /// Speculative functions that lost a contest this round.
    losers: Vec<u32>,
    /// (candidate, rival) pairs already contested this round.
    contested: HashSet<(u32, u32)>,
    /// Candidates whose trial failed this round; not retried until the next.
    failed_candidates: HashSet<u32>,
    /// Functions whose code has been scanned for immediates.
    scanned: HashSet<u32>,
    /// Every aligned word in a data section that is a code address.
    data_words: HashSet<u32>,
}

/// Rounds of "blacklist the weaker side of each conflict and start over".
const MAX_ROUNDS: usize = 8;

pub fn analyze(xbe: &Xbe) -> Result<Analysis, xbe::Error> {
    let mut blacklist = HashSet::new();
    let mut round = 0;
    loop {
        round += 1;
        let mut a = run(xbe, &blacklist)?;
        // When a better-evidenced function runs into bytes a speculative
        // one claimed first, the speculative one was wrong.
        let mut losers: Vec<u32> = a
            .conflicts
            .iter()
            .filter_map(|c| {
                let here = a.functions.get(&c.function)?.source;
                let owner = c.claimed_by.filter(|o| *o != c.function)?;
                let theirs = a.functions.get(&owner)?.source;
                (theirs > here && theirs >= Source::CodeRef).then_some(owner)
            })
            .collect();
        losers.extend(a.losers.iter().copied());
        losers.retain(|l| !blacklist.contains(l));
        if losers.is_empty() || round == MAX_ROUNDS {
            a.blacklisted = blacklist.len();
            a.rounds = round;
            a.converged = losers.is_empty();
            return Ok(a);
        }
        blacklist.extend(losers);
    }
}

fn run(xbe: &Xbe, blacklist: &HashSet<u32>) -> Result<Analysis, xbe::Error> {
    let imports = xbe.kernel_imports()?;
    let mut a = Analyzer {
        xbe,
        image: Image::new(xbe)?,
        funcs: BTreeMap::new(),
        open: BTreeSet::new(),
        queue: Vec::new(),
        current: 0,
        trial: None,
        conflicts: Vec::new(),
        rejected: BTreeMap::new(),
        labels: BTreeSet::new(),
        switches: BTreeMap::new(),
        jump_tables: BTreeMap::new(),
        tail_sites: HashSet::new(),
        thunk_slots: imports.iter().map(|k| (k.slot_va, k.ordinal)).collect(),
        kernel_calls: BTreeMap::new(),
        blacklist,
        last_fail: (0, 0),
        last_trial_insns: 0,
        last_overlay: Overlay::default(),
        losers: Vec::new(),
        contested: HashSet::new(),
        failed_candidates: HashSet::new(),
        scanned: HashSet::new(),
        data_words: HashSet::new(),
    };
    a.data_words = a.data_words()?;

    a.add_function(xbe.entry, Source::Entry, xbe.entry);
    a.run_certain();
    // Strongest evidence first, each to a fixpoint, so a weak candidate
    // rarely claims bytes a stronger one would have explained.
    loop {
        let before = a.funcs.len();
        a.code_refs_to_fixpoint();
        let ptrs = a.data_pointers()?;
        a.try_all(ptrs, Source::DataPointer);
        a.code_refs_to_fixpoint();
        let gaps = a.gap_starts();
        a.try_all(gaps, Source::GapScan);
        if a.funcs.len() == before {
            break;
        }
    }
    Ok(a.finish(imports.len()))
}

impl<'a> Analyzer<'a> {
    fn add_function(&mut self, va: u32, source: Source, from: u32) {
        // The function being explored is out of the map while we work on it.
        if self.funcs.contains_key(&va) || (va == self.current && self.trial.is_none()) {
            return;
        }
        match self.image.state(va) {
            None => self.conflict(from, ConflictKind::OutsideCode),
            Some(Byte::InsnBody) => self.conflict(va, ConflictKind::IntoInsnBody),
            Some(Byte::Data) => self.conflict(va, ConflictKind::IntoData),
            Some(Byte::Unknown | Byte::InsnStart) => {
                self.funcs.insert(va, FuncWork::new(source, va));
                self.open.insert(va);
                self.queue.push(va);
            }
        }
    }

    fn conflict(&mut self, va: u32, kind: ConflictKind) {
        let span = match kind {
            ConflictKind::IntoInsnBody | ConflictKind::IntoData => 1,
            ConflictKind::BadJumpTable => 4,
            _ => 0,
        };
        self.conflict_at(va, span, kind);
    }

    /// A conflict over `[va, va + span)`: whoever claimed those bytes is the
    /// other side of it. Kinds that aren't about claimed bytes (an invalid
    /// opcode, leaving the section) have no other side.
    fn conflict_at(&mut self, va: u32, span: u32, kind: ConflictKind) {
        match &mut self.trial {
            Some(t) => {
                if t.failed.is_none() {
                    self.last_fail = (va, span);
                }
                t.failed.get_or_insert(kind);
            }
            None => {
                let claimed_by = self.image.owner_in(va, span);
                self.conflicts.push(Conflict { va, kind, function: self.current, claimed_by })
            }
        }
    }

    fn failed(&self) -> bool {
        self.trial.as_ref().is_some_and(|t| t.failed.is_some())
    }

    /// `Some(returns)` once a function is fully explored.
    fn returns_of(&self, va: u32) -> Option<bool> {
        match self.funcs.get(&va) {
            Some(f) if f.done => Some(f.returns),
            Some(_) => None,
            // Not a function we could add (outside code, mid-instruction):
            // already reported; carry on as if it returns.
            None if self.trial.is_none() && va != self.current => Some(true),
            None => None,
        }
    }

    fn thunk_ordinal(&self, insn: &Instruction) -> Option<u32> {
        (insn.op0_kind() == OpKind::Memory
            && insn.memory_base() == Register::None
            && insn.memory_index() == Register::None)
            .then(|| self.thunk_slots.get(&insn.memory_displacement32()).copied())
            .flatten()
    }

    fn note_kernel_call(&mut self, ordinal: u32, fresh: bool) {
        if self.trial.is_none() && fresh {
            *self.kernel_calls.entry(ordinal).or_default() += 1;
        }
    }

    fn run_block(&mut self, f: &mut FuncWork, fstart: u32, start: u32) {
        let mut va = start;
        let mut guard = Guard::default();
        let mut info = InstructionInfoFactory::new();
        loop {
            if self.failed() {
                return;
            }
            if !f.seen.insert(va) {
                // Arriving guarded at a table jump that was first reached
                // without one: size it from this path.
                if let Some(i) = f.unguarded.iter().position(|(a, _)| *a == va)
                    && guard.bounds(f.unguarded[i].1.memory_index().full_register32())
                {
                    let (_, insn) = f.unguarded.swap_remove(i);
                    self.jump_table(f, fstart, va, &insn, guard);
                }
                return;
            }
            let Some(bytes) = self.image.bytes_from(va) else {
                self.conflict(va, ConflictKind::OutsideCode);
                return;
            };
            let insn = Decoder::with_ip(32, bytes, va as u64, DecoderOptions::NONE).decode();
            if insn.code() == Code::INVALID {
                self.conflict(va, ConflictKind::InvalidOpcode);
                return;
            }
            let len = insn.len();
            let fresh = match self.image.check_insn(va, len, self.trial.as_ref().map(|t| &t.overlay)) {
                Err(kind) => {
                    let span = if kind == ConflictKind::OverlapsClaimed { len as u32 } else { 1 };
                    self.conflict_at(va, span, kind);
                    return;
                }
                Ok(true) => false,
                Ok(false) => {
                    match &mut self.trial {
                        Some(t) => {
                            t.overlay.claim_insn(va, len);
                            t.insns += 1;
                            if t.insns > TRIAL_LIMIT {
                                t.failed.get_or_insert(ConflictKind::RunsOffSection);
                            }
                        }
                        None => {
                            self.image.claim_insn(va, len, fstart);
                            f.insns += 1;
                        }
                    }
                    true
                }
            };
            if self.trial.is_some() && implausible(&insn) {
                self.conflict(va, ConflictKind::Implausible);
                return;
            }
            guard.observe(&insn, &mut info);

            match insn.flow_control() {
                FlowControl::Next => {}
                FlowControl::ConditionalBranch => f.todo.push(insn.near_branch32()),
                FlowControl::UnconditionalBranch => {
                    if let Some(t) = direct_jump_target(&insn) {
                        if t != fstart && (self.funcs.contains_key(&t) || self.is_tail_target(va, t)) {
                            if self.trial.is_none() {
                                self.add_function(t, Source::TailCall, va);
                                self.tail_sites.insert(va);
                            } else if !self.plausible_target(t) {
                                return;
                            }
                            f.tails.push(t);
                        } else {
                            f.todo.push(t);
                        }
                    }
                    return;
                }
                FlowControl::IndirectBranch => {
                    if let Some(ord) = self.thunk_ordinal(&insn) {
                        self.note_kernel_call(ord, fresh);
                        f.returns |= !NORETURN_KERNEL.contains(&ord);
                    } else if is_table_jump(&insn) {
                        if guard.bounds(insn.memory_index().full_register32()) {
                            self.jump_table(f, fstart, va, &insn, guard);
                        } else {
                            f.unguarded.push((va, insn));
                        }
                    } else {
                        f.returns = true; // tail call through a pointer
                    }
                    return;
                }
                FlowControl::Call => {
                    let t = insn.near_branch32();
                    if self.trial.is_none() {
                        self.add_function(t, Source::Call, va);
                    } else if !self.plausible_target(t) {
                        return;
                    }
                    match self.returns_of(t) {
                        Some(true) => {}
                        Some(false) => return,
                        None if self.trial.as_ref().is_some_and(|t| t.assume_return) => {}
                        None if self.trial.is_some() => return,
                        None => {
                            f.pending.push((va + len as u32, t));
                            return;
                        }
                    }
                }
                FlowControl::IndirectCall => {
                    if let Some(ord) = self.thunk_ordinal(&insn) {
                        self.note_kernel_call(ord, fresh);
                        if NORETURN_KERNEL.contains(&ord) {
                            return;
                        }
                    }
                }
                FlowControl::Return => {
                    f.returns = true;
                    return;
                }
                FlowControl::Interrupt | FlowControl::Exception => {
                    // Calls that never return end their block above, so a
                    // path reaching a trap is suspicious in speculative code.
                    if self.trial.is_some() && matches!(insn.mnemonic(), Mnemonic::Int3 | Mnemonic::Ud2) {
                        self.conflict(va, ConflictKind::FallsIntoTrap);
                    }
                    return;
                }
                FlowControl::XbeginXabortXend => return,
            }
            va += len as u32;
        }
    }

    /// Run every block on `f`'s worklist, then the table jumps no guarded
    /// path reached, which may add more blocks.
    fn explore(&mut self, f: &mut FuncWork, start: u32) {
        loop {
            while let Some(b) = f.todo.pop() {
                self.run_block(f, start, b);
                if self.failed() {
                    return;
                }
            }
            if f.unguarded.is_empty() {
                return;
            }
            for (va, insn) in std::mem::take(&mut f.unguarded) {
                self.jump_table(f, start, va, &insn, Guard::default());
            }
        }
    }

    fn jump_table(&mut self, f: &mut FuncWork, fstart: u32, at: u32, insn: &Instruction, guard: Guard) {
        let index = insn.memory_index().full_register32();
        let table = insn.memory_displacement32();
        let region = self.image.region(at);

        if trace_enabled(at) {
            eprintln!(
                "table at {at:#x}: cmp {:?} bound {:?} byte_index {:?} trial {}",
                guard.cmp,
                guard.bound,
                guard.byte_index,
                self.trial.is_some()
            );
        }
        let mut index_table = None;
        let (count, kind) = match (guard.byte_index, guard.bound) {
            (Some((dst, idx_table, n)), _) if dst == index && n <= MAX_SWITCH => {
                let Ok(idx) = self.xbe.bytes_at(idx_table, n as usize) else {
                    self.conflict(at, ConflictKind::BadJumpTable);
                    return;
                };
                // Bit-flag switches can't reach their low indices, so the
                // compiler aims the table base before the real table, into
                // code. Only the part after the last claimed byte is table.
                let skip = (0..n)
                    .rev()
                    .find(|i| self.image.check_data(idx_table + i, 1, self.overlay()).is_err())
                    .map_or(0, |i| i + 1);
                let entries = idx[skip as usize..].iter().copied().max().map_or(0, |m| m as u32 + 1);
                self.claim_data(idx_table + skip, (n - skip) as usize);
                index_table = Some((idx_table, n));
                (entries, SwitchKind::Indexed)
            }
            (_, Some((r, n))) if r == index && n <= MAX_SWITCH => (n, SwitchKind::Bounded),
            _ => (MAX_UNBOUNDED_SWITCH, SwitchKind::Unbounded),
        };

        let mut read = 0;
        let mut targets = Vec::new();
        let mut holes = Vec::new();
        // Null entries of an unguarded table not yet followed by a target.
        let mut pending = Vec::new();
        for i in 0..count {
            let slot = table + i * 4;
            let target = self.xbe.read_u32(slot).ok();
            let ok_target = target.filter(|t| self.image.region(*t) == region);
            let ok_slot = self.image.check_data(slot, 4, self.overlay()).is_ok();
            // An enum switch with no range check leaves values it never
            // sees as null entries, with targets after them. A short run
            // of nulls is a hole only if a target follows it.
            // A null may already be the next table's.
            let null = kind == SwitchKind::Unbounded && target == Some(0);
            if null && i > 0 && self.in_other_table(at, slot) {
                break;
            }
            if null && ok_slot && pending.len() < MAX_SWITCH_HOLES {
                pending.push(i);
                continue;
            }
            // Past a null, a word that only looks like a code address may
            // be the next table or function, so the table goes on only for
            // a case of its own, in a slot no other table holds and nothing
            // else points at. Otherwise the table ends at the first null and
            // the rest stays unclaimed.
            let past_null = !holes.is_empty() || !pending.is_empty();
            if past_null
                && (self.in_other_table(at, slot)
                    || self.evidenced_start(slot)
                    || !ok_target.is_some_and(|t| self.own_case(fstart, table, slot, &targets, t)))
            {
                break;
            }
            match (kind, ok_target, ok_slot) {
                (_, Some(t), true) => {
                    for h in pending.drain(..) {
                        self.claim_data(table + h * 4, 4);
                        holes.push(h);
                    }
                    self.claim_data(slot, 4);
                    f.todo.push(t);
                    targets.push(t);
                    read = i + 1;
                }
                (SwitchKind::Unbounded, _, _) => break,
                _ => {
                    self.conflict(slot, ConflictKind::BadJumpTable);
                    return;
                }
            }
        }
        // An unguarded table may have entries we stopped short of, so it
        // proves nothing about the function never returning.
        if kind == SwitchKind::Unbounded {
            f.returns = true;
        }
        if read > 0 && self.trial.is_none() {
            self.switches.insert(at, kind);
            let bound = match kind {
                SwitchKind::Unbounded => None,
                SwitchKind::Indexed => index_table.map(|(_, n)| n),
                SwitchKind::Bounded => Some(count),
            };
            self.jump_tables
                .insert(at, JumpTable { site: at, table, kind, count: read, bound, index_table, targets, holes });
        }
    }

    fn overlay(&self) -> Option<&Overlay> {
        self.trial.as_ref().map(|t| &t.overlay)
    }

    fn claim_data(&mut self, va: u32, len: usize) {
        match &mut self.trial {
            Some(t) => t.overlay.claim_data(va, len),
            None => self.image.claim_data(va, len, self.current),
        }
    }

    /// Explore every queued function until all are done, resolving which
    /// calls return as callees finish.
    fn run_certain(&mut self) {
        loop {
            while let Some(s) = self.queue.pop() {
                let Some(mut f) = self.funcs.remove(&s) else { continue };
                self.current = s;
                self.explore(&mut f, s);
                self.funcs.insert(s, f);
            }

            let mut progressed = false;
            let open: Vec<u32> = self.open.iter().copied().collect();
            for k in &open {
                let mut f = self.funcs.remove(k).unwrap();
                self.current = *k;
                for (resume, callee) in std::mem::take(&mut f.pending) {
                    let status = if callee == *k { f.returns.then_some(true) } else { self.returns_of(callee) };
                    match status {
                        Some(true) => {
                            f.todo.push(resume);
                            progressed = true;
                        }
                        Some(false) => progressed = true,
                        None => f.pending.push((resume, callee)),
                    }
                }
                let mut tails_known = true;
                for &t in &f.tails {
                    match if t == *k { Some(f.returns) } else { self.returns_of(t) } {
                        Some(r) => f.returns |= r,
                        None => tails_known = false,
                    }
                }
                if !f.todo.is_empty() {
                    self.queue.push(*k);
                } else if f.pending.is_empty() && tails_known {
                    f.done = true;
                    self.open.remove(k);
                    progressed = true;
                }
                self.funcs.insert(*k, f);
            }

            if self.queue.is_empty() && !progressed && !self.break_cycles() {
                return;
            }
        }
    }

    /// Every open function is waiting on another open one: recursion or
    /// mutual tail calls. Settle each group that waits only on itself (a
    /// strongly connected component with no edges out) by least fixpoint: a
    /// member returns only if it reaches a `ret`, directly or through members
    /// known to return. Resume what that unlocks; if nothing, no member can
    /// return, and the continuations after calls into the group are dead.
    /// Returns whether anything changed.
    fn break_cycles(&mut self) -> bool {
        let open: Vec<u32> = self.open.iter().copied().collect();
        let deps = |f: &FuncWork| -> Vec<u32> { f.pending.iter().map(|p| p.1).chain(f.tails.iter().copied()).collect() };
        let edges: HashMap<u32, Vec<u32>> = open
            .iter()
            .map(|k| (*k, deps(&self.funcs[k]).into_iter().filter(|d| self.open.contains(d)).collect()))
            .collect();
        let mut changed = false;
        for scc in strongly_connected(&open, &edges) {
            let members: HashSet<u32> = scc.iter().copied().collect();
            if scc.iter().any(|k| edges[k].iter().any(|d| !members.contains(d))) {
                continue; // waits on another group; settle that first
            }
            // Spread "returns" through tail calls inside the group.
            loop {
                let returning: HashSet<u32> = scc.iter().copied().filter(|k| self.funcs[k].returns).collect();
                let mut grew = false;
                for k in &scc {
                    let f = self.funcs.get_mut(k).unwrap();
                    if !f.returns && f.tails.iter().any(|t| returning.contains(t)) {
                        f.returns = true;
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            let returning: HashSet<u32> = scc.iter().copied().filter(|k| self.funcs[k].returns).collect();
            let mut resumed = false;
            for k in &scc {
                let f = self.funcs.get_mut(k).unwrap();
                let (go, wait): (Vec<_>, Vec<_>) = f.pending.drain(..).partition(|(_, c)| returning.contains(c));
                f.pending = wait;
                if !go.is_empty() {
                    f.todo.extend(go.into_iter().map(|(resume, _)| resume));
                    self.queue.push(*k);
                    resumed = true;
                }
            }
            if !resumed {
                for k in &scc {
                    let f = self.funcs.get_mut(k).unwrap();
                    f.pending.clear();
                    f.done = true;
                    self.open.remove(k);
                }
            }
            changed = true;
        }
        changed
    }

    /// Explore a speculative function into an overlay; commit only if clean.
    fn try_candidate(&mut self, root: u32, source: Source) -> bool {
        if self.funcs.contains_key(&root) {
            return false;
        }
        if self.blacklist.contains(&root) || self.failed_candidates.contains(&root) {
            return false;
        }
        match self.image.state(root) {
            None => return false,
            Some(Byte::InsnStart) => {
                self.labels.insert(root);
                return false;
            }
            Some(Byte::InsnBody) => {
                *self.rejected.entry((source, ConflictKind::IntoInsnBody)).or_default() += 1;
                return false;
            }
            Some(Byte::Data) => {
                *self.rejected.entry((source, ConflictKind::IntoData)).or_default() += 1;
                return false;
            }
            Some(Byte::Unknown) => {}
        }
        // `add [eax], al` is two zero bytes: a pointer into zero fill.
        if self.image.bytes_from(root).is_some_and(|b| b.starts_with(&[0, 0])) {
            *self.rejected.entry((source, ConflictKind::Implausible)).or_default() += 1;
            self.failed_candidates.insert(root);
            return false;
        }

        // First assume unresolved callees return. If that trips over
        // something, try again assuming they don't: a call to a function that
        // never returns is usually followed by padding or data.
        let mut verdict = Ok(());
        for assume_return in [true, false] {
            verdict = self.trial(root, source, assume_return);
            if verdict.is_ok() {
                break;
            }
        }
        if trace_enabled(root) {
            eprintln!("trial {root:#x} ({source:?}): {verdict:?} (last failure at {:#x})", self.last_fail.0);
        }
        if verdict.is_err() {
            self.contest(root, source);
        }
        if let Err(kind) = verdict {
            *self.rejected.entry((source, kind)).or_default() += 1;
            self.failed_candidates.insert(root);
            return false;
        }

        // Validated: hand it to the certain pass, which works out which of
        // its callees really return and claims the bytes for real.
        self.labels.remove(&root);
        self.funcs.insert(root, FuncWork::new(source, root));
        self.open.insert(root);
        self.queue.push(root);
        // The trial may have assumed an unexplored callee never returns. If
        // exploring for real proves otherwise and the continuation is bad,
        // the candidate was wrong: it's thrown out next round.
        let before = self.conflicts.len();
        self.run_certain();
        // Running into another guess is settled by size, like a contest.
        let root_insns = self.funcs[&root].insns;
        for c in self.conflicts[before..].iter().filter(|c| c.function == root) {
            let rival = c.claimed_by.and_then(|o| self.funcs.get(&o).map(|f| (o, f)));
            let loser = match rival {
                Some((o, f)) if f.source >= Source::CodeRef && f.insns < root_insns => o,
                _ => root,
            };
            if trace_enabled(root) {
                eprintln!("promoted {root:#x} ({source:?}) hit {c:?}; {loser:#x} loses");
            }
            self.losers.push(loser);
        }
        true
    }

    /// A failed trial blocked by bytes a speculative function claimed: try
    /// again as if that function weren't there. If the candidate then passes
    /// and decodes more than its competitor, the competitor loses next round.
    /// Beating a rival with stronger evidence takes an overwhelming margin:
    /// a two-instruction gap-scan guess doesn't evict a one-instruction
    /// function an immediate pointed at, but a whole function does evict a
    /// stray immediate that lands in its prologue.
    fn contest(&mut self, root: u32, source: Source) {
        let (va, span) = self.last_fail;
        let Some(owner) = self.image.owner_in(va, span) else { return };
        let Some(rival) = self.funcs.get(&owner) else { return };
        if owner == root || rival.source < Source::CodeRef || !self.contested.insert((root, owner)) {
            return;
        }
        let (rival_insns, rival_source) = (rival.insns, rival.source);
        self.image.yielding = Some(owner);
        let mut ours = None;
        for assume_return in [true, false] {
            if self.trial(root, source, assume_return).is_ok() {
                ours = Some(self.last_trial_insns);
                break;
            }
        }
        self.image.yielding = None;
        // The candidate contains the rival if it decodes nearly every one of
        // the rival's instructions too: the rival was an entry into the
        // middle of it that fell back in step (an immediate landing on a
        // displacement). Then it wins whatever its evidence.
        let contains = ours.is_some() && {
            let starts = self.image.starts_owned_by(owner);
            let missed = starts.iter().filter(|va| self.last_overlay.bytes.get(va) != Some(&Byte::InsnStart)).count();
            missed <= 2 && starts.len() > missed
        };
        if let Some(n) = ours
            && (contains || n > if source > rival_source { 4 * rival_insns + 8 } else { rival_insns })
        {
            if trace_enabled(root) || trace_enabled(owner) {
                eprintln!("contest {root:#x} ({n} insns) beats {owner:#x} ({rival_insns} insns)");
            }
            self.losers.push(owner);
        }
    }

    fn trial(&mut self, root: u32, source: Source, assume_return: bool) -> Result<(), ConflictKind> {
        self.trial = Some(Trial { assume_return, ..Default::default() });
        self.current = root;
        let mut f = FuncWork::new(source, root);
        self.explore(&mut f, root);
        let t = self.trial.take().unwrap();
        self.last_trial_insns = t.insns;
        self.last_overlay = t.overlay;
        t.failed.map_or(Ok(()), Err)
    }

    /// In a trial, a call or tail-call target must be somewhere a function
    /// could start; fails the trial otherwise.
    fn plausible_target(&mut self, t: u32) -> bool {
        let kind = match self.image.state(t) {
            None => ConflictKind::OutsideCode,
            Some(Byte::InsnBody) => ConflictKind::IntoInsnBody,
            Some(Byte::Data) => ConflictKind::IntoData,
            Some(Byte::Unknown | Byte::InsnStart) => return true,
        };
        self.conflict(t, kind);
        false
    }

    /// Whether a `jmp` from `from` to `t` leaves the function. Splitting a
    /// function by mistake is harmless to the recompiler (all state lives in
    /// `Cpu`, so a jump between functions is just a tail call), so this errs
    /// towards yes.
    fn is_tail_target(&self, from: u32, t: u32) -> bool {
        let byte = |va: u32| {
            (self.image.region(va) == self.image.region(t))
                .then(|| self.xbe.bytes_at(va, 1).ok().map(|b| b[0]))
                .flatten()
        };
        // int3 padding only ever sits between functions.
        let after_padding = byte(t.wrapping_sub(1)) == Some(0xCC);
        // No function in this game spans 64 KB.
        let far = from.abs_diff(t) > 0x1_0000;
        let after_ret = byte(t.wrapping_sub(1)) == Some(0xC3)
            || (byte(t.wrapping_sub(3)) == Some(0xC2) && byte(t.wrapping_sub(1)) == Some(0x00));
        after_padding || far || (t.is_multiple_of(16) && after_ret)
    }

    fn code_refs_to_fixpoint(&mut self) {
        loop {
            let before = self.funcs.len();
            let refs = self.code_refs();
            self.try_all(refs, Source::CodeRef);
            if self.funcs.len() == before {
                return;
            }
        }
    }

    /// Where a compiler would put a function: 16-byte aligned, or straight
    /// after padding, `ret` or `ret n`. True for 97.6% of Ghidra's `.text`
    /// functions, and rarely true of a pointer into the middle of one.
    fn looks_like_start(&self, va: u32) -> bool {
        va.is_multiple_of(16) || self.after_boundary(va)
    }

    /// Straight after padding, `ret`, `ret n`, an instruction nothing
    /// falls through, or a jump table (MSVC puts a function's tables after
    /// its code, and nothing falls out of a table into the next function).
    /// Page-aligned values have to pass this, since being aligned says
    /// nothing about them: `0xFE000` is far more often a size than a
    /// function.
    fn after_boundary(&self, va: u32) -> bool {
        // `va` comes from arbitrary immediates and data words (0 is common),
        // so there may be no byte `n` before it: that is "no evidence", not
        // an address wrapped around the top of the 32-bit space.
        let prev = |n: u32| {
            let at = va.checked_sub(n)?;
            self.image.state(at)?;
            self.xbe.bytes_at(at, 1).ok().map(|b| b[0])
        };
        matches!(prev(1), Some(0xCC | 0x90 | 0xC3))
            || (prev(3) == Some(0xC2) && prev(1) == Some(0x00))
            || (self.image.state(va) != Some(Byte::Data)
                && va.checked_sub(1).is_some_and(|at| self.image.state(at) == Some(Byte::Data)))
            || self.after_terminator(va)
    }

    /// The instruction we decoded just before `va` ends exactly at `va` and
    /// nothing falls through it (jmp, ret, or a call that never returns).
    fn after_terminator(&self, va: u32) -> bool {
        for back in 1..=15u32 {
            let start = va.wrapping_sub(back);
            if self.image.state(start) != Some(Byte::InsnStart) {
                continue;
            }
            let Some(bytes) = self.image.bytes_from(start) else { return false };
            let insn = Decoder::with_ip(32, bytes, start as u64, DecoderOptions::NONE).decode();
            if insn.len() as u32 != back {
                return false;
            }
            return match insn.flow_control() {
                FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::IndirectBranch => true,
                FlowControl::Call => self.funcs.get(&insn.near_branch32()).is_some_and(|f| f.done && !f.returns),
                FlowControl::IndirectCall => {
                    self.thunk_ordinal(&insn).is_some_and(|o| NORETURN_KERNEL.contains(&o))
                }
                _ => false,
            };
        }
        false
    }

    fn try_all(&mut self, candidates: Vec<u32>, source: Source) {
        for c in candidates {
            self.try_candidate(c, source);
        }
    }

    /// Immediate operands in known code that point into code.
    fn code_refs(&mut self) -> Vec<u32> {
        let mut out = BTreeSet::new();
        let scanned = std::mem::take(&mut self.scanned);
        for r in &self.image.regions {
            for (off, b) in r.state.iter().enumerate() {
                // Only code claimed by functions added since the last scan.
                if *b != Byte::InsnStart || scanned.contains(&r.owner[off]) {
                    continue;
                }
                let va = r.va + off as u32;
                let insn = Decoder::with_ip(32, &r.bytes[off..], va as u64, DecoderOptions::NONE).decode();
                for op in 0..insn.op_count() {
                    if insn.op_kind(op) == OpKind::Immediate32 {
                        let v = insn.immediate(op) as u32;
                        // 4 KB-aligned values in code range are usually
                        // sizes or flags (0xC0000, 0xDE000), not addresses.
                        let round = v & 0xFFF == 0;
                        if (!round || self.after_boundary(v))
                            && matches!(self.image.state(v), Some(Byte::Unknown | Byte::InsnStart))
                            && self.looks_like_start(v)
                        {
                            out.insert(v);
                        }
                    }
                }
            }
        }
        self.scanned = scanned;
        self.scanned.extend(self.funcs.keys().copied());
        out.into_iter().collect()
    }

    /// Whether `t`, read from `slot` past a null, can be a case of a switch
    /// in the function at `fstart` with its table at `table`, given the
    /// cases `before` read so far. Compilers put the cases on one side of
    /// the table, between the code and the table, or (if the cases read so
    /// far are all after it) past the table, so a word on the other side is
    /// the next table. No function starts there, and it is not in the middle
    /// of an instruction or in data. A known instruction start is fine,
    /// labels included: a case can be a block another function falls into.
    /// (Data words that hold `t` are no evidence either way: one in DOLBY
    /// holds a case at 0x5a449.)
    fn own_case(&self, fstart: u32, table: u32, slot: u32, before: &[u32], t: u32) -> bool {
        let forward = !before.is_empty() && before.iter().all(|c| *c > table);
        let placed = if forward { t > slot } else { fstart < t && t < table };
        placed && !self.funcs.contains_key(&t) && matches!(self.image.state(t), Some(Byte::Unknown | Byte::InsnStart))
    }

    /// Whether `slot` is in a table already read for a switch other than
    /// the one at `at`: it bounds the table being read.
    fn in_other_table(&self, at: u32, slot: u32) -> bool {
        self.jump_tables.values().any(|j| j.site != at && j.table <= slot && slot - j.table < j.count * 4)
    }

    /// Whether something other than a table points at `va` as code: a known
    /// function or label, or a data word, if what it points at reads as
    /// code. Such a slot starts a function, whatever its bytes look like as
    /// a table entry. (A data word alone is no proof: tables are in code
    /// sections too, and metadata may point into one.)
    fn evidenced_start(&self, va: u32) -> bool {
        self.funcs.contains_key(&va)
            || self.labels.contains(&va)
            || (self.data_words.contains(&va) && self.runs_to_an_exit(va))
    }

    /// Whether straight-line decoding from `va` reaches a `ret` or `jmp`
    /// within a few instructions with nothing a speculative function
    /// couldn't hold on the way: no invalid opcode, `int3`, zero fill or
    /// instruction only data decodes to. Table words (code addresses, so
    /// `xx xx 01 00` and the like) rarely do.
    fn runs_to_an_exit(&self, va: u32) -> bool {
        let Some(bytes) = self.image.bytes_from(va) else { return false };
        let mut decoder = Decoder::with_ip(32, bytes, va as u64, DecoderOptions::NONE);
        for _ in 0..16 {
            let at = decoder.ip() as u32;
            if self.image.bytes_from(at).is_none_or(|b| b.starts_with(&[0, 0])) {
                return false;
            }
            let insn = decoder.decode();
            if insn.code() == Code::INVALID || implausible(&insn) || matches!(insn.mnemonic(), Mnemonic::Int3) {
                return false;
            }
            match insn.flow_control() {
                FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::IndirectBranch => return true,
                FlowControl::Interrupt | FlowControl::Exception => return false,
                _ => {}
            }
        }
        false
    }

    /// Every aligned word in a data section that is a code address.
    fn data_words(&self) -> Result<HashSet<u32>, xbe::Error> {
        let mut out = HashSet::new();
        for s in self.xbe.sections.iter().filter(|s| !is_code_section(s)) {
            let bytes = self.xbe.section_bytes(s)?;
            out.extend(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|w| u32::from_le_bytes(*w))
                    .filter(|v| self.image.region(*v).is_some()),
            );
        }
        Ok(out)
    }

    /// Aligned words in data sections that point into code. Runs of two or
    /// more (vtables, callback tables) come first.
    fn data_pointers(&self) -> Result<Vec<u32>, xbe::Error> {
        let mut runs = Vec::new();
        let mut singles = Vec::new();
        for s in self.xbe.sections.iter().filter(|s| !is_code_section(s)) {
            let words: Vec<u32> = self
                .xbe
                .section_bytes(s)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w))
                .collect();
            let is_code = |v: u32| self.image.region(v).is_some();
            for (i, &v) in words.iter().enumerate() {
                // 4 KB-aligned words are usually sizes or flags.
                if !is_code(v) || (v & 0xFFF == 0 && !self.after_boundary(v)) {
                    continue;
                }
                let neighbour = (i > 0 && is_code(words[i - 1])) || words.get(i + 1).is_some_and(|w| is_code(*w));
                if neighbour { runs.push(v) } else { singles.push(v) }
            }
        }
        runs.extend(singles);
        let mut seen = HashSet::new();
        runs.retain(|v| seen.insert(*v) && self.looks_like_start(*v));
        Ok(runs)
    }

    /// Starts of unknown runs that look like a compiler put a function there:
    /// after padding, after a `ret`, or 16-byte aligned.
    fn gap_starts(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for r in &self.image.regions {
            let mut i = 0;
            while i < r.state.len() {
                if r.state[i] != Byte::Unknown {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < r.state.len() && r.state[i] == Byte::Unknown {
                    i += 1;
                }
                let s = start + padding_len(&r.bytes[start..i], r.va + start as u32);
                if s == i || s >= r.raw {
                    continue;
                }
                let va = r.va + s as u32;
                if s > start || self.looks_like_start(va) {
                    out.push(va);
                }
            }
        }
        out
    }

    fn finish(self, kernel_imports: usize) -> Analysis {
        let mut sections = Vec::new();
        let mut gaps = Vec::new();
        for r in &self.image.regions {
            let mut c = SectionCoverage { name: r.name.clone(), size: r.bytes.len(), ..Default::default() };
            let mut i = 0;
            while i < r.state.len() {
                match r.state[i] {
                    Byte::InsnStart | Byte::InsnBody => c.code += 1,
                    Byte::Data => c.data += 1,
                    Byte::Unknown => {
                        // Runs stop where the file's bytes do: the zero
                        // fill after them is not code anyone left
                        // unexplained.
                        let start = i;
                        let end = if start < r.raw { r.raw } else { r.state.len() };
                        while i < end && r.state[i] == Byte::Unknown {
                            i += 1;
                        }
                        let run = &r.bytes[start..i];
                        if start >= r.raw || padding_len(run, r.va + start as u32) == run.len() {
                            c.padding += run.len();
                        } else {
                            c.unknown += run.len();
                            gaps.push((r.va + start as u32, run.len()));
                        }
                        continue;
                    }
                }
                i += 1;
            }
            c.functions = self.funcs.range(r.va..r.va + r.bytes.len() as u32).count();
            sections.push(c);
        }
        gaps.sort_by_key(|g| std::cmp::Reverse(g.1));
        gaps.truncate(20);

        let section_of = |va: u32| {
            self.image.region(va).map(|i| self.image.regions[i].name.clone()).unwrap_or_default()
        };
        let functions = self
            .funcs
            .iter()
            .map(|(va, f)| {
                (*va, Function { start: *va, source: f.source, section: section_of(*va), returns: f.returns })
            })
            .collect();
        let labels = self.labels.iter().copied().filter(|l| !self.funcs.contains_key(l)).collect();
        let pointers = self.pointer_census();
        let mut conflicts = self.conflicts;
        conflicts.sort_by_key(|c| c.va);
        conflicts.dedup_by_key(|c| (c.va, c.kind));

        Analysis {
            functions,
            sections,
            switches: self.switches,
            jump_tables: self.jump_tables,
            conflicts,
            rejected: self.rejected,
            labels,
            kernel_calls: self.kernel_calls,
            kernel_imports,
            gaps,
            ranges: self.image.regions.iter().map(|r| (r.va, r.bytes.len() as u32, r.name.clone())).collect(),
            blacklisted: 0,
            rounds: 1,
            converged: true,
            claims: self.image.regions.into_iter().map(|r| (r.va, r.state, r.owner)).collect(),
            losers: self.losers,
            tail_sites: self.tail_sites,
            pointers,
        }
    }

    fn pointer_census(&self) -> PointerCensus {
        let mut c = PointerCensus::default();
        let mut seen = HashSet::new();
        for s in self.xbe.sections.iter().filter(|s| !is_code_section(s)) {
            let Ok(bytes) = self.xbe.section_bytes(s) else { continue };
            for w in bytes.as_chunks::<4>().0 {
                let v = u32::from_le_bytes(*w);
                let Some(state) = self.image.state(v) else { continue };
                if !seen.insert(v) {
                    continue;
                }
                let slot = match state {
                    Byte::InsnStart if self.funcs.contains_key(&v) => &mut c.entries,
                    Byte::InsnStart => &mut c.interior,
                    Byte::InsnBody => &mut c.bodies,
                    Byte::Data => &mut c.data,
                    Byte::Unknown => {
                        let r = &self.image.regions[self.image.region(v).unwrap()];
                        let off = (v - r.va) as usize;
                        let run = r.state[off..].iter().take_while(|b| **b == Byte::Unknown).count();
                        let end = off + run;
                        let pad = off >= r.raw || padding_len(&r.bytes[off..end], v) == end - off;
                        if pad { &mut c.padding } else { &mut c.unknown }
                    }
                };
                *slot += 1;
            }
        }
        c
    }
}

/// `H2_TRACE=0x81780,0x1234` prints what happens to those candidates.
fn trace_enabled(va: u32) -> bool {
    static LIST: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        std::env::var("H2_TRACE")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
            .collect()
    })
    .contains(&va)
}

/// Strongly connected components of `nodes` (Tarjan's algorithm, iterative
/// so long call chains can't overflow the stack).
fn strongly_connected(nodes: &[u32], edges: &HashMap<u32, Vec<u32>>) -> Vec<Vec<u32>> {
    let mut index: HashMap<u32, usize> = HashMap::new();
    let mut low: HashMap<u32, usize> = HashMap::new();
    let mut on_stack: HashSet<u32> = HashSet::new();
    let mut stack = Vec::new();
    let mut out = Vec::new();
    let mut next = 0;
    for &root in nodes {
        if index.contains_key(&root) {
            continue;
        }
        // (node, next edge to look at)
        let mut work = vec![(root, 0usize)];
        while let Some(&mut (v, ref mut ei)) = work.last_mut() {
            if *ei == 0 && !index.contains_key(&v) {
                index.insert(v, next);
                low.insert(v, next);
                next += 1;
                stack.push(v);
                on_stack.insert(v);
            }
            let succ = edges.get(&v).map_or(&[][..], |e| e.as_slice());
            if let Some(&w) = succ.get(*ei) {
                *ei += 1;
                if !index.contains_key(&w) {
                    work.push((w, 0));
                } else if on_stack.contains(&w) {
                    let l = low[&v].min(index[&w]);
                    low.insert(v, l);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                let l = low[&parent].min(low[&v]);
                low.insert(parent, l);
            }
            if low[&v] == index[&v] {
                let mut scc = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack.remove(&w);
                    scc.push(w);
                    if w == v {
                        break;
                    }
                }
                out.push(scc);
            }
        }
    }
    out
}

/// The code selector of the flat ring-0 model: a far jump to it only
/// reloads CS, so it is a direct jump to its offset.
pub const FLAT_CS: u16 = 0x08;

/// Where an unconditional `jmp` with a direct operand goes: a near target,
/// or the offset of a far jump to [`FLAT_CS`].
pub fn direct_jump_target(insn: &Instruction) -> Option<u32> {
    match insn.op0_kind() {
        OpKind::NearBranch32 => Some(insn.near_branch32()),
        OpKind::FarBranch32 if insn.far_branch_selector() == FLAT_CS => Some(insn.far_branch32()),
        _ => None,
    }
}

fn is_table_jump(insn: &Instruction) -> bool {
    insn.op0_kind() == OpKind::Memory
        && insn.memory_base() == Register::None
        && insn.memory_index() != Register::None
        && insn.memory_index_scale() == 4
}

/// Instructions only data decodes to, as far as a speculative function is
/// concerned: Xbox titles run flat 32-bit code in ring 0, so they never
/// push, pop or load ES/CS/SS/DS, make far transfers, or do string port
/// I/O. (Code found from certain evidence is not held to this: the XAPI's
/// GDT helper does a far `jmp` on purpose.) Plain `in`/`out` are not on the
/// list, as D3D programs a GPIO port with them, and neither is `mov` from a
/// segment register: the CRT memmove's unread jump table at 0x320a80
/// decodes as one, and that decode is what recovers the copy tails after
/// it until switches masked with `and` are sized.
fn implausible(insn: &Instruction) -> bool {
    let sreg = |r: Register| matches!(r, Register::ES | Register::CS | Register::SS | Register::DS);
    let far = (0..insn.op_count()).any(|i| matches!(insn.op_kind(i), OpKind::FarBranch16 | OpKind::FarBranch32))
        || matches!(insn.mnemonic(), Mnemonic::Retf | Mnemonic::Iret | Mnemonic::Iretd)
        || matches!(insn.code(), Code::Jmp_m1632 | Code::Call_m1632 | Code::Jmp_m1616 | Code::Call_m1616);
    let segment = (matches!(insn.mnemonic(), Mnemonic::Push | Mnemonic::Pop) && sreg(insn.op0_register()))
        || matches!(insn.mnemonic(), Mnemonic::Lds | Mnemonic::Les | Mnemonic::Lss);
    let port_string = matches!(
        insn.mnemonic(),
        Mnemonic::Insb | Mnemonic::Insw | Mnemonic::Insd | Mnemonic::Outsb | Mnemonic::Outsw | Mnemonic::Outsd
    );
    far || segment || port_string
}

fn is_padding(b: u8) -> bool {
    matches!(b, 0xCC | 0x90 | 0x00)
}

/// Length of a do-nothing instruction compilers use to align loop heads:
/// `nop`, or `lea r, [r+0]` (`8d 49 00`, `8d 64 24 00`, `8d 9b 00000000`,
/// `8d a4 24 00000000`). `mov edi, edi` isn't counted: it starts
/// hot-patchable functions.
fn nop_len(bytes: &[u8], va: u32) -> usize {
    let insn = Decoder::with_ip(32, bytes, va as u64, DecoderOptions::NONE).decode();
    let is_nop = match insn.mnemonic() {
        Mnemonic::Nop => true,
        Mnemonic::Lea => {
            insn.op0_kind() == OpKind::Register
                && insn.memory_base() == insn.op0_register()
                && insn.memory_index() == Register::None
                && insn.memory_displacement32() == 0
        }
        _ => false,
    };
    if is_nop { insn.len() } else { 0 }
}

/// Bytes from `va` that are padding: int3/nop/zero fill or alignment no-ops.
fn padding_len(bytes: &[u8], va: u32) -> usize {
    let mut i = 0;
    while i < bytes.len() {
        if is_padding(bytes[i]) {
            i += 1;
            continue;
        }
        match nop_len(&bytes[i..], va + i as u32) {
            0 => break,
            n if i + n <= bytes.len() => i += n,
            _ => break,
        }
    }
    i
}

impl Analysis {
    /// What a code byte was claimed as ('?' unknown, 'I' instruction start,
    /// 'i' instruction body, 'd' data) and by which function.
    pub fn claim(&self, va: u32) -> Option<(char, Option<u32>)> {
        let (base, state, owner) = self.claims.iter().find(|(b, s, _)| va >= *b && va - *b < s.len() as u32)?;
        let i = (va - base) as usize;
        let c = match state[i] {
            Byte::Unknown => '?',
            Byte::InsnStart => 'I',
            Byte::InsnBody => 'i',
            Byte::Data => 'd',
        };
        Some((c, (owner[i] != 0).then_some(owner[i])))
    }

    /// Name of the code section containing `va`.
    pub fn section_of(&self, va: u32) -> Option<&str> {
        self.ranges.iter().find(|(s, len, _)| va >= *s && va - *s < *len).map(|(_, _, n)| n.as_str())
    }

    pub fn total(&self) -> SectionCoverage {
        let mut t = SectionCoverage { name: "total".into(), ..Default::default() };
        for s in &self.sections {
            t.size += s.size;
            t.code += s.code;
            t.data += s.data;
            t.padding += s.padding;
            t.unknown += s.unknown;
            t.functions += s.functions;
        }
        t
    }
}

impl fmt::Display for Analysis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{:<10} {:>9} {:>9} {:>8} {:>8} {:>9} {:>7} {:>7}",
            "section", "size", "code", "data", "pad", "unknown", "funcs", "known"
        )?;
        for s in self.sections.iter().chain(std::iter::once(&self.total())) {
            writeln!(
                f,
                "{:<10} {:>9} {:>9} {:>8} {:>8} {:>9} {:>7} {:>6.2}%",
                s.name, s.size, s.code, s.data, s.padding, s.unknown, s.functions, s.percent()
            )?;
        }
        let mut by_source: BTreeMap<Source, usize> = BTreeMap::new();
        for func in self.functions.values() {
            *by_source.entry(func.source).or_default() += 1;
        }
        let noreturn = self.functions.values().filter(|f| !f.returns).count();
        let mut switch_kinds: BTreeMap<SwitchKind, usize> = BTreeMap::new();
        for k in self.switches.values() {
            *switch_kinds.entry(*k).or_default() += 1;
        }
        let mut conflict_kinds: BTreeMap<ConflictKind, usize> = BTreeMap::new();
        for c in &self.conflicts {
            *conflict_kinds.entry(c.kind).or_default() += 1;
        }
        writeln!(f)?;
        writeln!(f, "functions by source: {by_source:?}")?;
        writeln!(f, "never return: {noreturn}")?;
        writeln!(f, "labels (address-taken code inside functions): {}", self.labels.len())?;
        let p = &self.pointers;
        writeln!(
            f,
            "data pointers into code: {} entries, {} interior, {} mid-instruction, {} into data, {} padding, {} unknown",
            p.entries, p.interior, p.bodies, p.data, p.padding, p.unknown
        )?;
        writeln!(f, "switches: {switch_kinds:?}")?;
        writeln!(f, "blacklisted speculative starts: {} (after {} rounds)", self.blacklisted, self.rounds)?;
        if !self.converged {
            writeln!(f, "WARNING: did not settle in {MAX_ROUNDS} rounds; some speculative functions are known bad")?;
        }
        writeln!(f, "conflicts: {} {conflict_kinds:?}", self.conflicts.len())?;
        for c in self.conflicts.iter().take(40) {
            let by = c.claimed_by.map_or(String::new(), |o| {
                let src = self.functions.get(&o).map_or("?".to_owned(), |f| format!("{:?}", f.source));
                format!(", already claimed by {o:#010x} ({src})")
            });
            writeln!(f, "  {:#010x} {:?} exploring {:#010x}{by}", c.va, c.kind, c.function)?;
        }
        writeln!(f, "speculative rejections:")?;
        for ((src, kind), n) in &self.rejected {
            writeln!(f, "  {src:?} {kind:?}: {n}")?;
        }
        writeln!(
            f,
            "kernel: {} imports, {} called directly from {} sites",
            self.kernel_imports,
            self.kernel_calls.len(),
            self.kernel_calls.values().sum::<usize>()
        )?;
        writeln!(f, "largest unknown runs:")?;
        for (va, len) in self.gaps.iter().take(10) {
            writeln!(f, "  {va:#010x} {len:>7} bytes")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
