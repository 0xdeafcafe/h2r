//! Installs crates/d3d through its own registration (`d3d::register`,
//! docs/d3d.md "Integration") on a headless RecordingBackend, so the boot
//! tracer gets past device creation without a window (or on a headless
//! GfxBackend with `H2_BOOT_GFX`). Uploads are `UploadPolicy::Tracked`,
//! fed by the write barrier below.

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
    /// The entries the write barrier wraps, in shadow-slot order.
    barrier: Vec<u32>,
    /// The vblank callback on the 60 Hz logical clock, for
    /// `Kernel::add_periodic` (taken once).
    pub vblank: Option<kernel::periodic::Handler>,
}

impl Installed {
    /// Where the write barrier moved `entry`'s real handler.
    pub fn barrier_shadow(&self, entry: u32) -> Option<u32> {
        self.barrier.iter().position(|&e| e == entry).map(|i| BARRIER_SHADOW + 4 * i as u32)
    }
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
            // `never`: render on the GPU but write no screenshots (benchmarks).
            let every: u64 = match std::env::var("H2_BOOT_GFX_EVERY").ok().as_deref() {
                Some("never") => u64::MAX,
                v => v.and_then(|v| v.parse().ok()).filter(|&n| n > 0).unwrap_or(30),
            };
            let ctx = gfx::GfxContext::headless().map_err(|e| anyhow::anyhow!("gfx headless context: {e}"))?;
            let mut swaps = 0u64;
            let backend = d3d::GfxBackend::new(ctx).on_present(move |ctx, store, id, _flags| {
                swaps += 1;
                if (swaps != 1 || every == u64::MAX) && !swaps.is_multiple_of(every) {
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
        for p in px.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
    }
    for p in px.as_chunks_mut::<4>().0 {
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
    let (backend, draw_stats) = crate::d3d_diag::DiagBackend::new(backend);
    // Tracked uploads trust the write barrier below; H2_BOOT_UPLOAD_ALWAYS
    // goes back to re-reading every resource on every use.
    let upload = if std::env::var_os("H2_BOOT_UPLOAD_ALWAYS").is_some() {
        UploadPolicy::Always
    } else {
        UploadPolicy::Tracked
    };
    let cfg = DeviceConfig {
        upload,
        nv2a_window: Some(NV2A_BASE),
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
            d3d::FrameWait::HALO2_CLOCKED,
            Arc::new(d3d::runtime_glue::default_stop),
        );
    });
    let r = out.unwrap();
    let entries: Vec<u32> = r.implemented.iter().chain(&r.stopping).map(|e| e.0).collect();
    install_write_barrier(rt, &device, &entries)?;
    let vblank: Option<kernel::periodic::Handler> = Some(Box::new(d3d::vblank::clocked_vblanks(Arc::clone(&device))));
    let dev = device;
    Ok(Installed {
        vblank,
        report: Box::new(move || {
            let mut d = dev.lock().unwrap();
            // Device events by kind and reason, most frequent first.
            let mut ev: std::collections::BTreeMap<String, u64> = Default::default();
            for e in d.take_events() {
                let k = match e {
                    d3d::Event::DrawSkipped { entry, reason } => format!("draw skipped in {entry}: {reason}"),
                    d3d::Event::BadState(b) => format!("bad state {b:?}"),
                    other => format!("{other:?}"),
                };
                let k: String = k.chars().take(200).collect();
                *ev.entry(k).or_default() += 1;
            }
            let mut ev: Vec<_> = ev.into_iter().collect();
            ev.sort_by_key(|e| std::cmp::Reverse(e.1));
            let ev: String = ev.iter().take(40).map(|(k, n)| format!("\n  event {n:>8}  {k}")).collect();
            format!("stats {:?}; last stop {:?}{ev}\n{}", d.stats(), d.last_stop(), draw_stats.lock().unwrap().report())
        }),
        implemented: r.implemented.len(),
        stopping: r.stopping.len(),
        native: r.native.len(),
        barrier: entries,
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



/// The NV2A register window crates/d3d maps and stores at device+0x934
/// (`DeviceConfig::nv2a_window`): the retail 0xfd000000 cannot be used, as
/// the runtime keeps the NV2A aperture free of RAM mappings.
pub const NV2A_BASE: u32 = 0xa040_0000;

/// Shadow page for the displaced crates/d3d handlers the boot wraps.
const D3D_SHADOW: u32 = 0xa0ff_2000;
const CREATE_DEVICE: u32 = 0x003f_5240;

/// Halo creates the device, releases it (the caller zeroes the device
/// object) and creates it again, so the back and depth surfaces are backed
/// after every Direct3D_CreateDevice ([`back_surfaces`]): crates/d3d's
/// handler moves to a shadow address and a continuation runs once it
/// returns.
pub fn install_back_surfaces(rt: &runtime::Runtime) -> anyhow::Result<()> {
    rt.memory().map_zeroed(D3D_SHADOW, 0x1000, runtime::guest::Permissions::RX)?;
    let device = d3d::GuestLayout::HALO2.device;
    let shadow = D3D_SHADOW;
    rt.edit_dispatch(|d| {
        let prev = d.register_override(CREATE_DEVICE, "boot:CreateDevice+back surfaces", move |cpu, ctx, _| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let ret_pc = mem.u32(cpu.esp)?;
            cpu.esp = cpu.esp.wrapping_add(4);
            ctx.call_guest(cpu, &mut mem, shadow, move |cpu, ctx| {
                if cpu.eax == 0 {
                    back_surfaces(&mut ctx.runtime().memory(), device);
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


/// Shadow page for the d3d entries the write barrier wraps (4 bytes each).
const BARRIER_SHADOW: u32 = 0xa0ff_4000;

/// The D3D write barrier (docs/d3d.md "Integration", step 3) on the
/// runtime's write watch: one subscriber watches all of RAM in the folded
/// view, and every d3d entry is wrapped so that
///
/// - on entry, the pages written since the last drain (lifted code, the
///   interpreter, kernel and HLE copies) go to `Device::note_write`, before
///   the device reads any resource;
/// - nothing is drained on return. The entry is a guest call: runner safe
///   points, interrupts and other contexts can store between the entry
///   drain and the return, and dropping the pages then would lose those
///   stores. The device's own stores are therefore reported back to it at
///   the next entry, which costs a re-check of a page it already has (its
///   render-target writebacks it invalidates itself).
///
/// The device's tracker keeps only the pages it watches, so watching all of
/// RAM here costs a bit per store and nothing more.
fn install_write_barrier<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    device: &Arc<Mutex<d3d::Device<B>>>,
    entries: &[u32],
) -> anyhow::Result<()> {
    use runtime::watch::CONTIGUOUS_BASE;
    let id = {
        let mut mem = rt.memory();
        mem.map_zeroed(BARRIER_SHADOW, 0x1000, runtime::guest::Permissions::RX)?;
        let id = mem.subscribe_writes("boot:d3d");
        mem.watch_writes(id, CONTIGUOUS_BASE, 0x0400_0000).map_err(|e| anyhow::anyhow!("{e}"))?;
        id
    };
    anyhow::ensure!(entries.len() <= 0x400, "too many d3d entries for the shadow page");
    rt.edit_dispatch(|d| {
        for (i, &addr) in entries.iter().enumerate() {
            let shadow = BARRIER_SHADOW + 4 * i as u32;
            let dev = Arc::clone(device);
            let Some(prev) = d.register_override(addr, "boot:d3d write barrier", move |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let dirty = mem.take_dirty(id).unwrap_or_default();
                if !dirty.is_empty() {
                    let mut dv = dev.lock().unwrap();
                    for r in dirty {
                        dv.note_write(r.addr, r.len);
                    }
                }
                let ret_pc = mem.u32(cpu.esp)?;
                cpu.esp = cpu.esp.wrapping_add(4);
                ctx.call_guest(cpu, &mut mem, shadow, move |_, _| Ok(runtime::TransferAbi::ret(ret_pc)))
            }) else {
                continue;
            };
            d.restore_override(shadow, prev);
        }
    });
    Ok(())
}
