//! IR to C11, one owner function at a time.
//!
//! Shape of a generated handler (see `c/h2gen.h` for the macros):
//!
//! ```c
//! TransferAbi h2fn_002d21c6(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {
//!     H2_LOCALS_HOMED(0x1f3u);      // written registers and EFLAGS into locals
//!     switch (start) {              // validated entry/resume selector
//!     case 0x002d21c6u: goto L_002d21c6;
//!     case 0x002d2205u: goto L_002d2205;   // call continuation
//!     default: break;
//!     }
//!     ...stop with H2SHIM_BAD_ENTRY...
//! L_002d21c6:
//!     /* 0x002d21c6 */ { uint32_t t0 = ebp; ... }
//! }
//! ```
//!
//! Registers and flags the handler writes live in locals for the whole
//! handler. Every way out (transfer, fault, stop) sets the outgoing
//! transfer and jumps to the one shared epilogue, which spills them to
//! `CpuAbi`, so state is canonical whenever the runtime sees it. A register
//! the handler never writes has no local: it is read from `CpuAbi` where it
//! is used, and never loaded, spilled or reloaded (see [`homes`]). Written
//! XMM registers can use the same homes. Guest memory goes through the ABI's
//! checked helpers or the gated inline RAM path (`h2fastmem.h`). The gate
//! is held across consecutive accesses and dropped (`H2_GATE_DROP`) before
//! anything that takes the context, at every exit, and every few loop trips. All arithmetic is
//! unsigned and fixed-width, through the total helpers in `h2gen.h`, so no
//! IR operation can become C undefined behaviour.
//!
//! Transfers (LIFTER.md §2): a direct jump or branch side marked `tail`,
//! and any computed jump, is `JUMP`/`ENTRY`; a call is `CALL`/`ENTRY` with
//! the continuation as resume PC; a return is `RETURN`/`RETURN_TARGET`. A
//! switch compares the target the guest actually loaded against its
//! validated cases and stops on anything else. An unsupported instruction
//! stops with its mnemonic and reason.
//!
//! Atomic instructions run their pure prefix (address and operand), then
//! one runtime call (`h2abi_v1_atomic_rmw`, `xchg` or `cmpxchg`) that does
//! the whole read-modify-write, flags included, under the runtime's lock.
//! The locals are spilled to `CpuAbi` before it and reloaded after, the old
//! value comes back in a local (never a pointer into `CpuAbi`), and a
//! failing call leaves through the usual fault exit with nothing changed.
//!
//! Inside a REP loop every exit (fault or stop) first restores EFLAGS and
//! the defined mask saved when the instruction began, matching the IR's
//! `Repeat` contract: a faulting `repe cmpsb` reports its starting flags.
//!
//! `LazyFlags` statements capture their formula's inputs and are settled
//! into `fl`/`fd` only where something reads them and in the epilogue; see
//! [`crate::lazy`].

use std::collections::{BTreeMap, BTreeSet};


use ir::{
    Addr, Atomic, AtomicOp, BinOp, CallKind, CastOp, CmpOp, Control, Dest, Expr, Fault, Flag, FlagValue, Gpr, Intrinsic,
    IrFunction, IrInsn, RepKind, Segment, Slice, Stmt, StopReason, Target, Temp, UnOp, Value, Width,
};

use crate::lazy::{self, Lazy, Pend};

mod span;

/// Everything the emitter needs to know about one owner's place in the
/// image, decided by [`crate::plan`].
pub struct OwnerShape<'a> {
    /// Start selectors the handler accepts: its entry, and the interior
    /// entries and call continuations it is the canonical owner of.
    pub selectors: &'a BTreeSet<u32>,
    /// Loops: each header polls for preemption ([`crate::backedge`]).
    pub loops: &'a crate::backedge::Loops,
    /// Count guest memory accesses in the handler ([`crate::Codegen`]).
    pub accounting: bool,
    /// The module's function entries (each has a handler `h2fn_<entry>`):
    /// a call or tail jump to one may invoke it directly (`H2_DCALL`,
    /// `H2_DJUMP`; RUNTIME-CONTRACT.md §4, "Direct calls"). `None` emits
    /// every transfer for the runner.
    pub functions: Option<&'a BTreeSet<u32>>,
    /// Keep only the registers the handler writes in locals ([`homes`]);
    /// `false` keeps all eight and EFLAGS in locals, as before.
    pub reg_homes: bool,
}

/// [`homes`] bit for EFLAGS and its defined mask (bits 0-7 are the GPRs in
/// encoding order, [`Gpr::index`]).
pub const HOME_FLAGS: u32 = 1 << 8;
/// Upper home-mask bits for XMM0..7, independent of the integer homes switch.
const XMM_BASE: u8 = 9;

/// All integer registers and EFLAGS in locals.
pub const HOME_ALL: u32 = 0x1ff;

/// Which registers `f`'s handler keeps in C locals: those it can write,
/// as a [`Gpr::index`] bit set plus [`HOME_FLAGS`] and bits 9–16 for XMM0–7.
///
/// A register outside the set is never assigned by the handler, so
/// `CpuAbi` holds its current value at every point: on entry, across a
/// direct call (the callee writes it there) and an atomic (the runtime
/// writes it there). The handler reads it from `CpuAbi` where it is used
/// and has nothing to load, spill or reload for it. A register inside the
/// set is loaded on entry, spilled at every exit and before a direct call
/// or atomic, and reloaded after them, exactly as before.
///
/// Partial writes (`AL`, `AH`, `AX`) read the rest of the register, so
/// they count as writes of the whole register and keep it in a local.
/// ESP is always kept: the direct-call macros read the local. EFLAGS is
/// kept whenever any flag is read or written, any `Repeat` runs (it saves
/// and restores EFLAGS and reads DF) and any atomic runs (the runtime
/// writes the flags).
pub fn homes(f: &IrFunction) -> u32 {
    fn walk(list: &[Stmt], m: &mut u32) {
        for s in list {
            match s {
                Stmt::WriteReg { reg, .. } => *m |= 1 << reg.index(),
                Stmt::WriteXmm { reg, .. } => *m |= 1 << (XMM_BASE + *reg),
                Stmt::ReadFlag { .. }
                | Stmt::WriteFlag { .. }
                | Stmt::ReadEflags { .. }
                | Stmt::WriteEflags { .. }
                | Stmt::LazyFlags(_) => *m |= HOME_FLAGS,
                Stmt::If { then, otherwise, .. } => {
                    walk(then, m);
                    walk(otherwise, m);
                }
                Stmt::Repeat { body, .. } => {
                    // The C loop counts ECX and the bulk copy moves ESI/EDI.
                    *m |= HOME_FLAGS | 1 << Gpr::Ecx.index() | 1 << Gpr::Esi.index() | 1 << Gpr::Edi.index();
                    walk(body, m);
                }
                _ => {}
            }
        }
    }
    let mut m = 1 << Gpr::Esp.index();
    for i in f.insns() {
        walk(&i.stmts, &mut m);
        if let Some(a) = &i.atomic {
            m |= HOME_FLAGS;
            if let Some((g, _)) = a.old {
                m |= 1 << g.index();
            }
        }
    }
    m
}

/// Homed registers each statement list may assign (its locals change).
fn assigned(list: &[Stmt]) -> u32 {
    list.iter()
        .map(|s| match s {
            Stmt::WriteReg { reg, .. } => 1 << reg.index(),
            Stmt::WriteXmm { reg, .. } => 1 << (XMM_BASE + *reg),
            Stmt::WriteFlag { .. } | Stmt::WriteEflags { .. } | Stmt::LazyFlags(_) => HOME_FLAGS,
            Stmt::If { then, otherwise, .. } => assigned(then) | assigned(otherwise),
            Stmt::Repeat { body, .. } => {
                HOME_FLAGS | 1 << Gpr::Ecx.index() | 1 << Gpr::Esi.index() | 1 << Gpr::Edi.index() | assigned(body)
            }
            _ => 0,
        })
        .fold(0, |a, m| a | m)
}

/// The homed registers whose local may differ from `CpuAbi` just before
/// each call or tail jump (by instruction PC): what a direct call has to
/// spill before invoking its callee ([`homes`] keeps the rest in `CpuAbi`
/// already).
///
/// A local and its `CpuAbi` field agree on entry (the handler loaded it),
/// after a direct call returns inline or an atomic (both reload every
/// homed local), and after any full spill. Only the statement that assigns
/// a local makes them differ, so this is a forward union over the local
/// CFG of [`assigned`] since the last of those points. Every start
/// selector (entries, continuations, REP restarts, loop resumes) is
/// entered with them agreeing, which adds nothing to the union.
pub fn dirty_at_exits(f: &IrFunction, home: u32) -> BTreeMap<u32, u32> {
    // Dirty at the end of each block, given dirty at its start; records
    // the mask at each call or tail jump when `out` is given.
    let walk = |b: &ir::IrBlock, mut dirty: u32, mut out: Option<&mut BTreeMap<u32, u32>>| {
        for i in &b.insns {
            if let Some(a) = &i.atomic {
                // H2_ATOMIC spills and reloads every homed local; then the
                // old value merges into its register.
                dirty = a.old.map_or(0, |(g, _)| 1 << g.index());
                continue;
            }
            dirty |= assigned(&i.stmts);
            if matches!(i.control, Control::Call { .. } | Control::Jump { tail: true, .. })
                && let Some(o) = out.as_deref_mut()
            {
                o.insert(i.pc, dirty & home);
            }
        }
        dirty
    };
    let succ = |b: &ir::IrBlock| -> Vec<u32> {
        let Some(last) = b.insns.last() else { return vec![] };
        let local = |d: &Dest| (!d.tail).then_some(d.pc);
        match &last.control {
            Control::Next => vec![last.next_pc()],
            Control::Jump { target: Target::Direct(t), tail: false, .. } => vec![*t],
            Control::Branch { taken, fallthrough, .. } => local(taken).into_iter().chain(local(fallthrough)).collect(),
            Control::Switch { cases, .. } => cases.iter().filter_map(local).collect(),
            _ => vec![],
        }
    };
    let mut dirty_in: BTreeMap<u32, u32> = f.blocks.keys().map(|&k| (k, 0)).collect();
    loop {
        let mut changed = false;
        for b in f.blocks.values() {
            let out = walk(b, dirty_in[&b.start], None);
            for t in succ(b) {
                if let Some(d) = dirty_in.get_mut(&t)
                    && *d | out != *d
                {
                    *d |= out;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut masks = BTreeMap::new();
    for b in f.blocks.values() {
        walk(b, dirty_in[&b.start], Some(&mut masks));
    }
    masks
}

/// Whether `f` has a guest load or store on the inline RAM path (an
/// atomic's go through its runtime helper instead).
fn accesses_memory(f: &IrFunction) -> bool {
    fn any(stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            Stmt::Load { .. } | Stmt::Store { .. } => true,
            Stmt::If { then, otherwise, .. } => any(then) || any(otherwise),
            Stmt::Repeat { body, .. } => any(body),
            _ => false,
        })
    }
    f.insns().any(|i| i.atomic.is_none() && any(&i.stmts))
}

/// The C symbol of an owner's handler.
pub fn symbol(entry: u32) -> String {
    format!("h2fn_{entry:08x}")
}

/// Emit one owner as a C function definition.
pub fn function(f: &IrFunction, shape: &OwnerShape<'_>) -> String {
    let lazy = Lazy::of(f);
    let entry_of_settle = format!("{:08x}", f.entry);
    let x87 = crate::x87::uses_x87(f);
    // With accounting a REP loop polls too, so it needs the poll's locals.
    let polled = !shape.loops.headers.is_empty()
        || (shape.accounting && f.insns().any(|i| i.stmts.iter().any(|s| matches!(s, Stmt::Repeat { .. }))));
    let home = homes(f) | if shape.reg_homes { 0 } else { HOME_ALL };
    let dirty = if shape.reg_homes { dirty_at_exits(f, home) } else { BTreeMap::new() };
    let mut e = Emitter {
        out: String::new(),
        shape,
        home,
        dirty,
        in_repeat: false,
        x87,
        polled,
        fp_ready: false,
        span: None,
        top: if x87 { crate::x87::top::hints(f) } else { BTreeMap::new() },
        lazy,
        pend: Pend::default(),
        entry_of_settle,
        direct: BTreeSet::new(),
    };
    e.function(f);
    // Prototypes of the handlers this one may invoke directly.
    let mut out: String = e
        .direct
        .iter()
        .map(|t| format!("TransferAbi {}(CpuAbi *, ExecCtxOpaque *, uint32_t);\n", symbol(*t)))
        .collect();
    out.push_str(&e.out);
    out
}

/// Recognize the complete flat MOVS iteration, including its address updates.
/// Never infer memory behavior from a mnemonic or an instruction byte string.
fn rep_movs_width(body: &[Stmt]) -> Option<u32> {
    let [
        Stmt::ReadFlag { dst: df, flag: Flag::Df },
        Stmt::Assign { dst: step, expr: Expr::Select { cond, a: neg, b: pos } },
        Stmt::ReadReg { dst: src, reg: Gpr::Esi, slice: Slice::Full },
        Stmt::ReadReg { dst, reg: Gpr::Edi, slice: Slice::Full },
        Stmt::Load { dst: value, addr: from },
        Stmt::Store { addr: to, value: stored },
        Stmt::Assign { dst: next_src, expr: Expr::Binary { op: BinOp::Add, a: sa, b: ss } },
        Stmt::WriteReg { reg: Gpr::Esi, slice: Slice::Full, value: new_src },
        Stmt::Assign { dst: next_dst, expr: Expr::Binary { op: BinOp::Add, a: da, b: ds } },
        Stmt::WriteReg { reg: Gpr::Edi, slice: Slice::Full, value: new_dst },
    ] = body else { return None };
    let width = value.width.bytes();
    (matches!(width, 1 | 2 | 4) && *cond == Value::Temp(*df)
        && *pos == Value::u32(width) && *neg == Value::u32(width.wrapping_neg())
        && *from == (Addr { seg: Segment::Flat, offset: Value::Temp(*src) })
        && *to == (Addr { seg: Segment::Flat, offset: Value::Temp(*dst) })
        && *stored == Value::Temp(*value)
        && *sa == Value::Temp(*src) && *ss == Value::Temp(*step)
        && *da == Value::Temp(*dst) && *ds == Value::Temp(*step)
        && *new_src == Value::Temp(*next_src) && *new_dst == Value::Temp(*next_dst))
        .then_some(width)
}

struct Emitter<'a> {
    out: String,
    shape: &'a OwnerShape<'a>,
    /// Registers kept in locals ([`homes`]); the others are read from
    /// `CpuAbi`.
    home: u32,
    /// [`dirty_at_exits`]: what each direct call or tail jump spills.
    dirty: BTreeMap<u32, u32>,
    /// Emitting a `Repeat` body: exits restore the saved EFLAGS first.
    in_repeat: bool,
    /// The handler has x87 ops: it has the x87 locals and fail block.
    x87: bool,
    /// The handler polls (loop headers, or REP loops with accounting): it
    /// has `H2_PREEMPT_LOCALS`.
    polled: bool,
    /// A host FP control sample dominates the current register-only run.
    fp_ready: bool,
    span: Option<(u32, span::Span)>,
    /// Guarded x87 TOP predictions at instruction entry.
    top: BTreeMap<u32, u8>,
    /// The handler's lazy flags, if it has any.
    lazy: Option<Lazy>,
    /// What may be pending at the statement being emitted.
    pend: Pend,
    /// `h2lz_` suffix: the handler's entry, as `%08x`.
    entry_of_settle: String,
    /// Handlers invoked directly, which need prototypes.
    direct: BTreeSet<u32>,
}

/// C for `reg[slice] = v` (a partial-register merge below `Full`); `v` is
/// a C expression of the slice's width or wider.
fn write_reg(g: Gpr, slice: Slice, v: &str) -> String {
    let r = reg_name(g);
    match slice {
        Slice::Full => format!("{r} = (uint32_t)({v});"),
        Slice::Low16 => format!("{r} = ({r} & 0xffff0000u) | (uint32_t)(uint16_t)({v});"),
        Slice::Low8 => format!("{r} = ({r} & 0xffffff00u) | (uint32_t)(uint8_t)({v});"),
        Slice::High8 => format!("{r} = ({r} & 0xffff00ffu) | ((uint32_t)(uint8_t)({v}) << 8);"),
    }
}

/// C storage type for a width. `W1` values are `uint8_t` holding 0 or 1.
pub(crate) fn ty(w: Width) -> &'static str {
    match w {
        Width::W1 | Width::W8 => "uint8_t",
        Width::W16 => "uint16_t",
        Width::W32 => "uint32_t",
        Width::W64 => "uint64_t",
        Width::W128 | Width::W80 => "h2u128",
    }
}

/// The unsigned type arithmetic is done in: at least `unsigned int`, so
/// narrow operands are never promoted to signed `int`.
fn uty(w: Width) -> &'static str {
    match w {
        Width::W1 | Width::W8 | Width::W16 | Width::W32 => "uint32_t",
        Width::W64 => "uint64_t",
        Width::W128 | Width::W80 => "h2u128",
    }
}

/// Helper-name suffix (`h2_shl32`).
fn n(w: Width) -> u32 {
    match w {
        Width::W1 => 8,
        w => w.bits(),
    }
}

fn lit(w: Width, bits: u128) -> String {
    match w {
        Width::W1 | Width::W8 => format!("(uint8_t){bits:#x}u"),
        Width::W16 => format!("(uint16_t){bits:#x}u"),
        Width::W32 => format!("{bits:#x}u"),
        Width::W64 => format!("UINT64_C({bits:#x})"),
        Width::W128 | Width::W80 => {
            let (hi, lo) = ((bits >> 64) as u64, bits as u64);
            if hi == 0 {
                format!("((h2u128)UINT64_C({lo:#x}))")
            } else {
                format!("(((h2u128)UINT64_C({hi:#x}) << 64) | UINT64_C({lo:#x}))")
            }
        }
    }
}

/// An unsigned constant in a wider arithmetic type (`uty`).
fn ulit(w: Width, bits: u128) -> String {
    match uty(w) {
        "uint32_t" => format!("{bits:#x}u"),
        "uint64_t" => format!("UINT64_C({bits:#x})"),
        _ => lit(Width::W128, bits),
    }
}

/// C running `arms[i]` when `lzk` is `kinds[i]`: the arm alone for one
/// kind, a `switch` for several.
fn by_kind(kinds: &[u32], arms: &[String]) -> String {
    match kinds {
        [_] => arms[0].clone(),
        _ => {
            let cases: String = kinds.iter().zip(arms).map(|(k, a)| format!("case {k}u: {{ {a}break; }} ")).collect();
            format!("switch (lzk) {{ {cases}default: break; }} ")
        }
    }
}

pub(crate) fn val(v: &Value) -> String {
    match v {
        Value::Temp(t) => format!("t{}", t.id),
        Value::Const { width, bits } => lit(*width, *bits),
    }
}

/// Cast an arithmetic-type expression back to `w`'s storage type.
fn wrap(w: Width, s: String) -> String {
    if w == Width::W1 { format!("(uint8_t)(({s}) & 1u)") } else { format!("({})({s})", ty(w)) }
}

fn reg_name(g: Gpr) -> &'static str {
    ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"][g.index()]
}

fn c_string(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push('"');
    out
}

/// `H2_RAISE(pc, saved_eip, code, arg)` for a guest exception at `insn`.
/// Traps report the next instruction as the saved EIP, faults their own.
fn raise(f: Fault, insn: &IrInsn) -> String {
    let (code, arg) = match f {
        Fault::DivideError => ("H2SHIM_DIVIDE_ERROR", "0u".into()),
        Fault::Breakpoint => ("H2SHIM_BREAKPOINT", "0u".into()),
        Fault::InvalidOpcode => ("H2SHIM_INVALID_OPCODE", "0u".into()),
        Fault::Interrupt(v) => ("H2SHIM_INTERRUPT", format!("{v}u")),
        Fault::Overflow => ("H2SHIM_OVERFLOW", "0u".into()),
        Fault::BoundRange => ("H2SHIM_BOUND_RANGE", "0u".into()),
        Fault::GeneralProtection => ("H2SHIM_GENERAL_PROTECTION", "0u".into()),
        Fault::SimdException => ("H2SHIM_SIMD_EXCEPTION", "0u".into()),
        // #MF reports the status word as it is when raised.
        Fault::FpError => ("H2SHIM_FP_ERROR", "(uint32_t)cpu->x87.status".into()),
    };
    let trap = matches!(f, Fault::Breakpoint | Fault::Interrupt(_) | Fault::Overflow);
    let saved = if trap { insn.next_pc() } else { insn.pc };
    format!("H2_RAISE({:#x}u, {saved:#x}u, {code}, {arg});", insn.pc)
}

fn stop_code(r: StopReason) -> &'static str {
    match r {
        StopReason::Halt => "H2SHIM_HALT",
        StopReason::UnknownTarget => "H2SHIM_UNKNOWN_TARGET",
        StopReason::Unsupported => "H2SHIM_UNKNOWN_TARGET",
        StopReason::DescentError => "H2SHIM_DESCENT_ERROR",
        StopReason::Unmodelled => "H2SHIM_UNMODELLED",
    }
}

impl Emitter<'_> {
    fn line(&mut self, depth: usize, s: impl AsRef<str>) {
        for _ in 0..depth {
            self.out.push_str("    ");
        }
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }

    fn function(&mut self, f: &IrFunction) {
        let name = symbol(f.entry);
        if self.lazy.is_some() {
            self.settle_fn(f.entry);
        }
        self.line(0, format!("TransferAbi {name}(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {{"));
        self.line(1, format!("H2_FLAT_OWNER({:#x}u);", f.entry));
        self.line(1, format!("H2_LOCALS_HOMED({:#x}u);", self.home));
        if let Some(l) = &self.lazy {
            let slots: Vec<String> = (0..l.slots).map(|i| format!("lz{i} = 0")).collect();
            self.line(1, format!("uint32_t lzk = 0, lzm = 0; uint64_t {};", slots.join(", ")));
        }
        let x87 = self.x87;
        if x87 {
            self.line(1, "H2X87_LOCALS();");
        }
        let headers = &self.shape.loops.headers;
        if self.polled {
            self.line(1, "H2_PREEMPT_LOCALS();");
        }
        // After every call the entry makes (acquire, the budget): a handler
        // that loads or stores may take the RAM gate now (H2_ENTRY_GATE).
        if accesses_memory(f) {
            self.line(1, "H2_GATE_ENTER();");
        }
        self.line(1, "int h2fp_ __attribute__((unused));");
        self.line(1, "switch (start) {");
        for s in self.shape.selectors {
            // Entered from the runner, which is a safe point: skip the poll.
            let label = if headers.contains(s) { "P" } else { "L" };
            self.line(1, format!("case {s:#010x}u: goto {label}_{s:08x};"));
        }
        self.line(1, "default: break;");
        self.line(1, "}");
        self.line(1, "{ H2_GATE_DROP(); uint32_t e_ = h2shim_v1_event(ctx, start, start, H2SHIM_BAD_ENTRY, start); H2_STOP_EVENT(start, e_); }");
        for b in f.blocks.values() {
            self.fp_ready = false;
            self.line(0, format!("L_{:08x}:;", b.start));
            if let Some(l) = &self.lazy {
                self.pend = l.entry[&b.start];
            }
            if headers.contains(&b.start) {
                // Every way round the loop passes here. clang rotates the
                // poll onto the back-edge: one load and a branch per trip.
                self.line(1, format!("H2_LOOP_POLL({:#x}u);", b.start));
                self.line(0, format!("P_{:08x}:;", b.start));
            }
            for (i, insn) in b.insns.iter().enumerate() {
                let last = i + 1 == b.insns.len();
                // A selector inside a block (a REP restart point): every
                // local is live from H2_LOCALS, so entering here is sound.
                if i > 0 && self.shape.selectors.contains(&insn.pc) {
                    self.fp_ready = false;
                    self.line(0, format!("L_{:08x}:;", insn.pc));
                }
                if self.span.is_none() {
                    if let Some(span) = span::group(&b.insns[i..], self.shape.selectors) {
                        let base = span.reg.map_or_else(|| "0u".into(), |r| {
                            let name = reg_name(r);
                            if self.home & (1 << r.index()) != 0 { name.to_string() } else { format!("cpu->{name}") }
                        });
                        self.line(1, format!("H2_SPAN(sp_{:08x}, (uint32_t)({base} + {:#x}u), {}u, {}u);",
                            insn.pc, span.start, span.len, span.rights));
                        self.span = Some((insn.pc, span));
                    }
                }
                self.insn(insn, last);
                if self.span.as_ref().is_some_and(|(_, s)| s.offsets.is_empty()) {
                    self.span = None;
                }
            }
        }
        if x87 {
            self.line(1, "H2X87_FAIL();");
        }
        if self.lazy.is_some() {
            // The epilogue, with everything still pending settled first.
            self.line(0, "h2_exit:;");
            let call = self.settle_call("lzm");
            self.line(1, format!("if (lzm) {{ fl = {call}; fd |= lzm; }}"));
            if x87 {
                self.line(1, "H2X87_SPILL();");
            }
            self.line(1, "H2_EPILOGUE_BODY();");
        } else if x87 {
            self.line(1, "H2X87_EPILOGUE();");
        } else {
            self.line(1, "H2_EPILOGUE();");
        }
        self.line(0, "}");
    }

    /// `h2lz_<entry>(kind, mask, slots..., fl)`: EFLAGS with the flags in
    /// `mask` of formula `kind` computed from the slots.
    fn settle_fn(&mut self, entry: u32) {
        let l = self.lazy.as_ref().unwrap();
        let formulas = l.formulas.clone();
        let slots: Vec<String> = (0..l.slots).map(|i| format!("uint64_t a{i}")).collect();
        self.line(0, format!("static uint32_t h2lz_{entry:08x}(uint32_t k, uint32_t m, {}, uint32_t fl) {{", slots.join(", ")));
        self.line(1, "switch (k) {");
        for (k, fm) in formulas.iter().enumerate() {
            self.line(1, format!("case {k}u: {{"));
            for (i, w) in fm.params.iter().enumerate() {
                self.line(2, format!("{t} t{i} = ({t})a{i};", t = ty(*w)));
            }
            for (t, e) in &fm.body {
                let rhs = self.expr(*t, e);
                self.line(2, format!("{} t{} = {rhs};", ty(t.width), t.id));
            }
            for (flag, v) in &fm.flags {
                let b = flag.eflags_bit();
                self.line(2, format!("if (m & {:#x}u) fl = (fl & ~{:#x}u) | ((uint32_t)({} & 1u) << {b});", 1u32 << b, 1u32 << b, val(v)));
            }
            self.line(2, "break;");
            self.line(1, "}");
        }
        self.line(1, "default: break;");
        self.line(1, "}");
        self.line(1, "return fl;");
        self.line(0, "}");
    }

    fn settle_call(&self, mask: &str) -> String {
        let l = self.lazy.as_ref().unwrap();
        let slots: Vec<String> = (0..l.slots).map(|i| format!("lz{i}")).collect();
        format!("h2lz_{}(lzk, {mask}, {}, fl)", self.entry_of_settle, slots.join(", "))
    }

    /// Settle the flags in `mask` that may be pending, before they are read
    /// or the slots change: in place when the formula is known, else
    /// through the handler's switch.
    fn settle(&mut self, mask: u32, d: usize) {
        let m = mask & self.pend.mask;
        if m == 0 {
            return;
        }
        let set = match self.pend.known() {
            Some(kinds) => {
                let arms: Vec<String> = kinds
                    .iter()
                    .map(|&k| {
                        let (mut body, values) = self.formula_inline(k);
                        for (flag, v) in values.iter().filter(|(f, _)| m & (1 << f.eflags_bit()) != 0) {
                            let b = flag.eflags_bit();
                            body.push_str(&format!(
                                "if (m_ & {:#x}u) fl = (fl & ~{:#x}u) | ((uint32_t)({v} & 1u) << {b}); ",
                                1u32 << b,
                                1u32 << b,
                            ));
                        }
                        body
                    })
                    .collect();
                by_kind(&kinds, &arms)
            }
            None => format!("fl = {}; ", self.settle_call("m_")),
        };
        self.line(d, format!("{{ uint32_t m_ = lzm & {m:#x}u; if (m_) {{ {set}fd |= m_; lzm &= ~m_; }} }}"));
        self.pend = self.pend.without(m);
    }

    /// Formula `k` computed from the slots, as declarations (temps renamed
    /// clear of the instruction's own) and each flag's C value.
    fn formula_inline(&self, k: u32) -> (String, Vec<(Flag, String)>) {
        const BASE: u32 = 1 << 20;
        let fm = &self.lazy.as_ref().unwrap().formulas[k as usize];
        let rn = |v: &Value| match v {
            Value::Temp(t) => Value::Temp(Temp { id: t.id + BASE, width: t.width }),
            c => *c,
        };
        let mut body = String::new();
        for (i, w) in fm.params.iter().enumerate() {
            body.push_str(&format!("{t} t{} = ({t})lz{i}; ", i as u32 + BASE, t = ty(*w)));
        }
        for (t, e) in &fm.body {
            let e = match *e {
                Expr::Unary { op, a } => Expr::Unary { op, a: rn(&a) },
                Expr::Binary { op, a, b } => Expr::Binary { op, a: rn(&a), b: rn(&b) },
                Expr::Compare { op, a, b } => Expr::Compare { op, a: rn(&a), b: rn(&b) },
                Expr::Cast { op, a } => Expr::Cast { op, a: rn(&a) },
                Expr::Select { cond, a, b } => Expr::Select { cond: rn(&cond), a: rn(&a), b: rn(&b) },
            };
            let t = Temp { id: t.id + BASE, width: t.width };
            body.push_str(&format!("{} t{} = {}; ", ty(t.width), t.id, self.expr(t, &e)));
        }
        (body, fm.flags.iter().map(|(f, v)| (*f, val(&rn(v)))).collect())
    }

    fn insn(&mut self, insn: &IrInsn, last: bool) {
        let pc = insn.pc;
        self.line(1, format!("/* {pc:#010x} */ {{"));
        match &insn.atomic {
            Some(a) => {
                // The runtime reads the spilled flags.
                self.settle(lazy::ALL, 2);
                self.atomic(insn, a);
                self.fp_ready = false;
                self.pend = Pend::default();
            }
            None => self.stmts(&insn.stmts, insn, 2, self.top.get(&pc).copied()),
        }
        self.control(insn, last);
        self.line(1, "}");
    }

    /// With accounting, charge the handler's count before something that
    /// spends `extra` accesses of its own or observes the slice. Unrelated
    /// to [`Self::settle`], which materialises lazy flags.
    fn acc_settle(&mut self, d: usize, extra: u32) {
        if self.shape.accounting {
            let m = if self.polled { "H2_ACC_SETTLE_POLLED" } else { "H2_ACC_SETTLE" };
            self.line(d, format!("{m}({extra}u);"));
        }
    }

    /// An atomic instruction (validated shape: pure prefix, one load, one
    /// store): the prefix, then the runtime's operation-wide accessor in
    /// place of the load..store statements.
    fn atomic(&mut self, insn: &IrInsn, a: &Atomic) {
        let pc = insn.pc;
        let li = insn.stmts.iter().position(|s| matches!(s, Stmt::Load { .. })).expect("validated atomic shape");
        let Stmt::Load { dst, addr } = &insn.stmts[li] else { unreachable!() };
        self.stmts(&insn.stmts[..li], insn, 2, None);
        let addr = self.addr(addr);
        let n = dst.width.bytes();
        let operand = a.operand.map_or("0u".to_string(), |v| format!("(uint32_t){}", val(&v)));
        let call = match a.op {
            AtomicOp::Xchg => format!("h2abi_v1_xchg(cpu, ctx, {pc:#x}u, {addr}, {n}u, {operand}, &o_)"),
            AtomicOp::Cmpxchg => format!("h2abi_v1_cmpxchg(cpu, ctx, {pc:#x}u, {addr}, {n}u, {operand}, &o_)"),
            op => format!(
                "h2abi_v1_atomic_rmw(cpu, ctx, {pc:#x}u, {addr}, {n}u, {}u, {operand}, &o_)",
                op.rmw_tag().expect("read-modify-write op")
            ),
        };
        // The atomic spends its own access: settle the count before it.
        self.acc_settle(2, 1);
        self.line(2, format!("H2_ATOMIC({pc:#x}u, {call});"));
        if let Some((g, s)) = a.old {
            self.line(2, write_reg(g, s, "o_"));
        }
    }

    /// What every exit from inside a `Repeat` body does first.
    fn restore(&self) -> &'static str {
        if self.in_repeat { "fl = rfl_; fd = rfd_; " } else { "" }
    }

    fn addr(&self, a: &Addr) -> String {
        match a.seg {
            Segment::Flat => val(&a.offset),
            Segment::Fs => format!("(uint32_t)(cpu->fs_base + {})", val(&a.offset)),
        }
    }

    fn expr(&self, dst: Temp, e: &Expr) -> String {
        let w = dst.width;
        match e {
            Expr::Unary { op, a } => {
                let aw = a.width();
                let x = val(a);
                match op {
                    UnOp::Not => wrap(w, format!("~({})({x})", uty(aw))),
                    UnOp::Neg => wrap(w, format!("({u})0 - ({u})({x})", u = uty(aw))),
                    UnOp::Parity => format!("h2_parity((uint8_t){x})"),
                    UnOp::Ctz => format!("h2_ctz{}({x})", n(aw)),
                    UnOp::Clz => format!("h2_clz{}({x})", n(aw)),
                    UnOp::Bswap => format!("h2_bswap{}({x})", n(aw)),
                }
            }
            Expr::Binary { op, a, b } => {
                let aw = a.width();
                let (x, y) = (val(a), val(b));
                let u = uty(aw);
                // At a native width the operands already have the
                // arithmetic type: no casts needed, no promotion possible.
                let native = u == ty(aw) && w == aw;
                let infix = |o: &str| {
                    if native { format!("{x} {o} {y}") } else { wrap(w, format!("({u})({x}) {o} ({u})({y})")) }
                };
                let call = |f: &str| format!("h2_{f}{}({x}, {y})", n(aw));
                match op {
                    BinOp::Add => infix("+"),
                    BinOp::Sub => infix("-"),
                    BinOp::Mul => infix("*"),
                    BinOp::And => infix("&"),
                    BinOp::Or => infix("|"),
                    BinOp::Xor => infix("^"),
                    BinOp::Shl => call("shl"),
                    BinOp::Shr => call("shr"),
                    BinOp::Sar => call("sar"),
                    BinOp::Rol => call("rol"),
                    BinOp::Ror => call("ror"),
                    BinOp::Udiv => call("udiv"),
                    BinOp::Urem => call("urem"),
                    BinOp::Sdiv => call("sdiv"),
                    BinOp::Srem => call("srem"),
                }
            }
            Expr::Compare { op, a, b } => {
                let aw = a.width();
                let u = uty(aw);
                let (x, y) = (format!("({u})({})", val(a)), format!("({u})({})", val(b)));
                let sign = ulit(aw, 1u128 << (aw.bits() - 1));
                let c = match op {
                    CmpOp::Eq => format!("{x} == {y}"),
                    CmpOp::Ne => format!("{x} != {y}"),
                    CmpOp::Ult => format!("{x} < {y}"),
                    CmpOp::Ule => format!("{x} <= {y}"),
                    // Signed order is unsigned order with the sign bit flipped.
                    CmpOp::Slt => format!("({x} ^ {sign}) < ({y} ^ {sign})"),
                    CmpOp::Sle => format!("({x} ^ {sign}) <= ({y} ^ {sign})"),
                };
                format!("(uint8_t)({c})")
            }
            Expr::Cast { op, a } => {
                let x = val(a);
                let u = uty(w);
                match op {
                    CastOp::Zext | CastOp::Trunc => wrap(w, format!("({u})({x})")),
                    CastOp::Sext => {
                        // (x ^ s) - s sign-extends an unsigned pattern whose
                        // sign bit is s, with no signed conversion.
                        let s = ulit(w, 1u128 << (a.width().bits() - 1));
                        wrap(w, format!("((({u})({x})) ^ {s}) - {s}"))
                    }
                }
            }
            Expr::Select { cond, a, b } => format!("({} ? {} : {})", val(cond), val(a), val(b)),
        }
    }

    fn stmts(&mut self, list: &[Stmt], insn: &IrInsn, d: usize, mut top: Option<u8>) {
        let pc = insn.pc;
        for s in list {
            let at = top;
            if top.is_some() {
                top = crate::x87::top::after(top, s);
            }
            // Only straight-line, register-only IR can keep the host sample.
            // In particular even a successful guest load could have called
            // a host MMIO handler, and a conditional body may never execute.
            use ir::SseOp::*;
            let native_fp = match s {
                Stmt::Intrinsic { op: Intrinsic::Sse(op @ (AddSs | AddPs | SubSs | SubPs | MulSs |
                    MulPs | DivSs | DivPs | SqrtSs | SqrtPs)), .. } if d == 2 => Some(*op),
                _ => None,
            };
            if native_fp.is_none() && !matches!(s, Stmt::Assign { .. } | Stmt::ReadReg { .. }
                | Stmt::WriteReg { .. } | Stmt::ReadXmm { .. } | Stmt::WriteXmm { .. }
                | Stmt::ReadFlag { .. } | Stmt::WriteFlag { .. } | Stmt::LazyFlags(_))
            {
                self.fp_ready = false;
            }
            // A flag read from known formulas is computed in place.
            let inline = matches!(s, Stmt::ReadFlag { .. }) && self.pend.known().is_some();
            self.settle(if inline { 0 } else { lazy::reads(s) }, d);
            // An eager write takes a pending flag back from the formula.
            let taken = self.pend.mask
                & match s {
                    Stmt::WriteFlag { flag, .. } => 1 << flag.eflags_bit(),
                    Stmt::WriteEflags { mask, .. } => *mask,
                    _ => 0,
                };
            if taken != 0 {
                self.line(d, format!("lzm &= ~{taken:#x}u;"));
                self.pend = self.pend.without(taken);
            }
            match s {
                Stmt::Assign { dst, expr } => {
                    let rhs = self.expr(*dst, expr);
                    self.line(d, format!("{} t{} = {rhs};", ty(dst.width), dst.id));
                }
                Stmt::ReadReg { dst, reg, slice } => {
                    // A register the handler never writes is current in
                    // CpuAbi (`homes`).
                    let r = if self.home & 1 << reg.index() != 0 {
                        reg_name(*reg).to_string()
                    } else {
                        format!("cpu->{}", reg_name(*reg))
                    };
                    let rhs = match slice {
                        Slice::Full => r.clone(),
                        Slice::Low16 => format!("(uint16_t){r}"),
                        Slice::Low8 => format!("(uint8_t){r}"),
                        Slice::High8 => format!("(uint8_t)({r} >> 8)"),
                    };
                    self.line(d, format!("{} t{} = {rhs};", ty(dst.width), dst.id));
                }
                Stmt::WriteReg { reg, slice, value } => {
                    let r = reg_name(*reg);
                    let v = val(value);
                    let s = match slice {
                        Slice::Full => format!("{r} = {v};"),
                        Slice::Low16 => format!("{r} = ({r} & 0xffff0000u) | (uint32_t){v};"),
                        Slice::Low8 => format!("{r} = ({r} & 0xffffff00u) | (uint32_t){v};"),
                        Slice::High8 => format!("{r} = ({r} & 0xffff00ffu) | ((uint32_t){v} << 8);"),
                    };
                    self.line(d, s);
                }
                Stmt::ReadFlag { dst, flag } => {
                    let b = flag.eflags_bit();
                    match self.pend.known() {
                        Some(kinds) if self.pend.mask & (1 << b) != 0 => {
                            // Pending (on some paths, at least): the
                            // formula's value, else the stored bit.
                            let id = dst.id;
                            let kinds: Vec<u32> = kinds
                                .into_iter()
                                .filter(|&k| self.lazy.as_ref().unwrap().formulas[k as usize].mask() & (1 << b) != 0)
                                .collect();
                            let arms: Vec<String> = kinds
                                .iter()
                                .map(|&k| {
                                    let (body, values) = self.formula_inline(k);
                                    let v = &values.iter().find(|(f, _)| f == flag).unwrap().1;
                                    format!("{body}t{id} = (uint8_t)({v} & 1u); ")
                                })
                                .collect();
                            self.line(
                                d,
                                format!(
                                    "uint8_t t{id} = H2_FLAG({b}); if (lzm & {:#x}u) {{ {}}}",
                                    1u32 << b,
                                    by_kind(&kinds, &arms)
                                ),
                            );
                        }
                        _ => self.line(d, format!("uint8_t t{} = H2_FLAG({b});", dst.id)),
                    }
                }
                Stmt::WriteFlag { flag, value } => match value {
                    FlagValue::Defined(v) => self.line(d, format!("H2_SETF({}, {});", flag.eflags_bit(), val(v))),
                    FlagValue::Undefined => self.line(d, format!("H2_UNDEFF({});", flag.eflags_bit())),
                },
                Stmt::ReadEflags { dst } => self.line(d, format!("uint32_t t{} = fl | 2u;", dst.id)),
                Stmt::WriteEflags { value, mask } => {
                    self.line(d, format!("fl = (fl & ~{mask:#x}u) | ({} & {mask:#x}u) | 2u; fd |= {mask:#x}u;", val(value)))
                }
                Stmt::ReadXmm { dst, reg } => self.line(d, format!("h2u128 t{} = H2_READ_XMM({reg});", dst.id)),
                Stmt::WriteXmm { reg, value } => self.line(d, format!("H2_WRITE_XMM({reg}, {});", val(value))),
                Stmt::ReadMmx { dst, reg } => self.line(d, format!("uint64_t t{} = h2_rmm(cpu, {reg});", dst.id)),
                Stmt::WriteMmx { reg, value } => self.line(d, format!("h2_wmm(cpu, {reg}, {});", val(value))),
                Stmt::Load { dst, addr } => {
                    let a = self.addr(addr);
                    let w = dst.width;
                    let args = format!("{}, {}, t{}, {pc:#x}u, {a}", ty(w), w.bytes(), dst.id);
                    if let Some((start, group)) = &mut self.span {
                        let off = group.offsets.pop_front().expect("group covers this load");
                        let call = format!("H2_SPAN_LD(sp_{start:08x}, {off}u, {args});");
                        self.line(d, call);
                    } else {
                        let m = if self.in_repeat { "H2_LDR" } else { "H2_LD" };
                        self.line(d, format!("{m}({args});"));
                    }
                }
                Stmt::Store { addr, value } => {
                    let a = self.addr(addr);
                    let w = value.width();
                    let args = format!("{}, {}, {pc:#x}u, {a}, {}", ty(w), w.bytes(), val(value));
                    if let Some((start, group)) = &mut self.span {
                        let off = group.offsets.pop_front().expect("group covers this store");
                        let call = format!("H2_SPAN_ST(sp_{start:08x}, {off}u, {args});");
                        self.line(d, call);
                    } else {
                        let m = if self.in_repeat { "H2_STR" } else { "H2_ST" };
                        self.line(d, format!("{m}({args});"));
                    }
                }
                Stmt::FaultIf { cond, fault } => {
                    self.line(d, format!("if ({}) {{ {}{} }}", val(cond), self.restore(), raise(*fault, insn)));
                }
                Stmt::StopIf { cond, reason } => {
                    self.line(
                        d,
                        format!("if ({}) {{ {}H2_HALT({pc:#x}u, {}, 0u); }}", val(cond), self.restore(), stop_code(*reason)),
                    );
                }
                Stmt::If { cond, then, otherwise } => {
                    self.line(d, format!("if ({}) {{", val(cond)));
                    let before = self.pend;
                    self.stmts(then, insn, d + 1, None);
                    let then_pend = std::mem::replace(&mut self.pend, before);
                    if !otherwise.is_empty() {
                        self.line(d, "} else {");
                        self.stmts(otherwise, insn, d + 1, None);
                    }
                    self.pend = self.pend.join(then_pend);
                    self.line(d, "}");
                }
                Stmt::Intrinsic { op: Intrinsic::ReadTsc, dsts, .. } => {
                    // The counter is the runtime's; it cannot fail. Guest
                    // time may follow the accesses spent: settle first.
                    self.acc_settle(d, 0);
                    self.line(d, "H2_GATE_DROP();");
                    self.line(d, format!("uint64_t t{} = h2shim_v1_rdtsc(ctx, {pc:#x}u);", dsts[0].id));
                }
                // In an x87 handler the wait shares the fail block: #MF
                // there is the same event 14 with the status word.
                Stmt::Intrinsic { op: Intrinsic::X87Wait, .. } if self.x87 && !self.in_repeat => {
                    self.line(d, format!("H2X87_WAIT({pc:#x}u);"));
                }
                Stmt::Intrinsic { op: Intrinsic::X87(op), args, dsts } => {
                    for l in crate::x87::stmt(*op, args, dsts, pc, at) {
                        self.line(d, l);
                    }
                }
                Stmt::Intrinsic { op, args, dsts } => {
                    // Results are locals of the instruction; the helper
                    // writes them through pointers and works on FP state in
                    // CpuAbi directly (never cached in locals).
                    for t in dsts {
                        self.line(d, format!("{} t{} = 0;", ty(t.width), t.id));
                    }
                    let mut call = if let Some(sse) = native_fp {
                        if !self.fp_ready {
                            self.line(d, "h2fp_ = H2_FP_SAMPLE();");
                            self.fp_ready = true;
                        }
                        format!("H2_SSE_RUN(h2fp_, {}, cpu", sse.name())
                    } else {
                        format!("h2i_{}(cpu", op.name())
                    };
                    for a in args {
                        call.push_str(&format!(", {}", val(a)));
                    }
                    for t in dsts {
                        call.push_str(&format!(", &t{}", t.id));
                    }
                    // In an x87 handler the pointer locals reach CpuAbi before
                    // the event (an #MF from emms reports the raiser).
                    let spill = if self.x87 { "H2X87_SPILL(); " } else { "" };
                    self.line(
                        d,
                        format!("{{ uint32_t s_ = {call}); if (s_) {{ {}{spill}H2_RAISE({pc:#x}u, {pc:#x}u, s_, H2_INTRINSIC_ARG(s_)); }} }}", self.restore()),
                    );
                }
                Stmt::Repeat { kind, body } => {
                    // One architectural iteration per loop trip, fully
                    // committed, matching the IR's Repeat contract; EFLAGS
                    // as the instruction began, for exits from the body.
                    self.line(d, "uint32_t rfl_ = fl, rfd_ = fd; (void)rfl_; (void)rfd_;");
                    self.line(d, "for (;;) {");
                    self.line(d + 1, "if (!ecx) break;");
                    // A long REP gives the RAM gate up every few trips.
                    self.line(d + 1, "h2_gate_tick(h2_mem_, &h2g_);");
                    if *kind == RepKind::Rep && let Some(width) = rep_movs_width(body) {
                        let limit = if self.shape.accounting { "(h2left > h2acc ? h2left - h2acc : 0u)" } else { "UINT64_MAX" };
                        // The bulk copy takes the gate itself.
                        self.line(d + 1, "H2_GATE_DROP();");
                        self.line(d + 1, format!("uint32_t n_ = h2_fast_movs(h2_mem_, esi, edi, ecx, {width}u, H2_FLAG(10), {limit});"));
                        self.line(d + 1, "if (n_) {");
                        self.line(d + 2, "H2_FLAT_NOTE(H2FC_TOTAL, 2ull * n_); H2_FLAT_NOTE(H2FC_BULK, 2ull * n_);");
                        self.line(d + 2, format!("uint32_t delta_ = n_ * {width}u; if (H2_FLAG(10)) delta_ = 0u - delta_; esi += delta_; edi += delta_; ecx -= n_;"));
                        if self.shape.accounting {
                            self.line(d + 2, "h2acc += (uint64_t)n_ * 2u;");
                            self.line(d + 2, format!("if (ecx) H2_REP_POLL({pc:#x}u);"));
                        }
                        self.line(d + 2, "continue;");
                        self.line(d + 1, "}");
                    }
                    self.line(d + 1, "{");
                    self.in_repeat = true;
                    self.stmts(body, insn, d + 2, None);
                    self.in_repeat = false;
                    self.line(d + 1, "}");
                    self.line(d + 1, "ecx -= 1u;");
                    let zf = Flag::Zf.eflags_bit();
                    match kind {
                        RepKind::Rep => {}
                        RepKind::RepE => self.line(d + 1, format!("if (!H2_FLAG({zf})) break;")),
                        RepKind::RepNe => self.line(d + 1, format!("if (H2_FLAG({zf})) break;")),
                    }
                    if self.shape.accounting {
                        // A bounded poll interval: one iteration.
                        self.line(d + 1, format!("if (ecx) H2_REP_POLL({pc:#x}u);"));
                    }
                    self.line(d, "}");
                }
                Stmt::LazyFlags(l) => {
                    let m = l.formula.mask();
                    // The slots are about to change: settle what this does
                    // not rewrite.
                    self.settle(!m, d);
                    let k = self.lazy.as_ref().unwrap().kind(&l.formula);
                    let pending = Pend::of(m, k);
                    let mut line = String::new();
                    for (i, a) in l.args.iter().enumerate() {
                        line.push_str(&format!("lz{i} = (uint64_t){}; ", val(a)));
                    }
                    line.push_str(&format!("lzk = {k}u; lzm = {m:#x}u;"));
                    self.line(d, line);
                    self.pend = pending;
                }
                Stmt::Unsupported { mnemonic, detail } => {
                    self.line(
                        d,
                        format!(
                            "{{ {}H2_GATE_DROP(); uint32_t e_ = h2shim_v1_unsupported(ctx, {pc:#x}u, {}, {}); H2_STOP_EVENT({pc:#x}u, e_); }}",
                            self.restore(),
                            c_string(mnemonic),
                            c_string(detail)
                        ),
                    );
                }
            }
        }
    }

    /// The homed locals a direct call or tail jump at `pc` must spill.
    fn dirty_at(&self, pc: u32) -> u32 {
        self.dirty.get(&pc).copied().unwrap_or(self.home)
    }

    /// Whether `t` is a function entry of the module, so its handler may be
    /// invoked directly; records it for a prototype.
    fn direct_target(&mut self, t: u32) -> bool {
        let yes = self.shape.functions.is_some_and(|f| f.contains(&t));
        if yes {
            self.direct.insert(t);
        }
        yes
    }

    /// `goto` a local block, or leave through an entry-routed jump so the
    /// target's dispatch (and any override) applies.
    fn go(d: Dest) -> String {
        if d.tail {
            format!("H2_XFER(H2_JUMP, H2_ENTRY, {:#x}u, 0);", d.pc)
        } else {
            format!("goto L_{:08x};", d.pc)
        }
    }

    fn target(t: &Target) -> String {
        match t {
            Target::Direct(a) => format!("{a:#x}u"),
            Target::Dynamic(v) => val(v),
        }
    }

    fn control(&mut self, insn: &IrInsn, last: bool) {
        let pc = insn.pc;
        let d = 2;
        match &insn.control {
            Control::Next => {
                if last {
                    self.line(d, Self::go(Dest::local(insn.next_pc())));
                }
            }
            Control::Jump { target: Target::Direct(t), tail: true, .. } if self.direct_target(*t) => {
                self.settle(lazy::ALL, d);
                let spill = if self.x87 { "H2X87_SPILL()" } else { "(void)0" };
                let dirty = self.dirty_at(pc);
                self.line(d, format!("H2_DJUMP({}, {t:#x}u, {spill}, {dirty:#x}u);", symbol(*t)));
            }
            Control::Jump { target: Target::Direct(t), tail, .. } => {
                self.line(d, Self::go(Dest { pc: *t, tail: *tail }));
            }
            Control::Jump { target: Target::Dynamic(v), .. } if self.shape.functions.is_some() => {
                self.settle(lazy::ALL, d);
                let spill = if self.x87 { "H2X87_SPILL()" } else { "(void)0" };
                let dirty = self.dirty_at(pc);
                self.line(d, format!("H2_IJUMP({}, {spill}, {dirty:#x}u);", val(v)));
            }
            Control::Jump { target, .. } => {
                self.line(d, format!("H2_XFER(H2_JUMP, H2_ENTRY, {}, 0);", Self::target(target)))
            }
            Control::Branch { cond, taken, fallthrough } => {
                self.line(d, format!("if ({}) {}", val(cond), Self::go(*taken)));
                self.line(d, Self::go(*fallthrough));
            }
            Control::Switch { target, cases, .. } => {
                // The real guest load decides; only validated cases match.
                self.line(d, format!("switch ({}) {{", val(target)));
                let mut seen = BTreeSet::new();
                for c in cases.iter().filter(|c| seen.insert(c.pc)) {
                    self.line(d, format!("case {:#x}u: {}", c.pc, Self::go(*c)));
                }
                self.line(d, format!("default: H2_HALT({pc:#x}u, H2SHIM_UNKNOWN_TARGET, {});", val(target)));
                self.line(d, "}");
            }
            Control::Call { target: Target::Direct(t), continuation, .. }
                if self.direct_target(*t) && self.shape.selectors.contains(continuation) =>
            {
                // The continuation is this handler's own selector, so a
                // return there resumes this handler with that start: going
                // on from its label is the same thing.
                self.settle(lazy::ALL, d);
                let label = if self.shape.loops.headers.contains(continuation) { "P" } else { "L" };
                let (spill, reload) =
                    if self.x87 { ("H2X87_SPILL()", "H2X87_RELOAD()") } else { ("(void)0", "(void)0") };
                let refresh = if self.polled { "H2_ACC_REFRESH()" } else { "(void)0" };
                let dirty = self.dirty_at(pc);
                self.line(
                    d,
                    format!(
                        "H2_DCALL({}, {t:#x}u, {continuation:#x}u, {label}_{continuation:08x}, {spill}, {reload}, {refresh}, {dirty:#x}u);",
                        symbol(*t)
                    ),
                );
            }
            // A call through a pointer, back to this handler: H2_ICALL finds
            // the target's handler in the direct slots the runner filled
            // and calls it as H2_DCALL would, or leaves for the runner as
            // below.
            Control::Call { target: Target::Dynamic(v), continuation, kind: CallKind::Indirect, .. }
                if self.shape.functions.is_some() && self.shape.selectors.contains(continuation) =>
            {
                self.settle(lazy::ALL, d);
                let label = if self.shape.loops.headers.contains(continuation) { "P" } else { "L" };
                let (spill, reload) =
                    if self.x87 { ("H2X87_SPILL()", "H2X87_RELOAD()") } else { ("(void)0", "(void)0") };
                let refresh = if self.polled { "H2_ACC_REFRESH()" } else { "(void)0" };
                let dirty = self.dirty_at(pc);
                self.line(
                    d,
                    format!(
                        "H2_ICALL({}, {continuation:#x}u, {label}_{continuation:08x}, {spill}, {reload}, {refresh}, {dirty:#x}u);",
                        val(v)
                    ),
                );
            }
            Control::Call { target, continuation, .. } => self.line(
                d,
                format!("H2_XFER(H2_CALL, H2_ENTRY, {}, {continuation:#x}u);", Self::target(target)),
            ),
            Control::Return { target, .. } => {
                self.line(d, format!("H2_XFER(H2_RETURN, H2_RETURN_TARGET, {}, 0);", val(target)))
            }
            Control::Fault(f) => self.line(d, raise(*f, insn)),
            // The Unsupported statement has already stopped.
            Control::Stop(StopReason::Unsupported) => {}
            Control::Stop(r) => self.line(d, format!("H2_HALT({pc:#x}u, {}, 0u);", stop_code(*r))),
        }
    }
}
