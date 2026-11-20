//! Records a decorator [`Plan`](crate::decorators::Plan)
//! (docs/graphics/native-decorators.md): the plan's instances staged once
//! per copy as an instance-rate vertex buffer ([`instance_layout`],
//! locations 11 to 15), then each batch one `draw_indexed` over the set's
//! u16 triangle list, its instance range drawing what the guest draws one
//! instance at a time.
//!
//! Each batch draws with its material's [`MaterialGpu`]: the pass's
//! program (`crate::decorators_load::compile`, `v11..v15` supplied by the
//! instance buffer), the set's vertices as that program reads them, its
//! stage textures, and per batch the uniforms with the view's constants
//! and the batch's `c[53]` ([`bind`]).

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Arc;

use gfx::compile::PendingPipeline;
use gfx::pipeline::{PipelineKey, ShaderId, ShaderProgram};
use gfx::vertex::OwnedVertexBufferLayout;

use shaders::psh::PixelUniforms;
use shaders::vsh::VertexUniforms;

use crate::decorators::{INSTANCE_STRIDE, Material, Plan};
use crate::decorators_load::{DecoratorBuffers, DecoratorProgram, uniforms};
use crate::gpu::{Image, texture};
use crate::look::Stage;
use crate::object_program_gpu::attributes;
use crate::object_streams::SLOT;
use crate::program::{ViewInputs, bias_signed, pipeline_state, sampler_descriptor, uniform_entry};

/// `v11..v15`, four floats each.
pub const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    11 => Float32x4,
    12 => Float32x4,
    13 => Float32x4,
    14 => Float32x4,
    15 => Float32x4,
];

/// The instance buffer's layout: last in a decorator pipeline's buffers.
pub fn instance_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: INSTANCE_STRIDE,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &INSTANCE_ATTRIBUTES,
    }
}

/// What one batch draws with.
#[derive(Clone, Debug)]
pub struct Bound {
    pub pipeline: wgpu::RenderPipeline,
    /// Groups 0.. in order (uniforms with the batch's `c[53]`, textures).
    pub bind_groups: Vec<wgpu::BindGroup>,
    /// The set's vertex streams in the pipeline's buffer order, each with
    /// its byte offset; the instance buffer follows them.
    pub streams: Vec<(wgpu::Buffer, u64)>,
    /// The set's u16 indices.
    pub indices: wgpu::Buffer,
    /// The pass's blend constant.
    pub blend_constant: wgpu::Color,
}

/// Where a plan is drawn.
pub struct DecoratorTarget<'a> {
    pub color: &'a wgpu::TextureView,
    pub depth: &'a wgpu::TextureView,
    /// Whether the depth format has a stencil aspect to keep.
    pub stencil: bool,
    /// The targets' size in texels: the viewport. The game's programs
    /// place vertices in the whole surface's pixels.
    pub size: (u32, u32),
    /// The view's rectangle `(x, y, width, height)` in texels: the
    /// scissor, as the D3D path clips the layer's draws to the viewport.
    pub viewport: [f32; 4],
}

/// The scissor of `viewport` in a target of `size`, `None` when empty.
fn scissor(viewport: [f32; 4], size: (u32, u32)) -> Option<[u32; 4]> {
    let [x, y, w, h] = viewport.map(|v| v.max(0.0).round() as u32);
    let (x, y) = (x.min(size.0), y.min(size.1));
    let (w, h) = (w.min(size.0 - x), h.min(size.1 - y));
    (w != 0 && h != 0).then_some([x, y, w, h])
}

/// Records `plan` with `bound` (one per batch) into its own render pass on
/// `target`, loading and keeping what is there. The number of guest draws
/// (placements) recorded, or why nothing was.
pub fn encode(
    device: &wgpu::Device, queue: &wgpu::Queue, uploads: &gfx::ring::SubmissionUploads, encoder: &mut wgpu::CommandEncoder,
    target: &DecoratorTarget<'_>, plan: &Plan, bound: &[Bound],
) -> Result<usize, &'static str> {
    if bound.len() != plan.batches.len() {
        return Err("a decorator batch has no pipeline");
    }
    if plan.instances.is_empty() {
        return Ok(plan.slots.len());
    }
    let Some([sx, sy, sw, sh]) = scissor(target.viewport, target.size) else {
        return Err("the view's rectangle is empty");
    };
    let (instances, offset) = uploads.stage(device, queue, plan.instance_bytes());
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("native decorators"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target.color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: target.depth,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
            stencil_ops: target.stencil.then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_viewport(0.0, 0.0, target.size.0 as f32, target.size.1 as f32, 0.0, 1.0);
    pass.set_scissor_rect(sx, sy, sw, sh);
    let span = plan.instance_bytes().len() as u64;
    for (batch, b) in plan.batches.iter().zip(bound) {
        pass.set_pipeline(&b.pipeline);
        pass.set_blend_constant(b.blend_constant);
        for (group, g) in b.bind_groups.iter().enumerate() {
            pass.set_bind_group(group as u32, g, &[]);
        }
        for (slot, (buffer, at)) in b.streams.iter().enumerate() {
            pass.set_vertex_buffer(slot as u32, buffer.slice(*at..));
        }
        pass.set_vertex_buffer(b.streams.len() as u32, instances.slice(offset..offset + span));
        pass.set_index_buffer(b.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(batch.first_index..batch.first_index + batch.index_count, 0, batch.instances.clone());
    }
    Ok(plan.slots.len())
}

/// One decorator material on a device: its program's module and layouts,
/// its stage textures, the set's buffers as the program reads them, and
/// its pipelines by target formats.
pub struct MaterialGpu {
    program: DecoratorProgram,
    uniforms_layout: wgpu::BindGroupLayout,
    textures: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    attributes: Vec<wgpu::VertexAttribute>,
    stride: u64,
    pipelines: crate::pipelines::Pipelines,
    formats: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), Arc<PendingPipeline>>,
}

impl MaterialGpu {
    /// Uploads a material: `images` holds each sampled bitmap stage's
    /// image. An error when the program does not validate, a sampled stage
    /// has no image, or the set has no geometry.
    pub fn new(
        device: &wgpu::Device, queue: &wgpu::Queue, program: DecoratorProgram, buffers: &DecoratorBuffers,
        images: &[Option<Image>; 4],
    ) -> Result<Self, &'static str> {
        if buffers.vertices.is_empty() || buffers.indices.is_empty() || buffers.registers.is_empty() {
            return Err("the set has no geometry");
        }
        let index_count = u32::try_from(buffers.indices.len()).map_err(|_| "the set has too many indices")?;
        let module = gfx::program::validated_resource(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native decorator program"),
                source: wgpu::ShaderSource::Wgsl(program.wgsl.as_str().into()),
            })
        })
        .ok_or("the decorator program does not validate")?;
        let mut views = Vec::new();
        for (n, dim) in program.dims.iter().enumerate() {
            if dim.is_none() {
                continue;
            }
            let Stage::Bitmap { .. } = program.stages[n] else {
                return Err("a sampled stage is not a bitmap");
            };
            let mut image = images[n].clone().ok_or("a sampled stage has no image")?;
            bias_signed(&mut image, program.signed[n]);
            views.push((n, texture(device, queue, &image, "native decorator stage")));
        }
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native decorator uniforms"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
            ],
        });
        let mut entries = Vec::new();
        for (n, dim) in program.dims.iter().enumerate() {
            let Some(dim) = *dim else { continue };
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: crate::light_gpu::view_dimension(dim),
                    multisampled: false,
                },
                count: None,
            });
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32 + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native decorator textures"),
            entries: &entries,
        });
        let samplers: Vec<(usize, wgpu::Sampler)> = views
            .iter()
            .map(|&(n, _)| (n, device.create_sampler(&sampler_descriptor(program.samplers[n].as_ref()))))
            .collect();
        let mut group = Vec::new();
        for ((n, view), (_, sampler)) in views.iter().zip(&samplers) {
            group.push(wgpu::BindGroupEntry { binding: 2 * *n as u32, resource: wgpu::BindingResource::TextureView(view) });
            group.push(wgpu::BindGroupEntry { binding: 2 * *n as u32 + 1, resource: wgpu::BindingResource::Sampler(sampler) });
        }
        let textures = gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
            label: Some("native decorator textures"),
            layout: &textures_layout,
            entries: &group,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native decorator"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let buffer = |label, contents: &[u8], usage| {
            gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        let vertices = buffer("native decorator vertices", bytemuck::cast_slice(&buffers.vertices), wgpu::BufferUsages::VERTEX);
        let indices = buffer("native decorator indices", bytemuck::cast_slice(&buffers.indices), wgpu::BufferUsages::INDEX);
        let shader = ShaderProgram { module, layout, vs_entry: "vs_main".into(), fs_entry: "fs_main".into(), count_entry: None };
        let pipelines = crate::pipelines::Pipelines::new(device, &program.wgsl, shader, "native decorator");
        Ok(Self {
            attributes: attributes(&buffers.registers, &buffers.formats),
            stride: (SLOT * buffers.registers.len()) as u64,
            program,
            uniforms_layout,
            textures,
            vertices,
            indices,
            index_count,
            pipelines,
            formats: HashMap::new(),
        })
    }

    fn pipeline(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> &Arc<PendingPipeline> {
        self.formats.entry((color, depth)).or_insert_with(|| {
            let state = &self.program.state;
            let (primitive, (depth_write, depth_compare)) = pipeline_state(Some(state), None);
            let key = PipelineKey {
                shader: ShaderId(0),
                vertex_buffers: vec![
                    OwnedVertexBufferLayout { stride: self.stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: self.attributes.clone() },
                    OwnedVertexBufferLayout { stride: INSTANCE_STRIDE, step_mode: wgpu::VertexStepMode::Instance, attributes: INSTANCE_ATTRIBUTES.to_vec() },
                ],
                primitive,
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth, depth_write_enabled: Some(depth_write), depth_compare: Some(depth_compare),
                    stencil: wgpu::StencilState::default(), bias: wgpu::DepthBiasState::default(),
                }),
                color_target: wgpu::ColorTargetState { format: color, blend: state.blend, write_mask: state.color_writes },
                samples: 1,
            };
            self.pipelines.request(device, &key)
        })
    }

    /// Request the target-specific pipeline before the host clears any
    /// slots. If a view misses, its siblings also use D3D for this frame.
    pub fn pipeline_ready(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> bool {
        self.pipeline(device, color, depth).previous_frame().is_some_and(Option::is_some)
    }

    /// Whether the pass writes depth.
    pub fn writes_depth(&self) -> bool {
        self.program.state.depth_write
    }
}

/// Each batch of `plan` bound with its material from `materials`: its
/// pipeline for the target formats, its uniforms (the view's constants
/// and the batch's `c[53]`, all batches' in one buffer at aligned
/// offsets), textures and buffers. An error when a batch's material is
/// not uploaded or its index range is past the set's.
pub fn bind(
    device: &wgpu::Device, materials: &mut HashMap<Material, MaterialGpu>, plan: &Plan, view: &ViewInputs,
    color: wgpu::TextureFormat, depth: wgpu::TextureFormat,
) -> Result<Vec<Bound>, &'static str> {
    if plan.batches.is_empty() {
        return Ok(Vec::new());
    }
    let align = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(1);
    let (vertex_size, pixel_size) = (VertexUniforms::SIZE as u64, PixelUniforms::SIZE as u64);
    let pixel_at = vertex_size.next_multiple_of(align);
    let stride = pixel_at + pixel_size.next_multiple_of(align);
    let mut bytes = vec![0u8; (stride * plan.batches.len() as u64) as usize];
    for (k, batch) in plan.batches.iter().enumerate() {
        let m = materials.get(&batch.material).ok_or("a batch's material is not uploaded")?;
        if batch.first_index.checked_add(batch.index_count).is_none_or(|end| end > m.index_count) {
            return Err("a batch's indices are past the set's");
        }
        let (vertex, pixel) = uniforms(&m.program, view, batch.lod_constant);
        let at = (stride * k as u64) as usize;
        vertex.write_bytes(&mut bytes[at..at + vertex_size as usize]);
        let p = at + pixel_at as usize;
        pixel.write_bytes(&mut bytes[p..p + pixel_size as usize]);
    }
    let buffer = gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor {
        label: Some("native decorator uniforms"),
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let mut out = Vec::with_capacity(plan.batches.len());
    for (k, batch) in plan.batches.iter().enumerate() {
        let m = materials.get_mut(&batch.material).ok_or("a batch's material is not uploaded")?;
        let pipeline = m.pipeline(device, color, depth).try_get().and_then(Option::as_ref)
            .cloned().ok_or("decorator pipeline unavailable")?;
        let at = stride * k as u64;
        let binding = |offset: u64, size: u64| {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset, size: NonZeroU64::new(size) })
        };
        let group = gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
            label: Some("native decorator uniforms"),
            layout: &m.uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: binding(at, vertex_size) },
                wgpu::BindGroupEntry { binding: 1, resource: binding(at + pixel_at, pixel_size) },
            ],
        });
        out.push(Bound {
            pipeline,
            bind_groups: vec![group, m.textures.clone()],
            streams: vec![(m.vertices.clone(), 0)],
            indices: m.indices.clone(),
            blend_constant: m.program.state.blend_constant,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use frame_scene::{DecoratorDraw, DecoratorInstance, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};

    use super::*;
    use crate::decorators::plan;

    const SIZE: u32 = 16;

    /// A model vertex is a position; the instance's rows place it. The
    /// colour is `v15`.
    const PROGRAM: &str = r"
struct In {
    @location(0) p: vec3f,
    @location(11) n: vec4f,
    @location(12) r0: vec4f,
    @location(13) r1: vec4f,
    @location(14) r2: vec4f,
    @location(15) c: vec4f,
}
struct Out { @builtin(position) pos: vec4f, @location(0) c: vec4f }
@vertex fn vs(i: In) -> Out {
    let v = vec4f(i.p, 1.0);
    var o: Out;
    o.pos = vec4f(dot(i.r0, v), dot(i.r1, v), 0.5 + 0.0 * dot(i.r2, v) * i.n.w, 1.0);
    o.c = i.c;
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4f { return o.c; }
";

    fn scene_plan() -> Plan {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        let camera = frame_scene::RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            viewport: [0, 0, SIZE as i16, SIZE as i16],
            near: 0.1,
            far: 100.0,
        };
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera).unwrap();
        let draw = DecoratorDraw {
            slot: 0, layer: 3, set: 1, shader: 2, variant: 0, pass: 0, argument: 0, lod_constant: [0.0, 0.0, 0.0, 1.0],
            instances: 0..0,
        };
        // Two instances of model 0 (indices 3..9 of the buffer): one at
        // x −0.5 red, one at x 0.5 green.
        let at = |x: f32, c: [f32; 4]| DecoratorInstance {
            index: 0,
            model: 0,
            first_index: 3,
            index_count: 6,
            registers: [[0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], c],
        };
        b.push_decorator(draw, &[at(-0.5, [1.0, 0.0, 0.0, 1.0]), at(0.5, [0.0, 1.0, 0.0, 1.0])]).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        plan(&s, &s.views[0], |_| true)
    }

    #[test]
    fn one_instanced_draw_places_each_instance_with_its_rows_and_colour() {
        use wgpu::{BufferUsages as B, TextureFormat as F, TextureUsages as U};
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let plan = scene_plan();
        assert_eq!(plan.batches.len(), 1, "both instances in one draw");
        let device = &ctx.device;
        let buffer = |bytes: &[u8], usage| {
            let b = gfx::churn::create_buffer(device, &wgpu::BufferDescriptor {
                label: None, size: bytes.len() as u64, usage: usage | B::COPY_DST, mapped_at_creation: false,
            });
            ctx.queue.write_buffer(&b, 0, bytes);
            b
        };
        let quad: [[f32; 3]; 4] = [[-0.25, -0.25, 0.0], [0.25, -0.25, 0.0], [0.25, 0.25, 0.0], [-0.25, 0.25, 0.0]];
        // A degenerate triangle first, so the draw's first index counts;
        // padded to whole words.
        let indices: [u16; 10] = [0, 0, 0, 0, 1, 2, 0, 2, 3, 0];
        let vertices = buffer(bytemuck::cast_slice(&quad), B::VERTEX);
        let indices = buffer(bytemuck::cast_slice(&indices), B::INDEX);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(PROGRAM.into()),
        });
        let position = wgpu::vertex_attr_array![0 => Float32x3];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &position }),
                    Some(instance_layout()),
                ],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: F::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: F::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let texture = |format| device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage: U::RENDER_ATTACHMENT | U::COPY_SRC, view_formats: &[],
        });
        let (color, depth) = (texture(F::Rgba8Unorm), texture(F::Depth32Float));
        let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
        let mut enc = device.create_command_encoder(&Default::default());
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &cv, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &dv, stencil_ops: None,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            }),
            timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        }));
        let arena = gfx::ring::UploadPool::default().begin();
        let bound = [Bound { pipeline, bind_groups: Vec::new(), streams: vec![(vertices, 0)], indices, blend_constant: wgpu::Color::BLACK }];
        let target = DecoratorTarget {
            color: &cv, depth: &dv, stencil: false, size: (SIZE, SIZE), viewport: [0.0, 0.0, SIZE as f32, SIZE as f32],
        };
        assert_eq!(encode(device, &ctx.queue, &arena, &mut enc, &target, &plan, &bound), Ok(1));
        assert_eq!(encode(device, &ctx.queue, &arena, &mut enc, &target, &plan, &[]), Err("a decorator batch has no pipeline"));
        let front = arena.upload_commands(&ctx.queue);
        ctx.queue.submit(front.into_iter().chain([enc.finish()]));
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let texels = gfx::readback::read_texture(&ctx, &color, wgpu::TextureAspect::All, 4, (SIZE, SIZE));
        let at = |x: u32, y: u32| {
            let i = 4 * (y * SIZE + x) as usize;
            [texels[i], texels[i + 1], texels[i + 2], texels[i + 3]]
        };
        assert_eq!(at(4, 8), [255, 0, 0, 255], "the first instance");
        assert_eq!(at(12, 8), [0, 255, 0, 255], "the second");
        assert_eq!(at(8, 8), [0, 0, 0, 0], "between them, nothing");
    }

    /// The loader's path end to end: a translated program (the model
    /// vertex placed by `v12..v14`, then `c[0..3]`), the set's gathered
    /// vertices, the material uploaded and each batch bound with its
    /// uniforms. The program writes the W-buffered depth, so what was
    /// drawn shows in the depth.
    #[test]
    fn a_translated_decorator_program_draws_each_instance_where_its_rows_place_it() {
        use crate::decorators_load::{SetGeometry, buffers, compile};
        use crate::decorators_load::tests::{decorator, stream};
        use crate::program::VIEW_CONSTANTS;
        use wgpu::{TextureFormat as F, TextureUsages as U};
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let device = &ctx.device;
        let plan = scene_plan();
        let program = compile(&decorator(None), &stream()).unwrap();
        // The quad of the other test, at indices 3..9 after a degenerate
        // triangle.
        let quad: [[f32; 3]; 4] = [[-0.25, -0.25, 0.0], [0.25, -0.25, 0.0], [0.25, 0.25, 0.0], [-0.25, 0.25, 0.0]];
        let set = SetGeometry {
            stream_type: 0,
            stride: 12,
            vertices: quad.iter().flatten().flat_map(|f| f.to_le_bytes()).collect(),
            indices: vec![0, 0, 0, 0, 1, 2, 0, 2, 3],
        };
        let geometry = buffers(&program, &set).unwrap();
        let mut materials = HashMap::new();
        let material = plan.batches[0].material;
        materials.insert(material, MaterialGpu::new(device, &ctx.queue, program, &geometry, &[None, None, None, None]).unwrap());
        // Guest pixels: x and y from −1..1 to 0..16; w 1.
        let mut constants = [[0.0; 4]; VIEW_CONSTANTS];
        let half = SIZE as f32 / 2.0;
        constants[..4].copy_from_slice(&[[half, 0.0, 0.0, half], [0.0, half, 0.0, half], [0.0, 0.0, 0.0, 0.5], [0.0, 0.0, 0.0, 1.0]]);
        let view = ViewInputs { constants, guest_size: (SIZE, SIZE), size: (SIZE, SIZE), z_scale: 16_777_215.0, w_clip: None };
        // The synthetic test owns the compile deadline. A live view
        // leaves its slots on D3D while the same pipeline is pending.
        let gpu = materials.get_mut(&material).unwrap();
        assert!(gpu.pipeline(device, F::Rgba8Unorm, F::Depth32Float).get().is_some());
        let bound = bind(device, &mut materials, &plan, &view, F::Rgba8Unorm, F::Depth32Float).unwrap();
        assert_eq!(bound.len(), 1);
        let mut short = plan.clone();
        short.batches[0].index_count = 16;
        assert!(bind(device, &mut materials, &short, &view, F::Rgba8Unorm, F::Depth32Float).is_err(), "past the set's indices");
        assert!(bind(device, &mut HashMap::new(), &plan, &view, F::Rgba8Unorm, F::Depth32Float).is_err(), "not uploaded");
        let texture = |format| device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage: U::RENDER_ATTACHMENT | U::COPY_SRC, view_formats: &[],
        });
        let (color, depth) = (texture(F::Rgba8Unorm), texture(F::Depth32Float));
        let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
        let mut enc = device.create_command_encoder(&Default::default());
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &cv, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &dv, stencil_ops: None,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            }),
            timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        }));
        let arena = gfx::ring::UploadPool::default().begin();
        let target = DecoratorTarget {
            color: &cv, depth: &dv, stencil: false, size: (SIZE, SIZE), viewport: [0.0, 0.0, SIZE as f32, SIZE as f32],
        };
        assert_eq!(encode(device, &ctx.queue, &arena, &mut enc, &target, &plan, &bound), Ok(1));
        let front = arena.upload_commands(&ctx.queue);
        ctx.queue.submit(front.into_iter().chain([enc.finish()]));
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let texels = gfx::readback::read_texture(&ctx, &depth, wgpu::TextureAspect::DepthOnly, 4, (SIZE, SIZE));
        let at = |x: u32, y: u32| {
            let i = 4 * (y * SIZE + x) as usize;
            f32::from_le_bytes(texels[i..i + 4].try_into().unwrap())
        };
        assert!(at(4, 8) < 1.0, "the first instance: {}", at(4, 8));
        assert!(at(12, 8) < 1.0, "the second: {}", at(12, 8));
        assert_eq!(at(8, 8), 1.0, "between them, nothing");
    }
}
