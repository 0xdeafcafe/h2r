//! Filling a depth/stencil surface from CPU values (a render-to-texture
//! depth level that starts with its texture's guest texels).
//!
//! WebGPU cannot copy into the depth aspect of `Depth32Float*` or
//! `Depth24Plus*` textures, so depth is drawn: a full-screen triangle whose
//! fragment shader loads the value from an `R32Float` texture and writes it
//! as `frag_depth` (compare always, depth write on, stencil kept). Stencil
//! is copied into the stencil aspect, which every stencil format allows.

use std::collections::HashMap;

use crate::context::GfxContext;

const SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) p: vec4f) -> @builtin(frag_depth) f32 {
    return textureLoad(src, vec2i(p.xy), 0).r;
}
"#;

/// The depth-writing pipelines, one per depth format.
pub(crate) struct DepthFill {
    module: wgpu::ShaderModule,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl DepthFill {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth fill"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth fill"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth fill"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        Self { module, bgl, layout, pipelines: HashMap::new() }
    }

    /// Writes `depth` (normalised, row-major, `size` texels) into the depth
    /// aspect of `target` (single-sample, format `format`) and `stencil`,
    /// if given, into its stencil aspect.
    pub(crate) fn fill(
        &mut self,
        ctx: &GfxContext,
        target: &wgpu::Texture,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        depth: &[f32],
        stencil: Option<&[u8]>,
    ) {
        let (w, h) = size;
        let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let src = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth fill source"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes: Vec<u8> = depth.iter().flat_map(|d| d.clamp(0.0, 1.0).to_le_bytes()).collect();
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &src, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            extent,
        );
        let src_view = src.create_view(&Default::default());
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("depth fill"),
            layout: &self.bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src_view) }],
        });
        let (module, layout) = (&self.module, &self.layout);
        let pipeline = self.pipelines.entry(format).or_insert_with(|| {
            ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("depth fill"),
                layout: Some(layout),
                vertex: wgpu::VertexState { module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState { module, entry_point: Some("fs_main"), compilation_options: Default::default(), targets: &[] }),
                multiview_mask: None,
                cache: None,
            })
        });
        let view = target.create_view(&Default::default());
        let has_stencil = format.has_stencil_aspect();
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("depth fill"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: has_stencil.then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit([enc.finish()]);
        if let (Some(s), true) = (stencil, has_stencil) {
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::StencilOnly },
                s,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
                extent,
            );
        }
    }
}
