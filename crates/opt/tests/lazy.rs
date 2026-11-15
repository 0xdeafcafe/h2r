//! The lazy-flag pass: which writes become `LazyFlags`, and that the
//! result runs like the original (the interpreter executes `LazyFlags`
//! where it stands, so this checks the rewrite, not a backend's laziness).

mod common;

use common::{equivalent, fixtures, function, optimized};
use ir::{Flag, IrFunction, Stmt};
use opt::Passes;

fn lazy_only() -> Passes {
    Passes::parse("lazy-flags").unwrap()
}

/// Per instruction: (eager flag writes, lazy flags).
fn shape(f: &IrFunction) -> Vec<(Vec<Flag>, Vec<Flag>)> {
    f.insns()
        .map(|i| {
            let mut eager = Vec::new();
            let mut lazy = Vec::new();
            for s in &i.stmts {
                match s {
                    Stmt::WriteFlag { flag, .. } => eager.push(*flag),
                    Stmt::LazyFlags(l) => lazy.extend(l.formula.flags.iter().map(|(f, _)| *f)),
                    _ => {}
                }
            }
            eager.sort();
            lazy.sort();
            (eager, lazy)
        })
        .collect()
}

use Flag::*;

#[test]
fn a_branch_keeps_what_it_reads_and_the_rest_waits_for_an_exit() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        let mut l = a.create_label();
        a.mov(ecx, 10)?;
        a.set_label(&mut l)?;
        a.add(eax, dword_ptr(esi))?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    });
    let (g, r) = optimized(&f, lazy_only());
    let s = shape(&g);
    // add: nothing reads its flags (dec rewrites five, the next add CF);
    // the load and the poll that observe them are exits.
    assert_eq!(s[1], (vec![], vec![Cf, Pf, Af, Zf, Sf, Of]), "{g}");
    // dec: jnz reads ZF in the same block, which the backend computes
    // from the formula, so ZF waits too.
    assert_eq!(s[2], (vec![], vec![Pf, Af, Zf, Sf, Of]), "{g}");
    assert_eq!(r.lazy_flags, 11);
}

#[test]
fn flags_read_in_the_block_stay_lazy_and_others_eager() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        a.cmp(eax, ebx)?;
        a.adc(ecx, 0)?;
        a.pushfd()?;
        a.ret()
    });
    let (g, _) = optimized(&f, lazy_only());
    let s = shape(&g);
    assert_eq!(s[0], (vec![], vec![Cf, Pf, Af, Zf, Sf, Of]), "adc reads CF from the formula: {g}");
    assert!(s[1].1.is_empty(), "pushfd reads everything adc writes: {g}");
}

#[test]
fn formulas_are_shared_between_instructions_of_one_shape() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        a.add(eax, ebx)?;
        a.mov(dword_ptr(edi), eax)?;
        a.add(ecx, edx)?;
        a.mov(dword_ptr(edi), ecx)?;
        a.xor(eax, eax)?;
        a.ret()
    });
    let (g, _) = optimized(&f, lazy_only());
    let formulas: Vec<_> = g
        .insns()
        .flat_map(|i| i.stmts.iter())
        .filter_map(|s| if let Stmt::LazyFlags(l) = s { Some(l.formula.clone()) } else { None })
        .collect();
    assert_eq!(formulas.len(), 3, "{g}");
    assert!(std::sync::Arc::ptr_eq(&formulas[0], &formulas[1]), "both adds share one formula");
    assert!(formulas[0].params.len() <= opt::lazy::MAX_ARGS);
}

#[test]
fn lazy_flags_run_like_eager_ones() {
    for (name, body) in fixtures() {
        let f = function(body);
        equivalent(name, &f, lazy_only(), 300);
        equivalent(name, &f, Passes::ALL, 300);
    }
}

#[test]
fn carry_alone_stays_eager() {
    use iced_x86::code_asm::*;
    let f = function(|a| {
        let mut l = a.create_label();
        a.set_label(&mut l)?;
        a.rol(eax, 3)?;
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    });
    let (g, _) = optimized(&f, lazy_only());
    let s = shape(&g);
    // rol's CF is one bit of its result; deferring it would only make dec
    // settle it.
    assert_eq!(s[0].1, vec![], "{g}");
    assert_eq!(s[1], (vec![], vec![Pf, Af, Zf, Sf, Of]), "{g}");
}
