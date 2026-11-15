//! Guarded, integer-only evaluation. See docs/x87-transcendentals.md for
//! domains, error budgets and the rounding/flag acceptance test.
#![forbid(unsafe_code)]

use super::*;

type Fast = Wf<4>;

fn operand(a: F80) -> Option<Fin> {
    match unpack(a) {
        Unp::Fin(f) if !f.denorm && (-32..=32).contains(&f.top()) => Some(f),
        _ => None,
    }
}

/// Accept only an interval entirely inside one rounding cell AND on one
/// side of its representable result. The latter preserves PE and C1.
/// This radius exceeds the per-function error bounds by at least 16 bits;
/// endpoint arithmetic's truncation is smaller than that margin too.
fn certify(v: Fast) -> Option<(F80, Flags)> {
    if v.is_zero() {
        return None;
    }
    let radius = Fast::one().scale(v.top() - 160);
    let lo = v.sub(radius).to_f80(0);
    let hi = v.add(radius).to_f80(0);
    (lo == hi && lo.1.contains(Flags::PE)).then_some(lo)
}

pub(super) fn yl2x(y: F80, x: F80) -> Option<(F80, Flags)> {
    let (fy, fx) = (operand(y)?, operand(x)?);
    if fx.neg {
        return None;
    }
    let (lg, exact) = log2_wf::<4>(wf(fx));
    if exact {
        return None;
    }
    certify(wf::<4>(fy).mul(lg))
}

pub(super) fn x2m1(a: F80) -> Option<(F80, Flags)> {
    let f = operand(a)?;
    if f.top() >= 0 {
        return None;
    }
    certify(expm1_series(wf::<4>(f).mul(ln2::<4>())).0)
}

pub(super) fn patan(y: F80, x: F80) -> Option<(F80, Flags)> {
    let (fy, fx) = (operand(y)?, operand(x)?);
    certify(patan_finite::<4>(fy, fx).0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trans::tests::Rng;

    #[test]
    fn accepted_results_and_flags_match_512_and_768_bits() {
        let mut rng = Rng(0x8de3_f0a1_7c24_b695);
        let mut hits = [0; 3];
        for _ in 0..4000 {
            let y = rng.f80(-32, 32);
            let x = rng.f80(-32, 32).abs();
            let a = rng.f80(-32, -1);
            let cases = [
                (yl2x(y, x), yl2x_n::<8>(y, x), yl2x_n::<12>(y, x)),
                (x2m1(a), x2m1_n::<8>(a), x2m1_n::<12>(a)),
                (patan(y, x), patan_n::<8>(y, x), patan_n::<12>(y, x)),
            ];
            for (i, (fast, reference, wider)) in cases.into_iter().enumerate() {
                assert_eq!(reference, wider);
                if let Some(fast) = fast {
                    assert_eq!(fast, reference, "function {i}, y={y:x?} x={x:x?} a={a:x?}");
                    hits[i] += 1;
                }
            }
            // Exercise all four atan quadrants, preserving both signs.
            for (y, x) in [(y.neg(), x), (y, x.neg()), (y.neg(), x.neg())] {
                if let Some(fast) = patan(y, x) {
                    assert_eq!(fast, patan_n::<8>(y, x));
                }
            }
        }
        assert!(hits.iter().all(|&n| n > 3900), "{hits:?}");
    }

    #[test]
    fn unrounded_errors_stay_within_the_documented_budgets() {
        fn check(fast: Fast, wide: Wf<12>, bits: i32) {
            let mut m = [0; 12];
            m[8..].copy_from_slice(&fast.m);
            let lifted = Wf::<12> {
                neg: fast.neg,
                exp: fast.exp - 512,
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
            let y = operand(rng.f80(-32, 32)).unwrap();
            let x = if i < 4 {
                // Both nearest neighbours of one, with both signs of y.
                operand(F80::new(
                    if i & 1 == 0 { 16383 } else { 16382 },
                    if i & 1 == 0 { (1 << 63) + 1 } else { u64::MAX },
                ))
                .unwrap()
            } else {
                operand(rng.f80(-32, 32).abs()).unwrap()
            };
            check(
                wf::<4>(y).mul(log2_wf::<4>(wf(x)).0),
                wf::<12>(y).mul(log2_wf::<12>(wf(x)).0),
                208,
            );
            let a = operand(rng.f80(-32, -1)).unwrap();
            check(
                expm1_series(wf::<4>(a).mul(ln2::<4>())).0,
                expm1_series(wf::<12>(a).mul(ln2::<12>())).0,
                224,
            );
            check(patan_finite::<4>(y, x).0, patan_finite::<12>(y, x).0, 208);
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
            F80::new(16383 - 33, 1 << 63),
            F80::new(16383 + 33, 1 << 63),
        ] {
            assert!(operand(a).is_none(), "{a:x?}");
            assert_eq!(patan(a, F80::ONE), None);
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
                let mut v = center.add(Fast::one().scale(-162));
                v.neg = sign;
                assert_eq!(certify(v), None);
                let mut v = center.add(Fast::one().scale(-150));
                v.neg = sign;
                assert!(certify(v).is_some());
            }
        }
    }

    #[test]
    fn public_dispatch_matches_reference_at_domain_edges() {
        for e in [-33, -32, -1, 0, 32, 33] {
            for sig in [1 << 63, (1 << 63) + 1, u64::MAX] {
                for sign in [0, 0x8000] {
                    let a = F80::new(sign | (16383 + e) as u16, sig);
                    for b in [F80::ONE, F80::PI, F80::PI.neg()] {
                        assert_eq!(crate::patan(a, b), patan_n::<8>(a, b));
                        assert_eq!(crate::patan(b, a), patan_n::<8>(b, a));
                        assert_eq!(crate::yl2x(b, a), yl2x_n::<8>(b, a));
                    }
                    assert_eq!(crate::x2m1(a), x2m1_n::<8>(a));
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
                        rng.f80(-32, -1)
                    };
                    let b = rng.f80(-32, 32);
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
                    for op in [YL2X, X2M1, PATAN] {
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
        for op in 0..3 {
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
                            (2, true) => patan(y, x).unwrap_or_else(|| patan_n::<8>(y, x)),
                            _ => patan_n::<8>(y, x),
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
