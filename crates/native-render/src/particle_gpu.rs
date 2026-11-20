//! Dynamic particle/effect draws from a fully resolved issue-time snapshot.
//!
//! This is a lane-local admission interface, deliberately independent of
//! D3D's dispatcher. The adapter owns ordering, guest state continuation,
//! attachment coherency and atomic admission of *all* target copies. It
//! must retain the original draw until preparation succeeds for every copy.
//! No queue writes or reusable per-frame buffers: a prepared draw owns its
//! vertex/index/uniform uploads until the command encoder retains them.
//!
//! [`ParticleRenderer::draws`] is the cheaper form: one packet for the D3D
//! render pass already open on the game's targets, its geometry and
//! uniforms staged in the submission's upload arena, its pipeline and
//! texture group cached across frames. [`ParticleRenderer::prepare`] keeps
//! the self-contained form (own buffers, own render pass) as the fallback.

use std::collections::HashMap;
use std::sync::Arc;

use gfx::compile::{Pending, PendingPipeline};
use gfx::pipeline::{PipelineKey, ShaderId};
use gfx::{BoundGroup, RecordedDraw, ScissorRect};
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
    /// Generated contrail/effect streams retain raw FLOAT/BGRA fetch.
    /// When present, vertices may be empty; the raw stream owns the count.
    pub raw_vertices: Option<RawVertices>,
    /// Triangle list or strip in original order; no material sorting.
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

/// A validated raw generated stream. BGRA stays packed for GPU fetch, as
/// on D3D; CPU normalization can round differently. Live declaration
/// locations are supplied by the adapter, never inferred from semantics.
#[derive(Clone, Debug)]
pub struct RawVertices {
    kind: crate::effect_geometry::Kind,
    bytes: Vec<u8>,
    stride: u64,
    attributes: Vec<wgpu::VertexAttribute>,
    provided: u16,
}
impl RawVertices {
    pub fn new(
        kind: crate::effect_geometry::Kind,
        bytes: Vec<u8>,
        mut attributes: Vec<wgpu::VertexAttribute>,
    ) -> Result<Self, Refused> {
        use crate::effect_geometry::Kind;
        use wgpu::VertexFormat as F;
        let (stride, formats): (u64, &[(u64, F)]) = match kind {
            Kind::Contrail => (
                24,
                &[(0, F::Float32x3), (12, F::Float32x2), (20, F::Unorm8x4Bgra)],
            ),
            Kind::EffectQuad => (20, &[(0, F::Float32x3), (12, F::Float32x2)]),
        };
        if bytes.is_empty()
            || bytes.len() > 8192
            || !(bytes.len() as u64).is_multiple_of(stride)
            || attributes.len() != formats.len()
        {
            return Err(Refused::Geometry);
        }
        attributes.sort_by_key(|a| a.offset);
        let mut provided = 0u16;
        for (a, &(offset, format)) in attributes.iter().zip(formats) {
            if a.offset != offset
                || a.format != format
                || a.shader_location >= 16
                || provided & (1u16 << a.shader_location) != 0
            {
                return Err(Refused::Geometry);
            }
            provided |= 1u16 << a.shader_location;
        }
        let count = bytes.len() as u64 / stride;
        if match kind {
            Kind::Contrail => count < 4 || !count.is_multiple_of(2),
            Kind::EffectQuad => count != 4,
        } {
            return Err(Refused::Geometry);
        }
        Ok(Self {
            kind,
            bytes,
            stride,
            attributes,
            provided,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn kind(&self) -> crate::effect_geometry::Kind {
        self.kind
    }
    pub fn vertex_count(&self) -> usize {
        self.bytes.len() / self.stride as usize
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    Identity,
    Target,
    Scissor,
    Geometry,
    Effect(&'static str),
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
        let vertex_count = self
            .raw_vertices
            .as_ref()
            .map_or(self.vertices.len(), RawVertices::vertex_count);
        let strip = self.primitive.topology == wgpu::PrimitiveTopology::TriangleStrip;
        let lines = self.primitive.topology == wgpu::PrimitiveTopology::LineList;
        if vertex_count == 0
            || vertex_count > 65536
            || self.indices.is_empty()
            || self.indices.len() > 393216
            || if strip {
                self.indices.len() < 3
            } else if lines {
                !self.indices.len().is_multiple_of(2)
            } else {
                !self.indices.len().is_multiple_of(3)
            }
            || self.indices.iter().any(|&i| i as usize >= vertex_count)
        {
            return Err(Refused::Geometry);
        }
        if (!strip && !lines && self.primitive.topology != wgpu::PrimitiveTopology::TriangleList)
            || self.primitive.strip_index_format != strip.then_some(wgpu::IndexFormat::Uint32)
            || self.primitive.polygon_mode != wgpu::PolygonMode::Fill
            || self.primitive.conservative
            || self.depth.depth_write_enabled.is_none()
            || self.depth.depth_compare.is_none()
            || self.shader.pixel_options.flat_shading != self.shader.flat_shading
            || (self.shader.pixel_options.w_buffer && self.vertex_uniforms.w_clip.is_none())
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
    provided: u16,
}

struct Program {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// The renderer's one uniforms layout (dynamic offsets), shared by every
    /// program so a submission's pooled uniform group is reused across them.
    uniforms: wgpu::BindGroupLayout,
    textures: wgpu::BindGroupLayout,
    locations: u16,
    sampled: [Option<psh::TextureDim>; 4],
    /// The pixel program can discard: the backend then keeps the draw out
    /// of a merged pass, as it does for the D3D draw it replaces.
    discards: bool,
}

/// Names the particle pipelines in a recorded packet's description.
const PARTICLE_PROGRAM: ShaderId = ShaderId(u32::MAX - 1);
/// Texture groups kept before the cache starts over: bounded so views of
/// retired textures are not held for ever.
const GROUP_LIMIT: usize = 256;

/// A pipeline made once: the pipeline itself for the self-contained pass,
/// and the same pipeline with its description for packets.
#[derive(Clone)]
struct Cached {
    pipeline: wgpu::RenderPipeline,
    described: PipelineKey,
    pending: Arc<PendingPipeline>,
}

/// What a renderer made so far, for tests of a steady scene: a second
/// identical frame must leave these unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Created {
    pub programs: u64,
    pub pipelines: u64,
    pub texture_groups: u64,
    pub samplers: u64,
}

/// The native translated programs and pipelines, scoped to one host device.
/// Drop/recreate on map generation changes to release obsolete programs.
pub struct ParticleRenderer {
    device: u64,
    programs: HashMap<ShaderKey, Result<Arc<Program>, Refused>>,
    pipelines: HashMap<(ShaderKey, String), Cached>,
    uniforms: Option<wgpu::BindGroupLayout>,
    samplers: HashMap<gfx::translate::SamplerKey, wgpu::Sampler>,
    groups: HashMap<(wgpu::BindGroupLayout, Vec<(u32, wgpu::TextureView, wgpu::Sampler)>), wgpu::BindGroup>,
    created: Created,
}

impl ParticleRenderer {
    pub fn new(device: u64) -> Self {
        Self {
            device,
            programs: HashMap::new(),
            pipelines: HashMap::new(),
            uniforms: None,
            samplers: HashMap::new(),
            groups: HashMap::new(),
            created: Created::default(),
        }
    }

    /// What this renderer created so far.
    pub fn created(&self) -> Created {
        self.created
    }

    /// The sampler for `key`, made once: a texture binding built from it
    /// keeps the same identity across frames, so its group stays cached.
    pub fn sampler(&mut self, device: &wgpu::Device, key: &gfx::translate::SamplerKey) -> wgpu::Sampler {
        let created = &mut self.created;
        self.samplers
            .entry(*key)
            .or_insert_with(|| {
                created.samplers += 1;
                device.create_sampler(&key.descriptor())
            })
            .clone()
    }

    /// Validates, then finds or makes the program and pipeline, and checks
    /// every sampled level. Nothing is uploaded and no group is made.
    fn resolve(
        &mut self,
        device: &wgpu::Device,
        s: &ImmediateDrawSnapshot,
    ) -> Result<(Arc<Program>, Cached), Refused> {
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
        let mut code = s.shader.pixel.clone();
        code.constant0 = [0; 8];
        code.constant1 = [0; 8];
        code.final_constant0 = 0;
        code.final_constant1 = 0;
        let key = ShaderKey {
            microcode: s.shader.microcode.clone(),
            pixel: code,
            options: s.shader.pixel_options,
            flat: s.shader.flat_shading,
            provided: s.raw_vertices.as_ref().map_or(u16::MAX, |r| r.provided),
        };
        let uniforms = self
            .uniforms
            .get_or_insert_with(|| crate::light_gpu::uniforms_layout(device))
            .clone();
        let created = &mut self.created;
        let program = self
            .programs
            .entry(key.clone())
            .or_insert_with(|| {
                created.programs += 1;
                compile(device, &key, &uniforms).map(Arc::new)
            })
            .clone()?;
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
                "{:?}/{:?}/{:?}/{:?}/{:?}/{:?}",
                s.target.color_format,
                s.primitive,
                s.depth,
                s.blend,
                s.color_writes,
                s.raw_vertices.as_ref().map(|r| (r.stride, &r.attributes))
            ),
        );
        if let Some(cached) = self.pipelines.get(&pipeline_key) {
            return Ok((program, cached.clone()));
        }
        let attributes: Vec<_> = if let Some(raw) = &s.raw_vertices {
            raw.attributes
                .iter()
                .filter(|a| program.locations & (1u16 << a.shader_location) != 0)
                .copied()
                .collect()
        } else {
            (0..16u64)
                .filter(|&r| program.locations & (1u16 << r) != 0)
                .map(|r| wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: r * 16,
                    shader_location: r as u32,
                })
                .collect()
        };
        let color_target = wgpu::ColorTargetState {
            format: s.target.color_format,
            blend: s.blend,
            write_mask: s.color_writes,
        };
        let pipeline = gfx::program::validated_resource(device, || {
            gfx::churn::create_render_pipeline(
                device,
                &wgpu::RenderPipelineDescriptor {
                    label: Some("native particle/effect pipeline"),
                    layout: Some(&program.layout),
                    vertex: wgpu::VertexState {
                        module: &program.module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: s
                                .raw_vertices
                                .as_ref()
                                .map_or(std::mem::size_of::<Vertex>() as u64, |r| r.stride),
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
                        targets: &[Some(color_target.clone())],
                    }),
                    multiview_mask: None,
                    cache: None,
                },
            )
        })
        .ok_or(Refused::State)?;
        self.created.pipelines += 1;
        let cached = Cached {
            described: PipelineKey {
                shader: PARTICLE_PROGRAM,
                vertex_buffers: Vec::new(),
                primitive: s.primitive,
                depth_stencil: Some(s.depth.clone()),
                color_target,
                samples: 1,
            },
            pending: Pending::ready(Some(pipeline.clone())),
            pipeline,
        };
        self.pipelines.insert(pipeline_key, cached.clone());
        Ok((program, cached))
    }

    /// The group of `s`'s resident textures for `program`, made once per
    /// distinct views and samplers.
    fn texture_group(&mut self, device: &wgpu::Device, program: &Program, s: &ImmediateDrawSnapshot) -> wgpu::BindGroup {
        let bound: Vec<(u32, wgpu::TextureView, wgpu::Sampler)> = program
            .sampled
            .iter()
            .enumerate()
            .filter(|(_, dim)| dim.is_some())
            .map(|(n, _)| {
                let t = s.textures[n].as_ref().expect("validated before any recording");
                (n as u32, t.view.clone(), t.sampler.clone())
            })
            .collect();
        let key = (program.textures.clone(), bound);
        if let Some(group) = self.groups.get(&key) {
            gfx::churn::native_cache(true);
            return group.clone();
        }
        gfx::churn::native_cache(false);
        let mut entries = Vec::new();
        for (n, view, sampler) in &key.1 {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n + 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let group = gfx::churn::create_bind_group(
            device,
            &wgpu::BindGroupDescriptor {
                label: Some("particle resident textures"),
                layout: &program.textures,
                entries: &entries,
            },
        );
        if self.groups.len() >= GROUP_LIMIT {
            self.groups.clear();
        }
        self.created.texture_groups += 1;
        self.groups.insert(key, group.clone());
        group
    }

    /// One packet for the D3D render pass open on the target's attachments:
    /// geometry staged in `uploads`, uniforms at dynamic offsets in a run
    /// of the same arena, pipeline and texture group cached. It draws what
    /// [`PreparedDraw::record`] draws: the whole surface as viewport, the
    /// snapshot's scissor, blend constant and stencil reference. A
    /// The flag is whether the pixel program discards (`ExternalDraws::discards`).
    /// A stencil reference above 255 (outside what a packet carries) is
    /// `Refused::State`; the caller then takes [`ParticleRenderer::prepare`].
    pub fn draws(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        s: &ImmediateDrawSnapshot,
    ) -> Result<(RecordedDraw, bool), Refused> {
        let reference = u8::try_from(s.stencil_reference).map_err(|_| Refused::State)?;
        let (program, cached) = self.resolve(device, s)?;
        let textures = self.texture_group(device, &program, s);
        let run = uploads
            .uniform_run(
                device,
                queue,
                &program.uniforms,
                [vsh::VertexUniforms::SIZE, psh::PixelUniforms::SIZE],
                1,
                |_, v, p| {
                    s.vertex_uniforms.write_bytes(v);
                    s.pixel_uniforms.write_bytes(p);
                },
            )
            .ok_or(Refused::State)?;
        let vertex_bytes: &[u8] = s
            .raw_vertices
            .as_ref()
            .map_or_else(|| bytemuck::cast_slice(&s.vertices), |r| r.bytes.as_slice());
        let index_bytes: &[u8] = bytemuck::cast_slice(&s.indices);
        let (vertices, vertex_at) = uploads.stage(device, queue, vertex_bytes);
        let (indices, index_at) = uploads.stage(device, queue, index_bytes);
        let groups = vec![
            BoundGroup { index: 0, group: run.group.clone(), offsets: run.offsets(0).to_vec() },
            BoundGroup::new(1, textures),
        ];
        let render = gfx::state::RenderState {
            stencil: gfx::state::StencilState { reference, ..Default::default() },
            ..Default::default()
        };
        let [x, y, width, height] = s.scissor;
        let draw = RecordedDraw::prepared_indexed(
            &cached.described,
            cached.pending,
            vec![(vertices, vertex_at, vertex_bytes.len() as u64)],
            (indices, index_at, index_bytes.len() as u64),
            s.indices.len() as u32,
            groups,
            &render,
            s.target.size,
            ScissorRect { x, y, width, height },
        )
        .with_blend_constant(s.blend_constant);
        Ok((draw, program.discards))
    }

    /// Resolves everything and uploads owned geometry/uniforms, without
    /// recording a render pass or changing an attachment. Prepare *every*
    /// required target copy before the adapter commits any replacement.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        s: &ImmediateDrawSnapshot,
    ) -> Result<PreparedDraw, Refused> {
        let (program, cached) = self.resolve(device, s)?;
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
        let textures = self.texture_group(device, &program, s);
        Ok(PreparedDraw {
            identity: s.identity,
            target: s.target,
            pipeline: cached.pipeline,
            uniforms,
            textures,
            vertices: buffer(
                "particle dynamic vertices",
                s.raw_vertices
                    .as_ref()
                    .map_or_else(|| bytemuck::cast_slice(&s.vertices), |r| r.bytes.as_slice()),
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

fn compile(
    device: &wgpu::Device,
    key: &ShaderKey,
    uniforms: &wgpu::BindGroupLayout,
) -> Result<Program, Refused> {
    let program = vsh::Program::parse(&key.microcode)
        .map_err(|e| Refused::Shader(format!("vertex: {e:?}")))?;
    if program.writes_constants() {
        return Err(Refused::Shader("vertex program writes constants".into()));
    }
    let vertex = vsh::translate(
        &program,
        &vsh::VertexOptions {
            provided_inputs: key.provided,
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
    shaders::validate(&wgsl).map_err(|e| Refused::Shader(e.to_string()))?;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native particles"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
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
        bind_group_layouts: &[Some(uniforms), Some(&textures)],
        immediate_size: 0,
    });
    Ok(Program {
        module,
        layout,
        uniforms: uniforms.clone(),
        textures,
        locations: vertex.locations,
        sampled: pixel.textures,
        discards: pixel.discards,
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
        pass.set_bind_group(0, &self.uniforms, &[0, 0]);
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
