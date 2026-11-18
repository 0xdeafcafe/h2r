//! The Cranelift tier against the IR interpreter and the C tier: every
//! synthetic fixture of emit's differential test (shared source, no game
//! data) from the same random states, ending the same way with the same
//! CPU state and memory in all three. Needs `h2abi.h` and clang, for the
//! C tier and for linking the Cranelift module.

#[path = "../../emit/tests/common/mod.rs"]
mod common;
#[path = "../../emit/tests/differential_harness/mod.rs"]
mod differential_harness;

use std::collections::BTreeMap;
use std::path::Path;

use differential_harness::*;
use emit::host::{BRIDGE, CpuAbi, Ctx, Generated};
use ir::IrFunction;

fn build_clif(owners: &[emit::Owner], header: &Path, name: &str) -> Generated {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let dylib = dir.join(format!("lib{name}.dylib"));
    // Cranelift's IR verifier on: invalid IR (an extend to its own width)
    // otherwise compiles to an undefined instruction and dies at run time.
    // SAFETY: this binary's only test; nothing reads the environment
    // concurrently.
    unsafe { std::env::set_var("H2_CLIF_VERIFY", "1") };
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let r = clif::build_dylib(owners, &dir, header, &[("bridge.c", BRIDGE)], &dylib, jobs).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("clif: {r:?}");
    // SAFETY: linked just now from this crate's objects for this host.
    unsafe { Generated::load(&dylib) }.unwrap()
}

/// Run every fixture through the interpreter, C and Cranelift; panics on
/// any mismatch. `None` if skipped.
fn three_way(name: &str, fixtures: &[(&'static str, Asm)]) -> Option<BTreeMap<&'static str, BTreeMap<String, usize>>> {
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
    let c = common::build(&owners, &header, &format!("{name}-c"));
    let cl = build_clif(&owners, &header, &format!("{name}-clif"));

    let mut ends: BTreeMap<&str, BTreeMap<String, usize>> = BTreeMap::new();
    let mut failures = Vec::new();
    for (k, (fname, f)) in funcs.iter().enumerate() {
        let mut rng = Rng(0x4832_7265_636f_6d70 ^ (k as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        for s in 0..STATES {
            let (mut cpu, mut mem) = random_state(&mut rng);
            let start = CpuAbi::from_interp(&cpu);
            let (mut c_abi, mut c_ctx) = (start, Ctx::new(mem.clone()));
            c_ctx.tsc = cpu.tsc;
            let (mut k_abi, mut k_ctx) = (start, Ctx::new(mem.clone()));
            k_ctx.tsc = cpu.tsc;
            let want = run_interp(f, &mut cpu, &mut mem);
            let c_got = run_c(&c, f.entry, &mut c_abi, &mut c_ctx);
            let got = run_c(&cl, f.entry, &mut k_abi, &mut k_ctx);
            let kind = format!("{want:?}").split([' ', '(', '{']).next().unwrap().to_owned();
            *ends.entry(fname).or_default().entry(kind).or_default() += 1;
            let mut why = Vec::new();
            if got != want {
                why.push(format!("end: clif {got:x?}, interpreter {want:x?}"));
            }
            if got != c_got {
                why.push(format!("end: clif {got:x?}, C {c_got:x?}"));
            }
            let kc = ir::interp::Cpu { tsc: k_ctx.tsc, ..k_abi.to_interp() };
            if kc != cpu {
                why.push(format!("cpu:\n  clif   {kc:x?}\n  interp {cpu:x?}"));
            }
            let cc = ir::interp::Cpu { tsc: c_ctx.tsc, ..c_abi.to_interp() };
            if kc != cc {
                why.push(format!("cpu:\n  clif {kc:x?}\n  C    {cc:x?}"));
            }
            if let Some(i) = k_ctx.mem.bytes.iter().zip(&mem.bytes).position(|(a, b)| a != b) {
                why.push(format!("memory differs from the interpreter first at {:#x}", MEM_BASE + i as u32));
            }
            if k_ctx.mem.bytes != c_ctx.mem.bytes {
                why.push("memory differs from C".into());
            }
            // Charged the same accesses as the C tier without accounting.
            if k_ctx.charged != c_ctx.charged {
                why.push(format!("charged {} accesses, C {}", k_ctx.charged, c_ctx.charged));
            }
            if !why.is_empty() {
                failures.push(format!("{fname} state {s}: {}", why.join("; ")));
            }
        }
    }
    eprintln!("outcomes: {ends:#?}");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    Some(ends)
}

#[test]
fn cranelift_matches_the_interpreter_and_c() {
    let Some(ends) = three_way("clif_differential", &fixtures()) else { return };
    let all: std::collections::BTreeSet<&String> = ends.values().flat_map(|m| m.keys()).collect();
    for k in ["Returned", "Left", "Memory", "Fault", "Stop"] {
        assert!(all.contains(&k.to_string()), "no fixture ended with {k}: {ends:?}");
    }
}
