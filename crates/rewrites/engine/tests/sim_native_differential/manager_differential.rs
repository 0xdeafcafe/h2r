//! Private-image oracle for the channel manager. Compare each segment before
//! its callee can consume or overwrite registers, flags, or guest memory.

use engine::sim_anim::manager;
use ir::interp::{Cpu, FlatMemory, Flow};
use lifted::Code;
use lifted::cpu::{from_interp, to_interp};
use lifted::image::{Image, Rng};
use runtime::guest::Permissions;
use runtime::{CpuAbi, Guest, TransferAbi, abi};
use std::collections::{BTreeMap, BTreeSet};

#[path = "manager_differential/low.rs"]
mod low;
#[path = "manager_differential/upper.rs"]
mod upper;
#[path = "manager_differential/tracks.rs"]
mod tracks;
#[path = "manager_differential/queries.rs"]
mod queries;
#[path = "manager_differential/actions.rs"]
mod actions;
#[path = "manager_differential/channel_eval.rs"]
mod channel_eval;
#[path = "manager_differential/channel_select.rs"]
mod channel_select;
#[path = "manager_differential/channel_blend.rs"]
mod channel_blend;
#[path = "manager_differential/advance.rs"]
mod advance;
#[path = "manager_differential/final_channels.rs"]
mod final_channels;
#[path = "manager_differential/graph.rs"]
mod graph;

const RAM: usize = 8 << 20;
const DATA: u32 = 0x0061_0000;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = lift::harness::HARNESS_EXIT;

fn word(m: &mut FlatMemory, addr: u32, value: u32) {
    assert!(m.load(addr, &value.to_le_bytes()), "fixture write at {addr:#x}");
}

fn half(m: &mut FlatMemory, addr: u32, value: u16) {
    assert!(m.load(addr, &value.to_le_bytes()), "fixture write at {addr:#x}");
}

fn cpu(pc: u32, seed: u32) -> CpuAbi {
    let mut rng = Rng(u64::from(seed) << 32 | u64::from(pc));
    let mut cpu = CpuAbi {
        eip: pc,
        esp: STACK,
        eax: DATA + 0x100,
        ecx: DATA + 0x200,
        edx: DATA + 0x300,
        ebx: DATA + 0x400,
        esi: DATA + 0x500,
        edi: DATA + 0x600,
        ebp: DATA + 0x700,
        mxcsr: 0x1f80 | ((seed % 4) << 13),
        eflags: 2 | (rng.u32() & 0x8d5),
        ..CpuAbi::default()
    };
    cpu.x87.control = [0x007f, 0x027f, 0x037f][seed as usize % 3];
    cpu.x87.status = ((seed % 8) as u16) << 11;
    for st in &mut cpu.x87.st {
        for byte in st {
            *byte = rng.u32() as u8;
        }
    }
    for xmm in &mut cpu.xmm {
        for lane in xmm.chunks_exact_mut(4) {
            let value = (rng.below(513) as i32 - 256) as f32 / 128.0;
            lane.copy_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    cpu
}

/// A child is external even when it has its own native body. The oracle
/// stops on a tail jump as well: its target must see the outgoing state.
fn original_boundary(
    code: &mut lifted::Original, body: &manager::Body, cpu: &mut Cpu, ram: &mut FlatMemory,
) -> TransferAbi {
    for _ in 0..100_000 {
        let step = lifted::code::step(code, cpu, ram).unwrap_or_else(|e| panic!("{e}"));
        match step {
            Flow::Call { target, continuation } => return TransferAbi::call(target, continuation),
            Flow::Return { target } => return TransferAbi::ret(target),
            Flow::Jump { target, tail: true } => return TransferAbi::jump(target),
            _ if !(body.entry..body.end).contains(&step.target()) => return TransferAbi::jump(step.target()),
            _ => {}
        }
    }
    panic!("no boundary in body {:#x}, pc {:#x}", body.entry, cpu.eip);
}

struct Harness<'a> {
    template: &'a FlatMemory,
    code: lifted::Original,
    native: Guest,
    physical: usize,
    bodies: BTreeMap<u32, &'static manager::Body>,
    seen: BTreeSet<(u32, u32)>,
}

impl<'a> Harness<'a> {
    fn new(image: &Image, template: &'a FlatMemory) -> Self {
        let mut native = Guest::new();
        native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
        let physical = native.mappings()[0].physical_start as usize;
        let mut bodies = BTreeMap::new();
        let cfgs = analyze::cfg::CfgBuilder::new(image.analysis, image.xbe);
        for body in manager::bodies() {
            assert!(bodies.insert(body.entry, body).is_none(), "duplicate body {:#x}", body.entry);
            let cfg = cfgs.build(body.entry).unwrap_or_else(|| panic!("unrecognized body {:#x}", body.entry));
            let original: BTreeSet<_> = cfg.calls.iter().filter(|c| c.returns).map(|c| c.continuation()).collect();
            let native: BTreeSet<_> = body.sites.iter().copied().collect();
            assert_eq!(original, native, "registered call continuations of {:#x}", body.entry);
        }
        let mut entries: BTreeSet<_> = image.analysis.functions.keys().copied().filter(|pc|
            (0x1c6440..=0x1c7450).contains(pc) || (0x1cb000..0x1ce000).contains(pc)
        ).collect();
        entries.extend([0x1daea0, 0x1dd560, 0x1dd9d0, 0x1ddb40]);
        let registered: BTreeSet<_> = bodies.keys().copied().collect();
        assert_eq!(registered, entries, "manager entry inventory");
        Self { template, native, physical, bodies, seen: BTreeSet::new(), code: image.original() }
    }

    fn begin(
        &mut self, pc: u32, seed: u32, setup: impl FnOnce(&mut CpuAbi, &mut FlatMemory),
    ) -> (CpuAbi, FlatMemory) {
        let mut mem = self.template.clone();
        let mut regs = cpu(pc, seed);
        word(&mut mem, STACK, EXIT);
        setup(&mut regs, &mut mem);
        assert_eq!(regs.eip, pc, "fixture changed its selector");
        self.native.write_bytes(0, &mem.bytes, 1).unwrap();
        (regs, mem)
    }

    fn check(
        &mut self, entry: u32, seed: u32, regs: &mut CpuAbi, mem: &mut FlatMemory,
    ) -> TransferAbi {
        let pc = regs.eip;
        let body = self.bodies[&entry];
        assert!(pc == entry || body.sites.contains(&pc), "undeclared selector {pc:#x} for {entry:#x}");
        let prediction = manager::arm::ENTRIES.contains(&entry).then(|| {
            let prediction = manager::arm::predict(regs, &mut self.native, pc)
                .unwrap_or_else(|| panic!("arm declined {entry:#x}/{pc:#x}, seed {seed}"));
            let live = &self.native.physical_ram()[self.physical..self.physical + RAM];
            assert!(live == mem.bytes, "arm wrote RAM at {entry:#x}/{pc:#x}, seed {seed}");
            prediction
        });
        self.code.prime(entry);
        let mut original = to_interp(regs);
        let expected = original_boundary(&mut self.code, body, &mut original, mem);
        let actual = manager::run(regs, &mut self.native, pc)
            .unwrap_or_else(|e| panic!("{entry:#x}/{pc:#x} seed {seed}: {e:?}"))
            .unwrap_or_else(|| panic!("declined {entry:#x}/{pc:#x}, seed {seed}"));
        let label = || format!("{entry:#x} from {pc:#x}, seed {seed}, target {:#x}", expected.target_pc);
        assert_eq!(actual, expected, "transfer {}", label());
        regs.eip = actual.target_pc;
        let guest_cpu = from_interp(&original);
        assert_eq!(*regs, guest_cpu, "CPU {}", label());
        let bytes = &self.native.physical_ram()[self.physical..self.physical + RAM];
        if bytes != mem.bytes {
            let at = bytes.iter().zip(&mem.bytes).position(|(a, b)| a != b).unwrap();
            panic!("RAM {} at {at:#x}: native {:#04x}, original {:#04x}", label(), bytes[at], mem.bytes[at]);
        }
        self.seen.insert((entry, pc));
        // The full RAM check above established that the native mapping holds
        // the interpreter's bytes. Check the same comparison used by the hook.
        if let Some(prediction) = prediction {
            assert_eq!(prediction.difference(&guest_cpu, &self.native, expected), None, "arm {}", label());
        }
        actual
    }

    /// Start at a real entry. Only the callee is stubbed; its arguments,
    /// saved PC, and the state its caller left are checked first.
    fn case(
        &mut self, entry: u32, seed: u32, setup: impl FnOnce(&mut CpuAbi, &mut FlatMemory),
        mut answer: impl FnMut(u32, u32, &mut CpuAbi, &mut FlatMemory) -> u32,
    ) {
        let (mut regs, mut mem) = self.begin(entry, seed, setup);
        for _ in 0..1024 {
            let transfer = self.check(entry, seed, &mut regs, &mut mem);
            match transfer.kind {
                abi::RETURN => {
                    assert_eq!(transfer.target_pc, EXIT, "return from {entry:#x}, seed {seed}");
                    return;
                }
                abi::JUMP => return,
                abi::CALL => {
                    let site = transfer.resume_pc;
                    let sp = regs.esp;
                    assert_eq!(mem.read_u32(sp), Some(site), "return PC to {entry:#x}");
                    let pop = answer(transfer.target_pc, site, &mut regs, &mut mem);
                    assert_eq!(regs.esp, sp, "stub cleans up through its return value");
                    regs.eip = mem.read_u32(sp).unwrap();
                    regs.esp = sp + 4 + pop;
                    self.native.write_bytes(0, &mem.bytes, 1).unwrap();
                }
                _ => panic!("unexpected transfer from {entry:#x}: {transfer:?}"),
            }
        }
        panic!("unbounded callbacks at {entry:#x}, seed {seed}");
    }

}

#[test]
fn every_manager_entry_and_return_site() {
    let image = Image::require();
    let template = FlatMemory { base: 0, bytes: image.ram.bytes[..RAM].to_vec() };
    let mut h = Harness::new(image, &template);
    low::cases(&mut h);
    upper::cases(&mut h);
    tracks::cases(&mut h);
    queries::cases(&mut h);
    actions::cases(&mut h);
    channel_eval::cases(&mut h);
    channel_select::cases(&mut h);
    channel_blend::cases(&mut h);
    advance::cases(&mut h);
    final_channels::cases(&mut h);
    graph::cases(&mut h);
    let required: BTreeSet<_> = h.bodies.values().flat_map(|b| {
        std::iter::once(b.entry).chain(b.sites.iter().copied()).map(move |pc| (b.entry, pc))
    }).collect();
    let missing: Vec<_> = required.difference(&h.seen).copied().collect();
    assert!(missing.is_empty(), "untested (entry, selector): {missing:x?}");
}

/// Drive through the channel lookup, the graph element resolver and the cache
/// preload gate. A shared preload must hand control to its installed hook
/// with both caller frames intact. An unshared preload returns through both.
#[test]
fn channel_lookup_drives_graph_and_preload() {
    const CHANNEL: u32 = 0x1c6440;
    const GRAPH: u32 = 0x1daea0;
    const PRELOAD: u32 = 0x1dd9d0;
    let image = Image::require();
    let template = FlatMemory { base: 0, bytes: image.ram.bytes[..RAM].to_vec() };
    let mut h = Harness::new(image, &template);
    for shared in [false, true] {
        let (mut regs, mut ram) = h.begin(CHANNEL, u32::from(shared), |c, m| {
            let (tags, graph, clips) = (DATA + 0x1000, DATA + 0x2000, DATA + 0x3000);
            c.ecx = DATA;
            word(m, DATA, 3);
            word(m, DATA + 4, (1 << 16) | 0xffff);
            word(m, guest_layout::points::TAG_ENTRIES.base, tags);
            word(m, tags + 3 * 16 + 8, graph);
            word(m, graph + 0x30, clips);
            m.bytes[engine::sim_native::graph::ENABLED as usize] = 0;
        });
        let mut reference = to_interp(&regs);
        let mut expected = |owner, target, reference: &mut Cpu, ram: &mut FlatMemory| {
            h.code.prime(owner);
            let transfer = original_boundary(&mut h.code, h.bodies[&owner], reference, ram);
            assert_eq!(transfer.target_pc, target, "lookup chain from {owner:#x}");
            transfer
        };
        expected(CHANNEL, GRAPH, &mut reference, &mut ram);
        let mut boundary = expected(GRAPH, PRELOAD, &mut reference, &mut ram);
        if !shared {
            expected(PRELOAD, 0x1daf1f, &mut reference, &mut ram);
            expected(GRAPH, 0x1c6466, &mut reference, &mut ram);
            boundary = expected(CHANNEL, EXIT, &mut reference, &mut ram);
        }
        let actual = manager::drive(&mut regs, &mut h.native, CHANNEL, |b| !(shared && b.entry == PRELOAD))
            .unwrap().expect("channel/graph/preload chain declined");
        assert_eq!(actual, boundary, "shared preload {shared}");
        regs.eip = actual.target_pc;
        assert_eq!(regs, from_interp(&reference), "shared preload {shared}");
        let got = &h.native.physical_ram()[h.physical..h.physical + RAM];
        assert!(got == ram.bytes, "chained lookup RAM, shared preload {shared}");
    }
}
