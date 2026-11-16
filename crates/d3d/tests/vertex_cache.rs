//! The vertex cache (`docs/d3d.md`, "Vertex cache") changes where a draw's
//! vertex bytes come from, never what they are: the same scripted frames
//! run on a device with the cache and on one without it, and the backend
//! must receive identical draws (CPU, synchronous and through the render
//! thread) and render identical pixels (GPU) in every frame, while the
//! guest leaves its vertices alone, rewrites a whole page of them, rewrites
//! four bytes, frees a buffer and reuses its memory for other vertices, and
//! rewrites one quad every frame (dynamic geometry).

mod common;

use common::*;
use d3d::backend::{Recorded, ResolvedDraw};
use d3d::{
    Backend, Device, Event, GfxBackend, GuestMemory, RecordingBackend, SparseMemory, Stats,
    ThreadedBackend, UploadPolicy,
};
use gfx::context::GfxContext;
use gfx::vertex::fvf;

/// Vertex data (physical; seen at `0x8000_0000 | pa`): 2.5 pages.
const DATA: u32 = 0x0040_0000;
const VA: u32 = 0x8000_0000 | DATA;
const HEADER: u32 = MISC + 0x1100;
/// The header a "new" buffer over the same memory gets.
const HEADER2: u32 = MISC + 0x1140;
/// Index lists, in the contiguous view the write tracker reports (pages
/// of their own).
const ALL_INNER: u32 = VA + 0x4000;
const SOME_INNER: u32 = VA + 0x5000;
/// An index list outside it: read every time.
const ELSEWHERE: u32 = MISC + 0x9000;
const STRIDE: u32 = 20;
const QUADS: u32 = 128;
const LEN: u32 = QUADS * 4 * STRIDE;
/// A one-quad buffer on its own page, rewritten every frame in the
/// dynamic frames (drawn over tile 9).
const DYN_HEADER: u32 = MISC + 0x1180;
const DYN_DATA: u32 = DATA + 0x3000;
const DYNAMIC_QUAD: u32 = 9;
/// Draws a frame makes.
const DRAWS: usize = 69;

/// Quads 0..64 fill the 8x8 tiles; 64..128 are 4x4 insets of them.
fn quad_bytes(q: u32, shift: f32, salt: u32) -> Vec<u8> {
    let t = q % 64;
    let (x, y) = ((t % 8) as f32 * 8.0, (t / 8) as f32 * 8.0);
    let (x0, y0, x1, y1) = if q < 64 {
        (x, y, x + 8.0, y + 8.0)
    } else {
        (x + 2.0, y + 2.0, x + 6.0, y + 6.0)
    };
    let color = 0xFF00_0000 | (q.wrapping_mul(0x9E37_79B1) ^ salt) & 0x00FF_FFFF;
    let mut v = Vec::new();
    for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
        v.extend(vertex(x + shift, y + shift, color));
    }
    v
}

fn all_quads(shift: f32, salt: u32) -> Vec<u8> {
    (0..QUADS)
        .flat_map(|q| quad_bytes(q, shift, salt))
        .collect()
}

fn header(m: &mut SparseMemory, at: u32) {
    header_at(m, at, DATA);
}

fn header_at(m: &mut SparseMemory, at: u32, data: u32) {
    m.write_u32(at, 1 | 0x0100_0000).unwrap();
    m.write_u32(at + 4, data).unwrap();
    m.write_u32(at + 8, 0).unwrap();
}

/// Triangle-list indices over quads `quads`.
fn index_list(quads: std::ops::Range<u32>) -> Vec<u8> {
    quads
        .flat_map(|q| {
            let b = 4 * q;
            [b, b + 1, b + 2, b + 2, b + 1, b + 3]
        })
        .flat_map(|i| (i as u16).to_le_bytes())
        .collect()
}

/// A guest store the runtime's write barrier reports.
fn store<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, va: u32, bytes: &[u8]) {
    m.write(va, bytes).unwrap();
    d.note_write(va, bytes.len() as u32);
}

fn setup<B: Backend>(backend: B, cache: bool, upload: UploadPolicy) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, upload);
    d.set_vertex_cache(cache.then_some(1 << 20));
    m.map(VA, 0x6000);
    set_states(&mut d, &mut m);
    m.write(VA, &all_quads(0.0, 0)).unwrap();
    header(&mut m, HEADER);
    m.write(0x8000_0000 | DYN_DATA, &quad_bytes(DYNAMIC_QUAD, 0.0, 1))
        .unwrap();
    header_at(&mut m, DYN_HEADER, DYN_DATA);
    m.write(ALL_INNER, &index_list(64..128)).unwrap();
    m.write(SOME_INNER, &index_list(70..73)).unwrap();
    m.write(ELSEWHERE, &index_list(64..66)).unwrap();
    (d, m)
}

/// What the guest changes before frame `f`.
fn mutate<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, f: u32) -> u32 {
    match f {
        // A whole page rewritten (the inner quads on it change colour).
        3 => {
            let page = all_quads(0.0, 0x00AB_CDEF)[0x2000..].to_vec();
            store(d, m, VA + 0x2000, &page);
        }
        // Four bytes: one vertex's colour; and an index list now names
        // other quads.
        4 => {
            store(d, m, VA + 5 * 80 + 16, &0xFF12_3456u32.to_le_bytes());
            store(d, m, SOME_INNER, &index_list(80..83));
        }
        // The buffer is freed and its memory reused by a new one with other
        // vertices (shifted by a pixel, other colours).
        5 => {
            store(d, m, VA, &all_quads(1.0, 0x0055_AA55));
            header(m, HEADER2);
            return HEADER2;
        }
        // Dynamic: the one-quad buffer rewritten every frame.
        6.. => {
            let q = quad_bytes(DYNAMIC_QUAD, 0.0, f * 0x0001_0203);
            store(d, m, 0x8000_0000 | DYN_DATA, &q);
            return HEADER2;
        }
        _ => {}
    }
    HEADER
}

const FRAMES: u32 = 16;

/// One frame's draws, from vertex buffer `vb`.
fn draw_frame<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, vb: u32) {
    d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    d.set_stream_source(m, 0, vb, STRIDE).unwrap();
    for q in 0..64 {
        d.draw_vertices(m, TRIANGLESTRIP, 4 * q, 4).unwrap();
    }
    // The dynamic quad, in two passes.
    d.set_stream_source(m, 0, DYN_HEADER, STRIDE).unwrap();
    for _ in 0..2 {
        d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
    }
    d.set_stream_source(m, 0, vb, STRIDE).unwrap();
    // Indexed: every inset (a window over two pages), then three of them
    // (an overlapping, smaller window).
    d.draw_indexed_vertices(m, 5, 64 * 6, ALL_INNER).unwrap();
    d.draw_indexed_vertices(m, 5, 3 * 6, SOME_INNER).unwrap();
    d.draw_indexed_vertices(m, 5, 2 * 6, ELSEWHERE).unwrap();
}

fn recorded_draws(calls: Vec<Recorded>) -> Vec<ResolvedDraw> {
    calls
        .into_iter()
        .filter_map(|c| match c {
            Recorded::Draw { draw, .. } => Some(*draw),
            _ => None,
        })
        .collect()
}

/// Runs the frames on a recording backend; returns each frame's draws and
/// the device's counters after each frame.
fn record_frames(cache: bool) -> (Vec<Vec<ResolvedDraw>>, Vec<Stats>) {
    let (mut d, mut m) = setup(RecordingBackend::new(), cache, UploadPolicy::Tracked);
    let (mut frames, mut stats) = (Vec::new(), Vec::new());
    for f in 0..FRAMES {
        let vb = mutate(&mut d, &mut m, f);
        draw_frame(&mut d, &mut m, vb);
        frames.push(recorded_draws(std::mem::take(&mut d.backend.calls)));
        d.swap(&mut m, 0).unwrap();
        stats.push(d.stats());
        assert!(d.take_events().is_empty());
    }
    (frames, stats)
}

#[test]
fn the_backend_receives_the_same_draws_with_the_cache_on_and_off() {
    let (off, off_stats) = record_frames(false);
    let (on, on_stats) = record_frames(true);
    assert_eq!(off.len(), on.len());
    for (f, (a, b)) in off.iter().zip(&on).enumerate() {
        assert_eq!(a.len(), DRAWS, "frame {f}");
        assert!(a == b, "frame {f}: draws differ with the cache on");
    }
    // Not vacuous: each change reached the draws.
    for f in [3, 4, 5, 6, 7] {
        assert!(off[f] != off[f - 1], "frame {f} changed nothing");
    }
    assert_eq!(off_stats.last().unwrap().vertex_cache_hits, 0);

    let delta = |s: &[Stats], f: usize, g: fn(&Stats) -> u64| g(&s[f]) - g(&s[f - 1]);
    let copied = |s: &Stats| s.vertex_bytes_copied;
    let fills = |s: &Stats| s.vertex_cache_fills;
    // Off: every frame copies every window.
    let per_frame = delta(&off_stats, 2, copied);
    assert!(per_frame > LEN as u64, "{per_frame}");
    // On: frame 0 copies, frame 1 snapshots, frame 2 copies nothing.
    assert_eq!(delta(&on_stats, 2, copied), 0);
    // Index lists: the two in the contiguous view come from the cache.
    let index_hits = |s: &Stats| s.index_cache_hits;
    assert_eq!(delta(&on_stats, 2, index_hits), 2);
    assert_eq!(off_stats.last().unwrap().index_cache_hits, 0);
    assert_eq!(
        on_stats[2].vertex_cache_hits - on_stats[1].vertex_cache_hits,
        DRAWS as u64
    );
    // A page rewrite re-reads only the windows on that page: the big
    // indexed window (the three small insets are on page 1).
    assert_eq!(delta(&on_stats, 3, copied), 256 * STRIDE as u64);
    // Four bytes: the tracker is page-granular, so every window on page 0
    // (quads 0 to 51) is read again; the big window, written in frame 3
    // and clean since, is snapshotted again; and the rewritten index list
    // reaches a window not seen before.
    assert_eq!(
        delta(&on_stats, 4, copied),
        52 * 80 + 256 * STRIDE as u64 + 3 * 80
    );
    assert_eq!(delta(&on_stats, 4, index_hits), 1);
    // Dynamic frames: the rewritten quad's window is copied for both its
    // draws, and after two rewrites it is no longer snapshotted; nothing
    // else is copied.
    for f in 8..FRAMES as usize {
        assert_eq!(delta(&on_stats, f, copied), 160, "frame {f}");
        assert_eq!(delta(&on_stats, f, fills), 0, "frame {f}");
    }
    let total_fills = on_stats.last().unwrap().vertex_cache_fills;
    assert!(total_fills < 2 * DRAWS as u64 + 10, "{total_fills} fills");
}

#[test]
fn the_render_thread_receives_the_same_draws_with_the_cache_on_and_off() {
    let run = |cache: bool| {
        let backend = ThreadedBackend::new(RecordingBackend::new()).unwrap();
        let (mut d, mut m) = setup(backend, cache, UploadPolicy::Tracked);
        for f in 0..FRAMES {
            let vb = mutate(&mut d, &mut m, f);
            draw_frame(&mut d, &mut m, vb);
            d.swap(&mut m, 0).unwrap();
        }
        let calls = d
            .backend
            .with_inner(|b| std::mem::take(&mut b.calls))
            .unwrap();
        (recorded_draws(calls), d.stats())
    };
    let (off, _) = run(false);
    let (on, stats) = run(true);
    assert_eq!(off.len(), FRAMES as usize * DRAWS);
    assert!(off == on, "draws differ with the cache on");
    assert!(stats.vertex_cache_hits > 0);
}

#[test]
fn verify_names_an_unreported_write_and_draws_the_current_bytes() {
    let (mut d, mut m) = setup(RecordingBackend::new(), true, UploadPolicy::Verify);
    for _ in 0..3 {
        draw_frame(&mut d, &mut m, HEADER);
        d.swap(&mut m, 0).unwrap();
    }
    assert!(d.take_events().is_empty());
    assert!(d.stats().vertex_cache_hits > 0);
    d.backend.calls.clear();
    // A store the barrier missed.
    let q = quad_bytes(3, 0.0, 0x77);
    m.write(VA + 3 * 80, &q).unwrap();
    draw_frame(&mut d, &mut m, HEADER);
    let missed: Vec<_> = d
        .take_events()
        .into_iter()
        .filter(|e| matches!(e, Event::MissedInvalidation { header: HEADER, .. }))
        .collect();
    assert!(!missed.is_empty());
    let draws = recorded_draws(std::mem::take(&mut d.backend.calls));
    assert_eq!(*draws[3].streams[0].data, q[..]);
    // The same for an index list.
    let list = index_list(90..93);
    m.write(SOME_INNER, &list).unwrap();
    draw_frame(&mut d, &mut m, HEADER);
    assert!(d.take_events().iter().any(|e| matches!(
        e,
        Event::MissedInvalidation {
            addr: SOME_INNER,
            ..
        }
    )));
    let draws = recorded_draws(std::mem::take(&mut d.backend.calls));
    let window = &draws[DRAWS - 2];
    assert_eq!(*window.streams[0].data, all_quads(0.0, 0)[90 * 80..93 * 80]);
    assert_eq!(window.indices.as_deref().unwrap()[..6], [0, 1, 2, 2, 1, 3]);
}

#[test]
fn upload_always_never_uses_the_cache() {
    let (mut d, mut m) = setup(RecordingBackend::new(), true, UploadPolicy::Always);
    for _ in 0..3 {
        draw_frame(&mut d, &mut m, HEADER);
        d.swap(&mut m, 0).unwrap();
    }
    assert_eq!(d.stats().vertex_cache_hits, 0);
    assert_eq!(d.vertex_cache_bytes(), 0);
}

#[test]
fn snapshots_stay_within_a_small_budget() {
    let (mut d, mut m) = setup(RecordingBackend::new(), true, UploadPolicy::Tracked);
    d.set_vertex_cache(Some(4096));
    for _ in 0..4 {
        draw_frame(&mut d, &mut m, HEADER);
        d.swap(&mut m, 0).unwrap();
        assert!(d.vertex_cache_bytes() <= 4096);
        d.backend.calls.clear();
    }
}

fn context() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

#[test]
fn the_gpu_renders_the_same_pixels_with_the_cache_on_and_off() {
    if context().is_none() {
        return;
    }
    let run = |cache: bool| {
        let ctx = context().expect("available above");
        let (mut d, mut m) = setup(GfxBackend::new(ctx), cache, UploadPolicy::Tracked);
        let mut frames = Vec::new();
        for f in 0..FRAMES {
            let vb = mutate(&mut d, &mut m, f);
            draw_frame(&mut d, &mut m, vb);
            let back = d.back_buffer().unwrap();
            frames.push(d.backend.read_color(back).unwrap().0);
            d.swap(&mut m, 0).unwrap();
        }
        (frames, d.backend.persistent_vertex_stats())
    };
    let (off, off_gpu) = run(false);
    let (on, on_gpu) = run(true);
    for (f, (a, b)) in off.iter().zip(&on).enumerate() {
        let differ = a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count();
        assert_eq!(differ, 0, "frame {f}: {differ} pixels differ");
    }
    for f in [3, 4, 5, 6, 7] {
        assert!(off[f] != off[f - 1], "frame {f} changed no pixel");
    }
    assert_eq!(off_gpu.hits + off_gpu.uploads, 0);
    // Static windows are uploaded once and bound again after that.
    assert!(on_gpu.hits > 8 * DRAWS as u64, "{on_gpu:?}");
    assert!(on_gpu.uploads < 4 * DRAWS as u64, "{on_gpu:?}");
    // The two cached index lists likewise.
    assert!(on_gpu.index_hits > 8 * 2, "{on_gpu:?}");
    assert!(on_gpu.index_uploads < 4 * 2, "{on_gpu:?}");
}
