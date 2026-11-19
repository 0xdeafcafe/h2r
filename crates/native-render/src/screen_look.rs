//! Stage 5: a screen effect's layer-15 template pass (`0x00016610` from
//! `0x0002d140`; docs/graphics/native-renderer.md, "Stage 5", "The screen
//! effects") as the native renderer would draw it.
//!
//! `0x0002d140` draws each of its egor's passes through the shader's
//! layer-15 template, then pushes the strips by hand: per vertex the four
//! texture coordinates as attributes 1..4 (`0x00101a10 + 16 s`) and the
//! position as attribute 0 (`0x00101a00`), so the vertex program reads
//! `v0`..`v4` ([`STRIP_INPUTS`]).
//!
//! `0x00016610(shader, layer, pass, implementation, target, flag)`
//! (Code): the layer's passes are the template's word at its layer index
//! (low 9 bits the first, high 7 the count); `pass` -1 draws them all,
//! else only that one (`engine::screen_effect::Pass::implementation`, the
//! layer's pass), each with implementation `implementation`
//! (`Pass::selector`; `0x132` bytes each in the `spas`, its vertex shader
//! at `+0x100`, a pass whose shader is -1 not drawn). After
//! `0x0001bbf0` sets the pass up, `0x0001c590` loads that vertex shader
//! with classification 0 (`push 0`; `0x0001c2e0`: `0x1c` bytes per
//! classification in the `vrtx`), so a screen pass always runs its vertex
//! shader's first program ([`SCREEN_CLASSIFICATION`]).
//!
//! [`screen_look`] takes the egor's shader resolved
//! (`tags::material::ShaderLibrary::material`) and returns what the native
//! pass draws, or why the effect keeps its D3D draws; [`compile_screen`]
//! translates it through `program::translate_pass`.

use map::TagId;
use tags::material::{INSTANCE_INTS, INSTANCE_SCALARS, InputReads, Material, Origin, Register, TextureSource};
use tags::shader::{PSDEF_SIZE, StageStates};

use shaders::psh::{PixelUniforms, TextureDim};
use shaders::vsh::VertexUniforms;

use crate::pass_state::{PassState, StateRefusal, pass_state};
use crate::program::{PassParts, translate_pass};

/// The template layer `0x0002d140` draws.
pub const TEMPLATE_LAYER: usize = 15;

/// The vertex registers the hand-pushed strips supply: `v0` the position,
/// `v1`..`v4` the four texture coordinates.
pub const STRIP_INPUTS: u16 = 0x1f;

/// The vertex shader classification a screen pass runs (`0x00016610`
/// loads its pass's vertex shader through `0x0001c590` with 0).
pub const SCREEN_CLASSIFICATION: u32 = 0;

/// The vertex registers past the strips' that a screen program may read
/// (`screen_xform_4` moves `v5`, `v6` into the colours and `v7.w` into
/// fog): the strips do not push them, so they hold whatever an earlier
/// draw left. The native pass gives them [`VertexUniforms`]' defaults and
/// is only made for a pixel side that reads no colour or fog
/// ([`reads_vertex_colours`]), where they cannot show.
pub const LEFT_INPUTS: u16 = 0xe0;

/// What a template pass's stage binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenStage {
    Empty,
    /// A bitmap of the shader's.
    Bitmap { bitmap: TagId, image: u32 },
    /// An engine texture (a render target: the view's colour, a glow
    /// target), supplied by the draw.
    Engine(u8),
}

/// What engine texture `id` binds (`0x00019530`, a jump table at
/// `0x00019b68` indexed by id - 1; Code). Render targets go through
/// `0x00014f60`; the bitmaps are the globals' rasterizer data
/// (`[0x00485a80]`) at the offset given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineTexture {
    /// A bitmap reference in the rasterizer data, at this offset (1:
    /// `+0xc`, the normalization cube; 9: `+4`, the attenuation volume;
    /// 14 and 15: `+0x14`).
    Rasterizer(u32),
    /// Render target `n` (2 and 3: 1; 6: 3; 8: 10; 13: 9; 17: `0x13`; 19:
    /// `0x15`; 20: `0x16`; 21: 13; 22: `0x11`; 23: `0x17`).
    Target(u32),
    /// The view's bound target (`[0x004858b8]`, 5).
    View,
    /// The target the word `[0x00485a18 + 4 id]` names (24 and 25): with
    /// `0x19`..`0x1b` sized through `0x0001dd60`, the screen effects'
    /// targets.
    Slot(u32),
    /// The stage is cleared (4).
    Cleared,
    /// The light's texture (10, 12): never a screen effect's.
    Light,
    /// Chosen at run time from other state (16: a global bitmap
    /// `[0x004e6978]`; 18: `0x00025960`'s target or the rasterizer's
    /// `+0x64`; 26: from the shader's template; 27: target `0x10` or
    /// another by `[0x00485a76]`).
    Dynamic,
    /// A null jump-table entry (7, 11) or past the table (0, above 27).
    Invalid,
}

/// [`EngineTexture`] for `id`.
pub fn engine_texture(id: u8) -> EngineTexture {
    use EngineTexture::*;
    match id {
        1 => Rasterizer(0xc),
        9 => Rasterizer(4),
        14 | 15 => Rasterizer(0x14),
        2 | 3 => Target(1),
        6 => Target(3),
        8 => Target(10),
        13 => Target(9),
        17 => Target(0x13),
        19 => Target(0x15),
        20 => Target(0x16),
        21 => Target(0xd),
        22 => Target(0x11),
        23 => Target(0x17),
        5 => View,
        24 | 25 => Slot(0x0048_5a18 + 4 * u32::from(id)),
        4 => Cleared,
        10 | 12 => Light,
        16 | 18 | 26 | 27 => Dynamic,
        _ => Invalid,
    }
}

/// One implementation of an effect's template pass.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenLook {
    /// The `spas` tag.
    pub pass: TagId,
    /// Its index among the layer's passes (`0x00016610`'s third argument).
    pub layer_pass: usize,
    pub implementation: usize,
    /// The `vrtx` tag.
    pub vertex_shader: TagId,
    pub textures: [ScreenStage; 4],
    pub stage_states: Vec<StageStates>,
    pub pixel_shader: [u8; PSDEF_SIZE],
    pub vertex_constants: Vec<Register>,
    pub vertex_defaults: Vec<Register>,
    pub state: PassState,
}

impl ScreenLook {
    /// The engine textures the draw must supply, in stage order.
    pub fn engine_textures(&self) -> Vec<u8> {
        self.textures
            .iter()
            .filter_map(|t| match t {
                ScreenStage::Engine(id) => Some(*id),
                _ => None,
            })
            .collect()
    }
}

/// Why an effect's template pass stays on the D3D path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenRefusal {
    /// The template has no layer-15 pass at the first level of detail.
    NoLayer,
    /// The layer has no pass of this index.
    NoPass(usize),
    /// The chosen implementation is not in the pass.
    NoImplementation(usize),
    NoVertexShader,
    /// Something in the pass is animated.
    Animated,
    /// It reads engine inputs nobody supplies here (beyond the shader
    /// instance's own).
    Reads(InputReads),
    UnsetSlot(u8),
    DefaultTexture,
    Lightmap,
    /// Its render states do not fit the native pass.
    State(StateRefusal),
    /// It reads engine-driven render or stage states.
    EngineStates,
    /// Its pixel constants come from the engine (the effect's values,
    /// which nothing supplies here yet).
    EnginePixelConstants,
    /// It tests or writes depth (the native pass is colour-only).
    Depth,
    /// A stage binds an engine texture the draw cannot supply
    /// ([`engine_texture`]: the light's, one chosen at run time, or an
    /// invalid id).
    EngineTexture(u8),
}

/// Implementation `implementation` of the layer-15 pass `layer_pass` of
/// `material` (first level of detail), `material` the egor's shader
/// resolved without engine inputs.
pub fn screen_look(material: &Material, layer_pass: usize, implementation: usize) -> Result<ScreenLook, ScreenRefusal> {
    let l = material
        .layers
        .iter()
        .find(|l| l.lod == 0 && l.layer == TEMPLATE_LAYER)
        .filter(|l| !l.passes.is_empty())
        .ok_or(ScreenRefusal::NoLayer)?;
    let pass = l.passes.iter().find(|p| p.index == layer_pass).ok_or(ScreenRefusal::NoPass(layer_pass))?;
    let imp = pass
        .implementations
        .iter()
        .find(|i| i.index == implementation)
        .ok_or(ScreenRefusal::NoImplementation(implementation))?;
    let vertex_shader = imp.vertex_shader.ok_or(ScreenRefusal::NoVertexShader)?;
    if !imp.animated.is_empty() {
        return Err(ScreenRefusal::Animated);
    }
    let r = imp.input_reads;
    let unsupplied = InputReads {
        scalars: r.scalars & !INSTANCE_SCALARS,
        vectors: r.vectors,
        ints: r.ints & !INSTANCE_INTS,
    };
    if unsupplied != InputReads::default() {
        return Err(ScreenRefusal::Reads(unsupplied));
    }
    if !imp.engine_render_states.is_empty() || !imp.engine_stage_states.is_empty() {
        return Err(ScreenRefusal::EngineStates);
    }
    if imp.pixel_constants_from_engine {
        return Err(ScreenRefusal::EnginePixelConstants);
    }
    let mut textures = [ScreenStage::Empty; 4];
    for (stage, t) in textures.iter_mut().zip(&imp.textures) {
        *stage = match t.map(|t| t.source) {
            None | Some(TextureSource::None) => ScreenStage::Empty,
            Some(TextureSource::Bitmap { bitmap, image, .. }) => ScreenStage::Bitmap { bitmap, image },
            Some(TextureSource::Engine(id)) => match engine_texture(id) {
                EngineTexture::Light | EngineTexture::Dynamic | EngineTexture::Invalid => {
                    return Err(ScreenRefusal::EngineTexture(id));
                }
                _ => ScreenStage::Engine(id),
            },
            Some(TextureSource::UnsetSlot(s)) => return Err(ScreenRefusal::UnsetSlot(s)),
            Some(TextureSource::Default) => return Err(ScreenRefusal::DefaultTexture),
            Some(TextureSource::Lightmap) => return Err(ScreenRefusal::Lightmap),
        };
    }
    let state = pass_state(&imp.render_states).map_err(ScreenRefusal::State)?;
    if state.depth_write || state.depth_compare != wgpu::CompareFunction::Always {
        return Err(ScreenRefusal::Depth);
    }
    Ok(ScreenLook {
        pass: pass.pass,
        layer_pass,
        implementation,
        vertex_shader,
        textures,
        stage_states: imp.stage_states.clone(),
        pixel_shader: imp.pixel_shader,
        vertex_constants: imp.vertex_constants.clone(),
        vertex_defaults: imp.vertex_defaults.clone(),
        state,
    })
}

/// A template pass translated ([`compile_screen`]).
#[derive(Clone, Debug)]
pub struct ScreenProgram {
    /// The linked module (`vs_main`, `fs_main`).
    pub wgsl: String,
    pub stages: [ScreenStage; 4],
    pub samplers: [Option<StageStates>; 4],
    pub dims: [Option<TextureDim>; 4],
    pub signed: [[bool; 4]; 4],
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    /// `v<n>` the program reads (within [`STRIP_INPUTS`]).
    pub inputs: u16,
    pub state: PassState,
}

/// Every bound stage is 2D: the shader's bitmaps and the render targets.
pub fn stage_dim(stage: ScreenStage) -> Option<TextureDim> {
    match stage {
        ScreenStage::Empty => None,
        ScreenStage::Bitmap { .. } | ScreenStage::Engine(_) => Some(TextureDim::D2),
    }
}

/// Whether the pixel side reads the vertex colours or fog (a general or
/// final combiner input `v0`, `v1`, fog or the final `v1 + r0`), or
/// leaves its final combiner to D3D (built from the fog and specular
/// states the pass inherits).
pub fn reads_vertex_colours(pixel_shader: &[u8; PSDEF_SIZE]) -> Result<bool, String> {
    use shaders::psh::Reg;
    let def = shaders::PixelShaderDef::from_bytes(pixel_shader).map_err(|e| format!("pixel shader: {e:?}"))?;
    let c = def.decode();
    let Some(f) = c.final_combiner else { return Ok(true) };
    Ok(c.stages
        .iter()
        .flat_map(|s| s.rgb.inputs.iter().chain(&s.alpha.inputs))
        .chain(&[f.a, f.b, f.c, f.d, f.e, f.f, f.g])
        .any(|i| matches!(i.reg, Reg::V0 | Reg::V1 | Reg::Fog | Reg::V1R0Sum)))
}

/// Translates `look` with its vertex program `microcode` over the strips'
/// inputs, or says why the effect keeps its D3D draws: a program reading
/// a register past [`STRIP_INPUTS`] and [`LEFT_INPUTS`], or one of the
/// latter with a pixel side that shows it ([`reads_vertex_colours`]).
pub fn compile_screen(look: &ScreenLook, microcode: &[u8]) -> Result<ScreenProgram, String> {
    let mut defaults = look.vertex_defaults.clone();
    if !reads_vertex_colours(&look.pixel_shader)? {
        for index in 0..16u8 {
            if LEFT_INPUTS & 1 << index != 0 && !defaults.iter().any(|r| r.index == index) {
                defaults.push(Register {
                    index,
                    value: VertexUniforms::default().defaults[usize::from(index)],
                    written: 0xf,
                    origin: Origin::Instance,
                });
            }
        }
    }
    let t = translate_pass(&PassParts {
        microcode,
        pixel_shader: &look.pixel_shader,
        dims: look.textures.map(stage_dim),
        biased: look.textures.map(|s| matches!(s, ScreenStage::Bitmap { .. })),
        stage_states: &look.stage_states,
        vertex_constants: &look.vertex_constants,
        vertex_defaults: &defaults,
        alpha_test: look.state.alpha_test,
        provided_inputs: STRIP_INPUTS,
        w_buffer: false,
    })?;
    Ok(ScreenProgram {
        wgsl: t.wgsl,
        stages: look.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex: t.vertex,
        pixel: t.pixel,
        inputs: t.inputs,
        state: look.state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::state::RenderStateType;
    use gfx::xdk::{HALO2_BUILD, RenderStateTable};
    use tags::material::{EngineInputs, ResolvedImplementation, ResolvedLayer, ResolvedPass, StageTexture};
    use tags::shader::RenderStateValue;

    fn texture(source: TextureSource) -> Option<StageTexture> {
        Some(StageTexture { source, size: None, stage_size: None })
    }

    fn state(t: RenderStateType, value: u32) -> RenderStateValue {
        let table = RenderStateTable::for_build(HALO2_BUILD);
        let state = (0..=255u8)
            .find(|&s| RenderStateType::from_guest_index(&table, u32::from(s)) == Some(t))
            .expect("a 5849 slot");
        RenderStateValue { state, value }
    }

    /// A screen effect's states (`effects\screen_effects\desaturate`'s
    /// shape): no blending, the depth buffer off.
    fn screen_states() -> Vec<RenderStateValue> {
        vec![state(RenderStateType::AlphaBlendEnable, 0), state(RenderStateType::ZEnable, 0)]
    }

    /// A blur's shape: the view's colour (an engine texture) and a bitmap.
    fn implementation(index: usize) -> ResolvedImplementation {
        ResolvedImplementation {
            index,
            vertex_shader: Some(TagId(0xe000_0010 + index as u32)),
            textures: [
                texture(TextureSource::Engine(5)),
                texture(TextureSource::Bitmap { bitmap: TagId(0xe000_0001), image: 0, lod_offset: 0.0 }),
                None,
                None,
            ],
            stage_states: Vec::new(),
            render_states: screen_states(),
            restore_states: Vec::new(),
            pixel_shader: [7; PSDEF_SIZE],
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: false,
            per_draw_inputs: false,
            input_reads: InputReads::default(),
            vector_bindings: Vec::new(),
            scalar_bindings: Vec::new(),
            vertex_constants: vec![Register { index: 18, value: [0.5, 0.5, 0.0, 0.0], written: 0xf, origin: Origin::Instance }],
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
        }
    }

    /// `passes` passes in `layer`, pass `n`'s implementations `imps` with
    /// their vertex shaders moved by `0x100 n`.
    fn material(layer: usize, passes: usize, imps: Vec<ResolvedImplementation>) -> Material {
        Material {
            shader: TagId(0xe000_0100),
            template: TagId(0xe000_0101),
            inputs: EngineInputs::default(),
            layers: vec![ResolvedLayer {
                lod: 0,
                layer,
                passes: (0..passes)
                    .map(|i| ResolvedPass {
                        index: i,
                        pass: TagId(0xe000_0200 + i as u32),
                        implementations: imps
                            .iter()
                            .cloned()
                            .map(|mut m| {
                                m.vertex_shader = m.vertex_shader.map(|v| TagId(v.0 + 0x100 * i as u32));
                                m
                            })
                            .collect(),
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn the_template_pass_names_its_stages() {
        let m = material(TEMPLATE_LAYER, 1, vec![implementation(0), implementation(1)]);
        let look = screen_look(&m, 0, 1).expect("a look");
        assert_eq!((look.layer_pass, look.implementation), (0, 1));
        assert_eq!(look.vertex_shader, TagId(0xe000_0011));
        assert_eq!(look.textures[0], ScreenStage::Engine(5));
        assert_eq!(look.textures[1], ScreenStage::Bitmap { bitmap: TagId(0xe000_0001), image: 0 });
        assert_eq!(look.engine_textures(), vec![5]);
        assert!(!look.state.depth_write && look.state.blend.is_none());
    }

    /// `effects\screen_effects\blur`'s shape: two passes in layer 15, each
    /// egor pass naming one (`0x00016610`'s third argument).
    #[test]
    fn each_pass_of_the_layer_is_its_own_look() {
        let m = material(TEMPLATE_LAYER, 2, vec![implementation(0)]);
        let first = screen_look(&m, 0, 0).expect("pass 0");
        let second = screen_look(&m, 1, 0).expect("pass 1");
        assert_eq!((first.pass, second.pass), (TagId(0xe000_0200), TagId(0xe000_0201)));
        assert_eq!((first.vertex_shader, second.vertex_shader), (TagId(0xe000_0010), TagId(0xe000_0110)));
        assert_eq!(screen_look(&m, 2, 0).unwrap_err(), ScreenRefusal::NoPass(2));
    }

    #[test]
    fn engine_textures_are_the_resolvers() {
        assert_eq!(engine_texture(5), EngineTexture::View);
        assert_eq!(engine_texture(2), EngineTexture::Target(1));
        assert_eq!(engine_texture(13), EngineTexture::Target(9));
        assert_eq!(engine_texture(24), EngineTexture::Slot(0x0048_5a78));
        assert_eq!(engine_texture(25), EngineTexture::Slot(0x0048_5a7c));
        assert_eq!(engine_texture(1), EngineTexture::Rasterizer(0xc));
        for id in [0, 7, 11, 28, 255] {
            assert_eq!(engine_texture(id), EngineTexture::Invalid, "{id}");
        }
    }

    #[test]
    fn what_keeps_the_d3d_draws() {
        let refusal = |imp: ResolvedImplementation| screen_look(&material(TEMPLATE_LAYER, 1, vec![imp]), 0, 0).unwrap_err();
        assert_eq!(screen_look(&material(12, 1, vec![implementation(0)]), 0, 0).unwrap_err(), ScreenRefusal::NoLayer);
        let m = material(TEMPLATE_LAYER, 1, vec![implementation(0)]);
        assert_eq!(screen_look(&m, 0, 3).unwrap_err(), ScreenRefusal::NoImplementation(3));
        let mut imp = implementation(0);
        imp.input_reads = InputReads { scalars: 0, vectors: 1 << 5, ints: 0 };
        assert_eq!(refusal(imp), ScreenRefusal::Reads(InputReads { scalars: 0, vectors: 1 << 5, ints: 0 }));
        let mut imp = implementation(0);
        imp.textures[2] = texture(TextureSource::Engine(10));
        assert_eq!(refusal(imp), ScreenRefusal::EngineTexture(10), "the light's texture");
        let mut imp = implementation(0);
        imp.textures[2] = texture(TextureSource::Lightmap);
        assert_eq!(refusal(imp), ScreenRefusal::Lightmap);
        let mut imp = implementation(0);
        imp.pixel_constants_from_engine = true;
        assert_eq!(refusal(imp), ScreenRefusal::EnginePixelConstants, "copy_texaccum_to_fb's c0");
        let mut imp = implementation(0);
        imp.render_states = vec![
            state(RenderStateType::AlphaBlendEnable, 0),
            state(RenderStateType::ZEnable, 1),
            state(RenderStateType::ZWriteEnable, 0),
        ];
        assert_eq!(refusal(imp), ScreenRefusal::Depth, "a depth test");
    }

    /// `dp4 oPos, v0, c[0..3]; mov oT0, v1; mov oT1, v<texcoord>`.
    fn strip_program(texcoord: u8) -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let row = |c, mask| Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(false, OutReg::Pos, mask);
        program(&[
            row(0, 8),
            row(1, 4),
            row(2, 2),
            row(3, 1),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(texcoord)), None, None).to_out(false, OutReg::T(1), 0xf),
        ])
    }

    /// The look with two 2D stages and a pixel side that programs its
    /// final combiner, reading fog with `fog`.
    fn look(fog: bool) -> ScreenLook {
        use shaders::psh::bits::{TEX_NONE, TEX_PROJECT2D, texture_modes};
        let (final_inputs_abcd, final_inputs_efg) = shaders::psh::d3d_default_final_combiner(fog, false);
        let def = shaders::PixelShaderDef {
            texture_modes: texture_modes([TEX_PROJECT2D, TEX_PROJECT2D, TEX_NONE, TEX_NONE]),
            final_inputs_abcd,
            final_inputs_efg,
            ..Default::default()
        };
        let m = material(TEMPLATE_LAYER, 1, vec![implementation(0)]);
        ScreenLook { pixel_shader: def.to_bytes(), ..screen_look(&m, 0, 0).unwrap() }
    }

    #[test]
    fn the_pass_compiles_over_the_strips_inputs() {
        let p = compile_screen(&look(false), &strip_program(2)).expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert!(!p.wgsl.contains("frag_depth"), "colour-only: no W-buffered depth");
        assert_eq!(p.dims, [Some(TextureDim::D2), Some(TextureDim::D2), None, None]);
        assert_eq!(p.inputs & !STRIP_INPUTS, 0);
        assert_eq!(p.vertex.constants[18], [0.5, 0.5, 0.0, 0.0], "instance constants");
    }

    /// `screen_xform_4` moves `v5`..`v7` into the colours and fog, which
    /// the strips leave to earlier draws: fine while nothing reads them.
    #[test]
    fn the_registers_the_strips_leave_count_only_when_seen() {
        assert_eq!(reads_vertex_colours(&look(false).pixel_shader), Ok(false));
        assert_eq!(reads_vertex_colours(&look(true).pixel_shader), Ok(true));
        let p = compile_screen(&look(false), &strip_program(5)).expect("v5 left, not seen");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        let e = compile_screen(&look(true), &strip_program(5)).unwrap_err();
        assert!(e.contains("do not supply"), "{e}");
        let e = compile_screen(&look(false), &strip_program(8)).unwrap_err();
        assert!(e.contains("do not supply"), "v8 is past them: {e}");
        // No final combiner: D3D builds one from the inherited fog state.
        let bare = shaders::PixelShaderDef::default().to_bytes();
        assert_eq!(reads_vertex_colours(&bare), Ok(true));
    }
}
