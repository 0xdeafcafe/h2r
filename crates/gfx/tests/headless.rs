//! GPU smoke tests. They need an adapter (Metal on macOS CI/dev machines) and
//! skip themselves with a message when none is available.

use gfx::blit::Blitter;
use gfx::pipeline::{ShaderId, ShaderProgram};
use gfx::renderer::{DrawError, SHADER_XYZRHW_DIFFUSE};
use gfx::state::{BlendFactor, CompareFunc, CullMode, RenderState};
use gfx::surface::{CopyRect, SurfaceDesc, SurfaceKind, SurfaceStore};
use gfx::texture::{TextureDesc, TextureKind};
use gfx::vertex::{PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister, fvf};
use gfx::{Clear, D3DFormat, DecodeOptions, Draw, GfxContext, OffscreenTarget, Renderer, VertexStream, decode_texture};

/// A context with a validation error scope open for the whole test: wgpu
/// validation errors surface as a test failure with the wgpu diagnostic
/// instead of an opaque panic from the uncaptured-error handler.
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

fn vertex_z(x: f32, y: f32, z: f32, color: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for f in [x, y, z * 16_777_215.0, 1.0] {
        v.extend_from_slice(&f.to_le_bytes());
    }
    v.extend_from_slice(&color.to_le_bytes());
    v
}

fn vertex(x: f32, y: f32, color: u32) -> Vec<u8> {
    vertex_z(x, y, 0.5, color)
}

fn pixel(px: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

fn draw<'a>(streams: &'a [VertexStream<'a>], count: u32, layout: &'a VertexLayout, prim: PrimitiveType, state: RenderState) -> Draw<'a> {
    Draw::new(SHADER_XYZRHW_DIFFUSE, layout, streams, count, prim, state)
}

/// A clockwise triangle is visible when its winding is not culled and
/// disappears when it is: this exercises cull translation end to end.
#[test]
fn cull_mode_controls_visibility() {
    let Some(ctx) = context() else { return };
    let (w, h) = (64u32, 64u32);
    let target = OffscreenTarget::new(&ctx.device, (w, h), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let verts = [vertex(0.0, 0.0, 0xFFFF0000), vertex(64.0, 0.0, 0xFFFF0000), vertex(0.0, 64.0, 0xFFFF0000)].concat();

    let mut run = |cull: CullMode| {
        let state = RenderState { cull_mode: cull, ..Default::default() };
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        renderer
            .render(
                &ctx,
                &mut enc,
                &target.target(),
                Clear::all([0.0, 0.0, 0.0, 1.0]),
                &[draw(&[VertexStream { data: &verts, stride: 0 }], 3, &layout, PrimitiveType::TriangleList, state)],
            )
            .unwrap();
        ctx.queue.submit([enc.finish()]);
        target.read_pixels(&ctx)
    };

    // BGRA bytes: red is [0, 0, 255, 255].
    let red = [0, 0, 255, 255];
    let black = [0, 0, 0, 255];
    let visible = run(CullMode::Ccw); // culls CCW, triangle is CW -> drawn
    assert_eq!(pixel(&visible, w, 8, 8), red);
    assert_eq!(pixel(&visible, w, 56, 56), black);
    assert_eq!(pixel(&run(CullMode::Cw), w, 8, 8), black);
    assert_eq!(pixel(&run(CullMode::None), w, 8, 8), red);
    assert_eq!(renderer.cache().len(), 3);
}

#[test]
fn quad_list_and_blending() {
    let Some(ctx) = context() else { return };
    let (w, h) = (32u32, 32u32);
    let target = OffscreenTarget::new(&ctx.device, (w, h), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let quad = [
        vertex(0.0, 0.0, 0x80FFFFFF),
        vertex(32.0, 0.0, 0x80FFFFFF),
        vertex(32.0, 32.0, 0x80FFFFFF),
        vertex(0.0, 32.0, 0x80FFFFFF),
    ]
    .concat();
    let state = RenderState {
        cull_mode: CullMode::None,
        blend_enable: true,
        src_blend: BlendFactor::SrcAlpha,
        dst_blend: BlendFactor::InvSrcAlpha,
        ..Default::default()
    };
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    renderer
        .render(
            &ctx,
            &mut enc,
            &target.target(),
            Clear::all([0.0, 0.0, 0.0, 1.0]),
            &[draw(&[VertexStream { data: &quad, stride: 0 }], 4, &layout, PrimitiveType::QuadList, state)],
        )
        .unwrap();
    ctx.queue.submit([enc.finish()]);
    let px = target.read_pixels(&ctx);
    let p = pixel(&px, w, 16, 16);
    for c in &p[..3] {
        assert!((*c as i32 - 128).abs() <= 2, "got {p:?}");
    }
}

#[test]
fn rejects_bad_draws() {
    let Some(ctx) = context() else { return };
    let target = OffscreenTarget::new(&ctx.device, (8, 8), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let mut run = |layout: &VertexLayout, shader: ShaderId, streams: &[VertexStream]| {
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let d = Draw {
            shader,
            streams,
            vertex_count: 3,
            layout,
            primitive: PrimitiveType::TriangleList,
            state: RenderState::default(),
            bind_groups: &[],
            viewport: None,
            scissor: None,
            indices: None,
            count_samples: false,
            window: None,
        };
        renderer.render(&ctx, &mut enc, &target.target(), Clear::NONE, &[d])
    };
    let xyz = VertexLayout::from_fvf(fvf::XYZ | fvf::DIFFUSE).unwrap();
    let s = [VertexStream { data: &[0; 64], stride: 0 }];
    assert_eq!(run(&xyz, SHADER_XYZRHW_DIFFUSE, &s), Err(DrawError::UnsupportedVertexFormat));
    let rhw = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    assert_eq!(run(&rhw, ShaderId(77), &s), Err(DrawError::UnknownShader(ShaderId(77))));
    assert_eq!(run(&rhw, SHADER_XYZRHW_DIFFUSE, &[]), Err(DrawError::StreamCount { given: 0, declared: 1 }));
    assert!(matches!(run(&rhw, SHADER_XYZRHW_DIFFUSE, &[VertexStream { data: &[0; 8], stride: 0 }]), Err(DrawError::Repack(_))));
}

/// Position in stream 0, colour in stream 1 stored as PBYTE4-style bytes
/// would be direct; here the colour stream is a stride-5 D3DCOLOR with a pad
/// byte (not 4-aligned), forcing the repacker to rewrite it.
#[test]
fn multi_stream_with_repack() {
    let Some(ctx) = context() else { return };
    let (w, h) = (32u32, 32u32);
    let target = OffscreenTarget::new(&ctx.device, (w, h), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let layout = VertexLayout {
        streams: vec![
            StreamLayout {
                stream: 0,
                attributes: vec![VertexAttribute { register: VertexRegister::POSITION, data_type: VertexDataType::Float4, offset: 0 }],
                stride: 16,
            },
            StreamLayout {
                stream: 1,
                attributes: vec![VertexAttribute { register: VertexRegister::DIFFUSE, data_type: VertexDataType::D3dColor, offset: 0 }],
                stride: 4,
            },
        ],
        pretransformed: true,
    };
    let pos: Vec<u8> = [(0.0f32, 0.0f32), (32.0, 0.0), (0.0, 32.0)]
        .iter()
        .flat_map(|&(x, y)| [x, y, 0.5, 1.0].into_iter().flat_map(f32::to_le_bytes))
        .collect();
    let mut col = Vec::new();
    for _ in 0..3 {
        col.extend_from_slice(&0xFF00FF00u32.to_le_bytes()); // green
        col.push(0xEE); // padding: guest stride 5
    }
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let state = RenderState { cull_mode: CullMode::None, ..Default::default() };
    renderer
        .render(
            &ctx,
            &mut enc,
            &target.target(),
            Clear::all([0.0, 0.0, 0.0, 1.0]),
            &[draw(
                &[VertexStream { data: &pos, stride: 16 }, VertexStream { data: &col, stride: 5 }],
                3,
                &layout,
                PrimitiveType::TriangleList,
                state,
            )],
        )
        .unwrap();
    ctx.queue.submit([enc.finish()]);
    let px = target.read_pixels(&ctx);
    assert_eq!(pixel(&px, w, 6, 6), [0, 255, 0, 255]);
}

/// A registered shader plugs in by id (the hook for generated programs).
#[test]
fn registered_shader_is_used() {
    let Some(ctx) = context() else { return };
    let target = OffscreenTarget::new(&ctx.device, (16, 16), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    // Ignores vertex colour and paints blue; consumes the same position input.
    let module = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("blue"),
        source: wgpu::ShaderSource::Wgsl(
            "@vertex fn vs_main(@location(0) p: vec4<f32>) -> @builtin(position) vec4<f32> {
                 return vec4<f32>(p.x / 8.0 - 1.0, 1.0 - p.y / 8.0, 0.5, 1.0);
             }
             @fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(0.0, 0.0, 1.0, 1.0); }"
                .into(),
        ),
    });
    let layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    assert!(!renderer.register_shader(
        SHADER_XYZRHW_DIFFUSE,
        ShaderProgram { module: module.clone(), layout: layout.clone(), vs_entry: "vs_main".into(), fs_entry: "fs_main".into() }
    ));
    assert!(renderer.register_shader(
        ShaderId(5),
        ShaderProgram { module, layout, vs_entry: "vs_main".into(), fs_entry: "fs_main".into() }
    ));
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let verts = [vertex(0.0, 0.0, 0), vertex(16.0, 0.0, 0), vertex(0.0, 16.0, 0)].concat();
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let streams = [VertexStream { data: &verts, stride: 0 }];
    let mut d = draw(&streams, 3, &vl, PrimitiveType::TriangleList, RenderState::default());
    d.shader = ShaderId(5);
    renderer.render(&ctx, &mut enc, &target.target(), Clear::all([0.0; 4]), &[d]).unwrap();
    ctx.queue.submit([enc.finish()]);
    assert_eq!(pixel(&target.read_pixels(&ctx), 16, 3, 3), [255, 0, 0, 255]); // BGRA blue
}

/// Decodes and uploads a swizzled DXT1 cube map with mips and a volume;
/// checks that wgpu accepts every copy extent.
#[test]
fn uploads_dxt_cube_and_volume() {
    let Some(ctx) = context() else { return };
    let opts = DecodeOptions { bc_supported: ctx.caps.texture_compression_bc, ..Default::default() };

    let cube = TextureDesc::new(D3DFormat::Dxt1, TextureKind::Cube, 8, 8, 1, 4);
    let data = vec![0x5Au8; cube.layout().unwrap().total_size];
    let tex = decode_texture(&cube, &data, None, opts).unwrap();
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let t = gfx::upload::upload_texture(&ctx.device, &ctx.queue, &tex, "cube");
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "cube upload failed validation: {err:?}");
    assert_eq!(t.depth_or_array_layers(), 6);

    let vol = TextureDesc::new(D3DFormat::A8R8G8B8, TextureKind::Volume, 4, 4, 4, 3);
    let data = vec![0x11u8; vol.layout().unwrap().total_size];
    let tex = decode_texture(&vol, &data, None, opts).unwrap();
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let t = gfx::upload::upload_texture(&ctx.device, &ctx.queue, &tex, "volume");
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "volume upload failed validation: {err:?}");
    assert_eq!(t.dimension(), wgpu::TextureDimension::D3);
}

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

fn tri_draw_state(depth_func: CompareFunc) -> RenderState {
    RenderState { cull_mode: CullMode::None, depth_func, ..Default::default() }
}

/// Render to a colour+depth surface pair, then read both back in guest
/// formats and layouts.
#[test]
fn render_target_and_depth_readback() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let (w, h) = (16u32, 16u32);
    // Linear (pitched) A8R8G8B8 colour, and D16 depth.
    let color = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinA8R8G8B8, w, h), SurfaceKind::Color).unwrap();
    let depth = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinD16, w, h), SurfaceKind::DepthStencil).unwrap();
    // Type checks.
    assert!(store.target(depth, None).is_err());
    assert!(store.target(color, Some(color)).is_err());

    let mut renderer = Renderer::new(&ctx);
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    // Full-target triangle pair as a quad at depth 0.25.
    let quad = [
        vertex_z(0.0, 0.0, 0.25, 0xFF0000FF),
        vertex_z(16.0, 0.0, 0.25, 0xFF0000FF),
        vertex_z(16.0, 16.0, 0.25, 0xFF0000FF),
        vertex_z(0.0, 16.0, 0.25, 0xFF0000FF),
    ]
    .concat();
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let target = store.target(color, Some(depth)).unwrap();
        renderer
            .render(
                &ctx,
                &mut enc,
                &target,
                Clear::all([0.0, 0.0, 0.0, 1.0]),
                &[draw(&[VertexStream { data: &quad, stride: 0 }], 4, &layout, PrimitiveType::QuadList, tri_draw_state(CompareFunc::Always))],
            )
            .unwrap();
    }
    ctx.queue.submit([enc.finish()]);

    let guest = store.read_color_guest(&ctx, color).unwrap();
    let pitch = store.get(color).unwrap().desc().layout.pitch().unwrap() as usize;
    assert_eq!(pitch, 64);
    assert_eq!(guest.len(), pitch * h as usize);
    assert_eq!(&guest[5 * pitch + 5 * 4..][..4], &[0xFF, 0x00, 0x00, 0xFF]); // B,G,R,A = blue

    let plane = store.read_depth(&ctx, depth).unwrap();
    assert_eq!(plane.depth.len(), 256);
    assert!((plane.depth[5 * 16 + 5] - 0.25).abs() < 1e-3, "{}", plane.depth[5 * 16 + 5]);
    let guest_z = store.read_depth_guest(&ctx, depth).unwrap();
    let z = u16::from_le_bytes([guest_z[5 * 64 + 10], guest_z[5 * 64 + 11]]);
    assert!((z as i32 - 16384).abs() <= 2, "{z}");
}

#[test]
fn d24s8_depth_stencil_readback_when_supported() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let color = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinX8R8G8B8, 8, 8), SurfaceKind::Color).unwrap();
    let depth = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinD24S8, 8, 8), SurfaceKind::DepthStencil).unwrap();
    let mut renderer = Renderer::new(&ctx);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let t = store.target(color, Some(depth)).unwrap();
        renderer.render(&ctx, &mut enc, &t, Clear { color: Some([0.0; 4]), depth: Some(0.5), stencil: Some(7) }, &[]).unwrap();
    }
    ctx.queue.submit([enc.finish()]);
    if !ctx.caps.depth32_stencil8 {
        assert!(store.read_depth(&ctx, depth).is_err());
        return;
    }
    let plane = store.read_depth(&ctx, depth).unwrap();
    assert!(plane.depth.iter().all(|d| (d - 0.5).abs() < 1e-6));
    assert_eq!(plane.stencil.as_ref().unwrap()[0], 7);
    let guest = store.read_depth_guest(&ctx, depth).unwrap();
    let word = u32::from_le_bytes(guest[..4].try_into().unwrap());
    assert_eq!(word & 0xFF, 7);
    assert_eq!(word >> 8, 0x800000);
}

#[test]
fn copy_rects_resolve_and_stretch() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let fmt = D3DFormat::LinA8R8G8B8;
    let src = store.create(&ctx, SurfaceDesc::new(fmt, 8, 8).with_samples(4), SurfaceKind::Color).unwrap();
    let dst = store.create(&ctx, SurfaceDesc::new(fmt, 8, 8), SurfaceKind::Color).unwrap();
    let small = store.create(&ctx, SurfaceDesc::new(fmt, 2, 2), SurfaceKind::Color).unwrap();
    let samples = store.get(src).unwrap().desc().samples;

    // Fill the (possibly multisampled) source with solid green.
    let mut renderer = Renderer::new(&ctx);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let t = store.target(src, None).unwrap();
        assert_eq!(t.samples, samples);
        renderer.render(&ctx, &mut enc, &t, Clear { color: Some([0.0, 1.0, 0.0, 1.0]), ..Clear::NONE }, &[]).unwrap();
    }
    // Resolve + copy only the top-left 4x4 into the bottom-right of dst.
    store
        .resolve(&mut enc, src, Some((dst, 4, 4)), Some(CopyRect { x: 0, y: 0, width: 4, height: 4 }))
        .unwrap();
    // Out-of-bounds copies are rejected before touching the encoder.
    assert!(store.copy_rect(&mut enc, src, dst, CopyRect { x: 6, y: 0, width: 4, height: 4 }, 0, 0).is_err());
    // Formats differing in host format need the blit path.
    let rgb = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinR5G6B5, 8, 8), SurfaceKind::Color).unwrap();
    assert!(store.copy_rect(&mut enc, src, rgb, CopyRect { x: 0, y: 0, width: 1, height: 1 }, 0, 0).is_err());
    let mut blitter = Blitter::new(&ctx);
    store.stretch(&ctx, &mut blitter, &mut enc, src, small, None, true).unwrap();
    store.stretch(&ctx, &mut blitter, &mut enc, src, rgb, None, false).unwrap();
    ctx.queue.submit([enc.finish()]);

    let (px, _) = store.read_color(&ctx, dst).unwrap();
    assert_eq!(pixel(&px, 8, 5, 5), [0, 255, 0, 255]); // BGRA green, copied
    assert_eq!(pixel(&px, 8, 1, 1), [0, 0, 0, 0]); // untouched
    let (px, _) = store.read_color(&ctx, small).unwrap();
    assert_eq!(pixel(&px, 2, 1, 1), [0, 255, 0, 255]);
    let guest = store.read_color_guest(&ctx, rgb).unwrap();
    let v = u16::from_le_bytes([guest[0], guest[1]]);
    assert_eq!(v, 0x07E0); // R5G6B5 green
}

#[test]
fn upload_from_guest_roundtrip_swizzled() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let s = store.create(&ctx, SurfaceDesc::new(D3DFormat::A8R8G8B8, 8, 8), SurfaceKind::Color).unwrap();
    assert_eq!(store.get(s).unwrap().desc().layout, gfx::surface::SurfaceLayout::Swizzled);
    let linear: Vec<u8> = (0..64).flat_map(|i| [i as u8, 255 - i as u8, 7, 255]).collect();
    let swz = gfx::swizzle::swizzle(&linear, 8, 8, 1, 4).unwrap();
    store.upload_color_from_guest(&ctx, s, &swz).unwrap();
    assert_eq!(store.read_color_guest(&ctx, s).unwrap(), swz);
}

#[test]
fn depth_bias_on_lines_and_enables_do_not_break_validation() {
    let Some(ctx) = context() else { return };
    let target = OffscreenTarget::new(&ctx.device, (16, 16), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let verts = [vertex(0.0, 0.0, 0xFFFFFFFF), vertex(15.0, 15.0, 0xFFFFFFFF)].concat();
    let state = RenderState {
        depth_bias: -1.0,
        depth_bias_slope_scale: 1.0,
        solid_offset_enable: true,
        wireframe_offset_enable: true,
        point_offset_enable: true,
        ..Default::default()
    };
    for prim in [PrimitiveType::LineList, PrimitiveType::PointList] {
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        renderer
            .render(&ctx, &mut enc, &target.target(), Clear::all([0.0; 4]), &[draw(&[VertexStream { data: &verts, stride: 0 }], 2, &layout, prim, state)])
            .unwrap();
        ctx.queue.submit([enc.finish()]);
    }
}

#[test]
fn depth_surface_larger_than_color_and_odd_uploads() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let color = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 8, 8), SurfaceKind::Color).unwrap();
    let depth = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinD16, 16, 16), SurfaceKind::DepthStencil).unwrap();
    assert!(store.target(color, Some(depth)).is_err()); // needs prepare_target
    let mut renderer = Renderer::new(&ctx);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    store.prepare_target(&ctx, &mut enc, color, depth).unwrap();
    {
        let t = store.target(color, Some(depth)).unwrap();
        assert_eq!(t.size, (8, 8));
        renderer.render(&ctx, &mut enc, &t, Clear { color: Some([0.0, 1.0, 0.0, 1.0]), depth: Some(1.0), stencil: None }, &[]).unwrap();
    }
    store.finish_target(&mut enc, color).unwrap();
    ctx.queue.submit([enc.finish()]);
    let (px, _) = store.read_color(&ctx, color).unwrap();
    assert_eq!(pixel(&px, 8, 7, 7), [0, 255, 0, 255]);

    // Same-surface copies and stretches are rejected.
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    assert!(store.copy_rect(&mut enc, color, color, CopyRect { x: 0, y: 0, width: 1, height: 1 }, 1, 1).is_err());
    assert!(store.copy_rect(&mut enc, color, color, CopyRect { x: u32::MAX, y: 0, width: 2, height: 1 }, 0, 0).is_err());

    // Linear 100x50 (non power of two) and a linear 64x64 round trip.
    for (w, h, fmt) in [(100u32, 50u32, D3DFormat::LinA8R8G8B8), (64, 64, D3DFormat::LinA8R8G8B8)] {
        let s = store.create(&ctx, SurfaceDesc::new(fmt, w, h), SurfaceKind::Color).unwrap();
        let pitch = store.get(s).unwrap().desc().layout.pitch().unwrap() as usize;
        let mut guest = vec![0u8; pitch * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                guest[y * pitch + x * 4..][..4].copy_from_slice(&[x as u8, y as u8, 3, 255]);
            }
        }
        store.upload_color_from_guest(&ctx, s, &guest).unwrap();
        assert_eq!(store.read_color_guest(&ctx, s).unwrap(), guest);
    }
    let s = store
        .create(&ctx, SurfaceDesc::new(D3DFormat::A8R8G8B8, 64, 64).with_layout(gfx::surface::SurfaceLayout::Linear { pitch: 256 }), SurfaceKind::Color)
        .unwrap();
    let guest: Vec<u8> = (0..256 * 64).map(|i| (i % 251) as u8).collect();
    store.upload_color_from_guest(&ctx, s, &guest).unwrap();
    assert_eq!(store.read_color_guest(&ctx, s).unwrap(), guest);
}

#[test]
fn deep_mip_headers_and_dxt_volumes_upload() {
    let Some(ctx) = context() else { return };
    let opts = DecodeOptions { bc_supported: true, ..Default::default() };
    // A 4x4 texture whose header claims 15 mip levels.
    let d = TextureDesc::new(D3DFormat::A8R8G8B8, TextureKind::D2, 4, 4, 1, 15);
    let data = vec![0x33u8; d.layout().unwrap().total_size];
    let tex = decode_texture(&d, &data, None, opts).unwrap();
    gfx::upload::upload_texture(&ctx.device, &ctx.queue, &tex, "deep");
    // DXT volume (CPU decoded).
    let d = TextureDesc::new(D3DFormat::Dxt1, TextureKind::Volume, 8, 8, 4, 2);
    let data = vec![0u8; d.layout().unwrap().total_size];
    let tex = decode_texture(&d, &data, None, opts).unwrap();
    let t = gfx::upload::upload_texture(&ctx.device, &ctx.queue, &tex, "dxt volume");
    assert_eq!(t.dimension(), wgpu::TextureDimension::D3);
}

#[test]
fn write_color_takes_the_guest_pitch_and_layout() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    // A 16x4 render-to-texture level whose guest pitch (128) is wider than
    // the surface's own (64).
    let s = store.create_texture_target(&ctx, SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 16, 4), 1).unwrap();
    let mut desc = TextureDesc::new(D3DFormat::LinA8R8G8B8, TextureKind::D2, 16, 4, 1, 1);
    desc.pitch = 128;
    let mut guest = vec![0xEEu8; 128 * 4];
    for y in 0..4usize {
        for x in 0..16usize {
            guest[y * 128 + x * 4..][..4].copy_from_slice(&[x as u8, y as u8, 7, 255]);
        }
    }
    store.write_color(&ctx, s, &desc, &guest).unwrap();
    let tight: Vec<u8> = (0..4).flat_map(|y| guest[y * 128..y * 128 + 64].to_vec()).collect();
    assert_eq!(store.read_color_guest(&ctx, s).unwrap(), tight);
    // Swizzled guest texels into the same surface.
    let swz_desc = TextureDesc::new(D3DFormat::A8R8G8B8, TextureKind::D2, 16, 4, 1, 1);
    let swz = gfx::swizzle::swizzle(&tight, 16, 4, 1, 4).unwrap();
    store.write_color(&ctx, s, &swz_desc, &swz).unwrap();
    assert_eq!(store.read_color_guest(&ctx, s).unwrap(), tight);
    // Not this surface's size, or compressed: refused.
    let small = TextureDesc::new(D3DFormat::LinA8R8G8B8, TextureKind::D2, 8, 4, 1, 1);
    assert!(store.write_color(&ctx, s, &small, &guest).is_err());
    let dxt = TextureDesc::new(D3DFormat::Dxt1, TextureKind::D2, 16, 4, 1, 1);
    assert!(store.write_color(&ctx, s, &dxt, &guest).is_err());
}

#[test]
fn snapshot_copies_a_surface_into_a_texture_that_can_be_sampled() {
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let fmt = D3DFormat::LinA8R8G8B8;
    let s = store.create(&ctx, SurfaceDesc::new(fmt, 8, 4).with_samples(4), SurfaceKind::Color).unwrap();
    let mut renderer = Renderer::new(&ctx);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let t = store.target(s, None).unwrap();
        renderer.render(&ctx, &mut enc, &t, Clear { color: Some([0.0, 0.0, 1.0, 0.5]), ..Clear::NONE }, &[]).unwrap();
    }
    let copy = store.snapshot(&ctx, &mut enc, s).unwrap();
    // Later rendering into the surface does not reach the snapshot.
    {
        let t = store.target(s, None).unwrap();
        renderer.render(&ctx, &mut enc, &t, Clear { color: Some([1.0, 0.0, 0.0, 1.0]), ..Clear::NONE }, &[]).unwrap();
    }
    ctx.queue.submit([enc.finish()]);
    assert_eq!(copy.format(), store.get(s).unwrap().wgpu_format());
    assert!(copy.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING));
    assert_eq!((copy.width(), copy.height()), (8, 4));
    let px = gfx::readback::read_texture(&ctx, &copy, wgpu::TextureAspect::All, 4, (8, 4));
    assert_eq!(pixel(&px, 8, 7, 3), [255, 0, 0, 128]); // BGRA blue, alpha 0.5
    // Depth surfaces cannot be snapshotted as colour.
    let d = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinD24S8, 8, 4), SurfaceKind::DepthStencil).unwrap();
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    assert!(store.snapshot(&ctx, &mut enc, d).is_err());
}

/// Counted draws add the samples they write: a 16x8 quad is 128, a draw
/// scissored to nothing adds 0, overdraw counts twice, a clear restarts the
/// count, and depth-tested samples that fail are not counted.
#[test]
fn pixel_counter_counts_samples() {
    use gfx::occlusion::PixelCounter;
    use gfx::surface::{SurfaceDesc, SurfaceKind, SurfaceStore};
    use gfx::ScissorRect;
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let color = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 32, 32), SurfaceKind::Color).unwrap();
    let depth = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinD24S8, 32, 32), SurfaceKind::DepthStencil).unwrap();
    let mut renderer = Renderer::new(&ctx);
    let mut counter = PixelCounter::new(&ctx);
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let c = 0xFFFF_FFFF;
    let quad = |z: f32| {
        [
            vertex_z(0.0, 0.0, z, c), vertex_z(16.0, 0.0, z, c), vertex_z(0.0, 8.0, z, c),
            vertex_z(16.0, 0.0, z, c), vertex_z(16.0, 8.0, z, c), vertex_z(0.0, 8.0, z, c),
        ]
        .concat()
    };
    let near = quad(0.25);
    let far = quad(0.75);
    let depth_state = RenderState { depth_func: CompareFunc::LessEqual, ..RenderState::default() };
    // One encoder per step, submitted at once, like the d3d backend.
    let mut step = |f: &mut dyn FnMut(&mut wgpu::CommandEncoder, &mut Renderer, &mut PixelCounter)| {
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        f(&mut enc, &mut renderer, &mut counter);
        ctx.queue.submit([enc.finish()]);
    };
    let draw_one = |enc: &mut wgpu::CommandEncoder,
                    r: &mut Renderer,
                    k: &mut PixelCounter,
                    verts: &[u8],
                    scissor: Option<ScissorRect>,
                    clear: Clear| {
        let streams = [VertexStream { data: verts, stride: 0 }];
        let mut d = draw(&streams, 6, &vl, PrimitiveType::TriangleList, depth_state);
        d.scissor = scissor;
        d.count_samples = true;
        let t = store.target(color, Some(depth)).unwrap();
        r.render_counted(&ctx, enc, &t, clear, &[d], Some(k)).unwrap();
    };
    let clear_all = Clear { color: Some([0.0; 4]), depth: Some(1.0), stencil: Some(0) };
    let mut reports = Vec::new();
    step(&mut |e, r, k| {
        k.clear(e);
        draw_one(e, r, k, &far, None, clear_all);
        reports.push(k.report(&ctx, e));
        // Scissored to nothing: adds 0.
        draw_one(e, r, k, &far, Some(ScissorRect { x: 0, y: 0, width: 0, height: 0 }), Clear::NONE);
        reports.push(k.report(&ctx, e));
        // In front of the first: passes again (overdraw counts).
        draw_one(e, r, k, &near, None, Clear::NONE);
        reports.push(k.report(&ctx, e));
    });
    step(&mut |e, r, k| {
        k.clear(e);
        // Behind what is there now: fails the depth test.
        draw_one(e, r, k, &far, None, Clear::NONE);
        reports.push(k.report(&ctx, e));
        // Half of it, scissored, at the same depth as the front quad.
        draw_one(e, r, k, &near, Some(ScissorRect { x: 0, y: 0, width: 8, height: 8 }), Clear::NONE);
        reports.push(k.report(&ctx, e));
    });
    let got: Vec<u64> = reports.iter().map(|r| r.read(&ctx).unwrap()).collect();
    assert_eq!(got, [128, 128, 256, 0, 64]);

    // A counted draw needs a counter.
    let streams = [VertexStream { data: &near, stride: 0 }];
    let mut d = draw(&streams, 6, &vl, PrimitiveType::TriangleList, RenderState::default());
    d.count_samples = true;
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let t = store.target(color, Some(depth)).unwrap();
    assert_eq!(renderer.render(&ctx, &mut enc, &t, Clear::NONE, &[d]), Err(DrawError::NoPixelCounter));
}

/// A counted draw's own depth and stencil writes reject its later
/// triangles in the count as they do in the real draw, and the real
/// attachment is changed once: two overlapping 16x8 quads in one draw
/// count 128 (256 would mean the count ignored what the draw itself wrote),
/// with depth (near first, then far, `Less`) and with stencil (`Equal 0`,
/// increment on pass), single-sampled and, where supported, at 4x.
#[test]
fn pixel_counter_keeps_depth_and_stencil_changes_within_a_draw() {
    use gfx::occlusion::PixelCounter;
    use gfx::state::{DepthBufferMode, StencilOp, StencilState};
    let Some(ctx) = context() else { return };
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let c = 0xFFFF_FFFF;
    let quad = |z: f32| {
        [
            vertex_z(0.0, 0.0, z, c), vertex_z(16.0, 0.0, z, c), vertex_z(0.0, 8.0, z, c),
            vertex_z(16.0, 0.0, z, c), vertex_z(16.0, 8.0, z, c), vertex_z(0.0, 8.0, z, c),
        ]
        .concat()
    };
    let depth_state = RenderState { depth_func: CompareFunc::Less, depth_write: true, ..RenderState::default() };
    let stencil = |reference: u8, pass: StencilOp| RenderState {
        depth_mode: DepthBufferMode::Disabled,
        depth_write: false,
        stencil: StencilState { enable: true, func: CompareFunc::Equal, reference, pass, ..StencilState::default() },
        ..RenderState::default()
    };
    for samples in [1u32, 4] {
        let mut store = SurfaceStore::new(ctx.caps);
        let mut cdesc = SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 32, 32);
        cdesc.samples = samples;
        let mut ddesc = SurfaceDesc::new(D3DFormat::LinD24S8, 32, 32);
        ddesc.samples = samples;
        let color = store.create(&ctx, cdesc, SurfaceKind::Color).unwrap();
        let depth = store.create(&ctx, ddesc, SurfaceKind::DepthStencil).unwrap();
        let got = store.get(depth).unwrap().desc().samples;
        if got != samples || store.get(color).unwrap().desc().samples != samples {
            eprintln!("skipping {samples}x: the adapter gives {got}x");
            continue;
        }
        let mut renderer = Renderer::new(&ctx);
        let mut counter = PixelCounter::new(&ctx);
        let mut count = |verts: &[u8], state: RenderState, clear: Clear| {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            counter.clear(&mut enc);
            let streams = [VertexStream { data: verts, stride: 0 }];
            let mut d = draw(&streams, (verts.len() / 20) as u32, &vl, PrimitiveType::TriangleList, state);
            d.count_samples = true;
            let t = store.target(color, Some(depth)).unwrap();
            renderer.render_counted(&ctx, &mut enc, &t, clear, &[d], Some(&mut counter)).unwrap();
            let report = counter.report(&ctx, &mut enc);
            ctx.queue.submit([enc.finish()]);
            report.read(&ctx).unwrap() / samples as u64
        };
        let clear_all = Clear { color: Some([0.0; 4]), depth: Some(1.0), stencil: Some(0) };
        let near_then_far = [quad(0.25), quad(0.75)].concat();
        assert_eq!(count(&near_then_far, depth_state, clear_all), 128, "{samples}x depth");
        // The real depth holds the near quad once: a quad between them is
        // behind it, one in front passes.
        assert_eq!(count(&quad(0.5), depth_state, Clear::NONE), 0, "{samples}x");
        assert_eq!(count(&quad(0.1), depth_state, Clear::NONE), 128, "{samples}x");

        let twice = [quad(0.5), quad(0.5)].concat();
        assert_eq!(count(&twice, stencil(0, StencilOp::IncrSat), clear_all), 128, "{samples}x stencil");
        // The real stencil was incremented once (to 1), not twice.
        assert_eq!(count(&quad(0.5), stencil(1, StencilOp::Keep), Clear::NONE), 128, "{samples}x");
        assert_eq!(count(&quad(0.5), stencil(2, StencilOp::Keep), Clear::NONE), 0, "{samples}x");
    }
}

/// A counted draw that writes neither depth nor stencil replays against the
/// real attachment, which then takes the pass's pending clear: the clear is
/// kept for the draws after it, and nothing the replay does changes depth.
#[test]
fn pixel_counter_replays_a_read_only_draw_in_place() {
    use gfx::occlusion::PixelCounter;
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let color = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 32, 32), SurfaceKind::Color).unwrap();
    let depth = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinD24S8, 32, 32), SurfaceKind::DepthStencil).unwrap();
    let mut renderer = Renderer::new(&ctx);
    let mut counter = PixelCounter::new(&ctx);
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let c = 0xFFFF_FFFF;
    let quad = |z: f32| {
        [
            vertex_z(0.0, 0.0, z, c), vertex_z(16.0, 0.0, z, c), vertex_z(0.0, 8.0, z, c),
            vertex_z(16.0, 0.0, z, c), vertex_z(16.0, 8.0, z, c), vertex_z(0.0, 8.0, z, c),
        ]
        .concat()
    };
    let (near, far) = (quad(0.25), quad(0.75));
    let writes = RenderState { depth_func: CompareFunc::LessEqual, depth_write: true, ..RenderState::default() };
    let reads = RenderState { depth_write: false, ..writes };
    let mut run = |draws: &[(&[u8], RenderState, bool)], clear: Clear| {
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        counter.clear(&mut enc);
        let streams: Vec<[VertexStream; 1]> = draws.iter().map(|(v, ..)| [VertexStream { data: v, stride: 0 }]).collect();
        let ds: Vec<Draw> = draws
            .iter()
            .zip(&streams)
            .map(|((_, state, counted), s)| {
                let mut d = draw(s, 6, &vl, PrimitiveType::TriangleList, *state);
                d.count_samples = *counted;
                d
            })
            .collect();
        let t = store.target(color, Some(depth)).unwrap();
        renderer.render_counted(&ctx, &mut enc, &t, clear, &ds, Some(&mut counter)).unwrap();
        let report = counter.report(&ctx, &mut enc);
        report.submitted(ctx.queue.submit([enc.finish()]));
        report.read(&ctx).unwrap()
    };
    let clear_all = Clear { color: Some([0.0; 4]), depth: Some(1.0), stencil: Some(0) };
    // Depth holds the near quad.
    assert_eq!(run(&[(&near, writes, false)], clear_all), 0);
    assert_eq!(run(&[(&far, reads, true)], Clear::NONE), 0, "behind the near quad");
    // The clear goes to the replay's attachment and stays for the draw
    // after it: the far quad passes, then writes the depth the near test
    // compares against.
    assert_eq!(run(&[(&far, reads, true), (&far, writes, false), (&near, reads, true)], clear_all), 256);
    // The replay wrote nothing; the real draw wrote the far quad.
    assert_eq!(run(&[(&far, reads, true)], Clear::NONE), 128);
    assert_eq!(run(&[(&quad(0.9), reads, true)], Clear::NONE), 0);
}

/// Replacing a registered shader also replaces what the count pass draws:
/// a replacement that moves the quad off the target counts 0.
#[test]
fn pixel_counter_follows_a_replaced_shader() {
    use gfx::occlusion::PixelCounter;
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let color = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 32, 32), SurfaceKind::Color).unwrap();
    let depth = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinD24S8, 32, 32), SurfaceKind::DepthStencil).unwrap();
    let mut renderer = Renderer::new(&ctx);
    let mut counter = PixelCounter::new(&ctx);
    let program = |x_offset: f32| {
        let module = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("count replace"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "@vertex fn vs_main(@location(0) p: vec4<f32>) -> @builtin(position) vec4<f32> {{
                         return vec4<f32>(p.x / 16.0 - 1.0 + {x_offset:.1}, 1.0 - p.y / 16.0, 0.5, 1.0);
                     }}
                     @fragment fn fs_main() -> @location(0) vec4<f32> {{ return vec4<f32>(1.0); }}"
                )
                .into(),
            ),
        });
        let layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        ShaderProgram { module, layout, vs_entry: "vs_main".into(), fs_entry: "fs_main".into() }
    };
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let c = 0xFFFF_FFFF;
    let verts = [
        vertex(0.0, 0.0, c), vertex(16.0, 0.0, c), vertex(0.0, 8.0, c),
        vertex(16.0, 0.0, c), vertex(16.0, 8.0, c), vertex(0.0, 8.0, c),
    ]
    .concat();
    let mut count = |renderer: &mut Renderer| {
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        counter.clear(&mut enc);
        let streams = [VertexStream { data: &verts, stride: 0 }];
        let mut d = draw(&streams, 6, &vl, PrimitiveType::TriangleList, RenderState::default());
        d.shader = ShaderId(99);
        d.count_samples = true;
        let t = store.target(color, Some(depth)).unwrap();
        let clear = Clear { color: Some([0.0; 4]), depth: Some(1.0), stencil: Some(0) };
        renderer.render_counted(&ctx, &mut enc, &t, clear, &[d], Some(&mut counter)).unwrap();
        let report = counter.report(&ctx, &mut enc);
        ctx.queue.submit([enc.finish()]);
        report.read(&ctx).unwrap()
    };
    assert!(renderer.register_shader(ShaderId(99), program(0.0)));
    assert_eq!(count(&mut renderer), 128);
    assert!(renderer.register_shader(ShaderId(99), program(5.0)));
    assert_eq!(count(&mut renderer), 0, "the count pass still used the replaced shader");
}

/// A counted draw that writes depth copies the depth attachment, so one
/// that cannot be copied is refused before anything is recorded.
#[test]
fn pixel_counter_needs_a_copyable_depth() {
    use gfx::occlusion::PixelCounter;
    let Some(ctx) = context() else { return };
    // OffscreenTarget's depth is a render attachment only.
    let target = OffscreenTarget::new(&ctx.device, (16, 16), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let mut counter = PixelCounter::new(&ctx);
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let verts = [vertex(0.0, 0.0, 0), vertex(16.0, 0.0, 0), vertex(0.0, 16.0, 0)].concat();
    let streams = [VertexStream { data: &verts, stride: 0 }];
    let mut d = draw(&streams, 3, &vl, PrimitiveType::TriangleList, RenderState::default());
    d.count_samples = true;
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    assert_eq!(
        renderer.render_counted(&ctx, &mut enc, &target.target(), Clear::NONE, &[d], Some(&mut counter)),
        Err(DrawError::CountDepthNotCopyable)
    );
}

/// A counted draw that only reads depth replays in place and copies
/// nothing, so a depth texture without `COPY_SRC` is fine: a 16x16 quad in
/// front of the cleared depth counts 256.
#[test]
fn pixel_counter_reads_a_non_copyable_depth() {
    use gfx::occlusion::PixelCounter;
    let Some(ctx) = context() else { return };
    // OffscreenTarget's depth is a render attachment only.
    let target = OffscreenTarget::new(&ctx.device, (16, 16), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let mut counter = PixelCounter::new(&ctx);
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let verts = [
        vertex(0.0, 0.0, 0), vertex(16.0, 0.0, 0), vertex(0.0, 16.0, 0),
        vertex(16.0, 0.0, 0), vertex(16.0, 16.0, 0), vertex(0.0, 16.0, 0),
    ]
    .concat();
    let streams = [VertexStream { data: &verts, stride: 0 }];
    let reads = RenderState { depth_write: false, ..RenderState::default() };
    let mut d = draw(&streams, 6, &vl, PrimitiveType::TriangleList, reads);
    d.count_samples = true;
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    counter.clear(&mut enc);
    let clear = Clear { color: Some([0.0; 4]), depth: Some(1.0), stencil: Some(0) };
    renderer.render_counted(&ctx, &mut enc, &target.target(), clear, &[d], Some(&mut counter)).unwrap();
    let report = counter.report(&ctx, &mut enc);
    report.submitted(ctx.queue.submit([enc.finish()]));
    assert_eq!(report.read(&ctx).unwrap(), 256);
}

/// Reports read long after their submission, and out of order, still give
/// their own counts: each waits for its own submission and keeps its value
/// while later submissions clear the counter and count again.
#[test]
fn pixel_reports_read_after_later_submissions() {
    use gfx::occlusion::PixelCounter;
    let Some(ctx) = context() else { return };
    let mut store = SurfaceStore::new(ctx.caps);
    let color = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 32, 32), SurfaceKind::Color).unwrap();
    let depth = store.create(&ctx, SurfaceDesc::new(D3DFormat::LinD24S8, 32, 32), SurfaceKind::DepthStencil).unwrap();
    let mut renderer = Renderer::new(&ctx);
    let mut counter = PixelCounter::new(&ctx);
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let c = 0xFFFF_FFFF;
    // A w x 8 quad: 8w samples.
    let quad = |w: f32| {
        [
            vertex(0.0, 0.0, c), vertex(w, 0.0, c), vertex(0.0, 8.0, c),
            vertex(w, 0.0, c), vertex(w, 8.0, c), vertex(0.0, 8.0, c),
        ]
        .concat()
    };
    let reads = RenderState { depth_write: false, ..RenderState::default() };
    let clear_all = Clear { color: Some([0.0; 4]), depth: Some(1.0), stencil: Some(0) };
    let mut reports = Vec::new();
    for w in [4.0, 8.0, 16.0, 32.0] {
        let verts = quad(w);
        let streams = [VertexStream { data: &verts, stride: 0 }];
        let mut d = draw(&streams, 6, &vl, PrimitiveType::TriangleList, reads);
        d.count_samples = true;
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        counter.clear(&mut enc);
        let t = store.target(color, Some(depth)).unwrap();
        renderer.render_counted(&ctx, &mut enc, &t, clear_all, &[d], Some(&mut counter)).unwrap();
        let report = counter.report(&ctx, &mut enc);
        report.submitted(ctx.queue.submit([enc.finish()]));
        reports.push(report);
    }
    // Last first, then the first, then the rest.
    assert_eq!(reports[3].read(&ctx).unwrap(), 256);
    assert_eq!(reports[0].read(&ctx).unwrap(), 32);
    assert_eq!(reports[2].read(&ctx).unwrap(), 128);
    assert_eq!(reports[1].read(&ctx).unwrap(), 64);
}

/// One render call with more draws than the built-in uniform ring holds
/// (1 MiB at the 256-byte offset alignment Metal and most adapters
/// have) and one vertex stream bigger than
/// the vertex ring (4 MiB): both rings move to new buffers mid-pass, and
/// every draw keeps its own uniforms and vertices. The alpha reference
/// travels in the uniforms: even draws pass the test, odd ones fail, so a
/// draw reading another's uniforms breaks the checkerboard.
#[test]
fn upload_rings_roll_over_within_one_pass() {
    let Some(ctx) = context() else { return };
    let (w, h) = (64u32, 96u32);
    let target = OffscreenTarget::new(&ctx.device, (w, h), wgpu::TextureFormat::Bgra8Unorm);
    let mut renderer = Renderer::new(&ctx);
    let vl = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let white = 0x80FF_FFFF;
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| {
        [
            vertex(x0, y0, white), vertex(x1, y0, white), vertex(x0, y1, white),
            vertex(x1, y0, white), vertex(x1, y1, white), vertex(x0, y1, white),
        ]
        .concat()
    };
    // 5120 one-pixel quads over rows 0..80.
    let cells: Vec<Vec<u8>> = (0..64 * 80)
        .map(|i| {
            let (x, y) = ((i % 64) as f32, (i / 64) as f32);
            rect(x, y, x + 1.0, y + 1.0)
        })
        .collect();
    let state = |i: usize| RenderState {
        cull_mode: CullMode::None,
        alpha_test_enable: true,
        alpha_func: CompareFunc::Greater,
        alpha_ref: if i.is_multiple_of(2) { 0x10 } else { 0xF0 },
        ..RenderState::default()
    };
    // Rows 80..88 at the end of a 5.2 MB stream of degenerate triangles,
    // then rows 88..96 from a small draw after it.
    let mut big = vertex(0.0, 0.0, 0).repeat(3 * 87_379);
    big.extend(rect(0.0, 80.0, 64.0, 88.0));
    let after = rect(0.0, 88.0, 64.0, 96.0);
    let cell_streams: Vec<[VertexStream; 1]> = cells.iter().map(|v| [VertexStream { data: v, stride: 0 }]).collect();
    let big_stream = [VertexStream { data: &big, stride: 0 }];
    let after_stream = [VertexStream { data: &after, stride: 0 }];
    let mut draws: Vec<Draw> = cell_streams
        .iter()
        .enumerate()
        .map(|(i, s)| draw(s, 6, &vl, PrimitiveType::TriangleList, state(i)))
        .collect();
    draws.push(draw(&big_stream, (big.len() / 20) as u32, &vl, PrimitiveType::TriangleList, state(0)));
    draws.push(draw(&after_stream, 6, &vl, PrimitiveType::TriangleList, state(0)));
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    renderer.render(&ctx, &mut enc, &target.target(), Clear::all([0.0, 0.0, 0.0, 1.0]), &draws).unwrap();
    ctx.queue.submit([enc.finish()]);
    let px = target.read_pixels(&ctx);
    let bad: Vec<(u32, [u8; 4])> = (0..64 * 80u32)
        .filter_map(|i| {
            let want = if i.is_multiple_of(2) { 0xFF } else { 0 };
            let p = pixel(&px, w, i % 64, i / 64);
            (p[0] != want).then_some((i, p))
        })
        .collect();
    assert!(bad.is_empty(), "{} cells wrong: {:?}", bad.len(), &bad[..bad.len().min(40)]);
    for y in [80, 87, 88, 95] {
        assert_eq!(pixel(&px, w, 5, y)[0], 0xFF, "row {y}");
    }
}

/// Texture groups are cached by view and sampler: the same views give the
/// same group, a replaced view a new one, and a group unused for more than
/// four frames is dropped (asking again builds a new one).
#[test]
fn texture_groups_are_replaced_and_evicted() {
    use gfx::program::ProgramBindings;
    use gfx::translate::SamplerKey;
    let Some(ctx) = context() else { return };
    let mut b = ProgramBindings::new(&ctx.device);
    let view = |fill: u8| {
        let desc = TextureDesc::new(D3DFormat::LinA8R8G8B8, TextureKind::D2, 4, 4, 1, 1);
        let tex = decode_texture(&desc, &[fill; 64], None, DecodeOptions::default()).unwrap();
        gfx::upload::upload_texture(&ctx.device, &ctx.queue, &tex, "group").create_view(&Default::default())
    };
    let (a, other) = (view(1), view(2));
    let dims = [Some(wgpu::TextureViewDimension::D2), None, None, None];
    let sampler = SamplerKey::from_stage(&gfx::state::TextureStageState::default_for_stage(0), ctx.caps);
    let group = |b: &mut ProgramBindings, v: &wgpu::TextureView| {
        b.textures(&ctx.device, &ctx.queue, dims, [Some(v), None, None, None], [sampler; 4], [[false; 4]; 4])
            .unwrap()
    };
    let first = group(&mut b, &a);
    assert_eq!(group(&mut b, &a), first, "cached");
    let replaced = group(&mut b, &other);
    assert_ne!(replaced, first, "a new view, a new group");
    // Used again within four frames: kept.
    for _ in 0..4 {
        b.end_frame();
    }
    assert_eq!(group(&mut b, &a), first);
    // Unused for five: dropped, so the same views build a new group.
    for _ in 0..5 {
        b.end_frame();
    }
    assert_ne!(group(&mut b, &a), first, "evicted");
    assert_ne!(group(&mut b, &other), replaced, "evicted");
}

/// Pipelines are built off the drawing thread: a draw-time miss is queued
/// on a compile worker and collected when the pass is encoded (output as
/// if built inline), and a prewarmed pipeline is never built by the
/// drawing thread at all. Without workers a miss is built at encode, on the
/// caller.
#[test]
fn pipelines_compile_off_the_drawing_thread() {
    use gfx::compile::Compiler;
    use std::time::{Duration, Instant};
    let Some(ctx) = context() else { return };
    let (w, h) = (16u32, 16u32);
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let verts = [vertex(0.0, 0.0, 0xFF00FF00), vertex(16.0, 0.0, 0xFF00FF00), vertex(0.0, 16.0, 0xFF00FF00)].concat();
    let state = RenderState { cull_mode: CullMode::None, ..Default::default() };
    let me = std::thread::current().id();
    let run = |renderer: &mut Renderer| {
        let target = OffscreenTarget::new(&ctx.device, (w, h), wgpu::TextureFormat::Bgra8Unorm);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        let streams = [VertexStream { data: &verts, stride: 0 }];
        let d = draw(&streams, 3, &layout, PrimitiveType::TriangleList, state);
        let recorded = renderer.record(&ctx, &target.target(), &d).unwrap();
        renderer.flush_uploads(&ctx.queue);
        Renderer::encode_pass(&ctx, &mut enc, &target.target(), Clear::all([0.0, 0.0, 0.0, 1.0]), &[recorded], None)
            .unwrap();
        ctx.queue.submit([enc.finish()]);
        target.read_pixels(&ctx)
    };
    let only = |r: &Renderer| {
        assert_eq!(r.cache().len(), 1);
        r.cache().iter().next().map(|(k, p)| (k.clone(), p.clone())).unwrap()
    };

    // No workers: the miss is not built when recorded, but at encode, here.
    let mut inline = Renderer::new(&ctx);
    inline.set_compiler(Compiler::new(0));
    let want = run(&mut inline);
    assert_eq!(pixel(&want, w, 2, 2), [0, 255, 0, 255]);
    let (key, p) = only(&inline);
    assert_eq!(p.built_on(), Some(me));
    assert_eq!(inline.take_new_keys(), vec![(key.clone(), false)]);
    assert!(inline.take_new_keys().is_empty());

    // Prewarmed: built on a worker before the draw, so the draw only
    // finds it; it is not a new key.
    let mut warm = Renderer::new(&ctx);
    warm.set_compiler(Compiler::new(2));
    assert!(warm.prewarm(&ctx, &key, false));
    assert!(!warm.prewarm(&ctx, &key, false), "already queued");
    let (_, p) = only(&warm);
    let start = Instant::now();
    while !p.is_ready() {
        assert!(start.elapsed() < Duration::from_secs(30), "prewarm never finished");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(run(&mut warm), want);
    assert!(p.built_on().is_some_and(|t| t != me));
    assert!(warm.take_new_keys().is_empty());
    assert_eq!((warm.cache().hits(), warm.cache().misses()), (1, 0));

    // A miss with workers: queued at record, same pixels.
    let mut miss = Renderer::new(&ctx);
    miss.set_compiler(Compiler::new(1));
    assert_eq!(run(&mut miss), want);
    assert_eq!(miss.take_new_keys(), vec![(key, false)]);

    // An unregistered shader cannot be prewarmed.
    let mut other = only(&inline).0;
    other.shader = ShaderId(99);
    assert!(!warm.prewarm(&ctx, &other, false));
}
