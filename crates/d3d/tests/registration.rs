//! `d3d::register` against the real `crates/runtime`: overrides installed in
//! a `Dispatch`, driven by `runtime::Runner` over a `runtime::Guest`, through
//! Halo's boot-time device sequence (`0x12420`: CreateDevice, Swap, Release
//! via the teardown `0x3fc530`; then `0x12560`: CreateDevice again and
//! SetShaderConstantMode).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use common::{H, MISC, PARAMS, PB, PP_DEVICE, SCRATCH, W, synthetic_encode_bytes};
use d3d::abi::{self, SharedDevice};
use d3d::backend::ResolvedDraw;
use d3d::runtime_glue::{self, Registration, Stop};
use d3d::shader::{PreparedShader, ShaderProvider};
use d3d::{DeviceConfig, GfxBackend, PresentParameters, RecordingBackend, SparseMemory};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi as rt};

const DEV: u32 = 0x0040_4FE0;
const STACK: u32 = MISC + 0xF000;

/// A guest with the D3D code pages executable (the runner checks the
/// target page even for overrides), the D3D globals, the device's push
/// buffer and scratch, and a parameter/stack page.
fn guest(cfg: &DeviceConfig, map_push_buffer: bool) -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x003F_0000, 0x1_4000, Permissions::RX)
        .unwrap();
    g.map_zeroed(0x0040_4000, 0x5000, Permissions::RW).unwrap();
    let encode = d3d::GuestLayout::HALO2.simple_encode;
    g.map_zeroed(encode & !0xFFF, 0x1000, Permissions::RW)
        .unwrap();
    g.write_bytes(encode, &synthetic_encode_bytes(), 1).unwrap();
    g.map_zeroed(MISC, 0x1_0000, Permissions::RW).unwrap();
    if map_push_buffer {
        runtime_glue::map_device_memory(&mut g, cfg).unwrap();
    } else {
        g.map_zeroed(cfg.scratch.addr, cfg.scratch.len, Permissions::RW)
            .unwrap();
    }
    // Halo's boot parameters (0x12490): 640x480 is too big for nothing, the
    // tests use the common 64x64 target.
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

struct Harness {
    ctx: ExecCtx,
    device: SharedDevice<RecordingBackend>,
    registration: Registration,
}

fn harness(hook: Option<runtime_glue::StopHook>) -> Harness {
    harness_with(hook, true)
}

fn harness_with(hook: Option<runtime_glue::StopHook>, map_push_buffer: bool) -> Harness {
    let cfg = DeviceConfig::halo2(PB, SCRATCH);
    let g = guest(&cfg, map_push_buffer);
    let device = runtime_glue::shared(cfg, RecordingBackend::new());
    let mut dispatch = Dispatch::default();
    let registration = match hook {
        Some(h) => runtime_glue::register_with(&mut dispatch, device.clone(), h),
        None => d3d::register(&mut dispatch, device.clone()),
    };
    let rt = Runtime::new(g, dispatch);
    Harness {
        ctx: ExecCtx::new(rt, 1),
        device,
        registration,
    }
}

impl Harness {
    /// Calls `entry` with `regs` and `stack` arguments, returning to the
    /// harness exit. Returns the final transfer and the CPU after it.
    fn call(&mut self, entry: u32, mut cpu: CpuAbi, stack: &[u32]) -> (TransferAbi, CpuAbi) {
        let esp = STACK - 4 - 4 * stack.len() as u32;
        {
            let mut m = self.ctx.memory();
            m.set_u32(esp, HARNESS_EXIT).unwrap();
            for (i, v) in stack.iter().enumerate() {
                m.set_u32(esp + 4 + 4 * i as u32, *v).unwrap();
            }
        }
        cpu.esp = esp;
        let t = Runner::default().run(&mut cpu, &mut self.ctx, entry);
        (t, cpu)
    }

    /// As [`call`](Self::call), asserting a normal return.
    fn ok(&mut self, entry: u32, cpu: CpuAbi, stack: &[u32]) -> CpuAbi {
        let (t, cpu) = self.call(entry, cpu, stack);
        assert_eq!(t.kind, rt::STOP, "{entry:#x}: {t:?}");
        assert_eq!(
            self.ctx.event(t.event_id),
            Some(&Event::HarnessExit),
            "{entry:#x} did not return: {:?}",
            self.device.lock().unwrap().last_stop()
        );
        assert_eq!(cpu.esp, STACK, "{entry:#x} pops its arguments");
        cpu
    }

    fn u32(&mut self, addr: u32) -> u32 {
        self.ctx.memory().u32(addr).unwrap()
    }
}

fn regs(eax: u32, ecx: u32) -> CpuAbi {
    CpuAbi {
        eax,
        ecx,
        ..CpuAbi::default()
    }
}

#[test]
fn register_installs_every_entry_but_the_native_helpers() {
    let mut dispatch = Dispatch::default();
    let device = runtime_glue::shared(DeviceConfig::halo2(PB, SCRATCH), RecordingBackend::new());
    let r = d3d::register(&mut dispatch, device);
    let entries = abi::entries::<RecordingBackend>();
    assert_eq!(r.implemented.len() + r.stopping.len(), entries.len());
    // The unimplemented entries plus the two port-I/O guards that stop.
    assert_eq!(r.stopping.len(), abi::UNIMPLEMENTED.len() + 2);
    // The counts docs/d3d.md gives: 88 overrides (85 + 3) and 7 native.
    assert_eq!(
        (r.implemented.len(), r.stopping.len(), r.native.len()),
        (85, 3, 7)
    );
    // The functions holding the GPIO port I/O (0x3fe131, 0x3feb3c,
    // 0x3fee0d) never run: 0x3fe005 and 0x3fed90 stop, the ISR 0x3feac0
    // answers "not mine".
    for addr in [0x3fe005, 0x3fed90] {
        assert!(r.stopping.iter().any(|e| e.0 == addr), "{addr:#x}");
    }
    assert!(r.implemented.iter().any(|e| e.0 == 0x3feac0));
    assert!(r.implemented.iter().any(|e| e.0 == 0x3f5cc0));
    for e in &entries {
        assert_eq!(dispatch.override_name(e.addr), Some(e.name));
    }
    for (addr, _) in &r.native {
        assert_eq!(dispatch.override_name(*addr), None, "{addr:#x}");
    }
    // The boot path is overridden: CreateDevice (whose original reaches the
    // NV2A at 0x3fe16b), Swap, teardown, SetShaderConstantMode.
    for addr in [0x3f5240, 0x3f9d00, 0x3fc530, 0x3f6e10] {
        assert!(r.implemented.iter().any(|e| e.0 == addr), "{addr:#x}");
    }
}

#[test]
fn halo_boot_device_sequence_runs_through_the_runner() {
    let mut h = harness(None);
    // 0x14850 (in 0x12560) runs the original surface helpers.
    for addr in [0x3fdbc0, 0x3fde00, 0x3fb7a0] {
        assert!(h.registration.native.iter().any(|n| n.0 == addr));
    }
    // 0x12490: EAX = 0x40, ECX = &g_device, [esp+4] = &params; ret 4.
    let cpu = h.ok(0x3f5240, regs(0x40, PP_DEVICE), &[PARAMS]);
    assert_eq!(cpu.eax, 0);
    assert_eq!(h.u32(PP_DEVICE), DEV);
    assert_eq!(h.u32(0x0040_7488), DEV, "D3D__pDevice");
    assert_eq!(h.u32(0x0040_5918), 1, "reference count (device + 0x938)");
    // 0x12433: Swap(0).
    let cpu = h.ok(0x3f9d00, regs(0, 0), &[]);
    assert_eq!(cpu.eax, 1);
    // 0x12448..0x1246b: refcount 1, so teardown, then the game zeroes the
    // device and D3D__pDevice itself.
    h.ok(0x3fc530, regs(DEV, 0), &[]);
    {
        let mut m = h.ctx.memory();
        m.write_bytes(DEV, &vec![0; 0x928 * 4], 1).unwrap();
        m.set_u32(0x0040_7488, 0).unwrap();
    }
    // 0x128c0 creates it again (a second create without teardown stops).
    let cpu = h.ok(0x3f5240, regs(0x40, PP_DEVICE), &[PARAMS]);
    assert_eq!(cpu.eax, 0);
    // 0x12675: SetShaderConstantMode(0x11).
    h.ok(0x3f6e10, regs(0x11, 0), &[]);
    assert_eq!(h.u32(DEV + 8) & 0x200, 0x200);
    assert_eq!(h.u32(DEV + 0x1928), 1);
    // 0x1267c: SetRenderState_ZEnable(2), stdcall.
    h.ok(0x3f54f0, regs(0, 0), &[2]);
    let d = h.device.lock().unwrap();
    assert!(d.last_stop().is_none());
    let presents = d
        .backend
        .calls
        .iter()
        .filter(|c| matches!(c, d3d::Recorded::Present { .. }));
    assert_eq!(presents.count(), 1);
}

#[test]
fn unimplemented_entries_stop_with_their_name() {
    let mut h = harness(None);
    h.ok(0x3f5240, regs(0x40, PP_DEVICE), &[PARAMS]);
    let (t, _) = h.call(0x3f77c0, regs(0, 0), &[]);
    assert_eq!(t.kind, rt::STOP);
    let d = h.device.lock().unwrap();
    let s = d.last_stop().unwrap().clone();
    drop(d);
    assert_eq!((s.entry, s.name), (0x3f77c0, "D3DDevice_Reset"));
    assert!(s.error.contains("unimplemented"), "{}", s.error);
    // The runtime's own event, with the same text and the recorded chain.
    match h.ctx.event(t.event_id) {
        Some(Event::Unsupported {
            pc,
            mnemonic,
            detail,
            backtrace,
        }) => {
            assert_eq!(*pc, 0x3f77c0);
            assert_eq!(mnemonic, "D3DDevice_Reset");
            assert_eq!(*detail, s.error);
            assert_eq!(*backtrace, h.ctx.backtrace(0x3f77c0));
            assert_eq!(backtrace.first().map(|f| f.pc), Some(0x3f77c0));
        }
        other => panic!("{other:?}"),
    }
    let report = h.ctx.report(t.event_id).unwrap().to_string();
    assert!(report.contains("D3DDevice_Reset"), "{report}");
}

#[test]
fn guest_faults_inside_d3d_are_runtime_memory_faults() {
    let mut h = harness(None);
    h.ok(0x3f5240, regs(0x40, PP_DEVICE), &[PARAMS]);
    // SetViewport(pViewport) with an unmapped pointer.
    let (t, _) = h.call(0x3f8100, regs(0, 0), &[0x0700_0000]);
    assert_eq!(t.kind, rt::FAULT, "{t:?}");
    assert!(
        matches!(h.ctx.event(t.event_id), Some(Event::MemoryFault { fault, .. }) if fault.addr == 0x0700_0000),
        "{:?}",
        h.ctx.event(t.event_id)
    );
}

/// A fault while draining the push buffer (`D3dError::Push(PushError::Guest)`)
/// is the runtime's fault too, not a stop.
#[test]
fn push_buffer_faults_are_runtime_memory_faults() {
    let mut h = harness_with(None, false);
    // CreateDevice pushes the viewport registers (as `0x3fbc02` does), and
    // the push buffer is not mapped.
    let (t, _) = h.call(0x3f5240, regs(0x40, PP_DEVICE), &[PARAMS]);
    assert_eq!(t.kind, rt::FAULT, "{t:?}");
    assert!(
        matches!(h.ctx.event(t.event_id), Some(Event::MemoryFault { fault, .. }) if fault.addr == PB.addr),
        "{:?}",
        h.ctx.event(t.event_id)
    );
    assert!(h.device.lock().unwrap().last_stop().is_none());
}

#[test]
fn a_custom_stop_hook_receives_the_error() {
    let seen = Arc::new(AtomicU32::new(0));
    let s = seen.clone();
    let hook: runtime_glue::StopHook = Arc::new(move |ctx: &mut ExecCtx, stop: &Stop<'_>| {
        assert!(
            stop.error.to_string().contains("no device"),
            "{}",
            stop.error
        );
        s.store(stop.entry, Ordering::SeqCst);
        ctx.emit(Event::HarnessExit)
    });
    let mut h = harness(Some(hook));
    // Swap before CreateDevice.
    let (t, _) = h.call(0x3f9d00, regs(0, 0), &[]);
    assert_eq!(t.kind, rt::STOP);
    assert_eq!(seen.load(Ordering::SeqCst), 0x3f9d00);
}

/// A provider with the full-state signature, installable in a `GfxBackend`
/// that is still `Send`.
struct NoShaders;

impl ShaderProvider for NoShaders {
    fn prepare(
        &mut self,
        _ctx: &gfx::GfxContext,
        _renderer: &mut gfx::Renderer,
        draw: &ResolvedDraw,
        _textures: [Option<&wgpu::TextureView>; 4],
    ) -> Result<PreparedShader, String> {
        Err(format!(
            "untranslated: {}x{} target, alpha ref {}",
            draw.target.width, draw.target.height, draw.state.render.alpha_ref
        ))
    }
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn gfx_backend_registration_type_checks() {
    assert_send_sync::<SharedDevice<GfxBackend>>();
    let _register: fn(&mut Dispatch, SharedDevice<GfxBackend>) -> Registration =
        d3d::register::<GfxBackend>;
    let _install: fn(GfxBackend) -> GfxBackend = |b| {
        b.with_shaders(Box::new(NoShaders))
            .on_present(|_, _, _, _, _| {})
    };
}
