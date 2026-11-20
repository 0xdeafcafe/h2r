//! The dead-flag pass: what it removes, what it must keep, and that the
//! result runs exactly like the original everywhere state is observable.

mod common;

use common::{equivalent, fixtures, function, optimized};
use ir::{Flag, IrFunction, Stmt};
use opt::Passes;

/// Just this pass.
fn dead_flags() -> Passes {
    Passes::parse("dead-flags").unwrap()
}

/// The flags each instruction (by PC order) still writes.
fn writes(f: &IrFunction) -> Vec<Vec<Flag>> {
    f.insns()
        .map(|i| {
            i.stmts
                .iter()
                .filter_map(|s| match s {
                    Stmt::WriteFlag { flag, .. } => Some(*flag),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

use Flag::*;

#[test]
fn flags_overwritten_before_anything_observes_them_go() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        a.add(eax, ebx)?;
        a.sub(eax, ecx)?;
        a.ret()
    });
    let (g, r) = optimized(&f, dead_flags());
    let w = writes(&g);
    assert!(w[0].is_empty(), "add's flags are all rewritten by sub: {w:?}");
    assert_eq!(w[1].len(), 6, "sub's reach the return");
    assert_eq!(r.dead_flags, 6);
    assert!(r.dead_stmts > 0, "the formulas went with them");
}

#[test]
fn a_possible_fault_observes_every_flag() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        a.add(eax, ebx)?;
        a.mov(ecx, dword_ptr(esi))?;
        a.sub(eax, ecx)?;
        a.ret()
    });
    let (g, _) = optimized(&f, dead_flags());
    assert_eq!(writes(&g)[0].len(), 6, "the load may fault with add's flags in EFLAGS");
}

#[test]
fn a_partial_rewrite_keeps_the_flags_it_leaves() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        a.add(eax, ebx)?;
        a.inc(eax)?;
        a.ret()
    });
    let (g, _) = optimized(&f, dead_flags());
    assert_eq!(writes(&g)[0], vec![Cf], "inc leaves CF");
}

#[test]
fn a_polled_loop_header_observes_every_flag() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        let mut l = a.create_label();
        a.mov(ecx, 10)?;
        a.set_label(&mut l)?;
        a.add(eax, ecx)?;
        a.add(eax, ecx)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    });
    let (g, _) = optimized(&f, dead_flags());
    let w = writes(&g);
    // mov, add, add, dec, jnz, ret.
    assert!(w[1].is_empty(), "the first add is rewritten by the second: {w:?}");
    assert_eq!(w[2], vec![Cf], "the second add's CF survives dec to the poll");
    assert_eq!(w[3].len(), 5);
}

#[test]
fn reads_keep_what_they_read() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        a.cmp(eax, ebx)?;
        a.setb(cl)?;
        a.add(eax, ebx)?;
        a.ret()
    });
    let (g, _) = optimized(&f, dead_flags());
    assert_eq!(writes(&g)[0], vec![Cf], "setb reads only CF of the cmp");
}

#[test]
fn no_passes_change_nothing() {
    for (name, body) in fixtures() {
        let f = function(body);
        let (g, r) = optimized(&f, Passes::NONE);
        assert_eq!(f, g, "{name}");
        assert_eq!((r.dead_flags, r.dead_stmts), (0, 0));
    }
}

#[test]
fn dead_flag_elimination_is_unobservable() {
    for (name, body) in fixtures() {
        equivalent(name, &function(body), dead_flags(), 300);
    }
}

#[test]
fn a_spec_names_its_passes_and_parses_back() {
    for name in Passes::names() {
        let p = Passes::parse(name).unwrap();
        assert_eq!(p.spec(), name, "a pass missing from Passes::spec");
    }
    for spec in ["all", "none", "dead-flags", "-dead-flags"] {
        let p = Passes::parse(spec).unwrap();
        assert_eq!(Passes::parse(&p.spec()).unwrap(), p, "{spec}");
    }
    assert_eq!(Passes::NONE.spec(), "none");
}

// ---- imprecise memory faults (docs/runtime/block-clock-imprecise-faults.md)

/// The precise passes plus `imprecise-faults`.
fn imprecise() -> Passes {
    Passes::parse("all,imprecise-faults").unwrap()
}

#[test]
fn imprecise_faults_are_named_not_part_of_all() {
    const { assert!(!Passes::ALL.imprecise_faults) };
    assert!(!Passes::parse("all").unwrap().imprecise_faults);
    let p = imprecise();
    assert!(p.dead_flags && p.lazy_flags && p.imprecise_faults);
    assert_eq!(p.spec(), "dead-flags,lazy-flags,imprecise-faults");
    assert_eq!(Passes::parse(&p.spec()).unwrap(), p);
    assert_eq!(Passes::parse("imprecise-faults,all").unwrap(), p, "all keeps it");
    assert_eq!(Passes::parse("all,imprecise-faults,-imprecise-faults").unwrap(), Passes::ALL);
}

#[test]
fn with_imprecise_faults_a_load_observes_no_flag() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        a.add(eax, ebx)?;
        a.mov(ecx, dword_ptr(esi))?;
        a.mov(dword_ptr(edi), ecx)?;
        a.sub(eax, ecx)?;
        a.ret()
    });
    let mut passes = dead_flags();
    passes.imprecise_faults = true;
    let (g, r) = optimized(&f, passes);
    assert!(writes(&g)[0].is_empty(), "sub rewrites add's flags; the accesses between see none");
    assert!(r.imprecise_faults, "the report says the module must declare it");
    let (g, r) = optimized(&f, dead_flags());
    assert_eq!(writes(&g)[0].len(), 6, "precise: the load may fault with add's flags");
    assert!(!r.imprecise_faults);
}

#[test]
fn with_imprecise_faults_other_exits_stay_precise() {
    use iced_x86::code_asm::*;
    let mut passes = dead_flags();
    passes.imprecise_faults = true;
    // A divide may fault (#DE): add's flags reach it.
    let f = function(|a| {
        a.add(eax, ebx)?;
        a.div(ecx)?;
        a.sub(eax, ecx)?;
        a.ret()
    });
    let (g, _) = optimized(&f, passes);
    assert_eq!(writes(&g)[0].len(), 6, "a FaultIf still observes every flag");
    // An atomic hands the flags to the runtime.
    let f = function(|a| {
        a.add(eax, ebx)?;
        a.lock().inc(dword_ptr(esi))?;
        a.sub(eax, ecx)?;
        a.ret()
    });
    let (g, _) = optimized(&f, passes);
    assert_eq!(writes(&g)[0].len(), 6, "an atomic still observes every flag");
}

/// From many random states (with random preemption at loop headers), the
/// optimised IR ends the same way, with the same memory and registers, and
/// the same flags unless the end is a memory fault: there only EFLAGS and
/// its defined mask may differ.
#[test]
fn imprecise_faults_change_only_a_memory_faults_flags() {
    use common::{End, Rng, run, state};
    let (mut faults, mut removed_precise, mut removed_imprecise) = (0, 0, 0);
    for (name, body) in fixtures() {
        let f = function(body);
        let (g, r) = optimized(&f, imprecise());
        removed_imprecise += r.dead_flags;
        removed_precise += optimized(&f, Passes::ALL).1.dead_flags;
        let mut rng = Rng(0x696d_7072_6563_6973 ^ name.len() as u64);
        for s in 0..300 {
            let (cpu0, mem0) = state(&mut rng);
            let seed = rng.next() as u64 | 1;
            let mut ends = Vec::new();
            for func in [&f, &g] {
                let (mut cpu, mut mem) = (cpu0.clone(), mem0.clone());
                let mut prng = Rng(seed);
                let end = run(func, &mut cpu, &mut mem, &mut || prng.next().is_multiple_of(7));
                ends.push((end, cpu, mem.bytes));
            }
            let (a, b) = (&ends[0], &ends[1]);
            assert_eq!(a.0, b.0, "{name} state {s}: end differs");
            assert!(a.2 == b.2, "{name} state {s}: memory differs");
            if matches!(&a.0, End::Exit(e) if e.starts_with("memory")) {
                faults += 1;
                let strip = |c: &ir::interp::Cpu| {
                    let mut c = c.clone();
                    c.eflags = 0;
                    c.flags_valid = 0;
                    c
                };
                assert_eq!(strip(&a.1), strip(&b.1), "{name} state {s}: registers differ at a memory fault\n{g}");
            } else {
                assert_eq!(a.1, b.1, "{name} state {s}: cpu differs after {:?}\n{g}", a.0);
            }
        }
    }
    assert!(faults > 0, "the fixtures fault somewhere");
    assert!(removed_imprecise > removed_precise, "{removed_imprecise} <= {removed_precise}");
}
