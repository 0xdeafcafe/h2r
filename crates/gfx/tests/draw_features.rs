//! Per-draw viewport and scissor, indexed draws, the built-in alpha test,
//! partial and channel-masked clears, and render-to-texture surfaces, all
//! checked by reading pixels back. They need an adapter (Metal on macOS) and
//! skip themselves when there is none.

use gfx::clear::MaskedClear;
use gfx::program::ProgramBindings;
use gfx::renderer::SHADER_XYZRHW_DIFFUSE;
use gfx::state::{CompareFunc, CullMode, DepthBufferMode, RenderState, TextureStageState};
use gfx::surface::{HostPixelOrder, SurfaceDesc, SurfaceKind, SurfaceStore};
use gfx::translate::SamplerKey;
use gfx::vertex::{PrimitiveType, VertexLayout, fvf};
use gfx::{
    Clear, D3DFormat, Draw, DrawViewport, GfxContext, OffscreenTarget, Renderer, ScissorRect,
    VertexStream,
};

struct Gpu {
    ctx: GfxContext,
    scope: Option<wgpu::ErrorScopeGuard>,
}

impl std::ops::Deref for Gpu {
    type Target = GfxContext;
    fn deref(&self) -> &GfxContext {
        &self.ctx
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some(scope) = self.scope.take() {
            let err = pollster::block_on(scope.pop());
            if !std::thread::panicking() {
                assert!(err.is_none(), "wgpu validation error: {err:?}");
            }
        }
    }
}

fn context() -> Option<Gpu> {
    match GfxContext::headless() {
        Ok(ctx) => {
            let scope = Some(ctx.device.push_error_scope(wgpu::ErrorFilter::Validation));
            Some(Gpu { ctx, scope })
        }
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

const W: u32 = 32;
const H: u32 = 32;

/// `XYZRHW | DIFFUSE`, z as a fraction of the 24-bit range.
fn vertex(x: f32, y: f32, z: f32, color: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for f in [x, y, z * 16_777_215.0, 1.0] {
        v.extend_from_slice(&f.to_le_bytes());
    }
    v.extend_from_slice(&color.to_le_bytes());
    v
}

fn quad(x0: f32, y0: f32, x1: f32, y1: f32, z: f32, color: u32) -> Vec<u8> {
    [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .iter()
        .flat_map(|&(x, y)| vertex(x, y, z, color))
        .collect()
}

fn layout() -> VertexLayout {
    VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap()
}

fn state() -> RenderState {
    RenderState {
        cull_mode: CullMode::None,
        depth_mode: DepthBufferMode::Disabled,
        ..Default::default()
    }
}

/// BGRA bytes of an offscreen target pixel, as RGBA.
fn rgba(px: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [px[i + 2], px[i + 1], px[i], px[i + 3]]
}

const RED: u32 = 0xFFFF_0000;
const BLACK: [u8; 4] = [0, 0, 0, 255];

fn draw_one(ctx: &Gpu, target: &OffscreenTarget, renderer: &mut Renderer, d: Draw<'_>) {
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    renderer
        .render(
            ctx,
            &mut enc,
            &target.target(),
            Clear::all([0.0, 0.0, 0.0, 1.0]),
            &[d],
        )
        .unwrap();
    ctx.queue.submit([enc.finish()]);
}

#[test]
fn scissor_clips_and_viewport_maps() {
    let Some(ctx) = context() else { return };
    let target = OffscreenTarget::new(&ctx.device, (W, H), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let l = layout();
    let full = quad(0.0, 0.0, W as f32, H as f32, 0.5, RED);
    let streams = [VertexStream {
        data: &full,
        stride: 0,
    }];
    let base = || {
        Draw::new(
            SHADER_XYZRHW_DIFFUSE,
            &l,
            &streams,
            4,
            PrimitiveType::QuadList,
            state(),
        )
    };

    // Scissor: only the rectangle is drawn.
    let d = Draw {
        scissor: Some(ScissorRect {
            x: 8,
            y: 4,
            width: 8,
            height: 20,
        }),
        ..base()
    };
    draw_one(&ctx, &target, &mut renderer, d);
    let px = target.read_pixels(&ctx);
    assert_eq!(rgba(&px, 8, 4), [255, 0, 0, 255]);
    assert_eq!(rgba(&px, 15, 23), [255, 0, 0, 255]);
    assert_eq!(rgba(&px, 7, 10), BLACK);
    assert_eq!(rgba(&px, 16, 10), BLACK);
    assert_eq!(rgba(&px, 10, 24), BLACK);

    // A scissor outside the target draws nothing.
    let d = Draw {
        scissor: Some(ScissorRect {
            x: 40,
            y: 0,
            width: 8,
            height: 8,
        }),
        ..base()
    };
    draw_one(&ctx, &target, &mut renderer, d);
    assert_eq!(rgba(&target.read_pixels(&ctx), 2, 2), BLACK);

    // Viewport: clip space maps into the right half.
    let d = Draw {
        viewport: Some(DrawViewport {
            x: 16.0,
            ..DrawViewport::full(16, H)
        }),
        ..base()
    };
    draw_one(&ctx, &target, &mut renderer, d);
    let px = target.read_pixels(&ctx);
    assert_eq!(rgba(&px, 15, 16), BLACK);
    assert_eq!(rgba(&px, 16, 16), [255, 0, 0, 255]);
    assert_eq!(rgba(&px, 31, 31), [255, 0, 0, 255]);

    // A viewport outside the target is an error.
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let d = Draw {
        viewport: Some(DrawViewport {
            x: 20.0,
            ..DrawViewport::full(16, H)
        }),
        ..base()
    };
    assert!(matches!(
        renderer.render(&ctx, &mut enc, &target.target(), Clear::NONE, &[d]),
        Err(gfx::renderer::DrawError::BadViewport(..))
    ));
}

#[test]
fn indexed_draws() {
    let Some(ctx) = context() else { return };
    let target = OffscreenTarget::new(&ctx.device, (W, H), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let l = layout();
    // Corners of the left and right halves, red and green.
    let verts: Vec<u8> = [
        vertex(0.0, 0.0, 0.5, RED),
        vertex(16.0, 0.0, 0.5, RED),
        vertex(16.0, 32.0, 0.5, RED),
        vertex(0.0, 32.0, 0.5, RED),
        vertex(16.0, 0.0, 0.5, 0xFF00_FF00),
        vertex(32.0, 0.0, 0.5, 0xFF00_FF00),
        vertex(32.0, 32.0, 0.5, 0xFF00_FF00),
        vertex(16.0, 32.0, 0.5, 0xFF00_FF00),
    ]
    .concat();
    let streams = [VertexStream {
        data: &verts,
        stride: 0,
    }];
    // A triangle list naming only the green half.
    let tri = [4u32, 5, 6, 4, 6, 7];
    let d = Draw {
        indices: Some(&tri),
        ..Draw::new(
            SHADER_XYZRHW_DIFFUSE,
            &l,
            &streams,
            8,
            PrimitiveType::TriangleList,
            state(),
        )
    };
    draw_one(&ctx, &target, &mut renderer, d);
    let px = target.read_pixels(&ctx);
    assert_eq!(rgba(&px, 4, 16), BLACK);
    assert_eq!(rgba(&px, 24, 16), [0, 255, 0, 255]);

    // A quad list over indices: the rewrite to triangles composes with them.
    let quads = [0u32, 1, 2, 3];
    let d = Draw {
        indices: Some(&quads),
        ..Draw::new(
            SHADER_XYZRHW_DIFFUSE,
            &l,
            &streams,
            8,
            PrimitiveType::QuadList,
            state(),
        )
    };
    draw_one(&ctx, &target, &mut renderer, d);
    let px = target.read_pixels(&ctx);
    assert_eq!(rgba(&px, 4, 16), [255, 0, 0, 255]);
    assert_eq!(rgba(&px, 24, 16), BLACK);

    // Out-of-range indices are rejected before anything is recorded.
    let bad = [0u32, 1, 8];
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let d = Draw {
        indices: Some(&bad),
        ..Draw::new(
            SHADER_XYZRHW_DIFFUSE,
            &l,
            &streams,
            8,
            PrimitiveType::TriangleList,
            state(),
        )
    };
    assert_eq!(
        renderer
            .render(&ctx, &mut enc, &target.target(), Clear::NONE, &[d])
            .unwrap_err(),
        gfx::renderer::DrawError::IndexOutOfRange {
            index: 8,
            vertices: 8
        }
    );
}

#[test]
fn builtin_alpha_test() {
    let Some(ctx) = context() else { return };
    let target = OffscreenTarget::new(&ctx.device, (W, H), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let l = layout();
    // Four columns with alpha 0x00, 0x40, 0x80, 0xFF, drawn in one pass.
    let cols: Vec<Vec<u8>> = [0x00u32, 0x40, 0x80, 0xFF]
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let x = 8.0 * i as f32;
            quad(x, 0.0, x + 8.0, H as f32, 0.5, (a << 24) | 0x00FF_FFFF)
        })
        .collect();
    let streams: Vec<[VertexStream; 1]> = cols
        .iter()
        .map(|c| [VertexStream { data: c, stride: 0 }])
        .collect();
    let mut run = |func: CompareFunc, reference: u8| {
        let rs = RenderState {
            alpha_test_enable: true,
            alpha_func: func,
            alpha_ref: reference,
            ..state()
        };
        let draws: Vec<Draw> = streams
            .iter()
            .map(|s| Draw::new(SHADER_XYZRHW_DIFFUSE, &l, s, 4, PrimitiveType::QuadList, rs))
            .collect();
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        renderer
            .render(
                &ctx,
                &mut enc,
                &target.target(),
                Clear::all([0.0, 0.0, 0.0, 1.0]),
                &draws,
            )
            .unwrap();
        ctx.queue.submit([enc.finish()]);
        let px = target.read_pixels(&ctx);
        (0..4)
            .map(|i| rgba(&px, 8 * i + 4, 4) != BLACK)
            .collect::<Vec<bool>>()
    };
    assert_eq!(run(CompareFunc::Greater, 0x40), [false, false, true, true]);
    assert_eq!(
        run(CompareFunc::GreaterEqual, 0x40),
        [false, true, true, true]
    );
    assert_eq!(run(CompareFunc::Equal, 0x80), [false, false, true, false]);
    assert_eq!(run(CompareFunc::Less, 0x80), [true, true, false, false]);
    assert_eq!(run(CompareFunc::NotEqual, 0xFF), [true, true, true, false]);
    assert_eq!(run(CompareFunc::Never, 0), [false; 4]);
    assert_eq!(run(CompareFunc::Always, 0xFF), [true; 4]);
}

#[test]
fn partial_and_masked_clears() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let color = store
        .create(
            &ctx,
            SurfaceDesc::new(D3DFormat::A8R8G8B8, W, H),
            SurfaceKind::Color,
        )
        .unwrap();
    let depth = store
        .create(
            &ctx,
            SurfaceDesc::new(D3DFormat::D24S8, W, H),
            SurfaceKind::DepthStencil,
        )
        .unwrap();
    let mut renderer = Renderer::new(&ctx);
    let read = |store: &SurfaceStore| {
        let (px, order) = store.read_color(&ctx, color).unwrap();
        move |x: u32, y: u32| {
            let i = ((y * W + x) * 4) as usize;
            let p = [px[i], px[i + 1], px[i + 2], px[i + 3]];
            match order {
                HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
                _ => p,
            }
        }
    };
    let submit = |enc: wgpu::CommandEncoder| ctx.queue.submit([enc.finish()]);

    // Whole target: blue, depth 1.
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    renderer.clear_rects(
        &ctx,
        &mut enc,
        &store.target(color, Some(depth)).unwrap(),
        MaskedClear {
            depth: Some(1.0),
            stencil: Some(0),
            ..MaskedClear::color([0.0, 0.0, 1.0, 1.0])
        },
        &[ScissorRect::full(W, H)],
    );
    submit(enc);
    assert_eq!(read(&store)(5, 5), [0, 0, 255, 255]);

    // Two rectangles red (one hangs off the target), plus a depth clear to 0.
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    renderer.clear_rects(
        &ctx,
        &mut enc,
        &store.target(color, Some(depth)).unwrap(),
        MaskedClear {
            depth: Some(0.0),
            ..MaskedClear::color([1.0, 0.0, 0.0, 1.0])
        },
        &[
            ScissorRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
            ScissorRect {
                x: 24,
                y: 24,
                width: 100,
                height: 100,
            },
        ],
    );
    submit(enc);
    let px = read(&store);
    assert_eq!(px(0, 0), [255, 0, 0, 255]);
    assert_eq!(px(7, 7), [255, 0, 0, 255]);
    assert_eq!(px(8, 8), [0, 0, 255, 255]);
    assert_eq!(px(31, 31), [255, 0, 0, 255]);

    // Green channel only, on the whole target: red and blue survive.
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    renderer.clear_rects(
        &ctx,
        &mut enc,
        &store.target(color, Some(depth)).unwrap(),
        MaskedClear {
            channels: wgpu::ColorWrites::GREEN,
            ..MaskedClear::color([1.0, 1.0, 1.0, 0.0])
        },
        &[ScissorRect::full(W, H)],
    );
    submit(enc);
    let px = read(&store);
    assert_eq!(px(0, 0), [255, 255, 0, 255]);
    assert_eq!(px(16, 16), [0, 255, 255, 255]);

    // The depth clear only touched its rectangles: a depth-tested quad at
    // z = 0.5 shows where depth is 1, not where it was cleared to 0.
    let l = layout();
    let full = quad(0.0, 0.0, W as f32, H as f32, 0.5, 0xFFFF_FFFF);
    let streams = [VertexStream {
        data: &full,
        stride: 0,
    }];
    let rs = RenderState {
        depth_mode: DepthBufferMode::Z,
        depth_func: CompareFunc::Less,
        ..state()
    };
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    renderer
        .render(
            &ctx,
            &mut enc,
            &store.target(color, Some(depth)).unwrap(),
            Clear::NONE,
            &[Draw::new(
                SHADER_XYZRHW_DIFFUSE,
                &l,
                &streams,
                4,
                PrimitiveType::QuadList,
                rs,
            )],
        )
        .unwrap();
    submit(enc);
    let px = read(&store);
    assert_eq!(px(4, 4), [255, 255, 0, 255], "depth 0 there: hidden");
    assert_eq!(px(16, 16), [255, 255, 255, 255], "depth 1 there: drawn");
}

#[test]
fn render_to_texture_levels() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let tex = store
        .create_texture_target(&ctx, SurfaceDesc::new(D3DFormat::A8R8G8B8, 16, 16), 3)
        .unwrap();
    assert_eq!(store.get(tex).unwrap().mip_levels(), 3);
    let mut renderer = Renderer::new(&ctx);
    let l = layout();
    // Level 0 green (through the ordinary target), level 1 red in its
    // top-left quarter, drawn in the level's own 8x8 window coordinates.
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    renderer.clear_rects(
        &ctx,
        &mut enc,
        &store.target(tex, None).unwrap(),
        MaskedClear::color([0.0, 1.0, 0.0, 1.0]),
        &[ScissorRect::full(16, 16)],
    );
    let small = quad(0.0, 0.0, 4.0, 4.0, 0.5, RED);
    let streams = [VertexStream {
        data: &small,
        stride: 0,
    }];
    let level1 = store.level_target(tex, 1, None).unwrap();
    assert_eq!(level1.size, (8, 8));
    renderer
        .render(
            &ctx,
            &mut enc,
            &level1,
            Clear::all([0.0, 0.0, 0.0, 1.0]),
            &[Draw::new(
                SHADER_XYZRHW_DIFFUSE,
                &l,
                &streams,
                4,
                PrimitiveType::QuadList,
                state(),
            )],
        )
        .unwrap();
    ctx.queue.submit([enc.finish()]);
    assert!(store.level_target(tex, 3, None).is_err());

    let surface = store.get(tex).unwrap();
    let level = |mip: u32, size: u32| -> Vec<u8> {
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * size as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: surface.texture(),
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit([enc.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        slice.get_mapped_range().unwrap().to_vec()
    };
    let bgra = surface.wgpu_format() == wgpu::TextureFormat::Bgra8Unorm;
    let at = |px: &[u8], x: usize, y: usize| {
        let p = &px[y * 256 + 4 * x..y * 256 + 4 * x + 4];
        if bgra {
            [p[2], p[1], p[0], p[3]]
        } else {
            [p[0], p[1], p[2], p[3]]
        }
    };
    let l0 = level(0, 16);
    assert_eq!(at(&l0, 0, 0), [0, 255, 0, 255]);
    assert_eq!(at(&l0, 15, 15), [0, 255, 0, 255]);
    let l1 = level(1, 8);
    assert_eq!(at(&l1, 1, 1), [255, 0, 0, 255]);
    assert_eq!(at(&l1, 6, 6), [0, 0, 0, 255]);

    // The whole chain binds as a texture.
    let mut bindings = ProgramBindings::new(&ctx.device);
    let dims = [Some(wgpu::TextureViewDimension::D2), None, None, None];
    let key = SamplerKey::from_stage(&TextureStageState::default_for_stage(0), ctx.caps);
    let group = bindings.textures(
        &ctx.device,
        &ctx.queue,
        dims,
        [Some(surface.view()), None, None, None],
        [key; 4],
    );
    assert!(group.is_some());
}

#[test]
fn indexed_strips() {
    let Some(ctx) = context() else { return };
    let target = OffscreenTarget::new(&ctx.device, (W, H), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let l = layout();
    // The red left half's corners, then the green right half's.
    let verts: Vec<u8> = [
        vertex(0.0, 0.0, 0.5, RED),
        vertex(16.0, 0.0, 0.5, RED),
        vertex(16.0, 32.0, 0.5, RED),
        vertex(0.0, 32.0, 0.5, RED),
        vertex(16.0, 0.0, 0.5, 0xFF00_FF00),
        vertex(32.0, 0.0, 0.5, 0xFF00_FF00),
        vertex(32.0, 32.0, 0.5, 0xFF00_FF00),
        vertex(16.0, 32.0, 0.5, 0xFF00_FF00),
    ]
    .concat();
    let streams = [VertexStream {
        data: &verts,
        stride: 0,
    }];
    // A triangle strip over the green half's corners in strip order.
    let strip = [4u32, 5, 7, 6];
    let d = Draw {
        indices: Some(&strip),
        ..Draw::new(
            SHADER_XYZRHW_DIFFUSE,
            &l,
            &streams,
            8,
            PrimitiveType::TriangleStrip,
            state(),
        )
    };
    draw_one(&ctx, &target, &mut renderer, d);
    let px = target.read_pixels(&ctx);
    assert_eq!(rgba(&px, 4, 16), BLACK);
    assert_eq!(rgba(&px, 20, 4), [0, 255, 0, 255]);
    assert_eq!(rgba(&px, 30, 30), [0, 255, 0, 255]);

    // A line strip along the red half's top and right edges.
    let lines = [0u32, 1, 2];
    let d = Draw {
        indices: Some(&lines),
        ..Draw::new(
            SHADER_XYZRHW_DIFFUSE,
            &l,
            &streams,
            8,
            PrimitiveType::LineStrip,
            state(),
        )
    };
    draw_one(&ctx, &target, &mut renderer, d);
    let px = target.read_pixels(&ctx);
    let lit = (0..W).flat_map(|x| (0..H).map(move |y| (x, y)));
    assert!(
        lit.clone()
            .any(|(x, y)| rgba(&px, x, y) == [255, 0, 0, 255])
    );
    assert_eq!(rgba(&px, 8, 16), BLACK, "lines, not a filled quad");
    assert_eq!(rgba(&px, 24, 16), BLACK);
}

#[test]
fn two_renders_before_one_submit_keep_their_uniforms() {
    let Some(ctx) = context() else { return };
    let a = OffscreenTarget::new(&ctx.device, (W, H), wgpu::TextureFormat::Bgra8Unorm);
    let b = OffscreenTarget::new(&ctx.device, (W / 2, H / 2), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let l = layout();
    // A 16x16 quad: a quarter of `a`, all of `b`.
    let q = quad(0.0, 0.0, 16.0, 16.0, 0.5, 0x80FF_0000);
    let streams = [VertexStream {
        data: &q,
        stride: 0,
    }];
    let draw = |rs| {
        Draw::new(
            SHADER_XYZRHW_DIFFUSE,
            &l,
            &streams,
            4,
            PrimitiveType::QuadList,
            rs,
        )
    };
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    // `a`: alpha 0x80 passes GREATER 0x40. `b`: it fails GREATER 0xC0.
    let pass_rs = RenderState {
        alpha_test_enable: true,
        alpha_func: CompareFunc::Greater,
        alpha_ref: 0x40,
        ..state()
    };
    let fail_rs = RenderState {
        alpha_ref: 0xC0,
        ..pass_rs
    };
    let clear = Clear::all([0.0, 0.0, 0.0, 1.0]);
    renderer
        .render(&ctx, &mut enc, &a.target(), clear, &[draw(pass_rs)])
        .unwrap();
    renderer
        .render(&ctx, &mut enc, &b.target(), clear, &[draw(fail_rs)])
        .unwrap();
    ctx.queue.submit([enc.finish()]);
    let pa = a.read_pixels(&ctx);
    assert_eq!(
        rgba(&pa, 4, 4),
        [255, 0, 0, 0x80],
        "a keeps its own alpha test"
    );
    assert_eq!(rgba(&pa, 20, 20), BLACK, "a keeps its own target size");
    let pb = b.read_pixels(&ctx);
    assert_eq!(&pb[..4], &[0, 0, 0, 255], "b discards");
}

#[test]
fn builtin_z_uses_the_depth_format_full_scale() {
    use gfx::renderer::{D3DZ_MAX_16, D3DZ_MAX_24, D3DZ_MAX_F16, D3DZ_MAX_F24, d3dz_max};
    assert_eq!(d3dz_max(None), D3DZ_MAX_24);
    assert_eq!(d3dz_max(Some(D3DFormat::LinD24S8)), D3DZ_MAX_24);
    assert_eq!(d3dz_max(Some(D3DFormat::D16)), D3DZ_MAX_16);
    assert_eq!(d3dz_max(Some(D3DFormat::LinF16)), 511.9375);
    assert_eq!(D3DZ_MAX_F16, (2.0 - 2f32.powi(-12)) * 256.0);
    assert_eq!(d3dz_max(Some(D3DFormat::F24S8)), D3DZ_MAX_F24);
    assert!(D3DZ_MAX_F24.is_finite() && D3DZ_MAX_F24 > 3.4e38);

    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let color = store
        .create(
            &ctx,
            SurfaceDesc::new(D3DFormat::LinA8R8G8B8, W, H),
            SurfaceKind::Color,
        )
        .unwrap();
    let mut renderer = Renderer::new(&ctx);
    let l = layout();
    for (format, z) in [(D3DFormat::LinF16, 128.0f32), (D3DFormat::LinD16, 16384.0)] {
        let depth = store
            .create(
                &ctx,
                SurfaceDesc::new(format, W, H),
                SurfaceKind::DepthStencil,
            )
            .unwrap();
        renderer.set_pretransformed_z_max(d3dz_max(Some(format)));
        let mut verts = Vec::new();
        for (x, y) in [
            (0.0f32, 0.0f32),
            (W as f32, 0.0),
            (W as f32, H as f32),
            (0.0, H as f32),
        ] {
            for f in [x, y, z, 1.0] {
                verts.extend_from_slice(&f.to_le_bytes());
            }
            verts.extend_from_slice(&RED.to_le_bytes());
        }
        let streams = [VertexStream {
            data: &verts,
            stride: 0,
        }];
        let rs = RenderState {
            cull_mode: CullMode::None,
            depth_func: CompareFunc::Always,
            ..Default::default()
        };
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        {
            let t = store.target(color, Some(depth)).unwrap();
            let d = Draw::new(
                SHADER_XYZRHW_DIFFUSE,
                &l,
                &streams,
                4,
                PrimitiveType::QuadList,
                rs,
            );
            renderer
                .render(&ctx, &mut enc, &t, Clear::all([0.0; 4]), &[d])
                .unwrap();
        }
        ctx.queue.submit([enc.finish()]);
        let plane = store.read_depth(&ctx, depth).unwrap();
        let got = plane.depth[5 * W as usize + 5];
        let want = z / d3dz_max(Some(format));
        assert!((got - want).abs() < 1e-3, "{format:?}: {got} vs {want}");
    }
}
