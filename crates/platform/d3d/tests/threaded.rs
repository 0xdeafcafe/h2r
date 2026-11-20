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

/// `GetVisibilityTestResult` asked until it answers, as a game that spins
/// on `D3DERR_TESTINCOMPLETE` would. The answer, which must not depend on
/// how many times it was incomplete.
fn poll_visibility<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    index: u32,
) -> d3d::device::VisibilityAnswer {
    loop {
        if let Some(a) = d.get_visibility_test_result(m, index).unwrap() {
            return a;
        }
        std::thread::yield_now();
    }
}

/// The device's stats without the count of incomplete visibility answers,
/// which depends on the render thread's timing.
fn timeless(s: d3d::Stats) -> d3d::Stats {
    d3d::Stats {
        visibility_incomplete: 0,
        ..s
    }
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
        let visible = poll_visibility(&mut d, &mut m, frame);
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
    assert_eq!(timeless(a.stats()), timeless(b.stats()));
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
    // Without and with the persistent pipeline cache, whose compile
    // workers and per-present maintenance move to the render thread with
    // the backend.
    let dir = std::env::temp_dir().join(format!("d3d-threaded-pipelines-{}", std::process::id()));
    for cache in [false, true] {
        let (ctx_a, ctx_b) = match (gfx::GfxContext::headless(), gfx::GfxContext::headless()) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("skipping GPU test: {e}");
                return;
            }
        };
        let make = |ctx, name: &str| {
            let g = GfxBackend::new(ctx);
            if cache { g.with_pipeline_cache(dir.join(name)) } else { g }
        };
        same_pixels(make(ctx_a, "a"), make(ctx_b, "b"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn same_pixels(direct: GfxBackend, queued: GfxBackend) {
    let (mut a, mut am, seen_a) = scenario(direct);
    let (mut b, mut bm, seen_b) = scenario(ThreadedBackend::new(queued).unwrap());
    assert_eq!(seen_a, seen_b, "what the guest observed");
    assert_eq!(a.take_events(), b.take_events());
    assert_eq!(timeless(a.stats()), timeless(b.stats()));
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
    // The same pipelines were built behind the render thread.
    let pipelines = b.backend.with_inner(|g| g.renderer.cache().len()).unwrap();
    assert!(pipelines > 0);
    assert_eq!(a.backend.renderer.cache().len(), pipelines);
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
        assert_eq!(timeless(a.stats()), timeless(b.stats()), "seed {seed:#x}");
        let calls = b.backend.with_inner(|r| r.calls.clone()).unwrap();
        assert_eq!(a.backend.calls, calls, "seed {seed:#x}");
        assert_eq!(
            am.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
            bm.read_vec(0x8000_0000 | DATA_A, LEN).unwrap()
        );
    }
}

/// Untransformed fixed-function vertices (`XYZ`, no `RHW`), which the
/// nv2a translator cannot draw, between two drawn quads.
fn untranslatable<B: Backend>(backend: B) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF10_2030, 1.0, 0).unwrap();
    quad_vb(&mut m, 0.0, 0.0, 32.0, 32.0, 0xFF40_8020);
    bind_quad(&mut d, &mut m);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [x, y, 0.5] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    d.set_vertex_shader(&mut m, fvf::XYZ | fvf::DIFFUSE).unwrap();
    d.set_stream_source(&mut m, 0, VB_HEADER, 16).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    quad_vb(&mut m, 32.0, 32.0, 64.0, 64.0, 0xFF20_4080);
    bind_quad(&mut d, &mut m);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.swap(&mut m, 0).unwrap();
    (d, m)
}

/// The render thread answers an untranslatable draw as the synchronous
/// backend does: skipped, with the same event, and the run goes on.
#[test]
fn an_untranslatable_draw_is_skipped_the_same_way() {
    let (ctx_a, ctx_b) = match (gfx::GfxContext::headless(), gfx::GfxContext::headless()) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let (mut a, _) = untranslatable(GfxBackend::new(ctx_a));
    let (mut b, _) = untranslatable(ThreadedBackend::new(GfxBackend::new(ctx_b)).unwrap());
    let (ea, eb) = (a.take_events(), b.take_events());
    assert!(
        ea.iter().any(|e| matches!(e, d3d::Event::DrawSkipped { reason, .. } if reason.contains("untransformed"))),
        "{ea:?}"
    );
    assert_eq!(ea, eb);
    assert_eq!(timeless(a.stats()), timeless(b.stats()));
    b.backend.finish().unwrap();
    assert_eq!(b.backend.misses(), 0);
    let (pa, _) = a.backend.read_color(a.back_buffer().unwrap()).unwrap();
    let back = b.back_buffer().unwrap();
    let (pb, _) = b.backend.with_surface(back, |g, s| g.read_color(s)).unwrap();
    assert_eq!(pa, pb);
}

/// A fixed sequence of backend calls, long enough to cross the render
/// thread's batch size many times: surfaces, clears, draws (handed over and
/// copied, with textures and without), uploads and texture drops, reads,
/// pixel counts (each draw counts, so a count is only right after every
/// draw before it ran) and presents. Returns every value the caller saw.
fn batched_calls<B: Backend<Surface = u32>>(b: &mut B, draw: &d3d::ResolvedDraw) -> Vec<String> {
    let desc = gfx::TextureDesc::new(gfx::D3DFormat::LinA8R8G8B8, gfx::texture::TextureKind::D2, 4, 4, 1, 1);
    let decoded = gfx::texture::decode_texture(&desc, &[0x11; 64], None, Default::default()).unwrap();
    let mut seen = Vec::new();
    let s = b.create_surface(gfx::D3DFormat::A8R8G8B8, 64, 64, false).unwrap();
    let z = b.create_surface(gfx::D3DFormat::D24S8, 64, 64, true).unwrap();
    let mut live: Vec<B::Texture> = Vec::new();
    b.clear_pixel_count().unwrap();
    for i in 0..(3 * d3d::threaded::BATCH as u32 + 17) {
        match i % 7 {
            0 => live.push(b.upload_texture(&decoded).unwrap()),
            1 if live.len() > 2 => drop(live.remove(0)),
            2 => b.clear(s, Some(z), gfx::Clear::NONE).unwrap(),
            _ => {
                let mut d = draw.clone();
                d.vertex_count = 4 + i % 3;
                let tex = [live.last(), None, None, None];
                let r = if i % 2 == 0 {
                    b.draw_owned(s, Some(z), d, tex).0
                } else {
                    b.draw(s, Some(z), &d, tex)
                };
                seen.push(format!("draw {i} {r:?}"));
            }
        }
        if i % 23 == 0 {
            let t = b.report_pixel_count().unwrap().unwrap();
            seen.push(format!("count {i} {:?}", b.read_pixel_count(t)));
        }
        if i % 41 == 0 {
            let layout = d3d::GuestLayoutKind::Linear { pitch: 256 };
            let px = b.read_surface(s, gfx::D3DFormat::A8R8G8B8, 64, 64, layout).unwrap();
            seen.push(format!("read {i} {}", px.len()));
        }
        if i % 60 == 59 {
            b.present(s, 0).unwrap();
        }
    }
    drop(live);
    b.destroy_surface(z).unwrap();
    seen
}

/// Batching changes neither the order of the backend calls nor what a
/// read sees: the same calls as the synchronous backend, in the same
/// order, and the same answers, with and without a lagging render thread.
#[test]
fn batched_commands_run_in_order_and_reads_see_everything_before_them() {
    let (a, _, _) = scenario(RecordingBackend::new());
    let mut draw = a
        .backend
        .calls
        .iter()
        .find_map(|c| match c {
            d3d::backend::Recorded::Draw { draw, .. } => Some(draw.clone()),
            _ => None,
        })
        .expect("a draw");
    draw.hw.zpass_count_enable = true;
    let mut direct = RecordingBackend::new();
    let seen_a = batched_calls(&mut direct, &draw);
    assert!(seen_a.iter().any(|s| s.starts_with("count") && !s.ends_with("Ok(0)")), "{seen_a:?}");
    for jitter in [None, Some(7)] {
        let mut t = ThreadedBackend::new(RecordingBackend::new()).unwrap();
        if let Some(seed) = jitter {
            t = t.with_jitter(seed);
        }
        let seen_b = batched_calls(&mut t, &draw);
        assert_eq!(seen_a, seen_b, "jitter {jitter:?}");
        let calls = t.with_inner(|r| r.calls.clone()).unwrap();
        assert!(calls.len() > 3 * d3d::threaded::BATCH, "{}", calls.len());
        assert_eq!(direct.calls, calls, "jitter {jitter:?}");
        assert_eq!(t.misses(), 0);
    }
}
