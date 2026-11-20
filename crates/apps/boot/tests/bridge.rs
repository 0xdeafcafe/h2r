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
H2_EXPORT uint32_t boot_memory_probe(ExecCtxOpaque *ctx, uint32_t addr) {
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
        > = lib.get(b"h2boot_install_v3").unwrap();
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
            let inline = enabled && std::env::var("H2_FASTMEM").as_deref() != Ok("0");
            assert_eq!(p.fast_memory_accesses, u64::from(inline));
            // Rejection must leave the previously installed table usable.
            assert_eq!(install(std::ptr::null(), 0), 0);
            assert_ne!(probe((&mut ctx as *mut ExecCtx).cast(), 0x3000), 0);
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// A module built before `h2abi_v1_exit` (its bridge without the `exit`
/// slot, installed as `h2boot_install_v2`; its handlers charging and then
/// releasing) still loads, through the v2 prefix of the table, and settles
/// the same accesses as a current one leaving through `h2abi_v1_exit`.
#[test]
fn a_module_from_before_the_exit_call_still_installs_and_settles() {
    let dir = std::env::temp_dir().join(format!("boot-exit-bridge-{}", std::process::id()));
    let src = dir.join("src");
    emit::write_module(&[], &src).unwrap();
    emit::compile::install_header(&src, Path::new(boot::H2ABI_H)).unwrap();
    let build = emit::compile::build(&src, &dir, emit::compile::Arch::host(), 1).unwrap();
    // The old bridge: this one without the slot, its symbol or its install.
    let exit_slot = "    void (*exit)(ExecCtxOpaque *, uint64_t);\n";
    let exit_fn =
        "void h2abi_v1_exit(ExecCtxOpaque *c, uint64_t accesses) { bridge.exit(c, accesses); }";
    assert!(boot::BRIDGE.contains(exit_slot) && boot::BRIDGE.contains(exit_fn));
    let old_bridge = boot::BRIDGE
        .replace(exit_slot, "")
        .replace(exit_fn, "")
        .replace("h2boot_install_v3", "h2boot_install_v2");
    // One handler's accesses: three inline (a 4-byte write each through the
    // view, uncounted under accounting so counted here by hand) and a
    // handler count of `n`, then its way out.
    let probe = |exit: &str| {
        format!(
            r#"
#include "h2gen.h"
H2_EXPORT uint32_t boot_exit_probe(ExecCtxOpaque *ctx, uint32_t addr, uint64_t n) {{
    H2FastAccess *f = h2abi_v1_fast_acquire(ctx);
    const uint32_t value = 0x12345678u;
    uint32_t result = 0;
    for (uint32_t i = 0; i < 3u && !result; i++)
        result = h2_mem_write(f, ctx, 0x1000u, addr + 4u * i, 4u, 1u, (const uint8_t *)&value);
    {exit}
    return result;
}}
"#
        )
    };
    let modules = [
        (
            "old",
            old_bridge,
            probe("if (n) h2abi_v1_charge(ctx, n); h2abi_v1_fast_release(ctx);"),
            2,
        ),
        (
            "new",
            boot::BRIDGE.to_string(),
            probe("h2abi_v1_exit(ctx, n);"),
            3,
        ),
    ];
    let mut results = Vec::new();
    for (name, bridge, probe, version) in modules {
        let path = dir.join(format!("{name}.dylib"));
        emit::compile::host_dylib(
            &build,
            &src,
            &[("boot_bridge.c", &bridge), ("probe.c", &probe)],
            &path,
        )
        .unwrap();
        // SAFETY: the test just built this dylib from known C and the current ABI.
        unsafe {
            let lib = libloading::Library::new(&path).unwrap();
            assert_eq!(
                boot::install_bridge(&lib, &boot::shim::bridge()).unwrap(),
                version,
                "{name}"
            );
            let probe: libloading::Symbol<
                unsafe extern "C" fn(*mut runtime::ExecCtxOpaque, u32, u64) -> u32,
            > = lib.get(b"boot_exit_probe").unwrap();
            let mut seen = Vec::new();
            for slice in [None, Some(1), Some(3), Some(4), Some(5), Some(9)] {
                for n in [0u64, 1, 2, 7] {
                    let rt = Runtime::new(Guest::new(), Dispatch::default());
                    rt.memory()
                        .map_zeroed(0x1000, 0x1000, Permissions::RW)
                        .unwrap();
                    let mut ctx = ExecCtx::new(rt.clone(), 0);
                    ctx.enable_profiling(true);
                    if let Some(s) = slice {
                        ctx.arm_preemption(s);
                    }
                    let c = (&mut ctx as *mut ExecCtx).cast();
                    assert_eq!(probe(c, 0x1000, n), 0);
                    assert_eq!(rt.memory().u32(0x1008).unwrap(), 0x12345678);
                    let budget = runtime::ffi::h2abi_v1_budget(c);
                    let p = ctx.take_profile().unwrap();
                    seen.push((
                        slice,
                        n,
                        p.memory_accessors,
                        p.fast_memory_accesses,
                        ctx.slice_spent(),
                        budget,
                        ctx.preempt_pending(),
                    ));
                }
            }
            results.push(seen);
        }
    }
    assert_eq!(results[0], results[1]);
    // The writes and the handler's count all reached the slice.
    let inline = std::env::var("H2_FASTMEM").as_deref() != Ok("0");
    for &(slice, n, charged, fast, spent, _, pending) in &results[1] {
        assert_eq!(charged, 3 + n);
        assert_eq!(fast, if inline { 3 } else { 0 });
        assert_eq!(spent, if slice.is_some() { 3 + n } else { 0 });
        assert_eq!(
            pending != 0,
            slice.is_some_and(|s| s <= 3 + n),
            "{slice:?} {n}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
