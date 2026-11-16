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
    let size = |b: &Scaled, s: Option<gfx::surface::SurfaceId>| {
        let d = *b.view().store.get(s.unwrap()).unwrap().desc();
        (d.width, d.height)
    };
    assert_eq!(size(&b, back.view), (2560, 1920));
    assert_eq!(size(&b, depth.view), (2560, 1920));
    assert_eq!(small.view, None, "another size stays in the reference");
    assert_eq!(
        level.view, None,
        "render-to-texture levels stay at 1x in phase 1"
    );
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
