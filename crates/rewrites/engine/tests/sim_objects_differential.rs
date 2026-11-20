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
//! docs/engine/sim-rewrite-objects.md); function values, the export
//! evaluator, the import dispatcher, every import handler and change
//! colours (slice 3), over synthetic object definitions whose exports,
//! imports and colour functions ask for the image's own function names
//! (read from the instructions that compare them, never written here);
//! the damage update (slice 8); and the small type updates (slice 11),
//! over the pool dressed with the fields they branch on. Every body the
//! root reaches is part of its tree; a callee that is not a body is a
//! callout. The fused finalize run must also record one `ObjectDeleted`
//! event per object it deletes.
//!
//! Every root also compares the float state at every callout and at its
//! return: the eight XMM registers and the whole x87 state (the eight
//! physical registers, TOP and the rest of the status word, the tag
//! word, the control word, FIP, FOP, FDP and their selectors). A root
//! leaves that comparison only through [`FLOAT_OPT_OUT`], with its
//! reason written there. Runs start with stale values in the empty x87
//! registers and last-instruction pointers that no instruction in a root
//! makes, so a body that forgets to update them differs. Stubs for the
//! callees that answer in a float register answer there: `xmm0` for
//! `30bf0` and `1e9700`, a pushed `st(0)` for `24590`.
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
/// The damage information `d5b60` answers with, and the actor array.
const DAMAGE_INFO: u32 = 0x0065_0000;
const ACTOR_ARRAY: u32 = 0x0065_8000;

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
        damage_state(g, body, slot);
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
    damage_info(g);
}

/// The damage update's inputs on one body: the damage flags in every
/// combination the function branches on, vitalities and timers, two
/// damage sections, a player and an actor for some.
fn damage_state(g: &mut Guest, body: u32, slot: u32) {
    let s = slot as usize;
    let f = |v: f32| v.to_bits();
    g.set_u16(body + 0x10a, [0, 0x0200, 0x0010, 0x0004, 0x0860, 0x2000, 0x0008, 0x0400, 0x0a10][s % 9]).unwrap();
    word(g, body + 0xe4, f([0.0, 1.0, 0.5][s % 3]));
    word(g, body + 0xe8, f(if slot % 5 == 0 { 0.0 } else { 1.0 }));
    word(g, body + 0xec, f([0.5, 2.0][s % 2]));
    word(g, body + 0xf0, f([0.25, 1.5, 3.5, 1.0][s % 4]));
    word(g, body + 0xf4, f(0.125));
    word(g, body + 0xfc, f(0.0625));
    g.set_u16(body + 0x104, if slot % 4 == 3 { 5 } else { 0 }).unwrap();
    g.set_u16(body + 0x106, if slot % 4 == 1 { 3 } else { 0 }).unwrap();
    byte(g, body + 0x108, [0, 2, 0x81][s % 3]);
    byte(g, body + 0x109, 1);
    // Two sections at body + 0x180: a record word each (timer bits and
    // a low three-bit field), then a level byte and a timer.
    g.set_u16(body + 0x120, 2 << 3).unwrap();
    g.set_u16(body + 0x122, 0x180).unwrap();
    g.set_u16(body + 0x184, ((slot * 9) & 0x3f) as u16).unwrap();
    g.set_u16(body + 0x18c, 0x0f).unwrap();
    byte(g, body + 0x182, if slot % 2 == 0 { 200 } else { 0 });
    g.set_u16(body + 0x186, (slot % 3) as u16).unwrap();
    byte(g, body + 0x18a, 3);
    word(g, body + 0x12c, if slot % 2 == 0 { u32::MAX } else { slot % 3 });
    word(g, body + 0x13c, if slot % 3 == 0 { u32::MAX } else { 0xe100_0000 | slot });
    g.set_u16(body + 0x138, (slot % 4) as u16).unwrap();
}

/// What `d5b60` points at: two sections with definitions 0x38 apart,
/// recharge rates and the recharge effect.
fn damage_info(g: &mut Guest) {
    let f = |v: f32| v.to_bits();
    word(g, DAMAGE_INFO + 0xbc, 2);
    word(g, DAMAGE_INFO + 0xc0, DAMAGE_INFO + 0x200);
    word(g, DAMAGE_INFO + 0xb8, 0xe300_0005);
    word(g, DAMAGE_INFO + 0xd0, f(0.5));
    word(g, DAMAGE_INFO + 0xd4, f(0.25));
    word(g, DAMAGE_INFO + 0x38, f(1.0));
    word(g, DAMAGE_INFO + 0x200 + 0x2c, f(2.0));
    word(g, DAMAGE_INFO + 0x238 + 0x2c, f(0.0));
    word(g, points::ACTORS.base, ACTOR_ARRAY);
    word(g, ACTOR_ARRAY + 0x44, ACTOR_ARRAY + 0x100);
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
        // Slice 8.
        &points::OBJECT_DAMAGE_UPDATE,
        // Slice 11.
        &points::ITEM_UPDATE,
        &points::SCENERY_UPDATE,
        &points::DEVICE_UPDATE,
        &points::MACHINE_UPDATE,
        &points::MACHINE_SLOT_48,
        &points::PROJECTILE_UPDATE,
        &points::CRATE_UPDATE,
        &points::CREATURE_UPDATE,
        &points::CREATURE_POST_UPDATE,
        // Slice 11, the projectile step.
        &points::PROJECTILE_STEP,
    ]
    .iter()
    .any(|p| p.base == root)
        || (function_root(root) && !object_in_eax(root))
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
    if function_root(root) {
        function_entry(g, cpu, root, case);
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
        0xbc470 | 0xbc820 | 0x109580 | 0x108c60 | 0xbe8e0 | 0xbc5e0 | 0xbd090 | 0xbdef0 => 4,
        t if SLOT_UPDATE.contains(&t) || SLOT_POST.contains(&t) => 4,
        0xb83b0 | 0xb8460 => 8,
        t if SLOT_AFTER_ROOT.contains(&t) => 8,
        0x1d3920 => 12,
        t if SLOT_POSE.contains(&t) => 16,
        0xbef30 => 20,
        0xbb050 => 28,
        0xbba20 | 0xa9570 | 0x1cd4e0 | 0x1ced00 | 0x108cd0 | 0xbe1d0 | 0x109660 | 0xfd910 | 0xbf380 => 0,
        // The damage update's callees (`1588b0` is slice 3's, `d6bc0` and
        // `176780` slice 11's, with the same `ret`).
        0xd9f70 | 0x13a6e8 | 0xa7a30 | 0xd8b50 => 4,
        0xd66d0 => 8,
        0xdbc80 | 0xdbfb0 => 12,
        0xd6800 | 0xba7f0 => 16,
        0xda110 => 20,
        0xd5b60 | 0x1e4a10 | 0x14de90 | 0x1e9720 | 0x138880 => 0,
        // Slice 3's callouts.
        t if FUNCTION_CALLOUTS.iter().any(|&(c, _)| c == t) => {
            FUNCTION_CALLOUTS.iter().find(|&&(c, _)| c == t).unwrap().1
        }
        t if let Some(&(_, pop)) = SLICE_11_CALLEES.iter().find(|c| c.0 == t) => pop,
        t if let Some(&(_, pop)) = STEP_CALLEES.iter().find(|c| c.0 == t) => pop,
        _ => panic!("no stub for callee {target:#x}: add its cleanup"),
    }
}

/// Callouts after which the live set changes: type callbacks, damage,
/// deletion, deferred weapon fire and the early-mover record.
fn mutates(target: u32) -> bool {
    matches!(target, 0xbc470 | 0xbc820 | 0x109580 | 0xb83b0 | 0xdbfb0 | 0xd6800 | 0xfd910 | 0x109660)
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
        0xbc470 | 0x108c60 | 0xbe8e0 | 0x1cd4e0 => n as u32 % 2,
        // Slice 3's callouts that answer in `al`: yes and no by turns.
        t if FUNCTION_ANSWERS_AL.contains(&t) => (n as u32 + case) % 2,
        0xbdef0 => (n as u32 / 2) % 2,
        // The damage update's: its information (none in some calls),
        // yes-or-no answers in `al`, and an overshield owner.
        0xd5b60 => if (n + case as usize) % 4 == 3 { 0 } else { DAMAGE_INFO },
        0xd9f70 | 0x138880 | 0xd8b50 => (seed >> 3) % 2,
        0x14de90 => if n % 2 == 0 { u32::MAX } else { handle(3) },
        // The projectile step's: the attachment handle (none for some),
        // and the collision, flyby and refresh answers in `al`.
        0xfd410 => if (n + case as usize) % 3 == 0 { u32::MAX } else { handle(3) },
        0xfc030 | 0x1897c0 | 0xb9d20 => (n as u32 + case) % 2,
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
    answer_floats(cpu, target, n, case);
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
    /// The XMM registers and the x87 state, unless the root opts out.
    xmm: Option<[u128; 8]>,
    x87: Option<ir::interp::X87>,
}

impl PartialEq for Callout {
    fn eq(&self, o: &Self) -> bool {
        let both = self.defined & o.defined;
        (self.kind, self.target, self.resume, self.gpr, self.ram)
            == (o.kind, o.target, o.resume, o.gpr, o.ram)
            && self.eflags & both == o.eflags & both
            && self.xmm == o.xmm
            && self.x87 == o.x87
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
    let floats = FLOATS.with(std::cell::Cell::get);
    Callout {
        xmm: floats.then_some(cpu.xmm),
        x87: floats.then(|| cpu.x87.clone()),
        kind: t.kind,
        target: t.target_pc,
        resume: t.resume_pc,
        gpr: cpu.gpr,
        eflags: cpu.eflags,
        defined: cpu.flags_valid,
        ram: digest(g, cpu.gpr[4]),
    }
}

std::thread_local! {
    /// Whether the root being run compares the float state.
    static FLOATS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Roots that do not compare the XMM registers and the x87 state, each
/// with the reason. Empty: every root compares them.
const FLOAT_OPT_OUT: &[(u32, &str)] = &[];

/// Whether `root` compares the float state.
fn floats(root: u32) -> bool {
    !FLOAT_OPT_OUT.iter().any(|&(r, _)| r == root)
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
    if function_root(root) {
        definitions(&mut g, case);
    }
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
    seed_x87(&mut cpu.x87);
    (cpu, g)
}

/// The x87 state a run starts with: the stack empty, as at every object
/// update's entry, but the physical registers holding stale values and
/// the last-instruction pointers naming an instruction outside every
/// root. The selectors are those every x87 instruction leaves (`GUEST_CS`,
/// `GUEST_DS`).
fn seed_x87(x: &mut ir::interp::X87) {
    for (i, reg) in x.st.iter_mut().enumerate() {
        *reg = [0x11 * (i as u8 + 1); 10];
        reg[9] = 0x3f;
    }
    x.instruction_pointer = 0x0012_3450;
    x.opcode = 0x1d9;
    x.data_pointer = 0x0060_0ff0;
    x.instruction_selector = 0x8;
    x.data_selector = 0x10;
}

/// Callees that answer in a float register (docs/engine/object-update.md,
/// "Slice 11 bodies"): the length `30bf0` leaves in `xmm0` after
/// normalising, the engine scale `1e9700` returns in `xmm0`, and the
/// vector length `24590` pushes on the x87 stack.
const XMM0_ANSWERS: &[u32] = &[0x30bf0, 0x1e9700];
const ST0_ANSWERS: &[u32] = &[0x24590];

/// The float answer of a stub, the same in every run: zero, small and
/// large lengths, by the call's index and the case.
fn float_answer(target: u32, n: usize, case: u32) -> f32 {
    let k = (n + case as usize + target as usize) % 4;
    [0.0, 0.75, 2.5, 1.0e-4][k]
}

fn answer_floats(cpu: &mut Cpu, target: u32, n: usize, case: u32) {
    if XMM0_ANSWERS.contains(&target) {
        cpu.xmm[0] = u128::from(float_answer(target, n, case).to_bits());
    }
    if ST0_ANSWERS.contains(&target) {
        let x = &mut cpu.x87;
        let mut a = x87::abi::X87Abi {
            st: x.st,
            control: x.control,
            status: x.status,
            tag: x.tag,
            opcode: x.opcode,
            instruction_pointer: x.instruction_pointer,
            data_pointer: x.data_pointer,
            instruction_selector: x.instruction_selector,
            data_selector: x.data_selector,
        };
        x87::abi::ld_f32(&mut a, float_answer(target, n, case).to_bits());
        x.st = a.st;
        x.status = a.status;
        x.tag = a.tag;
    }
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
    FLOATS.with(|x| x.set(floats(point.base)));
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

#[test]
fn object_damage_update() {
    transcripts(&points::OBJECT_DAMAGE_UPDATE, 6);
}

// Slice 3: function values, the imports and change colours.

/// Slice 3's roots, each entered directly.
const FUNCTION_ROOTS: &[&points::Point] = &[
    &points::OBJECT_FUNCTION_VALUE,
    &points::OBJECT_FUNCTION_EVALUATE,
    &points::OBJECT_FUNCTION_IMPORT,
    &points::BASE_OBJECT_IMPORT,
    &points::OBJECT_CHANGE_COLORS,
    &points::UNIT_FUNCTION_IMPORT,
    &points::UNIT_IMPORT_FRACTION,
    &points::UNIT_IMPORT_ANIMATION,
    &points::BIPED_FUNCTION_IMPORT,
    &points::VEHICLE_FUNCTION_IMPORT,
    &points::WEAPON_FUNCTION_IMPORT,
    &points::PROJECTILE_FUNCTION_IMPORT,
    &points::DEVICE_FUNCTION_IMPORT,
    &points::MACHINE_FUNCTION_IMPORT,
];

fn function_root(root: u32) -> bool {
    FUNCTION_ROOTS.iter().any(|p| p.base == root)
}

/// The two unit helpers take the unit in `eax`.
fn object_in_eax(root: u32) -> bool {
    root == points::UNIT_IMPORT_FRACTION.base || root == points::UNIT_IMPORT_ANIMATION.base
}

/// Guest functions slice 3's bodies call, and their `ret N`.
const FUNCTION_CALLOUTS: &[(u32, u32)] = &[
    (0x10aac0, 12),
    (0x13bb90, 8),
    (0x131c20, 0),
    (0xb8bd0, 4),
    (0xbff60, 4),
    (0xbca70, 0),
    (0xbcbb0, 0),
    (0xbcad0, 0),
    (0xbcb60, 0),
    (0xbcb10, 0),
    (0x1588b0, 4),
    (0xc18e0, 0),
    (0xbc970, 4),
    (0x102070, 4),
    (0x10cf50, 0),
    (0x162b10, 0),
    (0x162b70, 4),
    (0x1c6e30, 0),
    (0x1d3550, 4),
    (0xf5d70, 0),
    (0xe6fe0, 0),
    (0xf78e0, 4),
    (0xf78b0, 8),
    (0x24590, 0),
    (0xf8070, 4),
    (0x11d580, 0),
];

/// Those that answer in `al`.
const FUNCTION_ANSWERS_AL: &[u32] = &[0x10aac0, 0xbc970, 0x10cf50, 0x162b10, 0x1d3550, 0xf5d70, 0xe6fe0, 0xf8070];

/// Synthetic definitions: a tag table of four object definitions and a
/// model, export records, colour blocks and colour functions.
const TAG_TABLE: u32 = 0x0064_0000;
const DEFS: u32 = 0x0064_1000;
const DEF_BYTES: u32 = 0x400;
const MODEL_TAG: u32 = 0x0064_2000;
const EXPORT_RECORDS: u32 = 0x0064_3000;
const COLOR_BLOCKS: u32 = 0x0064_4000;
const COLOR_FUNCTIONS: u32 = 0x0064_5000;
const INPUT_KINDS: u32 = 0x0064_6000;
/// The value and active outputs a root writes.
const VALUE_OUT: u32 = 0x0060_0f00;
const ACTIVE_OUT: u32 = 0x0060_0f10;
/// Synthetic names: an export every definition has, one only some have,
/// one nothing has.
const EXPORT_NAME: u32 = 0x0abc_0001;
const OTHER_NAME: u32 = 0x0abc_0002;
const UNKNOWN_NAME: u32 = 0x0abc_0003;

/// Where the image keeps the function names the bodies compare: each
/// name is the sum of the imm32s at `.0` (the `cmp reg, imm32` or
/// `sub reg, imm32` that tests it), the sign-extended imm8s at `.1` (a
/// `sub reg, imm8` step between two names) and `.2`. The names
/// themselves are read from the image at run time; none is written here.
const NAMES: &[(&[u32], &[u32], u32)] = &[
    // bab40: one, zero.
    (&[0xbabab], &[], 0),
    (&[0xbabb7], &[], 0),
    // bcc20, including the two `sub` chains and the two ranges' tops.
    (&[0xbcc48], &[], 0),
    (&[0xbcc67], &[], 0),
    (&[0xbcc78], &[], 0),
    (&[0xbcc85], &[], 0),
    (&[0xbcc90], &[], 0),
    (&[0xbcc9b], &[], 0),
    (&[0xbcd13], &[], 0),
    (&[0xbcd1a], &[], 0),
    (&[0xbcd21], &[], 0),
    (&[0xbcdb3], &[], 0),
    (&[0xbcdbc], &[], 0),
    (&[0xbcdbc, 0xbcdc7], &[], 0),
    (&[0xbcdbc, 0xbcdc7], &[], 1),
    (&[0xbcdfc], &[], 0),
    (&[0xbce07], &[], 0),
    (&[0xbce55], &[], 0),
    (&[0xbce66], &[], 0),
    (&[0xbce77], &[], 0),
    (&[0xbce7e], &[], 0),
    (&[0xbce85], &[], 0),
    (&[0xbcf04], &[], 0),
    (&[0xbcf12], &[], 0),
    (&[0xbcf46], &[], 0),
    (&[0xbcf52], &[], 0),
    (&[0xbcf52, 0xbcf5a], &[], 0),
    (&[0xbcf52, 0xbcf5a], &[], 1),
    (&[0xbcf8a], &[], 0),
    (&[0xbcf98], &[], 0),
    // c6b90, its `sub` chain and the two names it passes to the weapon.
    (&[0xc6bb2], &[], 0),
    (&[0xc6bc7], &[], 0),
    (&[0xc6bd1], &[], 0),
    (&[0xc6bd9], &[], 0),
    (&[0xc6be1], &[], 0),
    (&[0xc6c31], &[], 0),
    (&[0xc6c39], &[], 0),
    (&[0xc6c41], &[], 0),
    (&[0xc6cc8], &[], 0),
    (&[0xc6cd3], &[], 0),
    (&[0xc6cd3, 0xc6cda], &[], 0),
    (&[0xc6cd3, 0xc6cda], &[], 1),
    (&[0xc6d29], &[], 0),
    (&[0xc6d31], &[], 0),
    (&[0xc6d39], &[], 0),
    (&[0xc6d43], &[], 0),
    (&[0xc6d4c], &[], 0),
    // de3b0.
    (&[0xde3f1], &[], 0),
    (&[0xde3fc], &[], 0),
    (&[0xde404], &[], 0),
    // ff5f0, its two `sub` chains.
    (&[0xff632], &[], 0),
    (&[0xff649], &[], 0),
    (&[0xff655], &[], 0),
    (&[0xff661], &[], 0),
    (&[0xff66d], &[], 0),
    (&[0xff7b2], &[], 0),
    (&[0xff7c4], &[], 0),
    (&[0xff7ce], &[], 0),
    (&[0xff7d6], &[], 0),
    (&[0xff7de], &[], 0),
    (&[0xff82d], &[], 0),
    (&[0xff835], &[], 0),
    (&[0xff8d8], &[], 0),
    (&[0xff8e2], &[], 0),
    (&[0xff8ee], &[], 0),
    (&[0xff8f6], &[], 0),
    (&[0xff90c], &[], 0),
    (&[0xff918], &[], 0),
    (&[0xff955], &[], 0),
    (&[0xff977], &[], 0),
    (&[0xff9a1], &[], 0),
    (&[0xff9d2], &[], 0),
    (&[0xffa17], &[], 0),
    (&[0xffa29], &[], 0),
    (&[0xffa31], &[], 0),
    (&[0xffa39], &[], 0),
    (&[0xffa8a], &[], 0),
    (&[0xffab3], &[], 0),
    (&[0xffafe], &[], 0),
    (&[0xffafe], &[0xffb0a], 0),
    (&[0xffb37], &[], 0),
    (&[0xffb37], &[0xffb3f], 0),
    (&[0xffb37, 0xffb43], &[0xffb3f], 0),
    (&[0xffba4], &[], 0),
    (&[0xffbff], &[], 0),
    // fbe70.
    (&[0xfbea5], &[], 0),
    (&[0xfbeb1], &[], 0),
    (&[0xfbeb8], &[], 0),
    (&[0xfbf05], &[], 0),
    (&[0xfbf0c], &[], 0),
    // 106d70.
    (&[0x106dad], &[], 0),
    (&[0x106dc4], &[], 0),
    (&[0x106dcb], &[], 0),
    (&[0x106dd2], &[], 0),
    (&[0x106ec9], &[], 0),
    (&[0x106ed4], &[], 0),
    (&[0x106edf], &[], 0),
    // f6d10.
    (&[0xf6d51], &[], 0),
    (&[0xf6d66], &[], 0),
    (&[0xf6d78], &[], 0),
    (&[0xf6d8a], &[], 0),
    (&[0xf6d94], &[], 0),
    (&[0xf6d9c], &[], 0),
    (&[0xf6e03], &[], 0),
    (&[0xf6e0b], &[], 0),
    (&[0xf6e58], &[], 0),
    (&[0xf6e66], &[], 0),
    (&[0xf6e66], &[0xf6e6e], 0),
    (&[0xf6eec], &[], 0),
    (&[0xf6f59], &[], 0),
    (&[0xf6f6b], &[], 0),
    (&[0xf6f7d], &[], 0),
    (&[0xf6f89], &[], 0),
    (&[0xf7091], &[], 0),
    (&[0xf71c2], &[], 0),
    (&[0xf721c], &[], 0),
    (&[0xf722e], &[], 0),
    (&[0xf722e], &[0xf7236], 0),
    (&[0xf7320], &[], 0),
    (&[0xf73ea], &[], 0),
    (&[0xf73fc], &[], 0),
    (&[0xf740e], &[], 0),
    (&[0xf7420], &[], 0),
    (&[0xf7420], &[0xf7428], 0),
    (&[0xf756c], &[], 0),
    (&[0xf7621], &[], 0),
    (&[0xf7633], &[], 0),
    (&[0xf7633], &[0xf763b], 0),
    (&[0xf7734], &[], 0),
    (&[0xf776d], &[], 0),
    (&[0xf777b], &[], 0),
    (&[0xf7789], &[], 0),
    (&[0xf7791], &[], 0),
    (&[0xf77f8], &[], 0),
    (&[0xf780c], &[], 0),
    (&[0xf7816], &[], 0),
    (&[0xf7816], &[0xf781e], 0),
    (&[0xf7823], &[], 0),
];

fn image_name(g: &Guest, (imm32, imm8, plus): (&[u32], &[u32], u32)) -> u32 {
    let mut name = plus;
    for &at in imm32 {
        name = name.wrapping_add(g.u32(at).unwrap());
    }
    for &at in imm8 {
        name = name.wrapping_add(g.u8(at).unwrap() as i8 as i32 as u32);
    }
    name
}

/// Names a root or a record asks for: none, the synthetic ones, then
/// every image name in turn.
fn pick(g: &Guest, k: u32) -> u32 {
    let synthetic = [0, EXPORT_NAME, OTHER_NAME, UNKNOWN_NAME];
    let k = k as usize % (synthetic.len() + NAMES.len());
    if k < synthetic.len() {
        return synthetic[k];
    }
    image_name(g, NAMES[k - synthetic.len()])
}

/// How many cases a slice 3 root runs: every name twice (`pick(case / 2)`),
/// over the six pool shapes.
fn function_cases() -> u32 {
    2 * (4 + NAMES.len() as u32)
}

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// Definitions, exports and colours for `case`, over the slice 1 pool.
fn definitions(g: &mut Guest, case: u32) {
    word(g, points::TAG_ENTRIES.base, TAG_TABLE);
    for i in 0..4 {
        word(g, TAG_TABLE + 16 * i + 8, DEFS + DEF_BYTES * i);
    }
    word(g, TAG_TABLE + 16 * 4 + 8, MODEL_TAG);
    word(g, MODEL_TAG + 0x50, case % 4);
    byte(g, INPUT_KINDS, 3);
    byte(g, INPUT_KINDS + 1, 0);
    for i in 0..4 {
        let def = DEFS + DEF_BYTES * i;
        // Change colours on for two of the four.
        byte(g, def + 0x1c, (i & 1) as u8);
        word(g, def + 0x38, 4);
        // Zero to three exports; the second never matches `EXPORT_NAME`.
        word(g, def + 0x64, i);
        word(g, def + 0x68, EXPORT_RECORDS + 0x100 * i);
        for j in 0..3 {
            let r = EXPORT_RECORDS + 0x100 * i + 0x20 * j;
            let k = case + j;
            word(g, r, k & 0xf);
            let input = pick(g, k + i);
            word(g, r + 4, input);
            word(g, r + 8, if j == 1 { OTHER_NAME } else { EXPORT_NAME });
            let gate = if k % 3 == 0 { 0 } else { pick(g, k + 7) };
            word(g, r + 0xc, gate);
            word(g, r + 0x10, [f(0.0), f(0.5), f(-1.0)][k as usize % 3]);
            word(g, r + 0x18, INPUT_KINDS + k % 2);
            // A scale from another value; never the record's own name, so
            // the evaluation ends.
            word(g, r + 0x1c, if j != 1 && k % 4 == 1 { OTHER_NAME } else { 0 });
        }
        word(g, def + 0xb0, COLOR_BLOCKS + 0x100 * i);
        for k in 0..3 {
            let block = COLOR_BLOCKS + 0x100 * i + 0x10 * k;
            word(g, block + 8, (case + k) % 3);
            let functions = COLOR_FUNCTIONS + 0x100 * (3 * i + k);
            word(g, block + 0xc, functions);
            for m in 0..2 {
                let rec = functions + 0x28 * m;
                word(g, rec + 4, case + m);
                word(g, rec + 0x20, if (case + m) % 3 == 0 { 0 } else { pick(g, case + m + 1) });
                word(g, rec + 0x24, if (case + m + k) % 2 == 0 { EXPORT_NAME } else { pick(g, case + k) });
            }
        }
    }
    for slot in 0..SLOTS {
        let body = RECORDS + BODY * slot;
        word(g, body, (slot + case) % 4);
        // Children (slots 6 and 7 under 2 in cases 3 to 5) take their
        // parent's value for a name they lack.
        if slot == 6 || slot == 7 {
            word(g, body + 4, 1 << 26);
        }
        // The scripted override for some.
        let c1 = g.u8(body + 0xc1).unwrap();
        byte(g, body + 0xc1, c1 | u8::from(slot % 7 == 3) << 1);
        // Up to two change colours, after a base RGB per colour.
        g.set_u16(body + 0x124, (24 * ((slot + case) % 3)) as u16).unwrap();
        g.set_u16(body + 0x126, 0x160).unwrap();
        for c in 0..6u32 {
            word(g, body + 0x160 + 4 * c, [f(0.25), f(1.5), f(-0.5)][((c + slot) % 3) as usize]);
        }
        // Vitality and shields, some out of range.
        for (n, off) in [0xec, 0xf0, 0xf4, 0xf8].into_iter().enumerate() {
            word(g, body + off, [f(-1.0), f(0.0), f(0.375), f(2.5), f(1.0)][(n + (slot + case) as usize) % 5]);
        }
        byte(g, body + 0xb1, (slot + case) as u8);
        byte(g, body + 0x10a, (slot * 3 + case) as u8);
    }
}

/// Arguments for slice 3's roots: the name, the outputs, and the record
/// for the export evaluator.
fn function_entry(g: &mut Guest, cpu: &mut Cpu, root: u32, case: u32) {
    let name = pick(g, case / 2);
    word(g, VALUE_OUT, 0xdead_beef);
    word(g, ACTIVE_OUT, 0xdead_beef);
    if object_in_eax(root) {
        cpu.gpr[0] = object(case);
        return;
    }
    if root == points::OBJECT_CHANGE_COLORS.base {
        return;
    }
    if root == points::OBJECT_FUNCTION_EVALUATE.base {
        // A record of the object's definition, the value out.
        let def = (object(case) as u16 as u32 + case) % 4;
        cpu.gpr[0] = EXPORT_RECORDS + 0x100 * def + 0x20 * (case % 3);
        word(g, STACK + 8, VALUE_OUT);
        return;
    }
    word(g, STACK + 8, if root == points::OBJECT_FUNCTION_VALUE.base && case % 3 == 0 { EXPORT_NAME } else { name });
    if root == points::OBJECT_FUNCTION_IMPORT.base {
        cpu.gpr[3] = VALUE_OUT;
        cpu.gpr[7] = ACTIVE_OUT;
        return;
    }
    word(g, STACK + 12, VALUE_OUT);
    word(g, STACK + 16, ACTIVE_OUT);
}

#[test]
fn function_value() {
    transcripts(&points::OBJECT_FUNCTION_VALUE, function_cases());
}

#[test]
fn function_evaluate() {
    transcripts(&points::OBJECT_FUNCTION_EVALUATE, function_cases());
}

#[test]
fn function_import_dispatch() {
    transcripts(&points::OBJECT_FUNCTION_IMPORT, function_cases());
}

#[test]
fn base_object_import() {
    transcripts(&points::BASE_OBJECT_IMPORT, function_cases());
}

#[test]
fn change_colors() {
    transcripts(&points::OBJECT_CHANGE_COLORS, function_cases());
}

#[test]
fn type_imports() {
    for point in [
        &points::UNIT_FUNCTION_IMPORT,
        &points::UNIT_IMPORT_FRACTION,
        &points::UNIT_IMPORT_ANIMATION,
        &points::BIPED_FUNCTION_IMPORT,
        &points::VEHICLE_FUNCTION_IMPORT,
        &points::WEAPON_FUNCTION_IMPORT,
        &points::PROJECTILE_FUNCTION_IMPORT,
        &points::DEVICE_FUNCTION_IMPORT,
        &points::MACHINE_FUNCTION_IMPORT,
    ] {
        transcripts(point, function_cases());
    }
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

// Slice 11: the small type updates (item, scenery, device, machine,
// crate, creature, projectile). Their roots run over the same pool,
// dressed per case with a definition, a game-time record and the fields
// each update branches on; with the full body table, so the dispatchers
// and walks above reach them too.

/// Every guest function a slice 11 body calls, with the bytes its `ret`
/// pops (docs/engine/object-update.md, "Slice 11 bodies").
const SLICE_11_CALLEES: &[(u32, u32)] = &[
    // Item `10b4d0`.
    (0x184000, 0),
    (0xbadc0, 4),
    // `b8bd0` (`ret 4`) is in `FUNCTION_CALLOUTS`.
    (0x142570, 4),
    (0x10d710, 8),
    (0x10cf80, 8),
    (0x10d5f0, 0),
    (0xbc190, 0),
    (0xb9dd0, 0),
    (0xd6660, 4),
    (0xba300, 0),
    (0xd6c80, 8),
    (0x176780, 16),
    (0xb8540, 4),
    (0x16eb20, 4),
    (0x10b850, 4),
    // Scenery `10a2f0`.
    (0x1cb710, 20),
    (0x1daea0, 4),
    // Crate `11bce0`, machine `1152e0`.
    (0x1d24a0, 8),
    // Projectile `f8de0`.
    (0x109a00, 12),
    (0xf8eb0, 8),
    // Machine `1152e0` and device `106790`.
    (0x1c6620, 4),
    (0x1c6920, 16),
    (0xb58c0, 8),
    (0x107520, 4),
    (0xb8d30, 12),
    (0xb9fc0, 0),
    (0x1071e0, 8),
    (0xbd020, 4),
    (0x115b10, 4),
    (0x115c70, 0),
    (0x107cc0, 12),
    (0x107980, 0),
    (0x11eed0, 20),
    (0xb6760, 0),
    // Creature `118700` and `118b80`.
    (0x118f90, 0),
    (0x119380, 8),
    (0x1197b0, 4),
    (0xba1d0, 4),
    (0x1e5bb0, 84),
    (0x1e6360, 8),
    (0x1e5af0, 4),
    (0x1e6120, 16),
    (0x1e55d0, 8),
    (0x119bc0, 0),
    (0x1c4b00, 8),
    (0x119500, 4),
    (0x1195c0, 0),
    (0x119c10, 4),
    (0x11a140, 0),
    (0xd6bc0, 4),
    (0x1ec500, 0),
    (0x118b10, 0),
    (0x1d1c10, 0),
    (0x1e68a0, 40),
    (0x1e5a60, 0),
];

/// The synthetic definition every pool object points at (tag index 0),
/// the game-time record, and `115a40`'s cluster tables and arguments.
const TAGS: u32 = 0x0064_0000;
const DEFINITION: u32 = 0x0064_1000;
const TIME: u32 = 0x0064_2000;
const STRUCTURE: u32 = 0x0064_3000;
const CLUSTER_LIST: u32 = 0x0064_3400;
const CLUSTER_INDICES: u32 = 0x0064_3800;
const CLUSTER_RECORDS: u32 = 0x0064_4000;
const VISIBLE: u32 = 0x0064_5000;
const ANSWER: u32 = 0x0064_5100;
/// Holds the structure the machine's cluster test reads (`+60` the
/// records, `+220` the index list).
const STRUCTURE_GLOBAL: u32 = 0x004e_0348;
/// Where the stubs' `eax` answers point (`answer`): `0x6e1000` plus a
/// byte.
const ANSWERS: u32 = 0x006e_1000;

fn f32w(g: &mut Guest, a: u32, v: f32) {
    word(g, a, v.to_bits());
}

/// The fields the slice 11 updates branch on, varied by `case`, on the
/// object a root is called on and its definition.
fn dress(g: &mut Guest, case: u32) {
    word(g, points::TAG_ENTRIES.base, TAGS);
    word(g, TAGS + 8, DEFINITION);
    word(g, points::GAME_TIME.base, TIME);
    g.set_u16(TIME + 2, 30).unwrap();
    f32w(g, TIME + 4, 1.0 / 30.0);
    word(g, TIME + 8, 0x1000 + case);
    // Definition: item `+bc` bit 2, `+104` positive or not, `+110` an
    // effect or none, `+128`; creature `+d4` bits 3 and 4.
    word(g, DEFINITION + 0xbc, if case % 3 == 2 { 4 } else { 0 });
    word(g, DEFINITION + 0x104, [0, 1, 0, 2, 0, 3][case as usize % 6]);
    word(g, DEFINITION + 0x110, if case % 2 == 0 { u32::MAX } else { 0x00e3_0001 });
    word(g, DEFINITION + 0x128, 0x00e4_0002);
    word(g, DEFINITION + 0xd4, [0, 8, 0x10, 0x18][case as usize % 4]);

    let body = g.u32(DATA + 12 * (object(case) & 0xffff) + 8).unwrap();
    // In the world (body `+04` bit 8) for most cases.
    word(g, body + 4, if case == 4 { 0 } else { 1 << 8 });
    // Woken by an early mover (`+c1` bit 0), crate component flag (`+c0`
    // bit 12), and a physics component for some.
    byte(g, body + 0xc1, [0, 1, 0x11, 0x10, 1, 0x11][case as usize % 6]);
    word(g, body + 0xb4, if case % 2 == 1 { case } else { u32::MAX });
    // Velocity and its damping.
    for (i, v) in [1.5f32, -0.25, 3.0].into_iter().enumerate() {
        f32w(g, body + 0x88 + 4 * i as u32, v * (case + 1) as f32);
        f32w(g, body + 0x94 + 4 * i as u32, v);
    }
    // Type flags `+12c` (item bits 4 to 6, projectile bits 14 and 15,
    // creature bits 5 and 7), the item timer `+12e`, the creature
    // counters `+130`, `+131`.
    word(g, body + 0x12c, [0x10, 0x20, 0x40, 0x4000, 0x8030, 0xc060][case as usize % 6]);
    g.set_u16(body + 0x12e, [0, 1, 5, 1, 0, 2][case as usize % 6]).unwrap();
    byte(g, body + 0x130, [0, 3, 0xfe, 1, 0, 9][case as usize % 6]);
    byte(g, body + 0x131, [0xfe, 0, 2, 7, 0, 1][case as usize % 6]);
    word(g, body + 0x13c, if case % 3 == 1 { handle(3) } else { u32::MAX });
    // Scenery: an animation block at `+180` in cases 1 to 5, its state
    // `+68`, tag `+0` and word `+6` none in turn.
    g.set_u16(body + 0x12a, if case == 0 { u16::MAX } else { 0x180 }).unwrap();
    let block = body + 0x180;
    word(g, block, if case == 2 { u32::MAX } else { 0 });
    word(g, block + 4, 0x00e5_0003);
    g.set_u16(block + 6, if case == 3 { u16::MAX } else { 1 }).unwrap();
    word(g, block + 0x68, if case == 1 { u32::MAX } else { 0 });
    // `115a40`: a range of entries at `+1da`/`+1dc`, each two cluster
    // indices; the visible-cluster bits; the answer byte.
    g.set_u16(body + 0x1da, 1).unwrap();
    g.set_u16(body + 0x1dc, [0, 1, 2, 3, 2, 1][case as usize % 6]).unwrap();
    word(g, STRUCTURE_GLOBAL, STRUCTURE);
    word(g, STRUCTURE + 0x220, CLUSTER_LIST);
    word(g, CLUSTER_LIST + 0xc, CLUSTER_INDICES);
    word(g, STRUCTURE + 0x60, CLUSTER_RECORDS);
    for i in 0..4u32 {
        g.set_u16(CLUSTER_INDICES + 2 * i, (3 - i) as u16).unwrap();
        g.set_u16(CLUSTER_RECORDS + 0x24 * i, (7 * i + case) as u16).unwrap();
        g.set_u16(CLUSTER_RECORDS + 0x24 * i + 2, (40 + i) as u16).unwrap();
    }
    word(g, VISIBLE, [0, 1 << 4, 1 << 9, 0, 1 << 2, 0][case as usize % 6]);
    word(g, VISIBLE + 4, if case == 5 { 1 << 9 } else { 0 });
    byte(g, ANSWER, 0);
    word(g, STACK + 8, VISIBLE);
    word(g, STACK + 12, ANSWER);
    // A record a stub answer points at: its `+14` word above 1 in odd
    // cases (the scenery's lookup then wakes it for the post walk).
    let fill = if case % 2 == 1 { 2 } else { 0 };
    g.write_bytes(ANSWERS, &[fill; 0x120], 1).unwrap();
}

/// [`transcripts`] over a [`dress`]ed pool.
fn dressed(point: &points::Point, cases: u32) {
    let Some((image, ram)) = image() else { return };
    let table: &Table = &*sim_objects::TABLE;
    let mut code = image.original();
    code.prime(point.base);
    for body in table.bodies() {
        code.prime(body.point.base);
    }
    assert!(code.insn(point.base).is_some(), "{}", point.name);
    assert!(table.owns(point.base), "{} is a body", point.name);
    FLOATS.with(|x| x.set(floats(point.base)));
    let mut h = Harness { code: &mut code, table, root: point.base, case: 0 };
    for case in 0..cases {
        h.case = case;
        let fresh = || {
            let (cpu, mut g) = start(ram, point.base, case);
            dress(&mut g, case);
            (cpu, g)
        };
        let (cpu, g) = fresh();
        let reference = h.run(cpu, g, &|_| false);
        let (cpu, g) = fresh();
        let fused = h.run(cpu, g, &|_| true);
        check(&format!("{} case {case} fused", point.name), &reference, &fused);
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
fn item_update() {
    dressed(&points::ITEM_UPDATE, 6);
}

#[test]
fn scenery_update() {
    dressed(&points::SCENERY_UPDATE, 6);
}

#[test]
fn device_update() {
    dressed(&points::DEVICE_UPDATE, 6);
}

#[test]
fn machine_update() {
    dressed(&points::MACHINE_UPDATE, 6);
}

#[test]
fn machine_slot_48() {
    dressed(&points::MACHINE_SLOT_48, 6);
}

#[test]
fn projectile_update() {
    dressed(&points::PROJECTILE_UPDATE, 6);
}

#[test]
fn crate_update() {
    dressed(&points::CRATE_UPDATE, 6);
}

#[test]
fn creature_update() {
    dressed(&points::CREATURE_UPDATE, 6);
}

#[test]
fn creature_post_update() {
    dressed(&points::CREATURE_POST_UPDATE, 6);
}

/// The dispatcher reaches each slice 11 body by its type's slot: every
/// type, over the dressed pool.
#[test]
fn type_update_dispatch_reaches_the_small_types() {
    dressed(&points::OBJECT_TYPE_UPDATE, 13);
    dressed(&points::OBJECT_TYPE_POST_UPDATE, 13);
}

// Slice 11, the projectile step `f8eb0`: entered directly over a pool
// dressed for it, and through the projectile update, which fuses it.

/// Every guest function the projectile step calls that no table above
/// lists, with the bytes its `ret` pops (`1e9700` through its tail jump
/// to `1e96a0`).
const STEP_CALLEES: &[(u32, u32)] = &[
    (0xfd410, 0),
    (0x17c880, 0),
    (0x16ba40, 0),
    (0xfd460, 4),
    (0x17bbd0, 8),
    (0x1e9700, 4),
    (0xf87f0, 4),
    (0x30bf0, 0),
    (0xb75a0, 12),
    (0xfaa30, 0),
    (0xfc030, 4),
    (0x1ca9f0, 8),
    (0xfaa60, 20),
    (0x1ca690, 20),
    (0x1897c0, 16),
    (0x11d000, 0),
    (0xb7740, 4),
    (0xb9b90, 4),
    (0xb9d20, 0),
    (0xb8b70, 0),
    (0xfc330, 12),
];

/// The gravity the step reads through `[51e9c4]`.
const STEP_GRAVITY: u32 = 0x0064_5200;
const GRAVITY_OWNER: u32 = 0x0051_e9c4;

/// The fields the projectile step branches on, on the object a root is
/// called on and its definition, over a [`dress`]ed pool: the flags that
/// skip the attachment, the loop and the drag; the state word `+130`;
/// a target to home on or none; the arming and age timers; the spin; the
/// definition's kind word, its orientation and homing flags, drag,
/// gravity scales and the flyby sound.
fn dress_step(g: &mut Guest, case: u32) {
    let k = case as usize;
    let body = g.u32(DATA + 12 * (object(case) & 0xffff) + 8).unwrap();
    word(g, GRAVITY_OWNER, STEP_GRAVITY);
    f32w(g, STEP_GRAVITY, 4.5);
    word(g, body + 0x12c, [0x0, 0x2, 0x101, 0x20, 0x8, 0x4000, 0x1, 0x102][k % 8]);
    byte(g, body + 0xc1, u8::from(k % 8 == 4));
    g.set_u16(body + 0x130, [0, 0, 1, 0, 2, 1, 0, 2][k % 8]).unwrap();
    word(g, body + 0x4, (1 << 8) | (u32::from(k % 2 == 1) << 3));
    word(g, body + 0x144, if k % 3 == 0 { u32::MAX } else { handle(3) });
    word(g, body + 0x14c, 0);
    for (off, v) in [
        (0x158, 0.25f32),
        (0x15c, 0.5),
        (0x160, [0.25, 2.0][k % 2]),
        (0x164, [0.0, 0.125][(k / 2) % 2]),
        (0x168, 3.0),
        (0x16c, 0.75),
        (0x170, [1.0, 10.0][(k / 2) % 2]),
        (0x174, 0.5),
        (0x178, -0.25),
        (0x17c, 0.75),
        (0x180, 0.125),
        (0x184, 0.875),
        (0x18c, [0.0, 0.5][k % 2]),
        (0x190, 1.5),
        (0x194, 2.0),
    ] {
        f32w(g, body + off, v);
    }
    g.set_u16(body + 0x1aa, 7).unwrap();
    word(g, body + 0xc4, 0x00e6_0004);
    for (i, v) in [1.0f32, 2.0, 3.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0].into_iter().enumerate() {
        f32w(g, body + 0x64 + 4 * i as u32, v);
    }
    word(g, DEFINITION + 0xbc, [0x1, 0x0, 0x101, 0x40, 0x1, 0x141][k % 6]);
    g.set_u16(DEFINITION + 0xc0, (k % 4) as u16).unwrap();
    g.set_u16(DEFINITION + 0xc2, 3).unwrap();
    word(g, DEFINITION + 0x13c, if k % 2 == 0 { u32::MAX } else { 0x00e7_0005 });
    for (off, v) in [
        (0xd8, [0.0f32, 0.5][k % 2]),
        (0xe0, [0.0, 5.0, 2.0][k % 3]),
        (0x164, 1.0),
        (0x170, 0.5),
        (0x17c, [0.0, 0.25][(k / 3) % 2]),
        (0x180, 1.0),
        (0x18c, 0.5),
        (0x194, 0.25),
    ] {
        f32w(g, DEFINITION + off, v);
    }
}

/// [`dressed`] with [`dress_step`] on top.
fn dressed_step(point: &points::Point, cases: u32) {
    let Some((image, ram)) = image() else { return };
    let table: &Table = &*sim_objects::TABLE;
    let mut code = image.original();
    code.prime(point.base);
    for body in table.bodies() {
        code.prime(body.point.base);
    }
    assert!(table.owns(point.base), "{} is a body", point.name);
    FLOATS.with(|x| x.set(floats(point.base)));
    let mut h = Harness { code: &mut code, table, root: point.base, case: 0 };
    for case in 0..cases {
        h.case = case;
        let fresh = || {
            let (cpu, mut g) = start(ram, point.base, case);
            dress(&mut g, case);
            dress_step(&mut g, case);
            (cpu, g)
        };
        let (cpu, g) = fresh();
        let reference = h.run(cpu, g, &|_| false);
        let (cpu, g) = fresh();
        let fused = h.run(cpu, g, &|_| true);
        check(&format!("{} case {case} fused", point.name), &reference, &fused);
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
fn projectile_step() {
    dressed_step(&points::PROJECTILE_STEP, 12);
}

/// The projectile update reaches the step by a fused call.
#[test]
fn projectile_update_fuses_the_step() {
    dressed_step(&points::PROJECTILE_UPDATE, 12);
}

/// The float comparison on its own: a run that differs from another only
/// in an XMM register nobody writes, or in the x87 last-instruction
/// pointer, gives a different transcript.
#[test]
fn the_harness_sees_a_float_difference() {
    let Some((image, ram)) = image() else { return };
    let root = points::OBJECTS_FINALIZE.base;
    let mut code = image.original();
    code.prime(root);
    for body in sim_objects::TABLE.bodies() {
        code.prime(body.point.base);
    }
    FLOATS.with(|x| x.set(floats(root)));
    let mut h = Harness { code: &mut code, table: &*sim_objects::TABLE, root, case: 1 };
    let (cpu, g) = start(ram, root, 1);
    let a = h.run(cpu, g, &|_| false);
    assert!(!a.callouts.is_empty(), "finalize makes callouts");
    let (mut cpu, g) = start(ram, root, 1);
    cpu.xmm[5] ^= 1;
    let b = h.run(cpu, g, &|_| false);
    assert_ne!(a.callouts[0], b.callouts[0], "an XMM register is compared");
    let (mut cpu, g) = start(ram, root, 1);
    cpu.x87.instruction_pointer ^= 0x10;
    let c = h.run(cpu, g, &|_| false);
    assert_ne!(a.callouts[0], c.callouts[0], "the x87 state is compared");
}
