//! Hand-checked cases. Wider coverage: tests/differential.rs (Rust vs C) and
//! tests/hardware.rs (vs recorded SSE results).

extern crate std;

use super::*;

const ONE: u32 = 0x3F80_0000;
const TWO: u32 = 0x4000_0000;
const INF: u32 = 0x7F80_0000;
const QNAN: u32 = 0x7FC0_0000;
const SNAN: u32 = 0x7FA0_0000;
const MIN_NORMAL: u32 = 0x0080_0000;
const RN: u32 = MXCSR_DEFAULT;
const RD: u32 = MXCSR_DEFAULT | 1 << RC_SHIFT;
const RU: u32 = MXCSR_DEFAULT | 2 << RC_SHIFT;
const RZ: u32 = MXCSR_DEFAULT | 3 << RC_SHIFT;

fn ss(
    f: fn(&mut u32, u128, u32) -> Result<u128, Xm>,
    mx: u32,
    a: u32,
    b: u32,
) -> (Result<u32, Xm>, u32) {
    let mut m = mx;
    let hi = 0x1111_1111_2222_2222_3333_3333_0000_0000u128;
    let r = f(&mut m, hi | a as u128, b).map(|v| {
        assert_eq!(v & !0xFFFF_FFFF, hi, "lanes 1-3 kept");
        v as u32
    });
    (r, m)
}

#[test]
fn basic_arithmetic() {
    assert_eq!(ss(addss, RN, ONE, ONE), (Ok(TWO), RN));
    assert_eq!(ss(subss, RN, ONE, ONE), (Ok(0), RN));
    assert_eq!(ss(subss, RD, ONE, ONE), (Ok(0x8000_0000), RD));
    assert_eq!(ss(mulss, RN, TWO, TWO), (Ok(0x4080_0000), RN));
    assert_eq!(ss(divss, RN, ONE, 0x4040_0000), (Ok(0x3EAA_AAAB), RN | PE));
    assert_eq!(ss(divss, RZ, ONE, 0x4040_0000), (Ok(0x3EAA_AAAA), RZ | PE));
    assert_eq!(ss(sqrtss, RN, 0, TWO), (Ok(0x3FB5_04F3), RN | PE));
    assert_eq!(ss(sqrtss, RN, 0, 0x4080_0000), (Ok(TWO), RN));
}

#[test]
fn rounding_modes_on_a_tie_and_beyond() {
    // 1 + 2^-24 is a tie: nearest-even keeps 1, up goes to 1 + 2^-23.
    let half_ulp = 0x3380_0000;
    assert_eq!(ss(addss, RN, ONE, half_ulp).0, Ok(ONE));
    assert_eq!(ss(addss, RU, ONE, half_ulp).0, Ok(ONE + 1));
    assert_eq!(ss(addss, RD, ONE, half_ulp).0, Ok(ONE));
    assert_eq!(ss(addss, RZ, ONE, half_ulp).0, Ok(ONE));
    assert_eq!(
        ss(addss, RD, ONE | 0x8000_0000, half_ulp | 0x8000_0000).0,
        Ok((ONE + 1) | 0x8000_0000)
    );
    // 1 + 3 * 2^-24 ties to even upward.
    assert_eq!(ss(addss, RN, ONE + 1, half_ulp).0, Ok(ONE + 2));
}

#[test]
fn nan_rules() {
    assert_eq!(ss(addss, RN, QNAN | 1, SNAN | 2), (Ok(QNAN | 1), RN | IE));
    assert_eq!(
        ss(addss, RN, SNAN | 1, QNAN | 2),
        (Ok(SNAN | QUIET | 1), RN | IE)
    );
    assert_eq!(
        ss(mulss, RN, ONE, SNAN | 5),
        (Ok(SNAN | QUIET | 5), RN | IE)
    );
    assert_eq!(ss(subss, RN, INF, INF), (Ok(DEFAULT_NAN), RN | IE));
    assert_eq!(ss(mulss, RN, 0, INF), (Ok(DEFAULT_NAN), RN | IE));
    assert_eq!(ss(divss, RN, 0, 0), (Ok(DEFAULT_NAN), RN | IE));
    assert_eq!(ss(sqrtss, RN, 0, 0xBF80_0000), (Ok(DEFAULT_NAN), RN | IE));
    assert_eq!(ss(sqrtss, RN, 0, 0x8000_0000), (Ok(0x8000_0000), RN));
    // The QNaN operand outranks the denormal operand.
    assert_eq!(ss(addss, RN, QNAN, 1), (Ok(QNAN), RN));
}

#[test]
fn min_max_return_the_second_operand() {
    assert_eq!(ss(minss, RN, QNAN, ONE), (Ok(ONE), RN | IE));
    assert_eq!(ss(minss, RN, ONE, SNAN), (Ok(SNAN), RN | IE));
    assert_eq!(ss(maxss, RN, 0, 0x8000_0000), (Ok(0x8000_0000), RN));
    assert_eq!(ss(minss, RN, 0x8000_0000, 0), (Ok(0), RN));
    assert_eq!(ss(maxss, RN, TWO, ONE), (Ok(TWO), RN));
    assert_eq!(ss(minss, RN, 1, ONE), (Ok(1), RN | DE));
}

#[test]
fn denormals_ftz_and_tininess() {
    // 2^-126 * 0.5 is an exact denormal: no flags, unless FTZ flushes it.
    assert_eq!(
        ss(mulss, RN, MIN_NORMAL, 0x3F00_0000),
        (Ok(0x0040_0000), RN)
    );
    assert_eq!(
        ss(mulss, RN | FTZ, MIN_NORMAL, 0x3F00_0000),
        (Ok(0), RN | FTZ | UE | PE)
    );
    // Exact-tiny with UE unmasked still raises #XM.
    let um = RN & !(UE << MASK_SHIFT);
    assert_eq!(ss(mulss, um, MIN_NORMAL, 0x3F00_0000), (Err(Xm), um | UE));
    // 2^-126 * (1 + 2^-23) * (1 - 2^-23) rounds to 2^-126 at 24 bits: not
    // tiny after rounding, so only PE.
    assert_eq!(
        ss(mulss, RN, MIN_NORMAL + 1, 0x3F7F_FFFE),
        (Ok(MIN_NORMAL), RN | PE)
    );
    assert_eq!(
        ss(mulss, RZ, MIN_NORMAL + 1, 0x3F7F_FFFE),
        (Ok(0x007F_FFFF), RZ | UE | PE)
    );
    // A denormal operand raises DE.
    assert_eq!(ss(addss, RN, 1, 1), (Ok(2), RN | DE));
}

#[test]
fn overflow() {
    let max = 0x7F7F_FFFF;
    assert_eq!(ss(mulss, RN, max, TWO), (Ok(INF), RN | OE | PE));
    assert_eq!(ss(mulss, RZ, max, TWO), (Ok(max), RZ | OE | PE));
    assert_eq!(ss(mulss, RD, max, TWO), (Ok(max), RD | OE | PE));
    assert_eq!(
        ss(mulss, RU, max | 0x8000_0000, TWO),
        (Ok(max | 0x8000_0000), RU | OE | PE)
    );
}

#[test]
fn divide_by_zero_outranks_denormal() {
    assert_eq!(ss(divss, RN, 1, 0), (Ok(INF), RN | ZE));
    assert_eq!(
        ss(divss, RN, 0xBF80_0000, 0),
        (Ok(INF | 0x8000_0000), RN | ZE)
    );
}

#[test]
fn unmasked_exceptions() {
    let zm = RN & !(ZE << MASK_SHIFT);
    let mut m = zm;
    assert_eq!(divps(&mut m, ONE as u128, 0), Err(Xm));
    // Lane 0 divides by zero; lanes 1-3 are 0/0: IE (masked) and ZE.
    assert_eq!(m, zm | ZE | IE);
    // An unmasked pre-computation exception drops post-computation flags.
    let mut m = zm;
    let a = (ONE as u128) << 32 | ONE as u128;
    let b = (0x4040_0000u128) << 32;
    assert_eq!(
        divps(&mut m, a, b | 0x3F80_0000_0000_0000_0000_0000_0000_0000),
        Err(Xm)
    );
    assert_eq!(m & 0x3F, ZE | IE);
    // Unmasked PE: flags recorded, #XM.
    let pm = RN & !(PE << MASK_SHIFT);
    let mut m = pm;
    assert_eq!(divss(&mut m, ONE as u128, 0x4040_0000), Err(Xm));
    assert_eq!(m, pm | PE);
}

#[test]
fn compares() {
    let mut m = RN;
    assert_eq!(comiss(&mut m, ONE, TWO), Ok(0x01));
    assert_eq!(comiss(&mut m, TWO, ONE), Ok(0));
    assert_eq!(comiss(&mut m, 0, 0x8000_0000), Ok(0x40));
    assert_eq!(m, RN);
    assert_eq!(ucomiss(&mut m, QNAN, ONE), Ok(0x45));
    assert_eq!(m, RN);
    assert_eq!(comiss(&mut m, QNAN, ONE), Ok(0x45));
    assert_eq!(m, RN | IE);
    let im = RN & !(IE << MASK_SHIFT);
    let mut m = im;
    assert_eq!(ucomiss(&mut m, SNAN, ONE), Err(Xm));
    for (pred, want, ie) in [
        (0, 0, 0),
        (1, 0, IE),
        (2, 0, IE),
        (3, !0, 0),
        (4, !0, 0),
        (5, !0, IE),
        (6, !0, IE),
        (7, 0, 0),
    ] {
        let (r, m) = cmpss_lane(pred, QNAN, ONE);
        assert_eq!((r, m), (Ok(want), RN | ie), "pred {pred}");
    }
    assert_eq!(cmpss_lane(2, ONE, ONE), (Ok(!0), RN));
    assert_eq!(cmpss_lane(0, 0x8000_0000, 0), (Ok(!0), RN));
    assert_eq!(
        cmpss_lane(8 | 1, ONE, TWO),
        (Ok(!0), RN),
        "imm8 bits 3-7 ignored"
    );
}

fn cmpss_lane(pred: u8, a: u32, b: u32) -> (Result<u32, Xm>, u32) {
    let mut m = RN;
    (cmpss(&mut m, a as u128, b, pred).map(|v| v as u32), m)
}

#[test]
fn conversions() {
    let mut m = RN;
    assert_eq!(cvtss2si(&mut m, 0x3FC0_0000), Ok(2)); // 1.5 -> 2
    assert_eq!(cvtss2si(&mut m, 0x4020_0000), Ok(2)); // 2.5 -> 2
    assert_eq!(m, RN | PE);
    let mut m = RN;
    assert_eq!(cvttss2si(&mut m, 0xBFF0_0000), Ok(-1i32 as u32)); // -1.875 -> -1
    assert_eq!(cvtss2si(&mut m, 0xCF00_0000), Ok(0x8000_0000)); // -2^31 exactly
    assert_eq!(m, RN | PE);
    let mut m = RN;
    assert_eq!(cvtss2si(&mut m, 0x4F00_0000), Ok(INT_INDEFINITE));
    assert_eq!(m, RN | IE);
    let mut m = RN;
    assert_eq!(cvtss2si(&mut m, QNAN), Ok(INT_INDEFINITE));
    assert_eq!(m, RN | IE);
    let mut m = RU;
    assert_eq!(
        cvtss2si(&mut m, 1),
        Ok(1),
        "a denormal rounds up to 1, PE but no DE"
    );
    assert_eq!(m, RU | PE);
    let mut m = RN;
    assert_eq!(cvtsi2ss(&mut m, 0, 0x0100_0001), Ok(0x4B80_0000));
    assert_eq!(m, RN | PE);
    let mut m = RN;
    assert_eq!(
        cvtpi2ps(&mut m, 0xAAAA_AAAA_BBBB_BBBB << 64, 3 << 32 | 0xFFFF_FFFF),
        Ok(0xAAAA_AAAA_BBBB_BBBB_4040_0000_BF80_0000)
    );
    let mut m = RN;
    assert_eq!(
        cvtps2pi(&mut m, (0x3F00_0000u64 << 32) | 0x4F00_0000),
        Ok(0x8000_0000)
    );
    assert_eq!(m, RN | IE | PE);
}

#[test]
fn approximations() {
    assert_eq!(rcp_approx(ONE), 0x3F7F_F000);
    assert_eq!(rcp_approx(0x3FFF_FFFF), 0x3F00_0800);
    assert_eq!(rcp_approx(0), INF);
    assert_eq!(rcp_approx(0x8000_0001), INF | 0x8000_0000);
    assert_eq!(rcp_approx(INF | 0x8000_0000), 0x8000_0000);
    assert_eq!(rcp_approx(0x7E80_0000), 0, "2^126 flushes to 0");
    assert_eq!(rcp_approx(SNAN), QNAN | 0x0020_0000);
    assert_eq!(rsqrt_approx(ONE), 0x3F7F_F000);
    assert_eq!(rsqrt_approx(TWO), 0x3F34_F800);
    assert_eq!(rsqrt_approx(0x4080_0000), 0x3EFF_F000);
    assert_eq!(rsqrt_approx(0x8000_0000), INF | 0x8000_0000);
    assert_eq!(rsqrt_approx(0xBF80_0000), DEFAULT_NAN);
    assert_eq!(rsqrt_approx(INF), 0);
    assert_eq!(rsqrt_approx(INF | 0x8000_0000), DEFAULT_NAN);
    // Relative error within the SDM's 1.5 * 2^-12 over a few binades.
    for x in (0x3F80_0000u32..0x4080_0000).step_by(97) {
        let v = f32::from_bits(x) as f64;
        let r = f32::from_bits(rcp_approx(x)) as f64;
        let s = f32::from_bits(rsqrt_approx(x)) as f64;
        assert!((r * v - 1.0).abs() <= 1.5 / 4096.0, "rcp {x:#x}");
        assert!((s * v.sqrt() - 1.0).abs() <= 1.5 / 4096.0, "rsqrt {x:#x}");
    }
}

/// Hand-derived, not differential: the flags of unmasked overflow and
/// underflow. PE reflects only rounding to 24 bits with an unbounded
/// exponent, taken before the trap (Intel's FP reference flowchart).
#[test]
fn unmasked_overflow_and_underflow_flags() {
    let om = 0x1B80; // OE unmasked
    let um = 0x1780; // UE unmasked
    let run = |mx: u32, a: u32, b: u32| {
        let mut m = mx;
        let r = mulss(&mut m, a as u128, b).map(|v| v as u32);
        (r, m)
    };
    // 2^127 * 2 = 2^128 exactly: OE alone.
    assert_eq!(run(om, 0x7F00_0000, TWO), (Err(Xm), 0x1B88));
    // max * 1.5 needs 26 significant bits: OE | PE.
    assert_eq!(run(om, 0x7F7F_FFFF, 0x3FC0_0000), (Err(Xm), 0x1BA8));
    // Masked, the same overflows report OE | PE and complete.
    assert_eq!(run(0x1F80, 0x7F00_0000, TWO), (Ok(INF), 0x1FA8));
    // 2^-126 * 2^-24 = 2^-150 exactly: UE alone.
    assert_eq!(run(um, MIN_NORMAL, 0x3380_0000), (Err(Xm), 0x1790));
    // (1 + 2^-23) * 2^-127 fits 24 bits, though denormalising it would not:
    // still UE alone.
    assert_eq!(run(um, MIN_NORMAL + 1, 0x3F00_0000), (Err(Xm), 0x1790));
    // A product inexact at 24 bits: UE | PE.
    assert_eq!(run(um, MIN_NORMAL + 1, 0x3EAA_AAAB), (Err(Xm), 0x17B0));
    // FTZ does nothing while UE is unmasked.
    assert_eq!(run(um | FTZ, MIN_NORMAL, 0x3380_0000), (Err(Xm), 0x9790));
    assert_eq!(
        run(um | FTZ, MIN_NORMAL + 1, 0x3EAA_AAAB),
        (Err(Xm), 0x97B0)
    );
    // Masked: 2^-150 rounds to the denormal 0 (tie to even) inexactly.
    assert_eq!(run(0x1F80, MIN_NORMAL, 0x3380_0000), (Ok(0), 0x1FB0));
    assert_eq!(run(0x9F80, MIN_NORMAL, 0x3380_0000), (Ok(0), 0x9FB0));
    // A masked exact tiny result under FTZ still flushes with UE | PE.
    assert_eq!(run(0x9F80, MIN_NORMAL, 0x3F00_0000), (Ok(0), 0x9FB0));
}

/// Hand-derived packed cases with mixed lanes.
#[test]
fn packed_mixed_lanes() {
    let v = |l: [u32; 4]| {
        l.iter()
            .enumerate()
            .fold(0u128, |a, (i, &x)| a | (x as u128) << (32 * i))
    };
    // Lanes: exact overflow, inexact normal, exact normal, exact denormal.
    let a = v([0x7F00_0000, 0x3EAA_AAAB, ONE, MIN_NORMAL]);
    let b = v([TWO, 0x3EAA_AAAB, ONE, 0x3F00_0000]);
    let mut m = 0x1F80;
    assert_eq!(
        mulps(&mut m, a, b),
        Ok(v([INF, 0x3DE3_8E3A, ONE, 0x0040_0000]))
    );
    assert_eq!(m, 0x1FA8, "OE|PE from lane 0, PE from lane 1");
    // OE unmasked: #XM; PE is recorded because lane 1 is genuinely inexact.
    let mut m = 0x1B80;
    assert_eq!(mulps(&mut m, a, b), Err(Xm));
    assert_eq!(m, 0x1BA8);
    // Without the inexact lane, the exact overflow records OE alone.
    let a2 = v([0x7F00_0000, ONE, ONE, MIN_NORMAL]);
    let b2 = v([TWO, ONE, ONE, 0x3F00_0000]);
    let mut m = 0x1B80;
    assert_eq!(mulps(&mut m, a2, b2), Err(Xm));
    assert_eq!(m, 0x1B88);
    // UE unmasked: the exact denormal lane traps with UE alone; FTZ on too.
    for mx in [0x1780, 0x9780] {
        let mut m = mx;
        assert_eq!(
            mulps(
                &mut m,
                v([ONE, ONE, ONE, MIN_NORMAL]),
                v([ONE, ONE, ONE, 0x3F00_0000])
            ),
            Err(Xm)
        );
        assert_eq!(m, mx | UE);
    }
    // FTZ with UE masked flushes that lane instead.
    let mut m = 0x9F80;
    assert_eq!(
        mulps(
            &mut m,
            v([ONE, ONE, ONE, MIN_NORMAL]),
            v([ONE, ONE, ONE, 0x3F00_0000])
        ),
        Ok(v([ONE, ONE, ONE, 0]))
    );
    assert_eq!(m, 0x9FB0);
    // An unmasked ZE in one lane suppresses another lane's OE and PE.
    let mut m = 0x1D80;
    assert_eq!(
        divps(
            &mut m,
            v([ONE, 0x7F7F_FFFF, ONE, ONE]),
            v([0, 0x3400_0000, 0x4040_0000, ONE])
        ),
        Err(Xm)
    );
    assert_eq!(m, 0x1D84);
}
