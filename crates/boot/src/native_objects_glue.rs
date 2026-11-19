//! The native objects' host (docs/graphics/native-renderer.md, stage 2):
//! render models loaded per map on a worker, as the objects that use them
//! first reach layer 3, and one external pass per view drawing the objects
//! the feature skipped (`feature::native_objects`). Inert unless
//! `impl.render.native_objects` is on (nothing asks).
//!
//! - **Load once per map.** On a new map a worker opens the loaded map
//!   through the VFS (the world's locator, `native_world_glue::locate`)
//!   with its shared maps and the object tag context. Each definition the
//!   feature meets is sent once; the worker resolves its model
//!   (`native_render::model_load`) and compiles its opaque materials'
//!   programs (below), dropping the materials that refuse (their items
//!   keep their D3D draws). The model is queued for upload and its
//!   definition and shaders are published in the native set. A refused
//!   model (a run-time permutation, a node mismatch, nothing opaque) is
//!   logged once and its objects stay on the D3D path.
//! - **Per view.** The device gets an `ObjectsPass` with the camera and the
//!   objects' node matrices; on the render thread it uploads the queued
//!   models, packs and culls the objects, and draws them skinned.
//! - **Opaque materials** (docs/graphics/native-renderer.md, "Object
//!   programs"). Every opaque material, whether or not its layer-3 pass
//!   reads the object's lighting ("lit" parts in the names here), is drawn
//!   through the game's own vertex program and pixel shader, per
//!   implementation of its layer-3 pass that compiles (a record picks one
//!   by its `+0` bits 29..31; an object whose records pick one some lit
//!   part lacks stays on D3D), its section's streams laid out as the game's
//!   declaration (the format table read from guest memory when the map is
//!   first seen) and its per-object constants (vertex and combiner) put in
//!   per draw. A material none of whose implementations compiles for every
//!   section and stage stays on D3D whole, with the reasons; never drawn
//!   with the base-map stand-in.
//! - **Transparent parts** (stage 3, with `impl.render.native_transparents`
//!   on too). The worker translates the layer-15 pass over each
//!   section's guest streams. The transparents host uses the record's
//!   model, section, part and node rows to draw only that entry.

use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};

use engine::memory::GuestMemory;
use feature::native_objects::{Host, NativeSet};
use map::{Map, ResourceFile, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::{DepthRule, Image, PassViewport};
use native_render::light_gpu::LayeredImage;
use native_render::instances::ObjectInput;
use native_render::load;
use native_render::model::Model;
use native_render::model_gpu::GpuModels;
use native_render::model_load;
use native_render::look;
use native_render::object_lighting::ShaderValues;
use native_render::object_nodes::{Compression, decompression_rows};
use native_render::object_program_gpu::{EngineStages, LitBuild, LitMaterial, LitModelInput, LitObjects, LitSection, Texaccum, lit_model, uniform};
use native_render::object_streams::{self, StreamData, StreamFormats};
use native_render::objects::ObjectFrame;
use native_render::program::ViewInputs;
use native_render::transparent::Entry;

use crate::native_world_glue::{Raw, StageDecoder, StageImages, StagePalettes, locate, palettes_hold, region, stage_palettes};

/// A model loaded on the worker, waiting for the render thread.
struct CpuModel {
    definition: u32,
    model: Model,
    base_maps: HashMap<u16, Image>,
    /// Layer-15 parts translated over their own raw declarations.
    transparents: LitModelInput,
    /// The parts drawn through the game's programs.
    lit: LitModelInput,
    /// The implementations (bit `n`) every lit part has.
    lit_implementations: u8,
    /// The normalization cube the lit parts' programs may sample.
    normalization: Option<Arc<LayeredImage>>,
}

/// One definition's supported layer-15 parts, by section and part index.
#[derive(Default)]
struct ObjectTransparents {
    model: u16,
    nodes: usize,
    parts: HashMap<(u16, u8), ObjectTransparentPart>,
}

struct ObjectTransparentPart {
    shader: u16,
    implementation: usize,
    needs_lighting_root: bool,
}

impl ObjectTransparents {
    fn implementation(&self, frame: &ObjectFrame, model: u32, section: u16, entry: &Entry) -> Option<usize> {
        let part = self.parts.get(&(section, entry.part))?;
        (model as u16 == self.model
            && frame.nodes.len() == self.nodes
            && entry.shader as u16 == part.shader
            && (!part.needs_lighting_root || frame.lighting.as_ref().and_then(|l| l.root).is_some()))
        .then_some(part.implementation)
    }
}

/// One map's models: queued, uploaded, and which definition is which.
#[derive(Default)]
pub(crate) struct Models {
    pending: Mutex<Vec<CpuModel>>,
    gpu: Mutex<Option<GpuModels>>,
    /// The lit parts on the GPU, by uploaded model (locked after `gpu`).
    lit: Mutex<Option<LitObjects>>,
    /// Definition tag index -> uploaded model.
    pub(crate) index: Mutex<HashMap<u16, u32>>,
    /// Definition tag index -> its transparent parts, published after the
    /// model is queued.
    transparents: Mutex<HashMap<u16, ObjectTransparents>>,
    pub(crate) transparent_gpu: Mutex<Option<LitObjects>>,
    /// The device palettes the loads expanded palettized base maps
    /// through, at their stages ([`StageDecoder::used`]): the models draw
    /// only while those stay bound.
    palettes: Mutex<StagePalettes>,
    /// Definitions whose lit parts sample the texture accumulation target:
    /// a view drawing one asks the device for that level.
    texaccum: Mutex<HashSet<u16>>,
}

impl Models {
    pub(crate) fn transparent_implementation(&self, o: &ObjectFrame, model: u32, section: u16, entry: &Entry) -> Option<usize> {
        self.transparents.lock().unwrap_or_else(|e| e.into_inner())
            .get(&(o.definition as u16))?.implementation(o, model, section, entry)
    }

    pub(crate) fn transparent_palettes_hold(&self, bound: &dyn Fn(usize) -> Option<Vec<u32>>) -> bool {
        palettes_hold(&self.palettes.lock().unwrap_or_else(|e| e.into_inner()), bound)
    }
}

/// The installed host's state, for the transparents' host.
static CURRENT: Mutex<Option<Arc<Mutex<Option<MapState>>>>> = Mutex::new(None);

/// The current map's models (`None` before the first view of a map, or
/// without the host).
pub(crate) fn current_models() -> Option<Arc<Models>> {
    let state = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.as_ref().map(|s| Arc::clone(&s.models))
}

pub(crate) fn want_transparent(mem: &dyn GuestMemory, definition: u32) -> Option<Arc<Models>> {
    let name = crate::health::loaded_map_name(mem).ok()?;
    let state = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    let map = st.as_mut().filter(|s| s.name == name)?;
    want_model(map, definition);
    Some(Arc::clone(&map.models))
}

fn want_model(map: &mut MapState, definition: u32) {
    if map.tried.insert(definition as u16)
        && let Some(worker) = &map.worker
    {
        let _ = worker.send(definition);
    }
}

/// Uploads the queued models (and their transparent materials) on the
/// render thread; the models' GPU side, locked.
pub(crate) fn upload_pending<'a>(models: &'a Models, t: &d3d::ExternalTarget<'_>) -> MutexGuard<'a, Option<GpuModels>> {
    let mut guard = models.gpu.lock().unwrap_or_else(|e| e.into_inner());
    let gpu = guard.get_or_insert_with(|| GpuModels::new(t.device, t.queue));
    let pending = std::mem::take(&mut *models.pending.lock().unwrap_or_else(|e| e.into_inner()));
    let mut index = models.index.lock().unwrap_or_else(|e| e.into_inner());
    let mut lit_gpu = models.lit.lock().unwrap_or_else(|e| e.into_inner());
    let mut transparent_gpu = models.transparent_gpu.lock().unwrap_or_else(|e| e.into_inner());
    for cpu in pending {
        let CpuModel {
            definition,
            model,
            mut base_maps,
            transparents,
            lit,
            lit_implementations: _,
            normalization,
        } = cpu;
        let i = gpu.add_model(t.device, t.queue, model, &mut |m| base_maps.remove(&m));
        if !lit.is_empty() {
            let l = lit_gpu.get_or_insert_with(|| LitObjects::new(t.device));
            if let Some(n) = &normalization {
                l.set_normalization(t.device, t.queue, n);
            }
            l.add_model(t.device, t.queue, i, lit);
        }
        if !transparents.is_empty() {
            transparent_gpu.get_or_insert_with(|| LitObjects::new(t.device))
                .add_model(t.device, t.queue, i, transparents);
        }
        index.insert(definition as u16, i);
    }
    drop(transparent_gpu);
    drop(lit_gpu);
    drop(index);
    guard
}

struct MapState {
    name: String,
    models: Arc<Models>,
    set: Arc<NativeSet>,
    tried: HashSet<u16>,
    worker: Option<mpsc::Sender<u32>>,
}

/// The host: the device and the current map's models.
pub struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: Arc<Mutex<Option<MapState>>>,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn native(&self, mem: &dyn GuestMemory) -> Option<Arc<NativeSet>> {
        let name = crate::health::loaded_map_name(mem).ok()?;
        if name.is_empty() {
            return None;
        }
        let current = |st: &Option<MapState>| st.as_ref().map(|s| s.name.as_str()) == Some(name.as_str());
        let loaded = current(&*self.state.lock().unwrap_or_else(|e| e.into_inner()));
        if !loaded {
            // The palettes bound now, for palettized base maps (P8, P8
            // bump), which the D3D path samples through the device palette
            // at their stage. Read with no other lock held, as the world's.
            let palettes = self.device.lock().ok().map(|d| stage_palettes(&d, mem)).unwrap_or_default();
            // The stream formats the game's declarations are built from,
            // read where the game keeps them (`None`: lit materials stay
            // on D3D).
            let formats = StreamFormats::read(&Ro(mem));
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current(&*st) {
                let models = Arc::new(Models::default());
                let worker = spawn_worker(name.clone(), Arc::clone(&models), Arc::clone(&self.state), palettes, formats);
                let set = NativeSet { map: name.clone(), ..NativeSet::default() };
                *st = Some(MapState {
                    name,
                    models,
                    set: Arc::new(set),
                    tried: HashSet::new(),
                    worker,
                });
            }
        }
        let (set, models) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let s = st.as_ref()?;
            (Arc::clone(&s.set), Arc::clone(&s.models))
        };
        // A base map expanded through a palette that is no longer bound
        // would sample other colours than the D3D path: then it draws
        // every object.
        let used = models.palettes.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if used.iter().any(Option::is_some) {
            let now = self.device.lock().ok().map(|d| stage_palettes(&d, mem))?;
            if !palettes_hold(&used, &|s| now[s].clone()) {
                if !PALETTE_CHANGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("native-objects: a stage palette changed since a base map was expanded through it; the D3D path draws the objects");
                }
                return None;
            }
        }
        Some(set)
    }

    fn want(&self, definition: u32) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = st.as_mut() {
            want_model(s, definition);
        }
    }

    fn draw(&self, camera: &RenderCamera, objects: Vec<ObjectFrame>) -> bool {
        let Some(models) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|s| Arc::clone(&s.models))
        else {
            return false;
        };
        let Ok(mut d) = self.device.lock() else {
            return false;
        };
        if !d.can_external_pass() {
            return false;
        }
        // An object whose lit parts sample the texture accumulation target
        // reads the level the D3D path drew in layer 1, in place: without
        // that the D3D path draws the view's objects.
        let texaccum = {
            let t = models.texaccum.lock().unwrap_or_else(|e| e.into_inner());
            objects.iter().any(|o| t.contains(&(o.definition as u16)))
        };
        if texaccum && !d.samples_levels_in_place() {
            return false;
        }
        d.request_external_pass(Box::new(ObjectsPass {
            models,
            camera: *camera,
            objects,
            view: None,
            texaccum,
            w_clip: None,
        }));
        true
    }
}

static PALETTE_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Guest memory, read only.
struct Ro<'a>(&'a dyn GuestMemory);

impl native_render::camera::GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// Installs the host on the device (beside the world's).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let state = Arc::new(Mutex::new(None));
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&state));
    feature::native_objects::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
    }));
}

/// The map's worker: opens the map once, then loads each definition sent.
/// `None` when the thread cannot start (every object stays on D3D).
fn spawn_worker(
    name: String,
    models: Arc<Models>,
    state: Arc<Mutex<Option<MapState>>>,
    palettes: StagePalettes,
    formats: Option<StreamFormats>,
) -> Option<mpsc::Sender<u32>> {
    let (tx, rx) = mpsc::channel::<u32>();
    std::thread::Builder::new()
        .name("native-objects load".into())
        .spawn(move || {
            let mut opened = match open(&name, palettes, formats) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("native-objects: {name}: map not opened ({e}); the D3D path draws every object");
                    return;
                }
            };
            let (mut loaded, mut refused) = (0u32, 0u32);
            while let Ok(def) = rx.recv() {
                if excepted(def) {
                    refused += 1;
                    eprintln!("native-objects: {name}: definition {def:#010x} left to the D3D path (H2_NATIVE_OBJECTS_EXCEPT)");
                    continue;
                }
                let t0 = std::time::Instant::now();
                let result = load_model(&mut opened, def);
                // Before the model is queued: a pass that draws it checks
                // the palettes its base maps went through.
                models.palettes.lock().unwrap_or_else(|e| e.into_inner()).clone_from(opened.decoder.used());
                match result {
                    Ok((cpu, shaders, transparents)) => {
                        let nodes = cpu.model.inverse_binds.len();
                        let draws = cpu.model.draws.len();
                        let see_through = cpu.transparents.parts.len();
                        let lit = cpu.lit.parts.len();
                        let lit_implementations = cpu.lit_implementations;
                        let texaccum = cpu.lit.samples_texaccum();
                        let lit_sections: HashSet<u16> = cpu.lit.parts.iter().map(|p| p.section).collect();
                        if texaccum {
                            models.texaccum.lock().unwrap_or_else(|e| e.into_inner()).insert(def as u16);
                        }
                        models.pending.lock().unwrap_or_else(|e| e.into_inner()).push(cpu);
                        if !transparents.parts.is_empty() {
                            models
                                .transparents
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(def as u16, transparents);
                        }
                        // Published after the model is queued: a pass that
                        // sees the definition finds the model.
                        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(s) = st.as_mut().filter(|s| s.name == name) else { return };
                        let mut set = (*s.set).clone();
                        set.definitions.insert(def as u16, nodes);
                        set.shaders.extend(shaders.into_iter().map(|s| (def as u16, s)));
                        if lit > 0 {
                            set.lit.insert(def as u16, lit_implementations);
                        }
                        if texaccum {
                            set.texaccum.insert(def as u16);
                        }
                        set.sections.extend(lit_sections.iter().map(|&s| (def as u16, s)));
                        s.set = Arc::new(set);
                        // No lock held across the print.
                        drop(st);
                        loaded += 1;
                        eprintln!(
                            "native-objects: {name}: definition {def:#010x}: {draws} draws, {lit} lit parts, {see_through} transparent, {nodes} nodes, in {:.0} ms ({loaded} models native, {refused} left to D3D)",
                            t0.elapsed().as_secs_f32() * 1000.0
                        );
                    }
                    Err(e) => {
                        refused += 1;
                        eprintln!("native-objects: {name}: definition {def:#010x} left to the D3D path ({e})");
                    }
                }
            }
        })
        .ok()?;
    Some(tx)
}

/// The map open for model loads.
struct Opened {
    map: Map<BufReader<std::fs::File>>,
    ctx: tags::object::Context,
    raw: Raw,
    /// Base maps as their stages sample them (palettized ones expanded
    /// through the stage's palette), shared by every model of the map.
    decoder: StageDecoder,
    images: StageImages,
    /// The shader tags, read at the first transparent material (`None`
    /// when they do not read: transparents stay on D3D).
    lib: Option<Option<tags::material::ShaderLibrary>>,
    /// The vector normalization cube, loaded as the lights load it at the
    /// first lit model (`Some(None)`: it does not load).
    normalization: Option<Option<Arc<LayeredImage>>>,
    /// The game's stream formats (`None`: lit materials stay on D3D).
    formats: Option<StreamFormats>,
    transparents_on: bool,
    /// What an object's draw samples for the lightmap (the globals'
    /// rasterizer bitmap at `+0x64`, `object_program::OBJECT_LIGHTMAP`),
    /// read at the first lit model: the image when uniform, else why not.
    lightmap: Option<Result<Image, String>>,
}

fn open(name: &str, palettes: StagePalettes, formats: Option<StreamFormats>) -> Result<Opened, load::Error> {
    let file = locate(name).ok_or("the map's file is not found")?;
    let mut map = Map::from_region(BufReader::new(std::fs::File::open(&file.path)?), file.offset, file.len)?;
    let mut files = assets::ResourceFiles::new();
    files.set(ResourceFile::Local, region(&file))?;
    for (which, short) in [
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
        (ResourceFile::SinglePlayerShared, "single_player_shared"),
    ] {
        if let Some(f) = locate(short) {
            files.set(which, region(&f))?;
        }
    }
    let ctx = tags::object::Context::read(&mut map)?;
    Ok(Opened {
        map,
        ctx,
        raw: Raw(files),
        decoder: StageDecoder::new(palettes),
        images: StageImages::new(),
        lib: None,
        normalization: None,
        lightmap: None,
        formats,
        transparents_on: feature::Switches::from_env().is_ok_and(|s| feature::native_transparents::on(&s)),
    })
}

/// The map's tag whose index is the datum's low word (the guest's datum
/// and the map's agree on the index; the salt is not relied on).
fn tag_of(map: &Map<BufReader<std::fs::File>>, datum: u32) -> Option<TagId> {
    if let Ok(t) = map.tag(TagId(datum)) {
        return Some(t.id);
    }
    map.tags().iter().find(|t| t.id.0 & 0xffff == datum & 0xffff).map(|t| t.id)
}

/// `shader` resolved, through the map's shader library (read once).
fn material(o: &mut Opened, shader: TagId) -> Option<tags::material::Material> {
    if o.lib.is_none() {
        let lib = tags::material::ShaderLibrary::read(&mut o.map);
        if let Err(e) = &lib {
            eprintln!("native-objects: the shader tags do not read ({e}); every object stays on D3D");
        }
        o.lib = Some(lib.ok());
    }
    o.lib.as_mut()?.as_mut()?.material(shader, None).ok()
}

/// Why an opaque material stays on D3D: no layer-3 implementation reads
/// only what the object path gives (`object_program::program_refusal`),
/// or it does not resolve. One that does is drawn through its programs
/// ([`lit_material`]) or refused there; never with a stand-in.
fn program_refusal(o: &mut Opened, shader: TagId) -> Option<String> {
    let Some(m) = material(o, shader) else {
        return Some("the material does not resolve".into());
    };
    native_render::object_program::program_refusal(&m)
}

/// Prepares a definition's supported layer-3 and layer-15 programs and
/// the geometry each replacement may skip.
fn load_model(o: &mut Opened, def: u32) -> Result<(CpuModel, Vec<u16>, ObjectTransparents), load::Error> {
    let id = tag_of(&o.map, def).ok_or("no such tag")?;
    let loaded = model_load::load_object_model(&mut o.map, &o.ctx, &mut o.raw, id)?;
    // Every opaque material through its own programs, or left to D3D: the
    // base-map stand-in is not used for opaque parts (it is neither the
    // game's shading nor, for an animated object, the pose its records'
    // rows give: the scarab of 03a's intro).
    let base_maps = HashMap::new();
    let mut shaders = Vec::new();
    let mut unlit = Vec::new();
    let mut lit_materials = HashMap::new();
    for m in loaded.model.materials() {
        let Some(&shader) = loaded.shaders.get(usize::from(m)) else { continue };
        if let Some(why) = program_refusal(o, shader) {
            unlit.push(format!("{:#010x} {why}", shader.0));
            continue;
        }
        match lit_material(o, shader) {
            Ok(lit) => {
                lit_materials.insert(m, lit);
            }
            Err(why) => unlit.push(format!("{:#010x} {why}", shader.0)),
        }
    }
    let mut table = ObjectTransparents {
        model: loaded.mode.0 as u16,
        nodes: loaded.model.inverse_binds.len(),
        ..Default::default()
    };
    let transparents = if o.transparents_on {
        let materials: HashMap<_, _> = loaded.lod_sections.iter().filter_map(|&s| loaded.geometry.sections.get(s))
            .flat_map(|s| &s.geometry.parts)
            .filter(|p| p.is_drawn() && p.kind == load::PART_TRANSPARENT)
            .map(|p| p.material)
            .collect::<HashSet<_>>()
            .into_iter()
            .filter_map(|m| {
                let &shader = loaded.shaders.get(usize::from(m))?;
                Some((m, lit_material(o, shader).ok()?))
            })
            .collect();
        let built = lit_parts(o, &loaded, &materials, true);
        for (m, why) in built.refused {
            eprintln!("native-transparents: object definition {def:#010x} material {m} left to D3D ({why})");
        }
        for part in &built.input.parts {
            if let Some((n, (program, _))) = part.implementations.iter().enumerate().find_map(|(n, p)| p.map(|p| (n, p))) {
                let imp = &built.input.programs[program].pass.implementation;
                // These are the lighting sources uniform preparation can
                // fail to evaluate after the implementation has compiled.
                let needs_lighting_root = imp.vector_bindings.iter().any(|b| b.source == 40)
                    || imp.scalar_bindings.iter().any(|b| b.source == 37);
                table.parts.insert((part.section, part.part), ObjectTransparentPart {
                    shader: part.shader_tag as u16,
                    implementation: n,
                    needs_lighting_root,
                });
            }
        }
        built.input
    } else {
        LitModelInput::default()
    };
    let (lit, lit_implementations) = if lit_materials.is_empty() {
        (LitModelInput::default(), 0)
    } else {
        let LitBuild { input, taken, refused, implementations } = lit_parts(o, &loaded, &lit_materials, false);
        for (m, why) in refused {
            unlit.push(format!("{:#010x} {why}", loaded.shaders[usize::from(m)].0));
        }
        shaders.extend(taken.iter().map(|&m| loaded.shaders[usize::from(m)].0 as u16));
        (input, implementations)
    };
    let model = keep_materials(loaded.model, &base_maps, &HashSet::new());
    if model.draws.is_empty() && lit.is_empty() && transparents.is_empty() {
        if !unlit.is_empty() {
            return Err(format!("its lit materials stay on D3D: {}", unlit.join(", ")).into());
        }
        return Err("no material with a base map".into());
    }
    Ok((
        CpuModel {
            definition: def,
            model,
            base_maps,
            transparents,
            normalization: if lit.is_empty() { None } else { o.normalization.clone().flatten() },
            lit,
            lit_implementations,
        },
        shaders,
        table,
    ))
}

/// What an opaque material's programs draw with: the material and its
/// shader's values (`shad +0x44`, `+0x48`), or why they do not read.
fn lit_material(o: &mut Opened, shader: TagId) -> Result<LitMaterial, String> {
    let material = material(o, shader).ok_or("the material does not resolve")?;
    let read = || -> Result<LitMaterial, String> {
        if o.formats.is_none() {
            return Err("the game's stream formats do not read".into());
        }
        let lib = o.lib.as_ref().and_then(Option::as_ref).ok_or("the shader tags do not read")?;
        let address = o.map.tag(shader).ok().and_then(|t| t.location).map(|l| l.address).ok_or("its shader tag has no address")?;
        let values = lib.metadata().bytes(address + SHADER_VALUES, 8).map_err(|_| "its shader values do not read")?;
        let word = |k: usize| u32::from_le_bytes([values[k], values[k + 1], values[k + 2], values[k + 3]]);
        Ok(LitMaterial {
            shader: ShaderValues { scalar_21: word(0), vector_24: word(4) },
            material: material.clone(),
        })
    };
    read()
}

/// The shader instance's values the object inputs scale by (`shad +0x44`
/// scalar 21, `+0x48`).
const SHADER_VALUES: u32 = 0x44;

/// The lit parts of `loaded`'s drawn sections for `materials`
/// ([`lit_model`]): its vertex programs per section classification read
/// from the shader tags, its stage bitmaps through the stage decoder.
fn lit_parts(o: &mut Opened, loaded: &model_load::LoadedModel, materials: &HashMap<u16, LitMaterial>, transparent: bool) -> LitBuild {
    if materials.is_empty() {
        return LitBuild::default();
    }
    let layer = if transparent { look::TRANSPARENT_LAYER } else { look::LIGHTMAP_LAYER };
    let geo = &loaded.geometry;
    let mut sections = Vec::new();
    // Every detail level: a distant object's records draw a lower one.
    for &si in &loaded.lod_sections {
        let Some(s) = geo.sections.get(si) else { continue };
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
        sections.push(LitSection {
            index: si as u16,
            geometry: s.wire.geometry_classification.0,
            influences: s.wire.unknown_014,
            decompression: decompression_rows(&compression),
            streams,
            vertex_count: g.vertex_count(),
            parts: g
                .parts
                .iter()
                .enumerate()
                .filter(|(_, p)| p.is_drawn() && (p.kind == load::PART_TRANSPARENT) == transparent)
                .filter_map(|(i, p)| Some((u8::try_from(i).ok()?, p.material, g.part_triangles(p))))
                .collect(),
        });
    }
    // The programs each section's classification takes, read first (the
    // stage decoder borrows the map after).
    let mut programs: HashMap<(TagId, u16, u8), Result<(Vec<u8>, Vec<u16>), String>> = HashMap::new();
    for lit in materials.values() {
        let vertex_shaders = lit
            .material
            .layers
            .iter()
            .filter(|l| l.lod == 0 && l.layer == layer)
            .flat_map(|l| &l.passes)
            .flat_map(|p| &p.implementations)
            .filter_map(|i| i.vertex_shader);
        for vs in vertex_shaders {
            for s in &sections {
                let key = (vs, s.geometry, s.influences);
                if !programs.contains_key(&key) {
                    let p = object_vertex_program(o, vs, s.geometry, s.influences);
                    programs.insert(key, p);
                }
            }
        }
    }
    // The normalization cube, as the lights load it (once per map).
    if !transparent && o.normalization.is_none() {
        let cube = match o.lib.as_ref().and_then(Option::as_ref) {
            Some(lib) => crate::native_lights_glue::light_textures(&mut o.map, lib.metadata(), &mut o.raw.0).0.map(Arc::new),
            None => None,
        };
        o.normalization = Some(cube);
    }
    if !transparent && o.lightmap.is_none() {
        let l = object_lightmap(o);
        match &l {
            Ok(i) => eprintln!("native-objects: the objects' lightmap stage loads ({}x{}, uniform)", i.width, i.height),
            Err(why) => eprintln!("native-objects: the objects' lightmap stage stays on D3D: {why}"),
        }
        o.lightmap = Some(l);
    }
    let Some(formats) = o.formats.clone() else {
        return LitBuild {
            refused: materials.keys().map(|&m| (m, "the game's stream formats do not read".to_string())).collect(),
            ..LitBuild::default()
        };
    };
    let mut vertex = |vs: TagId, geometry: u16, influences: u8| {
        programs.get(&(vs, geometry, influences)).cloned().unwrap_or_else(|| Err("its vertex program was not read".into()))
    };
    let mut image = |stage: usize, bitmap: TagId, image: u32| -> Option<Image> {
        o.decoder.decode(&mut o.map, &mut o.raw.0, &mut o.images, bitmap, image, stage, true).ok()?;
        o.images.get(&(bitmap, image)).cloned()
    };
    if transparent {
        return native_render::object_program_gpu::transparent_model(&sections, materials, &formats, &mut vertex, &mut image);
    }
    let engine = EngineStages {
        texaccum: true,
        normalization: o.normalization.as_ref().is_some_and(Option::is_some),
        lightmap: o.lightmap.clone().and_then(Result::ok),
    };
    lit_model(&sections, materials, &formats, &engine, &mut vertex, &mut image)
}

/// What an object's layer-3 draw samples where its pass reads the
/// lightmap (`native_render::object_program::OBJECT_LIGHTMAP`): the
/// globals' rasterizer bitmap at `+0x64`, decoded as stage 3 samples it,
/// when it is uniform (then the coordinates the draw adds do not matter).
fn object_lightmap(o: &mut Opened) -> Result<Image, String> {
    let lib = o.lib.as_ref().and_then(Option::as_ref).ok_or("the shader tags do not read")?;
    let matg = o
        .map
        .tags()
        .iter()
        .find(|t| t.group.0 == *b"matg")
        .and_then(|t| t.location.as_ref().map(|l| l.address))
        .ok_or("no matg")?;
    let read = |a: u32, n: usize| lib.metadata().bytes(a, n).ok().map(<[u8]>::to_vec);
    let bitmap = crate::native_lights_glue::rasterizer_bitmap(&read, matg, native_render::object_program::OBJECT_LIGHTMAP)
        .ok_or("the globals' rasterizer data names no bitmap at +0x64")?;
    o.decoder.decode(&mut o.map, &mut o.raw.0, &mut o.images, bitmap, 0, 3, false)?;
    let image = o.images.get(&(bitmap, 0)).cloned().ok_or("its image does not decode")?;
    if !uniform(&image) {
        return Err(format!("the globals' rasterizer bitmap {bitmap:?} ({}x{}) is not uniform, so the coordinates the draw adds (the BSP's stream 0x2e) would matter", image.width, image.height));
    }
    Ok(image)
}

/// Vertex shader `vs`'s program and input semantics for a section of
/// geometry classification `geometry` and influences `influences`, as
/// `0x0001cb20` picks its classification.
fn object_vertex_program(o: &mut Opened, vs: TagId, geometry: u16, influences: u8) -> Result<(Vec<u8>, Vec<u16>), String> {
    let address = o.map.tag(vs).ok().and_then(|t| t.location).map(|l| l.address).ok_or("the vertex shader tag has no address")?;
    let lib = o.lib.as_ref().and_then(Option::as_ref).ok_or("the shader tags do not read")?;
    let meta = lib.metadata();
    let count = look::classification_count(meta, address).ok_or("the vertex shader's classifications do not read")?;
    let class = object_streams::classification(geometry, influences, count).ok_or_else(|| format!("geometry classification {geometry}"))?;
    let code = look::vertex_program(meta, address, class).ok_or_else(|| format!("classification {class} has no program"))?;
    let semantics = look::vertex_semantics(meta, address, class).ok_or_else(|| format!("classification {class}'s inputs do not read"))?;
    Ok((code, semantics))
}

/// The model without the draws of materials left to the D3D path (opaque
/// ones without a base map, transparent ones not taken).
fn keep_materials(mut model: Model, native: &HashMap<u16, Image>, transparent: &HashSet<u16>) -> Model {
    model.draws.retain(|d| native.contains_key(&d.material));
    model.transparents.retain(|d| transparent.contains(&d.material));
    model
}

/// One view's objects.
struct ObjectsPass {
    models: Arc<Models>,
    camera: RenderCamera,
    objects: Vec<ObjectFrame>,
    /// The view's constants, for the lit parts' programs.
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    /// Whether a drawn object's lit parts sample the texture accumulation
    /// target ([`native_render::object_program::TEXACCUM_HEADER`]).
    texaccum: bool,
    /// The device's W-buffer clip at the pass (`d3d::DepthClip`), as the
    /// world's: the range and clamp the D3D path draws the layer with.
    /// Without it the programs clipped w at 0, not at the view's near
    /// (03a: 256 depth units), and drew the part of an object nearer
    /// than that, which the D3D path culls.
    w_clip: Option<([f32; 2], bool)>,
}

impl d3d::ExternalPass for ObjectsPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = Some((clip.range, clip.clamp));
    }

    fn sampled(&self) -> Vec<u32> {
        if self.texaccum { vec![native_render::object_program::TEXACCUM_HEADER] } else { Vec::new() }
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut guard = upload_pending(&self.models, &t);
        let Some(gpu) = guard.as_mut() else { return };
        let index = self.models.index.lock().unwrap_or_else(|e| e.into_inner());
        let inputs: Vec<ObjectInput<'_>> = self
            .objects
            .iter()
            .filter_map(|o| {
                Some(ObjectInput {
                    model: *index.get(&(o.definition as u16))?,
                    nodes: &o.nodes,
                    sphere: o.sphere,
                })
            })
            .collect();
        gpu.prepare(
            t.device,
            t.queue,
            &self.camera,
            DepthRule {
                full_scale: t.z_scale,
                floor: true,
            },
            &inputs,
        );
        let [x, y, width, height] = t.viewport;
        gpu.encode(
            t.device,
            &mut *t.encoder,
            t.color,
            t.color_format,
            t.depth,
            t.depth_format,
            PassViewport { x, y, width, height },
        );
        // The lit parts, through the game's programs (the viewport the
        // whole target, as they expect).
        let Some(constants) = self.view else { return };
        let mut lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lit) = lit.as_mut() else { return };
        let objects: Vec<(u32, &ObjectFrame)> = self
            .objects
            .iter()
            .filter_map(|o| {
                let i = *index.get(&(o.definition as u16))?;
                lit.has(i).then_some((i, o))
            })
            .collect();
        let probing = probe_at().is_some() && probe_due();
        if let Some(at) = probe_at()
            && probing
        {
            let vp = self.camera.view_projection();
            let size = (t.guest_size.0 as f32, t.guest_size.1 as f32);
            let texaccum = self.models.texaccum.lock().unwrap_or_else(|e| e.into_inner());
            let mut hits = 0;
            for o in &self.objects {
                let Some(cover) = sphere_covers(&vp, o.sphere, size, at) else { continue };
                hits += 1;
                let model = index.get(&(o.definition as u16)).copied();
                eprintln!(
                    "native-objects probe at {at:?}: definition {:#010x} handle {:#010x} sphere {:?} (centre {:.1} px off, radius {:.1} px, w {:.2}); its records draw (section, implementation) {:?}, its native parts come from sections {:?}{}{}",
                    o.definition,
                    o.handle,
                    o.sphere,
                    cover.0,
                    cover.1,
                    cover.2,
                    o.implementations,
                    model.map(|m| lit.sections(m)).unwrap_or_default(),
                    if model.is_some_and(|m| lit.has(m)) { "" } else { "; no lit model (not drawn by the programs)" },
                    if texaccum.contains(&(o.definition as u16)) { "; samples the texture accumulation target" } else { "" },
                );
            }
            eprintln!(
                "native-objects probe at {at:?}: {hits} of {} objects' spheres cover it; texture accumulation level {:?} (guest {:?}, copy {:?})",
                self.objects.len(),
                t.sampled.first().map(|l| l.as_ref().map(|l| l.size)),
                t.guest_size,
                t.size
            );
        }
        if objects.is_empty() {
            return;
        }
        let texaccum = texaccum_level(t.sampled, t.guest_size, t.size);
        if self.texaccum && texaccum.is_none() && TEXACCUM_MISSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
            eprintln!(
                "native-objects: the texture accumulation target was bound at the hook but the device gave no level for it; parts sampling it go undrawn this view (logged once, counted)"
            );
        }
        let view = self.view_inputs(constants, &t);
        if let Some(at) = probe_at()
            && probing
        {
            let vp = self.camera.view_projection();
            let size = (t.guest_size.0 as f32, t.guest_size.1 as f32);
            for &(model, o) in &objects {
                if sphere_covers(&vp, o.sphere, size, at).is_none() {
                    continue;
                }
                for line in lit.describe_parts(model, o, &view, texaccum) {
                    eprintln!("native-objects probe at {at:?}: definition {:#010x} handle {:#010x}: {line}", o.definition, o.handle);
                }
            }
        }
        lit.encode(
            t.device,
            t.queue,
            &mut *t.encoder,
            t.color,
            t.color_format,
            t.depth,
            t.depth_format,
            t.size,
            &view,
            &objects,
            texaccum,
        );
    }
}

/// `H2_NATIVE_OBJECTS_PROBE=x,y` (guest pixels): on the first passes and
/// every 250th after, each object of the pass whose bounding sphere covers
/// that pixel, with the sections its records draw, the sections its
/// native parts come from, and the texture accumulation level's size.
fn probe_at() -> Option<(f32, f32)> {
    static AT: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
    *AT.get_or_init(|| {
        let at = std::env::var("H2_NATIVE_OBJECTS_PROBE").ok().and_then(|v| parse_probe(&v));
        if let Some(at) = at {
            eprintln!("native-objects: probing guest pixel {at:?}");
        }
        at
    })
}

impl ObjectsPass {
    /// The view the lit parts' programs draw with: the device's constants,
    /// the target's sizes, and its W-buffer clip.
    fn view_inputs(&self, constants: [[f32; 4]; d3d::VIEW_CONSTANTS], t: &d3d::ExternalTarget<'_>) -> ViewInputs {
        ViewInputs {
            constants,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
            w_clip: self.w_clip,
        }
    }
}

/// `H2_NATIVE_OBJECTS_EXCEPT=0xe92207ad,...`: definitions (tag datums,
/// or their low words) left to the D3D path, to tell which object a
/// difference belongs to.
fn excepted(definition: u32) -> bool {
    static EXCEPT: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    EXCEPT
        .get_or_init(|| std::env::var("H2_NATIVE_OBJECTS_EXCEPT").map(|v| parse_definitions(&v)).unwrap_or_default())
        .contains(&(definition as u16))
}

/// A comma-separated list of definitions, hex (`0x` optional) datums or
/// tag indices: their tag indices.
fn parse_definitions(v: &str) -> Vec<u16> {
    v.split(',')
        .filter_map(|d| {
            let d = d.trim();
            let d = d.strip_prefix("0x").or_else(|| d.strip_prefix("0X")).unwrap_or(d);
            u32::from_str_radix(d, 16).ok().map(|x| x as u16)
        })
        .collect()
}

fn probe_due() -> bool {
    static PROBED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = PROBED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    n < 8 || n % 250 == 0
}

fn parse_probe(v: &str) -> Option<(f32, f32)> {
    let (x, y) = v.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// Whether `sphere` (centre, radius), seen through `view_proj` on a
/// screen of `size` pixels, covers pixel `at`: the centre's distance in
/// pixels, the projected radius (the farthest of its six axis points from
/// the centre on screen) and the centre's view depth `w`. A camera inside
/// or at the sphere covers every pixel (radius infinite).
fn sphere_covers(view_proj: &native_render::camera::Mat4, sphere: ([f32; 3], f32), size: (f32, f32), at: (f32, f32)) -> Option<(f32, f32, f32)> {
    let (c, r) = sphere;
    let clip = |p: [f32; 3]| native_render::camera::transform(view_proj, [p[0], p[1], p[2], 1.0]);
    let screen = |v: [f32; 4]| ((v[0] / v[3] + 1.0) * 0.5 * size.0, (1.0 - v[1] / v[3]) * 0.5 * size.1);
    let centre = clip(c);
    let (px, py) = (at.0 + 0.5, at.1 + 0.5);
    if centre[3] <= r {
        return Some((0.0, f32::INFINITY, centre[3]));
    }
    let (sx, sy) = screen(centre);
    let mut radius = 0f32;
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let mut p = c;
            p[axis] += sign * r;
            let v = clip(p);
            if v[3] <= 0.0 {
                return Some((0.0, f32::INFINITY, centre[3]));
            }
            let (x, y) = screen(v);
            radius = radius.max(((x - sx).powi(2) + (y - sy).powi(2)).sqrt());
        }
    }
    let distance = ((px - sx).powi(2) + (py - sy).powi(2)).sqrt();
    (distance <= radius).then_some((distance, radius, centre[3]))
}

/// Views whose texture accumulation level the device did not give though
/// a drawn object's parts sample it (bound at the hook,
/// `object_nodes::texaccum_bound`, yet no level over it after the drain):
/// those parts go undrawn. Counted, and logged once.
static TEXACCUM_MISSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The texture accumulation level the pass was given, with its size in
/// guest texels (this copy's texels over the colour's scale).
fn texaccum_level(sampled: &[Option<d3d::SampledLevel>], guest_size: (u32, u32), size: (u32, u32)) -> Option<Texaccum<'_>> {
    let level = sampled.first()?.as_ref()?;
    Some(Texaccum { view: &level.view, guest_size: guest_texels(level.size, guest_size, size) })
}

/// `texels` of a level in a copy whose colour target is `size` texels for
/// `guest` guest pixels, in guest texels.
fn guest_texels(texels: (u32, u32), guest: (u32, u32), size: (u32, u32)) -> (u32, u32) {
    let scale = |t: u32, g: u32, s: u32| (u64::from(t) * u64::from(g) / u64::from(s.max(1))) as u32;
    (scale(texels.0, guest.0, size.0), scale(texels.1, guest.1, size.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::model::{ModelDraw, NodeMatrix};

    /// The lit parts are W-buffered programs as the world's: they clip w
    /// to the range the device draws the layer with, not from 0, or an
    /// object nearer than the view's near is drawn where the D3D path
    /// culls it.
    #[test]
    fn the_lit_parts_clip_w_as_the_device_draws_the_layer() {
        use d3d::ExternalPass;
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            viewport: [0, 0, 480, 640],
            near: 0.01,
            far: 100.0,
        };
        let mut pass = ObjectsPass {
            models: Arc::new(Models::default()),
            camera,
            objects: Vec::new(),
            view: None,
            texaccum: false,
            w_clip: None,
        };
        pass.depth_clip(d3d::DepthClip { range: [256.0, 16_777_215.0], clamp: false, w_buffered: Some(true) });
        assert_eq!(pass.w_clip, Some(([256.0, 16_777_215.0], false)));
        let constants = [[0.0; 4]; d3d::VIEW_CONSTANTS];
        let view = ViewInputs { constants, guest_size: (640, 480), size: (640, 480), z_scale: 16_777_215.0, w_clip: pass.w_clip };
        let u = native_render::program::frame_vertex(&Default::default(), &view);
        assert_eq!(u.w_clip, Some([256.0, 16_777_215.0]), "near at the device's 256, not 0");
    }

    #[test]
    fn excepted_definitions_parse_as_tag_indices() {
        assert_eq!(parse_definitions("0xe92207ad, e703058e,zz,0X07FF"), vec![0x07ad, 0x058e, 0x07ff]);
        assert!(parse_definitions("").is_empty());
    }

    #[test]
    fn the_probe_names_the_objects_whose_sphere_covers_its_pixel() {
        assert_eq!(parse_probe(" 575, 410"), Some((575.0, 410.0)));
        assert_eq!(parse_probe("575"), None);
        // Identity but w = 2: x and y over 2 across the screen.
        let mut vp = [[0.0f32; 4]; 4];
        for (i, c) in vp.iter_mut().enumerate() {
            c[i] = 1.0;
        }
        vp[3][3] = 2.0;
        let size = (640.0, 480.0);
        // Centre at (320, 240); radius 0.2 over w 2: 32 px across, 24 down.
        let (d, r, w) = sphere_covers(&vp, ([0.0, 0.0, 0.0], 0.2), size, (330.0, 240.0)).expect("covered");
        assert!((d - 10.5).abs() < 0.1 && (r - 32.0).abs() < 0.1 && w == 2.0, "{d} {r} {w}");
        assert_eq!(sphere_covers(&vp, ([0.0, 0.0, 0.0], 0.2), size, (400.0, 240.0)), None, "outside");
        assert_eq!(sphere_covers(&vp, ([0.0, 0.0, 0.0], 3.0), size, (0.0, 0.0)).map(|c| c.1), Some(f32::INFINITY), "the camera inside");
    }

    #[test]
    fn a_levels_texels_under_the_render_scale_are_its_guest_size() {
        assert_eq!(guest_texels((640, 480), (640, 480), (640, 480)), (640, 480), "1x");
        assert_eq!(guest_texels((1280, 960), (640, 480), (1280, 960)), (640, 480), "2x");
        assert_eq!(guest_texels((320, 240), (640, 480), (1280, 960)), (160, 120), "a smaller level, scaled");
    }

    #[test]
    fn materials_left_to_the_d3d_path_leave_the_model() {
        let draw = |material, first_index| ModelDraw {
            material,
            first_index,
            index_count: 3,
        };
        let model = Model {
            draws: vec![draw(0, 0), draw(1, 3), draw(1, 6), draw(2, 9)],
            transparents: vec![draw(3, 12), draw(4, 15)],
            inverse_binds: vec![NodeMatrix::IDENTITY],
            ..Model::default()
        };
        let native: HashMap<u16, Image> = [(1, Image::solid([1; 4])), (2, Image::solid([1; 4]))].into_iter().collect();
        let m = keep_materials(model, &native, &[4].into_iter().collect());
        assert_eq!(m.draws.iter().map(|d| (d.material, d.first_index)).collect::<Vec<_>>(), vec![(1, 3), (1, 6), (2, 9)]);
        assert_eq!(m.materials(), vec![1, 2]);
        assert_eq!(m.transparent_materials(), vec![4]);
    }

    #[test]
    fn an_objects_transparent_entry_matches_its_exact_section_part_and_shader() {
        let t = ObjectTransparents {
            model: 12,
            nodes: 2,
            parts: [
                ((3, 1), ObjectTransparentPart { shader: 0x42, implementation: 0, needs_lighting_root: false }),
                ((5, 1), ObjectTransparentPart { shader: 0x42, implementation: 1, needs_lighting_root: true }),
            ].into_iter().collect(),
        };
        let mut frame = ObjectFrame {
            handle: 1, definition: 2, nodes: vec![native_render::model::NodeMatrix::IDENTITY; 2],
            sphere: ([0.0; 3], 1.0), lighting: None, defaults: None, node_rows: Vec::new(), implementations: Vec::new(),
        };
        let mut entry = Entry {
            source: native_render::transparent::Source::Object(1),
            record: 4, part: 1, shader: 0xe123_0042, depth: 0.0, position: [0.0; 3],
        };
        assert_eq!(t.implementation(&frame, 12, 3, &entry), Some(0));
        assert_eq!(t.implementation(&frame, 13, 3, &entry), None, "another render model");
        assert_eq!(t.implementation(&frame, 12, 4, &entry), None, "another LOD");
        assert_eq!(t.implementation(&frame, 12, 5, &entry), None, "lighting missing");
        entry.part = 2;
        assert_eq!(t.implementation(&frame, 12, 3, &entry), None, "another part of the same shader");
        entry.part = 1;
        entry.shader += 1;
        assert_eq!(t.implementation(&frame, 12, 3, &entry), None, "another shader");
        frame.nodes.pop();
        entry.shader -= 1;
        assert_eq!(t.implementation(&frame, 12, 3, &entry), None, "another node count");
    }
}
