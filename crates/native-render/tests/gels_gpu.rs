#![cfg(feature = "gpu")]
//! Synthetic shader target-13 production, ticket lifetime and copy
//! identity. No original shader code, bitmap or disc data is used.

use native_render::gels::Key;
use native_render::gels_gpu::{Frame, Producer, Program, Ticket, quad};
use native_render::pass_state::PassState;
use shaders::{psh::PixelUniforms, vsh::VertexUniforms};

fn program(color: &str) -> Program {
    let rs = gfx::state::RenderState {
        cull_mode: gfx::state::CullMode::None,
        ..Default::default()
    };
    let stage = gfx::state::TextureStageState::default();
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
fn target_thirteen_is_produced_and_a_later_light_cannot_overwrite_it() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let key = Key {
        sequence: 1,
        definition: 2,
        shader: 3,
        surface: 4,
    };
    let ticket = Ticket::new(key);
    let frame = Frame {
        vertex: VertexUniforms::default(),
        pixel: PixelUniforms::default(),
        vertices: quad((4, 4), [0.0, 1.0]),
    };
    let red = Producer::new(
        &ctx.device,
        &ctx.queue,
        program("vec4f(1.0,0.0,0.0,1.0)"),
        &Default::default(),
    )
    .unwrap();
    let green = Producer::new(
        &ctx.device,
        &ctx.queue,
        program("vec4f(0.0,1.0,0.0,1.0)"),
        &Default::default(),
    )
    .unwrap();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let first = red
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
    let second = green
        .encode(
            &ctx.device,
            &mut encoder,
            Key { sequence: 2, ..key },
            &frame,
            wgpu::TextureFormat::Rgba8Unorm,
            (4, 4),
            (4, 4),
        )
        .unwrap();
    ticket.publish(second.clone()); // Another invocation cannot publish to this ticket.
    assert!(ticket.output(key, (4, 4), (4, 4)).is_none());
    ticket.publish(first);
    let no_copy_source = ctx
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("comparison source without COPY_SRC"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let published = ticket.output(key, (4, 4), (4, 4)).unwrap();
    assert!(
        native_render::gels_gpu::compare_on_submit(
            &ctx.device,
            &mut encoder,
            &published,
            &no_copy_source
        )
        .is_err()
    );
    assert!(
        ticket.output(key, (4, 4), (4, 4)).is_some(),
        "comparison failure does not retract production"
    );
    assert!(ticket.output(key, (4, 4), (8, 8)).is_none());
    ctx.queue.submit([encoder.finish()]);
    let retained = ticket.output(key, (4, 4), (4, 4)).unwrap();
    assert_eq!(read(&ctx, &retained), [255, 0, 0, 255].repeat(16));
    assert_eq!(read(&ctx, &second), [0, 255, 0, 255].repeat(16));
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
