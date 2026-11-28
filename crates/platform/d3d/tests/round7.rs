//! D3D round 7: visibility tests (`BeginVisibilityTest` inline at `0x20cec`,
//! `EndVisibilityTest` `0x3f85d0`, `GetVisibilityTestResult` `0x3f8630`),
//! resource destruction (`D3DResource_Release` `0x3fb640` as compiled, the
//! internal-reference releases, and the native `D3D_DestroyResource`
//! `0x3fb520` the glue calls), and the miniport gamma-ramp load `0x3ff5c6`.
//! Synthetic values only; the ignored tests at the end decode the retail
//! image in `$H2_XBE`.

use std::sync::{Arc, Mutex};

use crate::common::{self, *};
use d3d::abi::{Regs, SharedDevice};
use d3d::backend::Recorded;
use d3d::hw::method as hw;
use d3d::layout::device_field as f;
use d3d::resource::common::INTREFCOUNT_ONE as ONE;
use d3d::runtime_glue;
use d3d::{
    D3dError, Device, DeviceConfig, GuestMemory, PresentParameters, RecordingBackend,
    SparseMemory, UploadPolicy,
};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi as rt};

const DEV: u32 = 0x0040_4FE0;
const END: u32 = 0x3f85d0;
const GET: u32 = 0x3f8630;
const BLOCK_ON_RESOURCE: u32 = 0x3fad50;
const OUT: u32 = MISC + 0x8000;
const STAMP: u32 = MISC + 0x8010;

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

/// What `0x20cec` pushes: `CLEAR_REPORT_VALUE` 1 and
/// `SET_ZPASS_PIXEL_COUNT_ENABLE` 1 in one command (`0x817C8`, then 1, 1).
fn begin_inline(m: &mut SparseMemory) {
    assert_eq!(header(hw::CLEAR_REPORT_VALUE, 2), 0x0008_17C8);
    push_words(m, &[header(hw::CLEAR_REPORT_VALUE, 2), 1, 1]);
}

fn draw_quad<B: d3d::Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    call(
        d,
        m,
        0x3faed0,
        Regs {
            eax: 4,
            ecx: 0,
            ..Regs::default()
        },
        &[TRIANGLESTRIP],
    )
    .unwrap();
}

fn end<B: d3d::Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    index: u32,
) -> Result<(), D3dError> {
    let out = call(
        d,
        m,
        END,
        Regs {
            eax: index,
            ..Regs::default()
        },
        &[],
    )?;
    assert_eq!(out.eax, Some(0));
    Ok(())
}

/// `GetVisibilityTestResult(index, &stamp, &count)` once: the count and
/// stamp, or `None` for `D3DERR_TESTINCOMPLETE`, which writes nothing.
fn get_once<B: d3d::Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    index: u32,
) -> Result<Option<(u32, u64)>, D3dError> {
    m.write_u32(OUT, 0xAAAA_AAAA).unwrap();
    m.write_u32(STAMP, 0xAAAA_AAAA).unwrap();
    m.write_u32(STAMP + 4, 0xAAAA_AAAA).unwrap();
    let r = Regs {
        eax: index,
        edx: STAMP,
        ..Regs::default()
    };
    let out = call(d, m, GET, r, &[OUT])?;
    if out.eax == Some(d3d::device::D3DERR_TESTINCOMPLETE) {
        for a in [OUT, STAMP, STAMP + 4] {
            assert_eq!(
                m.read_u32(a).unwrap(),
                0xAAAA_AAAA,
                "incomplete writes nothing"
            );
        }
        return Ok(None);
    }
    assert_eq!(out.eax, Some(0));
    let stamp = m.read_u32(STAMP).unwrap() as u64 | (m.read_u32(STAMP + 4).unwrap() as u64) << 32;
    Ok(Some((m.read_u32(OUT).unwrap(), stamp)))
}

/// [`get_once`] until it answers (a GPU or render thread may not be done).
fn get<B: d3d::Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    index: u32,
) -> Result<(u32, u64), D3dError> {
    loop {
        if let Some(a) = get_once(d, m, index)? {
            return Ok(a);
        }
        std::thread::yield_now();
    }
}

// Visibility tests ---------------------------------------------------------------

#[test]
fn the_timestamp_pointer_is_optional() {
    let (mut d, mut m) = rec();
    begin_inline(&mut m);
    end(&mut d, &mut m, 1).unwrap();
    m.write_u32(STAMP, 0xAAAA_AAAA).unwrap();
    let out = call(
        &mut d,
        &mut m,
        GET,
        Regs {
            eax: 1,
            ..Regs::default()
        },
        &[OUT],
    )
    .unwrap();
    assert_eq!(out.eax, Some(0));
    assert_eq!(m.read_u32(OUT).unwrap(), 0);
    assert_eq!(m.read_u32(STAMP).unwrap(), 0xAAAA_AAAA);
}

#[test]
fn unknown_and_out_of_range_indices_stop() {
    let (mut d, mut m) = rec();
    let err = get(&mut d, &mut m, 3).unwrap_err().to_string();
    assert!(err.contains("never ended"), "{err}");
    let err = end(&mut d, &mut m, d3d::device::VISIBILITY_INDICES)
        .unwrap_err()
        .to_string();
    assert!(err.contains("64 report pages"), "{err}");
    let err = get(&mut d, &mut m, 0x4000).unwrap_err().to_string();
    assert!(err.contains("64 report pages"), "{err}");
}

#[test]
fn clear_report_value_of_another_type_stops() {
    let (mut d, mut m) = rec();
    push_words(&mut m, &[header(hw::CLEAR_REPORT_VALUE, 1), 2]);
    let err = d.kick_off(&mut m).unwrap_err().to_string();
    assert!(err.contains("CLEAR_REPORT_VALUE type 2"), "{err}");
    // GET_REPORT only comes from the overridden End: unknown.
    let (mut d, mut m) = rec();
    push_words(&mut m, &[header(hw::GET_REPORT, 1), 0x0100_0000]);
    assert!(matches!(
        d.kick_off(&mut m),
        Err(D3dError::Push(d3d::pushbuf::PushError::UnknownMethod {
            method: 0x17D0,
            ..
        }))
    ));
}

#[test]
fn a_backend_that_cannot_count_stops_at_end() {
    /// Records, but has no pixel counter (the trait's defaults).
    struct NoCount(RecordingBackend);
    impl d3d::Backend for NoCount {
        type Surface = u32;
        type Texture = u32;
        fn create_surface(
            &mut self,
            f: gfx::D3DFormat,
            w: u32,
            h: u32,
            depth: bool,
        ) -> Result<u32, String> {
            self.0.create_surface(f, w, h, depth)
        }
        fn destroy_surface(&mut self, s: u32) -> Result<(), String> {
            self.0.destroy_surface(s)
        }
        fn upload_texture(&mut self, t: &gfx::texture::DecodedTexture) -> Result<u32, String> {
            self.0.upload_texture(t)
        }
        fn bc_supported(&self) -> bool {
            false
        }
        fn clear(&mut self, c: u32, z: Option<u32>, clear: gfx::Clear) -> Result<(), String> {
            self.0.clear(c, z, clear)
        }
        fn draw(
            &mut self,
            c: u32,
            z: Option<u32>,
            d: &d3d::ResolvedDraw,
            t: [Option<&u32>; 4],
        ) -> Result<d3d::DrawOutcome, String> {
            self.0.draw(c, z, d, t)
        }
        fn present(&mut self, s: u32, flags: u32) -> Result<(), String> {
            self.0.present(s, flags)
        }
    }
    let (mut d, mut m) = create(NoCount(RecordingBackend::new()), UploadPolicy::Tracked);
    begin_inline(&mut m);
    let err = end(&mut d, &mut m, 0).unwrap_err().to_string();
    assert!(err.contains("cannot count pixels"), "{err}");
}

#[test]
fn teardown_forgets_visibility_tests() {
    let (mut d, mut m) = rec();
    begin_inline(&mut m);
    end(&mut d, &mut m, 2).unwrap();
    d.release_device(&mut m, DEV).unwrap();
    assert!(d.backend.pixel_reports.is_empty());
    let regs = Regs {
        eax: 0x40,
        ecx: PP_DEVICE,
        ..Regs::default()
    };
    call(&mut d, &mut m, 0x3f5240, regs, &[PARAMS]).unwrap();
    assert!(!d.hw().zpass_count_enable);
    let err = get(&mut d, &mut m, 2).unwrap_err().to_string();
    assert!(err.contains("never ended"), "{err}");
}

/// The original `D3D_DestroyResource` waits on what it frees
/// (`BlockOnResource`), and a teardown's destroys run after the teardown
/// returned. Each resource the teardown destroyed may be waited on once
/// with no device; any other wait with no device still stops.
#[test]
fn a_teardown_destroy_may_wait_on_its_resource() {
    let (mut d, mut m) = rec();
    // Two vertex buffers held only by their bindings.
    for (i, hdr) in [VB_HEADER, VB_HEADER + 0x10].into_iter().enumerate() {
        m.write_u32(hdr, 0x0100_0001).unwrap();
        m.write_u32(hdr + 4, VB_DATA).unwrap();
        d.set_stream_source(&mut m, i as u32, hdr, 20).unwrap();
        let c = m.read_u32(hdr).unwrap();
        m.write_u32(hdr, c - 1).unwrap();
    }
    // Live, the wait is a kick-off.
    let wait = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory, r: u32| {
        let regs = Regs {
            eax: r,
            ..Regs::default()
        };
        call(d, m, BLOCK_ON_RESOURCE, regs, &[])
    };
    wait(&mut d, &mut m, VB_HEADER).unwrap();
    d.release_device(&mut m, DEV).unwrap();
    assert_eq!(d.take_pending_destroys(), [VB_HEADER, VB_HEADER + 0x10]);
    wait(&mut d, &mut m, VB_HEADER).unwrap();
    for r in [VB_HEADER, VB_HEADER + 0x20] {
        let err = wait(&mut d, &mut m, r).unwrap_err();
        assert!(matches!(err, D3dError::NoDevice(_)), "{r:#x}: {err}");
    }
    // A new device forgets what an old teardown destroyed (VB_HEADER + 0x10
    // was never waited on): after its own, empty, teardown that wait stops.
    let regs = Regs {
        eax: 0x40,
        ecx: PP_DEVICE,
        ..Regs::default()
    };
    call(&mut d, &mut m, 0x3f5240, regs, &[PARAMS]).unwrap();
    d.release_device(&mut m, DEV).unwrap();
    assert!(d.take_pending_destroys().is_empty());
    let err = wait(&mut d, &mut m, VB_HEADER + 0x10).unwrap_err();
    assert!(matches!(err, D3dError::NoDevice(_)), "{err}");
}

// Resource destruction --------------------------------------------------------------

/// A level of the test texture whose header the allocator `0x3fb7a0` would
/// make: refcount 1, and a reference on the parent.
fn level(m: &mut SparseMemory) -> u32 {
    texture(m, 0);
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(m, surf).unwrap();
    let p = m.read_u32(TEX_HEADER).unwrap();
    m.write_u32(TEX_HEADER, p + 1).unwrap();
    surf
}

fn release(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, h: u32) -> u32 {
    call(d, m, 0x3fb640, Regs::default(), &[h])
        .unwrap()
        .eax
        .unwrap()
}

#[test]
fn releasing_a_level_releases_its_parent_and_destroys_both() {
    let (mut d, mut m) = rec();
    let surf = level(&mut m);
    let p = m.read_u32(TEX_HEADER).unwrap();
    assert_eq!(p & 0xFFFF, 2, "the title's and the level's");
    // The title's own reference to the texture.
    assert_eq!(release(&mut d, &mut m, TEX_HEADER), 1);
    assert!(d.take_pending_destroys().is_empty());
    // The level's last reference: the parent goes first, then the level.
    assert_eq!(release(&mut d, &mut m, surf), 0);
    assert_eq!(d.take_pending_destroys(), vec![TEX_HEADER, surf]);
    assert_eq!(d.stats().destroys, 2);
}

#[test]
fn add_ref_of_an_unreferenced_level_references_the_parent() {
    let (mut d, mut m) = rec();
    let surf = level(&mut m);
    assert_eq!(release(&mut d, &mut m, TEX_HEADER), 1);
    // Bound, so the release keeps it: refcount 0, one internal reference.
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    assert_eq!(release(&mut d, &mut m, surf), 0);
    assert!(d.take_pending_destroys().is_empty());
    let p = m.read_u32(TEX_HEADER).unwrap();
    let n = call(&mut d, &mut m, 0x3fb600, Regs::default(), &[surf])
        .unwrap()
        .eax;
    assert_eq!(n, Some(1));
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p + 1, "0x3fb622");
    // Already referenced: the parent is left alone.
    call(&mut d, &mut m, 0x3fb600, Regs::default(), &[surf]).unwrap();
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p + 1);
}

#[test]
fn a_release_with_internal_references_keeps_the_resource() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0x40);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    let c = m.read_u32(TEX_HEADER).unwrap();
    assert_eq!(release(&mut d, &mut m, TEX_HEADER), 0);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), c - 1);
    assert!(d.take_pending_destroys().is_empty());
    // Unbinding it then destroys it, without the decrement.
    d.set_texture(&mut m, 0, 0).unwrap();
    assert_eq!(d.take_pending_destroys(), vec![TEX_HEADER]);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), c - 1);
}

#[test]
fn destroying_a_texture_drops_its_cached_upload() {
    let (mut d, mut m) = rec();
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, 16.0, 16.0, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    texture(&mut m, 0x40);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    draw_quad(&mut d, &mut m);
    draw_quad(&mut d, &mut m);
    assert_eq!(d.backend.uploads(), 1);
    release(&mut d, &mut m, TEX_HEADER);
    d.set_texture(&mut m, 0, 0).unwrap();
    assert_eq!(d.take_pending_destroys(), vec![TEX_HEADER]);
    // A new texture at the same header address uploads again.
    texture(&mut m, 0x40);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    draw_quad(&mut d, &mut m);
    assert_eq!(d.backend.uploads(), 2);
}

#[test]
fn releasing_an_unreferenced_resource_stops() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    m.write_u32(TEX_HEADER, m.read_u32(TEX_HEADER).unwrap() & !0xFFFF)
        .unwrap();
    let err = call(&mut d, &mut m, 0x3fb640, Regs::default(), &[TEX_HEADER])
        .unwrap_err()
        .to_string();
    assert!(err.contains("no reference to release"), "{err}");
}

#[test]
fn a_dirty_level_is_written_back_before_it_is_destroyed() {
    let (mut d, mut m) = rec();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let surf = level(&mut m);
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    assert_eq!(release(&mut d, &mut m, TEX_HEADER), 1);
    assert_eq!(release(&mut d, &mut m, surf), 0);
    assert_eq!(d.stats().rtt_writebacks, 0, "still bound");
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[back]).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 1);
    assert_eq!(d.take_pending_destroys(), vec![TEX_HEADER, surf]);
    let s = m.read_u32(surf).unwrap();
    assert_eq!(s & 0xFFFF, 0);
    assert_eq!(s & ONE, ONE, "destroyed as it was: no decrement");
}

// Gamma ramp --------------------------------------------------------------------

#[test]
fn the_gamma_ramp_is_kept() {
    let (mut d, mut m) = rec();
    let ramp: Vec<u8> = (0..768u32).map(|i| (i * 7) as u8).collect();
    m.write(MISC + 0x9000, &ramp).unwrap();
    let r = Regs {
        eax: DEV + d3d::device::MINIPORT,
        edx: MISC + 0x9000,
        ..Regs::default()
    };
    let out = call(&mut d, &mut m, 0x3ff5c6, r, &[]).unwrap();
    assert_eq!(out.eax, None);
    assert_eq!(&d.gamma_ramp().unwrap()[..], &ramp[..]);
    d.swap(&mut m, 0).unwrap();
    assert_eq!(&d.backend.present_gamma.as_ref().unwrap()[..], &ramp[..]);
    let r = Regs { eax: DEV, ..r };
    let err = call(&mut d, &mut m, 0x3ff5c6, r, &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("device + 0x1C28"), "{err}");
}

/// The frame step's publish (docs/graphics/retire-d3d-display.md, "The
/// final display"): the back buffer with the frame step's ramp, not the
/// device's, and a counted swap the next vertical blank reports.
#[test]
fn a_published_frame_presents_with_the_frame_steps_ramp_and_the_next_vblank_reports_its_swap() {
    let (mut d, mut m) = rec();
    let loaded: Vec<u8> = (0..768u32).map(|i| (i * 7) as u8).collect();
    m.write(MISC + 0x9000, &loaded).unwrap();
    let r = Regs {
        eax: DEV + d3d::device::MINIPORT,
        edx: MISC + 0x9000,
        ..Regs::default()
    };
    call(&mut d, &mut m, 0x3ff5c6, r, &[]).unwrap();
    let ramp: [u8; 768] = std::array::from_fn(|i| (i * 5) as u8);
    let before = d.swap_count();
    let headers = (m.read_u32(DEV + f::RENDER_TARGET).unwrap(), m.read_u32(DEV + f::BACK_BUFFER).unwrap());
    assert_eq!(d.publish(&mut m, Some(&ramp)).unwrap(), before + 1);
    assert_eq!(&d.backend.present_gamma.as_ref().unwrap()[..], &ramp[..]);
    assert!(matches!(d.backend.calls.last(), Some(Recorded::Present { flags: 5, .. })));
    assert_eq!(m.read_u32(DEV + f::SWAP_COUNT).unwrap(), before + 1);
    assert_eq!(
        (m.read_u32(DEV + f::RENDER_TARGET).unwrap(), m.read_u32(DEV + f::BACK_BUFFER).unwrap()),
        headers,
        "no flip: the next frame draws into the same back buffer"
    );
    m.write_u32(DEV + f::VBLANK_CALLBACK, 0x003F_0100).unwrap();
    let v = d.vblank(&m).unwrap().unwrap();
    assert_eq!(v.record[1..], [before + 1, d3d::device::VBLANK_SWAPDONE]);
    assert_eq!(d.vblank(&m).unwrap().unwrap().record[2], 0, "SWAPDONE once");
}

// Through the runner ------------------------------------------------------------

mod runner {
    use super::*;

    const STACK: u32 = MISC + 0xF000;

    fn guest(cfg: &DeviceConfig) -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x003F_0000, 0x1_4000, Permissions::RX)
            .unwrap();
        g.map_zeroed(0x0040_4000, 0x5000, Permissions::RW).unwrap();
        let encode = d3d::GuestLayout::HALO2.simple_encode;
        g.map_zeroed(encode & !0xFFF, 0x1000, Permissions::RW)
            .unwrap();
        g.write_bytes(encode, &synthetic_encode_bytes(), 1).unwrap();
        g.map_zeroed(MISC, 0x1_0000, Permissions::RW).unwrap();
        runtime_glue::map_device_memory(&mut g, cfg).unwrap();
        let mut params = SparseMemory::new();
        params.map(PARAMS, 0x100);
        write_params(&mut params);
        let mut bytes = vec![0; PresentParameters::SIZE as usize];
        params.read(PARAMS, &mut bytes).unwrap();
        g.write_bytes(PARAMS, &bytes, 1).unwrap();
        g
    }

    pub struct Harness {
        pub ctx: ExecCtx,
        pub device: SharedDevice<RecordingBackend>,
        /// `(EDI, ESP, return address)` at each call of the stand-in for
        /// `D3D_DestroyResource`.
        pub destroyed: Arc<Mutex<Vec<(u32, u32, u32)>>>,
    }

    pub fn harness() -> Harness {
        let cfg = DeviceConfig::halo2(PB, SCRATCH);
        let g = guest(&cfg);
        let device = runtime_glue::shared(cfg, RecordingBackend::new());
        let mut dispatch = Dispatch::default();
        let reg = d3d::register(&mut dispatch, device.clone());
        assert!(
            reg.native
                .iter()
                .any(|n| n.0 == runtime_glue::DESTROY_RESOURCE)
        );
        // The original is native, so nothing is registered there: a Rust
        // stand-in records each call and, as the original does for every
        // resource but a title surface (0x3fb537..0x3fb539), first waits on
        // it through the overridden D3D_BlockOnResource (EAX = resource),
        // then clobbers what it may and returns.
        let destroyed = Arc::new(Mutex::new(Vec::new()));
        let log = destroyed.clone();
        let _ = dispatch.register_override(
            runtime_glue::DESTROY_RESOURCE,
            "test D3D_DestroyResource",
            move |cpu, ctx, _| {
                let runtime = Arc::clone(ctx.runtime());
                let mut m = runtime.memory();
                let ret = m.u32(cpu.esp)?;
                log.lock().unwrap().push((cpu.edi, cpu.esp, ret));
                cpu.eax = cpu.edi;
                ctx.call_guest(cpu, &mut m, BLOCK_ON_RESOURCE, |cpu, ctx| {
                    let runtime = Arc::clone(ctx.runtime());
                    let m = runtime.memory();
                    cpu.eax = 0xDEAD_0001;
                    cpu.ecx = 0xDEAD_0002;
                    cpu.edx = 0xDEAD_0003;
                    m.ret(cpu, 0)
                })
            },
        );
        Harness {
            ctx: ExecCtx::new(Runtime::new(g, dispatch), 1),
            device,
            destroyed,
        }
    }

    impl Harness {
        pub fn run(&mut self, entry: u32, mut cpu: CpuAbi, stack: &[u32]) -> (TransferAbi, CpuAbi) {
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

        pub fn ok(&mut self, entry: u32, cpu: CpuAbi, stack: &[u32]) -> CpuAbi {
            let (t, cpu) = self.run(entry, cpu, stack);
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

        pub fn create(&mut self) {
            let cpu = CpuAbi {
                eax: 0x40,
                ecx: PP_DEVICE,
                ..CpuAbi::default()
            };
            assert_eq!(self.ok(0x3f5240, cpu, &[PARAMS]).eax, 0);
        }

        pub fn u32(&mut self, addr: u32) -> u32 {
            self.ctx.memory().u32(addr).unwrap()
        }

        pub fn set(&mut self, addr: u32, v: u32) {
            self.ctx.memory().set_u32(addr, v).unwrap();
        }

        /// Stores words at put, as game code does.
        pub fn push(&mut self, words: &[u32]) {
            let mut put = self.u32(DEV);
            for w in words {
                self.set(put, *w);
                put += 4;
            }
            self.set(DEV, put);
        }
    }
}

#[test]
fn visibility_entries_through_the_runner() {
    let mut h = runner::harness();
    h.create();
    h.push(&[header(hw::CLEAR_REPORT_VALUE, 2), 1, 1]);
    h.set(DEV + f::GPU_TIME, 0x55);
    let cpu = CpuAbi {
        eax: 0x42,
        ebx: 0xB0B0,
        edx: 0xE0E0,
        ..CpuAbi::default()
    };
    let after = h.ok(END, cpu, &[]);
    assert_eq!(after.eax, 0);
    assert_eq!((after.ebx, after.edx), (0xB0B0, 0xE0E0));
    h.set(OUT, 0xAAAA_AAAA);
    let cpu = CpuAbi {
        eax: 0x42,
        edx: STAMP,
        ..CpuAbi::default()
    };
    let after = h.ok(GET, cpu, &[OUT]);
    assert_eq!(after.eax, 0);
    assert_eq!(h.u32(OUT), 0, "nothing was drawn");
    assert_eq!((h.u32(STAMP), h.u32(STAMP + 4)), (0x55, 0));
}

#[test]
fn destroys_reach_the_original_through_the_runner() {
    let mut h = runner::harness();
    h.create();
    // A title vertex buffer with one reference (type 0, D3D-created).
    h.set(VB_HEADER, 0x0100_0001);
    h.set(VB_HEADER + 4, VB_DATA);
    let cpu = CpuAbi {
        eax: 0x77,
        esi: 0x5151,
        edi: 0x7171,
        ..CpuAbi::default()
    };
    let after = h.ok(0x3fb640, cpu, &[VB_HEADER]);
    // Release's result and every other register as the entry left them,
    // whatever the original clobbered.
    assert_eq!(after.eax, 0);
    assert_eq!(
        (after.esi, after.edi, after.ecx, after.edx),
        (0x5151, 0x7171, 0, 0)
    );
    let calls = h.destroyed.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let (edi, esp, _) = calls[0];
    assert_eq!(edi, VB_HEADER);
    // Called from the caller's ESP (the entry's frame already popped), so
    // its return token sits just below it.
    assert_eq!(esp, runner_stack() - 4);
    assert!(h.device.lock().unwrap().take_pending_destroys().is_empty());

    // A release that keeps the resource calls nothing.
    h.set(VB_HEADER, 0x0100_0002);
    let after = h.ok(0x3fb640, CpuAbi::default(), &[VB_HEADER]);
    assert_eq!(after.eax, 1);
    assert_eq!(h.destroyed.lock().unwrap().len(), 1);
}

/// The stand-in waits on each resource through `BlockOnResource`, as the
/// original does, after the device is gone: the teardown's destroys still
/// finish.
#[test]
fn teardown_destroys_in_order_through_the_runner() {
    let mut h = runner::harness();
    h.create();
    // Two vertex buffers held only by SetStreamSource bindings (through
    // the device, as the API setter is not reached from game code).
    for (i, hdr) in [VB_HEADER, VB_HEADER + 0x10].into_iter().enumerate() {
        h.set(hdr, 0x0100_0001);
        h.set(hdr + 4, VB_DATA);
        let dev = h.device.clone();
        let mut d = dev.lock().unwrap();
        let mut guard = h.ctx.memory();
        let mut mem = runtime_glue::GuestAdapter::new(&mut guard);
        d.set_stream_source(&mut mem, i as u32, hdr, 20).unwrap();
        let c = mem.read_u32(hdr).unwrap();
        mem.write_u32(hdr, c - 1).unwrap(); // the title lets go
    }
    let after = h.ok(
        0x3fc530,
        CpuAbi {
            eax: DEV,
            ..CpuAbi::default()
        },
        &[],
    );
    assert_eq!(after.eax, DEV);
    let order: Vec<u32> = h.destroyed.lock().unwrap().iter().map(|c| c.0).collect();
    assert_eq!(order, [VB_HEADER, VB_HEADER + 0x10]);
}

fn runner_stack() -> u32 {
    MISC + 0xF000
}

// The retail image -----------------------------------------------------------------

mod image {
    use iced_x86::{Code, Decoder, DecoderOptions, Instruction, Mnemonic, OpKind};

    pub struct Image<'a> {
        pub xbe: xbe::Xbe<'a>,
    }

    impl<'a> Image<'a> {
        pub fn new(bytes: &'a [u8]) -> Self {
            Self {
                xbe: xbe::Xbe::parse(bytes).unwrap(),
            }
        }

        pub fn decode(&self, va: u32) -> Instruction {
            let s = self.xbe.section_containing(va).unwrap();
            let avail = (s.va + s.raw_size.min(s.vsize) - va) as usize;
            let bytes = self.xbe.bytes_at(va, avail.min(16)).unwrap();
            Decoder::with_ip(32, bytes, va as u64, DecoderOptions::NONE).decode()
        }

        /// Instructions from `va` up to and including the first `ret`.
        pub fn function(&self, va: u32) -> Vec<Instruction> {
            let mut out = Vec::new();
            let mut at = va;
            loop {
                let i = self.decode(at);
                at += i.len() as u32;
                let ret = i.mnemonic() == Mnemonic::Ret;
                out.push(i);
                if ret || out.len() > 400 {
                    return out;
                }
            }
        }

        /// Instructions in `[va, end)`, decoded linearly.
        pub fn range(&self, va: u32, end: u32) -> Vec<Instruction> {
            let mut out = Vec::new();
            let mut at = va;
            while at < end {
                let i = self.decode(at);
                at += i.len() as u32;
                out.push(i);
            }
            out
        }

        /// Every instruction of the section named, decoded linearly.
        pub fn section(&self, name: &str) -> Vec<Instruction> {
            let s = self.xbe.section(name).unwrap();
            let bytes = self
                .xbe
                .bytes_at(s.va, s.raw_size.min(s.vsize) as usize)
                .unwrap();
            let mut d = Decoder::with_ip(32, bytes, s.va as u64, DecoderOptions::NONE);
            let mut out = Vec::new();
            while d.can_decode() {
                out.push(d.decode());
            }
            out
        }
    }

    pub fn calls(i: &Instruction, target: u32) -> bool {
        i.mnemonic() == Mnemonic::Call
            && i.op0_kind() == OpKind::NearBranch32
            && i.near_branch32() == target
    }

    pub fn stores_imm(i: &Instruction, value: u32) -> bool {
        i.code() == Code::Mov_rm32_imm32
            && i.op0_kind() == OpKind::Memory
            && i.immediate32() == value
    }

    pub fn ret_pops(f: &[Instruction]) -> u32 {
        let r = f.last().unwrap();
        assert_eq!(r.mnemonic(), Mnemonic::Ret);
        if r.op_count() == 1 {
            r.immediate16() as u32
        } else {
            0
        }
    }
}

fn retail_xbe() -> Vec<u8> {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    std::fs::read(path).unwrap()
}

/// The visibility-test conventions the adapters assume, and the call sites
/// that rely on them.
#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn visibility_conventions_in_the_image() {
    use iced_x86::{Mnemonic, OpKind, Register};
    let bytes = retail_xbe();
    let img = image::Image::new(&bytes);
    let p_device = d3d::GuestLayout::HALO2.p_device;

    // EndVisibilityTest: index in EAX (handed to the slot helper 0x3f8570),
    // the device from D3D__pDevice, one command `0x817CC` (ZPASS enable 0,
    // GET_REPORT), plain `ret`.
    // Two exits (E_OUTOFMEMORY at 0x3f85e2, S_OK at 0x3f862a), both `ret`.
    let end = img.range(END, GET);
    assert!(image::calls(&end[1], 0x3f8570), "{:?}", end[1]);
    assert!(end.iter().any(|i| image::stores_imm(i, 0x0008_17CC)));
    assert!(end.iter().any(|i| i.mnemonic() == Mnemonic::Mov
        && i.op1_kind() == OpKind::Memory
        && i.memory_displacement32() == p_device));
    let rets: Vec<_> = end
        .iter()
        .filter(|i| i.mnemonic() == Mnemonic::Ret)
        .collect();
    assert_eq!(rets.len(), 2);
    assert!(rets.iter().all(|r| r.op_count() == 0));
    // The slot helper reads the index from EAX, pages at +0x7D4.
    let slot = img.function(0x3f8570);
    assert!(slot.iter().any(|i| i.memory_displacement32() == 0x7D4));

    // GetVisibilityTestResult: EAX split into page (>> 8) and slot (& 0xFF),
    // +0x7D4, D3DERR_TESTINCOMPLETE, the count through [esp+0Ch] (the
    // first stack argument after two pushes), EDX tested for the stamp,
    // `ret 4` (both exits).
    let get = img.function(GET);
    assert!(get.iter().any(|i| i.mnemonic() == Mnemonic::Shr
        && i.op0_register() == Register::ESI
        && i.immediate8() == 8));
    assert!(get.iter().any(|i| i.mnemonic() == Mnemonic::And
        && i.op0_register() == Register::EAX
        && i.immediate32() == 0xFF));
    assert!(get.iter().any(|i| i.memory_displacement32() == 0x7D4));
    assert!(
        get.iter().any(|i| i.mnemonic() == Mnemonic::Mov
            && i.immediate32() == d3d::device::D3DERR_TESTINCOMPLETE)
    );
    assert_eq!(image::ret_pops(&get), 4);
    let rest = img.function(get.last().unwrap().next_ip() as u32);
    assert!(
        rest.iter()
            .any(|i| i.mnemonic() == Mnemonic::Test && i.op0_register() == Register::EDX)
    );
    assert!(
        rest.iter()
            .any(|i| i.memory_base() == Register::ESP && i.memory_displacement32() == 0xC)
    );
    assert_eq!(image::ret_pops(&rest), 4);

    // The 64 report pages: teardown frees 0x40 of them from +0x7D4.
    let mut teardown = img.decode(0x3fc69d);
    assert_eq!(teardown.memory_displacement32(), 0x7D4);
    teardown = img.decode(teardown.next_ip() as u32);
    assert_eq!(teardown.immediate32(), d3d::device::VISIBILITY_INDICES >> 8);

    // Only D3D code touches +0x7D4 as the report pages: 0x3f8570, 0x3f8630
    // and the teardown, all overridden or only called from overrides.
    let d3d = img.section("D3D");
    let sites: Vec<u32> = d3d
        .iter()
        .filter(|i| {
            i.op_count() > 0
                && i.memory_displacement32() == 0x7D4
                && i.memory_base() != Register::None
        })
        .map(|i| i.ip32())
        .collect();
    eprintln!("+0x7D4 in D3D: {sites:x?}");
    for s in &sites {
        assert!(
            (0x3f8570..END).contains(s)
                || (GET..0x3f868d).contains(s)
                || (0x3fc530..0x3fc720).contains(s),
            "{s:#x}"
        );
    }

    // Halo's sites: the inline begin `0x817C8` with 1, 1 at 0x20cec, End
    // at 0x20d0f and GetVisibilityTestResult at 0x20cbb / 0x20840, each
    // with the index in EAX.
    let begin = img.decode(0x20cec);
    assert!(image::stores_imm(&begin, 0x0008_17C8), "{begin:?}");
    for (site, target) in [(0x20d0f, END), (0x20cbb, GET), (0x20840, GET)] {
        assert!(image::calls(&img.decode(site), target), "{site:#x}");
    }
    let before_end = img.decode(0x20d0d);
    assert_eq!(
        (before_end.mnemonic(), before_end.op0_register()),
        (Mnemonic::Mov, Register::EAX)
    );
    let before_get = img.decode(0x20cb9);
    assert_eq!(
        (before_get.mnemonic(), before_get.op0_register()),
        (Mnemonic::Mov, Register::EAX)
    );
    // EDX is the stamp pointer at both GetVisibilityTestResult sites.
    for at in [0x20cb5, 0x2083c] {
        let i = img.decode(at);
        assert_eq!(
            (i.mnemonic(), i.op0_register()),
            (Mnemonic::Lea, Register::EDX),
            "{at:#x}"
        );
    }
    // 0x20861: the count is divided by the expected one.
    let mut at = 0x2085a;
    let mut divss = false;
    for _ in 0..8 {
        let i = img.decode(at);
        divss |= i.mnemonic() == Mnemonic::Divss;
        at = i.next_ip() as u32;
    }
    assert!(divss);

    // D3DERR_TESTINCOMPLETE is handled, not retried: right after each call
    // a non-zero EAX jumps forward, to where the loop moves on to the next
    // object (0x2096d, 0x20d5e), and from there to the loop's backward
    // branch nothing calls GetVisibilityTestResult again.
    for (ret_at, skip) in [(0x20845, 0x2096d), (0x20cc0, 0x20d5e)] {
        let mut at = ret_at;
        let mut target = None;
        for _ in 0..3 {
            let i = img.decode(at);
            if i.mnemonic() == Mnemonic::Jne {
                target = Some(i.near_branch32());
                break;
            }
            at = i.next_ip() as u32;
        }
        assert_eq!(target, Some(skip), "{ret_at:#x}");
        let mut at = skip;
        loop {
            let i = img.decode(at);
            assert!(!image::calls(&i, GET), "{at:#x}");
            if i.is_jcc_short_or_near() && i.near_branch32() < skip {
                break;
            }
            at = i.next_ip() as u32;
        }
    }
    // The original's incomplete path: when the GPU time (+0x2C) has not
    // moved since the End, `D3D_SetFence` (0x3fa680) before returning.
    let cmp = img.decode(0x3f8659);
    assert_eq!(
        (cmp.mnemonic(), cmp.memory_displacement32()),
        (Mnemonic::Cmp, d3d::layout::device_field::GPU_TIME)
    );
    assert!(image::calls(&img.decode(0x3f8660), 0x3fa680));
}

/// `D3D_DestroyResource` runs as compiled: `EDI` = resource, plain `ret`,
/// and its only side effects go through the overridden BlockOnResource,
/// the game allocators and D3D's list at `0x408644`, which nothing fills.
#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn destroy_resource_conventions_in_the_image() {
    use iced_x86::{Mnemonic, OpKind, Register};
    let bytes = retail_xbe();
    let img = image::Image::new(&bytes);
    let f = img.function(runtime_glue::DESTROY_RESOURCE);
    // `mov eax, [edi]`: the resource comes in EDI.
    assert_eq!(f[0].mnemonic(), Mnemonic::Mov);
    assert_eq!(
        (f[0].memory_base(), f[0].op0_register()),
        (Register::EDI, Register::EAX)
    );
    let targets: Vec<u32> = f
        .iter()
        .filter(|i| i.mnemonic() == Mnemonic::Call && i.op0_kind() == OpKind::NearBranch32)
        .map(|i| i.near_branch32())
        .collect();
    assert_eq!(targets, [0x3fad50, 0x3fb4d0, 0x2d0a60, 0x122c0]);
    assert_eq!(image::ret_pops(&f), 0);
    // BlockOnResource is overridden.
    let entries = d3d::abi::entries::<RecordingBackend>();
    assert!(entries.iter().any(|e| e.addr == 0x3fad50));
    assert!(
        d3d::abi::NATIVE
            .iter()
            .any(|n| n.0 == runtime_glue::DESTROY_RESOURCE)
    );
    // Nothing but 0x3fb4d0 names D3D's list head at 0x408644 (as an
    // address or an operand), so nothing ever links a node into it and the
    // unlink walk finds an empty list.
    let mut refs = Vec::new();
    for name in [".text", "D3D", "XPP"] {
        for i in img.section(name) {
            let named = (0..i.op_count()).any(|k| match i.op_kind(k) {
                OpKind::Memory => i.memory_displacement32() == 0x408644,
                OpKind::Immediate32 => i.immediate32() == 0x408644,
                _ => false,
            });
            if named {
                refs.push(i.ip32());
            }
        }
    }
    assert!(!refs.is_empty());
    assert!(
        refs.iter().all(|a| (0x3fb4d0..0x3fb520).contains(a)),
        "{refs:x?}"
    );
    // The other two direct calls (game code at 0x13d24, teardown at
    // 0x3fc611) release device +0x7A0, which nothing stores but zero: every
    // un-indexed store to a +0x7A0 field is an immediate 0 (0x13d2a), so
    // neither call runs and the device need not model the field.
    for at in [0x13d24u32, 0x3fc611] {
        assert!(
            image::calls(&img.decode(at), runtime_glue::DESTROY_RESOURCE),
            "{at:#x}"
        );
    }
    let mut stores = Vec::new();
    for name in [".text", "D3D", "XPP"] {
        for i in img.section(name) {
            if i.op_count() >= 2
                && i.op0_kind() == OpKind::Memory
                && i.memory_index() == Register::None
                && i.memory_base() != Register::None
                && i.memory_displacement32() == 0x7A0
                && i.mnemonic() == Mnemonic::Mov
            {
                stores.push((i.ip32(), i.op1_kind(), i.immediate32()));
            }
        }
    }
    assert!(!stores.is_empty());
    for (at, kind, imm) in &stores {
        assert!(
            *kind == OpKind::Immediate32 && *imm == 0,
            "store to +0x7A0 at {at:#x}: {kind:?}"
        );
    }
    // Release and the internal release destroy through it, with EDI set.
    for at in [0x3fb676u32, 0x3fb5e9, 0x3fb5d9] {
        assert!(
            image::calls(&img.decode(at), runtime_glue::DESTROY_RESOURCE),
            "{at:#x}"
        );
    }
}

/// The gamma-ramp load: `EAX` = miniport (its first dword the register
/// base), `EDX` = the ramp; DAC writes only; plain `ret`. Halo's one call
/// (`0x12d08`) passes device `+0x1C28`.
#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn gamma_ramp_conventions_in_the_image() {
    use iced_x86::{Mnemonic, Register};
    let bytes = retail_xbe();
    let img = image::Image::new(&bytes);
    let f = img.function(0x3ff5c6);
    assert_eq!(f[0].mnemonic(), Mnemonic::Mov);
    assert_eq!(
        (f[0].op0_register(), f[0].memory_base()),
        (Register::ECX, Register::EAX)
    );
    let stores: Vec<u32> = f
        .iter()
        .filter(|i| {
            i.op_count() == 2
                && i.op0_kind() == iced_x86::OpKind::Memory
                && i.memory_base() == Register::ECX
        })
        .map(|i| i.memory_displacement32())
        .collect();
    assert!(
        !stores.is_empty() && stores.iter().all(|d| [0x6813C8, 0x6813C9].contains(d)),
        "{stores:x?}"
    );
    assert!(f.iter().any(|i| i.mnemonic() == Mnemonic::Lea
        && i.memory_base() == Register::EDX
        && i.memory_displacement32() == 0x100));
    assert_eq!(image::ret_pops(&f), 0);
    assert!(image::calls(&img.decode(0x12d08), 0x3ff5c6));
    let lea = img.decode(0x12cb4);
    assert_eq!(
        (
            lea.mnemonic(),
            lea.op0_register(),
            lea.memory_displacement32()
        ),
        (Mnemonic::Lea, Register::EAX, d3d::device::MINIPORT)
    );
}
