//! `engine::sim_collision` against the lifted guest (ladder step 2 of
//! docs/engine/sim-rewrite-collision.md): each entry, and everything under
//! it, runs on the IR interpreter with nothing stubbed. Private XBE as the
//! oracle, synthetic BSPs (`sim_collision::synth`).
//!
//! A and B compare the entire CPU and memory including their saved stack
//! words. C and D compare outside the vector clobbers and dead stack slots,
//! including the return address, EFLAGS and live x87 registers; their
//! crossing exits compare every register and stack byte. F
//! (`mopp_box`) compares outside `eax`, `ecx`, `edx`, the x87 condition
//! codes and its dead stack. A decline
//! must preserve every guest register and byte. Each case takes the adapter
//! cold and warm, and compares the older per-function port where present.
//! G and H compare like C and D, with their own dead stack; their third arm
//! runs the lifted entry with one `sim_physics` port in place (the portal
//! test under G, the visit itself for H). I (`mopp_ray`) runs the guest
//! with the parity arm's observers and invented callbacks; the arm must
//! match or say why it could not, and the collection must get the keys
//! the native walk gives.
#![cfg(feature = "differential")]

mod common;
#[path = "sim_collision_differential/crossing.rs"]
mod crossing;
#[path = "sim_collision_differential/disc.rs"]
pub(crate) mod disc;
#[path = "sim_collision_differential/mopp_box.rs"]
mod mopp_box;
#[path = "sim_collision_differential/mopp_ray.rs"]
mod mopp_ray;
#[path = "sim_collision_differential/instance.rs"]
mod instance;
#[path = "sim_collision_differential/model.rs"]
mod model;
#[path = "sim_collision_differential/query.rs"]
mod query;
#[path = "sim_collision_differential/spatial.rs"]
mod spatial;
#[path = "sim_collision_differential/straight.rs"]
mod straight;

use engine::sim_collision::{self as adapter, CollisionWorld, layout};
use engine::{points, sim_native as sim};
use ir::interp::{self, Cpu};
use lift::harness::HARNESS_EXIT;
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi, guest::{Access, FaultReason, Permissions}};
use sim_collision::synth::{self, Builder, Rng};
use sim_collision::structure::{LeafTable, TableVersion};
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
const OUT: u32 = 0x0063_6000;
const CENTRE: u32 = VECTORS + 0x30;
const TOPOLOGY: u32 = 0x0066_0000;
/// A second collision BSP header for `[0x4e033c]`, with its own planes.
const OTHER_BSP: u32 = 0x0066_f000;
/// Per list: the head table, the datum array header and the records.
const REFERENCES: [(u32, u32, u32); 2] = [
    (0x0067_0000, 0x0067_0100, 0x0067_0200),
    (0x0067_1000, 0x0067_1100, 0x0067_1200),
];

#[derive(Clone)]
struct Ram<'a> {
    image: &'a [u8],
    /// A disc tag, borrowed at its original guest address. The sweep gives
    /// the interpreter and adapter this same immutable mapping.
    tag: Option<(u32, &'a [u8], TableVersion)>,
    overlay: BTreeMap<u32, u8>,
}

impl Ram<'_> {
    fn get(&self, p: u32) -> u8 {
        self.overlay
            .get(&p)
            .copied()
            .unwrap_or_else(|| self.image[p as usize])
    }
    fn read(&self, a: u32, dst: &mut [u8]) -> Result<(), GuestFault> {
        if let Some((base, bytes, _)) = self.tag
            && let Some(offset) = a.checked_sub(base)
            && let Some(tail) = bytes.get(offset as usize..)
            && let Some(source) = tail.get(..dst.len())
        {
            dst.copy_from_slice(source);
            return Ok(());
        }
        if !dst.is_empty() && u64::from(a) + dst.len() as u64 > self.image.len() as u64 {
            return Err(GuestFault {
                addr: a, len: dst.len(), access: Access::Read, reason: FaultReason::Unmapped,
            });
        }
        for (i, byte) in dst.iter_mut().enumerate() {
            *byte = self.get(a + i as u32);
        }
        Ok(())
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
    /// Addresses either side wrote, including the return address even when
    /// the entry uses both its own frame and its argument slots as scratch.
    fn touched<'b>(&'b self, other: &'b Ram<'_>, sp: u32, dead: (u32, u32)) -> BTreeSet<u32> {
        self.overlay
            .keys()
            .chain(other.overlay.keys())
            .copied()
            .filter(|&p| p.wrapping_sub(sp) < 4 || !(dead.0..dead.1).contains(&p))
            .collect()
    }
}

impl sim::Memory for Ram<'_> {
    fn read_bytes(&self, a: u32, dst: &mut [u8], _align: u32) -> Result<(), GuestFault> {
        self.read(a, dst)
    }
    fn write_bytes(&mut self, a: u32, data: &[u8], _align: u32) -> Result<(), GuestFault> {
        if !data.is_empty() && u64::from(a) + data.len() as u64 > self.image.len() as u64 {
            return Err(GuestFault {
                addr: a, len: data.len(), access: Access::Write, reason: FaultReason::Protection,
            });
        }
        self.store(a, data);
        Ok(())
    }
}

impl adapter::CollisionMemory for Ram<'_> {
    fn leaf_table(&mut self, count_addr: u32, leaves: u32, leaf: u32) -> Option<LeafTable> {
        let (base, bytes, version) = self.tag?;
        let offset = count_addr.checked_sub(base)? as usize;
        let count = u32::from_le_bytes(bytes.get(offset..)?.get(..4)?.try_into().ok()?);
        LeafTable::witnessed(leaves, count, leaf, |len| {
            let offset = leaves.checked_sub(base)? as usize;
            bytes.get(offset..)?.get(..len as usize)?;
            Some(version)
        })
    }

    fn physical_range(&self, addr: u32, len: u32) -> Option<u32> {
        let end = u64::from(addr) + u64::from(len);
        let in_tag = self.tag.is_some_and(|(base, bytes, _)| {
            addr >= base && end <= u64::from(base) + bytes.len() as u64
        });
        (end <= self.image.len() as u64 || in_tag).then_some(addr)
    }

    fn writable(&self, addr: u32, len: u32) -> bool {
        let end = u64::from(addr) + u64::from(len);
        end <= self.image.len() as u64 && !self.tag.is_some_and(|(base, bytes, _)| {
            u64::from(addr) < u64::from(base) + bytes.len() as u64 && u64::from(base) < end
        })
    }
}

impl interp::Memory for Ram<'_> {
    fn read(&mut self, a: u32, dst: &mut [u8]) -> Result<(), interp::MemFault> {
        Ram::read(self, a, dst).map_err(|f| interp::MemFault { addr: f.addr, write: false })
    }
    fn write(&mut self, a: u32, data: &[u8]) -> Result<(), interp::MemFault> {
        sim::Memory::write_bytes(self, a, data, 1)
            .map_err(|f| interp::MemFault { addr: f.addr, write: true })
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
const STATUS_KEPT: u32 = 0xb8ff;

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
    dead: (0, 0),
};
const LOCATION_ENTRY: Entry = Entry {
    point: &points::COLLISION_LOCATION,
    eax: u32::MAX,
    dead: (0, 0),
};
const VECTOR: Entry = Entry {
    point: &points::COLLISION_VECTOR_TEST,
    eax: 0xff,
    dead: (layout::VECTOR_DEAD_BELOW, 8),
};
const CONTEXT_ENTRY: Entry = Entry {
    point: &points::COLLISION_BSP_SEGMENT,
    eax: 0xff,
    dead: (layout::CONTEXT_DEAD_BELOW, 4 + layout::VECTOR_ARGS),
};

const FLOOD: Entry = Entry {
    point: &points::COLLISION_CLUSTER_FLOOD,
    eax: u32::MAX,
    dead: (layout::FLOOD_DEAD_BELOW, 4 + layout::FLOOD_ARGS),
};
const VISIT: Entry = Entry {
    point: &points::OBJECT_SPATIAL_VISIT,
    eax: u32::MAX,
    dead: (layout::VISIT_DEAD_BELOW, 4 + layout::VISIT_ARGS),
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
    fn prime(&mut self, pc: u32) {
        if !self.cache.contains_key(&pc) {
            let f = self
                .lifter
                .lift(pc)
                .unwrap_or_else(|| panic!("function at {pc:#x}"));
            for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                self.cache.insert(insn.pc, insn);
            }
        }
    }

    fn run(&mut self, c: &mut Cpu, mem: &mut impl interp::Memory, mut pc: u32) {
        for _ in 0..4_000_000 {
            c.eip = pc;
            if pc == HARNESS_EXIT {
                return;
            }
            self.prime(pc);
            pc = interp::step(c, mem, &self.cache[&pc])
                .unwrap_or_else(|e| panic!("at {pc:#x}: {e:?}"))
                .target();
        }
        panic!("step limit at {pc:#x}");
    }

    /// [`Oracle::run`] with `port` taking its entry and its return sites.
    /// `false` when the port does not support the state it is entered in.
    fn run_with_port(
        &mut self,
        c: &mut Cpu,
        mem: &mut GuestRam,
        mut pc: u32,
        port: &engine::sim_physics::Port,
    ) -> bool {
        for _ in 0..4_000_000 {
            c.eip = pc;
            if pc == HARNESS_EXIT {
                return true;
            }
            if pc == port.point.base || port.resumes.contains(&pc) {
                let mut abi = lifted::cpu::from_interp(c);
                if pc == port.point.base && !engine::sim_physics::supported(port, &abi, &mem.0) {
                    return false;
                }
                let transfer = port
                    .invoke(&mut abi, &mut mem.0, pc)
                    .unwrap_or_else(|f| panic!("port at {pc:#x}: {f:?}"));
                *c = lifted::cpu::to_interp(&abi);
                pc = transfer.target_pc;
                continue;
            }
            self.prime(pc);
            pc = interp::step(c, mem, &self.cache[&pc])
                .unwrap_or_else(|e| panic!("at {pc:#x}: {e:?}"))
                .target();
        }
        panic!("step limit at {pc:#x}");
    }
}

/// A whole guest for the port arm: the ports take a `Guest`.
struct GuestRam(Guest);

impl interp::Memory for GuestRam {
    fn read(&mut self, a: u32, dst: &mut [u8]) -> Result<(), interp::MemFault> {
        sim::Memory::read_bytes(&self.0, a, dst, 1)
            .map_err(|f| interp::MemFault { addr: f.addr, write: false })
    }
    fn write(&mut self, a: u32, data: &[u8]) -> Result<(), interp::MemFault> {
        sim::Memory::write_bytes(&mut self.0, a, data, 1)
            .map_err(|f| interp::MemFault { addr: f.addr, write: true })
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
        // The live game's MXCSR, PE clear: the kernels answer it and
        // raise PE as the guest does.
        0 | 4 | 5 => c.mxcsr = 0x1f80,
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
    } else {
        // A value below ST(0) must survive temporary loads into the two
        // free physical slots above it, including when TOP wraps.
        c.x87.tag &= !(3 << (2 * top));
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
    let dead = (c.esp.wrapping_sub(e.dead.0), c.esp.wrapping_add(e.dead.1));
    let mut reference: Option<(Cpu, Ram<'_>)> = None;
    let guest = |oracle: &mut Oracle<'_>| {
        let mut gm = m.clone();
        let mut gc = ir_cpu(c);
        oracle.run(&mut gc, &mut gm, e.point.base);
        (gc, gm)
    };
    let compare = |label: &str, got: &Cpu, nm: &Ram<'_>, (gc, gm): &(Cpu, Ram<'_>)| {
        if e.dead == (0, 0) {
            assert_eq!(got, gc, "{case} {label}: CPU");
        } else {
            assert_eq!(visible(e, got), visible(e, gc), "{case} {label}: registers");
            assert_eq!(got.eflags, gc.eflags, "{case} {label}: EFLAGS");
            assert_eq!(got.flags_valid, gc.flags_valid, "{case} {label}: defined flags");
            assert_eq!(got.fs_base, gc.fs_base, "{case} {label}: FS");
            for (slot, value) in gc.x87.st.iter().enumerate() {
                if gc.x87.tag >> (2 * slot) & 3 != 3 {
                    assert_eq!(&got.x87.st[slot], value, "{case} {label}: x87 slot {slot}");
                }
            }
        }
        for p in nm.touched(gm, c.esp, dead) {
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
        if e.point.base == VECTOR.point.base || e.point.base == CONTEXT_ENTRY.point.base {
            let mut entry = m.clone();
            let p = if e.point.base == CONTEXT_ENTRY.point.base {
                adapter::vector_parity::Prediction::gather_context(c, &mut entry, &world)
            } else {
                adapter::vector_parity::Prediction::gather(c, &mut entry, &world)
            }.unwrap();
            assert_eq!(entry.overlay, m.overlay, "{case}: parity gather wrote memory");
            assert_eq!(p.compare(reference.0.gpr[0] as u8, &reference.1).unwrap(), None, "{case} {label}");
            let guest = lifted::cpu::from_interp(&reference.0);
            assert_eq!(
                p.compare_exit(&guest, &reference.1).unwrap(),
                None,
                "{case} {label}: crossing exit"
            );
        }
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
        tag: None,
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
        let mut n = 0u32;
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

#[test]
fn vector_test_ring_groups_match_the_guest() {
    with_oracle("collision vector test ring groups", |oracle, image| {
        let polygon = [
            [0.0, -4.0, -4.0],
            [0.0, 0.0, -4.0],
            [0.0, 4.0, -4.0],
            [0.0, 4.0, 0.0],
            [0.0, 4.0, 4.0],
            [0.0, 0.0, 4.0],
            [0.0, -4.0, 4.0],
            [0.0, -4.0, 0.0],
            [0.0, -4.0, -2.0],
        ];
        for len in [3, 5, 8, 9] {
            for flip in [0, 0x8000] {
                let mut b = Builder::default();
                b.planes.push([1.0, 0.0, 0.0, 0.0]);
                b.nodes.push((0, synth::leaf(0), synth::leaf(1)));
                b.leaves.extend([(1, 1, 0), (1, 1, 0)]);
                b.refs.push((flip, synth::surface(0)));
                let points: Vec<_> =
                    (0..len).map(|i| polygon[i * polygon.len() / len]).collect();
                let first = b.ring(0, &points, flip != 0);
                b.surfaces.push((flip, first, 0, 0, 7));

                for (y, z) in [(0.0, -0.0), (-4.0, -4.0), (8.0, 8.0), (-8.0, 0.0)] {
                    let mut m = fresh(image);
                    write_bsp(&mut m, &b);
                    record_and_mask(&mut m, &mut Rng(17));
                    put_vector(&mut m, START, [-2.0, y, z]);
                    put_vector(&mut m, DELTA, [4.0, 0.0, 0.0]);
                    m.word(STACK + 4, 0);
                    m.word(STACK + 8, 0);
                    m.word(STACK + 12, START);
                    m.word(STACK + 16, DELTA);
                    let mut c = entry_cpu(&mut Rng(3), VECTOR.point.base);
                    (c.mxcsr, c.x87.control, c.x87.tag) = (0x1fa0, 0x027f, 0xffff);
                    c.eax = RECORD;
                    c.ecx = 0;
                    c.edx = BSP;
                    c.xmm[0][..4].copy_from_slice(&1.0f32.to_le_bytes());

                    for lanes in [false, true] {
                        let label = format!("{len} edges, flip {flip:#x}, ({y}, {z}), lanes {lanes}");
                        assert!(
                            sim_core::lanes::with(lanes, || check(
                                oracle,
                                &VECTOR,
                                &c,
                                &m,
                                |c, g, w| adapter::vector_test(c, g, w),
                                None,
                                &label,
                            )),
                            "{label}: declined"
                        );
                    }
                }
            }
        }
    });
}

/// The third arm for G and H: the lifted entry with `port` in place, from
/// the same state, against the lifted entry alone.
fn port_matches(
    oracle: &mut Oracle<'_>,
    e: &Entry,
    c: &CpuAbi,
    m: &Ram<'_>,
    port: &engine::sim_physics::Port,
    case: &str,
) -> bool {
    let dead = (c.esp.wrapping_sub(e.dead.0), c.esp.wrapping_add(e.dead.1));
    let (mut gc, mut gm) = (ir_cpu(c), m.clone());
    oracle.run(&mut gc, &mut gm, e.point.base);
    let len = (m.image.len() as u32 + 0xfff) & !0xfff;
    let mut g = Guest::new();
    g.map_zeroed(0, len, Permissions::RW).unwrap();
    g.write_bytes(0, m.image, 1).unwrap();
    for (&at, &byte) in &m.overlay {
        g.write_bytes(at, &[byte], 1).unwrap();
    }
    let mut mixed = GuestRam(g);
    let mut got = lifted::cpu::to_interp(c);
    if !oracle.run_with_port(&mut got, &mut mixed, e.point.base, port) {
        return false;
    }
    assert_eq!(visible(e, &got), visible(e, &gc), "{case} port: registers");
    assert_eq!(got.eflags, gc.eflags, "{case} port: EFLAGS");
    for (slot, value) in gc.x87.st.iter().enumerate() {
        if gc.x87.tag >> (2 * slot) & 3 != 3 {
            assert_eq!(&got.x87.st[slot], value, "{case} port: x87 slot {slot}");
        }
    }
    let start = mixed.0.mappings()[0].physical_start as usize;
    let ram = &mixed.0.physical_ram()[start..start + m.image.len()];
    let mut at: BTreeSet<u32> = gm.overlay.keys().copied().collect();
    at.extend(
        ram.iter()
            .zip(m.image)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i as u32),
    );
    for p in at {
        if p.wrapping_sub(c.esp) < 4 || !(dead.0..dead.1).contains(&p) {
            assert_eq!(ram[p as usize], gm.get(p), "{case} port: byte at {p:#x}");
        }
    }
    true
}

fn physics_port(point: &points::Point) -> &'static engine::sim_physics::Port {
    engine::sim_physics::ORACLES
        .iter()
        .find(|p| p.point.base == point.base)
        .unwrap()
}

/// A random portal graph at [`TOPOLOGY`], named by both structure globals.
/// Now and then `[0x4e033c]` names another BSP whose normals point the
/// other way, so the projection row changes while the distance does not.
fn write_topology(m: &mut Ram<'_>, rng: &mut Rng, clusters: u32) -> synth::Portals {
    let p = synth::random_portals(rng, clusters);
    let (end, bsp) = p.write(TOPOLOGY, |a, d| m.store(a, d));
    assert!(end < OTHER_BSP);
    m.word(layout::ACTIVE_STRUCTURE, TOPOLOGY);
    if rng.chance(6) {
        let planes = OTHER_BSP + 0x100;
        m.store(OTHER_BSP, &[0; 0x40]);
        m.word(OTHER_BSP + 8, p.planes.len() as u32);
        m.word(OTHER_BSP + 0xc, planes);
        for (i, plane) in p.planes.iter().enumerate() {
            for k in 0..4 {
                let v = if k < 3 { -plane[k] } else { plane[k] };
                m.float(planes + 16 * i as u32 + 4 * k as u32, v);
            }
        }
        m.word(layout::STRUCTURE_BSP, OTHER_BSP);
    } else {
        m.word(layout::STRUCTURE_BSP, bsp);
    }
    p
}

/// Mostly the environment the flood's x87 sequences run in, now and then
/// extended precision. Half the calls enter with the x87 precision flag
/// clear, as the live game does: the flood sets it as the guest would.
fn flood_cpu(rng: &mut Rng, point: u32) -> CpuAbi {
    let mut c = entry_cpu(rng, point);
    c.x87.control = [0x027f, 0x007f, 0x027f, 0x027f, 0x037f][rng.below(5) as usize];
    if rng.chance(2) {
        c.x87.status |= 0x20;
    }
    c
}

#[test]
fn cluster_flood_matches_the_guest() {
    with_oracle("collision cluster flood differential test", |oracle, image| {
        const CASES: u32 = 800;
        let port = physics_port(&points::OBJECT_SPATIAL_PORTAL);
        let (mut native, mut ported) = (0, 0);
        for n in 0..CASES {
            let mut rng = Rng(0x3c6e_f372_fe94_f82b
                ^ u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                | 1);
            let mut m = fresh(image);
            let clusters = 2 + rng.below(9);
            write_topology(&mut m, &mut rng, clusters);
            let epoch = rng.next();
            m.word(layout::FLOOD_EPOCH, epoch);
            for k in 0..clusters {
                let mark = if rng.chance(8) { epoch } else { epoch ^ (1 + k) };
                m.word(layout::CLUSTER_MARKS + 4 * k, mark);
            }
            m.store(OUT, &[0xcd; 48]);
            put_vector(
                &mut m,
                CENTRE,
                [rng.eighths(10), rng.eighths(4), rng.eighths(4)],
            );
            let radius = match rng.below(16) {
                0 => -1.0,
                1 => 0.0,
                2 => f32::from_bits(1),
                _ => [0.5f32, 1.0, 2.5, 6.0, 40.0][rng.below(5) as usize],
            };
            let capacity = [16u32, 3, 1, 0, 16, 16, 0x8000][rng.below(7) as usize];
            let mut c = flood_cpu(&mut rng, FLOOD.point.base);
            m.word(STACK + 4, rng.below(clusters) | (rng.next() << 16));
            m.word(STACK + 8, CENTRE);
            m.float(STACK + 12, radius);
            m.word(STACK + 16, capacity);
            m.word(STACK + 20, OUT);
            c.eax = rng.next();
            let case = format!("case {n}");
            let ok = check(
                oracle,
                &FLOOD,
                &c,
                &m,
                |c, g, w| adapter::cluster_flood(c, g, w),
                None,
                &case,
            );
            if ok && n % 4 == 0 {
                ported += u32::from(port_matches(oracle, &FLOOD, &c, &m, port, &case));
            }
            native += u32::from(ok);
        }
        eprintln!("cluster flood: the portal port ran in {ported} cases");
        report("cluster flood", native, CASES);
    });
}

/// Random references for each cluster of each list: a few slots shared
/// across clusters and lists, all types, spheres near the query.
fn write_references(m: &mut Ram<'_>, rng: &mut Rng, clusters: u32) {
    let globals = [
        (layout::COLLIDEABLE_HEADS, layout::COLLIDEABLE_ARRAY),
        (layout::OTHER_HEADS, layout::OTHER_ARRAY),
    ];
    for (k, &(heads, array, data)) in REFERENCES.iter().enumerate() {
        let list = synth::References {
            clusters: (0..clusters)
                .map(|_| {
                    (0..rng.below(4))
                        .map(|_| synth::Reference {
                            handle: (rng.next() << 16) | rng.below(24),
                            kind: rng.below(40) as u8,
                            bounds: [
                                rng.eighths(10),
                                rng.eighths(4),
                                rng.eighths(4),
                                rng.eighths(2).abs(),
                            ],
                        })
                        .collect()
                })
                .collect(),
        };
        list.write(heads, array, data, |a, d| m.store(a, d));
        m.word(globals[k].0, heads);
        m.word(globals[k].1, array);
    }
}

#[test]
fn object_visit_matches_the_guest() {
    with_oracle("collision object visit differential test", |oracle, image| {
        const CASES: u32 = 800;
        let port = physics_port(&points::OBJECT_SPATIAL_VISIT);
        let (mut native, mut ported) = (0, 0);
        for n in 0..CASES {
            let mut rng = Rng(0xa54f_f53a_5f1d_36f1
                ^ u64::from(n).wrapping_mul(0x2545_f491_4f6c_dd1d)
                | 1);
            let mut m = fresh(image);
            let clusters = 2 + rng.below(9);
            write_topology(&mut m, &mut rng, clusters);
            write_references(&mut m, &mut rng, clusters);
            let (epoch, generation) = (rng.next(), rng.next());
            m.word(layout::FLOOD_EPOCH, epoch);
            m.word(layout::VISIT_GENERATION, generation);
            for k in 0..clusters {
                let mark = if rng.chance(10) { epoch.wrapping_add(1) } else { epoch };
                m.word(layout::CLUSTER_MARKS + 4 * k, mark);
            }
            for slot in 0..24 {
                let mark = if rng.chance(8) { generation.wrapping_add(1) } else { generation };
                m.word(layout::OBJECT_MARKS + 4 * slot, mark);
            }
            m.store(OUT, &[0xcd; 48]);
            let cluster = if rng.chance(8) { 0xffff } else { rng.below(clusters) };
            m.word(LOCATION, rng.next());
            m.word(LOCATION + 4, cluster | (rng.next() << 16));
            put_vector(
                &mut m,
                CENTRE,
                [rng.eighths(10), rng.eighths(4), rng.eighths(4)],
            );
            let radius = match rng.below(8) {
                0 => -1.0,
                1 => 0.0,
                2 => -0.0,
                _ => [0.5f32, 1.0, 2.5, 6.0, 40.0][rng.below(5) as usize],
            };
            let lists = [0u32, 1, 2, 3, 0xffff_fffc][rng.below(5) as usize];
            let types = match rng.below(4) {
                0 => 0,
                1 => u32::MAX,
                _ => rng.next(),
            };
            let capacity = [8u32, 2, 1, 0, 0xffff][rng.below(5) as usize] | (rng.next() << 16);
            let mut c = flood_cpu(&mut rng, VISIT.point.base);
            for (k, arg) in [lists, types, LOCATION, CENTRE, radius.to_bits(), OUT, capacity]
                .into_iter()
                .enumerate()
            {
                m.word(STACK + 4 + 4 * k as u32, arg);
            }
            c.eax = rng.next();
            let case = format!("case {n}");
            let ok = check(
                oracle,
                &VISIT,
                &c,
                &m,
                |c, g, w| adapter::object_spatial_visit(c, g, w),
                None,
                &case,
            );
            if ok && n % 4 == 0 {
                ported += u32::from(port_matches(oracle, &VISIT, &c, &m, port, &case));
            }
            native += u32::from(ok);
        }
        eprintln!("object visit: the visit port ran in {ported} cases");
        report("object visit", native, CASES);
    });
}

/// Fixed walls and a sphere moved across them: the plane distance equal to
/// the radius, the circle grazing a polygon edge, a full output.
#[test]
fn cluster_flood_ties_match_the_guest() {
    with_oracle("collision cluster flood ties", |oracle, image| {
        let mut p = synth::Portals {
            clusters: vec![Vec::new(); 3],
            ..synth::Portals::default()
        };
        for k in 1..3i16 {
            p.planes.push([1.0, 0.0, 0.0, k as f32]);
            let mut vertices = synth::square(0, k as f32, (0.0, 0.0), 1.0);
            vertices.reverse();
            p.link(
                k - 1,
                k,
                synth::PortalSpec {
                    plane: k as u32 - 1,
                    sphere: [k as f32, 0.0, 0.0, 4.0],
                    vertices,
                    ..synth::PortalSpec::default()
                },
            );
        }
        let mut n = 0u32;
        for &x in &[0.25f32, 0.5, 0.75, 1.0, 1.5] {
            for &y in &[0.0f32, 1.0, 1.25, 1.5, 2.0] {
                for &radius in &[0.25f32, 0.5, 0.75, 1.0] {
                    for &(control, capacity) in &[(0x027fu16, 16u32), (0x007f, 1)] {
                        let mut m = fresh(image);
                        let (_, bsp) = p.write(TOPOLOGY, |a, d| m.store(a, d));
                        m.word(layout::ACTIVE_STRUCTURE, TOPOLOGY);
                        m.word(layout::STRUCTURE_BSP, bsp);
                        m.word(layout::FLOOD_EPOCH, 9);
                        m.store(OUT, &[0xcd; 16]);
                        put_vector(&mut m, CENTRE, [x, y, 0.0]);
                        let mut c = entry_cpu(&mut Rng(u64::from(n) | 1), FLOOD.point.base);
                        // The single-precision runs enter with both PE flags clear.
                        let sticky = control == 0x027f;
                        (c.mxcsr, c.x87.control, c.x87.tag) = (if sticky { 0x1fa0 } else { 0x1f80 }, control, 0xffff);
                        c.x87.status = if sticky { 0x20 } else { 0 };
                        m.word(STACK + 4, 0);
                        m.word(STACK + 8, CENTRE);
                        m.float(STACK + 12, radius);
                        m.word(STACK + 16, capacity);
                        m.word(STACK + 20, OUT);
                        let label = format!("centre ({x}, {y}) radius {radius} control {control:#x}");
                        assert!(
                            check(
                                oracle,
                                &FLOOD,
                                &c,
                                &m,
                                |c, g, w| adapter::cluster_flood(c, g, w),
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
