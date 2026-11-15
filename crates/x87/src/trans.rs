//! Transcendentals: FSIN, FCOS, FSINCOS, FPTAN, FPATAN, FYL2X, FYL2XP1,
//! F2XM1.
//!
//! Special operands follow the SDM's tables exactly. Finite operands are
//! evaluated in [`Wf<W>`](crate::wide::Wf) (512-bit significand) and rounded
//! once to 64 bits under round-to-nearest; the x87 transcendentals are not
//! affected by PC, and these entry points take no rounding mode (see the
//! report on docs/FLOAT-ABI.md §6).
//!
//! # Accuracy
//!
//! Every wide operation truncates with relative error below 2^-511. The
//! series are summed until the next term is below 2^-520 of the sum, and
//! argument reduction multiplies by a 512-bit 2/pi, which leaves at least
//! ~2^-290 relative accuracy in the reduced argument even if a 64-bit input
//! came within 2^-150 of a multiple of pi/2 (the closest approach is expected
//! to be far larger). So each result before rounding is within ~2^-280
//! relative of the exact value, and the rounding to 64 bits is wrong only if
//! the exact value lies that close to a rounding midpoint. That has not been
//! ruled out by a worst-case search, hence
//! [`TRANSCENDENTAL_IS_CORRECTLY_ROUNDED`](crate::TRANSCENDENTAL_IS_CORRECTLY_ROUNDED)
//! is `false`. The tests check that 8-limb and 12-limb evaluations agree bit
//! for bit on thousands of operands.
//!
//! Results that are exactly representable (x2m1 at integers, yl2x at powers
//! of two, yl2xp1 when 1+x is a power of two) are computed exactly. Series
//! that stop because the next term is negligible report the sign of that
//! term, so results like `sin(tiny)` and `cos(tiny)` get PE and C1 right.
#![forbid(unsafe_code)]

use core::cmp::Ordering;

use crate::consts::{ln2, pi, two_over_pi};
use crate::round::{Fin, Unp, inf, nan1, nan2, round_pack, unpack, zero};
use crate::wide::Wf;
use crate::{F80, Flags, Rc, Transcendental};

/// Working precision in 64-bit limbs.
pub(crate) const W: usize = 8;

#[cfg(any(feature = "fast-trans", test))]
#[path = "trans_fast.rs"]
mod fast;

/// Tail for a result known to be irrational whose error direction is not
/// tracked: treated as a sticky bit.
const INEXACT: i8 = 1;

type Tw<const N: usize> = (Wf<N>, i8);

fn wf<const N: usize>(f: Fin) -> Wf<N> {
    Wf::from_parts(f.neg, f.sig, f.exp)
}

fn negligible<const N: usize>(term: &Wf<N>, sum: &Wf<N>) -> bool {
    term.is_zero() || term.top() < sum.top() - 64 * N as i32 - 8
}

/// Direction of the omitted remainder, given the first omitted term of a
/// series whose terms decrease and either alternate or keep one sign.
fn tail_of<const N: usize>(term: &Wf<N>, sum: &Wf<N>) -> i8 {
    if term.is_zero() {
        INEXACT
    } else if term.neg == sum.neg {
        1
    } else {
        -1
    }
}

fn neg_t<const N: usize>((w, t): Tw<N>) -> Tw<N> {
    (w.neg(), t)
}

fn sin_series<const N: usize>(r: Wf<N>) -> Tw<N> {
    let r2 = r.mul(r);
    let (mut term, mut sum) = (r, r);
    let mut n = 1u64;
    loop {
        term = term.mul(r2).div_u64((2 * n) * (2 * n + 1)).neg();
        if negligible(&term, &sum) {
            return (sum, tail_of(&term, &sum));
        }
        sum = sum.add(term);
        n += 1;
    }
}

fn cos_series<const N: usize>(r: Wf<N>) -> Tw<N> {
    let r2 = r.mul(r);
    let (mut term, mut sum) = (Wf::one(), Wf::one());
    let mut n = 1u64;
    loop {
        term = term.mul(r2).div_u64((2 * n - 1) * (2 * n)).neg();
        if negligible(&term, &sum) {
            return (sum, tail_of(&term, &sum));
        }
        sum = sum.add(term);
        n += 1;
    }
}

/// atan(t) for small t (|t| < 2^-9).
fn atan_series<const N: usize>(t: Wf<N>) -> Tw<N> {
    let t2 = t.mul(t);
    let (mut p, mut sum) = (t, t);
    let mut n = 1u64;
    loop {
        p = p.mul(t2);
        let mut term = p.div_u64(2 * n + 1);
        if n % 2 == 1 {
            term = term.neg();
        }
        if negligible(&term, &sum) {
            return (sum, tail_of(&term, &sum));
        }
        sum = sum.add(term);
        n += 1;
    }
}

/// atanh(s) for |s| <= 0.18.
fn atanh_series<const N: usize>(s: Wf<N>) -> Tw<N> {
    let s2 = s.mul(s);
    let (mut p, mut sum) = (s, s);
    let mut n = 1u64;
    loop {
        p = p.mul(s2);
        let term = p.div_u64(2 * n + 1);
        if negligible(&term, &sum) {
            return (sum, tail_of(&term, &sum));
        }
        sum = sum.add(term);
        n += 1;
    }
}

/// e^z - 1 for |z| < 0.7.
fn expm1_series<const N: usize>(z: Wf<N>) -> Tw<N> {
    let (mut term, mut sum) = (z, z);
    let mut n = 2u64;
    loop {
        term = term.mul(z).div_u64(n);
        if negligible(&term, &sum) {
            return (sum, tail_of(&term, &sum));
        }
        sum = sum.add(term);
        n += 1;
    }
}

/// atan(t) for 0 < t <= 1: halve the angle with
/// `t' = t / (1 + sqrt(1 + t^2))` until t < 2^-10, then the series.
fn atan_pos<const N: usize>(t: Wf<N>) -> Tw<N> {
    let one = Wf::<N>::one();
    let mut t = t;
    let mut k = 0;
    while t.top() >= -10 {
        t = t.div(one.add(one.add(t.mul(t)).sqrt()));
        k += 1;
    }
    let (s, tail) = atan_series(t);
    (s.scale(k), if k == 0 { tail } else { INEXACT })
}

/// Reduce |x| (finite, < 2^63) to `r` in [-pi/4, pi/4] with
/// |x| = r + k*pi/2.
fn reduce<const N: usize>(f: Fin) -> (Wf<N>, u64) {
    let x = Wf::<N>::from_parts(false, f.sig, f.exp);
    let p = pi::<N>();
    if x.cmp_abs(&p.scale(-2)) == Ordering::Less {
        return (x, 0);
    }
    let y = x.mul(two_over_pi::<N>());
    let (mut k, mut fr) = y.int_frac();
    if !fr.is_zero() && fr.top() == -1 {
        k = k.wrapping_add(1);
        fr = fr.sub(Wf::one());
    }
    (fr.mul(p.scale(-1)), k)
}

/// (sin x, cos x) for finite |x| < 2^63.
fn sin_cos_wf<const N: usize>(f: Fin) -> (Tw<N>, Tw<N>) {
    let (r, k) = reduce::<N>(f);
    quadrant(sin_series(r), cos_series(r), k, f.neg)
}

/// (sin x, cos x) from (sin r, cos r), where `|x| = r + k*pi/2` and `neg`
/// is x's sign.
fn quadrant<const N: usize>(s: Tw<N>, c: Tw<N>, k: u64, neg: bool) -> (Tw<N>, Tw<N>) {
    let (sn, cs) = match k & 3 {
        0 => (s, c),
        1 => (c, neg_t(s)),
        2 => (neg_t(s), neg_t(c)),
        _ => (neg_t(c), s),
    };
    (if neg { neg_t(sn) } else { sn }, cs)
}

fn tan_wf<const N: usize>(f: Fin) -> Tw<N> {
    let (r, k) = reduce::<N>(f);
    let one = Wf::<N>::one();
    let tiny = r.is_zero() || r.top() < -(32 * N as i32 - 20);
    let v = if k & 1 == 0 {
        if tiny {
            // tan r = r (1 + r^2/3 + ...): just above r in magnitude.
            (r, 1)
        } else {
            (sin_series(r).0.div(cos_series(r).0), INEXACT)
        }
    } else if r.is_zero() {
        // Unreachable: no nonzero 64-bit value is a multiple of pi/2.
        (one.scale(1 << 20).neg(), INEXACT)
    } else if tiny {
        // -cot r = -(1/r - r/3 - ...): just below 1/r in magnitude.
        (one.div(r).neg(), -1)
    } else {
        (cos_series(r).0.div(sin_series(r).0).neg(), INEXACT)
    };
    if f.neg { neg_t(v) } else { v }
}

fn done(value: F80, flags: Flags) -> Transcendental {
    Transcendental {
        value,
        out_of_range: false,
        flags,
    }
}

fn de(d: bool) -> Flags {
    if d { Flags::DE } else { Flags::NONE }
}

/// Special operands of FSIN/FCOS/FSINCOS/FPTAN. `zero_value` is the result
/// for a zero operand given the operand.
fn trig_pre(a: F80, cos: bool) -> Result<Fin, Transcendental> {
    match unpack(a) {
        Unp::Bad | Unp::Inf(_) => Err(done(F80::INDEFINITE, Flags::IE)),
        Unp::Nan => {
            let (q, f) = nan1(a);
            Err(done(q, f))
        }
        Unp::Zero(_) => Err(done(if cos { F80::ONE } else { a }, Flags::NONE)),
        // |a| >= 2^63: C2, operand unchanged, no other flag.
        Unp::Fin(f) if f.exp >= 0 => Err(Transcendental {
            value: a,
            out_of_range: true,
            flags: Flags::NONE,
        }),
        Unp::Fin(f) => Ok(f),
    }
}

fn finish<const N: usize>((v, t): Tw<N>, denorm: bool) -> (F80, Flags) {
    let (r, f) = v.to_f80(t);
    (r, f | de(denorm))
}

pub(crate) fn sin_n<const N: usize>(a: F80) -> Transcendental {
    match trig_pre(a, false) {
        Err(t) => t,
        Ok(f) => {
            let (r, fl) = finish(sin_cos_wf::<N>(f).0, f.denorm);
            done(r, fl)
        }
    }
}

pub(crate) fn cos_n<const N: usize>(a: F80) -> Transcendental {
    match trig_pre(a, true) {
        Err(t) => t,
        Ok(f) => {
            let (r, fl) = finish(sin_cos_wf::<N>(f).1, f.denorm);
            done(r, fl)
        }
    }
}

pub(crate) fn tan_n<const N: usize>(a: F80) -> Transcendental {
    match trig_pre(a, false) {
        Err(t) => t,
        Ok(f) => {
            let (r, fl) = finish(tan_wf::<N>(f), f.denorm);
            done(r, fl)
        }
    }
}

/// FSIN.
pub fn sin(a: F80) -> Transcendental {
    #[cfg(feature = "fast-trans")]
    if let Some(v) = fast::sin(a) {
        return v;
    }
    sin_n::<W>(a)
}

/// FCOS.
pub fn cos(a: F80) -> Transcendental {
    #[cfg(feature = "fast-trans")]
    if let Some(v) = fast::cos(a) {
        return v;
    }
    cos_n::<W>(a)
}

/// FPTAN's ST(0) result (the instruction then pushes 1.0, which the ABI
/// layer does).
pub fn tan(a: F80) -> Transcendental {
    #[cfg(feature = "fast-trans")]
    if let Some(v) = fast::tan(a) {
        return v;
    }
    tan_n::<W>(a)
}

/// FSINCOS: `(sin, cos)`. The flags in the first element cover both
/// results; C1 is the cosine's, since it ends up in ST(0). For an
/// out-of-range operand both values are the unchanged operand; for a NaN
/// or invalid operand both are the NaN result.
pub fn sincos(a: F80) -> (Transcendental, F80) {
    #[cfg(feature = "fast-trans")]
    if let Some(v) = fast::sincos(a) {
        return v;
    }
    sincos_n::<W>(a)
}

/// FSINCOS's flags: both results' exceptions, and the cosine's C1.
fn sincos_flags(sf: Flags, cf: Flags) -> Flags {
    (sf | cf).without(Flags::C1) | Flags(cf.0 & Flags::C1.0)
}

pub(crate) fn sincos_n<const N: usize>(a: F80) -> (Transcendental, F80) {
    match trig_pre(a, false) {
        Err(t) => {
            let c = if t.out_of_range || a.classify() != crate::Class::Zero {
                t.value
            } else {
                F80::ONE
            };
            (t, c)
        }
        Ok(f) => {
            let (s, c) = sin_cos_wf::<N>(f);
            let (sv, sf) = finish(s, f.denorm);
            let (cv, cf) = finish(c, f.denorm);
            (done(sv, sincos_flags(sf, cf)), cv)
        }
    }
}

/// A rounded multiple of pi: `num/den * pi` with `den` a power of two.
fn pi_multiple<const N: usize>(num: u64, log2_den: i32, neg: bool) -> (F80, Flags) {
    let v = pi::<N>().mul_u64(num).scale(-log2_den);
    let v = if neg { v.neg() } else { v };
    v.to_f80(INEXACT)
}

/// FPATAN: `atan2(y, x)` with the SDM's table for zeros and infinities.
pub fn patan(y: F80, x: F80) -> (F80, Flags) {
    #[cfg(feature = "fast-trans")]
    if let Some(v) = fast::patan(y, x) {
        return v;
    }
    patan_n::<W>(y, x)
}

pub(crate) fn patan_n<const N: usize>(y: F80, x: F80) -> (F80, Flags) {
    let (uy, ux) = (unpack(y), unpack(x));
    if matches!(uy, Unp::Bad) || matches!(ux, Unp::Bad) {
        return (F80::INDEFINITE, Flags::IE);
    }
    if y.is_nan() || x.is_nan() {
        return nan2(y, x, true);
    }
    let d = de(uy.denorm() || ux.denorm());
    let with = |(v, f): (F80, Flags)| (v, f | d);
    let x_neg = match ux {
        Unp::Zero(n) | Unp::Inf(n) => n,
        Unp::Fin(f) => f.neg,
        _ => unreachable!(),
    };
    match (uy, ux) {
        (Unp::Inf(ny), Unp::Inf(nx)) => with(if nx {
            pi_multiple::<N>(3, 2, ny)
        } else {
            pi_multiple::<N>(1, 2, ny)
        }),
        (Unp::Inf(ny), _) => with(pi_multiple::<N>(1, 1, ny)),
        (Unp::Zero(ny), _) => with(if x_neg {
            pi_multiple::<N>(1, 0, ny)
        } else {
            (zero(ny), Flags::NONE)
        }),
        (Unp::Fin(fy), Unp::Inf(nx)) => with(if nx {
            pi_multiple::<N>(1, 0, fy.neg)
        } else {
            (zero(fy.neg), Flags::NONE)
        }),
        (Unp::Fin(fy), Unp::Zero(_)) => with(pi_multiple::<N>(1, 1, fy.neg)),
        (Unp::Fin(fy), Unp::Fin(fx)) => with(patan_wf::<N>(fy, fx).to_f80_pair()),
        _ => unreachable!(),
    }
}

/// atan2(y, x) for finite nonzero operands.
fn patan_wf<const N: usize>(fy: Fin, fx: Fin) -> Tw<N> {
    let ay = Wf::<N>::from_parts(false, fy.sig, fy.exp);
    let ax = Wf::<N>::from_parts(false, fx.sig, fx.exp);
    let p = pi::<N>();
    let (base, tail) = if ay.cmp_abs(&ax) == Ordering::Greater {
        (p.scale(-1).sub(atan_pos(ax.div(ay)).0), INEXACT)
    } else {
        atan_pos(ay.div(ax))
    };
    let (v, tail) = if fx.neg {
        (p.sub(base), INEXACT)
    } else {
        (base, tail)
    };
    (if fy.neg { v.neg() } else { v }, tail)
}

/// log2 of a positive wide value; `true` when the result is exact (an
/// integer, because the value is a power of two).
fn log2_wf<const N: usize>(u: Wf<N>) -> (Wf<N>, bool) {
    let mut e = u.top();
    let mut m = u.scale(-e);
    let sqrt2 = Wf::<N>::from_parts(false, 0xb504_f333_f9de_6484, -63);
    if m.cmp_abs(&sqrt2) == Ordering::Greater {
        m = m.scale(-1);
        e += 1;
    }
    let one = Wf::<N>::one();
    let num = m.sub(one);
    if num.is_zero() {
        return (Wf::from_i64(e as i64), true);
    }
    let s = num.div(m.add(one));
    let ln_m = atanh_series(s).0.scale(1);
    (ln_m.div(ln2::<N>()).add(Wf::from_i64(e as i64)), false)
}

/// FYL2X: `y * log2(x)`.
pub fn yl2x(y: F80, x: F80) -> (F80, Flags) {
    #[cfg(feature = "fast-trans")]
    if let Some(v) = fast::yl2x(y, x) {
        return v;
    }
    yl2x_n::<W>(y, x)
}

pub(crate) fn yl2x_n<const N: usize>(y: F80, x: F80) -> (F80, Flags) {
    let (uy, ux) = (unpack(y), unpack(x));
    if matches!(uy, Unp::Bad) || matches!(ux, Unp::Bad) {
        return (F80::INDEFINITE, Flags::IE);
    }
    if y.is_nan() || x.is_nan() {
        return nan2(y, x, true);
    }
    let d = de(uy.denorm() || ux.denorm());
    let ind = (F80::INDEFINITE, Flags::IE);
    match ux {
        Unp::Inf(true) => ind,
        Unp::Fin(fx) if fx.neg => ind,
        Unp::Zero(_) => match uy {
            Unp::Zero(_) => ind,
            Unp::Inf(ny) => (inf(!ny), d),
            // ZE outranks DE (SDM Vol. 1 §4.9.2).
            Unp::Fin(fy) => (inf(!fy.neg), Flags::ZE),
            _ => unreachable!(),
        },
        Unp::Inf(false) => match uy {
            Unp::Zero(_) => ind,
            Unp::Inf(ny) => (inf(ny), d),
            Unp::Fin(fy) => (inf(fy.neg), d),
            _ => unreachable!(),
        },
        Unp::Fin(fx) => {
            let is_one = fx.sig == 1 << 63 && fx.exp == -63;
            let below_one = fx.exp + 63 < 0;
            match uy {
                Unp::Inf(_) if is_one => ind,
                Unp::Inf(ny) => (inf(ny ^ below_one), d),
                Unp::Zero(ny) => (zero(ny ^ below_one), d),
                Unp::Fin(fy) if is_one => (zero(fy.neg), d),
                Unp::Fin(fy) => {
                    let (lg, exact) = log2_wf::<N>(wf(fx));
                    let (v, f) = wf::<N>(fy).mul(lg).to_f80(if exact { 0 } else { INEXACT });
                    (v, f | d)
                }
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

/// FYL2XP1: `y * log2(1 + x)`.
///
/// The SDM defines it for |x| < 1 - sqrt(2)/2. This implementation returns
/// the correctly rounded mathematical value for every x > -1; x = -1 is
/// treated like log2(0) (ZE, infinity), x < -1 and x = -inf like log2 of
/// a negative number (IE, indefinite), and x = +inf like FYL2X with +inf.
pub fn yl2xp1(y: F80, x: F80) -> (F80, Flags) {
    yl2xp1_n::<W>(y, x)
}

pub(crate) fn yl2xp1_n<const N: usize>(y: F80, x: F80) -> (F80, Flags) {
    let (uy, ux) = (unpack(y), unpack(x));
    if matches!(uy, Unp::Bad) || matches!(ux, Unp::Bad) {
        return (F80::INDEFINITE, Flags::IE);
    }
    if y.is_nan() || x.is_nan() {
        return nan2(y, x, true);
    }
    let d = de(uy.denorm() || ux.denorm());
    let ind = (F80::INDEFINITE, Flags::IE);
    match ux {
        Unp::Zero(nx) => match uy {
            Unp::Inf(_) => ind,
            Unp::Zero(ny) => (zero(ny ^ nx), d),
            Unp::Fin(fy) => (zero(fy.neg ^ nx), d),
            _ => unreachable!(),
        },
        Unp::Inf(true) => ind,
        Unp::Inf(false) => match uy {
            Unp::Zero(_) => ind,
            Unp::Inf(ny) => (inf(ny), d),
            Unp::Fin(fy) => (inf(fy.neg), d),
            _ => unreachable!(),
        },
        Unp::Fin(fx) => {
            let minus_one = fx.neg && fx.sig == 1 << 63 && fx.exp == -63;
            if minus_one {
                return match uy {
                    Unp::Zero(_) => ind,
                    Unp::Inf(ny) => (inf(!ny), d),
                    // ZE outranks DE (SDM Vol. 1 §4.9.2).
                    Unp::Fin(fy) => (inf(!fy.neg), Flags::ZE),
                    _ => unreachable!(),
                };
            }
            if fx.neg && fx.top() >= 0 {
                return ind;
            }
            match uy {
                Unp::Inf(ny) => (inf(ny ^ fx.neg), d),
                Unp::Zero(ny) => (zero(ny ^ fx.neg), d),
                Unp::Fin(fy) => {
                    let (lg, exact) = log2p1::<N>(fx);
                    let (v, f) = wf::<N>(fy).mul(lg).to_f80(if exact { 0 } else { INEXACT });
                    (v, f | d)
                }
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

/// log2(1 + x) for finite nonzero x > -1.
fn log2p1<const N: usize>(fx: Fin) -> (Wf<N>, bool) {
    let x = wf::<N>(fx);
    let lim = 64 * N as i32 - 70;
    let t = fx.top();
    if (-lim..=lim).contains(&t) {
        // 1 + x is exact in N limbs.
        log2_wf(Wf::one().add(x))
    } else if t > lim {
        (log2_wf(Wf::one().add(x)).0, false)
    } else {
        let s = x.div(Wf::from_u64(2).add(x));
        (atanh_series(s).0.scale(1).div(ln2::<N>()), false)
    }
}

/// F2XM1: `2^a - 1`.
///
/// The SDM defines it for |a| <= 1. This implementation is total: it
/// returns the correctly rounded value of `2^a - 1` for every finite `a`
/// (overflowing to +inf with OE for large `a`, approaching -1 with PE for
/// very negative `a`), `-1` for `-inf` and `+inf` for `+inf`.
pub fn x2m1(a: F80) -> (F80, Flags) {
    #[cfg(feature = "fast-trans")]
    if let Some(v) = fast::x2m1(a) {
        return v;
    }
    x2m1_n::<W>(a)
}

pub(crate) fn x2m1_n<const N: usize>(a: F80) -> (F80, Flags) {
    let f = match unpack(a) {
        Unp::Bad => return (F80::INDEFINITE, Flags::IE),
        Unp::Nan => return nan1(a),
        Unp::Zero(_) => return (a, Flags::NONE),
        Unp::Inf(true) => return (F80::ONE.neg(), Flags::NONE),
        Unp::Inf(false) => return (a, Flags::NONE),
        Unp::Fin(f) => f,
    };
    let d = de(f.denorm);
    let with = |(v, fl): (F80, Flags)| (v, fl | d);
    // Integer operand: 2^n - 1 exactly, then one rounding.
    let int = if f.exp >= 0 {
        Some(if f.neg { -(1i64 << 40) } else { 1i64 << 40 })
    } else if f.exp > -64 && f.sig & ((1u64 << -f.exp) - 1) == 0 {
        let n = (f.sig >> -f.exp).min(1 << 40) as i64;
        Some(if f.neg { -n } else { n })
    } else {
        None
    };
    if let Some(n) = int {
        return with(pow2m1_int(n));
    }
    // Non-integer.
    let t = f.top();
    if !f.neg && t >= 15 {
        // a > 2^15 > 16384: overflow.
        return with(round_pack(false, 1, 1 << 20, 0, 64, Rc::Near));
    }
    if f.neg && t >= 8 {
        // a < -256: 2^a - 1 is within 2^-256 above -1.
        return with(round_pack(true, u128::MAX, -128, 1, 64, Rc::Near));
    }
    let x = wf::<N>(f);
    if t < 0 {
        // |a| < 1.
        let z = x.mul(ln2::<N>());
        return with(expm1_series(z).to_f80_pair());
    }
    // a = n + fr with fr in (0, 1).
    let floor = if f.neg {
        -((f.sig >> -f.exp) as i64) - 1
    } else {
        (f.sig >> -f.exp) as i64
    };
    let fr = x.sub(Wf::from_i64(floor));
    let two_fr = expm1_series(fr.mul(ln2::<N>())).0.add(Wf::one());
    let v = two_fr.scale(floor as i32).sub(Wf::one());
    with(v.to_f80(INEXACT))
}

/// 2^n - 1 for an integer n (|n| clamped to 2^40), rounded once.
fn pow2m1_int(n: i64) -> (F80, Flags) {
    if n > 0 {
        if n <= 127 {
            round_pack(false, (1u128 << n) - 1, 0, 0, 64, Rc::Near)
        } else {
            round_pack(false, u128::MAX, n - 128, (n > 128) as i8, 64, Rc::Near)
        }
    } else {
        let k = -n;
        if k <= 127 {
            round_pack(true, (1u128 << k) - 1, -k, 0, 64, Rc::Near)
        } else {
            round_pack(true, u128::MAX, -128, (k > 128) as i8, 64, Rc::Near)
        }
    }
}

trait PairToF80 {
    fn to_f80_pair(self) -> (F80, Flags);
}

impl<const N: usize> PairToF80 for Tw<N> {
    fn to_f80_pair(self) -> (F80, Flags) {
        self.0.to_f80(self.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64*, deterministic.
    pub(super) struct Rng(pub(super) u64);
    impl Rng {
        pub(super) fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        /// A normal F80 with exponent (unbiased) in `lo..=hi`, random sign.
        pub(super) fn f80(&mut self, lo: i32, hi: i32) -> F80 {
            let r = self.next();
            let span = (hi - lo + 1) as u64;
            let e = lo + (r % span) as i32;
            let sign = if self.next() & 1 != 0 { 0x8000 } else { 0 };
            F80 {
                sig: self.next() | 1 << 63,
                se: sign | (e + 16383) as u16,
            }
        }
    }

    /// The accuracy measurement: evaluating at 8 limbs (512 bits) and at 12
    /// limbs (768 bits) must give the same F80 and flags. Any disagreement
    /// would mean the 8-limb error reached the final rounding.
    #[test]
    fn precision_8_and_12_limbs_agree() {
        assert_eq!(agree(0x9e37_79b9_7f4a_7c15, 150), 1350);
    }

    /// The measurement quoted in the report: run with
    /// `cargo test -p x87 --release --lib -- --ignored precision`.
    #[test]
    #[ignore = "slow; the default run covers a sample"]
    fn precision_8_and_12_limbs_agree_extended() {
        assert_eq!(agree(0x0123_4567_89ab_cdef, 20_000), 180_000);
    }

    fn agree(seed: u64, iters: usize) -> usize {
        let mut rng = Rng(seed);
        let mut count = 0;
        for i in 0..iters {
            let (lo, hi) = match i % 4 {
                0 => (-3, 3),
                1 => (-40, 62),
                2 => (-16382, -100),
                _ => (55, 62),
            };
            let a = rng.f80(lo, hi);
            let b = rng.f80(-3, 3);
            assert_eq!(sin_n::<8>(a), sin_n::<12>(a), "sin {a:x?}");
            assert_eq!(cos_n::<8>(a), cos_n::<12>(a), "cos {a:x?}");
            assert_eq!(tan_n::<8>(a), tan_n::<12>(a), "tan {a:x?}");
            assert_eq!(
                patan_n::<8>(a, b),
                patan_n::<12>(a, b),
                "patan {a:x?} {b:x?}"
            );
            assert_eq!(
                patan_n::<8>(b, a),
                patan_n::<12>(b, a),
                "patan {b:x?} {a:x?}"
            );
            let pos = a.abs();
            assert_eq!(
                yl2x_n::<8>(b, pos),
                yl2x_n::<12>(b, pos),
                "yl2x {b:x?} {pos:x?}"
            );
            let small = rng.f80(-70, -2);
            assert_eq!(
                yl2xp1_n::<8>(b, small),
                yl2xp1_n::<12>(b, small),
                "yl2xp1 {small:x?}"
            );
            let e = rng.f80(-60, 0);
            assert_eq!(x2m1_n::<8>(e), x2m1_n::<12>(e), "x2m1 {e:x?}");
            count += 9;
        }
        count
    }
}
