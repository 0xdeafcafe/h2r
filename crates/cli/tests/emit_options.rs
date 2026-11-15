//! Invalid build options fail before parsing an image or starting clang.
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn rejected(options: &[&str], message: &str) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "h2-build-options-{}-{}.xbe",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    // Enough for the CLI's magic check, deliberately too short to parse.
    std::fs::write(&path, b"XBEH synthetic").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_h2"))
        .arg("emit")
        .arg(&path)
        .args(options)
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains(message), "{error}");
    assert!(
        !error.contains("opening"),
        "validation must precede XBE parsing: {error}"
    );
}

#[test]
fn unknown_architecture_fails_before_parsing_the_image() {
    rejected(&["--arch", "mips"], "expected host, all, arm64 or x86_64");
}

#[test]
fn invalid_parallelism_fails_before_parsing_the_image() {
    for options in [&["--jobs", "0"][..], &["--jobs", "many"], &["--jobs"]] {
        rejected(options, "--jobs needs a positive count");
    }
}

#[test]
fn running_the_entry_requires_a_host_build() {
    let other = if cfg!(target_arch = "x86_64") {
        "arm64"
    } else {
        "x86_64"
    };
    rejected(
        &["--arch", other, "--run-entry"],
        "--run-entry requires the host architecture",
    );
}
