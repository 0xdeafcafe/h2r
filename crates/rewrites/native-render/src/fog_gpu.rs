//! Stage 4b's GPU side: the world's layer-16 passes, each compiled with
//! its own programs ([`crate::program::compile_pass`]) and drawn with its
//! own blend, depth and cull state ([`crate::pass_state`]), over the
//! loaded depth (docs/graphics/native-water-fog.md, "Stage 4b: planar
//! fog").
//!
//! The look is layer 3's ([`crate::look::implementation_look`]) plus the
//! one thing that made layer 3 refuse a fog pass: the combiner reading
//! the fog register, whose colour is `SET_FOG_COLOR` and whose factor the
//! NV2A fog unit's. Both are device state at the layer, resolved per view
//! ([`FogWords`]) and written into the programs' pixel uniforms
//! ([`FogSet::prepare`]). The vertex side needs nothing new: the pass's
//! own program writes `oFog` as the D3D path's does, and the module is
//! the D3D path's, text for text.
//!
//! A material is drawn natively only when every pass of its layer 16
//! translates and uploads; anything else keeps its D3D draws (the load
//! logs why, most parts first).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::gpu::Image;
use crate::look::{implementation_look_with_rasterizer, PassLook, Refusal, Stage};
use crate::pass_state::{pass_state, PassState, StateRefusal};
use crate::program::{
    compile_pass, described_pipeline, world_constants_read, Gpu, MaterialProgram, PipelineUse, Rigid, RigidProgram, ViewInputs,
};
use crate::world::{LightmapKey, ModelRows, Owner, World};
use gfx::state::RenderStateType;
use gfx::xdk::{HALO2_BUILD, RenderStateTable};
use shaders::psh::{self, PixelShaderDef, TexMode};
use tags::material::{Material, Register, ResolvedImplementation};
use shaders::vsh::VertexUniforms;
use tags::shader::RenderStateValue;

/// The render-state slots that set the fog a draw takes: `D3DRS_FOGENABLE`
/// (92, `RenderStateType::FogEnable`), the float states `FOGTABLEMODE`,
/// `FOGSTART`, `FOGEND`, `FOGDENSITY`, `RANGEFOGENABLE` (93..97) and
/// `D3DRS_FOGCOLOR` (138, `RenderStateType::FogColor`). The canonical
/// indices are the guest slots below 154 (`gfx::xdk`).
pub const FOG_WORD_SLOTS: [u32; 7] = [92, 93, 94, 95, 96, 97, 138];
const FOG_ENABLE: u32 = 92;
const FOG_TABLE_MODE: u32 = 93;
const FOG_START: u32 = 94;
const FOG_END: u32 = 95;
const FOG_DENSITY: u32 = 96;
const FOG_RANGE: u32 = 97;
const FOG_COLOR: u32 = 138;

/// Shader ids the fog's packets report to the pass they join (for
/// diagnostics; the pipelines are the passes' own).
const FOG_CLUSTER_PROGRAM: gfx::pipeline::ShaderId = gfx::pipeline::ShaderId(u32::MAX - 3);
const FOG_RIGID_PROGRAM: gfx::pipeline::ShaderId = gfx::pipeline::ShaderId(u32::MAX - 4);

/// The fog-state words a pass implementation sets itself, by slot
/// ([`FOG_WORD_SLOTS`]): `None` where it leaves the device's. The words
/// are the raw `D3DRS` values (`f32` bits for the float states, an
/// `A8R8G8B8` colour for `FOGCOLOR`); [`FogWords::resolve`] overlays them
/// on the device's words, and the glue turns the result into the fog
/// unit with `gfx::guest_program::d3d_fog`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FogWords {
    pub enable: Option<u32>,
    pub table_mode: Option<u32>,
    pub start: Option<u32>,
    pub end: Option<u32>,
    pub density: Option<u32>,
    pub range: Option<u32>,
    pub color: Option<u32>,
}

impl FogWords {
    /// The words an implementation's render states set, `None` where it
    /// leaves the device's.
    pub fn from_states(states: &[RenderStateValue]) -> Self {
        let mut w = FogWords::default();
        for s in states {
            match u32::from(s.state) {
                FOG_ENABLE => w.enable = Some(s.value),
                FOG_TABLE_MODE => w.table_mode = Some(s.value),
                FOG_START => w.start = Some(s.value),
                FOG_END => w.end = Some(s.value),
                FOG_DENSITY => w.density = Some(s.value),
                FOG_RANGE => w.range = Some(s.value),
                FOG_COLOR => w.color = Some(s.value),
                _ => {}
            }
        }
        w
    }

    /// The words this pass's fog state resolves to over the device's:
    /// `device[i]` is the device's word at `FOG_WORD_SLOTS[i]`, read
    /// from the scene view at the layer-16 call (`None`
    /// a slot that does not read; such a view is refused).
    pub fn resolve(&self, device: &[Option<u32>; 7]) -> Option<[u32; 7]> {
        let own = [self.enable, self.table_mode, self.start, self.end, self.density, self.range, self.color];
        let mut out = [0u32; 7];
        for (o, (mine, held)) in out.iter_mut().zip(own.iter().zip(device)) {
            *o = mine.or(*held)?;
        }
        Some(out)
    }
}

/// One pass of a material's layer 16 with its pipeline state and the fog
/// words it sets itself.
#[derive(Clone, Debug, PartialEq)]
pub struct FogPass {
    pub look: PassLook,
    pub state: PassState,
    pub fog: FogWords,
    pub planar: Option<PlanarBindings>,
}

/// Pixel registers written by the planar implementation's engine externals.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanarBindings {
    vectors: Vec<tags::material::VectorBinding>,
    scalars: Vec<tags::material::VectorBinding>,
}

impl PlanarBindings {
    fn from_implementation(imp: &ResolvedImplementation) -> Option<Self> {
        use tags::material::VectorTarget;
        let reads = imp.input_reads;
        if reads.ints != 0 || reads.vectors & !(1u64 << 31) != 0
            || reads.scalars & !((1u64 << 35) | (1u64 << 36)) != 0
            || imp.variant || !imp.pixel_constants_from_engine
            || !imp.vector_bindings.iter().all(|b| b.source == 31)
            || !imp.scalar_bindings.iter().all(|b| matches!(b.source, 35 | 36))
            || !imp.vector_bindings.iter().chain(&imp.scalar_bindings)
                .all(|b| matches!(b.target, VectorTarget::Pixel { slot: 0..=8, .. }))
        {
            return None;
        }
        Some(Self { vectors: imp.vector_bindings.clone(), scalars: imp.scalar_bindings.clone() })
    }

    pub fn sample(&self, base: &psh::PixelUniforms, inputs: &PlanarInputs) -> LayerConstants {
        use tags::material::{VectorTarget, engine_scalar, pack_engine_colour};
        let mut pixel = *base;
        for (bindings, vector) in [(&self.vectors, true), (&self.scalars, false)] {
            for b in bindings {
                let VectorTarget::Pixel { c1, slot, mask, half, transform } = b.target else { unreachable!() };
                let value = if vector {
                    [inputs.colour[0], inputs.colour[1], inputs.colour[2], 0.0]
                } else {
                    [engine_scalar(inputs.weights[usize::from(b.source - 35)], usize::from(transform)); 4]
                };
                crate::object_program::put_packed(&mut pixel, c1, slot, mask, pack_engine_colour(value, half));
            }
        }
        LayerConstants { vertex: Vec::new(), defaults: Vec::new(), c0: pixel.c0, c1: pixel.c1 }
    }
}

/// The two fade weights and colour in the current view's render context.
pub struct PlanarInputs {
    colour: [f32; 3],
    weights: [f32; 2],
}

impl PlanarInputs {
    pub fn read(mem: &dyn crate::camera::GuestRead) -> Option<Self> {
        use scene_extract::layout::layer_inputs::{scalar, vector};
        Some(Self { colour: vector(mem, 31, None)?, weights: [scalar(mem, 35)?, scalar(mem, 36)?] })
    }
}

/// Why a material's layer stays on the D3D path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FogRefusal {
    /// Its template draws nothing in the layer at the first level of
    /// detail.
    NoPass { layer: usize },
    /// Pass `pass` has `count` implementations and the caller did not
    /// say which the draw binds, or named one it lacks
    /// ([`Selection::implementation`]).
    Implementations { pass: usize, count: usize },
    /// Pass `pass`'s implementation does not fit ([`Refusal`]).
    Look { pass: usize, refusal: Refusal },
    /// Pass `pass`'s render states do not ([`StateRefusal`], as
    /// `pass_state` decodes them; the fog words are not the pipeline's,
    /// and the fog unit is resolved per view here, [`FogWords`]).
    State { pass: usize, refusal: StateRefusal },
    /// Pass `pass` does not set `CULLMODE`, and some pass of the layer
    /// sets a cull other than the layer's start ([`layer_cull`]), so what
    /// the pass draws with depends on the draw before it
    /// ([`Selection::layer_cull`]).
    InheritsCull { pass: usize },
    /// Pass `pass`'s definition has no final combiner of its own: D3D
    /// builds one from the fog and specular states, which the native
    /// pass does not model (first slice).
    NoFinalCombiner { pass: usize },
    /// Pass `pass` has a stage whose texture mode reads device state the
    /// native pass does not have (the bump-environment matrices, the eye
    /// of the reflection modes, BRDF).
    DeviceState { pass: usize, stage: usize },
    /// Pass `pass`, stage `stage` binds engine texture `id` that no
    /// per-view supply resolves (run-time chosen, the light's, a
    /// rasterizer bitmap, a cleared stage, invalid): the supplied-look
    /// path ([`supplied_layer_passes`]) only takes the view's own target
    /// and the render targets.
    Unsupplied { pass: usize, stage: usize, id: u8 },
    /// Pass `pass`'s translation refused it (`crate::program::compile_pass`:
    /// per-draw engine inputs, a constant the native pass does not set, a
    /// stage with nothing bound, ...).
    Compile { pass: usize, reason: String },
}

impl FogRefusal {
    /// One line for the load's log.
    pub fn describe(&self) -> String {
        match self {
            FogRefusal::NoPass { layer } => format!("no pass in layer {layer}"),
            FogRefusal::Implementations { pass, count } => format!("{count} implementations of pass {pass}"),
            FogRefusal::Look { pass, refusal } => format!("pass {pass}: {refusal:?}"),
            FogRefusal::State { pass, refusal } => format!("pass {pass} state: {refusal:?}"),
            FogRefusal::InheritsCull { pass } => format!("pass {pass} leaves CULLMODE to the previous draw"),
            FogRefusal::NoFinalCombiner { pass } => format!("pass {pass} has no final combiner of its own"),
            FogRefusal::DeviceState { pass, stage } => format!("pass {pass} stage {stage} reads device state"),
            FogRefusal::Unsupplied { pass, stage, id } => {
                format!("pass {pass} stage {stage} binds engine texture {id}, which no per-view supply resolves")
            }
            FogRefusal::Compile { pass, reason } => format!("pass {pass}: {reason}"),
        }
    }
}

/// What the caller knows of the draw that the material alone does not
/// say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// The implementation the engine binds for the item, which every pass
    /// with more than one takes (a pass with one takes it, whatever this
    /// says: the layer-draw rule `layer_pass::passes_of` applies). `None`:
    /// a pass with more than one is refused.
    pub implementation: Option<usize>,
    /// Every pass of the layer sets the cull the layer starts with or
    /// none ([`layer_sets_other_cull`] is false), so a pass that leaves
    /// `CULLMODE` unset draws with [`layer_cull`]. `false`: such a pass
    /// is refused ([`FogRefusal::InheritsCull`]).
    pub layer_cull: bool,
    /// The bitmap of the globals' rasterizer data at `+0x14`, which
    /// engine textures 14 and 15 bind (`0x00019530`). `None`: a stage
    /// naming one refuses the material.
    pub rasterizer: Option<map::TagId>,
}

/// The cull a layer's draws start with, and keep until a pass sets
/// another: the shader draw begin `0x0001bbf0` runs the layer setup on the
/// first draw after a layer draw (`[0x004670bc]`, set by the layer draw),
/// which calls `0x003f53d0` with `0x900` (`SET_CULL_FACE_ENABLE`,
/// `0x40308`: `CULLMODE` `CW`) and `0x003f5450` with `0x900`
/// (`SET_FRONT_FACE`, `0x403a0`: `FRONTFACE` `CW`). Nothing restores the
/// cull between the draws, so an unset pass takes the draw before's.
pub fn layer_cull() -> (gfx::state::CullMode, gfx::state::FrontFace) {
    (gfx::state::CullMode::Cw, gfx::state::FrontFace::Cw)
}

/// [`layer_cull`] as the primitive state's cull and winding.
fn layer_cull_primitive() -> (Option<wgpu::Face>, wgpu::FrontFace) {
    let (cull_mode, front_face) = layer_cull();
    let rs = gfx::state::RenderState { cull_mode, front_face, ..Default::default() };
    let p = gfx::translate::primitive_state(
        &rs,
        wgpu::PrimitiveTopology::TriangleList,
        gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
    );
    (p.cull_mode, p.front_face)
}

/// Whether any implementation of any pass of `material`'s `layer` (first
/// level of detail) sets `CULLMODE` or `FRONTFACE` to something other
/// than [`layer_cull`]'s: then a pass that leaves them unset may draw
/// after it and inherit it. Conservative: the implementations'
/// `restore_states` are not credited with putting the cull back, and a
/// value `gfx` does not decode counts. A host asks every material of the
/// world it draws the layer for, and sets [`Selection::layer_cull`] when
/// none does.
pub fn layer_sets_other_cull(material: &Material, layer: usize) -> bool {
    use gfx::state::{CullMode, FrontFace};
    let table = RenderStateTable::for_build(HALO2_BUILD);
    let start = layer_cull();
    material
        .layers
        .iter()
        .filter(|l| l.lod == 0 && l.layer == layer)
        .flat_map(|l| &l.passes)
        .flat_map(|p| &p.implementations)
        .any(|imp| {
            // The states the pass applies, its engine-driven ones over
            // its own where they resolve.
            let states = crate::engine_states::apply_static(imp, &material.inputs)
                .map(|a| a.render_states)
                .unwrap_or_else(|_| imp.render_states.clone());
            let (mut cull, mut front) = start;
            for s in &states {
                match RenderStateType::from_guest_index(&table, u32::from(s.state)) {
                    Some(RenderStateType::CullMode) => match CullMode::from_xbox(s.value) {
                        Some(c) => cull = c,
                        None => return true,
                    },
                    Some(RenderStateType::FrontFace) => match FrontFace::from_xbox(s.value) {
                        Some(f) => front = f,
                        None => return true,
                    },
                    _ => {}
                }
            }
            (cull, front) != start
        })
}

/// Whether any pass of `material`'s `layer` (first level of detail) has
/// more than one implementation: then which draws depends on the item
/// ([`Selection::implementation`]).
pub fn layer_has_choice(material: &Material, layer: usize) -> bool {
    material
        .layers
        .iter()
        .filter(|l| l.lod == 0 && l.layer == layer)
        .flat_map(|l| &l.passes)
        .any(|p| p.implementations.len() > 1)
}

/// Every pass of `material`'s `layer` (first level of detail), with its
/// look and state, in the order the layer draws them, or why not. All or
/// nothing per material: an item is drawn natively only when every pass
/// is. The compilation itself is the glue's (it owns the `vrtx` tag's
/// microcode, [`compile_pass`]). The look refuses engine-texture stages
/// ([`Refusal::EngineTexture`]); [`supplied_layer_passes`] is the same
/// with the per-view supplied ones kept. A pass with more than one
/// implementation, or one that leaves `CULLMODE` to the draw before,
/// refuses here; [`layer_passes_selected`] takes them where the caller
/// knows which.
pub fn layer_passes(material: &Material, layer: usize) -> Result<Vec<FogPass>, FogRefusal> {
    passes_of(material, layer, false, Selection::default(), None)
}

/// [`layer_passes`] for a draw that binds [`Selection::implementation`]
/// and, with [`Selection::layer_cull`], takes the layer's start cull for a
/// pass that leaves `CULLMODE` unset.
pub fn layer_passes_selected(material: &Material, layer: usize, selection: Selection) -> Result<Vec<FogPass>, FogRefusal> {
    passes_of(material, layer, false, selection, None)
}

/// The implementation each pass of `material`'s `layer` (LOD 0) binds,
/// in draw order, chosen as [`layer_passes_selected`] chooses with
/// `implementation`: what the guest's draw of an item sets and restores,
/// so a skip of it can be checked against the device
/// (`layer_pass::skip_holds`). `None` when the layer has no pass or a
/// pass's choice is not given.
pub fn layer_implementations(material: &Material, layer: usize, implementation: Option<usize>) -> Option<Vec<ResolvedImplementation>> {
    let layer = material.layers.iter().find(|l| l.lod == 0 && l.layer == layer).filter(|l| !l.passes.is_empty())?;
    layer
        .passes
        .iter()
        .map(|pass| match pass.implementations.as_slice() {
            [imp] => Some(imp.clone()),
            all => implementation.and_then(|i| all.get(i)).cloned(),
        })
        .collect()
}

/// [`layer_passes`] with the engine-texture stages a per-view supply
/// resolves kept as [`Stage::Engine`]: the view's own target (id 5) and
/// the render targets (`crate::screen_look::engine_texture`'s `Target`s),
/// what the water edge blend binds. An id nothing supplies refuses the
/// material ([`FogRefusal::Unsupplied`]).
pub fn supplied_layer_passes(material: &Material, layer: usize, rasterizer: Option<map::TagId>) -> Result<Vec<FogPass>, FogRefusal> {
    passes_of(material, layer, true, Selection { rasterizer, ..Selection::default() }, None)
}

/// Compiles the same pass shape the layer planner samples at draw time.
/// A sample supplies the registers and defaults for vertex input validation.
pub(crate) fn sampled_layer_passes(
    material: &Material, layer: usize, selection: Selection, properties: &[tags::shader::AnimatedProperty],
) -> Result<Vec<FogPass>, FogRefusal> {
    passes_of(material, layer, false, selection, Some(properties))
}

fn passes_of(
    material: &Material, layer: usize, supplied: bool, selection: Selection,
    properties: Option<&[tags::shader::AnimatedProperty]>,
) -> Result<Vec<FogPass>, FogRefusal> {
    let layer = material
        .layers
        .iter()
        .find(|l| l.lod == 0 && l.layer == layer)
        .filter(|l| !l.passes.is_empty())
        .ok_or(FogRefusal::NoPass { layer })?;
    let mut out = Vec::with_capacity(layer.passes.len());
    for (n, pass) in layer.passes.iter().enumerate() {
        let imp = match pass.implementations.as_slice() {
            [imp] => imp,
            all => selection
                .implementation
                .and_then(|i| all.get(i))
                .ok_or(FogRefusal::Implementations { pass: n, count: all.len() })?,
        };
        let sampled;
        let imp = if let Some(properties) = properties.filter(|_| !imp.animated.is_empty()) {
            sampled = crate::layer_pass::PassAnimation::baseline(imp, properties, false, n)
                .map_err(|reason| FogRefusal::Compile { pass: n, reason: format!("{}: {reason:?}", reason.name()) })?
                .0;
            &sampled
        } else {
            imp
        };
        let mut look = if supplied {
            // `implementation_look` with the engine stages kept when a
            // per-view supply resolves them.
            let mut textures = [Stage::Empty; 4];
            for (n_stage, (stage, t)) in textures.iter_mut().zip(&imp.textures).enumerate() {
                *stage = match t.map(|t| t.source) {
                    None | Some(tags::material::TextureSource::None) => Stage::Empty,
                    Some(tags::material::TextureSource::Bitmap { bitmap, image, .. }) => Stage::Bitmap { bitmap, image },
                    Some(tags::material::TextureSource::Lightmap) => Stage::Lightmap,
                    Some(tags::material::TextureSource::Engine(id @ (14 | 15))) if selection.rasterizer.is_some() => {
                        Stage::Bitmap {
                            bitmap: selection.rasterizer.unwrap_or(map::TagId::NULL),
                            image: crate::look::rasterizer_image(id),
                        }
                    }
                    Some(tags::material::TextureSource::Engine(id)) => {
                        match crate::screen_look::engine_texture(id) {
                            crate::screen_look::EngineTexture::View | crate::screen_look::EngineTexture::Target(_) => {
                                Stage::Engine(id)
                            }
                            _ => return Err(FogRefusal::Unsupplied { pass: n, stage: n_stage, id }),
                        }
                    }
                    Some(tags::material::TextureSource::Default) => {
                        return Err(FogRefusal::Look { pass: n, refusal: Refusal::DefaultTexture })
                    }
                    Some(tags::material::TextureSource::UnsetSlot(s)) => {
                        return Err(FogRefusal::Look { pass: n, refusal: Refusal::UnsetSlot(s) })
                    }
                };
            }
            PassLook {
                pass: pass.pass,
                implementation: imp.index,
                vertex_shader: imp
                    .vertex_shader
                    .ok_or(FogRefusal::Look { pass: n, refusal: Refusal::NoVertexShader })?,
                textures,
                stage_states: imp.stage_states.clone(),
                pixel_shader: imp.pixel_shader,
                vertex_constants: imp.vertex_constants.clone(),
                vertex_defaults: imp.vertex_defaults.clone(),
                per_draw_inputs: imp.per_draw_inputs,
            }
        } else {
            implementation_look_with_rasterizer(pass.pass, imp, selection.rasterizer)
                .map_err(|refusal| FogRefusal::Look { pass: n, refusal })?
        };
        if !imp.animated.is_empty() {
            return Err(FogRefusal::Look { pass: n, refusal: Refusal::Animated });
        }
        if let Some(r) = imp.vertex_constants.iter().find(|r| r.origin == tags::material::Origin::Engine) {
            return Err(FogRefusal::Look { pass: n, refusal: Refusal::EngineVertexConstant(r.index) });
        }
        // Engine-driven states over the pass's own, from the instance's
        // inputs (a driven fog colour with the fog on refuses: the fog
        // words are the pass's own, [`FogWords`]).
        let applied = crate::engine_states::apply_static(imp, &material.inputs)
            .map_err(|r| FogRefusal::Look { pass: n, refusal: Refusal::EngineStates(r) })?;
        look.stage_states = applied.stage_states;
        let states = applied.render_states;
        // Input masks also request evaluations whose result is never bound.
        // The states above are resolved. Without a register write or a
        // dynamic fragment selector, the shader itself is view-independent.
        if layer.layer == crate::fog::FOG_LAYER
            && !imp.pixel_constants_from_engine
            && imp.vertex_defaults.iter().all(|r| r.origin != tags::material::Origin::Engine)
            && (!imp.variant || imp.input_reads.ints & !tags::material::INSTANCE_INTS == 0)
        {
            look.per_draw_inputs = false;
        }
        match fog_parity(&look) {
            Ok(()) => {}
            Err(FogParity::NoFinalCombiner) => return Err(FogRefusal::NoFinalCombiner { pass: n }),
            Err(FogParity::DeviceState { stage }) => return Err(FogRefusal::DeviceState { pass: n, stage }),
        }
        // `pass_state`'s rules: a fog pass may turn the fog on itself,
        // and the fog unit is resolved per view ([`FogWords`]). The
        // pipeline state must be the implementation's own (a pass that
        // leaves blend or depth to the previous draw stays on the D3D
        // path, as the transparents').
        let mut state = layer_pass_state(&states, matches!(layer.layer, 1 | 3 | 5 | 6 | 7 | 8 | 16 | 20)).map_err(|refusal| FogRefusal::State { pass: n, refusal })?;
        if !sets_cull(&states) {
            if !selection.layer_cull {
                return Err(FogRefusal::InheritsCull { pass: n });
            }
            // `pass_state` leaves an unset cull at `RenderState`'s default;
            // the layer's draws start with [`layer_cull`].
            (state.primitive.cull_mode, state.primitive.front_face) = layer_cull_primitive();
        }
        let planar = if layer.layer == crate::fog::FOG_LAYER && look.per_draw_inputs {
            let bindings = PlanarBindings::from_implementation(imp).ok_or_else(|| FogRefusal::Compile {
                pass: n, reason: format!("unsupported planar engine inputs {:?}", imp.input_reads),
            })?;
            look.per_draw_inputs = false;
            Some(bindings)
        } else { None };
        out.push(FogPass { look, state, fog: FogWords::from_states(&states), planar });
    }
    Ok(out)
}

/// [`pass_state`] over the implementation's states, the geometry layers'
/// colour-mask rule where `geometry`. The fog words decode to no pipeline
/// state; they are [`FogWords`]'s.
fn layer_pass_state(states: &[RenderStateValue], geometry: bool) -> Result<PassState, StateRefusal> {
    if geometry { crate::pass_state::geometry_pass_state(states) } else { pass_state(states) }
}

/// Whether `states` set `CULLMODE` (build 5849's slots).
fn sets_cull(states: &[RenderStateValue]) -> bool {
    let table = RenderStateTable::for_build(HALO2_BUILD);
    states
        .iter()
        .any(|s| RenderStateType::from_guest_index(&table, u32::from(s.state)) == Some(RenderStateType::CullMode))
}

/// What in a pass's pixel shader definition still refuses it for the fog
/// layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FogParity {
    /// The definition has no final combiner of its own: D3D builds one
    /// from the fog and specular states.
    NoFinalCombiner,
    /// A stage's texture mode reads device state the native pass does not
    /// have.
    DeviceState { stage: usize },
}

/// [`crate::program::world_parity`] for the fog layer: the fog register
/// is the point of the pass and allowed (the state comes per view,
/// [`FogWords`]); what still refuses is a definition without its own
/// final combiner and the texture modes that read other device state.
/// The vertex-constant and border checks stay with the compile
/// ([`compile_pass`]).
fn fog_parity(look: &PassLook) -> Result<(), FogParity> {
    let Ok(def) = PixelShaderDef::from_bytes(&look.pixel_shader) else {
        return Err(FogParity::NoFinalCombiner);
    };
    let c = def.decode();
    if c.final_combiner.is_none() {
        return Err(FogParity::NoFinalCombiner);
    }
    for (n, m) in c.texture_modes.iter().enumerate() {
        if matches!(
            m,
            TexMode::BumpEnvMap | TexMode::BumpEnvMapLum | TexMode::Brdf | TexMode::DotRflctDiff | TexMode::DotRflctSpec | TexMode::DotRflctSpecConst
        ) {
            return Err(FogParity::DeviceState { stage: n });
        }
    }
    Ok(())
}

/// A later layer's parts drawn from the cull's slots
/// (`GpuWorld::culled_layer_draws`): each packet one indirect draw of its
/// part's slot in `early`, and the part's start value written into
/// `start` ([`crate::cull_gpu::OCCLUDABLE`] when every pass's hidden
/// fragments draw nothing, `gpu::hidden_draws_nothing`; else
/// [`crate::cull_gpu::ORDERED`]).
pub(crate) struct LayerSlots<'a> {
    pub early: &'a wgpu::Buffer,
    pub slots: &'a HashMap<(Owner, u8), usize>,
    pub start: &'a mut [u32],
}

/// The stencil a world layer's native draws take: the device's at the
/// layer call, as it is (`0x00015370`'s mode there: 0, the test off, or 1,
/// `ALWAYS` writing reference 0 on pass, which the scene's layers 1 and 3
/// begin in; `object_state::structure_stencil`). A pass that sets one of
/// its own draws that instead (`PassState::stencil`,
/// `program::described_pipeline`), and the structure items a layer's
/// native pass takes come before any object item the guest keeps that
/// switches it (`layer_pass::clear_layer_items`), so every one of them draws under that
/// state. The pipelines' stencil state (the packets set its reference,
/// [`FogSet::draws_stencil`]). A `depth` with no stencil aspect is a
/// guest `D16` surface, which has no stencil on the Xbox either: the test
/// is off there, as the D3D path's pipeline takes it
/// (`gfx::translate::depth_stencil_state`).
pub fn layer_stencil(device: &gfx::state::StencilState, depth: wgpu::TextureFormat) -> wgpu::StencilState {
    if !device.enable || !depth.has_stencil_aspect() {
        return wgpu::StencilState::default();
    }
    let rs = gfx::state::RenderState { stencil: *device, ..Default::default() };
    gfx::translate::depth_stencil_state(&rs, Some(depth), wgpu::PrimitiveTopology::TriangleList)
        .map_or_else(wgpu::StencilState::default, |d| d.stencil)
}

/// Whether drawing under `device`'s stencil changes the stencil buffer:
/// the test on, a write mask, and an operation other than `KEEP`.
pub fn stencil_writes(device: &gfx::state::StencilState) -> bool {
    use gfx::state::StencilOp::Keep;
    device.enable && device.write_mask != 0 && [device.fail, device.depth_fail, device.pass].iter().any(|&op| op != Keep)
}

/// One compiled layer-16 pass of a material.
#[derive(Clone, Debug)]
pub struct FogProgram {
    pub planar: Option<PlanarBindings>,
    pub program: MaterialProgram,
    pub state: PassState,
    /// The fog words the pass's implementation sets itself.
    pub words: FogWords,
    /// The pass with its vertex program's rigid classification, which
    /// draws the material's instanced placements (`compile_pass_rigid`,
    /// their rows in `c[50..52]` per draw as `0x0004d830` uploads them);
    /// `None` leaves them to the D3D path.
    pub rigid: Option<RigidProgram>,
}

impl FogProgram {
    /// Compiles `pass` with its vertex program `microcode` (the
    /// world-space classification, [`crate::look::world_program`]), or
    /// says why it stays on the D3D path: [`compile_pass`] plus the
    /// world's constants check ([`world_constants_read`]: every constant
    /// the program reads must be one the native pass sets — the view's
    /// `c[0..18]`, the instance's, the model rows and the decompression
    /// rows; whatever `0x25d50` uploads beyond them shows here).
    pub fn compile(pass: &FogPass, microcode: &[u8]) -> Result<Self, String> {
        Self::compile_rigid_pass(pass, microcode, None)
    }

    /// [`FogProgram::compile`] with the pass's rigid classification
    /// `rigid_microcode` (the `vrtx` tag's classification 1,
    /// [`crate::look::rigid_program`]): the placements' draws take it,
    /// with their rows per draw.
    pub fn compile_rigid_pass(
        pass: &FogPass,
        microcode: &[u8],
        rigid_microcode: Option<&[u8]>,
    ) -> Result<Self, String> {
        world_constants_read(&pass.look, microcode)?;
        let mut program = compile_pass(&pass.look, microcode, pass.state.alpha_test)?;
        // The pass's polygon offset is its fragments' (`frame_pixel` keeps
        // it under the frame's depth scale).
        crate::pass_state::DepthOffset::apply(pass.state.depth_offset, &mut program.pixel);
        shaders::vsh::Program::parse(microcode).map_err(|e| format!("vertex program: {e:?}"))?;
        let rigid = rigid_microcode
            .map(|code| crate::program::compile_pass_rigid(&pass.look, pass.state.alpha_test, code))
            .transpose()?;
        Ok(FogProgram { program, state: pass.state, words: pass.fog, planar: pass.planar.clone(), rigid })
    }
}

/// Constants from one sampled implementation, in hardware register order.
/// View inputs, placement rows and fog still come from the draw.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerConstants {
    vertex: Vec<Register>,
    defaults: Vec<Register>,
    c0: [[f32; 4]; 9],
    c1: [[f32; 4]; 9],
}

impl LayerConstants {
    pub fn from_implementation(imp: &ResolvedImplementation) -> Self {
        let def = PixelShaderDef::from_bytes(&imp.pixel_shader).expect("fixed-size pixel shader definition");
        let (c0, c1) = def.embedded_constants();
        Self { vertex: imp.vertex_constants.clone(), defaults: imp.vertex_defaults.clone(), c0, c1 }
    }

    /// `base` with the sampled vertex constants and attribute defaults.
    pub fn vertex(&self, base: &VertexUniforms) -> VertexUniforms {
        let mut vertex = base.clone();
        for r in &self.vertex {
            crate::program::put(&mut vertex.constants, r);
        }
        for r in &self.defaults {
            crate::program::put(&mut vertex.defaults, r);
        }
        vertex
    }

    /// `base` with the sampled combiner constants (C0 and C1, the final
    /// combiner's included); fog and the view's words stay `base`'s.
    pub fn pixel(&self, base: &psh::PixelUniforms) -> psh::PixelUniforms {
        let mut pixel = *base;
        (pixel.c0, pixel.c1) = (self.c0, self.c1);
        pixel
    }
}

/// The sampled registers over a compiled pass's uniforms, followed by the fog
/// words for this draw. Vertex classification and per-view state survive.
fn layer_uniforms<'a>(
    sample: Option<&LayerConstants>, base: &'a MaterialProgram, fog: &psh::fog::Fog, color: [f32; 4],
) -> (std::borrow::Cow<'a, VertexUniforms>, psh::PixelUniforms) {
    let (vertex, mut pixel) = match sample {
        Some(s) => (std::borrow::Cow::Owned(s.vertex(&base.vertex)), s.pixel(&base.pixel)),
        None => (std::borrow::Cow::Borrowed(&base.vertex), base.pixel),
    };
    pixel.fog = fog.uniform();
    pixel.fog_color = color;
    (vertex, pixel)
}

/// One layer's time sample, per material and in compiled pass order.
pub type LayerSamples = HashMap<u16, Vec<Option<LayerConstants>>>;

pub fn sampled_constants(draw: &crate::layer_pass::LayerDraw) -> Option<Vec<Option<LayerConstants>>> {
    if !draw.passes.iter().any(|p| draw.implementation(p).is_some_and(|i| !i.animated.is_empty())) {
        return None;
    }
    Some(draw.passes.iter().map(|p| {
        let imp = draw.implementation(p)?;
        (!imp.animated.is_empty()).then(|| LayerConstants::from_implementation(imp))
    }).collect())
}

struct FogMaterial {
    /// The passes in draw order, a placement's draws with the pass's
    /// rigid program when it compiled one.
    passes: Vec<(Gpu, PassState, Option<Rigid>)>,
    /// Per lightmap page the material's parts use: one texture group per
    /// pass.
    groups: HashMap<Option<LightmapKey>, Vec<wgpu::BindGroup>>,
}

/// The fog materials the native pass draws, and each part's index range
/// (the glue's parts list, in the world's draw order).
pub struct FogSet {
    ready_formats: HashSet<(wgpu::TextureFormat, wgpu::TextureFormat, wgpu::StencilState)>,
    materials: HashMap<u16, FogMaterial>,
    /// Index range, lightmap page and material for each eligible part.
    parts: HashMap<(Owner, u8), (u32, u32, Option<LightmapKey>, u16)>,
    /// The lightmap page textures the groups bind, by page, for a live
    /// edit's rewrite ([`FogSet::page_textures`]).
    pages: HashMap<LightmapKey, wgpu::Texture>,
}

impl FogSet {
    /// Every texture the passes' stages sample as image `image` of
    /// `bitmap`, with the stage and its signed channels, each once.
    pub fn held(&self, bitmap: map::TagId, image: u32) -> Vec<crate::gpu::Held> {
        self.held_stage(Stage::Bitmap { bitmap, image })
    }

    /// Every texture the passes' stages sample as `want`, with the stage
    /// and its signed channels, each once: [`FogSet::held`] for a stage
    /// no tag names (the editor's base maps, `HostStage::BaseMap`).
    pub fn held_stage(&self, want: Stage) -> Vec<crate::gpu::Held> {
        let mut materials: Vec<_> = self.materials.iter().collect();
        materials.sort_by_key(|(m, _)| **m);
        let mut out = Vec::new();
        for (_, m) in materials {
            for (g, _, _) in &m.passes {
                g.held(want, &mut out);
            }
        }
        out
    }

    /// The textures that hold lightmap page `key`.
    pub fn page_textures(&self, key: LightmapKey) -> Vec<wgpu::Texture> {
        self.pages.get(&key).cloned().into_iter().collect()
    }

    /// Uploads `programs` (each material's layer-16 passes in order) for
    /// `parts` (the load's parts list: owner and part, index range,
    /// lightmap page and material, in the world's draw order).
    /// `stage_image` decodes a stage's bitmap image, `lightmap` a page. A
    /// material is kept only when every part of it binds: a part whose
    /// lightmap page does not decode keeps the whole material on the D3D
    /// path, as the world's load.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
        programs: HashMap<u16, Vec<FogProgram>>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap_image: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        // Layer 3 may have retained a different set of parts. Use the fog
        // parts to select the placement rows from the shared world.
        let mut placements: HashMap<u16, BTreeMap<u32, ModelRows>> = HashMap::new();
        for &((owner, _), (_, _, _, material)) in &parts {
            if let Owner::Instance(i) = owner
                && let Some(rows) = world.rows(i)
            {
                placements.entry(material).or_default().insert(i, *rows);
            }
        }
        let mut materials = HashMap::new();
        'material: for (material, passes) in programs {
            if passes.is_empty() {
                continue;
            }
            let words = passes[0].words;
            if passes.iter().any(|p| p.words != words) {
                continue; // the passes disagree on the fog: not drawn
            }
            let mut gpus = Vec::with_capacity(passes.len());
            for p in passes {
                let mut program = p.program;
                program.rigid = p.rigid.clone();
                let Some(g) = Gpu::new(device, queue, program, stage_image) else {
                    continue 'material;
                };
                // A placement's draws take the rigid program, one holder
                // per pass with its own pipeline keys
                // ([`FogMaterial::passes`]).
                let rigid = placements.get(&material).and_then(|slots| {
                    Rigid::with_slots(device, &g, slots.iter().map(|(&i, rows)| (i, *rows)).collect())
                });
                gpus.push((g, p.state, rigid));
            }
            materials.insert(material, FogMaterial { passes: gpus, groups: HashMap::new() });
        }
        // A placement part draws only when its material compiled a rigid
        // program for every pass; anything else stays on the D3D path (a
        // material with no placement parts needs none).
        let mut parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))> = parts;
        parts.retain(|&((owner, _), (_, _, _, material))| {
            let Some(m) = materials.get(&material) else { return false };
            match owner {
                Owner::Cluster(_) => true,
                Owner::Instance(i) => m.passes.iter().all(|(_, _, r)| r.as_ref().and_then(|r| r.offset(i)).is_some()),
            }
        });
        let mut pages: HashMap<LightmapKey, Option<wgpu::TextureView>> = HashMap::new();
        let mut out = Vec::new();
        let mut missing: Vec<u16> = Vec::new();
        for (key, (first_index, index_count, lightmap, material)) in parts {
            let Some(m) = materials.get_mut(&material) else { continue };
            if !m.groups.contains_key(&lightmap) {
                let page = match lightmap {
                    Some(k) => pages
                        .entry(k)
                        .or_insert_with(|| lightmap_image(k).map(|i| crate::gpu::texture(device, queue, &i, "native fog lightmap")))
                        .clone(),
                    None => None,
                };
                let groups = m
                    .passes
                    .iter()
                    .map(|(g, _, _)| g.textures(device, page.as_ref()))
                    .collect::<Option<Vec<_>>>();
                match groups {
                    Some(groups) => {
                        m.groups.insert(lightmap, groups);
                    }
                    None => {
                        missing.push(material); // a part's page does not bind
                        continue;
                    }
                }
            }
            out.push((key, (first_index, index_count, lightmap, material)));
        }
        for material in missing {
            materials.remove(&material);
        }
        out.retain(|(_, (_, _, _, mat))| materials.contains_key(mat));
        let pages = crate::gpu::page_textures_of(&pages);
        Self { ready_formats: HashSet::new(), materials, parts: out.into_iter().collect(), pages }
    }

    /// How many materials draw natively.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// Whether the set draws `part` of `owner` (whatever the shader): the
    /// `prepare_copy` check that every selected part will record.
    pub fn draws_part(&self, owner: Owner, part: u8) -> bool {
        self.parts.get(&(owner, part)).is_some_and(|&(_, count, _, _)| count != 0)
    }

    /// The frame's uniforms for every pass, with each material's fog
    /// resolved over the device's words (`fogs`, material by material:
    /// `FogWords::resolve` through `gfx::guest_program::d3d_fog` in the glue) and
    /// its colour (`psh::argb` of `FOGCOLOR`), written through the
    /// submission's upload arena.
    pub fn prepare(
        &self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        fogs: &HashMap<u16, (psh::fog::Fog, [f32; 4])>,
    ) {
        self.prepare_layer(uploads, view, fogs, &LayerSamples::new());
    }

    pub(crate) fn prepare_layer(
        &self, uploads: &mut gfx::ring::EncoderUploads<'_>, view: &ViewInputs,
        fogs: &HashMap<u16, (psh::fog::Fog, [f32; 4])>, samples: &LayerSamples,
    ) {
        for (&material, m) in &self.materials {
            let Some((fog, color)) = fogs.get(&material) else { continue };
            for (n, (g, _, rigid)) in m.passes.iter().enumerate() {
                let sample = samples.get(&material).and_then(|p| p.get(n)).and_then(Option::as_ref);
                let (vertex, pixel) = layer_uniforms(sample, &g.program, fog, *color);
                g.prepare_with(uploads, view, &vertex, &pixel);
                if let Some(r) = rigid {
                    if let Some(s) = sample {
                        r.prepare_sampled(uploads, view, &s.vertex(&r.program.vertex));
                    } else {
                        r.prepare(uploads, view);
                    }
                }
            }
        }
    }

    /// [`FogSet::draw`]'s selection as packets for the D3D pass already
    /// open on these attachments, and [`FogSet::prepare`]'s bytes for what
    /// they read as writes ahead of them
    /// (`gfx::external::ExternalDraws::writes`): one packet per selected
    /// part per pass in reservation order, with the pass's pipeline, its
    /// uniform group (a placement's at its slot's dynamic offset), the
    /// part's texture group, the pass's blend constant, the part's index
    /// range and the whole target as viewport and the player view as scissor. The writes are
    /// the selected materials' blocks (and the selected placements'
    /// slots) only: nothing else is read. `cache` holds the described
    /// pipelines ([`crate::gpu::GpuWorld::fog_draws`]); the pipelines
    /// are made ([`FogSet::pipelines`]). Refused when a pass, group or
    /// fog state is missing, as `draw` records nothing then.
    // wgpu compares and hashes pipelines by their stable handle identity.
    #[allow(clippy::mutable_key_type)]
    pub(crate) fn draws(
        &self,
        cache: &mut HashMap<wgpu::RenderPipeline, std::sync::Arc<gfx::compile::PendingPipeline>>,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        target_size: (u32, u32),
        scissor: gfx::ScissorRect,
        selected: &[(Owner, u8)],
        view: &ViewInputs,
        fogs: &HashMap<u16, (psh::fog::Fog, [f32; 4])>,
        samples: &LayerSamples,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        let off = gfx::state::StencilState::default();
        self.draws_stencil(cache, vertices, indices, color, depth, target_size, scissor, selected, view, fogs, &off, samples, None)
    }

    /// [`FogSet::draws`] under the device's stencil at the layer call
    /// ([`layer_stencil`]): the pipelines made with its state
    /// ([`FogSet::prepare_pipelines_stencil`]), each packet setting its
    /// reference.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::mutable_key_type)]
    pub(crate) fn draws_stencil(
        &self,
        cache: &mut HashMap<wgpu::RenderPipeline, std::sync::Arc<gfx::compile::PendingPipeline>>,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        target_size: (u32, u32),
        scissor: gfx::ScissorRect,
        selected: &[(Owner, u8)],
        view: &ViewInputs,
        fogs: &HashMap<u16, (psh::fog::Fog, [f32; 4])>,
        device_stencil: &gfx::state::StencilState,
        samples: &LayerSamples,
        mut culled: Option<LayerSlots<'_>>,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        use gfx::{BoundGroup, RecordedDraw};
        let stencil = layer_stencil(device_stencil, depth);
        let mut packets = Vec::new();
        // The materials the selection reads, and per material the
        // placements it draws.
        let mut used: BTreeMap<u16, std::collections::BTreeSet<u32>> = BTreeMap::new();
        for &(owner, part) in selected {
            let &(first_index, index_count, lightmap, material) =
                self.parts.get(&(owner, part)).ok_or("a selected fog part did not bind")?;
            if index_count == 0 {
                return Err("a selected fog part draws nothing");
            }
            let end = u64::from(first_index) + u64::from(index_count);
            if end * 4 > indices.size() {
                return Err("a selected fog part reaches past the indices");
            }
            let m = self.materials.get(&material).ok_or("a selected fog material did not bind")?;
            let groups = m.groups.get(&lightmap).ok_or("a selected fog part's textures did not bind")?;
            if !fogs.contains_key(&material) {
                return Err("a selected fog material has no resolved fog");
            }
            let slots = used.entry(material).or_default();
            let slot = match &culled {
                Some(c) => Some(*c.slots.get(&(owner, part)).ok_or("a selected layer part has no cull slot")?),
                None => None,
            };
            // Occludable while every pass's hidden fragments draw nothing:
            // a pass with a polygon offset may draw in front of the depth
            // that hides its bounds.
            let mut occludable = true;
            for ((g, state, rigid), textures) in m.passes.iter().zip(groups) {
                // The packets set the stencil reference from the render
                // state: the pass's own stencil's, else the device's.
                let render = gfx::state::RenderState { stencil: state.stencil_under(device_stencil), ..Default::default() };
                let (shader, pipeline, uniforms, offsets) = match owner {
                    Owner::Instance(n) => {
                        let r = rigid.as_ref().ok_or("a fog placement has no rigid program")?;
                        let offset = r.offset(n).ok_or("a fog placement has no slot")?;
                        let pipeline = r.pipelines.get(color, depth, &stencil, None).ok_or("a fog pipeline is not made")?;
                        slots.insert(n);
                        (FOG_RIGID_PROGRAM, pipeline, r.uniforms.clone(), vec![offset])
                    }
                    Owner::Cluster(_) => {
                        let pipeline = g.pipelines.get(color, depth, &stencil, None).ok_or("a fog pipeline is not made")?;
                        (FOG_CLUSTER_PROGRAM, pipeline, g.uniforms.clone(), Vec::new())
                    }
                };
                let key = described_pipeline(shader, color, depth, Some(state), None, stencil.clone());
                occludable &= state.depth_offset.is_none() && crate::gpu::hidden_draws_nothing(&key);
                let pipeline = cache
                    .entry(pipeline.clone())
                    .or_insert_with(|| gfx::compile::Pending::ready(Some(pipeline.clone())))
                    .clone();
                let groups = vec![BoundGroup { index: 0, group: uniforms, offsets }, BoundGroup::new(1, textures.clone())];
                let packet = match (&culled, slot) {
                    // The slot's own range of the whole index buffer.
                    (Some(c), Some(slot)) => RecordedDraw::prepared_indexed(
                        &key, pipeline, vec![(vertices.clone(), 0, vertices.size())], (indices.clone(), 0, indices.size()),
                        0, groups, &render, target_size, scissor,
                    )
                    .with_indirect(c.early.clone(), slot as u64 * crate::cull_gpu::SLOT_SIZE, 1),
                    _ => RecordedDraw::prepared_indexed(
                        &key,
                        pipeline,
                        vec![(vertices.clone(), 0, vertices.size())],
                        (indices.clone(), u64::from(first_index) * 4, u64::from(index_count) * 4),
                        index_count,
                        groups,
                        &render,
                        target_size,
                        scissor,
                    ),
                };
                packets.push(packet.with_blend_constant(state.blend_constant));
            }
            if let (Some(c), Some(slot)) = (culled.as_mut(), slot) {
                let value = if occludable { crate::cull_gpu::OCCLUDABLE } else { crate::cull_gpu::ORDERED };
                // A part in two runs, or under two values: the frustum's alone.
                let at = &mut c.start[slot];
                *at = if *at == 0 || *at == value { value } else { crate::cull_gpu::ORDERED };
            }
        }
        if packets.is_empty() {
            return Err("fog selection draws nothing");
        }
        let mut writes = Vec::new();
        for (material, slots) in &used {
            let m = &self.materials[material];
            let (fog, color) = &fogs[material];
            for (n, (g, _, rigid)) in m.passes.iter().enumerate() {
                let sample = samples.get(material).and_then(|p| p.get(n)).and_then(Option::as_ref);
                let (vertex, pixel) = layer_uniforms(sample, &g.program, fog, *color);
                writes.extend(g.prepare_writes(view, &vertex, &pixel));
                if let Some(r) = rigid {
                    let vertex = sample.map(|s| s.vertex(&r.program.vertex));
                    writes.extend(r.prepare_writes(view, slots.iter().copied(), vertex.as_ref()));
                }
            }
        }
        // Conservative: the programs' alpha tests and kill sources discard.
        Ok(gfx::external::ExternalDraws { ranges: selected.len(), packets, discards: true, writes })
    }

    /// Makes the pipelines for these target formats (before the pass).
    pub fn pipelines(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        self.prepare_pipelines(device, color, depth, PipelineUse::Direct);
    }

    pub(crate) fn prepare_pipelines(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, usage: PipelineUse,
    ) -> bool {
        self.prepare_pipelines_stencil(device, color, depth, &wgpu::StencilState::default(), usage)
    }

    /// [`FogSet::prepare_pipelines`] with `stencil` (a world layer's,
    /// [`layer_stencil`]) in every pipeline.
    pub(crate) fn prepare_pipelines_stencil(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState, usage: PipelineUse,
    ) -> bool {
        if self.ready_formats.contains(&(color, depth, stencil.clone())) { return true; }
        let stencil = stencil.clone();
        let mut ready = true;
        for m in self.materials.values_mut() {
            for (g, state, rigid) in &mut m.passes {
                ready &= g.pipelines.prepare(device, color, depth, Some(state), None, &stencil, usage);
                if let Some(r) = rigid {
                    ready &= r.pipelines.prepare(device, color, depth, Some(state), None, &stencil, usage);
                }
            }
        }
        // Only an admission check observes the frame boundary. Direct rendering
        // can use a pipeline as soon as the compiler finishes.
        if ready && matches!(usage, PipelineUse::Preflight) {
            self.ready_formats.insert((color, depth, stencil));
        }
        ready
    }

    /// Records the selected items in reservation order. A missing pass
    /// invalidates the whole recording.
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        selected: &[(Owner, u8)],
    ) -> usize {
        let stencil = wgpu::StencilState::default();
        for &(owner, part) in selected {
            let Some(&(first_index, index_count, lightmap, material)) = self.parts.get(&(owner, part)) else { return 0 };
            let Some(m) = self.materials.get(&material) else { return 0 };
            let Some(groups) = m.groups.get(&lightmap) else { return 0 };
            for ((g, state, rigid), textures) in m.passes.iter().zip(groups) {
                match owner {
                    Owner::Instance(n) => {
                        let Some(r) = rigid else { return 0 };
                        let Some(offset) = r.offset(n) else { return 0 };
                        let Some(pipeline) = r.pipelines.get(color, depth, &stencil, None) else { return 0 };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &r.uniforms, &[offset]);
                    }
                    Owner::Cluster(_) => {
                        let Some(pipeline) = g.pipelines.get(color, depth, &stencil, None) else { return 0 };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &g.uniforms, &[]);
                    }
                }
                pass.set_bind_group(1, textures, &[]);
                pass.set_blend_constant(state.blend_constant);
                pass.set_stencil_reference(u32::from(state.stencil.map_or(0, |s| s.reference)));
                pass.draw_indexed(first_index..first_index + index_count, 0, 0..1);
            }
        }
        selected.len()
    }
}

/// Synthetic scenes shared by the packet tests of this stage and the
/// water's ([`crate::water_gpu`]): no game data.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::program::{compile_pass_rigid, MODEL_ROWS};
    use crate::world::{PartInput, Placement, SectionInput, WorldBuilder};
    use shaders::psh::TextureDim;
    use shaders::vsh::encode::{program, Inst, Operand as O};
    use shaders::vsh::{MacOp, OutReg};
    use wgpu::{TextureFormat as F, TextureUsages as U};

    pub(crate) const SIZE: u32 = 32;

    /// A pass of one implementation draws it for any item; a pass of
    /// several draws the chosen one, and without a choice the layer has
    /// none to check.
    #[test]
    fn each_pass_names_the_implementation_its_draw_binds() {
        use crate::layer_pass::tests::{implementation, material};
        let imp = |n| implementation(n, Default::default(), Vec::new());
        let m = material(16, vec![vec![imp(0)], vec![imp(1), imp(2), imp(3)]], Default::default());
        let chosen = layer_implementations(&m, 16, Some(2)).unwrap();
        assert_eq!(chosen.iter().map(|i| i.index).collect::<Vec<_>>(), vec![0, 3]);
        assert_eq!(layer_implementations(&m, 16, None), None);
        assert_eq!(layer_implementations(&m, 6, Some(2)), None, "no layer-6 pass");
    }

    pub(crate) fn player_viewport(split: bool) -> [f32; 4] {
        // A guest boundary at 10 / 19 of the screen falls between physical texels.
        let request = gfx::external::ExternalPassRequest {
            rect: if split { (10, 0, 9, 19) } else { (0, 0, 19, 19) },
            surface_size: (19, 19),
            display_projection_factor: 0.75,
            z_scale: 1.0,
        };
        gfx::external::external_viewport(&request, (SIZE, SIZE))
    }

    pub(crate) fn assert_split_images(images: &[(Vec<u8>, Vec<u8>)]) {
        assert_eq!(images[0], images[1], "full-view packet and direct draws");
        assert_eq!(images[2], images[3], "split-view packet and direct draws");
        let boundary = 17; // round(32 * 10 / 19)
        let mut covered = [false; 2];
        for (i, (full, split)) in images[0].0.chunks_exact(4).zip(images[2].0.chunks_exact(4)).enumerate() {
            let in_view = i % SIZE as usize >= boundary;
            covered[usize::from(in_view)] |= full.iter().any(|&byte| byte != 0);
            assert_eq!(split, if in_view { full } else { &[0; 4] }, "texel {i}");
        }
        assert_eq!(covered, [true, true], "geometry crosses the split");
    }

    pub(crate) fn look(stage0: Stage) -> PassLook {
        PassLook {
            pass: map::TagId(0xe000_0001),
            implementation: 0,
            vertex_shader: map::TagId(0xe000_0002),
            textures: [stage0, Stage::Empty, Stage::Empty, Stage::Empty],
            stage_states: Vec::new(),
            pixel_shader: shaders::PixelShaderDef::default().to_bytes(),
            vertex_constants: Vec::new(),
            vertex_defaults: Vec::new(),
            per_draw_inputs: false,
        }
    }

    /// `dp4 oPos, v0, c[0..3]; mov oT0, v1`.
    pub(super) fn cluster_code() -> Vec<u8> {
        let row = |c, mask| Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(false, OutReg::Pos, mask);
        program(&[
            row(0, 8),
            row(1, 4),
            row(2, 2),
            row(3, 1),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
        ])
    }

    /// `r10 = v0 · c[170] + c[171]`, `dph r0.xyz, r10, c[50..52]`, then
    /// `dph oPos, r0, c[0..3]`, `mov oT0, v1`: the rigid classification.
    fn rigid_code() -> Vec<u8> {
        let mut code = vec![
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(10, 0xf),
            Inst::mac(MacOp::Add, Some(O::r(10)), None, Some(O::c(171))).to_temp(10, 0xf),
        ];
        for k in 0..3u8 {
            code.push(Inst::mac(MacOp::Dph, Some(O::r(10)), Some(O::c(MODEL_ROWS as u8 + k)), None).to_temp(0, 8 >> k));
        }
        for k in 0..4u8 {
            code.push(Inst::mac(MacOp::Dph, Some(O::r(0)), Some(O::c(k)), None).to_out(false, OutReg::Pos, 8 >> k));
        }
        code.push(Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf));
        program(&code)
    }

    pub(crate) fn state() -> PassState {
        PassState {
            blend: None,
            blend_constant: wgpu::Color { r: 0.25, g: 0.5, b: 0.75, a: 1.0 },
            color_writes: wgpu::ColorWrites::ALL,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            primitive: wgpu::PrimitiveState::default(),
            alpha_test: None,
            stencil: None,
            depth_offset: None,
        }
    }

    /// A pass sampling `stage0` at stage 0 with both classifications.
    pub(crate) fn pass(stage0: Stage) -> (FogPass, FogProgram) {
        let look = look(stage0);
        let mut program = compile_pass(&look, &cluster_code(), None).expect("compiles");
        if stage0 != Stage::Empty {
            program.dims[0] = Some(TextureDim::D2);
            program.stages[0] = stage0;
        }
        let rigid = compile_pass_rigid(&look, None, &rigid_code()).expect("rigid compiles");
        let state = state();
        (
            FogPass { look, state, fog: FogWords::default(), planar: None },
            FogProgram { program, state, words: FogWords::default(), planar: None, rigid: Some(rigid) },
        )
    }

    #[test]
    fn a_layer_sample_reaches_both_uniform_blocks_and_keeps_the_placement_rows() {
        let (_, p) = pass(Stage::Empty);
        let mut imp = crate::engine_states::tests::implementation(Vec::new(), 0);
        let reg = Register { index: 21, value: [3.0, 5.0, 7.0, 9.0], written: 0b1010, origin: tags::material::Origin::Instance };
        imp.vertex_constants.push(reg);
        imp.vertex_defaults.push(Register { index: 7, ..reg });
        let mut def = PixelShaderDef::default();
        def.constant0[2] = 0x8060_4020;
        def.final_constant1 = 0x2244_6688;
        imp.pixel_shader = def.to_bytes();
        let sample = LayerConstants::from_implementation(&imp);
        let color = [0.3, 0.5, 0.7, 1.0];
        let (v, psh) = layer_uniforms(Some(&sample), &p.program, &psh::fog::Fog::OFF, color);
        assert_eq!(v.constants[21], [0.0, 5.0, 0.0, 9.0]);
        assert_eq!(v.defaults[7], [0.0, 5.0, 0.0, 9.0]);
        assert_eq!(psh.c0[2], shaders::psh::argb(def.constant0[2]));
        assert_eq!(psh.c1[8], shaders::psh::argb(def.final_constant1));
        assert_eq!(psh.fog_color, color);
        let rigid = sample.vertex(&p.rigid.as_ref().unwrap().vertex);
        let rows = [[2.0; 4], [3.0; 4], [4.0; 4]];
        let placed = crate::program::rigid_vertex(&rigid, &view(), &rows);
        assert_eq!(placed.constants[21], v.constants[21]);
        assert_eq!(placed.constants[MODEL_ROWS..MODEL_ROWS + 3], rows);
        assert_eq!(p.program.vertex.constants[21], [0.0; 4]);
    }

    /// A cluster (material 0) at x = 10 and a placement of the same
    /// material at x = 12 with identity rows, both facing the camera.
    pub(crate) fn world() -> World {
        let wall = |x: f32| SectionInput {
            positions: vec![[x, 2.0, -2.0], [x, -2.0, -2.0], [x, 2.0, 2.0], [x, -2.0, 2.0]],
            uvs: vec![[0.5; 2]; 4],
            normals: vec![[-1.0, 0.0, 0.0]; 4],
            parts: vec![PartInput { material: 0, transparent: false, triangles: vec![[0, 1, 2], [1, 3, 2]] }],
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &wall(10.0), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(5), &wall(12.0), &Placement::IDENTITY, None);
        b.place_rows(5, [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
        b.finish()
    }

    pub(crate) type Parts = Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>;

    pub(crate) fn parts(world: &World) -> Parts {
        world
            .draws
            .iter()
            .map(|d| ((d.owner, d.part), (d.first_index, d.index_count, d.lightmap, d.material)))
            .collect()
    }

    pub(crate) fn selected() -> Vec<(Owner, u8)> {
        vec![(Owner::Cluster(0), 0), (Owner::Instance(5), 0)]
    }

    pub(crate) fn view() -> ViewInputs {
        let mut constants = [[0.0; 4]; crate::program::VIEW_CONSTANTS];
        constants[..4].copy_from_slice(&[[0.0, -2.0, 0.0, 16.0], [0.0, 0.0, -2.0, 16.0], [0.5, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]]);
        ViewInputs { constants, guest_size: (SIZE, SIZE), size: (SIZE, SIZE), z_scale: 16_777_215.0, w_clip: None }
    }

    pub(crate) fn fogs() -> HashMap<u16, (psh::fog::Fog, [f32; 4])> {
        [(0, (psh::fog::Fog::OFF, [0.2, 0.4, 0.6, 1.0]))].into()
    }

    /// Renders into fresh targets cleared to transparent and depth 1,
    /// `draw` recording after the clear and the arena's front writes
    /// submitted first; returns the colour and depth texels.
    pub(crate) fn render(
        ctx: &gfx::GfxContext,
        draw: impl FnOnce(&mut wgpu::CommandEncoder, &gfx::ring::SubmissionUploads, &gfx::RenderTarget<'_>),
    ) -> (Vec<u8>, Vec<u8>) {
        let texture = |format| ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage: U::RENDER_ATTACHMENT | U::COPY_SRC, view_formats: &[],
        });
        let (color, depth) = (texture(F::Rgba8Unorm), texture(F::Depth32Float));
        let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
        let target = gfx::RenderTarget {
            color: &cv, color_format: F::Rgba8Unorm, depth: Some(&dv), depth_format: Some(F::Depth32Float),
            size: (SIZE, SIZE), samples: 1,
        };
        let arena = gfx::ring::UploadPool::default().begin();
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &cv, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &dv, stencil_ops: None,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            }),
            timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        }));
        draw(&mut enc, &arena, &target);
        let front = arena.upload_commands(&ctx.queue);
        ctx.queue.submit(front.into_iter().chain([enc.finish()]));
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let read = |t, aspect| gfx::readback::read_texture(ctx, t, aspect, 4, (SIZE, SIZE));
        (read(&color, wgpu::TextureAspect::All), read(&depth, wgpu::TextureAspect::DepthOnly))
    }

    /// The fog's packets with their uniforms written at the front of the
    /// submission draw what `encode_fog` draws on its own pass after
    /// `prepare_fog`'s in-order writes, a cluster and a placement (its
    /// slot's dynamic offset) alike, and write the bytes `prepare` does
    /// for what is drawn.
    #[test]
    fn fog_packets_draw_what_the_fog_pass_draws_with_the_uniforms_written_up_front() {
        use crate::gpu::GpuWorld;
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let world = world();
        let parts = parts(&world);
        let mut gpu = GpuWorld::new(&ctx.device, &ctx.queue, world, &mut |_| None, &mut |_| None);
        let (_, program) = pass(Stage::Bitmap { bitmap: map::TagId(7), image: 1 });
        let set = FogSet::new(
            &ctx.device, &ctx.queue, gpu.world(), parts, [(0, vec![program])].into(),
            &mut |_| Some(Image::solid([200, 100, 50, 255])), &mut |_| None,
        );
        assert_eq!(set.len(), 1);
        gpu.set_fog(set);
        let (selected, view, fogs) = (selected(), view(), fogs());
        let mut images = Vec::new();
        // Packets first: the uniforms they read are only those written ahead.
        for (packets, split) in [(true, false), (false, false), (true, true), (false, true)] {
            images.push(render(&ctx, |enc, arena, target| {
                let draws = gpu.fog_draws(
                    &ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), player_viewport(split),
                    &selected, &view, &fogs, &LayerSamples::new(),
                ).unwrap();
                assert_eq!((draws.ranges, draws.packets.len()), (2, 2), "one pass each, cluster and placement");
                assert_eq!(draws.writes.len(), 2 + 2, "the pass's two blocks, the placement's view prefix and tail");
                if packets {
                    for (buffer, offset, bytes) in &draws.writes {
                        arena.write_front(&ctx.device, &ctx.queue, buffer, *offset, bytes);
                    }
                    gfx::Renderer::encode_pass(&ctx, enc, target, gfx::Clear::NONE, &draws.packets, None).unwrap();
                } else {
                    let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, arena, encoder: enc };
                    gpu.prepare_fog(&mut uploads, &view, &fogs, &LayerSamples::new());
                    assert!(draws.writes.iter().all(|(buffer, ..)| arena.wrote_in_order(buffer)));
                    let recorded = gpu.encode_fog(
                        &ctx.device, enc, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float, (SIZE, SIZE), player_viewport(split), &selected,
                    );
                    assert_eq!(recorded, 2);
                }
            }));
        }
        assert!(images[0].0.iter().any(|&b| b != 0), "the fog drew something");
        assert_split_images(&images);
    }

    /// A world layer's draws take the device's stencil at the layer call:
    /// off as it is, the scene's layer-1 mode 1 (`ALWAYS`, `REPLACE` on
    /// pass, masks `0xff`) as its pipeline state, and only a state that is
    /// on needs the depth's stencil aspect.
    #[test]
    fn a_layers_stencil_is_the_devices_and_needs_a_stencil_aspect_only_when_on() {
        use gfx::state::{CompareFunc, StencilOp, StencilState};
        let off = StencilState::default();
        assert_eq!(layer_stencil(&off, F::Depth32Float), wgpu::StencilState::default());
        assert!(!stencil_writes(&off), "the test off writes nothing");
        let mode1 = crate::object_state::structure_stencil(0);
        let state = layer_stencil(&mode1, F::Depth24PlusStencil8);
        assert_eq!((state.read_mask, state.write_mask), (0xff, 0xff));
        for face in [state.front, state.back] {
            assert_eq!(face.compare, wgpu::CompareFunction::Always);
            assert_eq!((face.fail_op, face.depth_fail_op, face.pass_op), (
                wgpu::StencilOperation::Keep, wgpu::StencilOperation::Keep, wgpu::StencilOperation::Replace,
            ));
        }
        assert!(stencil_writes(&mode1), "mode 1 writes its reference on pass");
        assert_eq!(layer_stencil(&mode1, F::Depth32Float), wgpu::StencilState::default(), "no stencil aspect: the test off, as on D3D");
        let test_only = StencilState { enable: true, func: CompareFunc::Equal, reference: 3, ..off };
        assert_eq!(layer_stencil(&test_only, F::Depth24PlusStencil8).front.compare, wgpu::CompareFunction::Equal);
        assert!(!stencil_writes(&test_only), "a test that keeps every value writes nothing");
        assert!(!stencil_writes(&StencilState { write_mask: 0, pass: StencilOp::Replace, ..test_only }), "no write mask");
    }

    /// A layer's packets under a stencil that writes draw the colour they
    /// draw with the stencil off and leave the reference in the stencil
    /// of what they cover, and nothing elsewhere; without a stencil
    /// aspect they are refused.
    #[test]
    fn layer_packets_under_the_devices_stencil_write_its_reference_where_they_draw() {
        use crate::gpu::GpuWorld;
        use gfx::state::StencilState;
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let world = world();
        let parts = parts(&world);
        let mut gpu = GpuWorld::new(&ctx.device, &ctx.queue, world, &mut |_| None, &mut |_| None);
        let (_, program) = pass(Stage::Bitmap { bitmap: map::TagId(7), image: 1 });
        let set = FogSet::new(
            &ctx.device, &ctx.queue, gpu.world(), parts, [(0, vec![program])].into(),
            &mut |_| Some(Image::solid([200, 100, 50, 255])), &mut |_| None,
        );
        gpu.set_layer(1, set);
        let (selected, view, fogs) = (selected(), view(), fogs());
        let marking = StencilState { reference: 0x80, ..crate::object_state::structure_stencil(0) };
        assert!(
            gpu.layer_draws(1, &ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), &selected, &view, &fogs, &marking, &LayerSamples::new()).is_ok(),
            "no stencil aspect: drawn with the test off, as on D3D"
        );
        let depth_format = F::Depth24PlusStencil8;
        let mut out = Vec::new();
        for stencil in [StencilState::default(), marking] {
            let texture = |format| ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: None, size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
                usage: U::RENDER_ATTACHMENT | U::COPY_SRC, view_formats: &[],
            });
            let (color, depth) = (texture(F::Rgba8Unorm), texture(depth_format));
            let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
            let target = gfx::RenderTarget {
                color: &cv, color_format: F::Rgba8Unorm, depth: Some(&dv), depth_format: Some(depth_format),
                size: (SIZE, SIZE), samples: 1,
            };
            let draws = gpu
                .layer_draws(1, &ctx.device, F::Rgba8Unorm, depth_format, (SIZE, SIZE), &selected, &view, &fogs, &stencil, &LayerSamples::new())
                .expect("the layer draws under the device's stencil");
            assert_eq!((draws.ranges, draws.packets.len()), (2, 2), "one pass each, cluster and placement");
            let arena = gfx::ring::UploadPool::default().begin();
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            for (buffer, offset, bytes) in &draws.writes {
                arena.write_front(&ctx.device, &ctx.queue, buffer, *offset, bytes);
            }
            gfx::Renderer::encode_pass(&ctx, &mut enc, &target, gfx::Clear::all([0.0; 4]), &draws.packets, None).unwrap();
            let front = arena.upload_commands(&ctx.queue);
            ctx.queue.submit(front.into_iter().chain([enc.finish()]));
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let colour = gfx::readback::read_texture(&ctx, &color, wgpu::TextureAspect::All, 4, (SIZE, SIZE));
            let marks = gfx::readback::read_texture(&ctx, &depth, wgpu::TextureAspect::StencilOnly, 1, (SIZE, SIZE));
            out.push((colour, marks));
        }
        let [(plain, unmarked), (colour, marks)] = <[_; 2]>::try_from(out).unwrap();
        assert!(plain.iter().any(|&b| b != 0), "the layer drew something");
        assert_eq!(plain, colour, "the stencil changes no colour");
        assert!(unmarked.iter().all(|&s| s == 0), "the test off writes no stencil");
        for (texel, &mark) in marks.iter().enumerate() {
            let drawn = colour[texel * 4..texel * 4 + 4] != [0; 4];
            assert_eq!(mark, if drawn { 0x80 } else { 0 }, "texel {texel}: the reference where drawn, zero elsewhere");
        }
    }

    #[test]
    fn fog_packets_refuse_a_selection_the_pass_would_not_record() {
        use crate::gpu::GpuWorld;
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let world = world();
        let parts = parts(&world);
        let mut gpu = GpuWorld::new(&ctx.device, &ctx.queue, world, &mut |_| None, &mut |_| None);
        let (_, program) = pass(Stage::Bitmap { bitmap: map::TagId(7), image: 1 });
        let set = FogSet::new(
            &ctx.device, &ctx.queue, gpu.world(), parts, [(0, vec![program])].into(),
            &mut |_| Some(Image::solid([1; 4])), &mut |_| None,
        );
        gpu.set_fog(set);
        let (view, fogs) = (view(), fogs());
        let draws = |gpu: &mut GpuWorld, selected: &[(Owner, u8)], fogs: &HashMap<u16, (psh::fog::Fog, [f32; 4])>| {
            gpu.fog_draws(
                &ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), player_viewport(false),
                selected, &view, fogs, &LayerSamples::new(),
            )
        };
        assert!(draws(&mut gpu, &selected(), &fogs).is_ok());
        assert!(draws(&mut gpu, &[(Owner::Cluster(9), 0)], &fogs).is_err(), "a part the set does not hold");
        assert!(draws(&mut gpu, &[], &fogs).is_err(), "nothing to draw");
        assert!(draws(&mut gpu, &selected(), &HashMap::new()).is_err(), "a material with no resolved fog");
    }
}

/// Which implementation a layer's pass draws and what cull a pass that
/// leaves `CULLMODE` unset takes: synthetic materials, no game data.
#[cfg(test)]
mod selection_tests {
    use super::*;
    use crate::engine_states::tests::implementation;
    use map::TagId;
    use tags::material::{EngineInputs, ResolvedImplementation, ResolvedLayer, ResolvedPass};

    const ALPHA_BLEND_ENABLE: u32 = 59;
    const Z_WRITE_ENABLE: u32 = 64;
    const FRONT_FACE: u32 = 146;
    const CULL_MODE: u32 = 147;
    const CW: u32 = 0x900;
    const CCW: u32 = 0x901;

    fn state(canonical: u32, value: u32) -> RenderStateValue {
        let table = RenderStateTable::for_build(HALO2_BUILD);
        RenderStateValue { state: table.canonical_to_guest(canonical).expect("a 5849 slot") as u8, value }
    }

    /// Implementation `index` with its own final combiner, blending and
    /// depth write off, and `extra` states after them.
    fn imp(index: usize, extra: &[RenderStateValue]) -> ResolvedImplementation {
        let mut i = implementation(vec![state(ALPHA_BLEND_ENABLE, 0), state(Z_WRITE_ENABLE, 0)], 0);
        i.index = index;
        i.pixel_shader = PixelShaderDef::default().with_default_final_combiner(false, false).to_bytes();
        i.render_states.extend_from_slice(extra);
        i
    }

    /// A material whose `layer` has one pass per entry, each with the
    /// implementations given.
    fn material(layer: usize, passes: Vec<Vec<ResolvedImplementation>>) -> Material {
        Material {
            shader: TagId(0xe000_0001),
            template: TagId(0xe000_0002),
            inputs: EngineInputs::default(),
            layers: vec![ResolvedLayer {
                lod: 0,
                layer,
                passes: passes
                    .into_iter()
                    .enumerate()
                    .map(|(index, implementations)| ResolvedPass { index, pass: TagId(0xe000_0200 + index as u32), implementations })
                    .collect(),
            }],
        }
    }

    fn four(extra: &[RenderStateValue]) -> Vec<ResolvedImplementation> {
        (0..4).map(|i| imp(i, extra)).collect()
    }

    const FRONT: Option<wgpu::Face> = Some(wgpu::Face::Front);

    #[test]
    fn planar_pass_samples_the_current_views_combiner_constants() {
        use scene_extract::layout::layer_inputs::{FADE, MATERIAL, ONE};
        use scene_extract::testing::Mem;
        use tags::material::{VectorBinding, VectorTarget};

        let mut i = imp(0, &[state(CULL_MODE, CW)]);
        i.per_draw_inputs = true;
        i.pixel_constants_from_engine = true;
        i.input_reads.scalars = (1 << 35) | (1 << 36);
        i.input_reads.vectors = 1 << 31;
        let binding = |source, slot, mask| VectorBinding {
            source, target: VectorTarget::Pixel { c1: false, slot, mask, half: false, transform: 0 },
        };
        i.vector_bindings.push(binding(31, 0, 0x00ff_ffff));
        i.scalar_bindings.extend([binding(35, 0, 0xff00_0000), binding(36, 1, 0xff00_0000)]);
        let mut unsupported = i.clone();
        unsupported.input_reads.ints = 1;
        assert!(layer_passes(&material(16, vec![vec![unsupported]]), 16).is_err());
        let mat = material(16, vec![vec![i]]);
        let pass = layer_passes(&mat, 16).unwrap().remove(0);
        let program = FogProgram::compile(&pass, &super::tests::cluster_code()).unwrap();
        let mut mem = Mem::default();
        mem.f(ONE, 1.0);
        mem.f(FADE, 0.25);
        mem.f(MATERIAL + 0x90, 0.5);
        for (n, value) in [1.0, 0.5, 0.0].into_iter().enumerate() {
            mem.f(MATERIAL + 0x84 + 4 * n as u32, value);
        }
        let sample = |mem: &Mem| program.planar.as_ref().unwrap()
            .sample(&program.program.pixel, &PlanarInputs::read(mem).unwrap());
        assert_eq!(sample(&mem).c0[0], psh::argb(0x60ff_8000));
        assert_eq!(sample(&mem).c0[1][3], psh::argb(0x2000_0000)[3]);
        mem.f(FADE, 1.0);
        assert_eq!(sample(&mem).c0[0], psh::argb(0x00ff_8000));
        mem.0.remove(&ONE);
        assert!(PlanarInputs::read(&mem).is_none());
    }

    #[test]
    fn water_binds_both_images_of_the_supplied_rasterizer_bitmap() {
        use tags::material::{StageTexture, TextureSource};
        let mut pass = imp(0, &[state(CULL_MODE, CW)]);
        for (slot, id) in [14, 15].into_iter().enumerate() {
            pass.textures[slot] = Some(StageTexture { source: TextureSource::Engine(id), size: None, stage_size: None });
        }
        let water = material(6, vec![vec![pass]]);
        let bitmap = TagId(42);
        let passes = crate::water_gpu::water_passes(&water, Some(bitmap)).unwrap();
        assert_eq!(passes[0].look.textures[0], Stage::Bitmap { bitmap, image: 0 });
        assert_eq!(passes[0].look.textures[1], Stage::Bitmap { bitmap, image: 1 });
        assert!(crate::water_gpu::water_passes(&water, None).is_err());
    }

    #[test]
    fn a_layer_pass_draws_its_polygon_offset_in_its_fragments() {
        use gfx::state::RenderStateType as R;
        let table = RenderStateTable::for_build(HALO2_BUILD);
        let slot = |t: R| {
            (0..=255u8).find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t)).expect("a 5849 slot")
        };
        let word = |t: R, value: u32| RenderStateValue { state: slot(t), value };
        let pass = imp(0, &[
            state(CULL_MODE, CW),
            word(R::SolidOffsetEnable, 1),
            word(R::PolygonOffsetZOffset, (-8.0f32).to_bits()),
            word(R::PolygonOffsetZSlopeScale, 1.0f32.to_bits()),
        ]);
        let pass = layer_passes(&material(16, vec![vec![pass]]), 16).unwrap().remove(0);
        assert_eq!(pass.state.depth_offset, Some(crate::pass_state::DepthOffset { units: -8.0, slope: 1.0 }));
        let program = FogProgram::compile(&pass, &super::tests::cluster_code()).unwrap();
        assert_eq!((program.program.pixel.depth[1], program.program.pixel.depth[2]), (-8.0, 1.0));
        let plain = layer_passes(&material(16, vec![vec![imp(0, &[state(CULL_MODE, CW)])]]), 16).unwrap().remove(0);
        let plain = FogProgram::compile(&plain, &super::tests::cluster_code()).unwrap();
        assert_eq!((plain.program.pixel.depth[1], plain.program.pixel.depth[2]), (0.0, 0.0));
    }

    #[test]
    fn fog_ignores_an_unbound_input_and_retains_dynamic_bindings() {
        use tags::material::{VectorBinding, VectorTarget};

        let mut pass = imp(0, &[state(CULL_MODE, CW)]);
        pass.per_draw_inputs = true;
        pass.input_reads.vectors = 1;
        let resolve = |i| layer_passes(&material(16, vec![vec![i]]), 16);
        let static_pass = resolve(pass.clone()).unwrap().remove(0);
        assert!(!static_pass.look.per_draw_inputs);
        assert!(static_pass.planar.is_none());

        pass.input_reads.vectors = 1 << 31;
        pass.pixel_constants_from_engine = true;
        pass.vector_bindings.push(VectorBinding {
            source: 31,
            target: VectorTarget::Pixel { c1: false, slot: 0, mask: 0x00ff_ffff, half: false, transform: 0 },
        });
        assert!(resolve(pass.clone()).unwrap()[0].planar.is_some());
        pass.variant = true;
        assert!(resolve(pass).is_err());
    }

    #[test]
    fn a_pass_with_four_implementations_draws_the_one_selected() {
        let m = material(16, vec![four(&[state(CULL_MODE, CW)])]);
        assert_eq!(layer_passes(&m, 16).unwrap_err(), FogRefusal::Implementations { pass: 0, count: 4 });
        for wanted in 0..4 {
            let sel = Selection { implementation: Some(wanted), layer_cull: false, rasterizer: None };
            let passes = layer_passes_selected(&m, 16, sel).expect("the selected implementation fits");
            assert_eq!(passes.len(), 1);
            assert_eq!(passes[0].look.implementation, wanted, "the look is the selected implementation's");
        }
        let past = Selection { implementation: Some(4), layer_cull: false, rasterizer: None };
        assert_eq!(layer_passes_selected(&m, 16, past).unwrap_err(), FogRefusal::Implementations { pass: 0, count: 4 });
        assert!(layer_has_choice(&m, 16));
        assert!(!layer_has_choice(&m, 7), "another layer has no pass");
    }

    #[test]
    fn a_pass_with_one_implementation_ignores_the_selection() {
        let m = material(16, vec![vec![imp(0, &[state(CULL_MODE, CW)])], four(&[state(CULL_MODE, CW)])]);
        let sel = Selection { implementation: Some(2), layer_cull: false, rasterizer: None };
        let passes = layer_passes_selected(&m, 16, sel).unwrap();
        let chosen: Vec<_> = passes.iter().map(|p| p.look.implementation).collect();
        assert_eq!(chosen, vec![0, 2], "the single takes 0, the other the selection");
        let only = material(16, vec![vec![imp(0, &[state(CULL_MODE, CW)])]]);
        assert!(layer_passes(&only, 16).is_ok());
        assert!(!layer_has_choice(&only, 16));
    }

    #[test]
    fn an_unset_cull_takes_the_layers_start_only_when_the_layer_allows_it() {
        let m = material(1, vec![vec![imp(0, &[])]]);
        assert_eq!(layer_passes(&m, 1).unwrap_err(), FogRefusal::InheritsCull { pass: 0 });
        let refused = Selection { implementation: None, layer_cull: false, rasterizer: None };
        assert_eq!(layer_passes_selected(&m, 1, refused).unwrap_err(), FogRefusal::InheritsCull { pass: 0 });
        let allowed = Selection { implementation: None, layer_cull: true, rasterizer: None };
        let passes = layer_passes_selected(&m, 1, allowed).unwrap();
        // `CULLMODE` CW with `FRONTFACE` CW culls the front face of the
        // primitive state (`gfx::translate::primitive_state`), where
        // `RenderState`'s own default (CCW) culls the back.
        assert_eq!(passes[0].state.primitive.cull_mode, FRONT);
        assert_eq!(passes[0].state.primitive.front_face, wgpu::FrontFace::Cw);
        // A pass that sets its own keeps it.
        let own = material(1, vec![vec![imp(0, &[state(CULL_MODE, 0)])]]);
        let kept = layer_passes_selected(&own, 1, allowed).unwrap();
        assert_eq!(kept[0].state.primitive.cull_mode, None);
        assert!(layer_passes(&own, 1).is_ok(), "an explicit cull needs no layer rule");
    }

    #[test]
    fn the_layers_start_is_the_layer_setups() {
        assert_eq!(layer_cull(), (gfx::state::CullMode::Cw, gfx::state::FrontFace::Cw));
        assert_eq!(layer_cull().0.xbox_value(), CW, "the `0x900` the setup passes both calls");
        assert_eq!(layer_cull().1.xbox_value(), CW);
        assert_eq!(layer_cull_primitive(), (FRONT, wgpu::FrontFace::Cw));
    }

    #[test]
    fn a_layer_sets_another_cull_when_any_implementation_does() {
        let sets = |extra: &[RenderStateValue]| layer_sets_other_cull(&material(16, vec![four(extra)]), 16);
        assert!(!sets(&[]), "nothing set");
        assert!(!sets(&[state(CULL_MODE, CW), state(FRONT_FACE, CW)]), "the start itself");
        assert!(sets(&[state(CULL_MODE, CCW)]));
        assert!(sets(&[state(CULL_MODE, 0)]), "no cull");
        assert!(sets(&[state(FRONT_FACE, CCW)]));
        assert!(sets(&[state(CULL_MODE, 5)]), "a value gfx does not decode counts");
        // One implementation among the pass's is enough, and another
        // layer's states do not count.
        let mut passes = four(&[]);
        passes[3] = imp(3, &[state(CULL_MODE, CCW)]);
        let m = material(16, vec![passes]);
        assert!(layer_sets_other_cull(&m, 16));
        assert!(!layer_sets_other_cull(&m, 7));
    }
}
