//! Loop safe points through generated C (synthetic code, no game data):
//! a loop leaves with `JUMP`/`RESUME` to its header only while the
//! preempt word is set, and the loop resumed from there computes exactly
//! what it computes uninterrupted.

mod common;

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use emit::host::{CpuAbi, Ctx, ENTRY, HARNESS_EXIT, JUMP, RESUME, RETURN, TransferAbi};
use common::function;
use iced_x86::code_asm::*;
use ir::IrFunction;
use ir::interp::FlatMemory;

const A: u32 = 0x0040_0000;
const STACK: u32 = 0x0010_8000;
const FLAG: u32 = STACK - 0x800;

/// `spin(flag)`: wait for `*flag != 0`, then return 1.
fn spin() -> (IrFunction, u32) {
    let f = function(A, |a| {
        let mut l = a.create_label();
        a.mov(eax, dword_ptr(esp + 4))?;
        a.set_label(&mut l)?;
        a.cmp(dword_ptr(eax), 0)?;
        a.je(l)?;
        a.mov(eax, 1)?;
        a.ret()
    });
    // The header is the block after the first one.
    let header = *f.blocks.keys().nth(1).unwrap();
    (f, header)
}

/// `sum()`: eax = 100 + 99 + ... + 1, with the flags `dec` leaves.
fn sum() -> IrFunction {
    function(A, |a| {
        let mut l = a.create_label();
        a.xor(eax, eax)?;
        a.mov(ecx, 100)?;
        a.set_label(&mut l)?;
        a.add(eax, ecx)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    })
}

fn state(arg: u32) -> (CpuAbi, Ctx) {
    let mut mem = FlatMemory::new(STACK - 0x1000, 0x1000);
    mem.load(STACK - 8, &HARNESS_EXIT.to_le_bytes());
    mem.load(STACK - 4, &arg.to_le_bytes());
    let mut cpu = CpuAbi::default();
    cpu.gpr[4] = STACK - 8;
    (cpu, Ctx::new(mem))
}

#[test]
fn plan_registers_each_loop_header_as_a_resume_of_its_owner() {
    let (f, header) = spin();
    let owners = emit::plan(vec![f]);
    let o = &owners[0];
    assert_eq!(o.loops.headers, BTreeSet::from([header]));
    assert_eq!(o.resumes, vec![header]);
    assert!(o.selectors.contains(&header));
    let text = emit::c::function(&o.func, &emit::c::OwnerShape { selectors: &o.selectors, loops: &o.loops, accounting: false });
    assert_eq!(text.matches("H2_LOOP_POLL(").count(), 1, "{text}");
    assert_eq!(text.matches("H2_PREEMPT_LOCALS();").count(), 1);
    // A function without loops polls nothing.
    let straight = function(A, |a| a.ret());
    let owners = emit::plan(vec![straight]);
    let o = &owners[0];
    let text = emit::c::function(&o.func, &emit::c::OwnerShape { selectors: &o.selectors, loops: &o.loops, accounting: false });
    assert!(!text.contains("H2_PREEMPT") && !text.contains("H2_LOOP_POLL"), "{text}");
}

#[test]
fn a_spin_loop_leaves_at_its_back_edge_only_while_preemption_is_requested() {
    const NAME: &str = "a_spin_loop_leaves_at_its_back_edge_only_while_preemption_is_requested";
    let Some(abi) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let (f, header) = spin();
    let g = common::build(&emit::plan(vec![f]), &abi, "preempt-spin");
    let (mut cpu, mut ctx) = state(FLAG);
    ctx.preempt.store(1, Ordering::Relaxed);
    // Requested from the start: every trip round the loop is one handler
    // call that ends in JUMP/RESUME to the header.
    let (t, calls) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 3);
    assert_eq!((t.kind, t.route, t.target_pc), (JUMP, RESUME, header), "{t:x?}");
    assert_eq!((calls, cpu.eip), (3, header));
    assert_eq!(cpu.gpr[0], FLAG, "state at the header is canonical");
    // The other thread sets the flag and the request goes away: resuming
    // the transfer finishes the loop.
    ctx.mem.load(FLAG, &1u32.to_le_bytes());
    ctx.preempt.store(0, Ordering::Relaxed);
    let (t, calls) = g.run(&mut cpu, &mut ctx, t, 3);
    assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT), "{t:x?}");
    assert_eq!((calls, cpu.gpr[0]), (1, 1));
}

#[test]
fn a_loop_resumed_at_every_back_edge_computes_what_it_computes_uninterrupted() {
    const NAME: &str = "a_loop_resumed_at_every_back_edge_computes_what_it_computes_uninterrupted";
    let Some(abi) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let g = common::build(&emit::plan(vec![sum()]), &abi, "preempt-sum");
    let run = |preempt: bool| {
        let (mut cpu, mut ctx) = state(0);
        ctx.preempt.store(preempt as u32, Ordering::Relaxed);
        let (t, calls) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 1000);
        assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT), "{t:x?}");
        (cpu, calls)
    };
    let (quiet, quiet_calls) = run(false);
    let (preempted, preempted_calls) = run(true);
    assert_eq!(quiet.gpr[0], 5050);
    assert_eq!(quiet_calls, 1);
    // The first arrival at the header leaves, then each resume makes
    // exactly one trip before leaving again: 1 + 100 calls.
    assert_eq!(preempted_calls, 101);
    assert_eq!(preempted, quiet, "registers, flags and the defined mask agree");
}

/// Cost of the poll in a hot loop: the same function built with and without
/// its back-edge checks. `cargo test -p emit --release --test preempt --
/// --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn bench_back_edge_poll() {
    let Some(abi) = common::abi_header() else { return };
    let register = |n: u32| {
        function(A, move |a| {
            let mut l = a.create_label();
            a.xor(eax, eax)?;
            a.mov(ecx, n)?;
            a.set_label(&mut l)?;
            a.add(eax, ecx)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        })
    };
    let memory = |n: u32| {
        function(A, move |a| {
            let mut l = a.create_label();
            a.xor(eax, eax)?;
            a.mov(ecx, n)?;
            a.mov(esi, FLAG)?;
            a.set_label(&mut l)?;
            a.add(eax, dword_ptr(esi))?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        })
    };
    for (name, f, n) in
        [("register", register(200_000_000), 200_000_000u32), ("memory", memory(20_000_000), 20_000_000)]
    {
        let built: Vec<_> = [false, true]
            .into_iter()
            .map(|poll| {
                let mut owners = emit::plan(vec![f.clone()]);
                if !poll {
                    owners[0].loops = Default::default();
                }
                common::build(&owners, &abi, &format!("preempt-bench-{name}-{poll}"))
            })
            .collect();
        // Interleaved trials, best of each, so background load hits both.
        let mut times = [f64::MAX; 2];
        for _ in 0..15 {
            for (g, best) in built.iter().zip(&mut times) {
                let (mut cpu, mut ctx) = state(0);
                let t0 = std::time::Instant::now();
                let (t, _) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 4);
                *best = best.min(t0.elapsed().as_secs_f64() / n as f64 * 1e9);
                assert_eq!(t.target_pc, HARNESS_EXIT);
            }
        }
        println!(
            "{name:8} loop: {:.3} ns/iter without the poll, {:.3} ns/iter with it ({:+.1}%)",
            times[0],
            times[1],
            (times[1] / times[0] - 1.0) * 100.0
        );
    }
}
