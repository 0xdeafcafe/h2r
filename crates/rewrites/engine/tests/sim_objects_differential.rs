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
        // Slice 4: the node matrices `bd090` writes (`+116`) at the
        // body's end, not over its links, and no orientation block.
        g.set_u16(body + 0x116, 0x1c0).unwrap();
        g.set_u16(body + 0x112, u16::MAX).unwrap();
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
        // Slice 9.
        &points::WEAPON_UPDATE,
        // Slice 10.
        &points::VEHICLE_UPDATE,
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
        // Slice 4's callouts.
        t if let Some(&(_, pop)) = SLICE_4_CALLEES.iter().find(|c| c.0 == t) => pop,
        t if let Some(&(_, pop)) = SLICE_5_CALLEES.iter().find(|c| c.0 == t) => pop,
        t if let Some(&(_, pop)) = SLICE_6A_CALLEES.iter().find(|c| c.0 == t) => pop,
        t if let Some(&(_, pop)) = SLICE_9_CALLEES.iter().find(|c| c.0 == t) => pop,
        t if let Some(&(_, pop)) = SLICE_10_CALLEES.iter().find(|c| c.0 == t) => pop,
        t if let Some(&(_, pop)) = SLICE_6B_CALLEES.iter().find(|c| c.0 == t) => pop,
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
    if slice_5_answer(cpu, g, target) {
        return;
    }
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
        // Slice 9: the weapon's owner lookup answers none or a live
        // object (its `+13c` is read).
        0x101fb0 => if (n + case as usize) % 3 == 0 { u32::MAX } else { handle(3) },
        // Slice 10's callouts that answer in `al`: yes and no by turns.
        t if SLICE_10_ANSWERS_AL.contains(&t) => (n as u32 + case) % 2,
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
    slice_6a_answer(cpu, g, target, n, case);
    slice_6b_answer(cpu, g, target, n, case);
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
/// with the reason.
const FLOAT_OPT_OUT: &[(u32, &str)] = &[
    (0x000b_d090, "OBJECT_NODE_MATRICES: the body does not keep retail's XMM state (slice 4)"),
    (0x000b_c820, "OBJECT_POST_UPDATE: fuses OBJECT_NODE_MATRICES (slice 4)"),
    (0x000c_1400, "LIGHTS_UPDATE: the body does not keep retail's XMM state (slice 5)"),
    (0x000c_0c80, "LIGHT_RECORD_UPDATE: the body does not keep retail's XMM state (slice 5)"),
];

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
    dressed_with(point, cases, dress_step);
}

/// [`dressed`] with `extra` dressing the pool further.
fn dressed_with(point: &points::Point, cases: u32, extra: fn(&mut Guest, u32)) {
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
            extra(&mut g, case);
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

// Slice 4: the post-update shell (`bc5e0`, `bd090`, `bdef0`, `bef30`,
// `be1d0`) and the transform setters and getters (`b7430`, `b7740`,
// `b9dd0`, `b9fc0`, `ba1d0`). Their roots run over the pool dressed with
// a definition, node matrices on every body, a physics component, an
// attachment block and the vectors and outputs the setters and getters
// take; the walks and `bc820` reach the shell bodies through the table.

/// Every guest function a slice 4 body calls that no earlier table names,
/// with the bytes its `ret` pops (docs/engine/object-update.md, "Slice 4
/// bodies").
const SLICE_4_CALLEES: &[(u32, u32)] = &[
    // `bdef0`.
    (0x1d44f0, 12),
    (0xb87b0, 4),
    (0xb8600, 8),
    // `bef30`.
    (0xb9ce0, 0),
    (0xc3220, 0),
    (0xc2d00, 4),
    (0xc1720, 12),
    // `be1d0`.
    (0xb8c00, 4),
    (0x109140, 8),
    // `bc5e0` (`1d1c10` is in `SLICE_11_CALLEES`), `ba1d0`.
    (0x1d1ca0, 0),
    (0x1d4360, 4),
    (0xb91d0, 0),
    (0x1d09d0, 0),
    (0x1d0ad0, 0),
    (0x1d96d0, 0),
    // `b7430` (`b58c0` and `bd020` are in `SLICE_11_CALLEES`).
    (0x1c4a80, 8),
    // `bd090` (`1d3920` and `b8bd0` are named above).
    (0xe1670, 0),
    (0x16d940, 8),
    (0xbf5a0, 0),
    (0xbfc90, 0),
    (0xbff10, 0),
    (0xbfe80, 0),
    (0xbfed0, 0),
    (0xbdb60, 12),
    (0xbfe20, 0),
    (0xb6780, 0),
    (0x1c6440, 0),
    (0xa98b0, 0),
    (0xa9940, 0),
];

/// Where the slice 4 roots' synthetic tags, vectors and outputs live.
const SHELL_MODEL: u32 = 0x0066_0000;
const SHELL_RENDER: u32 = 0x0066_0400;
const SHELL_NODES: u32 = 0x0066_0800;
const SHELL_VECTORS: u32 = 0x0066_1000;
const SHELL_OUT: u32 = 0x0066_1100;
/// Body offsets of the attachment records and the node matrices.
const SHELL_ATTACHMENTS: u32 = 0x1a0;
const SHELL_MATRICES: u32 = 0x1c0;
const NONE: u32 = u32::MAX;

/// A node matrix at `at`: scale, an axis-aligned basis, a translation.
fn matrix(g: &mut Guest, at: u32, scale: f32, t: [f32; 3]) {
    let rows = [scale, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, t[0], t[1], t[2]];
    for (i, v) in rows.into_iter().enumerate() {
        f32w(g, at + 4 * i as u32, v);
    }
}

/// The fields the slice 4 bodies read, varied by `case`, on every body of
/// the pool (so parents, children and the object a root is called on all
/// have them) and on one definition, model and render model.
fn shell_dress(g: &mut Guest, case: u32) {
    let c = case as usize;
    word(g, points::TAG_ENTRIES.base, TAGS);
    word(g, TAGS + 8, DEFINITION);
    word(g, TAGS + 16 + 8, SHELL_MODEL);
    word(g, TAGS + 32 + 8, SHELL_RENDER);
    // Definition: its type byte (scenery, or biped in case 5), flags
    // (bit 12 picks the second sphere), radius, offset, second sphere,
    // and a model in some cases.
    byte(g, DEFINITION, if case == 5 { 0 } else { 6 });
    g.set_u16(DEFINITION + 2, if case % 2 == 1 { 0x1000 } else { 0 }).unwrap();
    f32w(g, DEFINITION + 4, 0.75);
    for (i, v) in [0.5f32, -0.25, 1.0].into_iter().enumerate() {
        let v = if case == 0 { 0.0 } else { v };
        f32w(g, DEFINITION + 8 + 4 * i as u32, v);
        f32w(g, DEFINITION + 0x24 + 4 * i as u32, v * 0.5);
    }
    f32w(g, DEFINITION + 0x20, [0.0, 1.5, -1.0][c % 3]);
    word(g, DEFINITION + 0x38, if case % 3 == 2 { 0x0001_0001 } else { NONE });
    // Model: the render model (tag 2) and a second reference that is
    // none in even cases; render model: one node.
    word(g, SHELL_MODEL + 4, 0x0002_0002);
    word(g, SHELL_MODEL + 0x14, if case % 2 == 0 { NONE } else { 0x0003_0003 });
    word(g, SHELL_RENDER + 0x48, 1);
    word(g, SHELL_RENDER + 0x4c, SHELL_NODES);
    // Vectors the setters take: a position inside the world, one far
    // outside it, a basis and velocities.
    for (i, v) in [1.0f32, 2.0, 3.0, 1.0e9, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.25, -0.5, 0.125]
        .into_iter()
        .enumerate()
    {
        f32w(g, SHELL_VECTORS + 4 * i as u32, v);
    }
    for slot in 0..SLOTS {
        let body = g.u32(DATA + 12 * slot + 8).unwrap();
        let k = (slot + case) as usize;
        // Flags: in the partition (bit 8), attachments (bit 4), lights
        // (bit 6), and bit 0 for a few.
        word(g, body + 4, [0x150, 0x100, 0x010, 0x140, 0x001, 0x110][k % 6]);
        f32w(g, body + 0xa0, [1.0, 2.0, 0.5][k % 3]);
        for (i, v) in [3.0f32, 4.0, 5.0].into_iter().enumerate() {
            let at = 4 * i as u32;
            f32w(g, body + 0x30 + at, v);
            f32w(g, body + 0x64 + at, v + 1.0);
            f32w(g, body + 0x88 + at, v * 0.5);
            f32w(g, body + 0x94 + at, -v);
        }
        for (off, v) in [(0x70, [1.0f32, 0.0, 0.0]), (0x7c, [0.0, 0.0, 1.0])] {
            for (i, x) in v.into_iter().enumerate() {
                f32w(g, body + off + 4 * i as u32, x);
            }
        }
        // The registered sphere: in case 0 the one the bounds compute
        // (definition offset zero, scale 1, the matrix translation), so
        // nothing moved; elsewhere a stale one.
        let t = [10.0 + slot as f32, 20.0, 30.0 - case as f32];
        let scale = if case == 0 || k % 3 == 0 { 1.0 } else { 2.0 };
        for (i, v) in t.into_iter().enumerate() {
            f32w(g, body + 0x40 + 4 * i as u32, if case == 0 { v } else { v + 7.0 });
        }
        f32w(g, body + 0x4c, if case == 0 { 0.75 * [1.0, 2.0, 0.5][k % 3] } else { 9.0 });
        g.set_u16(body + 0x114, 0x34).unwrap();
        g.set_u16(body + 0x116, SHELL_MATRICES as u16).unwrap();
        matrix(g, body + SHELL_MATRICES, scale, t);
        byte(g, body + 0x18, 0);
        // A physics component for odd slots: active (`+c0` bit 6, and
        // bit 11 in some), the rigid body index and count, its flags.
        let c0 = g.u8(body + 0xc0).unwrap();
        if slot % 2 == 1 {
            word(g, body + 0xb4, slot);
            byte(g, body + 0xc0, c0 | 0x40);
            byte(g, body + 0xc1, if k % 4 == 1 { 0x08 } else { 0x09 });
            let record = COMPONENTS + 0x100 + 0xa0 * slot;
            word(g, record + 4, [0, 0x1_0000, 0x100, 0x2, 0x8000, 0x1_0002][k % 6]);
            byte(g, record + 0x18, if k % 5 == 4 { 0xff } else { 0 });
            word(g, record + 0x74, if k % 7 == 6 { 0 } else { 1 });
        } else {
            byte(g, body + 0xc1, u8::from(k % 3 == 0));
        }
        // A replication entity for some.
        word(g, body + 0xd4, if k % 3 == 1 { NONE } else { 0x00e7_0000 | slot });
        // Two attachment records: a light, then an empty one.
        g.set_u16(body + 0x11c, 2 << 3).unwrap();
        g.set_u16(body + 0x11e, SHELL_ATTACHMENTS as u16).unwrap();
        byte(g, body + SHELL_ATTACHMENTS, if k % 4 == 3 { 1 } else { 0 });
        word(g, body + SHELL_ATTACHMENTS + 4, 0x00e8_0000 | slot);
        byte(g, body + SHELL_ATTACHMENTS + 8, 0);
        word(g, body + SHELL_ATTACHMENTS + 12, NONE);
    }
}

/// The arguments of a slice 4 root: the object, and the stack and
/// register arguments each takes, varied by `case`.
fn shell_arguments(g: &mut Guest, cpu: &mut Cpu, root: u32, case: u32) {
    let object = object(case);
    let vector = |i: u32| SHELL_VECTORS + 12 * i;
    let flag = |bit: u32| (case >> bit) & 1;
    word(g, STACK + 4, object);
    match root {
        r if r == points::OBJECT_ATTACHMENTS.base => {
            for (i, v) in [flag(0), flag(1), u32::from(case % 3 != 0), u32::from(case == 4)].into_iter().enumerate() {
                word(g, STACK + 8 + 4 * i as u32, v);
            }
        }
        r if r == points::OBJECT_SET_TRANSFORM.base => {
            // Position inside the world, outside it, or none; forward and
            // up, or neither; then the five flags.
            let position = [vector(0), vector(1), 0][case as usize % 3];
            let basis = if case % 2 == 0 { (vector(2), vector(3)) } else { (0, 0) };
            let args = [position, basis.0, basis.1, case, flag(0), flag(1), case + 7, flag(2)];
            for (i, v) in args.into_iter().enumerate() {
                word(g, STACK + 8 + 4 * i as u32, v);
            }
        }
        r if r == points::OBJECT_SET_VELOCITY.base => {
            cpu.gpr[0] = object;
            cpu.gpr[1] = if case % 3 == 1 { 0 } else { vector(4) };
            cpu.gpr[7] = if case % 2 == 1 { 0 } else { vector(3) };
            word(g, STACK + 4, flag(2));
        }
        r if r == points::OBJECT_NODES_NOTIFY.base => cpu.gpr[0] = object,
        r if r == points::OBJECT_WORLD_POSITION.base => {
            cpu.gpr[0] = object;
            cpu.gpr[6] = SHELL_OUT;
        }
        r if r == points::OBJECT_WORLD_BASIS.base => {
            cpu.gpr[0] = object;
            cpu.gpr[6] = if case % 3 == 2 { 0 } else { SHELL_OUT };
            cpu.gpr[7] = if case % 2 == 1 { 0 } else { SHELL_OUT + 0x10 };
        }
        r if r == points::OBJECT_ROOT_VELOCITY.base => {
            cpu.gpr[0] = object;
            cpu.gpr[1] = if case % 3 == 2 { 0 } else { SHELL_OUT };
            word(g, STACK + 4, if case % 2 == 1 { 0 } else { SHELL_OUT + 0x10 });
        }
        _ => {}
    }
}

/// [`transcripts`] over the pool [`shell_dress`]ed, with each root's
/// arguments; the float state is compared like every root's (see
/// [`FLOAT_OPT_OUT`]).
fn shell(point: &points::Point, cases: u32) {
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
            let (mut cpu, mut g) = start(ram, point.base, case);
            shell_dress(&mut g, case);
            shell_arguments(&mut g, &mut cpu, point.base, case);
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

// Slice 5: lights. A synthetic light datum array of eight elements whose
// owners and marker parents are pool objects, one light definition with
// the three modulation blocks, and a game-time record; the roots are the
// walk, the record update, its modulation step and the three list
// operations. The light lists, the datum iterator and the cluster
// queries stay callouts: the iterator is answered from the synthetic
// bitmap, so the walk visits the lights retail would. No light has a
// function source (`+54` is `ffff`), so the record update's `bab40` path
// and the link's owner-marker path (`b8c40`, `16640f`, `b8d30`) are not
// reached here; the marker-parent path is.

/// Every guest function a slice 5 body calls that no earlier table
/// lists, with the bytes its `ret` pops (docs/engine/object-update.md,
/// "Slice 5 bodies").
const SLICE_5_CALLEES: &[(u32, u32)] = &[
    (0x16bc00, 0),
    (0x1cae40, 8),
    (0xbad50, 0),
    (0x131d60, 8),
    (0x17ca10, 0),
    (0xb9ce0, 0),
    (0xb9d20, 0),
    (0x13b390, 8),
    (0x13bc00, 4),
    (0xc0840, 8),
    (0xb8c40, 0),
    (0x16640f, 16),
    (0xc28b0, 0),
    (0x16a8e0, 28),
    (0x11bed0, 0),
    (0x1cac60, 36),
    (0x16ba40, 0),
];

const LIGHT_ARRAY_GLOBAL: u32 = 0x004e_030c;
const LIGHT_ARRAY: u32 = 0x0066_0000;
const LIGHT_DATA: u32 = 0x0066_1000;
const LIGHT_BITS: u32 = 0x0066_2000;
const LIGHT_TAGS: u32 = 0x0066_3000;
const LIGHT_DEF: u32 = 0x0066_4000;
const LIGHT_BLOCKS: u32 = 0x0066_5000;
const LIGHT_TIME: u32 = 0x0066_6000;
const LIGHT_STRIDE: u32 = 0x110;
const LIGHT_COUNT: u32 = 8;

fn light_handle(i: u32) -> u32 {
    ((0x0100 + i) << 16) | i
}

fn light(i: u32) -> u32 {
    LIGHT_DATA + LIGHT_STRIDE * i
}

/// The datum iterator `16bc00(array: ecx, start: edx) -> eax` (the next
/// live index at or after `start`, or none), answered from the synthetic
/// array when a slice 5 body calls it. False for every other callout,
/// which `answer` handles.
fn slice_5_answer(cpu: &mut Cpu, g: &mut Guest, target: u32) -> bool {
    let sp = cpu.gpr[4];
    let ret = g.u32(sp).unwrap();
    match (target, ret) {
        (0x16bc00, 0xc1420 | 0xc32da) => {
            let array = cpu.gpr[1];
            let high = g.u32(array + 0x38).unwrap();
            let bits = g.u32(array + 0x48).unwrap();
            let start = cpu.gpr[2];
            cpu.gpr[0] = (start..high)
                .find(|&i| g.u32(bits + 4 * (i >> 5)).unwrap() & (1 << (i & 31)) != 0)
                .unwrap_or(u32::MAX);
            cpu.gpr[1] = 0x1600_0000 | start;
            cpu.gpr[2] = high;
            cpu.gpr[4] = sp + 4;
            cpu.eip = ret;
            true
        }
        _ => false,
    }
}

/// The light array, its definition and the game time, varied by `case`.
fn lights(g: &mut Guest, case: u32) {
    word(g, LIGHT_ARRAY_GLOBAL, LIGHT_ARRAY);
    word(g, LIGHT_ARRAY + 0x24, LIGHT_STRIDE);
    word(g, LIGHT_ARRAY + 0x38, [0, 3, 5, 8, 8, 6][case as usize % 6]);
    word(g, LIGHT_ARRAY + 0x44, LIGHT_DATA);
    word(g, LIGHT_ARRAY + 0x48, LIGHT_BITS);
    word(g, LIGHT_BITS, [0, 0b101, 0b1_1110, 0xff, 0b1001_0110, 0b10_0001][case as usize % 6]);
    // Objects are ready (the byte `c0c80` tests before a link) in odd
    // cases.
    byte(g, PHASE, (case % 2) as u8);
    word(g, points::GAME_TIME.base, LIGHT_TIME);
    g.set_u16(LIGHT_TIME + 2, 30).unwrap();
    f32w(g, LIGHT_TIME + 4, 1.0 / 30.0);
    word(g, LIGHT_TIME + 8, 1000 + case);
    // Tag 0: the pool objects' (empty) definition; tag 1: the light's.
    word(g, points::TAG_ENTRIES.base, LIGHT_TAGS);
    word(g, LIGHT_TAGS + 8, LIGHT_DEF + 0x800);
    word(g, LIGHT_TAGS + 0x18, LIGHT_DEF);
    // Definition flags: bit 3, the cluster query's bit 15.
    word(g, LIGHT_DEF, [0x8000, 0x8008, 0, 0x8008, 0x8000, 8][case as usize % 6]);
    f32w(g, LIGHT_DEF + 8, 0.25);
    f32w(g, LIGHT_DEF + 0xc, 2.0);
    for (i, off) in (0x40..0x78).step_by(4).enumerate() {
        f32w(g, LIGHT_DEF + off, [0.5, 1.5, -0.25, 0.0, 0.75][(i + case as usize) % 5]);
    }
    f32w(g, LIGHT_DEF + 0xb0, 1.0);
    g.set_u16(LIGHT_DEF + 0xb6, 3).unwrap();
    // The modulation blocks at `+c0`: flags, then three (count,
    // function) pairs.
    let counts = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 1), (1, 0, 1)][case as usize % 6];
    word(g, LIGHT_DEF + 0xc0, case % 2);
    word(g, LIGHT_DEF + 0xc4, counts.0);
    word(g, LIGHT_DEF + 0xc8, LIGHT_BLOCKS);
    word(g, LIGHT_DEF + 0xcc, counts.1);
    word(g, LIGHT_DEF + 0xd0, LIGHT_BLOCKS + 0x40);
    word(g, LIGHT_DEF + 0xd4, counts.2);
    word(g, LIGHT_DEF + 0xd8, LIGHT_BLOCKS + 0x80);
    word(g, LIGHT_BLOCKS + 4, LIGHT_BLOCKS + 0x100);
    byte(g, LIGHT_BLOCKS + 0x101, if case % 3 == 1 { 0x10 } else { 0 });
    f32w(g, LIGHT_BLOCKS + 0x104, 0.25);
    f32w(g, LIGHT_BLOCKS + 0x108, 0.75);
    for i in 0..LIGHT_COUNT {
        let l = light(i);
        g.set_u16(l, (light_handle(i) >> 16) as u16).unwrap();
        // Flags: active, linked, timed, and the two path bits.
        g.set_u16(l + 2, [0x0a, 0x02, 0x08, 0x1a, 0x00, 0x0b, 0x12, 0x0e][((i + case) % 8) as usize]).unwrap();
        word(g, l + 4, 0xe001_0001);
        word(g, l + 0x14, 1000 - 50 * i);
        for (k, v) in [1.0f32, -2.0, 0.5].into_iter().enumerate() {
            f32w(g, l + 0x18 + 4 * k as u32, v * (i + 1) as f32);
        }
        f32w(g, l + 0x24, [2.0, 0.0, -1.0, 4.0][((i + case) % 4) as usize]);
        g.set_u16(l + 0x48, if (i + case) % 5 == 2 { 3 } else { u16::MAX }).unwrap();
        // Owner (a biped or vehicle in some cases, so the damage
        // attenuation runs), marker parent, both or neither.
        let owner = match (i + case) % 4 {
            0 => handle(1),
            1 => handle(13 - case % 13),
            _ => u32::MAX,
        };
        word(g, l + 0x4c, owner);
        g.set_u16(l + 0x54, u16::MAX).unwrap();
        word(g, l + 0x58, if (i + case) % 3 == 1 { handle(3) } else { u32::MAX });
        g.set_u16(l + 0x5c, 0).unwrap();
        for k in 0..9 {
            f32w(g, l + 0x60 + 4 * k, 0.1 * (k + i) as f32);
        }
        f32w(g, l + 0xcc, [1.0, 0.5, 0.0][((i + case) % 3) as usize]);
        f32w(g, l + 0xd0, 0.5);
    }
    // The owners' damage amount the attenuation reads (body `+2b0`).
    for slot in 0..SLOTS {
        f32w(g, RECORDS + BODY * slot + 0x2b0, [0.0, 0.4, 1.5][(slot % 3) as usize]);
    }
}

/// The arguments of a slice 5 root, on the planted stack and in registers.
fn light_entry(g: &mut Guest, cpu: &mut Cpu, root: u32, case: u32) {
    let i = case % LIGHT_COUNT;
    let h = light_handle(i);
    let l = light(i);
    match root {
        r if r == points::LIGHT_UNLINK.base => cpu.gpr[0] = h,
        r if r == points::LIGHT_RECORD_PHASE.base => {
            cpu.gpr[0] = l + 0xd4;
            cpu.gpr[1] = LIGHT_DEF + 0xc0;
            word(g, STACK + 4, h);
            f32w(g, STACK + 8, 0.5);
            word(g, STACK + 12, l + 0xec);
            word(g, STACK + 16, l + 0xf0);
        }
        _ => {
            word(g, STACK + 4, h);
            word(g, STACK + 8, case % 2);
        }
    }
}

/// [`transcripts`] over the light array.
fn lit(point: &points::Point, cases: u32) {
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
            let (mut cpu, mut g) = start(ram, point.base, case);
            lights(&mut g, case);
            light_entry(&mut g, &mut cpu, point.base, case);
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

#[test]
fn object_bounds() {
    shell(&points::OBJECT_BOUNDS, 6);
}

#[test]
fn object_attachments() {
    shell(&points::OBJECT_ATTACHMENTS, 6);
}

#[test]
fn object_nodes_notify() {
    shell(&points::OBJECT_NODES_NOTIFY, 6);
}

#[test]
fn object_component_transform() {
    shell(&points::OBJECT_COMPONENT_TRANSFORM, 6);
}

#[test]
fn object_node_matrices() {
    shell(&points::OBJECT_NODE_MATRICES, 6);
}

#[test]
fn object_transform_setters() {
    for point in [&points::OBJECT_SET_TRANSFORM, &points::OBJECT_SET_VELOCITY] {
        shell(point, 6);
    }
}

#[test]
fn object_transform_getters() {
    for point in [&points::OBJECT_WORLD_POSITION, &points::OBJECT_WORLD_BASIS, &points::OBJECT_ROOT_VELOCITY] {
        shell(point, 6);
    }
}

/// `bc820` over the dressed pool: the shell bodies fused under it.
#[test]
fn object_post_update_over_the_shell() {
    shell(&points::OBJECT_POST_UPDATE, 6);
}

#[test]
fn lights_update() {
    lit(&points::LIGHTS_UPDATE, 6);
}

#[test]
fn light_record_update() {
    lit(&points::LIGHT_RECORD_UPDATE, 8);
}

#[test]
fn light_record_phase() {
    lit(&points::LIGHT_RECORD_PHASE, 6);
}

#[test]
fn light_link() {
    lit(&points::LIGHT_LINK, 8);
}

#[test]
fn light_unlink() {
    lit(&points::LIGHT_UNLINK, 8);
}

#[test]
fn light_expire() {
    lit(&points::LIGHT_EXPIRE, 8);
}

// Slice 6a: the unit update's shell `c6b30`, the movement `c60c0`, the
// direction clamp `cba50`, the idle test `c85c0` and the aim update
// `c7840`. The dispatcher and walk roots above reach them through the
// unit types' `+40` slot; the roots below run them over the pool dressed
// with a unit's rates, directions, limits and held weapons.

/// Every guest function a slice 6a body calls and is not stubbed above,
/// with the bytes its `ret` pops (docs/engine/object-update.md, "Slice 6a
/// bodies"). `b9fc0` is slice 11's, `e6fe0` slice 3's.
const SLICE_6A_CALLEES: &[(u32, u32)] = &[
    // The shell `c6b30`.
    (0xc49b0, 4),
    (0xc58f0, 4),
    (0xc5eb0, 0),
    (0xc6740, 0),
    (0x114c60, 0),
    (0xc6990, 0),
    (0xc6810, 0),
    (0xc50a0, 0),
    // The movement `c60c0`.
    (0xc4970, 0),
    (0x11f0d0, 12),
    (0x11ce20, 0),
    (0x1201a0, 0),
    (FTOL2, 0),
    // The clamp `cba50`.
    (0x30bf0, 0),
    (0x11d000, 0),
    (0x1420f0, 0),
    // The aim update `c7840`.
    (0xc7300, 20),
    (0xc75d0, 4),
    (0xc7750, 4),
    (0x322bd8, 0),
    // The idle test `c85c0`.
    (0x100880, 0),
];

/// `_ftol2`: `st(0)` popped into `edx:eax`.
const FTOL2: u32 = 0x322b0c;
/// The direction the clamp root is given.
const UNIT_DIRECTION: u32 = 0x0068_0000;
/// `[body+33e]`: the unit's limits record, in its body.
const UNIT_LIMITS: u16 = 0x1c0;
/// `[body+346]`: the unit's record word, at `+4` from here.
const UNIT_RECORD: u16 = 0x1f0;

std::thread_local! {
    /// Whether a slice 6a root is running: its stubs answer as a unit's.
    static UNIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The slice 6a stubs' effects beyond [`answer`]'s: `_ftol2` pops the
/// x87 stack (always; the dispatcher roots reach it too). Under a unit
/// root, the state tests answer no but one, the weapon test by turns, the
/// frame and the matrix are written.
fn slice_6a_answer(cpu: &mut Cpu, g: &mut Guest, target: u32, n: usize, case: u32) {
    if target == FTOL2 {
        let x = &mut cpu.x87;
        let top = (x.status >> 11) & 7;
        x.tag |= 3 << (2 * top);
        x.status = (x.status & !0x3800) | (((top + 1) & 7) << 11);
    }
    if !UNIT.with(std::cell::Cell::get) {
        return;
    }
    match target {
        0xe6fe0 => cpu.gpr[0] = u32::from(case == 2 && n == 3),
        0x100880 => cpu.gpr[0] = (n as u32 + case) % 2,
        // `b9fc0 (unit: eax, out: esi, edi)`.
        0xb9fc0 => {
            vec3(g, cpu.gpr[6], [1.0, 0.0, 0.0]);
            vec3(g, cpu.gpr[7], [0.0, 0.0, 1.0]);
        }
        // `1420f0`: a matrix at `edx`, turned about z by the case.
        0x1420f0 => {
            let m = cpu.gpr[2];
            let (s, c) = (0.4 * case as f32).sin_cos();
            f32w(g, m, 1.0);
            vec3(g, m + 4, [c, s, 0.0]);
            vec3(g, m + 0x10, [-s, c, 0.0]);
            vec3(g, m + 0x1c, [0.0, 0.0, 1.0]);
        }
        _ => {}
    }
}

/// Three floats at `at`.
fn vec3(g: &mut Guest, at: u32, v: [f32; 3]) {
    for (i, x) in v.into_iter().enumerate() {
        f32w(g, at + 4 * i as u32, x);
    }
}

/// [`dress`], then the fields the slice 6a bodies branch on, varied by
/// `case`: the definition's rates and early-out bit, the unit's
/// directions, its limits record, its record word and held weapons.
fn unit_dress(g: &mut Guest, case: u32) {
    dress(g, case);
    let c = case as usize % 6;
    // Aim rates `+144`, `+148`, the scale `+14c`, look rates `+150`, `+154`.
    let rates = [
        [0.0, 0.0, 1.0, 0.0, 0.0],
        [0.5, 0.25, 2.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.75, 0.5],
        [1.0, 1.0, 0.5, 1.0, 1.0],
        [0.0, 0.0, 1.0, 0.0, 0.0],
        [0.25, 0.0, 1.0, 0.0, 0.125],
    ][c];
    for (i, r) in rates.into_iter().enumerate() {
        f32w(g, DEFINITION + 0x144 + 4 * i as u32, r);
    }
    if case == 4 {
        let bits = g.u32(DEFINITION + 0xbc).unwrap();
        word(g, DEFINITION + 0xbc, bits | 1 << 10);
    }
    let body = g.u32(DATA + 12 * (object(case) & 0xffff) + 8).unwrap();
    byte(g, body + 0x10a, if case == 5 { 4 } else { 0 });
    let (s, k) = (0.3 * case as f32 + 0.2).sin_cos();
    let v = |at: u32| body + at;
    // Desired aim and aim, aim velocity, desired look and look, look
    // velocity: unequal but in case 0.
    vec3(g, v(0x15c), [k, s, 0.1]);
    vec3(g, v(0x168), if case == 0 { [k, s, 0.1] } else { [1.0, 0.0, 0.0] });
    vec3(g, v(0x174), [9.0, 9.0, 9.0]);
    vec3(g, v(0x180), [k, 0.0, s]);
    vec3(g, v(0x18c), if case == 0 { [k, 0.0, s] } else { [0.0, 1.0, 0.0] });
    vec3(g, v(0x198), [9.0, 9.0, 9.0]);
    byte(g, body + 0x1bc, u8::from(case % 2 == 1));
    // The limits: bit 5 the aim's, bit 6 the look's; each a yaw and a
    // pitch range.
    g.set_u16(body + 0x33e, UNIT_LIMITS).unwrap();
    let limits = body + u32::from(UNIT_LIMITS);
    byte(g, limits, [0, 0x20, 0x40, 0x60, 0x20, 0x60][c]);
    for (i, w) in [-0.5f32, 0.5, -0.4, 0.4, -1.0, 1.0, -0.3, 0.3].into_iter().enumerate() {
        f32w(g, limits + 4 + 4 * i as u32, w);
    }
    // The idle test: the record word's bits 26 and 27, the two held
    // weapons' indices and handles.
    g.set_u16(body + 0x346, UNIT_RECORD).unwrap();
    word(g, body + u32::from(UNIT_RECORD) + 4, [0, 1 << 26, 0, 1 << 27, 0, 0][c]);
    let held = [[0xff, 0xff], [0, 0xff], [0xff, 1], [0, 1], [1, 0], [0, 1]][c];
    byte(g, body + 0x212, held[0]);
    byte(g, body + 0x213, held[1]);
    word(g, body + 0x218, 0xe200_0001);
    word(g, body + 0x21c, if case == 4 { u32::MAX } else { 0xe200_0002 });
    // The clamp root's direction.
    let a = 0.9 * case as f32 + 0.1;
    for (i, w) in [a.cos(), a.sin(), 0.2 * case as f32 - 0.5].into_iter().enumerate() {
        f32w(g, UNIT_DIRECTION + 4 * i as u32, w);
    }
}

/// The arguments of a slice 6a root: the unit on the stack (`c6b30`,
/// `c60c0`) or in `eax` (`cba50`, `c85c0`); the clamp's direction and
/// look flag on the stack.
fn unit_entry(g: &mut Guest, cpu: &mut Cpu, root: u32, case: u32) {
    let unit = object(case);
    if root == points::UNIT_UPDATE.base || root == points::UNIT_MOVEMENT.base {
        word(g, STACK + 4, unit);
    } else {
        cpu.gpr[0] = unit;
    }
    if root == points::UNIT_CLAMP_DIRECTION.base {
        word(g, STACK + 4, UNIT_DIRECTION);
        word(g, STACK + 8, case % 2);
    }
}

/// [`transcripts`] of a slice 6a root over a [`unit_dress`]ed pool; the
/// float state is compared like every root's.
fn unit_transcripts(point: &points::Point, cases: u32) {
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
    UNIT.with(|u| u.set(true));
    let mut h = Harness { code: &mut code, table, root: point.base, case: 0 };
    for case in 0..cases {
        h.case = case;
        let fresh = || {
            let (mut cpu, mut g) = start(ram, point.base, case);
            unit_dress(&mut g, case);
            unit_entry(&mut g, &mut cpu, point.base, case);
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
    UNIT.with(|u| u.set(false));
}

#[test]
fn unit_update() {
    unit_transcripts(&points::UNIT_UPDATE, 6);
}

#[test]
fn unit_movement() {
    unit_transcripts(&points::UNIT_MOVEMENT, 6);
}

#[test]
fn unit_clamp_direction() {
    unit_transcripts(&points::UNIT_CLAMP_DIRECTION, 6);
}

#[test]
fn unit_idle_test() {
    unit_transcripts(&points::UNIT_IDLE_TEST, 6);
}


// Slice 9: the weapon update `fdfb0`. Its root runs over the slice 11
// dressing plus a weapon definition with up to two triggers and two
// barrels, their body states in every state the jump tables reach, and an
// animation block for some cases; it runs through [`dressed_with`] like
// every root, comparing the float state at every callout.

/// Every guest function `fdfb0` calls, with the bytes its `ret` pops
/// (docs/engine/object-update.md, "Slice 9 body"). `1cb710`, `1daea0`
/// and `badc0` are slice 11's.
const SLICE_9_CALLEES: &[(u32, u32)] = &[
    (0x101690, 4),
    (0xd1490, 0),
    (0x101fb0, 0),
    (0x105e80, 4),
    (0x105740, 0),
    (0x105800, 0),
    (0x10d4e0, 0),
    (0x105840, 0),
    (0x1039a0, 12),
    (0x101f20, 0),
    (0xc86e0, 4),
    (0x105fa0, 4),
    (0x1685a6, 8),
    (0x103a60, 0),
    (0x177610, 0),
    (0x177260, 4),
    (0xfdd50, 8),
    (0x102540, 12),
    (0x102f70, 0),
    (0x106280, 8),
    (0x103c40, 4),
    (0x103a90, 4),
    (0x103ce0, 4),
    (0x1027b0, 12),
    (0x103b10, 4),
    (0x103bd0, 4),
    (0x102d60, 0),
    (0x102e10, 0),
    (0x102f10, 0),
    (0x1061c0, 8),
    (0x103b70, 4),
    (0x1023d0, 4),
    (0x103e60, 4),
    (0x102100, 12),
    (0x102fb0, 8),
    (0x103f60, 8),
    (0x103dd0, 4),
    (0xba7f0, 16),
    // `_ftol2` (`FTOL2`) is in `SLICE_6A_CALLEES`.
];

/// The weapon definition's trigger records (`40` bytes each) and barrel
/// records (`ec` bytes each).
const WEAPON_TRIGGERS: u32 = 0x0066_0000;
const WEAPON_BARRELS: u32 = 0x0066_1000;

/// The weapon fields `fdfb0` branches on, varied by `case`, over
/// [`dress`].
fn dress_weapon(g: &mut Guest, case: u32) {
    let k = case as usize;
    let f = |v: f32| v.to_bits();
    // The definition.
    word(g, DEFINITION + 0x12c, [0, 1 << 27, 1 << 10, 1 << 27 | 1 << 10][k % 4]);
    word(g, DEFINITION + 0x150, f(0.25));
    word(g, DEFINITION + 0x154, f(0.5));
    word(g, DEFINITION + 0x160, f(0.5));
    word(g, DEFINITION + 0x168, f([0.0, 2.0][k % 2]));
    word(g, DEFINITION + 0x170, if k % 3 == 0 { u32::MAX } else { 0x00e6_0001 });
    word(g, DEFINITION + 0x248, if k % 2 == 0 { 0x00e6_0002 } else { u32::MAX });
    word(g, DEFINITION + 0x250, if k % 2 == 1 { 0x00e6_0003 } else { u32::MAX });
    word(g, DEFINITION + 0x254, f([0.0, 0.5][k % 2]));
    g.set_u16(DEFINITION + 0x292, [3, 0][k % 2]).unwrap();
    word(g, DEFINITION + 0x2c0, [0, 1, 2][k % 3]);
    word(g, DEFINITION + 0x2c8, if case == 0 { 0 } else { 2 });
    word(g, DEFINITION + 0x2cc, WEAPON_TRIGGERS);
    word(g, DEFINITION + 0x2d0, if case == 0 { 1 } else { 2 });
    word(g, DEFINITION + 0x2d4, WEAPON_BARRELS);
    word(g, DEFINITION + 0x2d8, f(1.0));
    word(g, DEFINITION + 0x2dc, f(0.5));
    for t in 0..2usize {
        let r = WEAPON_TRIGGERS + 0x40 * t as u32;
        g.set_u16(r + 4, ((k + t) % 4) as u16).unwrap();
        g.set_u16(r + 6, ((k + 2 * t) % 6) as u16).unwrap();
        g.set_u16(r + 8, if (k + t) % 5 == 4 { u16::MAX } else { ((k + t) % 2) as u16 }).unwrap();
        g.set_u16(r + 0xa, if (k + t) % 7 == 3 { u16::MAX } else { (1 - (k + t) % 2) as u16 }).unwrap();
        g.set_u16(r + 0x18, ((k + t) % 4) as u16).unwrap();
        g.set_u16(r + 0x1a, ((k + 2 * t + 1) % 4) as u16).unwrap();
    }
    for b in 0..2usize {
        let r = WEAPON_BARRELS + 0xec * b as u32;
        word(g, r, [0, 1 << 9, 1 << 10, 1 << 9 | 1 << 10][(k + b) % 4]);
        word(g, r + 0x14, f([0.0, 1.5][(k + b) % 2]));
        word(g, r + 0x18, f([0.0, 0.5, 0.9][(k + b) % 3]));
        word(g, r + 0x50, f(0.5));
        word(g, r + 0x54, f(0.75));
        for (i, off) in [0xc4u32, 0xc8, 0xcc, 0xd0, 0xd4, 0xd8, 0xdc, 0xe0].into_iter().enumerate() {
            word(g, r + off, f(0.25 + 0.5 * i as f32));
        }
    }

    // The body.
    let body = g.u32(DATA + 12 * (object(case) & 0xffff) + 8).unwrap();
    word(g, body + 0x154, if k % 2 == 0 { handle(3) } else { u32::MAX });
    byte(g, body + 0x16c, [0, 2, 4, 0x10, 0x20, 0x22, 6, 0x12][k % 8]);
    g.set_u16(body + 0x16e, [0, 1, 0x80, 0x40, 0x20, 0x108, 0x41, 0x1e][k % 8]).unwrap();
    byte(g, body + 0x170, [0, 1][k % 2]);
    byte(g, body + 0x171, [0, 0, 1][k % 3]);
    byte(g, body + 0x175, 10);
    g.set_u16(body + 0x17c, [0, 3][k % 2]).unwrap();
    word(g, body + 0x180, f([0.0, 0.75, 0.3][k % 3]));
    word(g, body + 0x184, f(0.5));
    word(g, body + 0x188, f([0.0, 0.0, 1.0][k % 3]));
    word(g, body + 0x18c, f(0.2));
    word(g, body + 0x190, f([0.0, 1.0][k % 2]));
    word(g, body + 0x194, if k % 2 == 1 { handle(3) } else { u32::MAX });
    g.set_u16(body + 0x19e, [1, 5][k % 2]).unwrap();
    word(g, body + 0x244, if k % 3 == 1 { 0x00e8_0001 } else { u32::MAX });
    // An animation block at `+100` in odd cases: tag index 0, a graph
    // word at `+6` (none in case 5), the flag bytes `+11`, `+12`, `+14`.
    g.set_u16(body + 0x12a, if k % 2 == 1 { 0x100 } else { u16::MAX }).unwrap();
    word(g, body + 0x100, 0);
    word(g, body + 0x104, if case == 5 { 0xffff_0005 } else { 0x0001_0005 });
    byte(g, body + 0x111, [0, 2, 9][k % 3]);
    byte(g, body + 0x112, [1, 0][k % 2]);
    byte(g, body + 0x114, [0, 1, 0][k % 3]);
    // Barrel states at `+1a4`, `34` bytes each.
    for b in 0..2usize {
        let r = body + 0x1a4 + 0x34 * b as u32;
        byte(g, r, [0, 0x7f, 5][(k + b) % 3]);
        byte(g, r + 1, ((k + b) % 5) as u8);
        g.set_u16(r + 2, [0, 1, 3][(k + 2 * b) % 3]).unwrap();
        byte(g, r + 4, [0, 1, 8, 9][(k + b) % 4]);
        word(g, r + 0x10, f([0.0, 0.5, 0.95][(k + b) % 3]));
        word(g, r + 0x14, f(0.25));
        word(g, r + 0x18, f(0.0));
        word(g, r + 0x1c, f([0.0, 0.5][(k + b) % 2]));
        word(g, r + 0x20, f([0.0, 2.0][(k + b) % 2]));
    }
    // Trigger states at `+20c`, `c` bytes each: every state over the
    // cases, the countdown, the flag bits and the effect.
    for t in 0..2usize {
        let r = body + 0x20c + 0xc * t as u32;
        byte(g, r, ((k + 3 * t) % 8) as u8);
        g.set_u16(r + 2, [0, 2][(k + t) % 2]).unwrap();
        word(g, r + 4, [0, 1, 0x40, 0x41, 0x80][(k + t) % 5]);
        word(g, r + 8, if (k + t) % 2 == 1 { 0x00e7_0001 } else { u32::MAX });
    }
}

#[test]
fn weapon_update() {
    dressed_with(&points::WEAPON_UPDATE, 16, dress_weapon);
}


// Slice 10: the vehicle update `efde0` and its leaves `f1320` and
// `f06c0` (reached from the root, never entered alone). The root runs
// over the slice 11 dressing plus the vehicle's fields ([`dressed_with`]),
// and compares the float state at every callout like every root: the body's float work is scalar SSE on
// the guest registers, and callees read `xmm0` to `xmm7` as it leaves them.

/// Every guest function the slice 10 body calls that no table above
/// lists, with the bytes its `ret` pops (docs/engine/object-update.md,
/// "Slice 10 body"). `1d24a0`, `ba1d0` and `d6660` are slice 11's,
/// `ba7f0` slice 8's; `bba20` is a body.
const SLICE_10_CALLEES: &[(u32, u32)] = &[
    (0x1d2460, 0),
    (0xefc30, 0),
    (0xb9b90, 4),
    (0xef4f0, 8),
    (0xef870, 0),
    (0xefb60, 0),
    (0x205510, 0),
    (0xba160, 4),
    (0x1cfb90, 0),
    (0xf6670, 8),
    (0xba3d0, 0),
    (0x205be0, 8),
    (0xf1cd0, 4),
    (0xf1f80, 8),
    (0xf2360, 8),
    (0xf3010, 12),
    (0xf4800, 8),
    (0x2056e0, 20),
    (0xf6010, 0),
    (0xf6bd0, 4),
    (0xcc380, 0),
    (0x372d48, 0),
    (0xb77d0, 4),
    (0xd7b80, 24),
    (0x113e40, 0),
    (0xe6830, 4),
    (0x111650, 8),
    (0xb9ef0, 0),
];

/// Slice 10's callouts whose `al` the body branches on.
const SLICE_10_ANSWERS_AL: &[u32] = &[0xef870, 0xefb60, 0x205510, 0x205be0, 0xcc380, 0x113e40, 0xe6830, 0x111650];

/// The matg the vehicle update reads (`[4e034c]`, its record at `+144`),
/// and the definition's marker array (`+2f4`, `4c` bytes each).
const VEHICLE_MATG: u32 = 0x0064_7000;
const VEHICLE_MATG_RECORD: u32 = 0x0064_7100;
const VEHICLE_MARKERS: u32 = 0x0064_1800;

/// The fields `efde0` branches on, varied by `case`, over [`dress`].
fn dress_vehicle(g: &mut Guest, case: u32) {
    let c = case as usize;
    // Definition: the vehicle type (0 to 5 the jump table's cases, 6
    // skips the physics, 7 none of them), the flags `+1ec` and `+2ac`,
    // the contact count `+2e8` (negative once), and the markers.
    word(g, DEFINITION + 0x1ec, if case % 3 != 0 { 0x40 } else { 0 });
    g.set_u16(DEFINITION + 0x1f0, (case % 8) as u16).unwrap();
    f32w(g, DEFINITION + 0x214, 0.5);
    g.set_u16(DEFINITION + 0x218, (case % 2) as u16).unwrap();
    f32w(g, DEFINITION + 0x25c, [0.0, 2.0, 0.5][c % 3]);
    byte(g, DEFINITION + 0x2ac, u8::from(case == 9));
    word(g, DEFINITION + 0x2e8, [0, 1, 2, 3, u32::MAX][c % 5]);
    word(g, DEFINITION + 0x2f0, [0, 2, 3][c % 3]);
    word(g, DEFINITION + 0x2f4, VEHICLE_MARKERS);
    for i in 0..3u32 {
        word(g, VEHICLE_MARKERS + 0x4c * i + 4, if i == 1 { 0 } else { 0x10 });
        word(g, VEHICLE_MARKERS + 0x4c * i + 0x48, [3, u32::MAX, 33][i as usize]);
    }

    let slot = object(case) & 0xffff;
    let body = g.u32(DATA + 12 * slot + 8).unwrap();
    // A vehicle, in the header and in the body (the dispatchers read the
    // body's).
    byte(g, DATA + 12 * slot + 3, 1);
    byte(g, body + 0xaa, 1);
    // Parented once; children (slots 9 and 10, both bipeds, the second
    // flagged at `+10a` bit 7) for two cases in three.
    word(g, body + 0x14, if case == 7 { handle(2) } else { u32::MAX });
    word(g, body + 0x10, if case % 3 != 0 { handle(9) } else { u32::MAX });
    word(g, RECORDS + BODY * 9 + 0x0c, handle(10));
    word(g, RECORDS + BODY * 10 + 0x0c, u32::MAX);
    byte(g, RECORDS + BODY * 9 + 0xaa, 0);
    byte(g, RECORDS + BODY * 10 + 0xaa, 0);
    let flagged = g.u8(RECORDS + BODY * 10 + 0x10a).unwrap() | 0x80;
    byte(g, RECORDS + BODY * 10 + 0x10a, flagged);
    // Body `+c0`: bit 6 (component active), 8 (woken by a mover), 10 and
    // 12; the wake latch bits from the pool kept.
    let latch = g.u16(body + 0xc0).unwrap() & 0x0c;
    let bits = [0x0040, 0x1140, 0x0440, 0x1540, 0x0000, 0x1040, 0x0140, 0x0040, 0x0040, 0x1140, 0x0540][c % 11];
    g.set_u16(body + 0xc0, latch | bits).unwrap();
    let damage = g.u16(body + 0x10a).unwrap() & !4;
    g.set_u16(body + 0x10a, damage | if case == 1 || case == 10 { 4 } else { 0 }).unwrap();
    word(g, body + 0x148, case % 2 | if case % 4 < 2 { 0x2000 } else { 0 });
    word(g, body + 0x134, 0x1234_0000);
    f32w(g, body + 0x25c, if case % 4 == 3 { 0.0 } else { 1.5 });
    f32w(g, body + 0x2c4, [0.0, 0.5, 1.0, 2.0][c % 4]);
    byte(g, body + 0x348, [0, 0x20, 0x04, 0x21, 0x22, 0x01, 0x23, 0][c % 8]);
    byte(g, body + 0x349, (case % 2) as u8);
    f32w(g, body + 0x354, [-1.0, 0.25, 3.0][c % 3]);
    for (i, v) in [1.0f32, 2.0, [-2.0, 0.0, 4.0][c % 3]].into_iter().enumerate() {
        f32w(g, body + 0x64 + 4 * i as u32, v);
    }
    // The animation block's name: the one `f0619` compares, read from the
    // instruction, once.
    if case == 4 {
        let name = g.u32(0xf061c).unwrap();
        word(g, body + 0x180 + 0x7c, name);
    }
    // The structure's heights, radius and point; the matg record.
    f32w(g, STRUCTURE + 0x1c, [0.0, 5.0][c % 2]);
    f32w(g, STRUCTURE + 0x20, [0.0, -3.0, 1.0][c % 3]);
    f32w(g, STRUCTURE + 0x1c0, [0.0, 4.0, 0.5][c % 3]);
    for (i, v) in [3.0f32, -1.0, 2.0].into_iter().enumerate() {
        f32w(g, STRUCTURE + 0x1c4 + 4 * i as u32, v);
    }
    word(g, points::GLOBALS_TAG.base, VEHICLE_MATG);
    word(g, VEHICLE_MATG + 0x144, VEHICLE_MATG_RECORD);
    f32w(g, VEHICLE_MATG_RECORD + 0x5c, [-10.0, 10.0][c % 2]);
    word(g, VEHICLE_MATG_RECORD + 0x28, 0x00e6_0004);
}

#[test]
fn vehicle_update() {
    dressed_with(&points::VEHICLE_UPDATE, 11, dress_vehicle);
}

/// The dispatcher reaches the vehicle body by the `vehi` slot over the
/// vehicle's dressing.
#[test]
fn type_update_dispatch_reaches_the_vehicle() {
    dressed_with(&points::OBJECT_TYPE_UPDATE, 11, dress_vehicle);
}

// Slice 6b: the unit update's control `c49b0`, state `c58f0`, powered
// seats `c5eb0` and the control refresh `ce0c0` the seats call. The unit
// update root above reaches the first three through the shell; the roots
// below run each over the slice 6a dressing plus the fields they branch
// on: the definition's early-out bits, powered seats and `+1d4..+1e0`
// rates, the unit's flag words, timers, linked objects, held weapons and
// control history.

/// Every guest function a slice 6b body calls, with the bytes its `ret`
/// pops (docs/engine/object-update.md, "Slice 6b bodies"). `cc380` is
/// slice 10's, `138880` the damage update's.
const SLICE_6B_CALLEES: &[(u32, u32)] = &[
    // The control `c49b0`.
    (0xd0d20, 0),
    (0xd0080, 0),
    (0x10f5f0, 0),
    (0xc7070, 0),
    (0x10f720, 4),
    (0x1143d0, 0),
    (0xf42f0, 0),
    // The state `c58f0`.
    (0xe68c0, 4),
    (0x113da0, 0),
    (0x113d20, 4),
    (0x113df0, 0),
    (0x113d60, 4),
    (0x110ab0, 4),
    (0xcbec0, 0),
    (0x189210, 0),
    (0x100430, 12),
    (0xd1540, 4),
    // The seats `c5eb0` and the refresh `ce0c0`.
    (0xd0c10, 0),
    (0xe4050, 0),
    (0xd1080, 8),
];

/// Slice 6b's callouts whose `al` the bodies branch on.
const SLICE_6B_ANSWERS_AL: &[u32] = &[0xc7070, 0x1143d0, 0xf42f0, 0x113da0, 0x113df0, 0x110ab0, 0xe4050];

const UNIT_STATE_SEATS: u32 = 0x0069_8000;
const UNIT_STATE_MATG: u32 = 0x0069_8100;
const UNIT_STATE_MATG_RECORD: u32 = 0x0069_8200;
const UNIT_STATE_PLAYERS: u32 = 0x0069_8300;

/// The slice 6b stubs' answers beyond [`answer`]'s: the name `10f5f0`
/// gives is one the bodies test for (read from the instructions that
/// compare it) or another by turns, `10f720` answers 0 to 2, `cbec0` none
/// or a seat, and `d1080` fills the control frame its two arguments point
/// at and answers whether it has one.
fn slice_6b_answer(cpu: &mut Cpu, g: &mut Guest, target: u32, n: usize, case: u32) {
    let k = n as u32 + case;
    match target {
        t if SLICE_6B_ANSWERS_AL.contains(&t) => cpu.gpr[0] = k % 2,
        0x10f5f0 => {
            cpu.gpr[0] = match k % 3 {
                0 => g.u32(0xc4cf5).unwrap(),
                1 => g.u32(0xc4cfd).unwrap(),
                _ => 0x0abc_0004,
            }
        }
        0x10f720 => cpu.gpr[0] = k % 3,
        0xcbec0 => cpu.gpr[0] = if k % 2 == 0 { 0xffff } else { 0x0001_0001 },
        0xd1080 => {
            let sp = cpu.gpr[4];
            let (frame, scales) = (g.u32(sp + 4).unwrap(), g.u32(sp + 8).unwrap());
            // Three scales, then the two thresholds (positive in turns).
            let thresholds = [[0.5, -1.0], [0.0, 0.25], [2.0, 0.0], [0.125, 0.125]][(k % 4) as usize];
            for (i, v) in [1.0, 0.5, 2.0, thresholds[0], thresholds[1]].into_iter().enumerate() {
                f32w(g, scales + 4 * i as u32, v);
            }
            // A basis and a position: 13 floats.
            for i in 0..13u32 {
                f32w(g, frame + 4 * i, (i as f32 - 4.0) * 0.25 + case as f32);
            }
            cpu.gpr[0] = u32::from(case % 4 != 3);
        }
        _ => {}
    }
}

/// [`unit_dress`], then the fields the slice 6b bodies branch on, varied
/// by `case`.
fn unit_state_dress(g: &mut Guest, case: u32) {
    unit_dress(g, case);
    let c = case as usize % 8;
    // Definition: `+bc` bits 10 (no states), 11 (no update) and 28 (the
    // `+334` floats); the powered seats; the `+1d4..+1e0` rates.
    let bits = [0, 1 << 28, 1 << 11, 1 << 10 | 1 << 28, 1 << 28, 1 << 10, 0, 1 << 28][c];
    let bc = g.u32(DEFINITION + 0xbc).unwrap() & !(1 << 10 | 1 << 11 | 1 << 28);
    word(g, DEFINITION + 0xbc, bc | bits);
    word(g, DEFINITION + 0x1b8, [0, 1, 2, 2, 3, 2, 1, 2][c]);
    word(g, DEFINITION + 0x1bc, UNIT_STATE_SEATS);
    for (i, t) in [0.5f32, 0.0, 0.25].into_iter().enumerate() {
        f32w(g, UNIT_STATE_SEATS + 8 * i as u32, t);
    }
    for (i, r) in [1.0f32, 2.0, 0.5, 0.25].into_iter().enumerate() {
        f32w(g, DEFINITION + 0x1d4 + 4 * i as u32, r);
    }
    // The weapon definition (the held weapon's is this one too): its
    // `+1fe` count and two names.
    g.set_u16(DEFINITION + 0x1fe, [0, 1, 3][c % 3]).unwrap();
    word(g, DEFINITION + 0x270, if c == 6 { u32::MAX } else { 0x00e7_0001 });
    word(g, DEFINITION + 0x278, 0x00e7_0002);
    // The globals' record the state update falls back to.
    word(g, points::GLOBALS_TAG.base, UNIT_STATE_MATG);
    word(g, UNIT_STATE_MATG + 0x134, UNIT_STATE_MATG_RECORD);
    word(g, UNIT_STATE_MATG_RECORD + 0xb8, [0, 1, 4][c % 3]);
    word(g, UNIT_STATE_MATG_RECORD + 0xc8, if c == 1 { u32::MAX } else { 0x00e7_0003 });
    word(g, UNIT_STATE_MATG_RECORD + 0xd0, 0x00e7_0004);
    word(g, points::PLAYER_GLOBALS.base, UNIT_STATE_PLAYERS);
    byte(g, UNIT_STATE_PLAYERS + 6, u8::from(c % 3 == 1));

    let body = g.u32(DATA + 12 * (object(case) & 0xffff) + 8).unwrap();
    // Flag words: `+134` bits 1, 21, 22, 25, 26 and 28; `+148`.
    word(g, body + 0x134, [1 << 21, 2, 0, 1 << 26 | 1 << 22, 1 << 28 | 1 << 25, 1 << 26 | 2, 1 << 21 | 1 << 26, 0][c]);
    word(g, body + 0x148, [0, 0x0800_0000, 0x4000, 0x1000_0000, 0x0001_0005, 0xffff_0000, 0x20, 0x3fff_0000][c]);
    // The timed bit, only where the image's rate gives a non-zero divisor.
    let rate = f32::from_bits(g.u32(0x0045_dc20).unwrap()) * 30.0;
    let timed = (1.0..1.0e6).contains(&rate);
    word(g, body + 0x1ec, if timed { [0, 3, 15, 1, 0, 30, 2, 0][c] } else { 0 });
    word(g, body + 0x1f0, [0, 0x10000, 0x10000, 0x10005, 0, 0x10000, 0, 0][c]);
    // Timers.
    byte(g, body + 0x1f5, [0, 1, 3, 0x80, 1, 0, 2, 0][c]);
    byte(g, body + 0x1fa, [0, 2, 0, 1, 0, 1, 0, 3][c]);
    byte(g, body + 0x1fb, [1, 0, 0, 2, 0, 0, 1, 0][c]);
    byte(g, body + 0x19, if c == 5 { 8 } else { 0 });
    byte(g, body + 0xaa, [0, 1, 0, 1, 1, 0, 0, 1][c]);
    // Linked objects: none, or live pool objects.
    word(g, body + 0x248, [u32::MAX, handle(3), handle(5), u32::MAX, handle(3), handle(1), u32::MAX, handle(5)][c]);
    word(g, body + 0x24c, [u32::MAX, u32::MAX, handle(3), handle(5), handle(3), u32::MAX, handle(1), handle(4)][c]);
    // Held weapons: the previous indices, the seats, the animation state.
    byte(g, body + 0x216, [0xff, 0, 1, 0, 0xff, 1, 0, 0xff][c]);
    byte(g, body + 0x217, [0xff, 0xff, 1, 1, 0, 0xff, 0, 1][c]);
    byte(g, body + 0x23c, [0, 1, 2, 2, 0, 3, 1, 0][c]);
    byte(g, body + 0x23d, [0, 2, 2, 1, 1, 3, 0, 2][c]);
    byte(g, body + 0x240, [0xff, 0, 1, 0xff, 2, 0, 0xff, 1][c]);
    byte(g, body + 0x241, [0, 0, 2, 1, 0xff, 1, 3, 1][c]);
    g.set_u16(body + 0x1fc, if c % 2 == 0 { 0xffff } else { 1 }).unwrap();
    // Seat power, control history and smoothed changes, the `+334` pair.
    for (i, p) in [0.0f32, 1.0, 0.5].into_iter().enumerate() {
        f32w(g, body + 0x25c + 4 * i as u32, p);
    }
    for i in 0..12u32 {
        f32w(g, body + 0x270 + 4 * i, (i as f32 - 6.0) * 0.5 * (c as f32 + 1.0));
    }
    f32w(g, body + 0x334, [0.0, 1.0, 0.5, 4.0, 0.25, 0.0, 2.0, 1.5][c]);
    f32w(g, body + 0x338, [0.0, 0.0, 0.5, 1.0, 0.0, 0.25, 0.0, 2.0][c]);
    f32w(g, body + 0x1c0, 0.75);
    f32w(g, body + 0x1c4, 0.25);
}

/// The arguments of a slice 6b root: the unit on the stack (`c49b0`,
/// `c58f0`), in `eax` (`c5eb0`) or in `edi` (`ce0c0`).
fn unit_state_entry(g: &mut Guest, cpu: &mut Cpu, root: u32, case: u32) {
    let unit = object(case);
    match root {
        r if r == points::UNIT_POWERED_SEATS.base => cpu.gpr[0] = unit,
        r if r == points::UNIT_CONTROL_REFRESH.base => cpu.gpr[7] = unit,
        _ => word(g, STACK + 4, unit),
    }
}

/// [`transcripts`] of a slice 6b root over a [`unit_state_dress`]ed pool;
/// the float state is compared like every root's.
fn unit_state_transcripts(point: &points::Point, cases: u32) {
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
    UNIT.with(|u| u.set(true));
    let mut h = Harness { code: &mut code, table, root: point.base, case: 0 };
    for case in 0..cases {
        h.case = case;
        let fresh = || {
            let (mut cpu, mut g) = start(ram, point.base, case);
            unit_state_dress(&mut g, case);
            unit_state_entry(&mut g, &mut cpu, point.base, case);
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
    UNIT.with(|u| u.set(false));
}

#[test]
fn unit_update_control() {
    unit_state_transcripts(&points::UNIT_UPDATE_CONTROL, 8);
}

#[test]
fn unit_update_state() {
    unit_state_transcripts(&points::UNIT_UPDATE_STATE, 8);
}

#[test]
fn unit_powered_seats() {
    unit_state_transcripts(&points::UNIT_POWERED_SEATS, 8);
}

#[test]
fn unit_control_refresh() {
    unit_state_transcripts(&points::UNIT_CONTROL_REFRESH, 8);
}

/// The unit update over the slice 6b dressing: the shell fuses the
/// control, the state, the movement and the seats.
#[test]
fn unit_update_over_the_state_children() {
    unit_state_transcripts(&points::UNIT_UPDATE, 8);
}
