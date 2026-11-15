//! High-precision constants for the transcendentals, and the load-constant
//! instructions.
#![forbid(unsafe_code)]

use crate::wide::Wf;
use crate::{Const, F80, Rc};

/// pi, 1024 bits, most significant limb first, leading bit 2^1.
pub(crate) const PI: [u64; 16] = [
    0xc90f_daa2_2168_c234,
    0xc4c6_628b_80dc_1cd1,
    0x2902_4e08_8a67_cc74,
    0x020b_bea6_3b13_9b22,
    0x514a_0879_8e34_04dd,
    0xef95_19b3_cd3a_431b,
    0x302b_0a6d_f25f_1437,
    0x4fe1_356d_6d51_c245,
    0xe485_b576_625e_7ec6,
    0xf44c_42e9_a637_ed6b,
    0x0bff_5cb6_f406_b7ed,
    0xee38_6bfb_5a89_9fa5,
    0xae9f_2411_7c4b_1fe6,
    0x4928_6651_ece4_5b3d,
    0xc200_7cb8_a163_bf05,
    0x98da_4836_1c55_d39a,
];
/// 2/pi, 1024 bits, leading bit 2^-1.
pub(crate) const TWO_OVER_PI: [u64; 16] = [
    0xa2f9_836e_4e44_1529,
    0xfc27_57d1_f534_ddc0,
    0xdb62_9599_3c43_9041,
    0xfe51_63ab_debb_c561,
    0xb724_6e3a_424d_d2e0,
    0x0649_2eea_09d1_921c,
    0xfe1d_eb1c_b129_a73e,
    0xe882_35f5_2ebb_4484,
    0xe99c_7026_b45f_7e41,
    0x3991_d639_8353_39f4,
    0x9c84_5f8b_bdf9_283b,
    0x1ff8_97ff_de05_980f,
    0xef2f_118b_5a0a_6d1f,
    0x6d36_7ecf_27cb_09b7,
    0x4f46_3f66_9e5f_ea2d,
    0x7527_bac7_ebe5_f17b,
];
/// ln 2, 1024 bits, leading bit 2^-1.
pub(crate) const LN2: [u64; 16] = [
    0xb172_17f7_d1cf_79ab,
    0xc9e3_b398_03f2_f6af,
    0x40f3_4326_7298_b62d,
    0x8a0d_175b_8baa_fa2b,
    0xe7b8_7620_6deb_ac98,
    0x5595_52fb_4afa_1b10,
    0xed2e_ae35_c138_2144,
    0x2757_3b29_1169_b825,
    0x3e96_ca16_224a_e8c5,
    0x1acb_da11_317c_387e,
    0xb9ea_9bc3_b136_603b,
    0x256f_a0ec_7657_f74b,
    0x72ce_87b1_9d65_48ca,
    0xf5df_a6bd_3830_3248,
    0x655f_a187_2f20_e3a2,
    0xda2d_97c5_0f3f_d5c6,
];

pub(crate) fn pi<const N: usize>() -> Wf<N> {
    Wf::from_be_limbs(&PI, 1)
}
pub(crate) fn two_over_pi<const N: usize>() -> Wf<N> {
    Wf::from_be_limbs(&TWO_OVER_PI, -1)
}
pub(crate) fn ln2<const N: usize>() -> Wf<N> {
    Wf::from_be_limbs(&LN2, -1)
}

pub(crate) const fn load_const(c: Const, rc: Rc) -> F80 {
    // (round-to-nearest value, whether that rounding went up).
    let (rn, rounded_up) = match c {
        Const::One => return F80::ONE,
        Const::Z => return F80::ZERO,
        Const::Pi => (F80::PI, true),
        Const::L2E => (F80::L2E, true),
        Const::L2T => (F80::L2T, false),
        Const::Lg2 => (F80::LG2, true),
        Const::Ln2 => (F80::LN2, true),
    };
    // Every constant is positive, irrational, and none has an all-ones or
    // all-zero significand, so a directed result is RN or its neighbour.
    let sig = match (rc, rounded_up) {
        (Rc::Near, _) | (Rc::Up, true) | (Rc::Down | Rc::Chop, false) => rn.sig,
        (Rc::Up, false) => rn.sig + 1,
        (Rc::Down | Rc::Chop, true) => rn.sig - 1,
    };
    F80 { sig, se: rn.se }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::println;

    pub(crate) type G = Wf<18>;

    /// atan(1/k) by its Taylor series.
    pub(crate) fn atan_inv(k: u64) -> G {
        let mut p = G::one().div_u64(k);
        let mut sum = p;
        let mut n = 1u64;
        loop {
            p = p.div_u64(k * k);
            let t = p.div_u64(2 * n + 1);
            if t.is_zero() || t.top() < sum.top() - 18 * 64 - 8 {
                return sum;
            }
            sum = if n % 2 == 1 { sum.sub(t) } else { sum.add(t) };
            n += 1;
        }
    }

    /// atanh(1/k) by its Taylor series.
    pub(crate) fn atanh_inv(k: u64) -> G {
        let mut p = G::one().div_u64(k);
        let mut sum = p;
        let mut n = 1u64;
        loop {
            p = p.div_u64(k * k);
            let t = p.div_u64(2 * n + 1);
            if t.is_zero() || t.top() < sum.top() - 18 * 64 - 8 {
                return sum;
            }
            sum = sum.add(t);
            n += 1;
        }
    }

    /// Machin: pi = 16 atan(1/5) - 4 atan(1/239).
    pub(crate) fn machin_pi() -> G {
        atan_inv(5).scale(4).sub(atan_inv(239).scale(2))
    }

    fn be(w: G) -> [u64; 16] {
        let mut out = [0u64; 16];
        for (i, o) in out.iter_mut().enumerate() {
            *o = w.m[17 - i];
        }
        out
    }

    /// Størmer's formula: pi/4 = 12 atan(1/49) + 32 atan(1/57) - 5 atan(1/239)
    /// + 12 atan(1/110443), independent of Machin's.
    #[test]
    fn pi_table_matches_two_formulas() {
        let table = G::from_be_limbs(&PI, 1);
        let stormer = atan_inv(49)
            .mul_u64(12)
            .add(atan_inv(57).mul_u64(32))
            .sub(atan_inv(239).mul_u64(5))
            .add(atan_inv(110443).mul_u64(12))
            .scale(2);
        for w in [machin_pi(), stormer] {
            let d = w.sub(table);
            assert!(
                d.is_zero() || d.top() < 1 - 1024 + 2,
                "pi differs at 2^{}",
                d.top()
            );
        }
    }

    /// The published hex digits of pi (3.243F6A88...), which are also
    /// Blowfish's P-array initialisation: 18 words, 576 fraction bits.
    #[test]
    fn pi_table_matches_published_hex_digits() {
        const P: [u32; 18] = [
            0x243f6a88, 0x85a308d3, 0x13198a2e, 0x03707344, 0xa4093822, 0x299f31d0, 0x082efa98,
            0xec4e6c89, 0x452821e6, 0x38d01377, 0xbe5466cf, 0x34e90c6c, 0xc0ac29b7, 0xc97c50dd,
            0x3f84d5b5, 0xb5470917, 0x9216d5d9, 0x8979fb1b,
        ];
        let mut want = std::vec![true, true]; // the integer part, 3
        for w in P {
            for b in (0..32).rev() {
                want.push(w >> b & 1 == 1);
            }
        }
        for (i, &bit) in want.iter().enumerate() {
            let got = PI[i / 64] >> (63 - i % 64) & 1 == 1;
            assert_eq!(got, bit, "pi bit {i}");
        }
    }

    #[test]
    fn two_over_pi_and_ln2_tables() {
        let pi = G::from_be_limbs(&PI, 1);
        let tpi = G::from_be_limbs(&TWO_OVER_PI, -1);
        let d = tpi.mul(pi).sub(G::from_u64(2));
        assert!(
            d.is_zero() || d.top() < -1018,
            "2/pi * pi - 2 = 2^{}",
            d.top()
        );
        // ln 2 = 2 atanh(1/3) = 4 atanh(1/7) + 2 atanh(1/17).
        let ln2 = G::from_be_limbs(&LN2, -1);
        for w in [
            atanh_inv(3).scale(1),
            atanh_inv(7).scale(2).add(atanh_inv(17).scale(1)),
        ] {
            let d = w.sub(ln2);
            assert!(
                d.is_zero() || d.top() < -1022,
                "ln2 differs at 2^{}",
                d.top()
            );
        }
    }

    /// Each load constant is the correctly rounded exact value under every
    /// RC; `load_const`'s rounding directions are checked against the exact
    /// constants.
    #[test]
    fn load_constants_are_correctly_rounded() {
        let ln2 = G::from_be_limbs(&LN2, -1);
        // ln 10 = 3 ln 2 + ln(5/4) = 3 ln 2 + 2 atanh(1/9).
        let ln10 = ln2.mul_u64(3).add(atanh_inv(9).scale(1));
        let exact = [
            (Const::Pi, G::from_be_limbs(&PI, 1)),
            (Const::L2E, G::one().div(ln2)),
            (Const::L2T, ln10.div(ln2)),
            (Const::Lg2, ln2.div(ln10)),
            (Const::Ln2, ln2),
        ];
        for (c, w) in exact {
            let (rn, _) = w.to_f80(1);
            assert_eq!(load_const(c, Rc::Near), rn, "{c:?}");
            let rn_w = G::from_parts(false, rn.sig, rn.se as i32 - 16446);
            let up = rn_w.cmp_abs(&w) == core::cmp::Ordering::Greater;
            let (lo, hi) = if up {
                (rn.sig - 1, rn.sig)
            } else {
                (rn.sig, rn.sig + 1)
            };
            assert_eq!(load_const(c, Rc::Up).sig, hi, "{c:?} up");
            assert_eq!(load_const(c, Rc::Down).sig, lo, "{c:?} down");
            assert_eq!(load_const(c, Rc::Chop).sig, lo, "{c:?} chop");
        }
        assert_eq!(load_const(Const::One, Rc::Down), F80::ONE);
        assert_eq!(load_const(Const::Z, Rc::Up), F80::ZERO);
    }

    #[test]
    #[ignore = "generator for the tables above"]
    fn print_constants() {
        let pi = machin_pi();
        let ln2 = atanh_inv(3).scale(1);
        let tpi = G::from_u64(2).div(pi);
        for (name, w) in [("PI", pi), ("TWO_OVER_PI", tpi), ("LN2", ln2)] {
            println!("{name} top={}", w.top());
            for l in be(w) {
                println!(
                    "    0x{:04x}_{:04x}_{:04x}_{:04x},",
                    l >> 48,
                    (l >> 32) & 0xffff,
                    (l >> 16) & 0xffff,
                    l & 0xffff
                );
            }
        }
    }
}
