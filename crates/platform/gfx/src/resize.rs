//! Per-axis render-scale transfers for colour, depth and stencil.
use std::collections::HashMap;

use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct Size { dst: vec2u, row_words: u32, pad: u32 }
@group(0) @binding(0) var<uniform> size: Size;
@group(0) @binding(1) var color: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;
@group(0) @binding(3) var stencil: texture_2d<u32>;
@group(0) @binding(4) var<storage, read_write> bytes: array<u32>;

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    return vec4f(f32((i << 1u) & 2u) * 2.0 - 1.0, 1.0 - f32(i & 2u) * 2.0, 0.0, 1.0);
}
fn source(p: vec2u, extent: vec2u) -> vec2u {
    // On reduction read the first sample at the rounded guest-pixel edge.
    // On enlargement, choose the guest cell between the two rounded edges.
    let down = (p * extent + size.dst / 2u) / size.dst;
    let up = ((2u * p + 1u) * extent - 1u) / (2u * size.dst);
    return min(select(up, down, extent >= size.dst), extent - 1u);
}
@fragment fn fs_color(@builtin(position) p: vec4f) -> @location(0) vec4f {
    return textureLoad(color, source(vec2u(p.xy), textureDimensions(color)), 0);
}
@fragment fn fs_depth(@builtin(position) p: vec4f) -> @builtin(frag_depth) f32 {
    return textureLoad(depth, source(vec2u(p.xy), textureDimensions(depth)), 0);
}
@compute @workgroup_size(8, 8)
fn pack_stencil(@builtin(global_invocation_id) id: vec3u) {
    let x = id.x * 4u;
    if x >= size.dst.x || id.y >= size.dst.y { return; }
    var word = 0u;
    for (var k = 0u; k < 4u && x + k < size.dst.x; k++) {
        let p = source(vec2u(x + k, id.y), textureDimensions(stencil));
        word |= textureLoad(stencil, p, 0).r << (k * 8u);
    }
    bytes[id.y * size.row_words + id.x] = word;
}
"#;

/// Pipelines shared by colour, depth and stencil surface transfers.
pub struct Resizer {
    shader: wgpu::ShaderModule,
    pipes: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    stencil: wgpu::ComputePipeline,
}

impl Resizer {
    /// Cache transfer pipelines on the rendering device.
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render scale transfer"), source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let stencil = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("render scale stencil"), layout: None, module: &shader,
            entry_point: Some("pack_stencil"), compilation_options: Default::default(), cache: None,
        });
        Self { shader, pipes: HashMap::new(), stencil }
    }

    /// Resize a single-sampled source over the destination's top-left extent. Depth and stencil use
    /// identical source coordinates and never round-trip through CPU memory.
    pub fn encode(&mut self, device: &wgpu::Device, enc: &mut wgpu::CommandEncoder,
        src: &wgpu::Texture, dst: &wgpu::Texture, extent: (u32, u32)) {
        let format = dst.format();
        let depth = format.is_depth_stencil_format();
        let (width, height) = extent;
        let row = width.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let size = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("render scale extent"), usage: wgpu::BufferUsages::UNIFORM,
            contents: bytemuck::cast_slice(&[width, height, row / 4, 0]),
        });
        if format.has_stencil_aspect() {
            let packed = crate::churn::create_buffer(device, &wgpu::BufferDescriptor {
                label: Some("render scale stencil rows"), size: u64::from(row) * u64::from(height),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
            });
            let view = src.create_view(&wgpu::TextureViewDescriptor {
                aspect: wgpu::TextureAspect::StencilOnly, ..Default::default()
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &self.stencil.get_bind_group_layout(0), entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: size.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 4, resource: packed.as_entire_binding() },
                ],
            });
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.stencil);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(width.div_ceil(32), height.div_ceil(8), 1);
            }
            enc.copy_buffer_to_texture(wgpu::TexelCopyBufferInfo { buffer: &packed,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
            }, wgpu::TexelCopyTextureInfo { texture: dst, mip_level: 0, origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::StencilOnly }, wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
        }
        let pipe = self.pipes.entry(format).or_insert_with(|| {
            let targets = [Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("render scale transfer"), layout: None,
                vertex: wgpu::VertexState { module: &self.shader, entry_point: Some("vs"),
                    compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(), multisample: Default::default(),
                depth_stencil: depth.then_some(wgpu::DepthStencilState {
                    format, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(), bias: Default::default(),
                }),
                fragment: Some(wgpu::FragmentState { module: &self.shader,
                    entry_point: Some(if depth { "fs_depth" } else { "fs_color" }),
                    compilation_options: Default::default(), targets: if depth { &[] } else { &targets } }),
                multiview_mask: None, cache: None,
            })
        });
        let input = src.create_view(&wgpu::TextureViewDescriptor {
            aspect: if depth { wgpu::TextureAspect::DepthOnly } else { wgpu::TextureAspect::All },
            ..Default::default()
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &pipe.get_bind_group_layout(0), entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: size.as_entire_binding() },
                wgpu::BindGroupEntry { binding: if depth { 2 } else { 1 }, resource: wgpu::BindingResource::TextureView(&input) },
            ],
        });
        let output = dst.create_view(&Default::default());
        let colors = [Some(wgpu::RenderPassColorAttachment { view: &output, depth_slice: None,
            resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })];
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("render scale transfer"), color_attachments: if depth { &[] } else { &colors },
            depth_stencil_attachment: depth.then_some(wgpu::RenderPassDepthStencilAttachment {
                view: &output, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }), timestamp_writes: None, occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_scissor_rect(0, 0, width, height);
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
