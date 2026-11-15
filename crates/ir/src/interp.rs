//! Reference interpreter: the executable definition of the IR.
//!
//! It is deliberately slow and literal. Every [`Stmt`] is applied in order to
//! a [`Cpu`] and a [`Memory`]; there is no caching and no shortcut, so when
//! a backend disagrees with it, the interpreter is the one that says what
//! the IR meant. Fixture tests run lowered x86 through it and compare with
//! known hardware results; that checks the *lowering*, since this file is
//! only as right as the IR's documented semantics.
//!
//! The CPU here is intentionally minimal and owned by this module, not by
//! `crates/runtime`: the runtime's `CpuAbi` is a transport layout for
//! generated code, this is a model for checking semantics.

use std::collections::HashMap;

use crate::*;

/// Byte-addressed guest memory as the interpreter needs it. Addresses are
/// linear (segment base already applied). Implementations decide what is
/// mapped; an access that touches anything unmapped fails as a whole.
pub trait Memory {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault>;
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault>;
}

/// A failed memory access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemFault {
    pub addr: u32,
    pub write: bool,
}

/// Sparse byte memory for tests: every address written is mapped, as is
/// anything [`SparseMemory::map`] covers. Reading an unmapped byte faults.
#[derive(Debug, Default, Clone)]
pub struct SparseMemory {
    bytes: HashMap<u32, u8>,
}

impl SparseMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Map `len` zero bytes at `addr`.
    pub fn map(&mut self, addr: u32, len: u32) {
        for i in 0..len {
            self.bytes.entry(addr.wrapping_add(i)).or_insert(0);
        }
    }

    pub fn load(&mut self, addr: u32, data: &[u8]) {
        for (i, b) in data.iter().enumerate() {
            self.bytes.insert(addr.wrapping_add(i as u32), *b);
        }
    }

    pub fn read_u32(&self, addr: u32) -> Option<u32> {
        let mut v = [0u8; 4];
        for (i, b) in v.iter_mut().enumerate() {
            *b = *self.bytes.get(&addr.wrapping_add(i as u32))?;
        }
        Some(u32::from_le_bytes(v))
    }

    pub fn read_u8(&self, addr: u32) -> Option<u8> {
        self.bytes.get(&addr).copied()
    }
}

impl Memory for SparseMemory {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        for (i, b) in buf.iter_mut().enumerate() {
            let a = addr.wrapping_add(i as u32);
            *b = *self.bytes.get(&a).ok_or(MemFault { addr: a, write: false })?;
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        // Writes never fault: they map what they touch, which keeps fixtures
        // short. Use `FlatMemory` to test a faulting store.
        for (i, b) in data.iter().enumerate() {
            self.bytes.insert(addr.wrapping_add(i as u32), *b);
        }
        Ok(())
    }
}

/// One contiguous mapped range `[base, base + bytes.len())`; anything else
/// faults. Enough for running a loaded image with a stack inside the range.
#[derive(Debug, Clone)]
pub struct FlatMemory {
    pub base: u32,
    pub bytes: Vec<u8>,
}

impl FlatMemory {
    pub fn new(base: u32, len: usize) -> Self {
        FlatMemory { base, bytes: vec![0; len] }
    }

    fn span(&self, addr: u32, len: usize) -> Option<std::ops::Range<usize>> {
        let off = addr.checked_sub(self.base)? as usize;
        let end = off.checked_add(len)?;
        (end <= self.bytes.len()).then_some(off..end)
    }

    /// Copy `data` in at `addr`; `false` if it doesn't fit.
    pub fn load(&mut self, addr: u32, data: &[u8]) -> bool {
        match self.span(addr, data.len()) {
            Some(r) => {
                self.bytes[r].copy_from_slice(data);
                true
            }
            None => false,
        }
    }

    pub fn read_u32(&self, addr: u32) -> Option<u32> {
        self.span(addr, 4).map(|r| u32::from_le_bytes(self.bytes[r].try_into().unwrap()))
    }
}

impl Memory for FlatMemory {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let r = self.span(addr, buf.len()).ok_or(MemFault { addr, write: false })?;
        buf.copy_from_slice(&self.bytes[r]);
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let r = self.span(addr, data.len()).ok_or(MemFault { addr, write: true })?;
        self.bytes[r].copy_from_slice(data);
        Ok(())
    }
}

/// EFLAGS bit 1 is reserved and always reads as one.
pub const EFLAGS_RESERVED: u32 = 1 << 1;

/// x87 state, in the runtime `X87Abi`'s shape: registers in physical
/// order (TOP is status bits 11-13), the full 16-bit tag word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X87 {
    pub st: [[u8; 10]; 8],
    pub control: u16,
    pub status: u16,
    pub tag: u16,
    pub opcode: u16,
    pub instruction_pointer: u32,
    pub data_pointer: u32,
    pub instruction_selector: u16,
    pub data_selector: u16,
}

impl Default for X87 {
    /// The state after `fninit`.
    fn default() -> Self {
        X87 {
            st: [[0; 10]; 8],
            control: 0x037F,
            status: 0,
            tag: 0xFFFF,
            opcode: 0,
            instruction_pointer: 0,
            data_pointer: 0,
            instruction_selector: 0,
            data_selector: 0,
        }
    }
}

/// MXCSR at reset: all exceptions masked, round to nearest.
pub const MXCSR_DEFAULT: u32 = 0x1F80;
/// MXCSR bits the guest's Pentium III implements (its FXSAVE MXCSR_MASK):
/// everything in the low 16 bits except DAZ (bit 6), which that CPU lacks.
/// `ldmxcsr` with any other bit set raises `#GP` and changes nothing.
pub const MXCSR_MASK: u32 = 0x0000_FFBF;

/// The architectural state the IR touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpu {
    pub gpr: [u32; 8],
    pub eip: u32,
    /// Full EFLAGS. Undefined flags hold 0.
    pub eflags: u32,
    /// Bits of `eflags` that hold a defined value. A test comparing against
    /// hardware masks out the rest.
    pub flags_valid: u32,
    pub fs_base: u32,
    /// XMM0-7, little-endian lanes (lane 0 in the low 32 bits).
    pub xmm: [u128; 8],
    pub x87: X87,
    pub mxcsr: u32,
    /// The time-stamp counter [`Intrinsic::ReadTsc`] returns. Each read
    /// returns it and adds one, so it is deterministic and never repeats; a
    /// runtime derives the real one from guest time instead.
    pub tsc: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Cpu {
            gpr: [0; 8],
            eip: 0,
            eflags: EFLAGS_RESERVED,
            flags_valid: u32::MAX,
            fs_base: 0,
            xmm: [0; 8],
            x87: X87::default(),
            mxcsr: MXCSR_DEFAULT,
            tsc: 0,
        }
    }
}

/// Execute one [`Intrinsic`] on `cpu`'s FP state (or its TSC). The C
/// helpers in the emitter's support header implement exactly this, except
/// [`Intrinsic::ReadTsc`], which the runtime answers. [`Intrinsic::X87`] ops
/// need the instruction's PC and can stop as well as fault: [`step`] runs
/// them, and this function panics on one.
pub fn intrinsic(cpu: &mut Cpu, op: Intrinsic, args: &[u128]) -> Result<Vec<u128>, Fault> {
    use Intrinsic::*;
    Ok(match op {
        X87ReadControl => vec![cpu.x87.control as u128],
        X87ReadStatus => vec![cpu.x87.status as u128],
        X87(op) => panic!("interp::intrinsic({op:?}): x87 ops run through step"),
        MxcsrRead => vec![cpu.mxcsr as u128],
        MxcsrWrite => {
            let v = args[0] as u32;
            if v & !MXCSR_MASK != 0 {
                return Err(Fault::GeneralProtection);
            }
            cpu.mxcsr = v;
            vec![]
        }
        MmxEnter => {
            cpu.x87.status &= !0x3800;
            cpu.x87.tag = 0;
            vec![]
        }
        Emms => {
            x87_wait(cpu)?;
            cpu.x87.tag = 0xFFFF;
            vec![]
        }
        X87Wait => {
            x87_wait(cpu)?;
            vec![]
        }
        Sse(op) => return sse(cpu, op, args),
        ReadTsc => {
            let t = cpu.tsc;
            cpu.tsc = t.wrapping_add(1);
            vec![t as u128]
        }
    })
}

/// The pending-#MF check of [`Intrinsic::X87Wait`] and [`Intrinsic::Emms`]:
/// status word ES (bit 7).
fn x87_wait(cpu: &Cpu) -> Result<(), Fault> {
    if cpu.x87.status & 0x80 != 0 { Err(Fault::FpError) } else { Ok(()) }
}

/// [`Intrinsic::Sse`]: the reference semantics, argument shapes per
/// [`SseOp::signature`].
fn sse(cpu: &mut Cpu, op: SseOp, args: &[u128]) -> Result<Vec<u128>, Fault> {
    use simd as s;
    use SseOp::*;
    let m = &mut cpu.mxcsr;
    let a = args[0];
    let b32 = || args[1] as u32;
    let r: Result<u128, s::Xm> = match op {
        AddSs => s::addss(m, a, b32()),
        AddPs => s::addps(m, a, args[1]),
        SubSs => s::subss(m, a, b32()),
        SubPs => s::subps(m, a, args[1]),
        MulSs => s::mulss(m, a, b32()),
        MulPs => s::mulps(m, a, args[1]),
        DivSs => s::divss(m, a, b32()),
        DivPs => s::divps(m, a, args[1]),
        SqrtSs => s::sqrtss(m, a, b32()),
        SqrtPs => s::sqrtps(m, a),
        MinSs => s::minss(m, a, b32()),
        MinPs => s::minps(m, a, args[1]),
        MaxSs => s::maxss(m, a, b32()),
        MaxPs => s::maxps(m, a, args[1]),
        RcpSs => Ok(s::rcpss(a, b32())),
        RcpPs => Ok(s::rcpps(a)),
        RsqrtSs => Ok(s::rsqrtss(a, b32())),
        RsqrtPs => Ok(s::rsqrtps(a)),
        CmpSs => s::cmpss(m, a, b32(), args[2] as u8),
        CmpPs => s::cmpps(m, a, args[1], args[2] as u8),
        Comiss => s::comiss(m, a as u32, b32()).map(u128::from),
        Ucomiss => s::ucomiss(m, a as u32, b32()).map(u128::from),
        Cvtsi2ss => s::cvtsi2ss(m, a, b32()),
        Cvtss2si => s::cvtss2si(m, a as u32).map(u128::from),
        Cvttss2si => s::cvttss2si(m, a as u32).map(u128::from),
        Cvtpi2ps => s::cvtpi2ps(m, a, args[1] as u64),
        Cvtps2pi => s::cvtps2pi(m, a as u64).map(u128::from),
        Cvttps2pi => s::cvttps2pi(m, a as u64).map(u128::from),
    };
    r.map(|v| vec![v]).map_err(|_| Fault::SimdException)
}

impl Cpu {
    pub fn reg(&self, r: Gpr) -> u32 {
        self.gpr[r.index()]
    }

    pub fn set_reg(&mut self, r: Gpr, v: u32) {
        self.gpr[r.index()] = v;
    }

    pub fn flag(&self, f: Flag) -> bool {
        self.eflags & (1 << f.eflags_bit()) != 0
    }

    pub fn set_flag(&mut self, f: Flag, v: bool) {
        let bit = 1 << f.eflags_bit();
        self.eflags = if v { self.eflags | bit } else { self.eflags & !bit };
        self.flags_valid |= bit;
    }

    /// Whether the flag holds a defined value.
    pub fn flag_defined(&self, f: Flag) -> bool {
        self.flags_valid & (1 << f.eflags_bit()) != 0
    }
}

/// Where execution goes after an instruction that completed normally. The
/// CPU's `eip` has already been set to the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Next(u32),
    Jump { target: u32, tail: bool },
    Branch { target: u32, taken: bool },
    /// A switch that landed on case `case` (an index into its `cases`).
    Switch { target: u32, case: usize },
    Call { target: u32, continuation: u32 },
    Return { target: u32 },
}

impl Flow {
    pub fn target(&self) -> u32 {
        match *self {
            Flow::Next(t)
            | Flow::Jump { target: t, .. }
            | Flow::Branch { target: t, .. }
            | Flow::Switch { target: t, .. }
            | Flow::Call { target: t, .. }
            | Flow::Return { target: t } => t,
        }
    }
}

/// Why an instruction did not complete normally. `eip` is left at the
/// faulting instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    /// A guest exception. `next` is the saved EIP for trap-style faults
    /// (`int3`, `int n`, `into`), which is the following instruction.
    Fault { pc: u32, fault: Fault, next: Option<u32> },
    Memory { pc: u32, fault: MemFault },
    Unsupported { pc: u32, mnemonic: String, detail: String },
    Stop { pc: u32, reason: StopReason },
    /// A switch loaded a target that is not one of its validated cases.
    UnknownTarget { pc: u32, target: u32 },
}

/// Mask, sign-extend and other bit-level helpers over `u128` storage.
fn sext(v: u128, from: Width) -> i128 {
    let shift = 128 - from.bits();
    ((v << shift) as i128) >> shift
}

/// Evaluate a pure expression whose result is `width` bits wide, reading
/// operands through `val`. This is the single definition of every
/// [`Expr`]'s meaning; constant folding in the lifter uses it too.
pub fn eval(width: Width, e: &Expr, val: &dyn Fn(&Value) -> u128) -> u128 {
    let w = width;
    match e {
        Expr::Unary { op, a } => {
            let aw = a.width();
            let x = val(a);
            match op {
                UnOp::Not => !x & aw.mask(),
                UnOp::Neg => x.wrapping_neg() & aw.mask(),
                UnOp::Parity => (x as u8).count_ones().is_multiple_of(2) as u128,
                UnOp::Ctz => {
                    if x == 0 { aw.bits() as u128 } else { x.trailing_zeros() as u128 }
                }
                UnOp::Clz => {
                    if x == 0 { aw.bits() as u128 } else { (x.leading_zeros() - (128 - aw.bits())) as u128 }
                }
                UnOp::Bswap => {
                    let n = aw.bytes() as usize;
                    let bytes = x.to_le_bytes();
                    let mut out = [0u8; 16];
                    for i in 0..n {
                        out[i] = bytes[n - 1 - i];
                    }
                    u128::from_le_bytes(out)
                }
            }
        }
        Expr::Binary { op, a, b } => {
            let aw = a.width();
            let bits = aw.bits() as u128;
            let (x, y) = (val(a), val(b));
            let m = aw.mask();
            let r = match op {
                BinOp::Add => x.wrapping_add(y),
                BinOp::Sub => x.wrapping_sub(y),
                BinOp::Mul => x.wrapping_mul(y),
                BinOp::And => x & y,
                BinOp::Or => x | y,
                BinOp::Xor => x ^ y,
                BinOp::Shl => {
                    if y >= bits { 0 } else { x << y }
                }
                BinOp::Shr => {
                    if y >= bits { 0 } else { x >> y }
                }
                BinOp::Sar => (sext(x, aw) >> y.min(127)) as u128,
                BinOp::Rol | BinOp::Ror => {
                    let c = y % bits;
                    let c = if *op == BinOp::Ror && c != 0 { bits - c } else { c };
                    if c == 0 { x } else { (x << c) | (x >> (bits - c)) }
                }
                BinOp::Udiv => x.checked_div(y).unwrap_or(0),
                BinOp::Urem => x.checked_rem(y).unwrap_or(x),
                BinOp::Sdiv => {
                    let (sx, sy) = (sext(x, aw), sext(y, aw));
                    if sy == 0 { 0 } else { sx.wrapping_div(sy) as u128 }
                }
                BinOp::Srem => {
                    let (sx, sy) = (sext(x, aw), sext(y, aw));
                    if sy == 0 { x } else { sx.wrapping_rem(sy) as u128 }
                }
            };
            r & m
        }
        Expr::Compare { op, a, b } => {
            let aw = a.width();
            let (x, y) = (val(a), val(b));
            (match op {
                CmpOp::Eq => x == y,
                CmpOp::Ne => x != y,
                CmpOp::Ult => x < y,
                CmpOp::Ule => x <= y,
                CmpOp::Slt => sext(x, aw) < sext(y, aw),
                CmpOp::Sle => sext(x, aw) <= sext(y, aw),
            }) as u128
        }
        Expr::Cast { op, a } => {
            let x = val(a);
            match op {
                CastOp::Zext | CastOp::Trunc => x & w.mask(),
                CastOp::Sext => sext(x, a.width()) as u128 & w.mask(),
            }
        }
        Expr::Select { cond, a, b } => {
            if val(cond) != 0 {
                val(a)
            } else {
                val(b)
            }
        }
    }
}

/// Instruction-local temp storage. Ids are small and dense in practice.
#[derive(Default)]
struct Temps(Vec<Option<u128>>);

impl Temps {
    fn set(&mut self, t: Temp, v: u128) {
        let i = t.id as usize;
        if i >= self.0.len() {
            self.0.resize(i + 1, None);
        }
        self.0[i] = Some(v & t.width.mask());
    }

    fn get(&self, t: Temp) -> u128 {
        self.0.get(t.id as usize).copied().flatten().unwrap_or_else(|| panic!("t{} read before definition", t.id))
    }
}

struct Exec<'a, M: Memory> {
    cpu: &'a mut Cpu,
    mem: &'a mut M,
    temps: Temps,
    pc: u32,
    /// The following instruction: the saved EIP of trap-style faults.
    next: u32,
}

/// Faults whose saved EIP is the following instruction.
fn is_trap(f: Fault) -> bool {
    matches!(f, Fault::Breakpoint | Fault::Interrupt(_) | Fault::Overflow)
}

impl<M: Memory> Exec<'_, M> {
    fn val(&self, v: &Value) -> u128 {
        match v {
            Value::Temp(t) => self.temps.get(*t),
            Value::Const { bits, .. } => *bits,
        }
    }

    fn linear(&self, a: &Addr) -> u32 {
        let off = self.val(&a.offset) as u32;
        match a.seg {
            Segment::Flat => off,
            Segment::Fs => self.cpu.fs_base.wrapping_add(off),
        }
    }

    fn run(&mut self, stmts: &[Stmt]) -> Result<(), Exit> {
        for s in stmts {
            match s {
                Stmt::Assign { dst, expr } => {
                    let v = eval(dst.width, expr, &|v| self.val(v));
                    self.temps.set(*dst, v);
                }
                Stmt::ReadReg { dst, reg, slice } => {
                    let full = self.cpu.reg(*reg) as u128;
                    self.temps.set(*dst, full >> slice.shift());
                }
                Stmt::WriteReg { reg, slice, value } => {
                    let v = self.val(value) as u32;
                    let mask = (slice.width().mask() as u32) << slice.shift();
                    let old = self.cpu.reg(*reg);
                    self.cpu.set_reg(*reg, (old & !mask) | ((v << slice.shift()) & mask));
                }
                Stmt::ReadFlag { dst, flag } => {
                    let v = self.cpu.flag(*flag);
                    self.temps.set(*dst, v as u128);
                }
                Stmt::WriteFlag { flag, value } => match value {
                    FlagValue::Defined(v) => {
                        let v = self.val(v) != 0;
                        self.cpu.set_flag(*flag, v);
                    }
                    FlagValue::Undefined => {
                        let bit = 1 << flag.eflags_bit();
                        self.cpu.eflags &= !bit;
                        self.cpu.flags_valid &= !bit;
                    }
                },
                Stmt::ReadEflags { dst } => {
                    let v = self.cpu.eflags | EFLAGS_RESERVED;
                    self.temps.set(*dst, v as u128);
                }
                Stmt::WriteEflags { value, mask } => {
                    let v = self.val(value) as u32;
                    self.cpu.eflags = (self.cpu.eflags & !mask) | (v & mask) | EFLAGS_RESERVED;
                    self.cpu.flags_valid |= mask;
                }
                Stmt::ReadXmm { dst, reg } => {
                    let v = self.cpu.xmm[*reg as usize];
                    self.temps.set(*dst, v);
                }
                Stmt::WriteXmm { reg, value } => {
                    let v = self.val(value);
                    self.cpu.xmm[*reg as usize] = v;
                }
                Stmt::ReadMmx { dst, reg } => {
                    let st = &self.cpu.x87.st[*reg as usize];
                    let v = u64::from_le_bytes(st[..8].try_into().unwrap());
                    self.temps.set(*dst, v as u128);
                }
                Stmt::WriteMmx { reg, value } => {
                    let v = self.val(value) as u64;
                    let st = &mut self.cpu.x87.st[*reg as usize];
                    st[..8].copy_from_slice(&v.to_le_bytes());
                    st[8] = 0xFF;
                    st[9] = 0xFF;
                }
                Stmt::Load { dst, addr } => {
                    let a = self.linear(addr);
                    let mut buf = [0u8; 16];
                    let n = dst.width.bytes() as usize;
                    if let Err(fault) = self.mem.read(a, &mut buf[..n]) {
                        return Err(Exit::Memory { pc: self.pc, fault });
                    }
                    self.temps.set(*dst, u128::from_le_bytes(buf));
                }
                Stmt::Store { addr, value } => {
                    let a = self.linear(addr);
                    let n = value.width().bytes() as usize;
                    let bytes = self.val(value).to_le_bytes();
                    if let Err(fault) = self.mem.write(a, &bytes[..n]) {
                        return Err(Exit::Memory { pc: self.pc, fault });
                    }
                }
                Stmt::FaultIf { cond, fault } => {
                    if self.val(cond) != 0 {
                        let next = is_trap(*fault).then_some(self.next);
                        return Err(Exit::Fault { pc: self.pc, fault: *fault, next });
                    }
                }
                Stmt::StopIf { cond, reason } => {
                    if self.val(cond) != 0 {
                        return Err(Exit::Stop { pc: self.pc, reason: *reason });
                    }
                }
                Stmt::If { cond, then, otherwise } => {
                    if self.val(cond) != 0 {
                        self.run(then)?;
                    } else {
                        self.run(otherwise)?;
                    }
                }
                Stmt::Intrinsic { op: Intrinsic::X87(op), args, dsts } => {
                    use crate::x87_exec::Outcome;
                    let args: Vec<u128> = args.iter().map(|a| self.val(a)).collect();
                    match crate::x87_exec::exec(self.cpu, self.pc, *op, &args) {
                        Ok(out) => {
                            for (d, v) in dsts.iter().zip(out) {
                                self.temps.set(*d, v);
                            }
                        }
                        Err(Outcome::Fault(fault)) => return Err(Exit::Fault { pc: self.pc, fault, next: None }),
                        Err(Outcome::Stop(reason)) => return Err(Exit::Stop { pc: self.pc, reason }),
                    }
                }
                Stmt::Intrinsic { op, args, dsts } => {
                    let args: Vec<u128> = args.iter().map(|a| self.val(a)).collect();
                    match intrinsic(self.cpu, *op, &args) {
                        Ok(out) => {
                            for (d, v) in dsts.iter().zip(out) {
                                self.temps.set(*d, v);
                            }
                        }
                        Err(fault) => {
                            let next = is_trap(fault).then_some(self.next);
                            return Err(Exit::Fault { pc: self.pc, fault, next });
                        }
                    }
                }
                Stmt::Repeat { kind, body } => {
                    // A fault (or stop) inside an iteration keeps the
                    // completed iterations' progress but reports EFLAGS as
                    // they were before the instruction.
                    let flags = (self.cpu.eflags, self.cpu.flags_valid);
                    loop {
                        let ecx = self.cpu.reg(Gpr::Ecx);
                        if ecx == 0 {
                            break;
                        }
                        if let Err(e) = self.run(body) {
                            (self.cpu.eflags, self.cpu.flags_valid) = flags;
                            return Err(e);
                        }
                        self.cpu.set_reg(Gpr::Ecx, ecx.wrapping_sub(1));
                        let zf = self.cpu.flag(Flag::Zf);
                        match kind {
                            RepKind::RepE if !zf => break,
                            RepKind::RepNe if zf => break,
                            _ => {}
                        }
                    }
                }
                Stmt::LazyFlags(l) => {
                    let args: Vec<u128> = l.args.iter().map(|a| self.val(a)).collect();
                    for (f, v) in l.formula.eval(&args) {
                        self.cpu.set_flag(f, v);
                    }
                }
                Stmt::Unsupported { mnemonic, detail } => {
                    return Err(Exit::Unsupported {
                        pc: self.pc,
                        mnemonic: mnemonic.clone(),
                        detail: detail.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Execute one instruction. On success the CPU's `eip` is the destination;
/// on an [`Exit`] it is the instruction's own PC.
pub fn step<M: Memory>(cpu: &mut Cpu, mem: &mut M, insn: &IrInsn) -> Result<Flow, Exit> {
    cpu.eip = insn.pc;
    let next = insn.next_pc();
    let mut ex = Exec { cpu, mem, temps: Temps::default(), pc: insn.pc, next };
    ex.run(&insn.stmts)?;
    let target = |ex: &Exec<M>, t: &Target| match t {
        Target::Direct(a) => *a,
        Target::Dynamic(v) => ex.val(v) as u32,
    };
    let flow = match &insn.control {
        Control::Next => Flow::Next(next),
        Control::Jump { target: t, tail, .. } => Flow::Jump { target: target(&ex, t), tail: *tail },
        Control::Branch { cond, taken, fallthrough } => {
            let c = ex.val(cond) != 0;
            Flow::Branch { target: if c { taken.pc } else { fallthrough.pc }, taken: c }
        }
        Control::Switch { target: t, cases, .. } => {
            let t = ex.val(t) as u32;
            match cases.iter().position(|c| c.pc == t) {
                Some(case) => Flow::Switch { target: t, case },
                None => return Err(Exit::UnknownTarget { pc: insn.pc, target: t }),
            }
        }
        Control::Call { target: t, continuation, .. } => {
            Flow::Call { target: target(&ex, t), continuation: *continuation }
        }
        Control::Return { target: t, .. } => Flow::Return { target: ex.val(t) as u32 },
        Control::Fault(f) => {
            return Err(Exit::Fault { pc: insn.pc, fault: *f, next: is_trap(*f).then_some(next) });
        }
        Control::Stop(reason) => return Err(Exit::Stop { pc: insn.pc, reason: *reason }),
    };
    ex.cpu.eip = flow.target();
    Ok(flow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u32, width: Width) -> Temp {
        Temp { id, width }
    }

    fn insn(stmts: Vec<Stmt>) -> IrInsn {
        IrInsn { pc: 0x1000, len: 1, atomic: None, stmts, control: Control::Next }
    }

    fn eval1(op: BinOp, w: Width, a: u128, b: u128) -> u128 {
        let d = t(0, w);
        let i = insn(vec![Stmt::Assign {
            dst: d,
            expr: Expr::Binary { op, a: Value::constant(w, a), b: Value::constant(w, b) },
        }]);
        i.validate().unwrap();
        let mut cpu = Cpu::default();
        let mut mem = SparseMemory::new();
        let mut ex = Exec { cpu: &mut cpu, mem: &mut mem, temps: Temps::default(), pc: 0, next: 1 };
        assert!(ex.run(&i.stmts).is_ok());
        ex.temps.get(d)
    }

    #[test]
    fn shifts_are_total() {
        assert_eq!(eval1(BinOp::Shl, Width::W8, 0x81, 1), 0x02);
        assert_eq!(eval1(BinOp::Shl, Width::W8, 0x81, 8), 0);
        assert_eq!(eval1(BinOp::Shr, Width::W32, 0x8000_0000, 40), 0);
        assert_eq!(eval1(BinOp::Sar, Width::W8, 0x80, 3), 0xF0);
        assert_eq!(eval1(BinOp::Sar, Width::W8, 0x80, 200), 0xFF);
        assert_eq!(eval1(BinOp::Rol, Width::W8, 0x81, 9), 0x03);
        assert_eq!(eval1(BinOp::Ror, Width::W16, 0x0001, 1), 0x8000);
    }

    #[test]
    fn division_is_total() {
        assert_eq!(eval1(BinOp::Udiv, Width::W32, 7, 0), 0);
        assert_eq!(eval1(BinOp::Urem, Width::W32, 7, 0), 7);
        assert_eq!(eval1(BinOp::Sdiv, Width::W32, 0x8000_0000, 0xFFFF_FFFF), 0x8000_0000);
        assert_eq!(eval1(BinOp::Srem, Width::W32, 0x8000_0000, 0xFFFF_FFFF), 0);
        // -7 / 2 = -3 rem -1 (truncating)
        assert_eq!(eval1(BinOp::Sdiv, Width::W8, 0xF9, 2), 0xFD);
        assert_eq!(eval1(BinOp::Srem, Width::W8, 0xF9, 2), 0xFF);
    }

    #[test]
    fn partial_register_writes_merge() {
        let mut cpu = Cpu::default();
        cpu.set_reg(Gpr::Eax, 0x1122_3344);
        let i = insn(vec![Stmt::WriteReg { reg: Gpr::Eax, slice: Slice::High8, value: Value::constant(Width::W8, 0xAB) }]);
        step(&mut cpu, &mut SparseMemory::new(), &i).unwrap();
        assert_eq!(cpu.reg(Gpr::Eax), 0x1122_AB44);
    }

    #[test]
    fn validator_rejects_width_mismatch() {
        let i = insn(vec![Stmt::Assign {
            dst: t(0, Width::W32),
            expr: Expr::Binary { op: BinOp::Add, a: Value::u32(1), b: Value::constant(Width::W8, 1) },
        }]);
        assert!(i.validate().is_err());
        let i = insn(vec![Stmt::WriteReg { reg: Gpr::Eax, slice: Slice::Full, value: t(3, Width::W32).into() }]);
        assert!(i.validate().is_err());
    }

    #[test]
    fn trap_style_fault_if_reports_the_next_instruction() {
        for (fault, next) in [(Fault::Overflow, Some(0x1001)), (Fault::DivideError, None)] {
            let i = insn(vec![Stmt::FaultIf { cond: Value::bool(true), fault }]);
            let e = step(&mut Cpu::default(), &mut SparseMemory::new(), &i).unwrap_err();
            assert_eq!(e, Exit::Fault { pc: 0x1000, fault, next });
        }
    }

    #[test]
    fn if_runs_one_side() {
        let w = |v| Stmt::WriteReg { reg: Gpr::Eax, slice: Slice::Full, value: Value::u32(v) };
        for (c, want) in [(true, 1), (false, 2)] {
            let i = insn(vec![Stmt::If { cond: Value::bool(c), then: vec![w(1)], otherwise: vec![w(2)] }]);
            i.validate().unwrap();
            let mut cpu = Cpu::default();
            step(&mut cpu, &mut SparseMemory::new(), &i).unwrap();
            assert_eq!(cpu.reg(Gpr::Eax), want);
        }
    }

    #[test]
    fn validator_rejects_a_flag_written_twice_on_one_path() {
        let set = |v| Stmt::WriteFlag { flag: Flag::Cf, value: FlagValue::Defined(Value::bool(v)) };
        assert!(insn(vec![set(true), set(false)]).validate().is_err());
        // Once on each side of an If is one write per path...
        let one = Stmt::If { cond: Value::bool(true), then: vec![set(true)], otherwise: vec![set(false)] };
        assert!(insn(vec![one.clone()]).validate().is_ok());
        // ...but not followed by another.
        assert!(insn(vec![one, set(true)]).validate().is_err());
    }

    #[test]
    fn validator_checks_the_atomic_shape() {
        let a = Addr { seg: Segment::Flat, offset: Value::u32(0x100) };
        let ld = Stmt::Load { dst: t(0, Width::W32), addr: a };
        let st = Stmt::Store { addr: a, value: t(0, Width::W32).into() };
        let reg = Stmt::WriteReg { reg: Gpr::Eax, slice: Slice::Full, value: t(0, Width::W32).into() };
        let xchg = Atomic { op: AtomicOp::Xchg, operand: Some(Value::u32(7)), old: Some((Gpr::Eax, Slice::Full)) };
        let atomic = |stmts| IrInsn { atomic: Some(xchg), ..insn(stmts) };
        assert!(atomic(vec![ld.clone(), st.clone(), reg.clone()]).validate().is_ok());
        assert!(atomic(vec![ld.clone(), reg.clone(), st.clone()]).validate().is_err(), "register write before the store");
        assert!(atomic(vec![ld.clone()]).validate().is_err(), "no store");
        let other = Stmt::Store { addr: Addr { seg: Segment::Flat, offset: Value::u32(0x104) }, value: Value::u32(0) };
        assert!(atomic(vec![ld.clone(), other]).validate().is_err(), "different address");
        // The descriptor must agree with the statements.
        let with = |a: Atomic| IrInsn { atomic: Some(a), ..insn(vec![ld.clone(), st.clone(), reg.clone()]) };
        assert!(with(Atomic { old: None, ..xchg }).validate().is_err(), "xchg without its register");
        assert!(with(Atomic { operand: None, ..xchg }).validate().is_err(), "xchg without its operand");
        assert!(with(Atomic { operand: Some(Value::constant(Width::W8, 7)), ..xchg }).validate().is_err(), "width");
        let late = t(1, Width::W32);
        let read_late = Stmt::ReadReg { dst: late, reg: Gpr::Ecx, slice: Slice::Full };
        let i = IrInsn {
            atomic: Some(Atomic { operand: Some(late.into()), ..xchg }),
            ..insn(vec![ld.clone(), read_late, st.clone(), reg.clone()])
        };
        assert!(i.validate().is_err(), "operand computed after the load");
        let flag = Stmt::ReadFlag { dst: t(2, Width::W1), flag: Flag::Cf };
        assert!(atomic(vec![flag, ld.clone(), st.clone(), reg.clone()]).validate().is_err(), "impure prefix");
        let stop = IrInsn { control: Control::Stop(StopReason::Halt), ..atomic(vec![ld, st, reg]) };
        assert!(stop.validate().is_err(), "atomic transfer");
    }

    #[test]
    fn a_fault_inside_repeat_restores_eflags_and_keeps_progress() {
        // while ECX: load [ESI]; CF = 1; ZF undefined; ESI += 1. The third
        // load leaves the mapping.
        let esi = t(0, Width::W32);
        let v = t(1, Width::W8);
        let n = t(2, Width::W32);
        let body = vec![
            Stmt::ReadReg { dst: esi, reg: Gpr::Esi, slice: Slice::Full },
            Stmt::Load { dst: v, addr: Addr { seg: Segment::Flat, offset: esi.into() } },
            Stmt::WriteFlag { flag: Flag::Cf, value: FlagValue::Defined(Value::bool(true)) },
            Stmt::WriteFlag { flag: Flag::Zf, value: FlagValue::Undefined },
            Stmt::Assign { dst: n, expr: Expr::Binary { op: BinOp::Add, a: esi.into(), b: Value::u32(1) } },
            Stmt::WriteReg { reg: Gpr::Esi, slice: Slice::Full, value: n.into() },
        ];
        let i = insn(vec![Stmt::Repeat { kind: RepKind::Rep, body }]);
        i.validate().unwrap();
        let mut mem = SparseMemory::new();
        mem.map(0x100, 2);
        let mut cpu = Cpu::default();
        cpu.set_reg(Gpr::Esi, 0x100);
        cpu.set_reg(Gpr::Ecx, 5);
        cpu.eflags = EFLAGS_RESERVED | 0x40;
        let before = (cpu.eflags, cpu.flags_valid);
        assert!(matches!(step(&mut cpu, &mut mem, &i), Err(Exit::Memory { .. })));
        assert_eq!((cpu.eflags, cpu.flags_valid), before, "EFLAGS as before the instruction");
        assert_eq!((cpu.reg(Gpr::Ecx), cpu.reg(Gpr::Esi)), (3, 0x102), "two iterations' progress kept");
    }

    #[test]
    fn ldmxcsr_rejects_daz_on_the_pentium_iii() {
        let mut cpu = Cpu::default();
        let daz = MXCSR_DEFAULT as u128 | 0x40;
        assert_eq!(intrinsic(&mut cpu, Intrinsic::MxcsrWrite, &[daz]), Err(Fault::GeneralProtection));
        assert_eq!(cpu.mxcsr, MXCSR_DEFAULT);
        assert_eq!(intrinsic(&mut cpu, Intrinsic::MxcsrWrite, &[0xffbf]), Ok(vec![]));
        assert_eq!(cpu.mxcsr, 0xffbf);
    }

    #[test]
    fn function_validator_checks_successors_and_entries() {
        let block = |start: u32, control| IrBlock { start, insns: vec![IrInsn { pc: start, control, ..insn(vec![]) }] };
        let func = |blocks: Vec<IrBlock>, interior: Vec<u32>| IrFunction {
            entry: 0x1000,
            blocks: blocks.into_iter().map(|b| (b.start, b)).collect(),
            interior_entries: interior,
            errors: vec![],
        };
        let ret = || Control::Stop(StopReason::Halt);
        assert!(func(vec![block(0x1000, Control::Next), block(0x1001, ret())], vec![0x1001]).validate().is_ok());
        assert!(func(vec![block(0x1000, Control::Next)], vec![]).validate().is_err(), "falls into nothing");
        let br = |tail| Control::Branch {
            cond: Value::bool(true),
            taken: Dest { pc: 0x2000, tail },
            fallthrough: Dest::local(0x1001),
        };
        assert!(func(vec![block(0x1000, br(false)), block(0x1001, ret())], vec![]).validate().is_err());
        assert!(func(vec![block(0x1000, br(true)), block(0x1001, ret())], vec![]).validate().is_ok());
        assert!(func(vec![block(0x1000, ret())], vec![0x1004]).validate().is_err(), "interior entry not a block");
        let mut f = func(vec![block(0x1000, ret())], vec![]);
        let b = f.blocks.remove(&0x1000).unwrap();
        f.blocks.insert(0x1000, IrBlock { start: 0x0fff, ..b });
        assert!(f.validate().is_err(), "key differs from start");
    }

    #[test]
    fn intrinsics_act_on_fp_state() {
        let mut cpu = Cpu::default();
        cpu.x87.control = 0x027F;
        assert_eq!(intrinsic(&mut cpu, Intrinsic::X87ReadControl, &[]), Ok(vec![0x027F]));
        assert_eq!(intrinsic(&mut cpu, Intrinsic::MxcsrWrite, &[0x1_0000]), Err(Fault::GeneralProtection));
        assert_eq!(cpu.mxcsr, MXCSR_DEFAULT);
        assert_eq!(intrinsic(&mut cpu, Intrinsic::X87Wait, &[]), Ok(vec![]));
        cpu.x87.status = 0x0081; // IE pending, ES
        assert_eq!(intrinsic(&mut cpu, Intrinsic::X87Wait, &[]), Err(Fault::FpError));
    }

    /// `Mark` is IR state, not a helper: it runs without crates/x87.
    #[test]
    fn x87_mark_records_the_last_instruction() {
        use crate::x87::{GUEST_CS, GUEST_DS, X87Op};
        let mut cpu = Cpu::default();
        let mut mem = SparseMemory::new();
        let mark = |mem: bool| Stmt::Intrinsic {
            op: Intrinsic::X87(X87Op::Mark { fop: 0x0d9 | 0xf800, mem }),
            args: if mem { vec![Value::u32(0x1234)] } else { vec![] },
            dsts: vec![],
        };
        let i = insn(vec![mark(true)]);
        i.validate().unwrap();
        step(&mut cpu, &mut mem, &i).unwrap();
        let x = &cpu.x87;
        assert_eq!((x.instruction_pointer, x.opcode, x.data_pointer), (0x1000, 0x0d9, 0x1234));
        assert_eq!((x.instruction_selector, x.data_selector), (GUEST_CS, GUEST_DS));
        // A register form keeps FDP.
        let mut i = insn(vec![mark(false)]);
        i.pc = 0x2000;
        step(&mut cpu, &mut mem, &i).unwrap();
        assert_eq!((cpu.x87.instruction_pointer, cpu.x87.data_pointer), (0x2000, 0x1234));
    }

    /// Helper-backed ops run through crates/x87: fxam of an empty ST(0)
    /// classifies it (C3|C0), and a pending exception is #MF at a waiting op.
    #[test]
    fn x87_ops_run_through_the_helpers() {
        use crate::x87::{X87Const, X87Op};
        let mut cpu = Cpu::default();
        let mut mem = SparseMemory::new();
        let i = insn(vec![Stmt::Intrinsic { op: Intrinsic::X87(X87Op::Xam), args: vec![], dsts: vec![] }]);
        step(&mut cpu, &mut mem, &i).unwrap();
        assert_eq!(cpu.x87.status & 0x4500, 0x4100);
        cpu.x87.status = 0x8081;
        let before = cpu.x87.clone();
        let i = insn(vec![Stmt::Intrinsic {
            op: Intrinsic::X87(X87Op::LoadConst(X87Const::One)),
            args: vec![],
            dsts: vec![],
        }]);
        assert!(matches!(step(&mut cpu, &mut mem, &i), Err(Exit::Fault { pc: 0x1000, fault: Fault::FpError, .. })));
        assert_eq!(cpu.x87, before);
    }

    #[test]
    fn a_pending_x87_exception_stops_mmx_and_emms() {
        let mut cpu = Cpu::default();
        assert_eq!(intrinsic(&mut cpu, Intrinsic::X87Wait, &[]), Ok(vec![]));
        // ES set (an unmasked IE recorded earlier): #MF, nothing changes.
        cpu.x87.status = 0x8081;
        cpu.x87.tag = 0x5555;
        let before = cpu.x87.clone();
        assert_eq!(intrinsic(&mut cpu, Intrinsic::X87Wait, &[]), Err(Fault::FpError));
        assert_eq!(intrinsic(&mut cpu, Intrinsic::Emms, &[]), Err(Fault::FpError));
        assert_eq!(cpu.x87, before);
        cpu.x87.status = 0;
        assert_eq!(intrinsic(&mut cpu, Intrinsic::Emms, &[]), Ok(vec![]));
        assert_eq!(cpu.x87.tag, 0xFFFF);
    }

    #[test]
    fn the_tsc_counts_reads_and_wraps() {
        let mut cpu = Cpu { tsc: u64::MAX, ..Cpu::default() };
        assert_eq!(Intrinsic::ReadTsc.signature(), (&[][..], &[Width::W64][..]));
        assert_eq!(intrinsic(&mut cpu, Intrinsic::ReadTsc, &[]), Ok(vec![u64::MAX as u128]));
        assert_eq!(intrinsic(&mut cpu, Intrinsic::ReadTsc, &[]), Ok(vec![0]));
        assert_eq!(cpu.tsc, 1);
    }
}

