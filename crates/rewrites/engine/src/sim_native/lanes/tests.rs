//! Host lanes against the scalar loop and the SSE model, with no XBE.
//! Every value here is synthetic.
use super::super::{CpuAbi, Memory, Op, Vec4, mopp_query, pack, pose, unpack};
use super::{scalar, with};
use runtime::{GuestFault, TransferAbi};
use std::collections::BTreeMap;

/// Bit patterns at and around every boundary the fast path cares about.
const EDGES: [u32; 24] = [
    0x0000_0000, // +0
    0x8000_0000, // -0
    0x0000_0001, // smallest subnormal
    0x8000_0001,
    0x007f_ffff, // largest subnormal
    0x0080_0000, // smallest normal
    0x1fff_ffff, // just below the fast range
    0x2000_0000, // first in range
    0xa000_0000,
    0x5eff_ffff, // last in range
    0xdeff_ffff,
    0x5f00_0000, // first above
    0x7f7f_ffff, // largest finite
    0xff7f_ffff,
    0x7f80_0000, // +inf
    0xff80_0000, // -inf
    0x7fc0_0000, // quiet NaN
    0xffc0_1234,
    0x7f80_0001, // signalling NaN
    0xff80_0abc,
    0x3f80_0000, // 1
    0xbf80_0000,
    0x3eaa_aaab,
    0x4b00_0001, // integer-spaced, odd ulp
];

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        // xorshift32
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Mostly magnitudes near one (so products and sums stay in the fast
    /// range for a while), some across the whole fast range, some edges.
    fn float(&mut self) -> u32 {
        let r = self.next();
        let sign = r & 0x8000_0000;
        match r % 10 {
            0 | 1 => EDGES[(r >> 8) as usize % EDGES.len()],
            2 => sign | (super::LOW + (r >> 4) % (super::HIGH - super::LOW)),
            3 => r,
            _ => sign | (0x3800_0000 + (r >> 4) % 0x1000_0000),
        }
    }

    fn vec(&mut self) -> Vec4 {
        std::array::from_fn(|_| self.float())
    }
}

const OPS: [Op; 3] = [Op::Add, Op::Sub, Op::Mul];

fn model(op: Op, a: Vec4, b: Vec4) -> (Vec4, u32) {
    let mut mxcsr = 0x1fa0;
    let (a, b) = (pack(a), pack(b));
    let r = match op {
        Op::Add => simd::addps(&mut mxcsr, a, b),
        Op::Sub => simd::subps(&mut mxcsr, a, b),
        Op::Mul => simd::mulps(&mut mxcsr, a, b),
    }
    .expect("masked");
    (unpack(r), mxcsr)
}

fn check(a: Vec4, b: Vec4, fast: &mut usize) {
    let ordinary = scalar::ordinary(a, b);
    assert_eq!(
        super::ordinary(a, b),
        ordinary,
        "range check {a:08x?} {b:08x?}"
    );
    if !ordinary {
        return;
    }
    *fast += 1;
    for op in OPS {
        let lanes = super::binary32(op, a, b);
        let loop_ = scalar::binary32(op, a, b);
        let (guest, mxcsr) = model(op, a, b);
        assert_eq!(lanes, loop_, "lanes vs scalar {a:08x?} {b:08x?}");
        assert_eq!(lanes, guest, "lanes vs SSE model {a:08x?} {b:08x?}");
        assert_eq!(mxcsr, 0x1fa0, "the fast path never changes MXCSR");
    }
}

#[test]
fn lanes_match_the_scalar_loop_and_the_sse_model() {
    let mut rng = Rng(0x9e37_79b9);
    let mut fast = 0;
    for _ in 0..200_000 {
        let (a, b) = (rng.vec(), rng.vec());
        check(a, b, &mut fast);
        // Aliased operands, as in `q * q`.
        check(a, a, &mut fast);
    }
    assert!(fast > 20_000, "too few fast-path samples: {fast}");
}

#[test]
fn every_edge_in_every_lane() {
    let mut rng = Rng(0x0bad_cafe);
    let mut fast = 0;
    for &edge in &EDGES {
        for lane in 0..8 {
            for _ in 0..64 {
                let mut v: [u32; 8] =
                    std::array::from_fn(|_| 0x3f00_0000 | (rng.next() & 0x807f_ffff));
                v[lane] = edge;
                let a = [v[0], v[1], v[2], v[3]];
                let b = [v[4], v[5], v[6], v[7]];
                check(a, b, &mut fast);
            }
        }
    }
    // Pairs of edges cover sums that cancel to zero (x + -x), signed
    // zero products and the range boundaries on both operands.
    for &x in &EDGES {
        for &y in &EDGES {
            check(
                [x, y, x ^ 0x8000_0000, y],
                [y, x, x, y ^ 0x8000_0000],
                &mut fast,
            );
        }
    }
    assert!(fast > 0);
}

/// Sparse guest memory. Unwritten words read as a synthetic float drawn
/// from the address, so pointer chases and constants are arbitrary but
/// repeatable. Every store is logged in order.
#[derive(Clone)]
struct Mem {
    seed: u32,
    bytes: BTreeMap<u32, u8>,
    writes: Vec<(u32, Vec<u8>)>,
}

impl Mem {
    fn new(seed: u32) -> Self {
        Mem {
            seed,
            bytes: BTreeMap::new(),
            writes: Vec::new(),
        }
    }

    fn filler(&self, p: u32) -> u8 {
        let word = p & !3;
        let mut rng = Rng((word ^ self.seed).wrapping_mul(0x2545_f491) | 1);
        rng.next();
        (rng.float() >> (8 * (p & 3))) as u8
    }

    fn word(&mut self, a: u32, v: u32) {
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.bytes.insert(a.wrapping_add(i as u32), b);
        }
    }
}

impl Memory for Mem {
    fn read_bytes(&self, a: u32, dst: &mut [u8], _: u32) -> Result<(), GuestFault> {
        for (i, b) in dst.iter_mut().enumerate() {
            let p = a.wrapping_add(i as u32);
            *b = self
                .bytes
                .get(&p)
                .copied()
                .unwrap_or_else(|| self.filler(p));
        }
        Ok(())
    }

    fn write_bytes(&mut self, a: u32, src: &[u8], _: u32) -> Result<(), GuestFault> {
        self.writes.push((a, src.to_vec()));
        for (i, &b) in src.iter().enumerate() {
            self.bytes.insert(a.wrapping_add(i as u32), b);
        }
        Ok(())
    }
}

const STACK: u32 = 0x0070_0000;
const A: u32 = 0x0062_0000;
const B: u32 = A + 0x100;
const OUT: u32 = B + 0x100;

fn cpu(n: usize, rng: &mut Rng) -> CpuAbi {
    let mut c = CpuAbi {
        esp: STACK,
        ebp: STACK - 0x80,
        // Every mode has all exceptions masked (the native bodies'
        // precondition). Round-to-nearest modes, with or without FZ and
        // DAZ, take the fast path once PE is sticky; directed rounding
        // never does.
        mxcsr: [0x1f80, 0x1fa0, 0x9fa0, 0x1fe0, 0x3fa0, 0x5fa0, 0x7fa0][n % 7],
        eflags: 0x202,
        ..CpuAbi::default()
    };
    c.x87.control = [0x037f, 0x027f, 0x007f, 0x0f7f][n % 4];
    c.x87.st = std::array::from_fn(|i| [(i + 1) as u8; 10]);
    c.xmm = std::array::from_fn(|_| pack(rng.vec()).to_le_bytes());
    c
}

type Body = fn(&mut CpuAbi, &mut Mem) -> Result<TransferAbi, GuestFault>;

/// The body's whole visible effect (transfer, registers, flags, x87,
/// XMM, MXCSR and every store in order) is the same with and without
/// host lanes.
fn same(c: CpuAbi, m: &Mem, body: Body, label: &str) {
    let run = |on| {
        let (mut c, mut m) = (c, m.clone());
        let t = with(on, || body(&mut c, &mut m)).unwrap();
        (t, c, m.writes)
    };
    let (simd, plain) = (run(true), run(false));
    assert_eq!(simd.0, plain.0, "{label}: transfer");
    assert_eq!(simd.1, plain.1, "{label}: registers");
    assert_eq!(simd.2, plain.2, "{label}: stores");
}

#[test]
fn compose_is_identical_with_and_without_lanes() {
    let mut rng = Rng(0x1234_5678);
    for n in 0..4_000 {
        let mut c = cpu(n, &mut rng);
        let mut m = Mem::new(n as u32);
        for base in [A, B] {
            for i in 0..13 {
                m.word(base + 4 * i, rng.float());
            }
        }
        (c.ecx, c.edx) = (A, B);
        // In place on either input and partially overlapping both.
        let out = [OUT, A, B, B - 4, B + 4, A + 4, A + 8][n % 7];
        m.word(STACK, 0xdead_0000);
        m.word(STACK + 4, out);
        same(c, &m, pose::compose, &format!("compose {n}"));
    }
}

#[test]
fn pose_matrix_is_identical_with_and_without_lanes() {
    let mut rng = Rng(0x2468_ace0);
    for n in 0..4_000 {
        let c = cpu(n, &mut rng);
        let mut m = Mem::new(n as u32);
        for i in 0..8 {
            m.word(A + 4 * i, rng.float());
        }
        m.word(pose::POSE_INIT_FLAGS, (n % 2) as u32);
        // A small threshold so most quaternions take the rotation path.
        m.word(pose::POSE_NORM_THRESHOLD, 0x3000_0000);
        m.word(STACK, 0xdead_0000);
        m.word(STACK + 4, [OUT, A, A + 8, A + 4][n % 4]);
        m.word(STACK + 8, A);
        same(c, &m, pose::pose_matrix, &format!("pose {n}"));
    }
}

#[test]
fn rotation_codec_is_identical_with_and_without_lanes() {
    let mut rng = Rng(0x0f0f_1234);
    for n in 0..4_000 {
        let c = cpu(n, &mut rng);
        let mut m = Mem::new(n as u32);
        m.word(pose::ROTATION_BLOCK, A);
        m.word(pose::ROTATION_TRACK, 2);
        m.word(A + 20, 64);
        m.word(pose::FRAME_A, 1);
        m.word(pose::FRAME_B, 2);
        // Track `A + 32 + 2 * 64`, frames 1 and 2.
        let first = A + 32 + 2 * 64 + 16;
        for i in 0..8 {
            m.word(first + 4 * i, rng.float());
        }
        m.word(pose::ROTATION_WEIGHT, rng.float());
        m.word(pose::ROTATION_OUT, [OUT, first, first + 16][n % 3]);
        m.word(STACK, 0xdead_0000);
        same(c, &m, pose::rotation_codec, &format!("codec {n}"));
    }
}

#[test]
fn mopp_ray_is_identical_with_and_without_lanes() {
    // Programs that rescale, bound and split, so the packed SSE clip
    // blends run. Endpoints include exact ties with the split planes.
    const PROGRAMS: [&[u8]; 9] = [
        &[0x02, 1, 2, 3, 0x26, 6, 30, 0x33],
        &[0x10, 32, 8, 1, 0x34, 0x36],
        &[0x26, 7, 30, 0x33],
        &[0x29, 0, 0, 7, 0, 0, 31, 0x32],
        &[0x20, 24, 1, 0x31, 0x32],
        &[0x23, 32, 8, 0, 0, 0, 1, 0x31, 0x32],
        &[0x15, 25, 13, 1, 0x33, 0x31],
        &[0x1b, 91, 96, 1, 0x31, 0x32],
        &[0x03, 1, 1, 1, 0x10, 40, 40, 1, 0x31, 0x32],
    ];
    const ENDPOINTS: [(f32, f32); 6] = [
        (1.25, 6.5),
        (65.0, 41.25),
        (32.0, 8.0),
        (8.0, 8.0),
        (24.0, 25.0),
        (7.0, 30.0),
    ];
    let (def, code, model) = (A + 0x4000, A + 0x3800, A + 0x4800);
    let mut rng = Rng(0x7777_0001);
    for n in 0..PROGRAMS.len() * ENDPOINTS.len() * 12 {
        let mut c = cpu(n, &mut rng);
        let mut m = Mem::new(n as u32);
        c.ecx = A;
        m.word(STACK, 0xdead_0000);
        m.word(STACK + 4, def);
        m.word(STACK + 8, code);
        m.word(STACK + 12, model);
        let (start, end) = ENDPOINTS[(n / PROGRAMS.len()) % ENDPOINTS.len()];
        m.word(model, start.to_bits());
        m.word(model + 16, end.to_bits());
        for lane in 1..4 {
            m.word(model + 4 * lane, rng.float());
            m.word(model + 16 + 4 * lane, rng.float());
        }
        m.word(A + 20, 1.0f32.to_bits());
        m.word(def + 20, 1.0f32.to_bits());
        m.word(A + 68, 0);
        let program = PROGRAMS[n % PROGRAMS.len()];
        for (i, &b) in program.iter().enumerate() {
            m.bytes.insert(code + i as u32, b);
        }
        same(c, &m, mopp_query::ray_query, &format!("ray {n}"));
    }
}
