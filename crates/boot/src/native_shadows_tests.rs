use super::*;
use d3d::ExternalPass;
use d3d::{
    backend::{TargetInfo, TextureInfo},
    shader::{CONSTANTS, ShaderInputs, VertexProcessing},
    state::StateWords,
};
use gfx::{
    D3DFormat, OffscreenTarget,
    state::{CompareFunc, CullMode, DepthBufferMode, StencilState},
    vertex::{
        PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
    },
};
use shaders::{
    psh::{PixelShaderDef, bits::*},
    vsh::{
        self, MacOp, OutReg,
        encode::{Inst, Operand, program},
    },
};

const MAP: u32 = 0x1000;
fn receiver() -> ResolvedDraw {
    let vs = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 15),
        Inst::mac(MacOp::Mov, Some(Operand::v(1)), None, None)
            .to_out(false, OutReg::T(0), 15)
            .last(),
    ]);
    let program: Vec<u32> = vsh::Program::parse(&vs)
        .unwrap()
        .instructions
        .iter()
        .flat_map(|i| i.words)
        .collect();
    let def = PixelShaderDef {
        texture_modes: texture_modes([TEX_PROJECT3D, 0, 0, 0]),
        final_inputs_abcd: inputs(0, 0, 0, REG_T0),
        final_inputs_efg: final_efg(0, 0, REG_T0 | CHAN_ALPHA, 0),
        ..Default::default()
    };
    let bytes = def.to_bytes();
    let ps: [u32; 60] =
        std::array::from_fn(|i| u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap()));
    let mut raw = StateWords::defaults(false);
    raw.render[..57].copy_from_slice(&ps[..57]);
    raw.render[136] = ps[54];
    raw.render[d3d::state::slot::SHADOWFUNC as usize] = CompareFunc::Greater.xbox_value();
    let mut state = raw.translate(&Default::default());
    state.render.depth_mode = DepthBufferMode::Disabled;
    state.render.depth_write = false;
    state.render.cull_mode = CullMode::None;
    state.render.color_write = gfx::state::ColorWriteMask::from_xbox(1 << 24);
    state.render.blend_enable = false;
    state.render.alpha_test_enable = false;
    state.render.stencil = StencilState {
        enable: true,
        func: CompareFunc::Equal,
        reference: 3,
        ..Default::default()
    };
    let mut verts = Vec::new();
    for (x, y) in [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)] {
        for v in [x, y, 0.5f32, 1.0, 0.0, 0.0, 100.0, 1.0] {
            verts.extend_from_slice(&v.to_le_bytes());
        }
    }
    let tex = TextureInfo {
        header: MAP,
        palette: 0,
        format: D3DFormat::LinD24S8,
        kind: gfx::texture::TextureKind::D2,
        width: 1,
        height: 1,
        depth: 1,
        levels: 1,
    };
    ResolvedDraw {
        entry: "synthetic shadow",
        primitive: PrimitiveType::TriangleList,
        vertex_count: 3,
        layout: VertexLayout {
            streams: vec![StreamLayout {
                stream: 0,
                stride: 32,
                attributes: vec![
                    VertexAttribute {
                        register: VertexRegister(0),
                        data_type: VertexDataType::Float4,
                        offset: 0,
                    },
                    VertexAttribute {
                        register: VertexRegister(1),
                        data_type: VertexDataType::Float4,
                        offset: 16,
                    },
                ],
            }],
            pretransformed: false,
        },
        streams: vec![d3d::StreamBytes {
            data: verts.into(),
            stride: 32,
        }],
        indices: None,
        target: TargetInfo {
            width: 16,
            height: 16,
            format: D3DFormat::A8R8G8B8,
            depth_format: Some(D3DFormat::D24S8),
        },
        textures: [Some(MAP), None, None, None],
        texture_info: [Some(tex), None, None, None],
        extras: raw.stage_extras(),
        state,
        shader: ShaderInputs {
            vertex: VertexProcessing::Program {
                start: 0,
                microcode: program.into(),
            },
            provided_inputs: 3,
            vertex_defaults: [[0.0; 4]; 16],
            constants: Arc::new([[0.0; 4]; CONSTANTS]),
            pixel_shader: Some(Arc::new(ps)),
            combiner_state: Arc::new(ps[..57].try_into().unwrap()),
            final_combiner_latch: true,
            affine_projection: false,
        },
        raw: Arc::new(raw),
        viewport: d3d::Viewport {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
            min_z: 0.0,
            max_z: 1.0,
        },
        clip: Some(d3d::PixelRect {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
        }),
        clip_rects: vec![],
        glyph_cache: 0,
        hw: Default::default(),
    }
}

#[test]
fn layer11_uses_only_its_map_and_preserves_rgb_stencil_and_alpha_test() {
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("SKIPPED shadow GPU test: no headless adapter");
        return;
    };
    let surface = OffscreenTarget::new(&ctx.device, (16, 16), wgpu::TextureFormat::Rgba8Unorm);
    let term = |depth: u8| {
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            texture.as_image_copy(),
            &[depth, 0, 0, 0],
            wgpu::TexelCopyBufferLayout::default(),
            texture.size(),
        );
        SampledMap {
            kind: MapKind::Depth,
            view: texture.create_view(&Default::default()),
            size: (1, 1),
        }
    };
    let fallback = term(0);
    let light = term(255);
    let blocked = term(0);
    let gpu = new(GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    });
    for (map, stencil, alpha_test, want) in [
        (&light, 3, false, 255),
        (&blocked, 3, false, 0),
        (&light, 4, false, 64),
        (&blocked, 3, true, 64),
    ] {
        let mut d = receiver();
        d.state.render.alpha_test_enable = alpha_test;
        d.state.render.alpha_func = CompareFunc::Greater;
        d.state.render.alpha_ref = 1;
        let mut range = Range {
            gpu: gpu.clone(),
            draw: d,
            packets: Vec::new(),
            recorded: None,
            map: Some((Arc::new(Mutex::new(Some(map.clone()))), MapKind::Depth, MAP)),
        };
        let target = surface.target();
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        Renderer::encode_pass(
            &ctx,
            &mut enc,
            &target,
            Clear {
                color: Some([1.0, 0.0, 1.0, 64.0 / 255.0]),
                depth: Some(1.0),
                stencil: Some(stencil),
            },
            &[],
            None,
        )
        .unwrap();
        let uploads = gfx::ring::UploadPool::default().begin();
        let destination = ExternalTarget {
            device: &ctx.device,
            queue: &ctx.queue,
            encoder: &mut enc,
            color: target.color,
            color_format: target.color_format,
            depth: target.depth.unwrap(),
            depth_format: target.depth_format.unwrap(),
            viewport: [0.0, 0.0, 16.0, 16.0],
            z_scale: 16_777_215.0,
            guest_size: (16, 16),
            size: (16, 16),
            sampled: &[],
            slots: &[],
            uploads: &uploads,
            copy: d3d::TargetCopy::Reference,
            stages: [Some(&fallback.view), None, None, None],
            on_submit: None,
        };
        range.prepare_copy(&destination).unwrap();
        assert_eq!(range.record_ranges(destination), 1);
        ctx.queue.submit([enc.finish()]);
        let px = surface.read_pixels(&ctx);
        assert_eq!(&px[(2 * 16 + 2) * 4..][..4], [255, 0, 255, want]);
        assert_eq!(&px[(15 * 16 + 15) * 4..][..4], [255, 0, 255, 64]);
    }
}

#[test]
fn mixed_native_shadow_unshadowed_and_refused_d3d_lights_match_all_d3d() {
    use d3d::{
        Backend, ExternalPassRequest, ExternalPlan, ExternalSurfaces, GfxBackend, TargetCopy,
    };
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("SKIPPED shadow GPU test: no headless adapter");
        return;
    };
    let mut backend = GfxBackend::new(crate::d3d_glue::share_context(&ctx));
    let gpu = new(crate::d3d_glue::share_context(&ctx));
    let mut make_source = |value: [f32; 4]| {
        let id = backend
            .create_surface(D3DFormat::LinA8R8G8B8, 1, 1, false)
            .unwrap();
        backend
            .clear(
                id,
                None,
                Clear {
                    color: Some(value),
                    ..Clear::NONE
                },
            )
            .unwrap();
        backend.flush().unwrap();
        backend.surface_texture(id).unwrap().unwrap()
    };
    let lit = make_source([1.0, 0.0, 0.0, 1.0]);
    let gel = make_source([0.0, 0.0, 0.0, 128.0 / 255.0]);
    let dark = make_source([0.0; 4]);
    let map = SampledMap {
        kind: MapKind::Depth,
        size: (1, 1),
        view: lit.texture.create_view(&Default::default()),
    };
    let render = |backend: &mut GfxBackend, native: bool| {
        let color = backend
            .create_surface(D3DFormat::A8R8G8B8, 16, 16, false)
            .unwrap();
        let depth = backend
            .create_surface(D3DFormat::D24S8, 16, 16, true)
            .unwrap();
        backend
            .clear(
                color,
                Some(depth),
                Clear {
                    color: Some([1.0, 0.0, 1.0, 0.25]),
                    depth: Some(1.0),
                    stencil: Some(3),
                },
            )
            .unwrap();
        let plan = ExternalPlan {
            stage: timeline::native::Stage::Lights,
            view: 0,
            ranges: 1,
            slots: vec![],
        };
        let surfaces = ExternalSurfaces {
            color,
            depth,
            sampled: vec![],
            slots: vec![],
            slot_sizes: vec![],
        };
        let request = ExternalPassRequest {
            rect: (0, 0, 16, 16),
            surface_size: (16, 16),
            z_scale: 16_777_215.0,
        };
        for (i, original) in [&lit, &gel, &dark].into_iter().enumerate() {
            let mut draw = receiver();
            draw.clip = Some(d3d::PixelRect {
                x: 4 * i as u32,
                y: 0,
                width: 4,
                height: 16,
            });
            if i == 1 {
                // The intervening unshadowed light reads an ordinary colour texture.
                draw.texture_info[0].as_mut().unwrap().format = D3DFormat::LinA8R8G8B8;
                let modes = texture_modes([TEX_PROJECT2D, 0, 0, 0]);
                Arc::make_mut(&mut draw.raw).render[136] = modes;
                Arc::make_mut(draw.shader.pixel_shader.as_mut().unwrap())[54] = modes;
            } else if native {
                let output = Arc::new(Mutex::new((i == 0).then(|| map.clone())));
                let pass = Range {
                    gpu: gpu.clone(),
                    draw: draw.clone(),
                    map: Some((output, MapKind::Depth, MAP)),
                    packets: vec![],
                    recorded: None,
                };
                let commands = backend
                    .prepare_external(
                        surfaces.clone(),
                        request,
                        [Some(&dark), None, None, None],
                        &plan,
                        TargetCopy::Reference,
                        Arc::new(Mutex::new(Box::new(pass))),
                    )
                    .unwrap();
                if i == 0 {
                    let commands = commands.expect("lit receiver admitted");
                    assert_eq!(commands.len(), 1);
                    for command in commands {
                        backend.commit_external(command);
                    }
                    continue;
                }
                assert!(
                    commands.is_none(),
                    "unavailable term must leave its original draw"
                );
            }
            assert_eq!(
                backend
                    .draw(
                        color,
                        Some(depth),
                        &draw,
                        [Some(original), None, None, None]
                    )
                    .unwrap(),
                d3d::DrawOutcome::Drawn
            );
        }
        backend.flush().unwrap();
        backend.store.read_color(&ctx, color).unwrap().0
    };
    let expected = render(&mut backend, false);
    let actual = render(&mut backend, true);
    assert_eq!(actual, expected);
    for (x, alpha) in [(1, 255), (5, 128), (9, 0), (13, 64)] {
        assert_eq!(actual[(16 + x) * 4 + 3], alpha);
    }
}
