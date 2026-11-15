//! Scenarios that crash on purpose, change task-wide ports or count port
//! names run in a child `fastmem-probe` process.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Output};

const SIGBUS: i32 = 10;
const SIGSEGV: i32 = 11;

fn probe(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fastmem-probe"))
        .args(args)
        .output()
        .unwrap()
}

fn expect_ok(scenario: &str) {
    let out = probe(&[scenario]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{scenario}: {:?}\n{stdout}{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains(&format!("{scenario} ok")), "{stdout}");
}

fn expect_signal(scenario: &str, signal: i32) {
    let out = probe(&[scenario]);
    assert_eq!(
        out.status.signal(),
        Some(signal),
        "{scenario}: {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn smoke() {
    expect_ok("smoke");
}

/// A fault our server declines reaches a task-level handler installed
/// before it (the kernel's thread -> task -> host triage).
#[test]
fn declined_fault_reaches_previous_task_handler() {
    expect_ok("chain-task");
}

/// Five install/teardown rounds leave the task's port-name count unchanged.
#[test]
fn teardown_leaks_no_ports() {
    expect_ok("teardown");
}

#[test]
fn null_deref_still_crashes_with_sigsegv() {
    expect_signal("null-deref", SIGSEGV);
}

#[test]
fn write_outside_regions_still_crashes_with_sigbus() {
    expect_signal("unregistered-write", SIGBUS);
}

#[test]
fn read_fault_in_region_is_not_taken_for_a_write() {
    expect_signal("region-read-none", SIGBUS);
}

#[test]
fn unattached_thread_is_not_watched() {
    expect_signal("unattached-thread-write", SIGBUS);
}

/// Runs the probe under lldb: handled faults stay invisible to the
/// debugger, and a foreign fault stops it with EXC_BAD_ACCESS. Ignored by
/// default because it needs Xcode's lldb and a GUI-less debug session.
#[test]
#[ignore = "needs lldb; run with --ignored"]
fn works_under_lldb() {
    let exe = env!("CARGO_BIN_EXE_fastmem-probe");
    let out = Command::new("lldb")
        .args(["--batch", "-o", "run", "--", exe, "all"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("teardown ok"), "{text}");
    assert!(text.contains("exited with status = 0"), "{text}");

    let out = Command::new("lldb")
        .args([
            "--batch",
            "-o",
            "run",
            "-o",
            "kill",
            "--",
            exe,
            "unregistered-write",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("stop reason = EXC_BAD_ACCESS (code=2"),
        "{text}"
    );
}
