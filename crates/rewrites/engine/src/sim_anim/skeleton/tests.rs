//! The walk against the register-level loop it replaces, on synthetic
//! guest memory (no image). The loop (`sim_physics::node_matrices::nodes`)
//! runs the exact `sim_native` pose and composition bodies, which
//! `tests/sim_physics_differential.rs` and `tests/sim_native_differential.rs`
//! hold equal to the lifted guest; here the kernel must leave the same
//! RAM, byte for byte, and the same CPU state, on both lane paths.
use super::*;
use crate::sim_physics::node_matrices::nodes;
use runtime::guest::Permissions;

const RAM: u32 = 0x0080_0000;
const STACK: u32 = 0x007f_0000;
const DEFINITION: u32 = 0x0060_0000;
const NODE_ARRAY: u32 = 0x0060_1000;
const ORIENTATIONS: u32 = 0x0061_0000;
const MATRICES: u32 = 0x0062_0000;

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
    /// A magnitude in `[0.5, 2)`, positive.
    fn scale(&mut self) -> u32 {
        (126 + (self.next() % 2) as u32) << 23 | (self.next() as u32 & 0x7f_ffff)
    }
}

/// One synthetic skeleton at the loop's entry.
#[derive(Clone)]
struct Case {
    count: u32,
    parents: Vec<i16>,
    keep: Vec<u32>,
    /// `[0x55ebb4]` bit 0 already set.
    initialised: bool,
    mxcsr: u32,
    seed: u64,
}

fn case(count: u32, seed: u64) -> Case {
    let mut rng = Rng(seed);
    // A random tree: each node's parent comes before it, except a few
    // that name a later node or themselves.
    let parents = (0..count)
        .map(|i| match (i, rng.next() % 16) {
            (0, _) => -1,
            (_, 0) => ((i + 1) % count) as i16,
            (_, 1) => i as i16,
            _ => (rng.next() % u64::from(i)) as i16,
        })
        .collect();
    let keep = (0..count.div_ceil(32))
        .map(|_| (rng.next() & rng.next()) as u32)
        .collect();
    Case {
        count,
        parents,
        keep,
        initialised: seed % 2 == 0,
        mxcsr: 0x1fa0,
        seed,
    }
}

fn machine(c: &Case) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    let mut rng = Rng(c.seed ^ 0x5bd1_e995);
    let w = |g: &mut Guest, a: u32, v: u32| g.set_u32(a, v).unwrap();
    w(&mut g, ONE, 1.0f32.to_bits());
    w(&mut g, SMALL, 1e-4f32.to_bits());
    for (i, v) in [0x8000_0000u32, 0, 0x8000_0000, 0].into_iter().enumerate() {
        w(&mut g, SIGNS + 4 * i as u32, v);
    }
    w(&mut g, POSE_INIT_FLAGS, if c.initialised { 0x101 } else { 0x100 });
    w(&mut g, POSE_NORM_THRESHOLD, if c.initialised { 2e-4f32.to_bits() } else { 0 });

    w(&mut g, STACK + DEFINITION_SLOT, DEFINITION);
    w(&mut g, STACK + ORIENTATIONS_SLOT, ORIENTATIONS);
    w(&mut g, STACK + MATRICES_SLOT, MATRICES);
    for (i, &k) in c.keep.iter().enumerate() {
        w(&mut g, STACK + KEEP_SLOT + 4 * i as u32, k);
    }
    w(&mut g, DEFINITION + NODE_COUNT, c.count);
    w(&mut g, DEFINITION + NODES, NODE_ARRAY);
    for i in 0..c.count {
        g.set_u16(NODE_ARRAY + 0x60 * i + PARENT, c.parents[i as usize] as u16).unwrap();
        let record = ORIENTATIONS + 0x20 * i;
        // Every eighth rotation is zero: the identity branch.
        let zero = rng.next() % 8 == 0;
        for k in 0..4 {
            let q = if zero { 0 } else { rng.value() };
            w(&mut g, record + 4 * k, q);
        }
        for k in 4..7 {
            let t = rng.value();
            w(&mut g, record + 4 * k, t);
        }
        let s = rng.scale();
        w(&mut g, record + 28, s);
        let matrix = MATRICES + 0x34 * i;
        // The matrices before the walk: the root, the kept nodes and the
        // parents named ahead of the walk read these.
        let s = rng.scale();
        w(&mut g, matrix, s);
        for k in 1..13 {
            let v = rng.value();
            w(&mut g, matrix + 4 * k, v);
        }
    }
    let mut cpu = CpuAbi {
        eax: rng.next() as u32,
        ecx: rng.next() as u32,
        edx: MATRICES + 0x34,
        ebx: 0x60,
        esp: STACK,
        ebp: ORIENTATIONS + 0x20,
        esi: 1,
        edi: rng.next() as u32,
        eflags: 0x246,
        eflags_defined: 0x8d5,
        mxcsr: c.mxcsr,
        ..CpuAbi::default()
    };
    for reg in cpu.xmm.iter_mut() {
        *reg = unlanes(std::array::from_fn(|_| rng.value()));
    }
    (cpu, g)
}

fn ram(g: &Guest) -> &[u8] {
    let start = g.mappings()[0].physical_start as usize;
    &g.physical_ram()[start..start + RAM as usize]
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

/// The loop and the kernel from the same state; both must agree, and the
/// kernel must not decline.
fn equal(c: &Case) {
    equal_with(c, |_, _| {});
}

/// [`equal`] after `edit` on both states.
fn equal_with(c: &Case, edit: impl Fn(&mut CpuAbi, &mut Guest)) {
    for simd in [true, false] {
        sim_core::lanes::with(simd, || {
            let (mut loop_cpu, mut loop_g) = machine(c);
            edit(&mut loop_cpu, &mut loop_g);
            nodes(&mut loop_cpu, &mut loop_g).unwrap();
            let (mut cpu, mut g) = machine(c);
            edit(&mut cpu, &mut g);
            assert!(walk(&mut cpu, &mut g).unwrap(), "count {} seed {:#x} declined", c.count, c.seed);
            if let Some(at) = first_difference(ram(&loop_g), ram(&g)) {
                panic!("count {} seed {:#x} simd {simd}: RAM differs at {at:#x}", c.count, c.seed);
            }
            // The loop's compares set the arithmetic flags; the epilogue's
            // `add esp` replaces them before anything can read them.
            cpu.eflags = loop_cpu.eflags;
            cpu.eflags_defined = loop_cpu.eflags_defined;
            assert_eq!(cpu, loop_cpu, "count {} seed {:#x} simd {simd}", c.count, c.seed);
        });
    }
}

#[test]
fn the_walk_equals_the_loop() {
    for count in [2u32, 3, 9, 31, 32, 33, 64, 65, 90] {
        for seed in 1..9u64 {
            equal(&case(count, seed * 0x9e37_79b9 + u64::from(count)));
        }
    }
}

#[test]
fn every_node_kept_writes_only_the_exit_registers() {
    let mut c = case(40, 3);
    c.keep = vec![u32::MAX; 2];
    equal(&c);
    let (before, _) = machine(&c);
    let (mut cpu, mut g) = machine(&c);
    assert!(walk(&mut cpu, &mut g).unwrap());
    assert_eq!(cpu.xmm, before.xmm);
}

#[test]
fn an_uninitialised_cutoff_is_set_by_the_walk() {
    let mut c = case(12, 4);
    c.initialised = false;
    c.keep = vec![0];
    equal(&c);
    let (mut cpu, mut g) = machine(&c);
    walk(&mut cpu, &mut g).unwrap();
    assert_eq!(g.u32(POSE_INIT_FLAGS).unwrap(), 0x101);
    assert_eq!(g.u32(POSE_NORM_THRESHOLD).unwrap(), 1e-4f32.to_bits());
}

/// A declined walk writes nothing and leaves the CPU as it was.
fn declines(c: &Case, edit: impl Fn(&mut CpuAbi, &mut Guest), why: Decline) {
    let (mut before_cpu, mut before) = machine(c);
    edit(&mut before_cpu, &mut before);
    let (mut cpu, mut g) = machine(c);
    edit(&mut cpu, &mut g);
    assert_eq!(plan(&cpu, &g).err(), Some(why));
    assert!(!walk(&mut cpu, &mut g).unwrap(), "{why:?}");
    assert_eq!(cpu, before_cpu, "{why:?}");
    assert_eq!(first_difference(ram(&before), ram(&g)), None, "{why:?}");
}

/// The walk runs under the guest's MXCSR, whatever it holds, and leaves
/// the loop's flags: the live game's `0x1f80` (PE clear) comes out with
/// PE set, as the loop's.
#[test]
fn the_walk_equals_the_loop_under_any_masked_mxcsr() {
    // The live MXCSR; rounding toward zero.
    for mxcsr in [0x1f80, 0x7fa0] {
        for seed in 1..4u64 {
            equal(&Case { mxcsr, ..case(33, seed) });
        }
    }
    let c = Case { mxcsr: 0x1f80, keep: vec![0], ..case(10, 5) };
    let (mut cpu, mut g) = machine(&c);
    assert!(walk(&mut cpu, &mut g).unwrap());
    assert_eq!(cpu.mxcsr, 0x1fa0);
    // Operands off the fast range take the SSE model: a denormal in a
    // posed node's quaternion, and an infinity in a dead lane of XMM1,
    // which the first pose adds.
    equal_with(&c, |_, g| g.set_u32(ORIENTATIONS + 0x20 * 3 + 4, 1).unwrap());
    equal_with(&c, |cpu, _| cpu.xmm[1][12..].copy_from_slice(&0x7f80_0000u32.to_le_bytes()));
}

#[test]
fn walks_the_guest_cannot_run_decline_untouched() {
    let mut c = case(10, 5);
    c.keep = vec![0];
    // PE unmasked: the first inexact operation would be `#XM`.
    declines(&Case { mxcsr: 0x0f80, ..c.clone() }, |_, _| {}, Decline::Float);
    // A parent past the matrices, and a negative one.
    for parent in [10u16, 0xffff] {
        declines(&c, move |_, g| g.set_u16(NODE_ARRAY + 0x60 * 4 + PARENT, parent).unwrap(), Decline::Parent);
    }
    // Node counts the walk does not take.
    for count in [1, MAX_NODES + 1] {
        declines(&c, move |_, g| g.set_u32(DEFINITION + NODE_COUNT, count).unwrap(), Decline::Count);
    }
    // The matrices over the node records, and over the dead stack.
    declines(&c, |_, g| g.set_u32(STACK + MATRICES_SLOT, ORIENTATIONS + 0x40).unwrap(), Decline::Alias);
    declines(&c, |_, g| g.set_u32(STACK + MATRICES_SLOT, STACK - 0x40).unwrap(), Decline::Alias);
    // Records past the end of RAM.
    declines(&c, |_, g| g.set_u32(STACK + ORIENTATIONS_SLOT, RAM - 0x20).unwrap(), Decline::Alias);
}
