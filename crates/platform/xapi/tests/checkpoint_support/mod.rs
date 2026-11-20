//! Runtime ABI bridge for the retail checkpoint fixture.
use runtime::{CpuAbi, ExecCtxOpaque};
use std::{ffi::c_char, path::Path};

/// emit's test bridge (`c/bridge.c`) forwards the ABI imports through this
/// table; here it points at the runtime's own exports.
#[repr(C)]
struct Bridge {
    read: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *mut u8) -> u32,
    write: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *const u8) -> u32,
    event: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32) -> u32,
    unsupported: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, *const c_char, *const c_char) -> u32,
    rdtsc: unsafe extern "C" fn(*mut ExecCtxOpaque, u32) -> u64,
    rmw: unsafe extern "C" fn(
        *mut CpuAbi,
        *mut ExecCtxOpaque,
        u32,
        u32,
        u32,
        u32,
        u32,
        *mut u32,
    ) -> u32,
    xchg:
        unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, *mut u32) -> u32,
    cmpxchg:
        unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, *mut u32) -> u32,
}

impl Bridge {
    /// What bridge.c's `h2bridge_layout` must report: each slot's offset in
    /// declaration order, then the size.
    const LAYOUT: [usize; 9] = [
        std::mem::offset_of!(Bridge, read),
        std::mem::offset_of!(Bridge, write),
        std::mem::offset_of!(Bridge, event),
        std::mem::offset_of!(Bridge, unsupported),
        std::mem::offset_of!(Bridge, rdtsc),
        std::mem::offset_of!(Bridge, rmw),
        std::mem::offset_of!(Bridge, xchg),
        std::mem::offset_of!(Bridge, cmpxchg),
        size_of::<Bridge>(),
    ];
}

/// Build the module for this host and load it, wired to the runtime.
pub fn build(functions: Vec<ir::IrFunction>, dir: &Path) -> libloading::Library {
    let header = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/runtime/include/h2abi.h");
    let src = dir.join("src");
    let owners = emit::plan(functions);
    emit::write_module(&owners, &src).unwrap();
    emit::compile::install_header(&src, &header).unwrap();
    let arch = emit::compile::Arch::host();
    let b = emit::compile::build(&src, dir, arch, 2).unwrap_or_else(|e| panic!("{e}"));
    let dylib = dir.join("libcheckpoints.dylib");
    emit::compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &dylib)
        .unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: built just now by emit for this host.
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    let bridge = Bridge {
        read: runtime::ffi::h2abi_v1_read,
        write: runtime::ffi::h2abi_v1_write,
        event: runtime::generated::h2shim_v1_event,
        unsupported: runtime::generated::h2shim_v1_unsupported,
        rdtsc: runtime::generated::h2shim_v1_rdtsc,
        rmw: runtime::ffi::h2abi_v1_atomic_rmw,
        xchg: runtime::ffi::h2abi_v1_xchg,
        cmpxchg: runtime::ffi::h2abi_v1_cmpxchg,
    };
    // SAFETY: bridge.c copies the table, whose layout is checked first;
    // the signatures match h2abi.h.
    unsafe {
        let layout: libloading::Symbol<unsafe extern "C" fn(u32) -> usize> =
            lib.get(b"h2bridge_layout").unwrap();
        let c: Vec<usize> = (0..Bridge::LAYOUT.len() as u32)
            .map(|i| layout(i))
            .collect();
        assert_eq!(
            c,
            Bridge::LAYOUT,
            "bridge.c's H2Bridge and this Bridge differ"
        );
        let install: libloading::Symbol<unsafe extern "C" fn(*const Bridge)> =
            lib.get(b"h2bridge_install").unwrap();
        install(&bridge);
    }
    lib
}
