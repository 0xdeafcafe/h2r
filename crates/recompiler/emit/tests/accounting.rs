//! Access accounting through generated C (h2gen.h, "Access accounting";
//! synthetic code, no game data). The same functions built with and without
//! it run under a slice of a few accesses, through a runner that, like the
//! runtime's, gives up the slice at a safe point once the preempt word is
//! set and re-arms it. With accounting:
//!
//! - every run ends in the same state, memory and charged total;
//! - a loop gives up its slice at the same safe points with the same
//!   accesses charged, as its count reaches the budget where the charged
//!   accessors would have set the word;
//! - a REP string instruction can give up its slice mid-instruction, at its
//!   own PC, at most one iteration after the budget ran out, and resumes
//!   where it stopped.

mod common;

use std::sync::atomic::Ordering;

use common::function;
use emit::host::{CpuAbi, Ctx, ENTRY, FAULT, HARNESS_EXIT, JUMP, RETURN, STOP, TransferAbi};
use iced_x86::code_asm::*;
use ir::IrFunction;
use ir::interp::FlatMemory;

const A: u32 = 0x0040_0000;
const BASE: u32 = 0x0010_0000;
const STACK: u32 = BASE + 0x1000;
const SRC: u32 = BASE + 0x100;
const DST: u32 = BASE + 0x500;
const WORDS: u32 = 64;

/// How one run went: where it gave up its slice (target, accesses charged
/// by then), how it ended, and the state it ended in.
#[derive(Debug, PartialEq)]
struct Run {
    stops: Vec<(u32, u64)>,
    end: (u32, u32),
    charged: u64,
    cpu: CpuAbi,
    mem: Vec<u8>,
}

/// Run from `A`; see [`drive_at`].
fn drive(g: &emit::host::Generated, cpu: CpuAbi, mem: &FlatMemory, slice: u64) -> Run {
    drive_at(g, cpu, mem, slice, A)
}

fn state(regs: [(usize, u32); 3]) -> (CpuAbi, FlatMemory) {
    let mut mem = FlatMemory::new(BASE, 0x1000);
    for (i, b) in mem.bytes.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(0x9e37_79b9).to_le_bytes()[3];
    }
    mem.load(STACK - 8, &HARNESS_EXIT.to_le_bytes());
    let mut cpu = CpuAbi::default();
    cpu.gpr[4] = STACK - 8;
    for (r, v) in regs {
        cpu.gpr[r] = v;
    }
    (cpu, mem)
}

/// Both builds of `funcs` (each at its own base), or `None` without clang
/// and the ABI header.
fn builds(name: &str, funcs: Vec<IrFunction>) -> Option<[emit::host::Generated; 2]> {
    let Some(abi) = common::abi_header() else {
        common::skip(name, "needs h2abi.h (H2ABI_HEADER)");
        return None;
    };
    let owners = emit::plan(funcs);
    Some([false, true].map(|accounting| {
        common::build_with(&owners, &abi, &format!("{name}-{accounting}"), emit::Codegen::accesses(accounting))
    }))
}

const ESI: usize = 6;
const EDI: usize = 7;
const ECX: usize = 1;

#[test]
fn a_counting_loop_gives_up_its_slice_where_a_charging_one_does() {
    const NAME: &str = "accounting-loops";
    // Loads and stores round a loop, then an atomic in one.
    let memory = function(A, |a| {
        let mut l = a.create_label();
        a.xor(eax, eax)?;
        a.set_label(&mut l)?;
        a.add(eax, dword_ptr(esi))?;
        a.mov(dword_ptr(edi), eax)?;
        a.add(esi, 4)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    });
    let atomic = function(A, |a| {
        let mut l = a.create_label();
        a.set_label(&mut l)?;
        a.lock().inc(dword_ptr(edi))?;
        a.add(eax, dword_ptr(esi))?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    });
    for (which, f) in [("memory", memory), ("atomic", atomic)] {
        let Some([charging, counting]) = builds(&format!("{NAME}-{which}"), vec![f]) else { return };
        for slice in [1, 2, 3, 5, 7, 64, 1000] {
            let (cpu, mem) = state([(ESI, SRC), (EDI, DST), (ECX, 20)]);
            let want = drive(&charging, cpu, &mem, slice);
            let got = drive(&counting, cpu, &mem, slice);
            assert_eq!((want.end.0, want.end.1), (RETURN, HARNESS_EXIT));
            assert_eq!(!want.stops.is_empty(), slice <= want.charged, "{which} {slice}: {:?}", want.stops);
            assert_eq!(got, want, "{which}, slice {slice}");
        }
    }
}

#[test]
fn a_rep_string_counts_each_iteration_and_can_stop_part_way() {
    const NAME: &str = "accounting-rep";
    let movs = function(A, |a| {
        a.rep().movsd()?;
        a.ret()
    });
    let Some([charging, counting]) = builds(NAME, vec![movs]) else { return };
    for slice in [1, 2, 3, 5, 64, 1000] {
        let (cpu, mem) = state([(ESI, SRC), (EDI, DST), (ECX, WORDS)]);
        let want = drive(&charging, cpu, &mem, slice);
        let got = drive(&counting, cpu, &mem, slice);
        // Two accesses an iteration, and the return address.
        assert_eq!(want.charged, 2 * WORDS as u64 + 1);
        assert_eq!((&got.end, got.charged, &got.cpu, &got.mem), (&want.end, want.charged, &want.cpu, &want.mem));
        // Charging, the instruction runs to the end in one go; counting, it
        // stops at its own PC once a slice is spent, within an iteration.
        assert!(want.stops.iter().all(|&(pc, _)| pc != A), "{slice}: {:?}", want.stops);
        let mut last = 0;
        for &(pc, charged) in &got.stops {
            if pc == A {
                let spent = charged - last;
                assert!(spent >= slice && spent < slice + 2, "slice {slice}: {spent} spent: {:?}", got.stops);
            }
            last = charged;
        }
        if slice < 2 * WORDS as u64 {
            assert!(got.stops.iter().any(|&(pc, _)| pc == A), "{slice}: {:?}", got.stops);
        }
    }
}

#[test]
fn rep_edge_cases_end_the_same_with_the_same_charge() {
    const NAME: &str = "accounting-rep-edges";
    let movs = function(A, |a| {
        a.rep().movsd()?;
        a.ret()
    });
    let cmps = function(A + 0x1000, |a| {
        a.repe().cmpsb()?;
        a.ret()
    });
    let Some([charging, counting]) = builds(NAME, vec![movs, cmps]) else { return };
    let cases = [
        // No iterations.
        (A, [(ESI, SRC), (EDI, DST), (ECX, 0)]),
        // Runs off the end of memory part-way: a fault mid-REP.
        (A, [(ESI, SRC), (EDI, STACK - 0x40), (ECX, WORDS)]),
        // Equal for 8 bytes (the same buffer), then unequal: an early exit.
        (A + 0x1000, [(ESI, SRC), (EDI, SRC), (ECX, 8)]),
        (A + 0x1000, [(ESI, SRC), (EDI, DST), (ECX, 64)]),
    ];
    for (entry, regs) in cases {
        for slice in [1, 3, 1000] {
            let (cpu, mem) = state(regs);
            let run = |g| drive_at(g, cpu, &mem, slice, entry);
            let (want, got) = (run(&charging), run(&counting));
            assert_eq!(
                (&got.end, got.charged, &got.cpu, &got.mem),
                (&want.end, want.charged, &want.cpu, &want.mem),
                "{entry:#x} {regs:x?} slice {slice}"
            );
        }
    }
}

/// Run `entry` from `cpu`, with a slice of `slice` accesses re-armed after
/// each safe point where it ran out, as the kernel arms each thread slice.
fn drive_at(g: &emit::host::Generated, cpu: CpuAbi, mem: &FlatMemory, slice: u64, entry: u32) -> Run {
    let mut ctx = Ctx::new(mem.clone());
    ctx.slice = Some(slice);
    let mut cpu = cpu;
    let mut t = TransferAbi::new(JUMP, ENTRY, entry);
    let mut stops = Vec::new();
    for _ in 0..100_000 {
        if matches!(t.kind, FAULT | STOP) || (t.kind == RETURN && t.target_pc == HARNESS_EXIT) {
            return Run { stops, end: (t.kind, t.target_pc), charged: ctx.charged, cpu, mem: ctx.mem.bytes };
        }
        // One handler call, then the runner's safe point.
        t = g.run(&mut cpu, &mut ctx, t, 1).0;
        if ctx.preempt.swap(0, Ordering::Relaxed) != 0 {
            stops.push((t.target_pc, ctx.charged));
            ctx.slice = Some(slice);
        }
    }
    panic!("did not finish")
}
