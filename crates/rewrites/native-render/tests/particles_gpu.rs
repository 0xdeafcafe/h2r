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

fn ribbon(mut green: ImmediateDrawSnapshot) -> ImmediateDrawSnapshot {
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
    green
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
    let green = snapshot(2, 36., [0., 1., 0., 1.]);
    // A generated ribbon uses the same immutable upload path, preserving
    // its strip order and packed BGRA colour fetch on the GPU.
    let green = ribbon(green);
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

fn attachment(ctx: &gfx::GfxContext, format: wgpu::TextureFormat) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// A solid 2D texture and the sampler the renderer caches for it.
fn solid_texture(
    ctx: &gfx::GfxContext,
    renderer: &mut ParticleRenderer,
    rgba: [u8; 4],
) -> TextureBinding {
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue.write_texture(
        texture.as_image_copy(),
        &rgba.repeat(16),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16), rows_per_image: Some(4) },
        wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
    );
    let caps = gfx::translate::Capabilities::from_features(ctx.device.features());
    let key = gfx::translate::SamplerKey::from_stage(&gfx::state::TextureStageState::default_for_stage(0), caps);
    TextureBinding {
        device: 1,
        generation: 1,
        identity: 7,
        dimension: psh::TextureDim::D2,
        view: texture.create_view(&Default::default()),
        sampler: renderer.sampler(&ctx.device, &key),
    }
}

/// A quad that takes its colour from texture 0.
fn textured(mut s: ImmediateDrawSnapshot, texture: TextureBinding) -> ImmediateDrawSnapshot {
    s.shader.microcode = program(&[
        Inst::mac(vsh::MacOp::Mov, Some(O::v(0)), None, None).to_out(false, vsh::OutReg::Pos, 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::v(9)), None, None).to_out(false, vsh::OutReg::T(0), 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::c(20)), None, None).to_out(false, vsh::OutReg::D0, 15),
    ]);
    for v in &mut s.vertices {
        v.registers[9] = [0.5, 0.5, 0., 1.];
    }
    s.shader.pixel.final_inputs_abcd = psh::bits::REG_T0 as u32;
    s.shader.pixel.texture_modes = psh::bits::texture_modes([psh::bits::TEX_PROJECT2D, 0, 0, 0]);
    s.shader.pixel_options.texture_dims[0] = Some(psh::TextureDim::D2);
    s.textures[0] = Some(texture);
    s
}

/// Red, a green strip, a scissored constant-blend quad over the red one,
/// and (given a texture) an orange textured quad: every state a packet
/// has to carry.
fn scene(texture: Option<TextureBinding>) -> Vec<ImmediateDrawSnapshot> {
    let mut blended = snapshot(3, 10., [1.; 4]);
    blended.blend = Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::REPLACE,
    });
    blended.blend_constant = wgpu::Color { r: 0.5, g: 0.25, b: 0., a: 1. };
    blended.scissor = [0, 0, 16, SIZE];
    let mut all = vec![
        snapshot(1, 4., [1., 0., 0., 1.]),
        ribbon(snapshot(2, 36., [0., 1., 0., 1.])),
        blended,
    ];
    if let Some(texture) = texture {
        let mut quad = snapshot(4, 36., [1.; 4]);
        quad.scissor = [0, 0, SIZE, 24];
        all.push(textured(quad, texture));
    }
    all
}

/// Draws `scene` over a cleared target, either as each draw's own pass
/// (`prepare` and `record`) or as packets in one pass (`draws`), and reads
/// the colour back.
fn frame(
    ctx: &gfx::GfxContext,
    renderer: &mut ParticleRenderer,
    pool: &gfx::ring::UploadPool,
    scene: &[ImmediateDrawSnapshot],
    packets: bool,
) -> Vec<u8> {
    let (color, depth) = (
        attachment(ctx, wgpu::TextureFormat::Rgba8Unorm),
        attachment(ctx, wgpu::TextureFormat::Depth32Float),
    );
    let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &cv,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &dv,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    let arena = pool.begin();
    if packets {
        let draws: Vec<_> = scene
            .iter()
            .map(|s| renderer.draws(&ctx.device, &ctx.queue, &arena, s).unwrap())
            .collect();
        let target = gfx::RenderTarget {
            color: &cv,
            color_format: wgpu::TextureFormat::Rgba8Unorm,
            depth: Some(&dv),
            depth_format: Some(wgpu::TextureFormat::Depth32Float),
            size: (SIZE, SIZE),
            samples: 1,
        };
        let draws: Vec<_> = draws.into_iter().map(|(draw, _)| draw).collect();
        gfx::Renderer::encode_pass(ctx, &mut encoder, &target, gfx::Clear::NONE, &draws, None).unwrap();
        let front = arena.upload_commands(&ctx.queue);
        ctx.queue.submit(front.into_iter().chain([encoder.finish()]));
    } else {
        for s in scene {
            let prepared = renderer.prepare(&ctx.device, s).unwrap();
            prepared.record(&mut encoder, s.identity, s.target, &cv, &dv).unwrap();
        }
        ctx.queue.submit([encoder.finish()]);
    }
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    gfx::readback::read_texture(ctx, &color, wgpu::TextureAspect::All, 4, (SIZE, SIZE))
}

#[test]
fn packets_draw_what_the_particle_pass_draws() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut renderer = ParticleRenderer::new(1);
    let pool = gfx::ring::UploadPool::default();
    let texture = solid_texture(&ctx, &mut renderer, [255, 128, 0, 255]);
    let scene = scene(Some(texture));
    let old = frame(&ctx, &mut renderer, &pool, &scene, false);
    let new = frame(&ctx, &mut renderer, &pool, &scene, true);
    assert_eq!(old, new, "the packets draw the same pixels");
    let px = |x: u32, y: u32| {
        let i = ((y * SIZE + x) * 4) as usize;
        [new[i], new[i + 1], new[i + 2], new[i + 3]]
    };
    assert_eq!(px(6, 32), [255, 0, 0, 255], "red, outside the blended quad");
    assert!(px(12, 32)[1] > 30, "the blend constant is added inside its scissor");
    assert_eq!(px(20, 32), [255, 0, 0, 255], "and nothing outside the scissor");
    assert_eq!(px(44, 40), [0, 255, 0, 255], "the strip below the textured quad's scissor");
    assert_eq!(px(44, 16), [255, 128, 0, 255], "the textured quad draws its texture");
}

#[test]
fn a_second_identical_frame_creates_no_programs_pipelines_groups_or_buffers() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut renderer = ParticleRenderer::new(1);
    let pool = gfx::ring::UploadPool::default();
    let texture = solid_texture(&ctx, &mut renderer, [255, 128, 0, 255]);
    let scene = scene(Some(texture));
    let start = gfx::churn::snapshot();
    let first = frame(&ctx, &mut renderer, &pool, &scene, true);
    let (created, churn) = (renderer.created(), gfx::churn::snapshot());
    let first_spent = churn - start;
    assert!(created.programs > 0 && created.pipelines > 0 && created.texture_groups > 0);
    let second = frame(&ctx, &mut renderer, &pool, &scene, true);
    assert_eq!(renderer.created(), created, "nothing cached is made again");
    let spent = gfx::churn::snapshot() - churn;
    // `frame` reads its colour back, and the readback's staging buffer is
    // the one buffer the test itself makes each frame; the arena's rings
    // (made on the first frame) come back from the pool on the second.
    assert!(first_spent.buffers_created > 1, "the first frame makes the arena's rings: {first_spent:?}");
    assert_eq!(spent.buffers_created, 1, "only the readback's buffer: the arena's are pooled");
    assert_eq!(first, second);
}

#[test]
fn a_packet_refuses_a_stencil_reference_it_cannot_carry() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut renderer = ParticleRenderer::new(1);
    let arena = gfx::ring::UploadPool::default().begin();
    let mut s = snapshot(1, 4., [1.; 4]);
    s.stencil_reference = 256;
    assert_eq!(renderer.draws(&ctx.device, &ctx.queue, &arena, &s).err(), Some(Refused::State));
    assert!(renderer.prepare(&ctx.device, &s).is_ok(), "the isolated pass still takes it");
}
