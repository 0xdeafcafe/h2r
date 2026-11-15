//! Synthetic draw throughput through the device and `GfxBackend` on a real
//! GPU: untextured API draws (the built-in shader), textured API draws (the
//! `Nv2aShaders` provider) and inline push-buffer quads, each timed per draw
//! and per frame (draws then a `Swap`). Every resource is synthetic.
//! Inline quads are drained by `MakeRequestedSpace`, which rewinds the push
//! buffer for the next frame.
//!
//! ```sh
//! cargo test --release -p d3d --test draw_bench -- --ignored --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use common::*;
use d3d::abi::Regs;
use d3d::{Device, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::vertex::fvf;

/// Draws per frame (Halo's menu draws about 600).
const DRAWS: u32 = 600;
/// Frames per scenario (after one warm-up frame); `H2_BENCH_FRAMES`
/// overrides it (for profiling).
fn frames() -> u32 {
    std::env::var("H2_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
}

fn swap(d: &mut Device<GfxBackend>, m: &mut SparseMemory) {
    call(d, m, 0x3f9d00, Regs::default(), &[]).unwrap();
}

/// Runs `frame` (which issues `DRAWS` draws) [`frames`] times, then waits for
/// the GPU; prints the mean wall time per frame and per draw.
fn time(
    name: &str,
    d: &mut Device<GfxBackend>,
    m: &mut SparseMemory,
    mut frame: impl FnMut(&mut Device<GfxBackend>, &mut SparseMemory),
) -> Duration {
    frame(d, m);
    swap(d, m);
    let frames = frames();
    let start = Instant::now();
    for _ in 0..frames {
        frame(d, m);
        swap(d, m);
    }
    let _ = d
        .backend
        .ctx
        .device
        .poll(wgpu::PollType::wait_indefinitely());
    let per_frame = start.elapsed() / frames;
    let s = d.backend.batch_stats();
    println!(
        "{name:<28} {:>8.2} ms/frame {:>7.1} us/draw  (passes {}, submits {})",
        per_frame.as_secs_f64() * 1e3,
        per_frame.as_secs_f64() * 1e6 / DRAWS as f64,
        s.passes,
        s.submits,
    );
    assert!(
        d.take_events()
            .iter()
            .all(|e| !matches!(e, d3d::Event::DrawSkipped { .. } | d3d::Event::BadState(_)))
    );
    per_frame
}

fn device() -> Option<(Device<GfxBackend>, SparseMemory)> {
    match GfxContext::headless() {
        Ok(ctx) => Some(create(GfxBackend::new(ctx), UploadPolicy::Tracked)),
        Err(e) => {
            eprintln!("skipping GPU benchmark: {e}");
            None
        }
    }
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn draw_throughput() {
    // Untextured strip quads from a vertex buffer: the built-in shader.
    if let Some((mut d, mut m)) = device() {
        set_states(&mut d, &mut m);
        quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_0000);
        bind_quad(&mut d, &mut m);
        time("DrawVertices, built-in", &mut d, &mut m, |d, m| {
            d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
            for _ in 0..DRAWS {
                d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
            }
        });
    }

    // Textured fixed-function quads: the Nv2aShaders provider.
    if let Some((mut d, mut m)) = device() {
        set_states(&mut d, &mut m);
        texture(&mut m, 0x80);
        d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
        let mut vb = Vec::new();
        for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            for f in [8.0 * x, 8.0 * y, 0.5, 1.0] {
                vb.extend_from_slice(&f.to_le_bytes());
            }
            vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            for f in [16.0 * x, 4.0 * y] {
                vb.extend_from_slice(&f.to_le_bytes());
            }
        }
        m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
        m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
        m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
        m.write_u32(VB_HEADER + 8, 0).unwrap();
        d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
            .unwrap();
        d.set_stream_source(&mut m, 0, VB_HEADER, 28).unwrap();
        time("DrawVertices, textured", &mut d, &mut m, |d, m| {
            d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
            for _ in 0..DRAWS {
                d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
            }
        });
    }

    // Inline push-buffer quads (BEGIN_END / VERTEX4F), drained at Swap.
    if let Some((mut d, mut m)) = device() {
        set_states(&mut d, &mut m);
        d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE)
            .unwrap();
        time("inline push-buffer quads", &mut d, &mut m, |d, m| {
            d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
            for i in 0..DRAWS {
                let x = (i % 8) as f32 * 8.0;
                inline_quad(m, x, 0.0, x + 8.0, 8.0, [0, 255, 0, 255]);
            }
            // MakeRequestedSpace: run what was pushed and rewind to the start.
            let regs = Regs {
                eax: 0x100,
                ..Regs::default()
            };
            call(d, m, 0x3faae0, regs, &[0x4000]).unwrap();
        });
    }
}
