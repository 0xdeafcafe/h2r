//! A world material's layer-3 pass as the game draws it: the pass
//! implementation, its vertex program and pixel shader definition, stage
//! textures, sampler states and instance constants, from `tags::material`
//! (docs/formats/shader-instances.md). The native pass runs these through
//! `shaders` instead of a program of ours (docs/graphics/native-renderer.md,
//! "What a world pass reads"). A material that does not fit the rules here
//! stays on the D3D path, with the reason.

use std::io::{Read, Seek};

use map::{Map, TagId};
use tags::material::{Material, Origin, Register, ResolvedImplementation, TextureSource};
use tags::shader::{Metadata, PSDEF_SIZE, StageStates};

use crate::pass_state::{PassState, StateRefusal, pass_state};

/// The layer the native world draws.
pub const LIGHTMAP_LAYER: usize = 3;
/// The `vrtx` geometry classification of world-space geometry (S: its
/// programs read `v0` as a world position with no model transform).
pub const WORLD_CLASSIFICATION: u32 = 0;
/// The `vrtx` classification of rigid geometry, which the D3D path draws
/// an instanced placement with (S: the only other classification the
/// world's lightmap programs have; its programs place `v0` with the model
/// rows `c[50..52]` the structure draw uploads for a placement,
/// `0x0004d830`, after the position decompression `c[170..171]`).
/// [`crate::program::compile_rigid`] checks that it reads the rows.
pub const RIGID_CLASSIFICATION: u32 = 1;
/// `vrtx` classification stride; `+0xc` the compiled shader's size, `+0x10`
/// its address (docs/graphics/shaders.md).
const CLASSIFICATION: u32 = 0x1c;

/// What a texture stage samples, for the native pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    Empty,
    /// An instance bitmap's image.
    Bitmap { bitmap: TagId, image: u32 },
    /// The section's lightmap page through its palette.
    Lightmap,
    /// An engine texture by id (`0x00019530`'s table): supplied per view,
    /// never decoded at load. The look refuses these
    /// ([`Refusal::EngineTexture`]); the stages that supply them (the
    /// water edge blend's scene copy and reflections,
    /// `crate::water_gpu`) build their own look with them.
    Engine(u8),
}

/// One material's layer-3 pass.
#[derive(Clone, Debug, PartialEq)]
pub struct PassLook {
    /// The `spas` tag.
    pub pass: TagId,
    /// Which implementation of the pass (the per-pixel lightmap one).
    pub implementation: usize,
    /// The `vrtx` tag.
    pub vertex_shader: TagId,
    pub textures: [Stage; 4],
    pub stage_states: Vec<StageStates>,
    /// The final `D3DPIXELSHADERDEF`.
    pub pixel_shader: [u8; PSDEF_SIZE],
    /// Instance vertex constants (`c[18 + i]`, hardware numbering).
    pub vertex_constants: Vec<Register>,
    /// Vertex attribute defaults.
    pub vertex_defaults: Vec<Register>,
    /// The implementation reads an engine input that can change per draw
    /// (`ResolvedImplementation::per_draw_inputs`). Engine-driven combiner
    /// constants that read only the instance's scalar 21 (the 03a
    /// `lightmap_tex_detail` pass's: `c0.a` of its specular stage, the
    /// `shad`'s `+0x44`) are already in `pixel_shader`.
    pub per_draw_inputs: bool,
}

/// Why a material's layer 3 stays on the D3D path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Its template draws nothing in layer 3 at the first level of detail.
    NoLayer3,
    /// More than one pass in layer 3 (blend templates accumulate first).
    Passes(usize),
    /// No implementation samples the lightmap.
    NoLightmapImplementation,
    /// The record selects an implementation absent from this pass.
    MissingImplementation(usize),
    /// The implementation has no vertex program.
    NoVertexShader,
    /// A stage samples an engine texture other than the lightmap.
    EngineTexture(u8),
    /// A stage samples what the previous draw left.
    UnsetSlot(u8),
    /// A stage samples the rasterizer's default texture.
    DefaultTexture,
    /// A vertex constant is computed from engine inputs per draw.
    EngineVertexConstant(u8),
    /// Something in the pass is animated.
    Animated,
    /// The pass's render states do not fit the native pass
    /// ([`crate::pass_state::pass_state`]).
    State(StateRefusal),
    /// The pass reads engine-driven render states.
    EngineRenderStates,
    /// The pass reads engine-driven texture stage states.
    EngineStageStates,
    /// The pass blends with what is under it (the native world draws
    /// opaque layer-3 passes only).
    Blended,
}

/// A world material's layer-3 pass with the pipeline state its render
/// states give.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldPass {
    pub look: PassLook,
    pub state: PassState,
    /// The implementation does not set `CULLMODE`: the pass culls as the
    /// device does when layer 3 is drawn (`crate::program::InheritedCull`),
    /// and nothing where that is not known (`state`'s cull is `None`).
    pub inherits_cull: bool,
}

/// [`layer3_look`] with the pass's own render states: depth test and
/// write, colour writes, alpha test and cull, as the D3D path applies them
/// on entry (`0x00018900`). A pass that reads engine-driven render or stage
/// states, or blends, stays on the D3D path. Where the implementation does
/// not set `CULLMODE` itself it inherits the previous draw's: the D3D
/// path's draws of placement 282 on 03a cull `Front` (`CULL_CW`, front face
/// `CW`) where the pass leaves it unset. Such a
/// pass is marked [`WorldPass::inherits_cull`] and takes the device's cull
/// at the pass; its own state culls nothing, for where that is not known
/// (an open, single-sided girder seen from behind then draws its back
/// faces: the 43 px on 03a).
pub fn layer3_pass(material: &Material) -> Result<WorldPass, Refusal> {
    let look = layer3_look(material)?;
    let imp = layer3_implementation(material).ok_or(Refusal::NoLayer3)?;
    world_pass(look, imp)
}

/// The lighting selector for a BSP without a matching first lightmap
/// group (record builder, `world_walk::Plan::lighting`). A layer mask
/// covers all passes, so this material path requires a single pass.
pub fn unlit_bsp_pass(pass: &tags::material::SelectedImplementation, normalization: Option<TagId>)
    -> Result<WorldPass, Refusal>
{
    if pass.layer_passes != 1 { return Err(Refusal::Passes(pass.layer_passes)); }
    let imp = &pass.implementation;
    if imp.index != UNLIT_BSP_IMPLEMENTATION {
        return Err(Refusal::MissingImplementation(UNLIT_BSP_IMPLEMENTATION));
    }
    let look = look_with_engine(pass.pass, imp, normalization)?;
    world_pass(look, imp)
}

/// Record lighting selector when the BSP has no matching lightmap.
pub const UNLIT_BSP_IMPLEMENTATION: usize = 3;

fn world_pass(look: PassLook, imp: &ResolvedImplementation) -> Result<WorldPass, Refusal> {
    if !imp.engine_render_states.is_empty() {
        return Err(Refusal::EngineRenderStates);
    }
    if !imp.engine_stage_states.is_empty() {
        return Err(Refusal::EngineStageStates);
    }
    let mut state = pass_state(&imp.render_states).map_err(Refusal::State)?;
    if state.blend.is_some() {
        return Err(Refusal::Blended);
    }
    let inherits_cull = !sets_state(&imp.render_states, gfx::state::RenderStateType::CullMode);
    if inherits_cull {
        state.primitive.cull_mode = None;
    }
    Ok(WorldPass { look, state, inherits_cull })
}

/// Whether `states` set `which` (build 5849's slots).
pub(crate) fn sets_state(states: &[tags::shader::RenderStateValue], which: gfx::state::RenderStateType) -> bool {
    let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
    states
        .iter()
        .any(|s| gfx::state::RenderStateType::from_guest_index(&table, u32::from(s.state)) == Some(which))
}

/// The implementation [`layer3_look`] takes.
fn layer3_implementation(material: &Material) -> Option<&ResolvedImplementation> {
    let layer = material.layers.iter().find(|l| l.lod == 0 && l.layer == LIGHTMAP_LAYER)?;
    let [pass] = layer.passes.as_slice() else { return None };
    pass.implementations.iter().find(|i| samples_lightmap(i))
}

/// The layer-3 pass of `material` (first level of detail), or why not.
/// The implementation is the first that samples the lightmap: the one the
/// engine binds for a lightmapped section (S, the lightmap bind at
/// `0x0004d40f`).
pub fn layer3_look(material: &Material) -> Result<PassLook, Refusal> {
    let layer = material
        .layers
        .iter()
        .find(|l| l.lod == 0 && l.layer == LIGHTMAP_LAYER)
        .filter(|l| !l.passes.is_empty())
        .ok_or(Refusal::NoLayer3)?;
    let [pass] = layer.passes.as_slice() else {
        return Err(Refusal::Passes(layer.passes.len()));
    };
    let imp = pass
        .implementations
        .iter()
        .find(|i| samples_lightmap(i))
        .ok_or(Refusal::NoLightmapImplementation)?;
    implementation_look(pass.pass, imp)
}

/// One implementation of the pass `pass` as the native pass draws it, or
/// why it stays on the D3D path: its vertex program, stage textures and
/// instance constants, nothing animated or computed per draw.
pub fn implementation_look(pass: TagId, imp: &ResolvedImplementation) -> Result<PassLook, Refusal> {
    look_with_engine(pass, imp, None)
}

fn look_with_engine(pass: TagId, imp: &ResolvedImplementation, normalization: Option<TagId>) -> Result<PassLook, Refusal> {
    let vertex_shader = imp.vertex_shader.ok_or(Refusal::NoVertexShader)?;
    if !imp.animated.is_empty() {
        return Err(Refusal::Animated);
    }
    if let Some(r) = imp.vertex_constants.iter().find(|r| r.origin == Origin::Engine) {
        return Err(Refusal::EngineVertexConstant(r.index));
    }
    let mut textures = [Stage::Empty; 4];
    for (stage, t) in textures.iter_mut().zip(&imp.textures) {
        *stage = match t.map(|t| t.source) {
            None | Some(TextureSource::None) => Stage::Empty,
            Some(TextureSource::Bitmap { bitmap, image, .. }) => Stage::Bitmap { bitmap, image },
            Some(TextureSource::Lightmap) => Stage::Lightmap,
            Some(TextureSource::Engine(1)) => Stage::Bitmap {
                bitmap: normalization.ok_or(Refusal::EngineTexture(1))?, image: 0,
            },
            Some(TextureSource::Engine(id)) => return Err(Refusal::EngineTexture(id)),
            Some(TextureSource::UnsetSlot(s)) => return Err(Refusal::UnsetSlot(s)),
            Some(TextureSource::Default) => return Err(Refusal::DefaultTexture),
        };
    }
    Ok(PassLook {
        pass,
        implementation: imp.index,
        vertex_shader,
        textures,
        stage_states: imp.stage_states.clone(),
        pixel_shader: imp.pixel_shader,
        vertex_constants: imp.vertex_constants.clone(),
        vertex_defaults: imp.vertex_defaults.clone(),
        per_draw_inputs: imp.per_draw_inputs,
    })
}

/// A lighting input an object's layer-3 pass reads
/// ([`crate::object_lighting`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightingInput {
    Vector(u8),
    Scalar(u8),
}

/// The first lighting input any implementation of `material`'s layer-3
/// passes (first level of detail) reads, or `None`. Every implementation
/// counts: the build picks the one an object draws from its lighting
/// (`0x0004cd35`). The object program path puts each in per object
/// (`object_program`); this names the first, for diagnostics and tests.
pub fn object_lighting_input(material: &Material) -> Option<LightingInput> {
    use crate::object_lighting::{LIGHTING_SCALARS, LIGHTING_VECTORS};
    let layers = material.layers.iter().filter(|l| l.lod == 0 && l.layer == LIGHTMAP_LAYER);
    for imp in layers.flat_map(|l| &l.passes).flat_map(|p| &p.implementations) {
        let r = imp.input_reads;
        let v = r.vectors & LIGHTING_VECTORS;
        if v != 0 {
            return Some(LightingInput::Vector(v.trailing_zeros() as u8));
        }
        let s = r.scalars & LIGHTING_SCALARS;
        if s != 0 {
            return Some(LightingInput::Scalar(s.trailing_zeros() as u8));
        }
    }
    None
}

fn samples_lightmap(i: &ResolvedImplementation) -> bool {
    i.textures
        .iter()
        .any(|t| t.is_some_and(|t| t.source == TextureSource::Lightmap))
}

/// The compiled vertex program (`'x' 0x20` microcode, what
/// `shaders::vsh::Program::parse` takes) of classification `index` of the
/// `vrtx` tag at `address`.
pub fn vertex_program(meta: &Metadata, address: u32, index: u32) -> Option<Vec<u8>> {
    let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(meta.bytes(a, 4).ok()?.try_into().ok()?)) };
    let count = word(address + 4)?;
    if index >= count {
        return None;
    }
    let entry = word(address + 8)? + index * CLASSIFICATION;
    let (size, at) = (word(entry + 0xc)?, word(entry + 0x10)?);
    if size == 0 {
        return None;
    }
    Some(meta.bytes(at, size as usize).ok()?.to_vec())
}

/// The classification count of the `vrtx` tag at `address`.
pub fn classification_count(meta: &Metadata, address: u32) -> Option<u32> {
    Some(u32::from_le_bytes(meta.bytes(address + 4, 4).ok()?.try_into().ok()?))
}

/// The input semantics of classification `index` of the `vrtx` tag at
/// `address`: register `v<i>` reads semantic `[i]` (classification `+4`
/// the count, `+8` the u16 list; `0x0001c3b0`).
pub fn vertex_semantics(meta: &Metadata, address: u32, index: u32) -> Option<Vec<u16>> {
    let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(meta.bytes(a, 4).ok()?.try_into().ok()?)) };
    if index >= word(address + 4)? {
        return None;
    }
    let entry = word(address + 8)? + index * CLASSIFICATION;
    let (count, at) = (word(entry + 4)?, word(entry + 8)?);
    if count > 16 {
        return None;
    }
    let bytes = meta.bytes(at, 2 * count as usize).ok()?;
    Some(bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect())
}

/// [`vertex_program`] of a `vrtx` tag in `map`, world-space classification.
pub fn world_program<R: Read + Seek>(map: &mut Map<R>, meta: &Metadata, vrtx: TagId) -> Option<Vec<u8>> {
    let address = map.tag(vrtx).ok()?.location?.address;
    vertex_program(meta, address, WORLD_CLASSIFICATION)
}

/// [`vertex_program`] of a `vrtx` tag in `map`, rigid classification
/// ([`RIGID_CLASSIFICATION`]).
pub fn rigid_program<R: Read + Seek>(map: &mut Map<R>, meta: &Metadata, vrtx: TagId) -> Option<Vec<u8>> {
    let address = map.tag(vrtx).ok()?.location?.address;
    vertex_program(meta, address, RIGID_CLASSIFICATION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::material::{EngineInputs, ResolvedLayer, ResolvedPass, StageTexture};
    use tags::shader::{ExternalBinding, RenderStateValue};

    fn texture(source: TextureSource) -> Option<StageTexture> {
        Some(StageTexture { source, size: None, stage_size: None })
    }

    fn implementation(index: usize, lightmap: bool) -> ResolvedImplementation {
        ResolvedImplementation {
            index,
            vertex_shader: Some(TagId(0xe000_0010 + index as u32)),
            textures: [
                texture(TextureSource::Bitmap { bitmap: TagId(0xe000_0001), image: 0, lod_offset: 0.0 }),
                texture(TextureSource::Bitmap { bitmap: TagId(0xe000_0002), image: 1, lod_offset: 0.0 }),
                None,
                texture(if lightmap { TextureSource::Lightmap } else { TextureSource::Engine(1) }),
            ],
            stage_states: Vec::new(),
            render_states: Vec::new(),
            restore_states: Vec::new(),
            pixel_shader: [7; PSDEF_SIZE],
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: false,
            per_draw_inputs: false,
            input_reads: Default::default(),
            vector_bindings: Vec::new(),
            scalar_bindings: Vec::new(),
            vertex_constants: vec![Register { index: 19, value: [10.0, 10.0, 0.0, 0.0], written: 0xf, origin: Origin::Instance }],
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
            dimension_bindings: Vec::new(),
        }
    }

    fn material(layer: usize, passes: Vec<Vec<ResolvedImplementation>>) -> Material {
        Material {
            shader: TagId(0xe000_0100),
            template: TagId(0xe000_0101),
            inputs: EngineInputs::default(),
            layers: vec![ResolvedLayer {
                lod: 0,
                layer,
                passes: passes
                    .into_iter()
                    .enumerate()
                    .map(|(index, implementations)| ResolvedPass { index, pass: TagId(0xe000_0200), implementations })
                    .collect(),
            }],
        }
    }

    #[test]
    fn an_object_material_reading_its_lighting_in_any_implementation_is_named() {
        let mut lit = implementation(1, false);
        lit.input_reads.vectors = 1 << 5 | 1 << 15;
        let m = material(3, vec![vec![implementation(0, false), lit.clone()]]);
        assert_eq!(object_lighting_input(&m), Some(LightingInput::Vector(15)));
        let mut luma = implementation(0, false);
        luma.input_reads.scalars = 1 << 21 | 1 << 23;
        let m = material(3, vec![vec![luma]]);
        assert_eq!(object_lighting_input(&m), Some(LightingInput::Scalar(23)));
        // Inputs that are not the lighting's, or another layer: none.
        let mut other = implementation(0, false);
        other.input_reads.vectors = 1 << 5;
        other.input_reads.scalars = 1 << 21;
        assert_eq!(object_lighting_input(&material(3, vec![vec![other]])), None);
        assert_eq!(object_lighting_input(&material(4, vec![vec![lit]])), None);
    }

    #[test]
    fn the_lightmap_implementation_of_the_one_pass_is_taken() {
        let m = material(3, vec![vec![implementation(0, false), implementation(1, true)]]);
        let look = layer3_look(&m).unwrap();
        assert_eq!(look.implementation, 1);
        assert_eq!(look.vertex_shader, TagId(0xe000_0011));
        assert_eq!(
            look.textures,
            [
                Stage::Bitmap { bitmap: TagId(0xe000_0001), image: 0 },
                Stage::Bitmap { bitmap: TagId(0xe000_0002), image: 1 },
                Stage::Empty,
                Stage::Lightmap,
            ]
        );
        assert_eq!(look.vertex_constants[0].index, 19);
        assert_eq!(look.pixel_shader, [7; PSDEF_SIZE]);
    }

    #[test]
    fn a_bsp_without_a_lightmap_selects_the_dynamic_program_and_cube() {
        let mut selected = tags::material::SelectedImplementation {
            pass: TagId(13), layer_passes: 1, render_writes: Vec::new(), implementation: opaque(implementation(3, false), &[]),
        };
        let cube = TagId(17);
        let pass = unlit_bsp_pass(&selected, Some(cube)).unwrap();
        assert_eq!(pass.look.implementation, 3);
        assert_eq!(pass.look.vertex_shader, selected.implementation.vertex_shader.unwrap());
        assert_eq!(pass.look.textures[3], Stage::Bitmap { bitmap: cube, image: 0 });
        assert_eq!(unlit_bsp_pass(&selected, None), Err(Refusal::EngineTexture(1)));
        selected.implementation = opaque(implementation(0, true), &[]);
        assert_eq!(unlit_bsp_pass(&selected, Some(cube)), Err(Refusal::MissingImplementation(3)));
        selected.layer_passes = 2;
        assert_eq!(unlit_bsp_pass(&selected, Some(cube)), Err(Refusal::Passes(2)));
    }

    #[test]
    fn materials_outside_the_rules_say_why() {
        assert_eq!(layer3_look(&material(1, vec![vec![implementation(0, true)]])), Err(Refusal::NoLayer3));
        let two = material(3, vec![vec![implementation(0, true)], vec![implementation(0, true)]]);
        assert_eq!(layer3_look(&two), Err(Refusal::Passes(2)));
        let unlit = material(3, vec![vec![implementation(0, false)]]);
        assert_eq!(layer3_look(&unlit), Err(Refusal::NoLightmapImplementation));
        let mut engine = implementation(0, true);
        engine.textures[2] = texture(TextureSource::Engine(5));
        assert_eq!(layer3_look(&material(3, vec![vec![engine]])), Err(Refusal::EngineTexture(5)));
        let mut constant = implementation(0, true);
        constant.vertex_constants[0].origin = Origin::Engine;
        assert_eq!(layer3_look(&material(3, vec![vec![constant]])), Err(Refusal::EngineVertexConstant(19)));
        let mut bare = implementation(0, true);
        bare.vertex_shader = None;
        assert_eq!(layer3_look(&material(3, vec![vec![bare]])), Err(Refusal::NoVertexShader));
    }

    fn slot(t: gfx::state::RenderStateType) -> u8 {
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        (0..=255u8)
            .find(|&s| gfx::state::RenderStateType::from_guest_index(&table, u32::from(s)) == Some(t))
            .unwrap()
    }

    /// The states the 03a world passes set: no blend, depth writes, all
    /// colour channels; then `extra`.
    fn opaque(mut imp: ResolvedImplementation, extra: &[(gfx::state::RenderStateType, u32)]) -> ResolvedImplementation {
        use gfx::state::RenderStateType as R;
        imp.render_states = [(R::ColorWriteEnable, 0x0101_0101), (R::AlphaBlendEnable, 0), (R::ZWriteEnable, 1)]
            .iter()
            .chain(extra)
            .map(|&(t, value)| RenderStateValue { state: slot(t), value })
            .collect();
        imp
    }

    #[test]
    fn a_world_pass_takes_its_own_states() {
        use gfx::state::{CompareFunc, CullMode, RenderStateType as R};
        let m = material(3, vec![vec![opaque(implementation(0, true), &[])]]);
        let p = layer3_pass(&m).unwrap();
        assert_eq!(p.look, layer3_look(&m).unwrap());
        assert_eq!(p.state.blend, None);
        assert!(p.state.depth_write);
        assert_eq!(p.state.depth_compare, wgpu::CompareFunction::LessEqual, "ZFUNC unset: the world's");
        assert_eq!(p.state.primitive.cull_mode, None, "CULLMODE unset: nothing culled");
        assert!(p.inherits_cull, "CULLMODE unset: the device's at the pass");
        assert_eq!(p.state.color_writes, wgpu::ColorWrites::ALL);
        assert_eq!(p.state.alpha_test, None);
        // 03a's alpha-tested world passes.
        let tested = opaque(
            implementation(0, true),
            &[(R::AlphaTestEnable, 1), (R::AlphaRef, 0xff), (R::AlphaFunc, CompareFunc::Less.xbox_value())],
        );
        let p = layer3_pass(&material(3, vec![vec![tested]])).unwrap();
        assert_eq!(p.state.alpha_test, Some((wgpu::CompareFunction::Less, 0xff)));
        // A pass that sets its cull mode keeps it.
        let culled = opaque(implementation(0, true), &[(R::CullMode, CullMode::Cw.xbox_value())]);
        let own = layer3_pass(&material(3, vec![vec![culled]])).unwrap();
        assert!(own.state.primitive.cull_mode.is_some());
        assert!(!own.inherits_cull);
        // Depth equal, no writes: as set.
        let equal = opaque(implementation(0, true), &[(R::ZFunc, CompareFunc::Equal.xbox_value()), (R::ZWriteEnable, 0)]);
        let p = layer3_pass(&material(3, vec![vec![equal]])).unwrap();
        assert_eq!((p.state.depth_compare, p.state.depth_write), (wgpu::CompareFunction::Equal, false));
    }

    #[test]
    fn world_passes_outside_the_native_state_stay_on_the_d3d_path() {
        use gfx::state::{BlendFactor, RenderStateType as R};
        let refusal = |imp| layer3_pass(&material(3, vec![vec![imp]])).unwrap_err();
        assert_eq!(refusal(implementation(0, true)), Refusal::State(StateRefusal::Unset("ALPHABLENDENABLE")));
        let blended = opaque(
            implementation(0, true),
            &[(R::AlphaBlendEnable, 1), (R::SrcBlend, BlendFactor::One.xbox_value()), (R::DestBlend, BlendFactor::One.xbox_value())],
        );
        assert_eq!(refusal(blended), Refusal::Blended);
        assert_eq!(refusal(opaque(implementation(0, true), &[(R::FogEnable, 1)])), Refusal::State(StateRefusal::Fog));
        let mut engine = opaque(implementation(0, true), &[]);
        engine.engine_render_states = vec![ExternalBinding { bytes: [0; 4] }];
        assert_eq!(refusal(engine), Refusal::EngineRenderStates);
        let mut engine = opaque(implementation(0, true), &[]);
        engine.engine_stage_states = vec![ExternalBinding { bytes: [0; 4] }];
        assert_eq!(refusal(engine), Refusal::EngineStageStates);
        // Every layer-3 refusal still applies.
        let mut texaccum = opaque(implementation(0, true), &[]);
        texaccum.textures[0] = texture(TextureSource::Engine(3));
        assert_eq!(refusal(texaccum), Refusal::EngineTexture(3));
    }

    #[test]
    fn the_vertex_program_is_read_from_its_classification() {
        // vrtx at 0x100: count 2, classifications at 0x200; the second
        // holds 6 bytes at 0x300.
        let mut bytes = vec![0u8; 0x400];
        let mut put = |a: usize, v: u32| bytes[a..a + 4].copy_from_slice(&v.to_le_bytes());
        put(0x104, 2);
        put(0x108, 0x1000 + 0x200);
        put(0x200 + CLASSIFICATION as usize + 0xc, 6);
        put(0x200 + CLASSIFICATION as usize + 0x10, 0x1000 + 0x300);
        bytes[0x300..0x306].copy_from_slice(b"x \x01\x00ab");
        let meta = Metadata::new(0x1000, bytes);
        assert_eq!(vertex_program(&meta, 0x1100, 1).as_deref(), Some(&b"x \x01\x00ab"[..]));
        assert_eq!(vertex_program(&meta, 0x1100, 0), None, "empty classification");
        assert_eq!(vertex_program(&meta, 0x1100, 2), None, "past the count");
    }
}
