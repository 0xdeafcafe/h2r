//! `ScaledBackend` (docs/graphics/high-resolution.md, "Exact mode: reference and view"): a reference backend
//! at the guest's size answers the guest, a view backend renders the frame
//! buffers at a multiple of it. One synthetic frame (render-to-texture with
//! a write-back, scissored and rectangle-cleared draws, a textured draw over
//! the level, a visibility test) must give the guest the same answers with
//! one plain backend and with the wrapper at 1x, 2x and 4x, and the view's
//! image box-filtered down must be the reference's. Synthetic resources;
//! the GPU tests skip without an adapter.

use crate::common::*;
use d3d::abi::Regs;
use d3d::hw::method as hw;
use d3d::scaled::ScaledBackend;
use d3d::{
    Backend, Device, Event, GfxBackend, SparseMemory, ThreadedBackend, UploadPolicy,
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
    ScaledBackend::with_single_copy(
        GfxBackend::new(share(ctx)),
        GfxBackend::new(share(ctx)),
        scale,
        false,
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
pub(super) fn textured_quad<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, r: (f32, f32, f32, f32)) {
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
    assert_eq!(size(&b, small.view), (320, 240));
    assert_eq!(
        size(&b, level.view),
        (2560, 1920),
        "a back-buffer-sized render-to-texture level scales"
    );
    assert_eq!(size(&b, bloom.view), (128, 128), "smaller levels stay at 1x");
    let r = *b.reference().store.get(back.reference).unwrap().desc();
    assert_eq!(
        (r.width, r.height),
        (640, 480),
        "the reference keeps the guest's size"
    );
    // A small target can hold a different scene in each copy.
    let snap = b.snapshot_surface(small, false).unwrap().unwrap();
    assert_ne!(snap.view.as_ref().unwrap().texture, snap.reference.texture);
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

// Phase 2: what the view used to leave out (docs/graphics/high-resolution.md, "Exact mode: reference and view").
// Each scene is drawn by a plain backend and by the
// wrapper; the reference must be the plain image and the view, box-filtered
// to the guest size, the reference. Every edge here is on a whole pixel, so
// the match is exact.

const L: d3d::GuestLayout = d3d::GuestLayout::HALO2;
const RS_ZENABLE: u32 = 143;
const RS_ZWRITEENABLE: u32 = 64;
const Z_LESS: u32 = 0x201;
const Z_ALWAYS: u32 = 0x207;

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

#[test]
fn clipped_world_bounds_reach_both_edges_of_the_presented_player_view() {
    use d3d::backend::PixelRect;

    let grid = d3d::scaled::RenderScale::between((640, 480), (5120, 2880)).unwrap();
    // A right-hand player view with a water plane clipped at the left
    // boundary and a fog plane clipped at the right boundary.
    let view = (320, 0, 320, 480);
    let physical = |x, width| {
        let rect = PixelRect { x, y: 0, width, height: 480 }.projected_x(view, 0.75);
        grid.rect(rect.x, rect.y, rect.width, rect.height)
    };
    assert_eq!(physical(320, 320), [2560, 0, 2560, 2880]);
    assert_eq!(physical(320, 200), [2560, 0, 1520, 2880]);
    assert_eq!(physical(440, 200), [3600, 0, 1520, 2880]);
}

#[test]
fn render_diagnostics_read_display_allocations_on_the_worker() {
    use d3d::backend::RenderExtents;

    let Some(ctx) = context() else { return };
    let guest = (16, 12);
    let display = (39, 25);
    let small = (8, 6);
    let scale = d3d::scaled::RenderScale::between(guest, display).unwrap();
    let color_format = gfx::D3DFormat::LinA8R8G8B8;
    for single in [false, true] {
        let scaled = ScaledBackend::with_resolution(
            GfxBackend::new(share(&ctx)),
            GfxBackend::new(share(&ctx)),
            scale,
            single,
        ).unwrap();
        let mut backend = ThreadedBackend::new(scaled).unwrap();
        let color = backend.create_surface(color_format, guest.0, guest.1, false).unwrap();
        let depth = backend.create_surface(gfx::D3DFormat::LinD24S8, guest.0, guest.1, true).unwrap();
        let blur = backend.create_texture_target(color_format, small.0, small.1).unwrap();
        backend.clear(color, None, gfx::Clear::all([1.0, 0.0, 0.0, 1.0])).unwrap();
        let frame_sample = backend.snapshot_surface(color, false).unwrap().unwrap();
        let blur_sample = backend.surface_texture(blur).unwrap().unwrap();

        assert_eq!(
            backend.render_extents(color, Some(depth), [Some(&frame_sample), None, Some(&blur_sample), None]),
            RenderExtents {
                color: Some(display),
                depth: Some(display),
                textures: [Some(display), None, Some(small), None],
            },
        );
        assert_eq!(backend.render_extents(blur, None, [None; 4]).color, Some(small));

        let (size, pixels) = backend.read_display_surface(color).unwrap().unwrap();
        assert_eq!(size, display);
        assert_eq!(pixels.len(), (display.0 * display.1 * 4) as usize);
        assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 255, 255]));
    }
}

#[test]
fn fractional_feedback_preserves_patterned_guest_cells_and_snapshot_lifetime() {
    patterned_feedback((149, 101));
}

#[test]
fn water_frame_sample_includes_the_rightmost_presented_column_at_sixteen_by_nine() {
    patterned_feedback((192, 108));
}

fn patterned_feedback(output: (u32, u32)) {
    let Some(ctx) = context() else { return };
    let grid = d3d::scaled::RenderScale::between((W, H), output).unwrap();
    let mut backend = ScaledBackend::with_resolution(
        GfxBackend::new(share(&ctx)), GfxBackend::new(share(&ctx)), grid, true,
    ).unwrap();
    let format = gfx::D3DFormat::LinA8R8G8B8;
    let frame = backend.create_surface(format, W, H, false).unwrap();
    let desc = gfx::TextureDesc::new(format, gfx::texture::TextureKind::D2, W, H, 1, 1);
    let cell = |x: u32, y: u32| [x as u8, y as u8, (x ^ y) as u8, 255];
    let guest: Vec<u8> = (0..H).flat_map(|y| (0..W).flat_map(move |x| cell(x, y))).collect();
    assert!(backend.write_surface(frame, &desc, &guest).unwrap());
    let snapshot = backend.snapshot_surface(frame, true).unwrap().unwrap().view.unwrap().texture;
    let readback = backend.read_surface(
        frame, format, W, H, d3d::GuestLayoutKind::Linear { pitch: W * 4 },
    ).unwrap();
    assert_eq!(readback, guest);
    backend.clear(frame, None, gfx::Clear::all([0.0; 4])).unwrap();
    backend.view_mut().flush().unwrap();
    assert_eq!((snapshot.width(), snapshot.height()), output);
    let pixels = gfx::readback::read_texture(&ctx, &snapshot, wgpu::TextureAspect::All, 4, output);
    for y in 0..H {
        for x in 0..W {
            let [left, top, width, height] = grid.rect(x, y, 1, 1);
            for row in top..top + height {
                for col in left..left + width {
                    let at = ((row * output.0 + col) * 4) as usize;
                    assert_eq!(&pixels[at..at + 4], &cell(x, y), "snapshot at ({col}, {row})");
                }
            }
        }
    }
}

// --- The reference --------------------------------------------------------

/// The guest's answers (counts, written-back bytes, skipped draws, device
/// memory) and the reference's frame are the plain backend's.
#[test]
fn the_reference_gives_the_guest_the_plain_answers() {
    let Some(ctx) = context() else { return };
    let (mut plain, _, expect) = frame(GfxBackend::new(share(&ctx)));
    let bb = plain.back_buffer().unwrap();
    let (plain_px, _) = pixels(&mut plain.backend, bb);
    for scale in [2, 4] {
        let (mut d, _, got) = frame(scaled(&ctx, scale));
        assert_eq!(got, expect, "{scale}x");
        let s = d.backend.stats();
        assert_eq!(s.mismatches, 0, "{s:?}");
        let bb = d.back_buffer().unwrap();
        let (ref_px, _) = pixels(d.backend.reference_mut(), bb.reference);
        assert!(ref_px == plain_px, "{scale}x: reference frame");
    }
}

// --- The Lighting row (docs/graphics/e3-lighting-setting.md) -------------
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

#[test]
fn single_copy_rtt_binds_the_scaled_allocation_and_snapshots_preserve_its_texels() {
    let Some(ctx) = context() else { return };
    let mut b = ScaledBackend::new(GfxBackend::new(share(&ctx)), GfxBackend::new(share(&ctx)), 4).unwrap();
    let format = gfx::D3DFormat::LinA8R8G8B8;
    b.create_surface(format, W, H, false).unwrap();
    let rtt = b.create_texture_target(format, W, H).unwrap();
    let scaled = b.view().store.get(b.view_surface(&rtt).unwrap()).unwrap().texture().clone();
    let live = b.surface_texture(rtt).unwrap().unwrap();
    assert_eq!(live.view.unwrap().texture, scaled);
    assert_eq!(live.reference.texture, scaled);
    b.clear(rtt, None, gfx::Clear::all([1., 0., 0., 1.])).unwrap();
    let snap = b.snapshot_surface(rtt, true).unwrap().unwrap().view.unwrap().texture;
    b.clear(rtt, None, gfx::Clear::all([0., 1., 0., 1.])).unwrap();
    b.view_mut().flush().unwrap();
    assert_ne!(snap, scaled);
    assert_eq!((snap.width(), snap.height()), (W * 4, H * 4));
    let old = gfx::readback::read_texture(&ctx, &snap, wgpu::TextureAspect::All, 4, (1, 1));
    assert_eq!(old, [0, 0, 255, 255]);
    assert_eq!(b.stats().downsamples, 0);
    assert_eq!(b.stats().cpu_readbacks, 0);
}

#[test]
fn single_copy_depth_and_stencil_read_at_guest_resolution() {
    let Some(ctx) = context() else { return };
    if !ctx.caps.depth32_stencil8 { return; }
    let mut b = ScaledBackend::new(GfxBackend::new(share(&ctx)), GfxBackend::new(share(&ctx)), 4).unwrap();
    let c = b.create_surface(gfx::D3DFormat::LinA8R8G8B8, W, H, false).unwrap();
    let df = gfx::D3DFormat::LinD24S8;
    let z = b.create_surface(df, W, H, true).unwrap();
    b.clear(c, Some(z), gfx::Clear { color: None, depth: Some(1.0), stencil: Some(0x5a) }).unwrap();
    b.clear_rects(c, Some(z), &d3d::RectClear {
        rects: vec![d3d::PixelRect { x: 3, y: 4, width: 5, height: 6 }], color: None,
        depth: Some(0.0), stencil: Some(0xa5),
    }).unwrap();
    // Reinterpret the first cleared word through an offset, one-texel
    // colour header. It covers a 4x4 block in the GPU alias texture.
    let desc = gfx::TextureDesc::new(gfx::D3DFormat::LinA8R8G8B8, gfx::texture::TextureKind::D2, 1, 1, 1, 1);
    let offset = ((4 * W + 3) * 4) as usize;
    let alias = b.depth_alias_texture(z, df, W, H, W * 4, 1, offset, &desc, Default::default())
        .unwrap().unwrap().view.unwrap().texture;
    b.view_mut().flush().unwrap();
    assert_eq!((alias.width(), alias.height()), (4, 4));
    assert_eq!(gfx::readback::read_texture(&ctx, &alias, wgpu::TextureAspect::All, 4, (1, 1)), [0, 0, 0xa5, 0]);
    let small = gfx::TextureDesc::new(gfx::D3DFormat::LinL8, gfx::texture::TextureKind::D2, 1, 1, 1, 1);
    let luminance = b.depth_alias_texture(z, df, W, H, W * 4, 1, offset, &small, Default::default())
        .unwrap().unwrap().view.unwrap().texture;
    b.view_mut().flush().unwrap();
    assert_eq!(gfx::readback::read_texture(&ctx, &luminance, wgpu::TextureAspect::All, 4, (1, 1)), [0xa5, 0xa5, 0xa5, 255]);
    assert_eq!(b.stats().downsamples, 0);
    let words = b.read_depth(z, df, W, H, d3d::GuestLayoutKind::Linear { pitch: W * 4 }).unwrap().unwrap();
    assert_eq!(words.len(), (W * H * 4) as usize);
    assert_eq!(u32::from_le_bytes(pixel(&words, 3, 4)), 0xa5);
    assert_eq!(u32::from_le_bytes(pixel(&words, 8, 10)), 0xffff_ff5a);
    assert_eq!((b.stats().downsamples, b.stats().cpu_readbacks), (1, 1));
}

#[test]
fn single_copy_retains_pixels_and_count_tickets_when_returning_to_one() {
    let Some(ctx) = context() else { return };
    let mut b = ScaledBackend::new(GfxBackend::new(share(&ctx)), GfxBackend::new(share(&ctx)), 4).unwrap();
    let format = gfx::D3DFormat::LinA8R8G8B8;
    let color = b.create_surface(format, W, H, false).unwrap();
    b.clear(color, None, gfx::Clear::all([0., 1., 0., 1.])).unwrap();
    b.clear_pixel_count().unwrap();
    let ticket = b.report_pixel_count().unwrap().unwrap();
    b.follow_scale(|| 1);
    b.present(color, 0).unwrap();
    assert_eq!(b.scale(), 1);
    assert_eq!(b.read_pixel_count(ticket).unwrap(), 0);
    let bytes = b.read_surface(color, format, W, H, d3d::GuestLayoutKind::Linear { pitch: W * 4 }).unwrap();
    assert_eq!(pixel(&bytes, 15, 19), [0, 255, 0, 255]);
}

#[test]
fn single_copy_reinterprets_colour_memory_on_gpu() {
    use gfx::texture::{DecodeOptions, TextureKind, decode_texture};
    use gfx::D3DFormat::*;

    let Some(ctx) = context() else { return };
    let mut b = ScaledBackend::new(GfxBackend::new(share(&ctx)), GfxBackend::new(share(&ctx)), 4).unwrap();
    let color = b.create_surface(LinA8R8G8B8, W, H, false).unwrap();
    let source = gfx::TextureDesc::new(LinA8R8G8B8, TextureKind::D2, W, H, 1, 1);
    let bytes: Vec<u8> = (0..W * H * 4).map(|i| (i.wrapping_mul(53) >> 1) as u8).collect();
    assert!(b.write_surface(color, &source, &bytes).unwrap());

    let options = DecodeOptions { bc_supported: false, signed: [true, false, true, false] };
    for format in [LinL8, LinR5G6B5, A8L8, Dxt1, Dxt3, Dxt5, Yuy2, LinR8G8B8A8] {
        let desc = gfx::TextureDesc::new(format, TextureKind::D2, 8, 8, 1, 1);
        let expected = decode_texture(&desc, &bytes[3..], None, options).unwrap();
        let alias = b.depth_alias_texture(color, LinA8R8G8B8, W, H, W * 4, 1, 3, &desc, options)
            .unwrap().unwrap().view.unwrap().texture;
        b.view_mut().flush().unwrap();
        let (w, h) = (alias.width(), alias.height());
        assert_eq!((w, h), (32, 32));
        let pixels = gfx::readback::read_texture(&ctx, &alias, wgpu::TextureAspect::All, 4, (w, h));
        for y in 0..h {
            for x in 0..w {
                let actual = ((y * w + x) * 4) as usize;
                let guest = ((y / 4 * desc.width + x / 4) * 4) as usize;
                assert_eq!(pixels[actual..actual + 4], expected.levels[0].data[guest..guest + 4], "{format:?} {x},{y}");
            }
        }
    }
    assert_eq!((b.stats().downsamples, b.stats().cpu_readbacks), (0, 0));
}

#[test]
fn fractional_frame_aliases_keep_cube_faces_and_volume_mips_at_guest_size() {
    use gfx::texture::{DecodeOptions, TextureKind, decode_texture};

    let Some(ctx) = context() else { return };
    let grid = d3d::scaled::RenderScale::between((W, H), (149, 101)).unwrap();
    let mut backend = ScaledBackend::with_resolution(
        GfxBackend::new(share(&ctx)), GfxBackend::new(share(&ctx)), grid, true,
    ).unwrap();
    let format = gfx::D3DFormat::LinA8R8G8B8;
    let frame = backend.create_surface(format, W, H, false).unwrap();
    let src = gfx::TextureDesc::new(format, TextureKind::D2, W, H, 1, 1);
    let bytes: Vec<_> = (0..W * H * 4).map(|i| (i ^ (i >> 5)) as u8).collect();
    assert!(backend.write_surface(frame, &src, &bytes).unwrap());

    let options = DecodeOptions::default();
    for (kind, depth, layers) in [(TextureKind::Cube, 1, 6), (TextureKind::Volume, 4, 4)] {
        let desc = gfx::TextureDesc::new(gfx::D3DFormat::A8B8G8R8, kind, 8, 8, depth, 4);
        let expected = decode_texture(&desc, &bytes, None, options).unwrap();
        let alias = backend.depth_alias_texture(frame, format, W, H, W * 4, 1, 0, &desc, options)
            .unwrap().unwrap().view.unwrap().texture;
        backend.view_mut().flush().unwrap();
        assert_eq!((alias.width(), alias.height(), alias.depth_or_array_layers()), (8, 8, layers));
        assert_eq!(alias.mip_level_count(), 4);
        let first_slice = gfx::readback::read_texture(&ctx, &alias, wgpu::TextureAspect::All, 4, (8, 8));
        assert_eq!(first_slice.as_slice(), &expected.levels[0].data[..8 * 8 * 4], "{kind:?}");
    }
}

#[test]
fn ui_fit_full_canvas_clip_spans_the_display_and_interior_clips_stay_fitted() {
    use d3d::backend::{PixelRect, UiFit};
    let full = PixelRect { x: 0, y: 0, width: 640, height: 480 };
    for output in [(1920, 1200), (2560, 1080)] {
        let fit = UiFit::about(output, [320., 240.]).unwrap();
        assert_eq!(fit.map_rect(full), full, "{output:?}");
        let [sx, sy, ox, oy] = fit.transform;
        // Left canvas edge stays on the display; the interior edge is fitted.
        let part = fit.map_rect(PixelRect { x: 0, y: 0, width: 160, height: 480 });
        assert_eq!((part.x, part.y, part.height), (0, 0, 480));
        assert_eq!(part.width, (160. * sx + ox).round() as u32, "{output:?}");
        let inner = fit.map_rect(PixelRect { x: 100, y: 100, width: 200, height: 200 });
        assert_eq!(inner.x, (100. * sx + ox).round() as u32);
        assert_eq!(inner.y, (100. * sy + oy).round() as u32);
    }
}
