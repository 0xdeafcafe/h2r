//! The render thread (`d3d::ThreadedBackend`): the same device sequence
//! through a backend directly and through the render thread must give the
//! same backend calls, the same pixels, the same guest memory and the same
//! device stats. Synthetic resources; the GPU test skips without an adapter.

mod common;

use common::*;
use d3d::{
    Backend, Device, GfxBackend, GuestMemory, RecordingBackend, SparseMemory, ThreadedBackend,
    UploadPolicy,
};
use gfx::vertex::fvf;

const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;
const RECTS: u32 = MISC + 0x7100;
const TEX_A: u32 = MISC + 0x4000;
const LEVEL_A: u32 = MISC + 0x4200;
const DATA_A: u32 = 0x0030_0000;
const SIZE: u32 = 32;
const LEN: usize = (SIZE * SIZE * 4) as usize;

/// A linear 32x32 `LIN_A8R8G8B8` texture with one level over `DATA_A`.
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

fn textured_quad<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, rect: [f32; 4], color: u32) {
    let [x0, y0, x1, y1] = rect;
    let mut vb = Vec::new();
    for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [x0 + (x1 - x0) * u, y0 + (y1 - y0) * v, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&color.to_le_bytes());
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

/// Render-to-texture into level A, sampled in place and through memory,
/// a guest store into the level, waits that write it back, fences, a
/// visibility test, a swap, and the frame drawn again. Returns the device
/// for the caller to read the back buffer, and every value the guest
/// could observe along the way: entry results, the device's guest words
/// (GPU time, completion word, swap count and fences, register window)
/// and the level's bytes after each wait.
fn scenario<B: Backend>(backend: B) -> (Device<B>, SparseMemory, Vec<String>) {
    let mut seen = Vec::new();
    let dev = d3d::GuestLayout::HALO2.device;
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    level(&mut m);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    for frame in 0..3u32 {
        d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000 | frame, 1.0, 0)
            .unwrap();
        d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
        m.write(RECTS, &[0u32, 0, 16, 32].map(u32::to_le_bytes).concat())
            .unwrap();
        d.clear(&mut m, 1, RECTS, 0xF0, 0xFFFF_0000 ^ frame, 1.0, 0)
            .unwrap();
        d.set_render_target(&mut m, back, 0).unwrap();
        d.set_texture(&mut m, 0, TEX_A).unwrap();
        for (slot, v) in [(RS_ALPHABLENDENABLE, 1), (RS_SRCBLEND, 0x302), (RS_DESTBLEND, 0x303)] {
            d.set_render_state(&mut m, slot, v).unwrap();
        }
        textured_quad(&mut d, &mut m, [0.0, 0.0, 32.0, 32.0], 0xC0FF_FFFF);
        d.set_render_state(&mut m, RS_ALPHABLENDENABLE, 0).unwrap();
        // The guest stores into the level, then waits on it (write-back).
        m.write(0x8000_0000 | DATA_A, &[0x77; 64]).unwrap();
        d.note_write(0x8000_0000 | DATA_A, 64);
        d.block_on_resource(&mut m, TEX_A).unwrap();
        seen.push(format!("level {:?}", m.read_vec(0x8000_0000 | DATA_A, LEN).unwrap()));
        textured_quad(&mut d, &mut m, [32.0, 0.0, 64.0, 32.0], 0xFFFF_FFFF);
        d.end_visibility_test(&mut m, frame).unwrap();
        let fence = d.set_fence(&mut m).unwrap();
        d.block_on_time(&mut m).unwrap();
        let busy = d.resource_busy(&mut m, TEX_A).unwrap();
        let visible = d.get_visibility_test_result(&mut m, frame).unwrap();
        seen.push(format!(
            "fence {fence} completed {} busy {busy} visible {visible:?}",
            d.gpu_completed()
        ));
        d.swap(&mut m, 0).unwrap();
        d.kick_off(&mut m).unwrap();
        seen.push(format!("device {:?}", m.read_vec(dev, 0x2000).unwrap()));
    }
    textured_quad(&mut d, &mut m, [0.0, 32.0, 32.0, 64.0], 0xFFFF_FFFF);
    (d, m, seen)
}

#[test]
fn the_render_thread_makes_the_same_backend_calls() {
    let recorder = || {
        let mut r = RecordingBackend::new();
        r.sample_surfaces = true;
        r
    };
    let (direct, queued) = (recorder(), recorder());
    let (mut a, mut am, seen_a) = scenario(direct);
    let (mut b, mut bm, seen_b) = scenario(ThreadedBackend::new(queued).unwrap());
    assert_eq!(seen_a, seen_b, "what the guest observed");
    let s = a.stats();
    assert!(s.rtt_writebacks > 0 && s.rtt_direct_samples > 0, "{s:?}");
    a.flush_render_targets(&mut am).unwrap();
    b.flush_render_targets(&mut bm).unwrap();
    assert_eq!(a.take_events(), b.take_events());
    assert_eq!(a.stats(), b.stats());
    let calls = b.backend.with_inner(|r| r.calls.clone()).unwrap();
    assert!(calls.len() > 20, "{calls:?}");
    assert_eq!(a.backend.calls, calls);
    assert_eq!(
        am.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
        bm.read_vec(0x8000_0000 | DATA_A, LEN).unwrap()
    );
    assert_eq!(b.backend.misses(), 0);
}

#[test]
fn the_render_thread_draws_the_same_pixels() {
    let (ctx_a, ctx_b) = match (gfx::GfxContext::headless(), gfx::GfxContext::headless()) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let (mut a, mut am, seen_a) = scenario(GfxBackend::new(ctx_a));
    let (mut b, mut bm, seen_b) = scenario(ThreadedBackend::new(GfxBackend::new(ctx_b)).unwrap());
    assert_eq!(seen_a, seen_b, "what the guest observed");
    assert_eq!(a.take_events(), b.take_events());
    assert_eq!(a.stats(), b.stats());
    let (pa, _) = a.backend.read_color(a.back_buffer().unwrap()).unwrap();
    let back = b.back_buffer().unwrap();
    let (pb, _) = b
        .backend
        .with_surface(back, |g, s| g.read_color(s))
        .unwrap();
    assert!(pa.iter().any(|&p| p != 0));
    assert_eq!(pa, pb, "back buffer");
    a.flush_render_targets(&mut am).unwrap();
    b.flush_render_targets(&mut bm).unwrap();
    assert_eq!(
        am.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
        bm.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
        "level A in guest memory"
    );
    let batches = b.backend.with_inner(|g| g.batch_stats()).unwrap();
    assert_eq!(a.backend.batch_stats(), batches);
}

#[test]
fn a_wrong_speculation_stops_the_next_call() {
    let mut inner = RecordingBackend::new();
    inner.no_copies = true;
    let mut t = ThreadedBackend::new(inner).unwrap();
    let s = t
        .create_surface(gfx::D3DFormat::A8R8G8B8, 4, 4, false)
        .unwrap();
    let u = t
        .create_surface(gfx::D3DFormat::A8R8G8B8, 4, 4, false)
        .unwrap();
    // Answered as a backend that copies would; the inner one cannot.
    assert_eq!(t.copy_surface(s, u), Ok(true));
    t.finish().unwrap_err();
    assert_eq!(t.misses(), 1);
    let e = t.clear(s, None, gfx::Clear::NONE).unwrap_err();
    assert!(e.contains("copy_surface returned false"), "{e}");
}

#[test]
fn reads_wait_for_the_work_before_them_and_textures_are_freed() {
    let mut inner = RecordingBackend::new();
    inner.readback_fill = 0x5A;
    let mut t = ThreadedBackend::new(inner).unwrap();
    let s = t
        .create_surface(gfx::D3DFormat::A8R8G8B8, 4, 4, false)
        .unwrap();
    t.clear(s, None, gfx::Clear::NONE).unwrap();
    let bytes = t
        .read_surface(
            s,
            gfx::D3DFormat::A8R8G8B8,
            4,
            4,
            d3d::GuestLayoutKind::Linear { pitch: 16 },
        )
        .unwrap();
    assert_eq!(bytes, vec![0x5A; 64]);
    let calls = t.with_inner(|r| r.calls.len()).unwrap();
    assert_eq!(calls, 3, "create, clear, read in order");
    let ticket = t.report_pixel_count().unwrap().unwrap();
    assert_eq!(t.read_pixel_count(ticket), Ok(0));
    t.read_pixel_count(ticket).unwrap_err();
    for _ in 0..4 {
        t.present(s, 4).unwrap();
    }
    t.finish().unwrap();
    assert_eq!(t.misses(), 0);
}

/// The render thread's timing changes nothing the guest sees: with it
/// lagging by random amounts (three seeds) every observed value, the
/// backend calls and guest memory match the synchronous backend.
#[test]
fn render_thread_timing_changes_nothing_the_guest_sees() {
    let recorder = || {
        let mut r = RecordingBackend::new();
        r.sample_surfaces = true;
        r
    };
    let (mut a, mut am, seen_a) = scenario(recorder());
    a.flush_render_targets(&mut am).unwrap();
    for seed in [1, 0x9e37_79b9, 0xdead_beef] {
        let t = ThreadedBackend::new(recorder()).unwrap().with_jitter(seed);
        let (mut b, mut bm, seen_b) = scenario(t);
        b.flush_render_targets(&mut bm).unwrap();
        assert_eq!(seen_a, seen_b, "seed {seed:#x}");
        assert_eq!(a.stats(), b.stats(), "seed {seed:#x}");
        let calls = b.backend.with_inner(|r| r.calls.clone()).unwrap();
        assert_eq!(a.backend.calls, calls, "seed {seed:#x}");
        assert_eq!(
            am.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
            bm.read_vec(0x8000_0000 | DATA_A, LEN).unwrap()
        );
    }
}
