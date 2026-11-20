//! The native objects' host (docs/graphics/native-renderer.md, stage 2):
//! render models loaded per map on a worker, as the objects that use them
//! first reach layer 3, and one external pass per view drawing the objects
//! the feature skipped (`feature::native_objects`). Inert unless
//! `impl.render.native_objects` is on (nothing asks).
//!
//! - **Load once per map.** On a new map a worker opens the loaded map
//!   through the VFS (`native_map`)
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
//!   with a base-map stand-in.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};

use engine::memory::GuestMemory;
use feature::native_objects::{Drawn, Host, ItemMasks, NativeSet, SetPart};
use map::TagId;
use native_render::camera::RenderCamera;
use native_render::gpu::Image;
use native_render::light_gpu::StaticLightTexture;
use native_render::load;
use native_render::model::Model;
use native_render::model_gpu::GpuModels;
use native_render::model_load;
use native_render::look;
use native_render::object_lighting::ShaderValues;
use native_render::object_nodes::{Compression, decompression_rows};
use native_render::object_program_gpu::{EngineStages, LitBuild, LitMaterial, LitModelInput, LitObjects, LitSection, PlannedDraw, Texaccum, lit_model, uniform};
use native_render::object_state::EntryStates;
use native_render::object_streams::{self, StreamData, StreamFormats};
use native_render::objects::{ItemDraw, ItemKey, ObjectFrame};
use native_render::world_layers::{Geometry, Owner, Site};
use native_render::program::ViewInputs;

use crate::native_map::{self, NativeMap};
use crate::native_world_glue::{Raw, StageDecoder, StageImages, StagePalettes, palettes_hold, stage_palettes};

/// A model loaded on the worker, waiting for the render thread.
struct CpuModel {
    definition: u32,
    model: Model,
    base_maps: HashMap<u16, Image>,
    /// The parts drawn through the game's programs.
    lit: LitModelInput,
    /// The normalization cube the lit parts' programs may sample.
    normalization: Option<Arc<StaticLightTexture>>,
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
    /// The device palettes the loads expanded palettized base maps
    /// through, at their stages ([`StageDecoder::used`]): the models draw
    /// only while those stay bound.
    palettes: Mutex<StagePalettes>,
}

/// Uploads the queued models on the render thread and returns their
/// locked GPU side.
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
            lit,
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
        index.insert(definition as u16, i);
    }
    drop(lit_gpu);
    drop(index);
    guard
}

/// What one load of models belongs to: the map's name, the active BSP's
/// address and its index (as the world's key them). A view under another
/// key starts a new load (new [`Models`], a new worker); nothing queued
/// or published for one reaches another.
type MapKey = (String, u32, u16);

struct MapState {
    key: MapKey,
    /// This load's number, from [`GENERATIONS`]: the set carries it, and
    /// `Host::draw` refuses a view planned against another.
    generation: u64,
    models: Arc<Models>,
    set: Arc<NativeSet>,
    tried: HashSet<u16>,
    worker: Option<mpsc::Sender<u32>>,
}

static GENERATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The device at the layer-3 call: the render states the items' draws
/// inherit ([`EntryStates`]) and whether its draws are W-buffered.
#[derive(Clone, Debug)]
struct LayerEntry {
    states: EntryStates,
    w_buffered: bool,
}

/// The host: the device and the current map's models.
pub struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: Arc<Mutex<Option<MapState>>>,
    admission: Arc<ObjectAdmission>,
}

/// The key of the load guest memory `mem` holds now, `None` without a map
/// name or an active BSP.
fn map_key(mem: &dyn GuestMemory) -> Option<MapKey> {
    let name = crate::health::loaded_map_name(mem).ok()?;
    if name.is_empty() {
        return None;
    }
    let bsp = mem.u32(feature::native_world::ACTIVE_BSP).ok()?;
    let index = crate::native_world_glue::active_bsp_index(mem)?;
    Some((name, bsp, index))
}

/// Whether a worker loading for `models` may publish to `current`: only
/// to the load it was started for, whatever its map's name.
fn publishes_to(current: &MapState, models: &Arc<Models>) -> bool {
    Arc::ptr_eq(&current.models, models)
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn native(&self, mem: &mut dyn GuestMemory) -> Option<Arc<NativeSet>> {
        self.admission.0.lock().ok()?.take();
        if !self.device.lock().ok()?.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)).ok()? {
            return None;
        }
        let key = map_key(mem)?;
        let current = |st: &Option<MapState>| st.as_ref().map(|s| &s.key) == Some(&key);
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
                let generation = GENERATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let models = Arc::new(Models::default());
                let worker = spawn_worker(key.0.clone(), Arc::clone(&models), Arc::clone(&self.state), palettes, formats);
                let set = NativeSet { map: key.0.clone(), generation, ..NativeSet::default() };
                *st = Some(MapState {
                    key: key.clone(),
                    generation,
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

    fn draw(
        &self, mem: &mut dyn GuestMemory, camera: &RenderCamera, generation: u64,
        objects: Vec<ObjectFrame>, items: Vec<ItemDraw>, _masks: ItemMasks,
    ) -> Option<Drawn> {
        use timeline::native::{Metric, Stage, Timer};
        let planning = Timer::start(Stage::Objects, Metric::PrepareNs);
        let models = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(&st.as_ref().filter(|s| s.generation == generation)?.models)
        };
        let mut device = self.device.lock().ok()?;
        // native() drained before the feature walked the guest list. Resolve the
        // implementation's inherited fields on the device that will reserve it.
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let read = crate::native_world_glue::ReadMemory(mem);
        let entry = LayerEntry {
            states: EntryStates::read(|t| table.canonical_to_guest(t.index()).and_then(|g| device.render_state(&read, g))),
            w_buffered: device.hw().control0.is_some_and(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE != 0),
        };
        let plan = plan_view(&models, &objects, &items, &entry, device.samples_levels_in_place());
        let ranges = plan.draws.len();
        let refused = (items.len() - ranges) as u64;
        Stage::Objects.count(Metric::Examined, refused);
        Stage::Objects.count(Metric::RefusedInputs, refused);
        let pending = !models.pending.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        if ranges == 0 && plan.compare.is_none() && !pending { return None; }
        let probing = probe_at().is_some();
        if pending || plan.compare.is_some() || probing {
            device.request_external_pass(Box::new(ObjectsPass {
                models: Arc::clone(&models), camera: *camera,
                objects: if probing { objects.clone() } else { Vec::new() },
                draws: if probing { plan.draws.clone() } else { Vec::new() },
                owners: if probing { plan.owners.clone() } else { Vec::new() },
                refused: plan.refused, compare: plan.compare,
                view: None, w_clip: None, fallback: None, run: 0,
            }));
        }
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !st.as_ref().is_some_and(|s| s.generation == generation && Arc::ptr_eq(&s.models, &models)) {
            Stage::Objects.count(Metric::RefusedInputs, ranges as u64);
            return None;
        }
        drop(planning);
        let object_count = plan.owners.iter().copied().collect::<HashSet<_>>().len();
        let draws: HashMap<_, _> = plan.drawn.into_iter().zip(plan.draws).map(|(index, draw)| {
            let item = &items[index];
            ((objects[item.object].handle, item.key), draw)
        }).collect();
        if draws.is_empty() { return None; }
        let count = draws.len();
        *self.admission.0.lock().ok()? = Some(ObjectLayer {
            frame: crate::d3d_diag::presents() + 1,
            bound: device.bound_target(),
            models,
            camera: *camera,
            draws,
        });
        Some(Drawn::Queued { objects: object_count, items: count })
    }
}

#[derive(Default)]
struct ObjectAdmission(Mutex<Option<ObjectLayer>>);

struct ObjectLayer {
    frame: u64,
    bound: [u32; 8],
    models: Arc<Models>,
    camera: RenderCamera,
    draws: HashMap<(u32, ItemKey), (u32, PlannedDraw)>,
}

fn object_key(geometry: Geometry) -> Option<(u32, ItemKey)> {
    let Owner::Object { handle, section, .. } = geometry.owner else { return None };
    Some((handle, ItemKey {
        record: geometry.record, shader: geometry.shader, section: section.try_into().ok()?,
        part: geometry.part, implementation: usize::from(geometry.implementation),
    }))
}

impl d3d::DrawAdmission for ObjectAdmission {
    fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let context = crate::native_world_layers_glue::indexed_draw()?;
        let draw = snapshot.draw;
        if context.layer.site != Site::Scene || context.layer.number != 3
            || draw.entry != "D3DDevice_DrawIndexedVertices"
            || context.index_count? as usize != draw.indices.as_ref()?.len()
            || !draw.clip_rects.is_empty()
            || !draw.clip.is_some_and(|r| r.is_whole(draw.target.width, draw.target.height))
        {
            return None;
        }
        let key = object_key(context.geometry?)?;
        let mut pending = self.0.lock().ok()?;
        let layer = pending.as_mut()?;
        if layer.frame != context.frame || layer.bound[..2] != [snapshot.color_header, snapshot.depth_header] {
            return None;
        }
        let (_, planned) = layer.draws.get(&key)?;
        if !object_state_matches(draw, planned)
            || !crate::native_world_layers_glue::matches_triangle_range(draw, planned.indices.len())
        {
            return None;
        }
        let accum = native_render::object_program::TEXACCUM_HEADER;
        if planned.texaccum != (0..4).any(|i| snapshot.sampled_stages[i] && draw.textures[i] == Some(accum)) {
            return None;
        }
        let planned = layer.draws.remove(&key)?;
        let pass = ObjectsPass {
            models: Arc::clone(&layer.models), camera: layer.camera, objects: Vec::new(),
            draws: vec![planned], owners: Vec::new(), refused: Vec::new(), compare: None,
            view: None, w_clip: None, fallback: Some(Arc::new(draw.clone())), run: layer.frame,
        };
        Some((d3d::ExternalPlan {
            stage: timeline::native::Stage::Objects, view: context.layer.view, ranges: 1, slots: Vec::new(),
        }, Box::new(pass)))
    }
}

fn object_state_matches(draw: &d3d::ResolvedDraw, planned: &PlannedDraw) -> bool {
    use gfx::state::{CompareFunc, DepthBufferMode, FillMode};
    let r = &draw.state.render;
    let p = planned.state;
    let primitive = gfx::translate::primitive_state(r, wgpu::PrimitiveTopology::TriangleList,
        gfx::translate::Capabilities::from_features(wgpu::Features::empty()));
    let depth = r.depth_mode != DepthBufferMode::Disabled;
    let compare = if depth { gfx::translate::compare_function(r.depth_func) } else { wgpu::CompareFunction::Always };
    let alpha = (r.alpha_test_enable && r.alpha_func != CompareFunc::Always)
        .then(|| (gfx::translate::compare_function(r.alpha_func), r.alpha_ref));
    draw.hw.control0.is_some_and(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE != 0)
        && draw.target.depth_format.is_some()
        && primitive == p.primitive && compare == p.depth_compare && (depth && r.depth_write) == p.depth_write
        && gfx::translate::color_writes(r.color_write) == p.color_writes && alpha == p.alpha_test
        && r.depth_clamp == p.w_clamp && !r.stencil.enable && !r.blend_enable && !r.fog_enable
        && r.fill_mode == FillMode::Solid && r.back_fill_mode == FillMode::Solid
        && (!r.solid_offset_enable || (r.depth_bias == 0.0 && r.depth_bias_slope_scale == 0.0))
}

/// One view's plan ([`plan_view`]).
#[derive(Default)]
struct ViewPlan {
    /// The items drawn (their places in the items handed in).
    drawn: Vec<usize>,
    /// Their draws, in the list's order, each with its model index, and
    /// the object each belongs to.
    draws: Vec<(u32, PlannedDraw)>,
    owners: Vec<usize>,
    /// The items refused, with their object and why (the probe's).
    refused: Vec<(usize, String)>,
    /// The comparison's part, planned: model, draw and label.
    compare: Option<(u32, PlannedDraw, String)>,
}

/// Plans each of `items` against the uploaded models: drawn only when its
/// model is on the GPU, its part and implementation are native, every
/// per-object constant and every state field resolves
/// (`LitObjects::plan`), the device draws W-buffered, and, for a part
/// sampling the texture accumulation target, the device samples levels in
/// place (`levels`). A shadow item is planned for the comparison only.
fn plan_view(models: &Models, objects: &[ObjectFrame], items: &[ItemDraw], entry: &LayerEntry, levels: bool) -> ViewPlan {
    let mut plan = ViewPlan::default();
    // Locked in `upload_pending`'s order: the index, then the lit parts.
    let Ok(index) = models.index.try_lock() else { return plan };
    let Ok(lit) = models.lit.try_lock() else { return plan };
    let compared = compare();
    for (k, item) in items.iter().enumerate() {
        let o = &objects[item.object];
        let planned = (|| -> Result<(u32, PlannedDraw), String> {
            if !entry.w_buffered {
                return Err("the device's draws are not W-buffered (SET_CONTROL0 without z-perspective)".into());
            }
            let model = *index.get(&(o.definition as u16)).ok_or("its model is not uploaded")?;
            let lit = lit.as_ref().ok_or("no model is uploaded")?;
            let d = lit.plan(model, o, &item.key, &item.rows, &entry.states)?;
            if d.texaccum && !levels {
                return Err("it samples the texture accumulation target and the device does not sample levels in place".into());
            }
            Ok((model, d))
        })();
        match planned {
            Ok((model, d)) if item.shadow => {
                if plan.compare.is_none() && compared.is_some_and(|c| c.definition == o.definition as u16 && c.part.picks(d.part, item.key.shader)) {
                    let label = format!("object {:#010x} definition {:#010x} part {} record {}", o.handle, o.definition, d.part, item.key.record);
                    plan.compare = Some((model, d, label));
                }
            }
            Ok((model, d)) => {
                plan.drawn.push(k);
                plan.draws.push((model, d));
                plan.owners.push(item.object);
            }
            Err(why) => {
                if item.shadow && compared.is_some() {
                    eprintln!("native-objects compare: definition {:#010x} record {}: {why}", o.definition, item.key.record);
                }
                count_refusal(&why);
                plan.refused.push((item.object, format!("record {} (section {} part {} implementation {}): {why}", item.key.record, item.key.section, item.key.part, item.key.implementation)));
            }
        }
    }
    plan
}

/// Items the plan gave back to the D3D path, and the first of each reason
/// (logged once).
static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REASONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn count_refusal(why: &str) {
    REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut seen = REASONS.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashSet::new);
    if seen.len() < 64 && seen.insert(why.to_owned()) {
        eprintln!("native-objects: an item keeps its D3D draw: {why} (each reason logged once)");
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
    let switches = feature::Switches::from_env().unwrap_or_default();
    if !switches.feature(feature::native_objects::SWITCH, true) {
        return;
    }
    let admission = Arc::new(ObjectAdmission::default());
    if let Ok(mut device) = device.lock() {
        device.add_draw_admission(admission.clone());
    }
    feature::native_objects::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state: Arc::new(Mutex::new(None)),
        admission,
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
                    if feature::native_verbose() {
                        eprintln!("native-objects: {name}: definition {def:#010x} left to the D3D path (H2_NATIVE_OBJECTS_EXCEPT)");
                    }
                    continue;
                }
                let t0 = std::time::Instant::now();
                let result = load_model(&mut opened, def);
                // Before the model is queued: a pass that draws it checks
                // the palettes its base maps went through.
                models.palettes.lock().unwrap_or_else(|e| e.into_inner()).clone_from(opened.decoder.used());
                match result {
                    Ok(cpu) => {
                        let nodes = cpu.model.inverse_binds.len();
                        let draws = cpu.model.draws.len();
                        let lit = cpu.lit.parts.len();
                        let parts = set_parts(&cpu.lit);
                        models.pending.lock().unwrap_or_else(|e| e.into_inner()).push(cpu);
                        // Published after the model is queued, and only to
                        // the load this worker serves (its `Models`): a
                        // worker of an earlier load of the same map (A, B,
                        // A again) publishes nothing.
                        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(s) = st.as_mut().filter(|s| publishes_to(s, &models)) else { return };
                        let mut set = (*s.set).clone();
                        set.definitions.insert(def as u16, nodes);
                        set.parts.extend(parts.into_iter().map(|((section, part), p)| ((def as u16, section, part), p)));
                        if compare().is_some_and(|c| c.definition == def as u16) {
                            set.shadowed.insert(def as u16);
                        }
                        s.set = Arc::new(set);
                        // No lock held across the print.
                        drop(st);
                        loaded += 1;
                        if feature::native_verbose() {
                            eprintln!(
                                "native-objects: {name}: definition {def:#010x}: {draws} draws, {lit} lit parts, {nodes} nodes, in {:.0} ms ({loaded} models native, {refused} left to D3D)",
                                t0.elapsed().as_secs_f32() * 1000.0
                            );
                        }
                    }
                    Err(e) => {
                        refused += 1;
                        if feature::native_verbose() {
                            eprintln!("native-objects: {name}: definition {def:#010x} left to the D3D path ({e})");
                        }
                    }
                }
            }
            eprintln!("native-objects: {name}: {loaded} models native, {refused} left to the D3D path");
        })
        .ok()?;
    Some(tx)
}

/// The map open for model loads.
struct Opened {
    map: NativeMap,
    ctx: tags::object::Context,
    raw: Raw,
    /// Base maps as their stages sample them (palettized ones expanded
    /// through the stage's palette), shared by every model of the map.
    decoder: StageDecoder,
    images: StageImages,
    /// The shader tags, read at the first material that needs them.
    lib: Option<Option<tags::material::ShaderLibrary>>,
    /// The vector normalization cube, loaded as the lights load it at the
    /// first lit model (`Some(None)`: it does not load).
    normalization: Option<Option<Arc<StaticLightTexture>>>,
    /// The game's stream formats (`None`: lit materials stay on D3D).
    formats: Option<StreamFormats>,
    /// What an object's draw samples for the lightmap (the globals'
    /// rasterizer bitmap at `+0x64`, `object_program::OBJECT_LIGHTMAP`),
    /// read at the first lit model: the image when uniform, else why not.
    lightmap: Option<Result<Image, String>>,
}

fn open(name: &str, palettes: StagePalettes, formats: Option<StreamFormats>) -> Result<Opened, load::Error> {
    let (mut map, files) = native_map::open(name)?;
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
fn tag_of(map: &NativeMap, datum: u32) -> Option<TagId> {
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

/// Prepares a definition's supported layer-3 programs and geometry.
fn load_model(o: &mut Opened, def: u32) -> Result<CpuModel, load::Error> {
    let id = tag_of(&o.map, def).ok_or("no such tag")?;
    let loaded = model_load::load_object_model(&mut o.map, &o.ctx, &mut o.raw, id)?;
    // Every opaque material through its own programs, or left to D3D: the
    // base-map stand-in is not used for opaque parts (it is neither the
    // game's shading nor, for an animated object, the pose its records'
    // rows give: the scarab of 03a's intro).
    let base_maps = HashMap::new();
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
    let lit = if lit_materials.is_empty() {
        LitModelInput::default()
    } else {
        let LitBuild { input, refused, .. } = lit_parts(o, &loaded, &lit_materials);
        for (m, why) in refused {
            unlit.push(format!("{:#010x} {why}", loaded.shaders[usize::from(m)].0));
        }
        input
    };
    let model = keep_materials(loaded.model, &base_maps);
    if model.draws.is_empty() && lit.is_empty() {
        if !unlit.is_empty() {
            return Err(format!("its lit materials stay on D3D: {}", unlit.join(", ")).into());
        }
        return Err("no material with a base map".into());
    }

    Ok(CpuModel {
        definition: def,
        model,
        base_maps,
        normalization: if lit.is_empty() { None } else { o.normalization.clone().flatten() },
        lit,
    })
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
                .filter(|(_, p)| p.is_drawn() && p.kind != load::PART_TRANSPARENT)
                .map(|(k, p)| (k as u8, p.material, g.part_triangles(p)))
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
            Some(lib) => crate::native_lights_glue::light_textures(&mut o.map, lib.metadata(), &mut o.raw.0).0,
            None => None,
        };
        o.normalization = Some(cube);
    }
    if o.lightmap.is_none() {
        let l = object_lightmap(o);
        match &l {
            Ok(i) => {
                if feature::native_verbose() {
                    eprintln!("native-objects: the objects' lightmap stage loads ({}x{}, uniform)", i.width, i.height);
                }
            }
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

/// The native set's entries of a model's lit parts, by (section, part):
/// the part's shader (tag index), its implementations, and those whose
/// program samples the texture accumulation target.
fn set_parts(lit: &LitModelInput) -> Vec<((u16, u8), SetPart)> {
    lit.parts
        .iter()
        .map(|p| {
            let mut e = SetPart { shader: p.shader_tag as u16, ..SetPart::default() };
            for (n, slot) in p.implementations.iter().enumerate() {
                let Some((program, _)) = slot else { continue };
                e.implementations |= 1 << n;
                if lit.programs.get(*program).is_some_and(|x| x.pass.texaccum.contains(&true)) {
                    e.texaccum |= 1 << n;
                }
            }
            ((p.section, p.part), e)
        })
        .collect()
}

/// Keeps the model's opaque draws for supported base maps. Sorted
/// transparent draws are handled at the indexed-draw boundary.
fn keep_materials(mut model: Model, native: &HashMap<u16, Image>) -> Model {
    model.draws.retain(|d| native.contains_key(&d.material));
    model.transparents.clear();
    model
}

/// One view's objects: the draws planned at the hook, in the list's order.
struct ObjectsPass {
    models: Arc<Models>,
    camera: RenderCamera,
    /// The frames handed with the items (the probe's spheres).
    objects: Vec<ObjectFrame>,
    /// The planned draws with their model index, and each one's object.
    draws: Vec<(u32, PlannedDraw)>,
    owners: Vec<usize>,
    /// The items left to D3D, with their object and why (the probe's).
    refused: Vec<(usize, String)>,
    /// The comparison's part, armed at the encode (with the view).
    compare: Option<(u32, PlannedDraw, String)>,
    /// The view's constants, for the lit parts' programs.
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    /// The device's W-buffer clip at the pass (`d3d::DepthClip`), as the
    /// world's: the range the D3D path draws the layer with (the clamp is
    /// each draw's, `object_state`). Without it the programs clipped w at
    /// 0, not at the view's near (03a: 256 depth units), and drew the part
    /// of an object nearer than that, which the D3D path culls.
    w_clip: Option<([f32; 2], bool)>,
    fallback: Option<Arc<d3d::ResolvedDraw>>,
    run: u64,
}

impl d3d::ExternalPass for ObjectsPass {
    fn uses_validated_resources(&self) -> bool { self.fallback.is_some() }

    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.fallback.clone()
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = Some((clip.range, clip.clamp));
    }

    fn sampled(&self) -> Vec<u32> {
        if self.draws.iter().any(|(_, d)| d.texaccum) || self.compare.as_ref().is_some_and(|c| c.1.texaccum) {
            vec![native_render::object_program::TEXACCUM_HEADER]
        } else {
            Vec::new()
        }
    }

    fn writes(&self) -> d3d::ExternalWrites {
        if self.fallback.is_none() {
            return d3d::ExternalWrites::default();
        }
        d3d::ExternalWrites {
            color: self.draws.iter().any(|(_, d)| !d.state.color_writes.is_empty()),
            depth_stencil: self.draws.iter().any(|(_, d)| d.state.depth_write),
        }
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.view.is_none() { return Err("object view constants unavailable"); }
        if self.draws.iter().any(|(_, d)| d.texaccum) && texaccum_level(t.sampled, t.guest_size, t.size).is_none() {
            return Err("object texture accumulation unavailable");
        }
        let lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        if lit.as_ref().is_some_and(|l| self.draws.iter().all(|(model, d)| l.has(*model) && !d.indices.is_empty())) {
            Ok(())
        } else {
            Err("planned object geometry unavailable")
        }
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        // Comparison-only views can still upload models for the following view.
        drop(upload_pending(&self.models, &t));
        self.record_ranges(t);
    }

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let Some(constants) = self.view else { return 0 };
        let mut lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lit) = lit.as_mut() else { return 0 };
        let texaccum = texaccum_level(t.sampled, t.guest_size, t.size);
        let view = self.view_inputs(constants, &t);
        if let Some((model, d, label)) = &self.compare {
            arm_compare(lit, *model, d, label, &view, texaccum);
        }
        if let Some(at) = probe_at()
            && probe_due()
        {
            let vp = self.camera.view_projection();
            let size = (t.guest_size.0 as f32, t.guest_size.1 as f32);
            for (k, o) in self.objects.iter().enumerate() {
                if sphere_covers(&vp, o.sphere, size, at).is_none() {
                    continue;
                }
                for ((model, d), _) in self.draws.iter().zip(&self.owners).filter(|(_, owner)| **owner == k) {
                    eprintln!("native-objects probe at {at:?}: definition {:#010x} handle {:#010x}: drawn: {}", o.definition, o.handle, lit.describe(*model, d));
                }
                for (_, why) in self.refused.iter().filter(|(owner, _)| *owner == k) {
                    eprintln!("native-objects probe at {at:?}: definition {:#010x} handle {:#010x}: left to D3D: {why}", o.definition, o.handle);
                }
            }
            eprintln!(
                "native-objects probe at {at:?}: texture accumulation level {:?} (guest {:?}, copy {:?})",
                t.sampled.first().map(|l| l.as_ref().map(|l| l.size)),
                t.guest_size,
                t.size
            );
        }
        0
    }

    fn record_bundles(&mut self, t: &mut d3d::ExternalTarget<'_>) -> Option<d3d::backend::ExternalBundles> {
        let constants = self.view?;
        let view = self.view_inputs(constants, t);
        let mut lit = self.models.lit.lock().ok()?;
        let texaccum = texaccum_level(t.sampled, t.guest_size, t.size);
        if let Some(accum) = &texaccum
            && (accum.view.texture() == t.color.texture() || accum.view.texture() == t.depth.texture())
        {
            return None;
        }
        lit.as_mut()?.bundles(t.device, t.queue, t.uploads, t.color_format, t.depth_format,
            t.size, &view, &self.draws, texaccum, self.run)
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

/// `H2_NATIVE_OBJECTS_COMPARE=definition:part[:after]`: one lit part
/// compared with the D3D path's draw of it (`d3d::uniform_compare`, as
/// the world's `H2_NATIVE_WORLD_DUMP`). The definition's items keep their
/// D3D draws (`NativeSet::shadowed`) and are planned as the pass would
/// draw them; on the pass after `after` (default 0) that plans its part
/// `part` (an index into the model's lit parts, one per section and
/// geometry part; or, written `0x...`, the first item whose shader is that
/// tag, e.g. `0xe92207ad:0xe5da0465:30` for 03a's rebar), that draw is
/// armed, and
/// the D3D draws with the same node rows are compared (the part's own
/// program field by field; the object's other draws, its other layers
/// among them, named with their cull and depth).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Compare {
    definition: u16,
    part: Part,
    after: u32,
}

/// Which of the compared definition's parts ([`Compare`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// An index into the model's lit parts.
    Index(usize),
    /// The first item whose shader has this tag index.
    Shader(u16),
}

impl Part {
    fn picks(self, part: usize, shader: u32) -> bool {
        match self {
            Part::Index(i) => i == part,
            Part::Shader(s) => s == shader as u16,
        }
    }
}

fn compare() -> Option<Compare> {
    static C: std::sync::OnceLock<Option<Compare>> = std::sync::OnceLock::new();
    *C.get_or_init(|| {
        let c = std::env::var("H2_NATIVE_OBJECTS_COMPARE").ok().and_then(|v| parse_compare(&v));
        if let Some(c) = c {
            eprintln!("native-objects: comparing {c:?} with the D3D path's draws (its objects left to D3D)");
        }
        c
    })
}

fn parse_compare(v: &str) -> Option<Compare> {
    let mut it = v.trim().split(':');
    let definition = *parse_definitions(it.next()?).first()?;
    let part = match it.next().map(str::trim) {
        None => Part::Index(0),
        Some(p) => match p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
            Some(hex) => Part::Shader(u32::from_str_radix(hex, 16).ok()? as u16),
            None => Part::Index(p.parse().ok()?),
        },
    };
    let after = it.next().map_or(Some(0), |a| a.trim().parse().ok())?;
    Some(Compare { definition, part, after })
}

/// Arms the comparison once, on the pass after the comparison's `after`
/// that planned its part: `d` of model `model`, named `label`.
fn arm_compare(lit: &LitObjects, model: u32, d: &PlannedDraw, label: &str, view: &ViewInputs, texaccum: Option<Texaccum<'_>>) {
    static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let Some(c) = compare() else { return };
    if SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) != c.after {
        return;
    }
    let label = label.to_owned();
    match lit.part_uniforms(model, d, view, texaccum) {
        Ok(u) => {
            eprintln!(
                "native-objects compare {label}: {}; {} program words, rows c[50..] {:?}, decompression c[170..173] {:?}, textures {:?}",
                lit.describe(model, d),
                u.words.len(),
                &u.vertex.constants[50..53],
                &u.vertex.constants[170..173],
                u.textures
            );
            d3d::uniform_compare::arm(d3d::uniform_compare::Expected {
                label,
                words: u.words,
                vertex: u.vertex,
                pixel: u.pixel,
                options: u.options,
                def: u.def,
                pixel_options: u.pixel_options,
                primitive: u.primitive,
                depth: (u.depth.0, u.depth.1, Default::default()),
                color: None,
                textures: u.textures,
                samplers: u.samplers,
                draws: 4,
            });
        }
        Err(e) => eprintln!("native-objects compare {label}: {e}"),
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
    fn an_object_draw_is_keyed_by_owner_record_shader_part_and_implementation() {
        let geometry = Geometry {
            shader: 0xe000_0001, variant: 0, implementation: 2, sorted_slot: 16,
            record: 5, part: 3, owner: Owner::Object { handle: 9, model: 18, section: 4 },
        };
        assert_eq!(object_key(geometry), Some((9, ItemKey {
            record: 5, shader: 0xe000_0001, section: 4, part: 3, implementation: 2,
        })));
        assert!(object_key(Geometry { owner: Owner::Cluster(4), ..geometry }).is_none());
    }

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
            draws: Vec::new(),
            owners: Vec::new(),
            refused: Vec::new(),
            compare: None,
            view: None,
            w_clip: None,
            fallback: None,
            run: 0,
        };
        pass.depth_clip(d3d::DepthClip { range: [256.0, 16_777_215.0], clamp: false, w_buffered: Some(true) });
        assert_eq!(pass.w_clip, Some(([256.0, 16_777_215.0], false)));
        assert!(pass.sampled().is_empty(), "no draw samples the texture accumulation level");
        let constants = [[0.0; 4]; d3d::VIEW_CONSTANTS];
        let view = ViewInputs { constants, guest_size: (640, 480), size: (640, 480), z_scale: 16_777_215.0, w_clip: pass.w_clip };
        let u = native_render::program::frame_vertex(&Default::default(), &view);
        assert_eq!(u.w_clip, Some([256.0, 16_777_215.0]), "near at the device's 256, not 0");
    }

    #[test]
    fn a_comparison_names_a_definition_a_part_and_a_pass() {
        assert_eq!(parse_compare("0xe92207ad:2:30"), Some(Compare { definition: 0x07ad, part: Part::Index(2), after: 30 }));
        assert_eq!(parse_compare("e92207ad"), Some(Compare { definition: 0x07ad, part: Part::Index(0), after: 0 }));
        assert_eq!(parse_compare("0xe92207ad:0xe5da0465:30"), Some(Compare { definition: 0x07ad, part: Part::Shader(0x0465), after: 30 }));
        assert_eq!(parse_compare("0xe92207ad:x"), None);
        assert!(Part::Shader(0x0465).picks(7, 0xe5da_0465));
        assert!(!Part::Index(2).picks(7, 0xe5da_0465));
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
        let m = keep_materials(model, &native);
        assert_eq!(m.draws.iter().map(|d| (d.material, d.first_index)).collect::<Vec<_>>(), vec![(1, 3), (1, 6), (2, 9)]);
        assert_eq!(m.materials(), vec![1, 2]);
        assert!(m.transparents.is_empty());
    }

    fn map_state(name: &str, generation: u64, models: &Arc<Models>) -> MapState {
        MapState {
            key: (name.to_owned(), 0x800e_6010, 0),
            generation,
            models: Arc::clone(models),
            set: Arc::new(NativeSet { map: name.to_owned(), generation, ..NativeSet::default() }),
            tried: HashSet::new(),
            worker: None,
        }
    }

    /// A worker publishes only to the load it was started for: after A,
    /// B and A again, the first A's worker (still finishing) finds a load
    /// of the same name but other models, and publishes nothing there.
    #[test]
    fn a_worker_publishes_only_to_its_own_load() {
        let first = Arc::new(Models::default());
        let again = Arc::new(Models::default());
        assert!(publishes_to(&map_state("03a_oldmombasa", 1, &first), &first));
        assert!(!publishes_to(&map_state("03a_oldmombasa", 3, &again), &first), "the same name, another load");
        assert!(publishes_to(&map_state("03a_oldmombasa", 3, &again), &again));
    }

    fn frame(definition: u32) -> ObjectFrame {
        ObjectFrame {
            handle: 0x8001_0001,
            definition,
            nodes: Vec::new(),
            sphere: ([0.0; 3], 1.0),
            lighting: None,
            defaults: None,
        }
    }

    fn item(object: usize, record: u16, shadow: bool) -> ItemDraw {
        ItemDraw {
            index: object,
            object,
            key: native_render::objects::ItemKey { record, shader: 0xe000_0010, section: 0, part: 0, implementation: 0 },
            rows: vec![[0.0; 4]; 3],
            shadow,
        }
    }

    /// Every item the plan cannot resolve is refused, with why, before the
    /// hook returns: a model not uploaded yet, or a device not
    /// W-buffered. Nothing is drawn then, and a shadow item never is.
    #[test]
    fn an_item_whose_draw_does_not_resolve_is_refused_at_the_hook() {
        let models = Models::default();
        let objects = vec![frame(0xe000_0100)];
        let entry = LayerEntry { states: EntryStates::default(), w_buffered: true };
        let plan = plan_view(&models, &objects, &[item(0, 1, false), item(0, 2, true)], &entry, true);
        assert!(plan.drawn.is_empty() && plan.draws.is_empty() && plan.compare.is_none());
        assert_eq!(plan.refused.len(), 2);
        assert!(plan.refused[0].1.contains("record 1") && plan.refused[0].1.contains("not uploaded"), "{:?}", plan.refused);
        models.index.lock().unwrap().insert(0x0100, 7);
        let plan = plan_view(&models, &objects, &[item(0, 1, false)], &entry, true);
        assert!(plan.refused[0].1.contains("no model is uploaded"), "{:?}", plan.refused);
        let z = LayerEntry { w_buffered: false, ..entry };
        let plan = plan_view(&models, &objects, &[item(0, 1, false)], &z, true);
        assert!(plan.refused[0].1.contains("W-buffered"), "{:?}", plan.refused);
    }

    /// The set is keyed by what a record names: (section, part), each
    /// with its shader, its implementations and those sampling the
    /// texture accumulation target.
    #[test]
    fn a_models_lit_parts_enter_the_set_by_section_and_part() {
        use native_render::object_program_gpu::LitPart;
        let part = |section, part, implementations| LitPart {
            section,
            part,
            decompression: [[0.0; 4]; 3],
            shader_tag: 0xe5da_0465,
            shader: Default::default(),
            indices: 0..3,
            implementations,
        };
        let lit = LitModelInput {
            parts: vec![part(2, 0, [Some((0, 0)), None, Some((1, 0)), None, None]), part(2, 3, [None, Some((2, 0)), None, None, None])],
            ..LitModelInput::default()
        };
        let parts: HashMap<(u16, u8), SetPart> = set_parts(&lit).into_iter().collect();
        assert_eq!(parts[&(2, 0)], SetPart { shader: 0x0465, implementations: 0b101, texaccum: 0 });
        assert_eq!(parts[&(2, 3)].implementations, 0b010);
        assert_eq!(parts.len(), 2);
    }
}
