//! Native UI packets over the device's pinned attachments. The guest's
//! shader programs and raster state are shared inputs with the D3D renderer.
//!
//! [`OverlayGpu`] draws the scene's overlay stage (`frame_scene::overlay`,
//! planned by [`crate::overlay`]) with named programs for the fixed
//! combiners and the blur, over no guest state at all.
mod hud_shader;
pub use hud_shader::{glyph_colour, hud_shader_supported};
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
pub struct UiGpu(Option<(wgpu::Device, DeviceCache)>);

const UI_SHADERS: usize = 256;
const UI_PIPELINES: usize = 1024;
const HUD_PIPELINES: usize = 256;

struct DeviceCache {
    frame: u32,
    fetch: FetchMode,
    bindings: ProgramBindings,
    shaders: HashMap<NativeDrawKey, Option<Program>>,
    pipelines: HashMap<PipelineKey, Arc<PendingPipeline>>,
    count_pipelines: HashMap<PipelineKey, Arc<PendingPipeline>>,
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
        if self.0.as_ref().is_none_or(|(device, _)| device != &target.device) {
            self.0 = Some((target.device.clone(), DeviceCache {
                frame,
                fetch: FetchMode::Gpu,
                bindings: ProgramBindings::new(&target.device),
                shaders: HashMap::new(),
                pipelines: HashMap::new(),
                count_pipelines: HashMap::new(),
            }));
        }
        let (_, cache) = self.0.as_mut().ok_or(Refusal::Target)?;
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
        if self.shaders.len() == UI_SHADERS && !self.shaders.contains_key(&program.key) {
            // ShaderIds are reused after this clear, so retire the pipelines
            // that key on them together with their shader modules.
            self.shaders.clear();
            self.pipelines.clear();
            self.count_pipelines.clear();
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
            let mut shader = self.bindings.program(device, module, linked.dims);
            if program.writes_depth {
                shader.count_entry = Some(shaders::psh::COUNT_DEPTH_ENTRY.into());
            }
            Some(Program {
                id,
                shader,
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
        if self.pipelines.len() == UI_PIPELINES && !self.pipelines.contains_key(&key) {
            self.pipelines.clear();
        }
        let pipeline = self.pipelines.entry(key.clone()).or_insert_with(|| {
            Pending::ready(validated_resource(device, || {
                create_render_pipeline(device, &key, &shader.shader, "native UI pipeline")
            }))
        }).clone();
        if pipeline.get().is_none() {
            return Err(Refusal::State);
        }

        let count_pipeline = if draw.hw.zpass_count_enable {
            if self.count_pipelines.len() == UI_PIPELINES && !self.count_pipelines.contains_key(&key) {
                self.count_pipelines.clear();
            }
            let count = self.count_pipelines.entry(key.clone()).or_insert_with(|| {
                Pending::ready(validated_resource(device, || {
                    gfx::renderer::create_sample_count_pipeline(device, &key, &shader.shader)
                }))
            }).clone();
            count.get().as_ref().ok_or(Refusal::State)?;
            Some(count)
        } else {
            None
        };

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
        let packet = match count_pipeline {
            Some(count) => packet.with_sample_count(count),
            None => packet,
        };
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

/// The overlay's blur step (`frame_scene::OverlayBlur`): every pixel a
/// blend of four bilinear taps at the corners of a square, the offset a
/// fraction of the target (`offset.xy`), the combiner's constant in
/// `offset.z`. The colour is blended in pairs as the guest's combiner
/// (`engine::glow::blur_combiner`) does, each stage's output clamped to
/// -1..1: `clamp(4 * (a - b))`, its weight `(d * pick + 1) / 2`, then
/// `w * a + (1 - w) * b`; the alpha is the mean. A `pick` of 0 is the
/// plain mean. One full-screen triangle.
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

fn pick(a: vec3<f32>, b: vec3<f32>, c: f32) -> vec3<f32> {
    let d = clamp(4.0 * (a - b), vec3<f32>(-1.0), vec3<f32>(1.0));
    let w = clamp((d * c + 1.0) * 0.5, vec3<f32>(0.0), vec3<f32>(1.0));
    return clamp(w * a + (1.0 - w) * b, vec3<f32>(-1.0), vec3<f32>(1.0));
}

@fragment
fn fs_blur(i: Out) -> @location(0) vec4<f32> {
    let d = blur.offset.xy;
    let c = blur.offset.z;
    let t0 = textureSample(source, taps, i.uv + vec2<f32>(-d.x, -d.y));
    let t1 = textureSample(source, taps, i.uv + vec2<f32>(d.x, -d.y));
    let t2 = textureSample(source, taps, i.uv + vec2<f32>(-d.x, d.y));
    let t3 = textureSample(source, taps, i.uv + vec2<f32>(d.x, d.y));
    let p0 = pick(t0.rgb, t1.rgb, c);
    let p1 = pick(t2.rgb, t3.rgb, c);
    let alpha = ((t0.a + t1.a) * 0.5 + (t2.a + t3.a) * 0.5) * 0.5;
    return vec4<f32>(pick(clamp(p0, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(p1, vec3<f32>(0.0), vec3<f32>(1.0)), c), alpha);
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

    /// Hold `hold`'s texture for a target of `size` and `format`
    /// ([`OverlayResidency::hold`]); `None` when there is none.
    fn held(&self, _hold: u32, _size: wgpu::Extent3d, _format: wgpu::TextureFormat) -> Option<&wgpu::TextureView> {
        None
    }
}

/// `inner` with the held textures of one target's size resolved:
/// `frame_scene::OverlayTexture::Held` names a hold, not a size.
struct Holding<'a> {
    inner: &'a dyn OverlayTextures,
    held: HashMap<u32, wgpu::TextureView>,
}

impl<'a> Holding<'a> {
    /// Every hold `plan` writes or samples, for a `size` and `format`
    /// target.
    fn new(
        inner: &'a dyn OverlayTextures,
        plan: &crate::overlay::OverlayPlan,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
    ) -> Holding<'a> {
        let sampled = plan.batches.iter().flat_map(|b| {
            std::iter::once(b.material.texture).chain(b.shader_stages.iter().flatten().copied())
        });
        let ids = plan.holds.iter().map(|h| h.hold).chain(sampled.filter_map(|t| match t {
            frame_scene::OverlayTexture::Held { hold } => Some(hold),
            _ => None,
        }));
        let mut held = HashMap::new();
        for id in ids {
            if let Some(view) = inner.held(id, size, format) {
                held.insert(id, view.clone());
            }
        }
        Holding { inner, held }
    }
}

impl OverlayTextures for Holding<'_> {
    fn view(&self, texture: frame_scene::OverlayTexture) -> Option<&wgpu::TextureView> {
        match texture {
            frame_scene::OverlayTexture::Held { hold } => self.held.get(&hold),
            _ => self.inner.view(texture),
        }
    }

    fn stage(&self, stage: usize, texture: frame_scene::OverlayTexture) -> Option<&wgpu::TextureView> {
        match texture {
            frame_scene::OverlayTexture::Held { hold } => self.held.get(&hold),
            _ => self.inner.stage(stage, texture),
        }
    }

    fn stage_sampler(&self, stage: usize, texture: frame_scene::OverlayTexture) -> Option<&wgpu::Sampler> {
        self.inner.stage_sampler(stage, texture)
    }

    fn sampler(&self, texture: frame_scene::OverlayTexture) -> Option<&wgpu::Sampler> {
        self.inner.sampler(texture)
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
#[allow(clippy::result_unit_err)]
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

/// One batch of an overlay plan as recorded: its pipeline, bindings,
/// scissor, index range and the colour target state the pipeline has.
struct BatchDraw {
    batch: usize,
    stages: Option<wgpu::BindGroup>,
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    scissor: crate::overlay::Scissor,
    indices: std::ops::Range<u32>,
    blend_constant: wgpu::Color,
    blend: Option<wgpu::BlendState>,
    write_mask: wgpu::ColorWrites,
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
    // The guest and display copies can alternate sizes on every dialog frame.
    blur_scratch: Vec<[wgpu::TextureView; 2]>,
    /// The overlay's depth target ([`OverlayGpu::depth_target`]), one for
    /// each size a copy draws at.
    depth: Vec<wgpu::TextureView>,
}

/// The overlay's depth format: what a model view's objects test and write
/// (docs/graphics/native-menus-overlay.md, "Model views"). D24 with
/// stencil, as the game's own depth surface: the object programs write the
/// W-buffer depth at D24's scale, and a marked part draws in stencil mode
/// 2. The overlay's own items draw with no depth.
pub const OVERLAY_DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

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

/// The object stage of a planned model view
/// (docs/graphics/native-menus-overlay.md, "Model views"): the packets
/// that draw `view`'s objects with `inputs`, its view constants
/// ([`crate::model_view::view_inputs`]: its camera, and `c[10]`, `c[11]`
/// over its viewport), on a `color` target of `size` texels with the
/// overlay's depth ([`OVERLAY_DEPTH`]). The overlay scissors each packet
/// to the view. An error names why the view's objects do not draw, and
/// the overlay stops on it (`unimplemented!`): no other path draws them.
pub trait ModelViewObjects {
    fn draws(
        &mut self,
        view: &crate::model_view::PlanModelView,
        inputs: &crate::program::ViewInputs,
        color: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> Result<Vec<RecordedDraw>, &'static str>;
}

/// No object stage: a pass whose plan holds no model view (the HUD, the
/// blur). A view in its plan stops it by name.
pub struct NoObjects;

impl ModelViewObjects for NoObjects {
    fn draws(
        &mut self,
        _view: &crate::model_view::PlanModelView,
        _inputs: &crate::program::ViewInputs,
        _color: wgpu::TextureFormat,
        _size: (u32, u32),
    ) -> Result<Vec<RecordedDraw>, &'static str> {
        Err("no object stage")
    }
}

/// A model view as recorded: after the batches and blurs before it, one
/// pass over the target and the overlay's depth (cleared), its objects'
/// packets in order.
struct PreparedView {
    before: usize,
    blurs: usize,
    packets: Vec<RecordedDraw>,
}

/// The packets `objects` makes for `view` on a `format` target of `size`,
/// each scissored to the view: refused when the stage refuses or a packet
/// would not record in the overlay's pass (a pipeline that did not build,
/// a draw counting its samples).
fn model_view_packets(
    objects: &mut dyn ModelViewObjects,
    view: &crate::model_view::PlanModelView,
    target: [u32; 2],
    format: wgpu::TextureFormat,
) -> Result<Vec<RecordedDraw>, &'static str> {
    let inputs = crate::model_view::view_inputs(&view.camera, view.guest, target);
    let size = (target[0], target[1]);
    let packets = objects.draws(view, &inputs, format, size)?;
    if packets.iter().any(|p| p.counts_samples() || !p.pipeline_valid()) {
        return Err("a model view's draw needs a pass of its own");
    }
    let s = view.scissor;
    let scissor = gfx::ScissorRect { x: s.x, y: s.y, width: s.width, height: s.height };
    Ok(packets.into_iter().map(|p| p.with_scissor(scissor, size)).collect())
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
            blur_scratch: Vec::new(),
            depth: Vec::new(),
        }
    }

    /// The overlay's depth target for a colour target of `size`: made the
    /// first time a copy of that size draws a model view, then kept. Each
    /// model view clears it before its objects draw, so nothing in it
    /// outlives a view. The guest's and the display's copies differ in
    /// size, and both are kept rather than remade every frame.
    pub fn depth_target(&mut self, device: &wgpu::Device, size: wgpu::Extent3d) -> wgpu::TextureView {
        let size = wgpu::Extent3d { depth_or_array_layers: 1, ..size };
        if let Some(view) = self.depth.iter().find(|v| v.texture().size() == size) {
            return view.clone();
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native overlay depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OVERLAY_DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        // Two copies' sizes at most: a third (a resized window) takes the
        // oldest's place.
        if self.depth.len() == 2 {
            self.depth.remove(0);
        }
        self.depth.push(view.clone());
        view
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
    ) -> Option<([wgpu::TextureView; 2], Vec<PreparedBlur>)> {
        use wgpu::util::DeviceExt;
        let texture = target.texture();
        let format = texture.format();
        if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
            || texture.sample_count() != 1
            || !filterable(format, device.features())
        {
            return None;
        }
        let views = if let Some(index) = self.blur_scratch.iter().position(|views| {
            let cached = views[0].texture();
            cached.size() == texture.size() && cached.format() == format
        }) {
            self.blur_scratch.remove(index)
        } else {
            if self.blur_scratch.len() == 2 {
                self.blur_scratch.remove(0);
            }
            [0, 1].map(|_| {
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
                }).create_view(&Default::default())
            })
        };
        self.blur_scratch.push(views.clone());
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
                    contents: bytemuck::cast_slice(&[offset[0], offset[1], blur.pick, 0.0f32]),
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
        Some((views, blurs))
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
    /// Each batch of `plan` as a draw on a `format` colour target: its
    /// pipeline, bindings, scissor and index range. A batch whose texture
    /// is not resident, whose blend is not a D3D blend or whose program
    /// does not translate is left out and counted in `stats`; `batches`
    /// counts the draws kept.
    fn batch_draws(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        plan: &crate::overlay::OverlayPlan,
        textures: &dyn OverlayTextures,
        stats: &mut crate::overlay::OverlayStats,
    ) -> Vec<BatchDraw> {
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
                            _ if matches!(batch.material.texture, frame_scene::OverlayTexture::Movie { .. }) => {
                                stats.refused_movie += batch.items;
                                continue;
                            }
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
                    if self.hud.len() == HUD_PIPELINES && !self.hud.contains_key(&key) {
                        self.hud.clear();
                    }
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
            draws.push(BatchDraw {
                batch: index,
                pipeline, group, stages,
                scissor: batch.scissor,
                indices: batch.first_index..batch.first_index + batch.index_count,
                blend_constant: blend_color(batch.blend_constant),
                blend,
                write_mask: gfx::translate::color_writes(gfx::state::ColorWriteMask::from_xbox(batch.color_write)),
            });
        }
        stats.batches = draws.len() as u32;
        draws
    }

    /// `plan`'s batches as packets for the frame's open render pass over a
    /// `format` colour target of `size` texels with no depth attachment:
    /// the vertices and indices staged once in `uploads`, one packet a
    /// batch, in order. Adjacent packets on one target share a render
    /// pass, so a run of widget quads costs one pass, not one each
    /// (docs/graphics/native-post-hud.md, "The HUD on the overlay"). A
    /// plan with a blur needs passes of its own and is refused.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_packets(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        plan: &crate::overlay::OverlayPlan,
        textures: &dyn OverlayTextures,
    ) -> Result<(Vec<RecordedDraw>, crate::overlay::OverlayStats), &'static str> {
        if [size.0, size.1] != plan.target {
            return Err("overlay target size changed");
        }
        if !plan.blurs.is_empty() {
            return Err("overlay blur needs its own passes");
        }
        if !plan.holds.is_empty() {
            return Err("overlay hold needs its own copy");
        }
        let mut stats = plan.stats;
        if !plan.model_views.is_empty() {
            unimplemented!("overlay packets: a model view needs the overlay's depth pass");
        }
        let draws = self.batch_draws(device, format, plan, textures, &mut stats);
        if draws.is_empty() {
            return Ok((Vec::new(), stats));
        }
        let (vertices, vertex_offset) = uploads.stage(device, queue, bytemuck::cast_slice(&plan.vertices));
        let vertex_len = std::mem::size_of_val(plan.vertices.as_slice()) as u64;
        let (indices, index_offset) = uploads.stage(device, queue, bytemuck::cast_slice(&plan.indices));
        let render = gfx::state::RenderState::default();
        let packets = draws
            .into_iter()
            .map(|d| {
                let key = PipelineKey {
                    shader: ShaderId(0),
                    vertex_buffers: Vec::new(),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    color_target: wgpu::ColorTargetState {
                        format,
                        blend: d.blend,
                        write_mask: d.write_mask,
                    },
                    samples: 1,
                };
                let mut groups = vec![BoundGroup::new(0, d.group)];
                groups.extend(d.stages.map(|stages| BoundGroup::new(1, stages)));
                let first = u64::from(d.indices.start) * 4;
                let count = d.indices.end - d.indices.start;
                let s = d.scissor;
                RecordedDraw::prepared_indexed(
                    &key,
                    Pending::ready(Some(d.pipeline)),
                    [(vertices.clone(), vertex_offset, vertex_len)],
                    (indices.clone(), index_offset + first, u64::from(count) * 4),
                    count,
                    groups,
                    &render,
                    size,
                    gfx::ScissorRect { x: s.x, y: s.y, width: s.width, height: s.height },
                )
                .with_blend_constant(d.blend_constant)
            })
            .collect();
        Ok((packets, stats))
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        target: &wgpu::TextureView,
        plan: &crate::overlay::OverlayPlan,
        textures: &dyn OverlayTextures,
    ) -> Result<(ExternalEncoded, crate::overlay::OverlayStats), &'static str> {
        self.prepare_with_objects(device, target, plan, textures, &mut NoObjects)
    }

    /// [`Self::prepare`] with `objects`, the stage that draws the plan's
    /// model views' objects ([`ModelViewObjects`]): each view one pass of
    /// its own over the target and the overlay's depth target
    /// ([`Self::depth_target`]), cleared to the far plane, in its place
    /// among the batches, holds and blurs.
    pub fn prepare_with_objects(
        &mut self,
        device: &wgpu::Device,
        target: &wgpu::TextureView,
        plan: &crate::overlay::OverlayPlan,
        textures: &dyn OverlayTextures,
        objects: &mut dyn ModelViewObjects,
    ) -> Result<(ExternalEncoded, crate::overlay::OverlayStats), &'static str> {
        let size = target.texture().size();
        if [size.width, size.height] != plan.target {
            return Err("overlay target size changed");
        }
        let format = target.texture().format();
        let mut stats = plan.stats;
        let textures = Holding::new(textures, plan, size, format);
        let draws = self.batch_draws(device, format, plan, &textures, &mut stats);
        // A hold copies the whole target into its texture of the target's
        // size and format; a target that cannot be copied from, or no
        // such texture, refuses it.
        let copyable = target.texture().usage().contains(wgpu::TextureUsages::COPY_SRC)
            && target.texture().sample_count() == 1;
        let mut holds = Vec::with_capacity(plan.holds.len());
        for hold in &plan.holds {
            match textures.held.get(&hold.hold).filter(|_| copyable) {
                Some(view) => holds.push((hold.before as usize, view.clone())),
                None => stats.refused_hold += 1,
            }
        }
        let mut model_views = Vec::with_capacity(plan.model_views.len());
        for view in &plan.model_views {
            match model_view_packets(objects, view, plan.target, format) {
                Ok(packets) => model_views.push(PreparedView {
                    before: view.before as usize,
                    blurs: view.blurs as usize,
                    packets,
                }),
                Err(why) => unimplemented!("overlay model view: {why}"),
            }
        }
        let depth = (!model_views.is_empty()).then(|| self.depth_target(device, size));
        let blurs = if plan.blurs.is_empty() {
            None
        } else {
            let prepared = self.prepare_blurs(device, target, plan);
            if prepared.is_none() {
                stats.refused_blur += plan.blurs.len() as u32;
            }
            prepared
        };
        if draws.is_empty() && blurs.is_none() && holds.is_empty() && model_views.is_empty() {
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
        // quad over the whole target, every channel written as the guest's
        // blur quads write them: its four corners and six indices follow
        // the plan's.
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
                color_write: frame_scene::OverlayItem::RGB_WRITE | (1 << 24),
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
        let (width, height) = (size.width as f32, size.height as f32);
        let steps: usize = blurs.iter().map(|b| b.steps.len()).sum();
        let count = draws.len() + steps + blurs.len() + model_views.iter().map(|v| v.packets.len()).sum::<usize>();
        let passes = 1 + holds.len() + blurs.iter().map(|b| b.steps.len() + 2).sum::<usize>() + 2 * model_views.len();
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
                    // A hold copies what the batches before it drew; at a
                    // blur's or a model view's point it runs first.
                    let mut held = holds.iter().peekable();
                    let hold = |encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView| {
                        encoder.copy_texture_to_texture(
                            target.texture().as_image_copy(),
                            view.texture().as_image_copy(),
                            target.texture().size(),
                        );
                    };
                    // A model view's objects: a pass of their own over the
                    // target and the depth, cleared, which the overlay's
                    // own items never test.
                    let view_pass = |encoder: &mut wgpu::CommandEncoder, view: &PreparedView| {
                        let Some(depth) = depth.as_ref() else { return };
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("native overlay model view"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &target,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                            })],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: depth,
                                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                            }),
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        pass.set_viewport(0.0, 0.0, width, height, 0.0, 1.0);
                        // Checked when the packets were made: every
                        // pipeline built, none counted.
                        let _ = gfx::renderer::encode_in_pass(&mut pass, &view.packets);
                    };
                    let mut model = model_views.iter().peekable();
                    for (index, blur) in blurs.iter().enumerate() {
                        let (Some(views), Some((copy, step))) = (scratch.as_ref(), write_back.as_ref())
                        else {
                            break;
                        };
                        // The views before this blur in the list.
                        while let Some(view) = model.next_if(|v| v.blurs <= index) {
                            while let Some((before, h)) = held.next_if(|(before, _)| *before <= view.before) {
                                segment(encoder, *before, &mut next);
                                hold(encoder, h);
                            }
                            segment(encoder, view.before, &mut next);
                            view_pass(encoder, view);
                        }
                        while let Some((before, view)) = held.next_if(|(before, _)| *before <= blur.before) {
                            segment(encoder, *before, &mut next);
                            hold(encoder, view);
                        }
                        segment(encoder, blur.before, &mut next);
                        encoder.copy_texture_to_texture(
                            target.texture().as_image_copy(),
                            views[0].texture().as_image_copy(),
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
                    for view in model {
                        while let Some((before, h)) = held.next_if(|(before, _)| *before <= view.before) {
                            segment(encoder, *before, &mut next);
                            hold(encoder, h);
                        }
                        segment(encoder, view.before, &mut next);
                        view_pass(encoder, view);
                    }
                    for (before, view) in held {
                        segment(encoder, *before, &mut next);
                        hold(encoder, view);
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
/// written once, by its rectangle ([`crate::overlay::GlyphPages`]); and
/// each movie's current frame, written when the frame moves
/// ([`OverlayResidency::sync_movie`]).
#[derive(Default)]
pub struct OverlayResidency {
    /// The assets' generation what it holds came from.
    generation: u64,
    images: HashMap<(u32, u16), wgpu::TextureView>,
    /// Each font's page: the version it holds, its texture and view.
    glyphs: HashMap<u16, (u64, wgpu::Texture, wgpu::TextureView)>,
    /// Each movie's frame: the frame it holds, its texture and view. Not
    /// a map's: another generation keeps them.
    movies: HashMap<u32, (u32, wgpu::Texture, wgpu::TextureView)>,
    /// Each hold's kept target, by hold and the target's size and format
    /// (a target's reference and display copies are two), oldest first.
    /// Not a map's either.
    holds: Vec<(u32, wgpu::Extent3d, wgpu::TextureFormat, wgpu::TextureView)>,
}

/// Held textures kept at once: one hold at two copies' sizes, and room
/// for a size change.
const HOLDS: usize = 4;

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

impl OverlayResidency {
    /// Writes `movie`'s frame when it is not the one held: a texture of
    /// the frame's size (`Bgra8Unorm`, the decoder's byte order), made
    /// again when the size changes. False, and nothing held for the
    /// movie, when the frame has no texels, fewer than its size needs, or
    /// is past the device's largest texture: its items are refused
    /// (`refused_movie`).
    pub fn sync_movie(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        movie: &crate::overlay::MovieImage,
    ) -> bool {
        let (w, h) = (movie.width, movie.height);
        let largest = device.limits().max_texture_dimension_2d;
        if w == 0 || h == 0 || w > largest || h > largest || movie.bgra.len() < (w * h * 4) as usize {
            self.movies.remove(&movie.movie);
            return false;
        }
        if let Some((frame, texture, _)) = self.movies.get_mut(&movie.movie)
            && texture.size().width == w
            && texture.size().height == h
        {
            if *frame != movie.frame {
                write_rgba(queue, texture, &movie.bgra[..(w * h * 4) as usize]);
                *frame = movie.frame;
            }
            return true;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native overlay movie"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_rgba(queue, &texture, &movie.bgra[..(w * h * 4) as usize]);
        let view = texture.create_view(&Default::default());
        self.movies.insert(movie.movie, (movie.frame, texture, view));
        true
    }
}

impl OverlayResidency {
    /// Makes sure hold `hold` has a texture for a target of `size` and
    /// `format` to copy into, made the first time (the copy fills it);
    /// what it holds stays across frames until the next hold of it. The
    /// oldest is dropped past [`HOLDS`]. False when the size is empty or
    /// past the device's largest texture.
    pub fn hold(&mut self, device: &wgpu::Device, hold: u32, size: wgpu::Extent3d, format: wgpu::TextureFormat) -> bool {
        let largest = device.limits().max_texture_dimension_2d;
        if size.width == 0 || size.height == 0 || size.width > largest || size.height > largest {
            return false;
        }
        let size = wgpu::Extent3d { depth_or_array_layers: 1, ..size };
        if self.holds.iter().any(|(h, s, f, _)| (*h, *s, *f) == (hold, size, format)) {
            return true;
        }
        if self.holds.len() == HOLDS {
            self.holds.remove(0);
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native overlay hold"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.holds.push((hold, size, format, texture.create_view(&Default::default())));
        true
    }
}

impl OverlayTextures for OverlayResidency {
    fn held(&self, hold: u32, size: wgpu::Extent3d, format: wgpu::TextureFormat) -> Option<&wgpu::TextureView> {
        let size = wgpu::Extent3d { depth_or_array_layers: 1, ..size };
        self.holds.iter().find(|(h, s, f, _)| (*h, *s, *f) == (hold, size, format)).map(|(_, _, _, view)| view)
    }

    fn view(&self, texture: frame_scene::OverlayTexture) -> Option<&wgpu::TextureView> {
        match texture {
            frame_scene::OverlayTexture::None | frame_scene::OverlayTexture::Bound { .. } => None,
            // A held texture is a target size's ([`OverlayTextures::held`]).
            frame_scene::OverlayTexture::Held { .. } => None,
            frame_scene::OverlayTexture::Bitmap { tag, image } => self.images.get(&(tag, image)),
            frame_scene::OverlayTexture::Glyphs { font } => self.glyphs.get(&font).map(|(_, _, view)| view),
            frame_scene::OverlayTexture::Movie { movie } => self.movies.get(&movie).map(|(_, _, view)| view),
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
        (Ui::OverlayRefusedBlur, stats.refused_blur),
        (Ui::OverlayMovieFrames, stats.movie_frames),
        (Ui::OverlayRefusedMovie, stats.refused_movie),
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

    /// The depth target is the target's size in the overlay's depth
    /// format, kept for each copy's size and remade for a new one.
    #[test]
    fn the_depth_target_is_kept_for_each_copys_size() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let size = |width, height| wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let guest = gpu.depth_target(&ctx.device, size(640, 480));
        assert_eq!((guest.texture().size(), guest.texture().format()), (size(640, 480), OVERLAY_DEPTH));
        assert!(guest.texture().usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT));
        let display = gpu.depth_target(&ctx.device, size(1280, 960));
        assert_eq!(display.texture().size(), size(1280, 960));
        assert!(gpu.depth_target(&ctx.device, size(640, 480)).texture() == guest.texture(), "the guest copy's is kept");
        assert!(gpu.depth_target(&ctx.device, size(1280, 960)).texture() == display.texture(), "and the display's");
        gpu.depth_target(&ctx.device, size(1920, 1080));
        assert!(gpu.depth_target(&ctx.device, size(640, 480)).texture() != guest.texture(), "a third size takes the oldest's place");
    }

    /// A list with one model view between two fills: the view and its
    /// constants as the overlay's stage hands them to the objects, its
    /// pass over the depth between the fills' passes.
    fn model_view_list() -> frame_scene::Overlay {
        use frame_scene::{ClipRect, Overlay, OverlayBlend, OverlayCamera, OverlayModelView};
        let mut overlay = Overlay::new([640, 480]);
        overlay.push_fill(ClipRect::full([640, 480]), [0.0, 0.0, 640.0, 480.0], [1.0, 0.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        let camera = OverlayCamera {
            position: [0.0, 1.0, 2.0],
            forward: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov_y: 0.75,
            planes: [0.0625, 200.0],
            projection: None,
        };
        let view = OverlayModelView { camera, objects: vec![42, 17] };
        overlay.push_model_view(ClipRect { x: 310, y: 228, width: 20, height: 24 }, view).unwrap();
        overlay.push_fill(ClipRect::full([640, 480]), [0.0, 0.0, 8.0, 8.0], [0.0, 1.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        overlay
    }

    #[test]
    fn a_model_view_hands_its_objects_its_view_and_draws_over_the_depth() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        #[derive(Default)]
        struct Seen(Vec<(Vec<u32>, crate::overlay::Scissor, crate::program::ViewInputs, wgpu::TextureFormat, (u32, u32))>);
        impl ModelViewObjects for Seen {
            fn draws(
                &mut self,
                view: &crate::model_view::PlanModelView,
                inputs: &crate::program::ViewInputs,
                color: wgpu::TextureFormat,
                size: (u32, u32),
            ) -> Result<Vec<RecordedDraw>, &'static str> {
                self.0.push((view.objects.clone(), view.scissor, *inputs, color, size));
                Ok(Vec::new())
            }
        }
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test overlay"),
            size: wgpu::Extent3d { width: 1280, height: 960, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let plan = crate::overlay::OverlayPlan::new(&model_view_list(), [1280, 960], &crate::overlay::NoGlyphs);
        assert_eq!((plan.batches.len(), plan.model_views.len()), (2, 1));
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let mut seen = Seen::default();
        let (encoded, _) = gpu.prepare_with_objects(&ctx.device, &view, &plan, &NoTextures, &mut seen).unwrap();
        let [(objects, scissor, inputs, color, size)] = seen.0.as_slice() else { panic!("one view: {}", seen.0.len()) };
        assert_eq!(objects, &[42, 17]);
        assert_eq!(*scissor, crate::overlay::Scissor { x: 620, y: 456, width: 40, height: 48 });
        let planned = &plan.model_views[0];
        assert_eq!(*inputs, crate::model_view::view_inputs(&planned.camera, [640, 480], [1280, 960]), "the view's own constants");
        assert_eq!((*color, *size), (wgpu::TextureFormat::Rgba8Unorm, (1280, 960)));
        assert_eq!(encoded.passes, 3, "the overlay's pass, the view's, and the overlay's after it");
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (encoded.commands)(&mut encoder);
        ctx.queue.submit([encoder.finish()]);
        assert!(pollster::block_on(scope.pop()).is_none(), "the view's pass over the depth target records");
        let depth = gpu.depth_target(&ctx.device, target.size());
        assert_eq!(depth.texture().format(), OVERLAY_DEPTH);
    }

    #[test]
    #[should_panic(expected = "overlay model view: no object stage")]
    fn a_model_view_with_no_object_stage_is_unimplemented() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            panic!("overlay model view: no object stage (no GPU here)");
        };
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test overlay"),
            size: wgpu::Extent3d { width: 640, height: 480, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let plan = crate::overlay::OverlayPlan::new(&model_view_list(), [640, 480], &crate::overlay::NoGlyphs);
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let _ = gpu.prepare(&ctx.device, &target.create_view(&Default::default()), &plan, &NoTextures);
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
        // The two text runs share their material, clip and indices: one batch.
        assert_eq!((stats.batches, stats.refused_glyph), (2, 0));
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

    /// A movie frame draws its decoder's B, G, R, A texels as colour, is
    /// written again only when its frame moves, and keeps across a map
    /// change; a movie with no upload is refused by name.
    #[test]
    fn a_movie_frame_draws_its_upload_and_follows_its_frame() {
        use frame_scene::{ClipRect, Combiner, Overlay, OverlayBlend, OverlayMaterial, OverlayMovie, OverlayTexture, OverlayVertex, Program, Sampler};
        use crate::overlay::{MovieImage, OverlayAssets, OverlayPlan};

        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test movie overlay"),
            size: wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let material = OverlayMaterial {
            program: Program::Fixed(Combiner::Texture),
            texture: OverlayTexture::None,
            sampler: Sampler { wrap: false, point: true },
            blend: OverlayBlend::Off,
        };
        let corner = |x, y, u, v| OverlayVertex { position: [x, y], uv: [u, v], color: [1.0; 4] };
        let all = [corner(0.0, 0.0, 0.0, 0.0), corner(2.0, 0.0, 1.0, 0.0), corner(2.0, 2.0, 1.0, 1.0), corner(0.0, 2.0, 0.0, 1.0)];
        let mut overlay = Overlay::new([2, 2]);
        overlay.push_movie_frame(material, ClipRect::full([2, 2]), all, OverlayMovie { movie: 5, frame: 1 }).unwrap();
        let plan = OverlayPlan::new(&overlay, [2, 2], &crate::overlay::NoGlyphs);
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let mut residency = OverlayResidency::default();
        let (_, stats) = gpu.prepare(&ctx.device, &view, &plan, &residency).unwrap();
        assert_eq!((stats.batches, stats.refused_movie, stats.refused_texture), (0, 1, 0), "no upload: refused as a movie");
        // Two texels a row: blue then red, as B, G, R, A.
        let frame = |n: u32, left: [u8; 4], right: [u8; 4]| MovieImage {
            movie: 5, frame: n, width: 2, height: 1,
            bgra: [left, right].concat().into(),
        };
        let draw = |gpu: &mut OverlayGpu, residency: &OverlayResidency| {
            let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, residency).unwrap();
            assert_eq!((stats.batches, stats.refused_movie, stats.movie_frames), (1, 0, 1));
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            (encoded.commands)(&mut encoder);
            ctx.queue.submit([encoder.finish()]);
            let pixels = gfx::readback::read_texture(&ctx, &target, wgpu::TextureAspect::All, 4, (2, 2));
            [[pixels[0], pixels[1], pixels[2]], [pixels[4], pixels[5], pixels[6]]]
        };
        assert!(residency.sync_movie(&ctx.device, &ctx.queue, &frame(1, [255, 0, 0, 255], [0, 0, 255, 255])));
        assert_eq!(draw(&mut gpu, &residency), [[0, 0, 255], [255, 0, 0]]);
        // The same frame number is not written again; the next one is.
        assert!(residency.sync_movie(&ctx.device, &ctx.queue, &frame(1, [0, 255, 0, 255], [0, 255, 0, 255])));
        assert_eq!(draw(&mut gpu, &residency), [[0, 0, 255], [255, 0, 0]]);
        assert!(residency.sync_movie(&ctx.device, &ctx.queue, &frame(2, [0, 255, 0, 255], [0, 255, 0, 255])));
        assert_eq!(draw(&mut gpu, &residency), [[0, 255, 0], [0, 255, 0]]);
        // Movies are not a map's: another generation keeps them.
        let next = OverlayAssets { generation: 3, ..OverlayAssets::default() };
        residency.sync(&ctx.device, &ctx.queue, &next);
        assert!(residency.view(OverlayTexture::Movie { movie: 5 }).is_some());
        // Too few texels drop the movie.
        let short = MovieImage { bgra: vec![0; 4].into(), ..frame(3, [0; 4], [0; 4]) };
        assert!(!residency.sync_movie(&ctx.device, &ctx.queue, &short));
        assert!(residency.view(OverlayTexture::Movie { movie: 5 }).is_none());
    }

    /// A hold keeps the target as the batches before it drew it, and a
    /// later frame with no hold still samples it (the frame tail's
    /// freeze frame, docs/graphics/retire-d3d-display.md, "The frame
    /// tail"). Frame 1: red, hold, green, then the held texture over the
    /// right half; frame 2: blue, then the held texture over the whole.
    #[test]
    fn a_held_texture_survives_into_the_next_frame() {
        use frame_scene::{ClipRect, Combiner, Overlay, OverlayBlend, OverlayMaterial, OverlayTexture, OverlayVertex, Program, Sampler};
        use crate::overlay::{NoGlyphs, OverlayPlan};

        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let size = wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 1 };
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test overlay hold"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let all = ClipRect::full([2, 2]);
        let held = OverlayMaterial {
            program: Program::Fixed(Combiner::Texture),
            texture: OverlayTexture::Held { hold: 1 },
            sampler: Sampler { wrap: false, point: true },
            blend: OverlayBlend::Off,
        };
        let corner = |x: f32, y: f32| OverlayVertex { position: [x, y], uv: [x / 2.0, y / 2.0], color: [1.0; 4] };
        let rect = |x0: f32, x1: f32| [corner(x0, 0.0), corner(x1, 0.0), corner(x1, 2.0), corner(x0, 2.0)];
        let mut first = Overlay::new([2, 2]);
        first.push_fill(all, [0.0, 0.0, 2.0, 2.0], [1.0, 0.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        first.push_hold(1).unwrap();
        first.push_fill(all, [0.0, 0.0, 2.0, 2.0], [0.0, 1.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        first.push_quad(held, all, rect(1.0, 2.0)).unwrap();
        let mut second = Overlay::new([2, 2]);
        second.push_fill(all, [0.0, 0.0, 2.0, 2.0], [0.0, 0.0, 1.0, 1.0], OverlayBlend::Off).unwrap();
        second.push_quad(held, all, rect(0.0, 2.0)).unwrap();
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let mut residency = OverlayResidency::default();
        let draw = |gpu: &mut OverlayGpu, residency: &OverlayResidency, overlay: &Overlay| {
            let plan = OverlayPlan::new(overlay, [2, 2], &NoGlyphs);
            let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, residency).unwrap();
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            (encoded.commands)(&mut encoder);
            ctx.queue.submit([encoder.finish()]);
            let pixels = gfx::readback::read_texture(&ctx, &target, wgpu::TextureAspect::All, 4, (2, 2));
            (stats, [[pixels[0], pixels[1], pixels[2]], [pixels[4], pixels[5], pixels[6]]])
        };
        // No texture for the hold: refused, and its quad with it.
        let (stats, _) = draw(&mut gpu, &residency, &first);
        assert_eq!((stats.holds, stats.refused_hold, stats.refused_texture), (1, 1, 1));
        assert!(residency.hold(&ctx.device, 1, size, wgpu::TextureFormat::Rgba8Unorm));
        assert!(residency.held(1, size, wgpu::TextureFormat::Bgra8Unorm).is_none(), "another format holds apart");
        let (stats, row) = draw(&mut gpu, &residency, &first);
        assert_eq!((stats.refused_hold, stats.refused_texture), (0, 0));
        assert_eq!(row, [[0, 255, 0], [255, 0, 0]], "green, then the red the hold kept");
        let (stats, row) = draw(&mut gpu, &residency, &second);
        assert_eq!((stats.holds, stats.refused_texture), (0, 0));
        assert_eq!(row, [[255, 0, 0], [255, 0, 0]], "the next frame samples what the hold kept");
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
        overlay.push_blur(right, OverlayBlur { tap: 0.5, spreads: vec![1.0], pick: 0.0 }).unwrap();
        overlay.push_fill(all, [3.0, 0.0, 4.0, 4.0], [0.0, 1.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        let plan = crate::overlay::OverlayPlan::new(&overlay, [4, 4], &crate::overlay::NoGlyphs);
        assert_eq!((plan.batches.len(), plan.blurs.len()), (2, 1));
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);

        let texture = target(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let view = texture.create_view(&Default::default());
        let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, &NoTextures).unwrap();
        let scratch = gpu.blur_scratch[0].clone();
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

        // The same blur with the combiner leaning all the way: each pair
        // takes its brighter tap (0.5 over 0), so the pixel keeps half
        // white where the mean gave a quarter.
        let mut leaning = overlay.clone();
        leaning.blurs[0].pick = 1.0;
        let plan = crate::overlay::OverlayPlan::new(&leaning, [4, 4], &crate::overlay::NoGlyphs);
        let texture = target(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let view = texture.create_view(&Default::default());
        let (encoded, _) = gpu.prepare(&ctx.device, &view, &plan, &NoTextures).unwrap();
        assert_eq!(gpu.blur_scratch.as_slice(), &[scratch]);
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (encoded.commands)(&mut encoder);
        ctx.queue.submit([encoder.finish()]);
        let pixels = gfx::readback::read_texture(&ctx, &texture, wgpu::TextureAspect::All, 4, (4, 4));
        for y in 0..4 {
            let i = 4 * (y * 4 + 2);
            let leant = [pixels[i], pixels[i + 1], pixels[i + 2]];
            assert!(leant.iter().all(|c| c.abs_diff(128) <= 1), "row {y}: leaning blur {leant:?}");
        }
    }

    /// The step's combiner on the CPU, as the WGSL computes it: a pick of
    /// 0 is the mean of the four taps; a positive one leans each pair to
    /// its brighter tap, by at most half of the pair's spread.
    #[test]
    fn the_blur_combiner_leans_to_the_brighter_tap_by_its_constant() {
        fn pick(a: f32, b: f32, c: f32) -> f32 {
            let d = (4.0 * (a - b)).clamp(-1.0, 1.0);
            let w = ((d * c + 1.0) * 0.5).clamp(0.0, 1.0);
            (w * a + (1.0 - w) * b).clamp(-1.0, 1.0)
        }
        let step = |t: [f32; 4], c: f32| pick(pick(t[0], t[1], c).clamp(0.0, 1.0), pick(t[2], t[3], c).clamp(0.0, 1.0), c);
        let taps = [0.5, 0.0, 0.25, 0.25];
        assert!((step(taps, 0.0) - 0.25).abs() < 1e-6, "the plain mean");
        // 64 over 255, the guest's 0.25 packed: the first pair leans by 64/255 of half its spread.
        let c = 64.0 / 255.0;
        let first = 0.5 * (0.5 + 0.5 * c) + 0.0 * (0.5 - 0.5 * c);
        assert!((pick(0.5, 0.0, c) - first).abs() < 1e-6);
        assert!(step(taps, c) > 0.25 && step(taps, c) < step(taps, 1.0));
        assert_eq!(pick(0.3, 0.3, 1.0), 0.3, "equal taps are unchanged");
        assert!(OVERLAY_BLUR_WGSL.contains("fn pick("));
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
