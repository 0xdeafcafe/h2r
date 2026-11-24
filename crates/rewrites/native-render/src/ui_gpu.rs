//! Native UI packets over the device's pinned attachments. The guest's
//! shader programs and raster state are shared inputs with the D3D renderer.
//!
//! [`OverlayGpu`] draws the scene's overlay stage (`frame_scene::overlay`,
//! planned by [`crate::overlay`]) with named programs for the fixed
//! combiners and the blur, over no guest state at all.
mod hud_shader;
pub use hud_shader::hud_shader_supported;
use hud_shader::{HudPipeline, HudPipelineKey};

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use d3d::backend::{ExternalAttachments, ExternalDraws, ResolvedDraw};
use d3d::nv2a::{NativeDrawKey, NativeDrawProgram, UiProgramError};
use d3d::scaled::RenderScale;
use gfx::compile::{Pending, PendingPipeline};
use gfx::pipeline::{PipelineKey, ShaderId, ShaderProgram, TargetFormats, create_render_pipeline};
use gfx::program::{ProgramBindings, StageDims, filterable, validated_resource};
use gfx::repack::{FetchMode, RepackPlan};
use gfx::state::FillMode;
use gfx::texture::TextureKind;
use gfx::translate::Capabilities;
use gfx::vertex::{HostPrimitive, PrimitiveType};
use gfx::external::ExternalEncoded;
use gfx::{BoundGroup, RecordedDraw};
use shaders::{psh::PixelUniforms, vsh::VertexUniforms};

use crate::program::scaled_scissor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Geometry,
    Target,
    State,
    Shader,
    Texture,
    Upload,
}

#[derive(Default)]
pub struct UiGpu(HashMap<wgpu::Device, DeviceCache>);

struct DeviceCache {
    frame: u32,
    fetch: FetchMode,
    bindings: ProgramBindings,
    shaders: HashMap<NativeDrawKey, Option<Program>>,
    pipelines: HashMap<PipelineKey, Arc<PendingPipeline>>,
}

#[derive(Clone)]
struct Program {
    id: ShaderId,
    shader: ShaderProgram,
    dims: StageDims,
    discards: bool,
}

impl UiGpu {
    pub fn prepare(
        &mut self,
        target: &ExternalAttachments,
        draw: &ResolvedDraw,
        frame: u32,
    ) -> Result<ExternalDraws, Refusal> {
        let cache = self.0.entry(target.device.clone()).or_insert_with(|| DeviceCache {
            frame,
            fetch: FetchMode::Gpu,
            bindings: ProgramBindings::new(&target.device),
            shaders: HashMap::new(),
            pipelines: HashMap::new(),
        });
        if cache.frame != frame {
            cache.frame = frame;
            cache.bindings.end_frame();
        }
        cache.prepare(target, draw)
    }
}

impl DeviceCache {
    fn prepare(
        &mut self,
        target: &ExternalAttachments,
        draw: &ResolvedDraw,
    ) -> Result<ExternalDraws, Refusal> {
        let guest = (draw.target.width, draw.target.height);
        let scale = RenderScale::between(guest, target.size).ok_or(Refusal::Target)?;
        let scissor = if draw.clip_rects.is_empty() {
            std::slice::from_ref(draw.clip.as_ref().ok_or(Refusal::Geometry)?)
        } else {
            &draw.clip_rects
        };
        let mut clips = scissor.iter().map(|&clip| {
            let clip = draw.display_rect(clip);
            scaled_scissor(&clip, scale, target.size)
        });
        let first = clips.next().flatten().ok_or(Refusal::Geometry)?;
        let rest = clips.collect::<Option<Vec<_>>>().ok_or(Refusal::Geometry)?;
        let indices = indices(draw.primitive, draw.vertex_count, draw.indices.as_deref())
            .ok_or(Refusal::Geometry)?;
        let count = u32::try_from(indices.len()).map_err(|_| Refusal::Geometry)?;
        if draw.streams.len() != draw.layout.streams.len() {
            return Err(Refusal::Geometry);
        }
        let strides: Vec<_> = draw.streams.iter().map(|stream| stream.stride).collect();
        let repack = RepackPlan::with_mode(&draw.layout, &strides, self.fetch);
        let device = &target.device;
        let caps = Capabilities::from_features(device.features());
        let depth = draw.target.depth_format.map(|_| target.depth_format);
        if gfx::surface::color_surface_format(draw.target.format) != Some(target.color_format)
            || draw.target.depth_format.and_then(|format| gfx::surface::depth_surface_format(format, caps)) != depth
        {
            return Err(Refusal::Target);
        }
        let mut state = draw.state.render;
        if (state.fill_mode == FillMode::Wireframe && !caps.polygon_mode_line)
            || (state.fill_mode == FillMode::Point && !caps.polygon_mode_point)
            || (state.depth_clamp && !caps.depth_clip_control)
        {
            return Err(Refusal::State);
        }
        let program = NativeDrawProgram::resolve_ui(
            draw, target.stages.each_ref().map(Option::is_some), caps, self.fetch, scale,
        ).map_err(|why| match why {
            UiProgramError::Texture => Refusal::Texture,
            UiProgramError::Shader => Refusal::Shader,
        })?;
        if program.writes_depth {
            // The W-depth fragment program already applies this offset.
            state.solid_offset_enable = false;
            state.wireframe_offset_enable = false;
            state.point_offset_enable = false;
        }
        let id = ShaderId(self.shaders.len().try_into().map_err(|_| Refusal::Shader)?);
        let shader = self.shaders.entry(program.key.clone()).or_insert_with(|| {
            let linked = program.key.translate().ok()?;
            let module = validated_resource(device, || device.create_shader_module(
                wgpu::ShaderModuleDescriptor {
                    label: Some("native UI shader"),
                    source: wgpu::ShaderSource::Wgsl(linked.source.into()),
                },
            ))?;
            Some(Program {
                id,
                shader: self.bindings.program(device, module, linked.dims),
                dims: linked.dims,
                discards: linked.discards,
            })
        }).clone().ok_or(Refusal::Shader)?;
        let views = target.stages.each_ref().map(Option::as_ref);
        for (n, dimension) in shader.dims.into_iter().enumerate() {
            let Some(dimension) = dimension else { continue };
            let view = views[n].ok_or(Refusal::Texture)?;
            let info = draw.texture_info[n].ok_or(Refusal::Texture)?;
            let actual = match info.kind {
                TextureKind::D2 => wgpu::TextureViewDimension::D2,
                TextureKind::Cube => wgpu::TextureViewDimension::Cube,
                TextureKind::Volume => wgpu::TextureViewDimension::D3,
            };
            if dimension != actual
                || view.texture() == target.color.texture()
                || (depth.is_some() && view.texture() == target.depth.texture())
                || !filterable(view.texture().format(), device.features())
            {
                return Err(Refusal::Texture);
            }
        }
        let textures = self.bindings.textures(
            device, &target.queue, shader.dims, views, program.samplers, [[false; 4]; 4],
        );
        if textures.is_none() && shader.dims.iter().any(Option::is_some) {
            return Err(Refusal::Texture);
        }

        let mut key = PipelineKey::from_plan(
            shader.id, &repack, draw.primitive, &state,
            TargetFormats { color: target.color_format, depth, samples: 1 }, caps,
        );
        if matches!(key.primitive.topology, wgpu::PrimitiveTopology::TriangleStrip | wgpu::PrimitiveTopology::LineStrip) {
            key.primitive.strip_index_format = Some(wgpu::IndexFormat::Uint32);
        }
        let pipeline = self.pipelines.entry(key.clone()).or_insert_with(|| {
            Pending::ready(validated_resource(device, || {
                create_render_pipeline(device, &key, &shader.shader, "native UI pipeline")
            }))
        }).clone();
        if pipeline.get().is_none() {
            return Err(Refusal::State);
        }

        let upload = |bytes: &[u8]| {
            let len = bytes.len() as u64;
            if len == 0 || len.next_power_of_two() > device.limits().max_buffer_size {
                return Err(Refusal::Upload);
            }
            let (buffer, offset) = target.uploads.stage(device, &target.queue, bytes);
            Ok((buffer, offset, len))
        };
        let mut streams = Vec::with_capacity(draw.streams.len());
        for (plan, stream) in repack.streams.iter().zip(&draw.streams) {
            let bytes = plan.repack(&stream.data, draw.vertex_count).map_err(|_| Refusal::Geometry)?;
            streams.push(upload(&bytes)?);
        }
        let indices = upload(bytemuck::cast_slice(&indices[..]))?;
        let uniforms = target.uploads.uniform_run(
            device, &target.queue, self.bindings.uniform_layout(),
            [VertexUniforms::SIZE, PixelUniforms::SIZE], 1,
            |_, vertex, pixel| {
                program.vertex.write_bytes(vertex);
                program.pixel.write_bytes(pixel);
            },
        ).ok_or(Refusal::Upload)?;
        let offsets = uniforms.offsets(0).to_vec();
        let mut groups = vec![BoundGroup { index: 0, group: uniforms.group, offsets }];
        groups.extend(textures.map(|group| BoundGroup::new(1, group)));
        let packet = RecordedDraw::prepared_indexed(
            &key, pipeline, streams, indices, count, groups, &state, target.size, first,
        );
        let mut packets = vec![packet];
        for clip in rest {
            packets.push(packets[0].clone().with_scissor(clip, target.size));
        }
        Ok(ExternalDraws { ranges: 1, packets, discards: shader.discards, writes: Vec::new() })
    }
}

/// The overlay's fixed combiners (`frame_scene::Combiner`), one entry
/// point each over one vertex program.
const OVERLAY_WGSL: &str = r#"
struct VsIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@group(0) @binding(0) var stage0: texture_2d<f32>;
@group(0) @binding(1) var sampler0: sampler;

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.position = vec4<f32>(v.position, 0.0, 1.0);
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

@fragment
fn fs_flat(i: VsOut) -> @location(0) vec4<f32> {
    return i.color;
}

@fragment
fn fs_texture(i: VsOut) -> @location(0) vec4<f32> {
    return textureSample(stage0, sampler0, i.uv) * i.color;
}

@fragment
fn fs_texture_alpha(i: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(stage0, sampler0, i.uv);
    return vec4<f32>(i.color.rgb, i.color.a * t.a);
}

@fragment
fn fs_texture_grey(i: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(stage0, sampler0, i.uv);
    let l = dot(t.rgb, vec3<f32>(0.299, 0.587, 0.114));
    return vec4<f32>(l, l, l, t.a) * i.color;
}
"#;

/// The overlay's blur step (`frame_scene::OverlayBlur`): every pixel the
/// mean of four bilinear taps at the corners of a square, the offset a
/// fraction of the target. One full-screen triangle.
const OVERLAY_BLUR_WGSL: &str = r#"
struct Blur {
    offset: vec4<f32>,
};
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var taps: sampler;
@group(0) @binding(2) var<uniform> blur: Blur;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_blur(i: Out) -> @location(0) vec4<f32> {
    let d = blur.offset.xy;
    let sum = textureSample(source, taps, i.uv + vec2<f32>(-d.x, -d.y))
        + textureSample(source, taps, i.uv + vec2<f32>(d.x, -d.y))
        + textureSample(source, taps, i.uv + vec2<f32>(-d.x, d.y))
        + textureSample(source, taps, i.uv + vec2<f32>(d.x, d.y));
    return sum * 0.25;
}
"#;

/// The textures an overlay names, as the renderer holds them.
pub trait OverlayTextures {
    /// A 2D, filterable view of `texture`; `None` when it is not resident.
    fn view(&self, texture: frame_scene::OverlayTexture) -> Option<&wgpu::TextureView>;

    fn stage(&self, _stage: usize, texture: frame_scene::OverlayTexture) -> Option<&wgpu::TextureView> {
        self.view(texture)
    }

    fn stage_sampler(&self, _stage: usize, texture: frame_scene::OverlayTexture) -> Option<&wgpu::Sampler> {
        self.sampler(texture)
    }

    /// The device sampler for a live HUD texture, including its mip and
    /// addressing state. Atlas pages and offline sprites use the material.
    fn sampler(&self, _texture: frame_scene::OverlayTexture) -> Option<&wgpu::Sampler> {
        None
    }
}

/// No textures: only flat items draw.
pub struct NoTextures;

impl OverlayTextures for NoTextures {
    fn view(&self, _texture: frame_scene::OverlayTexture) -> Option<&wgpu::TextureView> {
        None
    }
}

/// An overlay blend as wgpu takes it; `Err` when a value is not a D3D
/// blend factor or equation.
pub fn overlay_blend(blend: frame_scene::OverlayBlend) -> Result<Option<wgpu::BlendState>, ()> {
    use gfx::state::{BlendFactor, BlendOp};
    let frame_scene::OverlayBlend::On { source, dest, equation } = blend else {
        return Ok(None);
    };
    let operation = gfx::translate::blend_operation(BlendOp::from_xbox(equation).ok_or(())?);
    let (src_factor, dst_factor) = match operation {
        wgpu::BlendOperation::Min | wgpu::BlendOperation::Max => (wgpu::BlendFactor::One, wgpu::BlendFactor::One),
        _ => (
            gfx::translate::blend_factor(BlendFactor::from_xbox(source).ok_or(())?),
            gfx::translate::blend_factor(BlendFactor::from_xbox(dest).ok_or(())?),
        ),
    };
    let component = wgpu::BlendComponent { src_factor, dst_factor, operation };
    Ok(Some(wgpu::BlendState { color: component, alpha: component }))
}

/// An item's blend constant (`OverlayItem::blend_constant`) as the pass
/// sets it.
pub fn blend_color([r, g, b, a]: [f32; 4]) -> wgpu::Color {
    wgpu::Color { r: f64::from(r), g: f64::from(g), b: f64::from(b), a: f64::from(a) }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct OverlayPipelineKey {
    color_write: u32,
    combiner: frame_scene::Combiner,
    blend: frame_scene::OverlayBlend,
    format: wgpu::TextureFormat,
}

fn overlay_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    fragment: &str,
    color_write: u32,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x2, 4 => Float32x2, 5 => Float32x2];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("native overlay"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<crate::overlay::PlanVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: gfx::translate::color_writes(gfx::state::ColorWriteMask::from_xbox(color_write)),
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// The overlay stage's programs, pipelines and samplers on one device.
pub struct OverlayGpu {
    module: wgpu::ShaderModule,
    group_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<OverlayPipelineKey, wgpu::RenderPipeline>,
    samplers: HashMap<frame_scene::Sampler, wgpu::Sampler>,
    white: wgpu::TextureView,
    hud: HashMap<HudPipelineKey, Option<HudPipeline>>,
    blur_module: wgpu::ShaderModule,
    blur_group_layout: wgpu::BindGroupLayout,
    blur_layout: wgpu::PipelineLayout,
    blur_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

/// One step of a prepared blur: the scratch it draws into and its bound
/// source and offset.
struct BlurStep {
    dest: usize,
    group: wgpu::BindGroup,
}

/// A blur as recorded: after the draws before it, the target copied to
/// scratch 0, its steps, then the result written back inside the scissor.
struct PreparedBlur {
    before: usize,
    steps: Vec<BlurStep>,
    result: wgpu::BindGroup,
    scissor: crate::overlay::Scissor,
}

impl OverlayGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> OverlayGpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native overlay"),
            source: wgpu::ShaderSource::Wgsl(OVERLAY_WGSL.into()),
        });
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native overlay stage"),
            entries: &[
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
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native overlay"),
            bind_group_layouts: &[Some(&group_layout)],
            immediate_size: 0,
        });
        // What a flat item binds: its program samples nothing.
        let white = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native overlay white"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            white.as_image_copy(),
            &[255; 4],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
            white.size(),
        );
        let blur_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native overlay blur"),
            source: wgpu::ShaderSource::Wgsl(OVERLAY_BLUR_WGSL.into()),
        });
        let blur_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native overlay blur"),
            entries: &[
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let blur_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native overlay blur"),
            bind_group_layouts: &[Some(&blur_group_layout)],
            immediate_size: 0,
        });
        OverlayGpu {
            module,
            group_layout,
            layout,
            pipelines: HashMap::new(),
            samplers: HashMap::new(),
            white: white.create_view(&Default::default()),
            hud: HashMap::new(),
            blur_module,
            blur_group_layout,
            blur_layout,
            blur_pipelines: HashMap::new(),
        }
    }

    fn blur_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let (module, layout) = (&self.blur_module, &self.blur_layout);
        self.blur_pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native overlay blur"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_blur"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        }).clone()
    }

    /// The plan's blurs over `target`: two scratch copies of it and each
    /// step's bind group. `None` when the target cannot be copied from
    /// or sampled; then every blur is refused.
    fn prepare_blurs(
        &mut self,
        device: &wgpu::Device,
        target: &wgpu::TextureView,
        plan: &crate::overlay::OverlayPlan,
    ) -> Option<([wgpu::Texture; 2], Vec<PreparedBlur>)> {
        use wgpu::util::DeviceExt;
        let texture = target.texture();
        let format = texture.format();
        if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
            || texture.sample_count() != 1
            || !filterable(format, device.features())
        {
            return None;
        }
        let scratch = [0, 1].map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("native overlay blur"),
                size: texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let views = scratch.each_ref().map(|t| t.create_view(&Default::default()));
        // The taps are bilinear and clamp at the edges.
        let taps = self.sampler(device, frame_scene::Sampler { wrap: false, point: false });
        let copy = self.sampler(device, frame_scene::Sampler { wrap: false, point: true });
        let mut blurs = Vec::with_capacity(plan.blurs.len());
        for blur in &plan.blurs {
            let mut steps = Vec::with_capacity(blur.offsets.len());
            let mut source = 0;
            for offset in &blur.offsets {
                let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native overlay blur step"),
                    contents: bytemuck::cast_slice(&[offset[0], offset[1], 0.0f32, 0.0f32]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("native overlay blur step"),
                    layout: &self.blur_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[source]) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&taps) },
                        wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                    ],
                });
                steps.push(BlurStep { dest: 1 - source, group });
                source = 1 - source;
            }
            let result = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native overlay blur result"),
                layout: &self.group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[source]) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&copy) },
                ],
            });
            blurs.push(PreparedBlur { before: blur.before as usize, steps, result, scissor: blur.scissor });
        }
        Some((scratch, blurs))
    }

    fn pipeline(&mut self, device: &wgpu::Device, key: OverlayPipelineKey, blend: Option<wgpu::BlendState>) -> wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.layout);
        self.pipelines.entry(key).or_insert_with(|| {
            let fragment = match key.combiner {
                frame_scene::Combiner::Flat => "fs_flat",
                frame_scene::Combiner::Texture => "fs_texture",
                frame_scene::Combiner::TextureAlpha => "fs_texture_alpha",
                frame_scene::Combiner::TextureGrey => "fs_texture_grey",
            };
            overlay_pipeline(device, module, layout, fragment, key.color_write, key.format, blend)
        }).clone()
    }

    fn sampler(&mut self, device: &wgpu::Device, key: frame_scene::Sampler) -> wgpu::Sampler {
        self.samplers.entry(key).or_insert_with(|| {
            let address = if key.wrap { wgpu::AddressMode::Repeat } else { wgpu::AddressMode::ClampToEdge };
            let filter = if key.point { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("native overlay"),
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        }).clone()
    }

    /// Prepares `plan` over `target` (its colour, loaded and stored; the
    /// plan's target size must be the view's). Each batch whose texture
    /// is not resident, or whose blend is not a D3D blend, is dropped and
    /// counted in the returned stats' `refused_texture`; `batches` counts
    /// the draws recorded. A blur ends the pass, copies the target, runs
    /// its steps in scratch targets and writes the result back inside its
    /// scissor; over a target that cannot be copied or sampled every blur
    /// is dropped and counted in `refused_blur`. The plan's own counts are
    /// carried over.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        target: &wgpu::TextureView,
        plan: &crate::overlay::OverlayPlan,
        textures: &dyn OverlayTextures,
    ) -> Result<(ExternalEncoded, crate::overlay::OverlayStats), &'static str> {
        let size = target.texture().size();
        if [size.width, size.height] != plan.target {
            return Err("overlay target size changed");
        }
        let format = target.texture().format();
        let mut stats = plan.stats;
        struct Draw {
            batch: usize,
            stages: Option<wgpu::BindGroup>,
            pipeline: wgpu::RenderPipeline,
            group: wgpu::BindGroup,
            scissor: crate::overlay::Scissor,
            indices: std::ops::Range<u32>,
            blend_constant: wgpu::Color,
        }
        let mut draws = Vec::with_capacity(plan.batches.len());
        for (index, batch) in plan.batches.iter().enumerate() {
            let Ok(blend) = overlay_blend(batch.material.blend) else {
                stats.refused_texture += batch.items;
                continue;
            };
            let sampler = self.sampler(device, batch.material.sampler);
            let (pipeline, group, stages) = match batch.material.program {
                frame_scene::Program::Fixed(combiner) => {
                    let view = if batch.material.sampled() {
                        match textures.view(batch.material.texture) {
                            Some(view) if view.texture().dimension() == wgpu::TextureDimension::D2
                                && filterable(view.texture().format(), device.features()) => view.clone(),
                            _ => {
                                stats.refused_texture += batch.items;
                                continue;
                            }
                        }
                    } else {
                        self.white.clone()
                    };
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("native overlay stage"),
                        layout: &self.group_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                        ],
                    });
                    let key = OverlayPipelineKey { combiner, color_write: batch.color_write, blend: batch.material.blend, format };
                    (self.pipeline(device, key, blend), group, None)
                }
                frame_scene::Program::Shader { .. } => {
                    let Some(words) = batch.shader_words.as_deref().and_then(|s| s.try_into().ok()) else {
                        stats.refused_material += batch.items;
                        continue;
                    };
                    let mut def = shaders::psh::PixelShaderDef::from_words(words);
                    let (c0, c1) = def.embedded_constants();
                    // Fill changes update the uniforms and reuse the same pipeline.
                    def.constant0 = [0; 8];
                    def.constant1 = [0; 8];
                    def.final_constant0 = 0;
                    def.final_constant1 = 0;
                    let key = HudPipelineKey {
                        def, format, glyph: batch.glyph_cache, color_write: batch.color_write,
                        blend: batch.material.blend, alpha_kill: batch.alpha_kill,
                        alpha_func: batch.alpha_test.map(|(func, _)| func),
                    };
                    let entry = self.hud.entry(key);
                    let Some(program) = entry.or_insert_with_key(|key| HudPipeline::new(device, key, blend)) else {
                        stats.refused_material += batch.items;
                        continue;
                    };
                    let mut pixel = PixelUniforms { c0, c1, ..Default::default() };
                    if let Some((_, reference)) = batch.alpha_test {
                        pixel.alpha_ref[0] = f32::from(reference);
                    }
                    let Some((group, stages)) = program.bind(device, textures, batch, &sampler, &pixel) else {
                        stats.refused_texture += batch.items;
                        continue;
                    };
                    (program.pipeline.clone(), group, Some(stages))
                }
            };
            draws.push(Draw {
                batch: index,
                pipeline, group, stages,
                scissor: batch.scissor,
                indices: batch.first_index..batch.first_index + batch.index_count,
                blend_constant: blend_color(batch.blend_constant),
            });
        }
        stats.batches = draws.len() as u32;
        let blurs = if plan.blurs.is_empty() {
            None
        } else {
            let prepared = self.prepare_blurs(device, target, plan);
            if prepared.is_none() {
                stats.refused_blur += plan.blurs.len() as u32;
            }
            prepared
        };
        if draws.is_empty() && blurs.is_none() {
            return Ok((
                ExternalEncoded { ranges: 0, draws: 0, passes: 0, commands: Box::new(|_| {}) },
                stats,
            ));
        }
        let (scratch, blurs) = match blurs {
            Some((scratch, blurs)) => (Some(scratch), blurs),
            None => (None, Vec::new()),
        };
        // A blur's result goes back through the texture combiner as one
        // quad over the whole target: its four corners and six indices
        // follow the plan's.
        let mut plan_vertices = plan.vertices.clone();
        let mut plan_indices = plan.indices.clone();
        let whole = plan_indices.len() as u32..plan_indices.len() as u32 + 6;
        let base = plan_vertices.len() as u32;
        let corner = |x: f32, y: f32, u: f32, v: f32| crate::overlay::PlanVertex {
            position: [x, y],
            uv: [u, v],
            color: [1.0; 4],
            extra_uv: [[0.0; 2]; 3],
        };
        plan_vertices.extend([
            corner(-1.0, 1.0, 0.0, 0.0),
            corner(1.0, 1.0, 1.0, 0.0),
            corner(1.0, -1.0, 1.0, 1.0),
            corner(-1.0, -1.0, 0.0, 1.0),
        ]);
        plan_indices.extend(crate::overlay::QUAD_INDICES.map(|i| base + i));
        let write_back = (!blurs.is_empty()).then(|| {
            let key = OverlayPipelineKey {
                color_write: frame_scene::OverlayItem::RGB_WRITE,
                combiner: frame_scene::Combiner::Texture,
                blend: frame_scene::OverlayBlend::Off,
                format,
            };
            (self.pipeline(device, key, None), self.blur_pipeline(device, format))
        });
        use wgpu::util::DeviceExt;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native overlay vertices"),
            contents: bytemuck::cast_slice(&plan_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native overlay indices"),
            contents: bytemuck::cast_slice(&plan_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let target = target.clone();
        let scratch_views = scratch.as_ref().map(|s| s.each_ref().map(|t| t.create_view(&Default::default())));
        let (width, height) = (size.width as f32, size.height as f32);
        let steps: usize = blurs.iter().map(|b| b.steps.len()).sum();
        let count = draws.len() + steps + blurs.len();
        let passes = 1 + blurs.iter().map(|b| b.steps.len() + 2).sum::<usize>();
        Ok((
            ExternalEncoded {
                ranges: plan.batches.len(),
                draws: count,
                passes,
                commands: Box::new(move |encoder| {
                    let begin = |encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, label: &'static str| {
                        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some(label),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        }).forget_lifetime()
                    };
                    let mut next = 0;
                    let segment = |encoder: &mut wgpu::CommandEncoder, until: usize, next: &mut usize| {
                        let mut pass = begin(encoder, &target, "native overlay");
                        pass.set_viewport(0.0, 0.0, width, height, 0.0, 1.0);
                        pass.set_vertex_buffer(0, vertices.slice(..));
                        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                        while let Some(d) = draws.get(*next).filter(|d| d.batch < until) {
                            pass.set_pipeline(&d.pipeline);
                            pass.set_bind_group(0, &d.group, &[]);
                            if let Some(stages) = &d.stages { pass.set_bind_group(1, stages, &[]); }
                            pass.set_scissor_rect(d.scissor.x, d.scissor.y, d.scissor.width, d.scissor.height);
                            pass.set_blend_constant(d.blend_constant);
                            pass.draw_indexed(d.indices.clone(), 0, 0..1);
                            *next += 1;
                        }
                    };
                    for blur in &blurs {
                        let (Some(views), Some(scratch), Some((copy, step))) =
                            (scratch_views.as_ref(), scratch.as_ref(), write_back.as_ref())
                        else {
                            break;
                        };
                        segment(encoder, blur.before, &mut next);
                        encoder.copy_texture_to_texture(
                            target.texture().as_image_copy(),
                            scratch[0].as_image_copy(),
                            target.texture().size(),
                        );
                        for s in &blur.steps {
                            let mut pass = begin(encoder, &views[s.dest], "native overlay blur");
                            pass.set_pipeline(step);
                            pass.set_bind_group(0, &s.group, &[]);
                            pass.draw(0..3, 0..1);
                        }
                        let mut pass = begin(encoder, &target, "native overlay blur result");
                        pass.set_viewport(0.0, 0.0, width, height, 0.0, 1.0);
                        pass.set_vertex_buffer(0, vertices.slice(..));
                        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.set_pipeline(copy);
                        pass.set_bind_group(0, &blur.result, &[]);
                        let c = blur.scissor;
                        pass.set_scissor_rect(c.x, c.y, c.width, c.height);
                        pass.draw_indexed(whole.clone(), 0, 0..1);
                    }
                    segment(encoder, usize::MAX, &mut next);
                }),
            },
            stats,
        ))
    }
}

/// The textures an overlay names, resident on one device: each bitmap
/// image uploaded once, each font's glyph page uploaded when it first
/// appears and, as it grows, each glyph added since the version it holds
/// written once, by its rectangle ([`crate::overlay::GlyphPages`]).
#[derive(Default)]
pub struct OverlayResidency {
    /// The assets' generation what it holds came from.
    generation: u64,
    images: HashMap<(u32, u16), wgpu::TextureView>,
    /// Each font's page: the version it holds, its texture and view.
    glyphs: HashMap<u16, (u64, wgpu::Texture, wgpu::TextureView)>,
}

fn overlay_texture(device: &wgpu::Device, label: &str, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_rgba(queue: &wgpu::Queue, texture: &wgpu::Texture, rgba: &[u8]) {
    let size = texture.size();
    queue.write_texture(
        texture.as_image_copy(),
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * size.width),
            rows_per_image: Some(size.height),
        },
        size,
    );
}

/// Writes one glyph's rectangle of a `side` texel square page (`rgba`,
/// row-major) to `texture`: the texels a page gained, not the page.
fn write_glyph(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    side: u32,
    rgba: &[u8],
    rect: crate::overlay::GlyphRect,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: rect.x, y: rect.y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        &rgba[(rect.y * side + rect.x) as usize * 4..],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * side),
            rows_per_image: Some(rect.height),
        },
        wgpu::Extent3d { width: rect.width, height: rect.height, depth_or_array_layers: 1 },
    );
}

impl OverlayResidency {
    /// Uploads what `assets` holds that is not resident, or has changed:
    /// how many textures it wrote. A font's glyph page is written whole
    /// when it first appears and, when its version moves, in the rectangles
    /// of the glyphs added since the version held, so no glyph's texels
    /// are written twice. Assets of another generation (another map)
    /// replace all it holds. An image with no texels, or fewer than its
    /// size needs, or past the device's largest texture, stays out (its
    /// batches are refused as not resident).
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &crate::overlay::OverlayAssets,
    ) -> usize {
        if assets.generation != self.generation {
            self.images.clear();
            self.glyphs.clear();
            self.generation = assets.generation;
        }
        let largest = device.limits().max_texture_dimension_2d;
        let mut wrote = 0;
        for (&key, image) in &assets.images {
            if self.images.contains_key(&key) {
                continue;
            }
            let (w, h) = (image.width, image.height);
            if w == 0 || h == 0 || w > largest || h > largest || image.rgba.len() < (w * h * 4) as usize {
                continue;
            }
            let texture = overlay_texture(device, "native overlay bitmap", w, h);
            write_rgba(queue, &texture, &image.rgba[..(w * h * 4) as usize]);
            self.images.insert(key, texture.create_view(&Default::default()));
            wrote += 1;
        }
        for font in assets.glyphs.fonts() {
            let Some((side, rgba, version)) = assets.glyphs.page(font) else { continue };
            if side == 0 || side > largest {
                continue;
            }
            match self.glyphs.get_mut(&font) {
                Some((resident, _, _)) if *resident == version => {}
                Some((resident, texture, _)) => {
                    if let Some((side, rgba, added)) = assets.glyphs.added_since(font, *resident) {
                        for (_, rect) in added {
                            write_glyph(queue, texture, side, rgba, *rect);
                        }
                    }
                    *resident = version;
                    wrote += 1;
                }
                None => {
                    let texture = overlay_texture(device, "native overlay glyphs", side, side);
                    write_rgba(queue, &texture, rgba);
                    let view = texture.create_view(&Default::default());
                    self.glyphs.insert(font, (version, texture, view));
                    wrote += 1;
                }
            }
        }
        wrote
    }
}

impl OverlayTextures for OverlayResidency {
    fn view(&self, texture: frame_scene::OverlayTexture) -> Option<&wgpu::TextureView> {
        match texture {
            frame_scene::OverlayTexture::None | frame_scene::OverlayTexture::Bound { .. } => None,
            frame_scene::OverlayTexture::Bitmap { tag, image } => self.images.get(&(tag, image)),
            frame_scene::OverlayTexture::Glyphs { font } => self.glyphs.get(&font).map(|(_, _, view)| view),
        }
    }
}

/// Adds an overlay's stats to `render.stats`' `native_ui_overlay_*`
/// counters.
pub fn count_overlay(stats: &crate::overlay::OverlayStats) {
    use timeline::native::Ui;
    for (counter, n) in [
        (Ui::OverlayItems, stats.items),
        (Ui::OverlayQuads, stats.quads),
        (Ui::OverlayTextRuns, stats.text_runs),
        (Ui::OverlayFills, stats.fills),
        (Ui::OverlayGlyphs, stats.glyphs),
        (Ui::OverlayBatches, stats.batches),
        (Ui::OverlayRefusedMaterial, stats.refused_material),
        (Ui::OverlayRefusedClip, stats.refused_clip),
        (Ui::OverlayRefusedGlyph, stats.refused_glyph),
        (Ui::OverlayRefusedTexture, stats.refused_texture),
        (Ui::OverlayBlurs, stats.blurs),
        (Ui::OverlayModelViews, stats.model_views),
        (Ui::OverlayRefusedModelView, stats.refused_model_view),
        (Ui::OverlayRefusedBlur, stats.refused_blur),
    ] {
        if n != 0 {
            counter.count_n(u64::from(n));
        }
    }
}

fn indices(primitive: PrimitiveType, vertex_count: u32, original: Option<&[u32]>) -> Option<Cow<'_, [u32]>> {
    if original.is_some_and(|indices| indices.iter().any(|&i| i >= vertex_count)) {
        return None;
    }
    let count = original.map_or(vertex_count as usize, <[u32]>::len).try_into().ok()?;
    let indices = match (primitive.to_host(count), original) {
        (HostPrimitive::Native(_), Some(indices)) => Cow::Borrowed(indices),
        (HostPrimitive::Native(_), None) => Cow::Owned((0..count).collect()),
        (HostPrimitive::Indexed(_, order), None) => Cow::Owned(order),
        (HostPrimitive::Indexed(_, order), Some(indices)) => {
            Cow::Owned(order.into_iter().map(|i| indices[i as usize]).collect())
        }
    };
    (!indices.is_empty()).then_some(indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_fan_indices_keep_the_guest_order_even_with_a_remapped_stream() {
        let fan = PrimitiveType::TriangleFan;
        assert_eq!(&*indices(fan, 4, None).unwrap(), &[0, 1, 2, 0, 2, 3]);
        assert_eq!(&*indices(fan, 8, Some(&[2, 3, 6, 7])).unwrap(), &[2, 3, 6, 2, 6, 7]);
        assert!(indices(fan, 4, Some(&[0, 1, 2, 4])).is_none());
        assert!(indices(fan, 2, None).is_none());
    }

    #[test]
    fn overlay_blends_are_the_guests_d3d_values_and_others_are_refused() {
        use frame_scene::OverlayBlend;
        assert_eq!(overlay_blend(OverlayBlend::Off), Ok(None));
        let alpha = overlay_blend(OverlayBlend::ALPHA).unwrap().unwrap();
        assert_eq!(alpha.color.src_factor, wgpu::BlendFactor::SrcAlpha);
        assert_eq!(alpha.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(alpha.color.operation, wgpu::BlendOperation::Add);
        let max = overlay_blend(OverlayBlend::On { source: 0x302, dest: 0x303, equation: 0x8008 }).unwrap().unwrap();
        assert_eq!((max.color.src_factor, max.color.dst_factor), (wgpu::BlendFactor::One, wgpu::BlendFactor::One));
        assert!(overlay_blend(OverlayBlend::On { source: 0x999, dest: 0x303, equation: 0x8006 }).is_err());
        assert!(overlay_blend(OverlayBlend::On { source: 0x302, dest: 0x303, equation: 1 }).is_err());
    }

    /// A 2 by 2 guest overlay drawn into a 4 by 4 target: a red fill over
    /// everything, then a green sprite clipped to the right half, then a
    /// sprite whose bitmap is not resident (dropped, counted).
    #[test]
    fn the_overlay_draws_in_order_inside_its_clips_and_drops_a_missing_texture() {
        use frame_scene::{ClipRect, Combiner, Overlay, OverlayBlend, OverlayMaterial, OverlayTexture, OverlayVertex, Program, Sampler};

        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let texture = |size: u32, usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("test overlay"),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage,
                view_formats: &[],
            })
        };
        let target = texture(4, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let green = texture(1, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        ctx.queue.write_texture(
            green.as_image_copy(),
            &[0, 255, 0, 255],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
            green.size(),
        );
        struct One(wgpu::TextureView);
        impl OverlayTextures for One {
            fn view(&self, texture: OverlayTexture) -> Option<&wgpu::TextureView> {
                matches!(texture, OverlayTexture::Bitmap { tag: 7, .. }).then_some(&self.0)
            }
        }
        let textures = One(green.create_view(&Default::default()));
        let sprite = |tag| OverlayMaterial {
            program: Program::Fixed(Combiner::Texture),
            texture: OverlayTexture::Bitmap { tag, image: 0 },
            sampler: Sampler { wrap: false, point: true },
            blend: OverlayBlend::Off,
        };
        let corner = |x, y| OverlayVertex { position: [x, y], uv: [x / 2.0, y / 2.0], color: [1.0; 4] };
        let whole = [corner(0.0, 0.0), corner(2.0, 0.0), corner(2.0, 2.0), corner(0.0, 2.0)];
        let mut overlay = Overlay::new([2, 2]);
        overlay.push_fill(ClipRect::full([2, 2]), [0.0, 0.0, 2.0, 2.0], [1.0, 0.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        overlay.push_quad(sprite(7), ClipRect { x: 1, y: 0, width: 1, height: 2 }, whole).unwrap();
        overlay.push_quad(sprite(8), ClipRect::full([2, 2]), whole).unwrap();
        // Synthetic template: the final combiner copies final c0 into RGB.
        let mut words = [0u32; 60];
        words[8] = shaders::psh::bits::inputs(0, 0, 0, shaders::psh::bits::REG_C0);
        words[9] = shaders::psh::bits::final_efg(0, 0, shaders::psh::bits::REG_C0 | shaders::psh::bits::CHAN_ALPHA, 0);
        words[43] = 0x83_37_42_a8;
        let material = OverlayMaterial { program: Program::Shader { tag: 9 }, texture: OverlayTexture::None, ..sprite(7) };
        let item = overlay.push_quad(material, ClipRect { x: 0, y: 1, width: 1, height: 1 }, whole).unwrap();
        overlay.items[item as usize].shader_words = Some(words.into());
        overlay.items[item as usize].color_write = gfx::state::ColorWriteMask::ALL.xbox_value();

        let plan = crate::overlay::OverlayPlan::new(&overlay, [4, 4], &crate::overlay::NoGlyphs);
        assert_eq!(plan.batches.len(), 4);
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let view = target.create_view(&Default::default());
        let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, &textures).unwrap();
        assert_eq!((stats.batches, stats.refused_texture, stats.items), (3, 1, 4));
        assert_eq!((encoded.draws, encoded.passes), (3, 1));
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (encoded.commands)(&mut encoder);
        ctx.queue.submit([encoder.finish()]);
        let pixels = gfx::readback::read_texture(&ctx, &target, wgpu::TextureAspect::All, 4, (4, 4));
        let rgb = |x: usize, y: usize| {
            let i = 4 * (y * 4 + x);
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        for y in 0..4 {
            let left = if y < 2 { [255, 0, 0] } else { [0x37, 0x42, 0xa8] };
            assert_eq!(rgb(0, y), left);
            assert_eq!(rgb(1, y), left);
            assert_eq!(rgb(2, y), [0, 255, 0], "row {y}: the sprite");
            assert_eq!(rgb(3, y), [0, 255, 0], "row {y}: the sprite");
        }
        for (i, p) in pixels.chunks_exact(4).enumerate() {
            assert_eq!(p[3], if i / 4 >= 2 && i % 4 < 2 { 0x83 } else { 0 });
        }
        let wrong = crate::overlay::OverlayPlan::new(&overlay, [8, 8], &crate::overlay::NoGlyphs);
        assert!(gpu.prepare(&ctx.device, &view, &wrong, &textures).is_err(), "the plan is for another size");
    }

    /// The residency uploads a bitmap image and a glyph page from the host
    /// assets: a blue sprite over the left half, then a text run of one
    /// glyph (coverage 255 on its left texel, 0 on its right) in yellow
    /// over the right half.
    #[test]
    fn the_overlay_draws_resident_images_and_glyph_pages() {
        use frame_scene::{ClipRect, Combiner, Overlay, OverlayBlend, OverlayGlyph, OverlayMaterial, OverlayTexture, OverlayVertex, Program, Sampler};
        use crate::overlay::{OverlayAssets, OverlayImage, OverlayPlan};

        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test overlay"),
            size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut assets = OverlayAssets::default();
        assets.images.insert((7, 1), OverlayImage { width: 1, height: 1, rgba: vec![0, 0, 255, 255].into() });
        assert!(assets.glyphs.insert(6, 3, 2, 1, &[255, 0]));
        let mut residency = OverlayResidency::default();
        assert_eq!(residency.sync(&ctx.device, &ctx.queue, &assets), 2);
        assert_eq!(residency.sync(&ctx.device, &ctx.queue, &assets), 0, "nothing new");
        assert!(residency.view(OverlayTexture::Bitmap { tag: 7, image: 0 }).is_none());
        let sprite = OverlayMaterial {
            program: Program::Fixed(Combiner::Texture),
            texture: OverlayTexture::Bitmap { tag: 7, image: 1 },
            sampler: Sampler { wrap: false, point: true },
            blend: OverlayBlend::Off,
        };
        let corner = |x, y| OverlayVertex { position: [x, y], uv: [0.5, 0.5], color: [1.0; 4] };
        let left = [corner(0.0, 0.0), corner(1.0, 0.0), corner(1.0, 2.0), corner(0.0, 2.0)];
        let mut overlay = Overlay::new([2, 2]);
        overlay.push_quad(sprite, ClipRect::full([2, 2]), left).unwrap();
        let glyph = [OverlayGlyph { index: 3, rect: [1.0, 0.0, 2.0, 2.0] }];
        overlay.push_text_run(OverlayMaterial::text(6), ClipRect::full([2, 2]), [1.0, 1.0, 0.0, 1.0], &glyph).unwrap();
        let plan = OverlayPlan::new(&overlay, [4, 4], &assets.glyphs);
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let view = target.create_view(&Default::default());
        let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, &residency).unwrap();
        assert_eq!((stats.batches, stats.refused_texture, stats.refused_glyph), (2, 0, 0));
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (encoded.commands)(&mut encoder);
        ctx.queue.submit([encoder.finish()]);
        let pixels = gfx::readback::read_texture(&ctx, &target, wgpu::TextureAspect::All, 4, (4, 4));
        let rgb = |x: usize, y: usize| {
            let i = 4 * (y * 4 + x);
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        for y in 0..4 {
            assert_eq!(rgb(0, y), [0, 0, 255], "row {y}: the sprite");
            assert_eq!(rgb(1, y), [0, 0, 255], "row {y}: the sprite");
            assert_eq!(rgb(2, y), [255, 255, 0], "row {y}: the glyph's covered texel");
            assert_eq!(rgb(3, y), [0, 0, 0], "row {y}: the glyph's empty texel blends to nothing");
        }
        // A glyph added after the page is resident reaches it by its
        // rectangle alone (no texel of the page is written again) and
        // draws: a red glyph over the sprite's top left.
        assert!(assets.glyphs.insert(6, 4, 1, 1, &[255]));
        assert_eq!(residency.sync(&ctx.device, &ctx.queue, &assets), 1, "a grown page is written again");
        let added = [OverlayGlyph { index: 4, rect: [0.0, 0.0, 1.0, 1.0] }];
        let red = OverlayMaterial::text(6);
        overlay.push_text_run(red, ClipRect::full([2, 2]), [1.0, 0.0, 0.0, 1.0], &added).unwrap();
        let plan = OverlayPlan::new(&overlay, [4, 4], &assets.glyphs);
        let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, &residency).unwrap();
        assert_eq!((stats.batches, stats.refused_glyph), (3, 0));
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (encoded.commands)(&mut encoder);
        ctx.queue.submit([encoder.finish()]);
        let pixels = gfx::readback::read_texture(&ctx, &target, wgpu::TextureAspect::All, 4, (4, 4));
        let rgb = |x: usize, y: usize| {
            let i = 4 * (y * 4 + x);
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        assert_eq!((rgb(0, 0), rgb(1, 1)), ([255, 0, 0], [255, 0, 0]), "the glyph added");
        assert_eq!((rgb(0, 3), rgb(3, 3)), ([0, 0, 255], [0, 0, 0]), "the rest as it was");
        // Another map's assets start over.
        let next = OverlayAssets { generation: 1, ..OverlayAssets::default() };
        assert_eq!(residency.sync(&ctx.device, &ctx.queue, &next), 0);
        assert!(residency.view(OverlayTexture::Bitmap { tag: 7, image: 1 }).is_none());
    }

    /// A 4 by 4 overlay: white on the left half, black on the right, a
    /// one-step blur clipped to the right half, then a green fill in the
    /// last column. The blur reads the fill under it; the fill after it
    /// is drawn over it. Over a target that cannot be copied the blur is
    /// dropped and counted.
    #[test]
    fn a_blur_reads_what_was_drawn_and_writes_back_inside_its_clip() {
        use frame_scene::{ClipRect, Overlay, OverlayBlend, OverlayBlur};

        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let target = |usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("test overlay blur"),
                size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage,
                view_formats: &[],
            })
        };
        let all = ClipRect::full([4, 4]);
        let mut overlay = Overlay::new([4, 4]);
        overlay.push_fill(all, [0.0, 0.0, 2.0, 4.0], [1.0; 4], OverlayBlend::Off).unwrap();
        overlay.push_fill(all, [2.0, 0.0, 4.0, 4.0], [0.0, 0.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        let right = ClipRect { x: 2, y: 0, width: 2, height: 4 };
        overlay.push_blur(right, OverlayBlur { tap: 0.5, spreads: vec![1.0] }).unwrap();
        overlay.push_fill(all, [3.0, 0.0, 4.0, 4.0], [0.0, 1.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        let plan = crate::overlay::OverlayPlan::new(&overlay, [4, 4], &crate::overlay::NoGlyphs);
        assert_eq!((plan.batches.len(), plan.blurs.len()), (2, 1));
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);

        let texture = target(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let view = texture.create_view(&Default::default());
        let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, &NoTextures).unwrap();
        assert_eq!((stats.batches, stats.blurs, stats.refused_blur), (2, 1, 0));
        assert_eq!((encoded.draws, encoded.passes), (4, 4), "two batches, one step, one write-back");
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (encoded.commands)(&mut encoder);
        ctx.queue.submit([encoder.finish()]);
        let pixels = gfx::readback::read_texture(&ctx, &texture, wgpu::TextureAspect::All, 4, (4, 4));
        let rgb = |x: usize, y: usize| {
            let i = 4 * (y * 4 + x);
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        for y in 0..4 {
            assert_eq!(rgb(0, y), [255; 3], "row {y}: outside the clip");
            assert_eq!(rgb(1, y), [255; 3], "row {y}: outside the clip");
            // Taps at x = 2 and 3 texels: (255 + 0) / 2 and 0, averaged.
            let blurred = rgb(2, y);
            assert!(blurred.iter().all(|c| c.abs_diff(64) <= 1), "row {y}: blurred {blurred:?}");
            assert_eq!(rgb(3, y), [0, 255, 0], "row {y}: the fill after the blur");
        }

        let plain = target(wgpu::TextureUsages::RENDER_ATTACHMENT);
        let view = plain.create_view(&Default::default());
        let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, &NoTextures).unwrap();
        assert_eq!((stats.batches, stats.refused_blur), (2, 1));
        assert_eq!((encoded.draws, encoded.passes), (2, 1));
    }

    #[test]
    fn ui_copy_scissors_preserve_the_guest_pixels_and_clip_at_the_edge() {
        let guest = (640, 480);
        let host = (1440, 720);
        let scale = RenderScale::between(guest, host).unwrap();
        let clip = d3d::backend::PixelRect { x: 631, y: 11, width: 13, height: 9 };
        let result = scaled_scissor(&clip, scale, host).unwrap();
        assert_eq!((result.x, result.y, result.width, result.height), (1420, 17, 20, 13));
    }
}
