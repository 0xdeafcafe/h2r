//! Per-draw CPU cost through the device and `GfxBackend` with the state
//! churn of a level frame: every draw is textured (the `Nv2aShaders`
//! provider), changes its pixel uniforms (the texture factor), alternates
//! between two textures, and every few draws turn alpha test (a program
//! that can `discard`) or alpha blending on and off. Every resource is
//! synthetic. Each scenario prints the time per draw split into recording
//! (the device's draw calls) and the flush and submit at `Swap`, and the
//! render passes and submissions per frame.
//!
//! ```sh
//! cargo bench -p d3d --bench draw_churn
//! ```
//!
//! `H2_BENCH_FRAMES` sets the frame count (default 30), `H2_BENCH_DRAWS`
//! the draws per frame (default 600, or 1500 for city cases), and
//! `H2_BENCH_ONLY` runs only the scenarios whose name contains it.
//! `H2_BENCH_OUTPUT` writes final raw color bytes into an existing directory
//! after the timed window, for offline A/B comparison.

mod common;

use std::time::{Duration, Instant};

use common::*;
use d3d::abi::Regs;
use d3d::{Device, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::vertex::fvf;

const RS_ALPHATESTENABLE: u32 = 60;
const RS_ALPHAREF: u32 = 61;
const RS_ALPHAFUNC: u32 = 58;
const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;
const RS_TEXTUREFACTOR: u32 = 148;
/// A second 16x4 texture, after the first.
const TEX2_HEADER: u32 = TEX_HEADER + 0x40;
const TEX2_DATA: u32 = TEX_DATA + 0x800;

fn env(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn swap(d: &mut Device<GfxBackend>, m: &mut SparseMemory) {
    call(d, m, 0x3f9d00, Regs::default(), &[]).unwrap();
}

/// Draws per frame and how often (in draws) alpha test and blending flip.
#[derive(Clone, Copy)]
struct Churn {
    draws: u32,
    /// Consecutive draws using the same material.
    material_run: u32,
    alpha_test_every: Option<u32>,
    blend_every: Option<u32>,
    /// Every n-th draw writes only RGB (`COLORWRITEENABLE`).
    masked_every: Option<u32>,
}

/// Merged passes under the loose rule (the default) or the exact rule
/// (`set_merge_masks(false)`), or one render pass per draw.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Loose,
    Exact,
    PerDraw,
}

fn device(mode: Mode) -> Option<(Device<GfxBackend>, SparseMemory)> {
    match GfxContext::headless() {
        Ok(ctx) => {
            let mut b = GfxBackend::new(ctx);
            b.set_merge_passes(mode != Mode::PerDraw);
            b.set_merge_masks(mode != Mode::Exact);
            Some(create(b, UploadPolicy::Tracked))
        }
        Err(e) => {
            eprintln!("skipping GPU benchmark: {e}");
            None
        }
    }
}

fn setup(d: &mut Device<GfxBackend>, m: &mut SparseMemory) {
    set_states(d, m);
    texture(m, 0x80);
    // The second texture: the first's header over other bytes.
    for o in (0..20).step_by(4) {
        let v = m.read_u32(TEX_HEADER + o).unwrap();
        m.write_u32(TEX2_HEADER + o, v).unwrap();
    }
    m.write_u32(TEX2_HEADER + 4, TEX2_DATA).unwrap();
    m.write(0x8000_0000 | TEX2_DATA, &[0x40; 256]).unwrap();
    // Stage 0: colour = texture * texture factor, alpha = texture.
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.texture_state_slot(0, 12), 4).unwrap(); // COLOROP = MODULATE
    m.write_u32(l.texture_state_slot(0, 14), 2).unwrap(); // COLORARG1 = TEXTURE
    m.write_u32(l.texture_state_slot(0, 15), 3).unwrap(); // COLORARG2 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 13), 2).unwrap(); // ALPHAOP = SELECTARG1
    m.write_u32(l.texture_state_slot(0, 18), 2).unwrap(); // ALPHAARG1 = TEXTURE
    let mut vb = Vec::new();
    // A synthetic 8x8 skyline grid, covering the target rather than
    // repeatedly overwriting the same few pixels.
    for mesh in 0..64 {
        for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            for f in [
                8.0 * (x + (mesh % 8) as f32),
                8.0 * (y + (mesh / 8) as f32),
                0.5,
                1.0,
            ] {
                vb.extend_from_slice(&f.to_le_bytes());
            }
            vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            for f in [16.0 * x, 4.0 * y] {
                vb.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
    m.map((0x8000_0000 | VB_DATA) + 0x1000, 0x1000);
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
    d.set_render_state(m, RS_ALPHAREF, 0x20).unwrap();
    d.set_render_state(m, RS_ALPHAFUNC, 0x204).unwrap(); // GREATER
    d.set_render_state(m, RS_SRCBLEND, 0x302).unwrap(); // SRCALPHA
    d.set_render_state(m, RS_DESTBLEND, 0x303).unwrap(); // INVSRCALPHA
}

fn frame(d: &mut Device<GfxBackend>, m: &mut SparseMemory, c: Churn) {
    d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    for i in 0..c.draws {
        let material = i / c.material_run;
        let tex = if material.is_multiple_of(2) {
            TEX_HEADER
        } else {
            TEX2_HEADER
        };
        d.set_texture(m, 0, tex).unwrap();
        d.set_render_state(
            m,
            RS_TEXTUREFACTOR,
            0xFF00_0000 | (material & 0xFF) << 8 | 0x40,
        )
        .unwrap();
        if let Some(n) = c.alpha_test_every {
            d.set_render_state(m, RS_ALPHATESTENABLE, u32::from(material.is_multiple_of(n)))
                .unwrap();
        }
        if let Some(n) = c.blend_every {
            d.set_render_state(m, RS_ALPHABLENDENABLE, u32::from(material % n == 1))
                .unwrap();
        }
        if let Some(n) = c.masked_every {
            let mask = if i % n == 2 { 0x0001_0101 } else { 0x0101_0101 };
            m.write_u32(d3d::GuestLayout::HALO2.render_state_slot(67), mask)
                .unwrap();
        }
        d.draw_vertices(
            m,
            TRIANGLESTRIP,
            if c.material_run > 1 { 4 * (i % 64) } else { 0 },
            4,
        )
        .unwrap();
    }
}

/// Runs `c` for the configured frames after two warm-up frames and prints
/// the record and swap (flush, encode, submit) time per draw.
fn run(name: &str, mode: Mode, c: Churn) {
    if std::env::var("H2_BENCH_ONLY").is_ok_and(|only| !name.contains(only.as_str())) {
        return;
    }
    let Some((mut d, mut m)) = device(mode) else {
        return;
    };
    setup(&mut d, &mut m);
    for _ in 0..2 {
        frame(&mut d, &mut m, c);
        swap(&mut d, &mut m);
    }
    let _ = d
        .backend
        .ctx
        .device
        .poll(wgpu::PollType::wait_indefinitely());
    let before = d.backend.batch_stats();
    let frames = env("H2_BENCH_FRAMES", 30);
    let (mut record, mut submit) = (Duration::ZERO, Duration::ZERO);
    let start = Instant::now();
    for _ in 0..frames {
        let t = Instant::now();
        frame(&mut d, &mut m, c);
        let r = t.elapsed();
        swap(&mut d, &mut m);
        record += r;
        submit += t.elapsed() - r;
    }
    let _ = d
        .backend
        .ctx
        .device
        .poll(wgpu::PollType::wait_indefinitely());
    let wall = start.elapsed();
    let s = d.backend.batch_stats();
    if let Ok(dir) = std::env::var("H2_BENCH_OUTPUT") {
        let (pixels, _) = d
            .backend
            .read_color(d.backend.last_presented().unwrap())
            .unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{name}.rgba")),
            pixels,
        )
        .unwrap();
    }
    let per = |t: Duration| t.as_secs_f64() * 1e6 / f64::from(frames * c.draws);
    println!(
        "{name:<34} {:>6.1} us/draw (record {:>5.1}, swap {:>5.1}) {:>6.2} ms/frame  render passes {:>4}/frame, submits {}/frame",
        per(wall),
        per(record),
        per(submit),
        wall.as_secs_f64() * 1e3 / f64::from(frames),
        (s.render_passes - before.render_passes) / u64::from(frames),
        (s.submits - before.submits) / u64::from(frames),
    );
    assert!(
        d.take_events()
            .iter()
            .all(|e| !matches!(e, d3d::Event::DrawSkipped { .. } | d3d::Event::BadState(_)))
    );
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn draw_churn() {
    bench();
}

pub fn bench() {
    let draws = env("H2_BENCH_DRAWS", 600);
    let level = Churn {
        draws,
        material_run: 1,
        alpha_test_every: Some(3),
        blend_every: Some(5),
        masked_every: None,
    };
    let plain = Churn {
        alpha_test_every: None,
        blend_every: None,
        ..level
    };
    let masked = Churn {
        masked_every: Some(20),
        ..level
    };
    let city = Churn {
        draws: env("H2_BENCH_DRAWS", 1500),
        material_run: 6,
        ..level
    };
    run("loose, city 1500", Mode::Loose, city);
    run(
        "loose, city 1500 + masked",
        Mode::Loose,
        Churn {
            masked_every: Some(20),
            ..city
        },
    );
    run("loose, uniforms + textures", Mode::Loose, plain);
    run("loose, level churn", Mode::Loose, level);
    run("loose, level churn + masked", Mode::Loose, masked);
    run("exact, level churn", Mode::Exact, level);
    run("exact, level churn + masked", Mode::Exact, masked);
    run("pass per draw, level churn", Mode::PerDraw, level);
}
