//! What a handler's way out calls, as the C compiler sees it: the emitted
//! module preprocessed with the build's own flags (synthetic code, no game
//! data). The shared epilogue settles with the runtime in one
//! `h2abi_v1_exit`, after the spill; `H2_EXIT_CALL=0` is the pair it
//! replaced, a charge before the spill and a release after it.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::function;
use iced_x86::code_asm::*;

const A: u32 = 0x0040_0000;

/// One load and one store, so the handler counts accesses and can fault.
fn source(accounting: bool) -> Option<(PathBuf, PathBuf)> {
    let abi = common::abi_header()?;
    let f = function(A, |a| {
        a.mov(eax, dword_ptr(ecx))?;
        a.mov(dword_ptr(edx), eax)?;
        a.ret()
    });
    let owners = emit::plan(vec![f]);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("exits-{accounting}"));
    let src = dir.join("src");
    let _ = std::fs::remove_dir_all(&src);
    emit::write_module_with(
        &owners,
        &src,
        emit::Codegen {
            accounting,
            ..emit::Codegen::default()
        },
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
    let cc = emit::compile::compiler().unwrap();
    let mut cmd = Command::new(cc);
    cmd.args(["-E", "-P"])
        .args(emit::compile::cflags().unwrap())
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
    // The handler is the last function in the file, after the headers.
    let body = &text[text.rfind("TransferAbi h2_").expect("handler")..];
    body.split_whitespace().collect::<Vec<_>>().join(" ")
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
        for fastmem in [false, true] {
            let fm = format!("H2_FASTMEM={}", u8::from(fastmem));
            let new = preprocess(&src, &c, &[&fm]);
            let old = preprocess(&src, &c, &[&fm, "H2_EXIT_CALL=0"]);
            let (n, o) = (epilogue(&new), epilogue(&old));
            let case = format!("accounting {accounting}, fastmem {fastmem}");
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
            if fastmem {
                let count = if accounting { "h2acc" } else { "0u" };
                let call = format!("h2abi_v1_exit(ctx, {count});");
                let at = n.find(&call).unwrap_or_else(|| panic!("{case}: {n}"));
                assert!(n.find("cpu->eip = x_eip;").unwrap() < at, "{case}: {n}");
                // Only the (off) perf flush follows it.
                assert_eq!(n[at + call.len()..].trim(), "((void)0);", "{case}: {n}");
            }
            // Nothing else in the handler changed.
            let strip = |s: &str| s.replace(n, "").replace(o, "");
            assert_eq!(strip(&new), strip(&old), "{case}");
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
            let expect = match (accounting, fastmem) {
                (false, false) => "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip",
                (false, true) => {
                    "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip; h2abi_v1_exit(ctx, 0u)"
                }
                (true, false) => {
                    "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip; do { if (h2acc) { h2_gate_drop(h2_mem_, &h2g_); h2abi_v1_charge(ctx, h2acc)"
                }
                (true, true) => {
                    "h2_exit: h2_gate_drop(h2_mem_, &h2g_); cpu->eip = x_eip; h2abi_v1_exit(ctx, h2acc)"
                }
            };
            assert_eq!(shape(n), expect, "{case}: {n}");
        }
    }
}
