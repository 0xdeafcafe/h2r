//! The light passes' model parts, loaded once per map on a worker
//! (docs/graphics/native-lights.md, "Gathered groups in the scene"):
//! every `mode` tag's drawn opaque parts through
//! [`native_render::light_models::light_model`], with the vertex
//! programs of the classifications their sections take and the stage
//! images of their materials' lit passes. A light layer drawn from its
//! scene group alone ([`super::Glue::alone`]) draws an object entry from
//! these; until the load is done, or for a part it does not hold, the
//! layer keeps its handlers.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};

use map::TagId;
use native_render::gpu::Image;
use native_render::light_look::{DIFFUSE_LAYER, SPECULAR_LAYER};
use native_render::light_models::{LightModels, ModelSection, SectionPart, light_model};
use native_render::look;
use native_render::object_nodes::{Compression, decompression_rows};
use native_render::object_streams::{StreamData, StreamFormats, classification};
use render_bridge::native_map_work;
use tags::material::{EngineInputs, ShaderLibrary, TextureSource};
use tags::model::RenderModelGeometry;

use crate::native_world_glue::{Raw, StagePalettes, light_image};

/// What the light passes draw a map's models with.
pub struct ModelLights {
    pub models: LightModels,
    /// Vertex programs by (`vrtx`, classification).
    pub microcode: HashMap<(TagId, u32), Vec<u8>>,
    /// Stage images by (stage, bitmap, image), as the world's.
    pub images: HashMap<(usize, TagId, u32), Image>,
    pub palettized: HashSet<(usize, TagId, u32)>,
    /// The palettes the load expanded through.
    pub palettes: StagePalettes,
    /// Each shader's instance inputs (`EngineInputs::for_shader`), by
    /// datum.
    pub instances: HashMap<u32, (TagId, EngineInputs)>,
}

struct Load {
    map: String,
    generation: u64,
    checked_frame: Option<u64>,
    result: mpsc::Receiver<Arc<ModelLights>>,
    ready: Option<Arc<ModelLights>>,
}

#[derive(Default)]
pub struct Loader {
    state: Mutex<Option<Load>>,
}

impl Loader {
    /// A view sees the same models for the whole frame. The worker sends
    /// only after uploading the section buffers for this device.
    pub fn get(
        &self, name: &str, device: &wgpu::Device, formats: impl FnOnce() -> Option<StreamFormats>,
        palettes: impl FnOnce() -> StagePalettes,
    ) -> Option<Arc<ModelLights>> {
        let generation = native_map_work::generation();
        let frame = timeline::frames();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(load) = state.as_mut().filter(|load| load.map == name && load.generation == generation) {
            if load.ready.is_none() && load.checked_frame != Some(frame) {
                load.ready = load.result.try_recv().ok();
                load.checked_frame = Some(frame);
            }
            return load.ready.clone();
        }
        let _handoff = timeline::setup::dispatch();
        let (send, result) = mpsc::channel();
        let retired = state.replace(Load {
            map: name.to_string(), generation, result, checked_frame: Some(frame), ready: None,
        });
        drop(state);
        let formats = formats();
        let palettes = palettes();
        let device = device.clone();
        let name = name.to_string();
        let launched = background::spawn("native-lights models", move || {
            drop(retired);
            let Some(formats) = formats else {
                eprintln!("native-lights: {name}: the game's stream formats do not read; object entries keep their handlers");
                return;
            };
            let _setup = timeline::setup::Stage::Lights.worker();
            match load(&name, generation, &formats, &palettes, &device) {
                Ok(loaded) => { let _ = send.send(Arc::new(loaded)); }
                Err(why) if generation == native_map_work::generation() => {
                    eprintln!("native-lights: {name}: model parts not loaded ({why}); object entries keep their handlers");
                }
                Err(_) => {}
            }
        });
        if let Err(e) = launched {
            eprintln!("native-lights: could not start the model loader ({e})");
        }
        None
    }
}

/// The vertex shaders and stage bitmaps of a material's lit passes
/// (layers 12 and 13, first level of detail), resolved with default
/// inputs as the world's load reads them.
fn lit_passes(lib: &mut ShaderLibrary, shader: TagId) -> (Vec<TagId>, Vec<(usize, TagId, u32)>) {
    let Ok(material) = lib.material(shader, None) else { return Default::default() };
    let mut shaders = Vec::new();
    let mut bitmaps = Vec::new();
    for imp in material
        .layers
        .iter()
        .filter(|l| l.lod == 0 && (l.layer == DIFFUSE_LAYER || l.layer == SPECULAR_LAYER))
        .flat_map(|l| &l.passes)
        .flat_map(|p| &p.implementations)
    {
        if let Some(vs) = imp.vertex_shader
            && !shaders.contains(&vs)
        {
            shaders.push(vs);
        }
        for (stage, t) in imp.textures.iter().enumerate().take(4) {
            if let Some(TextureSource::Bitmap { bitmap, image, .. }) = t.map(|t| t.source) {
                bitmaps.push((stage, bitmap, image));
            }
        }
    }
    (shaders, bitmaps)
}

fn load(name: &str, generation: u64, formats: &StreamFormats, palettes: &StagePalettes, device: &wgpu::Device) -> Result<ModelLights, String> {
    let (mut map, files) = crate::native_map::open(name).map_err(|e| e.to_string())?;
    let mut raw = Raw(files);
    let mut lib = ShaderLibrary::read(&mut map).map_err(|e| format!("the shader tags do not read ({e})"))?;
    let meta = tags::object::Metadata::read(&mut map).ok();
    let modes: Vec<TagId> = map.tags().iter().filter(|t| t.group.0 == *b"mode").map(|t| t.id).collect();
    let mut out = ModelLights {
        models: LightModels::default(),
        microcode: HashMap::new(),
        images: HashMap::new(),
        palettized: HashSet::new(),
        palettes: palettes.clone(),
        instances: HashMap::new(),
    };
    let mut passes: HashMap<TagId, (Vec<TagId>, Vec<(usize, TagId, u32)>)> = HashMap::new();
    let mut programs: HashMap<(TagId, u16, u8), Option<(u32, Vec<u16>)>> = HashMap::new();
    let (mut parts, mut refused) = (0usize, 0usize);
    for mode in modes {
        if native_map_work::generation() != generation {
            return Err("the map changed".into());
        }
        let Ok(geo) = RenderModelGeometry::read_in(&mut map, mode, &mut raw, meta.as_ref()) else {
            refused += 1;
            continue;
        };
        let mut sections = Vec::new();
        for (si, s) in geo.sections.iter().enumerate() {
            let flags = s.wire.compression_flags.0;
            let bounds = geo.compression.as_ref();
            let compression = Compression {
                position: bounds.filter(|_| flags & 1 != 0).map(|c| c.position_bounds),
                texcoord: bounds.filter(|_| flags & 2 != 0).map(|c| c.texcoord_bounds),
            };
            let g = &s.geometry;
            let slots = g.vertex_buffers.iter().map(|b| b.slot + 1).max().unwrap_or(0);
            let mut streams = vec![None; slots];
            for b in &g.vertex_buffers {
                streams[b.slot] = Some((b.type_id, StreamData { bytes: g.data(b), stride: usize::from(b.stride) }));
            }
            let mut section_parts = Vec::new();
            for (k, p) in g.parts.iter().enumerate() {
                if k >= 16 || !p.is_drawn() || p.kind == native_render::load::PART_TRANSPARENT {
                    continue;
                }
                let Some(shader) = geo.materials.get(usize::from(p.material)).map(|m| m.shader.id) else { continue };
                let (vertex_shaders, bitmaps) = passes.entry(shader).or_insert_with(|| lit_passes(&mut lib, shader)).clone();
                if vertex_shaders.is_empty() {
                    continue;
                }
                if !out.instances.contains_key(&shader.0)
                    && let Ok(s) = lib.shader(shader)
                {
                    out.instances.insert(shader.0, (shader, EngineInputs::for_shader(&s)));
                }
                for key in bitmaps {
                    let (stage, bitmap, image) = key;
                    if !out.images.contains_key(&key)
                        && let Some((pixels, expanded)) = light_image(&mut map, &mut raw.0, bitmap, image as usize, stage, &palettes[stage])
                    {
                        out.images.insert(key, pixels);
                        if expanded {
                            out.palettized.insert(key);
                        }
                    }
                }
                for &vs in &vertex_shaders {
                    let key = (vs, s.wire.geometry_classification.0, s.wire.unknown_014);
                    if programs.contains_key(&key) {
                        continue;
                    }
                    let read = (|| {
                        let address = map.tag(vs).ok()?.location?.address;
                        let meta = lib.metadata();
                        let class = classification(key.1, key.2, look::classification_count(meta, address)?)?;
                        let semantics = look::vertex_semantics(meta, address, class)?;
                        let code = look::vertex_program(meta, address, class)?;
                        Some((class, semantics, code))
                    })();
                    programs.insert(key, read.map(|(class, semantics, code)| {
                        out.microcode.insert((vs, class), code);
                        (class, semantics)
                    }));
                }
                section_parts.push(SectionPart {
                    part: k as u8,
                    first_index: u32::from(p.first_index),
                    index_count: u32::from(p.index_count),
                    list: p.is_triangle_list(),
                    triangles: g.part_triangles(p),
                    vertex_shaders,
                });
            }
            if section_parts.is_empty() {
                continue;
            }
            parts += section_parts.len();
            sections.push(ModelSection {
                index: si as u16,
                geometry: s.wire.geometry_classification.0,
                influences: s.wire.unknown_014,
                decompression: decompression_rows(&compression),
                streams,
                vertex_count: g.vertex_count(),
                parts: section_parts,
            });
        }
        if sections.is_empty() {
            continue;
        }
        let semantics = |vs: TagId, geometry: u16, influences: u8| programs.get(&(vs, geometry, influences)).cloned().flatten();
        let (model, why) = light_model(&sections, formats, &semantics);
        if hooks::native_verbose() {
            for w in &why {
                eprintln!("native-lights: {name}: model {:#010x}: {w}", mode.0);
            }
        }
        for section in model.sections.values() {
            section.indices.get(device);
            for buffer in &section.buffers {
                buffer.vertices.get(device);
            }
        }
        out.models.models.insert(mode.0, model);
    }
    eprintln!(
        "native-lights: {name}: {} models, {parts} lit parts for the scene groups' object entries ({refused} models do not read), {} vertex programs, {} stage images",
        out.models.models.len(),
        out.microcode.len(),
        out.images.len()
    );
    Ok(out)
}
