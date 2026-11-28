use super::{checked_env, checked_guest_env, checked_inputs};

#[test]
fn the_guest_env_takes_pe_clear() {
    assert!(checked_guest_env(0x1f80) && checked_guest_env(0x1fa0));
    for unsupported in [0x9f80, 0x1fc0, 0x3f80, 0x1f00] {
        assert!(!checked_guest_env(unsupported), "mxcsr {unsupported:#x}");
    }
}

#[test]
fn guest_guards_inspect_bits() {
    assert!(checked_env(0x1fa0));
    for unsupported in [0x1f80, 0x1fe0, 0x9fa0, 0x3fa0, 0x1f20] {
        assert!(!checked_env(unsupported), "mxcsr {unsupported:#x}");
    }
    assert!(checked_inputs(&[
        0,
        0x8000_0000,
        f32::MIN_POSITIVE.to_bits(),
        f32::MAX.to_bits()
    ]));
    for declined in [
        1,
        0x807f_ffff,
        0x7f80_0000,
        0xff80_0000,
        0x7f80_0001,
        0xffc0_1234,
    ] {
        assert!(!checked_inputs(&[0, declined]));
    }
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
mod native {
    use super::super::{checked, host};
    use crate::det::Chain;
    use std::cmp::Ordering;
    use x87::abi as fp;

    #[test]
    fn sse_inexact_is_set_by_the_first_inexact_sse_operation() {
        let seen = checked(|r| {
            let exact = r.add(1.0, 2.0);
            let before = r.sse_inexact();
            let third = r.div(1.0, 3.0);
            let _ = r.mul(third, 3.0);
            Some((exact, before, r.sse_inexact()))
        });
        assert_eq!(seen, Some((3.0, false, true)));
        assert_eq!(checked(|r| Some(r.sse_inexact())), Some(false), "each region starts clear");
    }

    #[test]
    fn each_hardware_exception_discards_an_otherwise_good_result() {
        // Use the underlying operations, bypassing the boundary-value
        // check: the status sample itself must detect the exceptional work.
        // All results are dead before the closure returns.
        for (dividend, divisor) in [
            (0.0, 0.0),               // invalid
            (1.0, 0.0),               // divide by zero
            (f32::MAX, 0.5),          // overflow
            (f32::MIN_POSITIVE, 3.0), // underflow
        ] {
            let result = checked(|_| {
                host::div32(dividend, divisor);
                Some(19)
            });
            assert_eq!(result, None);
        }
        assert_eq!(checked(|r| Some(r.div(1.0, 3.0))), Some(1.0 / 3.0));
    }

    #[test]
    fn status_is_scoped_across_declines_nested_calls_and_unwind() {
        struct Reset(host::Word);
        impl Drop for Reset {
            fn drop(&mut self) {
                host::write(self.0);
            }
        }
        let _reset = Reset(host::read());
        // Set the incoming exceptions. Clean work in a region still succeeds.
        let incoming = host::read() | host::REJECT;
        host::write(incoming);
        assert_eq!(
            checked(|r| Some(r.mul(-0.0, 2.0).to_bits())),
            Some((-0.0f32).to_bits())
        );
        assert_eq!(host::read(), incoming);
        assert_eq!(
            checked(|_| {
                host::div32(1.0, 0.0);
                assert_eq!(checked::<()>(|_| panic!("nested kernel ran")), None);
                Some(0)
            }),
            None
        );
        assert_eq!(host::read(), incoming);
        let panic = std::panic::catch_unwind(|| checked::<()>(|_| panic!("unwind")));
        assert!(panic.is_err());
        assert_eq!(host::read(), incoming);
    }

    #[test]
    fn exact_subnormals_and_tininess_boundary_decline_on_both_hosts() {
        for factor in [0.5, 1.0] {
            assert_eq!(checked(|r| Some(r.mul(f32::MIN_POSITIVE, factor))), None);
        }
        let denormal = f32::from_bits(1);
        assert_eq!(checked(|r| Some(r.add(1.0, denormal))), None);
        assert_eq!(
            checked(|r| Some(Chain::load(r, 0x027f, denormal)?.store())),
            None
        );
        let above = f32::from_bits(f32::MIN_POSITIVE.to_bits() + 1);
        assert_eq!(checked(|r| Some(r.mul(above, 1.0))), Some(above));
    }

    // FLD, FADD, FSUB, FMUL, FDIV, FCHS. Reading each store without
    // popping also exposes the value held *before* the guest binary32 store.
    fn oracle(control: u16, input: [f32; 5]) -> (f64, u32, i32) {
        let [a, b, c, d, e] = input;
        let mut state = fp::X87Abi {
            control,
            tag: 0xffff,
            ..Default::default()
        };
        fp::ld_f32(&mut state, a.to_bits());
        for (op, operand) in [
            (fp::arith_op::ADD, b),
            (fp::arith_op::SUB, c),
            (fp::arith_op::MUL, d),
            (fp::arith_op::DIV, e),
        ] {
            fp::arith(
                &mut state,
                op,
                0,
                fp::src_kind::F32,
                operand.to_bits().into(),
            );
        }
        fp::unary(&mut state, fp::unary_op::CHS);
        (
            f64::from_bits(fp::st_f64(&state, 0).1.value),
            fp::st_f32(&state, 0).1.value,
            fp::st_i32(&state, 0).1.value,
        )
    }

    #[test]
    fn single_and_double_chains_match_x87_before_and_after_store() {
        let mut seed = 7u32;
        let mut operand = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let exponent = 125 + ((seed >> 24) % 5);
            f32::from_bits((seed & 0x807f_ffff) | (exponent << 23))
        };
        let edges = [
            [1.0, 2.0f32.powi(-24), 1.0, 1.0, 1.0],
            [0.0, -0.0, 0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0, 1.0, 3.0],
            [0.0, 5.0, 0.0, 1.0, 2.0],
            [7.0, 0.0, 0.0, 1.0, 2.0],
        ];
        for input in edges
            .into_iter()
            .chain((0..1024).map(|_| std::array::from_fn(|_| operand())))
        {
            for control in [0x007f, 0x027f] {
                let (wide, store, integer) = oracle(control, input);
                let actual = checked(|r| {
                    let load = |x| Chain::load(r, control, x);
                    let [a, b, c, d, e] = input;
                    let result = -(((load(a)? + load(b)? - load(c)?) * load(d)?) / load(e)?);
                    let stored = result.store();
                    Some((
                        stored.to_bits(),
                        result.integer()?,
                        result.compare(load(stored)?)?,
                    ))
                });
                assert_eq!(
                    actual,
                    Some((
                        store,
                        integer,
                        wide.partial_cmp(&f64::from(f32::from_bits(store))).unwrap()
                    )),
                    "control {control:#x}, inputs {input:?}"
                );
            }
        }
    }

    /// The chain's x87 PE after the arithmetic, after `fst m32` and after
    /// `fistp`, each sticky, against the x87 model's status word.
    #[test]
    fn x87_inexact_follows_the_status_word_and_not_sse() {
        let mut seed = 11u32;
        let mut operand = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let exponent = 125 + ((seed >> 24) % 5);
            f32::from_bits((seed & 0x807f_ffff) | (exponent << 23))
        };
        let edges = [[1.0, 2.0, 0.0, 1.0, 1.0], [0.5, 0.25, 0.0, 4.0, 1.0], [1.0, 2.0, 0.0, 1.0, 3.0]];
        for input in edges.into_iter().chain((0..512).map(|_| std::array::from_fn(|_| operand()))) {
            for control in [0x007f, 0x027f] {
                let [a, b, c, d, e] = input;
                let mut state = fp::X87Abi { control, tag: 0xffff, ..Default::default() };
                fp::ld_f32(&mut state, a.to_bits());
                for (op, operand) in [
                    (fp::arith_op::ADD, b),
                    (fp::arith_op::SUB, c),
                    (fp::arith_op::MUL, d),
                    (fp::arith_op::DIV, e),
                ] {
                    fp::arith(&mut state, op, 0, fp::src_kind::F32, operand.to_bits().into());
                }
                let pe = |status: u16| status & 0x20 != 0;
                let arith = pe(state.status);
                let stored = fp::st_f32(&state, 0).1;
                let integer = fp::st_i32(&state, 0).1;
                let expected = (arith, arith || pe(stored.status), arith || pe(integer.status));
                let actual = checked(|r| {
                    let load = |x| Chain::load(r, control, x);
                    let result = ((load(a)? + load(b)? - load(c)?) * load(d)?) / load(e)?;
                    let after_arith = r.x87_inexact();
                    let _ = result.store();
                    let after_store = r.x87_inexact();
                    // The integer store's PE taken on its own.
                    Some((after_arith, after_store, result.integer()?, r.sse_inexact()))
                });
                let Some((after_arith, after_store, _, sse)) = actual else { continue };
                assert_eq!((after_arith, after_store), (expected.0, expected.1), "control {control:#x}, {input:?}");
                assert!(!sse, "x87 work never sets the SSE flag");
                let integer_pe = checked(|r| {
                    let result = ((Chain::load(r, control, a)? + Chain::load(r, control, b)? - Chain::load(r, control, c)?)
                        * Chain::load(r, control, d)?)
                        / Chain::load(r, control, e)?;
                    let _ = result.integer()?;
                    Some(r.x87_inexact())
                });
                assert_eq!(integer_pe, Some(expected.2), "control {control:#x}, {input:?}");
            }
        }
    }

    #[test]
    fn square_root_rounds_at_the_x87_working_precision() {
        for control in [0x007f, 0x027f] {
            for value in [-0.0f32, 0.0, 2.0, 3.0, 4.0, f32::MAX] {
                let mut state = fp::X87Abi {
                    control,
                    tag: 0xffff,
                    ..Default::default()
                };
                fp::ld_f32(&mut state, value.to_bits());
                fp::unary(&mut state, fp::unary_op::SQRT);
                let wide = f64::from_bits(fp::st_f64(&state, 0).1.value);
                let single = fp::st_f32(&state, 0).1.value;
                assert_eq!(
                    checked(|r| {
                        let root = Chain::load(r, control, value)?.sqrt();
                        let stored = root.store();
                        Some((
                            stored.to_bits(),
                            root.compare(Chain::load(r, control, stored)?)?,
                        ))
                    }),
                    Some((
                        single,
                        wide.partial_cmp(&f64::from(f32::from_bits(single)))
                            .unwrap()
                    ))
                );
            }
            assert_eq!(
                checked(|r| Some(Chain::load(r, control, -1.0)?.sqrt().store())),
                None
            );
        }
    }

    #[test]
    fn unsupported_modes_inputs_and_integer_overflow_decline() {
        for control in [0x037f, 0x017f, 0x0a7f, 0x027e] {
            assert_eq!(
                checked(|r| Some(Chain::load(r, control, 1.0)?.store())),
                None
            );
        }
        assert_eq!(
            checked(|r| Some(Chain::load(r, 0x027f, f32::INFINITY)?.store())),
            None
        );
        assert_eq!(
            checked(|r| Chain::load(r, 0x027f, 2_147_483_648.0)?.integer()),
            None
        );
        assert_eq!(
            checked(|r| Chain::load(r, 0x027f, -2_147_483_648.0)?.integer()),
            Some(i32::MIN)
        );
        assert_eq!(
            checked(|r| {
                let half = Chain::load(r, 0x007f, 0.5)?;
                half.compare(-half)
            }),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn a_guest_region_models_what_the_checked_path_declines() {
        use super::super::checked_guest;
        // SSE: an overflow and a NaN operand run on the model and raise
        // OE/PE and IE; a subnormal operand DE. Nothing declines.
        let got = checked_guest(0x1f80, 0x027f, |r| {
            let big = r.mul(f32::MAX, 2.0);
            let nan = r.add(f32::NAN, 1.0);
            let tiny = r.mul(f32::from_bits(1), 1.0);
            let order = r.comiss(f32::NAN, 0.0, false);
            Some((big.to_bits(), nan.is_nan(), tiny.to_bits(), order, r.mxcsr()))
        });
        let (big, nan, tiny, order, mxcsr) = got.expect("a guest region never declines on a value");
        assert_eq!(big, f32::INFINITY.to_bits());
        assert!(nan);
        assert_eq!(tiny, 1);
        assert_eq!(order, None);
        let m = mxcsr.unwrap();
        assert_eq!(m & 0x3f, 0x01 | 0x02 | 0x08 | 0x20, "IE, DE, OE and PE: {m:#x}");

        // x87: a chain off the checked path and its store match the model.
        let (bits, flags, int) = checked_guest(0x1f80, 0x027f, |r| {
            let a = Chain::load(r, 0x027f, 1.0e-30)?;
            let b = Chain::load(r, 0x027f, 1.0e-30)?;
            let product = a * b;
            let stored = product.store();
            let int = Chain::load(r, 0x027f, f32::INFINITY)?.integer()?;
            Some((stored.to_bits(), r.x87_flags(), int))
        })
        .unwrap();
        let mut s = fp::X87Abi {
            control: 0x027f,
            tag: 0xffff,
            ..Default::default()
        };
        fp::ld_f32(&mut s, 1.0e-30f32.to_bits());
        fp::arith(&mut s, fp::arith_op::MUL, 0, fp::src_kind::F32, u64::from(1.0e-30f32.to_bits()));
        let (_, store) = fp::st_f32(&s, 0);
        fp::store_commit(&mut s, store.status, true);
        fp::ld_f32(&mut s, f32::INFINITY.to_bits());
        let (_, fist) = fp::st_i32(&s, 0);
        fp::store_commit(&mut s, fist.status, true);
        assert_eq!(bits, store.value);
        assert_eq!(int, fist.value);
        assert_eq!(flags, s.status & 0x3f);
        assert_eq!(int, i32::MIN);
    }
}
