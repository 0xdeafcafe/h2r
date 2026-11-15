//! Synthetic bridge ABI checks: no game image or retail instruction bytes.
use runtime::guest::Permissions;
use runtime::{Dispatch, ExecCtx, Guest, Runtime};
use std::path::Path;

#[test]
fn optional_callbacks_preserve_memory_tracking_and_reject_short_tables() {
    let dir = std::env::temp_dir().join(format!("boot-optional-bridge-{}", std::process::id()));
    let src = dir.join("src");
    emit::write_module(&[], &src).unwrap();
    emit::compile::install_header(&src, Path::new(boot::H2ABI_H)).unwrap();
    let build = emit::compile::build(&src, &dir, emit::compile::Arch::host(), 1).unwrap();
    let path = dir.join("bridge.dylib");
    let probe = r#"
#include "h2gen.h"
uint32_t boot_memory_probe(ExecCtxOpaque *ctx, uint32_t addr) {
    H2FastAccess *f = h2abi_v1_fast_acquire(ctx);
    const uint32_t value = 0x12345678u;
    uint32_t result = h2_mem_write(f, ctx, 0x1000u, addr, 4u, 1u, (const uint8_t *)&value);
    h2abi_v1_profile(ctx, 3u, 5u);
    h2abi_v1_fast_release(ctx);
    return result;
}
"#;
    emit::compile::host_dylib(
        &build,
        &src,
        &[("boot_bridge.c", boot::BRIDGE), ("probe.c", probe)],
        &path,
    )
    .unwrap();
    // SAFETY: the test just built this dylib from known C and the current ABI.
    unsafe {
        let lib = libloading::Library::new(&path).unwrap();
        let install: libloading::Symbol<
            unsafe extern "C" fn(*const boot::shim::Bridge, usize) -> u32,
        > = lib.get(b"h2boot_install_v2").unwrap();
        let probe: libloading::Symbol<
            unsafe extern "C" fn(*mut runtime::ExecCtxOpaque, u32) -> u32,
        > = lib.get(b"boot_memory_probe").unwrap();
        // A null short table must be rejected before any read or install.
        assert_eq!(
            install(std::ptr::null(), 9 * std::mem::size_of::<usize>()),
            0
        );
        for enabled in [false, true] {
            assert_eq!(
                install(
                    &boot::shim::bridge_with_fast_memory(enabled),
                    std::mem::size_of::<boot::shim::Bridge>()
                ),
                1
            );
            let rt = Runtime::new(Guest::new(), Dispatch::default());
            rt.memory()
                .map_zeroed(0x1000, 0x1000, Permissions::RW)
                .unwrap();
            let watch = rt.memory().subscribe_writes("bridge");
            let physical = rt.memory().fold_address(0x1000).unwrap();
            rt.memory().watch_writes(watch, physical, 0x1000).unwrap();
            let mut ctx = ExecCtx::new(rt.clone(), 0);
            ctx.enable_profiling(true);
            ctx.arm_preemption(1);
            assert_eq!(probe((&mut ctx as *mut ExecCtx).cast(), 0x1000), 0);
            assert_eq!(rt.memory().u32(0x1000).unwrap(), 0x12345678);
            assert!(rt.memory().has_dirty(watch));
            assert_eq!(ctx.slice_spent(), 1);
            let p = ctx.take_profile().unwrap();
            assert_eq!(
                (p.loop_safe_points, p.x87_helpers, p.memory_accessors),
                (3, 5, 1)
            );
            let inline = enabled && std::env::var("H2_FASTMEM").as_deref() == Ok("1");
            assert_eq!(p.fast_memory_accesses, u64::from(inline));
            // Rejection must leave the previously installed table usable.
            assert_eq!(install(std::ptr::null(), 0), 0);
            assert_ne!(probe((&mut ctx as *mut ExecCtx).cast(), 0x3000), 0);
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
