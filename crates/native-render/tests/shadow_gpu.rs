#![cfg(feature = "gpu")]

use gfx::renderer::SHADER_XYZRHW_DIFFUSE;
use gfx::state::{CullMode, RenderState};
use gfx::vertex::{PrimitiveType, VertexLayout, fvf};
use gfx::{Clear, Draw, GfxContext, OffscreenTarget, Renderer, VertexStream};
use native_render::shadow_gpu::{MapKind, Producer};

#[test]
fn object_mask_keeps_the_caster_and_clear_in_separate_generations() {
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    let size = (16, 16);
    let surface = OffscreenTarget::new(&ctx.device, size, wgpu::TextureFormat::Bgra8Unorm);
    let mut target = surface.target();
    target.depth = None;
    target.depth_format = None;
    let mut renderer = Renderer::new(&ctx);
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let mut verts = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (16.0, 0.0), (0.0, 16.0)] {
        for f in [x, y, 0.5, 1.0] {
            verts.extend_from_slice(&f.to_le_bytes());
        }
        verts.extend_from_slice(&0xff000000u32.to_le_bytes());
    }
    let streams = [VertexStream {
        data: &verts,
        stride: 0,
    }];
    let draw = Draw::new(
        SHADER_XYZRHW_DIFFUSE,
        &layout,
        &streams,
        3,
        PrimitiveType::TriangleList,
        RenderState {
            cull_mode: CullMode::None,
            ..Default::default()
        },
    );
    let packets = [renderer.record(&ctx, &target, &draw).unwrap()];
    renderer.flush_uploads(&ctx.queue);
    let bits = gfx::depth_bits::DepthBits::new(&ctx);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let [cast, empty] = [packets.as_slice(), &[]].map(|batch| {
        let mut p = Producer::new(&ctx.device, &mut enc, &target, MapKind::ColourMask).unwrap();
        p.record(&ctx, &mut enc, Clear::NONE, batch).unwrap();
        p.finish(&ctx, &mut enc, &bits).unwrap()
    });
    ctx.queue.submit([enc.finish()]);
    let pixels = |map: &native_render::shadow_gpu::SampledMap| {
        gfx::readback::read_texture(
            &ctx,
            map.view.texture(),
            wgpu::TextureAspect::All,
            4,
            map.size,
        )
    };
    let cast = pixels(&cast);
    assert_eq!(&cast[(2 * 16 + 2) * 4..][..4], [0, 0, 0, 255]);
    assert_eq!(&cast[(14 * 16 + 14) * 4..][..4], [255, 255, 255, 0]);
    assert!(
        pixels(&empty)
            .chunks_exact(4)
            .all(|p| p == [255, 255, 255, 0])
    );
}

#[test]
fn d24_map_preserves_caster_depth_stencil_and_the_untouched_clear() {
    use gfx::{
        RenderTarget,
        state::{CompareFunc, DepthBufferMode, StencilOp, StencilState},
    };
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    if !ctx.caps.depth32_stencil8 {
        return;
    }
    let size = (16, 16);
    let surface = OffscreenTarget::new(&ctx.device, size, wgpu::TextureFormat::Rgba8Unorm);
    let format = wgpu::TextureFormat::Depth32FloatStencil8;
    let depth = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: surface.texture.size(),
        dimension: wgpu::TextureDimension::D2,
        format,
        sample_count: 1,
        mip_level_count: 1,
        view_formats: &[],
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
    });
    let depth_view = depth.create_view(&Default::default());
    let target = RenderTarget {
        depth: Some(&depth_view),
        depth_format: Some(format),
        ..surface.target()
    };
    let mut renderer = Renderer::new(&ctx);
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let mut vertices = Vec::new();
    for (x, y) in [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)] {
        for f in [x, y, 8_388_608.0f32, 1.0] {
            vertices.extend_from_slice(&f.to_le_bytes());
        }
        vertices.extend_from_slice(&u32::MAX.to_le_bytes());
    }
    let streams = [VertexStream {
        data: &vertices,
        stride: 0,
    }];
    let rs = RenderState {
        cull_mode: CullMode::None,
        depth_mode: DepthBufferMode::Z,
        depth_write: true,
        depth_func: CompareFunc::Always,
        stencil: StencilState {
            enable: true,
            reference: 0x53,
            write_mask: 0xf0,
            pass: StencilOp::Replace,
            ..Default::default()
        },
        ..Default::default()
    };
    let draw = Draw::new(
        SHADER_XYZRHW_DIFFUSE,
        &layout,
        &streams,
        3,
        PrimitiveType::TriangleList,
        rs,
    );
    let packets = [renderer.record(&ctx, &target, &draw).unwrap()];
    renderer.flush_uploads(&ctx.queue);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let mut p = Producer::new(&ctx.device, &mut enc, &target, MapKind::Depth).unwrap();
    let clear = Clear {
        color: None,
        depth: Some(1.0),
        stencil: Some(5),
    };
    p.record(&ctx, &mut enc, clear, &packets).unwrap();
    let bits = gfx::depth_bits::DepthBits::new(&ctx);
    let native = p.finish(&ctx, &mut enc, &bits).unwrap();
    Renderer::encode_pass(&ctx, &mut enc, &target, clear, &packets, None).unwrap();
    let original = bits.convert(&ctx, &mut enc, &depth, size);
    ctx.queue.submit([enc.finish()]);
    let read = |t| gfx::readback::read_texture(&ctx, t, wgpu::TextureAspect::All, 4, size);
    let pixels = read(native.view.texture());
    assert_eq!(pixels, read(&original));
    let inside = &pixels[(2 * 16 + 2) * 4..][..4];
    let outside = &pixels[(14 * 16 + 14) * 4..][..4];
    assert_eq!(inside[3], 0x55);
    assert_ne!(&inside[..3], &[255; 3]);
    assert_eq!(outside, [255, 255, 255, 5]);
}
