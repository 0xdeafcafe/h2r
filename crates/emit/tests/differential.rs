//! Differential test: synthetic functions run through the IR interpreter
//! and through generated, compiled C, from the same random states, must
//! end the same way with the same CPU state and memory.
//!
//! The fixtures are small x86 functions (assembled here, no game data)
//! covering what the lowering emits: ALU and flag formulas at every width,
//! shifts and rotates by immediates and by CL (including zero), multiply
//! and divide (and `#DE`), bit ops, atomics, string ops with REP (and a
//! fault mid-iteration), stack and EFLAGS forms, branches and loops, SSE
//! moves (and the alignment `#GP`), FP control intrinsics, `rdtsc` and
//! `cpuid`, traps, and transfers that leave the function. Each runs from
//! many random states whose pointers land in, near and outside mapped
//! memory, so loads and stores fault some of the time. Needs `h2abi.h`
//! (`H2ABI_HEADER` or the workspace copy) and clang. The fixtures and
//! runners are in `differential_harness`, which `crates/clif` shares.

mod common;
mod differential_harness;

use std::collections::{BTreeMap, BTreeSet};

use differential_harness::*;
use emit::host::{CpuAbi, Ctx};
use ir::IrFunction;

/// The flat segment model's instructions: `mov` from CS, `sgdt` and a far `jmp` to the
/// code selector, which is a plain jump. Alone in its slice so its base is
/// `CODE_BASE`, the far jump's target the 7 bytes the jump takes.
#[test]
fn generated_c_matches_the_interpreter_for_cs_sgdt_and_far_jmp() {
    let fixtures: [(&'static str, Asm); 1] = [("flat-segments", |a| {
        use iced_x86::code_asm::*;
        let [t0, t1, t2, t3] = (CODE_BASE + 7).to_le_bytes();
        a.db(&[0xEA, t0, t1, t2, t3, 0x08, 0x00])?;
        a.mov(ax, cs)?;
        a.mov(edx, cs)?; // zero-extended
        a.mov(word_ptr(ebx), cs)?;
        a.sgdt(ptr(edi))?;
        a.ret()
    })];
    let Some(ends) = differential("generated_c_matches_the_interpreter_for_cs_sgdt_and_far_jmp", &fixtures) else {
        return;
    };
    assert_eq!(ends["flat-segments"].get("Returned"), Some(&STATES), "{:?}", ends["flat-segments"]);
}

#[test]
fn generated_c_matches_the_interpreter() {
    let ends = differential("generated_c_matches_the_interpreter", &fixtures());
    let Some(ends) = ends else { return };
    let all: BTreeSet<&String> = ends.values().flat_map(|m| m.keys()).collect();
    for k in ["Returned", "Left", "Memory", "Fault", "Stop"] {
        assert!(all.contains(&k.to_string()), "no fixture ended with {k}: {ends:?}");
    }
    // The deterministic fixtures end one way from every state.
    for (name, kind) in [("fp-control", "Returned"), ("x87-mf-at-fwait", "Fault")] {
        assert_eq!(ends[name].get(kind), Some(&STATES), "{name}: {:?}", ends[name]);
    }
    for kind in ["Returned", "Fault"] {
        assert!(ends["mxcsr-load"].contains_key(kind), "mxcsr-load: {:?}", ends["mxcsr-load"]);
    }
}

/// Run every fixture through both backends from the same random states;
/// panics on any mismatch. `None` if skipped.
fn differential(
    name: &str,
    fixtures: &[(&'static str, Asm)],
) -> Option<BTreeMap<&'static str, BTreeMap<String, usize>>> {
    let Some(header) = common::abi_header() else {
        common::skip(name, "needs h2abi.h (H2ABI_HEADER)");
        return None;
    };
    let funcs: Vec<(&str, IrFunction)> = fixtures
        .iter()
        .enumerate()
        .map(|(i, (n, body))| (*n, function(CODE_BASE + 0x1000 * i as u32, *body)))
        .collect();
    let owners = emit::plan(funcs.iter().map(|(_, f)| f.clone()).collect());
    let g = common::build(&owners, &header, name);
    // The same module counting its own accesses (h2gen.h, "Access
    // accounting"): it must end the same way and charge the same total.
    let counting = common::build_with(&owners, &header, &format!("{name}-acc"), emit::Codegen { accounting: true });

    let mut ends: BTreeMap<&str, BTreeMap<String, usize>> = BTreeMap::new();
    let mut failures = Vec::new();
    for (k, (name, f)) in funcs.iter().enumerate() {
        let mut rng = Rng(0x4832_7265_636f_6d70 ^ (k as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        for s in 0..STATES {
            let (mut cpu, mut mem) = random_state(&mut rng);
            let mut abi = CpuAbi::from_interp(&cpu);
            let mut ctx = Ctx::new(mem.clone());
            // The C side's counter lives in the host context.
            ctx.tsc = cpu.tsc;
            let (mut acc_abi, mut acc_ctx) = (abi, Ctx::new(mem.clone()));
            acc_ctx.tsc = cpu.tsc;
            let want = run_interp(f, &mut cpu, &mut mem);
            let got = run_c(&g, f.entry, &mut abi, &mut ctx);
            let acc_got = run_c(&counting, f.entry, &mut acc_abi, &mut acc_ctx);
            let kind = format!("{want:?}").split([' ', '(', '{']).next().unwrap().to_owned();
            *ends.entry(name).or_default().entry(kind).or_default() += 1;
            let mut why = Vec::new();
            if got != want {
                why.push(format!("end: C {got:x?}, interpreter {want:x?}"));
            }
            let c = ir::interp::Cpu { tsc: ctx.tsc, ..abi.to_interp() };
            if c != cpu {
                why.push(format!("cpu:\n  C    {c:x?}\n  interp {cpu:x?}"));
            }
            if let Some(i) = ctx.mem.bytes.iter().zip(&mem.bytes).position(|(a, b)| a != b) {
                why.push(format!("memory differs first at {:#x}", MEM_BASE + i as u32));
            }
            // Accounting: the same end, state and memory as without it, and
            // the same accesses charged, each counted one exactly once.
            let acc_cpu = ir::interp::Cpu { tsc: acc_ctx.tsc, ..acc_abi.to_interp() };
            if acc_got != got || acc_cpu != c || acc_ctx.mem.bytes != ctx.mem.bytes {
                why.push(format!("accounting: end {acc_got:x?}, cpu {acc_cpu:x?}"));
            }
            if ctx.counted != 0 || acc_ctx.charged != ctx.charged {
                why.push(format!("accounting charged {}, unflagged {} ({} counted)", acc_ctx.charged, ctx.charged, ctx.counted));
            }
            if acc_ctx.counted + acc_ctx.atomics.len() as u64 != acc_ctx.charged {
                why.push(format!(
                    "accounting: {} counted + {} atomics, {} charged",
                    acc_ctx.counted,
                    acc_ctx.atomics.len(),
                    acc_ctx.charged
                ));
            }
            // An #MF callback sees the raiser's pointers, as returned.
            if let (End::Fault { code: 14, .. }, Some(seen)) = (&got, ctx.fp_error_x87.last()) {
                let ptrs = |x: &emit::host::X87Abi| {
                    (x.instruction_pointer, x.opcode, x.instruction_selector, x.data_pointer, x.data_selector)
                };
                if ptrs(seen) != ptrs(&abi.x87) {
                    why.push(format!("#MF callback saw {:x?}, returned {:x?}", ptrs(seen), ptrs(&abi.x87)));
                }
            }
            if !why.is_empty() {
                failures.push(format!("{name} state {s}: {}", why.join("; ")));
            }
        }
    }
    // Each fixture should exercise more than one outcome where it can.
    eprintln!("outcomes: {ends:#?}");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    Some(ends)
}
