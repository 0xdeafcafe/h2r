//! Overlapping render-to-texture levels through the alias mirror
//! (`Device::set_alias_mirror`) against the
//! synchronous path that writes every overlapping level back to guest
//! memory: the same back buffer and the same guest memory, byte for byte,
//! at every point the guest reads it.
//!
//! One synthetic pool holds five levels: two linear `A8R8G8B8` levels with
//! different pitches and a partial overlap, a swizzled one over the same
//! bytes, a linear `X8R8G8B8` one with pitch padding, and a linear `D24S8`
//! depth level, as Halo's 160x120, 320x240 and 512x512 shadow-map levels
//! share one allocation. The scenario draws into each in turn, samples the
//! others (in place and through guest memory), stores into the pool from
//! the guest (whole words and part of a word, while a level is bound and
//! after it was moved into the mirror), waits on a texture (the guest
//! reads), and finally flushes. Skipped without an adapter.

mod common;

use common::*;
use d3d::resource::SurfaceHeader;
use d3d::{
    Backend, Device, Event, GfxBackend, GuestLayout, GuestMemory, SparseMemory, ThreadedBackend,
    UploadPolicy,
};
use gfx::vertex::fvf;
use gfx::{D3DFormat, GfxContext};

const L: GuestLayout = GuestLayout::HALO2;
const RECTS: u32 = MISC + 0x7100;
const POOL: u32 = 0x0040_0000;
const POOL_LEN: u32 = 0x2000;

const RS_ZWRITEENABLE: u32 = 64;
const RS_ZENABLE: u32 = 143;
const RS_STENCILENABLE: u32 = 52;

/// A level of the pool: texture header, level header, format, size,
/// pitch (`None`: swizzled), byte offset in the pool.
#[derive(Clone, Copy, Debug)]
struct Level {
    tex: u32,
    level: u32,
    format: D3DFormat,
    w: u32,
    h: u32,
    pitch: Option<u32>,
    at: u32,
}

const A: Level = Level {
    tex: MISC + 0x4000,
    level: MISC + 0x4040,
    format: D3DFormat::LinA8R8G8B8,
    w: 32,
    h: 32,
    pitch: Some(128),
    at: 0,
};
const B: Level = Level {
    tex: MISC + 0x4100,
    level: MISC + 0x4140,
    format: D3DFormat::LinA8R8G8B8,
    w: 16,
    h: 16,
    pitch: Some(64),
    at: 640,
};
const C: Level = Level {
    tex: MISC + 0x4200,
    level: MISC + 0x4240,
    format: D3DFormat::A8R8G8B8,
    w: 32,
    h: 32,
    pitch: None,
    at: 0,
};
fn words(l: Level) -> (u32, u32) {
    let fmt = (2 << 4) | ((l.format.raw() & 0xFF) << 8) | (1 << 16);
    match l.pitch {
        Some(p) => (fmt, (l.w - 1) | ((l.h - 1) << 12) | ((p / 64 - 1) << 24)),
        None => (
            fmt | (l.w.trailing_zeros() << 20) | (l.h.trailing_zeros() << 24),
            0,
        ),
    }
}

fn define(m: &mut SparseMemory, l: Level) {
    let (fmt, size) = words(l);
    let data = POOL + l.at;
    for (i, w) in [1 | (4 << 16) | 0x0100_0000, data, 0, fmt, size]
        .iter()
        .enumerate()
    {
        m.write_u32(l.tex + 4 * i as u32, *w).unwrap();
    }
    let mut h = SurfaceHeader::linear(l.format, l.w, l.h, l.pitch.unwrap_or(64).max(64), data);
    h.pixels.format = fmt;
    h.pixels.size = size;
    h.parent = l.tex;
    h.write(m, l.level).unwrap();
}

/// A quad over `rect` with per-corner diffuse colours, textured with the
/// whole of `tex` (`Some((w, h, linear))`) or untextured, at depth `z`.
fn quad<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    rect: (f32, f32, f32, f32),
    tex: Option<(u32, u32, bool)>,
    colors: [u32; 4],
    z: f32,
) {
    let (x0, y0, x1, y1) = rect;
    let mut vb = Vec::new();
    for (i, (u, v)) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        for f in [x0 + (x1 - x0) * u, y0 + (y1 - y0) * v, z, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&colors[i].to_le_bytes());
        if let Some((w, h, linear)) = tex {
            let (su, sv) = if linear {
                (w as f32, h as f32)
            } else {
                (1.0, 1.0)
            };
            for f in [su * u, sv * v] {
                vb.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    let (shader, stride) = if tex.is_some() {
        (fvf::XYZRHW | fvf::DIFFUSE | 0x100, 28)
    } else {
        (fvf::XYZRHW | fvf::DIFFUSE, 20)
    };
    d.set_vertex_shader(m, shader).unwrap();
    d.set_stream_source(m, 0, VB_HEADER, stride).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

fn sampled<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    rect: (f32, f32, f32, f32),
    l: Level,
    tint: u32,
) {
    d.set_texture(m, 0, l.tex).unwrap();
    quad(
        d,
        m,
        rect,
        Some((l.w, l.h, l.pitch.is_some())),
        [tint, 0xFFFF_FFFF, tint, 0xFF80_8080],
        0.5,
    );
    d.set_texture(m, 0, 0).unwrap();
}

fn clear_rect<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    rect: [u32; 4],
    flags: u32,
    color: u32,
    z: f32,
    s: u32,
) {
    m.write(RECTS, &rect.map(u32::to_le_bytes).concat())
        .unwrap();
    d.clear(m, 1, RECTS, flags, color, z, s).unwrap();
}

fn back<B: Backend>(d: &mut Device<B>) -> Vec<u8> {
    let b = d.back_buffer().unwrap();
    d.backend
        .read_surface(
            b,
            D3DFormat::LinA8R8G8B8,
            W,
            H,
            d3d::backend::GuestLayoutKind::Linear { pitch: W * 4 },
        )
        .unwrap()
}

fn gfx() -> Option<GfxBackend> {
    match GfxContext::headless() {
        Ok(ctx) => {
            if !ctx.caps.depth32_stencil8 {
                eprintln!("skipping GPU test: no DEPTH32FLOAT_STENCIL8");
                return None;
            }
            Some(GfxBackend::new(ctx))
        }
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

/// Levels that only the GPU reads never come back to the CPU: the
/// Heretic's pattern, a pool of colour levels bound in turn, each
/// sampling the one before, frame after frame.
fn pool_frames<B: Backend>(backend: B, mirror: bool, frames: u32) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_alias_mirror(mirror);
    set_states(&mut d, &mut m);
    m.map(0x8000_0000 | POOL, POOL_LEN);
    for l in [A, B, C] {
        define(&mut m, l);
    }
    let back_header = m
        .read_u32(L.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    for f in 0..frames {
        d.set_render_target(&mut m, A.level, 0).unwrap();
        quad(
            &mut d,
            &mut m,
            (0.0, 0.0, 32.0, 32.0),
            None,
            [
                0xFF00_0000 | (f * 0x10101),
                0xFFFF_0000,
                0xFF00_FF00,
                0xFF00_00FF,
            ],
            0.5,
        );
        d.set_render_target(&mut m, B.level, 0).unwrap();
        sampled(&mut d, &mut m, (0.0, 0.0, 16.0, 16.0), A, 0xFFFF_FFFF);
        d.set_render_target(&mut m, C.level, 0).unwrap();
        sampled(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), B, 0xFFC0_C0C0);
        d.set_render_target(&mut m, back_header, 0).unwrap();
        sampled(&mut d, &mut m, (0.0, 0.0, 64.0, 64.0), C, 0xFFFF_FFFF);
        call(&mut d, &mut m, 0x3f9d00, d3d::abi::Regs::default(), &[]).unwrap();
    }
    (d, m)
}

/// Wall time per frame of [`pool_frames`]: write-backs against the mirror,
/// on the device's thread and through the render thread.
///
/// ```sh
/// cargo test --release -p d3d --test alias_mirror -- --ignored --nocapture
/// ```
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn bench_overlapping_pool_frames() {
    let frames: u32 = std::env::var("H2_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    fn time<B: Backend>(name: &str, backend: B, mirror: bool, frames: u32) {
        let start = std::time::Instant::now();
        let (mut d, _m) = pool_frames(backend, mirror, frames);
        let _ = back(&mut d);
        let ms = start.elapsed().as_secs_f64() * 1e3 / frames as f64;
        let s = d.stats();
        println!(
            "{name:<32} {ms:>7.3} ms/frame  (overlap write-backs {}, mirror stores {})",
            s.writeback_causes.overlap, s.mirror_stores
        );
    }
    let Some(_) = gfx() else { return };
    for round in 0..2 {
        println!("round {round}, {frames} frames");
        time("sync, write-backs", gfx().unwrap(), false, frames);
        time("sync, mirror", gfx().unwrap(), true, frames);
        time(
            "render thread, write-backs",
            ThreadedBackend::new(gfx().unwrap()).unwrap(),
            false,
            frames,
        );
        time(
            "render thread, mirror",
            ThreadedBackend::new(gfx().unwrap()).unwrap(),
            true,
            frames,
        );
    }
}

/// The Heretic's sizes: a 320x240 and two 160x120 `LIN_A8R8G8B8` levels
/// over one 1 MiB pool (the 512x512 shadow map's), each frame 150 quads
/// into the back buffer, then the levels in turn, each sampling the one
/// before, then the back buffer sampling the last. Write-backs against the
/// mirror, `H2_BENCH_FRAMES` frames (default 100).
///
/// ```sh
/// cargo test --release -p d3d --test alias_mirror -- --ignored --nocapture
/// ```
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn bench_heretic_sized_pool() {
    const BIG: u32 = 0x0060_0000;
    let lv = |n: u32, w: u32, h: u32, at: u32| Level {
        tex: MISC + 0x5000 + n * 0x100,
        level: MISC + 0x5040 + n * 0x100,
        format: D3DFormat::LinA8R8G8B8,
        w,
        h,
        pitch: Some(w * 4),
        at,
    };
    let levels = [
        lv(0, 320, 240, 0),
        lv(1, 160, 120, 0),
        lv(2, 160, 120, 160 * 120 * 4),
    ];
    let frames: u32 = std::env::var("H2_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    fn run<B: Backend>(name: &str, backend: B, mirror: bool, frames: u32, levels: [Level; 3]) {
        let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
        d.set_alias_mirror(mirror);
        set_states(&mut d, &mut m);
        m.map(0x8000_0000 | BIG, 1 << 20);
        for l in levels {
            let (fmt, size) = words(l);
            let data = BIG + l.at;
            for (i, w) in [1 | (4 << 16) | 0x0100_0000, data, 0, fmt, size]
                .iter()
                .enumerate()
            {
                m.write_u32(l.tex + 4 * i as u32, *w).unwrap();
            }
            let mut h = SurfaceHeader::linear(l.format, l.w, l.h, l.w * 4, data);
            h.parent = l.tex;
            h.write(&mut m, l.level).unwrap();
        }
        let back_header = m
            .read_u32(L.device + d3d::layout::device_field::RENDER_TARGET)
            .unwrap();
        let start = std::time::Instant::now();
        for f in 0..frames {
            d.set_render_target(&mut m, back_header, 0).unwrap();
            for i in 0..150u32 {
                let (x, y) = ((i * 7 % 56) as f32, (i * 11 % 56) as f32);
                quad(
                    &mut d,
                    &mut m,
                    (x, y, x + 8.0, y + 8.0),
                    None,
                    [0xFF00_0000 | (i * 0x10203) | f; 4],
                    0.5,
                );
            }
            let mut prev: Option<Level> = None;
            for l in levels {
                d.set_render_target(&mut m, l.level, 0).unwrap();
                match prev {
                    Some(p) => sampled(
                        &mut d,
                        &mut m,
                        (0.0, 0.0, l.w as f32, l.h as f32),
                        p,
                        0xFFFF_FFFF,
                    ),
                    None => quad(
                        &mut d,
                        &mut m,
                        (0.0, 0.0, l.w as f32, l.h as f32),
                        None,
                        [0xFF20_4080 | f; 4],
                        0.5,
                    ),
                }
                prev = Some(l);
            }
            d.set_render_target(&mut m, back_header, 0).unwrap();
            sampled(
                &mut d,
                &mut m,
                (0.0, 0.0, 64.0, 64.0),
                prev.unwrap(),
                0xFFFF_FFFF,
            );
            call(&mut d, &mut m, 0x3f9d00, d3d::abi::Regs::default(), &[]).unwrap();
        }
        let _ = back(&mut d);
        let ms = start.elapsed().as_secs_f64() * 1e3 / frames as f64;
        let s = d.stats();
        println!(
            "{name:<32} {ms:>7.3} ms/frame  (overlap write-backs {}, mirror stores {}, flushes {})",
            s.writeback_causes.overlap, s.mirror_stores, s.mirror_flushes
        );
    }
    let Some(_) = gfx() else { return };
    for round in 0..2 {
        println!("round {round}, {frames} frames");
        run("sync, write-backs", gfx().unwrap(), false, frames, levels);
        run("sync, mirror", gfx().unwrap(), true, frames, levels);
        run(
            "render thread, write-backs",
            ThreadedBackend::new(gfx().unwrap()).unwrap(),
            false,
            frames,
            levels,
        );
        run(
            "render thread, mirror",
            ThreadedBackend::new(gfx().unwrap()).unwrap(),
            true,
            frames,
            levels,
        );
    }
}

/// How [`shadow_frames`] moves bytes between the levels over the shadow
/// map: write-backs, the mirror with the depth level filled through guest
/// memory (a read-back each time it is bound), or the mirror throughout.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Path {
    WriteBacks,
    MirrorDepthThroughMemory,
    Mirror,
}

/// A shadow-map pool: depth level `depth` (bound with the free-standing
/// colour target at `ct`) and colour levels `colors` over its bytes, as
/// Halo's 512x512 `LIN_D24S8` shadow map and the 320x240 and 160x120
/// levels share one allocation. Each frame: the shadow pass (part of the
/// depth cleared, a quad depth-tested against the rest, which holds what
/// the colour levels left there, then depth written), the colour levels in
/// turn (part drawn, each sampling the one before), the back buffer
/// sampling the last, `Swap`. Frame 1 also stores into the pool from the
/// guest. Returns the back buffer after each frame and the time spent in
/// the `SetRenderTarget` calls that bind the depth level.
fn shadow_frames<B: Backend>(
    backend: B,
    path: Path,
    upload: UploadPolicy,
    frames: u32,
    base: u32,
    depth: Level,
    colors: &[Level],
    ct: (u32, u32, u32, u32),
) -> (Device<B>, SparseMemory, Vec<Vec<u8>>, std::time::Duration) {
    let (mut d, mut m) = create(backend, upload);
    d.set_alias_mirror(path != Path::WriteBacks);
    d.set_alias_mirror_depth(path == Path::Mirror);
    set_states(&mut d, &mut m);
    let pool_len = depth.pitch.unwrap() * depth.h;
    m.map(0x8000_0000 | base, pool_len);
    let fill: Vec<u8> = (0..pool_len).map(|i| (i * 13 + (i >> 9)) as u8).collect();
    m.write(0x8000_0000 | base, &fill).unwrap();
    let (ct_header, ct_data, ct_w, ct_h) = ct;
    m.map(0x8000_0000 | ct_data, ct_w * ct_h * 4);
    SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, ct_w, ct_h, ct_w * 4, ct_data)
        .write(&mut m, ct_header)
        .unwrap();
    let rebased = |l: Level| Level {
        at: l.at + base - POOL,
        ..l
    };
    define(&mut m, rebased(depth));
    for l in colors {
        define(&mut m, rebased(*l));
    }
    let back_header = m
        .read_u32(L.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    let mut shown = Vec::new();
    let mut binding = std::time::Duration::ZERO;
    for f in 0..frames {
        let start = std::time::Instant::now();
        d.set_render_target(&mut m, ct_header, depth.level).unwrap();
        binding += start.elapsed();
        m.write_u32(L.render_state_slot(RS_ZENABLE), 1).unwrap();
        m.write_u32(L.render_state_slot(RS_ZWRITEENABLE), 1)
            .unwrap();
        m.write_u32(L.render_state_slot(RS_STENCILENABLE), 0)
            .unwrap();
        clear_rect(
            &mut d,
            &mut m,
            [0, 0, depth.w, depth.h / 4],
            0x03,
            0,
            0.25 + f as f32 / 64.0,
            0x40 + f,
        );
        // LESSEQUAL: the depth the level was filled with decides.
        m.write_u32(L.render_state_slot(RS_ZFUNC), 0x203).unwrap();
        quad(
            &mut d,
            &mut m,
            (0.0, 0.0, depth.w as f32, depth.h as f32),
            None,
            [0xFFFF_FFFF; 4],
            0.5,
        );
        m.write_u32(L.render_state_slot(RS_ZFUNC), 0x207).unwrap();
        m.write_u32(L.render_state_slot(RS_ZENABLE), 0).unwrap();
        m.write_u32(L.render_state_slot(RS_ZWRITEENABLE), 0)
            .unwrap();
        let mut prev: Option<Level> = None;
        for (i, l) in colors.iter().map(|l| rebased(*l)).enumerate() {
            d.set_render_target(&mut m, l.level, 0).unwrap();
            // The left half: the rest keeps what the pool held.
            let rect = (0.0, 0.0, (l.w / 2) as f32, l.h as f32);
            match prev {
                Some(p) => sampled(&mut d, &mut m, rect, p, 0xFFFF_FFFF),
                // Alpha is the depth's top byte where the shadow map
                // reads these bytes: some below the quad's 0.5, some above.
                None => quad(
                    &mut d,
                    &mut m,
                    rect,
                    None,
                    [
                        0x3020_4080 | f | (i as u32) << 8,
                        0xFF20_4080,
                        0x5080_2010,
                        0xC0FF_FFFF,
                    ],
                    0.5,
                ),
            }
            prev = Some(l);
        }
        d.set_render_target(&mut m, back_header, 0).unwrap();
        sampled(
            &mut d,
            &mut m,
            (0.0, 0.0, 64.0, 64.0),
            prev.unwrap(),
            0xFFFF_FFFF,
        );
        if f == 1 {
            // A guest store into the colour levels' bytes, whole words and
            // part of one, between frames.
            let at = colors[0].at + 4 * colors[0].w + 8;
            m.write(0x8000_0000 | (base + at), &[0x5A; 12]).unwrap();
            d.note_write(0x8000_0000 | (base + at), 12);
            m.write(0x8000_0000 | (base + at + 64), &[0x11]).unwrap();
            d.note_write(0x8000_0000 | (base + at + 64), 1);
        }
        if frames <= 8 {
            shown.push(back(&mut d));
        }
        call(&mut d, &mut m, 0x3f9d00, d3d::abi::Regs::default(), &[]).unwrap();
    }
    // A last shadow pass, so the pool's final bytes are the depth level's
    // (filled from what the colour levels left).
    d.set_render_target(&mut m, ct_header, depth.level).unwrap();
    m.write_u32(L.render_state_slot(RS_ZENABLE), 1).unwrap();
    m.write_u32(L.render_state_slot(RS_ZWRITEENABLE), 1)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_ZFUNC), 0x203).unwrap();
    quad(
        &mut d,
        &mut m,
        (0.0, 0.0, depth.w as f32, depth.h as f32),
        None,
        [0xFFFF_FFFF; 4],
        0.5,
    );
    m.write_u32(L.render_state_slot(RS_ZFUNC), 0x207).unwrap();
    m.write_u32(L.render_state_slot(RS_ZENABLE), 0).unwrap();
    m.write_u32(L.render_state_slot(RS_ZWRITEENABLE), 0)
        .unwrap();
    d.set_render_target(&mut m, back_header, 0).unwrap();
    let events = d.take_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_))),
        "{events:?}"
    );
    (d, m, shown, binding)
}

/// Per-switch cost of binding the shadow map at the Heretic's sizes (a
/// 512x512 `LIN_D24S8` level under a 320x240 and two 160x120
/// `LIN_A8R8G8B8` levels), with the depth level filled through guest
/// memory (a read-back and an upload each time, as on e9be08f) against
/// from the mirror on the GPU. `H2_BENCH_FRAMES` frames (default 100).
///
/// ```sh
/// cargo test --release -p d3d --test alias_mirror bench_shadow_map_binding -- --ignored --nocapture
/// ```
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn bench_shadow_map_binding() {
    const BIG: u32 = 0x0080_0000;
    const CT: u32 = MISC + 0x7A00;
    const CT_DATA: u32 = 0x00C0_0000;
    let depth = Level {
        tex: MISC + 0x6800,
        level: MISC + 0x6840,
        format: D3DFormat::LinD24S8,
        w: 512,
        h: 512,
        pitch: Some(2048),
        at: 0,
    };
    let lv = |n: u32, w: u32, h: u32, at: u32| Level {
        tex: MISC + 0x6900 + n * 0x100,
        level: MISC + 0x6940 + n * 0x100,
        format: D3DFormat::LinA8R8G8B8,
        w,
        h,
        pitch: Some(w * 4),
        at,
    };
    let colors = [
        lv(0, 320, 240, 0),
        lv(1, 160, 120, 0),
        lv(2, 160, 120, 160 * 120 * 4),
    ];
    let frames: u32 = std::env::var("H2_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    fn run<B: Backend>(name: &str, b: B, path: Path, frames: u32, depth: Level, colors: &[Level]) {
        // `H2_BENCH_ONLY=render` and the like: only matching runs.
        if std::env::var("H2_BENCH_ONLY").is_ok_and(|only| !name.contains(&only)) {
            return;
        }
        let start = std::time::Instant::now();
        let (mut d, _m, _, binding) = shadow_frames(
            b,
            path,
            UploadPolicy::Tracked,
            frames,
            BIG,
            depth,
            colors,
            (CT, CT_DATA, 512, 512),
        );
        let _ = back(&mut d);
        let total = start.elapsed().as_secs_f64() * 1e3 / frames as f64;
        let per = binding.as_secs_f64() * 1e3 / frames as f64;
        let s = d.stats();
        println!(
            "{name:<40} {per:>7.3} ms/switch {total:>7.3} ms/frame  (flushes {}, loads {}, overlap write-backs {})",
            s.mirror_flushes, s.mirror_loads, s.writeback_causes.overlap
        );
    }
    let Some(_) = gfx() else { return };
    for round in 0..2 {
        println!("round {round}, {frames} frames");
        for path in [
            Path::WriteBacks,
            Path::MirrorDepthThroughMemory,
            Path::Mirror,
        ] {
            run(
                &format!("sync, {path:?}"),
                gfx().unwrap(),
                path,
                frames,
                depth,
                &colors,
            );
        }
        for path in [
            Path::WriteBacks,
            Path::MirrorDepthThroughMemory,
            Path::Mirror,
        ] {
            run(
                &format!("render thread, {path:?}"),
                ThreadedBackend::new(gfx().unwrap()).unwrap(),
                path,
                frames,
                depth,
                &colors,
            );
        }
    }
}
