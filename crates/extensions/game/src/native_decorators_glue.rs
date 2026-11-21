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
//!   bitmaps decoded. A palettized stage bitmap refuses the pass: the
//!   stage expands no palette. One line says how many passes load and
//!   why the rest stay on the D3D path.
//! - **At first sight** ([`Host::prepare`], at the layer-3 call, once per
//!   load and material): the set's geometry and stream format from guest
//!   memory (`set_geometry_with`, `object_streams::StreamFormats`). A
//!   utility worker translates the pass, gathers the set's vertices and
//!   uploads its buffers and images. The first view of each frame takes
//!   finished materials from this load. A set that is not resident is
//!   tried again at the next view.
//! - **Per view** ([`Host::ask`]): the device's checks (W-buffered, no
//!   stencil), the skip over a preview (`clear_slots`), and the reserved
//!   pass, then the masks and the commit, as the fog's.
//! - **The pass** (`DecoratorPass`): the device's view constants and W
//!   clip at the issue and the pass's pipeline for each copy's formats.
//!   An unfinished pipeline keeps the slots on D3D. `decorators_gpu::bind`
//!   and `encode` record after all copies pass.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use engine::memory::GuestMemory;
use map::{FourCC, Map, TagId};
use native_render::decorators::{Material, Plan, clear_slots};
use native_render::decorators_gpu::{DecoratorTarget, MaterialGpu, bind, encode};
use native_render::decorators_load::{
    DECORATOR_CLASSIFICATION, DecoratorLook, SetGeometry, SetRefusal, buffers, compile, decorator_look, set_geometry_with,
};
use native_render::gpu::Image;
use native_render::look::{self, Stage};
use native_render::object_streams::{StreamElement, StreamFormats};
use native_render::program::ViewInputs;
use native_render::skip::{Cleared, LIGHTMAP_LAYER};
use render_bridge::native_decorators::Host;

use crate::native_world_glue::{ReadMemory, Ready, StageDecoder, StageImages, State, current_ready};

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
    prepared: Mutex<Prepared>,
    gpu: Mutex<HashMap<Material, MaterialGpu>>,
}

impl DecoratorLooks {
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

/// Whether image `image` of `bitmap` is palettized (the stage would
/// expand it through the device's palette).
fn palettized<R: Read + Seek>(map: &mut Map<R>, bitmap: TagId, image: u32) -> Result<bool, String> {
    let tag = tags::DecodedTag::read(map, bitmap).map_err(|e| format!("bitmap does not read ({e})"))?;
    let tags::TagData::Bitmap(b) = tag.data else { return Err("stage datum is not a bitmap".into()) };
    let i = b.images.get(image as usize).ok_or("bitmap has no such image")?;
    Ok(matches!(i.format, tags::PixelFormat::P8 | tags::PixelFormat::P8Bump))
}

/// Reads every layer-3 pass of the shaders the map's decorator sets name.
/// Called from the world's worker.
pub fn load<R: Read + Seek>(
    map: &mut Map<R>, files: &mut assets::ResourceFiles, decoder: &mut StageDecoder,
) -> Arc<DecoratorLooks> {
    let mut looks = HashMap::new();
    let mut images = StageImages::new();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("native-decorators: the shader tags do not read ({e}); the D3D path draws the decorators");
            return Arc::new(DecoratorLooks { looks, images, prepared: Mutex::default(), gpu: Mutex::default() });
        }
    };
    let shaders = set_shaders(map, &lib);
    let mut refused: HashMap<String, usize> = HashMap::new();
    let mut palettes: HashMap<(TagId, u32), Result<bool, String>> = HashMap::new();
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
                        let p = palettes
                            .entry((bitmap, image))
                            .or_insert_with(|| palettized(map, bitmap, image))
                            .clone();
                        if p.map_err(|why| format!("stage {n} {why}"))? {
                            return Err(format!("stage {n} bitmap is palettized"));
                        }
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
    let mut why: Vec<_> = refused.into_iter().collect();
    why.sort();
    eprintln!(
        "native-decorators: {} decorator shaders, {} of {} layer-3 passes load; on the D3D path: {why:?}",
        shaders.len(),
        looks.values().filter(|l| l.is_ok()).count(),
        looks.len(),
    );
    Arc::new(DecoratorLooks { looks, images, prepared: Mutex::default(), gpu: Mutex::default() })
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
}

impl Prepared {
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
        let Some(Ok(look)) = self.looks.look(&self.material) else {
            return Err("the load has no supported decorator pass for the shader".into());
        };
        let program = compile(look, &self.stream)?;
        let buffers = buffers(&program, &self.set)?;
        let mut images: [Option<Image>; 4] = Default::default();
        for (n, dim) in program.dims.iter().enumerate() {
            if dim.is_none() {
                continue;
            }
            let Stage::Bitmap { bitmap, image } = program.stages[n] else {
                return Err(format!("stage {n} samples what is not a bitmap"));
            };
            images[n] = Some(self.looks.images.get(&(bitmap, image)).cloned().ok_or_else(|| format!("stage {n} bitmap did not load"))?);
        }
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
        eprintln!("native-decorators: a material stays on the D3D path: {why}");
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
    worker: OnceLock<Option<mpsc::SyncSender<MaterialJob>>>,
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    fn ready(&self, mem: &dyn GuestMemory) -> Option<Arc<Ready>> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        current_ready(&state, mem).cloned()
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn prepare(&self, mem: &dyn GuestMemory, materials: &[Material]) -> HashSet<Material> {
        let mut out = HashSet::new();
        let Some(ready) = self.ready(mem) else { return out };
        let Some(looks) = ready.decorators.get() else { return out };
        let mut p = looks.prepared.lock().unwrap_or_else(|e| e.into_inner());
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

    fn ask(&self, mem: &mut dyn GuestMemory, plan: Plan) -> Option<Cleared> {
        use timeline::native::{Metric, Stage};
        let ready = self.ready(mem)?;
        let writes_depth = {
            let looks = ready.decorators.get()?;
            let p = looks.prepared.lock().ok()?;
            let mut writes_depth = false;
            for b in &plan.batches {
                writes_depth |= p.depth.get(&b.material).copied().flatten()?;
            }
            writes_depth
        };
        let mut d = self.device.lock().ok()?;
        if !d.can_external_pass()
            || !d.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)).ok()?
        {
            return None;
        }
        // The programs write w as depth (`w_buffer`): a view whose draws
        // store z is the D3D path's. The pipelines test no stencil: a
        // layer drawn with it on is the D3D path's.
        if d.hw().control0.is_some_and(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE == 0) {
            return None;
        }
        let read = ReadMemory(mem);
        if !d.stencil_state(&read).is_some_and(|s| !s.enable) {
            if !STENCIL.swap(true, Ordering::Relaxed) {
                eprintln!("native-decorators: layer 3 draws with the stencil on or unread; the D3D path draws the decorators");
            }
            return None;
        }
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
        let external = d3d::ExternalPlan { stage: Stage::World, view, ranges, slots: Vec::new() };
        let pass = Box::new(DecoratorPass { plan, ready: Arc::clone(&ready), view: None, w_clip: None, writes_depth });
        drop(planning);
        let reservation = d
            .prepare_external_bound(&mut crate::native_lights_glue::DeviceMemory(mem), external, pass, Vec::new())
            .ok()??;
        // Keep this load pinned until the masks and the sealed commands
        // commit.
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &ready)) {
            return None;
        }
        if !masks.apply(&mut render_bridge::native_decorators::Mem(mem)) {
            Stage::World.count(Metric::RefusedMasks, ranges as u64);
            return None;
        }
        reservation.commit();
        Some(cleared)
    }
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    render_bridge::native_decorators::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
        worker: OnceLock::new(),
    }));
}

static STENCIL: AtomicBool = AtomicBool::new(false);
static NO_VIEW: AtomicBool = AtomicBool::new(false);
static REFUSED: AtomicBool = AtomicBool::new(false);

/// One view's decorators.
struct DecoratorPass {
    plan: Plan,
    /// Pin the BSP until the last queued packet releases its GPU materials.
    ready: Arc<Ready>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: Option<([f32; 2], bool)>,
    writes_depth: bool,
}

impl DecoratorPass {
    /// Binds the plan's batches on `t`'s device and records them: the
    /// number of guest draws recorded.
    fn record(&self, mut t: d3d::ExternalTarget<'_>, view: &ViewInputs) -> Result<usize, &'static str> {
        let looks = self.ready.decorators.get().ok_or("decorator load unavailable")?;
        let mut gpu = looks.gpu.lock().unwrap_or_else(|e| e.into_inner());
        let bound = bind(t.device, &mut gpu, &self.plan, view, t.color_format, t.depth_format)?;
        drop(gpu);
        let target = DecoratorTarget {
            color: t.color,
            depth: t.depth,
            stencil: t.depth_format.has_stencil_aspect(),
            size: t.size,
            viewport: t.viewport,
        };
        encode(t.device, t.queue, t.uploads, &mut *t.encoder, &target, &self.plan, &bound)
    }
}

impl d3d::ExternalPass for DecoratorPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = (clip.w_buffered == Some(true)).then_some((clip.range, clip.clamp));
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.view.is_none() || self.w_clip.is_none() {
            return Err("decorator view constants or W-buffer clip unavailable");
        }
        let looks = self.ready.decorators.get().ok_or("decorator load unavailable")?;
        let mut gpu = looks.gpu.lock().map_err(|_| "decorator materials unavailable")?;
        let mut ready = true;
        for batch in &self.plan.batches {
            let material = gpu.get_mut(&batch.material).ok_or("decorator material unavailable")?;
            ready &= material.pipeline_ready(t.device, t.color_format, t.depth_format);
        }
        ready.then_some(()).ok_or("decorator pipeline pending")
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: self.writes_depth }
    }

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let Some(constants) = self.view else {
            if !NO_VIEW.swap(true, Ordering::Relaxed) {
                eprintln!("native-decorators: a pass came without the device's view constants; it records nothing");
            }
            return 0;
        };
        let view = ViewInputs { constants: t.view_constants(constants), guest_size: t.guest_size, size: t.size, z_scale: t.z_scale, w_clip: self.w_clip };
        match self.record(t, &view) {
            Ok(n) => n,
            Err(why) => {
                if !REFUSED.swap(true, Ordering::Relaxed) {
                    eprintln!("native-decorators: a pass records nothing: {why}");
                }
                0
            }
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
