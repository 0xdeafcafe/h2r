//! Objects' lit parts on the GPU, drawn with the game's own vertex
//! programs and pixel shaders ([`crate::object_program`]): per section
//! its vertex streams expanded once through each declaration a program
//! needs ([`crate::object_streams`]), each program uploaded once, and per
//! view changed vertex and pixel constants staged in the submission upload ring,
//! then one render pass over the game's targets
//! (docs/graphics/native-renderer.md, "Object programs").

use std::collections::HashMap;
use std::ops::Range;

use shaders::psh::PixelUniforms;
use shaders::vsh::VertexUniforms;
use wgpu::util::DeviceExt;

use crate::gpu::{Image, texture};
use crate::light_gpu::{StaticLightTexture, uniforms_layout};
use crate::look::Stage;
use crate::object_lighting::ShaderValues;
use crate::object_program::{
    IMPLEMENTATIONS, ObjectInputs, ObjectPass, Row, object_layer3, object_pass, object_pixel_uniforms, object_vertex_uniforms,
};
use crate::object_program::{LIGHTMAP_STREAM, texaccum_dimensions};
use crate::object_streams::{Attribute, Element, SLOT, StreamData, StreamFormats, declaration, gather, registers};
use crate::object_state::{self, DrawState, EntryStates};
use crate::objects::{ItemKey, ObjectFrame};
use crate::program::{ViewInputs, bias_signed, frame_pixel, frame_vertex, sampler_descriptor};

/// One section's vertices through one declaration: per vertex, per
/// register in `registers` order, its guest bytes in a slot
/// ([`crate::object_streams::gather`]) of the guest format in `formats`,
/// which the vertex unit converts as the D3D path's fetch does
/// ([`vertex_fetch`]); and the section's index list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LitBufferInput {
    pub registers: Vec<u8>,
    pub formats: Vec<Attribute>,
    pub vertices: Vec<[u8; SLOT]>,
    pub indices: Vec<u32>,
}

/// One compiled implementation and the images of its sampled stages.
pub struct LitProgramInput {
    pub pass: ObjectPass,
    pub images: [Option<Image>; 4],
}

/// One lit part of a model: its mode section and its index among the
/// section's parts (what a render record's key names, `objects::ItemKey`),
/// the section's decompression, the material's shader values, the index
/// range in its buffers, and per implementation index the program and
/// buffer it draws with.
#[derive(Clone, Debug, PartialEq)]
pub struct LitPart {
    pub section: u16,
    pub part: u8,
    pub decompression: [Row; 3],
    /// Its material's shader tag datum (diagnostics).
    pub shader_tag: u32,
    pub shader: ShaderValues,
    pub indices: Range<u32>,
    pub implementations: [Option<(usize, usize)>; IMPLEMENTATIONS],
}

/// A model's lit parts, ready to upload: programs and buffers indexed by
/// [`LitPart::implementations`].
#[derive(Default)]
pub struct LitModelInput {
    pub programs: Vec<LitProgramInput>,
    pub buffers: Vec<LitBufferInput>,
    pub parts: Vec<LitPart>,
}

impl LitModelInput {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// Whether a program samples the texture accumulation target
    /// ([`crate::object_program::TEXACCUM_TEXTURE`]): a pass drawing the
    /// model then needs the level bound ([`LitObjects::encode`]).
    pub fn samples_texaccum(&self) -> bool {
        self.programs.iter().any(|p| p.pass.texaccum.contains(&true))
    }
}

struct LitBuffer {
    layout_id: usize,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    registers: Vec<u8>,
    formats: Vec<Attribute>,
}

type InheritedState = std::sync::Mutex<Option<(bool, EntryStates, DrawState)>>;

struct LitProgram {
    pass: ObjectPass,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    textures_layout: wgpu::BindGroupLayout,
    /// The stages' views the program owns, and every stage's sampler.
    views: Vec<(usize, wgpu::TextureView)>,
    samplers: Vec<wgpu::Sampler>,
    /// Its stages' bind group, made once; `None` when a stage samples the
    /// texture accumulation target, whose group is cached by its live view.
    textures: Option<wgpu::BindGroup>,
    accum_groups: Vec<(wgpu::TextureView, Option<wgpu::BindGroup>)>,
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat, usize, DrawState), Option<wgpu::RenderPipeline>>,
    bundles: HashMap<BundleKey, (wgpu::RenderBundle, u64)>,
    uses: u64,
    inherited: InheritedState,
}

#[derive(PartialEq, Eq, Hash)]
struct BundleKey {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::BindGroup,
    textures: wgpu::BindGroup,
    offsets: [u32; 2],
    buffer: usize,
    indices: Range<u32>,
}

/// The bind group of `views` (and `extra`, the texture accumulation
/// level's stages) with their samplers.
fn texture_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    views: &[(usize, wgpu::TextureView)],
    extra: &[(usize, &wgpu::TextureView)],
    samplers: &[wgpu::Sampler],
) -> wgpu::BindGroup {
    let mut entries = Vec::new();
    for (n, view) in views.iter().map(|(n, v)| (*n, v)).chain(extra.iter().copied()) {
        entries.push(wgpu::BindGroupEntry { binding: 2 * n as u32, resource: wgpu::BindingResource::TextureView(view) });
        entries.push(wgpu::BindGroupEntry { binding: 2 * n as u32 + 1, resource: wgpu::BindingResource::Sampler(&samplers[n]) });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("native object program textures"), layout, entries: &entries })
}

/// One lit part's draw as [`LitObjects::encode`] issues it
/// ([`LitObjects::part_uniforms`]).
#[derive(Clone, Debug)]
pub struct PartUniforms {
    pub implementation: usize,
    pub words: Vec<u32>,
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    pub options: shaders::vsh::VertexOptions,
    pub def: shaders::psh::PixelShaderDef,
    pub pixel_options: shaders::psh::PixelOptions,
    pub samplers: [Option<gfx::translate::SamplerKey>; 4],
    pub textures: [Option<String>; 4],
    pub primitive: wgpu::PrimitiveState,
    pub depth: (bool, wgpu::CompareFunction),
}

/// The texture accumulation level an encode binds: its view and its size
/// in guest texels (the coordinates the programs give are texels of it).
#[derive(Clone, Copy)]
pub struct Texaccum<'a> {
    pub view: &'a wgpu::TextureView,
    pub guest_size: (u32, u32),
}

struct LitModel {
    programs: Vec<LitProgram>,
    buffers: Vec<LitBuffer>,
    parts: Vec<LitPart>,
    part_index: HashMap<(u16, u8), usize>,
}

/// Every model's lit parts on the GPU.
pub struct LitObjects {
    uniforms_layout: wgpu::BindGroupLayout,
    models: HashMap<u32, LitModel>,
    /// The vector normalization cube, as the lights load it (engine
    /// texture 1).
    normalization: Option<wgpu::TextureView>,
}

/// One accepted item's draw, resolved at the hook ([`LitObjects::plan`]):
/// the part and implementation its record picked, the program and buffer
/// they draw with, the part's indices, its vertex and pixel blocks with
/// every per-object constant in (the view's put in at the encode), the
/// state resolved field by field, and whether it samples the texture
/// accumulation level.
#[derive(Clone, Debug)]
pub struct PlannedDraw {
    pub part: usize,
    pub implementation: usize,
    pub program: usize,
    pub buffer: usize,
    pub indices: Range<u32>,
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    pub state: DrawState,
    pub texaccum: bool,
    pub default_final: bool,
}

impl LitBufferInput {
    /// Vertex `v`'s register `k` (in `registers` order) as the vertex unit
    /// reads it (the CPU's decode, for tests).
    pub fn decoded(&self, v: usize, k: usize) -> [f32; 4] {
        self.formats[k].decode(&self.vertices[v * self.registers.len() + k])
    }
}

/// The vertex format and shader fetch a guest attribute is read with, as
/// the D3D path reads it with its default (GPU) fetch
/// (`gfx::repack::AttributeFetch::raw`, `d3d::nv2a::input_fetch`): the
/// guest bytes as they are, converted by the vertex unit. Converted on
/// the CPU instead, normalised shorts can round another way than the
/// GPU's conversion, and a depth-equal pass (the third pass of 03a's
/// rebar, over the depth its D3D pass wrote) fails where w differs in the
/// last bit, as the world's packed normals did.
pub fn vertex_fetch(a: Attribute) -> (wgpu::VertexFormat, shaders::vsh::InputFetch) {
    use shaders::vsh::InputFetch as I;
    use wgpu::VertexFormat as F;
    match a {
        Attribute::Float(1) => (F::Float32, I::Float),
        Attribute::Float(2) => (F::Float32x2, I::Float),
        Attribute::Float(3) => (F::Float32x3, I::Float),
        Attribute::Float(_) => (F::Float32x4, I::Float),
        Attribute::PByte(1) => (F::Unorm8, I::Float),
        Attribute::PByte(2) => (F::Unorm8x2, I::Float),
        Attribute::PByte(3) => (F::Unorm8x4, I::Xyz1),
        Attribute::PByte(_) => (F::Unorm8x4, I::Float),
        Attribute::Short(1) => (F::Sint16, I::Int),
        Attribute::Short(2) => (F::Sint16x2, I::Int),
        Attribute::Short(3) => (F::Sint16x4, I::IntXyz1),
        Attribute::Short(_) => (F::Sint16x4, I::Int),
        Attribute::NormShort(1) => (F::Snorm16, I::Float),
        Attribute::NormShort(2) => (F::Snorm16x2, I::Float),
        Attribute::NormShort(3) => (F::Snorm16x4, I::Xyz1),
        Attribute::NormShort(_) => (F::Snorm16x4, I::Float),
        Attribute::NormPacked3 => (F::Uint32, I::NormPacked3),
        Attribute::Colour => (F::Unorm8x4Bgra, I::Float),
    }
}

/// The shader fetch of each register `declaration` supplies
/// ([`vertex_fetch`]); the rest are floats.
pub fn input_fetch(declaration: &[Element]) -> [shaders::vsh::InputFetch; 16] {
    let mut out = [shaders::vsh::InputFetch::Float; 16];
    for e in declaration {
        if let Some(f) = out.get_mut(usize::from(e.register)) {
            *f = vertex_fetch(e.format).1;
        }
    }
    out
}

/// The vertex attributes of a buffer holding `registers` of `formats`,
/// each in a [`SLOT`].
pub fn attributes(registers: &[u8], formats: &[Attribute]) -> Vec<wgpu::VertexAttribute> {
    registers
        .iter()
        .zip(formats)
        .enumerate()
        .map(|(k, (&r, &f))| wgpu::VertexAttribute {
            format: vertex_fetch(f).0,
            offset: (SLOT * k) as u64,
            shader_location: u32::from(r),
        })
        .collect()
}

impl LitObjects {
    pub fn new(device: &wgpu::Device) -> Self {
        Self { uniforms_layout: uniforms_layout(device), models: HashMap::new(), normalization: None }
    }

    /// Uploads the normalization cube the programs' normalization stages
    /// sample, once.
    pub fn set_normalization(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, image: &StaticLightTexture) {
        if self.normalization.is_none() {
            self.normalization = Some(image.view(device, queue));
        }
    }

    /// Uploads `input` as model `index`'s lit parts (replacing any); not
    /// when a program samples the normalization cube and it is not
    /// uploaded (the build refuses such a program without it).
    pub fn add_model(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, index: u32, input: LitModelInput) {
        let mut layouts = HashMap::new();
        let buffers = input
            .buffers
            .iter()
            .map(|b| LitBuffer {
                layout_id: {
                    let next = layouts.len();
                    *layouts.entry((b.registers.clone(), b.formats.clone())).or_insert(next)
                },
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native object program vertices"),
                    contents: bytemuck::cast_slice(&b.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native object program indices"),
                    contents: bytemuck::cast_slice(&b.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                registers: b.registers.clone(),
                formats: b.formats.clone(),
            })
            .collect();
        let Some(programs) = input.programs.into_iter().map(|p| self.program(device, queue, p)).collect::<Option<Vec<_>>>() else {
            return;
        };
        let part_index = index_parts(&input.parts);
        self.models.insert(index, LitModel { programs, buffers, parts: input.parts, part_index });
    }

    fn program(&self, device: &wgpu::Device, queue: &wgpu::Queue, input: LitProgramInput) -> Option<LitProgram> {
        let LitProgramInput { pass, mut images } = input;
        let p = &pass.program;
        let module = gfx::program::validated_resource(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native object program"),
                source: wgpu::ShaderSource::Wgsl(p.wgsl.clone().into()),
            })
        })?;
        let mut layout_entries = Vec::new();
        let mut views = Vec::new();
        let samplers: Vec<wgpu::Sampler> = (0..4).map(|n| device.create_sampler(&sampler_descriptor(p.samplers[n].as_ref()))).collect();
        for (n, d) in p.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: if pass.normalization[n] {
                        wgpu::TextureViewDimension::Cube
                    } else {
                        wgpu::TextureViewDimension::D2
                    },
                    multisampled: false,
                },
                count: None,
            });
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32 + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
            if pass.normalization[n] {
                // Not biased, as the lights sample it.
                views.push((n, self.normalization.clone()?));
                continue;
            }
            if pass.texaccum[n] {
                // The pass's level, bound per encode.
                continue;
            }
            // Checked at load: every sampled stage has its image.
            let mut image = images[n].take().unwrap_or(Image { width: 1, height: 1, rgba: vec![0, 0, 0, 255], mips: Vec::new(), blocks: None });
            bias_signed(&mut image, p.signed[n]);
            views.push((n, texture(device, queue, &image, "native object program stage")));
        }
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native object program textures"),
            entries: &layout_entries,
        });
        let textures = if pass.texaccum.contains(&true) {
            None
        } else {
            Some(gfx::program::validated_resource(device, || {
                texture_group(device, &textures_layout, &views, &[], &samplers)
            })?)
        };
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native object program"),
            bind_group_layouts: &[Some(&self.uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        Some(LitProgram {
            pass, module, layout, textures_layout, views, samplers, textures,
            accum_groups: Vec::new(), pipelines: HashMap::new(), bundles: HashMap::new(), uses: 0, inherited: Default::default(),
        })
    }

    /// Whether model `index` has lit parts.
    pub fn has(&self, index: u32) -> bool {
        self.models.contains_key(&index)
    }

    /// The draw of the item `key` (the part and implementation its record
    /// picked) of `frame` in model `index`, its record's node rows `rows`,
    /// over the render state the device held at the layer-3 call
    /// (`entry`): every per-object constant put in and every state field
    /// resolved ([`crate::object_state::resolve`]), only the view's own
    /// left for the encode ([`Self::encode`]: the view's constants and
    /// clip, and the texture accumulation level's size). An error says why
    /// the item keeps its D3D draw.
    pub fn plan(&self, index: u32, frame: &ObjectFrame, key: &ItemKey, rows: &[Row], entry: &EntryStates) -> Result<PlannedDraw, String> {
        self.plan_with_scene_state(index, frame, key, rows, entry, false)
    }

    pub fn plan_scene(&self, index: u32, frame: &ObjectFrame, key: &ItemKey, rows: &[Row], entry: &EntryStates) -> Result<PlannedDraw, String> {
        self.plan_with_scene_state(index, frame, key, rows, entry, true)
    }

    fn plan_with_scene_state(
        &self, index: u32, frame: &ObjectFrame, key: &ItemKey, rows: &[Row], entry: &EntryStates,
        scene: bool,
    ) -> Result<PlannedDraw, String> {
        let m = self.models.get(&index).ok_or("its model is not uploaded")?;
        let part = m.part_index.get(&(key.section, key.part)).copied();
        plan_indexed_part(&m.parts, part, &|program| m.programs.get(program).map(|p| (&p.pass, Some(&p.inherited))), frame, key, rows, entry, scene)
    }

    pub fn planned_pass(&self, index: u32, draw: &PlannedDraw) -> Option<&ObjectPass> {
        Some(&self.models.get(&index)?.programs.get(draw.program)?.pass)
    }

    /// The blocks `d` (of model `index`) is issued with in `view`, and the
    /// texture accumulation level's size when it samples one: what
    /// [`Self::encode`] uploads. `None` when the model is gone or `d`
    /// samples the level and there is none.
    fn issued(&self, index: u32, d: &PlannedDraw, view: &ViewInputs, texaccum: Option<Texaccum<'_>>) -> Option<(VertexUniforms, PixelUniforms)> {
        let pass = &self.models.get(&index)?.programs.get(d.program)?.pass;
        if d.texaccum && texaccum.is_none() {
            return None;
        }
        let mut vertex = frame_vertex(&d.vertex, view);
        vertex.w_clamp = d.state.w_clamp;
        let mut pixel = frame_pixel(&d.pixel, view);
        if let Some(t) = texaccum.filter(|_| d.texaccum) {
            texaccum_dimensions(&mut vertex, &pass.implementation, pass.texaccum, t.guest_size);
            texaccum_scale(&mut pixel, pass.texaccum, t.guest_size);
        }
        Some((vertex, pixel))
    }

    /// One line on how `d` (of model `index`) draws (diagnostics: the
    /// objects' probe).
    pub fn describe(&self, index: u32, d: &PlannedDraw) -> String {
        let section = self.models.get(&index).and_then(|m| m.parts.get(d.part)).map(|p| (p.section, p.part, p.shader_tag));
        format!(
            "part {} (section, part, shader) {section:x?} implementation {} indices {:?}: depth {:?} write {} cull {:?} front {:?} writes {:?} alpha test {:?} clamp {}{}",
            d.part,
            d.implementation,
            d.indices,
            d.state.depth_compare,
            d.state.depth_write,
            d.state.primitive.cull_mode,
            d.state.primitive.front_face,
            d.state.color_writes,
            d.state.alpha_test,
            d.state.w_clamp,
            if d.texaccum { "; samples the texture accumulation level" } else { "" },
        )
    }

    /// What `d` (of model `index`) is issued with in `view` (as
    /// [`Self::encode`] puts it in), to compare with the D3D path's draw
    /// of it (`d3d::uniform_compare`).
    pub fn part_uniforms(&self, index: u32, d: &PlannedDraw, view: &ViewInputs, texaccum: Option<Texaccum<'_>>) -> Result<PartUniforms, String> {
        let m = self.models.get(&index).ok_or("no lit model")?;
        let pass = &m.programs.get(d.program).ok_or("no program")?.pass;
        let (vertex, mut pixel) = self.issued(index, d, view, texaccum).ok_or("it samples the texture accumulation level and there is none")?;
        let textures = std::array::from_fn(|n| {
            if pass.texaccum[n] {
                Some(format!("the texture accumulation level ({:?} guest texels)", texaccum.map(|t| t.guest_size)))
            } else if pass.normalization[n] {
                Some("the normalization cube".to_string())
            } else {
                match pass.program.stages[n] {
                    Stage::Empty => None,
                    s => Some(format!("{s:?}")),
                }
            }
        });
        let def = pass.program.def.with_default_final_combiner(pixel.fog[0] != 0.0, pixel.fog[3] != 0.0);
        pixel.fog[3] = 0.0;
        Ok(PartUniforms {
            implementation: d.implementation,
            words: pass.words.clone(),
            vertex,
            pixel,
            options: pass.options,
            def,
            pixel_options: pass.program.pixel_options,
            samplers: std::array::from_fn(|n| pass.program.dims[n].and(pass.program.samplers[n].as_ref()).and_then(crate::program::sampler_key)),
            textures,
            primitive: d.state.primitive,
            depth: (d.state.depth_write, d.state.depth_compare),
        })
    }

    /// The sections model `index`'s lit parts come from, each once
    /// (diagnostics).
    pub fn sections(&self, index: u32) -> Vec<u16> {
        let mut s: Vec<u16> = self.models.get(&index).map(|m| m.parts.iter().map(|p| p.section).collect()).unwrap_or_default();
        s.sort_unstable();
        s.dedup();
        s
    }

    /// Issues `draws` (each with its model index, planned at the hook,
    /// [`Self::plan`]) in their order into `color` and `depth` (both
    /// loaded and kept), the viewport the whole target as the programs
    /// expect. A draw sampling the texture accumulation target takes
    /// `texaccum`, its stages' coordinates in that level's guest texels.
    /// Returns the number of ranges recorded so the caller can check the
    /// reservation before replacing any originals.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        view: &ViewInputs,
        draws: &[(u32, PlannedDraw)],
        texaccum: Option<Texaccum<'_>>,
    ) -> usize {
        let Some(packet) = self.bundles(device, queue, uploads, color_format, depth_format,
            target_size, view, draws, texaccum, 0)
        else {
            return 0;
        };
        let count = packet.ranges;
        d3d::backend::ExternalBundles::encode_run(encoder, color, depth, depth_format.has_stencil_aspect(), &[packet]);
        count
    }

    /// Build a packet for an ordered run on compatible colour and depth targets.
    #[allow(clippy::too_many_arguments)]
    pub fn bundles(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        view: &ViewInputs,
        draws: &[(u32, PlannedDraw)],
        texaccum: Option<Texaccum<'_>>,
        run: u64,
    ) -> Option<d3d::backend::ExternalBundles> {
        let mut issued: Vec<(&(u32, PlannedDraw), (VertexUniforms, PixelUniforms))> = Vec::new();
        for d in draws {
            if let Some(blocks) = self.issued(d.0, &d.1, view, texaccum) {
                issued.push((d, blocks));
            }
        }
        if issued.is_empty() {
            return None;
        }
        for (planned, _) in &issued {
            let (model, d) = (planned.0, &planned.1);
            let m = self.models.get_mut(&model).expect("checked in issued");
            let b = &m.buffers[d.buffer];
            let p = &mut m.programs[d.program];
            let key = (color_format, depth_format, b.layout_id, d.state);
            if p.pipelines.contains_key(&key) {
                continue;
            }
            if d.state.stencil.enable && !depth_format.has_stencil_aspect() {
                return None;
            }
            let stencil = gfx::translate::depth_stencil_state(
                &gfx::state::RenderState { stencil: d.state.stencil, ..Default::default() },
                Some(depth_format),
                wgpu::PrimitiveTopology::TriangleList,
            )?.stencil;
            let attrs = attributes(&b.registers, &b.formats);
            let pipeline = gfx::program::validated_resource(device, || device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native object program"),
                layout: Some(&p.layout),
                vertex: wgpu::VertexState {
                    module: &p.module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: (SLOT * b.registers.len()) as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                },
                primitive: d.state.primitive,
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(d.state.depth_write),
                    depth_compare: Some(d.state.depth_compare),
                    stencil,
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &p.module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        // Refused when it blends (`object_state::resolve`).
                        blend: None,
                        write_mask: d.state.color_writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }));
            p.pipelines.insert(key, pipeline);
        }
        let uniforms = uploads.resident_uniform_run(
            device, queue, &self.uniforms_layout, [VertexUniforms::SIZE, PixelUniforms::SIZE],
            issued.len(), |i, v, p| {
                let (_, (vertex, pixel)) = &issued[i];
                vertex.write_bytes(v);
                pixel.write_bytes(p);
            },
        )?;
        let mut groups = Vec::with_capacity(issued.len());
        for (i, (planned, _)) in issued.iter().enumerate() {
            let (model, d) = (planned.0, &planned.1);
            let m = self.models.get_mut(&model)?;
            let b = &m.buffers[d.buffer];
            if d.indices.is_empty() || u64::from(d.indices.end) > b.indices.size() / 4 {
                return None;
            }
            let p = &mut m.programs[d.program];
            let pipeline = p.pipelines.get(&(color_format, depth_format, b.layout_id, d.state))?.as_ref()?;
            let textures = match &p.textures {
                Some(textures) => textures,
                None => {
                    let accum = texaccum.as_ref()?;
                    let slot = if let Some(i) = p.accum_groups.iter().position(|(view, _)| view == accum.view) {
                        i
                    } else {
                        let extra: Vec<_> = (0..4).filter(|&n| p.pass.texaccum[n]).map(|n| (n, accum.view)).collect();
                        let group = gfx::program::validated_resource(device, || {
                            texture_group(device, &p.textures_layout, &p.views, &extra, &p.samplers)
                        });
                        // Keep both target copies hot. A resized target evicts the oldest view.
                        if p.accum_groups.len() == 2 { p.accum_groups.remove(0); }
                        p.accum_groups.push((accum.view.clone(), group));
                        p.accum_groups.len() - 1
                    };
                    p.accum_groups[slot].1.as_ref()?
                }
            };
            let offsets = uniforms.offsets(i);
            let key = BundleKey { pipeline: pipeline.clone(), uniforms: uniforms.group.clone(),
                textures: textures.clone(), offsets, buffer: d.buffer, indices: d.indices.clone() };
            let blend = p.pass.program.state.map_or(wgpu::Color::BLACK, |s| s.blend_constant);
            p.uses += 1;
            if let Some((bundle, used)) = p.bundles.get_mut(&key) {
                *used = p.uses;
                gfx::churn::native_cache(true);
                groups.push((blend, bundle.clone()));
                continue;
            }
            gfx::churn::native_cache(false);
            let mut bundle = device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                label: Some("native object geometry"),
                color_formats: &[Some(color_format)],
                depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                    format: depth_format, depth_read_only: false, stencil_read_only: !depth_format.has_stencil_aspect(),
                }),
                sample_count: 1, multiview: None,
            });
            bundle.set_pipeline(pipeline);
            bundle.set_bind_group(0, &uniforms.group, &offsets);
            bundle.set_bind_group(1, textures, &[]);
            bundle.set_vertex_buffer(0, b.vertices.slice(..));
            bundle.set_index_buffer(b.indices.slice(..), wgpu::IndexFormat::Uint32);
            bundle.draw_indexed(d.indices.clone(), 0, 0..1);
            let bundle = bundle.finish(&Default::default());
            // Keys hold pooled buffers and views. Retire cold bindings without
            // throwing away every hot part at once.
            const MAX_BUNDLES: usize = 256;
            if p.bundles.len() >= MAX_BUNDLES {
                let oldest = p.uses.saturating_sub(MAX_BUNDLES as u64 / 2);
                p.bundles.retain(|_, (_, used)| *used > oldest);
            }
            p.bundles.insert(key, (bundle.clone(), p.uses));
            groups.push((blend, bundle));
        }
        Some(d3d::backend::ExternalBundles {
            run, scissor: [0, 0, target_size.0, target_size.1], ranges: groups.len(), groups,
        })
    }
}

/// [`LitObjects::plan`] over a model's `parts` and its programs' passes
/// (`pass_of`): exactly the part the item's key names (section and part),
/// with exactly the implementation its record picked, its item's shader
/// the part's; then its per-object blocks and its state.
pub fn plan_part<'a>(
    parts: &[LitPart],
    pass_of: &dyn Fn(usize) -> Option<&'a ObjectPass>,
    frame: &ObjectFrame,
    key: &ItemKey,
    rows: &[Row],
    entry: &EntryStates,
) -> Result<PlannedDraw, String> {
    let part = parts.iter().position(|p| p.section == key.section && p.part == key.part);
    plan_indexed_part(parts, part, &|i| pass_of(i).map(|p| (p, None)), frame, key, rows, entry, false)
}

fn index_parts(parts: &[LitPart]) -> HashMap<(u16, u8), usize> {
    let mut index = HashMap::with_capacity(parts.len());
    for (i, p) in parts.iter().enumerate() {
        index.entry((p.section, p.part)).or_insert(i);
    }
    index
}

fn plan_indexed_part<'a>(
    parts: &[LitPart], part: Option<usize>,
    pass_of: &dyn Fn(usize) -> Option<(&'a ObjectPass, Option<&'a InheritedState>)>,
    frame: &ObjectFrame, key: &ItemKey, rows: &[Row], entry: &EntryStates, scene: bool,
) -> Result<PlannedDraw, String> {
    let part = part.ok_or_else(|| format!("section {} part {} is not a native part", key.section, key.part))?;
    let p = &parts[part];
    if p.shader_tag as u16 != key.shader as u16 || (scene && p.shader_tag != key.shader) {
        return Err(format!("the item names shader {:#010x}, the part's is {:#010x}", key.shader, p.shader_tag));
    }
    let (program, buffer) = p
        .implementations
        .get(key.implementation)
        .copied()
        .flatten()
        .ok_or_else(|| format!("the part has no implementation {}", key.implementation))?;
    let (pass, inherited) = pass_of(program).ok_or("its program is missing")?;
    let defaults = frame.defaults.ok_or("the inputs' defaults do not read")?;
    let inputs = ObjectInputs { lighting: frame.lighting.as_ref(), defaults: &defaults, shader: p.shader };
    let vertex = object_vertex_uniforms(&pass.program.vertex, &pass.implementation, &inputs, rows, p.decompression)?;
    let pixel = object_pixel_uniforms(&pass.program.pixel, &pass.implementation, &inputs)?;
    let compiled = pass.program.state.as_ref().ok_or("its pass has no state")?.alpha_test;
    let resolve = if scene { object_state::resolve_scene } else { object_state::resolve };
    let mut cached = inherited.map(|s| s.lock().unwrap_or_else(|e| e.into_inner()));
    let state = if let Some((_, _, state)) = cached.as_deref().and_then(Option::as_ref)
        .filter(|(was_scene, previous, _)| *was_scene == scene && previous == entry) {
        *state
    } else {
        let state = resolve(&pass.implementation.render_states, entry, compiled)?;
        if let Some(slot) = cached.as_deref_mut() { *slot = Some((scene, entry.clone(), state)); }
        state
    };
    Ok(PlannedDraw {
        part,
        implementation: key.implementation,
        program,
        buffer,
        indices: p.indices.clone(),
        vertex,
        pixel,
        state,
        texaccum: pass.texaccum.contains(&true),
        default_final: pass.default_final,
    })
}

/// The engine's textures the lit build may bind: whether the vector
/// normalization cube loads (engine texture 1), what an object's draw
/// samples for the lightmap ([`crate::object_program::OBJECT_LIGHTMAP`])
/// when it is uniform ([`uniform`]), and whether the host binds the
/// texture accumulation target (engine texture 3) per pass.
#[derive(Clone, Debug, Default)]
pub struct EngineStages {
    pub normalization: bool,
    pub lightmap: Option<Image>,
    pub texaccum: bool,
}

/// Sets the coordinate scale of the stages in `stages` (those sampling
/// the texture accumulation target) to one over its guest size: the
/// game's programs address that linear target in texels, as the D3D path
/// samples a linear texture.
pub fn texaccum_scale(pixel: &mut PixelUniforms, stages: [bool; 4], guest_size: (u32, u32)) {
    for (n, on) in stages.into_iter().enumerate() {
        if on {
            pixel.tex_scale[n] = [1.0 / guest_size.0.max(1) as f32, 1.0 / guest_size.1.max(1) as f32, 1.0, 1.0];
        }
    }
}

/// Whether every texel of `image`, at every level, is the same: then it
/// samples the same wherever its coordinates fall, with any filter.
pub fn uniform(image: &Image) -> bool {
    let first = image.rgba.get(..4);
    first.is_some() && image.rgba.chunks_exact(4).chain(image.mips.iter().flat_map(|m| m.chunks_exact(4))).all(|t| Some(t) == first)
}

/// `streams` with a zeroed stream of type [`LIGHTMAP_STREAM`] after them
/// (where the draw adds the lightmap coordinates), its bytes in `zeros`.
fn with_lightmap_stream<'a>(
    elements: &mut Vec<Vec<crate::object_streams::StreamElement>>,
    data: &mut Vec<Option<StreamData<'a>>>,
    formats: &StreamFormats,
    zeros: &'a [u8],
    stride: usize,
) -> Result<(), String> {
    elements.push(formats.elements(LIGHTMAP_STREAM)?);
    data.push(Some(StreamData { bytes: zeros, stride }));
    Ok(())
}

/// The bytes one vertex of stream type `kind` takes.
fn stream_stride(formats: &StreamFormats, kind: u8) -> Result<usize, String> {
    Ok(formats
        .elements(kind)?
        .iter()
        .map(|e| match *e {
            crate::object_streams::StreamElement::Skip(n) => usize::from(n),
            crate::object_streams::StreamElement::Attribute { format, .. } => format.size(),
        })
        .sum())
}

/// One mode section as the lit build reads it: its index (the render
/// record key's section), its classification inputs (`+0x18` geometry
/// classification, `+0x14` influences), its decompression rows, its
/// streams by descriptor slot (stream type, bytes), its vertex count, and
/// per drawn part its material and triangles.
#[derive(Clone)]
pub struct LitSection<'a> {
    pub index: u16,
    pub geometry: u16,
    pub influences: u8,
    pub decompression: [Row; 3],
    pub streams: Vec<Option<(u8, StreamData<'a>)>>,
    pub vertex_count: usize,
    /// Its drawn opaque parts: index among the section's parts (a record
    /// key's bits 0..3), material and triangles.
    pub parts: Vec<(u8, u16, Vec<[u16; 3]>)>,
}

/// A material the lit build takes: resolved, and its shader values.
pub struct LitMaterial {
    pub material: tags::material::Material,
    pub shader: ShaderValues,
}

/// What [`lit_model`] built: the input to upload, the materials taken,
/// those refused with the reason, and the implementations (bit `n`) every
/// taken material has (an object whose records draw another stays on D3D).
#[derive(Default)]
pub struct LitBuild {
    pub input: LitModelInput,
    pub taken: Vec<u16>,
    pub refused: Vec<(u16, String)>,
    pub implementations: u8,
}

/// What a `vrtx` gives a section: its program for the section's
/// classification and that classification's input semantics.
pub type VertexSource<'s> = dyn FnMut(map::TagId, u16, u8) -> Result<(Vec<u8>, Vec<u16>), String> + 's;

/// A stage's image as the D3D path samples it (stage, bitmap, image).
pub type StageSource<'s> = dyn FnMut(usize, map::TagId, u32) -> Option<Image> + 's;

/// Builds the lit parts of `sections` for `materials` (model-local
/// material -> what it draws with): per material, each implementation of
/// its layer-3 pass (the record picks one, `0x0004cc70`) compiled for
/// every section it is in, through the declaration the game would build
/// there (with `engine`'s textures for the stages that sample the
/// normalization cube or the lightmap; one sampling the lightmap reads
/// the coordinates the draw adds as zeros, `engine.lightmap` being
/// uniform); an implementation that fails anywhere is left out. A
/// material
/// none of whose implementations compiles is refused, with the reasons;
/// one in no drawn section is neither taken nor refused. Each part
/// independently matches the implementation in its sorted dispatch.
pub fn lit_model(
    sections: &[LitSection<'_>],
    materials: &HashMap<u16, LitMaterial>,
    formats: &StreamFormats,
    engine: &EngineStages,
    vertex: &mut VertexSource<'_>,
    image: &mut StageSource<'_>,
) -> LitBuild {
    let mut out = LitBuild { implementations: u8::MAX, ..LitBuild::default() };
    // (section, declaration) -> buffer.
    let mut buffers: HashMap<(u16, Vec<Element>), usize> = HashMap::new();
    // Per section, its lit parts' triangles in one list: (material, part,
    // range), one per part (a record draws one part).
    let mut lists: HashMap<u16, (Vec<u32>, Vec<(u16, u8, Range<u32>)>)> = HashMap::new();
    for s in sections {
        let mut list = Vec::new();
        let mut ranges = Vec::new();
        for (part, material, triangles) in &s.parts {
            if !materials.contains_key(material) {
                continue;
            }
            let start = list.len() as u32;
            list.extend(triangles.iter().flatten().map(|&i| u32::from(i)));
            ranges.push((*material, *part, start..list.len() as u32));
        }
        lists.insert(s.index, (list, ranges));
    }
    let mut ids: Vec<u16> = materials.keys().copied().collect();
    ids.sort_unstable();
    for m in ids {
        let lit = &materials[&m];
        let pass = match object_layer3(&lit.material) {
            Ok(p) => p,
            Err(e) => {
                out.refused.push((m, e));
                continue;
            }
        };
        // The material's parts: (section, part, index range).
        let parts: Vec<(&LitSection<'_>, u8, Range<u32>)> = sections
            .iter()
            .flat_map(|s| lists[&s.index].1.iter().filter(move |r| r.0 == m).map(move |r| (s, r.1, r.2.clone())))
            .collect();
        if parts.is_empty() {
            continue;
        }
        // Compiled once per section: its parts share the programs.
        let mut secs: Vec<&LitSection<'_>> = Vec::new();
        for (s, _, _) in &parts {
            if !secs.iter().any(|x| x.index == s.index) {
                secs.push(s);
            }
        }
        let mut per: Vec<[Option<(LitProgramInput, usize)>; IMPLEMENTATIONS]> = (0..secs.len()).map(|_| Default::default()).collect();
        let mut mask = 0u8;
        let mut reasons = Vec::new();
        for imp in pass.implementations.iter().filter(|i| i.index < IMPLEMENTATIONS) {
            let n = imp.index;
            // Buffers made for an implementation that fails later stay
            // unused.
            let samples_lightmap = engine.lightmap.is_some()
                && imp.textures.iter().flatten().any(|t| t.source == tags::material::TextureSource::Lightmap);
            let mut compile = || -> Result<Vec<(LitProgramInput, usize)>, String> {
                let vs = imp.vertex_shader.ok_or_else(|| format!("implementation {n} has no vertex program"))?;
                let mut got = Vec::new();
                for s in &secs {
                    let mut streams = s
                        .streams
                        .iter()
                        .map(|t| t.map_or(Ok(Vec::new()), |(kind, _)| formats.elements(kind)))
                        .collect::<Result<Vec<_>, _>>()?;
                    let stride = if samples_lightmap { stream_stride(formats, LIGHTMAP_STREAM)? } else { 0 };
                    let zeros = vec![0u8; stride * s.vertex_count];
                    let mut data: Vec<Option<StreamData<'_>>> = s.streams.iter().map(|t| t.map(|(_, d)| d)).collect();
                    if samples_lightmap {
                        with_lightmap_stream(&mut streams, &mut data, formats, &zeros, stride)?;
                    }
                    let (microcode, semantics) = vertex(vs, s.geometry, s.influences).map_err(|e| format!("section {}: {e}", s.index))?;
                    let decl = declaration(&semantics, &streams);
                    let compiled = object_pass(pass.pass, imp, &microcode, registers(&decl), input_fetch(&decl), engine.lightmap.is_some())
                        .map_err(|e| format!("section {} implementation {n}: {e}", s.index))?;
                    let key = (s.index, decl);
                    let buffer = match buffers.get(&key) {
                        Some(&b) => b,
                        None => {
                            let vertices = gather(&key.1, &data, s.vertex_count).map_err(|e| format!("section {}: {e}", s.index))?;
                            out.input.buffers.push(LitBufferInput {
                                registers: key.1.iter().map(|e| e.register).collect(),
                                formats: key.1.iter().map(|e| e.format).collect(),
                                vertices,
                                indices: lists[&s.index].0.clone(),
                            });
                            buffers.insert(key, out.input.buffers.len() - 1);
                            out.input.buffers.len() - 1
                        }
                    };
                    let mut images: [Option<Image>; 4] = Default::default();
                    for (k, d) in compiled.program.dims.iter().enumerate() {
                        if d.is_none() {
                            continue;
                        }
                        if compiled.normalization[k] {
                            if !engine.normalization {
                                return Err(format!("implementation {n} stage {k} samples the normalization cube, which does not load"));
                            }
                            continue;
                        }
                        if compiled.texaccum[k] {
                            if !engine.texaccum {
                                return Err(format!("implementation {n} stage {k} samples the texture accumulation target, which the host does not bind"));
                            }
                            continue;
                        }
                        if compiled.program.stages[k] == Stage::Lightmap {
                            images[k] = engine.lightmap.clone();
                            continue;
                        }
                        let Stage::Bitmap { bitmap, image: i } = compiled.program.stages[k] else {
                            return Err(format!("implementation {n} stage {k} samples no bitmap"));
                        };
                        images[k] = Some(image(k, bitmap, i).ok_or_else(|| format!("implementation {n} stage {k}'s bitmap does not decode"))?);
                    }
                    got.push((LitProgramInput { pass: compiled, images }, buffer));
                }
                Ok(got)
            };
            match compile() {
                Ok(got) => {
                    mask |= 1 << n;
                    for (slot, x) in per.iter_mut().zip(got) {
                        slot[n] = Some(x);
                    }
                }
                Err(e) => reasons.push(e),
            }
        }
        if mask == 0 {
            let why = if reasons.is_empty() { "no implementation".to_string() } else { reasons.join("; ") };
            out.refused.push((m, why));
            continue;
        }
        let mut by_section: Vec<[Option<(usize, usize)>; IMPLEMENTATIONS]> = Vec::with_capacity(secs.len());
        for slots in per {
            let mut implementations = [None; IMPLEMENTATIONS];
            for (n, slot) in slots.into_iter().enumerate() {
                let Some((program, buffer)) = slot else { continue };
                out.input.programs.push(program);
                implementations[n] = Some((out.input.programs.len() - 1, buffer));
            }
            by_section.push(implementations);
        }
        for (s, part, indices) in parts {
            let k = secs.iter().position(|x| x.index == s.index).expect("listed above");
            let implementations = by_section[k];
            out.input.parts.push(LitPart {
                section: s.index,
                part,
                decompression: s.decompression,
                shader_tag: lit.material.shader.0,
                shader: lit.shader,
                indices,
                implementations,
            });
        }
        out.taken.push(m);
        out.implementations &= mask;
    }
    if out.taken.is_empty() {
        out.implementations = 0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_lighting::Lighting;
    use gfx::state::RenderStateType;

    /// Synthetic stream formats: type 1 a 3-float position (semantic 0),
    /// a 2-float coordinate (semantic 3) and a 3-float normal (semantic 2);
    /// the lightmap stream's type a 2-float coordinate (semantic 7).
    fn formats() -> StreamFormats {
        let size = crate::object_streams::STREAM_FORMAT_SIZE;
        let mut t = vec![0u8; (usize::from(LIGHTMAP_STREAM) + 1) * size];
        t[size..size + 8].copy_from_slice(&[1, 0, 2, 3, 1, 2, 2, 0xff]);
        let l = usize::from(LIGHTMAP_STREAM) * size;
        t[l..l + 4].copy_from_slice(&[LIGHTMAP_STREAM, 7, 1, 0xff]);
        StreamFormats::from_bytes(&t)
    }

    #[test]
    fn a_uniform_image_is_one_texel_everywhere() {
        let grey = Image { width: 2, height: 1, rgba: vec![9, 9, 9, 255, 9, 9, 9, 255], mips: vec![vec![9, 9, 9, 255]], blocks: None };
        assert!(uniform(&grey));
        let mut mip = grey.clone();
        mip.mips[0][0] = 10;
        assert!(!uniform(&mip), "a level below differs");
        let mut texel = grey.clone();
        texel.rgba[4] = 0;
        assert!(!uniform(&texel));
        assert!(!uniform(&Image { width: 0, height: 0, rgba: Vec::new(), mips: Vec::new(), blocks: None }));
    }

    #[test]
    fn an_implementation_sampling_the_lightmap_gets_the_objects_lightmap_and_zeroed_coordinates() {
        use crate::object_program::tests::{lit, rigid_program};
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 0,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 0, vec![[0, 1, 2]])],
        };
        let mut imp = lit();
        imp.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
        imp.textures[3] = Some(tags::material::StageTexture {
            source: tags::material::TextureSource::Lightmap,
            size: None,
            stage_size: None,
        });
        // The combiners sample t3 (PROJECT2D, 5 bits a stage): a stage the
        // shader does not sample has no texture to bind.
        imp.pixel_shader = shaders::PixelShaderDef { texture_modes: 1 << 15, ..Default::default() }.to_bytes();
        let mut materials = HashMap::new();
        materials.insert(0, LitMaterial { material: material(vec![imp]), shader: ShaderValues::default() });
        // v1 reads semantic 7: the lightmap coordinates the draw adds.
        let mut vertex = |_: map::TagId, _: u16, _: u8| Ok::<_, String>((rigid_program(), vec![0u16, 7, 2]));
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let grey = Image { width: 1, height: 1, rgba: vec![128, 128, 128, 255], mips: Vec::new(), blocks: None };
        let without = lit_model(&[section.clone()], &materials, &formats(), &EngineStages::default(), &mut vertex, &mut image);
        assert!(without.taken.is_empty());
        assert!(without.refused[0].1.contains("samples the lightmap"), "{:?}", without.refused);
        let engine = EngineStages { normalization: false, lightmap: Some(grey.clone()), texaccum: false };
        let with = lit_model(&[section], &materials, &formats(), &engine, &mut vertex, &mut image);
        assert_eq!(with.taken, vec![0], "{:?}", with.refused);
        assert_eq!(with.input.programs[0].images[3], Some(grey), "stage 3 binds the object's lightmap");
        let b = &with.input.buffers[0];
        assert_eq!(b.registers, vec![0, 1, 2]);
        assert_eq!(b.decoded(0, 1), [0.0, 0.0, 0.0, 1.0], "v1, the lightmap coordinates, zero");
        assert_eq!(b.decoded(1, 0), [1.0, 0.0, 0.0, 1.0], "vertex 1's position from the section");
    }

    #[test]
    fn an_implementation_sampling_the_texture_accumulation_target_needs_the_host_to_bind_it() {
        use crate::object_program::TEXACCUM_TEXTURE;
        use crate::object_program::tests::{lit, rigid_program};
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 0,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 0, vec![[0, 1, 2]])],
        };
        let mut imp = lit();
        imp.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
        imp.textures[2] = Some(tags::material::StageTexture {
            source: tags::material::TextureSource::Engine(TEXACCUM_TEXTURE),
            size: None,
            stage_size: None,
        });
        imp.pixel_shader = shaders::PixelShaderDef { texture_modes: 1 << 10, ..Default::default() }.to_bytes();
        let mut materials = HashMap::new();
        materials.insert(0, LitMaterial { material: material(vec![imp]), shader: ShaderValues::default() });
        let mut vertex = |_: map::TagId, _: u16, _: u8| Ok::<_, String>((rigid_program(), vec![0u16, 3, 2]));
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let without = lit_model(&[section.clone()], &materials, &formats(), &EngineStages::default(), &mut vertex, &mut image);
        assert!(without.taken.is_empty());
        assert!(without.refused[0].1.contains("texture accumulation"), "{:?}", without.refused);
        let engine = EngineStages { texaccum: true, ..EngineStages::default() };
        let with = lit_model(&[section], &materials, &formats(), &engine, &mut vertex, &mut image);
        assert_eq!(with.taken, vec![0], "{:?}", with.refused);
        assert!(with.input.samples_texaccum());
        let pass = &with.input.programs[0].pass;
        assert_eq!(pass.texaccum, [false, false, true, false]);
        assert_eq!(with.input.programs[0].images[2], None, "no image: the pass's level is bound per encode");
        // Its coordinates are texels of the level.
        let mut pixel = pass.program.pixel.clone();
        texaccum_scale(&mut pixel, pass.texaccum, (640, 480));
        assert_eq!(pixel.tex_scale[2], [1.0 / 640.0, 1.0 / 480.0, 1.0, 1.0]);
        assert_eq!(pixel.tex_scale[0], pass.program.pixel.tex_scale[0]);
    }

    /// Render state `t` set to `value`, at its slot in the game's table.
    fn state(t: RenderStateType, value: u32) -> tags::shader::RenderStateValue {
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = (0..=255u8)
            .find(|&s| RenderStateType::from_guest_index(&table, u32::from(s)) == Some(t))
            .expect("a slot");
        tags::shader::RenderStateValue { state: slot, value }
    }

    fn material(implementations: Vec<tags::material::ResolvedImplementation>) -> tags::material::Material {
        tags::material::Material {
            shader: map::TagId(0xe000_0010),
            template: map::TagId(0xe000_0011),
            inputs: Default::default(),
            layers: vec![tags::material::ResolvedLayer {
                lod: 0,
                layer: crate::look::LIGHTMAP_LAYER,
                passes: vec![tags::material::ResolvedPass {
                    index: 0,
                    pass: map::TagId(0xe000_0001),
                    implementations,
                }],
            }],
        }
    }

    #[test]
    fn a_lit_material_compiles_every_implementation_it_may_draw_over_one_shared_buffer() {
        use crate::object_program::tests::{lit, rigid_program};
        // Three vertices of 32 bytes: position (k, 0, 0), coordinate, normal.
        let mut bytes = Vec::new();
        for k in 0..3 {
            for v in [k as f32, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0] {
                bytes.extend(v.to_le_bytes());
            }
        }
        let section = LitSection {
            index: 4,
            geometry: 0,
            influences: 0,
            decompression: [[1.0; 4]; 3],
            streams: vec![Some((1, StreamData { bytes: &bytes, stride: 32 }))],
            vertex_count: 3,
            parts: vec![(0, 5, vec![[2, 1, 0]]), (1, 0, vec![[0, 1, 2]]), (2, 3, vec![[0, 2, 1]]), (3, 1, vec![[1, 2, 0]])],
        };
        // An opaque pass: no blending, depth written (both must be set).
        let opaque = |index: usize| {
            let mut imp = lit();
            imp.index = index;
            imp.render_states = vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZWriteEnable, 1)];
            imp
        };
        // One reading the time (scalar 2): never compiled.
        let timed = |index: usize| {
            let mut imp = opaque(index);
            imp.input_reads.scalars |= 1 << 2;
            imp
        };
        let lit_material = |imps| LitMaterial { material: material(imps), shader: ShaderValues::default() };
        let mut materials = HashMap::new();
        materials.insert(0, lit_material(vec![opaque(0), opaque(1)]));
        materials.insert(1, lit_material(vec![timed(0)]));
        // In no section: neither taken nor refused.
        materials.insert(2, lit_material(vec![opaque(0), opaque(1)]));
        // Implementation 1 does not compile: 0 alone.
        materials.insert(3, lit_material(vec![opaque(0), timed(1)]));
        let mut asked = Vec::new();
        let mut vertex = |vs: map::TagId, geometry: u16, influences: u8| {
            asked.push((vs, geometry, influences));
            Ok::<_, String>((rigid_program(), vec![0u16, 3, 2]))
        };
        let mut image = |_: usize, _: map::TagId, _: u32| -> Option<Image> { None };
        let LitBuild { input, taken, refused, implementations } = lit_model(&[section], &materials, &formats(), &EngineStages::default(), &mut vertex, &mut image);
        assert_eq!(taken, vec![0, 3], "refused: {refused:?}");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].0, 1);
        assert!(refused[0].1.contains("reads scalar input 2"), "{}", refused[0].1);
        assert_eq!(implementations, 0b01, "material 3 has implementation 0 alone");
        assert_eq!(input.programs.len(), 3);
        assert_eq!(input.buffers.len(), 1, "both implementations read v0..v2 the same way");
        let b = &input.buffers[0];
        assert_eq!(b.registers, vec![0, 1, 2]);
        assert_eq!(b.decoded(1, 0), [1.0, 0.0, 0.0, 1.0], "vertex 1's position");
        assert_eq!(b.decoded(1, 1), [0.5, 0.5, 0.0, 1.0]);
        assert_eq!(b.indices, vec![0, 1, 2, 0, 2, 1, 1, 2, 0], "material 5 is not lit here");
        assert_eq!(input.parts.len(), 2);
        let p = &input.parts[0];
        assert_eq!((p.section, p.part, p.indices.clone()), (4, 1, 0..3));
        assert_eq!(p.implementations, [Some((0, 0)), Some((1, 0)), None, None, None]);
        let p = &input.parts[1];
        assert_eq!((p.part, p.indices.clone()), (2, 3..6));
        assert_eq!(p.implementations, [Some((2, 0)), None, None, None, None]);
        assert!(input.programs.iter().all(|p| p.pass.program.inputs == 0b111));
        assert_eq!(asked[0], (map::TagId(0xe000_0002), 0, 0));
    }

    #[test]
    fn a_buffers_registers_are_slots_in_order_read_as_the_d3d_path_fetches_them() {
        let formats = [Attribute::NormShort(3), Attribute::Float(2), Attribute::NormPacked3];
        let a = attributes(&[0, 3, 9], &formats);
        let got: Vec<(u64, u32, wgpu::VertexFormat)> = a.iter().map(|x| (x.offset, x.shader_location, x.format)).collect();
        assert_eq!(
            got,
            vec![(0, 0, wgpu::VertexFormat::Snorm16x4), (16, 3, wgpu::VertexFormat::Float32x2), (32, 9, wgpu::VertexFormat::Uint32)]
        );
        let decl = [
            Element { register: 0, stream: 0, offset: 0, format: Attribute::NormShort(3) },
            Element { register: 3, stream: 0, offset: 8, format: Attribute::Float(2) },
            Element { register: 9, stream: 0, offset: 16, format: Attribute::NormPacked3 },
        ];
        let fetch = input_fetch(&decl);
        use shaders::vsh::InputFetch as I;
        assert_eq!((fetch[0], fetch[3], fetch[9], fetch[1]), (I::Xyz1, I::Float, I::NormPacked3, I::Float));
        // Every guest format, as the D3D path's raw fetch reads it.
        for code in 0..=17u8 {
            let f = Attribute::from_code(code).unwrap();
            let (format, _) = vertex_fetch(f);
            assert!(format.size() as usize <= SLOT, "{f:?}");
            assert!(format.size() as usize >= f.size(), "{f:?} reads its bytes");
        }
    }

    fn part(section: u16, part: u8, implementations: [Option<(usize, usize)>; IMPLEMENTATIONS]) -> LitPart {
        LitPart {
            section,
            part,
            decompression: [[0.0; 4]; 3],
            shader_tag: 0xe5da_0465,
            shader: ShaderValues::default(),
            indices: u32::from(part) * 3..u32::from(part) * 3 + 3,
            implementations,
        }
    }

    /// An opaque implementation `index` of the lit fixture, setting depth
    /// Equal with no writes and leaving the rest (cull, winding, colour
    /// writes) to the device.
    fn equal_pass(index: usize) -> ObjectPass {
        use crate::object_program::tests::{lit, rigid_program};
        let mut imp = lit();
        imp.index = index;
        imp.render_states = vec![
            state(RenderStateType::AlphaBlendEnable, 0),
            state(RenderStateType::ZWriteEnable, 0),
            state(RenderStateType::ZFunc, gfx::state::CompareFunc::Equal.xbox_value()),
        ];
        object_pass(map::TagId(0xe000_0001), &imp, &rigid_program(), crate::program::INPUT_MASK, [shaders::vsh::InputFetch::Float; 16], true)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// The device at the layer-3 call: W-buffered, LessEqual written,
    /// `CULL_CW`, nothing else on.
    fn entry() -> EntryStates {
        use gfx::state::{CompareFunc, CullMode, DepthBufferMode, FillMode, FrontFace};
        EntryStates::read(|t| {
            Some(match t {
                RenderStateType::ZEnable => DepthBufferMode::W.xbox_value(),
                RenderStateType::ZFunc => CompareFunc::LessEqual.xbox_value(),
                RenderStateType::ZWriteEnable => 1,
                RenderStateType::ColorWriteEnable => 0x0101_0101,
                RenderStateType::CullMode => CullMode::Cw.xbox_value(),
                RenderStateType::FrontFace => FrontFace::Cw.xbox_value(),
                RenderStateType::FillMode | RenderStateType::BackFillMode => FillMode::Solid.xbox_value(),
                _ => 0,
            })
        })
    }

    fn frame() -> ObjectFrame {
        ObjectFrame {
            handle: 1,
            definition: 2,
            nodes: Vec::new(),
            sphere: ([0.0; 3], 1.0),
            lighting: Some(Lighting { words: [0u32; 30], root: Some(0) }),
            defaults: Some(Default::default()),
        }
    }

    #[test]
    fn model_index_matches_the_first_part_in_each_section_and_is_rebuilt_per_model() {
        let p = |section, number| part(section, number, [None; IMPLEMENTATIONS]);
        let index = index_parts(&[p(0, 2), p(1, 2), p(0, 2)]);
        assert_eq!(index.get(&(0, 2)), Some(&0));
        assert_eq!(index.get(&(1, 2)), Some(&1));
        let replacement = index_parts(&[p(1, 2)]);
        assert_eq!(replacement.get(&(1, 2)), Some(&0));
        assert!(!replacement.contains_key(&(0, 2)));
    }

    fn key(section: u16, part: u8, implementation: usize) -> ItemKey {
        ItemKey { record: 9, shader: 0xe5da_0465, section, part, implementation }
    }

    #[test]
    fn an_item_draws_exactly_the_part_and_implementation_its_record_picked() {
        let passes = [equal_pass(0), equal_pass(1), equal_pass(2)];
        let pass_of = |i: usize| passes.get(i);
        // Section 4: part 0 with implementations 0 and 1, part 1 with 2;
        // section 7: part 0 with 0.
        let parts = [
            part(4, 0, [Some((0, 0)), Some((1, 0)), None, None, None]),
            part(4, 1, [None, None, Some((2, 1)), None, None]),
            part(7, 0, [Some((0, 2)), None, None, None, None]),
        ];
        let rows = [[1.0f32; 4]; 3];
        let f = frame();
        let d = plan_part(&parts, &pass_of, &f, &key(4, 1, 2), &rows, &entry()).unwrap();
        assert_eq!((d.part, d.implementation, d.program, d.buffer, d.indices.clone()), (1, 2, 2, 1, 3..6), "part 1 only, not its section's part 0");
        assert_eq!(d.vertex.constants[50], [1.0; 4], "its record's rows");
        let d = plan_part(&parts, &pass_of, &f, &key(4, 0, 1), &rows, &entry()).unwrap();
        assert_eq!((d.part, d.program), (0, 1));
        let refused = |k: ItemKey| plan_part(&parts, &pass_of, &f, &k, &rows, &entry()).unwrap_err();
        assert!(refused(key(4, 0, 2)).contains("no implementation 2"));
        assert!(refused(key(4, 2, 0)).contains("not a native part"));
        assert!(refused(key(5, 0, 0)).contains("not a native part"));
        assert!(refused(ItemKey { shader: 0xe000_0010, ..key(4, 0, 0) }).contains("shader"));
    }

    #[test]
    fn object_bundles_survive_a_new_swap_and_pose() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut objects = LitObjects::new(&ctx.device);
        let pass = equal_pass(0);
        let registers: Vec<_> = (0u8..16).filter(|i| pass.program.inputs & (1u16 << i) != 0).collect();
        let buffer = LitBufferInput {
            vertices: vec![[0; SLOT]; 3 * registers.len()],
            formats: vec![Attribute::Float(4); registers.len()],
            registers, indices: vec![0, 1, 2],
        };
        objects.add_model(&ctx.device, &ctx.queue, 7, LitModelInput {
            programs: vec![LitProgramInput { pass, images: Default::default() }],
            buffers: vec![buffer],
            parts: vec![part(0, 0, [Some((0, 0)), None, None, None, None])],
        });
        let pool = gfx::ring::UploadPool::default();
        let view = ViewInputs { constants: Default::default(), guest_size: (16, 16), size: (16, 16),
            z_scale: 1.0, w_clip: None };
        let mut record = |swap, translate| {
            let arena = pool.begin();
            let rows = [[translate; 4]; 3];
            let draw = objects.plan(7, &frame(), &key(0, 0, 0), &rows, &entry()).unwrap();
            let packet = objects.bundles(&ctx.device, &ctx.queue, &arena, wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Depth24PlusStencil8, view.size, &view, &[(7, draw)], None, swap).unwrap();
            arena.flush(&ctx.queue);
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            packet.groups.into_iter().next().unwrap().1
        };
        let first = record(1, 0.5);
        let before = gfx::churn::snapshot();
        assert_eq!(first, record(2, 0.5));
        assert_eq!((gfx::churn::snapshot() - before).queue_writes, 0);
        assert_eq!(first, record(3, 1.5));
        let total = gfx::churn::snapshot() - before;
        assert_eq!((total.buffers_created, total.queue_writes), (0, 1));
    }

    #[test]
    fn an_items_draw_resolves_every_per_object_input_and_state_field_or_is_refused() {
        let passes = [equal_pass(0)];
        let pass_of = |i: usize| passes.get(i);
        let parts = [part(4, 0, [Some((0, 0)), None, None, None, None])];
        let rows = [[1.0f32; 4]; 3];
        let d = plan_part(&parts, &pass_of, &frame(), &key(4, 0, 0), &rows, &entry()).unwrap();
        assert_eq!((d.state.depth_compare, d.state.depth_write), (wgpu::CompareFunction::Equal, false), "its own");
        assert!(d.state.primitive.cull_mode.is_some(), "the device's cull, not none");
        // Lighting the implementation reads, absent: refused, not drawn
        // with zeros.
        let dark = ObjectFrame { lighting: None, ..frame() };
        assert!(plan_part(&parts, &pass_of, &dark, &key(4, 0, 0), &rows, &entry()).unwrap_err().contains("lighting"));
        let no_defaults = ObjectFrame { defaults: None, ..frame() };
        assert!(plan_part(&parts, &pass_of, &no_defaults, &key(4, 0, 0), &rows, &entry()).unwrap_err().contains("defaults"));
        // A state field the device's array does not give.
        let mut unread = entry();
        unread.0.retain(|(t, _)| *t != RenderStateType::FrontFace);
        assert!(plan_part(&parts, &pass_of, &frame(), &key(4, 0, 0), &rows, &unread).unwrap_err().contains("FrontFace"));
        // The stencil on at the layer-3 call.
        let mut stencil = entry();
        stencil.0.retain(|(t, _)| *t != RenderStateType::StencilEnable);
        stencil.0.push((RenderStateType::StencilEnable, 1));
        assert!(plan_part(&parts, &pass_of, &frame(), &key(4, 0, 0), &rows, &stencil).unwrap_err().contains("stencil"));
    }
}
