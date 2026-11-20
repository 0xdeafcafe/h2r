//! Stage 4b's GPU side: the world's layer-16 passes, each compiled with
//! its own programs ([`crate::program::compile_pass`]) and drawn with its
//! own blend, depth and cull state ([`crate::pass_state`]), over the
//! loaded depth (docs/graphics/native-renderer.md, "Stage 4b: planar
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

use std::collections::{BTreeMap, HashMap};

use crate::gpu::Image;
use crate::look::{implementation_look, PassLook, Refusal, Stage};
use crate::pass_state::{pass_state, PassState, StateRefusal};
use crate::program::{
    compile_pass, described_pipeline, world_constants_read, Gpu, MaterialProgram, Rigid, RigidProgram, ViewInputs,
};
use crate::world::{LightmapKey, ModelRows, Owner, World};
use gfx::state::RenderStateType;
use gfx::xdk::{HALO2_BUILD, RenderStateTable};
use shaders::psh::{self, PixelShaderDef, TexMode};
use tags::material::Material;
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
/// unit with `d3d::nv2a::d3d_fog` (that derivation is `d3d`'s; this
/// crate does not depend on it).
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
    /// through `d3d::Device::render_state` at the layer-16 call (`None`
    /// a slot that does not read; such a view stays on the D3D path).
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
}

/// Why a material's layer stays on the D3D path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FogRefusal {
    /// Its template draws nothing in the layer at the first level of
    /// detail.
    NoPass { layer: usize },
    /// Pass `pass` has more than one implementation (S: which one the
    /// engine binds for a fog item is not traced).
    Implementations { pass: usize, count: usize },
    /// Pass `pass`'s implementation does not fit ([`Refusal`]).
    Look { pass: usize, refusal: Refusal },
    /// Pass `pass`'s render states do not ([`StateRefusal`], as
    /// `pass_state` decodes them but for its `Fog` refusal: the fog unit
    /// is resolved per view here, [`FogWords`], not a refusal).
    State { pass: usize, refusal: StateRefusal },
    /// Pass `pass` does not set `CULLMODE`: layer 16 inherits whatever
    /// the previous draw left (a later slice can take the device's cull
    /// at the call, as the world's layer 3 does; first slice refused).
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

/// Every pass of `material`'s `layer` (first level of detail), with its
/// look and state, in the order the layer draws them, or why not. All or
/// nothing per material: an item is drawn natively only when every pass
/// is. The compilation itself is the glue's (it owns the `vrtx` tag's
/// microcode, [`compile_pass`]). The look refuses engine-texture stages
/// ([`Refusal::EngineTexture`]); [`supplied_layer_passes`] is the same
/// with the per-view supplied ones kept.
pub fn layer_passes(material: &Material, layer: usize) -> Result<Vec<FogPass>, FogRefusal> {
    passes_of(material, layer, false)
}

/// [`layer_passes`] with the engine-texture stages a per-view supply
/// resolves kept as [`Stage::Engine`]: the view's own target (id 5) and
/// the render targets (`crate::screen_look::engine_texture`'s `Target`s),
/// what the water edge blend binds. An id nothing supplies refuses the
/// material ([`FogRefusal::Unsupplied`]).
pub fn supplied_layer_passes(material: &Material, layer: usize) -> Result<Vec<FogPass>, FogRefusal> {
    passes_of(material, layer, true)
}

fn passes_of(material: &Material, layer: usize, supplied: bool) -> Result<Vec<FogPass>, FogRefusal> {
    let layer = material
        .layers
        .iter()
        .find(|l| l.lod == 0 && l.layer == layer)
        .filter(|l| !l.passes.is_empty())
        .ok_or(FogRefusal::NoPass { layer })?;
    let mut out = Vec::with_capacity(layer.passes.len());
    for (n, pass) in layer.passes.iter().enumerate() {
        let [imp] = pass.implementations.as_slice() else {
            return Err(FogRefusal::Implementations { pass: n, count: pass.implementations.len() });
        };
        let look = if supplied {
            // `implementation_look` with the engine stages kept when a
            // per-view supply resolves them.
            let mut textures = [Stage::Empty; 4];
            for (n_stage, (stage, t)) in textures.iter_mut().zip(&imp.textures).enumerate() {
                *stage = match t.map(|t| t.source) {
                    None | Some(tags::material::TextureSource::None) => Stage::Empty,
                    Some(tags::material::TextureSource::Bitmap { bitmap, image, .. }) => Stage::Bitmap { bitmap, image },
                    Some(tags::material::TextureSource::Lightmap) => Stage::Lightmap,
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
            implementation_look(pass.pass, imp).map_err(|refusal| FogRefusal::Look { pass: n, refusal })?
        };
        if !imp.animated.is_empty() {
            return Err(FogRefusal::Look { pass: n, refusal: Refusal::Animated });
        }
        if let Some(r) = imp.vertex_constants.iter().find(|r| r.origin == tags::material::Origin::Engine) {
            return Err(FogRefusal::Look { pass: n, refusal: Refusal::EngineVertexConstant(r.index) });
        }
        if !imp.engine_render_states.is_empty() {
            return Err(FogRefusal::Look { pass: n, refusal: Refusal::EngineRenderStates });
        }
        if !imp.engine_stage_states.is_empty() {
            return Err(FogRefusal::Look { pass: n, refusal: Refusal::EngineStageStates });
        }
        match fog_parity(&look) {
            Ok(()) => {}
            Err(FogParity::NoFinalCombiner) => return Err(FogRefusal::NoFinalCombiner { pass: n }),
            Err(FogParity::DeviceState { stage }) => return Err(FogRefusal::DeviceState { pass: n, stage }),
        }
        // `pass_state`'s rules but its `Fog` refusal: a fog pass may turn
        // the fog on itself, and the fog unit is resolved per view
        // ([`FogWords`]). The rest of the pipeline state must be the
        // implementation's own (a pass that leaves blend or depth to the
        // previous draw stays on the D3D path, as the transparents').
        let state = state_without_fog(&imp.render_states).map_err(|refusal| FogRefusal::State { pass: n, refusal })?;
        if !sets_cull(&imp.render_states) {
            return Err(FogRefusal::InheritsCull { pass: n });
        }
        out.push(FogPass { look, state, fog: FogWords::from_states(&imp.render_states) });
    }
    Ok(out)
}

/// [`pass_state`] over the implementation's states without the words that
/// set the fog (they are [`FogWords`]'s, not the pipeline's): the same
/// decode with the `Fog` refusal not taken.
fn state_without_fog(states: &[RenderStateValue]) -> Result<PassState, StateRefusal> {
    let without: Vec<RenderStateValue> = states
        .iter()
        .copied()
        .filter(|s| !FOG_WORD_SLOTS.contains(&u32::from(s.state)))
        .collect();
    pass_state(&without)
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

/// One compiled layer-16 pass of a material.
#[derive(Clone, Debug)]
pub struct FogProgram {
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
        let program = compile_pass(&pass.look, microcode, pass.state.alpha_test)?;
        shaders::vsh::Program::parse(microcode).map_err(|e| format!("vertex program: {e:?}"))?;
        let rigid = rigid_microcode
            .map(|code| crate::program::compile_pass_rigid(&pass.look, pass.state.alpha_test, code))
            .transpose()?;
        Ok(FogProgram { program, state: pass.state, words: pass.fog, rigid })
    }
}

struct FogMaterial {
    /// The passes in draw order, a placement's draws with the pass's
    /// rigid program when it compiled one.
    passes: Vec<(Gpu, PassState, Option<Rigid>)>,
    /// The fog words the passes set themselves (they share one, the
    /// layer's; a material whose passes disagree is not drawn — the
    /// load's `fog_passes` is all or nothing and takes one `FogWords`
    /// per pass, so the glue resolves them per material and refuses a
    /// disagreement).
    words: FogWords,
    /// Per lightmap page the material's parts use: one texture group per
    /// pass.
    groups: HashMap<Option<LightmapKey>, Vec<wgpu::BindGroup>>,
}

/// The fog materials the native pass draws, and each part's index range
/// (the glue's parts list, in the world's draw order).
pub struct FogSet {
    materials: HashMap<u16, FogMaterial>,
    /// Index range, lightmap page and material for each eligible part.
    parts: HashMap<(Owner, u8), (u32, u32, Option<LightmapKey>, u16)>,
}

impl FogSet {
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
            materials.insert(material, FogMaterial { passes: gpus, words, groups: HashMap::new() });
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
        Self { materials, parts: out.into_iter().collect() }
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
    /// `FogWords::resolve` through `d3d::nv2a::d3d_fog` in the glue) and
    /// its colour (`psh::argb` of `FOGCOLOR`), written through the
    /// submission's upload arena.
    pub fn prepare(
        &self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        fogs: &HashMap<u16, (psh::fog::Fog, [f32; 4])>,
    ) {
        for (&material, m) in &self.materials {
            let Some((fog, color)) = fogs.get(&material) else { continue };
            for (g, _, rigid) in &m.passes {
                let mut pixel = g.program.pixel;
                pixel.fog = fog.uniform();
                pixel.fog_color = *color;
                g.prepare_with(uploads, view, &pixel);
                if let Some(r) = rigid {
                    r.prepare(uploads, view);
                }
            }
        }
    }

    /// [`FogSet::draw`]'s selection as packets for the D3D pass already
    /// open on these attachments, and [`FogSet::prepare`]'s bytes for what
    /// they read as writes ahead of them
    /// (`d3d::backend::ExternalDraws::writes`): one packet per selected
    /// part per pass in reservation order, with the pass's pipeline, its
    /// uniform group (a placement's at its slot's dynamic offset), the
    /// part's texture group, the pass's blend constant, the part's index
    /// range and the whole target as viewport and scissor. The writes are
    /// the selected materials' blocks (and the selected placements'
    /// slots) only: nothing else is read. `cache` holds the described
    /// pipelines ([`crate::gpu::GpuWorld::fog_draws`]); the pipelines
    /// are made ([`FogSet::pipelines`]). Refused when a pass, group or
    /// fog state is missing, as `draw` records nothing then.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draws(
        &self,
        cache: &mut HashMap<wgpu::RenderPipeline, std::sync::Arc<gfx::compile::PendingPipeline>>,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        target_size: (u32, u32),
        selected: &[(Owner, u8)],
        view: &ViewInputs,
        fogs: &HashMap<u16, (psh::fog::Fog, [f32; 4])>,
    ) -> Result<d3d::backend::ExternalDraws, &'static str> {
        use gfx::{BoundGroup, RecordedDraw, ScissorRect};
        let stencil = wgpu::StencilState::default();
        let scissor = ScissorRect::full(target_size.0, target_size.1);
        let render = gfx::state::RenderState::default();
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
            for ((g, state, rigid), textures) in m.passes.iter().zip(groups) {
                let (shader, pipeline, uniforms, offsets) = match owner {
                    Owner::Instance(n) => {
                        let r = rigid.as_ref().ok_or("a fog placement has no rigid program")?;
                        let offset = r.offset(n).ok_or("a fog placement has no slot")?;
                        let pipeline = r.pipeline_at(color, depth, None, &stencil).ok_or("a fog pipeline is not made")?;
                        slots.insert(n);
                        (FOG_RIGID_PROGRAM, pipeline, r.uniforms.clone(), vec![offset])
                    }
                    Owner::Cluster(_) => {
                        let pipeline = g.pipelines.get(&(color, depth)).ok_or("a fog pipeline is not made")?;
                        (FOG_CLUSTER_PROGRAM, pipeline, g.uniforms.clone(), Vec::new())
                    }
                };
                let key = described_pipeline(shader, color, depth, Some(state), None, stencil.clone());
                let pipeline = cache
                    .entry(pipeline.clone())
                    .or_insert_with(|| gfx::compile::Pending::ready(Some(pipeline.clone())))
                    .clone();
                packets.push(
                    RecordedDraw::prepared_indexed(
                        &key,
                        pipeline,
                        vec![(vertices.clone(), 0, vertices.size())],
                        (indices.clone(), u64::from(first_index) * 4, u64::from(index_count) * 4),
                        index_count,
                        vec![BoundGroup { index: 0, group: uniforms, offsets }, BoundGroup::new(1, textures.clone())],
                        &render,
                        target_size,
                        scissor,
                    )
                    .with_blend_constant(state.blend_constant),
                );
            }
        }
        if packets.is_empty() {
            return Err("fog selection draws nothing");
        }
        let mut writes = Vec::new();
        for (material, slots) in &used {
            let m = &self.materials[material];
            let (fog, color) = &fogs[material];
            for (g, _, rigid) in &m.passes {
                let mut pixel = g.program.pixel;
                pixel.fog = fog.uniform();
                pixel.fog_color = *color;
                writes.extend(g.prepare_writes(view, &pixel));
                if let Some(r) = rigid {
                    writes.extend(r.prepare_writes(view, slots.iter().copied()));
                }
            }
        }
        // Conservative: the programs' alpha tests and kill sources discard.
        Ok(d3d::backend::ExternalDraws { ranges: selected.len(), packets, discards: true, writes })
    }

    /// Makes the pipelines for these target formats (before the pass).
    pub fn pipelines(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        for m in self.materials.values_mut() {
            for (g, state, rigid) in &mut m.passes {
                g.pipeline(device, color, depth, Some(&*state));
                if let Some(r) = rigid {
                    r.pipeline_for(device, color, depth, Some(&*state), None, &wgpu::StencilState::default());
                }
            }
        }
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
                        let Some(pipeline) = r.pipeline_at(color, depth, None, &stencil) else { return 0 };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &r.uniforms, &[offset]);
                    }
                    Owner::Cluster(_) => {
                        let Some(pipeline) = g.pipelines.get(&(color, depth)) else { return 0 };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &g.uniforms, &[]);
                    }
                }
                pass.set_bind_group(1, textures, &[]);
                pass.set_blend_constant(state.blend_constant);
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
    fn cluster_code() -> Vec<u8> {
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
            FogPass { look, state, fog: FogWords::default() },
            FogProgram { program, state, words: FogWords::default(), rigid: Some(rigid) },
        )
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
        for packets in [true, false] {
            images.push(render(&ctx, |enc, arena, target| {
                let draws = gpu.fog_draws(&ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), &selected, &view, &fogs).unwrap();
                assert_eq!((draws.ranges, draws.packets.len()), (2, 2), "one pass each, cluster and placement");
                assert_eq!(draws.writes.len(), 2 + 2, "the pass's two blocks, the placement's view prefix and tail");
                if packets {
                    for (buffer, offset, bytes) in &draws.writes {
                        arena.write_front(&ctx.device, &ctx.queue, buffer, *offset, bytes);
                    }
                    gfx::Renderer::encode_pass(&ctx, enc, target, gfx::Clear::NONE, &draws.packets, None).unwrap();
                } else {
                    let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, arena, encoder: enc };
                    gpu.prepare_fog(&mut uploads, &view, &fogs);
                    assert!(draws.writes.iter().all(|(buffer, ..)| arena.wrote_in_order(buffer)));
                    let recorded = gpu.encode_fog(
                        &ctx.device, enc, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float, (SIZE, SIZE), &selected,
                    );
                    assert_eq!(recorded, 2);
                }
            }));
        }
        assert!(images[0].0.iter().any(|&b| b != 0), "the fog drew something");
        assert_eq!(images[0], images[1]);
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
            gpu.fog_draws(&ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), selected, &view, fogs)
        };
        assert!(draws(&mut gpu, &selected(), &fogs).is_ok());
        assert!(draws(&mut gpu, &[(Owner::Cluster(9), 0)], &fogs).is_err(), "a part the set does not hold");
        assert!(draws(&mut gpu, &[], &fogs).is_err(), "nothing to draw");
        assert!(draws(&mut gpu, &selected(), &HashMap::new()).is_err(), "a material with no resolved fog");
    }
}
