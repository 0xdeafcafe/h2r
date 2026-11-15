//! Installs crates/d3d through its own registration (`d3d::register`,
//! docs/d3d.md "Integration") on a headless RecordingBackend, so the boot
//! tracer gets past device creation without a window. There is no write
//! barrier yet, so the device runs with `UploadPolicy::Always`.

use std::sync::{Arc, Mutex};

use d3d::{DeviceConfig, GuestRange, RecordingBackend, UploadPolicy};

pub const PUSH_BASE: u32 = 0xa0e0_0000;
pub const PUSH_LEN: u32 = 0x8_0000;
pub const SCRATCH_BASE: u32 = 0xa0e8_0000;

pub struct Installed {
    report: Box<dyn Fn() -> String + Send + Sync>,
    pub implemented: usize,
    pub stopping: usize,
    pub native: usize,
}

/// `d3d::register` on a headless `RecordingBackend`, or, with
/// `H2_BOOT_GFX=<dir>`, on a headless `GfxBackend` with the nv2a shader
/// translator; then every `Swap` that shows a new image (at most one per
/// `H2_BOOT_GFX_EVERY` swaps, default 30) is written to `<dir>` as
/// `frame-NNNNN.png`. Halo's frame wait goes through
/// `d3d::register_frame_wait` either way.
pub fn install(rt: &runtime::Runtime) -> anyhow::Result<Installed> {
    match std::env::var_os("H2_BOOT_GFX") {
        None => install_with(rt, RecordingBackend::new()),
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir)?;
            let every: u64 = std::env::var("H2_BOOT_GFX_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
            let ctx = gfx::GfxContext::headless().map_err(|e| anyhow::anyhow!("gfx headless context: {e}"))?;
            let mut swaps = 0u64;
            let backend = d3d::GfxBackend::new(ctx).on_present(move |ctx, store, id, _flags| {
                swaps += 1;
                if swaps != 1 && swaps % every != 0 {
                    return;
                }
                if let Err(e) = save_png(ctx, store, id, &dir.join(format!("frame-{swaps:05}.png"))) {
                    eprintln!("boot: gfx screenshot {swaps}: {e}");
                }
            });
            install_with(rt, backend)
        }
    }
}

fn save_png(
    ctx: &gfx::GfxContext,
    store: &gfx::surface::SurfaceStore,
    id: gfx::surface::SurfaceId,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    let (mut px, order) = store.read_color(ctx, id).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let d = store.get(id).map_err(|e| anyhow::anyhow!("{e:?}"))?.desc();
    if matches!(order, gfx::surface::HostPixelOrder::Bgra8) {
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
    }
    for p in px.chunks_exact_mut(4) {
        p[3] = 0xff; // X8R8G8B8 back buffer: alpha is undefined
    }
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(f, d.width, d.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&px)?;
    Ok(())
}

fn install_with<B: d3d::backend::Backend + Send + 'static>(rt: &runtime::Runtime, backend: B) -> anyhow::Result<Installed> {
    let cfg = DeviceConfig {
        upload: UploadPolicy::Always,
        ..DeviceConfig::halo2(
            GuestRange { addr: PUSH_BASE, len: PUSH_LEN },
            GuestRange { addr: SCRATCH_BASE, len: 0x1000 },
        )
    };
    d3d::runtime_glue::map_device_memory(&mut rt.memory(), &cfg)?;
    let device = d3d::runtime_glue::shared(cfg, backend);
    let mut out = None;
    rt.edit_dispatch(|d| {
        out = Some(d3d::register(d, Arc::clone(&device)));
        // Halo's frame wait 0x12b2e0 ("Vertical blank" in docs/d3d.md):
        // calls the vblank callback at device+0x1db8 until the counter
        // reaches the target, unpaced. Generated code would otherwise spin
        // on the counter natively and never return to the runner.
        d3d::register_frame_wait(
            d,
            Arc::clone(&device),
            d3d::FrameWait::HALO2,
            Arc::new(d3d::runtime_glue::default_stop),
        );
    });
    let r = out.unwrap();
    let dev: Arc<Mutex<d3d::Device<B>>> = device;
    Ok(Installed {
        report: Box::new(move || {
            let d = dev.lock().unwrap();
            format!("stats {:?}; last stop {:?}", d.stats(), d.last_stop())
        }),
        implemented: r.implemented.len(),
        stopping: r.stopping.len(),
        native: r.native.len(),
    })
}

/// One line for the run report.
pub fn describe(i: &Installed) -> String {
    format!(
        "d3d: {} implemented, {} stopping, {} native overrides; {}",
        i.implemented,
        i.stopping,
        i.native,
        (i.report)()
    )
}

/// crates/d3d entries listed as unimplemented whose retail body is safe to
/// run as generated code: the boot disables the d3d override so the
/// original runs. `(address, report name)`. Owner: crates/d3d (move them to
/// `abi::NATIVE`, or implement them).
///
/// - 0x3fdb20, Get2DSurfaceDesc. Register ABI (recovered here): EDI = pixel
///   container, EBX = level, ESI = `D3DSURFACE_DESC` out; plain `ret`.
///   Reads the container's format/size words through 0x3fb690 and 0x3fd9c0
///   (pure, format table 0x403a50) and device+0x1a14 / +0x196c (render
///   target, multisample type), and writes only the descriptor. Called
///   from 0x13e06.
pub const RUN_NATIVE: &[(u32, &str)] = &[
    (0x003f_db20, "Get2DSurfaceDesc (retail body)"),
];

pub fn install_stopgaps(rt: &runtime::Runtime, uses: &crate::stopgaps::Uses) {
    rt.edit_dispatch(|d| {
        for &(addr, name) in RUN_NATIVE {
            if d.set_enabled(addr, false) {
                uses.lock().unwrap().entry(addr).or_insert((name, 0));
            }
        }
    });
}


/// Retail NV2A register base. Halo's inlined GPU-idle waits read it from
/// device+0x934 and spin on PFIFO (`+0x3240` != `+0x3244`, CACHE1 DMA put
/// vs get) and PGRAPH status (`+0x400700` != 0), e.g. 0x12d0cf and
/// CDevice_KickOff's own tail at 0x3fac64. crates/d3d's CreateDevice does
/// not set device+0x934, so the read is from address 0x3240. Boot stopgap:
/// point it at a stand-in base and map just those two register pages,
/// zeroed, so the FIFO always reads empty and PGRAPH idle. The retail base
/// (0xfd000000) cannot be used: the runtime keeps the NV2A aperture free
/// of RAM mappings and has no MMIO handlers.
pub const NV2A_BASE: u32 = 0xa040_0000;
const DEVICE_NV2A_FIELD: u32 = 0x934;

/// Shadow page for the displaced crates/d3d handlers the boot wraps.
const D3D_SHADOW: u32 = 0xa0ff_2000;
const CREATE_DEVICE: u32 = 0x003f_5240;

/// Halo creates the device, releases it (the caller zeroes the device
/// object) and creates it again, so the field is set after every
/// Direct3D_CreateDevice: crates/d3d's handler moves to a shadow address
/// and a continuation writes device+0x934 once it returns.
pub fn install_gpu_registers(rt: &runtime::Runtime) -> anyhow::Result<()> {
    {
        let mut mem = rt.memory();
        mem.map_zeroed(NV2A_BASE + 0x3000, 0x1000, runtime::guest::Permissions::RW)?;
        mem.map_zeroed(NV2A_BASE + 0x40_0000, 0x1000, runtime::guest::Permissions::RW)?;
        mem.map_zeroed(D3D_SHADOW, 0x1000, runtime::guest::Permissions::RX)?;
    }
    let device = d3d::GuestLayout::HALO2.device;
    let shadow = D3D_SHADOW;
    rt.edit_dispatch(|d| {
        let prev = d.register_override(CREATE_DEVICE, "boot:CreateDevice+NV2A base", move |cpu, ctx, _| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let ret_pc = mem.u32(cpu.esp)?;
            cpu.esp = cpu.esp.wrapping_add(4);
            ctx.call_guest(cpu, &mut mem, shadow, move |cpu, ctx| {
                if cpu.eax == 0 {
                    let mut mem = ctx.runtime().memory();
                    mem.set_u32(device + DEVICE_NV2A_FIELD, NV2A_BASE)?;
                    back_surfaces(&mut mem, device);
                }
                Ok(runtime::TransferAbi::ret(ret_pc))
            })
        });
        if let Some(prev) = prev {
            d.restore_override(shadow, prev);
        }
    });
    Ok(())
}

/// Xbox D3D addresses the same physical page three ways: the contiguous
/// window `0x80000000 | phys`, the GPU's `phys`, and the CPU write-combined
/// window `0xf0000000 | phys` (Halo's CPU blits to surfaces use the last,
/// e.g. `or [esp+0Ch], 0xF0000000` at 0x13e4b after 0x3fdbc0). The runtime
/// maps none of 0xf0000000. Boot stopgap: alias `0xf0000000 | (va &
/// 0x0fffffff)` onto the physical pages behind `va` for the ranges the boot
/// itself maps (the fixed arena, the back and depth buffers). Blocker:
/// runtime/kernel (the write-combined window over all contiguous memory).
pub const WC_BASE: u32 = 0xf000_0000;

/// The write-combined window of every physical reservation (the loader was
/// given `WC_BASE | start` as an extra alias; see `run`).
pub fn map_reserved_write_combined(rt: &runtime::Runtime) -> anyhow::Result<()> {
    let mut mem = rt.memory();
    let rs: Vec<_> = mem.physical_reservations().iter().map(|r| (r.id, r.physical_start, r.len)).collect();
    for (id, start, len) in rs {
        mem.map_reserved(id, WC_BASE | start, len, runtime::guest::Permissions::RW)?;
    }
    Ok(())
}

pub fn map_write_combined(mem: &mut runtime::Guest, va: u32, len: u32) -> anyhow::Result<()> {
    let m = mem
        .mappings()
        .iter()
        .find(|m| m.virtual_start <= va && va as u64 + len as u64 <= m.virtual_start as u64 + m.len as u64)
        .map(|m| m.physical_start + (va - m.virtual_start))
        .ok_or_else(|| anyhow::anyhow!("{va:#x}: not one mapping"))?;
    let phys = m;
    mem.map(0xf000_0000 | (va & 0x0fff_ffff), len, phys, runtime::guest::Permissions::RW)?;
    Ok(())
}

/// crates/d3d's CreateDevice writes the back-buffer and depth surface
/// headers with `Data = 0` (no guest backing; the host surface is the real
/// one), so Halo's CPU writes into the back buffer (0x13d80, a full-screen
/// blit right after the first map load) land at 0xf0000000. Boot stopgap:
/// give each header that still has `Data = 0` its own zeroed pages, at a
/// physical address P mapped at both `0x80000000 | P` and `0xf0000000 |
/// P`, and store P as Data. Owner: crates/d3d (guest-backed surfaces).
pub fn back_surfaces(mem: &mut runtime::Guest, device: u32) {
    use d3d::layout::device_field as field;
    for f in [field::BACK_BUFFER, field::DEPTH_STENCIL] {
        let Ok(header) = mem.u32(device + f) else { continue };
        if header == 0 || mem.u32(header + 4).is_ok_and(|d| d != 0) {
            continue;
        }
        let Ok(size) = mem.u32(header + 16) else { continue };
        let (w, h, pitch) = ((size & 0xfff) + 1, ((size >> 12) & 0xfff) + 1, ((size >> 24) + 1) * 64);
        let len = (pitch * h).max(w * 4).next_multiple_of(0x1000);
        let Some(phys) = mem.find_free_physical(len, 0..0x0400_0000, 0x1_0000) else {
            crate::log::note(&format!("back surface {header:#x}: no free physical memory"));
            continue;
        };
        if mem.map(0x8000_0000 | phys, len, phys, runtime::guest::Permissions::RW).is_err() {
            crate::log::note(&format!("back surface {header:#x}: no room at {phys:#x}"));
            continue;
        }
        let _ = mem.map(0xf000_0000 | phys, len, phys, runtime::guest::Permissions::RW);
        let _ = mem.write_bytes(0x8000_0000 | phys, &vec![0; len as usize], 1);
        let _ = mem.set_u32(header + 4, phys);
    }
}
