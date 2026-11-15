//! x87 floating point: every instruction as [`ir::x87::X87Op`]s, one per
//! `h2x87_v1_*` helper call (docs/FLOAT-ABI.md §3).
//!
//! The shapes (see the `ir::x87` docs for the outcome rules):
//!
//! - **Memory sources** (`fld m`, `fadd m`, `fcom m`, `fldcw`, `fldenv`):
//!   `X87Wait` first, so a pending #MF wins over a page fault, then the
//!   ordinary guest `Load`, then the op with the bits by value.
//! - **Stores** (`fst m`, `fistp`, `fbstp`): `StoreConvert` (no state
//!   change), the guest `Store` only if the conversion said to write, then
//!   `StoreCommit`, which pops. A faulting write leaves the x87 state as it
//!   was.
//! - **`fcomi` forms**: the compare, then ZF/PF/CF from its payload (kept
//!   as they were when the compare was suppressed by an unmasked
//!   exception), OF/SF/AF cleared, all six defined.
//! - **Non-control instructions** end with `Mark`: FIP/FOP (and FDP for a
//!   memory operand) now describe this instruction. Control instructions
//!   (`fldcw`, `fnstcw`, `fnstsw`, `fnclex`, `fninit`, `fnstenv`, `fldenv`,
//!   `fnsave`, `frstor`, `fwait`) leave them alone.
//!
//! In 32-bit code the decoder splits `9B`-prefixed forms (`fstsw`, `finit`,
//! ...) into `wait` plus the non-waiting form, which is exactly the
//! hardware's behaviour; the waiting mnemonics are still accepted, with an
//! `X87Wait` in front, in case a decoder hands one over.
//!
//! `fnstsw ax; test ah, M; jcc` and `fnstsw ax; sahf; jcc` need no fusion:
//! the compare helper leaves C3/C2/C0 in the status word on OK and on
//! SUPPRESS alike, `fnstsw` is a field read, and the `test`/`sahf` that
//! follow are ordinary integer instructions over AX.
//!
//! `fneni`, `fndisi` and `fnsetpm` are no-ops on a P6; `fnop` is a waiting
//! no-op. `fisttp` is SSE3, not P6: #UD. FS-relative x87 memory operands
//! (none in the retail image) are refused: FDS would need the guest's FS
//! selector.

use iced_x86::MemorySize;
use ir::x87::{X87Arith, X87CmpKind, X87Const, X87Env, X87Format, X87Op, X87Src, X87Unary};

use super::*;

/// Whether `m` is lowered here.
pub(super) fn is_x87(m: Mnemonic) -> bool {
    use Mnemonic as M;
    matches!(
        m,
        M::Wait
            | M::Fld
            | M::Fild
            | M::Fbld
            | M::Fld1
            | M::Fldl2t
            | M::Fldl2e
            | M::Fldpi
            | M::Fldlg2
            | M::Fldln2
            | M::Fldz
            | M::Fst
            | M::Fstp
            | M::Fstpnce
            | M::Fist
            | M::Fistp
            | M::Fisttp
            | M::Fbstp
            | M::Fadd
            | M::Faddp
            | M::Fiadd
            | M::Fsub
            | M::Fsubp
            | M::Fisub
            | M::Fsubr
            | M::Fsubrp
            | M::Fisubr
            | M::Fmul
            | M::Fmulp
            | M::Fimul
            | M::Fdiv
            | M::Fdivp
            | M::Fidiv
            | M::Fdivr
            | M::Fdivrp
            | M::Fidivr
            | M::Fsqrt
            | M::Fabs
            | M::Fchs
            | M::Frndint
            | M::Fsin
            | M::Fcos
            | M::Fptan
            | M::Fpatan
            | M::Fyl2x
            | M::Fyl2xp1
            | M::F2xm1
            | M::Fscale
            | M::Fxtract
            | M::Fsincos
            | M::Fprem
            | M::Fprem1
            | M::Fcom
            | M::Fcomp
            | M::Fcompp
            | M::Fucom
            | M::Fucomp
            | M::Fucompp
            | M::Ficom
            | M::Ficomp
            | M::Fcomi
            | M::Fcomip
            | M::Fucomi
            | M::Fucomip
            | M::Ftst
            | M::Fxam
            | M::Fxch
            | M::Ffree
            | M::Ffreep
            | M::Fincstp
            | M::Fdecstp
            | M::Fcmovb
            | M::Fcmove
            | M::Fcmovbe
            | M::Fcmovu
            | M::Fcmovnb
            | M::Fcmovne
            | M::Fcmovnbe
            | M::Fcmovnu
            | M::Fldcw
            | M::Fnstcw
            | M::Fstcw
            | M::Fnstsw
            | M::Fstsw
            | M::Fnclex
            | M::Fclex
            | M::Fninit
            | M::Finit
            | M::Fnstenv
            | M::Fstenv
            | M::Fldenv
            | M::Fnsave
            | M::Fsave
            | M::Frstor
            | M::Fnop
            | M::Fneni
            | M::Feni
            | M::Fndisi
            | M::Fdisi
            | M::Fnsetpm
            | M::Fsetpm
    )
}

/// FOP from the encoding itself: past the legacy prefixes, the low three
/// bits of the `D8`-`DF` opcode byte, then the ModR/M byte, exactly as
/// encoded (a redundant SIB byte or an ignored prefix changes nothing).
fn fop_of_bytes(b: &[u8]) -> Option<u16> {
    let i = b
        .iter()
        .position(|x| !matches!(x, 0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67 | 0xf0 | 0xf2 | 0xf3))?;
    let (op, modrm) = (*b.get(i)?, *b.get(i + 1)?);
    (op & 0xf8 == 0xd8).then_some(((op as u16 & 7) << 8) | modrm as u16)
}

/// FOP rebuilt from the decoded instruction when its bytes are not at
/// hand. Register forms come straight from the opcode (`D8 C0+i`); memory
/// forms rebuild ModR/M from the addressing mode, which is exact only for
/// the canonical encoding: when the length says there is more (a redundant
/// SIB byte, an ignored prefix), which of them cannot be told, and `None`.
fn fop_of_decode(insn: &Instruction) -> Option<u16> {
    let oc = insn.op_code();
    let v = oc.op_code();
    if v > 0xff {
        let i = (0..insn.op_count())
            .filter(|&k| insn.op_kind(k) == OpKind::Register)
            .map(|k| insn.op_register(k))
            .filter(|r| (Register::ST0..=Register::ST7).contains(r))
            .map(|r| r as u32 - Register::ST0 as u32)
            .max()
            .unwrap_or(0);
        let plus = if oc.op_code_string().ends_with("+i") { i } else { 0 };
        return Some((((v >> 8) & 7) << 8 | ((v & 0xff) + plus)) as u16);
    }
    if !(0..insn.op_count()).any(|k| insn.op_kind(k) == OpKind::Memory) {
        return None;
    }
    let base = insn.memory_base();
    let index = insn.memory_index();
    let num = |r: Register| (r as u32 - Register::EAX as u32) & 7;
    let (md, rm, disp) = if base == Register::None && index == Register::None {
        (0, 5, 4)
    } else if base == Register::None {
        (0, 4, 4) // SIB with no base: disp32
    } else {
        let rm = if index != Register::None || base == Register::ESP { 4 } else { num(base) };
        match insn.memory_displ_size() {
            0 => (0, rm, 0),
            1 => (1, rm, 1),
            _ => (2, rm, 4),
        }
    };
    let prefixes = (insn.segment_prefix() != Register::None) as u32;
    let canonical = prefixes + 2 + (rm == 4) as u32 + disp;
    let reg = oc.group_index().max(0) as u32;
    (insn.len() as u32 == canonical).then_some((((v & 7) << 8) | (md << 6) | (reg << 3) | rm) as u16)
}

/// FOP for `insn`: from its encoding `bytes` when the lowering has them
/// ([`super::lower_encoded`]), else rebuilt from the decode. `None` when
/// it cannot be known exactly, which the lowering refuses.
pub fn fop(insn: &Instruction, bytes: Option<&[u8]>) -> Option<u16> {
    match bytes {
        Some(b) => fop_of_bytes(&b[..b.len().min(insn.len())]),
        None => fop_of_decode(insn),
    }
}

impl Lower<'_> {
    fn x87_op(&mut self, op: X87Op, args: Vec<Value>) -> Vec<Value> {
        self.intrinsic(Intrinsic::X87(op), args)
    }

    /// `fwait`'s check: #MF if an unmasked exception is pending.
    fn wait(&mut self) {
        self.intrinsic(Intrinsic::X87Wait, vec![]);
    }

    /// Operand `i` as a stack index.
    fn sti(&self, i: u32) -> R<u8> {
        let r = self.insn.op_register(i);
        if self.insn.op_kind(i) == OpKind::Register && (Register::ST0..=Register::ST7).contains(&r) {
            Ok(r as u8 - Register::ST0 as u8)
        } else {
            Err(format!("operand {i} is not st(0)-st(7)"))
        }
    }

    /// The memory operand's offset; flat only.
    fn x87_ea(&mut self) -> R<Addr> {
        let a = self.ea()?;
        if a.seg != Segment::Flat {
            return Err("FS-relative x87 memory operand".into());
        }
        Ok(a)
    }

    /// The memory operand: `Wait` (a pending #MF beats a page fault), then
    /// the read. Returns the bits and the address (for FDP).
    fn x87_read(&mut self, w: Width) -> R<(Value, Addr)> {
        let a = self.x87_ea()?;
        self.wait();
        Ok((self.b.load(a, w), a))
    }

    fn mark(&mut self, a: Option<Addr>) {
        // Checked on entry to `x87`.
        let fop = fop(self.insn, self.bytes).unwrap_or(0);
        match a {
            Some(a) => self.x87_op(X87Op::Mark { fop, mem: true }, vec![a.offset]),
            None => self.x87_op(X87Op::Mark { fop, mem: false }, vec![]),
        };
    }

    fn mem_format(&self) -> R<X87Format> {
        Ok(match self.insn.memory_size() {
            MemorySize::Float32 => X87Format::F32,
            MemorySize::Float64 => X87Format::F64,
            MemorySize::Float80 => X87Format::F80,
            MemorySize::Int16 => X87Format::I16,
            MemorySize::Int32 => X87Format::I32,
            MemorySize::Int64 => X87Format::I64,
            MemorySize::Bcd => X87Format::Bcd,
            m => return Err(format!("x87 memory operand {m:?}")),
        })
    }

    /// The memory source of an arithmetic or compare.
    fn mem_src(&self) -> R<X87Src> {
        Ok(match self.mem_format()? {
            X87Format::F32 => X87Src::F32,
            X87Format::F64 => X87Src::F64,
            X87Format::I16 => X87Src::I16,
            X87Format::I32 => X87Src::I32,
            f => return Err(format!("x87 arithmetic source {f:?}")),
        })
    }

    pub(super) fn x87(&mut self) -> R {
        use Mnemonic as M;
        let insn = self.insn;
        if insn.mnemonic() != M::Wait && fop(insn, self.bytes).is_none() {
            return Err("x87 encoding whose FOP cannot be rebuilt from the decode (pass its bytes)".into());
        }
        match insn.mnemonic() {
            // ---- control: no FIP/FOP update ------------------------------
            M::Wait => {
                self.wait();
            }
            M::Fneni | M::Fndisi | M::Fnsetpm => {}
            M::Feni | M::Fdisi | M::Fsetpm => {
                self.wait();
            }
            M::Fninit | M::Finit => {
                if insn.mnemonic() == M::Finit {
                    self.wait();
                }
                self.x87_op(X87Op::Init, vec![]);
            }
            M::Fnclex | M::Fclex => {
                if insn.mnemonic() == M::Fclex {
                    self.wait();
                }
                self.x87_op(X87Op::Clex, vec![]);
            }
            M::Fnstcw | M::Fstcw => {
                if insn.mnemonic() == M::Fstcw {
                    self.wait();
                }
                let cw = self.intrinsic(Intrinsic::X87ReadControl, vec![]);
                self.write(0, cw[0])?;
            }
            M::Fnstsw | M::Fstsw => {
                if insn.mnemonic() == M::Fstsw {
                    self.wait();
                }
                let sw = self.intrinsic(Intrinsic::X87ReadStatus, vec![]);
                self.write(0, sw[0])?;
            }
            M::Fldcw => {
                let (v, _) = self.x87_read(Width::W16)?;
                self.x87_op(X87Op::LoadCw, vec![v]);
            }
            M::Fnstenv | M::Fstenv | M::Fnsave | M::Fsave => {
                let (env, size) = match insn.mnemonic() {
                    M::Fnstenv | M::Fstenv => (X87Env::StEnv, MemorySize::FpuEnv28),
                    _ => (X87Env::Save, MemorySize::FpuState108),
                };
                if insn.memory_size() != size {
                    return Err("16-bit environment layout".into());
                }
                let a = self.x87_ea()?;
                if matches!(insn.mnemonic(), M::Fstenv | M::Fsave) {
                    self.wait();
                }
                let chunks = self.x87_op(X87Op::SaveImage(env), vec![]);
                let mut off = 0;
                for v in chunks {
                    let at = self.b.add(a.offset, c32(off));
                    self.b.store(Addr { seg: a.seg, offset: at }, v);
                    off += v.width().bytes();
                }
                self.x87_op(X87Op::SaveCommit(env), vec![]);
            }
            M::Fldenv | M::Frstor => {
                let (env, size) = match insn.mnemonic() {
                    M::Fldenv => (X87Env::LdEnv, MemorySize::FpuEnv28),
                    _ => (X87Env::Rstor, MemorySize::FpuState108),
                };
                if insn.memory_size() != size {
                    return Err("16-bit environment layout".into());
                }
                let a = self.x87_ea()?;
                self.wait();
                let mut off = 0;
                let mut parts = Vec::new();
                for &w in env.chunks() {
                    let at = self.b.add(a.offset, c32(off));
                    parts.push(self.b.load(Addr { seg: a.seg, offset: at }, w));
                    off += w.bytes();
                }
                self.x87_op(X87Op::LoadEnv(env), parts);
            }

            // ---- loads ----------------------------------------------------
            M::Fld | M::Fild | M::Fbld => {
                if self.is_mem(0) {
                    let f = self.mem_format()?;
                    let (v, a) = self.x87_read(f.width())?;
                    self.x87_op(X87Op::Load(f), vec![v]);
                    self.mark(Some(a));
                } else {
                    let i = self.sti(0)?;
                    self.x87_op(X87Op::LoadReg(i), vec![]);
                    self.mark(None);
                }
            }
            M::Fld1 | M::Fldl2t | M::Fldl2e | M::Fldpi | M::Fldlg2 | M::Fldln2 | M::Fldz => {
                let c = match insn.mnemonic() {
                    M::Fld1 => X87Const::One,
                    M::Fldl2t => X87Const::L2T,
                    M::Fldl2e => X87Const::L2E,
                    M::Fldpi => X87Const::Pi,
                    M::Fldlg2 => X87Const::Lg2,
                    M::Fldln2 => X87Const::Ln2,
                    _ => X87Const::Z,
                };
                self.x87_op(X87Op::LoadConst(c), vec![]);
                self.mark(None);
            }

            // ---- stores ---------------------------------------------------
            M::Fst | M::Fstp | M::Fstpnce | M::Fist | M::Fistp | M::Fbstp => {
                let pop = !matches!(insn.mnemonic(), M::Fst | M::Fist);
                if self.is_mem(0) {
                    let to = self.mem_format()?;
                    self.store(to, pop)?;
                } else {
                    let dst = self.sti(0)?;
                    self.x87_op(X87Op::StoreReg { dst, pop }, vec![]);
                    self.mark(None);
                }
            }
            // SSE3, not on the guest's Pentium III.
            M::Fisttp => self.control = Control::Fault(Fault::InvalidOpcode),

            // ---- arithmetic -----------------------------------------------
            M::Fadd
            | M::Fsub
            | M::Fsubr
            | M::Fmul
            | M::Fdiv
            | M::Fdivr
            | M::Faddp
            | M::Fsubp
            | M::Fsubrp
            | M::Fmulp
            | M::Fdivp
            | M::Fdivrp
            | M::Fiadd
            | M::Fisub
            | M::Fisubr
            | M::Fimul
            | M::Fidiv
            | M::Fidivr => {
                let (op, pop) = match insn.mnemonic() {
                    M::Fadd | M::Fiadd => (X87Arith::Add, false),
                    M::Faddp => (X87Arith::Add, true),
                    M::Fsub | M::Fisub => (X87Arith::Sub, false),
                    M::Fsubp => (X87Arith::Sub, true),
                    M::Fsubr | M::Fisubr => (X87Arith::SubR, false),
                    M::Fsubrp => (X87Arith::SubR, true),
                    M::Fmul | M::Fimul => (X87Arith::Mul, false),
                    M::Fmulp => (X87Arith::Mul, true),
                    M::Fdiv | M::Fidiv => (X87Arith::Div, false),
                    M::Fdivp => (X87Arith::Div, true),
                    M::Fdivr | M::Fidivr => (X87Arith::DivR, false),
                    _ => (X87Arith::DivR, true),
                };
                if self.is_mem(0) {
                    let src = self.mem_src()?;
                    let (v, a) = self.x87_read(src.width().expect("memory source"))?;
                    self.x87_op(X87Op::Arith { op, dst: 0, src, pop }, vec![v]);
                    self.mark(Some(a));
                } else {
                    // `op st(0), st(i)` or `op st(i), st(0)`; the first
                    // operand is the destination.
                    let (d, s) = (self.sti(0)?, self.sti(1)?);
                    if d != 0 && s != 0 {
                        return Err("x87 arithmetic without st(0)".into());
                    }
                    self.x87_op(X87Op::Arith { op, dst: d, src: X87Src::St(s), pop }, vec![]);
                    self.mark(None);
                }
            }
            M::Fsqrt
            | M::Fabs
            | M::Fchs
            | M::Frndint
            | M::Fsin
            | M::Fcos
            | M::Fptan
            | M::Fpatan
            | M::Fyl2x
            | M::Fyl2xp1
            | M::F2xm1
            | M::Fscale
            | M::Fxtract
            | M::Fsincos
            | M::Fprem
            | M::Fprem1 => {
                let u = match insn.mnemonic() {
                    M::Fsqrt => X87Unary::Sqrt,
                    M::Fabs => X87Unary::Abs,
                    M::Fchs => X87Unary::Chs,
                    M::Frndint => X87Unary::RndInt,
                    M::Fsin => X87Unary::Sin,
                    M::Fcos => X87Unary::Cos,
                    M::Fptan => X87Unary::Tan,
                    M::Fpatan => X87Unary::Patan,
                    M::Fyl2x => X87Unary::Yl2x,
                    M::Fyl2xp1 => X87Unary::Yl2xp1,
                    M::F2xm1 => X87Unary::X2m1,
                    M::Fscale => X87Unary::Scale,
                    M::Fxtract => X87Unary::Xtract,
                    M::Fsincos => X87Unary::SinCos,
                    M::Fprem => X87Unary::Prem,
                    _ => X87Unary::Prem1,
                };
                self.x87_op(X87Op::Unary(u), vec![]);
                self.mark(None);
            }

            // ---- compares ---------------------------------------------------
            M::Fcom | M::Fcomp | M::Fcompp | M::Fucom | M::Fucomp | M::Fucompp | M::Ficom | M::Ficomp => {
                let (kind, pop) = match insn.mnemonic() {
                    M::Fcom => (X87CmpKind::Com, 0),
                    M::Fcomp => (X87CmpKind::Com, 1),
                    M::Fcompp => (X87CmpKind::Com, 2),
                    M::Fucom => (X87CmpKind::Ucom, 0),
                    M::Fucomp => (X87CmpKind::Ucom, 1),
                    M::Fucompp => (X87CmpKind::Ucom, 2),
                    M::Ficom => (X87CmpKind::Icom, 0),
                    _ => (X87CmpKind::Icom, 1),
                };
                if self.is_mem(0) {
                    let src = self.mem_src()?;
                    let (v, a) = self.x87_read(src.width().expect("memory source"))?;
                    self.x87_op(X87Op::Cmp { kind, src, pop, to_flags: false }, vec![v]);
                    self.mark(Some(a));
                } else {
                    // `fcompp`/`fucompp` name no operand: ST(1).
                    let s = if insn.op_count() >= 2 { self.sti(1)? } else { 1 };
                    self.x87_op(X87Op::Cmp { kind, src: X87Src::St(s), pop, to_flags: false }, vec![]);
                    self.mark(None);
                }
            }
            M::Ftst => {
                // `fcom` against +0.0 from binary32: the same operation.
                let src = X87Src::F32;
                self.x87_op(X87Op::Cmp { kind: X87CmpKind::Com, src, pop: 0, to_flags: false }, vec![c32(0)]);
                self.mark(None);
            }
            M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip => {
                let kind =
                    if matches!(insn.mnemonic(), M::Fcomi | M::Fcomip) { X87CmpKind::Com } else { X87CmpKind::Ucom };
                let pop = matches!(insn.mnemonic(), M::Fcomip | M::Fucomip) as u8;
                let s = self.sti(1)?;
                let r = self.x87_op(X87Op::Cmp { kind, src: X87Src::St(s), pop, to_flags: true }, vec![]);
                let (payload, suppressed) = (r[0], r[1]);
                // ZF/PF/CF from the payload, or as they were if the compare
                // was suppressed; OF/SF/AF cleared; all six defined.
                for (flag, bit) in [(Flag::Zf, 6), (Flag::Pf, 2), (Flag::Cf, 0)] {
                    let new = self.b.bit(payload, bit);
                    let old = self.b.read_flag(flag);
                    let v = self.b.select(suppressed, old, new);
                    self.b.set_flag(flag, v);
                }
                for flag in [Flag::Of, Flag::Sf, Flag::Af] {
                    self.b.set_flag(flag, Value::bool(false));
                }
                self.mark(None);
            }
            M::Fxam => {
                self.x87_op(X87Op::Xam, vec![]);
                self.mark(None);
            }

            // ---- stack ------------------------------------------------------
            M::Fxch => {
                let s = if insn.op_count() >= 2 { self.sti(1)? } else { 1 };
                self.x87_op(X87Op::Xch(s), vec![]);
                self.mark(None);
            }
            M::Ffree | M::Ffreep => {
                let reg = self.sti(0)?;
                self.x87_op(X87Op::Free { reg, pop: insn.mnemonic() == M::Ffreep }, vec![]);
                self.mark(None);
            }
            M::Fincstp => {
                self.x87_op(X87Op::IncStp, vec![]);
                self.mark(None);
            }
            M::Fdecstp => {
                self.x87_op(X87Op::DecStp, vec![]);
                self.mark(None);
            }
            M::Fcmovb | M::Fcmove | M::Fcmovbe | M::Fcmovu | M::Fcmovnb | M::Fcmovne | M::Fcmovnbe | M::Fcmovnu => {
                use iced_x86::ConditionCode as C;
                let cc = match insn.mnemonic() {
                    M::Fcmovb => C::b,
                    M::Fcmove => C::e,
                    M::Fcmovbe => C::be,
                    M::Fcmovu => C::p,
                    M::Fcmovnb => C::ae,
                    M::Fcmovne => C::ne,
                    M::Fcmovnbe => C::a,
                    _ => C::np,
                };
                let take = self.condition(cc)?;
                let s = self.sti(1)?;
                self.x87_op(X87Op::Cmov(s), vec![take]);
                self.mark(None);
            }
            // A waiting no-op that is not a control instruction.
            M::Fnop => {
                self.wait();
                self.mark(None);
            }
            m => return Err(format!("{m:?} is not an x87 instruction")),
        }
        Ok(())
    }

    /// `fst`/`fstp`/`fist`/`fistp`/`fbstp` to memory: convert, write only
    /// if the conversion says so, commit (and pop).
    fn store(&mut self, to: X87Format, pop: bool) -> R {
        let a = self.x87_ea()?;
        let r = self.x87_op(X87Op::StoreConvert { to, src: 0 }, vec![]);
        let (value, sw, write) = (r[0], r[1], r[2]);
        self.if_else(
            write,
            |l| {
                l.b.store(a, value);
                Ok(())
            },
            |_| Ok(()),
        )?;
        self.x87_op(X87Op::StoreCommit { pop }, vec![sw, write]);
        self.mark(Some(a));
        Ok(())
    }
}
