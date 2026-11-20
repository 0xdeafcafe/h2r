//! The block clock through generated C (h2gen.h, "Block clock"; synthetic
//! code, no game data; docs/runtime/block-clock-imprecise-faults.md).
//!
//! - A handler charges its function entry's cost once and each loop
//!   header's cost once per trip, and nothing per access: the same
//!   charges for a loop that loads and stores as for one that does not.
//! - Under any slice it gives up its thread at exactly the points a
//!   reference model of the rule predicts from the IR alone (the IR
//!   interpreter, the plan's loop headers and `emit::clock`'s costs). Any
//!   backend that follows the rule interleaves the same way, which is what
//!   a lockstep session between backends needs.
//! - The access-clock build of the same code still counts accesses.

mod common;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::Ordering;

use common::function;
use emit::host::{CpuAbi, Ctx, ENTRY, FAULT, HARNESS_EXIT, JUMP, RESUME, RETURN, STOP, TransferAbi};
use iced_x86::code_asm::*;
use ir::IrFunction;
use ir::interp::{self, FlatMemory};

const A: u32 = 0x0040_0000;
const BASE: u32 = 0x0010_0000;
const STACK: u32 = BASE + 0x1000;
const SRC: u32 = BASE + 0x100;
const DST: u32 = BASE + 0x500;
const ESI: usize = 6;
const EDI: usize = 7;
const ECX: usize = 1;
const TRIPS: u32 = 20;

/// Where a run gave up its slice (target, charged by then), how it ended,
/// the total charged and the end state.
#[derive(Debug, PartialEq)]
struct Run {
    stops: Vec<(u32, u64)>,
    end: (u32, u32),
    charged: u64,
    cpu: CpuAbi,
    mem: Vec<u8>,
}

fn state() -> (CpuAbi, FlatMemory) {
    let mut mem = FlatMemory::new(BASE, 0x1000);
    for (i, b) in mem.bytes.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(0x9e37_79b9).to_le_bytes()[3];
    }
    mem.load(STACK - 8, &HARNESS_EXIT.to_le_bytes());
    let mut cpu = CpuAbi::default();
    cpu.gpr[4] = STACK - 8;
    cpu.gpr[ESI] = SRC;
    cpu.gpr[EDI] = DST;
    cpu.gpr[ECX] = TRIPS;
    (cpu, mem)
}

/// A loop that loads and stores every trip.
fn memory_loop() -> IrFunction {
    function(A, |a| {
        let mut l = a.create_label();
        a.xor(eax, eax)?;
        a.set_label(&mut l)?;
        a.add(eax, dword_ptr(esi))?;
        a.mov(dword_ptr(edi), eax)?;
        a.add(esi, 4)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    })
}

/// The same shape, instruction for instruction, with no memory operand.
fn register_loop() -> IrFunction {
    function(A, |a| {
        let mut l = a.create_label();
        a.xor(eax, eax)?;
        a.set_label(&mut l)?;
        a.add(eax, esi)?;
        a.mov(edi, eax)?;
        a.add(esi, 4)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    })
}

/// A loop whose header is the function entry: entered there, it charges
/// both the entry and the first trip.
fn entry_loop() -> IrFunction {
    function(A, |a| {
        let mut l = a.create_label();
        a.set_label(&mut l)?;
        a.add(eax, dword_ptr(esi))?;
        a.add(esi, 4)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    })
}

/// Run from `A` with a slice of `slice` re-armed after each safe point
/// where it ran out (as the kernel arms each thread slice); `None` runs
/// unarmed.
fn drive(g: &emit::host::Generated, slice: Option<u64>) -> Run {
    let (mut cpu, mem) = state();
    let mut ctx = Ctx::new(mem);
    ctx.slice = slice;
    let mut t = TransferAbi::new(JUMP, ENTRY, A);
    let mut stops = Vec::new();
    for _ in 0..100_000 {
        if matches!(t.kind, FAULT | STOP) || (t.kind == RETURN && t.target_pc == HARNESS_EXIT) {
            return Run { stops, end: (t.kind, t.target_pc), charged: ctx.charged, cpu, mem: ctx.mem.bytes };
        }
        t = g.run(&mut cpu, &mut ctx, t, 1).0;
        if ctx.preempt.swap(0, Ordering::Relaxed) != 0 {
            stops.push((t.target_pc, ctx.charged));
            ctx.slice = slice;
        }
    }
    panic!("did not finish")
}

/// The rule, from the IR alone: entering at the function entry charges
/// `entry_cost`; every arrival at a loop header polls (leaves if the
/// handler's count has reached the budget it read on entry), then charges
/// `header_cost`; a handler charges its count when it leaves, which ends
/// the slice once the count reaches what was left. A resumed header skips
/// its poll. No calls, atomics or REP in these fixtures, so nothing else
/// charges. Returns the stops and the total, as [`drive`] records them.
fn reference(f: &IrFunction, headers: &BTreeSet<u32>, slice: Option<u64>) -> (Vec<(u32, u64)>, u64) {
    let insns: HashMap<u32, &ir::IrInsn> = f.insns().map(|i| (i.pc, i)).collect();
    let (cpu0, mut mem) = state();
    let mut cpu = interp::Cpu::default();
    cpu.gpr = cpu0.gpr;
    let mut left = slice.unwrap_or(0);
    let (mut total, mut stops) = (0u64, Vec::new());
    let mut start = f.entry;
    loop {
        // One handler call.
        let budget = if left > 0 { left } else { u64::MAX };
        let mut count = 0u64;
        if start == f.entry {
            count += emit::clock::entry_cost(f) as u64;
        }
        if headers.contains(&start) {
            count += emit::clock::header_cost(f, start) as u64;
        }
        let mut pc = start;
        let left_at = loop {
            let flow = interp::step(&mut cpu, &mut mem, insns[&pc]).expect("no faults in these fixtures");
            let next = flow.target();
            if next == HARNESS_EXIT {
                break None;
            }
            if headers.contains(&next) {
                if count >= budget {
                    break Some(next);
                }
                count += emit::clock::header_cost(f, next) as u64;
            }
            pc = next;
        };
        // The exit's charge.
        total += count;
        if left > 0 && count > 0 {
            if count >= left {
                left = 0;
            } else {
                left -= count;
            }
            if left == 0 {
                stops.push((left_at.unwrap_or(HARNESS_EXIT), total));
                left = slice.unwrap_or(0);
            }
        }
        match left_at {
            None => return (stops, total),
            Some(h) => start = h,
        }
    }
}

/// The block-clock and access-clock builds of `funcs`, or `None` without
/// clang and the ABI header.
fn builds(name: &str, funcs: Vec<IrFunction>) -> Option<(Vec<emit::Owner>, emit::host::Generated, emit::host::Generated)> {
    let Some(abi) = common::abi_header() else {
        common::skip(name, "needs h2abi.h (H2ABI_HEADER)");
        return None;
    };
    let owners = emit::plan(funcs);
    let block = common::build_with(&owners, &abi, &format!("{name}-block"), emit::Codegen::BLOCK_CLOCK);
    let access = common::build_with(&owners, &abi, &format!("{name}-access"), emit::Codegen::accesses(true));
    Some((owners, block, access))
}

#[test]
fn a_block_clocked_loop_charges_its_entry_and_each_trip_not_its_accesses() {
    let Some((owners, block, access)) = builds("block-clock-loop", vec![memory_loop()]) else { return };
    let o = &owners[0];
    let header = *o.loops.headers.iter().next().expect("one loop");
    let entry = emit::clock::entry_cost(&o.func) as u64;
    let trip = emit::clock::header_cost(&o.func, header) as u64;
    assert_eq!((entry, trip), (1, 5), "xor; then add, mov, add, dec, jnz");
    let run = drive(&block, None);
    assert_eq!(run.end, (RETURN, HARNESS_EXIT));
    assert_eq!(run.charged, entry + TRIPS as u64 * trip);
    // The access clock counts the 2 accesses a trip and the return.
    let counted = drive(&access, None);
    assert_eq!(counted.charged, 2 * TRIPS as u64 + 1);
    assert_eq!((&run.cpu, &run.mem), (&counted.cpu, &counted.mem), "the clock changes no result");
}

#[test]
fn the_block_clock_does_not_move_with_memory_accesses() {
    let Some((_, mem_block, _)) = builds("block-clock-mem", vec![memory_loop()]) else { return };
    let Some((_, reg_block, _)) = builds("block-clock-reg", vec![register_loop()]) else { return };
    for slice in [None, Some(1), Some(3), Some(7), Some(16), Some(64)] {
        let (m, r) = (drive(&mem_block, slice), drive(&reg_block, slice));
        assert_eq!((&m.stops, m.charged), (&r.stops, r.charged), "slice {slice:?}");
    }
}

#[test]
fn every_slice_ends_where_the_ir_rule_says() {
    for (name, f) in [("block-clock-ref-loop", memory_loop()), ("block-clock-ref-entry", entry_loop())] {
        let Some((owners, block, _)) = builds(name, vec![f]) else { return };
        let o = &owners[0];
        let whole = drive(&block, None);
        for slice in [1, 2, 3, 5, 6, 7, 11, 64, 1000] {
            let run = drive(&block, Some(slice));
            let (stops, total) = reference(&o.func, &o.loops.headers, Some(slice));
            assert_eq!((&run.stops, run.charged), (&stops, total), "{name}, slice {slice}");
            assert_eq!(run.charged, whole.charged, "{name}: the total does not depend on the slice");
            assert_eq!((&run.cpu, &run.mem), (&whole.cpu, &whole.mem), "{name}, slice {slice}");
            // Each stop is at a loop header or the end, and between two the
            // clock overshoots the slice by less than one trip.
            let mut last = 0;
            for &(pc, charged) in &run.stops {
                assert!(pc == HARNESS_EXIT || o.loops.headers.contains(&pc), "{name}: stop at {pc:#x}");
                let spent = charged - last;
                let most = emit::clock::entry_cost(&o.func) as u64
                    + o.loops.headers.iter().map(|&h| emit::clock::header_cost(&o.func, h) as u64).max().unwrap_or(0);
                assert!(spent >= slice && spent < slice + most, "{name}, slice {slice}: {spent} spent");
                last = charged;
            }
        }
    }
}

#[test]
fn the_entry_and_header_charges_are_in_the_c() {
    let owners = emit::plan(vec![memory_loop()]);
    let o = &owners[0];
    let shape = |block_clock| emit::c::OwnerShape {
        selectors: &o.selectors,
        loops: &o.loops,
        accounting: true,
        block_clock,
        functions: None,
    };
    let text = emit::c::function(&o.func, &shape(true));
    assert!(text.contains(&format!("case {A:#010x}u: H2_CLOCK(1u); goto")), "{text}");
    let header = o.loops.headers.iter().next().unwrap();
    assert!(text.contains(&format!("P_{header:08x}:;\n    H2_CLOCK(5u);")), "{text}");
    let text = emit::c::function(&o.func, &shape(false));
    assert!(!text.contains("H2_CLOCK"), "{text}");
}

/// The resume selector a stop leaves at is the header's, so the runner
/// re-enters past the poll.
#[test]
fn a_block_clock_stop_resumes_at_the_header() {
    let Some((owners, block, _)) = builds("block-clock-resume", vec![memory_loop()]) else { return };
    let header = *owners[0].loops.headers.iter().next().unwrap();
    let (mut cpu, mem) = state();
    let mut ctx = Ctx::new(mem);
    ctx.slice = Some(1);
    let t = block.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 1).0;
    assert_eq!((t.kind, t.route, t.target_pc), (JUMP, RESUME, header));
}

/// A handler cut into parts (`emit::c::Split`, "Handler parts" in h2gen.h)
/// charges what the whole handler charges, at the same points: the router
/// enters a part mid-function with `block | H2_PART_CUT`, which charges no
/// entry cost, and a cut-in at a loop header passes its poll and charges
/// the trip as a back-edge inside one handler does.
#[test]
fn a_split_handler_charges_as_the_whole_one() {
    use emit::c::Split;
    let Some(abi) = common::abi_header() else {
        common::skip("block-clock-split", "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    for (name, f) in [("block-clock-split-loop", memory_loop()), ("block-clock-split-entry", entry_loop())] {
        let owners = emit::plan(vec![f]);
        let o = &owners[0];
        let whole = common::build_split(&owners, &abi, &format!("{name}-whole"), emit::Codegen::BLOCK_CLOCK, Split::NEVER);
        let split = common::build_split(&owners, &abi, &format!("{name}-parts"), emit::Codegen::BLOCK_CLOCK, Split::EVERY_BLOCK);
        for slice in [None, Some(1), Some(2), Some(3), Some(5), Some(7), Some(64)] {
            let (w, s) = (drive(&whole, slice), drive(&split, slice));
            assert_eq!((&s.stops, s.charged, s.end), (&w.stops, w.charged, w.end), "{name}, slice {slice:?}");
            assert_eq!((&s.cpu, &s.mem), (&w.cpu, &w.mem), "{name}, slice {slice:?}");
            if let Some(slice) = slice {
                let (stops, total) = reference(&o.func, &o.loops.headers, Some(slice));
                assert_eq!((&s.stops, s.charged), (&stops, total), "{name}, slice {slice}");
            }
        }
    }
}

/// In the parts' C the entry cost is charged once, at the function entry's
/// selector, and never at a cut-in.
#[test]
fn only_the_function_entry_selector_charges_the_entry_in_parts() {
    use emit::c::Split;
    let owners = emit::plan(vec![memory_loop()]);
    let o = &owners[0];
    let shape = emit::c::OwnerShape {
        selectors: &o.selectors,
        loops: &o.loops,
        accounting: true,
        block_clock: true,
        functions: None,
    };
    let text = emit::c::function_with(&o.func, &shape, Split::EVERY_BLOCK);
    assert!(text.contains("H2_PART_GOTO("), "cut: {text}");
    let entry = format!("case {A:#010x}u: H2_CLOCK(1u); goto");
    assert_eq!(text.matches(&entry).count(), 1, "{text}");
    for line in text.lines().filter(|l| l.contains("case 0x8")) {
        assert!(!line.contains("H2_CLOCK"), "a cut-in charges nothing: {line}");
    }
    let header = o.loops.headers.iter().next().unwrap();
    assert_eq!(text.matches(&format!("P_{header:08x}:;\n    H2_CLOCK(5u);")).count(), 1, "{text}");
}
