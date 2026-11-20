//! Bit-exact, MXCSR-exact reference semantics for the Pentium III's SSE1
//! floating point, for the IR interpreter. `c/h2simd.h` is the same code in C
//! for the generated code; the two give bit-identical results, and the tests
//! check that (`tests/differential.rs`) and check both against SSE hardware
//! results (`tests/hardware.rs`). The semantics are written up in
//! `docs/lifter/simd.md`.
//!
//! Everything is integer soft-float: no host floating point is involved.
//!
//! # Conventions
//!
//! * Registers are `u128` (lane `i` is bits `32*i..32*i+32`), scalars `u32`,
//!   MMX registers `u64`.
//! * Scalar `ss` forms compute lane 0 and keep lanes 1-3 of `a`.
//! * Functions that take `mxcsr` OR the sticky exception flags they detect
//!   into it, on `Ok` and on `Err` alike. `Err(Xm)` means an unmasked
//!   exception: the instruction raises `#XM` and its destination (register or
//!   EFLAGS) is not written.
//! * DAZ (MXCSR bit 6) is ignored: the Pentium III does not implement it and
//!   `ldmxcsr` rejects it.

#![no_std]
#![allow(clippy::result_unit_err)]

/// An unmasked SIMD floating-point exception: the instruction raises `#XM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Xm;

/// MXCSR exception flags (bits 0-5); the masks are the same bits shifted
/// left by [`MASK_SHIFT`].
pub const IE: u32 = 0x01;
pub const DE: u32 = 0x02;
pub const ZE: u32 = 0x04;
pub const OE: u32 = 0x08;
pub const UE: u32 = 0x10;
pub const PE: u32 = 0x20;
pub const MASK_SHIFT: u32 = 7;
/// Rounding control, bits 13-14: 0 nearest even, 1 down, 2 up, 3 toward zero.
pub const RC_SHIFT: u32 = 13;
/// Flush to zero.
pub const FTZ: u32 = 0x8000;
/// MXCSR at reset: everything masked, round to nearest.
pub const MXCSR_DEFAULT: u32 = 0x1F80;

/// The default (indefinite) NaN, the result of an invalid operation.
pub const DEFAULT_NAN: u32 = 0xFFC0_0000;
/// The integer indefinite value from an invalid conversion.
pub const INT_INDEFINITE: u32 = 0x8000_0000;

const PRE: u32 = IE | DE | ZE;
const QUIET: u32 = 0x0040_0000;
const UM: u32 = UE << MASK_SHIFT;
const OM: u32 = OE << MASK_SHIFT;

// ---- classification -------------------------------------------------------

fn is_nan(a: u32) -> bool {
    a & 0x7FFF_FFFF > 0x7F80_0000
}
fn is_snan(a: u32) -> bool {
    is_nan(a) && a & QUIET == 0
}
fn is_inf(a: u32) -> bool {
    a & 0x7FFF_FFFF == 0x7F80_0000
}
fn is_zero(a: u32) -> bool {
    a & 0x7FFF_FFFF == 0
}
fn is_denormal(a: u32) -> bool {
    a & 0x7F80_0000 == 0 && a & 0x007F_FFFF != 0
}
fn de2(a: u32, b: u32) -> u32 {
    if is_denormal(a) || is_denormal(b) {
        DE
    } else {
        0
    }
}
/// The NaN result of a two-operand arithmetic op with a NaN operand: the
/// first source's NaN wins, quieted.
fn nan2(a: u32, b: u32) -> (u32, u32) {
    let r = if is_nan(a) { a | QUIET } else { b | QUIET };
    (r, if is_snan(a) || is_snan(b) { IE } else { 0 })
}

/// Unpacks a finite non-zero value as `m * 2^(e - 150)`, `m` with bit 23 set.
fn unpack_norm(a: u32) -> (i32, u32) {
    let exp = ((a >> 23) & 0xFF) as i32;
    let frac = a & 0x007F_FFFF;
    if exp == 0 {
        let shift = frac.leading_zeros() as i32 - 8;
        (1 - shift, frac << shift)
    } else {
        (exp, frac | 0x0080_0000)
    }
}

fn shift_right_jam32(a: u32, dist: u32) -> u32 {
    if dist == 0 {
        a
    } else if dist < 31 {
        (a >> dist) | ((a << (32 - dist)) != 0) as u32
    } else {
        (a != 0) as u32
    }
}
fn shift_right_jam64(a: u64, dist: u32) -> u64 {
    if dist == 0 {
        a
    } else if dist < 64 {
        (a >> dist) | ((a << (64 - dist)) != 0) as u64
    } else {
        (a != 0) as u64
    }
}

// ---- rounding ---------------------------------------------------------------

/// Rounds `sig * 2^(exp - 156)` (`sig` below 2^31, bit 30 the leading bit
/// for a normal result, bits 0-6 the round bits) to binary32 under MXCSR's
/// rounding control and FTZ. Returns the masked-response result and the
/// post-computation flags. Tininess is detected after rounding.
fn round_pack(mx: u32, sign: u32, mut exp: i32, mut sig: u32) -> (u32, u32) {
    let rc = (mx >> RC_SHIFT) & 3;
    let near_even = rc == 0;
    let inc: u32 = match rc {
        0 => 0x40,
        1 => {
            if sign != 0 {
                0x7F
            } else {
                0
            }
        }
        2 => {
            if sign != 0 {
                0
            } else {
                0x7F
            }
        }
        _ => 0,
    };
    let mut flags = 0;
    let mut round_bits = sig & 0x7F;
    if exp as u32 >= 0xFD {
        if exp < 0 {
            let tiny = exp < -1 || sig + inc < 0x8000_0000;
            if tiny && mx & UM == 0 {
                // Unmasked underflow traps before denormalisation: PE says
                // whether rounding to 24 bits (unbounded exponent) was
                // inexact. FTZ does not apply. The value is never stored.
                return (sign << 31, UE | if round_bits != 0 { PE } else { 0 });
            }
            if tiny && mx & FTZ != 0 {
                return (sign << 31, UE | PE);
            }
            sig = shift_right_jam32(sig, exp.unsigned_abs());
            exp = 0;
            round_bits = sig & 0x7F;
            if tiny && round_bits != 0 {
                flags |= UE;
            }
        } else if exp > 0xFD || sig + inc >= 0x8000_0000 {
            if mx & OM == 0 {
                // Unmasked overflow: PE only for an inexact 24-bit rounding.
                return (sign << 31, OE | if round_bits != 0 { PE } else { 0 });
            }
            return (((sign << 31) | 0x7F80_0000) - (inc == 0) as u32, OE | PE);
        }
    }
    if round_bits != 0 {
        flags |= PE;
    }
    sig = (sig + inc) >> 7;
    if near_even && round_bits == 0x40 {
        sig &= !1;
    }
    if sig == 0 {
        exp = 0;
    }
    ((sign << 31) + ((exp as u32) << 23) + sig, flags)
}

/// Normalizes and rounds `m * 2^q` (`0 < m < 2^63`).
fn norm_round_pack(mx: u32, sign: u32, q: i32, m: u64) -> (u32, u32) {
    let s = m.leading_zeros() as i32 - 1;
    let m = m << s;
    let sig = (m >> 32) as u32 | (m as u32 != 0) as u32;
    round_pack(mx, sign, q - s + 32 + 156, sig)
}

fn rc_down(mx: u32) -> bool {
    (mx >> RC_SHIFT) & 3 == 1
}

// ---- per-element operations: (masked-response result, flags) -------------

fn add1(mx: u32, a: u32, b_in: u32, negate: bool) -> (u32, u32) {
    if is_nan(a) || is_nan(b_in) {
        return nan2(a, b_in);
    }
    let b = if negate { b_in ^ 0x8000_0000 } else { b_in };
    if is_inf(a) {
        if is_inf(b) && (a ^ b) >> 31 != 0 {
            return (DEFAULT_NAN, IE);
        }
        return (a, de2(a, b));
    }
    if is_inf(b) {
        return (b, de2(a, b));
    }
    let flags = de2(a, b);
    if is_zero(a) && is_zero(b) {
        let r = if a == b {
            a
        } else if rc_down(mx) {
            0x8000_0000
        } else {
            0
        };
        return (r, flags);
    }
    let (r, f) = if is_zero(a) {
        let (e, m) = unpack_norm(b);
        norm_round_pack(mx, b >> 31, e - 150, m as u64)
    } else if is_zero(b) {
        let (e, m) = unpack_norm(a);
        norm_round_pack(mx, a >> 31, e - 150, m as u64)
    } else {
        let (mut ea, mut ma) = unpack_norm(a);
        let (mut eb, mut mb) = unpack_norm(b);
        let (mut sa, mut sb) = (a >> 31, b >> 31);
        if eb > ea {
            core::mem::swap(&mut ea, &mut eb);
            core::mem::swap(&mut ma, &mut mb);
            core::mem::swap(&mut sa, &mut sb);
        }
        let x = (ma as u64) << 32;
        let y = shift_right_jam64((mb as u64) << 32, (ea - eb) as u32);
        let (sign, m) = if sa == sb {
            (sa, x + y)
        } else if x >= y {
            (sa, x - y)
        } else {
            (sb, y - x)
        };
        if m == 0 {
            return (if rc_down(mx) { 0x8000_0000 } else { 0 }, flags);
        }
        norm_round_pack(mx, sign, ea - 150 - 32, m)
    };
    (r, flags | f)
}

fn mul1(mx: u32, a: u32, b: u32) -> (u32, u32) {
    if is_nan(a) || is_nan(b) {
        return nan2(a, b);
    }
    let sign = (a ^ b) >> 31;
    if is_inf(a) || is_inf(b) {
        if is_zero(a) || is_zero(b) {
            return (DEFAULT_NAN, IE);
        }
        return ((sign << 31) | 0x7F80_0000, de2(a, b));
    }
    let flags = de2(a, b);
    if is_zero(a) || is_zero(b) {
        return (sign << 31, flags);
    }
    let (ea, ma) = unpack_norm(a);
    let (eb, mb) = unpack_norm(b);
    let (r, f) = norm_round_pack(mx, sign, ea + eb - 300, ma as u64 * mb as u64);
    (r, flags | f)
}

fn div1(mx: u32, a: u32, b: u32) -> (u32, u32) {
    if is_nan(a) || is_nan(b) {
        return nan2(a, b);
    }
    let sign = (a ^ b) >> 31;
    if is_inf(a) {
        if is_inf(b) {
            return (DEFAULT_NAN, IE);
        }
        return ((sign << 31) | 0x7F80_0000, de2(a, b));
    }
    if is_inf(b) {
        return (sign << 31, de2(a, b));
    }
    if is_zero(b) {
        if is_zero(a) {
            return (DEFAULT_NAN, IE);
        }
        // Divide-by-zero outranks the denormal-operand exception.
        return ((sign << 31) | 0x7F80_0000, ZE);
    }
    let flags = de2(a, b);
    if is_zero(a) {
        return (sign << 31, flags);
    }
    let (ea, ma) = unpack_norm(a);
    let (eb, mb) = unpack_norm(b);
    let n = (ma as u64) << 39;
    let q = n / mb as u64;
    let q = q | (q * mb as u64 != n) as u64;
    let (r, f) = norm_round_pack(mx, sign, ea - eb - 39, q);
    (r, flags | f)
}

/// Integer square root of `x`: (floor(sqrt(x)), remainder).
fn isqrt64(x: u64) -> (u64, u64) {
    let mut op = x;
    let mut res = 0u64;
    let mut one = 1u64 << 62;
    while one > op {
        one >>= 2;
    }
    while one != 0 {
        if op >= res + one {
            op -= res + one;
            res = (res >> 1) + one;
        } else {
            res >>= 1;
        }
        one >>= 2;
    }
    (res, op)
}

fn sqrt1(mx: u32, b: u32) -> (u32, u32) {
    if is_nan(b) {
        return (b | QUIET, if is_snan(b) { IE } else { 0 });
    }
    if is_zero(b) {
        return (b, 0);
    }
    if b >> 31 != 0 {
        // Invalid outranks the denormal-operand exception.
        return (DEFAULT_NAN, IE);
    }
    if is_inf(b) {
        return (b, 0);
    }
    let flags = de2(b, 0);
    let (e, m) = unpack_norm(b);
    let mut k = e - 150;
    let mut m = m as u64;
    if k & 1 != 0 {
        m <<= 1;
        k -= 1;
    }
    let (r, rem) = isqrt64(m << 38);
    let (res, f) = norm_round_pack(mx, 0, (k - 38) / 2 - 1, (r << 1) | (rem != 0) as u64);
    (res, flags | f)
}

/// Ordered `a < b` for non-NaN operands; -0 equals +0.
fn lt(a: u32, b: u32) -> bool {
    if is_zero(a) && is_zero(b) {
        return false;
    }
    let (sa, sb) = (a >> 31, b >> 31);
    if sa != sb {
        return sa != 0;
    }
    if sa == 0 { a < b } else { a > b }
}
fn eq(a: u32, b: u32) -> bool {
    a == b || (is_zero(a) && is_zero(b))
}

/// `minss`/`maxss`: the second operand on any NaN (raising IE, even for a
/// QNaN; the NaN is returned unquieted) and when both are zero.
fn minmax1(a: u32, b: u32, max: bool) -> (u32, u32) {
    if is_nan(a) || is_nan(b) {
        return (b, IE);
    }
    let flags = de2(a, b);
    let pick_a = if max { lt(b, a) } else { lt(a, b) };
    (if pick_a { a } else { b }, flags)
}

fn cmp1(a: u32, b: u32, pred: u8) -> (u32, u32) {
    let pred = pred & 7;
    let un = is_nan(a) || is_nan(b);
    let signalling = matches!(pred, 1 | 2 | 5 | 6);
    let flags = if is_snan(a) || is_snan(b) || (un && signalling) {
        IE
    } else if un {
        0
    } else {
        de2(a, b)
    };
    let t = match pred {
        0 => !un && eq(a, b),
        1 => !un && lt(a, b),
        2 => !un && (lt(a, b) || eq(a, b)),
        3 => un,
        4 => un || !eq(a, b),
        5 => un || !lt(a, b),
        6 => un || !(lt(a, b) || eq(a, b)),
        _ => !un,
    };
    (if t { u32::MAX } else { 0 }, flags)
}

fn comi1(a: u32, b: u32, signalling: bool) -> (u32, u32) {
    if is_nan(a) || is_nan(b) {
        let ie = is_snan(a) || is_snan(b) || signalling;
        return (0x45, if ie { IE } else { 0 });
    }
    let r = if lt(a, b) {
        0x01
    } else if eq(a, b) {
        0x40
    } else {
        0
    };
    (r, de2(a, b))
}

fn i2f1(mx: u32, i: u32) -> (u32, u32) {
    if i == 0 {
        return (0, 0);
    }
    let sign = i >> 31;
    let mag = if sign != 0 { i.wrapping_neg() } else { i };
    norm_round_pack(mx, sign, 0, mag as u64)
}

/// `cvtss2si` (`truncate` false: MXCSR rounding) and `cvttss2si`.
fn f2i1(mx: u32, a: u32, truncate: bool) -> (u32, u32) {
    let exp = (a >> 23) & 0xFF;
    let sign = a >> 31;
    if exp >= 127 + 32 {
        return (INT_INDEFINITE, IE);
    }
    let (e, m) = if exp == 0 {
        (1, a & 0x007F_FFFF)
    } else {
        (exp as i32, (a & 0x007F_FFFF) | 0x0080_0000)
    };
    // Fixed point with 32 fraction bits.
    let sh = e - 118;
    let f = if sh >= 0 {
        (m as u64) << sh
    } else {
        shift_right_jam64(m as u64, sh.unsigned_abs())
    };
    let mut int = f >> 32;
    let frac = f as u32;
    let rc = if truncate { 3 } else { (mx >> RC_SHIFT) & 3 };
    let up = match rc {
        0 => frac > 0x8000_0000 || (frac == 0x8000_0000 && int & 1 != 0),
        1 => sign != 0 && frac != 0,
        2 => sign == 0 && frac != 0,
        _ => false,
    };
    int += up as u64;
    if int > 0x7FFF_FFFF + sign as u64 {
        return (INT_INDEFINITE, IE);
    }
    let r = if sign != 0 {
        (int as u32).wrapping_neg()
    } else {
        int as u32
    };
    (r, if frac != 0 { PE } else { 0 })
}

/// The Pentium III's `rcpss` approximation (no exceptions, MXCSR ignored).
///
/// The input's top 11 fraction bits select an interval; the result's
/// significand is 1/midpoint of that interval rounded to nearest at 12
/// fraction bits, and its exponent is the input's mirrored. Measured Intel
/// tables match this rule for every entry (docs/lifter/simd.md). Zero and denormal
/// inputs give a signed infinity; results below the normal range flush to a
/// signed zero; infinity gives a signed zero; NaNs are quieted.
pub fn rcp_approx(x: u32) -> u32 {
    let s = x & 0x8000_0000;
    let e = (x >> 23) & 0xFF;
    let f = x & 0x007F_FFFF;
    if e == 0xFF {
        return if f != 0 { x | QUIET } else { s };
    }
    if e == 0 {
        return s | 0x7F80_0000;
    }
    let d = 4097 + 2 * (f >> 12);
    // round(2^25 / d), in 4097..=8190.
    let n = ((1u32 << 26) + d) / (2 * d);
    let oe = 253 - e as i32;
    if oe <= 0 {
        return s;
    }
    s | ((oe as u32) << 23) | ((n - 4096) << 11)
}

/// The Pentium III's `rsqrtss` approximation (no exceptions, MXCSR ignored).
///
/// The input's exponent parity and top 10 fraction bits select an interval
/// of [1, 4); the result's significand is 1/sqrt(midpoint) rounded to nearest
/// at 12 fraction bits. Zero and denormal inputs give a signed infinity
/// (-0 and negative denormals give -inf), other negative inputs and -inf the
/// default NaN, +inf gives +0, NaNs are quieted.
pub fn rsqrt_approx(x: u32) -> u32 {
    let s = x & 0x8000_0000;
    let e = (x >> 23) & 0xFF;
    let f = x & 0x007F_FFFF;
    if e == 0xFF {
        return if f != 0 {
            x | QUIET
        } else if s != 0 {
            DEFAULT_NAN
        } else {
            0
        };
    }
    if e == 0 {
        return s | 0x7F80_0000;
    }
    if s != 0 {
        return DEFAULT_NAN;
    }
    // p = 1 when the unbiased exponent is odd: the input is in [2, 4).
    let p = (e & 1) ^ 1;
    let d = 2049 + 2 * (f >> 13);
    // round(sqrt(2^(37 - p) / d)) = (floor(sqrt(floor(2^(39 - p) / d))) + 1) / 2
    let (m, _) = isqrt64((1u64 << (39 - p)) / d as u64);
    let n = ((m + 1) >> 1) as u32;
    let oe = 126 - (e as i32 - 127 - p as i32) / 2;
    ((oe as u32) << 23) | ((n - 4096) << 11)
}

// ---- lanes and exception delivery -------------------------------------------

fn lane(v: u128, i: u32) -> u32 {
    (v >> (32 * i)) as u32
}
fn with_lane0(a: u128, x: u32) -> u128 {
    (a & !(u32::MAX as u128)) | x as u128
}

/// Records `flags` (the OR over all elements) in MXCSR and decides whether
/// the instruction completes. An unmasked pre-computation exception (IE, DE,
/// ZE) in any element stops the instruction before post-computation
/// exceptions are looked at, so only the pre-computation flags are recorded.
fn deliver(mxcsr: &mut u32, flags: u32) -> Result<(), Xm> {
    let unmasked = !(*mxcsr >> MASK_SHIFT) & 0x3F;
    if flags & PRE & unmasked != 0 {
        *mxcsr |= flags & PRE;
        return Err(Xm);
    }
    *mxcsr |= flags;
    if flags & unmasked != 0 {
        Err(Xm)
    } else {
        Ok(())
    }
}

fn scalar(mxcsr: &mut u32, a: u128, (r, flags): (u32, u32)) -> Result<u128, Xm> {
    deliver(mxcsr, flags)?;
    Ok(with_lane0(a, r))
}

fn packed(
    mxcsr: &mut u32,
    a: u128,
    b: u128,
    op: impl Fn(u32, u32) -> (u32, u32),
) -> Result<u128, Xm> {
    let mut flags = 0;
    let mut out = 0u128;
    for i in 0..4 {
        let (r, f) = op(lane(a, i), lane(b, i));
        flags |= f;
        out |= (r as u128) << (32 * i);
    }
    deliver(mxcsr, flags)?;
    Ok(out)
}

// ---- the instructions ----------------------------------------------------------

macro_rules! arith {
    ($ss:ident, $ps:ident, $doc:literal, |$m:ident, $x:ident, $y:ident| $e:expr) => {
        #[doc = concat!("`", stringify!($ss), "`: lane 0 = ", $doc, "; lanes 1-3 from `a`.")]
        pub fn $ss(mxcsr: &mut u32, a: u128, b: u32) -> Result<u128, Xm> {
            let $m = *mxcsr;
            let ($x, $y) = (lane(a, 0), b);
            scalar(mxcsr, a, $e)
        }
        #[doc = concat!("`", stringify!($ps), "`: each lane = ", $doc, ".")]
        pub fn $ps(mxcsr: &mut u32, a: u128, b: u128) -> Result<u128, Xm> {
            let $m = *mxcsr;
            packed(mxcsr, a, b, |$x, $y| $e)
        }
    };
}

arith!(addss, addps, "a + b", |m, x, y| add1(m, x, y, false));
arith!(subss, subps, "a - b", |m, x, y| add1(m, x, y, true));
arith!(mulss, mulps, "a * b", |m, x, y| mul1(m, x, y));
arith!(divss, divps, "a / b", |m, x, y| div1(m, x, y));
arith!(
    minss,
    minps,
    "min(a, b), b on NaN or two zeros",
    |_m, x, y| minmax1(x, y, false)
);
arith!(
    maxss,
    maxps,
    "max(a, b), b on NaN or two zeros",
    |_m, x, y| minmax1(x, y, true)
);

/// `sqrtss`: lane 0 = sqrt(b); lanes 1-3 from `a`.
pub fn sqrtss(mxcsr: &mut u32, a: u128, b: u32) -> Result<u128, Xm> {
    let m = *mxcsr;
    scalar(mxcsr, a, sqrt1(m, b))
}
/// `sqrtps`: each lane = sqrt(b).
pub fn sqrtps(mxcsr: &mut u32, b: u128) -> Result<u128, Xm> {
    let m = *mxcsr;
    packed(mxcsr, b, b, |_, y| sqrt1(m, y))
}

/// `cmpss`: lane 0 = all ones if `a <pred> b` else 0; lanes 1-3 from `a`.
/// `pred` is imm8, of which bits 0-2 are used: 0 eq, 1 lt, 2 le, 3 unord,
/// 4 neq, 5 nlt, 6 nle, 7 ord.
pub fn cmpss(mxcsr: &mut u32, a: u128, b: u32, pred: u8) -> Result<u128, Xm> {
    scalar(mxcsr, a, cmp1(lane(a, 0), b, pred))
}
/// `cmpps`: [`cmpss`] on every lane.
pub fn cmpps(mxcsr: &mut u32, a: u128, b: u128, pred: u8) -> Result<u128, Xm> {
    packed(mxcsr, a, b, |x, y| cmp1(x, y, pred))
}

fn eflags(mxcsr: &mut u32, (r, flags): (u32, u32)) -> Result<u32, Xm> {
    deliver(mxcsr, flags)?;
    Ok(r)
}
/// `comiss`: EFLAGS ZF (0x40), PF (0x04) and CF (0x01) as the compare sets
/// them (unordered 0x45, less 0x01, equal 0x40, greater 0); the caller
/// clears OF, SF and AF. IE on any NaN.
pub fn comiss(mxcsr: &mut u32, a: u32, b: u32) -> Result<u32, Xm> {
    eflags(mxcsr, comi1(a, b, true))
}
/// `ucomiss`: as [`comiss`], but IE only on an SNaN.
pub fn ucomiss(mxcsr: &mut u32, a: u32, b: u32) -> Result<u32, Xm> {
    eflags(mxcsr, comi1(a, b, false))
}

/// `cvtsi2ss`: lane 0 = (float) (int32) i under MXCSR rounding.
pub fn cvtsi2ss(mxcsr: &mut u32, a: u128, i: u32) -> Result<u128, Xm> {
    let m = *mxcsr;
    scalar(mxcsr, a, i2f1(m, i))
}
/// `cvtpi2ps`: lanes 0-1 = the two int32s of `mm`; lanes 2-3 from `a`.
pub fn cvtpi2ps(mxcsr: &mut u32, a: u128, mm: u64) -> Result<u128, Xm> {
    let m = *mxcsr;
    let (r0, f0) = i2f1(m, mm as u32);
    let (r1, f1) = i2f1(m, (mm >> 32) as u32);
    deliver(mxcsr, f0 | f1)?;
    Ok((a & !(u64::MAX as u128)) | r0 as u128 | (r1 as u128) << 32)
}
/// `cvtss2si`: (int32) a under MXCSR rounding; 0x80000000 and IE when out
/// of range or NaN.
pub fn cvtss2si(mxcsr: &mut u32, a: u32) -> Result<u32, Xm> {
    let m = *mxcsr;
    eflags(mxcsr, f2i1(m, a, false))
}
/// `cvttss2si`: as [`cvtss2si`], truncating.
pub fn cvttss2si(mxcsr: &mut u32, a: u32) -> Result<u32, Xm> {
    let m = *mxcsr;
    eflags(mxcsr, f2i1(m, a, true))
}
fn f2i2(mxcsr: &mut u32, a: u64, truncate: bool) -> Result<u64, Xm> {
    let m = *mxcsr;
    let (r0, f0) = f2i1(m, a as u32, truncate);
    let (r1, f1) = f2i1(m, (a >> 32) as u32, truncate);
    deliver(mxcsr, f0 | f1)?;
    Ok(r0 as u64 | (r1 as u64) << 32)
}
/// `cvtps2pi`: [`cvtss2si`] on the two low lanes (`a` = lanes 0-1).
pub fn cvtps2pi(mxcsr: &mut u32, a: u64) -> Result<u64, Xm> {
    f2i2(mxcsr, a, false)
}
/// `cvttps2pi`: [`cvttss2si`] on the two low lanes.
pub fn cvttps2pi(mxcsr: &mut u32, a: u64) -> Result<u64, Xm> {
    f2i2(mxcsr, a, true)
}

/// `rcpss`: lane 0 = [`rcp_approx`]`(b)`; lanes 1-3 from `a`.
pub fn rcpss(a: u128, b: u32) -> u128 {
    with_lane0(a, rcp_approx(b))
}
/// `rcpps`: [`rcp_approx`] on every lane.
pub fn rcpps(b: u128) -> u128 {
    (0..4).fold(0, |acc, i| {
        acc | (rcp_approx(lane(b, i)) as u128) << (32 * i)
    })
}
/// `rsqrtss`: lane 0 = [`rsqrt_approx`]`(b)`; lanes 1-3 from `a`.
pub fn rsqrtss(a: u128, b: u32) -> u128 {
    with_lane0(a, rsqrt_approx(b))
}
/// `rsqrtps`: [`rsqrt_approx`] on every lane.
pub fn rsqrtps(b: u128) -> u128 {
    (0..4).fold(0, |acc, i| {
        acc | (rsqrt_approx(lane(b, i)) as u128) << (32 * i)
    })
}

#[cfg(test)]
mod tests;
