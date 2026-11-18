//! Textured full-target blits: the host half of `Swap`, and of
//! `CopyRects`/`Resolve` when a plain texture copy is not possible (different
//! host formats or sizes, or forced-opaque `X8` formats).

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::context::GfxContext;

/// Options for one blit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlitOptions<'a> {
    /// Source rectangle in normalised texture coordinates: `[x, y, w, h]`.
    pub src_rect: [f32; 4],
    /// Linear (true) or nearest (false) sampling.
    pub linear: bool,
    /// Force output alpha to 1.
    pub force_opaque: bool,
    /// Optional DAC ramp: 256 red, green and blue bytes.
    pub gamma: Option<&'a [u8; 768]>,
}

impl Default for BlitOptions<'_> {
    fn default() -> Self {
        Self { src_rect: [0.0, 0.0, 1.0, 1.0], linear: false, force_opaque: false, gamma: None }
    }
}

/// Draws a source texture over an entire destination view.
pub struct Blitter {
    module: wgpu::ShaderModule,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
}

impl Blitter {
    /// Creates the blit shader and samplers.
    pub fn new(ctx: &GfxContext) -> Self {
        let d = &ctx.device;
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/blit.wgsl").into()),
        });
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                entry(
                    0,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                ),
                entry(
                    1,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(2, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let sampler = |filter| {
            d.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            })
        };
        Self {
            module,
            bgl,
            layout,
            pipelines: HashMap::new(),
            nearest: sampler(wgpu::FilterMode::Nearest),
            linear: sampler(wgpu::FilterMode::Linear),
        }
    }

    /// Records a pass drawing `src` over all of `dst` (a single-sampled
    /// colour view of format `dst_format`).
    pub fn blit(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        opts: BlitOptions<'_>,
    ) {
        let (module, layout) = (&self.module, &self.layout);
        let pipeline = self.pipelines.entry(dst_format).or_insert_with(|| {
            ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("blit"),
                layout: Some(layout),
                vertex: wgpu::VertexState { module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: dst_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let mut params = vec![
            opts.src_rect[0],
            opts.src_rect[1],
            opts.src_rect[2],
            opts.src_rect[3],
            if opts.force_opaque { 1.0 } else { 0.0 },
            if opts.gamma.is_some() { 1.0 } else { 0.0 },
            0.0,
            0.0,
        ];
        for i in 0..256 {
            let channel = |c: usize| opts.gamma.map_or(i as f32 / 255.0, |g| g[c * 256 + i] as f32 / 255.0);
            params.extend_from_slice(&[channel(0), channel(1), channel(2), 0.0]);
        }
        let buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("blit params"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(if opts.linear { &self.linear } else { &self.nearest }),
                },
            ],
        });
        crate::churn::pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Draws a source texture over an entire destination view through a
/// projective warp: destination texel `(u, v)` (normalised) samples the
/// source at `H * (u, v, 1)` with linear filtering, clamped to the edge.
/// d3d's interpolated reprojection turns the camera between ticks with it.
pub struct Warper {
    module: wgpu::ShaderModule,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    sampler: wgpu::Sampler,
}

impl Warper {
    /// Creates the warp shader and sampler.
    pub fn new(ctx: &GfxContext) -> Self {
        let d = &ctx.device;
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("warp"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/warp.wgsl").into()),
        });
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("warp"),
            entries: &[
                entry(
                    0,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                ),
                entry(
                    1,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(2, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("warp"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Self { module, bgl, layout, pipelines: HashMap::new(), sampler }
    }

    /// Records a pass drawing `src` warped by `h` (rows) over all of `dst`
    /// (a single-sampled colour view of format `dst_format`).
    pub fn warp(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        h: [[f32; 3]; 3],
    ) {
        let (module, layout) = (&self.module, &self.layout);
        let pipeline = self.pipelines.entry(dst_format).or_insert_with(|| {
            ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("warp"),
                layout: Some(layout),
                vertex: wgpu::VertexState { module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: dst_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let params: Vec<f32> = h.iter().flat_map(|r| [r[0], r[1], r[2], 0.0]).collect();
        let buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("warp params"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("warp"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        crate::churn::pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("warp"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod warp_tests {
    #[test]
    fn the_warp_shader_validates() {
        use wgpu::naga;
        let module = naga::front::wgsl::parse_str(include_str!("../shaders/warp.wgsl")).expect("parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("validates");
    }
}
