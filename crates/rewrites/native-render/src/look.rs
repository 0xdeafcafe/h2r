//! A world material's layer-3 pass as the game draws it: the pass
//! implementation, its vertex program and pixel shader definition, stage
//! textures, sampler states and instance constants, from `tags::material`
//! (docs/formats/shader-instances.md). The native pass runs these through
//! `shaders` instead of a program of ours (docs/graphics/native-world.md,
//! "What a world pass reads"). A material that does not fit the rules here
//! stays on the D3D path, with the reason.

use std::io::{Read, Seek};

use map::{Map, TagId};
use tags::material::{EngineInputs, Material, Origin, Register, ResolvedImplementation, TextureSource};
use tags::shader::{Metadata, PSDEF_SIZE, StageStates};

use crate::engine_states::{EngineStateRefusal, apply_static};
use crate::pass_state::{PassState, StateRefusal, geometry_pass_state};

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
    /// never decoded at load. Layer 3 can bind target 1. The water and
    /// fog passes supply their own per-view resources.
    Engine(u8),
    /// An image a host with no game keys itself ([`HostStage`]): never a
    /// tag, so it cannot stand for one. Bound as a bitmap stage is, from
    /// the host's stage-image lookup.
    Host(HostStage),
}

impl Stage {
    /// Whether the stage binds an image from the host's stage-image
    /// lookup: an instance bitmap's, or a [`Stage::Host`] one.
    pub fn takes_image(self) -> bool {
        matches!(self, Stage::Bitmap { .. } | Stage::Host(_))
    }
}

/// The images the editor's base look samples (`crate::base_look`), which
/// no tag names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostStage {
    /// A material's base map, as the host's lighting mode draws it.
    BaseMap(u16),
    /// White: an unlit part's lightmap stage.
    White,
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
    /// The pass has no binding for this engine texture.
    EngineTexture(u8),
    /// A stage samples what the previous draw left.
    UnsetSlot(u8),
    /// A stage samples the rasterizer's default texture.
    DefaultTexture,
    /// A vertex constant is computed from engine inputs per draw.
    EngineVertexConstant(u8),
    /// Something in the pass is animated.
    Animated,
    /// An animated constant the sampler cannot apply
    /// ([`crate::layer_pass::PassAnimation::baseline`]), by the
    /// [`crate::layer_pass::LayerRefusal::name`] of its cause.
    AnimationSample(&'static str),
    /// The pass's render states do not fit the native pass
    /// ([`crate::pass_state::pass_state`]).
    State(StateRefusal),
    /// The pass's engine-driven render or stage states do not fit
    /// ([`crate::engine_states::apply_static`]).
    EngineStates(EngineStateRefusal),
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
    /// Effective writes that take their dimensions from target 1.
    pub texaccum_dimensions: Vec<(tags::material::EngineVertexConstant, u8)>,
    /// The implementation does not set `CULLMODE`: the pass culls as the
    /// device does when layer 3 is drawn (`crate::program::InheritedCull`),
    /// and nothing where that is not known (`state`'s cull is `None`).
    pub inherits_cull: bool,
}

/// [`layer3_look`] with the pass's own render states: depth test and
/// write, colour writes, alpha test and cull, as the D3D path applies them
/// on entry (`0x00018900`), with its engine-driven render and stage states
/// over them from the material's inputs
/// ([`crate::engine_states::apply_static`]). A pass whose engine-driven
/// states do not fit, or that blends, stays on the D3D path. Where the implementation does
/// not set `CULLMODE` itself it inherits the previous draw's: the D3D
/// path's draws of placement 282 on 03a cull `Front` (`CULL_CW`, front face
/// `CW`) where the pass leaves it unset. Such a
/// pass is marked [`WorldPass::inherits_cull`] and takes the device's cull
/// at the pass; its own state culls nothing, for where that is not known
/// (an open, single-sided girder seen from behind then draws its back
/// faces: the 43 px on 03a).
pub fn layer3_pass(material: &Material) -> Result<WorldPass, Refusal> {
    let (pass, imp) = layer3_selected(material)?;
    world_pass(world_implementation_look(pass, imp, None)?, imp, &material.inputs)
}

/// [`layer3_pass`] for a pass whose constants animate: compiled from its
/// registers at clock 0, with the animation each frame samples at the
/// render clock (`properties` the instance's, `shad +0x3c`; the
/// structure planner's sampler, [`crate::layer_pass::PassAnimation`]).
/// A target the sampler cannot apply stays on the D3D path
/// ([`Refusal::AnimationSample`]).
pub fn layer3_pass_sampled(
    material: &Material, properties: &[tags::shader::AnimatedProperty],
) -> Result<(WorldPass, Option<crate::layer_pass::PassAnimation>), Refusal> {
    let (pass, imp) = layer3_selected(material)?;
    let (imp, animation) = sampled(imp, properties)?;
    Ok((world_pass(world_implementation_look(pass, &imp, None)?, &imp, &material.inputs)?, animation))
}

fn sampled(
    imp: &ResolvedImplementation, properties: &[tags::shader::AnimatedProperty],
) -> Result<(ResolvedImplementation, Option<crate::layer_pass::PassAnimation>), Refusal> {
    crate::layer_pass::PassAnimation::baseline(imp, properties, false, 0).map_err(|r| Refusal::AnimationSample(r.name()))
}

/// The lighting selector for a BSP without a matching first lightmap
/// group (record builder, `world_walk::Plan::lighting`). A layer mask
/// covers all passes, so this material path requires a single pass.
/// `inputs` are those `pass` was resolved with (the instance's,
/// `EngineInputs::for_shader`, when the resolve was given none).
pub fn unlit_bsp_pass(
    pass: &tags::material::SelectedImplementation, inputs: &EngineInputs, normalization: Option<TagId>,
) -> Result<WorldPass, Refusal> {
    let imp = unlit_bsp_implementation(pass)?;
    let look = world_implementation_look(pass.pass, imp, normalization)?;
    world_pass(look, imp, inputs)
}

/// [`unlit_bsp_pass`] for a pass whose constants animate, as
/// [`layer3_pass_sampled`].
pub fn unlit_bsp_pass_sampled(
    pass: &tags::material::SelectedImplementation, inputs: &EngineInputs, normalization: Option<TagId>,
    properties: &[tags::shader::AnimatedProperty],
) -> Result<(WorldPass, Option<crate::layer_pass::PassAnimation>), Refusal> {
    unlit_bsp_implementation(pass)?;
    selected_bsp_pass_sampled(pass, inputs, normalization, properties)
}

/// Compile the implementation selected by the structure render record.
/// The part mask covers every pass in the layer, so a batched material
/// must still have exactly one pass.
pub fn selected_bsp_pass_sampled(
    pass: &tags::material::SelectedImplementation, inputs: &EngineInputs, normalization: Option<TagId>,
    properties: &[tags::shader::AnimatedProperty],
) -> Result<(WorldPass, Option<crate::layer_pass::PassAnimation>), Refusal> {
    if pass.layer_passes != 1 { return Err(Refusal::Passes(pass.layer_passes)); }
    let (imp, animation) = sampled(&pass.implementation, properties)?;
    let look = world_implementation_look(pass.pass, &imp, normalization)?;
    Ok((world_pass(look, &imp, inputs)?, animation))
}

fn unlit_bsp_implementation(pass: &tags::material::SelectedImplementation) -> Result<&ResolvedImplementation, Refusal> {
    if pass.layer_passes != 1 { return Err(Refusal::Passes(pass.layer_passes)); }
    let imp = &pass.implementation;
    if imp.index != UNLIT_BSP_IMPLEMENTATION {
        return Err(Refusal::MissingImplementation(UNLIT_BSP_IMPLEMENTATION));
    }
    Ok(imp)
}

/// Record lighting selector when the BSP has no matching lightmap.
pub const UNLIT_BSP_IMPLEMENTATION: usize = 3;

fn world_pass(mut look: PassLook, imp: &ResolvedImplementation, inputs: &EngineInputs) -> Result<WorldPass, Refusal> {
    let applied = apply_static(imp, inputs).map_err(Refusal::EngineStates)?;
    look.stage_states = applied.stage_states;
    // The structure draw `0x0004d0b0` applies an implementation's states
    // only when the implementation changes (`opaque_material::handoff`,
    // after the old one's restore range), then an item's stencil switch
    // (mode 0 or 2) and, after it, mode 1. So a pass's own stencil holds
    // only up to the first switching item of its run.
    let mut state = geometry_pass_state(&applied.render_states)
        .map_err(Refusal::State)?
        .device_stencil_only("layers 1 and 3");
    if state.blend.is_some() {
        // The world issues opaque parts together. A replacement blend is
        // independent of the destination only when the operation is known.
        use gfx::state::{BlendOp, RenderStateType as R};
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let operation = applied.render_states.iter().rev()
            .find(|s| R::from_guest_index(&table, u32::from(s.state)) == Some(R::BlendOp));
        if state.blend != Some(wgpu::BlendState::REPLACE)
            || operation.map(|s| s.value) != Some(BlendOp::Add.xbox_value())
        {
            return Err(Refusal::Blended);
        }
        state.blend = None;
    }
    let inherits_cull = !sets_state(&imp.render_states, gfx::state::RenderStateType::CullMode);
    if inherits_cull {
        state.primitive.cull_mode = None;
    }
    let texaccum_dimensions = texaccum_dimensions(imp);
    Ok(WorldPass { look, state, inherits_cull, texaccum_dimensions })
}

/// Whether `states` set `which` (build 5849's slots).
pub(crate) fn sets_state(states: &[tags::shader::RenderStateValue], which: gfx::state::RenderStateType) -> bool {
    let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
    states
        .iter()
        .any(|s| gfx::state::RenderStateType::from_guest_index(&table, u32::from(s.state)) == Some(which))
}

/// The layer-3 pass of `material` (first level of detail), or why not.
/// The implementation is the first that samples the lightmap: the one the
/// engine binds for a lightmapped section (S, the lightmap bind at
/// `0x0004d40f`).
pub fn layer3_look(material: &Material) -> Result<PassLook, Refusal> {
    let (pass, imp) = layer3_selected(material)?;
    implementation_look(pass, imp)
}

/// The pass and implementation [`layer3_look`] draws, or why none.
fn layer3_selected(material: &Material) -> Result<(TagId, &ResolvedImplementation), Refusal> {
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
    Ok((pass.pass, imp))
}

/// One implementation of the pass `pass` as the native pass draws it, or
/// why it stays on the D3D path: its vertex program, stage textures and
/// instance constants, nothing animated or computed per draw.
pub fn implementation_look(pass: TagId, imp: &ResolvedImplementation) -> Result<PassLook, Refusal> {
    look_with_engine(pass, imp, None, None, false)
}

/// The image of the rasterizer data's `+0x14` bitmap that engine texture
/// `id` binds. Both jump-table entries (`0x000197dd` for 14, `0x000197fb`
/// for 15) call `0x000143c0` with that bitmap; 14 clears `ecx` first (image
/// 0), 15 leaves the 1 `0x00019538` loaded (image 1).
pub fn rasterizer_image(id: u8) -> u32 {
    u32::from(id == 15)
}

/// [`implementation_look`] with the rasterizer data's bitmap at `+0x14`
/// (engine textures 14 and 15, `screen_look::engine_texture`) resolved to
/// `rasterizer`: a stage naming either samples it ([`rasterizer_image`]).
/// `None` refuses them as [`implementation_look`] does.
pub fn implementation_look_with_rasterizer(
    pass: TagId, imp: &ResolvedImplementation, rasterizer: Option<TagId>,
) -> Result<PassLook, Refusal> {
    look_with_engine(pass, imp, None, rasterizer, false)
}

/// Target 1 is a live render texture. Its size bindings can share a row
/// with instance data, another dimension binding or a scalar input.
/// Keep only lanes for which target 1 is the final engine write.
fn texaccum_dimensions(imp: &ResolvedImplementation) -> Vec<(tags::material::EngineVertexConstant, u8)> {
    use tags::material::VectorTarget;
    let mut covered = [[0u8; 16]; 2];
    for b in imp.scalar_bindings.iter().chain(&imp.vector_bindings) {
        if let VectorTarget::Vertex { bank, reg, components } = b.target {
            covered[usize::from(bank)][usize::from(reg)] |= components;
        }
    }
    let mut dimensions = Vec::new();
    for &d in imp.dimension_bindings.iter().rev() {
        let written = &mut covered[usize::from(d.bank)][usize::from(d.reg)];
        let lanes = d.components() & !*written;
        *written |= d.components();
        let stage = usize::from(d.stage).checked_sub(1).and_then(|n| imp.textures.get(n)).copied().flatten();
        if lanes != 0 && stage.is_some_and(|t| t.source == TextureSource::Engine(crate::object_program::TEXACCUM_TEXTURE)) {
            dimensions.push((d, lanes));
        }
    }
    dimensions
}

fn world_implementation_look(
    pass: TagId, imp: &ResolvedImplementation, normalization: Option<TagId>,
) -> Result<PassLook, Refusal> {
    look_with_engine(pass, imp, normalization, None, true)
}

fn look_with_engine(
    pass: TagId, imp: &ResolvedImplementation, normalization: Option<TagId>, rasterizer: Option<TagId>,
    world_accumulation: bool,
) -> Result<PassLook, Refusal> {
    use crate::object_program::TEXACCUM_TEXTURE;
    let vertex_shader = imp.vertex_shader.ok_or(Refusal::NoVertexShader)?;
    if !imp.animated.is_empty() {
        return Err(Refusal::Animated);
    }
    let accumulation_size = |r: &Register| world_accumulation && imp.dimension_bindings.iter().any(|d| {
        d.bank == 1 && 18 + d.reg == r.index && usize::from(d.stage).checked_sub(1)
            .and_then(|stage| imp.textures.get(stage)).copied().flatten()
            .is_some_and(|t| t.source == TextureSource::Engine(TEXACCUM_TEXTURE))
    });
    if let Some(r) = imp.vertex_constants.iter().find(|r| r.origin == Origin::Engine && !accumulation_size(r)) {
        return Err(Refusal::EngineVertexConstant(r.index));
    }
    let mut textures = [Stage::Empty; 4];
    for (stage, t) in textures.iter_mut().zip(&imp.textures) {
        *stage = match t.map(|t| t.source) {
            None | Some(TextureSource::None) => Stage::Empty,
            Some(TextureSource::Bitmap { bitmap, image, .. }) => Stage::Bitmap { bitmap, image },
            Some(TextureSource::Lightmap) => Stage::Lightmap,
            Some(TextureSource::Engine(TEXACCUM_TEXTURE)) if world_accumulation => Stage::Engine(TEXACCUM_TEXTURE),
            Some(TextureSource::Engine(1)) => Stage::Bitmap {
                bitmap: normalization.ok_or(Refusal::EngineTexture(1))?, image: 0,
            },
            Some(TextureSource::Engine(id @ (14 | 15))) => Stage::Bitmap {
                bitmap: rasterizer.ok_or(Refusal::EngineTexture(id))?, image: rasterizer_image(id),
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
            copies_target: false,
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
        let pass = unlit_bsp_pass(&selected, &EngineInputs::default(), Some(cube)).unwrap();
        assert_eq!(pass.look.implementation, 3);
        assert_eq!(pass.look.vertex_shader, selected.implementation.vertex_shader.unwrap());
        assert_eq!(pass.look.textures[3], Stage::Bitmap { bitmap: cube, image: 0 });
        assert_eq!(unlit_bsp_pass(&selected, &EngineInputs::default(), None), Err(Refusal::EngineTexture(1)));
        selected.implementation = opaque(implementation(0, true), &[]);
        assert_eq!(unlit_bsp_pass(&selected, &EngineInputs::default(), Some(cube)), Err(Refusal::MissingImplementation(3)));
        selected.layer_passes = 2;
        assert_eq!(unlit_bsp_pass(&selected, &EngineInputs::default(), Some(cube)), Err(Refusal::Passes(2)));
    }

    #[test]
    fn engine_textures_14_and_15_are_the_rasterizer_bitmap_s_first_and_second_image() {
        let mut imp = implementation(0, true);
        imp.textures[1] = texture(TextureSource::Engine(14));
        imp.textures[2] = texture(TextureSource::Engine(15));
        let bitmap = TagId(0xe000_0042);
        let look = implementation_look_with_rasterizer(TagId(1), &imp, Some(bitmap)).unwrap();
        assert_eq!(look.textures[1], Stage::Bitmap { bitmap, image: 0 });
        assert_eq!(look.textures[2], Stage::Bitmap { bitmap, image: 1 });
        // Without the bitmap, or through the plain look, 14 refuses first.
        assert_eq!(implementation_look_with_rasterizer(TagId(1), &imp, None), Err(Refusal::EngineTexture(14)));
        assert_eq!(implementation_look(TagId(1), &imp), Err(Refusal::EngineTexture(14)));
        imp.textures[1] = None;
        assert_eq!(implementation_look(TagId(1), &imp), Err(Refusal::EngineTexture(15)));
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

    /// A layer-3 pass whose constants animate stays on the D3D path
    /// without its properties, and compiles at clock 0 with them.
    #[test]
    fn an_animated_layer3_pass_compiles_with_its_animation() {
        let (target, props) = crate::layer_pass::tests::animated_z();
        let mut imp = opaque(implementation(0, true), &[]);
        imp.animated = vec![target];
        let m = material(3, vec![vec![imp]]);
        assert_eq!(layer3_pass(&m), Err(Refusal::Animated));
        assert_eq!(layer3_pass_sampled(&m, &[]).unwrap_err(), Refusal::AnimationSample("property"));
        let (pass, animation) = layer3_pass_sampled(&m, &props).unwrap();
        let animation = animation.expect("kept for the frame's clock");
        let c18 = pass.look.vertex_constants.iter().find(|r| r.index == 18).unwrap();
        assert_eq!(c18.value[2], 0.0, "clock 0");
        let v = animation.sample(Some(11.0)).unwrap().vertex(&shaders::vsh::VertexUniforms::default());
        assert_eq!(v.constants[18][2], 1.5);
        let still = material(3, vec![vec![opaque(implementation(0, true), &[])]]);
        let (pass, none) = layer3_pass_sampled(&still, &props).unwrap();
        assert!(none.is_none());
        assert_eq!(pass, layer3_pass(&still).unwrap());
    }

    /// An engine-driven alpha reference from the instance's scalar 21
    /// lands in the pass's alpha test, after the pass's own value; a
    /// driven LOD bias in its stage's states.
    #[test]
    fn a_world_pass_takes_its_engine_driven_states_from_the_instance() {
        use gfx::state::{CompareFunc, RenderStateType as R};
        let mut imp = opaque(
            implementation(0, true),
            &[(R::AlphaTestEnable, 1), (R::AlphaRef, 0xff), (R::AlphaFunc, CompareFunc::Less.xbox_value())],
        );
        imp.input_reads.scalars = 1 << 21;
        imp.stage_states = vec![StageStates {
            address_u: 1,
            address_v: 1,
            address_w: 1,
            mag_filter: 2,
            min_filter: 2,
            mip_filter: 2,
            mip_lod_bias: 0,
            max_mip_level: 0,
            max_anisotropy: 1,
            color_key_op: 0,
            color_sign: 0,
            alpha_kill: 0,
            border_color: 0,
            color_key_color: 0,
            unread: [0; 2],
        }];
        // ALPHAREF (0x3d) and stage 0's MIPMAPLODBIAS from scalar 21.
        imp.engine_render_states = vec![ExternalBinding { bytes: [2, 1, 0, 21] }];
        imp.engine_stage_states = vec![ExternalBinding { bytes: [0, 0, 1, 21] }];
        let mut m = material(3, vec![vec![imp]]);
        m.inputs.scalars[21] = 64.0;
        let p = layer3_pass(&m).unwrap();
        assert_eq!(p.state.alpha_test, Some((wgpu::CompareFunction::Less, 64)));
        assert_eq!(f32::from_bits(p.look.stage_states[0].mip_lod_bias), 64.0);
    }

    #[test]
    fn a_world_replacement_blend_needs_an_explicit_operation() {
        use gfx::state::{BlendFactor, BlendOp, RenderStateType as R};
        let mut states = vec![(R::AlphaBlendEnable, 1), (R::SrcBlend, BlendFactor::One.xbox_value()),
            (R::DestBlend, BlendFactor::Zero.xbox_value())];
        let build = |states: &[_]| layer3_pass(&material(3, vec![vec![opaque(implementation(0, true), states)]]));
        assert_eq!(build(&states).unwrap_err(), Refusal::Blended);
        states.push((R::BlendOp, BlendOp::Add.xbox_value()));
        assert_eq!(build(&states).unwrap().state.blend, None);
        states.push((R::BlendOp, BlendOp::Max.xbox_value()));
        assert_eq!(build(&states).unwrap_err(), Refusal::Blended);
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
        assert!(
            layer3_pass(&material(3, vec![vec![opaque(implementation(0, true), &[(R::FogEnable, 1)])]])).is_ok(),
            "fog the combiner does not read draws nothing"
        );
        // Engine-driven states from a draw's input (scalar 3) or past the
        // tables.
        let mut engine = opaque(implementation(0, true), &[]);
        engine.input_reads.scalars = 1 << 3;
        engine.engine_render_states = vec![ExternalBinding { bytes: [2, 1, 0, 3] }];
        assert_eq!(refusal(engine), Refusal::EngineStates(EngineStateRefusal::PerDraw { state: 0x3d, stage: None }));
        let mut engine = opaque(implementation(0, true), &[]);
        engine.engine_stage_states = vec![ExternalBinding { bytes: [0, 10, 1, 0] }];
        assert_eq!(refusal(engine), Refusal::EngineStates(EngineStateRefusal::Undecoded));
        // Every layer-3 refusal still applies.
        let mut texaccum = opaque(implementation(0, true), &[]);
        texaccum.textures[0] = texture(TextureSource::Engine(3));
        let mut binding = tags::material::EngineVertexConstant { bank: 1, reg: 1, mode: 9, stage: 1, source: 0 };
        texaccum.vertex_constants[0].origin = Origin::Engine;
        texaccum.dimension_bindings.push(binding);
        // Stage 1's later width occupies x of the same row.
        binding.stage = 2;
        binding.mode = 5;
        texaccum.dimension_bindings.push(binding);
        let pass = layer3_pass(&material(3, vec![vec![texaccum]])).unwrap();
        assert_eq!(pass.look.textures[0], Stage::Engine(3));
        assert_eq!(pass.texaccum_dimensions[0].1, 0b0010);
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
