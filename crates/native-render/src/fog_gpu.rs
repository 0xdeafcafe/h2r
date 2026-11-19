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

use std::collections::HashMap;

use crate::gpu::Image;
use crate::look::{implementation_look, PassLook, Refusal, Stage};
use crate::pass_state::{pass_state, PassState, StateRefusal};
use crate::program::{compile_pass, world_constants_read, Gpu, MaterialProgram, ViewInputs};
use crate::world::{LightmapKey, Owner};
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
            for (stage, t) in textures.iter_mut().zip(&imp.textures) {
                *stage = match t.map(|t| t.source) {
                    None | Some(tags::material::TextureSource::None) => Stage::Empty,
                    Some(tags::material::TextureSource::Bitmap { bitmap, image, .. }) => Stage::Bitmap { bitmap, image },
                    Some(tags::material::TextureSource::Lightmap) => Stage::Lightmap,
                    Some(tags::material::TextureSource::Engine(id)) => {
                        match crate::screen_look::engine_texture(id) {
                            crate::screen_look::EngineTexture::View | crate::screen_look::EngineTexture::Target(_) => {
                                Stage::Engine(id)
                            }
                            _ => return Err(FogRefusal::Unsupplied { pass: n, stage, id }),
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
    /// The program's instruction words (four per instruction, no header):
    /// what the D3D path's draw binds, for telling its draws apart
    /// (`d3d::uniform_compare`).
    pub code_words: Vec<u32>,
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
        world_constants_read(&pass.look, microcode)?;
        let program = compile_pass(&pass.look, microcode, pass.state.alpha_test)?;
        let parsed = shaders::vsh::Program::parse(microcode).map_err(|e| format!("vertex program: {e:?}"))?;
        let code_words = microcode[4..4 + 16 * parsed.instructions.len()]
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        Ok(FogProgram { program, state: pass.state, words: pass.fog, code_words })
    }
}

struct FogMaterial {
    /// The passes in draw order.
    passes: Vec<(Gpu, PassState)>,
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
    /// Each part the fog draws, by owner and part index: its index range,
    /// lightmap page and material, in the order the load named them (the
    /// world's draw order).
    parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
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
        parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
        programs: HashMap<u16, Vec<FogProgram>>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap_image: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
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
                let Some(g) = Gpu::new(device, queue, p.program, stage_image) else {
                    continue 'material;
                };
                gpus.push((g, p.state));
            }
            materials.insert(material, FogMaterial { passes: gpus, words, groups: HashMap::new() });
        }
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
                    .map(|(g, _)| g.textures(device, page.as_ref()))
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
        Self { materials, parts: out }
    }

    /// How many materials draw natively.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// The parts the set draws, in order.
    pub fn parts(&self) -> &[((Owner, u8), (u32, u32, Option<LightmapKey>, u16))] {
        &self.parts
    }

    /// Whether the set draws `part` of `owner` (whatever the shader): the
    /// `prepare_copy` check that every selected part will record.
    pub fn draws_part(&self, owner: Owner, part: u8) -> bool {
        self.parts.iter().any(|&((o, p), _)| o == owner && p == part)
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
            for (g, _) in &m.passes {
                let mut pixel = g.program.pixel;
                pixel.fog = fog.uniform();
                pixel.fog_color = *color;
                g.prepare_with(uploads, view, &pixel);
            }
        }
    }

    /// Makes the pipelines for these target formats (before the pass).
    pub fn pipelines(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        for m in self.materials.values_mut() {
            for (g, state) in &mut m.passes {
                g.pipeline(device, color, depth, Some(&*state));
            }
        }
    }

    /// Records every part's passes in order into `pass` (the world's
    /// vertex and index buffers bound), each part with every pass of its
    /// material. Returns how many parts were drawn.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> usize {
        let mut drawn = 0;
        for (_, &(first_index, index_count, lightmap, material)) in &self.parts {
            let Some(m) = self.materials.get(&material) else { continue };
            let Some(groups) = m.groups.get(&lightmap) else { continue };
            if m.passes.iter().any(|(g, _)| !g.pipelines.contains_key(&(color, depth))) {
                continue;
            }
            for ((g, state), textures) in m.passes.iter().zip(groups) {
                pass.set_pipeline(&g.pipelines[&(color, depth)]);
                pass.set_bind_group(0, &g.uniforms, &[]);
                pass.set_bind_group(1, textures, &[]);
                pass.set_blend_constant(state.blend_constant);
                pass.draw_indexed(first_index..first_index + index_count, 0, 0..1);
            }
            drawn += 1;
        }
        drawn
    }
}
