//! Native UI packets over the device's pinned attachments. The guest's
//! shader programs and raster state are shared inputs with the D3D renderer.
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
    fn ui_copy_scissors_preserve_the_guest_pixels_and_clip_at_the_edge() {
        let guest = (640, 480);
        let host = (1440, 720);
        let scale = RenderScale::between(guest, host).unwrap();
        let clip = d3d::backend::PixelRect { x: 631, y: 11, width: 13, height: 9 };
        let result = scaled_scissor(&clip, scale, host).unwrap();
        assert_eq!((result.x, result.y, result.width, result.height), (1420, 17, 20, 13));
    }
}
