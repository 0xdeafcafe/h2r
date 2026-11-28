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
//!
//! Texture groups are kept while draws use them ([`crate::texture_groups`]):
//! the caller names each draw's frame ([`ParticleRenderer::begin_frame`]),
//! and a group unused for a few frames is dropped with the views it holds.
//! A view of a frame-buffer copy or depth alias is new every frame, so a
//! group of one never outlives the frames that drew with it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use gfx::compile::{Pending, PendingPipeline};
use gfx::pipeline::{PipelineKey, ShaderId};
use gfx::{BoundGroup, RecordedDraw, ScissorRect};
use shaders::{psh, vsh};
use wgpu::util::DeviceExt;

use crate::particles::{DrawIdentity, Vertex};
pub use crate::texture_groups::Held;
use crate::texture_groups::TextureGroups;

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
    pub microcode: gfx::guest_program::Microcode,
    pub pixel: psh::PixelShaderDef,
    pub pixel_options: psh::PixelOptions,
    pub flat_shading: bool,
    pub input_fetch: [vsh::InputFetch; 16],
}

/// Already decoded effective register values, including inherited defaults.
/// This is the native declaration: sixteen FLOAT4 registers. The adapter
/// must compare source fetch against this expansion before admission.
#[derive(Clone, Debug)]
pub struct ImmediateDrawSnapshot<'a> {
    pub identity: DrawIdentity,
    pub target: TargetKey,
    pub shader: ShaderSnapshot,
    /// Uniforms resolved for this target copy (surface, shift, W clip,
    /// depth scale/bias, texture sizes, fog, stipple, constants, defaults).
    pub vertex_uniforms: vsh::VertexUniforms,
    pub pixel_uniforms: psh::PixelUniforms,
    pub textures: [Option<TextureBinding>; 4],
    pub vertices: Cow<'a, [Vertex]>,
    /// Generated contrail/effect streams retain raw FLOAT/BGRA fetch.
    /// When present, vertices may be empty; the raw stream owns the count.
    pub raw_vertices: Option<Cow<'a, RawVertices>>,
    /// Triangle list or strip in original order; no material sorting.
    pub indices: Cow<'a, [u32]>,
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
    bytes: Arc<[u8]>,
    stride: u64,
    attributes: Vec<wgpu::VertexAttribute>,
    provided: u16,
}
impl RawVertices {
    pub fn new(
        kind: crate::effect_geometry::Kind,
        bytes: impl Into<Arc<[u8]>>,
        mut attributes: Vec<wgpu::VertexAttribute>,
    ) -> Result<Self, Refused> {
        use crate::effect_geometry::Kind;
        use wgpu::VertexFormat as F;
        let bytes = bytes.into();
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
            return Err(Refused::Geometry("raw_bytes"));
        }
        attributes.sort_by_key(|a| a.offset);
        let mut provided = 0u16;
        for (a, &(offset, format)) in attributes.iter().zip(formats) {
            if a.offset != offset
                || a.format != format
                || a.shader_location >= 16
                || provided & (1u16 << a.shader_location) != 0
            {
                return Err(Refused::Geometry("raw_attributes"));
            }
            provided |= 1u16 << a.shader_location;
        }
        let count = bytes.len() as u64 / stride;
        if match kind {
            Kind::Contrail => count < 4 || !count.is_multiple_of(2),
            Kind::EffectQuad => count != 4,
        } {
            return Err(Refused::Geometry("raw_vertex_count"));
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

/// Why the native path cannot draw a snapshot. Every other shape of a
/// snapshot draws: see [`ImmediateDrawSnapshot::validate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The named geometry check that failed; the particle admission
    /// counts and samples each name apart.
    Geometry(&'static str),
    Effect(&'static str),
    State,
    Shader(Arc<str>),
}

/// What a snapshot draws once validated: the scissor clamped to the
/// target and the indices of its whole primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drawn {
    pub scissor: [u32; 4],
    pub indices: u32,
}

impl ImmediateDrawSnapshot<'_> {
    /// What the draw rasterises, as D3D rasterises it. `None` draws
    /// nothing: an empty scissor, or no whole primitive.
    ///
    /// - The scissor is clamped to the target, as D3D clamps it to the
    ///   surface.
    /// - A trailing partial primitive is dropped, as D3D drops it.
    /// - Point lists are `Refused::State`: a point sprite is a quad on the
    ///   NV2A, which a wgpu point is not.
    ///
    /// The adapter's own invariants (identity, target handles, texture
    /// bindings, indices in range, uniforms from one resolve) are asserted:
    /// a break is a bug in the adapter, never a draw left to the guest.
    pub fn validate(&self) -> Result<Option<Drawn>, Refused> {
        assert!(
            self.identity.device != 0 && self.identity.generation != 0,
            "the adapter names every draw's device and generation"
        );
        assert!(
            self.target.color != 0
                && self.target.depth != 0
                && self.target.size.0 != 0
                && self.target.size.1 != 0,
            "the adapter draws only into bound colour and depth targets"
        );
        let vertex_count = self
            .raw_vertices
            .as_deref()
            .map_or(self.vertices.len(), RawVertices::vertex_count);
        assert!(
            self.indices.iter().all(|&i| (i as usize) < vertex_count),
            "the adapter's index lists stay inside their vertices"
        );
        assert!(
            self.shader.pixel_options.flat_shading == self.shader.flat_shading
                && (!self.shader.pixel_options.w_buffer || self.vertex_uniforms.w_clip.is_some())
                && self.vertex_uniforms.surface.iter().all(|v| v.is_finite() && *v > 0.)
                && self.vertex_uniforms.z_max.is_finite()
                && self.vertex_uniforms.z_max > 0.,
            "the program and its uniforms come from one resolve"
        );
        for (n, t) in self.textures.iter().enumerate() {
            if let Some(t) = t {
                assert!(
                    t.device == self.identity.device
                        && t.generation == self.identity.generation
                        && t.identity != 0
                        && self.shader.pixel_options.texture_dims[n] == Some(t.dimension),
                    "stage {n} is bound for this draw"
                );
            }
        }
        use wgpu::PrimitiveTopology as T;
        let whole = self.indices.len() as u32;
        let indices = match self.primitive.topology {
            T::TriangleList => whole - whole % 3,
            T::LineList => whole - whole % 2,
            T::TriangleStrip if whole >= 3 => whole,
            T::LineStrip if whole >= 2 => whole,
            T::TriangleStrip | T::LineStrip => 0,
            T::PointList => return Err(Refused::State),
        };
        let (width, height) = self.target.size;
        let [x, y, w, h] = self.scissor;
        let (left, top) = (x.min(width), y.min(height));
        let right = x.saturating_add(w).min(width);
        let bottom = y.saturating_add(h).min(height);
        let scissor = [left, top, right - left, bottom - top];
        if vertex_count == 0 || indices == 0 || scissor[2] == 0 || scissor[3] == 0 {
            return Ok(None);
        }
        Ok(Some(Drawn { scissor, indices }))
    }

    /// The primitive state the pipeline is made with: strips restart on
    /// 32-bit indices, as the index buffer is.
    fn primitive(&self) -> wgpu::PrimitiveState {
        use wgpu::PrimitiveTopology as T;
        let strip = matches!(self.primitive.topology, T::TriangleStrip | T::LineStrip);
        wgpu::PrimitiveState {
            strip_index_format: strip.then_some(wgpu::IndexFormat::Uint32),
            ..self.primitive
        }
    }

    /// The depth state the pipeline is made with, on the target's format.
    fn depth(&self) -> wgpu::DepthStencilState {
        wgpu::DepthStencilState {
            format: self.target.depth_format,
            ..self.depth.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ShaderKey {
    microcode: gfx::guest_program::Microcode,
    pixel: psh::PixelShaderDef,
    options: psh::PixelOptions,
    flat: bool,
    input_fetch: [vsh::InputFetch; 16],
    provided: u16,
}

struct Program {
    id: u64,
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

static GROUP_BYTES: mem_watch::Counter =
    mem_watch::Counter::new("native.particle_group_texture_bytes");

type GroupKey = (
    wgpu::BindGroupLayout,
    [Option<(wgpu::TextureView, wgpu::Sampler)>; 4],
);

#[derive(PartialEq, Eq, Hash)]
struct RenderPipelineKey {
    program: u64,
    primitive: wgpu::PrimitiveState,
    depth: wgpu::DepthStencilState,
    color: wgpu::ColorTargetState,
    raw: Option<(u64, [Option<wgpu::VertexAttribute>; 3])>,
}

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

// Bound the number of translated shader variants and GPU states retained
// across maps. Encoded work owns handles and survives a cache reset.
const PROGRAM_CAPACITY: usize = 256;
const PIPELINE_CAPACITY: usize = 1024;
const SAMPLER_CAPACITY: usize = 128;

struct Cache<K, V, const CAPACITY: usize>(HashMap<K, V>);

impl<K: Eq + Hash, V, const CAPACITY: usize> Cache<K, V, CAPACITY> {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.0.get(key)
    }

    /// Insert after a lookup miss. Restart the cache when it fills.
    fn insert_miss(&mut self, key: K, value: V) {
        if self.0.len() == CAPACITY {
            self.0.clear();
        }
        self.0.insert(key, value);
    }
}

#[cfg(test)]
#[test]
fn particle_cache_limits_survive_variant_churn() {
    fn check<const CAPACITY: usize>() {
        let mut cache = Cache::<usize, Arc<usize>, CAPACITY>::new();
        let in_flight = Arc::new(0);
        cache.insert_miss(0, Arc::clone(&in_flight));
        for key in 1..CAPACITY * 3 {
            cache.insert_miss(key, Arc::new(key));
            assert!(cache.0.len() <= CAPACITY);
            assert_eq!(**cache.get(&key).unwrap(), key);
        }
        assert!(cache.get(&0).is_none());
        assert_eq!(Arc::strong_count(&in_flight), 1);
    }

    check::<PROGRAM_CAPACITY>();
    check::<PIPELINE_CAPACITY>();
    check::<SAMPLER_CAPACITY>();
}

/// The native translated programs and pipelines, scoped to one host device.
pub struct ParticleRenderer {
    device: u64,
    programs: Cache<ShaderKey, Result<Arc<Program>, Refused>, PROGRAM_CAPACITY>,
    pipelines: Cache<RenderPipelineKey, Cached, PIPELINE_CAPACITY>,
    uniforms: Option<wgpu::BindGroupLayout>,
    samplers: Cache<gfx::translate::SamplerKey, wgpu::Sampler, SAMPLER_CAPACITY>,
    groups: TextureGroups<GroupKey>,
    /// What a sampled stage with no texture reads: one zeroed texel of the
    /// stage's dimension, as the NV2A reads a stage it fetches nothing for.
    unbound: HashMap<psh::TextureDim, (wgpu::TextureView, wgpu::Sampler)>,
    created: Created,
}

impl ParticleRenderer {
    pub fn new(device: u64) -> Self {
        Self {
            device,
            programs: Cache::new(),
            pipelines: Cache::new(),
            uniforms: None,
            samplers: Cache::new(),
            groups: TextureGroups::new(&GROUP_BYTES),
            unbound: HashMap::new(),
            created: Created::default(),
        }
    }

    /// What this renderer created so far.
    pub fn created(&self) -> Created {
        self.created
    }

    /// The texture groups held now and the textures they keep alive.
    pub fn held(&self) -> Held {
        self.groups.held()
    }

    /// The frame the following draws belong to. Texture groups no draw
    /// used for [`crate::texture_groups::KEEP_FRAMES`] frames are dropped.
    pub fn begin_frame(&mut self, frame: u64) {
        self.groups.begin_frame(frame);
    }

    /// The sampler for `key`, made once: a texture binding built from it
    /// keeps the same identity across frames, so its group stays cached.
    pub fn sampler(&mut self, device: &wgpu::Device, key: &gfx::translate::SamplerKey) -> wgpu::Sampler {
        if let Some(sampler) = self.samplers.get(key) {
            return sampler.clone();
        }
        let sampler = device.create_sampler(&key.descriptor());
        self.created.samplers += 1;
        self.samplers.insert_miss(*key, sampler.clone());
        sampler
    }

    /// The zeroed texel a sampled stage with no texture reads, made once
    /// per dimension.
    fn unbound(&mut self, device: &wgpu::Device, dim: psh::TextureDim) -> (wgpu::TextureView, wgpu::Sampler) {
        self.unbound
            .entry(dim)
            .or_insert_with(|| {
                let (dimension, layers, view) = match dim {
                    psh::TextureDim::D2 => (wgpu::TextureDimension::D2, 1, wgpu::TextureViewDimension::D2),
                    psh::TextureDim::D3 => (wgpu::TextureDimension::D3, 1, wgpu::TextureViewDimension::D3),
                    psh::TextureDim::Cube => (wgpu::TextureDimension::D2, 6, wgpu::TextureViewDimension::Cube),
                };
                // wgpu zeroes a texture before its first use.
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("particle unbound stage"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(view),
                    ..Default::default()
                });
                (view, device.create_sampler(&Default::default()))
            })
            .clone()
    }

    /// Finds or makes the program and pipeline. Nothing is uploaded and no
    /// group is made. The snapshot is validated first.
    fn resolve(
        &mut self,
        device: &wgpu::Device,
        s: &ImmediateDrawSnapshot<'_>,
    ) -> Result<(Arc<Program>, Cached), Refused> {
        assert_eq!(self.device, s.identity.device, "a renderer draws for its own device");
        assert!(
            !s.primitive.unclipped_depth
                || device.features().contains(wgpu::Features::DEPTH_CLIP_CONTROL),
            "depth is unclipped only where the device can"
        );
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
            input_fetch: s.shader.input_fetch,
            provided: s.raw_vertices.as_ref().map_or(u16::MAX, |r| r.provided),
        };
        let program = if let Some(program) = self.programs.get(&key) {
            program.clone()?
        } else {
            let uniforms = self
                .uniforms
                .get_or_insert_with(|| crate::light_gpu::uniforms_layout(device));
            self.created.programs += 1;
            let program = compile(device, &key, uniforms, self.created.programs).map(Arc::new);
            self.programs.insert_miss(key, program.clone());
            program?
        };
        for (n, dim) in program.sampled.iter().enumerate() {
            if let (Some(dim), Some(t)) = (dim, &s.textures[n]) {
                assert_eq!(t.dimension, *dim, "stage {n} is bound as the program samples it");
            }
        }
        let (primitive, depth) = (s.primitive(), s.depth());
        let pipeline_key = RenderPipelineKey {
            program: program.id,
            primitive,
            depth: depth.clone(),
            color: wgpu::ColorTargetState {
                format: s.target.color_format,
                blend: s.blend,
                write_mask: s.color_writes,
            },
            raw: s.raw_vertices.as_ref().map(|r| {
                (r.stride, std::array::from_fn(|n| r.attributes.get(n).copied()))
            }),
        };
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
        let color_target = pipeline_key.color.clone();
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
                    primitive,
                    depth_stencil: Some(depth.clone()),
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
                primitive,
                depth_stencil: Some(depth),
                color_target,
                samples: 1,
            },
            pending: Pending::ready(Some(pipeline.clone())),
            pipeline,
        };
        self.pipelines.insert_miss(pipeline_key, cached.clone());
        Ok((program, cached))
    }

    /// The group of `s`'s textures for `program`, made once per distinct
    /// views and samplers and kept while draws use it.
    fn texture_group(&mut self, device: &wgpu::Device, program: &Program, s: &ImmediateDrawSnapshot<'_>) -> wgpu::BindGroup {
        let bound = std::array::from_fn(|n| {
            program.sampled[n].map(|dim| match &s.textures[n] {
                Some(t) => (t.view.clone(), t.sampler.clone()),
                None => self.unbound(device, dim),
            })
        });
        let key = (program.textures.clone(), bound);
        if let Some(group) = self.groups.get(&key) {
            gfx::churn::native_cache(true);
            return group;
        }
        gfx::churn::native_cache(false);
        let mut entries = Vec::new();
        for (n, binding) in key.1.iter().enumerate() {
            let Some((view, sampler)) = binding else { continue };
            let n = n as u32;
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
        self.created.texture_groups += 1;
        let textures = key
            .1
            .each_ref()
            .map(|binding| binding.as_ref().map(|(view, _)| view.texture().clone()));
        self.groups.insert(key, group.clone(), textures.into_iter().flatten());
        group
    }

    /// One packet for the D3D render pass open on the target's attachments:
    /// geometry staged in `uploads`, uniforms at dynamic offsets in a run
    /// of the same arena, pipeline and texture group cached. It draws what
    /// [`PreparedDraw::record`] draws: the whole surface as viewport, the
    /// clamped scissor, blend constant and stencil reference. The flag is
    /// whether the pixel program discards (`ExternalDraws::discards`).
    /// The stencil reference keeps its low 8 bits, as the GPU masks it to
    /// the stencil format. `None` is a draw that rasterises nothing
    /// ([`ImmediateDrawSnapshot::validate`]).
    pub fn draws(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        s: &ImmediateDrawSnapshot<'_>,
    ) -> Result<Option<(RecordedDraw, bool)>, Refused> {
        let Some(drawn) = s.validate()? else { return Ok(None) };
        let reference = (s.stencil_reference & 0xff) as u8;
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
            .map_or_else(|| bytemuck::cast_slice(&s.vertices), |r| r.bytes.as_ref());
        let index_bytes: &[u8] = bytemuck::cast_slice(&s.indices[..drawn.indices as usize]);
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
        let [x, y, width, height] = drawn.scissor;
        let draw = RecordedDraw::prepared_indexed(
            &cached.described,
            cached.pending,
            vec![(vertices, vertex_at, vertex_bytes.len() as u64)],
            (indices, index_at, index_bytes.len() as u64),
            drawn.indices,
            groups,
            &render,
            s.target.size,
            ScissorRect { x, y, width, height },
        )
        .with_blend_constant(s.blend_constant);
        Ok(Some((draw, program.discards)))
    }

    /// Resolves everything and uploads owned geometry/uniforms, without
    /// recording a render pass or changing an attachment. Prepare *every*
    /// required target copy before the adapter commits any replacement.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        s: &ImmediateDrawSnapshot<'_>,
    ) -> Result<PreparedDraw, Refused> {
        let Some(drawn) = s.validate()? else {
            return Ok(PreparedDraw { identity: s.identity, target: s.target, draw: None });
        };
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
        let draw = Prepared {
            pipeline: cached.pipeline,
            uniforms,
            textures,
            vertices: buffer(
                "particle dynamic vertices",
                s.raw_vertices
                    .as_ref()
                    .map_or_else(|| bytemuck::cast_slice(&s.vertices), |r| r.bytes.as_ref()),
                wgpu::BufferUsages::VERTEX,
            ),
            indices: buffer(
                "particle dynamic indices",
                bytemuck::cast_slice(&s.indices[..drawn.indices as usize]),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: drawn.indices,
            blend_constant: s.blend_constant,
            stencil_reference: s.stencil_reference,
            scissor: drawn.scissor,
        };
        Ok(PreparedDraw { identity: s.identity, target: s.target, draw: Some(draw) })
    }
}

fn compile(
    device: &wgpu::Device,
    key: &ShaderKey,
    uniforms: &wgpu::BindGroupLayout,
    id: u64,
) -> Result<Program, Refused> {
    let count = key.microcode.len() / 4;
    if !key.microcode.len().is_multiple_of(4) || !(1..=vsh::MAX_INSTRUCTIONS).contains(&count) {
        return Err(Refused::Shader("invalid particle vertex program length".into()));
    }
    let program = vsh::Program::from_words(vsh::ProgramKind::Normal, &key.microcode)
        .map_err(|e| Refused::Shader(format!("vertex: {e:?}").into()))?;
    if program.writes_constants() {
        return Err(Refused::Shader("vertex program writes constants".into()));
    }
    let vertex = vsh::translate(
        &program,
        &vsh::VertexOptions {
            provided_inputs: key.provided,
            flat_shading: key.flat,
            input_fetch: key.input_fetch,
        },
    )
    .map_err(|e| Refused::Shader(format!("vertex: {e:?}").into()))?;
    let pixel = psh::translate(&key.pixel, &key.options);
    if !pixel.warnings.is_empty() {
        return Err(Refused::Shader(pixel.warnings.join("; ").into()));
    }
    // Shadow bindings need depth/comparison layouts. No silent float view.
    if key.options.shadow.iter().any(Option::is_some) {
        return Err(Refused::State);
    }
    let wgsl = shaders::link(&vertex, &pixel).map_err(|e| Refused::Shader(e.to_string().into()))?;
    shaders::validate(&wgsl).map_err(|e| Refused::Shader(e.to_string().into()))?;
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
        id,
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
    /// `None` rasterises nothing ([`ImmediateDrawSnapshot::validate`]).
    draw: Option<Prepared>,
}

struct Prepared {
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

    /// Records the draw, or nothing for a draw that rasterises nothing.
    /// The adapter records a copy only into the target it was prepared for.
    pub fn record(
        self,
        encoder: &mut wgpu::CommandEncoder,
        identity: DrawIdentity,
        target: TargetKey,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) -> Recorded {
        assert!(self.matches(identity, target), "a prepared copy records into its own target");
        let Some(draw) = self.draw else {
            return Recorded { identity, indices: 0, draws: 0 };
        };
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
        let [x, y, w, h] = draw.scissor;
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(&draw.pipeline);
        pass.set_bind_group(0, &draw.uniforms, &[0, 0]);
        pass.set_bind_group(1, &draw.textures, &[]);
        pass.set_blend_constant(draw.blend_constant);
        pass.set_stencil_reference(draw.stencil_reference);
        pass.set_vertex_buffer(0, draw.vertices.slice(..));
        pass.set_index_buffer(draw.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..draw.index_count, 0, 0..1);
        drop(pass);
        Recorded {
            identity,
            indices: draw.index_count,
            draws: 1,
        }
    }
}
