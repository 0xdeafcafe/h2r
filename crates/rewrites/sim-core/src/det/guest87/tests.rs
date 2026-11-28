use super::*;
use x87::abi as fp;

/// `fld b; fld a; f<op> st, st(1); fsqrt?; fstp dword` on the `x87`
/// crate's model.
fn model(control: u16, a: u32, op: u32, b: u32, sqrt: bool) -> (u32, u16) {
    let mut s = fp::X87Abi {
        control,
        tag: 0xffff,
        ..Default::default()
    };
    fp::ld_f32(&mut s, b);
    fp::ld_f32(&mut s, a);
    fp::arith(&mut s, op, 0, fp::src_kind::ST, 1);
    if sqrt {
        fp::unary(&mut s, fp::unary_op::SQRT);
    }
    let (_, store) = fp::st_f32(&s, 0);
    fp::store_commit(&mut s, store.status, true);
    (store.value, s.status & 0x3f)
}

fn ours(control: u16, a: u32, op: u32, b: u32, sqrt: bool) -> (u32, u16) {
    let x = X87::new(control);
    let (a, b) = (x.load32(a), x.load32(b));
    let mut r = match op {
        fp::arith_op::ADD => x.add(a, b),
        fp::arith_op::SUB => x.sub(a, b),
        fp::arith_op::MUL => x.mul(a, b),
        _ => x.div(a, b),
    };
    if sqrt {
        r = x.sqrt(r);
    }
    (x.store32(r), x.flags())
}

const EDGES: [u32; 16] = [
    0,
    0x8000_0000,
    1,
    0x8000_0001,
    0x0080_0000,
    0x007f_ffff,
    0x3f80_0000,
    0xbf80_0000,
    0x7f7f_ffff,
    0xff7f_ffff,
    0x7f80_0000,
    0xff80_0000,
    0x7fc0_0000,
    0x7fa0_0000,
    0x0d80_0000,
    0x7280_0000,
];

#[test]
fn every_operation_matches_the_model_in_each_control() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let ops = [fp::arith_op::ADD, fp::arith_op::SUB, fp::arith_op::MUL, fp::arith_op::DIV];
    // Nearest in single, double and extended; then chop and up.
    let controls = [0x007f, 0x027f, 0x037f, 0x0e7f, 0x0a7f];
    for i in 0..40_000u32 {
        let r = next();
        let pick = |w: u64| {
            if w % 4 == 0 {
                EDGES[(w >> 8) as usize % EDGES.len()]
            } else {
                // Any exponent, any sign.
                (w >> 16) as u32
            }
        };
        let (a, b) = (pick(r), pick(r.rotate_left(29)));
        let op = ops[(r >> 60) as usize & 3];
        let sqrt = (r >> 59) & 1 == 1;
        let control = controls[i as usize % controls.len()];
        let want = model(control, a, op, b, sqrt);
        let got = ours(control, a, op, b, sqrt);
        // NaN payloads both come from the model; compare them exactly.
        assert_eq!(got, want, "{a:#x} op {op} {b:#x} sqrt {sqrt} control {control:#x}");
    }
}

#[test]
fn stores_conversions_and_compares_match_the_model() {
    for control in [0x007f, 0x027f, 0x037f] {
        for &a in &EDGES {
            for &b in &EDGES {
                let x = X87::new(control);
                let (sa, sb) = (x.load32(a), x.load32(b));
                let got = x.compare(sa, sb, false);
                let mut s = fp::X87Abi {
                    control,
                    tag: 0xffff,
                    ..Default::default()
                };
                fp::ld_f32(&mut s, b);
                fp::ld_f32(&mut s, a);
                let before = s.status & 0x3f;
                fp::cmp(&mut s, 0, fp::src_kind::ST, 1, 0, false);
                let c = s.status & 0x4500;
                let want = match c {
                    0x0000 => Some(Ordering::Greater),
                    0x0100 => Some(Ordering::Less),
                    0x4000 => Some(Ordering::Equal),
                    _ => None,
                };
                assert_eq!(got, want, "{a:#x} cmp {b:#x}");
                assert_eq!(x.flags(), (s.status & 0x3f) | before, "{a:#x} cmp {b:#x} flags");
            }
            // fistp of a and of a * 2^20.
            for scale in [0x3f80_0000u32, 0x4980_0000] {
                let x = X87::new(control);
                let v = x.mul(x.load32(a), x.load32(scale));
                let got = x.fist32(v);
                let mut s = fp::X87Abi {
                    control,
                    tag: 0xffff,
                    ..Default::default()
                };
                fp::ld_f32(&mut s, scale);
                fp::ld_f32(&mut s, a);
                fp::arith(&mut s, fp::arith_op::MUL, 0, fp::src_kind::ST, 1);
                let (_, store) = fp::st_i32(&s, 0);
                fp::store_commit(&mut s, store.status, true);
                assert_eq!(got, store.value, "fistp {a:#x} * {scale:#x}");
                assert_eq!(x.flags(), s.status & 0x3f, "fistp {a:#x} * {scale:#x} flags");
            }
        }
    }
}

#[test]
fn fast_path_values_stay_on_the_host() {
    let x = X87::new(0x027f);
    let v = x.mul(x.load32(1.5f32.to_bits()), x.load32(3.0f32.to_bits()));
    assert!(!v.is_wide());
    assert_eq!(x.store32(v), 4.5f32.to_bits());
    assert_eq!(x.flags(), 0);
    let third = x.div(x.load32(1.0f32.to_bits()), x.load32(3.0f32.to_bits()));
    assert!(!third.is_wide());
    assert_eq!(x.flags(), 0x20, "1/3 is inexact");
    // A NaN leaves the host and raises IE in the compare.
    let nan = x.load32(0x7fc0_0000);
    assert!(nan.is_wide());
    assert_eq!(x.compare(nan, third, false), None);
    assert_eq!(x.flags() & 1, 1);
}
