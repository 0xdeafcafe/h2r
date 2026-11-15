//! Guarded, integer-only evaluation. See docs/x87-transcendentals.md for
//! domains, error budgets and the rounding/flag acceptance test.
#![forbid(unsafe_code)]

use super::*;

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
    let (lg, exact) = log2_wf::<2>(wf(fx));
    if exact {
        return None;
    }
    certify(wf::<2>(fy).mul(lg))
}

pub(super) fn x2m1(a: F80) -> Option<(F80, Flags)> {
    let f = operand(a)?;
    if !(-1138..=-1).contains(&f.top()) {
        return None;
    }
    certify(expm1_series(wf::<2>(f).mul(ln2::<2>())).0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trans::tests::Rng;

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

    #[test]
    fn unrounded_errors_stay_within_the_documented_budgets() {
        fn check(fast: Fast, wide: Wf<12>, bits: i32) {
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
                wf::<2>(y).mul(log2_wf::<2>(wf(x)).0),
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
                    for op in [YL2X, X2M1] {
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
        let inputs: Vec<_> = (0..256)
            .map(|_| (rng.f80(-3, 3), rng.f80(-3, -1)))
            .collect();
        for op in 0..2 {
            let mut results = [Vec::new(), Vec::new()];
            // Paired, alternating order; discard five warmup pairs.
            for batch in 0..36 {
                for implementation in [batch % 2, 1 - batch % 2] {
                    let fast = implementation == 1;
                    let start = Instant::now();
                    for &(y, x) in &inputs {
                        let (y, x) = black_box((y, x));
                        let value = match (op, fast) {
                            (0, true) => {
                                yl2x(y, x.abs()).unwrap_or_else(|| yl2x_n::<8>(y, x.abs()))
                            }
                            (0, false) => yl2x_n::<8>(y, x.abs()),
                            (1, true) => x2m1(x).unwrap_or_else(|| x2m1_n::<8>(x)),
                            (1, false) => x2m1_n::<8>(x),
                            _ => unreachable!(),
                        };
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
