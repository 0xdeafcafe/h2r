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
//! Needs the user's XBE (`lifted::image`); skipped without it.
#![cfg(feature = "differential")]

use engine::entry::{self, Regs};
use engine::memory::GuestMemory;
use engine::sim_tick::{self, PHASES};
use engine::testing::FakeGuest;
use engine::entry::EntryFuture;
use engine::points;
use ir::interp::{Cpu, FlatMemory, Flow};
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
