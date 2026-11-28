//! `0x0013b390`, the tag function evaluator, instruction for instruction:
//! the SSE arithmetic in the original's operand order (so NaNs keep the
//! sign and payload the original's do), the x87 parts on the x87 model,
//! and the CRT's `fmod` and `pow` with their special cases and error
//! paths (`_87except`, `_handle_exc`) as the image has them.
//!
//! (Code, `h2 disasm`.) EDI points at the function's data reference
//! (count, then address); the two stack words are the input `x` and the
//! range input `r`. A missing or empty blob, types 6 and 8 and types above
//! 10 give 0; otherwise the blob's type byte picks the curve, its flags'
//! bit 0 a second curve blended in by `r`, and the result is clamped to
//! 0..1 (NaN passes):
//!
//! - 0, identity: `x`.
//! - 1, constant: `r` when blended, else 0 (unclamped).
//! - 2, transition: `a + (b - a) * shape(sub, x)` (`0x0017ca10`).
//! - 3, periodic: `c + (d - c) * wave(sub, x * f + p)` (`0x0017c900`).
//! - 4, linear: `x * m + b`.
//! - 5, linear key: three ramps, each clamped to 0..1, weighted.
//! - 7, spline: a cubic in `x`.
//! - 9, exponent: `a + (b - a) * pow(x, e)` in x87, or 1 for an
//!   exponent within 1e-4 of zero (or a negative one and `x` within 1e-4).
//! - 10, spline 2: the cubic at `pow(x, e)`, `e` an estimate of the
//!   ratio of two control segments' lengths (`rsqrtss` and the exponent
//!   halving trick), when both are long enough and differ.
//!
//! The blend is `second * r + (1 - r) * first`, except for type 9:
//! `(1 - r) * first + second * r` (the order decides which NaN wins).
//!
//! The wave and shape tables (`0x00468d48`, `0x0046bd48`, 1024 bytes per
//! sub-type) and every constant are read from the image at run time.
//! Checked against the lifted original by
//! `engine/tests/screen_effects_differential.rs`.

use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;

use x87::abi::{arith_op, src_kind, unary_op};

/// Addresses the evaluator and its helpers read.
pub mod constant {
    /// 1.0 (f32).
    pub const ONE: u32 = 0x0045_dbc0;
    /// 0.0 (f32), the spline-2 clamp's.
    pub const ZERO: u32 = 0x0045_dbd8;
    /// 1e-4 (f32): the evaluator's "near zero".
    pub const SMALL: u32 = 0x0045_dbdc;
    /// The shape lookup's scale (1023) and bias (0.1).
    pub const SHAPE_SCALE: u32 = 0x0045_df04;
    pub const SHAPE_BIAS: u32 = 0x0045_dc68;
    /// The wave lookup's scale (1024 over 28).
    pub const WAVE_SCALE: u32 = 0x0045_e058;
    /// 1/255: a table byte as a value.
    pub const BYTE: u32 = 0x0045_dbb4;
    /// The wrapping waves' thresholds (0.75, 0.25).
    pub const WRAP_HIGH: u32 = 0x0045_dc2c;
    pub const WRAP_LOW: u32 = 0x0045_dc20;
    /// The tables, 1024 bytes per sub-type.
    pub const WAVES: u32 = 0x0046_8d48;
    pub const SHAPES: u32 = 0x0046_bd48;
    /// Words per curve of each type (bytes, by type): `0x0013bec0`.
    pub const SET_WORDS: u32 = 0x0044_178c;

    /// The CRT's. The x87 real indefinite (tbyte), its +infinity (tbyte),
    /// 0.5 (double).
    pub const INDEFINITE: u32 = 0x0046_2738;
    pub const INFINITY_80: u32 = 0x0046_6e8c;
    pub const HALF_64: u32 = 0x0046_6e00;
    /// `pow`'s range check (doubles): the largest and smallest normal,
    /// the scales -1536 and 1536, infinity and zero.
    pub const DBL_MAX: u32 = 0x0041_6218;
    pub const DBL_MIN: u32 = 0x0041_6220;
    pub const DOWN: u32 = 0x0041_6228;
    pub const UP: u32 = 0x0041_6230;
    pub const OVER: u32 = 0x0041_6238;
    pub const UNDER: u32 = 0x0041_6240;
    /// `_powhlp` and `_handle_exc` (doubles): infinity, the indefinite,
    /// -0, the largest normal, 0 and 1.
    pub const INFINITY_64: u32 = 0x0046_20b8;
    pub const INDEFINITE_64: u32 = 0x0046_20c0;
    pub const NEGATIVE_ZERO_64: u32 = 0x0046_20d8;
    pub const MAX_64: u32 = 0x0046_20c8;
    pub const ZERO_64: u32 = 0x0045_dbe0;
    pub const ONE_64: u32 = 0x0045_dcc0;
    /// Set, `pow` skips its range check and error reporting. Only read.
    pub const NO_REPORT: u32 = 0x0048_0cd4;
}

use constant::*;

const SIGN_64: u64 = 1 << 63;
const QUIET_64: u64 = 1 << 51;

fn u64_at(mem: &dyn GuestMemory, a: u32) -> Result<u64, Error> {
    Ok(u64::from(mem.u32(a)?) | u64::from(mem.u32(a.wrapping_add(4))?) << 32)
}

fn tbyte(mem: &dyn GuestMemory, a: u32) -> Result<[u8; 10], Error> {
    let mut b = [0u8; 10];
    mem.read(a, &mut b)?;
    Ok(b)
}

// ---- x87 forms the light-constant helpers do not have ----

pub(crate) fn fld_m64(x: &mut X87, bits: u64) -> Result<(), Error> {
    X87::ok(x87::abi::ld_f64(&mut x.0, bits))
}

fn fld_m80(x: &mut X87, b: [u8; 10]) -> Result<(), Error> {
    X87::ok(x87::abi::ld_f80(&mut x.0, b))
}

fn fst_m64(x: &mut X87, pop: bool) -> Result<u64, Error> {
    let (code, store) = x87::abi::st_f64(&x.0, 0);
    X87::ok(code)?;
    x87::abi::store_commit(&mut x.0, store.status, pop);
    Ok(store.value)
}

fn fistp_m32(x: &mut X87) -> Result<i32, Error> {
    let (code, store) = x87::abi::st_i32(&x.0, 0);
    X87::ok(code)?;
    x87::abi::store_commit(&mut x.0, store.status, true);
    Ok(store.value)
}

fn arith_m64(x: &mut X87, op: u32, bits: u64) -> Result<(), Error> {
    X87::ok(x87::abi::arith(&mut x.0, op, 0, src_kind::F64, bits))
}

fn fld_const(x: &mut X87, which: u32) -> Result<(), Error> {
    X87::ok(x87::abi::ld_const(&mut x.0, which))
}

fn fld1(x: &mut X87) -> Result<(), Error> {
    fld_const(x, 0)
}

fn fldz(x: &mut X87) -> Result<(), Error> {
    fld_const(x, 1)
}

fn fchs(x: &mut X87) -> Result<(), Error> {
    x.unary(unary_op::CHS)
}

fn ldcw(x: &mut X87, cw: u16) -> Result<(), Error> {
    X87::ok(x87::abi::ldcw(&mut x.0, cw))
}

/// `fcom(p) m64`: the condition bits (C3 `0x4000`, C2 `0x400`, C0 `0x100`).
fn fcom_m64(x: &mut X87, bits: u64, pop: u32) -> Result<u32, Error> {
    let r = x87::abi::cmp(&mut x.0, 0, src_kind::F64, bits, pop, false);
    X87::ok(r)?;
    Ok(r & 0x4500)
}

/// `fcom(p)(p) st(1)`: the condition bits.
fn fcom_st1(x: &mut X87, pop: u32) -> Result<u32, Error> {
    let r = x87::abi::cmp(&mut x.0, 0, src_kind::ST, 1, pop, false);
    X87::ok(r)?;
    Ok(r & 0x4500)
}

const C0: u32 = 0x100;
const C3: u32 = 0x4000;

/// `fld small; fcomip st, st(1); fstp st(0); ja`: whether `[SMALL]` is
/// above `|v|`, with `|v|` computed from the f32 `v` (or already on the
/// stack when `v` is `None`).
fn small_above(mem: &dyn GuestMemory, x: &mut X87, v: Option<u32>) -> Result<bool, Error> {
    if let Some(v) = v {
        x.fld(v)?;
    }
    x.unary(unary_op::ABS)?;
    x.fld(mem.u32(SMALL)?)?;
    let f = x.fcomip(1)?;
    x.fstp_st(0)?;
    Ok(f & 0x41 == 0)
}

// ---- the CRT ----

/// `fmod(v, 1.0)` (`0x00373b6a` through the dispatcher `0x003733cc`) on
/// the f32 `v` loaded into the x87, then stored as f32: a finite non-zero
/// `v` its exact remainder (`fprem`, the sign of `v`), a zero +0
/// (`fldz`), a NaN quieted (`v + 1.0`), an infinity the real indefinite.
/// The error paths' `_87except` keeps the value (the invalid operation is
/// masked).
pub fn fmod_one(mem: &dyn GuestMemory, control: u16, v: u32) -> Result<u32, Error> {
    let f = f32::from_bits(v);
    Ok(if f.is_nan() {
        v | 0x0040_0000
    } else if f.is_infinite() {
        let mut x = X87::new(control);
        fld_m80(&mut x, tbyte(mem, INDEFINITE)?)?;
        x.fstp_m32()?
    } else if f == 0.0 {
        0
    } else {
        (f % 1.0).to_bits()
    })
}

/// `_d_inttype` on a finite, normal `y`: 0 not an integer, 1 odd, 2 even.
fn inttype(y: f64) -> u32 {
    if y.floor() != y {
        0
    } else if (y * 0.5).floor() == y * 0.5 {
        2
    } else {
        1
    }
}

/// `_powhlp` (`0x003738a8`): `x ^ y` with either infinite (neither NaN),
/// as a double, and whether it is an error (`|x| == 1` to an infinite
/// power: the indefinite).
fn powhlp(mem: &dyn GuestMemory, x: u64, y: u64) -> Result<(u64, bool), Error> {
    const POS_INF: u64 = 0x7ff0_0000_0000_0000;
    let inf = u64_at(mem, INFINITY_64)?;
    let zero = 0u64; // fldz
    let one = 1.0f64.to_bits(); // fld1
    let (xf, yf) = (f64::from_bits(x), f64::from_bits(y));
    let ax = if xf < 0.0 { -xf } else { xf };
    let one_64 = f64::from_bits(u64_at(mem, ONE_64)?);
    let zero_64 = f64::from_bits(u64_at(mem, ZERO_64)?);
    Ok(if y == POS_INF || y == POS_INF | SIGN_64 {
        let up = y == POS_INF;
        if ax > one_64 {
            (if up { inf } else { zero }, false)
        } else if ax < one_64 {
            (if up { zero } else { inf }, false)
        } else {
            (u64_at(mem, INDEFINITE_64)?, true)
        }
    } else if x == POS_INF {
        if yf > zero_64 {
            (inf, false)
        } else if yf < zero_64 {
            (zero, false)
        } else {
            (one, false)
        }
    } else {
        // x is -infinity.
        let odd = inttype(yf) == 1;
        if yf > zero_64 {
            (if odd { inf ^ SIGN_64 } else { inf }, false)
        } else if yf < zero_64 {
            (if odd { u64_at(mem, NEGATIVE_ZERO_64)? } else { zero }, false)
        } else {
            (one, false)
        }
    })
}

/// `0x00327fc9`: a double onto the stack; an infinity or NaN built as the
/// extended value with the same bits (no invalid operation for a
/// signalling NaN).
fn load_double(x: &mut X87, d: u64) -> Result<(), Error> {
    if (d >> 52) & 0x7ff == 0x7ff {
        let mut b = [0u8; 10];
        b[..8].copy_from_slice(&(d << 11).to_le_bytes());
        b[8..].copy_from_slice(&(((d >> 48) as u16) | 0x7fff).to_le_bytes());
        fld_m80(x, b)
    } else {
        fld_m64(x, d)
    }
}

/// `0x00372f31` on ST(0), which it pops: 0 not an integer, 1 odd, 2 even.
fn int_test(mem: &dyn GuestMemory, x: &mut X87) -> Result<u32, Error> {
    x.fld_st(0)?;
    x.unary(unary_op::RNDINT)?;
    if fcom_st1(x, 1)? & C3 == 0 {
        x.fstp_st(0)?;
        return Ok(0);
    }
    arith_m64(x, arith_op::MUL, u64_at(mem, HALF_64)?)?;
    x.fld_st(0)?;
    x.unary(unary_op::RNDINT)?;
    Ok(if fcom_st1(x, 2)? & C3 == 0 { 1 } else { 2 })
}

/// `0x00327f84`: 2 to the ST(0).
fn two_to(x: &mut X87) -> Result<(), Error> {
    x.fld_st(0)?;
    x.unary(unary_op::RNDINT)?;
    x.arith_reg(arith_op::SUBR, 1, 0, false)?;
    x.fxch(1)?;
    fchs(x)?;
    x.unary(unary_op::X2M1)?;
    fld1(x)?;
    x.arith_reg(arith_op::ADD, 1, 0, true)?;
    x.unary(unary_op::SCALE)?;
    x.fstp_st(1)
}

/// How `pow` leaves.
enum Exit {
    /// The value on the stack.
    Done,
    /// Through the range check `0x0032806d`.
    Range,
    /// Through the error path with this exception type.
    Fail(u32),
}

/// `_handle_exc` (`0x003246b5`) on the value `pow` reports, for the
/// masked exceptions the port takes: an overflow (type 3) becomes the
/// rounding mode's infinity or largest value, an underflow (type 4) the
/// value scaled back by 2^-1536 into a denormal by truncation, or zero;
/// the others keep it.
fn handle_exc(mem: &dyn GuestMemory, control: u16, kind: u32, v: u64) -> Result<u64, Error> {
    let positive = f64::from_bits(v) > f64::from_bits(u64_at(mem, ZERO_64)?);
    match kind {
        3 => {
            let inf = u64_at(mem, INFINITY_64)?;
            let max = u64_at(mem, MAX_64)?;
            Ok(match control & 0xc00 {
                0x000 => if positive { inf } else { inf ^ SIGN_64 },
                0x400 => if positive { max } else { inf ^ SIGN_64 },
                0x800 => if positive { inf } else { max ^ SIGN_64 },
                _ => if positive { max } else { max ^ SIGN_64 },
            })
        }
        4 => {
            let zero = u64_at(mem, ZERO_64)?;
            if f64::from_bits(v) == f64::from_bits(zero) {
                return Ok(v);
            }
            // _decomp: the mantissa in 0.5..1 (sign kept), the exponent.
            let e = ((v >> 52) & 0x7ff) as i32;
            let mant = (v & !(0x7ff << 52)) | (0x3fe << 52);
            let expn = e - 0x3fe - 0x600;
            if expn < -1074 {
                let mut x = X87::new(control);
                fld_m64(&mut x, mant)?;
                arith_m64(&mut x, arith_op::MUL, zero)?;
                return fst_m64(&mut x, true);
            }
            let negative = f64::from_bits(mant) < f64::from_bits(zero);
            let mut bits = (mant & 0x000f_ffff_ffff_ffff) | 0x0010_0000_0000_0000;
            if expn < -1021 {
                bits >>= -1021 - expn;
            }
            Ok(if negative { bits ^ SIGN_64 } else { bits })
        }
        _ => Ok(v),
    }
}

/// `pow` (`0x00372d48`) on the stack as the evaluator leaves it (ST(0)
/// the exponent, ST(1) the base): pops both, pushes the result, as the
/// original computes it under its control word (`(cw & 0x300) | 0x7f`
/// unless the caller's is `0x27f`). Control words with an exception
/// unmasked are refused ([`Error::X87`]).
pub(crate) fn pow(mem: &dyn GuestMemory, x: &mut X87) -> Result<(), Error> {
    let saved = x.0.control;
    if saved & 0x3f != 0x3f {
        return Err(Error::X87(x87::abi::status::UNMODELLED));
    }
    x.fxch(1)?;
    let xd = fst_m64(x, true)?;
    let pd = fst_m64(x, false)?;
    if saved != 0x27f {
        ldcw(x, (saved & 0x300) | 0x7f)?;
    }
    let exit = pow_body(mem, x, xd, pd)?;
    let report = mem.u32(NO_REPORT)? == 0;
    match exit {
        Exit::Range if report => range(mem, x, saved)?,
        Exit::Fail(kind) if report => fail(mem, x, saved, kind)?,
        _ => {}
    }
    if saved != 0x27f {
        ldcw(x, saved)?;
    }
    Ok(())
}

fn pow_body(mem: &dyn GuestMemory, x: &mut X87, xd: u64, pd: u64) -> Result<Exit, Error> {
    let special = |d: u64| (d >> 52) & 0x7ff == 0x7ff;
    let nan = |d: u64| special(d) && d & 0x000f_ffff_ffff_ffff != 0;
    // ST(0) the exponent. The base goes on top; `signalling` is ECX.
    let signalling;
    if special(pd) {
        load_double(x, xd)?;
        if nan(pd) {
            if pd & QUIET_64 != 0 {
                x.arith_reg(arith_op::ADD, 1, 0, true)?;
                return Ok(Exit::Fail(1));
            }
            signalling = true;
        } else {
            signalling = false;
        }
    } else {
        load_double(x, xd)?;
        if !special(xd) {
            return finite(mem, x, xd, pd);
        }
        signalling = false;
    }
    // 0x00372e49: a NaN base, a signalling exponent, or an infinity.
    if nan(xd) {
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        return Ok(Exit::Fail(if xd & QUIET_64 != 0 { 1 } else { 7 }));
    }
    if signalling {
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        return Ok(Exit::Fail(7));
    }
    x.fstp_st(0)?;
    x.fstp_st(0)?;
    let (r, error) = powhlp(mem, xd, pd)?;
    fld_m64(x, r)?;
    Ok(if error { Exit::Fail(1) } else { Exit::Done })
}

/// Both finite: ST(0) the base, ST(1) the exponent.
fn finite(mem: &dyn GuestMemory, x: &mut X87, xd: u64, pd: u64) -> Result<Exit, Error> {
    if (xd >> 52) & 0x7ff == 0 && xd & 0x000f_ffff_ffff_ffff == 0 {
        // A zero base.
        x.fstp_st(0)?;
        if pd & !SIGN_64 == 0 {
            x.fstp_st(0)?;
            fld1(x)?;
            return Ok(Exit::Done);
        }
        let cl = int_test(mem, x)?;
        let odd_negative = xd & SIGN_64 != 0 && cl == 1;
        if pd & SIGN_64 != 0 {
            fld_m80(x, tbyte(mem, INFINITY_80)?)?;
            if odd_negative {
                fchs(x)?;
            }
            return Ok(Exit::Fail(2));
        }
        fldz(x)?;
        if odd_negative {
            fchs(x)?;
        }
        return Ok(Exit::Done);
    }
    let mut odd = false;
    if xd & SIGN_64 != 0 {
        x.fld_st(1)?;
        let cl = int_test(mem, x)?;
        fchs(x)?;
        if cl == 0 {
            x.fstp_st(0)?;
            x.fstp_st(0)?;
            fld_m80(x, tbyte(mem, INDEFINITE)?)?;
            return Ok(Exit::Fail(1));
        }
        odd = cl == 1;
    }
    x.unary(unary_op::YL2X)?;
    two_to(x)?;
    if odd {
        fchs(x)?;
    }
    Ok(Exit::Range)
}

/// `0x0032806d`: a result whose double is denormal or zero, or infinite,
/// goes to the error path scaled by 2^1536 or 2^-1536 (or as zero or
/// infinity past that).
fn range(mem: &dyn GuestMemory, x: &mut X87, saved: u16) -> Result<(), Error> {
    let d = fst_m64(x, false)?;
    let e = (d >> 52) & 0x7ff;
    if e != 0 && e != 0x7ff {
        return Ok(());
    }
    let under = e == 0;
    fld_m64(x, u64_at(mem, if under { UP } else { DOWN })?)?;
    x.fxch(1)?;
    x.unary(unary_op::SCALE)?;
    x.fstp_st(1)?;
    x.fld_st(0)?;
    x.unary(unary_op::ABS)?;
    let c = fcom_m64(x, u64_at(mem, if under { DBL_MIN } else { DBL_MAX })?, 1)?;
    if under {
        if c & C0 != 0 {
            arith_m64(x, arith_op::MUL, u64_at(mem, UNDER)?)?;
        }
        fail(mem, x, saved, 4)
    } else {
        if c & (C0 | C3) == 0 {
            arith_m64(x, arith_op::MUL, u64_at(mem, OVER)?)?;
        }
        fail(mem, x, saved, 3)
    }
}

/// `0x00327f30`: the value through a double, `_87except` (`0x003293f0`)
/// with the caller's control word, back.
fn fail(mem: &dyn GuestMemory, x: &mut X87, saved: u16, kind: u32) -> Result<(), Error> {
    let v = fst_m64(x, true)?;
    let v = handle_exc(mem, saved, kind, v)?;
    fld_m64(x, v)
}

// ---- the helpers ----

/// `0x0017ca10` (AX `sub`, XMM0 `x`): `x` clamped to 0..1; sub-type 0
/// that, else the shape table's bytes at `x * 1023` interpolated, clamped.
pub(crate) fn shape(mem: &dyn GuestMemory, sse: &mut Sse, control: u16, sub: u8, x: u32) -> Result<u32, Error> {
    let one = mem.u32(ONE)?;
    let x = sse.clamp(x, 0, one)?;
    if sub == 0 {
        return Ok(x);
    }
    let table = SHAPES.wrapping_add(u32::from(sub) << 10);
    let scaled = sse.mul(x, mem.u32(SHAPE_SCALE)?)?;
    let frac = fmod_one(mem, control, scaled)?;
    let at = sse.sub(scaled, mem.u32(SHAPE_BIAS)?)?;
    let mut st = X87::new(control);
    st.fld(at)?;
    let i = fistp_m32(&mut st)? as u16;
    let k = mem.u32(BYTE)?;
    let v = if i == 0x3ff {
        let b = sse.int(i32::from(mem.u8(table.wrapping_add(0x3ff))?))?;
        sse.mul(b, k)?
    } else {
        let at = table.wrapping_add(i32::from(i as i16) as u32);
        let a = sse.int(i32::from(mem.u8(at)?))?;
        let b = sse.int(i32::from(mem.u8(at.wrapping_add(1))?))?;
        let a = sse.mul(a, k)?;
        let rest = sse.sub(one, frac)?;
        let a = sse.mul(a, rest)?;
        let b = sse.mul(b, k)?;
        let b = sse.mul(b, frac)?;
        sse.add(a, b)?
    };
    sse.clamp(v, 0, one)
}

/// `0x0017c900` (AX `sub`, the stack word `x`): sub-type 0 is 1; else the
/// wave table's bytes at `x * WAVE_SCALE` (wrapping at 1024)
/// interpolated; sub-types 6 and 7 (by `1 << sub`'s low byte) wrap the
/// value through 1.
pub(crate) fn wave(mem: &dyn GuestMemory, sse: &mut Sse, control: u16, sub: u8, x: u32) -> Result<u32, Error> {
    let one = mem.u32(ONE)?;
    if sub == 0 {
        return Ok(one);
    }
    let scaled = sse.mul(x, mem.u32(WAVE_SCALE)?)?;
    let frac = fmod_one(mem, control, scaled)?;
    let whole = sse.sub(scaled, frac)?;
    let mut st = X87::new(control);
    st.fld(whole)?;
    let i = (fistp_m32(&mut st)? as u32) & 0x3ff;
    let table = WAVES.wrapping_add(u32::from(sub) << 10);
    let a = sse.int(i32::from(mem.u8(table.wrapping_add(i))?))?;
    let b = sse.int(i32::from(mem.u8(table.wrapping_add((i + 1) & 0x3ff))?))?;
    let k = mem.u32(BYTE)?;
    let a = sse.mul(a, k)?;
    let mut b = sse.mul(b, k)?;
    let wraps = (1u32 << (sub & 31)) as u8 & 0xc0 != 0;
    if wraps && sse.above(a, mem.u32(WRAP_HIGH)?)? && sse.above(mem.u32(WRAP_LOW)?, b)? {
        b = sse.add(b, one)?;
    }
    let rest = sse.sub(one, frac)?;
    let v = sse.mul(rest, a)?;
    let b = sse.mul(b, frac)?;
    let v = sse.add(v, b)?;
    if wraps && sse.above(v, one)? {
        return sse.sub(v, one);
    }
    Ok(v)
}

// ---- the evaluator ----

/// `second * r + (1 - r) * first`.
fn blend(sse: &mut Sse, one: u32, first: u32, second: u32, r: u32) -> Result<u32, Error> {
    let rest = sse.sub(one, r)?;
    let rest = sse.mul(rest, first)?;
    let s = sse.mul(second, r)?;
    sse.add(s, rest)
}

/// `a + (b - a) * t`, as `(b - a) * t + a`.
fn lerp(sse: &mut Sse, a: u32, b: u32, t: u32) -> Result<u32, Error> {
    let d = sse.sub(b, a)?;
    let d = sse.mul(d, t)?;
    sse.add(d, a)
}

/// Type 9's curve at `p` (the exponent at `p + 8`, `a`, `b` at `p`,
/// `p + 4`): 1 near a zero exponent, else `(b - a) * pow(x, e) + a` in
/// x87, stored as f32.
fn exponent(mem: &dyn GuestMemory, sse: &mut Sse, control: u16, p: u32, x: u32) -> Result<u32, Error> {
    let e = mem.u32(p.wrapping_add(8))?;
    let mut st = X87::new(control);
    if small_above(mem, &mut st, Some(e))? || (sse.above(0, e)? && small_above(mem, &mut st, Some(x))?) {
        return mem.u32(ONE).map_err(Error::from);
    }
    st.fld(x)?;
    st.fld(e)?;
    pow(mem, &mut st)?;
    st.fld(mem.u32(p.wrapping_add(4))?)?;
    st.arith_m(arith_op::SUB, mem.u32(p)?)?;
    st.arith_reg(arith_op::MUL, 1, 0, true)?;
    st.fadd_m(mem.u32(p)?)?;
    st.fstp_m32()
}

/// `0x0013bec0`: point `k` of curve `c` of a type-10 blob at `data`.
fn point(mem: &dyn GuestMemory, data: u32, c: u32, k: u32) -> Result<(u32, u32), Error> {
    let stride = u32::from(mem.u8(SET_WORDS.wrapping_add(u32::from(mem.u8(data)?)))?);
    let at = data.wrapping_add(0x14).wrapping_add(4 * (stride * c + 2 * k));
    Ok((mem.u32(at)?, mem.u32(at.wrapping_add(4))?))
}

/// `(a - b) . (a - b)`.
fn length2(sse: &mut Sse, a: (u32, u32), b: (u32, u32)) -> Result<u32, Error> {
    let dy = sse.sub(a.1, b.1)?;
    let dx = sse.sub(a.0, b.0)?;
    let dx = sse.mul(dx, dx)?;
    let dy = sse.mul(dy, dy)?;
    sse.add(dx, dy)
}

/// Spline 2's parameter for curve `c`: `t` (what XMM0 holds) unless the
/// segment P2..P3 is long enough; 1 when P0..P1 is near zero; `t` again
/// when the two are near equal; else `pow(x, e)` clamped to 0..1, `e`
/// `rsqrtss(|P2 P3|^2)` times the halving-trick root of `|P0 P1|^2`.
fn spline2_t(mem: &dyn GuestMemory, sse: &mut Sse, control: u16, data: u32, c: u32, x: u32, t: u32) -> Result<u32, Error> {
    let p = [0, 1, 2, 3].map(|k| point(mem, data, c, k));
    let [p0, p1, p2, p3] = [p[0]?, p[1]?, p[2]?, p[3]?];
    let far = length2(sse, p2, p3)?;
    if !sse.above(far, mem.u32(SMALL)?)? {
        return Ok(t);
    }
    let near = length2(sse, p0, p1)?;
    let mut st = X87::new(control);
    if small_above(mem, &mut st, Some(near))? {
        return mem.u32(ONE).map_err(Error::from);
    }
    st.fld(near)?;
    st.arith_m(arith_op::SUB, far)?;
    if small_above(mem, &mut st, None)? {
        return Ok(t);
    }
    let root = ((near as i32) >> 1).wrapping_add(0x1fc0_0000) as u32;
    let q = simd::rsqrtss(0, far) as u32;
    st.fld(q)?;
    st.fmul_m(root)?;
    let e = st.fstp_m32()?;
    st.fld(x)?;
    st.fld(e)?;
    pow(mem, &mut st)?;
    let v = st.fst_m32()?;
    st.fld(mem.u32(ZERO)?)?;
    let f = st.fcomip(1)?;
    st.fstp_st(0)?;
    if f & 0x41 == 0 {
        return Ok(0);
    }
    let one = mem.u32(ONE)?;
    Ok(if sse.above(v, one)? { one } else { v })
}

/// The cubic with coefficients at `k` (t^3, t^2, t, 1) at `t`.
fn cubic(mem: &dyn GuestMemory, sse: &mut Sse, k: u32, t: u32) -> Result<u32, Error> {
    let c = |o: u32| mem.u32(k.wrapping_add(o));
    let t2 = sse.mul(t, t)?;
    let t3 = sse.mul(t2, t)?;
    let a = sse.mul(c(4)?, t2)?;
    let b = sse.mul(c(8)?, t)?;
    let v = sse.add(a, b)?;
    let d = sse.mul(c(0)?, t3)?;
    let v = sse.add(v, d)?;
    sse.add(v, c(0xc)?)
}

/// Linear key's three clamped ramps at `p` (`+0x24..` the starts,
/// `+0x34..` the slopes, `+0x40` the constant, `+0x44..` the weights).
fn keys(mem: &dyn GuestMemory, sse: &mut Sse, one: u32, p: u32, x: u32) -> Result<u32, Error> {
    let w = |sse: &mut Sse, i: u32| -> Result<u32, Error> {
        let v = sse.sub(x, mem.u32(p.wrapping_add(0x24 + 4 * i))?)?;
        let v = sse.mul(v, mem.u32(p.wrapping_add(0x34 + 4 * i))?)?;
        sse.clamp(v, 0, one)
    };
    let (w0, w1, w2) = (w(sse, 0)?, w(sse, 1)?, w(sse, 2)?);
    let v = sse.mul(mem.u32(p.wrapping_add(0x4c))?, w2)?;
    let b = sse.mul(mem.u32(p.wrapping_add(0x48))?, w1)?;
    let v = sse.add(v, b)?;
    let a = sse.mul(mem.u32(p.wrapping_add(0x44))?, w0)?;
    let v = sse.add(v, a)?;
    sse.add(v, mem.u32(p.wrapping_add(0x40))?)
}

/// `0x0013b390` with EDI `reference` (count, then the blob's address), at
/// input `x` and range input `r`, as f32 bits; SSE under `sse`'s MXCSR,
/// x87 under `control`.
pub(crate) fn evaluate(
    mem: &dyn GuestMemory,
    sse: &mut Sse,
    control: u16,
    reference: u32,
    x: u32,
    r: u32,
) -> Result<u32, Error> {
    let data = mem.u32(reference.wrapping_add(4))?;
    if data == 0 || mem.u32(reference)? as i32 <= 0 {
        return Ok(0);
    }
    let kind = mem.u8(data)?;
    let blended = mem.u8(data.wrapping_add(1))? & 1 != 0;
    let p = data.wrapping_add(0x14);
    let at = |o: u32| mem.u32(p.wrapping_add(o));
    let one = mem.u32(ONE)?;
    let v = match kind {
        0 => x,
        1 => {
            if !blended {
                return Ok(0);
            }
            r
        }
        2 => {
            let s = shape(mem, sse, control, mem.u8(data.wrapping_add(2))?, x)?;
            let v = lerp(sse, at(0)?, at(4)?, s)?;
            if !blended {
                v
            } else {
                let s = shape(mem, sse, control, mem.u8(data.wrapping_add(3))?, x)?;
                let w = lerp(sse, at(8)?, at(0xc)?, s)?;
                blend(sse, one, v, w, r)?
            }
        }
        3 => {
            let i = sse.mul(at(0)?, x)?;
            let i = sse.add(i, at(4)?)?;
            let s = wave(mem, sse, control, mem.u8(data.wrapping_add(2))?, i)?;
            let v = lerp(sse, at(8)?, at(0xc)?, s)?;
            if !blended {
                v
            } else {
                let i = sse.mul(at(0x10)?, x)?;
                let i = sse.add(i, at(0x14)?)?;
                let s = wave(mem, sse, control, mem.u8(data.wrapping_add(3))?, i)?;
                let w = lerp(sse, at(0x18)?, at(0x1c)?, s)?;
                blend(sse, one, v, w, r)?
            }
        }
        4 => {
            let v = sse.mul(at(0x10)?, x)?;
            let v = sse.add(v, at(0x14)?)?;
            if !blended {
                v
            } else {
                let w = sse.mul(at(0x28)?, x)?;
                let w = sse.add(w, at(0x2c)?)?;
                blend(sse, one, v, w, r)?
            }
        }
        5 => {
            let v = keys(mem, sse, one, p, x)?;
            if !blended {
                v
            } else {
                let w = keys(mem, sse, one, p.wrapping_add(0x50), x)?;
                blend(sse, one, v, w, r)?
            }
        }
        7 | 10 => {
            let t = if kind == 10 { spline2_t(mem, sse, control, data, 0, x, x)? } else { x };
            let v = cubic(mem, sse, data.wrapping_add(0x34), t)?;
            if !blended {
                v
            } else {
                let t = if kind == 10 { spline2_t(mem, sse, control, data, 1, x, t)? } else { t };
                let w = cubic(mem, sse, data.wrapping_add(0x64), t)?;
                blend(sse, one, v, w, r)?
            }
        }
        9 => {
            let v = exponent(mem, sse, control, p, x)?;
            if !blended {
                v
            } else {
                let w = exponent(mem, sse, control, p.wrapping_add(0xc), x)?;
                let rest = sse.sub(one, r)?;
                let rest = sse.mul(rest, v)?;
                let w = sse.mul(w, r)?;
                sse.add(rest, w)?
            }
        }
        _ => return Ok(0),
    };
    sse.clamp(v, 0, one)
}

/// The constants the evaluator, `fmod` and `pow` read, as IEEE values,
/// written into `m`: for the tests of a caller of [`pow`].
#[cfg(test)]
pub(crate) fn put_constants(m: &mut dyn GuestMemory) {
    for (a, v) in [
        (ONE, 1.0f32),
        (ZERO, 0.0),
        (SMALL, 1e-4),
        (SHAPE_SCALE, 1023.0),
        (SHAPE_BIAS, 0.1),
        (WAVE_SCALE, 1024.0 / 28.0),
        (BYTE, 1.0 / 255.0),
        (WRAP_HIGH, 0.75),
        (WRAP_LOW, 0.25),
    ] {
        m.set_f32(a, v).unwrap();
    }
    for (a, v) in [
        (HALF_64, 0.5f64),
        (DBL_MAX, f64::MAX),
        (DBL_MIN, f64::MIN_POSITIVE),
        (DOWN, -1536.0),
        (UP, 1536.0),
        (OVER, f64::INFINITY),
        (UNDER, 0.0),
        (INFINITY_64, f64::INFINITY),
        (MAX_64, f64::MAX),
        (ZERO_64, 0.0),
        (ONE_64, 1.0),
        (NEGATIVE_ZERO_64, -0.0),
    ] {
        m.write(a, &v.to_bits().to_le_bytes()).unwrap();
    }
    m.write(INDEFINITE_64, &0xfff8_0000_0000_0000u64.to_le_bytes()).unwrap();
    // The extended indefinite and +infinity.
    m.write(INDEFINITE, &[0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0xff]).unwrap();
    m.write(INFINITY_80, &[0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f]).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const DATA: u32 = 0x0048_8000;
    const REF: u32 = 0x0048_7ff0;

    /// The constants the evaluator reads, as IEEE values; the tables'
    /// first sub-types filled with ramps.
    fn image() -> FlatMemory {
        let mut m = FlatMemory::new(0x0041_0000, 0x0008_0000);
        put_constants(&mut m);
        for k in 0..=10u32 {
            m.set_u8(SET_WORDS + k, [0, 1, 2, 4, 6, 0x14, 0x20, 12, 4, 3, 12][k as usize]).unwrap();
        }
        for i in 0..1024u32 {
            m.set_u8(SHAPES + 1024 + i, (i / 4) as u8).unwrap();
            m.set_u8(WAVES + 1024 + i, (i / 4) as u8).unwrap();
        }
        m
    }

    fn blob(m: &mut FlatMemory, kind: u8, flags: u8, words: &[f32]) {
        m.write(DATA, &[0u8; 0x200]).unwrap();
        m.set_u8(DATA, kind).unwrap();
        m.set_u8(DATA + 1, flags).unwrap();
        for (i, w) in words.iter().enumerate() {
            m.set_f32(DATA + 0x14 + 4 * i as u32, *w).unwrap();
        }
        m.set_u32(REF, 0x100).unwrap();
        m.set_u32(REF + 4, DATA).unwrap();
    }

    fn eval(m: &FlatMemory, x: f32, r: f32) -> u32 {
        evaluate(m, &mut Sse { mxcsr: 0x1f80 }, 0x27f, REF, x.to_bits(), r.to_bits()).unwrap()
    }

    fn power(m: &FlatMemory, x: f32, p: f32) -> u64 {
        let mut st = X87::new(0x27f);
        st.fld(x.to_bits()).unwrap();
        st.fld(p.to_bits()).unwrap();
        pow(m, &mut st).unwrap();
        fst_m64(&mut st, true).unwrap()
    }

    #[test]
    fn fmod_keeps_the_sign_and_takes_the_crt_special_cases() {
        let m = image();
        let f = |v: f32| fmod_one(&m, 0x27f, v.to_bits()).unwrap();
        assert_eq!(f(2.75), 0.75f32.to_bits());
        assert_eq!(f(-2.5), (-0.5f32).to_bits());
        assert_eq!(f(-2.0), (-0.0f32).to_bits());
        assert_eq!(f(-0.0), 0, "a zero is fldz's +0");
        assert_eq!(f(f32::INFINITY), 0xffc0_0000, "the indefinite");
        assert_eq!(fmod_one(&m, 0x27f, 0x7f80_0001).unwrap(), 0x7fc0_0001, "a NaN quieted");
        assert_eq!(fmod_one(&m, 0x27f, 0xffc0_0002).unwrap(), 0xffc0_0002);
    }

    #[test]
    fn pow_takes_each_path() {
        let m = image();
        let p = |x: f32, e: f32| f64::from_bits(power(&m, x, e));
        assert_eq!(p(2.0, 3.0), 8.0);
        assert_eq!(p(-2.0, 3.0), -8.0, "odd power of a negative base");
        assert_eq!(p(-2.0, 2.0), 4.0);
        assert_eq!(power(&m, -2.0, 0.5), 0xfff8_0000_0000_0000, "the indefinite through a double");
        assert_eq!(p(0.0, 0.0), 1.0);
        assert_eq!(power(&m, 0.0, 3.0), 0);
        assert_eq!(power(&m, -0.0, 3.0), SIGN_64, "-0 to an odd power");
        assert_eq!(power(&m, -0.0, 2.0), 0);
        assert_eq!(p(0.0, -1.0), f64::INFINITY);
        assert_eq!(p(-0.0, -1.0), f64::NEG_INFINITY);
        assert_eq!(p(f32::INFINITY, -1.0), 0.0);
        assert_eq!(p(f32::NEG_INFINITY, 3.0), f64::NEG_INFINITY);
        assert_eq!(power(&m, f32::NEG_INFINITY, -3.0), SIGN_64);
        assert_eq!(p(0.5, f32::INFINITY), 0.0);
        assert_eq!(p(2.0, f32::NEG_INFINITY), 0.0);
        assert_eq!(power(&m, 1.0, f32::INFINITY), 0xfff8_0000_0000_0000, "|x| = 1: the indefinite");
        assert_eq!(power(&m, f32::NAN, 2.0) >> 51, 0xfff, "a NaN base");
        assert!(p(1e30, 20.0).is_infinite(), "an overflow becomes infinity");
        assert_eq!(p(1e-30, 20.0), 0.0, "far past the denormals: zero");
        let d = p(1e-30, 10.5);
        assert!(d > 0.0 && d < f64::MIN_POSITIVE, "a denormal, truncated: {d:e}");
    }

    #[test]
    fn the_basic_curves() {
        let mut m = image();
        blob(&mut m, 0, 0, &[]);
        assert_eq!(eval(&m, 0.25, 0.0), 0.25f32.to_bits());
        assert_eq!(eval(&m, 2.0, 0.0), 1.0f32.to_bits(), "clamped");
        assert_eq!(eval(&m, -2.0, 0.0), 0);
        blob(&mut m, 1, 0, &[]);
        assert_eq!(eval(&m, 0.5, 0.75), 0, "constant, not blended: 0");
        blob(&mut m, 1, 1, &[]);
        assert_eq!(eval(&m, 0.5, 0.75), 0.75f32.to_bits(), "blended: r");
        // Linear: 0.5 x + 0.25; blended with x - 0.5 by r.
        let mut w = [0f32; 12];
        w[4] = 0.5;
        w[5] = 0.25;
        w[10] = 1.0;
        w[11] = -0.5;
        blob(&mut m, 4, 0, &w);
        assert_eq!(eval(&m, 0.5, 0.0), 0.5f32.to_bits());
        blob(&mut m, 4, 1, &w);
        assert_eq!(eval(&m, 1.0, 0.5), 0.625f32.to_bits(), "0.5 * 0.5 + 0.5 * 0.75");
        blob(&mut m, 6, 0, &w);
        assert_eq!(eval(&m, 0.5, 0.0), 0);
        m.set_u32(REF, 0).unwrap();
        assert_eq!(eval(&m, 0.5, 0.0), 0, "an empty reference");
    }

    #[test]
    fn transition_and_periodic_read_the_tables() {
        let mut m = image();
        // Shape 1 is the ramp i / 4 / 255; at x = 0.4 the index is
        // round(409.2 - 0.1) = 409 and the fraction 0.2: bytes 102, 102.
        blob(&mut m, 2, 0, &[0.0, 1.0]);
        m.set_u8(DATA + 2, 1).unwrap();
        let s = f32::from_bits(eval(&m, 0.4, 0.0));
        assert!((s - 102.0 / 255.0).abs() < 1e-6, "{s}");
        // Sub-type 0: x itself.
        m.set_u8(DATA + 2, 0).unwrap();
        assert_eq!(eval(&m, 0.5, 0.0), 0.5f32.to_bits());
        // Periodic, sub-type 0: always the top of its range.
        blob(&mut m, 3, 0, &[1.0, 0.0, 0.25, 0.75]);
        assert_eq!(eval(&m, 0.3, 0.0), 0.75f32.to_bits());
    }

    #[test]
    fn exponent_is_x87_and_near_zero_exponents_are_one() {
        let mut m = image();
        blob(&mut m, 9, 0, &[0.0, 1.0, 2.0]);
        assert_eq!(eval(&m, 0.5, 0.0), 0.25f32.to_bits());
        blob(&mut m, 9, 0, &[0.0, 0.5, 0.00001]);
        assert_eq!(eval(&m, 0.5, 0.0), 1.0f32.to_bits(), "1, not mapped through 0..0.5");
        blob(&mut m, 9, 0, &[0.0, 1.0, 0.5]);
        assert_eq!(eval(&m, -0.25, 0.0), 0xffc0_0000, "a negative base to a fraction: the indefinite");
        // Blended: (1 - r) * first + second * r.
        blob(&mut m, 9, 1, &[0.0, 1.0, 2.0, 0.0, 1.0, 1.0]);
        assert_eq!(eval(&m, 0.5, 0.5), 0.375f32.to_bits());
    }

    #[test]
    fn splines() {
        let mut m = image();
        // Spline: t^3 coefficient 0, t^2 1, t 0, constant 0.
        let mut w = [0f32; 30];
        w[9] = 1.0;
        blob(&mut m, 7, 0, &w);
        assert_eq!(eval(&m, 0.5, 0.0), 0.25f32.to_bits());
        // Spline 2 with equal segments: t = x.
        let mut w = [0f32; 30];
        w[2] = 1.0; // P1 = (1, 0)
        w[4] = 2.0; // P2 = (2, 0)
        w[6] = 3.0; // P3 = (3, 0)
        w[9] = 1.0;
        blob(&mut m, 10, 0, &w);
        assert_eq!(eval(&m, 0.5, 0.0), 0.25f32.to_bits());
        // A short P2..P3: t = x as well.
        w[6] = 2.0;
        blob(&mut m, 10, 0, &w);
        assert_eq!(eval(&m, 0.5, 0.0), 0.25f32.to_bits());
        // P0..P1 near zero: t = 1.
        w[2] = 0.0;
        w[6] = 3.0;
        blob(&mut m, 10, 0, &w);
        assert_eq!(eval(&m, 0.5, 0.0), 1.0f32.to_bits());
        // P0..P1 twice P2..P3: t = pow(x, about 2).
        w[2] = 2.0;
        blob(&mut m, 10, 0, &w);
        let t = f32::from_bits(eval(&m, 0.5, 0.0)).sqrt();
        assert!((t - 0.25).abs() < 2e-3, "{t}");
    }
}
