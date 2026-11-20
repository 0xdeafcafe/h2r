//! D3D round 5: the resource busy check against the device's GPU model,
//! `SetStipple` and the stipple methods, the push-buffer software methods
//! (inlined `InsertCallback`), `NV097_BREAK_VERTEX_BUFFER_CACHE`,
//! `SetVertexShaderConstantNotInline`, and the guards over the functions
//! that hold GPIO port I/O. Synthetic values only; the ignored test at the
//! end checks the addresses against the retail image in `$H2_XBE` by
//! decoding its instructions.

use crate::common::{self, *};
use d3d::abi::Regs;
use d3d::device::PushCallback;
use d3d::layout::device_field as f;
use d3d::pushbuf::method;
use d3d::{
    D3dError, Device, GuestLayout, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy,
};

const DEV: u32 = 0x0040_4FE0;
const L: GuestLayout = GuestLayout::HALO2;

fn rec() -> (Device<RecordingBackend>, SparseMemory) {
    create(RecordingBackend::new(), UploadPolicy::Tracked)
}

fn push_words(m: &mut SparseMemory, words: &[u32]) {
    let mut put = m.read_u32(DEV).unwrap();
    for w in words {
        m.write_u32(put, *w).unwrap();
        put += 4;
    }
    m.write_u32(DEV, put).unwrap();
}

fn header(m: u32, count: u32) -> u32 {
    (count << 18) | m
}

/// What `0x2460e` stores: `SET_ZSTENCIL_CLEAR_VALUE` and
/// `SET_COLOR_CLEAR_VALUE` = (function, context), `WAIT_FOR_IDLE`, then
/// `NO_OPERATION` with software method `kind`.
fn insert_callback(m: &mut SparseMemory, function: u32, context: u32, kind: u32) {
    push_words(
        m,
        &[
            header(0x1D8C, 2),
            function,
            context,
            header(method::WAIT_FOR_IDLE, 1),
            0,
            header(method::NOP, 1),
            kind,
        ],
    );
}

fn stipple_row0(m: &mut SparseMemory, v: u32) {
    push_words(m, &[header(0x1480, 1), v]);
}

fn busy(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, r: u32) -> u32 {
    common::call(
        d,
        m,
        0x3fb710,
        Regs {
            ecx: r,
            ..Regs::default()
        },
        &[],
    )
    .unwrap()
    .eax
    .unwrap()
}

// Busy check ----------------------------------------------------------------

#[test]
fn create_device_points_the_completion_word_into_scratch() {
    let (mut d, mut m) = rec();
    let word = m.read_u32(DEV + f::GPU_COMPLETED).unwrap();
    assert_eq!(
        word,
        SCRATCH.addr + d3d::device::GPU_COMPLETED_OFFSET,
        "device +0x30"
    );
    assert_eq!(m.read_u32(word).unwrap(), 0);
    assert_eq!(m.read_u32(DEV + f::GPU_TIME).unwrap(), 0);
    // SetFence: the fence is both the GPU time and, since the device runs
    // the queue synchronously, the completed time.
    let fence = common::call(&mut d, &mut m, 0x3fa680, Regs::default(), &[0])
        .unwrap()
        .eax
        .unwrap();
    assert_eq!(fence, 1);
    assert_eq!(m.read_u32(DEV + f::GPU_TIME).unwrap(), 1);
    assert_eq!(m.read_u32(word).unwrap(), 1);
    assert_eq!(d.gpu_completed(), 1);
}

#[test]
fn busy_check_runs_the_fence_test_against_the_completed_time() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    for _ in 0..3 {
        common::call(&mut d, &mut m, 0x3fa680, Regs::default(), &[0]).unwrap();
    }
    assert_eq!(d.gpu_completed(), 3);
    // Last used at a fence that has passed (or never): idle, Lock cleared.
    for lock in [0, 1, 3] {
        m.write_u32(TEX_HEADER + 8, lock).unwrap();
        assert_eq!(busy(&mut d, &mut m, TEX_HEADER), 0, "Lock {lock}");
        assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 0);
    }
    // GPU_TIME - Lock < GPU_TIME - completed: a use newer than the last
    // completed fence is busy, and Lock stays. The device never stamps one
    // (its fences complete as they are issued), so the guest's GPU time is
    // moved ahead here to stand for work still queued on a real GPU.
    m.write_u32(DEV + f::GPU_TIME, 5).unwrap();
    m.write_u32(TEX_HEADER + 8, 4).unwrap();
    assert_eq!(busy(&mut d, &mut m, TEX_HEADER), 1);
    assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 4);
    // At the completed time exactly: passed.
    m.write_u32(TEX_HEADER + 8, 3).unwrap();
    assert_eq!(busy(&mut d, &mut m, TEX_HEADER), 0);
    // A lock far ahead wraps: GPU_TIME - Lock is huge, so not busy (the
    // unsigned compare at 0x3fb77f).
    m.write_u32(TEX_HEADER + 8, 0x8000_0000).unwrap();
    assert_eq!(busy(&mut d, &mut m, TEX_HEADER), 0);
}

#[test]
fn busy_check_lock_bits_win_over_the_fence() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    let common = m.read_u32(TEX_HEADER).unwrap();
    // An internal reference: busy whatever Lock says, Lock kept.
    m.write_u32(TEX_HEADER, common | 0x10_0000).unwrap();
    m.write_u32(TEX_HEADER + 8, 0).unwrap();
    assert_eq!(busy(&mut d, &mut m, TEX_HEADER), 1);
    m.write_u32(TEX_HEADER, common).unwrap();
    // A surface level whose parent texture is bound: busy; unbound, the
    // parent's Lock is the one tested and cleared.
    let surf = MISC + 0x6300;
    m.write_u32(surf, 1 | 0x5_0000).unwrap();
    m.write_u32(surf + 8, 0x77).unwrap();
    m.write_u32(surf + 0x14, TEX_HEADER).unwrap();
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    assert_eq!(busy(&mut d, &mut m, surf), 1);
    d.set_texture(&mut m, 0, 0).unwrap();
    // Unbinding stamps the parent's Lock with the device's fence (0 here,
    // and every fence has passed).
    assert_eq!(busy(&mut d, &mut m, surf), 0);
    assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 0);
    assert_eq!(m.read_u32(surf + 8).unwrap(), 0x77);
}

#[test]
fn busy_check_executes_queued_work_first() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    stipple_row0(&mut m, 0x1234);
    assert_eq!(busy(&mut d, &mut m, TEX_HEADER), 0);
    assert_eq!(d.stipple_pattern()[0], 0x1234, "drained before the test");
}

// Stipple -------------------------------------------------------------------

#[test]
fn stipple_methods_land_in_the_register_state() {
    let (mut d, mut m) = rec();
    // Inline, as at 0x1e4ae: all 32 rows.
    let mut words = vec![header(0x1480, 32)];
    words.extend((0..32).map(|i| 0xF0F0_0000 | i));
    push_words(&mut m, &words);
    // SET_STIPPLE_CONTROL outside the encode table: the fallback field.
    push_words(&mut m, &[header(0x147C, 1), 1]);
    d.commit(&mut m).unwrap();
    assert_eq!(d.stipple_pattern()[31], 0xF0F0_001F);
    assert_eq!(d.hw().stipple_control, Some(1));
    // SetStipple pushes the same method; the entry is named for it.
    let e = d3d::abi::entries::<RecordingBackend>()
        .into_iter()
        .find(|e| e.addr == 0x3f8cc0)
        .unwrap();
    assert_eq!(e.name, "D3DDevice_SetStipple");
}

// Software methods and push-buffer callbacks --------------------------------

#[test]
fn a_callback_stops_the_drain_right_after_itself() {
    let (mut d, mut m) = rec();
    stipple_row0(&mut m, 0xAAAA);
    insert_callback(&mut m, 0x0002_4970, 0x00C0_FFEE, 7);
    stipple_row0(&mut m, 0x5555);
    let cb = d.drain_until_callback(&mut m).unwrap().unwrap();
    assert_eq!(
        (cb.function, cb.context, cb.kind),
        (0x0002_4970, 0x00C0_FFEE, 7)
    );
    // Work before it has run, work after it has not.
    assert_eq!(d.stipple_pattern()[0], 0xAAAA);
    assert_eq!(d.hw().zstencil_clear_value, 0x0002_4970);
    // Going on runs the rest, and there is no second callback.
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    assert_eq!(d.stipple_pattern()[0], 0x5555);
    assert_eq!(d.stats().push_callbacks, 1);
    // A READ callback (6) is delivered the same way.
    insert_callback(&mut m, 0x0002_4970, 1, 6);
    assert!(matches!(
        d.drain_until_callback(&mut m),
        Ok(Some(PushCallback { kind: 6, .. }))
    ));
}

/// Guest memory that refuses writes to one range (the level's texels).
struct DenyWrites<'a> {
    inner: &'a mut SparseMemory,
    from: u32,
    len: u32,
}

impl GuestMemory for DenyWrites<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.inner.read(addr, out)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        let end = addr as u64 + data.len() as u64;
        if addr < self.from + self.len && end > self.from as u64 {
            return Err(d3d::GuestError {
                addr,
                len: data.len() as u32,
            });
        }
        self.inner.write(addr, data)
    }
}

/// Review recheck P2: a write-back that fails at the callback boundary
/// must not lose the callback. The error is returned unlatched (as for any
/// write-back), and the retry writes back and hands out the same callback
/// before the work queued after it runs.
#[test]
fn a_failed_write_back_keeps_the_callback_for_the_retry() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    let surf = MISC + 0x7200;
    let mut l =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    l.parent = TEX_HEADER;
    l.write(&mut m, surf).unwrap();
    d.backend.readback_fill = 0x5A;
    d.set_render_target(&mut m, surf, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    insert_callback(&mut m, 0x003F_0100, 0x1234, 7);
    stipple_row0(&mut m, 9);
    let texels = 0x8000_0000 | TEX_DATA;
    let e = d
        .drain_until_callback(&mut DenyWrites {
            inner: &mut m,
            from: texels,
            len: 256,
        })
        .unwrap_err();
    assert!(matches!(e, D3dError::Guest(_)), "{e}");
    assert_eq!(d.fault(), None, "not latched, like other write-backs");
    // Direct draining cannot deliver it, and does not skip past it.
    assert!(d.commit(&mut m).is_err());
    assert_eq!(d.stipple_pattern()[0], 0);
    // Memory works again: the same callback, written back first, and the
    // work after it still waits.
    let cb = d.drain_until_callback(&mut m).unwrap().unwrap();
    assert_eq!((cb.function, cb.context), (0x003F_0100, 0x1234));
    assert_eq!(m.read_u32(texels).unwrap(), 0x5A5A_5A5A);
    assert_eq!(d.stipple_pattern()[0], 0);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    assert_eq!(d.stipple_pattern()[0], 9);
    assert_eq!(d.stats().push_callbacks, 1);
}

#[test]
fn a_callback_outside_the_glue_stops_and_latches() {
    let (mut d, mut m) = rec();
    insert_callback(&mut m, 0x0002_4970, 0, 7);
    let e = d.commit(&mut m).unwrap_err();
    assert!(e.to_string().contains("runtime glue"), "{e}");
    assert!(matches!(d.commit(&mut m), Err(D3dError::Faulted(_))));
}

#[test]
fn other_software_methods() {
    // 11 does nothing in the original either (0x3ff366).
    let (mut d, mut m) = rec();
    push_words(&mut m, &[header(method::NOP, 1), 11]);
    d.commit(&mut m).unwrap();
    // 9 (a register write) only comes from D3D's own code: stop, named.
    push_words(&mut m, &[header(method::NOP, 1), 9]);
    let e = d.commit(&mut m).unwrap_err();
    assert!(e.to_string().contains("software method 9"), "{e}");
    // A callback with no function loaded.
    let (mut d, mut m) = rec();
    push_words(&mut m, &[header(method::NOP, 1), 7]);
    assert!(matches!(
        d.drain_until_callback(&mut m),
        Err(D3dError::Invalid { .. })
    ));
}

#[test]
fn clear_values_and_the_vertex_cache_break_are_accepted() {
    let (mut d, mut m) = rec();
    // 0x43a1a: BREAK_VERTEX_BUFFER_CACHE, 0. The 0x1D90 placeholder of the
    // unused encode slots is SET_COLOR_CLEAR_VALUE.
    push_words(
        &mut m,
        &[header(0x1710, 1), 0, header(0x1D90, 1), 0x00AB_CDEF],
    );
    d.commit(&mut m).unwrap();
    assert_eq!(d.hw().color_clear_value, 0x00AB_CDEF);
}

// SetVertexShaderConstantNotInline -------------------------------------------

#[test]
fn not_inline_constants_update_the_shadow_unless_flag_0x10() {
    let (mut d, mut m) = rec();
    let src = MISC + 0x4000;
    for i in 0..20u32 {
        m.write_u32(src + 4 * i, (i as f32).to_bits()).unwrap();
    }
    // 0x40d01: EBX = 0xB1 (register 81 + 96), EAX = 20 dwords, EDX = data.
    let regs = Regs {
        eax: 20,
        ebx: 0xB1,
        edx: src,
        ..Regs::default()
    };
    common::call(&mut d, &mut m, 0x3f7410, regs, &[]).unwrap();
    let shadow = L.vs_constants + 16 * 0xB1;
    assert_eq!(m.read_u32(shadow + 4 * 19).unwrap(), 19f32.to_bits());
    assert_eq!(d.constant(0xB1 + 4)[3], 19.0);
    // Device flag 0x10: pushed, not shadowed.
    let flags = m.read_u32(DEV + f::FLAGS).unwrap();
    m.write_u32(DEV + f::FLAGS, flags | 0x10).unwrap();
    m.write_u32(src, 7f32.to_bits()).unwrap();
    common::call(&mut d, &mut m, 0x3f7410, regs, &[]).unwrap();
    assert_eq!(m.read_u32(shadow).unwrap(), 0);
    assert_eq!(d.constant(0xB1)[0], 7.0);
}

// Port I/O guards ------------------------------------------------------------

#[test]
fn port_io_functions_are_guarded() {
    let (mut d, mut m) = rec();
    // The ISR: never this device's interrupt; AL = FALSE, the rest of EAX
    // kept, ret 8.
    let o = common::call(
        &mut d,
        &mut m,
        0x3feac0,
        Regs {
            eax: 0x1234_56FF,
            ..Regs::default()
        },
        &[0, 0],
    )
    .unwrap();
    assert_eq!(o.eax, Some(0x1234_5600));
    // The hardware bring-up and the vblank service stop, named.
    for (addr, name) in [
        (0x3fe005, "hardware init"),
        (0x3fed90, "vertical-blank service"),
    ] {
        let entries = d3d::abi::entries::<RecordingBackend>();
        let e = entries.iter().find(|e| e.addr == addr).unwrap();
        let err = d3d::abi::call(e, &mut d, &mut m, &Regs::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains(name) && err.contains("0x80C0"), "{err}");
        assert!(d3d::abi::stops(addr));
    }
    assert!(!d3d::abi::stops(0x3feac0));
}

// Push-buffer callbacks through the runtime -------------------------------------

mod through_the_runtime {
    use super::*;
    use d3d::abi::SharedDevice;
    use d3d::{DeviceConfig, PresentParameters, runtime_glue};
    use runtime::exec::HARNESS_EXIT;
    use runtime::guest::Permissions;
    use runtime::{
        CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi as rt,
    };

    const STACK: u32 = MISC + 0xF000;
    /// A stand-in for Halo's callback `0x24970`: logs its argument and the
    /// stipple row the GPU state shows at that moment.
    const CALLBACK: u32 = 0x003F_0100;
    const LOG: u32 = MISC + 0x8000;
    /// The first texel of the render-to-texture level, as each callback saw it.
    const TEXLOG: u32 = MISC + 0x9000;
    /// Texel memory of the level (`Data` = `TEX_DATA`).
    const TEXELS: u32 = 0x8000_0000 | TEX_DATA;
    /// A callback with this context calls `CDevice_KickOff` from inside
    /// itself, then logs `NESTED_END`.
    const NESTED: u32 = 0xC;
    const NESTED_END: u32 = 0xCC;

    fn log_entry(
        m: &mut runtime::Guest,
        context: u32,
        row: u32,
    ) -> Result<(), runtime::GuestFault> {
        let n = m.u32(LOG)?;
        m.set_u32(LOG + 4 + 8 * n, context)?;
        m.set_u32(LOG + 8 + 8 * n, row)?;
        let texel = m.u32(TEXELS)?;
        m.set_u32(TEXLOG + 4 * n, texel)?;
        m.set_u32(LOG, n + 1)
    }

    fn guest(cfg: &DeviceConfig) -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x003F_0000, 0x1_4000, Permissions::RX)
            .unwrap();
        g.map_zeroed(0x0040_4000, 0x5000, Permissions::RW).unwrap();
        let encode = L.simple_encode;
        g.map_zeroed(encode & !0xFFF, 0x1000, Permissions::RW)
            .unwrap();
        g.write_bytes(encode, &synthetic_encode_bytes(), 1).unwrap();
        g.map_zeroed(MISC, 0x1_0000, Permissions::RW).unwrap();
        g.map_zeroed(TEXELS, 0x1000, Permissions::RW).unwrap();
        runtime_glue::map_device_memory(&mut g, cfg).unwrap();
        let mut params = SparseMemory::new();
        params.map(PARAMS, 0x100);
        write_params(&mut params);
        let mut bytes = vec![0; PresentParameters::SIZE as usize];
        params.read(PARAMS, &mut bytes).unwrap();
        g.write_bytes(PARAMS, &bytes, 1).unwrap();
        g
    }

    struct Harness {
        ctx: ExecCtx,
        device: SharedDevice<RecordingBackend>,
    }

    fn harness() -> Harness {
        let cfg = DeviceConfig::halo2(PB, SCRATCH);
        let g = guest(&cfg);
        let device = runtime_glue::shared(cfg, RecordingBackend::new());
        let mut dispatch = Dispatch::default();
        d3d::register(&mut dispatch, device.clone());
        let dev = device.clone();
        let _ = dispatch.register_override(CALLBACK, "test push callback", move |cpu, ctx, _| {
            let row = dev.lock().unwrap().stipple_pattern()[0];
            let runtime = std::sync::Arc::clone(ctx.runtime());
            let mut m = runtime.memory();
            let context = m.u32(cpu.esp + 4)?;
            log_entry(&mut m, context, row)?;
            // cdecl may clobber anything the caller does not keep; the
            // glue restores the entry's registers.
            cpu.eax = 0xDEAD_0001;
            cpu.edx = 0xDEAD_0003;
            cpu.esi = 0xDEAD_0005;
            if context == NESTED {
                // A D3D call from inside the callback.
                let dev = dev.clone();
                return ctx.call_guest(cpu, &mut m, 0x3fac30, move |cpu, ctx| {
                    let row = dev.lock().unwrap().stipple_pattern()[0];
                    let mut m = ctx.memory();
                    log_entry(&mut m, NESTED_END, row)?;
                    m.ret(cpu, 0)
                });
            }
            m.ret(cpu, 0)
        });
        Harness {
            ctx: ExecCtx::new(Runtime::new(g, dispatch), 1),
            device,
        }
    }

    impl Harness {
        fn create(&mut self) {
            let cpu = CpuAbi {
                eax: 0x40,
                ecx: PP_DEVICE,
                ..CpuAbi::default()
            };
            assert_eq!(self.ok(0x3f5240, cpu, &[PARAMS]).eax, 0);
        }

        fn run(&mut self, entry: u32, mut cpu: CpuAbi, stack: &[u32]) -> (TransferAbi, CpuAbi) {
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

        fn texels(&mut self) -> Vec<u32> {
            let n = self.ctx.memory().u32(LOG).unwrap();
            (0..n)
                .map(|i| self.ctx.memory().u32(TEXLOG + 4 * i).unwrap())
                .collect()
        }

        fn ok(&mut self, entry: u32, mut cpu: CpuAbi, stack: &[u32]) -> CpuAbi {
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
            assert_eq!(t.kind, rt::STOP, "{entry:#x}: {t:?}");
            assert_eq!(
                self.ctx.event(t.event_id),
                Some(&Event::HarnessExit),
                "{entry:#x}: {:?}",
                self.device.lock().unwrap().last_stop()
            );
            assert_eq!(cpu.esp, STACK, "{entry:#x} balances the stack");
            cpu
        }

        fn push(&mut self, words: &[u32]) {
            let mut m = self.ctx.memory();
            let mut put = m.u32(DEV).unwrap();
            for w in words {
                m.set_u32(put, *w).unwrap();
                put += 4;
            }
            m.set_u32(DEV, put).unwrap();
        }

        fn callback(&mut self, context: u32) {
            self.push(&[
                header(0x1D8C, 2),
                CALLBACK,
                context,
                header(method::WAIT_FOR_IDLE, 1),
                0,
                header(method::NOP, 1),
                7,
            ]);
        }

        fn log(&mut self) -> Vec<(u32, u32)> {
            let m = self.ctx.memory();
            let n = m.u32(LOG).unwrap();
            (0..n)
                .map(|i| {
                    (
                        m.u32(LOG + 4 + 8 * i).unwrap(),
                        m.u32(LOG + 8 + 8 * i).unwrap(),
                    )
                })
                .collect()
        }
    }

    #[test]
    fn callbacks_run_in_order_between_the_work_around_them() {
        let mut h = harness();
        let cpu = CpuAbi {
            eax: 0x40,
            ecx: PP_DEVICE,
            ..CpuAbi::default()
        };
        assert_eq!(h.ok(0x3f5240, cpu, &[PARAMS]).eax, 0);
        // Row 0 = 1, callback A, row 0 = 2, callback B, row 0 = 3.
        h.push(&[header(0x1480, 1), 1]);
        h.callback(0xA);
        h.push(&[header(0x1480, 1), 2]);
        h.callback(0xB);
        h.push(&[header(0x1480, 1), 3]);
        // CDevice_KickOff: EDX is live across the call in its callers.
        let cpu = CpuAbi {
            edx: 0x5151,
            esi: 0x3333,
            ebx: 0x1111,
            ..CpuAbi::default()
        };
        let out = h.ok(0x3fac30, cpu, &[]);
        assert_eq!(h.log(), vec![(0xA, 1), (0xB, 2)]);
        assert_eq!((out.edx, out.esi, out.ebx), (0x5151, 0x3333, 0x1111));
        let d = h.device.lock().unwrap();
        assert_eq!(d.stipple_pattern()[0], 3, "the entry ran after them");
        assert_eq!(d.stats().push_callbacks, 2);
    }

    /// Review P1: a callback reads what the work before it rendered. The
    /// level of a render-to-texture target is drawn into (the recording
    /// backend reads back 0x5A); the callback queued after it must find
    /// those texels in guest memory, not the stale 0x11.
    #[test]
    fn a_callback_sees_render_targets_written_back() {
        let mut h = harness();
        h.create();
        {
            let runtime = std::sync::Arc::clone(h.ctx.runtime());
            let mut guest = runtime.memory();
            let mut m = runtime_glue::GuestAdapter::new(&mut guest);
            // A 16x4 LIN_A8R8G8B8 texture and its level 0 as a target.
            m.write_u32(TEX_HEADER, 1 | (4 << 16) | 0x0100_0000)
                .unwrap();
            m.write_u32(TEX_HEADER + 4, TEX_DATA).unwrap();
            m.write_u32(TEX_HEADER + 12, (2 << 4) | (0x12 << 8) | (1 << 16))
                .unwrap();
            m.write_u32(TEX_HEADER + 16, 15 | (3 << 12)).unwrap();
            m.write(TEXELS, &[0x11; 256]).unwrap();
            let surf = MISC + 0x7200;
            let mut l = d3d::resource::SurfaceHeader::linear(
                gfx::D3DFormat::LinA8R8G8B8,
                16,
                4,
                64,
                TEX_DATA,
            );
            l.parent = TEX_HEADER;
            l.write(&mut m, surf).unwrap();
            let mut d = h.device.lock().unwrap();
            d.backend.readback_fill = 0x5A;
            d.set_render_target(&mut m, surf, 0).unwrap();
            d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
        }
        h.callback(0xD);
        h.ok(0x3fac30, CpuAbi::default(), &[]);
        assert_eq!(h.log().len(), 1);
        assert_eq!(h.texels(), vec![0x5A5A_5A5A]);
    }

    /// Review P2: a D3D entry called from inside a callback must not drain
    /// the work queued after that callback (nor deliver the next one). It
    /// stops with a named error; nothing after the boundary has run.
    #[test]
    fn a_d3d_call_from_a_callback_stops_without_draining_past_it() {
        let mut h = harness();
        h.create();
        h.push(&[header(0x1480, 1), 1]);
        h.callback(NESTED);
        h.push(&[header(0x1480, 1), 2]);
        h.callback(0xB);
        let (t, _) = h.run(0x3fac30, CpuAbi::default(), &[]);
        assert_eq!(t.kind, rt::STOP);
        match h.ctx.event(t.event_id) {
            Some(Event::Unsupported { pc, detail, .. }) => {
                assert_eq!(*pc, 0x3fac30);
                assert!(
                    detail.contains("inside the push-buffer callback"),
                    "{detail}"
                );
            }
            other => panic!("{other:?}"),
        }
        // A started; B never ran, and neither did A's nested KickOff.
        assert_eq!(h.log(), vec![(NESTED, 1)]);
        let d = h.device.lock().unwrap();
        assert_eq!(d.stipple_pattern()[0], 1, "nothing after A was drained");
        assert_eq!(d.stats().push_callbacks, 1);
        assert!(d.active_push_callback().is_some());
    }
}

// The retail image ------------------------------------------------------------

/// Decodes the image in `$H2_XBE` linearly from `from` to `to` and returns
/// the instructions (`to` must land on an instruction boundary).
fn decode(mem: &dyn GuestMemory, from: u32, to: u32) -> Vec<iced_x86::Instruction> {
    let mut code = vec![0u8; (to - from + 16) as usize];
    mem.read(from, &mut code).unwrap();
    let mut dec =
        iced_x86::Decoder::with_ip(32, &code, from as u64, iced_x86::DecoderOptions::NONE);
    let mut out = Vec::new();
    while dec.ip() <= to as u64 {
        out.push(dec.decode());
    }
    assert_eq!(
        out.last().unwrap().ip(),
        to as u64,
        "{to:#x} is not on a boundary from {from:#x}"
    );
    out
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn round5_addresses_match_the_retail_image() {
    use iced_x86::{Code, Mnemonic, OpKind, Register};
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    let image = runtime::load_xbe(&std::fs::read(path).unwrap()).unwrap();
    let mut guest = image.runtime.memory();
    let mem = d3d::runtime_glue::GuestAdapter::new(&mut guest);

    // Each port instruction sits in the function the guard overrides, right
    // after `mov dx, 80C0h`.
    for (func, site, mnemonic) in [
        (0x3fe005, 0x3fe131, Mnemonic::Out),
        (0x3feac0, 0x3feb3c, Mnemonic::In),
        (0x3fed90, 0x3fee0d, Mnemonic::In),
    ] {
        let insns = decode(&mem, func, site);
        let port = insns.last().unwrap();
        assert_eq!(port.mnemonic(), mnemonic, "{site:#x}");
        let dx = insns
            .iter()
            .rev()
            .find(|i| i.mnemonic() == Mnemonic::Mov && i.op0_register() == Register::DX)
            .expect("mov dx");
        assert_eq!(dx.immediate16(), 0x80C0, "{site:#x}");
        assert!(d3d::abi::PORT_IO.iter().any(|p| p.0 == func));
    }

    // 0x3fb710: the fence test reads Lock [ecx+8], the GPU time [edi+2Ch]
    // and the completion pointer [edi+30h], and clears Lock at 0x3fb78d.
    let busy = decode(&mem, 0x3fb710, 0x3fb78d);
    let mem_reads = |disp: u64, base: Register| {
        busy.iter().any(|i| {
            (0..i.op_count()).any(|k| i.op_kind(k) == OpKind::Memory)
                && i.memory_base() == base
                && i.memory_displacement64() == disp
        })
    };
    assert!(
        mem_reads(8, Register::ECX)
            && mem_reads(0x2C, Register::EDI)
            && mem_reads(0x30, Register::EDI)
    );
    let clear = busy.last().unwrap();
    assert_eq!(
        (clear.mnemonic(), clear.memory_displacement64()),
        (Mnemonic::Mov, 8)
    );

    // 0x3f8cc0 pushes header 0x801480 (SET_STIPPLE_PATTERN, 32 words).
    let stipple = decode(&mem, 0x3f8cc0, 0x3f8cf9);
    assert_eq!(stipple.last().unwrap().immediate32(), 0x0080_1480);

    // 0x3f7410 tests device flag 0x10 at 0x404FE8 and shadows to 0x407738.
    let ni = decode(&mem, 0x3f7410, 0x3f7420);
    assert_eq!(ni[0].memory_displacement64(), (DEV + f::FLAGS) as u64);
    assert_eq!(ni.last().unwrap().immediate32(), L.vs_constants);

    // The inlined InsertCallback at 0x2460e: ZSTENCIL/COLOR_CLEAR_VALUE,
    // then NO_OPERATION 7 at +0x18.
    let site = decode(&mem, 0x2460e, 0x24633);
    assert_eq!(site[0].immediate32(), header(0x1D8C, 2));
    assert_eq!(site.last().unwrap().immediate32(), 7);
    assert!(
        site.iter()
            .any(|i| i.code() == Code::Mov_rm32_imm32 && i.immediate32() == header(method::NOP, 1))
    );

    // Software methods 6 and 7 share D3D's callback handler (0x3ff454 is
    // the table for codes 1..14).
    let six = mem.read_u32(0x3ff454 + 4 * 5).unwrap();
    let seven = mem.read_u32(0x3ff454 + 4 * 6).unwrap();
    assert_eq!(six, seven);
    let handler = decode(&mem, six, six + 0xF);
    assert!(
        handler
            .iter()
            .any(|i| i.mnemonic() == Mnemonic::Call && i.op0_register() == Register::EAX)
    );
}
