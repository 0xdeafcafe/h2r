//! What a handler's way out calls, as the C compiler sees it: the emitted
//! module preprocessed with the build's own flags (synthetic code, no game
//! data). The shared epilogue settles with the runtime in one
//! `h2abi_v1_exit`, after the spill; `H2_EXIT_CALL=0` is the pair it
//! replaced, a charge before the spill and a release after it. With direct
//! calls a handler may run on a view its direct caller lent it
//! (`H2_DIRECT_VIEW`): it then makes the call only to charge its own count,
//! and acquires nothing on entry.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::function;
use iced_x86::code_asm::*;

const A: u32 = 0x0040_0000;

const B: u32 = 0x0040_0100;

/// One load and one store, so the handler counts accesses and can fault.
fn source(accounting: bool) -> Option<(PathBuf, PathBuf)> {
    let f = function(A, |a| {
        a.mov(eax, dword_ptr(ecx))?;
        a.mov(dword_ptr(edx), eax)?;
        a.ret()
    });
    write(vec![f], &format!("exits-{accounting}"), accounting)
}

fn write(funcs: Vec<ir::IrFunction>, name: &str, accounting: bool) -> Option<(PathBuf, PathBuf)> {
    let abi = common::abi_header()?;
    let owners = emit::plan(funcs);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let src = dir.join("src");
    let _ = std::fs::remove_dir_all(&src);
    emit::write_module_with(
        &owners,
        &src,
        emit::Codegen { accounting },
    )
    .unwrap();
    emit::compile::install_header(&src, &abi).unwrap();
    let c = std::fs::read_dir(&src)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("shard_")
        })
        .unwrap_or_else(|| panic!("no function file in {}", src.display()));
    Some((src, c))
}

/// The handler as the compiler sees it, whitespace collapsed.
fn preprocess(src: &Path, c: &Path, defines: &[&str]) -> String {
    let text = preprocess_text(src, c, defines);
    // The handler is the last function in the file, after the headers.
    text[text.rfind("TransferAbi h2fn_").expect("handler")..].to_string()
}

/// The whole file as the compiler sees it, whitespace collapsed.
fn preprocess_text(src: &Path, c: &Path, defines: &[&str]) -> String {
    let cc = emit::compile::compiler().unwrap();
    let mut cmd = Command::new(cc);
    cmd.args(["-E", "-P"])
        .args(emit::compile::cflags().unwrap())
        // The default exits, whatever the environment asks for; a test of
        // H2_VIEW_CHARGE defines it again below.
        .arg("-UH2_VIEW_CHARGE")
        .arg("-I")
        .arg(src);
    for d in defines {
        cmd.arg(format!("-D{d}"));
    }
    let out = cmd.arg(c).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// From the epilogue's label to its return.
fn epilogue(body: &str) -> &str {
    let start = body.find("h2_exit:").expect("epilogue");
    let end = start + body[start..].find("return h2_transfer").expect("return");
    &body[start..end]
}

fn calls(body: &str, name: &str) -> usize {
    body.matches(&format!("{name}(ctx")).count()
}

#[test]
fn a_handler_leaves_through_one_exit_call() {
    for accounting in [false, true] {
        let Some((src, c)) = source(accounting) else {
            common::skip("exits", "needs h2abi.h (H2ABI_HEADER)");
            return;
        };
        for (fastmem, view) in [(false, false), (true, false), (true, true)] {
            let fm = format!("H2_FASTMEM={}", u8::from(fastmem));
            let lend = format!("H2_DIRECT_VIEW={}", u8::from(view));
            let new = preprocess(&src, &c, &[&fm, &lend]);
            let old = preprocess(&src, &c, &[&fm, "H2_EXIT_CALL=0", "H2_DIRECT_VIEW=0"]);
            let (n, o) = (epilogue(&new), epilogue(&old));
            let case = format!("accounting {accounting}, fastmem {fastmem}, lent views {view}");
            // Entry: one acquire with the view, either way.
            assert_eq!(
                calls(&new, "h2abi_v1_fast_acquire"),
                usize::from(fastmem),
                "{case}: {new}"
            );
            assert_eq!(
                calls(&old, "h2abi_v1_fast_acquire"),
                usize::from(fastmem),
                "{case}"
            );
            // Exit: one call with the view; the charge alone without it.
            assert_eq!(
                calls(n, "h2abi_v1_exit"),
                usize::from(fastmem),
                "{case}: {n}"
            );
            assert_eq!(calls(n, "h2abi_v1_fast_release"), 0, "{case}: {n}");
            assert_eq!(
                calls(n, "h2abi_v1_charge"),
                usize::from(accounting && !fastmem),
                "{case}: {n}"
            );
            assert_eq!(calls(o, "h2abi_v1_exit"), 0, "{case}: {o}");
            assert_eq!(
                calls(o, "h2abi_v1_fast_release"),
                usize::from(fastmem),
                "{case}: {o}"
            );
            assert_eq!(
                calls(o, "h2abi_v1_charge"),
                usize::from(accounting),
                "{case}: {o}"
            );
            // The exit call is the last thing before the transfer, after the
            // spill and the EIP store, and passes the handler's count.
            // A handler on a lent view makes it only to charge its count.
            if fastmem {
                let count = if accounting { "h2acc" } else { "0u" };
                let mut call = format!("h2abi_v1_exit(ctx, {count});");
                if view {
                    call = format!("if (!(h2_lent_ != 0) || {count}) {call} }} while (0);");
                }
                let at = n.find(&call).unwrap_or_else(|| panic!("{case}: {n}"));
                assert!(n.find("cpu->eip = x_eip;").unwrap() < at, "{case}: {n}");
                // Only the (off) perf flush follows it.
                assert_eq!(n[at + call.len()..].trim(), "((void)0);", "{case}: {n}");
            }
            // Nothing else in the handler changed, but for the entry that
            // takes a lent view before it acquires one.
            if view {
                assert_eq!(calls(&new, "h2_view_borrow"), 1, "{case}: {new}");
                assert!(
                    new.contains("h2_lent_ ? h2_lent_ : h2abi_v1_fast_acquire(ctx)"),
                    "{case}: {new}"
                );
            } else {
                let strip = |s: &str| s.replace(n, "").replace(o, "");
                assert_eq!(strip(&new), strip(&old), "{case}");
                assert_eq!(calls(&new, "h2_view_borrow"), 0, "{case}");
            }
            // Snapshot of the new epilogue's shape, the spill elided.
            let shape = |e: &str| {
                let calls: Vec<&str> = e
                    .split(';')
                    .map(str::trim)
                    .filter(|s| {
                        s.contains("h2abi_v1_")
                            || s.contains("h2_gate_drop")
                            || s.contains("cpu->eip")
                    })
                    .collect();
                calls.join("; ")
            };
            let expect = match (accounting, fastmem, view) {
                (false, false, _) => "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip",
                (false, true, false) => {
                    "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip; h2abi_v1_exit(ctx, 0u)"
                }
                (false, true, true) => {
                    "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip; do { if (!(h2_lent_ != 0) || 0u) h2abi_v1_exit(ctx, 0u)"
                }
                (true, false, _) => {
                    "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip; do { if (h2acc) { h2_gate_drop(h2_mem_, &h2g_); h2abi_v1_charge(ctx, h2acc)"
                }
                (true, true, false) => {
                    "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip; h2abi_v1_exit(ctx, h2acc)"
                }
                (true, true, true) => {
                    "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip; do { if (!(h2_lent_ != 0) || h2acc) h2abi_v1_exit(ctx, h2acc)"
                }
            };
            assert_eq!(shape(n), expect, "{case}: {n}");
        }
    }
}

/// A direct call lends the caller's view just before it invokes the
/// callee, and a hand-up from a handler on a lent view releases nothing.
/// Without direct calls, or with `H2_DIRECT_VIEW=0`, nothing is lent.
#[test]
fn a_direct_call_lends_the_callers_view() {
    let caller = function(A, |a| {
        a.mov(dword_ptr(edx), eax)?;
        a.call(B as u64)?;
        a.mov(dword_ptr(edx), eax)?;
        a.ret()
    });
    let callee = function(B, |a| {
        a.mov(eax, dword_ptr(ecx))?;
        a.ret()
    });
    let Some((src, c)) = write(vec![caller, callee], "exits-direct", true) else {
        common::skip("lent view", "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let text = |defines: &[&str]| preprocess_all(&src, &c, defines);
    let lend = "((d_)->view = h2_mem_); TransferAbi r_ = (";
    let on = text(&["H2_FASTMEM=1", "H2_DIRECT_CALLS=1"]);
    assert_eq!(on.matches(lend).count(), 1, "{on}");
    assert_eq!(
        on.matches(
            "do { if (!(h2_lent_ != 0)) h2abi_v1_fast_release(ctx); } while (0); return (r_);"
        )
        .count(),
        1,
        "{on}"
    );
    // Both handlers take a lent view before they would acquire.
    assert_eq!(calls(&on, "h2_view_borrow"), 2, "{on}");
    for off in [
        text(&["H2_FASTMEM=1", "H2_DIRECT_CALLS=1", "H2_DIRECT_VIEW=0"]),
        text(&["H2_FASTMEM=1", "H2_DIRECT_CALLS=0"]),
        text(&["H2_FASTMEM=0", "H2_DIRECT_CALLS=1"]),
    ] {
        assert!(
            !off.contains("view = h2_mem_") && !off.contains("h2_lent_"),
            "{off}"
        );
    }
    // The module declares the lent view to the runtime only when it lends.
    let registry = src.join("h2gen_registry.c");
    let features = |defines: &[&str]| {
        let t = preprocess_text(&src, &registry, defines);
        let at = t.find("h2gen_module = {").expect("module table");
        t[at..].split(',').nth(3).unwrap().trim().to_string()
    };
    assert_eq!(
        features(&["H2_FASTMEM=1", "H2_DIRECT_CALLS=1"]),
        "1u | (2u | 4u)"
    );
    assert_eq!(
        features(&["H2_FASTMEM=1", "H2_DIRECT_CALLS=1", "H2_DIRECT_VIEW=0"]),
        "1u | (2u | 0u)"
    );
    assert_eq!(
        features(&["H2_FASTMEM=0", "H2_DIRECT_CALLS=1"]),
        "1u | (2u | 0u)"
    );
}

/// `H2_VIEW_CHARGE`: a counting handler with a view calls nothing to
/// settle. Its exit spends its count in the view, as do its charges and
/// its loops' budget; each runtime call is left only for no view.
#[test]
fn a_view_charging_handler_settles_without_a_call() {
    let f = function(A, |a| {
        let mut l = a.create_label();
        a.set_label(&mut l)?;
        a.mov(eax, dword_ptr(ecx))?;
        a.mov(dword_ptr(edx), eax)?;
        a.dec(esi)?;
        a.jnz(l)?;
        a.call(B as u64)?;
        a.ret()
    });
    let g = function(B, |a| {
        a.mov(eax, dword_ptr(ecx))?;
        a.ret()
    });
    let Some((src, c)) = write(vec![f, g], "exits-view-charge", true) else {
        common::skip("view charge", "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    for view in [false, true] {
        for count in [false, true] {
            let lend = format!("H2_DIRECT_VIEW={}", u8::from(view));
            let carry = format!("H2_DIRECT_COUNT={}", u8::from(count));
            let defines = ["H2_FASTMEM=1", "H2_DIRECT_CALLS=1", &lend, &carry, "H2_VIEW_CHARGE=1"];
            let all = preprocess_all(&src, &c, &defines);
            let case = format!("lent views {view}, lent count {count}");
            // Each exit settles inline with a view, and calls without one.
            let exit = "if (!h2_mem_) h2abi_v1_exit(ctx, n_); else if (n_) h2_view_spend(h2_mem_, n_);";
            assert!(all.contains(exit), "{case}: {all}");
            assert_eq!(calls(&all, "h2abi_v1_exit"), all.matches(exit).count(), "{case}: {all}");
            // The loop's budget: from the view.
            let budget = "(h2_mem_ ? h2_view_left(h2_mem_) : h2abi_v1_budget(ctx))";
            assert!(all.contains(budget), "{case}: {all}");
            assert_eq!(calls(&all, "h2abi_v1_budget"), all.matches(budget).count(), "{case}: {all}");
            // A charge (before the direct call) only without a view.
            for (at, _) in all.match_indices("h2abi_v1_charge(ctx") {
                let before = &all[..at];
                assert!(
                    before.ends_with("} else { h2_gate_drop(h2_mem_, &h2g_); ((void)0); ")
                        || before.ends_with("if (h2acc && !h2_mem_) { "),
                    "{case}: a charge with a view: {}",
                    &all[at.saturating_sub(200)..at + 40]
                );
            }
        }
    }
    // Off, the exit is the call.
    let off = preprocess_all(&src, &c, &["H2_FASTMEM=1", "H2_DIRECT_CALLS=1"]);
    assert!(!off.contains("h2_view_spend(h2_mem_") && !off.contains("h2_view_left(h2_mem_)"), "{off}");
}

/// Every handler in the file, whitespace collapsed.
fn preprocess_all(src: &Path, c: &Path, defines: &[&str]) -> String {
    let full = preprocess_text(src, c, defines);
    full[full.find("TransferAbi h2fn_").expect("handler")..].to_string()
}
