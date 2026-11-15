//! `c/h2simd.h` must compile cleanly as strict C11 on its own, for the host
//! and (on macOS, where clang cross-compiles) for both arm64 and x86_64.

mod common;
use std::process::Command;

fn compile(extra: &[&str]) {
    let c = common::crate_dir().join("c");
    let tu = "#include \"h2simd.h\"\nint main(void) { return 0; }\n";
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("h2simd-tu{}.c", extra.join("")));
    std::fs::write(&path, tu).unwrap();
    let out = Command::new(common::cc())
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            "-Wconversion",
            "-Wsign-conversion",
        ])
        .args(["-Wshadow", "-fsyntax-only"])
        .args(extra)
        .arg(format!("-I{}", c.display()))
        .arg(&path)
        .output()
        .expect("run the C compiler");
    assert!(
        out.status.success(),
        "{extra:?}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn header_compiles_strictly() {
    compile(&[]);
    if cfg!(target_os = "macos") {
        compile(&["-arch", "arm64"]);
        compile(&["-arch", "x86_64"]);
    }
}
