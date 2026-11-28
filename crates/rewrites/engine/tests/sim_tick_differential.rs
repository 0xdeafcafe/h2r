//! The tick driver against the lifted `game_tick` (docs/engine/sim-rewrite.md,
//! "The tick driver"): the lifted body runs on the IR interpreter with every
//! callee answered by a stub that writes nothing, and the driver's async
//! body runs on a sparse guest with the same answers. The call transcripts
//! must agree, and so must what each leaves in the game-time record.
//!
//! `phases_run_in_retail_order`: the lifted body's calls, less the four
//! envelope calls, are `PHASES` in order. That covers the odd phase
//! addresses (`0x165dc1`, `0x1917b2`): the transcript names the target the
//! body really reaches.
//! `tick_matches_lifted`: the envelope's calls (the control word, the
//! record's build, before and after) take the same registers and stack
//! arguments, the record is the same place relative to the entry stack, and
//! the tick count moves the same way. The phases' registers are compared
//! too: phase 1's stack argument and `ECX`, `EBX` = record on the rest; the
//! other registers are not (the lifted body keeps callee-saved registers
//! loaded that the driver does not). Both runs seed the tick clock, and the
//! page compare covers its writes.
//!
//! `ai_update_matches_lifted`, `hs_update_matches_lifted`: the Rust phase
//! bodies against their lifted functions, the unported callees stubbed.
//!
//! Needs the user's XBE (`lifted::image`); skipped without it.
#![cfg(feature = "differential")]

use engine::entry::{self, Regs};
use engine::memory::GuestMemory;
use engine::sim_tick::{self, PHASES};
use engine::testing::FakeGuest;
use engine::entry::EntryFuture;
use engine::points;
use ir::interp::{Cpu, FlatMemory, Flow};
use lifted::Code;
use std::sync::Mutex;

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = 0xffff_fffc;
const TIME: u32 = 0x0060_0000;
const TICK: u32 = 1234;
/// The tick clock `TICK_CLOCK` points at: inside the game-time page.
const CLOCK: u32 = TIME + 0x100;

/// What one call into the guest saw: the target, the registers the
/// envelope sets, and the first two stack arguments. The record's address
/// is written `RECORD`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    target: u32,
    eax: Option<u32>,
    ecx: Option<u32>,
    ebx: Option<u32>,
    esi: Option<u32>,
    args: [u32; 2],
}

const RECORD: u32 = 0xdead_0000;

/// `_chkstk`, which the lifted body calls to allocate its frame.
const STACK_PROBE: u32 = 0x0032_0560;

fn is_envelope(target: u32) -> bool {
    [
        sim_tick::CONTROL_WORD_FN,
        sim_tick::RECORD_BUILD,
        sim_tick::RECORD_BEFORE,
        sim_tick::RECORD_AFTER,
    ]
    .contains(&target)
}

/// The callee's stack cleanup: the record's `before` and phase 1 are
/// `ret 4`.
fn cleanup(target: u32) -> u32 {
    u32::from(target == sim_tick::RECORD_BEFORE || target == PHASES[0].entry) * 4
}

/// The clock's inputs: the tick length at `TIME + 4`, the clock's time and
/// counter. The same values go to the driver's guest.
const LENGTH: f32 = 0.25;

fn seed_clock(mem: &mut FlatMemory) {
    assert!(mem.load(TIME + 4, &LENGTH.to_le_bytes()));
    assert!(mem.load(points::TICK_CLOCK.base, &CLOCK.to_le_bytes()));
    assert!(mem.load(CLOCK + 4, &1.0f32.to_le_bytes()));
    assert!(mem.load(CLOCK + 0x14, &5u32.to_le_bytes()));
}

/// The lifted body, run to its return: every call it makes.
fn lifted_calls() -> Option<(Vec<Seen>, u32, FlatMemory)> {
    let image = lifted::image::Image::get()?;
    let mut mem = image.ram.clone();
    mem.bytes.truncate(RAM);
    let mut code = image.original();
    let base = points::GAME_TICK.base;
    code.prime(base);
    assert!(code.insn(base).is_some(), "GAME_TICK is not in the image");
    assert!(mem.load(STACK, &EXIT.to_le_bytes()));
    assert!(mem.load(points::GAME_TIME.base, &TIME.to_le_bytes()));
    assert!(mem.load(TIME + 8, &TICK.to_le_bytes()));
    seed_clock(&mut mem);
    let mut cpu = Cpu { gpr: [0, 0, 0, 0, STACK, 0, 0, 0], eip: base, eflags: 0x2, ..Default::default() };
    let mut seen = Vec::new();
    // The record, as the lifted body names it: below its entry stack by
    // the frame the lifted body keeps. The first call that takes it tells.
    let mut record = None;
    for _ in 0..2_000_000 {
        match lifted::code::step(&mut code, &mut cpu, &mut mem).unwrap_or_else(|e| panic!("{e}")) {
            Flow::Return { .. } => {
                let tick = mem.read_u32(TIME + 8).unwrap();
                return Some((seen, tick, mem));
            }
            Flow::Call { target, .. } if target == STACK_PROBE => {
                // `_chkstk` (0x320560): it probes the frame's pages and
                // lowers ESP by EAX bytes under its own return address. It
                // is the frame's allocation, not a phase.
                let sp = cpu.gpr[4];
                let next = mem.read_u32(sp).unwrap();
                cpu.gpr[4] = (sp + 4).wrapping_sub(cpu.gpr[0]);
                cpu.eip = next;
            }
            Flow::Call { target, .. } => {
                let sp = cpu.gpr[4];
                let arg = |n: u32| mem.read_u32(sp + 4 + 4 * n).unwrap();
                if target == sim_tick::RECORD_BUILD {
                    record = Some(cpu.gpr[0]);
                }
                let norm = |v: u32| if Some(v) == record { RECORD } else { v };
                seen.push(Seen {
                    target,
                    eax: Some(norm(cpu.gpr[0])),
                    ecx: Some(norm(cpu.gpr[1])),
                    ebx: Some(norm(cpu.gpr[3])),
                    esi: Some(norm(cpu.gpr[6])),
                    args: [norm(arg(0)), norm(arg(1))],
                });
                let next = mem.read_u32(sp).unwrap();
                if target == sim_tick::RECORD_BEFORE {
                    // The before step sets the record's run flag.
                    let rec = arg(0);
                    assert!(mem.load(rec + sim_tick::RUN_FLAG, &[1u8]));
                }
                cpu.gpr[0] = 0;
                cpu.gpr[4] = sp + 4 + cleanup(target);
                cpu.eip = next;
            }
            _ => {}
        }
    }
    panic!("the lifted tick did not return");
}

/// The driver's body, run to its return on a sparse guest.
fn driver_calls() -> (Vec<Seen>, u32, FakeGuest) {
    let mut g = FakeGuest::new();
    g.map(STACK - 0x8000, 0x10000);
    g.put_u32(points::GAME_TIME.base, TIME).put_u32(TIME + 8, TICK).put_f32(TIME + 4, LENGTH);
    g.put_u32(points::TICK_CLOCK.base, CLOCK).put_f32(CLOCK + 4, 1.0).put_u32(CLOCK + 0x14, 5);
    let ledger: &'static Mutex<sim_tick::Ledger> = Box::leak(Box::new(Mutex::new(sim_tick::Ledger::new())));
    let f = move |cx: entry::Cx| -> EntryFuture { Box::pin(sim_tick::run(cx, ledger)) };
    let regs = Regs { esp: STACK, ..Default::default() };
    let record = STACK - sim_tick::RECORD_SIZE;
    let callee = |c: &entry::GuestCall, m: &mut dyn GuestMemory| {
        if c.target == sim_tick::RECORD_BEFORE {
            m.set_u8(record + sim_tick::RUN_FLAG, 1).unwrap();
        }
        Regs::default()
    };
    let run = entry::testing::run(&f, g, regs, callee);
    assert_eq!(run.result, Ok(entry::Ret::new(0, 0)));
    let norm = |v: u32| if v == record { RECORD } else { v };
    let seen = run
        .calls
        .iter()
        .map(|c| Seen {
            target: c.target,
            eax: c.eax.map(norm),
            ecx: c.ecx.map(norm),
            ebx: c.ebx.map(norm),
            esi: c.esi.map(norm),
            args: [c.args.first().copied().map_or(0, norm), c.args.get(1).copied().map_or(0, norm)],
        })
        .collect();
    let tick = run.guest.u32(TIME + 8).unwrap();
    (seen, tick, run.guest)
}

#[test]
fn phases_run_in_retail_order() {
    let Some((lifted, _, _)) = lifted_calls() else { return };
    let phases: Vec<u32> = lifted.iter().map(|c| c.target).filter(|&t| !is_envelope(t)).collect();
    let want: Vec<u32> = PHASES.iter().map(|p| p.entry).collect();
    assert_eq!(phases, want, "the lifted body's phase calls (both runs set the record's run flag at the before step)");
    let envelope: Vec<u32> = lifted.iter().map(|c| c.target).filter(|&t| is_envelope(t)).collect();
    assert_eq!(
        envelope,
        [
            sim_tick::CONTROL_WORD_FN,
            sim_tick::RECORD_BUILD,
            sim_tick::RECORD_BEFORE,
            sim_tick::RECORD_AFTER
        ]
    );
}

#[test]
fn tick_matches_lifted() {
    let Some((lifted, lifted_tick, lifted_mem)) = lifted_calls() else { return };
    let (ours, our_tick, guest) = driver_calls();
    // What each envelope call takes: the control word's two arguments, the
    // record in `EAX` for the build, on the stack for the before, in `ESI`
    // for the after.
    let envelope = |v: &[Seen]| -> Vec<(u32, [u32; 2])> {
        v.iter()
            .filter(|c| is_envelope(c.target))
            .map(|c| {
                let taken = match c.target {
                    sim_tick::RECORD_BUILD => [c.eax.unwrap_or(0), 0],
                    sim_tick::RECORD_BEFORE => [c.args[0], 0],
                    sim_tick::RECORD_AFTER => [c.esi.unwrap_or(0), 0],
                    _ => c.args,
                };
                (c.target, taken)
            })
            .collect()
    };
    assert_eq!(envelope(&ours), envelope(&lifted));
    // The phases' registers: phase 1 takes the record on the stack and in
    // ECX, the rest take EBX = record.
    let phase_regs = |v: &[Seen]| -> Vec<(u32, u32, u32, u32)> {
        v.iter()
            .filter(|c| !is_envelope(c.target))
            .map(|c| {
                if c.target == PHASES[0].entry {
                    (c.target, c.args[0], c.ecx.unwrap_or(0), 0)
                } else {
                    (c.target, 0, 0, c.ebx.unwrap_or(0))
                }
            })
            .collect()
    };
    assert_eq!(phase_regs(&ours), phase_regs(&lifted));
    // The control word's arguments are retail's, including the mask.
    let control = |v: &[Seen]| v.iter().find(|c| c.target == sim_tick::CONTROL_WORD_FN).map(|c| c.args);
    assert_eq!(control(&ours), Some([sim_tick::CONTROL_WORD_NEW, 0x000f_ffff]));
    assert_eq!(our_tick, TICK + 1);
    assert_eq!(lifted_tick, our_tick, "the tick count");
    // The game-time record's page, byte for byte.
    let page = TIME & !0xfff;
    let ours_page = guest.bytes(page, 0x1000).unwrap();
    assert_eq!(ours_page, lifted_mem.bytes[page as usize..page as usize + 0x1000]);
}

// The Rust phase bodies against their lifted functions
// (`engine::ai_update`, `engine::hs_update`): the lifted body runs on the
// interpreter with its unported callees (`stubs`) answered by a stub that
// writes nothing and every other callee lifted, and the Rust body runs on a
// guest holding the same RAM with the same stubs. The stubbed calls (target
// and `edi`) and every byte of RAM but the dead stack must agree.

/// One stubbed call: the target and `edi`.
type Stubbed = (u32, u32);

/// The lifted function at `entry`, run to its return on `mem`: its calls
/// into `stubs`. A tail jump into a stub counts as its call.
fn lifted_phase(entry: u32, stubs: &[u32], mem: &mut FlatMemory) -> Vec<Stubbed> {
    let image = lifted::image::Image::get().expect("image");
    let mut code = image.original();
    code.prime(entry);
    assert!(mem.load(STACK, &EXIT.to_le_bytes()));
    let mut cpu = Cpu { gpr: [0, 0, 0, 0, STACK, 0, 0, 0], eip: entry, eflags: 0x2, mxcsr: 0x1fa0, ..Default::default() };
    let mut seen = Vec::new();
    for _ in 0..2_000_000 {
        if stubs.contains(&cpu.eip) {
            // A tail jump: the return address is the caller's.
            seen.push((cpu.eip, cpu.gpr[7]));
            let sp = cpu.gpr[4];
            cpu.eip = mem.read_u32(sp).unwrap();
            cpu.gpr[4] = sp + 4;
        }
        if cpu.eip == EXIT {
            return seen;
        }
        match lifted::code::step(&mut code, &mut cpu, mem).unwrap_or_else(|e| panic!("{e}")) {
            Flow::Call { target, .. } if stubs.contains(&target) => {
                seen.push((target, cpu.gpr[7]));
                let sp = cpu.gpr[4];
                cpu.eip = mem.read_u32(sp).unwrap();
                cpu.gpr[4] = sp + 4;
                cpu.gpr[0] = 0;
            }
            Flow::Return { target } if target == EXIT => return seen,
            _ => {}
        }
    }
    panic!("{entry:#x} did not return");
}

/// The Rust body `f` on a guest holding `mem`: its guest calls, and the
/// guest after.
fn rust_phase(
    f: fn(entry::Cx) -> EntryFuture,
    mem: &FlatMemory,
) -> (Vec<Stubbed>, FakeGuest) {
    let mut g = FakeGuest::new();
    g.put(0, &mem.bytes);
    let run = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, |_, _| Regs::default());
    assert_eq!(run.result, Ok(entry::Ret::new(0, 0)));
    let calls = run.calls.iter().map(|c| (c.target, c.edi.unwrap_or(0))).collect();
    (calls, run.guest)
}

/// Every byte of `lifted` and `ours` outside the stack page under `STACK`.
fn same_ram(lifted: &FlatMemory, ours: &FakeGuest, why: &str) {
    let mut got = vec![0u8; lifted.bytes.len()];
    ours.read(0, &mut got).unwrap();
    let dead = (STACK as usize - 0x1000)..(STACK as usize + 4);
    let first = lifted.bytes.iter().zip(&got).enumerate().find(|(i, (a, b))| a != b && !dead.contains(i));
    assert_eq!(first.map(|(i, _)| format!("{i:#x}")), None, "{why}: first differing byte");
}

fn image_ram() -> Option<FlatMemory> {
    let image = lifted::image::Image::get()?;
    let mut mem = image.ram.clone();
    mem.bytes.truncate(RAM);
    Some(mem)
}

fn put(mem: &mut FlatMemory, at: u32, bytes: &[u8]) {
    assert!(mem.load(at, bytes), "{at:#x}");
}

#[test]
fn ai_update_matches_lifted() {
    use engine::ai_update::{self, ENGINE, GLOBALS, SYSTEMS, TEARDOWN};
    let Some(ram) = image_ram() else { return };
    const G: u32 = 0x0060_0000;
    const MODE: u32 = 0x0060_1000;
    let stubs: Vec<u32> = SYSTEMS.iter().copied().chain([TEARDOWN]).collect();
    let f: fn(entry::Cx) -> EntryFuture = |cx| {
        Box::pin(async move {
            ai_update::run(&cx, 0x40).await?;
            Ok(entry::Ret::new(0, 0))
        })
    };
    let mut cases = 0;
    for on in [0u8, 1] {
        for enabled in [0u8, 1] {
            for ran in [0u8, 1] {
                for mode in [1u8, 4] {
                    for countdown in [0i16, 1, 30, -2] {
                        let why = format!("on {on} enabled {enabled} ran {ran} mode {mode} countdown {countdown}");
                        let mut mem = ram.clone();
                        put(&mut mem, GLOBALS, &G.to_le_bytes());
                        put(&mut mem, ENGINE, &MODE.to_le_bytes());
                        put(&mut mem, MODE + 0xc, &[mode]);
                        put(&mut mem, G, &[on, enabled, ran]);
                        put(&mut mem, G + 0x22, &countdown.to_le_bytes());
                        let (ours, guest) = rust_phase(f, &mem);
                        let lifted = lifted_phase(ai_update::ENTRY, &stubs, &mut mem);
                        let targets = |v: &[Stubbed]| v.iter().map(|c| c.0).collect::<Vec<_>>();
                        assert_eq!(targets(&ours), targets(&lifted), "{why}");
                        same_ram(&mem, &guest, &why);
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 64);
}

#[test]
fn hs_update_matches_lifted() {
    use engine::hs_update::{self, AFTER, ENABLED, EXECUTE};
    let Some(ram) = image_ram() else { return };
    const POOL: u32 = 0x0060_0000;
    const BITMAP: u32 = 0x0060_0800;
    const DATA: u32 = 0x0060_1000;
    const THREAD: u32 = 0x418;
    let f: fn(entry::Cx) -> EntryFuture = |cx| {
        Box::pin(async move {
            hs_update::run(&cx, 0x40).await?;
            Ok(entry::Ret::new(0, 0))
        })
    };
    let mut seed = 0x1234_5678u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut ran = 0;
    for case in 0..200 {
        let mut mem = ram.clone();
        let enabled = u8::from(case % 10 != 0);
        let slots = 1 + next() % 12;
        put(&mut mem, ENABLED, &[enabled]);
        put(&mut mem, points::GAME_TIME.base, &TIME.to_le_bytes());
        put(&mut mem, TIME + 8, &50i32.to_le_bytes());
        put(&mut mem, engine::hs_pool::THREADS, &POOL.to_le_bytes());
        put(&mut mem, POOL + 0x20, &32u32.to_le_bytes());
        put(&mut mem, POOL + 0x24, &THREAD.to_le_bytes());
        put(&mut mem, POOL + 0x38, &slots.to_le_bytes());
        put(&mut mem, POOL + 0x44, &DATA.to_le_bytes());
        put(&mut mem, POOL + 0x48, &BITMAP.to_le_bytes());
        let mut bits = 0u32;
        for i in 0..slots {
            let r = next();
            if r % 5 == 0 {
                continue;
            }
            bits |= 1 << i;
            let at = DATA + THREAD * i;
            let kind = [0u8, 1, 2, 4][(r >> 3) as usize % 4];
            let wake = [0i32, 50, 51, -1, 10][(r >> 6) as usize % 5];
            put(&mut mem, at, &(0x8000 + i as u16).to_le_bytes());
            put(&mut mem, at + 2, &[kind]);
            put(&mut mem, at + 8, &wake.to_le_bytes());
        }
        put(&mut mem, BITMAP, &bits.to_le_bytes());
        let why = format!("case {case}: {slots} slots, bits {bits:#x}, enabled {enabled}");
        let (ours, guest) = rust_phase(f, &mem);
        let lifted = lifted_phase(hs_update::ENTRY, &[EXECUTE, AFTER], &mut mem);
        assert_eq!(ours, lifted, "{why}: the executor's threads (edi) and the tail call");
        same_ram(&mem, &guest, &why);
        ran += ours.iter().filter(|c| c.0 == EXECUTE).count();
    }
    assert!(ran > 100, "the cases ran threads: {ran}");
}
