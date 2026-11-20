//! GPU side of one resolved dynamic decal draw. The actual vertex program,
//! constants, combiner, stream bytes, stage views and clip rectangles come
//! from the ordered draw-admission snapshot. Two-copy recording uses fresh
//! uniform buffers on each copy and returns a real range count.

use std::collections::HashMap;
use std::sync::Arc;

use gfx::compile::{Pending, PendingPipeline};
use gfx::pipeline::{PipelineKey, ShaderId};
use gfx::{BoundGroup, RecordedDraw, ScissorRect};

use gfx::state::{
    BlendFactor, BlendOp, CompareFunc, CullMode, DepthBufferMode, RenderState, TextureAddress,
    TextureStageState,
};
use shaders::psh::{self, AlphaFunc, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};
use shaders::vsh::{self, VertexOptions, VertexUniforms};

use crate::decal::{Mode, VERTEX_SIZE, Vertex, quad_indices};
use crate::program;

/// Raw per-stage state outside `gfx`'s sampler model that the combiner needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageExtras {
    pub color_key_op: u32,
    pub color_sign: u32,
    pub alpha_kill: u32,
    pub texture_transform_flags: u32,
    pub bump_env_mat: [f32; 4],
    pub bump_env_lscale: f32,
    pub bump_env_loffset: f32,
}

/// A resolved writable area in guest target pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ClipRect {
    fn scaled(self, scale: d3d::scaled::RenderScale) -> ScissorRect {
        let [x, y, width, height] = scale.rect(self.x, self.y, self.width, self.height);
        ScissorRect { x, y, width, height }
    }
}

/// The non-geometry half of an immutable resolved-draw snapshot.
#[derive(Clone, Debug)]
pub struct ShaderSnapshot {
    pub render: RenderState,
    pub stages: [TextureStageState; 4],
    pub extras: [StageExtras; 4],
    pub combiner: PixelShaderDef,
    pub microcode: Vec<u8>,
    pub provided_inputs: u16,
    /// The draw's complete 192 constants and 16 attribute defaults.
    pub vertex: VertexUniforms,
    /// Whether the guest depth format truncates stored W.
    pub integer_depth: bool,
    /// Linear textures are fetched in texels of the bound image.
    pub tex_scale: Option<[f32; 4]>,
}

/// One resolved draw prepared for native replacement.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub mode: Mode,
    pub vertices: Vec<Vertex>,
    pub shader: ShaderSnapshot,
    /// Empty means the guest clipped the whole draw away.
    pub clips: Vec<ClipRect>,
}

/// The target contract boot fills from `d3d::ExternalTarget`.
pub struct DecalTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub color: &'a wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    pub depth: &'a wgpu::TextureView,
    pub depth_format: wgpu::TextureFormat,
    pub viewport: [f32; 4],
    pub z_scale: f32,
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
    pub stages: [Option<&'a wgpu::TextureView>; 4],
}

/// The W-buffer clip supplied by the same ordered boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalClip {
    pub range: [f32; 2],
    pub clamp: bool,
    pub w_buffered: Option<bool>,
}

fn expected_blend(mode: Mode) -> Option<(BlendFactor, BlendFactor, BlendOp)> {
    Some(match mode {
        Mode::Alpha0 | Mode::Alpha1 | Mode::Alpha5 => {
            (BlendFactor::One, BlendFactor::InvSrcAlpha, BlendOp::Add)
        }
        Mode::Multiply => (BlendFactor::DestColor, BlendFactor::SrcColor, BlendOp::Add),
        Mode::Darken => (BlendFactor::DestColor, BlendFactor::Zero, BlendOp::Add),
        Mode::Maximum => (BlendFactor::One, BlendFactor::One, BlendOp::Max),
        Mode::Opaque => return None,
    })
}

/// Checks the effective state captured after `0x00043a40`. In particular,
/// stencil is inherited and is admitted only when resolved disabled.
pub fn check_state(
    mode: Mode,
    rs: &RenderState,
    stages: &[TextureStageState; 4],
    extras: &[StageExtras; 4],
) -> Result<(), &'static str> {
    if rs.stencil.enable {
        return Err("stencil enabled");
    }
    if rs.alpha_test_enable || rs.color_write.xbox_value() != 0x0001_0101 {
        return Err("colour/alpha state");
    }
    if rs.fill_mode != gfx::state::FillMode::Solid
        || rs.back_fill_mode != gfx::state::FillMode::Solid
    {
        return Err("fill mode");
    }
    if stages.iter().any(|s| {
        s.address_u == TextureAddress::Border
            || s.address_v == TextureAddress::Border
            || s.address_w == TextureAddress::Border
    }) {
        return Err("border sampler");
    }
    if extras
        .iter()
        .any(|e| e.color_key_op != 0 || e.color_sign != 0 || e.texture_transform_flags != 0)
    {
        return Err("texture stage state");
    }
    if mode.w_buffered() {
        if rs.depth_mode != DepthBufferMode::W
            || rs.depth_func != CompareFunc::Always
            || !rs.depth_write
            || !rs.solid_offset_enable
            || rs.depth_bias_slope_scale != 0.0
            || !rs.depth_bias.is_finite()
            || rs.cull_mode != CullMode::Ccw
        {
            return Err("W depth/bias");
        }
    } else if rs.depth_mode != DepthBufferMode::Disabled
        || rs.solid_offset_enable
        || rs.cull_mode != CullMode::None
    {
        return Err("opaque depth/bias");
    }
    match expected_blend(mode) {
        Some((src, dst, op))
            if !rs.blend_enable || (rs.src_blend, rs.dst_blend, rs.blend_op) != (src, dst, op) =>
        {
            return Err("blend state");
        }
        None if rs.blend_enable => return Err("blend state"),
        _ => {}
    }
    Ok(())
}

fn alpha_func(f: CompareFunc) -> AlphaFunc {
    match f {
        CompareFunc::Never => AlphaFunc::Never,
        CompareFunc::Less => AlphaFunc::Less,
        CompareFunc::Equal => AlphaFunc::Equal,
        CompareFunc::LessEqual => AlphaFunc::LessEqual,
        CompareFunc::Greater => AlphaFunc::Greater,
        CompareFunc::NotEqual => AlphaFunc::NotEqual,
        CompareFunc::GreaterEqual => AlphaFunc::GreaterEqual,
        CompareFunc::Always => AlphaFunc::Always,
    }
}

struct Compiled {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    uniforms_layout: wgpu::BindGroupLayout,
    textures_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex: VertexUniforms,
    pixel: PixelUniforms,
    primitive: wgpu::PrimitiveState,
    depth: wgpu::DepthStencilState,
    blend: Option<wgpu::BlendState>,
    blend_constant: wgpu::Color,
    color_writes: wgpu::ColorWrites,
    sampled: bool,
}

/// A GPU program plus the resolved draw's immutable geometry.
pub struct DecalGpu {
    mode: Mode,
    render: RenderState,
    integer_depth: bool,
    compiled: Compiled,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    clips: Vec<ClipRect>,
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), wgpu::RenderPipeline>,
}

fn check_combiner_state(def: &PixelShaderDef) -> Result<(), &'static str> {
    let decoded = def.decode();
    let Some(final_combiner) = decoded.final_combiner else {
        return Err("final combiner");
    };
    if decoded
        .stages
        .iter()
        .flat_map(|s| s.rgb.inputs.iter().chain(&s.alpha.inputs))
        .chain(&[
            final_combiner.a,
            final_combiner.b,
            final_combiner.c,
            final_combiner.d,
            final_combiner.e,
            final_combiner.f,
            final_combiner.g,
        ])
        .any(|i| i.reg == psh::Reg::Fog)
    {
        return Err("fog state");
    }
    if decoded.texture_modes.iter().any(|m| {
        matches!(
            m,
            psh::TexMode::BumpEnvMap
                | psh::TexMode::BumpEnvMapLum
                | psh::TexMode::Brdf
                | psh::TexMode::DotRflctDiff
                | psh::TexMode::DotRflctSpec
                | psh::TexMode::DotRflctSpecConst
        )
    }) {
        return Err("eye/bump state");
    }
    Ok(())
}

/// The draw's pixel uniforms before the target and clip fill in the depth
/// terms: the combiner's embedded constants, alpha reference, LOD bias,
/// bump terms and the linear texture scale.
fn base_pixel(shader: &ShaderSnapshot) -> PixelUniforms {
    let def = &shader.combiner;
    let (c0, c1) = def.embedded_constants();
    let mut pixel = PixelUniforms {
        c0,
        c1,
        fog: psh::fog::Fog::OFF.uniform(),
        alpha_ref: [shader.render.alpha_ref as f32, 0.0, 0.0, 0.0],
        lod_bias: shader.stages.map(|s| {
            if s.mip_lod_bias.is_finite() {
                s.mip_lod_bias
            } else {
                0.0
            }
        }),
        ..Default::default()
    };
    for n in 0..4 {
        let e = &shader.extras[n];
        pixel.bump_mat[n] = e.bump_env_mat;
        pixel.bump_lum[n] = [e.bump_env_lscale, e.bump_env_loffset, 0.0, 0.0];
    }
    if let Some(scale) = shader.tex_scale {
        pixel.tex_scale[0] = scale;
    }
    pixel
}

/// The pixel translation options a decal's state and mode select.
fn pixel_options(mode: Mode, shader: &ShaderSnapshot) -> PixelOptions {
    PixelOptions {
        texture_dims: [Some(TextureDim::D2), None, None, None],
        alpha_kill: shader.extras.map(|e| e.alpha_kill & 4 != 0),
        alpha_test: shader
            .render
            .alpha_test_enable
            .then(|| alpha_func(shader.render.alpha_func)),
        w_buffer: mode.w_buffered(),
        flat_shading: shader.render.shade_mode == gfx::state::ShadeMode::Flat,
        ..Default::default()
    }
}

/// Validates the complete shader translation without creating GPU objects,
/// so admission refuses before it reserves anything.
pub fn check_program(mode: Mode, shader: &ShaderSnapshot) -> Result<(), &'static str> {
    check_state(mode, &shader.render, &shader.stages, &shader.extras)?;
    check_combiner_state(&shader.combiner)?;
    let program = vsh::Program::parse(&shader.microcode).map_err(|_| "vertex program")?;
    let supplied = (1 << 0) | (1 << 3);
    if shader.provided_inputs & !supplied != 0
        || program.inputs_read() & !shader.provided_inputs & !((1 << 10) | (1 << 11)) != 0
    {
        return Err("vertex inputs");
    }
    let vs = vsh::translate(
        &program,
        &VertexOptions {
            provided_inputs: shader.provided_inputs,
            flat_shading: shader.render.shade_mode == gfx::state::ShadeMode::Flat,
            ..Default::default()
        },
    )
    .map_err(|_| "vertex translation")?;
    let def = shader.combiner.clone();
    if !def.has_final_combiner() {
        return Err("final combiner");
    }
    let options = pixel_options(mode, shader);
    let ps = psh::translate(&def, &options);
    if !ps.warnings.is_empty()
        || ps
            .textures
            .iter()
            .enumerate()
            .any(|(n, d)| n != 0 && d.is_some())
    {
        return Err("pixel translation");
    }
    let wgsl = shaders::link(&vs, &ps).map_err(|_| "shader link")?;
    shaders::validate(&wgsl).map_err(|_| "shader validation")?;
    Ok(())
}

fn compile(
    device: &wgpu::Device,
    mode: Mode,
    shader: &ShaderSnapshot,
) -> Result<Compiled, &'static str> {
    check_program(mode, shader)?;
    let program = vsh::Program::parse(&shader.microcode).map_err(|_| "vertex program")?;
    let vs = vsh::translate(
        &program,
        &VertexOptions {
            provided_inputs: shader.provided_inputs,
            flat_shading: shader.render.shade_mode == gfx::state::ShadeMode::Flat,
            ..Default::default()
        },
    )
    .map_err(|_| "vertex translation")?;
    let def = shader.combiner.clone();
    let options = pixel_options(mode, shader);
    let ps = psh::translate(&def, &options);
    if !ps.warnings.is_empty()
        || ps
            .textures
            .iter()
            .enumerate()
            .any(|(n, d)| n != 0 && d.is_some())
    {
        return Err("pixel translation");
    }
    let wgsl = shaders::link(&vs, &ps).map_err(|_| "shader link")?;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native decal program"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let uniform = |binding| program::uniform_entry(binding, wgpu::ShaderStages::VERTEX_FRAGMENT);
    let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native decal uniforms"),
        entries: &[uniform(0), uniform(1)],
    });
    let sampled = ps.textures[0].is_some();
    let entries: Vec<_> = if sampled {
        vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]
    } else {
        Vec::new()
    };
    let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native decal textures"),
        entries: &entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native decal program"),
        bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
        immediate_size: 0,
    });
    let sampler = device.create_sampler(
        &gfx::translate::SamplerKey::from_stage(
            &shader.stages[0],
            gfx::translate::Capabilities::from_features(device.features()),
        )
        .descriptor(),
    );
    let pixel = base_pixel(shader);
    let primitive = gfx::translate::primitive_state(
        &shader.render,
        wgpu::PrimitiveTopology::TriangleList,
        gfx::translate::Capabilities::from_features(device.features()),
    );
    let mut depth = gfx::translate::depth_stencil_state(
        &shader.render,
        Some(wgpu::TextureFormat::Depth24PlusStencil8),
        wgpu::PrimitiveTopology::TriangleList,
    )
    .ok_or("depth state")?;
    if mode.w_buffered() {
        depth.bias = wgpu::DepthBiasState::default();
    }
    Ok(Compiled {
        module,
        layout,
        uniforms_layout,
        textures_layout,
        sampler,
        vertex: shader.vertex.clone(),
        pixel,
        primitive,
        depth,
        blend: gfx::translate::blend_state(&shader.render),
        blend_constant: gfx::translate::blend_constant(&shader.render),
        color_writes: gfx::translate::color_writes(shader.render.color_write),
        sampled,
    })
}

/// The vertex and pixel uniforms of a draw on a target of `size` texels
/// for a guest surface of `guest_size` (`z_scale` the depth format's full
/// scale): the surface, the half-texel shift under a render scale, and for
/// a W-buffered mode the clip and the depth terms.
fn draw_uniforms(
    mode: Mode,
    render: &RenderState,
    integer_depth: bool,
    vertex: &VertexUniforms,
    pixel: PixelUniforms,
    (guest_size, size, z_scale): ((u32, u32), (u32, u32), f32),
    clip: DecalClip,
) -> (VertexUniforms, PixelUniforms) {
    let mut vu = vertex.clone();
    vu.surface = [guest_size.0 as f32, guest_size.1 as f32];
    vu.z_max = z_scale;
    vu.shift = crate::program::target_shift(guest_size, size);
    vu.w_clip = mode.w_buffered().then_some(clip.range);
    vu.w_clamp = mode.w_buffered() && clip.clamp;
    let mut pu = pixel;
    if mode.w_buffered() {
        pu.depth = [
            z_scale,
            if render.solid_offset_enable {
                render.depth_bias
            } else {
                0.0
            },
            if render.solid_offset_enable {
                render.depth_bias_slope_scale
            } else {
                0.0
            },
            if integer_depth { 1.0 } else { 0.0 },
        ];
        pu.depth_range = [clip.range[0], clip.range[1], 0.0, 0.0];
    }
    (vu, pu)
}

impl DecalGpu {
    /// Translates and uploads a fully admitted resolved draw.
    pub fn new(device: &wgpu::Device, prepared: &Prepared) -> Result<Self, &'static str> {
        let compiled = compile(device, prepared.mode, &prepared.shader)?;
        let indices = quad_indices(prepared.vertices.len() as u32 / 4);
        use wgpu::util::DeviceExt;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native decal vertices"),
            contents: bytemuck::cast_slice(&prepared.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native decal indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Ok(Self {
            mode: prepared.mode,
            render: prepared.shader.render,
            integer_depth: prepared.shader.integer_depth,
            compiled,
            vertices,
            indices,
            clips: prepared.clips.clone(),
            pipelines: HashMap::new(),
        })
    }

    pub fn sampled(&self) -> bool {
        self.compiled.sampled
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let c = &self.compiled;
        self.pipelines.entry((color, depth)).or_insert_with(|| {
            let attributes = [
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Unorm8x4Bgra,
                    offset: 12,
                    shader_location: 3,
                },
            ];
            let mut ds = c.depth.clone();
            ds.format = depth;
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native decal program"),
                layout: Some(&c.layout),
                vertex: wgpu::VertexState {
                    module: &c.module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_SIZE as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: c.primitive,
                depth_stencil: Some(ds),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &c.module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color,
                        blend: c.blend,
                        write_mask: c.color_writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// Records this resolved draw on one target copy.
    pub fn encode(&mut self, t: &mut DecalTarget<'_>, clip: DecalClip) -> bool {
        if clip.w_buffered != Some(self.mode.w_buffered())
            || t.guest_size.0 == 0
            || t.guest_size.1 == 0
        {
            return false;
        }
        if gfx::translate::depth_stencil_state(
            &self.render,
            Some(t.depth_format),
            wgpu::PrimitiveTopology::TriangleList,
        )
        .is_none()
        {
            return false;
        }
        let Some(scale) = d3d::scaled::RenderScale::between(t.guest_size, t.size) else {
            return false;
        };
        let clips: Vec<_> = self.clips.iter().map(|c| c.scaled(scale)).collect();
        if clips.is_empty() {
            return true;
        }
        let (vu, pu) = draw_uniforms(
            self.mode,
            &self.render,
            self.integer_depth,
            &self.compiled.vertex,
            self.compiled.pixel,
            (t.guest_size, t.size, t.z_scale),
            clip,
        );
        let vertex_buffer = t.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native decal vertex uniforms"),
            size: VertexUniforms::SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pixel_buffer = t.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native decal pixel uniforms"),
            size: PixelUniforms::SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        t.queue.write_buffer(&vertex_buffer, 0, &vu.to_bytes());
        t.queue.write_buffer(&pixel_buffer, 0, &pu.to_bytes());
        let uniforms = t.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native decal draw uniforms"),
            layout: &self.compiled.uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: vertex_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pixel_buffer.as_entire_binding(),
                },
            ],
        });
        let textures = if self.compiled.sampled {
            let Some(view) = t.stages[0] else {
                return false;
            };
            t.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native decal draw textures"),
                layout: &self.compiled.textures_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.compiled.sampler),
                    },
                ],
            })
        } else {
            t.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native decal no textures"),
                layout: &self.compiled.textures_layout,
                entries: &[],
            })
        };
        let pipeline = self
            .pipeline(t.device, t.color_format, t.depth_format)
            .clone();
        let mut pass = t.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native decal draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: t.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: t.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: t
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
        pass.set_pipeline(&pipeline);
        pass.set_viewport(0.0, 0.0, t.size.0 as f32, t.size.1 as f32, 0.0, 1.0);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group(0, &uniforms, &[]);
        pass.set_bind_group(1, &textures, &[]);
        pass.set_blend_constant(self.compiled.blend_constant);
        let count = self.indices.size() as u32 / 4;
        for c in clips {
            pass.set_scissor_rect(c.x, c.y, c.width, c.height);
            pass.draw_indexed(0..count, 0, 0..1);
        }
        true
    }
}

/// Texture groups kept before the cache starts over, so views of retired
/// textures are not held for ever.
const GROUP_LIMIT: usize = 64;

/// What a program is made from; the combiner's constants are uniforms, not
/// part of the translation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProgramKey {
    microcode: Vec<u8>,
    provided_inputs: u16,
    combiner: PixelShaderDef,
    options: PixelOptions,
}

struct CachedProgram {
    id: ShaderId,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    textures: wgpu::BindGroupLayout,
    /// The group of a program that samples nothing.
    unsampled: Option<wgpu::BindGroup>,
    sampled: bool,
    discards: bool,
}

#[derive(Clone)]
struct CachedPipeline {
    described: PipelineKey,
    pending: Arc<PendingPipeline>,
}

/// What a cache made so far, for tests of a steady scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Created {
    pub programs: u64,
    pub pipelines: u64,
    pub texture_groups: u64,
    pub samplers: u64,
}

/// The target a decal's packets draw into: the attachments' formats and
/// sizes, the guest surface they stand for, the depth format's full scale
/// and the colour of stage 0's texture if the program samples one.
pub struct PacketTarget<'a> {
    pub color_format: wgpu::TextureFormat,
    pub depth_format: wgpu::TextureFormat,
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
    pub z_scale: f32,
    pub stage0: Option<&'a wgpu::TextureView>,
}

/// Programs, pipelines, samplers and texture groups of the decals' packets,
/// scoped to one device and kept across frames: a steady scene makes
/// nothing. Geometry and uniforms come from the submission's arena.
#[derive(Default)]
pub struct DecalCache {
    uniforms: Option<wgpu::BindGroupLayout>,
    programs: HashMap<ProgramKey, Result<Arc<CachedProgram>, &'static str>>,
    pipelines: HashMap<PipelineKey, CachedPipeline>,
    samplers: HashMap<gfx::translate::SamplerKey, wgpu::Sampler>,
    groups: HashMap<(wgpu::BindGroupLayout, wgpu::TextureView, wgpu::Sampler), wgpu::BindGroup>,
    created: Created,
}

impl DecalCache {
    pub fn created(&self) -> Created {
        self.created
    }

    fn program(
        &mut self,
        device: &wgpu::Device,
        key: ProgramKey,
    ) -> Result<Arc<CachedProgram>, &'static str> {
        if let Some(found) = self.programs.get(&key) {
            return found.clone();
        }
        let uniforms = self
            .uniforms
            .get_or_insert_with(|| crate::light_gpu::uniforms_layout(device))
            .clone();
        self.created.programs += 1;
        let made = build_program(device, &key, &uniforms, ShaderId(u32::MAX - 2 - self.created.programs as u32))
            .map(Arc::new);
        self.programs.insert(key, made.clone());
        made
    }

    /// One packet per clip rectangle for the D3D pass open on the
    /// target's attachments, with what `DecalGpu::encode` records: the
    /// whole surface as viewport, each clip as scissor, the draw's blend
    /// constant, no stencil. The flag is whether the pixel program
    /// discards. `Ok(None)` when the whole draw is clipped away (the caller
    /// takes the isolated recording, which then records no draw).
    pub fn draws(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        prepared: &Prepared,
        clip: DecalClip,
        t: &PacketTarget<'_>,
    ) -> Result<Option<(Vec<RecordedDraw>, bool)>, &'static str> {
        let mode = prepared.mode;
        let shader = &prepared.shader;
        if clip.w_buffered != Some(mode.w_buffered()) || t.guest_size.0 == 0 || t.guest_size.1 == 0 {
            return Err("issue snapshot");
        }
        let caps = gfx::translate::Capabilities::from_features(device.features());
        let scale = d3d::scaled::RenderScale::between(t.guest_size, t.size).ok_or("target scale")?;
        if prepared.clips.is_empty() {
            return Ok(None);
        }
        let mut depth = gfx::translate::depth_stencil_state(
            &shader.render,
            Some(wgpu::TextureFormat::Depth24PlusStencil8),
            wgpu::PrimitiveTopology::TriangleList,
        )
        .ok_or("depth state")?;
        if gfx::translate::depth_stencil_state(
            &shader.render,
            Some(t.depth_format),
            wgpu::PrimitiveTopology::TriangleList,
        )
        .is_none()
        {
            return Err("depth state");
        }
        if mode.w_buffered() {
            depth.bias = wgpu::DepthBiasState::default();
        }
        depth.format = t.depth_format;
        let mut combiner = shader.combiner.clone();
        combiner.constant0 = [0; 8];
        combiner.constant1 = [0; 8];
        combiner.final_constant0 = 0;
        combiner.final_constant1 = 0;
        let program = self.program(
            device,
            ProgramKey {
                microcode: shader.microcode.clone(),
                provided_inputs: shader.provided_inputs,
                combiner,
                options: pixel_options(mode, shader),
            },
        )?;
        let color_target = wgpu::ColorTargetState {
            format: t.color_format,
            blend: gfx::translate::blend_state(&shader.render),
            write_mask: gfx::translate::color_writes(shader.render.color_write),
        };
        let described = PipelineKey {
            shader: program.id,
            vertex_buffers: Vec::new(),
            primitive: gfx::translate::primitive_state(&shader.render, wgpu::PrimitiveTopology::TriangleList, caps),
            depth_stencil: Some(depth),
            color_target,
            samples: 1,
        };
        let pipeline = match self.pipelines.get(&described) {
            Some(found) => found.clone(),
            None => {
                let made = decal_pipeline(device, &program, &described).ok_or("pipeline")?;
                self.created.pipelines += 1;
                self.pipelines.insert(described.clone(), made.clone());
                made
            }
        };
        let textures = if program.sampled {
            let view = t.stage0.ok_or("texture view")?;
            let key = gfx::translate::SamplerKey::from_stage(&shader.stages[0], caps);
            let created = &mut self.created;
            let sampler = self
                .samplers
                .entry(key)
                .or_insert_with(|| {
                    created.samplers += 1;
                    device.create_sampler(&key.descriptor())
                })
                .clone();
            let group_key = (program.textures.clone(), view.clone(), sampler);
            if let Some(group) = self.groups.get(&group_key) {
                gfx::churn::native_cache(true);
                group.clone()
            } else {
                gfx::churn::native_cache(false);
                let group = gfx::churn::create_bind_group(
                    device,
                    &wgpu::BindGroupDescriptor {
                        label: Some("native decal textures"),
                        layout: &program.textures,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&group_key.1),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&group_key.2),
                            },
                        ],
                    },
                );
                if self.groups.len() >= GROUP_LIMIT {
                    self.groups.clear();
                }
                self.created.texture_groups += 1;
                self.groups.insert(group_key, group.clone());
                group
            }
        } else {
            program.unsampled.clone().ok_or("texture group")?
        };
        let (vertex, pixel) = draw_uniforms(
            mode,
            &shader.render,
            shader.integer_depth,
            &shader.vertex,
            base_pixel(shader),
            (t.guest_size, t.size, t.z_scale),
            clip,
        );
        let layout = self.uniforms.as_ref().ok_or("uniforms layout")?;
        let run = uploads
            .uniform_run(
                device,
                queue,
                layout,
                [VertexUniforms::SIZE, PixelUniforms::SIZE],
                1,
                |_, v, p| {
                    vertex.write_bytes(v);
                    pixel.write_bytes(p);
                },
            )
            .ok_or("decal constants")?;
        let vertex_bytes: &[u8] = bytemuck::cast_slice(&prepared.vertices);
        let indices = quad_indices(prepared.vertices.len() as u32 / 4);
        let index_bytes: &[u8] = bytemuck::cast_slice(&indices);
        let (vertices, vertex_at) = uploads.stage(device, queue, vertex_bytes);
        let (index_buffer, index_at) = uploads.stage(device, queue, index_bytes);
        let blend_constant = gfx::translate::blend_constant(&shader.render);
        let render = RenderState::default();
        let packets = prepared
            .clips
            .iter()
            .map(|c| {
                RecordedDraw::prepared_indexed(
                    &pipeline.described,
                    pipeline.pending.clone(),
                    vec![(vertices.clone(), vertex_at, vertex_bytes.len() as u64)],
                    (index_buffer.clone(), index_at, index_bytes.len() as u64),
                    indices.len() as u32,
                    vec![
                        BoundGroup { index: 0, group: run.group.clone(), offsets: run.offsets(0).to_vec() },
                        BoundGroup::new(1, textures.clone()),
                    ],
                    &render,
                    t.size,
                    c.scaled(scale),
                )
                .with_blend_constant(blend_constant)
            })
            .collect();
        Ok(Some((packets, program.discards)))
    }
}

fn build_program(
    device: &wgpu::Device,
    key: &ProgramKey,
    uniforms: &wgpu::BindGroupLayout,
    id: ShaderId,
) -> Result<CachedProgram, &'static str> {
    let program = vsh::Program::parse(&key.microcode).map_err(|_| "vertex program")?;
    let vs = vsh::translate(
        &program,
        &VertexOptions {
            provided_inputs: key.provided_inputs,
            flat_shading: key.options.flat_shading,
            ..Default::default()
        },
    )
    .map_err(|_| "vertex translation")?;
    let ps = psh::translate(&key.combiner, &key.options);
    if !ps.warnings.is_empty()
        || ps.textures.iter().enumerate().any(|(n, d)| n != 0 && d.is_some())
    {
        return Err("pixel translation");
    }
    let wgsl = shaders::link(&vs, &ps).map_err(|_| "shader link")?;
    shaders::validate(&wgsl).map_err(|_| "shader validation")?;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native decal program"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let sampled = ps.textures[0].is_some();
    let entries: Vec<_> = if sampled {
        vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]
    } else {
        Vec::new()
    };
    let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native decal textures"),
        entries: &entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native decal program"),
        bind_group_layouts: &[Some(uniforms), Some(&textures)],
        immediate_size: 0,
    });
    let unsampled = (!sampled).then(|| {
        gfx::churn::create_bind_group(
            device,
            &wgpu::BindGroupDescriptor {
                label: Some("native decal no textures"),
                layout: &textures,
                entries: &[],
            },
        )
    });
    Ok(CachedProgram { id, module, layout, textures, unsampled, sampled, discards: ps.discards })
}

fn decal_pipeline(
    device: &wgpu::Device,
    program: &CachedProgram,
    key: &PipelineKey,
) -> Option<CachedPipeline> {
    let attributes = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Unorm8x4Bgra,
            offset: 12,
            shader_location: 3,
        },
    ];
    let pipeline = gfx::program::validated_resource(device, || {
        gfx::churn::create_render_pipeline(
            device,
            &wgpu::RenderPipelineDescriptor {
                label: Some("native decal program"),
                layout: Some(&program.layout),
                vertex: wgpu::VertexState {
                    module: &program.module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_SIZE as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: key.primitive,
                depth_stencil: key.depth_stencil.clone(),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &program.module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(key.color_target.clone())],
                }),
                multiview_mask: None,
                cache: None,
            },
        )
    })?;
    Some(CachedPipeline { described: key.clone(), pending: Pending::ready(Some(pipeline)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(mode: Mode) -> RenderState {
        let mut rs = RenderState {
            color_write: gfx::state::ColorWriteMask::from_xbox(0x0001_0101),
            ..RenderState::default()
        };
        match mode {
            Mode::Opaque => {
                rs.depth_mode = DepthBufferMode::Disabled;
                rs.cull_mode = CullMode::None;
            }
            _ => {
                rs.depth_mode = DepthBufferMode::W;
                rs.depth_func = CompareFunc::Always;
                rs.depth_write = true;
                rs.solid_offset_enable = true;
                rs.depth_bias = -4.0;
                rs.blend_enable = true;
                (rs.src_blend, rs.dst_blend, rs.blend_op) = expected_blend(mode).unwrap();
            }
        }
        rs
    }

    #[test]
    fn every_mode_checks_its_depth_and_blend() {
        let stages = std::array::from_fn(|n| TextureStageState::default_for_stage(n as u32));
        let extras = [StageExtras::default(); 4];
        for mode in [
            Mode::Alpha0,
            Mode::Alpha1,
            Mode::Multiply,
            Mode::Darken,
            Mode::Maximum,
            Mode::Alpha5,
            Mode::Opaque,
        ] {
            assert_eq!(
                check_state(mode, &state(mode), &stages, &extras),
                Ok(()),
                "{mode:?}"
            );
            let mut stencil = state(mode);
            stencil.stencil.enable = true;
            assert_eq!(
                check_state(mode, &stencil, &stages, &extras),
                Err("stencil enabled")
            );
            let mut blend = state(mode);
            blend.blend_enable = !blend.blend_enable;
            assert_eq!(
                check_state(mode, &blend, &stages, &extras),
                Err("blend state")
            );
        }
    }

    #[test]
    fn mode_specific_bias_is_not_inherited() {
        let stages = std::array::from_fn(|n| TextureStageState::default_for_stage(n as u32));
        let extras = [StageExtras::default(); 4];
        let mut rs = state(Mode::Multiply);
        rs.solid_offset_enable = false;
        assert_eq!(
            check_state(Mode::Multiply, &rs, &stages, &extras),
            Err("W depth/bias")
        );
        rs.solid_offset_enable = true;
        rs.depth_bias_slope_scale = 1.0;
        assert_eq!(
            check_state(Mode::Multiply, &rs, &stages, &extras),
            Err("W depth/bias")
        );
    }
}
