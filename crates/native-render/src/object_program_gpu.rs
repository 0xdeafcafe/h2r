//! Objects' lit parts on the GPU, drawn with the game's own vertex
//! programs and pixel shaders ([`crate::object_program`]): per section
//! its vertex streams expanded once through each declaration a program
//! needs ([`crate::object_streams`]), each program uploaded once, and per
//! view one buffer holding every drawn part's vertex and pixel blocks at
//! dynamic offsets (as the lights do, [`crate::light_gpu::run_layout`]),
//! then one render pass over the game's targets
//! (docs/graphics/native-renderer.md, "Object programs").

use std::collections::HashMap;
use std::ops::Range;

use shaders::psh::PixelUniforms;
use shaders::vsh::VertexUniforms;
use wgpu::util::DeviceExt;

use crate::gpu::{Image, texture};
use crate::light_gpu::{run_layout, uniforms_layout};
use crate::look::Stage;
use crate::object_lighting::ShaderValues;
use crate::object_program::{
    IMPLEMENTATIONS, ObjectInputs, ObjectPass, Row, object_layer3, object_pass, object_pixel_uniforms, object_vertex_uniforms,
};
use crate::object_streams::{Element, StreamData, StreamFormats, declaration, expand, registers};
use crate::objects::ObjectFrame;
use crate::program::{ViewInputs, bias_signed, frame_pixel, frame_vertex, sampler_descriptor};

/// One section's vertices through one declaration: float4 per register,
/// `registers` in order, and the section's index list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LitBufferInput {
    pub registers: Vec<u8>,
    pub vertices: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// One compiled implementation and the images of its sampled stages.
pub struct LitProgramInput {
    pub pass: ObjectPass,
    pub images: [Option<Image>; 4],
}

/// One lit part of a model: its mode section (the key the frame's node
/// rows and implementations are found by), the section's decompression,
/// the material's shader values, the index range in its buffers, and per
/// implementation index the program and buffer it draws with.
#[derive(Clone, Debug, PartialEq)]
pub struct LitPart {
    pub section: u16,
    pub decompression: [Row; 3],
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
}

struct LitBuffer {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    registers: Vec<u8>,
}

struct LitProgram {
    pass: ObjectPass,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    textures: wgpu::BindGroup,
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat, Vec<u8>), wgpu::RenderPipeline>,
}

struct LitModel {
    programs: Vec<LitProgram>,
    buffers: Vec<LitBuffer>,
    parts: Vec<LitPart>,
}

/// Every model's lit parts on the GPU.
pub struct LitObjects {
    uniforms_layout: wgpu::BindGroupLayout,
    models: HashMap<u32, LitModel>,
}

/// One part's draw this view.
struct Draw {
    model: u32,
    program: usize,
    buffer: usize,
    indices: Range<u32>,
}

/// The vertex attributes of a buffer holding `registers` as float4s.
pub fn attributes(registers: &[u8]) -> Vec<wgpu::VertexAttribute> {
    registers
        .iter()
        .enumerate()
        .map(|(k, &r)| wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 16 * k as u64,
            shader_location: u32::from(r),
        })
        .collect()
}

/// The parts of `frame` drawn with `parts`, as (part index, implementation
/// index, node rows): each with the implementation its section's records
/// draw (copied at the hook); a part whose implementation, program or rows
/// are missing is left out (the hook checks that none is for a drawn
/// object).
pub fn frame_parts<'a>(frame: &'a ObjectFrame, parts: &[LitPart]) -> Vec<(usize, usize, &'a [Row])> {
    parts
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let imp = frame.implementations.iter().find(|(s, _)| *s == p.section)?.1;
            p.implementations.get(imp)?.as_ref()?;
            let rows = &frame.node_rows.iter().find(|(s, _)| *s == p.section)?.1;
            Some((i, imp, rows.as_slice()))
        })
        .collect()
}

impl LitObjects {
    pub fn new(device: &wgpu::Device) -> Self {
        Self { uniforms_layout: uniforms_layout(device), models: HashMap::new() }
    }

    /// Uploads `input` as model `index`'s lit parts (replacing any).
    pub fn add_model(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, index: u32, input: LitModelInput) {
        let buffers = input
            .buffers
            .iter()
            .map(|b| LitBuffer {
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
            })
            .collect();
        let programs = input.programs.into_iter().map(|p| self.program(device, queue, p)).collect();
        self.models.insert(index, LitModel { programs, buffers, parts: input.parts });
    }

    fn program(&self, device: &wgpu::Device, queue: &wgpu::Queue, input: LitProgramInput) -> LitProgram {
        let LitProgramInput { pass, mut images } = input;
        let p = &pass.program;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native object program"),
            source: wgpu::ShaderSource::Wgsl(p.wgsl.clone().into()),
        });
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
                    view_dimension: wgpu::TextureViewDimension::D2,
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
            // Checked at load: every sampled stage has its image.
            let mut image = images[n].take().unwrap_or(Image { width: 1, height: 1, rgba: vec![0, 0, 0, 255], mips: Vec::new() });
            bias_signed(&mut image, p.signed[n]);
            views.push((n, texture(device, queue, &image, "native object program stage")));
        }
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native object program textures"),
            entries: &layout_entries,
        });
        let mut entries = Vec::new();
        for (n, view) in &views {
            entries.push(wgpu::BindGroupEntry { binding: 2 * *n as u32, resource: wgpu::BindingResource::TextureView(view) });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(&samplers[*n]),
            });
        }
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native object program textures"),
            layout: &textures_layout,
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native object program"),
            bind_group_layouts: &[Some(&self.uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        LitProgram { pass, module, layout, textures, pipelines: HashMap::new() }
    }

    /// Whether model `index` has lit parts.
    pub fn has(&self, index: u32) -> bool {
        self.models.contains_key(&index)
    }

    /// Draws the lit parts of `objects` (each with its model index) into
    /// `color` and `depth` (both loaded and kept), the viewport the whole
    /// target as the programs expect. Returns the parts drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        view: &ViewInputs,
        objects: &[(u32, &ObjectFrame)],
    ) -> usize {
        let mut draws = Vec::new();
        let mut blocks: Vec<(VertexUniforms, PixelUniforms)> = Vec::new();
        for &(model, frame) in objects {
            let Some(m) = self.models.get(&model) else { continue };
            let defaults = frame.defaults.unwrap_or_default();
            for (i, imp, rows) in frame_parts(frame, &m.parts) {
                let part = &m.parts[i];
                let Some((program, buffer)) = part.implementations[imp] else { continue };
                let pass = &m.programs[program].pass;
                let inputs = ObjectInputs { lighting: frame.lighting.as_ref(), defaults: &defaults, shader: part.shader };
                let base = frame_vertex(&pass.program.vertex, view);
                let Ok(vertex) = object_vertex_uniforms(&base, &pass.implementation, &inputs, rows, part.decompression) else { continue };
                let Ok(pixel) = object_pixel_uniforms(&frame_pixel(&pass.program.pixel, view), &pass.implementation, &inputs) else { continue };
                blocks.push((vertex, pixel));
                draws.push(Draw { model, program, buffer, indices: part.indices.clone() });
            }
        }
        if draws.is_empty() {
            return 0;
        }
        for d in &draws {
            let m = self.models.get_mut(&d.model).expect("drawn model");
            let registers = m.buffers[d.buffer].registers.clone();
            let p = &mut m.programs[d.program];
            let key = (color_format, depth_format, registers.clone());
            if p.pipelines.contains_key(&key) {
                continue;
            }
            let state = p.pass.program.state.expect("an object pass has its state");
            let attrs = attributes(&registers);
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native object program"),
                layout: Some(&p.layout),
                vertex: wgpu::VertexState {
                    module: &p.module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 16 * registers.len() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                },
                primitive: state.primitive,
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(state.depth_write),
                    depth_compare: Some(state.depth_compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &p.module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: state.blend,
                        write_mask: state.color_writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            p.pipelines.insert(key, pipeline);
        }
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let (offsets, size) = run_layout(draws.len(), align);
        let mut bytes = vec![0u8; size as usize];
        for ((vertex, pixel), o) in blocks.iter().zip(&offsets) {
            let (v, p) = (o[0] as usize, o[1] as usize);
            let vertex = vertex.to_bytes();
            bytes[v..v + vertex.len()].copy_from_slice(&vertex);
            let pixel = pixel.to_bytes();
            bytes[p..p + pixel.len()].copy_from_slice(&pixel);
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native object program uniforms"),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &bytes);
        let entry = |binding: u32, size: usize| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size as u64),
            }),
        };
        let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native object program uniforms"),
            layout: &self.uniforms_layout,
            entries: &[entry(0, VertexUniforms::SIZE), entry(1, PixelUniforms::SIZE)],
        });
        let has_stencil = depth_format.has_stencil_aspect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native object programs"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: has_stencil.then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(0.0, 0.0, target_size.0 as f32, target_size.1 as f32, 0.0, 1.0);
        let mut drawn = 0;
        for (d, o) in draws.iter().zip(&offsets) {
            let m = &self.models[&d.model];
            let b = &m.buffers[d.buffer];
            let p = &m.programs[d.program];
            let Some(pipeline) = p.pipelines.get(&(color_format, depth_format, b.registers.clone())) else { continue };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &uniforms, o);
            pass.set_bind_group(1, &p.textures, &[]);
            pass.set_blend_constant(p.pass.program.state.map_or(wgpu::Color::BLACK, |s| s.blend_constant));
            pass.set_vertex_buffer(0, b.vertices.slice(..));
            pass.set_index_buffer(b.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(d.indices.clone(), 0, 0..1);
            drawn += 1;
        }
        drawn
    }
}

/// Whether every stage `pass` samples has an image in `images` (a
/// bitmap stage) or nothing to bind (an empty one).
pub fn stages_bound(pass: &ObjectPass, images: &[Option<Image>; 4]) -> bool {
    pass.program.dims.iter().enumerate().all(|(n, d)| d.is_none() || (matches!(pass.program.stages[n], Stage::Bitmap { .. }) && images[n].is_some()))
}

/// One mode section as the lit build reads it: its index (the render
/// record key's section), its classification inputs (`+0x18` geometry
/// classification, `+0x14` influences), its decompression rows, its
/// streams by descriptor slot (stream type, bytes), its vertex count, and
/// per drawn part its material and triangles.
pub struct LitSection<'a> {
    pub index: u16,
    pub geometry: u16,
    pub influences: u8,
    pub decompression: [Row; 3],
    pub streams: Vec<Option<(u8, StreamData<'a>)>>,
    pub vertex_count: usize,
    pub parts: Vec<(u16, Vec<[u16; 3]>)>,
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
/// there; an implementation that fails anywhere is left out. A material
/// none of whose implementations compiles is refused, with the reasons;
/// one in no drawn section is neither taken nor refused. When the taken
/// materials share no implementation, they are all refused.
pub fn lit_model(
    sections: &[LitSection<'_>],
    materials: &HashMap<u16, LitMaterial>,
    formats: &StreamFormats,
    vertex: &mut VertexSource<'_>,
    image: &mut StageSource<'_>,
) -> LitBuild {
    let mut out = LitBuild { implementations: u8::MAX, ..LitBuild::default() };
    // (section, declaration) -> buffer.
    let mut buffers: HashMap<(u16, Vec<Element>), usize> = HashMap::new();
    // Per section, its lit parts' triangles in one list: (material, range).
    let mut lists: HashMap<u16, (Vec<u32>, Vec<(u16, Range<u32>)>)> = HashMap::new();
    for s in sections {
        let mut list = Vec::new();
        let mut ranges = Vec::new();
        for (material, triangles) in &s.parts {
            if !materials.contains_key(material) {
                continue;
            }
            let start = list.len() as u32;
            list.extend(triangles.iter().flatten().map(|&i| u32::from(i)));
            ranges.push((*material, start..list.len() as u32));
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
        // The material's parts: (section, index range).
        let parts: Vec<(&LitSection<'_>, Range<u32>)> = sections
            .iter()
            .flat_map(|s| lists[&s.index].1.iter().filter(move |r| r.0 == m).map(move |r| (s, r.1.clone())))
            .collect();
        if parts.is_empty() {
            continue;
        }
        let mut per: Vec<[Option<(LitProgramInput, usize)>; IMPLEMENTATIONS]> = (0..parts.len()).map(|_| Default::default()).collect();
        let mut mask = 0u8;
        let mut reasons = Vec::new();
        for imp in pass.implementations.iter().filter(|i| i.index < IMPLEMENTATIONS) {
            let n = imp.index;
            // Buffers made for an implementation that fails later stay
            // unused.
            let mut compile = || -> Result<Vec<(LitProgramInput, usize)>, String> {
                let vs = imp.vertex_shader.ok_or_else(|| format!("implementation {n} has no vertex program"))?;
                let mut got = Vec::new();
                for (s, _) in &parts {
                    let streams = s
                        .streams
                        .iter()
                        .map(|t| t.map_or(Ok(Vec::new()), |(kind, _)| formats.elements(kind)))
                        .collect::<Result<Vec<_>, _>>()?;
                    let (microcode, semantics) = vertex(vs, s.geometry, s.influences).map_err(|e| format!("section {}: {e}", s.index))?;
                    let decl = declaration(&semantics, &streams);
                    let compiled = object_pass(pass.pass, imp, &microcode, registers(&decl))
                        .map_err(|e| format!("section {} implementation {n}: {e}", s.index))?;
                    let key = (s.index, decl);
                    let buffer = match buffers.get(&key) {
                        Some(&b) => b,
                        None => {
                            let data: Vec<Option<StreamData<'_>>> = s.streams.iter().map(|t| t.map(|(_, d)| d)).collect();
                            let vertices = expand(&key.1, &data, s.vertex_count).map_err(|e| format!("section {}: {e}", s.index))?;
                            out.input.buffers.push(LitBufferInput {
                                registers: key.1.iter().map(|e| e.register).collect(),
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
        for ((s, indices), slots) in parts.into_iter().zip(per) {
            let mut implementations = [None; IMPLEMENTATIONS];
            for (n, slot) in slots.into_iter().enumerate() {
                let Some((program, buffer)) = slot else { continue };
                out.input.programs.push(program);
                implementations[n] = Some((out.input.programs.len() - 1, buffer));
            }
            out.input.parts.push(LitPart {
                section: s.index,
                decompression: s.decompression,
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
    } else if out.implementations == 0 {
        let taken = std::mem::take(&mut out.taken);
        out.refused.extend(taken.into_iter().map(|m| (m, "the model's lit materials share no implementation".to_string())));
        out.input = LitModelInput::default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_lighting::Lighting;
    use gfx::state::RenderStateType;

    /// Synthetic stream formats: type 1 a 3-float position (semantic 0),
    /// a 2-float coordinate (semantic 3) and a 3-float normal (semantic 2).
    fn formats() -> StreamFormats {
        let mut t = vec![0u8; 2 * crate::object_streams::STREAM_FORMAT_SIZE];
        let r = &mut t[crate::object_streams::STREAM_FORMAT_SIZE..];
        r[..8].copy_from_slice(&[1, 0, 2, 3, 1, 2, 2, 0xff]);
        StreamFormats::from_bytes(&t)
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
            parts: vec![(5, vec![[2, 1, 0]]), (0, vec![[0, 1, 2]]), (3, vec![[0, 2, 1]]), (1, vec![[1, 2, 0]])],
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
        let LitBuild { input, taken, refused, implementations } = lit_model(&[section], &materials, &formats(), &mut vertex, &mut image);
        assert_eq!(taken, vec![0, 3], "refused: {refused:?}");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].0, 1);
        assert!(refused[0].1.contains("reads scalar input 2"), "{}", refused[0].1);
        assert_eq!(implementations, 0b01, "material 3 has implementation 0 alone");
        assert_eq!(input.programs.len(), 3);
        assert_eq!(input.buffers.len(), 1, "both implementations read v0..v2 the same way");
        let b = &input.buffers[0];
        assert_eq!(b.registers, vec![0, 1, 2]);
        assert_eq!(b.vertices[3], [1.0, 0.0, 0.0, 1.0], "vertex 1's position");
        assert_eq!(b.vertices[4], [0.5, 0.5, 0.0, 1.0]);
        assert_eq!(b.indices, vec![0, 1, 2, 0, 2, 1, 1, 2, 0], "material 5 is not lit here");
        assert_eq!(input.parts.len(), 2);
        let p = &input.parts[0];
        assert_eq!((p.section, p.indices.clone()), (4, 0..3));
        assert_eq!(p.implementations, [Some((0, 0)), Some((1, 0)), None, None, None]);
        let p = &input.parts[1];
        assert_eq!(p.indices, 3..6);
        assert_eq!(p.implementations, [Some((2, 0)), None, None, None, None]);
        assert!(input.programs.iter().all(|p| p.pass.program.inputs == 0b111));
        assert_eq!(asked[0], (map::TagId(0xe000_0002), 0, 0));
    }

    #[test]
    fn a_buffers_registers_are_float4s_in_order() {
        let a = attributes(&[0, 3, 9]);
        let got: Vec<(u64, u32)> = a.iter().map(|x| (x.offset, x.shader_location)).collect();
        assert_eq!(got, vec![(0, 0), (16, 3), (32, 9)]);
        assert!(a.iter().all(|x| x.format == wgpu::VertexFormat::Float32x4));
    }

    fn part(section: u16, implementations: [Option<(usize, usize)>; IMPLEMENTATIONS]) -> LitPart {
        LitPart {
            section,
            decompression: [[0.0; 4]; 3],
            shader: ShaderValues::default(),
            indices: 0..3,
            implementations,
        }
    }

    #[test]
    fn an_object_draws_each_part_with_the_implementation_its_records_pick() {
        let words = [0u32; 30];
        let mut f = ObjectFrame {
            handle: 1,
            definition: 2,
            nodes: Vec::new(),
            sphere: ([0.0; 3], 1.0),
            lighting: Some(Lighting { words, root: Some(0) }),
            defaults: None,
            node_rows: vec![(4, vec![[1.0; 4]; 3]), (7, vec![[1.0; 4]; 3])],
            implementations: vec![(4, 1)],
        };
        let parts = [
            part(4, [Some((0, 0)), Some((1, 0)), None, None, None]),
            part(4, [None, None, Some((2, 1)), None, None]),
            part(7, [Some((0, 0)), Some((1, 0)), None, None, None]),
        ];
        let picked: Vec<(usize, usize)> = frame_parts(&f, &parts).iter().map(|&(i, imp, _)| (i, imp)).collect();
        assert_eq!(picked, vec![(0, 1)], "part 1 has no implementation 1; section 7 no implementation copied");
        f.implementations = vec![(4, 2), (7, 0)];
        let picked: Vec<(usize, usize)> = frame_parts(&f, &parts).iter().map(|&(i, imp, _)| (i, imp)).collect();
        assert_eq!(picked, vec![(1, 2), (2, 0)]);
        f.node_rows.truncate(1);
        assert_eq!(frame_parts(&f, &parts).len(), 1, "section 7 has no rows");
    }
}
