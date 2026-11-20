//! The object update's harness (docs/engine/object-update.md,
//! "Verification"): a root (a walk or a per-object update) runs over
//! synthetic object pools once on the lifted original alone, and again
//! with `engine::sim_objects` fusing every body it owns, and again with
//! the two mixed at each resume point in turn.
//!
//! - **Fused transcripts.** The sequence of true callouts (calls and tail
//!   jumps that leave the root's tree: the root and every body) must be
//!   the same in every run: target, return site, the eight integer
//!   registers, the flags both sides define, and all memory outside the
//!   dead stack below `esp`. So must the state the root returns with.
//! - **Mixed frames.** At each resume point where a body could take over
//!   (its entry, or one of its sites), one run switches from the lifted
//!   code to the body there and another from the body to the lifted code.
//!
//! Callouts are answered by seeded stubs: a return value, clobbered
//! caller-saved registers, and changes to the live set between segments
//! (a slot freed, the high-water mark moved, a body moved by pool
//! compaction, an object marked for deletion). The stack probe and the
//! spatial visit `109580` calls are stubbed too: the probe moves `esp`
//! as the real one does, the visit fills the caller's result buffer.
//!
//! Roots: the three walks, the two per-object updates, the four type
//! dispatchers, the wake leaves and the early-mover wake (slice 1 of
//! docs/engine/sim-rewrite-objects.md). Every body the root reaches is
//! part of its tree; a callee that is not a body is a callout. The
//! fused finalize run must also record one `ObjectDeleted` event per
//! object it deletes.
//!
//! Needs the user's XBE (`lifted::image`); skipped without it.
#![cfg(feature = "differential")]

use engine::points;
use engine::sim_objects::{self, Table};
use ir::interp::{Cpu, Flow, MemFault};
use lifted::Code;
use lifted::cpu::{from_interp, to_interp};
use runtime::{Guest, TransferAbi, abi, guest::Permissions};
use std::hash::{DefaultHasher, Hasher};

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
/// The dead stack the comparison masks starts here.
const STACK_LOW: u32 = STACK - 0x0001_0000;
const EXIT: u32 = 0xffff_fffc;
const STEPS: u64 = 2_000_000;

// The synthetic object pool (the same shape as the per-function ports'
// harness, `sim_physics_differential.rs`).
const PHASE: u32 = 0x0060_0000;
const LIST: u32 = 0x0060_1000;
const HEADER: u32 = 0x0060_4000;
const DATA: u32 = 0x0060_5000;
const BITS: u32 = 0x0060_6000;
const RECORDS: u32 = 0x0061_0000;
const COMPONENTS: u32 = 0x0063_0000;
const BODY: u32 = 0x200;
const SLOTS: u32 = 100;

/// Globals the walks and the per-object updates read.
const PHASE_STATE: u32 = 0x004d_e2f4;
const LIST_OWNER: u32 = 0x0051_07f0;
const COMPONENT_ARRAY: u32 = 0x0051_e9b8;

/// The runtime's guest memory as the interpreter's.
struct OnGuest<'a>(&'a mut Guest);

impl ir::interp::Memory for OnGuest<'_> {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.0
            .read_bytes(addr, buf, 1)
            .map_err(|f| MemFault { addr: f.addr, write: false })
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        self.0
            .write_bytes(addr, data, 1)
            .map_err(|f| MemFault { addr: f.addr, write: true })
    }
}

fn word(g: &mut Guest, a: u32, v: u32) {
    g.set_u32(a, v).unwrap();
}

fn byte(g: &mut Guest, a: u32, v: u8) {
    g.set_u8(a, v).unwrap();
}

fn handle(slot: u32) -> u32 {
    0x8200_0000 | slot
}

/// A pool of `SLOTS` objects, an early-mover list of three, and a bitmap
/// whose shape depends on `case`: empty, a high-water mark at and across
/// word boundaries, a hole, children linked under a parent.
fn pool(g: &mut Guest, case: u32) {
    word(g, PHASE_STATE, PHASE);
    word(g, LIST_OWNER, LIST);
    word(g, points::OBJECTS.base, HEADER);
    word(g, points::GAME_GLOBALS.base, PHASE);
    byte(g, PHASE + 12, [4, 5, 0, 4, 1, 5][case as usize % 6]);
    word(g, HEADER + 0x24, 12);
    word(g, HEADER + 0x44, DATA);
    word(g, HEADER + 0x48, BITS);
    word(g, HEADER + 0x38, [0, 31, 32, 33, 97, 64][case as usize % 6]);
    word(g, LIST + 0x1d80, if case == 0 { 0 } else { 3 });
    for (i, slot) in [4, 2, 5].into_iter().enumerate() {
        word(g, LIST + 0x1d00 + 4 * i as u32, handle(slot));
    }
    word(g, COMPONENT_ARRAY, COMPONENTS);
    word(g, COMPONENTS + 0x44, COMPONENTS + 0x100);
    for slot in 0..SLOTS {
        let h = DATA + 12 * slot;
        let body = RECORDS + BODY * slot;
        g.set_u16(h, (handle(slot) >> 16) as u16).unwrap();
        // Flags: every combination of ACTIVE, AWAKE, POST, NEW, DELETE,
        // DEFER over the slots; types cycle through the table.
        byte(g, h + 2, [0x03, 0x07, 0x0b, 0x13, 0x23, 0x01, 0x83, 0x47][(slot + case) as usize % 8]);
        byte(g, h + 3, ((slot + case) % 13) as u8);
        word(g, h + 8, body);
        // No links unless a case adds them; no component; no animation.
        for off in [0x0c, 0x10, 0x14] {
            word(g, body + off, u32::MAX);
        }
        word(g, body + 0xb4, u32::MAX);
        // On the early-mover list (bit 3) for some, woken this tick (bit
        // 2, so the pre-update wakes them for the post walk) for others.
        byte(g, body + 0xc0, u8::from(slot % 3 == 1) << 3 | u8::from(slot % 4 == 2) << 2);
        // The dispatchers read the type from the body.
        byte(g, body + 0xaa, ((slot + case) % 13) as u8);
        g.set_u16(body + 0x12a, u16::MAX).unwrap();
        g.set_u16(body + 0x114, 0).unwrap();
    }
    if case >= 3 {
        // Slot 2 is the parent of 6 and 7: children are visited after it.
        word(g, RECORDS + BODY * 2 + 0x10, handle(6));
        word(g, RECORDS + BODY * 6 + 0x0c, handle(7));
        for child in [6, 7] {
            word(g, RECORDS + BODY * child + 0x14, handle(2));
        }
    }
    word(g, BITS, if case == 2 { 0 } else { 1 << 31 | 0b1111_0110 });
    word(g, BITS + 4, if case == 4 { u32::MAX } else { 1 });
    word(g, BITS + 8, 0);
    word(g, BITS + 12, u32::MAX);
}

/// The object a per-object root is called on.
fn object(case: u32) -> u32 {
    handle([2, 4, 5, 6, 1, 3][case as usize % 6])
}

/// Roots that take the object as their first stack argument.
fn object_on_stack(root: u32) -> bool {
    [
        &points::OBJECT_PRE_UPDATE,
        &points::OBJECT_POST_UPDATE,
        &points::OBJECT_TYPE_UPDATE,
        &points::OBJECT_WAKE_CHILDREN,
        &points::OBJECT_EARLY_MOVER_WAKE,
    ]
    .iter()
    .any(|p| p.base == root)
}

/// Entry state for a root: the planted return, the stack arguments, and
/// the register argument of the roots that take one.
fn entry(g: &mut Guest, cpu: &mut Cpu, root: u32, case: u32) {
    word(g, STACK, EXIT);
    word(g, STACK + 4, if object_on_stack(root) { object(case) } else { 0xdeaf_0002 });
    // The pose dispatcher's second and third arguments; the after-root
    // dispatcher's matrix is the first.
    word(g, STACK + 8, PHASE + 0x100);
    word(g, STACK + 12, case);
    let reg = match root {
        r if r == points::OBJECT_TYPE_POST_UPDATE.base => Some(7),
        r if r == points::OBJECT_TYPE_POSE.base || r == points::OBJECT_TYPE_AFTER_ROOT.base => Some(3),
        r if r == points::OBJECT_WAKE.base || r == points::OBJECT_WAKE_FOR_POST.base => Some(0),
        r if r == points::OBJECT_ROOT.base => Some(1),
        _ => None,
    };
    if let Some(reg) = reg {
        cpu.gpr[reg] = object(case);
    }
}

/// Functions the roots leave by `jmp` (finalize's tail), whether or not
/// the lifter marks the jump as a tail call.
const TAILS: &[u32] = &[0xbf380];

/// The type callbacks the dispatchers reach through the descriptors'
/// slots (docs/engine/sim-rewrite-objects.md section 2): `+40` and `+44`
/// take `(object)`, `+6c` four arguments, `+70` two.
const SLOT_UPDATE: &[u32] = &[
    0xc6b30, 0xdd360, 0xefde0, 0x10b4d0, 0xfdfb0, 0xf8de0, 0x10a2f0, 0x106790, 0x1152e0, 0x11bce0, 0x118700,
];
const SLOT_POST: &[u32] = &[0xdd990, 0x118b80];
const SLOT_POSE: &[u32] = &[0x112070, 0xf0f50, 0x107000];
const SLOT_AFTER_ROOT: &[u32] = &[0x112dc0, 0x10a390];

/// The stack probe `109580` makes its frame with.
const STACK_PROBE: u32 = 0x320560;

/// Callee cleanup (`ret N`) of every function the roots call.
fn cleanup(target: u32) -> u32 {
    match target {
        0xbc470 | 0xbc820 | 0x109580 | 0x108c60 | 0xd5de0 | 0xbe8e0 | 0xbc5e0 | 0xbd090 | 0xbdef0 => 4,
        t if SLOT_UPDATE.contains(&t) || SLOT_POST.contains(&t) => 4,
        0xb83b0 | 0xb8460 => 8,
        t if SLOT_AFTER_ROOT.contains(&t) => 8,
        0x1d3920 => 12,
        t if SLOT_POSE.contains(&t) => 16,
        0xbef30 => 20,
        0xbb050 => 28,
        0xbba20 | 0xa9570 | 0x1cd4e0 | 0x1ced00 | 0x108cd0 | 0xbe1d0 | 0x109660 | 0xfd910 | 0xbf380 => 0,
        _ => panic!("no stub for callee {target:#x}: add its cleanup"),
    }
}

/// Callouts after which the live set changes: type callbacks, damage,
/// deletion, deferred weapon fire and the early-mover record.
fn mutates(target: u32) -> bool {
    matches!(target, 0xbc470 | 0xbc820 | 0x109580 | 0xb83b0 | 0xd5de0 | 0xfd910 | 0x109660)
        || SLOT_UPDATE.contains(&target)
        || SLOT_POST.contains(&target)
}

/// The transfer a stub returns: the callee's effects on the state at a
/// callout, the same in every run.
fn answer(cpu: &mut Cpu, g: &mut Guest, target: u32, n: usize, case: u32) {
    let sp = cpu.gpr[4];
    let ret = g.u32(sp).unwrap();
    if target == STACK_PROBE {
        // `eax` bytes below the caller's `esp`: `push ecx` at the top,
        // `eax` the return address, `ecx` kept.
        let caller = sp + 4;
        word(g, caller - 8, cpu.gpr[1]);
        cpu.gpr[4] = caller - cpu.gpr[0];
        cpu.gpr[0] = ret;
        cpu.eip = ret;
        return;
    }
    if target == points::OBJECT_SPATIAL_VISIT.base {
        // Results into the caller's buffer (argument 5): live slots, a
        // free one and a child, varying with the case and the call.
        let out = g.u32(sp + 0x18).unwrap();
        let found = [1, 2, 9, 3, 6, 7, 10, 5];
        let count = (case as usize + n) % found.len() + 1;
        for (i, slot) in found.iter().take(count).enumerate() {
            word(g, out + 4 * i as u32, handle(*slot));
        }
        cpu.gpr[0] = 0xabcd_0000 | count as u32;
        cpu.gpr[1] = 0x1357;
        cpu.gpr[2] = 0x2468;
        cpu.gpr[4] = sp + 4 + cleanup(target);
        cpu.eip = ret;
        return;
    }
    let seed = target.wrapping_mul(0x9127).rotate_left(n as u32 % 32) ^ case;
    cpu.gpr[0] = match target {
        // Per-object updates answer in `al`.
        0xbc470 | 0x108c60 | 0xd5de0 | 0xbe8e0 | 0x1cd4e0 => n as u32 % 2,
        0xbdef0 => (n as u32 / 2) % 2,
        _ => 0x006e_1000 | (seed & 0xff),
    };
    cpu.gpr[1] = seed.rotate_left(9);
    cpu.gpr[2] = !seed;
    // Callbacks change the live set; both sides must re-read it.
    if mutates(target) {
        match (n + case as usize) % 5 {
            // Free the next slots and move the high-water mark.
            1 => {
                word(g, BITS + 4, 0b110);
                word(g, HEADER + 0x38, 35);
            }
            // Pool compaction: move slot 5's body. The old copy reads as
            // none everywhere, so code that kept the old pointer reads
            // ends of lists, not wild handles, and the difference shows
            // in what it writes.
            2 => {
                let from = g.u32(DATA + 12 * 5 + 8).unwrap();
                let to = RECORDS + BODY * (SLOTS + n as u32 % 8);
                let bytes = (0..BODY).map(|i| g.u8(from + i).unwrap()).collect::<Vec<_>>();
                g.write_bytes(to, &bytes, 1).unwrap();
                g.write_bytes(from, &vec![0xff; BODY as usize], 1).unwrap();
                word(g, DATA + 12 * 5 + 8, to);
            }
            // Mark slot 4 for deletion and clear its NEW.
            3 => {
                let f = g.u8(DATA + 12 * 4 + 2).unwrap();
                byte(g, DATA + 12 * 4 + 2, (f | 0x10) & !0x08);
            }
            // Detach slot 7 from its parent.
            4 => word(g, RECORDS + BODY * 2 + 0x10, u32::MAX),
            _ => {}
        }
    }
    cpu.gpr[4] = sp + 4 + cleanup(target);
    cpu.eip = ret;
}

/// One true callout, as the comparison sees it.
#[derive(Clone, Debug)]
struct Callout {
    kind: u32,
    target: u32,
    resume: u32,
    gpr: [u32; 8],
    eflags: u32,
    defined: u32,
    ram: u64,
}

impl PartialEq for Callout {
    fn eq(&self, o: &Self) -> bool {
        let both = self.defined & o.defined;
        (self.kind, self.target, self.resume, self.gpr, self.ram)
            == (o.kind, o.target, o.resume, o.gpr, o.ram)
            && self.eflags & both == o.eflags & both
    }
}

/// All memory except the dead stack below `esp`.
fn digest(g: &Guest, esp: u32) -> u64 {
    let start = g.mappings()[0].physical_start as usize;
    let ram = &g.physical_ram()[start..start + RAM];
    let dead = (STACK_LOW as usize)..(esp.clamp(STACK_LOW, STACK) as usize);
    let mut h = DefaultHasher::new();
    h.write(&ram[..dead.start]);
    h.write(&ram[dead.end..]);
    h.finish()
}

fn callout(cpu: &Cpu, g: &Guest, t: TransferAbi) -> Callout {
    Callout {
        kind: t.kind,
        target: t.target_pc,
        resume: t.resume_pc,
        gpr: cpu.gpr,
        eflags: cpu.eflags,
        defined: cpu.flags_valid,
        ram: digest(g, cpu.gpr[4]),
    }
}

/// One run's record: the callouts, the pc at each resume point, and the
/// state the root returned with.
struct Record {
    callouts: Vec<Callout>,
    resumes: Vec<u32>,
    end: Callout,
}

struct Harness<'a> {
    code: &'a mut lifted::Original,
    table: &'a Table,
    root: u32,
    case: u32,
}

impl Harness<'_> {
    /// The root and every body: code a run executes itself. Everything
    /// else is a callout.
    fn tree(&self, pc: u32) -> bool {
        pc == self.root || self.table.owns(pc)
    }

    /// Step the lifted code from `cpu.eip` to the next transfer the run
    /// loop decides on: a callout, the root's return, or (with `rust`) a
    /// transfer into a body.
    fn lifted(&mut self, cpu: &mut Cpu, g: &mut Guest, entry_esp: u32, rust: bool) -> TransferAbi {
        for _ in 0..STEPS {
            let flow = lifted::code::step(&mut *self.code, cpu, &mut OnGuest(g))
                .unwrap_or_else(|e| panic!("root {:#x} case {}: {e}", self.root, self.case));
            match flow {
                Flow::Call { target, continuation } => {
                    if !self.tree(target) || (rust && self.table.owns(target)) {
                        return TransferAbi::call(target, continuation);
                    }
                }
                Flow::Jump { target, tail } if tail || TAILS.contains(&target) => {
                    if !self.tree(target) || (rust && self.table.owns(target)) {
                        return TransferAbi::jump(target);
                    }
                }
                Flow::Return { target } => {
                    if cpu.gpr[4] > entry_esp || (rust && self.table.at(target).is_some()) {
                        return TransferAbi::ret(target);
                    }
                }
                _ => {}
            }
        }
        panic!("root {:#x} case {}: no transfer in {STEPS} steps", self.root, self.case);
    }

    /// Run the root from `cpu`; `rust(k)` says whether, after `k`
    /// callouts, a body may be handed to the dispatcher. In a run that
    /// never does, `resumes[k]` is where the run resumed after callout
    /// `k` (`resumes[0]` the root).
    fn run(&mut self, mut cpu: Cpu, mut g: Guest, rust: &dyn Fn(usize) -> bool) -> Record {
        let entry_esp = cpu.gpr[4];
        let mut rec = Record { callouts: Vec::new(), resumes: Vec::new(), end: callout(&cpu, &g, TransferAbi::ret(0)) };
        for _ in 0..4096 {
            let k = rec.callouts.len();
            let pc = cpu.eip;
            rec.resumes.push(pc);
            let on = rust(k);
            let t = if on && self.table.at(pc).is_some() && sim_objects::supported(&from_interp(&cpu)) {
                let mut regs = from_interp(&cpu);
                let t = sim_objects::drive(&mut regs, &mut g, self.table, pc)
                    .unwrap_or_else(|f| panic!("root {:#x} case {}: body fault {f:?}", self.root, self.case));
                assert_ne!(t.route, abi::RESUME, "a body escaped to generated code");
                let tsc = cpu.tsc;
                cpu = to_interp(&regs);
                cpu.tsc = tsc;
                cpu.eip = t.target_pc;
                t
            } else {
                self.lifted(&mut cpu, &mut g, entry_esp, on)
            };
            let leaves = match t.kind {
                abi::RETURN => {
                    if cpu.gpr[4] > entry_esp {
                        rec.end = callout(&cpu, &g, t);
                        return rec;
                    }
                    false
                }
                _ => !self.tree(t.target_pc),
            };
            if leaves {
                rec.callouts.push(callout(&cpu, &g, t));
                answer(&mut cpu, &mut g, t.target_pc, k, self.case);
                if cpu.gpr[4] > entry_esp {
                    // A tail callout returned to the root's caller.
                    rec.end = callout(&cpu, &g, TransferAbi::ret(cpu.eip));
                    return rec;
                }
            }
        }
        panic!("root {:#x} case {}: unbounded callouts", self.root, self.case);
    }
}

fn image() -> Option<(&'static lifted::image::Image, &'static ir::interp::FlatMemory)> {
    static RAM_IMAGE: std::sync::OnceLock<ir::interp::FlatMemory> = std::sync::OnceLock::new();
    let image = lifted::image::Image::get()?;
    let ram = RAM_IMAGE.get_or_init(|| {
        let mut ram = image.ram.clone();
        ram.bytes.truncate(RAM);
        ram
    });
    Some((image, ram))
}

fn start(ram: &ir::interp::FlatMemory, root: u32, case: u32) -> (Cpu, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    g.write_bytes(0, &ram.bytes, 1).unwrap();
    pool(&mut g, case);
    let mut cpu = Cpu {
        gpr: [0x1234_5678, 0x9876_5432, 0xfe3420, 0x7b1c, STACK, 0xaabbcc, 0x5151, 0xe37],
        eip: root,
        eflags: 0xa93,
        ..Default::default()
    };
    entry(&mut g, &mut cpu, root, case);
    for (i, reg) in cpu.xmm.iter_mut().enumerate() {
        *reg = (0x1234_u128 + i as u128) << 100;
    }
    (cpu, g)
}

fn check(name: &str, reference: &Record, other: &Record) {
    for (i, (a, b)) in reference.callouts.iter().zip(&other.callouts).enumerate() {
        assert_eq!(a, b, "{name}: callout {i} differs");
    }
    assert_eq!(reference.callouts.len(), other.callouts.len(), "{name}: callout count");
    assert_eq!(reference.end, other.end, "{name}: the state the root returned with");
}

/// Every run of `root` over `cases` pools agrees with the lifted one.
fn transcripts(point: &points::Point, cases: u32) {
    let Some((image, ram)) = image() else { return };
    let table: &Table = &*sim_objects::TABLE;
    let mut code = image.original();
    code.prime(point.base);
    for body in table.bodies() {
        code.prime(body.point.base);
    }
    assert!(code.insn(point.base).is_some(), "{}", point.name);
    let mut h = Harness { code: &mut code, table, root: point.base, case: 0 };
    for case in 0..cases {
        h.case = case;
        let fresh = || start(ram, point.base, case);
        let (cpu, g) = fresh();
        let reference = h.run(cpu, g, &|_| false);
        let (cpu, g) = fresh();
        ::sim_objects::events::clear();
        let fused = h.run(cpu, g, &|_| true);
        check(&format!("{} case {case} fused", point.name), &reference, &fused);
        if point.base == points::OBJECTS_FINALIZE.base && table.owns(point.base) {
            // One `ObjectDeleted` per object finalize deletes, recorded by
            // the body before `b83b0`.
            let deletes = reference.callouts.iter().filter(|c| c.target == 0xb83b0).count();
            let events = ::sim_objects::events::with(|e| e.len());
            assert_eq!(events, deletes, "{} case {case}: deletion events", point.name);
        }
        // Mixed frames: at each resume point a body could take, switch
        // from lifted to Rust there, and from Rust to lifted.
        for (k, &pc) in reference.resumes.iter().enumerate() {
            if table.at(pc).is_none() {
                continue;
            }
            let (cpu, g) = fresh();
            let into = h.run(cpu, g, &|i| i >= k);
            check(&format!("{} case {case} lifted then Rust at {k} ({pc:#x})", point.name), &reference, &into);
            let (cpu, g) = fresh();
            let out = h.run(cpu, g, &|i| i < k);
            check(&format!("{} case {case} Rust then lifted at {k} ({pc:#x})", point.name), &reference, &out);
        }
    }
}

#[test]
fn objects_pre_physics() {
    transcripts(&points::OBJECTS_PRE_PHYSICS, 6);
}

#[test]
fn objects_post_physics() {
    transcripts(&points::OBJECTS_POST_PHYSICS, 6);
}

#[test]
fn objects_finalize() {
    transcripts(&points::OBJECTS_FINALIZE, 6);
}

#[test]
fn object_pre_update() {
    transcripts(&points::OBJECT_PRE_UPDATE, 6);
}

#[test]
fn object_post_update() {
    transcripts(&points::OBJECT_POST_UPDATE, 6);
}

#[test]
fn type_update_dispatch() {
    transcripts(&points::OBJECT_TYPE_UPDATE, 13);
}

#[test]
fn type_post_update_dispatch() {
    transcripts(&points::OBJECT_TYPE_POST_UPDATE, 13);
}

#[test]
fn type_pose_dispatch() {
    transcripts(&points::OBJECT_TYPE_POSE, 13);
}

#[test]
fn type_after_root_dispatch() {
    transcripts(&points::OBJECT_TYPE_AFTER_ROOT, 13);
}

#[test]
fn wake_leaves() {
    for point in [
        &points::OBJECT_WAKE,
        &points::OBJECT_ROOT,
        &points::OBJECT_WAKE_FOR_POST,
        &points::OBJECT_WAKE_CHILDREN,
    ] {
        transcripts(point, 6);
    }
}

#[test]
fn early_mover_wake() {
    transcripts(&points::OBJECT_EARLY_MOVER_WAKE, 6);
}

/// The harness on its own: a run is deterministic, and a changed callee
/// answer shows up as a different transcript, so equal transcripts mean
/// something.
#[test]
fn the_harness_sees_a_difference() {
    let Some((image, ram)) = image() else { return };
    let root = points::OBJECTS_FINALIZE.base;
    let mut code = image.original();
    code.prime(root);
    for body in sim_objects::TABLE.bodies() {
        code.prime(body.point.base);
    }
    let mut h = Harness { code: &mut code, table: &*sim_objects::TABLE, root, case: 1 };
    let (cpu, g) = start(ram, root, 1);
    let a = h.run(cpu, g, &|_| false);
    let (cpu, g) = start(ram, root, 1);
    let b = h.run(cpu, g, &|_| false);
    check("finalize twice", &a, &b);
    assert!(!a.callouts.is_empty(), "finalize makes callouts");
    // The same pool with live slot 5 marked for deletion: finalize's
    // pass B deletes it, two more callouts.
    let (cpu, mut g) = start(ram, root, 1);
    let flags = g.u8(DATA + 12 * 5 + 2).unwrap();
    assert_eq!(flags & 0x10, 0);
    g.set_u8(DATA + 12 * 5 + 2, flags | 0x10).unwrap();
    let c = h.run(cpu, g, &|_| false);
    assert!(
        a.callouts != c.callouts || a.end != c.end,
        "a different pool gave the same transcript"
    );
}
