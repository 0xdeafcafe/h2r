//! Guarded, integer-only evaluation. See docs/lifter/x87-transcendentals.md for
//! domains, error budgets and the rounding/flag acceptance test.
#![forbid(unsafe_code)]

use super::*;
use core::cmp::Ordering;

type Fast = Wf<2>;

fn operand(a: F80) -> Option<Fin> {
    match unpack(a) {
        Unp::Fin(f) if !f.denorm => Some(f),
        _ => None,
    }
}

/// Accept only an interval entirely inside one rounding cell AND on one
/// side of its representable result. The latter preserves PE and C1.
/// This radius encloses both the 128-bit approximation error and the
/// reference's error, including endpoint arithmetic's truncation.
fn certify(v: Fast) -> Option<(F80, Flags)> {
    if v.is_zero() {
        return None;
    }
    let radius = Fast::one().scale(v.top() - 80);
    let lo = v.sub(radius).to_f80(0);
    let hi = v.add(radius).to_f80(0);
    (lo == hi && lo.1.contains(Flags::PE)).then_some(lo)
}

pub(super) fn yl2x(y: F80, x: F80) -> Option<(F80, Flags)> {
    let (fy, fx) = (operand(y)?, operand(x)?);
    if fx.neg || !(-1074..=1023).contains(&fy.top()) {
        return None;
    }
    let (lg, exact) = log2_fast(wf(fx));
    if exact {
        return None;
    }
    certify(wf::<2>(fy).mul(lg))
}

/// The first and last `j` of [`LOG2_J64`]: `1 + j/64` spans the reduced
/// significand's range `[sqrt(2)/2, sqrt(2)]`.
const LOG2_J_MIN: i64 = -19;
const LOG2_J_MAX: i64 = 27;

/// `floor(|log2(1 + j/64)| * 2^128)` for `j` in `LOG2_J_MIN..=LOG2_J_MAX`,
/// from the 768-bit reference (`tests::log2_table_matches_the_768_bit_reference`).
const LOG2_J64: [[u64; 2]; 47] = include!("log2_j64.in");

/// `floor(log2(e) * 2^127)`, the leading bit `2^0`
/// (`tests::log2_e_matches_the_768_bit_reference`).
const LOG2_E: [u64; 2] = [0xb8aa_3b29_5c17_f0bb, 0xbe87_fed0_691d_3e88];

fn log2_e() -> Fast {
    Fast::from_be_limbs(&LOG2_E, 0)
}

/// log2 for the fast path: as [`log2_wf`], with the reduced significand
/// `m` split as `c * (m / c)`, `c = 1 + j/64` the nearest table point. The
/// series then runs on `s = (m - c) / (m + c)`, `|s| < 2^-7` instead of
/// `2^-2.5`: 9 terms rather than about 25 for the same 128 bits. Within the
/// same error budget (`tests::unrounded_errors_stay_within_the_documented_budgets`),
/// so [`certify`] accepts and rejects as before and an accepted result is
/// still the 512-bit reference's.
fn log2_fast(u: Fast) -> (Fast, bool) {
    let mut e = u.top();
    let mut m = u.scale(-e);
    let sqrt2 = Fast::from_parts(false, 0xb504_f333_f9de_6484, -63);
    if m.cmp_abs(&sqrt2) == Ordering::Greater {
        m = m.scale(-1);
        e += 1;
    }
    let one = Fast::one();
    if m.sub(one).is_zero() {
        return (Fast::from_i64(e as i64), true);
    }
    // m in [2^-0.5, 2^0.5]: its top bit is 2^-1 or 2^0. j = round(64(m - 1)).
    let sig = ((m.m[1] as u128) << 64) | m.m[0] as u128;
    let q126 = sig >> (-(m.exp + 126)) as u32; // m * 2^126
    let j = (((q126 + (1 << 119)) >> 120) as i64 - 64).clamp(LOG2_J_MIN, LOG2_J_MAX);
    let c = Fast::from_i64(64 + j).scale(-6);
    let num = m.sub(c);
    let ln_r = if num.is_zero() {
        Fast::ZERO
    } else {
        atanh_series(num.div(m.add(c))).0.scale(1)
    };
    let mut lg = ln_r.mul(log2_e());
    if j != 0 {
        let t = Fast::from_be_limbs(&LOG2_J64[(j - LOG2_J_MIN) as usize], -1);
        lg = lg.add(if j < 0 { t.neg() } else { t });
    }
    (lg.add(Fast::from_i64(e as i64)), false)
}

pub(super) fn x2m1(a: F80) -> Option<(F80, Flags)> {
    let f = operand(a)?;
    if !(-1138..=-1).contains(&f.top()) {
        return None;
    }
    certify(expm1_series(wf::<2>(f).mul(ln2::<2>())).0)
}

/// The reduction's working width: 192 bits, so that the fraction of
/// `|x|*2/pi` keeps at least 109 correct bits after cancellation.
type Wide = Wf<3>;

/// Reject a reduced fraction more than this many binades below `|x|*2/pi`.
const REDUCE_SLACK: i32 = 80;

/// Keep the leading 128 bits (truncation).
fn narrow<const N: usize>(v: Wf<N>) -> Fast {
    if v.is_zero() {
        return Fast::ZERO;
    }
    Fast {
        neg: v.neg,
        exp: v.exp + 64 * (N as i32 - 2),
        m: [v.m[N - 2], v.m[N - 1]],
    }
}

/// The trig operand: every specially handled or denormal operand returns
/// None, as does an operand at or beyond 2^63 (C2).
fn trig_operand(a: F80) -> Option<Fin> {
    match trig_pre(a, false) {
        Ok(f) if !f.denorm => Some(f),
        _ => None,
    }
}

/// `|x| = r + k*pi/2` with `|r| <= pi/4` (to within the reduction error),
/// as in the reference's `reduce`. None when cancellation leaves too few
/// correct bits in the fraction (x close to a multiple of pi/2).
fn reduce_fast(f: Fin) -> Option<(Fast, u64)> {
    let x = Fast::from_parts(false, f.sig, f.exp);
    let half_pi = pi::<2>().scale(-1);
    if x.cmp_abs(&half_pi.scale(-1)) == Ordering::Less {
        return Some((x, 0));
    }
    let y = Wide::from_parts(false, f.sig, f.exp).mul(two_over_pi::<3>());
    let (mut k, mut fr) = y.int_frac();
    if !fr.is_zero() && fr.top() == -1 {
        k = k.wrapping_add(1);
        fr = fr.sub(Wide::one());
    }
    if fr.is_zero() || fr.top() < y.top() - REDUCE_SLACK {
        return None;
    }
    Some((narrow(fr).mul(half_pi), k))
}

/// Below 2^-32, `sin x`, `tan x` and `cos x` lie strictly within one
/// half-ulp of `x`, `x` and one, on a known side; the reference's packer
/// rounds them with that direction. Above -16000 the result stays normal.
const TINY: core::ops::RangeInclusive<i32> = -16000..=-33;

/// A reduced |r| below 2^-33 means the true |r| is below 2^-32 even with
/// the reduction error, and nonzero, so cos r is strictly inside
/// (1 - 2^-65, 1).
const TINY_R: i32 = -34;

/// A trig value before rounding: `(v, 0)` is an approximation for
/// [`certify`]; `(±1, -1)` is an exact one whose true value lies just below
/// it in magnitude, rounded with that direction like the reference.
type Approx = (Fast, i8);

fn round(a: Approx) -> Option<(F80, Flags)> {
    match a {
        (v, 0) => certify(v),
        (v, tail) => Some(v.to_f80(tail)),
    }
}

fn rounded(a: Approx) -> Option<Transcendental> {
    round(a).map(|(value, flags)| done(value, flags))
}

/// cos r, or one (rounded from below) when r is tiny.
fn cos_r(r: Fast) -> Approx {
    if r.is_zero() || r.top() <= TINY_R {
        (Fast::one(), -1)
    } else {
        (cos_series(r).0, 0)
    }
}

fn negate((v, t): Approx, neg: bool) -> Approx {
    (if neg { v.neg() } else { v }, t)
}

/// sin x before rounding (outside the tiny range).
fn sin_value(f: Fin) -> Option<Approx> {
    let (r, k) = reduce_fast(f)?;
    let s = if k & 1 == 0 {
        (sin_series(r).0, 0)
    } else {
        cos_r(r)
    };
    Some(negate(s, (k & 2 != 0) != f.neg))
}

/// cos x before rounding (outside the tiny range).
fn cos_value(f: Fin) -> Option<Approx> {
    let (r, k) = reduce_fast(f)?;
    let c = if k & 1 == 0 {
        cos_r(r)
    } else {
        (sin_series(r).0, 0)
    };
    Some(negate(c, k.wrapping_add(1) & 2 != 0))
}

/// tan x before rounding (outside the tiny range).
fn tan_value(f: Fin) -> Option<Fast> {
    let (r, k) = reduce_fast(f)?;
    let (s, c) = (sin_series(r).0, cos_series(r).0);
    let t = if k & 1 == 0 { s.div(c) } else { c.div(s).neg() };
    Some(if f.neg { t.neg() } else { t })
}

pub(super) fn sin(a: F80) -> Option<Transcendental> {
    let f = trig_operand(a)?;
    if TINY.contains(&f.top()) {
        // sin x = x (1 - x^2/6 + ...): just below x in magnitude.
        return rounded((wf(f), -1));
    }
    rounded(sin_value(f)?)
}

pub(super) fn cos(a: F80) -> Option<Transcendental> {
    let f = trig_operand(a)?;
    if f.top() <= *TINY.end() {
        // cos x = 1 - x^2/2 + ...: just below one.
        return rounded((Fast::one(), -1));
    }
    rounded(cos_value(f)?)
}

pub(super) fn sincos(a: F80) -> Option<(Transcendental, F80)> {
    let f = trig_operand(a)?;
    let ((sv, sf), (cv, cf)) = if TINY.contains(&f.top()) {
        (wf::<2>(f).to_f80(-1), Fast::one().to_f80(-1))
    } else {
        (round(sin_value(f)?)?, round(cos_value(f)?)?)
    };
    Some((done(sv, sincos_flags(sf, cf)), cv))
}

pub(super) fn tan(a: F80) -> Option<Transcendental> {
    let f = trig_operand(a)?;
    if TINY.contains(&f.top()) {
        // tan x = x (1 + x^2/3 + ...): just above x in magnitude.
        return rounded((wf(f), 1));
    }
    rounded((tan_value(f)?, 0))
}

/// floor(atan(j/64) * 2^128) for j = 0..=64, most significant limb first:
/// absolute error below 2^-128. Generated from, and checked against, the
/// 768-bit reference by `atan_table_matches_the_768_bit_reference`.
const ATAN_J64: [[u64; 2]; 65] = [
    [0x0000000000000000, 0x0000000000000000],
    [0x03ffeaab776e5356, 0xef9e31590057dd81],
    [0x07ff556eea5d892a, 0x13bcebbb6ed46310],
    [0x0bfdc0c2186d14fc, 0xf220e10d61df56ec],
    [0x0ffaaddb967ef4e3, 0x6cb2792dc0e2e0d5],
    [0x13f59f0e7c559d6b, 0x1338a177e11cd9be],
    [0x17ee182602f10e8c, 0x126acfcf099f06ce],
    [0x1be39ebe6f07c37d, 0xee3ca681661cbb3d],
    [0x1fd5ba9aac2f6dc6, 0x5912f313e7d111de],
    [0x23c3f5f6086e4dc9, 0x6f4dd64a60e82be6],
    [0x27adddd18cc4d8b0, 0xd1d8674940d83fa1],
    [0x2b93023c7d84d3be, 0xad534ffbc30b7a65],
    [0x2f72f6979cb6044d, 0x1ec2d3e207271d21],
    [0x334d51d2d90c4c39, 0xec03cf68691bbace],
    [0x3721aea524c14408, 0xbd88697072d54bc0],
    [0x3aefabbe40ae6ce3, 0x2468a9a2cbef5e39],
    [0x3eb6ebf25901bac5, 0x5b71e7bd7de885f9],
    [0x4277165f618d8962, 0xe47390cb8655e9d1],
    [0x462fd68c2fc5e098, 0x6523a458dfc414c6],
    [0x49e0dc815fbd16f8, 0x8322c92037f0a23d],
    [0x4d89dcdc1faf2f34, 0xe2d5da4c693d7994],
    [0x512a90db0abc26a2, 0xa1bc3aa4c45c6cf1],
    [0x54c2b6654735276d, 0x4cdbfbbdfbecf460],
    [0x5852100c273f8658, 0xda8ea8ee100507e1],
    [0x5bd86507937bc239, 0xc55190916e7f2241],
    [0x5f55812d8ecfdd69, 0xc885c2b249a08813],
    [0x62c934e5286c95b6, 0xd0ba3748fa85146e],
    [0x6633551535ac619e, 0x6c988fd0a76cdbe1],
    [0x6993bb0f308ff2db, 0x213e4af4800f389b],
    [0x6cea44769971b1ae, 0x187b1ca504031a2e],
    [0x7036d3253b27be33, 0xe318f6cb3cc65c01],
    [0x73794d0cb04d425d, 0x305bbe70e536e164],
    [0x76b19c1586ed3da2, 0xb7f222f65e1d4681],
    [0x79dfadfc5d68d10e, 0x53dc1bf34356f9fd],
    [0x7d03742d50505f2e, 0x33691e3eaee47661],
    [0x801ce39e0d205c99, 0xa6d6c6c54d938596],
    [0x832bf4a6d9867e2a, 0x4b6a09cb61a515c0],
    [0x8630a2dada1ed065, 0xd3e84ed5013ca37d],
    [0x892aecdfde9547b5, 0x094478fc472b4afb],
    [0x8c1ad445f3e09b8c, 0x439d801860205920],
    [0x8f005d5ef7f59f9b, 0x5c835e1665c43747],
    [0x91db8f1664f350e2, 0x10e4f9c1126e021f],
    [0x94ac72c9847186f6, 0x18c4f393f78a32f8],
    [0x97731420365e538b, 0xabd3fe19f1aeb6b2],
    [0x9a2f80e671bdda20, 0x4226f8e2204ff3bc],
    [0x9ce1c8e6a0b8cdb9, 0xf799c4e8174cf11c],
    [0x9f89fdc4f4b7a1ec, 0xf8b492644f0701df],
    [0xa22832dbcadaae08, 0x92fe9c08637af0e5],
    [0xa4bc7d1934f70924, 0x19a87f2a457dac9e],
    [0xa746f2ddb7602294, 0x67b7d66f2d74e019],
    [0xa9c7abdc4830f5c8, 0x916a84b5be7933f5],
    [0xac3ec0fb997dd6a1, 0xa36273a56afa8ef4],
    [0xaeac4c38b4d8c080, 0x14725e2f3e52070a],
    [0xb110688aebdc6f6a, 0x43d65788b9f6a7b5],
    [0xb36b31c91f043691, 0x590141744462f939],
    [0xb5bcc49059ecc4af, 0xf8f3cee75e3907d5],
    [0xb8053e2bc2319e73, 0xcb2da55210a4443d],
    [0xba44bc7dd470782f, 0x654c2cb10942e386],
    [0xbc7b5deae98af280, 0xd4113006e80fb290],
    [0xbea94144fd049aac, 0x1043c5e755282e7d],
    [0xc0ce85b8ac526640, 0x89dd62c46e92fa24],
    [0xc2eb4abb661628b5, 0xb373fe45c61bb9fa],
    [0xc4ffaffabf8fbd54, 0x8cb43d10bc9e0221],
    [0xc70bd54ce602ee13, 0xe7d54fbd09f2be38],
    [0xc90fdaa22168c234, 0xc4c6628b80dc1cd1],
];

fn atan_j64(j: usize) -> Fast {
    Fast::from_be_limbs(&ATAN_J64[j], -1)
}

/// The admitted operand exponents keep atan2's nonzero results normal.
const PATAN_EXPONENTS: core::ops::RangeInclusive<i32> = -4096..=4096;

/// atan(a/b) for 0 < a <= b: `atan(c) + atan((a - c*b)/(b + c*a))` with
/// c = j/64 nearest a/b, so the series argument is at most 2^-7.
fn atan_ratio(fa: Fin, fb: Fin) -> Fast {
    let a = Fast::from_parts(false, fa.sig, fa.exp);
    let b = Fast::from_parts(false, fb.sig, fb.exp);
    let d = fb.top() - fa.top();
    let j = if d > 8 {
        0
    } else {
        // floor(128*a/b), from the significands; then nearest j/64.
        let q = ((fa.sig as u128) << 7) / ((fb.sig as u128) << d);
        (q as usize).div_ceil(2)
    };
    if j == 0 {
        return atan_series(a.div(b)).0;
    }
    // c*b and c*a are exact in 71 bits; both sums are exact in 128 bits.
    let c = Fast::from_u64(j as u64).scale(-6);
    let num = a.sub(c.mul(b));
    let base = atan_j64(j);
    if num.is_zero() {
        return base;
    }
    base.add(atan_series(num.div(b.add(c.mul(a)))).0)
}

/// atan2(y, x) before rounding, for finite nonzero operands.
fn patan_value(fy: Fin, fx: Fin) -> Fast {
    let half_pi = pi::<2>().scale(-1);
    // |y| > |x|, compared exactly as the reference does.
    let swap = (fy.top(), fy.sig) > (fx.top(), fx.sig);
    let mut v = if swap {
        half_pi.sub(atan_ratio(fx, fy))
    } else {
        atan_ratio(fy, fx)
    };
    if fx.neg {
        v = pi::<2>().sub(v);
    }
    if fy.neg { v.neg() } else { v }
}

pub(super) fn patan(y: F80, x: F80) -> Option<(F80, Flags)> {
    let (fy, fx) = (operand(y)?, operand(x)?);
    if !PATAN_EXPONENTS.contains(&fy.top()) || !PATAN_EXPONENTS.contains(&fx.top()) {
        return None;
    }
    certify(patan_value(fy, fx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trans::tests::Rng;

    /// floor(atan(j/64) * 2^128) from the 768-bit reference.
    fn atan_fixed(j: u64) -> [u64; 2] {
        let v = atan_pos::<12>(Wf::from_u64(j).scale(-6)).0;
        let top = narrow(v);
        let m = ((top.m[1] as u128) << 64 | top.m[0] as u128) >> (-1 - v.top());
        [(m >> 64) as u64, m as u64]
    }

    /// floor(|log2(1 + j/64)| * 2^128) from the 768-bit reference.
    fn log2_fixed(j: i64) -> [u64; 2] {
        let v = log2_wf::<12>(Wf::from_i64(64 + j).scale(-6)).0;
        let top = narrow(v);
        let m = ((top.m[1] as u128) << 64 | top.m[0] as u128) >> (-1 - v.top());
        [(m >> 64) as u64, m as u64]
    }

    /// The table-driven log2 across every table point and both ends of the
    /// reduced range, against the 512-bit reference.
    #[test]
    fn yl2x_matches_the_reference_across_the_log2_table() {
        extended_yl2x(0x5f1e_77c3_a90d_2b64, 3000);
    }

    #[test]
    #[ignore = "slow; the default run covers a sample"]
    fn yl2x_matches_the_reference_across_the_log2_table_extended() {
        extended_yl2x(0x0b3d_91c4_e562_7a18, 300_000);
    }

    fn extended_yl2x(seed: u64, iters: usize) {
        let mut rng = Rng(seed);
        let mut hits = 0;
        for i in 0..iters {
            let y = if i % 3 == 0 {
                F80::LN2
            } else {
                rng.f80(-60, 60)
            };
            // Significands near each table point 1 + j/64, near the
            // rounding midpoints between them, and anywhere.
            // A significand in [0.5, 2) as a Q1.63 fixed-point number.
            let at = |q: u64| {
                if q >> 63 == 1 {
                    F80::new(16383, q)
                } else {
                    F80::new(16382, q << 1)
                }
            };
            // Near each table point 1 + j/64, near the rounding midpoints
            // between them, and anywhere.
            let j = (i / 4) as i64 % 47 + LOG2_J_MIN;
            let jitter = rng.next() >> (rng.next() % 60 + 4);
            let x = match i % 4 {
                0 => {
                    let point = ((64 + j) as u64) << 57;
                    at(if rng.next() & 1 == 0 {
                        point + jitter
                    } else {
                        point - jitter
                    })
                }
                1 => {
                    let mid = ((128 + 2 * j - 1) as u64) << 56;
                    at(if rng.next() & 1 == 0 {
                        mid + jitter
                    } else {
                        mid - jitter
                    })
                }
                _ => rng.f80(-200, 200).abs(),
            };
            let reference = yl2x_n::<8>(y, x);
            if let Some(fast) = yl2x(y, x) {
                assert_eq!(fast, reference, "y={y:x?} x={x:x?}");
                hits += 1;
            }
        }
        assert!(hits > iters * 9 / 10, "{hits}");
    }

    #[test]
    fn log2_e_matches_the_768_bit_reference() {
        let v = narrow(Wf::<12>::one().div(ln2::<12>()));
        assert_eq!(v.top(), 0);
        assert_eq!(
            LOG2_E,
            [v.m[1], v.m[0]],
            "{:#018x} {:#018x}",
            v.m[1],
            v.m[0]
        );
    }

    #[test]
    fn log2_table_matches_the_768_bit_reference() {
        for (i, entry) in LOG2_J64.iter().enumerate() {
            let j = i as i64 + LOG2_J_MIN;
            let want = if j == 0 { [0, 0] } else { log2_fixed(j) };
            assert_eq!(*entry, want, "j={j}");
        }
    }

    #[test]
    #[ignore = "prints LOG2_J64 (src/log2_j64.in)"]
    fn print_log2_table() {
        std::println!("[");
        for j in LOG2_J_MIN..=LOG2_J_MAX {
            let [hi, lo] = if j == 0 { [0, 0] } else { log2_fixed(j) };
            std::println!("    [{hi:#018x}, {lo:#018x}], // j = {j}");
        }
        std::println!("]");
    }

    #[test]
    fn atan_table_matches_the_768_bit_reference() {
        assert_eq!(ATAN_J64[0], [0, 0]);
        for (j, entry) in ATAN_J64.iter().enumerate().skip(1) {
            assert_eq!(*entry, atan_fixed(j as u64), "j={j}");
        }
    }

    #[test]
    #[ignore = "prints ATAN_J64"]
    fn print_atan_table() {
        for j in 0..=64 {
            let [hi, lo] = if j == 0 { [0, 0] } else { atan_fixed(j) };
            std::println!("    [{hi:#018x}, {lo:#018x}],");
        }
    }

    #[test]
    fn accepted_results_and_flags_match_512_and_768_bits() {
        let mut rng = Rng(0x8de3_f0a1_7c24_b695);
        let mut hits = [0; 2];
        for _ in 0..4000 {
            let y = rng.f80(-1074, 1023);
            let x = rng.f80(-1074, 1023).abs();
            let a = rng.f80(-1138, -1);
            let cases = [
                (yl2x(y, x), yl2x_n::<8>(y, x), yl2x_n::<12>(y, x)),
                (x2m1(a), x2m1_n::<8>(a), x2m1_n::<12>(a)),
            ];
            for (i, (fast, reference, wider)) in cases.into_iter().enumerate() {
                assert_eq!(reference, wider);
                if let Some(fast) = fast {
                    assert_eq!(fast, reference, "function {i}, y={y:x?} x={x:x?} a={a:x?}");
                    hits[i] += 1;
                }
            }
        }
        assert!(hits.iter().all(|&n| n > 3900), "{hits:?}");
    }

    fn check_budget(fast: Fast, wide: Wf<12>, bits: i32) {
        let mut m = [0; 12];
        m[10..].copy_from_slice(&fast.m);
        let lifted = Wf::<12> {
            neg: fast.neg,
            exp: fast.exp - 640,
            m,
        };
        let error = lifted.sub(wide);
        assert!(
            error.is_zero() || error.top() < wide.top() - bits,
            "error top={} result top={} budget={bits}",
            error.top(),
            wide.top()
        );
    }

    #[test]
    fn unrounded_errors_stay_within_the_documented_budgets() {
        let check = check_budget;
        let mut rng = Rng(0x953c_f42a_61d7_b08e);
        for i in 0..2000 {
            let y = operand(rng.f80(-1074, 1023)).unwrap();
            let x = if i < 4 {
                // Both nearest neighbours of one, with both signs of y.
                operand(F80::new(
                    if i & 1 == 0 { 16383 } else { 16382 },
                    if i & 1 == 0 { (1 << 63) + 1 } else { u64::MAX },
                ))
                .unwrap()
            } else {
                operand(rng.f80(-1074, 1023).abs()).unwrap()
            };
            check(
                wf::<2>(y).mul(log2_fast(wf(x)).0),
                wf::<12>(y).mul(log2_wf::<12>(wf(x)).0),
                96,
            );
            let a = operand(rng.f80(-1138, -1)).unwrap();
            check(
                expm1_series(wf::<2>(a).mul(ln2::<2>())).0,
                expm1_series(wf::<12>(a).mul(ln2::<12>())).0,
                96,
            );
        }
    }

    #[test]
    fn rejects_specials_domains_exact_values_and_rounding_boundaries() {
        for a in [
            F80::ZERO,
            F80::ZERO.neg(),
            F80::INFINITY,
            F80::INDEFINITE,
            F80::new(0x7fff, 0x8000_0000_0000_0001),
            F80::new(0, 1),
            F80::new(0, 1 << 63),
            F80::new(0x3fff, 1),
        ] {
            assert!(operand(a).is_none(), "{a:x?}");
            assert_eq!(yl2x(F80::ONE, a), None);
            assert_eq!(x2m1(a), None);
        }
        assert_eq!(yl2x(F80::ONE, F80::ONE.neg()), None);
        assert_eq!(yl2x(F80::ONE, F80::ONE), None);
        assert_eq!(x2m1(F80::ONE), None);
        // Both exact values and half-ulp ties are deliberately ambiguous.
        let exact = Fast::one();
        let midpoint = exact.add(Fast::one().scale(-64));
        for center in [exact, midpoint] {
            assert_eq!(certify(center), None);
            for sign in [false, true] {
                let mut v = center.add(Fast::one().scale(-82));
                v.neg = sign;
                assert_eq!(certify(v), None);
                let mut v = center.add(Fast::one().scale(-70));
                v.neg = sign;
                assert!(certify(v).is_some());
            }
        }
    }

    #[test]
    fn public_dispatch_matches_reference_at_domain_edges() {
        for e in [-16382, -1139, -1138, -1075, -1074, -1, 0, 1023, 1024, 16383] {
            for sig in [1 << 63, (1 << 63) + 1, u64::MAX] {
                for sign in [0, 0x8000] {
                    let a = F80::new(sign | (16383 + e) as u16, sig);
                    for b in [F80::ONE, F80::PI, F80::PI.neg()] {
                        assert_eq!(crate::yl2x(b, a), yl2x_n::<8>(b, a));
                    }
                    assert_eq!(crate::x2m1(a), x2m1_n::<8>(a));
                }
            }
        }
    }

    #[test]
    fn argument_exponent_sweeps_match_the_exact_helper() {
        let mut accepted = [0usize; 2];
        // Every normal ext80 x exponent; y spans binary32/binary64 edges.
        for e in -16382..=16383 {
            let ey = [-1074, -149, 0, 127, 1023][((e + 16382) % 5) as usize];
            let y = F80::new(
                (16383 + ey) as u16 | if e & 1 == 0 { 0 } else { 0x8000 },
                0xc90f_daa2_2168_c235,
            );
            let x = F80::new((16383 + e) as u16, 0xb504_f333_f9de_6484);
            let reference = yl2x_n::<8>(y, x);
            if let Some(v) = yl2x(y, x) {
                assert_eq!(v, reference, "FYL2X exponent {e}");
                accepted[0] += 1;
            }
            assert_eq!(crate::yl2x(y, x), reference);
        }
        // Every F2XM1 exponent in the guarded fractional domain, both signs
        // and significand edges. No game captures enter this synthetic test.
        for e in -1138..=-1 {
            for sig in [1 << 63, (1 << 63) + 1, 0xb172_17f7_d1cf_79ab, u64::MAX] {
                for sign in [0, 0x8000] {
                    let x = F80::new((16383 + e) as u16 | sign, sig);
                    let reference = x2m1_n::<8>(x);
                    if let Some(v) = x2m1(x) {
                        assert_eq!(v, reference, "F2XM1 exponent {e}");
                        accepted[1] += 1;
                    }
                    assert_eq!(crate::x2m1(x), reference);
                }
            }
        }
        assert!(accepted[0] > 32_000 && accepted[1] > 8_500, "{accepted:?}");
    }

    #[test]
    fn values_beyond_the_guard_use_the_reference() {
        for e in [-16382, -1139, 1024, 16383] {
            let y = F80::new((16383 + e) as u16, 0xb504_f333_f9de_6484);
            assert!(yl2x(y, F80::PI).is_none());
            assert_eq!(crate::yl2x(y, F80::PI), yl2x_n::<8>(y, F80::PI));
            if !(-1138..0).contains(&e) {
                assert!(x2m1(y).is_none());
                assert_eq!(crate::x2m1(y), x2m1_n::<8>(y));
            }
        }
    }

    #[test]
    fn multiplier_exponent_sweep_matches_the_reference() {
        let xs = [
            F80::new(16383, (1 << 63) + 1),
            F80::new(16382, u64::MAX),
            F80::PI,
        ];
        for e in -1074..=1023 {
            for sign in [0, 0x8000] {
                let y = F80::new((16383 + e) as u16 | sign, 0xb172_17f7_d1cf_79ab);
                for x in xs {
                    let reference = yl2x_n::<8>(y, x);
                    if let Some(v) = yl2x(y, x) {
                        assert_eq!(v, reference, "multiplier exponent {e}");
                    }
                    assert_eq!(crate::yl2x(y, x), reference);
                }
            }
        }
    }

    /// Sweep sizes: unoptimised builds run an eighth of each sweep (the
    /// 512/768-bit references dominate); release runs all of it.
    const SWEEP: usize = if cfg!(debug_assertions) { 8 } else { 1 };

    /// Every trig entry: fast result (if accepted) against the 512-bit and
    /// 768-bit references, the public dispatch against the reference, and
    /// the per-function acceptance count.
    fn check_trig(a: F80, hits: &mut [usize; 4]) {
        let cases = [
            (super::sin(a), sin_n::<8>(a), sin_n::<12>(a), crate::sin(a)),
            (super::cos(a), cos_n::<8>(a), cos_n::<12>(a), crate::cos(a)),
            (super::tan(a), tan_n::<8>(a), tan_n::<12>(a), crate::tan(a)),
        ];
        for (i, (fast, reference, wider, public)) in cases.into_iter().enumerate() {
            assert_eq!(reference, wider, "function {i}, a={a:x?}");
            assert_eq!(public, reference, "function {i}, a={a:x?}");
            if let Some(fast) = fast {
                assert_eq!(fast, reference, "function {i}, a={a:x?}");
                hits[i] += 1;
            }
        }
        let reference = sincos_n::<8>(a);
        assert_eq!(reference, sincos_n::<12>(a), "sincos a={a:x?}");
        assert_eq!(crate::sincos(a), reference, "sincos a={a:x?}");
        if let Some(fast) = super::sincos(a) {
            assert_eq!(fast, reference, "sincos a={a:x?}");
            hits[3] += 1;
        }
    }

    fn check_patan(y: F80, x: F80) -> bool {
        let reference = patan_n::<8>(y, x);
        assert_eq!(reference, patan_n::<12>(y, x), "y={y:x?} x={x:x?}");
        assert_eq!(crate::patan(y, x), reference, "y={y:x?} x={x:x?}");
        match super::patan(y, x) {
            Some(fast) => {
                assert_eq!(fast, reference, "y={y:x?} x={x:x?}");
                true
            }
            None => false,
        }
    }

    #[test]
    fn trig_accepted_results_and_flags_match_512_and_768_bits() {
        let mut rng = Rng(0x3b1f_77c0_d52e_9a46);
        // Game-like angles: 2^-32 <= |a| < 2^13 (binary32/binary64 radians
        // up to 8192), including the no-reduction range below pi/4.
        let mut hits = [0; 4];
        let n = 6000 / SWEEP;
        for _ in 0..n {
            check_trig(rng.f80(-32, 12), &mut hits);
        }
        std::println!("ACCEPT trig 2^-32..2^13 sin/cos/tan/sincos {hits:?} of {n}");
        assert!(hits.iter().all(|&h| h * 1000 > n * 999), "{hits:?}");
        // Every finite exponent below the C2 limit, tiny and huge included.
        let mut hits = [0; 4];
        for _ in 0..n {
            check_trig(rng.f80(-16382, 62), &mut hits);
        }
        std::println!("ACCEPT trig all finite exponents {hits:?} of {n}");
        assert!(hits.iter().all(|&h| h * 100 > n * 97), "{hits:?}");
        // The whole reduction range above the tiny guard.
        let mut hits = [0; 4];
        for _ in 0..n {
            check_trig(rng.f80(-32, 62), &mut hits);
        }
        std::println!("ACCEPT trig 2^-32..2^63 {hits:?} of {n}");
        assert!(hits.iter().all(|&h| h * 1000 > n * 998), "{hits:?}");
    }

    #[test]
    fn patan_accepted_results_and_flags_match_512_and_768_bits() {
        let mut rng = Rng(0xa4c2_09e7_5d31_f68b);
        for (lo, hi, n) in [
            (-20, 20, 6000 / SWEEP),
            (-1074, 1023, 3000 / SWEEP),
            (-4200, 4200, 3000 / SWEEP),
        ] {
            let mut hits = 0;
            for _ in 0..n {
                let (y, x) = (rng.f80(lo, hi), rng.f80(lo, hi));
                hits += check_patan(y, x) as usize;
            }
            // Out-of-guard exponents only occur in the last range.
            let need = if hi > 4096 { 940 } else { 999 };
            std::println!("ACCEPT patan exponents {lo}..{hi}: {hits} of {n}");
            assert!(hits * 1000 > n * need, "{lo}..{hi}: {hits}/{n}");
        }
        // Comparable magnitudes in every table cell: a/b near each j/64.
        for j in 1..=64u64 {
            for d in [-3i64, -1, 0, 1, 3] {
                for _ in 0..16 / SWEEP {
                    let x = rng.f80(-3, 3);
                    let ratio = (j << 57).wrapping_add_signed(d << 40) | 1 << 50;
                    let y = F80::new(
                        (x.se & 0x7fff) - 1 + (ratio >> 63) as u16,
                        if ratio >> 63 == 1 { ratio } else { ratio << 1 } | 1 << 63,
                    );
                    let y = F80::new(y.se ^ (rng.next() as u16 & 0x8000), y.sig);
                    check_patan(y, x);
                    check_patan(x, y);
                }
            }
        }
    }

    #[test]
    fn trig_unrounded_errors_stay_within_the_documented_budgets() {
        let mut rng = Rng(0x71e9_04ab_c35d_826f);
        for i in 0..3000 / SWEEP {
            let a = operand(rng.f80(if i % 2 == 0 { -32 } else { 4 }, 62)).unwrap();
            if let Some((v, 0)) = sin_value(a) {
                check_budget(v, sin_cos_wf::<12>(a).0.0, 100);
            }
            if let Some((v, 0)) = cos_value(a) {
                check_budget(v, sin_cos_wf::<12>(a).1.0, 100);
            }
            if let Some(v) = tan_value(a) {
                check_budget(v, tan_wf::<12>(a).0, 100);
            }
            let (y, x) = (
                operand(rng.f80(-60, 60)).unwrap(),
                operand(rng.f80(-60, 60)).unwrap(),
            );
            check_budget(patan_value(y, x), patan_wf::<12>(y, x).0, 100);
        }
    }

    /// The F80 nearest k*pi/4 and its neighbours: the reduction's worst
    /// cancellation, and the k / k+1 ties at odd multiples of pi/4.
    #[test]
    fn near_multiples_of_pi_over_four_match_the_reference() {
        let mut hits = [0; 4];
        let mut total = 0;
        let mut ks: std::vec::Vec<u64> = (1..=4096).step_by(SWEEP).collect();
        ks.extend([
            1 << 20,
            (1 << 40) + 3,
            (1 << 60) + 7,
            0xb7a9_3c1d_02f4_e6a5 >> 4,
        ]);
        for k in ks {
            let near = pi::<8>().mul_u64(k).scale(-2).to_f80(1).0;
            for d in [-2i64, -1, 0, 1, 2] {
                let sig = near.sig.wrapping_add_signed(d);
                if sig >> 63 == 1 && near.se & 0x7fff < 16383 + 63 {
                    for sign in [0, 0x8000] {
                        check_trig(F80::new(near.se | sign, sig), &mut hits);
                        total += 1;
                    }
                }
            }
        }
        // Even multiples of pi/2 cancel too far for the guard and use the
        // reference; everything else is accepted.
        std::println!("ACCEPT near k*pi/4 {hits:?} of {total}");
        assert!(
            hits.iter().all(|&h| h * 10 > total * 6),
            "{hits:?} of {total}"
        );
    }

    /// Rounded binary32/binary64 multiples of pi/2, the values a game uses.
    #[test]
    fn binary32_and_binary64_multiples_of_pi_match_the_reference() {
        let mut hits = [0; 4];
        let mut total = 0;
        for k in (-2000i32..=2000).step_by(SWEEP) {
            for frac in [1.0, 0.5, 0.25, 1.0 / 3.0] {
                let v = f64::from(k) * core::f64::consts::FRAC_PI_2 * frac;
                for a in [
                    F80::from_f64_bits(v.to_bits()),
                    F80::from_f32_bits((v as f32).to_bits()),
                ] {
                    check_trig(a, &mut hits);
                    total += 1;
                }
            }
        }
        std::println!("ACCEPT binary32/64 k*pi/2 {hits:?} of {total}");
        assert!(
            hits.iter().all(|&h| h * 100 > total * 99),
            "{hits:?} of {total}"
        );
    }

    #[test]
    fn trig_exponent_sweep_specials_and_c2_match_the_reference() {
        let mut hits = [0; 4];
        // Every exponent from the bottom of the normal range to 2^63 at the
        // tiny edges and the reduction top; a stride elsewhere.
        let exponents = (-16382..=62).filter(|e| !(-15990..=-210).contains(e) || e % 23 == 0);
        for e in exponents.chain([63, 64, 16383]) {
            for sig in [1 << 63, (1 << 63) + 1, 0xc90f_daa2_2168_c235, u64::MAX] {
                for sign in [0, 0x8000] {
                    let a = F80::new((16383 + e) as u16 | sign, sig);
                    check_trig(a, &mut hits);
                    if e >= 63 {
                        assert!(super::sin(a).is_none() && super::sincos(a).is_none());
                        assert!(sin_n::<8>(a).out_of_range && cos_n::<8>(a).out_of_range);
                    }
                }
            }
        }
        for a in [
            F80::ZERO,
            F80::ZERO.neg(),
            F80::INFINITY,
            F80::INFINITY.neg(),
            F80::INDEFINITE,
            F80::new(0x7fff, 0xc000_0000_0000_0001),
            F80::new(0x7fff, 0x8000_0000_0000_0001),
            F80::new(0, 1),
            F80::new(0x8000, 0x7fff_ffff_ffff_ffff),
            F80::new(0, 1 << 63),
            F80::new(0x3fff, 1),
            F80::new(0x403e, 0x7fff_ffff_ffff_ffff),
        ] {
            let mut none = [0; 4];
            check_trig(a, &mut none);
            assert_eq!(none, [0; 4], "{a:x?}");
        }
    }

    #[test]
    fn patan_quadrants_signs_specials_and_domain_edges() {
        let mags = [
            F80::ONE,
            F80::new(0x4000, 1 << 63),
            F80::new(0x3ffe, 1 << 63),
            F80::PI,
            F80::new(0x3fff, u64::MAX),
            F80::new(0x3ffe, u64::MAX),
            F80::new(0x3ff9, 0xaaaa_aaaa_aaaa_aaab),
        ];
        for y in mags {
            for x in mags {
                for (sy, sx) in [(0, 0), (0, 0x8000), (0x8000, 0), (0x8000, 0x8000)] {
                    let (y, x) = (F80::new(y.se | sy, y.sig), F80::new(x.se | sx, x.sig));
                    let accepted = check_patan(y, x);
                    if let Some((v, _)) = super::patan(y, x) {
                        // Sign of y; |result| > pi/2 exactly when x < 0.
                        assert_eq!(v.se & 0x8000, sy);
                        let above_half_pi = (v.abs().se, v.sig) > (0x3fff, 0xc90f_daa2_2168_c235);
                        assert_eq!(above_half_pi, sx != 0, "{y:x?} {x:x?}");
                    }
                    assert!(accepted || y.abs() == x.abs(), "{y:x?} {x:x?}");
                }
            }
        }
        let specials = [
            F80::ZERO,
            F80::ZERO.neg(),
            F80::INFINITY,
            F80::INFINITY.neg(),
            F80::INDEFINITE,
            F80::new(0x7fff, 0xc000_0000_0000_0001),
            F80::new(0x7fff, 0x8000_0000_0000_0001),
            F80::new(0, 1),
            F80::new(0x8000, 1 << 62),
            F80::new(0x3fff, 1),
            F80::ONE,
            F80::PI.neg(),
        ];
        for y in specials {
            for x in specials {
                let accepted = check_patan(y, x);
                assert_eq!(accepted, operand(y).is_some() && operand(x).is_some());
            }
        }
        for e in [-16382, -4097, -4096, 4096, 4097, 16383] {
            let big = F80::new((16383 + e) as u16, 0xb504_f333_f9de_6484);
            // Neither ratio is an exact power of two (see the guard notes).
            for other in [F80::new(0x4000, 0xa000_0000_0000_0001), F80::PI.neg()] {
                for (y, x) in [(big, other), (other, big)] {
                    let accepted = check_patan(y, x);
                    assert_eq!(accepted, (-4096..=4096).contains(&e), "{y:x?} {x:x?}");
                }
            }
        }
    }

    #[cfg(feature = "abi")]
    #[test]
    fn abi_matches_every_state_field_and_return_status() {
        use crate::abi::{self, X87Abi, unary_op::*};
        let mut rng = Rng(0x6e50_19b4_f2a3_7dc8);
        let edges = [
            F80::ZERO,
            F80::ONE,
            F80::ONE.neg(),
            F80::INFINITY,
            F80::INDEFINITE,
            F80::new(0, 1),
            F80::new(0x3fff, 1),
        ];
        for top in 0..8 {
            for cw in [
                0x007f, 0x027f, 0x037f, 0x017f, 0x067f, 0x0a7f, 0x0e7f, 0x035f, 0x037e, 0x037d,
            ] {
                for i in 0..64 {
                    let a = if i < edges.len() {
                        edges[i]
                    } else {
                        rng.f80(-1138, -1)
                    };
                    let b = rng.f80(-1074, 1023);
                    let mut initial = X87Abi {
                        control: cw,
                        status: top << 11 | (rng.next() as u16 & 0x472f),
                        instruction_pointer: rng.next() as u32,
                        data_pointer: rng.next() as u32,
                        opcode: rng.next() as u16,
                        instruction_selector: rng.next() as u16,
                        data_selector: rng.next() as u16,
                        ..X87Abi::default()
                    };
                    // All physical bytes, including empty-register remnants.
                    for reg in &mut initial.st {
                        *reg = rng.f80(-10, 10).to_bytes();
                    }
                    initial.set_st(0, a);
                    initial.set_st(1, b);
                    if i % 16 == 0 {
                        initial.tag = 0xffff;
                    }
                    if i % 17 == 0 {
                        initial.status |= abi::SW_ES | abi::SW_B;
                    }
                    let angle = rng.f80(-8, 12);
                    for op in [YL2X, X2M1, SIN, COS, TAN, SINCOS, PATAN] {
                        if matches!(op, SIN | COS | TAN | SINCOS) && i >= edges.len() {
                            initial.set_st(0, angle);
                        }
                        let (mut fast, mut reference) = (initial, initial);
                        assert_eq!(
                            abi::unary(&mut fast, op),
                            abi::unary_reference(&mut reference, op)
                        );
                        assert_eq!(fast, reference, "op={op}, cw={cw:x}, top={top}");
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "timing experiment; run serially in release mode"]
    fn benchmark_hot_transcendentals() {
        use std::{hint::black_box, time::Instant, vec::Vec};
        let mut rng = Rng(0x24a9_d3e7_01c6_b85f);
        // (multiplier, fraction, angle, atan2 operand). Angles are below 16
        // in magnitude, mostly within one turn.
        let inputs: Vec<_> = (0..256)
            .map(|_| {
                (
                    rng.f80(-3, 3),
                    rng.f80(-3, -1),
                    rng.f80(-4, 3),
                    rng.f80(-3, 3),
                )
            })
            .collect();
        let value = |op, fast, (y, x, a, w): (F80, F80, F80, F80)| -> F80 {
            match (op, fast) {
                (0, true) => {
                    yl2x(y, x.abs())
                        .unwrap_or_else(|| yl2x_n::<8>(y, x.abs()))
                        .0
                }
                (0, false) => yl2x_n::<8>(y, x.abs()).0,
                (1, true) => x2m1(x).unwrap_or_else(|| x2m1_n::<8>(x)).0,
                (1, false) => x2m1_n::<8>(x).0,
                (2, true) => super::sin(a).unwrap_or_else(|| sin_n::<8>(a)).value,
                (2, false) => sin_n::<8>(a).value,
                (3, true) => super::cos(a).unwrap_or_else(|| cos_n::<8>(a)).value,
                (3, false) => cos_n::<8>(a).value,
                (4, true) => super::tan(a).unwrap_or_else(|| tan_n::<8>(a)).value,
                (4, false) => tan_n::<8>(a).value,
                (5, true) => super::sincos(a).unwrap_or_else(|| sincos_n::<8>(a)).1,
                (5, false) => sincos_n::<8>(a).1,
                (6, true) => super::patan(y, w).unwrap_or_else(|| patan_n::<8>(y, w)).0,
                (6, false) => patan_n::<8>(y, w).0,
                _ => unreachable!(),
            }
        };
        for op in 0..7 {
            let mut results = [Vec::new(), Vec::new()];
            // Paired, alternating order; discard five warmup pairs.
            for batch in 0..36 {
                for implementation in [batch % 2, 1 - batch % 2] {
                    let fast = implementation == 1;
                    let start = Instant::now();
                    for &input in &inputs {
                        let value = value(op, fast, black_box(input));
                        black_box(value);
                    }
                    let ns = start.elapsed().as_nanos() as f64 / inputs.len() as f64;
                    if batch >= 5 {
                        results[implementation].push(ns);
                    }
                }
            }
            for (implementation, mut samples) in results.into_iter().enumerate() {
                samples.sort_by(f64::total_cmp);
                let mut deviations: Vec<_> =
                    samples.iter().map(|n| (n - samples[15]).abs()).collect();
                deviations.sort_by(f64::total_cmp);
                std::println!(
                    "TRANS_BENCH op={op} fast={} median_ns={:.2} mad_ns={:.2} min_ns={:.2}",
                    implementation == 1,
                    samples[15],
                    deviations[15],
                    samples[0]
                );
            }
        }
    }
}
