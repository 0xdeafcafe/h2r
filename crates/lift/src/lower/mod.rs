//! x86 (32-bit, integer core) to IR, one instruction at a time.
//!
//! Every function here follows the same pattern: read operands (loads
//! first, so they fault before anything changes), compute the result and
//! flags as pure values, write the destination, and queue flag writes (the
//! [`Builder`] puts them last). Anything outside what is implemented
//! returns `Err(detail)`, which [`lower`] turns into a single
//! [`Stmt::Unsupported`]: the instruction is never half-lowered and never
//! silently dropped.
//!
//! Flag formulas follow the Intel SDM vol. 2 pseudo-code. Where the SDM
//! calls a flag undefined, the IR says [`ir::FlagValue::Undefined`]; where
//! that depends on a run-time count (OF after a shift by CL), it is decided
//! at run time with an [`ir::Stmt::If`].
//!
//! This module holds the entry point, operand access and the shared flag
//! helpers; the instruction families are in `integer`, `string`, `sse`,
//! `mmx`, `x87` and `control`.

use iced_x86::{Code, ConditionCode, Instruction, Mnemonic, OpKind, Register};
use ir::{
    Addr, Atomic, AtomicOp, BinOp, CallKind, CmpOp, Control, Dest, Fault, Flag, FlagValue, Gpr, Intrinsic, IrInsn,
    RepKind, Segment,
    Slice, Stmt, StopReason, SwitchShape, Target, Temp, UnOp, Value, Width,
};

use crate::builder::Builder;

mod control;
mod integer;
mod mmx;
mod sse;
mod string;
mod system;
mod x87;

pub use system::XBOX_CPUID;

type R<T = ()> = Result<T, String>;

/// What the CFG says about the instruction being lowered. Only transfers
/// care; everything else is [`Site::Plain`].
#[derive(Debug, Clone, Copy, Default)]
pub enum Site<'a> {
    /// No CFG information: direct `jmp` is local, indirect `jmp` is a tail
    /// transfer, calls are assumed to return.
    #[default]
    Plain,
    /// A `jmp` the CFG resolved to a label inside the function.
    LocalJump,
    /// A `jmp` the CFG resolved as leaving the function.
    Tail,
    /// A table jump with its validated targets.
    Switch(&'a analyze::JumpTable),
    /// A call, and whether the analysis thinks its continuation is reached.
    Call { returns: bool },
}

/// Image-wide facts lowering needs.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// Kernel import slot address to ordinal.
    pub kernel_slots: std::collections::HashMap<u32, u32>,
    /// `[start, end)` of every read-only section, for the jump-table
    /// immutability fact.
    pub readonly: Vec<(u32, u32)>,
}

impl Env {
    /// The facts for an image.
    pub fn of(xbe: &xbe::Xbe) -> Result<Env, xbe::Error> {
        let kernel_slots = xbe.kernel_imports()?.into_iter().map(|k| (k.slot_va, k.ordinal)).collect();
        let readonly = xbe
            .sections
            .iter()
            .filter(|s| !s.flags.writable())
            .map(|s| (s.va, s.va.wrapping_add(s.vsize)))
            .collect();
        Ok(Env { kernel_slots, readonly })
    }

    /// Whether `[start, start + len)` lies inside one read-only section.
    pub fn is_readonly(&self, start: u32, len: u32) -> bool {
        let end = start as u64 + len as u64;
        self.readonly.iter().any(|&(a, b)| start >= a && end <= b as u64)
    }
}

/// Lower one decoded instruction.
pub fn lower(insn: &Instruction, env: &Env, site: Site<'_>) -> IrInsn {
    let mut l = Lower {
        insn,
        env,
        site,
        b: Builder::new(),
        ea: None,
        control: Control::Next,
        atomic: insn.has_lock_prefix(),
        atomic_operand: None,
        atomic_old: None,
    };
    match l.run().and_then(|()| l.atomic_descriptor()) {
        Ok(atomic) => IrInsn {
            pc: insn.ip32(),
            len: insn.len() as u8,
            atomic,
            stmts: mmx_waits_first(l.b.finish()),
            control: l.control,
        },
        Err(detail) => IrInsn {
            pc: insn.ip32(),
            len: insn.len() as u8,
            atomic: None,
            stmts: vec![Stmt::Unsupported { mnemonic: mnemonic_name(insn), detail: categorise(insn, detail) }],
            control: Control::Stop(StopReason::Unsupported),
        },
    }
}

/// An instruction that makes the MMX transition first checks for a pending
/// x87 exception (#MF), before any of its reads, writes or faults, so the
/// [`Intrinsic::X87Wait`] goes in front; [`Intrinsic::MmxEnter`] itself
/// stays last.
fn mmx_waits_first(mut stmts: Vec<Stmt>) -> Vec<Stmt> {
    let is_mmx = |s: &Stmt| matches!(s, Stmt::Intrinsic { op: Intrinsic::MmxEnter, .. });
    if stmts.iter().any(is_mmx) {
        stmts.insert(0, Stmt::Intrinsic { op: Intrinsic::X87Wait, args: vec![], dsts: vec![] });
    }
    stmts
}

/// Prefix an unsupported detail with the ISA extension the instruction
/// belongs to, so coverage reports separate "x87 not started" from gaps in
/// the integer core.
fn categorise(insn: &Instruction, detail: String) -> String {
    use iced_x86::CpuidFeature as F;
    let features = insn.cpuid_features();
    // Every x87 mnemonic starts with F, including the P6 additions
    // (fcomi, fcmov) whose CPUID feature is CMOV rather than FPU; `wait`
    // is the x87 synchronisation instruction whatever its CPUID bits say.
    let x87 = features.contains(&F::FPU)
        || format!("{:?}", insn.mnemonic()).starts_with('F')
        || insn.mnemonic() == Mnemonic::Wait;
    let ext = if x87 {
        "x87"
    } else if features.iter().any(|f| matches!(f, F::SSE | F::SSE2 | F::SSE3)) {
        "sse"
    } else if features.iter().any(|f| matches!(f, F::MMX)) {
        "mmx"
    } else {
        "integer"
    };
    format!("{ext}: {detail}")
}

/// The decoder's name for an instruction, lower case (`"movsd"`).
pub fn mnemonic_name(insn: &Instruction) -> String {
    format!("{:?}", insn.mnemonic()).to_ascii_lowercase()
}

/// Map a decoder register to an IR register slice.
pub fn gpr(r: Register) -> Option<(Gpr, Slice)> {
    use Register::*;
    let g = |i: usize| Gpr::ALL[i];
    Some(match r {
        AL | CL | DL | BL => (g(r as usize - AL as usize), Slice::Low8),
        AH | CH | DH | BH => (g(r as usize - AH as usize), Slice::High8),
        AX | CX | DX | BX | SP | BP | SI | DI => (g(r as usize - AX as usize), Slice::Low16),
        EAX | ECX | EDX | EBX | ESP | EBP | ESI | EDI => (g(r as usize - EAX as usize), Slice::Full),
        _ => return Option::None,
    })
}

/// Integer ALU operations sharing the read-modify-write shape.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Alu {
    Add,
    Adc,
    Sub,
    Sbb,
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shift {
    Shl,
    Shr,
    Sar,
    Rol,
    Ror,
    Rcl,
    Rcr,
}

struct Lower<'a> {
    insn: &'a Instruction,
    env: &'a Env,
    site: Site<'a>,
    b: Builder,
    /// The memory operand's address, computed once per instruction.
    ea: Option<Addr>,
    control: Control,
    /// LOCK prefix, or `xchg` with memory.
    atomic: bool,
    /// The value operand and old-value register of an atomic form, as the
    /// lowering met them (read before the memory operand).
    atomic_operand: Option<Value>,
    atomic_old: Option<(Gpr, Slice)>,
}

const fn c32(v: u32) -> Value {
    Value::u32(v)
}

fn k(w: Width, v: u128) -> Value {
    Value::constant(w, v)
}

impl Lower<'_> {
    /// The [`Atomic`] descriptor of a locked instruction (see the `ir`
    /// crate docs), or why it can't have one.
    fn atomic_descriptor(&self) -> R<Option<Atomic>> {
        if !self.atomic {
            return Ok(None);
        }
        use Mnemonic as M;
        let op = match self.insn.mnemonic() {
            M::Add => AtomicOp::Add,
            M::Sub => AtomicOp::Sub,
            M::And => AtomicOp::And,
            M::Or => AtomicOp::Or,
            M::Xor => AtomicOp::Xor,
            M::Inc => AtomicOp::Inc,
            M::Dec => AtomicOp::Dec,
            M::Neg => AtomicOp::Neg,
            M::Not => AtomicOp::Not,
            M::Bts => AtomicOp::Bts,
            M::Btr => AtomicOp::Btr,
            M::Btc => AtomicOp::Btc,
            M::Xadd => AtomicOp::Xadd,
            M::Xchg => AtomicOp::Xchg,
            M::Cmpxchg => AtomicOp::Cmpxchg,
            // Absent from the retail image. The runtime's atomic ADC/SBB
            // also undefine every status flag when CF is undefined, which
            // the IR has no way to say, so the two would disagree.
            M::Adc | M::Sbb => return Err("lock adc/sbb".into()),
            _ => return Err("LOCK on an instruction that does not take it (#UD)".into()),
        };
        if !self.is_mem(0) {
            return Err("LOCK with a register destination (#UD)".into());
        }
        let operand = if op.has_operand() { self.atomic_operand } else { None };
        Ok(Some(Atomic { op, operand, old: self.atomic_old }))
    }

    fn run(&mut self) -> R {
        let insn = self.insn;
        use Mnemonic as M;
        match insn.mnemonic() {
            M::Nop | M::Pause => {}
            // x87 floating point, through Intrinsic::X87 (and `wait`).
            m if x87::is_x87(m) => self.x87()?,
            M::Stmxcsr => {
                let v = self.intrinsic(Intrinsic::MxcsrRead, vec![]);
                self.write(0, v[0])?;
            }
            M::Ldmxcsr => {
                let v = self.read(0)?;
                self.intrinsic(Intrinsic::MxcsrWrite, vec![v]);
            }
            M::Mov => self.mov()?,
            M::Movzx => self.extend(false)?,
            M::Movsx => self.extend(true)?,
            M::Lea => self.lea()?,
            M::Xchg => self.xchg()?,
            M::Bswap => {
                let v = self.read(0)?;
                let r = self.b.un(UnOp::Bswap, v);
                self.write(0, r)?;
            }
            M::Push => self.push_insn()?,
            M::Pop => self.pop_insn()?,
            M::Pushad => self.pushad()?,
            M::Popad => self.popad()?,
            M::Pushfd => self.pushfd()?,
            M::Popfd => self.popfd()?,
            M::Lahf => self.lahf(),
            M::Sahf => self.sahf(),
            M::Leave => self.leave()?,
            M::Enter => self.enter()?,
            M::Cbw | M::Cwde => self.widen_acc(),
            M::Cwd | M::Cdq => self.sign_into_dx(),
            M::Xlatb => self.xlat()?,

            M::Add => self.alu(Alu::Add, true)?,
            M::Adc => self.alu(Alu::Adc, true)?,
            M::Sub => self.alu(Alu::Sub, true)?,
            M::Sbb => self.alu(Alu::Sbb, true)?,
            M::And => self.alu(Alu::And, true)?,
            M::Or => self.alu(Alu::Or, true)?,
            M::Xor => self.alu(Alu::Xor, true)?,
            M::Cmp => self.alu(Alu::Sub, false)?,
            M::Test => self.alu(Alu::And, false)?,
            M::Inc | M::Dec => self.inc_dec(insn.mnemonic() == M::Inc)?,
            M::Neg => self.neg()?,
            M::Not => {
                let v = self.read(0)?;
                let r = self.b.not(v);
                self.write(0, r)?;
            }
            M::Mul => self.mul_wide(false)?,
            M::Imul => self.imul()?,
            M::Div => self.div(false)?,
            M::Idiv => self.div(true)?,

            M::Shl | M::Sal => self.shift(Shift::Shl)?,
            M::Shr => self.shift(Shift::Shr)?,
            M::Sar => self.shift(Shift::Sar)?,
            M::Rol => self.shift(Shift::Rol)?,
            M::Ror => self.shift(Shift::Ror)?,
            M::Rcl => self.shift(Shift::Rcl)?,
            M::Rcr => self.shift(Shift::Rcr)?,
            M::Shld => self.double_shift(true)?,
            M::Shrd => self.double_shift(false)?,
            M::Bt | M::Bts | M::Btr | M::Btc => self.bit_test()?,
            M::Bsf | M::Bsr => self.bit_scan(insn.mnemonic() == M::Bsf)?,

            M::Cmpxchg => self.cmpxchg()?,
            M::Xadd => self.xadd()?,

            M::Clc | M::Stc | M::Cmc | M::Cld | M::Std => self.flag_insn(),

            _ if is_string(insn.code()) => self.string()?,
            _ if insn.mnemonic() == M::Jmp => self.jmp()?,
            _ if is_jcc(insn) => {
                let cond = self.condition(insn.condition_code())?;
                self.branch(cond);
            }
            M::Jecxz | M::Jcxz => {
                let reg = if insn.mnemonic() == M::Jecxz { Slice::Full } else { Slice::Low16 };
                let c = self.b.read_reg(Gpr::Ecx, reg);
                let cond = self.b.is_zero(c);
                self.branch(cond);
            }
            M::Loop | M::Loope | M::Loopne => self.loop_insn()?,
            M::Call => self.call()?,
            M::Ret => self.ret()?,
            _ if is_setcc(insn) => {
                let c = self.condition(insn.condition_code())?;
                let v = self.b.zext(c, Width::W8);
                self.write(0, v)?;
            }
            _ if is_cmovcc(insn) => {
                // The source is read (and may fault) whatever the condition;
                // the destination is always written.
                let src = self.read(1)?;
                let old = self.read(0)?;
                let c = self.condition(insn.condition_code())?;
                let v = self.b.select(c, src, old);
                self.write(0, v)?;
            }

            M::Int3 => self.control = Control::Fault(Fault::Breakpoint),
            // `int 3` (CD 03) raises the same #BP, with the same saved EIP,
            // as the one-byte `int3`.
            M::Int if insn.immediate8() == 3 => self.control = Control::Fault(Fault::Breakpoint),
            M::Int => self.control = Control::Fault(Fault::Interrupt(insn.immediate8())),
            M::Ud2 => self.control = Control::Fault(Fault::InvalidOpcode),
            M::Hlt => self.control = Control::Stop(StopReason::Halt),

            M::Rdtsc => self.rdtsc(),
            M::Cpuid => self.cpuid(),
            // IF is not modelled: nothing interrupts generated code, and
            // `popfd` stops rather than change IF. Guest memory is coherent
            // and uncached as far as lifted code can tell.
            M::Cli | M::Sti | M::Wbinvd => {}

            // SSE data movement and bitwise ops: exact bit copies, so no
            // floating-point semantics (MXCSR, NaNs, denormals) are needed.
            M::Movss => self.movss()?,
            M::Movaps | M::Movups => self.movaps(insn.mnemonic() == M::Movaps)?,
            M::Movlps | M::Movhps => self.movlhps_mem(insn.mnemonic() == M::Movhps)?,
            M::Movhlps | M::Movlhps => self.movhl(insn.mnemonic() == M::Movhlps)?,
            M::Xorps | M::Andps | M::Andnps | M::Orps => self.sse_logic()?,
            M::Shufps | M::Unpcklps | M::Unpckhps => self.sse_shuffle()?,
            M::Movmskps => self.movmskps()?,
            M::Movntps => self.movntps()?,

            // SSE floating point, through Intrinsic::Sse.
            M::Addss | M::Addps | M::Subss | M::Subps | M::Mulss | M::Mulps | M::Divss | M::Divps | M::Sqrtss
            | M::Sqrtps | M::Minss | M::Minps | M::Maxss | M::Maxps | M::Rcpss | M::Rcpps | M::Rsqrtss
            | M::Rsqrtps | M::Cmpss | M::Cmpps | M::Cvtsi2ss | M::Cvtpi2ps => self.sse_arith()?,
            M::Comiss | M::Ucomiss => self.sse_comi()?,
            M::Cvtss2si | M::Cvttss2si | M::Cvtps2pi | M::Cvttps2pi => self.sse_to_int()?,
            // Cache hints and store ordering: no architectural effect on a
            // single processor (prefetch never faults).
            M::Prefetcht0 | M::Prefetcht1 | M::Prefetcht2 | M::Prefetchnta | M::Sfence => {}

            // MMX and the SSE integer forms on MMX registers.
            _ => self.mmx()?,
        }
        Ok(())
    }

    // ---- operands -------------------------------------------------------

    /// Width of operand `i`. Immediates take the width of operand 0, which
    /// is what every ALU form means by them.
    fn width(&self, i: u32) -> R<Width> {
        match self.insn.op_kind(i) {
            OpKind::Register => {
                gpr(self.insn.op_register(i)).map(|(_, s)| s.width()).ok_or_else(|| self.bad_reg(i))
            }
            OpKind::Memory => {
                let n = self.insn.memory_size().size() as u32;
                Width::from_bytes(n).ok_or_else(|| format!("{n}-byte memory operand"))
            }
            OpKind::Immediate8 => Ok(Width::W8),
            OpKind::Immediate16 | OpKind::Immediate8to16 => Ok(Width::W16),
            OpKind::Immediate32 | OpKind::Immediate8to32 => Ok(Width::W32),
            k => Err(format!("operand kind {k:?}")),
        }
    }

    fn bad_reg(&self, i: u32) -> String {
        format!("register operand {:?}", self.insn.op_register(i))
    }

    /// The memory operand's address. Rejects 16-bit addressing and GS.
    fn ea(&mut self) -> R<Addr> {
        if let Some(a) = self.ea {
            return Ok(a);
        }
        let insn = self.insn;
        let seg = match insn.memory_segment() {
            Register::FS => Segment::Fs,
            Register::GS => return Err("GS-relative access".into()),
            _ => Segment::Flat,
        };
        let mut off = c32(insn.memory_displacement32());
        let index = insn.memory_index();
        if index != Register::None {
            let (g, s) = gpr(index).filter(|(_, s)| *s == Slice::Full).ok_or("16-bit or odd addressing")?;
            let mut v = self.b.read_reg(g, s);
            let scale = insn.memory_index_scale();
            if scale > 1 {
                v = self.b.bin(BinOp::Shl, v, c32(scale.trailing_zeros()));
            }
            off = self.b.add(v, off);
        }
        let base = insn.memory_base();
        if base != Register::None {
            let (g, s) = gpr(base).filter(|(_, s)| *s == Slice::Full).ok_or("16-bit or odd addressing")?;
            let v = self.b.read_reg(g, s);
            off = self.b.add(v, off);
        }
        let a = Addr { seg, offset: off };
        self.ea = Some(a);
        Ok(a)
    }

    /// Read operand `i` at its own width.
    fn read(&mut self, i: u32) -> R<Value> {
        let w = self.width(i)?;
        self.read_as(i, w)
    }

    /// Read operand `i`; immediates are produced at `w`.
    fn read_as(&mut self, i: u32, w: Width) -> R<Value> {
        let insn = self.insn;
        match insn.op_kind(i) {
            OpKind::Register => {
                let (g, s) = gpr(insn.op_register(i)).ok_or_else(|| self.bad_reg(i))?;
                Ok(self.b.read_reg(g, s))
            }
            OpKind::Memory => {
                let a = self.ea()?;
                Ok(self.b.load(a, w))
            }
            OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32 => Ok(k(w, insn.immediate(i) as u128)),
            k => Err(format!("operand kind {k:?}")),
        }
    }

    fn write(&mut self, i: u32, v: Value) -> R {
        let insn = self.insn;
        match insn.op_kind(i) {
            OpKind::Register => {
                let (g, s) = gpr(insn.op_register(i)).ok_or_else(|| self.bad_reg(i))?;
                self.b.write_reg(g, s, v);
            }
            OpKind::Memory => {
                let a = self.ea()?;
                self.b.store(a, v);
            }
            k => return Err(format!("write to operand kind {k:?}")),
        }
        Ok(())
    }

    fn is_mem(&self, i: u32) -> bool {
        self.insn.op_kind(i) == OpKind::Memory
    }

    fn esp(&mut self) -> Value {
        self.b.read_reg(Gpr::Esp, Slice::Full)
    }

    /// Store `v` below ESP and move ESP down by its size. The store comes
    /// first so a faulting push leaves ESP alone.
    fn push_value(&mut self, v: Value) {
        let esp = self.esp();
        let new = self.b.sub(esp, c32(v.width().bytes()));
        self.b.store(Addr { seg: Segment::Flat, offset: new }, v);
        self.b.write_reg(Gpr::Esp, Slice::Full, new);
    }

    /// Run `f` against a fresh nested builder (temps continue this
    /// instruction's numbering) and return the statements it produced,
    /// its pending flag writes last. A memory address first computed inside
    /// is not reused outside, since its temps are scoped to the body.
    fn nested(&mut self, f: impl FnOnce(&mut Self) -> R) -> R<Vec<Stmt>> {
        let ea = self.ea;
        let inner = self.b.nested();
        let outer = std::mem::replace(&mut self.b, inner);
        let r = f(self);
        let inner = std::mem::replace(&mut self.b, outer);
        self.ea = ea;
        let body = self.b.adopt(inner);
        r.map(|()| body)
    }

    /// `if cond { then } else { otherwise }`, each side built by a closure.
    fn if_else(&mut self, cond: Value, then: impl FnOnce(&mut Self) -> R, otherwise: impl FnOnce(&mut Self) -> R) -> R {
        let then = self.nested(then)?;
        let otherwise = self.nested(otherwise)?;
        self.b.push(Stmt::If { cond, then, otherwise });
        Ok(())
    }

    fn intrinsic(&mut self, op: Intrinsic, args: Vec<Value>) -> Vec<Value> {
        let dsts: Vec<Temp> = op.signature().1.iter().map(|w| self.b.temp(*w)).collect();
        self.b.push(Stmt::Intrinsic { op, args, dsts: dsts.clone() });
        dsts.into_iter().map(Value::from).collect()
    }

    /// A conditional branch to the instruction's target. Both sides start
    /// local; the lifter marks the ones the CFG says leave the function.
    fn branch(&mut self, cond: Value) {
        let (taken, fallthrough) = (Dest::local(self.insn.near_branch32()), Dest::local(self.insn.next_ip32()));
        self.control = Control::Branch { cond, taken, fallthrough };
    }

    // ---- flags ----------------------------------------------------------

    /// Write `flag` as `v` where `defined` holds and as undefined
    /// elsewhere. `Err(b)` is a condition known now; `Ok(c)` one decided at
    /// run time, which becomes an `If` (written in place, so only use it
    /// after the instruction's faulting accesses).
    fn flag_where(&mut self, flag: Flag, defined: Result<Value, bool>, v: Value) {
        match defined {
            Err(true) => self.b.set_flag(flag, v),
            Err(false) => self.b.undef_flag(flag),
            Ok(cond) => self.b.push(Stmt::If {
                cond,
                then: vec![Stmt::WriteFlag { flag, value: FlagValue::Defined(v) }],
                otherwise: vec![Stmt::WriteFlag { flag, value: FlagValue::Undefined }],
            }),
        }
    }

    /// SF, ZF and PF from a result.
    fn szp(&mut self, res: Value) {
        let sf = self.b.msb(res);
        let zf = self.b.is_zero(res);
        let low = self.b.trunc(res, Width::W8);
        let pf = self.b.un(UnOp::Parity, low);
        self.b.set_flag(Flag::Sf, sf);
        self.b.set_flag(Flag::Zf, zf);
        self.b.set_flag(Flag::Pf, pf);
    }

    /// AF: carry or borrow out of bit 3, for add and subtract alike.
    fn af(&mut self, a: Value, x: Value, res: Value) {
        let t = self.b.xor(a, x);
        let t = self.b.xor(t, res);
        let af = self.b.bit(t, 4);
        self.b.set_flag(Flag::Af, af);
    }

    /// Flags of `res = a + x (+ carry in)`. The carry-out formula
    /// `(a & x) | ((a | x) & !res)` is exact with or without a carry in,
    /// so `adc` needs no wider arithmetic.
    fn flags_add(&mut self, a: Value, x: Value, res: Value, carry: bool) {
        if carry {
            let ax = self.b.and(a, x);
            let ox = self.b.or(a, x);
            let nr = self.b.not(res);
            let t = self.b.and(ox, nr);
            let t = self.b.or(ax, t);
            let cf = self.b.msb(t);
            self.b.set_flag(Flag::Cf, cf);
        }
        let t1 = self.b.xor(a, res);
        let t2 = self.b.xor(x, res);
        let t = self.b.and(t1, t2);
        let of = self.b.msb(t);
        self.b.set_flag(Flag::Of, of);
        self.af(a, x, res);
        self.szp(res);
    }

    /// Flags of `res = a - x (- borrow in)`. Borrow out of the top bit is
    /// `(!a & x) | ((!a | x) & res)`, again exact with a borrow in.
    fn flags_sub(&mut self, a: Value, x: Value, res: Value, carry: bool) {
        if carry {
            let na = self.b.not(a);
            let t1 = self.b.and(na, x);
            let t2 = self.b.or(na, x);
            let t2 = self.b.and(t2, res);
            let t = self.b.or(t1, t2);
            let cf = self.b.msb(t);
            self.b.set_flag(Flag::Cf, cf);
        }
        let t1 = self.b.xor(a, x);
        let t2 = self.b.xor(a, res);
        let t = self.b.and(t1, t2);
        let of = self.b.msb(t);
        self.b.set_flag(Flag::Of, of);
        self.af(a, x, res);
        self.szp(res);
    }

    /// Flags of a logical result: CF = OF = 0, AF undefined.
    fn flags_logic(&mut self, res: Value) {
        self.b.set_flag(Flag::Cf, Value::bool(false));
        self.b.set_flag(Flag::Of, Value::bool(false));
        self.b.undef_flag(Flag::Af);
        self.szp(res);
    }

    /// A `W1` condition from the flags, as `jcc`/`setcc`/`cmovcc` test it.
    fn condition(&mut self, cc: ConditionCode) -> R<Value> {
        use ConditionCode as C;
        let f = |s: &mut Self, flag| s.b.read_flag(flag);
        let (v, negate) = match cc {
            C::o | C::no => (f(self, Flag::Of), cc == C::no),
            C::b | C::ae => (f(self, Flag::Cf), cc == C::ae),
            C::e | C::ne => (f(self, Flag::Zf), cc == C::ne),
            C::be | C::a => {
                let c = f(self, Flag::Cf);
                let z = f(self, Flag::Zf);
                (self.b.or(c, z), cc == C::a)
            }
            C::s | C::ns => (f(self, Flag::Sf), cc == C::ns),
            C::p | C::np => (f(self, Flag::Pf), cc == C::np),
            C::l | C::ge => {
                let s = f(self, Flag::Sf);
                let o = f(self, Flag::Of);
                (self.b.xor(s, o), cc == C::ge)
            }
            C::le | C::g => {
                let s = f(self, Flag::Sf);
                let o = f(self, Flag::Of);
                let z = f(self, Flag::Zf);
                let lt = self.b.xor(s, o);
                (self.b.or(z, lt), cc == C::g)
            }
            C::None => return Err("no condition code".into()),
        };
        Ok(if negate { self.b.not(v) } else { v })
    }

}

/// EFLAGS bits `popfd` writes: CF PF AF ZF SF DF OF AC ID.
pub const POPFD_MASK: u32 = 0x0024_0CD5;
/// System flags `popfd` may not change (TF IF IOPL NT); see `popfd`.
pub const POPFD_SYSTEM: u32 = 0x0000_7300;

fn is_string(code: Code) -> bool {
    use Code::*;
    matches!(
        code,
        Movsb_m8_m8
            | Movsw_m16_m16
            | Movsd_m32_m32
            | Cmpsb_m8_m8
            | Cmpsw_m16_m16
            | Cmpsd_m32_m32
            | Stosb_m8_AL
            | Stosw_m16_AX
            | Stosd_m32_EAX
            | Lodsb_AL_m8
            | Lodsw_AX_m16
            | Lodsd_EAX_m32
            | Scasb_AL_m8
            | Scasw_AX_m16
            | Scasd_EAX_m32
    )
}

fn is_jcc(insn: &Instruction) -> bool {
    insn.is_jcc_short_or_near()
}

fn is_setcc(insn: &Instruction) -> bool {
    use Mnemonic::*;
    matches!(
        insn.mnemonic(),
        Seta | Setae | Setb | Setbe | Sete | Setg | Setge | Setl | Setle | Setne | Setno | Setnp | Setns | Seto | Setp | Sets
    )
}

fn is_cmovcc(insn: &Instruction) -> bool {
    use Mnemonic::*;
    matches!(
        insn.mnemonic(),
        Cmova | Cmovae | Cmovb | Cmovbe | Cmove | Cmovg | Cmovge | Cmovl | Cmovle | Cmovne | Cmovno | Cmovnp | Cmovns
            | Cmovo | Cmovp | Cmovs
    )
}
