//! Layer-10 caster materials compiled without a guest geometry handler
//! (docs/graphics/native-shadow-layers.md, "The compiler"). The pass uses
//! the material's instance inputs, its own bitmap stages, and the shadow
//! projection in `c[40..48]`. Light-only inputs and engine textures keep
//! the layer on its resolved-draw path.

use shaders::psh::{
    AlphaFunc,
    shadow::{DepthFormat, Shadow, ShadowAddress},
};
use shaders::vsh::VertexUniforms;
use tags::material::{InputReads, Material, TextureSource};

use crate::light_look::{self, LightLook, LightProgram, LightRefusal, LightStage};
use crate::program::{PassParts, set_constants, translate_pass};

pub const CASTER_LAYER: usize = 10;
pub const PROJECTION_REGISTER: usize = 40;
pub const PROJECTION_DWORDS: usize = 36;

/// A caster pass from the instance-resolved `material`, or why it stays
/// on the resolved-draw path. Layer 10 has no light inputs of its own.
/// `selection` is the gathered entry's level of detail and pass
/// ([`light_look::selected_light_look`]); `None` takes the first level's
/// only pass.
pub fn shadow_look(
    material: &Material,
    implementation: usize,
    selection: Option<(u8, u8)>,
) -> Result<LightLook, LightRefusal> {
    let look = light_look::selected_light_look(material, CASTER_LAYER, implementation, selection)?;
    let lod = selection.map_or(0, |(variant, _)| usize::from(variant));
    let imp = material
        .layers
        .iter()
        .filter(|l| l.lod == lod && l.layer == CASTER_LAYER)
        .flat_map(|l| &l.passes)
        .filter(|p| selection.is_none_or(|(_, selector)| p.index == usize::from(selector)))
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
            LightStage::DepthShadow => 13,
            LightStage::ColourMask => 8,
        };
        return Err(LightRefusal::EngineTexture(id));
    }
    Ok(look)
}

pub const RECEIVER_LAYER: usize = 11;
pub const OPACITY_LAYER: usize = 14;

/// A receiver pass from the instance-resolved `material`. Layer 11 may
/// sample the frozen depth map, filtered colour mask and light gel; layer
/// 14 samples no shadow input. `selection` picks the level of detail and
/// pass as in [`shadow_look`].
pub fn receiver_look(
    material: &Material,
    layer: usize,
    implementation: usize,
    selection: Option<(u8, u8)>,
) -> Result<LightLook, LightRefusal> {
    if !matches!(layer, RECEIVER_LAYER | OPACITY_LAYER) {
        return Err(LightRefusal::NoLayer);
    }
    let lod = selection.map_or(0, |(variant, _)| usize::from(variant));
    let resolved = material
        .layers
        .iter()
        .find(|l| l.lod == lod && l.layer == layer)
        .filter(|l| !l.passes.is_empty())
        .ok_or(LightRefusal::NoLayer)?;
    let pass = match selection {
        Some((_, selector)) => resolved
            .passes
            .iter()
            .find(|p| p.index == usize::from(selector))
            .ok_or(LightRefusal::NoPass(selector))?,
        None => match resolved.passes.as_slice() {
            [pass] => pass,
            passes => return Err(LightRefusal::Passes(passes.len())),
        },
    };
    let imp = pass
        .implementations
        .iter()
        .find(|i| i.index == implementation)
        .ok_or(LightRefusal::NoImplementation(implementation))?;
    let vertex_shader = imp.vertex_shader.ok_or(LightRefusal::NoVertexShader)?;
    if !imp.animated.is_empty() {
        return Err(LightRefusal::Animated);
    }
    let r = imp.input_reads;
    let light_inputs = InputReads {
        scalars: r.scalars & light_look::LIGHT_SCALARS,
        vectors: r.vectors & light_look::LIGHT_VECTORS,
        ints: r.ints & light_look::LIGHT_INTS,
    };
    if light_inputs != InputReads::default() {
        return Err(LightRefusal::Reads(light_inputs));
    }
    let unsupplied = InputReads {
        scalars: r.scalars & !(light_look::LIGHT_SCALARS | tags::material::INSTANCE_SCALARS),
        vectors: r.vectors & !light_look::LIGHT_VECTORS,
        ints: r.ints & !(light_look::LIGHT_INTS | tags::material::INSTANCE_INTS),
    };
    if unsupplied != InputReads::default() {
        return Err(LightRefusal::Reads(unsupplied));
    }
    let applied = crate::engine_states::apply_static(imp, &material.inputs)
        .map_err(LightRefusal::EngineStates)?;
    let mut textures = [LightStage::Empty; 4];
    for (stage, t) in textures.iter_mut().zip(&imp.textures) {
        *stage = match t.map(|t| t.source) {
            None | Some(TextureSource::None) => LightStage::Empty,
            Some(TextureSource::Bitmap { bitmap, image, .. }) => {
                LightStage::Bitmap { bitmap, image }
            }
            Some(TextureSource::Engine(13)) if layer == RECEIVER_LAYER => LightStage::DepthShadow,
            Some(TextureSource::Engine(8)) if layer == RECEIVER_LAYER => LightStage::ColourMask,
            Some(TextureSource::Engine(10)) if layer == RECEIVER_LAYER => LightStage::Gel,
            Some(TextureSource::Engine(id)) => return Err(LightRefusal::EngineTexture(id)),
            Some(TextureSource::UnsetSlot(s)) => return Err(LightRefusal::UnsetSlot(s)),
            Some(TextureSource::Default) => return Err(LightRefusal::DefaultTexture),
            Some(TextureSource::Lightmap) => return Err(LightRefusal::Lightmap),
        };
    }
    let state =
        crate::pass_state::pass_state(&applied.render_states)
            .map_err(LightRefusal::State)?
            .device_stencil_only("shadow passes");
    Ok(LightLook {
        layer,
        pass: pass.pass,
        implementation,
        vertex_shader,
        textures,
        stage_states: applied.stage_states,
        pixel_shader: imp.pixel_shader,
        vertex_constants: imp.vertex_constants.clone(),
        vertex_defaults: imp.vertex_defaults.clone(),
        vector_bindings: imp.vector_bindings.clone(),
        inherits_cull: !crate::look::sets_state(
            &imp.render_states,
            gfx::state::RenderStateType::CullMode,
        ),
        copies_target: imp.copies_target,
        state,
    })
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
        [None; 4],
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
    compile(look, microcode, provided, fetch, true, [None; 4])
}

/// The receiver's D24 compare for each engine-13 stage, with the stage's
/// own filtering and addressing.
pub fn shadow_stages(look: &LightLook) -> [Option<Shadow>; 4] {
    std::array::from_fn(|n| {
        (look.textures[n] == LightStage::DepthShadow).then(|| {
            let state = look.stage_states.get(n);
            let point = |filter: u8| matches!(filter, 0 | 1);
            let address = |address: u8| match address {
                1 => ShadowAddress::Wrap,
                2 => ShadowAddress::Mirror,
                _ => ShadowAddress::Clamp,
            };
            Shadow {
                format: DepthFormat::D24,
                func: AlphaFunc::Greater,
                linear: state.is_some_and(|s| !point(s.min_filter) || !point(s.mag_filter)),
                address: state.map_or([ShadowAddress::Clamp; 2], |s| {
                    [address(s.address_u), address(s.address_v)]
                }),
            }
        })
    })
}

/// Translate a structure receiver pass with its D24 compare stages.
pub fn compile_receiver(look: &LightLook, microcode: &[u8]) -> Result<LightProgram, String> {
    let options = light_look::vertex_options();
    let shadow = shadow_stages(look);
    compile(
        look,
        microcode,
        options.provided_inputs,
        options.input_fetch,
        false,
        shadow,
    )
}

/// Translate a model receiver pass over its resident declaration.
pub fn compile_receiver_model(
    look: &LightLook,
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
) -> Result<LightProgram, String> {
    let shadow = shadow_stages(look);
    compile(look, microcode, provided, fetch, true, shadow)
}

fn compile(
    look: &LightLook,
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
    model: bool,
    shadow: [Option<Shadow>; 4],
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
        shadow,
    })?;
    crate::program::pixel_parity(&t.def, &look.stage_states, t.dims.map(|d| d.is_some()))?;
    if look.vertex_constants.iter().any(|r| {
        r.written != 0
            && (PROJECTION_REGISTER..PROJECTION_REGISTER + PROJECTION_DWORDS / 4)
                .contains(&usize::from(r.index))
    }) {
        return Err("material writes into the shadow projection constants".into());
    }
    // The pass's polygon offset is its fragments' (`frame_pixel` keeps it).
    let mut pixel = t.pixel;
    crate::pass_state::DepthOffset::apply(look.state.depth_offset, &mut pixel);
    let vs = shaders::vsh::Program::parse(microcode)
        .map_err(|e| format!("shadow vertex program: {e:?}"))?;
    let set = set_constants(&look.vertex_constants);
    // Layer 14 draws without a shadow projection, including after a shadowed light.
    let projection = |c: u8| {
        look.layer != OPACITY_LAYER
            && (PROJECTION_REGISTER..PROJECTION_REGISTER + PROJECTION_DWORDS / 4)
                .contains(&usize::from(c))
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
        copies_target: look.copies_target,
        animation: None,
        wgsl: t.wgsl,
        stages: look.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex: t.vertex,
        pixel,
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

    fn program(texcoord_constant: u8) -> Vec<u8> {
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
            Inst::mac(MacOp::Mov, Some(O::c(texcoord_constant)), None, None)
                .to_out(false, OutReg::T(0), 0xf),
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
                stencil: None,
                depth_offset: None,
            },
            inherits_cull: false,
            copies_target: false,
        }
    }

    #[test]
    fn a_caster_compiles_wgsl_with_its_projection_and_no_light_frame() {
        let p = compile_shadow(&look(), &program(40)).expect("caster program");
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
    fn a_shadow_pass_draws_its_polygon_offset_in_its_fragments() {
        let mut offset = look();
        offset.state.depth_offset = Some(crate::pass_state::DepthOffset { units: 3.0, slope: 1.0 });
        let p = compile_shadow(&offset, &program(40)).expect("caster program");
        assert_eq!((p.pixel.depth[1], p.pixel.depth[2]), (3.0, 1.0));
        let plain = compile_shadow(&look(), &program(40)).unwrap();
        assert_eq!((plain.pixel.depth[1], plain.pixel.depth[2]), (0.0, 0.0));
    }

    #[test]
    fn a_receiver_compiles_a_d24_compare_stage() {
        let mut look = look();
        look.layer = RECEIVER_LAYER;
        look.textures[0] = LightStage::DepthShadow;
        let p = compile_receiver(&look, &program(40)).expect("receiver program");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        let shadow = p.pixel_options.shadow[0].expect("D24 stage");
        assert_eq!(shadow.format, DepthFormat::D24);
        assert_eq!(shadow.func, AlphaFunc::Greater);
        assert!(!shadow.linear);
        assert_eq!(shadow.address, [ShadowAddress::Clamp; 2]);
        // `dims` are the stages the pixel shader samples (this test's
        // default combiner samples none); the bound stage is the option's.
        assert_eq!(
            p.pixel_options.texture_dims[0],
            Some(shaders::psh::TextureDim::D2)
        );
    }

    #[test]
    fn opacity_shaders_must_source_their_coordinates_without_a_shadow_projection() {
        let mut look = look();
        look.layer = OPACITY_LAYER;
        let options = light_look::vertex_options();
        for register in [40, 48, 0] {
            let code = program(register);
            for compiled in [
                compile_receiver(&look, &code),
                compile_receiver_model(&look, &code, options.provided_inputs, options.input_fetch),
            ] {
                if register == 0 {
                    compiled.expect("view constant is supplied on layer 14");
                } else {
                    let reason = compiled.unwrap_err();
                    assert!(
                        reason.contains(&format!("unsupplied c[{register}]")),
                        "{reason}"
                    );
                }
            }
        }
    }

    /// `layer` at the first level with passes 0 and 1 (`TagId(25)`), and
    /// at level 2 with pass 1 (`TagId(75)`); pass 1's implementation is 1.
    fn selected(layer: usize) -> Material {
        use tags::material::{ResolvedImplementation, ResolvedPass};
        let plain = |index| -> ResolvedImplementation {
            let mut imp = light_look::tests::implementation(index, TextureSource::None);
            imp.input_reads = InputReads::default();
            imp.textures[1] = None;
            imp
        };
        let mut m = light_look::tests::material(layer, vec![plain(0)]);
        let second = |pass| ResolvedPass {
            index: 1,
            pass: TagId(pass),
            implementations: vec![plain(1)],
        };
        let mut lod = m.layers[0].clone();
        lod.lod = 2;
        lod.passes = vec![second(75)];
        m.layers[0].passes.push(second(25));
        m.layers.push(lod);
        m
    }

    #[test]
    fn a_shadow_entry_selects_its_lod_and_pass() {
        for layer in [CASTER_LAYER, RECEIVER_LAYER, OPACITY_LAYER] {
            let m = selected(layer);
            let look = |imp, selection| match layer {
                CASTER_LAYER => shadow_look(&m, imp, selection),
                _ => receiver_look(&m, layer, imp, selection),
            };
            assert_eq!(look(1, Some((2, 1))).unwrap().pass, TagId(75), "layer {layer}");
            assert_eq!(look(1, Some((0, 1))).unwrap().pass, TagId(25), "layer {layer}");
            assert_eq!(look(0, Some((0, 0))).unwrap().pass, TagId(0xe000_0200), "layer {layer}");
            assert_eq!(look(1, Some((1, 1))).unwrap_err(), LightRefusal::NoLayer, "layer {layer}");
            assert_eq!(look(1, Some((2, 2))).unwrap_err(), LightRefusal::NoPass(2), "layer {layer}");
            assert_eq!(
                look(0, Some((2, 1))).unwrap_err(),
                LightRefusal::NoImplementation(0),
                "layer {layer}"
            );
            // Without a selection the first level must have one pass.
            assert_eq!(look(0, None).unwrap_err(), LightRefusal::Passes(2), "layer {layer}");
        }
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
            compile_shadow(&look, &program(40))
                .unwrap_err()
                .contains("projection constants")
        );
    }
}
