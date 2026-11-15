//! Semantic IR for lifted guest (32-bit x86) code.
//!
//! This is the contract between the lifter (`crates/lift`, which turns
//! decoded instructions into IR) and every backend (the C11 emitter first,
//! maybe LLVM or Cranelift later, and the reference interpreter in
//! [`interp`]). It describes *what an instruction does to architectural
//! state*, not how a host should do it, so it contains no C syntax and no
//! inferred host signatures (see `docs/LIFTER.md` §1 and §3).
//!
//! # Shape
//!
//! - An [`IrFunction`] is a set of [`IrBlock`]s keyed by guest address.
//! - An [`IrBlock`] is a straight-line run of [`IrInsn`]s. Every instruction
//!   but the last has [`Control::Next`] (or is an unsupported stop).
//! - An [`IrInsn`] is exactly one guest instruction: its PC and length, a
//!   list of [`Stmt`]s that read and write architectural state, and the
//!   [`Control`] outcome that says where execution goes next.
//!
//! # Values and temporaries
//!
//! Statements compute into [`Temp`]s: fixed-width, instruction-local
//! values. A temp is assigned by exactly one statement, before any use, and
//! is never visible outside the instruction that defines it. (Inside a
//! [`Stmt::Repeat`] body a temp is redefined on every iteration.) Everything
//! that lives across instructions is architectural state: general registers,
//! flags, memory. That keeps the IR trivially "correct before optimised":
//! a backend may hold registers in locals, but the IR itself never asks it
//! to reason about liveness across instructions.
//!
//! All arithmetic is on unsigned fixed-width bit patterns; signedness lives
//! in the operation ([`BinOp::Sar`], [`CmpOp::Slt`], [`CastOp::Sext`], ...),
//! never in the value. Every operation is *total*: there is no undefined
//! behaviour, no poison, no host traps. Where x86 would fault (divide by
//! zero), the lifter emits an explicit [`Stmt::FaultIf`] before the
//! operation, and the operation itself still has a defined (if useless)
//! result. Backends must preserve that totality; a C backend may not lower
//! [`BinOp::Shl`] to a bare `<<`, for instance.
//!
//! # Effects and faults
//!
//! Statements execute in order. Statements that may fault ([`Stmt::Load`],
//! [`Stmt::Store`], [`Stmt::FaultIf`], [`Stmt::Unsupported`]) leave the
//! instruction at that point with every *earlier* effect applied. The lifter
//! orders statements so that all potentially faulting reads and checks come
//! before the first register or flag write, which makes a fault report the
//! instruction's architectural pre-state (x86 faults are precise). The
//! remaining exception is a store that faults after an earlier store of the
//! same instruction (`push` of a memory operand, `movs`), which matches the
//! hardware closely enough to report and is documented where it happens.
//!
//! # Flags
//!
//! Each flag an instruction touches gets at most one [`Stmt::WriteFlag`]
//! on any execution path (per iteration of a [`Stmt::Repeat`] body; once
//! across an [`Stmt::If`] and the statements around it), carrying either a
//! defined value or [`FlagValue::Undefined`]. The validator enforces this.
//! A flag with no write is *preserved*. "Undefined" is a first-class outcome
//! so replay and differential tests can mask it rather than compare garbage;
//! see [`IrInsn::flag_effects`].
//!
//! **Undefined flags store 0.** This is normative, not an interpreter
//! detail: writing [`FlagValue::Undefined`] clears the flag's EFLAGS bit and
//! its bit in the defined mask. Every backend must do the same, so a later
//! `pushfd`/`lahf` (which observe the stored bit) agrees across backends.
//!
//! # Lazy flags
//!
//! The lifter never produces [`Stmt::LazyFlags`]; the optimiser (`crates/opt`)
//! does, for flags that are seldom read before they are written again. Its
//! meaning is exactly that of the [`Stmt::WriteFlag`]s it replaces, executed
//! where it stands: each flag gets its formula's value and becomes defined. A
//! backend may compute the values later, as long as they are exact wherever
//! they are read or state can be observed; see [`LazyFlags`].
//!
//! # Atomic instructions
//!
//! An [`IrInsn`] with [`IrInsn::atomic`] set (LOCK prefix, or `xchg` with
//! memory) carries its full semantics in its statements, as every
//! instruction does, *and* an [`Atomic`] descriptor naming the one
//! operation it is. The statements have a fixed shape the validator checks:
//!
//! - a pure prefix (`Assign`, `ReadReg`) that computes the address and the
//!   descriptor's operand;
//! - exactly one [`Stmt::Load`] and one [`Stmt::Store`], to the same
//!   [`Addr`], load first, nothing nested, no state written before the
//!   store; control [`Control::Next`].
//!
//! The interpreter runs the statements. A backend with real concurrency
//! runs the prefix and then performs the descriptor's operation as one
//! indivisible step (the runtime's `h2abi_v1_atomic_rmw`, `xchg` and
//! `cmpxchg`, which validate the whole span and update memory, flags and
//! result under one lock, changing nothing on a fault). The descriptor and
//! the statements describe the same effect; the emitter's differential
//! tests hold them together.
//!
//! # Floating-point state and intrinsics
//!
//! x87 and SSE arithmetic are not spelled out in integer statements.
//! FP state is architectural state like the GPRs, held in the CPU in the
//! same shape as the runtime's `X87Abi` (eight 80-bit registers in physical
//! order, control/status/tag words with TOP in status bits 11-13,
//! last-instruction pointers) plus MXCSR. It is read and changed only
//! through [`Stmt::Intrinsic`]: a named helper with explicit value operands
//! and results that may also read and write the FP state, and may raise a
//! fault. SSE goes through [`Intrinsic::Sse`] (the `simd` crate and its C
//! twin `h2simd.h`); x87 through [`Intrinsic::X87`], one op per helper of
//! `crates/x87` (`h2x87.h`), with stack-relative operands resolved against
//! TOP inside the helper. The [`x87`] module has the op set and its
//! outcome rules. Each intrinsic has one definition in the interpreter and
//! one in the emitter; the differential tests hold the two together.

pub mod interp;
mod display;
mod validate;
pub mod x87;
mod x87_exec;

pub use validate::ValidateError;

/// Width of a value in bits.
///
/// `W1` is a boolean (flags and comparison results). `W80` and `W128` exist
/// for x87 extended values and XMM registers; the integer core does not use
/// them yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Width {
    W1,
    W8,
    W16,
    W32,
    W64,
    W80,
    W128,
}

impl Width {
    pub const fn bits(self) -> u32 {
        match self {
            Width::W1 => 1,
            Width::W8 => 8,
            Width::W16 => 16,
            Width::W32 => 32,
            Width::W64 => 64,
            Width::W80 => 80,
            Width::W128 => 128,
        }
    }

    /// Bytes occupied in memory. `W1` has no memory representation.
    pub const fn bytes(self) -> u32 {
        self.bits() / 8
    }

    /// All-ones pattern of this width.
    pub const fn mask(self) -> u128 {
        if self.bits() == 128 { u128::MAX } else { (1u128 << self.bits()) - 1 }
    }

    /// The width with `bytes` bytes, for memory and register operands.
    pub const fn from_bytes(bytes: u32) -> Option<Width> {
        Some(match bytes {
            1 => Width::W8,
            2 => Width::W16,
            4 => Width::W32,
            8 => Width::W64,
            10 => Width::W80,
            16 => Width::W128,
            _ => return None,
        })
    }

    /// Twice this width, for widening multiplies and dividends.
    pub const fn double(self) -> Option<Width> {
        Some(match self {
            Width::W8 => Width::W16,
            Width::W16 => Width::W32,
            Width::W32 => Width::W64,
            Width::W64 => Width::W128,
            _ => return None,
        })
    }
}

/// An instruction-local SSA value. `id` is unique within its [`IrInsn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Temp {
    pub id: u32,
    pub width: Width,
}

/// An operand: a temp or a constant of a stated width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Value {
    Temp(Temp),
    /// `bits` must fit in `width` (no bits above it set).
    Const { width: Width, bits: u128 },
}

impl Value {
    pub const fn width(&self) -> Width {
        match self {
            Value::Temp(t) => t.width,
            Value::Const { width, .. } => *width,
        }
    }

    /// A constant, truncated to `width`.
    pub const fn constant(width: Width, bits: u128) -> Value {
        Value::Const { width, bits: bits & width.mask() }
    }

    pub const fn bool(b: bool) -> Value {
        Value::Const { width: Width::W1, bits: b as u128 }
    }

    pub const fn u32(v: u32) -> Value {
        Value::Const { width: Width::W32, bits: v as u128 }
    }

    pub const fn as_const(&self) -> Option<u128> {
        match self {
            Value::Const { bits, .. } => Some(*bits),
            Value::Temp(_) => None,
        }
    }
}

impl From<Temp> for Value {
    fn from(t: Temp) -> Value {
        Value::Temp(t)
    }
}

/// A 32-bit general-purpose register, numbered as x86 encodes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Gpr {
    Eax = 0,
    Ecx = 1,
    Edx = 2,
    Ebx = 3,
    Esp = 4,
    Ebp = 5,
    Esi = 6,
    Edi = 7,
}

impl Gpr {
    pub const ALL: [Gpr; 8] = [Gpr::Eax, Gpr::Ecx, Gpr::Edx, Gpr::Ebx, Gpr::Esp, Gpr::Ebp, Gpr::Esi, Gpr::Edi];

    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Which bits of a [`Gpr`] an access touches.
///
/// Writing a slice narrower than [`Slice::Full`] is a *partial-register
/// merge*: the other bits of the full register are preserved, exactly as
/// x86 does in 32-bit mode (`mov ah, 1` leaves bits 0-7 and 16-31 alone).
/// There is no implicit zero extension anywhere in 32-bit x86.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slice {
    /// Bits 0-7: `al`, `cl`, `dl`, `bl`.
    Low8,
    /// Bits 8-15: `ah`, `ch`, `dh`, `bh`. Only exists for EAX..EBX.
    High8,
    /// Bits 0-15: `ax`, `sp`, ...
    Low16,
    /// Bits 0-31.
    Full,
}

impl Slice {
    pub const fn width(self) -> Width {
        match self {
            Slice::Low8 | Slice::High8 => Width::W8,
            Slice::Low16 => Width::W16,
            Slice::Full => Width::W32,
        }
    }

    /// Bit position of the slice's least significant bit.
    pub const fn shift(self) -> u32 {
        match self {
            Slice::High8 => 8,
            _ => 0,
        }
    }
}

/// An individually modelled EFLAGS bit.
///
/// The six status flags plus DF, which string instructions read. Other
/// EFLAGS bits (IF, TF, IOPL, ...) are only reachable through
/// [`Stmt::ReadEflags`]/[`Stmt::WriteEflags`], so `pushfd`/`popfd` keep
/// them intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flag {
    Cf,
    Pf,
    Af,
    Zf,
    Sf,
    Df,
    Of,
}

impl Flag {
    pub const ALL: [Flag; 7] = [Flag::Cf, Flag::Pf, Flag::Af, Flag::Zf, Flag::Sf, Flag::Df, Flag::Of];
    /// The flags arithmetic instructions define: everything but DF.
    pub const STATUS: [Flag; 6] = [Flag::Cf, Flag::Pf, Flag::Af, Flag::Zf, Flag::Sf, Flag::Of];

    /// The flag's bit position in EFLAGS.
    pub const fn eflags_bit(self) -> u32 {
        match self {
            Flag::Cf => 0,
            Flag::Pf => 2,
            Flag::Af => 4,
            Flag::Zf => 6,
            Flag::Sf => 7,
            Flag::Df => 10,
            Flag::Of => 11,
        }
    }
}

/// A set of [`Flag`]s, as a bitmask over [`Flag::eflags_bit`] positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct FlagSet(pub u32);

impl FlagSet {
    pub const fn empty() -> Self {
        FlagSet(0)
    }
    pub const fn contains(self, f: Flag) -> bool {
        self.0 & (1 << f.eflags_bit()) != 0
    }
    pub fn insert(&mut self, f: Flag) {
        self.0 |= 1 << f.eflags_bit();
    }
    pub fn iter(self) -> impl Iterator<Item = Flag> {
        Flag::ALL.into_iter().filter(move |f| self.contains(*f))
    }
}

/// The value a [`Stmt::WriteFlag`] stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagValue {
    /// A `W1` value.
    Defined(Value),
    /// Architecturally undefined after this instruction. Every backend
    /// stores 0 and clears the flag's defined bit (normative; see the crate
    /// docs); never host undef/poison.
    Undefined,
}

/// What each flag an instruction touches ends up as. Flags in neither set
/// are preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlagEffects {
    pub defined: FlagSet,
    pub undefined: FlagSet,
}

/// Which segment base applies to a memory access.
///
/// The Xbox runs flat: CS/DS/ES/SS have base 0, so all of those are
/// [`Segment::Flat`]. FS points at the guest KPCR and has a real base held
/// in CPU state. GS is unused by retail titles and is rejected by the
/// lifter rather than guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Segment {
    Flat,
    Fs,
}

/// A guest memory address: a segment and a `W32` offset. The linear
/// address is `segment_base + offset`, wrapping at 2^32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Addr {
    pub seg: Segment,
    pub offset: Value,
}

/// Single-operand pure operations. The result has the operand's width
/// unless stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    /// Bitwise complement.
    Not,
    /// Two's complement negation.
    Neg,
    /// `W1`: true if the *low eight bits* have an even number of ones. This
    /// is x86 PF, which only ever looks at the low byte.
    Parity,
    /// Number of trailing zero bits; the operand's bit count if it is zero.
    Ctz,
    /// Number of leading zero bits; the operand's bit count if it is zero.
    Clz,
    /// Reverse the byte order (`bswap`). Width must be a multiple of 8.
    Bswap,
}

/// Two-operand pure operations. Both operands and the result have the same
/// width, except where stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    /// Low half of the product.
    Mul,
    And,
    Or,
    Xor,
    /// Shift left. The count is the second operand, read as unsigned; a
    /// count of the width or more gives 0. (x86 masks counts to 5 bits
    /// first: the lifter does that explicitly, it is not part of this op.)
    Shl,
    /// Logical shift right; a count of the width or more gives 0.
    Shr,
    /// Arithmetic shift right; a count of the width or more gives all
    /// copies of the sign bit.
    Sar,
    /// Rotate left by `count mod width`.
    Rol,
    /// Rotate right by `count mod width`.
    Ror,
    /// Unsigned quotient. Division by zero gives 0: the lifter emits the
    /// architectural `#DE` check as a [`Stmt::FaultIf`] first.
    Udiv,
    /// Unsigned remainder. Division by zero gives the dividend.
    Urem,
    /// Signed quotient, truncating toward zero. Division by zero gives 0;
    /// `MIN / -1` wraps to `MIN`.
    Sdiv,
    /// Signed remainder, sign of the dividend. Division by zero gives the
    /// dividend; `MIN % -1` gives 0.
    Srem,
}

/// Comparisons. Operands have equal width; the result is `W1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpOp {
    Eq,
    Ne,
    /// Unsigned less than.
    Ult,
    Ule,
    /// Signed less than.
    Slt,
    Sle,
}

/// Width changes. The target width is the destination temp's width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CastOp {
    /// Zero extend to a wider (or equal) width.
    Zext,
    /// Sign extend to a wider (or equal) width.
    Sext,
    /// Keep the low bits, to a narrower (or equal) width.
    Trunc,
}

/// A pure computation, assigned to a temp by [`Stmt::Assign`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Expr {
    Unary { op: UnOp, a: Value },
    Binary { op: BinOp, a: Value, b: Value },
    Compare { op: CmpOp, a: Value, b: Value },
    Cast { op: CastOp, a: Value },
    /// `if cond { a } else { b }`; `cond` is `W1`, `a` and `b` the
    /// destination's width. Both sides are already-computed values, so this
    /// never has effects.
    Select { cond: Value, a: Value, b: Value },
}

impl Expr {
    /// The operands, in order.
    pub fn operands(&self) -> impl Iterator<Item = &Value> {
        let (a, b, c) = match self {
            Expr::Unary { a, .. } | Expr::Cast { a, .. } => (Some(a), None, None),
            Expr::Binary { a, b, .. } | Expr::Compare { a, b, .. } => (Some(a), Some(b), None),
            Expr::Select { cond, a, b } => (Some(cond), Some(a), Some(b)),
        };
        a.into_iter().chain(b).chain(c)
    }

    /// The result, if every operand is a constant. Uses the interpreter's
    /// definition, so folding can never disagree with execution.
    pub fn fold(&self, width: Width) -> Option<u128> {
        if !self.operands().all(|v| matches!(v, Value::Const { .. })) {
            return None;
        }
        Some(interp::eval(width, self, &|v| v.as_const().unwrap_or(0)))
    }
}

/// Prefix kind for a [`Stmt::Repeat`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepKind {
    /// `rep`: run while ECX != 0.
    Rep,
    /// `repe`/`repz`: also stop after an iteration that clears ZF.
    RepE,
    /// `repne`/`repnz`: also stop after an iteration that sets ZF.
    RepNe,
}

/// A guest-visible exception an instruction raises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fault {
    /// `#DE`: divide by zero, or a quotient that does not fit.
    DivideError,
    /// `int3`. The saved EIP is the *next* instruction (`pc + len`), unlike
    /// most faults; consumers report both.
    Breakpoint,
    /// `#UD`, including `ud2`.
    InvalidOpcode,
    /// `int n` for some other vector. Saved EIP is the next instruction.
    Interrupt(u8),
    /// `into` with OF set.
    Overflow,
    /// `bound` with an index out of range.
    BoundRange,
    /// `#GP(0)`. Raised for a misaligned 16-byte SSE operand; other
    /// causes (privilege, segment limits) are not modelled.
    GeneralProtection,
    /// `#XM`: an unmasked SIMD floating-point exception. MXCSR's flags
    /// record it; the destination is not written.
    SimdException,
    /// `#MF` (vector 16): a waiting x87 instruction, an MMX instruction or
    /// `emms` found an unmasked x87 exception pending (status word ES set).
    /// Nothing changed; the event's argument is the x87 status word.
    FpError,
}

/// SSE floating-point operations, one per instruction form, executed by
/// [`Intrinsic::Sse`]. MXCSR-exact: rounding control, sticky flags, FTZ
/// and the x86 NaN rules are the reference semantics' business (the
/// `simd` crate and its C twin `h2simd.h`); an unmasked exception is
/// [`Fault::SimdException`] with MXCSR's flags set and no result. Operand
/// shapes, by [`SseOp::signature`]:
///
/// - scalar arithmetic `*Ss`: `(a: W128, b: W32) -> W128`, `a` the
///   destination register (lanes 1-3 come back unchanged), `b` the source
///   lane or m32;
/// - packed arithmetic `*Ps`: `(a: W128, b: W128) -> W128`; the unary ones
///   (`SqrtPs`, `RcpPs`, `RsqrtPs`) take only `b`;
/// - `CmpSs`/`CmpPs`: as their arithmetic shape plus `pred: W8` (imm8 & 7);
/// - `Comiss`/`Ucomiss`: `(a: W32, b: W32) -> W32`, the EFLAGS image of
///   ZF, PF and CF (bits 6, 2, 0);
/// - `Cvtsi2ss`: `(a: W128, i: W32) -> W128`; `Cvtss2si`/`Cvttss2si`:
///   `(a: W32) -> W32`; `Cvtpi2ps`: `(a: W128, mm: W64) -> W128` (lanes
///   2-3 kept); `Cvtps2pi`/`Cvttps2pi`: `(lanes 0-1: W64) -> W64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SseOp {
    AddSs,
    AddPs,
    SubSs,
    SubPs,
    MulSs,
    MulPs,
    DivSs,
    DivPs,
    SqrtSs,
    SqrtPs,
    MinSs,
    MinPs,
    MaxSs,
    MaxPs,
    RcpSs,
    RcpPs,
    RsqrtSs,
    RsqrtPs,
    CmpSs,
    CmpPs,
    Comiss,
    Ucomiss,
    Cvtsi2ss,
    Cvtss2si,
    Cvttss2si,
    Cvtpi2ps,
    Cvtps2pi,
    Cvttps2pi,
}

impl SseOp {
    /// (argument widths, result widths).
    pub fn signature(self) -> (&'static [Width], &'static [Width]) {
        use SseOp::*;
        use Width::*;
        match self {
            AddSs | SubSs | MulSs | DivSs | SqrtSs | MinSs | MaxSs | RcpSs | RsqrtSs | Cvtsi2ss => (&[W128, W32], &[W128]),
            AddPs | SubPs | MulPs | DivPs | MinPs | MaxPs => (&[W128, W128], &[W128]),
            SqrtPs | RcpPs | RsqrtPs => (&[W128], &[W128]),
            CmpSs => (&[W128, W32, W8], &[W128]),
            CmpPs => (&[W128, W128, W8], &[W128]),
            Comiss | Ucomiss => (&[W32, W32], &[W32]),
            Cvtss2si | Cvttss2si => (&[W32], &[W32]),
            Cvtpi2ps => (&[W128, W64], &[W128]),
            Cvtps2pi | Cvttps2pi => (&[W64], &[W64]),
        }
    }

    /// Lower-case mnemonic: the reference function's name, and the C
    /// helpers `h2s_<name>` (`h2simd.h`) and `h2i_sse_<name>` (`h2gen.h`).
    pub fn name(self) -> &'static str {
        use SseOp::*;
        match self {
            AddSs => "addss",
            AddPs => "addps",
            SubSs => "subss",
            SubPs => "subps",
            MulSs => "mulss",
            MulPs => "mulps",
            DivSs => "divss",
            DivPs => "divps",
            SqrtSs => "sqrtss",
            SqrtPs => "sqrtps",
            MinSs => "minss",
            MinPs => "minps",
            MaxSs => "maxss",
            MaxPs => "maxps",
            RcpSs => "rcpss",
            RcpPs => "rcpps",
            RsqrtSs => "rsqrtss",
            RsqrtPs => "rsqrtps",
            CmpSs => "cmpss",
            CmpPs => "cmpps",
            Comiss => "comiss",
            Ucomiss => "ucomiss",
            Cvtsi2ss => "cvtsi2ss",
            Cvtss2si => "cvtss2si",
            Cvttss2si => "cvttss2si",
            Cvtpi2ps => "cvtpi2ps",
            Cvtps2pi => "cvtps2pi",
            Cvttps2pi => "cvttps2pi",
        }
    }
}

/// Floating-point state operations, executed by [`Stmt::Intrinsic`], and
/// the time-stamp counter. See the crate docs for the FP state they act on.
/// `args`/`dsts` widths are given by [`Intrinsic::signature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Intrinsic {
    /// `fnstcw`: `dsts[0]` (`W16`) = control word.
    X87ReadControl,
    /// `fnstsw`: `dsts[0]` (`W16`) = status word (TOP is in it).
    X87ReadStatus,
    /// `stmxcsr`: `dsts[0]` (`W32`) = MXCSR.
    MxcsrRead,
    /// `ldmxcsr`: MXCSR = `args[0]` (`W32`); `#GP` if a bit the guest's
    /// Pentium III reserves is set ([`interp::MXCSR_MASK`]: bits 16-31 and
    /// DAZ, bit 6), with MXCSR unchanged.
    MxcsrWrite,
    /// The x87-to-MMX transition every instruction touching an MMX register
    /// makes: TOP (status bits 11-13) = 0, every tag valid (tag word 0).
    /// It comes after the instruction's faulting accesses; the pending-#MF
    /// check comes first, as [`Intrinsic::X87Wait`].
    MmxEnter,
    /// `emms`: [`Fault::FpError`] if an x87 exception is pending, else
    /// every tag empty (tag word 0xFFFF).
    Emms,
    /// `fwait`, the check every MMX instruction makes before anything
    /// else, and the one every waiting x87 instruction makes before reading
    /// a memory source: [`Fault::FpError`] if an unmasked x87 exception is
    /// pending (status word ES, bit 7). Changes nothing.
    X87Wait,
    /// An SSE floating-point operation; see [`SseOp`].
    Sse(SseOp),
    /// `rdtsc`: `dsts[0]` (`W64`) = the time-stamp counter. It belongs to
    /// the runtime, not to the instruction stream: the C backend asks the
    /// runtime for it (`h2shim_v1_rdtsc`), which derives it from guest time
    /// at the Xbox CPU's clock and never returns the same value twice. The
    /// interpreter's [`interp::Cpu::tsc`] is a stand-in that counts reads.
    ReadTsc,
    /// An x87 operation through the `crates/x87` helpers; see [`x87`].
    /// `fninit`, `fnclex` and `fldcw` are here too (their helpers keep the
    /// control word normalised and ES/B consistent).
    X87(x87::X87Op),
}

impl Intrinsic {
    /// (argument widths, result widths).
    pub fn signature(self) -> (&'static [Width], &'static [Width]) {
        use Intrinsic::*;
        match self {
            X87ReadControl | X87ReadStatus => (&[], &[Width::W16]),
            MxcsrRead => (&[], &[Width::W32]),
            MxcsrWrite => (&[Width::W32], &[]),
            MmxEnter | Emms | X87Wait => (&[], &[]),
            Sse(op) => op.signature(),
            ReadTsc => (&[], &[Width::W64]),
            X87(op) => op.signature(),
        }
    }

    /// Lower-case name; the emitter's C helper is `h2i_<name>`, except for
    /// [`Intrinsic::X87`], which the emitter lowers to `h2x87_v1_*` calls.
    pub fn name(self) -> &'static str {
        use Intrinsic::*;
        match self {
            MmxEnter => "mmx_enter",
            Emms => "emms",
            X87Wait => "x87_wait",
            Sse(op) => match op {
                SseOp::AddSs => "sse_addss",
                SseOp::AddPs => "sse_addps",
                SseOp::SubSs => "sse_subss",
                SseOp::SubPs => "sse_subps",
                SseOp::MulSs => "sse_mulss",
                SseOp::MulPs => "sse_mulps",
                SseOp::DivSs => "sse_divss",
                SseOp::DivPs => "sse_divps",
                SseOp::SqrtSs => "sse_sqrtss",
                SseOp::SqrtPs => "sse_sqrtps",
                SseOp::MinSs => "sse_minss",
                SseOp::MinPs => "sse_minps",
                SseOp::MaxSs => "sse_maxss",
                SseOp::MaxPs => "sse_maxps",
                SseOp::RcpSs => "sse_rcpss",
                SseOp::RcpPs => "sse_rcpps",
                SseOp::RsqrtSs => "sse_rsqrtss",
                SseOp::RsqrtPs => "sse_rsqrtps",
                SseOp::CmpSs => "sse_cmpss",
                SseOp::CmpPs => "sse_cmpps",
                SseOp::Comiss => "sse_comiss",
                SseOp::Ucomiss => "sse_ucomiss",
                SseOp::Cvtsi2ss => "sse_cvtsi2ss",
                SseOp::Cvtss2si => "sse_cvtss2si",
                SseOp::Cvttss2si => "sse_cvttss2si",
                SseOp::Cvtpi2ps => "sse_cvtpi2ps",
                SseOp::Cvtps2pi => "sse_cvtps2pi",
                SseOp::Cvttps2pi => "sse_cvttps2pi",
            },
            X87ReadControl => "x87_read_control",
            X87ReadStatus => "x87_read_status",
            X87(_) => "x87",
            MxcsrRead => "mxcsr_read",
            MxcsrWrite => "mxcsr_write",
            ReadTsc => "read_tsc",
        }
    }
}

/// A pure computation of flag values from captured inputs, shared by every
/// [`Stmt::LazyFlags`] of the same shape. Parameter `i` is the temp with id
/// `i` and width `params[i]`; `body` assigns further temps (ids from
/// `params.len()`, each once, before use); `flags` are the `W1` results.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlagFormula {
    pub params: Vec<Width>,
    pub body: Vec<(Temp, Expr)>,
    pub flags: Vec<(Flag, Value)>,
}

impl FlagFormula {
    /// The flag values for `args` (one per parameter).
    pub fn eval(&self, args: &[u128]) -> Vec<(Flag, bool)> {
        let mut temps: Vec<u128> = args.iter().zip(&self.params).map(|(v, w)| v & w.mask()).collect();
        let get = |temps: &Vec<u128>, v: &Value| match v {
            Value::Const { bits, .. } => *bits,
            Value::Temp(t) => temps[t.id as usize],
        };
        for (t, e) in &self.body {
            let v = interp::eval(t.width, e, &|v| get(&temps, v));
            debug_assert_eq!(temps.len(), t.id as usize);
            temps.push(v & t.width.mask());
        }
        self.flags.iter().map(|(f, v)| (*f, get(&temps, v) != 0)).collect()
    }

    /// The EFLAGS bits this formula writes.
    pub fn mask(&self) -> u32 {
        self.flags.iter().map(|(f, _)| 1u32 << f.eflags_bit()).fold(0, |a, b| a | b)
    }
}

/// Flags written by a formula over values captured here ([`Stmt::LazyFlags`]).
///
/// Executing it is executing `WriteFlag { flag, Defined(value) }` for each
/// of the formula's flags, at this point; nothing else is promised. What
/// makes it worth having is where the optimiser puts it: on writes whose
/// flags are, at most, read by a `ReadFlag` later in the same block (a
/// branch on `ZF`, an `adc` on `CF`) and otherwise only observed by exits
/// from the handler. A backend may keep the inputs and compute the flags
/// late, on every exit path and before every read (`ReadFlag`,
/// `ReadEflags`, `Repeat`, the spill before an atomic instruction); a read
/// of one flag can take that flag alone from the formula.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LazyFlags {
    pub formula: std::sync::Arc<FlagFormula>,
    /// One per parameter, at the parameter's width.
    pub args: Vec<Value>,
}

/// One effect or computation in an instruction's semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    /// `dst = expr`.
    Assign { dst: Temp, expr: Expr },
    /// `dst = reg[slice]`; `dst.width == slice.width()`.
    ReadReg { dst: Temp, reg: Gpr, slice: Slice },
    /// Write `value` into `reg[slice]`, preserving the other bits of the
    /// register (a partial-register merge for anything but [`Slice::Full`]).
    WriteReg { reg: Gpr, slice: Slice, value: Value },
    /// `dst` (`W1`) = the flag's current value.
    ReadFlag { dst: Temp, flag: Flag },
    /// Set or undefine one flag.
    WriteFlag { flag: Flag, value: FlagValue },
    /// `dst` (`W32`) = the whole EFLAGS register, for `pushfd`/`lahf`.
    /// Undefined flags read as whatever deterministic value was stored.
    ReadEflags { dst: Temp },
    /// Write the bits of `value` (`W32`) selected by `mask` into EFLAGS.
    /// Bits outside `mask` are preserved. Used by `popfd`/`sahf`.
    WriteEflags { value: Value, mask: u32 },
    /// `dst` (`W128`) = XMM register `reg` (0-7), all 128 bits. Scalar and
    /// half-register forms are explicit bit operations on the full value,
    /// so preserved lanes are visible in the IR.
    ReadXmm { dst: Temp, reg: u8 },
    /// Replace all 128 bits of XMM register `reg` with `value` (`W128`).
    WriteXmm { reg: u8, value: Value },
    /// `dst` (`W64`) = MMX register `reg` (0-7): the 64-bit significand of
    /// x87 physical register `reg` (`X87Abi.st[reg]` bytes 0-7).
    ReadMmx { dst: Temp, reg: u8 },
    /// MMX register `reg` = `value` (`W64`): `st[reg]` bytes 0-7 = value,
    /// bytes 8-9 (sign and exponent) = 0xFFFF, as hardware does. The tag
    /// and TOP changes are [`Intrinsic::MmxEnter`]'s.
    WriteMmx { reg: u8, value: Value },
    /// Little-endian load of `dst.width` bytes. May fault.
    Load { dst: Temp, addr: Addr },
    /// Little-endian store of `value.width()` bytes. May fault.
    Store { addr: Addr, value: Value },
    /// Raise `fault` if `cond` (`W1`) is true. The instruction stops here.
    /// For the trap-style faults ([`Fault::Breakpoint`], [`Fault::Interrupt`],
    /// [`Fault::Overflow`]) the saved EIP is the next instruction.
    FaultIf { cond: Value, fault: Fault },
    /// Stop guest execution with `reason` if `cond` (`W1`) is true, with
    /// every earlier effect applied (used for guest requests the recompiler
    /// does not model, such as `popfd` setting TF).
    StopIf { cond: Value, reason: StopReason },
    /// Run `then` if `cond` (`W1`) is true, else `otherwise`. Temps defined
    /// in either are not visible after it. Used where hardware does nothing
    /// at all for some operand values (a shift by a CL of zero: no store,
    /// no flag change), and where a flag is defined only for some (OF after
    /// a shift by CL is defined only for a count of one).
    If { cond: Value, then: Vec<Stmt>, otherwise: Vec<Stmt> },
    /// A floating-point state operation; see [`Intrinsic`]. May fault (the
    /// interpreter and the C helper raise the same [`Fault`]).
    Intrinsic { op: Intrinsic, args: Vec<Value>, dsts: Vec<Temp> },
    /// A REP-prefixed string instruction. Semantics:
    ///
    /// ```text
    /// while ECX != 0 {
    ///     body                  // one complete iteration
    ///     ECX = ECX - 1
    ///     if kind == RepE  && ZF == 0 { break }
    ///     if kind == RepNe && ZF == 1 { break }
    /// }
    /// ```
    ///
    /// Each iteration commits fully, so a fault inside `body` leaves the
    /// completed iterations' effects (ESI/EDI/ECX progress) in place and
    /// the instruction restartable at its own PC, as on hardware. EFLAGS
    /// are the exception: any exit from inside `body` (fault or stop)
    /// restores EFLAGS and the defined mask to their values when the
    /// statement began, which is the instruction's pre-state (nothing writes
    /// a flag before a `Repeat`). A faulting `repe cmpsb` reports the flags
    /// it started with, not the last completed comparison's. The ECX test
    /// and decrement are part of this statement, not of `body`.
    ///
    /// The counter is always ECX: the lifter refuses 16-bit address-size
    /// string ops (CX counter) rather than parameterising this. Between
    /// iterations the architectural state is complete and EIP is still the
    /// instruction's own PC, so a backend may yield or check for
    /// interruption there (a long `rep movsd` need not run to completion
    /// in one go): resuming at the PC continues where it stopped.
    Repeat { kind: RepKind, body: Vec<Stmt> },
    /// Write the formula's flags, which a backend may compute late; see
    /// [`LazyFlags`]. Only the optimiser produces this.
    LazyFlags(LazyFlags),
    /// An instruction (or form of one) the lifter does not implement. It is
    /// a stop with diagnostics: executing it must not silently continue.
    /// `mnemonic` is the decoder's name for it, `detail` says what is
    /// missing. The instruction's control is always
    /// [`Control::Stop`]`(`[`StopReason::Unsupported`]`)`.
    Unsupported { mnemonic: String, detail: String },
}

/// A control-flow target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A guest address known at lift time.
    Direct(u32),
    /// A `W32` value computed by the instruction (register, loaded pointer).
    Dynamic(Value),
}

/// What kind of call or jump a [`Control::Call`] / [`Control::Jump`] is,
/// from analysis. Advisory: the target value is always evaluated from guest
/// state, so a rewritten import slot or vtable is observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// `call rel32` / `jmp rel`.
    Direct,
    /// `call r/m32` / `jmp r/m32` through a pointer analysis knows nothing
    /// about.
    Indirect,
    /// `call [slot]` / `jmp [slot]` through a kernel import slot.
    Kernel { slot: u32, ordinal: u32 },
}

/// A direct control-flow destination: a block of this function, or (when
/// `tail`) another function's entry, reached by an entry-routed transfer
/// so its override applies (`docs/LIFTER.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dest {
    pub pc: u32,
    pub tail: bool,
}

impl Dest {
    pub const fn local(pc: u32) -> Dest {
        Dest { pc, tail: false }
    }
}

/// What a [`Control::Switch`]'s table looked like to analysis, and so how
/// far its `cases` can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchShape {
    /// A guard bounds the index: `entries` table entries.
    Bounded { entries: u32 },
    /// A byte table (`index_entries` bytes at `index_table`) selects the
    /// entry; the guard bounds the byte index, not the entry.
    Indexed { index_table: u32, index_entries: u32 },
    /// No guard: `cases` is what the table held before an entry stopped
    /// pointing at code. A heuristic, not a bound.
    Unbounded,
}

/// Why a [`Control::Stop`] or [`Stmt::StopIf`] ends guest execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `hlt`.
    Halt,
    /// An indirect jump whose target analysis could not bound. The runner
    /// should resolve it like a tail call or stop with diagnostics; this
    /// variant is for sites the lifter knows it cannot handle at all.
    UnknownTarget,
    /// The instruction has a [`Stmt::Unsupported`]; never reached past it.
    Unsupported,
    /// The CFG descent could not follow control past this instruction (the
    /// next bytes don't decode, run into another instruction, or leave the
    /// code sections). See [`IrFunction::errors`].
    DescentError,
    /// The guest asked for machine state the recompiler does not model
    /// (`popfd` changing TF, IF, IOPL or NT).
    Unmodelled,
}

/// Where execution goes after an instruction. Stack effects (pushing the
/// return address, popping it, `ret n` cleanup) are already in the
/// instruction's [`Stmt`]s; controls only describe the transfer, so a
/// consumer never applies them twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Fall through to `pc + len`, which (at the end of a block) is a block
    /// of the same function. Falling into another function's entry is a
    /// tail [`Control::Jump`] instead.
    Next,
    /// Unconditional jump. `tail` is true when the target is outside the
    /// owning function (a tail transfer the runner dispatches), false for a
    /// local label. `kind` says what analysis knows about the target
    /// (a `jmp [slot]` through a kernel import is
    /// [`CallKind::Kernel`]).
    Jump { target: Target, tail: bool, kind: CallKind },
    /// Conditional branch on a `W1` value: `taken` if true, else `fallthrough`.
    Branch { cond: Value, taken: Dest, fallthrough: Dest },
    /// A table jump analysis bounded. `target` is the address actually
    /// loaded from guest memory; `cases` are the validated targets (in table
    /// order, duplicates kept). A target outside `cases` is an
    /// unsupported-target stop, never a fallback to the nearest label.
    /// `immutable` is true when the table lies in a read-only section, so
    /// the image's entries are the only ones the guest can load.
    Switch { target: Value, table: u32, shape: SwitchShape, immutable: bool, cases: Vec<Dest> },
    /// Call: the return address `continuation` has been pushed. `returns`
    /// is analysis's belief that control can come back to `continuation`;
    /// it is advisory (a guest may still return there) and false for calls
    /// to known non-returning functions such as `KeBugCheck`.
    Call { target: Target, continuation: u32, returns: bool, kind: CallKind },
    /// Return to `target`, already popped. `cleanup` is the `ret imm16`
    /// immediate, already added to ESP (recorded for diagnostics/ABI checks).
    Return { target: Value, cleanup: u16 },
    /// Raise an exception unconditionally (`int3`, `ud2`).
    Fault(Fault),
    /// Stop guest execution.
    Stop(StopReason),
}

/// The single operation an atomic instruction performs on its memory
/// operand. Read-modify-write tags equal the runtime ABI's `H2_ATOMIC_*`
/// values ([`AtomicOp::rmw_tag`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtomicOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Inc,
    Dec,
    Neg,
    Not,
    /// Bit test and set/reset/complement: `operand` is the bit offset, used
    /// modulo the operand width (a register offset's word displacement is
    /// already in the address).
    Bts,
    Btr,
    Btc,
    /// `xadd`: memory = old + operand, flags as `add`; `old` receives the
    /// old value.
    Xadd,
    /// `xchg`: memory = operand; `old` receives the old value; no flags.
    Xchg,
    /// `cmpxchg`: compare the accumulator (AL/AX/EAX by width) with memory;
    /// equal: memory = operand, else accumulator = memory. Flags as `cmp
    /// acc, mem`.
    Cmpxchg,
}

impl AtomicOp {
    /// `H2_ATOMIC_*` for the operations `h2abi_v1_atomic_rmw` performs
    /// (`xadd` is `ADD` returning the old value); `None` for `xchg` and
    /// `cmpxchg`, which have entry points of their own.
    pub const fn rmw_tag(self) -> Option<u32> {
        Some(match self {
            AtomicOp::Add | AtomicOp::Xadd => 1,
            AtomicOp::Sub => 3,
            AtomicOp::And => 5,
            AtomicOp::Or => 6,
            AtomicOp::Xor => 7,
            AtomicOp::Inc => 8,
            AtomicOp::Dec => 9,
            AtomicOp::Neg => 10,
            AtomicOp::Not => 11,
            AtomicOp::Bts => 12,
            AtomicOp::Btr => 13,
            AtomicOp::Btc => 14,
            AtomicOp::Xchg | AtomicOp::Cmpxchg => return None,
        })
    }

    /// Whether the operation takes a value operand.
    pub const fn has_operand(self) -> bool {
        !matches!(self, AtomicOp::Inc | AtomicOp::Dec | AtomicOp::Neg | AtomicOp::Not)
    }

    /// Whether the old memory value goes to a register.
    pub const fn returns_old(self) -> bool {
        matches!(self, AtomicOp::Xadd | AtomicOp::Xchg)
    }
}

/// An atomic instruction as one operation. The memory operand's address
/// and width are those of the instruction's single [`Stmt::Load`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Atomic {
    pub op: AtomicOp,
    /// The value operand at the memory operand's width, computed by the
    /// pure prefix (or a constant); `Some` exactly when
    /// [`AtomicOp::has_operand`].
    pub operand: Option<Value>,
    /// The register slice that receives the old memory value (`xadd`
    /// source, `xchg` register); `Some` exactly when
    /// [`AtomicOp::returns_old`].
    pub old: Option<(Gpr, Slice)>,
}

/// One lowered guest instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrInsn {
    /// Guest address of the instruction.
    pub pc: u32,
    /// Encoded length; `pc + len` is the architectural next EIP.
    pub len: u8,
    /// Set when the memory access is one atomic read-modify-write (LOCK
    /// prefix, or `xchg` with memory, which locks implicitly); see the
    /// crate docs for the statement shape this implies.
    pub atomic: Option<Atomic>,
    pub stmts: Vec<Stmt>,
    pub control: Control,
}

impl IrInsn {
    /// The architectural next EIP.
    pub fn next_pc(&self) -> u32 {
        self.pc.wrapping_add(self.len as u32)
    }

    /// Whether any statement (at any depth) is [`Stmt::Unsupported`].
    pub fn is_supported(&self) -> bool {
        self.unsupported().is_none()
    }

    /// The first unsupported statement, as `(mnemonic, detail)`.
    pub fn unsupported(&self) -> Option<(&str, &str)> {
        fn find(stmts: &[Stmt]) -> Option<(&str, &str)> {
            stmts.iter().find_map(|s| match s {
                Stmt::Unsupported { mnemonic, detail } => Some((mnemonic.as_str(), detail.as_str())),
                Stmt::Repeat { body, .. } => find(body),
                Stmt::If { then, otherwise, .. } => find(then).or_else(|| find(otherwise)),
                _ => None,
            })
        }
        find(&self.stmts)
    }

    /// Which flags the instruction defines or undefines. For flags written
    /// on only some paths (inside a [`Stmt::Repeat`] or [`Stmt::If`], or through
    /// [`Stmt::WriteEflags`]), this is the union of possibilities: a flag in
    /// `defined` may still be preserved at run time when the loop runs zero
    /// times.
    pub fn flag_effects(&self) -> FlagEffects {
        fn walk(stmts: &[Stmt], fx: &mut FlagEffects) {
            for s in stmts {
                match s {
                    Stmt::WriteFlag { flag, value: FlagValue::Defined(_) } => fx.defined.insert(*flag),
                    Stmt::WriteFlag { flag, value: FlagValue::Undefined } => fx.undefined.insert(*flag),
                    Stmt::WriteEflags { mask, .. } => {
                        for f in Flag::ALL {
                            if mask & (1 << f.eflags_bit()) != 0 {
                                fx.defined.insert(f);
                            }
                        }
                    }
                    Stmt::Repeat { body, .. } => walk(body, fx),
                    Stmt::If { then, otherwise, .. } => {
                        walk(then, fx);
                        walk(otherwise, fx);
                    }
                    Stmt::LazyFlags(l) => {
                        for (f, _) in &l.formula.flags {
                            fx.defined.insert(*f);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut fx = FlagEffects::default();
        walk(&self.stmts, &mut fx);
        fx
    }
}

/// A basic block: instructions in address order, all but the last with
/// [`Control::Next`] — or with [`Control::Stop`]`(`[`StopReason::Unsupported`]`)`,
/// which may sit anywhere (blocks come from the CFG, which knows nothing of
/// lowering gaps); the instructions after it are then unreachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrBlock {
    pub start: u32,
    pub insns: Vec<IrInsn>,
}

impl IrBlock {
    pub fn control(&self) -> Option<&Control> {
        self.insns.last().map(|i| &i.control)
    }
}

/// A lifted guest function: one owner of a set of blocks reachable from
/// `entry` without passing through another function's entry (ownership is
/// reachability, so two functions may still contain copies of shared code
/// that is not itself an entry; see [`canonical_owner`] for which copy
/// answers external entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrFunction {
    pub entry: u32,
    pub blocks: std::collections::BTreeMap<u32, IrBlock>,
    /// Address-taken labels inside the function that the runner may enter
    /// at directly; each starts a block.
    pub interior_entries: Vec<u32>,
    /// Where the CFG descent gave up. Each block that ends at one finishes
    /// with [`Control::Stop`]`(`[`StopReason::DescentError`]`)`.
    pub errors: Vec<DescentError>,
}

/// Something the CFG descent couldn't follow (mirrors the analyzer's
/// `CfgError`, which this crate does not depend on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescentError {
    /// Bytes at this address don't decode.
    Decode(u32),
    /// Control reaches this address, which isn't in a code section.
    OutsideCode(u32),
    /// A branch lands inside an instruction decoded from another path.
    IntoInsnBody(u32),
    /// The instruction here runs over the start of another one.
    OverlapsInsn(u32),
}

impl IrFunction {
    pub fn insns(&self) -> impl Iterator<Item = &IrInsn> {
        self.blocks.values().flat_map(|b| b.insns.iter())
    }
}

/// The canonical owner of `pc` among `owners` (entries of functions whose
/// blocks contain it): the function with the greatest entry at or below
/// `pc` (the one the code sits in by layout), or failing that the lowest
/// entry. Deterministic, and independent of the order owners are listed.
/// Continuation PCs and interior entries in the runtime's resume map are
/// registered once, against this owner; other copies of shared code exist
/// only for their own function's internal control flow.
pub fn canonical_owner(pc: u32, owners: impl IntoIterator<Item = u32>) -> Option<u32> {
    let mut below = None;
    let mut lowest = None;
    for o in owners {
        if o <= pc && below.is_none_or(|b| o > b) {
            below = Some(o);
        }
        if lowest.is_none_or(|l| o < l) {
            lowest = Some(o);
        }
    }
    below.or(lowest)
}
