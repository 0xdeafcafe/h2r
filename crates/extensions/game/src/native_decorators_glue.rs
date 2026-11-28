//! The native decorators' host (docs/graphics/native-decorators.md, "The
//! glue"): over the native world's load (`native_world_glue`), the
//! decorator passes read from the map's tags; at layer 3, each material
//! the open view draws made ready from guest memory; and one external
//! pass per view (`render_bridge::native_decorators`). Inert when
//! `impl.render.native` is off (nothing is loaded and the hook is not
//! placed).
//!
//! - **At load** ([`load`], on the world's worker): every shader a
//!   decorator set (`DECR`) names, resolved (`tags::material`), and each
//!   of its layer-3 passes at each detail level as the build picks it
//!   (`native_render::decorators_load::decorator_look`), with the stage
//!   bitmaps decoded (a palettized one expanded through the device
//!   palette its stage holds at load, as the world's are). One line says
//!   how many passes load and why the rest do not.
//! - **At first sight** ([`Host::prepare`], at the layer-3 call, once per
//!   load and material): the set's geometry and stream format from guest
//!   memory (`set_geometry_with`, `object_streams::StreamFormats`). A
//!   utility worker translates the pass, gathers the set's vertices and
//!   uploads its buffers and images. The first view of each frame takes
//!   finished materials from this load. A set that is not resident is
//!   tried again at the next view.
//! - **Per view** ([`Host::holds`]): the ready materials whose guest
//!   draws a skip leaves out without changing a render state held at the
//!   layer-3 call (`decorators_load::skip_holds`).
//! - **Per view** ([`Host::ask`]): the stencil and cull for the pipelines
//!   (`decorators_gpu::DrawKey`), the skip over a preview (`clear_slots`),
//!   then the masks and the stage pushed at the opaque slot.
//! - **The pass** (`DecoratorPass`): the view's constants and W clip from
//!   the frame graph, and the pass's pipeline for the attachments' formats
//!   under the view's stencil and cull. `decorators_gpu::bind` and
//!   `encode` record it.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use engine::memory::GuestMemory;
use map::{FourCC, Map, TagId};
use native_render::decorators::{Material, Plan, clear_slots};
use native_render::decorators_gpu::{DecoratorTarget, DrawKey, MaterialGpu, bind, encode};
use native_render::decorators_load::{
    DECORATOR_CLASSIFICATION, DecoratorLook, SetGeometry, SetRefusal, buffers, compile, decorator_look, set_geometry_with,
    skip_holds, stage_images,
};
use gfx::external::{ExternalAttachments, ExternalEncoded};
use native_render::stage::StageWork;
use native_render::gpu::{Image, PassViewport};
use native_render::look::{self, Stage};
use native_render::object_streams::{StreamElement, StreamFormats};
use native_render::program::ViewInputs;
use native_render::skip::{Cleared, LIGHTMAP_LAYER};
use render_bridge::native_decorators::Host;

use crate::native_world_glue::{ReadMemory, Ready, StageDecoder, StageImages, StagePalettes, State, current_ready};

/// The layer the model decorators draw in (`skip::LIGHTMAP_LAYER`), as
/// the template's layer list numbers it.
const DECORATOR_LAYER: u8 = LIGHTMAP_LAYER as u8;
/// Most shader references one decorator set may hold.
const MAX_SET_SHADERS: u32 = 256;
/// A decorator set's root: `+0` its block of shader references (8 bytes
/// each, the datum at `+4`).
const SET_SHADERS: u32 = 0;
const TAGREF_SIZE: u32 = 8;
const TAGREF_DATUM: u32 = 4;
const DECR: FourCC = FourCC(*b"DECR");
const SHAD: FourCC = FourCC(*b"shad");

/// The decorator passes of one load: by shader tag index, detail level
/// and pass, layer 3's pass as the stage draws it or why it does not.
pub struct DecoratorLooks {
    looks: HashMap<(u16, u8, u8), Result<DecoratorLook, String>>,
    images: StageImages,
    /// Images a live edit decoded again, by (bitmap, image): a material
    /// made after the edit takes them over the load's `images`.
    edited: Mutex<StageImages>,
    /// What a lightmap stage samples: the globals' rasterizer bitmap at
    /// `+0x64`, uniform (`native_objects_glue::engine_lightmap`), read
    /// when a pass samples it.
    lightmap: Option<Image>,
    /// The device palettes the load expanded palettized stage bitmaps
    /// through (`StageDecoder::used`): a view draws only while they hold.
    palettes: StagePalettes,
    prepared: Mutex<Prepared>,
    gpu: Mutex<HashMap<Material, MaterialGpu>>,
}

impl DecoratorLooks {
    /// The shaders the decorator passes were compiled from, by tag index:
    /// a live edit of one reloads the world (docs/editor/realtime-edits.md,
    /// "The native refresh").
    pub fn shaders(&self) -> std::collections::BTreeSet<u16> {
        self.looks.keys().map(|&(shader, _, _)| shader).collect()
    }

    /// The pass `material` draws with (its shader's, at its detail level
    /// and pass), `None` when the load has none for it.
    fn look(&self, material: &Material) -> Option<&Result<DecoratorLook, String>> {
        if material.layer != DECORATOR_LAYER {
            return None;
        }
        self.looks.get(&(material.shader as u16, material.variant, material.pass))
    }
}

/// The shaders the map's decorator sets name, each once.
fn set_shaders<R: Read + Seek>(map: &Map<R>, lib: &tags::material::ShaderLibrary) -> Vec<TagId> {
    let meta = lib.metadata();
    let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(meta.bytes(a, 4).ok()?.try_into().ok()?)) };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tag in map.tags().iter().filter(|t| t.group == DECR) {
        let Some(root) = tag.location.map(|l| l.address) else { continue };
        let (Some(count), Some(at)) = (word(root + SET_SHADERS), word(root + SET_SHADERS + 4)) else { continue };
        for k in 0..count.min(MAX_SET_SHADERS) {
            let Some(datum) = word(at.wrapping_add(k * TAGREF_SIZE + TAGREF_DATUM)) else { break };
            let id = TagId(datum);
            if id != TagId::NULL && lib.group_of(id) == Some(SHAD) && seen.insert(id) {
                out.push(id);
            }
        }
    }
    out
}


/// Reads every layer-3 pass of the shaders the map's decorator sets name.
/// Called from the world's worker.
pub fn load<R: Read + Seek>(
    map: &mut Map<R>, files: &mut assets::ResourceFiles, decoder: &mut StageDecoder,
    mut images: StageImages,
) -> Arc<DecoratorLooks> {
    let mut looks = HashMap::new();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("native-decorators: the shader tags do not read ({e}); no decorator draws");
            return Arc::new(DecoratorLooks {
                looks,
                images: StageImages::new(),
                edited: Mutex::default(),
                lightmap: None,
                palettes: StagePalettes::default(),
                prepared: Mutex::default(),
                gpu: Mutex::default(),
            });
        }
    };
    let shaders = set_shaders(map, &lib);
    let mut refused: HashMap<String, usize> = HashMap::new();
    for &shader in &shaders {
        let material = match lib.material(shader, None) {
            Ok(m) => m,
            Err(e) => {
                *refused.entry(format!("material: {e}")).or_default() += 1;
                continue;
            }
        };
        let Ok(index) = u16::try_from(shader.index()) else { continue };
        for list in material.layers.iter().filter(|l| l.layer == usize::from(DECORATOR_LAYER)) {
            let Ok(lod) = u8::try_from(list.lod) else { continue };
            for pass in 0..list.passes.len() {
                let Ok(pass) = u8::try_from(pass) else { break };
                let meta = lib.metadata();
                let mut program = |vrtx: TagId| {
                    let address = map.tag(vrtx).ok()?.location?.address;
                    Some((
                        look::vertex_program(meta, address, DECORATOR_CLASSIFICATION)?,
                        look::vertex_semantics(meta, address, DECORATOR_CLASSIFICATION)?,
                    ))
                };
                let look = decorator_look(&material, lod, DECORATOR_LAYER, pass, &mut program);
                let look = look.and_then(|look| {
                    for (n, s) in look.look.textures.iter().enumerate() {
                        let Stage::Bitmap { bitmap, image } = *s else { continue };
                        decoder.decode(map, files, &mut images, bitmap, image, n, true)?;
                    }
                    Ok(look)
                });
                if let Err(why) = &look {
                    *refused.entry(why.clone()).or_default() += 1;
                }
                looks.insert((index, lod, pass), look);
            }
        }
    }
    // The lightmap stage's image, once, at the first stage that samples
    // it; the passes sampling it do not draw when it does not load.
    let samples = |l: &DecoratorLook| l.look.textures.iter().position(|s| *s == Stage::Lightmap);
    let mut lightmap = None;
    if let Some(stage) = looks.values().filter_map(|l| l.as_ref().ok().and_then(samples)).next() {
        match crate::native_objects_glue::engine_lightmap(map, &lib, files, decoder, &mut images, stage) {
            Ok(image) => lightmap = Some(image),
            Err(e) => {
                let why = format!("a stage samples the lightmap: {e}");
                for look in looks.values_mut() {
                    if look.as_ref().ok().and_then(samples).is_some() {
                        *refused.entry(why.clone()).or_default() += 1;
                        *look = Err(why.clone());
                    }
                }
            }
        }
    }
    let mut why: Vec<_> = refused.into_iter().collect();
    why.sort();
    eprintln!(
        "native-decorators: {} decorator shaders, {} of {} layer-3 passes load; not drawn: {why:?}",
        shaders.len(),
        looks.values().filter(|l| l.is_ok()).count(),
        looks.len(),
    );
    let used: HashSet<_> = looks.values().filter_map(|look| look.as_ref().ok())
        .flat_map(|look| &look.look.textures)
        .filter_map(|stage| match *stage {
            Stage::Bitmap { bitmap, image } => Some((bitmap, image)),
            _ => None,
        })
        .collect();
    images.retain(|key, _| used.contains(key));
    Arc::new(DecoratorLooks {
        looks,
        images,
        edited: Mutex::default(),
        lightmap,
        palettes: decoder.used().clone(),
        prepared: Mutex::default(),
        gpu: Mutex::default(),
    })
}

type MaterialResult = Result<MaterialGpu, String>;

/// One outstanding job per load. The result channel belongs only to that
/// load, so a late upload cannot publish into a different BSP.
#[derive(Default)]
struct Prepared {
    pending: Option<(Material, mpsc::Receiver<MaterialResult>)>,
    /// None for a refused material, otherwise whether its pass writes depth.
    depth: HashMap<Material, Option<bool>>,
    frame: Option<u64>,
    /// A live edit of a model or a section made the prepared materials'
    /// geometry stale ([`Prepared::settle`]).
    stale: bool,
}

impl Prepared {
    /// Forgets every prepared material once a live edit has made their
    /// geometry stale: the next view prepares each again from guest
    /// memory, which the edit wrote, and until it is done the guest's own
    /// draws stand. Waits (returns, with `stale` kept) while the previous
    /// frame is still binding a batch.
    fn settle(&mut self, materials: &Mutex<HashMap<Material, MaterialGpu>>) {
        if !self.stale {
            return;
        }
        let Ok(mut materials) = materials.try_lock() else { return };
        materials.clear();
        self.depth.clear();
        self.pending = None;
        self.stale = false;
    }

    /// All views of a guest frame see the same set of uploaded materials.
    fn begin_frame(&mut self, frame: u64, materials: &Mutex<HashMap<Material, MaterialGpu>>) {
        if self.frame == Some(frame) {
            return;
        }
        self.frame = Some(frame);
        if let Some((material, receiver)) = &self.pending {
            // The previous frame may still be binding a batch. Keep the
            // completed upload in its mailbox for another frame in that case.
            let Ok(mut materials) = materials.try_lock() else { return };
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Err("material worker stopped".into()),
            };
            let depth = match result {
                Ok(gpu) => {
                    let depth = gpu.writes_depth();
                    materials.insert(*material, gpu);
                    Some(depth)
                }
                Err(why) => {
                    drop(materials);
                    log_once(&why);
                    None
                }
            };
            self.depth.insert(*material, depth);
            self.pending = None;
        }
    }
}

/// The only input that comes from live guest memory is an owned copy of
/// the resident geometry and its stream declaration.
struct MaterialJob {
    material: Material,
    set: SetGeometry,
    stream: Vec<StreamElement>,
    looks: Arc<DecoratorLooks>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    generation: u64,
    reply: mpsc::Sender<MaterialResult>,
}

impl MaterialJob {
    fn upload(&self) -> MaterialResult {
        let _scope = mem_watch::scope("native.decorators.upload");
        let Some(Ok(look)) = self.looks.look(&self.material) else {
            return Err("the load has no supported decorator pass for the shader".into());
        };
        let program = compile(look, &self.stream)?;
        let buffers = buffers(&program, &self.set)?;
        let edited = self.looks.edited.lock().unwrap_or_else(|e| e.into_inner());
        let bitmap = |bitmap: TagId, image: u32| {
            edited.get(&(bitmap, image)).or_else(|| self.looks.images.get(&(bitmap, image))).cloned()
        };
        let images = stage_images(&program, &bitmap, self.looks.lightmap.as_ref())?;
        let gpu = MaterialGpu::new(&self.device, &self.queue, program, &buffers, &images).map_err(str::to_owned)?;
        self.queue.submit([]);
        Ok(gpu)
    }
}

fn material_worker() -> Option<mpsc::SyncSender<MaterialJob>> {
    let (tx, rx) = mpsc::sync_channel::<MaterialJob>(1);
    let worker = background::spawn("native-decorators load", move || {
        for job in rx {
            if job.generation == render_bridge::native_map_work::bsp_generation() {
                let _timer = timeline::setup::Stage::Decorators.worker();
                let _ = job.reply.send(job.upload());
            }
        }
    });
    if let Err(e) = worker {
        eprintln!("native-decorators: could not start material worker ({e})");
        return None;
    }
    Some(tx)
}

/// Each refusal reason, logged once.
static LOGGED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn log_once(why: &str) {
    let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
    if logged.get_or_insert_with(HashSet::new).insert(why.to_owned()) {
        eprintln!("native-decorators: a material does not draw: {why}");
    }
}

struct Glue {
    state: State,
    worker: OnceLock<Option<mpsc::SyncSender<MaterialJob>>>,
    /// A live edit's image refresh is running ([`Glue::refresh_edits`]);
    /// edits wait in the queue until it ends, so they apply in order.
    refreshing: Arc<AtomicBool>,
}

impl Glue {
    fn ready(&self, mem: &dyn GuestMemory) -> Option<Arc<Ready>> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        current_ready(&state, mem).cloned()
    }

    /// Applies the queued live edits to the decorators
    /// (docs/editor/realtime-edits.md, "The native refresh"): an edited
    /// model or section makes every prepared material's geometry stale,
    /// so each is prepared again from guest memory ([`Prepared::settle`]);
    /// each edited image is decoded again at the stages the passes sample
    /// it, on a worker, and the prepared materials' textures are rewritten
    /// in place ([`refresh_images`]).
    fn refresh_edits(&self, mem: &dyn GuestMemory, looks: &Arc<DecoratorLooks>, p: &mut Prepared) {
        use render_bridge::native_edits::{self, Consumer, Edit};
        p.settle(&looks.gpu);
        if !native_edits::pending(Consumer::Decorators) || self.refreshing.load(Ordering::Acquire) {
            return;
        }
        let current = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            crate::native_world_glue::current_ready_keyed(&state, mem)
        };
        let Some((ready, key)) = current else { return };
        let Some((_, queue)) = ready.upload_device.clone() else { return };
        let mut images = Vec::new();
        for edit in native_edits::drain(Consumer::Decorators) {
            match edit {
                Edit::Model { .. } | Edit::Section { .. } => p.stale = true,
                Edit::Image { bitmap, image } => images.push((TagId(bitmap), image)),
                Edit::Tag { .. } | Edit::Shader { .. } => {}
            }
        }
        p.settle(&looks.gpu);
        if images.is_empty() {
            return;
        }
        self.refreshing.store(true, Ordering::Release);
        let refreshing = Arc::clone(&self.refreshing);
        let looks = Arc::clone(looks);
        let launched = background::spawn("native-decorators edits", move || {
            refresh_images(&looks, &key, &queue, &images);
            refreshing.store(false, Ordering::Release);
        });
        if let Err(e) = launched {
            self.refreshing.store(false, Ordering::Release);
            eprintln!("native-decorators: could not start the live edit refresh ({e}); the decorators keep their pixels until the map reloads");
        }
    }
}

/// The stages whose pass samples image `image` of `bitmap`, ascending, each
/// once.
fn stages_of(looks: &DecoratorLooks, bitmap: TagId, image: u32) -> Vec<usize> {
    let want = Stage::Bitmap { bitmap, image };
    let mut stages: Vec<usize> = looks
        .looks
        .values()
        .filter_map(|look| look.as_ref().ok())
        .flat_map(|look| look.look.textures.iter().enumerate().filter(|(_, s)| **s == want).map(|(n, _)| n))
        .collect();
    stages.sort_unstable();
    stages.dedup();
    stages
}

/// Decodes each of `images` (bitmap, image) again, from the map file the
/// raw overlay serves edited, at the stages the decorator passes sample it
/// (through the palettes the load expanded through), and rewrites the
/// prepared materials' stage textures in place. Materials made later take
/// the new pixels from `looks.edited`. A texture whose size, levels or
/// format changed keeps its pixels until the map reloads, and says so.
fn refresh_images(looks: &DecoratorLooks, key: &crate::native_world_glue::BspKey, queue: &wgpu::Queue, images: &[(TagId, u32)]) {
    let wanted: Vec<_> = images
        .iter()
        .map(|&(bitmap, image)| ((bitmap, image), stages_of(looks, bitmap, image)))
        .filter(|(_, stages)| !stages.is_empty())
        .collect();
    if wanted.is_empty() || !key.current() {
        return;
    }
    let (mut map, mut files) = match crate::native_map::open(key.map()) {
        Ok(opened) => opened,
        Err(e) => {
            eprintln!("native-decorators: a live edit's map does not open ({e}); the decorators keep their pixels");
            return;
        }
    };
    let decoded: Vec<_> = wanted
        .into_iter()
        .map(|((bitmap, image), stages)| {
            let pixels = native_render::stage_images::decode_stages(&mut map, &mut files, &looks.palettes, bitmap, image, &stages);
            ((bitmap, image), pixels)
        })
        .collect();
    if !key.current() {
        return;
    }
    {
        let mut edited = looks.edited.lock().unwrap_or_else(|e| e.into_inner());
        for ((bitmap, image), pixels) in &decoded {
            // One image per (bitmap, image): the lowest stage's decode, as
            // the load's.
            if let Some(first) = pixels.keys().min().and_then(|n| pixels.get(n)) {
                edited.insert((*bitmap, *image), first.clone());
            }
        }
    }
    {
        let materials = looks.gpu.lock().unwrap_or_else(|e| e.into_inner());
        for ((bitmap, image), pixels) in &decoded {
            let held: Vec<_> = materials.values().flat_map(|m| m.held(*bitmap, *image)).collect();
            if held.is_empty() {
                continue;
            }
            if let Err(why) = native_render::gpu::rewrite_held(queue, &held, pixels) {
                eprintln!(
                    "native-decorators: bitmap {:#010x} image {image} keeps its pixels until the map reloads ({why:?})",
                    bitmap.0
                );
            }
        }
    }
    queue.submit([]);
}

impl Host for Glue {
    fn prepare(&self, mem: &dyn GuestMemory, materials: &[Material]) -> HashSet<Material> {
        let mut out = HashSet::new();
        let Some(ready) = self.ready(mem) else { return out };
        let Some(looks) = ready.decorators.get() else { return out };
        let mut p = looks.prepared.lock().unwrap_or_else(|e| e.into_inner());
        self.refresh_edits(mem, looks, &mut p);
        p.begin_frame(timeline::frames(), &looks.gpu);
        let read = ReadMemory(mem);
        let mut queue_full = false;
        for m in materials {
            if let Some(depth) = p.depth.get(m) {
                if depth.is_some() {
                    out.insert(*m);
                }
                continue;
            }
            if queue_full || p.pending.is_some() {
                continue;
            }
            if !matches!(looks.look(m), Some(Ok(_))) {
                p.depth.insert(*m, None);
                continue;
            }
            let Some((device, queue)) = ready.upload_device.as_ref() else { continue };
            let _handoff = timeline::setup::dispatch();
            let Some(worker) = self.worker.get_or_init(material_worker) else { continue };
            let _capture = timeline::native::Timer::start(timeline::native::Stage::World, timeline::native::Metric::ExtractNs);
            let set = match set_geometry_with(&read, m.set, |at, len| {
                mem.bytes(at, len as usize).map_err(|_| SetRefusal::Unread)
            }) {
                Ok(set) => set,
                Err(SetRefusal::NotResident | SetRefusal::Unread) => continue,
                Err(why) => {
                    log_once(&format!("set geometry: {why:?}"));
                    p.depth.insert(*m, None);
                    continue;
                }
            };
            let stream = StreamFormats::read(&read)
                .ok_or_else(|| "the stream format table does not read".to_owned())
                .and_then(|f| f.elements(set.stream_type));
            let stream = match stream {
                Ok(stream) => stream,
                Err(why) => {
                    log_once(&why);
                    p.depth.insert(*m, None);
                    continue;
                }
            };
            let (reply, result) = mpsc::channel();
            let job = MaterialJob {
                material: *m, set, stream, looks: Arc::clone(looks),
                device: device.clone(), queue: queue.clone(),
                generation: render_bridge::native_map_work::bsp_generation(), reply,
            };
            match worker.try_send(job) {
                Ok(()) => p.pending = Some((*m, result)),
                Err(mpsc::TrySendError::Full(_)) => queue_full = true,
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    p.depth.insert(*m, None);
                }
            }
        }
        out
    }

    fn holds(&self, mem: &dyn GuestMemory, ready: &HashSet<Material>) -> HashSet<Material> {
        let mut out = HashSet::new();
        if ready.is_empty() {
            return out;
        }
        let Some(load) = self.ready(mem) else { return out };
        let Some(looks) = load.decorators.get() else { return out };
        let device_state = |slot: u32| crate::native_objects_glue::device_state::render_state(mem, slot);
        for m in ready {
            if matches!(looks.look(m), Some(Ok(look)) if skip_holds(look, &device_state)) {
                out.insert(*m);
            }
        }
        out
    }

    fn ask(&self, mem: &mut dyn GuestMemory, plan: Plan) -> Option<Cleared> {
        use timeline::native::{Metric, Stage};
        let ready = self.ready(mem)?;
        let looks = ready.decorators.get()?;
        {
            // Every batch's material is prepared.
            let p = looks.prepared.lock().ok()?;
            for b in &plan.batches {
                p.depth.get(&b.material).copied().flatten()?;
            }
        }
        let inherits_cull = plan.batches.iter().any(|b| matches!(looks.look(&b.material), Some(Ok(l)) if l.inherits_cull));
        let read = ReadMemory(mem);
        let draw = layer_draw(&*mem, inherits_cull)?;

        // The skip over a preview: nothing touches the guest until the
        // reservation has recorded on every copy.
        let planning = timeline::native::Timer::start(Stage::World, Metric::PrepareNs);
        let mut preview = native_render::skip::MaskPreview::new(&read)?;
        let cleared = clear_slots(&mut preview, LIGHTMAP_LAYER, &plan.slots)?;
        let masks = preview.finish()?;
        let ranges = plan.slots.len();
        if ranges == 0 || cleared.items.len() != ranges {
            return None;
        }
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let token = crate::native_world_glue::view_token(crate::presents::presents() + 1, view);
        let pass = Box::new(DecoratorPass { plan, ready: Arc::clone(&ready), draw, token });
        drop(planning);
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &ready)) {
            return None;
        }
        if !masks.apply(&mut render_bridge::native_decorators::Mem(mem)) {
            Stage::World.count(Metric::RefusedMasks, ranges as u64);
            return None;
        }
        native_render::stage::frame_builder().push(native_render::stage::FrameOp::Stage {
            slot: frame_types::SlotAt { slot: frame_types::Slot::Opaque, view: view as u8 },
            stage: pass,
        });
        Some(cleared)
    }
}

/// The stencil and cull the placements draw under at the layer-3 call
/// (`0x0004e260` sets neither), from the guest's render-state shadow
/// (`native_targets_glue::stencil_state`, `cull_state`); the cull only
/// when a pass inherits it. `None` when the stencil does not read.
fn layer_draw(mem: &dyn GuestMemory, inherits_cull: bool) -> Option<DrawKey> {
    let stencil = crate::native_targets_glue::stencil_state(mem)?;
    let cull = inherits_cull
        .then(|| crate::native_targets_glue::cull_state(mem).map(crate::native_world_glue::inherited_cull))
        .flatten();
    Some(DrawKey { stencil, cull })
}

/// The display's projection factor, the lens the GPU culls by.
fn display_factor() -> f32 {
    crate::native_world_glue::projection_factor()
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install(state: State) {
    render_bridge::native_decorators::set_host(Arc::new(Glue {
        state,
        worker: OnceLock::new(),
        refreshing: Arc::default(),
    }));
}


/// One view's decorators.
struct DecoratorPass {
    plan: Plan,
    /// Pin the BSP until the last queued packet releases its GPU materials.
    ready: Arc<Ready>,
    /// The stencil and cull at the layer-3 call.
    draw: DrawKey,
    /// This view in this frame (`native_world_glue::view_token`).
    token: u64,
}

impl DecoratorPass {
    /// Binds the plan's batches on `t`'s device, and the commands that
    /// draw them over the whole bound attachments.
    fn encoded(&self, t: &ExternalAttachments, view: &ViewInputs) -> Result<ExternalEncoded, &'static str> {
        let looks = self.ready.decorators.get().ok_or("decorator load unavailable")?;
        let mut gpu = looks.gpu.lock().unwrap_or_else(|e| e.into_inner());
        for batch in &self.plan.batches {
            let material = gpu.get_mut(&batch.material).ok_or("decorator material unavailable")?;
            if !material.pipeline_ready(&t.device, t.color_format, t.depth_format, self.draw) {
                return Err("decorator pipeline pending");
            }
        }
        let bound = bind(&t.device, &mut gpu, &self.plan, view, t.color_format, t.depth_format, self.draw)?;
        drop(gpu);
        let viewport = [0.0, 0.0, t.size.0 as f32, t.size.1 as f32];
        // Each instance by its group's sphere, on the GPU; with no camera
        // or no GPU world every one draws.
        let culled = self.plan.camera.and_then(|camera| {
            let mut world = self.ready.gpu();
            let world = world.as_mut()?;
            let [x, y, width, height] = viewport;
            let lens = native_render::camera::display_camera(camera, display_factor());
            let occlusion = Some((t.depth.texture(), &lens, PassViewport { x, y, width, height }, self.token));
            let occludable: Vec<bool> = bound.iter().map(|b| b.occludable).collect();
            Some(world.culled_decorators(
                &t.device, &t.queue, &t.uploads, &self.plan, &occludable, &camera, view, t.size, occlusion,
            ))
        });
        let (device, queue, uploads) = (t.device.clone(), t.queue.clone(), t.uploads.clone());
        let (color, depth) = (t.color.clone(), t.depth.clone());
        let stencil = t.depth_format.has_stencil_aspect();
        let stencil_reference = u32::from(self.draw.stencil.reference);
        let size = t.size;
        let plan = self.plan.clone();
        let ranges = plan.slots.len();
        let draws = plan.batches.len();
        Ok(ExternalEncoded {
            ranges,
            draws,
            passes: 1,
            commands: Box::new(move |encoder| {
                let target = DecoratorTarget { color: &color, depth: &depth, stencil, stencil_reference, size, viewport };
                if let Err(why) = encode(&device, &queue, &uploads, encoder, &target, &plan, &bound, culled.as_ref()) {
                    panic!("decorators: encode: {why}");
                }
            }),
        })
    }
}

impl native_render::stage::Stage for DecoratorPass {
    fn name(&self) -> &'static str {
        "decorators"
    }

    fn record(&mut self, t: &ExternalAttachments, view: &ViewInputs) -> StageWork {
        match self.encoded(t, view) {
            Ok(encoded) => StageWork::Encoded(encoded),
            Err(why) => panic!("decorators: {why}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_views_of_a_frame_see_the_same_material_completion() {
        let material = Material { set: 1, shader: 2, variant: 0, pass: 0, layer: DECORATOR_LAYER };
        let (send, receive) = mpsc::channel();
        let mut load = Prepared { pending: Some((material, receive)), ..Prepared::default() };
        let gpu = Mutex::default();
        load.begin_frame(4, &gpu);
        assert!(send.send(Err("synthetic refusal".into())).is_ok());
        load.begin_frame(4, &gpu);
        assert!(load.pending.is_some());
        assert!(!load.depth.contains_key(&material));
        let previous_frame = gpu.lock().unwrap();
        load.begin_frame(5, &gpu);
        drop(previous_frame);
        assert!(load.pending.is_some());
        load.begin_frame(6, &gpu);
        assert!(load.pending.is_none());
        assert_eq!(load.depth.get(&material), Some(&None));
    }
}
