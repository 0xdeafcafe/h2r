//! `ScaledBackend` (docs/graphics/high-resolution.md, phase 1): a reference backend
//! at the guest's size answers the guest, a view backend renders the frame
//! buffers at a multiple of it. One synthetic frame (render-to-texture with
//! a write-back, scissored and rectangle-cleared draws, a textured draw over
//! the level, a visibility test) must give the guest the same answers with
//! one plain backend and with the wrapper at 1x, 2x and 4x, and the view's
//! image box-filtered down must be the reference's. Synthetic resources;
//! the GPU tests skip without an adapter.

mod common;

use common::*;
use d3d::abi::Regs;
use d3d::hw::method as hw;
use d3d::scaled::ScaledBackend;
use d3d::{
    Backend, Device, Event, GfxBackend, GuestMemory, SparseMemory, ThreadedBackend, UploadPolicy,
};
use gfx::GfxContext;
use gfx::vertex::fvf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;

const DEV: u32 = 0x0040_4FE0;
const END: u32 = 0x3f85d0;
const GET: u32 = 0x3f8630;
const OUT: u32 = MISC + 0x8000;
const STAMP: u32 = MISC + 0x8010;
const RECTS: u32 = MISC + 0x7100;

/// A 32x32 `LIN_A8R8G8B8` texture (pitch 128) and its one level.
const TEX_A: u32 = MISC + 0x4000;
const LEVEL_A: u32 = MISC + 0x4200;
const DATA_A: u32 = 0x0030_0000;
const SIZE: u32 = 32;
const LEN: usize = (SIZE * SIZE * 4) as usize;

fn context() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

fn share(ctx: &GfxContext) -> GfxContext {
    GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

fn scaled(ctx: &GfxContext, scale: u32) -> Scaled {
    ScaledBackend::new(
        GfxBackend::new(share(ctx)),
        GfxBackend::new(share(ctx)),
        scale,
    )
    .unwrap()
}

fn level(m: &mut SparseMemory) {
    m.map(0x8000_0000 | DATA_A, LEN as u32);
    m.write(0x8000_0000 | DATA_A, &vec![0x30; LEN]).unwrap();
    let format = (2 << 4) | (0x12 << 8) | (1 << 16);
    let size = (SIZE - 1) | ((SIZE - 1) << 12) | ((128 / 64 - 1) << 24);
    for (i, w) in [1 | (4 << 16) | 0x0100_0000, DATA_A, 0, format, size]
        .iter()
        .enumerate()
    {
        m.write_u32(TEX_A + 4 * i as u32, *w).unwrap();
    }
    for (i, w) in [1 | (5 << 16) | 0x0100_0000, DATA_A, 0, format, size, TEX_A]
        .iter()
        .enumerate()
    {
        m.write_u32(LEVEL_A + 4 * i as u32, *w).unwrap();
    }
}

fn rects(m: &mut SparseMemory, list: &[[u32; 4]]) {
    for (i, w) in list.iter().flatten().enumerate() {
        m.write_u32(RECTS + 4 * i as u32, *w).unwrap();
    }
}

fn flat_quad<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    r: (f32, f32, f32, f32),
    color: u32,
) {
    quad_vb(m, r.0, r.1, r.2, r.3, color);
    // Reported, as the runtime's write barrier would (the vertex cache
    // relies on it).
    d.note_write(0x8000_0000 | VB_DATA, 4 * 20);
    bind_quad(d, m);
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

/// A triangle whose long edge crosses pixels at every angle.
fn triangle<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, color: u32) {
    let mut bytes = Vec::new();
    for (x, y) in [(3.3, 40.2), (61.7, 47.9), (20.4, 63.1)] {
        bytes.extend(vertex(x, y, color));
    }
    m.write(0x8000_0000 | VB_DATA, &bytes).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, bytes.len() as u32);
    bind_quad(d, m);
    d.draw_vertices(m, 5 /* TRIANGLELIST */, 0, 3).unwrap();
}

/// A quad over `r` sampling all of the 32x32 linear texture (texel
/// coordinates).
fn textured_quad<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, r: (f32, f32, f32, f32)) {
    let mut vb = Vec::new();
    for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [r.0 + (r.2 - r.0) * u, r.1 + (r.3 - r.1) * v, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for f in [SIZE as f32 * u, SIZE as f32 * v] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, vb.len() as u32);
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

fn push_words(m: &mut SparseMemory, words: &[u32]) {
    let mut put = m.read_u32(DEV).unwrap();
    for w in words {
        m.write_u32(put, *w).unwrap();
        put += 4;
    }
    m.write_u32(DEV, put).unwrap();
}

/// What the guest saw.
#[derive(Debug, PartialEq)]
struct Answers {
    /// The visibility test's count.
    count: u32,
    /// The level's bytes in guest memory after the write-back.
    level: Vec<u8>,
    /// Draws skipped.
    skipped: usize,
    /// The guest memory the device wrote, as a digest of the mapped pages.
    device_page: Vec<u8>,
}

/// The synthetic frame's draws into the back buffer `back` (and the
/// level), and the visibility count the guest reads.
fn draw_pass<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, back: u32) -> u32 {
    d.clear(m, 0, 0, 0xF0, 0xFF10_2030, 1.0, 0).unwrap();
    // The level: a rectangle clear and a flat quad.
    d.set_render_target(m, LEVEL_A, 0).unwrap();
    rects(m, &[[0, 0, 16, 32]]);
    d.clear(m, 1, RECTS, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    set_states(d, m);
    flat_quad(d, m, (8.0, 8.0, 24.0, 20.0), 0xFF00_FF00);
    // The back buffer: a rectangle clear, a quad through two scissor
    // rectangles, the level drawn over the right half, a triangle.
    d.set_render_target(m, back, 0).unwrap();
    rects(m, &[[5, 7, 29, 19]]);
    d.clear(m, 1, RECTS, 0xF0, 0xFF80_8080, 1.0, 0).unwrap();
    rects(m, &[[2, 30, 20, 50], [12, 40, 31, 61]]);
    d.set_scissors(m, 2, 0, RECTS).unwrap();
    flat_quad(d, m, (0.0, 0.0, 64.0, 64.0), 0xFF00_00FF);
    d.set_scissors(m, 0, 0, 0).unwrap();
    d.set_texture(m, 0, TEX_A).unwrap();
    textured_quad(d, m, (32.0, 0.0, 64.0, 32.0));
    d.set_texture(m, 0, 0).unwrap();
    triangle(d, m, 0xFFFF_FF00);
    // A visibility test around two quads, read back.
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    push_words(m, &[(2 << 18) | hw::CLEAR_REPORT_VALUE, 1, 1]);
    inline_quad(m, 0.0, 0.0, 32.0, 16.0, [0, 255, 0, 255]);
    inline_quad(m, 40.0, 50.0, 47.0, 61.0, [0, 0, 255, 255]);
    let out = call(
        d,
        m,
        END,
        Regs {
            eax: 3,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    assert_eq!(out.eax, Some(0));
    let r = Regs {
        eax: 3,
        edx: STAMP,
        ..Regs::default()
    };
    call(d, m, GET, r, &[OUT]).unwrap();
    m.read_u32(OUT).unwrap()
}

/// One synthetic frame, drawn again after the swap so the back buffer
/// holds it.
fn frame<B: Backend>(backend: B) -> (Device<B>, SparseMemory, Answers) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    // The test is read in the frame it ends: waiting makes the answer
    // the same at every scale whatever the GPU's timing.
    d.set_visibility_poll(false);
    d.set_rtt_direct(true);
    set_states(&mut d, &mut m);
    level(&mut m);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    let mut count = 0;
    for pass in 0..2 {
        count = draw_pass(&mut d, &mut m, back);
        if pass == 0 {
            d.swap(&mut m, 0).unwrap();
        }
    }
    let skipped = d
        .take_events()
        .iter()
        .filter(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_)))
        .count();
    d.flush_render_targets(&mut m).unwrap();
    let answers = Answers {
        count,
        level: m.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
        skipped,
        device_page: m.read_vec(0x0040_4000, 0x5000).unwrap(),
    };
    (d, m, answers)
}

/// RGBA bytes of a colour surface, and its size.
fn pixels(b: &mut GfxBackend, s: gfx::surface::SurfaceId) -> (Vec<u8>, (u32, u32)) {
    let (px, _) = b.read_color(s).unwrap();
    let d = *b.store.get(s).unwrap().desc();
    (px, (d.width, d.height))
}

/// `px` (`w x h` at `scale`) box-filtered to the guest size, rounded.
fn downsample(px: &[u8], w: u32, scale: u32) -> Vec<u8> {
    let (gw, gh) = (w / scale, px.len() as u32 / 4 / w / scale);
    let mut out = Vec::with_capacity((gw * gh * 4) as usize);
    for gy in 0..gh {
        for gx in 0..gw {
            for c in 0..4 {
                let mut sum = 0u32;
                for y in gy * scale..(gy + 1) * scale {
                    for x in gx * scale..(gx + 1) * scale {
                        sum += px[((y * w + x) * 4 + c) as usize] as u32;
                    }
                }
                let n = scale * scale;
                out.push(((sum + n / 2) / n) as u8);
            }
        }
    }
    out
}

#[test]
fn the_guest_sees_the_same_answers_at_every_scale() {
    let Some(ctx) = context() else { return };
    let (mut plain, _, expect) = frame(GfxBackend::new(share(&ctx)));
    assert!(expect.skipped == 0, "{expect:?}");
    assert_eq!(
        expect.count,
        32 * 16 + 7 * 11,
        "the visibility test counted"
    );
    assert!(
        expect.level.iter().any(|&b| b != 0x30),
        "the level was written back"
    );
    let bb = plain.back_buffer().unwrap();
    let (plain_px, _) = pixels(&mut plain.backend, bb);
    for scale in [1, 2, 4] {
        let (mut d, _, got) = frame(scaled(&ctx, scale));
        assert_eq!(got, expect, "guest-visible answers at {scale}x");
        let bb = d.back_buffer().unwrap();
        let (ref_px, size) = pixels(d.backend.reference_mut(), bb.reference);
        assert_eq!(size, (W, H));
        assert_eq!(
            ref_px, plain_px,
            "the reference's frame at {scale}x is the plain one"
        );
        let s = d.backend.stats();
        assert_eq!(
            (s.view_errors, s.mismatches, s.fallback_frames),
            (0, 0, 0),
            "{s:?}"
        );
        // At 1x the view sleeps and the frame is the reference's.
        let shown = if scale == 1 { (0, 1) } else { (1, 0) };
        assert_eq!((s.view_frames, s.reference_frames), shown, "{s:?}");
    }
}

#[test]
fn the_view_renders_the_frame_at_scale_and_matches_the_reference_downsampled() {
    let Some(ctx) = context() else { return };
    for scale in [2, 3, 4] {
        let (mut d, _, _) = frame(scaled(&ctx, scale));
        let bb = d.back_buffer().unwrap();
        let (ref_px, _) = pixels(d.backend.reference_mut(), bb.reference);
        let (view_px, size) = pixels(
            d.backend.view_mut(),
            bb.view.expect("the back buffer scales"),
        );
        assert_eq!(size, (W * scale, H * scale));
        let down = downsample(&view_px, size.0, scale);
        // Everything but the triangle has whole-pixel edges (clears,
        // scissors, quads, a 1:1 texel copy) and must match exactly; the
        // triangle's edge pixels are coverage averages.
        let mut differ = 0;
        let mut max = 0;
        let mut total = 0u64;
        for (i, (a, b)) in down.iter().zip(&ref_px).enumerate() {
            let (x, y) = ((i / 4) as u32 % W, (i / 4) as u32 / W);
            let diff = a.abs_diff(*b);
            total += diff as u64;
            if y < 40 && diff != 0 {
                panic!(
                    "{scale}x: pixel ({x}, {y}) channel {} is {a}, the reference {b}",
                    i % 4
                );
            }
            max = max.max(diff);
            if diff > 16 {
                differ += 1;
            }
        }
        let mean = total as f64 / down.len() as f64;
        eprintln!("{scale}x: mean {mean:.3}, max {max}, {differ} channels over 16");
        assert!(mean < 2.0, "{scale}x: mean difference {mean}");
        // The triangle's long edges span about 150 pixels.
        assert!(
            differ < 150 * 4,
            "{scale}x: {differ} channels differ by more than 16"
        );
    }
}

#[test]
fn only_frame_sized_surfaces_scale() {
    let Some(ctx) = context() else { return };
    let mut b = scaled(&ctx, 4);
    let fmt = gfx::D3DFormat::LinA8R8G8B8;
    let back = b.create_surface(fmt, 640, 480, false).unwrap();
    let depth = b
        .create_surface(gfx::D3DFormat::LinD24S8, 640, 480, true)
        .unwrap();
    let small = b.create_surface(fmt, 320, 240, false).unwrap();
    let level = b.create_texture_target(fmt, 640, 480).unwrap();
    let bloom = b.create_texture_target(fmt, 128, 128).unwrap();
    let size = |b: &Scaled, s: Option<gfx::surface::SurfaceId>| {
        let d = *b.view().store.get(s.unwrap()).unwrap().desc();
        (d.width, d.height)
    };
    assert_eq!(size(&b, back.view), (2560, 1920));
    assert_eq!(size(&b, depth.view), (2560, 1920));
    assert_eq!(small.view, None, "another size stays in the reference");
    assert_eq!(
        size(&b, level.view),
        (2560, 1920),
        "a back-buffer-sized render-to-texture level scales"
    );
    assert_eq!(bloom.view, None, "smaller levels stay at 1x");
    let r = *b.reference().store.get(back.reference).unwrap().desc();
    assert_eq!(
        (r.width, r.height),
        (640, 480),
        "the reference keeps the guest's size"
    );
    // The reference's textures are shared, not uploaded twice.
    let snap = b.snapshot_surface(small, false).unwrap().unwrap();
    assert_eq!(snap.view.as_ref().unwrap().texture, snap.reference.texture);
    let snap = b.snapshot_surface(back, false).unwrap().unwrap();
    assert_eq!(
        snap.view.as_ref().unwrap().texture.width(),
        2560,
        "a snapshot of a scaled surface is scaled"
    );
}

#[test]
fn rejects_scales_outside_one_to_six() {
    let Some(ctx) = context() else { return };
    for bad in [0, 7] {
        assert!(
            ScaledBackend::new(
                GfxBackend::new(share(&ctx)),
                GfxBackend::new(share(&ctx)),
                bad
            )
            .is_err()
        );
    }
}

// Phase 2: what the view used to leave out (docs/graphics/high-resolution.md,
// "Phase 2 as built"). Each scene is drawn by a plain backend and by the
// wrapper; the reference must be the plain image and the view, box-filtered
// to the guest size, the reference. Every edge here is on a whole pixel, so
// the match is exact.

const L: d3d::GuestLayout = d3d::GuestLayout::HALO2;
const RS_ZENABLE: u32 = 143;
const RS_ZWRITEENABLE: u32 = 64;
const Z_LESS: u32 = 0x201;
const Z_EQUAL: u32 = 0x202;
const Z_ALWAYS: u32 = 0x207;

/// A 64x64 (frame-sized) `LIN_A8R8G8B8` texture and its level, like
/// Halo's texaccum.
const TEX_B: u32 = MISC + 0x5000;
const LEVEL_B: u32 = MISC + 0x5100;
const DATA_B: u32 = 0x0031_0000;
/// A 64x64 `LIN_D24S8` texture and its level.
const DTEX: u32 = MISC + 0x5200;
const DLEVEL: u32 = MISC + 0x5300;
const DDATA: u32 = 0x0034_0000;
/// A colour surface over the depth buffer's memory (Halo's target 3).
const CSURF: u32 = MISC + 0x5400;
/// A `LIN_A8R8G8B8` texture over the depth buffer's memory (Halo's
/// `0x509368`).
const ALIAS: u32 = MISC + 0x5500;
const FRAME_LEN: u32 = W * H * 4;

fn z_state(m: &mut SparseMemory, enable: bool, write: bool, func: u32) {
    m.write_u32(L.render_state_slot(RS_ZENABLE), enable as u32)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_ZWRITEENABLE), write as u32)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_ZFUNC), func).unwrap();
}

fn device_field(m: &SparseMemory, field: u32) -> u32 {
    m.read_u32(L.device + field).unwrap()
}

/// A linear texture header at `tex` and its level at `level`: `format`
/// (raw, 32 bits a texel), `n x n`, tight rows at `data` holding `fill`.
fn square_level(
    m: &mut SparseMemory,
    tex: u32,
    level: u32,
    format: u32,
    data: u32,
    n: u32,
    fill: &[u8],
) {
    m.map(0x8000_0000 | data, n * n * 4);
    m.write(0x8000_0000 | data, fill).unwrap();
    let fmt = (2 << 4) | (format << 8) | (1 << 16);
    let size = (n - 1) | ((n - 1) << 12) | ((n * 4 / 64 - 1) << 24);
    for (i, w) in [1 | (4 << 16) | 0x0100_0000, data, 0, fmt, size]
        .iter()
        .enumerate()
    {
        m.write_u32(tex + 4 * i as u32, *w).unwrap();
    }
    for (i, w) in [1 | (5 << 16) | 0x0100_0000, data, 0, fmt, size, tex]
        .iter()
        .enumerate()
    {
        m.write_u32(level + 4 * i as u32, *w).unwrap();
    }
}

/// A header at `at` over the depth buffer's memory: a `LIN_A8R8G8B8`
/// texture (`surface` false) or colour surface, 64x64.
fn over_depth(m: &mut SparseMemory, at: u32, surface: bool) {
    let common = if surface {
        1 | (5 << 16) | 0x0100_0000
    } else {
        0x0004_0001
    };
    let fmt = 1 | (2 << 4) | (0x12 << 8) | (1 << 16);
    let size = (W - 1) | ((H - 1) << 12) | ((W * 4 / 64 - 1) << 24);
    let data = d3d::device::DEPTH_BUFFER_DATA;
    for (i, w) in [common, data, 0, fmt, size, 0].iter().enumerate() {
        m.write_u32(at + 4 * i as u32, *w).unwrap();
    }
}

/// A quad over `r` at depth `z` sampling a `size`-texel linear texture 1:1
/// from its corner.
fn texel_quad<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    r: (f32, f32, f32, f32),
    z: f32,
) {
    let mut vb = Vec::new();
    for (x, y) in [(r.0, r.1), (r.2, r.1), (r.0, r.3), (r.2, r.3)] {
        for f in [x, y, z, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for f in [x, y] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, vb.len() as u32);
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

/// A flat quad at depth `z`.
fn z_quad<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    r: (f32, f32, f32, f32),
    z: f32,
    color: u32,
) {
    let mut bytes = Vec::new();
    for (x, y) in [(r.0, r.1), (r.2, r.1), (r.0, r.3), (r.2, r.3)] {
        for f in [x, y, z, 1.0] {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes.extend_from_slice(&color.to_le_bytes());
    }
    m.write(0x8000_0000 | VB_DATA, &bytes).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, bytes.len() as u32);
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    bind_quad(d, m);
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

#[derive(Clone, Copy, Debug)]
enum Scene {
    /// A frame-sized render-to-texture level drawn with the depth buffer
    /// bound (writing it), then sampled into the back buffer by a pass
    /// that tests depth EQUAL: Halo's texaccum.
    FrameSizedTarget,
    /// The same with a 32x32 level: a guest-size target with the scaled
    /// depth buffer.
    SmallTargetWritesDepth,
    /// A frame-sized depth level whose guest bytes hold the depth, bound
    /// with the back buffer and tested LESS.
    GuestDepthLevel,
    /// The same with a depth level twice the frame's size: the view's
    /// back buffer with a depth surface it does not scale, so the
    /// reference's image stands in.
    LargerDepthLevel,
    /// Depth from clears, then the depth buffer's memory sampled as a
    /// colour texture over the whole screen.
    DepthAlias,
    /// A colour surface over the depth buffer's memory drawn into, then
    /// sampled through the texture over that memory.
    ColorOverDepth,
}

/// Pixel `(x, y)` of a 64-wide BGRA readback as `0xAARRGGBB`.
fn argb(px: &[u8], x: u32, y: u32) -> u32 {
    let [b, g, r, a] = pixel(px, x, y);
    u32::from_be_bytes([a, r, g, b])
}

/// Pre-transformed z for a `D24S8` buffer: depth `z` of 0..1 scaled to
/// the buffer's range, as the game passes it.
const fn d24(z: f32) -> f32 {
    z * 16_777_215.0
}

/// Draws `scene` into the back buffer.
fn scene<B: Backend>(backend: B, scene: Scene) -> Device<B> {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_rtt_direct(true);
    set_states(&mut d, &mut m);
    let back = device_field(&m, d3d::layout::device_field::RENDER_TARGET);
    let depth = device_field(&m, d3d::layout::device_field::DEPTH_STENCIL);
    assert_ne!(depth, 0);
    // Colour, depth 1 and stencil 0 everywhere.
    d.clear(&mut m, 0, 0, 0xF3, 0xFF10_2030, 1.0, 0).unwrap();
    match scene {
        Scene::FrameSizedTarget | Scene::SmallTargetWritesDepth => {
            let (level, tex, span) = match scene {
                Scene::FrameSizedTarget => {
                    square_level(
                        &mut m,
                        TEX_B,
                        LEVEL_B,
                        0x12,
                        DATA_B,
                        W,
                        &[0x30; FRAME_LEN as usize],
                    );
                    (LEVEL_B, TEX_B, 40.0)
                }
                _ => {
                    level(&mut m);
                    (LEVEL_A, TEX_A, 24.0)
                }
            };
            d.set_render_target(&mut m, level, depth).unwrap();
            set_states(&mut d, &mut m);
            z_state(&mut m, true, true, Z_ALWAYS);
            z_quad(&mut d, &mut m, (8.0, 8.0, span, span), 0.5, 0xFFFF_0000);
            z_quad(&mut d, &mut m, (2.0, 12.0, 6.0, 20.0), 0.5, 0xFF00_FF00);
            // The composite: only where the level's pass left depth 0.5.
            d.set_render_target(&mut m, back, depth).unwrap();
            set_states(&mut d, &mut m);
            z_state(&mut m, true, false, Z_EQUAL);
            match scene {
                Scene::FrameSizedTarget => {
                    d.set_texture(&mut m, 0, tex).unwrap();
                    texel_quad(&mut d, &mut m, (0.0, 0.0, W as f32, H as f32), 0.5);
                    d.set_texture(&mut m, 0, 0).unwrap();
                }
                _ => z_quad(
                    &mut d,
                    &mut m,
                    (0.0, 0.0, W as f32, H as f32),
                    0.5,
                    0xFF00_80FF,
                ),
            }
        }
        Scene::GuestDepthLevel | Scene::LargerDepthLevel => {
            // z = 0.25 on the left of the frame, 1 elsewhere; stencil 0.
            let n = match scene {
                Scene::GuestDepthLevel => W,
                _ => 2 * W,
            };
            let mut bytes = Vec::with_capacity((n * n * 4) as usize);
            for _y in 0..n {
                for x in 0..n {
                    let z: u32 = if x < W / 2 { 0x40_0000 } else { 0xFF_FFFF };
                    bytes.extend_from_slice(&(z << 8).to_le_bytes());
                }
            }
            square_level(&mut m, DTEX, DLEVEL, 0x2E, DDATA, n, &bytes);
            d.set_render_target(&mut m, back, DLEVEL).unwrap();
            set_states(&mut d, &mut m);
            z_state(&mut m, true, false, Z_LESS);
            z_quad(
                &mut d,
                &mut m,
                (0.0, 0.0, W as f32, H as f32),
                d24(0.5),
                0xFF00_FF00,
            );
        }
        Scene::DepthAlias => {
            for (rect, z, s) in [
                ([0u32, 0, 40, 24], 0.333_333, 0x5A),
                ([20, 10, 64, 50], 0.7, 0xA5),
                ([5, 40, 30, 64], 0.000_123, 0x01),
            ] {
                rects(&mut m, &[rect]);
                d.clear(&mut m, 1, RECTS, 0x03, 0, z, s).unwrap();
            }
            over_depth(&mut m, ALIAS, false);
            z_state(&mut m, false, false, Z_ALWAYS);
            d.set_texture(&mut m, 0, ALIAS).unwrap();
            texel_quad(&mut d, &mut m, (0.0, 0.0, W as f32, H as f32), 0.5);
            d.set_texture(&mut m, 0, 0).unwrap();
        }
        Scene::ColorOverDepth => {
            over_depth(&mut m, CSURF, true);
            over_depth(&mut m, ALIAS, false);
            d.set_render_target(&mut m, CSURF, 0).unwrap();
            set_states(&mut d, &mut m);
            d.clear(&mut m, 0, 0, 0xF0, 0xFF40_4040, 1.0, 0).unwrap();
            flat_quad(&mut d, &mut m, (8.0, 16.0, 48.0, 40.0), 0xFFFF_8000);
            d.set_render_target(&mut m, back, depth).unwrap();
            set_states(&mut d, &mut m);
            d.set_texture(&mut m, 0, ALIAS).unwrap();
            texel_quad(&mut d, &mut m, (0.0, 0.0, W as f32, H as f32), 0.5);
            d.set_texture(&mut m, 0, 0).unwrap();
        }
    }
    let skipped: Vec<_> = d
        .take_events()
        .into_iter()
        .filter(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_)))
        .collect();
    assert!(skipped.is_empty(), "{scene:?}: {skipped:?}");
    d
}

/// Draws `s` plainly and at `scale`; checks the reference is the plain
/// image and the view matches it box-filtered. Returns the plain image and
/// the wrapper's stats.
fn check_scene(ctx: &GfxContext, s: Scene, scale: u32) -> (Vec<u8>, d3d::scaled::ScaleStats) {
    let mut plain = scene(GfxBackend::new(share(ctx)), s);
    let bb = plain.back_buffer().unwrap();
    let (plain_px, _) = pixels(&mut plain.backend, bb);
    let mut d = scene(scaled(ctx, scale), s);
    let bb = d.back_buffer().unwrap();
    let (ref_px, _) = pixels(d.backend.reference_mut(), bb.reference);
    assert!(
        ref_px == plain_px,
        "{s:?} {scale}x: the reference is not the plain image"
    );
    let (view_px, size) = pixels(d.backend.view_mut(), bb.view.expect("scaled"));
    assert_eq!(size, (W * scale, H * scale));
    let down = downsample(&view_px, size.0, scale);
    let differ: Vec<_> = (0..(W * H) as usize)
        .filter(|&i| down[i * 4..i * 4 + 4] != ref_px[i * 4..i * 4 + 4])
        .map(|i| (i as u32 % W, i as u32 / W))
        .collect();
    assert!(
        differ.is_empty(),
        "{s:?} {scale}x: {} pixels differ from the reference, first {:?}: view {:?}, reference {:?}",
        differ.len(),
        differ[..differ.len().min(8)].to_vec(),
        pixel(&down, differ[0].0, differ[0].1),
        pixel(&ref_px, differ[0].0, differ[0].1),
    );
    let stats = d.backend.stats();
    assert_eq!(
        (stats.view_errors, stats.depth_missed, stats.fallback_frames),
        (0, 0, 0),
        "{s:?} {scale}x: {stats:?}"
    );
    (plain_px, stats)
}

#[test]
fn a_frame_sized_target_drawn_with_the_depth_buffer_scales() {
    let Some(ctx) = context() else { return };
    for scale in [2, 3, 4] {
        let (px, stats) = check_scene(&ctx, Scene::FrameSizedTarget, scale);
        // Not vacuous: the composite drew the level's red where the level's
        // pass wrote depth, and nothing elsewhere.
        assert_eq!(argb(&px, 20, 20), 0xFFFF_0000);
        assert_eq!(argb(&px, 4, 16), 0xFF00_FF00);
        assert_eq!(argb(&px, 50, 50), 0xFF10_2030);
        assert_eq!(
            (stats.mismatches, stats.color_resyncs, stats.depth_resyncs),
            (0, 0, 0),
            "{stats:?}"
        );
    }
}

#[test]
fn a_small_target_drawn_with_the_depth_buffer_reaches_the_view_depth() {
    let Some(ctx) = context() else { return };
    for scale in [2, 4] {
        let (px, stats) = check_scene(&ctx, Scene::SmallTargetWritesDepth, scale);
        assert_eq!(argb(&px, 16, 16), 0xFF00_80FF);
        assert_eq!(argb(&px, 4, 16), 0xFF00_80FF);
        assert_eq!(argb(&px, 30, 30), 0xFF10_2030);
        assert_eq!(stats.proxy_depth_draws, 2, "{stats:?}");
        assert_eq!(stats.depth_resyncs, 0, "{stats:?}");
    }
}

#[test]
fn guest_bytes_in_a_frame_sized_depth_level_reach_the_view() {
    let Some(ctx) = context() else { return };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping: the depth level cannot be read back");
        return;
    }
    for scale in [2, 3] {
        let (px, _) = check_scene(&ctx, Scene::GuestDepthLevel, scale);
        assert_eq!(argb(&px, 10, 10), 0xFF10_2030);
        assert_eq!(argb(&px, 50, 10), 0xFF00_FF00);
    }
}

#[test]
fn the_depth_buffer_as_a_texture_matches_downsampled() {
    let Some(ctx) = context() else { return };
    for scale in [2, 4] {
        let (px, stats) = check_scene(&ctx, Scene::DepthAlias, scale);
        assert_ne!(pixel(&px, 10, 10), pixel(&px, 50, 30));
        if ctx.caps.depth32_stencil8 {
            assert!(stats.scaled_depth_aliases > 0, "{stats:?}");
        }
    }
}

#[test]
fn a_colour_surface_over_the_depth_memory_matches_downsampled() {
    let Some(ctx) = context() else { return };
    for scale in [2, 3] {
        let (px, _) = check_scene(&ctx, Scene::ColorOverDepth, scale);
        assert_eq!(argb(&px, 20, 20), 0xFFFF_8000);
        assert_eq!(argb(&px, 2, 2), 0xFF40_4040);
    }
}

#[test]
fn a_draw_the_view_cannot_repeat_shows_the_reference_not_a_hole() {
    let Some(ctx) = context() else { return };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping: the depth level cannot be read back");
        return;
    }
    for scale in [2, 3] {
        let (px, stats) = check_scene(&ctx, Scene::LargerDepthLevel, scale);
        assert_eq!(argb(&px, 10, 10), 0xFF10_2030);
        assert_eq!(argb(&px, 50, 10), 0xFF00_FF00);
        assert_eq!(stats.color_resyncs, 1, "{stats:?}");
    }
}

/// The device's back buffer, as `frame` reads it.
fn back_address(m: &SparseMemory) -> u32 {
    m.read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap()
}

/// The view's back buffer of a run started at `scale`: what a run that
/// changed to `scale` must show.
fn view_image_at(ctx: &GfxContext, scale: u32) -> (Vec<u8>, (u32, u32)) {
    let (mut d, _, _) = frame(scaled(ctx, scale));
    let bb = d.back_buffer().unwrap();
    let v = d.backend.view_surface(&bb).expect("the back buffer scales");
    pixels(d.backend.view_mut(), v)
}

#[test]
fn the_scale_changes_at_a_frame_boundary_with_no_fallback_frames() {
    let Some(ctx) = context() else { return };
    let (_, _, expect) = frame(GfxBackend::new(share(&ctx)));
    let wanted = Arc::new(AtomicU32::new(1));
    let mut b = scaled(&ctx, 1);
    let w = Arc::clone(&wanted);
    b.follow_scale(move || w.load(Ordering::Relaxed));
    // Started at 1x: the view sleeps and the frame is the reference's.
    let (mut d, mut m, got) = frame(b);
    assert_eq!(got, expect);
    let s = d.backend.stats();
    assert_eq!(
        (s.view_frames, s.reference_frames, s.fallback_frames),
        (0, 1, 0)
    );
    let back = back_address(&m);
    let mut frames = (0, 1);
    for scale in [2, 3, 1, 4, 2] {
        wanted.store(scale, Ordering::Relaxed);
        // This frame is shown at the old scale; the change is made at its
        // end, before the next frame's first draw.
        if d.backend.scale() == 1 {
            frames.1 += 1;
        } else {
            frames.0 += 1;
        }
        d.swap(&mut m, 0).unwrap();
        assert_eq!(d.backend.scale(), scale);
        assert_eq!(
            draw_pass(&mut d, &mut m, back),
            expect.count,
            "the guest's answer at {scale}x"
        );
        let bb = d.back_buffer().unwrap();
        match d.backend.view_surface(&bb) {
            None => assert_eq!(scale, 1, "only 1x has no view surface"),
            Some(v) => {
                let (px, size) = pixels(d.backend.view_mut(), v);
                assert_eq!(size, (W * scale, H * scale), "the new size at {scale}x");
                // Drawn at the new scale exactly as a run started there.
                assert!(
                    px == view_image_at(&ctx, scale).0,
                    "{scale}x: the image differs from a run started at {scale}x"
                );
            }
        }
        let s = d.backend.stats();
        assert_eq!(
            (s.view_frames, s.reference_frames),
            frames,
            "{scale}x: {s:?}"
        );
        assert_eq!(
            (s.fallback_frames, s.view_errors, s.mismatches),
            (0, 0, 0),
            "{scale}x: {s:?}"
        );
        assert_eq!(s.scale, scale);
    }
    d.swap(&mut m, 0).unwrap();
    let s = d.backend.stats();
    assert_eq!(s.rescales, 5);
    assert_eq!((s.view_frames, s.fallback_frames), (frames.0 + 1, 0));
}

#[test]
fn on_the_render_thread_the_change_is_made_there_and_shown_at_once() {
    let Some(ctx) = context() else { return };
    let (_, _, expect) = frame(GfxBackend::new(share(&ctx)));
    let wanted = Arc::new(AtomicU32::new(1));
    let threads = Arc::new(Mutex::new(Vec::new()));
    let mut b = scaled(&ctx, 1);
    let stats = b.shared_stats();
    let (w, t) = (Arc::clone(&wanted), Arc::clone(&threads));
    b.follow_scale(move || {
        let name = std::thread::current().name().unwrap_or("").to_string();
        t.lock().unwrap().push(name);
        w.load(Ordering::Relaxed)
    });
    let (mut d, mut m, got) = frame(ThreadedBackend::new(b).unwrap());
    assert_eq!(got, expect);
    let back = back_address(&m);
    let guest = std::thread::current().name().unwrap_or("").to_string();
    for scale in [3, 1, 2] {
        wanted.store(scale, Ordering::Relaxed);
        d.swap(&mut m, 0).unwrap();
        assert_eq!(draw_pass(&mut d, &mut m, back), expect.count);
        let bb = d.back_buffer().unwrap();
        let size = d
            .backend
            .with_surface(bb, |b: &mut Scaled, s| {
                Ok(b.view_surface(&s).map(|v| pixels(b.view_mut(), v).1))
            })
            .unwrap();
        assert_eq!(size, (scale > 1).then_some((W * scale, H * scale)));
        d.swap(&mut m, 0).unwrap();
        d.backend.finish().unwrap();
        let s = *stats.lock().unwrap();
        assert_eq!(s.scale, scale);
        assert_eq!((s.fallback_frames, s.view_errors), (0, 0), "{s:?}");
    }
    let s = *stats.lock().unwrap();
    assert_eq!(s.rescales, 3);
    let names = threads.lock().unwrap();
    assert!(!names.is_empty());
    assert!(
        names.iter().all(|n| *n != guest),
        "read on the render thread, not the guest's: {names:?}"
    );
}

// The frame's far edges (docs/graphics/high-resolution.md, "Half-pixel and texel
// offsets"). A full-screen pass drawn the D3D way for a 1:1 texel mapping,
// a quad from -0.5 to the size less 0.5 (window 0 to the size), covers
// every pixel at 1x. Halo's menus composite their blurred, darkened level
// like that. At scale the view shifts positions by `window_shift`, which
// once left the last host columns and rows of such a quad unwritten (one
// at 2x and 3x, two at 4x) with whatever the target held before, and a
// later pass sampling that target spread it inwards.

/// Stage 0 filters bilinearly and clamps, like a blur's upsample.
fn linear_clamp(m: &mut SparseMemory) {
    // D3DTSS_ADDRESSU/V = CLAMP, D3DTSS_MAGFILTER/MINFILTER = LINEAR.
    for (state, value) in [(0, 3), (1, 3), (3, 2), (4, 2)] {
        m.write_u32(L.texture_state_slot(0, state), value).unwrap();
    }
}

/// A quad over `r` sampling a linear texture from texel `(0, 0)` to
/// `texels`.
fn uv_quad<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    r: (f32, f32, f32, f32),
    texels: (f32, f32),
) {
    let mut vb = Vec::new();
    for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [r.0 + (r.2 - r.0) * u, r.1 + (r.3 - r.1) * v, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for f in [texels.0 * u, texels.1 * v] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, vb.len() as u32);
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

#[derive(Clone, Copy, Debug)]
enum EdgePass {
    /// A flat quad (gfx's built-in pre-transformed shader).
    Flat,
    /// A 32x32 render-to-texture level (guest size: only the reference
    /// draws it) upsampled over the frame (a translated program).
    SmallBlur,
    /// A frame-sized level (scaled) drawn by a flat quad over stale
    /// contents, then sampled bilinearly over the frame.
    FrameChain,
}

/// The stale colour every target holds before the pass.
const STALE: u32 = 0xFF10_2030;
/// What the pass draws.
const PASS: u32 = 0xFFFF_8000;
/// A full-screen quad with a 1:1 texel mapping: window 0 to the size.
const FULL: (f32, f32, f32, f32) = (-0.5, -0.5, W as f32 - 0.5, H as f32 - 0.5);

fn edge_scene<B: Backend>(backend: B, pass: EdgePass) -> Device<B> {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_rtt_direct(true);
    set_states(&mut d, &mut m);
    let back = device_field(&m, d3d::layout::device_field::RENDER_TARGET);
    d.clear(&mut m, 0, 0, 0xF0, STALE, 1.0, 0).unwrap();
    match pass {
        EdgePass::Flat => flat_quad(&mut d, &mut m, FULL, PASS),
        EdgePass::SmallBlur | EdgePass::FrameChain => {
            let (level, tex, n) = match pass {
                EdgePass::SmallBlur => {
                    level(&mut m);
                    (LEVEL_A, TEX_A, SIZE)
                }
                _ => {
                    square_level(
                        &mut m,
                        TEX_B,
                        LEVEL_B,
                        0x12,
                        DATA_B,
                        W,
                        &[0x30; FRAME_LEN as usize],
                    );
                    (LEVEL_B, TEX_B, W)
                }
            };
            d.set_render_target(&mut m, level, 0).unwrap();
            set_states(&mut d, &mut m);
            d.clear(&mut m, 0, 0, 0xF0, STALE, 1.0, 0).unwrap();
            let n = n as f32;
            flat_quad(&mut d, &mut m, (-0.5, -0.5, n - 0.5, n - 0.5), PASS);
            d.set_render_target(&mut m, back, 0).unwrap();
            set_states(&mut d, &mut m);
            linear_clamp(&mut m);
            d.set_texture(&mut m, 0, tex).unwrap();
            uv_quad(&mut d, &mut m, FULL, (n, n));
            d.set_texture(&mut m, 0, 0).unwrap();
        }
    }
    let skipped: Vec<_> = d
        .take_events()
        .into_iter()
        .filter(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_)))
        .collect();
    assert!(skipped.is_empty(), "{pass:?}: {skipped:?}");
    d
}

#[test]
fn a_full_screen_pass_reaches_the_far_edges_at_every_scale() {
    let Some(ctx) = context() else { return };
    for pass in [EdgePass::Flat, EdgePass::SmallBlur, EdgePass::FrameChain] {
        let mut plain = edge_scene(GfxBackend::new(share(&ctx)), pass);
        let bb = plain.back_buffer().unwrap();
        let (plain_px, _) = pixels(&mut plain.backend, bb);
        // Not vacuous: at 1x the pass covers the whole frame.
        for (x, y) in [
            (0, 0),
            (W - 1, 0),
            (0, H - 1),
            (W - 1, H - 1),
            (W / 2, H / 2),
        ] {
            assert_eq!(argb(&plain_px, x, y), PASS, "{pass:?} 1x ({x}, {y})");
        }
        for scale in [2, 3, 4] {
            let mut d = edge_scene(scaled(&ctx, scale), pass);
            let bb = d.back_buffer().unwrap();
            let (ref_px, _) = pixels(d.backend.reference_mut(), bb.reference);
            assert!(ref_px == plain_px, "{pass:?} {scale}x: the reference");
            let (view_px, size) = pixels(d.backend.view_mut(), bb.view.expect("scaled"));
            assert_eq!(size, (W * scale, H * scale));
            // The view against the reference upscaled, host pixel by host
            // pixel, counted by host column and row.
            let mut cols = vec![0u32; size.0 as usize];
            let mut rows = vec![0u32; size.1 as usize];
            for y in 0..size.1 {
                for x in 0..size.0 {
                    let i = ((y * size.0 + x) * 4) as usize;
                    if view_px[i..i + 4] != pixel(&ref_px, x / scale, y / scale) {
                        cols[x as usize] += 1;
                        rows[y as usize] += 1;
                    }
                }
            }
            let total: u32 = cols.iter().sum();
            // Lines of the view that are mostly wrong.
            let lines = |counts: &[u32], len: u32| -> Vec<usize> {
                (0..counts.len()).filter(|&i| counts[i] * 2 > len).collect()
            };
            let i = ((size.1 * size.0 - 1) * 4) as usize;
            assert!(
                total == 0,
                "{pass:?} {scale}x ({}x{}): {total} host pixels differ from the reference \
                 upscaled, filling host columns {:?} and rows {:?}; the last pixel is {:?}",
                size.0,
                size.1,
                lines(&cols, size.1),
                lines(&rows, size.0),
                &view_px[i..i + 4],
            );
            let s = d.backend.stats();
            assert_eq!(
                (s.view_errors, s.mismatches),
                (0, 0),
                "{pass:?} {scale}x: {s:?}"
            );
        }
    }
}

// --- The lazy reference (`ScaledBackend::set_lazy_reference`) ----------

fn scaled_lazy(ctx: &GfxContext, scale: u32, lazy: bool) -> Scaled {
    let mut b = scaled(ctx, scale);
    b.set_lazy_reference(lazy);
    b
}

/// The guest's answers (counts, written-back bytes, skipped draws, device
/// memory) and the reference's frame are the same with the lazy reference
/// on and off, and both are the plain backend's.
#[test]
fn the_lazy_reference_gives_the_guest_the_same_answers() {
    let Some(ctx) = context() else { return };
    let (mut plain, _, expect) = frame(GfxBackend::new(share(&ctx)));
    let bb = plain.back_buffer().unwrap();
    let (plain_px, _) = pixels(&mut plain.backend, bb);
    for scale in [2, 4] {
        for lazy in [false, true] {
            let (mut d, _, got) = frame(scaled_lazy(&ctx, scale, lazy));
            assert_eq!(got, expect, "{scale}x, lazy {lazy}");
            let s = d.backend.stats();
            assert_eq!((s.lazy_mismatches, s.mismatches), (0, 0), "{s:?}");
            if lazy {
                assert!(s.reference_deferred > 0, "not vacuous: {s:?}");
            } else {
                assert_eq!(s.reference_deferred, 0, "{s:?}");
            }
            let bb = d.back_buffer().unwrap();
            let (ref_px, _) = pixels(d.backend.reference_mut(), bb.reference);
            assert!(ref_px == plain_px, "{scale}x, lazy {lazy}: reference frame");
        }
    }
}

/// Every phase-2 scene: the reference (run when read) is the plain image
/// and the view matches it box-filtered, with the lazy reference off and on.
#[test]
fn every_scene_is_the_same_with_the_lazy_reference_on_and_off() {
    let Some(ctx) = context() else { return };
    let scenes = [
        Scene::FrameSizedTarget,
        Scene::SmallTargetWritesDepth,
        Scene::GuestDepthLevel,
        Scene::LargerDepthLevel,
        Scene::DepthAlias,
        Scene::ColorOverDepth,
    ];
    for s in scenes {
        for scale in [2, 4] {
            let images: Vec<(Vec<u8>, Vec<u8>)> = [false, true]
                .into_iter()
                .map(|lazy| {
                    let mut d = scene(scaled_lazy(&ctx, scale, lazy), s);
                    let bb = d.back_buffer().unwrap();
                    let (r, _) = pixels(d.backend.reference_mut(), bb.reference);
                    let (v, _) = pixels(d.backend.view_mut(), bb.view.expect("scaled"));
                    let st = d.backend.stats();
                    assert_eq!(st.lazy_mismatches, 0, "{s:?} {scale}x: {st:?}");
                    (r, v)
                })
                .collect();
            assert!(images[0].0 == images[1].0, "{s:?} {scale}x: reference");
            assert!(images[0].1 == images[1].1, "{s:?} {scale}x: view");
        }
        // The default (lazy) against the plain backend, and the view
        // against the reference.
        check_scene(&ctx, s, 2);
    }
}

/// Frames that clear the back buffer and depth whole, draw, and present
/// with nothing reading them: the reference never draws them, until a
/// read needs the last one.
#[test]
fn unread_frames_are_never_drawn_by_the_reference() {
    let Some(ctx) = context() else { return };
    const FRAMES: u32 = 5;
    // Without and with depth testing and writes: a frame's draws then
    // depend on the depth its own whole clear wrote, not on the frame
    // before's.
    for depth in [false, true] {
        let run = |backend: Scaled| {
            let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
            set_states(&mut d, &mut m);
            if depth {
                z_state(&mut m, true, true, Z_LESS);
            }
            for f in 0..FRAMES {
                d.clear(&mut m, 0, 0, 0xF3, 0xFF10_2030, 1.0, 0).unwrap();
                for q in 0..4u32 {
                    let x = (q * 16) as f32;
                    flat_quad(
                        &mut d,
                        &mut m,
                        (x, 4.0 * f as f32, x + 12.0, 40.0),
                        0xFF00_0000 | (f * 40 + q * 7) << 8,
                    );
                }
                d.swap(&mut m, 0).unwrap();
            }
            let per_frame = d.backend.stats();
            let bb = d.back_buffer().unwrap();
            // The device flips: the frame shown last is the front buffer, the
            // back buffer holds the one before.
            let (px, _) = pixels(d.backend.reference_mut(), bb.reference);
            (px, per_frame, d.backend.stats())
        };
        let (eager, _, _) = run(scaled_lazy(&ctx, 4, false));
        let (lazy, before_read, after_read) = run(scaled_lazy(&ctx, 4, true));
        assert!(
            lazy == eager,
            "depth {depth}: the reference's image when read"
        );
        // Each frame: one clear and four draws.
        assert_eq!(before_read.reference_deferred, u64::from(FRAMES) * 5);
        // A frame's whole clear drops the work of the last frame on that
        // buffer (the device flips between two), so only the last two
        // frames' work is pending.
        assert_eq!(
            before_read.reference_dropped,
            u64::from(FRAMES - 2) * 5,
            "{before_read:?}"
        );
        assert_eq!(before_read.reference_replayed, 0, "{before_read:?}");
        assert_eq!(after_read.reference_replayed, 2 * 5, "{after_read:?}");
    }
}

/// A deferred draw into the back buffer samples a guest-size level in
/// place; the level is drawn over afterwards (not deferred). The deferred
/// draw must still see the level as it was: the write runs the queue first.
#[test]
fn a_deferred_draw_sees_a_level_it_samples_as_it_was() {
    let Some(ctx) = context() else { return };
    let run = |backend: Scaled| {
        let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
        d.set_rtt_direct(true);
        set_states(&mut d, &mut m);
        level(&mut m);
        let back = device_field(&m, d3d::layout::device_field::RENDER_TARGET);
        d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
        set_states(&mut d, &mut m);
        flat_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), 0xFFFF_0000);
        d.set_render_target(&mut m, back, 0).unwrap();
        set_states(&mut d, &mut m);
        d.clear(&mut m, 0, 0, 0xF0, 0xFF10_2030, 1.0, 0).unwrap();
        d.set_texture(&mut m, 0, TEX_A).unwrap();
        textured_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0));
        d.set_texture(&mut m, 0, 0).unwrap();
        // The level changes after the back buffer's draw sampled it.
        d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
        set_states(&mut d, &mut m);
        flat_quad(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), 0xFF00_FF00);
        d.set_render_target(&mut m, back, 0).unwrap();
        let stats = d.backend.stats();
        let bb = d.back_buffer().unwrap();
        let (px, _) = pixels(d.backend.reference_mut(), bb.reference);
        (px, stats)
    };
    let (eager, _) = run(scaled_lazy(&ctx, 2, false));
    let (lazy, stats) = run(scaled_lazy(&ctx, 2, true));
    assert_eq!(
        argb(&eager, 10, 10),
        0xFFFF_0000,
        "not vacuous: the red level"
    );
    assert!(lazy == eager, "{stats:?}");
    assert!(stats.reference_deferred > 0, "{stats:?}");
}

// --- E3 lighting, phase 1 (docs/graphics/e3-lighting.md) ----------------
//
// A 32x32 `LIN_D24S8` shadow map (a depth level bound as `pNewZStencil`
// with a 32x32 colour level) gets a slanted caster triangle; the back
// buffer then samples it over the whole frame with PROJECT3D and
// `GREATER`, the first level's lookup, each guest pixel at half its
// coordinates (a shadow texel covers 2x2 guest pixels). Lit is white,
// shadowed black. With E3 lighting the view draws the map at the render
// scale (at least 2) and compares it with one tap; the reference, and so
// the guest, stays retail.

/// The shadow map's texture header, its level, and its texels.
const SMAP_TEX: u32 = MISC + 0x5600;
const SMAP_LEVEL: u32 = MISC + 0x5620;
const SMAP_DATA: u32 = 0x0036_0000;
/// The colour level drawn with it.
const SMAP_COLOR: u32 = MISC + 0x5640;
const SMAP_COLOR_DATA: u32 = 0x0036_8000;
const SMAP: u32 = 32;
const SMAP_LEN: usize = (SMAP * SMAP * 4) as usize;
/// The pixel shader whose output is t0.
const SMAP_PSDEF: u32 = MISC + 0x5700;
const SMAP_PS: u32 = MISC + 0x57F0;
const RS_SHADOWFUNC: u32 = 156;
/// The caster, in the shadow map's window coordinates: every edge slanted.
const CASTER: [(f32, f32); 3] = [(3.3, 2.2), (29.7, 9.9), (8.4, 30.1)];
/// The receiver's reference depth: half the D24 range, so the cleared
/// map (1.0) passes `GREATER` and the caster (near 0) does not.
const SMAP_R: f32 = 8_388_608.0;

fn smap_headers(m: &mut SparseMemory) {
    use d3d::resource::SurfaceHeader;
    m.map(0x8000_0000 | SMAP_DATA, SMAP_LEN as u32);
    m.write(0x8000_0000 | SMAP_DATA, &vec![0xFF; SMAP_LEN])
        .unwrap();
    m.map(0x8000_0000 | SMAP_COLOR_DATA, SMAP_LEN as u32);
    let pitch = SMAP * 4;
    let fmt = (2 << 4) | (0x2E << 8) | (1 << 16);
    let size = (SMAP - 1) | ((SMAP - 1) << 12) | ((pitch / 64 - 1) << 24);
    for (i, w) in [0x0004_0001, SMAP_DATA, 0, fmt, size].iter().enumerate() {
        m.write_u32(SMAP_TEX + 4 * i as u32, *w).unwrap();
    }
    let mut h = SurfaceHeader::linear(gfx::D3DFormat::LinD24S8, SMAP, SMAP, pitch, SMAP_DATA);
    h.pixels.format = fmt;
    h.pixels.size = size;
    h.parent = SMAP_TEX;
    h.write(m, SMAP_LEVEL).unwrap();
    SurfaceHeader::linear(
        gfx::D3DFormat::LinA8R8G8B8,
        SMAP,
        SMAP,
        pitch,
        SMAP_COLOR_DATA,
    )
    .write(m, SMAP_COLOR)
    .unwrap();
}

/// Stage 0 in PROJECT3D, the output its result.
fn smap_shader<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    use shaders::psh::bits::*;
    let def = shaders::psh::PixelShaderDef {
        texture_modes: texture_modes([TEX_PROJECT3D, 0, 0, 0]),
        final_inputs_abcd: inputs(0, 0, 0, REG_T0),
        final_inputs_efg: final_efg(0, 0, REG_T0 | CHAN_ALPHA, 0),
        ..Default::default()
    };
    m.write(SMAP_PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, SMAP_PSDEF].iter().enumerate() {
        m.write_u32(SMAP_PS + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(m, SMAP_PS).unwrap();
}

/// The receiver: the whole frame, texture coordinate (x/2, y/2, r, 1) in
/// the map's texels.
fn receiver<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let (x, y) = (W as f32 * x, H as f32 * y);
        for v in [x, y, 0.5, 1.0] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for v in [x / 2.0, y / 2.0, SMAP_R, 1.0] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, vb.len() as u32);
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    // TEX1 with D3DFVF_TEXCOORDSIZE4(0).
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100 | (2 << 16))
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 36).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

/// One frame's shadow pass and receiver. `linear`: the stage filters
/// linearly (retail's 2x2 percentage-closer filter).
fn shadow_pass<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, linear: bool) {
    let back = device_field(m, d3d::layout::device_field::RENDER_TARGET);
    // Shadow generate: clear the map's depth and stencil, draw the caster.
    d.set_render_target(m, SMAP_COLOR, SMAP_LEVEL).unwrap();
    set_states(d, m);
    d.clear(m, 0, 0, 0x03, 0, 1.0, 0).unwrap();
    z_state(m, true, true, Z_LESS);
    let mut bytes = Vec::new();
    for (x, y) in CASTER {
        bytes.extend(vertex(x, y, 0xFF00_0000));
    }
    m.write(0x8000_0000 | VB_DATA, &bytes).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, bytes.len() as u32);
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    bind_quad(d, m);
    d.draw_vertices(m, 5 /* TRIANGLELIST */, 0, 3).unwrap();
    // Shadow apply: the back buffer samples it.
    d.set_render_target(m, back, 0).unwrap();
    set_states(d, m);
    z_state(m, false, false, Z_ALWAYS);
    d.clear(m, 0, 0, 0xF0, 0xFF10_2030, 1.0, 0).unwrap();
    let filter = if linear { 2 } else { 1 };
    // CLAMP, and the filter.
    for (state, value) in [(0, 3), (1, 3), (3, filter), (4, filter)] {
        m.write_u32(L.texture_state_slot(0, state), value).unwrap();
    }
    smap_shader(d, m);
    m.write_u32(L.render_state_slot(RS_SHADOWFUNC), 0x204)
        .unwrap();
    d.set_texture(m, 0, SMAP_TEX).unwrap();
    receiver(d, m);
    d.set_texture(m, 0, 0).unwrap();
    d.set_pixel_shader(m, 0).unwrap();
    let skipped: Vec<_> = d
        .take_events()
        .into_iter()
        .filter(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_)))
        .collect();
    assert!(skipped.is_empty(), "{skipped:?}");
}

fn shadow_scene<B: Backend>(backend: B, linear: bool) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_rtt_direct(true);
    smap_headers(&mut m);
    shadow_pass(&mut d, &mut m, linear);
    (d, m)
}

/// What the guest can read after the scene: the map's and the colour
/// level's bytes once written back, and the device's memory.
fn shadow_answers<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) -> [Vec<u8>; 3] {
    d.flush_render_targets(m).unwrap();
    [
        m.read_vec(0x8000_0000 | SMAP_DATA, SMAP_LEN).unwrap(),
        m.read_vec(0x8000_0000 | SMAP_COLOR_DATA, SMAP_LEN).unwrap(),
        m.read_vec(0x0040_4000, 0x5000).unwrap(),
    ]
}

fn e3(ctx: &GfxContext, scale: u32) -> Scaled {
    let mut b = scaled(ctx, scale);
    b.set_lighting(d3d::Lighting::E3);
    b
}

/// Whether guest point `(x, y)` of the frame is in the caster's shadow:
/// its map point `(x/2, y/2)` inside the caster.
fn in_shadow(x: f32, y: f32) -> bool {
    let p = (x / 2.0, y / 2.0);
    let side = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
    let [a, b, c] = CASTER;
    let s = [side(a, b), side(b, c), side(c, a)];
    s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0)
}

/// Host pixels of a `w`-wide image at `scale` (`at(x, y)` reads one)
/// whose light disagrees with the caster's exact shadow at their centre;
/// and the pixels that are neither black nor white.
fn shadow_error(w: u32, h: u32, scale: u32, at: impl Fn(u32, u32) -> [u8; 4]) -> (u32, u32) {
    let (mut wrong, mut grey) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            let p = at(x, y);
            if p[..3].iter().any(|&c| c != 0 && c != 0xFF) {
                grey += 1;
            }
            let dark = p[0] < 0x80;
            let s = scale as f32;
            if dark != in_shadow((x as f32 + 0.5) / s, (y as f32 + 0.5) / s) {
                wrong += 1;
            }
        }
    }
    (wrong, grey)
}

/// With E3 lighting the shadow map is made at the render scale in the
/// view, and drawn there; the guest's answers (the map's written-back
/// bytes, the colour level's, device memory) and the reference's frame
/// are the plain backend's, byte for byte.
#[test]
fn e3_shadow_maps_scale_in_the_view_and_the_guest_reads_retail_bytes() {
    let Some(ctx) = context() else { return };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping: depth levels are not sampled in place");
        return;
    }
    let (mut plain, mut pm) = shadow_scene(GfxBackend::new(share(&ctx)), true);
    let bb = plain.back_buffer().unwrap();
    let (plain_px, _) = pixels(&mut plain.backend, bb);
    // Not vacuous: lit and shadowed both.
    assert_eq!(argb(&plain_px, 60, 60), 0xFFFF_FFFF);
    assert_eq!(argb(&plain_px, 20, 24), 0);
    let plain_answers = shadow_answers(&mut plain, &mut pm);
    for scale in [1, 2, 3, 4] {
        let (mut d, mut m) = shadow_scene(e3(&ctx, scale), true);
        let bb = d.back_buffer().unwrap();
        let (ref_px, _) = pixels(d.backend.reference_mut(), bb.reference);
        assert!(
            ref_px == plain_px,
            "{scale}x: the reference is not the plain image"
        );
        let factor = scale.max(2);
        let copies = d.backend.shadow_copies();
        assert_eq!(copies.len(), 1, "{scale}x");
        let (size, copy) = copies[0];
        assert_eq!(size, (SMAP, SMAP));
        let desc = *d.backend.view().store.get(copy).unwrap().desc();
        assert_eq!(
            (desc.width, desc.height),
            (SMAP * factor, SMAP * factor),
            "{scale}x: the view's copy"
        );
        let s = d.backend.stats();
        assert_eq!(s.shadow_scale, factor);
        assert_eq!(
            (s.shadow_draws, s.shadow_textures),
            (1, 1),
            "{scale}x: {s:?}"
        );
        assert_eq!(
            (
                s.view_errors,
                s.fallback_frames,
                s.mismatches,
                s.shadow_refills
            ),
            (0, 0, 0, 0),
            "{scale}x: {s:?}"
        );
        assert_eq!(
            shadow_answers(&mut d, &mut m),
            plain_answers,
            "{scale}x: what the guest reads"
        );
    }
}

/// Retail lighting through the wrapper: no shadow copy, no shadow draw in
/// the view, and every frame byte for byte what it was.
#[test]
fn retail_lighting_leaves_shadow_maps_to_the_reference() {
    let Some(ctx) = context() else { return };
    let (mut plain, mut pm) = shadow_scene(GfxBackend::new(share(&ctx)), true);
    let bb = plain.back_buffer().unwrap();
    let (plain_px, _) = pixels(&mut plain.backend, bb);
    let plain_answers = shadow_answers(&mut plain, &mut pm);
    for scale in [1, 2] {
        let (mut d, mut m) = shadow_scene(scaled(&ctx, scale), true);
        assert_eq!(d.backend.lighting(), d3d::Lighting::Retail);
        let bb = d.back_buffer().unwrap();
        let (ref_px, _) = pixels(d.backend.reference_mut(), bb.reference);
        assert!(ref_px == plain_px, "{scale}x: the reference");
        if let Some(v) = d.backend.view_surface(&bb) {
            // The view samples the reference's 1x map, filtered as retail.
            let (view_px, size) = pixels(d.backend.view_mut(), v);
            let (_, grey) = shadow_error(size.0, size.1, scale, |x, y| {
                let i = ((y * size.0 + x) * 4) as usize;
                [view_px[i], view_px[i + 1], view_px[i + 2], view_px[i + 3]]
            });
            assert!(grey > 0, "{scale}x: retail's filtered edges");
        } else {
            assert_eq!(scale, 1, "only 1x sleeps");
        }
        assert!(d.backend.shadow_copies().is_empty());
        let s = d.backend.stats();
        assert_eq!(
            (
                s.shadow_scale,
                s.shadow_draws,
                s.shadow_textures,
                s.shadow_refills
            ),
            (1, 0, 0, 0),
            "{s:?}"
        );
        assert_eq!(shadow_answers(&mut d, &mut m), plain_answers, "{scale}x");
    }
}

/// E3's shadows are hard (every pixel black or white: one comparison
/// tap) and sharper: against the caster's exact outline they are wrong
/// at fewer host pixels than retail's at the same scale, point sampled or
/// filtered and thresholded.
#[test]
fn e3_shadows_are_one_tap_and_follow_the_caster_closer() {
    let Some(ctx) = context() else { return };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping: depth levels are not sampled in place");
        return;
    }
    for scale in [2, 4] {
        let error = |backend: Scaled, linear: bool| {
            let (mut d, _) = shadow_scene(backend, linear);
            let bb = d.back_buffer().unwrap();
            let v = d.backend.view_surface(&bb).expect("scaled");
            let (px, size) = pixels(d.backend.view_mut(), v);
            assert_eq!(size, (W * scale, H * scale));
            shadow_error(size.0, size.1, scale, |x, y| {
                let i = ((y * size.0 + x) * 4) as usize;
                [px[i], px[i + 1], px[i + 2], px[i + 3]]
            })
        };
        let (e3_wrong, e3_grey) = error(e3(&ctx, scale), true);
        let (point_wrong, point_grey) = error(scaled(&ctx, scale), false);
        let (pcf_wrong, pcf_grey) = error(scaled(&ctx, scale), true);
        eprintln!(
            "{scale}x: host pixels off the caster's outline: E3 {e3_wrong}, retail point \
             {point_wrong}, retail filtered {pcf_wrong} ({pcf_grey} grey)"
        );
        assert_eq!(e3_grey, 0, "{scale}x: E3 compares once");
        assert_eq!(point_grey, 0, "{scale}x: retail point sampling");
        assert!(pcf_grey > 0, "{scale}x: retail's 2x2 filter");
        assert!(
            e3_wrong * 3 < point_wrong * 2 && e3_wrong * 3 < pcf_wrong * 2,
            "{scale}x: E3 {e3_wrong} host pixels off the outline, retail point {point_wrong}, \
             retail filtered {pcf_wrong}"
        );
        // The E3 image filtered with point sampling is the same: the
        // stage's filter no longer matters.
        let (e3_point, _) = error(e3(&ctx, scale), false);
        assert_eq!(e3_point, e3_wrong, "{scale}x");
    }
}

/// The Lighting row changes at a frame boundary, both ways, with no
/// restart and no fallback frame; at render scale 1 E3 wakes the view
/// (frame at 1x, shadow map at 2x) and retail puts it back to sleep. The
/// guest's answers stay the plain backend's throughout.
#[test]
fn lighting_changes_at_a_frame_boundary() {
    let Some(ctx) = context() else { return };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping: depth levels are not sampled in place");
        return;
    }
    // The same frames on a plain backend.
    let (mut plain, mut pm) = shadow_scene(GfxBackend::new(share(&ctx)), true);
    for _ in 0..3 {
        plain.swap(&mut pm, 0).unwrap();
        shadow_pass(&mut plain, &mut pm, true);
    }
    let plain_answers = shadow_answers(&mut plain, &mut pm);
    for scale in [1, 2] {
        let wanted = Arc::new(AtomicU32::new(0));
        let mut b = scaled(&ctx, scale);
        let w = Arc::clone(&wanted);
        b.follow_lighting(move || {
            if w.load(Ordering::Relaxed) == 1 {
                d3d::Lighting::E3
            } else {
                d3d::Lighting::Retail
            }
        });
        let (mut d, mut m) = shadow_scene(b, true);
        for (step, e3_on) in [true, false, true].into_iter().enumerate() {
            wanted.store(e3_on as u32, Ordering::Relaxed);
            d.swap(&mut m, 0).unwrap();
            let want = if e3_on {
                d3d::Lighting::E3
            } else {
                d3d::Lighting::Retail
            };
            assert_eq!(d.backend.lighting(), want, "{scale}x step {step}");
            shadow_pass(&mut d, &mut m, true);
            let bb = d.back_buffer().unwrap();
            let s = d.backend.stats();
            match d.backend.view_surface(&bb) {
                Some(v) => {
                    let (px, size) = pixels(d.backend.view_mut(), v);
                    assert_eq!(size, (W * scale, H * scale), "{scale}x step {step}");
                    let (_, grey) = shadow_error(size.0, size.1, scale, |x, y| {
                        let i = ((y * size.0 + x) * 4) as usize;
                        [px[i], px[i + 1], px[i + 2], px[i + 3]]
                    });
                    assert_eq!(grey == 0, e3_on, "{scale}x step {step}: {grey} grey pixels");
                }
                None => assert!(scale == 1 && !e3_on, "{scale}x step {step}: asleep"),
            }
            assert_eq!(s.shadow_scale, if e3_on { scale.max(2) } else { 1 });
            assert_eq!(
                d.backend.shadow_copies().len(),
                e3_on as usize,
                "{scale}x step {step}"
            );
            assert_eq!(
                (s.fallback_frames, s.view_errors, s.mismatches),
                (0, 0, 0),
                "{scale}x step {step}: {s:?}"
            );
        }
        let s = d.backend.stats();
        assert_eq!(s.shadow_draws, 2, "{scale}x: the two E3 frames: {s:?}");
        assert_eq!(shadow_answers(&mut d, &mut m), plain_answers, "{scale}x");
    }
}

// --- E3 lighting through Halo's render-target pool ------------------------
//
// In a level the shadow map shares its memory with the colour levels the
// frame's later passes draw into (a 320x240 and two 160x120 `LIN_A8R8G8B8`
// levels over the 512x512 `LIN_D24S8` map; docs/graphics/d3d.md,
// "Overlapping levels on the GPU"). So between one frame's shadow pass and
// the next, a colour level is drawn over the map's bytes, the device moves
// the map into the alias mirror (`mirror_store`), and binding the map again
// fills it from there (`mirror_load`), which leaves the view's copy stale
// until the shadow pass clears it. Here a 16x16 colour level over the 32x32
// map's first rows stands for the pool.

/// The colour level over the map's first 1 KiB, and its texture.
const POOL_TEX: u32 = MISC + 0x5660;
const POOL_COLOR: u32 = MISC + 0x5680;
const POOL: u32 = 16;

fn pool_header(m: &mut SparseMemory) {
    use d3d::resource::SurfaceHeader;
    let pitch = POOL * 4;
    // LIN_A8R8G8B8 (0x12), 2D.
    let fmt = (2 << 4) | (0x12 << 8) | (1 << 16);
    let size = (POOL - 1) | ((POOL - 1) << 12) | ((pitch / 64 - 1) << 24);
    for (i, w) in [0x0004_0001, SMAP_DATA, 0, fmt, size].iter().enumerate() {
        m.write_u32(POOL_TEX + 4 * i as u32, *w).unwrap();
    }
    let mut h = SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, POOL, POOL, pitch, SMAP_DATA);
    h.pixels.format = fmt;
    h.pixels.size = size;
    h.parent = POOL_TEX;
    h.write(m, POOL_COLOR).unwrap();
}

/// A later pass of the frame (bloom, say): a triangle into the pool's
/// colour level, over the shadow map's bytes. The back buffer is the
/// target again after it.
fn pool_pass<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    let back = device_field(m, d3d::layout::device_field::RENDER_TARGET);
    d.set_render_target(m, POOL_COLOR, 0).unwrap();
    set_states(d, m);
    z_state(m, false, false, Z_ALWAYS);
    let mut bytes = Vec::new();
    for (x, y) in [
        (0.0, 0.0),
        (2.0 * POOL as f32, 0.0),
        (0.0, 2.0 * POOL as f32),
    ] {
        bytes.extend(vertex(x, y, 0xFF40_8020));
    }
    m.write(0x8000_0000 | VB_DATA, &bytes).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, bytes.len() as u32);
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    bind_quad(d, m);
    d.draw_vertices(m, 5 /* TRIANGLELIST */, 0, 3).unwrap();
    d.set_render_target(m, back, 0).unwrap();
}

/// `frames` frames of Halo's order: shadow pass, then the pool's pass,
/// then the swap; the last frame stops after its shadow pass, so the
/// back buffer holds its shadows (the pool's bytes are the map's first
/// rows, so [`shadow_answers`] covers them).
fn pool_scene<B: Backend>(backend: B, frames: u32) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_rtt_direct(true);
    smap_headers(&mut m);
    pool_header(&mut m);
    for f in 0..frames {
        shadow_pass(&mut d, &mut m, true);
        if f + 1 < frames {
            pool_pass(&mut d, &mut m);
            d.swap(&mut m, 0).unwrap();
        }
    }
    (d, m)
}

/// The view's image of the back buffer, and its error against the
/// caster ([`shadow_error`]).
fn view_shadows(d: &mut Device<Scaled>, scale: u32) -> (Vec<u8>, (u32, u32)) {
    let bb = d.back_buffer().unwrap();
    let v = d.backend.view_surface(&bb).expect("the view is awake");
    let (px, size) = pixels(d.backend.view_mut(), v);
    assert_eq!(size, (W * scale, H * scale));
    let err = shadow_error(size.0, size.1, scale, |x, y| {
        let i = ((y * size.0 + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    });
    (px, err)
}

/// Whether host pixel `(x, y)` at `scale` is within two map texels of
/// the caster's outline (retail's 2x2 filter reaches one).
fn near_outline(x: u32, y: u32, scale: u32) -> bool {
    let s = scale as f32;
    let p = ((x as f32 + 0.5) / s / 2.0, (y as f32 + 0.5) / s / 2.0);
    let [a, b, c] = CASTER;
    [(a, b), (b, c), (c, a)].iter().any(|&(a, b)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (ex, ey) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
        ex * ex + ey * ey < 4.0
    })
}

/// Through the pool, as in a level: the map goes into the alias mirror
/// and back every frame, and every frame's shadows are still drawn from
/// the view's scaled copy with one tap (the image is the single-pass
/// scene's exactly, no refill), different from retail's at the shadow
/// edges and only there. The guest's answers (the map and so the pool,
/// the colour level, device memory) and the reference's frame are the
/// plain backend's; retail through the wrapper draws nothing extra.
#[test]
fn e3_shadow_maps_survive_halos_render_target_pool() {
    let Some(ctx) = context() else { return };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping: depth levels are not sampled in place");
        return;
    }
    const FRAMES: u32 = 3;
    let (mut plain, mut pm) = pool_scene(GfxBackend::new(share(&ctx)), FRAMES);
    let st = plain.stats();
    assert!(
        st.mirror_stores >= FRAMES as u64 && st.mirror_loads >= FRAMES as u64 - 1,
        "the map goes through the alias mirror: {st:?}"
    );
    let bb = plain.back_buffer().unwrap();
    let (plain_px, _) = pixels(&mut plain.backend, bb);
    assert_eq!(argb(&plain_px, 60, 60), 0xFFFF_FFFF);
    assert_eq!(argb(&plain_px, 20, 24), 0);
    let plain_answers = shadow_answers(&mut plain, &mut pm);
    for scale in [1, 2, 4] {
        // E3 through the pool against the single-pass scene.
        let (mut once, _) = shadow_scene(e3(&ctx, scale), true);
        let (want, _) = view_shadows(&mut once, scale);
        let (mut d, mut m) = pool_scene(e3(&ctx, scale), FRAMES);
        let st = d.stats();
        assert!(st.mirror_loads >= FRAMES as u64 - 1, "{scale}x: {st:?}");
        let (px, (wrong, grey)) = view_shadows(&mut d, scale);
        assert_eq!(grey, 0, "{scale}x: one tap");
        assert!(
            px == want,
            "{scale}x: the pool's last frame is the single pass's image ({wrong} off the outline)"
        );
        let s = d.backend.stats();
        assert_eq!(
            (s.shadow_draws, s.shadow_textures, s.shadow_refills),
            (FRAMES as u64, FRAMES as u64, 0),
            "{scale}x: every frame drawn and sampled at {}x: {s:?}",
            scale.max(2)
        );
        assert_eq!(
            (s.view_errors, s.fallback_frames, s.mismatches),
            (0, 0, 0),
            "{scale}x: {s:?}"
        );
        let bb = d.back_buffer().unwrap();
        let (ref_px, _) = pixels(d.backend.reference_mut(), bb.reference);
        assert!(ref_px == plain_px, "{scale}x: the reference");
        assert_eq!(
            shadow_answers(&mut d, &mut m),
            plain_answers,
            "{scale}x: what the guest reads"
        );
        if scale == 1 {
            continue;
        }
        // Retail at the same scale: filtered edges where E3's are hard.
        let (mut r, mut rm) = pool_scene(scaled(&ctx, scale), FRAMES);
        let (retail_px, (retail_wrong, retail_grey)) = view_shadows(&mut r, scale);
        assert!(retail_grey > 0, "{scale}x: retail's 2x2 filter");
        assert!(
            wrong < retail_wrong,
            "{scale}x: E3 {wrong}, retail {retail_wrong} off the outline"
        );
        let w = W * scale;
        let mut differ = 0;
        for (i, (a, b)) in px.chunks(4).zip(retail_px.chunks(4)).enumerate() {
            if a != b {
                let (x, y) = (i as u32 % w, i as u32 / w);
                assert!(
                    near_outline(x, y, scale),
                    "{scale}x: ({x}, {y}) differs away from the shadow's edges"
                );
                differ += 1;
            }
        }
        assert!(differ > 0, "{scale}x: E3 and retail differ at the edges");
        eprintln!("{scale}x: {differ} host pixels differ from retail, all at the shadow's edges");
        let s = r.backend.stats();
        assert_eq!(
            (s.shadow_draws, s.shadow_textures),
            (0, 0),
            "{scale}x retail: {s:?}"
        );
        assert!(r.backend.shadow_copies().is_empty());
        assert_eq!(
            shadow_answers(&mut r, &mut rm),
            plain_answers,
            "{scale}x retail"
        );
    }
}
