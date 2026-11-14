//! Compile C fixtures for runtime integration tests. Production runtime builds
//! do not depend on this crate.
use std::{env, path::PathBuf, process::Command};
fn run(command: &mut Command) {
    let status = command.status().expect("C ABI probe requires Clang and ar");
    assert!(status.success(), "C ABI probe command failed: {command:?}");
}
fn main() {
    for file in ["../include/h2abi.h", "../tests/abi_probe.c"] {
        println!("cargo:rerun-if-changed={file}");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let object = out.join("abi_probe.o");
    let library = out.join("libh2abi_probe.a");
    run(Command::new("clang")
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-target",
            &target,
            "-I../include",
            "-c",
            "../tests/abi_probe.c",
            "-o",
        ])
        .arg(&object));
    run(Command::new("ar").arg("crs").arg(&library).arg(&object));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=h2abi_probe");
}
