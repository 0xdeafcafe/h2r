//! Compile C fixtures for runtime integration tests. Production runtime builds
//! do not depend on this crate.
fn main() {
    println!("cargo:rerun-if-changed=../include/h2abi.h");
    println!("cargo:rerun-if-changed=../tests/abi_probe.c");
    println!("cargo:rerun-if-changed=../tests/generated_module.c");
    // Honor the same target-specific CC, CFLAGS (including the sysroot) and
    // AR as the host's other native dependencies, including on a cross-build.
    cc::Build::new()
        .std("c11")
        .warnings(true)
        .extra_warnings(true)
        .warnings_into_errors(true)
        .include("../include")
        .file("../tests/abi_probe.c")
        .file("../tests/generated_module.c")
        .compile("h2abi_probe");
}
