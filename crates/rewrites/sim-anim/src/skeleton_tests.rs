//! The skeleton walk on synthetic records and matrices only. The two
//! leaves are checked against the guest's formulas written out in plain
//! `f32` (each operation rounded once, in the guest's order); the walk is
//! checked against the leaves called one node at a time. The guest-memory
//! equivalence against the register-level ports is in
//! `engine::sim_anim::skeleton`'s tests.
use crate::kernel::{Record, Unsupported};
use crate::skeleton::{Constants, Walk, build_matrices, build_matrices_guest, compose, pose};
use sim_core::det::{self, Fp};
use sim_core::lanes::Vec4;
use sim_core::math::{NodeMatrix, Vec3};

const ONE: u32 = 0x3f80_0000;
const SIGN: u32 = 0x8000_0000;
/// A sign mask with lanes both set and clear, so the XOR is visible.
const SIGNS: Vec4 = [SIGN, 0, 0, SIGN];

fn k() -> Constants {
    Constants {
        one: ONE,
        threshold: 1e-6f32.to_bits(),
        signs: SIGNS,
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// A value of magnitude in `[0.25, 4)` with either sign: on the fast
    /// path, with products and sums that stay on it.
    fn value(&mut self) -> u32 {
        let r = self.next();
        let exponent = 125 + (r % 4) as u32;
        let mantissa = (r >> 8) as u32 & 0x7f_ffff;
        let sign = ((r >> 40) as u32 & 1) << 31;
        sign | exponent << 23 | mantissa
    }
    /// A node record. The scale stays in `[0.5, 2)` so a chain of
    /// compositions keeps its products on the fast path.
    fn record(&mut self) -> Record {
        let mut r: Record = std::array::from_fn(|_| self.value());
        r[7] = (r[7] & 0x807f_ffff) | (126 + (self.next() % 2) as u32) << 23;
        r
    }
    fn matrix(&mut self) -> NodeMatrix {
        let mut v = || Vec3([self.value(), self.value(), self.value()]);
        let (forward, left, up, position) = (v(), v(), v(), v());
        NodeMatrix {
            scale: self.value(),
            forward,
            left,
            up,
            position,
        }
    }
}

fn f(x: u32) -> f32 {
    f32::from_bits(x)
}

fn bits(v: [f32; 3]) -> Vec3 {
    Vec3(v.map(f32::to_bits))
}

/// `1421f0` written out in `f32`.
fn reference_pose(k: &Constants, r: &Record) -> NodeMatrix {
    let q = [f(r[0]), f(r[1]), f(r[2]), f(r[3])];
    let sq = q.map(|x| x * x);
    let norm = (sq[0] + sq[2]) + (sq[1] + sq[3]);
    let one = f(k.one);
    let (forward, left, up) = if norm <= f(k.threshold) {
        ([one, 0.0, 0.0], [0.0, one, 0.0], [0.0, 0.0, one])
    } else {
        let r = f(det::rcp(norm.to_bits()));
        let inverse = (r + r) - (norm * r) * r;
        let factor = inverse + inverse;
        let s = q.map(|x| x * factor);
        let xx = s.map(|x| q[0] * x);
        let ww = s.map(|x| q[3] * x);
        let yy = s.map(|x| q[1] * x);
        let zz = s[2] * q[2];
        let w = [ww[2], ww[1], ww[2], ww[1]];
        let signed: [f32; 4] = std::array::from_fn(|i| f(w[i].to_bits() ^ k.signs[i]));
        let cross = [
            xx[1] + signed[0],
            xx[2] + signed[1],
            xx[1] + signed[2],
            xx[2] + signed[3],
        ];
        (
            [(one - zz) - yy[1], cross[2], cross[3]],
            [cross[0], (one - zz) - xx[0], yy[2] + ww[0]],
            [cross[1], yy[2] - ww[0], (one - xx[0]) - yy[1]],
        )
    };
    NodeMatrix {
        scale: r[7],
        forward: bits(forward),
        left: bits(left),
        up: bits(up),
        position: Vec3([r[4], r[5], r[6]]),
    }
}

/// `142a60` written out in `f32`.
fn reference_compose(a: &NodeMatrix, b: &NodeMatrix) -> NodeMatrix {
    let v = |v: Vec3| v.0.map(f);
    let (af, al, au, ap) = (v(a.forward), v(a.left), v(a.up), v(a.position));
    let s = f(a.scale);
    let apply = |c: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|i| c[0] * af[i] + (c[1] * al[i] + c[2] * au[i]))
    };
    let t = v(b.position);
    let position: [f32; 3] =
        std::array::from_fn(|i| ((t[0] * af[i] + t[1] * al[i]) + t[2] * au[i]) * s + ap[i]);
    NodeMatrix {
        scale: (f(b.scale) * s).to_bits(),
        forward: bits(apply(v(b.forward))),
        left: bits(apply(v(b.left))),
        up: bits(apply(v(b.up))),
        position: bits(position),
    }
}

fn identity() -> NodeMatrix {
    NodeMatrix {
        scale: ONE,
        forward: Vec3([ONE, 0, 0]),
        left: Vec3([0, ONE, 0]),
        up: Vec3([0, 0, ONE]),
        position: Vec3([0; 3]),
    }
}

#[test]
fn pose_matches_the_f32_reference() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let k = k();
    for _ in 0..2000 {
        let r = rng.record();
        let mut fp = Fp::new();
        let got = pose(&mut fp, &k, &r, [0; 4]);
        assert!(fp.exact());
        assert_eq!(got, reference_pose(&k, &r), "{r:x?}");
    }
}

#[test]
fn pose_at_or_below_the_cutoff_is_the_identity_basis() {
    let mut r: Record = [0; 8];
    r[4..8].copy_from_slice(&[2.0f32, -3.0, 0.5, 1.5].map(f32::to_bits));
    let k = Constants { threshold: 0, ..k() };
    let mut fp = Fp::new();
    let got = pose(&mut fp, &k, &r, [0; 4]);
    assert!(fp.exact());
    let want = NodeMatrix {
        scale: 1.5f32.to_bits(),
        position: Vec3([2.0f32, -3.0, 0.5].map(f32::to_bits)),
        ..identity()
    };
    assert_eq!(got, want, "a zero quaternion's norm equals the cutoff");
}

#[test]
fn compose_matches_the_f32_reference_and_the_identity_parent() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for _ in 0..2000 {
        let (a, b) = (rng.matrix(), rng.matrix());
        let mut fp = Fp::new();
        let (got, xmm) = compose(&mut fp, &a, &b);
        assert!(fp.exact());
        assert_eq!(got, reference_compose(&a, &b));
        // Lane 1 of the last result is a signed zero; the others are the
        // forward column.
        assert_eq!([xmm[0][0], xmm[0][2], xmm[0][3]], got.forward.0);
        assert_eq!(xmm[0][1] & 0x7fff_ffff, 0);
        assert_eq!(xmm[3], [got.scale, 0, 0, 0]);
        assert_eq!(xmm[7], [a.scale; 4]);
        let (same, _) = compose(&mut fp, &identity(), &b);
        assert_eq!(same, b, "the identity parent leaves the child");
    }
}

/// The walk one node at a time through the leaves.
fn by_hand(
    k: &Constants,
    locals: &[Record],
    parents: &[i16],
    keep: &[u32],
    mut xmm: [Vec4; 8],
    out: &mut [NodeMatrix],
) -> Walk {
    let mut last = None;
    for i in 1..out.len() {
        if keep[i / 32] & (1 << (i % 32)) != 0 {
            continue;
        }
        let mut fp = Fp::new();
        let posed = pose(&mut fp, k, &locals[i], xmm[1]);
        out[i] = posed;
        let (m, regs) = compose(&mut fp, &out[parents[i] as usize], &posed);
        assert!(fp.exact());
        out[i] = m;
        xmm = regs;
        last = Some(i);
    }
    Walk { last, xmm }
}

#[test]
fn the_walk_keeps_masked_nodes_across_mask_words() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let k = k();
    for count in [2usize, 31, 32, 33, 40, 70] {
        let locals: Vec<Record> = (0..count).map(|_| rng.record()).collect();
        let parents: Vec<i16> = (0..count)
            .map(|i| if i == 0 { -1 } else { (rng.next() % i as u64) as i16 })
            .collect();
        let keep: Vec<u32> = (0..count.div_ceil(32)).map(|_| rng.next() as u32).collect();
        let start: Vec<NodeMatrix> = (0..count).map(|_| rng.matrix()).collect();
        let xmm: [Vec4; 8] = std::array::from_fn(|_| std::array::from_fn(|_| rng.value()));

        let mut got = start.clone();
        let walk = build_matrices(&k, &locals, &parents, &keep, xmm, &mut got).unwrap();
        let mut want = start.clone();
        let expected = by_hand(&k, &locals, &parents, &keep, xmm, &mut want);
        assert_eq!(walk, expected, "count {count}");
        assert_eq!(got, want, "count {count}");
        for i in 0..count {
            if i == 0 || keep[i / 32] & (1 << (i % 32)) != 0 {
                assert_eq!(got[i], start[i], "node {i} is kept");
            }
        }
    }
}

#[test]
fn every_node_kept_leaves_matrices_and_registers() {
    let mut rng = Rng(77);
    let start: Vec<NodeMatrix> = (0..5).map(|_| rng.matrix()).collect();
    let locals = vec![rng.record(); 5];
    let xmm = [[ONE, 2, 3, 4]; 8];
    let mut out = start.clone();
    let walk = build_matrices(&k(), &locals, &[0; 5], &[u32::MAX], xmm, &mut out).unwrap();
    assert_eq!(walk, Walk { last: None, xmm });
    assert_eq!(out, start);
}

#[test]
fn parents_are_read_as_they_stand() {
    let mut rng = Rng(0xfeed_beef);
    let k = k();
    let locals: Vec<Record> = (0..4).map(|_| rng.record()).collect();
    let start: Vec<NodeMatrix> = (0..4).map(|_| rng.matrix()).collect();
    // Node 2 names node 3, which the walk has not reached; node 3 names
    // itself, so it composes with its own pose.
    let parents = [-1i16, 0, 3, 3];
    let mut out = start.clone();
    let walk = build_matrices(&k, &locals, &parents, &[0], [[0; 4]; 8], &mut out).unwrap();
    assert_eq!(walk.last, Some(3));

    let mut fp = Fp::new();
    let p1 = pose(&mut fp, &k, &locals[1], [0; 4]);
    let (m1, x1) = compose(&mut fp, &start[0], &p1);
    let p2 = pose(&mut fp, &k, &locals[2], x1[1]);
    let (m2, x2) = compose(&mut fp, &start[3], &p2);
    let p3 = pose(&mut fp, &k, &locals[3], x2[1]);
    let (m3, x3) = compose(&mut fp, &p3, &p3);
    assert!(fp.exact());
    assert_eq!(out, vec![start[0], m1, m2, m3]);
    assert_eq!(walk.xmm, x3);
}

#[test]
fn bad_parents_short_inputs_and_off_path_operands_decline() {
    let mut rng = Rng(5);
    let k = k();
    let locals: Vec<Record> = (0..3).map(|_| rng.record()).collect();
    let start: Vec<NodeMatrix> = (0..3).map(|_| rng.matrix()).collect();
    let run = |locals: &[Record], parents: &[i16], keep: &[u32], xmm: [Vec4; 8]| {
        let mut out = start.clone();
        build_matrices(&k, locals, parents, keep, xmm, &mut out)
    };
    let zero = [[0; 4]; 8];
    assert!(run(&locals[..], &[-1, 0, 1], &[0], zero).is_ok());
    assert_eq!(run(&locals[..], &[-1, -1, 1], &[0], zero), Err(Unsupported));
    assert_eq!(run(&locals[..], &[-1, 0, 3], &[0], zero), Err(Unsupported));
    // A bad parent under a kept node is never read.
    assert!(run(&locals[..], &[-1, 0, 3], &[0b100], zero).is_ok());
    assert_eq!(run(&locals[..2], &[-1, 0, 1], &[0], zero), Err(Unsupported));
    assert_eq!(run(&locals[..], &[-1, 0], &[0], zero), Err(Unsupported));
    assert_eq!(run(&locals[..], &[-1, 0, 1], &[], zero), Err(Unsupported));

    // A denormal in a quaternion.
    let mut odd = locals.clone();
    odd[2][1] = 1;
    assert_eq!(run(&odd[..], &[-1, 0, 1], &[0], zero), Err(Unsupported));
    // A denormal translation is an operand of the composition.
    let mut odd = locals.clone();
    odd[1][5] = 0x8000_0001;
    assert_eq!(run(&odd[..], &[-1, 0, 1], &[0], zero), Err(Unsupported));
    // An infinity in a dead lane of XMM1 on entry: the guest adds it.
    let mut xmm = zero;
    xmm[1][3] = 0x7f80_0000;
    assert_eq!(run(&locals[..], &[-1, 0, 1], &[0], xmm), Err(Unsupported));
    // The same lane of another register is never read.
    let mut xmm = zero;
    xmm[2][3] = 0x7f80_0000;
    assert!(run(&locals[..], &[-1, 0, 1], &[0], xmm).is_ok());
}

#[test]
fn host_lanes_and_the_scalar_loop_agree() {
    let mut rng = Rng(0xabcd_ef01_2345_6789);
    let k = k();
    let count = 48;
    let locals: Vec<Record> = (0..count).map(|_| rng.record()).collect();
    // A binary tree: six levels deep.
    let parents: Vec<i16> = (0..count).map(|i| (i as i16 - 1).max(0) / 2).collect();
    let start: Vec<NodeMatrix> = (0..count).map(|_| rng.matrix()).collect();
    let run = |simd| {
        sim_core::lanes::with(simd, || {
            let mut out = start.clone();
            let walk = build_matrices(&k, &locals, &parents, &[0, 0], [[0; 4]; 8], &mut out);
            (walk, out)
        })
    };
    let (a, b) = (run(true), run(false));
    assert!(a.0.is_ok());
    assert_eq!(a, b);
}

/// Under the live MXCSR (PE clear) the walk is the fast path's, and the
/// MXCSR comes out with PE set; an unmasked PE refuses.
#[test]
fn the_guest_walk_runs_under_the_live_mxcsr() {
    let mut rng = Rng(0x0a1d_face);
    let k = k();
    let locals: Vec<Record> = (0..12).map(|_| rng.record()).collect();
    let parents: Vec<i16> = (0..12)
        .map(|i| if i == 0 { -1 } else { (rng.next() % i as u64) as i16 })
        .collect();
    let start: Vec<NodeMatrix> = (0..12).map(|_| rng.matrix()).collect();
    let mut fast = start.clone();
    let want = build_matrices(&k, &locals, &parents, &[0], [[0; 4]; 8], &mut fast).unwrap();
    for mxcsr in [0x1f80, 0x1fa0] {
        let mut got = start.clone();
        let walk = build_matrices_guest(mxcsr, &k, &locals, &parents, &[0], [[0; 4]; 8], &mut got);
        assert_eq!(walk, Ok((want, 0x1fa0)), "{mxcsr:#x}");
        assert_eq!(got, fast, "{mxcsr:#x}");
    }
    let mut got = start.clone();
    let refused = build_matrices_guest(0x0f80, &k, &locals, &parents, &[0], [[0; 4]; 8], &mut got);
    assert_eq!(refused, Err(Unsupported));
}
