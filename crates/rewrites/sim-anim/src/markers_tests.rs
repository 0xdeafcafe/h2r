//! The marker kernel against formulas written out in plain `f32`, and
//! the fill against its leaves called instance by instance. The leaves
//! (`basis`, `compose`) and the walk are pinned against the lifted guest
//! by `tests/marker_differential.rs`.
use crate::markers::*;
use sim_core::det::Fp;
use sim_core::math::{NodeMatrix, Vec3};

fn constants() -> Constants {
    Constants {
        one: 1.0f32.to_bits(),
        small: 1.0e-4f32.to_bits(),
        two: 2.0f32.to_bits(),
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
    /// A magnitude in `[0.25, 4)`, either sign.
    fn value(&mut self) -> u32 {
        let r = self.next();
        let sign = ((r >> 40) as u32 & 1) << 31;
        sign | (125 + (r % 4) as u32) << 23 | (r >> 8) as u32 & 0x7f_ffff
    }
    /// A quaternion with components of magnitude in `[0.5, 2)`.
    fn quat(&mut self) -> [u32; 4] {
        std::array::from_fn(|_| {
            let r = self.next();
            (((r >> 32) as u32 & 1) << 31)
                | (126 + (r % 2) as u32) << 23
                | (r >> 8) as u32 & 0x7f_ffff
        })
    }
}

/// The basis as the guest computes it, one rounded operation at a time:
/// the squared norm, a scale factor from a division, and every product of
/// a component with an already-scaled factor.
fn reference_basis(k: &Constants, q: &[f32; 4]) -> [f32; 9] {
    let norm = ((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) + q[3] * q[3];
    let s = if norm <= f32::from_bits(k.small) {
        0.0
    } else {
        f32::from_bits(k.two) / norm
    };
    let q0s = q[0] * s;
    let q1s = q[1] * s;
    let q2s = s * q[2];
    let q0q0 = q[0] * q0s;
    let q0q1 = q[0] * q1s;
    let q0q2 = q[0] * q2s;
    let q0q3 = q0s * q[3];
    let q1q1 = q1s * q[1];
    let q1q2 = q[1] * q2s;
    let q1q3 = q1s * q[3];
    let q2q2 = q2s * q[2];
    let q2q3 = q2s * q[3];
    [
        1.0 - (q2q2 + q1q1),
        q0q1 + q2q3,
        q0q2 - q1q3,
        q0q1 - q2q3,
        1.0 - (q2q2 + q0q0),
        q1q2 + q0q3,
        q0q2 + q1q3,
        q1q2 - q0q3,
        1.0 - (q1q1 + q0q0),
    ]
}

#[test]
fn the_scan_finds_the_first_group_of_a_name() {
    assert_eq!(find_group(&[], 7), None);
    let names = [3, 7, 7, 1];
    assert_eq!(find_group(&names, 7), Some(1));
    assert_eq!(find_group(&names, 1), Some(3));
    assert_eq!(find_group(&names, 5), None);
    assert_eq!(find_group(&names, u32::MAX), None);
}

#[test]
fn the_basis_matches_the_formula() {
    let k = constants();
    for seed in 1..24u64 {
        let mut rng = Rng(seed * 0x9e37_79b9);
        let mut q = rng.quat();
        if seed % 4 == 0 {
            // A unit quaternion.
            let norm = q
                .iter()
                .map(|w| f64::from(f32::from_bits(*w)))
                .map(|x| x * x)
                .sum::<f64>()
                .sqrt();
            q = q.map(|w| ((f64::from(f32::from_bits(w)) / norm) as f32).to_bits());
        }
        let mut fp = Fp::new();
        let got = basis(&mut fp, &k, &q);
        assert!(fp.exact(), "seed {seed}");
        let want = reference_basis(&k, &std::array::from_fn(|i| f32::from_bits(q[i])));
        for (g, w) in got.iter().zip(want) {
            assert_eq!(*g, w.to_bits(), "seed {seed}");
        }
    }
}

#[test]
fn a_small_squared_norm_gives_the_identity_basis() {
    let k = constants();
    let mut fp = Fp::new();
    let got = basis(&mut fp, &k, &[1e-3f32.to_bits(); 4]); // norm 4e-6, below the cutoff
    assert!(fp.exact());
    let identity = [
        k.one, 0, 0, //
        0, k.one, 0, //
        0, 0, k.one,
    ];
    assert_eq!(got, identity);
}

#[test]
fn a_denormal_quaternion_declines() {
    let k = constants();
    let mut fp = Fp::new();
    basis(&mut fp, &k, &[1, 1, 1, 1]);
    assert!(!fp.exact());
}

/// One instance from its parts.
fn instance(node: u8, name: u8, out: u8, t: [f32; 3], q: [f32; 4], extra: f32) -> Vec<u8> {
    let mut b = vec![node, name, out, 0];
    for v in t {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for v in q {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&extra.to_le_bytes());
    assert_eq!(b.len(), INSTANCE_BYTES);
    b
}

#[test]
fn the_selection_filters_by_name_and_cuts_at_capacity() {
    // The window: node 2's byte is 9, node 3's is 8; a 0xff node skips
    // the filter whatever its name byte.
    let mut window = [0u8; 256];
    window[128 + 2] = 9;
    window[128 + 3] = 8;
    let instances: Vec<u8> = [
        instance(2, 9, 5, [0.0; 3], [1.0; 4], 0.0),     // matches
        instance(2, 8, 6, [0.0; 3], [1.0; 4], 0.0),     // name differs: skipped
        instance(0xff, 77, 7, [0.0; 3], [1.0; 4], 0.0), // no node: kept
        instance(3, 8, 8, [0.0; 3], [1.0; 4], 0.0),     // matches
    ]
    .concat();
    let sel = select(&instances, Some(&window), 4);
    assert_eq!(
        sel,
        vec![
            Selection { at: 0, node: 5 },
            Selection {
                at: 2 * INSTANCE_BYTES,
                node: 7
            },
            Selection {
                at: 3 * INSTANCE_BYTES,
                node: 8
            },
        ]
    );
    // A full output stops the walk: the last instance is not emitted.
    assert_eq!(select(&instances, Some(&window), 2).len(), 2);
    assert_eq!(select(&instances, Some(&window), 0).len(), 0);
    assert_eq!(select(&instances, Some(&window), -1).len(), 0);
    // No node array: every instance passes.
    assert_eq!(select(&instances, None, 4).len(), 4);
    // A negative node index reads the window's low half.
    let mut low = [0u8; 256];
    low[128 - 2] = 4;
    let sel = select(
        &instance(u8::MAX - 1, 4, 9, [0.0; 3], [1.0; 4], 0.0),
        Some(&low),
        1,
    );
    assert_eq!(sel, vec![Selection { at: 0, node: 9 }]);
}

#[test]
fn the_fill_composes_each_instance_onto_its_node() {
    let k = constants();
    let mut rng = Rng(0x5bd1_e995);
    let mut matrices = Vec::new();
    for _ in 0..4 {
        let mut m = NodeMatrix::default();
        m.scale = rng.value();
        for w in m
            .forward
            .0
            .iter_mut()
            .chain(&mut m.left.0)
            .chain(&mut m.up.0)
            .chain(&mut m.position.0)
        {
            *w = rng.value();
        }
        matrices.push(m);
    }
    let q = rng.quat();
    let t: [f32; 3] = std::array::from_fn(|_| f32::from_bits(rng.value()) * 0.5);
    let instances = instance(
        0xff,
        0,
        2,
        t,
        std::array::from_fn(|i| f32::from_bits(q[i])),
        7.5,
    );
    let sel = select(&instances, None, 4);

    let got = model_markers(&k, &instances, &sel, &matrices, false).unwrap();
    assert_eq!(got.len(), 1);
    let m = &got[0];
    assert_eq!(m.node, 2);
    assert_eq!(m.extra, 7.5f32.to_bits());
    assert_eq!(m.local.scale, k.one);
    assert_eq!(m.local.position, Vec3(t.map(f32::to_bits)));
    // The leaves, called by hand on the same parts.
    let mut fp = Fp::new();
    let b = basis(&mut fp, &k, &q);
    let local = NodeMatrix {
        scale: k.one,
        forward: Vec3([b[0], b[1], b[2]]),
        left: Vec3([b[3], b[4], b[5]]),
        up: Vec3([b[6], b[7], b[8]]),
        position: Vec3(t.map(f32::to_bits)),
    };
    let (world, _) = crate::skeleton::compose(&mut fp, &matrices[2], &local);
    assert_eq!(m.world, world);
    assert!(fp.exact());
}

#[test]
fn a_mirrored_fill_negates_the_world_left_column_only() {
    let k = constants();
    let mut rng = Rng(0x2545_f491);
    let mut matrix = NodeMatrix::default();
    matrix.scale = rng.value();
    for w in matrix
        .forward
        .0
        .iter_mut()
        .chain(&mut matrix.left.0)
        .chain(&mut matrix.up.0)
        .chain(&mut matrix.position.0)
    {
        *w = rng.value();
    }
    let q = rng.quat();
    let instances = instance(
        1,
        0,
        0,
        [0.0; 3],
        std::array::from_fn(|i| f32::from_bits(q[i])),
        0.0,
    );
    let sel = select(&instances, None, 4);

    let plain = model_markers(&k, &instances, &sel, std::slice::from_ref(&matrix), false).unwrap();
    let mirrored =
        model_markers(&k, &instances, &sel, std::slice::from_ref(&matrix), true).unwrap();
    assert_eq!(plain.len(), 1);
    assert_eq!(mirrored.len(), 1);
    let (p, m) = (&plain[0], &mirrored[0]);
    assert_eq!(m.local, p.local);
    assert_eq!(m.world.scale, p.world.scale);
    assert_eq!(m.world.forward, p.world.forward);
    assert_eq!(m.world.up, p.world.up);
    assert_eq!(m.world.position, p.world.position);
    // `0 - x`, which keeps a positive zero positive, not a sign flip.
    for (g, w) in m.world.left.0.iter().zip(p.world.left.0) {
        assert_eq!(*g, (0.0f32 - f32::from_bits(w)).to_bits());
    }
}

#[test]
fn a_node_outside_the_matrices_declines() {
    let k = constants();
    let instances = instance(0xff, 0, 3, [0.0; 3], [1.0; 4], 0.0);
    let sel = select(&instances, None, 4);
    assert_eq!(sel[0].node, 3);
    let short = [NodeMatrix::default(), NodeMatrix::default()];
    assert!(model_markers(&k, &instances, &sel, &short, false).is_err());
}

#[test]
fn the_record_is_the_guests_layout() {
    let _k = constants();
    let mut m = Marker {
        node: 0x1234,
        local: NodeMatrix::default(),
        world: NodeMatrix::default(),
        extra: 0xdead_beef,
    };
    m.local.scale = 0x1111_1111;
    m.world.position = Vec3([0x2222_2222, 0x3333_3333, 0x4444_4444]);
    let b = m.to_bytes();
    assert_eq!(u16::from_le_bytes([b[0], b[1]]), 0x1234);
    assert_eq!(&b[2..4], &[0, 0]);
    assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 0x1111_1111);
    assert_eq!(
        u32::from_le_bytes(b[0x38..0x3c].try_into().unwrap()),
        0 // world scale
    );
    assert_eq!(
        u32::from_le_bytes(b[0x60..0x64].try_into().unwrap()),
        0x2222_2222
    );
    assert_eq!(
        u32::from_le_bytes(b[0x6c..0x70].try_into().unwrap()),
        0xdead_beef
    );
    assert_eq!(b.len(), MARKER_BYTES);
    // And back.
    let back = NodeMatrix::from_bytes(&(b[4..0x38].try_into().unwrap()));
    assert_eq!(back, m.local);
    let back = NodeMatrix::from_bytes(&(b[0x38..0x6c].try_into().unwrap()));
    assert_eq!(back, m.world);
}
