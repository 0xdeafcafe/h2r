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
//!   memory (`set_geometry`, `object_streams::StreamFormats`), the pass
//!   translated for that stream (`compile`) and the vertices gathered as
//!   it reads them (`buffers`). A set that is not resident is tried
//!   again at the next view; any other refusal holds for the load and is
//!   logged once.
//! - **Per view** ([`Host::ask`]): the device's checks (W-buffered, no
//!   stencil), the skip over a preview (`clear_slots`), and the reserved
//!   pass, then the masks and the commit, as the fog's.
//! - **The pass** (`DecoratorPass`): the device's view constants and W
//!   clip at the issue, each material uploaded on the device at its first
//!   draw (`MaterialGpu`, kept for the load), then `decorators_gpu::bind`
//!   and `encode`.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use map::{FourCC, Map, TagId};
use native_render::decorators::{Material, Plan, clear_slots};
use native_render::decorators_gpu::{DecoratorTarget, MaterialGpu, bind, encode};
use native_render::decorators_load::{
    DECORATOR_CLASSIFICATION, DecoratorBuffers, DecoratorLook, DecoratorProgram, SetRefusal, buffers, compile,
    decorator_look, set_geometry,
};
use native_render::gpu::Image;
use native_render::look::{self, Stage};
use native_render::object_streams::StreamFormats;
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
pub(crate) fn load<R: Read + Seek>(
    map: &mut Map<R>, files: &mut assets::ResourceFiles, decoder: &mut StageDecoder,
) -> Arc<DecoratorLooks> {
    let mut looks = HashMap::new();
    let mut images = StageImages::new();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("native-decorators: the shader tags do not read ({e}); the D3D path draws the decorators");
            return Arc::new(DecoratorLooks { looks, images });
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
    Arc::new(DecoratorLooks { looks, images })
}

/// One material made ready: its program for the set's stream, the set's
/// vertices as it reads them, and its stage images.
struct LoadedMaterial {
    program: DecoratorProgram,
    buffers: DecoratorBuffers,
    images: [Option<Image>; 4],
}

/// What [`Glue::prepare`] made ready for one load, and the materials the
/// pass uploaded on the device (kept for the load).
struct Prepared {
    ready: Arc<Ready>,
    loaded: HashMap<Material, Result<Arc<LoadedMaterial>, String>>,
    gpu: Arc<Mutex<HashMap<Material, MaterialGpu>>>,
}

/// Makes `material` ready, or why it is not: `Ok(None)` when the set is
/// not resident (tried again at the next view).
fn make_ready(
    looks: &DecoratorLooks, mem: &ReadMemory<'_>, formats: &mut Option<Option<StreamFormats>>, material: &Material,
) -> Result<Option<LoadedMaterial>, String> {
    let look = match looks.look(material) {
        Some(Ok(look)) => look,
        Some(Err(why)) => return Err(why.clone()),
        None => return Err("the load has no layer-3 pass for the shader".into()),
    };
    let set = match set_geometry(mem, material.set) {
        Ok(set) => set,
        Err(SetRefusal::NotResident | SetRefusal::Unread) => return Ok(None),
        Err(why) => return Err(format!("set geometry: {why:?}")),
    };
    let formats = formats
        .get_or_insert_with(|| StreamFormats::read(mem))
        .as_ref()
        .ok_or("the stream format table does not read")?;
    let program = compile(look, &formats.elements(set.stream_type)?)?;
    let buffers = buffers(&program, &set)?;
    let mut images: [Option<Image>; 4] = Default::default();
    for (n, dim) in program.dims.iter().enumerate() {
        if dim.is_none() {
            continue;
        }
        let Stage::Bitmap { bitmap, image } = program.stages[n] else {
            return Err(format!("stage {n} samples what is not a bitmap"));
        };
        images[n] = Some(looks.images.get(&(bitmap, image)).cloned().ok_or_else(|| format!("stage {n} bitmap did not load"))?);
    }
    Ok(Some(LoadedMaterial { program, buffers, images }))
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
    prepared: Mutex<Option<Prepared>>,
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
        let Some(looks) = ready.decorators.clone() else { return out };
        let mut prepared = self.prepared.lock().unwrap_or_else(|e| e.into_inner());
        if !prepared.as_ref().is_some_and(|p| Arc::ptr_eq(&p.ready, &ready)) {
            *prepared = Some(Prepared { ready: Arc::clone(&ready), loaded: HashMap::new(), gpu: Arc::default() });
        }
        let Some(p) = prepared.as_mut() else { return out };
        let read = ReadMemory(mem);
        let mut formats = None;
        for m in materials {
            if !p.loaded.contains_key(m) {
                match make_ready(&looks, &read, &mut formats, m) {
                    Ok(Some(loaded)) => {
                        p.loaded.insert(*m, Ok(Arc::new(loaded)));
                    }
                    Ok(None) => continue,
                    Err(why) => {
                        log_once(&why);
                        p.loaded.insert(*m, Err(why));
                    }
                }
            }
            if p.loaded.get(m).is_some_and(|l| l.is_ok()) {
                out.insert(*m);
            }
        }
        out
    }

    fn ask(&self, mem: &mut dyn GuestMemory, plan: Plan) -> Option<Cleared> {
        use timeline::native::{Metric, Stage};
        let ready = self.ready(mem)?;
        let (loaded, gpu) = {
            let prepared = self.prepared.lock().unwrap_or_else(|e| e.into_inner());
            let p = prepared.as_ref().filter(|p| Arc::ptr_eq(&p.ready, &ready))?;
            let mut loaded = HashMap::new();
            for b in &plan.batches {
                let m = p.loaded.get(&b.material)?.as_ref().ok()?;
                loaded.insert(b.material, Arc::clone(m));
            }
            (loaded, Arc::clone(&p.gpu))
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
        let writes_depth = loaded.values().any(|m| m.program.state.depth_write);
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let external = d3d::ExternalPlan { stage: Stage::World, view, ranges, slots: Vec::new() };
        let pass = Box::new(DecoratorPass { plan, loaded, gpu, view: None, w_clip: None, writes_depth });
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
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    render_bridge::native_decorators::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
        prepared: Mutex::new(None),
    }));
}

static STENCIL: AtomicBool = AtomicBool::new(false);
static NO_VIEW: AtomicBool = AtomicBool::new(false);
static REFUSED: AtomicBool = AtomicBool::new(false);

/// One view's decorators.
struct DecoratorPass {
    plan: Plan,
    /// The plan's materials, made ready at the ask.
    loaded: HashMap<Material, Arc<LoadedMaterial>>,
    /// The load's materials on the device.
    gpu: Arc<Mutex<HashMap<Material, MaterialGpu>>>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: Option<([f32; 2], bool)>,
    writes_depth: bool,
}

impl DecoratorPass {
    /// Uploads each of the plan's materials not yet on the device.
    fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(), &'static str> {
        let mut gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
        for (m, loaded) in &self.loaded {
            if gpu.contains_key(m) {
                continue;
            }
            let material = MaterialGpu::new(device, queue, loaded.program.clone(), &loaded.buffers, &loaded.images)?;
            gpu.insert(*m, material);
        }
        Ok(())
    }
}

impl DecoratorPass {
    /// Binds the plan's batches on `t`'s device and records them: the
    /// number of guest draws recorded.
    fn record(&self, mut t: d3d::ExternalTarget<'_>, view: &ViewInputs) -> Result<usize, &'static str> {
        self.upload(t.device, t.queue)?;
        let mut gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
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
        self.upload(t.device, t.queue)
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
        let view = ViewInputs { constants, guest_size: t.guest_size, size: t.size, z_scale: t.z_scale, w_clip: self.w_clip };
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
