//! `ScaledBackend` (docs/high-resolution.md, phase 1): a reference backend
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
use d3d::{Backend, Device, Event, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::vertex::fvf;

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
        d.clear(&mut m, 0, 0, 0xF0, 0xFF10_2030, 1.0, 0).unwrap();
        // The level: a rectangle clear and a flat quad.
        d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
        rects(&mut m, &[[0, 0, 16, 32]]);
        d.clear(&mut m, 1, RECTS, 0xF0, 0xFFFF_0000, 1.0, 0)
            .unwrap();
        set_states(&mut d, &mut m);
        flat_quad(&mut d, &mut m, (8.0, 8.0, 24.0, 20.0), 0xFF00_FF00);
        // The back buffer: a rectangle clear, a quad through two scissor
        // rectangles, the level drawn over the right half, a triangle.
        d.set_render_target(&mut m, back, 0).unwrap();
        rects(&mut m, &[[5, 7, 29, 19]]);
        d.clear(&mut m, 1, RECTS, 0xF0, 0xFF80_8080, 1.0, 0)
            .unwrap();
        rects(&mut m, &[[2, 30, 20, 50], [12, 40, 31, 61]]);
        d.set_scissors(&mut m, 2, 0, RECTS).unwrap();
        flat_quad(&mut d, &mut m, (0.0, 0.0, 64.0, 64.0), 0xFF00_00FF);
        d.set_scissors(&mut m, 0, 0, 0).unwrap();
        d.set_texture(&mut m, 0, TEX_A).unwrap();
        textured_quad(&mut d, &mut m, (32.0, 0.0, 64.0, 32.0));
        d.set_texture(&mut m, 0, 0).unwrap();
        triangle(&mut d, &mut m, 0xFFFF_FF00);
        // A visibility test around two quads, read back.
        d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE)
            .unwrap();
        push_words(&mut m, &[(2 << 18) | hw::CLEAR_REPORT_VALUE, 1, 1]);
        inline_quad(&mut m, 0.0, 0.0, 32.0, 16.0, [0, 255, 0, 255]);
        inline_quad(&mut m, 40.0, 50.0, 47.0, 61.0, [0, 0, 255, 255]);
        let out = call(
            &mut d,
            &mut m,
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
        call(&mut d, &mut m, GET, r, &[OUT]).unwrap();
        count = m.read_u32(OUT).unwrap();
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
        assert_eq!(s.view_frames, 1);
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

// Phase 2: what the view used to leave out (docs/high-resolution.md,
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

// The frame's far edges (docs/high-resolution.md, "Half-pixel and texel
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
