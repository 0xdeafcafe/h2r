//! Render-thread cost of a frame at 4x with the lazy 1x reference on and
//! off (`ScaledBackend::set_lazy_reference`). Synthetic: each frame clears
//! the back buffer and depth, makes `H2_BENCH_DRAWS` (default 400) draws
//! into them through the shader provider (a texture-factor colour each),
//! and swaps with `D3DSWAP_FINISH`; the "snapshot" case also takes a
//! snapshot of the back buffer mid-frame (a read of the reference's frame,
//! which runs its queue).
//!
//! ```sh
//! cargo bench -p d3d --bench scale_bench
//! ```

mod common;

use std::time::{Duration, Instant};

use common::*;
use d3d::scaled::ScaledBackend;
use d3d::{Device, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::vertex::fvf;

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;

const RS_TEXTUREFACTOR: u32 = 148;
const RS_ZENABLE: u32 = 143;

fn env(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// CPU time this thread has used.
fn thread_cpu() -> Duration {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, tp: *mut Timespec) -> i32;
    }
    #[cfg(target_os = "macos")]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 16;
    #[cfg(not(target_os = "macos"))]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut t = Timespec { sec: 0, nsec: 0 };
    // SAFETY: `t` is a valid out-pointer for the call.
    if unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut t) } != 0 {
        return Duration::ZERO;
    }
    Duration::new(t.sec as u64, t.nsec as u32)
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

fn device(ctx: &GfxContext, lazy: bool) -> (Device<Scaled>, SparseMemory) {
    let mut b =
        ScaledBackend::new(GfxBackend::new(share(ctx)), GfxBackend::new(share(ctx)), 4).unwrap();
    b.set_lazy_reference(lazy);
    let (mut d, mut m) = create(b, UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.texture_state_slot(0, 14), 3).unwrap(); // COLORARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 12), 2).unwrap(); // COLOROP = SELECTARG1
    m.write_u32(l.texture_state_slot(0, 18), 3).unwrap(); // ALPHAARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 13), 2).unwrap(); // ALPHAOP = SELECTARG1
    m.write_u32(l.render_state_slot(RS_ZENABLE), 1).unwrap();
    let mut vb = Vec::new();
    for q in 0..32u32 {
        let (x, y) = ((q % 8) as f32 * 8.0, (q / 8) as f32 * 8.0);
        for (dx, dy) in [(0.0, 0.0), (8.0, 0.0), (0.0, 8.0), (8.0, 8.0)] {
            vb.extend(vertex(x + dx, y + dy, 0xFFFF_FFFF));
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE)
        .unwrap();
    d.set_stream_source(&mut m, 0, VB_HEADER, 20).unwrap();
    (d, m)
}

fn frame(d: &mut Device<Scaled>, m: &mut SparseMemory, f: u32, draws: u32, snapshot: bool) {
    d.clear(m, 0, 0, 0xF3, 0xFF10_2030, 1.0, 0).unwrap();
    for i in 0..draws {
        d.set_render_state(
            m,
            RS_TEXTUREFACTOR,
            0xFF00_0000 | (f * 31 + i * 7) & 0xFF_FFFF,
        )
        .unwrap();
        d.draw_vertices(m, TRIANGLESTRIP, 4 * (i % 32), 4).unwrap();
        if snapshot && i == draws / 2 {
            let bb = d.back_buffer().unwrap();
            let _ = d3d::Backend::snapshot_surface(&mut d.backend, bb, true).unwrap();
        }
    }
    d.swap(m, 4).unwrap();
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn scale_bench() {
    bench();
}

pub fn bench() {
    let ctx = match GfxContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU benchmark: {e}");
            return;
        }
    };
    let (frames, draws) = (env("H2_BENCH_FRAMES", 60), env("H2_BENCH_DRAWS", 400));
    for snapshot in [false, true] {
        for lazy in [false, true] {
            let (mut d, mut m) = device(&ctx, lazy);
            for f in 0..4 {
                frame(&mut d, &mut m, f, draws, snapshot);
            }
            let before = d.backend.stats();
            let (t, cpu) = (Instant::now(), thread_cpu());
            for f in 4..4 + frames {
                frame(&mut d, &mut m, f, draws, snapshot);
            }
            let cpu = (thread_cpu() - cpu) / frames;
            let wall = t.elapsed() / frames;
            let s = d.backend.stats();
            let per = |a: u64, b: u64| (a - b) as f64 / f64::from(frames);
            println!(
                "4x, {draws} draws{:<11} lazy {:<5} {:>7.3} ms/frame (CPU {:>6.3} ms)  reference: deferred {:>6.1}, run late {:>6.1}, dropped {:>6.1} per frame",
                if snapshot { ", snapshot" } else { "" },
                lazy,
                wall.as_secs_f64() * 1e3,
                cpu.as_secs_f64() * 1e3,
                per(s.reference_deferred, before.reference_deferred),
                per(s.reference_replayed, before.reference_replayed),
                per(s.reference_dropped, before.reference_dropped),
            );
            assert_eq!(s.lazy_mismatches, 0);
            assert!(
                d.take_events()
                    .iter()
                    .all(|e| !matches!(e, d3d::Event::DrawSkipped { .. })),
            );
        }
    }
}
