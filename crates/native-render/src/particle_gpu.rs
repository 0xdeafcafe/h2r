//! Dynamic particle/effect draws from a fully resolved issue-time snapshot.
//!
//! This is a lane-local admission interface, deliberately independent of
//! D3D's dispatcher. The adapter owns ordering, guest state continuation,
//! attachment coherency and atomic admission of *all* target copies. It
//! must retain the original draw until preparation succeeds for every copy.
//! No queue writes or reusable per-frame buffers: a prepared draw owns its
//! vertex/index/uniform uploads until the command encoder retains them.

use std::collections::HashMap;
use std::sync::Arc;

use shaders::{psh, vsh};
use wgpu::util::DeviceExt;

use crate::particles::{DrawIdentity, Vertex};

/// Host attachment identities include recreation generations. `copy` in
/// DrawIdentity distinguishes reference/display, even at equal sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetKey {
    pub color: u64,
    pub depth: u64,
    pub size: (u32, u32),
    pub color_format: wgpu::TextureFormat,
    pub depth_format: wgpu::TextureFormat,
}

/// Resident effective texture view, including the palette, signed-channel
/// conversion and mip chain D3D would sample. Never resolve a tag here.
#[derive(Clone, Debug)]
pub struct TextureBinding {
    pub device: u64,
    pub generation: u64,
    pub identity: u64,
    pub dimension: psh::TextureDim,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

#[derive(Clone, Debug)]
pub struct ShaderSnapshot {
    /// Read from the user's executable/map at runtime, never embedded.
    pub microcode: Vec<u8>,
    pub pixel: psh::PixelShaderDef,
    pub pixel_options: psh::PixelOptions,
    pub flat_shading: bool,
}

/// Already decoded effective register values, including inherited defaults.
/// This is the native declaration: sixteen FLOAT4 registers. The adapter
/// must compare source fetch against this expansion before admission.
#[derive(Clone, Debug)]
pub struct ImmediateDrawSnapshot {
    pub identity: DrawIdentity,
    pub target: TargetKey,
    pub shader: ShaderSnapshot,
    /// Uniforms resolved for this target copy (surface, shift, W clip,
    /// depth scale/bias, texture sizes, fog, stipple, constants, defaults).
    pub vertex_uniforms: vsh::VertexUniforms,
    pub pixel_uniforms: psh::PixelUniforms,
    pub textures: [Option<TextureBinding>; 4],
    pub vertices: Vec<Vertex>,
    /// Triangle list, in original primitive order; no material sorting.
    pub indices: Vec<u32>,
    pub primitive: wgpu::PrimitiveState,
    pub depth: wgpu::DepthStencilState,
    pub blend: Option<wgpu::BlendState>,
    pub color_writes: wgpu::ColorWrites,
    pub blend_constant: wgpu::Color,
    pub stencil_reference: u32,
    /// Host texels; already intersected with the target and guest scissor.
    pub scissor: [u32; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    Identity,
    Target,
    Scissor,
    Geometry,
    State,
    Texture(usize),
    Shader(String),
}

impl ImmediateDrawSnapshot {
    /// Checks independent of a GPU. Unknown attachment/depth state, partial
    /// triangles, texture generations and mismatched clip modes fail closed.
    pub fn validate(&self) -> Result<(), Refused> {
        if self.identity.device == 0 || self.identity.generation == 0 {
            return Err(Refused::Identity);
        }
        if self.target.color == 0
            || self.target.depth == 0
            || self.target.size.0 == 0
            || self.target.size.1 == 0
            || self.depth.format != self.target.depth_format
        {
            return Err(Refused::Target);
        }
        let [x, y, w, h] = self.scissor;
        if w == 0
            || h == 0
            || x.checked_add(w).is_none_or(|r| r > self.target.size.0)
            || y.checked_add(h).is_none_or(|b| b > self.target.size.1)
        {
            return Err(Refused::Scissor);
        }
        if self.vertices.is_empty()
            || self.vertices.len() > 65536
            || self.indices.is_empty()
            || self.indices.len() > 393216
            || !self.indices.len().is_multiple_of(3)
            || self
                .indices
                .iter()
                .any(|&i| i as usize >= self.vertices.len())
        {
            return Err(Refused::Geometry);
        }
        if self.primitive.topology != wgpu::PrimitiveTopology::TriangleList
            || self.primitive.strip_index_format.is_some()
            || self.primitive.polygon_mode != wgpu::PolygonMode::Fill
            || self.primitive.conservative
            || self.depth.depth_write_enabled.is_none()
            || self.depth.depth_compare.is_none()
            || self.shader.pixel_options.flat_shading != self.shader.flat_shading
            || self.shader.pixel_options.w_buffer != self.vertex_uniforms.w_clip.is_some()
            || self
                .vertex_uniforms
                .surface
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.)
            || !self.vertex_uniforms.z_max.is_finite()
            || self.vertex_uniforms.z_max <= 0.
        {
            return Err(Refused::State);
        }
        for (n, texture) in self.textures.iter().enumerate() {
            if let Some(t) = texture {
                if t.device != self.identity.device
                    || t.generation != self.identity.generation
                    || t.identity == 0
                    || self.shader.pixel_options.texture_dims[n] != Some(t.dimension)
                {
                    return Err(Refused::Texture(n));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ShaderKey {
    microcode: Vec<u8>,
    pixel: psh::PixelShaderDef,
    options: psh::PixelOptions,
    flat: bool,
}

struct Program {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    uniforms: wgpu::BindGroupLayout,
    textures: wgpu::BindGroupLayout,
    locations: u16,
    sampled: [Option<psh::TextureDim>; 4],
}

/// The native translated programs and pipelines, scoped to one host device.
/// Drop/recreate on map generation changes to release obsolete programs.
pub struct ParticleRenderer {
    device: u64,
    programs: HashMap<ShaderKey, Arc<Program>>,
    pipelines: HashMap<(ShaderKey, String), wgpu::RenderPipeline>,
}

impl ParticleRenderer {
    pub fn new(device: u64) -> Self {
        Self {
            device,
            programs: HashMap::new(),
            pipelines: HashMap::new(),
        }
    }

    /// Resolves everything and uploads owned geometry/uniforms, without
    /// recording a render pass or changing an attachment. Prepare *every*
    /// required target copy before the adapter commits any replacement.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        s: &ImmediateDrawSnapshot,
    ) -> Result<PreparedDraw, Refused> {
        s.validate()?;
        if self.device != s.identity.device {
            return Err(Refused::Identity);
        }
        if s.primitive.unclipped_depth
            && !device
                .features()
                .contains(wgpu::Features::DEPTH_CLIP_CONTROL)
        {
            return Err(Refused::State);
        }
        let key = ShaderKey {
            microcode: s.shader.microcode.clone(),
            pixel: s.shader.pixel.clone(),
            options: s.shader.pixel_options,
            flat: s.shader.flat_shading,
        };
        let program = if let Some(p) = self.programs.get(&key) {
            Arc::clone(p)
        } else {
            let p = Arc::new(compile(device, &key)?);
            self.programs.insert(key.clone(), Arc::clone(&p));
            p
        };
        // Check every sampled level before pipeline/groups/uploads. A missing
        // stage cannot turn into a skipped range in the recording loop.
        for (n, dim) in program.sampled.iter().enumerate() {
            if let Some(dim) = dim {
                let t = s.textures[n].as_ref().ok_or(Refused::Texture(n))?;
                if t.dimension != *dim {
                    return Err(Refused::Texture(n));
                }
            }
        }
        let pipeline_key = (
            key,
            format!(
                "{:?}/{:?}/{:?}/{:?}/{:?}",
                s.target.color_format, s.primitive, s.depth, s.blend, s.color_writes
            ),
        );
        let pipeline = self
            .pipelines
            .entry(pipeline_key)
            .or_insert_with(|| {
                let attributes: Vec<_> = (0..16u64)
                    .filter(|&r| program.locations & (1u16 << r) != 0)
                    .map(|r| wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: r * 16,
                        shader_location: r as u32,
                    })
                    .collect();
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("native particle/effect pipeline"),
                    layout: Some(&program.layout),
                    vertex: wgpu::VertexState {
                        module: &program.module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        })],
                    },
                    primitive: s.primitive,
                    depth_stencil: Some(s.depth.clone()),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &program.module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: s.target.color_format,
                            blend: s.blend,
                            write_mask: s.color_writes,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .clone();
        let buffer = |label, bytes: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage,
            })
        };
        let vertex_uniform = buffer(
            "particle vertex uniforms",
            &s.vertex_uniforms.to_bytes(),
            wgpu::BufferUsages::UNIFORM,
        );
        let pixel_uniform = buffer(
            "particle pixel uniforms",
            &s.pixel_uniforms.to_bytes(),
            wgpu::BufferUsages::UNIFORM,
        );
        let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle uniforms"),
            layout: &program.uniforms,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: vertex_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pixel_uniform.as_entire_binding(),
                },
            ],
        });
        let mut entries = Vec::new();
        for (n, dim) in program.sampled.iter().enumerate() {
            if dim.is_none() {
                continue;
            }
            let t = s.textures[n]
                .as_ref()
                .expect("validated before any recording");
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n as u32,
                resource: wgpu::BindingResource::TextureView(&t.view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(&t.sampler),
            });
        }
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle resident textures"),
            layout: &program.textures,
            entries: &entries,
        });
        Ok(PreparedDraw {
            identity: s.identity,
            target: s.target,
            pipeline,
            uniforms,
            textures,
            vertices: buffer(
                "particle dynamic vertices",
                bytemuck::cast_slice(&s.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            indices: buffer(
                "particle dynamic indices",
                bytemuck::cast_slice(&s.indices),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: s.indices.len() as u32,
            blend_constant: s.blend_constant,
            stencil_reference: s.stencil_reference,
            scissor: s.scissor,
        })
    }
}

fn compile(device: &wgpu::Device, key: &ShaderKey) -> Result<Program, Refused> {
    let program = vsh::Program::parse(&key.microcode)
        .map_err(|e| Refused::Shader(format!("vertex: {e:?}")))?;
    let vertex = vsh::translate(
        &program,
        &vsh::VertexOptions {
            provided_inputs: u16::MAX,
            flat_shading: key.flat,
            ..Default::default()
        },
    )
    .map_err(|e| Refused::Shader(format!("vertex: {e:?}")))?;
    let pixel = psh::translate(&key.pixel, &key.options);
    if !pixel.warnings.is_empty() {
        return Err(Refused::Shader(pixel.warnings.join("; ")));
    }
    // Shadow bindings need depth/comparison layouts. No silent float view.
    if key.options.shadow.iter().any(Option::is_some) {
        return Err(Refused::State);
    }
    let wgsl = shaders::link(&vertex, &pixel).map_err(|e| Refused::Shader(e.to_string()))?;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native particles"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let uniform = |binding, size| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    };
    let uniforms = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("particle uniforms"),
        entries: &[
            uniform(0, vsh::VertexUniforms::SIZE as u64),
            uniform(1, psh::PixelUniforms::SIZE as u64),
        ],
    });
    let mut entries = Vec::new();
    for (n, dim) in pixel.textures.iter().enumerate() {
        let Some(dim) = dim else {
            continue;
        };
        let view_dimension = match dim {
            psh::TextureDim::D2 => wgpu::TextureViewDimension::D2,
            psh::TextureDim::D3 => wgpu::TextureViewDimension::D3,
            psh::TextureDim::Cube => wgpu::TextureViewDimension::Cube,
        };
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 2 * n as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension,
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
    let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("particle textures"),
        entries: &entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("particle pipeline"),
        bind_group_layouts: &[Some(&uniforms), Some(&textures)],
        immediate_size: 0,
    });
    Ok(Program {
        module,
        layout,
        uniforms,
        textures,
        locations: vertex.locations,
        sampled: pixel.textures,
    })
}

/// All resources resolved; one immutable target copy, consumed by recording.
/// No public fields: callers cannot remove an upload after admission.
pub struct PreparedDraw {
    identity: DrawIdentity,
    target: TargetKey,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::BindGroup,
    textures: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    blend_constant: wgpu::Color,
    stencil_reference: u32,
    scissor: [u32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub identity: DrawIdentity,
    pub indices: u32,
    pub draws: u32,
}

impl PreparedDraw {
    /// The adapter must validate all target keys before recording any copy.
    pub fn matches(&self, identity: DrawIdentity, target: TargetKey) -> bool {
        self.identity == identity && self.target == target
    }

    /// Records exactly one draw or refuses before touching the encoder.
    /// Only a successful receipt allows the adapter to suppress this copy.
    pub fn record(
        self,
        encoder: &mut wgpu::CommandEncoder,
        identity: DrawIdentity,
        target: TargetKey,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) -> Result<Recorded, Refused> {
        if !self.matches(identity, target) {
            return Err(Refused::Identity);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native particles/effects"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: target
                    .depth_format
                    .has_stencil_aspect()
                    .then_some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // The translated program already applies the guest viewport using
        // its constants. As D3D, rasterize against the whole host surface.
        pass.set_viewport(0., 0., target.size.0 as f32, target.size.1 as f32, 0., 1.);
        let [x, y, w, h] = self.scissor;
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniforms, &[]);
        pass.set_bind_group(1, &self.textures, &[]);
        pass.set_blend_constant(self.blend_constant);
        pass.set_stencil_reference(self.stencil_reference);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
        drop(pass);
        Ok(Recorded {
            identity,
            indices: self.index_count,
            draws: 1,
        })
    }
}
