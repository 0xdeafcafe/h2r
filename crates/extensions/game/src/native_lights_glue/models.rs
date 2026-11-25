//! The light passes' model parts, loaded once per map on a worker
//! (docs/graphics/native-lights.md, "Gathered groups in the scene"):
//! every `mode` tag's drawn parts through
//! [`native_render::light_models::light_model`], with the vertex
//! programs of the classifications their sections take and the stage
//! images of their materials' lit, caster and receiver passes. A light or shadow
//! layer drawn from its scene group alone ([`super::Glue::alone`] and
//! [`super::Glue::shadow_alone`]) draws an object entry from these; until
//! the load is done, or for a part it does not hold, the layer keeps its
//! handlers.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicU64, Ordering};

use map::TagId;
use native_render::gpu::Image;
use native_render::light_models::{LightModels, ModelSection, SectionPart, light_model};
use native_render::look;
use native_render::object_nodes::{Compression, decompression_rows};
use native_render::object_streams::{StreamData, StreamFormats};
use render_bridge::native_map_work;
use tags::material::{EngineInputs, ShaderLibrary, TextureSource};
use tags::model::RenderModelGeometry;

use crate::native_world_glue::{Raw, StagePalettes, light_image};

/// Identity of one decoded set of model buffers, programs and stage images.
/// Live edits replace the set within the same map generation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ModelSetId(u64);

impl Default for ModelSetId {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// What the light passes draw a map's models with.
#[derive(Default)]
pub struct ModelLights {
    pub(super) id: ModelSetId,
    retire: Option<crate::native_map::RetireQueue>,
    pub models: LightModels,
    model_entries: Option<mem_watch::Gauge>,
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

impl Drop for ModelLights {
    fn drop(&mut self) {
        if let Some(queue) = self.retire.take() {
            crate::native_map::retire(queue, timeline::setup::Stage::Lights, self);
        }
    }
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
    /// Forgets the loaded models: the next [`Loader::get`] loads them
    /// again from the map file, which the raw overlay serves edited (a
    /// live edit of a `mode`, docs/editor/realtime-edits.md). Until that
    /// load is done the object entries keep their handlers.
    pub fn invalidate(&self) {
        let old = self.state.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(old) = old {
            crate::native_map::retire_owned(timeline::setup::Stage::Lights, old);
        }
    }

    /// A view sees the same models for the whole frame. The worker sends
    /// only after uploading the section buffers for this device.
    pub fn get(
        &self,
        name: &str,
        device: &wgpu::Device,
        formats: impl FnOnce() -> Option<StreamFormats>,
        palettes: impl FnOnce() -> StagePalettes,
    ) -> Option<Arc<ModelLights>> {
        let generation = native_map_work::generation();
        let frame = timeline::frames();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if generation != native_map_work::generation() {
            return None;
        }
        if let Some(load) = state
            .as_mut()
            .filter(|load| load.map == name && load.generation == generation)
        {
            if load.ready.is_none() && load.checked_frame != Some(frame) {
                load.ready = load.result.try_recv().ok();
                load.checked_frame = Some(frame);
            }
            return load.ready.clone();
        }
        let _handoff = timeline::setup::dispatch();
        let (send, result) = mpsc::channel();
        let retired = state.replace(Load {
            map: name.to_string(),
            generation,
            result,
            checked_frame: Some(frame),
            ready: None,
        });
        drop(state);
        if let Some(retired) = retired {
            crate::native_map::retire_owned(timeline::setup::Stage::Lights, retired);
        }
        let formats = formats();
        let palettes = palettes();
        let device = device.clone();
        let name = name.to_string();
        let launched = background::spawn("native-lights models", move || {
            let Some(formats) = formats else {
                eprintln!(
                    "native-lights: {name}: the game's stream formats do not read; object entries keep their handlers"
                );
                return;
            };
            let current = || generation == native_map_work::generation();
            let Some(_load) = crate::native_map::begin_load(timeline::setup::Stage::Lights, current) else {
                return;
            };
            let _setup = timeline::setup::Stage::Lights.worker();
            let _scope = mem_watch::scope("native.light_models.load");
            match load(&name, generation, &formats, &palettes, &device) {
                Ok(loaded) if current() => {
                    let _ = send.send(Arc::new(loaded));
                }
                Ok(_) => {}
                Err(why) if generation == native_map_work::generation() => {
                    eprintln!(
                        "native-lights: {name}: model parts not loaded ({why}); object entries keep their handlers"
                    );
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

/// The vertex shaders and stage bitmaps of a material's lit and caster
/// passes at the levels of detail the light renderer supports, resolved with
/// default inputs as the world's load reads them.
fn lit_passes(lib: &mut ShaderLibrary, shader: TagId) -> (Vec<TagId>, Vec<(usize, TagId, u32)>) {
    let Ok(material) = lib.material(shader, None) else {
        return Default::default();
    };
    let mut shaders = Vec::new();
    let mut bitmaps = Vec::new();
    for imp in material
        .layers
        .iter()
        .filter(|l| super::light_layer(l.lod, l.layer))
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

/// An item carries its section part in one byte. Kind 4 uses the section's
/// second influence byte when the light build chooses the vertex program.
fn light_part(index: usize, part: &tags::geometry::Part, opaque: u8, transparent: u8) -> Option<(u8, u8)> {
    if !part.is_drawn() {
        return None;
    }
    let influences = if part.kind == native_render::load::PART_TRANSPARENT { transparent } else { opaque };
    Some((u8::try_from(index).ok()?, influences))
}

/// The light builder's program choice (`native_lights::walk`): the
/// weighted section's influence byte is sign extended before adding two.
fn classification(geometry: u16, influences: u8, count: u32) -> Option<u32> {
    match geometry {
        0..=2 => Some(u32::from(geometry)),
        3 => {
            let class = (i32::from(influences as i8) + 2) as u32;
            Some(if class < count { class } else { 0 })
        }
        _ => None,
    }
}

fn load(
    name: &str,
    generation: u64,
    formats: &StreamFormats,
    palettes: &StagePalettes,
    device: &wgpu::Device,
) -> Result<ModelLights, String> {
    let (mut map, files) = crate::native_map::open(name).map_err(|e| e.to_string())?;
    let mut raw = Raw(files);
    let meta = tags::object::Metadata::read(&mut map)
        .map_err(|e| format!("the shader tags do not read ({e})"))?;
    let mut lib = ShaderLibrary::with_metadata(&map, meta.shader_view());
    let modes: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group.0 == *b"mode")
        .map(|t| t.id)
        .collect();
    let mut out = ModelLights {
        id: ModelSetId::default(),
        retire: Some(crate::native_map::retire_queue().map_err(|e| e.to_string())?),
        models: LightModels::default(),
        model_entries: None,
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
        let Ok(geo) = RenderModelGeometry::read_in(&mut map, mode, &mut raw, Some(&meta)) else {
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
            let slots = g
                .vertex_buffers
                .iter()
                .map(|b| b.slot + 1)
                .max()
                .unwrap_or(0);
            let mut streams = vec![None; slots];
            for b in &g.vertex_buffers {
                streams[b.slot] = Some((
                    b.type_id,
                    StreamData {
                        bytes: g.data(b),
                        stride: usize::from(b.stride),
                    },
                ));
            }
            let mut section_parts = Vec::new();
            for (k, p) in g.parts.iter().enumerate() {
                let Some((part, influences)) =
                    light_part(k, p, s.wire.unknown_014, s.wire.transparent_node_influences)
                else {
                    continue;
                };
                let Some(shader) = geo
                    .materials
                    .get(usize::from(p.material))
                    .map(|m| m.shader.id)
                else {
                    continue;
                };
                let (vertex_shaders, bitmaps) = passes
                    .entry(shader)
                    .or_insert_with(|| lit_passes(&mut lib, shader))
                    .clone();
                if vertex_shaders.is_empty() {
                    continue;
                }
                if !out.instances.contains_key(&shader.0)
                    && let Ok(s) = lib.shader(shader)
                {
                    out.instances
                        .insert(shader.0, (shader, EngineInputs::for_shader(&s)));
                }
                for key in bitmaps {
                    let (stage, bitmap, image) = key;
                    if !out.images.contains_key(&key)
                        && let Some((pixels, expanded)) = light_image(
                            &mut map,
                            &mut raw.0,
                            bitmap,
                            image as usize,
                            stage,
                            &palettes[stage],
                        )
                    {
                        out.images.insert(key, pixels);
                        if expanded {
                            out.palettized.insert(key);
                        }
                    }
                }
                for &vs in &vertex_shaders {
                    let key = (vs, s.wire.geometry_classification.0, influences);
                    if programs.contains_key(&key) {
                        continue;
                    }
                    let read = (|| {
                        let address = map.tag(vs).ok()?.location?.address;
                        let meta = lib.metadata();
                        let class = classification(
                            key.1,
                            key.2,
                            look::classification_count(meta, address)?,
                        )?;
                        let semantics = look::vertex_semantics(meta, address, class)?;
                        let code = look::vertex_program(meta, address, class)?;
                        Some((class, semantics, code))
                    })();
                    programs.insert(
                        key,
                        read.map(|(class, semantics, code)| {
                            out.microcode.insert((vs, class), code);
                            (class, semantics)
                        }),
                    );
                }
                section_parts.push(SectionPart {
                    part,
                    influences,
                    first_index: u32::from(p.first_index),
                    list: p.is_triangle_list(),
                    raw_indices: g.part_indices(p),
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
                decompression: decompression_rows(&compression),
                streams,
                vertex_count: g.vertex_count(),
                parts: section_parts,
            });
        }
        if sections.is_empty() {
            continue;
        }
        let semantics = |vs: TagId, geometry: u16, influences: u8| {
            programs.get(&(vs, geometry, influences)).cloned().flatten()
        };
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
    static MODELS: mem_watch::Counter = mem_watch::Counter::new("native.light_model_entries");
    out.model_entries = Some(MODELS.track(out.models.models.len() as u64));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{classification, light_part};

    #[test]
    fn gathered_model_parts_use_the_whole_item_byte_and_their_own_influences() {
        let part = |kind| tags::geometry::Part {
            kind,
            flags: 0,
            material: 0,
            first_index: 0,
            index_count: 3,
            first_subpart: 0,
            subpart_count: 0,
            raw: Vec::new(),
        };
        assert_eq!(light_part(0, &part(2), 1, 3), Some((0, 1)));
        assert_eq!(light_part(23, &part(4), 1, 3), Some((23, 3)));
        assert_eq!(light_part(255, &part(2), 1, 3), Some((255, 1)));
        assert_eq!(light_part(256, &part(2), 1, 3), None);
        assert_eq!(light_part(0, &part(5), 1, 3), None);
        assert_eq!(light_part(0, &part(0), 1, 3), None);
    }

    #[test]
    fn light_vertex_class_uses_the_signed_influence_and_available_programs() {
        assert_eq!(classification(1, 7, 8), Some(1));
        assert_eq!(classification(3, 2, 5), Some(4));
        assert_eq!(classification(3, 2, 4), Some(0));
        assert_eq!(classification(3, 0xff, 4), Some(1));
        assert_eq!(classification(3, 0x80, 4), Some(0));
        assert_eq!(classification(4, 0, 4), None);
    }
}
