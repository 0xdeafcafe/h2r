//! A shader-driven gel producer. Each invocation owns its texture; later
//! lights cannot overwrite a retained result. The light graph binds
//! `Output::view` in the existing `LightStage::Gel` slot.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gfx::translate::SamplerKey;
use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};
use shaders::vsh::{self, VertexOptions, VertexUniforms};
use wgpu::util::DeviceExt;

use crate::gels::Key;
use crate::gpu::{Image, texture};
use crate::pass_state::PassState;

pub const INPUTS: u16 = 0x7f;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub registers: [[f32; 4]; 7],
}

/// `0x00015ec0`'s four vertices. It pushes v1..v4 with 2F (z=0,w=1),
/// v5/v6 with 4F, then v0, in triangle-strip order.
pub fn quad(size: (u32, u32), depth: [f32; 2]) -> [Vertex; 4] {
    std::array::from_fn(|n| {
        let u = (n & 1) as f32;
        let v = (n >> 1) as f32;
        let mut registers = [[0.0; 4]; 7];
        registers[0] = [u * size.0 as f32, v * size.1 as f32, depth[0], depth[1]];
        registers[1..5].fill([u, v, 0.0, 1.0]);
        Vertex { registers }
    })
}

pub struct Program {
    pub wgsl: String,
    pub dims: [Option<TextureDim>; 4],
    pub samplers: [SamplerKey; 4],
    pub state: PassState,
}

impl Program {
    /// The host supplies the evaluated pixel definition and sampler/state
    /// snapshot, after guest shader setup. Constant animations therefore
    /// use the same values as D3D without another function evaluator.
    pub fn compile(
        microcode: &[u8],
        def: &PixelShaderDef,
        options: &PixelOptions,
        samplers: [SamplerKey; 4],
        state: PassState,
    ) -> Result<Self, String> {
        Self::compile_inner(microcode, def, options, samplers, state, false)
    }

    /// A draw into the live target preserves pixels the shader does not
    /// overwrite, including blend/discard and partial colour writes.
    pub fn compile_target(
        microcode: &[u8],
        def: &PixelShaderDef,
        options: &PixelOptions,
        samplers: [SamplerKey; 4],
        state: PassState,
    ) -> Result<Self, String> {
        Self::compile_inner(microcode, def, options, samplers, state, true)
    }

    fn compile_inner(
        microcode: &[u8],
        def: &PixelShaderDef,
        options: &PixelOptions,
        samplers: [SamplerKey; 4],
        state: PassState,
        preserves: bool,
    ) -> Result<Self, String> {
        let program =
            vsh::Program::parse(microcode).map_err(|e| format!("gel vertex program: {e:?}"))?;
        if program.writes_constants() {
            return Err("gel vertex program writes constants".into());
        }
        if program.inputs_read() & !INPUTS != 0 {
            return Err("gel reads inherited vertex attributes".into());
        }
        let vs = vsh::translate(
            &program,
            &VertexOptions {
                provided_inputs: INPUTS,
                ..Default::default()
            },
        )
        .map_err(|e| format!("gel vertex translation: {e:?}"))?;
        let ps = psh::translate(def, options);
        if ps.discards && !preserves {
            return Err("gel can leave discarded pixels from the previous target".into());
        }
        if !ps.warnings.is_empty() {
            return Err(format!("gel pixel translation: {:?}", ps.warnings));
        }
        if state.depth_write
            || state.depth_compare != wgpu::CompareFunction::Always
            || options.w_buffer
        {
            return Err("gel requires depth".into());
        }
        // A fresh target must not depend on old target-13 pixels.
        if !preserves && (state.blend.is_some() || state.color_writes != wgpu::ColorWrites::ALL) {
            return Err("gel depends on previous target contents".into());
        }
        let wgsl = shaders::link(&vs, &ps).map_err(|e| format!("gel link: {e}"))?;
        Ok(Self {
            wgsl,
            dims: ps.textures,
            samplers,
            state,
        })
    }
}

/// Per-production uniform data. Buffers are allocated for the submission,
/// so reference/scaled encodes never overwrite each other's uniforms.
pub struct Frame {
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    pub vertices: [Vertex; 4],
}

/// A fresh output is valid only when all four transformed corners cover
/// the full target and survive clipping. Otherwise untouched pixels need
/// the previous contents of target 13, which this producer does not own.
pub fn full_target(microcode: &[u8], frame: &Frame, size: (u32, u32)) -> Result<(), String> {
    let program = vsh::Program::parse(microcode).map_err(|e| format!("gel coverage: {e:?}"))?;
    for (n, vertex) in frame.vertices.iter().enumerate() {
        let mut inputs = frame.vertex.defaults;
        inputs[..7].copy_from_slice(&vertex.registers);
        let mut constants = frame.vertex.constants;
        let pos = vsh::interp::run(&program, &inputs, &mut constants).regs[0];
        if pos.iter().any(|v| !v.is_finite()) || pos[3] <= 0.0 {
            return Err("gel has an unestablished full-target projection".into());
        }
        let surface = [size.0 as f32, size.1 as f32];
        let clip = match frame.vertex.w_clip {
            Some(range) => vsh::interp::fixup_w_buffer(pos, surface, range, frame.vertex.w_clamp),
            None => vsh::interp::fixup(pos, surface, frame.vertex.z_max),
        };
        let expected = [
            if n & 1 == 0 { -1.0 } else { 1.0 },
            if n & 2 == 0 { 1.0 } else { -1.0 },
        ];
        if clip.iter().any(|v| !v.is_finite())
            || clip[0] / clip[3] != expected[0]
            || clip[1] / clip[3] != expected[1]
            || clip[2] < 0.0
            || clip[2] > clip[3]
        {
            return Err("gel does not replace every target pixel".into());
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct Output {
    pub key: Key,
    pub view: wgpu::TextureView,
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
}

/// The packets' pipeline id ([`gfx::pipeline::PipelineKey::shader`]).
const GEL_PROGRAM: gfx::pipeline::ShaderId = gfx::pipeline::ShaderId(u32::MAX - 6);

/// A cached pipeline: its description for packets and the validated
/// pipeline (`None` when it was refused).
type Pipeline = (gfx::pipeline::PipelineKey, Arc<gfx::compile::PendingPipeline>);

pub struct Producer {
    program: Program,
    textures: wgpu::BindGroup,
    uniforms_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    pipelines: Mutex<HashMap<wgpu::TextureFormat, Pipeline>>,
}

impl Producer {
    /// Resolve every sampled image before recording anything. No default
    /// texture or partial gel is published on failure.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: Program,
        images: &[Option<Image>; 4],
    ) -> Result<Self, String> {
        let mut uploaded: [Option<wgpu::TextureView>; 4] = Default::default();
        for n in 0..4 {
            let Some(dim) = program.dims[n] else { continue };
            if dim != TextureDim::D2 {
                return Err(format!("gel stage {n} is not 2D"));
            }
            let image = images[n]
                .as_ref()
                .ok_or_else(|| format!("gel stage {n} is unresolved"))?;
            uploaded[n] = Some(texture(device, queue, image, "native gel bitmap"));
        }
        Self::resident(device, program, uploaded.each_ref().map(Option::as_ref))
    }

    /// Uses the resolved draw's exact resident views: no uploads or queue
    /// writes escape a cancelled two-copy recording transaction.
    pub fn resident(
        device: &wgpu::Device,
        program: Program,
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Result<Self, String> {
        let mut views = Vec::new();
        let mut samplers = Vec::new();
        let mut entries = Vec::new();
        for n in 0..4 {
            let Some(dim) = program.dims[n] else { continue };
            if dim != TextureDim::D2 {
                return Err(format!("gel stage {n} is not 2D"));
            }
            let view = stages[n].ok_or_else(|| format!("gel stage {n} is unresolved"))?;
            views.push((n, view.clone()));
            samplers.push(device.create_sampler(&program.samplers[n].descriptor()));
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
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
            label: Some("native gel textures"),
            entries: &entries,
        });
        let mut bindings = Vec::new();
        for ((n, view), sampler) in views.iter().zip(&samplers) {
            bindings.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            bindings.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native gel textures"),
            layout: &textures_layout,
            entries: &bindings,
        });
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native gel uniforms"),
            entries: &std::array::from_fn::<_, 2, _>(|n| wgpu::BindGroupLayoutEntry {
                binding: n as u32,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                // At dynamic offsets: the submission's arena for packets
                // ([`Producer::packet`]), 0 in a buffer of their own.
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(if n == 0 {
                        VertexUniforms::SIZE
                    } else {
                        PixelUniforms::SIZE
                    } as u64),
                },
                count: None,
            }),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native gel"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native gel"),
            source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
        });
        Ok(Self {
            program,
            textures,
            uniforms_layout,
            layout,
            module,
            pipelines: Mutex::new(HashMap::new()),
        })
    }

    /// Records the native equivalent of target 13. The caller invokes
    /// this at the original quad, before scheduling the consuming light.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: Key,
        frame: &Frame,
        format: wgpu::TextureFormat,
        guest_size: (u32, u32),
        size: (u32, u32),
    ) -> Result<Output, String> {
        if guest_size.0 == 0 || guest_size.1 == 0 || size.0 == 0 || size.1 == 0 {
            return Err("empty gel target".into());
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native target 13"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.draw(
            device,
            encoder,
            frame,
            &view,
            format,
            guest_size,
            size,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        )?;
        Ok(Output {
            key,
            view,
            guest_size,
            size,
        })
    }

    /// Write the native gel into the real guest target, retaining its
    /// previous pixels where blend/discard or colour masks require them.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_into(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        view: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        guest_size: (u32, u32),
        size: (u32, u32),
    ) -> Result<(), String> {
        self.draw(
            device,
            encoder,
            frame,
            view,
            format,
            guest_size,
            size,
            wgpu::LoadOp::Load,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        view: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        guest_size: (u32, u32),
        size: (u32, u32),
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Result<(), String> {
        if guest_size.0 == 0 || guest_size.1 == 0 || size.0 == 0 || size.1 == 0 {
            return Err("empty gel target".into());
        }
        let (vu, pu) = Self::uniforms(frame, guest_size, size);
        let uniform = |bytes: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native gel uniforms"),
                contents: bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let vertex = uniform(&vu);
        let pixel = uniform(&pu);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native gel uniforms"),
            layout: &self.uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: vertex.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pixel.as_entire_binding(),
                },
            ],
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native gel quad"),
            contents: bytemuck::cast_slice(&frame.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let (_, pipeline) = self.pipeline(device, format);
        let Some(pipeline) = pipeline.get().clone() else {
            return Err("gel pipeline refused".into());
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native target 13"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_blend_constant(self.program.state.blend_constant);
        pass.set_bind_group(0, &group, &[0, 0]);
        pass.set_bind_group(1, &self.textures, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.draw(0..4, 0..1);
        drop(pass);
        Ok(())
    }

    /// The native gel as a packet for the D3D pass already open on the
    /// real guest target (colour `format`, no depth): the same pipeline,
    /// groups and uniforms as [`Self::encode_into`], the uniforms, quad
    /// and its strip indices staged in the submission's `uploads`. No
    /// render pass or command buffer of its own.
    #[allow(clippy::too_many_arguments)]
    pub fn packet(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        frame: &Frame,
        format: wgpu::TextureFormat,
        guest_size: (u32, u32),
        size: (u32, u32),
    ) -> Result<gfx::RecordedDraw, String> {
        if guest_size.0 == 0 || guest_size.1 == 0 || size.0 == 0 || size.1 == 0 {
            return Err("empty gel target".into());
        }
        let (key, pipeline) = self.pipeline(device, format);
        if pipeline.get().is_none() {
            return Err("gel pipeline refused".into());
        }
        let (vu, pu) = Self::uniforms(frame, guest_size, size);
        let sizes = [VertexUniforms::SIZE, PixelUniforms::SIZE];
        let uniforms = uploads
            .uniform_run(device, queue, &self.uniforms_layout, sizes, 1, |_, v, p| {
                v.copy_from_slice(&vu);
                p.copy_from_slice(&pu);
            })
            .ok_or("gel uniforms unavailable")?;
        let quad: &[u8] = bytemuck::cast_slice(&frame.vertices);
        let (vertices, at) = uploads.stage(device, queue, quad);
        let strip: &[u8] = bytemuck::cast_slice(&[0u32, 1, 2, 3]);
        let (indices, index_at) = uploads.stage(device, queue, strip);
        Ok(gfx::RecordedDraw::prepared_indexed(
            &key,
            pipeline,
            vec![(vertices, at, quad.len() as u64)],
            (indices, index_at, strip.len() as u64),
            4,
            vec![
                gfx::BoundGroup {
                    index: 0,
                    group: uniforms.group.clone(),
                    offsets: uniforms.offsets(0).to_vec(),
                },
                gfx::BoundGroup::new(1, self.textures.clone()),
            ],
            &gfx::RenderState::default(),
            size,
            gfx::ScissorRect::full(size.0, size.1),
        )
        .with_blend_constant(self.program.state.blend_constant))
    }

    /// The vertex and pixel uniforms' bytes for a target `guest_size`
    /// guest pixels, `size` texels.
    fn uniforms(frame: &Frame, guest_size: (u32, u32), size: (u32, u32)) -> (Vec<u8>, Vec<u8>) {
        let mut vu = frame.vertex.clone();
        vu.surface = [guest_size.0 as f32, guest_size.1 as f32];
        vu.shift = crate::program::target_shift(guest_size, size);
        let mut vb = vec![0; VertexUniforms::SIZE];
        vu.write_bytes(&mut vb);
        let mut pb = vec![0; PixelUniforms::SIZE];
        let mut pixel = frame.pixel;
        pixel.stipple_scale = PixelUniforms::stipple_grid(guest_size, size);
        pixel.write_bytes(&mut pb);
        (vb, pb)
    }

    /// The pipeline for colour `format`, made once and validated: the
    /// quad as a triangle strip, drawn or indexed (`Uint32`, so a
    /// packet's strip indices restart nothing), no depth.
    fn pipeline(&self, device: &wgpu::Device, format: wgpu::TextureFormat) -> Pipeline {
        let mut pipelines = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        pipelines
            .entry(format)
            .or_insert_with(|| {
                let attributes: [wgpu::VertexAttribute; 7] =
                    std::array::from_fn(|n| wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: n as u64 * 16,
                        shader_location: n as u32,
                    });
                let primitive = wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    strip_index_format: Some(wgpu::IndexFormat::Uint32),
                    ..self.program.state.primitive
                };
                let color_target = wgpu::ColorTargetState {
                    format,
                    blend: self.program.state.blend,
                    write_mask: self.program.state.color_writes,
                };
                let pipeline = gfx::program::validated_resource(device, || {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("native target 13"),
                        layout: Some(&self.layout),
                        vertex: wgpu::VertexState {
                            module: &self.module,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[Some(wgpu::VertexBufferLayout {
                                array_stride: std::mem::size_of::<Vertex>() as u64,
                                step_mode: wgpu::VertexStepMode::Vertex,
                                attributes: &attributes,
                            })],
                        },
                        primitive,
                        depth_stencil: None,
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &self.module,
                            entry_point: Some("fs_main"),
                            compilation_options: Default::default(),
                            targets: &[Some(color_target.clone())],
                        }),
                        multiview_mask: None,
                        cache: None,
                    })
                });
                let key = gfx::pipeline::PipelineKey {
                    shader: GEL_PROGRAM,
                    vertex_buffers: Vec::new(),
                    primitive,
                    depth_stencil: None,
                    color_target,
                    samples: 1,
                };
                (key, gfx::compile::Pending::ready(pipeline))
            })
            .clone()
    }
}

/// GPU programs/textures are scoped to a map generation and the exact
/// resident views (including palette and streaming residency). Uniform
/// buffers remain per recording; only immutable resources are cached.
#[derive(Default)]
pub struct ResidentCache(Mutex<HashMap<ResidentKey, Arc<Producer>>>);

#[derive(Clone, PartialEq, Eq, Hash)]
struct ResidentKey {
    generation: u64,
    shader: String,
    state: String,
    samplers: [SamplerKey; 4],
    stages: [Option<wgpu::TextureView>; 4],
}

impl ResidentCache {
    pub fn producer(
        &self,
        device: &wgpu::Device,
        generation: u64,
        program: Program,
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Result<Arc<Producer>, String> {
        let key = ResidentKey {
            generation,
            shader: program.wgsl.clone(),
            state: format!("{:?}", program.state),
            samplers: program.samplers,
            stages: stages.map(|s| s.cloned()),
        };
        let mut cache = self.0.lock().map_err(|_| "gel program cache lock")?;
        if let Some(producer) = cache.get(&key) {
            return Ok(Arc::clone(producer));
        }
        // Validated here: a packet ([`Producer::packet`]) runs in the D3D
        // pass with no validation of its own.
        let producer =
            gfx::program::validated_resource(device, || Producer::resident(device, program, stages))
                .ok_or("gel textures refused")??;
        let producer = Arc::new(producer);
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&producer));
        Ok(producer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_matches_the_helper_register_writes() {
        let q = quad((32, 16), [0.25, 0.5]);
        assert_eq!(q[0].registers[0], [0.0, 0.0, 0.25, 0.5]);
        assert_eq!(q[1].registers[0], [32.0, 0.0, 0.25, 0.5]);
        assert_eq!(q[2].registers[0], [0.0, 16.0, 0.25, 0.5]);
        assert_eq!(q[3].registers[0], [32.0, 16.0, 0.25, 0.5]);
        for v in q {
            assert_eq!(
                v.registers[1],
                [v.registers[0][0] / 32.0, v.registers[0][1] / 16.0, 0.0, 1.0]
            );
            assert_eq!(v.registers[5], [0.0; 4]);
            assert_eq!(v.registers[6], [0.0; 4]);
        }
    }
}
