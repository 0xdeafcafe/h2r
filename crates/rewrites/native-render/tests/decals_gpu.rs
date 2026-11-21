//! Synthetic only. The coordinator runs this; no retail assets or game.
#![cfg(feature = "gpu")]
use gfx::state::{ColorWriteMask, DepthBufferMode, RenderState, TextureStageState};
use native_render::decal::{Mode, Vertex};
use native_render::decal_gpu::{
    ClipRect, DecalCache, DecalClip, DecalGpu, DecalTarget, PacketTarget, Prepared, ShaderSnapshot,
    StageExtras,
};
use shaders::psh;
use shaders::vsh::encode::{program, Inst, Operand as O};
use shaders::vsh::{self, VertexUniforms};

const SIZE: u32 = 64;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// An opaque decal over `[x, x + 40] x [8, 56]` in `bgra`, clipped to the
/// left and right strips of the target; with a texture it takes stage 0's.
fn decal(x: f32, bgra: [u8; 4], textured: bool) -> Prepared {
    let render = RenderState {
        color_write: ColorWriteMask::from_xbox(0x0001_0101),
        depth_mode: DepthBufferMode::Disabled,
        cull_mode: gfx::state::CullMode::None,
        ..Default::default()
    };
    let mut combiner = psh::PixelShaderDef {
        final_inputs_abcd: psh::bits::REG_V0 as u32,
        final_inputs_efg: ((psh::bits::REG_V0 | psh::bits::CHAN_ALPHA) as u32) << 8,
        ..Default::default()
    };
    let mut insts = vec![
        Inst::mac(vsh::MacOp::Mov, Some(O::v(0)), None, None).to_out(false, vsh::OutReg::Pos, 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::v(3)), None, None).to_out(false, vsh::OutReg::D0, 15),
    ];
    let mut vertex = VertexUniforms {
        surface: [SIZE as f32; 2],
        z_max: 1.,
        ..Default::default()
    };
    if textured {
        combiner.final_inputs_abcd = psh::bits::REG_T0 as u32;
        combiner.texture_modes = psh::bits::texture_modes([psh::bits::TEX_PROJECT2D, 0, 0, 0]);
        insts.push(
            Inst::mac(vsh::MacOp::Mov, Some(O::c(21)), None, None).to_out(false, vsh::OutReg::T(0), 15),
        );
        vertex.constants[21] = [0.5, 0.5, 0., 1.];
    }
    let corners = [[x, 8.], [x + 40., 8.], [x + 40., 56.], [x, 56.]];
    Prepared {
        mode: Mode::Opaque,
        vertices: corners
            .map(|[x, y]| Vertex { position: [x, y, 0.5], color: bgra })
            .to_vec(),
        shader: ShaderSnapshot {
            render,
            stages: std::array::from_fn(|n| TextureStageState::default_for_stage(n as u32)),
            extras: [StageExtras::default(); 4],
            combiner,
            microcode: program(&insts),
            provided_inputs: (1 << 0) | (1 << 3),
            vertex,
            integer_depth: true,
            tex_scale: None,
        },
        clips: vec![
            ClipRect { x: 0, y: 0, width: 24, height: SIZE },
            ClipRect { x: 40, y: 0, width: 24, height: SIZE },
        ],
    }
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

fn solid_view(ctx: &gfx::GfxContext, rgba: [u8; 4]) -> wgpu::TextureView {
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
    texture.create_view(&Default::default())
}

const CLIP: DecalClip = DecalClip { range: [0., 1.], clamp: false, w_buffered: Some(false) };

/// Draws `scene` over a cleared target, either as each decal's own pass
/// (`DecalGpu::encode`) or as packets in one pass (`DecalCache::draws`),
/// and reads the colour back.
fn frame(
    ctx: &gfx::GfxContext,
    cache: &mut DecalCache,
    pool: &gfx::ring::UploadPool,
    scene: &[(Prepared, Option<wgpu::TextureView>)],
    packets: bool,
) -> Vec<u8> {
    let (color, depth) = (attachment(ctx, COLOR), attachment(ctx, DEPTH));
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
        let mut draws = Vec::new();
        for (prepared, texture) in scene {
            let target = PacketTarget {
                color_format: COLOR,
                depth_format: DEPTH,
                guest_size: (SIZE, SIZE),
                size: (SIZE, SIZE),
                z_scale: 1.,
                stage0: texture.as_ref(),
            };
            let (packet, discards) = cache
                .draws(&ctx.device, &ctx.queue, &arena, prepared, CLIP, &target)
                .unwrap()
                .expect("something to draw");
            assert!(!discards);
            assert_eq!(packet.len(), 2, "one packet per clip");
            draws.extend(packet);
        }
        let target = gfx::RenderTarget {
            color: &cv,
            color_format: COLOR,
            depth: Some(&dv),
            depth_format: Some(DEPTH),
            size: (SIZE, SIZE),
            samples: 1,
        };
        gfx::Renderer::encode_pass(ctx, &mut encoder, &target, gfx::Clear::NONE, &draws, None).unwrap();
        let front = arena.upload_commands(&ctx.queue);
        ctx.queue.submit(front.into_iter().chain([encoder.finish()]));
    } else {
        for (prepared, texture) in scene {
            let mut gpu = DecalGpu::new(&ctx.device, prepared).unwrap();
            let mut target = DecalTarget {
                device: &ctx.device,
                queue: &ctx.queue,
                encoder: &mut encoder,
                color: &cv,
                color_format: COLOR,
                depth: &dv,
                depth_format: DEPTH,
                viewport: [0., 0., SIZE as f32, SIZE as f32],
                z_scale: 1.,
                guest_size: (SIZE, SIZE),
                size: (SIZE, SIZE),
                stages: [texture.as_ref(), None, None, None],
            };
            assert!(gpu.encode(&mut target, CLIP));
        }
        ctx.queue.submit([encoder.finish()]);
    }
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    gfx::readback::read_texture(ctx, &color, wgpu::TextureAspect::All, 4, (SIZE, SIZE))
}

fn scene(ctx: &gfx::GfxContext) -> Vec<(Prepared, Option<wgpu::TextureView>)> {
    vec![
        // Red (BGRA), then a textured one that takes the texture's orange.
        (decal(4., [0, 0, 255, 255], false), None),
        (decal(20., [255, 255, 255, 255], true), Some(solid_view(ctx, [255, 128, 0, 255]))),
    ]
}

#[test]
fn packets_draw_what_the_decal_pass_draws() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut cache = DecalCache::default();
    let pool = gfx::ring::UploadPool::default();
    let scene = scene(&ctx);
    let old = frame(&ctx, &mut cache, &pool, &scene, false);
    let new = frame(&ctx, &mut cache, &pool, &scene, true);
    assert_eq!(old, new, "the packets draw the same pixels");
    let px = |x: u32| {
        let i = ((32 * SIZE + x) * 4) as usize;
        [new[i], new[i + 1], new[i + 2], new[i + 3]]
    };
    assert_eq!(px(10)[0], 255, "red where only the first decal reaches");
    assert_eq!(px(30), [0, 0, 0, 255], "between the clips");
    assert_eq!(px(50), [255, 128, 0, 255], "the textured decal draws its texture");
}

#[test]
fn a_second_identical_frame_creates_no_programs_pipelines_groups_or_buffers() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut cache = DecalCache::default();
    let pool = gfx::ring::UploadPool::default();
    let scene = scene(&ctx);
    let start = gfx::churn::snapshot();
    let first = frame(&ctx, &mut cache, &pool, &scene, true);
    let (created, churn) = (cache.created(), gfx::churn::snapshot());
    let first_spent = churn - start;
    assert!(created.programs == 2 && created.pipelines == 2 && created.texture_groups == 1);
    let second = frame(&ctx, &mut cache, &pool, &scene, true);
    assert_eq!(cache.created(), created, "nothing cached is made again");
    let spent = gfx::churn::snapshot() - churn;
    // `frame` reads its colour back, and the readback's staging buffer is
    // the one buffer the test itself makes each frame; the arena's rings
    // (made on the first frame) come back from the pool on the second.
    assert!(first_spent.buffers_created > 1, "the first frame makes the arena's rings: {first_spent:?}");
    assert_eq!(spent.buffers_created, 1, "only the readback's buffer: the arena's are pooled");
    assert_eq!(first, second);
}

#[test]
fn a_draw_clipped_away_entirely_is_left_to_the_isolated_recording() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut cache = DecalCache::default();
    let arena = gfx::ring::UploadPool::default().begin();
    let mut prepared = decal(4., [0, 0, 255, 255], false);
    prepared.clips.clear();
    let target = PacketTarget {
        color_format: COLOR,
        depth_format: DEPTH,
        guest_size: (SIZE, SIZE),
        size: (SIZE, SIZE),
        z_scale: 1.,
        stage0: None,
    };
    assert!(matches!(cache.draws(&ctx.device, &ctx.queue, &arena, &prepared, CLIP, &target), Ok(None)));
    let wrong_mode = DecalClip { w_buffered: Some(true), ..CLIP };
    prepared.clips.push(ClipRect { x: 0, y: 0, width: 8, height: 8 });
    assert!(cache.draws(&ctx.device, &ctx.queue, &arena, &prepared, wrong_mode, &target).is_err());
}
