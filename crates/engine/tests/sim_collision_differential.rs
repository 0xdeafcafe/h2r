//! `engine::sim_collision` against the lifted guest (ladder step 2 of
//! docs/engine/sim-rewrite-collision.md): each entry, and everything under
//! it, runs on the IR interpreter with nothing stubbed. Private XBE as the
//! oracle, synthetic BSPs (`sim_collision::synth`).
//!
//! The adapters replace whole subtrees and keep only the contract's
//! registers (docs/engine/collision-queries.md), so the comparison is
//! final state: guest memory outside each entry's dead stack window, the
//! callee-saved registers, `esp`, the return target, the result register,
//! MXCSR, and the x87 control, tag and status words without the condition
//! codes. A declined call must have written nothing. Each case runs the
//! adapter cold (empty world) and warm (the BSP the cold run decoded),
//! and the per-function ports for points and segments run as a third arm.
#![cfg(feature = "differential")]

mod common;

use engine::sim_collision::{self as adapter, CollisionWorld, layout};
use engine::{points, sim_native as sim};
use ir::interp::{self, Cpu};
use lift::harness::HARNESS_EXIT;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::synth::{self, Builder, Rng};
use sim_core::Epoch;
use std::collections::{BTreeMap, BTreeSet};

const STACK: u32 = 0x007f_f000;
const VECTORS: u32 = 0x0062_0000;
const START: u32 = VECTORS;
const DELTA: u32 = VECTORS + 0x10;
const POINT: u32 = VECTORS + 0x20;
const RECORD: u32 = 0x0063_0000;
const CONTEXT: u32 = 0x0063_1000;
const MASK: u32 = 0x0063_2000;
const LOCATION: u32 = 0x0063_3000;
const STRUCTURE: u32 = 0x0063_4000;
const CLUSTERS: u32 = 0x0063_5000;
const BSP: u32 = 0x0064_0000;

#[derive(Clone)]
struct Ram<'a> {
    image: &'a [u8],
    overlay: BTreeMap<u32, u8>,
}

impl Ram<'_> {
    fn get(&self, p: u32) -> u8 {
        self.overlay
            .get(&p)
            .copied()
            .unwrap_or(self.image[p as usize])
    }
    fn read(&self, a: u32, dst: &mut [u8]) {
        for (i, byte) in dst.iter_mut().enumerate() {
            *byte = self.get(a + i as u32);
        }
    }
    fn store(&mut self, a: u32, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            let p = a + i as u32;
            assert!((p as usize) < self.image.len());
            self.overlay.insert(p, byte);
        }
    }
    fn word(&mut self, a: u32, v: u32) {
        self.store(a, &v.to_le_bytes());
    }
    fn float(&mut self, a: u32, v: f32) {
        self.word(a, v.to_bits());
    }
    /// Addresses either side wrote, outside `[lo, hi)`.
    fn touched<'b>(&'b self, other: &'b Ram<'_>, dead: (u32, u32)) -> BTreeSet<u32> {
        self.overlay
            .keys()
            .chain(other.overlay.keys())
            .copied()
            .filter(|&p| !(dead.0..dead.1).contains(&p))
            .collect()
    }
}

impl sim::Memory for Ram<'_> {
    fn read_bytes(&self, a: u32, dst: &mut [u8], _align: u32) -> Result<(), GuestFault> {
        self.read(a, dst);
        Ok(())
    }
    fn write_bytes(&mut self, a: u32, data: &[u8], _align: u32) -> Result<(), GuestFault> {
        self.store(a, data);
        Ok(())
    }
}

impl interp::Memory for Ram<'_> {
    fn read(&mut self, a: u32, dst: &mut [u8]) -> Result<(), interp::MemFault> {
        Ram::read(self, a, dst);
        Ok(())
    }
    fn write(&mut self, a: u32, data: &[u8]) -> Result<(), interp::MemFault> {
        self.store(a, data);
        Ok(())
    }
}

fn ir_cpu(c: &CpuAbi) -> Cpu {
    Cpu {
        gpr: [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi],
        eip: c.eip,
        fs_base: c.fs_base,
        eflags: c.eflags,
        flags_valid: c.eflags_defined,
        xmm: c.xmm.map(u128::from_le_bytes),
        mxcsr: c.mxcsr,
        x87: interp::X87 {
            st: c.x87.st,
            control: c.x87.control,
            status: c.x87.status,
            tag: c.x87.tag,
            opcode: c.x87.opcode,
            instruction_pointer: c.x87.instruction_pointer,
            data_pointer: c.x87.data_pointer,
            instruction_selector: c.x87.instruction_selector,
            data_selector: c.x87.data_selector,
        },
        ..Cpu::default()
    }
}

/// The x87 status bits outside the declared clobbers: the exception
/// flags, ES and TOP (not C0..C3).
const STATUS_KEPT: u32 = 0x38ff;

/// One entry's comparison scope.
#[derive(Clone, Copy)]
struct Entry {
    point: &'static points::Point,
    /// The bits of `eax` the entry defines.
    eax: u32,
    /// Stack bytes below and above the entry `esp` the original may write.
    dead: (u32, u32),
}

const POINT_TO_LEAF: Entry = Entry {
    point: &points::COLLISION_POINT_TO_LEAF,
    eax: u32::MAX,
    dead: (0x40, 0),
};
const LOCATION_ENTRY: Entry = Entry {
    point: &points::COLLISION_LOCATION,
    eax: 0,
    dead: (0x40, 0),
};
const VECTOR: Entry = Entry {
    point: &points::COLLISION_VECTOR_TEST,
    eax: 0xff,
    dead: (layout::VECTOR_DEAD_BELOW, 4 + layout::VECTOR_ARGS),
};
const CONTEXT_ENTRY: Entry = Entry {
    point: &points::COLLISION_BSP_SEGMENT,
    eax: 0xff,
    dead: (layout::VECTOR_DEAD_BELOW, 4 + layout::VECTOR_ARGS),
};

fn visible(e: &Entry, c: &Cpu) -> [u32; 11] {
    [
        c.gpr[0] & e.eax,
        c.gpr[3],
        c.gpr[4],
        c.gpr[5],
        c.gpr[6],
        c.gpr[7],
        c.eip,
        c.mxcsr,
        u32::from(c.x87.control),
        u32::from(c.x87.status) & STATUS_KEPT,
        u32::from(c.x87.tag),
    ]
}

struct Oracle<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, ir::IrInsn>,
}

impl Oracle<'_> {
    fn run(&mut self, c: &mut Cpu, mem: &mut Ram<'_>, mut pc: u32) {
        for _ in 0..4_000_000 {
            c.eip = pc;
            if pc == HARNESS_EXIT {
                return;
            }
            if !self.cache.contains_key(&pc) {
                let f = self
                    .lifter
                    .lift(pc)
                    .unwrap_or_else(|| panic!("function at {pc:#x}"));
                for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                    self.cache.insert(insn.pc, insn);
                }
            }
            pc = interp::step(c, mem, &self.cache[&pc])
                .unwrap_or_else(|e| panic!("at {pc:#x}: {e:?}"))
                .target();
        }
        panic!("step limit at {pc:#x}");
    }
}

fn with_oracle(name: &str, f: impl FnOnce(&mut Oracle<'_>, &[u8])) {
    let Some(path) = common::xbe_path() else {
        assert!(
            std::env::var_os("H2_XBE").is_none(),
            "H2_XBE does not name a readable file"
        );
        eprintln!("SKIPPED {name}: set H2_XBE");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(common::sha256(&bytes), common::IMAGE_SHA256);
    let xbe = xbe::Xbe::parse(&bytes).unwrap();
    let analysis = analyze::analyze(&xbe).unwrap();
    let lifter = lift::Lifter::new(&analysis, &xbe).unwrap();
    let image = lift::harness::load_image(&xbe).unwrap().bytes;
    let mut oracle = Oracle {
        lifter: &lifter,
        cache: BTreeMap::new(),
    };
    f(&mut oracle, &image);
}

/// The float environments a case may start in: mostly the fast path,
/// then PE not yet sticky, FTZ/DAZ, an unmasked exception and directed
/// rounding (the last three must decline where they matter).
fn entry_cpu(rng: &mut Rng, point: u32) -> CpuAbi {
    let mut c = CpuAbi {
        eax: 0x1234_5678,
        ecx: 0x3456_789a,
        edx: 0x2345_6789,
        ebx: 0x4567_89ab,
        esi: 0x1773_5566,
        edi: 0x6257_1918,
        ebp: STACK - 0x80,
        esp: STACK,
        eip: point,
        eflags: 0x293,
        mxcsr: 0x1fa0,
        ..CpuAbi::default()
    };
    match rng.below(16) {
        0 => c.mxcsr = 0x1f80,
        1 | 2 => c.mxcsr = 0x9fe0,
        3 => c.mxcsr = 0x3fa0,
        _ => {}
    }
    let controls = [
        0x027f, 0x037f, 0x007f, 0x027f, 0x027f, 0x037f, 0x0a7f, 0x027e,
    ];
    c.x87.control = controls[rng.below(8) as usize];
    let top = rng.below(8) as u16;
    c.x87.status = top << 11;
    if rng.chance(16) {
        // A full x87 stack: the original's pushes would overflow.
        c.x87.tag = 0;
    }
    c.x87.st = std::array::from_fn(|i| [(i + 1) as u8; 10]);
    c.xmm = std::array::from_fn(|i| {
        let lane = [0x3f80_1000u32, 0x7f80_2001, 0x8000_0000, 0xffc0_1234][i % 4];
        ((u128::from(lane) << 96) | (u128::from(lane) << 64) | 13).to_le_bytes()
    });
    c
}

type Native =
    fn(&mut CpuAbi, &mut Ram<'_>, &CollisionWorld) -> Result<Option<TransferAbi>, GuestFault>;
type Port = fn(&mut CpuAbi, &mut Ram<'_>) -> sim::Result;

/// Run the adapter on `(c, m)` cold, then warm with the same world, and
/// compare each with the guest; then the per-function port, if any.
/// `false` when the adapter declined (having written nothing).
fn check(
    oracle: &mut Oracle<'_>,
    e: &Entry,
    c: &CpuAbi,
    m: &Ram<'_>,
    native: Native,
    port: Option<Port>,
    case: &str,
) -> bool {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    let dead = (STACK.wrapping_sub(e.dead.0), STACK.wrapping_add(e.dead.1));
    let mut reference: Option<(Cpu, Ram<'_>)> = None;
    let guest = |oracle: &mut Oracle<'_>| {
        let mut gm = m.clone();
        let mut gc = ir_cpu(c);
        oracle.run(&mut gc, &mut gm, e.point.base);
        (gc, gm)
    };
    let compare = |label: &str, got: &Cpu, nm: &Ram<'_>, (gc, gm): &(Cpu, Ram<'_>)| {
        assert_eq!(visible(e, got), visible(e, gc), "{case} {label}: registers");
        for p in nm.touched(gm, dead) {
            assert_eq!(nm.get(p), gm.get(p), "{case} {label}: byte at {p:#x}");
        }
    };
    let mut decodes = 0;
    for label in ["cold", "warm"] {
        let (mut nc, mut nm) = (*c, m.clone());
        let got = native(&mut nc, &mut nm, &world).unwrap();
        let Some(transfer) = got else {
            assert_eq!(
                nm.overlay, m.overlay,
                "{case} {label}: a declined call wrote"
            );
            assert_eq!(nc, *c, "{case} {label}: a declined call moved registers");
            assert_eq!(
                label, "cold",
                "{case}: the warm call declined after the cold call ran"
            );
            return false;
        };
        assert_eq!(transfer.target_pc, HARNESS_EXIT, "{case} {label}");
        let reference = reference.get_or_insert_with(|| guest(oracle));
        let mut got_cpu = ir_cpu(&nc);
        got_cpu.eip = transfer.target_pc;
        compare(label, &got_cpu, &nm, reference);
        let stats = world.stats();
        if label == "warm" {
            assert_eq!(
                stats.decodes, decodes,
                "{case}: the warm call decoded again"
            );
        }
        decodes = stats.decodes;
    }
    if let Some(port) = port {
        let (mut pc, mut pm) = (*c, m.clone());
        let transfer = port(&mut pc, &mut pm).unwrap();
        let mut got = ir_cpu(&pc);
        oracle.run(&mut got, &mut pm, transfer.target_pc);
        compare("port", &got, &pm, reference.as_ref().unwrap());
    }
    true
}

fn write_bsp(m: &mut Ram<'_>, b: &Builder) {
    b.write(BSP, |a, d| m.store(a, d));
}

fn fresh<'a>(image: &'a [u8]) -> Ram<'a> {
    let mut m = Ram {
        image,
        overlay: BTreeMap::new(),
    };
    m.word(STACK, HARNESS_EXIT);
    m
}

/// A coordinate on the eighth grid, sometimes NaN, infinite, tiny or huge.
fn coordinate(rng: &mut Rng, r: u32) -> f32 {
    match rng.below(128) {
        0 => f32::NAN,
        1 => f32::INFINITY,
        2 => 1e-25,
        3 => 3e20,
        _ => rng.eighths(r),
    }
}

fn vector(rng: &mut Rng, r: u32) -> [f32; 3] {
    [coordinate(rng, r), coordinate(rng, r), coordinate(rng, r)]
}

fn put_vector(m: &mut Ram<'_>, at: u32, v: [f32; 3]) {
    for (k, x) in v.into_iter().enumerate() {
        m.float(at + 4 * k as u32, x);
    }
}

fn report(name: &str, native: u32, cases: u32) {
    eprintln!("{name}: {native} native of {cases}");
    // About half the cases start outside the fast path on purpose.
    assert!(
        native * 3 > cases,
        "{name}: the native path ran for {native} of {cases} cases"
    );
}

#[test]
fn point_to_leaf_matches_the_guest() {
    with_oracle(
        "collision point-to-leaf differential test",
        |oracle, image| {
            const CASES: u32 = 800;
            let mut native = 0;
            for n in 0..CASES {
                let mut rng = Rng(0x9e37_79b9_7f4a_7c15
                    ^ u64::from(n).wrapping_mul(0x2545_f491_4f6c_dd1d)
                    | 1);
                let b = synth::random(&mut rng, 1 + n % 7);
                let mut m = fresh(image);
                write_bsp(&mut m, &b);
                let p = vector(&mut rng, 12);
                put_vector(&mut m, POINT, p);
                let mut c = entry_cpu(&mut rng, POINT_TO_LEAF.point.base);
                c.ecx = BSP;
                c.edx = POINT;
                c.eax = match rng.below(6) {
                    0 => synth::leaf(rng.below(4)),
                    1 => rng.below(b.nodes.len() as u32),
                    _ => 0,
                };
                let ok = check(
                    oracle,
                    &POINT_TO_LEAF,
                    &c,
                    &m,
                    |c, g, w| adapter::point_to_leaf(c, g, w),
                    Some(|c, g| sim::point_to_leaf(c, g)),
                    &format!("case {n}"),
                );
                native += u32::from(ok);
            }
            report("point to leaf", native, CASES);
        },
    );
}

#[test]
fn location_matches_the_guest() {
    with_oracle("collision location differential test", |oracle, image| {
        const CASES: u32 = 600;
        let mut native = 0;
        for n in 0..CASES {
            let mut rng =
                Rng(0x2545_f491_4f6c_dd1d ^ u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
            let b = synth::random(&mut rng, 1 + n % 7);
            let mut m = fresh(image);
            write_bsp(&mut m, &b);
            let index: u16 = if rng.chance(8) {
                0xffff
            } else {
                rng.below(3) as u16
            };
            m.store(layout::BSP_INDEX, &index.to_le_bytes());
            m.word(layout::STRUCTURE_BSP, BSP);
            m.word(layout::ACTIVE_STRUCTURE, STRUCTURE);
            m.word(STRUCTURE + layout::STRUCTURE_LEAVES, CLUSTERS);
            for l in 0..b.leaves.len() as u32 {
                let cluster = if rng.chance(6) {
                    0xffff
                } else {
                    rng.below(40) as u16
                };
                m.store(CLUSTERS + 8 * l, &cluster.to_le_bytes());
                m.store(CLUSTERS + 8 * l + 2, &[0xa5; 6]);
            }
            m.store(LOCATION, &[0xcd; 8]);
            put_vector(&mut m, POINT, vector(&mut rng, 12));
            let mut c = entry_cpu(&mut rng, LOCATION_ENTRY.point.base);
            c.edx = POINT;
            c.esi = LOCATION;
            let ok = check(
                oracle,
                &LOCATION_ENTRY,
                &c,
                &m,
                |c, g, w| adapter::location(c, g, w),
                None,
                &format!("case {n}"),
            );
            native += u32::from(ok);
        }
        report("location", native, CASES);
    });
}

/// The record and mask a vector test reads: R's words pre-filled with a
/// pattern so stray writes show, the breakable words random.
fn record_and_mask(m: &mut Ram<'_>, rng: &mut Rng) {
    m.store(RECORD, &vec![0xcd; layout::record::SIZE as usize + 8]);
    for k in 0..8 {
        let w = if rng.chance(2) { u32::MAX } else { rng.next() };
        m.word(MASK + 4 * k, w);
    }
}

fn limit(rng: &mut Rng) -> f32 {
    match rng.below(10) {
        0 => 0.5,
        1 => 2.0,
        2 => -1.0,
        3 => -0.0,
        4 => 0.0,
        5 => rng.eighths(1),
        6 => 0.3,
        _ => 1.0,
    }
}

#[test]
fn vector_test_matches_the_guest() {
    with_oracle(
        "collision vector test differential test",
        |oracle, image| {
            const CASES: u32 = 1200;
            let mut native = 0;
            for n in 0..CASES {
                let mut rng = Rng(0x6a09_e667_f3bc_c908
                    ^ u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                    | 1);
                let b = synth::random(&mut rng, 1 + n % 7);
                let mut m = fresh(image);
                write_bsp(&mut m, &b);
                record_and_mask(&mut m, &mut rng);
                put_vector(&mut m, START, vector(&mut rng, 12));
                put_vector(&mut m, DELTA, vector(&mut rng, 24));
                let mut c = entry_cpu(&mut rng, VECTOR.point.base);
                c.eax = RECORD;
                c.ecx = rng.below(32) | (rng.next() & 0xffff_ff00);
                c.edx = BSP;
                let mut x0 = c.xmm[0];
                x0[..4].copy_from_slice(&limit(&mut rng).to_bits().to_le_bytes());
                c.xmm[0] = x0;
                let count = [0x100u32, 8, 0, 3][rng.below(4) as usize] | (rng.next() << 16);
                m.word(STACK + 4, count);
                m.word(STACK + 8, if rng.chance(2) { MASK } else { 0 });
                m.word(STACK + 12, START);
                m.word(STACK + 16, DELTA);
                let ok = check(
                    oracle,
                    &VECTOR,
                    &c,
                    &m,
                    |c, g, w| adapter::vector_test(c, g, w),
                    None,
                    &format!("case {n}"),
                );
                native += u32::from(ok);
            }
            report("vector test", native, CASES);
        },
    );
}

#[test]
fn vector_context_matches_the_guest() {
    with_oracle(
        "collision vector context differential test",
        |oracle, image| {
            use layout::context as ctx;
            const CASES: u32 = 1200;
            let mut native = 0;
            for n in 0..CASES {
                let mut rng = Rng(0xbb67_ae85_84ca_a73b
                    ^ u64::from(n).wrapping_mul(0x2545_f491_4f6c_dd1d)
                    | 1);
                let b = synth::random(&mut rng, 1 + n % 7);
                let mut m = fresh(image);
                write_bsp(&mut m, &b);
                record_and_mask(&mut m, &mut rng);
                put_vector(&mut m, START, vector(&mut rng, 12));
                put_vector(&mut m, DELTA, vector(&mut rng, 24));
                let fraction = [1.0f32, 0.5, 0.75, 0.0][rng.below(4) as usize];
                m.float(RECORD + layout::record::FRACTION, fraction);
                let listed = [0u32, 5, 0xff, 0x100, 0x150][rng.below(5) as usize];
                m.word(RECORD + layout::record::COUNT, listed);
                m.word(
                    CONTEXT + ctx::FLAGS,
                    rng.below(32) | (rng.next() & 0xffff_ff00),
                );
                m.word(CONTEXT + ctx::BSP, BSP);
                m.word(
                    CONTEXT + ctx::BREAKABLE_COUNT,
                    [0x100u32, 8][rng.below(2) as usize] | (rng.next() << 16),
                );
                m.word(CONTEXT + ctx::MASK, if rng.chance(2) { MASK } else { 0 });
                m.word(CONTEXT + ctx::START, START);
                m.word(CONTEXT + ctx::DELTA, DELTA);
                m.word(CONTEXT + ctx::RECORD, RECORD);
                let leaves = b.leaves.len() as u32;
                let prev_leaf = if rng.chance(2) {
                    u32::MAX
                } else {
                    rng.below(leaves)
                };
                m.word(CONTEXT + ctx::PREV_LEAF, prev_leaf);
                m.word(
                    CONTEXT + ctx::PREV_KIND,
                    rng.below(4) | (rng.next() & 0xffff_ff00),
                );
                let plane = if rng.chance(2) {
                    u32::MAX
                } else {
                    rng.below(b.planes.len() as u32)
                };
                m.word(CONTEXT + ctx::PLANE, plane);
                let node = match rng.below(6) {
                    0 => synth::leaf(rng.below(leaves.max(1))),
                    1 => u32::MAX,
                    2 => rng.below(b.nodes.len() as u32),
                    _ => 0,
                };
                let (t0, t1) =
                    [(0.0f32, 1.0f32), (0.25, 0.75), (0.0, 0.5), (0.5, 0.5)][rng.below(4) as usize];
                m.word(STACK + 4, CONTEXT);
                m.word(STACK + 8, node);
                m.float(STACK + 12, t0);
                m.float(STACK + 16, t1);
                let c = entry_cpu(&mut rng, CONTEXT_ENTRY.point.base);
                let ok = check(
                    oracle,
                    &CONTEXT_ENTRY,
                    &c,
                    &m,
                    |c, g, w| adapter::vector_context(c, g, w),
                    Some(|c, g| sim::bsp::segment(c, g)),
                    &format!("case {n}"),
                );
                native += u32::from(ok);
            }
            report("vector context", native, CASES);
        },
    );
}

/// Ties and edges on a fixed wall: segments that end exactly on the plane,
/// start on it, split exactly at the limit, and hit with every query bit.
#[test]
fn vector_test_ties_match_the_guest() {
    with_oracle("collision vector test ties", |oracle, image| {
        let mut b = Builder::default();
        b.planes.push([1.0, 0.0, 0.0, 0.0]);
        b.planes.push([0.0, 0.0, 1.0, 2.0]);
        b.nodes.push((0, synth::node(1), synth::SOLID));
        b.nodes.push((1, synth::leaf(0), synth::leaf(1)));
        b.leaves.push((0, 1, 0));
        b.leaves.push((1, 2, 1));
        b.refs.push((0, synth::surface(0)));
        b.refs.push((0x8000, synth::surface(1)));
        b.refs.push((1, 0));
        b.nodes_2d
            .push(([1.0, 0.0, 0.0], [synth::surface(0), synth::surface(1)]));
        let square = [
            [0.0, -4.0, -4.0],
            [0.0, 4.0, -4.0],
            [0.0, 4.0, 4.0],
            [0.0, -4.0, 4.0],
        ];
        let e0 = b.ring(0, &square, false);
        let e1 = b.ring(1, &square, true);
        b.surfaces.push((0, e0, 0, 0, 1));
        b.surfaces.push((0x8000, e1, 8, 2, 2));
        let starts = [-2.0f32, 0.0, -0.0, 2.0, -1.0];
        let deltas = [4.0f32, 2.0, -2.0, 1.0, 3.0, 0.0];
        let mut n = 0;
        for &sx in &starts {
            for &dx in &deltas {
                for flags in [0u32, 1, 2, 3, 4, 5, 9, 0x11] {
                    for &lim in &[1.0f32, 0.5, 2.0] {
                        let mut rng = Rng(u64::from(n) | 1);
                        let mut m = fresh(image);
                        write_bsp(&mut m, &b);
                        record_and_mask(&mut m, &mut rng);
                        put_vector(&mut m, START, [sx, 0.0, 0.0]);
                        put_vector(&mut m, DELTA, [dx, 0.0, dx]);
                        let mut c = entry_cpu(&mut Rng(3), VECTOR.point.base);
                        (c.mxcsr, c.x87.control, c.x87.tag) = (0x1fa0, 0x027f, 0xffff);
                        c.eax = RECORD;
                        c.ecx = flags;
                        c.edx = BSP;
                        let mut x0 = c.xmm[0];
                        x0[..4].copy_from_slice(&lim.to_bits().to_le_bytes());
                        c.xmm[0] = x0;
                        m.word(STACK + 4, 0x100);
                        m.word(STACK + 8, MASK);
                        m.word(STACK + 12, START);
                        m.word(STACK + 16, DELTA);
                        let label = format!("start {sx} delta {dx} flags {flags:#x} limit {lim}");
                        assert!(
                            check(
                                oracle,
                                &VECTOR,
                                &c,
                                &m,
                                |c, g, w| adapter::vector_test(c, g, w),
                                None,
                                &label
                            ),
                            "{label}: declined"
                        );
                        n += 1;
                    }
                }
            }
        }
    });
}
