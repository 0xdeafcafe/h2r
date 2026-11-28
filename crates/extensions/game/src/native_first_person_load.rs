//! Map-local models for the native first-person pass. This deliberately
//! owns a separate cache: the objects lane's cache and loader are read-only
//! to this lane, while `model_load`, `object_program` and `object_program_gpu`
//! provide the model, material and stream implementation.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use render_bridge::native_objects::{NativeSet, SetPart};
use map::TagId;
use native_render::gpu::Image;
use native_render::light_gpu::StaticLightTexture;
use native_render::load;
use native_render::look;
use native_render::model_load;
use native_render::object_lighting::ShaderValues;
use native_render::object_nodes::{Compression, decompression_rows};
use native_render::object_program_gpu::{
    EngineStages, LitBuild, LitMaterial, LitModelInput, LitObjects, LitSection, lit_model, uniform,
};
use native_render::object_streams::{self, StreamData, StreamFormats};
use tags::model::RenderModelGeometry;

use crate::native_map::{self, NativeMap};
use crate::native_world_glue::{Raw, StageDecoder, StageImages, StagePalettes};

struct CpuModel {
    nodes: usize,
    lit: LitModelInput,
    normalization: Option<Arc<StaticLightTexture>>,
}

#[derive(Default)]
pub struct Models {
    retire: OnceLock<native_map::RetireQueue>,
    pub lit: Mutex<Option<LitObjects>>,
    /// Upload-side sharing tables, separate from the render-side lock.
    uploads: Mutex<Option<LitObjects>>,
    pub palettes: Mutex<StagePalettes>,
}

impl Drop for Models {
    fn drop(&mut self) {
        if let Some(send) = self.retire.take() {
            native_map::retire(send, timeline::setup::Stage::Objects, self);
        }
    }
}

pub struct MapState {
    pub name: String,
    pub generation: u64,
    pub map_generation: u64,
    pub models: Arc<Models>,
    pub set: Arc<NativeSet>,
    pub next: Option<(u64, Arc<NativeSet>)>,
    pub tried: HashSet<u16>,
    /// The definition each requested model was asked for by, by index: a
    /// live edit of its `mode` asks the worker for it again under the
    /// same key.
    pub requested: HashMap<u16, u32>,
    pub worker: Option<mpsc::Sender<u32>>,
}

impl MapState {
    pub fn retire(mut self) {
        // Wake the idle worker before queueing the model payload.
        drop(self.worker.take());
        native_map::retire_owned(timeline::setup::Stage::Objects, self);
    }

    pub fn activate(&mut self, frame: u64) {
        if self.next.as_ref().is_some_and(|(finished, _)| frame > *finished)
            && let Some((_, set)) = self.next.take()
        {
            self.set = set;
        }
    }
}

/// Starts this map generation's loader. The shared state owns the request
/// sender, so the worker holds only a weak reference to it.
pub fn spawn_worker(
    mut set: Arc<NativeSet>,
    models: Weak<Models>,
    state: Weak<Mutex<Option<MapState>>>,
    palettes: StagePalettes,
    formats: Option<StreamFormats>,
    (device, queue): (wgpu::Device, wgpu::Queue),
    map_generation: u64,
) -> Option<mpsc::Sender<u32>> {
    let (tx, rx) = mpsc::channel::<u32>();
    background::spawn("native-first-person load", move || {
        let name = set.map.clone();
        let worker_generation = set.generation;
        let current = || {
            map_generation == render_bridge::native_map_work::generation()
                && state.upgrade().is_some_and(|state| {
                    state.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
                        .is_some_and(|state| state.generation == worker_generation)
                })
        };
        let (mut loaded, mut refused) = (0u32, 0u32);
        while let Ok(first) = rx.recv() {
            let Some(_load) = native_map::begin_load(timeline::setup::Stage::Objects, current) else {
                return;
            };
            let Some(models) = models.upgrade() else { return };
            let _timer = timeline::setup::Stage::Objects.worker();
            let _scope = mem_watch::scope("native.first_person.load");
            if models.retire.get().is_none() {
                let Ok(retire) = native_map::retire_queue() else { return };
                let _ = models.retire.set(retire);
            }
            // Use one map and decoded image cache for queued models. Both
            // belong to this batch and are released before the worker sleeps.
            let mut opened = match open(&name, palettes.clone(), formats.clone()) {
                Ok(opened) => opened,
                Err(e) => {
                    eprintln!("native-first-person: {name}: map not opened ({e}); first person not drawn");
                    return;
                }
            };
            let mut uploads = models.uploads.lock().unwrap_or_else(|e| e.into_inner());
            let uploads = uploads.get_or_insert_with(|| LitObjects::new(&device));
            models.lit.lock().unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(|| uploads.empty_with_layout());
            let mut asked = HashSet::new();
            for definition in std::iter::once(first).chain(rx.try_iter()) {
                if !current() {
                    return;
                }
                // A model asked for again is a live edit's reload
                // (docs/editor/realtime-edits.md, "The native refresh"): the
                // map is opened again, so the geometry is read as the raw
                // overlay serves it now.
                if !asked.insert(definition) {
                    match open(&name, palettes.clone(), formats.clone()) {
                        Ok(reopened) => opened = reopened,
                        Err(e) => {
                            eprintln!("native-first-person: {name}: a live edit's map does not open ({e}); the model keeps its loaded geometry");
                            continue;
                        }
                    }
                }
                let result = load_model(&mut opened, definition).and_then(|cpu| {
                    let parts = set_parts(&cpu.lit);
                    if let Some(texture) = cpu.normalization {
                        uploads.set_normalization(&device, &queue, &texture);
                    }
                    if !uploads.add_model(&device, &queue, definition, cpu.lit) {
                        return Err("GPU refused the model's programs or textures".into());
                    }
                    Ok((cpu.nodes, parts))
                });
                // Flush writes even if a later program in the upload failed.
                queue.submit([]);
                match result {
                    Ok((nodes, parts)) => {
                        let part_count = parts.len();
                        let next = Arc::make_mut(&mut set);
                        next.definitions.insert(definition as u16, nodes);
                        next.parts.retain(|&(model, _, _), _| model != definition as u16);
                        next.parts.extend(parts.into_iter().map(|((section, part), native)| {
                            ((definition as u16, section, part), native)
                        }));
                        {
                            let mut used = models.palettes.lock().unwrap_or_else(|e| e.into_inner());
                            for (saved, decoded) in used.iter_mut().zip(opened.decoder.used()) {
                                if decoded.is_some() {
                                    saved.clone_from(decoded);
                                }
                            }
                        }
                        {
                            let mut lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
                            let Some(lit) = lit.as_mut() else { return };
                            if !uploads.move_model_into(definition, lit) {
                                return;
                            }
                        }
                        {
                            let Some(state) = state.upgrade() else { return };
                            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                            let Some(current) = st.as_mut().filter(|s| {
                                s.generation == set.generation && s.map_generation == render_bridge::native_map_work::generation()
                            }) else { return };
                            current.next = Some((timeline::frames(), Arc::clone(&set)));
                        }
                        loaded += 1;
                        if hooks::native_verbose() {
                            eprintln!(
                                "native-first-person: {name}: mode {definition:#010x}: {part_count} lit parts, {nodes} nodes ({loaded} models native, {refused} left out)"
                            );
                        }
                    }
                    Err(why) => {
                        refused += 1;
                        if hooks::native_verbose() {
                            eprintln!("native-first-person: {name}: definition {definition:#010x} left out ({why})");
                        }
                    }
                }
            }
        }
        eprintln!("native-first-person: {name}: {loaded} models native, {refused} left out");
    }).ok()?;
    Some(tx)
}

/// Applies the images of one drain of live edits to `models`, the first-person
/// models of map `name` loaded at map generation `map_generation`
/// (docs/editor/realtime-edits.md, "The native refresh"): each image is
/// decoded again, from the map file the raw overlay serves edited, at the
/// stages that sample it (through the palettes the load expanded
/// through), and its textures are rewritten in place. An image whose size,
/// levels or format changed keeps its pixels until the map reloads, and
/// says so.
pub fn refresh_images(
    name: &str, map_generation: u64, models: &Models, images: &[(TagId, u32)], queue: &wgpu::Queue,
) {
    let current = || map_generation == render_bridge::native_map_work::generation();
    if images.is_empty() || !current() {
        return;
    }
    let (mut map, files) = match native_map::open(name) {
        Ok(opened) => opened,
        Err(e) => {
            eprintln!("native-first-person: {name}: a live edit's map does not open ({e}); the models keep their pixels");
            return;
        }
    };
    let mut raw = Raw(files);
    let palettes = models.palettes.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // Which stages sample each image, read under the lock; the decodes
    // run without it.
    let stages: Vec<((TagId, u32), Vec<usize>)> = {
        let lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lit) = lit.as_ref() else { return };
        images.iter().map(|&(bitmap, image)| ((bitmap, image), lit.image_stages(bitmap, image))).collect()
    };
    let decoded: Vec<_> = stages
        .into_iter()
        .filter(|(_, at)| !at.is_empty())
        .map(|((bitmap, image), at)| {
            let pixels = native_render::stage_images::decode_stages(&mut map, &mut raw.0, &palettes, bitmap, image, &at);
            ((bitmap, image), pixels)
        })
        .collect();
    if decoded.is_empty() || !current() {
        return;
    }
    {
        let lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lit) = lit.as_ref() else { return };
        for ((bitmap, image), pixels) in &decoded {
            if let Err(why) = lit.rewrite_image(queue, *bitmap, *image, pixels) {
                eprintln!(
                    "native-first-person: {name}: bitmap {:#010x} image {image} keeps its pixels until the map reloads ({why:?})",
                    bitmap.0
                );
            }
        }
    }
    queue.submit([]);
}

/// The native set's entries by the section and geometry part a record names.
fn set_parts(lit: &LitModelInput) -> Vec<((u16, u8), SetPart)> {
    lit.parts
        .iter()
        .map(|part| {
            let mut native = SetPart {
                shader: part.shader_tag as u16,
                ..SetPart::default()
            };
            for (implementation, slot) in part.implementations.iter().enumerate() {
                let Some((program, _)) = slot else {
                    continue;
                };
                native.implementations |= 1 << implementation;
                if lit
                    .programs
                    .get(*program)
                    .is_some_and(|program| program.pass.texaccum.contains(&true))
                {
                    native.texaccum |= 1 << implementation;
                }
            }
            ((part.section, part.part), native)
        })
        .collect()
}

struct Opened {
    map: NativeMap,
    raw: Raw,
    decoder: StageDecoder,
    images: StageImages,
    metadata: tags::object::Metadata,
    lib: tags::material::ShaderLibrary,
    normalization: Option<Option<Arc<StaticLightTexture>>>,
    formats: Option<StreamFormats>,
    lightmap: Option<Result<Image, String>>,
}

fn open(
    name: &str,
    palettes: StagePalettes,
    formats: Option<StreamFormats>,
) -> Result<Opened, load::Error> {
    let (mut map, files) = native_map::open(name)?;
    let metadata = tags::object::Metadata::read(&mut map)?;
    let lib = tags::material::ShaderLibrary::with_metadata(&map, metadata.shader_view());
    Ok(Opened {
        map,
        raw: Raw(files),
        decoder: StageDecoder::new(palettes),
        images: StageImages::new(),
        metadata,
        lib,
        normalization: None,
        formats,
        lightmap: None,
    })
}

fn material(opened: &mut Opened, shader: TagId) -> Option<tags::material::Material> {
    opened.lib.material(shader, None).ok()
}

fn lit_material(opened: &mut Opened, shader: TagId) -> Result<LitMaterial, String> {
    let material = material(opened, shader).ok_or("the material does not resolve")?;
    if opened.formats.is_none() {
        return Err("the game's stream formats do not read".into());
    }
    let lib = &opened.lib;
    let address = opened
        .map
        .tag(shader)
        .ok()
        .and_then(|tag| tag.location)
        .map(|location| location.address)
        .ok_or("its shader tag has no address")?;
    let values = lib
        .metadata()
        .bytes(address + SHADER_VALUES, 8)
        .map_err(|_| "its shader values do not read")?;
    let word = |offset: usize| {
        u32::from_le_bytes([
            values[offset],
            values[offset + 1],
            values[offset + 2],
            values[offset + 3],
        ])
    };
    Ok(LitMaterial {
        shader: ShaderValues {
            scalar_21: word(0),
            vector_24: word(4),
        },
        material,
        properties: native_render::layer_pass::instance_properties(lib, shader),
    })
}

const SHADER_VALUES: u32 = 0x44;

/// Loads one complete record model. Every section is built: the record key,
/// not the owner definition's default variant, selects the first-person
/// permutation and LOD. Each part publishes only the passes it compiles.
fn load_model(opened: &mut Opened, definition: u32) -> Result<CpuModel, load::Error> {
    let mode = native_map::tag_of(&opened.map, definition).ok_or("no such mode tag")?;
    let geometry = RenderModelGeometry::read_in(
        &mut opened.map, mode, &mut opened.raw, Some(&opened.metadata),
    )?;
    let nodes = geometry.nodes.len();
    if geometry.nodes.iter().any(|node| model_load::inverse_bind(&node.wire.unknown_028[..]).is_none()) {
        return Err("a node's inverse rest frame is not finite".into());
    }
    let input = compiled_parts(lit_parts(opened, &geometry))?;
    let normalization = opened.normalization.clone().flatten();
    Ok(CpuModel {
        nodes,
        lit: input,
        normalization,
    })
}

/// A layer-3 refusal names one material. Other layers of that material
/// and compiled siblings have independent entries in the GPU input.
fn compiled_parts(build: LitBuild) -> Result<LitModelInput, load::Error> {
    if !build.input.is_empty() {
        return Ok(build.input);
    }
    let why = build.refused.into_iter().next()
        .map(|(local, why)| format!("material {local}: {why}"))
        .unwrap_or_else(|| "no first-person part has a native pass".into());
    Err(why.into())
}

fn lit_parts(
    opened: &mut Opened,
    geo: &RenderModelGeometry,
) -> LitBuild {
    let mut sections = Vec::new();
    let mut used = BTreeSet::new();
    for (section_index, section) in geo.sections.iter().enumerate() {
        let flags = section.wire.compression_flags.0;
        let bounds = geo.compression.as_ref();
        let compression = Compression {
            position: bounds.filter(|_| flags & 1 != 0).map(|c| c.position_bounds),
            texcoord: bounds.filter(|_| flags & 2 != 0).map(|c| c.texcoord_bounds),
        };
        let geometry = &section.geometry;
        let slots = geometry
            .vertex_buffers
            .iter()
            .map(|b| b.slot + 1)
            .max()
            .unwrap_or(0);
        let mut streams = vec![None; slots];
        for buffer in &geometry.vertex_buffers {
            streams[buffer.slot] = Some((
                buffer.type_id,
                StreamData {
                    bytes: geometry.data(buffer),
                    stride: usize::from(buffer.stride),
                },
            ));
        }
        let parts: Vec<_> = geometry.parts.iter().enumerate()
            .filter(|(_, part)| part.is_drawn() && part.kind != load::PART_TRANSPARENT)
            .map(|(index, part)| (index as u8, part.material, geometry.part_triangles(part)))
            .collect();
        let vertex_count = section.vertices.positions.len();
        used.extend(parts.iter().filter_map(|(_, material, triangles)| {
            triangles.iter().any(|[a, b, c]| {
                a != b && b != c && a != c
                    && [a, b, c].into_iter().all(|index| usize::from(*index) < vertex_count)
            }).then_some(*material)
        }));
        sections.push(LitSection {
            index: section_index as u16,
            geometry: section.wire.geometry_classification.0,
            influences: section.wire.unknown_014,
            decompression: decompression_rows(&compression),
            streams,
            vertex_count: geometry.vertex_count(),
            parts,
            // A record with bit 6 may draw a choice of them
            // (`frame_scene::Subparts`).
            subparts: geometry
                .parts
                .iter()
                .enumerate()
                .filter(|(_, part)| part.is_drawn() && part.kind != load::PART_TRANSPARENT)
                .filter_map(|(index, part)| {
                    Some((
                        index as u8,
                        native_render::object_program_gpu::lit_subparts(&geometry.indices, &geometry.subparts, part)?,
                    ))
                })
                .collect(),
            // A record without bit 9 draws a strip part as a list.
            listed: geometry
                .parts
                .iter()
                .enumerate()
                .filter(|(_, part)| part.is_drawn() && part.kind != load::PART_TRANSPARENT)
                .filter_map(|(index, part)| {
                    Some((index as u8, native_render::object_program_gpu::lit_listed(&geometry.indices, part)?))
                })
                .collect(),
        });
    }

    let materials: HashMap<_, _> = used.into_iter().filter_map(|local| {
        let shader = geo.materials.get(usize::from(local))?.shader.id;
        lit_material(opened, shader).ok().map(|material| (local, material))
    }).collect();

    let mut programs: HashMap<(TagId, u16, u8), Result<(Vec<u8>, Vec<u16>), String>> =
        HashMap::new();
    for lit in materials.values() {
        let vertex_shaders = lit
            .material
            .layers
            .iter()
            .filter(|layer| (layer.lod == 0 && layer.layer == look::LIGHTMAP_LAYER) || crate::native_objects_glue::drawn_layer(layer.layer))
            .flat_map(|layer| &layer.passes)
            .flat_map(|pass| &pass.implementations)
            .filter_map(|implementation| implementation.vertex_shader);
        for shader in vertex_shaders {
            for section in &sections {
                let key = (shader, section.geometry, section.influences);
                programs.entry(key).or_insert_with(|| {
                    object_vertex_program(opened, shader, section.geometry, section.influences)
                });
            }
        }
    }
    if opened.normalization.is_none() {
        let cube = crate::native_lights_glue::light_textures(
            &mut opened.map, opened.lib.metadata(), &mut opened.raw.0,
        ).0;
        opened.normalization = Some(cube);
    }
    if opened.lightmap.is_none() {
        opened.lightmap = Some(object_lightmap(opened));
    }
    let Some(formats) = opened.formats.clone() else {
        return LitBuild {
            refused: materials
                .keys()
                .map(|&local| (local, "the game's stream formats do not read".to_string()))
                .collect(),
            ..LitBuild::default()
        };
    };
    let mut vertex = |shader: TagId, geometry: u16, influences: u8| {
        programs
            .get(&(shader, geometry, influences))
            .cloned()
            .unwrap_or_else(|| Err("its vertex program was not read".into()))
    };
    let mut image = |stage: usize, bitmap: TagId, image: u32| -> Option<Image> {
        opened
            .decoder
            .decode(
                &mut opened.map,
                &mut opened.raw.0,
                &mut opened.images,
                bitmap,
                image,
                stage,
                true,
            )
            .ok()?;
        opened.images.get(&(bitmap, image)).cloned()
    };
    // The first person's layers draw under no sorted handler: nothing
    // copies the view for them.
    let engine = EngineStages {
        texaccum: false,
        view: false,
        scene_copy: false,
        normalization: opened.normalization.as_ref().is_some_and(Option::is_some),
        lightmap: opened.lightmap.clone().and_then(Result::ok),
    };
    lit_model(
        &sections,
        &materials,
        &formats,
        &engine,
        &mut vertex,
        &mut image,
    )
}

fn object_lightmap(opened: &mut Opened) -> Result<Image, String> {
    let lib = &opened.lib;
    let matg = opened
        .map
        .tags()
        .iter()
        .find(|tag| tag.group.0 == *b"matg")
        .and_then(|tag| tag.location.as_ref().map(|location| location.address))
        .ok_or("no matg")?;
    let read = |address: u32, count: usize| {
        lib.metadata()
            .bytes(address, count)
            .ok()
            .map(<[u8]>::to_vec)
    };
    let bitmap = crate::native_lights_glue::rasterizer_bitmap(
        &read,
        matg,
        native_render::object_program::OBJECT_LIGHTMAP,
    )
    .ok_or("the globals' rasterizer data names no lightmap bitmap")?;
    opened.decoder.decode(
        &mut opened.map,
        &mut opened.raw.0,
        &mut opened.images,
        bitmap,
        0,
        3,
        false,
    )?;
    let image = opened
        .images
        .get(&(bitmap, 0))
        .cloned()
        .ok_or("its image does not decode")?;
    if !uniform(&image) {
        return Err("the object's lightmap is not uniform".into());
    }
    Ok(image)
}

fn object_vertex_program(
    opened: &mut Opened,
    shader: TagId,
    geometry: u16,
    influences: u8,
) -> Result<(Vec<u8>, Vec<u16>), String> {
    let address = opened
        .map
        .tag(shader)
        .ok()
        .and_then(|tag| tag.location)
        .map(|location| location.address)
        .ok_or("the vertex shader tag has no address")?;
    let lib = &opened.lib;
    let metadata = lib.metadata();
    let count = look::classification_count(metadata, address)
        .ok_or("the vertex shader's classifications do not read")?;
    let class = object_streams::classification(geometry, influences, count)
        .ok_or_else(|| format!("geometry classification {geometry}"))?;
    let code = look::vertex_program(metadata, address, class)
        .ok_or_else(|| format!("classification {class} has no program"))?;
    let semantics = look::vertex_semantics(metadata, address, class)
        .ok_or_else(|| format!("classification {class}'s inputs do not read"))?;
    Ok((code, semantics))
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    #[test]
    fn a_layer_three_refusal_keeps_the_other_compiled_layers() {
        use native_render::object_program_gpu::{LitLayerPart, LitLayerPass};
        let mut build = LitBuild::default();
        build.refused.push((4, "no layer-3 pass".into()));
        build.input.layers.push(LitLayerPart {
            layer: 8,
            level: 0,
            section: 2,
            part: 3,
            decompression: [[0.0; 4]; 3],
            shader_tag: 7,
            shader: ShaderValues::default(),
            indices: 0..3,
            subparts: None,
            listed: None,
            passes: vec![LitLayerPass { count: 1, empty: 1, ..Default::default() }],
        });
        let parts = compiled_parts(build).unwrap();
        assert!(parts.parts.is_empty());
        assert_eq!((parts.layers[0].layer, parts.layers[0].part), (8, 3));
        assert!(compiled_parts(LitBuild::default()).is_err());
    }

    #[test]
    fn the_previous_model_set_draws_until_the_next_swap() {
        let mut state = MapState {
            name: "arena".into(),
            generation: 2,
            map_generation: 1,
            models: Arc::new(Models::default()),
            set: Arc::new(NativeSet::default()),
            next: None,
            tried: HashSet::new(),
            requested: HashMap::new(),
            worker: None,
        };
        Arc::make_mut(&mut state.set).definitions.insert(4, 2);
        let mut next = (*state.set).clone();
        next.definitions.insert(7, 3);
        state.next = Some((8, Arc::new(next)));
        state.activate(8);
        assert_eq!(state.set.node_count(4), Some(2));
        assert_eq!(state.set.node_count(7), None);
        state.activate(9);
        assert_eq!(state.set.node_count(4), Some(2));
        assert_eq!(state.set.node_count(7), Some(3));
    }
}
