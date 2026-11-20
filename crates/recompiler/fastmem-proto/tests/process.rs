//! Scenarios that crash on purpose, change task-wide ports or count port
//! names run in a child `fastmem-probe` process.
#![cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64")
))]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Output};

#[cfg(target_os = "macos")]
const PROTECTION_FAULT: i32 = 10; // SIGBUS
const SIGSEGV: i32 = 11;
#[cfg(target_os = "linux")]
const PROTECTION_FAULT: i32 = SIGSEGV;

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
#[cfg(target_os = "macos")]
#[test]
fn declined_fault_reaches_previous_task_handler() {
    expect_ok("chain-task");
}

#[cfg(target_os = "linux")]
#[test]
fn previous_sigaction_receives_the_original_protection_fault() {
    let out = probe(&["chain-handler"]);
    assert_eq!(out.status.code(), Some(42), "{out:?}");
}

/// Five install/teardown rounds release the OS handler state.
#[test]
fn teardown_restores_prior_handler() {
    expect_ok("teardown");
}

#[test]
fn null_deref_still_crashes_with_sigsegv() {
    expect_signal("null-deref", SIGSEGV);
}

#[test]
fn write_outside_regions_still_crashes() {
    expect_signal("unregistered-write", PROTECTION_FAULT);
}

#[test]
fn read_fault_in_region_is_not_taken_for_a_write() {
    expect_signal("region-read-none", PROTECTION_FAULT);
}

#[test]
fn unattached_thread_is_not_watched() {
    expect_signal("unattached-thread-write", PROTECTION_FAULT);
}

/// Runs the probe under lldb: handled faults stay invisible to the
/// debugger, and a foreign fault stops it with EXC_BAD_ACCESS. Ignored by
/// default because it needs Xcode's lldb and a GUI-less debug session.
#[cfg(target_os = "macos")]
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
