//! A Cranelift fast-tier module (crates/recompiler/clif) builds as
//! `tier::build_fast` builds it, with no C compiler anywhere on `PATH`,
//! exports everything `load_module` looks up, with its handlers in the
//! table, and takes boot's own bridge and helpers. Synthetic x86 bytes
//! only; no game image. Unix: the test stands in its own `PATH`.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use iced_x86::{Decoder, DecoderOptions};
use ir::{IrBlock, IrFunction};

/// One block, lowered as the lifter would: x87 (helper imports), a guest
/// store (the checked accessors) and a return.
fn function(base: u32, code: &[u8]) -> IrFunction {
    let env = lift::Env::default();
    let insns = Decoder::with_ip(32, code, base as u64, DecoderOptions::NONE)
        .into_iter()
        .map(|i| lift::lower(&i, &env, lift::Site::Plain))
        .collect();
    IrFunction {
        entry: base,
        blocks: BTreeMap::from([(base, IrBlock { start: base, insns })]),
        interior_entries: Vec::new(),
        errors: Vec::new(),
    }
}

/// Programs a fast build must never run: each, if run, leaves a mark in
/// `dir` and fails.
const COMPILERS: [&str; 5] = ["clang", "cc", "gcc", "clang-22", "c++"];

/// A `PATH` of one directory holding only the linker (and, on a Mac, the
/// `xcrun` it asks for the SDK) from the real `PATH`, plus a fake of each
/// [`COMPILERS`] that marks `marks` when run.
fn no_compiler_path(dir: &Path, marks: &Path) -> OsString {
    use std::os::unix::fs::PermissionsExt;
    let real = std::env::var_os("PATH");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(marks).unwrap();
    let tools: &[&str] = if cfg!(target_os = "linux") { &["ld.lld", "ld"] } else { &["ld", "xcrun"] };
    for t in tools {
        if let Some(p) = emit::compile::resolve_on_path(t, real.clone()) {
            let _ = std::fs::remove_file(bin.join(t));
            std::os::unix::fs::symlink(p, bin.join(t)).unwrap();
        }
    }
    for c in COMPILERS {
        let p = bin.join(c);
        std::fs::write(&p, format!("#!/bin/sh\ntouch '{}/{c}'\nexit 1\n", marks.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin.into_os_string()
}

fn marks(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default()
}

#[test]
fn a_fast_tier_module_builds_without_a_c_compiler_and_takes_boots_bridge() {
    assert_eq!(size_of::<recomp::shim::Bridge>() as u64, clif::BOOT_BRIDGE_SIZE, "h2boot_install_v3's size check");
    assert_eq!(recomp::shim::clif_helpers().len(), clif::CLIF_COUNT);

    // fld1; fstp dword [ebx]; mov [ebx+4], eax; addss xmm0, xmm1; ret
    let code = [0xd9, 0xe8, 0xd9, 0x1b, 0x89, 0x43, 0x04, 0xf3, 0x0f, 0x58, 0xc1, 0xc3];
    let entry = 0x0001_0000;
    let owners = emit::plan(vec![function(entry, &code)]);
    let dir = std::env::temp_dir().join(format!("boot-fast-tier-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib_path = dir.join("fast").join(recomp::DYLIB);
    let mark_dir = dir.join("marks");
    let path = no_compiler_path(&dir, &mark_dir);
    // Every lookup in the build, and every program it starts, sees only
    // that PATH. SAFETY: this binary's only test; nothing reads the
    // environment concurrently.
    let saved = std::env::var_os("PATH");
    unsafe { std::env::set_var("PATH", &path) };
    let r = clif::build_dylib(&owners, &dir.join("fast"), &lib_path, 1);
    match saved {
        Some(p) => unsafe { std::env::set_var("PATH", p) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    let ran = marks(&mark_dir);
    let checked = r.map(|_| {
        // SAFETY: linked just now for this host from synthetic code and
        // the current ABI; only its exported tables and installers are
        // used.
        unsafe {
            let lib = libloading::Library::new(&lib_path).unwrap();
            let version: libloading::Symbol<unsafe extern "C" fn() -> u32> = lib.get(b"h2gen_module_abi_version").unwrap();
            assert_eq!(version(), runtime::cpu::ABI_VERSION);
            assert_eq!(recomp::install_bridge(&lib, &recomp::shim::bridge()).unwrap(), 3, "boot's v3 bridge");
            type Install = unsafe extern "C" fn(*const *const std::ffi::c_void, u32);
            let x87: libloading::Symbol<Install> = lib.get(b"h2x87bridge_install").expect("the x87 bridge installer");
            let fns = recomp::shim::x87_helpers();
            x87(fns.as_ptr(), fns.len() as u32);
            let own: libloading::Symbol<Install> = lib.get(b"h2clif_install_v1").expect("the fast tier's helper installer");
            let fns = recomp::shim::clif_helpers();
            own(fns.as_ptr(), fns.len() as u32);
            let m: libloading::Symbol<*const runtime::generated::H2GenModule> = lib.get(b"h2gen_module").unwrap();
            let m = &**m;
            assert_eq!(m.abi_version, runtime::cpu::ABI_VERSION);
            assert_eq!(m.features, 0);
            assert_eq!(m.entry_count, 1);
            let e = &*m.entries;
            assert_eq!(e.addr, entry);
            assert!(e.handler.is_some());
            assert_eq!((e.abi_version.unwrap())(), runtime::cpu::ABI_VERSION);
            // The zero row after the last entry.
            let end = &*m.entries.add(1);
            assert_eq!((end.addr, end.handler.is_none(), end.abi_version.is_none()), (0, true, true));
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ran.is_empty(), "the fast build ran a C compiler: {ran:?}");
    checked.unwrap();
}
