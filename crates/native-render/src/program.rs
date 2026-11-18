//! A world material drawn with its own programs: the pass's vertex program
//! and pixel shader definition through `shaders`, the instance constants
//! where the game puts them, and the view's constants from the device
//! each frame (docs/graphics/native-renderer.md, "What a world pass reads").
//! [`compile`] is pure (WGSL and uniform values); [`ProgramSet`] holds the
//! GPU side for [`crate::gpu::GpuWorld`].

use std::collections::HashMap;

use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};
use shaders::vsh::{self, VertexOptions, VertexUniforms};
use tags::material::Register;
use tags::shader::StageStates;

use crate::gpu::{Image, texture};
use crate::look::{PassLook, Stage};
use crate::world::{LightmapKey, World, WorldVertex};

/// The per-view vertex constants the device gives the pass
/// (`d3d::VIEW_CONSTANTS`).
pub const VIEW_CONSTANTS: usize = 18;

/// The texture-coordinate decompression register the world programs read
/// (`mad r, v1, c[172].xy, c[172].zw`); the BSP's is the identity
/// (`0x0001cf29`).
pub const TEXCOORD_DECOMPRESSION: usize = 172;

/// The model rows the structure draw uploads for a placement
/// (`0x0004d830`, hardware numbering).
pub const MODEL_ROWS: usize = 50;

/// The vertex registers a [`WorldVertex`] supplies, with the field each
/// comes from: `v0` position, `v1` texture coordinate, `v2..v4` the tangent
/// frame (normal, binormal, tangent: the program dots the lighting vector
/// with `v4`, `v3`, `v2` into tangent space), `v5` the lighting vector,
/// `v6` the lightmap coordinate.
pub const INPUTS: [(u32, u64, wgpu::VertexFormat); 7] = [
    (0, std::mem::offset_of!(WorldVertex, position) as u64, wgpu::VertexFormat::Float32x3),
    (1, std::mem::offset_of!(WorldVertex, uv) as u64, wgpu::VertexFormat::Float32x2),
    (2, std::mem::offset_of!(WorldVertex, normal) as u64, wgpu::VertexFormat::Float32x3),
    (3, std::mem::offset_of!(WorldVertex, binormal) as u64, wgpu::VertexFormat::Float32x3),
    (4, std::mem::offset_of!(WorldVertex, tangent) as u64, wgpu::VertexFormat::Float32x3),
    (5, std::mem::offset_of!(WorldVertex, lighting) as u64, wgpu::VertexFormat::Float32x3),
    (6, std::mem::offset_of!(WorldVertex, lightmap_uv) as u64, wgpu::VertexFormat::Float32x2),
];

/// Bit `n` for each supplied `v<n>`.
pub const INPUT_MASK: u16 = 0x7f;

/// A material's compiled pass.
#[derive(Clone, Debug)]
pub struct MaterialProgram {
    /// The linked module (`vs_main`, `fs_main`).
    pub wgsl: String,
    /// What each stage samples, and its sampler.
    pub stages: [Stage; 4],
    pub samplers: [Option<StageStates>; 4],
    /// The texture each stage's fetch expects (`None`: not sampled).
    pub dims: [Option<TextureDim>; 4],
    /// Per stage, the channels (R, G, B, A) read as signed bytes: the
    /// image is stored biased for them ([`bias_signed`]).
    pub signed: [[bool; 4]; 4],
    /// The vertex uniforms but for the view's constants, surface and depth.
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
}

/// Translates `look`'s pass with its vertex program `microcode` (the
/// world-space classification, [`crate::look::world_program`]), or says
/// why it stays on the D3D path.
pub fn compile(look: &PassLook, microcode: &[u8]) -> Result<MaterialProgram, String> {
    if look.engine_pixel_constants {
        // Their inputs are not named yet (spas +0x38 through 0x00019ca0).
        return Err("engine-driven pixel constants".into());
    }
    let program = vsh::Program::parse(microcode).map_err(|e| format!("vertex program: {e:?}"))?;
    let unsupplied = program.inputs_read() & !INPUT_MASK;
    if unsupplied != 0 {
        return Err(format!("vertex program reads registers we do not supply ({unsupplied:#06x})"));
    }
    let vs = vsh::translate(&program, &VertexOptions { provided_inputs: INPUT_MASK, ..Default::default() })
        .map_err(|e| format!("vertex program: {e:?}"))?;
    let def = PixelShaderDef::from_bytes(&look.pixel_shader).map_err(|e| format!("pixel shader: {e:?}"))?;
    let samplers: [Option<StageStates>; 4] = std::array::from_fn(|n| look.stage_states.get(n).copied());
    let mut options = PixelOptions {
        texture_dims: look.textures.map(|s| (s != Stage::Empty).then_some(TextureDim::D2)),
        // The game's world is W-buffered (docs/graphics/native-renderer.md).
        w_buffer: true,
        ..Default::default()
    };
    let mut signed = [[false; 4]; 4];
    for (n, s) in samplers.iter().enumerate() {
        let Some(s) = s else { continue };
        let sign = psh::bits::color_sign(s.color_sign);
        if sign != 0 {
            if !matches!(look.textures[n], Stage::Bitmap { .. }) {
                // Only a stage's own bitmap is stored biased here.
                return Err(format!("stage {n} reads signed channels of the lightmap"));
            }
            options.color_sign[n] = sign;
            signed[n] = signed_channels(s.color_sign);
        }
        options.alpha_kill[n] = s.alpha_kill != 0;
    }
    let ps = psh::translate(&def, &options);
    if !ps.warnings.is_empty() {
        return Err(format!("pixel shader: {}", ps.warnings.join("; ")));
    }
    for (n, d) in ps.textures.iter().enumerate() {
        if d.is_some() && look.textures[n] == Stage::Empty {
            return Err(format!("stage {n} is sampled with nothing bound"));
        }
        if d.is_some_and(|d| d != TextureDim::D2) {
            return Err(format!("stage {n} is not a 2D fetch"));
        }
    }
    let wgsl = shaders::link(&vs, &ps).map_err(|e| e.to_string())?;
    let mut vertex = VertexUniforms::default();
    vertex.constants[TEXCOORD_DECOMPRESSION] = [1.0, 1.0, 0.0, 0.0];
    // The vertices are already placed: the model rows are the identity
    // (clusters never upload them; a program that reads them gets these).
    vertex.constants[MODEL_ROWS..MODEL_ROWS + 3]
        .copy_from_slice(&[[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
    for r in &look.vertex_constants {
        put(&mut vertex.constants, r);
    }
    for r in &look.vertex_defaults {
        put(&mut vertex.defaults, r);
    }
    let (c0, c1) = def.embedded_constants();
    let pixel = PixelUniforms { c0, c1, ..Default::default() };
    Ok(MaterialProgram { wgsl, stages: look.textures, samplers, dims: ps.textures, signed, vertex, pixel })
}

/// The channels (R, G, B, A) a `D3DTSS_COLORSIGN` value reads as signed
/// (`D3DTSIGN_ASIGNED` is bit 28, `RSIGNED` 29, `GSIGNED` 30, `BSIGNED` 31;
/// as `gfx::texture::DecodeOptions::with_color_sign`).
pub fn signed_channels(color_sign: u32) -> [bool; 4] {
    let bit = |n: u32| color_sign >> n & 1 != 0;
    [bit(29), bit(30), bit(31), bit(28)]
}

/// Stores the signed channels of an RGBA8 image as `s + 128`, so that
/// filtering interpolates signed values (`shaders::psh::PixelOptions::color_sign`).
pub fn bias_signed(image: &mut Image, signed: [bool; 4]) {
    let flip: [u8; 4] = signed.map(|s| if s { 0x80 } else { 0 });
    if flip == [0; 4] {
        return;
    }
    for level in std::iter::once(&mut image.rgba).chain(image.mips.iter_mut()) {
        for px in level.chunks_exact_mut(4) {
            for (b, f) in px.iter_mut().zip(flip) {
                *b ^= f;
            }
        }
    }
}

/// Writes the components `r` writes into its register of `regs`.
fn put(regs: &mut [[f32; 4]], r: &Register) {
    let Some(reg) = regs.get_mut(usize::from(r.index)) else { return };
    for k in 0..4 {
        if r.written & (1 << k) != 0 {
            reg[k] = r.value[k];
        }
    }
}

/// One frame's view of the target: the constants the device gave the pass
/// and the target's sizes and depth scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewInputs {
    pub constants: [[f32; 4]; VIEW_CONSTANTS],
    /// Guest pixels and this copy's texels.
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
    pub z_scale: f32,
}

/// `base` with the view's constants, surface and depth for one frame.
pub fn frame_vertex(base: &VertexUniforms, view: &ViewInputs) -> VertexUniforms {
    let mut u = base.clone();
    u.constants[..VIEW_CONSTANTS].copy_from_slice(&view.constants);
    u.surface = [view.guest_size.0 as f32, view.guest_size.1 as f32];
    u.z_max = view.z_scale;
    // S: the W-buffer's clip range is the whole depth range.
    u.w_clip = Some([0.0, view.z_scale]);
    u.w_clamp = false;
    let s = view.size.0 as f32 / view.guest_size.0.max(1) as f32;
    // As the D3D path shifts a scaled target (gfx::renderer::window_shift).
    u.shift = if s > 1.0 { (2.0 * s - 1.0) / (4.0 * s) } else { 0.0 };
    u
}

/// `base` with the frame's depth scale.
pub fn frame_pixel(base: &PixelUniforms, view: &ViewInputs) -> PixelUniforms {
    PixelUniforms {
        depth: [view.z_scale, 0.0, 0.0, 1.0],
        depth_range: [0.0, view.z_scale, 0.0, 0.0],
        ..*base
    }
}

/// The wgpu sampler for a stage's states (XDK values: address 1 wrap, 2
/// mirror, 3 clamp, 4 border, 5 mirror once; filter 1 point, else linear;
/// mip filter 0 none, 1 point, 2 linear).
pub fn sampler_descriptor(s: Option<&StageStates>) -> wgpu::SamplerDescriptor<'static> {
    let address = |a: u8| match a {
        2 | 5 => wgpu::AddressMode::MirrorRepeat,
        3 | 4 => wgpu::AddressMode::ClampToEdge,
        _ => wgpu::AddressMode::Repeat,
    };
    let filter = |f: u8| if f == 1 { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
    let Some(s) = s else {
        return wgpu::SamplerDescriptor {
            label: Some("native world program"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        };
    };
    wgpu::SamplerDescriptor {
        label: Some("native world program"),
        address_mode_u: address(s.address_u),
        address_mode_v: address(s.address_v),
        address_mode_w: address(s.address_w),
        mag_filter: filter(s.mag_filter),
        min_filter: filter(s.min_filter),
        mipmap_filter: match s.mip_filter {
            2 => wgpu::MipmapFilterMode::Linear,
            _ => wgpu::MipmapFilterMode::Nearest,
        },
        lod_max_clamp: if s.mip_filter == 0 { 0.0 } else { 32.0 },
        ..Default::default()
    }
}

/// One material's GPU side.
struct Gpu {
    program: MaterialProgram,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    textures_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    pixel_buffer: wgpu::Buffer,
    uniforms: wgpu::BindGroup,
    samplers: [wgpu::Sampler; 4],
    stage_views: [Option<wgpu::TextureView>; 4],
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), wgpu::RenderPipeline>,
}

/// The materials drawn with their own programs, and each batch's texture
/// group (its lightmap page bound where the pass samples the lightmap).
pub struct ProgramSet {
    materials: HashMap<u16, Gpu>,
    /// Per batch of the world: the texture group, when the batch's
    /// material has a program.
    batch_groups: Vec<Option<wgpu::BindGroup>>,
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl ProgramSet {
    /// Uploads `programs` (by material) for `world`. `stage_image` decodes
    /// a stage's bitmap image; `lightmap` gives a batch's page. A material
    /// whose bitmaps do not decode is left out (it keeps stage 1's program).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        programs: HashMap<u16, MaterialProgram>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        let mut materials = HashMap::new();
        'material: for (material, program) in programs {
            let mut stage_views: [Option<wgpu::TextureView>; 4] = Default::default();
            for (n, s) in program.stages.iter().enumerate() {
                if let Stage::Bitmap { .. } = s
                    && program.dims[n].is_some()
                {
                    let Some(mut image) = stage_image(*s) else { continue 'material };
                    bias_signed(&mut image, program.signed[n]);
                    stage_views[n] = Some(texture(device, queue, &image, "native world stage"));
                }
            }
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native world program"),
                source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
            });
            let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("native world program uniforms"),
                entries: &[
                    uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                    uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
                ],
            });
            let mut entries = Vec::new();
            for (n, d) in program.dims.iter().enumerate() {
                if d.is_none() {
                    continue;
                }
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
                label: Some("native world program textures"),
                entries: &entries,
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native world program"),
                bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
                immediate_size: 0,
            });
            let buffer = |label, size: usize| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: size as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            let vertex_buffer = buffer("native world program vertex", VertexUniforms::SIZE);
            let pixel_buffer = buffer("native world program pixel", PixelUniforms::SIZE);
            let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native world program uniforms"),
                layout: &uniforms_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: vertex_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: pixel_buffer.as_entire_binding() },
                ],
            });
            let samplers = std::array::from_fn(|n| device.create_sampler(&sampler_descriptor(program.samplers[n].as_ref())));
            materials.insert(
                material,
                Gpu {
                    program,
                    module,
                    layout,
                    textures_layout,
                    vertex_buffer,
                    pixel_buffer,
                    uniforms,
                    samplers,
                    stage_views,
                    pipelines: HashMap::new(),
                },
            );
        }
        let mut pages: HashMap<LightmapKey, Option<wgpu::TextureView>> = HashMap::new();
        let batch_groups = world
            .batches
            .iter()
            .map(|b| {
                let g = materials.get(&b.material)?;
                let page = match b.lightmap {
                    Some(k) => pages
                        .entry(k)
                        .or_insert_with(|| lightmap(k).map(|i| texture(device, queue, &i, "native world lightmap")))
                        .clone(),
                    None => None,
                };
                let mut views = Vec::new();
                for (n, d) in g.program.dims.iter().enumerate() {
                    if d.is_none() {
                        continue;
                    }
                    let view = match g.program.stages[n] {
                        Stage::Lightmap => page.clone()?,
                        _ => g.stage_views[n].clone()?,
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
                        resource: wgpu::BindingResource::Sampler(&g.samplers[*n]),
                    });
                }
                Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("native world program textures"),
                    layout: &g.textures_layout,
                    entries: &entries,
                }))
            })
            .collect();
        Self { materials, batch_groups }
    }

    /// How many materials draw with their own programs.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// The frame's uniforms for every material.
    pub fn prepare(&self, queue: &wgpu::Queue, view: &ViewInputs) {
        for g in self.materials.values() {
            queue.write_buffer(&g.vertex_buffer, 0, &frame_vertex(&g.program.vertex, view).to_bytes());
            queue.write_buffer(&g.pixel_buffer, 0, &frame_pixel(&g.program.pixel, view).to_bytes());
        }
    }

    /// Makes the pipelines for these target formats (before the pass).
    pub fn pipelines(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        let attributes: Vec<wgpu::VertexAttribute> = INPUTS
            .iter()
            .map(|&(location, offset, format)| wgpu::VertexAttribute { format, offset, shader_location: location })
            .collect();
        for g in self.materials.values_mut() {
            let (module, layout) = (&g.module, &g.layout);
            g.pipelines.entry((color, depth)).or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("native world program"),
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
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: depth,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::LessEqual),
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
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            });
        }
    }

    /// Batch `index`'s pipeline and groups, when its material has a
    /// program and the pipelines for these formats are made.
    pub fn batch(
        &self, index: usize, material: u16, color: wgpu::TextureFormat, depth: wgpu::TextureFormat,
    ) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup, &wgpu::BindGroup)> {
        let g = self.materials.get(&material)?;
        let textures = self.batch_groups.get(index)?.as_ref()?;
        Some((g.pipelines.get(&(color, depth))?, &g.uniforms, textures))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::material::Origin;

    #[test]
    fn the_inputs_follow_the_vertex_fields_and_registers() {
        let regs: Vec<u32> = INPUTS.iter().map(|i| i.0).collect();
        assert_eq!(regs, (0..7).collect::<Vec<_>>());
        assert_eq!(INPUT_MASK, INPUTS.iter().fold(0, |m, i| m | (1 << i.0)));
        assert_eq!(INPUTS[6].1, 20, "v6 is the lightmap coordinate, after position and uv");
        assert_eq!(INPUTS[5].1, 64, "v5 is the lighting vector, the last field");
    }

    #[test]
    fn instance_constants_write_only_their_components() {
        let mut regs = [[9.0f32; 4]; 4];
        put(&mut regs, &Register { index: 2, value: [1.0, 2.0, 3.0, 4.0], written: 0b0101, origin: Origin::Instance });
        assert_eq!(regs[2], [1.0, 9.0, 3.0, 9.0]);
        put(&mut regs, &Register { index: 7, value: [0.0; 4], written: 0xf, origin: Origin::Instance });
        assert_eq!(regs[3], [9.0; 4], "out of range: ignored");
    }

    #[test]
    fn a_frame_puts_the_view_constants_first_and_scales_the_shift() {
        let mut base = VertexUniforms::default();
        base.constants[18] = [10.0, 10.0, 0.0, 0.0];
        let mut constants = [[0.0; 4]; VIEW_CONSTANTS];
        constants[0] = [1.0, 2.0, 3.0, 4.0];
        let view = ViewInputs { constants, guest_size: (640, 480), size: (1280, 960), z_scale: 16_777_215.0 };
        let u = frame_vertex(&base, &view);
        assert_eq!(u.constants[0], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(u.constants[18], [10.0, 10.0, 0.0, 0.0], "instance constants kept");
        assert_eq!(u.surface, [640.0, 480.0]);
        assert_eq!(u.w_clip, Some([0.0, 16_777_215.0]));
        assert_eq!(u.shift, 3.0 / 8.0, "(2s - 1) / 4s at 2x");
        assert_eq!(frame_vertex(&base, &ViewInputs { size: (640, 480), ..view }).shift, 0.0);
        let p = frame_pixel(&PixelUniforms::default(), &ViewInputs { z_scale: 65_535.0, ..view });
        assert_eq!((p.depth[0], p.depth_range[1]), (65_535.0, 65_535.0));
    }

    #[test]
    fn engine_pixel_constants_keep_the_material_on_the_d3d_path() {
        let look = PassLook { engine_pixel_constants: true, ..plain_look() };
        assert_eq!(compile(&look, &plain_program()).unwrap_err(), "engine-driven pixel constants");
        assert!(compile(&plain_look(), b"").unwrap_err().starts_with("vertex program"), "no microcode");
    }

    fn plain_look() -> PassLook {
        PassLook {
            pass: map::TagId(0xe000_0001),
            implementation: 0,
            vertex_shader: map::TagId(0xe000_0002),
            textures: [Stage::Empty; 4],
            stage_states: Vec::new(),
            pixel_shader: shaders::PixelShaderDef::default().to_bytes(),
            vertex_constants: vec![Register { index: 19, value: [2.0, 3.0, 0.0, 0.0], written: 0b0011, origin: Origin::Instance }],
            vertex_defaults: Vec::new(),
            engine_pixel_constants: false,
        }
    }

    /// `dp4 oPos, v0, c[0..3]; mov oT0, v1; mov oT1, v6`.
    fn plain_program() -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let row = |c, mask| Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(false, OutReg::Pos, mask);
        program(&[
            row(0, 8),
            row(1, 4),
            row(2, 2),
            row(3, 1),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(6)), None, None).to_out(false, OutReg::T(1), 0xf),
        ])
    }

    #[test]
    fn a_plain_pass_links_into_a_valid_module_with_its_constants() {
        let p = compile(&plain_look(), &plain_program()).expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert_eq!(p.vertex.constants[19], [2.0, 3.0, 0.0, 0.0]);
        assert_eq!(p.vertex.constants[TEXCOORD_DECOMPRESSION], [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.vertex.constants[MODEL_ROWS + 1], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.dims, [None; 4], "samples nothing");
        assert_eq!(p.signed, [[false; 4]; 4]);
    }

    #[test]
    fn a_program_reading_an_unsupplied_register_stays_on_the_d3d_path() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let reads_v9 = program(&[Inst::mac(MacOp::Mov, Some(O::v(9)), None, None).to_out(false, OutReg::Pos, 0xf)]);
        let e = compile(&plain_look(), &reads_v9).unwrap_err();
        assert!(e.contains("do not supply"), "{e}");
    }

    #[test]
    fn signed_channels_are_stored_biased() {
        assert_eq!(signed_channels(1 << 29 | 1 << 28), [true, false, false, true]);
        let mut i = Image::from_levels(vec![(2, 1, vec![0x7f, 0x01, 0xff, 0x80, 0, 0, 0, 0]), (1, 1, vec![0x81, 0, 0, 0])]).unwrap();
        bias_signed(&mut i, [true, false, false, true]);
        assert_eq!(i.rgba, vec![0xff, 0x01, 0xff, 0x00, 0x80, 0, 0, 0x80]);
        assert_eq!(i.mips, vec![vec![0x01, 0, 0, 0x80]], "every level");
    }

    #[test]
    fn samplers_follow_the_stage_states() {
        let mut s = StageStates {
            address_u: 1,
            address_v: 3,
            address_w: 2,
            mag_filter: 1,
            min_filter: 2,
            mip_filter: 0,
            mip_lod_bias: 0,
            max_mip_level: 0,
            max_anisotropy: 0,
            color_key_op: 0,
            color_sign: 0,
            alpha_kill: 0,
            border_color: 0,
            color_key_color: 0,
            unread: [0; 2],
        };
        let d = sampler_descriptor(Some(&s));
        assert_eq!(
            (d.address_mode_u, d.address_mode_v, d.address_mode_w),
            (wgpu::AddressMode::Repeat, wgpu::AddressMode::ClampToEdge, wgpu::AddressMode::MirrorRepeat)
        );
        assert_eq!((d.mag_filter, d.min_filter), (wgpu::FilterMode::Nearest, wgpu::FilterMode::Linear));
        assert_eq!(d.lod_max_clamp, 0.0, "no mip filter: level 0 only");
        s.mip_filter = 2;
        assert_eq!(sampler_descriptor(Some(&s)).mipmap_filter, wgpu::MipmapFilterMode::Linear);
    }
}
