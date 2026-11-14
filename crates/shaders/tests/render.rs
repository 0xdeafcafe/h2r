//! End-to-end render through a linked module, exercising the contract a
//! renderer relies on: one module with `vs_main` + `fs_main`, vertex register
//! N at location N, uniforms at group 0 and the stage-0 texture/sampler at
//! group 1. Skips when no GPU adapter is available.
// Fixtures start from Default and set the fields each case is about.
#![allow(clippy::field_reassign_with_default)]

mod common;

use shaders::psh::bits::*;
use shaders::psh::{self, PixelShaderDef, PixelUniforms};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{self, MacOp, OutReg, VertexUniforms};
use wgpu::util::DeviceExt;

#[test]
fn linked_pipeline_renders() {
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let d = &gpu.device;
    // mov oPos, v0 ; mov oT0, v9 ; mov oD0, v3
    let bytes = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 15),
        Inst::mac(MacOp::Mov, Some(Operand::v(9)), None, None).to_out(false, OutReg::T(0), 15),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None).to_out(false, OutReg::D0, 15),
    ]);
    let vs = vsh::translate(&vsh::Program::parse(&bytes).unwrap(), &Default::default()).unwrap();
    let mut def = PixelShaderDef::default();
    def.combiner_count = 1;
    def.texture_modes = texture_modes([TEX_PROJECT2D, 0, 0, 0]);
    def.rgb_inputs[0] = inputs(REG_T0, REG_V0, 0, 0);
    def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.alpha_inputs[0] = inputs(REG_T0 | CHAN_ALPHA, REG_V0 | CHAN_ALPHA, 0, 0);
    def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    let ps = psh::translate(&def, &Default::default());
    let source = shaders::link(&vs, &ps).unwrap();
    common::check(&source);
    let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("linked"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    let ub = |binding, vis| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let bgl0 = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            ub(0, wgpu::ShaderStages::VERTEX),
            ub(1, wgpu::ShaderStages::FRAGMENT),
        ],
    });
    let bgl1 = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bgl0), Some(&bgl1)],
        immediate_size: 0,
    });
    let attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 16,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 32,
            shader_location: 9,
        },
    ];
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 48,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attrs,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    });

    // Two triangles covering a 4x4 target, in NV2A window coordinates.
    let diffuse = [1.0f32, 0.5, 1.0, 1.0];
    let corner = |x: f32, y: f32| {
        let mut v = vec![x * 4.0, y * 4.0, 0.0, 1.0];
        v.extend(diffuse);
        v.extend([x, y, 0.0, 1.0]);
        v
    };
    let verts: Vec<f32> = [
        (0.0, 0.0),
        (1.0, 0.0),
        (0.0, 1.0),
        (1.0, 0.0),
        (1.0, 1.0),
        (0.0, 1.0),
    ]
    .iter()
    .flat_map(|&(x, y)| corner(x, y))
    .collect();
    let vbuf = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &verts
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>(),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let mut vu = VertexUniforms::default();
    vu.surface = [4.0, 4.0];
    vu.z_max = 1.0;
    let vs_uniforms = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &vu.to_bytes(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let ps_uniforms = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &PixelUniforms::default().to_bytes(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    // 2x2 texture: red, green / blue, white (top row first).
    let texels: [u8; 16] = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let tex = d.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &texels,
    );
    let sampler = d.create_sampler(&wgpu::SamplerDescriptor::default());
    let view = tex.create_view(&Default::default());
    let bg0 = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl0,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: vs_uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: ps_uniforms.as_entire_binding(),
            },
        ],
    });
    let bg1 = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl1,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let target = d.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let tview = target.create_view(&Default::default());
    let readback = d.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = d.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &tview,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg0, &[]);
        pass.set_bind_group(1, &bg1, &[]);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.draw(0..6, 0..1);
    }
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(4),
            },
        },
        wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([enc.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    d.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = slice.get_mapped_range().expect("mapped");
    let px = |x: usize, y: usize| data[y * 256 + x * 4..y * 256 + x * 4 + 4].to_vec();
    // Nearest sampling: each 2x2 block of the target shows one texel, times
    // diffuse (1, 0.5, 1, 1). Guest screen row 0 is the target's top row.
    assert_eq!(px(0, 0), [255, 0, 0, 255]);
    assert_eq!(px(3, 0), [0, 128, 0, 255]);
    assert_eq!(px(0, 3), [0, 0, 255, 255]);
    assert_eq!(px(3, 3), [255, 128, 255, 255]);
}
