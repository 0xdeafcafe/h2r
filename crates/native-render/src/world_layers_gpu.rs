//! Records opaque indexed geometry from an ordered, resolved draw. The
//! guest has already selected the live shader, streams and attachments.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalDraws, ExternalPass, ExternalPassRequest, ExternalTarget,
    ExternalWrites, PixelRect, ResolvedDraw,
};
use d3d::nv2a::{NativeDrawKey, NativeDrawProgram};
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
use wgpu::util::DeviceExt;

use crate::world_layers::Texaccum;

const CACHE_BUDGET: u64 = 64 * 1024 * 1024;

#[derive(Default)]
pub struct Cache(Mutex<HashMap<wgpu::Device, DeviceCache>>);

struct DeviceCache {
    bindings: ProgramBindings,
    shaders: HashMap<NativeDrawKey, Option<Program>>,
    pipelines: HashMap<PipelineKey, Arc<PendingPipeline>>,
    buffers: HashMap<BufferKey, wgpu::Buffer>,
    buffer_bytes: u64,
    frame: u64,
}

#[derive(Clone)]
struct Program {
    id: ShaderId,
    shader: ShaderProgram,
    dims: StageDims,
    discards: bool,
}

type BufferRange = (wgpu::Buffer, u64, u64);

#[derive(Debug, PartialEq, Eq, Hash)]
enum BufferKey {
    RawVertex(u64, u32, u32),
    RepackedVertex(u64, u32, StreamPlan),
    Indices(u64),
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
            frame: 0,
        }
    }

    fn buffer<'a>(
        &mut self,
        target: &ExternalAttachments,
        key: Option<BufferKey>,
        usage: wgpu::BufferUsages,
        bytes: impl FnOnce() -> Option<Cow<'a, [u8]>>,
    ) -> Option<BufferRange> {
        if let Some(buffer) = key.as_ref().and_then(|k| self.buffers.get(k)) {
            return Some((buffer.clone(), 0, buffer.size()));
        }
        let bytes = bytes()?;
        let len = bytes.len() as u64;
        let device = &target.device;
        if len == 0 || len.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) > device.limits().max_buffer_size {
            return None;
        }
        let key = key.filter(|_| len < CACHE_BUDGET);
        if key.is_none() {
            if len.next_power_of_two() > device.limits().max_buffer_size {
                return None;
            }
            let (buffer, offset) = target.uploads.stage(device, &target.queue, &bytes);
            return Some((buffer, offset, len));
        }
        gfx::churn::buffer_created();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("opaque indexed geometry"),
            contents: &bytes,
            usage,
        });
        if let Some(key) = key {
            if self.buffer_bytes + buffer.size() > CACHE_BUDGET {
                self.buffers.clear();
                self.buffer_bytes = 0;
            }
            self.buffer_bytes += buffer.size();
            self.buffers.insert(key, buffer.clone());
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
}

impl IndexedPass {
    pub fn new(
        draw: &ResolvedDraw,
        cache: Arc<Cache>,
        frame: u64,
        texaccum: Option<Texaccum>,
    ) -> Option<Self> {
        let indices = draw.indices.as_ref()?;
        if indices.len() < 3
            || indices.iter().any(|&index| index >= draw.vertex_count)
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
            cache,
            frame,
            fetch: FetchMode::from_env(),
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
        let program = NativeDrawProgram::resolve(
            draw,
            target.stages.each_ref().map(Option::is_some),
            caps,
            self.fetch,
            scale,
        )
        .ok()?;
        let mut cache = self.cache.0.lock().ok()?;
        let cache = cache
            .entry(target.device.clone())
            .or_insert_with(|| DeviceCache::new(&target.device));
        if cache.frame != self.frame {
            cache.frame = self.frame;
            cache.bindings.end_frame();
        }

        let new_id = ShaderId(cache.shaders.len().try_into().ok()?);
        let shader = cache
            .shaders
            .entry(program.key.clone())
            .or_insert_with(|| {
                let linked = program.key.translate().ok()?;
                let module = gfx::program::validated_resource(&target.device, || {
                    target.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("opaque indexed program"),
                        source: wgpu::ShaderSource::Wgsl(linked.source.into()),
                    })
                })?;
                Some(Program {
                    id: new_id,
                    shader: cache.bindings.program(&target.device, module, linked.dims),
                    dims: linked.dims,
                    discards: linked.discards,
                })
            })
            .clone()?;
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
        let textures = cache.bindings.textures(
            &target.device,
            &target.queue,
            dims,
            target.stages.each_ref().map(Option::as_ref),
            program.samplers,
            [[false; 4]; 4],
        );
        if textures.is_none() && dims.iter().any(Option::is_some) {
            return None;
        }

        let strides: Vec<u32> = draw.streams.iter().map(|s| s.stride).collect();
        let repack = RepackPlan::with_mode(&draw.layout, &strides, self.fetch);
        let mut state = draw.state.render;
        if program.writes_depth {
            // The same offset is already in the translated W-depth uniforms.
            state.solid_offset_enable = false;
            state.wireframe_offset_enable = false;
            state.point_offset_enable = false;
        }
        let mut key = PipelineKey::from_plan(
            shader.id,
            &repack,
            draw.primitive,
            &state,
            TargetFormats {
                color: target.color_format,
                depth: Some(target.depth_format),
                samples: 1,
            },
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
        pipeline.get().as_ref()?;

        let clip = if draw.clip_rects.is_empty() {
            std::slice::from_ref(draw.clip.as_ref()?)
        } else {
            &draw.clip_rects
        };
        let clip: Vec<PixelRect> = clip
            .iter()
            .map(|r| scaled_clip(r, scale, target.size))
            .collect::<Option<_>>()?;
        let mut streams = Vec::with_capacity(draw.streams.len());
        for (plan, stream) in repack.streams.iter().zip(&draw.streams) {
            // Raw streams with different declarations may share the bytes. Check
            // the last required attribute even on a cache hit for a shorter layout.
            if plan.identity {
                plan.check(&stream.data, draw.vertex_count).ok()?;
            }
            let key = stream
                .data
                .cache_id()
                .map(|id| BufferKey::vertex(id, plan, draw.vertex_count));
            streams.push(
                cache.buffer(target, key, wgpu::BufferUsages::VERTEX, || {
                    plan.repack(&stream.data, draw.vertex_count).ok()
                })?,
            );
        }
        let indices = draw.indices.as_ref()?;
        let index_count = indices.len().try_into().ok()?;
        let index_key = indices.cache_id().map(BufferKey::Indices);
        let indices = cache.buffer(target, index_key, wgpu::BufferUsages::INDEX, || {
            Some(Cow::Borrowed(bytemuck::cast_slice(&indices[..])))
        })?;
        let params = target.uploads.uniform_run(
            &target.device,
            &target.queue,
            cache.bindings.uniform_layout(),
            [VertexUniforms::SIZE, PixelUniforms::SIZE],
            1,
            |_, v, p| {
                program.vertex.write_bytes(v);
                program.pixel.write_bytes(p);
            },
        )?;
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
        Some(ExternalDraws { ranges: 1, packets, discards: shader.discards })
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
