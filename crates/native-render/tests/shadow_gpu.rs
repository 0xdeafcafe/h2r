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
