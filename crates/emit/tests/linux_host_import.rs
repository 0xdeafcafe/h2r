//! A generated Linux module may leave a function for the executable to
//! supply. Check both eager failure and binding to the executable on Linux.
#![cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]

mod common;

use std::path::Path;
use std::process::Command;

use emit::compile::{self, Arch};

#[test]
fn linux_elf_resolves_a_direct_host_import_at_load() {
    let arch = Arch::host();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("linux-host-import");
    let src = dir.join("src");
    let owners = emit::plan(vec![common::function(0x11000, |a| a.ret())]);
    emit::write_module(&owners, &src).unwrap();
    compile::install_header(&src, &common::abi_header().unwrap()).unwrap();
    let build = compile::build(&src, &dir, arch, 1).unwrap();
    let module = dir.join(arch.module_file_name());
    compile::host_dylib(&build, &src, &[("bridge.c", emit::host::BRIDGE)], &module).unwrap();
    {
        // The normal module receives Rust callbacks by pointer. It loads
        // in this executable without exporting any host function by name.
        let generated = unsafe { emit::host::Generated::load(&module) }.unwrap();
        assert!(generated.entries.contains_key(&0x11000));
        let process = libloading::os::unix::Library::this();
        let global = unsafe { process.get::<*const ()>(b"h2bridge_layout_v3") };
        assert!(global.is_err(), "the bridge leaked into the process-wide symbol table");
    }

    let probe = r#"
#include "h2gen.h"
extern int h2_test_host_answer(void);
H2_EXPORT int h2_test_call_host(void) { return h2_test_host_answer(); }
"#;
    compile::host_dylib(&build, &src, &[("bridge.c", emit::host::BRIDGE), ("probe.c", probe)], &module).unwrap();

    // The Rust test process has no h2_test_host_answer. The module is valid,
    // but RTLD_NOW must reject it before anyone calls h2_test_call_host.
    let error = unsafe { emit::host::open_library(&module) }.unwrap_err();
    assert!(error.to_string().contains("h2_test_host_answer"), "{error}");

    // The same module works when an executable exports the C ABI name.
    let host = dir.join("host");
    let source = dir.join("host.c");
    std::fs::write(
        &source,
        r#"
#include <dlfcn.h>
#include <stdio.h>
int h2_test_host_answer(void) { return 42; }
int main(int argc, char **argv) {
    if (argc != 2) return 1;
    void *module = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!module) { fprintf(stderr, "%s\n", dlerror()); return 1; }
    int (*call_host)(void) = dlsym(module, "h2_test_call_host");
    return !(call_host && call_host() == 42);
}
"#,
    )
    .unwrap();
    let link = Command::new(compile::compiler().unwrap())
        .args(arch.target_args().unwrap())
        .arg("-rdynamic")
        .arg(&source)
        .arg("-ldl")
        .arg("-o")
        .arg(&host)
        .output()
        .unwrap();
    assert!(link.status.success(), "{}", String::from_utf8_lossy(&link.stderr));
    let run = Command::new(host).arg(module).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
}
