//! `gfx::guest_words` against the CPU paths it replaces, byte for byte:
//! a colour level encoded into the mirror against `read_color` plus
//! `encode_color` (the write-back), a level decoded from the mirror against
//! `write_color` of the same bytes (the fill), a depth level against
//! `read_depth_as`, and a texture over the depth buffer's memory against
//! `read_depth_as` plus `decode_texture`. Synthetic random texels, swizzled
//! and linear layouts with padded pitches and word offsets. Skipped without
//! an adapter (and the depth cases without `DEPTH32FLOAT_STENCIL8`).

use gfx::GfxContext;
use gfx::HostFormat;
use gfx::format::D3DFormat;
use gfx::guest_words::{GuestWords, WordLayout, sign_flip};
use gfx::surface::{SurfaceDesc, SurfaceKind, SurfaceLayout, SurfaceStore};
use gfx::texture::{DecodeOptions, TextureDesc, TextureKind, decode_texture};

fn ctx() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

/// xorshift bytes.
fn random(seed: u32, n: usize) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s >> 7) as u8
        })
        .collect()
}

fn mirror(ctx: &GfxContext, bytes: &[u8]) -> wgpu::Buffer {
    let b = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test mirror"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    ctx.queue.write_buffer(&b, 0, bytes);
    b
}

fn read_buffer(ctx: &GfxContext, b: &wgpu::Buffer) -> Vec<u8> {
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test mirror read"),
        size: b.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(b, 0, &staging, 0, b.size());
    ctx.queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    slice.get_mapped_range().unwrap().to_vec()
}

/// Byte offset in a level's guest bytes of texel `(x, y)`: swizzled or
/// rows of `pitch` bytes.
fn texel_at(x: u32, y: u32, w: u32, h: u32, pitch: Option<u32>) -> usize {
    match pitch {
        Some(p) => (y * p + x * 4) as usize,
        None => {
            let s = gfx::swizzle::Swizzler::new(w, h, 1).unwrap();
            s.index(x, y, 0) * 4
        }
    }
}

fn layout_of(pitch: Option<u32>) -> SurfaceLayout {
    match pitch {
        Some(pitch) => SurfaceLayout::Linear { pitch },
        None => SurfaceLayout::Swizzled,
    }
}

fn desc(format: D3DFormat, w: u32, h: u32, pitch: Option<u32>) -> TextureDesc {
    let mut d = TextureDesc::new(format, TextureKind::D2, w, h, 1, 1);
    if let Some(p) = pitch {
        d.pitch = p;
    }
    d
}

/// Guest bytes a level occupies.
fn guest_len(w: u32, h: u32, pitch: Option<u32>) -> usize {
    match pitch {
        Some(p) => (p * (h - 1) + w * 4) as usize,
        None => (w * h * 4) as usize,
    }
}

/// Colour levels: swizzled, linear with a padded pitch, X8 (alpha forced).
const COLOR_CASES: [(D3DFormat, u32, u32, Option<u32>); 4] = [
    (D3DFormat::A8R8G8B8, 32, 16, None),
    (D3DFormat::LinA8R8G8B8, 20, 12, Some(96)),
    (D3DFormat::LinX8R8G8B8, 20, 12, Some(80)),
    (D3DFormat::X8R8G8B8, 16, 16, None),
];

#[test]
fn color_levels_store_into_the_mirror_as_the_readback_encodes_them() {
    let Some(ctx) = ctx() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let words = GuestWords::new(&ctx);
    for (i, &(format, w, h, pitch)) in COLOR_CASES.iter().enumerate() {
        let s = store
            .create_texture_target(
                &ctx,
                SurfaceDesc::new(format, w, h).with_layout(layout_of(pitch)),
                1,
            )
            .unwrap();
        let texels = random(11 + i as u32, guest_len(w, h, pitch));
        store
            .write_color(&ctx, s, &desc(format, w, h, pitch), &texels)
            .unwrap();
        // The old path: read back, encode.
        let (px, order) = store.read_color(&ctx, s).unwrap();
        let want_level =
            gfx::encode::encode_color(format, order, &px, w, h, &layout_of(pitch)).unwrap();
        // The mirror: random bytes around, the level at word 37.
        let base = 37u32;
        let before = random(
            99 + i as u32,
            (base as usize * 4) + guest_len(w, h, pitch) + 64,
        );
        let m = mirror(&ctx, &before);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let tex = store.get(s).unwrap().texture().clone();
        let layout = WordLayout {
            base,
            pitch_words: pitch.map_or(0, |p| p / 4),
        };
        words
            .store_color(&ctx, &mut enc, &tex, format, (w, h), &m, layout)
            .unwrap();
        ctx.queue.submit([enc.finish()]);
        let got = read_buffer(&ctx, &m);
        let mut want = before.clone();
        for y in 0..h {
            for x in 0..w {
                let o = texel_at(x, y, w, h, pitch);
                let at = base as usize * 4 + o;
                want[at..at + 4].copy_from_slice(&want_level[o..o + 4]);
            }
        }
        assert_eq!(got, want, "{format:?} {w}x{h} pitch {pitch:?}");
    }
}

#[test]
fn color_levels_load_from_the_mirror_as_the_fill_decodes_them() {
    let Some(ctx) = ctx() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let mut words = GuestWords::new(&ctx);
    for (i, &(format, w, h, pitch)) in COLOR_CASES.iter().enumerate() {
        let make = |store: &mut SurfaceStore| {
            store
                .create_texture_target(
                    &ctx,
                    SurfaceDesc::new(format, w, h).with_layout(layout_of(pitch)),
                    1,
                )
                .unwrap()
        };
        let (cpu, gpu) = (make(&mut store), make(&mut store));
        let base = 21u32;
        let bytes = random(
            7 + i as u32,
            base as usize * 4 + guest_len(w, h, pitch) + 32,
        );
        store
            .write_color(
                &ctx,
                cpu,
                &desc(format, w, h, pitch),
                &bytes[base as usize * 4..],
            )
            .unwrap();
        let m = mirror(&ctx, &bytes);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let tex = store.get(gpu).unwrap().texture().clone();
        let layout = WordLayout {
            base,
            pitch_words: pitch.map_or(0, |p| p / 4),
        };
        words
            .load_color(&ctx, &mut enc, &tex, format, (w, h), &m, layout)
            .unwrap();
        ctx.queue.submit([enc.finish()]);
        assert_eq!(
            store.read_color(&ctx, gpu).unwrap(),
            store.read_color(&ctx, cpu).unwrap(),
            "{format:?} {w}x{h} pitch {pitch:?}"
        );
    }
}

fn depth_ctx() -> Option<GfxContext> {
    let ctx = ctx()?;
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping GPU test: no DEPTH32FLOAT_STENCIL8");
        return None;
    }
    Some(ctx)
}

/// A `D24S8` depth surface filled with random guest words.
fn depth_surface(
    ctx: &GfxContext,
    store: &mut SurfaceStore,
    format: D3DFormat,
    w: u32,
    h: u32,
    pitch: Option<u32>,
    seed: u32,
) -> gfx::surface::SurfaceId {
    let s = store
        .create(
            ctx,
            SurfaceDesc::new(format, w, h).with_layout(layout_of(pitch)),
            SurfaceKind::DepthStencil,
        )
        .unwrap();
    let bytes = random(seed, guest_len(w, h, pitch));
    store
        .write_depth(ctx, s, &desc(format, w, h, pitch), &bytes)
        .unwrap();
    s
}

#[test]
fn depth_levels_store_into_the_mirror_as_the_readback_encodes_them() {
    let Some(ctx) = depth_ctx() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let words = GuestWords::new(&ctx);
    for (i, &(format, w, h, pitch)) in [
        (D3DFormat::D24S8, 32, 32, None),
        (D3DFormat::LinD24S8, 24, 10, Some(112)),
    ]
    .iter()
    .enumerate()
    {
        let s = depth_surface(&ctx, &mut store, format, w, h, pitch, 5 + i as u32);
        let want_level = store
            .read_depth_as(&ctx, s, format, &layout_of(pitch))
            .unwrap();
        let base = 9u32;
        let before = random(3, base as usize * 4 + guest_len(w, h, pitch) + 16);
        let m = mirror(&ctx, &before);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let tex = store.get(s).unwrap().texture().clone();
        let layout = WordLayout {
            base,
            pitch_words: pitch.map_or(0, |p| p / 4),
        };
        words
            .store_depth(&ctx, &mut enc, &tex, (w, h), &m, layout)
            .unwrap();
        ctx.queue.submit([enc.finish()]);
        let got = read_buffer(&ctx, &m);
        let mut want = before.clone();
        for y in 0..h {
            for x in 0..w {
                let o = texel_at(x, y, w, h, pitch);
                let at = base as usize * 4 + o;
                want[at..at + 4].copy_from_slice(&want_level[o..o + 4]);
            }
        }
        assert_eq!(got, want, "{format:?} {w}x{h} pitch {pitch:?}");
    }
}

#[test]
fn depth_levels_load_from_the_mirror_as_the_fill_decodes_them() {
    let Some(ctx) = depth_ctx() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let mut words = GuestWords::new(&ctx);
    for (i, &(format, w, h, pitch)) in [
        (D3DFormat::D24S8, 32, 32, None),
        (D3DFormat::LinD24S8, 24, 10, Some(112)),
        // A width whose stencil rows are not 256-byte aligned.
        (D3DFormat::LinD24S8, 37, 9, Some(160)),
        (D3DFormat::LinD24S8, 512, 512, Some(2048)),
    ]
    .iter()
    .enumerate()
    {
        let make = |store: &mut SurfaceStore| {
            store
                .create(
                    &ctx,
                    SurfaceDesc::new(format, w, h).with_layout(layout_of(pitch)),
                    SurfaceKind::DepthStencil,
                )
                .unwrap()
        };
        let (cpu, gpu) = (make(&mut store), make(&mut store));
        let base = 13u32;
        let mut bytes = random(
            41 + i as u32,
            base as usize * 4 + guest_len(w, h, pitch) + 24,
        );
        // The ends of the range and each power of two (and one below it).
        let mut edges = vec![0u32, 1, 0xFF_FFFF, 0xFF_FFFE];
        for b in 1..24 {
            edges.extend([1 << b, (1 << b) - 1, (1 << b) + 1]);
        }
        for (k, z) in edges.iter().enumerate().take((w * h) as usize) {
            let (x, y) = (k as u32 % w, k as u32 / w);
            let at = base as usize * 4 + texel_at(x, y, w, h, pitch);
            bytes[at..at + 4].copy_from_slice(&((z << 8) | (k as u32 & 0xFF)).to_le_bytes());
        }
        store
            .write_depth(
                &ctx,
                cpu,
                &desc(format, w, h, pitch),
                &bytes[base as usize * 4..],
            )
            .unwrap();
        let m = mirror(&ctx, &bytes);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let tex = store.get(gpu).unwrap().texture().clone();
        let layout = WordLayout {
            base,
            pitch_words: pitch.map_or(0, |p| p / 4),
        };
        words
            .load_depth(&ctx, &mut enc, &tex, (w, h), &m, layout)
            .unwrap();
        ctx.queue.submit([enc.finish()]);
        let (got, want) = (
            store.read_depth(&ctx, gpu).unwrap(),
            store.read_depth(&ctx, cpu).unwrap(),
        );
        let bits = |d: &[f32]| d.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            bits(&got.depth),
            bits(&want.depth),
            "{format:?} {w}x{h} pitch {pitch:?} depth"
        );
        assert_eq!(
            got.stencil, want.stencil,
            "{format:?} {w}x{h} pitch {pitch:?} stencil"
        );
        assert!(want.stencil.is_some());
    }
}

/// Decoded host texels as logical RGBA.
fn logical(host: HostFormat, data: &[u8]) -> Vec<u8> {
    match host {
        HostFormat::Bgra8Unorm => data
            .chunks(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect(),
        _ => data.to_vec(),
    }
}

#[test]
fn depth_aliases_decode_on_the_gpu_as_the_readback_decodes_them() {
    let Some(ctx) = depth_ctx() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let words = GuestWords::new(&ctx);
    // The depth buffer: linear D24S8, 64x16, rows of 256 bytes.
    let (dw, dh) = (64u32, 16u32);
    let depth = depth_surface(
        &ctx,
        &mut store,
        D3DFormat::LinD24S8,
        dw,
        dh,
        Some(dw * 4),
        41,
    );
    let bytes = store
        .read_depth_as(
            &ctx,
            depth,
            D3DFormat::LinD24S8,
            &SurfaceLayout::Linear { pitch: dw * 4 },
        )
        .unwrap();
    let tex = store.get(depth).unwrap().texture().clone();
    let signs = [
        [false; 4],
        [true, false, true, false],
        [false, false, false, true],
    ];
    for (format, w, h, pitch, offset) in [
        // The whole buffer as one texture (Halo's 0x509368 and 0x50936C).
        (D3DFormat::LinA8R8G8B8, 64, 16, Some(256), 0u32),
        (D3DFormat::LinA8B8G8R8, 64, 16, Some(256), 0),
        // Part of it, from an offset, with another pitch.
        (D3DFormat::LinA8R8G8B8, 20, 7, Some(132), 344),
        (D3DFormat::LinX8R8G8B8, 16, 4, Some(256), 1024),
        (D3DFormat::LinR8G8B8A8, 8, 8, Some(64), 12),
        (D3DFormat::LinB8G8R8A8, 8, 8, Some(64), 12),
        (D3DFormat::A8R8G8B8, 16, 16, None, 64),
    ] {
        for signed in signs {
            let d = desc(format, w, h, pitch);
            let opts = DecodeOptions {
                signed,
                ..DecodeOptions::default()
            };
            let cpu = decode_texture(&d, &bytes[offset as usize..], None, opts).unwrap();
            let want = logical(cpu.host, &cpu.levels[0].data);
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            let layout = WordLayout {
                base: offset / 4,
                pitch_words: pitch.map_or(0, |p| p / 4),
            };
            let out = words
                .depth_alias(
                    &ctx,
                    &mut enc,
                    &tex,
                    format,
                    (w, h),
                    layout,
                    sign_flip(signed),
                )
                .unwrap();
            ctx.queue.submit([enc.finish()]);
            let got = gfx::readback::read_texture(&ctx, &out, wgpu::TextureAspect::All, 4, (w, h));
            assert_eq!(
                got, want,
                "{format:?} {w}x{h} at +{offset} signed {signed:?}"
            );
        }
    }
}

#[test]
fn levels_past_the_mirror_or_in_other_formats_are_refused() {
    let Some(ctx) = ctx() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let words = GuestWords::new(&ctx);
    let s = store
        .create_texture_target(&ctx, SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 8, 8), 1)
        .unwrap();
    let tex = store.get(s).unwrap().texture().clone();
    let m = mirror(&ctx, &[0; 64]);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let at = |base| WordLayout {
        base,
        pitch_words: 8,
    };
    assert!(
        words
            .store_color(
                &ctx,
                &mut enc,
                &tex,
                D3DFormat::LinA8R8G8B8,
                (8, 8),
                &m,
                at(0)
            )
            .is_err()
    );
    assert!(
        words
            .store_color(
                &ctx,
                &mut enc,
                &tex,
                D3DFormat::LinR5G6B5,
                (2, 2),
                &m,
                at(0)
            )
            .is_err()
    );
    assert!(
        words
            .store_color(
                &ctx,
                &mut enc,
                &tex,
                D3DFormat::A8R8G8B8,
                (3, 2),
                &m,
                WordLayout {
                    base: 0,
                    pitch_words: 0
                }
            )
            .is_err()
    );
    assert!(
        words
            .store_color(
                &ctx,
                &mut enc,
                &tex,
                D3DFormat::LinA8R8G8B8,
                (2, 2),
                &m,
                at(6)
            )
            .is_ok()
    );
}
