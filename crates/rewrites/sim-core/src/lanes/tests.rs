//! Host lanes against the scalar loop. Every value is synthetic.
use super::*;

const EDGES: [u32; 16] = [
    0x0000_0000,
    0x8000_0000,
    0x0000_0001,
    0x007f_ffff,
    0x0080_0000,
    0x1fff_ffff,
    0x2000_0000,
    0x5eff_ffff,
    0x5f00_0000,
    0x7f7f_ffff,
    0x7f80_0000,
    0xff80_0000,
    0x7fc0_0000,
    0x7f80_0001,
    0x3f80_0000,
    0xbeaa_aaab,
];

fn xorshift(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

#[test]
fn ordinary_matches_the_scalar_check_on_every_edge() {
    for (i, &a) in EDGES.iter().enumerate() {
        for &b in &EDGES {
            let x = [a, b, EDGES[(i + 3) % EDGES.len()], 0x3f80_0000];
            let y = [b, 0, a, 0x4000_0000];
            assert_eq!(ordinary(x, y), scalar::ordinary(x, y), "{x:08x?} {y:08x?}");
        }
    }
    assert!(ordinary1(0) && ordinary1(0x8000_0000) && ordinary1(LOW) && !ordinary1(HIGH));
    assert!(!ordinary1(LOW - 1) && ordinary1(HIGH - 1) && !ordinary1(1));
}

#[test]
fn host_lanes_match_one_scalar_operation_per_lane() {
    let mut state = 0x1234_5678;
    for _ in 0..20_000 {
        let mut v = || {
            let r = xorshift(&mut state);
            (r & 0x8000_0000) | (0x3800_0000 + (r >> 4) % 0x1000_0000)
        };
        let a = [v(), v(), v(), v()];
        let b = [v(), v(), v(), v()];
        assert!(ordinary(a, b));
        for op in [Op::Add, Op::Sub, Op::Mul] {
            assert_eq!(binary32(op, a, b), scalar::binary32(op, a, b));
        }
    }
}
