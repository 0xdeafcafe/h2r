//! Stage 4's GPU side: a compiled light pass ([`crate::light_look`]) with
//! its stage textures, the engine's light textures (the normalization cube
//! and the attenuation volume), and dynamic uniform offsets for the lights
//! in each ordered run (docs/graphics/native-renderer.md, "Stage 4").

use std::{collections::HashMap, sync::Mutex};

use gfx::ring::{SubmissionUploads, UniformRun};
use shaders::psh::PixelUniforms;
use shaders::vsh::VertexUniforms;

use crate::gpu::{Image, texture};
use crate::light_look::{FRAME_DWORDS, LightProgram, LightStage, colour_uniforms, light_vertex};
use crate::program::{Cull, INPUTS, ViewInputs, pipeline_state, bias_signed, frame_pixel, frame_vertex, sampler_descriptor};
use crate::world::WorldVertex;

/// The texture shapes a light stage binds.
pub use shaders::psh::TextureDim;

/// A texture with layers: a cube map's six faces or a volume's slices,
/// with its mip chain. The D3D path uploads the whole chain and the light
/// passes' samplers filter between mips (03a: mip filter linear), so the
/// native textures carry it too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayeredImage {
    pub dim: TextureDim,
    pub width: u32,
    pub height: u32,
    /// Six for a cube, the depth for a volume.
    pub layers: u32,
    /// Layer after layer, rows of RGBA8.
    pub rgba: Vec<u8>,
    /// Levels 1.. ([`Self::level_size`]), each laid out as `rgba`.
    pub mips: Vec<Vec<u8>>,
}

impl LayeredImage {
    /// Level `level`'s `(width, height, layers)`: each halved down to 1, a
    /// cube's six faces kept.
    pub fn level_size(&self, level: u32) -> (u32, u32, u32) {
        let half = |v: u32| (v >> level).max(1);
        let layers = match self.dim {
            TextureDim::Cube => 6,
            _ => half(self.layers),
        };
        (half(self.width), half(self.height), layers)
    }

    /// The image with its levels 1.. in order; the chain stops at the first
    /// level whose bytes do not fit its size, or below 1x1x1 (1x1 for a
    /// cube).
    pub fn with_mips(mut self, mips: Vec<Vec<u8>>) -> Self {
        for (i, bytes) in mips.into_iter().enumerate() {
            let below = self.level_size(i as u32);
            if below.0 == 1 && below.1 == 1 && (self.dim == TextureDim::Cube || below.2 == 1) {
                break;
            }
            let (w, h, l) = self.level_size(i as u32 + 1);
            if bytes.len() != (w * h * l * 4) as usize {
                break;
            }
            self.mips.push(bytes);
        }
        self
    }

    /// A cube map from its faces `(face, width, height, rgba)` in any order:
    /// all six, square, the same size.
    pub fn cube(mut faces: Vec<(u32, u32, u32, Vec<u8>)>) -> Option<Self> {
        faces.sort_by_key(|f| f.0);
        if faces.iter().map(|f| f.0).collect::<Vec<_>>() != (0..6).collect::<Vec<_>>() {
            return None;
        }
        let (w, h) = (faces[0].1, faces[0].2);
        if w == 0 || w != h || faces.iter().any(|f| (f.1, f.2) != (w, h) || f.3.len() != (w * h * 4) as usize) {
            return None;
        }
        Some(Self {
            dim: TextureDim::Cube,
            width: w,
            height: h,
            layers: 6,
            rgba: faces.into_iter().flat_map(|f| f.3).collect(),
            mips: Vec::new(),
        })
    }

    /// A volume of `depth` slices stacked in `rgba`.
    pub fn volume(width: u32, height: u32, depth: u32, rgba: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 || depth == 0 || rgba.len() != (width * height * depth * 4) as usize {
            return None;
        }
        Some(Self {
            dim: TextureDim::D3,
            width,
            height,
            layers: depth,
            rgba,
            mips: Vec::new(),
        })
    }
}

/// Uploads a [`LayeredImage`] and returns its view (a cube or a 3D view).
pub fn layered_texture(device: &wgpu::Device, queue: &wgpu::Queue, image: &LayeredImage, label: &str) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: image.width,
        height: image.height,
        depth_or_array_layers: image.layers,
    };
    let (dimension, view) = match image.dim {
        TextureDim::D3 => (wgpu::TextureDimension::D3, wgpu::TextureViewDimension::D3),
        TextureDim::Cube => (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::Cube),
        TextureDim::D2 => (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::D2),
    };
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1 + image.mips.len() as u32,
        sample_count: 1,
        dimension,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, bytes) in std::iter::once(&image.rgba).chain(&image.mips).enumerate() {
        let level = level as u32;
        let (w, h, layers) = image.level_size(level);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: layers,
            },
        );
    }
    t.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(view),
        ..Default::default()
    })
}

/// One decoded engine texture shared by worlds, objects and view copies.
/// Its GPU view is reused while the same device is active.
pub struct StaticLightTexture {
    image: LayeredImage,
    view: Mutex<Option<(wgpu::Device, wgpu::TextureView)>>,
}

impl StaticLightTexture {
    pub fn new(image: LayeredImage) -> Self {
        Self { image, view: Mutex::new(None) }
    }

    pub fn image(&self) -> &LayeredImage {
        &self.image
    }

    pub fn view(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
        let mut cached = self.view.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((owner, view)) = cached.as_ref()
            && owner == device
        {
            return view.clone();
        }
        let label = match self.image.dim {
            TextureDim::Cube => "native light normalization",
            TextureDim::D3 => "native light attenuation",
            TextureDim::D2 => "native light texture",
        };
        let view = layered_texture(device, queue, &self.image, label);
        *cached = Some((device.clone(), view.clone()));
        view
    }
}

/// The engine's light textures for the map (texture ids 1 and 9, from the
/// globals' rasterizer bitmaps); a pass sampling one that is missing is
/// not drawn.
#[derive(Default)]
pub struct LightTextures {
    pub normalization: Option<wgpu::TextureView>,
    pub attenuation: Option<wgpu::TextureView>,
}

pub(crate) fn view_dimension(d: TextureDim) -> wgpu::TextureViewDimension {
    match d {
        TextureDim::D2 => wgpu::TextureViewDimension::D2,
        TextureDim::D3 => wgpu::TextureViewDimension::D3,
        TextureDim::Cube => wgpu::TextureViewDimension::Cube,
    }
}

/// One compiled light pass on the GPU.
pub struct LightGpu {
    pub program: LightProgram,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    textures_layout: wgpu::BindGroupLayout,
    samplers: [wgpu::Sampler; 4],
    stage_views: [Option<wgpu::TextureView>; 4],
    static_groups: HashMap<Option<u32>, Option<wgpu::BindGroup>>,
    gel_target_group: Option<(wgpu::TextureView, Option<wgpu::BindGroup>)>,
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat, Option<Cull>), Option<wgpu::RenderPipeline>>,
}

impl LightGpu {
    /// Uploads `program`, its uniforms in group 0 laid out as
    /// `uniforms_layout` ([`uniforms_layout`]: one buffer per run, dynamic
    /// offsets); `None` when a stage's bitmap does not decode.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: LightProgram,
        uniforms_layout: &wgpu::BindGroupLayout,
        stage_image: &mut dyn FnMut(usize, LightStage) -> Option<Image>,
    ) -> Option<Self> {
        let mut stage_views: [Option<wgpu::TextureView>; 4] = Default::default();
        for (n, s) in program.stages.iter().enumerate() {
            if let LightStage::Bitmap { .. } = s
                && program.dims[n].is_some()
            {
                let mut image = stage_image(n, *s)?;
                bias_signed(&mut image, program.signed[n]);
                stage_views[n] = Some(texture(device, queue, &image, "native light stage"));
            }
        }
        let module = gfx::program::validated_resource(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native light pass"),
                source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
            })
        })?;
        let mut entries = Vec::new();
        for (n, d) in program.dims.iter().enumerate() {
            let Some(d) = d else { continue };
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: view_dimension(*d),
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
            label: Some("native light textures"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native light pass"),
            bind_group_layouts: &[Some(uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let samplers = std::array::from_fn(|n| device.create_sampler(&sampler_descriptor(program.samplers[n].as_ref())));
        Some(Self {
            program,
            module,
            layout,
            textures_layout,
            samplers,
            stage_views,
            static_groups: HashMap::new(),
            gel_target_group: None,
            pipelines: HashMap::new(),
        })
    }

    /// The texture group: the pass's bitmaps, the engine's light textures
    /// and the light's `gel`; `None` when a sampled stage has nothing.
    pub fn textures(&self, device: &wgpu::Device, engine: &LightTextures, gel: Option<&wgpu::TextureView>) -> Option<wgpu::BindGroup> {
        let mut views = Vec::new();
        for (n, d) in self.program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            let view = match self.program.stages[n] {
                LightStage::Empty => return None,
                LightStage::Bitmap { .. } => self.stage_views[n].clone()?,
                LightStage::Normalization => engine.normalization.clone()?,
                LightStage::Attenuation => engine.attenuation.clone()?,
                LightStage::Gel => gel?.clone(),
            };
            views.push((n, view));
        }
        let mut entries = Vec::new();
        for (n, view) in &views {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(&self.samplers[*n]),
            });
        }
        gfx::program::validated_resource(device, || {
            gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
                label: Some("native light textures"),
                layout: &self.textures_layout,
                entries: &entries,
            })
        })
    }

    /// Makes the pipeline for these target formats, with the pass's own
    /// blend, depth and cull state.
    pub fn pipeline(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, cull: Option<Cull>) -> Option<&wgpu::RenderPipeline> {
        let (module, layout, s) = (&self.module, &self.layout, self.program.state);
        self.pipelines.entry((color, depth, cull)).or_insert_with(|| {
            let attributes = INPUTS.map(|(location, offset, format)| wgpu::VertexAttribute {
                format, offset, shader_location: location,
            });
            gfx::program::validated_resource(device, || device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native light pass"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<WorldVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: pipeline_state(Some(&s), cull).0,
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth,
                    depth_write_enabled: Some(s.depth_write),
                    depth_compare: Some(s.depth_compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color,
                        blend: s.blend,
                        write_mask: s.color_writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }))
        }).as_ref()
    }
}

/// What makes two compiled light passes one GPU pass (module, layouts,
/// samplers, stage textures, pipeline state): lights whose inputs differ
/// only in constants share it, each with its own uniforms.
pub fn light_gpu_key(p: &LightProgram) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.wgsl.hash(&mut h);
    format!("{:?}", (&p.stages, &p.samplers, &p.dims, &p.signed, &p.state)).hash(&mut h);
    h.finish()
}

fn same_gpu_pass(a: &LightProgram, b: &LightProgram) -> bool {
    a.wgsl == b.wgsl
        && a.stages == b.stages
        && a.samplers == b.samplers
        && a.dims == b.dims
        && a.signed == b.signed
        && a.state == b.state
}

/// The uniforms group's layout: the vertex and pixel uniforms, each at a
/// dynamic offset in one buffer per run.
pub fn uniforms_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding, size: usize| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: wgpu::BufferSize::new(size as u64),
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native light uniforms"),
        entries: &[entry(0, VertexUniforms::SIZE), entry(1, PixelUniforms::SIZE)],
    })
}

/// Where each draw's uniforms sit in a run's buffer: `(vertex, pixel)`
/// offsets for draw `i` of `count`, each block aligned to `align`, and the
/// buffer's size.
pub fn run_layout(count: usize, align: u64) -> (Vec<[u32; 2]>, u64) {
    let (pixel, stride) = uniform_layout(align);
    let offsets = (0..count as u64).map(|i| [(i * stride) as u32, (i * stride + pixel) as u32]).collect();
    (offsets, count as u64 * stride)
}

fn uniform_layout(align: u64) -> (u64, u64) {
    let align = align.max(1);
    let up = |n: u64| n.div_ceil(align) * align;
    let pixel = up(VertexUniforms::SIZE as u64);
    (pixel, pixel + up(PixelUniforms::SIZE as u64))
}

/// One light's draw of one lit pass: the pass resolved with the light's
/// inputs but its colours, its GPU key ([`light_gpu_key`]), the light's
/// colours (put in by [`colour_uniforms`]), frame constants and gel, and
/// the world's index ranges it covers.
pub struct LightDraw<'a> {
    pub program: &'a LightProgram,
    pub key: u64,
    pub cull: Option<Cull>,
    /// Colours A and B (engine vectors 5 and 6).
    pub colours: [[f32; 3]; 2],
    pub frame: &'a [u32; FRAME_DWORDS],
    /// The gel's `bitm` datum (image 0), for a pass that samples it.
    pub gel: Option<u32>,
    /// Current target copy and guest-coordinate scale for engine texture 10.
    pub gel_view: Option<&'a wgpu::TextureView>,
    pub gel_scale: Option<[f32; 4]>,
    pub ranges: &'a [std::ops::Range<u32>],
}

impl LightDraw<'_> {
    /// Used by both the GPU upload and its reference comparison.
    pub fn uniforms(&self, view: &ViewInputs) -> (VertexUniforms, PixelUniforms) {
        let [a, b] = self.colours;
        let (vertex, pixel) = colour_uniforms(self.program, a, b);
        let mut pixel = frame_pixel(&pixel, view);
        apply_gel_scale(&mut pixel, &self.program.stages, self.gel_scale);
        (light_vertex(&frame_vertex(&vertex, view), self.frame), pixel)
    }
}

fn apply_gel_scale(pixel: &mut PixelUniforms, stages: &[LightStage; 4], scale: Option<[f32; 4]>) {
    if let Some(scale) = scale {
        for (stage, destination) in stages.iter().zip(&mut pixel.tex_scale) {
            if *stage == LightStage::Gel { *destination = scale; }
        }
    }
}

/// The world's light passes on the GPU: the engine's light textures, the
/// gels by datum and the passes by key, made as lights first need them.
/// `None` entries remember what did not upload.
pub struct LightSet {
    pub textures: LightTextures,
    uniforms_layout: wgpu::BindGroupLayout,
    gels: HashMap<u32, Option<wgpu::TextureView>>,
    passes: HashMap<u64, Option<LightGpu>>,
}

struct Prepared<'a> {
    draw: &'a LightDraw<'a>,
    pipeline: wgpu::RenderPipeline,
    textures: wgpu::BindGroup,
}

impl LightSet {
    /// Get the engine's light texture views once for this world.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, normalization: Option<&StaticLightTexture>, attenuation: Option<&StaticLightTexture>) -> Self {
        Self {
            textures: LightTextures {
                normalization: normalization.map(|i| i.view(device, queue)),
                attenuation: attenuation.map(|i| i.view(device, queue)),
            },
            uniforms_layout: uniforms_layout(device),
            gels: HashMap::new(),
            passes: HashMap::new(),
        }
    }

    /// How many passes are on the GPU.
    pub fn len(&self) -> usize {
        self.passes.values().filter(|p| p.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Resolve pipelines and texture groups before allocating uniform
    /// space. A failed draw leaves all earlier packets intact.
    #[allow(clippy::too_many_arguments)]
    fn prepare<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        view: &ViewInputs,
        draws: &'a [LightDraw<'a>],
        uploads: &SubmissionUploads,
        stage_image: &mut dyn FnMut(usize, LightStage) -> Option<Image>,
        gel_image: &mut dyn FnMut(u32) -> Option<Image>,
    ) -> Option<(UniformRun, Vec<Prepared<'a>>)> {
        let mut ready = Vec::new();
        for d in draws {
            if d.ranges.is_empty() {
                continue;
            }
            let reads_gel = d.program.stages.iter().zip(d.program.dims)
                .any(|(stage, dim)| *stage == LightStage::Gel && dim.is_some());
            let static_gel = if reads_gel && d.gel_view.is_none() { Some(d.gel?) } else { None };
            let layout = &self.uniforms_layout;
            let mut make = || LightGpu::new(device, queue, d.program.clone(), layout, &mut *stage_image);
            let cached = self.passes.entry(d.key).or_insert_with(&mut make);
            if cached.as_ref().is_some_and(|p| !same_gpu_pass(&p.program, d.program)) {
                *cached = make();
            }
            let pass = cached.as_mut()?;
            let textures = if let Some(view) = d.gel_view.filter(|_| reads_gel) {
                if let Some((cached_view, group)) = &pass.gel_target_group
                    && cached_view == view
                {
                    group.clone()?
                } else {
                    let group = pass.textures(device, &self.textures, Some(view));
                    pass.gel_target_group = Some((view.clone(), group.clone()));
                    group?
                }
            } else if let Some(group) = pass.static_groups.get(&static_gel) {
                group.clone()?
            } else {
                let gel = match static_gel {
                    Some(g) => self.gels.entry(g)
                        .or_insert_with(|| gel_image(g).map(|i| texture(device, queue, &i, "native light gel")))
                        .as_ref(),
                    None => None,
                };
                let group = pass.textures(device, &self.textures, gel);
                pass.static_groups.insert(static_gel, group.clone());
                group?
            };
            ready.push(Prepared { draw: d, pipeline: pass.pipeline(device, color, depth, d.cull)?.clone(), textures });
        }
        let uniforms = uploads.uniform_run(
            device, queue, &self.uniforms_layout,
            [VertexUniforms::SIZE, PixelUniforms::SIZE], ready.len(),
            |i, v, p| {
                let (vertex, pixel) = ready[i].draw.uniforms(view);
                vertex.write_bytes(v);
                pixel.write_bytes(p);
            },
        )?;
        Some((uniforms, ready))
    }

    /// Record each blend group before the guest can consume any entries.
    /// The enclosing pass supplies its scissor and dynamic blend constant.
    pub fn bundles(&mut self, target: &d3d::ExternalTarget<'_>,
        vertices: &wgpu::Buffer, indices: &wgpu::Buffer,
        view: &ViewInputs, draws: &[LightDraw<'_>],
        stage_image: &mut dyn FnMut(usize, LightStage) -> Option<Image>,
        gel_image: &mut dyn FnMut(u32) -> Option<Image>,
    ) -> Option<LightBundles> {
        if vertices.size() == 0 || indices.size() == 0 { return None; }
        if draws.iter().filter_map(|draw| draw.gel_view).any(|view| {
            view.texture() == target.color.texture() || view.texture() == target.depth.texture()
        }) {
            return None;
        }
        let (device, color, depth) = (target.device, target.color_format, target.depth_format);
        let (uniforms, prepared) = self.prepare(
            device, target.queue, color, depth, view, draws, target.uploads, stage_image, gel_image,
        )?;
        let mut groups = Vec::with_capacity(prepared.len());
        let mut ranges = 0;
        for (i, prepared) in prepared.iter().enumerate() {
            let draw = prepared.draw;
            let mut bundle = device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                label: Some("native light geometry"), color_formats: &[Some(color)],
                depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                    format: depth, depth_read_only: false, stencil_read_only: !depth.has_stencil_aspect(),
                }),
                sample_count: 1, multiview: None,
            });
            bundle.set_vertex_buffer(0, vertices.slice(..));
            bundle.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            bundle.set_pipeline(&prepared.pipeline);
            bundle.set_bind_group(0, &uniforms.group, &uniforms.offsets(i));
            bundle.set_bind_group(1, &prepared.textures, &[]);
            for range in draw.ranges {
                if range.is_empty() || u64::from(range.end) > indices.size() / 4 { return None; }
                bundle.draw_indexed(range.clone(), 0, 0..1);
                ranges += 1;
            }
            groups.push((draw.program.state.blend_constant, bundle.finish(&Default::default())));
        }
        Some(LightBundles { groups, ranges })
    }
}

pub struct LightBundles {
    groups: Vec<(wgpu::Color, wgpu::RenderBundle)>,
    ranges: usize,
}

impl LightBundles {
    pub fn into_packet(self, run: u64, scissor: [u32; 4]) -> d3d::backend::ExternalBundles {
        d3d::backend::ExternalBundles { run, scissor, groups: self.groups, ranges: self.ranges }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_lights_reuse_warm_gpu_resources_with_distinct_uniform_ranges() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let (device, queue) = (&ctx.device, &ctx.queue);
        let cube = StaticLightTexture::new(LayeredImage::cube(
            (0..6).map(|f| (f, 1, 1, vec![0; 4])).collect(),
        ).unwrap());
        let volume = StaticLightTexture::new(LayeredImage::volume(1, 1, 1, vec![0; 4]).unwrap());
        let mut set = LightSet::new(device, queue, Some(&cube), Some(&volume));
        let other_view = LightSet::new(device, queue, Some(&cube), Some(&volume));
        assert_eq!(set.textures.normalization, other_view.textures.normalization);
        assert_eq!(set.textures.attenuation, other_view.textures.attenuation);

        let program = LightProgram {
            wgsl: "
                @vertex fn vs_main() -> @builtin(position) vec4f { return vec4f(0.0, 0.0, 0.0, 1.0); }
                @fragment fn fs_main() -> @location(0) vec4f { return vec4f(1.0); }
            ".into(),
            words: Vec::new(),
            inputs: 0,
            def: Default::default(),
            pixel_options: Default::default(),
            stages: [LightStage::Normalization, LightStage::Attenuation, LightStage::Gel, LightStage::Empty],
            samplers: [None; 4],
            dims: [Some(TextureDim::Cube), Some(TextureDim::D3), Some(TextureDim::D2), None],
            signed: [[false; 4]; 4],
            vertex: VertexUniforms::default(),
            pixel: PixelUniforms::default(),
            colours: Vec::new(),
            state: crate::pass_state::PassState {
                blend: None,
                blend_constant: wgpu::Color::BLACK,
                color_writes: wgpu::ColorWrites::ALL,
                depth_write: false,
                depth_compare: wgpu::CompareFunction::Always,
                primitive: wgpu::PrimitiveState::default(),
                alpha_test: None,
            },
            inherits_cull: false,
        };
        let view = ViewInputs {
            constants: [[0.; 4]; d3d::VIEW_CONSTANTS], guest_size: (8, 8), size: (8, 8),
            z_scale: 1., w_clip: None,
        };
        let gel = texture(device, queue, &Image::solid([255; 4]), "test light gel");
        let draw = LightDraw {
            program: &program, key: light_gpu_key(&program), cull: None, colours: [[0.; 3]; 2],
            frame: &[0; FRAME_DWORDS], gel: None, gel_view: Some(&gel), gel_scale: None, ranges: &[0..3],
        };
        let uploads = gfx::ring::UploadPool::default().begin();
        // These are independent reservations. Even the single-light path
        // takes offsets from the submission, with no private uniform page.
        let mut prepare = || set.prepare(
            device, queue, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Depth32Float,
            &view, std::slice::from_ref(&draw), &uploads,
            &mut |_, _| panic!("no stage bitmap"), &mut |_| panic!("no gel"),
        ).unwrap();
        let (a, first) = prepare();
        let warm = gfx::churn::snapshot();
        let (b, second) = prepare();
        let (c, third) = prepare();
        assert_eq!((gfx::churn::snapshot() - warm).buffers_created, 0);
        assert_eq!(a.group, b.group);
        assert_eq!(b.group, c.group);
        assert!(a.offsets(0)[1] < b.offsets(0)[0]);
        assert!(b.offsets(0)[1] < c.offsets(0)[0]);
        assert_eq!(first[0].pipeline, third[0].pipeline);
        assert_eq!(first[0].textures, third[0].textures);
    }

    #[test]
    fn linear_target_coordinates_only_scale_the_gel_stages() {
        let mut pixel = PixelUniforms::default();
        let original = pixel.tex_scale;
        let stages = [LightStage::Gel, LightStage::Attenuation, LightStage::Gel, LightStage::Empty];
        apply_gel_scale(&mut pixel, &stages, None);
        assert_eq!(pixel.tex_scale, original);
        let scale = [1.0 / 37.0, 1.0 / 21.0, 1.0, 1.0];
        apply_gel_scale(&mut pixel, &stages, Some(scale));
        assert_eq!(pixel.tex_scale, [scale, original[1], scale, original[3]]);
    }

    #[test]
    fn a_cube_needs_six_square_faces_of_one_size() {
        let face = |f| (f, 2, 2, vec![f as u8; 16]);
        let c = LayeredImage::cube((0..6).rev().map(face).collect()).expect("six faces");
        assert_eq!((c.dim, c.width, c.layers), (TextureDim::Cube, 2, 6));
        assert_eq!(c.rgba[..16], [0; 16], "face 0 first");
        assert_eq!(c.rgba[5 * 16..], [5; 16]);
        assert!(LayeredImage::cube((0..5).map(face).collect()).is_none(), "five faces");
        let mut odd: Vec<_> = (0..6).map(face).collect();
        odd[3] = (3, 1, 1, vec![0; 4]);
        assert!(LayeredImage::cube(odd).is_none(), "sizes differ");
        assert!(LayeredImage::cube((0..6).map(|f| (f, 2, 1, vec![0; 8])).collect()).is_none(), "not square");
    }

    #[test]
    fn a_runs_uniforms_share_one_buffer_at_aligned_offsets() {
        let (v, p) = (VertexUniforms::SIZE as u64, PixelUniforms::SIZE as u64);
        let up = |n: u64| n.div_ceil(256) * 256;
        let (offsets, size) = run_layout(3, 256);
        let stride = up(v) + up(p);
        assert_eq!(size, 3 * stride);
        for (i, o) in offsets.iter().enumerate() {
            assert_eq!(u64::from(o[0]), i as u64 * stride);
            assert_eq!(u64::from(o[1]), i as u64 * stride + up(v));
            assert_eq!((o[0] % 256, o[1] % 256), (0, 0));
            assert!(u64::from(o[1]) + p <= size);
        }
        assert_eq!(run_layout(0, 256), (Vec::new(), 0));
        assert_eq!(run_layout(1, 0).1, v + p, "no alignment asked");
    }

    #[test]
    fn layered_mips_halve_to_one_and_a_cube_keeps_six_faces() {
        let v = LayeredImage::volume(4, 2, 8, vec![0; 4 * 2 * 8 * 4]).unwrap();
        assert_eq!((v.level_size(1), v.level_size(2), v.level_size(3)), ((2, 1, 4), (1, 1, 2), (1, 1, 1)));
        let chain = vec![vec![1; 2 * 4 * 4], vec![2; 2 * 4], vec![3; 4], vec![4; 4]];
        let v = v.with_mips(chain);
        assert_eq!(v.mips, vec![vec![1; 32], vec![2; 8], vec![3; 4]], "nothing below 1x1x1");
        let v = LayeredImage::volume(4, 2, 8, vec![0; 256]).unwrap().with_mips(vec![vec![1; 32], vec![2; 4], vec![3; 4]]);
        assert_eq!(v.mips.len(), 1, "stops at a level that does not fit");

        let c = LayeredImage::cube((0..6).map(|f| (f, 2, 2, vec![0; 16])).collect()).unwrap();
        assert_eq!(c.level_size(1), (1, 1, 6));
        let c = c.with_mips(vec![vec![5; 24], vec![6; 24]]);
        assert_eq!(c.mips, vec![vec![5; 24]], "a cube ends at 1x1");
    }

    #[test]
    fn a_volume_holds_its_slices() {
        let v = LayeredImage::volume(2, 2, 3, vec![0; 48]).expect("fits");
        assert_eq!((v.dim, v.layers), (TextureDim::D3, 3));
        assert!(LayeredImage::volume(2, 2, 3, vec![0; 47]).is_none());
        assert!(LayeredImage::volume(2, 2, 0, Vec::new()).is_none());
    }
}
