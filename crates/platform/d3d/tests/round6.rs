//! D3D round 6: the vertex-array methods and inline arrays that BeginPush
//! (`0x3fae10`) and `CDevice_SetStateVB` (`0x401030`) feed, the same-size
//! SetRenderTarget (`0x3f7b30`), and the NV2A register window behind
//! device `+0x934`. Synthetic values only; the ignored tests at the end
//! decode the retail image in `$H2_XBE`.

use crate::common::{self, *};
use d3d::abi::Regs;
use d3d::backend::Viewport;
use d3d::hw::method as hw;
use d3d::layout::device_field as f;
use d3d::pushbuf::method;
use d3d::{D3dError, Device, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};
use gfx::vertex::fvf;

const DEV: u32 = 0x0040_4FE0;

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

/// What `CDevice_SetStateVB` pushes for a vertex: all 16 formats in one
/// incrementing command (`0x4010b2`), then one single-word offset command
/// per enabled register (`0x401134`).
fn set_state_vb(m: &mut SparseMemory, formats: &[(usize, u32)], stride: u32) {
    let mut words = vec![header(hw::VERTEX_DATA_ARRAY_FORMAT, 16)];
    let mut regs = [d3d::hw::VERTEX_FORMAT_DISABLED; 16];
    for &(r, fmt) in formats {
        regs[r] = stride << 8 | fmt;
    }
    words.extend(regs);
    for &(r, _) in formats {
        words.push(header(hw::VERTEX_DATA_ARRAY_OFFSET + 4 * r as u32, 1));
        words.push(VB_DATA + 0x10 * r as u32);
    }
    push_words(m, &words);
}

/// What `0x4b220` pushes after BeginPush: `BEGIN_END`, one non-incrementing
/// `INLINE_ARRAY` with the vertex bytes, `BEGIN_END` 0.
fn inline_array(m: &mut SparseMemory, primitive: u32, bytes: &[u8]) {
    assert!(bytes.len().is_multiple_of(4));
    let n = (bytes.len() / 4) as u32;
    let mut words = vec![header(method::BEGIN_END, 1), primitive];
    words.push(0x4000_0000 | header(method::INLINE_ARRAY, n));
    words.extend(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c)),
    );
    words.extend([header(method::BEGIN_END, 1), 0]);
    push_words(m, &words);
}

fn kick(d: &mut Device<RecordingBackend>, m: &mut SparseMemory) -> Result<(), D3dError> {
    d.kick_off(m)
}

// Inline arrays ---------------------------------------------------------------

#[test]
fn set_state_vb_formats_reach_the_register_state() {
    let (mut d, mut m) = rec();
    set_state_vb(&mut m, &[(0, 0x42), (3, 0x40)], 20);
    kick(&mut d, &mut m).unwrap();
    let hw = d.hw();
    assert_eq!(hw.vertex_format[0], 20 << 8 | 0x42);
    assert_eq!(hw.vertex_format[3], 20 << 8 | 0x40);
    assert_eq!(hw.vertex_format[1], d3d::hw::VERTEX_FORMAT_DISABLED);
    assert_eq!(hw.vertex_offset[3], VB_DATA + 0x30);
}

#[test]
fn inline_array_with_every_format_disabled_stops() {
    let (mut d, mut m) = rec();
    d.set_vertex_shader(&mut m, fvf::XYZRHW).unwrap();
    set_state_vb(&mut m, &[], 0);
    inline_array(&mut m, TRIANGLESTRIP, &[0u8; 48]);
    let err = kick(&mut d, &mut m).unwrap_err().to_string();
    assert!(err.contains("every vertex array format disabled"), "{err}");
    assert!(d.fault().is_some(), "the device latches the fault");
}

/// A vertex program selected the way Halo does it (SelectVertexShaderDirect
/// with an attribute format at `vaf`), so draws run as programs.
fn program(d: &mut Device<RecordingBackend>, m: &mut SparseMemory) {
    let vaf = MISC + 0x400;
    for reg in 0..16u32 {
        m.write_u32(vaf + 16 * reg + 8, 0x02).unwrap(); // D3DVSDT_NONE
    }
    d.select_vertex_shader_direct(m, vaf, 0).unwrap();
}

#[test]
fn inline_array_with_an_unknown_format_stops() {
    let (mut d, mut m) = rec();
    d.set_vertex_shader(&mut m, fvf::XYZRHW).unwrap();
    // Type 3 is not a vertex type.
    set_state_vb(&mut m, &[(0, 0x43)], 16);
    inline_array(&mut m, TRIANGLESTRIP, &[0u8; 48]);
    let err = kick(&mut d, &mut m).unwrap_err().to_string();
    assert!(err.contains("array format 0x43"), "{err}");
}

// SetRenderTarget, same size ----------------------------------------------------

/// A 64x64 linear target the size of the test back buffer.
fn same_size_target(m: &mut SparseMemory) -> u32 {
    let surf = MISC + 0x7300;
    m.map(0x8000_0000 | 0x0030_0000, 64 * 64 * 4);
    d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, W, H, W * 4, 0x0030_0000)
        .write(m, surf)
        .unwrap();
    surf
}

#[test]
fn same_size_set_render_target_keeps_the_viewport() {
    let (mut d, mut m) = rec();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let depth = m.read_u32(DEV + f::DEPTH_STENCIL).unwrap();
    let surf = same_size_target(&mut m);
    // A smaller viewport the original would leave alone.
    let vp = MISC + 0x300;
    for (i, w) in [8u32, 4, 16, 16, 0.0f32.to_bits(), 1.0f32.to_bits()]
        .iter()
        .enumerate()
    {
        m.write_u32(vp + 4 * i as u32, *w).unwrap();
    }
    d.set_viewport(&mut m, vp).unwrap();
    let before = d.viewport();
    let flags = m.read_u32(DEV + f::FLAGS).unwrap();

    // [esp+4] = target, EBX = depth (none), ret 4.
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), surf);
    assert_eq!(m.read_u32(DEV + f::DEPTH_STENCIL).unwrap(), 0);
    assert_eq!(d.viewport(), before);
    assert_eq!(
        before,
        Viewport {
            x: 8,
            y: 4,
            width: 16,
            height: 16,
            min_z: 0.0,
            max_z: 1.0
        }
    );

    // Back to the back buffer with its depth buffer; flag 0x100 is cleared
    // there (0x3f7c7d), and only there.
    m.write_u32(DEV + f::FLAGS, flags | 0x100).unwrap();
    common::call(
        &mut d,
        &mut m,
        0x3f7b30,
        Regs {
            ebx: depth,
            ..Regs::default()
        },
        &[back],
    )
    .unwrap();
    assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), back);
    assert_eq!(m.read_u32(DEV + f::DEPTH_STENCIL).unwrap(), depth);
    assert_eq!(m.read_u32(DEV + f::FLAGS).unwrap() & 0x100, 0);
    assert_eq!(d.viewport(), before);
}

#[test]
fn same_size_set_render_target_rejects_a_null_target() {
    let (mut d, mut m) = rec();
    let err = common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[0])
        .unwrap_err()
        .to_string();
    assert!(err.contains("null render target"), "{err}");
}

#[test]
fn same_size_set_render_target_writes_a_texture_target_back() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, surf).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[back]).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 1);
}

#[test]
fn same_size_targets_hold_an_internal_reference_while_bound() {
    use d3d::resource::common::INTREFCOUNT_ONE as ONE;
    let (mut d, mut m) = rec();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let surf = same_size_target(&mut m);
    let common = m.read_u32(surf).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), common + ONE);
    // The title lets go of its reference while the surface is bound: the
    // binding keeps it alive.
    let n = common::call(&mut d, &mut m, 0x3fb640, Regs::default(), &[surf]);
    assert_eq!(n.unwrap().eax, Some(0));
    // Unbinding the last reference destroys it, as is (`0x3fb5e9`): the
    // original D3D_DestroyResource gets it from the runtime glue.
    let bound = m.read_u32(surf).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[back]).unwrap();
    assert_eq!(d.take_pending_destroys(), vec![surf]);
    assert_eq!(m.read_u32(surf).unwrap(), bound);
    assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), back);
}

#[test]
fn texture_level_targets_reference_their_parent_once() {
    use d3d::resource::common::INTREFCOUNT_ONE as ONE;
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let depth = m.read_u32(DEV + f::DEPTH_STENCIL).unwrap();
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, surf).unwrap();
    let (s0, p0) = (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap());
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    // Again while bound: the surface gains a reference, the parent not.
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0 + ONE);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0 + ONE);
    // As a depth buffer too (EBX), through the full SetRenderTarget.
    m.write_u32(DEV + f::GPU_TIME, 7).unwrap();
    common::call(
        &mut d,
        &mut m,
        0x3f7b30,
        Regs {
            ebx: depth,
            ..Regs::default()
        },
        &[back],
    )
    .unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0);
    // Lock of the parent (the level's resource) is stamped with the time.
    assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 7);
    d.set_render_target(&mut m, surf, 0).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0 + ONE);
    d.set_render_target(&mut m, back, depth).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0);
}

#[test]
fn native_guest_bind_tracks_levels_and_defers_final_destruction() {
    let (mut d, mut m) = rec();
    d.set_native_residual(true);
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let surf = texture_level(&mut m);
    let parent = m.read_u32(TEX_HEADER).unwrap();
    d.set_render_target_same_size(&mut m, surf, 0).unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), parent + d3d::resource::common::INTREFCOUNT_ONE);
    m.write_u32(DEV + f::GPU_TIME, 19).unwrap();
    d.set_render_target_same_size(&mut m, back, 0).unwrap();
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), parent);
    assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 19);
    let standalone = same_size_target(&mut m);
    d.set_render_target_same_size(&mut m, standalone, 0).unwrap();
    assert_eq!(d.stats().native_guest_binds, 4);
    common::call(&mut d, &mut m, 0x3fb640, Regs::default(), &[standalone]).unwrap();
    d.set_render_target_same_size(&mut m, back, 0).unwrap();
    assert_eq!(d.take_pending_destroys(), vec![standalone]);
    assert_eq!(d.stats().guest_bind_refused_destroy, 1);
    assert_eq!(d.stats().native_guest_binds, 4);
}

/// A 16x4 level of the test texture, with the texture as its parent.
fn texture_level(m: &mut SparseMemory) -> u32 {
    texture(m, 0);
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(m, surf).unwrap();
    surf
}

#[test]
fn teardown_releases_the_bound_title_targets() {
    // A level (parent accounting) and a plain surface, each still bound
    // when the device goes; the caller keeps its own reference.
    for level in [true, false] {
        let (mut d, mut m) = rec();
        let surf = if level {
            texture_level(&mut m)
        } else {
            same_size_target(&mut m)
        };
        let (s0, p0) = (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap());
        common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
        m.write_u32(DEV + f::GPU_TIME, 9).unwrap();
        d.release_device(&mut m, DEV).unwrap();
        assert_eq!(m.read_u32(surf).unwrap(), s0, "level {level}");
        assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0, "level {level}");
        let stamped = if level { TEX_HEADER } else { surf };
        assert_eq!(m.read_u32(stamped + 8).unwrap(), 9, "level {level}");
        assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), 0);
        assert_eq!(m.read_u32(DEV + f::DEPTH_STENCIL).unwrap(), 0);
    }
}

/// Teardown releases a level only the binding still holds: the parent's
/// last reference goes with it, and both are destroyed, parent first
/// (`0x3fb5d9`, then `0x3fb5e9`).
#[test]
fn a_teardown_destroys_what_only_the_binding_held() {
    use d3d::resource::common::INTREFCOUNT_ONE as ONE;
    let (mut d, mut m) = rec();
    let surf = texture_level(&mut m);
    let p0 = m.read_u32(TEX_HEADER).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    // Releasing the level's last title reference releases the parent's
    // too (`0x3fb668`); the binding keeps both.
    common::call(&mut d, &mut m, 0x3fb640, Regs::default(), &[surf]).unwrap();
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0 - 1 + ONE);
    let s = m.read_u32(surf).unwrap();
    assert!(d.take_pending_destroys().is_empty());
    d.release_device(&mut m, DEV).unwrap();
    assert_eq!(d.take_pending_destroys(), vec![TEX_HEADER, surf]);
    assert_eq!(m.read_u32(surf).unwrap(), s, "destroyed as it is");
    assert_eq!(
        m.read_u32(TEX_HEADER).unwrap(),
        p0 - 1,
        "decremented, then destroyed"
    );
    assert_eq!(d.stats().destroys, 2);
}

/// Unbinding a level only the binding holds destroys it and its parent,
/// writes its pixels back first, and drops its host surface.
#[test]
fn an_unbind_destroys_a_level_only_the_binding_held() {
    use d3d::resource::common::INTREFCOUNT_ONE as ONE;
    let (mut d, mut m) = rec();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let surf = texture_level(&mut m);
    let (s0, p0) = (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap());
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    let live = d.backend.live_surfaces();
    common::call(&mut d, &mut m, 0x3fb640, Regs::default(), &[surf]).unwrap();
    assert_eq!(
        (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap()),
        (s0 - 1 + ONE, p0 - 1 + ONE)
    );
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[back]).unwrap();
    assert_eq!(d.take_pending_destroys(), vec![TEX_HEADER, surf]);
    assert_eq!(d.stats().rtt_writebacks, 1);
    assert_eq!(
        d.backend.live_surfaces(),
        live - 1,
        "the level's host surface"
    );
    assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), back);
}

#[test]
fn commit_keeps_the_bits_set_state_vb_consumes() {
    let (mut d, mut m) = rec();
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.dirty_flags, 0xFFFF_FFFF).unwrap();
    common::call(&mut d, &mut m, 0x400fa0, Regs::default(), &[]).unwrap();
    assert_eq!(m.read_u32(l.dirty_flags).unwrap(), 0xC000_0070);
    // Shader selection sets the vertex bits; a commit before BeginPush
    // leaves them for the native SetStateVB.
    m.write_u32(l.dirty_flags, 0).unwrap();
    program(&mut d, &mut m);
    common::call(&mut d, &mut m, 0x400fa0, Regs::default(), &[]).unwrap();
    assert_eq!(m.read_u32(l.dirty_flags).unwrap() & 0x70, 0x70);
}

// The NV2A register window ------------------------------------------------------

use d3d::regs::{self, reg};
use std::collections::{BTreeMap, BTreeSet};

/// The window base the synthetic tests use (the common memory leaves it
/// free).
const WINDOW: u32 = 0x0500_0000;

fn windowed() -> (Device<RecordingBackend>, SparseMemory) {
    let mut m = memory();
    write_params(&mut m);
    for p in regs::RegisterWindow::new(WINDOW).unwrap().pages() {
        m.map(p.addr, p.len);
    }
    let mut d = Device::new(
        d3d::DeviceConfig {
            nv2a_window: Some(WINDOW),
            ..d3d::DeviceConfig::halo2(PB, SCRATCH)
        },
        RecordingBackend::new(),
    );
    common::call(
        &mut d,
        &mut m,
        0x3f5240,
        Regs {
            eax: 0x40,
            ecx: PP_DEVICE,
            ..Regs::default()
        },
        &[PARAMS],
    )
    .unwrap();
    (d, m)
}

fn reg_word(m: &SparseMemory, r: u32) -> u32 {
    m.read_u32(WINDOW + r).unwrap()
}

#[test]
fn create_device_points_0x934_at_an_idle_gpu() {
    let (d, m) = windowed();
    assert_eq!(m.read_u32(DEV + f::NV2A_REGISTERS).unwrap(), WINDOW);
    assert_eq!(d.register_window(), Some(WINDOW));
    let put = PB.addr & 0x0FFF_FFFF;
    assert_eq!(reg_word(&m, reg::PFIFO_CACHE1_DMA_PUT), put);
    assert_eq!(reg_word(&m, reg::PFIFO_CACHE1_DMA_GET), put);
    assert_eq!(reg_word(&m, reg::PGRAPH_STATUS), 0);
    // Without a window the field is left alone.
    let (_, plain) = rec();
    assert_eq!(plain.read_u32(DEV + f::NV2A_REGISTERS).unwrap(), 0);
}

#[test]
fn kick_off_leaves_put_equal_to_get_at_the_drained_position() {
    let (mut d, mut m) = windowed();
    // A few words of inline state, then KickOff (EDX live, no args).
    stipple_row(&mut m);
    let put = m.read_u32(DEV).unwrap();
    assert_ne!(put, PB.addr);
    common::call(&mut d, &mut m, 0x3fac30, Regs::default(), &[]).unwrap();
    assert_eq!(reg_word(&m, reg::PFIFO_CACHE1_DMA_PUT), put & 0x0FFF_FFFF);
    assert_eq!(reg_word(&m, reg::PFIFO_CACHE1_DMA_GET), put & 0x0FFF_FFFF);
    assert_eq!(reg_word(&m, reg::PGRAPH_STATUS), 0);
}

fn stipple_row(m: &mut SparseMemory) {
    push_words(m, &[header(0x1480, 1), 0xAAAA_AAAA]);
}

#[test]
fn the_fence_register_follows_set_fence() {
    let (mut d, mut m) = windowed();
    for _ in 0..3 {
        stipple_row(&mut m);
        common::call(&mut d, &mut m, 0x3fa680, Regs::default(), &[0]).unwrap();
    }
    let at = m.read_u32(DEV).unwrap();
    let completed = m
        .read_u32(m.read_u32(DEV + f::GPU_COMPLETED).unwrap())
        .unwrap();
    assert_eq!(completed, 6);
    let fence = reg_word(&m, reg::PGRAPH_FENCE);
    // What BlockOnTime (0x3fa910) waits for: bits 6:2 equal to the
    // completion word's low bits.
    assert_eq!((fence ^ (completed << 2)) & 0x7C, 0);
    // What MakeRequestedSpace (0x3fa8bb) reads back: the address the GPU
    // fetched to, which is where the last fence was issued.
    assert_eq!(
        ((fence >> 5) & 0x07FF_FFFC) | 0x8000_0000,
        (at & 0x07FF_FFFC) | 0x8000_0000
    );
    assert_eq!(fence, regs::fence_word(completed, at, 0));
}

#[test]
fn guest_stores_into_the_window_are_logged_and_undone() {
    let (mut d, mut m) = windowed();
    d.take_events();
    m.write_u32(WINDOW + reg::PGRAPH_STATUS, 1).unwrap();
    m.write_u32(WINDOW + reg::PFIFO_CACHE1_DMA_PUT, 0x1234)
        .unwrap();
    common::call(&mut d, &mut m, 0x3fac30, Regs::default(), &[]).unwrap();
    let events = d.take_events();
    assert_eq!(
        events,
        vec![
            d3d::Event::RegisterWrite {
                offset: reg::PFIFO_CACHE1_DMA_PUT,
                value: 0x1234
            },
            d3d::Event::RegisterWrite {
                offset: reg::PGRAPH_STATUS,
                value: 1
            },
        ]
    );
    assert_eq!(reg_word(&m, reg::PGRAPH_STATUS), 0);
    assert_eq!(
        reg_word(&m, reg::PFIFO_CACHE1_DMA_PUT),
        m.read_u32(DEV).unwrap() & 0x0FFF_FFFF
    );
}

#[test]
fn the_window_survives_release_and_a_second_create() {
    let (mut d, mut m) = windowed();
    // Halo's boot: teardown (EAX = device), the game zeroes the device
    // object, then CreateDevice again.
    common::call(
        &mut d,
        &mut m,
        0x3fc530,
        Regs {
            eax: DEV,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    m.write(DEV, &vec![0; 0x2480]).unwrap();
    assert_eq!(m.read_u32(DEV + f::NV2A_REGISTERS).unwrap(), 0);
    common::call(
        &mut d,
        &mut m,
        0x3f5240,
        Regs {
            eax: 0x40,
            ecx: PP_DEVICE,
            ..Regs::default()
        },
        &[PARAMS],
    )
    .unwrap();
    assert_eq!(m.read_u32(DEV + f::NV2A_REGISTERS).unwrap(), WINDOW);
    assert!(d.take_events().is_empty());
}

#[test]
fn a_misplaced_window_fails_create_device() {
    let mut m = memory();
    write_params(&mut m);
    let mut d = Device::new(
        d3d::DeviceConfig {
            nv2a_window: Some(WINDOW + 4),
            ..d3d::DeviceConfig::halo2(PB, SCRATCH)
        },
        RecordingBackend::new(),
    );
    let err = d
        .create_device(&mut m, 0x40, PARAMS, PP_DEVICE)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not page-aligned"), "{err}");
}

// Runtime: the window as mapped memory ------------------------------------------

mod runtime_level {
    use super::*;
    use runtime::exec::HARNESS_EXIT;
    use runtime::guest::Permissions;
    use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, abi as rt};

    /// Push buffer, scratch, a parameter and stack page, and the window, in
    /// a `Guest` with the D3D code pages executable and the D3D globals.
    pub fn guest(cfg: &d3d::DeviceConfig) -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x003F_0000, 0x1_4000, Permissions::RX)
            .unwrap();
        g.map_zeroed(0x0040_4000, 0x5000, Permissions::RW).unwrap();
        let encode = d3d::GuestLayout::HALO2.simple_encode;
        g.map_zeroed(encode & !0xFFF, 0x1000, Permissions::RW)
            .unwrap();
        g.write_bytes(encode, &synthetic_encode_bytes(), 1).unwrap();
        g.map_zeroed(MISC, 0x1_0000, Permissions::RW).unwrap();
        d3d::runtime_glue::map_device_memory(&mut g, cfg).unwrap();
        let mut params = SparseMemory::new();
        params.map(PARAMS, 0x100);
        write_params(&mut params);
        let mut bytes = vec![0; d3d::PresentParameters::SIZE as usize];
        GuestMemory::read(&params, PARAMS, &mut bytes).unwrap();
        g.write_bytes(PARAMS, &bytes, 1).unwrap();
        g
    }

    pub fn call(ctx: &mut ExecCtx, entry: u32, mut cpu: CpuAbi, stack: &[u32]) -> CpuAbi {
        let top = MISC + 0xF000;
        let esp = top - 4 - 4 * stack.len() as u32;
        {
            let mut m = ctx.memory();
            m.set_u32(esp, HARNESS_EXIT).unwrap();
            for (i, v) in stack.iter().enumerate() {
                m.set_u32(esp + 4 + 4 * i as u32, *v).unwrap();
            }
        }
        cpu.esp = esp;
        let t = Runner::default().run(&mut cpu, ctx, entry);
        assert_eq!(t.kind, rt::STOP, "{entry:#x}: {t:?}");
        assert_eq!(
            ctx.event(t.event_id),
            Some(&Event::HarnessExit),
            "{entry:#x}"
        );
        cpu
    }

    #[test]
    fn the_window_is_guest_memory_the_runtime_reads() {
        let cfg = d3d::DeviceConfig {
            nv2a_window: Some(WINDOW),
            ..d3d::DeviceConfig::halo2(PB, SCRATCH)
        };
        let g = guest(&cfg);
        let device = d3d::runtime_glue::shared(cfg, RecordingBackend::new());
        let mut dispatch = Dispatch::default();
        d3d::register(&mut dispatch, device.clone());
        let mut ctx = ExecCtx::new(Runtime::new(g, dispatch), 1);
        let cpu = CpuAbi {
            eax: 0x40,
            ecx: PP_DEVICE,
            ..CpuAbi::default()
        };
        call(&mut ctx, 0x3f5240, cpu, &[PARAMS]);
        // What the game sites do after KickOff (0x12d0c4..0x12d0e3):
        // base = [D3D__pDevice] + 0x934, then CACHE1 put/get and PGRAPH.
        call(&mut ctx, 0x3fac30, CpuAbi::default(), &[]);
        let m = ctx.memory();
        let dev = m.u32(d3d::GuestLayout::HALO2.p_device).unwrap();
        let base = m.u32(dev + f::NV2A_REGISTERS).unwrap();
        assert_eq!(base, WINDOW);
        assert_eq!(
            m.u32(base + reg::PFIFO_CACHE1_DMA_PUT).unwrap(),
            m.u32(base + reg::PFIFO_CACHE1_DMA_GET).unwrap()
        );
        assert_eq!(m.u32(base + reg::PGRAPH_STATUS).unwrap(), 0);
        // A register page the model does not back faults instead of
        // reading as zero, and the real aperture stays a device fault.
        assert!(m.u32(base + 0x60_0100).is_err());
        assert!(m.u32(regs::RETAIL_BASE + reg::PGRAPH_STATUS).is_err());
    }
}

// The retail image ---------------------------------------------------------------

/// Every `device+0x934` site in the retail image, found by decoding it.
mod image {
    use std::collections::{BTreeMap, BTreeSet};

    use iced_x86::{
        Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Mnemonic,
        OpAccess, OpKind, Register,
    };

    /// `CDevice_KickOff`, which every game site calls first.
    pub const KICK_OFF: u32 = 0x3fac30;
    /// Game sites read the base after `mov reg, [D3D__pDevice]` and a call.
    pub const LOOKBACK: usize = 3;
    /// Instructions followed after the base load.
    pub const LOOKAHEAD: usize = 16;

    pub struct Image<'a> {
        pub xbe: xbe::Xbe<'a>,
        pub analysis: analyze::Analysis,
        pub d3d: (u32, u32),
    }

    /// A register access through a base read from device `+0x934`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Access {
        pub at: u32,
        pub offset: u32,
        pub write: bool,
    }

    #[derive(Clone, Debug)]
    pub struct Site {
        /// The `mov r, [base + 0x934]`.
        pub at: u32,
        pub function: u32,
        /// The `mov base, [D3D__pDevice]` (game sites).
        pub device_load: Option<u32>,
        /// A call to `KickOff` between the two.
        pub kicks: bool,
        pub accesses: Vec<Access>,
        /// Where the tracked window ends: the instruction after the one
        /// that overwrites the base, or the call or jump that leaves.
        pub end: u32,
    }

    impl<'a> Image<'a> {
        pub fn new(bytes: &'a [u8]) -> Self {
            let xbe = xbe::Xbe::parse(bytes).unwrap();
            let analysis = analyze::analyze(&xbe).unwrap();
            let s = xbe.section("D3D").expect("D3D section");
            let d3d = (s.va, s.va + s.vsize);
            Self { xbe, analysis, d3d }
        }

        pub fn in_d3d(&self, va: u32) -> bool {
            (self.d3d.0..self.d3d.1).contains(&va)
        }

        pub fn decode(&self, va: u32) -> Instruction {
            let s = self.xbe.section_containing(va).unwrap();
            let avail = (s.va + s.raw_size.min(s.vsize) - va) as usize;
            let bytes = self.xbe.bytes_at(va, avail.min(16)).unwrap();
            Decoder::with_ip(32, bytes, va as u64, DecoderOptions::NONE).decode()
        }

        /// Every instruction the analysis claims, with its function.
        pub fn instructions(&self) -> Vec<(Instruction, Option<u32>)> {
            let mut out = Vec::new();
            for (start, len, _) in &self.analysis.ranges {
                let mut va = *start;
                while va < start + len {
                    match self.analysis.claim(va) {
                        Some(('I', owner)) => {
                            let i = self.decode(va);
                            va += i.len() as u32;
                            out.push((i, owner));
                        }
                        _ => va += 1,
                    }
                }
            }
            out
        }

        /// The instruction before `va` in the same function, by address.
        fn previous(&self, va: u32, function: u32) -> Option<Instruction> {
            (1..16)
                .map(|k| va - k)
                .find_map(|a| match self.analysis.claim(a) {
                    Some(('I', Some(f))) if f == function => Some(self.decode(a)),
                    _ => None,
                })
        }
    }

    fn mem_operand(i: &Instruction) -> bool {
        (0..i.op_count()).any(|k| i.op_kind(k) == OpKind::Memory)
    }

    fn writes(info: &mut InstructionInfoFactory, i: &Instruction, r: Register) -> bool {
        info.info(i).used_registers().iter().any(|u| {
            u.register().full_register32() == r
                && matches!(
                    u.access(),
                    OpAccess::Write
                        | OpAccess::CondWrite
                        | OpAccess::ReadWrite
                        | OpAccess::ReadCondWrite
                )
        })
    }

    /// Every load of device `+0x934` into a register: game sites (base from
    /// `D3D__pDevice` just before) and D3D-section sites (any base: the
    /// device is the only `+0x934` there).
    pub fn sites(img: &Image, p_device: u32) -> Vec<Site> {
        let mut info = InstructionInfoFactory::new();
        let mut out = Vec::new();
        for (i, owner) in img.instructions() {
            let va = i.ip32();
            let Some(function) = owner else { continue };
            if !(i.mnemonic() == Mnemonic::Mov
                && i.op_count() == 2
                && i.op0_kind() == OpKind::Register
                && i.op1_kind() == OpKind::Memory
                && i.memory_base() != Register::None
                && i.memory_index() == Register::None
                && i.memory_displacement32() == 0x934)
            {
                continue;
            }
            let base = i.memory_base();
            let mut device_load = None;
            let mut kicks = false;
            let mut at = va;
            for _ in 0..LOOKBACK {
                let Some(p) = img.previous(at, function) else {
                    break;
                };
                at = p.ip32();
                if p.flow_control() == FlowControl::Call {
                    kicks |= p.near_branch32() == KICK_OFF;
                    continue;
                }
                if writes(&mut info, &p, base) {
                    let from_device = p.mnemonic() == Mnemonic::Mov
                        && p.op1_kind() == OpKind::Memory
                        && p.memory_base() == Register::None
                        && p.memory_displacement32() == p_device;
                    device_load = from_device.then_some(at);
                    break;
                }
            }
            if device_load.is_none() && !img.in_d3d(va) {
                continue; // some other object's +0x934
            }
            let dest = i.op0_register().full_register32();
            let (accesses, end) = follow(img, &mut info, va + i.len() as u32, dest);
            out.push(Site {
                at: va,
                function,
                device_load,
                kicks,
                accesses,
                end,
            });
        }
        out
    }

    /// Accesses through `base` from `va` until `base` is overwritten or
    /// control leaves the straight line (conditional branches are followed
    /// by falling through: the sites' only one skips a single read).
    fn follow(
        img: &Image,
        info: &mut InstructionInfoFactory,
        mut va: u32,
        base: Register,
    ) -> (Vec<Access>, u32) {
        let mut out = Vec::new();
        for _ in 0..LOOKAHEAD {
            let i = img.decode(va);
            if mem_operand(&i) && i.memory_base() == base && i.memory_index() == Register::None {
                let used = info.info(&i).used_memory().to_vec();
                let write = used.iter().any(|m| {
                    m.base() == base
                        && matches!(
                            m.access(),
                            OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite
                        )
                });
                out.push(Access {
                    at: va,
                    offset: i.memory_displacement32(),
                    write,
                });
            }
            let leaves = !matches!(
                i.flow_control(),
                FlowControl::Next | FlowControl::ConditionalBranch
            );
            if leaves {
                return (out, va);
            }
            if writes(info, &i, base) {
                // Its own access (`mov eax, [eax+400700h]`) is in the window.
                return (out, va + i.len() as u32);
            }
            va += i.len() as u32;
        }
        (out, va)
    }

    /// Who refers to each address of the D3D section: the functions whose
    /// code calls, jumps to, or takes it as an operand, and whether any
    /// non-code word holds it.
    pub struct Refs {
        pub code: BTreeMap<u32, BTreeSet<u32>>,
        pub data: BTreeSet<u32>,
    }

    pub fn refs(img: &Image) -> Refs {
        let mut code: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
        for (i, owner) in img.instructions() {
            for k in 0..i.op_count() {
                let v = match i.op_kind(k) {
                    OpKind::NearBranch32 => i.near_branch32(),
                    OpKind::Immediate32 => i.immediate32(),
                    OpKind::Memory => i.memory_displacement32(),
                    _ => continue,
                };
                if img.in_d3d(v) {
                    code.entry(v).or_default().insert(owner.unwrap_or(0));
                }
            }
        }
        let mut data = BTreeSet::new();
        for s in &img.xbe.sections {
            let Ok(b) = img.xbe.section_bytes(s) else {
                continue;
            };
            for (k, w) in b.as_chunks::<4>().0.iter().enumerate() {
                let at = s.va + 4 * k as u32;
                let v = u32::from_le_bytes(*w);
                if img.in_d3d(v)
                    && !matches!(img.analysis.claim(at), Some(('I' | 'i', _)))
                    && !matches!(img.analysis.claim(at + 3), Some(('I' | 'i', _)))
                {
                    data.insert(v);
                }
            }
        }
        Refs { code, data }
    }

    /// Whether `f` only runs under an override: it is one, or everything
    /// that refers to it does (transitively), and no data word holds it.
    pub fn only_under_overrides(refs: &Refs, overridden: &BTreeSet<u32>, f: u32) -> bool {
        fn walk(refs: &Refs, o: &BTreeSet<u32>, f: u32, seen: &mut BTreeSet<u32>) -> bool {
            if o.contains(&f) {
                return true;
            }
            if refs.data.contains(&f) || !seen.insert(f) {
                return !refs.data.contains(&f);
            }
            let Some(by) = refs.code.get(&f) else {
                return true; // unreferenced: unreachable
            };
            by.iter().all(|&g| g != 0 && walk(refs, o, g, seen))
        }
        walk(refs, overridden, f, &mut BTreeSet::new())
    }
}

fn retail_xbe() -> Vec<u8> {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    std::fs::read(path).unwrap()
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn every_0x934_site_reads_only_modelled_registers() {
    let bytes = retail_xbe();
    let img = image::Image::new(&bytes);
    let p_device = d3d::GuestLayout::HALO2.p_device;
    let sites = image::sites(&img, p_device);
    let (game, internal): (Vec<_>, Vec<_>) = sites.iter().partition(|s| !img.in_d3d(s.at));
    let list = |v: &[&image::Site]| v.iter().map(|s| format!("{:#x}", s.at)).collect::<Vec<_>>();
    eprintln!("game sites ({}): {:?}", game.len(), list(&game));
    eprintln!("D3D sites ({}): {:?}", internal.len(), list(&internal));

    // Game code: 17 sites, each `mov reg, [D3D__pDevice]`, `call KickOff`,
    // then the base load, and only reads of the modelled registers.
    assert_eq!(game.len(), 17);
    for s in &game {
        assert!(s.device_load.is_some() && s.kicks, "{:#x}", s.at);
        let offsets: BTreeSet<u32> = s.accesses.iter().map(|a| a.offset).collect();
        assert_eq!(
            offsets,
            BTreeSet::from([
                reg::PFIFO_CACHE1_DMA_PUT,
                reg::PFIFO_CACHE1_DMA_GET,
                reg::PGRAPH_STATUS
            ]),
            "{:#x}",
            s.at
        );
        assert!(s.accesses.iter().all(|a| !a.write), "{:#x} writes", s.at);
    }

    // D3D's own sites read the same registers plus the fence register, and
    // only run under an override.
    let overridden: BTreeSet<u32> = d3d::abi::entries::<RecordingBackend>()
        .iter()
        .map(|e| e.addr)
        .collect();
    let refs = image::refs(&img);
    assert!(!internal.is_empty());
    for s in &internal {
        for a in &s.accesses {
            assert!(reg::ALL.contains(&a.offset), "{:#x}: {:#x}", a.at, a.offset);
        }
        assert!(
            image::only_under_overrides(&refs, &overridden, s.function),
            "{:#x} (in {:#x}) is reachable without an override",
            s.at,
            s.function
        );
    }
    let all: BTreeSet<u32> = sites
        .iter()
        .flat_map(|s| s.accesses.iter().map(|a| a.offset))
        .collect();
    assert_eq!(all, BTreeSet::from(reg::ALL));

    // The other direction: every game-code access at a modelled register
    // offset is one the sites above make.
    let reached: BTreeSet<u32> = game
        .iter()
        .flat_map(|s| s.accesses.iter().map(|a| a.at))
        .collect();
    for (i, _) in img.instructions() {
        if img.in_d3d(i.ip32()) {
            continue;
        }
        let at_register = (0..i.op_count()).any(|k| i.op_kind(k) == iced_x86::OpKind::Memory)
            && i.memory_base() != iced_x86::Register::None
            && reg::ALL.contains(&i.memory_displacement32());
        if at_register {
            assert!(
                reached.contains(&i.ip32()),
                "{:#x} is not under a site",
                i.ip32()
            );
        }
    }
}

/// [`ir::interp::Memory`] over the runtime's guest memory.
struct RuntimeMemory<'a>(&'a runtime::Runtime);

impl ir::interp::Memory for RuntimeMemory<'_> {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ir::interp::MemFault> {
        self.0
            .memory()
            .read_bytes(addr, buf, 1)
            .map_err(|_| ir::interp::MemFault { addr, write: false })
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), ir::interp::MemFault> {
        self.0
            .memory()
            .write_bytes(addr, data, 1)
            .map_err(|_| ir::interp::MemFault { addr, write: true })
    }
}

/// The game sites as the boot runs them: the retail image loaded into the
/// runtime with the D3D overrides and a window, CreateDevice through the
/// runner, then each site's lifted IR from its `D3D__pDevice` load on the
/// IR interpreter, with the `KickOff` call going through the runner to the
/// override. Every register read must execute (the PGRAPH read only runs
/// when `DMA_PUT == DMA_GET`) and none may fault.
#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn game_sites_find_an_idle_gpu_through_lifted_code() {
    use ir::Gpr;
    use runtime::exec::HARNESS_EXIT;
    use runtime::guest::Permissions;
    use runtime::{CpuAbi, Event, ExecCtx, Runner, abi as rt};

    let bytes = retail_xbe();
    let img = image::Image::new(&bytes);
    let p_device = d3d::GuestLayout::HALO2.p_device;
    let sites: Vec<_> = image::sites(&img, p_device)
        .into_iter()
        .filter(|s| !img.in_d3d(s.at))
        .collect();
    let lifter = lift::Lifter::new(&img.analysis, &img.xbe).unwrap();

    // Host-chosen ranges, clear of the image.
    let pb = d3d::GuestRange {
        addr: 0x7000_0000,
        len: 0x4_0000,
    };
    let scratch = d3d::GuestRange {
        addr: 0x7004_0000,
        len: 0x1000,
    };
    let misc = 0x7005_0000u32;
    let (params, pp_device, stack) = (misc, misc + 0x100, misc + 0xF000);
    let window = 0x7100_0000;
    let cfg = d3d::DeviceConfig {
        upload: UploadPolicy::Always,
        nv2a_window: Some(window),
        ..d3d::DeviceConfig::halo2(pb, scratch)
    };
    let image = runtime::load_xbe(&bytes).unwrap();
    let rt = image.runtime.clone();
    {
        let mut g = rt.memory();
        d3d::runtime_glue::map_device_memory(&mut g, &cfg).unwrap();
        g.map_zeroed(misc, 0x1_0000, Permissions::RW).unwrap();
        let mut mem = d3d::runtime_glue::GuestAdapter::new(&mut g);
        d3d::PresentParameters {
            width: 64,
            height: 64,
            back_buffer_format: 0x06,
            back_buffer_count: 1,
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
        .write(&mut mem, params)
        .unwrap();
    }
    let device = d3d::runtime_glue::shared(cfg, RecordingBackend::new());
    rt.edit_dispatch(|d| {
        d3d::register(d, device.clone());
    });
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let run = |ctx: &mut ExecCtx, entry: u32, mut cpu: CpuAbi, esp: u32| -> CpuAbi {
        ctx.memory().set_u32(esp, HARNESS_EXIT).unwrap();
        cpu.esp = esp;
        let t = Runner::default().run(&mut cpu, ctx, entry);
        assert_eq!(t.kind, rt::STOP, "{entry:#x}: {t:?}");
        assert_eq!(
            ctx.event(t.event_id),
            Some(&Event::HarnessExit),
            "{entry:#x}"
        );
        cpu
    };
    ctx.memory().set_u32(stack - 4, params).unwrap();
    let cpu = CpuAbi {
        eax: 0x40,
        ecx: pp_device,
        ..CpuAbi::default()
    };
    let cpu = run(&mut ctx, 0x3f5240, cpu, stack - 8);
    assert_eq!(cpu.eax, 0, "CreateDevice");

    for s in &sites {
        let f = lifter.lift(s.function).expect("lifted");
        let insns: BTreeMap<u32, ir::IrInsn> = f
            .blocks
            .into_values()
            .flat_map(|b| b.insns)
            .map(|i| (i.pc, i))
            .collect();
        let mut cpu = ir::interp::Cpu::default();
        cpu.set_reg(Gpr::Esp, stack);
        let mut mem = RuntimeMemory(&rt);
        let mut pc = s.device_load.unwrap();
        let mut executed = BTreeSet::new();
        let mut kicked = false;
        for _ in 0..64 {
            if pc == s.end {
                break;
            }
            let insn = insns
                .get(&pc)
                .unwrap_or_else(|| panic!("{pc:#x} not lifted"));
            executed.insert(pc);
            match ir::interp::step(&mut cpu, &mut mem, insn) {
                Ok(ir::interp::Flow::Call {
                    target: image::KICK_OFF,
                    continuation,
                }) => {
                    // The interpreter pushed the continuation; the override
                    // returns to the harness instead.
                    let esp = cpu.reg(Gpr::Esp);
                    let abi = CpuAbi {
                        eax: cpu.reg(Gpr::Eax),
                        ecx: cpu.reg(Gpr::Ecx),
                        edx: cpu.reg(Gpr::Edx),
                        ebx: cpu.reg(Gpr::Ebx),
                        ebp: cpu.reg(Gpr::Ebp),
                        esi: cpu.reg(Gpr::Esi),
                        edi: cpu.reg(Gpr::Edi),
                        ..CpuAbi::default()
                    };
                    let out = run(&mut ctx, image::KICK_OFF, abi, esp);
                    for (r, v) in [
                        (Gpr::Eax, out.eax),
                        (Gpr::Ecx, out.ecx),
                        (Gpr::Edx, out.edx),
                        (Gpr::Ebx, out.ebx),
                        (Gpr::Ebp, out.ebp),
                        (Gpr::Esi, out.esi),
                        (Gpr::Edi, out.edi),
                    ] {
                        cpu.set_reg(r, v);
                    }
                    cpu.set_reg(Gpr::Esp, esp + 4);
                    kicked = true;
                    pc = continuation;
                }
                Ok(flow) => pc = flow.target(),
                Err(e) => panic!("site {:#x}: {e:?}", s.at),
            }
        }
        assert_eq!(pc, s.end, "site {:#x} ran past its window", s.at);
        assert!(kicked, "site {:#x}", s.at);
        for a in &s.accesses {
            assert!(
                executed.contains(&a.at),
                "site {:#x}: the read at {:#x} ({:#x}) did not run",
                s.at,
                a.at,
                a.offset
            );
        }
    }
    let d = device.lock().unwrap();
    assert!(d.last_stop().is_none(), "{:?}", d.last_stop());
    assert_eq!(d.register_window(), Some(window));
}
