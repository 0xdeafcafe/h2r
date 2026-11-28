//! The world layers' load (docs/graphics/framescene.md, slice 4): each
//! world material's passes of every layer `layer_pass` plans (1, 5, 7, 8,
//! 20, the overlay layer 2 and layer 17: `layer_pass::planned_layers`) compiled with their
//! own programs (`fog_gpu::layer_passes_selected` and `FogProgram`, the same
//! translation the fog's layer 16 takes), all or nothing per material and
//! layer, their stage images decoded; the parts that can draw them; and
//! each material resolved once, with its instance's animated properties
//! and input scales, for the plan (`layer_pass`). Read from the map file
//! alone (`tags::material::ShaderLibrary`), so the game's glue and the
//! editor load the same layers.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use frame_scene::{ItemKey, LayerItem, LayerOwner, LayerPass};
use map::{Map, TagId};
use tags::material::{EngineInputs, Material};

use crate::fog_gpu::{self, FogProgram, FogWords};
use crate::gpu::Image;
use crate::layer_pass::{self, Instance, LayerRefusal, Materials};
use crate::look::{self, Stage};
use crate::object_lighting::ShaderValues;
use crate::program;
use crate::stage_images::{StageDecoder, StageImages};
use crate::world::{LightmapKey, Owner, World};

/// The cause a per-draw resolution is refused with: the materials are
/// resolved once, at the load.
pub const PER_DRAW: &str = "per-draw inputs";

/// Layer 15, the sorted transparent geometry's: compiled and uploaded with
/// the planned layers, drawn only for the entries R2's geometry arm answers
/// (`effects_shadow::geometry`), never planned from the scene.
pub const TRANSPARENT_LAYER: u8 = 15;

pub type Part = ((Owner, u8), (u32, u32, Option<LightmapKey>, u16));

/// The plan's view of the load's resolved shaders: each material as the
/// load resolved it, with its instance. A per-draw resolution is refused
/// with [`PER_DRAW`]: the compiled programs hold the load's.
pub struct Resolved<'a>(pub &'a LayersView);

impl Materials for Resolved<'_> {
    fn material(
        &mut self,
        shader: u32,
        inputs: Option<&EngineInputs>,
    ) -> Result<Arc<Material>, String> {
        if inputs.is_some() {
            return Err(PER_DRAW.to_owned());
        }
        self.0
            .materials
            .get(&shader)
            .cloned()
            .ok_or_else(|| "not a world material".to_owned())
    }

    fn instance(&mut self, shader: u32) -> Result<Arc<Instance>, String> {
        self.0
            .instances
            .get(&shader)
            .cloned()
            .ok_or_else(|| "not a world material".to_owned())
    }
}

/// A plan refusal's cause as the hosts count it: the plan's name, with a
/// per-draw resolution ([`PER_DRAW`]) apart from a missing material.
pub fn refusal_name(r: &LayerRefusal) -> &'static str {
    match r {
        LayerRefusal::Material(why) if why == PER_DRAW => "per_draw",
        r => r.name(),
    }
}

/// One layer of one loaded world: what compiled and the parts that draw.
#[derive(Default)]
pub struct LayerData {
    /// Each material's compiled passes, in draw order.
    pub programs: HashMap<u16, Vec<FogProgram>>,
    /// Each material's own fog words, resolved over the device's at the ask.
    pub words: HashMap<u16, FogWords>,
    /// The parts the set draws, in the world's draw order.
    pub parts: Vec<Part>,
    /// Each drawn part with its material index and shader datum.
    pub part_set: HashMap<(Owner, u8), (u16, u32)>,
    /// Whether any pass's state writes depth.
    pub writes_depth: bool,
    /// Why each material that has passes of the layer does not draw, with
    /// how many are refused for each cause.
    pub refused: BTreeMap<String, usize>,
}

/// The layers of one loaded world and what the plan reads of its shaders.
#[derive(Default)]
pub struct LayersView {
    pub layers: HashMap<u8, LayerData>,
    /// Each world material's shader, resolved with its own inputs.
    pub materials: HashMap<u32, Arc<Material>>,
    pub instances: HashMap<u32, Arc<Instance>>,
    /// Why the world materials that did not resolve did not (the shader
    /// tags not reading at all, or one material's), with how many: they
    /// have no layer passes to compile.
    pub unresolved: BTreeMap<String, usize>,
}

/// Compiles every world material's passes of the layers the stage takes
/// and picks the parts that can draw them. Called from the world's
/// worker, before its draws are kept.
pub fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
) -> LayersView {
    load_with(map, files, shaders, world, lightmaps, images, decoder, None)
}

/// [`load`], with the view constants the caller gives. With `given`, a
/// pass whose vertex program reads a constant (or a component of one)
/// the mask leaves out is refused, named with the register
/// (`program::ungiven_view_read`), so no pass draws with a register the
/// view leaves at zero. The game gives every constant (`None`); the
/// editor gives `program::CAMERA_GIVEN`.
pub fn load_with<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
    given: Option<&[u8; program::VIEW_CONSTANTS]>,
) -> LayersView {
    let mut out = LayersView::default();
    let materials: Vec<u16> = world
        .draws
        .iter()
        .map(|d| d.material)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if materials.is_empty() {
        return out;
    }
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!(
                "native-layers: the shader tags do not read ({e}); the D3D path draws layers 1, 5, 7, 8 and 20"
            );
            out.unresolved.insert(
                format!("the shader tags do not read ({e})"),
                materials.len(),
            );
            return out;
        }
    };
    let mut resolved: HashMap<u16, Arc<Material>> = HashMap::new();
    for &m in &materials {
        let Some(&shader) = shaders.get(usize::from(m)) else {
            *out.unresolved.entry("no shader".to_owned()).or_default() += 1;
            continue;
        };
        let material = match lib.material(shader, None) {
            Ok(material) => Arc::new(material),
            Err(e) => {
                *out.unresolved.entry(e.to_string()).or_default() += 1;
                continue;
            }
        };
        if let Ok(instance) = lib.shader(shader) {
            out.instances.insert(
                shader.0,
                Arc::new(Instance {
                    properties: instance
                        .postprocess
                        .first()
                        .map(|p| p.properties.clone())
                        .unwrap_or_default(),
                    values: ShaderValues {
                        scalar_21: instance.scalar_input.to_bits(),
                        vector_24: instance.vector_input.to_bits(),
                    },
                }),
            );
        }
        out.materials.insert(shader.0, Arc::clone(&material));
        resolved.insert(m, material);
    }
    for layer in layer_pass::planned_layers().chain(std::iter::once(TRANSPARENT_LAYER)) {
        let mut data = LayerData::default();
        let mut refused: BTreeMap<String, usize> = BTreeMap::new();
        // A pass that leaves `CULLMODE` unset draws with the cull the layer
        // starts with ([`fog_gpu::layer_cull`]) unless some pass of the
        // layer sets another, which it may then follow.
        let selection = fog_gpu::Selection {
            implementation: None,
            layer_cull: !resolved.values().any(|m| fog_gpu::layer_sets_other_cull(m, usize::from(layer))),
            rasterizer: None,
        };
        for (&m, material) in &resolved {
            if !material
                .layers
                .iter()
                .any(|l| l.lod == 0 && l.layer == usize::from(layer))
            {
                continue; // the layer draws nothing of it
            }
            let properties = out.instances.get(&material.shader.0).map_or(&[][..], |i| i.properties.as_slice());
            match compile(map, files, &lib, images, decoder, material, layer, selection, given, properties) {
                Ok(p) => {
                    data.words
                        .insert(m, p.first().map(|p| p.words).unwrap_or_default());
                    data.writes_depth |= p.iter().any(|p| p.state.writes_depth_stencil());
                    data.programs.insert(m, p);
                }
                Err(why) => *refused.entry(why).or_default() += 1,
            }
        }
        for d in &world.draws {
            let Some(passes) = data.programs.get(&d.material) else {
                continue;
            };
            if !passes.iter().all(|p| match d.owner {
                Owner::Cluster(_) => p.program.inputs & !d.inputs == 0,
                Owner::Instance(i) => {
                    world.rows(i).is_some()
                        && p.rigid.as_ref().is_some_and(|r| r.inputs & !d.inputs == 0)
                }
            }) {
                continue;
            }
            let samples_lightmap = passes.iter().any(|p| {
                p.program
                    .stages
                    .iter()
                    .zip(&p.program.dims)
                    .any(|(s, dim)| *s == Stage::Lightmap && dim.is_some())
            });
            if samples_lightmap && !d.lightmap.is_some_and(|k| lightmaps.contains_key(&k)) {
                continue;
            }
            data.parts.push((
                (d.owner, d.part),
                (d.first_index, d.index_count, d.lightmap, d.material),
            ));
        }
        data.part_set = data
            .parts
            .iter()
            .filter_map(|&(k, (_, _, _, m))| Some((k, (m, shaders.get(usize::from(m))?.0))))
            .collect();
        if !data.programs.is_empty() || !refused.is_empty() {
            eprintln!(
                "native-layers: layer {layer}: {} materials translate, {} parts native; on the D3D path: {refused:?}",
                data.programs.len(),
                data.parts.len()
            );
        }
        data.refused = refused;
        out.layers.insert(layer, data);
    }
    out
}

/// One material's passes of `layer`, compiled (with the rigid
/// classification for its placements where it compiles) and their stage
/// images decoded, or why it stays on the D3D path. With `given`, a vertex
/// program reading a view constant outside it refuses the material.
fn compile<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    lib: &tags::material::ShaderLibrary,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
    material: &Material,
    layer: u8,
    selection: fog_gpu::Selection,
    given: Option<&[u8; program::VIEW_CONSTANTS]>,
    properties: &[tags::shader::AnimatedProperty],
) -> Result<Vec<FogProgram>, String> {
    let passes = fog_gpu::sampled_layer_passes(material, usize::from(layer), selection, properties).map_err(|r| r.describe())?;
    let mut out = Vec::with_capacity(passes.len());
    for p in &passes {
        let code = look::world_program(map, lib.metadata(), p.look.vertex_shader)
            .ok_or("no world-space vertex program")?;
        if let Some(why) = given.and_then(|g| program::ungiven_view_read(&code, g)) {
            return Err(why);
        }
        let mut program = FogProgram::compile(p, &code)?;
        if let Some(rigid) = look::rigid_program(map, lib.metadata(), p.look.vertex_shader)
            && given.is_none_or(|g| program::ungiven_view_read(&rigid, g).is_none())
            && let Ok(r) = program::compile_pass_rigid(&p.look, p.state.alpha_test, &rigid)
        {
            program.rigid = Some(r);
        }
        for (n, s) in program.program.stages.iter().enumerate() {
            if let Stage::Bitmap { bitmap, image } = *s
                && program.program.dims[n].is_some()
            {
                decoder.decode(map, files, images, bitmap, image, n, false)?;
            }
        }
        out.push(program);
    }
    Ok(out)
}

/// What a host with no game (the editor) draws of one loaded layer
/// ([`plan_without_game`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StaticLayer {
    /// The materials whose passes of the layer compiled.
    pub compiled: usize,
    /// The materials the plan accepts with fog off.
    pub drawn: BTreeSet<u16>,
    /// Sampled uniforms for the drawn materials with animated constants.
    pub constants: fog_gpu::LayerSamples,
    /// The parts of the drawn materials, in the world's draw order.
    pub parts: Vec<Part>,
    /// Why each other material with passes of the layer does not draw,
    /// as `cause: detail`, with how many materials: the load's refusals
    /// as `program`, the plan's by [`refusal_name`], and `fog`.
    pub refused: BTreeMap<String, usize>,
    /// Layer 3's parts of a drawn material that do not take its pass
    /// ([`plan_layer3`]), by cause, with how many parts: the host draws
    /// them with its base look.
    pub parts_refused: BTreeMap<String, usize>,
}

/// Plans `layer` of `view` with no game: each material whose passes
/// compiled, as a structure item (its plan does not depend on the part:
/// a cluster's and a placement's items draw with no lighting), through
/// [`layer_pass::plan_item`] with the game-free layer inputs
/// (`scene_extract::layout::layer_inputs::without_game` at `time`). A
/// material draws when the plan accepts its inputs and its own fog words
/// leave fog off. Animated pass constants enter the plan at `time`.
/// Every other material is named with its cause; nothing is drawn with
/// an input, a constant or a fog the host does not give.
pub fn plan_without_game(
    view: &LayersView,
    shaders: &[TagId],
    layer: u8,
    time: f64,
) -> StaticLayer {
    let Some(data) = view.layers.get(&layer) else {
        return StaticLayer::default();
    };
    let mut out = StaticLayer {
        compiled: data.programs.len(),
        ..Default::default()
    };
    for (why, &n) in &data.refused {
        *out.refused.entry(format!("program: {why}")).or_default() += n;
    }
    let pass = LayerPass {
        layer,
        // The game-free pass holds no stencil.
        stencil_off: true,
        texaccum: false,
        items: 0..0,
        remainder: 0,
        inputs: scene_extract::layout::layer_inputs::without_game(time),
    };
    let compiled: BTreeSet<u16> = data.programs.keys().copied().collect();
    for m in compiled {
        let Some(&shader) = shaders.get(usize::from(m)) else {
            *out.refused
                .entry("material: no shader".to_owned())
                .or_default() += 1;
            continue;
        };
        let item = LayerItem {
            slot: 0,
            key: ItemKey {
                record: 0,
                shader: shader.0,
                section: 0,
                part: 0,
                implementation: 0,
            },
            level: 0, pass: 0, owner: LayerOwner::Cluster(0),
            model: u32::MAX,
            rows: 0..0,
            lighting: None,
            flags: frame_scene::PartFlags::NONE,
            subparts: None,
            refusal: None,
            prt: None,
        };
        let fogged = data
            .words
            .get(&m)
            .is_some_and(|w| w.enable.is_some_and(|e| e != 0));
        let why = match layer_pass::plan_item(&pass, &item, &mut Resolved(view)) {
            Err(r) => format!("{}: {r:?}", refusal_name(&r)),
            Ok(_) if fogged => {
                "fog: the material enables fog, which the game-free run state does not give"
                    .to_owned()
            }
            Ok(draw) => {
                if let Some(constants) = fog_gpu::sampled_constants(&draw) {
                    out.constants.insert(m, constants);
                }
                out.drawn.insert(m);
                continue;
            }
        };
        *out.refused.entry(why).or_default() += 1;
    }
    out.parts = data
        .parts
        .iter()
        .filter(|&&(_, (_, _, _, m))| out.drawn.contains(&m))
        .copied()
        .collect();
    out
}

/// The world layer a host's base look stands for: the game's layer 3
/// (base map, detail and lightmap).
pub const LAYER_3: u8 = 3;

/// One world's layer 3 for a host with no game ([`load_layer3`]).
#[derive(Default)]
pub struct Layer3 {
    /// What compiled, the parts that take it and why the rest do not.
    pub data: LayerData,
    /// The parts of compiled materials that do not take the pass, by
    /// cause (`program::part_fit`), with how many.
    pub parts_refused: BTreeMap<String, usize>,
    /// The compiled materials whose pass animates its constants, sampled
    /// at the host's clock by [`plan_layer3`].
    pub animations: HashMap<u16, layer_pass::PassAnimation>,
}

/// Every world material's layer-3 pass, compiled for a host with no game
/// as the game's world glue compiles it: `look::layer3_pass_sampled`, or
/// `look::unlit_bsp_pass_sampled` for a BSP with no lightmap group
/// (`unlit_bsp`; no normalization map is given), an animated pass at
/// clock 0 with its animation kept ([`Layer3::animations`]), the world's pixel parity
/// (`program::world_parity`) and constants, and the rigid classification
/// for its placements. It draws through a `fog_gpu::FogSet` with fog off
/// (`FogProgram`, fog words unset: layer 3 has none). With `given`, a
/// pass reading a view constant outside it is refused, named with the
/// register; a stage bitmap that is not 2D (the set binds 2D images
/// only), is linear (addressed in texels on the D3D path) or palettized
/// (no stage palettes) is refused, named. A part draws when it supplies
/// what the pass reads (`program::part_fit`) and its page decoded.
pub fn load_layer3<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
    unlit_bsp: bool,
    given: Option<&[u8; program::VIEW_CONSTANTS]>,
) -> Layer3 {
    let mut out = Layer3::default();
    let materials: BTreeSet<u16> = world.draws.iter().map(|d| d.material).collect();
    if materials.is_empty() {
        return out;
    }
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(lib) => lib,
        Err(e) => {
            out.data.refused.insert(
                format!("the shader tags do not read ({e})"),
                materials.len(),
            );
            return out;
        }
    };
    for &m in &materials {
        let compiled = match shaders.get(usize::from(m)) {
            None => Err("no shader".to_owned()),
            Some(&shader) => compile_layer3(
                map, files, &mut lib, images, decoder, shader, unlit_bsp, given,
            ),
        };
        match compiled {
            Ok((p, animation)) => {
                out.data.writes_depth |= p.state.writes_depth_stencil();
                out.data.words.insert(m, p.words);
                out.data.programs.insert(m, vec![p]);
                if let Some(animation) = animation {
                    out.animations.insert(m, animation);
                }
            }
            Err(why) => *out.data.refused.entry(why).or_default() += 1,
        }
    }
    let page = |k: LightmapKey| {
        if lightmaps.contains_key(&k) {
            Ok(())
        } else {
            Err("a part's lightmap page does not decode".to_owned())
        }
    };
    for d in &world.draws {
        let Some([p]) = out.data.programs.get(&d.material).map(Vec::as_slice) else {
            continue;
        };
        let inputs = match d.owner {
            Owner::Cluster(_) => Ok(p.program.inputs),
            Owner::Instance(i) if world.rows(i).is_none() => {
                Err("placement rows unavailable".to_owned())
            }
            Owner::Instance(_) => p
                .rigid
                .as_ref()
                .map(|r| r.inputs)
                .ok_or_else(|| "rigid vertex program unavailable".to_owned()),
        };
        match inputs.and_then(|inputs| program::part_fit(d, inputs, &p.program, &page)) {
            Ok(()) => out.data.parts.push((
                (d.owner, d.part),
                (d.first_index, d.index_count, d.lightmap, d.material),
            )),
            Err(why) => *out.parts_refused.entry(why).or_default() += 1,
        }
    }
    out.data.part_set = out
        .data
        .parts
        .iter()
        .filter_map(|&(k, (_, _, _, m))| Some((k, (m, shaders.get(usize::from(m))?.0))))
        .collect();
    out
}

/// One material's layer-3 pass compiled for [`load_layer3`], its stage
/// images decoded, or why it is not drawn.
fn compile_layer3<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    lib: &mut tags::material::ShaderLibrary,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
    shader: TagId,
    unlit_bsp: bool,
    given: Option<&[u8; program::VIEW_CONSTANTS]>,
) -> Result<(FogProgram, Option<layer_pass::PassAnimation>), String> {
    let properties = layer_pass::instance_properties(lib, shader);
    let (pass, animation) = if unlit_bsp {
        let selection = tags::material::PassSelection {
            lod: 0,
            layer: look::LIGHTMAP_LAYER,
            pass: 0,
            implementation: look::UNLIT_BSP_IMPLEMENTATION,
        };
        let selected = lib
            .selected_implementation(shader, selection, None, None)
            .map_err(|e| format!("material: {e}"))?;
        look::unlit_bsp_pass_sampled(&selected, &tags::material::EngineInputs::default(), None, &properties)
            .map_err(|r| format!("layer 3: {r:?}"))?
    } else {
        let material = lib
            .material(shader, None)
            .map_err(|e| format!("material: {e}"))?;
        look::layer3_pass_sampled(&material, &properties).map_err(|r| format!("layer 3: {r:?}"))?
    };
    let code = look::world_program(map, lib.metadata(), pass.look.vertex_shader)
        .ok_or("no world-space vertex program")?;
    if let Some(why) = given.and_then(|g| program::ungiven_view_read(&code, g)) {
        return Err(why);
    }
    program::world_parity(&pass.look)?;
    for (n, stage) in pass.look.textures.iter().enumerate() {
        match *stage {
            Stage::Bitmap { bitmap, image } => two_d(map, bitmap, image, n)?,
            Stage::Empty | Stage::Lightmap => {}
            Stage::Engine(id) => return Err(format!("stage {n} is engine texture {id}")),
            Stage::Host(h) => return Err(format!("stage {n} is a host stage ({h:?})")),
        }
    }
    let mut state = pass.state;
    if pass.inherits_cull {
        let (cull_mode, front_face) = device_default_cull();
        state.primitive.cull_mode = cull_mode;
        state.primitive.front_face = front_face;
    }
    let fog_pass = fog_gpu::FogPass {
        look: pass.look,
        state,
        fog: FogWords::default(),
        planar: None,
    };
    let mut p = FogProgram::compile(&fog_pass, &code)?;
    if let Some(rigid) = look::rigid_program(map, lib.metadata(), fog_pass.look.vertex_shader)
        && given.is_none_or(|g| program::ungiven_view_read(&rigid, g).is_none())
        && let Ok(r) =
            program::compile_pass_rigid(&fog_pass.look, fog_pass.state.alpha_test, &rigid)
    {
        p.rigid = Some(r);
    }
    for (n, s) in p.program.stages.iter().enumerate() {
        if let Stage::Bitmap { bitmap, image } = *s
            && p.program.dims[n].is_some()
        {
            decoder.decode(map, files, images, bitmap, image, n, true)?;
        }
    }
    Ok((p, animation))
}

/// The cull a layer-3 pass that leaves `CULLMODE` unset draws with when
/// no game holds the device's (`look::WorldPass::inherits_cull`): the D3D
/// device's default, `CULLMODE` CCW with `FRONTFACE` CW. The source is
/// the guest device default, which translates to
/// `gfx::state::RenderState::default()` (its test
/// `defaults_translate_cleanly_to_gfx_defaults`), and the world draws
/// keep it unless a pass sets its own.
pub fn device_default_cull() -> (Option<wgpu::Face>, wgpu::FrontFace) {
    let p = gfx::translate::primitive_state(
        &gfx::state::RenderState::default(),
        wgpu::PrimitiveTopology::TriangleList,
        gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
    );
    (p.cull_mode, p.front_face)
}

/// Whether image `image` of `bitmap` (bound at stage `stage`) is 2D: a
/// cube or a volume would need its own binding.
fn two_d<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    bitmap: TagId,
    image: u32,
    stage: usize,
) -> Result<(), String> {
    let tags::TagData::Bitmap(tag) = tags::DecodedTag::read(map, bitmap)
        .map_err(|e| format!("stage {stage} bitmap does not read ({e})"))?
        .data
    else {
        return Err(format!("stage {stage} datum is not a bitmap"));
    };
    let kind = tag
        .images
        .get(image as usize)
        .ok_or_else(|| format!("stage {stage} bitmap has no image {image}"))?
        .kind;
    match kind {
        tags::TextureKind::D2 => Ok(()),
        tags::TextureKind::D3 | tags::TextureKind::Cube | tags::TextureKind::Unknown(_) => {
            Err(format!("stage {stage} bitmap is {kind:?}, not 2D"))
        }
    }
}

/// [`load_layer3`] as a host with no game draws it: every compiled
/// material, its parts that take the pass, the load's refusals as
/// `program: cause`, and the parts left to the base look by cause. Layer
/// 3's passes are compiled with the instance's constants and nothing per
/// draw, as the game's are; an animated one's constants are sampled at
/// `time` (the host's clock, [`Layer3::animations`]), and one the sampler
/// refuses is named `animation: cause` with its parts left out.
pub fn plan_layer3(layer: &Layer3, time: f64) -> StaticLayer {
    let data = &layer.data;
    let mut drawn: BTreeSet<u16> = data.programs.keys().copied().collect();
    let mut constants = fog_gpu::LayerSamples::new();
    let mut refused: BTreeMap<String, usize> =
        data.refused.iter().map(|(why, &n)| (format!("program: {why}"), n)).collect();
    for (&m, animation) in &layer.animations {
        match animation.sample(Some(time)) {
            Ok(c) => {
                constants.insert(m, vec![Some(c)]);
            }
            Err(r) => {
                drawn.remove(&m);
                *refused.entry(format!("animation: {}", r.name())).or_default() += 1;
            }
        }
    }
    StaticLayer {
        compiled: data.programs.len(),
        parts: data.parts.iter().filter(|(_, (_, _, _, m))| drawn.contains(m)).copied().collect(),
        drawn,
        constants,
        refused,
        parts_refused: layer.parts_refused.clone(),
    }
}

/// Each material's fog unit and colour, as a layer set's uniforms take
/// them ([`fog_gpu::FogSet::prepare`]).
pub type Fogs = HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>;

/// A game-free layer on the GPU ([`upload_static`]).
pub struct StaticSet {
    pub set: fog_gpu::FogSet,
    /// The parts the set draws, in the world's draw order.
    pub selected: Vec<(Owner, u8)>,
    /// Fog off for every drawn material: the game-free run state has no
    /// fog, and [`plan_without_game`] refused the materials whose own
    /// words enable it.
    pub fogs: Fogs,
}

/// Uploads the materials [`plan_without_game`] draws of `layer` and
/// selects the parts the set binds (a part whose images did not upload is
/// left out: `FogSet::draw` stops at a part it cannot draw). `None` when
/// none binds.
pub fn upload_static(
    view: &LayersView,
    layer: u8,
    plan: &StaticLayer,
    images: &StageImages,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &World,
    lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
) -> Option<StaticSet> {
    let data = view.layers.get(&layer)?;
    if plan.parts.is_empty() {
        return None;
    }
    let programs = data
        .programs
        .iter()
        .filter(|(m, _)| plan.drawn.contains(m))
        .map(|(&m, p)| (m, p.clone()))
        .collect();
    let set = fog_gpu::FogSet::new(
        device,
        queue,
        world,
        plan.parts.clone(),
        programs,
        &mut |s| match s {
            Stage::Bitmap { bitmap, image } => images.get(&(bitmap, image)).cloned(),
            _ => None,
        },
        lightmap,
    );
    let selected: Vec<(Owner, u8)> = plan
        .parts
        .iter()
        .map(|&(k, _)| k)
        .filter(|&(owner, part)| set.draws_part(owner, part))
        .collect();
    if selected.is_empty() {
        return None;
    }
    let fogs = plan
        .drawn
        .iter()
        .map(|&m| (m, (shaders::psh::fog::Fog::OFF, [0.0; 4])))
        .collect();
    Some(StaticSet {
        set,
        selected,
        fogs,
    })
}

/// Builds the layers resources on the BSP loader's device.
pub fn upload_layers(
    view: &mut LayersView,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &World,
    stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
    lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
) -> Vec<(u8, fog_gpu::FogSet)> {
    let mut sets = Vec::new();
    for (&layer, data) in &mut view.layers {
        if data.programs.is_empty() {
            continue;
        }
        let set = fog_gpu::FogSet::new(
            device,
            queue,
            world,
            data.parts.clone(),
            data.programs.clone(),
            stage_image,
            lightmap,
        );
        data.part_set
            .retain(|&(owner, part), _| set.draws_part(owner, part));
        sets.push((layer, set));
    }
    sets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_reads_only_what_the_load_resolved() {
        let view = LayersView::default();
        let mut resolved = Resolved(&view);
        assert_eq!(
            resolved.material(7, None).unwrap_err(),
            "not a world material"
        );
        assert_eq!(
            resolved
                .material(7, Some(&EngineInputs::default()))
                .unwrap_err(),
            PER_DRAW
        );
        assert!(resolved.instance(7).is_err());
    }

    #[test]
    fn a_per_draw_refusal_counts_apart_from_a_missing_material() {
        assert_eq!(
            refusal_name(&LayerRefusal::Material(PER_DRAW.to_owned())),
            "per_draw"
        );
        assert_eq!(
            refusal_name(&LayerRefusal::Material("not a world material".to_owned())),
            "material"
        );
        assert_eq!(refusal_name(&LayerRefusal::NoPass), "no_pass");
    }

    /// The load's refusals carry into the game-free plan as `program`,
    /// and a layer the load did not keep plans nothing.
    #[test]
    fn a_game_free_plan_names_what_the_load_refused() {
        let mut view = LayersView::default();
        let mut data = LayerData::default();
        data.refused.insert(
            "vertex program reads c[6].x, which the view does not give".to_owned(),
            2,
        );
        view.layers.insert(5, data);
        let plan = plan_without_game(&view, &[], 5, 0.0);
        assert_eq!(plan.compiled, 0);
        assert!(plan.drawn.is_empty() && plan.parts.is_empty());
        assert_eq!(
            plan.refused,
            BTreeMap::from([(
                "program: vertex program reads c[6].x, which the view does not give".to_owned(),
                2
            )])
        );
        assert_eq!(
            plan_without_game(&view, &[], 8, 0.0),
            StaticLayer::default()
        );
    }

    /// The device's default cull culls one face: a pass inheriting it no
    /// longer draws both.
    #[test]
    fn a_pass_inheriting_cull_takes_the_devices_default() {
        let (cull, _) = device_default_cull();
        assert!(cull.is_some());
    }

    /// Layer 3's plan draws every compiled material with the parts that
    /// take its pass, and names the load's refusals and the parts left
    /// to the base look.
    #[test]
    fn layer_3s_plan_names_what_does_not_draw() {
        let mut layer = Layer3::default();
        layer.data.refused.insert("layer 3: Blended".to_owned(), 2);
        layer
            .parts_refused
            .insert("a part has no lightmap page".to_owned(), 3);
        let part = ((Owner::Cluster(0), 0), (0, 6, None, 4));
        layer.data.parts.push(part);
        layer.data.programs.insert(4, Vec::new());
        let plan = plan_layer3(&layer, 0.0);
        assert_eq!(plan.compiled, 1);
        assert_eq!(plan.drawn, BTreeSet::from([4]));
        assert_eq!(plan.parts, [part]);
        assert_eq!(
            plan.refused,
            BTreeMap::from([("program: layer 3: Blended".to_owned(), 2)])
        );
        assert_eq!(
            plan.parts_refused,
            BTreeMap::from([("a part has no lightmap page".to_owned(), 3)])
        );
    }

    /// An animated layer-3 material's constants are sampled at the host's
    /// clock; one the sampler refuses is named and its parts left out.
    #[test]
    fn layer_3s_plan_samples_its_animated_materials() {
        use crate::layer_pass::tests::{animated_z, implementation};
        let (target, props) = animated_z();
        let imp = implementation(0, tags::material::InputReads::default(), vec![target]);
        let (_, animation) = layer_pass::PassAnimation::baseline(&imp, &props, false, 0).unwrap();
        let mut layer = Layer3::default();
        let part = ((Owner::Cluster(0), 0), (0, 6, None, 4));
        let still = ((Owner::Cluster(1), 0), (6, 6, None, 5));
        layer.data.parts.extend([part, still]);
        layer.data.programs.insert(4, Vec::new());
        layer.data.programs.insert(5, Vec::new());
        layer.animations.insert(4, animation.unwrap());
        let plan = plan_layer3(&layer, 11.0);
        assert_eq!(plan.drawn, BTreeSet::from([4, 5]));
        let [Some(c)] = plan.constants[&4].as_slice() else { panic!("one sampled pass") };
        assert_eq!(c.vertex(&shaders::vsh::VertexUniforms::default()).constants[18][2], 1.5);
        assert!(!plan.constants.contains_key(&5), "nothing animates");
        let refused = plan_layer3(&layer, f64::NAN);
        assert_eq!(refused.drawn, BTreeSet::from([5]));
        assert_eq!(refused.parts, [still]);
        assert_eq!(refused.refused, BTreeMap::from([("animation: property".to_owned(), 1)]));
    }
}
