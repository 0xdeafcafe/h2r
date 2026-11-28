//! The decorator stage's loader (docs/graphics/native-decorators.md, "The
//! loader"): a decorator material's pass as the build picks it, its
//! program translated with the set's stream and the instance registers
//! supplied, and the set's vertices as that program reads them. Pure:
//! the material comes from `tags::material`, the set's geometry from
//! guest memory (`scene_extract::layout::decorators::set_geometry`), the
//! stream formats from `crate::object_streams`. A material that does not
//! fit stays on the D3D path with the reason.

use map::TagId;
use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};
use shaders::vsh::{self, VertexUniforms};
use tags::material::Material as TagMaterial;
use tags::shader::{RenderStateValue, StageStates};

use crate::decorators::PROVIDED_INPUTS;
use crate::engine_states::apply_static;
use crate::gpu::Image;
use crate::look::{self, PassLook, Stage};
use crate::object_program_gpu::input_fetch;
use crate::object_streams::{
    Attribute, Element, SLOT, StreamData, StreamElement, declaration, gather, registers,
};
use crate::pass_state::{PassState, geometry_pass_state};
use crate::program::{
    PassParts, VIEW_CONSTANTS, ViewInputs, constants_read, frame_pixel, frame_vertex,
    translate_pass_with_pixel, world_parity,
};

pub use scene_extract::layout::decorators::{
    DECORATOR_CLASSIFICATION, Refusal as SetRefusal, SetGeometry, set_geometry, set_geometry_with,
};

/// `c[53]`, the batch's detail-level constant.
pub const LOD_CONSTANT: usize = frame_scene::decorator::LOD_CONSTANT as usize;

/// One decorator pass as the build picks it (implementation 0 of the
/// pass the template lists at the item's detail level, layer and pass;
/// `scene_extract::layout::decorators::implementation`) and as the stage
/// draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct DecoratorLook {
    pub look: PassLook,
    pub state: PassState,
    /// The `vrtx`'s classification-1 program (`'x' 0x20` microcode) and
    /// its input semantics (register `v<i>` reads `semantics[i]`).
    pub microcode: Vec<u8>,
    pub semantics: Vec<u16>,
    /// The pass sets no `CULLMODE`: it draws with the cull and winding
    /// the device holds at the layer-3 call
    /// (`decorators_gpu::DrawKey::cull`).
    pub inherits_cull: bool,
    /// The render states (guest slots) the guest's draw of the pass
    /// leaves set once the next switch has restored it: each state of the
    /// implementation's list the restore list does not write back, at the
    /// last value the draw sets it to (its engine-driven values after the
    /// list), then the implementation's restore list ([`skip_holds`]).
    pub leaves: Vec<RenderStateValue>,
}

/// D3D's `TWOSIDEDLIGHTING` (canonical state), which the draw `0x0004e260`
/// turns on before its placements (`0x0004e2da`) and off after them
/// (`0x0004ebb4`).
pub const TWO_SIDED_LIGHTING: u32 = 141;

static STATE_TABLE: std::sync::LazyLock<gfx::xdk::RenderStateTable> =
    std::sync::LazyLock::new(|| gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD));

/// Each state `states` sets, at the last value it sets it to, in the
/// order of those last values.
fn last_values(states: &[RenderStateValue]) -> Vec<RenderStateValue> {
    let mut out: Vec<RenderStateValue> = Vec::with_capacity(states.len());
    for s in states.iter().rev() {
        if !out.iter().any(|o| o.state == s.state) {
            out.push(*s);
        }
    }
    out.reverse();
    out
}

/// Whether skipping the guest's draw of a placement whose pass is `look`
/// leaves the device's render states as drawing it would (`read`: the
/// device's render state at a guest slot, at the layer-3 call), the rule
/// `crate::layer_pass::skip_holds` applies to a world layer's items
/// (docs/graphics/native-decorators.md, "The glue"; **Code**, `h2
/// disasm`): the draw's shader begin `0x0001bbf0` sets the pass's list
/// and its engine-driven values without comparing them with the device
/// (`0x0001a170`; those values come from the shader instance's own inputs,
/// [`decorator_look`]), the next pass switch or layer setup restores the
/// implementation's restore list (`0x00018900`) before another
/// implementation's list is set (`0x00016b90`), so a state the list sets
/// and the restore list writes back is left at the restore's value, and
/// the draw leaves `TwoSidedLighting` off. Skipped, none of it runs. The
/// two agree exactly when every state of [`DecoratorLook::leaves`] and
/// `TwoSidedLighting` off already hold; a state that does not read
/// refuses.
pub fn skip_holds(look: &DecoratorLook, read: &dyn Fn(u32) -> Option<u32>) -> bool {
    let two_sided = STATE_TABLE.canonical_to_guest(TWO_SIDED_LIGHTING);
    two_sided.and_then(read) == Some(0)
        && look
            .leaves
            .iter()
            .all(|s| read(u32::from(s.state)) == Some(s.value))
}

/// The pass of `material` at detail level `lod`, layer `layer`, pass
/// `pass`, implementation 0, or why the stage does not draw it.
/// `program` gives a `vrtx` tag's classification-1 program and semantics
/// (`crate::look::vertex_program`, `vertex_semantics`).
pub fn decorator_look(
    material: &TagMaterial,
    lod: u8,
    layer: u8,
    pass: u8,
    program: &mut dyn FnMut(TagId) -> Option<(Vec<u8>, Vec<u16>)>,
) -> Result<DecoratorLook, String> {
    let list = material
        .layers
        .iter()
        .find(|l| l.lod == usize::from(lod) && l.layer == usize::from(layer))
        .ok_or_else(|| format!("the template has no layer {layer} at detail level {lod}"))?;
    let p = list
        .passes
        .get(usize::from(pass))
        .ok_or_else(|| format!("the layer has no pass {pass}"))?;
    let imp = p
        .implementations
        .first()
        .ok_or("the pass has no implementation")?;
    // Engine-driven states over the pass's own, from the instance's
    // inputs: the stage translates the pass once for every draw.
    let applied =
        apply_static(imp, &material.inputs).map_err(|r| format!("engine-driven states: {r:?}"))?;
    // A pass that leaves the cull to the device draws as the draws before
    // it left it: the device's at the layer-3 call, read at the ask.
    let inherits_cull =
        !look::sets_state(&imp.render_states, gfx::state::RenderStateType::CullMode);
    // A stage sampling the lightmap takes the globals' bitmap (the draw
    // sets the page to -1, `0x0004e277`): [`stage_images`].
    let mut look = look::implementation_look(p.pass, imp).map_err(|r| format!("{r:?}"))?;
    look.stage_states = applied.stage_states;
    let state = geometry_pass_state(&applied.render_states)
        .and_then(PassState::device_stencil_only)
        .and_then(PassState::no_depth_offset)
        .map_err(|r| format!("render states: {r:?}"))?;
    let (microcode, semantics) =
        program(look.vertex_shader).ok_or("the vrtx has no classification-1 program")?;
    // A state the restore list writes back is left at the restore's
    // value, whatever the list set it to.
    let restored = |s: &RenderStateValue| imp.restore_states.iter().any(|r| r.state == s.state);
    let mut leaves: Vec<RenderStateValue> = last_values(&applied.render_states)
        .into_iter()
        .filter(|s| !restored(s))
        .collect();
    leaves.extend_from_slice(&imp.restore_states);
    Ok(DecoratorLook {
        look,
        state,
        microcode,
        semantics,
        inherits_cull,
        leaves,
    })
}

/// A decorator pass translated for one set's stream.
#[derive(Clone, Debug)]
pub struct DecoratorProgram {
    /// The linked module (`vs_main`, `fs_main`).
    pub wgsl: String,
    pub stages: [Stage; 4],
    pub samplers: [Option<StageStates>; 4],
    /// The texture each stage's fetch expects (`None`: not sampled).
    pub dims: [Option<TextureDim>; 4],
    /// Per stage, the channels read as signed bytes (the image is stored
    /// biased for them, `crate::program::bias_signed`).
    pub signed: [[bool; 4]; 4],
    /// The uniforms but for the view's constants and `c[53]`.
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    pub state: PassState,
    /// The registers the set's stream supplies, in vertex-buffer order.
    pub declaration: Vec<Element>,
    /// [`DecoratorLook::inherits_cull`].
    pub inherits_cull: bool,
}

/// The pixel shader as the draw has it: `TwoSidedLighting` on
/// (`0x0004e2da`), so a back face reads the back colours.
fn two_sided(def: &PixelShaderDef, options: &PixelOptions) -> psh::PixelShader {
    psh::translate(
        def,
        &PixelOptions {
            two_sided_lighting: true,
            ..*options
        },
    )
}

/// Whether the stage sets `c[c]`: the view's (`c[0..18]`, from the device
/// at the pass), the instance's, and `c[53]` (per batch).
fn sets_constant(look: &PassLook, c: u8) -> bool {
    usize::from(c) < VIEW_CONSTANTS
        || usize::from(c) == LOD_CONSTANT
        || look
            .vertex_constants
            .iter()
            .any(|r| r.index == c && r.written != 0)
}

/// Translates `look` for a set whose stream has the format `stream` (its
/// type's record, `crate::object_streams::StreamFormats::elements`), or
/// says why the stage does not draw it:
///
/// - nothing computed per draw, and nothing the D3D path takes from the
///   device that the stage does not have (`crate::program::world_parity`);
/// - every constant the program reads is one the stage sets
///   ([`sets_constant`]), none addressed relative to `a0.x`;
/// - the stream supplies the registers the program reads but
///   `v11..v15`, which the instance buffer supplies, and none of those.
pub fn compile(look: &DecoratorLook, stream: &[StreamElement]) -> Result<DecoratorProgram, String> {
    let pass = &look.look;
    if pass.per_draw_inputs {
        return Err("per-draw engine inputs".into());
    }
    world_parity(pass)?;
    let program =
        vsh::Program::parse(&look.microcode).map_err(|e| format!("vertex program: {e:?}"))?;
    if let Some(c) = constants_read(&program)?
        .into_iter()
        .find(|&c| !sets_constant(pass, c))
    {
        return Err(format!(
            "vertex program reads c[{c}], which the decorator stage does not set"
        ));
    }
    let declaration = declaration(&look.semantics, &[stream.to_vec()]);
    let streamed = registers(&declaration);
    if streamed & PROVIDED_INPUTS != 0 {
        return Err(format!(
            "the set's stream supplies an instance register ({:#06x})",
            streamed & PROVIDED_INPUTS
        ));
    }
    if declaration.is_empty() {
        return Err("the set's stream supplies no register the program reads".into());
    }
    let t = translate_pass_with_pixel(
        &PassParts {
            microcode: &look.microcode,
            pixel_shader: &pass.pixel_shader,
            dims: pass
                .textures
                .map(|s| (s != Stage::Empty).then_some(TextureDim::D2)),
            biased: pass.textures.map(|s| matches!(s, Stage::Bitmap { .. })),
            stage_states: &pass.stage_states,
            vertex_constants: &pass.vertex_constants,
            vertex_defaults: &pass.vertex_defaults,
            alpha_test: look.state.alpha_test,
            provided_inputs: streamed | PROVIDED_INPUTS,
            input_fetch: input_fetch(&declaration),
            w_buffer: true,
            shadow: [None; 4],
        },
        two_sided,
    )?;
    Ok(DecoratorProgram {
        wgsl: t.wgsl,
        stages: pass.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex: t.vertex,
        pixel: t.pixel,
        state: look.state,
        declaration,
        inherits_cull: look.inherits_cull,
    })
}

/// The images `program`'s sampled stages bind: a bitmap stage's from
/// `bitmap` (bitmap, image), a lightmap stage's `lightmap`. The draw
/// `0x0004e260` sets the current lightmap page to -1 (`0x0004e277`), so
/// the stage setup binds the globals' rasterizer bitmap at `+0x64` as for
/// an object's record, and the draw adds no lightmap coordinates: the
/// stage takes that image only when it is uniform
/// (`crate::object_program_gpu::uniform`), which samples the same
/// wherever the coordinates fall. An error says why the material stays on
/// the D3D path.
pub fn stage_images(
    program: &DecoratorProgram,
    bitmap: &dyn Fn(TagId, u32) -> Option<Image>,
    lightmap: Option<&Image>,
) -> Result<[Option<Image>; 4], String> {
    let mut images: [Option<Image>; 4] = Default::default();
    for (n, dim) in program.dims.iter().enumerate() {
        if dim.is_none() {
            continue;
        }
        images[n] = Some(match program.stages[n] {
            Stage::Bitmap { bitmap: id, image } => {
                bitmap(id, image).ok_or_else(|| format!("stage {n} bitmap did not load"))?
            }
            Stage::Lightmap => {
                let image = lightmap
                    .ok_or_else(|| format!("stage {n} samples the lightmap, which did not load"))?;
                if !crate::object_program_gpu::uniform(image) {
                    return Err(format!(
                        "stage {n} samples the lightmap, whose image ({}x{}) is not uniform, so the coordinates the draw leaves would matter",
                        image.width, image.height
                    ));
                }
                image.clone()
            }
            _ => return Err(format!("stage {n} samples what is not a bitmap")),
        });
    }
    Ok(images)
}

/// A set's vertices as one program reads them, and its indices: per
/// vertex, per register of the program's declaration, the guest bytes in
/// a [`SLOT`] (`crate::object_streams::gather`), converted by the vertex
/// unit as the D3D path's fetch converts them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecoratorBuffers {
    pub registers: Vec<u8>,
    pub formats: Vec<Attribute>,
    pub vertices: Vec<[u8; SLOT]>,
    pub indices: Vec<u16>,
}

/// `set`'s geometry for `program`, or why it does not read.
pub fn buffers(program: &DecoratorProgram, set: &SetGeometry) -> Result<DecoratorBuffers, String> {
    if set.stride == 0 {
        return Err("the set's stream has no stride".into());
    }
    let count = set.vertex_count();
    if set.indices.iter().any(|&i| usize::from(i) >= count) {
        return Err("an index names a vertex past the set's stream".into());
    }
    let stream = StreamData {
        bytes: &set.vertices,
        stride: usize::from(set.stride),
    };
    let vertices = gather(&program.declaration, &[Some(stream)], count)?;
    Ok(DecoratorBuffers {
        registers: program.declaration.iter().map(|e| e.register).collect(),
        formats: program.declaration.iter().map(|e| e.format).collect(),
        vertices,
        indices: set.indices.clone(),
    })
}

/// A batch's uniforms: the program's with the view's constants, surface
/// and depth, and `c[53]` the batch's.
pub fn uniforms(
    program: &DecoratorProgram,
    view: &ViewInputs,
    lod_constant: [f32; 4],
) -> (VertexUniforms, PixelUniforms) {
    let mut vertex = frame_vertex(&program.vertex, view);
    vertex.constants[LOD_CONSTANT] = lod_constant;
    (vertex, frame_pixel(&program.pixel, view))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use shaders::vsh::encode::{Inst, Operand as O, program};
    use shaders::vsh::{MacOp, OutReg};

    /// A pass with no texture and the final combiner D3D programs for
    /// the fixed-function path (the colour is `r0`): the stage refuses a
    /// definition without one (`crate::program::world_parity`).
    pub(crate) fn look() -> PassLook {
        let (final_inputs_abcd, final_inputs_efg) = psh::d3d_default_final_combiner(false, false);
        PassLook {
            pass: TagId(0xe000_0001),
            implementation: 0,
            vertex_shader: TagId(0xe000_0002),
            textures: [Stage::Empty; 4],
            stage_states: Vec::new(),
            pixel_shader: PixelShaderDef {
                final_inputs_abcd,
                final_inputs_efg,
                ..Default::default()
            }
            .to_bytes(),
            vertex_constants: Vec::new(),
            vertex_defaults: Vec::new(),
            per_draw_inputs: false,
        }
    }

    pub(crate) fn state() -> PassState {
        PassState {
            blend: None,
            blend_constant: wgpu::Color::BLACK,
            color_writes: wgpu::ColorWrites::ALL,
            depth_write: true,
            depth_compare: wgpu::CompareFunction::Always,
            primitive: wgpu::PrimitiveState::default(),
            alpha_test: None,
            stencil: None,
            depth_offset: None,
        }
    }

    /// The model vertex `v0` moved to `r1` (an instruction reads one input
    /// register, as the encoding has one field for it), placed by the
    /// instance's rows (`dph` with `v12..v14`), then `dph oPos, r0,
    /// c[0..3]`; the colour `v15` times `c[53]`, the normal `v11` to
    /// `oT0`, and `extra` read as well.
    pub(crate) fn code(extra: Option<O>) -> Vec<u8> {
        let mut code = vec![Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_temp(1, 0xf)];
        for k in 0..3u8 {
            code.push(
                Inst::mac(MacOp::Dph, Some(O::r(1)), Some(O::v(12 + k)), None).to_temp(0, 8 >> k),
            );
        }
        for k in 0..4u8 {
            code.push(
                Inst::mac(MacOp::Dph, Some(O::r(0)), Some(O::c(k)), None).to_out(
                    false,
                    OutReg::Pos,
                    8 >> k,
                ),
            );
        }
        code.push(
            Inst::mac(
                MacOp::Mul,
                Some(O::v(15)),
                Some(O::c(LOD_CONSTANT as u8)),
                None,
            )
            .to_out(false, OutReg::D0, 0xf),
        );
        code.push(Inst::mac(MacOp::Mov, Some(O::v(11)), None, None).to_out(
            false,
            OutReg::T(0),
            0xf,
        ));
        if let Some(extra) = extra {
            code.push(Inst::mac(MacOp::Mov, Some(extra), None, None).to_out(
                false,
                OutReg::T(1),
                0xf,
            ));
        }
        program(&code)
    }

    /// Semantic 0 (a three-float position) in `v0`.
    pub(crate) fn decorator(extra: Option<O>) -> DecoratorLook {
        DecoratorLook {
            look: look(),
            state: state(),
            microcode: code(extra),
            semantics: vec![0],
            inherits_cull: false,
            leaves: Vec::new(),
        }
    }

    pub(crate) fn stream() -> Vec<StreamElement> {
        vec![StreamElement::Attribute {
            semantic: 0,
            format: Attribute::Float(3),
        }]
    }

    #[test]
    fn the_program_takes_the_position_from_the_stream_and_v11_to_v15_from_the_instances() {
        let p = compile(&decorator(None), &stream()).unwrap();
        assert_eq!(p.declaration.len(), 1);
        assert_eq!(
            (p.declaration[0].register, p.declaration[0].format),
            (0, Attribute::Float(3))
        );
        assert!(p.wgsl.contains("vs_main") && p.wgsl.contains("fs_main"));
        assert_eq!(p.state, state());
    }

    #[test]
    fn what_the_stage_does_not_set_or_supply_is_refused() {
        let e = compile(&decorator(Some(O::c(60))), &stream()).unwrap_err();
        assert!(e.contains("c[60]"), "{e}");
        let e = compile(&decorator(Some(O::v(1))), &stream()).unwrap_err();
        assert!(e.contains("do not supply"), "{e}");
        // A stream that supplies an instance register.
        let mut d = decorator(None);
        d.semantics = (0..13).collect();
        let s = vec![
            StreamElement::Attribute {
                semantic: 12,
                format: Attribute::Float(4),
            },
            stream()[0],
        ];
        assert!(compile(&d, &s).unwrap_err().contains("instance register"));
        // Nothing streamed that the program reads.
        let mut d = decorator(None);
        d.semantics = vec![7];
        assert!(compile(&d, &stream()).is_err());
        let mut d = decorator(None);
        d.look.per_draw_inputs = true;
        assert!(compile(&d, &stream()).unwrap_err().contains("per-draw"));
    }

    #[test]
    fn a_sets_vertices_are_gathered_by_the_declaration_and_c53_is_the_batchs() {
        let p = compile(&decorator(None), &stream()).unwrap();
        let positions: [[f32; 3]; 2] = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        let set = SetGeometry {
            stream_type: 1,
            stride: 12,
            vertices: positions
                .iter()
                .flatten()
                .flat_map(|f| f.to_le_bytes())
                .collect(),
            indices: vec![0, 1, 1],
        };
        let b = buffers(&p, &set).unwrap();
        assert_eq!(
            (b.registers.as_slice(), b.formats.as_slice()),
            (&[0u8][..], &[Attribute::Float(3)][..])
        );
        assert_eq!(b.vertices.len(), 2);
        assert_eq!(b.vertices[1][..12], set.vertices[12..]);
        assert_eq!(b.indices, [0, 1, 1]);
        assert!(
            buffers(
                &p,
                &SetGeometry {
                    stride: 0,
                    ..set.clone()
                }
            )
            .is_err()
        );
        let short = SetGeometry {
            vertices: set.vertices[..20].to_vec(),
            ..set
        };
        assert!(buffers(&p, &short).is_err(), "a vertex past the stream");
        let view = ViewInputs {
            constants: [[0.5; 4]; VIEW_CONSTANTS],
            guest_size: (8, 8),
            size: (8, 8),
            z_scale: 1.0,
            w_clip: None,
        };
        let (v, _) = uniforms(&p, &view, [1.0, 2.0, 0.0, 1.0]);
        assert_eq!(v.constants[LOD_CONSTANT], [1.0, 2.0, 0.0, 1.0]);
        assert_eq!(v.constants[0], [0.5; 4]);
    }

    /// An engine-driven blend-colour alpha from the instance's scalar 21
    /// goes over the pass's own colour; one from a draw's input refuses.
    #[test]
    fn a_pass_takes_its_engine_driven_states_from_the_instance() {
        use gfx::state::RenderStateType as R;
        use tags::material::{EngineInputs, ResolvedLayer, ResolvedPass};
        use tags::shader::{ExternalBinding, RenderStateValue};
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = |t| {
            (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .expect("a 5849 slot")
        };
        let rs = |t, value| RenderStateValue {
            state: slot(t),
            value,
        };
        let mut imp = crate::engine_states::tests::implementation(
            vec![
                rs(R::AlphaBlendEnable, 0),
                rs(R::ZWriteEnable, 1),
                rs(R::BlendColor, 0x00ff_ffff),
            ],
            0,
        );
        imp.input_reads.scalars = 1 << 21;
        imp.engine_render_states = vec![ExternalBinding {
            bytes: [0, 1, 0, 21],
        }];
        let mut material = TagMaterial {
            shader: TagId(0xe000_0100),
            template: TagId(0xe000_0101),
            inputs: EngineInputs::default(),
            layers: vec![ResolvedLayer {
                lod: 0,
                layer: 3,
                passes: vec![ResolvedPass {
                    index: 0,
                    pass: TagId(0xe000_0200),
                    implementations: vec![imp],
                }],
            }],
        };
        material.inputs.scalars[21] = 1.0;
        let mut program = |_: TagId| Some((code(None), vec![0]));
        let look = decorator_look(&material, 0, 3, 0, &mut program).unwrap();
        assert_eq!(look.state.blend_constant, wgpu::Color::WHITE);
        material.layers[0].passes[0].implementations[0].engine_render_states =
            vec![ExternalBinding {
                bytes: [0, 1, 0, 3],
            }];
        let e = decorator_look(&material, 0, 3, 0, &mut program).unwrap_err();
        assert!(
            e.contains("engine-driven states") && e.contains("PerDraw"),
            "{e}"
        );
    }

    /// A placement's guest draw is skipped only where what it leaves set
    /// already holds: its list at the values it sets (an engine-driven
    /// value over the list's own), its restore list, and `TwoSidedLighting`
    /// off.
    #[test]
    fn a_skipped_placement_holds_only_when_what_its_draw_leaves_already_holds() {
        use gfx::state::RenderStateType as R;
        use std::collections::HashMap;
        use tags::material::{EngineInputs, ResolvedLayer, ResolvedPass};
        use tags::shader::ExternalBinding;
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = |t| {
            (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .expect("a 5849 slot")
        };
        let rs = |t, value| RenderStateValue {
            state: slot(t),
            value,
        };
        let mut imp = crate::engine_states::tests::implementation(
            vec![
                rs(R::AlphaBlendEnable, 0),
                rs(R::ZWriteEnable, 1),
                rs(R::BlendColor, 0x00ff_ffff),
            ],
            0,
        );
        imp.restore_states = vec![rs(R::AlphaTestEnable, 0)];
        // The blend colour's alpha from the instance's scalar 21 (1.0).
        imp.input_reads.scalars = 1 << 21;
        imp.engine_render_states = vec![ExternalBinding {
            bytes: [0, 1, 0, 21],
        }];
        let mut material = TagMaterial {
            shader: TagId(0xe000_0100),
            template: TagId(0xe000_0101),
            inputs: EngineInputs::default(),
            layers: vec![ResolvedLayer {
                lod: 0,
                layer: 3,
                passes: vec![ResolvedPass {
                    index: 0,
                    pass: TagId(0xe000_0200),
                    implementations: vec![imp],
                }],
            }],
        };
        material.inputs.scalars[21] = 1.0;
        let mut program = |_: TagId| Some((code(None), vec![0]));
        let look = decorator_look(&material, 0, 3, 0, &mut program).unwrap();
        let blend = rs(R::BlendColor, 0xffff_ffff);
        assert_eq!(
            look.leaves,
            [
                rs(R::AlphaBlendEnable, 0),
                rs(R::ZWriteEnable, 1),
                blend,
                rs(R::AlphaTestEnable, 0)
            ],
            "the driven value in place of the list's"
        );
        let two_sided = table
            .canonical_to_guest(TWO_SIDED_LIGHTING)
            .expect("a 5849 slot");
        let mut device: HashMap<u32, u32> = look
            .leaves
            .iter()
            .map(|s| (u32::from(s.state), s.value))
            .collect();
        device.insert(two_sided, 0);
        let holds =
            |device: &HashMap<u32, u32>| skip_holds(&look, &|slot| device.get(&slot).copied());
        assert!(holds(&device));
        let with = |slot: u8, value: u32| {
            let mut d = device.clone();
            d.insert(u32::from(slot), value);
            d
        };
        assert!(
            !holds(&with(blend.state, 0x00ff_ffff)),
            "the draw would set the driven value"
        );
        assert!(
            !holds(&with(slot(R::AlphaTestEnable), 1)),
            "the switch after it would restore AlphaTestEnable"
        );
        assert!(
            !holds(&with(slot(R::ZWriteEnable), 0)),
            "the draw would set ZWriteEnable"
        );
        let mut lit = device.clone();
        lit.insert(two_sided, 1);
        assert!(!holds(&lit), "the draw leaves two-sided lighting off");
        let mut unread = device.clone();
        unread.remove(&u32::from(slot(R::AlphaBlendEnable)));
        assert!(!holds(&unread), "a state that does not read");
    }

    /// A pass that turns depth writes off and its restore list back on is
    /// skipped over a device that holds them on: the switch after the
    /// guest's draw would leave them on, as skipping it does.
    #[test]
    fn a_placement_whose_restore_list_writes_back_what_it_sets_holds() {
        use gfx::state::RenderStateType as R;
        use std::collections::HashMap;
        use tags::material::{EngineInputs, ResolvedLayer, ResolvedPass};
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = |t| {
            (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .expect("a 5849 slot")
        };
        let rs = |t, value| RenderStateValue {
            state: slot(t),
            value,
        };
        let mut imp = crate::engine_states::tests::implementation(
            vec![rs(R::AlphaBlendEnable, 0), rs(R::ZWriteEnable, 0)],
            0,
        );
        imp.restore_states = vec![rs(R::ZWriteEnable, 1)];
        let material = TagMaterial {
            shader: TagId(0xe000_0100),
            template: TagId(0xe000_0101),
            inputs: EngineInputs::default(),
            layers: vec![ResolvedLayer {
                lod: 0,
                layer: 3,
                passes: vec![ResolvedPass {
                    index: 0,
                    pass: TagId(0xe000_0200),
                    implementations: vec![imp],
                }],
            }],
        };
        let mut program = |_: TagId| Some((code(None), vec![0]));
        let look = decorator_look(&material, 0, 3, 0, &mut program).unwrap();
        assert_eq!(
            look.leaves,
            [rs(R::AlphaBlendEnable, 0), rs(R::ZWriteEnable, 1)],
            "the restore's value, not the list's"
        );
        let two_sided = table
            .canonical_to_guest(TWO_SIDED_LIGHTING)
            .expect("a 5849 slot");
        let device = |z: u32| -> HashMap<u32, u32> {
            [
                (u32::from(slot(R::AlphaBlendEnable)), 0),
                (u32::from(slot(R::ZWriteEnable)), z),
                (two_sided, 0),
            ]
            .into_iter()
            .collect()
        };
        let holds =
            |device: &HashMap<u32, u32>| skip_holds(&look, &|slot| device.get(&slot).copied());
        assert!(
            holds(&device(1)),
            "depth writes off for the draw, back on after it"
        );
        assert!(
            !holds(&device(0)),
            "the restore would turn on depth writes the device holds off"
        );
    }

    /// A lightmap stage binds the globals' bitmap when it is uniform, as
    /// an object's does; a bitmap stage its own image.
    #[test]
    fn a_lightmap_stage_takes_the_uniform_globals_bitmap() {
        let mut p = compile(&decorator(None), &stream()).unwrap();
        let base = TagId(0xe000_0007);
        p.stages = [
            Stage::Bitmap {
                bitmap: base,
                image: 0,
            },
            Stage::Lightmap,
            Stage::Empty,
            Stage::Empty,
        ];
        p.dims = [Some(TextureDim::D2), Some(TextureDim::D2), None, None];
        let image = |rgba: Vec<u8>| Image::from_rgba(2, 1, rgba, Vec::new());
        let grey = image(vec![9, 9, 9, 255, 9, 9, 9, 255]);
        let varied = image(vec![9, 9, 9, 255, 1, 2, 3, 255]);
        let bitmap = |id: TagId, n: u32| (id == base && n == 0).then(|| varied.clone());
        let got = stage_images(&p, &bitmap, Some(&grey)).unwrap();
        assert_eq!(
            (got[0].as_ref(), got[1].as_ref(), got[2].as_ref()),
            (Some(&varied), Some(&grey), None)
        );
        assert!(
            stage_images(&p, &bitmap, Some(&varied))
                .unwrap_err()
                .contains("not uniform")
        );
        assert!(
            stage_images(&p, &bitmap, None)
                .unwrap_err()
                .contains("did not load")
        );
        assert!(
            stage_images(&p, &|_, _| None, Some(&grey))
                .unwrap_err()
                .contains("stage 0 bitmap")
        );
    }
}
