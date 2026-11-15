//! The clocked vertical blank end to end: `d3d::vblank::clocked_vblanks` on
//! a kernel periodic source at 60 Hz, the frame wait in `Delivery::Clock`
//! mode yielding through the kernel's own `NtYieldExecution`, under
//! `kernel::Machine`. The guest callback is a Rust stand-in for Halo's
//! `0x12b2a0` (it bumps the 64-bit counter at `0x485AB0`); generated-code
//! spins on a vblank counter are in the kernel's tests/preempt.rs.

mod common;

use common::{H, MISC, PARAMS, PB, PP_DEVICE, SCRATCH, W, synthetic_encode_bytes};
use d3d::{
    DeviceConfig, FrameWait, PresentParameters, RecordingBackend, SparseMemory, runtime_glue,
};
use kernel::periodic::{Rate, Source};
use kernel::{Machine, RunOutcome};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Guest, Runtime};

const DEV: u32 = 0x0040_4FE0;
const CALLBACK: u32 = 0x003F_0100;
/// Thread entry: create the device, then call the frame wait twice.
const ENTRY: u32 = 0x003F_0300;
const STACK_BOTTOM: u32 = 0x6FFF_0000;
const STACK_TOP: u32 = 0x7000_0000;
const FW: FrameWait = FrameWait::HALO2_CLOCKED;
/// Thread 1's EAX at each step, for the test to read back.
const OUT: u32 = MISC + 0x9000;

fn guest(cfg: &DeviceConfig) -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x003F_0000, 0x1_4000, Permissions::RX)
        .unwrap();
    g.map_zeroed(0x0012_B000, 0x1000, Permissions::RX).unwrap();
    g.map_zeroed(0x0040_4000, 0x5000, Permissions::RW).unwrap();
    g.map_zeroed(0x0048_5000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x004E_6000, 0x1000, Permissions::RW).unwrap();
    let encode = d3d::GuestLayout::HALO2.simple_encode;
    g.map_zeroed(encode & !0xFFF, 0x1000, Permissions::RW)
        .unwrap();
    g.write_bytes(encode, &synthetic_encode_bytes(), 1).unwrap();
    g.map_zeroed(MISC, 0x1_0000, Permissions::RW).unwrap();
    g.map_zeroed(STACK_BOTTOM, STACK_TOP - STACK_BOTTOM, Permissions::RW)
        .unwrap();
    runtime_glue::map_device_memory(&mut g, cfg).unwrap();
    let mut params = SparseMemory::new();
    params.map(PARAMS, 0x100);
    PresentParameters {
        width: W,
        height: H,
        back_buffer_format: 0x06,
        back_buffer_count: 0,
        multisample_type: 0,
        swap_effect: 0,
        enable_auto_depth_stencil: true,
        auto_depth_stencil_format: 0x2A,
        flags: 0,
        refresh_rate: 0,
        presentation_interval: 0x8000_0000,
        buffer_surface: 0,
        depth_stencil_surface: 0,
    }
    .write(&mut params, PARAMS)
    .unwrap();
    let mut bytes = vec![0; PresentParameters::SIZE as usize];
    d3d::GuestMemory::read(&params, PARAMS, &mut bytes).unwrap();
    g.write_bytes(PARAMS, &bytes, 1).unwrap();
    g
}

fn u64_at(g: &Guest, addr: u32) -> u64 {
    (g.u32(addr + 4).unwrap() as u64) << 32 | g.u32(addr).unwrap() as u64
}

#[test]
fn the_frame_wait_sleeps_to_each_clocked_vblank() {
    let cfg = DeviceConfig::halo2(PB, SCRATCH);
    let g = guest(&cfg);
    let device = runtime_glue::shared(cfg, RecordingBackend::new());
    let mut dispatch = Dispatch::default();
    d3d::register(&mut dispatch, device.clone());
    d3d::register_frame_wait(
        &mut dispatch,
        device.clone(),
        FW,
        std::sync::Arc::new(runtime_glue::default_stop),
    );
    // 0x12b2a0 -> 0x14280, reduced: count the vblank, keep the record.
    let _ = dispatch.register_override(CALLBACK, "test vblank callback", |cpu, ctx, _| {
        assert_eq!(ctx.thread_id, kernel::threads::DPC_THREAD_ID);
        let mut m = ctx.memory();
        let record = m.u32(cpu.esp + 4)?;
        let v = (m.u32(FW.counter + 4)? as u64) << 32 | m.u32(FW.counter)? as u64;
        let v = v + 1;
        m.set_u32(FW.counter, v as u32)?;
        m.set_u32(FW.counter + 4, (v >> 32) as u32)?;
        let vblank = m.u32(record)?;
        m.set_u32(OUT + 0x10, vblank)?;
        m.ret(cpu, 0)
    });
    // CreateDevice (EAX = 0x40, ECX = ppDevice, [esp+4] = params), register
    // the callback the way 0x12b093 does, then wait for target 3 twice.
    let _ = dispatch.register_override(ENTRY, "thread", |cpu, ctx, _| {
        let runtime = std::sync::Arc::clone(ctx.runtime());
        let mut m = runtime.memory();
        cpu.eax = 0x40;
        cpu.ecx = PP_DEVICE;
        cpu.esp -= 4;
        m.set_u32(cpu.esp, PARAMS)?;
        ctx.call_guest(cpu, &mut m, 0x3f5240, |cpu, ctx| {
            let runtime = std::sync::Arc::clone(ctx.runtime());
            let mut m = runtime.memory();
            m.set_u32(OUT, cpu.eax)?;
            m.set_u32(DEV + 0x1DB8, CALLBACK)?;
            m.set_u32(FW.target, 3)?;
            ctx.call_guest(cpu, &mut m, FW.entry, |cpu, ctx| {
                let runtime = std::sync::Arc::clone(ctx.runtime());
                let mut m = runtime.memory();
                m.set_u32(OUT + 4, cpu.edx)?;
                m.set_u32(FW.target, 5)?;
                ctx.call_guest(cpu, &mut m, FW.entry, |cpu, ctx| {
                    let mut m = ctx.memory();
                    m.set_u32(OUT + 8, cpu.edx)?;
                    m.ret(cpu, 0)
                })
            })
        })
    });
    let runtime = Runtime::new(g, dispatch);
    let kernel = kernel::Kernel::install(&runtime, kernel::Config::default()).unwrap();
    let id = kernel
        .add_periodic(Source {
            name: "vblank",
            rate: Rate::HZ_60,
            max_catch_up: 4,
            handler: Box::new(d3d::vblank::clocked_vblanks(device.clone())),
        })
        .unwrap();
    let cpu = CpuAbi {
        eip: ENTRY,
        esp: STACK_TOP - 4,
        ..CpuAbi::xbox_thread()
    };
    runtime.memory().set_u32(cpu.esp, HARNESS_EXIT).unwrap();
    let mut m = Machine::new(
        runtime.clone(),
        kernel.clone(),
        cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    assert!(matches!(m.run(100).unwrap(), RunOutcome::Complete { .. }));
    let g = runtime.memory();
    assert_eq!(g.u32(OUT).unwrap(), 0, "CreateDevice");
    // Each wait returns with the counter at target - 1 (EDX), reached by
    // the clock: two vblanks, then two more.
    assert_eq!((g.u32(OUT + 4).unwrap(), g.u32(OUT + 8).unwrap()), (2, 4));
    assert_eq!(u64_at(&g, FW.counter), 4);
    assert_eq!(g.u32(OUT + 0x10).unwrap(), 4, "the record's VBlank");
    // Nothing else was runnable, so the clock went straight to each
    // vblank: it ends exactly at the fourth.
    assert_eq!(kernel.clock().counter(), (4 * 10_000_000u64).div_ceil(60));
    assert_eq!(m.preemptions(), 0);
    let stats = kernel.periodic_stats(id).unwrap();
    assert_eq!((stats.delivered, stats.dropped), (4, 0));
    assert_eq!(device.lock().unwrap().stats().vblanks, 4);
}
