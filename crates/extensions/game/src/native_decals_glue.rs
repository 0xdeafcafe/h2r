//! Dynamic decal admission at the actual resolved `D3DDevice_DrawVertices`
//! packet. The guest callback still runs, so texture setup, duplicate markers
//! and state tails are unchanged. A replacement is returned only after the
//! complete draw resolves; the handoff records it transactionally on both
//! target copies before suppressing D3D.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use d3d::backend::{
    DrawAdmission, DrawSnapshot, ExternalAttachments, ExternalDraws, ExternalPlan, ExternalWrites,
};
use d3d::shader::VertexProcessing;
use gfx::vertex::{PrimitiveType, StreamLayout, VertexDataType, VertexRegister};
use native_render::decal::{Mode, VERTEX_SIZE, Vertex, decode_packed};
use native_render::decal_gpu::{
    ClipRect, DecalCache, DecalClip, DecalGpu, DecalTarget, PacketTarget, Prepared, ReferenceCopy,
    ShaderSnapshot, StageExtras,
};
use shaders::vsh::VertexUniforms;
use timeline::native::Stage;

const STIPPLE_ENABLE: usize = 83;
const TWO_SIDED_LIGHTING: usize = 141;

/// The per-device caches of the decals' programs, pipelines and texture
/// groups, shared by every admitted draw.
type Caches = Arc<Mutex<HashMap<wgpu::Device, DecalCache>>>;

#[derive(Default)]
struct Glue {
    caches: Caches,
}

/// Register on the guest's ordered draw, which also owns its fallback.
pub fn install<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) {
    if !hooks::Switches::from_env().is_ok_and(|s| render_bridge::native_decals::on(&s)) {
        return;
    }
    let Ok(mut d) = device.lock() else { return };
    d.add_draw_admission(Arc::new(Glue::default()));
}

fn clip(r: d3d::PixelRect) -> ClipRect {
    ClipRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

/// The resolved draw's vertices, decoded from its one stream. Halo 2's decal
/// declaration (the attribute list at `0x0043f7b9`, bound with stride 16) is
/// float3 position at `+0` in `v0` and `NORMPACKED3` at `+12` in the second declared register: an
/// 11:11:10 signed normalised dword, decoded as the NV2A fetches it
/// ([`decode_packed`]). Any other declaration is refused.
fn decal_vertices(
    layout: &StreamLayout,
    stream: &d3d::backend::StreamBytes,
    vertex_count: u32,
) -> Result<Vec<Vertex>, &'static str> {
    if layout.stride != VERTEX_SIZE as u32
        || stream.stride != VERTEX_SIZE as u32
        || layout.attributes.len() != 2
    {
        return Err("vertex declaration");
    }
    let position = layout.attributes.iter().any(|a| {
        (a.register, a.data_type, a.offset) == (VertexRegister::POSITION, VertexDataType::Float3, 0)
    });
    let packed = layout.attributes.iter().any(|a| {
        (1..16).contains(&a.register.0)
            && a.data_type == VertexDataType::NormPacked3
            && a.offset == 12
    });
    if !position || !packed {
        return Err("vertex declaration");
    }
    if stream.data.len() != vertex_count as usize * VERTEX_SIZE {
        return Err("vertex bytes");
    }
    Ok(stream
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
            diffuse: decode_packed(v[12..16].try_into().expect("packed")),
        })
        .collect())
}

/// Filled quads use the live polygon mask. A pushed NV2A control word
/// takes precedence over the D3D state shadow, as it does on the D3D path.
fn decal_stipple(hw: &d3d::hw::HwState, raw_enable: u32) -> Option<[u32; 32]> {
    (hw.stipple_control.unwrap_or(raw_enable) != 0).then_some(hw.stipple_pattern)
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
    let vertices = decal_vertices(&d.layout.streams[0], &d.streams[0], d.vertex_count)?;
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
    native_render::decal_gpu::check_state(mode, &d.state.render, &extras)?;
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
        two_sided_lighting: d.raw.render[TWO_SIDED_LIGHTING] != 0,
        stipple: decal_stipple(&d.hw, d.raw.render[STIPPLE_ENABLE]),
        vertex: VertexUniforms {
            constants: *d.shader.constants,
            defaults: d.shader.vertex_defaults,
            surface: [d.target.width as f32, d.target.height as f32],
            z_max: gfx::renderer::d3dz_max(d.target.depth_format),
            ui_transform: d
                .ui_fit
                .map_or(d3d::backend::UiFit::IDENTITY.transform, |fit| fit.transform),
            ..Default::default()
        },
        w_buffered: d3d::hw::w_buffered(d),
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
    let clips = if d.clip_rects.is_empty() {
        d.clip.as_slice()
    } else {
        &d.clip_rects
    };
    let reference = (d.retail_constants.is_some() || d.ui_fit.is_some()).then(|| ReferenceCopy {
        constants: d
            .retail_constants
            .as_ref()
            .map_or(&d.shader.constants, |(constants, _)| constants)
            .clone(),
        clips: clips.iter().copied().map(clip).collect(),
    });
    Ok(Prepared {
        mode,
        vertices,
        shader,
        clips: clips.iter().map(|&r| clip(d.display_rect(r))).collect(),
        reference,
    })
}

/// The registers a program reads and the first of the four rows its
/// position goes through. `None` when the microcode does not decode.
fn constant_reads(
    microcode: &[u32],
) -> Option<(native_render::decal_material::Reads, Option<u8>)> {
    use d3d::clip_space::epilogue::{Position, position};
    use shaders::vsh::{Program, ProgramKind, SrcReg};
    let program = Program::from_words(ProgramKind::Normal, microcode).ok()?;
    let mut reads = native_render::decal_material::Reads::default();
    for inst in &program.instructions {
        for src in inst.sources() {
            if let SrcReg::Const { index, relative } = src.reg {
                if relative {
                    reads.relative = true;
                }
                reads.registers.insert(index);
            }
        }
    }
    let base = match position(microcode) {
        Position::Rows(b) => Some(b),
        _ => None,
    };
    Some((reads, base))
}

impl DrawAdmission for Glue {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let (mode, _) = render_bridge::native_decals::current()?;
        if snapshot.draw.entry != "D3DDevice_DrawVertices" {
            return None;
        }
        render_bridge::native_decals::note_examined();
        let prepared = match prepare(mode, snapshot) {
            Ok(p) => p,
            Err(why @ ("vertex declaration" | "vertex streams")) => {
                let d = snapshot.draw;
                let strides: Vec<_> = d.streams.iter().map(|s| s.stride).collect();
                render_bridge::native_decals::note_admission_refusal(format!(
                    "{why}: {:?}, bound strides {strides:?}", d.layout
                ));
                return None;
            }
            Err(why) => {
                render_bridge::native_decals::note_admission_refusal(why);
                return None;
            }
        };
        render_bridge::native_decals::note_candidate();
        // The Rust material against this draw's own, 1 in 64.
        if render_bridge::native_decals::proof_sample() {
            let material = native_render::decal_material::material(
                mode,
                render_bridge::native_decals::offset_bits(),
            );
            let d = snapshot.draw;
            let mut fields = d.shader.pixel_shader.as_ref().map_or_else(
                || vec!["combiner_missing".to_owned()],
                |pixel| native_render::decal_material::compare(&material, pixel, &d.raw.render),
            );
            if let VertexProcessing::Program { microcode, .. } = &d.shader.vertex {
                let viewport = [d.viewport.x, d.viewport.y, d.viewport.width, d.viewport.height];
                match constant_reads(microcode) {
                    Some((reads, base)) => fields.extend(native_render::decal_material::compare_constants(
                        &reads,
                        base,
                        render_bridge::native_decals::view().as_ref(),
                        viewport,
                        &*d.shader.constants,
                    )),
                    None => fields.push("constant_reads_unread".to_owned()),
                }
                let mut code = vec![0x78, 0x20];
                code.extend_from_slice(&((microcode.len() / 4) as u16).to_le_bytes());
                code.extend(microcode.iter().flat_map(|w| w.to_le_bytes()));
                match render_bridge::native_decals::program_in_tags(&code) {
                    Some(true) => {}
                    Some(false) => fields.push("vertex_program".to_owned()),
                    None => fields.push("vertex_program_unread".to_owned()),
                }
            } else {
                fields.push("vertex_program_missing".to_owned());
            }
            render_bridge::native_decals::proof_record(&fields);
        }
        Some((
            ExternalPlan {
                stage: Stage::Decals,
                view: 0,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(DecalPass {
                prepared,
                draw: Arc::new(snapshot.draw.clone()),
                clip: None,
                gpu: None,
                caches: self.caches.clone(),
                frame: crate::d3d_diag::presents(),
            }),
        ))
    }
}

struct DecalPass {
    prepared: Prepared,
    draw: Arc<d3d::ResolvedDraw>,
    clip: Option<d3d::DepthClip>,
    gpu: Option<DecalGpu>,
    caches: Caches,
    /// The guest frame of the admission: the cache keeps a texture group
    /// only while draws of recent frames use it.
    frame: u64,
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
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }

    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: true,
            depth_stencil: self.draw.writes_depth_stencil(),
        }
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        self.prepare(t)
    }

    /// The draw's packets appended to the D3D pass open on these
    /// attachments, from the device's cached program and pipeline: no
    /// command buffer, render pass, shader or buffer of its own. `None`
    /// (the isolated recording below) when the guest clipped it all away.
    fn record_draws(
        &mut self,
        t: &ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let refuse = |why: &'static str| {
            render_bridge::native_decals::note_refusal(why);
            Some(Err(why))
        };
        let Some(clip) = self.clip else {
            return refuse("issue snapshot");
        };
        let clip = DecalClip {
            range: clip.range,
            clamp: clip.clamp,
            w_buffered: clip.w_buffered,
        };
        // A decal sampling the target it draws into cannot share its pass.
        let stage0 = t.stages[0].as_ref();
        if stage0.is_some_and(|v| [t.color.texture(), t.depth.texture()].contains(&v.texture())) {
            return refuse("texture is the target");
        }
        let target = PacketTarget {
            color_format: t.color_format,
            depth_format: t.depth_format,
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            stage0,
            copy: t.copy,
        };
        let mut caches = self.caches.lock().unwrap_or_else(|e| e.into_inner());
        let cache = caches.entry(t.device.clone()).or_default();
        cache.begin_frame(self.frame);
        match cache.draws(&t.device, &t.queue, &t.uploads, &self.prepared, clip, &target) {
            Ok(Some((packets, discards))) => Some(Ok(ExternalDraws {
                ranges: 1,
                packets,
                discards,
                writes: Vec::new(),
            })),
            Ok(None) => None,
            Err(why) => refuse(why),
        }
    }

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        if let Err(why) = self.prepare(&t) {
            render_bridge::native_decals::note_refusal(why);
            return 0;
        }
        let Some(clip) = self.clip else {
            render_bridge::native_decals::note_refusal("issue snapshot");
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
            copy: t.copy,
        };
        usize::from(
            self.gpu
                .as_mut()
                .expect("prepared above")
                .encode(&mut target, clip),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_program_reports_its_constant_reads_and_matrix_base() {
        use shaders::vsh::encode::{Inst, Operand as O};
        use shaders::vsh::{MacOp, OutReg};
        let mut p = Vec::new();
        for (i, mask) in [(0u8, 8u8), (1, 4), (2, 2), (3, 1)] {
            p.push(
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(10 + i)), None)
                    .to_out(false, OutReg::Pos, mask),
            );
        }
        p.push(Inst::mac(MacOp::Mov, Some(O::c(40)), None, None).to_out(false, OutReg::T(0), 15));
        let last = p.len() - 1;
        let mut words: Vec<u32> = p.iter().flat_map(|i| i.words()).collect();
        words[last * 4 + 3] |= 1;
        let (reads, base) = constant_reads(&words).unwrap();
        assert_eq!(reads.registers.into_iter().collect::<Vec<_>>(), [10, 11, 12, 13, 40]);
        assert_eq!((reads.relative, base), (false, Some(10)));
    }

    use super::*;
    use d3d::backend::{StreamBytes, StreamData};
    use gfx::vertex::VertexAttribute;

    /// Fixture for the v3 binding of the packed decal field:
    /// `v0` float3 at +0, `v3` `NORMPACKED3` at +12, stride 16.
    fn declaration() -> StreamLayout {
        StreamLayout {
            stream: 0,
            attributes: vec![
                VertexAttribute {
                    register: VertexRegister::POSITION,
                    data_type: VertexDataType::Float3,
                    offset: 0,
                },
                VertexAttribute {
                    register: VertexRegister::DIFFUSE,
                    data_type: VertexDataType::NormPacked3,
                    offset: 12,
                },
            ],
            stride: 16,
        }
    }

    fn stream(vertices: &[[u8; VERTEX_SIZE]], stride: u32) -> StreamBytes {
        StreamBytes {
            data: StreamData::Owned(vertices.concat()),
            stride,
        }
    }

    fn vertex(position: [f32; 3], packed: u32) -> [u8; VERTEX_SIZE] {
        let mut v = [0; VERTEX_SIZE];
        for (i, f) in position.iter().enumerate() {
            v[4 * i..4 * i + 4].copy_from_slice(&f.to_le_bytes());
        }
        v[12..16].copy_from_slice(&packed.to_le_bytes());
        v
    }

    #[test]
    fn decal_stipple_uses_hardware_control_before_the_raw_state() {
        let mut hw = d3d::hw::HwState::default();
        hw.stipple_pattern[10] = 1 << 11;

        assert_eq!(decal_stipple(&hw, 0), None);
        assert_eq!(decal_stipple(&hw, 1), Some(hw.stipple_pattern));
        hw.stipple_control = Some(0);
        assert_eq!(decal_stipple(&hw, 1), None);
        hw.stipple_control = Some(1);
        assert_eq!(decal_stipple(&hw, 0), Some(hw.stipple_pattern));
        hw.stipple_pattern = [0; 32];
        assert_eq!(decal_stipple(&hw, 0), Some([0; 32]));
    }

    #[test]
    fn halo2s_declaration_resolves_native_and_decodes() {
        let vs = [
            vertex([1.0, 2.0, 3.0], 0x0000_0000),
            vertex([-4.0, 0.5, 1e3], 0x7fdf_fbff),
            vertex([0.0, -1.0, 7.25], 0x8020_0400),
        ];
        let got = decal_vertices(&declaration(), &stream(&vs, 16), 3).unwrap();
        assert_eq!(
            got,
            [
                Vertex { position: [1.0, 2.0, 3.0], diffuse: [0.0, 0.0, 0.0] },
                Vertex { position: [-4.0, 0.5, 1e3], diffuse: [1.0, 1.0, 1.0] },
                Vertex { position: [0.0, -1.0, 7.25], diffuse: [-1.0, -1.0, -1.0] },
            ]
        );
    }

    #[test]
    fn packed_semantic_decodes_at_each_non_position_register() {
        let mut layout = declaration();
        let bytes = stream(&[vertex([2., -1., 7.], 1)], 16);
        for register in 1..16 {
            layout.attributes[1].register = VertexRegister(register);
            let got = decal_vertices(&layout, &bytes, 1).unwrap();
            assert_eq!(got[0].position, [2., -1., 7.]);
            assert_eq!(got[0].diffuse, [1. / 1023., 0., 0.]);
        }
        for register in [0, 16] {
            layout.attributes[1].register = VertexRegister(register);
            assert_eq!(decal_vertices(&layout, &bytes, 1), Err("vertex declaration"));
        }
    }

    #[test]
    fn other_declarations_are_refused() {
        let vs = [vertex([0.0; 3], 0)];
        // D3DCOLOR instead of NORMPACKED3 (the old assumption).
        let mut layout = declaration();
        layout.attributes[1].data_type = VertexDataType::D3dColor;
        assert_eq!(
            decal_vertices(&layout, &stream(&vs, 16), 1),
            Err("vertex declaration")
        );
        // A third attribute on a wider vertex.
        let mut layout = declaration();
        layout.attributes.push(VertexAttribute {
            register: VertexRegister::TEXCOORD0,
            data_type: VertexDataType::Float2,
            offset: 16,
        });
        layout.stride = 24;
        assert_eq!(
            decal_vertices(&layout, &stream(&vs, 24), 1),
            Err("vertex declaration")
        );
        // The stream's own stride disagrees.
        assert_eq!(
            decal_vertices(&declaration(), &stream(&vs, 20), 1),
            Err("vertex declaration")
        );
        // The bytes do not cover the vertices.
        let short = StreamBytes { data: StreamData::Owned(vec![0; 12]), stride: 16 };
        assert_eq!(decal_vertices(&declaration(), &short, 1), Err("vertex bytes"));
    }
}
