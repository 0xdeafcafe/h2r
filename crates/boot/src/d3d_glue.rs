//! Installs crates/d3d through its own registration (`d3d::register`,
//! docs/d3d.md "Integration") on a headless RecordingBackend, so the boot
//! tracer gets past device creation without a window. There is no write
//! barrier yet, so the device runs with `UploadPolicy::Always`.

use std::sync::{Arc, Mutex};

use d3d::{DeviceConfig, GuestRange, RecordingBackend, UploadPolicy};

pub const PUSH_BASE: u32 = 0xa0e0_0000;
pub const PUSH_LEN: u32 = 0x8_0000;
pub const SCRATCH_BASE: u32 = 0xa0e8_0000;

pub type Shared = Arc<Mutex<d3d::Device<RecordingBackend>>>;

pub struct Installed {
    pub device: Shared,
    pub implemented: usize,
    pub stopping: usize,
    pub native: usize,
}

pub fn install(rt: &runtime::Runtime) -> anyhow::Result<Installed> {
    let cfg = DeviceConfig {
        upload: UploadPolicy::Always,
        ..DeviceConfig::halo2(
            GuestRange { addr: PUSH_BASE, len: PUSH_LEN },
            GuestRange { addr: SCRATCH_BASE, len: 0x1000 },
        )
    };
    d3d::runtime_glue::map_device_memory(&mut rt.memory(), &cfg)?;
    let device = d3d::runtime_glue::shared(cfg, RecordingBackend::new());
    let mut out = None;
    rt.edit_dispatch(|d| out = Some(d3d::register(d, Arc::clone(&device))));
    let r = out.unwrap();
    Ok(Installed { device, implemented: r.implemented.len(), stopping: r.stopping.len(), native: r.native.len() })
}

/// One line for the run report.
pub fn describe(i: &Installed) -> String {
    let d = i.device.lock().unwrap();
    format!(
        "d3d: {} implemented, {} stopping, {} native overrides; stats {:?}; last stop {:?}",
        i.implemented,
        i.stopping,
        i.native,
        d.stats(),
        d.last_stop()
    )
}

/// crates/d3d entries listed as unimplemented that the boot turns into
/// no-ops, as `(address, report name, bytes popped by the ret)`. Each one
/// only computes from device fields the HLE does not keep and writes NV2A
/// methods into the push buffer (or device-private matrices), which the
/// device's read-back does not decode; the HLE keeps its own viewport and
/// transforms from the D3DDevice_* calls. Owner: crates/d3d.
///
/// - 0x3f6190, viewport push fragment. EAX = put, ESI = device, returns the
///   new put in EAX, plain `ret`. Writes viewport offset/scale (0x0a20,
///   0x0af0) and the clip range (0x0394). Halo's rasterizer calls it inline
///   (0x12dc8) and stores EAX as put; EAX is left unchanged (nothing pushed).
/// - 0x3f6840, D3D_UpdateProjectionViewportTransform. No args, plain `ret`.
///   Builds the projection-viewport matrix at device+0x980 and sets dirty
///   bit 0x200 in 0x407490. Called from 0x35c95 when the game flips the
///   depth mode at 0x4076d4.
/// - 0x3f6f40, transform/viewport push. ESI = device, plain `ret`; pushes
///   the transform constants and stores the new put in [esi] itself, so
///   doing nothing leaves put as it was. Called right after 0x3f6840.
/// - 0x3f6390, control push fragment. EDX = put, [esp+4] = device,
///   `ret 4`, and it returns the new put in **EAX** (`lea eax,[edx+8]`;
///   crates/d3d's table says EDX). Writes NV097_SET_CONTROL0 (0x0290) with
///   the depth-format bits (0x100001, | 0x10000000 when 0x407718, | 0x10000
///   when 0x4076d4 == 2, | 0x1000 for some depth surface formats).
///   Called from 0x35cb8; EAX = EDX (nothing pushed).
pub const NO_OPS: &[(u32, &str, u32)] = &[
    (0x003f_6190, "D3D viewport push fragment (nothing pushed)", 0),
    (0x003f_6840, "D3D_UpdateProjectionViewportTransform (no-op)", 0),
    (0x003f_6f40, "D3D transform/viewport push 0x3f6f40 (no-op)", 0),
    (0x003f_6390, "D3D control push fragment 0x3f6390 (nothing pushed)", 4),
];

/// Entries in [`NO_OPS`] whose new put is EDX on entry, returned in EAX.
const PUT_IN_EDX: &[u32] = &[0x003f_6390];

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
        for &(addr, name, pop) in NO_OPS {
            let u = Arc::clone(uses);
            d.register_override(addr, "boot:d3d no-op", move |cpu, ctx, _| {
                u.lock().unwrap().entry(addr).or_insert((name, 0)).1 += 1;
                if PUT_IN_EDX.contains(&addr) {
                    cpu.eax = cpu.edx;
                }
                ctx.runtime().memory().ret(cpu, pop)
            });
        }
    });
}

/// D3DDevice_SetSoftDisplayFilter at 0x3f8c70 (crates/d3d: "display/flicker
/// filter", unimplemented). Register ABI: ESI = enable, plain `ret`. The
/// retail body sends AvSendTVEncoderOption(device+0x1c28 register base,
/// 0xe, enable, NULL) unless the cached state already matches, then caches
/// it: [0x40863c] = 1 (valid), [0x408630] = enable != 0. There is no TV
/// encoder, so only the cache is kept. Called from 0x14170.
pub const SET_SOFT_DISPLAY_FILTER: u32 = 0x003f_8c70;

pub fn install_display_filter(rt: &runtime::Runtime, uses: &crate::stopgaps::Uses) {
    let u = Arc::clone(uses);
    rt.edit_dispatch(|d| {
        d.register_override(SET_SOFT_DISPLAY_FILTER, "boot:D3DDevice_SetSoftDisplayFilter", move |cpu, ctx, _| {
            u.lock().unwrap().entry(SET_SOFT_DISPLAY_FILTER).or_insert(("D3DDevice_SetSoftDisplayFilter (cached only)", 0)).1 += 1;
            let mut mem = ctx.runtime().memory();
            mem.set_u32(0x0040_863c, 1)?;
            mem.set_u32(0x0040_8630, (cpu.esi != 0) as u32)?;
            mem.ret(cpu, 0)
        });
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

/// The XDK resource busy check, `0x003fb710` (register ABI: ECX = resource,
/// plain `ret`, BOOL in EAX; EDI preserved). Retail body: busy (1) if the
/// lock bits `0x780000` are set in the resource's Common word or, for a
/// type-5 resource with a parent at +0x14, in the parent's; otherwise it
/// compares the resource's push-buffer fence `[res+8]` with the GPU read
/// pointer (`[[device+0x30]]`) and put (`[device+0x2c]`), and when the GPU
/// is past it clears the fence and returns 0. Called from the game's pool
/// allocator (`0x12d1ea`) before it reuses a block.
///
/// Boot stopgap: the same lock-bit checks, with the fence always passed
/// (the HLE device executes every push synchronously, so nothing is in
/// flight). Owner: d3d (crates/d3d lists it as a stopping entry).
pub const RESOURCE_BUSY: u32 = 0x003f_b710;

pub fn install_resource_busy(rt: &runtime::Runtime, uses: &crate::stopgaps::Uses) {
    const LOCKS: u32 = 0x0078_0000;
    let u = Arc::clone(uses);
    rt.edit_dispatch(|d| {
        d.register_override(RESOURCE_BUSY, "boot:D3D resource busy check (GPU idle)", move |cpu, ctx, _| {
            u.lock().unwrap().entry(RESOURCE_BUSY).or_insert(("D3D resource busy 0x3fb710 (GPU idle)", 0)).1 += 1;
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let is_child = |c: u32| c & 0x7_0000 == 0x5_0000;
            let mut res = cpu.ecx;
            let common = mem.u32(res)?;
            let parent = mem.u32(res + 0x14)?;
            if is_child(common) && parent != 0 {
                if common & LOCKS != 0 || mem.u32(parent)? & LOCKS != 0 {
                    cpu.eax = 1;
                    return mem.ret(cpu, 0);
                }
                res = parent;
            }
            let common = mem.u32(res)?;
            if common & LOCKS != 0 {
                cpu.eax = 1;
                return mem.ret(cpu, 0);
            }
            if is_child(common) {
                let p = mem.u32(res + 0x14)?;
                if p != 0 && mem.u32(p)? & LOCKS != 0 {
                    cpu.eax = 1;
                    cpu.ecx = res;
                    return mem.ret(cpu, 0);
                }
            }
            if mem.u32(res + 8)? != 0 {
                mem.set_u32(res + 8, 0)?;
            }
            cpu.eax = 0;
            cpu.ecx = res;
            mem.ret(cpu, 0)
        });
    });
}

/// `0x003f8cc0` (stdcall, 1 arg: 32 dwords; called from 0x47a55, the
/// d3d table calls it the gamma/display ramp). Retail body: unless
/// device+8 bit 0x10, cache the 32 dwords at device+0x197c; then push them
/// as NV2A method 0x1480 x32. crates/d3d's push parser stops on method
/// 0x1480 ("unimplemented NV2A method"), so running the retail body only
/// moves the stop.
///
/// Boot stopgap: the cache write, no push. Owner: d3d (method 0x1480 in the
/// push parser, then this entry can run as the retail body).
pub const RAMP_1480: u32 = 0x003f_8cc0;

pub fn install_ramp(rt: &runtime::Runtime, uses: &crate::stopgaps::Uses) {
    let u = Arc::clone(uses);
    rt.edit_dispatch(|d| {
        d.register_override(RAMP_1480, "boot:0x3f8cc0 cache only (method 0x1480 not pushed)", move |cpu, ctx, _| {
            u.lock().unwrap().entry(RAMP_1480).or_insert(("D3D 0x3f8cc0 ramp (cached, method 0x1480 not pushed)", 0)).1 += 1;
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let device = mem.u32(0x0040_7488)?;
            let src = mem.u32(cpu.esp + 4)?;
            if mem.u8(device + 8)? & 0x10 == 0 {
                for i in 0..32 {
                    let w = mem.u32(src + i * 4)?;
                    mem.set_u32(device + 0x197c + i * 4, w)?;
                }
            }
            mem.ret(cpu, 4)
        });
    });
}

/// `D3DDevice_SetRenderTarget` (`0x003f7870`, stdcall `pRenderTarget,
/// pNewZStencil`, `ret 8`) when either surface is a level of a texture
/// (`Parent`, header +0x14, non-zero). crates/d3d stops on those
/// (render-to-texture): `0x14bc0` selects the offscreen targets `0x14850`
/// built with `0x3fb7a0`.
///
/// Boot stopgap: record the pair in the device's game-visible fields
/// (+0x1a04 / +0x1a08, which is all the HLE writes to guest memory) and
/// leave the HLE drawing into its current target. Offscreen passes land on
/// the current target and the texture keeps its old bytes, so those frames
/// are wrong, but the trace goes on. Every other call goes to crates/d3d
/// unchanged. Owner: d3d (render-to-texture).
pub const SET_RENDER_TARGET: u32 = 0x003f_7870;

pub fn install_render_to_texture(rt: &runtime::Runtime, uses: &crate::stopgaps::Uses) {
    // Opt-in: the next stop is then the 320x240 offscreen viewport on the
    // 640x480 back buffer, which this stopgap itself causes.
    if std::env::var_os("H2_BOOT_RTT").is_none() {
        return;
    }
    let inner = D3D_SHADOW + 0x10;
    let u = Arc::clone(uses);
    rt.edit_dispatch(|d| {
        let prev = d.register_override(SET_RENDER_TARGET, "boot:SetRenderTarget (texture levels kept off the HLE)", move |cpu, ctx, _| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let target = mem.u32(cpu.esp + 4)?;
            let z = mem.u32(cpu.esp + 8)?;
            let level = |m: &runtime::Guest, s: u32| -> Result<bool, runtime::GuestFault> {
                Ok(s != 0 && m.u32(s + 0x14)? != 0)
            };
            if !level(&mem, target)? && !level(&mem, z)? {
                return Ok(runtime::TransferAbi::jump(inner));
            }
            u.lock().unwrap().entry(SET_RENDER_TARGET).or_insert(("SetRenderTarget to a texture level (HLE target kept)", 0)).1 += 1;
            let device = d3d::GuestLayout::HALO2.device;
            if target != 0 {
                mem.set_u32(device + d3d::layout::device_field::RENDER_TARGET, target)?;
            }
            mem.set_u32(device + d3d::layout::device_field::DEPTH_STENCIL, z)?;
            mem.ret(cpu, 8)
        });
        if let Some(prev) = prev {
            d.restore_override(inner, prev);
        }
    });
}
