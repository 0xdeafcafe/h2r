//! Overlapping render-to-texture levels through the alias mirror
//! (`Device::set_alias_mirror`, `H2_D3D_ALIAS_MIRROR`) against the
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
/// A free-standing 32x32 colour target for the depth pass.
const CSURF: u32 = MISC + 0x7840;
const CDATA: u32 = 0x0050_0000;

const RS_ALPHABLENDENABLE: u32 = 59;
const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;
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
const X: Level = Level {
    tex: MISC + 0x4300,
    level: MISC + 0x4340,
    format: D3DFormat::LinX8R8G8B8,
    w: 32,
    h: 16,
    pitch: Some(192),
    at: 1024,
};
const D: Level = Level {
    tex: MISC + 0x4400,
    level: MISC + 0x4440,
    format: D3DFormat::LinD24S8,
    w: 32,
    h: 32,
    pitch: Some(128),
    at: 256,
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

fn blend(m: &mut SparseMemory, on: bool) {
    m.write_u32(L.render_state_slot(RS_ALPHABLENDENABLE), on as u32)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_SRCBLEND), 0x302)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_DESTBLEND), 0x303)
        .unwrap();
}

fn z_writes(m: &mut SparseMemory, on: bool) {
    m.write_u32(L.render_state_slot(RS_ZENABLE), on as u32)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_ZWRITEENABLE), on as u32)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_ZFUNC), 0x207).unwrap();
    m.write_u32(L.render_state_slot(RS_STENCILENABLE), 0)
        .unwrap();
}

/// A guest store into the pool, reported exactly.
fn store<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, at: u32, bytes: &[u8]) {
    m.write(0x8000_0000 | (POOL + at), bytes).unwrap();
    d.note_write(0x8000_0000 | (POOL + at), bytes.len() as u32);
}

fn pool(m: &SparseMemory) -> Vec<u8> {
    m.read_vec(0x8000_0000 | POOL, POOL_LEN as usize).unwrap()
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

/// What the guest could see: the back buffer and the pool at each wait,
/// then both once everything is written back.
#[derive(Debug, PartialEq)]
struct Seen(Vec<(&'static str, Vec<u8>)>);

fn scenario<B: Backend>(backend: B, mirror: bool, upload: UploadPolicy) -> (Seen, d3d::Stats) {
    let (mut d, mut m) = create(backend, upload);
    d.set_alias_mirror(mirror);
    set_states(&mut d, &mut m);
    m.map(0x8000_0000 | POOL, POOL_LEN);
    let fill: Vec<u8> = (0..POOL_LEN).map(|i| (i * 7 + (i >> 8)) as u8).collect();
    m.write(0x8000_0000 | POOL, &fill).unwrap();
    m.map(0x8000_0000 | CDATA, 32 * 32 * 4);
    SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, 32, 32, 128, CDATA)
        .write(&mut m, CSURF)
        .unwrap();
    for l in [A, B, C, X, D] {
        define(&mut m, l);
    }
    let back_header = m
        .read_u32(L.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    let mut seen = Vec::new();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF10_2030, 1.0, 0).unwrap();

    // A: part cleared, part a gradient; the rest from the pool.
    d.set_render_target(&mut m, A.level, 0).unwrap();
    clear_rect(&mut d, &mut m, [0, 0, 20, 12], 0xF0, 0xFFC0_2010, 1.0, 0);
    quad(
        &mut d,
        &mut m,
        (8.0, 6.0, 30.0, 28.0),
        None,
        [0xFF00_00FF, 0x80FF_0000, 0x4000_FF00, 0xFFFF_FFFF],
        0.5,
    );
    // B over part of A (another pitch): samples A, partly.
    d.set_render_target(&mut m, B.level, 0).unwrap();
    sampled(&mut d, &mut m, (2.0, 2.0, 14.0, 12.0), A, 0xFFFF_80C0);
    // C over A, swizzled: B blended over part of it.
    d.set_render_target(&mut m, C.level, 0).unwrap();
    blend(&mut m, true);
    sampled(&mut d, &mut m, (4.0, 4.0, 28.0, 30.0), B, 0x80FF_FFFF);
    blend(&mut m, false);
    // A guest store while C is the target (the target's write-back wins).
    store(&mut d, &mut m, 3000, &[0x5A; 64]);
    // X over C's bytes, with pitch padding.
    d.set_render_target(&mut m, X.level, 0).unwrap();
    clear_rect(&mut d, &mut m, [0, 0, 32, 4], 0xF0, 0x1234_5678, 1.0, 0);
    sampled(&mut d, &mut m, (0.0, 4.0, 32.0, 16.0), C, 0xFFFF_FFFF);
    // Guest stores into bytes the mirror (or memory) took from C.
    store(&mut d, &mut m, 200, &[0xA5; 24]);
    store(&mut d, &mut m, 2052, &[0x11, 0x22]);
    // The depth level over X's bytes, with a free-standing colour target.
    d.set_render_target(&mut m, CSURF, D.level).unwrap();
    z_writes(&mut m, true);
    clear_rect(&mut d, &mut m, [0, 0, 32, 16], 0x03, 0, 0.25, 0x3C);
    quad(
        &mut d,
        &mut m,
        (6.0, 10.0, 30.0, 31.0),
        None,
        [0xFFFF_FFFF; 4],
        0.75,
    );
    z_writes(&mut m, false);
    // The back buffer samples them all: A and C in place, X through
    // memory (X8 is not sampled in place), B in place.
    d.set_render_target(&mut m, back_header, 0).unwrap();
    sampled(&mut d, &mut m, (0.0, 0.0, 32.0, 32.0), A, 0xFFFF_FFFF);
    sampled(&mut d, &mut m, (32.0, 0.0, 64.0, 32.0), C, 0xFFFF_FFFF);
    sampled(&mut d, &mut m, (0.0, 32.0, 32.0, 48.0), X, 0xFFFF_FFFF);
    sampled(&mut d, &mut m, (32.0, 32.0, 48.0, 48.0), B, 0xFFFF_FFFF);
    seen.push(("back after the first pass", back(&mut d)));
    // The guest waits on B: B's bytes as it then reads them. (Bytes
    // outside what it waits on may still be on the host either way.)
    d.block_on_resource(&mut m, B.tex).unwrap();
    let span = (B.at as usize)..(B.at + B.pitch.unwrap() * B.h) as usize;
    seen.push((
        "B's bytes after BlockOnResource(B)",
        pool(&m)[span].to_vec(),
    ));
    // A second round: part of a word stored, then A drawn and C sampled.
    store(&mut d, &mut m, 130, &[0x77]);
    d.set_render_target(&mut m, A.level, 0).unwrap();
    quad(
        &mut d,
        &mut m,
        (0.0, 20.0, 16.0, 32.0),
        None,
        [0xFF33_6699; 4],
        0.5,
    );
    d.set_render_target(&mut m, C.level, 0).unwrap();
    sampled(&mut d, &mut m, (16.0, 0.0, 32.0, 16.0), A, 0xFFFF_FFFF);
    d.set_render_target(&mut m, back_header, 0).unwrap();
    sampled(&mut d, &mut m, (0.0, 48.0, 32.0, 64.0), C, 0xFFFF_FFFF);
    seen.push(("back after the second pass", back(&mut d)));
    d.flush_render_targets(&mut m).unwrap();
    seen.push(("pool at the end", pool(&m)));
    let events = d.take_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_))),
        "{events:?}"
    );
    (Seen(seen), d.stats())
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

fn assert_seen(what: &str, got: &Seen, want: &Seen) {
    for ((name, g), (_, w)) in got.0.iter().zip(&want.0) {
        if g != w {
            let i = g.iter().zip(w).position(|(a, b)| a != b).unwrap();
            panic!(
                "{what}: {name} differs first at byte {i}: {:#04x} vs {:#04x}",
                g[i], w[i]
            );
        }
    }
    assert_eq!(got.0.len(), want.0.len());
}

#[test]
fn overlapping_levels_through_the_mirror_match_the_write_backs() {
    for upload in [UploadPolicy::Tracked, UploadPolicy::Verify] {
        let Some(b) = gfx() else { return };
        let (want, sync) = scenario(b, false, upload);
        let (got, mirror) = scenario(gfx().unwrap(), true, upload);
        assert_seen(&format!("{upload:?}"), &got, &want);
        // The bytes were really moved on the GPU, and fewer levels went
        // through guest memory.
        assert!(sync.writeback_causes.overlap >= 4, "{sync:?}");
        assert!(mirror.mirror_stores >= 4, "{mirror:?}");
        assert!(mirror.mirror_loads >= 2, "{mirror:?}");
        assert!(
            mirror.writeback_causes.overlap < sync.writeback_causes.overlap,
            "{mirror:?}"
        );
        // The depth level's fill and the X8 sample took bytes through
        // guest memory.
        assert!(mirror.mirror_flushes >= 1, "{mirror:?}");

        let (threaded, _) = scenario(ThreadedBackend::new(gfx().unwrap()).unwrap(), true, upload);
        assert_seen(&format!("{upload:?}, render thread"), &threaded, &want);
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

#[test]
fn a_pool_only_the_gpu_reads_takes_no_read_back() {
    let Some(b) = gfx() else { return };
    let (mut d, mut m) = pool_frames(b, true, 5);
    let s = d.stats();
    assert_eq!(s.writeback_causes.overlap, 0, "{s:?}");
    assert_eq!(s.mirror_flushes, 0, "{s:?}");
    assert_eq!(d.backend.readbacks(), 0);
    let shown = back(&mut d);
    d.flush_render_targets(&mut m).unwrap();
    let mem = pool(&m);
    let (mut e, mut n) = pool_frames(gfx().unwrap(), false, 5);
    assert!(e.stats().writeback_causes.overlap >= 10);
    assert_eq!(shown, back(&mut e));
    e.flush_render_targets(&mut n).unwrap();
    assert_eq!(mem, pool(&n));
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
