//! `b8d30`'s marker lookup against the lifted guest with the phase 1c
//! kernel on: the register-level port (`sim_native::marker`) runs the
//! entry, and `engine::sim_anim::markers` fills the records where the
//! port would call `1d8f50`. Nothing is stubbed: the lifted side runs
//! `b8d30`, `1d8f00`, `1d8f50`, `141e10` and `142a60` on the IR
//! interpreter from the same state. Private XBE as the oracle, synthetic
//! models.
//!
//! The kernel replaces a whole subtree, so the comparison is final state,
//! not the store transcript: guest memory outside the dead stack below
//! the entry (the pushed arguments and the family's frames), the
//! callee-saved registers, `esp`, the next pc, MXCSR and the x87 control,
//! status and tag words. Each case runs on host lanes and on the scalar
//! loop; both must match the guest. A declined call panics `anim marker:
//! <reason>` in the port (no guest call runs); the test counts it and
//! skips the comparison.
//!
//! This file is its own test binary (Cargo.toml): it flips the marker
//! kernel's process-global switch on, which the stubbed port differential
//! in the `it` binary needs off.
#![cfg(feature = "differential")]

mod common;

use engine::sim_anim::markers as adapter;
use engine::{points, sim_native as sim};
use ir::interp::{self, Cpu};
use lift::harness::HARNESS_EXIT;
use lifted::cpu::to_interp as ir_cpu;
use runtime::{CpuAbi, TransferAbi};
use std::collections::{BTreeMap, BTreeSet};

const STACK: u32 = 0x007f_f000;
const HEADER: u32 = 0x0050_0000;
const SLOTS: u32 = 0x0050_1000;
const OBJECT: u32 = 0x0060_0000;
const TAGS: u32 = 0x0061_0000;
const DEF: u32 = 0x0061_1000;
const MODEL_A: u32 = 0x0061_2000;
const RENDER: u32 = 0x0061_3000;
const GROUPS_AT: u32 = 0x0061_4000;
const INSTANCES_AT: u32 = 0x0061_5000;
const NODE_ARRAY: u32 = OBJECT + 0x4000;
const MATRICES: u32 = OBJECT + 0x6000;
const OUT: u32 = 0x0063_0000;
const HANDLE: u32 = 4;
const DEF_HANDLE: u32 = 1;
const MODEL_HANDLE: u32 = 2;
const RENDER_HANDLE: u32 = 3;
const NAME: u32 = 0x2a;

/// Written by the original, read by nothing outside the family: the
/// pushed arguments and the frames of `1d8f00` and `1d8f50` below the
/// entry stack.
const UNCOMPARED: [(u32, u32); 1] = [(STACK - 0x400, STACK)];

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
    /// Addresses either side wrote, outside the uncompared ranges.
    fn touched<'b>(&'b self, other: &'b Ram<'_>) -> BTreeSet<u32> {
        self.overlay
            .keys()
            .chain(other.overlay.keys())
            .copied()
            .filter(|&p| !UNCOMPARED.iter().any(|&(lo, hi)| (lo..hi).contains(&p)))
            .collect()
    }
}

impl sim::Memory for Ram<'_> {
    fn read_bytes(&self, a: u32, dst: &mut [u8], _align: u32) -> Result<(), runtime::GuestFault> {
        self.read(a, dst);
        Ok(())
    }
    fn write_bytes(&mut self, a: u32, data: &[u8], _align: u32) -> Result<(), runtime::GuestFault> {
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
        Ram::store(self, a, data);
        Ok(())
    }
}

/// What the comparison covers: `ebx`, `esp`, `ebp`, `esi`, `edi`, the
/// next pc, MXCSR, x87 control/status/tag.
fn visible(c: &Cpu) -> [u32; 10] {
    [
        c.gpr[3],
        c.gpr[4],
        c.gpr[5],
        c.gpr[6],
        c.gpr[7],
        c.eip,
        c.mxcsr,
        u32::from(c.x87.control),
        u32::from(c.x87.status),
        u32::from(c.x87.tag),
    ]
}

struct Oracle<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, ir::IrInsn>,
}

impl Oracle<'_> {
    /// Lift the function at `entry`, so a run that resumes in its middle
    /// (the kernel's handoff at `b8ded`) finds those instructions.
    fn prime(&mut self, entry: u32) {
        let f = self
            .lifter
            .lift(entry)
            .unwrap_or_else(|| panic!("function at {entry:#x}"));
        for insn in f.blocks.into_values().flat_map(|b| b.insns) {
            self.cache.insert(insn.pc, insn);
        }
    }

    fn run(&mut self, c: &mut Cpu, mem: &mut Ram<'_>, mut pc: u32) {
        for _ in 0..2_000_000 {
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

/// Load the image and the lifter, then run `f`. `None` without `H2_XBE`.
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
    oracle.prime(points::SIM_MARKER_TRANSFORMS.base);
    f(&mut oracle, &image);
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// A magnitude in `[0.25, 4)`, either sign: ordinary for the kernel.
    fn value(&mut self) -> u32 {
        let r = self.next();
        let sign = ((r >> 40) as u32 & 1) << 31;
        sign | (125 + (r % 4) as u32) << 23 | (r >> 8) as u32 & 0x7f_ffff
    }
}

/// One synthetic model and call. Returns whether the kernel is expected
/// to fill (the call reaches `1d8f50` on the fast path).
fn case(rng: &mut Rng, n: u32, c: &mut CpuAbi, m: &mut Ram<'_>) -> bool {
    let w = |m: &mut Ram, a: u32, v: u32| m.word(a, v);
    w(m, points::OBJECTS.base, HEADER);
    w(m, HEADER + 0x44, SLOTS);
    w(m, SLOTS + 12 * HANDLE + 8, OBJECT);
    w(m, points::TAG_ENTRIES.base, TAGS);
    for (h, data) in [
        (DEF_HANDLE, DEF),
        (MODEL_HANDLE, MODEL_A),
        (RENDER_HANDLE, RENDER),
    ] {
        w(m, TAGS + 16 * h + 8, data);
    }

    // The groups: two or three, the wanted name in one of them (or none).
    let groups = (1 + rng.next() % 3) as u32;
    let wanted = (rng.next() % 4) as u32; // < groups, or absent
    let absent = wanted >= groups;
    let instances = if absent { 0 } else { (rng.next() % 6) as u32 };
    let node_count: u32 = 4;
    w(m, RENDER + 0x58, groups);
    w(m, RENDER + 0x5c, GROUPS_AT);
    for i in 0..groups {
        let group = GROUPS_AT + 12 * i;
        let holds = !absent && i == wanted;
        w(m, group, if holds { NAME } else { NAME + 1 + i });
        w(m, group + 4, if holds { instances } else { 0 });
        w(m, group + 8, if holds { INSTANCES_AT } else { 0 });
    }

    // The instances: a node byte (sometimes none), a name byte the node
    // array answers or not, an output node, a translation, a quaternion
    // (sometimes all zero: the identity basis) and a trailing float.
    for i in 0..instances {
        let at = INSTANCES_AT + 0x24 * i;
        let none = rng.next() % 7 == 0;
        let node = if none {
            0xff
        } else {
            (rng.next() % u64::from(node_count)) as u8
        };
        let answers = rng.next() % 5 != 0;
        let name = if answers { 0x40u8.wrapping_add(node) } else { 0x80 };
        m.store(at, &[node, name, (rng.next() % 5) as u8, 0]);
        for k in 1..9 {
            let v = if rng.next() % 6 == 0 {
                0 // a zero quaternion component
            } else {
                rng.value()
            };
            w(m, at + 4 * k as u32, v);
        }
    }
    // The node array answers each node with its own name.
    for i in 0..node_count {
        m.store(NODE_ARRAY + i as u32, &[0x40 + i as u8]);
    }
    // The matrices, ordinary throughout.
    for i in 0..5u32 {
        for k in 0..13u32 {
            w(m, MATRICES + 0x34 * i + 4 * k, rng.value());
        }
    }

    // The object: mirrored or not, no parent, matrices at +0x6000, nodes
    // at +0x4000, and a byte size the divide magic turns into a count.
    let mirror = rng.next() % 2 == 0;
    w(m, OBJECT, DEF_HANDLE);
    w(m, OBJECT + 4, if mirror { 1 << 10 } else { 0 });
    w(m, OBJECT + 0x14, u32::MAX);
    m.store(OBJECT + 0x114, &(0x34u16 * 5).to_le_bytes());
    m.store(OBJECT + 0x116, &((MATRICES - OBJECT) as u16).to_le_bytes());
    m.store(
        OBJECT + 0x11a,
        &((NODE_ARRAY - OBJECT) as u16).to_le_bytes(),
    );
    w(
        m,
        DEF + 0x38,
        if n % 11 == 10 { u32::MAX } else { MODEL_HANDLE },
    );
    w(m, MODEL_A + 4, RENDER_HANDLE);

    // The call: the capacity cuts the output or not, the name is the
    // wanted one (or a sentinel the scan refuses).
    let capacity = match n % 5 {
        0 => instances.max(1),
        1 => 1,
        2 => 0,
        3 => 0xffff, // -1 as the guest reads it
        _ => 8,
    };
    let name = match n % 13 {
        11 => u32::MAX,
        12 => 0,
        _ => NAME,
    };
    w(m, STACK, HARNESS_EXIT);
    w(m, STACK + 4, HANDLE);
    w(m, STACK + 8, name);
    w(m, STACK + 0xc, capacity);
    *c = CpuAbi {
        eax: 0x1234_5678,
        ecx: 1, // `cl` set: no parent resolution
        edx: 0x2345_6789,
        ebx: OUT,
        esp: STACK,
        ebp: STACK - 0x80,
        esi: 0x1773_5566,
        edi: 0x6257_1918,
        eip: points::SIM_MARKER_TRANSFORMS.base,
        eflags: 0x293,
        mxcsr: if n % 9 == 8 { 0x1f80 } else { 0x1fa0 },
        ..CpuAbi::default()
    };
    c.x87.control = 0x027f;
    for (i, reg) in c.xmm.iter_mut().enumerate() {
        let lane = [0x3f80_1000u32, 0x7f80_2001, 0x8000_0000, 0xffc0_1234][i % 4];
        *reg = ((u128::from(lane) << 96) | (u128::from(lane) << 64) | 13).to_le_bytes();
    }
    n % 11 != 10 // the model is present
}

/// Run the port on `(c, m)` and continue lifted from its transfer; then
/// compare with the guest run from the entry. `false` when the kernel
/// declined (the port panicked `anim marker: <reason>`; nothing to compare).
fn check(oracle: &mut Oracle<'_>, c: &CpuAbi, m: &Ram<'_>, case: &str) -> bool {
    let mut reference: Option<(Cpu, Ram<'_>)> = None;
    let mut native = false;
    for (simd, label) in [(true, "host lanes"), (false, "scalar")] {
        let (mut nc, mut nm) = (*c, m.clone());
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sim::with_simd(simd, || sim::marker::transforms(&mut nc, &mut nm)).unwrap()
        }));
        let transfer = match run {
            Ok(t) => t,
            Err(panic) => {
                let message = panic.downcast_ref::<String>().map_or("", String::as_str);
                assert!(message.contains("anim marker: "), "{case} {label}: {message}");
                return false;
            }
        };
        // The kernel's handoff is the continuation at `b8ded`; anything
        // else (the scan bail, the fallback) did not fill.
        native = transfer == TransferAbi::resume(adapter::AFTER_MARKERS);
        let mut got = ir_cpu(&nc);
        oracle.run(&mut got, &mut nm, transfer.target_pc);
        let (guest_cpu, guest_mem) = reference.get_or_insert_with(|| {
            let mut gm = m.clone();
            let mut gc = ir_cpu(c);
            oracle.run(&mut gc, &mut gm, points::SIM_MARKER_TRANSFORMS.base);
            (gc, gm)
        });
        assert_eq!(
            visible(&got),
            visible(guest_cpu),
            "{case} {label}: registers"
        );
        for p in nm.touched(guest_mem) {
            assert_eq!(
                nm.get(p),
                guest_mem.get(p),
                "{case} {label}: byte at {p:#x}"
            );
        }
    }
    native
}

#[test]
fn marker_transforms_match_the_guest() {
    with_oracle("marker differential test", |oracle, image| {
        adapter::set_enabled(true);
        const CASES: u32 = 200;
        let (mut filled, mut declined) = (0u32, 0u32);
        for n in 0..CASES {
            let mut rng = Rng(0x9e37_79b9 ^ u64::from(n).wrapping_mul(0x2545_f491) | 1);
            let mut c = CpuAbi::default();
            let mut m = Ram {
                image,
                overlay: BTreeMap::new(),
            };
            let present = case(&mut rng, n, &mut c, &mut m);
            if check(oracle, &c, &m, &format!("case {n}")) {
                assert!(present, "case {n}: filled without a model");
                filled += 1;
            } else {
                declined += 1;
            }
        }
        eprintln!("marker transforms: {filled} filled, {declined} declined of {CASES}");
        assert!(
            filled * 3 > CASES,
            "the kernel filled {filled} of {CASES} cases"
        );
    });
}
