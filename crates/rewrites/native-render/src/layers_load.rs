//! The world layers' load (docs/graphics/framescene.md, slice 4): each
//! world material's passes of layers 1, 5, 7, 8 and 20 compiled with their
//! own programs (`fog_gpu::layer_passes` and `FogProgram`, the same
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
    /// Each drawn part with its material's shader datum.
    pub part_set: HashMap<(Owner, u8), u32>,
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
#[allow(clippy::too_many_arguments)]
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
    for layer in layer_pass::LAYERS {
        let mut data = LayerData::default();
        let mut refused: BTreeMap<String, usize> = BTreeMap::new();
        for (&m, material) in &resolved {
            if !material
                .layers
                .iter()
                .any(|l| l.lod == 0 && l.layer == usize::from(layer))
            {
                continue; // the layer draws nothing of it
            }
            match compile(map, files, &lib, images, decoder, material, layer, given) {
                Ok(p) => {
                    data.words
                        .insert(m, p.first().map(|p| p.words).unwrap_or_default());
                    data.writes_depth |= p.iter().any(|p| p.state.depth_write);
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
            .filter_map(|&(k, (_, _, _, m))| Some((k, shaders.get(usize::from(m))?.0)))
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
#[allow(clippy::too_many_arguments)]
fn compile<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    lib: &tags::material::ShaderLibrary,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
    material: &Material,
    layer: u8,
    given: Option<&[u8; program::VIEW_CONSTANTS]>,
) -> Result<Vec<FogProgram>, String> {
    let passes = fog_gpu::layer_passes(material, usize::from(layer)).map_err(|r| r.describe())?;
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StaticLayer {
    /// The materials whose passes of the layer compiled.
    pub compiled: usize,
    /// Those the plan takes as static draws with fog off: what the host
    /// draws.
    pub drawn: BTreeSet<u16>,
    /// The parts of the drawn materials, in the world's draw order.
    pub parts: Vec<Part>,
    /// Why each other material with passes of the layer does not draw,
    /// as `cause: detail`, with how many materials: the load's refusals
    /// as `program`, the plan's by [`refusal_name`], `animated` and
    /// `fog`.
    pub refused: BTreeMap<String, usize>,
}

/// Plans `layer` of `view` with no game: each material whose passes
/// compiled, as a structure item (its plan does not depend on the part:
/// a cluster's and a placement's items draw with no lighting), through
/// [`layer_pass::plan_item`] with the game-free layer inputs
/// (`scene_extract::layout::layer_inputs::without_game` at `time`). A
/// material draws when the plan takes it, its draw is static (what the
/// compiled programs hold: no animation, no per-draw resolution) and its
/// own fog words leave fog off, as the game-free run state has no fog.
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
            owner: LayerOwner::Cluster(0),
            model: u32::MAX,
            rows: 0..0,
            lighting: None,
        };
        let fogged = data
            .words
            .get(&m)
            .is_some_and(|w| w.enable.is_some_and(|e| e != 0));
        let why = match layer_pass::plan_item(&pass, &item, &mut Resolved(view)) {
            Err(r) => format!("{}: {r:?}", refusal_name(&r)),
            Ok(d) if !layer_pass::is_static(&d) => {
                "animated: a pass animates a constant the compiled programs hold".to_owned()
            }
            Ok(_) if fogged => {
                "fog: the material enables fog, which the game-free run state does not give"
                    .to_owned()
            }
            Ok(_) => {
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
#[allow(clippy::too_many_arguments)]
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
}
