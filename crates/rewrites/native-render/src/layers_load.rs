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

use map::{Map, TagId};
use tags::material::Material;

use crate::fog_gpu::{self, FogProgram, FogWords};
use crate::gpu::Image;
use crate::layer_pass::{self, Instance};
use crate::look::{self, Stage};
use crate::object_lighting::ShaderValues;
use crate::program;
use crate::stage_images::{StageDecoder, StageImages};
use crate::world::{LightmapKey, Owner, World};

/// The cause a per-draw resolution is refused with: the materials are
/// resolved once, at the load.
pub const PER_DRAW: &str = "per-draw inputs";

pub type Part = ((Owner, u8), (u32, u32, Option<LightmapKey>, u16));

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
}

/// The layers of one loaded world and what the plan reads of its shaders.
#[derive(Default)]
pub struct LayersView {
    pub layers: HashMap<u8, LayerData>,
    /// Each world material's shader, resolved with its own inputs.
    pub materials: HashMap<u32, Arc<Material>>,
    pub instances: HashMap<u32, Arc<Instance>>,
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
            return out;
        }
    };
    let mut resolved: HashMap<u16, Arc<Material>> = HashMap::new();
    for &m in &materials {
        let Some(&shader) = shaders.get(usize::from(m)) else {
            continue;
        };
        let Ok(material) = lib.material(shader, None) else {
            continue;
        };
        let material = Arc::new(material);
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
            match compile(map, files, &lib, images, decoder, material, layer) {
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
        out.layers.insert(layer, data);
    }
    out
}

/// One material's passes of `layer`, compiled (with the rigid
/// classification for its placements where it compiles) and their stage
/// images decoded, or why it stays on the D3D path.
fn compile<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    lib: &tags::material::ShaderLibrary,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
    material: &Material,
    layer: u8,
) -> Result<Vec<FogProgram>, String> {
    let passes = fog_gpu::layer_passes(material, usize::from(layer)).map_err(|r| r.describe())?;
    let mut out = Vec::with_capacity(passes.len());
    for p in &passes {
        let code = look::world_program(map, lib.metadata(), p.look.vertex_shader)
            .ok_or("no world-space vertex program")?;
        let mut program = FogProgram::compile(p, &code)?;
        if let Some(rigid) = look::rigid_program(map, lib.metadata(), p.look.vertex_shader)
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
