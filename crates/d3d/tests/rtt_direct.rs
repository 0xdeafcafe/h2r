//! Lazy render-to-texture: draws sample a level's host surface in place,
//! and its pixels reach guest memory only when the guest is about to read
//! them. Each GPU scenario runs eagerly (write-back at every target change
//! and through an upload) and lazily, and must give the same back buffer
//! and, once everything is written back, the same guest memory. The
//! recording backend checks when write-backs happen. Synthetic resources;
//! the GPU tests skip without an adapter.

mod common;

use common::*;
use d3d::{Device, Event, GfxBackend, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};
use gfx::vertex::fvf;

const RS_ALPHABLENDENABLE: u32 = 59;
const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;
const RECTS: u32 = MISC + 0x7100;

/// Two 32x32 textures with a level each: A linear (`LIN_A8R8G8B8`, pitch
/// 128), B swizzled (`A8R8G8B8`).
const TEX_A: u32 = MISC + 0x4000;
const TEX_B: u32 = MISC + 0x4100;
const LEVEL_A: u32 = MISC + 0x4200;
const LEVEL_B: u32 = MISC + 0x4300;
const DATA_A: u32 = 0x0030_0000;
const DATA_B: u32 = 0x0031_0000;
const SIZE: u32 = 32;
const LEN: usize = (SIZE * SIZE * 4) as usize;

/// Format word: 2D, one level, `format`, and for a swizzled one the
/// log2 size.
fn format_word(format: u32, swizzled: bool) -> u32 {
    let log2 = if swizzled { (5 << 20) | (5 << 24) } else { 0 };
    (2 << 4) | (format << 8) | (1 << 16) | log2
}

fn size_word(swizzled: bool) -> u32 {
    if swizzled {
        0
    } else {
        (SIZE - 1) | ((SIZE - 1) << 12) | ((128 / 64 - 1) << 24)
    }
}

/// Texture `header` and its level `level` over `data`, which holds
/// `fill` in every byte.
fn level(m: &mut SparseMemory, header: u32, level: u32, data: u32, swizzled: bool, fill: u8) {
    m.map(0x8000_0000 | data, LEN as u32);
    m.write(0x8000_0000 | data, &vec![fill; LEN]).unwrap();
    let format = if swizzled { 0x06 } else { 0x12 };
    for (i, w) in [
        1 | (4 << 16) | 0x0100_0000,
        data,
        0,
        format_word(format, swizzled),
        size_word(swizzled),
    ]
    .iter()
    .enumerate()
    {
        m.write_u32(header + 4 * i as u32, *w).unwrap();
    }
    for (i, w) in [
        1 | (5 << 16) | 0x0100_0000,
        data,
        0,
        format_word(format, swizzled),
        size_word(swizzled),
        header,
    ]
    .iter()
    .enumerate()
    {
        m.write_u32(level + 4 * i as u32, *w).unwrap();
    }
}

/// An `XYZRHW | DIFFUSE | TEX1` quad over `rect` of the target, reading
/// the whole of a 32x32 texture (texel coordinates for a linear one,
/// normalised for a swizzled one), in diffuse colour `color`.
fn textured_quad(
    d: &mut Device<GfxBackend>,
    m: &mut SparseMemory,
    rect: (f32, f32, f32, f32),
    swizzled: bool,
    color: u32,
) {
    let (x0, y0, x1, y1) = rect;
    let scale = if swizzled { 1.0 } else { SIZE as f32 };
    let mut vb = Vec::new();
    for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [x0 + (x1 - x0) * u, y0 + (y1 - y0) * v, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&color.to_le_bytes());
        for f in [scale * u, scale * v] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

/// Clears `rect` (x1, y1, x2, y2) of the target to `color`.
fn clear_rect(d: &mut Device<GfxBackend>, m: &mut SparseMemory, rect: [u32; 4], color: u32) {
    m.write(RECTS, &rect.map(u32::to_le_bytes).concat())
        .unwrap();
    d.clear(m, 1, RECTS, 0xF0, color, 1.0, 0).unwrap();
}

fn gpu(lazy: bool) -> Option<(Device<GfxBackend>, SparseMemory)> {
    match gfx::GfxContext::headless() {
        Ok(ctx) => {
            let (mut d, mut m) = create(GfxBackend::new(ctx), UploadPolicy::Tracked);
            d.set_rtt_direct(lazy);
            set_states(&mut d, &mut m);
            level(&mut m, TEX_A, LEVEL_A, DATA_A, false, 0x30);
            level(&mut m, TEX_B, LEVEL_B, DATA_B, true, 0x90);
            Some((d, m))
        }
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

/// What a scenario leaves: the back buffer, and both levels' guest bytes
/// once every target is written back.
#[derive(Debug, PartialEq)]
struct Outcome {
    back: Vec<u8>,
    a: Vec<u8>,
    b: Vec<u8>,
}

fn outcome(d: &mut Device<GfxBackend>, m: &mut SparseMemory) -> Outcome {
    let events = d.take_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_))),
        "{events:?}"
    );
    let (back, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
    d.flush_render_targets(m).unwrap();
    Outcome {
        back,
        a: m.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
        b: m.read_vec(0x8000_0000 | DATA_B, LEN).unwrap(),
    }
}

/// Ping-pong between the two levels, each pass sampling the other, with
/// blending and partial coverage, then both sampled into the back buffer.
fn ping_pong(lazy: bool) -> Option<(Outcome, d3d::Stats)> {
    let (mut d, mut m) = gpu(lazy)?;
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    // A: part cleared red, part green; the rest stays 0x30 from memory.
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    clear_rect(&mut d, &mut m, [0, 0, 16, 32], 0xFFFF_0000);
    clear_rect(&mut d, &mut m, [16, 8, 24, 24], 0x8000_FF00);
    // B: the top half samples A, tinted.
    d.set_render_target(&mut m, LEVEL_B, 0).unwrap();
    d.set_texture(&mut m, 0, TEX_A).unwrap();
    textured_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 16.0), false, 0xFF80_C0FF);
    // A again: B blended over its lower right.
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    d.set_texture(&mut m, 0, TEX_B).unwrap();
    for (slot, v) in [
        (RS_ALPHABLENDENABLE, 1),
        (RS_SRCBLEND, 0x302),
        (RS_DESTBLEND, 0x303),
    ] {
        d.set_render_state(&mut m, slot, v).unwrap();
    }
    textured_quad(&mut d, &mut m, (8.0, 8.0, 32.0, 32.0), true, 0xC0FF_FFFF);
    d.set_render_state(&mut m, RS_ALPHABLENDENABLE, 0).unwrap();
    // B again, from A.
    d.set_render_target(&mut m, LEVEL_B, 0).unwrap();
    d.set_texture(&mut m, 0, TEX_A).unwrap();
    textured_quad(&mut d, &mut m, (0.0, 16.0, 32.0, 32.0), false, 0xFFFF_FFFF);
    // The back buffer: A on the left, B on the right.
    d.set_render_target(&mut m, back, 0).unwrap();
    d.set_texture(&mut m, 0, TEX_A).unwrap();
    textured_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), false, 0xFFFF_FFFF);
    d.set_texture(&mut m, 0, TEX_B).unwrap();
    textured_quad(&mut d, &mut m, (32.0, 0.0, 64.0, 32.0), true, 0xFFFF_FFFF);
    d.swap(&mut m, 0).unwrap();
    d.kick_off(&mut m).unwrap();
    let before = d.stats();
    // Draw the frame again after the swap, so the back buffer read holds
    // it.
    d.set_texture(&mut m, 0, TEX_A).unwrap();
    textured_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), false, 0xFFFF_FFFF);
    d.set_texture(&mut m, 0, TEX_B).unwrap();
    textured_quad(&mut d, &mut m, (32.0, 0.0, 64.0, 32.0), true, 0xFFFF_FFFF);
    Some((outcome(&mut d, &mut m), before))
}

#[test]
fn ping_pong_is_the_same_sampled_in_place() {
    let Some((eager, eager_stats)) = ping_pong(false) else {
        return;
    };
    let Some((lazy, lazy_stats)) = ping_pong(true) else {
        return;
    };
    assert_eq!(eager_stats.rtt_direct_samples, 0);
    assert!(eager_stats.rtt_writebacks >= 4, "{eager_stats:?}");
    // Lazily nothing went to guest memory through the swap and KickOff,
    // and every sample read a surface in place.
    assert_eq!(lazy_stats.rtt_writebacks, 0, "{lazy_stats:?}");
    assert_eq!(lazy_stats.rtt_direct_samples, 5, "{lazy_stats:?}");
    assert!(eager.back.iter().any(|&b| b != 0), "something was drawn");
    assert_eq!(eager.back, lazy.back, "back buffer");
    assert_eq!(eager.a, lazy.a, "level A in guest memory");
    assert_eq!(eager.b, lazy.b, "level B in guest memory");
}

/// A level drawn into, then rewritten in part by the guest before anything
/// reads it, then sampled: the guest's bytes win where it wrote, the drawn
/// pixels everywhere else (eagerly the write-back came first; lazily the
/// two are merged when the level is next used). A reported write that
/// changes nothing writes nothing back.
fn guest_rewrite(lazy: bool, change: bool) -> Option<(Outcome, d3d::Stats)> {
    let (mut d, mut m) = gpu(lazy)?;
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    clear_rect(&mut d, &mut m, [0, 0, 32, 16], 0xFF20_40FF);
    d.set_render_target(&mut m, back, 0).unwrap();
    // Rows 4..6 of A (pitch 128): drawn over, and rows 20..22: left from
    // memory.
    for row in [4u32, 5, 20, 21] {
        let at = 0x8000_0000 | (DATA_A + 128 * row);
        let bytes = if change {
            vec![0x5A; 128]
        } else {
            m.read_vec(at, 128).unwrap()
        };
        m.write(at, &bytes).unwrap();
        d.note_write(at, 128);
    }
    let before = d.stats();
    d.set_texture(&mut m, 0, TEX_A).unwrap();
    textured_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), false, 0xFFFF_FFFF);
    let after = d.stats();
    let mut stats = after;
    stats.rtt_writebacks -= before.rtt_writebacks;
    Some((outcome(&mut d, &mut m), stats))
}

#[test]
fn guest_writes_into_a_level_kept_on_the_host_are_merged() {
    for change in [true, false] {
        let Some((eager, _)) = guest_rewrite(false, change) else {
            return;
        };
        let Some((lazy, stats)) = guest_rewrite(true, change) else {
            return;
        };
        assert_eq!(eager.back, lazy.back, "back buffer, change {change}");
        assert_eq!(eager.a, lazy.a, "level A, change {change}");
        // The sample merged the guest's rows in (one write-back), or found
        // nothing changed (none).
        assert_eq!(stats.rtt_writebacks, u64::from(change), "change {change}");
        assert_eq!(stats.rtt_direct_samples, 1);
        if change {
            let row = |r: u32| &lazy.a[(128 * r) as usize..(128 * r + 128) as usize];
            assert!(row(4).iter().all(|&b| b == 0x5A), "the guest's row wins");
            assert_eq!(row(0)[..4], [0xFF, 0x40, 0x20, 0xFF], "drawn row kept");
        }
    }
}

/// A draw that samples the level it draws into reads what the level held
/// before the draw, through guest memory, both ways.
fn feedback(lazy: bool) -> Option<(Outcome, d3d::Stats)> {
    let (mut d, mut m) = gpu(lazy)?;
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    clear_rect(&mut d, &mut m, [0, 0, 16, 32], 0xFFFF_0000);
    d.set_texture(&mut m, 0, TEX_A).unwrap();
    textured_quad(&mut d, &mut m, (8.0, 0.0, 32.0, 32.0), false, 0xFF80_80FF);
    d.set_render_target(&mut m, back, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    textured_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), false, 0xFFFF_FFFF);
    let stats = d.stats();
    Some((outcome(&mut d, &mut m), stats))
}

#[test]
fn sampling_the_target_being_drawn_goes_through_memory() {
    let Some((eager, _)) = feedback(false) else {
        return;
    };
    let Some((lazy, stats)) = feedback(true) else {
        return;
    };
    assert_eq!(eager, lazy);
    // The feedback draw wrote back and uploaded; the back buffer's draw
    // sampled in place.
    assert_eq!(stats.rtt_writebacks, 1, "{stats:?}");
    assert_eq!(stats.rtt_direct_samples, 1, "{stats:?}");
}

/// The recording backend with in-place sampling on, a 16x4 level of the
/// test texture drawn into (cleared) and left.
fn recorded() -> (Device<RecordingBackend>, SparseMemory, u32) {
    let mut backend = RecordingBackend::new();
    backend.sample_surfaces = true;
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    texture(&mut m, 0x11);
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, surf).unwrap();
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    (d, m, surf)
}

#[test]
fn lazily_only_reads_of_the_level_write_it_back() {
    let (mut d, mut m, surf) = recorded();
    d.swap(&mut m, 0).unwrap();
    d.kick_off(&mut m).unwrap();
    // A busy check or wait on something else.
    d.resource_busy(&mut m, VB_HEADER).unwrap();
    d.block_on_resource(&mut m, VB_HEADER).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 0);
    // The texture, its level, or any fence wait.
    d.resource_busy(&mut m, TEX_HEADER).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 1);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    let draw_again = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory| {
        d.set_render_target(m, surf, 0).unwrap();
        d.clear(m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
        d.set_render_target(m, back, 0).unwrap();
    };
    draw_again(&mut d, &mut m);
    d.block_on_resource(&mut m, surf).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 2);
    draw_again(&mut d, &mut m);
    d.block_on_time(&mut m).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 3);
    // Eagerly, the target change already did.
    d.set_rtt_direct(false);
    draw_again(&mut d, &mut m);
    assert_eq!(d.stats().rtt_writebacks, 4);
}
