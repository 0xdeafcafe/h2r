//! What floating point an XBE uses, as measured over the instructions the
//! function analysis claimed.
//!
//! Counts x87, SSE1 (scalar, packed, the MMX-register integer extensions and
//! state/cacheability), MMX and anything beyond a Pentium III by mnemonic and
//! by owning function. On top of the counts it looks for the details a
//! lifter has to get right:
//!
//! - every `fldcw`/`ldmxcsr`, with the value it loads where a short backward
//!   scan can tell (`fnstcw [m]; mov ax, [m]; or ah, 0Ch; mov [m2], ax;
//!   fldcw [m2]` resolves to "saved | 0x0c00", i.e. round toward zero);
//! - `fnstsw` and what consumes the status word (`sahf`, `test ah, imm`);
//! - 80-bit and BCD memory operands, x87 environment save/restore, and
//!   transcendentals;
//! - a static x87 stack-depth walk per function: entry depth the body
//!   needs, depth at every `ret`, at every call, and whether two paths
//!   reach one instruction with different depths.
//!
//! Everything here is static. The depth walk follows direct branches and the
//! analyzer's jump tables, not indirect jumps it cannot resolve, so its
//! coverage is reported alongside its results.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use iced_x86::{
    CpuidFeature, Decoder, DecoderOptions, FlowControl, Instruction, MemorySize, Mnemonic, OpKind,
    Register,
};
use xbe::Xbe;

use crate::{Analysis, is_code_section};

/// Instructions looked back over when resolving a control word's value.
const WINDOW: usize = 24;
/// Instructions looked ahead past a call for the first x87 use of its result.
const LOOKAHEAD: usize = 8;
/// Steps one function's depth walk may take.
const WALK_LIMIT: usize = 200_000;
/// Rounds of the interprocedural stack-effect fixpoint before giving up.
const MAX_ROUNDS: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    X87,
    /// `*ss` arithmetic, compares, moves and `cvt*ss*` conversions.
    SseScalar,
    /// `*ps` arithmetic, logic, shuffles, moves and `cvt*ps*` conversions.
    SsePacked,
    /// SSE1's new integer instructions on MMX registers (`pshufw`, `pminub`,
    /// `movntq`, ...).
    SseMmxInt,
    /// `ldmxcsr`, `stmxcsr`, `fxsave`, `fxrstor`, `sfence`, `prefetch*`.
    SseState,
    Mmx,
    /// SSE2 or later, 3DNow!: not on a Pentium III, so either dead
    /// CPU-dispatch code or data decoded as code.
    Beyond,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::X87 => "x87",
            Class::SseScalar => "sse scalar",
            Class::SsePacked => "sse packed",
            Class::SseMmxInt => "sse mmx-int",
            Class::SseState => "sse state",
            Class::Mmx => "mmx",
            Class::Beyond => "beyond piii",
        }
    }

    fn is_sse(self) -> bool {
        matches!(
            self,
            Class::SseScalar | Class::SsePacked | Class::SseMmxInt | Class::SseState
        )
    }
}

/// Where a control-word or MXCSR value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Value {
    /// An immediate built up in registers or memory.
    Const(u32),
    /// The value saved by an earlier `fnstcw`/`stmxcsr`, then `(saved & and) | or`.
    Saved {
        and: u32,
        or: u32,
    },
    /// A global; `initial` is its value in the image if it has one, and
    /// `written` says whether any code stores to it.
    Global {
        va: u32,
        initial: Option<u32>,
        written: bool,
    },
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct Site {
    pub va: u32,
    pub owner: u32,
    pub value: Value,
}

#[derive(Debug, Clone, Default)]
pub struct FnFp {
    pub counts: BTreeMap<Class, usize>,
}

impl FnFp {
    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }
    fn get(&self, c: Class) -> usize {
        self.counts.get(&c).copied().unwrap_or(0)
    }
    fn sse(&self) -> usize {
        self.counts
            .iter()
            .filter(|(c, _)| c.is_sse())
            .map(|(_, n)| n)
            .sum()
    }
}

/// How much a stack-effect result rests on assumptions. Ordered from most
/// to least trustworthy; a function takes the worst grade of anything it
/// relied on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Grade {
    /// Every call went to a function whose own effect is proven, and no
    /// conditional push was assumed to happen.
    #[default]
    Proven,
    /// Relied on the MSVC convention at an unknown or indirect callee with an
    /// empty x87 stack and no result read, or on `fptan`/`fsincos` pushing
    /// (in-range operand).
    Convention,
    /// Guessed an unknown callee's arguments or result from the code around
    /// the call.
    Heuristic,
}

/// A callee's effect on its caller's stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    /// Slots it consumes from the caller.
    pub need: i32,
    /// Net change in depth.
    pub delta: i32,
    pub grade: Grade,
}

/// One function's x87 stack-depth walk. Depths are relative to entry.
#[derive(Debug, Clone, Default)]
pub struct FnStack {
    /// x87 registers the body reads before it pushes them: values passed in.
    pub entry_need: i32,
    pub ret_depths: BTreeSet<i32>,
    pub call_depths: BTreeSet<i32>,
    pub max_depth: i32,
    /// Two paths reached one instruction with different depths.
    pub inconsistent: Option<u32>,
    /// `frstor`/`fldenv`/`fxrstor` set TOP from memory.
    pub dynamic_top: usize,
    pub indirect_jumps: usize,
    pub x87_reached: usize,
    /// Calls whose result arity came from the callee's own walk and
    /// disagreed with what the caller's next instructions suggest.
    pub arity_disagreements: usize,
    pub truncated: bool,
    /// Tail jumps into functions whose own effect is unknown.
    pub unresolved_tails: usize,
    /// Calls to callees with no known effect: (depth relative to entry,
    /// result slots read after the call).
    pub guessed_calls: Vec<(i32, i32)>,
    /// Worst grade among the callee effects it applied.
    pub callee_grade: Grade,
    /// `fptan`/`fsincos` reached: they push only when the operand is in range.
    pub conditional_pushes: usize,
    /// MMX instructions in a function that also uses x87: MMX sets TOP to 0
    /// and every tag valid, `emms` empties them, so slot tracking breaks.
    pub mmx_mixed: bool,
}

impl FnStack {
    /// Every check the walk makes passed: one depth per instruction, no
    /// dynamic TOP, no unresolved jump or tail, within 0..=8 slots, no MMX
    /// aliasing, at most one return depth.
    pub fn resolved(&self) -> bool {
        self.inconsistent.is_none()
            && self.dynamic_top == 0
            && self.indirect_jumps == 0
            && self.unresolved_tails == 0
            && !self.truncated
            && !self.mmx_mixed
            && self.entry_need <= 8
            && self.max_depth + self.entry_need <= 8
            && self.ret_depths.len() <= 1
    }

    pub fn grade(&self) -> Grade {
        let mut g = self.callee_grade;
        if self.conditional_pushes > 0 {
            g = g.max(Grade::Convention);
        }
        for &(d, result) in &self.guessed_calls {
            let abs = d + self.entry_need;
            g = g.max(if abs == 0 && result == 0 {
                Grade::Convention
            } else {
                Grade::Heuristic
            });
        }
        g
    }

    /// What a call to it does to the caller's stack, if it is resolved and
    /// returns at exactly one depth.
    pub fn effect(&self) -> Option<Effect> {
        if !self.resolved() || self.ret_depths.len() != 1 {
            return None;
        }
        self.ret_depths.first().map(|d| Effect {
            need: self.entry_need,
            delta: *d,
            grade: self.grade(),
        })
    }

    /// x87 values it leaves on the stack when it returns, if every return agrees.
    pub fn ret_arity(&self) -> Option<i32> {
        self.effect().map(|e| e.need + e.delta)
    }
}

#[derive(Default)]
pub struct FpStats {
    pub insns: usize,
    pub by_class: BTreeMap<Class, usize>,
    /// Class counts per XBE section (`.text`, `D3D`, `DSOUND`, `WMADEC`, ...).
    pub by_section: BTreeMap<String, BTreeMap<Class, usize>>,
    pub by_mnemonic: BTreeMap<(Class, String), usize>,
    /// The same, counting only functions in `reach_calls` and `reach_refs`.
    pub by_mnemonic_reach: BTreeMap<(Class, String), (usize, usize)>,
    /// Functions with any floating-point or SIMD instruction; 0 collects
    /// instructions with no owner.
    pub functions: BTreeMap<u32, FnFp>,
    pub fldcw: Vec<Site>,
    pub ldmxcsr: Vec<Site>,
    /// Status-word reads and the instruction that consumes them.
    pub status_reads: BTreeMap<String, Vec<u32>>,
    /// 80-bit real and BCD memory operands, by instruction.
    pub mem80: BTreeMap<String, Vec<u32>>,
    /// x87 memory operands by size: how much is float, double, integer.
    pub x87_mem: BTreeMap<String, usize>,
    pub env: BTreeMap<String, usize>,
    pub transcendental: BTreeMap<String, usize>,
    /// Direct call sites into functions that contain an `fldcw`.
    pub fldcw_fn_callers: BTreeMap<u32, usize>,
    /// Calls into `fldcw` functions and their short wrappers, by callee and
    /// the first two pushed arguments where they are constants: call sites.
    pub cw_calls: BTreeMap<(u32, Option<u32>, Option<u32>), Vec<u32>>,
    pub stacks: BTreeMap<u32, FnStack>,
    /// Rounds the stack-effect fixpoint took, and whether it settled.
    pub rounds: usize,
    pub converged: bool,
    /// Direct call sites into each function, for the x87 helper report.
    pub callers: HashMap<u32, usize>,
    /// Instructions each function owns.
    pub sizes: HashMap<u32, usize>,
    /// Functions reachable from the entry point through direct calls and
    /// jumps only, and through those plus immediates naming a function
    /// (`push offset f`: thread routines, callbacks). Lower bounds on what
    /// runs: virtual calls and data tables are not followed.
    pub reach_calls: BTreeSet<u32>,
    pub reach_refs: BTreeSet<u32>,
    /// Owning function of each floating-point instruction.
    pub owners: HashMap<u32, u32>,
}

struct Insn {
    ins: Instruction,
    owner: u32,
    class: Option<Class>,
}

/// `roots` are extra starting points for the direct-call reachability
/// split, besides the entry point (a game-main address, say).
pub fn fpstats(xbe: &Xbe, a: &Analysis, roots: &[u32]) -> Result<FpStats, xbe::Error> {
    let mut insns: Vec<Insn> = Vec::new();
    for s in xbe.sections.iter().filter(|s| is_code_section(s)) {
        let bytes = xbe.section_bytes(s)?;
        let bytes = &bytes[..bytes.len().min(s.vsize as usize)];
        let mut dec = Decoder::with_ip(32, bytes, s.va as u64, DecoderOptions::NONE);
        for off in 0..bytes.len() {
            let va = s.va + off as u32;
            let Some(('I', owner)) = a.claim(va) else {
                continue;
            };
            dec.set_position(off).expect("offset is in the section");
            dec.set_ip(va as u64);
            let ins = dec.decode();
            if ins.is_invalid() {
                continue;
            }
            let class = classify(&ins);
            insns.push(Insn {
                ins,
                owner: owner.unwrap_or(0),
                class,
            });
        }
    }
    let index: HashMap<u32, usize> = insns
        .iter()
        .enumerate()
        .map(|(i, x)| (x.ins.ip32(), i))
        .collect();

    let mut st = FpStats {
        insns: insns.len(),
        ..Default::default()
    };
    let written_globals = written_absolute(&insns);
    for (i, x) in insns.iter().enumerate() {
        let ins = &x.ins;
        let Some(class) = x.class else { continue };
        let m = mnemonic(ins);
        *st.by_class.entry(class).or_default() += 1;
        let sec = a.section_of(ins.ip32()).unwrap_or("?").to_owned();
        *st.by_section
            .entry(sec)
            .or_default()
            .entry(class)
            .or_default() += 1;
        *st.by_mnemonic.entry((class, m.clone())).or_default() += 1;
        *st.functions
            .entry(x.owner)
            .or_default()
            .counts
            .entry(class)
            .or_default() += 1;
        st.owners.insert(ins.ip32(), x.owner);

        match ins.mnemonic() {
            Mnemonic::Fldcw | Mnemonic::Ldmxcsr => {
                let width = if ins.mnemonic() == Mnemonic::Fldcw {
                    0xffff
                } else {
                    u32::MAX
                };
                let value = resolve_mem(xbe, &insns, i, mem_key(ins), width, &written_globals, 0);
                let site = Site {
                    va: ins.ip32(),
                    owner: x.owner,
                    value,
                };
                if ins.mnemonic() == Mnemonic::Fldcw {
                    st.fldcw.push(site)
                } else {
                    st.ldmxcsr.push(site)
                }
            }
            Mnemonic::Fnstsw | Mnemonic::Fstsw => {
                let key = if ins.op0_kind() == OpKind::Register {
                    match status_consumer(&insns, i).map(|n| &n.ins) {
                        Some(n) if n.mnemonic() == Mnemonic::Sahf => "fnstsw ax; sahf".to_owned(),
                        Some(n)
                            if matches!(
                                n.mnemonic(),
                                Mnemonic::Test | Mnemonic::And | Mnemonic::Cmp
                            ) =>
                        {
                            let r = if n.op0_kind() == OpKind::Register {
                                n.op0_register()
                            } else {
                                Register::None
                            };
                            let imm =
                                (n.op_count() > 1 && is_imm(n.op1_kind())).then(|| n.immediate(1));
                            match imm {
                                Some(v) => {
                                    format!("fnstsw ax; {} {}, {v:#x}", mnemonic(n), reg_name(r))
                                }
                                None => format!("fnstsw ax; {} (non-immediate)", mnemonic(n)),
                            }
                        }
                        Some(n) => format!("fnstsw ax; then {}", mnemonic(n)),
                        None => "fnstsw ax; end of code".to_owned(),
                    }
                } else {
                    "fnstsw m16".to_owned()
                };
                st.status_reads.entry(key).or_default().push(ins.ip32());
            }
            Mnemonic::Fsin
            | Mnemonic::Fcos
            | Mnemonic::Fsincos
            | Mnemonic::Fptan
            | Mnemonic::Fpatan
            | Mnemonic::Fyl2x
            | Mnemonic::Fyl2xp1
            | Mnemonic::F2xm1
            | Mnemonic::Fsqrt
            | Mnemonic::Fscale
            | Mnemonic::Fprem
            | Mnemonic::Fprem1
            | Mnemonic::Frndint
            | Mnemonic::Fxtract => *st.transcendental.entry(m.clone()).or_default() += 1,
            Mnemonic::Fnsave
            | Mnemonic::Fsave
            | Mnemonic::Frstor
            | Mnemonic::Fnstenv
            | Mnemonic::Fstenv
            | Mnemonic::Fldenv
            | Mnemonic::Fxsave
            | Mnemonic::Fxrstor
            | Mnemonic::Fninit
            | Mnemonic::Finit
            | Mnemonic::Fnclex
            | Mnemonic::Fclex
            | Mnemonic::Emms
            | Mnemonic::Femms
            | Mnemonic::Ffree
            | Mnemonic::Ffreep
            | Mnemonic::Fincstp
            | Mnemonic::Fdecstp => *st.env.entry(m.clone()).or_default() += 1,
            _ => {}
        }
        if class == Class::X87 && mem_key(ins).is_some() {
            *st.x87_mem
                .entry(format!("{:?}", ins.memory_size()).to_ascii_lowercase())
                .or_default() += 1;
        }
        if matches!(ins.memory_size(), MemorySize::Float80 | MemorySize::Bcd) {
            st.mem80
                .entry(format!("{m} m80"))
                .or_default()
                .push(ins.ip32());
        }
    }

    let fldcw_fns: BTreeSet<u32> = st
        .fldcw
        .iter()
        .map(|s| s.owner)
        .filter(|o| *o != 0)
        .collect();
    for x in &insns {
        if x.ins.flow_control() == FlowControl::Call
            && matches!(x.ins.op0_kind(), OpKind::NearBranch32)
        {
            let t = x.ins.near_branch32();
            if fldcw_fns.contains(&t) {
                *st.fldcw_fn_callers.entry(t).or_default() += 1;
            }
        }
    }

    // Short functions that call one of those: `_controlfp` wrapping
    // `_control87`, say.
    let mut owned_len: HashMap<u32, usize> = HashMap::new();
    for x in &insns {
        *owned_len.entry(x.owner).or_default() += 1;
    }
    for x in &insns {
        if let Some(t) = direct_call(&x.ins) {
            *st.callers.entry(t).or_default() += 1;
        }
    }
    let mut cw_fns = fldcw_fns.clone();
    for x in &insns {
        if direct_call(&x.ins).is_some_and(|t| fldcw_fns.contains(&t))
            && owned_len.get(&x.owner).is_some_and(|n| *n <= 16)
        {
            cw_fns.insert(x.owner);
        }
    }
    st.sizes = owned_len.clone();
    let mut from: Vec<u32> = vec![xbe.entry];
    from.extend(roots);
    st.reach_calls = reach(&from, a, &insns, false);
    st.reach_refs = reach(&[xbe.entry], a, &insns, true);
    for x in &insns {
        if let Some(c) = x.class {
            let e = st
                .by_mnemonic_reach
                .entry((c, mnemonic(&x.ins)))
                .or_default();
            e.0 += usize::from(st.reach_calls.contains(&x.owner));
            e.1 += usize::from(st.reach_refs.contains(&x.owner));
        }
    }
    for (i, x) in insns.iter().enumerate() {
        if let Some(t) = direct_call(&x.ins).filter(|t| cw_fns.contains(t)) {
            let (a1, a2) = pushed_args(xbe, &insns, i, &written_globals);
            st.cw_calls
                .entry((t, a1, a2))
                .or_default()
                .push(x.ins.ip32());
        }
    }

    // Iterate to a fixpoint: a round's effects feed the next round's calls.
    let mut effects: HashMap<u32, Effect> = HashMap::new();
    for round in 1..=MAX_ROUNDS {
        let mut stacks = BTreeMap::new();
        for f in a.functions.keys() {
            let s = walk(*f, a, xbe, &insns, &index, &effects);
            stacks.insert(*f, s);
        }
        let next: HashMap<u32, Effect> = stacks
            .iter()
            .filter_map(|(f, s)| s.effect().map(|e| (*f, e)))
            .collect();
        st.stacks = stacks;
        st.rounds = round;
        if next == effects {
            st.converged = true;
            break;
        }
        effects = next;
    }
    Ok(st)
}

fn reach(roots: &[u32], a: &Analysis, insns: &[Insn], refs: bool) -> BTreeSet<u32> {
    let mut edges: HashMap<u32, Vec<u32>> = HashMap::new();
    for x in insns {
        let i = &x.ins;
        let mut add = |t: u32| {
            if t != x.owner && a.functions.contains_key(&t) {
                edges.entry(x.owner).or_default().push(t);
            }
        };
        if matches!(i.op0_kind(), OpKind::NearBranch32) {
            add(i.near_branch32());
        }
        if refs {
            for k in 0..i.op_count() {
                if is_imm(i.op_kind(k)) {
                    add(i.immediate(k) as u32);
                }
            }
        }
    }
    let mut seen: BTreeSet<u32> = roots.iter().copied().collect();
    let mut todo = roots.to_vec();
    while let Some(f) = todo.pop() {
        for t in edges.get(&f).into_iter().flatten() {
            if seen.insert(*t) {
                todo.push(*t);
            }
        }
    }
    seen
}

fn direct_call(i: &Instruction) -> Option<u32> {
    (i.flow_control() == FlowControl::Call && i.op0_kind() == OpKind::NearBranch32)
        .then(|| i.near_branch32())
}

/// The instruction that reads AX after `fnstsw ax` at `at`, skipping x87
/// and SSE work and moves that leave EAX alone.
fn status_consumer(insns: &[Insn], at: usize) -> Option<&Insn> {
    for x in insns.iter().skip(at + 1).take(5) {
        let i = &x.ins;
        let other_mov = i.mnemonic() == Mnemonic::Mov
            && !(i.op0_kind() == OpKind::Register
                && i.op0_register().full_register32() == Register::EAX)
            && !(i.op_count() > 1
                && i.op1_kind() == OpKind::Register
                && i.op1_register().full_register32() == Register::EAX);
        let fp = x.class.is_some() && !matches!(i.mnemonic(), Mnemonic::Fnstsw | Mnemonic::Fstsw);
        if !(fp || other_mov) {
            return Some(x);
        }
    }
    None
}

/// The first two arguments pushed before the call at `at`, where they are
/// constants: `push imm` or `push reg` with a constant in the register.
fn pushed_args(
    xbe: &Xbe,
    insns: &[Insn],
    at: usize,
    written: &BTreeSet<u32>,
) -> (Option<u32>, Option<u32>) {
    let mut args = Vec::new();
    for j in (at.saturating_sub(12)..at).rev() {
        let p = &insns[j].ins;
        if stops_scan(p) || args.len() == 2 {
            break;
        }
        if p.mnemonic() != Mnemonic::Push {
            continue;
        }
        let v = match p.op0_kind() {
            k if is_imm(k) => Some(p.immediate(0) as u32),
            OpKind::Register => {
                match resolve_reg(xbe, insns, j, p.op0_register(), u32::MAX, written, 0) {
                    Value::Const(c) => Some(c),
                    _ => None,
                }
            }
            // A global nothing stores to holds its image value.
            OpKind::Memory
                if p.memory_base() == Register::None && p.memory_index() == Register::None =>
            {
                let va = p.memory_displacement32();
                (!written.contains(&va))
                    .then(|| xbe.read_u32(va).ok())
                    .flatten()
            }
            _ => None,
        };
        args.push(v);
    }
    (
        args.first().copied().flatten(),
        args.get(1).copied().flatten(),
    )
}

fn classify(i: &Instruction) -> Option<Class> {
    use CpuidFeature as F;
    let f = i.cpuid_features();
    let has = |c: F| f.contains(&c);
    const BEYOND: &[F] = &[
        F::SSE2,
        F::SSE3,
        F::SSSE3,
        F::SSE4_1,
        F::SSE4_2,
        F::SSE4A,
        F::AVX,
        F::AVX2,
        F::FMA,
        F::D3NOW,
        F::D3NOWEXT,
        F::F16C,
    ];
    if f.iter().any(|c| BEYOND.contains(c)) {
        return Some(Class::Beyond);
    }
    if has(F::FPU) || has(F::FPU287) || has(F::FPU387) || i.mnemonic() == Mnemonic::Wait {
        return Some(Class::X87);
    }
    if has(F::FXSR) {
        return Some(Class::SseState);
    }
    if has(F::SSE) {
        let m = mnemonic(i);
        let mm =
            (0..i.op_count()).any(|k| i.op_kind(k) == OpKind::Register && i.op_register(k).is_mm());
        return Some(if m.ends_with("ss") || m.ends_with("ss2si") {
            Class::SseScalar
        } else if m.ends_with("ps") || (m.starts_with("cvt") && m.contains("ps")) {
            Class::SsePacked
        } else if mm || matches!(i.mnemonic(), Mnemonic::Movntq | Mnemonic::Maskmovq) {
            Class::SseMmxInt
        } else {
            Class::SseState
        });
    }
    if has(F::MMX) {
        return Some(Class::Mmx);
    }
    None
}

fn mnemonic(i: &Instruction) -> String {
    format!("{:?}", i.mnemonic()).to_ascii_lowercase()
}

fn reg_name(r: Register) -> String {
    format!("{r:?}").to_ascii_lowercase()
}

fn is_imm(k: OpKind) -> bool {
    matches!(
        k,
        OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
    )
}

/// A memory operand compared syntactically: base, index, scale, displacement.
type MemKey = (Register, Register, u32, u32);

fn mem_key(i: &Instruction) -> Option<MemKey> {
    (0..i.op_count())
        .any(|k| i.op_kind(k) == OpKind::Memory)
        .then(|| {
            (
                i.memory_base(),
                i.memory_index(),
                i.memory_index_scale(),
                i.memory_displacement32(),
            )
        })
}

/// Does `i` store to memory operand 0 (the only form the patterns use)?
fn stores_to(i: &Instruction, key: MemKey) -> bool {
    i.op_count() > 0
        && i.op0_kind() == OpKind::Memory
        && mem_key(i) == Some(key)
        && !matches!(
            i.mnemonic(),
            Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push | Mnemonic::Fldcw | Mnemonic::Ldmxcsr
        )
}

/// How an instruction moves ESP, if that is a known constant: pushes, pops,
/// `add`/`sub esp, imm`. `None` for anything else that writes ESP.
fn esp_change(i: &Instruction) -> Option<i32> {
    let writes_esp = i.op_count() > 0
        && i.op0_kind() == OpKind::Register
        && i.op0_register() == Register::ESP;
    if writes_esp {
        if i.op_count() > 1 && is_imm(i.op1_kind()) {
            let k = i.immediate(1) as u32 as i32;
            return match i.mnemonic() {
                Mnemonic::Add => Some(k),
                Mnemonic::Sub => Some(k.wrapping_neg()),
                _ => None,
            };
        }
        return None;
    }
    if matches!(i.mnemonic(), Mnemonic::Leave | Mnemonic::Enter) {
        return None;
    }
    Some(i.stack_pointer_increment())
}

fn stops_scan(i: &Instruction) -> bool {
    matches!(
        i.flow_control(),
        FlowControl::Call
            | FlowControl::IndirectCall
            | FlowControl::Return
            | FlowControl::UnconditionalBranch
            | FlowControl::IndirectBranch
    )
}

/// Absolute addresses any instruction stores to.
fn written_absolute(insns: &[Insn]) -> BTreeSet<u32> {
    insns
        .iter()
        .filter(|x| {
            let i = &x.ins;
            i.op_count() > 0
                && i.op0_kind() == OpKind::Memory
                && i.memory_base() == Register::None
                && i.memory_index() == Register::None
                && !matches!(
                    i.mnemonic(),
                    Mnemonic::Cmp
                        | Mnemonic::Test
                        | Mnemonic::Push
                        | Mnemonic::Fldcw
                        | Mnemonic::Ldmxcsr
                )
        })
        .map(|x| x.ins.memory_displacement32())
        .collect()
}

enum Op {
    Or(u32),
    And(u32),
}

fn apply(base: Value, ops: &[Op], width: u32) -> Value {
    let (mut and, mut or) = (width, 0u32);
    // `ops` were collected walking backwards; apply them forwards.
    for op in ops.iter().rev() {
        match *op {
            Op::Or(k) => or |= k & width,
            Op::And(k) => {
                and &= k;
                or &= k;
            }
        }
    }
    match base {
        Value::Const(c) => Value::Const(((c & and) | or) & width),
        Value::Saved { and: a0, or: o0 } => Value::Saved {
            and: a0 & and,
            or: ((o0 & and) | or) & width,
        },
        Value::Global { .. } if ops.is_empty() => base,
        _ => Value::Unknown,
    }
}

/// The value memory operand `key` holds just before instruction `at`.
fn resolve_mem(
    xbe: &Xbe,
    insns: &[Insn],
    at: usize,
    key: Option<MemKey>,
    width: u32,
    written: &BTreeSet<u32>,
    depth: u32,
) -> Value {
    let Some(mut key) = key else {
        return Value::Unknown;
    };
    let mut ops = Vec::new();
    for j in (at.saturating_sub(WINDOW)..at).rev() {
        let p = &insns[j].ins;
        if stops_scan(p) {
            break;
        }
        let esp_based = key.0 == Register::ESP && key.1 == Register::None;
        // `push` stores to the new [esp]: in the coordinates after it, that
        // is displacement 0.
        if esp_based && key.3 == 0 && p.mnemonic() == Mnemonic::Push {
            let base = match p.op0_kind() {
                k if is_imm(k) => Value::Const(p.immediate(0) as u32),
                OpKind::Register => {
                    resolve_reg(xbe, insns, j, p.op0_register(), width, written, depth + 1)
                }
                OpKind::Memory if depth < 3 => {
                    resolve_mem(xbe, insns, j, mem_key(p), width, written, depth + 1)
                }
                _ => Value::Unknown,
            };
            return apply(base, &ops, width);
        }
        if !stores_to(p, key) {
            if esp_based {
                // Carry an [esp + d] operand back across ESP changes.
                match esp_change(p) {
                    Some(delta) => key.3 = key.3.wrapping_add(delta as u32),
                    None => break,
                }
            }
            continue;
        }
        let base = match p.mnemonic() {
            Mnemonic::Fnstcw | Mnemonic::Fstcw | Mnemonic::Stmxcsr => {
                Value::Saved { and: width, or: 0 }
            }
            Mnemonic::Mov if p.op_count() > 1 && is_imm(p.op1_kind()) => {
                Value::Const(p.immediate(1) as u32)
            }
            Mnemonic::Mov if p.op1_kind() == OpKind::Register => {
                resolve_reg(xbe, insns, j, p.op1_register(), width, written, depth)
            }
            Mnemonic::Or if is_imm(p.op1_kind()) => {
                ops.push(Op::Or(p.immediate(1) as u32));
                continue;
            }
            Mnemonic::And if is_imm(p.op1_kind()) => {
                ops.push(Op::And(p.immediate(1) as u32));
                continue;
            }
            _ => Value::Unknown,
        };
        return apply(base, &ops, width);
    }
    let (b, ix, _, disp) = key;
    if b == Register::None && ix == Register::None {
        let initial = xbe
            .bytes_at(disp, 4)
            .ok()
            .map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]) & width);
        return apply(
            Value::Global {
                va: disp,
                initial,
                written: written.contains(&disp),
            },
            &ops,
            width,
        );
    }
    Value::Unknown
}

/// The value register `r` holds just before instruction `at`.
fn resolve_reg(
    xbe: &Xbe,
    insns: &[Insn],
    at: usize,
    r: Register,
    width: u32,
    written: &BTreeSet<u32>,
    depth: u32,
) -> Value {
    if depth > 2 {
        return Value::Unknown;
    }
    let full = r.full_register32();
    let mut ops = Vec::new();
    for j in (at.saturating_sub(WINDOW)..at).rev() {
        let p = &insns[j].ins;
        if stops_scan(p) {
            break;
        }
        if p.op_count() == 0
            || p.op0_kind() != OpKind::Register
            || p.op0_register().full_register32() != full
        {
            continue;
        }
        let d = p.op0_register();
        // AH, CH, DH, BH sit 8 bits up.
        let shift = if d.is_gpr8()
            && matches!(d, Register::AH | Register::CH | Register::DH | Register::BH)
        {
            8
        } else {
            0
        };
        let field = match (shift, d.size()) {
            (8, _) => 0xff00,
            (_, 1) => 0xff,
            (_, 2) => 0xffff,
            _ => u32::MAX,
        };
        let imm = (p.op_count() > 1 && is_imm(p.op1_kind()))
            .then(|| ((p.immediate(1) as u32) << shift) & field);
        let base = match (p.mnemonic(), imm) {
            (Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push, _) => continue,
            (Mnemonic::Or, Some(k)) => {
                ops.push(Op::Or(k));
                continue;
            }
            (Mnemonic::And, Some(k)) => {
                ops.push(Op::And(k | !field));
                continue;
            }
            (Mnemonic::Mov, Some(k)) if d.size() >= 2 => Value::Const(k),
            (Mnemonic::Xor, None) if p.op1_kind() == OpKind::Register && p.op1_register() == d => {
                Value::Const(0)
            }
            (Mnemonic::Mov | Mnemonic::Movzx, None) if p.op1_kind() == OpKind::Memory => {
                resolve_mem(xbe, insns, j, mem_key(p), width, written, depth + 1)
            }
            // `pop r` loads [esp] as it was before the pop.
            (Mnemonic::Pop, None) => resolve_mem(
                xbe,
                insns,
                j,
                Some((Register::ESP, Register::None, 1, 0)),
                width,
                written,
                depth + 1,
            ),
            (Mnemonic::Mov, None) if p.op1_kind() == OpKind::Register => {
                resolve_reg(xbe, insns, j, p.op1_register(), width, written, depth + 1)
            }
            _ => Value::Unknown,
        };
        return apply(base, &ops, width);
    }
    Value::Unknown
}

/// x87 instructions that do not read ST(0) unless they name it.
fn no_st0(m: Mnemonic) -> bool {
    use Mnemonic as M;
    matches!(
        m,
        M::Fld
            | M::Fild
            | M::Fbld
            | M::Fld1
            | M::Fldz
            | M::Fldpi
            | M::Fldl2e
            | M::Fldl2t
            | M::Fldlg2
            | M::Fldln2
            | M::Fldcw
            | M::Fnstcw
            | M::Fstcw
            | M::Fnstsw
            | M::Fstsw
            | M::Fnstenv
            | M::Fstenv
            | M::Fldenv
            | M::Fnsave
            | M::Fsave
            | M::Frstor
            | M::Fninit
            | M::Finit
            | M::Fnclex
            | M::Fclex
            | M::Wait
            | M::Ffree
            | M::Ffreep
            | M::Fdecstp
            | M::Fincstp
            | M::Fnop
            | M::Fxsave
            | M::Fxrstor
    )
}

/// Stack slots an x87 instruction reads (before its own push or pop).
fn needs(i: &Instruction) -> i32 {
    let mut n = 0;
    for k in 0..i.op_count() {
        if i.op_kind(k) == OpKind::Register && i.op_register(k).is_st() {
            n = n.max(i.op_register(k) as i32 - Register::ST0 as i32 + 1);
        }
    }
    let inc = i.fpu_stack_increment_info().increment();
    if inc > 0 {
        n = n.max(inc);
    }
    if !no_st0(i.mnemonic()) {
        n = n.max(1);
    }
    // Implicit ST(0) and ST(1): `fpatan`/`fyl2x`/`fyl2xp1` pop two and push
    // one; `fprem`/`fprem1`/`fscale` read both and pop nothing.
    if matches!(
        i.mnemonic(),
        Mnemonic::Fpatan
            | Mnemonic::Fyl2x
            | Mnemonic::Fyl2xp1
            | Mnemonic::Fprem
            | Mnemonic::Fprem1
            | Mnemonic::Fscale
    ) {
        n = n.max(2);
    }
    n
}

/// Values a call seems to leave on the x87 stack: how far the straight-line
/// x87 code after it reads below what it pushed itself, starting `depth`
/// deep.
fn arity_from_use(insns: &[Insn], index: &HashMap<u32, usize>, next: u32, depth: i32) -> i32 {
    let Some(&start) = index.get(&next) else {
        return 0;
    };
    let (mut d, mut r) = (depth, 0);
    for x in insns.iter().skip(start).take(LOOKAHEAD) {
        if x.class == Some(Class::X87) {
            r = r.max(needs(&x.ins) - d);
            d -= x.ins.fpu_stack_increment_info().increment();
        }
        if x.ins.flow_control() != FlowControl::Next {
            break;
        }
    }
    r
}

fn walk(
    f: u32,
    a: &Analysis,
    xbe: &Xbe,
    insns: &[Insn],
    index: &HashMap<u32, usize>,
    effects: &HashMap<u32, Effect>,
) -> FnStack {
    let mut s = FnStack::default();
    if let Some(&start) = index.get(&f) {
        let body = insns[start..].iter().take_while(|x| x.owner == f);
        let (mut x87, mut mmx) = (false, false);
        for x in body {
            x87 |= x.class == Some(Class::X87);
            mmx |= uses_mmx(x);
        }
        s.mmx_mixed = x87 && mmx;
    }
    let mut seen: HashMap<u32, i32> = HashMap::new();
    let mut todo = vec![(f, 0i32)];
    let mut steps = 0;
    while let Some((va, depth)) = todo.pop() {
        if let Some(&d) = seen.get(&va) {
            if d != depth && s.inconsistent.is_none() {
                s.inconsistent = Some(va);
            }
            continue;
        }
        steps += 1;
        if steps > WALK_LIMIT {
            s.truncated = true;
            break;
        }
        let Some(&ix) = index.get(&va) else { continue };
        seen.insert(va, depth);
        let x = &insns[ix];
        let ins = &x.ins;
        let mut d = depth;
        if x.class == Some(Class::X87) {
            if x.owner == f {
                s.x87_reached += 1;
            }
            s.entry_need = s.entry_need.max(needs(ins) - d);
            let info = ins.fpu_stack_increment_info();
            if info.conditional() {
                s.conditional_pushes += 1;
            }
            match ins.mnemonic() {
                Mnemonic::Fninit | Mnemonic::Finit | Mnemonic::Fnsave | Mnemonic::Fsave => {
                    d = -s.entry_need
                }
                Mnemonic::Frstor | Mnemonic::Fldenv => {
                    s.dynamic_top += 1;
                    continue;
                }
                _ => d -= info.increment(),
            }
            s.max_depth = s.max_depth.max(d);
        } else if ins.mnemonic() == Mnemonic::Fxrstor {
            s.dynamic_top += 1;
            continue;
        }
        let next = ins.next_ip32();
        match ins.flow_control() {
            FlowControl::Next => todo.push((next, d)),
            FlowControl::ConditionalBranch => {
                todo.push((next, d));
                if is_near(ins) {
                    todo.push((ins.near_branch32(), d));
                }
            }
            FlowControl::UnconditionalBranch if is_near(ins) => {
                let t = ins.near_branch32();
                if t != f && a.functions.contains_key(&t) {
                    // A tail call: returns with whatever the target leaves.
                    match effects.get(&t) {
                        Some(e) => {
                            s.entry_need = s.entry_need.max(e.need - d);
                            s.ret_depths.insert(d + e.delta);
                            s.callee_grade = s.callee_grade.max(e.grade);
                        }
                        None if x87_free(t, insns, index) => {
                            s.ret_depths.insert(d);
                        }
                        None => s.unresolved_tails += 1,
                    }
                } else {
                    todo.push((t, d));
                }
            }
            FlowControl::IndirectBranch => {
                let targets = table_targets(ins, a, xbe, x.owner);
                if targets.is_empty() {
                    s.indirect_jumps += 1;
                }
                todo.extend(targets.into_iter().map(|t| (t, d)));
            }
            FlowControl::Call | FlowControl::IndirectCall => {
                let callee = (ins.flow_control() == FlowControl::Call && is_near(ins))
                    .then(|| ins.near_branch32());
                // Unknown callees follow the MSVC convention: whatever is
                // on the stack at the call is its arguments, and it leaves
                // a result only if the code after it reads one.
                let result = arity_from_use(insns, index, next, 0);
                let (need, delta) = match callee.and_then(|t| effects.get(&t)) {
                    Some(e) => {
                        if e.need + e.delta != result {
                            s.arity_disagreements += 1;
                        }
                        s.callee_grade = s.callee_grade.max(e.grade);
                        (e.need, e.delta)
                    }
                    None => {
                        s.guessed_calls.push((d, result));
                        // Relative depth: values the body pushed itself. Any
                        // passed in are its caller's business.
                        let live = d.max(0);
                        (live, result - live)
                    }
                };
                s.entry_need = s.entry_need.max(need - d);
                // Values below the ones the callee consumes stay live across it.
                s.call_depths.insert(d - need);
                if callee
                    .and_then(|t| a.functions.get(&t))
                    .is_some_and(|c| !c.returns)
                {
                    continue;
                }
                todo.push((next, d + delta));
            }
            FlowControl::Return => {
                s.ret_depths.insert(d);
            }
            _ => {}
        }
    }
    s
}

/// Does the function at `f` own no x87 instruction? Then a tail jump into
/// it leaves the stack alone (bar calls it makes; those follow the convention).
fn x87_free(f: u32, insns: &[Insn], index: &HashMap<u32, usize>) -> bool {
    let Some(&start) = index.get(&f) else {
        return false;
    };
    !insns[start..]
        .iter()
        .take_while(|x| x.owner == f)
        .any(|x| x.class == Some(Class::X87))
}

/// Does the instruction touch an MMX register or MMX state (`emms`)?
fn uses_mmx(x: &Insn) -> bool {
    matches!(x.class, Some(Class::Mmx | Class::SseMmxInt))
        || (0..x.ins.op_count())
            .any(|k| x.ins.op_kind(k) == OpKind::Register && x.ins.op_register(k).is_mm())
}

fn is_near(i: &Instruction) -> bool {
    matches!(i.op0_kind(), OpKind::NearBranch32 | OpKind::NearBranch16)
}

/// Targets of `jmp [r*4 + table]`, read while they land on instructions
/// the same function owns.
fn table_targets(i: &Instruction, a: &Analysis, xbe: &Xbe, owner: u32) -> Vec<u32> {
    if !a.switches.contains_key(&i.ip32())
        || i.op0_kind() != OpKind::Memory
        || i.memory_index_scale() != 4
    {
        return Vec::new();
    }
    let table = i.memory_displacement32();
    let mut out = Vec::new();
    for k in 0..crate::MAX_SWITCH {
        let Ok(t) = xbe.read_u32(table.wrapping_add(k * 4)) else {
            break;
        };
        match a.claim(t) {
            Some(('I', o)) if o.unwrap_or(0) == owner => out.push(t),
            _ => break,
        }
    }
    out
}

fn describe_x87(v: u32) -> String {
    let pc = match (v >> 8) & 3 {
        0 => "24",
        2 => "53",
        3 => "64",
        _ => "res",
    };
    let rc = ["near", "down", "up", "chop"][((v >> 10) & 3) as usize];
    format!("PC={pc} RC={rc} masks={:#04x}", v & 0x3f)
}

/// Effect of `(saved & and) | or` on one field: forced, kept or mixed.
fn field(and: u32, or: u32, mask: u32, shift: u32, names: &[&str]) -> String {
    if and & mask == 0 || or & mask == mask {
        names[((or & mask) >> shift) as usize].to_owned()
    } else if and & mask == mask && or & mask == 0 {
        "kept".to_owned()
    } else {
        "mixed".to_owned()
    }
}

fn describe_cw(v: Value) -> String {
    match v {
        Value::Const(c) => format!("const {c:#06x} ({})", describe_x87(c)),
        Value::Saved { and, or } => {
            if and == 0xffff && or == 0 {
                return "restore saved".to_owned();
            }
            let pc = field(and, or, 0x300, 8, &["PC=24", "PC=res", "PC=53", "PC=64"]);
            let rc = field(
                and,
                or,
                0xc00,
                10,
                &["RC=near", "RC=down", "RC=up", "RC=chop"],
            );
            format!(
                "(saved & {and:#06x}) | {or:#06x} ({}, {})",
                label(&pc, "PC"),
                label(&rc, "RC")
            )
        }
        Value::Global {
            va,
            initial,
            written,
        } => format!(
            "global {va:#010x} initial {} {}",
            initial.map_or("none (bss)".to_owned(), |c| format!(
                "{c:#06x} ({})",
                describe_x87(c)
            )),
            if written {
                "written by code"
            } else {
                "never written"
            }
        ),
        Value::Unknown => "unresolved".to_owned(),
    }
}

fn label(s: &str, what: &str) -> String {
    if s.contains('=') {
        s.to_owned()
    } else {
        format!("{what} {s}")
    }
}

fn describe_mxcsr(v: Value) -> String {
    let fields = |c: u32| {
        format!(
            "RC={} FTZ={} DAZ={} masks={:#04x}",
            ["near", "down", "up", "chop"][((c >> 13) & 3) as usize],
            (c >> 15) & 1,
            (c >> 6) & 1,
            (c >> 7) & 0x3f
        )
    };
    match v {
        Value::Const(c) => format!("const {c:#06x} ({})", fields(c)),
        Value::Saved { and, or } if and == u32::MAX && or == 0 => "restore saved".to_owned(),
        Value::Saved { and, or } => format!(
            "(saved & {and:#010x}) | {or:#06x} (RC {}, FTZ {}, DAZ {})",
            field(and, or, 0x6000, 13, &["near", "down", "up", "chop"]),
            field(and, or, 0x8000, 15, &["0", "1"]),
            field(and, or, 0x40, 6, &["0", "1"])
        ),
        Value::Global {
            va,
            initial,
            written,
        } => format!(
            "global {va:#010x} initial {} {}",
            initial.map_or("none (bss)".to_owned(), |c| format!(
                "{c:#06x} ({})",
                fields(c)
            )),
            if written {
                "written by code"
            } else {
                "never written"
            }
        ),
        Value::Unknown => "unresolved".to_owned(),
    }
}

fn pct(n: usize, of: usize) -> f64 {
    if of == 0 {
        0.0
    } else {
        100.0 * n as f64 / of as f64
    }
}

impl FpStats {
    fn owner_of(&self, va: u32) -> Option<u32> {
        self.owners.get(&va).copied()
    }
}

impl fmt::Display for FpStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fp: usize = self.by_class.values().sum();
        writeln!(
            f,
            "instructions decoded: {}, floating point or SIMD: {fp} ({:.2}%)",
            self.insns,
            pct(fp, self.insns)
        )?;
        writeln!(f, "\nby class:")?;
        for (c, n) in &self.by_class {
            writeln!(f, "  {:<12} {n:>8} {:>6.2}%", c.label(), pct(*n, fp))?;
        }

        writeln!(f, "\nby section:")?;
        let classes: Vec<Class> = self.by_class.keys().copied().collect();
        let head: Vec<String> = classes
            .iter()
            .map(|c| format!("{:>12}", c.label()))
            .collect();
        writeln!(f, "  {:<10}{}", "", head.join(""))?;
        for (sec, by) in &self.by_section {
            let row: Vec<String> = classes
                .iter()
                .map(|c| format!("{:>12}", by.get(c).copied().unwrap_or(0)))
                .collect();
            writeln!(f, "  {sec:<10}{}", row.join(""))?;
        }

        let owned: Vec<(&u32, &FnFp)> = self.functions.iter().filter(|(a, _)| **a != 0).collect();
        let users = |p: &dyn Fn(&FnFp) -> bool| owned.iter().filter(|(_, x)| p(x)).count();
        writeln!(f, "\nfunctions using:")?;
        writeln!(f, "  any fp/simd  {:>6}", owned.len())?;
        writeln!(f, "  x87          {:>6}", users(&|x| x.get(Class::X87) > 0))?;
        writeln!(f, "  sse          {:>6}", users(&|x| x.sse() > 0))?;
        writeln!(
            f,
            "  sse scalar   {:>6}",
            users(&|x| x.get(Class::SseScalar) > 0)
        )?;
        writeln!(
            f,
            "  sse packed   {:>6}",
            users(&|x| x.get(Class::SsePacked) > 0)
        )?;
        writeln!(
            f,
            "  x87 and sse  {:>6}",
            users(&|x| x.get(Class::X87) > 0 && x.sse() > 0)
        )?;
        writeln!(
            f,
            "  mmx          {:>6}",
            users(&|x| x.get(Class::Mmx) + x.get(Class::SseMmxInt) > 0)
        )?;
        writeln!(
            f,
            "  mmx and x87  {:>6}",
            users(&|x| x.get(Class::Mmx) > 0 && x.get(Class::X87) > 0)
        )?;
        writeln!(
            f,
            "  beyond piii  {:>6}",
            users(&|x| x.get(Class::Beyond) > 0)
        )?;
        if let Some(u) = self.functions.get(&0) {
            writeln!(f, "  (instructions with no owning function: {})", u.total())?;
        }

        writeln!(f, "\nfp instructions per function (functions with any):")?;
        let buckets = [
            (1, 1),
            (2, 5),
            (6, 20),
            (21, 100),
            (101, 500),
            (501, usize::MAX),
        ];
        for (lo, hi) in buckets {
            let fns: Vec<usize> = owned
                .iter()
                .map(|(_, x)| x.total())
                .filter(|n| *n >= lo && *n <= hi)
                .collect();
            let label = if hi == usize::MAX {
                format!("{lo}+")
            } else {
                format!("{lo}-{hi}")
            };
            writeln!(
                f,
                "  {label:<8} {:>6} functions {:>8} insns",
                fns.len(),
                fns.iter().sum::<usize>()
            )?;
        }
        let mut sizes: Vec<usize> = owned.iter().map(|(_, x)| x.total()).collect();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        let top = |k: usize| sizes.iter().take(k).sum::<usize>();
        let k10 = sizes.len().div_ceil(10);
        writeln!(
            f,
            "  top 100 functions hold {:.1}%, top 10% ({k10}) hold {:.1}%",
            pct(top(100), fp),
            pct(top(k10), fp)
        )?;

        for (title, pick) in [
            (
                "top x87 functions",
                &(|x: &FnFp| x.get(Class::X87)) as &dyn Fn(&FnFp) -> usize,
            ),
            ("top sse functions", &|x: &FnFp| x.sse()),
            ("top mmx functions", &|x: &FnFp| {
                x.get(Class::Mmx) + x.get(Class::SseMmxInt)
            }),
        ] {
            let mut v: Vec<(u32, usize, usize)> = owned
                .iter()
                .map(|(a, x)| (**a, pick(x), x.total()))
                .filter(|(_, n, _)| *n > 0)
                .collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            writeln!(f, "\n{title}:")?;
            for (a, n, t) in v.iter().take(12) {
                writeln!(f, "  {a:#010x} {n:>6} (of {t} fp)")?;
            }
        }

        for (title, set) in [
            (
                "direct calls/jumps from the entry and --root addresses",
                &self.reach_calls,
            ),
            (
                "calls/jumps and code-address immediates from the entry",
                &self.reach_refs,
            ),
        ] {
            let mut by: BTreeMap<Class, usize> = BTreeMap::new();
            let mut x87_fns = 0;
            for a in set {
                if let Some(x) = self.functions.get(a) {
                    for (c, n) in &x.counts {
                        *by.entry(*c).or_default() += n;
                    }
                    x87_fns += usize::from(x.get(Class::X87) > 0);
                }
            }
            writeln!(
                f,
                "\nreachable by {title}: {} functions ({x87_fns} with x87)",
                set.len()
            )?;
            for (c, n) in &by {
                writeln!(
                    f,
                    "  {:<12} {n:>8} of {:>8} ({:.1}%)",
                    c.label(),
                    self.by_class[c],
                    pct(*n, self.by_class[c])
                )?;
            }
            let fldcw = self.fldcw.iter().filter(|s| set.contains(&s.owner)).count();
            let ldmxcsr = self
                .ldmxcsr
                .iter()
                .filter(|s| set.contains(&s.owner))
                .count();
            writeln!(f, "  fldcw sites {fldcw}, ldmxcsr sites {ldmxcsr}")?;
        }

        writeln!(
            f,
            "\nby mnemonic (all, reachable by calls, reachable by calls+refs):"
        )?;
        let mut cur = None;
        for ((c, m), n) in &self.by_mnemonic {
            if cur != Some(*c) {
                writeln!(f, "  [{}]", c.label())?;
                cur = Some(*c);
            }
            let (rc, rr) = self
                .by_mnemonic_reach
                .get(&(*c, m.clone()))
                .copied()
                .unwrap_or_default();
            writeln!(f, "    {m:<12} {n:>8} {rc:>8} {rr:>8}")?;
        }

        writeln!(f, "\ntranscendental and friends:")?;
        for (m, n) in &self.transcendental {
            writeln!(f, "  {m:<10} {n:>6}")?;
        }
        writeln!(f, "\nx87 memory operands by size:")?;
        for (m, n) in &self.x87_mem {
            writeln!(f, "  {m:<14} {n:>6}")?;
        }
        writeln!(f, "\n80-bit / BCD memory operands:")?;
        if self.mem80.is_empty() {
            writeln!(f, "  none")?;
        }
        for (m, vas) in &self.mem80 {
            let owners: BTreeSet<u32> = vas.iter().filter_map(|v| self.owner_of(*v)).collect();
            let list: Vec<String> = owners.iter().take(20).map(|v| format!("{v:#x}")).collect();
            writeln!(
                f,
                "  {m:<14} {:>6} in {} functions: {}",
                vas.len(),
                owners.len(),
                list.join(" ")
            )?;
        }
        writeln!(f, "\nx87/SIMD state and environment:")?;
        for (m, n) in &self.env {
            writeln!(f, "  {m:<10} {n:>6}")?;
        }
        writeln!(f, "\nstatus word reads and their consumer:")?;
        for (k, vas) in &self.status_reads {
            let list: Vec<String> = vas.iter().take(3).map(|v| format!("{v:#x}")).collect();
            writeln!(f, "  {k:<34} {:>6}  e.g. {}", vas.len(), list.join(" "))?;
        }

        writeln!(f, "\nfldcw sites: {}", self.fldcw.len())?;
        let mut kinds: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for s in &self.fldcw {
            kinds.entry(describe_cw(s.value)).or_default().push(s.va);
        }
        for (k, vas) in &kinds {
            let list: Vec<String> = vas.iter().take(6).map(|v| format!("{v:#x}")).collect();
            writeln!(
                f,
                "  {:>4} x {k}\n         at {}{}",
                vas.len(),
                list.join(" "),
                if vas.len() > 6 { " ..." } else { "" }
            )?;
        }
        let fns: BTreeSet<u32> = self.fldcw.iter().map(|s| s.owner).collect();
        writeln!(
            f,
            "  in {} functions; direct call sites into them:",
            fns.len()
        )?;
        let mut callers: Vec<(&u32, &usize)> = self.fldcw_fn_callers.iter().collect();
        callers.sort_by(|a, b| b.1.cmp(a.1));
        for (fun, n) in callers.iter().take(12) {
            let here = self.fldcw.iter().filter(|s| s.owner == **fun).count();
            writeln!(f, "    {fun:#010x} {n:>6} callers ({here} fldcw)")?;
        }
        writeln!(
            f,
            "  calls into them and their short wrappers, by first two pushed arguments:"
        )?;
        let show = |v: &Option<u32>| v.map_or("?".to_owned(), |c| format!("{c:#x}"));
        for ((fun, a1, a2), sites) in &self.cw_calls {
            let list: Vec<String> = sites.iter().take(4).map(|v| format!("{v:#x}")).collect();
            writeln!(
                f,
                "    {fun:#010x}({}, {}) x {:<3} from {}",
                show(a1),
                show(a2),
                sites.len(),
                list.join(" ")
            )?;
        }

        writeln!(
            f,
            "\nMXCSR writes: {} ldmxcsr, {} fxrstor",
            self.ldmxcsr.len(),
            self.env.get("fxrstor").unwrap_or(&0)
        )?;
        for s in &self.ldmxcsr {
            writeln!(
                f,
                "  {:#010x} in {:#010x}: {}",
                s.va,
                s.owner,
                describe_mxcsr(s.value)
            )?;
        }

        // Stack-depth walk, over functions with x87 code.
        let x87fns: Vec<(&u32, &FnStack)> = self
            .stacks
            .iter()
            .filter(|(a, _)| self.functions.get(a).is_some_and(|x| x.get(Class::X87) > 0))
            .collect();
        let owned_x87: usize = x87fns
            .iter()
            .map(|(a, _)| self.functions[a].get(Class::X87))
            .sum();
        let reached: usize = x87fns.iter().map(|(_, s)| s.x87_reached).sum();
        writeln!(
            f,
            "\nx87 stack-depth walk ({} functions with x87; fixpoint {} after {} rounds):",
            x87fns.len(),
            if self.converged { "settled" } else { "DID NOT settle" },
            self.rounds
        )?;
        writeln!(
            f,
            "  x87 instructions reached: {reached} of {owned_x87} owned ({:.1}%)",
            pct(reached, owned_x87)
        )?;
        let x87_of = |a: &u32| self.functions[a].get(Class::X87);
        writeln!(
            f,
            "  resolved bodies (one depth per insn, 0..=8 slots, no dynamic TOP/MMX/unresolved jump or tail, <= 1 return depth), by grade:"
        )?;
        let mut cum = 0;
        for g in [Grade::Proven, Grade::Convention, Grade::Heuristic] {
            let fns: Vec<&u32> = x87fns
                .iter()
                .filter(|(_, s)| s.resolved() && s.grade() == g)
                .map(|(a, _)| *a)
                .collect();
            let n: usize = fns.iter().map(|a| x87_of(a)).sum();
            cum += n;
            writeln!(
                f,
                "    {:<11} {:>5} functions {:>6} x87 insns ({:>5.1}%, cumulative {:.1}%)",
                format!("{g:?}"),
                fns.len(),
                n,
                pct(n, owned_x87),
                pct(cum, owned_x87)
            )?;
        }
        let unres: usize = x87fns
            .iter()
            .filter(|(_, s)| !s.resolved())
            .map(|(a, _)| x87_of(a))
            .sum();
        writeln!(
            f,
            "    unresolved  {:>5} functions {:>6} x87 insns ({:.1}%)",
            x87fns.iter().filter(|(_, s)| !s.resolved()).count(),
            unres,
            pct(unres, owned_x87)
        )?;
        let with_effect = x87fns.iter().filter(|(_, s)| s.effect().is_some()).count();
        writeln!(f, "  with a callable effect (resolved, exactly one return depth): {with_effect}")?;
        writeln!(
            f,
            "  MMX mixed with x87 (unresolved): {}, over 8 slots (unresolved): {}, conditional fptan/fsincos push (graded Convention): {}",
            x87fns.iter().filter(|(_, s)| s.mmx_mixed).count(),
            x87fns
                .iter()
                .filter(|(_, s)| s.entry_need > 8 || s.max_depth + s.entry_need > 8)
                .count(),
            x87fns.iter().filter(|(_, s)| s.conditional_pushes > 0).count()
        )?;
        let count = |p: &dyn Fn(&FnStack) -> bool| x87fns.iter().filter(|(_, s)| p(s)).count();
        writeln!(
            f,
            "  inconsistent depth at a join:   {}",
            count(&|s| s.inconsistent.is_some())
        )?;
        for (a, s) in x87fns
            .iter()
            .filter(|(_, s)| s.inconsistent.is_some())
            .take(10)
        {
            writeln!(f, "    {a:#010x} at {:#010x}", s.inconsistent.unwrap())?;
        }
        writeln!(
            f,
            "  unresolved indirect jumps:      {}",
            count(&|s| s.indirect_jumps > 0)
        )?;
        writeln!(
            f,
            "  TOP loaded from memory:         {}",
            count(&|s| s.dynamic_top > 0)
        )?;
        writeln!(
            f,
            "  several return depths:          {}",
            count(&|s| s.ret_depths.len() > 1)
        )?;
        writeln!(
            f,
            "  walk truncated:                 {}",
            count(&|s| s.truncated)
        )?;
        writeln!(
            f,
            "  x87 values passed in (entry needs > 0): {}",
            count(&|s| s.entry_need > 0)
        )?;
        for (a, s) in x87fns.iter().filter(|(_, s)| s.entry_need > 0).take(10) {
            writeln!(f, "    {a:#010x} needs {}", s.entry_need)?;
        }
        let mut arity: BTreeMap<String, usize> = BTreeMap::new();
        for (_, s) in &x87fns {
            let k = s
                .ret_arity()
                .map_or("unresolved".to_owned(), |r| r.to_string());
            *arity.entry(k).or_default() += 1;
        }
        writeln!(f, "  x87 values left at return: {arity:?}")?;
        // Every function, not just x87 ones: callers of float-returning
        // non-x87 functions are covered via the x87 functions anyway.
        let mut maxd: BTreeMap<i32, usize> = BTreeMap::new();
        for (_, s) in &x87fns {
            *maxd.entry(s.max_depth + s.entry_need).or_default() += 1;
        }
        writeln!(f, "  max stack depth: {maxd:?}")?;
        let live: Vec<&(&u32, &FnStack)> = x87fns
            .iter()
            .filter(|(_, s)| s.call_depths.iter().any(|d| d + s.entry_need > 0))
            .collect();
        writeln!(
            f,
            "  functions calling with x87 values live on the stack: {}",
            live.len()
        )?;
        for (a, s) in live.iter().take(10) {
            let ds: Vec<i32> = s.call_depths.iter().map(|d| d + s.entry_need).collect();
            writeln!(f, "    {a:#010x} depths at calls {ds:?}")?;
        }
        writeln!(
            f,
            "\nmost-called x87 functions (helpers such as float-to-int and CRT math):"
        )?;
        let mut helpers: Vec<(u32, usize)> = x87fns
            .iter()
            .map(|(a, _)| (**a, self.callers.get(a).copied().unwrap_or(0)))
            .filter(|(_, n)| *n > 0)
            .collect();
        helpers.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (a, n) in helpers.iter().take(20) {
            let s = &self.stacks[a];
            let fx = s
                .effect()
                .map_or("effect unresolved".to_owned(), |e| {
                    format!("takes {}, returns {} ({:?})", e.need, e.need + e.delta, e.grade)
                });
            let m: Vec<String> = self.functions[a]
                .counts
                .iter()
                .map(|(c, n)| format!("{} {n}", c.label()))
                .collect();
            writeln!(
                f,
                "  {a:#010x} {n:>6} callers, {:>5} insns, {fx}; {}",
                self.sizes.get(a).copied().unwrap_or(0),
                m.join(", ")
            )?;
        }
        let dis: usize = self.stacks.values().map(|s| s.arity_disagreements).sum();
        writeln!(
            f,
            "  call result arity: callee walk disagreed with caller's use at {dis} call sites"
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;

    fn stats(code: &[u8]) -> FpStats {
        let mut c = vec![0xCCu8; 0x100];
        c[..code.len()].copy_from_slice(code);
        let data = crate::tests::xbe_with(&c);
        let x = Xbe::parse(&data).unwrap();
        let a = analyze(&x).unwrap();
        fpstats(&x, &a, &[]).unwrap()
    }

    #[test]
    fn ftol_control_word_resolves_to_chop() {
        #[rustfmt::skip]
        let s = stats(&[
            0x83, 0xEC, 0x0C,             // sub esp, 12
            0xD9, 0x7C, 0x24, 0x02,       // fnstcw [esp+2]
            0x66, 0x8B, 0x44, 0x24, 0x02, // mov ax, [esp+2]
            0x80, 0xCC, 0x0C,             // or ah, 0Ch
            0x66, 0x89, 0x04, 0x24,       // mov [esp], ax
            0xD9, 0x2C, 0x24,             // fldcw [esp]
            0xDF, 0x7C, 0x24, 0x04,       // fistp qword [esp+4]
            0xD9, 0x6C, 0x24, 0x02,       // fldcw [esp+2]
            0x8B, 0x44, 0x24, 0x04,       // mov eax, [esp+4]
            0x83, 0xC4, 0x0C,             // add esp, 12
            0xC3,                         // ret
        ]);
        assert_eq!(s.fldcw.len(), 2);
        assert_eq!(
            s.fldcw[0].value,
            Value::Saved {
                and: 0xffff,
                or: 0x0c00
            }
        );
        assert!(describe_cw(s.fldcw[0].value).contains("RC=chop"));
        assert!(describe_cw(s.fldcw[0].value).contains("PC kept"));
        assert_eq!(describe_cw(s.fldcw[1].value), "restore saved");
        // _ftol consumes the value its caller passed in ST(0).
        let w = &s.stacks[&0x11000];
        assert_eq!(w.entry_need, 1);
        assert_eq!(w.ret_arity(), Some(0));
    }

    #[test]
    fn compare_idioms_and_depth() {
        #[rustfmt::skip]
        let s = stats(&[
            0xD9, 0x05, 0x00, 0x20, 0x01, 0x00, // fld dword [0x12000]
            0xD9, 0xE8,                         // fld1
            0xDE, 0xD9,                         // fcompp
            0xDF, 0xE0,                         // fnstsw ax
            0xF6, 0xC4, 0x41,                   // test ah, 41h
            0x75, 0x05,                         // jnz +5
            0xD9, 0xEE,                         // fldz
            0xDD, 0xD8,                         // fstp st(0)
            0x90,                               // nop
            0xD9, 0xE8,                         // fld1
            0xC3,                               // ret (returns a float)
        ]);
        assert_eq!(
            s.status_reads.get("fnstsw ax; test ah, 0x41").map(Vec::len),
            Some(1)
        );
        let w = &s.stacks[&0x11000];
        assert_eq!(w.entry_need, 0);
        assert_eq!(w.max_depth, 2);
        assert!(w.inconsistent.is_none());
        assert_eq!(w.ret_arity(), Some(1));
    }

    #[test]
    fn mxcsr_built_through_push_and_pop_sets_ftz() {
        #[rustfmt::skip]
        let s = stats(&[
            0x51,                         // push ecx (room)
            0x0F, 0xAE, 0x1C, 0x24,       // stmxcsr [esp]
            0x58,                         // pop eax
            0x0D, 0x00, 0x80, 0x00, 0x00, // or eax, 8000h
            0x50,                         // push eax
            0x0F, 0xAE, 0x14, 0x24,       // ldmxcsr [esp]
            0x59,                         // pop ecx
            0xC3,
        ]);
        assert_eq!(s.ldmxcsr.len(), 1);
        assert_eq!(s.ldmxcsr[0].value, Value::Saved { and: u32::MAX, or: 0x8000 });
        assert!(describe_mxcsr(s.ldmxcsr[0].value).contains("FTZ 1"));
    }

    #[test]
    fn stack_bounds_mmx_and_implicit_operands() {
        // Nine pushes overflow the eight-slot stack.
        let mut code = [0xD9, 0xE8].repeat(9); // fld1 x9
        code.extend([0xDD, 0xD8].repeat(9)); // fstp st(0) x9
        code.push(0xC3);
        let s = stats(&code);
        assert!(!s.stacks[&0x11000].resolved(), "nine pushes must not resolve");

        // emms empties the stack under the pushed value.
        #[rustfmt::skip]
        let s = stats(&[0xD9, 0xE8, 0x0F, 0x77, 0xDD, 0xD8, 0xC3]); // fld1; emms; fstp st(0); ret
        assert!(s.stacks[&0x11000].mmx_mixed);
        assert!(!s.stacks[&0x11000].resolved());

        // fpatan reads ST(0) and ST(1), pops two, pushes one.
        #[rustfmt::skip]
        let s = stats(&[0xD9, 0xE8, 0xD9, 0xF3, 0xDD, 0xD8, 0xC3]); // fld1; fpatan; fstp st(0); ret
        let w = &s.stacks[&0x11000];
        assert_eq!(w.entry_need, 1, "fpatan needs a second operand from the caller");
        assert_eq!(w.ret_arity(), Some(0));
    }

    #[test]
    fn sse_classes() {
        #[rustfmt::skip]
        let s = stats(&[
            0xF3, 0x0F, 0x58, 0xC1,       // addss xmm0, xmm1
            0x0F, 0x59, 0xC1,             // mulps xmm0, xmm1
            0xF3, 0x0F, 0x2C, 0xC0,       // cvttss2si eax, xmm0
            0x0F, 0x70, 0xC1, 0x1B,       // pshufw mm0, mm1, 1Bh
            0x0F, 0xAE, 0x14, 0x24,       // ldmxcsr [esp]
            0x0F, 0x77,                   // emms
            0xC3,
        ]);
        assert_eq!(s.by_class.get(&Class::SseScalar), Some(&2));
        assert_eq!(s.by_class.get(&Class::SsePacked), Some(&1));
        assert_eq!(s.by_class.get(&Class::SseMmxInt), Some(&1));
        assert_eq!(s.by_class.get(&Class::SseState), Some(&1));
        assert_eq!(s.by_class.get(&Class::Mmx), Some(&1));
        assert_eq!(s.ldmxcsr.len(), 1);
    }
}
