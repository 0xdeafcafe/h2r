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
//!   (`native_render::model_load`), decodes each material's base map, and
//!   drops the draws of materials whose base map does not decode (their
//!   items keep their D3D draws). The model is queued for upload and its
//!   definition and shaders are published in the native set. A refused
//!   model (a run-time permutation, a node mismatch, nothing opaque) is
//!   logged once and its objects stay on the D3D path.
//! - **Per view.** The device gets an `ObjectsPass` with the camera and the
//!   objects' node matrices; on the render thread it uploads the queued
//!   models, packs and culls the objects, and draws them skinned.
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
use native_render::instances::ObjectInput;
use native_render::load;
use native_render::model::Model;
use native_render::model_gpu::GpuModels;
use native_render::model_load;
use native_render::look;
use native_render::objects::ObjectFrame;
use native_render::pass_state::PassState;

use crate::native_world_glue::{Raw, decode_first, locate, region};

/// A model loaded on the worker, waiting for the render thread.
struct CpuModel {
    definition: u32,
    model: Model,
    base_maps: HashMap<u16, Image>,
    /// The transparent materials taken: base map and pass state.
    transparents: Vec<(u16, Image, PassState)>,
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
    /// Definition tag index -> uploaded model.
    pub(crate) index: Mutex<HashMap<u16, u32>>,
    /// Definition tag index -> its transparent parts, published after the
    /// model is queued.
    pub(crate) transparents: Mutex<HashMap<u16, ObjectTransparents>>,
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
    for cpu in pending {
        let CpuModel {
            definition,
            model,
            mut base_maps,
            transparents,
        } = cpu;
        let i = gpu.add_model(t.device, t.queue, model, &mut |m| base_maps.remove(&m));
        for (material, image, state) in transparents {
            // Checked on the worker; a refusal here leaves those parts
            // undrawn for this model.
            let _ = gpu.set_transparent(t.device, t.queue, i, material, Some(image), state);
        }
        index.insert(definition as u16, i);
    }
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
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.as_ref().map(|s| s.name.as_str()) != Some(name.as_str()) {
            let models = Arc::new(Models::default());
            let worker = spawn_worker(name.clone(), Arc::clone(&models), Arc::clone(&self.state));
            *st = Some(MapState {
                name,
                models,
                set: Arc::new(NativeSet::default()),
                tried: HashSet::new(),
                worker,
            });
        }
        st.as_ref().map(|s| Arc::clone(&s.set))
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
        }));
        true
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
fn spawn_worker(name: String, models: Arc<Models>, state: Arc<Mutex<Option<MapState>>>) -> Option<mpsc::Sender<u32>> {
    let (tx, rx) = mpsc::channel::<u32>();
    std::thread::Builder::new()
        .name("native-objects load".into())
        .spawn(move || {
            let mut opened = match open(&name) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("native-objects: {name}: map not opened ({e}); the D3D path draws every object");
                    return;
                }
            };
            let (mut loaded, mut refused) = (0u32, 0u32);
            while let Ok(def) = rx.recv() {
                let t0 = std::time::Instant::now();
                match load_model(&mut opened, def) {
                    Ok((cpu, shaders, transparents)) => {
                        let nodes = cpu.model.inverse_binds.len();
                        let draws = cpu.model.draws.len();
                        let see_through = cpu.model.transparents.len();
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
                        set.shaders.extend(shaders);
                        s.set = Arc::new(set);
                        // No lock held across the print.
                        drop(st);
                        loaded += 1;
                        eprintln!(
                            "native-objects: {name}: definition {def:#010x}: {draws} draws, {see_through} transparent, {nodes} nodes, in {:.0} ms ({loaded} models native, {refused} left to D3D)",
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
    decoded: HashMap<TagId, Option<Image>>,
    /// The shader tags, read at the first transparent material (`None`
    /// when they do not read: transparents stay on D3D).
    lib: Option<Option<tags::material::ShaderLibrary>>,
}

fn open(name: &str) -> Result<Opened, load::Error> {
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
        decoded: HashMap::new(),
        lib: None,
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

/// `bitmap`'s first image, decoded once per map.
fn decoded(o: &mut Opened, bitmap: TagId) -> Option<Image> {
    if let Some(i) = o.decoded.get(&bitmap) {
        return i.clone();
    }
    let i = decode_first(&mut o.map, &mut o.raw.0, bitmap);
    o.decoded.insert(bitmap, i.clone());
    i
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

/// Why an opaque material stays on D3D for its lighting: its layer-3
/// pass reads the object's lighting, which the native pass does not apply
/// yet (`look::object_lighting_input`), or it does not resolve, so that
/// cannot be ruled out. Never drawn unlit.
fn lighting_refusal(o: &mut Opened, shader: TagId) -> Option<String> {
    let Some(m) = material(o, shader) else {
        return Some("the material does not resolve".into());
    };
    look::object_lighting_input(&m).map(|input| format!("reads lighting input {input:?}"))
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
    Some((decoded(o, bitmap)?, state))
}

/// Loads definition `def`'s model with its base maps; the shader indices
/// it draws, and its transparent parts the native pass takes. Materials
/// without a base map leave the model.
fn load_model(o: &mut Opened, def: u32) -> Result<(CpuModel, Vec<u16>, ObjectTransparents), load::Error> {
    let id = tag_of(&o.map, def).ok_or("no such tag")?;
    let loaded = model_load::load_object_model(&mut o.map, &o.ctx, &mut o.raw, id)?;
    let mut base_maps = HashMap::new();
    let mut shaders = Vec::new();
    let mut unlit = Vec::new();
    for m in loaded.model.materials() {
        let Some(&shader) = loaded.shaders.get(usize::from(m)) else { continue };
        if let Some(why) = lighting_refusal(o, shader) {
            unlit.push(format!("{:#010x} {why}", shader.0));
            continue;
        }
        let Some(bitmap) = load::base_map(&mut o.map, shader) else { continue };
        if let Some(image) = decoded(o, bitmap) {
            base_maps.insert(m, image);
            shaders.push(shader.0 as u16);
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
    let kept: HashSet<u16> = transparents.iter().map(|t| t.0).collect();
    let model = keep_materials(loaded.model, &base_maps, &kept);
    if model.draws.is_empty() {
        if !unlit.is_empty() {
            return Err(format!("its materials are lit per object, which the native pass does not apply yet: {}", unlit.join(", ")).into());
        }
        return Err("no material with a base map".into());
    }
    Ok((
        CpuModel {
            definition: def,
            model,
            base_maps,
            transparents,
        },
        shaders,
        table,
    ))
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
}

impl d3d::ExternalPass for ObjectsPass {
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
            t.encoder,
            t.color,
            t.color_format,
            t.depth,
            t.depth_format,
            PassViewport { x, y, width, height },
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
}
