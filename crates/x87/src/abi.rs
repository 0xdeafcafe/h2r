//! The x87 runtime helpers of docs/FLOAT-ABI.md §1-§4, operating on the
//! runtime's `X87Abi` (crates/runtime/src/cpu.rs, h2abi.h v1).
//!
//! The architectural logic is safe Rust on `&mut X87Abi` (the functions
//! without the `h2x87_v1_` prefix); [`ffi`] wraps each as the versioned
//! `extern "C"` symbol declared in `include/h2x87.h`.
//!
//! State: `st` holds the eight *physical* registers as guest 10-byte values;
//! `ST(i)` is `st[(TOP + i) & 7]` with TOP in `status` bits 11..13. There is
//! no other copy of TOP.
//!
//! Every helper returns a `u32` whose top byte is a status ([`status`]):
//!
//! * `OK` (0): the instruction completed. If it raised an exception the
//!   control word does not mask that is a *pre-computation* exception (IE,
//!   DE, ZE, or a stack fault), nothing but the status word changed: the
//!   flags, C1 for a stack fault, ES and B are set, and no register, tag,
//!   TOP or destination is written. The guest sees #MF at its next waiting
//!   instruction, exactly as on hardware. (Stores and [`cmp`] report this
//!   case as `SUPPRESS` instead.) Only the pre-computation flags are
//!   recorded: the suppressed computation raises no OE/UE/PE.
//! * `MF` (1): ES was already set when this waiting instruction started, so
//!   it did nothing; generated code raises event 14 (#MF) with
//!   `saved_eip` = this instruction and `arg` = the status word.
//! * `UNMODELLED` (2): the instruction raised an unmasked OE, UE or PE
//!   (post-computation exceptions, where hardware stores a bias-adjusted or
//!   rounded result before faulting). UE counts as raised for any tiny
//!   result when UM is clear, exact or not ([`Flags::TINY`]). Nothing
//!   changed; generated code stops with event 12. The game masks all of
//!   them.
//! * `SUPPRESS` (3): an unmasked pre-computation exception, from the two
//!   helpers whose caller must act differently:
//!   * stores: nothing changed yet. Do not write the destination; call
//!     [`store_commit`] with `pop = 0` to record the status word.
//!   * [`cmp`]: the status word is already recorded (flags, ES, B, C1 for a
//!     stack fault); C3/C2/C0, the stack and EFLAGS are as before. The low
//!     bits are the unchanged C3/C2/C0 for the status-word forms and 0 for
//!     the FCOMI forms, which must not merge ZF/PF/CF.
//!
//! The low 24 bits are zero except for [`cmp`], which returns the relation.
//! Instruction/data pointers and selectors are left alone: the emitter
//! materialises them (FLOAT-ABI.md §2.4).
#![deny(unsafe_code)]

use crate::arith::{add_impl, div_impl};
use crate::{
    Class, Cmp, Const, F80, Flags, Mode, Rc, compare, mul, patan, prem, prem1, rndint, scale, sin,
    sincos, sqrt, tan, x2m1, xtract, yl2x, yl2xp1,
};

/// `X87Abi` of h2abi.h v1: 100 bytes, align 4.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct X87Abi {
    /// Physical registers: little-endian significand, then sign/exponent.
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

/// Helper return statuses, in the top byte of the return value.
pub mod status {
    pub const OK: u32 = 0;
    pub const MF: u32 = 1 << 24;
    pub const UNMODELLED: u32 = 2 << 24;
    pub const SUPPRESS: u32 = 3 << 24;
    pub const MASK: u32 = 0xff00_0000;
}

/// The runtime event code generated code raises for `status::MF`
/// (`H2SHIM_FP_ERROR`, vector 16); `arg` is the status word.
pub const H2SHIM_FP_ERROR: u32 = 14;
/// The stop code for `status::UNMODELLED` (`H2SHIM_UNMODELLED`).
pub const H2SHIM_UNMODELLED: u32 = 12;

pub const H2_FP_IE: u16 = 0x0001;
pub const H2_FP_DE: u16 = 0x0002;
pub const H2_FP_ZE: u16 = 0x0004;
pub const H2_FP_OE: u16 = 0x0008;
pub const H2_FP_UE: u16 = 0x0010;
pub const H2_FP_PE: u16 = 0x0020;
pub const H2_FP_SF: u16 = 0x0040;

/// Status word bits.
pub const SW_ES: u16 = 0x0080;
pub const SW_C0: u16 = 0x0100;
pub const SW_C1: u16 = 0x0200;
pub const SW_C2: u16 = 0x0400;
pub const SW_C3: u16 = 0x4000;
pub const SW_B: u16 = 0x8000;
pub const SW_TOP: u16 = 0x3800;

/// Control-word bits that exist on a P6 (masks, PC, RC, the infinity bit);
/// bit 6 always reads as 1.
pub const CW_DEFINED: u16 = 0x1f3f;
pub const CW_ONE: u16 = 0x0040;

/// Tag values.
pub const TAG_VALID: u16 = 0;
pub const TAG_ZERO: u16 = 1;
pub const TAG_SPECIAL: u16 = 2;
pub const TAG_EMPTY: u16 = 3;

/// EFLAGS bits returned by the `fcomi` forms.
pub const EFL_CF: u32 = 0x0001;
pub const EFL_PF: u32 = 0x0004;
pub const EFL_ZF: u32 = 0x0040;

/// `h2x87_v1_unary` operation numbers, in `X87Unary` declaration order.
pub mod unary_op {
    pub const SQRT: u32 = 0;
    pub const ABS: u32 = 1;
    pub const CHS: u32 = 2;
    pub const RNDINT: u32 = 3;
    pub const SIN: u32 = 4;
    pub const COS: u32 = 5;
    pub const TAN: u32 = 6;
    pub const PATAN: u32 = 7;
    pub const YL2X: u32 = 8;
    pub const YL2XP1: u32 = 9;
    pub const X2M1: u32 = 10;
    pub const SCALE: u32 = 11;
    pub const XTRACT: u32 = 12;
    pub const SINCOS: u32 = 13;
    pub const PREM: u32 = 14;
    pub const PREM1: u32 = 15;
}

/// `h2x87_v1_arith` operation numbers. OR in [`ARITH_POP`] for the `...p`
/// forms.
pub mod arith_op {
    pub const ADD: u32 = 0;
    pub const SUB: u32 = 1;
    pub const SUBR: u32 = 2;
    pub const MUL: u32 = 3;
    pub const DIV: u32 = 4;
    pub const DIVR: u32 = 5;
}
pub const ARITH_POP: u32 = 0x100;

/// Source kinds for `arith` and `cmp`.
pub mod src_kind {
    pub const ST: u32 = 0;
    pub const F32: u32 = 1;
    pub const F64: u32 = 2;
    pub const I16: u32 = 3;
    pub const I32: u32 = 4;
}

/// Environment operations.
pub mod env_op {
    pub const FNSTENV: u32 = 0;
    pub const FLDENV: u32 = 1;
    pub const FNSAVE: u32 = 2;
    pub const FRSTOR: u32 = 3;
}

impl Default for X87Abi {
    /// The FNINIT state with zeroed registers (the runtime's default).
    fn default() -> Self {
        let mut s = X87Abi {
            st: [[0; 10]; 8],
            control: 0,
            status: 0,
            tag: 0,
            opcode: 0,
            instruction_pointer: 0,
            data_pointer: 0,
            instruction_selector: 0,
            data_selector: 0,
        };
        init(&mut s);
        s
    }
}

/// Tag for a nonempty register holding `v`.
pub fn tag_for(v: F80) -> u16 {
    match v.classify() {
        Class::Zero => TAG_ZERO,
        Class::Normal => TAG_VALID,
        _ => TAG_SPECIAL,
    }
}

/// `(cw & 0x1f3f) | 0x40`: what FLDCW/FLDENV/FRSTOR leave in the control
/// word on a P6.
pub const fn normalise_cw(cw: u16) -> u16 {
    (cw & CW_DEFINED) | CW_ONE
}

impl X87Abi {
    pub fn top(&self) -> u32 {
        ((self.status >> 11) & 7) as u32
    }
    fn set_top(&mut self, t: u32) {
        self.status = (self.status & !SW_TOP) | (((t & 7) as u16) << 11);
    }
    pub fn phys(&self, i: u32) -> usize {
        ((self.top() + i) & 7) as usize
    }
    pub fn tag_of(&self, p: usize) -> u16 {
        (self.tag >> (2 * p)) & 3
    }
    fn set_tag(&mut self, p: usize, t: u16) {
        self.tag = (self.tag & !(3 << (2 * p))) | (t << (2 * p));
    }
    /// Physical register `p` as a value, whatever its tag.
    pub fn reg(&self, p: usize) -> F80 {
        F80::from_bytes(self.st[p])
    }
    /// `ST(i)`, or `None` when that register is empty.
    pub fn st(&self, i: u32) -> Option<F80> {
        let p = self.phys(i);
        (self.tag_of(p) != TAG_EMPTY).then(|| self.reg(p))
    }
    /// Write `ST(i)` and its tag.
    pub fn set_st(&mut self, i: u32, v: F80) {
        let p = self.phys(i);
        self.st[p] = v.to_bytes();
        self.set_tag(p, tag_for(v));
    }
    fn mode(&self) -> Mode {
        Mode::from_cw(self.control)
    }
    fn rc(&self) -> Rc {
        Rc::from_cw(self.control)
    }
    fn push(&mut self, v: F80) {
        self.set_top(self.top() + 7);
        self.set_st(0, v);
    }
    fn pop(&mut self) {
        let p = self.phys(0);
        self.set_tag(p, TAG_EMPTY);
        self.set_top(self.top() + 1);
    }
    /// An unmasked exception is pending (ES set).
    pub fn pending(&self) -> bool {
        self.status & SW_ES != 0
    }
    fn push_overflows(&self) -> bool {
        self.st(7).is_some()
    }
}

const STACK_FAULT: Flags = Flags(Flags::IE.0 | Flags::SF.0);
const POST: u16 = H2_FP_OE | H2_FP_UE | H2_FP_PE;

/// Condition-code update applied on commit: bits in `mask` take `val`.
#[derive(Clone, Copy)]
struct Cc {
    mask: u16,
    val: u16,
}

impl Cc {
    const NONE: Cc = Cc { mask: 0, val: 0 };
    fn c1(set: bool) -> Cc {
        Cc::NONE.with(SW_C1, set)
    }
    /// C1 from a rounding: set iff the result was rounded up.
    fn round(f: Flags) -> Cc {
        Cc::c1(f.contains(Flags::C1))
    }
    fn with(self, mask: u16, set: bool) -> Cc {
        Cc {
            mask: self.mask | mask,
            val: (self.val & !mask) | if set { mask } else { 0 },
        }
    }
}

/// The status-word flags an unmasked pre-computation exception records:
/// the computation did not happen, so no OE/UE/PE from it.
const PRE: u16 = H2_FP_IE | H2_FP_DE | H2_FP_ZE | H2_FP_SF;

/// What an instruction's exceptions do, before anything is written.
enum Outcome {
    /// Masked or none: commit.
    Commit,
    /// Unmasked IE/DE/ZE/stack fault: status only.
    Suppress,
    /// Unmasked OE/UE/PE alone: stop, change nothing.
    Unmodelled,
}

fn outcome(s: &X87Abi, fl: Flags) -> Outcome {
    let mut raised = fl.exceptions();
    if fl.contains(Flags::TINY) {
        // Unmasked underflow is signalled on tininess alone (SDM Vol. 1
        // §4.9.1.5); masked, UE needs inexactness and is already in `fl`.
        raised |= H2_FP_UE;
    }
    let unmasked = raised & !(s.control & 0x3f);
    if unmasked == 0 {
        Outcome::Commit
    } else if unmasked & !POST == 0 {
        Outcome::Unmodelled
    } else {
        Outcome::Suppress
    }
}

/// Record `fl` and the condition codes; on [`Outcome::Suppress`] only the
/// pre-computation flags, ES and B (and C1 for a stack fault).
fn record(s: &mut X87Abi, fl: Flags, stack_c1: Option<bool>, cc: Cc, out: &Outcome) {
    if matches!(out, Outcome::Suppress) {
        s.status |= (fl.0 & PRE) | SW_ES | SW_B;
    } else {
        s.status |= fl.0 & 0x7f;
        s.status = (s.status & !cc.mask) | (cc.val & cc.mask);
    }
    if let Some(c1) = stack_c1 {
        s.status = (s.status & !SW_C1) | if c1 { SW_C1 } else { 0 };
    }
}

/// Apply `fl`: fault-free or masked → `commit` and the condition codes.
fn finish(
    s: &mut X87Abi,
    fl: Flags,
    stack_c1: Option<bool>,
    cc: Cc,
    commit: impl FnOnce(&mut X87Abi),
) -> u32 {
    let out = outcome(s, fl);
    match out {
        Outcome::Unmodelled => return status::UNMODELLED,
        Outcome::Commit => commit(s),
        Outcome::Suppress => {}
    }
    record(s, fl, stack_c1, cc, &out);
    status::OK
}

macro_rules! waiting {
    ($s:expr) => {
        if $s.pending() {
            return status::MF;
        }
    };
}

// ---------------------------------------------------------------- loads

fn push_value(s: &mut X87Abi, v: F80, fl: Flags, src_fault: bool) -> u32 {
    if s.push_overflows() {
        return finish(s, fl | STACK_FAULT, Some(true), Cc::NONE, |s| {
            s.push(F80::INDEFINITE)
        });
    }
    if src_fault {
        return finish(s, fl | STACK_FAULT, Some(false), Cc::NONE, |s| {
            s.push(F80::INDEFINITE)
        });
    }
    finish(s, fl, None, Cc::c1(false), |s| s.push(v))
}

fn f32_snan(b: u32) -> bool {
    (b >> 23) & 0xff == 0xff && b & 0x7f_ffff != 0 && b & 0x40_0000 == 0
}
fn f32_denormal(b: u32) -> bool {
    (b >> 23) & 0xff == 0 && b & 0x7f_ffff != 0
}
fn f64_snan(b: u64) -> bool {
    (b >> 52) & 0x7ff == 0x7ff && b & ((1 << 52) - 1) != 0 && b & (1 << 51) == 0
}
fn f64_denormal(b: u64) -> bool {
    (b >> 52) & 0x7ff == 0 && b & ((1 << 52) - 1) != 0
}

/// FLD m32: exact widening; an SNaN is quieted with IE, a denormal sets DE.
pub fn ld_f32(s: &mut X87Abi, bits: u32) -> u32 {
    waiting!(s);
    let mut v = F80::from_f32_bits(bits);
    let mut fl = Flags::NONE;
    if f32_snan(bits) {
        fl |= Flags::IE;
        v.sig |= 1 << 62;
    }
    if f32_denormal(bits) {
        fl |= Flags::DE;
    }
    push_value(s, v, fl, false)
}

/// FLD m64: as `ld_f32`.
pub fn ld_f64(s: &mut X87Abi, bits: u64) -> u32 {
    waiting!(s);
    let mut v = F80::from_f64_bits(bits);
    let mut fl = Flags::NONE;
    if f64_snan(bits) {
        fl |= Flags::IE;
        v.sig |= 1 << 62;
    }
    if f64_denormal(bits) {
        fl |= Flags::DE;
    }
    push_value(s, v, fl, false)
}

/// FLD m80: bit copy, no exceptions for any encoding.
pub fn ld_f80(s: &mut X87Abi, m: [u8; 10]) -> u32 {
    waiting!(s);
    push_value(s, F80::from_bytes(m), Flags::NONE, false)
}

/// FILD (all widths are exact).
pub fn ld_int(s: &mut X87Abi, v: i64) -> u32 {
    waiting!(s);
    push_value(s, F80::from_i64(v), Flags::NONE, false)
}

/// FBLD.
pub fn ld_bcd(s: &mut X87Abi, m: [u8; 10]) -> u32 {
    waiting!(s);
    push_value(s, F80::from_bcd(m), Flags::NONE, false)
}

/// FLD1/FLDZ/FLDPI/FLDL2E/FLDL2T/FLDLG2/FLDLN2 for `which` 0..=6
/// (`X87LoadConst` order), rounded by RC.
///
/// # Panics
/// On any other `which` (a generator bug; aborts through the C ABI).
pub fn ld_const(s: &mut X87Abi, which: u32) -> u32 {
    let c = match which {
        0 => Const::One,
        1 => Const::Z,
        2 => Const::Pi,
        3 => Const::L2E,
        4 => Const::L2T,
        5 => Const::Lg2,
        6 => Const::Ln2,
        _ => panic!("h2x87: bad load-constant selector {which}"),
    };
    waiting!(s);
    let rc = s.rc();
    push_value(s, F80::constant(c, rc), Flags::NONE, false)
}

/// FLD ST(i).
pub fn ld_st(s: &mut X87Abi, i: u32) -> u32 {
    waiting!(s);
    match s.st(i & 7) {
        Some(v) => push_value(s, v, Flags::NONE, false),
        None => push_value(s, F80::INDEFINITE, Flags::NONE, true),
    }
}

// --------------------------------------------------------------- stores

/// A converted store: the bytes to write and the status word to commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Store<T> {
    pub value: T,
    pub status: u16,
}

/// Phase 1 of a store: convert `ST(i)` (or produce the format's indefinite
/// on stack underflow) without changing the state. On `OK` the caller
/// writes `value` to guest memory and, if that succeeds, calls
/// [`store_commit`] with `pop`; on `SUPPRESS` it writes nothing and calls
/// [`store_commit`] with `pop = false`; on `MF`/`UNMODELLED` it does
/// neither. A failing guest write therefore leaves the x87 state untouched.
fn store<T: Copy>(
    s: &X87Abi,
    i: u32,
    indef: T,
    conv: impl FnOnce(F80, Mode) -> (T, Flags),
) -> (u32, Store<T>) {
    let none = Store {
        value: indef,
        status: s.status,
    };
    if s.pending() {
        return (status::MF, none);
    }
    let (value, fl, c1, cc) = match s.st(i & 7) {
        None => (indef, STACK_FAULT, Some(false), Cc::NONE),
        Some(v) => {
            let (val, fl) = conv(v, s.mode());
            (val, fl, None, Cc::round(fl))
        }
    };
    let out = outcome(s, fl);
    let code = match out {
        Outcome::Unmodelled => return (status::UNMODELLED, none),
        Outcome::Commit => status::OK,
        Outcome::Suppress => status::SUPPRESS,
    };
    let mut c = *s;
    record(&mut c, fl, c1, cc, &out);
    (
        code,
        Store {
            value,
            status: c.status,
        },
    )
}

/// Phase 3 of a store (and FNSTENV/FNSAVE): install the status word from
/// phase 1, then pop if asked.
pub fn store_commit(s: &mut X87Abi, sw: u16, pop: bool) {
    s.status = (sw & !SW_TOP) | (s.status & SW_TOP);
    if pop {
        s.pop();
    }
}

pub fn st_f32(s: &X87Abi, i: u32) -> (u32, Store<u32>) {
    store(s, i, 0xffc0_0000, |v, m| v.to_f32_bits(m))
}

pub fn st_f64(s: &X87Abi, i: u32) -> (u32, Store<u64>) {
    store(s, i, 0xfff8_0000_0000_0000, |v, m| v.to_f64_bits(m))
}

/// FST(P) m80: bit copy, no exceptions besides stack underflow.
pub fn st_f80(s: &X87Abi, i: u32) -> (u32, Store<[u8; 10]>) {
    store(s, i, F80::INDEFINITE.to_bytes(), |v, _| {
        (v.to_bytes(), Flags::NONE)
    })
}

pub fn st_i16(s: &X87Abi, i: u32) -> (u32, Store<i16>) {
    store(s, i, i16::MIN, |v, m| v.to_i16(m, false))
}

pub fn st_i32(s: &X87Abi, i: u32) -> (u32, Store<i32>) {
    store(s, i, i32::MIN, |v, m| v.to_i32(m, false))
}

pub fn st_i64(s: &X87Abi, i: u32) -> (u32, Store<i64>) {
    store(s, i, i64::MIN, |v, m| v.to_i64(m, false))
}

/// Truncating store, ignoring RC (not a P6 instruction: for CRT overrides).
pub fn st_i32_trunc(s: &X87Abi, i: u32) -> (u32, Store<i32>) {
    store(s, i, i32::MIN, |v, m| v.to_i32(m, true))
}

pub fn st_bcd(s: &X87Abi, i: u32) -> (u32, Store<[u8; 10]>) {
    store(s, i, crate::convert::BCD_INDEFINITE, |v, m| v.to_bcd(m.rc))
}

/// FST(P) ST(dst): copy ST(0) to ST(dst) exactly, then pop if asked.
pub fn st_st(s: &mut X87Abi, dst: u32, pop: bool) -> u32 {
    waiting!(s);
    let dst = dst & 7;
    let (v, fl, c1) = match s.st(0) {
        Some(v) => (v, Flags::NONE, None),
        None => (F80::INDEFINITE, STACK_FAULT, Some(false)),
    };
    let cc = if c1.is_none() {
        Cc::c1(false)
    } else {
        Cc::NONE
    };
    finish(s, fl, c1, cc, |s| {
        s.set_st(dst, v);
        if pop {
            s.pop();
        }
    })
}

/// The `_ftol2` override (CRT 0x322b0c): convert ST(0) to int64 by
/// truncation and pop it. NaN, infinity, unsupported, or |x| >= 2^63 give
/// 0x8000_0000_0000_0000 with IE; an inexact truncation sets PE. The
/// caller puts `*out` in EDX:EAX. Stack underflow: IE|SF, indefinite.
pub fn ftol2(s: &mut X87Abi, out: &mut u64) -> u32 {
    waiting!(s);
    let (v, fl, c1) = match s.st(0) {
        None => (i64::MIN, STACK_FAULT, Some(false)),
        Some(x) => {
            let (v, fl) = x.to_i64(Mode::EXTENDED, true);
            (v, fl, None)
        }
    };
    let cc = if c1.is_none() {
        Cc::round(fl)
    } else {
        Cc::NONE
    };
    finish(s, fl, c1, cc, |s| {
        *out = v as u64;
        s.pop();
    })
}

// ----------------------------------------------------------- arithmetic

/// A memory or register source operand: `(value, source was a denormal in
/// its own format, register was empty)`.
///
/// # Panics
/// On an unknown `kind`.
fn source(s: &X87Abi, kind: u32, src: u64) -> (F80, bool, bool) {
    match kind {
        src_kind::ST => match s.st(src as u32 & 7) {
            Some(v) => (v, false, false),
            None => (F80::INDEFINITE, false, true),
        },
        src_kind::F32 => (
            F80::from_f32_bits(src as u32),
            f32_denormal(src as u32),
            false,
        ),
        src_kind::F64 => (F80::from_f64_bits(src), f64_denormal(src), false),
        src_kind::I16 => (F80::from_i64(src as u16 as i16 as i64), false, false),
        src_kind::I32 => (F80::from_i64(src as u32 as i32 as i64), false, false),
        _ => panic!("h2x87: bad source kind {kind}"),
    }
}

/// DE for a memory operand that was denormal in its own format (it is
/// normal once widened), unless the operation raised a higher-priority IE
/// or ZE (SDM Vol. 1 §4.9.2) or a NaN decided it.
fn mem_de(fl: Flags, den: bool, other: F80) -> Flags {
    if den && !fl.contains(Flags::IE) && !fl.contains(Flags::ZE) && !other.is_nan() {
        fl | Flags::DE
    } else {
        fl
    }
}

/// FADD/FSUB/FSUBR/FMUL/FDIV/FDIVR and their `p` and `i` forms.
///
/// `op` is 0..=5 (add, sub, subr, mul, div, divr), OR [`ARITH_POP`] to pop
/// after. `dst_i` is the destination register; `src_kind` as in
/// [`src_kind`] (memory forms use `dst_i = 0`). PC and RC apply.
///
/// # Panics
/// On an unknown `op` or `src_kind`.
pub fn arith(s: &mut X87Abi, op: u32, dst_i: u32, kind: u32, src: u64) -> u32 {
    assert!(
        op & !ARITH_POP <= arith_op::DIVR,
        "h2x87: bad arith op {op}"
    );
    let (b, den, src_empty) = source(s, kind, src);
    waiting!(s);
    let pop = op & ARITH_POP != 0;
    let dst_i = dst_i & 7;
    let commit = |v: F80| {
        move |s: &mut X87Abi| {
            s.set_st(dst_i, v);
            if pop {
                s.pop();
            }
        }
    };
    let a = match s.st(dst_i) {
        Some(a) if !src_empty => a,
        _ => {
            return finish(
                s,
                STACK_FAULT,
                Some(false),
                Cc::NONE,
                commit(F80::INDEFINITE),
            );
        }
    };
    let m = s.mode();
    let (v, fl) = match op & 0xff {
        arith_op::ADD => add_impl(a, b, false, m, true),
        arith_op::SUB => add_impl(a, b, true, m, true),
        arith_op::SUBR => add_impl(b, a, true, m, false),
        arith_op::MUL => mul(a, b, m),
        arith_op::DIV => div_impl(a, b, m, true),
        _ => div_impl(b, a, m, false),
    };
    let fl = mem_de(fl, den, a);
    finish(s, fl, None, Cc::round(fl), commit(v))
}

/// The one- and two-operand instructions of `X87Unary`, numbered as in
/// [`unary_op`]. Operands are ST(0) and, where the instruction uses it,
/// ST(1).
///
/// # Panics
/// On an unknown `op`.
pub fn unary(s: &mut X87Abi, op: u32) -> u32 {
    unary_impl::<{ cfg!(feature = "fast-trans") }>(s, op)
}

#[cfg(test)]
pub(crate) fn unary_reference(s: &mut X87Abi, op: u32) -> u32 {
    unary_impl::<false>(s, op)
}

fn unary_impl<const FAST: bool>(s: &mut X87Abi, op: u32) -> u32 {
    use unary_op::*;
    assert!(op <= PREM1, "h2x87: bad unary op {op}");
    waiting!(s);
    let two = matches!(op, PATAN | YL2X | YL2XP1 | SCALE | PREM | PREM1);
    let pushes = matches!(op, TAN | XTRACT | SINCOS);
    let (a, b) = match (s.st(0), s.st(1)) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) if !two => (a, F80::INDEFINITE),
        _ => {
            // Stack underflow: the destination(s) get the indefinite.
            return finish(s, STACK_FAULT, Some(false), Cc::NONE, move |s| match op {
                PATAN | YL2X | YL2XP1 => {
                    s.set_st(1, F80::INDEFINITE);
                    s.pop();
                }
                TAN | XTRACT | SINCOS => {
                    s.set_st(0, F80::INDEFINITE);
                    if !s.push_overflows() {
                        s.push(F80::INDEFINITE);
                    }
                }
                _ => s.set_st(0, F80::INDEFINITE),
            });
        }
    };
    if pushes && s.push_overflows() {
        // The push for the second result has nowhere to go.
        return finish(s, STACK_FAULT, Some(true), Cc::NONE, |s| {
            s.set_st(0, F80::INDEFINITE);
            s.push(F80::INDEFINITE);
        });
    }
    let m = s.mode();
    let transcendental = matches!(op, SIN | COS | TAN | SINCOS | PATAN | YL2X | YL2XP1 | X2M1);
    if transcendental && m.rc != Rc::Near {
        // The crate rounds transcendentals to nearest only; the game never
        // runs one under a directed RC (FLOAT.md §2.5). Stop, don't guess.
        return status::UNMODELLED;
    }
    let set0 = |v: F80| move |s: &mut X87Abi| s.set_st(0, v);
    match op {
        SQRT => {
            let (v, fl) = sqrt(a, m);
            finish(s, fl, None, Cc::round(fl), set0(v))
        }
        ABS => finish(s, Flags::NONE, None, Cc::c1(false), set0(a.abs())),
        CHS => finish(s, Flags::NONE, None, Cc::c1(false), set0(a.neg())),
        RNDINT => {
            let (v, fl) = rndint(a, m.rc);
            finish(s, fl, None, Cc::round(fl), set0(v))
        }
        SIN | COS => {
            let t = match (op == SIN, FAST) {
                (true, true) => sin(a),
                (true, false) => crate::trans::sin_n::<8>(a),
                (false, true) => crate::cos(a),
                (false, false) => crate::trans::cos_n::<8>(a),
            };
            let cc = Cc::round(t.flags).with(SW_C2, t.out_of_range);
            finish(s, t.flags, None, cc, set0(t.value))
        }
        TAN => {
            let t = if FAST {
                tan(a)
            } else {
                crate::trans::tan_n::<8>(a)
            };
            let cc = Cc::round(t.flags).with(SW_C2, t.out_of_range);
            let out = t.out_of_range;
            finish(s, t.flags, None, cc, move |s| {
                if !out {
                    s.set_st(0, t.value);
                    s.push(F80::ONE);
                }
            })
        }
        SINCOS => {
            let (t, c) = if FAST {
                sincos(a)
            } else {
                crate::trans::sincos_n::<8>(a)
            };
            let cc = Cc::round(t.flags).with(SW_C2, t.out_of_range);
            let out = t.out_of_range;
            finish(s, t.flags, None, cc, move |s| {
                if !out {
                    s.set_st(0, t.value);
                    s.push(c);
                }
            })
        }
        PATAN | YL2X | YL2XP1 => {
            let (v, fl) = match op {
                PATAN if FAST => patan(b, a),
                PATAN => crate::trans::patan_n::<8>(b, a),
                YL2X if FAST => yl2x(b, a),
                YL2X => crate::trans::yl2x_n::<8>(b, a),
                _ => yl2xp1(b, a),
            };
            finish(s, fl, None, Cc::round(fl), move |s| {
                s.set_st(1, v);
                s.pop();
            })
        }
        X2M1 => {
            let (v, fl) = if FAST {
                x2m1(a)
            } else {
                crate::trans::x2m1_n::<8>(a)
            };
            finish(s, fl, None, Cc::round(fl), set0(v))
        }
        SCALE => {
            let (v, fl) = scale(a, b, m.rc);
            finish(s, fl, None, Cc::round(fl), set0(v))
        }
        XTRACT => {
            let (e, sig, fl) = xtract(a);
            finish(s, fl, None, Cc::c1(false), move |s| {
                s.set_st(0, e);
                s.push(sig);
            })
        }
        _ => {
            let p = if op == PREM { prem(a, b) } else { prem1(a, b) };
            // C2 = incomplete; when complete C0 = Q2, C3 = Q1, C1 = Q0.
            // When incomplete the SDM pseudo-code leaves C0/C1/C3 alone.
            let mut cc = Cc::NONE.with(SW_C2, p.incomplete);
            if !p.incomplete {
                cc = cc
                    .with(SW_C0, p.q & 4 != 0)
                    .with(SW_C3, p.q & 2 != 0)
                    .with(SW_C1, p.q & 1 != 0);
            }
            finish(s, p.flags, None, cc, set0(p.value))
        }
    }
}

/// FCOM/FUCOM/FICOM (`kind` 0/1/2) of ST(0) against a source as in
/// [`arith`], popping `pop` (0..=2) times. FTST is `kind` 0 against
/// `F32` +0.0.
///
/// Without `to_flags` the result goes to C3/C2/C0 (C1 cleared) and the
/// low bits of the return value are those status-word bits. With
/// `to_flags` (the FCOMI forms) only C1 is cleared in the status word and
/// the low bits are EFLAGS ZF/PF/CF; the caller merges them, clears
/// OF/SF/AF and marks all six defined.
///
/// An unmasked exception (IE, DE, or a stack fault) returns
/// `status::SUPPRESS`: the status word has its flags, ES and B (and C1 for
/// a stack fault) but C3/C2/C0 and the stack are unchanged, and the
/// low bits are those unchanged C3/C2/C0 (status-word forms, so a fused
/// `fnstsw`/`test`/`jcc` sees what `fnstsw` would) or 0 (FCOMI forms: the
/// caller must leave ZF/PF/CF alone and only clear OF/SF/AF).
///
/// # Panics
/// On an unknown `kind`, `src_kind` or `pop`.
pub fn cmp(s: &mut X87Abi, kind: u32, skind: u32, src: u64, pop: u32, to_flags: bool) -> u32 {
    assert!(kind <= 2 && pop <= 2, "h2x87: bad compare {kind}/{pop}");
    let (b, den, src_empty) = source(s, skind, src);
    waiting!(s);
    let (rel, fl, c1) = match s.st(0) {
        Some(a) if !src_empty => {
            let (rel, fl) = compare(a, b, kind == 1);
            let fl = if rel == Cmp::Unordered {
                fl
            } else {
                mem_de(fl, den, a)
            };
            (rel, fl, None)
        }
        _ => (Cmp::Unordered, STACK_FAULT, Some(false)),
    };
    let (c3, c2, c0) = match rel {
        Cmp::Greater => (false, false, false),
        Cmp::Less => (false, false, true),
        Cmp::Equal => (true, false, false),
        Cmp::Unordered => (true, true, true),
    };
    let cc = if to_flags {
        Cc::c1(false)
    } else {
        Cc::c1(false)
            .with(SW_C3, c3)
            .with(SW_C2, c2)
            .with(SW_C0, c0)
    };
    let out = outcome(s, fl);
    let r = finish(s, fl, c1, cc, |s| {
        for _ in 0..pop {
            s.pop();
        }
    });
    if r != status::OK {
        return r;
    }
    if matches!(out, Outcome::Suppress) {
        let kept = if to_flags {
            0
        } else {
            (s.status & (SW_C3 | SW_C2 | SW_C0)) as u32
        };
        return status::SUPPRESS | kept;
    }
    let bit = |b: bool, v: u32| if b { v } else { 0 };
    if to_flags {
        bit(c3, EFL_ZF) | bit(c2, EFL_PF) | bit(c0, EFL_CF)
    } else {
        bit(c3, SW_C3 as u32) | bit(c2, SW_C2 as u32) | bit(c0, SW_C0 as u32)
    }
}

/// FXAM: C1 = sign; C3,C2,C0 = class (empty 101, unsupported 000, NaN 001,
/// normal 010, infinity 011, zero 100, denormal 110).
pub fn xam(s: &mut X87Abi) -> u32 {
    waiting!(s);
    let p = s.phys(0);
    let raw = s.reg(p);
    let (c3, c2, c0) = if s.tag_of(p) == TAG_EMPTY {
        (true, false, true)
    } else {
        match raw.classify() {
            Class::Unsupported => (false, false, false),
            Class::QNaN | Class::SNaN => (false, false, true),
            Class::Normal => (false, true, false),
            Class::Inf => (false, true, true),
            Class::Zero => (true, false, false),
            Class::Denormal => (true, true, false),
        }
    };
    let cc = Cc::c1(raw.is_negative())
        .with(SW_C3, c3)
        .with(SW_C2, c2)
        .with(SW_C0, c0);
    finish(s, Flags::NONE, None, cc, |_| {})
}

/// FXCH ST(i). An empty operand is a stack underflow; masked, it is
/// replaced by the indefinite before the exchange.
pub fn xch(s: &mut X87Abi, i: u32) -> u32 {
    waiting!(s);
    let i = i & 7;
    let (a, b) = (s.st(0), s.st(i));
    let (fl, c1) = if a.is_some() && b.is_some() {
        (Flags::NONE, None)
    } else {
        (STACK_FAULT, Some(false))
    };
    let a = a.unwrap_or(F80::INDEFINITE);
    let b = b.unwrap_or(F80::INDEFINITE);
    let cc = if c1.is_none() {
        Cc::c1(false)
    } else {
        Cc::NONE
    };
    finish(s, fl, c1, cc, |s| {
        s.set_st(0, b);
        s.set_st(i, a);
    })
}

/// FFREE ST(i) (and FFREEP when `pop`).
pub fn free(s: &mut X87Abi, i: u32, pop: bool) -> u32 {
    waiting!(s);
    let p = s.phys(i & 7);
    s.set_tag(p, TAG_EMPTY);
    if pop {
        s.pop();
    }
    status::OK
}

/// FINCSTP (+1) / FDECSTP (-1): TOP changes, tags do not; C1 cleared.
pub fn adj_top(s: &mut X87Abi, delta: i32) -> u32 {
    waiting!(s);
    s.set_top((s.top() as i32 + delta) as u32);
    s.status &= !SW_C1;
    status::OK
}

/// FCMOVcc ST(0), ST(i): `take` is the evaluated condition. An empty ST(0)
/// or ST(i) is a stack underflow whatever the condition.
pub fn cmov(s: &mut X87Abi, i: u32, take: bool) -> u32 {
    waiting!(s);
    let i = i & 7;
    match (s.st(0), s.st(i)) {
        (Some(_), Some(v)) => finish(s, Flags::NONE, None, Cc::NONE, |s| {
            if take {
                s.set_st(0, v);
            }
        }),
        _ => finish(s, STACK_FAULT, Some(false), Cc::NONE, |s| {
            if take {
                s.set_st(0, F80::INDEFINITE);
            }
        }),
    }
}

/// FWAIT, and the check MMX entry/EMMS make: `MF` if an unmasked exception
/// is pending.
pub fn wait(s: &X87Abi) -> u32 {
    if s.pending() { status::MF } else { status::OK }
}

/// FLDCW (a waiting instruction): the word is normalised (`normalise_cw`).
/// Unmasking a pending
/// exception sets ES and B (the fault comes at the next waiting
/// instruction); masking all clears them.
pub fn ldcw(s: &mut X87Abi, cw: u16) -> u32 {
    waiting!(s);
    s.control = normalise_cw(cw);
    update_es(s);
    status::OK
}

fn update_es(s: &mut X87Abi) {
    if s.status & 0x3f & !(s.control & 0x3f) != 0 {
        s.status |= SW_ES | SW_B;
    } else {
        s.status &= !(SW_ES | SW_B);
    }
}

/// FNCLEX: clear IE..PE, SF, ES and B.
pub fn clex(s: &mut X87Abi) {
    s.status &= !(0x00ff | SW_B);
}

/// FNINIT. Register contents are kept.
pub fn init(s: &mut X87Abi) {
    s.control = 0x037f;
    s.status = 0;
    s.tag = 0xffff;
    s.opcode = 0;
    s.instruction_pointer = 0;
    s.data_pointer = 0;
    s.instruction_selector = 0;
    s.data_selector = 0;
}

fn put16(m: &mut [u8], at: usize, v: u16) {
    m[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(m: &mut [u8], at: usize, v: u32) {
    m[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn get16(m: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([m[at], m[at + 1]])
}
fn get32(m: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([m[at], m[at + 1], m[at + 2], m[at + 3]])
}

/// Guest bytes an environment operation reads or writes: 28 for
/// FNSTENV/FLDENV, 108 for FNSAVE/FRSTOR.
///
/// # Panics
/// On an unknown `op`.
pub const fn env_len(op: u32) -> usize {
    match op {
        env_op::FNSTENV | env_op::FLDENV => 28,
        env_op::FNSAVE | env_op::FRSTOR => 108,
        _ => panic!("h2x87: bad environment op"),
    }
}

/// Phase 1 of FNSTENV/FNSAVE: build the 32-bit protected-mode image
/// without changing the state. Layout: cw, sw, tag at 0/4/8 (upper halves
/// 0xffff), FIP at 12, FCS at 16, FOP (11 bits) at 18, FDP at 20, FDS at 24;
/// for FNSAVE, ST(0)..ST(7) as 10-byte values from 28. The caller writes
/// `env_len(op)` bytes to the guest, then calls [`save_commit`].
pub fn save_image(s: &X87Abi, op: u32) -> [u8; 108] {
    assert!(
        op == env_op::FNSTENV || op == env_op::FNSAVE,
        "h2x87: bad save op {op}"
    );
    let mut m = [0u8; 108];
    put16(&mut m, 0, s.control);
    put16(&mut m, 2, 0xffff);
    put16(&mut m, 4, s.status);
    put16(&mut m, 6, 0xffff);
    put16(&mut m, 8, s.tag);
    put16(&mut m, 10, 0xffff);
    put32(&mut m, 12, s.instruction_pointer);
    put16(&mut m, 16, s.instruction_selector);
    put16(&mut m, 18, s.opcode & 0x07ff);
    put32(&mut m, 20, s.data_pointer);
    put16(&mut m, 24, s.data_selector);
    put16(&mut m, 26, 0xffff);
    if op == env_op::FNSAVE {
        for i in 0..8u32 {
            let at = 28 + 10 * i as usize;
            m[at..at + 10].copy_from_slice(&s.st[s.phys(i)]);
        }
    }
    m
}

/// Phase 3 of FNSTENV (mask all exceptions) or FNSAVE (FNINIT). The image
/// from phase 1 keeps ES and B; the live status recomputes them, so with
/// everything masked a pending #MF is discarded, as on hardware.
pub fn save_commit(s: &mut X87Abi, op: u32) {
    match op {
        env_op::FNSTENV => {
            s.control |= 0x3f;
            update_es(s);
        }
        env_op::FNSAVE => init(s),
        _ => panic!("h2x87: bad save op {op}"),
    }
}

/// FLDENV/FRSTOR from an image the caller already read from the guest
/// (`env_len(op)` meaningful bytes). The control word is normalised, ES
/// and B are recomputed from the loaded flags and masks, and a register
/// whose loaded tag is not 11 gets the tag its contents imply. Both are
/// waiting instructions: generated code calls [`wait`] before reading the
/// guest image, so a pending #MF wins over a guest page fault; this
/// function checks again.
pub fn load_env(s: &mut X87Abi, op: u32, m: &[u8; 108]) -> u32 {
    assert!(
        op == env_op::FLDENV || op == env_op::FRSTOR,
        "h2x87: bad load op {op}"
    );
    waiting!(s);
    s.control = normalise_cw(get16(m, 0));
    s.status = get16(m, 4);
    let tag = get16(m, 8);
    s.instruction_pointer = get32(m, 12);
    s.instruction_selector = get16(m, 16);
    s.opcode = get16(m, 18) & 0x07ff;
    s.data_pointer = get32(m, 20);
    s.data_selector = get16(m, 24);
    if op == env_op::FRSTOR {
        for i in 0..8u32 {
            let at = 28 + 10 * i as usize;
            let p = s.phys(i);
            s.st[p].copy_from_slice(&m[at..at + 10]);
        }
    }
    for p in 0..8 {
        let t = if (tag >> (2 * p)) & 3 == TAG_EMPTY {
            TAG_EMPTY
        } else {
            tag_for(s.reg(p))
        };
        s.set_tag(p, t);
    }
    update_es(s);
    status::OK
}

/// The versioned `extern "C"` entry points of `include/h2x87.h`. A null
/// pointer or an out-of-range selector is a generator bug: the helper
/// panics, which aborts the process at the C boundary.
#[allow(unsafe_code)]
pub mod ffi {
    use super::*;

    /// # Safety
    /// `p` is null or valid for reads and writes of an `X87Abi`.
    unsafe fn state<'a>(p: *mut X87Abi) -> &'a mut X87Abi {
        // SAFETY: the caller guarantees validity when non-null.
        unsafe { p.as_mut() }.expect("h2x87: null X87Abi")
    }

    /// # Safety
    /// `p` is null or valid for reads of an `X87Abi`, which nothing writes
    /// while the reference lives. For the entry points that only read the
    /// state: the C side may pass a `const X87Abi *` to shared storage.
    unsafe fn state_ref<'a>(p: *const X87Abi) -> &'a X87Abi {
        // SAFETY: the caller guarantees validity when non-null.
        unsafe { p.as_ref() }.expect("h2x87: null X87Abi")
    }

    /// # Safety
    /// `p` is null or valid for a write of `T`.
    unsafe fn out<'a, T>(p: *mut T) -> &'a mut T {
        // SAFETY: as above.
        unsafe { p.as_mut() }.expect("h2x87: null out-pointer")
    }

    /// # Safety
    /// `p` is null or valid for reads of `N` bytes.
    unsafe fn bytes<const N: usize>(p: *const u8) -> [u8; N] {
        assert!(!p.is_null(), "h2x87: null buffer");
        // SAFETY: the caller guarantees N readable bytes.
        unsafe { core::ptr::read_unaligned(p as *const [u8; N]) }
    }

    macro_rules! store_entry {
        ($name:ident, $f:ident, $t:ty) => {
            /// Phase 1 of a store; see [`super::store_commit`].
            ///
            /// # Safety
            /// `s`, `value` and `sw` are valid.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(
                s: *mut X87Abi,
                i: u32,
                value: *mut $t,
                sw: *mut u16,
            ) -> u32 {
                // SAFETY: forwarded from the caller.
                let (st, value, sw) = unsafe { (state_ref(s), out(value), out(sw)) };
                let (r, o) = $f(st, i);
                *value = o.value;
                *sw = o.status;
                r
            }
        };
    }

    macro_rules! entry {
        ($(#[$doc:meta])* fn $name:ident($($a:ident: $t:ty),*) => $body:expr) => {
            $(#[$doc])*
            /// # Safety
            /// `s` is valid.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(s: *mut X87Abi $(, $a: $t)*) -> u32 {
                // SAFETY: forwarded from the caller.
                let s = unsafe { state(s) };
                $body(s)
            }
        };
    }

    entry!(fn h2x87_v1_ld_f32(bits: u32) => |s| ld_f32(s, bits));
    entry!(fn h2x87_v1_ld_f64(bits: u64) => |s| ld_f64(s, bits));
    entry!(fn h2x87_v1_ld_i16(v: i16) => |s| ld_int(s, v as i64));
    entry!(fn h2x87_v1_ld_i32(v: i32) => |s| ld_int(s, v as i64));
    entry!(fn h2x87_v1_ld_i64(v: i64) => |s| ld_int(s, v));
    entry!(fn h2x87_v1_ld_const(which: u32) => |s| ld_const(s, which));
    entry!(fn h2x87_v1_ld_st(i: u32) => |s| ld_st(s, i));
    entry!(fn h2x87_v1_st_st(dst: u32, pop: u32) => |s| st_st(s, dst, pop != 0));
    entry!(fn h2x87_v1_arith(op: u32, dst_i: u32, kind: u32, src: u64)
        => |s| arith(s, op, dst_i, kind, src));
    entry!(fn h2x87_v1_unary(op: u32) => |s| unary(s, op));
    entry!(fn h2x87_v1_cmp(kind: u32, skind: u32, src: u64, pop: u32, to_flags: u32)
        => |s| cmp(s, kind, skind, src, pop, to_flags != 0));
    entry!(fn h2x87_v1_xam() => xam);
    entry!(fn h2x87_v1_xch(i: u32) => |s| xch(s, i));
    entry!(fn h2x87_v1_free(i: u32, pop: u32) => |s| free(s, i, pop != 0));
    entry!(fn h2x87_v1_adj_top(delta: i32) => |s| adj_top(s, delta));
    entry!(fn h2x87_v1_cmov(i: u32, take: u32) => |s| cmov(s, i, take != 0));
    entry!(fn h2x87_v1_ldcw(cw: u16) => |s| ldcw(s, cw));
    entry!(fn h2x87_v1_wait() => |s: &mut X87Abi| wait(s));
    entry!(fn h2x87_v1_clex() => |s| { clex(s); status::OK });
    entry!(fn h2x87_v1_init() => |s| { init(s); status::OK });
    entry!(fn h2x87_v1_save_commit(op: u32) => |s| { save_commit(s, op); status::OK });
    entry!(fn h2x87_v1_store_commit(sw: u16, pop: u32)
        => |s| { store_commit(s, sw, pop != 0); status::OK });

    /// # Safety
    /// `s` is valid; `m` is valid for 10 bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn h2x87_v1_ld_f80(s: *mut X87Abi, m: *const u8) -> u32 {
        // SAFETY: forwarded from the caller.
        let (s, b) = unsafe { (state(s), bytes::<10>(m)) };
        ld_f80(s, b)
    }
    /// # Safety
    /// `s` is valid; `m` is valid for 10 bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn h2x87_v1_ld_bcd(s: *mut X87Abi, m: *const u8) -> u32 {
        // SAFETY: forwarded from the caller.
        let (s, b) = unsafe { (state(s), bytes::<10>(m)) };
        ld_bcd(s, b)
    }

    store_entry!(h2x87_v1_st_f32, st_f32, u32);
    store_entry!(h2x87_v1_st_f64, st_f64, u64);
    store_entry!(h2x87_v1_st_i16, st_i16, i16);
    store_entry!(h2x87_v1_st_i32, st_i32, i32);
    store_entry!(h2x87_v1_st_i64, st_i64, i64);
    store_entry!(h2x87_v1_st_i32_trunc, st_i32_trunc, i32);
    store_entry!(h2x87_v1_st_f80, st_f80, [u8; 10]);
    store_entry!(h2x87_v1_st_bcd, st_bcd, [u8; 10]);

    /// # Safety
    /// `s` and `out` are valid.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn h2x87_v1_ftol2(s: *mut X87Abi, o: *mut u64) -> u32 {
        // SAFETY: forwarded from the caller.
        let (s, o) = unsafe { (state(s), out(o)) };
        ftol2(s, o)
    }
    /// # Safety
    /// `s` is valid; `img` is valid for 108 bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn h2x87_v1_save_image(s: *const X87Abi, op: u32, img: *mut u8) -> u32 {
        // SAFETY: forwarded from the caller.
        let (s, img) = unsafe { (state_ref(s), out(img as *mut [u8; 108])) };
        *img = save_image(s, op);
        status::OK
    }
    /// # Safety
    /// `s` is valid; `img` is valid for 108 bytes (only `env_len(op)` are
    /// read for meaning; the rest may be anything).
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn h2x87_v1_load_env(s: *mut X87Abi, op: u32, img: *const u8) -> u32 {
        // SAFETY: forwarded from the caller.
        let (s, img) = unsafe { (state(s), bytes::<108>(img)) };
        load_env(s, op, &img)
    }
}
