//! Layer-10 caster materials compiled without a guest geometry handler
//! (docs/graphics/native-shadow-layers.md, "The compiler"). The pass uses
//! the material's instance inputs, its own bitmap stages, and the shadow
//! projection in `c[40..48]`. Light-only inputs and engine textures keep
//! the layer on its resolved-draw path.

use tags::material::{InputReads, Material};

use shaders::vsh::VertexUniforms;

use crate::light_look::{self, LightLook, LightProgram, LightRefusal, LightStage};
use crate::program::{PassParts, set_constants, translate_pass};

pub const CASTER_LAYER: usize = 10;
pub const PROJECTION_REGISTER: usize = 40;
pub const PROJECTION_DWORDS: usize = 36;

/// A caster pass from the instance-resolved `material`, or why it stays
/// on the resolved-draw path. Layer 10 has no light inputs of its own.
pub fn shadow_look(material: &Material, implementation: usize) -> Result<LightLook, LightRefusal> {
    let look = light_look::light_look(material, CASTER_LAYER, implementation)?;
    let imp = material
        .layers
        .iter()
        .filter(|l| l.lod == 0 && l.layer == CASTER_LAYER)
        .flat_map(|l| &l.passes)
        .flat_map(|p| &p.implementations)
        .find(|i| i.index == implementation)
        .expect("light_look found the implementation");
    let r = imp.input_reads;
    let light_inputs = InputReads {
        scalars: r.scalars & light_look::LIGHT_SCALARS,
        vectors: r.vectors & light_look::LIGHT_VECTORS,
        ints: r.ints & light_look::LIGHT_INTS,
    };
    if light_inputs != InputReads::default() {
        return Err(LightRefusal::Reads(light_inputs));
    }
    for stage in look.textures {
        let id = match stage {
            LightStage::Empty | LightStage::Bitmap { .. } => continue,
            LightStage::Normalization => 1,
            LightStage::Attenuation => 9,
            LightStage::Gel => 10,
        };
        return Err(LightRefusal::EngineTexture(id));
    }
    Ok(look)
}

/// Translate a structure caster pass over the world's vertex streams.
pub fn compile_shadow(look: &LightLook, microcode: &[u8]) -> Result<LightProgram, String> {
    let options = light_look::vertex_options();
    compile(
        look,
        microcode,
        options.provided_inputs,
        options.input_fetch,
        false,
    )
}

/// Translate a model caster pass over the declaration its section's
/// resident buffer supplies, with node rows and decompression.
pub fn compile_shadow_model(
    look: &LightLook,
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
) -> Result<LightProgram, String> {
    compile(look, microcode, provided, fetch, true)
}

fn compile(
    look: &LightLook,
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
    model: bool,
) -> Result<LightProgram, String> {
    let t = translate_pass(&PassParts {
        microcode,
        pixel_shader: &look.pixel_shader,
        dims: look.textures.map(light_look::stage_dim),
        biased: look
            .textures
            .map(|s| matches!(s, LightStage::Bitmap { .. })),
        stage_states: &look.stage_states,
        vertex_constants: &look.vertex_constants,
        vertex_defaults: &look.vertex_defaults,
        alpha_test: look.state.alpha_test,
        provided_inputs: provided,
        input_fetch: fetch,
        w_buffer: true,
    })?;
    crate::program::pixel_parity(&t.def, &look.stage_states, t.dims.map(|d| d.is_some()))?;
    if look.vertex_constants.iter().any(|r| {
        r.written != 0
            && (PROJECTION_REGISTER..PROJECTION_REGISTER + PROJECTION_DWORDS / 4)
                .contains(&usize::from(r.index))
    }) {
        return Err("material writes into the shadow projection constants".into());
    }
    let vs = shaders::vsh::Program::parse(microcode)
        .map_err(|e| format!("shadow vertex program: {e:?}"))?;
    let set = set_constants(&look.vertex_constants);
    let projection = |c: u8| {
        (PROJECTION_REGISTER..PROJECTION_REGISTER + PROJECTION_DWORDS / 4).contains(&usize::from(c))
    };
    if model {
        use crate::program::{MODEL_ROWS, POSITION_DECOMPRESSION};
        for i in &vs.instructions {
            for s in i.sources() {
                let shaders::vsh::SrcReg::Const { index, relative } = s.reg else {
                    continue;
                };
                if relative && !(MODEL_ROWS..POSITION_DECOMPRESSION).contains(&usize::from(index)) {
                    return Err(format!(
                        "shadow vertex program reads c[a0.x + {index}], outside the node rows"
                    ));
                }
                if !relative && !set(index) && !projection(index) {
                    return Err(format!("shadow vertex program reads unsupplied c[{index}]"));
                }
            }
        }
    } else if let Some(c) = crate::program::constants_read(&vs)?
        .into_iter()
        .find(|&c| !set(c) && !projection(c))
    {
        return Err(format!("shadow vertex program reads unsupplied c[{c}]"));
    }
    let words = microcode[4..4 + 16 * vs.instructions.len()]
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        .collect();
    Ok(LightProgram {
        words,
        def: t.def,
        pixel_options: t.pixel_options,
        inputs: t.inputs,
        inherits_cull: look.inherits_cull,
        wgsl: t.wgsl,
        stages: look.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex: t.vertex,
        pixel: t.pixel,
        colours: look.vector_bindings.clone(),
        state: look.state,
    })
}

/// `base` with the group's 36 projection words in `c[40..48]`.
pub fn shadow_vertex(
    base: &VertexUniforms,
    projection: &[u32; PROJECTION_DWORDS],
) -> VertexUniforms {
    let mut u = base.clone();
    for (reg, words) in u.constants
        [PROJECTION_REGISTER..PROJECTION_REGISTER + PROJECTION_DWORDS / 4]
        .iter_mut()
        .zip(projection.chunks_exact(4))
    {
        *reg = std::array::from_fn(|k| f32::from_bits(words[k]));
    }
    u
}

#[cfg(test)]
mod tests {
    use map::TagId;
    use tags::material::Register;

    use super::*;

    fn program() -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let row = |c, mask| {
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(
                false,
                OutReg::Pos,
                mask,
            )
        };
        program(&[
            row(0, 8),
            row(1, 4),
            row(2, 2),
            row(3, 1),
            Inst::mac(MacOp::Mov, Some(O::c(40)), None, None).to_out(false, OutReg::T(0), 0xf),
        ])
    }

    fn look() -> LightLook {
        let mut def = shaders::PixelShaderDef::default();
        (def.final_inputs_abcd, def.final_inputs_efg) =
            shaders::psh::d3d_default_final_combiner(false, false);
        LightLook {
            layer: CASTER_LAYER,
            pass: TagId(0xe000_0200),
            implementation: 2,
            vertex_shader: TagId(0xe000_0010),
            textures: [LightStage::Empty; 4],
            stage_states: Vec::new(),
            pixel_shader: def.to_bytes(),
            vertex_constants: Vec::new(),
            vertex_defaults: Vec::new(),
            vector_bindings: Vec::new(),
            state: crate::pass_state::PassState {
                blend: None,
                blend_constant: wgpu::Color::BLACK,
                color_writes: wgpu::ColorWrites::ALL,
                depth_write: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                primitive: wgpu::PrimitiveState::default(),
                alpha_test: None,
            },
            inherits_cull: false,
        }
    }

    #[test]
    fn a_caster_compiles_wgsl_with_its_projection_and_no_light_frame() {
        let p = compile_shadow(&look(), &program()).expect("caster program");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        let mut base = VertexUniforms::default();
        base.constants[39] = [7.0; 4];
        base.constants[49] = [8.0; 4];
        let projection: [u32; PROJECTION_DWORDS] = std::array::from_fn(|n| (n as f32).to_bits());
        let u = shadow_vertex(&base, &projection);
        assert_eq!(u.constants[40], [0.0, 1.0, 2.0, 3.0]);
        assert_eq!(u.constants[48], [32.0, 33.0, 34.0, 35.0]);
        assert_eq!(u.constants[39], base.constants[39]);
        assert_eq!(u.constants[49], base.constants[49]);
    }

    #[test]
    fn a_material_cannot_overwrite_the_projection() {
        let mut look = look();
        look.vertex_constants.push(Register {
            index: 40,
            value: [0.0; 4],
            written: 0xf,
            origin: tags::material::Origin::Instance,
        });
        assert!(
            compile_shadow(&look, &program())
                .unwrap_err()
                .contains("projection constants")
        );
    }
}
