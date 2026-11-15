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
