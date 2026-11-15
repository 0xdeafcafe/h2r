//! x87 floating point in the IR: one [`X87Op`] per runtime helper call.
//!
//! The x87 stack is architectural state (the CPU's `X87`, the runtime's
//! `X87Abi`): eight physical 80-bit registers, TOP in status bits 11-13, the
//! full tag word, and the last-instruction pointers. No x87 value is ever a
//! temp. Each operation is a [`crate::Intrinsic::X87`] that names its
//! registers as the instruction does, `ST(i)` relative to TOP, and maps 1:1
//! onto one `h2x87_v1_*` helper of `crates/x87` (docs/FLOAT-ABI.md §3-§4).
//! The helpers never touch guest memory: the lifter reads memory operands
//! with ordinary [`crate::Stmt::Load`]s and passes the bits by value, and
//! writes stores with ordinary [`crate::Stmt::Store`]s.
//!
//! # Outcomes
//!
//! Every helper returns a status. The IR-level meaning, which the
//! interpreter and the C emitter both implement:
//!
//! - `OK`: the operation ran (an unmasked pre-computation exception may
//!   have changed only the status word: flag, ES, B).
//! - `MF`: ES was set on entry, nothing ran. The instruction raises
//!   [`crate::Fault::FpError`] (#MF, event 14) with the saved EIP at this,
//!   the waiting, instruction. FIP/FOP/FDP still describe the raiser, the
//!   last non-control x87 instruction before it.
//! - `UNMODELLED`: an unmasked OE/UE/PE, nothing ran. The instruction stops
//!   with [`crate::StopReason::Unmodelled`] (code 12).
//! - `SUPPRESS`, stores: an unmasked pre-computation fault; the store is
//!   not written, and the commit installs the status word without popping.
//!   [`X87Op::StoreConvert`] reports it as `write = 0`.
//! - `SUPPRESS`, compares: an unmasked pre-computation fault, already in
//!   the status word; C3/C2/C0, TOP and the tags are unchanged. The payload
//!   is the old C3/C2/C0 (or 0 with `to_flags`). [`X87Op::Cmp`] reports it
//!   as `suppressed = 1` and execution continues.
//!
//! # Instruction shapes the lifter produces
//!
//! - A waiting instruction with a memory source:
//!   [`crate::Intrinsic::X87Wait`], then the `Load`, then the operation, so
//!   a pending #MF wins over a page fault.
//! - A store: [`X87Op::StoreConvert`] (no state change), `If write { Store }`,
//!   [`X87Op::StoreCommit`]. A faulting write leaves the x87 state as it was.
//! - Every non-control instruction ends with [`X87Op::Mark`], which records
//!   FIP/FOP/FDP. A backend may hold them anywhere as long as `X87Abi` has
//!   them whenever the runtime or a helper that reads them can see it.
//! - `fcomi`-family compares: [`X87Op::Cmp`] with `to_flags`, then ZF/PF/CF
//!   from the payload (kept as they were if the compare was suppressed),
//!   OF/SF/AF cleared, all six defined.

use crate::Width;

/// FCS for every guest instruction: the Xbox kernel's flat ring-0 code
/// selector (NT's `KGDT_R0_CODE`). Unverified against hardware; observable
/// only through `fnsave`/`fnstenv` images.
pub const GUEST_CS: u16 = 0x0008;
/// FDS for a flat memory operand: the flat ring-0 data selector.
pub const GUEST_DS: u16 = 0x0010;

/// Helper statuses (`h2x87.h`): the top byte of every return value.
pub mod status {
    pub const OK: u32 = 0x0000_0000;
    pub const MF: u32 = 0x0100_0000;
    pub const UNMODELLED: u32 = 0x0200_0000;
    pub const SUPPRESS: u32 = 0x0300_0000;
    pub const MASK: u32 = 0xff00_0000;
}

/// Status-word ES (error summary): an unmasked exception is pending.
pub const SW_ES: u16 = 0x0080;
/// `h2x87_v1_cmp` payload bits with `to_flags` (EFLAGS positions).
pub const EFL_CF: u32 = 0x01;
pub const EFL_PF: u32 = 0x04;
pub const EFL_ZF: u32 = 0x40;

/// An x87 memory format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum X87Format {
    F32,
    F64,
    F80,
    I16,
    I32,
    I64,
    /// 18-digit packed BCD (`fbld`/`fbstp`), 10 bytes.
    Bcd,
}

impl X87Format {
    /// Width of the memory operand (the IR value carrying its bits).
    pub const fn width(self) -> Width {
        match self {
            X87Format::F32 | X87Format::I32 => Width::W32,
            X87Format::F64 | X87Format::I64 => Width::W64,
            X87Format::F80 | X87Format::Bcd => Width::W80,
            X87Format::I16 => Width::W16,
        }
    }

    /// Lower-case suffix of the helper (`h2x87_v1_ld_f32`).
    pub const fn suffix(self) -> &'static str {
        match self {
            X87Format::F32 => "f32",
            X87Format::F64 => "f64",
            X87Format::F80 => "f80",
            X87Format::I16 => "i16",
            X87Format::I32 => "i32",
            X87Format::I64 => "i64",
            X87Format::Bcd => "bcd",
        }
    }
}

/// The second operand of an arithmetic or compare: a stack register or a
/// memory operand whose bits are the op's one argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum X87Src {
    St(u8),
    F32,
    F64,
    I16,
    I32,
}

impl X87Src {
    /// The helper's `src_kind` selector.
    pub const fn kind(self) -> u32 {
        match self {
            X87Src::St(_) => 0,
            X87Src::F32 => 1,
            X87Src::F64 => 2,
            X87Src::I16 => 3,
            X87Src::I32 => 4,
        }
    }

    /// Width of the memory operand, `None` for a register.
    pub const fn width(self) -> Option<Width> {
        match self {
            X87Src::St(_) => None,
            X87Src::F32 | X87Src::I32 => Some(Width::W32),
            X87Src::F64 => Some(Width::W64),
            X87Src::I16 => Some(Width::W16),
        }
    }
}

/// `h2x87_v1_arith` operations, in selector order. The destination is the
/// first operand: `Sub` is `dst - src`, `SubR` is `src - dst`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum X87Arith {
    Add,
    Sub,
    SubR,
    Mul,
    Div,
    DivR,
}

/// `h2x87_v1_unary` operations, in selector order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum X87Unary {
    Sqrt,
    Abs,
    Chs,
    RndInt,
    Sin,
    Cos,
    /// `fptan`: ST(0) = tan, then push 1.0.
    Tan,
    Patan,
    Yl2x,
    Yl2xp1,
    /// `f2xm1`.
    X2m1,
    Scale,
    Xtract,
    SinCos,
    Prem,
    Prem1,
}

/// `h2x87_v1_cmp` kinds: `fcom` (IE on a QNaN), `fucom`, `ficom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum X87CmpKind {
    Com,
    Ucom,
    Icom,
}

/// `h2x87_v1_ld_const` constants, in selector order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum X87Const {
    One,
    Z,
    Pi,
    L2E,
    L2T,
    Lg2,
    Ln2,
}

/// Environment operations, in selector order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum X87Env {
    /// `fnstenv`: 28-byte image, then mask every exception.
    StEnv,
    /// `fldenv`.
    LdEnv,
    /// `fnsave`: 108-byte image, then `fninit`.
    Save,
    /// `frstor`.
    Rstor,
}

impl X87Env {
    /// Image size in bytes (32-bit protected-mode layout).
    pub const fn bytes(self) -> u32 {
        match self {
            X87Env::StEnv | X87Env::LdEnv => 28,
            X87Env::Save | X87Env::Rstor => 108,
        }
    }

    /// The image as IR values, lowest address first: 16-byte chunks, then
    /// an 8- and a 4-byte tail (28 = 16+8+4, 108 = 6*16+8+4).
    pub const fn chunks(self) -> &'static [Width] {
        use Width::*;
        match self {
            X87Env::StEnv | X87Env::LdEnv => &[W128, W64, W32],
            X87Env::Save | X87Env::Rstor => &[W128, W128, W128, W128, W128, W128, W64, W32],
        }
    }
}

/// One x87 helper operation. Argument and result widths are given by
/// [`X87Op::signature`]; register numbers are `ST(i)` indices (0-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum X87Op {
    /// Push a memory value: `args[0]` holds its bits (`ld_<fmt>`).
    Load(X87Format),
    /// Push a constant (`ld_const`).
    LoadConst(X87Const),
    /// Push a copy of `ST(i)` (`ld_st`).
    LoadReg(u8),
    /// Phase 1 of a store (`st_<fmt>`): convert `ST(src)` without changing
    /// any state. Results: the bits to write, the status word the commit
    /// installs, and `write` (`W1`): 1 on `OK`, 0 on `SUPPRESS`.
    StoreConvert {
        to: X87Format,
        src: u8,
    },
    /// Phase 3 (`store_commit`): `args` = (status word, write). Installs the
    /// status word and pops when `pop` and `write` both hold.
    StoreCommit {
        pop: bool,
    },
    /// `fst`/`fstp ST(i)` (`st_st`).
    StoreReg {
        dst: u8,
        pop: bool,
    },
    /// `ST(dst) = ST(dst) op src`, then pop if asked (`arith`). Memory
    /// sources take their bits as `args[0]`; `dst` is then 0.
    Arith {
        op: X87Arith,
        dst: u8,
        src: X87Src,
        pop: bool,
    },
    Unary(X87Unary),
    /// Compare `ST(0)` with `src`, pop `pop` (0-2) times (`cmp`). Results:
    /// the payload (`W32`: C3|C2|C0 as status-word bits, or ZF|PF|CF with
    /// `to_flags`) and `suppressed` (`W1`): the helper answered `SUPPRESS`
    /// (an unmasked exception; nothing compared or popped). `ftst` is `Com`
    /// against F32 +0.0.
    Cmp {
        kind: X87CmpKind,
        src: X87Src,
        pop: u8,
        to_flags: bool,
    },
    Xam,
    Xch(u8),
    /// `ffree ST(i)`, or `ffreep` with `pop`.
    Free {
        reg: u8,
        pop: bool,
    },
    IncStp,
    DecStp,
    /// `fcmovcc`: `args[0]` (`W1`) is the condition, evaluated by the lifter.
    Cmov(u8),
    /// `fldcw`: `args[0]` (`W16`); the helper normalises it.
    LoadCw,
    Clex,
    Init,
    /// `fnstenv`/`fnsave` phase 1: the image, as [`X87Env::chunks`]
    /// results. Reads FIP/FOP/FDP, so a backend that caches them commits
    /// them first.
    SaveImage(X87Env),
    /// Phase 3: mask everything (`fnstenv`) or `fninit` (`fnsave`).
    SaveCommit(X87Env),
    /// `fldenv`/`frstor` from the image in `args` ([`X87Env::chunks`]).
    LoadEnv(X87Env),
    /// Record this instruction as the last non-control x87 instruction:
    /// FIP = its PC, FCS = [`GUEST_CS`], FOP = `fop`; with `mem`, FDP =
    /// `args[0]` (`W32`, the operand's offset) and FDS = [`GUEST_DS`].
    /// Never fails, and calls no helper.
    Mark {
        fop: u16,
        mem: bool,
    },
    /// The `_ftol2` CRT override: truncate `ST(0)` to an int64 (result,
    /// `W64`) and pop.
    Ftol2,
}

impl X87Op {
    /// (argument widths, result widths).
    pub fn signature(self) -> (&'static [Width], &'static [Width]) {
        use Width::*;
        fn one(w: Width) -> &'static [Width] {
            match w {
                W16 => &[W16],
                W32 => &[W32],
                W64 => &[W64],
                W80 => &[W80],
                _ => unreachable!("x87 operand width {w:?}"),
            }
        }
        match self {
            X87Op::Load(f) => (one(f.width()), &[]),
            X87Op::StoreConvert { to, .. } => (
                &[],
                match to.width() {
                    W16 => &[W16, W16, W1],
                    W32 => &[W32, W16, W1],
                    W64 => &[W64, W16, W1],
                    _ => &[W80, W16, W1],
                },
            ),
            X87Op::StoreCommit { .. } => (&[W16, W1], &[]),
            X87Op::Arith { src, .. } => (src.width().map_or(&[][..], one), &[]),
            X87Op::Cmp { src, .. } => (src.width().map_or(&[][..], one), &[W32, W1]),
            X87Op::Cmov(_) => (&[W1], &[]),
            X87Op::LoadCw => (&[W16], &[]),
            X87Op::SaveImage(e) => (&[], e.chunks()),
            X87Op::LoadEnv(e) => (e.chunks(), &[]),
            X87Op::Mark { mem: true, .. } => (&[W32], &[]),
            X87Op::Ftol2 => (&[], &[W64]),
            X87Op::LoadConst(_)
            | X87Op::LoadReg(_)
            | X87Op::StoreReg { .. }
            | X87Op::Unary(_)
            | X87Op::Xam
            | X87Op::Xch(_)
            | X87Op::Free { .. }
            | X87Op::IncStp
            | X87Op::DecStp
            | X87Op::Clex
            | X87Op::Init
            | X87Op::SaveCommit(_)
            | X87Op::Mark { mem: false, .. } => (&[], &[]),
        }
    }

    /// Whether the op calls a helper (everything but [`X87Op::Mark`]).
    pub const fn calls_helper(self) -> bool {
        !matches!(self, X87Op::Mark { .. })
    }
}
