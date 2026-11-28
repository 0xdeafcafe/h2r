//! The BSP ocean drawn from the map (docs/graphics/retire-d3d-display.md,
//! "The water end"): the ocean shader's layer-3 pass 0, implementation 0,
//! compiled as an object pass ([`crate::object_program::object_layer_pass`])
//! with the grid stream's declaration, and drawn with the frame's words
//! (`frame_scene::late::Ocean`) on the bound target.
//!
//! | Piece | Made from |
//! |---|---|
//! | vertex program | the `vrtx`'s rigid classification, its inputs the grid stream's ([`compile`]) |
//! | `c[0..18]` | the view's, from the device |
//! | `c[21..25]`, bump matrices of stages 1 to 3, fog colour | the definition's parameter block (`frame_scene::StructureParameters`) |
//! | `c[50..53]` | the grid's rows (`0x004c1b38`) |
//! | stages | map bitmaps; engine textures 14, 15 the rasterizer's; 19 and 20 targets `0x15` and `0x16` sampled in place, in texels of their guest size |
//! | geometry | the grid stream's vertices and the quads as triangles on the Xbox diagonal ([`quad_triangles`]) |
//!
//! A stage, state or input the object path refuses is a load error, which
//! the draw names (`water ocean: program`); there is no other draw.

use std::collections::HashMap;
use std::sync::Arc;

use frame_scene::StructureParameters;
use gfx::compile::PendingPipeline;
use gfx::pipeline::{ShaderId, ShaderProgram};
use gfx::vertex::OwnedVertexBufferLayout;
use shaders::psh::PixelUniforms;
use shaders::vsh::VertexUniforms;
use tags::material::{Material, ResolvedImplementation, TextureSource};
use tags::shader::AnimatedProperty;

use crate::gpu::{Image, texture};
use crate::look::Stage;
use crate::object_program::{ObjectPass, VIEW_TEXTURE, animated_blocks, object_layer_pass};
use crate::object_program_gpu::{attributes, input_fetch, texaccum_scale, with_structure_parameters};
use crate::object_streams::{Element, SLOT, StreamData, StreamElement, declaration, gather, registers};
use crate::program::{MODEL_ROWS, ViewInputs, bias_signed, described_pipeline, frame_pixel, frame_vertex, sampler_descriptor, uniform_entry};

/// The shader layer the ocean draws.
pub const OCEAN_LAYER: usize = 3;
/// The engine textures of the reflection (target `0x15`) and the
/// refraction, the scene copy `0x0003bea0` makes (target `0x16`).
pub const REFLECTION_TEXTURE: u8 = 19;
pub const REFRACTION_TEXTURE: u8 = 20;

/// The shader id the ocean's pipeline reports.
const OCEAN_PROGRAM: ShaderId = ShaderId(u32::MAX - 7);

/// The game target engine texture `id` names when an ocean stage samples
/// it in place: 19 and 20 only (`crate::screen_look::engine_texture`).
pub fn sampled_target(id: u8) -> Option<u8> {
    match (id, crate::screen_look::engine_texture(id)) {
        (REFLECTION_TEXTURE | REFRACTION_TEXTURE, crate::screen_look::EngineTexture::Target(t)) => u8::try_from(t).ok(),
        _ => None,
    }
}

/// What the load gives the compile: the ocean shader's material and
/// animated properties, the globals' rasterizer bitmap (engine textures
/// 14 and 15), and the pass's `vrtx` rigid classification (microcode and
/// input semantics).
pub struct OceanSource<'a> {
    pub material: &'a Material,
    pub properties: &'a [AnimatedProperty],
    pub rasterizer: Option<map::TagId>,
    pub microcode: &'a [u8],
    pub semantics: &'a [u16],
}

/// The ocean's pass compiled for its grid stream.
#[derive(Clone, Debug)]
pub struct OceanProgram {
    pub pass: ObjectPass,
    /// Per stage, the game target it samples in place (`ObjectPass::view`
    /// marks the same stages).
    pub targets: [Option<u8>; 4],
    /// The grid stream's declaration for the program's inputs.
    pub declaration: Vec<Element>,
    /// Bytes per grid vertex in the guest's stream.
    pub stride: usize,
}

impl OceanProgram {
    /// The targets the pass samples ([`sampled_targets`]).
    pub fn sampled(&self) -> Vec<u8> {
        sampled_targets(&self.targets)
    }
}

/// The targets of `targets`, in stage order, each once: the order of the
/// sampled levels a recording is handed.
pub fn sampled_targets(targets: &[Option<u8>; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    for &t in targets.iter().flatten() {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// `imp` with its engine stages as the object path takes them: 19 and 20
/// a view stage whose target is kept in the second value, 14 and 15 the
/// rasterizer bitmap's images 0 and 1. Any other engine id is an error
/// naming its stage.
pub fn supplied(imp: &ResolvedImplementation, rasterizer: Option<map::TagId>) -> Result<(ResolvedImplementation, [Option<u8>; 4]), String> {
    let mut out = imp.clone();
    let mut targets = [None; 4];
    for (n, t) in out.textures.iter_mut().enumerate() {
        let Some(t) = t else { continue };
        let TextureSource::Engine(id) = t.source else { continue };
        if let Some(target) = sampled_target(id) {
            targets[n] = Some(target);
            t.source = TextureSource::Engine(VIEW_TEXTURE);
        } else if let (14 | 15, Some(bitmap)) = (id, rasterizer) {
            t.source = TextureSource::Bitmap { bitmap, image: crate::look::rasterizer_image(id), lod_offset: 0.0 };
        } else {
            return Err(format!("stage {n} binds engine texture {id}, which the ocean does not supply"));
        }
    }
    Ok((out, targets))
}

/// Bytes per vertex of a stream of format `stream`.
pub fn stride(stream: &[StreamElement]) -> usize {
    stream
        .iter()
        .map(|e| match *e {
            StreamElement::Skip(n) => usize::from(n),
            StreamElement::Attribute { format, .. } => format.size(),
        })
        .sum()
}

/// The ocean's layer-3 pass 0 and its implementation 0, or why not: what
/// the load reads the vertex shader and bitmaps of, and [`compile`] takes.
pub fn implementation(material: &Material) -> Result<(&tags::material::ResolvedPass, &ResolvedImplementation), String> {
    let layer = material
        .layers
        .iter()
        .find(|l| l.lod == 0 && l.layer == OCEAN_LAYER)
        .ok_or("no layer 3 at the first level of detail")?;
    let pass = layer.passes.first().ok_or("no pass 0 in layer 3")?;
    let imp = pass.implementations.first().ok_or("layer 3 pass 0 has no implementation")?;
    Ok((pass, imp))
}

/// The ocean's layer-3 pass 0, implementation 0, compiled for a grid
/// stream of format `stream`, or why not.
pub fn compile(src: &OceanSource<'_>, stream: &[StreamElement]) -> Result<OceanProgram, String> {
    let (pass, imp) = implementation(src.material)?;
    let (imp, targets) = supplied(imp, src.rasterizer)?;
    let declaration = declaration(src.semantics, &[stream.to_vec()]);
    let compiled = object_layer_pass(pass.pass, &imp, src.properties, src.microcode, registers(&declaration), input_fetch(&declaration))?;
    for n in 0..4 {
        if compiled.normalization[n] || compiled.texaccum[n] || compiled.scene_copy[n] {
            return Err(format!("stage {n} binds an engine texture the ocean does not supply"));
        }
    }
    Ok(OceanProgram { pass: compiled, targets, declaration, stride: stride(stream) })
}

/// One draw's inputs.
#[derive(Clone, Copy, Debug)]
pub struct OceanInputs<'a> {
    pub view: ViewInputs,
    /// `c[50..53]`, the grid's rows.
    pub rows: [[f32; 4]; 3],
    pub structure: &'a StructureParameters,
    /// The render clock, for an animated pass.
    pub time: Option<f64>,
    /// Per stage that samples a target, the target's guest size.
    pub target_sizes: [Option<(u32, u32)>; 4],
}

/// The vertex and pixel blocks of one draw: the pass's (sampled at the
/// render clock), the view's, the rows, the parameter block's constants,
/// bump matrices and fog colour, and the target stages' texel scale. An
/// animated stage image is an error: the set binds the compiled ones.
pub fn uniforms(p: &OceanProgram, i: &OceanInputs<'_>) -> Result<(VertexUniforms, PixelUniforms), String> {
    let (vertex, pixel, frames) = animated_blocks(&p.pass, i.time)?;
    if let Some(n) = frames.iter().position(Option::is_some) {
        return Err(format!("stage {n} animates its bitmap's frame"));
    }
    let mut vertex = frame_vertex(&vertex, &i.view);
    vertex.constants[MODEL_ROWS..MODEL_ROWS + 3].copy_from_slice(&i.rows);
    let mut pixel = frame_pixel(&pixel, &i.view);
    with_structure_parameters(&p.pass, Some(i.structure), &mut vertex, &mut pixel)?;
    pixel.fog_color = shaders::psh::argb(i.structure.fog_colour);
    for (n, target) in p.targets.iter().enumerate() {
        if target.is_some() {
            let size = i.target_sizes[n].ok_or_else(|| format!("stage {n}'s target has no size"))?;
            let mut one = [false; 4];
            one[n] = true;
            texaccum_scale(&mut pixel, one, size);
        }
    }
    Ok((vertex, pixel))
}

/// The quads as triangles on the Xbox quad's diagonal: `[a, b, c, d]` as
/// `a b c` and `a c d`, corners in the guest's order.
pub fn quad_triangles(quads: &[[u16; 4]]) -> Vec<u32> {
    quads.iter().flat_map(|&[a, b, c, d]| [a, b, c, a, c, d].map(u32::from)).collect()
}

/// The vertices the quads reach: one past the highest index.
pub fn vertex_count(quads: &[[u16; 4]]) -> usize {
    quads.iter().flatten().max().map_or(0, |&i| usize::from(i) + 1)
}

/// The grid as the GPU takes it: per vertex, per declared register, a
/// [`SLOT`] of the guest's bytes; and the triangle indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OceanGrid {
    /// What the bytes were read from (the stream's vertex address and the
    /// quads' hash): a set uploads a grid of a new key again.
    pub key: u64,
    pub vertices: Vec<[u8; SLOT]>,
    pub indices: Vec<u32>,
}

/// The grid of `quads` over the stream's `bytes` (from its first vertex),
/// `p`'s declaration and stride.
pub fn grid(p: &OceanProgram, bytes: &[u8], quads: &[[u16; 4]], key: u64) -> Result<OceanGrid, String> {
    grid_of(&p.declaration, p.stride, bytes, quads, key)
}

fn grid_of(declaration: &[Element], stride: usize, bytes: &[u8], quads: &[[u16; 4]], key: u64) -> Result<OceanGrid, String> {
    let vertices = gather(declaration, &[Some(StreamData { bytes, stride })], vertex_count(quads))?;
    Ok(OceanGrid { key, vertices, indices: quad_triangles(quads) })
}

/// One draw as a recording takes it.
pub struct OceanDraw<'a> {
    /// The device's `c[0..18]` and W clip at the issue.
    pub view: [[f32; 4]; crate::program::VIEW_CONSTANTS],
    pub w_clip: Option<([f32; 2], bool)>,
    pub rows: [[f32; 4]; 3],
    pub structure: &'a StructureParameters,
    pub time: Option<f64>,
    pub grid: &'a OceanGrid,
    /// Per target in [`OceanProgram::sampled`]'s order, its guest size.
    pub target_sizes: &'a [(u32, u32)],
}

/// The ocean on the GPU: its pipeline per attachment formats, uniform
/// buffers, the bitmap stages, and the grid last uploaded.
pub struct OceanSet {
    program: Arc<OceanProgram>,
    pipelines: crate::pipelines::Pipelines,
    vertices: OwnedVertexBufferLayout,
    pending: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), Arc<PendingPipeline>>,
    textures_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    pixel_buffer: wgpu::Buffer,
    uniforms: wgpu::BindGroup,
    samplers: [wgpu::Sampler; 4],
    stage_views: [Option<wgpu::TextureView>; 4],
    grid: Option<(u64, wgpu::Buffer, wgpu::Buffer, u32)>,
}

impl OceanSet {
    /// Uploads `program`: `stage_image` decodes a bitmap stage. An error
    /// names a stage with no image, or a program that does not validate.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: Arc<OceanProgram>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
    ) -> Result<Self, String> {
        let p = &program.pass.program;
        let mut stage_views: [Option<wgpu::TextureView>; 4] = Default::default();
        for (n, &stage) in p.stages.iter().enumerate() {
            if p.dims[n].is_none() || program.targets[n].is_some() {
                continue;
            }
            match stage {
                Stage::Bitmap { .. } => {
                    let mut image = stage_image(stage).ok_or_else(|| format!("stage {n}'s bitmap did not decode"))?;
                    bias_signed(&mut image, p.signed[n]);
                    stage_views[n] = Some(texture(device, queue, &image, "native ocean stage"));
                }
                other => return Err(format!("stage {n} samples {other:?}, which the ocean does not supply")),
            }
        }
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native ocean uniforms"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT), uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let mut entries = Vec::new();
        for (n, dim) in p.dims.iter().enumerate() {
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
            label: Some("native ocean textures"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native ocean program"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let module = gfx::program::validated_resource(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native ocean program"),
                source: wgpu::ShaderSource::Wgsl(p.wgsl.as_str().into()),
            })
        })
        .ok_or("the ocean program does not validate")?;
        let buffer = |label, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let vertex_buffer = buffer("native ocean vertex", VertexUniforms::SIZE);
        let pixel_buffer = buffer("native ocean pixel", PixelUniforms::SIZE);
        let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native ocean uniforms"),
            layout: &uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: vertex_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: pixel_buffer.as_entire_binding() },
            ],
        });
        let samplers = std::array::from_fn(|n| device.create_sampler(&sampler_descriptor(p.samplers[n].as_ref())));
        let registers: Vec<u8> = program.declaration.iter().map(|e| e.register).collect();
        let formats: Vec<_> = program.declaration.iter().map(|e| e.format).collect();
        let vertices = OwnedVertexBufferLayout {
            stride: (SLOT * program.declaration.len()) as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attributes(&registers, &formats),
        };
        let shader = ShaderProgram { module, layout, vs_entry: "vs_main".into(), fs_entry: "fs_main".into(), count_entry: None };
        let pipelines = crate::pipelines::Pipelines::new(device, &p.wgsl, shader, "native ocean program");
        Ok(Self {
            program,
            pipelines,
            vertices,
            pending: HashMap::new(),
            textures_layout,
            vertex_buffer,
            pixel_buffer,
            uniforms,
            samplers,
            stage_views,
            grid: None,
        })
    }

    /// Whether this set holds `program`.
    pub fn holds(&self, program: &Arc<OceanProgram>) -> bool {
        Arc::ptr_eq(&self.program, program)
    }

    fn pipeline(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> Result<wgpu::RenderPipeline, &'static str> {
        let state = self.program.pass.program.state.as_ref();
        let (pipelines, vertices) = (&self.pipelines, &self.vertices);
        let pending = self.pending.entry((color, depth)).or_insert_with(|| {
            let mut key = described_pipeline(OCEAN_PROGRAM, color, depth, state, None, wgpu::StencilState::default());
            key.vertex_buffers.push(vertices.clone());
            pipelines.request(device, &key)
        });
        pending.get().clone().ok_or("pipeline")
    }

    fn upload_grid(&mut self, device: &wgpu::Device, grid: &OceanGrid) {
        if self.grid.as_ref().is_some_and(|(key, ..)| *key == grid.key) {
            return;
        }
        use wgpu::util::DeviceExt;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native ocean grid"),
            contents: grid.vertices.as_flattened(),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let bytes: Vec<u8> = grid.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native ocean grid indices"),
            contents: &bytes,
            usage: wgpu::BufferUsages::INDEX,
        });
        self.grid = Some((grid.key, vertices, indices, grid.indices.len() as u32));
    }

    /// Records the draw on `t`'s attachments: its uniforms staged in the
    /// recording's uploads, then one render pass that loads both.
    /// `t.sampled` holds the targets in [`OceanProgram::sampled`]'s order.
    pub fn record(&mut self, t: &mut d3d::ExternalTarget<'_>, draw: &OceanDraw<'_>) -> Result<(), &'static str> {
        let sampled = self.program.sampled();
        if t.sampled.len() != sampled.len() || draw.target_sizes.len() != sampled.len() {
            return Err("target");
        }
        let mut views: Vec<(usize, wgpu::TextureView)> = Vec::new();
        let mut sizes = [None; 4];
        for (n, target) in self.program.targets.iter().enumerate() {
            let Some(target) = target else {
                if let Some(v) = &self.stage_views[n] {
                    views.push((n, v.clone()));
                }
                continue;
            };
            let at = sampled.iter().position(|s| s == target).ok_or("target")?;
            let level = t.sampled[at].as_ref().ok_or("target")?;
            let texture = level.view.texture();
            if texture == t.color.texture() || texture == t.depth.texture() {
                return Err("target");
            }
            views.push((n, level.view.clone()));
            sizes[n] = Some(draw.target_sizes[at]);
        }
        let view = ViewInputs {
            constants: t.view_constants(draw.view),
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
            w_clip: draw.w_clip,
        };
        let inputs = OceanInputs { view, rows: draw.rows, structure: draw.structure, time: draw.time, target_sizes: sizes };
        let (vertex, pixel) = uniforms(&self.program, &inputs).map_err(|_| "uniforms")?;
        let pipeline = self.pipeline(t.device, t.color_format, t.depth_format)?;
        self.upload_grid(t.device, draw.grid);
        let mut entries = Vec::new();
        for (n, v) in &views {
            entries.push(wgpu::BindGroupEntry { binding: 2 * *n as u32, resource: wgpu::BindingResource::TextureView(v) });
            entries.push(wgpu::BindGroupEntry { binding: 2 * *n as u32 + 1, resource: wgpu::BindingResource::Sampler(&self.samplers[*n]) });
        }
        let textures = gfx::program::validated_resource(t.device, || {
            t.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("native ocean textures"), layout: &self.textures_layout, entries: &entries })
        })
        .ok_or("textures")?;
        {
            let mut uploads = t.encoder_uploads();
            uploads.write(&self.vertex_buffer, 0, &vertex.to_bytes());
            uploads.write(&self.pixel_buffer, 0, &pixel.to_bytes());
        }
        let (_, vertices, indices, count) = self.grid.as_ref().ok_or("grid")?;
        let stencil = t.depth_format.has_stencil_aspect();
        let mut pass = t.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native ocean"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: t.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: t.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: stencil.then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
            }),
            ..Default::default()
        });
        let [x, y, w, h] = t.viewport;
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &self.uniforms, &[]);
        pass.set_bind_group(1, &textures, &[]);
        if let Some(state) = &self.program.pass.program.state {
            pass.set_blend_constant(state.blend_constant);
        }
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..*count, 0, 0..1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_streams::Attribute;
    use tags::material::StageTexture;

    fn stage(source: TextureSource) -> Option<StageTexture> {
        Some(StageTexture { source, size: None, stage_size: None })
    }

    #[test]
    fn engine_textures_19_and_20_are_targets_0x15_and_0x16() {
        assert_eq!(sampled_target(REFLECTION_TEXTURE), Some(0x15));
        assert_eq!(sampled_target(REFRACTION_TEXTURE), Some(0x16));
        for id in [0, 1, 3, 5, 14, 17, 21, 24] {
            assert_eq!(sampled_target(id), None, "{id}");
        }
    }

    #[test]
    fn the_grid_quads_keep_corners_and_diagonal() {
        let quads = [[3, 6, 7, 4], [0, 0, 1, 2]];
        assert_eq!(quad_triangles(&quads), [3, 6, 7, 3, 7, 4, 0, 0, 1, 0, 1, 2]);
        assert_eq!(vertex_count(&quads), 8);
        assert_eq!(vertex_count(&[]), 0);
    }

    #[test]
    fn a_stream_stride_counts_attributes_and_skips() {
        let stream = [
            StreamElement::Attribute { semantic: 0, format: Attribute::Float(3) },
            StreamElement::Skip(4),
            StreamElement::Attribute { semantic: 5, format: Attribute::Float(2) },
        ];
        assert_eq!(stride(&stream), 12 + 4 + 8);
    }

    #[test]
    fn the_grid_gathers_the_declared_registers_per_vertex() {
        let stream = [StreamElement::Skip(4), StreamElement::Attribute { semantic: 0, format: Attribute::Float(1) }];
        let decl = declaration(&[0], &[stream.to_vec()]);
        let bytes: Vec<u8> = (0u8..24).collect();
        let g = grid_of(&decl, stride(&stream), &bytes, &[[0, 1, 2, 2]], 7).unwrap();
        assert_eq!((g.key, g.vertices.len()), (7, 3));
        assert_eq!(&g.vertices[1][..4], &[12, 13, 14, 15]);
        assert_eq!(g.indices, [0, 1, 2, 0, 2, 2]);
        assert!(grid_of(&decl, stride(&stream), &bytes[..20], &[[0, 1, 2, 2]], 7).is_err(), "past the stream");
    }

    #[test]
    fn an_unsupplied_stage_is_named() {
        let mut imp = crate::engine_states::tests::implementation(Vec::new(), 4);
        imp.textures[2] = stage(TextureSource::Engine(5));
        let err = supplied(&imp, None).unwrap_err();
        assert!(err.contains("stage 2") && err.contains("engine texture 5"), "{err}");
        imp.textures[2] = stage(TextureSource::Engine(14));
        assert!(supplied(&imp, None).is_err(), "no rasterizer bitmap");
        let (out, targets) = supplied(&imp, Some(map::TagId(9))).unwrap();
        assert_eq!(targets, [None; 4]);
        assert!(matches!(out.textures[2].as_ref().unwrap().source, TextureSource::Bitmap { bitmap: map::TagId(9), .. }));
    }

    #[test]
    fn the_reflection_and_refraction_stages_keep_their_targets() {
        let mut imp = crate::engine_states::tests::implementation(Vec::new(), 4);
        imp.textures[1] = stage(TextureSource::Engine(REFRACTION_TEXTURE));
        imp.textures[3] = stage(TextureSource::Engine(REFLECTION_TEXTURE));
        let (out, targets) = supplied(&imp, None).unwrap();
        assert_eq!(targets, [None, Some(0x16), None, Some(0x15)]);
        for n in [1, 3] {
            assert_eq!(out.textures[n].as_ref().unwrap().source, TextureSource::Engine(VIEW_TEXTURE));
        }
        assert_eq!(sampled_targets(&targets), [0x16, 0x15]);
        assert_eq!(sampled_targets(&[Some(0x15), None, Some(0x15), None]), [0x15]);
    }

    #[test]
    fn a_material_without_layer_three_is_named() {
        let material = Material { shader: map::TagId(1), template: map::TagId(2), inputs: Default::default(), layers: Vec::new() };
        let src = OceanSource { material: &material, properties: &[], rasterizer: None, microcode: &[], semantics: &[] };
        assert!(compile(&src, &[]).unwrap_err().contains("no layer 3"));
    }
}
