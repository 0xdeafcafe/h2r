//! The basic arithmetic, which is exact: each result is the infinitely
//! precise value rounded once.
#![forbid(unsafe_code)]

use core::cmp::Ordering;

use crate::round::{
    Fin, Pre, Unp, inf, nan1, prelude2, round_int, round_pack, shr_sticky, unpack, zero,
};
use crate::{F80, Flags, Mode, Partial, Rc};

fn pack(f: Fin, m: Mode) -> (F80, Flags) {
    round_pack(f.neg, f.sig as u128, f.exp as i64, 0, m.pc.bits(), m.rc)
}

pub fn add(a: F80, b: F80, m: Mode) -> (F80, Flags) {
    add_impl(a, b, false, m, true)
}

pub fn sub(a: F80, b: F80, m: Mode) -> (F80, Flags) {
    add_impl(a, b, true, m, true)
}

/// `a + b` (or `a - b`); `tie_a` says `a` is the destination for the NaN
/// tie rule (false for FSUBR, where the destination is `b`).
pub(crate) fn add_impl(a: F80, b: F80, negate_b: bool, m: Mode, tie_a: bool) -> (F80, Flags) {
    let (ua, ub, fl) = match prelude2(a, b, tie_a) {
        Pre::Done(r, f) => return (r, f),
        Pre::Go(ua, ub, fl) => (ua, ub, fl),
    };
    let ub = if negate_b { ub.negate() } else { ub };
    match (ua, ub) {
        (Unp::Inf(x), Unp::Inf(y)) => {
            if x == y {
                (inf(x), fl)
            } else {
                (F80::INDEFINITE, Flags::IE)
            }
        }
        (Unp::Inf(x), _) | (_, Unp::Inf(x)) => (inf(x), fl),
        (Unp::Zero(x), Unp::Zero(y)) => {
            // Exact zero sum: +0 unless both are -0, or RC=down.
            let neg = if x == y { x } else { m.rc == Rc::Down };
            (zero(neg), fl)
        }
        (Unp::Zero(_), Unp::Fin(f)) | (Unp::Fin(f), Unp::Zero(_)) => {
            let (r, f2) = pack(f, m);
            (r, fl | f2)
        }
        (Unp::Fin(x), Unp::Fin(y)) => {
            let (r, f2) = add_fin(x, y, m);
            (r, fl | f2)
        }
        _ => unreachable!("NaN and unsupported handled by prelude2"),
    }
}

fn add_fin(x: Fin, y: Fin, m: Mode) -> (F80, Flags) {
    let (big, small) = if (x.exp, x.sig) >= (y.exp, y.sig) {
        (x, y)
    } else {
        (y, x)
    };
    // 62 guard bits below each significand; the smaller one is aligned with
    // a sticky bit, which is exact for rounding because at most one bit of
    // cancellation can occur once the exponents differ by two or more.
    let d = (big.exp - small.exp) as u64;
    let mb = (big.sig as u128) << 62;
    let ms = shr_sticky((small.sig as u128) << 62, d);
    let r = if big.neg == small.neg {
        mb + ms
    } else {
        mb - ms
    };
    if r == 0 {
        return (zero(m.rc == Rc::Down), Flags::NONE);
    }
    round_pack(big.neg, r, big.exp as i64 - 62, 0, m.pc.bits(), m.rc)
}

pub fn mul(a: F80, b: F80, m: Mode) -> (F80, Flags) {
    let (ua, ub, fl) = match prelude2(a, b, true) {
        Pre::Done(r, f) => return (r, f),
        Pre::Go(ua, ub, fl) => (ua, ub, fl),
    };
    let neg = sign(&ua) ^ sign(&ub);
    match (ua, ub) {
        (Unp::Inf(_), Unp::Zero(_)) | (Unp::Zero(_), Unp::Inf(_)) => (F80::INDEFINITE, Flags::IE),
        (Unp::Inf(_), _) | (_, Unp::Inf(_)) => (inf(neg), fl),
        (Unp::Zero(_), _) | (_, Unp::Zero(_)) => (zero(neg), fl),
        (Unp::Fin(x), Unp::Fin(y)) => {
            let p = x.sig as u128 * y.sig as u128;
            let (r, f2) = round_pack(neg, p, (x.exp + y.exp) as i64, 0, m.pc.bits(), m.rc);
            (r, fl | f2)
        }
        _ => unreachable!(),
    }
}

pub fn div(a: F80, b: F80, m: Mode) -> (F80, Flags) {
    div_impl(a, b, m, true)
}

pub(crate) fn div_impl(a: F80, b: F80, m: Mode, tie_a: bool) -> (F80, Flags) {
    let (ua, ub, fl) = match prelude2(a, b, tie_a) {
        Pre::Done(r, f) => return (r, f),
        Pre::Go(ua, ub, fl) => (ua, ub, fl),
    };
    let neg = sign(&ua) ^ sign(&ub);
    match (ua, ub) {
        (Unp::Inf(_), Unp::Inf(_)) | (Unp::Zero(_), Unp::Zero(_)) => (F80::INDEFINITE, Flags::IE),
        (Unp::Inf(_), _) => (inf(neg), fl),
        (_, Unp::Inf(_)) | (Unp::Zero(_), _) => (zero(neg), fl),
        // ZE outranks DE (SDM Vol. 1 §4.9.2): a denormal dividend by zero
        // reports ZE alone.
        (Unp::Fin(_), Unp::Zero(_)) => (inf(neg), Flags::ZE),
        (Unp::Fin(x), Unp::Fin(y)) => {
            let (q, exp, sticky) = div_sig(x.sig, y.sig);
            let (r, f2) = round_pack(
                neg,
                q,
                (x.exp - y.exp) as i64 + exp,
                sticky as i8,
                m.pc.bits(),
                m.rc,
            );
            (r, fl | f2)
        }
        _ => unreachable!(),
    }
}

/// 128-bit quotient of two normalised significands: `a/b ≈ q * 2^exp`,
/// with `sticky` set when the remainder is nonzero.
fn div_sig(a: u64, b: u64) -> (u128, i64, bool) {
    let (num, exp) = if a < b {
        ((a as u128) << 64, -128)
    } else {
        ((a as u128) << 63, -127)
    };
    let (b128, hi) = (b as u128, num / b as u128);
    let r = num % b128;
    let num2 = r << 64;
    let lo = num2 / b128;
    let r2 = num2 % b128;
    ((hi << 64) | lo, exp, r2 != 0)
}

fn sign(u: &Unp) -> bool {
    match *u {
        Unp::Zero(n) | Unp::Inf(n) => n,
        Unp::Fin(f) => f.neg,
        _ => false,
    }
}

pub fn sqrt(a: F80, m: Mode) -> (F80, Flags) {
    match unpack(a) {
        Unp::Bad => (F80::INDEFINITE, Flags::IE),
        Unp::Nan => nan1(a),
        Unp::Zero(_) => (a, Flags::NONE),
        Unp::Inf(false) => (a, Flags::NONE),
        Unp::Inf(true) => (F80::INDEFINITE, Flags::IE),
        Unp::Fin(f) if f.neg => (F80::INDEFINITE, Flags::IE),
        Unp::Fin(f) => {
            // a = R * 2^e2 with e2 even.
            let (r, e2) = if f.exp & 1 != 0 {
                ((f.sig as u128) << 1, f.exp - 1)
            } else {
                (f.sig as u128, f.exp)
            };
            // sqrt(R * 2^120) has about 93 bits: enough for 64 plus a round
            // bit, with the remainder as the sticky bit.
            let (root, inexact) = isqrt256(r >> 8, r << 120);
            let de = if f.denorm { Flags::DE } else { Flags::NONE };
            let (v, fl) = round_pack(
                false,
                root,
                (e2 / 2 - 60) as i64,
                inexact as i8,
                m.pc.bits(),
                m.rc,
            );
            (v, fl | de)
        }
    }
}

/// Integer square root of the 256-bit value `hi * 2^128 + lo` by the
/// digit-by-digit method. The root must fit in 126 bits.
fn isqrt256(hi: u128, lo: u128) -> (u128, bool) {
    let mut rem: u128 = 0;
    let mut root: u128 = 0;
    for i in (0..128u32).rev() {
        let pos = 2 * i;
        let two = if pos >= 128 {
            (hi >> (pos - 128)) & 3
        } else {
            (lo >> pos) & 3
        };
        rem = (rem << 2) | two;
        let trial = (root << 2) | 1;
        if rem >= trial {
            rem -= trial;
            root = (root << 1) | 1;
        } else {
            root <<= 1;
        }
    }
    (root, rem != 0)
}

/// FRNDINT: round to an integer under `rc`, at full precision.
pub fn rndint(a: F80, rc: Rc) -> (F80, Flags) {
    match unpack(a) {
        Unp::Bad => (F80::INDEFINITE, Flags::IE),
        Unp::Nan => nan1(a),
        Unp::Zero(_) | Unp::Inf(_) => (a, Flags::NONE),
        Unp::Fin(f) => {
            let mut fl = if f.denorm { Flags::DE } else { Flags::NONE };
            if f.exp >= 0 {
                return (a, fl);
            }
            let (q, inexact, up) = round_int(f.neg, f.sig, f.exp, rc);
            if inexact {
                fl |= Flags::PE;
            }
            if up {
                fl |= Flags::C1;
            }
            let v = if q == 0 {
                zero(f.neg)
            } else {
                round_pack(f.neg, q, 0, 0, 64, Rc::Near).0
            };
            (v, fl)
        }
    }
}

/// `trunc(b)` clamped to a range that already forces overflow/underflow.
fn trunc_clamped(g: Fin) -> i64 {
    const LIMIT: i64 = 1 << 20;
    let mag = if g.exp >= 0 {
        LIMIT
    } else {
        let d = -(g.exp as i64);
        if d >= 64 {
            0
        } else {
            ((g.sig >> d) as i64).min(LIMIT)
        }
    };
    if g.neg { -mag } else { mag }
}

/// FSCALE: `a * 2^trunc(b)`, rounded at 64 bits under `rc` (no PC effect).
pub fn scale(a: F80, b: F80, rc: Rc) -> (F80, Flags) {
    let (ua, ub, fl) = match prelude2(a, b, true) {
        Pre::Done(r, f) => return (r, f),
        Pre::Go(ua, ub, fl) => (ua, ub, fl),
    };
    match (ua, ub) {
        (Unp::Inf(_), Unp::Inf(true)) | (Unp::Zero(_), Unp::Inf(false)) => {
            (F80::INDEFINITE, Flags::IE)
        }
        (Unp::Inf(n), _) => (inf(n), fl),
        (Unp::Zero(n), _) => (zero(n), fl),
        (Unp::Fin(f), Unp::Inf(false)) => (inf(f.neg), fl),
        (Unp::Fin(f), Unp::Inf(true)) => (zero(f.neg), fl),
        // Exact, but arithmetic never produces a pseudo-denormal (SDM Vol. 1
        // §8.2.2): repack so one becomes its normal encoding (exponent 1),
        // and a true denormal reports TINY.
        (Unp::Fin(f), Unp::Zero(_)) => {
            let (r, f2) = round_pack(f.neg, f.sig as u128, f.exp as i64, 0, 64, rc);
            (r, fl | f2)
        }
        (Unp::Fin(f), Unp::Fin(g)) => {
            let n = trunc_clamped(g);
            let (r, f2) = round_pack(f.neg, f.sig as u128, f.exp as i64 + n, 0, 64, rc);
            (r, fl | f2)
        }
        _ => unreachable!(),
    }
}

/// FXTRACT: `(exponent, significand)`; the significand keeps `a`'s sign
/// with biased exponent 0x3fff. Denormals are normalised first.
pub fn xtract(a: F80) -> (F80, F80, Flags) {
    match unpack(a) {
        Unp::Bad => (F80::INDEFINITE, F80::INDEFINITE, Flags::IE),
        Unp::Nan => {
            let (q, f) = nan1(a);
            (q, q, f)
        }
        Unp::Zero(n) => (F80::NEG_INFINITY, zero(n), Flags::ZE),
        Unp::Inf(n) => (F80::INFINITY, inf(n), Flags::NONE),
        Unp::Fin(f) => {
            let de = if f.denorm { Flags::DE } else { Flags::NONE };
            let s = F80 {
                sig: f.sig,
                se: (a.se & 0x8000) | 0x3fff,
            };
            (F80::from_i64(f.top() as i64), s, de)
        }
    }
}

/// FPREM: `a - b*trunc(a/b)`, sign of `a`.
pub fn prem(a: F80, b: F80) -> Partial {
    remainder(a, b, false)
}

/// FPREM1: IEEE remainder, quotient rounded to nearest-even.
pub fn prem1(a: F80, b: F80) -> Partial {
    remainder(a, b, true)
}

fn remainder(a: F80, b: F80, nearest: bool) -> Partial {
    let done = |value, flags| Partial {
        value,
        incomplete: false,
        q: 0,
        flags,
    };
    let (ua, ub, fl) = match prelude2(a, b, true) {
        Pre::Done(r, f) => return done(r, f),
        Pre::Go(ua, ub, fl) => (ua, ub, fl),
    };
    let (x, y) = match (ua, ub) {
        (Unp::Inf(_), _) | (_, Unp::Zero(_)) => return done(F80::INDEFINITE, Flags::IE),
        (Unp::Zero(_), _) => return done(a, fl),
        // Quotient 0, remainder a: repacked, as every finite result is.
        (Unp::Fin(x), Unp::Inf(_)) => {
            let (v, tiny) = repack(x);
            return done(v, fl | tiny);
        }
        (Unp::Fin(x), Unp::Fin(y)) => (x, y),
        _ => unreachable!(),
    };
    let d = x.top() - y.top();
    // value = mag * 2^exp with sign `neg`; quotient `q`.
    let (mut neg, mag, exp, q, incomplete): (bool, u128, i32, u128, bool);
    if d >= 64 {
        // Partial reduction (SDM FPREM/FPREM1 pseudo-code, D >= 64):
        // QQ = trunc((a/b) / 2^(D-N)) for an implementation-defined N in
        // 32..=63. N = 32 + (D mod 32) is chosen here.
        let n = 32 + (d % 32) as u32;
        let num = (x.sig as u128) << n;
        neg = x.neg;
        mag = num % y.sig as u128;
        exp = x.exp - n as i32;
        q = 0;
        incomplete = true;
    } else if d >= 0 {
        let num = (x.sig as u128) << d;
        let mut qq = num / y.sig as u128;
        let mut r = num % y.sig as u128;
        neg = x.neg;
        if nearest {
            let twice = r << 1;
            let ys = y.sig as u128;
            if twice > ys || (twice == ys && qq & 1 == 1) {
                qq += 1;
                r = ys - r;
                neg = !neg;
            }
        }
        mag = r;
        exp = y.exp;
        q = qq;
        incomplete = false;
    } else if d == -1 && nearest && x.sig > y.sig {
        // |a| in (|b|/2, |b|): the nearest quotient is 1.
        neg = !x.neg;
        mag = ((y.sig as u128) << 1) - x.sig as u128;
        exp = y.exp - 1;
        q = 1;
        incomplete = false;
    } else {
        // |a| < |b| (or, for FPREM1, too small to round the quotient to 1):
        // quotient 0, remainder a, repacked.
        let (v, tiny) = repack(x);
        return done(v, fl | tiny);
    }
    if mag == 0 {
        neg = x.neg;
    }
    let (value, tiny) = if mag == 0 {
        (zero(neg), Flags::NONE)
    } else {
        // Exact: the remainder always fits in 64 bits, but it can be tiny.
        let (v, f) = round_pack(neg, mag, exp as i64, 0, 64, Rc::Near);
        (v, Flags(f.0 & Flags::TINY.0))
    };
    Partial {
        value,
        incomplete,
        q: if incomplete { 0 } else { (q & 7) as u8 },
        flags: fl | tiny,
    }
}

/// A finite operand returned unchanged in value as an arithmetic result:
/// packed exactly at 64 bits, so a pseudo-denormal comes back as its normal
/// encoding (arithmetic never produces one, SDM Vol. 1 §8.2.2), with only
/// [`Flags::TINY`] from the packing (a true denormal is tiny; exact, so no
/// UE or PE).
fn repack(x: Fin) -> (F80, Flags) {
    let (v, f) = round_pack(x.neg, x.sig as u128, x.exp as i64, 0, 64, Rc::Near);
    (v, Flags(f.0 & Flags::TINY.0))
}

/// The result of an x87 comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cmp {
    Less,
    Equal,
    Greater,
    Unordered,
}

/// Compare `a` with `b` (FCOM/FUCOM/FCOMI/FUCOMI semantics). Unsupported
/// operands and SNaNs set IE; a QNaN sets IE unless `quiet` (the FUCOM
/// forms). A denormal operand of an ordered comparison sets DE.
pub fn compare(a: F80, b: F80, quiet: bool) -> (Cmp, Flags) {
    let (ua, ub) = (unpack(a), unpack(b));
    if matches!(ua, Unp::Bad) || matches!(ub, Unp::Bad) {
        return (Cmp::Unordered, Flags::IE);
    }
    if a.is_nan() || b.is_nan() {
        let ie = a.is_snan() || b.is_snan() || !quiet;
        return (Cmp::Unordered, if ie { Flags::IE } else { Flags::NONE });
    }
    let fl = if ua.denorm() || ub.denorm() {
        Flags::DE
    } else {
        Flags::NONE
    };
    let key = |u: &Unp| -> (bool, u8, i32, u64) {
        match *u {
            Unp::Zero(_) => (false, 0, 0, 0),
            Unp::Fin(f) => (f.neg, 1, f.exp, f.sig),
            Unp::Inf(n) => (n, 2, 0, 0),
            _ => unreachable!(),
        }
    };
    let (ka, kb) = (key(&ua), key(&ub));
    let ord = if ka.0 != kb.0 {
        if ka.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    } else {
        let mag = (ka.1, ka.2, ka.3).cmp(&(kb.1, kb.2, kb.3));
        if ka.0 { mag.reverse() } else { mag }
    };
    let c = match ord {
        Ordering::Less => Cmp::Less,
        Ordering::Equal => Cmp::Equal,
        Ordering::Greater => Cmp::Greater,
    };
    (c, fl)
}
