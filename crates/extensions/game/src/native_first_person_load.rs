//! Map-local models for the native first-person pass. This deliberately
//! owns a separate cache: the objects lane's cache and loader are read-only
//! to this lane, while `model_load`, `object_program` and `object_program_gpu`
//! provide the model, material and stream implementation.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use render_bridge::native_objects::{NativeSet, SetPart};
use map::TagId;
use native_render::gpu::Image;
use native_render::light_gpu::StaticLightTexture;
use native_render::load;
use native_render::look;
use native_render::model::{Model, ModelInput, ModelSectionInput};
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
    pub worker: Option<mpsc::Sender<u32>>,
}

impl MapState {
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
    models: Arc<Models>,
    state: Weak<Mutex<Option<MapState>>>,
    palettes: StagePalettes,
    formats: Option<StreamFormats>,
    (device, queue): (wgpu::Device, wgpu::Queue),
    map_generation: u64,
) -> Option<mpsc::Sender<u32>> {
    let (tx, rx) = mpsc::channel::<u32>();
    background::spawn("native-first-person load", move || {
        if map_generation != render_bridge::native_map_work::generation() {
            return;
        }
        let name = set.map.clone();
        match native_map::retire_queue() {
            Ok(queue) => {
                let _ = models.retire.set(queue);
            }
            Err(error) => {
                eprintln!("native-first-person: {name}: could not start resource retirement ({error})");
                return;
            }
        }
        let (mut opened, mut uploads) = {
            let _timer = timeline::setup::Stage::Objects.worker();
            match open(&name, palettes, formats) {
                Ok(o) => (o, LitObjects::new(&device)),
                Err(e) => {
                    eprintln!("native-first-person: {name}: map not opened ({e}); first person stays on D3D");
                    return;
                }
            }
        };
        *models.lit.lock().unwrap_or_else(|e| e.into_inner()) = Some(uploads.empty_with_layout());
        let (mut loaded, mut refused) = (0u32, 0u32);
        while let Ok(definition) = rx.recv() {
            if map_generation != render_bridge::native_map_work::generation() {
                return;
            }
            let _timer = timeline::setup::Stage::Objects.worker();
            let result = load_model(&mut opened, definition).and_then(|cpu| {
                let parts = set_parts(&cpu.lit);
                if let Some(texture) = cpu.normalization {
                    uploads.set_normalization(&device, &queue, &texture);
                }
                uploads.add_model(&device, &queue, definition, cpu.lit);
                if !uploads.has(definition) {
                    return Err("GPU refused the model's programs or textures".into());
                }
                queue.submit([]);
                Ok((cpu.nodes, parts))
            });
            match result {
                Ok((nodes, parts)) => {
                    let part_count = parts.len();
                    let next = Arc::make_mut(&mut set);
                    next.definitions.insert(definition as u16, nodes);
                    next.parts.extend(parts.into_iter().map(|((section, part), native)| {
                        ((definition as u16, section, part), native)
                    }));
                    models.palettes.lock().unwrap_or_else(|e| e.into_inner()).clone_from(opened.decoder.used());
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
                            "native-first-person: {name}: mode {definition:#010x}: {part_count} lit parts, {nodes} nodes ({loaded} models native, {refused} left to D3D)"
                        );
                    }
                }
                Err(why) => {
                    refused += 1;
                    if hooks::native_verbose() {
                        eprintln!("native-first-person: {name}: definition {definition:#010x} left to D3D ({why})");
                    }
                }
            }
        }
        eprintln!("native-first-person: {name}: {loaded} models native, {refused} left to the D3D path");
    }).ok()?;
    Some(tx)
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
    lib: Option<Option<tags::material::ShaderLibrary>>,
    normalization: Option<Option<Arc<StaticLightTexture>>>,
    formats: Option<StreamFormats>,
    lightmap: Option<Result<Image, String>>,
}

fn open(
    name: &str,
    palettes: StagePalettes,
    formats: Option<StreamFormats>,
) -> Result<Opened, load::Error> {
    let (map, files) = native_map::open(name)?;
    Ok(Opened {
        map,
        raw: Raw(files),
        decoder: StageDecoder::new(palettes),
        images: StageImages::new(),
        lib: None,
        normalization: None,
        formats,
        lightmap: None,
    })
}

fn material(opened: &mut Opened, shader: TagId) -> Option<tags::material::Material> {
    if opened.lib.is_none() {
        let lib = tags::material::ShaderLibrary::read(&mut opened.map);
        if let Err(e) = &lib {
            eprintln!(
                "native-first-person: shader tags do not read ({e}); first person stays on D3D"
            );
        }
        opened.lib = Some(lib.ok());
    }
    opened.lib.as_mut()?.as_mut()?.material(shader, None).ok()
}

fn lit_material(opened: &mut Opened, shader: TagId) -> Result<LitMaterial, String> {
    let material = material(opened, shader).ok_or("the material does not resolve")?;
    if opened.formats.is_none() {
        return Err("the game's stream formats do not read".into());
    }
    let lib = opened
        .lib
        .as_ref()
        .and_then(Option::as_ref)
        .ok_or("the shader tags do not read")?;
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
    })
}

const SHADER_VALUES: u32 = 0x44;

/// Loads one complete record model. Every section is built: the record key,
/// not the owner definition's default variant, selects the first-person
/// permutation and LOD. Any refused material or absent program leaves the
/// whole mode on D3D.
fn load_model(opened: &mut Opened, definition: u32) -> Result<CpuModel, load::Error> {
    let mode = native_map::tag_of(&opened.map, definition).ok_or("no such mode tag")?;
    let geometry = RenderModelGeometry::read(&mut opened.map, mode, &mut opened.raw)?;
    let nodes = geometry.nodes.len();
    let inverse_binds = geometry
        .nodes
        .iter()
        .map(|node| model_load::inverse_bind(&node.wire.unknown_028[..]))
        .collect::<Option<Vec<_>>>()
        .ok_or("a node's inverse rest frame is not finite")?;
    let sections: Vec<usize> = (0..geometry.sections.len()).collect();
    let mut input = ModelInput {
        inverse_binds,
        sections: Vec::new(),
    };
    for &section_index in &sections {
        let Some(section) = geometry.sections.get(section_index) else {
            continue;
        };
        let vertices = &section.vertices;
        input.sections.push(ModelSectionInput {
            positions: vertices.positions.clone(),
            uvs: vertices.texcoords.clone(),
            normals: vertices.normals.clone(),
            node_indices: vertices.node_indices.clone(),
            node_weights: vertices.node_weights.clone(),
            node_map: section.node_map.clone(),
            parts: section
                .geometry
                .parts
                .iter()
                .filter(|part| part.is_drawn())
                .map(|part| native_render::model::ModelPartInput {
                    material: part.material,
                    transparent: part.kind == load::PART_TRANSPARENT,
                    triangles: section.geometry.part_triangles(part),
                })
                .collect(),
        });
    }
    let model = Model::build(&input);
    let shaders = geometry
        .materials
        .iter()
        .map(|material| material.shader.id)
        .collect();
    let loaded = model_load::LoadedModel {
        mode,
        model,
        shaders,
        geometry,
        lod_sections: sections.clone(),
        sections,
    };
    let mut materials = HashMap::new();
    for local in loaded.model.materials() {
        let Some(&shader) = loaded.shaders.get(usize::from(local)) else {
            return Err(format!("material {local} has no shader").into());
        };
        if let Some(why) = native_render::object_program::program_refusal(
            &material(opened, shader).ok_or("the material does not resolve")?,
        ) {
            return Err(format!("shader {:#010x}: {why}", shader.0).into());
        }
        materials.insert(
            local,
            lit_material(opened, shader)
                .map_err(|why| format!("shader {:#010x}: {why}", shader.0))?,
        );
    }
    let LitBuild {
        input,
        taken,
        refused,
        implementations: _,
    } = lit_parts(opened, &loaded, &materials);
    if !refused.is_empty() {
        let reasons = refused
            .into_iter()
            .map(|(local, why)| format!("material {local}: {why}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(reasons.into());
    }
    if input.is_empty() || taken.len() != materials.len() {
        return Err("not every opaque material has a native program".into());
    }
    if input.samples_texaccum() {
        return Err(
            "its programs sample the texture accumulation target, which this pass does not bind"
                .into(),
        );
    }
    let normalization = if input.is_empty() {
        None
    } else {
        opened.normalization.clone().flatten()
    };
    Ok(CpuModel {
        nodes,
        lit: input,
        normalization,
    })
}

fn lit_parts(
    opened: &mut Opened,
    loaded: &model_load::LoadedModel,
    materials: &HashMap<u16, LitMaterial>,
) -> LitBuild {
    let geo = &loaded.geometry;
    let mut sections = Vec::new();
    for &section_index in &loaded.lod_sections {
        let Some(section) = geo.sections.get(section_index) else {
            continue;
        };
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
        sections.push(LitSection {
            index: section_index as u16,
            geometry: section.wire.geometry_classification.0,
            influences: section.wire.unknown_014,
            decompression: decompression_rows(&compression),
            streams,
            vertex_count: geometry.vertex_count(),
            parts: geometry
                .parts
                .iter()
                .enumerate()
                .filter(|(_, part)| part.is_drawn() && part.kind != load::PART_TRANSPARENT)
                .map(|(index, part)| (index as u8, part.material, geometry.part_triangles(part)))
                .collect(),
            // First-person items draw whole parts (`plan` with no subparts).
            subparts: HashMap::new(),
        });
    }

    let mut programs: HashMap<(TagId, u16, u8), Result<(Vec<u8>, Vec<u16>), String>> =
        HashMap::new();
    for lit in materials.values() {
        let vertex_shaders = lit
            .material
            .layers
            .iter()
            .filter(|layer| layer.lod == 0 && layer.layer == look::LIGHTMAP_LAYER)
            .flat_map(|layer| &layer.passes)
            .flat_map(|pass| &pass.implementations)
            .filter_map(|implementation| implementation.vertex_shader);
        for shader in vertex_shaders {
            for section in &sections {
                let key = (shader, section.geometry, section.influences);
                if !programs.contains_key(&key) {
                    programs.insert(
                        key,
                        object_vertex_program(opened, shader, section.geometry, section.influences),
                    );
                }
            }
        }
    }
    if opened.normalization.is_none() {
        let cube = match opened.lib.as_ref().and_then(Option::as_ref) {
            Some(lib) => crate::native_lights_glue::light_textures(
                &mut opened.map,
                lib.metadata(),
                &mut opened.raw.0,
            )
            .0,
            None => None,
        };
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
    let engine = EngineStages {
        texaccum: false,
        normalization: opened.normalization.as_ref().is_some_and(Option::is_some),
        lightmap: opened.lightmap.clone().and_then(Result::ok),
    };
    lit_model(
        &sections,
        materials,
        &formats,
        &engine,
        &mut vertex,
        &mut image,
    )
}

fn object_lightmap(opened: &mut Opened) -> Result<Image, String> {
    let lib = opened
        .lib
        .as_ref()
        .and_then(Option::as_ref)
        .ok_or("the shader tags do not read")?;
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
    let lib = opened
        .lib
        .as_ref()
        .and_then(Option::as_ref)
        .ok_or("the shader tags do not read")?;
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
    fn the_previous_model_set_draws_until_the_next_swap() {
        let mut state = MapState {
            name: "arena".into(),
            generation: 2,
            map_generation: 1,
            models: Arc::new(Models::default()),
            set: Arc::new(NativeSet::default()),
            next: None,
            tried: HashSet::new(),
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
