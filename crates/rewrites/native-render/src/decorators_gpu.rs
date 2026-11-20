//! Records a decorator [`Plan`](crate::decorators::Plan)
//! (docs/graphics/native-decorators.md): the plan's instances staged once
//! per copy as an instance-rate vertex buffer ([`instance_layout`],
//! locations 11 to 15), then each batch one `draw_indexed` over the set's
//! u16 triangle list, its instance range drawing what the guest draws one
//! instance at a time.
//!
//! The batches' pipelines and bindings ([`Bound`]) come from whoever owns
//! the decorator sets' geometry and the materials' programs: the program
//! translated with `decorators::PROVIDED_INPUTS` supplied, its vertex
//! buffers the set's streams followed by [`instance_layout`], `c[53]` the
//! batch's constant in its uniforms.

use crate::decorators::{INSTANCE_STRIDE, Plan};

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
}

/// Where a plan is drawn.
pub struct DecoratorTarget<'a> {
    pub color: &'a wgpu::TextureView,
    pub depth: &'a wgpu::TextureView,
    /// Whether the depth format has a stencil aspect to keep.
    pub stencil: bool,
    /// `(x, y, width, height)` in the targets' texels.
    pub viewport: [f32; 4],
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
    let [x, y, w, h] = target.viewport;
    pass.set_viewport(x, y, w, h, 0.0, 1.0);
    let span = plan.instance_bytes().len() as u64;
    for (batch, b) in plan.batches.iter().zip(bound) {
        pass.set_pipeline(&b.pipeline);
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
        let bound = [Bound { pipeline, bind_groups: Vec::new(), streams: vec![(vertices, 0)], indices }];
        let target = DecoratorTarget { color: &cv, depth: &dv, stencil: false, viewport: [0.0, 0.0, SIZE as f32, SIZE as f32] };
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
}
