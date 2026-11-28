//! Synthetic only. The coordinator runs this; no retail assets or game.
#![cfg(feature = "gpu")]
use std::borrow::Cow;
use native_render::particle_gpu::*;
use native_render::particles::{DrawIdentity, Vertex, QUAD_INDICES};
use shaders::vsh::encode::{Inst, Operand as O};
use shaders::{psh, vsh};

const SIZE: u32 = 64;
fn program(instructions: &[Inst]) -> gfx::guest_program::Microcode {
    let encoded = vsh::encode::program(instructions);
    let words: Vec<_> = encoded[4..]
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(u32::from_le_bytes)
        .collect();
    words.into()
}

fn snapshot(call: u64, x: f32, tint: [f32; 4]) -> ImmediateDrawSnapshot<'static> {
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
            input_fetch: Default::default(),
        },
        vertex_uniforms,
        pixel_uniforms: Default::default(),
        textures: Default::default(),
        vertices: vertices.into(),
        raw_vertices: None,
        indices: Cow::Borrowed(&QUAD_INDICES),
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

fn ribbon(mut green: ImmediateDrawSnapshot<'static>) -> ImmediateDrawSnapshot<'static> {
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
    green.raw_vertices = Some(Cow::Owned(
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
    ));
    green.vertices.to_mut().clear();
    green.indices = Cow::Borrowed(&[0, 1, 2, 3]);
    green.primitive.topology = wgpu::PrimitiveTopology::TriangleStrip;
    green.primitive.strip_index_format = Some(wgpu::IndexFormat::Uint32);
    green.shader.microcode = program(&[
        Inst::mac(vsh::MacOp::Mov, Some(O::v(0)), None, None).to_out(false, vsh::OutReg::Pos, 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::v(3)), None, None).to_out(false, vsh::OutReg::D0, 15),
    ]);
    green
}

fn drawn(s: &ImmediateDrawSnapshot<'_>) -> Option<Drawn> {
    s.validate().unwrap()
}

#[test]
fn a_trailing_partial_primitive_is_dropped_as_d3d_drops_it() {
    let s = snapshot(1, 4., [1.; 4]);
    assert_eq!(drawn(&s), Some(Drawn { scissor: [0, 0, SIZE, SIZE], indices: 6 }));
    let mut partial = s.clone();
    partial.indices.to_mut().extend([0, 1]);
    assert_eq!(drawn(&partial).map(|d| d.indices), Some(6));
    let mut one = s.clone();
    one.indices = Cow::Borrowed(&[0, 1]);
    assert_eq!(drawn(&one), None, "no whole triangle rasterises nothing");
    let mut strip = s;
    strip.primitive.topology = wgpu::PrimitiveTopology::TriangleStrip;
    strip.indices = Cow::Borrowed(&[0, 1]);
    assert_eq!(drawn(&strip), None);
}

#[test]
fn the_scissor_is_clamped_to_the_target_and_an_empty_one_draws_nothing() {
    let mut s = snapshot(1, 4., [1.; 4]);
    s.scissor = [8, 4, u32::MAX, 100];
    assert_eq!(drawn(&s).map(|d| d.scissor), Some([8, 4, SIZE - 8, SIZE - 4]));
    s.scissor = [u32::MAX, 0, 4, 4];
    assert_eq!(drawn(&s), None);
    s.scissor = [0, 0, 0, SIZE];
    assert_eq!(drawn(&s), None);
}

#[test]
#[should_panic(expected = "index lists stay inside")]
fn an_index_past_the_vertices_is_an_adapter_bug_not_a_refusal() {
    let mut s = snapshot(1, 4., [1.; 4]);
    s.indices.to_mut()[0] = 4;
    let _ = s.validate();
}

#[test]
#[should_panic(expected = "device and generation")]
fn an_unnamed_draw_is_an_adapter_bug_not_a_refusal() {
    let mut s = snapshot(1, 4., [1.; 4]);
    s.identity.generation = 0;
    let _ = s.validate();
}

#[test]
fn weather_lines_draw_whole_pairs_and_points_remain_unadmitted() {
    let mut s = snapshot(1, 4., [1.; 4]);
    s.primitive.topology = wgpu::PrimitiveTopology::LineList;
    s.indices = Cow::Borrowed(&[0, 1, 2, 3]);
    assert_eq!(drawn(&s).map(|d| d.indices), Some(4));
    s.indices.to_mut().pop();
    assert_eq!(drawn(&s).map(|d| d.indices), Some(2));
    s.primitive.topology = wgpu::PrimitiveTopology::PointList;
    assert_eq!(s.validate(), Err(Refused::State));
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
    assert_eq!(a.record(&mut encoder, red.identity, red.target, &cv, &dv).indices, 6);
    assert_eq!(b.record(&mut encoder, green.identity, green.target, &cv, &dv).draws, 1);
    // A draw that rasterises nothing records no pass.
    let mut empty = red.clone();
    empty.scissor = [0, 0, 0, 0];
    let none = renderer.prepare(&ctx.device, &empty).unwrap();
    assert_eq!(none.record(&mut encoder, empty.identity, empty.target, &cv, &dv).draws, 0);
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
fn textured(
    mut s: ImmediateDrawSnapshot<'static>,
    texture: TextureBinding,
) -> ImmediateDrawSnapshot<'static> {
    s.shader.microcode = program(&[
        Inst::mac(vsh::MacOp::Mov, Some(O::v(0)), None, None).to_out(false, vsh::OutReg::Pos, 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::v(9)), None, None).to_out(false, vsh::OutReg::T(0), 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::c(20)), None, None).to_out(false, vsh::OutReg::D0, 15),
    ]);
    for v in s.vertices.to_mut() {
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
fn scene(texture: Option<TextureBinding>) -> Vec<ImmediateDrawSnapshot<'static>> {
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
    scene: &[ImmediateDrawSnapshot<'_>],
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
            .filter_map(|s| renderer.draws(&ctx.device, &ctx.queue, &arena, s).unwrap())
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
            prepared.record(&mut encoder, s.identity, s.target, &cv, &dv);
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
fn a_packet_takes_any_stencil_reference_masked_to_eight_bits() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut renderer = ParticleRenderer::new(1);
    let arena = gfx::ring::UploadPool::default().begin();
    let mut s = snapshot(1, 4., [1.; 4]);
    s.stencil_reference = 0x1ff;
    assert!(renderer.draws(&ctx.device, &ctx.queue, &arena, &s).unwrap().is_some());
}

#[test]
fn a_sampled_stage_with_no_texture_reads_zero() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut renderer = ParticleRenderer::new(1);
    let pool = gfx::ring::UploadPool::default();
    let texture = solid_texture(&ctx, &mut renderer, [255, 128, 0, 255]);
    let mut unbound = textured(snapshot(2, 4., [1.; 4]), texture);
    unbound.textures[0] = None;
    let scene = [snapshot(1, 4., [1., 0., 0., 1.]), unbound];
    for packets in [false, true] {
        let pixels = frame(&ctx, &mut renderer, &pool, &scene, packets);
        let i = ((32 * SIZE + 12) * 4) as usize;
        assert_eq!(pixels[i..i + 4], [0, 0, 0, 255], "zero colour over the red quad, v0 alpha");
    }
}

/// One textured packet of `texture`, staged in a fresh arena as frame `n`.
fn textured_packet(
    ctx: &gfx::GfxContext,
    renderer: &mut ParticleRenderer,
    n: u64,
    texture: TextureBinding,
) {
    renderer.begin_frame(n);
    let arena = gfx::ring::UploadPool::default().begin();
    let s = textured(snapshot(1, 4., [1.; 4]), texture);
    renderer.draws(&ctx.device, &ctx.queue, &arena, &s).unwrap().unwrap();
}

#[test]
fn a_texture_bound_every_frame_keeps_one_group() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut renderer = ParticleRenderer::new(1);
    let texture = solid_texture(&ctx, &mut renderer, [255, 128, 0, 255]);
    for n in 1..=30 {
        textured_packet(&ctx, &mut renderer, n, texture.clone());
    }
    assert_eq!(
        renderer.created().texture_groups,
        1,
        "made once, hit on every later frame"
    );
    assert_eq!(
        renderer.held(),
        Held {
            groups: 1,
            textures: 1,
            texture_bytes: 4 * 4 * 4
        }
    );
}

#[test]
fn a_new_texture_every_frame_is_released_once_its_frames_pass() {
    // A frame-buffer copy or depth alias is a new texture each frame. Its
    // group must go with the frames that drew with it, not stay until a
    // count fills (256 groups of 2560x1920 copies is over 5 GB).
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let keep = native_render::texture_groups::KEEP_FRAMES;
    let mut renderer = ParticleRenderer::new(1);
    for n in 1..=40 {
        let fresh = solid_texture(&ctx, &mut renderer, [n as u8, 0, 0, 255]);
        textured_packet(&ctx, &mut renderer, n, fresh);
        let held = renderer.held();
        let live = (keep + 1).min(n) as usize;
        assert_eq!(
            held.groups, live,
            "frame {n}: only the last {live} frames' groups"
        );
        assert_eq!(held.textures, live);
        assert_eq!(held.texture_bytes, live as u64 * 4 * 4 * 4);
    }
    assert_eq!(
        renderer.created().texture_groups,
        40,
        "one group per new texture"
    );
    // Draws that bind none of them: every group ages out and frees its texture.
    renderer.begin_frame(40 + keep + 1);
    assert_eq!(renderer.held(), Held::default());
}
