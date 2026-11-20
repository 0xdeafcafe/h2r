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

struct Run {
    runtime: std::sync::Arc<Runtime>,
    kernel: std::sync::Arc<kernel::Kernel>,
    device: d3d::abi::SharedDevice<RecordingBackend>,
    machine: Machine,
    source: u32,
}

/// One thread: CreateDevice, register the callback the way `0x12b093`
/// does, then the frame wait for `targets.0` and for `targets.1`. The clock
/// is moved on by `jump` ticks first.
fn setup(targets: (u32, u32), jump: u64) -> Run {
    setup_with(targets, jump, true)
}

/// [`setup`], with or without the clocked source (`source` is 0 without).
fn setup_with(targets: (u32, u32), jump: u64, with_source: bool) -> Run {
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
    let _ = dispatch.register_override(ENTRY, "thread", move |cpu, ctx, _| {
        let runtime = std::sync::Arc::clone(ctx.runtime());
        let mut m = runtime.memory();
        cpu.eax = 0x40;
        cpu.ecx = PP_DEVICE;
        cpu.esp -= 4;
        m.set_u32(cpu.esp, PARAMS)?;
        ctx.call_guest(cpu, &mut m, 0x3f5240, move |cpu, ctx| {
            let runtime = std::sync::Arc::clone(ctx.runtime());
            let mut m = runtime.memory();
            m.set_u32(OUT, cpu.eax)?;
            m.set_u32(DEV + 0x1DB8, CALLBACK)?;
            m.set_u32(FW.target, targets.0)?;
            ctx.call_guest(cpu, &mut m, FW.entry, move |cpu, ctx| {
                let runtime = std::sync::Arc::clone(ctx.runtime());
                let mut m = runtime.memory();
                m.set_u32(OUT + 4, cpu.edx)?;
                m.set_u32(FW.target, targets.1)?;
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
    let id = if with_source {
        kernel
            .add_periodic(Source {
                name: "vblank",
                rate: Rate::HZ_60,
                max_catch_up: 4,
                handler: Box::new(d3d::vblank::clocked_vblanks(device.clone())),
            })
            .unwrap()
    } else {
        0
    };
    let cpu = CpuAbi {
        eip: ENTRY,
        esp: STACK_TOP - 4,
        ..CpuAbi::xbox_thread()
    };
    runtime.memory().set_u32(cpu.esp, HARNESS_EXIT).unwrap();
    kernel.advance_time(&runtime, jump).unwrap();
    let machine = Machine::new(
        runtime.clone(),
        kernel.clone(),
        cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    Run {
        runtime,
        kernel,
        device,
        machine,
        source: id,
    }
}

#[test]
fn the_frame_wait_sleeps_to_each_clocked_vblank() {
    let Run {
        runtime,
        kernel,
        device,
        machine: mut m,
        source: id,
    } = setup((3, 5), 0);
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

#[test]
fn a_clock_jump_reaches_the_game_counter_one_vblank_at_a_time() {
    // A second passes before anything runs: 60 vblanks due. The first
    // drain (four of them) runs before the thread has created the device,
    // so D3D counts those but has no callback to call. The other 56 must
    // all reach Halo's counter (0x485AB0, one per callback), four a drain,
    // or frame time falls behind the clock; dropped, the wait for 60 would
    // only end near two seconds.
    let Run {
        runtime,
        kernel,
        device,
        machine: mut m,
        source: id,
    } = setup((61, 61), 10_000_000);
    assert!(matches!(m.run(1_000).unwrap(), RunOutcome::Complete { .. }));
    let g = runtime.memory();
    assert_eq!(g.u32(OUT + 4).unwrap(), 60, "the wait returned at 60");
    assert_eq!(u64_at(&g, FW.counter), 60);
    assert_eq!(g.u32(OUT + 0x10).unwrap(), 64, "the record's VBlank");
    let stats = kernel.periodic_stats(id).unwrap();
    assert_eq!((stats.delivered, stats.dropped, stats.owed), (64, 0, 0));
    assert_eq!(device.lock().unwrap().stats().vblanks, 64);
    // The whole debt within the second, then four ordinary vblanks.
    assert_eq!(kernel.clock().counter(), (64 * 10_000_000u64).div_ceil(60));
    let dpc = kernel::threads::DPC_THREAD_ID;
    assert!(m.schedule().split(|t| *t != dpc).all(|r| r.len() <= 4));
    // The pre-device batch called nothing, so it is not in the schedule:
    // the thread, then a batch of four callbacks, then the thread again.
    assert_eq!(&m.schedule()[..6], [1, 0, 0, 0, 0, 1]);
}

const WORKER: u32 = 0x003F_0500;
/// Worker rounds, and the relative interval a sleeping worker uses.
const ROUNDS: u32 = OUT + 0x200;
const INTERVAL: u32 = OUT + 0x300;

/// Start a second thread (PsCreateSystemThreadEx) that loops until the
/// frame-wait counter moves: each round either does `accesses` charged guest
/// reads (through the generated code's own access path) and yields, or,
/// with `sleep`, sleeps 100 ms.
fn spawn_worker(r: &Run, accesses: u32, sleep: bool) {
    r.runtime
        .memory()
        .set_u64(INTERVAL, (-1_000_000i64) as u64)
        .unwrap();
    r.runtime.edit_dispatch(|d| {
        d.register_override(WORKER, "test worker", move |cpu, ctx, _| {
            if ctx.memory().u32(FW.counter)? > 0 {
                return ctx.memory().ret(cpu, 0);
            }
            let n = ctx.memory().u32(ROUNDS)?;
            ctx.memory().set_u32(ROUNDS, n + 1)?;
            let spent = ctx.slice_spent();
            let mut out = [0u8; 4];
            for _ in 0..accesses {
                // SAFETY: `ctx` is a live ExecCtx and `out` holds the 4
                // bytes read; this is the entry generated code calls.
                let status = unsafe {
                    runtime::ffi::h2abi_v1_read(
                        (ctx as *mut runtime::ExecCtx).cast(),
                        WORKER,
                        ROUNDS,
                        4,
                        1,
                        out.as_mut_ptr(),
                    )
                };
                assert_eq!(status, 0);
            }
            assert_eq!(ctx.slice_spent() - spent, accesses as u64);
            let rt = ctx.runtime().clone();
            let mut g = rt.memory();
            if sleep {
                cpu.esp -= 12;
                g.set_u32(cpu.esp, 0)?;
                g.set_u32(cpu.esp + 4, 0)?;
                g.set_u32(cpu.esp + 8, INTERVAL)?;
                let token = runtime::kernel::function_token(99); // KeDelayExecutionThread
                return ctx.call_guest(cpu, &mut g, token, |_, _| {
                    Ok(runtime::TransferAbi::jump(WORKER))
                });
            }
            let token = runtime::kernel::function_token(238); // NtYieldExecution
            ctx.call_guest(cpu, &mut g, token, |_, _| {
                Ok(runtime::TransferAbi::jump(WORKER))
            })
        });
    });
    let mut cpu = CpuAbi {
        esp: OUT + 0x2000,
        ..CpuAbi::xbox_thread()
    };
    let args = [OUT + 0x100, 0, 0x4000, 0, 0, WORKER, 0, 0, 0, 0];
    {
        let mut g = r.runtime.memory();
        g.set_u32(cpu.esp, HARNESS_EXIT).unwrap();
        for (i, a) in args.iter().enumerate() {
            g.set_u32(cpu.esp + 4 + 4 * i as u32, *a).unwrap();
        }
    }
    let mut ctx = runtime::ExecCtx::new(r.runtime.clone(), 1);
    let t =
        runtime::Runner::default().run(&mut cpu, &mut ctx, runtime::kernel::function_token(255));
    assert_eq!(ctx.event(t.event_id), Some(&runtime::Event::HarnessExit));
}

#[test]
fn a_busy_worker_does_not_trip_the_frame_wait_watchdog() {
    // Astra's repro: beside the waiting main thread, a worker does exactly
    // 100 charged accesses per yield. Its handoffs are productive, so the
    // clock reaches the first vblank by work, one tick a round: 166667
    // rounds, far more than MAX_IDLE_YIELDS yields of the wait. The clock
    // moves on every one of them, so the wait keeps going and ends there.
    let mut r = setup((2, 2), 0);
    spawn_worker(&r, 100, false);
    let outcome = r.machine.run(1_000_000).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}: {:?}",
        r.device.lock().unwrap().last_stop()
    );
    let g = r.runtime.memory();
    assert_eq!(
        g.u32(OUT + 4).unwrap(),
        1,
        "the wait returned at the vblank"
    );
    assert_eq!(g.u32(ROUNDS).unwrap(), 166_667);
    drop(g);
    assert_eq!(r.kernel.clock().counter(), 166_667);
    assert_eq!(r.kernel.periodic_stats(r.source).unwrap().delivered, 1);
}

#[test]
fn a_frame_wait_with_no_source_stops_after_two_seconds_of_logical_time() {
    // No source, but a worker sleeping 100 ms at a time keeps the clock
    // moving, so the yield count never trips: the logical-time bound does.
    let mut r = setup_with((2, 2), 0, false);
    spawn_worker(&r, 0, true);
    let outcome = r.machine.run(1_000_000).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Stopped { thread: 1, .. }),
        "{outcome:?}"
    );
    let stop = r.device.lock().unwrap().last_stop().unwrap().error.clone();
    assert!(
        stop.contains("ticks of logical time without a vblank"),
        "{stop}"
    );
    let ticks = r.kernel.clock().counter();
    assert!(
        (d3d::vblank::MAX_VBLANK_GAP..d3d::vblank::MAX_VBLANK_GAP + 1_000_001).contains(&ticks),
        "{ticks}"
    );
}
