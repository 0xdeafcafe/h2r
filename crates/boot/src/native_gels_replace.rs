//! Draw-boundary admission for the original target-13 gel quad. The
//! handoff owns cancellation, two-copy recording and the commit that
//! surrenders the D3D packet. This module never suppresses a guest call.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use d3d::{
    DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ExternalSlot, ExternalTarget,
    ExternalWrites,
};
use gfx::state::{DepthBufferMode, FillMode, ShadeMode, TextureAddress};
use gfx::translate::{Capabilities, SamplerKey};
use gfx::vertex::{PrimitiveType, VertexDataType};
use native_render::gels::{Key, Target};
use native_render::gels_gpu::{Frame, INPUTS, Program, Vertex};
use native_render::pass_state::PassState;
use shaders::psh::{self, PixelOptions, PixelUniforms, TextureDim};
use shaders::vsh::VertexUniforms;
use timeline::native::{Metric, Stage};

/// Target 13 as this invocation leaves it, whether the admitted quad was
/// committed natively or retained by D3D. The consumer adds this surface
/// to its own plan and gets the correct reference/display sample view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub key: Key,
    pub guest_size: (u32, u32),
}
impl Binding {
    pub fn slot(self) -> ExternalSlot {
        ExternalSlot {
            header: self.key.surface,
            written: false,
        }
    }

    /// Generated target 13 is linear: its shader coordinates are guest
    /// texels, even when the sampled physical copy is larger.
    pub fn tex_scale(self) -> [f32; 4] {
        [
            1.0 / self.guest_size.0 as f32,
            1.0 / self.guest_size.1 as f32,
            1.0,
            1.0,
        ]
    }

    pub fn append(self, plan: &mut ExternalPlan) -> usize {
        let index = plan.slots.len();
        plan.slots.push(Some(self.slot()));
        index
    }

    pub fn sample<'a>(
        &self,
        target: &'a ExternalTarget<'_>,
        slot: usize,
    ) -> Option<&'a wgpu::TextureView> {
        let surface = target.slots.get(slot)?.as_ref()?;
        (surface.guest_size == self.guest_size).then_some(&surface.sample)
    }
}

#[derive(Clone, Copy)]
struct Scope {
    key: Key,
    target: Target,
    view: u32,
    generation: u64,
}

#[derive(Default)]
pub struct Replacements {
    scope: Mutex<Option<Scope>>,
    armed: AtomicBool,
    cache: Arc<native_render::gels_gpu::ResidentCache>,
}
impl Replacements {
    pub fn arm(&self, key: Key, target: Target, view: u32, generation: u64) {
        *self.scope.lock().unwrap_or_else(|e| e.into_inner()) = Some(Scope {
            key,
            target,
            view,
            generation,
        });
        self.armed.store(true, Ordering::Release);
    }
    pub fn cancel(&self) {
        self.armed.store(false, Ordering::Release);
        self.scope.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

impl DrawAdmission for Replacements {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        // Exactly the next resolved draw belongs to the armed helper.
        // Any discrepancy consumes the scope and leaves that packet on D3D.
        if !self.armed.load(Ordering::Acquire) || !self.armed.swap(false, Ordering::AcqRel) {
            return None;
        }
        let scope = self.scope.lock().ok()?.take()?;
        if snapshot.color_header != scope.target.surface
            || snapshot.draw.target.width != scope.target.size.0
            || snapshot.draw.target.height != scope.target.size.1
            || !matches!(snapshot.draw.entry, "inline Begin/End" | "D3DDevice_End")
        {
            Stage::Gels.count(Metric::RefusedOwnership, 1);
            return None;
        }
        let pass = match Replacement::snapshot(snapshot, scope, Arc::clone(&self.cache)) {
            Ok(pass) => pass,
            Err(reason) => {
                Stage::Gels.count(Metric::RefusedResources, 1);
                static REASONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
                let mut reasons = REASONS.lock().unwrap_or_else(|e| e.into_inner());
                if reasons.len() < 16 && !reasons.contains(&reason) {
                    eprintln!(
                        "native-gels: replacement refused: {reason}; original packet retained"
                    );
                    reasons.push(reason);
                }
                return None;
            }
        };
        let plan = ExternalPlan {
            stage: Stage::Gels,
            view: scope.view,
            ranges: 1,
            slots: Vec::new(),
        };
        Some((plan, Box::new(pass)))
    }
}

struct Replacement {
    program: Program,
    frame: Frame,
    target: Target,
    key: Key,
    cache: Arc<native_render::gels_gpu::ResidentCache>,
    generation: u64,
}

impl Replacement {
    fn snapshot(
        snapshot: &DrawSnapshot<'_>,
        scope: Scope,
        cache: Arc<native_render::gels_gpu::ResidentCache>,
    ) -> Result<Self, String> {
        let draw = snapshot.draw;
        if draw.primitive != PrimitiveType::TriangleStrip
            || draw.vertex_count != 4
            || draw.indices.is_some()
            || draw.shader.provided_inputs != INPUTS
            || !draw.clip_rects.is_empty()
            || !draw
                .clip
                .is_some_and(|r| r.is_whole(scope.target.size.0, scope.target.size.1))
            || draw.viewport.x != 0
            || draw.viewport.y != 0
            || (draw.viewport.width, draw.viewport.height) != scope.target.size
            || draw.viewport.min_z != 0.0
            || draw.viewport.max_z != 1.0
        {
            return Err("quad declaration, shape, viewport or scissor is unresolved".into());
        }
        let vertices = vertices(&draw.layout, &draw.streams)?;
        let rs = &draw.state.render;
        if !draw.state.bad.is_empty()
            || rs.depth_mode != DepthBufferMode::Disabled
            || rs.stencil.enable
            || rs.fog_enable
            || rs.alpha_test_enable
            || rs.shade_mode != ShadeMode::Gouraud
            || rs.fill_mode != FillMode::Solid
            || rs.back_fill_mode != FillMode::Solid
        {
            return Err("quad requires unsupported depth/stencil/fog/alpha or fill state".into());
        }
        if draw.extras.iter().any(|e| e.color_key_op != 0)
            || draw
                .state
                .stages
                .iter()
                .any(|s| [s.address_u, s.address_v, s.address_w].contains(&TextureAddress::Border))
        {
            return Err("quad requires colour key or border handling".into());
        }
        let d3d::shader::VertexProcessing::Program { microcode, .. } = &draw.shader.vertex else {
            return Err("gel is not a vertex program".into());
        };
        if microcode.len() % 4 != 0 {
            return Err("gel microcode is not instruction aligned".into());
        }
        let count = u16::try_from(microcode.len() / 4).map_err(|_| "gel microcode count")?;
        let mut code = vec![0x78, 0x20];
        code.extend_from_slice(&count.to_le_bytes());
        code.extend(microcode.iter().flat_map(|w| w.to_le_bytes()));
        let psdef = draw
            .shader
            .pixel_shader
            .as_ref()
            .ok_or("gel has no pixel definition")?;
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let raw = |canonical| {
            table
                .canonical_to_guest(canonical)
                .map(|n| draw.raw.render[n as usize])
                .unwrap_or(0)
        };
        let def = d3d::nv2a::guest_def(
            psdef,
            &draw.shader.combiner_state,
            draw.raw.render[d3d::state::slot::PS_TEXTURE_MODES as usize],
            draw.shader.final_combiner_latch,
        )
        .with_default_final_combiner(false, raw(103) != 0);
        let mut dims = [None; 4];
        for (n, info) in draw.texture_info.iter().enumerate() {
            if let Some(info) = info {
                if info.kind != gfx::TextureKind::D2
                    || info.width == 0
                    || info.height == 0
                    || info.format.is_depth()
                {
                    return Err(format!("gel stage {n} is not a resolved 2D texture"));
                }
                dims[n] = Some(TextureDim::D2);
            }
        }
        let options = PixelOptions {
            texture_dims: dims,
            alpha_kill: draw.extras.map(|e| e.alpha_kill & 4 != 0),
            color_sign: draw.extras.map(|e| psh::bits::color_sign(e.color_sign)),
            two_sided_lighting: raw(141) != 0,
            stipple: draw.hw.stipple_control.unwrap_or_else(|| raw(83)) != 0,
            ..Default::default()
        };
        let caps = Capabilities::from_features(wgpu::Features::empty());
        let samplers = std::array::from_fn(|n| SamplerKey::from_stage(&draw.state.stages[n], caps));
        let state = PassState {
            blend: gfx::translate::blend_state(rs),
            blend_constant: gfx::translate::blend_constant(rs),
            color_writes: gfx::translate::color_writes(rs.color_write),
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Always,
            primitive: gfx::translate::primitive_state(
                rs,
                wgpu::PrimitiveTopology::TriangleStrip,
                caps,
            ),
            alpha_test: None,
        };
        let program = Program::compile_target(&code, &def, &options, samplers, state)?;
        let mut vertex = VertexUniforms {
            constants: *draw.shader.constants,
            defaults: draw.shader.vertex_defaults,
            surface: [draw.target.width as f32, draw.target.height as f32],
            z_max: gfx::renderer::d3dz_max(draw.target.depth_format),
            ..Default::default()
        };
        if d3d::nv2a::w_buffered(draw) {
            vertex.w_clip = Some(d3d::nv2a::w_clip_range(&draw.hw, vertex.z_max));
            vertex.w_clamp = d3d::nv2a::draw_clamps_depth(draw);
        }
        let (c0, c1) = def.embedded_constants();
        let [x, y, z] = draw.hw.eye_vector;
        let mut pixel = PixelUniforms {
            c0,
            c1,
            eye: [x, y, z, 0.0],
            fog_color: psh::argb(rs.fog_color),
            fog: psh::fog::Fog::OFF.uniform(),
            stipple: draw.hw.stipple_pattern,
            ..Default::default()
        };
        for n in 0..4 {
            let e = draw.extras[n];
            pixel.bump_mat[n] = e.bump_env_mat;
            pixel.bump_lum[n] = [e.bump_env_lscale, e.bump_env_loffset, 0.0, 0.0];
            let bias = draw.state.stages[n].mip_lod_bias;
            pixel.lod_bias[n] = if bias.is_finite() { bias } else { 0.0 };
            if let Some(info) = draw.texture_info[n]
                && info.format.layout() == gfx::format::Layout::Linear
            {
                pixel.tex_scale[n] = [1.0 / info.width as f32, 1.0 / info.height as f32, 1.0, 1.0];
            }
        }
        Ok(Self {
            program,
            frame: Frame {
                vertex,
                pixel,
                vertices,
            },
            target: scope.target,
            key: scope.key,
            generation: scope.generation,
            cache,
        })
    }
}

/// Only the helper's actual float4 stream is accepted. No synthetic quad,
/// retail constants or static bitmap lookups stand in for resolved input.
fn vertices(
    layout: &gfx::vertex::VertexLayout,
    streams: &[d3d::StreamBytes],
) -> Result<[Vertex; 4], String> {
    if layout.streams.len() != streams.len() {
        return Err("gel stream count".into());
    }
    let mut vertices = [Vertex {
        registers: [[0.0; 4]; 7],
    }; 4];
    let mut supplied = 0u16;
    for (layout, stream) in layout.streams.iter().zip(streams) {
        for attribute in &layout.attributes {
            let n = usize::from(attribute.register.0);
            if n >= 7 {
                continue;
            }
            if attribute.data_type != VertexDataType::Float4
                || supplied & 1 << n != 0
                || attribute
                    .offset
                    .checked_add(16)
                    .is_none_or(|end| end > stream.stride)
            {
                return Err("gel attribute is not one unique float4".into());
            }
            for (v, vertex) in vertices.iter_mut().enumerate() {
                let start = v
                    .checked_mul(stream.stride as usize)
                    .and_then(|o| o.checked_add(attribute.offset as usize))
                    .ok_or("gel stream offset overflow")?;
                let bytes = stream
                    .data
                    .get(start..start.checked_add(16).ok_or("gel stream offset overflow")?)
                    .ok_or("gel stream is truncated")?;
                for (component, word) in vertex.registers[n].iter_mut().zip(bytes.chunks_exact(4)) {
                    *component = f32::from_bits(u32::from_le_bytes(word.try_into().unwrap()));
                }
            }
            supplied |= 1 << n;
        }
    }
    if supplied != INPUTS {
        return Err("gel attributes are incomplete".into());
    }
    Ok(vertices)
}

impl ExternalPass for Replacement {
    fn colour_only(&self) -> bool {
        true
    }
    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: true,
            depth_stencil: false,
        }
    }
    fn prepare_copy(&mut self, t: &ExternalTarget<'_>) -> Result<(), &'static str> {
        if t.guest_size != self.target.size
            || t.viewport != [0.0, 0.0, t.size.0 as f32, t.size.1 as f32]
            || t.color.texture().sample_count() != 1
            || t.color.texture().format() != t.color_format
        {
            return Err("gel target copy does not match the admitted quad");
        }
        for n in 0..4 {
            if self.program.dims[n].is_some()
                && (self.program.dims[n] != Some(TextureDim::D2) || t.stages[n].is_none())
            {
                return Err("gel sampled stage is unresolved on a required copy");
            }
        }
        Ok(())
    }
    fn record_ranges(&mut self, t: ExternalTarget<'_>) -> usize {
        let program = Program {
            wgsl: self.program.wgsl.clone(),
            dims: self.program.dims,
            samplers: self.program.samplers,
            state: self.program.state,
        };
        let result = self
            .cache
            .producer(t.device, self.generation, program, t.stages)
            .and_then(|producer| {
                producer.encode_into(
                    t.device,
                    t.encoder,
                    &self.frame,
                    t.color,
                    t.color_format,
                    t.guest_size,
                    t.size,
                )
            });
        if let Err(reason) = result {
            eprintln!("native-gels {:?}: record refused: {reason}", self.key);
            return 0;
        }
        Stage::Gels.count(Metric::Buffers, 3);
        Stage::Gels.count(Metric::BindGroups, 2);
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::vertex::{StreamLayout, VertexAttribute, VertexLayout, VertexRegister};

    fn fixture() -> (VertexLayout, Vec<d3d::StreamBytes>) {
        let layout = VertexLayout {
            pretransformed: false,
            streams: vec![StreamLayout {
                stream: 0,
                stride: 112,
                attributes: (0..7)
                    .map(|n| VertexAttribute {
                        register: VertexRegister(n),
                        data_type: VertexDataType::Float4,
                        offset: u32::from(n) * 16,
                    })
                    .collect(),
            }],
        };
        let bytes: Vec<u8> = (0..112)
            .flat_map(|n| (n as f32).to_bits().to_le_bytes())
            .collect();
        (
            layout,
            vec![d3d::StreamBytes {
                data: bytes.into(),
                stride: 112,
            }],
        )
    }

    #[test]
    fn resolved_vertices_keep_every_component_in_original_order() {
        let (layout, streams) = fixture();
        let decoded = vertices(&layout, &streams).unwrap();
        for (v, vertex) in decoded.iter().enumerate() {
            for (n, register) in vertex.registers.iter().enumerate() {
                for (c, value) in register.iter().enumerate() {
                    assert_eq!(*value, (v * 28 + n * 4 + c) as f32);
                }
            }
        }
    }

    #[test]
    fn partial_duplicate_or_non_float4_streams_are_refused() {
        let (mut layout, mut streams) = fixture();
        layout.streams[0].attributes[2].data_type = VertexDataType::Float2;
        assert!(vertices(&layout, &streams).is_err());
        layout.streams[0].attributes[2].data_type = VertexDataType::Float4;
        layout.streams[0].attributes[2].register = VertexRegister(1);
        assert!(vertices(&layout, &streams).is_err());
        layout.streams[0].attributes[2].register = VertexRegister(2);
        streams[0].data = vec![0u8; 447].into();
        assert!(vertices(&layout, &streams).is_err());
    }
}
