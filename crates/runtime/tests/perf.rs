use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, Runner, Runtime, TransferAbi};
use runtime::{
    guest::Permissions,
    perf::{Profile, Scope},
};

#[test]
fn profiling_is_opt_in_drains_and_merges_without_changing_execution() {
    let mut guest = Guest::new();
    guest.map_zeroed(0x10000, 4096, Permissions::RX).unwrap();
    let mut dispatch = Dispatch::default();
    dispatch.register_override(0x10000, "test graphics", |_, ctx, _| {
        ctx.measure(Scope::Gfx, |ctx| ctx.measure(Scope::Fallback, |_| ()));
        Ok(TransferAbi::ret(runtime::exec::HARNESS_EXIT))
    });
    assert!(dispatch.profile_override(0x10000, Scope::D3d));
    assert!(!dispatch.profile_override(0x20000, Scope::D3d));
    let mut ctx = ExecCtx::new(Runtime::new(guest, dispatch), 1);
    let mut cpu = CpuAbi::default();
    let runner = Runner::default();
    assert_eq!(
        runner.run(&mut cpu, &mut ctx, 0x10000).kind,
        runtime::abi::STOP
    );
    assert!(ctx.profile().is_none());
    ctx.enable_profiling(true);
    assert_eq!(
        runner.run(&mut cpu, &mut ctx, 0x10000).kind,
        runtime::abi::STOP
    );
    let p = ctx.take_profile().unwrap();
    assert_eq!((p.transfers, p.runner_safe_points), (1, 2));
    for scope in [Scope::Runner, Scope::D3d, Scope::Gfx, Scope::Fallback] {
        assert_eq!(p.timing(scope).calls, 1);
    }
    assert!(p.timing(Scope::Runner).elapsed >= p.timing(Scope::D3d).elapsed);
    assert!(p.timing(Scope::D3d).elapsed >= p.timing(Scope::Gfx).elapsed);
    let mut total = Profile::default();
    total.merge(&p);
    total.merge(&p);
    assert_eq!(total.transfers, 2);
    assert_eq!(total.timing(Scope::Gfx).calls, 2);
    assert_eq!(ctx.profile().unwrap(), &Profile::default());
    assert!(total.to_string().contains("Kernel: unobserved"));
    ctx.enable_profiling(false);
    assert!(ctx.take_profile().is_none());
}

#[test]
fn ffi_direct_buffers_preserve_cross_mapping_fault_atomicity_and_alignment() {
    use runtime::ffi::{h2abi_v1_read, h2abi_v1_write};
    let mut guest = Guest::new();
    guest.map(0x1000, 4096, 0, Permissions::RW).unwrap();
    guest.map(0x2000, 4096, 8192, Permissions::READ).unwrap();
    let rt = Runtime::new(guest, Dispatch::default());
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.enable_profiling(true);
    let ptr = (&mut ctx as *mut ExecCtx).cast();
    for width in [1, 2, 4, 8, 10, 16] {
        let input = [0x5au8; 16];
        let mut output = [0xa5u8; 16];
        // SAFETY: buffers are live, correctly sized, and disjoint from ctx.
        unsafe {
            assert_eq!(h2abi_v1_write(ptr, 0, 0x1101, width, 1, input.as_ptr()), 0);
            assert_eq!(
                h2abi_v1_read(ptr, 0, 0x1101, width, 1, output.as_mut_ptr()),
                0
            );
        }
        assert_eq!(&output[..width as usize], &input[..width as usize]);
        assert!(output[width as usize..].iter().all(|b| *b == 0xa5));
    }
    rt.memory().write_bytes(0x1ff8, &[0x44; 8], 1).unwrap();
    let mut output = [0xa5; 16];
    let input = [0x5a; 16];
    // Cross-mapping write fails on the read-only second page with no prefix
    // changed; read traverses physically discontiguous pages successfully.
    unsafe {
        assert_ne!(h2abi_v1_write(ptr, 0, 0x1ff8, 16, 1, input.as_ptr()), 0);
        assert_eq!(h2abi_v1_read(ptr, 0, 0x1ff8, 16, 1, output.as_mut_ptr()), 0);
    }
    assert_eq!(&output[..8], &[0x44; 8]);
    assert_eq!(&output[8..], &[0; 8]);
    rt.memory()
        .protect_range(0x2000, 4096, Permissions::NONE)
        .unwrap();
    output.fill(0xa5);
    unsafe {
        assert_ne!(h2abi_v1_read(ptr, 0, 0x1ff8, 16, 1, output.as_mut_ptr()), 0);
        assert_ne!(
            h2abi_v1_read(ptr, 0, 0x1101, 16, 16, output.as_mut_ptr()),
            0
        );
    }
    assert_eq!(output, [0xa5; 16]);
    assert_eq!(ctx.profile().unwrap().memory_accessors, 16);
}

#[test]
fn mapping_hints_follow_insert_protect_unmap_remap_and_alias_changes() {
    use runtime::guest::{Access, FaultReason};
    let mut guest = Guest::new();
    guest.map(0x4000, 4096, 0, Permissions::RW).unwrap();
    guest.set_u32(0x4000, 7).unwrap();
    assert_eq!(guest.u32(0x4000).unwrap(), 7); // fill the hint
    guest.map(0x1000, 4096, 4096, Permissions::RW).unwrap(); // shift indices
    assert_eq!(guest.u32(0x4000).unwrap(), 7);
    guest.map(0x8000, 4096, 0, Permissions::READ).unwrap();
    assert_eq!(guest.u32(0x8000).unwrap(), 7);
    guest
        .protect_range(0x4000, 4096, Permissions::READ)
        .unwrap();
    assert_eq!(
        guest.set_u32(0x4000, 8).unwrap_err().reason,
        FaultReason::Protection
    );
    guest.unmap(0x4000, 4096).unwrap();
    assert_eq!(guest.u32(0x4000).unwrap_err().reason, FaultReason::Unmapped);
    guest.map(0x4000, 4096, 8192, Permissions::RW).unwrap();
    guest.set_u32(0x4000, 9).unwrap();
    assert_eq!(guest.u32(0x4000).unwrap(), 9);
    assert_eq!(guest.u32(0x8000).unwrap(), 7); // old backing still aliased
    guest
        .protect_range(0x4000, 4096, Permissions::NONE)
        .unwrap();
    assert_eq!(
        guest
            .check(0x4000, 1, Access::Execute, 1)
            .unwrap_err()
            .reason,
        FaultReason::Protection
    );
    guest.protect_range(0x4000, 4096, Permissions::RW).unwrap();
    guest.map(0x5000, 4096, 12288, Permissions::NONE).unwrap();
    guest.set_u32(0x4ffc, 0x11223344).unwrap();
    assert!(guest.write_bytes(0x4ffc, &[0x55; 8], 1).is_err());
    assert_eq!(guest.u32(0x4ffc).unwrap(), 0x11223344);
    // A hint must never make device addresses or 32-bit wrap valid.
    assert_eq!(
        guest.u32(0xfd000000).unwrap_err().reason,
        FaultReason::UnsupportedDevice
    );
    assert_eq!(
        guest.u32(u32::MAX).unwrap_err().reason,
        FaultReason::AddressOverflow
    );
}

#[test]
fn mapping_hints_remain_safe_for_shared_readers() {
    let mut guest = Guest::new();
    for i in 0..16 {
        guest
            .map(0x1000 + i * 8192, 4096, i * 4096, Permissions::RW)
            .unwrap();
        guest.set_u32(0x1000 + i * 8192, i).unwrap();
    }
    std::thread::scope(|scope| {
        for t in 0..4 {
            let guest = &guest;
            scope.spawn(move || {
                for n in 0..1000 {
                    let i = (n + t) % 16;
                    assert_eq!(guest.u32(0x1000 + i * 8192).unwrap(), i);
                }
            });
        }
    });
}
