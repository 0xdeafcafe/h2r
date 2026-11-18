//! A Cranelift fast-tier module (crates/clif) links with boot's own bridges
//! (`BRIDGE`, `BRIDGE_X87`), as `tier::build_fast` links it, and exports
//! everything `load_module` looks up, with its handlers in the table.
//! Synthetic x86 bytes only; no game image.

use std::collections::BTreeMap;
use std::path::Path;

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

#[test]
fn a_fast_tier_module_links_with_boots_bridges() {
    // fld1; fstp dword [ebx]; mov [ebx+4], eax; ret
    let code = [0xd9, 0xe8, 0xd9, 0x1b, 0x89, 0x43, 0x04, 0xc3];
    let entry = 0x0001_0000;
    let owners = emit::plan(vec![function(entry, &code)]);
    let dir = std::env::temp_dir().join(format!("boot-fast-tier-{}", std::process::id()));
    let lib_path = dir.join(boot::DYLIB);
    let r = clif::build_dylib(
        &owners,
        &dir,
        Path::new(boot::H2ABI_H),
        &[("h2boot_bridge.c", boot::BRIDGE), ("h2boot_bridge_x87.c", boot::BRIDGE_X87)],
        &lib_path,
        1,
    );
    let checked = r.map(|_| {
        // SAFETY: linked just now for this host from synthetic code and
        // the current ABI; only its exported tables are read.
        unsafe {
            let lib = libloading::Library::new(&lib_path).unwrap();
            let version: libloading::Symbol<unsafe extern "C" fn() -> u32> = lib.get(b"h2gen_module_abi_version").unwrap();
            assert_eq!(version(), runtime::cpu::ABI_VERSION);
            lib.get::<*const ()>(b"h2boot_install_v3").expect("boot's bridge installer");
            lib.get::<*const ()>(b"h2x87bridge_install").expect("the x87 bridge installer");
            let m: libloading::Symbol<*const runtime::generated::H2GenModule> = lib.get(b"h2gen_module").unwrap();
            let m = &**m;
            assert_eq!(m.abi_version, runtime::cpu::ABI_VERSION);
            assert_eq!(m.entry_count, 1);
            let e = &*m.entries;
            assert_eq!(e.addr, entry);
            assert!(e.handler.is_some());
            assert_eq!((e.abi_version.unwrap())(), runtime::cpu::ABI_VERSION);
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
    checked.unwrap();
}
