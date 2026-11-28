//! Textures over the depth buffer's memory made on the GPU
//! (`GfxBackend::set_gpu_depth_aliases`) against
//! the old synchronous path (the device reads the depth buffer back,
//! decodes the bytes and uploads them): the same back buffer, byte for
//! byte, for every alias layout here, on the device's thread and through
//! the render thread. Varied depth (partial clears at several depths and
//! stencils, a quad with a depth gradient), aliases of several formats,
//! sizes, offsets, pitches and `COLORSIGN` masks, sampled after the depth
//! changed and again after it changed back. Synthetic headers; skipped
//! without an adapter or without `DEPTH32FLOAT_STENCIL8`.

mod common;

use common::*;
use d3d::device::DEPTH_BUFFER_DATA;
use d3d::{
    Backend, Device, GfxBackend, GuestLayout, GuestMemory, SparseMemory, ThreadedBackend,
    UploadPolicy,
};
use gfx::GfxContext;
use gfx::vertex::fvf;

const L: GuestLayout = GuestLayout::HALO2;
const ALIAS: u32 = MISC + 0x7860;
const RECTS: u32 = MISC + 0x7100;
const RS_ZWRITEENABLE: u32 = 64;
const RS_ZENABLE: u32 = 143;
const RS_ZFUNC_ALWAYS: u32 = 0x207;

/// An alias header: format, texel size, swizzled (`None`) or linear pitch,
/// byte offset into the depth buffer.
#[derive(Clone, Copy, Debug)]
struct Alias {
    format: u32,
    w: u32,
    h: u32,
    pitch: Option<u32>,
    offset: u32,
}

const CASES: [Alias; 7] = [
    // Halo's 0x509368 (LIN_A8R8G8B8) and 0x50936C (LIN_A8B8G8R8): the
    // whole buffer.
    Alias {
        format: 0x12,
        w: W,
        h: H,
        pitch: Some(W * 4),
        offset: 0,
    },
    Alias {
        format: 0x3F,
        w: W,
        h: H,
        pitch: Some(W * 4),
        offset: 0,
    },
    // LIN_X8R8G8B8, the lower half.
    Alias {
        format: 0x1E,
        w: W,
        h: 32,
        pitch: Some(W * 4),
        offset: W * 4 * 32,
    },
    // A narrower texture with a pitch of its own, from an odd row.
    Alias {
        format: 0x12,
        w: 24,
        h: 20,
        pitch: Some(128),
        offset: W * 4 * 7 + 64,
    },
    // LIN_B8G8R8A8 and LIN_R8G8B8A8.
    Alias {
        format: 0x40,
        w: 32,
        h: 32,
        pitch: Some(192),
        offset: 512,
    },
    Alias {
        format: 0x41,
        w: 32,
        h: 32,
        pitch: Some(192),
        offset: 512,
    },
    // Swizzled A8R8G8B8 over the top 32 rows.
    Alias {
        format: 0x06,
        w: 32,
        h: 32,
        pitch: None,
        offset: 0,
    },
];

fn write_alias(m: &mut SparseMemory, a: Alias) {
    let common = match m.read_u32(ALIAS).unwrap() {
        0 => 0x0004_0001,
        c => c,
    };
    let (format, size) = match a.pitch {
        Some(p) => (
            1 | (2 << 4) | (a.format << 8) | (1 << 16),
            (a.w - 1) | ((a.h - 1) << 12) | ((p / 64 - 1) << 24),
        ),
        None => (
            1 | (2 << 4)
                | (a.format << 8)
                | (1 << 16)
                | (a.w.trailing_zeros() << 20)
                | (a.h.trailing_zeros() << 24),
            0,
        ),
    };
    for (i, word) in [common, DEPTH_BUFFER_DATA + a.offset, 0, format, size]
        .iter()
        .enumerate()
    {
        m.write_u32(ALIAS + 4 * i as u32, *word).unwrap();
    }
}

/// A strip of `XYZRHW | DIFFUSE | TEX1` vertices: `(x, y, z, u, v)`.
fn strip<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    verts: &[(f32, f32, f32, f32, f32)],
    tex: bool,
) {
    let mut vb = Vec::new();
    for &(x, y, z, u, v) in verts {
        for f in [x, y, z, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        if tex {
            for f in [u, v] {
                vb.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    let (shader, stride) = if tex {
        (fvf::XYZRHW | fvf::DIFFUSE | 0x100, 28)
    } else {
        (fvf::XYZRHW | fvf::DIFFUSE, 20)
    };
    d.set_vertex_shader(m, shader).unwrap();
    d.set_stream_source(m, 0, VB_HEADER, stride).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, verts.len() as u32)
        .unwrap();
}

fn z_writes(m: &mut SparseMemory, on: bool) {
    m.write_u32(L.render_state_slot(RS_ZENABLE), on as u32)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_ZWRITEENABLE), on as u32)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_ZFUNC), RS_ZFUNC_ALWAYS)
        .unwrap();
}

/// Depth content `variant`: rectangles at several depths and stencils, and
/// a quad whose depth runs across it.
fn fill_depth<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, variant: u32) {
    let v = variant as f32;
    d.clear(m, 0, 0, 0x03, 0, 0.125 + 0.1 * v, 0x11 + variant)
        .unwrap();
    for (i, (rect, z, s)) in [
        ([0u32, 0, 40, 24], 0.333_333, 0x5A),
        ([20, 10, 64, 50], 0.9 - 0.2 * v, 0xA5),
        ([5, 40, 30, 64], 0.000_123, 0x01),
    ]
    .iter()
    .enumerate()
    {
        m.write(RECTS, &rect.map(u32::to_le_bytes).concat())
            .unwrap();
        d.clear(m, 1, RECTS, 0x03, 0, *z, s + i as u32 * variant)
            .unwrap();
    }
    z_writes(m, true);
    strip(
        d,
        m,
        &[
            (8.0, 8.0, 0.05, 0.0, 0.0),
            (56.0, 4.0, 0.7, 0.0, 0.0),
            (4.0, 60.0, 0.3 + 0.1 * v, 0.0, 0.0),
            (60.0, 56.0, 0.999, 0.0, 0.0),
        ],
        false,
    );
    z_writes(m, false);
}

/// Draws alias `a` with sign mask `sign` texel for texel into the top left
/// of the back buffer (cleared first) and reads the back buffer.
fn sample<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, a: Alias, sign: u32) -> Vec<u8> {
    let back = d.back_buffer().unwrap();
    d.clear(m, 0, 0, 0xF0, 0xFF20_4060, 1.0, 0).unwrap();
    write_alias(m, a);
    m.write_u32(L.texture_state_slot(0, 10), sign).unwrap();
    d.set_texture(m, 0, ALIAS).unwrap();
    let (w, h) = (a.w as f32, a.h as f32);
    let (u, v) = if a.pitch.is_some() {
        (w, h)
    } else {
        (1.0, 1.0)
    };
    strip(
        d,
        m,
        &[
            (0.0, 0.0, 0.5, 0.0, 0.0),
            (w, 0.0, 0.5, u, 0.0),
            (0.0, h, 0.5, 0.0, v),
            (w, h, 0.5, u, v),
        ],
        true,
    );
    d.set_texture(m, 0, 0).unwrap();
    d.backend
        .read_surface(
            back,
            gfx::D3DFormat::LinA8R8G8B8,
            W,
            H,
            d3d::backend::GuestLayoutKind::Linear { pitch: W * 4 },
        )
        .unwrap()
}

/// Every case and sign mask over three depth contents (the second differs,
/// the third is the first again: a new depth generation either way).
fn run<B: Backend>(backend: B) -> (Vec<Vec<u8>>, d3d::Stats) {
    let (mut d, mut m) = create(backend, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let mut out = Vec::new();
    for variant in [0, 1, 0] {
        fill_depth(&mut d, &mut m, variant);
        for a in CASES {
            for sign in [0, 0xA000_0000, 0x1000_0000] {
                out.push(sample(&mut d, &mut m, a, sign));
            }
        }
    }
    let events = d.take_events();
    assert!(events.is_empty(), "{events:?}");
    (out, d.stats())
}

fn backend(gpu: bool) -> Option<GfxBackend> {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping GPU test: no DEPTH32FLOAT_STENCIL8");
        return None;
    }
    let mut b = GfxBackend::new(ctx);
    b.set_gpu_depth_aliases(gpu);
    Some(b)
}

fn assert_same(what: &str, got: &[Vec<u8>], want: &[Vec<u8>]) {
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        if g != w {
            let p = g.iter().zip(w).position(|(a, b)| a != b).unwrap() / 4;
            panic!(
                "{what}: sample {i} (case {}, variant {}) differs first at pixel ({}, {}): {:?} vs {:?}",
                (i / 3) % CASES.len(),
                i / (3 * CASES.len()),
                p as u32 % W,
                p as u32 / W,
                &g[p * 4..p * 4 + 4],
                &w[p * 4..p * 4 + 4]
            );
        }
    }
}

/// A feedback shader samples a 2x2 window in a rendered level using a
/// different channel order. Each camera has a different second texel.
#[test]
fn partial_frame_alias_reads_the_same_guest_texels_from_each_camera_at_both_rasters() {
    let Ok(ctx) = GfxContext::headless() else { return };
    let guest = (16, 12);
    let source = gfx::D3DFormat::LinA8R8G8B8;
    let mut desc = gfx::TextureDesc::new(
        gfx::D3DFormat::LinA8B8G8R8, gfx::texture::TextureKind::D2, 2, 2, 1, 1,
    );
    desc.pitch = guest.0 * 4;
    let offset = ((3 * guest.0 + 5) * 4) as usize;

    // A 64x48 drawable at full and three-quarter resolution.
    for output in [(64, 48), (48, 36)] {
        let grid = d3d::scaled::RenderScale::between(guest, output).unwrap();
        let mut backend = d3d::ScaledBackend::with_resolution(
            GfxBackend::new(ctx.clone()), GfxBackend::new(ctx.clone()), grid, false,
        ).unwrap();
        assert!(backend.handles_surface_aliases());
        backend.create_surface(source, guest.0, guest.1, false).unwrap();
        let frame = backend.create_texture_target(source, guest.0, guest.1).unwrap();
        backend.clear(frame, None, gfx::Clear::all([1.0, 0.0, 0.0, 1.0])).unwrap();
        let display = backend.view_surface(&frame).unwrap();
        backend.view_mut().clear_rects(display, None, &d3d::RectClear {
            rects: vec![d3d::PixelRect { x: 6, y: 3, width: 1, height: 1 }],
            color: Some(([0.0, 1.0, 0.0, 1.0], d3d::ChannelMask::from_flags(0xf0))),
            depth: None,
            stencil: None,
        }).unwrap();
        let alias = backend.depth_alias_texture(
            frame, source, guest.0, guest.1, guest.0 * 4, 1, offset, &desc, Default::default(),
        ).unwrap().unwrap();
        backend.reference_mut().flush().unwrap();
        backend.view_mut().flush().unwrap();

        let read = |t: &wgpu::Texture, size| {
            assert_eq!((t.width(), t.height()), size);
            gfx::readback::read_texture(&ctx, t, wgpu::TextureAspect::All, 4, size)
        };
        let blue = [0, 0, 255, 255]; // red source decoded with R and B exchanged
        let green = [0, 255, 0, 255];
        assert!(read(&alias.reference.texture, (2, 2)).chunks_exact(4).all(|p| p == blue));
        let (w, h) = grid.size(2, 2);
        let display_pixels = read(&alias.view.unwrap().texture, (w, h));
        for (i, pixel) in display_pixels.chunks_exact(4).enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let expected = if x >= w / 2 && y < h / 2 { green } else { blue };
            assert_eq!(pixel, expected, "display alias at ({x}, {y}), raster {output:?}");
        }
    }
}

/// Depth-alias passes recorded into the open encoder (the default,
/// `GfxBackend::set_defer_side_passes`) draw what a submission of their own
/// per alias draws, with fewer submissions.
#[test]
fn deferred_depth_alias_passes_draw_what_submitted_ones_draw() {
    let Some(mut now) = backend(true) else { return };
    now.set_defer_side_passes(false);
    let (want, _) = run(now);
    let mut deferred = backend(true).unwrap();
    deferred.set_defer_side_passes(true);
    let (got, _) = run(deferred);
    assert_same("deferred side passes", &got, &want);
    let submits = |defer: bool| {
        let mut b = backend(true).unwrap();
        b.set_defer_side_passes(defer);
        let (mut d, mut m) = create(b, UploadPolicy::Always);
        set_states(&mut d, &mut m);
        fill_depth(&mut d, &mut m, 1);
        let before = d.backend.batch_stats().submits;
        for a in CASES {
            d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
            write_alias(&mut m, a);
            d.set_texture(&mut m, 0, ALIAS).unwrap();
            strip(
                &mut d,
                &mut m,
                &[
                    (0.0, 0.0, 0.5, 0.0, 0.0),
                    (8.0, 0.0, 0.5, 1.0, 0.0),
                    (0.0, 8.0, 0.5, 0.0, 1.0),
                ],
                true,
            );
        }
        d.backend.flush().unwrap();
        d.backend.batch_stats().submits - before
    };
    let (now, deferred) = (submits(false), submits(true));
    assert!(
        deferred < now,
        "deferred {deferred} submissions, submitted {now}"
    );
}

/// The GPU path takes no read-back: the depth words never come to the CPU.
#[test]
fn gpu_depth_aliases_read_nothing_back() {
    let Some(b) = backend(true) else { return };
    let (mut d, mut m) = create(b, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    fill_depth(&mut d, &mut m, 1);
    for a in CASES {
        d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
        write_alias(&mut m, a);
        d.set_texture(&mut m, 0, ALIAS).unwrap();
        strip(
            &mut d,
            &mut m,
            &[
                (0.0, 0.0, 0.5, 0.0, 0.0),
                (8.0, 0.0, 0.5, 1.0, 0.0),
                (0.0, 8.0, 0.5, 0.0, 1.0),
            ],
            true,
        );
    }
    assert!(d.take_events().is_empty());
    assert_eq!(d.stats().depth_alias_uploads, CASES.len() as u64);
    assert_eq!(d.backend.readbacks(), 0);
}

/// Wall time per frame of the menu's fog pattern at 640x480: a frame clears
/// colour and depth (a new depth generation), draws 200 depth-writing
/// quads, samples the whole depth buffer through a `LIN_A8R8G8B8` header in
/// one full-screen quad, draws 100 more quads and swaps. Old path (read
/// back, decode, upload) against the GPU pass, on the device's thread and
/// through the render thread; `H2_BENCH_FRAMES` frames (default 60) after
/// one warm-up frame, then a read-back of the back buffer drains the queue.
///
/// ```sh
/// cargo test --release -p d3d --test gpu_depth_alias -- --ignored --nocapture
/// ```
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn bench_depth_alias_frames() {
    const BW: u32 = 640;
    const BH: u32 = 480;
    let frames: u32 = std::env::var("H2_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    fn frame<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
        d.clear(m, 0, 0, 0xF3, 0xFF00_0000, 1.0, 0).unwrap();
        z_writes(m, true);
        let quad = |i: u32| {
            let (x, y) = ((i * 37 % 600) as f32, (i * 53 % 440) as f32);
            let z = (i % 97) as f32 / 97.0;
            [
                (x, y, z, 0.0, 0.0),
                (x + 40.0, y, z, 0.0, 0.0),
                (x, y + 40.0, z, 0.0, 0.0),
                (x + 40.0, y + 40.0, z, 0.0, 0.0),
            ]
        };
        for i in 0..200 {
            strip(d, m, &quad(i), false);
        }
        z_writes(m, false);
        write_alias(
            m,
            Alias {
                format: 0x12,
                w: BW,
                h: BH,
                pitch: Some(BW * 4),
                offset: 0,
            },
        );
        d.set_texture(m, 0, ALIAS).unwrap();
        let (w, h) = (BW as f32, BH as f32);
        strip(
            d,
            m,
            &[
                (0.0, 0.0, 0.5, 0.0, 0.0),
                (w, 0.0, 0.5, w, 0.0),
                (0.0, h, 0.5, 0.0, h),
                (w, h, 0.5, w, h),
            ],
            true,
        );
        d.set_texture(m, 0, 0).unwrap();
        for i in 200..300 {
            strip(d, m, &quad(i), false);
        }
        call(d, m, 0x3f9d00, d3d::abi::Regs::default(), &[]).unwrap();
    }
    fn time<B: Backend>(name: &str, backend: B, frames: u32) -> Option<f64> {
        let (mut d, mut m) = create_with(backend, UploadPolicy::Tracked, |p| {
            p.width = BW;
            p.height = BH;
        });
        set_states(&mut d, &mut m);
        frame(&mut d, &mut m);
        let start = std::time::Instant::now();
        for _ in 0..frames {
            frame(&mut d, &mut m);
        }
        let back = d.back_buffer().unwrap();
        d.backend
            .read_surface(
                back,
                gfx::D3DFormat::LinA8R8G8B8,
                BW,
                BH,
                d3d::backend::GuestLayoutKind::Linear { pitch: BW * 4 },
            )
            .unwrap();
        let ms = start.elapsed().as_secs_f64() * 1e3 / frames as f64;
        let events = d.take_events();
        assert!(events.is_empty(), "{events:?}");
        println!(
            "{name:<34} {ms:>8.2} ms/frame  ({} depth reads, {} alias textures)",
            d.stats().depth_reads,
            d.stats().depth_alias_uploads
        );
        Some(ms)
    }
    let Some(_) = backend(true) else { return };
    for round in 0..2 {
        println!("round {round}, {frames} frames");
        time(
            "sync, read-back (GPU aliases off)",
            backend(false).unwrap(),
            frames,
        );
        time("sync, GPU aliases", backend(true).unwrap(), frames);
        time(
            "render thread, read-back",
            ThreadedBackend::new(backend(false).unwrap()).unwrap(),
            frames,
        );
        time(
            "render thread, GPU aliases",
            ThreadedBackend::new(backend(true).unwrap()).unwrap(),
            frames,
        );
    }
}
