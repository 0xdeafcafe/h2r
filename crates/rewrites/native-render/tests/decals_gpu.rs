//! Synthetic only. The coordinator runs this; no retail assets or game.
#![cfg(feature = "gpu")]
use d3d::TargetCopy;
use gfx::state::{ColorWriteMask, DepthBufferMode, RenderState, TextureStageState};
use native_render::decal::{Mode, Vertex};
use native_render::decal_gpu::{
    ClipRect, DecalCache, DecalClip, DecalGpu, DecalTarget, PacketTarget, Prepared, ReferenceCopy,
    ShaderSnapshot, StageExtras,
};
use shaders::psh;
use shaders::vsh::encode::{program, Inst, Operand as O};
use shaders::vsh::{self, VertexUniforms};

const SIZE: u32 = 64;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// An opaque decal over `[x, x + 40] x [8, 56]` in `rgb`, clipped to the
/// left and right strips of the target; with a texture it takes stage 0's.
fn decal(x: f32, rgb: [f32; 3], textured: bool) -> Prepared {
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
    let packed_register = if textured { 3 } else { 9 };
    let mut insts = vec![
        Inst::mac(vsh::MacOp::Mov, Some(O::v(0)), None, None).to_out(false, vsh::OutReg::Pos, 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::v(packed_register)), None, None).to_out(false, vsh::OutReg::D0, 15),
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
            .map(|[x, y]| Vertex { position: [x, y, 0.5], diffuse: rgb })
            .to_vec(),
        shader: ShaderSnapshot {
            render,
            stages: std::array::from_fn(|n| TextureStageState::default_for_stage(n as u32)),
            extras: [StageExtras::default(); 4],
            combiner,
            microcode: program(&insts),
            provided_inputs: 1 | (1 << packed_register),
            vertex,
            w_buffered: false,
            integer_depth: true,
            tex_scale: None,
        },
        clips: vec![
            ClipRect { x: 0, y: 0, width: 24, height: SIZE },
            ClipRect { x: 40, y: 0, width: 24, height: SIZE },
        ],
        reference: None,
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
    copy: TargetCopy,
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
            let clip = DecalClip { w_buffered: Some(prepared.shader.w_buffered), ..CLIP };
            let target = PacketTarget {
                color_format: COLOR,
                depth_format: DEPTH,
                guest_size: (SIZE, SIZE),
                size: (SIZE, SIZE),
                z_scale: 1.,
                stage0: texture.as_ref(),
                copy,
            };
            let (packet, discards) = cache
                .draws(&ctx.device, &ctx.queue, &arena, prepared, clip, &target)
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
            let clip = DecalClip { w_buffered: Some(prepared.shader.w_buffered), ..CLIP };
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
                copy,
            };
            assert!(gpu.encode(&mut target, clip));
        }
        ctx.queue.submit([encoder.finish()]);
    }
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    gfx::readback::read_texture(ctx, &color, wgpu::TextureAspect::All, 4, (SIZE, SIZE))
}

fn scene(ctx: &gfx::GfxContext) -> Vec<(Prepared, Option<wgpu::TextureView>)> {
    vec![
        // Red, then a textured one that takes the texture's orange.
        (decal(4., [1., 0., 0.], false), None),
        (decal(20., [1., 1., 1.], true), Some(solid_view(ctx, [255, 128, 0, 255]))),
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
    let old = frame(&ctx, &mut cache, &pool, &scene, false, TargetCopy::Display);
    let new = frame(&ctx, &mut cache, &pool, &scene, true, TargetCopy::Display);
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
fn stage_zero_samples_a_live_arbitrary_border_colour() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut cache = DecalCache::default();
    let pool = gfx::ring::UploadPool::default();
    let mut prepared = decal(4., [1., 1., 1.], true);
    prepared.shader.stages[0].address_u = gfx::state::TextureAddress::Border;
    prepared.shader.stages[0].border_color = 0xff20_40c0;
    prepared.shader.vertex.constants[21][0] = -0.5;
    let texture = solid_view(&ctx, [255, 128, 0, 255]);
    let mut scene = [(prepared, Some(texture))];
    let i = ((32 * SIZE + 10) * 4) as usize;
    for packets in [false, true] {
        let pixels = frame(&ctx, &mut cache, &pool, &scene, packets, TargetCopy::Display);
        assert_eq!(&pixels[i..i + 4], &[32, 64, 192, 255]);
    }
    let created = cache.created();
    scene[0].0.shader.stages[0].border_color = 0xff80_2104;
    let pixels = frame(&ctx, &mut cache, &pool, &scene, true, TargetCopy::Display);
    assert_eq!(&pixels[i..i + 4], &[128, 33, 4, 255]);
    assert_eq!(cache.created(), created, "border colour is a uniform");
    scene[0].0.shader.vertex.constants[21][0] = 0.5;
    let pixels = frame(&ctx, &mut cache, &pool, &scene, true, TargetCopy::Display);
    assert_eq!(&pixels[i..i + 4], &[255, 128, 0, 255]);
}

#[test]
fn constant_bias_offsets_w_before_the_depth_test() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut cache = DecalCache::default();
    let pool = gfx::ring::UploadPool::default();
    let make = |color, w| {
        let mut prepared = decal(4., color, false);
        prepared.shader.w_buffered = true;
        prepared.shader.integer_depth = false;
        prepared.shader.render.depth_mode = DepthBufferMode::W;
        prepared.shader.render.depth_func = gfx::state::CompareFunc::Less;
        prepared.shader.render.depth_write = true;
        prepared.shader.vertex.constants[21][3] = w;
        prepared.shader.microcode = program(&[
            Inst::mac(vsh::MacOp::Mov, Some(O::v(0)), None, None)
                .to_out(false, vsh::OutReg::Pos, 14),
            Inst::mac(vsh::MacOp::Mov, Some(O::c(21)), None, None)
                .to_out(false, vsh::OutReg::Pos, 1),
            Inst::mac(vsh::MacOp::Mov, Some(O::v(9)), None, None)
                .to_out(false, vsh::OutReg::D0, 15),
        ]);
        prepared
    };
    let first = make([1., 0., 0.], 0.6);
    let mut second = make([0., 1., 0.], 0.8);
    second.shader.render.depth_bias = -0.4;
    let mut scene = [(first, None), (second, None)];
    let i = ((32 * SIZE + 10) * 4) as usize;
    for (offset, expected) in [(false, [255, 0, 0, 255]), (true, [0, 255, 0, 255])] {
        scene[1].0.shader.render.solid_offset_enable = offset;
        for packets in [false, true] {
            let pixels = frame(&ctx, &mut cache, &pool, &scene, packets, TargetCopy::Display);
            assert_eq!(&pixels[i..i + 4], &expected);
        }
    }
}

#[test]
fn reference_and_display_draw_their_own_constants_and_scissors() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut cache = DecalCache::default();
    let pool = gfx::ring::UploadPool::default();
    let mut prepared = decal(4., [1., 0., 0.], false);
    prepared.shader.microcode = program(&[
        Inst::mac(vsh::MacOp::Add, Some(O::v(0)), None, Some(O::c(21)))
            .to_out(false, vsh::OutReg::Pos, 15),
        Inst::mac(vsh::MacOp::Mov, Some(O::v(9)), None, None)
            .to_out(false, vsh::OutReg::D0, 15),
    ]);
    prepared.reference = Some(ReferenceCopy {
        constants: prepared.shader.vertex.constants.into(),
        clips: prepared.clips.clone(),
    });
    prepared.shader.vertex.constants[21][0] = 12.;
    prepared.shader.vertex.ui_transform[2] = 8.;
    prepared.clips = vec![
        ClipRect { x: 8, y: 0, width: 24, height: SIZE },
        ClipRect { x: 48, y: 0, width: 16, height: SIZE },
    ];
    let scene = [(prepared, None)];
    for packets in [false, true] {
        let reference = frame(&ctx, &mut cache, &pool, &scene, packets, TargetCopy::Reference);
        let display = frame(&ctx, &mut cache, &pool, &scene, packets, TargetCopy::Display);
        let reds = |image: &[u8]| [12, 20, 28, 40, 54].map(|x| image[(32 * SIZE as usize + x) * 4]);
        assert_eq!(reds(&reference), [255, 255, 0, 255, 0]);
        assert_eq!(reds(&display), [0, 0, 255, 0, 255]);
    }
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
    let first = frame(&ctx, &mut cache, &pool, &scene, true, TargetCopy::Display);
    let (created, churn) = (cache.created(), gfx::churn::snapshot());
    let first_spent = churn - start;
    assert!(created.programs == 2 && created.pipelines == 2 && created.texture_groups == 1);
    let second = frame(&ctx, &mut cache, &pool, &scene, true, TargetCopy::Display);
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
    let mut prepared = decal(4., [1., 0., 0.], false);
    prepared.clips.clear();
    let target = PacketTarget {
        color_format: COLOR,
        depth_format: DEPTH,
        guest_size: (SIZE, SIZE),
        size: (SIZE, SIZE),
        z_scale: 1.,
        stage0: None,
        copy: TargetCopy::Display,
    };
    assert!(matches!(cache.draws(&ctx.device, &ctx.queue, &arena, &prepared, CLIP, &target), Ok(None)));
    let wrong_mode = DecalClip { w_buffered: Some(true), ..CLIP };
    prepared.clips.push(ClipRect { x: 0, y: 0, width: 8, height: 8 });
    assert!(cache.draws(&ctx.device, &ctx.queue, &arena, &prepared, wrong_mode, &target).is_err());
}

/// One textured decal packet of `view` as frame `n`.
fn textured_packet(
    ctx: &gfx::GfxContext,
    cache: &mut DecalCache,
    n: u64,
    view: &wgpu::TextureView,
) {
    cache.begin_frame(n);
    let arena = gfx::ring::UploadPool::default().begin();
    let target = PacketTarget {
        color_format: COLOR,
        depth_format: DEPTH,
        guest_size: (SIZE, SIZE),
        size: (SIZE, SIZE),
        z_scale: 1.,
        stage0: Some(view),
        copy: TargetCopy::Display,
    };
    let prepared = decal(20., [1., 1., 1.], true);
    assert!(matches!(
        cache.draws(&ctx.device, &ctx.queue, &arena, &prepared, CLIP, &target),
        Ok(Some(_))
    ));
}

#[test]
fn decal_texture_groups_last_while_draws_use_them() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let keep = native_render::texture_groups::KEEP_FRAMES;
    let mut cache = DecalCache::default();
    let steady = solid_view(&ctx, [255, 128, 0, 255]);
    for n in 1..=20 {
        textured_packet(&ctx, &mut cache, n, &steady);
    }
    assert_eq!(
        cache.created().texture_groups,
        1,
        "a steady texture's group is made once"
    );
    // A new texture each frame (a frame-buffer copy) keeps only the
    // groups of the frames still in the window.
    for n in 21..=60 {
        textured_packet(&ctx, &mut cache, n, &solid_view(&ctx, [n as u8, 0, 0, 255]));
        let held = cache.held();
        assert!(held.groups as u64 <= keep + 1, "frame {n}: {held:?}");
        if n > 20 + keep {
            assert_eq!(
                held.groups as u64,
                keep + 1,
                "frame {n}: the steady group has aged out"
            );
        }
        assert_eq!(
            held.texture_bytes,
            held.groups as u64 * 4 * 4 * 4,
            "each group holds its own texture"
        );
    }
    cache.begin_frame(60 + keep + 1);
    assert_eq!(cache.held(), native_render::texture_groups::Held::default());
}
