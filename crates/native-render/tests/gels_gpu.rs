#![cfg(feature = "gpu")]
//! Synthetic shader target-13 production and the resident program cache.
//! No original shader code, bitmap or disc data is used.

use native_render::gels::Key;
use native_render::gels_gpu::{Frame, Producer, Program, quad};
use native_render::pass_state::PassState;
use shaders::{psh::PixelUniforms, vsh::VertexUniforms};

fn program(color: &str) -> Program {
    let rs = gfx::state::RenderState {
        cull_mode: gfx::state::CullMode::None,
        ..Default::default()
    };
    let stage = gfx::state::TextureStageState::default_for_stage(0);
    let caps = gfx::translate::Capabilities::from_features(wgpu::Features::empty());
    Program {
        wgsl: format!(
            r#"
            @vertex fn vs_main(@location(0) p: vec4f) -> @builtin(position) vec4f {{
                return vec4f(p.x / 2.0 - 1.0, 1.0 - p.y / 2.0, 0.0, 1.0);
            }}
            @fragment fn fs_main() -> @location(0) vec4f {{ return {color}; }}
        "#
        ),
        dims: [None; 4],
        samplers: [gfx::translate::SamplerKey::from_stage(&stage, caps); 4],
        state: PassState {
            blend: None,
            blend_constant: wgpu::Color::TRANSPARENT,
            color_writes: wgpu::ColorWrites::ALL,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Always,
            primitive: gfx::translate::primitive_state(
                &rs,
                wgpu::PrimitiveTopology::TriangleStrip,
                caps,
            ),
            alpha_test: None,
        },
    }
}

fn read(ctx: &gfx::GfxContext, output: &native_render::gels_gpu::Output) -> Vec<u8> {
    let texture = output.view.texture();
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(4),
            },
        },
        texture.size(),
    );
    ctx.queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let data = buffer.slice(..).get_mapped_range().unwrap();
    data.chunks_exact(256)
        .flat_map(|row| row[..16].iter().copied())
        .collect()
}

#[test]
fn replacing_loads_the_live_target_and_reuses_only_the_matching_generation() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let cache = native_render::gels_gpu::ResidentCache::default();
    let masked = || {
        let mut p = program("vec4f(1.0,0.0,0.0,1.0)");
        p.state.color_writes = wgpu::ColorWrites::RED;
        p
    };
    let a = cache.producer(&ctx.device, 1, masked(), [None; 4]).unwrap();
    let same = cache.producer(&ctx.device, 1, masked(), [None; 4]).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &same));
    let another_map = cache.producer(&ctx.device, 2, masked(), [None; 4]).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&a, &another_map));
    let green = Producer::new(
        &ctx.device,
        &ctx.queue,
        program("vec4f(0.0,1.0,0.0,1.0)"),
        &Default::default(),
    )
    .unwrap();
    let frame = Frame {
        vertex: VertexUniforms::default(),
        pixel: PixelUniforms::default(),
        vertices: quad((4, 4), [0.0, 1.0]),
    };
    let key = Key {
        sequence: 1,
        definition: 2,
        shader: 3,
        surface: 4,
    };
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let target = green
        .encode(
            &ctx.device,
            &mut encoder,
            key,
            &frame,
            wgpu::TextureFormat::Rgba8Unorm,
            (4, 4),
            (4, 4),
        )
        .unwrap();
    a.encode_into(
        &ctx.device,
        &mut encoder,
        &frame,
        &target.view,
        wgpu::TextureFormat::Rgba8Unorm,
        (4, 4),
        (4, 4),
    )
    .unwrap();
    ctx.queue.submit([encoder.finish()]);
    assert_eq!(read(&ctx, &target), [255, 255, 0, 255].repeat(16));
}

/// The gel as a packet in the D3D pass already open on the live target
/// (`Producer::packet`, its bytes in the submission's arena) draws what
/// its own pass (`Producer::encode_into`) draws: the loaded pixels, the
/// blend with its constant and the colour mask.
#[test]
fn a_gel_packet_in_the_open_pass_draws_what_its_own_pass_draws() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut blended = program("vec4f(0.5,0.25,1.0,1.0)");
    blended.state.blend = Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::REPLACE,
    });
    blended.state.blend_constant = wgpu::Color {
        r: 0.25,
        g: 0.5,
        b: 0.75,
        a: 1.0,
    };
    blended.state.color_writes =
        wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN | wgpu::ColorWrites::ALPHA;
    let cache = native_render::gels_gpu::ResidentCache::default();
    let producer = cache.producer(&ctx.device, 1, blended, [None; 4]).unwrap();
    let frame = Frame {
        vertex: VertexUniforms::default(),
        pixel: PixelUniforms::default(),
        vertices: quad((4, 4), [0.0, 1.0]),
    };
    let original = [10u8, 20, 30, 40];
    let mut images = Vec::new();
    for packet in [true, false] {
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("live target 13"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            texture.as_image_copy(),
            &original.repeat(16),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(16),
                rows_per_image: Some(4),
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        let arena = gfx::ring::UploadPool::default().begin();
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        if packet {
            let draw = producer
                .packet(&ctx.device, &ctx.queue, &arena, &frame, format, (4, 4), (4, 4))
                .unwrap();
            let target = gfx::RenderTarget {
                color: &view,
                color_format: format,
                depth: None,
                depth_format: None,
                size: (4, 4),
                samples: 1,
            };
            gfx::Renderer::encode_pass(&ctx, &mut encoder, &target, gfx::Clear::NONE, &[draw], None)
                .unwrap();
        } else {
            producer
                .encode_into(&ctx.device, &mut encoder, &frame, &view, format, (4, 4), (4, 4))
                .unwrap();
        }
        arena.flush(&ctx.queue);
        ctx.queue.submit([encoder.finish()]);
        images.push(gfx::readback::read_texture(
            &ctx,
            &texture,
            wgpu::TextureAspect::All,
            4,
            (4, 4),
        ));
    }
    assert_eq!(images[0], images[1]);
    assert_ne!(&images[0][..4], &original[..], "the gel drew");
    assert_eq!(images[0][2], original[2], "blue is masked");
}
