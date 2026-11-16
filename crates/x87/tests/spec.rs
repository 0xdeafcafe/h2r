//! The test list of docs/lifter/FLOAT-ABI.md §6, groups 1-8 (group 8's layout half
//! is in tests/abi.rs).
//!
//! Expected values are hex literals. Each comment says where the value comes
//! from: the SDM definition or table it follows, or the exactness argument
//! that fixes it. Values marked "integer arithmetic" were derived with exact
//! big-integer arithmetic from the published hex digits of pi
//! (3.243F6A88 85A308D3 ..., the Blowfish P-array) or from an integer square
//! root; no emulator or floating-point library produced any expectation.

use x87::*;

const fn f(se: u16, sig: u64) -> F80 {
    F80 { sig, se }
}

const J: u64 = 1 << 63;
const ONE: F80 = f(0x3fff, J);
const NEG_ONE: F80 = f(0xbfff, J);
const TWO: F80 = f(0x4000, J);
const THREE: F80 = f(0x4000, 0xc000_0000_0000_0000);
const HALF: F80 = f(0x3ffe, J);
const PZERO: F80 = f(0x0000, 0);
const NZERO: F80 = f(0x8000, 0);
const PINF: F80 = f(0x7fff, J);
const NINF: F80 = f(0xffff, J);
const MAX: F80 = f(0x7ffe, !0);
const MIN_NORMAL: F80 = f(0x0001, J); // 2^-16382
const MIN_DENORMAL: F80 = f(0x0000, 1); // 2^-16445
const PSEUDO_DENORMAL: F80 = f(0x0000, J); // value 2^-16382
const QNAN: F80 = f(0x7fff, 0xc000_0000_0000_0001);
const SNAN: F80 = f(0x7fff, 0x8000_0000_0000_0001);
const PSEUDO_NAN: F80 = f(0x7fff, 0x4000_0000_0000_0001);
const PSEUDO_INF: F80 = f(0x7fff, 0);
const UNNORMAL: F80 = f(0x3fff, 0x4000_0000_0000_0000);
const IND: F80 = f(0xffff, 0xc000_0000_0000_0000);

const EXT: Mode = Mode::EXTENDED;
const fn m(rc: Rc, pc: Pc) -> Mode {
    Mode { rc, pc }
}
const RCS: [Rc; 4] = [Rc::Near, Rc::Down, Rc::Up, Rc::Chop];

const NONE: Flags = Flags::NONE;
const IE: Flags = Flags::IE;
const DE: Flags = Flags::DE;
const ZE: Flags = Flags::ZE;
const OE: Flags = Flags::OE;
const UE: Flags = Flags::UE;
const PE: Flags = Flags::PE;
const C1: Flags = Flags::C1;
/// Not an exception: the result is tiny (see [`Flags::TINY`]).
const TINY: Flags = Flags::TINY;

// ============================================================ group 1
// Special values: SDM Vol. 1 §4.8.3 (NaNs), Table 4-7 (NaN selection),
// §8.2.2 (unsupported encodings) and each instruction's special-case table
// in Vol. 2.

#[test]
fn g1_indefinite_and_classes() {
    // SDM Vol. 1 Table 4-3: the real indefinite is the negative QNaN with
    // only the quiet bit set.
    assert_eq!(F80::INDEFINITE, f(0xffff, 0xc000_0000_0000_0000));
    assert_eq!(F80::INDEFINITE.classify(), Class::QNaN);
    // SDM Vol. 1 §8.2.2 / Table 8-3: encodings with J=0 and a nonzero
    // exponent are unsupported on the 387 and later; J=1 with a zero
    // exponent is a pseudo-denormal, processed as a denormal.
    assert_eq!(PSEUDO_NAN.classify(), Class::Unsupported);
    assert_eq!(PSEUDO_INF.classify(), Class::Unsupported);
    assert_eq!(UNNORMAL.classify(), Class::Unsupported);
    assert_eq!(PSEUDO_DENORMAL.classify(), Class::Denormal);
    assert_eq!(MIN_DENORMAL.classify(), Class::Denormal);
    assert_eq!(SNAN.classify(), Class::SNaN);
    assert_eq!(QNAN.classify(), Class::QNaN);
    assert_eq!(PINF.classify(), Class::Inf);
    assert_eq!(NZERO.classify(), Class::Zero);
    assert_eq!(ONE.classify(), Class::Normal);
}

#[test]
fn g1_unsupported_operands_are_invalid() {
    // SDM Vol. 1 §8.2.2: an unsupported operand raises #IA and the masked
    // response is the indefinite.
    for bad in [PSEUDO_NAN, PSEUDO_INF, UNNORMAL] {
        assert_eq!(add(bad, ONE, EXT), (IND, IE));
        assert_eq!(add(ONE, bad, EXT), (IND, IE));
        assert_eq!(mul(bad, ONE, EXT), (IND, IE));
        assert_eq!(div(ONE, bad, EXT), (IND, IE));
        assert_eq!(sqrt(bad, EXT), (IND, IE));
        assert_eq!(rndint(bad, Rc::Near), (IND, IE));
        assert_eq!(scale(bad, ONE, Rc::Near), (IND, IE));
        assert_eq!(xtract(bad), (IND, IND, IE));
        assert_eq!(prem(bad, ONE).value, IND);
        assert_eq!(sin(bad).value, IND);
        assert_eq!(patan(bad, ONE), (IND, IE));
        assert_eq!(yl2x(ONE, bad), (IND, IE));
        assert_eq!(x2m1(bad), (IND, IE));
        assert_eq!(compare(bad, ONE, true), (Cmp::Unordered, IE));
        // binary32 / binary64 indefinite (SDM Vol. 1 Table 4-3).
        assert_eq!(bad.to_f32_bits(EXT), (0xffc0_0000, IE));
        assert_eq!(bad.to_f64_bits(EXT), (0xfff8_0000_0000_0000, IE));
        assert_eq!(bad.to_i32(EXT, false), (i32::MIN, IE));
    }
}

#[test]
fn g1_nan_propagation() {
    let qa = f(0x7fff, 0xc000_0000_0000_0005);
    let qb = f(0xffff, 0xc000_0000_0000_0003);
    let sa = f(0x7fff, 0x8000_0000_0000_0003);
    let sb = f(0xffff, 0x8000_0000_0000_0005);
    // Table 4-7: two QNaNs, the larger significand wins (sign ignored).
    assert_eq!(add(qa, qb, EXT), (qa, NONE));
    assert_eq!(add(qb, qa, EXT), (qa, NONE));
    // Two SNaNs: the larger significand, quieted, with IE.
    assert_eq!(add(sa, sb, EXT), (f(0xffff, 0xc000_0000_0000_0005), IE));
    // SNaN and QNaN: x87 returns the QNaN, with IE for the SNaN.
    assert_eq!(mul(sb, qb, EXT), (qb, IE));
    assert_eq!(mul(qb, sb, EXT), (qb, IE));
    // NaN and a number: the NaN, quieted.
    assert_eq!(sub(ONE, SNAN, EXT), (f(0x7fff, 0xc000_0000_0000_0001), IE));
    assert_eq!(div(QNAN, ONE, EXT), (QNAN, NONE));
    // Equal significands: the destination operand (the first one here).
    let neg_q = f(0xffff, QNAN.sig);
    assert_eq!(add(QNAN, neg_q, EXT), (QNAN, NONE));
    assert_eq!(add(neg_q, QNAN, EXT), (neg_q, NONE));
    // A NaN beats a denormal operand: no DE.
    assert_eq!(add(QNAN, MIN_DENORMAL, EXT), (QNAN, NONE));
    // One-operand instructions quiet an SNaN with IE.
    assert_eq!(sqrt(SNAN, EXT), (f(0x7fff, 0xc000_0000_0000_0001), IE));
    assert_eq!(sin(SNAN).value, f(0x7fff, 0xc000_0000_0000_0001));
    assert_eq!(sin(SNAN).flags, IE);
    assert_eq!(x2m1(QNAN), (QNAN, NONE));
}

#[test]
fn g1_invalid_operations_without_nan_give_indefinite() {
    // SDM Vol. 1 §4.9.1.2 / §8.5.1: masked invalid operation -> indefinite.
    assert_eq!(add(PINF, NINF, EXT), (IND, IE));
    assert_eq!(sub(PINF, PINF, EXT), (IND, IE));
    assert_eq!(mul(PZERO, NINF, EXT), (IND, IE));
    assert_eq!(div(PZERO, NZERO, EXT), (IND, IE));
    assert_eq!(div(PINF, NINF, EXT), (IND, IE));
    assert_eq!(sqrt(NEG_ONE, EXT), (IND, IE));
    assert_eq!(sqrt(NINF, EXT), (IND, IE));
}

#[test]
fn g1_zeros_infinities_and_divide_by_zero() {
    // IEEE 754 §6.3: exact zero sums are +0 except under round-down.
    assert_eq!(add(PZERO, NZERO, EXT), (PZERO, NONE));
    assert_eq!(add(PZERO, NZERO, m(Rc::Down, Pc::Extended)), (NZERO, NONE));
    assert_eq!(add(NZERO, NZERO, EXT), (NZERO, NONE));
    assert_eq!(sub(ONE, ONE, EXT), (PZERO, NONE));
    assert_eq!(sub(ONE, ONE, m(Rc::Down, Pc::Double)), (NZERO, NONE));
    assert_eq!(mul(NZERO, ONE, EXT), (NZERO, NONE));
    assert_eq!(add(PINF, MAX, EXT), (PINF, NONE));
    assert_eq!(div(ONE, PINF, EXT), (PZERO, NONE));
    assert_eq!(div(NEG_ONE, PINF, EXT), (NZERO, NONE));
    // FDIV: finite nonzero / 0 is #Z and an infinity of the xor sign.
    assert_eq!(div(ONE, PZERO, EXT), (PINF, ZE));
    assert_eq!(div(ONE, NZERO, EXT), (NINF, ZE));
    assert_eq!(div(NEG_ONE, PZERO, EXT), (NINF, ZE));
    // FSQRT: sqrt(-0) = -0, sqrt(+inf) = +inf.
    assert_eq!(sqrt(NZERO, EXT), (NZERO, NONE));
    assert_eq!(sqrt(PINF, EXT), (PINF, NONE));
}

#[test]
fn g1_denormal_operands() {
    // #D is raised for a denormal operand of an arithmetic instruction
    // (SDM Vol. 1 §4.9.1.3); the operation then proceeds. x + 0 at PC=64 is
    // exact, so no UE/PE, but the result is tiny.
    assert_eq!(add(MIN_DENORMAL, PZERO, EXT), (MIN_DENORMAL, DE | TINY));
    // A pseudo-denormal is a denormal operand whose value is 2^-16382; the
    // result is the canonical (normal) encoding of that value.
    assert_eq!(add(PSEUDO_DENORMAL, PZERO, EXT), (MIN_NORMAL, DE));
    assert_eq!(mul(PSEUDO_DENORMAL, TWO, EXT), (f(0x0002, J), DE));
    // 2^-16445 * 2^16445 = 1 exactly (16445 = 0x403d); FSCALE reports #D.
    assert_eq!(
        scale(MIN_DENORMAL, f(0x400d, 0x807a_0000_0000_0000), Rc::Near),
        (ONE, DE)
    );
    // Denormal * inf is still DE and inf.
    assert_eq!(mul(MIN_DENORMAL, PINF, EXT), (PINF, DE));
    // Denormal / 0: ZE alone, as ZE outranks DE (SDM Vol. 1 §4.9.2), in
    // either sign. Invalid outranks both: 0/0 and denormal/SNaN give IE.
    assert_eq!(div(MIN_DENORMAL, PZERO, EXT), (PINF, ZE));
    assert_eq!(div(MIN_DENORMAL.neg(), PZERO, EXT), (NINF, ZE));
    assert_eq!(div(PZERO, PZERO, EXT), (IND, IE));
    assert_eq!(div(MIN_DENORMAL, SNAN, EXT).1, IE);
    // log2(0) is a zero divide that outranks a denormal y.
    assert_eq!(yl2x(MIN_DENORMAL, PZERO), (NINF, ZE));
    // sqrt(2^-16444) = 2^-8222 exactly.
    assert_eq!(sqrt(f(0x0000, 2), EXT), (f(0x1fe1, J), DE));
}

#[test]
fn g1_gradual_underflow() {
    // 2^-16382 * 0.5 = 2^-16383: exact denormal, so neither UE nor PE
    // (masked UE requires inexact, SDM Vol. 1 §4.9.1.5); TINY records it
    // for an unmasked UE.
    assert_eq!(
        mul(MIN_NORMAL, HALF, EXT),
        (f(0x0000, 0x4000_0000_0000_0000), TINY)
    );
    // 2^-16445 * 0.5 = 2^-16446 is a tie between 0 and the smallest
    // denormal: RN picks the even one (0); RC=up picks the denormal.
    assert_eq!(mul(MIN_DENORMAL, HALF, EXT), (PZERO, DE | UE | PE | TINY));
    assert_eq!(
        mul(MIN_DENORMAL, HALF, m(Rc::Up, Pc::Extended)),
        (MIN_DENORMAL, DE | UE | PE | C1 | TINY)
    );
    // The largest denormal times one is exact: tiny (it has 63 bits, so
    // rounding with an unbounded exponent leaves it below 2^-16382) but not
    // inexact, so no UE.
    let just_below = f(0x0000, 0x7fff_ffff_ffff_ffff);
    assert_eq!(mul(just_below, ONE, EXT), (just_below, DE | TINY));
    // (1 - 2^-64) * 2^-16382 is 2^63 - 1/2 denormal units: a tie, even ->
    // 2^63 units = 2^-16382. With an unbounded exponent the value has 64
    // bits and is exact, so it is tiny after rounding: UE.
    let a = f(0x3ffe, 0xffff_ffff_ffff_ffff); // 1 - 2^-64
    assert_eq!(
        mul(a, PSEUDO_DENORMAL, EXT),
        (MIN_NORMAL, DE | UE | PE | C1 | TINY)
    );
    // 2^-16382 - 2^-16436 = (1 - 2^-54) 2^-16382 at PC=53: rounding to 53
    // bits with an unbounded exponent ties to 2^-16382, so it is not tiny
    // after rounding: PE and C1 but no UE. At PC=64 it is an exact denormal.
    let small = f(0x0000, 0x200);
    assert_eq!(
        sub(MIN_NORMAL, small, m(Rc::Near, Pc::Double)),
        (MIN_NORMAL, DE | PE | C1)
    );
    assert_eq!(
        sub(MIN_NORMAL, small, EXT),
        (f(0x0000, 0x7fff_ffff_ffff_fe00), DE | TINY)
    );
    // 2^63 - 0x401 units at PC=53 rounds down to 2^63 - 0x800: tiny, UE.
    assert_eq!(
        sub(MIN_NORMAL, f(0x0000, 0x401), m(Rc::Near, Pc::Double)),
        (f(0x0000, 0x7fff_ffff_ffff_f800), DE | UE | PE | TINY)
    );
}

#[test]
fn g1_overflow_masked_response() {
    // SDM Vol. 1 Table 4-10: masked overflow gives inf or the largest
    // finite value of the destination precision, by RC and sign.
    assert_eq!(mul(MAX, TWO, EXT), (PINF, OE | PE | C1));
    assert_eq!(mul(MAX, TWO, m(Rc::Chop, Pc::Extended)), (MAX, OE | PE));
    assert_eq!(mul(MAX, TWO, m(Rc::Down, Pc::Extended)), (MAX, OE | PE));
    assert_eq!(mul(MAX, TWO, m(Rc::Up, Pc::Extended)), (PINF, OE | PE | C1));
    assert_eq!(
        mul(MAX.neg(), TWO, m(Rc::Up, Pc::Extended)),
        (MAX.neg(), OE | PE)
    );
    assert_eq!(
        mul(MAX.neg(), TWO, m(Rc::Down, Pc::Extended)),
        (NINF, OE | PE | C1)
    );
    // At PC=53 the largest finite has 53 significand bits.
    assert_eq!(
        mul(MAX, TWO, m(Rc::Chop, Pc::Double)),
        (f(0x7ffe, 0xffff_ffff_ffff_f800), OE | PE)
    );
    assert_eq!(
        mul(MAX, TWO, m(Rc::Chop, Pc::Single)),
        (f(0x7ffe, 0xffff_ff00_0000_0000), OE | PE)
    );
}

#[test]
fn g1_compare() {
    // FCOM signals on QNaN, FUCOM does not; both signal on SNaN.
    assert_eq!(compare(QNAN, ONE, false), (Cmp::Unordered, IE));
    assert_eq!(compare(QNAN, ONE, true), (Cmp::Unordered, NONE));
    assert_eq!(compare(ONE, SNAN, true), (Cmp::Unordered, IE));
    // +0 = -0; ordering across signs and infinities.
    assert_eq!(compare(PZERO, NZERO, false), (Cmp::Equal, NONE));
    assert_eq!(compare(NINF, MAX.neg(), false), (Cmp::Less, NONE));
    assert_eq!(
        compare(NEG_ONE, f(0xbfff, 0xc000_0000_0000_0000), false),
        (Cmp::Greater, NONE)
    );
    assert_eq!(compare(MIN_DENORMAL, PZERO, false), (Cmp::Greater, DE));
    assert_eq!(
        compare(PSEUDO_DENORMAL, MIN_NORMAL, false),
        (Cmp::Equal, DE)
    );
}

#[test]
fn g1_xtract_and_scale_specials() {
    // FXTRACT table: +-0 -> (-inf, +-0) with #Z; +-inf -> (+inf, +-inf).
    assert_eq!(xtract(NZERO), (NINF, NZERO, ZE));
    assert_eq!(xtract(NINF), (PINF, NINF, NONE));
    // 5.5 = 1.375 * 2^2.
    assert_eq!(
        xtract(f(0xc001, 0xb000_0000_0000_0000)),
        (TWO, f(0xbfff, 0xb000_0000_0000_0000), NONE)
    );
    // Smallest denormal: exponent -16445 (= -0x403d), significand 1.0.
    assert_eq!(
        xtract(MIN_DENORMAL),
        (f(0xc00d, 0x807a_0000_0000_0000), ONE, DE)
    );
    // FSCALE table.
    assert_eq!(scale(ONE, NINF, Rc::Near), (PZERO, NONE));
    // Scaling by zero is exact, but arithmetic never produces a
    // pseudo-denormal (SDM Vol. 1 §8.2.2): it comes back normalised, with
    // DE. A true denormal stays as it is and is tiny.
    let pseudo = f(0x0000, 0x8000_0000_0000_0001);
    assert_eq!(
        scale(pseudo, PZERO, Rc::Near),
        (f(0x0001, 0x8000_0000_0000_0001), DE)
    );
    assert_eq!(scale(PSEUDO_DENORMAL, NZERO, Rc::Near), (MIN_NORMAL, DE));
    assert_eq!(
        scale(MIN_DENORMAL, PZERO, Rc::Near),
        (MIN_DENORMAL, DE | TINY)
    );
    assert_eq!(scale(ONE, PZERO, Rc::Near), (ONE, NONE));
    assert_eq!(scale(NEG_ONE, PINF, Rc::Near), (NINF, NONE));
    assert_eq!(scale(PZERO, PINF, Rc::Near), (IND, IE));
    assert_eq!(scale(PINF, NINF, Rc::Near), (IND, IE));
    assert_eq!(scale(NZERO, THREE, Rc::Near), (NZERO, NONE));
    assert_eq!(scale(NINF, THREE, Rc::Near), (NINF, NONE));
    // trunc(2.9) = 2: 1 * 2^2 = 4; trunc(-2.9) = -2.
    let two_nine = f(0x4000, 0xb999_9999_9999_999a);
    assert_eq!(scale(ONE, two_nine, Rc::Near), (f(0x4001, J), NONE));
    assert_eq!(scale(ONE, two_nine.neg(), Rc::Near), (f(0x3ffd, J), NONE));
    // 1 * 2^16384 overflows; 1 * 2^-16445 is the smallest denormal,
    // 2^-16446 ties to 0 under RN and to the denormal under RC=up.
    assert_eq!(scale(ONE, f(0x400d, J), Rc::Near), (PINF, OE | PE | C1));
    assert_eq!(
        scale(ONE, f(0xc00d, 0x807a_0000_0000_0000), Rc::Near),
        (MIN_DENORMAL, TINY)
    );
    assert_eq!(
        scale(ONE, f(0xc00d, 0x807c_0000_0000_0000), Rc::Near),
        (PZERO, UE | PE | TINY)
    );
    assert_eq!(
        scale(ONE, f(0xc00d, 0x807c_0000_0000_0000), Rc::Up),
        (MIN_DENORMAL, UE | PE | C1 | TINY)
    );
    // Huge |b| is clamped and still overflows / underflows.
    assert_eq!(scale(ONE, f(0x7ffe, J), Rc::Chop), (MAX, OE | PE));
    assert_eq!(scale(ONE, f(0xfffe, J), Rc::Near), (PZERO, UE | PE | TINY));
}

#[test]
fn g1_transcendental_specials() {
    // FSIN/FCOS/FPTAN/FSINCOS tables: +-0 -> +-0 (cos: +1), +-inf -> #IA.
    assert_eq!(
        sin(NZERO),
        Transcendental {
            value: NZERO,
            out_of_range: false,
            flags: NONE
        }
    );
    assert_eq!(tan(NZERO).value, NZERO);
    assert_eq!(cos(NZERO).value, ONE);
    assert_eq!(sincos(PZERO).1, ONE);
    for t in [sin(PINF), cos(NINF), tan(PINF), sincos(NINF).0] {
        assert_eq!((t.value, t.flags, t.out_of_range), (IND, IE, false));
    }
    // FPATAN table (SDM Vol. 2 Table 3-29).
    let pi = (f(0x4000, 0xc90f_daa2_2168_c235), PE | C1); // RN(pi)
    let half_pi = (f(0x3fff, 0xc90f_daa2_2168_c235), PE | C1);
    let quarter_pi = (f(0x3ffe, 0xc90f_daa2_2168_c235), PE | C1);
    // 3pi/4 = 0x4000 96cbe3f9990e91a8 (RN, rounded up), integer arithmetic.
    let three_quarter_pi = (f(0x4000, 0x96cb_e3f9_990e_91a8), PE | C1);
    let neg = |(v, fl): (F80, Flags)| (v.neg(), fl);
    assert_eq!(patan(PZERO, NZERO), pi);
    assert_eq!(patan(NZERO, NZERO), neg(pi));
    assert_eq!(patan(PZERO, PZERO), (PZERO, NONE));
    assert_eq!(patan(NZERO, ONE), (NZERO, NONE));
    assert_eq!(patan(NZERO, NINF), neg(pi));
    assert_eq!(patan(ONE, NINF), pi);
    assert_eq!(patan(NEG_ONE, PINF), (NZERO, NONE));
    assert_eq!(patan(NEG_ONE, PZERO), neg(half_pi));
    assert_eq!(patan(PINF, NEG_ONE), half_pi);
    assert_eq!(patan(PINF, PINF), quarter_pi);
    assert_eq!(patan(NINF, PINF), neg(quarter_pi));
    assert_eq!(patan(PINF, NINF), three_quarter_pi);
    assert_eq!(patan(NINF, NINF), neg(three_quarter_pi));
    // FYL2X table.
    assert_eq!(yl2x(ONE, NEG_ONE), (IND, IE));
    assert_eq!(yl2x(ONE, NINF), (IND, IE));
    assert_eq!(yl2x(ONE, PZERO), (NINF, ZE));
    assert_eq!(yl2x(NEG_ONE, NZERO), (PINF, ZE));
    assert_eq!(yl2x(PZERO, PZERO), (IND, IE));
    assert_eq!(yl2x(NINF, PZERO), (PINF, NONE));
    assert_eq!(yl2x(PINF, ONE), (IND, IE));
    assert_eq!(yl2x(NEG_ONE, ONE), (NZERO, NONE));
    assert_eq!(yl2x(PZERO, HALF), (NZERO, NONE));
    assert_eq!(yl2x(PINF, HALF), (NINF, NONE));
    assert_eq!(yl2x(NZERO, PINF), (IND, IE));
    assert_eq!(yl2x(NEG_ONE, PINF), (NINF, NONE));
    // FYL2XP1 table.
    assert_eq!(yl2xp1(PINF, PZERO), (IND, IE));
    assert_eq!(yl2xp1(NEG_ONE, NZERO), (PZERO, NONE));
    assert_eq!(yl2xp1(ONE, NZERO), (NZERO, NONE));
    assert_eq!(yl2xp1(NINF, f(0xbffd, J)), (PINF, NONE));
    // F2XM1 table: +-0 -> +-0.
    assert_eq!(x2m1(NZERO), (NZERO, NONE));
}

#[test]
fn g1_transcendental_exact_values() {
    // 3 * log2(8) = 9 exactly: no PE.
    assert_eq!(
        yl2x(THREE, f(0x4002, J)),
        (f(0x4002, 0x9000_0000_0000_0000), NONE)
    );
    // log2(0.5) = -1 exactly.
    assert_eq!(yl2x(ONE, HALF), (NEG_ONE, NONE));
    // log2(1 + 1) = 1 and log2(1 - 0.5) = -1 exactly (outside the SDM range;
    // this crate's documented total behaviour).
    assert_eq!(yl2xp1(ONE, ONE), (ONE, NONE));
    assert_eq!(yl2xp1(ONE, f(0xbffe, J)), (NEG_ONE, NONE));
    // 2^1 - 1 = 1, 2^-1 - 1 = -0.5, 2^0.5 - 1 = sqrt(2) - 1.
    assert_eq!(x2m1(ONE), (ONE, NONE));
    assert_eq!(x2m1(NEG_ONE), (f(0xbffe, J), NONE));
    // sqrt(2) - 1 = 0x3ffd d413cccfe7799211 (fraction below the last bit
    // 0.398 ulp, so RN rounds down), integer square root of 2^401.
    assert_eq!(x2m1(HALF), (f(0x3ffd, 0xd413_cccf_e779_9211), PE));
    // 2^64 - 1 needs 64 bits: exact. 2^65 - 1 is a tie: even -> 2^65.
    assert_eq!(x2m1(f(0x4005, J)), (f(0x403e, !0), NONE));
    assert_eq!(
        x2m1(f(0x4005, 0x8200_0000_0000_0000)),
        (f(0x4040, J), PE | C1)
    );
    // log2(10) is the FLDL2T constant; pi/4 is RN(pi)/4.
    assert_eq!(yl2x(ONE, f(0x4002, 0xa000_0000_0000_0000)), (F80::L2T, PE));
    assert_eq!(patan(ONE, ONE), (f(0x3ffe, 0xc90f_daa2_2168_c235), PE | C1));
    // log2(1 + 2^-70) = 2^-70 log2(e) (1 - 2^-71 + ...). log2(e)'s bits
    // past 64 are 0.744 ulp, and the 2^-71 relative correction moves that
    // by under 0.01 ulp, so the result rounds up to FLDL2E's significand.
    assert_eq!(
        yl2xp1(ONE, f(0x3fb9, J)),
        (f(0x3fb9, 0xb8aa_3b29_5c17_f0bc), PE | C1)
    );
}

#[test]
fn g1_trig_exact_values_near_pi() {
    // d = RN(pi) - pi = 0x3fbe ece675d1fc8f8cbb.5c... (integer arithmetic
    // from the hex digits of pi; fraction 0.359 ulp, far from a midpoint).
    let pi80 = f(0x4000, 0xc90f_daa2_2168_c235);
    let d = f(0x3fbe, 0xece6_75d1_fc8f_8cbb);
    // sin(pi + d) = -sin d = -(d - d^3/6): rounds to -d, magnitude down.
    assert_eq!(sin(pi80).value, d.neg());
    assert_eq!(sin(pi80).flags, PE);
    // tan(pi + d) = tan d = d + d^3/3: rounds to d.
    assert_eq!(tan(pi80).value, d);
    // cos(pi + d) = -(1 - d^2/2): rounds to -1, magnitude up (C1).
    assert_eq!(cos(pi80).value, NEG_ONE);
    assert_eq!(cos(pi80).flags, PE | C1);
    // pi80/2 = pi/2 + d/2: sin = cos(d/2) -> 1 (C1); cos = -sin(d/2) -> -d/2.
    let half_pi80 = f(0x3fff, 0xc90f_daa2_2168_c235);
    assert_eq!(sin(half_pi80).value, ONE);
    assert_eq!(sin(half_pi80).flags, PE | C1);
    assert_eq!(cos(half_pi80).value, f(0xbfbd, 0xece6_75d1_fc8f_8cbb));
    let (s, c) = sincos(half_pi80);
    assert_eq!((s.value, c), (ONE, f(0xbfbd, 0xece6_75d1_fc8f_8cbb)));
    // Tiny arguments: sin x = x - x^3/6 rounds to x, which is larger in
    // magnitude (PE, C1); tan x = x + x^3/3 rounds down to x (PE only);
    // cos x rounds up to 1 (C1). Both a moderately tiny x (where x^3/6 is
    // within the working precision) and a very tiny one (where it is not).
    for tiny in [
        f(0x3f00, 0xa5a5_a5a5_a5a5_a5a5),
        f(0x0100, 0xa5a5_a5a5_a5a5_a5a5),
    ] {
        assert_eq!(sin(tiny).value, tiny);
        assert_eq!(sin(tiny).flags, PE | C1);
        assert_eq!(sin(tiny.neg()).value, tiny.neg());
        assert_eq!(sin(tiny.neg()).flags, PE | C1);
        assert_eq!(tan(tiny.neg()).value, tiny.neg());
        assert_eq!(tan(tiny.neg()).flags, PE);
        assert_eq!(patan(tiny, ONE), (tiny, PE | C1));
        assert_eq!(cos(tiny).value, ONE);
        assert_eq!(cos(tiny).flags, PE | C1);
    }
    let tiny = f(0x3f00, 0xa5a5_a5a5_a5a5_a5a5);
    assert_eq!(cos(tiny).value, ONE);
    assert_eq!(cos(tiny).flags, PE | C1);
    // A denormal operand: DE, and the result is tiny and inexact (UE).
    let s = sin(f(0x0000, 0x0000_0000_1234_5678));
    assert_eq!(s.value, f(0x0000, 0x0000_0000_1234_5678));
    assert_eq!(s.flags, DE | UE | PE | C1 | TINY);
}

// ============================================================ group 2
// Precision control: SDM Vol. 1 §8.1.5.2. The significand is rounded to the
// PC width; the exponent keeps its 15-bit range.

#[test]
fn g2_pc53_keeps_the_ext80_operand() {
    // The review's case: (1 + 2^-63) - 1 at PC=53. The operands are full
    // ext80 values; only the result is rounded to 53 bits. The exact
    // difference 2^-63 needs one significand bit, so it is exact.
    let a = f(0x3fff, 0x8000_0000_0000_0001);
    let r = sub(a, ONE, m(Rc::Near, Pc::Double));
    assert_eq!(r, (f(0x3fc0, J), NONE));
    // The same computation after narrowing the operand to double gives 0:
    // 1 + 2^-63 is not a double (it rounds to 1.0).
    let (a64, fl) = a.to_f64_bits(EXT);
    assert_eq!((a64, fl), (0x3ff0_0000_0000_0000, PE));
    assert_eq!(
        sub(F80::from_f64_bits(a64), ONE, m(Rc::Near, Pc::Double)),
        (PZERO, NONE)
    );
}

#[test]
fn g2_single_rounding_beats_double_rounding() {
    // 1 + (2^-53 + 2^-70): one rounding to 53 bits goes up (above the tie)
    // to 1 + 2^-52; rounding to 64 bits first would give 1 + 2^-53 and then
    // a tie to even, 1.
    let b = f(0x3fca, 0x8000_4000_0000_0000);
    assert_eq!(
        add(ONE, b, m(Rc::Near, Pc::Double)),
        (f(0x3fff, 0x8000_0000_0000_0800), PE | C1)
    );
    assert_eq!(add(ONE, b, EXT), (f(0x3fff, 0x8000_0000_0000_0400), PE));
    // 1 + 2^-24 is a tie at 24 bits: even -> 1. Exact at 53 and 64.
    let t = f(0x3fe7, J);
    assert_eq!(add(ONE, t, m(Rc::Near, Pc::Single)), (ONE, PE));
    assert_eq!(
        add(ONE, t, m(Rc::Near, Pc::Double)),
        (f(0x3fff, 0x8000_0080_0000_0000), NONE)
    );
    assert_eq!(add(ONE, t, EXT), (f(0x3fff, 0x8000_0080_0000_0000), NONE));
    // PC applies to mul/div/sqrt too: 1/3 at each width (0xaaaa... rounded).
    let third = |pc| div(ONE, THREE, m(Rc::Near, pc)).0;
    assert_eq!(third(Pc::Single), f(0x3ffd, 0xaaaa_ab00_0000_0000));
    assert_eq!(third(Pc::Double), f(0x3ffd, 0xaaaa_aaaa_aaaa_a800));
    assert_eq!(third(Pc::Extended), f(0x3ffd, 0xaaaa_aaaa_aaaa_aaab));
    // sqrt(2) = 0xb504f333f9de6484|597d... (integer square root): at 64
    // bits round down; at 24 bits 0xb504f3|33 rounds down; at 53 bits the
    // dropped 11 bits are 0x484 > 0x400, so it rounds up to ...6800 (the
    // binary64 sqrt(2), 0x3ff6a09e667f3bcd).
    assert_eq!(sqrt(TWO, EXT), (f(0x3fff, 0xb504_f333_f9de_6484), PE));
    assert_eq!(
        sqrt(TWO, m(Rc::Near, Pc::Single)),
        (f(0x3fff, 0xb504_f300_0000_0000), PE)
    );
    assert_eq!(
        sqrt(TWO, m(Rc::Near, Pc::Double)),
        (f(0x3fff, 0xb504_f333_f9de_6800), PE | C1)
    );
    assert_eq!(
        sqrt(TWO, m(Rc::Chop, Pc::Double)),
        (f(0x3fff, 0xb504_f333_f9de_6000), PE)
    );
    // Exact square roots are exact at every PC.
    assert_eq!(
        sqrt(f(0x4002, 0x9000_0000_0000_0000), m(Rc::Near, Pc::Single)),
        (THREE, NONE)
    );
}

#[test]
fn g2_exponent_range_is_15_bits_at_every_pc() {
    // 2^-16000 * 2^-382 = 2^-16382 at PC=24: normal in ext80 (a float or
    // double would underflow to 0).
    let a = f(0x3fff - 16000, J);
    let b = f(0x3fff - 382, J);
    for pc in [Pc::Single, Pc::Double, Pc::Extended] {
        assert_eq!(mul(a, b, m(Rc::Near, pc)), (MIN_NORMAL, NONE));
        // 2^8000 * 2^8000 = 2^16000: no overflow.
        assert_eq!(
            mul(f(0x3fff + 8000, J), f(0x3fff + 8000, J), m(Rc::Near, pc)),
            (f(0x3fff + 16000, J), NONE)
        );
    }
}

#[test]
fn g2_denormal_rounding_point_is_fixed_in_the_field() {
    // 2^-16383 * (1 + 2^-40) is the denormal field 0x4000000000400000. At
    // PC=24 the rounding point stays at field bit 40 (SDM PC semantics with
    // the extended exponent range, as SoftFloat extF80 implements), so the
    // 2^22 tail is dropped: PE and, being tiny, UE.
    let a = f(0x3ffe, 0x8000_0000_0080_0000); // 0.5 * (1 + 2^-40)
    assert_eq!(
        mul(MIN_NORMAL, a, m(Rc::Near, Pc::Single)),
        (f(0x0000, 0x4000_0000_0000_0000), UE | PE | TINY)
    );
    // At PC=53 the point is at field bit 11, below the 2^22 bit: exact.
    assert_eq!(
        mul(MIN_NORMAL, a, m(Rc::Near, Pc::Double)),
        (f(0x0000, 0x4000_0000_0040_0000), TINY)
    );
    assert_eq!(
        mul(MIN_NORMAL, a, EXT),
        (f(0x0000, 0x4000_0000_0040_0000), TINY)
    );
}

// ============================================================ group 3
// FPREM / FPREM1 (SDM Vol. 2 pseudo-code).

#[test]
fn g3_remainder_shortcuts_repack() {
    // Quotient 0 (|a| < |b|) and an infinite divisor return a as the
    // remainder, packed like any result: a true denormal is TINY, a
    // pseudo-denormal becomes its normal encoding; both are DE operands.
    let pseudo = f(0x0000, 0x8000_0000_0000_0001);
    for b in [ONE, PINF, NINF] {
        for p in [prem(MIN_DENORMAL, b), prem1(MIN_DENORMAL, b)] {
            assert_eq!(
                (p.value, p.incomplete, p.q, p.flags),
                (MIN_DENORMAL, false, 0, DE | TINY),
                "{b:?}"
            );
        }
        for p in [prem(pseudo, b), prem1(pseudo, b)] {
            assert_eq!(
                (p.value, p.incomplete, p.q, p.flags),
                (f(0x0001, 0x8000_0000_0000_0001), false, 0, DE),
                "{b:?}"
            );
        }
    }
    // A normal operand is unchanged and raises nothing.
    assert_eq!(prem(HALF, ONE).value, HALF);
    assert_eq!(prem(HALF, ONE).flags, NONE);
    assert_eq!(prem(ONE.neg(), PINF).value, NEG_ONE);
}

#[test]
fn g3_prem_and_prem1() {
    let five_five = f(0x4001, 0xb000_0000_0000_0000);
    // 5.5 = 2*2 + 1.5: FPREM truncates, Q = 2 (C0=0, C3=1, C1=0).
    let p = prem(five_five, TWO);
    assert_eq!(
        (p.value, p.incomplete, p.q, p.flags),
        (f(0x3fff, 0xc000_0000_0000_0000), false, 2, NONE)
    );
    // FPREM1 rounds 2.75 to 3: 5.5 - 6 = -0.5, Q = 3.
    let p = prem1(five_five, TWO);
    assert_eq!((p.value, p.incomplete, p.q), (f(0xbffe, J), false, 3));
    // Negative dividend: FPREM keeps the dividend's sign; FPREM1 is the
    // IEEE remainder -5.5 - 2*(-3) = 0.5.
    assert_eq!(
        prem(five_five.neg(), TWO).value,
        f(0xbfff, 0xc000_0000_0000_0000)
    );
    assert_eq!(prem(five_five.neg(), TWO).q, 2);
    assert_eq!(prem1(five_five.neg(), TWO).value, HALF);
    // FPREM1 ties go to the even quotient: 5 / 2 = 2.5 -> 2, remainder 1;
    // 7 / 2 = 3.5 -> 4, remainder -1.
    assert_eq!(prem1(f(0x4001, 0xa000_0000_0000_0000), TWO).value, ONE);
    assert_eq!(prem1(f(0x4001, 0xe000_0000_0000_0000), TWO).value, NEG_ONE);
    // |a| < |b|/2 and |b|/2 < |a| < |b| for FPREM1.
    assert_eq!(prem1(f(0x3ffd, J), ONE).value, f(0x3ffd, J)); // 0.25
    assert_eq!(
        prem1(f(0x3ffe, 0xc000_0000_0000_0000), ONE).value,
        f(0xbffd, J)
    ); // 0.75 -> -0.25
    assert_eq!(
        prem(f(0x3ffe, 0xc000_0000_0000_0000), ONE).value,
        f(0x3ffe, 0xc000_0000_0000_0000)
    );
    // Exact multiple: zero with the dividend's sign.
    assert_eq!(prem(f(0xc001, J), TWO).value, NZERO);
    // Specials: b = 0 or a = inf -> #IA; b = inf or a = 0 -> a.
    assert_eq!(prem(ONE, PZERO).value, IND);
    assert_eq!(prem(ONE, PZERO).flags, IE);
    assert_eq!(prem1(PINF, ONE).value, IND);
    assert_eq!(prem(ONE, NINF).value, ONE);
    assert_eq!(prem(NZERO, THREE).value, NZERO);
}

/// Run FPREM/FPREM1 as the guest loop does, until C2 clears.
fn iterate(a: F80, b: F80, nearest: bool) -> (Partial, u32) {
    let mut x = a;
    let mut n = 0;
    loop {
        let p = if nearest { prem1(x, b) } else { prem(x, b) };
        n += 1;
        if !p.incomplete {
            return (p, n);
        }
        assert_eq!(p.q, 0);
        // A partial remainder keeps the sign and shrinks the exponent.
        assert!(p.value.exponent_field() < x.exponent_field());
        x = p.value;
    }
}

#[test]
fn g3_large_exponent_difference_iterates() {
    let two_100 = f(0x3fff + 100, J);
    let two_200 = f(0x3fff + 200, J);
    let seven = f(0x4001, 0xe000_0000_0000_0000);
    let ten = f(0x4002, 0xa000_0000_0000_0000);
    // D = 99 >= 64: the first step is partial.
    assert!(prem(two_100, THREE).incomplete);
    // 2^100 mod 3 = 1 and trunc(2^100/3) = 0x5555...5 ends in 101b. Each
    // partial step removes a multiple of b * 2^(D-N) with D-N >= 32, so
    // the final step's quotient bits are the true quotient's low bits.
    let (p, n) = iterate(two_100, THREE, false);
    assert_eq!((p.value, p.q), (ONE, 5));
    assert!(n >= 2);
    let (p, _) = iterate(two_100, THREE, true);
    assert_eq!((p.value, p.q), (ONE, 5));
    // 2^100 mod 7 = 2, trunc(2^100/7) & 7 = 2.
    let (p, _) = iterate(two_100, seven, false);
    assert_eq!((p.value, p.q), (TWO, 2));
    // 2^200 mod 10 = 6, trunc(2^200/10) & 7 = 1; FPREM1 rounds up to the
    // next quotient: remainder 6 - 10 = -4, Q & 7 = 2.
    let (p, _) = iterate(two_200, ten, false);
    assert_eq!((p.value, p.q), (f(0x4001, 0xc000_0000_0000_0000), 1));
    let (p, _) = iterate(two_200, ten, true);
    assert_eq!((p.value, p.q), (f(0xc001, J), 2));
}

// ============================================================ group 4
// Trigonometric operand range: SDM Vol. 2 FSIN/FCOS/FPTAN/FSINCOS, "if the
// source operand is outside the range -2^63 to +2^63, C2 is set and ST(0)
// is unchanged".

#[test]
fn g4_trig_range() {
    let below = f(0x403d, !0); // 2^63 - 2^-1, the largest value below 2^63
    let at = f(0x403e, J); // 2^63
    let above = f(0x403e, J | 1);
    for x in [at, at.neg(), above, f(0x7ffe, J)] {
        for t in [sin(x), cos(x), tan(x), sincos(x).0] {
            assert_eq!(
                t,
                Transcendental {
                    value: x,
                    out_of_range: true,
                    flags: NONE
                }
            );
        }
    }
    for x in [below, below.neg()] {
        for t in [sin(x), cos(x), tan(x), sincos(x).0] {
            assert!(!t.out_of_range);
            assert!(t.flags.contains(PE));
            assert_eq!(t.flags.without(C1), PE);
        }
        // sin^2 + cos^2 = 1 to within a couple of ulps at 64 bits.
        let s = sin(x).value;
        let c = cos(x).value;
        let one = add(mul(s, s, EXT).0, mul(c, c, EXT).0, EXT).0;
        let diff = sub(one, ONE, EXT).0;
        assert!(diff.exponent_field() <= 0x3fff - 62, "{diff:x?}");
        let (ss, cc) = sincos(x);
        assert_eq!((ss.value, cc), (s, c));
    }
    for x in [PINF, NINF] {
        assert_eq!(sin(x).value, IND);
        assert!(!sin(x).out_of_range);
    }
    assert_eq!(sin(PZERO).value, PZERO);
    assert_eq!(cos(NZERO).value, ONE);
}

// ============================================================ group 5
// Rounding control (SDM Vol. 1 §4.8.4): ties, directed rounding of
// negative values, FRNDINT.

#[test]
fn g5_rounding_modes_on_ties() {
    let ulp_half = f(0x3fbf, J); // 2^-64: half an ulp of 1.0
    // 1 + 2^-64: tie between 1 (even) and 1 + 2^-63.
    let want = [ONE, ONE, f(0x3fff, J | 1), ONE];
    let up = [PE, PE, PE | C1, PE];
    for i in 0..4 {
        assert_eq!(
            add(ONE, ulp_half, m(RCS[i], Pc::Extended)),
            (want[i], up[i]),
            "{:?}",
            RCS[i]
        );
    }
    // Negative: -1 - 2^-64. Down moves away from zero, Up toward it.
    let want = [NEG_ONE, f(0xbfff, J | 1), NEG_ONE, NEG_ONE];
    let up = [PE, PE | C1, PE, PE];
    for i in 0..4 {
        assert_eq!(
            sub(NEG_ONE, ulp_half, m(RCS[i], Pc::Extended)),
            (want[i], up[i]),
            "{:?}",
            RCS[i]
        );
    }
    // (1 + 2^-63) + 2^-64: tie with an odd lower neighbour -> rounds up.
    let odd = f(0x3fff, J | 1);
    assert_eq!(add(odd, ulp_half, EXT), (f(0x3fff, J | 2), PE | C1));
    // 1/3 under each mode at 64 bits (0xaaaa...aa|aa...).
    let lo = f(0x3ffd, 0xaaaa_aaaa_aaaa_aaaa);
    let hi = f(0x3ffd, 0xaaaa_aaaa_aaaa_aaab);
    let want = [hi, lo, hi, lo];
    for i in 0..4 {
        assert_eq!(div(ONE, THREE, m(RCS[i], Pc::Extended)).0, want[i]);
        assert_eq!(
            div(NEG_ONE, THREE, m(RCS[i], Pc::Extended)).0,
            [hi, hi, lo, lo][i].neg()
        );
    }
}

#[test]
fn g5_rndint() {
    let v = |x: f64| F80::from_f64_bits(x.to_bits());
    // (operand, [Near, Down, Up, Chop]) - IEEE roundToIntegral by mode.
    let cases: [(f64, [f64; 4]); 9] = [
        (2.5, [2.0, 2.0, 3.0, 2.0]),
        (3.5, [4.0, 3.0, 4.0, 3.0]),
        (-2.5, [-2.0, -3.0, -2.0, -2.0]),
        (-3.5, [-4.0, -4.0, -3.0, -3.0]),
        (2.25, [2.0, 2.0, 3.0, 2.0]),
        (-2.75, [-3.0, -3.0, -2.0, -2.0]),
        (0.3, [0.0, 0.0, 1.0, 0.0]),
        (-0.3, [-0.0, -1.0, -0.0, -0.0]),
        (-0.5, [-0.0, -1.0, -0.0, -0.0]),
    ];
    for (x, want) in cases {
        for i in 0..4 {
            let (r, flags) = rndint(v(x), RCS[i]);
            assert_eq!(r, v(want[i]), "rndint({x}) {:?}", RCS[i]);
            assert!(flags.contains(PE));
            let magnitude_up = want[i].abs() > x.abs();
            assert_eq!(
                flags.contains(C1),
                magnitude_up,
                "C1 rndint({x}) {:?}",
                RCS[i]
            );
        }
    }
    // Already integral (including 2^63 and above): exact, unchanged.
    assert_eq!(
        rndint(f(0x4005, 0xc000_0000_0000_0000), Rc::Up),
        (f(0x4005, 0xc000_0000_0000_0000), NONE)
    );
    assert_eq!(rndint(f(0x403e, J | 1), Rc::Near), (f(0x403e, J | 1), NONE));
    // 2^63 - 0.5 is a tie: even -> 2^63.
    assert_eq!(rndint(f(0x403d, !0), Rc::Near), (f(0x403e, J), PE | C1));
    // Denormal: DE, rounds to zero or one.
    assert_eq!(rndint(MIN_DENORMAL, Rc::Near), (PZERO, DE | PE));
    assert_eq!(
        rndint(MIN_DENORMAL.neg(), Rc::Down),
        (NEG_ONE, DE | PE | C1)
    );
    assert_eq!(rndint(NINF, Rc::Near), (NINF, NONE));
}

// ============================================================ group 6
// Integer conversion (FIST/FISTP, SDM Vol. 2): out of range, NaN or
// infinity gives the integer indefinite (most negative value) and #IA.

#[test]
fn g6_integer_conversion() {
    let two31 = f(0x401e, J);
    let two63 = f(0x403e, J);
    // +2^31 does not fit an i32; -2^31 does.
    assert_eq!(two31.to_i32(EXT, false), (i32::MIN, IE));
    assert_eq!(two31.neg().to_i32(EXT, false), (i32::MIN, NONE));
    assert_eq!(two31.to_i64(EXT, false), (0x8000_0000, NONE));
    // 2^31 - 0.5 ties to even 2^31: out of range. Truncation gives 2^31-1.
    let t = f(0x401d, 0xffff_ffff_0000_0000);
    assert_eq!(t.to_i32(EXT, false), (i32::MIN, IE));
    assert_eq!(t.to_i32(EXT, true), (0x7fff_ffff, PE));
    assert_eq!(
        t.to_i32(m(Rc::Down, Pc::Extended), false),
        (0x7fff_ffff, PE)
    );
    // -2^31 - 0.5: RN ties to even (-2^31); RC=down overflows.
    let u = f(0xc01e, 0x8000_0000_8000_0000);
    assert_eq!(u.to_i32(EXT, false), (i32::MIN, PE));
    assert_eq!(u.to_i32(m(Rc::Down, Pc::Extended), false), (i32::MIN, IE));
    // 2^63 does not fit an i64; -2^63 does; 2^63 - 0.5 rounds out of range.
    assert_eq!(two63.to_i64(EXT, false), (i64::MIN, IE));
    assert_eq!(two63.neg().to_i64(EXT, false), (i64::MIN, NONE));
    assert_eq!(f(0x403d, !0).to_i64(EXT, true), (0x7fff_ffff_ffff_ffff, PE));
    assert_eq!(f(0x403d, !0).to_i64(EXT, false), (i64::MIN, IE));
    // NaN and infinity.
    for x in [QNAN, SNAN, PINF, NINF, IND] {
        assert_eq!(x.to_i32(EXT, false), (i32::MIN, IE));
        assert_eq!(x.to_i64(EXT, true), (i64::MIN, IE));
        assert_eq!(x.to_i16(EXT, false), (i16::MIN, IE));
    }
    // Directed rounding of -1.5 and the 16-bit width.
    let neg15 = f(0xbfff, 0xc000_0000_0000_0000);
    let want = [-2, -2, -1, -1];
    for i in 0..4 {
        assert_eq!(neg15.to_i32(m(RCS[i], Pc::Extended), false).0, want[i]);
    }
    assert_eq!(
        f(0x400d, 0xffff_0000_0000_0000).to_i16(EXT, false),
        (i16::MIN, IE)
    ); // 32767.5 -> 32768
    assert_eq!(
        f(0xc00e, 0x8000_0000_0000_0000).to_i16(EXT, false),
        (i16::MIN, NONE)
    ); // -32768
    assert_eq!(NZERO.to_i32(EXT, false), (0, NONE));
    // Loads are exact.
    assert_eq!(F80::from_i64(i64::MIN), f(0xc03e, J));
    assert_eq!(F80::from_i64(-1), NEG_ONE);
    assert_eq!(F80::from_i64(0), PZERO);
    assert_eq!(F80::from_i64(0x8000_0000), two31);
    assert_eq!(F80::from_i64(i64::MAX), f(0x403d, !0 << 1));
}

#[test]
fn g6_bcd() {
    // FBLD/FBSTP: 18 digits, sign in byte 9 bit 7.
    let m = [0x89, 0x67, 0x45, 0x23, 0x01, 0, 0, 0, 0, 0x80];
    let v = F80::from_bcd(m);
    assert_eq!(v, F80::from_i64(-123_456_789));
    assert_eq!(v.to_bcd(Rc::Near), (m, NONE));
    let max = [0x99; 9];
    let mut mm = [0u8; 10];
    mm[..9].copy_from_slice(&max);
    assert_eq!(F80::from_bcd(mm), F80::from_i64(999_999_999_999_999_999));
    // 10^18 is out of range: BCD indefinite ff ff c0 00 ... (high to low).
    let ind = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0xff];
    assert_eq!(
        F80::from_i64(1_000_000_000_000_000_000).to_bcd(Rc::Near),
        (ind, IE)
    );
    assert_eq!(QNAN.to_bcd(Rc::Near), (ind, IE));
    // Rounded by RC: 2.5 -> 2 (near), 3 (up).
    let two5 = f(0x4000, 0xa000_0000_0000_0000);
    assert_eq!(two5.to_bcd(Rc::Near).0[0], 0x02);
    assert_eq!(two5.to_bcd(Rc::Up).0[0], 0x03);
    // -0 keeps its sign byte.
    assert_eq!(NZERO.to_bcd(Rc::Near).0[9], 0x80);
}

// ============================================================ group 7
// Binary32/64 conversions: widening is exact (every value is an ext80
// value), narrowing rounds once (SDM FST, IEEE 754 §5.4.2).

/// The exact value of a binary32 encoding as an ext80, built from the
/// IEEE definition (independent of `from_f32_bits`'s code path).
fn f32_value(b: u32) -> F80 {
    let sign = if b >> 31 != 0 { 0x8000u16 } else { 0 };
    let e = (b >> 23) & 0xff;
    let frac = (b & 0x7f_ffff) as u64;
    match e {
        0 if frac == 0 => f(sign, 0),
        // frac * 2^-149 = (frac / 2^22) * 2^-127 ... normalise by hand.
        0 => {
            let top = 63 - frac.leading_zeros() as i32; // bit index of the leading 1
            let exp = top - 149;
            f(sign | (exp + 16383) as u16, frac << (63 - top))
        }
        0xff => f(sign | 0x7fff, J | frac << 40),
        _ => f(sign | (e as i32 - 127 + 16383) as u16, J | frac << 40),
    }
}

fn round_trip_f32(b: u32) {
    let x = F80::from_f32_bits(b);
    assert_eq!(x, f32_value(b), "widen {b:#010x}");
    let is_snan = (b >> 23) & 0xff == 0xff && b & 0x7f_ffff != 0 && b & 0x40_0000 == 0;
    for rc in RCS {
        let (back, fl) = x.to_f32_bits(m(rc, Pc::Extended));
        // A binary32 denormal is exact but tiny in its own format.
        let tiny = if (b >> 23) & 0xff == 0 && b & 0x7f_ffff != 0 {
            TINY
        } else {
            NONE
        };
        if is_snan {
            assert_eq!((back, fl), (b | 0x40_0000, IE), "{b:#010x}");
        } else {
            assert_eq!((back, fl), (b, tiny), "{b:#010x} {rc:?}");
        }
    }
    // Every binary32 is also exactly a binary64.
    if !is_snan {
        let (d, fl) = x.to_f64_bits(EXT);
        assert_eq!(fl, NONE);
        assert_eq!(F80::from_f64_bits(d), x);
    }
}

#[test]
fn g7_f32_round_trips_sampled() {
    // A stride through all 2^32 encodings plus every class boundary. The
    // exhaustive version is `g7_f32_round_trips_exhaustive` (ignored by
    // default; run it with --release --ignored).
    let mut b: u32 = 0;
    loop {
        round_trip_f32(b);
        match b.checked_add(65_521) {
            Some(n) => b = n,
            None => break,
        }
    }
    for base in [0u32, 0x8000_0000] {
        for b in [
            0,
            1,
            2,
            0x7f_ffff,
            0x80_0000,
            0x80_0001,
            0x3f80_0000,
            0x7f7f_ffff,
            0x7f80_0000,
            0x7f80_0001,
            0x7fbf_ffff,
            0x7fc0_0000,
            0x7fff_ffff,
        ] {
            round_trip_f32(base | b);
        }
    }
}

#[test]
#[ignore = "4 billion conversions; run with --release --ignored"]
fn g7_f32_round_trips_exhaustive() {
    for b in 0..=u32::MAX {
        round_trip_f32(b);
    }
}

fn round_trip_f64(b: u64) {
    let x = F80::from_f64_bits(b);
    let e = (b >> 52) & 0x7ff;
    let frac = b & ((1 << 52) - 1);
    let is_snan = e == 0x7ff && frac != 0 && frac & (1 << 51) == 0;
    for rc in RCS {
        let (back, fl) = x.to_f64_bits(m(rc, Pc::Extended));
        let tiny = if e == 0 && frac != 0 { TINY } else { NONE };
        if is_snan {
            assert_eq!((back, fl), (b | 1 << 51, IE), "{b:#018x}");
        } else {
            assert_eq!((back, fl), (b, tiny), "{b:#018x} {rc:?}");
        }
    }
}

#[test]
fn g7_f64_round_trips() {
    let mut s: u64 = 0x1234_5678_9abc_def1;
    for _ in 0..200_000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        round_trip_f64(s);
    }
    for base in [0u64, 1 << 63] {
        for b in [
            0u64,
            1,
            (1 << 52) - 1,
            1 << 52,
            0x3ff0_0000_0000_0000,
            0x7fef_ffff_ffff_ffff,
            0x7ff0_0000_0000_0000,
            0x7ff0_0000_0000_0001,
            0x7ff8_0000_0000_0000,
            0x7fff_ffff_ffff_ffff,
        ] {
            round_trip_f64(base | b);
        }
    }
}

#[test]
fn g7_narrowing_special_cases() {
    // 1 + 2^-24: tie at 24 bits -> 1.0 (even); RC=up -> 1 + 2^-23.
    let t = f(0x3fff, 0x8000_0080_0000_0000);
    assert_eq!(t.to_f32_bits(EXT), (0x3f80_0000, PE));
    assert_eq!(
        t.to_f32_bits(m(Rc::Up, Pc::Extended)),
        (0x3f80_0001, PE | C1)
    );
    // 1.5 * 2^-149 ties between denormals 1 and 2 -> 2 (even): tiny, so UE.
    assert_eq!(
        f(0x3fff - 149, 0xc000_0000_0000_0000).to_f32_bits(EXT),
        (2, UE | PE | C1 | TINY)
    );
    // 2^-150 ties between 0 and the smallest denormal -> +0.
    assert_eq!(f(0x3fff - 150, J).to_f32_bits(EXT), (0, UE | PE | TINY));
    // (1 - 2^-25) * 2^-126 rounds up to the smallest normal: tininess is
    // detected after rounding, so there is no UE.
    assert_eq!(
        f(0x3fff - 127, 0xffff_ff80_0000_0000).to_f32_bits(EXT),
        (0x0080_0000, PE | C1)
    );
    // An ext80 denormal is far below binary32: +0 or the smallest denormal.
    assert_eq!(MIN_DENORMAL.to_f32_bits(EXT), (0, UE | PE | TINY));
    assert_eq!(
        MIN_DENORMAL.to_f32_bits(m(Rc::Up, Pc::Extended)),
        (1, UE | PE | C1 | TINY)
    );
    assert_eq!(
        MIN_DENORMAL.neg().to_f64_bits(m(Rc::Down, Pc::Extended)),
        (1 << 63 | 1, UE | PE | C1 | TINY)
    );
    // Overflow: RN -> inf; chop -> largest finite.
    let big = f(0x3fff + 128, J);
    assert_eq!(big.to_f32_bits(EXT), (0x7f80_0000, OE | PE | C1));
    assert_eq!(
        big.to_f32_bits(m(Rc::Chop, Pc::Extended)),
        (0x7f7f_ffff, OE | PE)
    );
    assert_eq!(
        big.neg().to_f32_bits(m(Rc::Up, Pc::Extended)),
        (0xff7f_ffff, OE | PE)
    );
    assert_eq!(
        f(0x3fff + 1024, J).to_f64_bits(EXT),
        (0x7ff0_0000_0000_0000, OE | PE | C1)
    );
    // Largest float + half an ulp ties to even -> overflow.
    assert_eq!(
        f(0x3fff + 127, 0xffff_ff80_0000_0000).to_f32_bits(EXT),
        (0x7f80_0000, OE | PE | C1)
    );
    // NaN payload is truncated, quiet bit set; SNaN raises IE.
    assert_eq!(
        f(0xffff, 0xc000_0123_4567_89ab).to_f32_bits(EXT),
        (0xffc0_0001, NONE)
    );
    assert_eq!(SNAN.to_f64_bits(EXT), (0x7ff8_0000_0000_0000, IE));
}

/// Cross-check (not a source of expectations above): IEEE 754 binary64
/// arithmetic in host hardware is correctly rounded, so at PC=53/RN with
/// operands and results in the normal binary64 range, ext80 arithmetic
/// narrowed to double must agree bit for bit. Same for PC=24 and binary32.
#[test]
fn g7_cross_check_pc53_against_host_binary64() {
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let pc53 = m(Rc::Near, Pc::Double);
    let pc24 = m(Rc::Near, Pc::Single);
    let mut checked = 0;
    for _ in 0..100_000 {
        // Moderate exponents so nothing leaves the double normal range.
        let r = next();
        let mk = |r: u64| (r & 0x800f_ffff_ffff_ffff) | ((0x3ff - 200 + (r >> 52) % 400) << 52);
        let (a, b) = (f64::from_bits(mk(r)), f64::from_bits(mk(next())));
        let (xa, xb) = (
            F80::from_f64_bits(a.to_bits()),
            F80::from_f64_bits(b.to_bits()),
        );
        let host = [a + b, a - b, a * b, a / b, a.abs().sqrt()];
        let ours = [
            add(xa, xb, pc53).0,
            sub(xa, xb, pc53).0,
            mul(xa, xb, pc53).0,
            div(xa, xb, pc53).0,
            sqrt(xa.abs(), pc53).0,
        ];
        for (h, o) in host.iter().zip(ours) {
            assert_eq!(o.to_f64_bits(EXT), (h.to_bits(), NONE), "{a:e} {b:e}");
            checked += 1;
        }
        let mk32 =
            |r: u64| ((r as u32) & 0x807f_ffff) | (((127 - 50 + (r >> 40) % 100) as u32) << 23);
        let (fa, fb) = (f32::from_bits(mk32(next())), f32::from_bits(mk32(next())));
        let (ya, yb) = (
            F80::from_f32_bits(fa.to_bits()),
            F80::from_f32_bits(fb.to_bits()),
        );
        let host = [fa + fb, fa - fb, fa * fb, fa / fb, fa.abs().sqrt()];
        let ours = [
            add(ya, yb, pc24).0,
            sub(ya, yb, pc24).0,
            mul(ya, yb, pc24).0,
            div(ya, yb, pc24).0,
            sqrt(ya.abs(), pc24).0,
        ];
        for (h, o) in host.iter().zip(ours) {
            assert_eq!(o.to_f32_bits(EXT), (h.to_bits(), NONE), "{fa:e} {fb:e}");
            checked += 1;
        }
        // binary64 -> binary32 narrowing (through ext80) against the host's
        // correctly rounded conversion, including overflow and underflow.
        assert_eq!(xa.to_f32_bits(EXT).0, (a as f32).to_bits());
    }
    assert_eq!(checked, 1_000_000);
}

// ============================================================ group 8
// F80 <-> 10-byte memory format (SDM Vol. 1 Figure 4-3: 64-bit significand
// in bytes 0-7, sign and exponent in bytes 8-9, little-endian).

#[test]
fn g8_bytes_round_trip() {
    let pi = F80::PI;
    let b = pi.to_bytes();
    assert_eq!(
        b,
        [0x35, 0xc2, 0x68, 0x21, 0xa2, 0xda, 0x0f, 0xc9, 0x00, 0x40]
    );
    assert_eq!(F80::from_bytes(b), pi);
    assert_eq!(
        F80::INDEFINITE.to_bytes(),
        [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0xff]
    );
    let mut s: u64 = 7;
    for _ in 0..10_000 {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let x = f((s >> 48) as u16, s.rotate_left(17) ^ s);
        assert_eq!(F80::from_bytes(x.to_bytes()), x);
    }
}

#[test]
fn g8_constants() {
    // FLDPI etc. under RC=nearest: the full 64-bit significands
    // (each verified against a 1024-bit evaluation in consts.rs).
    assert_eq!(F80::PI, f(0x4000, 0xc90f_daa2_2168_c235));
    assert_eq!(F80::L2E, f(0x3fff, 0xb8aa_3b29_5c17_f0bc));
    assert_eq!(F80::L2T, f(0x4000, 0xd49a_784b_cd1b_8afe));
    assert_eq!(F80::LG2, f(0x3ffd, 0x9a20_9a84_fbcf_f799));
    assert_eq!(F80::LN2, f(0x3ffe, 0xb172_17f7_d1cf_79ac));
    // Rounded per RC: pi is just below 0x...c235, log2(10) just above
    // 0x...8afe.
    assert_eq!(
        F80::constant(Const::Pi, Rc::Down),
        f(0x4000, 0xc90f_daa2_2168_c234)
    );
    assert_eq!(F80::constant(Const::Pi, Rc::Up), F80::PI);
    assert_eq!(
        F80::constant(Const::L2T, Rc::Up),
        f(0x4000, 0xd49a_784b_cd1b_8aff)
    );
    assert_eq!(F80::constant(Const::L2T, Rc::Chop), F80::L2T);
    assert_eq!(F80::constant(Const::One, Rc::Down), ONE);
}

// The transcendentals are not proven correctly rounded (FLOAT-ABI.md §6).
const _: () = assert!(!TRANSCENDENTAL_IS_CORRECTLY_ROUNDED);
