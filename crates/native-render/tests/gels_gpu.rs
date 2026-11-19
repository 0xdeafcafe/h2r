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
    assert!(ticket.output(key, (4, 4), (8, 8)).is_none());
    ctx.queue.submit([encoder.finish()]);
    let retained = ticket.output(key, (4, 4), (4, 4)).unwrap();
    assert_eq!(read(&ctx, &retained), [255, 0, 0, 255].repeat(16));
    assert_eq!(read(&ctx, &second), [0, 255, 0, 255].repeat(16));
}
