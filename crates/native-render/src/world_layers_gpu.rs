//! Records opaque indexed geometry from an ordered, resolved draw. The
//! guest has already selected the live shader, streams and attachments.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalDraws, ExternalPass, ExternalPassRequest, ExternalTarget,
    ExternalWrites, PixelRect, ResolvedDraw,
};
use d3d::nv2a::{NativeDrawKey, NativeDrawMaterial};
use gfx::compile::{Pending, PendingPipeline};
use gfx::pipeline::{PipelineKey, ShaderId, ShaderProgram, TargetFormats, create_render_pipeline};
use gfx::program::{ProgramBindings, StageDims};
use gfx::repack::{FetchMode, RepackPlan, StreamPlan};
use gfx::state::{DepthBufferMode, StencilOp};
use gfx::texture::TextureKind;
use gfx::translate::Capabilities;
use gfx::{BoundGroup, RecordedDraw, ScissorRect};
use gfx::vertex::PrimitiveType;
use shaders::{psh::PixelUniforms, vsh::VertexUniforms};

use crate::world_layers::{Geometry, Owner, Texaccum};

const CACHE_BUDGET: u64 = 64 * 1024 * 1024;

pub struct Cache {
    devices: Mutex<HashMap<wgpu::Device, DeviceCache>>,
    fetch: FetchMode,
}

impl Default for Cache {
    fn default() -> Self {
        Self { devices: Mutex::default(), fetch: FetchMode::Gpu }
    }
}

struct DeviceCache {
    bindings: ProgramBindings,
    shaders: HashMap<NativeDrawKey, Option<Program>>,
    pipelines: HashMap<PipelineKey, Arc<PendingPipeline>>,
    buffers: HashMap<BufferKey, (wgpu::Buffer, u64)>,
    buffer_bytes: u64,
    frame: u64,
    scope: Option<(u64, u32)>,
    sections: HashMap<SectionKey, Vec<ResidentSection>>,
}

#[derive(Clone)]
struct Program {
    id: ShaderId,
    shader: ShaderProgram,
    dims: StageDims,
    discards: bool,
}

type BufferRange = (wgpu::Buffer, u64, u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum BufferKey {
    RawVertex(u64, u32, u32),
    RepackedVertex(u64, u32, StreamPlan),
    Indices(u64),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum SectionOwner {
    Cluster(u32),
    Placement(u32),
    Model(u32, u32),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct SectionKey {
    owner: SectionOwner,
    part: u8,
    shader: u32,
    variant: u8,
    implementation: u8,
    layer: u32,
}

impl SectionKey {
    fn new(geometry: Geometry, layer: u32) -> Self {
        let owner = match geometry.owner {
            Owner::Cluster(i) => SectionOwner::Cluster(i),
            Owner::Placement(i) => SectionOwner::Placement(i),
            Owner::Object { model, section, .. } => SectionOwner::Model(model, section),
        };
        Self { owner, part: geometry.part, shader: geometry.shader, variant: geometry.variant,
            implementation: geometry.implementation, layer }
    }
}

struct ResidentSection {
    last_used: u64,
    source: Arc<ResolvedDraw>,
    fetch: FetchMode,
    repack: RepackPlan,
    streams: Vec<BufferRange>,
    indices: BufferRange,
    material: Option<ResidentMaterial>,
}

struct ResidentMaterial {
    source: Arc<ResolvedDraw>,
    present: [bool; 4],
    program: Arc<NativeDrawMaterial>,
    shader: Program,
    // Pin the material/lightmap bind group for each live target copy.
    groups: Vec<([Option<wgpu::TextureView>; 4], Option<wgpu::BindGroup>)>,
    // Reference and display may require different attachment formats.
    pipelines: HashMap<TargetFormats, (PipelineKey, Arc<PendingPipeline>)>,
}

impl ResidentSection {
    fn matches(&self, draw: &ResolvedDraw, fetch: FetchMode) -> bool {
        let old = &self.source;
        let same_snapshot = |a: Option<u64>, b| a.is_some() && a == b;
        if std::ptr::eq(self.source.as_ref(), draw) { return self.fetch == fetch; }
        self.fetch == fetch && old.vertex_count == draw.vertex_count && old.layout == draw.layout
            && old.streams.len() == draw.streams.len()
            && old.streams.iter().zip(&draw.streams).all(|(a, b)| a.stride == b.stride
                && (same_snapshot(a.data.cache_id(), b.data.cache_id()) || a.data == b.data))
            && old.indices.as_ref().zip(draw.indices.as_ref()).is_some_and(|(a, b)|
                same_snapshot(a.cache_id(), b.cache_id()) || a == b)
    }
}

impl ResidentMaterial {
    // All shader and pipeline selection inputs. Vertex registers and stage
    // views are supplied live, including relocated lightmaps and texaccum.
    fn matches(&self, draw: &ResolvedDraw, present: [bool; 4]) -> bool {
        let old = &self.source;
        let (a, b) = (&old.shader, &draw.shader);
        self.present == present && old.primitive == draw.primitive && old.raw == draw.raw
            && old.state == draw.state && old.extras == draw.extras
            && d3d::nv2a::w_buffered(old) == d3d::nv2a::w_buffered(draw)
            && old.hw.stipple_control == draw.hw.stipple_control
            && old.texture_info == draw.texture_info && old.target == draw.target
            && old.glyph_cache == draw.glyph_cache && a.vertex == b.vertex
            && a.provided_inputs == b.provided_inputs && a.pixel_shader == b.pixel_shader
            && a.combiner_state == b.combiner_state && a.final_combiner_latch == b.final_combiner_latch
            && a.affine_projection == b.affine_projection
    }
}

impl BufferKey {
    fn vertex(id: u64, plan: &StreamPlan, count: u32) -> Self {
        if plan.identity {
            Self::RawVertex(id, plan.guest_stride, count)
        } else {
            Self::RepackedVertex(id, count, plan.clone())
        }
    }
}

impl DeviceCache {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            bindings: ProgramBindings::new(device),
            shaders: HashMap::new(),
            pipelines: HashMap::new(),
            buffers: HashMap::new(),
            buffer_bytes: 0,
            frame: 0, scope: None, sections: HashMap::new(),
        }
    }

    fn resolve_shader(
        &mut self, device: &wgpu::Device, draw: &ResolvedDraw, present: [bool; 4],
        fetch: FetchMode, caps: Capabilities,
    ) -> Option<(Arc<NativeDrawMaterial>, Program)> {
        let program = Arc::new(NativeDrawMaterial::resolve(draw, present, caps, fetch).ok()?);
        let new_id = ShaderId(self.shaders.len().try_into().ok()?);
        let shader = self
            .shaders
            .entry(program.key.clone())
            .or_insert_with(|| {
                let linked = program.key.translate().ok()?;
                let module = gfx::program::validated_resource(device, || {
                    device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("opaque indexed program"),
                        source: wgpu::ShaderSource::Wgsl(linked.source.into()),
                    })
                })?;
                Some(Program {
                    id: new_id,
                    shader: self.bindings.program(device, module, linked.dims),
                    dims: linked.dims,
                    discards: linked.discards,
                })
            })
            .clone()?;
        Some((program, shader))
    }

    fn upload_section(
        &mut self, target: &ExternalAttachments, draw: &Arc<ResolvedDraw>,
        fetch: FetchMode, resident: bool,
    ) -> Option<ResidentSection> {
        let indices = draw.indices.as_ref()?;
        if resident && indices.iter().any(|&index| index >= draw.vertex_count) { return None; }
        let strides: Vec<_> = draw.streams.iter().map(|s| s.stride).collect();
        let repack = RepackPlan::with_mode(&draw.layout, &strides, fetch);
        let mut streams = Vec::with_capacity(draw.streams.len());
        for (plan, stream) in repack.streams.iter().zip(&draw.streams) {
            if plan.identity {
                plan.check(&stream.data, draw.vertex_count).ok()?;
            }
            let key = stream.data.cache_id().map(|id| BufferKey::vertex(id, plan, draw.vertex_count));
            streams.push(self.buffer(target, key, wgpu::BufferUsages::VERTEX, resident, || {
                plan.repack(&stream.data, draw.vertex_count).ok()
            })?);
        }
        let indices = self.buffer(target, indices.cache_id().map(BufferKey::Indices),
            wgpu::BufferUsages::INDEX, resident, || Some(Cow::Borrowed(bytemuck::cast_slice(&indices[..]))))?;
        Some(ResidentSection {
            last_used: self.frame, source: Arc::clone(draw), fetch, repack, streams, indices, material: None,
        })
    }

    fn buffer<'a>(
        &mut self,
        target: &ExternalAttachments,
        key: Option<BufferKey>,
        usage: wgpu::BufferUsages,
        resident: bool,
        bytes: impl FnOnce() -> Option<Cow<'a, [u8]>>,
    ) -> Option<BufferRange> {
        if let Some((buffer, used)) = key.as_ref().and_then(|k| self.buffers.get_mut(k)) {
            *used = self.frame;
            return Some((buffer.clone(), 0, buffer.size()));
        }
        let bytes = bytes()?;
        let len = bytes.len() as u64;
        let device = &target.device;
        if len == 0 || len.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) > device.limits().max_buffer_size {
            return None;
        }
        let key = key.filter(|_| len < CACHE_BUDGET);
        if key.is_none() && !resident {
            if len.next_power_of_two() > device.limits().max_buffer_size {
                return None;
            }
            let (buffer, offset) = target.uploads.stage(device, &target.queue, &bytes);
            return Some((buffer, offset, len));
        }
        let buffer = gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor {
            label: Some("opaque indexed geometry"),
            contents: &bytes,
            usage,
        });
        if let Some(key) = key {
            if self.buffer_bytes + buffer.size() > CACHE_BUDGET {
                // Section entries retain their own buffers until the BSP changes.
                // Trim only the shared snapshot index, oldest sightings first.
                let mut age: Vec<_> = self.buffers.iter().map(|(k, (_, used))| (*used, k.clone())).collect();
                age.sort_unstable_by_key(|(used, _)| *used);
                for (_, old) in age {
                    if self.buffer_bytes + buffer.size() <= CACHE_BUDGET * 3 / 4 { break; }
                    if let Some((retired, _)) = self.buffers.remove(&old) { self.buffer_bytes -= retired.size(); }
                }
            }
            self.buffer_bytes += buffer.size();
            self.buffers.insert(key, (buffer.clone(), self.frame));
        }
        Some((buffer, 0, len))
    }
}

/// One unmodified indexed range from the standard geometry emitter.
pub struct IndexedPass {
    draw: Arc<ResolvedDraw>,
    cache: Arc<Cache>,
    frame: u64,
    fetch: FetchMode,
    texaccum: Option<Texaccum>,
    texaccum_stages: [bool; 4],
    section: Option<((u64, u32), SectionKey)>,
}

impl IndexedPass {
    pub fn new(
        draw: &ResolvedDraw,
        cache: Arc<Cache>,
        frame: u64,
        texaccum: Option<Texaccum>,
    ) -> Option<Self> {
        if draw.indices.as_ref()?.iter().any(|&i| i >= draw.vertex_count) { return None; }
        Self::candidate(draw, cache, frame, texaccum, None)
    }

    /// Sorted slots, render records, object handles and swap counters are
    /// transient. Validate the index range once with the matching geometry.
    pub fn for_section(
        draw: &ResolvedDraw, cache: Arc<Cache>, frame: u64, texaccum: Option<Texaccum>,
        scope: (u64, u32), geometry: Geometry, layer: u32,
    ) -> Option<Self> {
        Self::candidate(draw, cache, frame, texaccum, Some((scope, SectionKey::new(geometry, layer))))
    }

    fn candidate(
        draw: &ResolvedDraw, cache: Arc<Cache>, frame: u64, texaccum: Option<Texaccum>,
        section: Option<((u64, u32), SectionKey)>,
    ) -> Option<Self> {
        let indices = draw.indices.as_ref()?;
        if indices.len() < 3
            || !matches!(
                draw.primitive,
                PrimitiveType::TriangleList | PrimitiveType::TriangleStrip
            )
            || draw.target.depth_format.is_none()
            || draw.streams.len() != draw.layout.streams.len()
            || draw.hw.zpass_count_enable
        {
            return None;
        }
        Some(Self {
            draw: Arc::new(draw.clone()),
            fetch: cache.fetch,
            cache,
            frame,
            section,
            texaccum,
            texaccum_stages: draw
                .textures
                .map(|header| texaccum.is_some_and(|t| header == Some(t.texture))),
        })
    }

    fn prepare(
        &self,
        target: &ExternalAttachments,
        guest_size: (u32, u32),
    ) -> Option<ExternalDraws> {
        let draw = &self.draw;
        let (width, height) = guest_size;
        if target.targets.0 == target.targets.1 {
            return None;
        }
        let scale = target.size.0.checked_div(width)?;
        if scale == 0
            || width.checked_mul(scale)? != target.size.0
            || height.checked_mul(scale)? != target.size.1
            || (width, height) != (draw.target.width, draw.target.height)
            || target.size
                != (
                    target.depth.texture().width(),
                    target.depth.texture().height(),
                )
            || (draw.state.render.stencil.enable && !target.depth_format.has_stencil_aspect())
        {
            return None;
        }
        let caps = Capabilities::from_features(target.device.features());
        let mut cache = self.cache.devices.lock().ok()?;
        let cache = cache
            .entry(target.device.clone())
            .or_insert_with(|| DeviceCache::new(&target.device));
        if cache.frame != self.frame {
            cache.frame = self.frame;
            cache.bindings.end_frame();
        }

        let section_key = self.section.map(|(scope, key)| {
            if cache.scope != Some(scope) {
                cache.sections.clear();
                cache.buffers.clear();
                cache.buffer_bytes = 0;
                cache.scope = Some(scope);
            }
            key
        });
        let present = target.stages.each_ref().map(Option::is_some);
        // Visibility can split one section part into several indexed windows.
        // Keep each live window: alternating ranges must not replace each other.
        let previous = section_key.and_then(|key| {
            let windows = cache.sections.get(&key)?;
            let i = windows.iter().position(|s| s.matches(draw, self.fetch))?;
            Some((key, i))
        });
        let material = previous.and_then(|(key, i)| cache.sections.get(&key)?[i].material.as_ref())
            .filter(|m| m.matches(draw, present));
        let material_hit = material.is_some();
        let formats = TargetFormats { color: target.color_format, depth: Some(target.depth_format), samples: 1 };
        let cached_pipeline = material.and_then(|m| m.pipelines.get(&formats)).cloned();
        let (program, shader) = if let Some(material) = material {
            (Arc::clone(&material.program), material.shader.clone())
        } else {
            cache.resolve_shader(&target.device, draw, present, self.fetch, caps)?
        };
        let dims = shader.dims;
        for (n, dim) in dims.iter().enumerate() {
            let Some(dim) = dim else { continue };
            let view = target.stages[n].as_ref()?;
            let have = match draw.texture_info[n]?.kind {
                TextureKind::D2 => wgpu::TextureViewDimension::D2,
                TextureKind::Volume => wgpu::TextureViewDimension::D3,
                TextureKind::Cube => wgpu::TextureViewDimension::Cube,
            };
            if *dim != have
                || view.texture() == target.color.texture()
                || view.texture() == target.depth.texture()
                || !gfx::program::filterable(view.texture().format(), target.device.features())
            {
                return None;
            }
            if self.texaccum_stages[n] {
                let accum = self.texaccum?;
                let info = draw.texture_info[n]?;
                let sampled = target.sampled.first()?.as_ref()?;
                let source = target.slots.first()?.as_ref()?;
                if (info.width, info.height) != accum.size
                    || info.format.layout() != gfx::format::Layout::Linear
                    || source.guest_size != accum.size
                    || source.size != sampled.size
                    || source.sample.texture() != sampled.view.texture()
                    || source.sample.texture() != view.texture()
                {
                    return None;
                }
            }
        }
        // A header that was bound but unreferenced does not establish
        // that this is a combined material's texture accumulation pass.
        if self.texaccum.is_some()
            && !self
                .texaccum_stages
                .iter()
                .zip(dims)
                .any(|(&accum, dim)| accum && dim.is_some())
        {
            return None;
        }
        let views = std::array::from_fn(|n| dims[n].and(target.stages[n].clone()));
        let cached_textures = previous.and_then(|(key, i)| cache.sections.get(&key)?[i].material.as_ref())
            .filter(|_| material_hit)
            .and_then(|m| m.groups.iter().find(|(stages, _)| stages == &views))
            .map(|(_, textures)| textures.clone());
        let bindings_hit = cached_textures.is_some();
        let textures = cached_textures.unwrap_or_else(|| cache.bindings.textures(
            &target.device,
            &target.queue,
            dims,
            views.each_ref().map(Option::as_ref),
            program.samplers,
            [[false; 4]; 4],
        ));
        if textures.is_none() && dims.iter().any(Option::is_some) {
            return None;
        }

        let indices = draw.indices.as_ref()?;
        let index_count = indices.len().try_into().ok()?;
        let mut fresh;
        let section = if let Some((key, i)) = previous {
            let section = &mut cache.sections.get_mut(&key)?[i];
            section.last_used = self.frame;
            // Promote the current snapshot IDs after an identical first-use copy.
            section.source = Arc::clone(draw);
            if material_hit {
                section.material.as_mut()?.source = Arc::clone(draw);
            }
            section
        } else {
            fresh = cache.upload_section(target, draw, self.fetch, section_key.is_some())?;
            if let Some(key) = section_key {
                let windows = cache.sections.entry(key).or_default();
                // A streaming part can change every frame. Keep its working set
                // across swaps without retaining every retired resource version.
                windows.retain(|s| self.frame.wrapping_sub(s.last_used) <= 1);
                windows.push(fresh);
                windows.last_mut()?
            } else { &mut fresh }
        };
        let mut state = draw.state.render;
        if program.writes_depth {
            // The same offset is already in the translated W-depth uniforms.
            state.solid_offset_enable = false;
            state.wireframe_offset_enable = false;
            state.point_offset_enable = false;
        }
        let pipeline_hit = cached_pipeline.is_some();
        let (key, pipeline) = if let Some(cached) = cached_pipeline {
            cached
        } else {
            let mut key = PipelineKey::from_plan(
                shader.id,
                &section.repack,
                draw.primitive,
                &state,
                formats,
                caps,
            );
            if draw.primitive == PrimitiveType::TriangleStrip {
                key.primitive.strip_index_format = Some(wgpu::IndexFormat::Uint32);
            }
            let pipeline = cache
                .pipelines
                .entry(key.clone())
                .or_insert_with(|| {
                    Pending::ready(gfx::program::validated_resource(&target.device, || {
                        create_render_pipeline(&target.device, &key, &shader.shader, "native opaque layer")
                    }))
                })
                .clone();
            if section_key.is_some() {
                if !material_hit {
                    section.material = Some(ResidentMaterial {
                        source: Arc::clone(draw), present, program: Arc::clone(&program),
                        shader: shader.clone(), pipelines: HashMap::new(), groups: Vec::new(),
                    });
                }
                section.material.as_mut()?.pipelines.insert(formats, (key.clone(), pipeline.clone()));
            }
            (key, pipeline)
        };
        pipeline.get().as_ref()?;
        if section_key.is_some() && !bindings_hit {
            let groups = &mut section.material.as_mut()?.groups;
            if groups.len() == 2 { groups.remove(0); }
            groups.push((views, textures.clone()));
        }

        let clip = if draw.clip_rects.is_empty() {
            std::slice::from_ref(draw.clip.as_ref()?)
        } else {
            &draw.clip_rects
        };
        let clip: Vec<PixelRect> = clip
            .iter()
            .map(|r| scaled_clip(r, scale, target.size))
            .collect::<Option<_>>()?;
        let streams = section.streams.clone();
        let indices = section.indices.clone();
        let (vertex, pixel) = program.uniforms(draw, scale);
        let sizes = [VertexUniforms::SIZE, PixelUniforms::SIZE];
        let fill = |_: usize, v: &mut [u8], p: &mut [u8]| {
            vertex.write_bytes(v);
            pixel.write_bytes(p);
        };
        let layout = cache.bindings.uniform_layout();
        let params = if section_key.is_some() {
            gfx::churn::native_cache(pipeline_hit && bindings_hit);
            target.uploads.resident_uniform_run(&target.device, &target.queue, layout, sizes, 1, fill)
        } else {
            target.uploads.uniform_run(&target.device, &target.queue, layout, sizes, 1, fill)
        }?;
        let offsets = params.offsets(0);
        let mut groups = vec![BoundGroup { index: 0, group: params.group, offsets: offsets.to_vec() }];
        groups.extend(textures.map(|group| BoundGroup::new(1, group)));
        let mut clip = clip.into_iter().map(|r| ScissorRect {
            x: r.x, y: r.y, width: r.width, height: r.height,
        });
        let first = RecordedDraw::prepared_indexed(
            &key, pipeline, streams, indices, index_count, groups, &state, target.size, clip.next()?,
        );
        let mut packets = Vec::with_capacity(clip.len() + 1);
        packets.push(first);
        for scissor in clip {
            packets.push(packets[0].clone().with_scissor(scissor, target.size));
        }
        Some(ExternalDraws { ranges: 1, packets, discards: shader.discards, writes: Vec::new() })
    }
}

impl ExternalPass for IndexedPass {
    fn uses_validated_resources(&self) -> bool { true }

    fn encode(&mut self, _target: ExternalTarget<'_>) {}

    fn resolved_fallback(&self) -> Option<Arc<ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }

    fn sampled(&self) -> Vec<u32> {
        self.texaccum.iter().map(|target| target.texture).collect()
    }

    fn writes(&self) -> ExternalWrites {
        let r = &self.draw.state.render;
        let stencil = &r.stencil;
        ExternalWrites {
            color: !gfx::translate::color_writes(r.color_write).is_empty(),
            depth_stencil: (r.depth_mode != DepthBufferMode::Disabled && r.depth_write)
                || (stencil.enable
                    && stencil.write_mask != 0
                    && [stencil.fail, stencil.depth_fail, stencil.pass]
                        .iter()
                        .any(|&op| op != StencilOp::Keep)),
        }
    }

    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        Some(self.prepare(target, request.surface_size).ok_or("opaque draw resources"))
    }
}

fn scaled_clip(rect: &PixelRect, scale: u32, extent: (u32, u32)) -> Option<PixelRect> {
    let x = rect.x.checked_mul(scale)?;
    let y = rect.y.checked_mul(scale)?;
    let width = rect.width.checked_mul(scale)?.min(extent.0.checked_sub(x)?);
    let height = rect
        .height
        .checked_mul(scale)?
        .min(extent.1.checked_sub(y)?);
    (width != 0 && height != 0).then_some(PixelRect {
        x,
        y,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converted_streams_require_the_same_snapshot_count_and_repack_plan() {
        use gfx::vertex::{
            StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
        };
        let plan = |ty| {
            let layout = VertexLayout {
                streams: vec![StreamLayout {
                    stream: 0,
                    stride: 4,
                    attributes: vec![VertexAttribute {
                        register: VertexRegister(0),
                        data_type: ty,
                        offset: 0,
                    }],
                }],
                pretransformed: false,
            };
            RepackPlan::with_mode(&layout, &[4], FetchMode::Cpu)
                .streams
                .remove(0)
        };
        let short = plan(VertexDataType::Short2);
        let packed = plan(VertexDataType::NormPacked3);
        assert!(!short.identity && !packed.identity);
        let key = BufferKey::vertex(7, &short, 3);
        assert_eq!(key, BufferKey::vertex(7, &short.clone(), 3));
        assert_ne!(key, BufferKey::vertex(7, &packed, 3));
        assert_ne!(key, BufferKey::vertex(8, &short, 3));
        assert_ne!(key, BufferKey::vertex(7, &short, 4));
    }

    #[test]
    fn scaled_scissors_keep_the_same_guest_rectangle() {
        let clip = PixelRect {
            x: 7,
            y: 5,
            width: 11,
            height: 9,
        };
        assert_eq!(
            scaled_clip(&clip, 2, (32, 32)),
            Some(PixelRect {
                x: 14,
                y: 10,
                width: 18,
                height: 18
            })
        );
        assert_eq!(scaled_clip(&clip, 2, (14, 32)), None);
    }
}
