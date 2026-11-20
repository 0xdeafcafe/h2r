//! Unpacking, the single rounding step, and NaN propagation.
#![forbid(unsafe_code)]

use crate::{Class, F80, Flags, Rc};

pub(crate) const SIGN: u16 = 0x8000;

/// A finite nonzero operand: `value = (-1)^neg * sig * 2^exp`, `sig`
/// normalised (bit 63 set). `denorm` records that the encoding was a
/// denormal or pseudo-denormal (for DE).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fin {
    pub neg: bool,
    pub sig: u64,
    pub exp: i32,
    pub denorm: bool,
}

impl Fin {
    /// Exponent of the leading bit (unbiased).
    pub fn top(&self) -> i32 {
        self.exp + 63
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Unp {
    Zero(bool),
    Fin(Fin),
    Inf(bool),
    Nan,
    Bad,
}

impl Unp {
    pub fn denorm(&self) -> bool {
        matches!(self, Unp::Fin(f) if f.denorm)
    }
    pub fn negate(self) -> Unp {
        match self {
            Unp::Zero(n) => Unp::Zero(!n),
            Unp::Inf(n) => Unp::Inf(!n),
            Unp::Fin(mut f) => {
                f.neg = !f.neg;
                Unp::Fin(f)
            }
            o => o,
        }
    }
}

pub(crate) fn unpack(x: F80) -> Unp {
    let neg = x.se & SIGN != 0;
    match x.classify() {
        Class::Zero => Unp::Zero(neg),
        Class::Inf => Unp::Inf(neg),
        Class::QNaN | Class::SNaN => Unp::Nan,
        Class::Unsupported => Unp::Bad,
        Class::Normal => Unp::Fin(Fin {
            neg,
            sig: x.sig,
            exp: (x.se & 0x7fff) as i32 - 16446,
            denorm: false,
        }),
        Class::Denormal => {
            // Effective biased exponent 1, no hidden bit.
            let lz = x.sig.leading_zeros();
            Unp::Fin(Fin {
                neg,
                sig: x.sig << lz,
                exp: -16445 - lz as i32,
                denorm: true,
            })
        }
    }
}

pub(crate) const fn zero(neg: bool) -> F80 {
    F80 {
        sig: 0,
        se: if neg { SIGN } else { 0 },
    }
}

pub(crate) const fn inf(neg: bool) -> F80 {
    F80 {
        sig: 1 << 63,
        se: 0x7fff | if neg { SIGN } else { 0 },
    }
}

/// Drop the low `drop` bits of `m` (1..=127) under `rc`. Returns the kept
/// value (possibly carried one bit wider), whether anything nonzero was
/// dropped, and whether the kept value was incremented.
pub(crate) fn rnd(m: u128, drop: u32, neg: bool, rc: Rc) -> (u128, bool, bool) {
    debug_assert!((1..=127).contains(&drop));
    let q = m >> drop;
    let low = m & ((1u128 << drop) - 1);
    let half = 1u128 << (drop - 1);
    let inc = match rc {
        Rc::Near => low > half || (low == half && q & 1 == 1),
        Rc::Chop => false,
        Rc::Up => !neg && low != 0,
        Rc::Down => neg && low != 0,
    };
    (q + inc as u128, low != 0, inc)
}

/// `m >> s` with every shifted-out bit ORed into bit 0.
pub(crate) fn shr_sticky(m: u128, s: u64) -> u128 {
    if s == 0 {
        m
    } else if s >= 128 {
        (m != 0) as u128
    } else {
        (m >> s) | ((m & ((1u128 << s) - 1)) != 0) as u128
    }
}

/// Round `value = (m + tail*eps) * 2^exp` (with `0 < eps < 1`, `tail` in
/// {-1, 0, 1} on the magnitude) once to `prec` significand bits under `rc`
/// and pack it with the 15-bit exponent range.
///
/// Gradual underflow follows the x87 / SoftFloat `extF80` rule: a denormal
/// keeps the rounding point fixed in the 64-bit field, so at PC=24/53 a
/// denormal has fewer than `prec` significant bits. Tininess is detected
/// after rounding (x86): a tiny result carries [`Flags::TINY`], and UE is
/// reported, as for a masked underflow, only when it is also inexact.
/// Overflow gives the masked response.
pub(crate) fn round_pack(
    neg: bool,
    m: u128,
    exp: i64,
    tail: i8,
    prec: u32,
    rc: Rc,
) -> (F80, Flags) {
    debug_assert!(m != 0);
    let mut sh = m.leading_zeros() as i64;
    let mut m = m << sh;
    if tail < 0 {
        m -= 1;
        if m >> 127 == 0 {
            m <<= 1;
            sh += 1;
        }
    }
    if tail != 0 {
        m |= 1;
    }
    // Biased exponent of the leading bit.
    let e = exp - sh + 127 + 16383;
    let drop = 128 - prec;
    let sign = if neg { SIGN } else { 0 };
    let mut flags = Flags::NONE;
    if e >= 1 {
        let (mut q, inexact, up) = rnd(m, drop, neg, rc);
        let mut e = e;
        if q >> prec != 0 {
            q >>= 1;
            e += 1;
        }
        if e >= 0x7fff {
            return overflow(neg, prec, rc);
        }
        if inexact {
            flags |= Flags::PE;
        }
        if up {
            flags |= Flags::C1;
        }
        (
            F80 {
                sig: (q as u64) << (64 - prec),
                se: sign | e as u16,
            },
            flags,
        )
    } else {
        let tiny = e < 0 || rnd(m, drop, neg, rc).0 >> prec == 0;
        let m2 = shr_sticky(m, (1 - e) as u64);
        let (q, inexact, up) = rnd(m2, drop, neg, rc);
        let sig = (q as u64) << (64 - prec);
        if tiny {
            flags |= Flags::TINY;
        }
        if inexact {
            flags |= Flags::PE;
            if tiny {
                flags |= Flags::UE;
            }
        }
        if up {
            flags |= Flags::C1;
        }
        (
            F80 {
                sig,
                se: sign | (sig >> 63) as u16,
            },
            flags,
        )
    }
}

/// Masked overflow response at `prec` bits.
pub(crate) fn overflow(neg: bool, prec: u32, rc: Rc) -> (F80, Flags) {
    let to_inf = match rc {
        Rc::Near => true,
        Rc::Chop => false,
        Rc::Up => !neg,
        Rc::Down => neg,
    };
    let sign = if neg { SIGN } else { 0 };
    if to_inf {
        (inf(neg), Flags::OE | Flags::PE | Flags::C1)
    } else {
        (
            F80 {
                sig: !0u64 << (64 - prec),
                se: sign | 0x7ffe,
            },
            Flags::OE | Flags::PE,
        )
    }
}

/// Round `sig * 2^exp` (`exp < 0`) to an integer magnitude.
pub(crate) fn round_int(neg: bool, sig: u64, exp: i32, rc: Rc) -> (u128, bool, bool) {
    debug_assert!(exp < 0);
    let d = -(exp as i64);
    if d >= 128 {
        // |value| < 2^-64: only a directed mode away from zero gives 1.
        let inc = match rc {
            Rc::Up => !neg,
            Rc::Down => neg,
            _ => false,
        };
        return (inc as u128, true, inc);
    }
    rnd(sig as u128, d as u32, neg, rc)
}

pub(crate) const fn quiet(x: F80) -> F80 {
    F80 {
        sig: x.sig | 1 << 62,
        se: x.se,
    }
}

/// One NaN operand: quiet it, IE if it was signalling.
pub(crate) fn nan1(x: F80) -> (F80, Flags) {
    let f = if x.is_snan() { Flags::IE } else { Flags::NONE };
    (quiet(x), f)
}

/// SDM Vol. 1 Table 4-7, x87 column: at least one of `a`, `b` is a NaN.
/// SNaN against QNaN gives the QNaN; two of a kind give the larger
/// significand, ties to `a` when `tie_a` (the destination), else `b`.
/// A lone NaN is returned quieted. Any SNaN raises IE.
pub(crate) fn nan2(a: F80, b: F80, tie_a: bool) -> (F80, Flags) {
    let (an, bn) = (a.is_nan(), b.is_nan());
    let mut f = Flags::NONE;
    if a.is_snan() || b.is_snan() {
        f = Flags::IE;
    }
    let pick_a = if an && bn {
        let (asn, bsn) = (a.is_snan(), b.is_snan());
        if asn != bsn {
            bsn
        } else if a.sig != b.sig {
            a.sig > b.sig
        } else {
            tie_a
        }
    } else {
        an
    };
    (quiet(if pick_a { a } else { b }), f)
}

pub(crate) enum Pre {
    Done(F80, Flags),
    Go(Unp, Unp, Flags),
}

/// Shared handling for two-operand arithmetic: unsupported encodings give
/// the indefinite with IE, NaNs propagate, and a denormal operand sets DE.
pub(crate) fn prelude2(a: F80, b: F80, tie_a: bool) -> Pre {
    let (ua, ub) = (unpack(a), unpack(b));
    if matches!(ua, Unp::Bad) || matches!(ub, Unp::Bad) {
        return Pre::Done(F80::INDEFINITE, Flags::IE);
    }
    if a.is_nan() || b.is_nan() {
        let (r, f) = nan2(a, b, tie_a);
        return Pre::Done(r, f);
    }
    let de = if ua.denorm() || ub.denorm() {
        Flags::DE
    } else {
        Flags::NONE
    };
    Pre::Go(ua, ub, de)
}
