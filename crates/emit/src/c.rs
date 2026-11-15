//! IR to C11, one owner function at a time.
//!
//! Shape of a generated handler (see `c/h2gen.h` for the macros):
//!
//! ```c
//! TransferAbi h2fn_002d21c6(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {
//!     H2_LOCALS();                  // registers and EFLAGS into locals
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
//! Registers and flags live in locals for the whole handler. Every way out
//! (transfer, fault, stop) sets the outgoing transfer and jumps to the one
//! shared epilogue, which spills them to `CpuAbi`, so state is canonical
//! whenever the runtime sees it. Memory goes through the ABI's checked
//! `h2abi_v1_read`/`write` every time; there is no RAM pointer. XMM and FP
//! state are read and written in `CpuAbi` directly. All arithmetic is
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

use std::collections::BTreeSet;


use ir::{
    Addr, Atomic, AtomicOp, BinOp, CastOp, CmpOp, Control, Dest, Expr, Fault, Flag, FlagValue, Gpr, Intrinsic,
    IrFunction, IrInsn, RepKind, Segment, Slice, Stmt, StopReason, Target, Temp, UnOp, Value, Width,
};

/// Everything the emitter needs to know about one owner's place in the
/// image, decided by [`crate::plan`].
pub struct OwnerShape<'a> {
    /// Start selectors the handler accepts: its entry, and the interior
    /// entries and call continuations it is the canonical owner of.
    pub selectors: &'a BTreeSet<u32>,
}

/// The C symbol of an owner's handler.
pub fn symbol(entry: u32) -> String {
    format!("h2fn_{entry:08x}")
}

/// Emit one owner as a C function definition.
pub fn function(f: &IrFunction, shape: &OwnerShape<'_>) -> String {
    let mut e = Emitter { out: String::new(), shape, in_repeat: false, x87: crate::x87::uses_x87(f) };
    e.function(f);
    e.out
}

struct Emitter<'a> {
    out: String,
    shape: &'a OwnerShape<'a>,
    /// Emitting a `Repeat` body: exits restore the saved EFLAGS first.
    in_repeat: bool,
    /// The handler has x87 ops: it has the x87 locals and fail block.
    x87: bool,
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
        self.line(0, format!("TransferAbi {name}(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {{"));
        self.line(1, "H2_LOCALS();");
        let x87 = self.x87;
        if x87 {
            self.line(1, "H2X87_LOCALS();");
        }
        self.line(1, "switch (start) {");
        for s in self.shape.selectors {
            self.line(1, format!("case {s:#010x}u: goto L_{s:08x};"));
        }
        self.line(1, "default: break;");
        self.line(1, "}");
        self.line(1, "{ uint32_t e_ = h2shim_v1_event(ctx, start, start, H2SHIM_BAD_ENTRY, start); H2_STOP_EVENT(start, e_); }");
        for b in f.blocks.values() {
            self.line(0, format!("L_{:08x}:;", b.start));
            for (i, insn) in b.insns.iter().enumerate() {
                let last = i + 1 == b.insns.len();
                // A selector inside a block (a REP restart point): every
                // local is live from H2_LOCALS, so entering here is sound.
                if i > 0 && self.shape.selectors.contains(&insn.pc) {
                    self.line(0, format!("L_{:08x}:;", insn.pc));
                }
                self.insn(insn, last);
            }
        }
        if x87 {
            self.line(1, "H2X87_FAIL();");
            self.line(1, "H2X87_EPILOGUE();");
        } else {
            self.line(1, "H2_EPILOGUE();");
        }
        self.line(0, "}");
    }

    fn insn(&mut self, insn: &IrInsn, last: bool) {
        let pc = insn.pc;
        self.line(1, format!("/* {pc:#010x} */ {{"));
        match &insn.atomic {
            Some(a) => self.atomic(insn, a),
            None => self.stmts(&insn.stmts, insn, 2),
        }
        self.control(insn, last);
        self.line(1, "}");
    }

    /// An atomic instruction (validated shape: pure prefix, one load, one
    /// store): the prefix, then the runtime's operation-wide accessor in
    /// place of the load..store statements.
    fn atomic(&mut self, insn: &IrInsn, a: &Atomic) {
        let pc = insn.pc;
        let li = insn.stmts.iter().position(|s| matches!(s, Stmt::Load { .. })).expect("validated atomic shape");
        let Stmt::Load { dst, addr } = &insn.stmts[li] else { unreachable!() };
        self.stmts(&insn.stmts[..li], insn, 2);
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

    fn stmts(&mut self, list: &[Stmt], insn: &IrInsn, d: usize) {
        let pc = insn.pc;
        for s in list {
            match s {
                Stmt::Assign { dst, expr } => {
                    let rhs = self.expr(*dst, expr);
                    self.line(d, format!("{} t{} = {rhs};", ty(dst.width), dst.id));
                }
                Stmt::ReadReg { dst, reg, slice } => {
                    let r = reg_name(*reg);
                    let rhs = match slice {
                        Slice::Full => r.to_string(),
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
                    self.line(d, format!("uint8_t t{} = H2_FLAG({});", dst.id, flag.eflags_bit()))
                }
                Stmt::WriteFlag { flag, value } => match value {
                    FlagValue::Defined(v) => self.line(d, format!("H2_SETF({}, {});", flag.eflags_bit(), val(v))),
                    FlagValue::Undefined => self.line(d, format!("H2_UNDEFF({});", flag.eflags_bit())),
                },
                Stmt::ReadEflags { dst } => self.line(d, format!("uint32_t t{} = fl | 2u;", dst.id)),
                Stmt::WriteEflags { value, mask } => {
                    self.line(d, format!("fl = (fl & ~{mask:#x}u) | ({} & {mask:#x}u) | 2u; fd |= {mask:#x}u;", val(value)))
                }
                Stmt::ReadXmm { dst, reg } => self.line(d, format!("h2u128 t{} = h2_rx(cpu, {reg});", dst.id)),
                Stmt::WriteXmm { reg, value } => self.line(d, format!("h2_wx(cpu, {reg}, {});", val(value))),
                Stmt::ReadMmx { dst, reg } => self.line(d, format!("uint64_t t{} = h2_rmm(cpu, {reg});", dst.id)),
                Stmt::WriteMmx { reg, value } => self.line(d, format!("h2_wmm(cpu, {reg}, {});", val(value))),
                Stmt::Load { dst, addr } => {
                    let a = self.addr(addr);
                    let w = dst.width;
                    let m = if self.in_repeat { "H2_LDR" } else { "H2_LD" };
                    self.line(d, format!("{m}({}, {}, t{}, {pc:#x}u, {a});", ty(w), w.bytes(), dst.id));
                }
                Stmt::Store { addr, value } => {
                    let a = self.addr(addr);
                    let w = value.width();
                    let m = if self.in_repeat { "H2_STR" } else { "H2_ST" };
                    self.line(d, format!("{m}({}, {}, {pc:#x}u, {a}, {});", ty(w), w.bytes(), val(value)));
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
                    self.stmts(then, insn, d + 1);
                    if !otherwise.is_empty() {
                        self.line(d, "} else {");
                        self.stmts(otherwise, insn, d + 1);
                    }
                    self.line(d, "}");
                }
                Stmt::Intrinsic { op: Intrinsic::ReadTsc, dsts, .. } => {
                    // The counter is the runtime's; it cannot fail.
                    self.line(d, format!("uint64_t t{} = h2shim_v1_rdtsc(ctx, {pc:#x}u);", dsts[0].id));
                }
                // In an x87 handler the wait shares the fail block: #MF
                // there is the same event 14 with the status word.
                Stmt::Intrinsic { op: Intrinsic::X87Wait, .. } if self.x87 && !self.in_repeat => {
                    self.line(d, format!("H2X87_WAIT({pc:#x}u);"));
                }
                Stmt::Intrinsic { op: Intrinsic::X87(op), args, dsts } => {
                    for l in crate::x87::stmt(*op, args, dsts, pc) {
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
                    let mut call = format!("h2i_{}(cpu", op.name());
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
                    self.line(d + 1, "{");
                    self.in_repeat = true;
                    self.stmts(body, insn, d + 2);
                    self.in_repeat = false;
                    self.line(d + 1, "}");
                    self.line(d + 1, "ecx -= 1u;");
                    let zf = Flag::Zf.eflags_bit();
                    match kind {
                        RepKind::Rep => {}
                        RepKind::RepE => self.line(d + 1, format!("if (!H2_FLAG({zf})) break;")),
                        RepKind::RepNe => self.line(d + 1, format!("if (H2_FLAG({zf})) break;")),
                    }
                    self.line(d, "}");
                }
                Stmt::Unsupported { mnemonic, detail } => {
                    self.line(
                        d,
                        format!(
                            "{{ {}uint32_t e_ = h2shim_v1_unsupported(ctx, {pc:#x}u, {}, {}); H2_STOP_EVENT({pc:#x}u, e_); }}",
                            self.restore(),
                            c_string(mnemonic),
                            c_string(detail)
                        ),
                    );
                }
            }
        }
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
            Control::Jump { target: Target::Direct(t), tail, .. } => {
                self.line(d, Self::go(Dest { pc: *t, tail: *tail }));
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
