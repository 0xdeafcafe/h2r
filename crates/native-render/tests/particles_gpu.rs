//! Synthetic only. The coordinator runs this; no retail assets or game.
#![cfg(feature = "gpu")]
use native_render::particle_gpu::*;
use native_render::particles::{DrawIdentity, Vertex, QUAD_INDICES};
use shaders::vsh::encode::{program, Inst, Operand as O};
use shaders::{psh, vsh};

const SIZE: u32 = 64;
fn snapshot(call: u64, x: f32, tint: [f32; 4]) -> ImmediateDrawSnapshot {
    let microcode = program(&[
        Inst::mac(vsh::MacOp::Mov, Some(O::v(0)), None, None).to_out(false, vsh::OutReg::Pos, 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::c(20)), None, None).to_out(false, vsh::OutReg::D0, 15),
    ]);
    let mut vertex_uniforms = vsh::VertexUniforms {
        surface: [SIZE as f32; 2],
        z_max: 1.,
        ..Default::default()
    };
    vertex_uniforms.constants[20] = tint;
    let vertices = [[x, 8.], [x + 20., 8.], [x + 20., 56.], [x, 56.]]
        .map(|[x, y]| {
            let mut registers = [[0.; 4]; 16];
            registers[0] = [x, y, 0.5, 1.];
            Vertex { registers }
        })
        .to_vec();
    ImmediateDrawSnapshot {
        identity: DrawIdentity {
            device: 1,
            generation: 1,
            view: call,
            call,
            copy: 0,
        },
        target: TargetKey {
            color: 1,
            depth: 2,
            size: (SIZE, SIZE),
            color_format: wgpu::TextureFormat::Rgba8Unorm,
            depth_format: wgpu::TextureFormat::Depth32Float,
        },
        shader: ShaderSnapshot {
            microcode,
            pixel: psh::PixelShaderDef {
                final_inputs_abcd: psh::bits::REG_V0 as u32,
                final_inputs_efg: ((psh::bits::REG_V0 | psh::bits::CHAN_ALPHA) as u32) << 8,
                ..Default::default()
            },
            pixel_options: Default::default(),
            flat_shading: false,
        },
        vertex_uniforms,
        pixel_uniforms: Default::default(),
        textures: Default::default(),
        vertices,
        raw_vertices: None,
        indices: QUAD_INDICES.to_vec(),
        primitive: wgpu::PrimitiveState::default(),
        depth: wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        },
        blend: None,
        color_writes: wgpu::ColorWrites::ALL,
        blend_constant: wgpu::Color::TRANSPARENT,
        stencil_reference: 0,
        scissor: [0, 0, SIZE, SIZE],
    }
}

#[test]
fn admission_refuses_partial_geometry_unknown_depth_and_bad_clip() {
    let s = snapshot(1, 4., [1.; 4]);
    assert_eq!(s.validate(), Ok(()));
    let mut bad = s.clone();
    bad.indices.push(0);
    assert_eq!(bad.validate(), Err(Refused::Geometry));
    bad = s.clone();
    bad.indices[0] = 4;
    assert_eq!(bad.validate(), Err(Refused::Geometry));
    bad = s.clone();
    bad.depth.depth_compare = None;
    assert_eq!(bad.validate(), Err(Refused::State));
    bad = s.clone();
    bad.shader.pixel_options.w_buffer = true;
    assert_eq!(bad.validate(), Err(Refused::State));
    bad = s.clone();
    bad.scissor[0] = u32::MAX;
    assert_eq!(bad.validate(), Err(Refused::Scissor));
    bad = s;
    bad.identity.generation = 0;
    assert_eq!(bad.validate(), Err(Refused::Identity));
}

#[test]
fn weather_lines_require_pairs_and_points_remain_unadmitted() {
    let mut s = snapshot(1, 4., [1.; 4]);
    s.primitive.topology = wgpu::PrimitiveTopology::LineList;
    s.indices = vec![0, 1, 2, 3];
    assert_eq!(s.validate(), Ok(()));
    s.indices.pop();
    assert_eq!(s.validate(), Err(Refused::Geometry));
    s.indices.push(3);
    s.primitive.topology = wgpu::PrimitiveTopology::PointList;
    assert!(s.validate().is_err());
}

#[test]
fn two_views_keep_distinct_geometry_and_uniforms_in_one_submission() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let target = |format| {
        ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let color = target(wgpu::TextureFormat::Rgba8Unorm);
    let depth = target(wgpu::TextureFormat::Depth32Float);
    let cv = color.create_view(&Default::default());
    let dv = depth.create_view(&Default::default());
    let mut renderer = ParticleRenderer::new(1);
    let red = snapshot(1, 4., [1., 0., 0., 1.]);
    let mut green = snapshot(2, 36., [0., 1., 0., 1.]);
    // A generated ribbon uses the same immutable upload path, preserving
    // its strip order and packed BGRA colour fetch on the GPU.
    let mut bytes = Vec::new();
    for i in [0, 1, 3, 2] {
        for f in green.vertices[i].registers[0][..3]
            .iter()
            .copied()
            .chain([0., 0.])
        {
            bytes.extend(f.to_le_bytes());
        }
        bytes.extend([0, 255, 0, 255]);
    }
    green.raw_vertices = Some(
        RawVertices::new(
            native_render::effect_geometry::Kind::Contrail,
            bytes,
            vec![
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 12,
                    shader_location: 9,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Unorm8x4Bgra,
                    offset: 20,
                    shader_location: 3,
                },
            ],
        )
        .unwrap(),
    );
    green.vertices.clear();
    green.indices = vec![0, 1, 2, 3];
    green.primitive.topology = wgpu::PrimitiveTopology::TriangleStrip;
    green.primitive.strip_index_format = Some(wgpu::IndexFormat::Uint32);
    green.shader.microcode = program(&[
        Inst::mac(vsh::MacOp::Mov, Some(O::v(0)), None, None).to_out(false, vsh::OutReg::Pos, 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::v(3)), None, None).to_out(false, vsh::OutReg::D0, 15),
    ]);
    let a = renderer.prepare(&ctx.device, &red).unwrap();
    let b = renderer.prepare(&ctx.device, &green).unwrap();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &cv,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &dv,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    assert_eq!(
        a.record(&mut encoder, red.identity, red.target, &cv, &dv)
            .unwrap()
            .indices,
        6
    );
    assert_eq!(
        b.record(&mut encoder, green.identity, green.target, &cv, &dv)
            .unwrap()
            .draws,
        1
    );
    // A stale target must refuse without opening any render pass.
    let stale = renderer.prepare(&ctx.device, &red).unwrap();
    let mut changed = red.target;
    changed.color += 1;
    assert_eq!(
        stale.record(&mut encoder, red.identity, changed, &cv, &dv),
        Err(Refused::Identity)
    );
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (SIZE * SIZE * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let pixels = buffer.slice(..).get_mapped_range().unwrap();
    let px = |x: u32| {
        let i = ((32 * SIZE + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    assert_eq!(
        px(12),
        [255, 0, 0, 255],
        "first view must retain its red constant and left geometry"
    );
    assert_eq!(px(44), [0, 255, 0, 255], "second view has its own uploads");
    assert_eq!(px(30), [0, 0, 0, 255], "gap remains untouched");
}
