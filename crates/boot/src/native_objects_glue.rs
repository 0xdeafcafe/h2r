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
//!   with the base-map stand-in (that is for transparent parts only).
//! - **Transparent parts** (stage 3, with `impl.render.native_transparents`
//!   on too). The worker also takes each transparent material with a
//!   single layer-15 pass, no alpha test and a base map that decodes (S:
//!   drawn as the base map with the models' lighting and the pass's blend);
//!   the definition's node count and its shaders' materials are published
//!   in [`Models::transparents`] for the transparents' host
//!   (`native_transparents_glue`), which draws them run by run.

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
use native_render::object_program_gpu::{EngineStages, LitBuild, LitMaterial, LitModelInput, LitObjects, LitSection, lit_model, uniform};
use native_render::object_streams::{self, StreamData, StreamFormats};
use native_render::objects::ObjectFrame;
use native_render::pass_state::PassState;
use native_render::program::ViewInputs;

use crate::native_world_glue::{Raw, StageDecoder, StageImages, StagePalettes, locate, palettes_hold, region, stage_palettes};

/// A model loaded on the worker, waiting for the render thread.
struct CpuModel {
    definition: u32,
    model: Model,
    base_maps: HashMap<u16, Image>,
    /// The transparent materials taken: base map and pass state.
    transparents: Vec<(u16, Image, PassState)>,
    /// The parts drawn through the game's programs.
    lit: LitModelInput,
    /// The implementations (bit `n`) every lit part has.
    lit_implementations: u8,
    /// The normalization cube the lit parts' programs may sample.
    normalization: Option<Arc<LayeredImage>>,
}

/// One definition's transparent parts the native pass draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ObjectTransparents {
    /// The model's node count (an object with another is not drawn).
    pub(crate) nodes: usize,
    /// Shader tag index -> the model-local materials that use it.
    pub(crate) shaders: HashMap<u16, Vec<u16>>,
}

impl ObjectTransparents {
    /// The materials an object's entry with `shader` draws, when its node
    /// count is the model's.
    pub(crate) fn materials(&self, nodes: usize, shader: u32) -> Option<&[u16]> {
        if nodes != self.nodes {
            return None;
        }
        self.shaders.get(&(shader as u16)).map(Vec::as_slice)
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
    pub(crate) transparents: Mutex<HashMap<u16, ObjectTransparents>>,
    /// The device palettes the loads expanded palettized base maps
    /// through, at their stages ([`StageDecoder::used`]): the models draw
    /// only while those stay bound.
    palettes: Mutex<StagePalettes>,
}

impl Models {
    /// The materials object `o`'s transparent entry with `shader` draws.
    pub(crate) fn transparent_materials(&self, o: &ObjectFrame, shader: u32) -> Option<Vec<u16>> {
        let t = self.transparents.lock().unwrap_or_else(|e| e.into_inner());
        t.get(&(o.definition as u16))?.materials(o.nodes.len(), shader).map(<[u16]>::to_vec)
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

/// Uploads the queued models (and their transparent materials) on the
/// render thread; the models' GPU side, locked.
pub(crate) fn upload_pending<'a>(models: &'a Models, t: &d3d::ExternalTarget<'_>) -> MutexGuard<'a, Option<GpuModels>> {
    let mut guard = models.gpu.lock().unwrap_or_else(|e| e.into_inner());
    let gpu = guard.get_or_insert_with(|| GpuModels::new(t.device, t.queue));
    let pending = std::mem::take(&mut *models.pending.lock().unwrap_or_else(|e| e.into_inner()));
    let mut index = models.index.lock().unwrap_or_else(|e| e.into_inner());
    let mut lit_gpu = models.lit.lock().unwrap_or_else(|e| e.into_inner());
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
        for (material, image, state) in transparents {
            // Checked on the worker; a refusal here leaves those parts
            // undrawn for this model.
            let _ = gpu.set_transparent(t.device, t.queue, i, material, Some(image), state);
        }
        index.insert(definition as u16, i);
    }
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
        let Some(s) = st.as_mut() else { return };
        if s.tried.insert(definition as u16)
            && let Some(w) = &s.worker
        {
            let _ = w.send(definition);
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
        d.request_external_pass(Box::new(ObjectsPass {
            models,
            camera: *camera,
            objects,
            view: None,
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
                let t0 = std::time::Instant::now();
                let result = load_model(&mut opened, def);
                // Before the model is queued: a pass that draws it checks
                // the palettes its base maps went through.
                models.palettes.lock().unwrap_or_else(|e| e.into_inner()).clone_from(opened.decoder.used());
                match result {
                    Ok((cpu, shaders, transparents)) => {
                        let nodes = cpu.model.inverse_binds.len();
                        let draws = cpu.model.draws.len();
                        let see_through = cpu.model.transparents.len();
                        let lit = cpu.lit.parts.len();
                        let lit_implementations = cpu.lit_implementations;
                        models.pending.lock().unwrap_or_else(|e| e.into_inner()).push(cpu);
                        if !transparents.shaders.is_empty() {
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

/// `shader`'s base map `bitmap` (its first image) as the D3D path samples
/// it: through the shared stage decoder at the stage its material binds it
/// ([`base_map_stage`], stage 0 when no layer-3 implementation names it),
/// so a palettized one (P8, P8 bump) is expanded through that stage's
/// device palette, not drawn as its index. `None` when the stage cannot
/// have it (no palette there, or it does not decode).
fn decoded(o: &mut Opened, shader: TagId, bitmap: TagId) -> Option<Image> {
    let stage = material(o, shader).and_then(|m| base_map_stage(&m, bitmap)).unwrap_or(0);
    o.decoder
        .decode(&mut o.map, &mut o.raw.0, &mut o.images, bitmap, 0, stage, false)
        .ok()?;
    o.images.get(&(bitmap, 0)).cloned()
}

/// The stage `material`'s layer-3 passes (first level of detail) bind
/// `bitmap` at, in their first implementation that does.
fn base_map_stage(material: &tags::material::Material, bitmap: TagId) -> Option<usize> {
    let stages = material
        .layers
        .iter()
        .filter(|l| l.lod == 0 && l.layer == look::LIGHTMAP_LAYER)
        .flat_map(|l| &l.passes)
        .flat_map(|p| &p.implementations)
        .map(|i| i.textures.map(|t| match t.map(|t| t.source) {
            Some(tags::material::TextureSource::Bitmap { bitmap: b, .. }) => Some(b),
            _ => None,
        }));
    first_stage(stages, bitmap)
}

/// The first stage, over implementations' stage bitmaps in order, that
/// binds `bitmap`.
fn first_stage(mut implementations: impl Iterator<Item = [Option<TagId>; 4]>, bitmap: TagId) -> Option<usize> {
    implementations.find_map(|stages| stages.iter().position(|&s| s == Some(bitmap)))
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

/// A transparent material the models' pass can stand in for: one layer-15
/// pass without an alpha test (S), and its shader's base map.
fn transparent_material(o: &mut Opened, shader: TagId) -> Option<(Image, PassState)> {
    let material = material(o, shader)?;
    let state = match look::transparent_passes(&material).ok()?.as_slice() {
        [one] => one.state,
        _ => return None,
    };
    if state.alpha_test.is_some() {
        return None;
    }
    let bitmap = load::base_map(&mut o.map, shader)?;
    Some((decoded(o, shader, bitmap)?, state))
}

/// Loads definition `def`'s model with its base maps; the shader indices
/// it draws, and its transparent parts the native pass takes. Materials
/// without a base map leave the model.
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
    let mut transparents = Vec::new();
    let mut table = ObjectTransparents {
        nodes: loaded.model.inverse_binds.len(),
        shaders: HashMap::new(),
    };
    for m in loaded.model.transparent_materials() {
        let Some(&shader) = loaded.shaders.get(usize::from(m)) else { continue };
        if let Some((image, state)) = transparent_material(o, shader) {
            transparents.push((m, image, state));
            table.shaders.entry(shader.0 as u16).or_default().push(m);
        }
    }
    let (lit, lit_implementations) = if lit_materials.is_empty() {
        (LitModelInput::default(), 0)
    } else {
        let LitBuild { input, taken, refused, implementations } = lit_parts(o, &loaded, &lit_materials);
        for (m, why) in refused {
            unlit.push(format!("{:#010x} {why}", loaded.shaders[usize::from(m)].0));
        }
        shaders.extend(taken.iter().map(|&m| loaded.shaders[usize::from(m)].0 as u16));
        (input, implementations)
    };
    let kept: HashSet<u16> = transparents.iter().map(|t| t.0).collect();
    let model = keep_materials(loaded.model, &base_maps, &kept);
    if model.draws.is_empty() && lit.is_empty() {
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
fn lit_parts(o: &mut Opened, loaded: &model_load::LoadedModel, materials: &HashMap<u16, LitMaterial>) -> LitBuild {
    let geo = &loaded.geometry;
    let mut sections = Vec::new();
    for &si in &loaded.sections {
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
                .filter(|p| p.is_drawn() && p.kind != load::PART_TRANSPARENT)
                .map(|p| (p.material, g.part_triangles(p)))
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
            .filter(|l| l.lod == 0 && l.layer == look::LIGHTMAP_LAYER)
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
    if o.normalization.is_none() {
        let cube = match o.lib.as_ref().and_then(Option::as_ref) {
            Some(lib) => crate::native_lights_glue::light_textures(&mut o.map, lib.metadata(), &mut o.raw.0).0.map(Arc::new),
            None => None,
        };
        o.normalization = Some(cube);
    }
    if o.lightmap.is_none() {
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
    let engine = EngineStages {
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
}

impl d3d::ExternalPass for ObjectsPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
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
        if objects.is_empty() {
            return;
        }
        let view = ViewInputs {
            constants,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
            w_clip: None,
        };
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
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::model::{ModelDraw, NodeMatrix};

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
    fn an_objects_transparent_entry_names_its_shaders_materials_at_the_models_node_count() {
        let t = ObjectTransparents {
            nodes: 2,
            shaders: [(0x42, vec![3, 5])].into_iter().collect(),
        };
        assert_eq!(t.materials(2, 0xe123_0042), Some(&[3, 5][..]));
        assert_eq!(t.materials(3, 0xe123_0042), None, "another node count");
        assert_eq!(t.materials(2, 0xe123_0043), None, "another shader");
    }

    #[test]
    fn a_base_map_is_decoded_at_the_stage_its_material_binds_it() {
        // The stage picks the device palette a palettized base map (P8,
        // P8 bump) is expanded through: the first implementation that binds
        // the bitmap names it; one that binds it nowhere has none.
        let (base, detail) = (TagId(0xe000_0050), TagId(0xe000_0051));
        let lightmap_only = [None, None, None, None];
        let first = [Some(detail), None, Some(base), None];
        let second = [Some(base), None, None, None];
        assert_eq!(first_stage([lightmap_only, first, second].into_iter(), base), Some(2));
        assert_eq!(first_stage([second, first].into_iter(), base), Some(0));
        assert_eq!(first_stage([first].into_iter(), detail), Some(0));
        assert_eq!(first_stage([lightmap_only].into_iter(), base), None);
        assert_eq!(first_stage(std::iter::empty(), base), None);
    }
}
