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

use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

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
use native_render::objects::ObjectFrame;

use crate::native_world_glue::{Raw, decode_first, locate, region};

/// A model loaded on the worker, waiting for the render thread.
struct CpuModel {
    definition: u32,
    model: Model,
    base_maps: HashMap<u16, Image>,
}

/// One map's models: queued, uploaded, and which definition is which.
#[derive(Default)]
struct Models {
    pending: Mutex<Vec<CpuModel>>,
    gpu: Mutex<Option<GpuModels>>,
    /// Definition tag index -> uploaded model.
    index: Mutex<HashMap<u16, u32>>,
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
    feature::native_objects::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state: Arc::new(Mutex::new(None)),
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
                    Ok((cpu, shaders)) => {
                        let nodes = cpu.model.inverse_binds.len();
                        let draws = cpu.model.draws.len();
                        models.pending.lock().unwrap_or_else(|e| e.into_inner()).push(cpu);
                        // Published after the model is queued: a pass that
                        // sees the definition finds the model.
                        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(s) = st.as_mut().filter(|s| s.name == name) else { return };
                        let mut set = (*s.set).clone();
                        set.definitions.insert(def as u16, nodes);
                        set.shaders.extend(shaders);
                        s.set = Arc::new(set);
                        loaded += 1;
                        eprintln!(
                            "native-objects: {name}: definition {def:#010x}: {draws} draws, {nodes} nodes, in {:.0} ms ({loaded} models native, {refused} left to D3D)",
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

/// Loads definition `def`'s model with its base maps; the shader indices
/// it draws. Materials without a base map leave the model.
fn load_model(o: &mut Opened, def: u32) -> Result<(CpuModel, Vec<u16>), load::Error> {
    let id = tag_of(&o.map, def).ok_or("no such tag")?;
    let loaded = model_load::load_object_model(&mut o.map, &o.ctx, &mut o.raw, id)?;
    let mut base_maps = HashMap::new();
    let mut shaders = Vec::new();
    for m in loaded.model.materials() {
        let Some(&shader) = loaded.shaders.get(usize::from(m)) else { continue };
        let Some(bitmap) = load::base_map(&mut o.map, shader) else { continue };
        let image = match o.decoded.get(&bitmap) {
            Some(i) => i.clone(),
            None => {
                let i = decode_first(&mut o.map, &mut o.raw.0, bitmap);
                o.decoded.insert(bitmap, i.clone());
                i
            }
        };
        if let Some(image) = image {
            base_maps.insert(m, image);
            shaders.push(shader.0 as u16);
        }
    }
    let model = keep_materials(loaded.model, &base_maps);
    if model.draws.is_empty() {
        return Err("no material with a base map".into());
    }
    Ok((
        CpuModel {
            definition: def,
            model,
            base_maps,
        },
        shaders,
    ))
}

/// The model without the draws of materials left to the D3D path.
fn keep_materials(mut model: Model, native: &HashMap<u16, Image>) -> Model {
    model.draws.retain(|d| native.contains_key(&d.material));
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
        let mut guard = self.models.gpu.lock().unwrap_or_else(|e| e.into_inner());
        let gpu = guard.get_or_insert_with(|| GpuModels::new(t.device, t.queue));
        let pending = std::mem::take(&mut *self.models.pending.lock().unwrap_or_else(|e| e.into_inner()));
        let mut index = self.models.index.lock().unwrap_or_else(|e| e.into_inner());
        for cpu in pending {
            let CpuModel {
                definition,
                model,
                mut base_maps,
            } = cpu;
            let i = gpu.add_model(t.device, t.queue, model, &mut |m| base_maps.remove(&m));
            index.insert(definition as u16, i);
        }
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
            inverse_binds: vec![NodeMatrix::IDENTITY],
            ..Model::default()
        };
        let native: HashMap<u16, Image> = [(1, Image::solid([1; 4])), (2, Image::solid([1; 4]))].into_iter().collect();
        let m = keep_materials(model, &native);
        assert_eq!(m.draws.iter().map(|d| (d.material, d.first_index)).collect::<Vec<_>>(), vec![(1, 3), (1, 6), (2, 9)]);
        assert_eq!(m.materials(), vec![1, 2]);
    }
}
