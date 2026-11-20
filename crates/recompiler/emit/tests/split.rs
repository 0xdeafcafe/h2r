//! Handlers cut into parts (`emit::c::Split`, "Split handlers" in
//! `h2gen.h`), on synthetic code only. Every fixture of the differential
//! harness, cut at every block, must end as the interpreter does, with the
//! same CPU state and memory, and with access accounting charge exactly
//! what the whole handler charges. A loop whose blocks are in different
//! parts still polls at its header.

mod common;
mod differential_harness;

use std::sync::atomic::Ordering;

use differential_harness::*;
use emit::c::{OwnerShape, Split};
use emit::host::{CpuAbi, Ctx, ENTRY, HARNESS_EXIT, JUMP, RESUME, RETURN, TransferAbi};
use ir::interp::FlatMemory;

#[test]
fn split_handlers_match_the_interpreter_and_the_whole_handlers() {
    const NAME: &str = "split_handlers_match_the_interpreter_and_the_whole_handlers";
    let Some(header) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let funcs: Vec<(&str, ir::IrFunction)> = fixtures()
        .iter()
        .enumerate()
        .map(|(i, (n, body))| (*n, function(CODE_BASE + 0x1000 * i as u32, *body)))
        .collect();
    let owners = emit::plan(funcs.iter().map(|(_, f)| f.clone()).collect());
    let counting = emit::Codegen { accounting: true };
    let whole = common::build_split(&owners, &header, &format!("{NAME}-whole"), counting, Split::NEVER);
    let split = common::build_split(&owners, &header, &format!("{NAME}-split"), emit::Codegen::default(), Split::EVERY_BLOCK);
    let split_counting = common::build_split(&owners, &header, &format!("{NAME}-split-acc"), counting, Split::EVERY_BLOCK);

    let (mut cut, mut multi) = (0, 0);
    let mut failures = Vec::new();
    for (k, (name, f)) in funcs.iter().enumerate() {
        let o = owners.iter().find(|o| o.func.entry == f.entry).unwrap();
        let shape = OwnerShape { selectors: &o.selectors, loops: &o.loops, accounting: false, functions: None };
        let text = emit::c::function_with(&o.func, &shape, Split::EVERY_BLOCK);
        // Every handler of more than one block is cut; one block is not.
        let routed = text.contains("H2_PART_LENT();");
        assert_eq!(routed, o.func.blocks.len() > 1, "{name}: {text}");
        cut += text.contains("H2_PART_GOTO(") as usize;
        multi += routed as usize;
        let mut rng = Rng(0x5370_6c69_7448_616e ^ (k as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        for s in 0..STATES {
            let (mut cpu, mut mem) = random_state(&mut rng);
            let start = CpuAbi::from_interp(&cpu);
            let fresh = |mem: &FlatMemory| {
                let mut ctx = Ctx::new(mem.clone());
                ctx.tsc = cpu.tsc;
                ctx
            };
            let (mut a, mut actx) = (start, fresh(&mem));
            let (mut b, mut bctx) = (start, fresh(&mem));
            let (mut w, mut wctx) = (start, fresh(&mem));
            let want = run_interp(f, &mut cpu, &mut mem);
            let got = run_c(&split, f.entry, &mut a, &mut actx);
            let got_counting = run_c(&split_counting, f.entry, &mut b, &mut bctx);
            let got_whole = run_c(&whole, f.entry, &mut w, &mut wctx);
            let mut why = Vec::new();
            if got != want || got_counting != want {
                why.push(format!("end: split {got:x?}, counting {got_counting:x?}, interpreter {want:x?}"));
            }
            for (what, abi, ctx) in [("split", &a, &actx), ("counting", &b, &bctx)] {
                let c = ir::interp::Cpu { tsc: ctx.tsc, ..abi.to_interp() };
                if c != cpu {
                    why.push(format!("{what} cpu:\n  C      {c:x?}\n  interp {cpu:x?}"));
                }
                if ctx.mem.bytes != mem.bytes {
                    why.push(format!("{what}: memory differs"));
                }
            }
            // Each cut settles the count so far; the total is the whole
            // handler's.
            if got_whole != got_counting || bctx.charged != wctx.charged || bctx.counted != wctx.counted {
                why.push(format!(
                    "accounting: split charged {} ({} counted), whole {} ({} counted)",
                    bctx.charged, bctx.counted, wctx.charged, wctx.counted
                ));
            }
            if !why.is_empty() {
                failures.push(format!("{name} state {s}: {}", why.join("; ")));
            }
        }
    }
    eprintln!("{multi} of {} fixtures in parts, {cut} with an edge between parts", funcs.len());
    assert!(cut > 0, "no fixture has an edge between parts");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}

const A: u32 = 0x0040_0000;
const STACK: u32 = 0x0010_8000;

/// `sum()`: eax = 100 + 99 + ... + 1, with the flags `dec` leaves, in a
/// loop of two blocks once the add is its own block.
fn sum() -> ir::IrFunction {
    use iced_x86::code_asm::*;
    common::function(A, |a| {
        let mut l = a.create_label();
        let mut m = a.create_label();
        a.xor(eax, eax)?;
        a.mov(ecx, 100)?;
        a.set_label(&mut l)?;
        a.add(eax, ecx)?;
        a.jmp(m)?;
        a.set_label(&mut m)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    })
}

#[test]
fn a_loop_across_parts_polls_and_resumes_as_a_whole_one() {
    const NAME: &str = "a_loop_across_parts_polls_and_resumes_as_a_whole_one";
    let Some(abi) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let owners = emit::plan(vec![sum()]);
    let o = &owners[0];
    let shape = OwnerShape { selectors: &o.selectors, loops: &o.loops, accounting: false, functions: None };
    let text = emit::c::function_with(&o.func, &shape, Split::EVERY_BLOCK);
    assert!(text.contains("H2_PART_GOTO(") && text.contains("_p1("), "{text}");
    // The router alone has the handler's name; perf's marker attribution
    // reads the parts' markers as the router's.
    assert_eq!(text.matches("TransferAbi h2fn_00400000(CpuAbi *cpu").count(), 1, "{text}");
    let whole = emit::c::function_with(&o.func, &shape, Split::NEVER);
    assert!(!whole.contains("H2_PART"), "{whole}");

    let header = *o.loops.headers.iter().next().unwrap();
    for (name, split) in [("whole", Split::NEVER), ("split", Split::EVERY_BLOCK)] {
        let g = common::build_split(&owners, &abi, &format!("split-sum-{name}"), emit::Codegen::default(), split);
        let run = |preempt: bool| {
            let mut mem = FlatMemory::new(STACK - 0x1000, 0x1000);
            mem.load(STACK - 4, &HARNESS_EXIT.to_le_bytes());
            let mut cpu = CpuAbi::default();
            cpu.gpr[4] = STACK - 4;
            let mut ctx = Ctx::new(mem);
            ctx.preempt.store(preempt as u32, Ordering::Relaxed);
            let first = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 1);
            if preempt {
                // The first arrival at the header leaves for the runner.
                assert_eq!((first.0.kind, first.0.route, first.0.target_pc), (JUMP, RESUME, header), "{name}");
            }
            let (t, calls) = g.run(&mut cpu, &mut ctx, first.0, 1000);
            assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT), "{name}: {t:x?}");
            (cpu, first.1 + calls)
        };
        let (quiet, quiet_calls) = run(false);
        let (preempted, preempted_calls) = run(true);
        assert_eq!(quiet.gpr[0], 5050, "{name}");
        assert_eq!(quiet_calls, 1, "{name}: the parts run inside one handler call");
        // Each resume makes one trip and leaves again at the header, cut or
        // not: 1 + 100 calls.
        assert_eq!(preempted_calls, 101, "{name}");
        assert_eq!(preempted, quiet, "{name}: registers, flags and the defined mask agree");
    }
}
