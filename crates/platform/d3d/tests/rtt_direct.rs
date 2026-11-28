//! Lazy render-to-texture: draws sample a level's host surface in place,
//! and its pixels reach guest memory only when the guest is about to read
//! them. Each GPU scenario runs eagerly (write-back at every target change
//! and through an upload) and lazily, and must give the same back buffer
//! and, once everything is written back, the same guest memory. The
//! recording backend checks when write-backs happen. Synthetic resources;
//! the GPU tests skip without an adapter.

use crate::common::*;
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

/// A level drawn into, then rewritten in part by the guest before anything
/// reads it, then sampled: the guest's bytes win where it wrote, the drawn
/// pixels everywhere else (eagerly the write-back came first; lazily the
/// two are merged when the level is next used). Pages reported written
/// (the runtime's write barrier) that hold what was last synced write
/// nothing back.
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
        if change {
            m.write(at, &[0x5A; 128]).unwrap();
            d.note_write(at, 128);
        } else {
            d.note_written_pages(at, 128);
        }
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

/// Guest stores into level A after it was drawn, through exact
/// `note_write` ranges, where some or all of the stored bytes equal what
/// the level held when it was last synced: row 4 gets `[30 5A 30 5A]`
/// repeated (the 0x30s are the memory fill), row 6 the whole of the
/// original fill, row 20 (never drawn) the mixed row too. Every store must
/// win over the drawn pixels, as it does eagerly. With `during`, the same
/// stores land while A is still the target: eagerly leaving it writes the
/// level over them, and lazily must too.
fn exact_stores(lazy: bool, during: bool) -> Option<(Outcome, d3d::Stats)> {
    let (mut d, mut m) = gpu(lazy)?;
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    clear_rect(&mut d, &mut m, [0, 0, 32, 16], 0xFF20_40FF);
    let store = |d: &mut Device<GfxBackend>, m: &mut SparseMemory| {
        let mixed = [0x30u8, 0x5A, 0x30, 0x5A].repeat(32);
        for (row, bytes) in [(4u32, &mixed), (6, &vec![0x30; 128]), (20, &mixed)] {
            let at = 0x8000_0000 | (DATA_A + 128 * row);
            m.write(at, bytes).unwrap();
            d.note_write(at, 128);
        }
    };
    if during {
        store(&mut d, &mut m);
    }
    d.set_render_target(&mut m, back, 0).unwrap();
    if !during {
        store(&mut d, &mut m);
    }
    let before = d.stats();
    d.set_texture(&mut m, 0, TEX_A).unwrap();
    textured_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), false, 0xFFFF_FFFF);
    let mut stats = d.stats();
    stats.rtt_writebacks -= before.rtt_writebacks;
    Some((outcome(&mut d, &mut m), stats))
}

#[test]
fn guest_stores_of_the_synced_bytes_still_win() {
    for during in [false, true] {
        let Some((eager, _)) = exact_stores(false, during) else {
            return;
        };
        let Some((lazy, stats)) = exact_stores(true, during) else {
            return;
        };
        assert_eq!(eager.back, lazy.back, "back buffer, during {during}");
        assert_eq!(eager.a, lazy.a, "level A, during {during}");
        assert_eq!(stats.rtt_direct_samples, 1, "during {during}");
        let row = |r: u32| &lazy.a[(128 * r) as usize..(128 * r + 128) as usize];
        let drawn = [0xFF, 0x40, 0x20, 0xFF].repeat(32);
        if during {
            // Leaving the target wrote the drawn pixels over the stores.
            assert_eq!(row(4), drawn, "row 4");
            assert_eq!(row(6), drawn, "row 6");
        } else {
            assert_eq!(stats.rtt_writebacks, 1);
            assert_eq!(row(4), [0x30, 0x5A, 0x30, 0x5A].repeat(32), "row 4");
            assert_eq!(row(6), [0x30; 128], "row 6");
            assert_eq!(row(5), drawn, "row 5");
        }
        // Eagerly leaving writes the whole level back, undrawn rows too.
        let row_20 = if during {
            vec![0x30; 128]
        } else {
            [0x30, 0x5A, 0x30, 0x5A].repeat(32)
        };
        assert_eq!(row(20), row_20, "row 20");
    }
}

/// A 32x16 linear texture whose data starts 8 rows into level A.
const TEX_ALIAS: u32 = MISC + 0x4400;
const ALIAS_DATA: u32 = DATA_A + 8 * 128;

fn alias_header(m: &mut SparseMemory) {
    let size = (SIZE - 1) | (15 << 12) | ((128 / 64 - 1) << 24);
    for (i, w) in [
        1 | (4 << 16) | 0x0100_0000,
        ALIAS_DATA,
        0,
        format_word(0x12, false),
        size,
    ]
    .iter()
    .enumerate()
    {
        m.write_u32(TEX_ALIAS + 4 * i as u32, *w).unwrap();
    }
}

/// Level A drawn over (rows 0..24 blue), then a texture over its rows
/// 8..24 used: sampled into the back buffer, or (with `wait`) waited on,
/// after which guest memory must hold the drawn rows without any further
/// flush.
fn offset_alias(lazy: bool, wait: bool) -> Option<(Outcome, Vec<u8>, d3d::Stats)> {
    let (mut d, mut m) = gpu(lazy)?;
    alias_header(&mut m);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    clear_rect(&mut d, &mut m, [0, 0, 32, 24], 0xFF00_00FF);
    d.set_render_target(&mut m, back, 0).unwrap();
    if wait {
        d.block_on_resource(&mut m, TEX_ALIAS).unwrap();
    } else {
        d.set_texture(&mut m, 0, TEX_ALIAS).unwrap();
        textured_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 16.0), false, 0xFFFF_FFFF);
    }
    let stats = d.stats();
    let seen = m.read_vec(0x8000_0000 | DATA_A, LEN).unwrap();
    Some((outcome(&mut d, &mut m), seen, stats))
}

#[test]
fn a_texture_inside_a_drawn_level_sees_the_drawn_pixels() {
    for wait in [false, true] {
        let Some((eager, eager_seen, _)) = offset_alias(false, wait) else {
            return;
        };
        let Some((lazy, lazy_seen, stats)) = offset_alias(true, wait) else {
            return;
        };
        assert_eq!(eager, lazy, "wait {wait}");
        assert_eq!(
            eager_seen, lazy_seen,
            "guest memory at the use, wait {wait}"
        );
        assert_eq!(stats.rtt_writebacks, 1, "wait {wait}: {stats:?}");
        assert_eq!(stats.rtt_direct_samples, 0, "wait {wait}");
        let row = |r: u32| &lazy_seen[(128 * r) as usize..(128 * r + 4) as usize];
        assert_eq!(row(8), [0xFF, 0x00, 0x00, 0xFF], "drawn row in memory");
        assert_eq!(row(28), [0x30; 4], "undrawn row in memory");
        if !wait {
            assert!(lazy.back.iter().any(|&b| b != 0), "something drawn");
        }
    }
}

#[test]
fn waits_on_a_texture_inside_a_level_write_it_back() {
    let (mut d, mut m, _) = recorded();
    // Headers over the level's second row, and just past its end.
    let mut header = |at: u32, data: u32| {
        let h = d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 3, 64, data);
        h.write(&mut m, at).unwrap();
    };
    let inside = MISC + 0x7300;
    let after = MISC + 0x7340;
    header(inside, TEX_DATA + 64);
    header(after, TEX_DATA + 256);
    d.block_on_resource(&mut m, after).unwrap();
    d.resource_busy(&mut m, after).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 0);
    d.block_on_resource(&mut m, inside).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 1);
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

/// Asked again about the same texture with nothing changed, a wait
/// writes nothing back (the device remembers the answer); a draw into the
/// level, or the texture's header moved over a dirty level, asks again.
#[test]
fn repeated_waits_write_back_only_after_a_change() {
    let (mut d, mut m, surf) = recorded();
    d.block_on_resource(&mut m, TEX_HEADER).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 1);
    d.block_on_resource(&mut m, TEX_HEADER).unwrap();
    d.resource_busy(&mut m, TEX_HEADER).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 1);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    d.block_on_resource(&mut m, TEX_HEADER).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 2);
    // A header past the level, asked about while the level is dirty;
    // then rewritten in place over the level's second row.
    d.set_render_target(&mut m, surf, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_00FF, 1.0, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    let other = MISC + 0x7340;
    let header = |m: &mut SparseMemory, data: u32| {
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 3, 64, data)
            .write(m, other)
            .unwrap();
    };
    header(&mut m, TEX_DATA + 256);
    d.block_on_resource(&mut m, other).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 2);
    header(&mut m, TEX_DATA + 64);
    d.block_on_resource(&mut m, other).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 3);
}

/// A store made while a depth/stencil level is bound is overwritten when
/// the target changes, just like a store into the active colour target.
fn active_depth_store<B: d3d::Backend>(backend: B, lazy: bool) -> Vec<u8> {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_rtt_direct(lazy);
    let texture = MISC + 0x7800;
    let level = MISC + 0x7820;
    let color = MISC + 0x7840;
    let data = 0x0032_0000;
    let addr = 0x8000_0000 | data;
    m.map(addr, 0x1000);
    m.write(addr, &[0x11; 1024]).unwrap();
    let mut h = d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinD24S8, 16, 16, 64, data);
    h.parent = texture;
    h.write(&mut m, level).unwrap();
    let pc = h.pixels;
    m.write(
        texture,
        &[0x0004_0001u32, data, 0, pc.format, pc.size]
            .map(u32::to_le_bytes)
            .concat(),
    )
    .unwrap();
    d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 16, 64, 0x0033_0000)
        .write(&mut m, color)
        .unwrap();
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.set_render_target(&mut m, color, level).unwrap();
    d.clear(&mut m, 0, 0, 0x03, 0, 0.5, 0xAB).unwrap();
    // Equal to the upload baseline: this must not become an exact-store
    // override of the pending depth/stencil clear.
    m.write(addr, &[0x11; 4]).unwrap();
    d.note_write(addr, 4);
    d.set_render_target(&mut m, back, 0).unwrap();
    // Lazy depth levels (on the GPU) stay on the host until a wait.
    d.block_on_resource(&mut m, texture).unwrap();
    assert_eq!(d.take_events(), []);
    m.read_vec(addr, 8).unwrap()
}

#[test]
fn stores_into_the_active_depth_target_are_overwritten() {
    // D24S8: depth 0.5 in the high 24 bits, stencil 0xAB in the low byte.
    let expected = [0xAB, 0, 0, 0x80].repeat(2);
    for lazy in [false, true] {
        let mut backend = RecordingBackend::new();
        backend.sample_surfaces = true;
        backend.depth_readback = Some(expected.clone());
        assert_eq!(active_depth_store(backend, lazy), expected, "lazy {lazy}");
    }
    for lazy in [false, true] {
        let ctx = match gfx::GfxContext::headless() {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("skipping GPU depth test: {e}");
                return;
            }
        };
        assert_eq!(
            active_depth_store(GfxBackend::new(ctx), lazy),
            expected,
            "GPU, lazy {lazy}"
        );
    }
}

#[test]
fn waits_and_busy_checks_accept_twelve_byte_resource_headers() {
    for busy in [false, true] {
        let (mut d, mut m, _) = recorded();
        let page = 0x0500_0000;
        m.map(page, 0x1000);
        let header = page + 0x1000 - d3d::resource::ResourceHeader::SIZE;
        // Vertex buffer: Common, Data and Lock. The next page is unmapped.
        m.write(header, &[1u32, VB_DATA, 0].map(u32::to_le_bytes).concat())
            .unwrap();
        if busy {
            assert_eq!(d.resource_busy(&mut m, header).unwrap(), 0);
        } else {
            d.block_on_resource(&mut m, header).unwrap();
        }
        assert_eq!(d.stats().rtt_writebacks, 0, "unrelated resource");
    }
}

/// Fails the first write of the level, leaving device bookkeeping writable.
struct FailLevelWrite {
    inner: SparseMemory,
    fail: bool,
}

impl GuestMemory for FailLevelWrite {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.inner.read(addr, out)
    }

    fn write(&mut self, addr: u32, bytes: &[u8]) -> Result<(), d3d::GuestError> {
        if self.fail && addr == (0x8000_0000 | TEX_DATA) && bytes.len() == 256 {
            self.fail = false;
            return Err(d3d::GuestError {
                addr,
                len: bytes.len() as u32,
            });
        }
        self.inner.write(addr, bytes)
    }
}

#[test]
fn failed_writeback_keeps_exact_guest_stores_for_retry() {
    for guest in [[0x11, 0x5A, 0x11, 0x5A], [0x11; 4]] {
        let (mut d, mut m, _) = recorded();
        d.backend.readback_fill = 0xCC;
        let addr = 0x8000_0000 | TEX_DATA;
        m.write(addr, &guest).unwrap();
        d.note_write(addr, 4);
        let mut m = FailLevelWrite {
            inner: m,
            fail: true,
        };
        assert!(matches!(
            d.block_on_resource(&mut m, TEX_HEADER),
            Err(d3d::D3dError::Guest(_))
        ));
        assert_eq!(d.fault(), None, "the write-back can be retried");
        assert_eq!(d.stats().rtt_writebacks, 0);
        assert_eq!(m.read_vec(addr, 4).unwrap(), guest);
        d.block_on_resource(&mut m, TEX_HEADER).unwrap();
        assert_eq!(m.read_vec(addr, 4).unwrap(), guest, "guest stores win");
        assert_eq!(m.read_vec(addr + 4, 4).unwrap(), [0xCC; 4], "drawn pixels");
        assert_eq!(d.stats().rtt_writebacks, 1);
        d.block_on_resource(&mut m, TEX_HEADER).unwrap();
        assert_eq!(d.stats().rtt_writebacks, 1, "the retry committed the level");
    }
}

/// A level drawn into and left as the target again and again, with guest
/// stores into it (exact, and page reports of unchanged bytes) between
/// some of the visits and during one, then written back. Leaving it reads
/// its guest bytes only when they may have changed since its match (the
/// stores between visits are merged when it is bound again, which makes a
/// new match); the backend calls and the bytes written back are the same
/// as reading them at every leave (`Device::set_leave_read`).
fn revisits(leave_read: bool) -> (Vec<d3d::Recorded>, Vec<u8>, d3d::Stats) {
    let (mut d, mut m, surf) = recorded();
    d.set_leave_read(leave_read);
    let setup = d.stats().rtt_leave_reads_skipped;
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    let data = 0x8000_0000 | TEX_DATA;
    for visit in 0..6u32 {
        d.set_render_target(&mut m, surf, 0).unwrap();
        d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000 | visit, 1.0, 0)
            .unwrap();
        if visit == 5 {
            // An exact store into row 2 while the level is the target.
            m.write(data + 128, &[0xA5; 64]).unwrap();
            d.note_write(data + 128, 64);
        }
        d.set_render_target(&mut m, back, 0).unwrap();
        match visit {
            // An exact store into row 1 while the level is on the host.
            1 => {
                m.write(data + 64, &[0x5A; 64]).unwrap();
                d.note_write(data + 64, 64);
            }
            // The runtime reports the level's page with nothing changed.
            3 => d.note_written_pages(data, 0x1000),
            _ => {}
        }
    }
    // A page report after the last leave: the write-back compares the
    // guest bytes with the match, which must hold the store above.
    d.note_written_pages(data, 0x1000);
    d.block_on_resource(&mut m, surf).unwrap();
    let bytes = m.read_vec(data, 256).unwrap();
    let mut stats = d.stats();
    stats.rtt_leave_reads_skipped -= setup;
    (std::mem::take(&mut d.backend.calls), bytes, stats)
}

#[test]
fn leaving_an_unwritten_level_does_not_read_it_again() {
    let (calls, bytes, stats) = revisits(false);
    let (read_calls, read_bytes, read_stats) = revisits(true);
    assert_eq!(calls, read_calls, "backend calls");
    assert_eq!(bytes, read_bytes, "level in guest memory");
    assert_eq!(stats.rtt_writebacks, read_stats.rtt_writebacks);
    assert_eq!(read_stats.rtt_leave_reads_skipped, 0);
    // Six leaves: only the one after the store made while the level was
    // the target (the last visit) reads it.
    assert_eq!(stats.rtt_leave_reads_skipped, 5, "{stats:?}");
}

/// Samples the texture headers it is made with. Colour only: `recorded`
/// leaves the back buffer bound with no depth (`set_render_target(back,
/// 0)`), where a pass that needs depth is refused (`NoDepth`).
struct Samples(Vec<u32>);

impl d3d::ExternalPass for Samples {
    fn encode(&mut self, _t: d3d::ExternalTarget<'_>) {}
    fn sampled(&self) -> Vec<u32> {
        self.0.clone()
    }
    fn colour_only(&self) -> bool {
        true
    }
}

/// An external pass that samples a texture whose level the D3D path drew
/// into (the native objects' layer 3 reading the texture accumulation
/// target layer 1 drew) gets the level's own surface, in place: nothing
/// written back. A header over no drawn level gets none.
#[test]
fn an_external_pass_samples_a_drawn_level_in_place() {
    let (mut d, mut m, _surf) = recorded();
    d.backend.external_passes = true;
    let nothing = MISC + 0x7300;
    d.request_external_pass(Box::new(Samples(vec![TEX_HEADER, nothing])));
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(d.stats().external_passes, 1, "refusals {:?}", d.stats().external_refusals);
    let sampled = &d.backend.external_sampled;
    assert_eq!(sampled.len(), 1);
    assert_eq!(sampled[0].len(), 2, "one per header, in order");
    let level = sampled[0][0].expect("the drawn level");
    assert_ne!(level, d.backend.external[0].0, "not the colour target");
    assert_eq!(sampled[0][1], None);
    assert_eq!(d.stats().rtt_writebacks, 0, "sampled in place");
}

/// Layer 3's native objects sample the texture accumulation target after
/// the D3D work drawn into it before the walk (layer 1's kept items) and
/// before the work drawn into it after: the pass is issued at the drain,
/// in stream order, so it never reads the level ahead of its albedo.
#[test]
fn an_external_pass_samples_a_level_after_the_work_drawn_into_it_before_the_drain() {
    let (mut d, mut m, surf) = recorded();
    d.backend.external_passes = true;
    d.request_external_pass(Box::new(Samples(vec![TEX_HEADER])));
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(d.stats().external_passes, 1, "refusals {:?}", d.stats().external_refusals);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    let level = d.backend.external_sampled[0][0].expect("the drawn level");
    let at = d.backend.external_at[0];
    let into_level: Vec<usize> = d
        .backend
        .calls
        .iter()
        .enumerate()
        .filter_map(|(i, call)| match call {
            d3d::Recorded::Clear { color, .. } | d3d::Recorded::ClearRects { color, .. } if *color == level => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(into_level.len(), 2, "the clear before the pass and the one after: {into_level:?}");
    assert!(into_level[0] < at, "the work before the drain is recorded first ({into_level:?}, pass at {at})");
    assert!(into_level[1] >= at, "the work after it follows the pass ({into_level:?}, pass at {at})");
}
