//! Compile C fixtures for runtime integration tests. Production runtime builds
//! do not depend on this crate.
use std::{env, path::PathBuf, process::Command};
fn run(command: &mut Command) {
    let status = command.status().expect("C ABI probe requires Clang and ar");
    assert!(status.success(), "C ABI probe command failed: {command:?}");
}
fn main() {
    let sources = ["abi_probe", "generated_module"];
    println!("cargo:rerun-if-changed=../include/h2abi.h");
    for name in sources {
        println!("cargo:rerun-if-changed=../tests/{name}.c");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let library = out.join("libh2abi_probe.a");
    let mut objects = Vec::new();
    for name in sources {
        let object = out.join(format!("{name}.o"));
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
                &format!("../tests/{name}.c"),
                "-o",
            ])
            .arg(&object));
        objects.push(object);
    }
    let _ = std::fs::remove_file(&library);
    run(Command::new("ar").arg("crs").arg(&library).args(&objects));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=h2abi_probe");
}
