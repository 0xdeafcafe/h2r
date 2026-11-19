//! Dynamic decal admission at the actual resolved `D3DDevice_DrawVertices`
//! packet. The guest callback still runs, so texture setup, duplicate markers
//! and state tails are unchanged. A replacement is returned only after the
//! complete draw resolves; the handoff records it transactionally on both
//! target copies before suppressing D3D.

use std::sync::Arc;

use d3d::backend::{DrawAdmission, DrawSnapshot, ExternalPlan, ExternalWrites};
use d3d::shader::VertexProcessing;
use gfx::vertex::{PrimitiveType, VertexDataType, VertexRegister};
use native_render::decal::{Mode, VERTEX_SIZE, Vertex};
use native_render::decal_gpu::{
    ClipRect, DecalClip, DecalGpu, DecalTarget, Prepared, ShaderSnapshot, StageExtras,
};
use shaders::vsh::VertexUniforms;
use timeline::native::Stage;

struct Glue;

/// Registers the admission with the device. No callback suppression or custom
/// geometry/texture loader is installed.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) {
    let Ok(mut d) = device.lock() else { return };
    d.add_draw_admission(Arc::new(Glue));
}

fn clip(r: d3d::PixelRect) -> ClipRect {
    ClipRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

fn prepare(mode: Mode, snapshot: &DrawSnapshot<'_>) -> Result<Prepared, &'static str> {
    let d = snapshot.draw;
    if d.primitive != PrimitiveType::QuadList
        || d.indices.is_some()
        || d.vertex_count == 0
        || d.vertex_count % 4 != 0
    {
        return Err("geometry");
    }
    if d.layout.streams.len() != 1 || d.streams.len() != 1 {
        return Err("vertex streams");
    }
    let layout = &d.layout.streams[0];
    let stream = &d.streams[0];
    if layout.stride != VERTEX_SIZE as u32
        || stream.stride != VERTEX_SIZE as u32
        || layout.attributes.len() != 2
    {
        return Err("vertex declaration");
    }
    for (register, data_type, offset) in [
        (VertexRegister::POSITION, VertexDataType::Float3, 0),
        (VertexRegister::DIFFUSE, VertexDataType::D3dColor, 12),
    ] {
        if !layout
            .attributes
            .iter()
            .any(|a| (a.register, a.data_type, a.offset) == (register, data_type, offset))
        {
            return Err("vertex declaration");
        }
    }
    if stream.data.len() != d.vertex_count as usize * VERTEX_SIZE {
        return Err("vertex bytes");
    }
    let vertices = stream
        .data
        .as_chunks::<VERTEX_SIZE>()
        .0
        .iter()
        .map(|v| Vertex {
            position: [
                f32::from_bits(u32::from_le_bytes(v[0..4].try_into().expect("position"))),
                f32::from_bits(u32::from_le_bytes(v[4..8].try_into().expect("position"))),
                f32::from_bits(u32::from_le_bytes(v[8..12].try_into().expect("position"))),
            ],
            color: v[12..16].try_into().expect("colour"),
        })
        .collect::<Vec<_>>();
    let VertexProcessing::Program { microcode, .. } = &d.shader.vertex else {
        return Err("vertex program");
    };
    if microcode.len() % 4 != 0 || microcode.is_empty() || microcode.len() > 136 * 4 {
        return Err("vertex program");
    }
    let mut code = vec![0x78, 0x20];
    code.extend_from_slice(&((microcode.len() / 4) as u16).to_le_bytes());
    for word in microcode.iter() {
        code.extend_from_slice(&word.to_le_bytes());
    }
    let pixel = d.shader.pixel_shader.as_ref().ok_or("combiner")?;
    let combiner_state: &[u32; 57] = &d.shader.combiner_state;
    let combiner = d3d::nv2a::guest_def(
        pixel,
        combiner_state,
        d.raw.render[136],
        d.shader.final_combiner_latch,
    );
    let extras = d.extras.map(|e| StageExtras {
        color_key_op: e.color_key_op,
        color_sign: e.color_sign,
        alpha_kill: e.alpha_kill,
        texture_transform_flags: e.texture_transform_flags,
        bump_env_mat: e.bump_env_mat,
        bump_env_lscale: e.bump_env_lscale,
        bump_env_loffset: e.bump_env_loffset,
    });
    native_render::decal_gpu::check_state(mode, &d.state.render, &d.state.stages, &extras)?;
    if d.raw.render[83] != 0 || d.raw.render[141] != 0 {
        return Err("lighting/stipple");
    }
    if d.raw.render[d3d::state::slot::ZBIAS as usize] != 0 {
        return Err("ZBIAS");
    }
    if d.texture_info[0]
        .is_some_and(|info| info.kind != gfx::texture::TextureKind::D2 || info.format.is_depth())
    {
        return Err("texture kind");
    }
    let tex_scale = d.texture_info[0].and_then(|info| {
        (info.format.layout() == gfx::format::Layout::Linear && info.width != 0 && info.height != 0)
            .then_some([1.0 / info.width as f32, 1.0 / info.height as f32, 1.0, 1.0])
    });
    let shader = ShaderSnapshot {
        render: d.state.render,
        stages: d.state.stages,
        extras,
        combiner,
        microcode: code,
        provided_inputs: d.shader.provided_inputs,
        vertex: VertexUniforms {
            constants: *d.shader.constants,
            defaults: d.shader.vertex_defaults,
            surface: [d.target.width as f32, d.target.height as f32],
            z_max: gfx::renderer::d3dz_max(d.target.depth_format),
            ..Default::default()
        },
        integer_depth: !matches!(
            d.target.depth_format,
            Some(
                gfx::D3DFormat::F16
                    | gfx::D3DFormat::LinF16
                    | gfx::D3DFormat::F24S8
                    | gfx::D3DFormat::LinF24S8
            )
        ),
        tex_scale,
    };
    native_render::decal_gpu::check_program(mode, &shader)?;
    let clips = if d.clip_rects.is_empty() {
        d.clip.into_iter().map(clip).collect()
    } else {
        d.clip_rects.iter().copied().map(clip).collect()
    };
    Ok(Prepared {
        mode,
        vertices,
        shader,
        clips,
    })
}

impl DrawAdmission for Glue {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let (mode, list) = feature::native_decals::current()?;
        if snapshot.draw.entry != "D3DDevice_DrawVertices" {
            return None;
        }
        feature::native_decals::note_examined();
        let comparing = feature::native_decals::comparing(mode, list);
        let prepared = match prepare(mode, snapshot) {
            Ok(p) => p,
            Err(why) => {
                feature::native_decals::note_refusal(why);
                if comparing {
                    feature::native_decals::note_compared();
                    eprintln!(
                        "native-decals: compare mode {} list {}: {why}",
                        mode.index(),
                        list
                    );
                }
                return None;
            }
        };
        if comparing {
            feature::native_decals::note_compared();
            eprintln!(
                "native-decals: compare mode {} list {}: {} quads, textures {:?}: prepared",
                mode.index(),
                list,
                prepared.vertices.len() / 4,
                snapshot.draw.textures,
            );
            return None;
        }
        feature::native_decals::note_admitted();
        Some((
            ExternalPlan {
                stage: Stage::Decals,
                view: 0,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(DecalPass {
                prepared,
                clip: None,
                gpu: None,
            }),
        ))
    }
}

struct DecalPass {
    prepared: Prepared,
    clip: Option<d3d::DepthClip>,
    gpu: Option<DecalGpu>,
}

impl DecalPass {
    fn prepare(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.gpu.is_none() {
            self.gpu = Some(DecalGpu::new(t.device, &self.prepared)?);
        }
        if self.gpu.as_ref().is_some_and(|g| g.sampled()) && t.stages[0].is_none() {
            return Err("texture view");
        }
        Ok(())
    }
}

impl d3d::ExternalPass for DecalPass {
    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: true,
            depth_stencil: self.prepared.mode.w_buffered(),
        }
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        self.prepare(t)
    }

    fn record_ranges(&mut self, mut t: d3d::ExternalTarget<'_>) -> usize {
        if let Err(why) = self.prepare(&t) {
            feature::native_decals::note_refusal(why);
            return 0;
        }
        let Some(clip) = self.clip else {
            feature::native_decals::note_refusal("issue snapshot");
            return 0;
        };
        let clip = DecalClip {
            range: clip.range,
            clamp: clip.clamp,
            w_buffered: clip.w_buffered,
        };
        let mut target = DecalTarget {
            device: t.device,
            queue: t.queue,
            encoder: &mut *t.encoder,
            color: t.color,
            color_format: t.color_format,
            depth: t.depth,
            depth_format: t.depth_format,
            viewport: t.viewport,
            z_scale: t.z_scale,
            guest_size: t.guest_size,
            size: t.size,
            stages: t.stages,
        };
        usize::from(
            self.gpu
                .as_mut()
                .expect("prepared above")
                .encode(&mut target, clip),
        )
    }
}
