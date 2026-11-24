//! Native UI packets over the device's pinned attachments. The guest's
//! shader programs and raster state are shared inputs with the D3D renderer.
//!
//! [`OverlayGpu`] draws the scene's overlay stage (`frame_scene::overlay`,
//! planned by [`crate::overlay`]) with named programs for the fixed
//! combiners, over no guest state at all.
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

/// The textures an overlay names, as the renderer holds them.
pub trait OverlayTextures {
    /// A 2D, filterable view of `texture`; `None` when it is not resident.
    fn view(&self, texture: frame_scene::OverlayTexture) -> Option<&wgpu::TextureView>;
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct OverlayPipelineKey {
    combiner: frame_scene::Combiner,
    blend: frame_scene::OverlayBlend,
    format: wgpu::TextureFormat,
}

/// The overlay stage's programs, pipelines and samplers on one device.
pub struct OverlayGpu {
    module: wgpu::ShaderModule,
    group_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<OverlayPipelineKey, wgpu::RenderPipeline>,
    samplers: HashMap<frame_scene::Sampler, wgpu::Sampler>,
    white: wgpu::TextureView,
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
        OverlayGpu {
            module,
            group_layout,
            layout,
            pipelines: HashMap::new(),
            samplers: HashMap::new(),
            white: white.create_view(&Default::default()),
        }
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
            let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
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
                    // The screen-quad begin writes colour without alpha.
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.format,
                        blend,
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
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
    /// the draws recorded. The plan's own counts are carried over.
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
            pipeline: wgpu::RenderPipeline,
            group: wgpu::BindGroup,
            scissor: crate::overlay::Scissor,
            indices: std::ops::Range<u32>,
        }
        let mut draws = Vec::with_capacity(plan.batches.len());
        for batch in &plan.batches {
            let frame_scene::Program::Fixed(combiner) = batch.material.program else {
                stats.refused_material += batch.items;
                continue;
            };
            let Ok(blend) = overlay_blend(batch.material.blend) else {
                stats.refused_texture += batch.items;
                continue;
            };
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
            let sampler = self.sampler(device, batch.material.sampler);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native overlay stage"),
                layout: &self.group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            });
            let key = OverlayPipelineKey { combiner, blend: batch.material.blend, format };
            draws.push(Draw {
                pipeline: self.pipeline(device, key, blend),
                group,
                scissor: batch.scissor,
                indices: batch.first_index..batch.first_index + batch.index_count,
            });
        }
        stats.batches = draws.len() as u32;
        if draws.is_empty() {
            return Ok((
                ExternalEncoded { ranges: 0, draws: 0, passes: 0, commands: Box::new(|_| {}) },
                stats,
            ));
        }
        use wgpu::util::DeviceExt;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native overlay vertices"),
            contents: bytemuck::cast_slice(&plan.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native overlay indices"),
            contents: bytemuck::cast_slice(&plan.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let target = target.clone();
        let (width, height) = (size.width as f32, size.height as f32);
        let count = draws.len();
        Ok((
            ExternalEncoded {
                ranges: plan.batches.len(),
                draws: count,
                passes: 1,
                commands: Box::new(move |encoder| {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("native overlay"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_viewport(0.0, 0.0, width, height, 0.0, 1.0);
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    for d in &draws {
                        pass.set_pipeline(&d.pipeline);
                        pass.set_bind_group(0, &d.group, &[]);
                        pass.set_scissor_rect(d.scissor.x, d.scissor.y, d.scissor.width, d.scissor.height);
                        pass.draw_indexed(d.indices.clone(), 0, 0..1);
                    }
                }),
            },
            stats,
        ))
    }
}

/// The textures an overlay names, resident on one device: each bitmap
/// image uploaded once, each font's glyph page uploaded when it first
/// appears and written again when its version moves
/// ([`crate::overlay::GlyphPages`]).
#[derive(Default)]
pub struct OverlayResidency {
    /// The assets' generation what it holds came from.
    generation: u64,
    images: HashMap<(u32, u16), wgpu::TextureView>,
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

impl OverlayResidency {
    /// Uploads what `assets` holds that is not resident, or has changed:
    /// how many textures it wrote. Assets of another generation (another
    /// map) replace all it holds. An image with no texels, or fewer than
    /// its size needs, or past the device's largest texture, stays out
    /// (its batches are refused as not resident).
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
                    write_rgba(queue, texture, rgba);
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
            frame_scene::OverlayTexture::None => None,
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
        let plan = crate::overlay::OverlayPlan::new(&overlay, [4, 4], &crate::overlay::NoGlyphs);
        assert_eq!(plan.batches.len(), 3);
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let view = target.create_view(&Default::default());
        let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, &textures).unwrap();
        assert_eq!((stats.batches, stats.refused_texture, stats.items), (2, 1, 3));
        assert_eq!((encoded.draws, encoded.passes), (2, 1));
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (encoded.commands)(&mut encoder);
        ctx.queue.submit([encoder.finish()]);
        let pixels = gfx::readback::read_texture(&ctx, &target, wgpu::TextureAspect::All, 4, (4, 4));
        let rgb = |x: usize, y: usize| {
            let i = 4 * (y * 4 + x);
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        for y in 0..4 {
            assert_eq!(rgb(0, y), [255, 0, 0], "row {y}: the fill");
            assert_eq!(rgb(1, y), [255, 0, 0], "row {y}: left of the clip");
            assert_eq!(rgb(2, y), [0, 255, 0], "row {y}: the sprite");
            assert_eq!(rgb(3, y), [0, 255, 0], "row {y}: the sprite");
        }
        assert!(pixels.chunks(4).all(|p| p[3] == 0), "colour writes leave alpha alone");
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
        // A grown page is written again; another map's assets start over.
        assert!(assets.glyphs.insert(6, 4, 1, 1, &[9]));
        assert_eq!(residency.sync(&ctx.device, &ctx.queue, &assets), 1);
        let next = OverlayAssets { generation: 1, ..OverlayAssets::default() };
        assert_eq!(residency.sync(&ctx.device, &ctx.queue, &next), 0);
        assert!(residency.view(OverlayTexture::Bitmap { tag: 7, image: 1 }).is_none());
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
