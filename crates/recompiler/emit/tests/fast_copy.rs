//! Synthetic RAM views, including physical aliases; no guest data.
use std::{path::PathBuf, process::Command};
#[test]
fn bulk_copy_checks_and_accounting() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fast-copy");
    std::fs::create_dir_all(&scratch).unwrap();
    for accounting in [0, 1] {
        let exe = scratch.join(format!("copy-{accounting}"));
        let out = Command::new("clang")
            .args([
                "-std=c11",
                "-O1",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsanitize=undefined",
                "-fno-sanitize-recover=all",
            ])
            .arg(format!("-DH2_ACCOUNTING={accounting}"))
            .arg("-I")
            .arg(root.join("c"))
            .arg("-I")
            .arg(root.join("../../runtime/runtime/include"))
            .arg(root.join("tests/c/fast_copy.c"))
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(Command::new(exe).status().unwrap().success());
    }
}

#[test]
#[ignore = "microbenchmark; run with --nocapture"]
fn guest_microbenchmarks() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("guest-bench");
    std::fs::create_dir_all(&scratch).unwrap();
    let executables: Vec<_> = [0, 1]
        .map(|native| {
            let exe = scratch.join(format!("bench-{native}"));
            let out = Command::new("clang")
                .args([
                    "-std=c11",
                    "-O1",
                    "-fno-fast-math",
                    "-ffp-contract=off",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-DH2_SSE_NATIVE=1",
                ])
                .arg(format!("-DNATIVE={native}"))
                .arg("-I")
                .arg(root.join("c"))
                .arg("-I")
                .arg(root.join("../simd/c"))
                .arg("-I")
                .arg(root.join("../../runtime/runtime/include"))
                .arg(root.join("tests/c/guest_bench.c"))
                .arg("-o")
                .arg(&exe)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            exe
        })
        .into();
    // Alternating order exposes drift; thread CPU time excludes descheduling.
    for native in [0, 1, 1, 0] {
        assert!(
            Command::new(&executables[native])
                .status()
                .unwrap()
                .success()
        );
    }
}
